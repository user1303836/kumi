"""Kumi's Hermes tools (RESEARCH.md C1): each handler takes the model's arguments and Hermes's call context, and returns
JSON. A handler never raises into Hermes: what goes wrong comes back as `{"error": {"code", "reason"}}`.
"""
from __future__ import annotations

import json
import logging
from pathlib import Path
from typing import Any, Callable
import uuid

from . import runtime, schemas, setup

log = logging.getLogger(__name__)

PLUGIN = "kumi"
TOOLSET = "kumi"
STATUS_TIMEOUT = 15.0
CONNECT_TIMEOUT = 60.0


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
        from plugins.plugin_storage import plugin_data_dir  # Hermes's, documented: <profile home>/plugin-data/kumi
        return plugin_data_dir(PLUGIN)

    def daw(self) -> str:
        daw = self._ctx.get_config("daw", "ableton")
        return daw if daw in schemas.DAWS else "ableton"

    def install(self, data_dir: Path) -> runtime.Install:
        return runtime.installed(data_dir, self._pinned())

    def identity(self, kwargs: dict) -> dict:
        """Hermes's identity for one call, which the runtime records and deduplicates by (C2 `_host`)."""
        return {"profile_id": str(getattr(self._ctx, "profile_name", "") or ""),
                "session_id": str(kwargs.get("session_id") or ""),
                "tool_call_id": str(kwargs.get("tool_call_id") or ""),
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
    live = live or host.runtimes.start(data_dir, install, daw, kwargs.get("session_id"))
    return live.call("connect", {"daw": daw}, host.identity(kwargs), timeout=CONNECT_TIMEOUT)


# name, schema, handler, emoji: every one registered every time (catalog rule 6).
TOOLS: tuple[tuple[str, dict, Callable[..., dict], str], ...] = (
    ("kumi_status", schemas.KUMI_STATUS, kumi_status, "🎛️"),
    ("kumi_connect", schemas.KUMI_CONNECT, kumi_connect, "🔌"),
)


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
