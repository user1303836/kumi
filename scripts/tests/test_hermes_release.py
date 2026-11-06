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
LISTING = {"serve_operations": ["change", "connect", "generate", "status"],
           "generate_kinds": ["audio_to_midi", "sfx"], "ledger_policy_keys": ["audio_to_midi", "sfx"],
           "listen_sources": {"ableton": ["artifact", "file", "sound"], "reaper": ["artifact", "file", "sound", "track"],
                              "renoise": []}}


def run(github: FakeGitHub, *args: str, probe=None, lister=None) -> tuple[int, str]:
    """hermes_pin.py's main on `github`, with `probe` and `lister` standing in for running the runtime (USAGE and
    LISTING by default)."""
    err = io.StringIO()
    with contextlib.redirect_stderr(err), contextlib.redirect_stdout(io.StringIO()):
        code = pin.main(list(args), github=github, probe=probe or (lambda kumi, scratch: USAGE),
                        lister=lister or (lambda kumi, scratch, version: LISTING))
    return code, err.getvalue()


def generate(kinds: list[str] | None, keys: list[str] | None) -> dict:
    """A `generate` tool as tools/list describes it: its input schema's `kind` enum and its `_meta` ledger policy keys
    (`None`: left out)."""
    tool: dict = {"name": "generate", "inputSchema": {"type": "object", "properties": {}}}
    if kinds is not None:
        tool["inputSchema"]["properties"]["kind"] = {"type": "string", "enum": kinds}
    if keys is not None:
        tool["_meta"] = {"kumi/ledger_policy_keys": keys}
    return tool


# serve's listen as tools/list describes it: a source that's a file, a revision-scoped sound or an artifact, and since
# CC-2 also one with a track or the mix over a beat window.
FILE_SOURCE = {"type": "object", "properties": {"file": {"type": "string"}, "sound_id": {"type": "string"},
                                                "map_revision": {"type": "string"}, "artifact_id": {"type": "string"}}}
FILES_ONLY = {"type": "object", "required": ["source"], "properties": {"source": FILE_SOURCE, "compare_to": FILE_SOURCE}}
WITH_TRACK = {"type": "object", "oneOf": [FILES_ONLY, {
    "type": "object", "required": ["source"], "properties": {"source": {
        "type": "object", "required": ["track", "from_beat", "beats"],
        "properties": {"track": {"oneOf": [{"required": ["ref"]}, {"required": ["mix"]}]},
                       "from_beat": {"type": "number"}, "beats": {"type": "number"}}}}}]}


def listen(schema: dict, sources: object = None) -> dict:
    """A `listen` tool with `schema` as its input schema and `sources` as its `_meta` kumi/listen_sources (`None`: left
    out, as a runtime from before core-gap item 2 does)."""
    tool: dict = {"name": "listen", "inputSchema": schema}
    if sources is not None:
        tool["_meta"] = {"kumi/listen_sources": sources}
    return tool


# Each DAW lists something the others don't, so only a union over the three gets it all. Each DAW's listen is its own:
# Live's from a runtime that declares nothing (files only, from the schema), REAPER's declared with a track.
TOOLS = {"ableton": [{"name": "status"}, {"name": "change"}, listen(FILES_ONLY)],
         "reaper": [{"name": "status"}, {"name": "connect"}, generate(["sfx", "audio_to_midi"], ["sfx", "audio_to_midi"]),
                    listen(WITH_TRACK, ["track", "file", "sound", "artifact"])],
         "renoise": [{"name": "status"}, generate(["video_to_sfx", "sfx"], ["video_to_sfx"])]}


def stand_in(version: str = VERSION, tools: dict[str, list[dict]] | None = None) -> bytes:
    """A POSIX stand-in for the pinned `kumi`: `serve <unknown>` prints USAGE and exits 2; `serve --stdio … --daw D`
    answers MCP's initialize as kumi-serve `version` and tools/list with `tools[D]`. Either way it exits 9 unless it
    runs with the scratch HOME and no update check."""
    tools = TOOLS if tools is None else tools
    return f"""#!/usr/bin/env python3
import json, os, sys
if not os.environ.get("HOME", "").endswith("probe-home") or os.environ.get("KUMI_NO_UPDATE_CHECK") != "1":
    sys.exit(9)
args = sys.argv[1:]
if args[:2] == ["serve", {pin.PROBE!r}]:
    sys.stderr.write({USAGE!r})
    sys.exit(2)
if args[:2] != ["serve", "--stdio"]:
    sys.exit(9)
daw = args[args.index("--daw") + 1]
for line in sys.stdin:
    message = json.loads(line)
    if message.get("id") == 1:
        result = {{"protocolVersion": {pin.MCP_PROTOCOL!r}, "serverInfo": {{"name": "kumi-serve", "version": {version!r}}}}}
    elif message.get("id") == 2:
        result = {{"tools": {tools!r}.get(daw, [])}}
    else:
        continue
    print(json.dumps({{"jsonrpc": "2.0", "id": message["id"], "result": result}}), flush=True)
""".encode()


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
        for edit in ("sha256", "serve_options", "serve_operations", "generate_kinds", "ledger_policy_keys",
                     "listen_sources"):
            with self.subTest(edit):
                edited = copy.deepcopy(written)
                if edit == "sha256":
                    edited["targets"]["x86_64-apple-darwin"]["sha256"] = "e" * 64
                elif edit == "serve_options":
                    edited["serve_options"] = [*OPTIONS, "--newer-than-the-release"]
                elif edit == "listen_sources":
                    edited["listen_sources"]["ableton"] = ["artifact", "file", "sound", "track"]
                else:
                    edited[edit] = [*LISTING[edit], "newer_than_the_release"]
                self.out.write_text(json.dumps(edited))
                code, err = run(self.github, VERSION, "--out", str(self.out), "--check")
                self.assertEqual(code, 1)
                self.assertIn("doesn't match the published v1.24.66", err)

    def test_the_pin_records_what_kumi_serve_lists_from_this_computers_runtime(self) -> None:
        given: list[tuple[bytes, str]] = []

        def lister(kumi: Path, scratch: Path, version: str) -> dict[str, list[str]]:
            given.append((kumi.read_bytes(), version))
            return LISTING

        code, err = run(self.github, VERSION, "--out", str(self.out), lister=lister)
        self.assertEqual(code, 0, err)
        written = json.loads(self.out.read_text())
        self.assertEqual({key: written[key] for key in LISTING}, LISTING)
        self.assertEqual(given, [(f"kumi for {pin.host_target()}".encode(), VERSION)], "run once, told its version")

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

    @unittest.skipIf(os.name == "nt", "the stand-in runtime is a POSIX script")
    def test_the_runtime_itself_is_run_with_a_scratch_home_for_its_usage_and_each_daws_listing(self) -> None:
        target = pin.host_target()
        self.github.replace_bundle(target, tar_xz({**files(target), pin.executable(target): stand_in()}))
        code, err = run(self.github, VERSION, "--out", str(self.out), probe=pin.serve_usage, lister=pin.serve_listing)
        self.assertEqual(code, 0, err)
        written = json.loads(self.out.read_text())
        self.assertEqual(written["serve_options"], OPTIONS)
        self.assertEqual((written["serve_operations"], written["generate_kinds"], written["ledger_policy_keys"]),
                         (["change", "connect", "generate", "listen", "status"],
                          ["audio_to_midi", "sfx", "video_to_sfx"], ["audio_to_midi", "sfx", "video_to_sfx"]),
                         "sorted unions over the DAWs: what one DAW lists counts")
        self.assertEqual(written["listen_sources"],
                         {"ableton": ["artifact", "file", "sound"], "reaper": ["artifact", "file", "sound", "track"],
                          "renoise": []},
                         "each DAW's own, sorted: Live's read from its files-only schema, REAPER's as it declares "
                         "them, nothing where no listen is listed")
        self.github = FakeGitHub()
        self.github.replace_bundle(target, tar_xz({**files(target), pin.executable(target): stand_in(
            tools={daw: [{"name": "status"}] for daw in pin.DAWS})}))
        self.assertEqual(run(self.github, VERSION, "--out", str(self.out), probe=pin.serve_usage,
                             lister=pin.serve_listing)[0], 0)
        written = json.loads(self.out.read_text())
        self.assertEqual((written["generate_kinds"], written["ledger_policy_keys"]), ([], []), "no generate: none")
        self.assertEqual(written["listen_sources"], {daw: [] for daw in pin.DAWS}, "no listen: nothing")

    @unittest.skipIf(os.name == "nt", "the stand-in runtime is a POSIX script")
    def test_a_runtime_that_isnt_the_pinned_one_or_doesnt_say_what_its_operations_take_is_refused(self) -> None:
        target = pin.host_target()
        status = [{"name": "status"}]
        for runtime, said in ((stand_in(version="1.24.65"), "not kumi-serve 1.24.66"),
                              (stand_in(tools={}), "listed no operations"),
                              (stand_in(tools={**TOOLS, "renoise": [*status, generate(None, ["sfx"])]}),
                               "generate enumerates no kinds"),
                              (stand_in(tools={**TOOLS, "renoise": [*status, generate([], ["sfx"])]}),
                               "generate enumerates no kinds"),
                              (stand_in(tools={**TOOLS, "renoise": [*status, generate(["sfx"], None)]}),
                               "names no kumi/ledger_policy_keys"),
                              (stand_in(tools={**TOOLS, "renoise": [*status, generate(["sfx"], [])]}),
                               "names no kumi/ledger_policy_keys"),
                              (stand_in(tools={**TOOLS, "renoise": [*status, listen(WITH_TRACK)]}),
                               "offers a track or the mix but doesn't say"),
                              (stand_in(tools={**TOOLS, "renoise": [*status, listen(FILES_ONLY, [])]}),
                               "names no kumi/listen_sources"),
                              (stand_in(tools={**TOOLS, "renoise": [*status, listen(FILES_ONLY, ["file", "stems"])]}),
                               "sources this script doesn't know: stems"),
                              (stand_in(tools={**TOOLS, "renoise": [*status, listen({"type": "object"})]}),
                               "names no source it takes")):
            with self.subTest(said):
                self.github = FakeGitHub()
                self.out.unlink(missing_ok=True)
                self.github.replace_bundle(target, tar_xz({**files(target), pin.executable(target): runtime}))
                code, err = run(self.github, VERSION, "--out", str(self.out), probe=pin.serve_usage,
                                lister=pin.serve_listing)
                self.assertEqual(code, 1, err)
                self.assertIn(said, err)
                self.assertFalse(self.out.exists(), "a refused release must leave no pin behind")

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
