"""CC-13, W2's side: paid generation with Mirelo, when the producer switched it on. The public `kumi_generate` tool
(quote, then create), the generation jobs `kumi_job` follows, the credit ledger calls (W3's `ledger.py`, never written
here), the private `_mirelo` request fields and the producer's terminal-only credit actions. Agreed in
CONTRACT-CHANGES.md CC-13, with 13a–13e and 13-R1 to 13-R8. Credits are Mirelo API credits: an estimate and a
reservation, never a price."""
from __future__ import annotations

import logging
import math
from pathlib import Path
import time
from typing import Any

from . import calls, runtime, snapshots

log = logging.getLogger(__name__)

KINDS = ("sfx", "audio_to_midi")
# Each kind's own switch (plugin.yaml), and where Hermes keeps the key (its `mirelo_api_key` secret).
SETTINGS = {"sfx": "mirelo_sfx_enabled", "audio_to_midi": "mirelo_midi_enabled"}
KEY_ENV = "MIRELO_API_KEY"
# A quote asks for an estimate (for MIDI it also prepares the approved file); a create posts the job. Both answer
# quickly: the generation itself is a job, followed with kumi_job.
TIMEOUT = 60.0
PROMPT_MAX = 500
# W3's frozen layout: the ledger is <plugin-data>/ledger.json, beside its lock. Opening it makes it, so a profile that
# never used Mirelo has none, and kumi_job doesn't open it there.
LEDGER_FILE = "ledger.json"
# How long each ledger call waits for the ledger's lock before it's ledger_unavailable (amendment B-c): the approval
# hook makes at most four such calls, within Hermes's hook timeout (policy._generate).
LEDGER_WAIT = 2.0
# The card event (13a): the public result, MIDI `notes` left out; one over this goes without its card.
CARD_BYTES = 256 * 1024
_SOURCE_KEYS = ({"artifact_id"}, {"sound_id", "map_revision"}, {"file", "provenance"})


class LedgerUnavailable(Exception):
    """The credit ledger can't be read or written (CC-13's `ledger_unavailable`): nothing is paid."""

    def __init__(self, code: str, reason: str):
        super().__init__(reason)
        self.code, self.reason = code, reason


def error(code: str, reason: str, ledger: dict | None = None) -> dict:
    """CC-13's error before a job exists, with the aggregate ledger when it could be read."""
    return {"error": {"code": code, "reason": reason}, **({"ledger": ledger} if ledger is not None else {})}


def refusal(args: dict) -> str | None:
    """What's wrong with a kumi_generate call's arguments, or None; the hook checks this, and the handler again. A quote
    is one job: a sound effect from a prompt (and optional seconds), or MIDI notes from one CC-11 source. A create
    names only the quote it carries out (13-R1)."""
    action = args.get("action")
    if action == "create":
        if set(args) - {"action", "quote_id"} or snapshots.file_id(args.get("quote_id")) is None:
            return "create takes only the quote_id a quote returned."
        return None
    if action != "quote":
        return "action is quote or create."
    kind = args.get("kind")
    if kind == "sfx":
        prompt, seconds = args.get("prompt"), args.get("seconds")
        if set(args) - {"action", "kind", "prompt", "seconds"}:
            return "a sound effect's quote takes a prompt and, if you like, seconds."
        if not isinstance(prompt, str) or not prompt.strip() or len(prompt) > PROMPT_MAX:
            return f"prompt is what the sound effect should be, at most {PROMPT_MAX} characters."
        if seconds is not None and (isinstance(seconds, bool) or not isinstance(seconds, (int, float))
                                    or not math.isfinite(seconds) or seconds <= 0):
            return "seconds is how long the sound effect is: a number above 0."
        return None
    if kind == "audio_to_midi":
        source = args.get("source")
        if set(args) - {"action", "kind", "source"} or not isinstance(source, dict) or set(source) not in _SOURCE_KEYS:
            return ("audio to MIDI takes one source: {artifact_id}, {sound_id, map_revision}, or {file, provenance} in "
                    "an approved folder.")
        return None
    return "kind is sfx or audio_to_midi."


def ready(host: Any, kind: Any, key: str | None) -> dict | None:
    """Why a paid generation of `kind` can't run now, as CC-13's error, or None: the producer has acknowledged Mirelo
    and switched this kind on (not_enabled), and Hermes holds the key (not_configured). `key` is read once by the
    caller, so the key checked is the key sent. None of it is the model's."""
    settings = host.generation()
    if kind not in KINDS or not settings["acknowledged"] or not settings["enabled"].get(kind):
        return error("not_enabled", "Paid generation of this kind is off. The producer switches it on in Kumi's "
                                    "settings in Hermes (Mirelo acknowledged, and the kind enabled).")
    if not key:
        return error("not_configured", "No Mirelo API key is set: the producer adds it in Kumi's settings in Hermes.")
    return None


def private(key: str, **extra: Any) -> dict:
    """The `_mirelo` fields of one request (CC-13), only after the producer's approval: the key ready() checked, and
    that the producer acknowledged Mirelo and enabled this kind (ready() checked both). Never in a result, record or
    log."""
    return {"_mirelo": {"api_key": key, "acknowledged": True, "enabled": True, **extra}}


def _snapshot(ledger: Any) -> dict | None:
    try:
        return ledger.snapshot()
    except Exception:
        log.warning("Kumi couldn't read its credit ledger's totals")
        return None


def _ledger_error(failure: Exception, ledger: Any = None) -> dict:
    code = getattr(failure, "code", None)
    reason = getattr(failure, "message", None) or getattr(failure, "reason", None)
    return error(code if isinstance(code, str) and code else "ledger_unavailable",
                 reason if isinstance(reason, str) and reason else "Kumi's credit ledger refused this.",
                 _snapshot(ledger) if ledger is not None else None)


def call_id(kwargs: dict) -> str | None:
    call = kwargs.get("kumi_call")
    return call.tool_call_id if isinstance(call, calls.Entry) else None


def quote(host: Any, args: dict, identity: dict, kwargs: dict) -> dict:
    """A quote (CC-13): the runtime prepares it (for MIDI, the approved file, pinned by the approval) and asks Mirelo
    for its estimate; the ledger keeps the quote, so a create's approval is built from it, never from the model. While
    the kind is off, the ledger isn't opened (or made)."""
    key = host.mirelo_key()
    not_ready = ready(host, args.get("kind"), key)
    if not_ready:
        return not_ready
    try:
        ledger = host.ledger()
    except LedgerUnavailable as failure:
        return error(failure.code, failure.reason)
    asked = {name: args[name] for name in ("action", "kind", "prompt", "seconds", "source") if name in args}
    try:
        result = host.live().call("generate", asked, identity, timeout=TIMEOUT, private=private(key))
    except runtime.Refused as refused:
        return {**refused.as_json(), "ledger": _snapshot(ledger)}
    except runtime.Unavailable as failure:
        return error(failure.code, failure.reason, _snapshot(ledger))
    if "error" in result or result.get("state") != "quoted":
        return {**result, "ledger": _snapshot(ledger)}
    try:
        ledger.record_quote(result, call_id(kwargs))
    except Exception as failure:
        return _ledger_error(failure, ledger)
    return {**result, "ledger": _snapshot(ledger)}


def create(host: Any, args: dict, identity: dict, kwargs: dict) -> dict:
    """A create (CC-13): only the quote the producer approved, as approved (13-R1). Everything Kumi's side needs (the
    ledger, the switches and the key, the runtime) is had before the ledger reserves the estimate under its file lock,
    so a create that can't be sent never holds credits; the runtime then posts it once. Only proof that nothing was
    posted frees the reservation: the runtime's `not_submitted`, or Kumi's own refusal before anything was written to
    the runtime (runtime.NotSent). Any other loss keeps it, and nothing is ever sent again."""
    call = kwargs.get("kumi_call")
    extra = call.extra if isinstance(call, calls.Entry) else {}
    quote_id = args["quote_id"]
    if extra.get("quote_id") != quote_id or not isinstance(extra.get("request_sha256"), str) \
            or not isinstance(extra.get("max_credits"), int):
        return error("not_approved", "Kumi didn't create anything: this isn't the quote the producer approved.")
    try:
        ledger = host.ledger()
    except LedgerUnavailable as failure:
        return error(failure.code, failure.reason)
    try:
        record = ledger.quote(quote_id)
    except Exception as failure:
        return _ledger_error(failure, ledger)
    key = host.mirelo_key()
    not_ready = ready(host, record.get("kind"), key)
    if not_ready:
        return {**not_ready, "ledger": _snapshot(ledger)}
    estimate = record.get("estimate") if isinstance(record.get("estimate"), dict) else {}
    if record.get("request_sha256") != extra["request_sha256"] or estimate.get("max_credits") != extra["max_credits"]:
        return error("stale", "The quote isn't the one the producer approved; quote again.", _snapshot(ledger))
    try:
        live = host.live()
    except runtime.Unavailable as failure:
        return error(failure.code, f"Nothing was reserved or sent. {failure.reason}", _snapshot(ledger))
    try:
        reservation = ledger.reserve(quote_id, extra["request_sha256"], extra["max_credits"],
                                     host_call_id=call.tool_call_id)
    except Exception as failure:
        return _ledger_error(failure, ledger)
    reservation_id = reservation["reservation_id"]
    sealed = private(key, reservation_id=reservation_id, reserved_credits=reservation["reserved_credits"])
    try:
        result = live.call("generate", {"action": "create", "quote_id": quote_id}, identity, timeout=TIMEOUT,
                           private=sealed)
    except runtime.NotSent as failure:  # refused before a byte reached the runtime: nothing reached Mirelo either
        return _freed(ledger, reservation_id, failure.reason, error(failure.code, failure.reason))
    except runtime.Refused as refused:
        if refused.code == "not_submitted":  # the runtime's proof nothing was posted (13-R8)
            return _freed(ledger, reservation_id, refused.reason, refused.as_json())
        return {**refused.as_json(), "ledger": _snapshot(ledger)}
    except runtime.Unavailable as failure:  # posted or not is unknown: the reservation stays, and nothing is resent
        return {"state": "unsure", "code": failure.code, "quote_id": quote_id, "reason": failure.reason,
                "hint": ("Whether Mirelo got it is unknown, so its credits stay reserved. Don't create it again: "
                         "kumi_job shows the job if there is one, and the producer can check Mirelo."),
                "ledger": _snapshot(ledger)}
    if isinstance(result.get("error"), dict):
        if result["error"].get("code") == "not_submitted":
            return _freed(ledger, reservation_id, str(result["error"].get("reason") or "not submitted"), result)
        return {**result, "ledger": _snapshot(ledger)}
    job_id = result.get("job_id")
    if isinstance(job_id, str) and job_id and not _bind(ledger, reservation_id, job_id):
        result = {**result, "hint": ("Kumi couldn't link this job to its credit reservation, so following it won't "
                                     "settle it: its credits stay reserved until the producer checks Mirelo and "
                                     "hermes kumi credits.")}
    return {**result, "ledger": _snapshot(ledger)}


def _freed(ledger: Any, reservation_id: str, reason: str, answer: dict) -> dict:
    """`answer` to a create that provably never left, its reservation freed (`not_submitted`, the only release the
    model's calls make); when the ledger couldn't free it, the answer says it's still reserved, and how it can be."""
    try:
        ledger.refuse(reservation_id, not_submitted=True, reason=reason[:200])
    except Exception:
        log.warning("Kumi couldn't free a reservation its runtime didn't submit")
        answer = {**answer, "hint": (f"Nothing was sent, but Kumi couldn't free its reservation {reservation_id}: "
                                     "its credits stay reserved until the producer releases it in a terminal (hermes "
                                     "kumi credits).")}
    return {**answer, "ledger": _snapshot(ledger)}


def _bind(ledger: Any, reservation_id: str, job_id: str) -> bool:
    """Link a created job to its reservation, so kumi_job settles it; tried twice (a lock busy once may be free)."""
    for _ in range(2):
        try:
            ledger.bind(reservation_id, job_id)
            return True
        except Exception:
            continue
    log.warning("Kumi couldn't bind a generation job to its credit reservation")
    return False


def find(host: Any, job_id: Any) -> tuple[Any, dict] | None:
    """The ledger and its record of generation job `job_id`, or None: not a job id, a profile that never used Mirelo
    (no ledger, which isn't opened then), or a job the ledger hasn't bound (kumi_job then reads it plainly, and
    `adopt` may still claim it)."""
    if snapshots.file_id(job_id) is None or not (Path(host.data_dir()) / LEDGER_FILE).is_file():
        return None
    try:
        ledger = host.ledger()
        found = ledger.find_job(job_id)
    except Exception:
        return None
    return (ledger, found) if isinstance(found, dict) else None


def adopt(host: Any, job_id: Any, plain: Any) -> tuple[Any, dict] | None:
    """A generation job the ledger never bound, because the create's answer was lost (`unsure`) while the runtime went
    on and made it: the runtime's own job record (`plain`, kumi_job's plain read) names its quote, and the one unsettled
    reservation of that quote is bound to it, so it's followed and settled like any other (W3's calls, agent-m's C7
    check). Nothing is created or sent again. None for any other job, or when the quote doesn't name exactly one."""
    if not isinstance(plain, dict) or plain.get("kind") != "generate" or plain.get("job_id") != job_id \
            or snapshots.file_id(job_id) is None or not (Path(host.data_dir()) / LEDGER_FILE).is_file():
        return None
    result = plain.get("result") if isinstance(plain.get("result"), dict) else {}
    quote_id = result.get("quote_id")
    try:
        ledger = host.ledger()
        held = [row for row in ledger.reservations()
                if isinstance(row, dict) and row.get("quote_id") == quote_id and row.get("job_id") in (None, job_id)]
        if not isinstance(quote_id, str) or len(held) != 1:
            return None
        ledger.bind(held[0]["reservation_id"], job_id)
        found = ledger.find_job(job_id)
    except Exception:
        log.warning("Kumi couldn't bind a generation job it found to its credit reservation")
        return None
    return (ledger, found) if isinstance(found, dict) else None


def follow(host: Any, ledger: Any, found: dict, job_id: str, identity: dict, timeout: float) -> dict:
    """Generation job `job_id` as kumi_job gives it. Its runtime reads the saved state, and with the producer's key may
    only GET the vendor job the ledger already knows, never prepare, upload or create (W3-GENERATE.md: an approved,
    billed job is still fetched after new generations are switched off). So the key goes whenever Hermes has it, with
    the switches as they are now; without a key the runtime answers from what it saved. Every answer goes to the ledger
    (a finished one settles its reservation), and the result carries the fresh totals; an unknown charge stays
    reserved."""
    key = host.mirelo_key()
    settings = host.generation()
    sealed = {"_mirelo": {"api_key": key, "acknowledged": settings["acknowledged"],
                          "enabled": settings["enabled"].get(found.get("kind")) is True}} if key else None
    envelope = host.live().call("generate", {"action": "job", "job_id": job_id}, identity, timeout=timeout,
                                private=sealed)
    if isinstance(found.get("reservation_id"), str):
        try:
            ledger.reconcile(found["reservation_id"], envelope)
        except Exception:
            log.warning("Kumi couldn't bring a generation's credit reservation up to date")
    result = envelope.get("result") if isinstance(envelope.get("result"), dict) else {}
    totals = _snapshot(ledger)
    return {**envelope, "result": {**result, **({"ledger": totals} if totals is not None else {})}}


def card(result: dict) -> dict:
    """A generation's card (13a): the public result, MIDI's possibly large `notes` left out (`note_count` stays)."""
    trimmed = {key: value for key, value in result.items() if key != "notes"}
    if isinstance(result.get("result"), dict):
        trimmed["result"] = {key: value for key, value in result["result"].items() if key != "notes"}
    return trimmed


def expired(record: dict, now_ms: int | None = None) -> bool:
    at = record.get("expires_at")
    now = int(time.time() * 1000) if now_ms is None else now_ms
    return not isinstance(at, int) or isinstance(at, bool) or at <= now
