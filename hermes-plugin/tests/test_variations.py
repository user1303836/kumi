"""CC-24: variations through serve. A make or a change is asked with every take the runtime's preview drew, and runs
exactly those takes or nothing; list is a read; drop and extract never run here."""
from __future__ import annotations

import unittest

import support

policy = support.module("policy")

PROJECT = {"expected_project_key": "live:night-drive", "expected_epoch": "1"}


def make(**fields) -> dict:
    return {"action": "make", "source": {"clipRef": "t2/c1"}, **PROJECT, **fields}


class Variations(unittest.TestCase):
    def setUp(self):
        self.host, self.ctx, self.data = support.host(self)
        support.call(self.host, "kumi_connect", {}, session_id="s1")

    def hook(self, args: dict) -> dict | None:
        return policy.pre_tool_call(self.host, tool_name="kumi_variations", args=args, session_id="s1",
                                    tool_call_id="c1", task_id="t1", turn_id="u1", api_request_id="r1",
                                    middleware_trace=[])

    def test_a_make_is_asked_with_every_take_and_writes_those_takes(self):
        args = make(takes=[{"direction": "busier hats"}, {"direction": "half-time feel"}])
        decision = self.hook(args)
        self.assertEqual(decision["rule_key"], "kumi:write:clip:s1")
        self.assertEqual(decision["message"].splitlines()[:3], [
            "Kumi: make 2 takes of “Bassline” on Bass, each its own, never over it",
            "• V1 (busier hats): 8 bars, 31 notes",
            "• V2 (half-time feel): 8 bars, 31 notes"])
        made = support.call(self.host, "kumi_variations", args, session_id="s1")
        self.assertEqual((made["state"], len(made["history_ids"])), ("applied", 2), "one HISTORY row per take")
        keep = {"action": "keep", "names": ["V1"], "expected_project_key": made["project"]["key"],
                "expected_epoch": str(made["project"]["epoch"])}
        asked = self.hook(keep)
        self.assertEqual((asked["rule_key"], asked["message"].splitlines()[:2]), (
            "kumi:write:clip:s1", ["Kumi: keep 1 take of the source, renamed", '• V1 → "V1 · Bassline · kept"']))
        undone = support.call(self.host, "kumi_undo", {}, session_id="s1")
        self.assertEqual((undone["state"], sorted(row["history_id"] for row in undone["rows"])),
                         ("undone", sorted(made["history_ids"])), "\"undo that\" takes the whole make back, every take")

    def test_takes_that_would_come_out_differently_after_the_approval_are_never_written(self):
        approved = make(count=2)
        self.assertEqual(self.hook(approved)["action"], "approve")
        support.call(self.host, "kumi_variations", make(count=3), session_id="s1")  # a make in between: V3 is taken
        late = support.handle(self.host, "kumi_variations", approved, session_id="s1", task_id="t1")
        self.assertEqual((late["state"], late["code"]), ("refused", "stale"), "the approved V1 and V2 aren't these")
        self.assertEqual(len(support.call(self.host, "kumi_history", {}, session_id="s1")["items"]), 3)

    def test_the_runtimes_refusal_is_said_before_anyone_is_asked(self):
        decision = self.hook(make(count=8))
        self.assertEqual(decision["action"], "block")
        self.assertIn("Only 6 of the 8 takes fit", decision["message"], "24-r2: refused, never trimmed")

    def test_list_reads_and_drop_and_extract_never_run(self):
        self.assertIsNone(self.hook({"action": "list"}))
        self.assertEqual(support.call(self.host, "kumi_variations", {"action": "list"}, session_id="s1")["source"],
                         "Bassline")
        for action, words in (("drop", "undo the ones you don't want"), ("extract", "doesn't extract")):
            with self.subTest(action=action):
                decision = self.hook({"action": action, "names": ["V1"]})
                self.assertEqual(decision["action"], "block")
                self.assertIn(words, decision["message"])

    def test_many_takes_fit_the_terminals_panel_by_their_directions(self):
        args = make(takes=[{"direction": f"a much busier variation with ghost notes, number {index}"}
                           for index in range(1, 7)])
        decision = self.hook(args)
        self.assertEqual(decision["action"], "approve")
        self.assertTrue(policy.shows_whole(decision["message"], "tui"))
        self.assertIn("V6 a much busier variation with ghost notes, number 6", decision["message"],
                      "each take by its V number and direction, when its full line doesn't fit")


if __name__ == "__main__":
    unittest.main()
