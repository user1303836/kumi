#!/usr/bin/env python3
"""Build native Kumi bundles and receipt-bound bridge artifacts (Python 3.11+)."""
from __future__ import annotations
import argparse
import datetime
import gzip
import hashlib
import json
import os
from pathlib import Path
import platform
import posixpath
import re
import shutil
import subprocess
import tarfile
import tempfile
import tomllib
from urllib.parse import quote

ROOT = Path(__file__).resolve().parents[1]
SCHEMA = "ableton-mcp-native-release/v1"
BRIDGE_BINARIES = ("ableton-mcp-server", "ableton-mcp-analysis-worker")
BINARIES = ("kumi", *BRIDGE_BINARIES, "kumi-harness", "kumi-library-measure", "kumi-library-learner")
DOCUMENTS = [("crates/ableton-mcp-server/README.md", "README.md")] + [(f"docs/en/{name}.md", f"{name}.md") for name in (
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
                    "protocol/ableton-live-v1.operations.json": "../remote-script/AbletonMcpBridge/ableton-live-v1.operations.json"}
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
    for source, dest in [("remote-script/README.md", "remote-script/README.md"),
                         ("remote-script/AbletonMcpBridge/__init__.py", "remote-script/AbletonMcpBridge/__init__.py"),
                         ("remote-script/ableton_mcp_remote_script.py", "remote-script/AbletonMcpBridge/ableton_mcp_remote_script.py"),
                         ("protocol/ableton-live-v1.operations.json", "remote-script/AbletonMcpBridge/ableton-live-v1.operations.json")]:
        copy(root / source, package / dest)
    stage_willington(root, remote, target)
    registry = json.loads((remote / "ableton-live-v1.operations.json").read_text())
    canonical = json.dumps(registry, ensure_ascii=False, sort_keys=True, separators=(",", ":"))
    registry_hash = hashlib.sha256(canonical.encode()).hexdigest()
    json_write(remote / "manifest.json", {"package": "AbletonMcpBridge", "algorithm": "sha256", "registryHash": registry_hash,
        "files": {name: digest(remote / name) for name in ("__init__.py", "ableton_mcp_remote_script.py", "ableton-live-v1.operations.json")}}, False)
    extension = root / "apps/live-extension"
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
    copy(root / "crates/ableton-mcp-server/README.md", package / "README.md")
    for source, destination in DOCUMENTS:
        target = package / "release-docs" / destination
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_text(transform_document((root / source).read_text(encoding="utf-8"), root, source, revision), encoding="utf-8")
    return registry_hash

def stage_bridge(root: Path, package: Path, binary: Path, target: str, source: dict, builder: dict, recipe: str) -> dict:
    if package.exists() and any(package.iterdir()):
        raise ValueError("bridge staging folder must be empty")
    package.mkdir(parents=True, exist_ok=True)
    metadata = tomllib.loads((root / "crates/ableton-mcp-server/Cargo.toml").read_text())["package"]
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

def archive(folder: Path, destination: Path, prefix: str, timestamp: int) -> str:
    """Regular USTAR entries only: no links, directory entries, PAX headers or host metadata."""
    files = inventory(folder)
    with destination.open("wb") as output:
        with gzip.GzipFile(filename="", mode="wb", fileobj=output, mtime=0) as compressed:
            with tarfile.open(fileobj=compressed, mode="w|", format=tarfile.USTAR_FORMAT) as tar:
                for name in files:
                    path = folder / name
                    entry = tarfile.TarInfo(f"{prefix}/{name}" if prefix else name)
                    entry.size = path.stat().st_size
                    entry.mode = 0o755 if path.stat().st_mode & 0o111 else 0o644
                    entry.mtime = timestamp
                    with path.open("rb") as content:
                        tar.addfile(entry, content)
    return digest(destination)

def stage_hands(root: Path, bundle: Path, target: str) -> None:
    name = "kumi-hands-" + digest(root / "crates/kumi-runtime/src/hands/KumiHands.swift")[:12]
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

def build_release(root: Path, binaries: Path, out: Path, target: str, source: dict, builder: dict, recipe: str, bridge_only=False) -> dict:
    out.mkdir(parents=True, exist_ok=True)
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
        version = tomllib.loads((root / "crates/kumi/Cargo.toml").read_text())["package"]["version"]
        metadata = {"name": "kumi", "version": version, "bridge": manifest["package"]["version"], "runtime": "rust-native", "target": target}
        json_write(bundle / "package.json", metadata)
        # Existing JavaScript applications use this entry to return from an explicit rollback.
        # Normal installed launchers execute the root native binary directly.
        copy(root / "scripts/migration/kumi.mjs", bundle / "apps/kumi/bin/kumi.mjs")
        json_write(bundle / "apps/mcp-server/package.json", {"version": metadata["bridge"]})
        json_write(bundle / "kumi-install.json", {"kumi": version, "bridge": metadata["bridge"], "runtime": "rust-native", "target": target})
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
        release = {"kumi": version, "bundle": bundle_name, "sha256": bundle_hash, "runtime": "rust-native", "target": target, "bridge": metadata["bridge"]}
        os.replace(archive_path, out / bundle_name)
        json_write(out / "kumi-release.json", release)
        (out / "SHA256SUMS").write_text(f"{bundle_hash}  {bundle_name}\n", encoding="ascii")
        return release

def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--target", help="Rust target triple (the rustc host by default)")
    parser.add_argument("--out", type=Path, help="output folder (release/native/<target>)")
    parser.add_argument("--binaries-dir", type=Path, help="already built binaries; skips cargo build")
    parser.add_argument("--bridge-only", action="store_true", help="build only the receipt-bound bridge artifact")
    parser.add_argument("--profile", default="release",
                        help="Cargo profile: release (the default, what's published) or ci-release (CI's quicker build)")
    args = parser.parse_args()
    target = args.target or re.search(r"^host: (.+)$", run(["rustc", "-vV"]), re.M)[1]
    if not re.fullmatch(r"[A-Za-z0-9_.-]+", target):
        raise ValueError("invalid Rust target triple")
    source = source_evidence(ROOT)
    if not re.fullmatch(r"[a-z][a-z0-9-]*", args.profile):
        raise ValueError("invalid Cargo profile name")
    profile = ["--release"] if args.profile == "release" else ["--profile", args.profile]
    command = ["cargo", "build", "--locked", *profile, "--target", target]
    command += ["-p", "ableton-mcp-server", "--bin", "ableton-mcp-server", "--bin", "ableton-mcp-analysis-worker"] if args.bridge_only else ["--workspace", "--bins"]
    recipe = " ".join(command)
    if args.binaries_dir is None:
        subprocess.run(command, cwd=ROOT, check=True)
        binaries = Path(os.environ.get("CARGO_TARGET_DIR", ROOT / "target")) / target / args.profile
    else:
        binaries = args.binaries_dir.resolve()
        recipe = "prebuilt binaries supplied with --binaries-dir; expected build: " + recipe
    builder = {"rustc": run(["rustc", "--version"]), "cargo": run(["cargo", "--version"]),
        "platform": {"Darwin": "darwin", "Windows": "win32"}.get(platform.system(), platform.system().lower()),
        "architecture": platform.machine(), "runnerImage": os.environ.get("ImageOS", "local"),
        "runnerImageVersion": os.environ.get("ImageVersion", "local"), "cargoLockSha256": digest(ROOT / "Cargo.lock"),
        "workflowSha256": digest(ROOT / ".github/workflows/installer.yml")}
    result = build_release(ROOT, binaries, args.out or ROOT / "release/native" / target, target, source, builder, recipe, args.bridge_only)
    print(json.dumps({key: value for key, value in result.items() if key != "manifest"}, indent=2))

if __name__ == "__main__":
    main()
