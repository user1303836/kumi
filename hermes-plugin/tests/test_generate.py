"""Paid generation with Mirelo, W2's side (CC-13 and its amendment A), against the fixture runtime's `generate` and a
stand-in for W3's ledger: off until the producer switches it on, a quote then a create approved from the stored quote
alone, once, within the cap, never where nobody can answer, and the key nowhere but the one request."""
from __future__ import annotations

import io
import json
import logging
import os
from pathlib import Path
import unittest
from unittest import mock

import fake_ledger
import support

commands = support.module("commands")
generate = support.module("generate")
policy = support.module("policy")
tools = support.module("tools")

KEY = "mk-SENTINEL-4f2a9c"  # a key that must turn up nowhere but in the request that carries it
ON = {"daw": "ableton", "mirelo_acknowledged": True, "mirelo_sfx_enabled": True, "mirelo_midi_enabled": True,
      "mirelo_credit_cap": 100}
SFX = {"action": "quote", "kind": "sfx", "prompt": "a short metallic impact"}


class Tty(io.StringIO):
    def isatty(self) -> bool:
        return True


class Generation(unittest.TestCase):
    def host(self, settings: dict | None = None, *fixture_args: str, key: str | None = KEY):
        host, ctx, data = support.host(self, dict(ON if settings is None else settings), *fixture_args,
                                       ledger=fake_ledger.Ledger, mirelo_key=key)
        support.call(host, "kumi_connect", {}, session_id="s1")
        return host, ctx, data

    def sent(self, data: Path, action: str) -> list[dict]:
        """The runtime's `generate` calls of `action`, the approval hook's previews left out."""
        return [m["params"]["arguments"] for m in support.requests(data / "state")
                if m.get("params", {}).get("name") == "generate" and m["params"]["arguments"].get("action") == action
                and not m["params"]["arguments"].get("preview")]

    def hook(self, host, args: dict, call: str = "c1") -> dict | None:
        return policy.pre_tool_call(host, tool_name="kumi_generate", args=args, session_id="s1", tool_call_id=call)

    def test_paid_generation_is_off_until_the_producer_switches_it_on_and_holds_a_key(self):
        for settings, key, code in (({"daw": "ableton"}, KEY, "not_enabled"),
                                    ({**ON, "mirelo_sfx_enabled": False}, KEY, "not_enabled"),
                                    (ON, None, "not_configured")):
            with self.subTest(code=code, settings=settings):
                host, _, data = self.host(settings, key=key)
                self.assertEqual(support.call(host, "kumi_generate", SFX, session_id="s1")["error"]["code"], code)
                self.assertEqual(self.sent(data, "quote"), [], "Mirelo is never asked")

    def test_a_sound_effect_is_quoted_unasked_then_created_only_as_approved_from_its_stored_quote(self):
        host, _, data = self.host()
        self.assertIsNone(self.hook(host, SFX), "an estimate costs nothing: no approval")
        quoted = support.call(host, "kumi_generate", SFX, session_id="s1")
        self.assertEqual((quoted["state"], quoted["ledger"]["left"]), ("quoted", 100), quoted)
        create = {"action": "create", "quote_id": quoted["quote_id"]}
        asked = self.hook(host, create, "c2")
        self.assertEqual(asked["rule_key"], f"kumi:paid:{quoted['quote_id']}", "the quote's own key, in no chat's")
        self.assertIn(SFX["prompt"], asked["message"])
        self.assertIn(str(quoted["estimate"]["max_credits"]), asked["message"])
        running = support.call(host, "kumi_generate", create, session_id="s1")
        self.assertEqual((running["state"], running["ledger"]["reserved"]), ("running", 15), running)
        [dispatched] = self.sent(data, "create")
        self.assertEqual(dispatched["_mirelo"], {"api_key": "<given>", "acknowledged": True, "enabled": True,
                                                 "reservation_id": "r-1", "reserved_credits": 15})
        done = support.call(host, "kumi_job", {"job_id": running["job_id"]}, session_id="s1")
        self.assertEqual(done["state"], "done")
        self.assertEqual({key: done["result"]["ledger"][key] for key in ("spent", "reserved", "left")},
                         {"spent": 12, "reserved": 0, "left": 88}, "settled by its reservation, as charged")
        self.assertTrue(done["result"]["media_tag"].startswith("MEDIA:"), "its kept file, for the chat")

    def test_a_create_runs_only_the_quote_approved_as_approved_and_only_once(self):
        host, _, data = self.host()
        quote_id = support.call(host, "kumi_generate", SFX, session_id="s1")["quote_id"]
        self.assertEqual(self.hook(host, {"action": "create", "quote_id": quote_id, "prompt": "louder"})["action"],
                         "block", "a create can't carry other words than the quote's")
        self.assertEqual(self.hook(host, {"action": "create", "quote_id": quote_id}, "c2")["action"], "approve")
        policy.post_approval_response(host, tool_call_id="c2", choice="once")
        book = fake_ledger.Ledger.books[str(data)]
        book["quotes"][quote_id]["request_sha256"] = "0" * 64  # the stored quote changed after the approval
        stale = support.handle(host, "kumi_generate", {"action": "create", "quote_id": quote_id}, session_id="s1")
        self.assertEqual((stale["error"]["code"], stale["ledger"]["reserved"]), ("stale", 0), "nothing reserved")
        self.assertEqual(self.sent(data, "create"), [], "and nothing sent")

        quote_id = support.call(host, "kumi_generate", SFX, session_id="s1")["quote_id"]
        support.call(host, "kumi_generate", {"action": "create", "quote_id": quote_id}, session_id="s1")
        again = self.hook(host, {"action": "create", "quote_id": quote_id}, "c4")
        self.assertEqual(again["action"], "block", "a used quote is never asked again")
        self.assertEqual(len(self.sent(data, "create")), 1)

    def test_the_cap_a_freeze_and_a_turn_with_nobody_in_it_refuse_before_anyone_is_asked(self):
        for settings in ({**ON, "mirelo_credit_cap": 0}, {**ON, "mirelo_credit_cap": 10}):
            with self.subTest(cap=settings["mirelo_credit_cap"]):
                host, _, _ = self.host(settings)
                quote_id = support.call(host, "kumi_generate", SFX, session_id="s1")["quote_id"]
                self.assertEqual(self.hook(host, {"action": "create", "quote_id": quote_id})["action"], "block")
        host, ctx, data = self.host()
        quote_id = support.call(host, "kumi_generate", SFX, session_id="s1")["quote_id"]
        fake_ledger.Ledger.books[str(data)]["frozen"] = True
        self.assertEqual(self.hook(host, {"action": "create", "quote_id": quote_id})["action"], "block", "frozen")
        fake_ledger.Ledger.books[str(data)]["frozen"] = False
        for surface in ("webhook", "api_server"):
            with self.subTest(surface=surface):
                ctx.surface = surface
                self.assertEqual(self.hook(host, {"action": "create", "quote_id": quote_id})["message"],
                                 policy.UNATTENDED)
                self.assertIsNone(self.hook(host, SFX), "a free estimate may still be read there")
        ctx.surface, ctx.cron_execution = "tui", object()
        self.assertEqual(self.hook(host, SFX)["message"], tools.SCHEDULED)

    def test_only_a_native_not_submitted_frees_the_reservation_and_a_lost_answer_keeps_it(self):
        host, _, _ = self.host(None, "--generate-create", "not_submitted")
        quote_id = support.call(host, "kumi_generate", SFX, session_id="s1")["quote_id"]
        refused = support.call(host, "kumi_generate", {"action": "create", "quote_id": quote_id}, session_id="s1")
        self.assertEqual((refused["error"]["code"], refused["ledger"]["reserved"]), ("not_submitted", 0), refused)
        host, _, data = self.host(None, "--slow", "generate=2")
        quote_id = support.call(host, "kumi_generate", SFX, session_id="s1")["quote_id"]
        with mock.patch.object(generate, "TIMEOUT", 0.3):
            unsure = support.call(host, "kumi_generate", {"action": "create", "quote_id": quote_id}, session_id="s1")
        self.assertEqual((unsure["state"], unsure["ledger"]["reserved"]), ("unsure", 15), "kept, never sent again")

    def test_sending_audio_to_mirelo_is_approved_first_from_the_runtimes_own_description_of_it(self):
        host, _, data = self.host()
        midi = {"action": "quote", "kind": "audio_to_midi", "source": {"sound_id": "s-r1-1", "map_revision": "r1"}}
        asked = self.hook(host, midi)
        self.assertEqual(asked["action"], "approve")
        self.assertRegex(asked["rule_key"], r"^kumi:upload:[0-9a-f]{64}:s1$", "the file's own key, in this chat")
        self.assertIn("Dusty Snare 04.wav", asked["message"])
        self.assertEqual(self.sent(data, "quote"), [], "only the local preview so far")
        quoted = support.call(host, "kumi_generate", midi, session_id="s1")
        self.assertEqual((quoted["state"], quoted["upload"], quoted["file"]), ("quoted", True, "Dusty Snare 04.wav"))

    def test_the_key_turns_up_nowhere_but_in_the_requests_that_carry_it(self):
        events: list = []
        records: list = []
        catcher = logging.Handler(logging.DEBUG)
        catcher.emit = records.append
        logging.getLogger().addHandler(catcher)
        self.addCleanup(logging.getLogger().removeHandler, catcher)
        with mock.patch.object(tools, "broadcast", side_effect=lambda event, payload: events.append(payload)):
            host, _, data = self.host()
            answers = [support.call(host, "kumi_generate", SFX, session_id="s1")]
            answers.append(support.call(host, "kumi_generate", {"action": "create", "quote_id": answers[0]["quote_id"]},
                                        session_id="s1"))
            answers.append(support.call(host, "kumi_job", {"job_id": answers[1]["job_id"]}, session_id="s1"))
            for tool, answer in zip(("kumi_generate", "kumi_generate", "kumi_job"), answers):
                policy.post_tool_call(host, tool_name=tool, tool_call_id="c", result=json.dumps(answer), status="ok",
                                      session_id="s1")
            host.runtimes.stop_all()
        self.assertEqual(len(events), 3, "a card for the quote, the create and the job")
        said = [json.dumps(answers), json.dumps(events), *(record.getMessage() for record in records)]
        for folder, _, files in os.walk(data):
            said += [Path(folder, name).read_text(encoding="utf-8", errors="replace") for name in files]
        self.assertFalse([text for text in said if KEY in text], "nowhere: answers, events, logs, the state folder")

    def test_hermes_kumi_credits_shows_the_ledger_and_changes_it_only_after_a_yes_in_a_terminal(self):
        host, _, data = self.host()
        host.ledger().book["frozen"] = True  # a charge came in above its estimate
        out = io.StringIO()
        self.assertEqual(commands.cli_credits(host, None, stdout=out), 0)
        self.assertIn("100", out.getvalue())
        self.assertEqual(commands.cli_credits(host, "unfreeze", stdin=io.StringIO("y\n"), stdout=io.StringIO()), 1,
                         "no terminal: nothing changes")
        self.assertEqual(commands.cli_credits(host, "unfreeze", stdin=Tty("n\n"), stdout=Tty()), 1)
        self.assertTrue(fake_ledger.Ledger.books[str(data)]["frozen"])
        self.assertEqual(commands.cli_credits(host, "unfreeze", stdin=Tty("y\n"), stdout=Tty()), 0)
        self.assertFalse(fake_ledger.Ledger.books[str(data)]["frozen"])
        self.assertFalse([name for name, _, _, _ in tools.TOOLS if "credit" in name], "no tool reaches them")

    def test_a_generation_card_leaves_out_midis_notes(self):
        card = generate.card({"job_id": "g-1", "kind": "generate", "state": "done",
                              "result": {"kind": "audio_to_midi", "note_count": 2, "notes": [{"pitch": 60}] * 2}})
        self.assertEqual(card["result"], {"kind": "audio_to_midi", "note_count": 2})


if __name__ == "__main__":
    unittest.main()
