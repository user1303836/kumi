"""Read the state Kumi's runtime publishes for the plugin's other halves (contract C3).

`kumi serve` writes these files under the profile's plugin data, each by atomic replace, with
`{"schema": 1, "seq", "at", "owner"}`. The UI backend (`dashboard/plugin_api.py`, which loads this file by path) and
the FOCUS hook (`context.py`) only read them: nothing here writes, spawns or reaches the runtime, and nothing raises on
bad data. A missing, unreadable or foreign file reads as absent.

Module-level imports stay in the standard library: `register()` runs with only Hermes on the path, so Hermes's own
helpers are imported where they're used.
"""

from __future__ import annotations

import hashlib
import json
import logging
import math
import os
import posixpath
import re
import stat
import sys
import time
from pathlib import Path
from typing import Any, Callable

log = logging.getLogger(__name__)

SCHEMA = 1
# A heartbeat older than this makes the state stale: shown as last known, never acted on (C3).
STALE_AFTER_S = 10
HISTORY_MAX = 200
# The largest file a media route hands over (C4).
MEDIA_MAX_BYTES = 50 * 1024 * 1024
# CC-19 r4/r8: one cap for a script everywhere (the plugin's schema, serve's preview, the saved file, this read).
SCRIPT_MAX_BYTES = 32 * 1024
SCRIPT_MAX_LINES = 400
# A script's file extension, fixed by its language (CC-19): nothing else is ever read from state/scripts. A device's
# file (CC-22) is its whole spec as canonical JSON, named by that JSON's hash: what the approval keyed on.
SCRIPT_EXTENSIONS = {"python": ".py", "lua": ".lua", "device": ".device.json"}
# A device's spec holds its code (CC-19's cap) and up to 16 controls; JSON escaping can grow the code a little.
DEVICE_MAX_BYTES = 64 * 1024
_SHA256 = re.compile(r"[0-9a-f]{64}")
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


def file_id(value: object) -> str | None:
    """`value` when it's an id the runtime names a snapshot file by (a job, a map revision), else None."""
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
    """How fresh the runtime is, from owner.json: stale or not, its heartbeat's age, and `running: false` once its last
    word was that it stopped (a stop, not a silence); its `owner` too when it names one (a snapshot keeps its own
    else)."""
    own = owner(state, now_ms)
    live = {"stale": own["stale"], "age_ms": own["age_ms"], "running": own.get("running") if isinstance(own.get("running"), bool) else None}
    if isinstance(own.get("owner"), dict):
        live["owner"] = own["owner"]
    return live


def map_current(state: Path) -> str | None:
    """The current map revision's id (`map/current` holds it)."""
    try:
        with (state / "map" / "current").open("r", encoding="utf-8") as file:
            return file_id(file.read(256).strip())
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
        # The plugin's own palettes file (palettes.py), which both UIs and the agent edit: 0 before the first write.
        "palette": _seq(read(state, "palettes")) or 0,
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
    name = file_id(revision) if revision is not None else map_current(state)
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
    return None if data is None else public_of_map(data)


def public_of_map(data: dict) -> dict:
    """`public_map` of a revision already read (`map_revision`), for a caller that also needs its private paths."""
    sounds = data.get("sounds")
    public = {key: value for key, value in data.items() if key not in ("schema", "sounds")}
    public["sounds"] = (
        {sound_id: {k: v for k, v in entry.items() if k != "path"} for sound_id, entry in sounds.items() if isinstance(entry, dict)}
        if isinstance(sounds, dict)
        else {}
    )
    return public


def job(state: Path, job_id: str) -> dict | None:
    name = file_id(job_id)
    return read(state, f"jobs/{name}") if name else None


def _inside(path: str, roots: list[Path]) -> bool:
    for root in roots:
        try:
            Path(path).relative_to(root)
            return True
        except ValueError:
            continue
    return False


# Windows path forms (W5 review L-S6). Kumi's native snapshots name files in the verbatim drive form (`\\?\C:\…`), and
# a sample folder the producer approved is usually a plain drive path (`C:\…`): both are compared in the plain form.
# A network or device path (`\\server\share`, `\\?\UNC\…`, `\\.\…`, `\\?\GLOBALROOT\…`) is refused before anything
# touches the filesystem, since even resolving one can reach another machine. So is a path that isn't drive-absolute
# (`C:x`, `\x`), which depends on the process's own drive and folder, and one Windows would read as something else: a
# reserved device name (`NUL`, `COM1.wav`), a name ending in a dot or space (Windows drops them, so `a.wav.` is
# `a.wav`), or an alternate data stream (`a.wav:hidden`). Network folders therefore can't be auditioned on Windows.
# Elsewhere, the automounter's network roots (`/net`, macOS's `/Network`, and both again under macOS's firmlinked
# `/System/Volumes/Data`) are refused the same way: looking one up mounts another machine's share. So is macOS's `/home`,
# a link to `/System/Volumes/Data/home`, which its automounter serves (auto_home); Linux's `/home` is a plain folder.
_VERBATIM_DRIVE = re.compile(r"^[\\/]{2}\?[\\/](?=[A-Za-z]:[\\/])")
_DRIVE_ABSOLUTE = re.compile(r"^[A-Za-z]:[\\/]")
_RESERVED = re.compile(r"^(?:CON|PRN|AUX|NUL|COM[0-9¹²³]|LPT[0-9¹²³]|CONIN\$|CONOUT\$) *(?:\..*)?$", re.IGNORECASE)
_NETWORK_ROOT = re.compile(r"^/(?:System/Volumes/Data/)?(?:net|Network)(?:/|$)", re.IGNORECASE)
_AUTO_HOME = re.compile(r"^/(?:System/Volumes/Data/)?home(?:/|$)", re.IGNORECASE)
# How many links one path may pass through before it's taken as a loop.
_HOPS = 32


def local(path: str, windows: bool = os.name == "nt", darwin: bool = sys.platform == "darwin") -> str | None:
    """`path` as a file path on this machine, in the form the containment check compares; None for one never served
    or read, decided without touching the filesystem. Elsewhere than Windows it is returned as it is."""
    if not windows:
        plain = posixpath.normpath(path).replace("//", "/", 1)
        return None if _NETWORK_ROOT.match(plain) or (darwin and _AUTO_HOME.match(plain)) else path
    plain = _VERBATIM_DRIVE.sub("", path, count=1)
    if not _DRIVE_ABSOLUTE.match(plain) or ":" in plain[2:]:
        return None
    for part in re.split(r"[\\/]+", plain[3:]):
        if part in ("", ".", ".."):
            continue
        if part[-1] in ". " or _RESERVED.match(part.rstrip(". ")):
            return None
    return plain


def resolved(path: str, windows: bool = os.name == "nt") -> str | None:
    """`path` (already through `local`) with its links followed (`follow`), each link's target checked by `local`
    before anything under it is touched: a link inside an approved folder, or a folder that is one, can lead to a
    network share, which a lookup alone would reach (rgzm). A long path can come back in the verbatim form."""
    return follow(path, lambda each: local(each, windows))


def follow(path: str, allowed: Callable[[str], str | None]) -> str | None:
    """`path` with every link in it followed from the root down, as `realpath` would, except that nothing is looked up
    before it passes `allowed`: each step on the way (a relative link can climb into `/net`), and each link's target,
    read without following it (`readlink`). `allowed` answers the path to go on with (its plain form), or None to
    refuse it. None too if the links loop."""
    start = allowed(path)
    if start is None:
        return None
    anchor, *names = Path(start).parts
    current, hops = anchor, 0
    while names:
        name = names.pop(0)
        if name in ("", "."):
            continue
        if name == "..":
            current = os.path.dirname(current)
            continue
        step = os.path.join(current, name)
        if allowed(step) is None:  # before its lstat: looking up a network root mounts it
            return None
        try:  # lstat and readlink: neither follows `step`, and everything above it is already followed
            target = os.readlink(step) if _linked(step) else None
        except (OSError, ValueError):
            return None
        if target is None:
            current = step
            continue
        hops += 1
        if hops > _HOPS or (Path(target).anchor and not Path(target).is_absolute()):  # a loop, or `\x`/`C:x`
            return None
        if Path(target).is_absolute():
            plain = allowed(target)
            if plain is None:
                return None
            current, names = Path(plain).anchor, [*Path(plain).parts[1:], *names]
        else:
            names = [*Path(target).parts, *names]
    if allowed(current) is None:
        return None
    return allowed(os.path.realpath(current))  # no links are left: realpath only spells it canonically


def _linked(path: str) -> bool:
    """Whether `path` is a link, or on Windows a junction (a mount-point reparse point, which `os.path.isjunction` sees
    only from Python 3.12), from one `lstat` that follows neither. A path that isn't there is no link."""
    try:
        info = os.lstat(path)
    except (OSError, ValueError):
        return False
    return stat.S_ISLNK(info.st_mode) or (getattr(info, "st_reparse_tag", 0) != 0
                                          and info.st_reparse_tag == getattr(stat, "IO_REPARSE_TAG_MOUNT_POINT", -1))


def hermes_home() -> str | None:
    """This profile's Hermes home (`hermes_constants.get_hermes_home`), or None where Hermes isn't importable."""
    try:
        from hermes_constants import get_hermes_home  # Hermes's

        return str(get_hermes_home())
    except Exception:
        return None


def library_folder(folder: Any, *, hermes_home: str | None = None) -> tuple[str | None, str | None]:
    """A sample folder the producer may approve, as Kumi will keep and read it: an absolute path (`~` expanded; on
    Windows with its drive), normalized. A network or device path is refused before anything touches the filesystem
    (`local`: even a lookup can reach another machine). Its real place, links followed and matched by identity, is
    never a disk's or volume's root, the home folder or one holding it, nor Hermes's own folder, inside or holding it
    (`hermes_home`). Returns (folder, None), or (None, why not)."""
    if not isinstance(folder, str) or not folder.strip():
        return None, "name the folder, as a full path."
    if any(ord(char) < 32 or ord(char) == 127 for char in folder):
        return None, "the folder's name has control characters."
    expanded = os.path.expanduser(folder)
    if not Path(expanded).is_absolute():
        return None, f"{folder!r} isn't a full path."
    normal = os.path.normpath(expanded)
    if local(normal) is None:
        return None, f"{normal} is a network or device path; name a folder on this computer."
    real = resolved(normal)
    if real is None:
        return None, f"{normal} leads to a network or device path; name a folder on this computer."
    if os.path.dirname(real) == real or os.path.ismount(real):
        return None, f"{normal} is a whole drive or volume; name a sample folder."
    if _holds(real, os.path.expanduser("~")):
        return None, "that's the home folder, or a folder holding it; name a sample folder."
    if hermes_home and (_holds(real, hermes_home) or _within(real, hermes_home)):
        return None, "that's Hermes's own folder; name a sample folder."
    return normal, None


def typed_library_folder(folder: Any, *, hermes_home: str | None = None) -> tuple[str | None, str | None]:
    """A folder the producer named themself (the terminal, `/kumi samples add`, the map's Sample folders control):
    `library_folder`'s answer for it, its rules unchanged, after taking off what a shell or a paste leaves around a
    path (a pair of quotes; the closing quote cmd leaves from `"D:\\Samples\\"`). The answer is the folder's real
    place, links followed: the producer sees where it leads before saying yes, and it's the one path `kumi serve`
    takes for a folder (it refuses a link on the way). A path with no folder at it is said here, not found out by an
    empty scan. Returns (folder, None), or (None, why not)."""
    if isinstance(folder, str):
        folder = folder.strip()
        if len(folder) >= 2 and folder[0] == folder[-1] and folder[0] in "\"'":
            folder = folder[1:-1].strip()
        elif folder.endswith('"') and '"' not in folder[:-1]:
            folder = folder[:-1]
    normal, refusal = library_folder(folder, hermes_home=hermes_home)
    if normal is None:
        return None, refusal
    real = real_folder(normal)  # the guard passed, so every step of it is on this computer
    if real is None:
        return None, f"there's no folder at {normal}."
    return library_folder(real, hermes_home=hermes_home)


def real_folder(folder: str) -> str | None:
    """A folder's real place, links followed, when a folder is there now; None otherwise (an unplugged drive, a
    renamed folder, a network path, which is never looked up)."""
    plain = local(folder)
    real = resolved(plain) if plain else None
    return real if real and os.path.isdir(real) else None


def same_place(folder: str) -> str:
    """A folder's real place, in a form every spelling of it shares: links followed, Windows's verbatim prefix and
    letter case aside. `kumi serve` keeps its folders as the disk spells them (`/private/tmp/…` for `/tmp/…`,
    `\\\\?\\C:\\…` on Windows), so this is how one of the settings' folders is told from one the runtime already has.
    A path that can't be resolved here stands for itself."""
    plain = local(folder)
    real = resolved(plain) if plain else None
    return os.path.normcase(real or folder)


def has_folder(folders: list[str], folder: str) -> bool:
    """Whether `folder` is one of `folders`, under any spelling (`same_place`)."""
    place = same_place(folder)
    return any(same_place(each) == place for each in folders)


def library_folders(folders: Any, *, hermes_home: str | None = None) -> list[str]:
    """The `library_folders` setting as the guard keeps it: each folder normalized, once. One the guard refuses
    (relative, a disk, the home folder, Hermes's own, a network path), however it got into the setting, is left out
    with a warning: `kumi serve` would refuse to start with it, and no media route reads it."""
    kept: list[str] = []
    for folder in folders if isinstance(folders, list) else []:
        normal, refusal = library_folder(folder, hermes_home=hermes_home)
        if normal is None:
            log.warning("Kumi left a sample folder setting out: %s", refusal)
        elif normal not in kept:
            kept.append(normal)
    return kept


def _holds(folder: str, target: str) -> bool:
    """Whether `folder` is `target` or holds it, under any name: the path that reaches `target` from one of its own
    folders (itself included) reaches it from `folder` too. So a link to home or above counts, and so does macOS's
    /System/Volumes/Data, which isn't / (nor a mount point) but holds home as Users/<name> (rgzm)."""
    place = Path(os.path.realpath(target))
    for parent in (place, *place.parents):
        try:
            if os.path.samefile(os.path.join(folder, os.path.relpath(place, parent)), place):
                return True
        except (OSError, ValueError):
            continue
    return False


def _within(folder: str, root: str) -> bool:
    """Whether `folder` is inside `root`, by identity: one of its folders is `root` under any spelling, so a
    case-insensitive disk's other spelling of it counts (rgzm)."""
    for parent in Path(folder).parents:
        try:
            if os.path.samefile(parent, root):
                return True
        except OSError:
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


# A sound the dashboard made (`designs.py`, `POST /design`): `a-d` and 22 letters or digits.
DESIGN_ID = re.compile(r"a-d[A-Za-z0-9]{22}")


def _designed(state: Path, media_id: str) -> dict | None:
    """The answer `designs.py` listed for a sound it made (`<state>/designs.json`), or None when it lists none."""
    if not DESIGN_ID.fullmatch(media_id):
        return None
    data = _load(state / "designs.json")
    listed = data.get("designs") if isinstance(data, dict) and data.get("schema") == SCHEMA else None
    return next((d for d in listed or [] if isinstance(d, dict) and d.get("artifact_id") == media_id), None)


def _design_name(answer: dict) -> str:
    """What a palette calls a sound the dashboard made: its recipe's lead ("Blend of 3", "Modulate Vox") and its seed,
    so two variations of one recipe tell apart."""
    recipe = answer.get("recipe") if isinstance(answer.get("recipe"), str) else ""
    lead = " ".join(re.split(r"[:;]", recipe, maxsplit=1)[0].split())[:60]
    if not lead:
        lead = str(answer.get("mode") or answer.get("op") or "Design").capitalize()
    seed = answer.get("seed")
    return f"{lead}, seed {seed}" if isinstance(seed, int) and not isinstance(seed, bool) else lead


def media(state: Path, media_id: str, library_folders: list[str] | None = None, *, current: dict | None = None) -> dict | None:
    """The file behind a media id, `{id, path, kind, mime, bytes}`, or None. Only three kinds of id have one (CC-4):
    an artifact (artifacts.json) whose real path is inside `<state>/artifacts` or an approved sample folder, a sound
    the dashboard made (`designs.json`), only as `<state>/designs/<id>.wav`, and a sound of the current map revision
    whose real path is inside an approved sample folder. Nothing else in the plugin
    data (Kumi's home, the runtime, setup) is ever served. The file must be a regular one, at most 50 MiB. `current`:
    the current map revision, already read (`map_revision`), for a caller asking about many of its sounds at once."""
    approved = _approved(library_folders)
    entry: dict | None = _artifacts(state).get(media_id)
    if entry is not None:
        roots = _artifact_roots(state) + approved
    elif _designed(state, media_id) is not None:
        entry = {"path": str(state / "designs" / f"{media_id}.wav"), "kind": "audio", "mime": "audio/wav"}
        roots = [Path(real) for real in [resolved(str(state / "designs"))] if real]
    else:
        if current is not None:
            sounds = current.get("sounds")
            found = sounds.get(media_id) if isinstance(sounds, dict) else None
        else:
            found = sound(state, media_id)
        if not isinstance(found, dict):
            return None
        entry = {"path": found.get("path"), "kind": "audio", "mime": found.get("mime")}
        roots = approved
    real = _real_file(entry.get("path"), roots)
    if real is None:
        return None
    size = os.path.getsize(real)
    if size > MEDIA_MAX_BYTES:
        return None
    mime = entry.get("mime") if isinstance(entry.get("mime"), str) else _mime_of(real)
    # Only what the players and the listen card use: never a type a browser would render as a page.
    if not isinstance(mime, str) or not _MEDIA_TYPE.fullmatch(mime):
        return None
    return {"id": media_id, "path": real, "kind": entry.get("kind") or "audio", "mime": mime, "bytes": size}


def artifact_file(state: Path, artifact_id: str) -> str | None:
    """The real path of a file the runtime kept as an artifact (artifacts.json) in `<state>/artifacts`, a regular
    file, or None. A listen's `MEDIA:` line may name only this: no size or type limit, as it names the file and
    doesn't serve it."""
    entry = _artifacts(state).get(artifact_id)
    return _real_file(entry.get("path"), _artifact_roots(state)) if entry is not None else None


def palette_artifact(state: Path, artifact_id: Any, library_folders: list[str] | None = None) -> str | None:
    """The name a palette keeps for an artifact it gathers (its own `name`, else its file's stem; a sound the dashboard
    made, by its recipe and seed), when it's audio the media guard serves (`media`, so `/audio?id=` plays it); None for
    anything else, a map sound's id included."""
    if not isinstance(artifact_id, str) or len(artifact_id) > 128:
        return None
    entry = _artifacts(state).get(artifact_id)
    designed = _designed(state, artifact_id) if entry is None else None
    found = media(state, artifact_id, library_folders) if entry is not None or designed is not None else None
    if found is None or not found["mime"].startswith("audio/"):
        return None
    if designed is not None:
        return _design_name(designed)
    name = entry.get("name")
    if isinstance(name, str) and name.strip():
        return " ".join(name.split())[:200]
    return os.path.splitext(os.path.basename(found["path"]))[0]


def _artifact_roots(state: Path) -> list[Path]:
    return [Path(real) for real in [resolved(str(state / "artifacts"))] if real]


def _approved(library_folders: list[str] | None) -> list[Path]:
    found = (local(os.path.expanduser(f)) for f in library_folders or [] if isinstance(f, str) and f)
    return [Path(real) for real in (resolved(path) for path in found if path) if real]


def _real_file(path: Any, roots: list[Path]) -> str | None:
    """`path`'s real place when it's a regular file inside one of `roots`, checked as `media()` checks a file."""
    found = local(path) if isinstance(path, str) and path else None
    real = resolved(found) if found is not None else None
    return real if real is not None and _inside(real, roots) and os.path.isfile(real) else None


def read_media(state: Path, media_id: str, library_folders: list[str] | None = None) -> tuple[dict, bytes] | None:
    """`media()`'s file and its bytes. The resolved path must still be a regular file, not a link, and the file opened
    must be that same file (device and inode, which Windows reports too): a file swapped for a link or for another
    file after the check is refused, on systems without `O_NOFOLLOW` as well (#1000). Within the size cap."""
    found = media(state, media_id, library_folders)
    if found is None:
        return None
    try:
        checked = os.lstat(found["path"])
        handle = os.open(found["path"], os.O_RDONLY | getattr(os, "O_NOFOLLOW", 0) | getattr(os, "O_BINARY", 0))
    except OSError:
        return None
    with os.fdopen(handle, "rb") as file:
        info = os.fstat(file.fileno())
        same = (info.st_dev, info.st_ino) == (checked.st_dev, checked.st_ino)
        if not stat.S_ISREG(checked.st_mode) or not stat.S_ISREG(info.st_mode) or not same or info.st_size > MEDIA_MAX_BYTES:
            return None
        data = file.read(MEDIA_MAX_BYTES + 1)
    if len(data) > MEDIA_MAX_BYTES:
        return None
    return {**found, "bytes": len(data)}, data


def script(state: Path, sha256: str, language: str) -> dict | None:
    """The code of a script the plugin saved when its approval asked (CC-19), for the Desktop pane to show in full:
    `state/scripts/<sha256>.<ext>`, the extension fixed by `language`. Read only when the folder isn't a link and the
    file is a regular file directly in it (the same file `lstat` saw, opened without following links), within CC-19's
    cap, valid UTF-8, and only if its bytes still hash to its name, so the pane never shows code other than what the
    approval named. None otherwise. Read-only: nothing here runs anything."""
    extension = SCRIPT_EXTENSIONS.get(language)
    most = DEVICE_MAX_BYTES if language == "device" else SCRIPT_MAX_BYTES
    if extension is None or not isinstance(sha256, str) or not _SHA256.fullmatch(sha256):
        return None
    folder = state / "scripts"
    path = folder / f"{sha256}{extension}"
    try:
        if not stat.S_ISDIR(os.lstat(folder).st_mode):
            return None
        checked = os.lstat(path)
        handle = os.open(path, os.O_RDONLY | getattr(os, "O_NOFOLLOW", 0) | getattr(os, "O_BINARY", 0))
    except OSError:
        return None
    with os.fdopen(handle, "rb") as file:
        info = os.fstat(file.fileno())
        same = (info.st_dev, info.st_ino) == (checked.st_dev, checked.st_ino)
        if not stat.S_ISREG(checked.st_mode) or not stat.S_ISREG(info.st_mode) or not same or info.st_size > most:
            return None
        data = file.read(most + 1)
    if len(data) > most or hashlib.sha256(data).hexdigest() != sha256:
        return None
    try:
        code = data.decode("utf-8")
    except UnicodeDecodeError:
        return None
    if language == "device":
        # The canonical spec: an object whose code is a string within CC-19's line cap. Its lines are the code's.
        try:
            spec = json.loads(code)
        except ValueError:
            return None
        if not isinstance(spec, dict) or not isinstance(spec.get("code"), str) or not isinstance(spec.get("controls", []), list):
            return None
        lines = len(spec["code"].splitlines())
    else:
        lines = len(code.splitlines())
    if lines > SCRIPT_MAX_LINES:
        return None
    return {"sha256": sha256, "language": language, "code": code, "lines": lines}


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
