"""policy.py (C6, CC-3): what the producer is asked before a Kumi call runs, in the runtime's own words, and what is
blocked before anyone is asked."""
from __future__ import annotations

import unittest

import support

policy = support.module("policy")

PROJECT = {"expected_project_key": "live:night-drive", "expected_epoch": "1"}


def volume(ref: str, db: float) -> dict:
    return {"tool": "set_track_volume", "input": {"track_ref": ref, "volume_db": db}}


class Approvals(unittest.TestCase):
    def setUp(self):
        self.host, self.ctx, self.data = support.host(self)
        support.call(self.host, "kumi_connect", {}, session_id="s1")

    def hook(self, tool: str, args: dict) -> dict | None:
        return policy.pre_tool_call(self.host, tool_name=tool, args=args, session_id="s1", tool_call_id="c1",
                                    task_id="t1", turn_id="u1", api_request_id="r1", middleware_trace=[])

    def change(self, *steps: dict, **project) -> dict:
        return {"title": "Louder drums", "steps": list(steps or [volume("t1", -4)]), **PROJECT, **project}

    def test_reads_connect_and_other_plugins_tools_need_no_approval(self):
        for tool in ("kumi_status", "kumi_connect", "kumi_observe", "kumi_read", "kumi_history", "kumi_sounds",
                     "kumi_listen", "kumi_job", "terminal", "elevenlabs_sound_effect"):
            self.assertIsNone(self.hook(tool, {"scope": "focus"}), tool)
        self.assertIsNone(self.hook("kumi_library", {"action": "status"}))

    def test_a_change_is_approved_in_the_runtimes_words_and_nothing_runs_first(self):
        decision = self.hook("kumi_change", self.change())
        self.assertEqual(decision, {"action": "approve", "rule_key": "kumi:write:mixer", "message": (
            "Kumi: Louder drums\n• Set Drums volume from -6 dB to -4 dB\n(Values as Kumi read them just now.)")})
        self.assertEqual(support.call(self.host, "kumi_history", {})["items"], [], "the preview wrote nothing")
        asked = [m["params"]["arguments"] for m in support.requests(self.data / "state")
                 if m.get("params", {}).get("name") == "change"]
        self.assertEqual([args.get("preview") for args in asked], [True])

    def test_a_plans_families_make_its_rule_key_so_always_covers_only_that_kind(self):
        sample = {"tool": "replace_sample", "input": {"track_ref": "t1", "sample": "Dusty Snare 04.wav"}}
        decision = self.hook("kumi_change", self.change(sample, volume("t2", -10)))
        self.assertEqual(decision["rule_key"], "kumi:write:mixer+sample")
        self.assertIn("• Replace the sample in the Simpler on Drums > Drum Rack pad D1 (Snare 01.wav) with Dusty "
                      "Snare 04.wav", decision["message"])

    def test_a_plan_the_runtime_would_refuse_is_blocked_before_anyone_is_asked(self):
        for args, reason in ((self.change(expected_epoch="0"), "stale"),
                             (self.change({"tool": "delete_track", "input": {"track_ref": "t1"}}), "delete_track"),
                             (self.change(volume("t9", -4)), "no track"),
                             (self.change(*[volume("t1", -4)] * 21), "doesn't take these steps")):
            with self.subTest(reason=reason):
                decision = self.hook("kumi_change", args)
                self.assertEqual(decision["action"], "block")
                self.assertIn(reason, decision["message"])
        self.assertEqual(support.call(self.host, "kumi_history", {})["items"], [])

    def test_a_long_plan_is_shown_in_twenty_lines(self):
        decision = self.hook("kumi_change", self.change(*[volume("t1", -db) for db in range(1, 21)]))
        lines = decision["message"].splitlines()
        self.assertEqual(len(lines), 20)
        self.assertEqual(lines[-1], "… and 3 more")

    def test_an_undo_is_approved_by_what_it_undoes_and_an_expired_one_is_blocked(self):
        applied = support.call(self.host, "kumi_change", self.change(), session_id="s1")
        undo = {"history_id": applied["history_ids"][0]}
        self.assertEqual(self.hook("kumi_undo", undo), {"action": "approve", "message": "Kumi: Undo Louder drums on Drums",
                                                         "rule_key": "kumi:write:undo"})
        self.assertEqual(support.call(self.host, "kumi_undo", undo, session_id="s1")["state"], "undone")
        decision = self.hook("kumi_undo", undo)
        self.assertEqual((decision["action"], decision["message"]), ("block", "Kumi didn't undo anything: expired"))

    def test_adding_a_folder_and_scanning_say_what_kumi_will_read(self):
        decision = self.hook("kumi_library", {"action": "add_folder", "folder": "/Samples/Drums"})
        self.assertEqual(decision["rule_key"], "kumi:folder_read:/Samples/Drums")
        self.assertIn("read the folder /Samples/Drums and everything in it", decision["message"])
        self.ctx.settings["library_folders"] = ["/Samples/Drums", "/Samples/Pads"]
        decision = self.hook("kumi_library", {"action": "scan"})
        self.assertEqual(decision["rule_key"], "kumi:folder_read:scan")
        self.assertTrue(decision["message"].endswith("• /Samples/Drums\n• /Samples/Pads"), decision["message"])
        self.assertEqual(self.hook("kumi_library", {"action": "erase"})["action"], "block")


class Blocked(unittest.TestCase):
    def hook(self, host, tool: str, args: dict) -> dict | None:
        return policy.pre_tool_call(host, tool_name=tool, args=args, session_id="s1", tool_call_id="c1")

    def test_without_a_running_runtime_a_change_or_undo_is_blocked(self):
        host, _, _ = support.host(self)
        for tool, args in (("kumi_change", {"title": "x", "steps": [volume("t1", -4)], **PROJECT}),
                           ("kumi_undo", {"history_id": "h-1"})):
            decision = self.hook(host, tool, args)
            self.assertEqual(decision["action"], "block")
            self.assertIn("kumi_connect", decision["message"])

    def test_an_unknown_kumi_tool_and_a_failing_check_block(self):
        host, _, _ = support.host(self)
        self.assertEqual(self.hook(host, "kumi_erase_everything", {})["action"], "block")

        class Broken:
            def live(self):
                raise RuntimeError("boom")

        with self.assertLogs(policy.log, "ERROR"):
            decision = self.hook(Broken(), "kumi_change", {"title": "x", "steps": []})
        self.assertEqual(decision["action"], "block")
        self.assertIn("didn't run", decision["message"])


if __name__ == "__main__":
    unittest.main()
