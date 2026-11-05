"""runtime.py against the fixture runtime: what setup installed, the scrubbed launch, the check that it's the pinned
runtime, calls and their errors, a runtime that stops, doesn't answer, floods its stderr, closes its output or won't
exit, events, one runtime per profile, and who stops it (its chat, idleness)."""
from __future__ import annotations

import contextvars
import json
import logging
import os
from pathlib import Path
import stat
import subprocess
import sys
import tempfile
import threading
import time
import unittest
from unittest import mock

import support
from support import eventually

runtime = support.module("runtime")


def _takes(lock) -> bool:
    """Whether `lock` (a StateLock) can be taken now; it's held when this says so."""
    try:
        lock.acquire()
    except runtime.Unavailable:
        return False
    return True


class Runtimes(unittest.TestCase):
    def setUp(self):
        folder = tempfile.TemporaryDirectory()
        self.addCleanup(folder.cleanup)
        self.data = Path(folder.name)
        self.events: list[tuple[str, dict]] = []
        self.runtimes = self.make()

    def make(self, **options):
        options.setdefault("hermes", lambda: (None, None))  # never this machine's Hermes homes
        runtimes = runtime.Runtimes(events=lambda event, payload: self.events.append((event, payload)),
                                    environ=lambda: {"PATH": "/usr/bin:/bin", "HOME": str(self.data),
                                                     "OPENAI_API_KEY": "sk-secret", "MIRELO_API_KEY": "m-secret",
                                                     "HERMES_HOME": "/elsewhere", "KUMI_HOME": "/elsewhere"},
                                    start_timeout=10, **options)
        self.addCleanup(support.stopped, runtimes, self.data)
        return runtimes

    def start(self, *fixture_args: str, session: str | None = "s1", runtimes=None, daw: str = "ableton"):
        support.install(self.data, *fixture_args)
        return (runtimes or self.runtimes).start(self.data, runtime.installed(self.data, support.VERSION), daw, session,
                                                 profile="studio")

    def host(self, session: str = "s1") -> dict:
        return {"profile_id": "default", "session_id": session, "task_id": "t1", "tool_call_id": None,
                "request_id": "r1"}

    def started(self) -> dict:
        return json.loads((self.data / "state" / "fixture-start.json").read_text(encoding="utf-8"))

    def test_a_started_runtime_is_the_pinned_one_and_answers(self):
        live = self.start()
        self.assertEqual(live.info, {"name": "kumi-serve", "version": support.VERSION})
        status = live.call("status", {}, self.host(), timeout=10)
        self.assertEqual(status["runtime"], {"version": support.VERSION, "protocol": "kumi-serve/1"})
        self.assertEqual(self.started()["argv"], ["serve", "--stdio", "--state-dir", str(self.data / "state"),
                                                  "--kumi-home", str(self.data / "kumi-home"), "--daw", "ableton"])
        self.assertIs(self.runtimes.get(self.data), live)
        self.assertIs(self.runtimes.start(self.data, live.install, "ableton", "s2", profile="studio"), live,
                      "one runtime a profile")
        with self.assertRaises(runtime.Unavailable) as other:
            self.runtimes.start(self.data, live.install, "reaper", "s2", profile="studio")
        self.assertEqual(other.exception.code, "other_daw")

    def test_the_runtime_gets_no_key_and_never_checks_for_updates(self):
        self.start()
        environment = set(self.started()["environment"])
        self.assertTrue({"PATH", "HOME", "KUMI_NO_UPDATE_CHECK"} <= environment, environment)
        self.assertFalse(environment & {"OPENAI_API_KEY", "MIRELO_API_KEY", "HERMES_HOME", "KUMI_HOME"}, environment)
        self.assertEqual(runtime.launch_environment({"Path": "C:\\Windows", "SystemRoot": "C:\\Windows",
                                                     "AWS_SECRET_ACCESS_KEY": "x"}, windows=True),
                         {"Path": "C:\\Windows", "SystemRoot": "C:\\Windows", "KUMI_NO_UPDATE_CHECK": "1"})

    def test_the_runtime_is_told_hermess_homes_by_argument_never_by_its_environment(self):
        home, root = str(self.data / "hermes" / "profiles" / "studio"), str(self.data / "hermes")
        self.start(runtimes=self.make(hermes=lambda: (home, root)))  # where it takes no library root (#985)
        started = self.started()
        self.assertEqual(started["argv"][-4:], ["--hermes-home", home, "--hermes-root", root])
        self.assertNotIn("HERMES_HOME", started["environment"])
        argv = runtime.serve_argv(runtime.installed(self.data, support.VERSION), self.data / "state", "ableton",
                                  hermes=("relative/home", None))
        self.assertNotIn("--hermes-home", argv, "only a full path")

    @unittest.skipIf(os.name == "nt", "a session of its own is POSIX's; Windows gives it no console")
    def test_the_runtime_runs_in_a_session_of_its_own_so_hermess_ctrl_c_never_reaches_it(self):
        live = self.start()
        self.assertNotEqual(os.getsid(live.client.pid), os.getsid(0))

    def test_another_version_or_protocol_is_refused_before_anything_else_and_stopped(self):
        for args in (("--server-version", "1.24.65"), ("--protocol", "kumi-serve/2")):
            with self.subTest(args=args):
                with self.assertRaises(runtime.Unavailable) as refused:
                    self.start(*args)
                self.assertEqual(refused.exception.code, "wrong_runtime")
                self.assertIsNone(self.runtimes.get(self.data))
                asked = [message.get("params", {}).get("name") for message in support.requests(self.data / "state")
                         if message.get("method") == "tools/call"]
                self.assertLessEqual(set(asked), {"status"}, "nothing but the status naming its protocol was asked")
                (self.data / "state" / "fixture-requests.jsonl").unlink()
        self.start()  # the profile's lock was freed both times

    def test_a_refusal_carries_the_runtimes_code_reason_and_owner(self):
        live = self.start("--leased-by", "s9")
        with self.assertRaises(runtime.Refused) as refused:
            live.call("observe", {"scope": "focus"}, self.host(), timeout=10)
        self.assertEqual(refused.exception.as_json(), {"error": {
            "code": "not_connected", "reason": "Kumi isn't connected to the DAW: connect first."}})
        with self.assertRaises(runtime.Refused) as leased:
            live.call("change", {"title": "x"}, self.host(), timeout=10)
        self.assertEqual((leased.exception.code, leased.exception.owner["session_id"]), ("leased", "s9"))

    def test_a_runtime_that_stops_mid_call_fails_it_and_nothing_is_sent_again(self):
        live = self.start("--exit-on", "change")
        live.call("connect", {"daw": "ableton"}, self.host(), timeout=10)
        self.events.clear()
        with self.assertRaises(runtime.Unavailable) as stopped:
            live.call("change", {"title": "Louder", "steps": []}, self.host(), timeout=10)
        self.assertEqual(stopped.exception.code, "stopped")
        with self.assertRaises(runtime.Unavailable) as after:
            live.call("status", {}, self.host(), timeout=10)
        self.assertEqual(after.exception.code, "not_running", "a call after the end was never sent")
        changes = [m for m in support.requests(self.data / "state") if m.get("params", {}).get("name") == "change"]
        self.assertEqual(len(changes), 1, "the change was sent once and never again")
        self.assertIsNone(self.runtimes.get(self.data))
        stopped = ("status.changed", {"seq": None, "profile": "studio"})
        self.assertTrue(eventually(lambda: [event for event in self.events if event[1]["seq"] is None] == [stopped]),
                        self.events)

    def test_a_call_past_its_time_is_cancelled_and_reported_unknown(self):
        live = self.start("--slow", "history=1.5")
        with self.assertRaises(runtime.Unavailable) as late:
            live.call("history", {}, self.host(), timeout=0.3)
        self.assertEqual(late.exception.code, "timeout")
        self.assertIn("unknown", late.exception.reason)
        live.call("status", {}, self.host(), timeout=10)  # the runtime still answers once it's done
        received = support.requests(self.data / "state")
        asked = next(m["id"] for m in received if m.get("params", {}).get("name") == "history")
        self.assertIn({"jsonrpc": "2.0", "method": "notifications/cancelled",
                       "params": {"requestId": asked, "reason": "Hermes stopped waiting"}}, received)

    def on_disk(self, *names: str) -> set[tuple[str, int]]:
        """The (event, seq) each named snapshot (status, focus, history by default) has on disk now."""
        state = self.data / "state"
        return {(f"{name}.changed", json.loads((state / f"{name}.json").read_text(encoding="utf-8"))["seq"])
                for name in names or ("status", "focus", "history")}

    def reported(self) -> set[tuple[str, object]]:
        return {(event, payload.get("seq")) for event, payload in self.events}

    def test_each_snapshot_the_runtime_says_has_landed_is_a_desktop_event_at_once(self):
        with mock.patch.object(runtime, "WATCH_EVERY", 3600):  # nothing is polled: only what the runtime says
            live = self.start()  # it offers C2's resources
            self.events.clear()
            live.call("connect", {"daw": "ableton"}, self.host(), timeout=10)
            expected = self.on_disk()
            self.assertTrue(eventually(lambda: expected <= self.reported()), self.events)
            live.call("library", {"action": "scan"}, self.host(), timeout=10)
            self.assertEqual(live.call("job", {"job_id": "j-scan-1"}, self.host(), timeout=10)["state"], "done")
            seq = json.loads((self.data / "state" / "map" / "r1.json").read_text(encoding="utf-8"))["seq"]
            changed = ("map.changed", {"revision": "r1", "seq": seq, "profile": "studio"})
            self.assertTrue(eventually(lambda: changed in self.events), "the map's pointer flipped to the scan's")
        self.assertEqual({payload["profile"] for _, payload in self.events}, {"studio"})

    def test_a_snapshot_the_runtime_tells_of_is_never_reported_twice(self):
        live = self.start()
        self.events.clear()
        live.call("connect", {"daw": "ableton"}, self.host(), timeout=10)
        expected = self.on_disk()
        live.call("library", {"action": "scan"}, self.host(), timeout=10)  # a job's file: only the polling sees it
        self.assertTrue(eventually(lambda: any(event == "job.progress" for event, _ in self.events)), self.events)
        told = [(event, payload.get("seq")) for event, payload in self.events if event != "job.progress"]
        self.assertEqual(sorted(told), sorted(expected), "a pass that polled after them reported none again")

    def test_what_the_runtime_writes_as_it_starts_is_reported_at_once(self):
        with mock.patch.object(runtime, "WATCH_EVERY", 3600):  # one look, at once, and no polling after it
            self.start("--publish-at-start")  # a new runtime's HISTORY and FOCUS, before anything subscribed
            published = self.on_disk("history", "focus")
            self.assertTrue(eventually(lambda: published <= self.reported()), self.events)

    def test_a_snapshot_the_runtime_wont_tell_of_is_still_found_and_each_change_is_reported_once(self):
        with mock.patch.object(runtime, "WATCH_EVERY", 0.02):  # the backstop looks every 0.2 s
            live = self.start("--refuse-subscribe", "kumi://focus", "--quiet", "kumi://history")
            self.events.clear()
            live.call("connect", {"daw": "ableton"}, self.host(), timeout=10)
            expected = self.on_disk()  # status told, focus polled, history found by the backstop
            self.assertTrue(eventually(lambda: expected <= self.reported()), self.events)
            louder = {"title": "Louder drums", "expected_project_key": "live:night-drive", "expected_epoch": "1",
                      "steps": [{"tool": "set_track_volume", "input": {"track_ref": "t1", "volume_db": -4}}]}
            self.assertEqual(live.call("change", louder, self.host(), timeout=10)["state"], "applied")
            expected = self.on_disk()
            self.assertTrue(eventually(lambda: expected <= self.reported()),
                            "the change's HISTORY too, which only a later backstop look can find")
        said = [(event, payload["seq"]) for event, payload in self.events if payload.get("seq") is not None]
        self.assertEqual(len(said), len(set(said)), f"each change once, however it was found: {said}")

    def test_a_runtime_that_offers_no_resources_has_each_snapshot_it_writes_found_and_reported(self):
        live = self.start("--no-resources")  # kumi serve before #1047
        self.events.clear()
        live.call("connect", {"daw": "ableton"}, self.host(), timeout=10)
        expected = self.on_disk()
        self.assertTrue(eventually(lambda: expected <= self.reported()), self.events)
        live.call("library", {"action": "scan"}, self.host(), timeout=10)
        job = ("job.progress", {"job_id": "j-scan-1", "state": "running", "progress": {"done": 0, "total": 2},
                                "profile": "studio"})
        self.assertTrue(eventually(lambda: job in self.events), self.events)
        live.call("job", {"job_id": "j-scan-1"}, self.host(), timeout=10)
        self.assertTrue(eventually(lambda: any(event == "map.changed" for event, _ in self.events)), self.events)
        self.assertEqual({payload["profile"] for _, payload in self.events}, {"studio"})

    def test_a_runtime_flooding_its_stderr_still_answers(self):
        live = self.start("--stderr-flood", str(4 * 1024 * 1024))
        self.assertTrue(live.call("status", {}, self.host(), timeout=10)["runtime"])

    def test_ending_the_chat_that_started_it_stops_it_once_and_another_chats_end_doesnt(self):
        live = self.start(session="s1")
        self.runtimes.stop_session("s2")
        self.assertIs(self.runtimes.get(self.data), live)
        self.events.clear()
        self.runtimes.stop_session("s1")
        self.assertIsNone(self.runtimes.get(self.data))
        self.assertTrue(eventually(lambda: not live.client.running), "its process ended")
        self.assertEqual([event for event in self.events if event[1]["seq"] is None],
                         [("status.changed", {"seq": None, "profile": "studio"})], "one stop event")

    def test_an_idle_runtime_stops_even_with_no_chat_but_never_with_a_call_in_flight(self):
        runtimes = self.make(idle_after=0.3)
        live = self.start("--slow", "history=1.2", session=None, runtimes=runtimes)
        answers = []
        caller = threading.Thread(target=lambda: answers.append(live.call("history", {}, self.host(), timeout=10)))
        caller.start()
        caller.join()
        self.assertEqual(len(answers), 1, "the slow call outlived the idle time and was answered")
        self.assertTrue(eventually(lambda: runtimes.get(self.data) is None), "then, idle, it stopped")
        self.assertTrue(eventually(lambda: not live.client.running), "and its process ended")

    def test_a_runtime_connected_to_the_daw_for_a_chat_waits_longer_before_its_idle_stop(self):
        runtimes = self.make(idle_after=0.3, idle_connected=3600)
        connected = self.start(session="s1", runtimes=runtimes)  # looked at first in each pass: started first
        connected.call("connect", {"daw": "ableton"}, self.host(), timeout=10)
        other = Path(os.path.realpath(self.enterContext(tempfile.TemporaryDirectory())))
        support.install(other)
        self.addCleanup(support.stopped, runtimes, other)
        idle = runtimes.start(other, runtime.installed(other, support.VERSION), "ableton", "s2", profile="other")
        self.assertTrue(eventually(lambda: runtimes.get(other) is None), "a chat's runtime that isn't connected stops")
        self.assertIs(runtimes.get(self.data), connected,
                      "the connected one, idle longer and looked at in the same pass, still runs (kumi-adv's D2)")
        status = json.loads((self.data / "state" / "status.json").read_text(encoding="utf-8"))
        (self.data / "state" / "status.json").write_text(json.dumps({**status, "connected": False}), encoding="utf-8")
        self.assertTrue(eventually(lambda: runtimes.get(self.data) is None), "once the DAW is gone, the short wait")
        self.assertTrue(eventually(lambda: not idle.client.running and not connected.client.running))

    def test_a_runtime_running_a_job_nobody_asks_about_is_not_stopped_as_idle_until_it_ends(self):
        runtimes = self.make(idle_after=0.3, job_busy_for=3600)
        scanning = self.start(session=None, runtimes=runtimes)  # looked at first in each pass: started first
        scanning.call("library", {"action": "scan"}, self.host(), timeout=10)  # jobs/j-scan-1.json: running
        other = Path(os.path.realpath(self.enterContext(tempfile.TemporaryDirectory())))
        support.install(other)
        self.addCleanup(support.stopped, runtimes, other)
        runtimes.start(other, runtime.installed(other, support.VERSION), "ableton", None, profile="other")
        self.assertTrue(eventually(lambda: runtimes.get(other) is None), "a runtime with nothing running stops")
        self.assertIs(runtimes.get(self.data), scanning, "the scanning one, idle longer, still runs (rgzm, #1059)")
        job = self.data / "state" / "jobs" / "j-scan-1.json"
        job.write_text(json.dumps({**json.loads(job.read_text(encoding="utf-8")), "state": "done"}), encoding="utf-8")
        self.assertTrue(eventually(lambda: runtimes.get(self.data) is None), "its scan done, the short wait again")

    def test_an_idle_runtime_is_stopped_in_the_context_of_the_profile_that_started_it(self):
        profile = contextvars.ContextVar("profile")
        heard: list[tuple[object, object]] = []

        class Heard(logging.Handler):
            def emit(self, record):
                subject = record.args[0] if isinstance(record.args, tuple) and record.args else None
                heard.append((subject, profile.get(None)))

        handler, level = Heard(logging.INFO), runtime.log.level
        runtime.log.addHandler(handler)
        runtime.log.setLevel(logging.INFO)
        self.addCleanup(runtime.log.setLevel, level)
        self.addCleanup(runtime.log.removeHandler, handler)
        runtimes = self.make(idle_after=0.3)
        other = Path(self.enterContext(tempfile.TemporaryDirectory()))

        def start(name: str, data: Path):
            profile.set(name)
            support.install(data)
            return runtimes.start(data, runtime.installed(data, support.VERSION), "ableton", None, profile=name)

        first = contextvars.copy_context().run(start, "first", self.data)  # this start begins the one idle watch
        second = contextvars.copy_context().run(start, "second", other)
        # Stopped, exited and the profile's lock let go (Windows can't remove a folder while it's held).
        self.assertTrue(eventually(lambda: first.gone.is_set() and second.gone.is_set()))
        about_second = [name for subject, name in heard if subject == other]
        self.assertTrue(about_second, "its idle stop was logged")
        self.assertEqual(set(about_second), {"second"}, "never in the first profile's context")

    def test_a_chats_end_mid_call_returns_at_once_and_the_runtime_settles_holding_the_profile(self):
        live = self.start("--slow", "history=3")  # a call that takes a while to settle, as a cancelled render does
        answers = []
        caller = threading.Thread(target=lambda: answers.append(live.call("history", {}, self.host(), timeout=30)))
        caller.start()
        sent = lambda: [m for m in support.requests(self.data / "state") if m.get("params", {}).get("name") == "history"]
        # Counted in flight isn't sent yet: a stop between the two ends the call with "Nothing was sent" (Windows, #1051).
        self.assertTrue(eventually(lambda: live.in_flight == 1 and sent()), "the call is in flight, at the runtime")
        self.events.clear()
        self.runtimes.stop_session("s1")
        self.assertTrue(live.client.running, "the stop returned without waiting for the call to settle")
        self.assertIn(("status.changed", {"seq": None, "profile": "studio", "settling": True}), self.events)
        self.assertIs(self.runtimes.settling(self.data), live)
        for attempt in (lambda: self.runtimes.start(self.data, live.install, "ableton", "s2", profile="studio"),
                        lambda: self.runtimes.require(self.data)):
            with self.assertRaises(runtime.Unavailable) as busy:
                attempt()
            self.assertEqual(busy.exception.code, "busy", "nothing starts over a runtime still settling")
        self.runtimes.stop_all()  # a second stop, as an unload after the chat's end, returns at once
        self.assertTrue(live.client.running)
        caller.join()
        self.assertEqual(len(answers), 1, "the call in flight was answered as the runtime settled")
        self.assertTrue(eventually(lambda: not live.client.running and self.runtimes.get(self.data) is None))
        self.assertTrue(eventually(lambda: live.gone.is_set()), "then it exited and was retired")
        self.start(session="s2")  # and the profile came free with its exit

    @unittest.skipIf(os.name == "nt", "the test's kumi.bat runs the fixture under cmd.exe, which terminate ends without "
                                      "its child; Client's tests end the fixture itself on every system")
    def test_a_runtime_that_doesnt_settle_in_time_is_ended_and_frees_the_profile(self):
        runtimes = self.make(settle_wait=0.5)
        live = self.start("--slow", "history=30", "--ignore-eof", runtimes=runtimes)  # never exits on its own
        failures = []

        def call():
            try:
                live.call("history", {}, self.host(), timeout=60)
            except runtime.Unavailable as failure:
                failures.append(failure.code)

        caller = threading.Thread(target=call)
        caller.start()
        self.assertTrue(eventually(lambda: live.in_flight == 1 and any(
            m.get("params", {}).get("name") == "history" for m in support.requests(self.data / "state"))),
            "the call is in flight, at the runtime")
        runtimes.stop_session("s1")
        caller.join()
        self.assertEqual(failures, ["stopped"], "its call ended unknown, never resent")
        self.assertTrue(eventually(lambda: live.gone.is_set() and not live.client.running))
        lock = runtime.StateLock(self.data / "state" / "runtime.lock")
        self.assertTrue(eventually(lambda: _takes(lock)), "the profile came free")
        lock.release()

    def test_a_call_whose_answer_was_lost_still_gets_its_settle(self):
        runtimes = self.make(stop_wait=0.5)  # an idle stop would end it long before its call is done
        live = self.start("--slow", "history=3", runtimes=runtimes)
        live.call("connect", {"daw": "ableton"}, self.host(), timeout=10)  # it publishes owner.json from here on
        with self.assertRaises(runtime.Unavailable) as lost:
            live.call("history", {}, self.host(), timeout=0.3)  # cancelled: what it did is unknown, still running
        self.assertEqual((lost.exception.code, live.in_flight), ("timeout", 0))
        runtimes.stop_session("s1")
        owner = self.data / "state" / "owner.json"
        self.assertTrue(eventually(lambda: live.gone.is_set()), "it settled and exited")
        self.assertIs(json.loads(owner.read_text(encoding="utf-8"))["running"], False, "a clean stop, never a kill")

    def test_a_late_answer_clears_the_mark_so_a_later_stop_is_quick_again(self):
        live = self.start("--slow", "history=1")
        with self.assertRaises(runtime.Unavailable):
            live.call("history", {}, self.host(), timeout=0.2)
        self.assertTrue(live.client.unsettled)
        self.assertTrue(eventually(lambda: not live.client.unsettled), "its late answer came")
        self.runtimes.stop_session("s1")
        self.assertTrue(eventually(lambda: not live.client.running), "an idle runtime is waited for, as before")

    def test_a_stopped_runtime_gets_at_least_as_long_to_settle_as_any_call_of_the_plugins_waits(self):
        tools = support.module("tools")
        self.assertGreaterEqual(runtime.SETTLE_WAIT, max(tools.SETUP_TIMEOUT, tools.WRITE_TIMEOUT,
                                                         tools.CONNECT_TIMEOUT, runtime.TRACK_DEADLINE_MAX))

    def test_a_request_caught_by_a_stop_is_refused_unsent_never_reported_unknown(self):
        live = self.start()
        live.client.end_input()  # the stop ended its input just before this call queued its line
        with self.assertRaises(runtime.Unavailable) as refused:
            live.client.request("tools/call", {"name": "history", "arguments": {}}, timeout=5)
        self.assertEqual(refused.exception.code, "not_running", "refused, not unknown")
        self.assertTrue(eventually(lambda: not live.client.running))
        self.assertNotIn("history", [m.get("params", {}).get("name") for m in support.requests(self.data / "state")],
                         "it never reached the runtime")

    def test_a_second_hermes_process_on_the_profile_starts_no_runtime(self):
        live = self.start()
        with self.assertRaises(runtime.Unavailable) as other:
            self.start(runtimes=self.make())  # another Hermes process's registry, on the same profile
        self.assertEqual(other.exception.code, "other_runtime")
        self.assertIn(f"pid {os.getpid()}", other.exception.reason)
        self.runtimes.stop(self.data)
        self.assertTrue(eventually(lambda: not live.client.running))
        self.start(runtimes=self.make())  # the lock came free with the first runtime's end

    @unittest.skipIf(os.name == "nt", "on Windows the lock isn't inherited; owner.json's live pid holds the folder")
    def test_the_lock_outlives_a_hermes_process_whose_runtime_is_still_settling(self):
        live = self.start()
        live.state_lock._file.close()  # Hermes's own hold ends, as it would if Hermes died
        with self.assertRaises(runtime.Unavailable) as other:
            runtime.StateLock(self.data / "state" / "runtime.lock").acquire()
        self.assertEqual(other.exception.code, "other_runtime")
        live.state_lock._file = None
        self.runtimes.stop(self.data)
        lock = runtime.StateLock(self.data / "state" / "runtime.lock")
        lock.acquire()  # the runtime's end freed it
        lock.release()

    def test_a_chat_ending_or_an_unload_while_its_runtime_starts_ends_that_runtime(self):
        for stop in (lambda: self.runtimes.stop_session("s1"), self.runtimes.stop_all):
            with self.subTest(stop=stop):
                outcome = []

                def start():
                    try:
                        outcome.append(self.start("--slow", "status=1.5"))
                    except runtime.Unavailable as failure:
                        outcome.append(failure)

                starter = threading.Thread(target=start)
                starter.start()
                started = self.data / "state" / "fixture-start.json"
                self.assertTrue(eventually(started.exists), "the runtime is starting")
                stop()
                starter.join()
                self.assertIsInstance(outcome[0], runtime.Unavailable)
                self.assertEqual(outcome[0].code, "not_running")
                self.assertIsNone(self.runtimes.get(self.data))
                lock = runtime.StateLock(self.data / "state" / "runtime.lock")
                self.assertTrue(eventually(lambda: _takes(lock)), "the profile's lock came free")
                lock.release()
                started.unlink()

    def test_a_live_runtime_named_in_owner_json_holds_the_folder(self):
        state = self.data / "state"
        state.mkdir()
        owner = {"schema": 1, "owner": {"pid": os.getpid()}, "heartbeat": int(time.time() * 1000)}
        (state / "owner.json").write_text(json.dumps(owner), encoding="utf-8")
        self.assertEqual(runtime._live_owner(state), os.getpid())
        self.assertIsNone(runtime._live_owner(state, now_ms=owner["heartbeat"] + 11_000), "a stale heartbeat")


class Client(unittest.TestCase):
    """The client against runtimes that misbehave in ways the fixture runtime can't reach through `Runtimes`."""

    def setUp(self):
        folder = tempfile.TemporaryDirectory()
        self.addCleanup(folder.cleanup)
        self.state = Path(folder.name)

    def spawn(self, *knobs: str, **options):
        argv = [sys.executable, str(support.FIXTURE), "serve", "--stdio", "--state-dir", str(self.state), "--kumi-home",
                str(self.state), "--daw", "ableton", *knobs]
        exits = []
        client = runtime.Client(argv, {"PATH": os.environ.get("PATH", "")}, on_exit=exits.append, **options)
        self.addCleanup(client.close, 2)
        return client, exits

    def initialize(self, client) -> None:
        client.request("initialize", {"protocolVersion": runtime.MCP_PROTOCOL, "capabilities": {},
                                      "clientInfo": runtime.CLIENT_INFO}, timeout=10)

    def test_a_runtime_that_closes_its_output_and_stays_is_ended(self):
        client, exits = self.spawn("--close-stdout", exit_wait=0.3)
        with self.assertLogs(runtime.log, "WARNING") as logged:
            self.initialize(client)
            self.assertTrue(eventually(lambda: exits), "its output ended")
        self.assertIn("closed its output and is still running", logged.output[0])
        self.assertIsNotNone(client._process.poll(), "and it was ended rather than left running")

    def test_a_runtime_that_stays_after_its_stdin_ends_is_ended_by_close(self):
        client, _ = self.spawn("--ignore-eof")
        self.initialize(client)
        with self.assertLogs(runtime.log, "WARNING"):
            client.close(timeout=0.3)
        self.assertIsNotNone(client._process.poll())

    def test_a_program_that_cant_start_means_setup_is_needed(self):
        broken = self.state / "kumi"
        broken.write_text("not a program\n", encoding="utf-8")
        broken.chmod(broken.stat().st_mode | stat.S_IXUSR)
        with self.assertRaises(runtime.Unavailable) as needed:
            runtime.Client([str(broken), "serve"], {"PATH": ""})
        self.assertEqual(needed.exception.code, "setup_needed")


class Installed(unittest.TestCase):
    def setUp(self):
        folder = tempfile.TemporaryDirectory()
        self.addCleanup(folder.cleanup)
        self.data = Path(folder.name)

    def reason(self, pinned: str | None = support.VERSION) -> str:
        with self.assertRaises(runtime.Unavailable) as needed:
            runtime.installed(self.data, pinned)
        self.assertEqual(needed.exception.code, "setup_needed")
        return needed.exception.reason

    def receipt(self, **changes) -> None:
        path = self.data / "runtime" / "current.json"
        path.write_text(json.dumps({**json.loads(path.read_text(encoding="utf-8")), **changes}), encoding="utf-8")

    def test_setup_is_needed_until_setup_installed_the_pinned_runtime_for_this_computer(self):
        self.assertIn("pins no Kumi release", self.reason(pinned=None))
        self.assertIn("isn't set up", self.reason())
        kumi = support.install(self.data)
        found = runtime.installed(self.data, support.VERSION)
        self.assertEqual((found.version, found.kumi, found.kumi_home), (support.VERSION, kumi.resolve(),
                                                                         self.data / "kumi-home"))
        self.assertIn("needs 1.24.67", self.reason(pinned="1.24.67"))
        self.receipt(target="riscv64-unknown-linux-gnu")
        self.assertIn("not this computer", self.reason())

    def test_a_receipt_pointing_outside_plugin_data_or_at_nothing_runnable_is_never_run(self):
        kumi = support.install(self.data)
        outside = tempfile.TemporaryDirectory()
        self.addCleanup(outside.cleanup)
        program = Path(outside.name) / "kumi"
        program.write_text("#!/bin/sh\n", encoding="utf-8")
        program.chmod(stat.S_IRWXU)
        paths = ["../kumi", "/bin/sh", "runtime/1.24.66/missing"]
        try:
            (self.data / "runtime" / "link").symlink_to(program)  # a runnable program, outside plugin data
            paths.append("runtime/link")
        except OSError:  # Windows, without the right to make symlinks
            pass
        for path in paths:
            with self.subTest(kumi=path):
                self.receipt(kumi=path)
                self.assertIn("missing or damaged", self.reason())
        self.receipt(kumi="runtime/1.24.66/kumi")
        if os.name != "nt":
            kumi.chmod(stat.S_IRUSR | stat.S_IWUSR)
            self.assertIn("missing or damaged", self.reason(), "not executable")
            kumi.chmod(stat.S_IRWXU)
        self.receipt(schema=2)
        self.assertIn("another plugin version", self.reason())
        (self.data / "runtime" / "current.json").write_text("{", encoding="utf-8")
        self.assertIn("can't be read", self.reason())

    def test_an_adopted_transport_home_is_used_only_as_a_full_path(self):
        support.install(self.data)
        self.receipt(kumi_home=str(Path("/Users/someone/.kumi").resolve()))
        self.assertEqual(runtime.installed(self.data, support.VERSION).kumi_home, Path("/Users/someone/.kumi").resolve())
        self.receipt(kumi_home="someone/.kumi")
        self.assertIn("full path", self.reason())

    def test_the_ffmpeg_setup_allowed_is_passed_only_while_it_is_the_same_file(self):
        support.install(self.data)
        tools = tempfile.TemporaryDirectory()
        self.addCleanup(tools.cleanup)
        folder = Path(os.path.realpath(tools.name)) / "bin"  # setup records the canonical path
        folder.mkdir()
        ffmpeg = folder / ("ffmpeg.exe" if os.name == "nt" else "ffmpeg")
        ffmpeg.write_bytes(b"#!/bin/sh\n")
        ffmpeg.chmod(stat.S_IRWXU)
        found = ffmpeg.stat()
        allowed = {"path": str(ffmpeg), "size": found.st_size, "mtime_ns": found.st_mtime_ns, "ino": found.st_ino}
        self.receipt(tools={"ffmpeg": allowed})

        def argv() -> list[str]:
            return runtime.serve_argv(runtime.installed(self.data, support.VERSION), self.data / "state", "ableton")

        self.assertEqual(argv()[-2:], ["--tool", f"ffmpeg={ffmpeg}"])
        ffmpeg.write_bytes(b"#!/bin/sh\n# upgraded in place\n")  # same path, another program
        self.assertNotIn("--tool", argv())
        for record in ({**allowed, "path": "ffmpeg"}, {**allowed, "size": str(found.st_size)}, "/usr/bin/ffmpeg"):
            with self.subTest(record=record):
                self.receipt(tools={"ffmpeg": record})
                self.assertNotIn("--tool", argv())
        moved = folder.with_name("moved")  # the folder becomes a link to another one with the very same file
        folder.rename(moved)
        try:
            folder.symlink_to(moved, target_is_directory=True)
        except OSError:  # Windows, without the right to make links
            return
        found = (moved / ffmpeg.name).stat()
        self.receipt(tools={"ffmpeg": {**allowed, "size": found.st_size, "mtime_ns": found.st_mtime_ns,
                                       "ino": found.st_ino}})
        self.assertNotIn("--tool", argv(), "an allowed path that now leads through a link is changed")


if __name__ == "__main__":
    unittest.main()
