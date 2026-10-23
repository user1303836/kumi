#!/usr/bin/env python3
"""Aggregate target releases and the compatibility bundle consumed by existing Kumi updaters."""
from __future__ import annotations
import argparse
import importlib.util
import json
import os
from pathlib import Path
import re
import shutil
import tempfile

SPEC = importlib.util.spec_from_file_location("native_release", Path(__file__).with_name("build-native-release.py"))
native = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(native)


def build(inputs: list[Path], out: Path, node: str, timestamp: int = 0, release_base: str | None = None) -> dict:
    """The compatibility bundle holds only the adapter and where each target's archive is published:
    old updaters download kumi.tar.gz whole into memory, so it stays the size of a JavaScript release,
    and the adapter fetches the one archive its platform needs."""
    if not re.fullmatch(r"\d+\.\d+\.\d+", node):
        raise ValueError("legacy updater compatibility requires an exact Node version")
    releases = {}
    for path in inputs:
        value = json.loads(path.read_text())
        target = value.get("target", "")
        if value.get("runtime") != "rust-native" or not re.fullmatch(r"[A-Za-z0-9_.-]+", target):
            raise ValueError("expected a native target release manifest")
        if target in releases:
            raise ValueError("duplicate target")
        bundle = value.get("bundle", "")
        if not re.fullmatch(r"[A-Za-z0-9_.-]+\.tar\.gz", bundle):
            raise ValueError("unsafe bundle name")
        if native.digest(path.parent / bundle) != value.get("sha256"):
            raise ValueError("native archive checksum mismatch")
        if not re.fullmatch(r"\d+\.\d+\.\d+(?:-[\w.]+)?", value.get("kumi", "")):
            raise ValueError("invalid Kumi version")
        releases[target] = (value, path.parent / bundle)
    if not releases:
        raise ValueError("at least one target release is required")
    # One commit's bundles: the same Kumi, Live's bridge and other DAWs' bridges (scripts/daws.toml) on every target.
    versions = {tuple(value[0].get(key) for key in ("kumi", "bridge", *native.DAW_BRIDGES)) for value in releases.values()}
    if len(versions) != 1:
        names = ["Kumi", "bridge", *(f"{daw['name']} {daw['bridge']}" for daw in native.DAW_BRIDGES.values())]
        raise ValueError(f"all targets must have the same {', '.join(names[:-1])} and {names[-1]} versions")
    version, bridge, *_ = versions.pop()
    targets = {target: item[0] for target, item in sorted(releases.items())}
    base = (release_base or f"https://github.com/user1303836/kumi/releases/download/v{version}").rstrip("/")
    if not re.fullmatch(r"https?://[^\s]+", base):
        raise ValueError("unsafe release base")
    out.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="kumi-migration-", dir=out) as directory:
        stage = Path(directory)
        payload = stage / "bundle"
        native.copy(native.ROOT / "scripts/migration/kumi.mjs", payload / "apps/kumi/bin/kumi.mjs")
        native.json_write(payload / "package.json", {"name": "kumi", "version": version, "bridge": bridge, "runtime": "native-migration"})
        native.json_write(payload / "apps/mcp-server/package.json", {"version": bridge})
        native.json_write(payload / "native-targets.json", {"releases": base, "targets": targets})
        for target, (release, archive) in releases.items():
            shutil.copyfile(archive, stage / release["bundle"])
            native.json_write(stage / f"kumi-release-{target}.json", release)
        digest = native.archive(payload, stage / "kumi.tar.gz", "", timestamp)
        manifest = {"kumi": version, "bundle": "kumi.tar.gz", "sha256": digest, "node": node, "bridge": bridge, "targets": targets}
        native.json_write(stage / "kumi-release.json", manifest)
        hashes = {"kumi.tar.gz": digest, **{value["bundle"]: value["sha256"] for value in targets.values()}}
        (stage / "SHA256SUMS").write_text("".join(f"{digest}  {name}\n" for name, digest in sorted(hashes.items())), encoding="ascii")
        for path in stage.iterdir():
            if path.is_file():
                os.replace(path, out / path.name)
    return manifest


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("manifests", type=Path, nargs="+")
    parser.add_argument("--out", type=Path, default=native.ROOT / "release")
    parser.add_argument("--node", required=True, help="current legacy installer's bundled Node version")
    parser.add_argument("--releases", help="where the target archives are published (default: this version's GitHub release)")
    args = parser.parse_args()
    manifest = build(args.manifests, args.out, args.node, release_base=args.releases)
    print(json.dumps({key: value for key, value in manifest.items() if key != "targets"}, indent=2))

if __name__ == "__main__":
    main()
