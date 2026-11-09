"""Sound design for the Desktop and web dashboard (`POST /design`, `GET /designs`): two to sixteen sounds of the map or
earlier results crossed, fused or stacked into a new one by the pinned runtime's one-shot `kumi design`, which takes
files and needs no DAW, session or network. The runtime the chat holds is never started, stopped or asked (C3): the
agent's own designs go through its `design` operation instead.

A request names its sounds by id only; the caller resolves each to an approved file (`snapshots.media`, the one media
guard) and hands it in. A result is `<state>/designs/<artifact_id>.wav`, its id `a-d` and 22 letters or digits, listed
newest first in `<state>/designs.json` (`{"schema": 1, "seq", "designs": [answer + "at"]}`), at most `KEEP` of them: an
older one goes, file and all, unless a palette keeps it (`<state>/palettes.json`). `snapshots.media` serves a listed one,
so `/audio`, the palette and a later design take its id. Neither file holds a path, and nothing here answers one.

Written whole (temp file + `os.replace`) under an OS lock on `<state>/designs.lock`. `dashboard/plugin_api.py` loads
this file by path, so it imports nothing else of the plugin: the media guard comes in as an argument.
"""

from __future__ import annotations

import contextlib
import json
import math
import os
import re
import secrets
import stat
import string
import subprocess
import tempfile
import threading
import time
from pathlib import Path
from typing import Callable, Iterator

SCHEMA = 1
FILE = "designs.json"
LOCK = "designs.lock"
FOLDER = "designs"
# The results kept, and how many `GET /designs` answers.
KEEP = 200
HISTORY = 50
# The longest a render may take: a 30-second smear of sixteen 30-second sounds takes a few seconds.
TIMEOUT_S = 120
# Renders at once: each is a process of its own, and a second click shouldn't queue behind a slow one.
SLOTS = 2
DESIGN_ID = re.compile(r"a-d[A-Za-z0-9]{22}")
OPS = {"cross": ("blend", "attack", "resonate", "modulate"), "fuse": ("texture", "loop", "smear"), "stack": ()}
MIN_SOURCES, MAX_SOURCES, MAX_STACK = 2, 16, 8
MAX_SEED = 2**53 - 1
ID_MAX, NAME_MAX = 200, 120
OPTIONS = {"seconds", "bpm", "bars", "attack_from", "split", "depth", "stretch"}
# What the runtime's process gets of the environment: what a program needs to start and find temporary space.
_LAUNCH = ("PATH", "HOME", "USER", "LANG", "LC_ALL", "TMPDIR", "TEMP", "TMP", "SYSTEMROOT", "WINDIR", "USERPROFILE")
_FILE_MAX_BYTES = 16 * 1024 * 1024
LOCK_WAIT_S = 2.0
_LOCK_POLL_S = 0.02
_slots = threading.BoundedSemaphore(SLOTS)


class Invalid(ValueError):
    """A request no design can take, or one the engine refused: the message says why, for the producer."""


class Busy(Exception):
    """Every render slot, or the list's lock, is taken just now."""


class Unavailable(Exception):
    """The pinned runtime isn't set up in this profile."""


class TimedOut(Exception):
    """A render took longer than `TIMEOUT_S`."""


def _now_ms() -> int:
    return int(time.time() * 1000)


def _whole(value: object) -> bool:
    return isinstance(value, int) and not isinstance(value, bool)


def _number(value: object) -> bool:
    return (_whole(value) or isinstance(value, float)) and math.isfinite(value)


def _text(value: object, most: int) -> bool:
    return isinstance(value, str) and 0 < len(value) <= most and value.isprintable()


def parse(body: object) -> dict:
    """The request a body is (`{op, mode?, sources, seed?, options?}`, sources by id only), its shape checked; the
    engine checks the ranges and says what's wrong in its own words."""
    if not isinstance(body, dict):
        raise Invalid("A design request is a JSON object.")
    unknown = set(body) - {"op", "mode", "sources", "seed", "options"}
    if unknown:
        raise Invalid(f"A design request has no {', '.join(sorted(unknown))}.")
    op, mode = body.get("op"), body.get("mode")
    if op not in OPS:
        raise Invalid("op is cross, fuse or stack.")
    if OPS[op] and mode not in OPS[op]:
        raise Invalid(f"{op} needs a mode: {', '.join(OPS[op])}.")
    if not OPS[op] and mode is not None:
        raise Invalid("stack takes no mode.")
    sources = body.get("sources")
    most = MAX_STACK if op == "stack" else MAX_SOURCES
    if not isinstance(sources, list) or not MIN_SOURCES <= len(sources) <= most:
        raise Invalid(f"{op} takes {MIN_SOURCES} to {most} sounds.")
    checked = []
    for source in sources:
        if not isinstance(source, dict) or set(source) - {"sound_id", "map_revision", "artifact_id", "name"}:
            raise Invalid("Each sound is {\"sound_id\"} (with its map_revision) or {\"artifact_id\"}, and maybe a name.")
        if ("sound_id" in source) == ("artifact_id" in source) or ("map_revision" in source and "sound_id" not in source):
            raise Invalid("Each sound is one of a sound_id or an artifact_id.")
        if any(not _text(value, NAME_MAX if key == "name" else ID_MAX) for key, value in source.items()):
            raise Invalid("A sound's ids and name are short lines of text.")
        checked.append(dict(source))
    request: dict = {"op": op, "sources": checked}
    if mode is not None:
        request["mode"] = mode
    if "seed" in body:
        if not _whole(body["seed"]) or not 0 <= body["seed"] <= MAX_SEED:
            raise Invalid(f"seed is a whole number from 0 to {MAX_SEED}.")
        request["seed"] = body["seed"]
    options = body.get("options", {})
    if not isinstance(options, dict) or set(options) - OPTIONS:
        raise Invalid(f"options are {', '.join(sorted(OPTIONS))}.")
    for key, value in options.items():
        ok = all(_whole(i) for i in value) and len(value) <= MAX_SOURCES if key == "split" and isinstance(value, list) else _number(value)
        if not ok:
            raise Invalid(f"{key} is a number{' list' if key == 'split' else ''}.")
    request["options"] = dict(options)
    return request


def runtime(data_dir: Path, pinned: str | None, target: str | None) -> tuple[str, str | None]:
    """The pinned runtime's `kumi` set up in this profile for this computer (`target`), and the ffmpeg the producer
    allowed at setup while it's unchanged (as `runtime.installed` and `runtime.allowed_tool` read them; that module
    can't be loaded by path)."""
    try:
        receipt = json.loads((data_dir / "runtime" / "current.json").read_text(encoding="utf-8"))
    except (OSError, ValueError):
        raise Unavailable("Kumi isn't set up in this Hermes profile yet.") from None
    if (not isinstance(receipt, dict) or receipt.get("schema") != 1 or not pinned or receipt.get("version") != pinned
            or not target or receipt.get("target") != target):
        raise Unavailable("Kumi's runtime isn't the one this plugin pins: set Kumi up again.")
    root, kumi = data_dir.resolve(), receipt.get("kumi")
    program = (data_dir / kumi).resolve() if isinstance(kumi, str) and kumi else None
    if program is None or root not in program.parents or not program.is_file() or not os.access(program, os.X_OK):
        raise Unavailable("Kumi's runtime is missing or damaged: set Kumi up again.")
    tools = receipt.get("tools") if isinstance(receipt.get("tools"), dict) else {}
    record = tools.get("ffmpeg") if isinstance(tools.get("ffmpeg"), dict) else None
    return str(program), _allowed(record)


def _allowed(record: dict | None) -> str | None:
    path = record.get("path") if isinstance(record, dict) else None
    if not isinstance(path, str) or not os.path.isabs(path) or os.path.realpath(path) != path:
        return None
    try:
        found = os.stat(path, follow_symlinks=False)
    except OSError:
        return None
    same = all(_whole(record.get(key)) and record[key] == getattr(found, f"st_{key}") for key in ("size", "mtime_ns", "ino"))
    return path if same and stat.S_ISREG(found.st_mode) and os.access(path, os.X_OK) else None


def new_id() -> str:
    return "a-d" + "".join(secrets.choice(string.ascii_letters + string.digits) for _ in range(22))


def render(state: Path, request: dict, files: list[tuple[str, str]], kumi: str, ffmpeg: str | None) -> dict:
    """The design `request` asks for, from `files` (each sound's approved path and name, in order): rendered by `kumi
    design` into `<state>/designs/<id>.wav`, listed, and answered as the engine answered it, with its id and the
    request's own sources (ids, never paths)."""
    if not _slots.acquire(blocking=False):
        raise Busy("Kumi is already making two sounds; try again in a moment.")
    try:
        folder = state / FOLDER
        folder.mkdir(parents=True, exist_ok=True)
        artifact_id = new_id()
        out = folder / f"{artifact_id}.wav"
        engine = {key: value for key, value in request.items() if key != "sources"}
        engine["sources"] = [{"file": path, "name": name} for path, name in files]
        argv = [kumi, "design", "--json", "-", "--out", str(out)] + (["--ffmpeg", ffmpeg] if ffmpeg else [])
        environment = {key: os.environ[key] for key in _LAUNCH if key in os.environ}
        environment["KUMI_NO_UPDATE_CHECK"] = "1"
        try:
            done = subprocess.run(argv, input=json.dumps(engine), capture_output=True, text=True, timeout=TIMEOUT_S,
                                  env=environment)
        except subprocess.TimeoutExpired:
            out.unlink(missing_ok=True)
            raise TimedOut(f"Making that sound took over {TIMEOUT_S} seconds.") from None
        lines = done.stdout.strip().splitlines()
        try:
            answer = json.loads(lines[-1]) if lines else None
        except ValueError:
            answer = None
        if not isinstance(answer, dict) or done.returncode != 0 or "error" in answer:
            out.unlink(missing_ok=True)
            said = answer.get("error") if isinstance(answer, dict) else None
            raise Invalid(_unpathed(said, files) if isinstance(said, str) else "Kumi couldn't make that sound.")
        if not out.is_file():
            raise Invalid("Kumi couldn't make that sound.")
    finally:
        _slots.release()
    answer = {**answer, "artifact_id": artifact_id, "sources": request["sources"]}
    _record(state, answer)
    return answer


def _unpathed(said: str, files: list[tuple[str, str]]) -> str:
    """The engine's words with any path of the request's said as its sound's name: nothing answers a path (CC-4)."""
    for path, name in files:
        said = said.replace(path, name)
    return said


def history(state: Path, limit: int = HISTORY) -> list[dict]:
    """The designs made here, newest first."""
    return [{key: value for key, value in d.items() if key != "at"} for d in _read(state)["designs"][:limit]]


def _read(state: Path) -> dict:
    try:
        with open(state / FILE, "rb") as file:
            data = json.loads(file.read(_FILE_MAX_BYTES + 1) or b"null")
    except (OSError, ValueError):
        data = None
    if not isinstance(data, dict) or data.get("schema") != SCHEMA or not isinstance(data.get("designs"), list):
        return {"schema": SCHEMA, "seq": 0, "designs": []}
    designs = [d for d in data["designs"] if isinstance(d, dict) and DESIGN_ID.fullmatch(str(d.get("artifact_id")))]
    return {"schema": SCHEMA, "seq": data.get("seq") if _whole(data.get("seq")) else 0, "designs": designs}


def _kept_by_palettes(state: Path) -> set[str]:
    """The artifact ids a palette keeps (`palettes.py`'s `<state>/palettes.json`, its `{"source": "artifact"}` items):
    results that stay however old they get."""
    try:
        data = json.loads((state / "palettes.json").read_text(encoding="utf-8"))
    except (OSError, ValueError):
        return set()
    palettes = data.get("palettes") if isinstance(data, dict) else None
    return {item["artifact_id"] for palette in palettes or [] if isinstance(palette, dict)
            for item in palette.get("items") or []
            if isinstance(item, dict) and item.get("source") == "artifact" and isinstance(item.get("artifact_id"), str)}


def _record(state: Path, answer: dict) -> None:
    with _locked(state):
        data = _read(state)
        designs = [{**answer, "at": _now_ms()}] + data["designs"]
        kept = _kept_by_palettes(state) if len(designs) > KEEP else set()
        stay = designs[:KEEP] + [d for d in designs[KEEP:] if d["artifact_id"] in kept]
        for gone in designs[KEEP:]:
            if gone["artifact_id"] not in kept:
                (state / FOLDER / f"{gone['artifact_id']}.wav").unlink(missing_ok=True)
        fd, temporary = tempfile.mkstemp(prefix=".designs-", dir=state)
        try:
            with os.fdopen(fd, "w", encoding="utf-8") as file:
                json.dump({"schema": SCHEMA, "seq": data["seq"] + 1, "designs": stay}, file, ensure_ascii=False,
                          separators=(",", ":"))
            os.replace(temporary, state / FILE)
        except BaseException:
            Path(temporary).unlink(missing_ok=True)
            raise


@contextlib.contextmanager
def _locked(state: Path) -> Iterator[None]:
    """An exclusive OS lock on `<state>/designs.lock` (flock, or msvcrt on Windows), released by the OS if the process
    dies. Waits up to `LOCK_WAIT_S`: writes are tiny."""
    state.mkdir(parents=True, exist_ok=True)
    with open(state / LOCK, "a+b") as handle:
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
                    raise Busy("Kumi is saving another sound it made; try again.") from None
                time.sleep(_LOCK_POLL_S)
        try:
            yield
        finally:
            if os.name == "nt":
                handle.seek(0)
                msvcrt.locking(handle.fileno(), msvcrt.LK_UNLCK, 1)
            else:
                fcntl.flock(handle.fileno(), fcntl.LOCK_UN)


def resolve(request: dict, revision: str | None, sound: Callable[[str], tuple[dict | None, str | None] | None],
            artifact: Callable[[str], dict | None]) -> list[tuple[str, str]]:
    """Each source's approved file and name. `revision` is the current map revision; `sound(id)` is None for an id
    that isn't one of its sounds, else the media guard's answer for it and its name; `artifact(id)` the guard's answer
    for a runtime artifact or a listed design. A source picked on an older map, or one the guard won't serve, is
    refused by its place."""
    files = []
    for place, source in enumerate(request["sources"]):
        if "sound_id" in source:
            if "map_revision" in source and source["map_revision"] != revision:
                raise Invalid(f"Sound {place + 1} was picked on an older map: pick the sounds again.")
            found, name = sound(source["sound_id"]) or (None, None)
        else:
            found, name = artifact(source["artifact_id"]), None
        if not isinstance(found, dict) or not isinstance(found.get("path"), str):
            raise Invalid(f"Sound {place + 1} isn't one Kumi can read here: it's not on the current map, or its file "
                          "has gone.")
        files.append((found["path"], source.get("name") or name or f"Sound {place + 1}"))
    return files
