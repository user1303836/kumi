#!/usr/bin/env python3
"""Build the source-bound universal macOS helper for Logic Pro, at target/logic-hands/kumi-logic-hands-<source digest>.

The folder is left holding only the helper of the current source, as the release script expects: builds of
older sources are removed, and any other file there is refused. Importable: build() returns the helper's path.
"""
import hashlib
import os
from pathlib import Path
import platform
import subprocess
import sys
import tempfile

ROOT = Path(__file__).resolve().parents[1]
SOURCE = ROOT / "crates/logic-kumi/helper/KumiLogicHands.swift"
OUT = ROOT / "target/logic-hands"
PREFIX = "kumi-logic-hands-"
ARCHITECTURES = ("arm64", "x86_64")

def digest(source: Path = SOURCE) -> str:
    return hashlib.sha256(source.read_bytes()).hexdigest()[:12]

def helper_name(source: Path = SOURCE) -> str:
    return PREFIX + digest(source)

def clean(out: Path, keep: Path) -> None:
    """Only the current helper may stay: older builds go, anything else stops the build."""
    for entry in sorted(out.iterdir()):
        if entry == keep:
            continue
        if entry.is_file() and not entry.is_symlink() and entry.name.startswith(PREFIX):
            entry.unlink()
        else:
            raise SystemExit(f"{out} holds something that is not a helper build: {entry.name}")

def build(source: Path = SOURCE, out: Path = OUT, reuse: bool = False) -> Path:
    """The helper for this source: both slices, one universal file, ad-hoc signed. With reuse, an existing build of the same source is kept."""
    if platform.system() != "Darwin":
        raise SystemExit("The macOS helper must be built on macOS.")
    out.mkdir(parents=True, exist_ok=True)
    target = out / helper_name(source)
    clean(out, target)
    if reuse and target.is_file():
        return target
    with tempfile.TemporaryDirectory(prefix="kumi-logic-hands-") as directory:
        slices = []
        for architecture in ARCHITECTURES:
            output = Path(directory) / architecture
            subprocess.run(["xcrun", "swiftc", "-O", "-swift-version", "5", "-target", f"{architecture}-apple-macos13", "-o", str(output), str(source)], check=True)
            slices.append(str(output))
        partial = out / (target.name + ".partial")
        subprocess.run(["lipo", "-create", "-output", str(partial), *slices], check=True)
        # codesign says "replacing existing signature" of the linker's ad-hoc arm64 signature every time: kept for a failure only.
        signed = subprocess.run(["codesign", "--force", "--sign", "-", str(partial)], capture_output=True, text=True)
        if signed.returncode != 0:
            sys.stderr.write(signed.stderr)
            raise SystemExit(f"codesign failed ({signed.returncode}) on {partial}")
        os.replace(partial, target)
    return target

def main(argv=None) -> None:
    arguments = sys.argv[1:] if argv is None else argv
    if arguments:
        raise SystemExit("usage: build-logic-hands.py (no arguments; the helper is written under target/logic-hands)")
    print(build())

if __name__ == "__main__":
    main()
