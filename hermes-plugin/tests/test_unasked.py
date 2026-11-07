"""Kumi's default, "Ask before each change" off (the owner's decision): the changes the producer asks for in the DAW
run without an approval, each still previewed and bound to what its preview saw, while setup, scripts, sample folders
and Mirelo's jobs are asked, a device in a chat that read outside content is asked, and every refusal and the guards
for unattended and forked turns hold."""
from __future__ import annotations

import os
import unittest
from unittest import mock

import support
import test_device
import test_generate
import test_listen
import test_policy
import test_scripts

policy = support.module("policy")
tools = support.module("tools")

change, volume, PROJECT = test_policy.change, test_policy.volume, test_policy.PROJECT


class ByDefault(unittest.TestCase):
    def use(self, settings: dict | None = None, *fixture_args: str, surface: str = "tui") -> None:
        """A fresh install's settings (nothing set for "Ask before each change"), in a chat with the producer in it."""
        self.host, self.ctx, self.data = support.host(self, settings, *fixture_args, ask_before_changes=None)
        self.ctx.surface = surface
        support.call(self.host, "kumi_connect", {}, session_id="s1")

    def setUp(self):
        self.enterContext(mock.patch.object(tools, "broadcast", lambda event, payload: None))  # Desktop's events
        self.use()

    def hook(self, tool: str, args: dict, call: str = "c1", **context) -> dict | None:
        return policy.pre_tool_call(self.host, tool_name=tool, args=args, **{
            "session_id": "s1", "tool_call_id": call, "task_id": "t1", "turn_id": "u1", **context})

    def sent(self, name: str) -> list[dict]:
        return [m["params"]["arguments"] for m in support.requests(self.data / "state")
                if m.get("params", {}).get("name") == name]

    def test_a_change_runs_unasked_as_its_preview_saw_it_and_only_once(self):
        self.assertFalse(self.host.ask_before_changes(), "off unless the producer turns it on")
        self.assertIsNone(self.hook("kumi_change", change()))
        [preview] = self.sent("change")
        self.assertEqual((preview["preview"], preview["expected_epoch"]), (True, "1"), "previewed at the epoch read")
        done = support.handle(self.host, "kumi_change", change(), session_id="s1", task_id="t1")
        self.assertEqual(done["state"], "applied", done)
        write = self.sent("change")[-1]
        self.assertEqual((write.get("preview"), write["expected_epoch"], write["_host"]["tool_call_id"]),
                         (None, "1", "c1"), "the write carries the epoch and names the call the hook previewed")
        self.assertEqual(len(support.call(self.host, "kumi_history", {}, session_id="s1")["items"]), 1)
        again = support.handle(self.host, "kumi_change", change(), session_id="s1", task_id="t1")
        self.assertEqual(again["error"]["code"], "not_approved", "the hook's record is used once")
        other = support.handle(self.host, "kumi_change", change(volume("t1", 6)), session_id="s1", task_id="t1")
        self.assertEqual(other["error"]["code"], "not_approved", "arguments the hook never saw don't run")

    def test_an_undo_a_listen_playback_and_takes_run_unasked(self):
        support.call(self.host, "kumi_change", change(), session_id="s1")
        self.assertIsNone(self.hook("kumi_undo", {}, "u-1"))
        self.assertEqual(support.handle(self.host, "kumi_undo", {}, session_id="s1", task_id="t1")["state"], "undone")
        self.assertIsNone(self.hook("kumi_listen", test_listen.track(), "l-1"))
        self.assertEqual(test_listen.played(self.data), [], "the hook only previews")
        heard = support.handle(self.host, "kumi_listen", test_listen.track(), session_id="s1", task_id="t1")
        self.assertNotIn("error", heard)
        self.assertEqual(len(test_listen.played(self.data)), 1, "played once, within the preview's bounds")
        play = {"action": "play", "clip_ref": "t1/s0", "expected_project_key": PROJECT["expected_project_key"]}
        self.assertIsNone(self.hook("kumi_transport", play, "p-1"))
        self.assertEqual(support.handle(self.host, "kumi_transport", play, session_id="s1", task_id="t1")["state"],
                         "playing")
        takes = {"action": "make", "source": {"clipRef": "t2/c1"}, "count": 2, **PROJECT}
        self.assertIsNone(self.hook("kumi_variations", takes, "v-1"))
        made = support.handle(self.host, "kumi_variations", takes, session_id="s1", task_id="t1")
        self.assertEqual((made["state"], len(made["history_ids"])), ("applied", 2), "the takes the preview drew")

    def test_a_preset_is_placed_unasked_as_its_preview_named_it_even_where_its_approval_wouldnt_fit(self):
        self.use(None, "--preset-settings", "12")  # the terminal's panel can't show twelve settings
        build = {"plugin": "serum-2", "name": "Night Bass", "edits": []}
        self.assertEqual(support.call(self.host, "kumi_preset_build", build, session_id="s1")["state"], "built")
        self.assertIsNone(self.hook("kumi_preset", {"artifact_id": "a-preset-1"}, "p-1"))
        placed = support.handle(self.host, "kumi_preset", {"artifact_id": "a-preset-1"}, session_id="s1", task_id="t1")
        [place] = [args for args in self.sent("preset") if args.get("action") == "place"]
        self.assertEqual(place["file"], placed["file"], "the file the preview named, bound")
        self.assertRegex(place["digest"], "^[0-9a-f]{64}$")
        self.ctx.settings["ask_before_changes"] = True
        self.assertEqual(self.hook("kumi_preset", {"artifact_id": "a-preset-1"}, "p-2"),
                         policy.block(policy.PRESET_TOO_LONG), "asked, it must fit, as before")

    def test_a_device_is_built_unasked_in_a_clean_chat_and_asked_once_the_chat_read_outside_content(self):
        policy.chat_turn(self.host, session_id="s1", is_first_turn=True)
        spec = {"spec": test_device.SPEC}
        self.assertIsNone(self.hook("kumi_device", spec, "d-1"))
        built = support.handle(self.host, "kumi_device", spec, session_id="s1", task_id="t1")
        self.assertEqual(built["file"], test_device.FILE)
        policy.post_tool_call(self.host, tool_name="web_extract", tool_call_id="x1", args={}, result="{}",
                              status="ok", session_id="s1")
        self.ctx.surface = "desktop"
        with mock.patch.object(tools, "approvals_bypassed", lambda host, **kw: False):
            asked = self.hook("kumi_device", {"spec": {**test_device.SPEC, "name": "Warm Fold 2"}}, "d-2")
        self.assertEqual(asked["action"], "approve", "asked as a script is (CC-19)")

    def test_setup_scripts_folders_and_mirelo_are_still_asked(self):
        policy.chat_turn(self.host, session_id="s1", is_first_turn=True)
        with mock.patch.object(tools, "approvals_bypassed", lambda host, **kw: False):
            self.assertEqual(self.hook("kumi_script", test_scripts.script(), "s-1")["action"], "approve")
        folder = self.hook("kumi_library", {"action": "add_folder", "folder": support.DRUMS}, "f-1")
        self.assertEqual(folder["action"], "approve")

        self.enterContext(mock.patch.object(support.module("setup"), "_ffmpeg_search", return_value=[]))
        host, ctx, _ = support.host(self, daw_component=False, ask_before_changes=None)
        ctx.surface = "desktop"
        support.call(host, "kumi_setup", {"action": "plan"}, session_id="s1")
        asked = policy.pre_tool_call(host, "kumi_setup", {"action": "install"}, session_id="s1", tool_call_id="c1")
        self.assertEqual((asked["action"], asked["rule_key"][:11]), ("approve", "kumi:setup:"))

        mirelo, _, _ = support.host(self, dict(test_generate.ON), mirelo_key=test_generate.KEY,
                                    ask_before_changes=None)
        support.call(mirelo, "kumi_connect", {}, session_id="s1")
        quoted = support.call(mirelo, "kumi_generate", test_generate.SFX, session_id="s1")
        paid = policy.pre_tool_call(mirelo, tool_name="kumi_generate", session_id="s1", tool_call_id="g-1",
                                    args={"action": "create", "quote_id": quoted["quote_id"]})
        self.assertEqual((paid["action"], paid["rule_key"]), ("approve", f"kumi:paid:{quoted['quote_id']}"))
        upload = policy.pre_tool_call(mirelo, tool_name="kumi_generate", args=test_generate.MIDI, session_id="s1",
                                      tool_call_id="g-2")
        self.assertEqual(upload["action"], "approve", "a file sent to Mirelo is asked")

    def test_refusals_unattended_and_forked_turns_still_refuse(self):
        stale = self.hook("kumi_change", change(expected_epoch="0"))
        self.assertEqual(stale["action"], "block")
        self.assertIn("changed since it was read", stale["message"])
        refused = support.handle(self.host, "kumi_change", change(expected_epoch="0"), session_id="s1", task_id="t1")
        self.assertEqual(refused["error"]["code"], "not_approved", "nothing was recorded for a refused plan")
        self.assertEqual(self.hook("kumi_variations", {"action": "drop"})["action"], "block")
        track = test_listen.track()
        with mock.patch.dict(os.environ, {"HERMES_SINGLE_QUERY_SESSION": "1"}):
            for tool, args in (("kumi_change", change()), ("kumi_undo", {}), ("kumi_listen", track),
                               ("kumi_preset", {"artifact_id": "a-preset-1"}),
                               ("kumi_device", {"spec": test_device.SPEC})):
                with self.subTest(tool=tool):
                    self.assertEqual(self.hook(tool, args), policy.block(policy.UNATTENDED))
        self.ctx.surface = "webhook"
        self.assertEqual(self.hook("kumi_transport", {"action": "play", "clip_ref": "t1/s0",
                                                      "expected_project_key": PROJECT["expected_project_key"]}),
                         policy.block(policy.UNATTENDED), "refused after its preview, as before")
        self.ctx.surface = "tui"
        policy.turn_started(self.host, session_id="s1", turn_id="u1")
        for tool, args in (("kumi_change", change()), ("kumi_undo", {}), ("kumi_listen", track)):
            with self.subTest(tool=tool, turn="forked"):
                self.assertEqual(self.hook(tool, args, turn_id="s1:t2:9f3a1c2e"), policy.block(policy.FORKED))
        self.assertEqual(support.call(self.host, "kumi_history", {}, session_id="s1")["items"], [])

    def test_with_the_setting_on_each_change_is_asked_as_before(self):
        self.ctx.settings["ask_before_changes"] = True
        decision = self.hook("kumi_change", change())
        self.assertEqual((decision["action"], decision["rule_key"]), ("approve", "kumi:write:mixer:s1"))
        self.assertTrue(decision["message"].endswith(policy.CHAT_ONLY))


if __name__ == "__main__":
    unittest.main()
