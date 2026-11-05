"""policy.py (C6, CC-3, CC-5): what the producer is asked before a Kumi call runs, in the runtime's own words; what is
blocked before anyone is asked; and that the hook never holds up Hermes's other tools."""
from __future__ import annotations

import contextvars
import os
import unittest
from unittest import mock

import support

policy = support.module("policy")
runtime = support.module("runtime")
tools = support.module("tools")

PROJECT = {"expected_project_key": "live:night-drive", "expected_epoch": "1"}


def volume(ref: str, db: float) -> dict:
    return {"tool": "set_track_volume", "input": {"track_ref": ref, "volume_db": db}}


def change(*steps: dict, **fields) -> dict:
    return {"title": "Louder drums", "steps": list(steps or [volume("t1", -4)]), **PROJECT, **fields}


class Approvals(unittest.TestCase):
    def setUp(self):
        self.host, self.ctx, self.data = support.host(self)
        support.call(self.host, "kumi_connect", {}, session_id="s1")

    def hook(self, tool: str, args: dict) -> dict | None:
        return policy.pre_tool_call(self.host, tool_name=tool, args=args, session_id="s1", tool_call_id="c1",
                                    task_id="t1", turn_id="u1", api_request_id="r1", middleware_trace=[])

    def test_reads_connect_and_other_plugins_tools_need_no_approval(self):
        for tool in ("kumi_status", "kumi_connect", "kumi_observe", "kumi_read", "kumi_history", "kumi_sounds",
                     "kumi_job", "terminal", "elevenlabs_sound_effect"):
            self.assertIsNone(self.hook(tool, {"scope": "focus"}), tool)
        self.assertIsNone(self.hook("kumi_library", {"action": "status"}))
        self.assertIsNone(self.hook("kumi_listen", {"source": {"sound_id": "s-r1-1", "map_revision": "r1"},
                                                    "compare_to": {"file": "/Samples/a.wav"}}))

    def test_a_scheduled_job_reads_only_kumis_published_status_and_history_and_never_reaches_the_runtime(self):
        track = {"source": {"track": {"ref": "t1"}, "from_beat": 0, "beats": 8}, **PROJECT}
        calls = {"kumi_connect": {}, "kumi_setup": {}, "kumi_change": change(), "kumi_undo": {"history_id": "h-1"},
                 "kumi_listen": track, "kumi_library": {"action": "add_folder", "folder": support.DRUMS},
                 "kumi_observe": {"scope": "focus"}, "kumi_read": {"action": "data"}, "kumi_sounds": {"query": "a"},
                 "kumi_job": {"job_id": "j-1"}}
        before = support.requests(self.data / "state")
        self.ctx.cron_execution = object()  # Hermes's ctx.current_cron_execution(): a cron run is executing now
        for tool, args in calls.items():  # with the producer's chat's runtime running, whatever Hermes would allow
            decision = self.hook(tool, args)
            self.assertEqual((decision["action"], decision["message"]), ("block", tools.SCHEDULED), tool)
        self.assertIsNone(self.hook("kumi_status", {}))
        self.assertIsNone(self.hook("kumi_history", {}))
        published = support.call(self.host, "kumi_status", {}, session_id="cron-1")
        self.assertEqual((published["connected"], published["project"]["name"]), (True, "Night Drive"))
        self.assertEqual(support.requests(self.data / "state"), before, "nothing reached the runtime")
        self.ctx.cron_execution = None
        self.assertEqual(self.hook("kumi_change", change())["action"], "approve", "a chat's change is asked as before")

    def test_where_nobody_can_answer_an_approval_nothing_that_needs_one_runs(self):
        track = {"source": {"track": {"ref": "t1"}, "from_beat": 0, "beats": 8}, **PROJECT}
        with mock.patch.dict(os.environ, {"HERMES_SINGLE_QUERY_SESSION": "1"}):  # what `hermes -q` exports
            for tool, args in (("kumi_change", change()), ("kumi_undo", {"history_id": "h-1"}), ("kumi_listen", track),
                               ("kumi_library", {"action": "add_folder", "folder": support.DRUMS})):
                decision = self.hook(tool, args)
                self.assertEqual((decision["action"], decision["message"]), ("block", policy.UNATTENDED), tool)
            self.assertIsNone(self.hook("kumi_connect", {}), "a one-shot query still connects")
            self.assertIsNone(self.hook("kumi_observe", {"scope": "focus"}), "and reads")

    def test_an_api_call_or_a_webhook_never_runs_what_needs_an_approval_and_desktop_still_asks(self):
        for surface in ("api_server", "webhook", "msgraph_webhook"):  # Hermes's ctx.session_surface()
            with self.subTest(surface=surface):
                self.ctx.surface = surface
                self.assertEqual(self.hook("kumi_change", change())["message"], policy.UNATTENDED)
                self.assertIsNone(self.hook("kumi_observe", {"scope": "focus"}), "reads still answer")
        self.ctx.surface = "desktop"
        self.assertEqual(self.hook("kumi_change", change())["action"], "approve")

    def test_when_hermes_cant_say_who_is_there_nobody_is_assumed_there(self):
        self.ctx.cron_execution = RuntimeError("the plugin host's channel is gone")
        self.assertEqual(self.hook("kumi_change", change())["message"], tools.SCHEDULED)
        self.ctx.cron_execution, self.ctx.surface = None, RuntimeError("the plugin host's channel is gone")
        self.assertEqual(self.hook("kumi_change", change())["message"], policy.UNATTENDED)

    def test_a_scheduled_job_is_refused_by_the_handler_too_when_the_hook_didnt_run(self):
        before = support.requests(self.data / "state")
        self.ctx.cron_execution = object()
        answer = support.handle(self.host, "kumi_observe", {"scope": "focus"}, session_id="cron-1")
        self.assertEqual(answer["error"]["code"], "scheduled")
        self.assertEqual(support.requests(self.data / "state"), before)

    def test_hermess_own_session_variables_tell_a_scheduled_job_and_a_webhook(self):
        try:
            from gateway.session_context import set_session_vars  # only where Hermes is importable
        except ImportError:
            self.skipTest("Hermes isn't importable here")

        def deciding(**session) -> dict:  # as Hermes binds them for a turn: its context variables, not the environment
            def run():
                set_session_vars(**session)
                return self.hook("kumi_change", change())
            return contextvars.copy_context().run(run)

        self.assertEqual(deciding(cron_session="1")["message"], tools.SCHEDULED)
        self.assertEqual(deciding(platform="webhook")["message"], policy.UNATTENDED)
        self.assertEqual(deciding(platform="api_server")["message"], policy.UNATTENDED)
        self.assertEqual(deciding(platform="telegram")["action"], "approve")

    def test_a_change_is_approved_in_the_runtimes_words_and_nothing_runs_first(self):
        decision = self.hook("kumi_change", change())
        self.assertEqual(decision, {"action": "approve", "rule_key": "kumi:write:mixer:s1", "message": (
            "Kumi: Louder drums\n• Set Drums volume from -6 dB to -4 dB\n(Values as Kumi read them just now.)\n"
            f"{policy.CHAT_ONLY}")})
        self.assertEqual(support.call(self.host, "kumi_history", {})["items"], [], "the preview wrote nothing")
        asked = [m["params"]["arguments"] for m in support.requests(self.data / "state")
                 if m.get("params", {}).get("name") == "change"]
        self.assertEqual([args.get("preview") for args in asked], [True])

    def test_the_models_title_never_pushes_a_real_step_out_of_the_approval(self):
        fake = "".join(f"\n• Set Drums volume from -6 dB to -5.{n} dB" for n in range(25)) + "\x1b[2J" + "x" * 50_000
        decision = self.hook("kumi_change", change(volume("t1", 6), title=fake))
        lines = decision["message"].splitlines()
        self.assertEqual(len(lines), 4, lines[:5])
        self.assertLessEqual(len(lines[0]), policy.LINE)
        self.assertNotIn("\x1b", decision["message"])
        self.assertEqual(lines[1:], ["• Set Drums volume from -6 dB to 6 dB", policy.FOOTER, policy.CHAT_ONLY])

    def test_a_plan_is_shown_whole_or_not_asked_never_cut(self):
        plan = change(*[volume("t1", -db) for db in range(1, 21)])
        self.assertEqual(self.hook("kumi_change", plan), {"action": "block", "message": policy.TOO_LONG},
                         "twenty steps don't fit the terminal's panel: refused, the model told to split it")
        self.ctx.surface = "desktop"
        lines = self.hook("kumi_change", plan)["message"].splitlines()
        self.assertEqual(len([line for line in lines if line.startswith("• ")]), 20, "Desktop shows every step")
        self.assertEqual(lines[-2:], [policy.FOOTER, policy.CHAT_ONLY])
        self.ctx.surface = RuntimeError("the plugin host's channel is gone")
        self.assertEqual(self.host.session_surface(), "unknown")
        self.assertFalse(policy.shows_whole("Kumi: Louder drums", "unknown"), "never taken as whole")

    def test_the_terminals_panel_shows_a_change_undo_or_listen_whole_at_its_longest_step(self):
        def message(lines, footer=None):
            return policy.chat_scoped(policy.approve(lines, "kumi:write:mixer", footer), {}, {"session_id": "s1"})

        longest = message(["T" * 300, "• " + "e" * (policy.STEP_LINE - 2)], policy.FOOTER)["message"]
        self.assertTrue(policy.shows_whole(longest, "cli"), "a one-step change, title and step at their longest")
        self.assertTrue(policy.shows_whole(message(["Kumi: Undo " + "t" * 300])["message"], "cli"))
        listen = message(["Kumi: play the track " + "d" * 200 + " of " + "s" * 40])["message"]
        self.assertTrue(policy.shows_whole(listen, "cli"), "a track listen with its names at their longest")
        two = message(["T", "• " + "e" * (policy.STEP_LINE - 2), "• " + "e" * (policy.STEP_LINE - 2)], policy.FOOTER)
        self.assertFalse(policy.shows_whole(two["message"], "cli"), "so a longer plan is refused, never cut")
        self.assertEqual(policy.approve(["T", "• " + "e" * policy.STEP_LINE], "kumi:write:mixer")["action"], "block",
                         "a step longer than the runtime ever writes is refused, never cut")

    def test_a_plans_families_make_its_rule_key_so_always_covers_only_that_kind_in_this_chat(self):
        sample = {"tool": "replace_sample", "input": {"track_ref": "t1", "sample": "Dusty Snare 04.wav"}}
        decision = self.hook("kumi_change", change(sample, volume("t2", -10)))
        self.assertEqual(decision["rule_key"], "kumi:write:mixer+sample:s1")
        self.assertIn("• Replace the sample in the Simpler on Drums > Drum Rack pad D1 (Snare 01.wav) with Dusty "
                      "Snare 04.wav", decision["message"])

    def test_an_always_in_one_chat_never_covers_another_chat_a_one_shot_run_or_a_compacted_chat(self):
        keys = {session: policy.pre_tool_call(self.host, tool_name="kumi_change", args=change(), session_id=session,
                                              tool_call_id=f"c-{session}")["rule_key"]
                for session in ("chat-a", "chat-b", "q-oneshot", "chat-a-compacted")}
        self.assertEqual(keys["chat-a"], "kumi:write:mixer:chat-a")
        self.assertEqual(len(set(keys.values())), 4, "an always saved in chat A matches none of the others")
        nameless = policy.pre_tool_call(self.host, tool_name="kumi_change", args=change(), tool_call_id="c-none")
        self.assertRegex(nameless["rule_key"], r"^kumi:write:mixer:plan-[0-9a-f]{64}$",
                         "without a session id, the plan's own digest, never a family alone")

    def test_a_plan_the_runtime_would_refuse_is_blocked_before_anyone_is_asked(self):
        for args, reason in ((change(expected_epoch="0"), "stale"),
                             (change({"tool": "delete_track", "input": {"track_ref": "t1"}}), "delete_track"),
                             (change(volume("t9", -4)), "no track"),
                             (change(*[volume("t1", -4)] * 21), "doesn't take these steps")):
            with self.subTest(reason=reason):
                decision = self.hook("kumi_change", args)
                self.assertEqual(decision["action"], "block")
                self.assertIn(reason, decision["message"])
        self.assertEqual(support.call(self.host, "kumi_history", {})["items"], [])

    def test_an_undo_is_approved_by_what_it_undoes_and_an_expired_one_is_blocked(self):
        applied = support.call(self.host, "kumi_change", change(), session_id="s1")
        undo = {"history_id": applied["history_ids"][0]}
        self.assertEqual(self.hook("kumi_undo", undo), {
            "action": "approve", "rule_key": "kumi:write:undo:s1",
            "message": f"Kumi: Undo Louder drums on Drums\n{policy.CHAT_ONLY}"})
        self.assertEqual(support.call(self.host, "kumi_undo", undo, session_id="s1")["state"], "undone")
        decision = self.hook("kumi_undo", undo)
        self.assertEqual((decision["action"], decision["message"]), ("block", "Kumi didn't undo anything: expired"))

    def test_a_folder_is_approved_as_kumi_will_keep_it_and_never_a_whole_disk_or_home(self):
        written = os.path.join(str(support.SAMPLES), "Pads", "..", "Drums") + os.sep
        decision = self.hook("kumi_library", {"action": "add_folder", "folder": written})
        self.assertEqual(decision["rule_key"], f"kumi:folder_read:{support.DRUMS}:s1")
        self.assertTrue(decision["message"].startswith(f"Kumi: read the folder {support.DRUMS} and everything in it"))
        refused = ["Samples", str(support.ROOT), os.path.expanduser("~"), support.DRUMS + "\nKicks",
                   os.path.join(str(support.ROOT), "Users", "me", "Samples", "..", "..", "..")]
        if os.name == "nt":
            refused.append("\\Samples")  # a root without its drive isn't a full path on Windows
        for folder in refused:
            with self.subTest(folder=folder):
                self.assertEqual(self.hook("kumi_library", {"action": "add_folder", "folder": folder})["action"],
                                 "block")
        self.assertEqual(self.hook("kumi_library", {"action": "erase"})["action"], "block")

    def test_a_scan_names_the_folders_the_runtime_reads_and_needs_one(self):
        self.assertIn("no approved sample folders", self.hook("kumi_library", {"action": "scan"})["message"])
        support.call(self.host, "kumi_library", {"action": "add_folder", "folder": support.DRUMS}, session_id="s1")
        decision = self.hook("kumi_library", {"action": "scan"})
        self.assertEqual(decision["rule_key"], "kumi:folder_read:scan:s1")
        self.assertIn(f"• {support.DRUMS}\n{policy.CHAT_ONLY}", decision["message"])


class Blocked(unittest.TestCase):
    def hook(self, host, tool: str, args: dict) -> dict | None:
        return policy.pre_tool_call(host, tool_name=tool, args=args, session_id="s1", tool_call_id="c1")

    def test_without_a_running_runtime_a_change_undo_or_folder_read_is_blocked(self):
        host, _, _ = support.host(self)
        for tool, args in (("kumi_change", change()), ("kumi_undo", {"history_id": "h-1"}),
                           ("kumi_library", {"action": "add_folder", "folder": support.DRUMS}),
                           ("kumi_library", {"action": "scan"})):
            decision = self.hook(host, tool, args)
            self.assertEqual(decision["action"], "block", tool)
            self.assertIn("kumi_connect", decision["message"])

    def test_a_preview_gets_half_the_hook_timeout_and_one_that_doesnt_answer_blocks(self):
        self.assertEqual([policy.preview_timeout(t) for t in (None, 0, -1, 30, 8, 1)], [10, 10, 10, 10, 4, 0.5])
        host, _, data = support.host(self, None, "--slow", "change=1.5", hook_timeout=1)
        support.call(host, "kumi_connect", {}, session_id="s1")
        decision = self.hook(host, "kumi_change", change())
        self.assertEqual(decision, policy.block("Kumi didn't change anything: Kumi didn't answer in time; nothing ran."))

    def test_a_runtime_that_dies_on_the_preview_blocks(self):
        host, _, _ = support.host(self, None, "--exit-on", "change")
        support.call(host, "kumi_connect", {}, session_id="s1")
        decision = self.hook(host, "kumi_change", change())
        self.assertEqual(decision["action"], "block")
        self.assertIn("stopped", decision["message"])

    def test_a_tool_that_isnt_kumis_never_touches_kumi(self):
        class Untouchable:
            def __getattr__(self, name):
                raise AssertionError(f"the hook used host.{name} for another plugin's tool")

        self.assertIsNone(self.hook(Untouchable(), "terminal", {"command": "ls"}))

    def test_an_unknown_kumi_tool_and_a_failing_check_block(self):
        host, _, _ = support.host(self)
        self.assertEqual(self.hook(host, "kumi_erase_everything", {})["action"], "block")

        class Broken:
            def data_dir(self):
                raise RuntimeError("boom")

        with self.assertLogs(policy.log, "ERROR"):
            decision = self.hook(Broken(), "kumi_change", {"title": "x", "steps": []})
        self.assertEqual(decision["action"], "block")
        self.assertIn("didn't run", decision["message"])

    def test_a_preview_that_doesnt_describe_every_step_blocks(self):
        class Live:
            def __init__(self, answer):
                self.answer = answer

            def call(self, tool, args, host, *, timeout):
                return self.answer

        class Host:
            def __init__(self, answer):
                self.runtimes = type("Runtimes", (), {"require": lambda _, data: Live(answer)})()

            def data_dir(self):
                return None

            def identity(self, kwargs):
                return {}

            def hook_timeout(self):
                return 30

        good = {"tool": "set_track_volume", "family": "mixer", "effect": "write", "text": "Set Drums volume"}
        for steps in ([{**good, "effect": "erase"}], [{**good, "family": ""}], [good, good], [{**good, "text": " "}]):
            with self.subTest(steps=steps):
                decision = policy._change(Host({"state": "preview", "steps": steps}), change(), {})
                self.assertEqual(decision["action"], "block")


class CallIds(unittest.TestCase):
    """CC-5: the model's tool-call id, from the hook to the handler; and a write runs only as approved."""

    def setUp(self):
        self.host, self.ctx, self.data = support.host(self)
        support.call(self.host, "kumi_connect", {}, session_id="s1", task_id="t1")

    def received(self, tool: str) -> list[dict]:
        return [m["params"]["arguments"] for m in support.requests(self.data / "state")
                if m.get("params", {}).get("name") == tool and not m["params"]["arguments"].get("preview")]

    def hook(self, tool: str, args: dict, ident: str) -> dict | None:
        return policy.pre_tool_call(self.host, tool_name=tool, args=args, session_id="s1", task_id="t1",
                                    tool_call_id=ident)

    def test_a_handler_gets_the_tool_call_id_hermes_gave_the_hook(self):
        self.hook("kumi_history", {"limit": 5}, "read-7")
        support.handle(self.host, "kumi_history", {"limit": 5}, session_id="s1", task_id="t1")
        self.assertEqual(self.received("history")[-1]["_host"]["tool_call_id"], "read-7")
        support.handle(self.host, "kumi_history", {"limit": 5}, session_id="s1", task_id="t1")
        self.assertIsNone(self.received("history")[-1]["_host"]["tool_call_id"], "taken once; never made up")

    def test_a_denied_call_then_an_identical_approved_one_runs_as_the_approved_one(self):
        self.hook("kumi_change", change(), "a")
        policy.post_approval_response(self.host, tool_call_id="a", choice="deny")
        self.hook("kumi_change", change(), "b")
        policy.post_approval_response(self.host, tool_call_id="b", choice="once")
        self.assertEqual(support.handle(self.host, "kumi_change", change(), session_id="s1", task_id="t1")["state"],
                         "applied")
        self.assertEqual([args["_host"]["tool_call_id"] for args in self.received("change")], ["b"])

    def test_a_write_runs_only_with_the_arguments_its_approval_saw(self):
        self.hook("kumi_change", change(), "c")
        modified = support.handle(self.host, "kumi_change", change(volume("t1", 6)), session_id="s1", task_id="t1")
        self.assertEqual(modified["error"]["code"], "not_approved", "another plugin's modify after the approval")
        skipped = support.handle(self.host, "kumi_undo", {"history_id": "h-1"}, session_id="s1", task_id="t1")
        self.assertEqual(skipped["error"]["code"], "not_approved", "a write that never passed the hook")
        self.assertEqual(self.received("change"), [])
        with_preview = support.handle(self.host, "kumi_change", {**change(), "preview": True}, session_id="s1",
                                      task_id="t1")
        self.assertEqual(with_preview["state"], "applied", "the hook's record matches without `preview`")

    def test_a_call_hermes_blocked_leaves_nothing_a_later_dispatch_could_run_on(self):
        self.hook("kumi_change", change(), "vetoed")  # Kumi approved; another plugin's hook, or no human, blocked it
        policy.post_tool_call(self.host, tool_name="kumi_change", tool_call_id="vetoed", status="blocked")
        self.assertEqual(support.handle(self.host, "kumi_change", change(), session_id="s1", task_id="t1")
                         ["error"]["code"], "not_approved")
        self.hook("kumi_change", change(), "next")
        policy.post_approval_response(self.host, tool_call_id="next", choice="once")
        self.assertEqual(support.handle(self.host, "kumi_change", change(), session_id="s1", task_id="t1")["state"],
                         "applied")
        self.assertEqual([args["_host"]["tool_call_id"] for args in self.received("change")], ["next"])

    def test_hermess_coercion_of_the_arguments_keeps_their_record(self):
        self.hook("kumi_history", {"limit": "5"}, "coerced")  # the model wrote "5"; Hermes's handler gets 5
        support.handle(self.host, "kumi_history", {"limit": 5}, session_id="s1", task_id="t1")
        self.assertEqual(self.received("history")[-1]["_host"]["tool_call_id"], "coerced")


class ThroughHermes(unittest.TestCase):
    """Hermes's own gate (v0.22.7) with Kumi's hook: a denied or unanswered approval sends no change."""

    def setUp(self):
        try:
            from hermes_cli import plugins as hermes_plugins  # only where Hermes is importable (CI's Hermes job)
        except ImportError:
            self.skipTest("Hermes isn't importable here")
        self.resolve = hermes_plugins._resolve_block_from_details
        self.directive = hermes_plugins._PreToolCallDirective
        self.host, _, self.data = support.host(self)
        support.call(self.host, "kumi_connect", {}, session_id="s1")

    def test_no_human_means_the_change_never_reaches_the_runtime(self):
        decision = policy.pre_tool_call(self.host, tool_name="kumi_change", args=change(), session_id="s1",
                                        tool_call_id="h1")
        details = self.directive(action="approve", message=decision["message"], rule_key=decision["rule_key"])
        blocked = self.resolve(details, "kumi_change", tool_call_id="h1", session_id="s1")
        self.assertIsNotNone(blocked, "no one is here to approve, so Hermes blocks")
        policy.post_tool_call(self.host, tool_name="kumi_change", tool_call_id="h1", status="blocked")  # as Hermes does
        late = support.handle(self.host, "kumi_change", change(), session_id="s1")
        self.assertEqual(late["error"]["code"], "not_approved", "a later dispatch without the hook can't run it")
        sent = [m for m in support.requests(self.data / "state")
                if m.get("params", {}).get("name") == "change" and not m["params"]["arguments"].get("preview")]
        self.assertEqual(sent, [])


if __name__ == "__main__":
    unittest.main()
