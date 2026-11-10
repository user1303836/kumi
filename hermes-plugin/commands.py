"""Kumi's commands.

- `/kumi` in a chat: where Kumi stands, from this process's runtime. It only reads (C6): no change, no approval.
- `hermes kumi status` in a terminal: the same from what the runtime last published (C3), since the runtime belongs to
  whichever Hermes process started it, never to this short-lived one.
- `hermes kumi setup` in a terminal: setup, asking before each step (C8).
- `hermes kumi doctor` and `/kumi doctor`: what Kumi needs in this profile, each item with what to do (`doctor`). It
  only reads.
- `hermes kumi samples` and `/kumi samples`: the sample folders Kumi may read. `… samples add <folder>` adds one the
  producer names themself, through the folder guard: the terminal asks first, as setup does; in a chat the typed
  command is the producer's yes, in Hermes's own clients only. Neither reads a file: a scan does, and it asks.
"""
from __future__ import annotations

import argparse
from pathlib import Path
import sys
from typing import Any, TextIO

from . import doctor, policy, runtime, schemas, scripts, setup, snapshots, tools

DESCRIPTION = ("Kumi's state: setup, its runtime, the DAW connection and the open project (`doctor`: what Kumi needs; "
               "`samples`: its sample folders, `samples add <folder>` adds one)")
ARGS_HINT = "[doctor | samples [add <folder>]]"
CLI_HELP = "Kumi in this Hermes profile: where it stands, what it needs, its sample folders, and setting it up"


def kumi(host: tools.Host, raw_args: str = "", **kwargs: Any) -> str:
    words = raw_args.strip()
    if words.lower() == "doctor":
        return doctor.report(doctor.checks(host, **kwargs))
    if words.lower().split(None, 1)[:1] == ["samples"]:
        return samples_text(host, words[len("samples"):].strip(), kwargs.get("platform"))
    try:
        status = tools.kumi_status(host, {}, **kwargs)
    except (runtime.Unavailable, runtime.Refused) as failure:
        return f"Kumi: its runtime didn't answer ({_said(failure.reason)})"
    daw = _said(schemas.DAW_NAMES.get(status.get("daw"), status.get("daw")), 40)
    state = status.get("setup") or {}
    warning = [f"Approvals: {tools.changes_mode(host)}.", *([status["approvals"]] if status.get("approvals") else [])]
    if state.get("state") != "ready":
        how = ("Ask Hermes to set Kumi up." if _commands_left_out(host)
               else "Ask Hermes to set Kumi up, or run `hermes kumi setup` in a terminal.")
        return "\n".join([f"Kumi isn't ready: {state.get('message')} {how}", *warning])
    lines = [f"Kumi {(state.get('runtime') or {}).get('version')} is set up "
             f"({setup.platform_name(state.get('target'))}); DAW: {daw}."]
    none_yet = ([] if host.library_folders()
                else [f"Sample folders: none yet. {_how_to_add(host, kwargs.get('platform'))}"])
    if not status.get("connected"):
        lines.append(_said(status.get("message"), 300) or f"Kumi isn't connected to {daw}: ask Hermes to connect.")
        return "\n".join([*lines, *none_yet, *warning])
    project = status.get("project") or {}
    # The chat using Kumi is the one that started its runtime in this process: the runtime's own status names a holder
    # only while a change runs, and never its chat (fresh-user run 1, F13).
    live = host.runtimes.get(host.data_dir())
    owner = live.owner_session if live is not None else None
    using = ("This chat is using Kumi." if owner and owner == kwargs.get("session_id")
             else "Another chat is using Kumi: use that chat, or end it." if owner else "")
    lines.append(f"Connected to {daw}: {_said(project.get('name', 'a project'), 80)}. {using}".rstrip())
    return "\n".join([*lines, *none_yet, *warning])


ADD_IN_TERMINAL = "`hermes kumi samples add <folder>` in a terminal"
SCAN_NEXT = 'Kumi reads it at its next scan: in a chat, ask "Kumi, scan my sample folders".'


def _local_chat(host: tools.Host, platform: Any) -> bool:
    """Whether this chat is in one of Hermes's own clients (its terminal, TUI, Desktop, an editor): where the person
    typing is the producer at their computer. Hermes tells a command its `platform` (`cli`, `tui`, `desktop`, or a
    messaging app's name); without it, the calling session's surface. A messaging app, or a Hermes that says neither,
    isn't one."""
    surface = platform if isinstance(platform, str) and platform else host.session_surface()
    return surface in scripts.LOCAL_SURFACES


def _how_to_add(host: tools.Host, platform: Any) -> str:
    return ("`/kumi samples add <folder>` adds one." if _local_chat(host, platform)
            else f"On your computer, {ADD_IN_TERMINAL} adds one.")


def samples_text(host: tools.Host, rest: str, platform: Any = None) -> str:
    """`/kumi samples`: the sample folders Kumi may read. `/kumi samples add <folder>`: add one the producer names,
    through the folder guard (`snapshots.typed_library_folder`), kept in the setting. The typed command is the
    producer's yes, so it adds only in Hermes's own clients; nothing is read until a scan, whose approval names every
    folder."""
    if not rest:
        folders = host.library_folders()
        if not folders:
            return f"Sample folders: none yet. {_how_to_add(host, platform)}"
        return "\n".join(["Sample folders:", *(f"• {_said(folder, 4096)}" for folder in folders),
                          f"To add one: {_how_to_add(host, platform)}"])
    action, _, named = rest.partition(" ")
    if action.lower() != "add" or not named.strip():
        return "Kumi's samples command is `/kumi samples`, or `/kumi samples add <folder>` with the folder's full path."
    if not _local_chat(host, platform):
        return ("Kumi didn't add the folder: it takes a sample folder from a chat only in Hermes on your computer "
                f"(its terminal or Desktop). There, type `/kumi samples add <folder>`, or run {ADD_IN_TERMINAL}. "
                "Here, ask Hermes to add it: it asks you first.")
    folder, refusal = snapshots.typed_library_folder(named, hermes_home=host.hermes_home())
    if refusal is not None:
        return policy.refused("Kumi didn't add the folder", refusal)
    if not tools.keep_library_folder(host, folder):
        return f"Kumi already has {_said(folder, 4096)}. {SCAN_NEXT}"
    return "\n".join(["Kumi will read the audio files in this folder and its subfolders:", f"• {_said(folder, 4096)}",
                      policy.AUDIO_ONLY, SCAN_NEXT])


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
    lines.append("Sample folders: " + (", ".join(_said(folder, 300) for folder in folders) if folders
                                       else "none yet. Add one with `hermes kumi samples add <folder>`."))
    lines.append(setup.compressed_audio(data)["message"])
    lines.append(f"Approvals: off in Hermes (approvals.mode: off or --yolo), so {tools.UNASKED} run without asking first."
                 if tools.approvals_bypassed(host, chat=False)
                 else f"Approvals: {tools.CHANGES_UNASKED}." if not host.ask_before_changes()
                 else f"Approvals: {tools.CHANGES_ASKED}; a chat's /yolo skips that.")
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
    credits_parser = actions.add_parser("credits", help="Mirelo credits: what's spent, reserved and left, and your "
                                                        "own fixes (each asks you first)")
    choices = credits_parser.add_subparsers(dest="credits_action")
    choices.add_parser("unfreeze", help="let new paid generations be created again, after checking Mirelo")
    release = choices.add_parser("release", help="release a reservation whose charge stayed unknown, after checking "
                                                 "Mirelo's dashboard")
    release.add_argument("reservation_id")
    samples_parser = actions.add_parser("samples", help="the sample folders Kumi may read for your sound map; "
                                                        "`add <folder>` adds one, asking you first")
    samples_choices = samples_parser.add_subparsers(dest="samples_action")
    add = samples_choices.add_parser("add", help="add a sample folder, after showing what Kumi will read")
    add.add_argument("folder", nargs="+", help="the folder's full path; one with spaces needs no quotes")


def cli(host: tools.Host, args: argparse.Namespace) -> int:
    if getattr(args, "kumi_action", None) == "setup":
        return cli_setup(host, getattr(args, "daw", None) or host.daw(), use=getattr(args, "use", False) is True)
    if getattr(args, "kumi_action", None) == "doctor":
        items = doctor.checks(host)
        print(doctor.report(items))
        return 1 if doctor.needs_fixing(items) else 0
    if getattr(args, "kumi_action", None) == "samples":
        return cli_samples(host, getattr(args, "samples_action", None), getattr(args, "folder", None))
    if getattr(args, "kumi_action", None) == "credits":
        return cli_credits(host, getattr(args, "credits_action", None), getattr(args, "reservation_id", None))
    print(status_text(host))  # `status`, or no action at all
    return 0


def credits_line(totals: Any) -> str:
    """The credit ledger's totals in a line (CC-13): Mirelo API credits, never money."""
    if not isinstance(totals, dict):
        return "Mirelo credits: Kumi can't read its credit ledger."
    line = (f"Mirelo credits: {totals.get('spent')} spent and {totals.get('reserved')} reserved, of a cap of "
            f"{totals.get('cap')}; {totals.get('left')} left.")
    if totals.get("frozen"):
        line += " New paid generations are paused: a charge came in above its estimate."
    return line


KIND_WORDS = {"sfx": "sound effect", "audio_to_midi": "audio to MIDI"}


def reservation_lines(held: Any) -> list[str]:
    """The ledger's unsettled reservations, one line each, with the id `release` takes (W3's `reservations()`): those
    still held, then those the producer released by hand, which hold nothing now (CC-13 B-b amendment: a charge Mirelo
    reports for one later is still counted)."""
    rows = [row for row in held if isinstance(row, dict)] if isinstance(held, list) else []
    lines = []
    for released, heading in ((False, "Reserved, not settled yet:"),
                              (True, "Released by you, so nothing is held; a charge Mirelo reports later still "
                                     "counts:")):
        group = [row for row in rows if (row.get("settlement") == "manual_release") is released]
        if group:
            lines.append(heading)
        for row in group:
            job = row.get("job_id")
            amount = f"was {_said(row.get('reserved_credits'), 20)}" if released else _said(row.get("reserved_credits"), 20)
            lines.append(f"• {_said(row.get('reservation_id'), 80)}: {KIND_WORDS.get(row.get('kind'), 'a generation')}, "
                         f"{amount} credits, {f'job {_said(job, 80)}' if isinstance(job, str) and job else 'no job'}, "
                         f"submitted: {_said(row.get('submission') or 'unknown', 40)}")
    return lines


def cli_credits(host: tools.Host, action: str | None, reservation_id: str | None = None, *,
                stdin: TextIO | None = None, stdout: TextIO | None = None) -> int:
    """`hermes kumi credits [unfreeze | release <reservation_id>]` (CC-13's 13-R5): the ledger's totals and its
    unsettled reservations, and the two things only the producer may do, in a terminal, after an interactive yes. No
    tool reaches them."""
    stdin, stdout = stdin or sys.stdin, stdout or sys.stdout
    try:
        ledger = host.ledger()
        print(credits_line(ledger.snapshot()), file=stdout)
        for line in reservation_lines(ledger.reservations()):
            print(line, file=stdout)
    except Exception as failure:
        print(credits_line(None), _said(getattr(failure, "reason", None) or ""), file=stdout)
        return 1
    if action is None:
        return 0
    if not (stdin.isatty() and stdout.isatty()):
        print(f"hermes kumi credits {action} asks you first, so it needs a terminal.", file=stdout)
        return 1
    question = ("Let new paid generations be created again? Check Mirelo's dashboard for the charge first."
                if action == "unfreeze" else
                f"Release reservation {_said(reservation_id, 80)}? Only when Mirelo's dashboard shows that job wasn't "
                "charged; Kumi can't tell.")
    print(question, "Go ahead? [y/N] ", end="", file=stdout, flush=True)
    if stdin.readline().strip().lower() not in ("y", "yes"):
        print("Nothing was changed.", file=stdout)
        return 1
    try:
        totals = ledger.unfreeze() if action == "unfreeze" else ledger.release(reservation_id)
    except Exception as failure:
        print("Nothing was changed:", _said(getattr(failure, "message", None) or "the credit ledger refused it."),
              file=stdout)
        return 1
    print(credits_line(totals), file=stdout)
    return 0


def cli_samples(host: tools.Host, action: str | None, words: list[str] | None = None, *,
                stdin: TextIO | None = None, stdout: TextIO | None = None) -> int:
    """`hermes kumi samples [add <folder>]`: the sample folders Kumi may read, and adding one the producer names. The
    folder guard answers for it (`snapshots.typed_library_folder`), the folder is shown as Kumi will read it, and it's
    kept in the setting after an interactive yes, as setup asks. Nothing is read here: a scan reads the folders."""
    stdin, stdout = stdin or sys.stdin, stdout or sys.stdout
    if action != "add":
        folders = host.library_folders()
        print("Sample folders:" if folders else "Sample folders: none yet.", file=stdout)
        for folder in folders:
            print(f"• {_said(folder, 4096)}", file=stdout)
        print("To add one: hermes kumi samples add <folder>", file=stdout)
        return 0
    folder, refusal = snapshots.typed_library_folder(" ".join(words or []), hermes_home=host.hermes_home())
    if refusal is not None:
        print(policy.refused("Kumi didn't add the folder", refusal), file=stdout)
        return 1
    if folder in host.library_folders():
        print(f"Kumi already has {_said(folder, 4096)}. {SCAN_NEXT}", file=stdout)
        return 0
    if not (stdin.isatty() and stdout.isatty()):
        print("hermes kumi samples add asks you first, so it needs a terminal.", file=stdout)
        return 1
    print("Kumi: read the audio files in this folder and its subfolders, to build your sound map:",
          f"• {_said(folder, 4096)}", policy.AUDIO_ONLY, sep="\n  ", file=stdout)
    print("Go ahead? [y/N] ", end="", file=stdout, flush=True)
    if stdin.readline().strip().lower() not in ("y", "yes"):
        print("Nothing was added.", file=stdout)
        return 1
    tools.keep_library_folder(host, folder)
    print(f"Added. {SCAN_NEXT}", file=stdout)
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
