"""CC-1, W2's side: with `listen_interpretation` on, a measured kumi_listen gets Hermes's listening model's reading of
its numbers, or why there's none, and keeps its measurements either way; off, or for a listen that didn't measure,
no model is asked."""
from __future__ import annotations

from types import SimpleNamespace
import unittest

import support

listening = support.module("listening")
tools = support.module("tools")
SOUND = {"source": {"sound_id": "s-r1-1", "map_revision": "r1"}}


class Model:
    """Hermes's ctx.llm as far as the interpreter uses it: one structured completion, recorded."""

    def __init__(self, error: Exception | None = None):
        self.calls: list[dict] = []
        self.error = error

    def complete_structured(self, **kwargs):
        self.calls.append(kwargs)
        if self.error is not None:
            raise self.error
        return SimpleNamespace(parsed={"text": "The centroid sits low for a snare; the numbers can't say more."},
                               model="vision-model")


class Interpretation(unittest.TestCase):
    def setUp(self):
        self.host, self.ctx, _ = support.host(self)
        self.ctx.llm = self.model = Model()
        support.call(self.host, "kumi_connect", {}, session_id="s1")

    def listen(self, args: dict = SOUND) -> dict:
        return support.call(self.host, "kumi_listen", args, session_id="s1")

    def test_off_by_default_no_model_is_asked_and_the_result_is_the_runtimes(self):
        heard = self.listen()
        self.assertTrue(heard["metrics"]["kumiAudio"])
        self.assertFalse({"interpretation", "interpretation_error"} & set(heard))
        self.assertEqual(self.model.calls, [])

    def test_on_a_measured_listen_gets_the_models_reading_beside_its_measurements(self):
        self.ctx.settings["listen_interpretation"] = True
        heard = self.listen()
        self.assertEqual(heard["metrics"]["loudness"]["integratedLufs"], -14.2, "the measurements stay")
        self.assertTrue(heard["interpretation"]["text"].startswith(
            "Model interpretation of measurements (no raw audio heard):"))
        self.assertEqual(heard["interpretation"]["model"], "vision-model")
        [asked] = self.model.calls
        self.assertEqual((asked["task"], asked["timeout"]), ("kumi_listen", 20.0),
                         "the task's 60 s over Hermes's three attempts")

    def test_a_model_that_fails_leaves_the_measurements_and_says_why(self):
        self.ctx.settings["listen_interpretation"] = True
        self.model.error = TimeoutError()
        heard = self.listen()
        self.assertEqual(heard["metrics"]["loudness"]["integratedLufs"], -14.2)
        self.assertNotIn("interpretation", heard)
        self.assertIn("timed out", heard["interpretation_error"])
        self.assertEqual(len(self.model.calls), 1, "never asked again")

    def test_a_listen_that_didnt_measure_is_never_sent_to_a_model(self):
        host, ctx, _ = support.host(self, {"daw": "ableton", "listen_interpretation": True}, "--listen-outcome",
                                    "unsure-with-media")
        ctx.llm = model = Model()
        support.call(host, "kumi_connect", {}, session_id="s1")
        track = {"source": {"track": {"ref": "t1"}, "from_beat": 0, "beats": 8},
                 "expected_project_key": "live:night-drive", "expected_epoch": "1"}
        heard = support.call(host, "kumi_listen", track, session_id="s1")
        self.assertEqual(heard["state"], "unsure")
        self.assertEqual(model.calls, [])

    def test_the_producers_task_settings_set_each_attempts_time(self):
        hermes = {"auxiliary": {"kumi_listen": {"timeout": 90}, "transient_retries": 5}}
        host, ctx, _ = support.host(self, {"daw": "ableton", "listen_interpretation": True}, hermes_config=hermes)
        ctx.llm = model = Model()
        support.call(host, "kumi_connect", {}, session_id="s1")
        support.call(host, "kumi_listen", SOUND, session_id="s1")
        self.assertEqual(model.calls[-1]["timeout"], 15.0, "90 s over 1 + 5 attempts")


if __name__ == "__main__":
    unittest.main()
