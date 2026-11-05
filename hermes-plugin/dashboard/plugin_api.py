"""Kumi's backend for its Desktop and web-dashboard halves (contract C4): `/api/plugins/kumi/…`.

Read-only by design. Every route is a GET over the snapshots `kumi serve` publishes in this profile's plugin data
(contract C3), re-resolved on every request. Nothing here starts, stops or talks to the runtime, the DAW or the
network, and no route writes anything: a change in the DAW only ever comes from a tool call the user approves, which
the UI asks for by putting a request into the chat.
"""

from __future__ import annotations

import base64
import importlib.util
import logging
import sys
from pathlib import Path
from typing import Any

from fastapi import APIRouter, HTTPException, Query

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


def _liveness(state: Path) -> dict:
    own = snapshots.owner(state)
    return {"connected": not own["stale"], "stale": own["stale"]}


@router.get("/status")
def status() -> dict:
    """Setup and connection, the project, the owner's session, and each snapshot's revision."""
    return snapshots.status(snapshots.state_dir())


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
    """A sound-map revision (the current one by default): points, groups and sounds, never a file's path (CC-4)."""
    state = snapshots.state_dir()
    current = snapshots.map_current(state)
    public = snapshots.public_map(state, revision or current)
    out: dict = {"revision": None, "points": [], "groups": {}, "sounds": {}, **(public or {}), **_liveness(state)}
    out["current"] = current
    return out


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
    `snapshots.media`: `/sound`'s `playable` asks the same)."""
    read = snapshots.read_media(snapshots.state_dir(), id, _library_folders())
    if read is None:
        raise HTTPException(status_code=404, detail="Kumi has no audio for that id here.")
    found, data = read
    encoded = base64.b64encode(data).decode("ascii")
    return {"id": id, "mime": found["mime"], "bytes": len(data), "data_url": f"data:{found['mime']};base64,{encoded}"}


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
                return found
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
