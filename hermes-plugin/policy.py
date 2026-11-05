"""Kumi's approval policy (RESEARCH.md C6, CONTRACT-CHANGES CC-3 and CC-5): which Kumi tool calls the producer
approves first, in what words, and under which rule key. Hermes's own approval gate asks (once, this session, always,
deny) and fails closed with no one there; Kumi's HISTORY makes an approved change reversible, but never replaces the
approval.

Reads and kumi_connect pass. A change or an undo is described by the runtime's own preview (CC-3: the targets by their
current names, values as just read), and a plan the runtime would refuse is blocked before anyone is asked. A folder the
producer adds, and a scan of the folders the runtime reads, are approved by name. Anything else of Kumi's is blocked.

The hook runs for every tool call in Hermes, and one that's slow or blocked stops them all: a call that isn't Kumi's
returns at once, the runtime is never waited for, and a preview gets at most half of Hermes's hook timeout.
"""
from __future__ import annotations

import logging
import re
from typing import Any
import unicodedata

from . import runtime, tools

log = logging.getLogger(__name__)

PREVIEW_TIMEOUT = 10.0
STEP_LINES = 18
LINE = 120
EFFECTS = {"write", "import", "playback"}
PASS = {"kumi_status", "kumi_connect", "kumi_observe", "kumi_read", "kumi_history", "kumi_sounds", "kumi_job"}
FOOTER = "(Values as Kumi read them just now.)"
APPROVED = ("once", "session", "always")
_CONTROL = re.compile(r"[\x00-\x1f\x7f-\x9f\u2028\u2029]+")


def pre_tool_call(host: Any, tool_name: str = "", args: Any = None, **kwargs: Any) -> dict | None:
    """Hermes's `pre_tool_call` for Kumi's tools: None (no approval needed), `approve` with the message and rule key
    the producer sees, or `block` with the reason. Other plugins' tools are left alone. A failure blocks. A call that
    isn't blocked is recorded for its handler (CC-5), with whether it was put to the producer."""
    if not isinstance(tool_name, str) or not tool_name.startswith("kumi_"):
        return None
    args = args if isinstance(args, dict) else {}
    try:
        decision, extra = decide(host, tool_name, args, kwargs)
    except Exception:
        log.exception("Kumi's approval check for %s failed", tool_name)
        return block("Kumi couldn't check this call, so it didn't run. Hermes's log says why.")
    if decision is None or decision["action"] == "approve":
        host.calls.record(tool_name, args, kwargs, {"approved": decision is not None, **extra})
    return decision


def post_approval_response(host: Any, tool_call_id: Any = None, choice: Any = None, **kwargs: Any) -> None:
    """Hermes's `post_approval_response`: a Kumi call the producer denied, or nobody answered, loses its record."""
    if choice not in APPROVED:
        host.calls.forget(tool_call_id)


def post_tool_call(host: Any, tool_name: str = "", tool_call_id: Any = None, **kwargs: Any) -> None:
    """Hermes's `post_tool_call`, after every call however it ended, a blocked one included (another plugin's veto, or
    no one there to approve): the call's record goes, so no later dispatch of the same arguments can run on it."""
    if isinstance(tool_name, str) and tool_name.startswith("kumi_"):
        host.calls.forget(tool_call_id)


def decide(host: Any, tool: str, args: dict, kwargs: dict) -> tuple[dict | None, dict]:
    if tool in PASS:
        return None, {}
    if tool == "kumi_listen":
        return _listen(args), {}
    if tool == "kumi_change":
        return _change(host, args, kwargs), {}
    if tool == "kumi_undo":
        return _undo(host, args, kwargs), {}
    if tool == "kumi_library":
        return _library(host, args, kwargs), {}
    return block(f"{tool} isn't a Kumi tool this plugin knows how to approve, so it didn't run."), {}


def preview_timeout(hook_timeout: float | None) -> float:
    """At most half of Hermes's hook timeout (`plugins.hook_callback_timeout`), and never over 10 s: a hook Hermes times
    out is switched off for a minute, and every tool call is blocked meanwhile."""
    if hook_timeout is None or hook_timeout <= 0:
        return PREVIEW_TIMEOUT
    return min(PREVIEW_TIMEOUT, hook_timeout / 2)


def one_line(text: Any, limit: int = LINE) -> str:
    """`text` as one line of at most `limit` characters: no line breaks, control characters, or invisible format
    characters (zero-width, direction overrides, Unicode tags), so model- or project-written text can neither push the
    runtime's own words out of an approval nor say something a person reading it can't see."""
    visible = "".join(char for char in _CONTROL.sub(" ", str(text)) if unicodedata.category(char) != "Cf")
    line = " ".join(visible.split())
    return line if len(line) <= limit else line[:limit - 1].rstrip() + "…"


def approve(lines: list[str], rule_key: str, footer: str | None = None) -> dict:
    """`lines[0]` is the title; the rest are effects, of which at most 18 are shown, then the footer."""
    title, effects = one_line(lines[0]), [one_line(line) for line in lines[1:]]
    shown = effects[:STEP_LINES]
    if len(effects) > STEP_LINES:
        shown.append(f"… and {len(effects) - STEP_LINES} more steps")
    message = [title, *shown, *([footer] if footer else [])]
    return {"action": "approve", "message": "\n".join(message), "rule_key": rule_key}


def block(message: str) -> dict:
    return {"action": "block", "message": message}


def _live(host: Any) -> runtime.Runtime:
    """This profile's running runtime, without waiting for one that's starting (C6: the hook never waits)."""
    live = host.runtimes.get(host.data_dir())
    if live is None:
        raise runtime.Unavailable("not_connected", "Kumi isn't connected to the DAW: call kumi_connect first.")
    return live


def _ask(host: Any, tool: str, args: dict, kwargs: dict) -> dict:
    """One of the runtime's reads for the hook, within the hook's time."""
    return _live(host).call(tool, args, host.identity(kwargs), timeout=preview_timeout(host.hook_timeout()))


def _reason(failure: Exception) -> str:
    if isinstance(failure, runtime.Unavailable) and failure.code == "timeout":
        return "Kumi didn't answer in time; nothing ran."
    return getattr(failure, "reason", str(failure))


def _listen(args: dict) -> dict | None:
    """Measuring a file, a sound or an artifact is a read. Any other source (a DAW track, which plays the project:
    CC-2) isn't approved here yet, so it doesn't run."""
    refusal = tools.listen_refusal(args)
    return block(f"Kumi didn't measure anything: {refusal}") if refusal else None


def _change(host: Any, args: dict, kwargs: dict) -> dict:
    preview = {key: value for key, value in args.items() if key != "preview"}
    if "expected_epoch" in preview:
        preview["expected_epoch"] = str(preview["expected_epoch"])
    try:
        answer = _ask(host, "change", {**preview, "preview": True}, kwargs)
    except (runtime.Unavailable, runtime.Refused) as failure:
        return block(f"Kumi didn't change anything: {_reason(failure)}")
    if answer.get("state") != "preview":
        return block(f"Kumi didn't change anything: {answer.get('reason') or 'its runtime refused the plan.'}")
    steps = [step for step in answer.get("steps") or [] if isinstance(step, dict)]
    asked = args.get("steps") if isinstance(args.get("steps"), list) else []
    if (not steps or len(steps) != len(asked)
            or any(not isinstance(step.get("text"), str) or not step["text"].strip() for step in steps)
            or any(step.get("effect") not in EFFECTS or not step.get("family") for step in steps)):
        return block("Kumi didn't change anything: its runtime couldn't describe every step of this plan.")
    effects = sorted({one_line(step["effect"], 20) for step in steps})
    families = sorted({one_line(step["family"], 40) for step in steps})
    title = f"Kumi: {answer.get('title') or args.get('title') or 'a change'}"
    return approve([title, *(f"• {step['text']}" for step in steps)], f"kumi:{'+'.join(effects)}:{'+'.join(families)}",
                   footer=FOOTER)


def _undo(host: Any, args: dict, kwargs: dict) -> dict:
    try:
        answer = _ask(host, "undo", {key: value for key, value in args.items() if key != "preview"} | {"preview": True},
                      kwargs)
    except (runtime.Unavailable, runtime.Refused) as failure:
        return block(f"Kumi didn't undo anything: {_reason(failure)}")
    if answer.get("state") != "preview" or not answer.get("undo_available"):
        return block(f"Kumi didn't undo anything: {answer.get('reason') or 'that change can no longer be undone.'}")
    return approve([f"Kumi: {answer.get('text') or 'undo a Kumi change'}"], "kumi:write:undo")


def _library(host: Any, args: dict, kwargs: dict) -> dict | None:
    action = args.get("action")
    if action == "status":
        return None
    if action not in ("add_folder", "scan"):
        return block("Kumi didn't run: kumi_library's action is status, add_folder or scan.")
    if host.runtimes.get(host.data_dir()) is None:
        return block("Kumi didn't read any folder: it isn't connected. Call kumi_connect first.")
    if action == "add_folder":
        folder, refusal = tools.library_folder(args.get("folder"))
        if refusal is not None:
            return block(f"Kumi didn't add the folder: {refusal}")
        return approve([f"Kumi: read the folder {folder} and everything in it, to build your sound map. It never "
                        "changes or moves your files."], f"kumi:folder_read:{folder}")
    try:
        status = _ask(host, "library", {"action": "status"}, kwargs)
    except (runtime.Unavailable, runtime.Refused) as failure:
        return block(f"Kumi didn't scan anything: {_reason(failure)}")
    folders = [folder for folder in status.get("folders") or [] if isinstance(folder, str) and folder]
    if not folders:
        return block("Kumi didn't scan anything: it has no approved sample folders yet. Add one first.")
    return approve(["Kumi: scan these sample folders and update your sound map (it runs in the background and reads "
                    "every audio file in them):", *(f"• {folder}" for folder in folders)], "kumi:folder_read:scan")
