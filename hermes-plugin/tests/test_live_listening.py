"""Live's other listening tools (serve's sound, audition, groove and render ops, Live only). sound, audition and render
are listens and groove's change or apply is a change: all run unasked by default and are asked in their arguments' words
with "Ask before each change" on. A groove start or done is never asked. None of them runs unattended, forked or
scheduled. Malformed arguments (a path for a reference, a groove's audio stem) are refused before serve, and each call
is sent once as cleared."""
from __future__ import annotations

import os
import unittest
from unittest import mock

import support
import test_policy

policy = support.module("policy")
tools = support.module("tools")

PROJECT = test_policy.PROJECT
SOUND = {"tracks": ["Kick", "Bass"], "against": "Kick", "from_beat": 16, **PROJECT}
AUDITION = {"candidates": [{"track": "t1", "label": "Warm"}, {"track": "t2", "label": "Bright"}], "from_beat": 0,
            "beats": 16, "reference": {"sound_id": "s-1", "map_revision": "m1"}, **PROJECT}
CLIPS = {"candidates": [{"track": "t1", "clip": "t1/c0"}, {"track": "t2", "clip": "t2/c0"}], **PROJECT}
START = {"clip": "1:clip:4", "reference": "1:clip:5", **PROJECT}
CHANGE = {"change": "Pushed the hats later", **PROJECT}
APPLY = {"apply": True, "amount": 0.5, **PROJECT}
DONE = {"done": True, **PROJECT}
RENDER = {"track": "t3", "from_beat": 0, "beats": 16, **PROJECT}
ASKED = (("kumi_sound", SOUND, "kumi:playback:sound:s1"), ("kumi_audition", AUDITION, "kumi:playback:audition:s1"),
         ("kumi_audition", CLIPS, "kumi:playback:audition:s1"), ("kumi_groove", CHANGE, "kumi:write:groove:s1"),
         ("kumi_groove", APPLY, "kumi:write:groove:s1"), ("kumi_render", RENDER, "kumi:playback:render:s1"))
READS = (("kumi_groove", START), ("kumi_groove", DONE))


class LiveListening(unittest.TestCase):
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
        """Each call's decision: unasked by default; asked with the setting on, under its key (a groove start or done
        never); blocked unattended, forked and scheduled whatever the setting."""
        for ask in (False, True):
            self.use(ask)
            for tool, args, key in ASKED:
                with self.subTest(tool=tool, args=args, ask=ask):
                    decision = self.hook(tool, args)
                    if not ask:
                        self.assertIsNone(decision)
                        continue
                    self.assertEqual((decision["action"], decision["rule_key"]), ("approve", key))
            for tool, args in READS:
                with self.subTest(tool=tool, args=args, ask=ask, read=True):
                    self.assertIsNone(self.hook(tool, args))
            for tool, args in [*((tool, args) for tool, args, _ in ASKED), *READS]:
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
            self.assertEqual([op for op in ("sound", "audition", "groove", "render") if self.sent(op)], [],
                             "the hook never calls serve for these")

    def test_the_approval_says_what_is_heard_and_what_is_put_back(self):
        self.use(True)
        sound = self.hook("kumi_sound", SOUND)["message"].split("\n")
        self.assertEqual(sound[:2], ['Kumi: measure "Kick", "Bass" against "Kick", 8 bars from beat 16',
                                     policy.LISTENING_PLAYS])
        audition = self.hook("kumi_audition", AUDITION, "c2")["message"].split("\n")
        self.assertEqual(audition[:3], ['Kumi: audition "Warm", "Bright", beats 0–16', 'Against "s-1", 60 s of it',
                                        policy.LISTENING_PLAYS])
        self.assertNotIn(policy.AUDITION_COPIES, audition)
        clips = self.hook("kumi_audition", CLIPS, "c3")["message"].split("\n")
        self.assertEqual(clips[0], 'Kumi: audition "t1", "t2", their Session clips')
        self.assertIn(policy.AUDITION_COPIES, clips)
        render = self.hook("kumi_render", RENDER, "c4")["message"].split("\n")
        self.assertEqual(render[:2], ['Kumi: render track "t3"\'s own clips, beats 0–16', policy.RENDER_KEPT])
        change = self.hook("kumi_groove", CHANGE, "c5")["message"].split("\n")
        self.assertEqual(change[:2], ['Kumi: judge this groove change, "Pushed the hats later"', policy.GROOVE_KEEPS])
        apply = self.hook("kumi_groove", APPLY, "c6")["message"].split("\n")
        self.assertEqual(apply[:2], ["Kumi: move the part's notes toward the reference's feel (0.5 of the way)",
                                     policy.GROOVE_APPLY])

    def test_malformed_arguments_are_refused_before_serve_whatever_the_setting(self):
        malformed = (
            ("kumi_sound", {**SOUND, "tracks": []}),
            ("kumi_sound", {**SOUND, "tracks": [f"t{n}" for n in range(9)]}),
            ("kumi_sound", {key: value for key, value in SOUND.items() if key != "from_beat"}),
            ("kumi_sound", {**SOUND, "beats": 1000}),
            ("kumi_audition", {**AUDITION, "reference": "~/Refs/song.wav"}),
            ("kumi_audition", {**AUDITION, "reference_seconds": 90}),
            ("kumi_audition", {key: value for key, value in AUDITION.items() if key != "from_beat"}),
            ("kumi_audition", {**AUDITION, "candidates": [{"mix": True}, {"track": "t1"}]}),
            ("kumi_audition", {**CLIPS, "reference_seconds": 10}),
            ("kumi_groove", {**START, "audio": "1:clip:6"}),
            ("kumi_groove", {**START, "reference_tempo": 120}),
            ("kumi_groove", {**START, "reference": "~/drums.wav"}),
            ("kumi_groove", {**START, "reference": "beat.mid"}),
            ("kumi_groove", {**START, "reference": "/Users/me/drums.wav"}),
            ("kumi_groove", {**CHANGE, "apply": True}),
            ("kumi_groove", {"amount": 0.5, **PROJECT}),
            ("kumi_groove", {"done": False, **PROJECT}),
            ("kumi_render", {key: value for key, value in RENDER.items() if key != "beats"}),
            ("kumi_render", {**RENDER, "file": "/tmp/x.wav"}),
            ("kumi_render", {**RENDER, "expected_epoch": True}),
        )
        for ask in (False, True):
            self.use(ask)
            for tool, args in malformed:
                with self.subTest(tool=tool, args=args, ask=ask):
                    self.assertEqual(self.hook(tool, args)["action"], "block")
        self.use(True)
        for tool, args in (("kumi_audition", {**AUDITION, "reference": {"clip_ref": "t4/c0"}}),
                           ("kumi_audition", {**CLIPS, "reference": {"artifact_id": "a-1"}, "reference_seconds": 30})):
            with self.subTest(tool=tool, args=args, accepted=True):
                self.assertEqual(self.hook(tool, args, "ok")["action"], "approve")
        self.use(daw="reaper")
        for tool, args, _ in ASKED:
            with self.subTest(tool=tool, daw="reaper"):
                self.assertEqual(self.hook(tool, args), policy.block(policy.LISTENING_ONLY_LIVE))

    def test_handler_sends_the_cleared_args_once_without_preview(self):
        self.use()
        for tool, args, op in (("kumi_sound", SOUND, "sound"), ("kumi_audition", AUDITION, "audition"),
                               ("kumi_groove", APPLY, "groove"), ("kumi_render", RENDER, "render")):
            with self.subTest(tool=tool):
                asked = {**args, "preview": True}
                self.assertIsNone(self.hook(tool, asked, f"{tool}-1"))
                answer = support.handle(self.host, tool, asked, session_id="s1", task_id="t1")
                self.assertNotIn("error", answer)
                [write] = self.sent(op)
                self.assertEqual(write, dict(args), "exactly what was cleared")
                again = support.handle(self.host, tool, args, session_id="s1", task_id="t1")
                self.assertEqual(again["error"]["code"], "not_approved", "the clearance is used once")
                self.assertEqual(len(self.sent(op)), 1)

    def test_a_listen_is_never_undone_a_groove_apply_is_and_a_take_back_isnt_undone_again(self):
        self.use()
        support.call(self.host, "kumi_change", test_policy.change(), session_id="s1")
        changed = self.sent("change")
        heard = support.call(self.host, "kumi_audition", AUDITION, session_id="s1")
        self.assertEqual((heard["state"], len(heard["history_ids"])), ("done", 1))
        support.call(self.host, "kumi_undo", {}, session_id="s1")
        self.assertNotEqual(self.sent("undo")[-1]["history_id"], heard["history_ids"][0],
                            "\"undo that\" after a listen undoes the change before it, never the heard row")
        self.assertTrue(changed)
        support.call(self.host, "kumi_groove", START, session_id="s1")
        applied = support.call(self.host, "kumi_groove", APPLY, session_id="s1")
        support.call(self.host, "kumi_undo", {}, session_id="s1")
        self.assertEqual(self.sent("undo")[-1]["history_id"], applied["history_ids"][-1])
        support.call(self.host, "kumi_groove", APPLY, session_id="s1")
        worse = support.call(self.host, "kumi_groove", {**CHANGE, "change": "worse now"}, session_id="s1")
        self.assertEqual((worse["history_ids"], len(worse["taken_back"])), ([], 1))
        self.assertEqual(support.call(self.host, "kumi_undo", {}, session_id="s1"), {"blocked": tools.NOTHING_TO_UNDO},
                         "what the round took back isn't undone again")
        rendered = support.call(self.host, "kumi_render", RENDER, session_id="s1")
        self.assertEqual(rendered["round"]["artifact_id"], "a-render-1")
        self.assertNotIn("file", rendered["round"])

    def test_a_lost_answer_is_unsure_and_never_sent_again(self):
        self.host, self.ctx, self.data = support.host(self, None, "--slow", "sound=2")
        support.call(self.host, "kumi_connect", {}, session_id="s1")
        with mock.patch.object(tools, "JUDGED_TIMEOUT", 0.3):
            unsure = support.call(self.host, "kumi_sound", SOUND, session_id="s1")
        self.assertEqual(unsure.get("state"), "unsure", unsure)
        self.assertEqual(len(self.sent("sound")), 1)


if __name__ == "__main__":
    unittest.main()
