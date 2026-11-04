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

from . import runtime, schemas, setup

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
                 pinned: Callable[[], str | None] = setup.pinned_version):
        self._ctx = ctx
        self.runtimes = runtimes
        self._data_dir = data_dir
        self._pinned = pinned

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
        """Hermes's identity for one call, which the runtime records (C2 `_host`). Hermes gives a tool handler its
        session and task, never the model's tool-call id: `tool_call_id` stays null until the pre_tool_call hook hands
        it over (CONTRACT-CHANGES CC-5), and the runtime dedups only on a real one."""
        return {"profile_id": str(getattr(self._ctx, "profile_name", "") or ""),
                "session_id": kwargs.get("session_id") or None,
                "task_id": kwargs.get("task_id") or None,
                "tool_call_id": None,
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
    """A write (C2): sent once. An answer that never came means the outcome is unknown: reported as unsure, never
    resent, and never called unchanged."""
    live = host.live()
    try:
        return live.call(tool, args, host.identity(kwargs), timeout=WRITE_TIMEOUT)
    except runtime.Unavailable as failure:
        if failure.code not in ("timeout", "stopped"):
            raise
        return {"state": "unsure", "reason": failure.reason, "hint": UNSURE_HINT}


def kumi_change(host: Host, args: dict, **kwargs: Any) -> dict:
    change = {key: value for key, value in args.items() if key != "preview"}  # previews are the approval hook's (CC-3)
    if "expected_epoch" in change:
        change["expected_epoch"] = str(change["expected_epoch"])
    return _write(host, "change", change, kwargs)


def kumi_undo(host: Host, args: dict, **kwargs: Any) -> dict:
    return _write(host, "undo", {key: value for key, value in args.items() if key != "preview"}, kwargs)


def kumi_sounds(host: Host, args: dict, **kwargs: Any) -> dict:
    """The runtime's sound search, as the model may see it: sound ids and what the map says of each, never a file's
    path (CC-4: a path is the media route's alone)."""
    result = host.live().call("sounds", args, host.identity(kwargs), timeout=READ_TIMEOUT)
    items = result.get("items")
    if isinstance(items, list):
        result["items"] = [{key: value for key, value in item.items() if key != "path"} if isinstance(item, dict)
                           else item for item in items]
    return result


def kumi_library(host: Host, args: dict, **kwargs: Any) -> dict:
    """The library tool; a folder the producer approved adding is also kept in the plugin's library_folders setting, so
    the next runtime starts with it (`--library-root`)."""
    action = args.get("action")
    if action not in ("status", "add_folder", "scan"):
        return {"error": {"code": "invalid", "reason": "action is status, add_folder or scan."}}
    folder = args.get("folder")
    if action == "add_folder":
        if not isinstance(folder, str) or not os.path.isabs(folder):
            return {"error": {"code": "invalid", "reason": "add_folder needs an absolute folder path."}}
        args = {**args, "folder": os.path.normpath(folder)}
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
    ("kumi_listen", schemas.KUMI_LISTEN, _forward("listen", WRITE_TIMEOUT), "📈"),
    ("kumi_sounds", schemas.KUMI_SOUNDS, kumi_sounds, "🔎"),
    ("kumi_library", schemas.KUMI_LIBRARY, kumi_library, "🗂️"),
    ("kumi_job", schemas.KUMI_JOB, _forward("job", READ_TIMEOUT), "⏳"),
)
# What a tool needs before it's shown to the model: kumi_status and kumi_connect always are (RESEARCH.md §5.4).
ALWAYS_SHOWN = {"kumi_status", "kumi_connect"}


def handler(function: Callable[..., dict], host: Host) -> Callable[..., str]:
    """`function` as Hermes calls a tool: `(args, **context) -> str`, never raising."""

    def call(args: Any = None, **kwargs: Any) -> str:
        try:
            result = function(host, args if isinstance(args, dict) else {}, **kwargs)
        except runtime.Refused as refused:
            result = refused.as_json()
        except runtime.Unavailable as failure:
            result = {"error": {"code": failure.code, "reason": failure.reason}}
        except Exception:
            log.exception("Kumi's %s failed", getattr(function, "__name__", "tool"))
            result = {"error": {"code": "failed", "reason": "Kumi's plugin failed; Hermes's log says why."}}
        return json.dumps(result, ensure_ascii=False)

    call.__name__ = getattr(function, "__name__", "kumi_tool")
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
