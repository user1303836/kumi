"""Kumi's slash command, `/kumi`: where Kumi stands, in a few lines. It only reads (C6): no change, no approval."""
from __future__ import annotations

from typing import Any

from . import runtime, schemas, tools

DESCRIPTION = "Kumi's state: setup, its runtime, the DAW connection and the open project"


def kumi(host: tools.Host, raw_args: str = "", **kwargs: Any) -> str:
    try:
        status = tools.kumi_status(host, {}, **kwargs)
    except (runtime.Unavailable, runtime.Refused) as failure:
        return f"Kumi: its runtime didn't answer ({failure.reason})"
    daw = schemas.DAW_NAMES.get(status.get("daw"), status.get("daw"))
    setup = status.get("setup") or {}
    if not setup.get("installed"):
        return f"Kumi isn't ready: {setup.get('message')}"
    lines = [f"Kumi {setup.get('version')} is set up ({setup.get('target')}); DAW: {daw}."]
    if not status.get("connected"):
        lines.append(status.get("message") or f"Kumi isn't connected to {daw}: ask the agent to connect.")
        return "\n".join(lines)
    project = status.get("project") or {}
    owner = (status.get("owner") or {}).get("session_id")
    held = "this chat" if owner and owner == kwargs.get("session_id") else f"chat {owner}" if owner else "no chat yet"
    lines.append(f"Connected to {daw}: {project.get('name', 'a project')} (epoch {project.get('epoch')}). "
                 f"Changes are held by {held}.")
    return "\n".join(lines)
