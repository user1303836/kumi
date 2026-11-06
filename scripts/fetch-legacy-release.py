#!/usr/bin/env python3
"""Fetch the published Kumi 1.7.5, the last JavaScript release, and unpack it.

Existing installations run its updater and bridge lifecycle, so the migration tests take them from here.
Prints the unpacked app's folder, for KUMI_LEGACY_APP:
  export KUMI_LEGACY_APP="$(python3 scripts/fetch-legacy-release.py [folder])"
"""
import hashlib
import http.client
import io
import sys
import tarfile
import tempfile
import time
import urllib.error
import urllib.request
from pathlib import Path

URL = "https://github.com/user1303836/kumi/releases/download/v1.7.5/kumi.tar.gz"
SHA256 = "1e932a401e88a1e6d3985d1f21c2c6af11883887675d5ae1de291dbb75385795"
# GitHub's release downloads fail now and then (a 5xx, a reset connection): #1074's Extended lost its migration tests
# to one. A download that fails so is tried again, this many times in all, waiting 2, 4 then 8 s in between.
ATTEMPTS = 4


def download() -> bytes:
    """The bundle's bytes. A server error or a network failure is tried again; a client error (4xx) isn't, and the last
    try's failure is the script's."""
    for attempt in range(1, ATTEMPTS):
        try:
            return fetch()
        except urllib.error.HTTPError as error:
            if error.code < 500:
                raise
            failure = f"HTTP {error.code}"
        except (OSError, http.client.HTTPException) as error:  # URLError, a timeout, a reset or a short read
            failure = f"{type(error).__name__}: {error}"
        print(f"Kumi 1.7.5's download failed ({failure}); trying again ({attempt + 1} of {ATTEMPTS})",
              file=sys.stderr)
        time.sleep(2 ** attempt)
    return fetch()


def fetch() -> bytes:
    with urllib.request.urlopen(URL, timeout=300) as response:
        return response.read()


def main():
    target = Path(sys.argv[1] if len(sys.argv) > 1 else Path(tempfile.gettempdir()) / "kumi-1.7.5").resolve()
    if not (target / "apps/kumi/dist/src/install.js").is_file():
        data = download()
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
