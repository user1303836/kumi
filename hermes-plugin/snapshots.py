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
import os
import re
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


def state_dir() -> Path:
    """This profile's state folder, resolved on every call (the profile can change between requests)."""
    from plugins.plugin_storage import plugin_data_dir

    return plugin_data_dir("kumi") / "state"


def _now_ms() -> int:
    return int(time.time() * 1000)


def _load(path: Path) -> Any:
    try:
        if path.stat().st_size > SNAPSHOT_MAX_BYTES:
            return None
        with path.open("rb") as file:
            return json.load(file)
    except (OSError, ValueError):
        return None


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
    """The runtime's lease as last published, with how old its heartbeat is. Stale when missing or quiet for more
    than `STALE_AFTER_S`; a stale lease only means the UI shows last-known state, never that anyone may take over."""
    data = read(state, "owner")
    if data is None:
        return {"stale": True, "age_ms": None}
    beat = data.get("heartbeat", data.get("at"))
    now = _now_ms() if now_ms is None else now_ms
    age = now - int(beat) if isinstance(beat, (int, float)) and not isinstance(beat, bool) else None
    stale = age is None or not -STALE_AFTER_S * 1000 <= age <= STALE_AFTER_S * 1000
    return {**_public(data), "stale": stale, "age_ms": age}


def _liveness(state: Path, now_ms: int | None) -> dict:
    own = owner(state, now_ms)
    return {"stale": own["stale"], "age_ms": own["age_ms"], "owner": own.get("owner")}


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
    """A sound of the current map revision, with its id and revision."""
    revision = map_current(state)
    current = map_revision(state, revision)
    sounds = (current or {}).get("sounds")
    entry = sounds.get(sound_id) if isinstance(sounds, dict) else None
    if not isinstance(entry, dict):
        return None
    return {**entry, "sound_id": sound_id, "revision": revision}


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
    """The file behind a media id: an artifact (artifacts.json) or a sound of the current map revision, and only
    when its real path is a regular file inside the plugin data or an approved sample folder, under the size cap.
    `{id, path, kind, mime, bytes}`, or None."""
    entry: dict | None = _artifacts(state).get(media_id)
    if entry is None:
        found = sound(state, media_id)
        if found is not None:
            entry = {"path": found.get("path"), "kind": "audio", "mime": found.get("mime")}
    path = entry.get("path") if entry else None
    if not isinstance(path, str) or not path:
        return None
    roots = [Path(os.path.realpath(state.parent))]
    roots += [Path(os.path.realpath(os.path.expanduser(folder))) for folder in library_folders or [] if isinstance(folder, str) and folder]
    real = os.path.realpath(path)
    if not _inside(real, roots) or not os.path.isfile(real):
        return None
    size = os.path.getsize(real)
    if size > MEDIA_MAX_BYTES:
        return None
    mime = entry.get("mime") if isinstance(entry.get("mime"), str) else _mime_of(real)
    return {"id": media_id, "path": real, "kind": entry.get("kind") or "audio", "mime": mime, "bytes": size}


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
