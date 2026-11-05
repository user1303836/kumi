"""scripts/hermes_pin.py: the Kumi release pin the Hermes plugin downloads from, checked against every record of it."""
import contextlib
import copy
import hashlib
import importlib.util
import io
import json
import os
from pathlib import Path
import tarfile
import tempfile
import unittest

SPEC = importlib.util.spec_from_file_location("hermes_pin", Path(__file__).parents[1] / "hermes_pin.py")
pin = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(pin)

VERSION, REPOSITORY = "1.24.66", "user1303836/kumi"
TAG = f"v{VERSION}"
# A plugin.yaml as the plugin has it: the pinned version on one top-level line, and a nested key of the same name.
MANIFEST = 'name: kumi\n# The Kumi release the plugin pins\nversion: "1.24.60"\nconfig_schema:\n  daw:\n    version: "keep"\n'


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


# What a pinned `kumi serve` prints for an argument it doesn't take: the error line (which quotes it, and names --help),
# the usage line, its indented continuations, then a line that isn't the usage's (which names --verbose).
USAGE = (f'kumi serve: unknown argument "{pin.PROBE}" (try --help)\n'
         "usage: kumi serve --stdio --state-dir <folder> --kumi-home <folder> --daw <ableton|reaper|renoise>\n"
         "                  [--library-root <folder>]... [--media-root <folder>]... [--tool <name>=<path>]...\n"
         "                  [--hermes-home <folder>] [--hermes-root <folder>]\n"
         "Kumi's runtime for a host such as Hermes; see --verbose\n")
OPTIONS = ["--daw", "--hermes-home", "--hermes-root", "--kumi-home", "--library-root", "--media-root", "--state-dir",
           "--stdio", "--tool"]


def run(github: FakeGitHub, *args: str, probe=None) -> tuple[int, str]:
    """hermes_pin.py's main on `github`, with `probe` standing in for running the runtime (USAGE by default)."""
    err = io.StringIO()
    with contextlib.redirect_stderr(err), contextlib.redirect_stdout(io.StringIO()):
        code = pin.main(list(args), github=github, probe=probe or (lambda kumi, scratch: USAGE))
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
        written = json.loads(self.out.read_text())
        for edit in ("sha256", "serve_options"):
            with self.subTest(edit):
                edited = copy.deepcopy(written)
                if edit == "sha256":
                    edited["targets"]["x86_64-apple-darwin"]["sha256"] = "e" * 64
                else:
                    edited["serve_options"] = [*OPTIONS, "--newer-than-the-release"]
                self.out.write_text(json.dumps(edited))
                code, err = run(self.github, VERSION, "--out", str(self.out), "--check")
                self.assertEqual(code, 1)
                self.assertIn("doesn't match the published v1.24.66", err)

    def test_the_pin_records_the_options_kumi_serve_takes_from_its_usage_lines_only(self) -> None:
        given: list[bytes] = []

        def probe(kumi: Path, scratch: Path) -> str:
            given.append(kumi.read_bytes())
            return USAGE

        code, err = run(self.github, VERSION, "--out", str(self.out), probe=probe)
        self.assertEqual(code, 0, err)
        self.assertEqual(json.loads(self.out.read_text())["serve_options"], OPTIONS,
                         "sorted; neither the error line's --help nor the line after the usage's --verbose")
        self.assertEqual(given, [f"kumi for {pin.host_target()}".encode()], "this computer's runtime, run once")

    def test_a_runtime_whose_usage_names_no_options_is_refused(self) -> None:
        for printed, said in (("kumi: unknown command\n", "no usage line"),
                              ("usage: kumi serve\n", "names no options"),
                              (f'error: "{pin.PROBE}"\nusage: kumi serve {pin.PROBE}\n', "names no options")):
            with self.subTest(printed):
                code, err = run(self.github, VERSION, "--out", str(self.out), probe=lambda kumi, scratch: printed)
                self.assertEqual(code, 1, err)
                self.assertIn(said, err)
                self.assertFalse(self.out.exists(), "a refused release must leave no pin behind")

    @unittest.skipIf(os.name == "nt", "the stand-in runtime is a POSIX shell script")
    def test_the_runtime_itself_is_run_with_a_scratch_home_to_read_its_usage(self) -> None:
        target = pin.host_target()
        stand_in = (f'#!/bin/sh\n[ "$1 $2" = "serve {pin.PROBE}" ] || exit 9\n'
                    'case "$HOME" in *probe-home) ;; *) exit 8 ;; esac\n'
                    f"cat >&2 <<'USAGE'\n{USAGE}USAGE\nexit 2\n").encode()
        self.github.replace_bundle(target, tar_xz({**files(target), pin.executable(target): stand_in}))
        code, err = run(self.github, VERSION, "--out", str(self.out), probe=pin.serve_usage)
        self.assertEqual(code, 0, err)
        self.assertEqual(json.loads(self.out.read_text())["serve_options"], OPTIONS)

    def test_a_downloaded_bundle_that_differs_from_the_records_is_refused(self) -> None:
        self.github.files["kumi-x86_64-apple-darwin.tar.xz"] = tar_xz({"kumi": b"tampered"})
        self.assertIn("downloaded kumi-x86_64-apple-darwin.tar.xz doesn't match", self.refused())

    def test_a_bundle_setup_would_not_extract_is_refused(self) -> None:
        target = "aarch64-apple-darwin"
        bad = {"no Kumi executable": tar_xz({"README.md": b"readme"}),
               "only a lookalike of it": tar_xz({".kumi": b"k", "bin/kumi": b"k"}),
               "a symlink": tar_xz(files(target), links={"escape": "/etc/passwd"}),
               "a climbing path": tar_xz({**files(target), "../outside": b"x"}),
               "not an archive": b"plain bytes"}
        for problem, data in bad.items():
            with self.subTest(problem):
                self.github = FakeGitHub()
                self.out.unlink(missing_ok=True)
                self.github.replace_bundle(target, data)
                self.assertIn(f"kumi-{target}.tar.xz", self.refused())

    def manifest(self, text: str) -> Path:
        """A plugin.yaml holding `text` beside the pin, and no pin yet."""
        self.out.unlink(missing_ok=True)
        path = self.out.parent / "plugin.yaml"
        path.write_bytes(text.encode())
        return path

    def test_the_pin_sets_the_manifests_version_line_and_nothing_else(self) -> None:
        for newline in ("\n", "\r\n"):
            with self.subTest(newline=repr(newline)):
                path = self.manifest(MANIFEST.replace("\n", newline))
                code, err = run(self.github, VERSION, "--out", str(self.out))
                self.assertEqual(code, 0, err)
                expected = MANIFEST.replace('"1.24.60"', f'"{VERSION}"').replace("\n", newline)
                self.assertEqual(path.read_bytes().decode(), expected)
                self.assertEqual(run(self.github, VERSION, "--out", str(self.out), "--check")[0], 0)

    def test_a_manifest_without_exactly_one_version_line_leaves_both_files_alone(self) -> None:
        for problem, text in {"none": "name: kumi\n", "two": 'version: "1.0.0"\nversion: "2.0.0"\n',
                              "unquoted": "version: 1.24.60\n", "only nested": 'a:\n  version: "1.0.0"\n'}.items():
            with self.subTest(problem):
                path = self.manifest(text)
                self.assertIn("exactly one", self.refused())
                self.assertEqual(path.read_bytes().decode(), text)

    def test_check_fails_when_the_manifest_names_another_version(self) -> None:
        path = self.manifest(MANIFEST)
        self.assertEqual(run(self.github, VERSION, "--out", str(self.out))[0], 0)
        path.write_bytes(MANIFEST.encode())  # the manifest's version changed by hand afterwards
        code, err = run(self.github, VERSION, "--out", str(self.out), "--check")
        self.assertEqual(code, 1, err)
        self.assertIn('says version "1.24.60"', err)


if __name__ == "__main__":
    unittest.main()
