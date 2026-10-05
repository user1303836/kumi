#!/usr/bin/env python3
"""Build the source-bound universal macOS Accessibility helper, at target/hands/kumi-hands-<source digest>."""
import hashlib
from pathlib import Path
import platform
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parents[1]

def main():
    if platform.system() != "Darwin":
        raise SystemExit("The macOS helper must be built on macOS.")
    source = ROOT / "crates/kumi-runtime/src/hands/KumiHands.swift"
    digest = hashlib.sha256(source.read_bytes()).hexdigest()[:12]
    with tempfile.TemporaryDirectory(prefix="kumi-hands-") as directory:
        slices = []
        for arch in ("arm64", "x86_64"):
            output = Path(directory) / arch
            subprocess.run(["xcrun", "swiftc", "-O", "-target", f"{arch}-apple-macos13", "-o", str(output), str(source)], check=True)
            slices.append(str(output))
        target = ROOT / "target/hands" / f"kumi-hands-{digest}"
        target.parent.mkdir(parents=True, exist_ok=True)
        subprocess.run(["lipo", "-create", "-output", str(target), *slices], check=True)
        subprocess.run(["codesign", "--force", "--sign", "-", str(target)], check=True)
        print(target)

if __name__ == "__main__":
    main()
