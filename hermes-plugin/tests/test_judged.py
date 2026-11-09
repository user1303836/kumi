"""Judged rounds, tunes and wavetables (serve's judge, tune and wavetable ops, Live only): unasked by default, asked in
their arguments' words with "Ask before each change" on, never unattended, forked or scheduled, refused before serve
when their arguments are malformed, and sent once as cleared."""
from __future__ import annotations

import os
import unittest
from unittest import mock

import support
import test_policy

policy = support.module("policy")
tools = support.module("tools")

PROJECT = test_policy.PROJECT
GOAL = {"goal": {"loudness": -14, "true_peak": -1}, "track": "Bass", "from_beat": 16, "beats": 32, **PROJECT}
ROUND = {"change": "Cut 300 Hz on Bass by 2 dB", **PROJECT}
DONE = {"done": True, **PROJECT}
TUNE = {"device": "t2/d1", "how": "search", "knobs": ["Threshold", "Ratio"], **PROJECT}
TABLE = {"device_ref": "t3/d0", "wavetable": {"name": "Glass", "keyframes": [{"shape": "saw"}, {"shape": "sine"}],
                                              "count": 64}}
CALLS = (("kumi_judge", GOAL), ("kumi_judge", ROUND), ("kumi_judge", DONE), ("kumi_tune", TUNE),
         ("kumi_wavetable", TABLE))


class Judged(unittest.TestCase):
    def use(self, ask: bool | None = None, daw: str = "ableton") -> None:
        self.host, self.ctx, self.data = support.host(self, {"daw": daw}, ask_before_changes=ask)
        self.ctx.surface = "desktop"
        support.call(self.host, "kumi_connect", {}, session_id="s1")

    def hook(self, tool: str, args: dict, call: str = "c1", **context) -> dict | None:
        return policy.pre_tool_call(self.host, tool_name=tool, args=args, **{
            "session_id": "s1", "tool_call_id": call, "task_id": "t1", "turn_id": "u1", **context})

    def sent(self, name: str) -> list[dict]:
        return [{key: value for key, value in m["params"]["arguments"].items() if key != "_host"}
                for m in support.requests(self.data / "state") if m.get("params", {}).get("name") == name]

    def test_policy_matrix(self):
        """Each call's decision: unasked by default; asked with the setting on, under its key; blocked unattended,
        forked and scheduled whatever the setting."""
        keys = {"kumi_judge": "kumi:write:judge:s1", "kumi_tune": "kumi:write:tune:s1"}
        for ask in (False, True):
            self.use(ask)
            for tool, args in CALLS:
                with self.subTest(tool=tool, args=args, ask=ask):
                    decision = self.hook(tool, args)
                    if not ask:
                        self.assertIsNone(decision)
                        continue
                    self.assertEqual(decision["action"], "approve")
                    expected = keys.get(tool, f"kumi:wavetable:{support.module('calls').digest(args)}")
                    self.assertEqual(decision["rule_key"], expected)
            for tool, args in CALLS:
                with self.subTest(tool=tool, ask=ask, turn="webhook"):
                    self.ctx.surface = "webhook"
                    self.assertEqual(self.hook(tool, args), policy.block(policy.UNATTENDED))
                with self.subTest(tool=tool, ask=ask, turn="one-shot"), \
                        mock.patch.dict(os.environ, {"HERMES_SINGLE_QUERY_SESSION": "1"}):
                    self.ctx.surface = "desktop"
                    self.assertEqual(self.hook(tool, args), policy.block(policy.UNATTENDED))
                with self.subTest(tool=tool, ask=ask, turn="forked"):
                    policy.turn_started(self.host, session_id="s1", turn_id="u1")
                    self.assertEqual(self.hook(tool, args, turn_id="s1:t2:9f3a1c2e"), policy.block(policy.FORKED))
                with self.subTest(tool=tool, ask=ask, turn="scheduled"):
                    self.ctx.cron_execution = object()
                    self.assertEqual(self.hook(tool, args), policy.block(tools.SCHEDULED))
                    self.ctx.cron_execution = None
            self.assertEqual(self.sent("judge") + self.sent("tune") + self.sent("wavetable"), [],
                             "the hook never calls serve for these")

    def test_the_approval_says_what_is_heard_how_and_what_a_round_keeps(self):
        self.use(True)
        start = self.hook("kumi_judge", GOAL)["message"].split("\n")
        self.assertEqual(start[:3], ['Kumi: start a judged run on track "Bass", beats 16–48',
                                     "Toward: -14 LUFS; true peak at most -1 dBTP; clearing the problems it finds",
                                     policy.JUDGED_PLAYS])
        round_lines = self.hook("kumi_judge", ROUND, "c2")["message"].split("\n")
        self.assertEqual(round_lines[0], 'Kumi: judge this round\'s change, "Cut 300 Hz on Bass by 2 dB"')
        self.assertIn(policy.JUDGED_KEEPS, round_lines)
        mix = self.hook("kumi_judge", {key: value for key, value in GOAL.items() if key != "track"}, "c3")["message"]
        self.assertTrue(mix.startswith("Kumi: start a judged run on the mix, beats 16–48"))
        tune = self.hook("kumi_tune", TUNE, "c4")["message"].split("\n")
        self.assertEqual(tune[0], 'Kumi: tune device "t2/d1" (search: "Threshold", "Ratio") toward the run\'s next '
                                  'target')
        self.assertIn(policy.SCRATCH, tune)
        self.assertIn(policy.JUDGED_KEEPS, tune)
        fit = self.hook("kumi_tune", {"device": "t2/d2", "how": "fit", **PROJECT}, "c5")["message"]
        self.assertNotIn(policy.SCRATCH, fit, "fit makes no scratch copies")
        table = self.hook("kumi_wavetable", TABLE, "c6")["message"].split("\n")
        self.assertEqual(table[:3], ['Kumi: make "Glass" for plug-in "t3/d0"', "2 keyframes morphing, 64 frames",
                                     policy.WAVETABLE_WARNING])

    def test_malformed_arguments_are_refused_before_serve_whatever_the_setting(self):
        source = {"sound_id": "s-1", "map_revision": "m1"}
        malformed = (
            ("kumi_judge", {**GOAL, "goal": {"reference": "~/Refs/song.wav"}}),
            ("kumi_judge", {**GOAL, "change": "louder"}),
            ("kumi_judge", {"done": False, **PROJECT}),
            ("kumi_judge", {**ROUND, "from_beat": 0}),
            ("kumi_judge", {key: value for key, value in GOAL.items() if key != "beats"}),
            ("kumi_judge", {**GOAL, "beats": 100000}),
            ("kumi_judge", {"change": "louder"}),
            ("kumi_judge", {**ROUND, "expected_epoch": True}),
            ("kumi_tune", {**TUNE, "knobs": ["Threshold"]}),
            ("kumi_tune", {**TUNE, "how": "guess"}),
            ("kumi_wavetable", {**TABLE, "wavetable": {"from_audio": {"file": "/tmp/a.wav"}}}),
            ("kumi_wavetable", {**TABLE, "wavetable": {"from_audio": {"source": "/tmp/a.wav"}}}),
            ("kumi_wavetable", {**TABLE, "file": "/tmp/x.wav"}),
        )
        for ask in (False, True):
            self.use(ask)
            for tool, args in malformed:
                with self.subTest(tool=tool, args=args, ask=ask):
                    self.assertEqual(self.hook(tool, args)["action"], "block")
        for tool, args in (("kumi_judge", {**GOAL, "goal": {"reference": {"clip_ref": "t4/c0"}}}),
                           ("kumi_judge", {**GOAL, "goal": {"reference": source}}),
                           ("kumi_wavetable", {**TABLE, "wavetable": {"from_audio": {"source": source}}})):
            with self.subTest(tool=tool, args=args, accepted=True):
                self.assertEqual(self.hook(tool, args, "ok")["action"], "approve")
        self.use(daw="reaper")
        self.assertEqual(self.hook("kumi_judge", GOAL), policy.block(policy.JUDGED_ONLY_LIVE))
        self.assertEqual(self.hook("kumi_wavetable", TABLE), policy.block(policy.WAVETABLE_ONLY_LIVE))

    def test_handler_sends_the_cleared_args_once_without_preview(self):
        self.use()
        for tool, args, op in (("kumi_judge", ROUND, "judge"), ("kumi_tune", TUNE, "tune"),
                               ("kumi_wavetable", TABLE, "wavetable")):
            with self.subTest(tool=tool):
                asked = {**args, "preview": True}
                self.assertIsNone(self.hook(tool, asked, f"{tool}-1"))
                answer = support.handle(self.host, tool, asked, session_id="s1", task_id="t1")
                self.assertNotIn("error", answer)
                [write] = self.sent(op)
                self.assertEqual(write, {key: value for key, value in args.items()}, "exactly what was cleared")
                again = support.handle(self.host, tool, args, session_id="s1", task_id="t1")
                self.assertEqual(again["error"]["code"], "not_approved", "the clearance is used once")
                self.assertEqual(len(self.sent(op)), 1)
        epoch = {**ROUND, "expected_epoch": 1}
        self.assertIsNone(self.hook("kumi_judge", epoch, "e-1"))
        support.handle(self.host, "kumi_judge", epoch, session_id="s1", task_id="t1")
        self.assertEqual(self.sent("judge")[-1]["expected_epoch"], "1", "the epoch goes as a string")

    def test_a_round_is_undoable_a_round_taken_back_isnt_undone_again_and_a_wavetable_stays(self):
        self.use()
        kept = support.call(self.host, "kumi_tune", TUNE, session_id="s1")
        self.assertEqual(kept["state"], "done")
        undone = support.call(self.host, "kumi_undo", {}, session_id="s1")
        self.assertEqual(self.sent("undo")[-1]["history_id"], kept["history_ids"][-1], undone)
        support.call(self.host, "kumi_change", test_policy.change(), session_id="s1")
        worse = support.call(self.host, "kumi_judge", {**ROUND, "change": "worse now"}, session_id="s1")
        self.assertEqual((worse["history_ids"], len(worse["taken_back"])), ([], 1))
        self.assertEqual(support.call(self.host, "kumi_undo", {}, session_id="s1"), {"blocked": tools.NOTHING_TO_UNDO},
                         "what the round took back isn't undone again")
        made = support.call(self.host, "kumi_wavetable", TABLE, session_id="s1")
        self.assertEqual((made["file"], made["folder"]), ("Glass.wav", "Vital's wavetable folder"))
        self.assertEqual(support.call(self.host, "kumi_undo", {}, session_id="s1"),
                         {"blocked": tools.FILE_STAYS.format(file="Glass.wav into Vital's wavetable folder")},
                         "the file stays after an undo")

    def test_a_lost_answer_is_unsure_and_never_sent_again(self):
        for tool, args, op in (("kumi_judge", ROUND, "judge"), ("kumi_wavetable", TABLE, "wavetable")):
            with self.subTest(tool=tool):
                self.host, self.ctx, self.data = support.host(self, None, "--slow", f"{op}=2")
                support.call(self.host, "kumi_connect", {}, session_id="s1")
                with mock.patch.object(tools, "JUDGED_TIMEOUT", 0.3), mock.patch.object(tools, "WRITE_TIMEOUT", 0.3):
                    unsure = support.call(self.host, tool, args, session_id="s1")
                self.assertEqual(unsure.get("state"), "unsure", unsure)
                self.assertEqual(len(self.sent(op)), 1)
                if tool == "kumi_wavetable":
                    self.assertEqual(unsure["hint"], tools.WAVETABLE_UNSURE_HINT)


if __name__ == "__main__":
    unittest.main()
