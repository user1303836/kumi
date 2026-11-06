"""CC-19: scripts in the DAW (Live's Python, REAPER's and Renoise's Lua), trusted by the chat's provenance.

A script is code with the DAW's access to the computer, so the approval is the boundary. What it takes depends on what
the chat has read:
- A chat is **clean** from its first turn until untrusted outside content enters it (19a, r2): any tool result that
  isn't Kumi's own or one of Hermes's local file, to-do, question and skill tools. That includes the web, the browser,
  video, MCP, delegated subagents, memory, past sessions, every other plugin's output, and Hermes's terminal and code
  tools, which can fetch anything (its youtube-content skill gets transcripts through the terminal): fail closed
  (rgzm's answer to the lead's open question).
- Once **tainted**, a chat stays tainted (19c). The record is kept in the plugin's data (r1), so a restart doesn't
  clean a chat. A chat Kumi first sees after its first turn (resumed after a restart or an upgrade, or after
  compaction) has no record and counts as tainted. So does a messaging chat (Discord, Slack, Telegram…), and a surface
  Hermes can't name.
- A **delegated subagent** never runs a script: its task is the model's words, not the producer's.

Each script's approval asks under `kumi:script` while the chat is clean, which Kumi's chat scoping makes
`kumi:script:<session id>`, so "Allow this session" covers the chat's next scripts. Once the chat is tainted, it asks
under `kumi:script:<sha256 of the code>`: an earlier session allow no longer matches, and "Always" re-matches only the
same code. Under Hermes's YOLO or `approvals.mode: off`, a clean chat's scripts run, as the owner uses Kumi, and a
tainted chat's are refused, since nobody reviews them.

The code is saved before anyone is asked, to `<plugin data>/state/scripts/<sha256>.py|.lua` (0600, never
overwritten), where the approval names it and Desktop's pane reads it (agent-i's route re-hashes it). A Max for Live
device's canonical spec is kept the same way, as `<sha256>.device.json` (CC-22, `devices.py`).
"""
from __future__ import annotations

import hashlib
import json
import logging
import os
from pathlib import Path
import tempfile
import threading
import time
from typing import Any

log = logging.getLogger(__name__)

# CC-19 r4/r8: one cap everywhere (serve's preview refuses past it too).
MOST_LINES = 400
MOST_BYTES = 32 * 1024
# A device's saved file is its canonical spec (CC-22's 22-r3), named by that spec's SHA-256 like a script's code.
LANGUAGES = {"python": ".py", "lua": ".lua", "device": ".device.json"}
LANGUAGE_NAMES = {"python": "Python", "lua": "Lua"}
# What each DAW runs: Live, Python in its Remote Script; REAPER and Renoise, Lua.
DAW_LANGUAGE = {"ableton": "python", "reaper": "lua", "renoise": "lua"}

# 19e, word for word: the approval, Desktop's pane (through `script.pending`) and the docs all say it this way.
WARNING = "Runs with the DAW's access to your computer. Kumi can't undo it; the DAW's own Undo may."

# Hermes's tools whose results bring nothing from outside this computer (r2): its local file tools, its to-do list,
# the producer's own answers (`clarify`) and the skills installed here (Kumi's included). Every other tool taints, its
# terminal and code tools and a new tool included, until it's listed here.
LOCAL_TOOLS = frozenset({"read_file", "write_file", "patch", "search_files", "todo_list", "clarify", "skills_list",
                         "skill_view", "skill_manage"})
# Hermes's chat surfaces where the producer is at this computer: the terminal, Desktop, and an editor (ACP). Anything
# else (a messaging app, or a surface Hermes can't name) is a messaging chat here: tainted, and refused unless
# `scripts_in_messaging` is on.
LOCAL_SURFACES = frozenset({"cli", "tui", "desktop", "acp"})

CLEAN, TAINTED, SUBAGENT = "clean", "tainted", "subagent"
NO_RECORD = "Kumi didn't see this chat start"
UNNAMED = "Hermes didn't name this chat"
MESSAGING = "it's a messaging chat"
KEPT = 4000  # sessions remembered; the oldest go first, and a forgotten chat counts as tainted (fail-closed)
# In the plugin's data: the record of which chats read outside content (outside `state/`, whose top-level JSON files are
# the runtime's snapshots), and the code each approval names, which Desktop's pane reads.
TRUST_FILE = "script-trust.json"
CODE_FOLDER = ("state", "scripts")


def asked_because(why: str, what: str = "script") -> str:
    """The approval's line saying why every script (or device, CC-22) in this chat is asked (r5)."""
    if why == MESSAGING:
        return f"This is a messaging chat, so each {what} is asked."
    if why in (NO_RECORD, UNNAMED):
        return f"{why}, so each {what} is asked."
    return f"This chat read outside content ({why}), so each {what} is asked."


def refused_because(why: str) -> str:
    """Why a chat's scripts are asked each time, inside a refusal's parentheses."""
    return why if why in (MESSAGING, NO_RECORD, UNNAMED) else f"it read outside content: {why}"


def sha256(code: str) -> str:
    return hashlib.sha256(code.encode("utf-8")).hexdigest()


def lines_of(code: str) -> int:
    """The script's lines as serve counts them: a final newline doesn't start another."""
    return len(code[:-1].split("\n")) if code.endswith("\n") else len(code.split("\n"))


def taints(tool: str) -> str | None:
    """Why a finished call of `tool` brings outside content into the chat, or None when it doesn't (r2)."""
    if not isinstance(tool, str) or not tool:
        return "a tool Kumi can't name"
    if tool.startswith("kumi_") or tool in LOCAL_TOOLS:
        return None
    return tool


class Trust:
    """Each chat's provenance (CC-19 19a–19c, r1), kept in the plugin's data so a restart doesn't clean a chat. One
    lock for the file; a failure to write leaves the chat as the stricter of what was known. Another Hermes process
    (the gateway, a terminal resuming the same chat) may write it too, so it's read again whenever it changed."""

    def __init__(self, path: Path):
        self._path = path
        self._lock = threading.Lock()
        self._sessions: dict[str, dict] | None = None
        self._read_at: tuple[int, ...] | None = None  # the file as it was when it was last read: see `_stamp`

    def _stamp(self) -> tuple[int, ...] | None:
        """Which file is there, and its size and time. Windows stamps a file's time by its clock's tick, so two writes
        can share one; each write replaces the file with a new one (`_save`), which the file's id tells apart."""
        try:
            stat = self._path.stat()
        except OSError:
            return None
        return stat.st_dev, stat.st_ino, stat.st_size, stat.st_mtime_ns

    def _load(self) -> dict[str, dict]:
        changed = self._stamp()
        if self._sessions is None or changed != self._read_at:
            try:
                data = json.loads(self._path.read_text(encoding="utf-8"))
                sessions = data.get("sessions") if isinstance(data, dict) else None
                self._sessions = {key: value for key, value in (sessions or {}).items()
                                  if isinstance(key, str) and isinstance(value, dict)
                                  and value.get("state") in (CLEAN, TAINTED, SUBAGENT)}
            except FileNotFoundError:
                self._sessions = {}
            except (OSError, ValueError):
                log.warning("Kumi couldn't read its record of which chats read outside content: every chat counts as "
                            "having read some", exc_info=True)
                self._sessions = {}
            self._read_at = changed
        return self._sessions

    def _save(self) -> None:
        sessions = self._load()
        if len(sessions) > KEPT:
            for key in sorted(sessions, key=lambda key: sessions[key].get("at", 0))[:len(sessions) - KEPT]:
                del sessions[key]
        self._path.parent.mkdir(parents=True, exist_ok=True)
        fd, temporary = tempfile.mkstemp(prefix=".script-trust-", dir=self._path.parent)
        try:
            with os.fdopen(fd, "w", encoding="utf-8") as file:
                json.dump({"version": 1, "sessions": sessions}, file, ensure_ascii=False)
            os.replace(temporary, self._path)
        except BaseException:
            Path(temporary).unlink(missing_ok=True)
            raise

    def _set(self, session: str, state: str, why: str) -> None:
        self._load()[session] = {"state": state, "why": why, "at": int(time.time() * 1000)}
        try:
            self._save()
        except OSError:
            log.warning("Kumi couldn't keep its record that a chat read outside content", exc_info=True)

    def first_turn(self, session: Any, *, surface: str) -> None:
        """A chat's first turn (Hermes's `pre_llm_call` with `is_first_turn`): clean, unless it's a messaging chat.
        A chat Kumi already knows keeps what it has."""
        if not isinstance(session, str) or not session:
            return
        with self._lock:
            if session in self._load():
                return
            if surface in LOCAL_SURFACES:
                self._set(session, CLEAN, "")
            else:
                self._set(session, TAINTED, MESSAGING)

    def taint(self, session: Any, why: str) -> None:
        if not isinstance(session, str) or not session:
            return
        with self._lock:
            known = self._load().get(session)
            if known is not None and known["state"] != CLEAN:
                return
            self._set(session, TAINTED, why)

    def subagent(self, session: Any) -> None:
        if isinstance(session, str) and session:
            with self._lock:
                self._set(session, SUBAGENT, "it's a delegated subagent")

    def state(self, session: Any) -> tuple[str, str]:
        """(clean, tainted or subagent; why). A chat Kumi has no record of counts as tainted (r1)."""
        if not isinstance(session, str) or not session:
            return TAINTED, UNNAMED
        with self._lock:
            known = self._load().get(session)
        if known is None:
            return TAINTED, NO_RECORD
        return known["state"], str(known.get("why") or "")


def save(folder: Path, code: str, language: str) -> Path:
    """The approved code at `<folder>/<sha256><ext>`, 0600, written once: a file already there with that name holds
    the same code, which is checked, never replaced."""
    digest = sha256(code)
    path = folder / f"{digest}{LANGUAGES[language]}"
    folder.mkdir(parents=True, exist_ok=True)
    try:
        fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL | getattr(os, "O_NOFOLLOW", 0), 0o600)
    except FileExistsError:
        if path.is_symlink() or not path.is_file() or sha256(path.read_text(encoding="utf-8")) != digest:
            raise OSError(f"{path} isn't the code it's named for")
        return path
    with os.fdopen(fd, "w", encoding="utf-8", newline="") as file:
        file.write(code)
    return path


def code_line(text: str, width: int) -> str:
    """One line of the code for the approval: tabs as four spaces, indentation kept, cut at `width`. Serve has refused
    every hidden, direction-changing or control character before the approval is built (r7)."""
    line = text.replace("\t", "    ").rstrip()
    return line if len(line) <= width else line[:width - 1] + "…"
