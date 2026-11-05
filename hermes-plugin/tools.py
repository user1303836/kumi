"""Kumi's Hermes tools (RESEARCH.md C1): each handler takes the model's arguments and Hermes's call context, and returns
JSON. A handler never raises into Hermes: what goes wrong comes back as `{"error": {"code", "reason"}}`.
"""
from __future__ import annotations

import json
import logging
import os
from pathlib import Path
from typing import Any, Callable
import uuid

from . import calls, runtime, schemas, setup

log = logging.getLogger(__name__)

PLUGIN = "kumi"
TOOLSET = "kumi"
STATUS_TIMEOUT = 15.0
CONNECT_TIMEOUT = 60.0
READ_TIMEOUT = 30.0
WRITE_TIMEOUT = 120.0
UNSURE_HINT = ("What it did is unknown. Don't send it again: check kumi_history and the project (kumi_observe) "
               "first.")


class Host:
    """What the tools need from Hermes, read on every call: the active profile can change between calls, and with it
    the plugin-data folder and the settings."""

    def __init__(self, ctx: Any, runtimes: runtime.Runtimes, *, data_dir: Callable[[], Path] | None = None,
                 pinned: Callable[[], str | None] = setup.pinned_version,
                 hook_timeout: Callable[[], float | None] | None = None):
        self._ctx = ctx
        self.runtimes = runtimes
        self.calls = calls.CallIds()
        self._data_dir = data_dir
        self._pinned = pinned
        self._hook_timeout = hook_timeout

    def hook_timeout(self) -> float | None:
        """Hermes's `plugins.hook_callback_timeout` (default 30 s; 0 or less: none), read on each call."""
        if self._hook_timeout is not None:
            return self._hook_timeout()
        from hermes_cli.config import load_config_readonly  # Hermes's, documented
        plugins = (load_config_readonly() or {}).get("plugins")
        value = plugins.get("hook_callback_timeout") if isinstance(plugins, dict) else None
        try:
            return 30.0 if value is None else float(value)
        except (TypeError, ValueError):
            return 30.0

    def data_dir(self) -> Path:
        if self._data_dir is not None:
            return self._data_dir()
        # Hermes's, documented: <profile home>/plugin-data/kumi, which it creates (empty) on the first call; the only
        # write a read tool makes.
        from plugins.plugin_storage import plugin_data_dir
        return plugin_data_dir(PLUGIN)

    def daw(self) -> str:
        daw = self._ctx.get_config("daw", "ableton")
        return daw if daw in schemas.DAWS else "ableton"

    def inject_focus(self) -> bool:
        return self._ctx.get_config("inject_focus", True) is not False

    def library_folders(self) -> list[str]:
        folders = self._ctx.get_config("library_folders", [])
        return [folder for folder in folders if isinstance(folder, str) and folder] if isinstance(folders, list) else []

    def set_library_folders(self, folders: list[str]) -> None:
        self._ctx.set_config("library_folders", folders)

    def install(self, data_dir: Path) -> runtime.Install:
        return runtime.installed(data_dir, self._pinned())

    def live(self) -> runtime.Runtime:
        """This profile's running runtime, or Unavailable saying what to do first."""
        data_dir = self.data_dir()
        self.install(data_dir)
        live = self.runtimes.get(data_dir)
        if live is None:
            raise runtime.Unavailable("not_connected", "Kumi isn't connected to the DAW: call kumi_connect first.")
        return live

    def identity(self, kwargs: dict) -> dict:
        """Hermes's identity for one call, which the runtime records and dedups on (C2 `_host`, CC-5). Hermes gives a
        tool handler its session and task; the model's tool-call id comes from the approval hook's record of this
        call, and is null without one (never made up)."""
        call = kwargs.get("kumi_call")
        return {"profile_id": str(getattr(self._ctx, "profile_name", "") or ""),
                "session_id": kwargs.get("session_id") or None,
                "task_id": kwargs.get("task_id") or None,
                "tool_call_id": call.tool_call_id if isinstance(call, calls.Entry) else None,
                "request_id": uuid.uuid4().hex}

    def on_session_finalize(self, session_id: str | None = None, **kwargs: Any) -> None:
        """The chat ended: stop the runtime it started (its DAW stays open)."""
        self.runtimes.stop_session(session_id)


def broadcast(event: str, payload: dict) -> None:
    """Tell this process's Desktop clients a snapshot changed (C5: `plugin.kumi.<event>`)."""
    from hermes_cli.plugin_events import broadcast_plugin_event  # Hermes's, documented
    broadcast_plugin_event(PLUGIN, event, payload)


def kumi_status(host: Host, args: dict, **kwargs: Any) -> dict:
    daw = host.daw()
    report: dict[str, Any] = {"setup": None, "runtime": None, "connected": False, "daw": daw, "project": None,
                              "owner": None, "capabilities": []}
    data_dir = host.data_dir()
    try:
        install = host.install(data_dir)
    except runtime.Unavailable as failure:
        report["setup"] = {"installed": False, "message": failure.reason}
        return report
    report["setup"] = {"installed": True, "version": install.version, "target": install.target}
    live = host.runtimes.get(data_dir)
    if live is None:
        report["message"] = "Kumi's runtime isn't running: kumi_connect starts it and connects to the DAW."
        return report
    status = live.call("status", {}, host.identity(kwargs), timeout=STATUS_TIMEOUT)
    report.update({key: status[key] for key in ("runtime", "connected", "daw", "project", "owner", "capabilities")
                   if key in status})
    return report


def kumi_connect(host: Host, args: dict, **kwargs: Any) -> dict:
    daw = args.get("daw") or host.daw()
    if daw not in schemas.DAWS:
        return {"error": {"code": "invalid", "reason": f"Kumi works in {', '.join(schemas.DAWS)}, not {daw!r}."}}
    data_dir = host.data_dir()
    install = host.install(data_dir)
    live = host.runtimes.get(data_dir)
    if live is not None and live.daw != daw:
        return {"error": {"code": "other_daw", "reason": (
            f"Kumi's runtime in this profile works in {schemas.DAW_NAMES[live.daw]}; it works in "
            f"{schemas.DAW_NAMES[daw]} once the chat that started it ends.")}}
    live = live or host.runtimes.start(data_dir, install, daw, kwargs.get("session_id"),
                                       library_roots=host.library_folders())
    return live.call("connect", {"daw": daw}, host.identity(kwargs), timeout=CONNECT_TIMEOUT)


def _forward(tool: str, timeout: float) -> Callable[..., dict]:
    """A read tool: the runtime's tool of the same name (C2), with the model's arguments as they are."""

    def handler(host: Host, args: dict, **kwargs: Any) -> dict:
        return host.live().call(tool, args, host.identity(kwargs), timeout=timeout)

    handler.__name__ = f"kumi_{tool}"
    return handler


def _write(host: Host, tool: str, args: dict, kwargs: dict) -> dict:
    """A write (C2): sent once. An answer that never came, or can't be read, means the outcome is unknown: reported as
    unsure, never resent, and never called unchanged."""
    live = host.live()
    try:
        return live.call(tool, args, host.identity(kwargs), timeout=WRITE_TIMEOUT)
    except runtime.Unavailable as failure:
        if failure.code not in ("timeout", "stopped", "protocol"):
            raise
        return {"state": "unsure", "reason": failure.reason, "hint": UNSURE_HINT}


def kumi_change(host: Host, args: dict, **kwargs: Any) -> dict:
    change = {key: value for key, value in args.items() if key != "preview"}  # previews are the approval hook's (CC-3)
    if "expected_epoch" in change:
        change["expected_epoch"] = str(change["expected_epoch"])
    return _write(host, "change", change, kwargs)


def kumi_undo(host: Host, args: dict, **kwargs: Any) -> dict:
    return _write(host, "undo", {key: value for key, value in args.items() if key != "preview"}, kwargs)


SOURCE_KEYS = {"file", "sound_id", "map_revision", "artifact_id"}


def listen_refusal(args: dict) -> str | None:
    """Why `kumi_listen` can't measure this, or None: a source is a file, a sound or an artifact. (A DAW track plays the
    project, so it needs its own approval: CC-2.)"""
    for name in ("source", "compare_to"):
        source = args.get(name)
        if source is None and name == "compare_to":
            continue
        if not isinstance(source, dict) or not source or not set(source) <= SOURCE_KEYS:
            return f"{name} is a file, a sound or an artifact here."
    return None


def kumi_listen(host: Host, args: dict, **kwargs: Any) -> dict:
    refusal = listen_refusal(args)  # the approval hook checks this too; a dispatch without it mustn't play the DAW
    if refusal:
        return {"error": {"code": "invalid", "reason": refusal}}
    return host.live().call("listen", args, host.identity(kwargs), timeout=WRITE_TIMEOUT)


def kumi_sounds(host: Host, args: dict, **kwargs: Any) -> dict:
    """The runtime's sound search, as the model may see it: sound ids and what the map says of each, never a file's
    path (CC-4: a path is the media route's alone)."""
    result = host.live().call("sounds", args, host.identity(kwargs), timeout=READ_TIMEOUT)
    items = result.get("items")
    if isinstance(items, list):
        result["items"] = [{key: value for key, value in item.items() if key != "path"} if isinstance(item, dict)
                           else item for item in items]
    return result


def library_folder(folder: Any) -> tuple[str | None, str | None]:
    """A sample folder the producer may approve, as Kumi will keep and read it: an absolute path (on Windows with its
    drive), normalized, never a disk's root or the home folder itself. Returns (folder, None), or (None, why not)."""
    if not isinstance(folder, str) or not folder.strip():
        return None, "name the folder, as a full path."
    if any(ord(char) < 32 or ord(char) == 127 for char in folder):
        return None, "the folder's name has control characters."
    if not Path(folder).is_absolute():
        return None, f"{folder!r} isn't a full path."
    normal = os.path.normpath(folder)
    if os.path.dirname(normal) == normal:
        return None, f"{normal} is a whole disk; name a sample folder."
    if normal == os.path.normpath(os.path.expanduser("~")):
        return None, "that's the whole home folder; name a sample folder."
    return normal, None


def kumi_library(host: Host, args: dict, **kwargs: Any) -> dict:
    """The library tool; a folder the producer approved adding is also kept in the plugin's library_folders setting, so
    the next runtime starts with it (`--library-root`)."""
    action = args.get("action")
    if action not in ("status", "add_folder", "scan"):
        return {"error": {"code": "invalid", "reason": "action is status, add_folder or scan."}}
    if action == "add_folder":
        folder, refusal = library_folder(args.get("folder"))
        if refusal is not None:
            return {"error": {"code": "invalid", "reason": refusal}}
        args = {**args, "folder": folder}
    result = host.live().call("library", args, host.identity(kwargs), timeout=READ_TIMEOUT)
    if action == "add_folder" and "error" not in result:
        folders = host.library_folders()
        if args["folder"] not in folders:
            host.set_library_folders([*folders, args["folder"]])
    return result


# name, schema, handler, emoji: every one registered every time (catalog rule 6).
TOOLS: tuple[tuple[str, dict, Callable[..., dict], str], ...] = (
    ("kumi_status", schemas.KUMI_STATUS, kumi_status, "🎛️"),
    ("kumi_connect", schemas.KUMI_CONNECT, kumi_connect, "🔌"),
    ("kumi_observe", schemas.KUMI_OBSERVE, _forward("observe", READ_TIMEOUT), "👀"),
    ("kumi_read", schemas.KUMI_READ, _forward("read", READ_TIMEOUT), "📖"),
    ("kumi_change", schemas.KUMI_CHANGE, kumi_change, "🎚️"),
    ("kumi_undo", schemas.KUMI_UNDO, kumi_undo, "↩️"),
    ("kumi_history", schemas.KUMI_HISTORY, _forward("history", READ_TIMEOUT), "🕘"),
    ("kumi_listen", schemas.KUMI_LISTEN, kumi_listen, "📈"),
    ("kumi_sounds", schemas.KUMI_SOUNDS, kumi_sounds, "🔎"),
    ("kumi_library", schemas.KUMI_LIBRARY, kumi_library, "🗂️"),
    ("kumi_job", schemas.KUMI_JOB, _forward("job", READ_TIMEOUT), "⏳"),
)
# What a tool needs before it's shown to the model: kumi_status and kumi_connect always are (RESEARCH.md §5.4).
ALWAYS_SHOWN = {"kumi_status", "kumi_connect"}


def approved_only(name: str, args: dict) -> bool:
    """The calls policy.py puts to the producer: they run only as approved (CC-5)."""
    return name in ("kumi_change", "kumi_undo") or (name == "kumi_library" and args.get("action") in ("add_folder", "scan"))


def handler(name: str, function: Callable[..., dict], host: Host) -> Callable[..., str]:
    """`function` as Hermes calls tool `name`: `(args, **context) -> str`, never raising. The approval hook's record of
    the call (CC-5) gives it the model's tool-call id; a call the producer approves runs only with the arguments the
    approval saw, and never without it."""

    def call(args: Any = None, **kwargs: Any) -> str:
        args = args if isinstance(args, dict) else {}
        record = host.calls.take(name, args, kwargs)
        try:
            if approved_only(name, args) and not (record and record.extra.get("approved")):
                result = {"error": {"code": "not_approved", "reason": (
                    "Kumi didn't run this: it isn't the call the producer approved (its arguments changed after the "
                    "approval, or it came without Hermes's approval step).")}}
            else:
                result = function(host, args, **{**kwargs, "kumi_call": record})
        except runtime.Refused as refused:
            result = refused.as_json()
        except runtime.Unavailable as failure:
            result = {"error": {"code": failure.code, "reason": failure.reason}}
        except Exception:
            log.exception("Kumi's %s failed", name)
            result = {"error": {"code": "failed", "reason": "Kumi's plugin failed; Hermes's log says why."}}
        return json.dumps(result, ensure_ascii=False)

    call.__name__ = name
    return call


def installed_check(host: Host) -> Callable[[], bool]:
    """A tool's `check_fn`: shown once Kumi is set up in this profile (it reads one small file; no runtime call)."""

    def check() -> bool:
        try:
            host.install(host.data_dir())
        except runtime.Unavailable:
            return False
        except Exception:
            log.exception("Kumi couldn't tell whether it's set up")
            return False
        return True

    return check
