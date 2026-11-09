"""The producer's palettes: the sounds they gathered to work with, shared by the Desktop and web map UI (the dashboard's
`/palette` routes) and the agent (`kumi_palette`, and the turn's note).

`kumi serve` never opens native Kumi's database, so the plugin keeps its own palettes, with native Kumi's semantics
(`crates/kumi-agent/src/core/palette.rs`): one file, `<state>/palettes.json`, plugin-owned and never served, written
whole (temp file + `os.replace`) under an OS lock on `<state>/palettes.lock`. Every write bumps its `seq`.

An item is a map sound, by its file's canonical path, size and mtime as native keys it, or an artifact (a sound-design
result) by its id. Map sound ids belong to one map revision, so ids come in and go out mapped through the current
revision's private file, and no path ever leaves this module.

The dashboard loads this file on its own by path, so it imports nothing else from the plugin: the caller passes in the
map revision's data.
"""
from __future__ import annotations

import contextlib
import json
import logging
import os
import re
import secrets
import threading
import time
from pathlib import Path
from typing import Any, Callable, Iterator

log = logging.getLogger(__name__)

SCHEMA = 1
# Native's: `/palette loose` with no number, the largest reach, and the most files one gather takes.
LOOSE = 5
FARTHEST = 50
MOST_GATHERED = 10_000
# The most artifacts one gather takes, and the longest palette name.
MOST_ARTIFACTS = 64
NAME_MAX = 80
# How long a write waits for another one's lock before it gives up.
LOCK_WAIT_S = 2.0
FILE = "palettes.json"
LOCK = "palettes.lock"

_CONTROL = re.compile(r"[\x00-\x1f\x7f-\x9f\u2028\u2029]")
PALETTE_ID = re.compile(r"pal-[0-9a-f]{12}")
ITEM_KEY = re.compile(r"it-[0-9a-f]{12}")

NONE = "There's no palette yet: gather sounds on the map, or make one with new and a name."
OFF_MAP = "Those sounds aren't on the current map: read it again."


class Invalid(ValueError):
    """A request this module refuses, in one line the producer or the model can act on."""


class NoPalette(Invalid):
    """An edit of the current palette when there's none."""


class OffMap(Invalid):
    """Sound ids that aren't the current map revision's."""


class Busy(RuntimeError):
    """Another write held the lock past `LOCK_WAIT_S`."""


def _now_ms() -> int:
    return int(time.time() * 1000)


def counted(count: int) -> str:
    return "1 sound" if count == 1 else f"{count} sounds"


def mode(reach: int) -> str:
    """Native's `PaletteView::mode`: "strict" or "loose, 5 nearest"."""
    return "strict" if reach == 0 else f"loose, {reach} nearest"


def words(palette: str, added: int, had: int) -> str:
    """Native's `Gathering::words`: "Added 3 sounds to Drums." / "Drums has that sound already." / "Added 1 sound to
    Drums (1 it had already)."."""
    if added == 0:
        return f"{palette} has {'that sound' if had == 1 else f'those {had} sounds'} already."
    if had == 0:
        return f"Added {counted(added)} to {palette}."
    return f"Added {counted(added)} to {palette} ({had} it had already)."


def clean_name(name: Any) -> str:
    """A palette's name as it's kept: trimmed, 1-80 characters, no control characters."""
    if not isinstance(name, str) or not name.strip():
        raise Invalid("A palette's name is 1 to 80 characters.")
    name = name.strip()
    if len(name) > NAME_MAX:
        raise Invalid("A palette's name is 1 to 80 characters.")
    if _CONTROL.search(name):
        raise Invalid("A palette's name can't hold control characters or line breaks.")
    return name


# The file.


def _empty() -> dict:
    return {"schema": SCHEMA, "seq": 0, "current": None, "palettes": []}


def _whole(value: Any, low: int = 0) -> int | None:
    return value if isinstance(value, int) and not isinstance(value, bool) and value >= low else None


def _item(raw: Any) -> dict | None:
    """One item as kept, or None for anything this module never writes."""
    if not isinstance(raw, dict) or not isinstance(raw.get("key"), str) or not ITEM_KEY.fullmatch(raw["key"]):
        return None
    name = raw.get("name") if isinstance(raw.get("name"), str) else ""
    kind = raw.get("class") if isinstance(raw.get("class"), str) else None
    common = {"key": raw["key"], "name": name, "class": kind, "excluded": raw.get("excluded") is True,
              "added_at": _whole(raw.get("added_at")) or 0}
    if raw.get("source") == "artifact" and isinstance(raw.get("artifact_id"), str):
        return {"key": raw["key"], "source": "artifact", "artifact_id": raw["artifact_id"], **common, "class": None}
    if raw.get("source") == "sound" and isinstance(raw.get("path"), str) and raw["path"]:
        return {"key": raw["key"], "source": "sound", "path": raw["path"], "size": _whole(raw.get("size")) or 0,
                "mtime": _whole(raw.get("mtime")) or 0, **common}
    return None


def _palette(raw: Any) -> dict | None:
    if not isinstance(raw, dict) or not isinstance(raw.get("id"), str) or not PALETTE_ID.fullmatch(raw["id"]):
        return None
    if not isinstance(raw.get("name"), str) or not raw["name"].strip():
        return None
    reach = _whole(raw.get("reach"))
    items = [item for item in map(_item, raw.get("items") or []) if item is not None] \
        if isinstance(raw.get("items"), list) else []
    return {"id": raw["id"], "name": raw["name"], "reach": reach if reach is not None and reach <= FARTHEST else 0,
            "created_at": _whole(raw.get("created_at")) or 0, "updated_at": _whole(raw.get("updated_at")) or 0,
            "items": items}


def _parse(data: Any) -> dict | None:
    if not isinstance(data, dict) or data.get("schema") != SCHEMA:
        return None
    palettes = [p for p in map(_palette, data.get("palettes") or []) if p is not None] \
        if isinstance(data.get("palettes"), list) else []
    ids = {p["id"] for p in palettes}
    current = data.get("current") if data.get("current") in ids else None
    return {"schema": SCHEMA, "seq": _whole(data.get("seq")) or 0, "current": current, "palettes": palettes}


def _read(state: Path) -> tuple[dict, bool]:
    """The file, and whether it's there but unreadable (so a write keeps it aside rather than over it)."""
    path = state / FILE
    try:
        text = path.read_text(encoding="utf-8")
    except FileNotFoundError:
        return _empty(), False
    except OSError:
        log.warning("Kumi couldn't read its palettes")
        return _empty(), True
    try:
        parsed = _parse(json.loads(text))
    except ValueError:
        parsed = None
    if parsed is None:
        log.warning("Kumi's palettes file isn't one it writes")
        return _empty(), True
    return parsed, False


def load(state: Path) -> dict:
    """The palettes as last written (an empty file's worth when there are none)."""
    return _read(state)[0]


def seq(state: Path) -> int:
    return load(state)["seq"]


def _save(state: Path, data: dict) -> None:
    path = state / FILE
    temporary = path.with_name(f".{FILE}.{os.getpid()}.{threading.get_ident()}.tmp")
    temporary.write_text(json.dumps(data, ensure_ascii=False, separators=(",", ":")), encoding="utf-8")
    os.replace(temporary, path)


@contextlib.contextmanager
def _locked(state: Path) -> Iterator[None]:
    """An exclusive OS lock on `<state>/palettes.lock` (flock, or msvcrt on Windows), released by the OS if the process
    dies. It waits up to `LOCK_WAIT_S` for another write."""
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
                    raise Busy("Kumi's palette is busy with another change; try again.") from None
                time.sleep(0.02)
        try:
            yield
        finally:
            if os.name == "nt":
                handle.seek(0)
                msvcrt.locking(handle.fileno(), msvcrt.LK_UNLCK, 1)
            else:
                fcntl.flock(handle.fileno(), fcntl.LOCK_UN)


def _write(state: Path, change: Callable[[dict], str]) -> str:
    """`change` applied to the file under its lock, then the file written whole with its seq bumped. What `change`
    raises leaves the file as it was."""
    with _locked(state):
        data, unreadable = _read(state)
        said = change(data)
        if unreadable:  # keep what couldn't be read beside the new file, never under it
            with contextlib.suppress(OSError):
                os.replace(state / FILE, state / f"{FILE}.unreadable")
        data["seq"] += 1
        _save(state, data)
    return said


def _new_id(prefix: str) -> str:
    return f"{prefix}-{secrets.token_hex(6)}"


def _current(data: dict) -> dict | None:
    return next((p for p in data["palettes"] if p["id"] == data["current"]), None)


def _named(data: dict, name: str) -> dict | None:
    wanted = name.strip().casefold()
    return next((p for p in data["palettes"] if p["name"].casefold() == wanted), None)


def _which(data: dict, which: Any) -> dict | None:
    """A palette by its id, else by its name without regard to case."""
    if not isinstance(which, str) or not which.strip():
        return None
    return next((p for p in data["palettes"] if p["id"] == which), None) or _named(data, which)


def _next_name(data: dict) -> str:
    number = 1
    while _named(data, f"Palette {number}") is not None:
        number += 1
    return f"Palette {number}"


def _make(data: dict, name: str, now: int) -> dict:
    palette = {"id": _new_id("pal"), "name": name, "reach": 0, "created_at": now, "updated_at": now, "items": []}
    data["palettes"].append(palette)
    return palette


def _held(palette: dict) -> int:
    return sum(1 for item in palette["items"] if not item["excluded"])


def _touch(palette: dict, now: int) -> None:
    palette["updated_at"] = max(palette["updated_at"], now)


# The map: a revision's private file, read once per revision.


def _file_name(path: str) -> str:
    return re.split(r"[\\/]", path)[-1]


class Map:
    """A map revision's sounds as this module needs them: id → its entry and group, and path → id. Built from the
    revision's private file (`state/map/<rev>.json`, its `sounds[id].path` and `points[i][3]`)."""

    def __init__(self, revision: str, data: dict):
        self.revision = revision
        sounds = data.get("sounds") if isinstance(data.get("sounds"), dict) else {}
        self.sounds: dict[str, dict] = {key: value for key, value in sounds.items()
                                        if isinstance(value, dict) and isinstance(value.get("path"), str)}
        self.groups: dict[str, int] = {}
        for point in data.get("points") if isinstance(data.get("points"), list) else []:
            if isinstance(point, list) and len(point) >= 4 and isinstance(point[0], str) and _whole(point[3]) is not None:
                self.groups[point[0]] = point[3]
        self.by_path: dict[str, str] = {entry["path"]: key for key, entry in self.sounds.items()}
        self._by_file_name: dict[str, list[str]] | None = None

    def named_like(self, path: str) -> list[str]:
        """The ids of the sounds whose file has `path`'s file name."""
        if self._by_file_name is None:
            found: dict[str, list[str]] = {}
            for key, entry in self.sounds.items():
                found.setdefault(_file_name(entry["path"]), []).append(key)
            self._by_file_name = found
        return self._by_file_name.get(_file_name(path), [])


_MAPS: dict[tuple, Map] = {}
_MAPS_KEPT = 4
_MAPS_LOCK = threading.Lock()


def current_map(state: Path, revision: str | None, read_map: Callable[[str], dict | None]) -> Map | None:
    """The map revision `revision`, built once per revision (and again only if its file was rewritten). `read_map`
    reads the revision's private file; it's called only when the revision isn't built yet."""
    if not revision:
        return None
    try:
        stat = (state / "map" / f"{revision}.json").stat()
        mark = (str(state), revision, stat.st_mtime_ns, stat.st_size)
    except OSError:
        return None
    with _MAPS_LOCK:
        built = _MAPS.get(mark)
    if built is not None:
        return built
    data = read_map(revision)
    if not isinstance(data, dict):
        return None
    built = Map(revision, data)
    with _MAPS_LOCK:
        _MAPS[mark] = built
        while len(_MAPS) > _MAPS_KEPT:
            _MAPS.pop(next(iter(_MAPS)))
    return built


def _file_key(path: str) -> tuple[int, int]:
    """A file's size and mtime in ms, as native keys an item; 0 and 0 for one that can't be read (as native)."""
    try:
        info = os.stat(path)
    except OSError:
        return 0, 0
    return info.st_size, round(info.st_mtime_ns / 1_000_000)


def _on_map(the_map: Map | None, sound_ids: list[str]) -> list[str]:
    """The paths of `sound_ids`, every one a sound of the current map, or OffMap."""
    if the_map is None or any(not isinstance(i, str) or i not in the_map.sounds for i in sound_ids):
        raise OffMap(OFF_MAP)
    return [the_map.sounds[i]["path"] for i in sound_ids]


# What the UI and the agent see: items mapped to the current revision, never a path.


def _resolved(item: dict, the_map: Map | None) -> dict:
    sound_id = the_map.by_path.get(item["path"]) if item["source"] == "sound" and the_map is not None else None
    entry = the_map.sounds[sound_id] if sound_id else {}
    return {"key": item["key"], "source": item["source"], "sound_id": sound_id,
            "artifact_id": item.get("artifact_id"), "name": item["name"], "class": item["class"],
            "group": the_map.groups.get(sound_id) if sound_id else None,
            "folder": entry.get("folder") if isinstance(entry.get("folder"), str) else None,
            "excluded": item["excluded"]}


def refound(state: Path, the_map: Map | None) -> None:
    """Native's refound: a sound item whose file is gone is looked for among the current map's sounds by its file
    name and size; a match takes the new path and mtime. Nothing is ever pruned: one not found stays as it was. Only
    items off the map are looked at (one on it is where the map says), and the file is written only for a match."""
    if the_map is None:
        return
    found: dict[str, tuple[str, str, int]] = {}  # key → (old path, new path, new mtime)
    for palette in load(state)["palettes"]:
        for item in palette["items"]:
            if item["source"] != "sound" or item["size"] <= 0 or item["path"] in the_map.by_path:
                continue
            if os.path.exists(item["path"]):
                continue
            for candidate in the_map.named_like(item["path"]):
                path = the_map.sounds[candidate]["path"]
                size, mtime = _file_key(path)
                if size == item["size"]:
                    found[item["key"]] = (item["path"], path, mtime)
                    break
    if not found:
        return

    def change(data: dict) -> str:
        for palette in data["palettes"]:
            for item in palette["items"]:
                moved = found.get(item["key"])
                if moved is not None and item.get("path") == moved[0]:
                    item["path"], item["mtime"] = moved[1], moved[2]
        return ""

    _write(state, change)


def view(state: Path, the_map: Map | None) -> dict:
    """`GET /palette`'s answer: the seq, the map revision ids are for, the current palette with its items in order
    (an excluded one isn't among them, only counted), and every palette. Moved files are re-found first."""
    try:
        refound(state, the_map)
    except Busy:
        pass  # the next read looks again
    data = load(state)
    palette = _current(data)
    current = None
    if palette is not None:
        items = [_resolved(item, the_map) for item in palette["items"] if not item["excluded"]]
        current = {"id": palette["id"], "name": palette["name"], "reach": palette["reach"],
                   "mode": mode(palette["reach"]), "excluded": len(palette["items"]) - len(items),
                   "items": [{key: item[key] for key in ("key", "source", "sound_id", "artifact_id", "name", "class",
                                                         "group")} for item in items]}
    return {"seq": data["seq"], "map_revision": the_map.revision if the_map else None, "current": current,
            "palettes": [{"id": p["id"], "name": p["name"], "items": _held(p), "current": p["id"] == data["current"]}
                         for p in data["palettes"]]}


def current(state: Path, the_map: Map | None) -> dict | None:
    """The current palette for the agent: `{id, name, reach, mode, items, excluded}`, its items mapped to the current
    revision (`{key, source, sound_id, artifact_id, name, class, group, folder}`), never a path; None without one."""
    with contextlib.suppress(Busy):  # the next read looks again
        refound(state, the_map)
    data = load(state)
    palette = _current(data)
    if palette is None:
        return None
    resolved = [_resolved(item, the_map) for item in palette["items"]]
    return {"id": palette["id"], "name": palette["name"], "reach": palette["reach"], "mode": mode(palette["reach"]),
            "items": [item for item in resolved if not item["excluded"]],
            "excluded": [item for item in resolved if item["excluded"]],
            "palettes": [{"name": p["name"], "items": _held(p), "current": p["id"] == data["current"]}
                         for p in data["palettes"]]}


def listed(state: Path) -> list[dict]:
    data = load(state)
    return [{"name": p["name"], "items": _held(p), "current": p["id"] == data["current"]} for p in data["palettes"]]


# Writes, each in one line of words.


def gather(state: Path, the_map: Map | None, sound_ids: list[str] = (), artifacts: list[tuple[str, str]] = ()) -> str:
    """Sounds of the current map (by id) and artifacts (`(id, name)`, each one the media guard serves) into the current
    palette, or a new `Palette N` when there's none. A file it has already (same path, size and mtime) counts as had;
    an excluded one comes back in. One gather takes at most 10,000 sounds and 64 artifacts."""
    sound_ids, artifacts = list(sound_ids), list(artifacts)
    if not sound_ids and not artifacts:
        raise Invalid("Gather takes sound ids or artifact ids.")
    if len(artifacts) > MOST_ARTIFACTS:
        raise Invalid(f"One gather takes at most {MOST_ARTIFACTS} artifacts.")
    picked = len(sound_ids)
    sound_ids = sound_ids[:MOST_GATHERED]
    paths = _on_map(the_map, sound_ids) if sound_ids else []
    entries = [the_map.sounds[i] for i in sound_ids] if sound_ids else []
    keys = [_file_key(path) for path in paths]  # outside the lock: stat can be slow on a big gather

    def change(data: dict) -> str:
        now = _now_ms()
        palette = _current(data) or _make(data, _next_name(data), now)
        data["current"] = palette["id"]
        had_file = {(i["path"], i["size"], i["mtime"]): i for i in palette["items"] if i["source"] == "sound"}
        had_artifact = {i["artifact_id"]: i for i in palette["items"] if i["source"] == "artifact"}
        added = had = 0
        at = now
        for path, (size, mtime), entry in zip(paths, keys, entries):
            own = had_file.get((path, size, mtime))
            if own is not None and not own["excluded"]:
                had += 1
                continue
            if own is not None:
                own["excluded"] = False
            else:
                own = {"key": _new_id("it"), "source": "sound", "path": path, "size": size, "mtime": mtime,
                       "name": entry.get("name") if isinstance(entry.get("name"), str) else _file_name(path),
                       "class": entry.get("class") if isinstance(entry.get("class"), str) else None,
                       "excluded": False, "added_at": at}
                at += 1
                palette["items"].append(own)
                had_file[(path, size, mtime)] = own
            added += 1
        for artifact_id, name in artifacts:
            own = had_artifact.get(artifact_id)
            if own is not None and not own["excluded"]:
                had += 1
                continue
            if own is not None:
                own["excluded"] = False
            else:
                own = {"key": _new_id("it"), "source": "artifact", "artifact_id": artifact_id, "name": name,
                       "class": None, "excluded": False, "added_at": at}
                at += 1
                palette["items"].append(own)
                had_artifact[artifact_id] = own
            added += 1
        _touch(palette, now)
        said = words(palette["name"], added, had)
        if picked > MOST_GATHERED:
            said += f" The first {MOST_GATHERED:,} of {picked:,} picked went in."
        return said

    return _write(state, change)


def _needs_current(data: dict) -> dict:
    palette = _current(data)
    if palette is None:
        raise NoPalette(NONE)
    return palette


def remove_keys(state: Path, keys: list[str]) -> str:
    """Items out of the current palette by their keys (an excluded one too: it's no longer excluded, just gone)."""
    wanted = set(keys)

    def change(data: dict) -> str:
        palette = _needs_current(data)
        before = len(palette["items"])
        palette["items"] = [item for item in palette["items"] if item["key"] not in wanted]
        removed = before - len(palette["items"])
        if not removed:
            raise Invalid(f"{palette['name']} has none of those sounds.")
        _touch(palette, _now_ms())
        return f"Took {counted(removed)} out of {palette['name']}."

    return _write(state, change)


def remove_sounds(state: Path, the_map: Map | None, sound_ids: list[str]) -> int:
    """Sounds of the current map out of the current palette (every size and mtime of their files, as native's
    `remove_paths`): how many it had."""
    paths = set(_on_map(the_map, sound_ids))
    removed = 0

    def change(data: dict) -> str:
        nonlocal removed
        palette = _needs_current(data)
        kept = [item for item in palette["items"]
                if item["excluded"] or item["source"] != "sound" or item["path"] not in paths]
        removed = len(palette["items"]) - len(kept)
        palette["items"] = kept
        _touch(palette, _now_ms())
        return ""

    _write(state, change)
    return removed


def exclude_sounds(state: Path, the_map: Map | None, sound_ids: list[str]) -> int:
    """Sounds of the current map the current palette must never bring in (one of its own stops being one, keeping its
    place): how many were given, as native's `exclude_paths` says."""
    paths = _on_map(the_map, sound_ids)
    entries = [the_map.sounds[i] for i in sound_ids]
    keys = [_file_key(path) for path in paths]

    def change(data: dict) -> str:
        palette = _needs_current(data)
        now = _now_ms()
        for at, (path, (size, mtime), entry) in enumerate(zip(paths, keys, entries)):
            own = [item for item in palette["items"] if item["source"] == "sound" and item["path"] == path]
            if own:
                for item in own:
                    item["excluded"] = True
                continue
            palette["items"].append({"key": _new_id("it"), "source": "sound", "path": path, "size": size,
                                     "mtime": mtime, "name": entry.get("name") if isinstance(entry.get("name"), str)
                                     else _file_name(path), "class": entry.get("class")
                                     if isinstance(entry.get("class"), str) else None, "excluded": True,
                                     "added_at": now + at})
        _touch(palette, now)
        return ""

    _write(state, change)
    return len(paths)


def move(state: Path, key: str, to: int) -> str:
    """One item of the current palette to place `to` (0 first) among its items; past the end is last."""
    def change(data: dict) -> str:
        palette = _needs_current(data)
        index = next((n for n, item in enumerate(palette["items"]) if item["key"] == key), None)
        if index is None:
            raise Invalid(f"{palette['name']} has no such sound: read it again.")
        item = palette["items"].pop(index)
        # `to` counts the items the UI shows (excluded ones aren't): the place before the shown item now at `to`.
        shown = [n for n, each in enumerate(palette["items"]) if not each["excluded"]]
        place = shown[to] if to < len(shown) else len(palette["items"])
        palette["items"].insert(place, item)
        _touch(palette, _now_ms())
        return f"Moved {item['name']} to place {min(to, len(shown)) + 1} in {palette['name']}."

    return _write(state, change)


def rename(state: Path, name: Any) -> str:
    name = clean_name(name)

    def change(data: dict) -> str:
        palette = _needs_current(data)
        other = _named(data, name)
        if other is not None and other["id"] != palette["id"]:
            raise Invalid(f"There's a palette called {other['name']} already.")
        palette["name"] = name
        _touch(palette, _now_ms())
        return f"The palette is called {name} now."

    return _write(state, change)


def new(state: Path, name: Any) -> str:
    name = clean_name(name)

    def change(data: dict) -> str:
        other = _named(data, name)
        if other is not None:
            raise Invalid(f"There's a palette called {other['name']} already: use it to work with it.")
        data["current"] = _make(data, name, _now_ms())["id"]
        return f"{name} is the palette now: gather sounds on the map to add them."

    return _write(state, change)


def use(state: Path, which: Any) -> str:
    def change(data: dict) -> str:
        palette = _which(data, which)
        if palette is None:
            raise Invalid(f"There's no palette called {_shown(which)}.")
        data["current"] = palette["id"]
        return f"{palette['name']} ({counted(_held(palette))}) is the palette now."

    return _write(state, change)


def forget(state: Path, which: Any) -> str:
    def change(data: dict) -> str:
        palette = _which(data, which)
        if palette is None:
            raise Invalid(f"There's no palette called {_shown(which)}.")
        data["palettes"] = [p for p in data["palettes"] if p["id"] != palette["id"]]
        if data["current"] == palette["id"]:
            data["current"] = None
        return f"Forgot the palette {palette['name']} and its {counted(_held(palette))}."

    return _write(state, change)


def reach(state: Path, reach_: int) -> str:
    """Native's strict (0) and loose (1-50: each sound brings in its nearest)."""
    if _whole(reach_) is None or reach_ > FARTHEST:
        raise Invalid(f"Reach is 0 (strict) or how many of each sound's nearest to bring, from 1 to {FARTHEST}.")

    def change(data: dict) -> str:
        palette = _needs_current(data)
        palette["reach"] = reach_
        _touch(palette, _now_ms())
        if reach_ == 0:
            return "Strict: Kumi keeps to the palette's own sounds."
        return (f"Loose: each of the palette's sounds brings its {reach_} nearest, and Kumi says when it uses one of "
                "those.")

    return _write(state, change)


def _shown(which: Any) -> str:
    return " ".join(str(which).split())[:NAME_MAX] if isinstance(which, str) else "that"


# The turn's note.


def brief(state: Path) -> tuple[str, str] | None:
    """Native's `brief()` for the turn's note: the current palette's name, and in words how many sounds and of what
    class, strict or loose and what that means, and how many it excludes. None without a palette or with nothing in
    it. The name is the caller's to quote; classes are cleaned and bounded here."""
    palette = _current(load(state))
    if palette is None:
        return None
    files = [item for item in palette["items"] if not item["excluded"]]
    if not files:
        return None
    classes: dict[str, int] = {}
    for item in files:
        kind = " ".join((item["class"] or "other").split())[:24] or "other"
        classes[kind] = classes.get(kind, 0) + 1
    ranked = sorted(classes.items(), key=lambda pair: (-pair[1], pair[0]))
    shown = ", ".join(f"{count} {kind}" for kind, count in ranked[:8]) + (", …" if len(ranked) > 8 else "")
    if palette["reach"] == 0:
        how = ("It's strict: when they ask for sounds, use these (kumi_palette lists and searches them), and look "
               "further only when they ask you to.")
    else:
        how = (f"It's loose: these, and each one's {palette['reach']} nearest on their sound map (kumi_palette "
               "neighbours). When you use one of those, say which, and offer to add it to the palette.")
    excluded = len(palette["items"]) - len(files)
    never = "" if excluded == 0 else (" Never use the sound it excludes." if excluded == 1
                                      else f" Never use the {excluded} sounds it excludes.")
    return palette["name"], f"{counted(len(files))} ({shown}). {how}{never}"
