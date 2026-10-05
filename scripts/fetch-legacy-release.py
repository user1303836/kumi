#!/usr/bin/env python3
"""Fetch the published Kumi 1.7.5, the last JavaScript release, and unpack it.

Existing installations run its updater and bridge lifecycle, so the migration tests take them from here.
Prints the unpacked app's folder, for KUMI_LEGACY_APP:
  export KUMI_LEGACY_APP="$(python3 scripts/fetch-legacy-release.py [folder])"
"""
import hashlib
import io
import sys
import tarfile
import tempfile
import urllib.request
from pathlib import Path

URL = "https://github.com/user1303836/kumi/releases/download/v1.7.5/kumi.tar.gz"
SHA256 = "1e932a401e88a1e6d3985d1f21c2c6af11883887675d5ae1de291dbb75385795"


def main():
    target = Path(sys.argv[1] if len(sys.argv) > 1 else Path(tempfile.gettempdir()) / "kumi-1.7.5").resolve()
    if not (target / "apps/kumi/dist/src/install.js").is_file():
        with urllib.request.urlopen(URL, timeout=300) as response:
            data = response.read()
        if hashlib.sha256(data).hexdigest() != SHA256:
            sys.exit("The download isn't the published Kumi 1.7.5 bundle.")
        target.mkdir(parents=True, exist_ok=True)
        with tarfile.open(fileobj=io.BytesIO(data), mode="r:gz") as archive:
            try:
                archive.extractall(target, filter="data")
            except TypeError:  # Python before 3.11.4 has no extraction filters
                archive.extractall(target)
    print(target)


if __name__ == "__main__":
    main()
