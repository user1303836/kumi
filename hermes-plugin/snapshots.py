"""Read the state Kumi's runtime publishes for the plugin's other halves (contract C3).

`kumi serve` writes these files under the profile's plugin data, each by atomic replace, with
`{"schema": 1, "seq", "at", "owner"}`. The UI backend (`dashboard/plugin_api.py`, which loads this file by path) and
the FOCUS hook (`context.py`) only read them: nothing here writes, spawns or reaches the runtime, and nothing raises on
bad data. A missing, unreadable or foreign file reads as absent.

Module-level imports stay in the standard library: `register()` runs with only Hermes on the path, so Hermes's own
helpers are imported where they're used.
"""

from __future__ import annotations

import json
import math
import os
import re
import stat
import time
from pathlib import Path
from typing import Any

SCHEMA = 1
# A heartbeat older than this makes the state stale: shown as last known, never acted on (C3).
STALE_AFTER_S = 10
HISTORY_MAX = 200
# The largest file a media route hands over (C4).
MEDIA_MAX_BYTES = 50 * 1024 * 1024
# The largest snapshot read: a big library's map is a few MB.
SNAPSHOT_MAX_BYTES = 64 * 1024 * 1024
# Ids that also name files (map revisions, jobs): no separators, no leading dot.
_FILE_ID = re.compile(r"[A-Za-z0-9][A-Za-z0-9_.-]{0,127}")
# The media types a media id is served as: audio, and the listen card's spectrogram.
_MEDIA_TYPE = re.compile(r"audio/[a-z0-9][a-z0-9.+-]{0,40}|image/png")


def state_dir() -> Path:
    """This profile's state folder, resolved on every call (the profile can change between requests)."""
    from plugins.plugin_storage import plugin_data_dir

    return plugin_data_dir("kumi") / "state"


def _now_ms() -> int:
    return int(time.time() * 1000)


# What the runtime writes is never wider or deeper than this: a file that is isn't one of its snapshots.
_WHOLE_MAX = 2**64
_DEPTH_MAX = 64
# A time in ms the UI can do arithmetic with (JavaScript's exact integers).
_MS_MAX = 2**53


def _no_constant(name: str) -> Any:
    raise ValueError(f"{name} is no number the runtime writes")


def _finite(text: str) -> float:
    value = float(text)
    if not math.isfinite(value):
        raise ValueError(f"{text} is no finite number")
    return value


def _whole(text: str) -> int:
    value = int(text)
    if not -_WHOLE_MAX < value < _WHOLE_MAX:
        raise ValueError("a number wider than the runtime writes")
    return value


def _shallow(value: Any) -> bool:
    """Whether `value` nests no deeper than `_DEPTH_MAX`, checked without recursion (a deeper document would parse,
    then fail whatever encodes it again)."""
    stack = [(value, 1)]
    while stack:
        node, depth = stack.pop()
        children = node.values() if isinstance(node, dict) else node if isinstance(node, list) else ()
        for child in children:
            if isinstance(child, (dict, list)):
                if depth >= _DEPTH_MAX:
                    return False
                stack.append((child, depth + 1))
    return True


def _load(path: Path) -> Any:
    """A JSON file, or None when it's missing, too big or not what the runtime writes: NaN, Infinity, numbers too big
    to be finite or wider than 64 bits, and nesting deeper than `_DEPTH_MAX` included (no route could answer with
    them)."""
    try:
        if path.stat().st_size > SNAPSHOT_MAX_BYTES:
            return None
        with path.open("rb") as file:
            data = json.load(file, parse_constant=_no_constant, parse_float=_finite, parse_int=_whole)
    except (OSError, ValueError, RecursionError):
        return None
    return data if _shallow(data) else None


def _ms(value: object) -> int | None:
    """A time or duration in ms, when `value` is one the UI can count with."""
    if isinstance(value, bool) or not isinstance(value, (int, float)):
        return None
    if isinstance(value, float) and not math.isfinite(value):
        return None
    return int(value) if -_MS_MAX < value < _MS_MAX else None


def read(state: Path, name: str) -> dict | None:
    """One snapshot (`status`, `focus`, `map/r7`, `jobs/j3`, …), or None when it's missing or not schema 1."""
    data = _load(state / f"{name}.json")
    if not isinstance(data, dict) or data.get("schema") != SCHEMA:
        return None
    return data


def _file_id(value: object) -> str | None:
    return value if isinstance(value, str) and _FILE_ID.fullmatch(value) and ".." not in value else None


def _public(data: dict | None) -> dict:
    """A snapshot's own fields, without its envelope's schema number."""
    return {key: value for key, value in (data or {}).items() if key != "schema"}


def owner(state: Path, now_ms: int | None = None) -> dict:
    """The runtime's lease as last published, with how old its heartbeat is. Stale when missing, stopped (its last
    write says `running: false`) or quiet for more than `STALE_AFTER_S`; a stale lease only means the UI shows
    last-known state, never that anyone may take over."""
    data = read(state, "owner")
    if data is None:
        return {"stale": True, "age_ms": None}
    beat = _ms(data.get("heartbeat", data.get("at")))
    now = _now_ms() if now_ms is None else now_ms
    age = now - beat if beat is not None else None
    quiet = age is None or not -STALE_AFTER_S * 1000 <= age <= STALE_AFTER_S * 1000
    return {**_public(data), "stale": quiet or data.get("running") is False, "age_ms": age}


def _liveness(state: Path, now_ms: int | None) -> dict:
    """How fresh the runtime is, from owner.json; its `owner` too when it names one (a snapshot keeps its own else)."""
    own = owner(state, now_ms)
    live = {"stale": own["stale"], "age_ms": own["age_ms"]}
    if isinstance(own.get("owner"), dict):
        live["owner"] = own["owner"]
    return live


def map_current(state: Path) -> str | None:
    """The current map revision's id (`map/current` holds it)."""
    try:
        with (state / "map" / "current").open("r", encoding="utf-8") as file:
            return _file_id(file.read(256).strip())
    except (OSError, ValueError):
        return None


def _seq(data: dict | None) -> int | None:
    value = (data or {}).get("seq")
    return value if isinstance(value, int) and not isinstance(value, bool) else None


def status(state: Path, now_ms: int | None = None) -> dict:
    """status.json as the UI shows it: connected only while the owner's heartbeat is fresh, and each snapshot's
    revision, so a client that missed an event still notices what changed."""
    live = _liveness(state, now_ms)
    data = read(state, "status")
    out = _public(data)
    out.update(live)
    out["connected"] = bool(out.get("connected")) and not live["stale"]
    out["revisions"] = {
        "status": _seq(data),
        "focus": _seq(read(state, "focus")),
        "history": _seq(read(state, "history")),
        "map": map_current(state),
    }
    return out


def focus(state: Path, now_ms: int | None = None) -> dict:
    """focus.json: the project, what's in focus, the transport and the catch-up, as last published."""
    live = _liveness(state, now_ms)
    data = read(state, "focus")
    out = _public(data)
    out.update(live)
    out["connected"] = not live["stale"] and data is not None
    return out


def history(state: Path, limit: object = 20, now_ms: int | None = None) -> dict:
    """HISTORY, newest first. Without a live runtime nothing can be undone, so `undo_available` reads false then."""
    live = _liveness(state, now_ms)
    data = read(state, "history")
    raw = (data or {}).get("items")
    items = [dict(item) for item in raw if isinstance(item, dict)] if isinstance(raw, list) else []
    try:
        count = max(1, min(int(limit), HISTORY_MAX))  # type: ignore[call-overload]
    except (TypeError, ValueError):
        count = 20
    items = items[:count]
    if live["stale"]:
        for item in items:
            item["undo_available"] = False
    return {"items": items, "seq": _seq(data), "connected": not live["stale"] and data is not None, **live}


def map_revision(state: Path, revision: str | None = None) -> dict | None:
    """One map revision (the current one when none is named)."""
    name = _file_id(revision) if revision is not None else map_current(state)
    return read(state, f"map/{name}") if name else None


def sound(state: Path, sound_id: str) -> dict | None:
    """A sound of the current map revision, with its id and revision. Carries the backend-only `path` (CC-4): for
    anything that leaves the backend, use `public_sound`."""
    revision = map_current(state)
    current = map_revision(state, revision)
    sounds = (current or {}).get("sounds")
    entry = sounds.get(sound_id) if isinstance(sounds, dict) else None
    if not isinstance(entry, dict):
        return None
    return {**entry, "sound_id": sound_id, "revision": revision}


def public_sound(state: Path, sound_id: str) -> dict | None:
    """`sound()` without the file's path, which stays on the backend (CC-4)."""
    found = sound(state, sound_id)
    return None if found is None else {key: value for key, value in found.items() if key != "path"}


def public_map(state: Path, revision: str | None = None) -> dict | None:
    """A map revision as a client may see it: its sounds without their paths (CC-4), and without the envelope's
    schema number."""
    data = map_revision(state, revision)
    if data is None:
        return None
    sounds = data.get("sounds")
    public = {key: value for key, value in data.items() if key not in ("schema", "sounds")}
    public["sounds"] = (
        {sound_id: {k: v for k, v in entry.items() if k != "path"} for sound_id, entry in sounds.items() if isinstance(entry, dict)}
        if isinstance(sounds, dict)
        else {}
    )
    return public


def job(state: Path, job_id: str) -> dict | None:
    name = _file_id(job_id)
    return read(state, f"jobs/{name}") if name else None


def _inside(path: str, roots: list[Path]) -> bool:
    for root in roots:
        try:
            Path(path).relative_to(root)
            return True
        except ValueError:
            continue
    return False


# The keys every snapshot carries around its own data.
_ENVELOPE = {"schema", "seq", "at", "owner"}


def _artifacts(state: Path) -> dict:
    """artifacts.json's table, `{artifact_id: {path, kind, mime, tool, at}}`, whether it sits under `artifacts` or
    beside the envelope's keys."""
    data = read(state, "artifacts") or {}
    table = data.get("artifacts")
    if not isinstance(table, dict):
        table = {key: value for key, value in data.items() if key not in _ENVELOPE}
    return {key: value for key, value in table.items() if isinstance(value, dict)}


def media(state: Path, media_id: str, library_folders: list[str] | None = None) -> dict | None:
    """The file behind a media id, `{id, path, kind, mime, bytes}`, or None. Only two kinds of id have one (CC-4):
    an artifact (artifacts.json) whose real path is inside `<state>/artifacts` or an approved sample folder, and a
    sound of the current map revision whose real path is inside an approved sample folder. Nothing else in the plugin
    data (Kumi's home, the runtime, setup) is ever served. The file must be a regular one, at most 50 MiB."""
    approved = [Path(os.path.realpath(os.path.expanduser(f))) for f in library_folders or [] if isinstance(f, str) and f]
    entry: dict | None = _artifacts(state).get(media_id)
    if entry is not None:
        roots = [Path(os.path.realpath(state / "artifacts")), *approved]
    else:
        found = sound(state, media_id)
        if found is None:
            return None
        entry = {"path": found.get("path"), "kind": "audio", "mime": found.get("mime")}
        roots = approved
    path = entry.get("path")
    if not isinstance(path, str) or not path:
        return None
    real = os.path.realpath(path)
    if not _inside(real, roots) or not os.path.isfile(real):
        return None
    size = os.path.getsize(real)
    if size > MEDIA_MAX_BYTES:
        return None
    mime = entry.get("mime") if isinstance(entry.get("mime"), str) else _mime_of(real)
    # Only what the players and the listen card use: never a type a browser would render as a page.
    if not isinstance(mime, str) or not _MEDIA_TYPE.fullmatch(mime):
        return None
    return {"id": media_id, "path": real, "kind": entry.get("kind") or "audio", "mime": mime, "bytes": size}


def read_media(state: Path, media_id: str, library_folders: list[str] | None = None) -> tuple[dict, bytes] | None:
    """`media()`'s file and its bytes. The resolved path is opened without following a link (a file swapped for one
    after the check is refused) and checked again on the open file: regular, within the size cap."""
    found = media(state, media_id, library_folders)
    if found is None:
        return None
    try:
        handle = os.open(found["path"], os.O_RDONLY | getattr(os, "O_NOFOLLOW", 0) | getattr(os, "O_BINARY", 0))
    except OSError:
        return None
    with os.fdopen(handle, "rb") as file:
        info = os.fstat(file.fileno())
        if not stat.S_ISREG(info.st_mode) or info.st_size > MEDIA_MAX_BYTES:
            return None
        data = file.read(MEDIA_MAX_BYTES + 1)
    if len(data) > MEDIA_MAX_BYTES:
        return None
    return {**found, "bytes": len(data)}, data


_MIMES = {
    ".wav": "audio/wav",
    ".wave": "audio/wav",
    ".aif": "audio/aiff",
    ".aiff": "audio/aiff",
    ".flac": "audio/flac",
    ".mp3": "audio/mpeg",
    ".ogg": "audio/ogg",
    ".opus": "audio/ogg",
    ".m4a": "audio/mp4",
    ".png": "image/png",
    ".mid": "audio/midi",
    ".midi": "audio/midi",
}


def _mime_of(path: str) -> str:
    return _MIMES.get(os.path.splitext(path)[1].lower(), "application/octet-stream")
