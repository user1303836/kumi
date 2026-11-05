"""CC-13 ledger behavior through its public consumer API; no provider or owner profile."""
import copy
import importlib.util
import json
import os
from pathlib import Path
import stat
import subprocess
import sys
import tempfile
import time
import unittest
from unittest import mock

MODULE = Path(__file__).resolve().parents[1] / "ledger.py"
SPEC = importlib.util.spec_from_file_location("kumi_ledger", MODULE)
ledger = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(ledger)

IMPORT = """
import importlib.util, sys
from pathlib import Path
spec = importlib.util.spec_from_file_location("kumi_ledger", sys.argv[1])
ledger = importlib.util.module_from_spec(spec)
spec.loader.exec_module(ledger)
"""
HOLD_LOCK = """
import os, sys
from pathlib import Path
with open(Path(sys.argv[1]) / 'ledger.lock', 'a+b') as stream:
    if os.name == 'nt':
        import msvcrt
        stream.seek(0)
        msvcrt.locking(stream.fileno(), msvcrt.LK_NBLCK, 1)
    else:
        import fcntl
        fcntl.flock(stream.fileno(), fcntl.LOCK_EX | fcntl.LOCK_NB)
    print('locked', flush=True)
    sys.stdin.readline()
"""
RESERVE = IMPORT + """
import json
store = ledger.Ledger(Path(sys.argv[2]), credit_cap=15)
print('ready', flush=True)
sys.stdin.readline()
try:
    result = store.reserve(sys.argv[3], 'a' * 64, 15, host_call_id=sys.argv[4])
    print(json.dumps({'reservation_id': result['reservation_id']}), flush=True)
except ledger.LedgerError as error:
    print(json.dumps({'code': error.code}), flush=True)
"""


def quoted(quote_id="q-one", *, amount=15, kind="sfx", expires_at=None):
    return {"state": "quoted", "quote_id": quote_id, "kind": kind,
            "prompt": "a short metallic impact" if kind == "sfx" else None,
            "file": None if kind == "sfx" else "source.wav", "seconds": 5,
            "estimate": {"credits": amount, "max_credits": amount}, "credit_unit": "mirelo_credits",
            "upload": kind == "audio_to_midi", "expires_at": expires_at if expires_at is not None else time.time_ns() // 1_000_000 + 300_000,
            "request_sha256": "a" * 64, "summary": "One 5-second generation"}


def envelope(quote, *, job_id="j-one", state="running", submission="submitted", actual=None, provider="vendor-one", output=False):
    result = {"quote_id": quote["quote_id"], "kind": quote["kind"], "prompt": quote["prompt"], "seconds": quote["seconds"],
              "provider": "mirelo", "provider_job_id": provider if submission == "submitted" else None,
              "submission": submission, "artifact_id": None, "file": None, "output_type": None,
              "outputs": [], "media_tag": None,
              "credits": {"estimate": quote["estimate"]["credits"], "reserved": quote["estimate"]["max_credits"], "actual": actual},
              "credit_unit": "mirelo_credits"}
    if output:
        kind = "audio" if quote["kind"] == "sfx" else "midi"
        name = "output.wav" if kind == "audio" else "output.mid"
        result.update(artifact_id="a-primary", file=name, output_type=kind,
                      outputs=[{"artifact_id": "a-primary", "file": name, "type": kind,
                                "mime": "audio/wav" if kind == "audio" else "audio/midi", "bytes": 120}],
                      media_tag="MEDIA:/private-retained-artifacts/" + name)
    if quote["kind"] == "audio_to_midi":
        result.update(note_count=0, notes=[], timing="seconds")
    return {"job_id": job_id, "kind": "generate", "state": state,
            "progress": {"done": 0, "message": "Mirelo is generating"}, "result": result}


class LedgerTest(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.data = Path(self.temporary.name) / "plugin-data"
        self.store = ledger.Ledger(self.data, credit_cap=100)
        self.store.configure(acknowledged=True, enabled={"sfx": True, "audio_to_midi": True})

    def reserve(self, quote=None, *, call="create-one", store=None):
        quote = quoted() if quote is None else quote
        store = self.store if store is None else store
        store.record_quote(quote, "quote-" + quote["quote_id"])
        reservation = store.reserve(quote["quote_id"], quote["request_sha256"], quote["estimate"]["max_credits"], host_call_id=call)
        return quote, reservation["reservation_id"]

    def assert_code(self, code, function, *args, **kwargs):
        with self.assertRaises(ledger.LedgerError) as caught:
            function(*args, **kwargs)
        self.assertEqual(caught.exception.code, code)
        self.assertEqual(str(caught.exception), caught.exception.message)
        self.assertNotIn(str(self.data), caught.exception.message)

    def stored(self):
        return json.loads((self.data / "ledger.json").read_text(encoding="utf-8"))

    def child(self, script, *args):
        child = subprocess.Popen([sys.executable, "-c", script, *map(str, args)],
                                 stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
        def cleanup():
            if child.poll() is None:
                child.kill()
            child.communicate(timeout=5)
        self.addCleanup(cleanup)
        return child

    def test_missing_policy_is_disabled_not_key_authority(self):
        fresh = ledger.Ledger(self.data / "fresh", credit_cap=100)
        self.assertEqual(fresh.snapshot(), {"cap": 100, "spent": 0, "reserved": 0, "left": 100,
                                           "frozen": False, "freeze_reason": None})
        quote = quoted()
        quote["_mirelo"] = {"api_key": "SENTINEL_PRIVATE_KEY", "acknowledged": True, "enabled": True}
        fresh.record_quote(quote, "quote-call")
        self.assert_code("not_configured", fresh.reserve, "q-one", "a" * 64, 15, host_call_id="create-call")
        fresh.configure(acknowledged=True, enabled={"sfx": False, "audio_to_midi": False})
        self.assert_code("not_enabled", fresh.reserve, "q-one", "a" * 64, 15, host_call_id="create-call")
        self.assertNotIn("SENTINEL_PRIVATE_KEY", (self.data / "fresh" / "ledger.json").read_text())
        raw = self.stored()
        del raw["policy"]
        (self.data / "ledger.json").write_text(json.dumps(raw))
        self.store.record_quote(quoted(), "quote-call")
        self.assert_code("not_configured", self.store.reserve, "q-one", "a" * 64, 15, host_call_id="create-call")

    def test_constructor_and_policy_reject_unsafe_numbers_and_flags(self):
        for value in (True, False, -1, 0.5, float("inf"), float("nan"), 1 << 53, "10"):
            with self.subTest(value=value):
                self.assert_code("invalid_request", ledger.Ledger, self.data, credit_cap=value)
        for value in (True, -1, float("inf"), float("nan"), 61, "2"):
            self.assert_code("invalid_request", ledger.Ledger, self.data, credit_cap=10, lock_timeout=value)
        for flags in ({"sfx": True}, {"sfx": 1, "audio_to_midi": False}, {"sfx": True, "audio_to_midi": False, "key": "SECRET"}):
            self.assert_code("invalid_request", self.store.configure, acknowledged=True, enabled=flags)
        self.assert_code("invalid_request", self.store.configure, acknowledged=1, enabled={"sfx": True, "audio_to_midi": True})

    def test_current_cap_and_zero_cap_apply_even_to_free_estimate(self):
        self.store.record_quote(quoted(amount=0), "quote-call")
        zero = ledger.Ledger(self.data, credit_cap=0)
        self.assert_code("cap_exceeded", zero.reserve, "q-one", "a" * 64, 0, host_call_id="create-call")
        self.assertFalse(self.store.quote("q-one")["consumed"])
        self.reserve(quoted("q-paid", amount=80))
        lowered = ledger.Ledger(self.data, credit_cap=50)
        self.assertEqual(lowered.snapshot()["left"], 0)
        self.assertEqual(lowered.snapshot()["reserved"], 80)
        self.assert_code("cap_exceeded", lowered.reserve, "q-one", "a" * 64, 0, host_call_id="create-free")

    def test_quote_binding_expiry_and_repeated_identity(self):
        quote = quoted()
        self.store.record_quote(quote, "quote-call")
        self.assert_code("stale", self.store.reserve, "q-one", "b" * 64, 15, host_call_id="create-call")
        self.assert_code("stale", self.store.reserve, "q-one", "a" * 64, 14, host_call_id="create-call")
        self.assertEqual(self.store.reservations(), [])
        conflict = dict(quote, summary="Different approval")
        self.assert_code("stale", self.store.record_quote, conflict, "quote-call")
        self.assert_code("stale", self.store.record_quote, quote, "another-call")
        self.assert_code("unknown_quote", self.store.quote, "q-missing")
        self.store.record_quote(quoted("q-expired", expires_at=1), "quote-expired")
        self.assert_code("expired_quote", self.store.reserve, "q-expired", "a" * 64, 15, host_call_id="create-expired")
        self.store.reserve("q-one", "a" * 64, 15, host_call_id="create-call")
        self.assertTrue(self.store.record_quote(quote, "quote-call")["consumed"])
        self.assert_code("quote_consumed", self.store.reserve, "q-one", "a" * 64, 15, host_call_id="create-next")
        self.store.record_quote(quoted("q-next"), "quote-next")
        self.assert_code("quote_consumed", self.store.reserve, "q-next", "a" * 64, 15, host_call_id="create-call")

    def test_quote_projection_never_persists_private_or_path_fields(self):
        quote = quoted(kind="audio_to_midi")
        quote.update(_mirelo={"api_key": "SENTINEL_PRIVATE_KEY"}, source={"file": "/SENTINEL_SOURCE/path.wav"},
                     provider_blob={"url": "https://signed.example/SENTINEL_TOKEN"}, ledger={"spent": 900})
        stored = self.store.record_quote(quote, "quote-call")
        self.assertEqual(stored["file"], "source.wav")
        self.assertEqual(set(stored), {"state", "quote_id", "kind", "prompt", "file", "seconds", "estimate", "credit_unit",
                                       "upload", "expires_at", "request_sha256", "summary", "host_call_id", "consumed"})
        text = (self.data / "ledger.json").read_text()
        for value in ("SENTINEL", "api_key", "provider_blob", "https://"):
            self.assertNotIn(value, text)
        stored["estimate"]["max_credits"] = 999
        self.assertEqual(self.store.quote("q-one")["estimate"]["max_credits"], 15)

    def test_rejects_bad_quote_values_without_writing(self):
        before = (self.data / "ledger.json").read_bytes()
        changes = [("seconds", False), ("seconds", 0), ("seconds", float("nan")), ("seconds", float("inf")),
                   ("expires_at", True), ("expires_at", 1 << 53), ("quote_id", "../outside"),
                   ("request_sha256", "A" * 64), ("summary", "line\nbreak"), ("prompt", "\ud800"),
                   ("upload", 0), ("credit_unit", "dollars"), ("file", "/source.wav")]
        for key, value in changes:
            with self.subTest(key=key, value=value):
                self.assert_code("invalid_request", self.store.record_quote, dict(quoted(), **{key: value}), "quote-call")
        for amount in (True, -1, 0.5, float("nan"), 1 << 53):
            self.assert_code("invalid_request", self.store.record_quote, quoted(amount=amount), "quote-call")
        for name in ("/source.wav", "..\\source.wav", "C:source.wav", "..", "bad\x00.wav"):
            quote = quoted(kind="audio_to_midi")
            quote["file"] = name
            self.assert_code("invalid_request", self.store.record_quote, quote, "quote-call")
        self.assertEqual((self.data / "ledger.json").read_bytes(), before)

    def test_unknown_survives_restart_expiry_cap_change_and_lost_transport(self):
        quote, reservation = self.reserve()
        self.assertEqual(self.store.refuse(reservation, not_submitted=False, reason="transport lost")["reserved"], 15)
        self.store.bind(reservation, "j-one")
        self.store.reconcile(reservation, envelope(quote, submission="unknown", state="interrupted"))
        restarted = ledger.Ledger(self.data, credit_cap=200)
        with mock.patch.object(ledger, "_now", return_value=quote["expires_at"] + 1):
            self.assertEqual(restarted.snapshot()["reserved"], 15)
            self.assert_code("expired_quote", restarted.reserve, "q-one", "a" * 64, 15, host_call_id="never-retry")
        self.assertEqual(restarted.find_job("j-one")["settlement"], "pending")
        self.assertIsNone(restarted.find_job("j-one")["actual_credits"])
        self.assertEqual(restarted.reservations(), [{"reservation_id": reservation, "quote_id": "q-one", "job_id": "j-one",
                                                    "kind": "sfx", "reserved_credits": 15, "submission": "unknown",
                                                    "created_at": self.stored()["reservations"][reservation]["created_at"]}])

    def test_missing_receipt_and_running_charge_never_refund_or_spend(self):
        quote, reservation = self.reserve()
        missing = {"job_id": "j-one", "kind": "generate", "state": "interrupted"}
        self.assert_code("stale", self.store.reconcile, reservation, missing)
        self.assertIsNone(self.store.find_job("j-one"))
        self.store.bind(reservation, "j-one")
        result = self.store.reconcile(reservation, {"job_id": "j-one", "kind": "generate", "state": "interrupted",
                                                   "error": {"code": "interrupted", "reason": "Runtime stopped"}})
        self.assertEqual((result["spent"], result["reserved"]), (0, 15))
        result = self.store.reconcile(reservation, envelope(quote, actual=12))
        self.assertEqual((result["spent"], result["reserved"]), (0, 15))
        self.assertEqual(self.store.find_job("j-one")["provider_job_id"], "vendor-one")
        self.store.reconcile(reservation, envelope(quote, submission="unknown"))
        self.assertEqual(self.store.find_job("j-one")["provider_job_id"], "vendor-one")
        result = self.store.reconcile(reservation, envelope(quote, state="done", output=True))
        self.assertEqual((result["spent"], result["reserved"]), (0, 15))

    def test_terminal_output_failure_keeps_charge_and_settles_once(self):
        quote, reservation = self.reserve()
        self.store.bind(reservation, "j-one")
        failed = envelope(quote, state="failed", actual=12)
        failed["error"] = {"code": "output_failed", "reason": "SENTINEL_PROVIDER_ERROR /private/path?token=secret"}
        failed["result"]["provider_blob"] = {"url": "https://SENTINEL_SECRET"}
        # Output decoding may fail altogether; it cannot suppress a known terminal charge.
        for field in ("prompt", "seconds", "artifact_id", "file", "output_type", "outputs", "media_tag"):
            del failed["result"][field]
        first = self.store.reconcile(reservation, failed)
        self.assertEqual((first["spent"], first["reserved"], first["left"]), (12, 0, 88))
        restarted = ledger.Ledger(self.data, credit_cap=100)
        self.assertEqual(restarted.reconcile(reservation, failed), first)
        self.assertEqual(restarted.bind(reservation, "j-one"), first)
        self.assertEqual(restarted.reconcile(reservation, envelope(quote, submission="unknown")), first)
        self.assertEqual(restarted.reservations(), [])
        self.assertEqual(restarted.find_job("j-one")["actual_credits"], 12)
        self.assert_code("stale", restarted.reconcile, reservation, envelope(quote, state="done", actual=13))
        self.assert_code("stale", restarted.bind, reservation, "j-replacement")
        self.assertNotIn("SENTINEL", (self.data / "ledger.json").read_text())
        self.assertNotIn("private/path", (self.data / "ledger.json").read_text())

    def test_actual_overrun_freezes_until_explicit_unfreeze_and_keeps_audit(self):
        quote, reservation = self.reserve()
        final = envelope(quote, state="done", actual=16, output=True)
        result = self.store.reconcile(reservation, final)
        self.assertTrue(result["frozen"])
        self.assertEqual(result["freeze_reason"], "actual_exceeded_estimate")
        larger = ledger.Ledger(self.data, credit_cap=1000)
        larger.configure(acknowledged=True, enabled={"sfx": True, "audio_to_midi": True})
        larger.record_quote(quoted("q-next"), "quote-next")
        self.assert_code("ledger_frozen", larger.reserve, "q-next", "a" * 64, 15, host_call_id="create-next")
        self.assertFalse(larger.unfreeze()["frozen"])
        self.assertFalse(larger.reconcile(reservation, final)["frozen"])
        self.assertEqual(self.stored()["reservations"][reservation]["actual_credits"], 16)
        larger.reserve("q-next", "a" * 64, 15, host_call_id="create-next")
        self.assertEqual(larger.snapshot()["reserved"], 15)

    def test_only_definitive_refusal_releases_and_keeps_quote_consumed(self):
        quote, reservation = self.reserve()
        self.assert_code("invalid_request", self.store.refuse, reservation, not_submitted=1, reason="no")
        self.assertEqual(self.store.refuse(reservation, not_submitted=False, reason="timeout")["reserved"], 15)
        released = self.store.refuse(reservation, not_submitted=True, reason="typed pre-outbox NotSent: SENTINEL")
        self.assertEqual((released["spent"], released["reserved"]), (0, 0))
        self.assertEqual(self.store.refuse(reservation, not_submitted=True, reason="repeat"), released)
        saved = self.stored()["reservations"][reservation]
        self.assertEqual((saved["submission"], saved["settlement"], saved["actual_credits"]), ("not_submitted", "not_submitted", None))
        self.assert_code("quote_consumed", self.store.reserve, quote["quote_id"], "a" * 64, 15, host_call_id="create-next")
        self.assertNotIn("SENTINEL", (self.data / "ledger.json").read_text())

    def test_running_not_submitted_is_not_a_refund_but_terminal_is(self):
        quote, reservation = self.reserve()
        self.assertEqual(self.store.reconcile(reservation, envelope(quote, submission="not_submitted"))["reserved"], 15)
        self.assert_code("stale", self.store.release, reservation)
        failed = envelope(quote, state="failed", submission="not_submitted")
        failed["error"] = {"code": "not_submitted", "reason": "No billed generation was sent"}
        self.assertEqual(self.store.reconcile(reservation, failed)["reserved"], 0)
        self.assertEqual(self.store.reconcile(reservation, failed)["spent"], 0)
        self.assertEqual(self.store.find_job("j-one")["settlement"], "not_submitted")

    def test_manual_release_is_explicit_unknown_only_and_never_fake_zero(self):
        quote, reservation = self.reserve()
        released = self.store.release(reservation)
        self.assertEqual((released["spent"], released["reserved"]), (0, 0))
        self.assertEqual(self.store.release(reservation), released)
        record = self.stored()["reservations"][reservation]
        self.assertEqual((record["submission"], record["settlement"], record["actual_credits"]), ("unknown", "manual_release", None))
        self.assertTrue(self.store.quote("q-one")["consumed"])
        self.assertEqual(self.store.reservations(), [])
        second, r2 = self.reserve(quoted("q-two"), call="create-two")
        self.store.reconcile(r2, envelope(second, job_id="j-two"))
        self.assert_code("stale", self.store.release, r2)
        self.assert_code("stale", self.store.refuse, r2, not_submitted=True, reason="generic exception")
        self.assertEqual(self.store.snapshot()["reserved"], 15)

    def test_receipt_identity_and_complete_envelope_are_required(self):
        quote, reservation = self.reserve()
        self.store.bind(reservation, "j-one")
        cases = []
        for key, value in (("quote_id", "q-other"), ("kind", "audio_to_midi")):
            candidate = envelope(quote, state="done", actual=12)
            candidate["result"][key] = value
            cases.append(("stale", candidate))
        wrong_job = envelope(quote, job_id="j-other")
        cases.append(("stale", wrong_job))
        for field in ("provider", "credit_unit", "credits", "provider_job_id", "submission"):
            candidate = envelope(quote, state="done", actual=12)
            del candidate["result"][field]
            cases.append(("invalid_request", candidate))
        for value in (True, -1, 0.1, float("inf"), float("nan"), 1 << 53):
            candidate = envelope(quote, state="done", actual=value)
            cases.append(("invalid_request", candidate))
        candidate = envelope(quote)
        candidate["result"]["credits"]["reserved"] = 14
        cases.append(("stale", candidate))
        candidate = envelope(quote)
        candidate["kind"] = "render"
        cases.append(("invalid_request", candidate))
        cases.append(("invalid_request", envelope(quote, submission="unknown", actual=12)))
        before = (self.data / "ledger.json").read_bytes()
        for code, candidate in cases:
            with self.subTest(candidate=candidate):
                self.assert_code(code, self.store.reconcile, reservation, candidate)
        self.assertEqual((self.data / "ledger.json").read_bytes(), before)
        self.assertEqual(self.store.snapshot()["reserved"], 15)

    def test_binding_and_vendor_ids_cannot_move_between_reservations(self):
        quote, reservation = self.reserve()
        second, r2 = self.reserve(quoted("q-two"), call="create-two")
        self.store.bind(reservation, "j-one")
        self.assert_code("stale", self.store.bind, r2, "j-one")
        self.store.reconcile(reservation, envelope(quote))
        self.assert_code("stale", self.store.reconcile, reservation, envelope(quote, provider="different-vendor"))
        self.assert_code("stale", self.store.reconcile, r2, envelope(second, job_id="j-two"))
        self.assertEqual(self.store.snapshot()["reserved"], 30)

    def test_midi_output_payload_cannot_block_billing_or_enter_ledger(self):
        quote, reservation = self.reserve(quoted(kind="audio_to_midi"))
        result = envelope(quote, state="done", actual=12, output=True)
        result["result"].update(note_count=-1, notes={"raw": "SENTINEL_NOTE_PAYLOAD"}, outputs="broken")
        self.assertEqual(self.store.reconcile(reservation, result)["spent"], 12)
        text = (self.data / "ledger.json").read_text()
        self.assertNotIn("MEDIA:", text)
        self.assertNotIn("private-retained-artifacts", text)
        self.assertNotIn("outputs", text)
        self.assertNotIn("notes", text)
        self.assertNotIn("SENTINEL", text)

    def test_aggregate_overflow_refuses_without_losing_prior_receipts(self):
        maximum = (1 << 53) - 1
        unlimited = ledger.Ledger(self.data, credit_cap=maximum)
        unlimited.configure(acknowledged=True, enabled={"sfx": True, "audio_to_midi": True})
        quote, reservation = self.reserve(quoted(amount=maximum - 1), store=unlimited)
        unlimited.reconcile(reservation, envelope(quote, state="done", actual=maximum - 1))
        second, r2 = self.reserve(quoted("q-two", amount=1), call="create-two", store=unlimited)
        before = (self.data / "ledger.json").read_bytes()
        self.assert_code("ledger_unavailable", unlimited.reconcile, r2, envelope(second, job_id="j-two", provider="vendor-two", state="done", actual=2))
        self.assertEqual((self.data / "ledger.json").read_bytes(), before)
        self.assertEqual(unlimited.snapshot()["spent"], maximum - 1)
        self.assertEqual(unlimited.snapshot()["reserved"], 1)

    def test_simultaneous_separate_process_reservations_fit_exactly_one(self):
        self.store.record_quote(quoted("q-first"), "quote-first")
        self.store.record_quote(quoted("q-second"), "quote-second")
        first = self.child(RESERVE, MODULE, self.data, "q-first", "create-first")
        second = self.child(RESERVE, MODULE, self.data, "q-second", "create-second")
        self.assertEqual(first.stdout.readline().strip(), "ready")
        self.assertEqual(second.stdout.readline().strip(), "ready")
        for child in (first, second):
            child.stdin.write("go\n")
            child.stdin.flush()
        outcomes = []
        for child in (first, second):
            stdout, stderr = child.communicate(timeout=10)
            self.assertEqual(child.returncode, 0, stderr)
            outcomes.append(json.loads(stdout))
        self.assertEqual(sum("reservation_id" in item for item in outcomes), 1)
        self.assertEqual([item["code"] for item in outcomes if "code" in item], ["cap_exceeded"])
        self.assertEqual(self.store.snapshot()["reserved"], 15)
        self.assertEqual(len(self.store.reservations()), 1)
        self.assertEqual(sum(self.store.quote(key)["consumed"] for key in ("q-first", "q-second")), 1)

    def test_every_method_has_a_bounded_cross_process_lock_wait(self):
        quote, reservation = self.reserve()
        holder = self.child(HOLD_LOCK, self.data)
        self.assertEqual(holder.stdout.readline().strip(), "locked")
        busy = ledger.Ledger(self.data, credit_cap=100, lock_timeout=0.03)
        calls = [lambda: busy.configure(acknowledged=True, enabled={"sfx": True, "audio_to_midi": True}),
                 lambda: busy.record_quote(quote, "quote-q-one"), lambda: busy.quote("q-one"),
                 lambda: busy.reserve("q-one", "a" * 64, 15, host_call_id="create-two"),
                 lambda: busy.bind(reservation, "j-one"),
                 lambda: busy.refuse(reservation, not_submitted=True, reason="NotSent"),
                 lambda: busy.reconcile(reservation, envelope(quote)), lambda: busy.find_job("j-one"),
                 busy.snapshot, busy.reservations, busy.unfreeze, lambda: busy.release(reservation)]
        for call in calls:
            started = time.monotonic()
            self.assert_code("ledger_unavailable", call)
            self.assertLess(time.monotonic() - started, 1.0)
        holder.communicate(input="release\n", timeout=5)
        self.assertEqual(busy.snapshot()["reserved"], 15)

    def test_process_death_releases_lock_without_refunding_reservation(self):
        self.reserve()
        holder = self.child(HOLD_LOCK, self.data)
        self.assertEqual(holder.stdout.readline().strip(), "locked")
        holder.kill()
        holder.communicate(timeout=5)
        self.assertEqual(ledger.Ledger(self.data, credit_cap=100, lock_timeout=0).snapshot()["reserved"], 15)

    def test_atomic_write_is_private_and_fsynced_before_return(self):
        events = []
        real_sync, real_replace = os.fsync, ledger._replace
        def sync(fd):
            events.append("sync")
            return real_sync(fd)
        def replace(*args, **kwargs):
            events.append("replace")
            return real_replace(*args, **kwargs)
        with mock.patch.object(ledger.os, "fsync", side_effect=sync), mock.patch.object(ledger, "_replace", side_effect=replace):
            self.store.record_quote(quoted(), "quote-call")
        self.assertLess(events.index("sync"), events.index("replace"))
        if os.name != "nt":
            self.assertEqual(events[-1], "sync")
            for name in ("ledger.lock", "ledger.json"):
                self.assertEqual(stat.S_IMODE((self.data / name).stat().st_mode), 0o600)
        before = (self.data / "ledger.json").read_bytes()
        with mock.patch.object(ledger, "_replace", side_effect=OSError("SENTINEL_PATH")):
            self.assert_code("ledger_unavailable", self.store.configure, acknowledged=False, enabled={"sfx": False, "audio_to_midi": False})
        self.assertEqual((self.data / "ledger.json").read_bytes(), before)
        self.assertEqual(sorted(path.name for path in self.data.iterdir()), ["ledger.json", "ledger.lock"])

    def test_malformed_storage_fails_closed_without_overwriting_history(self):
        self.reserve()
        original = self.stored()
        variants = []
        changed = copy.deepcopy(original)
        changed["schema"] = True
        variants.append(json.dumps(changed))
        changed = copy.deepcopy(original)
        changed["policy"]["cap"] = True
        variants.append(json.dumps(changed))
        changed = copy.deepcopy(original)
        changed["policy"]["api_key"] = "SENTINEL_SECRET"
        variants.append(json.dumps(changed))
        changed = copy.deepcopy(original)
        changed["quotes"]["q-one"]["consumed"] = False
        variants.append(json.dumps(changed))
        changed = copy.deepcopy(original)
        next(iter(changed["reservations"].values()))["actual_credits"] = 0
        variants.append(json.dumps(changed))
        variants.extend(['{"schema":1,"schema":1}', "not json", "[1]", " " * (4 * 1024 * 1024 + 1)])
        for encoded in variants:
            with self.subTest(prefix=encoded[:50]):
                (self.data / "ledger.json").write_text(encoded)
                self.assert_code("ledger_unavailable", self.store.snapshot)
                self.assert_code("ledger_unavailable", self.store.configure, acknowledged=True, enabled={"sfx": True, "audio_to_midi": True})
                self.assertEqual((self.data / "ledger.json").read_text(), encoded)

    def test_symlinks_are_not_followed_or_chmodded(self):
        target = Path(self.temporary.name) / "outside"
        target.write_text("private sentinel")
        target.chmod(0o640)
        for name in ("ledger.json", "ledger.lock"):
            with self.subTest(name=name):
                path = self.data / name
                previous = path.read_bytes()
                path.unlink()
                try:
                    path.symlink_to(target)
                except OSError as error:
                    path.write_bytes(previous)
                    path.chmod(0o600)
                    self.skipTest(f"Symlinks unavailable: {error.__class__.__name__}")
                self.assert_code("ledger_unavailable", self.store.snapshot)
                self.assertEqual(target.read_text(), "private sentinel")
                if os.name != "nt":
                    self.assertEqual(stat.S_IMODE(target.stat().st_mode), 0o640)
                path.unlink()
                path.write_bytes(previous)
                path.chmod(0o600)

    @unittest.skipIf(os.name == "nt", "Unix hard-link ownership and permissions")
    def test_hardlinks_are_rejected_before_permission_changes(self):
        target = Path(self.temporary.name) / "outside"
        target.write_text("private sentinel")
        target.chmod(0o640)
        path = self.data / "ledger.json"
        path.unlink()
        os.link(target, path)
        self.assert_code("ledger_unavailable", self.store.snapshot)
        self.assertEqual(target.read_text(), "private sentinel")
        self.assertEqual(stat.S_IMODE(target.stat().st_mode), 0o640)

    @unittest.skipIf(os.name == "nt", "Unix mode refusal")
    def test_shared_files_are_refused_not_silently_chmodded(self):
        for name in ("ledger.json", "ledger.lock"):
            with self.subTest(name=name):
                path = self.data / name
                path.chmod(0o640)
                self.assert_code("ledger_unavailable", self.store.snapshot)
                self.assertEqual(stat.S_IMODE(path.stat().st_mode), 0o640)
                path.chmod(0o600)

    @unittest.skipIf(os.name == "nt", "Unix pinned-directory operations")
    def test_directory_swap_cannot_redirect_an_active_transaction(self):
        original = self.data
        moved = original.with_name("original-data")
        redirected = original.with_name("redirected-data")
        redirected.mkdir()
        real_projection = ledger._quote_projection
        swapped = False
        def swap_then_project(value):
            nonlocal swapped
            if not swapped:
                original.rename(moved)
                original.symlink_to(redirected, target_is_directory=True)
                swapped = True
            return real_projection(value)
        with mock.patch.object(ledger, "_quote_projection", side_effect=swap_then_project):
            self.store.record_quote(quoted(), "quote-call")
        self.assertEqual(list(redirected.iterdir()), [])
        self.assertEqual(ledger.Ledger(moved, credit_cap=100).quote("q-one")["quote_id"], "q-one")
        self.assert_code("ledger_unavailable", self.store.snapshot)


if __name__ == "__main__":
    unittest.main()
