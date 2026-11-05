"""Kumi's commands, which only read (C6): no change, no approval.

- `/kumi` in a chat: where Kumi stands, from this process's runtime.
- `hermes kumi status` in a terminal: the same from what the runtime last published (C3), since the runtime belongs to
  whichever Hermes process started it, never to this short-lived one.
"""
from __future__ import annotations

import argparse
from pathlib import Path
from typing import Any

from . import runtime, schemas, snapshots, tools

DESCRIPTION = "Kumi's state: setup, its runtime, the DAW connection and the open project"
CLI_HELP = "Kumi in this Hermes profile: where it stands"


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


def status_text(host: tools.Host) -> str:
    """`hermes kumi status`: setup, the DAW setting, the runtime as it last published itself, and the folders."""
    data = Path(host.data_dir())
    daw = schemas.DAW_NAMES.get(host.daw(), host.daw())
    try:
        install = host.install(data)
        lines = [f"Setup: Kumi {install.version} is set up ({install.target}).", f"DAW: {daw}."]
    except runtime.Unavailable as failure:
        return "\n".join([f"Setup: {failure.reason}", f"DAW: {daw}."])
    state = data / "state"
    now = snapshots.status(state)
    owner = snapshots.owner(state)
    holder = owner.get("owner") if isinstance(owner.get("owner"), dict) else {}
    project = now.get("project") if isinstance(now.get("project"), dict) else {}
    if owner.get("age_ms") is None:
        lines.append("Runtime: not running.")
    elif owner.get("stale"):
        lines.append(f"Runtime: not running (last heard {max(0, round(owner['age_ms'] / 1000))} s ago).")
    else:
        pid = f" (pid {holder['pid']})" if holder.get("pid") else ""
        chat = f", for chat {holder['session_id']}" if holder.get("session_id") else ""
        lines.append(f"Runtime: running{pid}{chat}.")
        if now.get("connected") and project:
            lines.append(f"Connected to {daw}: {project.get('name', 'a project')} (epoch {project.get('epoch')}).")
        else:
            lines.append(f"Not connected to {daw}.")
    folders = host.library_folders()
    lines.append("Sample folders: " + (", ".join(folders) if folders else "none yet."))
    return "\n".join(lines)


def register_cli(ctx: Any, host: tools.Host) -> None:
    """`hermes kumi …` (Hermes's `register_cli_command`): its own argparse tree, one subcommand per job."""
    ctx.register_cli_command("kumi", help=CLI_HELP, setup_fn=cli_parser, handler_fn=lambda args: cli(host, args),
                             description="Kumi works in your DAW (Ableton Live, REAPER or Renoise) from Hermes.")


def cli_parser(parser: argparse.ArgumentParser) -> None:
    actions = parser.add_subparsers(dest="kumi_action")
    actions.add_parser("status", help="where Kumi stands in this profile (what `hermes kumi` alone shows)")


def cli(host: tools.Host, args: argparse.Namespace) -> int:
    print(status_text(host))  # `status`, or no action at all
    return 0
