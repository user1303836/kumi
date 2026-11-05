#!/usr/bin/env python3
"""Pin the Kumi release that Kumi's Hermes plugin installs (Python 3.11+, and gh: signed in or not).

The plugin downloads Kumi's runtime at setup only from the release that `hermes-plugin/kumi_release.json`
names, and refuses a bundle whose SHA-256 differs from the one recorded there. That is the Hermes catalog's
rule 17 (a native component fetched at setup from the official source, pinned by checksum in reviewed code)
and rule 3 (only a new pin, reviewed and re-pinned in the catalog, moves users to another release). So this
script writes the file only from a published release, never `latest`, a draft or a prerelease, and confirms
every checksum three ways: the release's `kumi-release.json` and each target's `kumi-release-<target>.json`,
its `SHA256SUMS`, and the digest GitHub recorded for the uploaded file.

    python3 scripts/hermes_pin.py 1.24.67             write the pin for v1.24.67
    python3 scripts/hermes_pin.py 1.24.67 --check     say whether the committed pin matches that release
    python3 scripts/hermes_pin.py 1.24.67 --yt-dlp 2026.10.26
                                                      also pin yt-dlp's standalone builds from that yt-dlp release

Both also download each bundle once (about 170 MB, one at a time, deleted after), confirm its size and SHA-256,
and read it as setup will: a `.tar.xz` of plain files and folders, with `kumi` (`kumi.exe` on Windows) at its root.

The `yt_dlp` section names the yt-dlp release setup installs for reading videos (CC-21): for each target, the
standalone build's URL, SHA-256 and size, from an immutable, published release of yt-dlp/yt-dlp, each confirmed in its
`SHA2-256SUMS`, GitHub's digest and a download (five files, one at a time, deleted after). Without `--yt-dlp`, the
tag the pin already names is kept and checked again; with neither, the pin has no such section.

The file (schema 1) holds the version, the tag and, for each target, the exact URL, SHA-256, download size and
unpacked size of its `.tar.xz` bundle, with the DAW component versions it carries. It also holds what the pinned
runtime itself says it takes, read by running the bundle for this computer's target with a scratch HOME, KUMI_HOME,
state and Kumi home, no DAW, and no update check:
- `serve_options`, the long options `kumi serve` takes, from the usage it prints for an argument it doesn't know;
- `serve_operations`, the operations `kumi serve --stdio` lists for each DAW it serves (MCP initialize, then
  tools/list), sorted;
- `generate_kinds` and `ledger_policy_keys`, from that listing's `generate` operation: the `kind` values its input
  schema enumerates, and the ledger policy keys its `_meta` names (`kumi/ledger_policy_keys`), each sorted, and both
  empty when the runtime offers no `generate` (CC-15's 15a and W5-c);
- `listen_sources`, for each DAW, the sources that DAW's `listen` takes, sorted: `file`, `sound`, `artifact`, and
  `track` (a track or the mix). A runtime that can hear the project says which DAWs can, per DAW, in `listen`'s
  `_meta` (`kumi/listen_sources`). Without it, the input schema must be files-only, and the kinds are read from its
  source fields; a schema offering a track without that declaration is refused. `[]` for a DAW that lists no `listen`
  (core-gap item 2).
The plugin's tests check every flag it passes `kumi serve`, every operation it calls, every kind it offers and every
ledger key it writes against them, and the plugin offers a track or mix listen only where its DAW's `listen_sources`
has `track`, so a plugin that needs a newer runtime than its pin fails before it reaches anyone.
When a `plugin.yaml` sits beside it, the script also sets that manifest's one top-level `version: "x.y.z"` line to the
pinned version (the catalog shows it as the plugin's version), and `--check` fails when the two differ. It changes
nothing else in the manifest.
"""
from __future__ import annotations

import argparse
from collections.abc import Callable
import hashlib
import json
import os
from pathlib import Path
import platform
import re
import shutil
import subprocess
import sys
import tarfile
import tempfile

ROOT = Path(__file__).resolve().parents[1]
PIN = ROOT / "hermes-plugin" / "kumi_release.json"
REPOSITORY = "user1303836/kumi"
SCHEMA = 1
# The six bundles the Installer workflow builds and publishes for a release (.github/workflows/installer.yml).
TARGETS = (
    "aarch64-apple-darwin",
    "x86_64-apple-darwin",
    "aarch64-pc-windows-msvc",
    "x86_64-pc-windows-msvc",
    "aarch64-unknown-linux-gnu",
    "x86_64-unknown-linux-gnu",
)
VERSION = re.compile(r"[0-9]+\.[0-9]+\.[0-9]+")
SHA256 = re.compile(r"[0-9a-f]{64}")
# The DAW component versions each target's release file names, carried into the pin for setup to show.
COMPONENTS = ("bridge", "reaper", "renoise")
# The argument `kumi serve` is run with to read its usage: one it doesn't take, so it prints the usage and stops.
PROBE = "--hermes-pin-probe"
# The DAWs `kumi serve --daw` serves, each asked for its operations, and the MCP version the plugin speaks to it
# (hermes-plugin/runtime.py's MCP_PROTOCOL).
DAWS = ("ableton", "reaper", "renoise")
MCP_PROTOCOL = "2025-11-25"
# The sources a `listen` may take, as `listen_sources` names them (`track`: a track or the mix), and, for a runtime
# that declares none in `_meta`, the input schema's source field that names each files-only kind.
LISTEN_KINDS = ("artifact", "file", "sound", "track")
SOURCE_FIELDS = {"file": "file", "sound_id": "sound", "artifact_id": "artifact"}
# yt-dlp's release (CC-21): the standalone build each Kumi target's setup installs.
YT_DLP_REPOSITORY = "yt-dlp/yt-dlp"
YT_DLP_TAG = re.compile(r"[0-9]{4}\.[0-9]{2}\.[0-9]{2}(\.[0-9]+)?")
YT_DLP_ASSETS = {
    "aarch64-apple-darwin": "yt-dlp_macos",
    "x86_64-apple-darwin": "yt-dlp_macos",
    "aarch64-pc-windows-msvc": "yt-dlp_arm64.exe",
    "x86_64-pc-windows-msvc": "yt-dlp.exe",
    "aarch64-unknown-linux-gnu": "yt-dlp_linux_aarch64",
    "x86_64-unknown-linux-gnu": "yt-dlp_linux",
}


class Refused(Exception):
    """A release, or a pin, that must not be written or trusted, with the reason."""


class GitHub:
    """A release as the script reads it, through gh."""

    def __init__(self, repository: str) -> None:
        if not shutil.which("gh"):
            raise Refused("gh isn't installed: https://cli.github.com")
        self.repository = repository

    def _gh(self, args: list[str]) -> bytes:
        result = subprocess.run(["gh", *args], capture_output=True)
        if result.returncode:
            raise Refused(f"gh {' '.join(args)}: {result.stderr.decode(errors='replace').strip()}")
        return result.stdout

    def release(self, tag: str) -> dict:
        return json.loads(self._gh(["api", f"repos/{self.repository}/releases/tags/{tag}"]))

    def asset(self, tag: str, name: str) -> bytes:
        return self._gh(["release", "download", tag, "--repo", self.repository, "--pattern", name, "--output", "-"])

    def download(self, tag: str, name: str, into: Path) -> Path:
        self._gh(["release", "download", tag, "--repo", self.repository, "--pattern", name, "--dir", str(into)])
        return into / name


def checksums(text: str) -> dict[str, str]:
    """`SHA256SUMS` as {file name: digest}. A line that isn't `<digest>  <name>` or a file listed twice is refused."""
    sums: dict[str, str] = {}
    for number, line in enumerate(text.splitlines(), 1):
        if not line.strip():
            continue
        parts = line.split()
        if len(parts) != 2 or not SHA256.fullmatch(parts[0]):
            raise Refused(f"SHA256SUMS line {number} isn't '<sha256>  <file>': {line!r}")
        digest, name = parts[0], parts[1].removeprefix("*")
        if name in sums and sums[name] != digest:
            raise Refused(f"SHA256SUMS lists {name} twice, with different digests")
        sums[name] = digest
    return sums


def parsed(name: str, data: bytes) -> dict:
    try:
        value = json.loads(data)
    except ValueError as error:
        raise Refused(f"{name} isn't JSON: {error}") from None
    if not isinstance(value, dict):
        raise Refused(f"{name} isn't a JSON object")
    return value


def pin_of(version: str, repository: str, release: dict, manifest: bytes, sums: bytes,
           per_target: dict[str, bytes]) -> dict:
    """The pin for `version`, from what its published release holds: the GitHub release, its `kumi-release.json`,
    its `SHA256SUMS`, and each target's `kumi-release-<target>.json`. Refuses anything that doesn't agree."""
    tag = f"v{version}"
    if release.get("tag_name") != tag:
        raise Refused(f"GitHub returned the release {release.get('tag_name')!r} for {tag}")
    if release.get("draft") or release.get("prerelease"):
        raise Refused(f"{tag} is a draft or a prerelease: only a published release can be pinned")
    combined = parsed("kumi-release.json", manifest)
    if combined.get("kumi") != version:
        raise Refused(f"kumi-release.json is for Kumi {combined.get('kumi')!r}, not {version}")
    listed = combined.get("targets")
    if not isinstance(listed, dict):
        raise Refused("kumi-release.json has no targets")
    if missing := [target for target in TARGETS if target not in listed]:
        raise Refused(f"{tag} has no bundle for {', '.join(missing)}: the plugin supports all six targets")
    if unknown := sorted(set(listed) - set(TARGETS)):
        raise Refused(f"{tag} has targets this script doesn't know: {', '.join(unknown)} (add them to TARGETS)")
    digests = checksums(sums.decode(errors="replace"))
    assets = {asset.get("name"): asset for asset in release.get("assets") or []}
    targets = {}
    for target in TARGETS:
        entry, name = listed[target], f"kumi-{target}.tar.xz"
        if not isinstance(entry, dict):
            raise Refused(f"kumi-release.json's {target} entry isn't an object")
        own = parsed(f"kumi-release-{target}.json", per_target[target])
        for source, record in (("kumi-release.json", entry), (f"kumi-release-{target}.json", own)):
            if record.get("kumi") != version or record.get("target", target) != target or record.get("bundle_xz") != name:
                raise Refused(f"{source} doesn't describe {name} of Kumi {version}")
        sha = entry.get("sha256_xz")
        if not isinstance(sha, str) or not SHA256.fullmatch(sha):
            raise Refused(f"kumi-release.json's {target} entry has no SHA-256 for {name}")
        if own.get("sha256_xz") != sha:
            raise Refused(f"kumi-release-{target}.json and kumi-release.json disagree on {name}'s SHA-256")
        if digests.get(name) != sha:
            raise Refused(f"SHA256SUMS disagrees with kumi-release.json on {name}")
        asset = assets.get(name)
        if asset is None:
            raise Refused(f"{tag} has no {name} uploaded")
        if asset.get("digest") != f"sha256:{sha}":
            raise Refused(f"GitHub's digest of {name} is {asset.get('digest')!r}, not sha256:{sha}")
        url = f"https://github.com/{repository}/releases/download/{tag}/{name}"
        if asset.get("browser_download_url") != url:
            raise Refused(f"{name} downloads from {asset.get('browser_download_url')!r}, not {url}")
        size = asset.get("size")
        if not isinstance(size, int) or size <= 0:
            raise Refused(f"{name} has no size")
        targets[target] = {"url": url, "sha256": sha, "size": size,
                           **{key: entry[key] for key in COMPONENTS if isinstance(entry.get(key), str)}}
    return {"schema": SCHEMA, "kumi": version, "repository": repository, "tag": tag,
            "release": f"https://github.com/{repository}/releases/tag/{tag}", "targets": targets}


def read_pin(github: GitHub, version: str) -> dict:
    tag = f"v{version}"
    release = github.release(tag)
    manifest = github.asset(tag, "kumi-release.json")
    sums = github.asset(tag, "SHA256SUMS")
    per_target = {target: github.asset(tag, f"kumi-release-{target}.json") for target in TARGETS}
    return pin_of(version, github.repository, release, manifest, sums, per_target)


def executable(target: str) -> str:
    return "kumi.exe" if "windows" in target else "kumi"


def unpacked_bytes(path: Path, target: str) -> int:
    """The bundle's size once extracted. Refuses an archive setup would refuse: anything but plain files and
    folders, a name that is absolute or climbs out with `..`, or no Kumi executable at its root."""
    try:
        with tarfile.open(path, "r:xz") as archive:
            members = archive.getmembers()
    except (tarfile.TarError, OSError, EOFError) as error:
        raise Refused(f"{path.name} isn't a readable .tar.xz: {error}") from None
    for member in members:
        parts = Path(member.name).parts
        if member.name.startswith(("/", "\\")) or ".." in parts or not (member.isfile() or member.isdir()):
            raise Refused(f"{path.name} holds {member.name!r}, which setup won't extract")
    if not any(member.isfile() and member.name.removeprefix("./") == executable(target) for member in members):
        raise Refused(f"{path.name} has no {executable(target)} at its root")
    return sum(member.size for member in members if member.isfile())


def host_target() -> str | None:
    """This computer's release target, as the Installer names them; None for a system no release is built for."""
    arch = {"arm64": "aarch64", "aarch64": "aarch64", "x86_64": "x86_64", "amd64": "x86_64"}.get(platform.machine().lower())
    system = {"Darwin": "apple-darwin", "Linux": "unknown-linux-gnu", "Windows": "pc-windows-msvc"}.get(platform.system())
    target = f"{arch}-{system}" if arch and system else None
    return target if target in TARGETS else None


def extracted(path: Path, target: str, into: Path) -> Path:
    """The bundle's Kumi executable, alone, extracted into `into` and made executable."""
    name = executable(target)
    with tarfile.open(path, "r:xz") as archive:
        member = next(member for member in archive.getmembers()
                      if member.isfile() and member.name.removeprefix("./") == name)
        data = archive.extractfile(member).read()
    out = into / "probe" / name
    out.parent.mkdir(exist_ok=True)
    out.write_bytes(data)
    out.chmod(0o755)
    return out


def scratch_env(scratch: Path) -> dict[str, str]:
    """The environment the pinned runtime is run with: a scratch HOME and KUMI_HOME, so nothing of this computer's Kumi
    is read or written, and no update check, as the plugin starts it."""
    home = scratch / "probe-home"
    home.mkdir(exist_ok=True)
    kept = {key: value for key, value in os.environ.items() if key in ("PATH", "SYSTEMROOT", "TMPDIR", "TEMP", "TMP")}
    return {**kept, "HOME": str(home), "USERPROFILE": str(home), "KUMI_HOME": str(home / "kumi"),
            "KUMI_NO_UPDATE_CHECK": "1"}


def serve_usage(kumi: Path, scratch: Path) -> str:
    """What `kumi serve` prints for an argument it doesn't take: its usage, on stderr."""
    try:
        result = subprocess.run([str(kumi), "serve", PROBE], capture_output=True, stdin=subprocess.DEVNULL,
                                env=scratch_env(scratch), timeout=30)
    except (OSError, subprocess.TimeoutExpired) as error:
        raise Refused(f"couldn't run the pinned kumi to read kumi serve's options: {error}") from None
    return result.stderr.decode(errors="replace") + result.stdout.decode(errors="replace")


def serve_listing(kumi: Path, scratch: Path, version: str) -> dict:
    """What `kumi serve --stdio` lists for each DAW it serves (MCP's initialize, then tools/list; the end of its input
    stops it): `serve_operations`, and `generate`'s `generate_kinds` and `ledger_policy_keys` (both empty without a
    `generate`), each the sorted union over the DAWs; and `listen_sources`, each DAW's own (`listen_kinds`). Refused
    unless it names itself kumi-serve at `version` and lists some operations, unless a `generate` it lists names both
    its kinds and its policy keys, and unless each `listen` it lists says which sources it takes."""
    exchange = "".join(json.dumps(message) + "\n" for message in (
        {"jsonrpc": "2.0", "id": 1, "method": "initialize",
         "params": {"protocolVersion": MCP_PROTOCOL, "capabilities": {},
                    "clientInfo": {"name": "hermes-pin", "version": "1"}}},
        {"jsonrpc": "2.0", "method": "notifications/initialized", "params": {}},
        {"jsonrpc": "2.0", "id": 2, "method": "tools/list", "params": {}})).encode()
    names: set[str] = set()
    kinds: set[str] = set()
    keys: set[str] = set()
    sources: dict[str, list[str]] = {}
    for daw in DAWS:
        room = Path(tempfile.mkdtemp(prefix=f"serve-{daw}-", dir=scratch))
        argv = [str(kumi), "serve", "--stdio", "--state-dir", str(room / "state"), "--kumi-home", str(room / "kumi-home"),
                "--daw", daw]
        try:
            result = subprocess.run(argv, input=exchange, capture_output=True, env=scratch_env(scratch), timeout=60)
        except (OSError, subprocess.TimeoutExpired) as error:
            raise Refused(f"couldn't run the pinned kumi serve --stdio to list its operations: {error}") from None
        replies = {}
        for line in result.stdout.decode(errors="replace").splitlines():
            try:
                message = json.loads(line)
            except ValueError:
                continue
            if isinstance(message, dict) and message.get("id") in (1, 2):
                replies[message["id"]] = message.get("result")
        info = (replies.get(1) or {}).get("serverInfo") if isinstance(replies.get(1), dict) else None
        if not isinstance(info, dict) or (info.get("name"), info.get("version")) != ("kumi-serve", version):
            raise Refused(f"the pinned kumi serve --daw {daw} answered initialize as {info!r}, not kumi-serve {version}")
        tools = (replies.get(2) or {}).get("tools") if isinstance(replies.get(2), dict) else None
        tools = [tool for tool in tools if isinstance(tool, dict)] if isinstance(tools, list) else []
        listed = [tool.get("name") for tool in tools]
        if not listed or not all(isinstance(name, str) and name for name in listed):
            raise Refused(f"the pinned kumi serve --daw {daw} listed no operations by name: {tools!r:.200}")
        names.update(listed)
        sources[daw] = []
        for tool in tools:
            if tool.get("name") == "generate":
                kinds.update(_strings(((tool.get("inputSchema") or {}).get("properties") or {}).get("kind", {}).get("enum"),
                                      f"--daw {daw}'s generate enumerates no kinds in its input schema"))
                keys.update(_strings((tool.get("_meta") or {}).get("kumi/ledger_policy_keys"),
                                     f"--daw {daw}'s generate names no kumi/ledger_policy_keys in its _meta"))
            elif tool.get("name") == "listen":
                sources[daw] = listen_kinds(tool, daw)
    return {"serve_operations": sorted(names), "generate_kinds": sorted(kinds), "ledger_policy_keys": sorted(keys),
            "listen_sources": sources}


def listen_kinds(tool: dict, daw: str) -> list[str]:
    """The sources `--daw daw`'s `listen` takes, sorted (core-gap item 2): the kinds its `_meta` declares
    (`kumi/listen_sources`, which the runtime's dispatch also enforces), or, without that, the files-only kinds its
    input schema's source fields name. A schema that offers a track or the mix without the declaration is refused:
    whether a DAW can capture is the runtime's to say, per DAW, and the schema is the same for every DAW."""
    meta = tool.get("_meta")
    declared = meta.get("kumi/listen_sources") if isinstance(meta, dict) else None
    if declared is not None:
        found = _strings(declared, f"--daw {daw}'s listen names no kumi/listen_sources in its _meta")
    elif _offers_track(tool.get("inputSchema")):
        raise Refused(f"the pinned kumi serve --daw {daw}'s listen offers a track or the mix but doesn't say, in its "
                      "_meta's kumi/listen_sources, whether this DAW can capture")
    else:
        schema = tool.get("inputSchema") if isinstance(tool.get("inputSchema"), dict) else {}
        source = (schema.get("properties") or {}).get("source")
        fields = source.get("properties") if isinstance(source, dict) else None
        found = [SOURCE_FIELDS[name] for name in (fields if isinstance(fields, dict) else {}) if name in SOURCE_FIELDS]
        if not found:
            raise Refused(f"the pinned kumi serve --daw {daw}'s listen names no source it takes: {schema!r:.200}")
    unknown = sorted(set(found) - set(LISTEN_KINDS))
    if unknown:
        raise Refused(f"the pinned kumi serve --daw {daw}'s listen takes sources this script doesn't know: "
                      f"{', '.join(unknown)}")
    return sorted(set(found))


def _offers_track(node: object) -> bool:
    """Whether a JSON schema, anywhere in it, has a `track` property or requires one."""
    if isinstance(node, dict):
        properties, required = node.get("properties"), node.get("required")
        if (isinstance(properties, dict) and "track" in properties) or (isinstance(required, list) and "track" in required):
            return True
        return any(_offers_track(value) for value in node.values())
    return isinstance(node, list) and any(_offers_track(item) for item in node)


def _strings(value: object, missing: str) -> list[str]:
    """A non-empty list of non-empty strings, or a refusal saying what the pinned runtime left out."""
    if not isinstance(value, list) or not value or not all(isinstance(item, str) and item for item in value):
        raise Refused(f"the pinned kumi serve {missing}: {value!r:.200}")
    return value


def serve_options(usage: str) -> list[str]:
    """`kumi serve`'s long options, sorted, from the usage it printed: the `usage:` line and the indented lines that
    continue it, never the error line above them, which quotes the argument it didn't take."""
    lines = usage.splitlines()
    start = next((index for index, line in enumerate(lines) if line.lstrip().startswith("usage:")), None)
    if start is None:
        raise Refused("the pinned kumi serve printed no usage line, so its options can't be recorded")
    block = [lines[start]]
    for line in lines[start + 1:]:
        if not line.strip() or not line[:1].isspace():
            break
        block.append(line)
    options = sorted(set(re.findall(r"(?<![\w-])--[a-z][a-z0-9-]*", "\n".join(block))))
    if not options or PROBE in options:
        raise Refused(f"the pinned kumi serve's usage names no options of its own: {block[0].strip()!r}")
    return options


def measured(github: GitHub, pin: dict, probe: Callable[[Path, Path], str] | None = None,
             lister: Callable[[Path, Path, str], dict] | None = None) -> dict:
    """The pin with each target's unpacked size, after downloading each bundle (one at a time, deleted after) and
    confirming its size and SHA-256 against the release's own records; and what the bundle for this computer's target
    says it takes when run: `serve_options` (`probe`, `serve_usage` unless a test says otherwise), and
    `serve_operations`, `generate_kinds`, `ledger_policy_keys` and `listen_sources` (`lister`, `serve_listing`)."""
    host = host_target()
    if host not in pin["targets"]:
        raise Refused(f"run this on one of the release's targets ({', '.join(TARGETS)}): it reads kumi serve's "
                      "options and operations by running the pinned runtime")
    targets, options, listing = {}, [], {}
    with tempfile.TemporaryDirectory(prefix="hermes-pin-") as scratch:
        for target, record in pin["targets"].items():
            name = record["url"].rsplit("/", 1)[1]
            path = github.download(pin["tag"], name, Path(scratch))
            digest = hashlib.sha256()
            with path.open("rb") as stream:
                for chunk in iter(lambda: stream.read(1 << 20), b""):
                    digest.update(chunk)
            size = path.stat().st_size
            if size != record["size"] or digest.hexdigest() != record["sha256"]:
                raise Refused(f"the downloaded {name} doesn't match its pin ({size} bytes, sha256 {digest.hexdigest()})")
            unpacked = unpacked_bytes(path, target)
            if target == host:
                kumi = extracted(path, target, Path(scratch))
                options = serve_options((probe or serve_usage)(kumi, Path(scratch)))
                listing = (lister or serve_listing)(kumi, Path(scratch), pin["kumi"])
                print(f"  kumi serve {pin['kumi']} takes {' '.join(options)}")
                print(f"  kumi serve {pin['kumi']} offers {' '.join(listing['serve_operations'])}; generate's kinds: "
                      f"{' '.join(listing['generate_kinds']) or 'none'}; ledger policy keys: "
                      f"{' '.join(listing['ledger_policy_keys']) or 'none'}")
                print("  listen takes: " + "; ".join(f"{daw} {' '.join(kinds) or 'nothing'}"
                                                     for daw, kinds in listing["listen_sources"].items()))
            path.unlink()
            targets[target] = {**record, "unpacked_bytes": unpacked}
            print(f"  {target}: {name} matches ({size} bytes, {unpacked} unpacked)")
    return {**pin, "serve_options": options, "serve_operations": listing["serve_operations"],
            "generate_kinds": listing["generate_kinds"], "ledger_policy_keys": listing["ledger_policy_keys"],
            "listen_sources": listing["listen_sources"], "targets": targets}


def yt_dlp_pin(github: GitHub, tag: str) -> dict:
    """The `yt_dlp` section for yt-dlp's release `tag`: refused unless the release is published and immutable, and
    each target's build agrees in its `SHA2-256SUMS`, GitHub's digest, its URL, and a download of it (each distinct
    file once, deleted after)."""
    if not YT_DLP_TAG.fullmatch(tag):
        raise Refused(f"{tag!r} isn't a yt-dlp tag like 2026.10.26: the pin names one exact release")
    release = github.release(tag)
    if release.get("tag_name") != tag:
        raise Refused(f"GitHub returned the yt-dlp release {release.get('tag_name')!r} for {tag}")
    if release.get("draft") or release.get("prerelease"):
        raise Refused(f"yt-dlp {tag} is a draft or a prerelease: only a published release can be pinned")
    if release.get("immutable") is not True:
        raise Refused(f"yt-dlp {tag} isn't an immutable release: its files could change after they're pinned")
    digests = checksums(github.asset(tag, "SHA2-256SUMS").decode(errors="replace"))
    assets = {asset.get("name"): asset for asset in release.get("assets") or []}
    entries = {}
    for name in dict.fromkeys(YT_DLP_ASSETS.values()):
        sha = digests.get(name)
        if sha is None:
            raise Refused(f"yt-dlp {tag}'s SHA2-256SUMS doesn't list {name}")
        asset = assets.get(name)
        if asset is None:
            raise Refused(f"yt-dlp {tag} has no {name} uploaded")
        if asset.get("digest") != f"sha256:{sha}":
            raise Refused(f"GitHub's digest of yt-dlp's {name} is {asset.get('digest')!r}, not sha256:{sha}")
        url = f"https://github.com/{YT_DLP_REPOSITORY}/releases/download/{tag}/{name}"
        if asset.get("browser_download_url") != url:
            raise Refused(f"yt-dlp's {name} downloads from {asset.get('browser_download_url')!r}, not {url}")
        size = asset.get("size")
        if not isinstance(size, int) or isinstance(size, bool) or size <= 0:
            raise Refused(f"yt-dlp's {name} has no size")
        entries[name] = {"asset": name, "url": url, "sha256": sha, "size": size}
    with tempfile.TemporaryDirectory(prefix="hermes-pin-yt-dlp-") as scratch:
        for name, entry in entries.items():
            path = github.download(tag, name, Path(scratch))
            digest = hashlib.sha256()
            with path.open("rb") as stream:
                for chunk in iter(lambda: stream.read(1 << 20), b""):
                    digest.update(chunk)
            size = path.stat().st_size
            path.unlink()
            if size != entry["size"] or digest.hexdigest() != entry["sha256"]:
                raise Refused(f"the downloaded yt-dlp {name} doesn't match its release "
                              f"({size} bytes, sha256 {digest.hexdigest()})")
            print(f"  yt-dlp {tag}: {name} matches ({size} bytes)")
    return {"repository": YT_DLP_REPOSITORY, "tag": tag,
            "release": f"https://github.com/{YT_DLP_REPOSITORY}/releases/tag/{tag}",
            "targets": {target: dict(entries[name]) for target, name in YT_DLP_ASSETS.items()}}


def committed_yt_dlp_tag(path: Path) -> str | None:
    """The yt-dlp tag the pin at `path` already names, or None when there's no pin or it names none."""
    try:
        committed = json.loads(path.read_text(encoding="utf-8"))
    except FileNotFoundError:
        return None
    except (OSError, ValueError) as error:
        raise Refused(f"can't read {path}: {error}") from None
    section = committed.get("yt_dlp") if isinstance(committed, dict) else None
    tag = section.get("tag") if isinstance(section, dict) else None
    return tag if isinstance(tag, str) else None


def text_of(pin: dict) -> str:
    return json.dumps(pin, indent=2) + "\n"


def replace(path: Path, data: bytes) -> None:
    if not path.parent.is_dir():
        raise Refused(f"{path.parent} doesn't exist")
    scratch = path.with_name(f".{path.name}.tmp")
    scratch.write_bytes(data)
    os.replace(scratch, path)


# A manifest's top-level `version: "x.y.z"` line (not one nested in config_schema), with its line ending.
MANIFEST_VERSION = re.compile(r'^version:[ \t]*"([^"\r\n]*)"[ \t]*(\r?)$', re.M)


def manifest_line(manifest: Path) -> tuple[str, re.Match[str]] | None:
    """The plugin.yaml beside the pin: its text and its one top-level version line, or None when there's no
    plugin.yaml. Anything but exactly one such line is refused: that line is all this script may change there."""
    try:
        text = manifest.read_bytes().decode("utf-8")
    except FileNotFoundError:
        return None
    except (OSError, UnicodeDecodeError) as error:
        raise Refused(f"can't read {manifest}: {error}") from None
    lines = list(MANIFEST_VERSION.finditer(text))
    if len(lines) != 1:
        raise Refused(f'{manifest} has {len(lines)} top-level version: "x.y.z" lines; the pin sets exactly one')
    return text, lines[0]


def main(argv: list[str] | None = None, github: GitHub | None = None,
         probe: Callable[[Path, Path], str] | None = None,
         lister: Callable[[Path, Path, str], dict] | None = None, yt_dlp: GitHub | None = None) -> int:
    parser = argparse.ArgumentParser(description="Pin the Kumi release that Kumi's Hermes plugin installs.")
    parser.add_argument("version", help="a published Kumi version, e.g. 1.24.67 (never 'latest')")
    parser.add_argument("--repository", default=REPOSITORY, help=f"owner/name (default {REPOSITORY})")
    parser.add_argument("--out", type=Path, default=PIN, help=f"the pin file (default {PIN.relative_to(ROOT)})")
    parser.add_argument("--check", action="store_true", help="don't write: say whether --out matches the release")
    parser.add_argument("--yt-dlp", metavar="TAG", help="a yt-dlp release tag, e.g. 2026.10.26 (default: the one "
                        "--out names, if any)")
    args = parser.parse_args(argv)
    try:
        version = args.version.removeprefix("v")
        if not VERSION.fullmatch(version):
            raise Refused(f"{args.version!r} isn't a version like 1.24.67: the pin names one exact release")
        github = github or GitHub(args.repository)
        yt_dlp_tag = args.yt_dlp if args.yt_dlp is not None else committed_yt_dlp_tag(args.out)
        if yt_dlp_tag is not None and not YT_DLP_TAG.fullmatch(yt_dlp_tag):
            raise Refused(f"{yt_dlp_tag!r} isn't a yt-dlp tag like 2026.10.26: the pin names one exact release")
        pin = measured(github, read_pin(github, version), probe, lister)
        if yt_dlp_tag is not None:
            pin["yt_dlp"] = yt_dlp_pin(yt_dlp or GitHub(YT_DLP_REPOSITORY), yt_dlp_tag)
        manifest = args.out.parent / "plugin.yaml"
        line = manifest_line(manifest)
        if args.check:
            try:
                committed = json.loads(args.out.read_text(encoding="utf-8"))
            except (OSError, ValueError) as error:
                raise Refused(f"can't read {args.out}: {error}") from None
            if committed != pin:
                raise Refused(f"{args.out} doesn't match the published {pin['tag']}; rewrite it with this script")
            if line and line[1].group(1) != version:
                raise Refused(f'{manifest} says version "{line[1].group(1)}", not {version}; '
                              "rewrite both with this script")
            print(f"{args.out} matches the published {pin['tag']}"
                  + (f", and {manifest.name} says {version}" if line else ""))
            return 0
        replace(args.out, text_of(pin).encode("utf-8"))
        if line:
            text, match = line
            replace(manifest, (text[:match.start()] + f'version: "{version}"' + match.group(2)
                               + text[match.end():]).encode("utf-8"))
        print(f"wrote {args.out}: Kumi {version}, {len(pin['targets'])} targets, each checked against "
              "kumi-release.json, its target's file, SHA256SUMS, GitHub's digest and the downloaded bundle"
              + (f"; {manifest.name}'s version is {version}" if line else f"; no {manifest.name} beside it"))
        return 0
    except Refused as refusal:
        print(f"hermes_pin.py: {refusal}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())
