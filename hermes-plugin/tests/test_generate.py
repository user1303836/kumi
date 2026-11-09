"""Paid generation with Mirelo, W2's side (CC-13 and its amendment A), against the fixture runtime's `generate` and
W3's ledger (ledger.py), set up only through the ledger's own calls: off until the producer switches it on, a quote
then a create approved from the stored quote alone, once, within the cap, never where nobody can answer, credits held
only for a create that may have left, and the key nowhere but the one request."""
from __future__ import annotations

import io
import json
import logging
import os
from pathlib import Path
import signal
import sys
import time
import types
import unittest
from unittest import mock

import support

calls = support.module("calls")
commands = support.module("commands")
generate = support.module("generate")
ledger = support.module("ledger")
policy = support.module("policy")
tools = support.module("tools")

KEY = "mk-SENTINEL-4f2a9c"  # a key that must turn up nowhere but in the request that carries it
ON = {"daw": "ableton", "mirelo_acknowledged": True, "mirelo_sfx_enabled": True, "mirelo_midi_enabled": True,
      "mirelo_credit_cap": 100}
SFX = {"action": "quote", "kind": "sfx", "prompt": "a short metallic impact"}
MIDI = {"action": "quote", "kind": "audio_to_midi", "source": {"sound_id": "s-r1-1", "map_revision": "r1"}}


class Tty(io.StringIO):
    def isatty(self) -> bool:
        return True


class Unfreeable(ledger.Ledger):
    """A ledger whose lock is busy just when a reservation is to be freed."""

    def refuse(self, reservation_id: str, *, not_submitted: bool, reason: str) -> dict:
        raise ledger.LedgerError("ledger_unavailable", "Kumi's credit ledger is busy.")


class MireloKey(unittest.TestCase):
    """The key Kumi sends is the active profile's, as Hermes's secret scope resolves it (catalog rule 11): never what
    the process environment holds, which can be another profile's, and none at all when Hermes refuses a read made
    without a profile scope."""

    def key_through(self, get_secret) -> str | None:
        host, _, _ = support.host(self, dict(ON))
        host._mirelo_key = None  # as a real host has it: the key comes from Hermes
        scope = types.ModuleType("agent.secret_scope")
        scope.get_secret = get_secret
        with mock.patch.dict(sys.modules, {"agent.secret_scope": scope}), \
                mock.patch.dict(os.environ, {generate.KEY_ENV: "mk-ANOTHER-PROFILE"}):
            return host.mirelo_key()

    def test_the_key_is_the_active_profiles_and_none_when_hermes_refuses_an_unscoped_read(self) -> None:
        self.assertEqual(self.key_through(lambda name, default=None: KEY if name == generate.KEY_ENV else default), KEY)

        def unscoped(name, default=None):
            raise RuntimeError("get_secret called with no profile secret scope active while multiplexing is on")

        self.assertIsNone(self.key_through(unscoped), "no key, never the environment's")
        self.assertIsNone(self.key_through(lambda name, default=None: "  "), "a blank secret is no key")


class Generation(unittest.TestCase):
    def host(self, settings: dict | None = None, *fixture_args: str, key: str | None = KEY, ledger: type | None = None):
        host, ctx, data = support.host(self, dict(ON if settings is None else settings), *fixture_args,
                                       ledger=ledger, mirelo_key=key)
        support.call(host, "kumi_connect", {}, session_id="s1")
        return host, ctx, data

    def sent(self, data: Path, action: str) -> list[dict]:
        """The runtime's `generate` calls of `action`, the approval hook's previews left out."""
        return [m["params"]["arguments"] for m in support.requests(data / "state")
                if m.get("params", {}).get("name") == "generate" and m["params"]["arguments"].get("action") == action
                and not m["params"]["arguments"].get("preview")]

    def hook(self, host, args: dict, call: str = "c1") -> dict | None:
        return policy.pre_tool_call(host, tool_name="kumi_generate", args=args, session_id="s1", tool_call_id=call)

    def approved(self, host, args: dict, call: str) -> None:
        """The hook asks, and the producer allows it once."""
        self.assertEqual(self.hook(host, args, call)["action"], "approve")
        policy.post_approval_response(host, tool_call_id=call, choice="once")

    def generation(self, host) -> dict:
        """A sound effect quoted, created as approved and followed to its end: its job's answer."""
        quote_id = support.call(host, "kumi_generate", SFX, session_id="s1")["quote_id"]
        running = support.call(host, "kumi_generate", {"action": "create", "quote_id": quote_id}, session_id="s1")
        return support.call(host, "kumi_job", {"job_id": running["job_id"]}, session_id="s1")

    def test_paid_generation_is_off_until_the_producer_switches_it_on_and_holds_a_key(self):
        for settings, key, code in (({"daw": "ableton"}, KEY, "not_enabled"),
                                    ({**ON, "mirelo_sfx_enabled": False}, KEY, "not_enabled"),
                                    (ON, None, "not_configured")):
            with self.subTest(code=code, settings=settings):
                host, _, data = self.host(settings, key=key)
                self.assertEqual(support.call(host, "kumi_generate", SFX, session_id="s1")["error"]["code"], code)
                self.assertEqual(self.sent(data, "quote"), [], "Mirelo is never asked")
                self.assertFalse((data / generate.LEDGER_FILE).exists(), "and no ledger is made")

    def test_mirelos_readiness_says_what_is_set_never_the_key_and_opens_no_ledger(self):
        host, ctx, data = self.host()
        ready = host.mirelo_status()
        self.assertEqual(ready, {"key": True, "acknowledged": True, "task": True, "cap": True})
        self.assertNotIn(KEY, json.dumps(ready), "the key itself is never given")
        ctx.settings.update(mirelo_sfx_enabled=False, mirelo_midi_enabled=False, mirelo_credit_cap=0)
        self.assertEqual(host.mirelo_status(), {"key": True, "acknowledged": True, "task": False, "cap": False})
        bare, _, _ = self.host({"daw": "ableton"}, key=None)
        self.assertEqual(bare.mirelo_status(), {"key": False, "acknowledged": False, "task": False, "cap": False})
        self.assertFalse((data / generate.LEDGER_FILE).exists(), "read only: the ledger isn't opened, or made")

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
        [held] = host.ledger().reservations()
        self.assertEqual(dispatched["_mirelo"], {"api_key": "<given>", "acknowledged": True, "enabled": True,
                                                 "reservation_id": held["reservation_id"], "reserved_credits": 15})
        done = support.call(host, "kumi_job", {"job_id": running["job_id"]}, session_id="s1")
        self.assertEqual(done["state"], "done")
        self.assertEqual({key: done["result"]["ledger"][key] for key in ("spent", "reserved", "left")},
                         {"spent": 12, "reserved": 0, "left": 88}, "settled by its reservation, as charged")
        self.assertTrue(done["result"]["media_tag"].startswith("MEDIA:"), "its kept file, for the chat")

    def test_a_create_runs_only_the_quote_approved_as_approved_and_only_once(self):
        host, _, data = self.host()
        quote_id = support.call(host, "kumi_generate", SFX, session_id="s1")["quote_id"]
        create = {"action": "create", "quote_id": quote_id}
        self.assertEqual(self.hook(host, {**create, "prompt": "louder"})["action"], "block",
                         "a create can't carry other words than the quote's")
        other_terms = calls.Entry("c9", 0.0, {"approved": True, "quote_id": quote_id, "request_sha256": "0" * 64,
                                              "max_credits": 15})
        stale = generate.create(host, create, host.identity({"session_id": "s1"}), {"kumi_call": other_terms})
        self.assertEqual((stale["error"]["code"], stale["ledger"]["reserved"]), ("stale", 0),
                         "approved with other terms than the stored quote's: nothing reserved")
        for call in ("c2", "c3"):  # the model sent the same create twice, and the producer allowed both
            self.approved(host, create, call)
        answers = [support.handle(host, "kumi_generate", create, session_id="s1") for _ in range(2)]
        self.assertEqual(sorted(answer.get("state") or answer["error"]["code"] for answer in answers),
                         ["quote_consumed", "running"])
        self.assertEqual(len(self.sent(data, "create")), 1, "sent once")
        self.assertEqual(self.hook(host, create, "c4")["action"], "block", "a used quote is never asked again")

    def test_a_quote_that_expired_or_a_kind_switched_off_is_refused_at_the_hook_and_at_dispatch(self):
        host, ctx, data = self.host(None, "--generate-expires-ms", "3000")
        first = support.call(host, "kumi_generate", SFX, session_id="s1")
        create = {"action": "create", "quote_id": first["quote_id"]}
        self.approved(host, create, "c2")
        self.assertTrue(support.eventually(lambda: time.time() * 1000 > first["expires_at"]),
                        "the quote expires between its approval and its dispatch")
        late = support.handle(host, "kumi_generate", create, session_id="s1")
        self.assertEqual((late["error"]["code"], late["ledger"]["reserved"]), ("expired_quote", 0))
        self.assertEqual(self.hook(host, create, "c3")["action"], "block", "an expired quote is never asked")

        second = support.call(host, "kumi_generate", SFX, session_id="s1")
        create = {"action": "create", "quote_id": second["quote_id"]}
        self.approved(host, create, "c4")
        ctx.settings["mirelo_sfx_enabled"] = False  # the producer switched sound effects off after approving
        off = support.handle(host, "kumi_generate", create, session_id="s1")
        self.assertEqual((off["error"]["code"], off["ledger"]["reserved"]), ("not_enabled", 0))
        self.assertEqual(self.hook(host, create, "c5")["action"], "block", "never asked while it's off")
        self.assertEqual(self.sent(data, "create"), [])

    def test_the_cap_a_freeze_and_a_turn_with_nobody_in_it_refuse_before_anyone_is_asked(self):
        for cap, estimate in ((0, "15"), (0, "0"), (10, "15")):  # a cap of 0 refuses even a zero estimate
            with self.subTest(cap=cap, estimate=estimate):
                host, _, _ = self.host({**ON, "mirelo_credit_cap": cap}, "--generate-estimate", estimate)
                quote_id = support.call(host, "kumi_generate", SFX, session_id="s1")["quote_id"]
                self.assertEqual(self.hook(host, {"action": "create", "quote_id": quote_id})["action"], "block")
        host, ctx, data = self.host(None, "--generate-actual", "20")  # a charge above its estimate (15)
        self.assertTrue(self.generation(host)["result"]["ledger"]["frozen"])
        quote_id = support.call(host, "kumi_generate", SFX, session_id="s1")["quote_id"]
        self.assertEqual(self.hook(host, {"action": "create", "quote_id": quote_id})["action"], "block", "frozen")
        for surface in ("webhook", "api_server"):
            with self.subTest(surface=surface):
                ctx.surface = surface
                self.assertEqual(self.hook(host, {"action": "create", "quote_id": quote_id})["message"],
                                 policy.UNATTENDED)
                self.assertEqual(self.hook(host, MIDI)["message"], policy.UNATTENDED)
                self.assertIsNone(self.hook(host, SFX), "a free estimate may still be read there")
        looked = [m for m in support.requests(data / "state") if m.get("params", {}).get("arguments", {}).get("preview")]
        self.assertEqual(looked, [], "with nobody there, the runtime never looked at the file")
        ctx.surface, ctx.cron_execution = "tui", object()
        self.assertEqual(self.hook(host, SFX)["message"], tools.SCHEDULED)

    def test_only_a_native_not_submitted_frees_the_reservation_and_a_lost_answer_keeps_it(self):
        host, _, _ = self.host(None, "--generate-create", "not_submitted")
        quote_id = support.call(host, "kumi_generate", SFX, session_id="s1")["quote_id"]
        refused = support.call(host, "kumi_generate", {"action": "create", "quote_id": quote_id}, session_id="s1")
        self.assertEqual((refused["error"]["code"], refused["ledger"]["reserved"]), ("not_submitted", 0), refused)
        host, _, _ = self.host(None, "--slow", "generate=2")
        quote_id = support.call(host, "kumi_generate", SFX, session_id="s1")["quote_id"]
        with mock.patch.object(generate, "TIMEOUT", 0.3):
            unsure = support.call(host, "kumi_generate", {"action": "create", "quote_id": quote_id}, session_id="s1")
        self.assertEqual((unsure["state"], unsure["ledger"]["reserved"]), ("unsure", 15), "kept, never sent again")
        host, _, _ = self.host(None, "--generate-create", "not_submitted", ledger=Unfreeable)
        quote_id = support.call(host, "kumi_generate", SFX, session_id="s1")["quote_id"]
        stuck = support.call(host, "kumi_generate", {"action": "create", "quote_id": quote_id}, session_id="s1")
        self.assertEqual((stuck["error"]["code"], stuck["ledger"]["reserved"]), ("not_submitted", 15), stuck)
        self.assertIn("hint", stuck, "not freed: the model is told its credits stay reserved, and how they can go")

    def test_a_generation_whose_create_answer_was_lost_is_found_by_its_quote_and_settled(self):
        host, _, data = self.host(None, "--slow", "generate=2")
        quote_id = support.call(host, "kumi_generate", SFX, session_id="s1")["quote_id"]
        with mock.patch.object(generate, "TIMEOUT", 0.3):  # the answer is lost; the runtime goes on and makes the job
            unsure = support.call(host, "kumi_generate", {"action": "create", "quote_id": quote_id}, session_id="s1")
        self.assertEqual(unsure["state"], "unsure")
        jobs = data / "state" / "jobs"
        self.assertTrue(support.eventually(lambda: any(jobs.glob("g-*.json"))), "the runtime lists the job it made")
        [listed] = jobs.glob("g-*.json")
        job_id = json.loads(listed.read_text(encoding="utf-8"))["job_id"]
        [held] = host.ledger().reservations()
        self.assertIsNone(held["job_id"], "never bound: its answer never came")
        done = support.call(host, "kumi_job", {"job_id": job_id}, session_id="s1")
        self.assertEqual({key: done["result"]["ledger"][key] for key in ("spent", "reserved")},
                         {"spent": 12, "reserved": 0}, "settled as charged, never left looking unknown")
        self.assertEqual(host.ledger().reservations(), [], "so it can't be released by hand as unknown")
        self.assertEqual(len(self.sent(data, "create")), 1, "nothing was created again")

    def test_a_job_already_made_is_still_fetched_with_the_key_after_new_generations_are_switched_off(self):
        host, ctx, data = self.host()
        quote_id = support.call(host, "kumi_generate", SFX, session_id="s1")["quote_id"]
        running = support.call(host, "kumi_generate", {"action": "create", "quote_id": quote_id}, session_id="s1")
        ctx.settings.update(mirelo_acknowledged=False, mirelo_sfx_enabled=False)  # no new paid generations
        done = support.call(host, "kumi_job", {"job_id": running["job_id"]}, session_id="s1")
        self.assertEqual({key: done["result"]["ledger"][key] for key in ("spent", "reserved")},
                         {"spent": 12, "reserved": 0}, "the job already paid for is fetched and settled")
        [fetched] = self.sent(data, "job")
        self.assertEqual(fetched["_mirelo"], {"api_key": "<given>", "acknowledged": False, "enabled": False},
                         "the key for its GET, with the switches as they are now")
        self.assertEqual(len(self.sent(data, "create")), 1)

    def test_a_create_a_busy_ledger_refused_goes_again_as_the_same_call_with_the_same_reservation(self):
        def created(host) -> dict:
            quote_id = support.call(host, "kumi_generate", SFX, session_id="s1")["quote_id"]
            return support.call(host, "kumi_generate", {"action": "create", "quote_id": quote_id}, session_id="s1")

        with mock.patch.object(generate, "LEDGER_RETRY_PAUSE", 0):
            host, _, data = self.host(None, "--generate-create", "ledger_busy_once")
            running = created(host)
            self.assertEqual(running["state"], "running", running)
            first, again = self.sent(data, "create")
            self.assertEqual((first["_host"], first["_mirelo"]), (again["_host"], again["_mirelo"]),
                             "the same host call and reservation, never a new one")
            self.assertEqual(host.ledger().snapshot()["reserved"], 15, "reserved once")
            host, _, data = self.host(None, "--generate-create", "ledger_busy")
            busy = created(host)
        self.assertEqual((busy["error"]["code"], busy["ledger"]["reserved"]), ("ledger_unavailable", 15), busy)
        self.assertIn("don't create it again", busy["hint"])
        self.assertEqual(len(self.sent(data, "create")), generate.LEDGER_RETRIES + 1, "then it stops")

    def test_a_create_kumi_can_not_send_holds_no_credits(self):
        host, _, data = self.host()
        quote_id = support.call(host, "kumi_generate", SFX, session_id="s1")["quote_id"]
        create = {"action": "create", "quote_id": quote_id}
        self.approved(host, create, "c2")
        host.runtimes.stop_all()  # Kumi's runtime stopped after the approval
        stopped = support.handle(host, "kumi_generate", create, session_id="s1")
        self.assertEqual((sorted(stopped), stopped["ledger"]["reserved"]), (["error", "ledger"], 0), stopped)

        support.call(host, "kumi_connect", {}, session_id="s1")
        quote_id = support.call(host, "kumi_generate", SFX, session_id="s1")["quote_id"]
        create = {"action": "create", "quote_id": quote_id}
        self.approved(host, create, "c3")
        held = host.live()
        host.runtimes.stop_all()  # it stopped after Kumi had it, before the create was written to it
        with mock.patch.object(host, "live", return_value=held):
            raced = support.handle(host, "kumi_generate", create, session_id="s1")
        self.assertEqual((raced["error"]["code"], raced["ledger"]["reserved"]), ("not_running", 0),
                         "proof nothing left frees it")
        self.assertEqual(self.sent(data, "create"), [], "nothing was sent")

    def test_sending_audio_to_mirelo_is_approved_first_from_the_runtimes_own_description_of_it(self):
        host, _, data = self.host()
        asked = self.hook(host, MIDI)
        self.assertEqual(asked["action"], "approve")
        self.assertRegex(asked["rule_key"], r"^kumi:upload:[0-9a-f]{64}:s1$", "the file's own key, in this chat")
        self.assertIn("Dusty Snare 04.wav", asked["message"])
        self.assertIn("480 KB", asked["message"], "a file under a megabyte isn't shown as 0.5 MB, or 0.0")
        self.assertTrue((data / generate.LEDGER_FILE).is_file(), "the producer's policy is in the ledger first")
        self.assertEqual(self.sent(data, "quote"), [], "only the local preview so far")
        quoted = support.call(host, "kumi_generate", MIDI, session_id="s1")
        self.assertEqual((quoted["state"], quoted["upload"], quoted["file"]), ("quoted", True, "Dusty Snare 04.wav"),
                         "the quote of the call whose preview was approved")
        unpinned = generate.quote(host, MIDI, host.identity({"session_id": "s1", "tool_call_id": "never-asked"}), {})
        self.assertEqual(unpinned["error"]["code"], "stale", "the runtime takes a quote only for its call's preview")

    def test_the_key_turns_up_nowhere_but_in_the_requests_that_carry_it(self):
        events: list = []
        records: list = []
        catcher = logging.Handler(logging.DEBUG)
        catcher.emit = records.append
        root = logging.getLogger()
        self.addCleanup(root.setLevel, root.level)
        root.setLevel(logging.DEBUG)
        root.addHandler(catcher)
        self.addCleanup(root.removeHandler, catcher)
        with mock.patch.object(tools, "broadcast", side_effect=lambda event, payload: events.append(payload)):
            host, _, data = self.host()
            answers = [support.call(host, "kumi_generate", SFX, session_id="s1")]
            answers.append(support.call(host, "kumi_generate", {"action": "create", "quote_id": answers[0]["quote_id"]},
                                        session_id="s1"))
            answers.append(support.call(host, "kumi_job", {"job_id": answers[1]["job_id"]}, session_id="s1"))
            answers.append(support.call(host, "kumi_generate", MIDI, session_id="s1"))
            answers.append(support.call(host, "kumi_generate", {"action": "create", "quote_id": answers[3]["quote_id"]},
                                        session_id="s1"))
            for tool, answer in zip(("kumi_generate", "kumi_generate", "kumi_job", "kumi_generate", "kumi_generate"),
                                    answers):
                policy.post_tool_call(host, tool_name=tool, tool_call_id="c", result=json.dumps(answer), status="ok",
                                      session_id="s1")
            started = json.loads((data / "state" / "fixture-start.json").read_text(encoding="utf-8"))
            os.kill(started["pid"], getattr(signal, "SIGKILL", signal.SIGTERM))  # the fixture itself, not kumi.bat
            self.assertTrue(support.eventually(lambda: host.runtimes.get(data) is None))
        self.assertEqual(len(events), 5, "a card for each quote, create and job")
        said = [json.dumps(answers), json.dumps(events)]
        for record in records:
            said += [record.getMessage(), record.exc_text or "",
                     logging.Formatter().formatException(record.exc_info) if record.exc_info else ""]
        for folder, _, files in os.walk(data):
            said += [Path(folder, name).read_text(encoding="utf-8", errors="replace") for name in files]
        self.assertFalse([text for text in said if KEY in text], "nowhere: answers, events, logs, the state folder")

    def test_hermes_kumi_credits_lists_whats_held_and_changes_it_only_after_a_yes_in_a_terminal(self):
        host, _, _ = self.host(None, "--slow", "generate=2")
        quote_id = support.call(host, "kumi_generate", SFX, session_id="s1")["quote_id"]
        with mock.patch.object(generate, "TIMEOUT", 0.3):  # whether Mirelo got it is unknown: release's only case
            support.call(host, "kumi_generate", {"action": "create", "quote_id": quote_id}, session_id="s1")
        [held] = host.ledger().reservations()
        out = io.StringIO()
        self.assertEqual(commands.cli_credits(host, None, stdout=out), 0)
        self.assertIn(held["reservation_id"], out.getvalue(), "the id `release` takes is shown")
        release = ("release", held["reservation_id"])
        self.assertEqual(commands.cli_credits(host, *release, stdin=io.StringIO("y\n"), stdout=io.StringIO()), 1,
                         "no terminal: nothing changes")
        self.assertEqual(commands.cli_credits(host, *release, stdin=Tty("n\n"), stdout=Tty()), 1)
        self.assertEqual(host.ledger().snapshot()["reserved"], 15)
        self.assertEqual(commands.cli_credits(host, *release, stdin=Tty("y\n"), stdout=Tty()), 0)
        self.assertEqual(host.ledger().snapshot()["reserved"], 0, "released: nothing held")
        out = io.StringIO()
        commands.cli_credits(host, None, stdout=out)
        self.assertIn("Released by you, so nothing is held", out.getvalue())
        self.assertIn(f"{held['reservation_id']}: sound effect, was 15 credits", out.getvalue())

        host, _, _ = self.host(None, "--generate-actual", "20")  # a charge above its estimate (15) pauses new ones
        self.generation(host)
        self.assertTrue(host.ledger().snapshot()["frozen"])
        self.assertEqual(commands.cli_credits(host, "unfreeze", stdin=Tty("n\n"), stdout=Tty()), 1)
        self.assertTrue(host.ledger().snapshot()["frozen"])
        self.assertEqual(commands.cli_credits(host, "unfreeze", stdin=Tty("y\n"), stdout=Tty()), 0)
        self.assertFalse(host.ledger().snapshot()["frozen"])
        self.assertFalse([name for name, _, _, _ in tools.TOOLS if "credit" in name], "no tool reaches them")

    def test_a_charge_mirelo_reports_after_the_producer_released_its_reservation_still_counts(self):
        host, _, data = self.host(None, "--slow", "generate=2")
        quote_id = support.call(host, "kumi_generate", SFX, session_id="s1")["quote_id"]
        with mock.patch.object(generate, "TIMEOUT", 0.3):  # the answer is lost; the runtime goes on and makes the job
            unsure = support.call(host, "kumi_generate", {"action": "create", "quote_id": quote_id}, session_id="s1")
        self.assertEqual(unsure["state"], "unsure")
        [held] = host.ledger().reservations()
        self.assertEqual(commands.cli_credits(host, "release", held["reservation_id"], stdin=Tty("y\n"), stdout=Tty()),
                         0, "the producer checked too early, and released it")
        self.assertEqual({key: host.ledger().snapshot()[key] for key in ("spent", "reserved")},
                         {"spent": 0, "reserved": 0})
        jobs = data / "state" / "jobs"
        self.assertTrue(support.eventually(lambda: any(jobs.glob("g-*.json"))), "but the job was made")
        [listed] = jobs.glob("g-*.json")
        job_id = json.loads(listed.read_text(encoding="utf-8"))["job_id"]
        plain = host.live().call("job", {"job_id": job_id}, host.identity({"session_id": "s1"}), timeout=10)
        other = {**plain, "result": {**plain["result"], "credits": {**plain["result"]["credits"], "reserved": 99}}}
        self.assertIsNone(generate.adopt(host, job_id, other), "a receipt that isn't that job's is refused")
        self.assertEqual(host.ledger().find_job(job_id), None, "and nothing was bound")
        done = support.call(host, "kumi_job", {"job_id": job_id}, session_id="s1")
        totals = done["result"]["ledger"]
        self.assertEqual({key: totals[key] for key in ("spent", "reserved", "frozen")},
                         {"spent": 12, "reserved": 0, "frozen": True},
                         "found by its quote and bound with its own record: the charge counts, and, held nowhere, "
                         "pauses new generations (amendment C)")
        self.assertEqual(len(self.sent(data, "create")), 1, "nothing was created again")

    def test_a_card_goes_only_for_a_call_hermes_ran_and_none_over_its_size(self):
        sent: list = []
        with mock.patch.object(tools, "broadcast", side_effect=lambda event, payload: sent.append((event, payload))):
            host, _, _ = self.host()
            quoted = support.call(host, "kumi_generate", SFX, session_id="s1")
            for status in ("blocked", "error"):
                policy.post_tool_call(host, tool_name="kumi_generate", tool_call_id="c1", result=json.dumps(quoted),
                                      status=status, session_id="s1")
            self.assertEqual(sent, [], "no card for a call Hermes didn't run through")
            huge = {**quoted, "summary": "x" * generate.CARD_BYTES}
            policy.post_tool_call(host, tool_name="kumi_generate", tool_call_id="c2", result=json.dumps(huge),
                                  status="ok", session_id="s1")
        [(event, payload)] = sent
        self.assertEqual((event, payload["tool_call_id"], "card" in payload), ("generate.updated", "c2", False),
                         "too big for a card: the Jobs row refreshes through kumi_job")

    def test_a_generation_card_leaves_out_midis_notes(self):
        card = generate.card({"job_id": "g-1", "kind": "generate", "state": "done",
                              "result": {"kind": "audio_to_midi", "note_count": 2, "notes": [{"pitch": 60}] * 2}})
        self.assertEqual(card["result"], {"kind": "audio_to_midi", "note_count": 2})


if __name__ == "__main__":
    unittest.main()
