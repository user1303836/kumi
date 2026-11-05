"""Kumi's commands.

- `/kumi` in a chat: where Kumi stands, from this process's runtime. It only reads (C6): no change, no approval.
- `hermes kumi status` in a terminal: the same from what the runtime last published (C3), since the runtime belongs to
  whichever Hermes process started it, never to this short-lived one.
- `hermes kumi setup` in a terminal: setup, asking before each step (C8).
- `hermes kumi doctor` and `/kumi doctor`: what Kumi needs in this profile, each item with what to do (`doctor`). It
  only reads.
"""
from __future__ import annotations

import argparse
from pathlib import Path
from typing import Any, TextIO

from . import doctor, policy, runtime, schemas, setup, snapshots, tools

DESCRIPTION = "Kumi's state: setup, its runtime, the DAW connection and the open project (`doctor`: what Kumi needs)"
ARGS_HINT = "[doctor]"
CLI_HELP = "Kumi in this Hermes profile: where it stands, what it needs, and setting it up"


def kumi(host: tools.Host, raw_args: str = "", **kwargs: Any) -> str:
    if raw_args.strip().lower() == "doctor":
        return doctor.report(doctor.checks(host, **kwargs))
    try:
        status = tools.kumi_status(host, {}, **kwargs)
    except (runtime.Unavailable, runtime.Refused) as failure:
        return f"Kumi: its runtime didn't answer ({_said(failure.reason)})"
    daw = _said(schemas.DAW_NAMES.get(status.get("daw"), status.get("daw")), 40)
    state = status.get("setup") or {}
    warning = [status["approvals"]] if status.get("approvals") else []
    if state.get("state") != "ready":
        how = ("Ask Hermes to set Kumi up." if _commands_left_out(host)
               else "Ask Hermes to set Kumi up, or run `hermes kumi setup` in a terminal.")
        return "\n".join([f"Kumi isn't ready: {state.get('message')} {how}", *warning])
    lines = [f"Kumi {(state.get('runtime') or {}).get('version')} is set up "
             f"({setup.platform_name(state.get('target'))}); DAW: {daw}."]
    if not status.get("connected"):
        lines.append(_said(status.get("message"), 300) or f"Kumi isn't connected to {daw}: ask Hermes to connect.")
        return "\n".join([*lines, *warning])
    project = status.get("project") or {}
    # The chat using Kumi is the one that started its runtime in this process: the runtime's own status names a holder
    # only while a change runs, and never its chat (fresh-user run 1, F13).
    live = host.runtimes.get(host.data_dir())
    owner = live.owner_session if live is not None else None
    using = ("This chat is using Kumi." if owner and owner == kwargs.get("session_id")
             else "Another chat is using Kumi: use that chat, or end it." if owner else "")
    lines.append(f"Connected to {daw}: {_said(project.get('name', 'a project'), 80)}. {using}".rstrip())
    return "\n".join([*lines, *warning])


def _commands_left_out(host: tools.Host) -> bool:
    """Whether Hermes leaves plugin commands out, `hermes kumi setup` among them: `plugins.isolation: host`, read as
    Hermes reads it (`hermes_cli.plugin_isolation.isolation_mode`)."""
    plugins = host.hermes_config().get("plugins")
    mode = plugins.get("isolation") if isinstance(plugins, dict) else None
    return str(mode or "").strip().lower().replace("-", "_") == "host"


def status_text(host: tools.Host) -> str:
    """`hermes kumi status`: setup, the DAW setting, the runtime as it last published itself, the folders, and whether
    Kumi reads compressed audio (the ffmpeg allowed at setup, CC-10)."""
    data = Path(host.data_dir())
    daw = schemas.DAW_NAMES.get(host.daw(), host.daw())
    try:
        install = host.install(data)
        lines = [f"Setup: Kumi {install.version} is set up ({setup.platform_name(install.target)}).", f"DAW: {daw}."]
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
            lines.append(f"Connected to {daw}: {_said(project.get('name', 'a project'), 80)}.")
        else:
            lines.append(f"Not connected to {daw}.")
    folders = host.library_folders()
    lines.append("Sample folders: " + (", ".join(_said(folder, 300) for folder in folders) if folders else "none yet."))
    lines.append(setup.compressed_audio(data)["message"])
    lines.append(f"Approvals: off in Hermes (approvals.mode: off or --yolo), so {tools.UNASKED} run without asking first."
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
    actions.add_parser("doctor", help="what Kumi needs in this profile, each item with what to do (it only reads)")
    setup_parser = actions.add_parser("setup", help="set Kumi up in this profile, asking before each step")
    setup_parser.add_argument("--daw", choices=schemas.DAWS, help="the DAW to set up for (default: the DAW setting)")
    setup_parser.add_argument("--use", action="store_true",
                              help="make that DAW Kumi's DAW in this profile, even when another one is set up")


def cli(host: tools.Host, args: argparse.Namespace) -> int:
    if getattr(args, "kumi_action", None) == "setup":
        return cli_setup(host, getattr(args, "daw", None) or host.daw(), use=getattr(args, "use", False) is True)
    if getattr(args, "kumi_action", None) == "doctor":
        items = doctor.checks(host)
        print(doctor.report(items))
        return 1 if doctor.needs_fixing(items) else 0
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
