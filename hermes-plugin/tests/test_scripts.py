"""CC-19: scripts in the DAW, trusted by the chat's provenance (r6's list): what makes a chat tainted and what doesn't,
the record surviving a restart, the rule keys, YOLO and approvals off, nobody there, messaging chats, subagents, the
runtime's refusals before anyone is asked, the approval's words on each surface, the saved code, and Desktop's
events."""
from __future__ import annotations

import os
from pathlib import Path
import stat
import unittest
from unittest import mock

import support

policy = support.module("policy")
scripts = support.module("scripts")
tools = support.module("tools")

PROJECT = "live:night-drive"
CODE = "import Live\nsong = Live.Application.get_application().get_document()\nresult = song.tempo\n"


def script(code: str = CODE, purpose: str = "Read the tempo", language: str = "python") -> dict:
    return {"language": language, "purpose": purpose, "code": code, "expected_project_key": PROJECT}


class Scripts(unittest.TestCase):
    def setUp(self):
        self.host, self.ctx, self.data = support.host(self)
        self.events: list[tuple[str, dict]] = []
        self.enterContext(mock.patch.object(tools, "broadcast", lambda event, payload: self.events.append(
            (event, payload))))
        self.bypassed = False
        self.enterContext(mock.patch.object(tools, "approvals_bypassed", lambda host, **kw: self.bypassed))
        support.call(self.host, "kumi_connect", {}, session_id="s1")

    def start(self, session: str = "s1") -> None:
        policy.chat_turn(self.host, session_id=session, is_first_turn=True)

    def hook(self, args: dict, session: str = "s1") -> dict:
        return policy.pre_tool_call(self.host, tool_name="kumi_script", args=args, session_id=session,
                                    tool_call_id="c1", task_id="t1", turn_id="u1", api_request_id="r1",
                                    middleware_trace=[])

    def finished(self, tool: str, args: dict | None = None, session: str = "s1") -> None:
        policy.post_tool_call(self.host, tool_name=tool, tool_call_id="x1", args=args or {}, result="{}",
                              status="ok", session_id=session)

    def test_a_clean_chat_asks_for_this_chat_and_after_web_content_asks_by_the_code_and_says_why(self):
        self.start()
        clean = self.hook(script())
        self.assertEqual((clean["action"], clean["rule_key"]), ("approve", "kumi:script:s1"))
        self.assertIn(scripts.WARNING, clean["message"])
        self.assertIn('Hermes says: "Read the tempo"', clean["message"])
        self.finished("web_extract", {"urls": ["https://example.com/tutorial"]})
        tainted = self.hook(script())
        digest = scripts.sha256(CODE)
        self.assertEqual(tainted["rule_key"], f"kumi:script:{digest}",
                         "an earlier 'Allow this session' (kumi:script:s1) no longer matches")
        self.assertIn("This chat read outside content (web_extract), so each script is asked.", tainted["message"])

    def test_outside_content_taints_a_chat_and_local_tools_leave_it_clean(self):
        outside = [("web_search", {"query": "serum patch"}), ("browser_exec", {"code": "page.goto(x)"}),
                   ("delegate_task", {"goal": "find a tutorial"}), ("mcp_github_search", {"q": "lua"}),
                   ("elevenlabs_music_compose", {"prompt": "pad"}), ("memory", {"action": "read"}),
                   ("session_search", {"query": "last week"}), ("vision_analyze", {"image_url": "https://x/y.png"}),
                   ("terminal", {"command": "ls -la ~/Music"}), ("execute_code", {"code": "print(2 + 2)"}),
                   ("a_tool_hermes_adds_next_year", {})]
        local = [("read_file", {"path": "/Users/me/notes.txt"}), ("kumi_observe", {"scope": "focus"}),
                 ("skill_view", {"name": "kumi:daw"}), ("clarify", {"question": "Which pad?"}),
                 ("todo_list", {"action": "add"})]
        for index, (tool, args) in enumerate(outside):
            with self.subTest(tool=tool, args=args):
                session = f"out-{index}"
                self.start(session)
                self.finished(tool, args, session)
                self.assertEqual(self.host.trust().state(session)[0], scripts.TAINTED)
        for index, (tool, args) in enumerate(local):
            with self.subTest(tool=tool, args=args):
                session = f"in-{index}"
                self.start(session)
                self.finished(tool, args, session)
                self.assertEqual(self.host.trust().state(session), (scripts.CLEAN, ""))

    def test_a_chat_first_met_after_its_first_turn_is_tainted_and_a_restart_keeps_every_chats_record(self):
        self.assertEqual(self.hook(script(), session="resumed")["rule_key"], f"kumi:script:{scripts.sha256(CODE)}")
        self.start("clean")
        self.start("read-the-web")
        self.finished("web_extract", {}, "read-the-web")
        again = scripts.Trust(self.data / scripts.TRUST_FILE)  # as a new process reads it
        self.assertEqual(again.state("clean"), (scripts.CLEAN, ""))
        self.assertEqual(again.state("read-the-web"), (scripts.TAINTED, "web_extract"))
        self.assertEqual(again.state("resumed"), (scripts.TAINTED, scripts.NO_RECORD))
        again.first_turn("read-the-web", surface="tui")
        self.assertEqual(again.state("read-the-web")[0], scripts.TAINTED, "a first turn never cleans a chat")

    def test_with_approvals_off_a_clean_chats_script_goes_to_hermes_and_a_tainted_chats_is_refused(self):
        self.start()
        self.bypassed = True
        self.assertEqual(self.hook(script())["action"], "approve", "Hermes runs it unasked, as the owner uses Kumi")
        self.finished("web_extract")
        refused = self.hook(script())
        self.assertEqual(refused["action"], "block")
        self.assertIn("nobody would review it", refused["message"])
        self.bypassed = None  # Kumi can't tell (Hermes's plugin host)
        self.assertIn("can't tell whether Hermes's approvals are on", self.hook(script())["message"])

    def test_nobody_there_and_messaging_chats_without_the_setting_are_refused(self):
        self.start()
        self.ctx.cron_execution = object()
        self.assertEqual(self.hook(script())["message"], tools.SCHEDULED)
        self.ctx.cron_execution = None
        self.ctx.surface = "webhook"
        self.assertEqual(self.hook(script())["message"], policy.UNATTENDED)
        for surface in ("telegram", "discord", "unknown"):
            with self.subTest(surface=surface):
                self.ctx.surface = surface
                self.assertEqual(self.hook(script())["message"], policy.SCRIPT_MESSAGING)

    def test_an_opted_in_messaging_chat_asks_every_script_by_its_code_within_the_apps_length(self):
        self.ctx.settings["scripts_in_messaging"] = True
        for surface, budget in (("telegram", 500), ("discord", 300), ("slack", 500)):
            with self.subTest(surface=surface):
                self.ctx.surface = surface
                self.start(f"chat-{surface}")
                asked = self.hook(script(), session=f"chat-{surface}")
                self.assertEqual(asked["rule_key"], f"kumi:script:{scripts.sha256(CODE)}")
                self.assertIn(scripts.WARNING, asked["message"])
                self.assertTrue(policy.shows_whole(asked["message"], surface), len(asked["message"]))
                self.assertLessEqual(len(asked["message"]), budget)

    def test_a_delegated_subagents_chat_never_runs_a_script(self):
        policy.subagent_started(self.host, parent_session_id="s1", child_session_id="child")
        policy.chat_turn(self.host, session_id="child", is_first_turn=True)
        self.assertEqual(self.hook(script(), session="child"), {"action": "block", "message": policy.SCRIPT_SUBAGENT})

    def test_code_the_runtime_refuses_is_never_put_to_the_producer(self):
        self.start()
        refused = {
            "import os\nos.system('rm -rf ~')\n": "Kumi refuses scripts that use `os.system`",
            "x = 1\ny = 'a\u202eb'\n": "U+202E on line 2",
            "x = 1\n" * 401: "over 400 lines or 32 KB",
        }
        for code, words in refused.items():
            with self.subTest(words=words):
                decision = self.hook(script(code))
                self.assertEqual(decision["action"], "block")
                self.assertIn(words, decision["message"])
        self.assertIn("this one is Lua", self.hook(script("print(1)", language="lua"))["message"])
        self.assertEqual(self.hook(script("x = 1\n" * 400))["action"], "approve", "400 lines is the cap, not over it")
        for code in refused:
            self.assertFalse((self.data / "state" / "scripts" / f"{scripts.sha256(code)}.py").exists(),
                             "refused code is never kept")

    def test_always_in_a_tainted_chat_rematches_only_the_same_code(self):
        self.start()
        self.finished("web_search")
        first, again = self.hook(script()), self.hook(script(purpose="Read it again"))
        other = self.hook(script(CODE + "result = song.is_playing\n"))
        self.assertEqual(first["rule_key"], again["rule_key"], "the key is the code's, whatever the purpose says")
        self.assertNotEqual(first["rule_key"], other["rule_key"])

    def test_the_approval_fits_the_terminals_panel_and_desktop_shows_the_code_and_where_it_is(self):
        self.start()
        code = "".join(f"line_{number} = {number}\n" for number in range(1, 31))
        terminal = self.hook(script(code))
        self.assertEqual(terminal["action"], "approve")
        self.assertTrue(policy.shows_whole(terminal["message"], "tui"))
        self.assertIn(f"SHA-256 {scripts.sha256(code)[:16]}", terminal["message"])
        self.ctx.surface = "desktop"
        desktop = self.hook(script(code))["message"]
        self.assertIn("  line_12 = 12", desktop)
        self.assertIn("  … 18 more", desktop)
        saved = self.data / "state" / "scripts" / f"{scripts.sha256(code)}.py"
        self.assertIn(f"Full code: {saved}", desktop)

    def test_the_code_is_kept_once_and_runs_only_as_approved_into_a_history_row_kumi_never_undoes(self):
        self.start()
        ran = support.call(self.host, "kumi_script", script(), session_id="s1")
        self.assertEqual(ran["state"], "ran")
        saved = self.data / "state" / "scripts" / f"{scripts.sha256(CODE)}.py"
        self.assertEqual(saved.read_text(encoding="utf-8"), CODE)
        if os.name == "posix":
            self.assertEqual(stat.S_IMODE(saved.stat().st_mode), 0o600)
        row = next(row for row in support.call(self.host, "kumi_history", {}, session_id="s1")["items"]
                   if row["id"] == ran["history_id"])
        self.assertEqual((row["title"], row["family"], row["undo_available"]), ("Script: Read the tempo", "script",
                                                                               False))
        undo = policy.pre_tool_call(self.host, tool_name="kumi_undo", args={"history_id": ran["history_id"]},
                                    session_id="s1", tool_call_id="c2", task_id="t1", turn_id="u1",
                                    api_request_id="r1", middleware_trace=[])
        self.assertEqual(undo["message"], "Kumi didn't undo anything: Kumi can't undo a script: Live's own Undo may, "
                                          "if its step is still the latest there.")
        self.assertEqual(support.handle(self.host, "kumi_script", script(), session_id="s1")["error"]["code"],
                         "not_approved", "a call no approval saw never runs")

    def test_a_code_file_that_isnt_the_code_its_named_for_is_never_used(self):
        folder = self.data / "state" / "scripts"
        folder.mkdir(parents=True)
        (folder / f"{scripts.sha256(CODE)}.py").write_text("print('something else')\n", encoding="utf-8")
        self.start()
        self.assertEqual(self.hook(script())["message"],
                         "Kumi didn't run the script: it couldn't keep a copy of the code to show you.")

    def test_desktop_is_told_when_a_script_is_asked_and_how_it_ended(self):
        self.start()
        support.call(self.host, "kumi_script", script(), session_id="s1")
        pending = next(payload for event, payload in self.events if event == "script.pending")
        self.assertEqual({key: pending[key] for key in ("language", "daw", "purpose", "warning", "sha256", "lines")},
                         {"language": "python", "daw": "Live", "purpose": "Read the tempo",
                          "warning": scripts.WARNING, "sha256": scripts.sha256(CODE), "lines": 3})
        self.assertNotIn("code", pending, "the code never travels in an event")
        policy.post_tool_call(self.host, tool_name="kumi_script", tool_call_id="c9", args=script(),
                              result='{"state": "ran", "history_id": "h1"}', status="ok", session_id="s1")
        policy.post_approval_response(self.host, tool_call_id="c10", choice="deny")
        policy.post_tool_call(self.host, tool_name="kumi_script", tool_call_id="c10", args=script(),
                              result="Denied by the user.", status="error", session_id="s1")
        settled = [payload["outcome"] for event, payload in self.events if event == "script.settled"]
        self.assertEqual(settled[-2:], ["ran", "denied"])


if __name__ == "__main__":
    unittest.main()
