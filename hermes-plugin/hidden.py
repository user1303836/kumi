"""The map's hidden sounds: the ones the UI found it can't play here (their file is gone, or the browser can't decode
it), kept in `<state>/map-hidden.json` so the map leaves them out quietly instead of failing on every click.

The file is the plugin's own (`kumi serve` never reads or writes it) and is never served: it holds file paths (CC-4).
`{"schema": 1, "seq", "items": [{"path", "size", "mtime", "why": "gone" | "unplayable", "at"}]}`, oldest first, at
most `MAX_ITEMS`. A sound is known by its file, as native Kumi keys a palette's: the map revision's canonical path plus
the file's size and mtime when it was hidden (None for a file that wasn't there). Sound ids change with every map
revision, so they are never kept. An entry hides its sound only while the file still reads as it did: a gone file that
is back, or a file that can't play and has since changed, shows again.

Written whole (temp file + `os.replace`) under an OS lock on `<state>/map-hidden.lock`. `dashboard/plugin_api.py`
loads this file by path, so it imports nothing else of the plugin: the map and the media guard come in as arguments.
"""

from __future__ import annotations

import contextlib
import json
import os
import re
import tempfile
import time
from pathlib import Path
from typing import Callable, Iterator

SCHEMA = 1
FILE = "map-hidden.json"
LOCK = "map-hidden.lock"
MAX_ITEMS = 20_000
# One report from the UI: what a burst of failed plays sends in one debounced batch.
MAX_IDS = 500
ID_MAX = 128
WHY = ("gone", "unplayable")
# A file of 20,000 entries is a few MB; anything much bigger isn't one this module wrote.
_FILE_MAX_BYTES = 16 * 1024 * 1024
# How long a write waits for another one (the other UI, a second dashboard) before giving up.
LOCK_WAIT_S = 2.0
_LOCK_POLL_S = 0.02
# How `_write` starts the file: enough for `seq` to read the number without parsing the rest.
_HEAD = re.compile(rb'\{"schema":1,"seq":([0-9]{1,15}),')


class Busy(Exception):
    """Another write held the lock for longer than `LOCK_WAIT_S`."""


class Invalid(ValueError):
    """A request body that isn't one of the two shapes `POST /map/hidden` takes; the message says why."""


def _now_ms() -> int:
    return int(time.time() * 1000)


def _whole(value: object) -> bool:
    return isinstance(value, int) and not isinstance(value, bool)


def _item(value: object) -> dict | None:
    if not isinstance(value, dict):
        return None
    path, why = value.get("path"), value.get("why")
    size, mtime, at = value.get("size"), value.get("mtime"), value.get("at")
    if not isinstance(path, str) or not path or why not in WHY:
        return None
    if not (size is None or _whole(size)) or not (mtime is None or _whole(mtime)):
        return None
    return {"path": path, "size": size, "mtime": mtime, "why": why, "at": at if _whole(at) else 0}


def read(state: Path) -> dict:
    """The file as `{"seq", "items"}`; a missing, unreadable or foreign one reads as empty at seq 0."""
    try:
        with open(state / FILE, "rb") as file:
            raw = file.read(_FILE_MAX_BYTES + 1)
        data = json.loads(raw) if len(raw) <= _FILE_MAX_BYTES else None
    except (OSError, ValueError, RecursionError):
        data = None
    if not isinstance(data, dict) or data.get("schema") != SCHEMA:
        return {"seq": 0, "items": []}
    seq = data.get("seq")
    items = data.get("items")
    kept = [item for item in (_item(value) for value in items) if item is not None] if isinstance(items, list) else []
    return {"seq": seq if _whole(seq) and seq >= 0 else 0, "items": kept}


def seq(state: Path) -> int:
    """The file's seq, for `/status`'s revisions. `/status` is polled, so the head this module writes is read alone
    rather than parsing up to 20,000 entries; a file in any other shape is read whole."""
    try:
        with open(state / FILE, "rb") as file:
            head = file.read(64)
    except OSError:
        return 0
    found = _HEAD.match(head)
    return int(found.group(1)) if found else read(state)["seq"]


@contextlib.contextmanager
def _locked(state: Path) -> Iterator[None]:
    """An exclusive OS lock on `<state>/map-hidden.lock` (flock, or msvcrt on Windows), released by the OS if the
    process dies. Waits up to `LOCK_WAIT_S`: writes are tiny, so a held lock is about to be free."""
    state.mkdir(parents=True, exist_ok=True)
    with open(state / LOCK, "a+b") as handle:  # the file stays; only the lock on it comes and goes
        deadline = time.monotonic() + LOCK_WAIT_S
        while True:
            try:
                if os.name == "nt":
                    import msvcrt
                    handle.seek(0)
                    msvcrt.locking(handle.fileno(), msvcrt.LK_NBLCK, 1)
                else:
                    import fcntl
                    fcntl.flock(handle.fileno(), fcntl.LOCK_EX | fcntl.LOCK_NB)
                break
            except OSError:
                if time.monotonic() >= deadline:
                    raise Busy from None
                time.sleep(_LOCK_POLL_S)
        try:
            yield
        finally:
            if os.name == "nt":
                handle.seek(0)
                msvcrt.locking(handle.fileno(), msvcrt.LK_UNLCK, 1)
            else:
                fcntl.flock(handle.fileno(), fcntl.LOCK_UN)


def _write(state: Path, seq_now: int, items: list[dict]) -> None:
    fd, temporary = tempfile.mkstemp(prefix=".map-hidden-", dir=state)
    try:
        with os.fdopen(fd, "w", encoding="utf-8") as file:
            json.dump({"schema": SCHEMA, "seq": seq_now, "items": items}, file, ensure_ascii=False, separators=(",", ":"))
        os.replace(temporary, state / FILE)
    except BaseException:
        Path(temporary).unlink(missing_ok=True)
        raise


def parse(body: object) -> tuple[str, str | None, list[tuple[str, str]]]:
    """`("clear", None, [])` or `("hide", revision, [(id, why), …])`; `Invalid` for anything else."""
    if not isinstance(body, dict):
        raise Invalid("The body must be a JSON object.")
    if "action" in body:
        if body.get("action") != "clear" or set(body) != {"action"}:
            raise Invalid('The only action is "clear", on its own.')
        return "clear", None, []
    if set(body) != {"revision", "ids"}:
        raise Invalid('The body is {"revision", "ids"} or {"action": "clear"}.')
    revision, ids = body["revision"], body["ids"]
    if not isinstance(revision, str) or not 0 < len(revision) <= ID_MAX:
        raise Invalid(f"revision must be a map revision's id (1-{ID_MAX} characters).")
    if not isinstance(ids, list) or not 0 < len(ids) <= MAX_IDS:
        raise Invalid(f"ids must list 1 to {MAX_IDS} sounds.")
    reports = []
    for each in ids:
        if not isinstance(each, dict) or set(each) != {"id", "why"}:
            raise Invalid('Each of ids is {"id", "why"}.')
        sound_id, why = each["id"], each["why"]
        if not isinstance(sound_id, str) or not 0 < len(sound_id) <= ID_MAX:
            raise Invalid(f"Each id is a sound id of 1-{ID_MAX} characters.")
        if why not in WHY:
            raise Invalid('Each why is "gone" or "unplayable".')
        reports.append((sound_id, why))
    return "hide", revision, reports


def _identity(path: str, checked: Callable[[str], str | None]) -> tuple[int, int] | None:
    """The file's size and whole-second mtime now, or None when it isn't there (or is a path never looked up, such as
    a network one: `checked` is the media guard's own path check, so nothing here mounts a share)."""
    if checked(path) is None:
        return None
    try:
        info = os.stat(path)
    except (OSError, ValueError):
        return None
    return info.st_size, int(info.st_mtime)


def _paths(sounds: object) -> dict[str, str]:
    """A map revision's `{sound_id: path}`, from its private `sounds`."""
    if not isinstance(sounds, dict):
        return {}
    return {sound_id: entry["path"] for sound_id, entry in sounds.items()
            if isinstance(entry, dict) and isinstance(entry.get("path"), str) and entry["path"]}


def hiding(state: Path, sounds: object, checked: Callable[[str], str | None]) -> set[str]:
    """The ids among a revision's private `sounds` that are hidden now. Entries are indexed by path, and only the
    files of the revision's sounds that have an entry are looked at, so a 25,000-sound map costs a dict lookup per
    sound and one `stat` per hidden one."""
    items = read(state)["items"]
    if not items:
        return set()
    entries = {item["path"]: item for item in items}
    paths = _paths(sounds)
    wanted = {path for path in paths.values() if path in entries}
    still = set()
    for path in wanted:
        entry = entries[path]
        now = _identity(path, checked)
        was = None if entry["size"] is None or entry["mtime"] is None else (entry["size"], entry["mtime"])
        if now == was:
            still.add(path)
    return {sound_id for sound_id, path in paths.items() if path in still}


def visible(public: dict, sounds: object, state: Path, checked: Callable[[str], str | None]) -> dict:
    """`public` (a map revision as a client sees it) without its hidden points and their sounds, with `hidden`: how
    many points were left out. Each group's `count` drops by its hidden points so the legend matches the map."""
    hidden_ids = hiding(state, sounds, checked)
    out = dict(public)
    points = public.get("points")
    if not hidden_ids or not isinstance(points, list):
        out["hidden"] = 0
        return out
    kept, dropped = [], {}
    for point in points:
        if isinstance(point, list) and point and point[0] in hidden_ids:
            group = str(point[3]) if len(point) > 3 else None
            dropped[group] = dropped.get(group, 0) + 1
        else:
            kept.append(point)
    out["points"] = kept
    if isinstance(public.get("sounds"), dict):
        out["sounds"] = {key: value for key, value in public["sounds"].items() if key not in hidden_ids}
    if isinstance(public.get("groups"), dict):
        groups = {}
        for key, group in public["groups"].items():
            if isinstance(group, dict) and _whole(group.get("count")) and dropped.get(str(key)):
                group = {**group, "count": max(0, group["count"] - dropped[str(key)])}
            groups[key] = group
        out["groups"] = groups
    out["hidden"] = len(points) - len(kept)
    return out


def _hidden_points(points: object, hidden_ids: set[str]) -> int:
    if not isinstance(points, list):
        return 0
    return sum(1 for point in points if isinstance(point, list) and point and point[0] in hidden_ids)


def hide(state: Path, current: str | None, revision: dict | None, revision_id: str, reports: list[tuple[str, str]],
         servable: Callable[[str], bool], checked: Callable[[str], str | None], now_ms: int | None = None) -> dict:
    """Hide the reported sounds of the current map revision (`current`, read as `revision`, its private file). An id
    of any other revision is ignored, as is one the revision doesn't have. `gone` is taken only when the media guard
    can't serve that sound either (`servable`): otherwise the client was wrong, or the failure was passing. `unplayable`
    is taken as reported: only the browser knows what it can decode. Answers `{"hidden": points of the current
    revision now hidden, "ignored", "seq"}`."""
    sounds = (revision or {}).get("sounds")
    paths = _paths(sounds) if current is not None and revision_id == current else {}
    taken: dict[str, dict] = {}
    ignored = 0
    at = _now_ms() if now_ms is None else now_ms
    for sound_id, why in reports:
        path = paths.get(sound_id)
        if path is None or (why == "gone" and servable(sound_id)):
            ignored += 1
            continue
        now = _identity(path, checked)
        size, mtime = now if now is not None else (None, None)
        taken[path] = {"path": path, "size": size, "mtime": mtime, "why": why, "at": at}
    with _locked(state):
        data = read(state)
        seq_now = data["seq"]
        if taken:
            # The newest report about a file replaces an older one and goes to the end; the oldest go first.
            items = [item for item in data["items"] if item["path"] not in taken] + list(taken.values())
            seq_now += 1
            _write(state, seq_now, items[-MAX_ITEMS:])
    hidden_now = _hidden_points((revision or {}).get("points"), hiding(state, sounds, checked)) if current is not None else 0
    return {"hidden": hidden_now, "ignored": ignored, "seq": seq_now}


def clear(state: Path) -> dict:
    """Show every hidden sound again."""
    with _locked(state):
        seq_now = read(state)["seq"] + 1
        _write(state, seq_now, [])
    return {"hidden": 0, "ignored": 0, "seq": seq_now}
