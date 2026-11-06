"""From Hermes's approval hook to the tool's handler (CONTRACT-CHANGES CC-5): the model's tool-call id, and what the
approval was given for.

Hermes hands `pre_tool_call` the tool-call id, and a tool's handler only the chat's session and task. The hook records
each Kumi call under (session, task, tool, digest of the arguments without `preview`), and the handler takes the newest
record with the same key (LIFO: a denied call followed by an identical approved one gets the approved one's id). So the
runtime gets the id it dedups on, and a write runs only with arguments an approval saw: a call whose arguments changed
after the hook (another plugin's `modify`), or that never passed the hook, finds no record. A blocked or denied call's
record is dropped; the rest expire. Standard library only.
"""
from __future__ import annotations

from dataclasses import dataclass, field
import hashlib
import json
import threading
import time
from typing import Any, Callable

TTL = 30 * 60.0  # long enough for a producer to answer an approval
PER_KEY = 64
TOTAL = 512


def digest(args: dict) -> str:
    """SHA-256 of the arguments without `preview` (C5-2). Hermes's agent loop hands the hook the model's arguments as
    written, and the handler the same arguments after its own coercion ("5" → 5, "true" → True), so numbers and truth
    values count by how they read: a coerced call matches the call the hook saw."""
    clean = {key: _plain(value) for key, value in args.items() if key != "preview"}
    text = json.dumps(clean, sort_keys=True, separators=(",", ":"), ensure_ascii=False, default=str)
    return hashlib.sha256(text.encode("utf-8")).hexdigest()


def _plain(value: Any) -> Any:
    if isinstance(value, dict):
        return {str(key): _plain(item) for key, item in value.items()}
    if isinstance(value, (list, tuple)):
        return [_plain(item) for item in value]
    if isinstance(value, bool):
        return "true" if value else "false"
    if isinstance(value, float) and value.is_integer():
        return str(int(value))
    if isinstance(value, (int, float)):
        return str(value)
    if isinstance(value, str) and value.strip().lower() in ("true", "false"):
        return value.strip().lower()
    return value


@dataclass
class Entry:
    tool_call_id: str
    at: float
    # What the hook decided for this call: whether the producer was asked (`approved`), and what the approval bound
    # (kumi_setup's plan digest).
    extra: dict = field(default_factory=dict)


class CallIds:
    def __init__(self, clock: Callable[[], float] = time.monotonic):
        self._clock = clock
        self._lock = threading.Lock()
        self._entries: dict[tuple, list[Entry]] = {}

    @staticmethod
    def key(tool: str, args: dict, context: dict) -> tuple:
        return (context.get("session_id") or None, context.get("task_id") or None, tool, digest(args))

    def record(self, tool: str, args: dict, context: dict, extra: dict | None = None) -> None:
        """The hook saw this call with `context` (Hermes's hook keywords, `tool_call_id` among them)."""
        ident = context.get("tool_call_id")
        if not isinstance(ident, str) or not ident:
            return
        with self._lock:
            now = self._clock()
            self._expire(now)
            entries = self._entries.setdefault(self.key(tool, args, context), [])
            entries.append(Entry(ident, now, dict(extra or {})))
            del entries[:-PER_KEY]
            self._trim()

    def forget(self, tool_call_id: Any) -> None:
        """The call was blocked or denied: its record goes."""
        if not isinstance(tool_call_id, str) or not tool_call_id:
            return
        with self._lock:
            for key in list(self._entries):
                kept = [entry for entry in self._entries[key] if entry.tool_call_id != tool_call_id]
                if kept:
                    self._entries[key] = kept
                else:
                    del self._entries[key]

    def take(self, tool: str, args: dict, context: dict) -> Entry | None:
        """The newest record of this call, removed: None when the hook never saw these arguments in this chat."""
        with self._lock:
            self._expire(self._clock())
            entries = self._entries.get(self.key(tool, args, context))
            if not entries:
                return None
            entry = entries.pop()
            if not entries:
                del self._entries[self.key(tool, args, context)]
            return entry

    def _expire(self, now: float) -> None:
        for key in list(self._entries):
            kept = [entry for entry in self._entries[key] if now - entry.at < TTL]
            if kept:
                self._entries[key] = kept
            else:
                del self._entries[key]

    def _trim(self) -> None:
        everything = sorted(((entry.at, key, entry) for key, entries in self._entries.items() for entry in entries),
                            key=lambda item: item[0])
        for _, key, entry in everything[:max(len(everything) - TOTAL, 0)]:
            self._entries[key].remove(entry)
            if not self._entries[key]:
                del self._entries[key]


TURNS_PER_CHAT = 16
CHATS = 2000


class Turns:
    """The producer's own turns, as Hermes's `pre_llm_call` names them (session id → its latest turn ids).

    Hermes's background review (and a side question, `/btw`) forks a chat after a turn: the fork has the chat's session
    id and a turn id of its own, and it never goes through `pre_llm_call` (`_persist_disabled`, Hermes v0.22.7). Its
    tool whitelist is thread-local and doesn't reach the thread a tool runs on, so its calls reach Kumi's hook, where a
    "this session" approval would let them run unasked. A turn of a chat whose turns Kumi has seen, and that isn't one
    of them, is such a fork. A chat Kumi has seen no turn of (it was enabled mid-turn) can't be told apart."""

    def __init__(self) -> None:
        self._turns: dict[str, list[str]] = {}
        self._lock = threading.Lock()

    def saw(self, session: Any, turn: Any) -> None:
        if not (isinstance(session, str) and session and isinstance(turn, str) and turn):
            return
        with self._lock:
            seen = self._turns.pop(session, [])  # (re-inserted last: the dict keeps the chats in the order last seen)
            self._turns[session] = [*seen[1 - TURNS_PER_CHAT:], turn] if turn not in seen else seen
            while len(self._turns) > CHATS:
                del self._turns[next(iter(self._turns))]

    def forked(self, session: Any, turn: Any) -> bool:
        """Whether a call of `turn` in `session` comes from a turn Hermes forked, not one of the producer's own."""
        if not (isinstance(session, str) and session and isinstance(turn, str) and turn):
            return False
        with self._lock:
            seen = self._turns.get(session)
            return seen is not None and turn not in seen
