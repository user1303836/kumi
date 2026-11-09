"""hidden.py: the map's sounds that can't play here, hidden quietly until their file is back."""

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


fixtures = _load("kumi_test_ui_fixtures", HERE / "ui_fixtures.py")
snapshots = _load("kumi_test_hidden_snapshots", HERE.parent / "snapshots.py")
hidden = _load("kumi_test_hidden", HERE.parent / "hidden.py")

# In the fixture state (`audio="some"`), s-r7-0091 is a snare whose file is there; s-r7-0001's file isn't.
PLAYS, MISSING = "s-r7-0091", "s-r7-0001"


class Hidden(unittest.TestCase):
    def setUp(self) -> None:
        self.tmp = tempfile.TemporaryDirectory()
        root = Path(self.tmp.name)
        self.library = root / "samples"
        self.state = fixtures.write_state(root / "plugin-data" / "kumi", self.library, audio="some")
        self.current = snapshots.map_revision(self.state)

    def tearDown(self) -> None:
        self.tmp.cleanup()

    def hide(self, reports: list[tuple[str, str]], revision: str = "r7") -> dict:
        folders = [str(self.library)]
        return hidden.hide(
            self.state, "r7", self.current, revision, reports,
            servable=lambda sound_id: snapshots.media(self.state, sound_id, folders, current=self.current) is not None,
            checked=snapshots.local,
        )

    def visible(self) -> dict:
        return hidden.visible(snapshots.public_of_map(self.current), self.current["sounds"], self.state, snapshots.local)

    def path(self, sound_id: str) -> Path:
        return Path(self.current["sounds"][sound_id]["path"])

    def test_gone_is_taken_only_when_the_backend_cant_serve_the_file_either(self) -> None:
        self.assertEqual(self.hide([(MISSING, "gone"), (PLAYS, "gone")]), {"hidden": 1, "ignored": 1, "seq": 1})
        shown = self.visible()
        self.assertEqual(shown["hidden"], 1)
        self.assertNotIn(MISSING, shown["sounds"])
        self.assertIn(PLAYS, shown["sounds"])
        self.assertNotIn(MISSING, [point[0] for point in shown["points"]])
        self.assertEqual(len(shown["points"]), len(self.current["points"]) - 1)
        group = str(next(point[3] for point in self.current["points"] if point[0] == MISSING))
        self.assertEqual(shown["groups"][group]["count"], self.current["groups"][group]["count"] - 1)
        # The file holds no id (they change every revision) and stays the plugin's own.
        saved = json.loads((self.state / hidden.FILE).read_text(encoding="utf-8"))
        self.assertEqual(saved["items"][0]["path"], str(self.path(MISSING)))
        self.assertEqual((saved["items"][0]["size"], saved["items"][0]["mtime"], saved["items"][0]["why"]), (None, None, "gone"))

    def test_a_gone_file_that_is_back_shows_again(self) -> None:
        self.hide([(MISSING, "gone")])
        self.path(MISSING).parent.mkdir(parents=True, exist_ok=True)
        self.path(MISSING).write_bytes(b"RIFF")
        self.assertEqual(self.visible()["hidden"], 0)

    def test_unplayable_is_taken_as_reported_until_the_file_changes(self) -> None:
        self.assertEqual(self.hide([(PLAYS, "unplayable")])["hidden"], 1)
        self.assertNotIn(PLAYS, self.visible()["sounds"])
        info = os.stat(self.path(PLAYS))
        with open(self.path(PLAYS), "ab") as file:
            file.write(b"\x00\x00")
        os.utime(self.path(PLAYS), (info.st_atime, info.st_mtime))
        self.assertEqual(self.visible()["hidden"], 0, "a changed size alone shows it again")

    def test_only_ids_of_the_current_revision_count(self) -> None:
        self.assertEqual(self.hide([(MISSING, "gone")], revision="r6"), {"hidden": 0, "ignored": 1, "seq": 0})
        self.assertEqual(self.hide([("s-r7-9999", "unplayable"), ("s-r6-0001", "gone")]), {"hidden": 0, "ignored": 2, "seq": 0})
        self.assertFalse((self.state / hidden.FILE).exists())
        # The answer counts the current revision's hidden points even when a stale client's report is ignored.
        self.hide([(MISSING, "gone")])
        self.assertEqual(self.hide([(MISSING, "gone")], revision="r6"), {"hidden": 1, "ignored": 1, "seq": 1})

    def test_clear_shows_everything_again(self) -> None:
        self.hide([(MISSING, "gone"), (PLAYS, "unplayable")])
        self.assertEqual(hidden.clear(self.state), {"hidden": 0, "ignored": 0, "seq": 2})
        self.assertEqual(self.visible()["hidden"], 0)
        self.assertEqual(hidden.seq(self.state), 2)

    def test_a_file_is_kept_once_and_the_oldest_go_past_the_cap(self) -> None:
        saved = hidden.MAX_ITEMS
        hidden.MAX_ITEMS = 3
        try:
            for number in (1, 2, 3, 4):
                self.hide([(f"s-r7-{number:04d}", "gone")])
            self.hide([("s-r7-0002", "unplayable")])
        finally:
            hidden.MAX_ITEMS = saved
        items = hidden.read(self.state)["items"]
        self.assertEqual([item["path"] for item in items], [str(self.path(f"s-r7-{n:04d}")) for n in (3, 4, 2)])
        self.assertEqual(items[-1]["why"], "unplayable")
        self.assertEqual(hidden.seq(self.state), 5)

    def test_a_missing_or_foreign_file_reads_as_nothing_hidden(self) -> None:
        self.assertEqual((hidden.read(self.state), hidden.seq(self.state)), ({"seq": 0, "items": []}, 0))
        (self.state / hidden.FILE).write_text('{"schema": 2, "seq": 9, "items": []}', encoding="utf-8")
        self.assertEqual((hidden.seq(self.state), self.visible()["hidden"]), (0, 0))
        (self.state / hidden.FILE).write_text("not json", encoding="utf-8")
        self.assertEqual(self.hide([(MISSING, "gone")])["seq"], 1)

    def test_only_the_two_documented_bodies_parse(self) -> None:
        one = {"id": MISSING, "why": "gone"}
        self.assertEqual(hidden.parse({"action": "clear"}), ("clear", None, []))
        self.assertEqual(hidden.parse({"revision": "r7", "ids": [one] * 500})[2], [(MISSING, "gone")] * 500)
        for body in (
            [], "clear", {"action": "forget"}, {"action": "clear", "revision": "r7"},
            {"revision": "r7", "ids": []}, {"revision": "r7", "ids": [one] * 501}, {"revision": "", "ids": [one]},
            {"revision": "r7"}, {"revision": "r7", "ids": [one], "extra": 1},
            {"revision": "r7", "ids": [{"id": "x" * 129, "why": "gone"}]}, {"revision": "r7", "ids": [{"id": MISSING, "why": "broken"}]},
            {"revision": "r7", "ids": [{"id": MISSING}]}, {"revision": "r7", "ids": [{"id": 7, "why": "gone"}]},
        ):
            with self.assertRaises(hidden.Invalid, msg=body):
                hidden.parse(body)

    @unittest.skipIf(os.name == "nt", "flock")
    def test_a_write_waits_briefly_for_the_lock_then_says_busy(self) -> None:
        import fcntl

        saved = hidden.LOCK_WAIT_S
        hidden.LOCK_WAIT_S = 0.05
        try:
            with open(self.state / hidden.LOCK, "a+b") as other:
                fcntl.flock(other.fileno(), fcntl.LOCK_EX)
                with self.assertRaises(hidden.Busy):
                    hidden.clear(self.state)
                fcntl.flock(other.fileno(), fcntl.LOCK_UN)
            self.assertEqual(hidden.clear(self.state)["seq"], 1)
        finally:
            hidden.LOCK_WAIT_S = saved


if __name__ == "__main__":
    unittest.main()
