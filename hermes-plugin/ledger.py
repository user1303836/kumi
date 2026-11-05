"""CC-13's profile-local Mirelo authority and lifetime credit receipts.

Only producer settings configure policy. Approval consumes a quote before dispatch;
transport uncertainty never refunds it. The native runtime shares ledger.lock and
schema 1, but Python alone writes this file. No provider payload is persisted.
"""
from __future__ import annotations

import contextlib
import errno
import json
import math
import os
from pathlib import Path
import re
import secrets
import stat
import time
from typing import Iterator

_MAX_INTEGER = (1 << 53) - 1
_MAX_BYTES = 4 * 1024 * 1024
_KINDS = {"sfx", "audio_to_midi"}
_TERMINAL = {"done", "failed", "cancelled", "interrupted"}
_SUBMISSIONS = {"unknown", "submitted", "not_submitted"}
_SETTLEMENTS = {"pending", "charged", "not_submitted", "manual_release"}
_QUOTE_FIELDS = {"state", "quote_id", "kind", "prompt", "file", "seconds", "estimate",
                 "credit_unit", "upload", "expires_at", "request_sha256", "summary"}
_RESERVATION_FIELDS = {"reservation_id", "quote_id", "request_sha256", "host_call_id", "kind",
                       "reserved_credits", "expires_at", "created_at", "job_id", "provider_job_id",
                       "submission", "settlement", "actual_credits"}
_MESSAGES = {
    "not_configured": "Acknowledge Mirelo generation in Kumi's settings first.",
    "not_enabled": "This Mirelo generation kind is disabled.",
    "not_approved": "Definitive non-submission proof is required.",
    "invalid_request": "The generation ledger request is invalid.",
    "unknown_quote": "This quote is not in the generation ledger; request a new quote.",
    "expired_quote": "This quote has expired; request a new quote.",
    "quote_consumed": "This quote or create call has already been used; it cannot be sent again.",
    "stale": "The generation identity or receipt conflicts with the saved approval.",
    "cap_exceeded": "The current lifetime credit cap does not allow this reservation.",
    "ledger_frozen": "New generations are paused because an actual charge exceeded its estimate.",
    "ledger_unavailable": "The generation ledger is unavailable; no new generation was authorized.",
}


class LedgerError(Exception):
    """A stable refusal code and a safe, path- and provider-free message."""

    def __init__(self, code: str, message: str | None = None) -> None:
        # Never reflect caller/provider text, even if an integration supplies it.
        self.code = code if code in _MESSAGES else "invalid_request"
        self.message = _MESSAGES[self.code]
        super().__init__(self.message)


def _require(condition: bool, code: str = "invalid_request") -> None:
    if not condition:
        raise LedgerError(code)


def _integer(value: object) -> bool:
    return type(value) is int and 0 <= value <= _MAX_INTEGER


def _text(value: object, maximum: int) -> bool:
    if not isinstance(value, str) or not value or any(ord(c) < 32 or ord(c) == 127 for c in value):
        return False
    try:
        return len(value.encode("utf-8")) <= maximum
    except UnicodeError:
        return False


def _identity(value: object) -> bool:
    return isinstance(value, str) and re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9_.:-]{0,255}", value) is not None and ".." not in value


def _digest(value: object) -> bool:
    return isinstance(value, str) and re.fullmatch(r"[0-9a-f]{64}", value) is not None


def _basename(value: object) -> bool:
    return _text(value, 255) and value not in {".", ".."} and not any(c in value for c in "/\\:")


def _seconds(value: object) -> bool:
    return type(value) in (int, float) and 0 < value <= _MAX_INTEGER and math.isfinite(value)


def _now() -> int:
    return time.time_ns() // 1_000_000


def _policy() -> dict:
    return {"acknowledged": False, "enabled": {"sfx": False, "audio_to_midi": False}, "cap": 0}


def _empty() -> dict:
    return {"schema": 1, "policy": _policy(), "frozen": False, "freeze_reason": None,
            "quotes": {}, "reservations": {}}


def _quote_projection(value: object) -> dict:
    _require(type(value) is dict and _QUOTE_FIELDS <= value.keys())
    quote = {key: value[key] for key in _QUOTE_FIELDS}
    _require(quote["state"] == "quoted" and _identity(quote["quote_id"]))
    _require(isinstance(quote["kind"], str) and quote["kind"] in _KINDS and _digest(quote["request_sha256"]))
    _require(_seconds(quote["seconds"]) and _integer(quote["expires_at"]))
    _require(quote["credit_unit"] == "mirelo_credits" and _text(quote["summary"], 1024))
    estimate = quote["estimate"]
    _require(type(estimate) is dict and set(estimate) == {"credits", "max_credits"})
    _require(_integer(estimate["credits"]) and _integer(estimate["max_credits"]))
    _require(estimate["credits"] <= estimate["max_credits"])
    quote["estimate"] = dict(estimate)
    if quote["kind"] == "sfx":
        _require(_text(quote["prompt"], 4096) and quote["file"] is None and quote["upload"] is False)
    else:
        _require(quote["prompt"] is None and _basename(quote["file"]) and quote["upload"] is True)
    return quote


def _totals(data: dict) -> tuple[int, int]:
    spent = sum(r["actual_credits"] for r in data["reservations"].values() if r["settlement"] == "charged")
    reserved = sum(r["reserved_credits"] for r in data["reservations"].values() if r["settlement"] == "pending")
    _require(_integer(spent) and _integer(reserved) and _integer(spent + reserved), "ledger_unavailable")
    return spent, reserved


def _validate_storage(data: object) -> dict:
    _require(type(data) is dict)
    fields = {"schema", "policy", "frozen", "freeze_reason", "quotes", "reservations"}
    _require(set(data) in (fields, fields - {"policy"}))
    _require(type(data["schema"]) is int and data["schema"] == 1)
    if data.get("policy") is None:
        data["policy"] = _policy()
    policy = data["policy"]
    _require(type(policy) is dict and set(policy) == {"acknowledged", "enabled", "cap"})
    _require(type(policy["acknowledged"]) is bool and _integer(policy["cap"]))
    _require(type(policy["enabled"]) is dict and set(policy["enabled"]) == _KINDS)
    _require(all(type(v) is bool for v in policy["enabled"].values()))
    _require(type(data["frozen"]) is bool)
    _require(data["freeze_reason"] == ("actual_exceeded_estimate" if data["frozen"] else None))
    _require(type(data["quotes"]) is dict and type(data["reservations"]) is dict)
    for key, quote in data["quotes"].items():
        _require(type(quote) is dict and set(quote) == _QUOTE_FIELDS | {"host_call_id", "consumed"})
        _quote_projection(quote)
        _require(key == quote["quote_id"] and _identity(quote["host_call_id"]) and type(quote["consumed"]) is bool)
    used_quotes, used_calls, used_jobs, used_providers = set(), set(), set(), set()
    for key, r in data["reservations"].items():
        _require(type(r) is dict and set(r) == _RESERVATION_FIELDS)
        _require(_identity(key) and key == r["reservation_id"] and _identity(r["host_call_id"]))
        _require(_identity(r["quote_id"]) and r["quote_id"] in data["quotes"])
        q = data["quotes"][r["quote_id"]]
        _require(q["consumed"] and r["quote_id"] not in used_quotes and r["host_call_id"] not in used_calls)
        used_quotes.add(r["quote_id"])
        used_calls.add(r["host_call_id"])
        _require(r["request_sha256"] == q["request_sha256"] and r["kind"] == q["kind"])
        _require(_integer(r["reserved_credits"]) and r["reserved_credits"] == q["estimate"]["max_credits"])
        _require(_integer(r["created_at"]) and _integer(r["expires_at"]) and r["created_at"] < r["expires_at"] == q["expires_at"])
        _require(r["submission"] in _SUBMISSIONS and r["settlement"] in _SETTLEMENTS)
        for field, seen in (("job_id", used_jobs), ("provider_job_id", used_providers)):
            identity = r[field]
            _require(identity is None or (_identity(identity) and identity not in seen))
            if identity is not None:
                seen.add(identity)
        _require((r["submission"] == "submitted") == (r["provider_job_id"] is not None))
        _require(r["provider_job_id"] is None or r["job_id"] is not None)
        if r["settlement"] == "charged":
            _require(_integer(r["actual_credits"]) and r["submission"] == "submitted")
        else:
            _require(r["actual_credits"] is None)
        if r["settlement"] == "not_submitted":
            _require(r["submission"] == "not_submitted")
        if r["settlement"] == "manual_release":
            _require(r["submission"] == "unknown")
    _require(all(q["consumed"] == (key in used_quotes) for key, q in data["quotes"].items()))
    _totals(data)
    return data


def _unique_object(pairs: list) -> dict:
    result = {}
    for key, value in pairs:
        _require(key not in result)
        result[key] = value
    return result


def _windows_open(path: Path, *, create: bool, exclusive: bool, readonly: bool) -> int:
    """Open the reparse point itself, with a protected owner-only DACL (stdlib)."""
    import ctypes
    from ctypes import wintypes
    import msvcrt

    class SecurityAttributes(ctypes.Structure):
        _fields_ = [("length", wintypes.DWORD), ("descriptor", wintypes.LPVOID), ("inherit", wintypes.BOOL)]

    kernel = ctypes.WinDLL("kernel32", use_last_error=True)
    advapi = ctypes.WinDLL("advapi32", use_last_error=True)
    convert = advapi.ConvertStringSecurityDescriptorToSecurityDescriptorW
    convert.argtypes = [wintypes.LPCWSTR, wintypes.DWORD, ctypes.POINTER(wintypes.LPVOID), ctypes.POINTER(wintypes.DWORD)]
    convert.restype = wintypes.BOOL
    kernel.CreateFileW.argtypes = [wintypes.LPCWSTR, wintypes.DWORD, wintypes.DWORD,
                                  ctypes.POINTER(SecurityAttributes), wintypes.DWORD, wintypes.DWORD, wintypes.HANDLE]
    kernel.CreateFileW.restype = wintypes.HANDLE
    kernel.CloseHandle.argtypes = [wintypes.HANDLE]
    kernel.LocalFree.argtypes = [wintypes.LPVOID]
    advapi.SetKernelObjectSecurity.argtypes = [wintypes.HANDLE, wintypes.DWORD, wintypes.LPVOID]
    advapi.SetKernelObjectSecurity.restype = wintypes.BOOL
    descriptor = wintypes.LPVOID()
    if not convert("D:P(A;;FA;;;OW)", 1, ctypes.byref(descriptor), None):
        raise ctypes.WinError(ctypes.get_last_error())
    handle = None
    try:
        attrs = SecurityAttributes(ctypes.sizeof(SecurityAttributes), descriptor, False)
        access = 0x80000000 | 0x00040000  # GENERIC_READ | WRITE_DAC
        if not readonly:
            access |= 0x40000000  # GENERIC_WRITE
        disposition = 1 if exclusive else (4 if create else 3)  # CREATE_NEW / OPEN_ALWAYS / OPEN_EXISTING
        handle = kernel.CreateFileW(str(path), access, 7, ctypes.byref(attrs), disposition, 0x00200000, None)
        if handle == wintypes.HANDLE(-1).value:
            handle = None
            raise ctypes.WinError(ctypes.get_last_error())
        # No DACL modification until the opened object is proven to be the same regular, non-reparse file.
        flags = os.O_RDONLY if readonly else os.O_RDWR
        fd = msvcrt.open_osfhandle(handle, flags | os.O_BINARY)
        handle = None  # fd now owns it
        try:
            _check_file(path, fd)
            if not advapi.SetKernelObjectSecurity(msvcrt.get_osfhandle(fd), 0x80000004, descriptor):
                raise ctypes.WinError(ctypes.get_last_error())
            return fd
        except BaseException:
            os.close(fd)
            raise
    finally:
        if handle is not None:
            kernel.CloseHandle(handle)
        kernel.LocalFree(descriptor)


def _check_file(path: Path, fd: int, directory: int | None = None) -> None:
    opened = os.fstat(fd)
    current = os.stat(path, dir_fd=directory, follow_symlinks=False)
    _require(stat.S_ISREG(opened.st_mode) and stat.S_ISREG(current.st_mode), "ledger_unavailable")
    _require(not (getattr(current, "st_file_attributes", 0) & 0x400), "ledger_unavailable")
    _require(opened.st_nlink == 1 and (opened.st_dev, opened.st_ino) == (current.st_dev, current.st_ino), "ledger_unavailable")
    if os.name != "nt":
        _require(opened.st_uid == os.geteuid(), "ledger_unavailable")
        _require(opened.st_mode & 0o077 == 0, "ledger_unavailable")


def _open_private(path: Path, *, create: bool = False, exclusive: bool = False,
                  readonly: bool = False, directory: int | None = None) -> int:
    if os.name == "nt":
        return _windows_open(path, create=create, exclusive=exclusive, readonly=readonly)
    flags = (os.O_RDONLY if readonly else os.O_RDWR) | os.O_NOFOLLOW | os.O_NONBLOCK
    flags |= getattr(os, "O_CLOEXEC", 0)
    if create:
        flags |= os.O_CREAT
    if exclusive:
        flags |= os.O_EXCL
    fd = os.open(path, flags, 0o600, dir_fd=directory)
    try:
        _check_file(path, fd, directory)
        return fd
    except BaseException:
        os.close(fd)
        raise


def _replace(source: Path, target: Path, directory: int | None) -> None:
    if os.name == "nt":
        import ctypes
        from ctypes import wintypes
        kernel = ctypes.WinDLL("kernel32", use_last_error=True)
        kernel.MoveFileExW.argtypes = [wintypes.LPCWSTR, wintypes.LPCWSTR, wintypes.DWORD]
        kernel.MoveFileExW.restype = wintypes.BOOL
        # Same-directory atomic replacement, with write-through before returning.
        if not kernel.MoveFileExW(str(source), str(target), 0x1 | 0x8):
            raise ctypes.WinError(ctypes.get_last_error())
    else:
        os.replace(source, target, src_dir_fd=directory, dst_dir_fd=directory)


class Ledger:
    def __init__(self, data_dir, *, credit_cap: int, lock_timeout: float = 2.0):
        _require(_integer(credit_cap))
        _require(type(lock_timeout) in (int, float) and 0 <= lock_timeout <= 60 and math.isfinite(lock_timeout))
        self._dir = Path(data_dir)
        self._cap = credit_cap
        self._timeout = lock_timeout

    @contextlib.contextmanager
    def _locked(self) -> Iterator[tuple[dict, int | None]]:
        fd, directory, locked = None, None, False
        try:
            self._dir.mkdir(mode=0o700, parents=True, exist_ok=True)
            info = self._dir.lstat()
            _require(stat.S_ISDIR(info.st_mode) and not (getattr(info, "st_file_attributes", 0) & 0x400), "ledger_unavailable")
            if os.name != "nt":
                directory = os.open(self._dir, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW | getattr(os, "O_CLOEXEC", 0))
                pinned = os.fstat(directory)
                _require((pinned.st_dev, pinned.st_ino) == (info.st_dev, info.st_ino), "ledger_unavailable")
            lock_path = self._dir / "ledger.lock" if directory is None else Path("ledger.lock")
            fd = _open_private(lock_path, create=True, directory=directory)
            deadline = time.monotonic() + self._timeout
            while True:
                try:
                    if os.name == "nt":
                        import msvcrt
                        os.lseek(fd, 0, os.SEEK_SET)
                        msvcrt.locking(fd, msvcrt.LK_NBLCK, 1)
                    else:
                        import fcntl
                        fcntl.flock(fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
                    locked = True
                    break
                except OSError as exc:
                    if exc.errno not in (errno.EACCES, errno.EAGAIN, errno.EDEADLK, errno.EINTR):
                        raise
                    remaining = deadline - time.monotonic()
                    _require(remaining > 0, "ledger_unavailable")
                    time.sleep(min(0.01, remaining))
            _check_file(lock_path, fd, directory)
            yield self._load(directory), directory
        except (OSError, ValueError, TypeError, OverflowError, RecursionError):
            raise LedgerError("ledger_unavailable") from None
        finally:
            try:
                if fd is not None:
                    try:
                        if locked:
                            if os.name == "nt":
                                import msvcrt
                                os.lseek(fd, 0, os.SEEK_SET)
                                msvcrt.locking(fd, msvcrt.LK_UNLCK, 1)
                            else:
                                import fcntl
                                fcntl.flock(fd, fcntl.LOCK_UN)
                    finally:
                        os.close(fd)
            except OSError:
                raise LedgerError("ledger_unavailable") from None
            finally:
                if directory is not None:
                    os.close(directory)

    def _load(self, directory: int | None) -> dict:
        path = self._dir / "ledger.json" if directory is None else Path("ledger.json")
        try:
            fd = _open_private(path, readonly=True, directory=directory)
        except FileNotFoundError:
            return _empty()
        try:
            with os.fdopen(fd, "rb") as stream:
                _require(os.fstat(stream.fileno()).st_size <= _MAX_BYTES, "ledger_unavailable")
                encoded = stream.read(_MAX_BYTES + 1)
            _require(len(encoded) <= _MAX_BYTES, "ledger_unavailable")
            return _validate_storage(json.loads(encoded, object_pairs_hook=_unique_object))
        except (LedgerError, ValueError, TypeError, KeyError, RecursionError):
            raise LedgerError("ledger_unavailable") from None

    def _save(self, data: dict, directory: int | None) -> None:
        _validate_storage(data)
        encoded = (json.dumps(data, ensure_ascii=False, allow_nan=False, separators=(",", ":")) + "\n").encode("utf-8")
        _require(len(encoded) <= _MAX_BYTES, "ledger_unavailable")
        root = self._dir if directory is None else Path()
        scratch = root / (".ledger-" + secrets.token_hex(16) + ".tmp")
        fd = _open_private(scratch, create=True, exclusive=True, directory=directory)
        try:
            with os.fdopen(fd, "wb") as stream:
                stream.write(encoded)
                stream.flush()
                os.fsync(stream.fileno())
            # Inspect the target without following a link; replacement never follows it either.
            target = root / "ledger.json"
            try:
                current = os.stat(target, dir_fd=directory, follow_symlinks=False)
            except FileNotFoundError:
                pass
            else:
                _require(stat.S_ISREG(current.st_mode) and current.st_nlink == 1
                         and not (getattr(current, "st_file_attributes", 0) & 0x400), "ledger_unavailable")
            _replace(scratch, target, directory)
            if directory is not None:
                os.fsync(directory)
        finally:
            try:
                os.unlink(scratch, dir_fd=directory)
            except FileNotFoundError:
                pass

    def _snapshot(self, data: dict) -> dict:
        spent, reserved = _totals(data)
        return {"cap": self._cap, "spent": spent, "reserved": reserved,
                "left": max(0, self._cap - spent - reserved), "frozen": data["frozen"],
                "freeze_reason": data["freeze_reason"]}

    @staticmethod
    def _reservation(data: dict, reservation_id: str) -> dict:
        _require(_identity(reservation_id))
        _require(reservation_id in data["reservations"], "stale")
        return data["reservations"][reservation_id]

    @staticmethod
    def _bind(data: dict, r: dict, job_id: str) -> None:
        _require(_identity(job_id))
        _require(r["job_id"] in (None, job_id), "stale")
        _require(not any(other["reservation_id"] != r["reservation_id"] and other["job_id"] == job_id
                         for other in data["reservations"].values()), "stale")
        r["job_id"] = job_id

    def configure(self, *, acknowledged: bool, enabled: dict[str, bool]) -> None:
        with self._locked() as (data, directory):
            _require(type(acknowledged) is bool and type(enabled) is dict and set(enabled) == _KINDS)
            _require(all(type(value) is bool for value in enabled.values()))
            data["policy"] = {"acknowledged": acknowledged, "enabled": dict(enabled), "cap": self._cap}
            self._save(data, directory)

    def record_quote(self, quote_result: dict, host_call_id: str) -> dict:
        with self._locked() as (data, directory):
            _require(_identity(host_call_id))
            quote = _quote_projection(quote_result)
            quote.update(host_call_id=host_call_id, consumed=False)
            old = data["quotes"].get(quote["quote_id"])
            if old is not None:
                _require(all(old[key] == value for key, value in quote.items() if key != "consumed"), "stale")
                return old
            data["quotes"][quote["quote_id"]] = quote
            self._save(data, directory)
            return quote

    def quote(self, quote_id: str) -> dict:
        with self._locked() as (data, directory):
            _require(_identity(quote_id))
            _require(quote_id in data["quotes"], "unknown_quote")
            return data["quotes"][quote_id]

    def reserve(self, quote_id: str, approved_sha256: str, approved_max: int, *, host_call_id: str) -> dict:
        with self._locked() as (data, directory):
            _require(_identity(quote_id) and _identity(host_call_id) and _digest(approved_sha256) and _integer(approved_max))
            _require(quote_id in data["quotes"], "unknown_quote")
            quote = data["quotes"][quote_id]
            _require(approved_sha256 == quote["request_sha256"] and approved_max == quote["estimate"]["max_credits"], "stale")
            now = _now()
            _require(quote["expires_at"] > now, "expired_quote")
            _require(not quote["consumed"] and not any(r["host_call_id"] == host_call_id for r in data["reservations"].values()), "quote_consumed")
            _require(data["policy"]["acknowledged"], "not_configured")
            _require(data["policy"]["enabled"][quote["kind"]], "not_enabled")
            _require(not data["frozen"], "ledger_frozen")
            spent, reserved = _totals(data)
            _require(self._cap > 0 and spent + reserved + approved_max <= self._cap, "cap_exceeded")
            reservation_id = "r-" + secrets.token_hex(16)
            r = {"reservation_id": reservation_id, "quote_id": quote_id, "request_sha256": approved_sha256,
                 "host_call_id": host_call_id, "kind": quote["kind"], "reserved_credits": approved_max,
                 "expires_at": quote["expires_at"], "created_at": now, "job_id": None, "provider_job_id": None,
                 "submission": "unknown", "settlement": "pending", "actual_credits": None}
            data["reservations"][reservation_id] = r
            quote["consumed"] = True
            self._save(data, directory)
            return {key: r[key] for key in ("reservation_id", "quote_id", "kind", "reserved_credits", "expires_at")}

    def bind(self, reservation_id: str, job_id: str) -> dict:
        with self._locked() as (data, directory):
            r = self._reservation(data, reservation_id)
            _require(r["settlement"] == "pending" or r["job_id"] == job_id, "stale")
            self._bind(data, r, job_id)
            self._save(data, directory)
            return self._snapshot(data)

    def refuse(self, reservation_id: str, *, not_submitted: bool, reason: str) -> dict:
        with self._locked() as (data, directory):
            _require(type(not_submitted) is bool and isinstance(reason, str))
            r = self._reservation(data, reservation_id)
            if not not_submitted:
                return self._snapshot(data)
            _require(r["settlement"] in {"pending", "not_submitted"} and r["submission"] != "submitted", "stale")
            r.update(submission="not_submitted", settlement="not_submitted")
            # reason is display-only caller text; it is never stored or echoed.
            self._save(data, directory)
            return self._snapshot(data)

    def reconcile(self, reservation_id: str, job: dict) -> dict:
        with self._locked() as (data, directory):
            r = self._reservation(data, reservation_id)
            receipt = _job_receipt(job, data["quotes"][r["quote_id"]], r)
            if receipt is None:
                _require(r["job_id"] == job["job_id"], "stale")
                return self._snapshot(data)
            _require(r["settlement"] == "pending" or r["job_id"] == job["job_id"], "stale")
            self._bind(data, r, job["job_id"])
            submission, provider, actual = receipt
            _require(r["provider_job_id"] in (None, provider) or provider is None, "stale")
            if provider is not None:
                _require(not any(other["reservation_id"] != reservation_id and other["provider_job_id"] == provider
                                 for other in data["reservations"].values()), "stale")
            terminal = job["state"] in _TERMINAL
            if r["settlement"] != "pending":
                if r["settlement"] == "charged":
                    _require(actual is None or (terminal and actual == r["actual_credits"] and provider == r["provider_job_id"]), "stale")
                    _require(submission != "not_submitted", "stale")
                elif r["settlement"] == "not_submitted":
                    _require(submission == "not_submitted" and actual is None and provider is None, "stale")
                else:
                    _require(submission == "unknown" and actual is None and provider is None, "stale")
                return self._snapshot(data)
            if r["submission"] == "submitted":
                _require(submission != "not_submitted", "stale")
                # Delayed unknown snapshots must not erase an already-known vendor ID.
            else:
                r["submission"] = submission
            if provider is not None:
                r["provider_job_id"] = provider
            if terminal and actual is not None:
                r.update(settlement="charged", actual_credits=actual)
                if actual > r["reserved_credits"]:
                    data.update(frozen=True, freeze_reason="actual_exceeded_estimate")
            elif terminal and submission == "not_submitted":
                r["settlement"] = "not_submitted"
            self._save(data, directory)
            return self._snapshot(data)

    def find_job(self, job_id: str) -> dict | None:
        with self._locked() as (data, directory):
            _require(_identity(job_id))
            for r in data["reservations"].values():
                if r["job_id"] == job_id:
                    return {key: r[key] for key in ("reservation_id", "quote_id", "kind", "job_id", "provider_job_id",
                                                   "reserved_credits", "submission", "settlement", "actual_credits")}
            return None

    def snapshot(self) -> dict:
        with self._locked() as (data, directory):
            return self._snapshot(data)

    def reservations(self) -> list[dict]:
        with self._locked() as (data, directory):
            return [{key: r[key] for key in ("reservation_id", "quote_id", "job_id", "kind", "reserved_credits", "submission", "created_at")}
                    for r in data["reservations"].values() if r["settlement"] == "pending"]

    def unfreeze(self) -> dict:
        with self._locked() as (data, directory):
            data.update(frozen=False, freeze_reason=None)
            self._save(data, directory)
            return self._snapshot(data)

    def release(self, reservation_id: str) -> dict:
        with self._locked() as (data, directory):
            r = self._reservation(data, reservation_id)
            _require(r["settlement"] in {"pending", "manual_release"} and r["submission"] == "unknown", "stale")
            r["settlement"] = "manual_release"
            self._save(data, directory)
            return self._snapshot(data)


def _job_receipt(job: object, quote: dict, reservation: dict) -> tuple[str, str | None, int | None] | None:
    """Validate the native envelope; retain only identity and typed billing facts."""
    _require(type(job) is dict and {"job_id", "kind", "state"} <= job.keys())
    _require(_identity(job["job_id"]) and job["kind"] == "generate")
    _require(isinstance(job["state"], str) and job["state"] in _TERMINAL | {"running"})
    result = job.get("result")
    if result is None:
        return None  # An interruption without a receipt proves nothing about billing.
    fields = {"quote_id", "kind", "provider", "provider_job_id", "submission", "credits", "credit_unit"}
    _require(type(result) is dict and fields <= result.keys())
    _require(result["provider"] == "mirelo" and result["credit_unit"] == "mirelo_credits")
    _require(result["quote_id"] == quote["quote_id"] and result["kind"] == quote["kind"], "stale")
    credits = result["credits"]
    _require(type(credits) is dict and set(credits) == {"estimate", "reserved", "actual"})
    _require(_integer(credits["estimate"]) and _integer(credits["reserved"]))
    _require(credits["estimate"] == quote["estimate"]["credits"] and credits["reserved"] == reservation["reserved_credits"], "stale")
    actual = credits["actual"]
    _require(actual is None or _integer(actual))
    submission, provider = result["submission"], result["provider_job_id"]
    _require(isinstance(submission, str) and submission in _SUBMISSIONS)
    _require(provider is None or _identity(provider))
    _require((submission == "submitted") == (provider is not None))
    _require(actual is None or submission == "submitted")
    return submission, provider, actual
