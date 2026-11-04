"""scripts/hermes_pin.py: the Kumi release pin the Hermes plugin downloads from, checked against every record of it."""
import contextlib
import copy
import hashlib
import importlib.util
import io
import json
from pathlib import Path
import tarfile
import tempfile
import unittest

SPEC = importlib.util.spec_from_file_location("hermes_pin", Path(__file__).parents[1] / "hermes_pin.py")
pin = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(pin)

VERSION, REPOSITORY = "1.24.66", "user1303836/kumi"
TAG = f"v{VERSION}"


def tar_xz(members: dict[str, bytes], links: dict[str, str] | None = None) -> bytes:
    """A .tar.xz holding `members` as files, and `links` as symlinks (name: target)."""
    out = io.BytesIO()
    with tarfile.open(fileobj=out, mode="w:xz") as archive:
        for name, data in members.items():
            info = tarfile.TarInfo(name)
            info.size = len(data)
            archive.addfile(info, io.BytesIO(data))
        for name, target in (links or {}).items():
            info = tarfile.TarInfo(name)
            info.type, info.linkname = tarfile.SYMTYPE, target
            archive.addfile(info)
    return out.getvalue()


def files(target: str) -> dict[str, bytes]:
    return {pin.executable(target): f"kumi for {target}".encode(), "daw-mcp-server": b"host", "README.md": b"readme"}


def unpacked(target: str) -> int:
    return sum(len(data) for data in files(target).values())


class FakeGitHub:
    """A published release, as GitHub and its assets would return it; tests change it to break one thing."""

    def __init__(self) -> None:
        self.repository = REPOSITORY
        self.files = {f"kumi-{target}.tar.xz": tar_xz(files(target)) for target in pin.TARGETS}
        digest = {target: hashlib.sha256(self.files[f"kumi-{target}.tar.xz"]).hexdigest() for target in pin.TARGETS}
        self.digest = digest
        entries = {target: {"kumi": VERSION, "bundle": f"kumi-{target}.tar.gz", "sha256": "0" * 64,
                            "bundle_xz": f"kumi-{target}.tar.xz", "sha256_xz": digest[target],
                            "runtime": "rust-native", "target": target, "bridge": "1.0.104", "reaper": "0.1.12",
                            "renoise": "1.0.19"} for target in pin.TARGETS}
        self.manifest = {"kumi": VERSION, "bundle": "kumi.tar.gz", "sha256": "1" * 64, "bridge": "1.0.104",
                         "targets": entries}
        self.per_target = {target: dict(entry) for target, entry in entries.items()}
        self.sums = "".join(f"{digest[target]}  kumi-{target}.tar.xz\n" for target in pin.TARGETS)
        self.release_data = {
            "tag_name": TAG, "draft": False, "prerelease": False,
            "assets": [{"name": f"kumi-{target}.tar.xz", "size": len(self.files[f"kumi-{target}.tar.xz"]),
                        "digest": f"sha256:{digest[target]}",
                        "browser_download_url": f"https://github.com/{REPOSITORY}/releases/download/{TAG}/kumi-{target}.tar.xz"}
                       for target in pin.TARGETS]}

    def replace_bundle(self, target: str, data: bytes) -> None:
        """Publish a different bundle consistently everywhere, as a release made from it would."""
        name, digest = f"kumi-{target}.tar.xz", hashlib.sha256(data).hexdigest()
        self.files[name] = data
        self.sums = self.sums.replace(self.digest[target], digest)
        self.manifest["targets"][target]["sha256_xz"] = self.per_target[target]["sha256_xz"] = digest
        asset = next(asset for asset in self.release_data["assets"] if asset["name"] == name)
        asset.update(size=len(data), digest=f"sha256:{digest}")
        self.digest[target] = digest

    def release(self, tag: str) -> dict:
        return copy.deepcopy(self.release_data)

    def asset(self, tag: str, name: str) -> bytes:
        if name == "kumi-release.json":
            return json.dumps(self.manifest).encode()
        if name == "SHA256SUMS":
            return self.sums.encode()
        target = name.removeprefix("kumi-release-").removesuffix(".json")
        return json.dumps(self.per_target[target]).encode()

    def download(self, tag: str, name: str, into: Path) -> Path:
        path = into / name
        path.write_bytes(self.files[name])
        return path


def run(github: FakeGitHub, *args: str) -> tuple[int, str]:
    err = io.StringIO()
    with contextlib.redirect_stderr(err), contextlib.redirect_stdout(io.StringIO()):
        code = pin.main(list(args), github=github)
    return code, err.getvalue()


class PinTest(unittest.TestCase):
    def setUp(self) -> None:
        self.dir = tempfile.TemporaryDirectory()
        self.out = Path(self.dir.name) / "kumi_release.json"
        self.github = FakeGitHub()

    def tearDown(self) -> None:
        self.dir.cleanup()

    def refused(self, *args: str) -> str:
        code, err = run(self.github, VERSION, "--out", str(self.out), *args)
        self.assertEqual(code, 1, err)
        self.assertFalse(self.out.exists(), "a refused release must leave no pin behind")
        return err

    def test_a_consistent_release_pins_every_target_with_its_url_digest_and_sizes(self) -> None:
        code, err = run(self.github, VERSION, "--out", str(self.out))
        self.assertEqual(code, 0, err)
        written = json.loads(self.out.read_text())
        self.assertEqual((written["schema"], written["kumi"], written["tag"]), (1, VERSION, TAG))
        self.assertEqual(list(written["targets"]), list(pin.TARGETS))
        target = "aarch64-apple-darwin"
        self.assertEqual(written["targets"][target], {
            "url": f"https://github.com/{REPOSITORY}/releases/download/{TAG}/kumi-{target}.tar.xz",
            "sha256": self.github.digest[target], "size": len(self.github.files[f"kumi-{target}.tar.xz"]),
            "bridge": "1.0.104", "reaper": "0.1.12", "renoise": "1.0.19", "unpacked_bytes": unpacked(target)})
        self.assertEqual(written["targets"]["x86_64-pc-windows-msvc"]["unpacked_bytes"], unpacked("x86_64-pc-windows-msvc"))

    def test_the_pin_names_one_exact_release(self) -> None:
        for version in ("latest", "1.24", "1.24.66-rc1", ""):
            code, err = run(self.github, version, "--out", str(self.out))
            self.assertEqual(code, 1, version)
            self.assertIn("isn't a version", err)

    def test_a_draft_or_prerelease_is_refused(self) -> None:
        for flag in ("draft", "prerelease"):
            self.github = FakeGitHub()
            self.github.release_data[flag] = True
            self.assertIn("draft or a prerelease", self.refused())

    def test_a_checksum_that_any_source_disagrees_on_is_refused(self) -> None:
        target, wrong = "x86_64-pc-windows-msvc", "f" * 64
        breaks = {
            "SHA256SUMS": lambda github: setattr(github, "sums", github.sums.replace(github.digest[target], wrong)),
            "its target's file": lambda github: github.per_target[target].update(sha256_xz=wrong),
            "GitHub's digest": lambda github: github.release_data["assets"][3].update(digest=f"sha256:{wrong}"),
        }
        for source, broken in breaks.items():
            with self.subTest(source):
                self.github = FakeGitHub()
                broken(self.github)
                self.assertIn("kumi-x86_64-pc-windows-msvc.tar.xz", self.refused())

    def test_a_release_missing_a_target_or_its_upload_is_refused(self) -> None:
        del self.github.manifest["targets"]["aarch64-unknown-linux-gnu"]
        self.assertIn("no bundle for aarch64-unknown-linux-gnu", self.refused())
        self.github = FakeGitHub()
        self.github.release_data["assets"].pop()
        self.assertIn("has no kumi-x86_64-unknown-linux-gnu.tar.xz uploaded", self.refused())

    def test_a_manifest_for_another_version_or_a_url_elsewhere_is_refused(self) -> None:
        self.github.manifest["kumi"] = "1.24.65"
        self.assertIn("not 1.24.66", self.refused())
        self.github = FakeGitHub()
        self.github.release_data["assets"][0]["browser_download_url"] = "https://example.com/kumi-aarch64-apple-darwin.tar.xz"
        self.assertIn("downloads from", self.refused())

    def test_check_passes_on_the_written_pin_and_fails_on_an_edited_one(self) -> None:
        self.assertEqual(run(self.github, VERSION, "--out", str(self.out))[0], 0)
        self.assertEqual(run(self.github, VERSION, "--out", str(self.out), "--check")[0], 0)
        edited = json.loads(self.out.read_text())
        edited["targets"]["x86_64-apple-darwin"]["sha256"] = "e" * 64
        self.out.write_text(json.dumps(edited))
        code, err = run(self.github, VERSION, "--out", str(self.out), "--check")
        self.assertEqual(code, 1)
        self.assertIn("doesn't match the published v1.24.66", err)

    def test_a_downloaded_bundle_that_differs_from_the_records_is_refused(self) -> None:
        self.github.files["kumi-x86_64-apple-darwin.tar.xz"] = tar_xz({"kumi": b"tampered"})
        self.assertIn("downloaded kumi-x86_64-apple-darwin.tar.xz doesn't match", self.refused())

    def test_a_bundle_setup_would_not_extract_is_refused(self) -> None:
        target = "aarch64-apple-darwin"
        bad = {"no Kumi executable": tar_xz({"README.md": b"readme"}),
               "a symlink": tar_xz(files(target), links={"escape": "/etc/passwd"}),
               "a climbing path": tar_xz({**files(target), "../outside": b"x"}),
               "not an archive": b"plain bytes"}
        for problem, data in bad.items():
            with self.subTest(problem):
                self.github = FakeGitHub()
                self.github.replace_bundle(target, data)
                self.assertIn(f"kumi-{target}.tar.xz", self.refused())


if __name__ == "__main__":
    unittest.main()
