"""kumi_listen on a track or the mix (CONTRACT-CHANGES CC-2, W2's side): asked in the words of the runtime's preview,
blocked unasked when the runtime would refuse it, played once and only as approved, and a listen that didn't succeed
carries no measurements and no MEDIA: line."""
from __future__ import annotations

import json
import unittest
from unittest import mock

import support

policy = support.module("policy")
tools = support.module("tools")

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
        self.assertEqual(self.hook(track()), {"action": "approve", "rule_key": "kumi:playback:listen", "message": (
            "Kumi: play the track Drums of Night Drive from beat 0 to beat 8 (Arrangement) through Kumi Ears and "
            "measure it; routing and the transport change for a moment and are put back (about 3.87 s)")})
        self.assertEqual([args.get("preview") for args in asked(self.data)], [True])
        self.assertEqual(played(self.data), [])

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

    def test_an_answer_that_never_came_is_unsure_and_never_sent_again(self):
        with mock.patch.object(tools, "TRACK_TIMEOUT", 0.5):
            result, data = self.listen("--slow", "listen=2")
        self.assertEqual((result["state"], result["code"], result["cleanup"]), ("unsure", "timeout", "unsure"))
        self.assertEqual(result["hint"], tools.UNSURE_HINT)
        self.assertEqual([args.get("preview") for args in asked(data)], [True, None], "previewed, then sent once")


if __name__ == "__main__":
    unittest.main()
