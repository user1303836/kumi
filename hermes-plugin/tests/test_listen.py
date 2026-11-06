"""kumi_listen on a track or the mix (CONTRACT-CHANGES CC-2, W2's side): asked in the words of the runtime's preview,
blocked unasked when the runtime would refuse it, played once and only as approved, and a listen that didn't succeed
carries no measurements and no MEDIA: line."""
from __future__ import annotations

import json
import struct
import unittest
from unittest import mock

import support

policy = support.module("policy")
tools = support.module("tools")
runtime = support.module("runtime")

PROJECT = {"expected_project_key": "live:night-drive", "expected_epoch": "1"}


def track(ref: str = "t1", from_beat=0, beats=8, **fields) -> dict:
    return {"source": {"track": {"ref": ref}, "from_beat": from_beat, "beats": beats}, **PROJECT, **fields}


def played(data) -> list[dict]:
    """The listens the fixture runtime really ran (a preview is never one)."""
    path = data / "state" / "fixture-listens.jsonl"
    return [json.loads(line) for line in path.read_text(encoding="utf-8").splitlines()] if path.exists() else []


def asked(data) -> list[dict]:
    return [m["params"]["arguments"] for m in support.requests(data / "state")
            if m.get("params", {}).get("name") == "listen"]


class TrackListen(unittest.TestCase):
    def setUp(self):
        self.host, self.ctx, self.data = support.host(self)
        support.call(self.host, "kumi_connect", {}, session_id="s1")

    def hook(self, args: dict) -> dict | None:
        return policy.pre_tool_call(self.host, tool_name="kumi_listen", args=args, session_id="s1", tool_call_id="l1",
                                    task_id="t1")

    def test_a_track_listen_is_asked_in_the_runtimes_words_and_nothing_plays_first(self):
        self.assertEqual(self.hook(track()), {"action": "approve", "rule_key": "kumi:playback:listen:s1", "message": (
            "Kumi: play the track Drums of Night Drive from beat 0 to beat 8 (Arrangement) through Kumi Ears and "
            "measure it; routing and the transport change for a moment and are put back (about 3.87 s)\n"
            f"{policy.CHAT_ONLY}")})
        self.assertEqual([args.get("preview") for args in asked(self.data)], [True])
        self.assertEqual(played(self.data), [])

    def test_a_track_listen_is_offered_only_where_the_pinned_runtime_takes_one(self):
        """CC-16: kumi_release.json records the sources each DAW's pinned runtime listens to; a pin without them (from
        before CC-16) listens to files alone. A source the producer's DAW doesn't list is refused before anyone is
        asked or anything plays, in the hook and again at dispatch."""
        files = ["artifact", "file", "sound"]
        sound = {"source": {"sound_id": "s-r1-1", "map_revision": "r1"}}
        for sources in (None, {**support.LISTEN_ALL, "ableton": files}):
            with self.subTest(sources=sources):
                support.pin(self.host.plugin_dir, listen_sources=sources)
                before = len(asked(self.data))
                decision = self.hook(track())
                self.assertEqual(decision["action"], "block")
                self.assertIn("can't listen to a track or the mix in Live", decision["message"])
                answer = tools.kumi_listen(self.host, track(), session_id="s1", tool_call_id="l2")
                self.assertEqual(answer["error"]["code"], "unsupported")
                self.assertEqual(len(asked(self.data)), before, "nothing reached the runtime, not even a preview")
                self.assertIsNone(self.hook(sound), "a file, a sound or an artifact is measured as before")
        support.pin(self.host.plugin_dir, listen_sources={**support.LISTEN_ALL, "ableton": []})
        self.assertIn("can't measure a sound in Live", self.hook(sound)["message"], "a DAW that lists nothing")
        self.ctx.settings["daw"] = "reaper"
        support.pin(self.host.plugin_dir, listen_sources={**support.LISTEN_ALL, "ableton": files})
        self.assertIsNone(tools.listen_unsupported(self.host, track()), "REAPER's runtime takes a track listen")

    def test_the_mix_and_a_window_written_as_text_are_previewed_as_numbers(self):
        args = {"source": {"track": {"mix": "true"}, "from_beat": "4", "beats": "16"}, **PROJECT}
        decision = self.hook(args)
        self.assertIn("play the mix of Night Drive from beat 4 to beat 20", decision["message"])
        self.assertEqual(asked(self.data)[-1]["source"], {"track": {"mix": True}, "from_beat": 4, "beats": 16})

    def test_a_listen_the_runtime_would_refuse_is_blocked_before_anyone_is_asked(self):
        for args, reason in ((track(expected_epoch="0"), "observe it again"), (track("t9"), "no track 't9'"),
                             (track(beats=200), "60 s at most")):
            with self.subTest(reason=reason):
                decision = self.hook(args)
                self.assertEqual(decision["action"], "block")
                self.assertIn(reason, decision["message"])
        unconnected, _, _ = support.host(self)
        decision = policy.pre_tool_call(unconnected, tool_name="kumi_listen", args=track(), session_id="s1")
        self.assertIn("kumi_connect", decision["message"])
        self.assertEqual(played(self.data), [])

    def test_a_preview_without_valid_bounds_isnt_asked_and_nothing_plays(self):
        reference = {"compare_to": {"sound_id": "s-r1-1", "map_revision": "r1"}}
        cases = [*((("--listen-deadline", value), {}) for value in ("0", "-5", "361", "75.5", "true", "null", "missing")),
                 *((("--listen-prepare", value), {}) for value in ("5", "-1", "0.0", "true", "missing")),  # none to get
                 *((("--listen-prepare", value), reference) for value in ("121", "-1", "2.5", "null", "missing")),
                 *((("--listen-estimated", value), {}) for value in ("0", "-1", "61", "NaN", "Infinity", "true",
                                                                     "missing"))]
        for fixture_args, extra in cases:  # CC-2's A2: Kumi waits only for bounds the runtime may give
            with self.subTest(fixture_args=fixture_args, extra=extra):
                host, _, data = support.host(self, None, *fixture_args)
                support.call(host, "kumi_connect", {}, session_id="s1")
                decision = policy.pre_tool_call(host, tool_name="kumi_listen", args=track(**extra), session_id="s1")
                self.assertEqual(decision["action"], "block")
                self.assertEqual(played(data), [])

    def test_the_runtime_gets_the_approved_bounds_privately_and_only_on_the_real_call(self):
        approved = {"estimated_seconds": 3.87, "prepare_seconds": 0, "deadline_seconds": 75}
        forged = {"estimated_seconds": 60, "prepare_seconds": 120, "deadline_seconds": 360}
        heard = support.call(self.host, "kumi_listen", {**track(), "_listen": forged}, session_id="s1")
        self.assertEqual(heard["capture"]["cleanup"], "restored")
        [run] = played(self.data)
        self.assertEqual(run["args"]["_listen"], approved, "the bounds the producer approved, never the model's")
        self.assertEqual(["_listen" in args for args in asked(self.data)], [False, True], "a preview carries none")

    def test_the_windows_seconds_reach_the_runtime_exactly_as_its_preview_gave_them(self):
        # The runtime renders only if the window's seconds are at most the approved ones, compared exactly (#1035):
        # nothing between the preview and `_listen` may round or format them.
        seconds = 2.0000000000000004  # the float just above 2.0, which any rounding would turn into 2.0
        host, _, data = support.host(self, None, "--listen-estimated", repr(seconds))
        support.call(host, "kumi_connect", {}, session_id="s1")
        self.assertEqual(support.call(host, "kumi_listen", track(), session_id="s1")["capture"]["cleanup"], "restored")
        [run] = played(data)
        self.assertEqual(struct.pack("<d", run["args"]["_listen"]["estimated_seconds"]), struct.pack("<d", seconds))

    def test_a_window_or_source_kumi_doesnt_take_is_blocked_without_asking_the_runtime(self):
        for args in (track(beats=0), track(beats=257), track(beats=float("nan")), track(from_beat=-1),
                     track(from_beat="soon"), {**track(), "expected_epoch": None},
                     {"source": {"track": {"ref": "t1", "mix": True}, "from_beat": 0, "beats": 8}, **PROJECT},
                     {"source": {"track": "Drums", "from_beat": 0, "beats": 8}, **PROJECT},
                     {"source": {"track": {"ref": "t1"}, "from_beat": 0, "beats": 8, "loop": True}, **PROJECT},
                     {"source": {"sound_id": "s-r1-1", "map_revision": "r1"}, "compare_to": track()["source"]}):
            with self.subTest(args=args):
                self.assertEqual(self.hook(args)["action"], "block")
        self.assertEqual(asked(self.data), [])

    def test_an_approved_listen_plays_once_as_approved_and_returns_its_capture(self):
        heard = support.call(self.host, "kumi_listen", {**track(), "preview": True}, session_id="s1")
        self.assertEqual(heard["capture"], {"method": "ears", "from_beat": 0, "beats": 8, "seconds": 3.87,
                                            "cleanup": "restored"})
        self.assertTrue(heard["media_tag"].startswith("MEDIA:"))
        [run] = played(self.data)
        self.assertNotIn("preview", run["args"], "a model's preview never reaches the runtime")
        self.assertTrue(run["host"]["tool_call_id"].startswith("call-"))

    def test_a_listen_that_skipped_the_hook_or_was_denied_never_plays(self):
        self.assertEqual(support.handle(self.host, "kumi_listen", track(), session_id="s1")["error"]["code"],
                         "not_approved")
        self.assertIn("blocked", support.call(self.host, "kumi_listen", track(), answer="deny", session_id="s1"))
        self.assertEqual(support.handle(self.host, "kumi_listen", track(), session_id="s1")["error"]["code"],
                         "not_approved", "a denied approval leaves nothing a later dispatch could use")
        self.assertEqual(played(self.data), [])


class Outcomes(unittest.TestCase):
    """What comes back when a listen didn't end measured: as the runtime said it, without measurements or media."""

    def listen(self, *fixture_args: str) -> tuple[dict, object]:
        host, _, data = support.host(self, None, *fixture_args)
        support.call(host, "kumi_connect", {}, session_id="s1")
        return support.call(host, "kumi_listen", track(), session_id="s1"), data

    def test_refused_cancelled_and_unsure_pass_through_with_no_measurements_or_media(self):
        for outcome, state in (("refused", "refused"), ("cancelled", "cancelled"), ("unsure-with-media", "unsure")):
            with self.subTest(outcome=outcome):
                result, data = self.listen("--listen-outcome", outcome)
                self.assertEqual(result["state"], state)
                self.assertNotIn("MEDIA:", json.dumps(result))
                self.assertFalse({"metrics", "artifact_id", "verdict_text", "capture"} & set(result), result)
                self.assertEqual(len(played(data)), 1)
                if state == "unsure":
                    self.assertEqual((result["cleanup"], result["hint"]), ("unsure", tools.UNSURE_HINT))

    def test_an_answer_past_the_runtimes_own_deadline_is_unsure_and_never_sent_again(self):
        with mock.patch.object(runtime, "TRACK_MARGIN", 0):  # the preview says 1 s; the answer takes 2 s
            result, data = self.listen("--slow", "listen=2", "--listen-deadline", "1")
        self.assertEqual((result["state"], result["code"], result["cleanup"]), ("unsure", "timeout", "unsure"))
        self.assertEqual(result["hint"], tools.UNSURE_HINT)
        self.assertEqual([args.get("preview") for args in asked(data)], [True, None], "previewed, then sent once")

    def test_a_slow_listen_within_the_runtimes_deadline_is_waited_for(self):
        result, _ = self.listen("--slow", "listen=1", "--listen-deadline", "5")
        self.assertEqual(result["capture"]["cleanup"], "restored")

    def test_getting_a_reference_ready_has_its_own_time_beside_the_runtimes_deadline(self):
        reference = {"compare_to": {"sound_id": "s-r1-1", "map_revision": "r1"}}
        host, _, _ = support.host(self, None, "--slow", "listen=2", "--listen-deadline", "1", "--listen-prepare", "4")
        support.call(host, "kumi_connect", {}, session_id="s1")
        with mock.patch.object(runtime, "TRACK_MARGIN", 0):  # 4 s to prepare, then 1 s for the native call
            result = support.call(host, "kumi_listen", track(**reference), session_id="s1")
        self.assertEqual(result["capture"]["cleanup"], "restored", "a 2 s answer is waited for, not cut off at 1 s")

    def test_a_track_listen_recorded_without_its_runtimes_deadline_never_plays(self):
        host, _, data = support.host(self)
        support.call(host, "kumi_connect", {}, session_id="s1")
        host.calls.record("kumi_listen", track(), {"session_id": "s1", "tool_call_id": "l9"}, {"approved": True})
        refused = support.handle(host, "kumi_listen", track(), session_id="s1")
        self.assertEqual(refused["error"]["code"], "not_approved", "no deadline is ever guessed")
        self.assertEqual(played(data), [])


if __name__ == "__main__":
    unittest.main()
