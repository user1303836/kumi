#!/usr/bin/env python3
"""Build native Kumi bundles and receipt-bound bridge artifacts (Python 3.11+)."""
from __future__ import annotations
import argparse
import datetime
import gzip
import hashlib
import json
import lzma
import os
from pathlib import Path
import platform
import posixpath
import re
import shutil
import struct
import subprocess
import tarfile
import tempfile
import tomllib
from urllib.parse import quote

ROOT = Path(__file__).resolve().parents[1]
SCHEMA = "ableton-mcp-native-release/v1"
BRIDGE_BINARIES = ("ableton-mcp-server", "ableton-mcp-analysis-worker")
# daw-mcp-server is Kumi's host for the other DAWs: Kumi starts it from beside its own binary.
BINARIES = ("kumi", *BRIDGE_BINARIES, "daw-mcp-server", "kumi-harness", "kumi-library-measure", "kumi-library-learner")
# The other DAWs' bridges, by the DAW's id (scripts/daws.toml, which release.py and build-migration-release.py read
# through here). Each is in daws/<id> in the bundle, where Kumi installs it from (daws/own/kumi-daw-own/src/provider.rs):
# a library with its version, its crate's, in a file beside it, or a Renoise tool's folder.
DAWS = "daws"
DAW_BRIDGES = {daw["id"]: daw for daw in tomllib.loads((ROOT / "scripts/daws.toml").read_text(encoding="utf-8"))["daw"]}
# What Kumi's installer or the DAW writes into a bridge's folder: never shipped (daw-bridge's NOT_PAYLOAD).
NOT_PAYLOAD = ("bridge-reference.json", "preferences.xml")
# A Renoise tool's own files.
TOOL_SUFFIXES = (".lua", ".xml", ".json")
# The newest glibc a Linux extension may need: what Kumi's own Linux binaries need, built on the same Ubuntu 22.04
# runners. A newer runner would quietly raise it, and a DAW on an older Linux would then refuse the extension.
GLIBC_MOST = (2, 35)
# The C runtime as Windows DLLs: an extension links it in (+crt-static), so a DAW without the Visual C++ runtime
# installed loads it.
WINDOWS_CRT = re.compile(r"(vcruntime\d+.*|msvcp\d+.*|msvcr\d+.*|ucrtbase.*|api-ms-win-crt-.*)\.dll", re.I)
DOCUMENTS = [("daws/ableton/ableton-mcp-server/README.md", "README.md")] + [(f"docs/en/{name}.md", f"{name}.md") for name in (
    "USER_GUIDE", "USER_JOURNEYS", "OPERATIONS", "RECOVERY", "LIVE_SAFETY", "AUDIO_INTELLIGENCE",
    "REALTIME_CONTROL", "DELIVERY", "DEVELOPER_GUIDE", "TESTING", "IMPLEMENTATION_STATUS",
    "DISTRIBUTION_POLICY", "SUPPORT_MATRIX", "CAPABILITY_MATRIX")]
EXCLUSIONS = ["tests", "verification-scripts", "source-maps", "credentials", "configuration", "local-state", "logs",
              "backups", "captured-media", "generated-evidence", "dependency-trees", "protected-local-material"]
MIT_SHA256 = "f6a4bf820a492313c9d4e100e16bd474cd5cf06c0fba27c1035238acb4af75cb"
# Willington's runtime files, as an update puts them in vendor/willington with a release.json naming each
# file's SHA-256. Only these may ship: its C++ sources, headers and debug files stay in its own repository.
WILLINGTON = "vendor/willington"
WILLINGTON_SCHEMA = "kumi-willington-vendor/v1"
WILLINGTON_COMPONENTS = ("WillingtonRuntime", "WillingtonBindings", "WillingtonDeviceTools", "WillingtonRackZones", "WillingtonEditing")
WILLINGTON_SUFFIXES = (".py", ".json", ".md", ".pyd", ".dylib")
WILLINGTON_LICENSES = ("LICENSE", "LICENSE.md")
WILLINGTON_MAX_BYTES = 16 * 1024 * 1024
# The native libraries for each platform Live runs on: a bundle carries only its own platform's.
WILLINGTON_NATIVES = {"windows": ".pyd", "macos": ".dylib"}
# Names Windows keeps for devices, with or without an extension.
WINDOWS_DEVICE = re.compile(r"(con|prn|aux|nul|com[0-9]|lpt[0-9])(\..*)?", re.I)

def digest(path: Path) -> str:
    with path.open("rb") as file:
        return hashlib.file_digest(file, "sha256").hexdigest()

def run(args: list[str], root: Path = ROOT) -> str:
    return subprocess.check_output(args, cwd=root, text=True).strip()

def json_write(path: Path, value: object, pretty=True) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(value, ensure_ascii=False, indent=2 if pretty else None,
                               separators=None if pretty else (",", ":")) + "\n", encoding="utf-8")

def copy(source: Path, target: Path) -> None:
    if source.is_symlink() or not source.is_file():
        raise ValueError(f"payload must be a regular file: {source}")
    target.parent.mkdir(parents=True, exist_ok=True)
    shutil.copyfile(source, target)
    target.chmod(0o644)

def inventory(folder: Path) -> dict[str, str]:
    result = {}
    for path in sorted(folder.rglob("*")):
        if path.is_symlink() or not (path.is_file() or path.is_dir()):
            raise ValueError(f"payload contains a link or special file: {path}")
        if path.is_file():
            result[path.relative_to(folder).as_posix()] = digest(path)
    return result

def role(name: str) -> str:
    if name in BRIDGE_BINARIES or name.removesuffix(".exe") in BRIDGE_BINARIES:
        return "native-runtime"
    if name == "LICENSE.md":
        return "license"
    if name == "package.json":
        return "package-metadata"
    if name == "README.md" or (name.startswith("release-docs/") and name.endswith(".md")):
        return "documentation"
    if name.startswith("remote-script/"):
        return "ableton-remote-script"
    if name.startswith("live-extension/"):
        return "ableton-live-extension"
    raise ValueError(f"unrecognized payload role: {name}")

def source_evidence(root: Path) -> dict:
    commit = run(["git", "rev-parse", "HEAD"], root)
    if not re.fullmatch("[a-f0-9]{40}", commit):
        raise ValueError("release staging requires an exact Git revision")
    return {"commit": commit, "commitTimestamp": run(["git", "show", "-s", "--format=%cI", "HEAD"], root),
            "dirty": bool(run(["git", "status", "--porcelain"], root))}

def external(target: str) -> bool:
    if re.match(r"^[a-z]:[\\/]", target, re.I):
        return False
    return not target or target.startswith(("#", "//")) or bool(re.match(r"^[a-z][a-z0-9+.-]*:", target, re.I))

def document_target(root: Path, source: str, target: str, revision: str, kind: str) -> str:
    bracketed = target.startswith("<") and target.endswith(">")
    target = target[1:-1] if bracketed else target
    if external(target):
        result = target
    else:
        split = re.search(r"[?#]", target)
        path, suffix = (target[:split.start()], target[split.start():]) if split else (target, "")
        if path.startswith("/") or re.match(r"^[a-z]:", path, re.I) or "\\" in path or "\0" in path:
            raise ValueError(f"unsafe documentation target: {source} -> {target}")
        normalized = posixpath.normpath(posixpath.join(posixpath.dirname(source), path))
        if path.endswith("/") and normalized != ".":
            normalized += "/"
        if normalized == ".." or normalized.startswith("../"):
            raise ValueError(f"documentation target escapes repository: {source} -> {target}")
        mapped = dict(DOCUMENTS).get(normalized)
        relative = {"LICENSE.md": "../LICENSE.md",
                    "daws/ableton/protocol/ableton-live-v1.operations.json": "../remote-script/AbletonMcpBridge/ableton-live-v1.operations.json"}
        if mapped or normalized in relative:
            result = (mapped or relative[normalized]) + suffix
        else:
            absolute = root / normalized
            if not absolute.exists():
                raise ValueError(f"documentation target missing: {source} -> {target}")
            encoded = "/".join(quote(part, safe="~!*'()") for part in normalized.split("/"))
            if kind == "src" and absolute.is_file():
                result = f"https://raw.githubusercontent.com/user1303836/kumi/{revision}/{encoded}{suffix}"
            else:
                result = f"https://github.com/user1303836/kumi/{'tree' if absolute.is_dir() else 'blob'}/{revision}/{encoded}{suffix}"
    return f"<{result}>" if bracketed else result

def transform_document(text: str, root: Path, source: str, revision: str) -> str:
    def markdown(match):
        prefix, target, suffix = match.groups()
        return prefix + document_target(root, source, target, revision, "src" if prefix.startswith("!") else "href") + suffix
    text = re.sub(r"(!?\[[^\]]*\]\(\s*)(<[^>]+>|[^)\s]+)([^)]*\))", markdown, text)
    def html(match):
        prefix, quote_char, target = match.groups()
        return prefix + quote_char + document_target(root, source, target, revision, "src" if prefix.strip().lower().startswith("src") else "href") + quote_char
    text = re.sub(r"(\b(?:href|src)\s*=\s*)([\"'])([^\"']+)\2", html, text, flags=re.I)
    return re.sub(r"^(\s*\[[^\]]+\]:\s*)(<[^>]+>|\S+)(.*)$", markdown, text, flags=re.M)

def portable(name: str) -> bool:
    """A path every platform's checkout can hold: plain ASCII segments, none a Windows device name or ending
    in a dot."""
    return all(re.fullmatch(r"[A-Za-z0-9._-]+", part) and not part.endswith(".") and not WINDOWS_DEVICE.fullmatch(part)
               for part in name.split("/"))

def willington_platform(target: str) -> str | None:
    """The platform a Rust target's bundle puts the bridge in Live on; None where Live doesn't run."""
    return "windows" if "-windows-" in target else "macos" if target.endswith("-apple-darwin") else None

def willington_for(files: dict[str, str], platform: str) -> list[str]:
    """What a bundle for the platform carries: every file but other platforms' native libraries."""
    others = tuple(suffix for name, suffix in WILLINGTON_NATIVES.items() if name != platform)
    return [name for name in files if not name.endswith(others)]

def willington_files(folder: Path) -> dict[str, str] | None:
    """The vendored Willington files and their SHA-256, each checked against release.json; None when there are none.

    Any file release.json doesn't list, or that it lists but isn't one of Willington's runtime files (a
    source, a header, a debug file), refuses the release.
    """
    if not folder.exists():
        return None
    manifest_path = folder / "release.json"
    if folder.is_symlink() or not folder.is_dir() or manifest_path.is_symlink() or not manifest_path.is_file():
        raise ValueError(f"{WILLINGTON} must be a folder with its release.json")
    manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
    if not isinstance(manifest, dict) or set(manifest) != {"schema", "version", "commit", "files"} or manifest["schema"] != WILLINGTON_SCHEMA:
        raise ValueError(f"{WILLINGTON}/release.json isn't a {WILLINGTON_SCHEMA} manifest")
    version, commit, listed = manifest["version"], manifest["commit"], manifest["files"]
    if not isinstance(version, str) or not re.fullmatch(r"[0-9A-Za-z][0-9A-Za-z.+-]{0,63}", version):
        raise ValueError(f"{WILLINGTON}/release.json has an invalid version")
    if not isinstance(commit, str) or not re.fullmatch("[a-f0-9]{40}", commit):
        raise ValueError(f"{WILLINGTON}/release.json has an invalid commit")
    if not isinstance(listed, dict) or not listed:
        raise ValueError(f"{WILLINGTON}/release.json lists no files")
    # Checked here, on the update's pull request, rather than when a release is built on the system that can't.
    unportable = sorted(name for name in listed if not portable(name))
    if unportable:
        raise ValueError(f"{WILLINGTON}/release.json names files some systems can't hold: {', '.join(unportable)}")
    if len({name.lower() for name in listed}) != len(listed):
        raise ValueError(f"{WILLINGTON}/release.json names files that differ only in case")
    present = inventory(folder)
    del present["release.json"]
    if set(present) != set(listed):
        raise ValueError(f"{WILLINGTON} differs from its release.json: {', '.join(sorted(set(present) ^ set(listed)))}")
    for name, expected in listed.items():
        parts = name.split("/")
        runtime = parts[0] in WILLINGTON_COMPONENTS and len(parts) > 1 and name.endswith(WILLINGTON_SUFFIXES) and "__pycache__" not in parts
        if not runtime and name not in WILLINGTON_LICENSES:
            raise ValueError(f"{WILLINGTON}/{name} isn't one of Willington's runtime files")
        if present[name] != expected:
            raise ValueError(f"{WILLINGTON}/{name} doesn't match its SHA-256 in release.json")
    for required in ("WillingtonRuntime/__init__.py", "WillingtonRuntime/matrix.json"):
        if required not in present:
            raise ValueError(f"{WILLINGTON} is missing {required}")
    if not any(name in present for name in WILLINGTON_LICENSES):
        raise ValueError(f"{WILLINGTON} needs Willington's license notice: Kumi's MIT license doesn't cover these files")
    for platform in WILLINGTON_NATIVES:
        if sum((folder / name).stat().st_size for name in willington_for(present, platform)) > WILLINGTON_MAX_BYTES:
            raise ValueError(f"{WILLINGTON} is larger than {WILLINGTON_MAX_BYTES // (1024 * 1024)} MiB for {platform}")
    return present

def stage_willington(root: Path, remote: Path, target: str) -> None:
    """Willington's vendored files for the target's platform, inside AbletonMcpBridge: only the bridge loads
    them from there, and Live doesn't list a folder inside another as a Control Surface. Where Live doesn't
    run, nothing. release.json stays Willington's own, naming every platform's files."""
    folder = root / WILLINGTON
    files = willington_files(folder)
    platform = willington_platform(target)
    if files is not None and platform is not None:
        for name in (*willington_for(files, platform), "release.json"):
            copy(folder / name, remote / "willington" / name)

def stage_assets(root: Path, package: Path, revision: str, target: str) -> str:
    remote = package / "remote-script" / "AbletonMcpBridge"
    for source, dest in [("daws/ableton/remote-script/README.md", "remote-script/README.md"),
                         ("daws/ableton/remote-script/AbletonMcpBridge/__init__.py", "remote-script/AbletonMcpBridge/__init__.py"),
                         ("daws/ableton/remote-script/ableton_mcp_remote_script.py", "remote-script/AbletonMcpBridge/ableton_mcp_remote_script.py"),
                         ("daws/ableton/protocol/ableton-live-v1.operations.json", "remote-script/AbletonMcpBridge/ableton-live-v1.operations.json")]:
        copy(root / source, package / dest)
    stage_willington(root, remote, target)
    registry = json.loads((remote / "ableton-live-v1.operations.json").read_text())
    canonical = json.dumps(registry, ensure_ascii=False, sort_keys=True, separators=(",", ":"))
    registry_hash = hashlib.sha256(canonical.encode()).hexdigest()
    json_write(remote / "manifest.json", {"package": "AbletonMcpBridge", "algorithm": "sha256", "registryHash": registry_hash,
        "files": {name: digest(remote / name) for name in ("__init__.py", "ableton_mcp_remote_script.py", "ableton-live-v1.operations.json")}}, False)
    extension = root / "daws/ableton/live-extension"
    expected = (extension / "dist/extension.js.sha256").read_text().split()[0]
    if digest(extension / "dist/extension.js") != expected:
        raise ValueError("Live extension build does not match its committed SHA-256")
    for name in ("manifest.json", "dist/extension.js", "dist/extension.js.sha256"):
        copy(extension / name, package / "live-extension" / name)
    metadata = json.loads((extension / "package.json").read_text())
    json_write(package / "live-extension/package.json", {"name": metadata["name"], "version": metadata["version"],
               "private": True, "license": metadata["license"], "main": "dist/extension.js"})
    copy(root / "LICENSE.md", package / "LICENSE.md")
    if digest(package / "LICENSE.md") != MIT_SHA256:
        raise ValueError("release license differs from the receipt policy's MIT license")
    copy(root / "daws/ableton/ableton-mcp-server/README.md", package / "README.md")
    for source, destination in DOCUMENTS:
        target = package / "release-docs" / destination
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_text(transform_document((root / source).read_text(encoding="utf-8"), root, source, revision), encoding="utf-8")
    return registry_hash

def stage_bridge(root: Path, package: Path, binary: Path, target: str, source: dict, builder: dict, recipe: str) -> dict:
    if package.exists() and any(package.iterdir()):
        raise ValueError("bridge staging folder must be empty")
    package.mkdir(parents=True, exist_ok=True)
    metadata = tomllib.loads((root / "daws/ableton/ableton-mcp-server/Cargo.toml").read_text())["package"]
    extension = ".exe" if "windows" in target else ""
    for command in BRIDGE_BINARIES:
        name = command + extension
        copy(binary.parent / name, package / name)
        (package / name).chmod(0o755)
    json_write(package / "package.json", {"name": "@ableton-mcp/mcp-server", "version": metadata["version"],
               "private": True, "license": "MIT", "runtime": "rust-native", "target": target,
               "bin": {command: command + extension for command in BRIDGE_BINARIES}})
    registry_hash = stage_assets(root, package, source["commit"], target)
    files = inventory(package)
    manifest = {"schema": SCHEMA, "package": {"name": "@ableton-mcp/mcp-server", "version": metadata["version"], "license": "MIT", "private": True},
        "source": source, "build": {"runtime": "rust-native", "target": target, "builder": builder, "recipe": recipe},
        "protocol": {"host": "2026-07-28", "supportedHostVersions": ["2026-07-28", "2025-11-25"], "bridge": "ableton-live/v1", "registryHash": registry_hash},
        "distribution": {"channel": "local-native-tarball", "published": False, "signed": False, "notarized": False, "integrityIsIdentityProof": False},
        "exclusions": EXCLUSIONS, "algorithm": "sha256", "files": files, "roles": {name: role(name) for name in files}}
    json_write(package / "release-manifest.json", manifest)
    return manifest

def write_tar(fileobj, folder: Path, prefix: str, timestamp: int) -> None:
    """Regular USTAR entries only: no links, directory entries, PAX headers or host metadata."""
    with tarfile.open(fileobj=fileobj, mode="w|", format=tarfile.USTAR_FORMAT) as tar:
        for name in inventory(folder):
            path = folder / name
            entry = tarfile.TarInfo(f"{prefix}/{name}" if prefix else name)
            entry.size = path.stat().st_size
            entry.mode = 0o755 if path.stat().st_mode & 0o111 else 0o644
            entry.mtime = timestamp
            with path.open("rb") as content:
                tar.addfile(entry, content)

def archive(folder: Path, destination: Path, prefix: str, timestamp: int) -> str:
    """The folder as a gzipped tar, which every tar reads; its SHA-256."""
    with destination.open("wb") as output:
        with gzip.GzipFile(filename="", mode="wb", fileobj=output, mtime=0) as compressed:
            write_tar(compressed, folder, prefix, timestamp)
    return digest(destination)

def archive_xz(folder: Path, destination: Path, prefix: str, timestamp: int) -> str:
    """The same entries xz-compressed: a bundle of binaries comes out a third smaller than gzipped. A tar that reads
    xz (macOS's bsdtar, GNU tar beside xz) takes this one; every other, and Windows, the gzipped one."""
    with lzma.open(destination, "wb", format=lzma.FORMAT_XZ, check=lzma.CHECK_CRC64, preset=9) as compressed:
        write_tar(compressed, folder, prefix, timestamp)
    return digest(destination)

def stage_hands(root: Path, bundle: Path, target: str) -> None:
    name = "kumi-hands-" + digest(root / "daws/ableton/kumi-daw-ableton/src/hands/KumiHands.swift")[:12]
    hands = root / "target/hands"
    files = inventory(hands) if hands.exists() else {}
    if any(candidate != name for candidate in files):
        raise ValueError(f"hands helper payload must match the current source: {name}")
    if name not in files:
        if target.endswith("-apple-darwin"):
            raise ValueError(f"macOS release requires {hands / name}; run python3 scripts/build-hands.py first")
        return
    # The installed path of the JavaScript releases' helper: macOS ties the Accessibility permission to it.
    destination = bundle / "packages/runtime/hands" / name
    copy(hands / name, destination)
    destination.chmod(0o755)

# ── What a built library holds ───────────────────────────────────────────────────────────────────────────────────
# Read here, the same way on every system, rather than with each system's tools (nm, objdump, dumpbin): each
# architecture's exported names, and what a DAW's loader cares about on that system.

MACHO_ARCHS = {0x01000007: "x86_64", 0x0100000C: "arm64"}
ELF_ARCHS = {62: "x86_64", 183: "arm64"}
PE_ARCHS = {0x8664: "x86_64", 0xAA64: "arm64", 0xA641: "arm64ec", 0x014C: "x86"}

def uleb128(data: bytes, at: int) -> tuple[int, int]:
    value = shift = 0
    while True:
        byte = data[at]
        at += 1
        value |= (byte & 0x7F) << shift
        shift += 7
        if byte < 0x80:
            return value, at

def c_string(data: bytes, at: int) -> str:
    return data[at:data.index(b"\0", at)].decode("utf-8", "replace")

def macho_exports(trie: bytes) -> set[str]:
    """The names in a Mach-O export trie: each terminal node's path from the root."""
    names, stack, seen = set(), [(0, "")], set()
    while stack:
        node, prefix = stack.pop()
        if node in seen or node >= len(trie):
            raise ValueError("the export trie loops or points outside itself")
        seen.add(node)
        terminal, at = uleb128(trie, node)
        if terminal:
            names.add(prefix)
        at += terminal
        children = trie[at]
        at += 1
        for _ in range(children):
            end = trie.index(b"\0", at)
            edge = trie[at:end].decode("utf-8", "replace")
            child, at = uleb128(trie, end + 1)
            stack.append((child, prefix + edge))
    return names

def macho_slice(data: bytes) -> dict:
    if data[:4] != b"\xcf\xfa\xed\xfe":
        raise ValueError("not a 64-bit little-endian Mach-O")
    cputype, _, _, ncmds = struct.unpack_from("<iIII", data, 4)
    facts = {"arch": MACHO_ARCHS.get(cputype & 0xFFFFFFFF, f"cpu {cputype & 0xFFFFFFFF:#x}"), "exports": set(),
             "install_name": None, "signed": False}
    trie = symtab = None
    at = 32
    for _ in range(ncmds):
        command, size = struct.unpack_from("<II", data, at)
        if command == 0xD:  # LC_ID_DYLIB
            facts["install_name"] = c_string(data, at + struct.unpack_from("<I", data, at + 8)[0])
        elif command == 0x1D:  # LC_CODE_SIGNATURE
            facts["signed"] = True
        elif command == 0x80000033:  # LC_DYLD_EXPORTS_TRIE
            trie = struct.unpack_from("<II", data, at + 8)
        elif command in (0x22, 0x80000022) and struct.unpack_from("<I", data, at + 44)[0]:  # LC_DYLD_INFO(_ONLY)
            trie = struct.unpack_from("<II", data, at + 40)
        elif command == 0x2:  # LC_SYMTAB
            symtab = struct.unpack_from("<IIII", data, at + 8)
        at += size
    if trie:
        names = macho_exports(data[trie[0]:trie[0] + trie[1]])
    elif symtab:  # external, defined in a section, not a debugging entry
        symbols, count, strings, _ = symtab
        names = set()
        for i in range(count):
            name, kind = struct.unpack_from("<IB", data, symbols + 16 * i)
            if kind & 0x01 and kind & 0x0E == 0x0E and not kind & 0xE0:
                names.add(c_string(data, strings + name))
    else:
        names = set()
    facts["exports"] = {name[1:] if name.startswith("_") else name for name in names}
    return facts

def macho_facts(data: bytes) -> list[dict]:
    magic = struct.unpack_from(">I", data, 0)[0]
    if magic not in (0xCAFEBABE, 0xCAFEBABF):
        return [macho_slice(data)]
    wide = magic == 0xCAFEBABF
    slices = []
    for i in range(struct.unpack_from(">I", data, 4)[0]):
        if wide:
            _, _, offset, size, _, _ = struct.unpack_from(">iIQQII", data, 8 + 32 * i)
        else:
            _, _, offset, size, _ = struct.unpack_from(">iIIII", data, 8 + 20 * i)
        slices.append(macho_slice(data[offset:offset + size]))
    return slices

def elf_facts(data: bytes) -> list[dict]:
    if data[4] != 2:
        raise ValueError("not a 64-bit ELF")
    endian = "<" if data[5] == 1 else ">"
    machine = struct.unpack_from(endian + "H", data, 18)[0]
    sections_at = struct.unpack_from(endian + "Q", data, 40)[0]
    entry_size, count = struct.unpack_from(endian + "HH", data, 58)
    # (name, type, flags, address, offset, size, link, info, alignment, entry size)
    sections = [struct.unpack_from(endian + "IIQQQQIIQQ", data, sections_at + i * entry_size) for i in range(count)]
    exports, glibc = set(), None
    for _, kind, _, _, offset, size, link, info, _, entry in sections:
        strings = sections[link][4] if link < len(sections) else 0
        if kind == 11:  # SHT_DYNSYM: global or weak, defined, visible
            for i in range(size // entry if entry else 0):
                name, info_byte, other, section = struct.unpack_from(endian + "IBBH", data, offset + i * entry)
                if name and section and info_byte >> 4 in (1, 2) and other & 3 in (0, 3):
                    exports.add(c_string(data, strings + name))
        elif kind == 0x6FFFFFFE:  # SHT_GNU_verneed: the versions each needed library must have
            at = offset
            for _ in range(info):
                _, needed, _, aux, following = struct.unpack_from(endian + "HHIII", data, at)
                here = at + aux
                for _ in range(needed):
                    _, _, _, name, step = struct.unpack_from(endian + "IHHII", data, here)
                    version = re.fullmatch(r"GLIBC_(\d+(?:\.\d+)*)", c_string(data, strings + name))
                    if version:
                        glibc = max(glibc or (), tuple(map(int, version[1].split("."))))
                    here += step
                at += following
    return [{"arch": ELF_ARCHS.get(machine, f"machine {machine}"), "exports": exports, "glibc": glibc}]

def pe_facts(data: bytes) -> list[dict]:
    header = struct.unpack_from("<I", data, 0x3C)[0]
    if data[header:header + 4] != b"PE\0\0":
        raise ValueError("not a PE image")
    machine, count, _, _, _, optional_size, _ = struct.unpack_from("<HHIIIHH", data, header + 4)
    optional = header + 24
    directories = optional + (112 if struct.unpack_from("<H", data, optional)[0] == 0x20B else 96)
    listed = struct.unpack_from("<I", data, directories - 4)[0]
    # (name, virtual size, virtual address, raw size, raw offset)
    sections = [struct.unpack_from("<8sIIII", data, optional + optional_size + 40 * i) for i in range(count)]

    def at(rva: int) -> int:
        for _, virtual, address, raw_size, raw in sections:
            if address <= rva < address + max(virtual, raw_size):
                return rva - address + raw
        raise ValueError(f"address {rva:#x} is in no section")

    def directory(index: int) -> int:
        return struct.unpack_from("<I", data, directories + 8 * index)[0] if index < listed else 0

    exports, imports = set(), set()
    if rva := directory(0):
        names, table = struct.unpack_from("<I", data, at(rva) + 24)[0], struct.unpack_from("<I", data, at(rva) + 32)[0]
        exports = {c_string(data, at(struct.unpack_from("<I", data, at(table) + 4 * i)[0])) for i in range(names)}
    if rva := directory(1):  # IMAGE_IMPORT_DESCRIPTOR, until an empty one
        entry = at(rva)
        while any(struct.unpack_from("<IIIII", data, entry)):
            imports.add(c_string(data, at(struct.unpack_from("<I", data, entry + 12)[0])).lower())
            entry += 20
    if rva := directory(13):  # delay-load descriptors
        entry = at(rva)
        while name := struct.unpack_from("<I", data, entry + 4)[0]:
            imports.add(c_string(data, at(name)).lower())
            entry += 32
    return [{"arch": PE_ARCHS.get(machine, f"machine {machine:#x}"), "exports": exports, "imports": imports}]

def binary_facts(path: Path) -> list[dict]:
    """Each architecture a library holds (a universal Mach-O holds several), with its exported names and, by format,
    its own name and signature (Mach-O), the newest glibc it needs (ELF), or the DLLs it imports (PE)."""
    data = path.read_bytes()
    try:
        if data[:4] == b"\x7fELF":
            return elf_facts(data)
        if data[:2] == b"MZ":
            return pe_facts(data)
        if data[:4] in (b"\xcf\xfa\xed\xfe", b"\xca\xfe\xba\xbe", b"\xca\xfe\xba\xbf"):
            return macho_facts(data)
    except (struct.error, IndexError, ValueError) as error:
        raise ValueError(f"{path.name} can't be read as a library: {error}") from None
    raise ValueError(f"{path.name} isn't a Mach-O, ELF or PE library")

# ── The other DAWs' bridges: a library or a Renoise tool ─────────────────────────────────────────────────────────

def system_of(target: str) -> str:
    if target.endswith("-apple-darwin"):
        return "macos"
    if target.endswith("-pc-windows-msvc"):
        return "windows"
    if target.endswith("-unknown-linux-gnu"):
        return "linux"
    raise ValueError(f"no extension is built for {target}")

def crate_names(daw: dict) -> tuple[str, str]:
    """A library bridge's crate's package name and its library's (reaper-kumi, reaper_kumi)."""
    manifest = tomllib.loads((ROOT / daw["library"]["crate"] / "Cargo.toml").read_text(encoding="utf-8"))
    package = manifest["package"]["name"]
    return package, manifest.get("lib", {}).get("name", package.replace("-", "_"))

def extension_name(daw: dict, target: str) -> str:
    """The file the DAW loads on the target's system."""
    return crate_names(daw)[1] + {"macos": ".dylib", "windows": ".dll", "linux": ".so"}[system_of(target)]

def install_name(daw: dict) -> str:
    """The macOS extension's own name for itself; the linker's default would be the folder it was built in."""
    return f"@rpath/{crate_names(daw)[1]}.dylib"

def extension_builds(target: str) -> list[str]:
    """What the target's bundle builds an extension for. On a Mac both architectures, in one file: the DAW may run under
    Rosetta (x86_64) on an Apple silicon Mac whatever Kumi runs as. On Windows x64: a DAW for Windows on Arm may be an
    ARM64EC build (REAPER's is), which loads x64 libraries, as an x64 DAW under emulation does."""
    system = system_of(target)
    if system == "macos":
        return ["aarch64-apple-darwin", "x86_64-apple-darwin"]
    if system == "windows":
        return ["x86_64-pc-windows-msvc"]
    return [target]

def extension_archs(target: str) -> list[str]:
    arch = {"aarch64": "arm64", "x86_64": "x86_64"}
    return sorted(arch[build.split("-")[0]] for build in extension_builds(target))

def check_extension(daw: dict, path: Path, target: str, archs: list[str] | None = None) -> dict:
    """The extension as the target's DAW loads it: the architectures the bundle promises (`archs`, else the target's
    builds), each exporting the DAW's entry point; on macOS signed (Apple silicon runs no unsigned code) and named for
    itself, not for the build folder; on Windows with the C runtime linked in; on Linux needing no newer glibc than Kumi
    does."""
    system = system_of(target)
    entry = daw["library"]["entry"]
    slices = binary_facts(path)
    found, expected = sorted(piece["arch"] for piece in slices), sorted(archs or extension_archs(target))
    if found != expected:
        raise ValueError(f"{path.name} holds {', '.join(found)}, not {', '.join(expected)}")
    for piece in slices:
        where = f"{path.name} ({piece['arch']})"
        if entry not in piece["exports"]:
            raise ValueError(f"{where} doesn't export {entry}, so {daw['name']} wouldn't load it")
        if system == "macos" and not piece["signed"]:
            raise ValueError(f"{where} isn't signed: macOS on Apple silicon refuses unsigned code")
        if system == "macos" and piece["install_name"] != install_name(daw):
            raise ValueError(f"{where} names itself {piece['install_name']}, not {install_name(daw)}")
        crt = sorted(name for name in piece.get("imports", ()) if WINDOWS_CRT.fullmatch(name))
        if system == "windows" and crt:
            raise ValueError(f"{where} needs the C runtime installed ({', '.join(crt)}): build it with +crt-static")
        if system == "linux" and (piece.get("glibc") or ()) > GLIBC_MOST:
            raise ValueError(f"{where} needs glibc {'.'.join(map(str, piece['glibc']))}, newer than Kumi's "
                             f"{'.'.join(map(str, GLIBC_MOST))}: build it on the bundle's Ubuntu 22.04 runner")
    return {"archs": found, "sha256": digest(path)}

def extension_of(path: Path) -> dict:
    """The DAW a built library is the extension of, by its name as Cargo builds it or as it ships (libreaper_kumi.dylib,
    reaper_kumi.dll)."""
    stem = path.name.split(".")[0]
    for daw in DAW_BRIDGES.values():
        if "library" in daw and crate_names(daw)[1] in (stem, stem.removeprefix("lib")):
            return daw
    raise ValueError(f"{path.name} isn't the extension of a DAW in scripts/daws.toml")

def crate_version(root: Path, crate: str) -> str:
    return tomllib.loads((root / crate / "Cargo.toml").read_text(encoding="utf-8"))["package"]["version"]

def renoise_manifest_version(version: str) -> str:
    """The tool manifest's <Version>, which Renoise reads as a number: major.minor patch as M.mmpp, the patch's
    zeros left off (1.0.0 is 1.00, 1.2.3 is 1.0203), so each later version is a larger number."""
    if not re.fullmatch(r"\d+\.\d+\.\d+", version):
        raise ValueError(f"the Renoise tool's version {version!r} isn't major.minor.patch")
    major, minor, patch = map(int, version.split("."))
    if minor > 99 or patch > 99:
        raise ValueError(f"the Renoise tool's version {version} doesn't fit its manifest's number (minor and patch under 100)")
    return f"{major}.{minor:02d}" + (f"{patch:02d}" if patch else "")

def renoise_tool(daw: dict, root: Path) -> tuple[Path, str, dict[str, str]]:
    """A Renoise tool's folder, version and files, checked: its manifest names its id (the folder's name) and API, its
    version file and manifest agree, and it holds only the tool's own files (no preferences or reference a Renoise or a
    Kumi wrote into it)."""
    folder, api = root / daw["tool"]["folder"], daw["tool"]["api"]
    files = inventory(folder)
    manifest = (folder / "manifest.xml").read_text(encoding="utf-8")
    field = lambda name: (re.findall(rf"<{name}>\s*([^<]*?)\s*</{name}>", manifest) or [None])[0]
    tool_id = folder.name.removesuffix(".xrnx")
    if field("Id") != tool_id or folder.name != f"{tool_id}.xrnx":
        raise ValueError(f"the Renoise tool's manifest names {field('Id')}, not {tool_id}: song tool_data is keyed by it")
    if field("ApiVersion") != api:
        raise ValueError(f"the Renoise tool declares API {field('ApiVersion')}, not {api} (scripts/daws.toml)")
    declared = re.search(r'return\s+"([^"]+)"', (folder / "payload/version.lua").read_text(encoding="utf-8"))
    version = declared[1] if declared else ""
    if field("Version") != renoise_manifest_version(version):
        raise ValueError(f"the Renoise tool's manifest says Version {field('Version')}, and payload/version.lua {version} "
                         f"(its manifest number is {renoise_manifest_version(version)})")
    for name in files:
        if name in NOT_PAYLOAD or any(part.startswith(".") for part in name.split("/")) or not name.endswith(TOOL_SUFFIXES):
            raise ValueError(f"{daw['tool']['folder']}/{name} isn't one of the tool's own files (Lua, XML, JSON)")
    return folder, version, files

def stage_daws(root: Path, bundle: Path, target: str, extensions: dict[str, Path]) -> dict[str, str]:
    """Each other DAW's bridge into the bundle's daws/<id> folder: a library from `extensions` (checked, with its
    version beside it) or a Renoise tool; their versions, by the DAW's id."""
    versions = {}
    for daw_id, daw in DAW_BRIDGES.items():
        folder = bundle / DAWS / daw_id
        if "library" in daw:
            check_extension(daw, extensions[daw_id], target)
            copy(extensions[daw_id], folder / extension_name(daw, target))
            (folder / extension_name(daw, target)).chmod(0o755)
            versions[daw_id] = crate_version(root, daw["library"]["crate"])
            (folder / f"{crate_names(daw)[1]}.version").write_text(versions[daw_id] + "\n", encoding="ascii")
        else:
            source, versions[daw_id], files = renoise_tool(daw, root)
            for name in files:
                copy(source / name, folder / source.name / name)
    return versions

def cargo_library(daw: dict, rust_target: str) -> str:
    name = crate_names(daw)[1]
    return {"macos": f"lib{name}.dylib", "windows": f"{name}.dll", "linux": f"lib{name}.so"}[system_of(rust_target)]

def build_extension(daw: dict, root: Path, target: str, profile: str, out: Path) -> tuple[Path, list[str]]:
    """A DAW's extension for the target's bundle, finished as it ships, in `out`; the commands that built it.

    Never stripped by rustc: on macOS 27 a rustc strip can misalign the library's LINKEDIT string pool, and dyld then
    refuses it (Cargo.toml keeps the crate at strip = "none"). The platform's own strip runs afterwards instead. A Mac
    gets one universal file, signed again after lipo and strip; Windows links the C runtime in.
    """
    profile_args = ["--release"] if profile == "release" else ["--profile", profile]
    commands, built = [], []
    for rust_target in extension_builds(target):
        environment = dict(os.environ)
        target_dir = Path(environment.get("CARGO_TARGET_DIR", root / "target"))
        if system_of(rust_target) == "windows":
            # In a target folder of its own: the flag changes every crate's build, and the workspace's has none.
            environment["RUSTFLAGS"] = (environment.get("RUSTFLAGS", "") + " -C target-feature=+crt-static").strip()
            target_dir = target_dir / "crt-static"
        command = ["cargo", "build", "--locked", *profile_args, "--target", rust_target, "-p", crate_names(daw)[0],
                   "--target-dir", str(target_dir)]
        subprocess.run(command, cwd=root, env=environment, check=True)
        commands.append(" ".join(command))
        built.append(target_dir / rust_target / profile / cargo_library(daw, rust_target))
    out.mkdir(parents=True, exist_ok=True)
    final = out / extension_name(daw, target)
    final.unlink(missing_ok=True)
    system = system_of(target)
    if system == "macos":
        subprocess.run(["lipo", "-create", "-output", str(final), *map(str, built)], check=True)
        subprocess.run(["strip", "-x", str(final)], check=True)
        subprocess.run(["codesign", "--force", "--sign", "-", str(final)], check=True)
    else:
        shutil.copyfile(built[0], final)
        if system == "linux":
            subprocess.run(["strip", "--strip-unneeded", str(final)], check=True)
    return final, commands

def build_release(root: Path, binaries: Path, out: Path, target: str, source: dict, builder: dict, recipe: str, bridge_only=False,
                  extensions: dict[str, Path] | None = None) -> dict:
    """The target's bundle in `out` (or, `bridge_only`, the Ableton bridge's artifact alone). A bundle carries each other
    DAW's bridge: a library built for it (`extensions`, by the DAW's id) or a Renoise tool."""
    out.mkdir(parents=True, exist_ok=True)
    extensions = extensions or {}
    unbuilt = [daw for daw in DAW_BRIDGES.values() if "library" in daw and extensions.get(daw["id"]) is None]
    if not bridge_only and unbuilt:
        raise ValueError(f"a bundle carries {unbuilt[0]['name']}'s {unbuilt[0]['bridge']}: build it (build_extension) or name it")
    extension = ".exe" if "windows" in target else ""
    timestamp = int(datetime.datetime.fromisoformat(source["commitTimestamp"]).timestamp())
    with tempfile.TemporaryDirectory(prefix="kumi-native-stage-", dir=out) as temp:
        stage = Path(temp)
        package = stage / "package"
        manifest = stage_bridge(root, package, binaries / f"ableton-mcp-server{extension}", target, source, builder, recipe)
        artifact_name = f"ableton-mcp-server-{manifest['package']['version']}-{target}.tar.gz"
        artifact = stage / artifact_name
        artifact_hash = archive(package, artifact, "package", timestamp)
        if artifact.stat().st_size > 32 * 1024 * 1024 or sum(path.stat().st_size for path in package.rglob("*") if path.is_file()) > 63 * 1024 * 1024:
            raise ValueError("native bridge artifact exceeds lifecycle's 32 MiB compressed / 64 MiB tar bounds")
        if bridge_only:
            final = out / artifact_name
            os.replace(artifact, final)
            json_write(out / "prepared.json", {"artifact": artifact_name, "sha256": artifact_hash, "version": manifest["package"]["version"]})
            # An artifact consumer extracts package/ and verifies it against the exact tarball.
            return {"artifact": artifact_name, "sha256": artifact_hash, "manifest": manifest}
        bundle = stage / "bundle"
        for name in BINARIES:
            copy(binaries / f"{name}{extension}", bundle / f"{name}{extension}")
            (bundle / f"{name}{extension}").chmod(0o755)
        for folder in ("remote-script", "live-extension"):
            shutil.copytree(package / folder, bundle / folder)
        stage_hands(root, bundle, target)
        daws = stage_daws(root, bundle, target, extensions)
        version = tomllib.loads((root / "crates/kumi/Cargo.toml").read_text())["package"]["version"]
        # `bridge` is Live's bridge, as every earlier Kumi reads it; each other DAW's bridge is a key of its own, its id.
        metadata = {"name": "kumi", "version": version, "bridge": manifest["package"]["version"], **daws, "runtime": "rust-native", "target": target}
        json_write(bundle / "package.json", metadata)
        # Existing JavaScript applications use this entry to return from an explicit rollback.
        # Normal installed launchers execute the root native binary directly.
        copy(root / "scripts/migration/kumi.mjs", bundle / "apps/kumi/bin/kumi.mjs")
        json_write(bundle / "apps/mcp-server/package.json", {"version": metadata["bridge"]})
        json_write(bundle / "kumi-install.json", {"kumi": version, "bridge": metadata["bridge"], **daws, "runtime": "rust-native", "target": target})
        for name in ("LICENSE.md", "THIRD_PARTY_NOTICES.md", "README.md", "CHANGELOG.md"):
            copy(root / name, bundle / name)
        prepared = bundle / "bridge"
        prepared.mkdir()
        shutil.move(package, prepared / "package")
        shutil.move(artifact, prepared / artifact_name)
        json_write(prepared / "prepared.json", {"artifact": artifact_name, "sha256": artifact_hash, "version": metadata["bridge"]})
        bundle_name = f"kumi-{target}.tar.gz"
        archive_path = stage / bundle_name
        bundle_hash = archive(bundle, archive_path, "", timestamp)
        xz_name = f"kumi-{target}.tar.xz"
        xz_hash = archive_xz(bundle, stage / xz_name, "", timestamp)
        release = {"kumi": version, "bundle": bundle_name, "sha256": bundle_hash, "bundle_xz": xz_name, "sha256_xz": xz_hash,
                   "runtime": "rust-native", "target": target, "bridge": metadata["bridge"], **daws}
        os.replace(archive_path, out / bundle_name)
        os.replace(stage / xz_name, out / xz_name)
        json_write(out / "kumi-release.json", release)
        (out / "SHA256SUMS").write_text(f"{bundle_hash}  {bundle_name}\n{xz_hash}  {xz_name}\n", encoding="ascii")
        return release

def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--target", help="Rust target triple (the rustc host by default)")
    parser.add_argument("--out", type=Path, help="output folder (release/native/<target>)")
    parser.add_argument("--binaries-dir", type=Path, help="already built binaries; skips cargo build")
    parser.add_argument("--bridge-only", action="store_true", help="build only the receipt-bound bridge artifact")
    parser.add_argument("--profile", default="release",
                        help="Cargo profile: release (the default, what's published) or ci-release (CI's quicker build)")
    for daw in DAW_BRIDGES.values():
        if "library" in daw:
            parser.add_argument(f"--{daw['id']}-extension", type=Path,
                                help=f"{daw['name']}'s {daw['bridge']}, already built for the target's bundle")
    parser.add_argument("--check-extension", type=Path, metavar="LIBRARY",
                        help="only check a DAW's extension, as built, as the target's bundle would carry it, then stop")
    parser.add_argument("--thin", action="store_true", help="with --check-extension: one build for the target's own architecture")
    args = parser.parse_args()
    target = args.target or re.search(r"^host: (.+)$", run(["rustc", "-vV"]), re.M)[1]
    if not re.fullmatch(r"[A-Za-z0-9_.-]+", target):
        raise ValueError("invalid Rust target triple")
    if args.check_extension:
        archs = [{"aarch64": "arm64", "x86_64": "x86_64"}[target.split("-")[0]]] if args.thin else None
        print(json.dumps(check_extension(extension_of(args.check_extension), args.check_extension, target, archs)))
        return
    source = source_evidence(ROOT)
    if not re.fullmatch(r"[a-z][a-z0-9-]*", args.profile):
        raise ValueError("invalid Cargo profile name")
    profile = ["--release"] if args.profile == "release" else ["--profile", args.profile]
    command = ["cargo", "build", "--locked", *profile, "--target", target]
    command += ["-p", "ableton-mcp-server", "--bin", "ableton-mcp-server", "--bin", "ableton-mcp-analysis-worker"] if args.bridge_only else ["--workspace", "--bins"]
    recipe = " ".join(command)
    out = args.out or ROOT / "release/native" / target
    extensions = {daw_id: getattr(args, daw_id.replace("-", "_") + "_extension")
                  for daw_id, daw in DAW_BRIDGES.items() if "library" in daw}
    unbuilt = [] if args.bridge_only else [DAW_BRIDGES[daw_id] for daw_id, built in extensions.items() if built is None]
    if args.binaries_dir is None:
        subprocess.run(command, cwd=ROOT, check=True)
        binaries = Path(os.environ.get("CARGO_TARGET_DIR", ROOT / "target")) / target / args.profile
        for daw in unbuilt:
            extensions[daw["id"]], _ = build_extension(daw, ROOT, target, args.profile, Path(os.environ.get("CARGO_TARGET_DIR", ROOT / "target")) / f"{daw['id']}-extension" / target)
    else:
        binaries = args.binaries_dir.resolve()
        recipe = "prebuilt binaries supplied with --binaries-dir; expected build: " + recipe
        if unbuilt:
            raise ValueError(f"with --binaries-dir, name {unbuilt[0]['name']}'s {unbuilt[0]['bridge']} built for the bundle with "
                             f"--{unbuilt[0]['id']}-extension")
    builder = {"rustc": run(["rustc", "--version"]), "cargo": run(["cargo", "--version"]),
        "platform": {"Darwin": "darwin", "Windows": "win32"}.get(platform.system(), platform.system().lower()),
        "architecture": platform.machine(), "runnerImage": os.environ.get("ImageOS", "local"),
        "runnerImageVersion": os.environ.get("ImageVersion", "local"), "cargoLockSha256": digest(ROOT / "Cargo.lock"),
        "workflowSha256": digest(ROOT / ".github/workflows/installer.yml")}
    result = build_release(ROOT, binaries, out, target, source, builder, recipe, args.bridge_only, extensions)
    print(json.dumps({key: value for key, value in result.items() if key != "manifest"}, indent=2))

if __name__ == "__main__":
    main()
