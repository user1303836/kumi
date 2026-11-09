"""Kumi's backend for its Desktop and web-dashboard halves (contract C4): `/api/plugins/kumi/…`.

GET routes read the snapshots `kumi serve` publishes in this profile's plugin data (contract C3), re-resolved on every
request. The few POST routes the contract names (`/map/hidden`, `/palette`, `/design`) keep the UI's own state: they
take small, bounded JSON bodies, never a file path, answer without one, change no project and write only under
`<state>`. Nothing here starts, stops or talks to the runtime the chat holds, the DAW or the network: `/design` runs the
pinned runtime's one-shot `kumi design` on files the media guard approved, which needs no DAW or session. A change in
the DAW only ever comes from a tool call the user approves, which the UI asks for by putting a request into the chat.
"""

from __future__ import annotations

import base64
import importlib.util
import json
import logging
import sys
from pathlib import Path
from typing import Any

from fastapi import APIRouter, HTTPException, Query, Request
from starlette.concurrency import run_in_threadpool

_log = logging.getLogger(__name__)

_PLUGIN_DIR = Path(__file__).resolve().parent.parent


def _load(name: str, file: str) -> Any:
    """A module of this plugin's own, loaded by path (this file is imported on its own, outside the plugin package)."""
    module_name = f"hermes_dashboard_plugin_kumi_{name}"
    loaded = sys.modules.get(module_name)
    if loaded is not None:
        return loaded
    spec = importlib.util.spec_from_file_location(module_name, _PLUGIN_DIR / file)
    if spec is None or spec.loader is None:
        raise ImportError(f"Kumi's {file} is missing from {_PLUGIN_DIR}")
    module = importlib.util.module_from_spec(spec)
    sys.modules[module_name] = module
    try:
        spec.loader.exec_module(module)
    except Exception:
        sys.modules.pop(module_name, None)
        raise
    return module


snapshots = _load("snapshots", "snapshots.py")
schemas = _load("schemas", "schemas.py")
hidden = _load("hidden", "hidden.py")
aiff = _load("aiff", "aiff.py")
palettes = _load("palettes", "palettes.py")
designs = _load("designs", "designs.py")

# The largest body `POST /map/hidden` and `POST /design` read: 500 ids of 128 characters with their reasons fit well
# inside it, and a design's sixteen sounds with their names much more so.
_BODY_MAX = 256 * 1024
# The largest body `POST /palette` reads: a gather of 10,000 current-revision ids (about 45 bytes each) fits in it.
_PALETTE_BODY_MAX = 1024 * 1024

router = APIRouter()


def _setting(key: str) -> object:
    """One of the plugin's settings (`plugins.entries.kumi.settings`, else the legacy `config` subtree), or None."""
    try:
        from hermes_cli.config import load_config_readonly
    except ImportError:
        return None
    entry = (((load_config_readonly() or {}).get("plugins") or {}).get("entries") or {}).get("kumi") or {}
    for subtree in ("settings", "config"):
        value = (entry.get(subtree) or {}).get(key)
        if value is not None:
            return value
    return None


def _library_folders() -> list[str]:
    """The sample folders the user approved (the plugin's `library_folders` setting), as the guard keeps them
    (`snapshots.library_folders`): the only places besides the plugin's own data that a media route may read."""
    return snapshots.library_folders(_setting("library_folders"), hermes_home=snapshots.hermes_home())


def _chosen_daw() -> str | None:
    """The DAW chosen in Kumi's settings, or None while the setting is unset or names a DAW Kumi doesn't know: until the
    producer picks one, nothing names a DAW for them, and setup is offered for each (setup makes the first one set up
    Kumi's DAW)."""
    daw = _setting("daw")
    return daw if isinstance(daw, str) and daw in schemas.DAWS else None


def _liveness(state: Path) -> dict:
    own = snapshots.owner(state)
    return {"connected": not own["stale"], "stale": own["stale"]}


@router.get("/status")
def status() -> dict:
    """Setup and connection, the project, the owner's session, and each snapshot's revision. Until a runtime has said
    which DAW it works in, the DAW is the one chosen in Kumi's settings, so "not connected" names it."""
    state = snapshots.state_dir()
    out = snapshots.status(state)
    out["revisions"]["hidden"] = hidden.seq(state)
    if not out.get("daw") and (chosen := _chosen_daw()):
        out["daw"] = chosen
    return out


@router.get("/focus")
def focus() -> dict:
    """What the producer is looking at: track, device, clip, view, the transport and the catch-up."""
    return snapshots.focus(snapshots.state_dir())


@router.get("/history")
def history(limit: int = Query(20, ge=1, le=snapshots.HISTORY_MAX)) -> dict:
    """HISTORY, newest first."""
    return snapshots.history(snapshots.state_dir(), limit)


@router.get("/map")
def sound_map(revision: str | None = Query(None, max_length=128)) -> dict:
    """A sound-map revision (the current one by default): points, groups and sounds, never a file's path (CC-4).
    Sounds the UI found it can't play here are left out (`hidden.visible`), and `hidden` says how many."""
    state = snapshots.state_dir()
    current = snapshots.map_current(state)
    private = snapshots.map_revision(state, revision or current)
    public = None
    if private is not None:
        public = hidden.visible(snapshots.public_of_map(private), private.get("sounds"), state, snapshots.local)
    out: dict = {"revision": None, "points": [], "groups": {}, "sounds": {}, "hidden": 0, **(public or {}), **_liveness(state)}
    out["current"] = current
    return out


def _map_hidden(body: object) -> dict:
    """`POST /map/hidden`'s work, off the event loop: hide the sounds of the current revision a client reports
    (`{"revision", "ids": [{"id", "why"}]}`), or show them all again (`{"action": "clear"}`)."""
    try:
        action, revision_id, reports = hidden.parse(body)
    except hidden.Invalid as invalid:
        raise HTTPException(status_code=400, detail=str(invalid)) from None
    state = snapshots.state_dir()
    try:
        if action == "clear":
            return hidden.clear(state)
        current = snapshots.map_current(state)
        private = snapshots.map_revision(state, current) if current else None
        folders = _library_folders()
        return hidden.hide(
            state, current, private, revision_id, reports,
            servable=lambda sound_id: snapshots.media(state, sound_id, folders, current=private) is not None,
            checked=snapshots.local,
        )
    except hidden.Busy:
        raise HTTPException(status_code=503, detail="Kumi is saving its hidden sounds for another request; try again.") from None
    except OSError:
        _log.warning("Kumi couldn't save its hidden sounds", exc_info=True)
        raise HTTPException(status_code=500, detail="Kumi couldn't save its hidden sounds.") from None


async def _json(request: Request, most: int) -> object:
    """A POST's JSON body, read up to `most` bytes: 413 past it, 400 when it isn't JSON."""
    raw = bytearray()
    async for chunk in request.stream():
        raw += chunk
        if len(raw) > most:
            raise HTTPException(status_code=413, detail="That request is too big.")
    try:
        return json.loads(raw)
    except (ValueError, RecursionError):
        raise HTTPException(status_code=400, detail="The body must be JSON.") from None


@router.post("/map/hidden")
async def map_hidden(request: Request) -> dict:
    """Hide sounds the UI can't play here (their file is gone, or the browser can't decode it) so the map leaves them
    out quietly, or show them all again. Answers `{"hidden": points of the current revision now hidden, "ignored":
    ids not taken, "seq"}`; 400 with `detail` for any other body."""
    return await run_in_threadpool(_map_hidden, await _json(request, _BODY_MAX))


def _design(body: object) -> dict:
    """`POST /design`'s work, off the event loop: the request checked, each sound resolved to its approved file by
    the media guard (`snapshots.media`), the pinned runtime's `kumi design` run, the result listed and answered."""
    try:
        request = designs.parse(body)
    except designs.Invalid as invalid:
        raise HTTPException(status_code=400, detail=str(invalid)) from None
    state = snapshots.state_dir()
    try:
        setup = _load("setup", "setup.py")
        kumi, ffmpeg = designs.runtime(state.parent, setup.pinned_version(), setup.host_target())
    except designs.Unavailable as unavailable:
        raise HTTPException(status_code=503, detail=str(unavailable)) from None
    revision = snapshots.map_current(state)
    current = snapshots.map_revision(state, revision) if revision else None
    sounds = current.get("sounds") if isinstance(current, dict) and isinstance(current.get("sounds"), dict) else {}
    folders = _library_folders()

    def sound(sound_id: str) -> tuple[dict | None, str | None] | None:
        entry = sounds.get(sound_id)
        if not isinstance(entry, dict):
            return None
        name = entry.get("name") if isinstance(entry.get("name"), str) else None
        return snapshots.media(state, sound_id, folders, current=current), name

    def artifact(artifact_id: str) -> dict | None:
        return None if artifact_id in sounds else snapshots.media(state, artifact_id, folders)

    try:
        return designs.render(state, request, designs.resolve(request, revision, sound, artifact), kumi, ffmpeg)
    except designs.Invalid as invalid:
        raise HTTPException(status_code=400, detail=str(invalid)) from None
    except designs.Busy as busy:
        raise HTTPException(status_code=503, detail=str(busy)) from None
    except designs.TimedOut as late:
        raise HTTPException(status_code=504, detail=str(late)) from None
    except OSError:
        _log.warning("Kumi couldn't make or keep a sound", exc_info=True)
        raise HTTPException(status_code=500, detail="Kumi couldn't make or keep that sound.") from None


@router.post("/design")
async def design(request: Request) -> dict:
    """Make a new sound from two to sixteen of the map's sounds or earlier results (by id): cross, fuse or stack, with
    a seed (contract: the engine's interface). Answers the engine's answer with the result's `artifact_id`, which
    `/audio` serves; 400 with `detail` for a request the engine refuses, 503 while Kumi isn't set up or is already
    making two, 504 past two minutes."""
    return await run_in_threadpool(_design, await _json(request, _BODY_MAX))


@router.get("/designs")
def designs_made() -> dict:
    """The sounds made here, newest first (at most 50): the Make panel's variations survive a reload."""
    return {"designs": designs.history(snapshots.state_dir())}


@router.get("/sound")
def sound(id: str = Query(..., min_length=1, max_length=128)) -> dict:  # noqa: A002 - the query parameter is `id`
    """One sound of the current map revision, and whether it can be auditioned; never its path (CC-4)."""
    state = snapshots.state_dir()
    found = snapshots.public_sound(state, id)
    if found is None:
        raise HTTPException(status_code=404, detail="That sound isn't on the current map.")
    return {**found, "playable": snapshots.media(state, id, _library_folders()) is not None}


@router.get("/audio")
def audio(id: str = Query(..., min_length=1, max_length=128)) -> dict:  # noqa: A002 - the query parameter is `id`
    """An artifact's or a current map sound's bytes, as a data URL. Only ids the runtime published are served, only
    from the state folder's artifacts or an approved sample folder, up to 50 MiB, and only as audio or PNG (CC-4,
    `snapshots.media`: `/sound`'s `playable` asks the same). AIFF goes out as WAV: Chromium, and so Hermes Desktop,
    plays no AIFF (`aiff.to_wav`; one it can't convert goes out as it is)."""
    read = snapshots.read_media(snapshots.state_dir(), id, _library_folders())
    if read is None:
        raise HTTPException(status_code=404, detail="Kumi has no audio for that id here.")
    found, data = read
    mime = found["mime"]
    wav = aiff.to_wav(data) if mime.startswith("audio/") else None
    if wav is not None:
        data, mime = wav, "audio/wav"
    encoded = base64.b64encode(data).decode("ascii")
    return {"id": id, "mime": mime, "bytes": len(data), "data_url": f"data:{mime};base64,{encoded}"}


def _palette_map(state: Path) -> Any:
    """The current map revision as the palette maps ids through it (built once per revision)."""
    return palettes.current_map(state, snapshots.map_current(state), lambda rev: snapshots.map_revision(state, rev))


@router.get("/palette")
def palette() -> dict:
    """The palette both UIs and the agent share: the current one's items in order, as current-revision sound ids (an
    item whose file isn't on this map has none), and every palette by name. Never a path (CC-4)."""
    state = snapshots.state_dir()
    return palettes.view(state, _palette_map(state))


def _strings(body: dict, field: str, most: int, *, least: int = 1, pattern: Any = None) -> list[str]:
    """`body[field]`: a list of `least` to `most` ids, each a short string (matching `pattern` when given)."""
    value = body.get(field, [])
    if (not isinstance(value, list) or not least <= len(value) <= most
            or any(not isinstance(each, str) or not 0 < len(each) <= 128 for each in value)
            or (pattern is not None and any(not pattern.fullmatch(each) for each in value))):
        raise palettes.Invalid(f"{field} is a list of {least} to {most:,} ids.")
    return value


def _whole(body: dict, field: str, low: int, high: int) -> int:
    value = body.get(field)
    if not isinstance(value, int) or isinstance(value, bool) or not low <= value <= high:
        raise palettes.Invalid(f"{field} is a whole number from {low} to {high:,}.")
    return value


def _id(body: dict, field: str, pattern: Any) -> str:
    value = body.get(field)
    if not isinstance(value, str) or not pattern.fullmatch(value):
        raise palettes.Invalid(f"{field} isn't one the palette gave: read it again.")
    return value


def _palette_edit(state: Path, body: Any) -> str:
    if not isinstance(body, dict):
        raise palettes.Invalid("The palette takes a JSON object with an action.")
    action = body.get("action")
    if action == "gather":
        sound_ids = _strings(body, "sound_ids", palettes.MOST_GATHERED, least=0)
        artifact_ids = _strings(body, "artifact_ids", palettes.MOST_ARTIFACTS, least=0)
        if not sound_ids and not artifact_ids:
            raise palettes.Invalid("Gather takes sound ids or artifact ids.")
        folders = _library_folders() if artifact_ids else []
        artifacts = [(each, snapshots.palette_artifact(state, each, folders)) for each in artifact_ids]
        if any(name is None for _, name in artifacts):
            raise palettes.Invalid("Kumi can't play that result here, so it can't go in the palette.")
        return palettes.gather(state, _palette_map(state) if sound_ids else None, sound_ids, artifacts)
    if action == "remove":
        return palettes.remove_keys(state, _strings(body, "keys", palettes.MOST_GATHERED, pattern=palettes.ITEM_KEY))
    if action == "move":
        return palettes.move(state, _id(body, "key", palettes.ITEM_KEY), _whole(body, "to", 0, 1_000_000))
    if action in ("rename", "new"):
        return (palettes.rename if action == "rename" else palettes.new)(state, body.get("name"))
    if action in ("use", "forget"):
        return (palettes.use if action == "use" else palettes.forget)(state, _id(body, "id", palettes.PALETTE_ID))
    if action == "reach":
        return palettes.reach(state, _whole(body, "reach", 0, palettes.FARTHEST))
    raise palettes.Invalid("action is gather, remove, move, rename, new, use, forget or reach.")


def _palette_answer(body: object) -> dict:
    """`POST /palette`'s work, off the event loop."""
    state = snapshots.state_dir()
    try:
        said = _palette_edit(state, body)
    except palettes.Invalid as invalid:
        raise HTTPException(status_code=400, detail=str(invalid)) from None
    except palettes.Busy as busy:
        raise HTTPException(status_code=503, detail=str(busy)) from None
    return {"said": said, "palette": palettes.view(state, _palette_map(state))}


@router.post("/palette")
async def palette_edit(request: Request) -> dict:
    """One edit of the palette, from either UI: gather (current-revision sound ids, up to 10,000, and artifact ids
    `/audio` serves, up to 64), remove (item keys), move (key, to), rename and new (name), use and forget (id), reach
    (0 strict, 1-50 loose). Answers what it did in one line and the palette as `GET /palette` has it; 400 says why
    an edit is refused. It changes the plugin's own file only, never a project."""
    return await run_in_threadpool(_palette_answer, await _json(request, _PALETTE_BODY_MAX))


@router.get("/setup")
def setup() -> dict:
    """Where setup stands (the runtime, the DAW component, the next step), read without network or writes by
    setup.py's own `status()` (W5). A setup.py without one, or one that fails, answers that the state is unknown; a
    failure's own words go to the log, never to the page (they can hold paths)."""
    try:
        module = _load("setup", "setup.py") if (_PLUGIN_DIR / "setup.py").is_file() else None
        read_status = getattr(module, "status", None)
        if callable(read_status):
            daw = _setting("daw")
            found = read_status(daw=daw if isinstance(daw, str) else None)
            if isinstance(found, dict):
                # Whether the producer chose a DAW: before that, the page offers setup for each DAW instead of the
                # default one setup.status() reads as.
                return {**found, "daw_chosen": isinstance(daw, str) and daw in schemas.DAWS}
            _log.warning("kumi: setup.status() answered %s, not a dict", type(found).__name__)
            return {"state": "unknown", "message": "Kumi couldn't read its setup state."}
    except Exception:  # health: allow BLE001 -- setup state is shown, never fatal to the UI
        _log.warning("kumi: setup.status() failed", exc_info=True)
        return {"state": "unknown", "message": "Kumi couldn't read its setup state."}
    return {"state": "unknown", "message": "This version of Kumi's plugin can't tell how far setup got."}


@router.get("/job")
def job(id: str = Query(..., min_length=1, max_length=128)) -> dict:  # noqa: A002 - the query parameter is `id`
    """A background job's state and progress."""
    found = snapshots.job(snapshots.state_dir(), id)
    if found is None:
        raise HTTPException(status_code=404, detail="Kumi has no job with that id here.")
    return {key: value for key, value in found.items() if key != "schema"}


@router.get("/script")
def script(sha: str = Query(..., min_length=64, max_length=64), language: str = Query(..., max_length=8)) -> dict:
    """The full code of a script whose approval asked (CC-19), for the Desktop pane to show beside it: the file the
    plugin saved then (`state/scripts/<sha256>.py|.lua`), re-hashed against its name before it's sent
    (`snapshots.script`). Read-only: nothing here runs anything."""
    found = snapshots.script(snapshots.state_dir(), sha, language)
    if found is None:
        raise HTTPException(status_code=404, detail="Kumi has no saved script with that SHA-256 here.")
    return found
