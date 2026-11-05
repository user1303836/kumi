"""Kumi's commands.

- `/kumi` in a chat: where Kumi stands, from this process's runtime. It only reads (C6): no change, no approval.
- `hermes kumi status` in a terminal: the same from what the runtime last published (C3), since the runtime belongs to
  whichever Hermes process started it, never to this short-lived one.
- `hermes kumi setup` in a terminal: setup, asking before each step (C8).
"""
from __future__ import annotations

import argparse
from pathlib import Path
from typing import Any, TextIO

from . import policy, runtime, schemas, setup, snapshots, tools

DESCRIPTION = "Kumi's state: setup, its runtime, the DAW connection and the open project"
CLI_HELP = "Kumi in this Hermes profile: where it stands, and setting it up"


def kumi(host: tools.Host, raw_args: str = "", **kwargs: Any) -> str:
    try:
        status = tools.kumi_status(host, {}, **kwargs)
    except (runtime.Unavailable, runtime.Refused) as failure:
        return f"Kumi: its runtime didn't answer ({_said(failure.reason)})"
    daw = _said(schemas.DAW_NAMES.get(status.get("daw"), status.get("daw")), 40)
    state = status.get("setup") or {}
    warning = [status["approvals"]] if status.get("approvals") else []
    if state.get("state") != "ready":
        return "\n".join([f"Kumi isn't ready: {state.get('message')} Ask the agent to set Kumi up, or run `hermes kumi "
                          "setup` in a terminal (Hermes leaves plugin commands out when `plugins.isolation` is `host`).",
                          *warning])
    lines = [f"Kumi {(state.get('runtime') or {}).get('version')} is set up ({state.get('target')}); DAW: {daw}."]
    if not status.get("connected"):
        lines.append(_said(status.get("message"), 300) or f"Kumi isn't connected to {daw}: ask the agent to connect.")
        return "\n".join([*lines, *warning])
    project = status.get("project") or {}
    owner = _said((status.get("owner") or {}).get("session_id") or "", 80)
    held = "this chat" if owner and owner == kwargs.get("session_id") else f"chat {owner}" if owner else "no chat yet"
    lines.append(f"Connected to {daw}: {_said(project.get('name', 'a project'), 80)} "
                 f"(epoch {_said(project.get('epoch'), 40)}). Changes are held by {held}.")
    return "\n".join([*lines, *warning])


def status_text(host: tools.Host) -> str:
    """`hermes kumi status`: setup, the DAW setting, the runtime as it last published itself, the folders, and whether
    Kumi reads compressed audio (the ffmpeg allowed at setup, CC-10)."""
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
        pid = f" (pid {_said(holder['pid'], 20)})" if holder.get("pid") else ""
        chat = f", for chat {_said(holder['session_id'], 80)}" if holder.get("session_id") else ""
        lines.append(f"Runtime: running{pid}{chat}.")
        if now.get("connected") and project:
            lines.append(f"Connected to {daw}: {_said(project.get('name', 'a project'), 80)} "
                         f"(epoch {_said(project.get('epoch'), 40)}).")
        else:
            lines.append(f"Not connected to {daw}.")
    folders = host.library_folders()
    lines.append("Sample folders: " + (", ".join(_said(folder, 300) for folder in folders) if folders else "none yet."))
    lines.append(setup.compressed_audio(data)["message"])
    lines.append("Approvals: off in Hermes (approvals.mode: off or --yolo), so Kumi's changes run without asking."
                 if tools.approvals_bypassed(host, chat=False)
                 else "Approvals: Hermes asks before each Kumi change (a chat's /yolo skips that).")
    lines.extend(f"Approvals: {gap}" for gap in tools.approval_gaps(host))
    return "\n".join(lines)


def _said(text: Any, limit: int = policy.LINE) -> str:
    """What a runtime, a snapshot or a setting says, printed as one plain line: no control characters, escapes or
    line breaks that could forge or hide a line here (research-1's review, item 10)."""
    return policy.one_line(text, limit) if text is not None else ""


def register_cli(ctx: Any, host: tools.Host) -> None:
    """`hermes kumi …` (Hermes's `register_cli_command`): its own argparse tree, one subcommand per job."""
    ctx.register_cli_command("kumi", help=CLI_HELP, setup_fn=cli_parser, handler_fn=lambda args: cli(host, args),
                             description="Kumi works in your DAW (Ableton Live, REAPER or Renoise) from Hermes.")


def cli_parser(parser: argparse.ArgumentParser) -> None:
    actions = parser.add_subparsers(dest="kumi_action")
    actions.add_parser("status", help="where Kumi stands in this profile (what `hermes kumi` alone shows)")
    setup_parser = actions.add_parser("setup", help="set Kumi up in this profile, asking before each step")
    setup_parser.add_argument("--daw", choices=schemas.DAWS, help="the DAW to set up for (default: the DAW setting)")
    setup_parser.add_argument("--use", action="store_true",
                              help="make that DAW Kumi's DAW in this profile, even when another one is set up")


def cli(host: tools.Host, args: argparse.Namespace) -> int:
    if getattr(args, "kumi_action", None) == "setup":
        return cli_setup(host, getattr(args, "daw", None) or host.daw(), use=getattr(args, "use", False) is True)
    print(status_text(host))  # `status`, or no action at all
    return 0


def cli_setup(host: tools.Host, daw: str, *, use: bool = False, stdin: TextIO | None = None,
              stdout: TextIO | None = None) -> int:
    """setup.cli_main with this profile's plugin data and DAW setting, which an approved step making `daw` Kumi's DAW
    sets. The DAW step uses a runtime this command starts, which stops before the command ends."""
    try:
        return setup.cli_main(daw, setup_daw=tools.setup_daw_caller(host, daw, {}), data_dir=host.data_dir(),
                              plugin_dir=host.plugin_dir, stdin=stdin, stdout=stdout, current=host.daw(), use=use,
                              make_current=host.set_daw)
    finally:
        host.runtimes.stop_all()
