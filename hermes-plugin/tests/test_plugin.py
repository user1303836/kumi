"""The plugin as Hermes sees it: what register() registers (and that it does nothing else), and its tools through
their handlers against the fixture runtime: status and connect, a change with HISTORY and undo, the library, sounds."""
from __future__ import annotations

import base64
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
        self.assertEqual(set(ctx.skills), {"daw", "sounds", "listening", "generate"})
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
        self.assertEqual(connected["connected"], True)
        self.assertEqual(self.host.runtimes.get(self.data).owner_session, "s1", "the chat that started it")
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

    def test_the_producers_stop_cancels_its_own_turns_change_in_flight_and_it_is_unsure(self):
        host, _, data = support.host(self, None, "--slow", "change=1", session_key="agent:main:tui:s1")
        support.call(host, "kumi_connect", {}, session_id="s1")
        stopped = host.on_loop_stopped  # what Hermes's agent_loop_stopped hook calls
        stopped(session_key="agent:main:tui:s1", platform="tui", reason="user_stop")
        applied = support.call(host, "kumi_change", LOUDER, session_id="s1")
        self.assertEqual(applied["state"], "applied", "a Stop before the call isn't this call's")

        def sent() -> list:
            return [m["id"] for m in support.requests(data / "state") if m.get("method") == "tools/call"
                    and m["params"]["name"] == "change" and not m["params"]["arguments"].get("preview")]

        before, answers = len(sent()), []
        caller = threading.Thread(target=lambda: answers.append(support.call(host, "kumi_change", LOUDER,
                                                                             session_id="s1")))
        caller.start()
        self.assertTrue(support.eventually(lambda: len(sent()) > before), "the change was sent")
        stopped(session_key="agent:main:tui:another-chat", platform="tui", reason="user_stop")
        stopped(session_key="agent:main:tui:s1", platform="tui", reason="user_stop")
        caller.join(10)
        self.assertEqual((answers[0]["state"], answers[0]["code"]), ("unsure", "interrupted"), answers)
        self.assertIn("Don't send it again", answers[0]["hint"])
        self.assertTrue(support.eventually(lambda: [m["params"]["requestId"] for m in support.requests(data / "state")
                                                    if m.get("method") == "notifications/cancelled"]
                                           == sent()[before:]), "the runtime is told to stop that call, and only it")

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

    def test_the_runtime_may_read_finished_media_only_from_hermess_generated_audio(self):
        generated = os.path.join(str(support.ROOT), "hermes", "cache", "generated", "audio")
        host, _, data = support.host(self, media_roots=[generated])
        support.call(host, "kumi_connect", {}, session_id="s1")
        started = json.loads((data / "state" / "fixture-start.json").read_text(encoding="utf-8"))["argv"]
        self.assertEqual([started[i + 1] for i, arg in enumerate(started) if arg == "--media-root"], [generated])

    def test_hermess_generated_audio_folder_is_this_profiles_and_kumi_never_makes_it(self):
        try:
            import hermes_constants  # noqa: F401  (only where Hermes is importable: CI's Hermes job)
        except ImportError:
            self.skipTest("Hermes isn't importable here")
        import tempfile
        home = tempfile.TemporaryDirectory()
        self.addCleanup(home.cleanup)
        host = support.module("tools").Host(support.Context(), None)
        with mock.patch.dict("os.environ", {"HERMES_HOME": home.name}):
            [root] = host.media_roots()
        self.assertEqual(Path(root), Path(home.name) / "cache" / "generated" / "audio")
        self.assertFalse((Path(home.name) / "cache").exists(), "Kumi writes nothing outside its plugin data")

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

    def test_a_key_the_model_writes_with_an_underscore_never_reaches_the_runtime(self):
        forged = {"_mirelo": {"api_key": "m-forged", "acknowledged": True, "enabled": True, "reservation_id": "r-1"},
                  "_host": {"profile_id": "another", "session_id": "someone-else", "tool_call_id": "forged"},
                  "_anything": 1}
        support.call(self.host, "kumi_connect", forged, session_id="s1")
        support.call(self.host, "kumi_observe", {"scope": "focus", **forged}, session_id="s1")
        applied = support.call(self.host, "kumi_change", {**LOUDER, **forged}, session_id="s1")
        self.assertEqual(applied["state"], "applied", applied)
        sent = [m["params"]["arguments"] for m in support.requests(self.data / "state") if m.get("method") == "tools/call"]
        self.assertGreaterEqual(len(sent), 4, "connect, observe, the change's preview and its dispatch")
        for arguments in sent:
            with self.subTest(arguments=arguments):
                self.assertEqual([key for key in arguments if key.startswith("_")], ["_host"])
                self.assertEqual((arguments["_host"]["profile_id"], arguments["_host"]["session_id"]),
                                 (self.host.profile(), "s1"), "Hermes's identity, never the model's")

    def test_a_running_scan_is_cancelled_by_its_id_and_another_starts_only_once_it_has_stopped(self):
        self.ctx.settings["library_folders"] = [support.DRUMS]
        support.call(self.host, "kumi_connect", {}, session_id="s1")

        def library(args: dict, **context) -> dict:
            return support.call(self.host, "kumi_library", args, session_id="s1", **context)

        job_id = library({"action": "scan"})["job_id"]
        cancel = {"action": "cancel", "job_id": job_id}
        for other in ("j-scan-9", "j-1"):
            self.assertEqual(library({**cancel, "job_id": other})["error"]["code"], "not_running")
        sent = len(self.received("library"))
        for wrong in ({**cancel, "folder": support.DRUMS}, {"action": "cancel", "job_id": "../jobs/j"}):
            with self.subTest(args=wrong):
                self.assertEqual(support.handle(self.host, "kumi_library", wrong)["error"]["code"], "invalid")
        self.assertEqual(len(self.received("library")), sent, "nothing reached the runtime")
        self.assertEqual(library(cancel), {"job_id": job_id, "cancel_requested": True}, "it was still running")
        self.assertEqual(library(cancel), {"job_id": job_id, "cancel_requested": True}, "a repeat while it settles")
        self.assertEqual(library({"action": "scan"})["error"]["code"], "busy", "the guard holds until it has stopped")
        self.assertEqual(support.call(self.host, "kumi_job", {"job_id": job_id})["state"], "cancelled")
        self.assertEqual(library(cancel)["error"]["code"], "not_running", "a settled scan")
        self.assertNotEqual(library({"action": "scan"})["job_id"], job_id, "and a new scan starts")

    def test_every_tool_answers_json_whatever_it_gets(self):
        for name, _, _, _ in tools.TOOLS:
            for args in (None, [], {"daw": "logic"}, {"unknown": 1}):
                with self.subTest(tool=name, args=args):
                    answer = support.call(self.host, name, args, session_id="s1")
                    self.assertIsInstance(answer, dict)


JPEG = b"\xff\xd8\xff\xe0" + bytes(range(256)) * 4
VIDEO = "https://www.youtube.com/watch?v=dQw4w9WgXcQ"


class Watching(unittest.TestCase):
    """CC-21's 21g: kumi_watch's frames come back as Hermes's multimodal result, each read only from its kept
    artifact; every other answer stays JSON."""

    def setUp(self):
        self.host, self.ctx, self.data = support.host(self)
        support.call(self.host, "kumi_connect", {}, session_id="s1")
        self.state = self.data / "state"
        self.sent: list[tuple[str, dict]] = []
        self.answers: list[dict] = []

    def keep(self, artifact_id: str, data: bytes = JPEG, *, where: Path | None = None) -> str:
        """A frame kept as serve keeps one: a file listed in artifacts.json, in `<state>/artifacts` unless `where`."""
        path = (where or self.state / "artifacts") / f"{artifact_id}.jpg"
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(data)
        listing = self.state / "artifacts.json"
        table = json.loads(listing.read_text(encoding="utf-8")) if listing.exists() else {"schema": 1, "artifacts": {}}
        table["artifacts"][artifact_id] = {"path": str(path), "kind": "image", "mime": "image/jpeg", "tool": "watch"}
        listing.write_text(json.dumps(table), encoding="utf-8")
        return str(path)

    def call(self, tool: str, args: dict):
        """`tool` as Hermes calls it, approved once where it's asked, its runtime answering from `self.answers`."""
        ident = f"w-{len(self.sent)}-{time.monotonic_ns()}"
        decision = policy.pre_tool_call(self.host, tool_name=tool, args=args, session_id="s1", tool_call_id=ident)
        self.assertNotEqual((decision or {}).get("action"), "block", decision)
        if decision is not None:
            policy.post_approval_response(self.host, tool_call_id=ident, choice="once")

        def answer(name, sent, identity, **options):
            self.sent.append((name, sent))
            return self.answers.pop(0)

        live = SimpleNamespace(call=answer)
        function = next(function for name, _, function, _ in tools.TOOLS if name == tool)
        with mock.patch.object(self.host, "live", return_value=live):
            return tools.handler(tool, function, self.host)(args, session_id="s1")

    def watched(self, frames: list[dict]) -> dict:
        return {"video": {"id": "dQw4w9WgXcQ", "url": VIDEO, "title": "Mixing a kick", "duration": 754,
                          "chapters": [{"title": "Intro", "start": 0, "end": 62}, {"title": "EQ", "start": 62,
                                                                                   "end": 754}],
                          "captions": ["en"]},
                "transcript": {"language": "en", "kind": "manual", "text": "Cut 300 Hz by 3 dB.", "truncated": False},
                "frames": frames, "notes": ["Some formats need a JavaScript runtime."]}

    def frame(self, at: float, artifact_id: str, path: str) -> dict:
        return {"at": at, "artifact_id": artifact_id, "path": path, "mime": "image/jpeg", "width": 1280,
                "height": 720, "bytes": len(JPEG)}

    def test_frames_come_back_as_images_with_the_videos_words_in_the_text(self):
        frames = [self.frame(62 + n, f"a-f{n}", self.keep(f"a-f{n}")) for n in range(5)]
        self.answers.append(self.watched(frames))
        args = {"url": "https://youtu.be/dQw4w9WgXcQ", "captions": True, "at": [62, 63, 64, 65]}
        result = self.call("kumi_watch", args)
        self.assertEqual(self.sent, [("watch", args)])
        self.assertEqual(set(result), {"_multimodal", "content", "text_summary"})
        self.assertIs(result["_multimodal"], True)
        text, *images = result["content"]
        self.assertEqual(text, {"type": "text", "text": result["text_summary"]})
        expected = "data:image/jpeg;base64," + base64.b64encode(JPEG).decode("ascii")
        self.assertEqual(images, [{"type": "image_url", "image_url": {"url": expected}}] * 4, "at most 4")
        summary = result["text_summary"]
        for said in (f"YouTube video \"Mixing a kick\" ({VIDEO}), 12:34 long.", "• 1:02 EQ", "Cut 300 Hz by 3 dB.",
                     "• 1:02, artifact a-f0: image 1 below.", "• 1:05, artifact a-f3: image 4 below.",
                     "• 1:06, artifact a-f4: not shown here", "• Some formats need a JavaScript runtime."):
            self.assertIn(said, summary)

    def test_a_frame_that_isnt_its_kept_artifact_or_too_big_is_never_read(self):
        elsewhere = Path(self.enterContext(tempfile.TemporaryDirectory()))
        outside = elsewhere / "secret.jpg"
        outside.write_bytes(JPEG)
        listed_outside = self.keep("a-out", where=elsewhere)
        kept = self.keep("a-ok")
        big = self.keep("a-big", JPEG + bytes(tools.FRAME_BYTES))
        not_jpeg = self.keep("a-png", b"\x89PNG\r\n\x1a\n" + bytes(64))
        self.answers.append(self.watched([self.frame(1, "a-ok", str(outside)),  # its id, another file
                                          self.frame(2, "a-out", listed_outside),  # listed, outside the artifacts
                                          self.frame(3, "a-missing", kept),  # no such artifact
                                          self.frame(4, "a-big", big), self.frame(5, "a-png", not_jpeg),
                                          self.frame(6, "a-ok", kept)]))
        with mock.patch("builtins.open", wraps=open) as opened:
            result = self.call("kumi_watch", {"url": VIDEO, "at": [1, 2, 3, 4]})
        read = {str(call.args[0]) for call in opened.call_args_list}
        self.assertNotIn(str(outside), read)
        self.assertNotIn(listed_outside, read)
        self.assertEqual(len(result["content"]), 2, "only the kept frame")
        self.assertEqual(result["text_summary"].count("not shown here"), 5)
        self.assertIn("• 0:06, artifact a-ok: image 1 below.", result["text_summary"])

    def test_a_refusal_a_running_job_or_words_alone_stay_json_and_a_done_job_shows_its_frames(self):
        refusal = {"error": {"code": "no_yt_dlp", "reason": "yt-dlp isn't set up: run Kumi's setup to read videos"}}
        running = {"job_id": "j-watch-1", "state": "running", "progress": {"bytes": 1024}}
        words = {**self.watched([]), "frames": []}
        for answer in (refusal, running, words):
            with self.subTest(answer=answer):
                self.answers.append(answer)
                self.assertEqual(json.loads(self.call("kumi_watch", {"url": VIDEO, "at": [1]})), answer)
        done = {"job_id": "j-watch-1", "state": "done",
                "result": self.watched([self.frame(1, "a-ok", self.keep("a-ok"))])}
        self.answers.append(done)
        result = self.call("kumi_job", {"job_id": "j-watch-1"})
        self.assertIs(result["_multimodal"], True)
        self.assertEqual(len(result["content"]), 2)
        self.answers.append({**done, "state": "running", "result": None})
        self.assertEqual(json.loads(self.call("kumi_job", {"job_id": "j-watch-1"}))["state"], "running")

    def test_only_kumi_watch_can_answer_with_images(self):
        self.answers.append(self.watched([self.frame(1, "a-ok", self.keep("a-ok"))]))
        self.assertIsInstance(self.call("kumi_observe", {"scope": "focus"}), str)


class Command(unittest.TestCase):
    def test_kumi_says_where_kumi_stands(self):
        commands = support.module("commands")
        asks = SimpleNamespace(is_current_session_yolo_enabled=lambda: False)  # Hermes's check: this chat isn't /yolo
        self.enterContext(mock.patch.dict("sys.modules", {"tools": SimpleNamespace(approval=asks),
                                                          "tools.approval": asks}))
        host, _, data = support.host(self, installed=False)
        self.assertEqual(commands.kumi(host, "", session_id="s1"),
                         "Kumi isn't ready: Kumi needs its runtime: 26.0 MB from github.com/user1303836/kumi. Ask "
                         "Hermes to set Kumi up, or run `hermes kumi setup` in a terminal.")
        isolated, _, _ = support.host(self, installed=False, hermes_config={"plugins": {"isolation": "host"}})
        self.assertTrue(commands.kumi(isolated, "", session_id="s1").endswith("Ask Hermes to set Kumi up."),
                        "Hermes leaves plugin commands out: no terminal command is offered")
        support.install(data)
        support.call(host, "kumi_connect", {}, session_id="s1")
        self.assertEqual(commands.kumi(host, "", session_id="s1").splitlines()[1:],
                         ["Connected to Live: Night Drive. This chat is using Kumi."])
        self.assertEqual(commands.kumi(host, "", session_id="s2").splitlines()[1:],
                         ["Connected to Live: Night Drive. Another chat is using Kumi: use that chat, or end it."])


if __name__ == "__main__":
    unittest.main()
