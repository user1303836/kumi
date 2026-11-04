"""snapshots.py: what the UI and the FOCUS hook make of Kumi's state files (contract C3)."""

from __future__ import annotations

import importlib.util
import json
import os
import sys
import tempfile
import unittest
from pathlib import Path

HERE = Path(__file__).resolve().parent


def _load(name: str, path: Path):
    spec = importlib.util.spec_from_file_location(name, path)
    module = importlib.util.module_from_spec(spec)
    sys.modules[name] = module
    spec.loader.exec_module(module)
    return module


snapshots = _load("kumi_test_snapshots", HERE.parent / "snapshots.py")
fixtures = _load("kumi_test_ui_fixtures", HERE / "ui_fixtures.py")

NOW = 1_762_300_000_000


class StateTest(unittest.TestCase):
    def setUp(self) -> None:
        self.tmp = tempfile.TemporaryDirectory()
        root = Path(self.tmp.name)
        self.data = root / "plugin-data" / "kumi"
        self.library = root / "samples"
        self.state = fixtures.write_state(self.data, self.library, audio="some", now_ms=NOW)

    def tearDown(self) -> None:
        self.tmp.cleanup()


class Liveness(StateTest):
    def test_a_fresh_heartbeat_is_connected_and_lists_each_snapshots_revision(self) -> None:
        status = snapshots.status(self.state, now_ms=NOW + 2_000)
        self.assertTrue(status["connected"])
        self.assertFalse(status["stale"])
        self.assertEqual(status["revisions"], {"status": 12, "focus": 88, "history": 9, "map": "r7"})
        self.assertEqual(status["project"]["name"], "Night Drive")
        self.assertEqual(status["owner"]["session_id"], fixtures.OWNER["session_id"])
        self.assertNotIn("schema", status)

    def test_a_heartbeat_older_than_ten_seconds_is_stale_and_nothing_can_be_undone(self) -> None:
        later = NOW + (snapshots.STALE_AFTER_S + 1) * 1000
        status = snapshots.status(self.state, now_ms=later)
        self.assertEqual((status["connected"], status["stale"]), (False, True))
        focus = snapshots.focus(self.state, now_ms=later)
        # What was last in focus stays readable, marked stale.
        self.assertEqual((focus["connected"], focus["stale"]), (False, True))
        self.assertEqual(focus["focus"]["track"]["name"], "Drums")
        history = snapshots.history(self.state, now_ms=later)
        self.assertEqual(len(history["items"]), 4)
        self.assertTrue(all(item["undo_available"] is False for item in history["items"]))
        # While fresh, the runtime's own word stands.
        fresh = snapshots.history(self.state, now_ms=NOW)
        self.assertEqual([item["undo_available"] for item in fresh["items"]], [True, True, False, False])

    def test_a_heartbeat_from_the_future_is_stale_too(self) -> None:
        self.assertTrue(snapshots.owner(self.state, now_ms=NOW - 60_000)["stale"])

    def test_no_state_folder_reads_as_disconnected_never_as_an_error(self) -> None:
        empty = Path(self.tmp.name) / "nothing" / "state"
        status = snapshots.status(empty, now_ms=NOW)
        self.assertEqual((status["connected"], status["stale"]), (False, True))
        self.assertEqual(status["revisions"], {"status": None, "focus": None, "history": None, "map": None})
        self.assertEqual(snapshots.history(empty, now_ms=NOW)["items"], [])
        self.assertFalse(snapshots.focus(empty, now_ms=NOW)["connected"])
        self.assertIsNone(snapshots.map_revision(empty))


class Files(StateTest):
    def test_a_file_of_another_schema_or_not_json_reads_as_missing(self) -> None:
        (self.state / "focus.json").write_text(json.dumps({"schema": 2, "seq": 1}), encoding="utf-8")
        self.assertIsNone(snapshots.read(self.state, "focus"))
        (self.state / "history.json").write_text("{not json", encoding="utf-8")
        self.assertIsNone(snapshots.read(self.state, "history"))
        self.assertEqual(snapshots.history(self.state, now_ms=NOW)["items"], [])

    def test_history_is_cut_to_the_limit_asked_within_two_hundred(self) -> None:
        self.assertEqual(len(snapshots.history(self.state, limit=2, now_ms=NOW)["items"]), 2)
        self.assertEqual(snapshots.history(self.state, limit=2, now_ms=NOW)["items"][0]["label"], "c14")
        self.assertEqual(len(snapshots.history(self.state, limit=0, now_ms=NOW)["items"]), 1)
        self.assertEqual(len(snapshots.history(self.state, limit="many", now_ms=NOW)["items"]), 4)

    def test_map_revisions_and_job_ids_never_name_a_file_outside_their_folder(self) -> None:
        (self.state.parent / "secret.json").write_text(json.dumps({"schema": 1, "seq": 1}), encoding="utf-8")
        self.assertIsNone(snapshots.map_revision(self.state, "../../secret"))
        self.assertIsNone(snapshots.map_revision(self.state, ".."))
        self.assertIsNone(snapshots.job(self.state, "../owner"))
        self.assertEqual(snapshots.job(self.state, "j-scan-1")["state"], "running")
        (self.state / "map" / "current").write_text("../owner\n", encoding="utf-8")
        self.assertIsNone(snapshots.map_current(self.state))

    def test_a_sound_is_found_only_on_the_current_map_revision(self) -> None:
        current = snapshots.map_revision(self.state)
        self.assertEqual(current["revision"], "r7")
        self.assertEqual(len(current["points"]), sum(group[4] for group in fixtures.GROUPS))
        sound = snapshots.sound(self.state, "s-r7-0091")
        self.assertEqual((sound["class"], sound["revision"], sound["sound_id"]), ("snare", "r7", "s-r7-0091"))
        self.assertIsNone(snapshots.sound(self.state, "s-r6-0091"))


class Media(StateTest):
    def test_an_artifact_in_the_plugin_data_is_served_with_its_size_and_type(self) -> None:
        found = snapshots.media(self.state, "a-7f32")
        self.assertEqual((found["kind"], found["mime"]), ("audio", "audio/wav"))
        self.assertEqual(found["bytes"], os.path.getsize(found["path"]))

    def test_a_map_sound_is_served_only_from_an_approved_sample_folder(self) -> None:
        self.assertIsNone(snapshots.media(self.state, "s-r7-0091"))
        found = snapshots.media(self.state, "s-r7-0091", [str(self.library)])
        self.assertTrue(found["path"].endswith(".wav"))
        # A sound with no file on disk isn't served.
        self.assertIsNone(snapshots.media(self.state, "s-r7-0001", [str(self.library)]))

    def test_a_path_outside_every_approved_root_is_refused_even_through_a_symlink(self) -> None:
        outside = Path(self.tmp.name) / "elsewhere"
        outside.mkdir()
        (outside / "private.wav").write_bytes(b"RIFF")
        artifacts = json.loads((self.state / "artifacts.json").read_text(encoding="utf-8"))
        artifacts["artifacts"]["a-out"] = {"path": str(outside / "private.wav"), "kind": "audio"}
        link = self.data / "artifacts" / "link.wav"
        link.symlink_to(outside / "private.wav")
        artifacts["artifacts"]["a-link"] = {"path": str(link), "kind": "audio"}
        (self.state / "artifacts.json").write_text(json.dumps(artifacts), encoding="utf-8")
        self.assertIsNone(snapshots.media(self.state, "a-out", [str(self.library)]))
        self.assertIsNone(snapshots.media(self.state, "a-link", [str(self.library)]))
        self.assertIsNone(snapshots.media(self.state, "nothing-like-it"))

    def test_a_file_over_fifty_mib_is_not_served(self) -> None:
        big = self.data / "artifacts" / "big.wav"
        with big.open("wb") as file:
            file.truncate(snapshots.MEDIA_MAX_BYTES + 1)
        artifacts = json.loads((self.state / "artifacts.json").read_text(encoding="utf-8"))
        artifacts["artifacts"]["a-big"] = {"path": str(big), "kind": "audio"}
        (self.state / "artifacts.json").write_text(json.dumps(artifacts), encoding="utf-8")
        self.assertIsNone(snapshots.media(self.state, "a-big"))


class Imports(unittest.TestCase):
    def test_the_module_imports_with_the_standard_library_alone(self) -> None:
        source = (HERE.parent / "snapshots.py").read_text(encoding="utf-8")
        top = [line for line in source.splitlines() if line.startswith(("import ", "from "))]
        allowed = {"__future__", "json", "os", "re", "time", "pathlib", "typing"}
        self.assertEqual({line.split()[1].split(".")[0] for line in top} - allowed, set())


if __name__ == "__main__":
    unittest.main()
