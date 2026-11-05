"""doctor.py's `hermes kumi doctor` and `/kumi doctor`: each item read from Hermes's settings and Kumi's own state, with
what to do about it; the terminal's exit status; and that it changes nothing and never opens Hermes's secrets."""
from __future__ import annotations

import argparse
import contextlib
import io
import json
import os
from pathlib import Path
import tempfile
import time
from types import SimpleNamespace
import unittest
from unittest import mock

import support

commands = support.module("commands")
doctor = support.module("doctor")
setup = support.module("setup")
tools = support.module("tools")
SEARCHING = mock.patch.object(setup, "_ffmpeg_search", return_value=[])  # never this computer's ffmpeg


def setUpModule():
    SEARCHING.start()


def tearDownModule():
    SEARCHING.stop()


ASKS = {"approvals": {"mode": "manual"}}


def publish(data: Path, *, connected: bool = True, key: str | None = "p-1", scan: str | None = "done",
            sounds: int = 3, daw: str = "ableton", component: dict | None = None, stopped: bool = False) -> None:
    """What a runtime leaves in its plugin data: its lease (heartbeat now; `stopped`: its last word was a stop), status
    (`component`: its own reading of Kumi's part in the DAW), a scan job and a map."""
    state = data / "state"
    (state / "jobs").mkdir(parents=True, exist_ok=True)
    (state / "map").mkdir(parents=True, exist_ok=True)
    now = int(time.time() * 1000)
    write = lambda name, body: (state / f"{name}.json").write_text(json.dumps({"schema": 1, "seq": 1, **body}),
                                                                   encoding="utf-8")
    write("owner", {"heartbeat": now, "running": not stopped, "owner": {"pid": 1, "session_id": "s1"}})
    project = {"name": "Night Drive", "epoch": "1", **({"key": key} if key else {})}
    write("status", {"connected": connected, "daw": daw, "project": project,
                     **({"setup": {"component": component, "daw_open": True}} if component else {})})
    if scan:
        write("jobs/j1", {"job_id": "j1", "kind": "scan", "state": scan, "at": now,
                          "progress": {"done": 1, "total": 2}})
    if sounds:
        write("map/r1", {"sounds": {f"s-{n}": {"name": f"Snare {n}.wav"} for n in range(sounds)}})
        (state / "map" / "current").write_text("r1", encoding="utf-8")


def tree(folder: Path) -> dict:
    return {path: path.read_bytes() for path in sorted(folder.rglob("*")) if path.is_file()}


class Doctor(unittest.TestCase):
    def setUp(self):
        # A home of the test's own: setup's look for another Kumi on this computer reads HOME and KUMI_HOME.
        home = tempfile.TemporaryDirectory()
        self.addCleanup(home.cleanup)
        kept = {key: value for key, value in os.environ.items()
                if key not in ("HERMES_YOLO_MODE", "KUMI_HOME", tools.PLUGIN_HOST_ENV)}
        environ = mock.patch.dict("os.environ", {**kept, "HOME": home.name, "USERPROFILE": home.name}, clear=True)
        environ.start()
        self.addCleanup(environ.stop)
        self.home = Path(home.name)

    def ready(self, hermes: dict | None = None, settings: dict | None = None, **published):
        host, ctx, data = support.host(self, {"daw": "ableton", "library_folders": [str(support.DRUMS)],
                                              **(settings or {})}, hermes_config=ASKS if hermes is None else hermes)
        publish(data, **published)
        return host, ctx, data

    def item(self, items: list[dict], name: str) -> dict:
        found = [item for item in items if item["name"] == name]
        self.assertEqual(len(found), 1, f"{name} in {[item['name'] for item in items]}")
        return found[0]

    def test_a_profile_ready_for_the_demo_has_nothing_to_fix(self):
        host, _, _ = self.ready()
        items = doctor.checks(host)
        self.assertFalse(doctor.needs_fixing(items), doctor.report(items))
        for name in ("Approvals", "Plugin isolation", "Hook time", "Hermes", "Kumi's runtime", "Kumi's part in Live",
                     "Connection", "Project", "Sound map"):
            self.assertEqual(self.item(items, name)["state"], doctor.OK, name)
        self.assertIn("3 sounds on the map", self.item(items, "Sound map")["detail"])
        self.assertEqual(self.item(items, "Hermes Desktop")["state"], doctor.CHECK, "Desktop keeps its own switches")
        self.assertEqual(doctor.report(items).splitlines()[-1], "0 to fix, 1 to check.")

    def test_each_hermes_setting_that_hides_an_approval_is_named_with_its_fix(self):
        cases = (("approvals.mode: off", {"approvals": {"mode": "off"}}, doctor.FIX, "approvals.mode: manual"),
                 ("one-shot runs approved", {"approvals": {"mode": "manual", "single_query_mode": "approve"}},
                  doctor.FIX, "single_query_mode: deny"),
                 ("smart (Hermes's default)", {"approvals": {"mode": "smart"}}, doctor.CHECK, "approvals.mode: manual"),
                 ("plugin host", {**ASKS, "plugins": {"isolation": "host"}}, doctor.FIX, "plugins.isolation: in_process"))
        for name, hermes, state, fix in cases:
            with self.subTest(name):
                host, _, _ = self.ready(hermes)
                items = doctor.checks(host)
                flagged = [item for item in items if item["state"] == state and fix in item["fix"]]
                self.assertEqual(len(flagged), 1, doctor.report(items))
        with self.subTest("--yolo"), mock.patch.dict("os.environ", {"HERMES_YOLO_MODE": "1"}):
            host, _, _ = self.ready()
            self.assertEqual(self.item(doctor.checks(host), "Approvals")["state"], doctor.FIX)

    def test_a_chats_yolo_counts_in_the_chat_and_a_terminal_says_it_cant_see_it(self):
        approval = SimpleNamespace(is_current_session_yolo_enabled=lambda: True)
        with mock.patch.dict("sys.modules", {"tools": SimpleNamespace(approval=approval), "tools.approval": approval}):
            host, _, _ = self.ready()
            self.assertEqual(self.item(doctor.checks(host, session_id="s1"), "Approvals")["state"], doctor.FIX)
            terminal = self.item(doctor.checks(host), "Approvals")
            self.assertEqual(terminal["state"], doctor.OK)
            self.assertIn("can't see a chat's /yolo", terminal["detail"])

    def test_a_hook_timeout_that_cuts_the_approval_preview_short_is_a_fix(self):
        for timeout, state in ((10.0, doctor.FIX), (19.9, doctor.FIX), (20.0, doctor.OK), (30.0, doctor.OK),
                               (0.0, doctor.OK)):
            with self.subTest(timeout=timeout):
                host, _, _ = self.ready()
                host._hook_timeout = lambda: timeout
                self.assertEqual(self.item(doctor.checks(host), "Hook time")["state"], state)

    def test_the_listening_model_is_named_and_left_to_check_only_when_interpretation_is_on(self):
        host, _, _ = self.ready()
        self.assertEqual(self.item(doctor.checks(host), "Listening model")["state"], doctor.OFF)
        host, _, _ = self.ready({**ASKS, "auxiliary": {"kumi_listen": {"model": "vision-pro-1"}}},
                                {"listen_interpretation": True})
        listening = self.item(doctor.checks(host), "Listening model")
        self.assertEqual(listening["state"], doctor.CHECK)
        self.assertIn("vision-pro-1", listening["detail"])
        self.assertIn("image-capable", listening["fix"])

    def test_what_setup_still_needs_is_a_fix_with_the_setup_step(self):
        host, _, _ = support.host(self, hermes_config=ASKS, installed=False)
        items = doctor.checks(host)
        self.assertEqual(self.item(items, "Kumi's runtime")["state"], doctor.FIX)
        self.assertIn("hermes kumi setup", self.item(items, "Kumi's runtime")["fix"])
        self.assertEqual(self.item(items, "Kumi's part in Live")["state"], doctor.OFF, "the runtime comes first")
        host, _, _ = support.host(self, hermes_config=ASKS, daw_component=False)
        part = self.item(doctor.checks(host), "Kumi's part in Live")
        self.assertEqual((part["state"], "hermes kumi setup" in part["fix"]), (doctor.FIX, True))

    def test_the_project_needs_a_key_and_the_map_a_finished_scan(self):
        host, _, _ = self.ready(key=None)
        project = self.item(doctor.checks(host), "Project")
        self.assertEqual(project["state"], doctor.FIX)
        self.assertIn("Save the project", project["fix"])
        for scan, sounds, stopped, state, said in (
                ("running", 0, False, doctor.CHECK, "Wait until it shows done"),
                ("running", 0, True, doctor.FIX, "Connect again"),
                ("failed", 3, False, doctor.FIX, "scan the folders again"),
                (None, 0, False, doctor.FIX, "scan them"),
                ("done", 0, False, doctor.FIX, "WAV or AIFF"),
                ("done", 3, False, doctor.OK, "")):
            with self.subTest(scan=scan, sounds=sounds, stopped=stopped):
                host, _, _ = self.ready(scan=scan, sounds=sounds, stopped=stopped)
                found = self.item(doctor.checks(host), "Sound map")
                self.assertEqual(found["state"], state, found)
                self.assertIn(said, found["fix"])
        host, _, _ = self.ready(settings={"library_folders": []})
        self.assertEqual(self.item(doctor.checks(host), "Sound map")["state"], doctor.CHECK, "a map is optional")

    def recorded(self, data: Path, *, parts: list | None = None, adopted: bool = False) -> None:
        """Setup's record of Kumi's part in Live: its sub-parts' states, or adopted from a standalone Kumi here."""
        path = data / "setup" / "receipt.json"
        receipt = json.loads(path.read_text(encoding="utf-8"))
        component = receipt["daw_components"]["ableton"]
        component["parts"] = parts or []
        if adopted:
            (self.home / ".kumi").mkdir(exist_ok=True)
            receipt["adopted"] = {"kumi_home": str(self.home / ".kumi"), "daw": "ableton", "at": 0}
            component["adopted"] = True
        path.write_text(json.dumps(receipt), encoding="utf-8")

    def test_the_runtimes_own_reading_of_the_daw_part_comes_before_setups_record(self):
        def part(parts: list | None = None, adopted: bool = False, **published) -> dict:
            host, _, data = self.ready(**published)
            self.recorded(data, parts=parts, adopted=adopted)
            return self.item(doctor.checks(host), "Kumi's part in Live")

        reading = lambda version: {"installed": True, "version": version, "bundled": "1.0.105"}
        older = part(component=reading("1.0.99"))
        self.assertEqual(older["state"], doctor.FIX, "setup's record says the same; the runtime knows better")
        self.assertIn("older than this Kumi's 1.0.105", older["detail"])
        self.assertIn("hermes kumi setup", older["fix"])
        self.assertEqual(part(component=reading("1.0.106"))["state"], doctor.CHECK)
        same = part(component=reading("1.0.105"))
        self.assertEqual(same["state"], doctor.OK)
        self.assertIn("the same as this Kumi's, as its runtime read it just now", same["detail"])
        for version in (None, "dev", "1.0.105-rc1"):
            with self.subTest(version=version):
                unsure = part(component=reading(version))
                self.assertEqual(unsure["state"], doctor.CHECK, "only two dotted versions that compare equal match")
                self.assertIn("can't compare", unsure["detail"])
        ears = [{"name": "Remote Script", "state": "same"}, {"name": "Kumi Ears", "state": "other_owner"}]
        incomplete = part(ears, component=reading("1.0.105"))
        self.assertEqual(incomplete["state"], doctor.FIX, "the runtime reads the Remote Script's version, not Ears'")
        self.assertIn("isn't complete: Kumi Ears", incomplete["detail"])
        adopted = part(adopted=True, component=reading("1.0.99"))
        self.assertEqual(adopted["state"], doctor.FIX)
        self.assertIn("another Kumi installation put in Live", adopted["detail"])
        self.assertIn("kumi --daw ableton bridge", adopted["fix"], "this profile's setup refuses an adopted part")
        self.assertNotIn("hermes kumi setup", adopted["fix"])
        for published in ({"component": reading("1.0.99"), "stopped": True},
                          {"component": reading("1.0.99"), "daw": "reaper"}, {}):
            with self.subTest(published=published):
                last = part(**published)
                self.assertEqual(last["state"], doctor.OK, "only setup's record to go on")
                self.assertIn("as setup last recorded it", last["detail"])

    def test_the_connection_names_the_daw_the_runtime_runs_for(self):
        host, _, _ = self.ready(daw="reaper")
        connection = self.item(doctor.checks(host), "Connection")
        self.assertEqual(connection["state"], doctor.CHECK)
        self.assertEqual(connection["detail"], "Connected to REAPER: Night Drive. Kumi's DAW setting is Live.")
        self.assertIn("hermes kumi setup --daw reaper --use", connection["fix"])
        host, _, _ = self.ready()
        self.assertEqual(self.item(doctor.checks(host), "Connection")["detail"], "Connected to Live: Night Drive.")

    def test_not_connected_is_left_to_check_and_the_project_waits_for_it(self):
        host, _, _ = self.ready(connected=False)
        items = doctor.checks(host)
        self.assertEqual(self.item(items, "Connection")["state"], doctor.CHECK)
        self.assertEqual(self.item(items, "Project")["state"], doctor.OFF)

    def test_mirelo_is_checked_only_where_the_host_reports_it_and_its_key_is_never_shown(self):
        host, _, _ = self.ready()
        self.assertNotIn("Mirelo", [item["name"] for item in doctor.checks(host)])
        for ready, state, said in (({"key": False}, doctor.OFF, "never contacts Mirelo"),
                                   ({"key": True, "acknowledged": True, "task": True, "cap": False}, doctor.FIX,
                                    "mirelo_credit_cap above 0"),
                                   ({"key": True, "acknowledged": True, "task": True, "cap": True}, doctor.OK, "ready")):
            with self.subTest(ready=ready):
                host.mirelo_status = lambda ready=ready: {**ready, "secret": "sk-not-for-the-report"}
                mirelo = self.item(doctor.checks(host), "Mirelo")
                self.assertEqual(mirelo["state"], state)
                self.assertIn(said, mirelo["detail"] + " " + mirelo["fix"])
                self.assertNotIn("sk-not-for-the-report", doctor.report(doctor.checks(host)))

    def test_the_terminal_command_exits_1_only_when_something_needs_fixing_and_the_chat_command_answers(self):
        parser = argparse.ArgumentParser()
        commands.cli_parser(parser)
        for hermes, code in ((ASKS, 0), ({"approvals": {"mode": "off"}}, 1)):
            with self.subTest(hermes=hermes):
                host, _, _ = self.ready(hermes)
                out = io.StringIO()
                with contextlib.redirect_stdout(out):
                    self.assertEqual(commands.cli(host, parser.parse_args(["doctor"])), code, out.getvalue())
                self.assertTrue(out.getvalue().startswith("Kumi doctor:"))
        host, _, _ = self.ready()
        said = commands.kumi(host, " Doctor ", session_id="s1")
        self.assertTrue(said.startswith("Kumi doctor:"), said)

    def test_it_changes_nothing_names_no_folder_and_never_opens_hermes_secrets(self):
        host, _, data = self.ready(settings={"listen_interpretation": True})
        before = tree(data)
        opened: list[str] = []
        real = io.open

        def watched(file, *args, **kwargs):
            opened.append(os.fspath(file) if isinstance(file, (str, os.PathLike)) else str(file))
            return real(file, *args, **kwargs)

        with mock.patch("io.open", watched), mock.patch("builtins.open", watched):
            said = doctor.report(doctor.checks(host, session_id="s1"))
        self.assertEqual(tree(data), before, "nothing written in plugin data")
        self.assertFalse([path for path in opened if Path(path).name in (".env", "auth.json")], opened)
        self.assertNotIn(str(data), said, "no folder on this computer")
        self.assertNotIn(str(support.DRUMS), said)

    def test_a_profile_kumi_never_ran_in_keeps_no_plugin_data_folder(self):
        hermes_home = self.home / "hermes"
        constants = SimpleNamespace(get_hermes_home=lambda: hermes_home)
        host, _, _ = support.host(self, hermes_config=ASKS)
        host._data_dir = None  # Hermes's own folder, as outside the tests
        with mock.patch.dict("sys.modules", {"hermes_constants": constants}):
            items = doctor.checks(host)
            self.assertEqual(self.item(items, "Kumi's runtime")["state"], doctor.FIX)
            self.assertFalse((hermes_home / "plugin-data").exists(), "Hermes's plugin_data_dir would have made it")
            self.assertEqual(host.data_dir(), host.data_path(), "what writes uses the same folder")
            self.assertTrue((hermes_home / "plugin-data" / "kumi").is_dir(), "made on first use")


if __name__ == "__main__":
    unittest.main()
