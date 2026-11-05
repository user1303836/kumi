"""kumi_transport (CC-17), W2's side: play asked in the runtime's words under the chat-scoped playback class, stop
never asked, each DAW's own place to start, refusals before anyone is asked, and no HISTORY row."""
from __future__ import annotations

import unittest

import support

policy = support.module("policy")
tools = support.module("tools")

KEY = {"expected_project_key": "live:night-drive"}


class Transport(unittest.TestCase):
    def setUp(self):
        self.host, self.ctx, self.data = support.host(self)
        support.call(self.host, "kumi_connect", {}, session_id="s1")

    def hook(self, args: dict) -> dict | None:
        return policy.pre_tool_call(self.host, tool_name="kumi_transport", args=args, session_id="s1",
                                    tool_call_id="p1")

    def sent(self) -> list[dict]:
        return [m["params"]["arguments"] for m in support.requests(self.data / "state")
                if m.get("params", {}).get("name") == "transport" and not m["params"]["arguments"].get("preview")]

    def test_play_is_asked_in_the_runtimes_words_and_stop_never_is(self):
        launch = {"action": "play", "clip_ref": "t1/s0", **KEY}
        self.assertEqual(self.hook(launch), {"action": "approve", "rule_key": "kumi:playback:transport:s1",
                                             "message": f"Kumi: Play Night Drive from clip ref t1/s0\n{policy.CHAT_ONLY}"})
        self.assertEqual(self.sent(), [], "only the runtime's preview so far")
        self.assertEqual(support.call(self.host, "kumi_transport", launch, session_id="s1")["state"], "playing")
        self.assertIsNone(self.hook({"action": "stop"}), "stopping is never asked")
        self.assertEqual(support.call(self.host, "kumi_transport", {"action": "stop"}, session_id="s1")["state"],
                         "stopped")
        self.assertEqual(support.call(self.host, "kumi_history", {})["items"], [], "playback writes no HISTORY")
        self.assertEqual(self.hook({}), {"action": "block", "message": "Kumi didn't play anything: action is play or "
                                                                       "stop."})

    def test_each_daw_takes_its_own_place_to_start_and_refuses_another_in_words(self):
        for daw, args, said in (
                ("ableton", {"action": "play", "from_bar": 9, **KEY}, "Live plays the Arrangement from its own start "
                                                                      "marker here"),
                ("renoise", {"action": "play", "clip_ref": "t1/s0", **KEY}, "Renoise plays from a sequence slot"),
                ("reaper", {"action": "play", "from_bar": 9, "from_beat": 4, **KEY}, "play starts from one place"),
                ("reaper", {"action": "play", "from_bar": 9}, "play takes kumi_observe's project_key"),
                ("reaper", {"action": "stop", "from_bar": 9}, "stop takes nothing else")):
            with self.subTest(daw=daw, args=args):
                self.ctx.settings["daw"] = daw
                decision = self.hook(args)
                self.assertEqual(decision["action"], "block")
                self.assertIn(said, decision["message"])
                self.assertEqual(tools.kumi_transport(self.host, args, session_id="s1")["error"]["code"], "invalid")
        self.ctx.settings["daw"] = "reaper"
        self.assertEqual(self.hook({"action": "play", "from_bar": 9, **KEY})["action"], "approve")
        self.ctx.settings["daw"] = "renoise"
        self.assertEqual(self.hook({"action": "play", "from_slot": 2, "from_line": 16, **KEY})["action"], "approve")
        self.assertEqual(self.sent(), [], "nothing played")

    def test_play_is_refused_unasked_while_the_daw_records_and_where_nobody_can_answer(self):
        play = {"action": "play", **KEY}
        recording, _, data = support.host(self, None, "--recording")
        support.call(recording, "kumi_connect", {}, session_id="s1")
        decision = policy.pre_tool_call(recording, tool_name="kumi_transport", args=play, session_id="s1",
                                        tool_call_id="p2")
        self.assertEqual(decision["action"], "block")
        self.assertIn("is recording, or record is armed", decision["message"])
        self.ctx.surface = "webhook"
        self.assertEqual(self.hook(play), {"action": "block", "message": policy.UNATTENDED})
        self.assertIsNone(self.hook({"action": "stop"}), "stopping is always allowed")


if __name__ == "__main__":
    unittest.main()
