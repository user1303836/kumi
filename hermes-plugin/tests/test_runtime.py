"""runtime.py against the fixture runtime: what setup installed, the scrubbed launch, the check that it's the pinned
runtime, calls and their errors, a runtime that stops or doesn't answer, events, and who stops it."""
from __future__ import annotations

import json
from pathlib import Path
import tempfile
import time
import unittest

import support

runtime = support.module("runtime")


class Runtimes(unittest.TestCase):
    def setUp(self):
        folder = tempfile.TemporaryDirectory()
        self.addCleanup(folder.cleanup)
        self.data = Path(folder.name)
        self.events: list[tuple[str, dict]] = []
        self.runtimes = runtime.Runtimes(events=lambda event, payload: self.events.append((event, payload)),
                                         environ=lambda: {"PATH": "/usr/bin:/bin", "HOME": str(self.data),
                                                          "OPENAI_API_KEY": "sk-secret", "MIRELO_API_KEY": "m-secret",
                                                          "HERMES_HOME": "/elsewhere", "KUMI_HOME": "/elsewhere"},
                                         start_timeout=10)
        self.addCleanup(self.runtimes.stop_all)

    def start(self, *fixture_args: str, session: str = "s1"):
        support.install(self.data, *fixture_args)
        return self.runtimes.start(self.data, runtime.installed(self.data, support.VERSION), "ableton", session)

    def host(self, session: str = "s1") -> dict:
        return {"profile_id": "default", "session_id": session, "tool_call_id": "call-1", "request_id": "r1"}

    def test_a_started_runtime_is_the_pinned_one_and_answers(self):
        live = self.start()
        self.assertEqual(live.info, {"name": "kumi-serve", "version": support.VERSION})
        status = live.call("status", {}, self.host(), timeout=10)
        self.assertEqual(status["runtime"], {"version": support.VERSION, "protocol": "kumi-serve/1"})
        self.assertFalse(status["connected"])
        started = json.loads((self.data / "state" / "fixture-start.json").read_text(encoding="utf-8"))
        self.assertEqual(started["argv"], ["serve", "--stdio", "--state-dir", str(self.data / "state"),
                                           "--kumi-home", str(self.data / "kumi-home"), "--daw", "ableton"])
        self.assertIs(self.runtimes.get(self.data), live)
        self.assertIs(self.runtimes.start(self.data, live.install, "ableton", "s2"), live, "one runtime a profile")

    def test_the_runtime_gets_no_key_and_never_checks_for_updates(self):
        self.start()
        environment = set(json.loads((self.data / "state" / "fixture-start.json").read_text(encoding="utf-8"))["environment"])
        self.assertTrue({"PATH", "HOME", "KUMI_NO_UPDATE_CHECK"} <= environment, environment)
        self.assertFalse(environment & {"OPENAI_API_KEY", "MIRELO_API_KEY", "HERMES_HOME", "KUMI_HOME"}, environment)
        self.assertEqual(runtime.launch_environment({"Path": "C:\\Windows", "SystemRoot": "C:\\Windows",
                                                     "AWS_SECRET_ACCESS_KEY": "x"}, windows=True),
                         {"Path": "C:\\Windows", "SystemRoot": "C:\\Windows", "KUMI_NO_UPDATE_CHECK": "1"})

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

    def test_a_refusal_carries_the_runtimes_code_and_reason(self):
        live = self.start()
        with self.assertRaises(runtime.Refused) as refused:
            live.call("observe", {"scope": "focus"}, self.host(), timeout=10)
        self.assertEqual(refused.exception.as_json(), {"error": {
            "code": "not_connected", "reason": "Kumi isn't connected to the DAW: connect first."}})

    def test_a_runtime_that_stops_mid_call_fails_it_and_nothing_is_sent_again(self):
        live = self.start("--exit-on", "change")
        live.call("connect", {"daw": "ableton"}, self.host(), timeout=10)
        self.events.clear()
        with self.assertRaises(runtime.Unavailable) as stopped:
            live.call("change", {"title": "Louder", "steps": []}, self.host(), timeout=10)
        self.assertEqual(stopped.exception.code, "stopped")
        with self.assertRaises(runtime.Unavailable):
            live.call("status", {}, self.host(), timeout=10)
        changes = [m for m in support.requests(self.data / "state") if m.get("params", {}).get("name") == "change"]
        self.assertEqual(len(changes), 1, "the change was sent once and never again")
        self.assertIsNone(self.runtimes.get(self.data))
        deadline = time.monotonic() + 5
        while ("status.changed", {"seq": None}) not in self.events and time.monotonic() < deadline:
            time.sleep(0.02)
        self.assertIn(("status.changed", {"seq": None}), self.events, "the UIs hear that the runtime stopped")

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

    def test_each_snapshot_the_runtime_updates_becomes_a_desktop_event_with_its_seq(self):
        live = self.start()
        self.events.clear()
        live.call("connect", {"daw": "ableton"}, self.host(), timeout=10)
        deadline = time.monotonic() + 5
        while len(self.events) < 3 and time.monotonic() < deadline:
            time.sleep(0.02)
        seqs = {json.loads((self.data / "state" / f"{name}.json").read_text(encoding="utf-8"))["seq"]
                for name in ("status", "focus", "history")}
        self.assertEqual({event for event, _ in self.events}, {"status.changed", "focus.changed", "history.changed"})
        self.assertEqual({payload["seq"] for _, payload in self.events}, seqs)

    def test_ending_the_chat_that_started_it_stops_it_and_another_chats_end_doesnt(self):
        live = self.start(session="s1")
        self.runtimes.stop_session("s2")
        self.assertIs(self.runtimes.get(self.data), live)
        self.runtimes.stop_session("s1")
        self.assertIsNone(self.runtimes.get(self.data))
        self.assertFalse(live.client.running)


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

    def test_a_receipt_pointing_outside_plugin_data_or_at_nothing_is_never_run(self):
        support.install(self.data)
        for kumi in ("../kumi", "/bin/sh", "runtime/1.24.66/missing"):
            with self.subTest(kumi=kumi):
                self.receipt(kumi=kumi)
                self.assertIn("missing or damaged", self.reason())
        self.receipt(kumi="runtime/1.24.66/kumi", schema=2)
        self.assertIn("another plugin version", self.reason())
        (self.data / "runtime" / "current.json").write_text("{", encoding="utf-8")
        self.assertIn("can't be read", self.reason())

    def test_an_adopted_transport_home_is_used(self):
        support.install(self.data)
        self.receipt(kumi_home="/Users/someone/.kumi")
        self.assertEqual(runtime.installed(self.data, support.VERSION).kumi_home, Path("/Users/someone/.kumi"))


if __name__ == "__main__":
    unittest.main()
