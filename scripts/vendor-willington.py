#!/usr/bin/env python3
"""Put a Bundle run's Willington files in vendor/willington, or check that they're what's there.

    python3 scripts/vendor-willington.py --run <run>     # replace vendor/willington with the run's files
    python3 scripts/vendor-willington.py --check <run>   # check vendor/willington is exactly the run's files

The run must be a successful Bundle run on a push to Willington's main, and its commit still on main. Its
Willington-matrix artifact must match the digest GitHub recorded, and the bundle inside it its SHA-256. With
Willington's license at that commit and a release.json naming each file, the folder is checked the way a
release checks it before it replaces the old one, so a refused run leaves vendor/willington as it was.
docs/en/DEVELOPER_GUIDE.md has the whole update.
"""
import argparse
import hashlib
import importlib.util
import io
import json
import re
import shutil
import stat
import subprocess
import sys
import tempfile
import zipfile
from pathlib import Path, PurePosixPath

ROOT = Path(__file__).resolve().parents[1]
# One copy of the release script, shared with its tests, so a patch on either reaches both.
release = sys.modules.get("native_release")
if release is None:
    SPEC = importlib.util.spec_from_file_location("native_release", ROOT / "scripts/build-native-release.py")
    release = importlib.util.module_from_spec(SPEC)
    sys.modules["native_release"] = release
    SPEC.loader.exec_module(release)

REPOSITORY = "xonedsp/willington"
WORKFLOW = ".github/workflows/bundle.yml"
ARTIFACT = "Willington-matrix"


def gh(path: str, *headers: str) -> bytes:
    """A GitHub API response's body, through gh: an HTTP error raises rather than returning its error page."""
    command = ["gh", "api", path]
    for header in headers:
        command += ["-H", header]
    result = subprocess.run(command, capture_output=True)
    if result.returncode != 0:
        raise RuntimeError(f"gh api {path} failed: {result.stderr.decode(errors='replace').strip()}")
    return result.stdout


def fetch(run_id: int, api=gh) -> dict:
    """A Bundle run on Willington's main: its commit, bundle and license, and the run and artifact to name.

    Refuses a run from another event, branch, repository or workflow, one that didn't succeed, a commit that
    isn't on main, an artifact that doesn't match its digest and a bundle that doesn't match its SHA-256.
    """
    run = json.loads(api(f"repos/{REPOSITORY}/actions/runs/{run_id}"))
    expected = {"event": "push", "head_branch": "main", "path": WORKFLOW, "status": "completed", "conclusion": "success",
                "head_repository": REPOSITORY, "repository": REPOSITORY}
    actual = {key: (run.get(key) or {}).get("full_name") if key.endswith("repository") else run.get(key) for key in expected}
    wrong = [f"{key} {actual[key]!r}" for key in expected if actual[key] != expected[key]]
    if wrong:
        raise ValueError(f"run {run_id} isn't a successful Bundle run on a push to {REPOSITORY}'s main: {', '.join(wrong)}")
    commit = run.get("head_sha")
    if not isinstance(commit, str) or not re.fullmatch("[0-9a-f]{40}", commit):
        raise ValueError(f"run {run_id} names no commit")
    # identical: main is at the commit; behind: the commit is in main's history.
    status = json.loads(api(f"repos/{REPOSITORY}/compare/main...{commit}")).get("status")
    if status not in ("identical", "behind"):
        raise ValueError(f"run {run_id} built {commit}, which isn't on {REPOSITORY}'s main ({status})")
    listed = json.loads(api(f"repos/{REPOSITORY}/actions/runs/{run_id}/artifacts?per_page=100"))["artifacts"]
    artifacts = [artifact for artifact in listed if artifact.get("name") == ARTIFACT]
    if len(artifacts) != 1:
        raise ValueError(f"run {run_id} has {len(artifacts)} {ARTIFACT} artifacts, not one")
    artifact = artifacts[0]
    if artifact.get("expired"):
        raise ValueError(f"run {run_id}'s {ARTIFACT} has expired: rerun it with gh run rerun {run_id} -R {REPOSITORY}")
    archive = api(f"repos/{REPOSITORY}/actions/artifacts/{artifact['id']}/zip")
    digest = "sha256:" + hashlib.sha256(archive).hexdigest()
    if digest != artifact.get("digest"):
        raise ValueError(f"{ARTIFACT} {artifact['id']} is {digest}, not the {artifact.get('digest')} GitHub recorded")
    with zipfile.ZipFile(io.BytesIO(archive)) as outer:
        if sorted(outer.namelist()) != [ARTIFACT + ".zip", ARTIFACT + ".zip.sha256"]:
            raise ValueError(f"{ARTIFACT} {artifact['id']} holds {', '.join(sorted(outer.namelist()))}")
        bundle = outer.read(ARTIFACT + ".zip")
        recorded = outer.read(ARTIFACT + ".zip.sha256").decode("ascii").split()
    if recorded[:1] != [hashlib.sha256(bundle).hexdigest()]:
        raise ValueError(f"{ARTIFACT}.zip doesn't match its .sha256")
    names = {entry["name"] for entry in json.loads(api(f"repos/{REPOSITORY}/contents?ref={commit}")) if entry.get("type") == "file"}
    licenses = {name: api(f"repos/{REPOSITORY}/contents/{name}?ref={commit}", "Accept: application/vnd.github.raw")
                for name in release.WILLINGTON_LICENSES if name in names}
    if not licenses:
        raise ValueError(f"{REPOSITORY} has no {' or '.join(release.WILLINGTON_LICENSES)} at {commit}")
    return {"run": run_id, "url": run.get("html_url"), "commit": commit, "artifact": artifact["id"], "digest": digest,
            "bundle": bundle, "licenses": licenses}


def stage(bundle: bytes, licenses: dict[str, bytes], commit: str, version: str, folder: Path) -> dict[str, str]:
    """Write the bundle's files, the licenses and release.json into a new folder, checked as a release checks it."""
    folder.mkdir()
    with zipfile.ZipFile(io.BytesIO(bundle)) as archive:
        members = [member for member in archive.infolist() if not member.is_dir()]
        names = [member.filename for member in members]
        if len(set(names)) != len(names):
            raise ValueError("the bundle holds a file twice")
        # macOS and Windows keep one file for two names one letter's case apart.
        if len({name.lower() for name in names}) != len(names):
            raise ValueError("the bundle holds two files whose names differ only in case")
        for member in members:
            name = member.filename
            parts = PurePosixPath(name).parts
            # Only files inside Willington's runtime folders; the release check refuses any that aren't runtime files.
            if (name.startswith("/") or "\\" in name or ".." in parts or len(parts) < 2 or parts[0] not in release.WILLINGTON_COMPONENTS
                    or stat.S_ISLNK(member.external_attr >> 16)):
                raise ValueError(f"the bundle holds {name!r}, which can't go in {release.WILLINGTON}")
            path = folder / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(archive.read(member))
    for name, content in licenses.items():
        if name not in release.WILLINGTON_LICENSES:
            raise ValueError(f"Willington's license notice is {' or '.join(release.WILLINGTON_LICENSES)}, not {name}")
        (folder / name).write_bytes(content)
    files = release.inventory(folder)
    manifest = {"schema": release.WILLINGTON_SCHEMA, "version": version, "commit": commit, "files": files}
    # Keep --run/--check byte-identical across Windows and POSIX checkouts.
    (folder / "release.json").write_text(json.dumps(manifest, indent=2, sort_keys=True) + "\n", encoding="utf-8", newline="\n")
    release.willington_files(folder)
    return files


def vendor(bundle: bytes, licenses: dict[str, bytes], commit: str, version: str | None = None, root: Path = ROOT) -> dict[str, str]:
    """Replace root's vendor/willington with the bundle's files, the licenses and release.json; each file's SHA-256."""
    folder = root / release.WILLINGTON
    folder.parent.mkdir(parents=True, exist_ok=True)
    # Staged beside the folder, so the finished one moves into place on the same disk.
    with tempfile.TemporaryDirectory(dir=folder.parent, prefix=".willington-") as temp:
        staged = Path(temp) / "willington"
        files = stage(bundle, licenses, commit, version or commit[:12], staged)
        if folder.exists():
            shutil.rmtree(folder)
        staged.rename(folder)
    return files


def check(fetched: dict, root: Path = ROOT) -> list[str]:
    """The files that differ between root's vendor/willington and the fetched run's, with its release.json's version."""
    folder = root / release.WILLINGTON
    try:
        version = json.loads((folder / "release.json").read_text(encoding="utf-8"))["version"]
    except (OSError, ValueError, KeyError, TypeError):
        raise ValueError(f"{release.WILLINGTON} has no release.json with a version to check against") from None
    with tempfile.TemporaryDirectory(prefix="kumi-willington-check-") as temp:
        staged = Path(temp) / "willington"
        stage(fetched["bundle"], fetched["licenses"], fetched["commit"], version, staged)
        expected = release.inventory(staged)
    actual = release.inventory(folder)
    return sorted(name for name in expected.keys() | actual.keys() if expected.get(name) != actual.get(name))


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    mode = parser.add_mutually_exclusive_group(required=True)
    mode.add_argument("--run", type=int, metavar="RUN", help="replace vendor/willington with this Bundle run's files")
    mode.add_argument("--check", type=int, metavar="RUN", help="check vendor/willington is exactly this Bundle run's files")
    parser.add_argument("--version", help="with --run, what release.json calls this Willington; the commit's first 12 characters by default")
    args = parser.parse_args()
    if args.check and args.version:
        parser.error("--version goes with --run: --check takes the version from release.json")
    # A refusal or a failed gh call says why in one line; anything else is a bug and keeps its traceback.
    try:
        fetched = fetch(args.run or args.check)
        if args.check:
            differences = check(fetched)
            if differences:
                sys.exit(f"{release.WILLINGTON} differs from Bundle run {args.check}: {', '.join(differences)}")
            print(f"{release.WILLINGTON} is exactly Bundle run {args.check}'s files, from Willington {fetched['commit']}.")
            return
        files = vendor(fetched["bundle"], fetched["licenses"], fetched["commit"], args.version)
    except (OSError, ValueError, RuntimeError, zipfile.BadZipFile) as error:
        sys.exit(str(error))
    natives = ", ".join(f"{sum(name.endswith(suffix) for name in files)} for {platform}"
                        for platform, suffix in release.WILLINGTON_NATIVES.items())
    print(f"{release.WILLINGTON}: {len(files)} files, native libraries {natives}")
    print(f"- Willington commit: {fetched['commit']}")
    print(f"- Bundle run: {fetched['url']}")
    print(f"- Artifact: {ARTIFACT} {fetched['artifact']}, {fetched['digest']}")


if __name__ == "__main__":
    main()
