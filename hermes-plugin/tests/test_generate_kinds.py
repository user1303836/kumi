"""CC-15, W2's side: Mirelo's sound effects for a video, extend and inpaint, against the fixture runtime and W3's ledger:
each kind off until its own switch is on, a quote with only its kind's fields, a file approved for what it's sent for,
and a create approved from the quote's own typed window, never its summary."""
from __future__ import annotations

import unittest

import support
from test_generate import KEY, ON as BEFORE, Paid

generate = support.module("generate")
ledger = support.module("ledger")

ON = {**BEFORE, "mirelo_video_to_sfx_enabled": True, "mirelo_extend_enabled": True, "mirelo_inpaint_enabled": True}
AUDIO = {"sound_id": "s-r1-1", "map_revision": "r1"}  # the fixture's "Dusty Snare 04.wav"
VIDEO = {"action": "quote", "kind": "video_to_sfx", "source": {"artifact_id": "a-video"}, "seconds": 6}
EXTEND = {"action": "quote", "kind": "extend", "source": AUDIO, "append_seconds": 3}
INPAINT = {"action": "quote", "kind": "inpaint", "source": AUDIO, "region_start_seconds": 2, "region_end_seconds": 4}
NEW = {"video_to_sfx": VIDEO, "extend": EXTEND, "inpaint": INPAINT}


class Unwindowed(ledger.Ledger):
    """A ledger whose stored quotes have lost their source's length."""

    def quote(self, quote_id: str) -> dict:
        return {key: value for key, value in super().quote(quote_id).items() if key != "source_seconds"}


class Newer(ledger.Ledger):
    """A ledger a newer Kumi for Hermes wrote: its policy names a kind this plugin doesn't know (W5-d)."""

    def configure(self, *, acknowledged: bool, enabled: dict) -> None:
        raise ledger.LedgerError("ledger_newer_policy")


class Kinds(Paid, unittest.TestCase):
    def host(self, settings: dict | None = None, *fixture_args: str, key: str | None = KEY, ledger: type | None = None):
        return super().host(dict(ON if settings is None else settings), *fixture_args, key=key, ledger=ledger)

    def quoted(self, host, args: dict) -> dict:
        """A new kind's quote, its file's sending approved."""
        return support.call(host, "kumi_generate", args, session_id="s1")

    def test_each_new_kind_is_off_until_its_own_switch_is_on(self):
        for kind, args in NEW.items():
            for settings in ({**ON, generate.SETTINGS[kind]: False}, BEFORE):
                with self.subTest(kind=kind, earlier=settings is BEFORE):
                    host, _, data = self.host(settings)
                    asked = self.hook(host, args)
                    self.assertEqual(asked["action"], "block", "nothing is sent to be quoted")
                    self.assertIn("Paid generation of this kind is off", asked["message"])
                    answer = generate.quote(host, args, host.identity({"session_id": "s1"}), {})
                    self.assertEqual(answer["error"]["code"], "not_enabled")
                    self.assertEqual(self.sent(data, "quote"), [], "Mirelo is never asked")
                    self.assertFalse((data / generate.LEDGER_FILE).exists(), "and no ledger is made")

    def test_a_field_that_isnt_the_kinds_is_refused_never_dropped(self):
        host, _, data = self.host()
        for args, said in (({**EXTEND, "seconds": 10}, "Not seconds"),
                           ({**INPAINT, "append_seconds": 1}, "Not append_seconds"),
                           ({**VIDEO, "append_seconds": 1}, "Not append_seconds"),
                           ({"action": "quote", "kind": "sfx", "prompt": "a hit", "source": AUDIO}, "Not source"),
                           ({**VIDEO, "source": AUDIO}, "never a sound-map id"),
                           ({**INPAINT, "region_end_seconds": 2}, "comes after region_start_seconds"),
                           ({key: value for key, value in EXTEND.items() if key != "append_seconds"},
                            "append_seconds"),
                           ({**INPAINT, "region_start_seconds": -1}, "0 or more")):
            with self.subTest(args=args):
                asked = self.hook(host, args)
                self.assertEqual(asked["action"], "block")
                self.assertIn(said, asked["message"])
                answer = support.module("tools").kumi_generate(host, args, session_id="s1", tool_call_id="c9")
                self.assertEqual(answer["error"]["code"], "invalid_request", "refused at dispatch too")
        self.assertFalse([m for m in support.requests(data / "state") if m.get("params", {}).get("name") == "generate"],
                         "nothing reached the runtime, not even a preview")

    def test_a_file_is_approved_for_what_its_sent_for_and_only_that(self):
        host, _, data = self.host()
        asked = self.hook(host, {"action": "quote", "kind": "audio_to_midi", "source": AUDIO}, "c1")
        self.assertEqual(asked["action"], "approve")
        support.module("policy").post_approval_response(host, tool_call_id="c1", choice="session")
        again = self.hook(host, EXTEND, "c2")
        self.assertEqual(again["action"], "approve", "sending it for MIDI notes never approved extending it")
        self.assertRegex(again["rule_key"], r"^kumi:upload:extend:[0-9a-f]{64}:s1$")
        self.assertIn("send \"Dusty Snare 04.wav\" (480 KB) to Mirelo, to extend it", again["message"])
        self.assertIn("The whole file goes; the original isn't changed.", again["message"])
        video = self.hook(host, VIDEO, "c3")["message"]
        self.assertIn("send the video \"clip.mp4\" (480 KB) to Mirelo, to make sound effects for it", video)
        self.assertIn("its speech isn't kept", video)
        self.assertEqual(self.sent(data, "quote"), [], "only local previews so far")

    def test_a_create_is_approved_from_the_quotes_own_window(self):
        host, _, data = self.host()
        for args, said in ((VIDEO, "From the video \"clip.mp4\" (12 s): sound for its first 6 s; its speech isn't kept"),
                           (EXTEND, "Adds 3 s after the end of \"Dusty Snare 04.wav\" (12 s); a new file, the original "
                                    "kept"),
                           (INPAINT, "Replaces 2–4 s of \"Dusty Snare 04.wav\" (12 s); a new file, the original kept")):
            with self.subTest(kind=args["kind"]):
                quote = self.quoted(host, args)
                self.assertEqual((quote["state"], quote["source_seconds"]), ("quoted", 12.0))
                asked = self.hook(host, {"action": "create", "quote_id": quote["quote_id"]}, f"c-{args['kind']}")
                self.assertEqual(asked["action"], "approve")
                self.assertIn(said, asked["message"])
                self.assertNotIn(quote["summary"], asked["message"], "the runtime's summary is display text only")
        self.assertEqual(self.sent(data, "create"), [], "nothing is created until the producer says yes")

    def test_a_stored_quote_without_its_window_is_refused_before_anyone_is_asked(self):
        host, _, data = self.host(ledger=Unwindowed)
        quote = self.quoted(host, EXTEND)
        asked = self.hook(host, {"action": "create", "quote_id": quote["quote_id"]})
        self.assertEqual(asked["action"], "block")
        self.assertIn("doesn't say plainly what it makes", asked["message"])
        self.assertEqual(host.ledger().snapshot()["reserved"], 0, "nothing is reserved")

    def test_a_ledger_a_newer_plugin_wrote_is_kept_and_said_to_need_this_one_updated(self):
        host, _, data = self.host(ledger=Newer)
        answer = support.call(host, "kumi_generate", {"action": "quote", "kind": "sfx", "prompt": "a hit"},
                              session_id="s1")
        self.assertEqual(answer["error"]["code"], "ledger_newer_policy")
        self.assertIn("hermes plugins update kumi", answer["error"]["reason"])
        self.assertEqual(self.sent(data, "quote"), [], "Mirelo is never asked")


if __name__ == "__main__":
    unittest.main()
