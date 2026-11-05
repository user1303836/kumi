"""A stand-in for W3's `ledger.Ledger` (W3-GENERATE.md, CC-13 amendment A) until ledger.py lands: the same calls and
refusal codes, kept in memory per plugin-data folder; like the real one, opening it makes `ledger.json` there. It
serves the plugin's own tests only; the real ledger's file lock, fsync and cross-process atomicity (13-R4) are W3's to
test."""
from __future__ import annotations

from pathlib import Path
import time
from typing import Any

TERMINAL = ("done", "failed", "cancelled", "interrupted")
QUOTE_FIELDS = ("quote_id", "kind", "prompt", "file", "seconds", "estimate", "expires_at", "request_sha256", "upload",
                "summary")


class LedgerError(Exception):
    def __init__(self, code: str, message: str):
        super().__init__(message)
        self.code, self.message = code, message


class Ledger:
    books: dict[str, dict] = {}

    def __init__(self, data_dir: Any, *, credit_cap: int, lock_timeout: float = 2.0):
        self.book = Ledger.books.setdefault(str(data_dir), {"quotes": {}, "reservations": {}, "spent": 0,
                                                             "frozen": False, "policy": None})
        self.cap = credit_cap
        (Path(data_dir) / "ledger.json").touch()

    def configure(self, *, acknowledged: bool, enabled: dict) -> None:
        self.book["policy"] = {"acknowledged": acknowledged, "enabled": dict(enabled), "cap": self.cap}

    def record_quote(self, result: dict, host_call_id: Any) -> dict:
        stored = {key: result.get(key) for key in QUOTE_FIELDS}
        existing = self.book["quotes"].get(stored["quote_id"])
        if existing is not None and any(existing[key] != stored[key] for key in QUOTE_FIELDS):
            raise LedgerError("invalid_request", "That quote id is already recorded with other terms.")
        self.book["quotes"].setdefault(stored["quote_id"], {**stored, "consumed": False, "host_call_id": host_call_id})
        return dict(self.book["quotes"][stored["quote_id"]])

    def quote(self, quote_id: str) -> dict:
        if quote_id not in self.book["quotes"]:
            raise LedgerError("unknown_quote", "Kumi has no such quote.")
        return dict(self.book["quotes"][quote_id])

    def reserve(self, quote_id: str, approved_sha256: str, approved_max: int, *, host_call_id: Any) -> dict:
        record = self.quote(quote_id)
        if record["request_sha256"] != approved_sha256 or record["estimate"]["max_credits"] != approved_max:
            raise LedgerError("stale", "The quote isn't the one approved.")
        if record["expires_at"] <= int(time.time() * 1000):
            raise LedgerError("expired_quote", "That quote expired.")
        if record["consumed"]:
            raise LedgerError("quote_consumed", "That quote was already used.")
        if self.book["frozen"]:
            raise LedgerError("ledger_frozen", "New paid generations are paused.")
        if self.cap == 0 or approved_max > self.snapshot()["left"]:
            raise LedgerError("cap_exceeded", "That's more than the credit cap leaves.")
        self.book["quotes"][quote_id]["consumed"] = True
        reservation_id = f"r-{len(self.book['reservations']) + 1}"
        self.book["reservations"][reservation_id] = {"quote_id": quote_id, "kind": record["kind"], "job_id": None,
                                                     "reserved": approved_max, "settled": None, "actual": None,
                                                     "host_call_id": host_call_id, "submission": "unknown",
                                                     "created_at": int(time.time() * 1000)}
        return {"reservation_id": reservation_id, "quote_id": quote_id, "kind": record["kind"],
                "reserved_credits": approved_max, "expires_at": record["expires_at"]}

    def bind(self, reservation_id: str, job_id: str) -> dict:
        held = self.book["reservations"][reservation_id]
        if held["job_id"] not in (None, job_id):
            raise LedgerError("invalid_request", "That reservation has another job.")
        held["job_id"], held["submission"] = job_id, "submitted"
        return self.snapshot()

    def refuse(self, reservation_id: str, *, not_submitted: bool, reason: str) -> dict:
        if not_submitted:
            self.book["reservations"][reservation_id]["settled"] = "not_submitted"
        return self.snapshot()

    def reconcile(self, reservation_id: str, job: dict) -> dict:
        held = self.book["reservations"][reservation_id]
        if held["settled"] is not None or held["job_id"] != job.get("job_id") or job.get("state") not in TERMINAL:
            return self.snapshot()
        credits = (job.get("result") or {}).get("credits") or {}
        actual = credits.get("actual")
        if isinstance(actual, int) and not isinstance(actual, bool):
            held["settled"], held["actual"] = "charged", actual
            self.book["spent"] += actual
            if actual > held["reserved"]:
                self.book["frozen"] = True
        return self.snapshot()

    def find_job(self, job_id: str) -> dict | None:
        for reservation_id, held in self.book["reservations"].items():
            if held["job_id"] == job_id:
                return {"reservation_id": reservation_id, "quote_id": held["quote_id"], "kind": held["kind"],
                        "job_id": job_id, "provider_job_id": None, "reserved_credits": held["reserved"],
                        "submission": "submitted", "settlement": held["settled"], "actual_credits": held["actual"]}
        return None

    def reservations(self) -> list[dict]:
        return [{"reservation_id": reservation_id, "quote_id": held["quote_id"], "job_id": held["job_id"],
                 "kind": held["kind"], "reserved_credits": held["reserved"], "submission": held["submission"],
                 "created_at": held["created_at"]}
                for reservation_id, held in self.book["reservations"].items() if held["settled"] is None]

    def snapshot(self) -> dict:
        reserved = sum(held["reserved"] for held in self.book["reservations"].values() if held["settled"] is None)
        spent = self.book["spent"]
        return {"cap": self.cap, "spent": spent, "reserved": reserved, "left": max(0, self.cap - spent - reserved),
                "frozen": self.book["frozen"],
                "freeze_reason": "actual_exceeded_estimate" if self.book["frozen"] else None}

    def unfreeze(self) -> dict:
        self.book["frozen"] = False
        return self.snapshot()

    def release(self, reservation_id: str) -> dict:
        held = self.book["reservations"].get(reservation_id)
        if held is None or held["settled"] is not None or held["submission"] != "unknown":
            raise LedgerError("stale", "Only an unsettled reservation whose sending is unknown can be released.")
        held["settled"] = "released"
        return self.snapshot()
