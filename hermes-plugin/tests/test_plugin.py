"""The plugin as Hermes sees it: what register() registers (and that it does nothing else), and its tools through
their handlers against the fixture runtime: status and connect, a change with HISTORY and undo, the library, sounds."""
from __future__ import annotations

import json
import os
from pathlib import Path
import re
import tempfile
import threading
import time
from types import SimpleNamespace
import unittest
from unittest import mock

import support

kumi = support.plugin()
runtime = support.module("runtime")
tools = support.module("tools")
policy = support.module("policy")

PROJECT = {"expected_project_key": "live:night-drive", "expected_epoch": "1"}
LOUDER = {"title": "Louder drums", "steps": [{"tool": "set_track_volume", "input": {"track_ref": "t1", "volume_db": -4}}],
          **PROJECT}


def declared(field: str) -> set[str]:
    """A `provides_*` list of plugin.yaml (written in flow style: `[a, b]`)."""
    text = (support.PLUGIN_DIR / "plugin.yaml").read_text(encoding="utf-8")
    found = re.search(rf"^{field}:\s*\[([^\]]*)\]", text, re.MULTILINE | re.DOTALL)
    return {name.strip() for name in found[1].split(",") if name.strip()} if found else set()


class Register(unittest.TestCase):
    def test_it_registers_what_the_manifest_declares_and_does_nothing_else(self):
        ctx = support.Context()
        kumi.register(ctx)  # Context raises on anything but registering and settings
        self.assertEqual(set(ctx.tools), declared("provides_tools"))
        self.assertEqual(set(ctx.hooks), declared("provides_hooks"))
        self.assertEqual(set(ctx.commands), {"kumi"})
        self.assertEqual(set(ctx.cli), {"kumi"})
        self.assertEqual(set(ctx.references), {"kumi"})
        self.assertEqual(set(ctx.skills), {"daw", "sounds", "listening"})
        self.assertIn("kumi:daw", ctx.prompt["kumi"])

    def test_only_status_setup_and_connect_are_shown_before_kumi_is_set_up(self):
        host, _, data = support.host(self, installed=False)
        shown = tools.installed_check(host)
        self.assertFalse(shown())
        support.install(data)
        self.assertTrue(shown())
        self.assertEqual(tools.ALWAYS_SHOWN, {"kumi_status", "kumi_setup", "kumi_connect"})


class Tools(unittest.TestCase):
    def setUp(self):
        self.host, self.ctx, self.data = support.host(self)

    def received(self, tool: str) -> list[dict]:
        """The runtime's calls of `tool`, the approval hook's previews left out."""
        return [m["params"]["arguments"] for m in support.requests(self.data / "state")
                if m.get("params", {}).get("name") == tool and not m["params"]["arguments"].get("preview")]

    def test_status_before_setup_says_so_and_starts_nothing(self):
        host, _, data = support.host(self, installed=False)
        status = support.call(host, "kumi_status", {}, session_id="s1")
        self.assertEqual((status["setup"]["state"], status["setup"]["runtime"]), ("needs_runtime", {"installed": False}))
        self.assertEqual(status["message"], status["setup"]["message"])
        self.assertIn("github.com/user1303836/kumi", status["message"])
        self.assertEqual((status["connected"], status["daw"], status["runtime"]), (False, "ableton", None))
        self.assertEqual(support.call(host, "kumi_observe", {"scope": "focus"})["error"]["code"], "setup_needed")
        support.install(data)
        status = support.call(host, "kumi_status", {}, session_id="s1")
        self.assertIn("kumi_connect starts it", status["message"])
        self.assertIsNone(host.runtimes.get(data), "status never starts the runtime")
        self.assertEqual(support.call(host, "kumi_observe", {"scope": "focus"})["error"]["code"], "not_connected")

    def test_connect_starts_this_chats_runtime_and_status_then_reads_it(self):
        connected = support.call(self.host, "kumi_connect", {}, session_id="s1", task_id="t1", user_task="connect")
        self.assertEqual((connected["connected"], connected["owner"]["session_id"]), (True, "s1"))
        self.assertEqual(connected["project"], {"name": "Night Drive", "key": "live:night-drive", "epoch": "1"})
        status = support.call(self.host, "kumi_status", {}, session_id="s2")
        self.assertEqual((status["connected"], status["runtime"]["protocol"]), (True, "kumi-serve/1"))
        host = self.received("connect")[0]["_host"]
        self.assertEqual({key: host[key] for key in ("profile_id", "session_id", "task_id")},
                         {"profile_id": "default", "session_id": "s1", "task_id": "t1"})
        self.assertRegex(host["tool_call_id"], r"^call-\d+$", "the id Hermes gave the hook (CC-5)")
        self.host.on_session_finalize(session_id="s1", platform="cli")
        self.assertIsNone(self.host.runtimes.get(self.data), "the chat that started it ended")

    def test_connect_to_another_daw_than_the_runtimes_is_refused(self):
        support.call(self.host, "kumi_connect", {"daw": "ableton"}, session_id="s1")
        refused = support.call(self.host, "kumi_connect", {"daw": "reaper"}, session_id="s1")
        self.assertEqual(refused["error"]["code"], "other_daw")
        self.assertEqual(support.call(self.host, "kumi_connect", {"daw": "logic"})["error"]["code"], "invalid")

    def test_a_change_its_history_and_its_undo(self):
        support.call(self.host, "kumi_connect", {}, session_id="s1")
        applied = support.call(self.host, "kumi_change", {**LOUDER, "preview": True}, session_id="s1")
        self.assertEqual((applied["state"], applied["undo_available"]), ("applied", True))
        self.assertNotIn("preview", self.received("change")[0], "a preview is the approval hook's, never the model's")
        [row] = support.call(self.host, "kumi_history", {"limit": 5})["items"]
        self.assertEqual((row["id"], row["title"], row["family"]), (applied["history_ids"][0], "Louder drums", "mixer"))
        self.assertLessEqual(set(row), set(tools.HISTORY_FIELDS), "a live answer says C1's fields only too")
        undone = support.call(self.host, "kumi_undo", {"history_id": row["id"]}, session_id="s1")
        self.assertEqual(undone["state"], "undone")
        self.assertEqual(support.call(self.host, "kumi_read", {"action": "data"})["rows"][0]["volume_db"], -6.0)

    def test_without_a_runtime_here_history_is_as_last_published_c1s_fields_and_nothing_undoable(self):
        self.assertEqual(support.call(self.host, "kumi_history", {}),
                         {"items": [], "from": "last_published", "note": "Kumi hasn't run in this profile yet."})
        support.call(self.host, "kumi_connect", {}, session_id="s1")
        applied = support.call(self.host, "kumi_change", LOUDER, session_id="s1")
        elsewhere = tools.Host(self.ctx, runtime.Runtimes(), data_dir=lambda: self.data, pinned=lambda: support.VERSION)
        another = support.call(elsewhere, "kumi_history", {"limit": 5})  # another Hermes process: its own runtimes
        [row] = another["items"]
        self.assertEqual(row["id"], applied["history_ids"][0])
        self.assertLessEqual(set(row), set(tools.HISTORY_FIELDS), "C1's fields only, never CC-5's (12a)")
        self.assertEqual((row["undo_available"], another["stale"]), (False, False), "running elsewhere (12b)")
        self.assertIn("another Hermes process", another["note"])
        self.host.runtimes.stop_all()
        stopped = support.call(self.host, "kumi_history", {"limit": 5})
        self.assertIsNone(self.host.runtimes.get(self.data), "reading HISTORY starts nothing")
        self.assertEqual((stopped["items"][0]["id"], stopped["running"]), (applied["history_ids"][0], False))
        self.assertNotIn("unexpectedly", stopped["note"], "a clean stop isn't a crash")
        self.assertIsInstance(stopped["as_of"], int)

    def test_published_history_shows_whatever_setup_says_and_says_when_it_cant_be_read(self):
        support.call(self.host, "kumi_connect", {}, session_id="s1")
        applied = support.call(self.host, "kumi_change", LOUDER, session_id="s1")
        self.host.runtimes.stop_all()
        repinned = tools.Host(self.ctx, runtime.Runtimes(), data_dir=lambda: self.data, pinned=lambda: "9.9.9")
        self.assertEqual([row["id"] for row in support.call(repinned, "kumi_history", {})["items"]],
                         applied["history_ids"], "a setup that needs redoing doesn't hide HISTORY")
        (self.data / "state" / "history.json").write_text(json.dumps({"schema": 2, "items": []}), encoding="utf-8")
        self.assertEqual(support.call(self.host, "kumi_history", {})["note"], "Kumi's published HISTORY can't be read.")

    def test_history_after_kumi_died_says_its_last_change_may_be_missing(self):
        support.call(self.host, "kumi_connect", {}, session_id="s1")
        support.call(self.host, "kumi_change", LOUDER, session_id="s1")
        self.host.runtimes.stop_all()
        owner = self.data / "state" / "owner.json"  # as a runtime that died leaves it: running, its last beat long ago
        owner.write_text(json.dumps({**json.loads(owner.read_text(encoding="utf-8")), "running": True, "heartbeat": 1,
                                     "at": 1}), encoding="utf-8")
        published = support.call(self.host, "kumi_history", {})
        self.assertTrue(published["stale"])
        self.assertIn("Kumi stopped unexpectedly; its last change may be missing here or unsure.", published["note"])

    def test_a_scheduled_job_reads_published_history_even_with_kumi_running_here(self):
        support.call(self.host, "kumi_connect", {}, session_id="s1")
        support.call(self.host, "kumi_change", LOUDER, session_id="s1")
        asked = len(self.received("history"))
        self.ctx.cron_execution = object()  # Hermes's ctx.current_cron_execution(): a cron run is executing now
        published = support.call(self.host, "kumi_history", {}, session_id="cron-1")
        self.assertEqual((published["from"], len(published["items"])), ("last_published", 1))
        self.assertIn("scheduled job", published["note"])
        self.assertEqual(len(self.received("history")), asked, "the runtime was never asked (12d)")

    def test_while_a_stopped_runtime_settles_status_and_the_hook_say_so(self):
        host, _, data = support.host(self, None, "--slow", "history=3")
        support.call(host, "kumi_connect", {}, session_id="s1")
        live = host.runtimes.get(data)
        caller = threading.Thread(target=lambda: support.call(host, "kumi_history", {}, session_id="s1"))
        caller.start()
        self.addCleanup(live.gone.wait, 15)  # after the call: the runtime settles and exits before its folder goes
        self.addCleanup(caller.join)
        deadline = time.monotonic() + 10
        while live.in_flight != 1 and time.monotonic() < deadline:  # the call is in flight
            time.sleep(0.02)
        host.on_session_finalize("s1")  # the chat ends with its call in flight
        status = support.call(host, "kumi_status", {})
        self.assertEqual((status["running"], status["settling"], status["owner"]["session_id"]), (True, True, "s1"),
                         "the holder the runtime last published")
        decision = policy.pre_tool_call(host, tool_name="kumi_change", args=LOUDER, session_id="s2", tool_call_id="c1")
        self.assertEqual(decision["action"], "block")
        self.assertNotIn("change", [m.get("params", {}).get("name") for m in support.requests(data / "state")],
                         "the hook asked the stopping runtime for nothing")

    def test_a_write_whose_answer_never_came_is_unsure_and_never_sent_again(self):
        host, _, data = support.host(self, None, "--slow", "change=2")
        support.call(host, "kumi_connect", {}, session_id="s1")
        with mock.patch.object(tools, "WRITE_TIMEOUT", 0.3):
            unsure = support.call(host, "kumi_change", LOUDER, session_id="s1")
        self.assertEqual(unsure["state"], "unsure")
        self.assertIn("Don't send it again", unsure["hint"])
        sent = [m for m in support.requests(data / "state") if m.get("params", {}).get("name") == "change"
                and not m["params"]["arguments"].get("preview")]
        self.assertEqual(len(sent), 1)

    def test_an_approved_folder_is_kept_for_the_next_runtime(self):
        support.call(self.host, "kumi_connect", {}, session_id="s1")
        added = support.call(self.host, "kumi_library", {"action": "add_folder", "folder": support.DRUMS + os.sep})
        self.assertEqual(added["folders"], [support.DRUMS])
        self.assertEqual(self.ctx.settings["library_folders"], [support.DRUMS])
        support.call(self.host, "kumi_library", {"action": "add_folder", "folder": support.DRUMS})
        self.assertEqual(self.ctx.settings["library_folders"], [support.DRUMS], "kept once")
        self.assertIn("isn't a full path", support.call(self.host, "kumi_library", {"action": "add_folder",
                                                                                   "folder": "Drums"})["blocked"])
        self.host.runtimes.stop_all()
        support.call(self.host, "kumi_connect", {}, session_id="s1")
        started = json.loads((self.data / "state" / "fixture-start.json").read_text(encoding="utf-8"))
        self.assertEqual(started["argv"][-2:], ["--library-root", support.DRUMS])

    def test_sounds_reach_the_model_by_id_never_by_path(self):
        support.call(self.host, "kumi_connect", {}, session_id="s1")
        found = support.call(self.host, "kumi_sounds", {"query": "snare"})
        self.assertEqual([item["sound_id"] for item in found["items"]], ["s-r1-1", "s-r1-2"])
        self.assertFalse([item for item in found["items"] if "path" in item], found)
        measured = support.call(self.host, "kumi_listen", {"source": {"sound_id": "s-r1-1", "map_revision": "r1"}})
        self.assertTrue(measured["media_tag"].startswith("MEDIA:"))

    def test_a_listens_media_line_is_passed_on_only_when_it_names_its_kept_file(self):
        support.call(self.host, "kumi_connect", {}, session_id="s1")
        measured = support.call(self.host, "kumi_listen", {"source": {"sound_id": "s-r1-1", "map_revision": "r1"}})
        kept = os.path.realpath(self.data / "state" / "artifacts" / f"{measured['artifact_id']}.wav")
        self.assertEqual(measured["media_tag"], f"MEDIA:{kept}", "the kept file, by its real path")
        roundabout = self.data / "state" / "artifacts" / ".." / "artifacts" / f"{measured['artifact_id']}.wav"
        self.assertEqual(tools.kept_media(self.host, {**measured, "media_tag": f"MEDIA:{roundabout}"}), measured)
        with mock.patch.object(support.module("snapshots"), "MEDIA_MAX_BYTES", 1):
            self.assertEqual(tools.kept_media(self.host, measured), measured, "a long render's too, past the cap")
        for forged in ("MEDIA:" + str(self.data / "kumi-home" / "bridge.secret"), "MEDIA:/etc/hosts", "not a tag"):
            with self.subTest(forged=forged):
                passed = tools.kept_media(self.host, {**measured, "media_tag": forged})
                self.assertNotIn("media_tag", passed)
                self.assertEqual(passed["metrics"], measured["metrics"], "the measurements stay")

    def test_a_folder_setting_with_a_tilde_is_expanded_and_one_the_guard_refuses_left_out(self):
        home = Path(os.path.realpath(self.enterContext(tempfile.TemporaryDirectory())))
        (home / "Samples").mkdir()
        with mock.patch.dict(os.environ, {"HOME": str(home), "USERPROFILE": str(home)}):
            self.ctx.settings["library_folders"] = ["~/Samples", "Samples", os.path.abspath(os.sep), "~", "~/Samples"]
            self.assertEqual(self.host.library_folders(), [str(home / "Samples")], "kumi serve starts with these")

    def test_every_tool_answers_json_whatever_it_gets(self):
        for name, _, _, _ in tools.TOOLS:
            for args in (None, [], {"daw": "logic"}, {"unknown": 1}):
                with self.subTest(tool=name, args=args):
                    answer = support.call(self.host, name, args, session_id="s1")
                    self.assertIsInstance(answer, dict)


class Command(unittest.TestCase):
    def test_kumi_says_where_kumi_stands(self):
        commands = support.module("commands")
        asks = SimpleNamespace(is_current_session_yolo_enabled=lambda: False)  # Hermes's check: this chat isn't /yolo
        self.enterContext(mock.patch.dict("sys.modules", {"tools": SimpleNamespace(approval=asks),
                                                          "tools.approval": asks}))
        host, _, data = support.host(self, installed=False)
        self.assertEqual(commands.kumi(host, "", session_id="s1"),
                         "Kumi isn't ready: Kumi needs its runtime: 26.0 MB from github.com/user1303836/kumi. Ask the "
                         "agent to set Kumi up, or run `hermes kumi setup` in a terminal (Hermes leaves plugin "
                         "commands out when `plugins.isolation` is `host`).")
        support.install(data)
        support.call(host, "kumi_connect", {}, session_id="s1")
        self.assertEqual(commands.kumi(host, "", session_id="s1").splitlines()[1],
                         "Connected to Live: Night Drive (epoch 1). Changes are held by this chat.")


if __name__ == "__main__":
    unittest.main()
