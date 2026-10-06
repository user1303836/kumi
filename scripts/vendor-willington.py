#!/usr/bin/env python3
"""Put a Willington bundle in vendor/willington: its runtime files, Willington's license and their release.json.

    python3 scripts/vendor-willington.py Willington-matrix.zip --license LICENSE.md --commit <Willington commit>

The bundle is the Willington-matrix artifact of a Bundle run on Willington's main, and the commit is the one
that run built. The new folder is checked the way a release checks it before it replaces the old one, so a
refused bundle leaves vendor/willington as it was. docs/en/DEVELOPER_GUIDE.md has the whole update.
"""
import argparse
import importlib.util
import json
import shutil
import stat
import tempfile
import zipfile
from pathlib import Path, PurePosixPath

ROOT = Path(__file__).resolve().parents[1]
SPEC = importlib.util.spec_from_file_location("native_release", ROOT / "scripts/build-native-release.py")
release = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(release)


def vendor(bundle: Path, license_file: Path, commit: str, version: str | None = None, root: Path = ROOT) -> dict[str, str]:
    """Replace root's vendor/willington with the bundle's files, the license and release.json; each file's SHA-256."""
    if license_file.name not in release.WILLINGTON_LICENSES:
        raise ValueError(f"Willington's license notice is {' or '.join(release.WILLINGTON_LICENSES)}, not {license_file.name}")
    folder = root / release.WILLINGTON
    folder.parent.mkdir(parents=True, exist_ok=True)
    # Staged beside the folder, so the finished one moves into place on the same disk.
    with tempfile.TemporaryDirectory(dir=folder.parent, prefix=".willington-") as temp:
        staged = Path(temp) / "willington"
        staged.mkdir()
        with zipfile.ZipFile(bundle) as archive:
            members = [member for member in archive.infolist() if not member.is_dir()]
            names = [member.filename for member in members]
            if len(set(names)) != len(names):
                raise ValueError("the bundle holds a file twice")
            for member in members:
                name = member.filename
                # The release check refuses anything else that isn't a runtime file, once it's all in place.
                if (name.startswith("/") or "\\" in name or ".." in PurePosixPath(name).parts or stat.S_ISLNK(member.external_attr >> 16)
                        or name in (*release.WILLINGTON_LICENSES, "release.json")):
                    raise ValueError(f"the bundle holds {name!r}, which can't go in {release.WILLINGTON}")
                path = staged / name
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_bytes(archive.read(member))
        shutil.copyfile(license_file, staged / license_file.name)
        files = release.inventory(staged)
        manifest = {"schema": release.WILLINGTON_SCHEMA, "version": version or commit[:12], "commit": commit, "files": files}
        (staged / "release.json").write_text(json.dumps(manifest, indent=2, sort_keys=True) + "\n", encoding="utf-8")
        release.willington_files(staged)
        if folder.exists():
            shutil.rmtree(folder)
        staged.rename(folder)
    return files


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("bundle", type=Path, help="Willington-matrix.zip from a Bundle run on Willington's main")
    parser.add_argument("--license", type=Path, required=True, help="Willington's LICENSE.md (or LICENSE) at that commit")
    parser.add_argument("--commit", required=True, help="the Willington commit the run built")
    parser.add_argument("--version", help="what release.json calls this Willington; the commit's first 12 characters by default")
    args = parser.parse_args()
    files = vendor(args.bundle, args.license, args.commit, args.version)
    natives = ", ".join(f"{sum(name.endswith(suffix) for name in files)} for {platform}"
                        for platform, suffix in release.WILLINGTON_NATIVES.items())
    print(f"{release.WILLINGTON}: {len(files)} files from Willington {args.commit}, native libraries {natives}")


if __name__ == "__main__":
    main()
