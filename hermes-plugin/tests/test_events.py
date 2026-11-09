"""CC-9: a sound search or a listen that went well reaches Desktop as a plugin event after Hermes has its result, with
the profile, chat and call it belongs to, and only what the model's copy holds; nothing for anything else."""
from __future__ import annotations

import json
import unittest
from unittest import mock

import support

policy = support.module("policy")
tools = support.module("tools")


class Announce(unittest.TestCase):
    def setUp(self):
        self.host, self.ctx, _ = support.host(self)
        self.ctx.profile_name = "studio"
        support.call(self.host, "kumi_connect", {}, session_id="s1")
        self.sent: list[tuple[str, dict]] = []
        patcher = mock.patch.object(tools, "broadcast", lambda event, payload: self.sent.append((event, payload)))
        patcher.start()
        self.addCleanup(patcher.stop)

    def ended(self, tool: str, result: str, status: str = "ok") -> None:
        """Hermes's post_tool_call for call c1 of chat s1, with what the handler returned."""
        policy.post_tool_call(self.host, tool_name=tool, args={}, result=result, tool_call_id="c1", session_id="s1",
                              task_id="t1", status=status, duration_ms=5)

    def handled(self, tool: str, args: dict) -> str:
        function = next(function for name, _, function, _ in tools.TOOLS if name == tool)
        return tools.handler(tool, function, self.host)(args, session_id="s1")

    def test_a_search_that_found_sounds_sends_their_ids_reasons_and_closeness_never_a_name_or_path(self):
        self.ended("kumi_sounds", self.handled("kumi_sounds", {"query": "snare"}))
        why = ["name matches 'snare'"]
        self.assertEqual(self.sent, [("sounds.found", {
            "profile": "studio", "session_id": "s1", "tool_call_id": "c1", "map_revision": "r1",
            "sound_ids": ["s-r1-1", "s-r1-2"], "indexed": True,
            "matches": [{"sound_id": "s-r1-1", "why": why}, {"sound_id": "s-r1-2", "why": why}]})])
        self.sent.clear()
        self.ended("kumi_sounds", self.handled("kumi_sounds", {"query": "snare", "near": "s-r1-2"}))
        [(_, near)] = self.sent
        self.assertEqual((near["near"], near["matches"]), ("s-r1-2", [{"sound_id": "s-r1-1", "why": why,
                                                                       "closeness": 87}]), "the seed apart")
        self.sent.clear()
        self.ended("kumi_sounds", self.handled("kumi_sounds", {"query": "", "near": ["s-r1-2", "s-r1-3"]}))
        [(_, several)] = self.sent
        self.assertEqual(several["near"], ["s-r1-2", "s-r1-3"], "more like these: the picks as given")
        self.assertNotIn("s-r1-2", several["sound_ids"], "the picks apart")
        self.sent.clear()
        # (reasons past serve's bounds, 4 of at most 60 characters: they go first, the ids and closeness stay)
        hits = [{"sound_id": f"s-r1-{n}", "why": ["x" * 2000] * 4, "closeness": 50} for n in range(250)]
        self.ended("kumi_sounds", json.dumps({"map_revision": "r1", "indexed": True, "items": hits}))
        [(_, big)] = self.sent
        self.assertEqual((len(big["sound_ids"]), big["matches"][0]), (tools.SOUNDS_FOUND,
                                                                     {"sound_id": "s-r1-0", "closeness": 50}))
        self.assertLessEqual(len(json.dumps(big).encode("utf-8")), tools.LISTEN_EVENT_BYTES)

    def test_a_measured_listen_sends_the_result_the_model_got(self):
        result = self.handled("kumi_listen", {"source": {"sound_id": "s-r1-1", "map_revision": "r1"}})
        self.ended("kumi_listen", result)
        [(event, payload)] = self.sent
        self.assertEqual((event, payload["profile"], payload["tool_call_id"]), ("listen.done", "studio", "c1"))
        self.assertEqual(payload["result"], json.loads(result))

    def test_a_listen_too_big_to_send_goes_without_its_result(self):
        big = json.dumps({"metrics": {"lufsIntegrated": -14.0}, "verdict_text": "x" * tools.LISTEN_EVENT_BYTES})
        self.ended("kumi_listen", big)
        self.assertEqual(self.sent, [("listen.done", {"profile": "studio", "session_id": "s1", "tool_call_id": "c1"})])

    def test_nothing_is_sent_for_a_failure_a_block_an_empty_search_or_a_listen_that_didnt_measure(self):
        for tool, result, status in (
                ("kumi_sounds", json.dumps({"map_revision": "r1", "items": []}), "ok"),
                ("kumi_sounds", json.dumps({"error": {"code": "not_connected", "reason": "…"}}), "error"),
                ("kumi_sounds", self.handled("kumi_sounds", {"query": "snare"}), "blocked"),
                ("kumi_listen", json.dumps({"state": "unsure", "code": "timeout", "cleanup": "unsure"}), "ok"),
                ("kumi_listen", json.dumps({"state": "refused", "code": "playing", "metrics": {"lufs": -9}}), "ok"),
                ("kumi_listen", "not json", "ok"),
                ("kumi_change", json.dumps({"state": "applied"}), "ok")):
            with self.subTest(tool=tool, result=result[:40], status=status):
                self.ended(tool, result, status)
        self.assertEqual(self.sent, [])

    def test_a_broadcast_that_fails_never_fails_the_call(self):
        with mock.patch.object(tools, "broadcast", side_effect=RuntimeError("no Desktop")), \
                self.assertLogs(tools.log, "ERROR"):
            self.ended("kumi_sounds", self.handled("kumi_sounds", {"query": "snare"}))


class ThroughHermes(unittest.TestCase):
    """Hermes's own event door (v0.22.7, `hermes_cli.plugin_events`): the names pass its checks and reach the gateway's
    global event stream, which stands in here for a Desktop client."""

    def test_the_events_pass_hermess_door_by_their_plugin_names(self):
        try:
            from hermes_cli import plugin_events  # only where Hermes is importable (CI's Hermes job)
        except ImportError:
            self.skipTest("Hermes isn't importable here")
        host, _, _ = support.host(self)
        support.call(host, "kumi_connect", {}, session_id="s1")
        stream = mock.Mock()
        handled = {name: tools.handler(name, function, host) for name, _, function, _ in tools.TOOLS}
        with mock.patch.dict("sys.modules", {"tui_gateway.server": mock.Mock(_broadcast_global_event=stream)}):
            for tool, args in (("kumi_sounds", {"query": "snare"}),
                               ("kumi_listen", {"source": {"sound_id": "s-r1-1", "map_revision": "r1"}})):
                policy.post_tool_call(host, tool_name=tool, result=handled[tool](args, session_id="s1"),
                                      tool_call_id="c1", session_id="s1", status="ok")
        self.assertEqual([call.args[0] for call in stream.call_args_list],
                         [plugin_events.plugin_event_name("kumi", "sounds.found"), "plugin.kumi.listen.done"])


if __name__ == "__main__":
    unittest.main()
