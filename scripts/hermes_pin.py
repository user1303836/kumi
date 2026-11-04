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

Both also download each bundle once (about 170 MB, one at a time, deleted after), confirm its size and SHA-256,
and read it as setup will: a `.tar.xz` of plain files and folders, with `kumi` (`kumi.exe` on Windows) at its root.

The file (schema 1) holds the version, the tag and, for each target, the exact URL, SHA-256, download size and
unpacked size of its `.tar.xz` bundle, with the DAW component versions it carries. When a `plugin.yaml` sits beside
it, the script also sets that manifest's one top-level `version: "x.y.z"` line to the pinned version (the catalog
shows it as the plugin's version), and `--check` fails when the two differ. It changes nothing else in the manifest.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path
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


def measured(github: GitHub, pin: dict) -> dict:
    """The pin with each target's unpacked size, after downloading each bundle (one at a time, deleted after) and
    confirming its size and SHA-256 against the release's own records."""
    targets = {}
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
            path.unlink()
            targets[target] = {**record, "unpacked_bytes": unpacked}
            print(f"  {target}: {name} matches ({size} bytes, {unpacked} unpacked)")
    return {**pin, "targets": targets}


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


def main(argv: list[str] | None = None, github: GitHub | None = None) -> int:
    parser = argparse.ArgumentParser(description="Pin the Kumi release that Kumi's Hermes plugin installs.")
    parser.add_argument("version", help="a published Kumi version, e.g. 1.24.67 (never 'latest')")
    parser.add_argument("--repository", default=REPOSITORY, help=f"owner/name (default {REPOSITORY})")
    parser.add_argument("--out", type=Path, default=PIN, help=f"the pin file (default {PIN.relative_to(ROOT)})")
    parser.add_argument("--check", action="store_true", help="don't write: say whether --out matches the release")
    args = parser.parse_args(argv)
    try:
        version = args.version.removeprefix("v")
        if not VERSION.fullmatch(version):
            raise Refused(f"{args.version!r} isn't a version like 1.24.67: the pin names one exact release")
        github = github or GitHub(args.repository)
        pin = measured(github, read_pin(github, version))
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
