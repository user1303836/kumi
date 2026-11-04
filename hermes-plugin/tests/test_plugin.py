"""The plugin as Hermes sees it: what register() registers (and that it does nothing else), and kumi_status and
kumi_connect through their handlers, against the fixture runtime."""
from __future__ import annotations

import json
from pathlib import Path
import re
import tempfile
import unittest

import support

kumi = support.plugin()
runtime = support.module("runtime")
tools = support.module("tools")


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
        self.assertEqual({tool["toolset"] for tool in ctx.tools.values()}, {"kumi"})
        self.assertEqual(len(ctx.unloads), 1)


class Tools(unittest.TestCase):
    def setUp(self):
        folder = tempfile.TemporaryDirectory()
        self.addCleanup(folder.cleanup)
        self.data = Path(folder.name)
        self.ctx = support.Context({"daw": "ableton"})
        self.runtimes = runtime.Runtimes(environ=lambda: {"PATH": "/usr/bin:/bin"}, start_timeout=10)
        self.addCleanup(self.runtimes.stop_all)
        self.host = tools.Host(self.ctx, self.runtimes, data_dir=lambda: self.data, pinned=lambda: support.VERSION)

    def call(self, name: str, args=None, **context) -> dict:
        function = next(function for tool, _, function, _ in tools.TOOLS if tool == name)
        answer = tools.handler(function, self.host)(args, **context)
        self.assertIsInstance(answer, str)
        return json.loads(answer)

    def test_status_before_setup_says_so_and_starts_nothing(self):
        status = self.call("kumi_status", {}, session_id="s1")
        self.assertEqual(status["setup"], {"installed": False, "message": "Kumi isn't set up in this Hermes profile yet."})
        self.assertEqual((status["connected"], status["daw"], status["runtime"]), (False, "ableton", None))
        support.install(self.data)
        status = self.call("kumi_status", {}, session_id="s1")
        self.assertEqual(status["setup"]["installed"], True)
        self.assertIn("kumi_connect starts it", status["message"])
        self.assertIsNone(self.runtimes.get(self.data), "status never starts the runtime")

    def test_connect_starts_this_chats_runtime_and_status_then_reads_it(self):
        support.install(self.data)
        connected = self.call("kumi_connect", {}, session_id="s1", tool_call_id="call-1")
        self.assertTrue(connected["connected"])
        self.assertEqual(connected["project"], {"name": "Night Drive", "key": "live:night-drive", "epoch": "1"})
        self.assertEqual(connected["owner"]["session_id"], "s1")
        status = self.call("kumi_status", {}, session_id="s2")
        self.assertEqual((status["connected"], status["project"]["name"], status["runtime"]["protocol"]),
                         (True, "Night Drive", "kumi-serve/1"))
        received = support.requests(self.data / "state")
        host = next(m["params"]["arguments"]["_host"] for m in received if m.get("params", {}).get("name") == "connect")
        self.assertEqual({key: host[key] for key in ("profile_id", "session_id", "tool_call_id")},
                         {"profile_id": "default", "session_id": "s1", "tool_call_id": "call-1"})
        self.host.on_session_finalize(session_id="s1", platform="cli")
        self.assertIsNone(self.runtimes.get(self.data), "the chat that started it ended")

    def test_connect_to_another_daw_than_the_runtimes_is_refused(self):
        support.install(self.data)
        self.call("kumi_connect", {"daw": "ableton"}, session_id="s1")
        refused = self.call("kumi_connect", {"daw": "reaper"}, session_id="s1")
        self.assertEqual(refused["error"]["code"], "other_daw")
        self.assertIn("Live", refused["error"]["reason"])

    def test_every_tool_answers_json_whatever_it_gets(self):
        for name, _, _, _ in tools.TOOLS:
            for args in (None, [], {"daw": "logic"}, {"unknown": 1}):
                with self.subTest(tool=name, args=args):
                    answer = self.call(name, args, session_id="s1")
                    self.assertTrue("error" in answer or "setup" in answer, answer)
        self.assertEqual(self.call("kumi_connect", {"daw": "logic"})["error"]["code"], "invalid")
        self.assertEqual(self.call("kumi_connect", {})["error"]["code"], "setup_needed")


if __name__ == "__main__":
    unittest.main()
