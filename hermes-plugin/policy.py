"""Kumi's approval policy (RESEARCH.md C6, CONTRACT-CHANGES CC-3): which Kumi tool calls the producer approves first,
in what words, and under which rule key. Hermes's own approval gate asks (once, this session, always, deny) and fails
closed with no one there; Kumi's HISTORY makes an approved change reversible, but never replaces the approval.

Reads and kumi_connect pass. A change or an undo is described by the runtime's own preview (CC-3: the targets by their
current names, values as just read), and a plan the runtime would refuse is blocked before anyone is asked. A folder
the producer adds, and a scan of the approved folders, are approved by name. Anything else of Kumi's is blocked.
"""
from __future__ import annotations

import logging
from typing import Any

from . import runtime

log = logging.getLogger(__name__)

# The preview is asked for inside Hermes's pre_tool_call hook, which Hermes stops after 30 s: well within that.
PREVIEW_TIMEOUT = 10.0
MESSAGE_LINES = 20
PASS = {"kumi_status", "kumi_connect", "kumi_observe", "kumi_read", "kumi_history", "kumi_listen", "kumi_sounds",
        "kumi_job"}


def pre_tool_call(host: Any, tool_name: str = "", args: Any = None, **kwargs: Any) -> dict | None:
    """Hermes's `pre_tool_call` for Kumi's tools: None (no approval needed), `approve` with the message and rule key
    the producer sees, or `block` with the reason. Other plugins' tools are left alone. A failure blocks."""
    if not isinstance(tool_name, str) or not tool_name.startswith("kumi_"):
        return None
    args = args if isinstance(args, dict) else {}
    try:
        return decide(host, tool_name, args, kwargs)
    except Exception:
        log.exception("Kumi's approval check for %s failed", tool_name)
        return block("Kumi couldn't check this call, so it didn't run. Hermes's log says why.")


def decide(host: Any, tool: str, args: dict, kwargs: dict) -> dict | None:
    if tool in PASS:
        return None
    if tool == "kumi_change":
        return _change(host, args, kwargs)
    if tool == "kumi_undo":
        return _undo(host, args, kwargs)
    if tool == "kumi_library":
        return _library(host, args)
    return block(f"{tool} isn't a Kumi tool this plugin knows how to approve, so it didn't run.")


def approve(message: str, rule_key: str) -> dict:
    lines = message.splitlines()
    if len(lines) > MESSAGE_LINES:
        lines = lines[:MESSAGE_LINES - 1] + [f"… and {len(lines) - MESSAGE_LINES + 1} more"]
    return {"action": "approve", "message": "\n".join(lines), "rule_key": rule_key}


def block(message: str) -> dict:
    return {"action": "block", "message": message}


def _preview(host: Any, tool: str, args: dict, kwargs: dict) -> dict:
    """The runtime's preview of a change or an undo (CC-3): it writes nothing and takes no lease."""
    live = host.live()
    preview = {key: value for key, value in args.items() if key != "preview"}
    if tool == "change" and "expected_epoch" in preview:
        preview["expected_epoch"] = str(preview["expected_epoch"])
    return live.call(tool, {**preview, "preview": True}, host.identity(kwargs), timeout=PREVIEW_TIMEOUT)


def _change(host: Any, args: dict, kwargs: dict) -> dict:
    try:
        preview = _preview(host, "change", args, kwargs)
    except (runtime.Unavailable, runtime.Refused) as failure:
        return block(f"Kumi didn't change anything: {failure.reason}")
    if preview.get("state") != "preview":
        return block(f"Kumi didn't change anything: {preview.get('reason') or 'its runtime refused the plan.'}")
    steps = [step for step in preview.get("steps") or [] if isinstance(step, dict)]
    if not steps or any(not isinstance(step.get("text"), str) or not step["text"] for step in steps):
        return block("Kumi didn't change anything: its runtime couldn't describe every step.")
    effects = sorted({str(step.get("effect") or "write") for step in steps})
    families = sorted({str(step.get("family") or step.get("tool") or "change") for step in steps})
    title = preview.get("title") or args.get("title") or "Kumi change"
    lines = [f"Kumi: {title}", *(f"• {step['text']}" for step in steps), "(Values as Kumi read them just now.)"]
    return approve("\n".join(lines), f"kumi:{'+'.join(effects)}:{'+'.join(families)}")


def _undo(host: Any, args: dict, kwargs: dict) -> dict:
    try:
        preview = _preview(host, "undo", args, kwargs)
    except (runtime.Unavailable, runtime.Refused) as failure:
        return block(f"Kumi didn't undo anything: {failure.reason}")
    if preview.get("state") != "preview" or not preview.get("undo_available"):
        reason = preview.get("reason") or "that change can't be undone."
        return block(f"Kumi didn't undo anything: {reason}")
    return approve(f"Kumi: {preview.get('text') or 'Undo a Kumi change'}", "kumi:write:undo")


def _library(host: Any, args: dict) -> dict | None:
    action = args.get("action")
    if action == "status":
        return None
    if action == "add_folder":
        folder = args.get("folder")
        if not isinstance(folder, str) or not folder:
            return block("Kumi didn't add a folder: name the folder.")
        return approve(f"Kumi: read the folder {folder} and everything in it, to build your sound map. "
                       "It reads the folder; it never changes or moves your files.", f"kumi:folder_read:{folder}")
    if action == "scan":
        folders = host.library_folders()
        listed = "\n".join(f"• {folder}" for folder in folders) or "• (no folders yet)"
        return approve(f"Kumi: scan your approved sample folders and update the sound map (it runs in the background "
                       f"and reads every audio file in them):\n{listed}", "kumi:folder_read:scan")
    return block("Kumi didn't run: kumi_library's action is status, add_folder or scan.")
