"""setup.py (C8): status, plan, approval and install, as the plugin's tools, hook, CLI and backend use them.

No network, no Hermes, no Kumi: the release is a local .tar.xz served by a fake opener, and the runtime's setup_daw is
a function that answers the way W1's does (W1-REGISTRY.md, comments).
"""
import hashlib
import http.client
import importlib.util
import io
import json
import os
from pathlib import Path
import subprocess
import sys
import tarfile
import tempfile
import unittest
from unittest import mock

SPEC = importlib.util.spec_from_file_location("kumi_setup", Path(__file__).resolve().parents[1] / "setup.py")
setup = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(setup)
SEARCH = setup._ffmpeg_search  # the real one; each test searches only the folders it gives (see SetupTest.setUp)
FFMPEG = "ffmpeg.exe" if os.name == "nt" else "ffmpeg"

TARGET, VERSION = "aarch64-apple-darwin", "1.24.67"
URL = f"https://github.com/user1303836/kumi/releases/download/v{VERSION}/kumi-{TARGET}.tar.xz"
# Where the fake runtime says Live's User Library is: absolute on every system (a POSIX "/Users/…" isn't on Windows).
# Nothing is ever written there.
FAKE_LIBRARY = Path(tempfile.gettempdir()) / "kumi-setup-tests" / "User Library"
# Another process's setup step: it takes setup's lock on a data folder, says so, and holds it until it is killed.
HOLD_LOCK = """
import importlib.util, sys, time
from pathlib import Path
spec = importlib.util.spec_from_file_location("kumi_setup", sys.argv[1])
setup = importlib.util.module_from_spec(spec)
spec.loader.exec_module(setup)
with setup._setup_lock(Path(sys.argv[2])):
    print("locked", flush=True)
    time.sleep(60)
"""


def tar_xz(files: dict[str, bytes], links: dict[str, str] | None = None) -> bytes:
    out = io.BytesIO()
    with tarfile.open(fileobj=out, mode="w:xz") as archive:
        for name, data in files.items():
            info = tarfile.TarInfo(name)
            info.size, info.mode = len(data), 0o755
            archive.addfile(info, io.BytesIO(data))
        for name, target in (links or {}).items():
            info = tarfile.TarInfo(name)
            info.type, info.linkname = tarfile.SYMTYPE, target
            archive.addfile(info)
    return out.getvalue()


BUNDLE = tar_xz({"kumi": b"#!/bin/sh\necho kumi\n", "daw-mcp-server": b"host", "README.md": b"readme"})


class Served:
    """A fake urlopen: serves `data` for `URL` and records what was asked for."""

    def __init__(self, data: bytes) -> None:
        self.data, self.asked = data, []

    def __call__(self, url: str):
        self.asked.append(url)
        return io.BytesIO(self.data)


class Tty(io.StringIO):
    """A terminal, for `hermes kumi setup`."""

    def isatty(self) -> bool:
        return True


class Runtime:
    """The runtime's setup_daw as W1 answers it: a dry run names the paths; a real install does exactly that."""

    def __init__(self, state: str = "missing", owner: str | None = None, library: str = str(FAKE_LIBRARY),
                 **reply) -> None:
        self.state, self.owner, self.library, self.reply, self.calls = state, owner, library, reply, []

    def __call__(self, args: dict) -> dict:
        self.calls.append(args)
        reply = {"daw": args["daw"], "component": "Live Remote Script", "version": "1.0.105",
                 "paths": [f"{self.library}/Remote Scripts/AbletonMcpBridge"],
                 "state": self.state, "owner": {"kumi_home": self.owner} if self.owner else None,
                 "after": "choose_control_surface",
                 "effects": [f"Copy the AbletonMcpBridge Remote Script into {self.library}/Remote Scripts"],
                 **self.reply}
        if not args["dry_run"]:
            reply.update(state="same", installed_at=1730764800000)
        return reply


class SetupTest(unittest.TestCase):
    def setUp(self) -> None:
        self.tmp = tempfile.TemporaryDirectory()
        root = Path(self.tmp.name)
        self.plugin, self.data = root / "plugin", root / "data"
        self.plugin.mkdir()
        self.pin(BUNDLE)
        # A standalone Kumi on the test machine must not leak into plans.
        patcher = mock.patch.dict(os.environ, {"KUMI_HOME": str(root / "no-standalone-kumi")})
        patcher.start()
        self.addCleanup(patcher.stop)
        # Nor an ffmpeg on the test machine: setup searches only the folders a test puts here.
        searching = mock.patch.object(setup, "_ffmpeg_search", return_value=[])
        self.search = searching.start()
        self.addCleanup(searching.stop)

    def tearDown(self) -> None:
        self.tmp.cleanup()

    def pin(self, bundle: bytes, **changes) -> None:
        record = {"url": URL, "sha256": hashlib.sha256(bundle).hexdigest(), "size": len(bundle),
                  "unpacked_bytes": 1234, "bridge": "1.0.105"}
        record.update(changes)
        (self.plugin / "kumi_release.json").write_text(json.dumps(
            {"schema": 1, "kumi": VERSION, "repository": "user1303836/kumi", "tag": f"v{VERSION}", "targets": {TARGET: record}}))

    def plan(self, **kw) -> dict:
        self.planned = setup.plan("ableton", data_dir=self.data, plugin_dir=self.plugin, target=TARGET, **kw)
        return self.planned

    def approved(self) -> str | None:
        """The digest of the plan this test last made, as the approval hook passes it on."""
        return self.planned["digest"] if getattr(self, "planned", None) else None

    def install(self, opener, **kw) -> dict:
        kw.setdefault("expected_digest", self.approved())
        return setup.install("ableton", data_dir=self.data, plugin_dir=self.plugin, target=TARGET, opener=opener, **kw)

    def status(self) -> dict:
        return setup.status(daw="ableton", data_dir=self.data, plugin_dir=self.plugin, target=TARGET)

    def install_runtime(self) -> None:
        self.plan()
        self.assertTrue(self.install(Served(BUNDLE))["ok"])

    def adopt(self, runtime) -> dict:
        return setup.adopt("ableton", expected_digest=self.approved(), setup_daw=runtime, data_dir=self.data,
                           plugin_dir=self.plugin, target=TARGET)

    def ffmpeg(self, folder: str = "bin", content: bytes = b"#!/bin/sh\nexit 0\n") -> Path:
        """A stand-in ffmpeg (never run) in a folder setup searches; its canonical path."""
        where = self.data.parent / folder
        where.mkdir(parents=True, exist_ok=True)
        program = where / FFMPEG
        program.write_bytes(content)
        program.chmod(0o755)
        self.search.return_value = [str(where)]
        return Path(os.path.realpath(program))

    def current(self) -> dict:
        return json.loads((self.data / "runtime" / "current.json").read_text())

    def test_host_target_names_kumis_bundles_and_nothing_else(self) -> None:
        self.assertEqual(setup.host_target("Darwin", "arm64"), "aarch64-apple-darwin")
        self.assertEqual(setup.host_target("Windows", "AMD64"), "x86_64-pc-windows-msvc")
        self.assertEqual(setup.host_target("Linux", "aarch64"), "aarch64-unknown-linux-gnu")
        self.assertIsNone(setup.host_target("FreeBSD", "amd64"))
        self.assertIsNone(setup.host_target("Linux", "riscv64"))

    def test_status_before_setup_says_what_is_needed_without_writing_anything(self) -> None:
        state = self.status()
        self.assertEqual((state["state"], state["next_step"], state["pinned_version"]), ("needs_runtime", "install_runtime", VERSION))
        self.assertEqual(state["download_bytes"], len(BUNDLE))
        self.assertFalse(self.data.exists(), "status() must not create the data folder")
        (self.plugin / "kumi_release.json").unlink()
        self.assertEqual(self.status()["state"], "no_pin")
        self.assertEqual(setup.status(daw="ableton", data_dir=self.data, plugin_dir=self.plugin, target="riscv64-x")["state"], "no_pin")

    def test_status_on_a_machine_kumi_has_no_build_for(self) -> None:
        self.pin(BUNDLE)
        state = setup.status(daw="ableton", data_dir=self.data, plugin_dir=self.plugin, target="x86_64-unknown-linux-gnu")
        self.assertEqual(state["state"], "unsupported_platform")

    def test_the_runtime_plan_names_the_exact_download_and_folder_and_needs_approval(self) -> None:
        plan = self.plan()
        self.assertEqual(plan["step"], "runtime")
        self.assertEqual(plan["download"]["url"], URL)
        self.assertIn(URL, plan["effects"][0])
        self.assertEqual(plan["paths"], [str(self.data / "runtime" / VERSION)])
        asked = setup.approval("ableton", data_dir=self.data)
        self.assertIn(URL, asked["message"])
        self.assertEqual(asked["digest"], plan["digest"])
        self.assertEqual((asked["title"], asked["effects"]),
                         (f"Kumi: set up Kumi {VERSION}'s runtime", [*plan["effects"], f"Writes: {plan['paths'][0]}"]),
                         "each effect is its own line, its URL and folder in full, then the folder it writes")
        self.assertIn("reconnects at its next Kumi call", asked["message"], "it says a running runtime stops")
        self.assertIsNone(setup.approval("reaper", data_dir=self.data), "a plan for Live doesn't approve REAPER")
        later = plan["made_at"] + setup.PLAN_TTL_MS + 1
        self.assertIsNone(setup.approval("ableton", data_dir=self.data, now=later), "a stale plan approves nothing")

    def test_installing_the_runtime_verifies_extracts_and_records_it_for_runtime_py(self) -> None:
        self.plan()
        served = Served(BUNDLE)
        result = self.install(served)
        self.assertTrue(result["ok"], result)
        self.assertEqual(served.asked, [URL])
        current = json.loads((self.data / "runtime" / "current.json").read_text())
        self.assertEqual({k: current[k] for k in ("schema", "version", "target", "kumi", "sha256")},
                         {"schema": 1, "version": VERSION, "target": TARGET, "kumi": f"runtime/{VERSION}/kumi",
                          "sha256": hashlib.sha256(BUNDLE).hexdigest()})
        self.assertTrue(os.access(self.data / current["kumi"], os.X_OK))
        self.assertEqual(setup.runtime_state(self.data, VERSION, TARGET)[0], "ready")
        self.assertEqual(self.status()["state"], "needs_daw_component")
        self.assertEqual(sorted(p.name for p in (self.data / "runtime").iterdir()), ["1.24.67", "current.json"],
                         "no download, staging or old folders are left behind")

    def test_an_install_needs_an_approved_fresh_plan_and_one_plan_installs_once(self) -> None:
        self.assertEqual(self.install(Served(BUNDLE))["error"]["code"], "not_approved")
        self.plan()
        self.assertEqual(self.install(Served(BUNDLE), expected_digest=None)["error"]["code"], "not_approved")
        self.assertTrue(self.install(Served(BUNDLE))["ok"])
        self.assertEqual(self.install(Served(BUNDLE))["error"]["code"], "no_plan")

    def test_an_install_carries_out_only_the_plan_whose_digest_was_approved(self) -> None:
        approved = self.plan()
        with mock.patch.object(setup, "_now_ms", return_value=approved["made_at"] + 5):
            self.assertEqual(self.plan()["digest"], approved["digest"], "the digest is the plan's content, not its time")
        other = setup.plan("reaper", data_dir=self.data, plugin_dir=self.plugin, target=TARGET)  # another chat
        self.assertNotEqual(other["digest"], approved["digest"])
        served = Served(BUNDLE)
        self.assertEqual(self.install(served)["error"]["code"], "plan_changed")
        self.assertEqual(served.asked, [], "nothing is downloaded for a plan that wasn't approved")
        self.assertEqual(setup.install("reaper", expected_digest=other["digest"], data_dir=self.data,
                                       plugin_dir=self.plugin, target=TARGET, opener=served)["error"]["code"], "no_plan",
                         "the refused install used up the other plan: whoever approved it plans again")

    def test_a_daw_step_another_chat_planned_differently_isnt_installed_under_this_approval(self) -> None:
        self.install_runtime()
        self.plan(setup_daw=Runtime())
        elsewhere = Runtime(library=str(FAKE_LIBRARY.parent / "Other Library"))
        setup.plan("ableton", setup_daw=elsewhere, data_dir=self.data, plugin_dir=self.plugin, target=TARGET)
        self.assertEqual(self.install(Served(b""), setup_daw=elsewhere)["error"]["code"], "plan_changed")
        self.assertNotIn({"daw": "ableton", "dry_run": False}, elsewhere.calls, "nothing approved was installed")

    def test_an_approval_answered_after_the_plan_expired_is_refused_as_expired(self) -> None:
        approved = self.plan()
        served = Served(BUNDLE)
        with mock.patch.object(setup, "_now_ms", return_value=approved["made_at"] + setup.PLAN_TTL_MS + 1):
            result = self.install(served)
        self.assertEqual(result["error"]["code"], "expired", "nothing changed: the plan only got too old")
        self.assertEqual(served.asked, [])

    def test_the_daw_steps_approval_shows_each_path_it_writes_verbatim(self) -> None:
        self.install_runtime()
        paths = [str(FAKE_LIBRARY / "Remote Scripts" / "AbletonMcpBridge"),
                 str(FAKE_LIBRARY.parent / "Preferences" / "Live 12" / "My  Library.cfg"),
                 str(FAKE_LIBRARY.parent / ("long " * 180) / "Kumi")]
        self.plan(setup_daw=Runtime(paths=paths, effects=["Copy the Remote Script into your User Library"]))
        asked = setup.approval("ableton", data_dir=self.data)
        self.assertEqual(asked["effects"], ["Copy the Remote Script into your User Library",
                                            *(f"Writes: {path}" for path in paths)],
                         "every path the plan writes, whatever the runtime's words, each whole and as it is")

    def test_runtime_text_setup_couldnt_show_as_it_is_is_refused_not_cleaned(self) -> None:
        self.install_runtime()
        bridge = str(FAKE_LIBRARY / "Remote Scripts" / "AbletonMcpBridge")
        hidden = {"an escape": "\x1b[2J", "a newline": "\n", "a bidi override": "\u202e", "a zero-width space": "\u200b",
                  "a BOM": "\ufeff", "a tag character": "\U000e0041", "a line separator": "\u2028"}
        replies = {f"a path with {name}": {"paths": [bridge + char]} for name, char in hidden.items()}
        replies.update({f"an effect with {name}": {"effects": [f"Copy it{char} into your User Library"]}
                        for name, char in hidden.items()})
        replies.update({
            "an owner folder with a bidi override": {"state": "other_owner",
                                                     "owner": {"kumi_home": str(self.data.parent / "kumi\u202e")}},
            "a component name with an escape": {"component": "Remote Script\x1b[31m"},
            "a path longer than any path": {"paths": [str(FAKE_LIBRARY / ("x" * 1025))]},
            "more lines than an approval holds": {"effects": [f"Step {n}" for n in range(setup._MAX_LINES)]},
            "a digest that isn't one": {"digest": "not a digest"},
        })
        for name, reply in replies.items():
            with self.subTest(name):
                with self.assertRaises(setup.SetupError) as refused:
                    self.plan(setup_daw=Runtime(**reply))
                self.assertEqual(refused.exception.code, "bad_runtime_reply")
                self.assertFalse((self.data / "setup" / "plan.json").exists(), "nothing is planned to be approved")

    def test_one_setup_step_runs_at_a_time_and_a_dead_setups_lock_frees_itself(self) -> None:
        self.plan()
        holder = subprocess.Popen([sys.executable, "-c", HOLD_LOCK, SPEC.origin, str(self.data)],
                                  stdout=subprocess.PIPE, text=True)
        self.addCleanup(holder.wait)
        self.addCleanup(holder.kill)
        self.assertEqual(holder.stdout.readline().strip(), "locked")
        self.assertEqual(self.install(Served(BUNDLE))["error"]["code"], "busy")
        holder.kill()  # the setup holding the lock dies: the OS lets go of its lock
        holder.wait()
        holder.stdout.close()
        self.assertTrue(self.install(Served(BUNDLE))["ok"], "a busy refusal leaves the approved plan in place")

    def test_a_connection_that_breaks_mid_download_installs_nothing(self) -> None:
        self.plan()

        def breaks(url: str):
            raise http.client.IncompleteRead(b"partial")

        result = self.install(breaks)
        self.assertEqual(result["error"]["code"], "download_failed")
        self.assertFalse((self.data / "runtime" / "current.json").exists())

    def test_a_plan_that_changed_since_approval_installs_nothing(self) -> None:
        self.plan()
        other = tar_xz({"kumi": b"another build"})
        self.pin(other)
        served = Served(other)
        self.assertEqual(self.install(served)["error"]["code"], "plan_changed")
        self.assertEqual(served.asked, [])

    def test_a_download_that_differs_from_the_pin_installs_nothing(self) -> None:
        self.plan()
        result = self.install(Served(tar_xz({"kumi": b"tampered"})))
        self.assertEqual(result["error"]["code"], "checksum_mismatch")
        self.assertFalse((self.data / "runtime" / "current.json").exists())
        self.assertEqual([p.name for p in (self.data / "runtime").iterdir()], [])

    def test_a_data_folder_setup_cannot_write_in_is_reported_not_raised(self) -> None:
        self.data.mkdir()
        (self.data / "setup").write_text("a file where setup keeps its plan")
        with self.assertRaises(setup.SetupError) as raised:
            self.plan()
        self.assertEqual(raised.exception.code, "io_failed")
        (self.data / "setup").unlink()
        self.plan()
        (self.data / "runtime").write_text("a file where the runtime goes")
        self.assertEqual(self.install(Served(BUNDLE))["error"]["code"], "io_failed")

    def test_a_bundle_setup_would_not_unpack_installs_nothing(self) -> None:
        for problem, bundle in {"a symlink": tar_xz({"kumi": b"k"}, links={"escape": "/etc"}),
                                "a climbing path": tar_xz({"kumi": b"k", "../outside": b"x"}),
                                "no kumi": tar_xz({"README.md": b"r"}),
                                "only a lookalike of kumi": tar_xz({".kumi": b"k", "bin/kumi": b"k"})}.items():
            with self.subTest(problem):
                self.pin(bundle)
                self.plan()
                self.assertEqual(self.install(Served(bundle))["error"]["code"], "bad_bundle")
                self.assertFalse((self.data / "runtime" / "current.json").exists())
                self.assertFalse((self.data.parent / "outside").exists())

    def test_a_pin_pointing_anywhere_but_the_pinned_release_is_refused_before_downloading(self) -> None:
        for bad in ({"url": "https://github.com/user1303836/kumi/releases/latest/download/kumi-aarch64-apple-darwin.tar.xz"},
                    {"url": URL.replace("github.com", "example.com")},
                    {"sha256": "not-a-digest"}, {"size": 0}):
            with self.subTest(bad):
                self.pin(BUNDLE, **bad)
                with self.assertRaises(setup.SetupError) as raised:
                    self.plan()
                self.assertEqual(raised.exception.code, "bad_pin")

    def test_the_daw_step_is_planned_from_the_runtimes_dry_run_and_does_exactly_that(self) -> None:
        self.install_runtime()
        runtime = Runtime()
        plan = self.plan(setup_daw=runtime)
        self.assertEqual(plan["step"], "daw_component")
        self.assertEqual(runtime.calls, [{"daw": "ableton", "dry_run": True}])
        self.assertIn("Remote Scripts", setup.approval("ableton", data_dir=self.data)["message"])
        result = self.install(Served(b""), setup_daw=runtime)
        self.assertTrue(result["ok"], result)
        self.assertEqual(runtime.calls[-1], {"daw": "ableton", "dry_run": False})
        state = self.status()
        self.assertEqual((state["state"], state["next_step"]), ("ready", "choose_control_surface"))
        self.assertEqual(state["daw_component"]["version"], "1.0.105")
        self.assertEqual(self.plan(setup_daw=Runtime("same"))["step"], "none")

    def test_the_runtime_installs_only_the_daw_step_that_was_approved(self) -> None:
        self.install_runtime()
        approved = "a1" * 32
        runtime = Runtime(digest=approved)
        self.plan(setup_daw=runtime)
        self.assertTrue(self.install(Served(b""), setup_daw=runtime)["ok"])
        self.assertEqual(runtime.calls[-1], {"daw": "ableton", "dry_run": False, "expected": approved},
                         "the runtime plans again under the DAW's lease and refuses one that isn't this digest's")

    def test_a_component_another_kumi_owns_is_never_overwritten(self) -> None:
        self.install_runtime()
        runtime = Runtime("other_owner")
        self.assertEqual(self.plan(setup_daw=runtime)["step"], "owned_elsewhere")
        self.assertIsNone(setup.approval("ableton", data_dir=self.data))
        self.assertEqual(self.install(Served(b""), setup_daw=runtime)["error"]["code"], "owned_elsewhere")
        self.assertNotIn({"daw": "ableton", "dry_run": False}, runtime.calls)

    def test_a_runtime_that_refuses_or_fails_is_a_setup_error_with_its_code(self) -> None:
        self.install_runtime()

        class Refusal(Exception):  # what the runtime client raises for an isError answer
            code, reason = "other_daw", "Kumi's runtime in this profile works in REAPER"

        def refuses(args: dict) -> dict:
            raise Refusal()

        with self.assertRaises(setup.SetupError) as raised:
            self.plan(setup_daw=refuses)
        self.assertEqual(raised.exception.code, "other_daw")
        self.assertIn("works in REAPER", raised.exception.message)
        self.plan(setup_daw=Runtime())

        def dies_installing(args: dict) -> dict:
            if args["dry_run"]:
                return Runtime()(args)
            raise RuntimeError("Kumi's runtime stopped")

        result = self.install(Served(b""), setup_daw=dies_installing)
        self.assertEqual(result["error"]["code"], "daw_step_failed")
        self.assertIn("runtime stopped", result["error"]["message"])
        self.assertEqual(self.status()["daw_component"]["state"], "not_installed")

    def test_reaper_and_renoise_steps_say_what_their_component_writes_into_a_project(self) -> None:
        self.install_runtime()
        tagging = {"reaper": "keeps an id for each saved project in the project (ext state), saved with it",
                   "renoise": "keeps an id for each song in the song's tool data, saved with it"}
        for daw, words in tagging.items():
            with self.subTest(daw):
                setup.plan(daw, setup_daw=Runtime(), data_dir=self.data, plugin_dir=self.plugin, target=TARGET)
                self.assertIn(words, setup.approval(daw, data_dir=self.data)["message"], "it's in what's approved")
                owner = str(self.data.parent / f"standalone-{daw}")
                setup.plan(daw, setup_daw=Runtime("other_owner", owner=owner), data_dir=self.data,
                           plugin_dir=self.plugin, target=TARGET)
                self.assertIn(words, setup.approval(daw, action="adopt", data_dir=self.data)["message"])
                setup.plan(daw, setup_daw=Runtime(keep_identity=True), data_dir=self.data, plugin_dir=self.plugin,
                           target=TARGET)
                self.assertNotIn(words, setup.approval(daw, data_dir=self.data)["message"],
                                 "a component from the bridge bump reports keep_identity, and keeps no id (CC-8 8c)")
                setup.plan(daw, setup_daw=Runtime("other_owner", owner=owner, keep_identity=True), data_dir=self.data,
                           plugin_dir=self.plugin, target=TARGET)
                self.assertNotIn(words, setup.approval(daw, action="adopt", data_dir=self.data)["message"])
                setup.plan(daw, setup_daw=Runtime(keep_identity="true"), data_dir=self.data, plugin_dir=self.plugin,
                           target=TARGET)
                self.assertIn(words, setup.approval(daw, data_dir=self.data)["message"], "only a real true drops it")
        self.plan(setup_daw=Runtime())
        self.assertNotIn("keeps an id", setup.approval("ableton", data_dir=self.data)["message"], "Live's doesn't")

    def test_a_component_this_kumi_installed_before_is_recorded_when_planning(self) -> None:
        self.install_runtime()
        self.assertEqual(self.status()["state"], "needs_daw_component")
        self.assertEqual(self.plan(setup_daw=Runtime("same"))["step"], "none")
        state = self.status()
        self.assertEqual((state["state"], state["daw_component"]["state"]), ("ready", "installed"))

    def test_adopting_the_installation_that_owns_the_component_records_only_its_folder(self) -> None:
        self.install_runtime()
        standalone = self.data.parent / "standalone-kumi"
        standalone.mkdir()
        runtime = Runtime("other_owner", owner=str(standalone))
        self.assertEqual(self.adopt(runtime)["error"]["code"], "no_plan")
        self.plan(setup_daw=runtime)
        self.assertIsNone(setup.approval("ableton", data_dir=self.data), "install never approves an adoption")
        asked = setup.approval("ableton", action="adopt", data_dir=self.data)
        self.assertIn(str(standalone), asked["message"])
        self.assertIn("never its sign-in", asked["message"])
        self.assertIn("write its own connection state there", asked["message"])
        self.assertIn("reconnects at its next Kumi call", asked["message"], "it says a running runtime stops")
        result = self.adopt(runtime)
        self.assertTrue(result["ok"], result)
        current = json.loads((self.data / "runtime" / "current.json").read_text())
        self.assertEqual(current["kumi_home"], str(standalone))
        self.assertEqual(setup.runtime_state(self.data, VERSION, TARGET)[0], "ready")
        self.assertEqual(self.status()["daw_component"]["state"], "installed")
        self.assertNotIn({"daw": "ableton", "dry_run": False}, runtime.calls, "adoption installs nothing")

    def test_an_adoption_whose_owner_changed_or_vanished_is_refused(self) -> None:
        self.install_runtime()
        first, second = self.data.parent / "kumi-a", self.data.parent / "kumi-b"
        first.mkdir()
        second.mkdir()
        self.plan(setup_daw=Runtime("other_owner", owner=str(first)))
        self.assertEqual(self.adopt(Runtime("other_owner", owner=str(second)))["error"]["code"], "plan_changed")
        self.plan(setup_daw=Runtime("other_owner", owner=str(first)))
        first.rmdir()
        self.assertEqual(self.adopt(Runtime("other_owner", owner=str(first)))["error"]["code"], "not_found")
        self.assertNotIn("kumi_home", json.loads((self.data / "runtime" / "current.json").read_text()))

    def test_a_record_that_points_outside_the_data_folder_is_damaged(self) -> None:
        self.install_runtime()
        current = self.data / "runtime" / "current.json"
        record = json.loads(current.read_text())
        current.write_text(json.dumps({**record, "kumi": "../../bin/sh"}))
        self.assertEqual(setup.runtime_state(self.data, VERSION, TARGET)[0], "damaged")
        self.assertEqual(self.status()["next_step"], "install_runtime")
        current.write_text(json.dumps(record))
        binary = self.data / record["kumi"]
        binary.unlink()
        binary.symlink_to(sys.executable)  # a real program outside the data folder
        self.assertEqual(setup.runtime_state(self.data, VERSION, TARGET)[0], "damaged",
                         "a binary that is a link out of the data folder isn't trusted")

    def test_setup_looks_for_ffmpeg_only_in_local_absolute_folders(self) -> None:
        tools = "/opt/tools/bin"  # the search is text only, so a POSIX path, whatever system runs the test
        self.assertEqual(SEARCH({"PATH": ":".join([".", "", "relative/bin", tools, "//server/share/bin"])}, "darwin"),
                         [tools, "/opt/homebrew/bin", "/usr/local/bin", "/opt/local/bin"],
                         "never the working folder, a relative or empty entry, or a network path")
        self.assertEqual(SEARCH({"PATH": ";".join([".", "bin", r"\\server\share\bin", r"\\?\C:\ffmpeg\bin",
                                                    r"\\.\C:\ffmpeg\bin", r"\ffmpeg\bin", "C:ffmpeg",
                                                    r'"C:\Program Files\ffmpeg\bin"'])}, "win32"),
                         [r"C:\Program Files\ffmpeg\bin"],
                         "Windows: a drive and a root only, never UNC or a device namespace, which could reach the "
                         "network before anything is approved")
        project = self.data.parent / "downloaded project"
        (project / "bin").mkdir(parents=True)
        for where in (project, project / "bin"):
            (where / FFMPEG).write_bytes(b"#!/bin/sh\n")
            (where / FFMPEG).chmod(0o755)
        here = os.getcwd()
        os.chdir(project)
        try:
            with mock.patch.object(setup, "_ffmpeg_search", SEARCH), mock.patch.dict(setup._FFMPEG_FOLDERS, clear=True):
                self.assertIsNone(setup.find_ffmpeg({"PATH": os.pathsep.join(["", ".", "bin"])}),
                                  "an ffmpeg in the working folder is never found")
        finally:
            os.chdir(here)

    def test_a_mapped_network_drive_or_an_automount_folder_is_never_touched(self) -> None:
        kinds = {"C": setup._DRIVE_FIXED, "Z": 4}  # 4: DRIVE_REMOTE, a mapped network drive
        with mock.patch.object(setup, "_drive_type", side_effect=lambda root: kinds[root[0]]):
            self.assertTrue(setup._on_this_computer(r"C:\ffmpeg\bin\ffmpeg.exe", windows=True))
            self.assertFalse(setup._on_this_computer(r"Z:\ffmpeg\bin\ffmpeg.exe", windows=True),
                             "a mapped drive can make Windows authenticate to its server")
            self.assertFalse(setup._on_this_computer(r"\\server\share\ffmpeg.exe", windows=True))
        for path in ("/net/server/bin/ffmpeg", "/Network/Servers/server/ffmpeg", "/NET/server/ffmpeg"):
            self.assertFalse(setup._on_this_computer(path, windows=False), f"{path} mounts a server's share")
        self.assertTrue(setup._on_this_computer("/opt/homebrew/bin/ffmpeg", windows=False))

    @unittest.skipIf(os.name == "nt", "links need privileges on Windows")
    def test_links_are_followed_only_to_this_computer_from_the_root_down(self) -> None:
        cellar = self.data.parent / "Cellar" / "ffmpeg" / "8.1" / "bin"
        cellar.mkdir(parents=True)
        (cellar / FFMPEG).write_bytes(b"#!/bin/sh\n")
        (cellar / FFMPEG).chmod(0o755)
        brew = self.data.parent / "brew-bin"
        brew.mkdir()
        (brew / FFMPEG).symlink_to(Path("..") / "Cellar" / "ffmpeg" / "8.1" / "bin" / FFMPEG)
        self.search.return_value = [str(brew)]
        self.assertEqual(setup.find_ffmpeg()["path"], os.path.realpath(cellar / FFMPEG), "Homebrew's link is resolved")
        (brew / FFMPEG).unlink()
        (brew / FFMPEG).symlink_to("//server/share/ffmpeg")
        self.assertIsNone(setup.find_ffmpeg(), "a link to a network path is never followed")
        tools = self.data.parent / "tools"
        tools.symlink_to("//server/share")  # a folder above the candidate, linked to a share
        self.search.return_value = [str(tools / "bin")]
        touched = []
        islink = os.path.islink
        with mock.patch.object(setup.os.path, "islink", side_effect=lambda path: touched.append(path) or islink(path)):
            self.assertIsNone(setup.find_ffmpeg())
        self.assertTrue(touched, "the candidate's folders were looked at")
        under = tuple(os.path.join(parent, "tools") + os.sep  # as given, and with the temp folder's own links resolved
                      for parent in (str(self.data.parent), os.path.realpath(self.data.parent)))
        self.assertFalse([path for path in touched if path.startswith(under)], "nothing under the linked folder")

    def test_allowing_ffmpeg_is_an_optional_last_step_that_binds_the_program(self) -> None:
        self.install_runtime()
        self.assertEqual(self.plan(setup_daw=Runtime("same"))["step"], "none", "no ffmpeg here: setup is done")
        audio = self.status()["compressed_audio"]
        self.assertEqual((audio["state"], audio["found"]), ("not_allowed", None))
        self.assertIn("WAV and AIFF only: there's no ffmpeg on this computer", audio["message"])
        program = self.ffmpeg()
        planned = self.plan(setup_daw=Runtime("same"))
        self.assertEqual((planned["step"], planned["optional"]), ("ffmpeg", True))
        self.assertEqual(self.status()["state"], "ready", "Kumi is set up without it")
        self.assertEqual(self.status()["compressed_audio"]["found"], str(program))
        asked = setup.approval("ableton", data_dir=self.data)
        self.assertEqual(asked["title"], "Kumi: let Kumi's runtime run the ffmpeg already on this computer")
        self.assertIn(f"Kumi's runtime will run {program} to decode MP3, FLAC", asked["message"])
        self.assertIn("doesn't download or install ffmpeg", asked["message"])
        done = self.install(None, setup_daw=Runtime("same"))
        self.assertTrue(done["ok"], done)
        info = os.stat(program)
        self.assertEqual(self.current()["tools"]["ffmpeg"], {"path": str(program), "size": info.st_size,
                                                             "mtime_ns": info.st_mtime_ns, "ino": info.st_ino})
        self.assertEqual(self.status()["compressed_audio"]["state"], "available")
        self.assertEqual(self.plan(setup_daw=Runtime("same"))["step"], "none", "it isn't offered again")
        (self.data / "runtime" / VERSION / "kumi").unlink()  # the runtime is set up again
        self.assertEqual(self.plan()["step"], "runtime")
        self.assertTrue(self.install(Served(BUNDLE))["ok"])
        self.assertEqual(self.current()["tools"]["ffmpeg"]["path"], str(program), "a new runtime keeps it")

    def test_an_ffmpeg_that_changed_after_planning_or_allowing_isnt_used(self) -> None:
        self.install_runtime()
        program = self.ffmpeg()
        self.plan(setup_daw=Runtime("same"))
        program.write_bytes(b"#!/bin/sh\necho another program at the same path\n")
        self.assertEqual(self.install(None, setup_daw=Runtime("same"))["error"]["code"], "plan_changed")
        self.assertNotIn("tools", self.current(), "the program that was approved isn't the one there now")
        self.plan(setup_daw=Runtime("same"))
        self.assertTrue(self.install(None, setup_daw=Runtime("same"))["ok"])
        program.write_bytes(b"#!/bin/sh\necho swapped after it was allowed\n")
        audio = self.status()["compressed_audio"]
        self.assertEqual(audio["state"], "changed")
        self.assertIn("changed since you allowed it; to allow it again, run hermes kumi setup", audio["message"])
        self.assertEqual(self.plan(setup_daw=Runtime("same"))["step"], "ffmpeg", "setup offers it again")
        program.unlink()
        self.assertEqual(self.status()["compressed_audio"]["state"], "gone")

    @unittest.skipIf(os.name == "nt", "links need privileges on Windows")
    def test_an_allowed_ffmpeg_whose_folder_became_a_link_isnt_used(self) -> None:
        self.install_runtime()
        program = self.ffmpeg()
        self.plan(setup_daw=Runtime("same"))
        self.assertTrue(self.install(None, setup_daw=Runtime("same"))["ok"])
        folder = program.parent
        folder.rename(folder.with_name("bin-elsewhere"))
        folder.symlink_to(folder.with_name("bin-elsewhere"))  # the same file, now reached through a link
        self.assertEqual(self.status()["compressed_audio"]["state"], "changed", "its path now leads through a link")

    def test_in_the_terminal_ffmpeg_is_asked_last_and_no_keeps_wav_and_aiff(self) -> None:
        self.install_runtime()
        program = self.ffmpeg()

        def cli(answers: str) -> tuple[int, str]:
            out = Tty()
            code = setup.cli_main("ableton", setup_daw=Runtime("same"), data_dir=self.data, plugin_dir=self.plugin,
                                  target=TARGET, stdin=Tty(answers), stdout=out)
            return code, out.getvalue()

        code, said = cli("n\n")
        self.assertEqual(code, 0, "setup is done without it")
        self.assertIn(f"Kumi's runtime will run {program}", said)
        self.assertIn("Kumi reads WAV and AIFF only", said)
        self.assertNotIn("tools", self.current())
        code, said = cli("y\n")
        self.assertEqual(code, 0, said)
        self.assertIn(f"Compressed audio (MP3, FLAC…): yes, with the ffmpeg at {program}.", said)

    def test_the_cli_asks_before_each_step_and_refuses_without_a_terminal(self) -> None:
        def cli(answers: str, stdin_is_tty: bool = True, setup_daw=None) -> tuple[int, str]:
            out = Tty()
            stdin = Tty(answers) if stdin_is_tty else io.StringIO(answers)
            code = setup.cli_main("ableton", setup_daw=setup_daw, data_dir=self.data, plugin_dir=self.plugin,
                                  target=TARGET, opener=Served(BUNDLE), stdin=stdin, stdout=out)
            return code, out.getvalue()

        code, said = cli("y\n", stdin_is_tty=False)
        self.assertEqual(code, 1)
        self.assertIn("needs a terminal", said)
        self.assertFalse(self.data.exists(), "without a terminal nothing is planned or installed")
        code, said = cli("n\n")
        self.assertEqual(code, 1)
        self.assertIn(URL, said)
        self.assertIn(f"Writes: {self.data / 'runtime' / VERSION}", said, "the terminal asks with the approval's lines")
        self.assertFalse((self.data / "runtime" / "current.json").exists())
        code, said = cli("y\n")
        self.assertEqual(code, 0, said)
        self.assertEqual(setup.runtime_state(self.data, VERSION, TARGET)[0], "ready")
        standalone = self.data.parent / "standalone-kumi"
        standalone.mkdir()
        code, said = cli("y\n", setup_daw=Runtime("other_owner", owner=str(standalone)))
        self.assertEqual(code, 0, said)
        self.assertIn("write its own connection state there", said, "the CLI asks with the adoption's own words")
        current = json.loads((self.data / "runtime" / "current.json").read_text())
        self.assertEqual(current["kumi_home"], str(standalone))


if __name__ == "__main__":
    unittest.main()
