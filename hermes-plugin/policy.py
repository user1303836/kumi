"""Kumi's approval policy (RESEARCH.md C6, CONTRACT-CHANGES CC-3 and CC-5): which Kumi tool calls the producer
approves first, in what words, and under which rule key. Hermes's own approval gate asks (once, this session, always,
deny) and fails closed with no one there; Kumi's HISTORY makes an approved change reversible, but never replaces the
approval.

Reads and kumi_connect pass. A change or an undo is described by the runtime's own preview (CC-3: the targets by their
current names, values as just read), and a plan the runtime would refuse is blocked before anyone is asked. A folder the
producer adds, and a scan of the folders the runtime reads, are approved by name; setup's install and adopt in the
words of the plan they carry out (C8). Anything else of Kumi's is blocked.

Nobody is there in a scheduled job (Hermes's cron), and Hermes can pass an approval anyway (`approvals.cron_mode:
approve`, or a chat's earlier "Always", which Hermes checks before it looks for a person). So a scheduled job only reads
what Kumi last published: it never starts, connects, sets up or calls the runtime, which a gateway's chats share with
it. Where nobody can answer an approval (a one-shot `hermes -q`, a webhook), nothing that needs one runs.

The hook runs for every tool call in Hermes, and one that's slow or blocked stops them all: a call that isn't Kumi's
returns at once, the runtime is never waited for, and a preview gets at most half of Hermes's hook timeout.
"""
from __future__ import annotations

import logging
import re
import textwrap
from typing import Any
import unicodedata

from . import calls, runtime, setup, snapshots, tools

log = logging.getLogger(__name__)

PREVIEW_TIMEOUT = 10.0
LINE = 120
# How the approval's first line labels the model's title as Hermes's, never Kumi's: the title is cut so the whole line,
# label and quotes included, stays within LINE (rgzm, #1055).
TITLE_LABEL = ("Hermes's change \"", "\", which Kumi will make:")
TITLE = LINE - len("".join(TITLE_LABEL))
# The longest line of a change's preview the runtime writes (W1: values first, each name quoted and capped at 40, one
# line of at most 200). A longer one is never cut: the approval is blocked (research-1's review, item 2).
STEP_LINE = 200
TOO_LONG = ("Kumi didn't run this: its approval is longer than this window shows whole, and Kumi asks only with every "
            "line in view. Split it into smaller changes, or approve it in Hermes Desktop.")
# A track listen's one line: which track or the mix, the exact beats, and what's put back after. The runtime writes it
# and bounds the names in it, so it's cut only far past what a preview says.
PLAYBACK_LINE = 600
EFFECTS = {"write", "import", "playback"}
PASS = {"kumi_status", "kumi_connect", "kumi_observe", "kumi_read", "kumi_history", "kumi_sounds", "kumi_job"}
# What the producer always approves first. What a scheduled job may call is tools.SCHEDULED_READS.
ASKED = {"kumi_change", "kumi_undo", "kumi_setup"}
UNATTENDED = ("Nobody is here to approve this (a one-shot query, a webhook or an API call), so Kumi didn't run it. Ask "
              "in a chat.")
UNATTENDED_CANCEL = ("Kumi stops a scan only for someone in a chat, and this call came from a webhook or an API call, so "
                     "the scan goes on.")
FOOTER = "(Values as Kumi read them just now.)"
APPROVED = ("once", "session", "always")
_CONTROL = re.compile(r"[\x00-\x1f\x7f-\x9f\u2028\u2029]+")
# Where Hermes shows less than an approval's whole text (v0.22.7): the terminal's approval panel, at most 10 rows of
# 64 columns (`hermes_cli/cli_tui_mixin.py:205`), and chat apps that cut the reason at a length (`_EA_REASON_BUDGET`).
# Desktop shows it whole. Kumi asks only where every line is shown (rgzm, #954 LOW 17; research-1's review, item 3).
PANEL_ROWS, PANEL_COLUMNS = 10, 64
REASON_BUDGETS = {"telegram": 500, "slack": 500, "discord": 300}
_TELEGRAM_ESCAPED = set(r"_*[]()~`>#+-=|{}.!\\")


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
    no one there to approve): the call's record goes, so no later dispatch of the same arguments can run on it. A
    search or a listen that went well is announced to Desktop (CC-9)."""
    if isinstance(tool_name, str) and tool_name.startswith("kumi_"):
        host.calls.forget(tool_call_id)
        tools.announce(host, tool_name, kwargs.get("result"), status=kwargs.get("status"),
                       session_id=kwargs.get("session_id"), tool_call_id=tool_call_id)


def decide(host: Any, tool: str, args: dict, kwargs: dict) -> tuple[dict | None, dict]:
    """The decision for one Kumi call. In a scheduled job, only a published read passes, before anything else is
    looked at. Where nobody can answer an approval, a call that needs one is blocked whatever Hermes's settings or an
    earlier "Always" say: before the runtime is asked when it always needs one, else after its decision."""
    unattended = host.unattended()
    if unattended == "scheduled" and tool not in tools.SCHEDULED_READS:
        return block(tools.SCHEDULED), {}
    if unattended and tool in ASKED:
        return block(UNATTENDED), {}
    if unattended and tool == "kumi_library" and args.get("action") == "cancel":  # CC-14: a chat's, never a webhook's
        return block(UNATTENDED_CANCEL), {}
    decision, extra = _decide(host, tool, args, kwargs)
    if unattended and decision is not None and decision["action"] == "approve":
        return block(UNATTENDED), {}
    if decision is not None and decision["action"] == "approve" and not decision["rule_key"].startswith("kumi:setup:"):
        decision = chat_scoped(decision, args, kwargs)
        if not shows_whole(decision["message"], host.session_surface()):
            return block(TOO_LONG), {}
    return decision, extra


# kumi-adv's decision: "this session" and "always" cover a kind of Kumi change within one chat, the convenience quick
# tweaks need, never a new chat, a scheduled job or a webhook, which Hermes then asks about or refuses. So the rule key
# ends with the chat's session id (Hermes's, as C1's identity has it); without one, with the plan's own digest, never a
# family alone. A one-shot run that resumes a chat (`hermes chat -q --resume`) is that chat, and its approvals apply
# there. Setup's key is its plan's digest already (and a paid call's will be its quote's). Hermes keeps an "always" in
# its config's command allowlist; Kumi never edits that. The approval says so in one row, so the steps keep the rest
# of the window (research-1's copy review, P2); the docs say the whole rule.
CHAT_ONLY = "Kumi's approvals cover this chat only."


def chat_scoped(decision: dict, args: dict, kwargs: dict) -> dict:
    rule_key = decision["rule_key"]
    session = kwargs.get("session_id")
    scope = session.strip() if isinstance(session, str) and session.strip() else f"plan-{calls.digest(args)}"
    return {**decision, "rule_key": f"{rule_key}:{scope}", "message": f"{decision['message']}\n{CHAT_ONLY}"}


def _decide(host: Any, tool: str, args: dict, kwargs: dict) -> tuple[dict | None, dict]:
    if tool in PASS:
        return None, {}
    if tool == "kumi_listen":
        return _listen(host, args, kwargs)
    if tool == "kumi_change":
        return _change(host, args, kwargs), {}
    if tool == "kumi_undo":
        return _undo(host, args, kwargs), {}
    if tool == "kumi_library":
        return _library(host, args, kwargs), {}
    if tool == "kumi_setup":
        return _setup(host, args)
    return block(f"{tool} isn't a Kumi tool this plugin knows how to approve, so it didn't run."), {}


def preview_timeout(hook_timeout: float | None) -> float:
    """At most half of Hermes's hook timeout (`plugins.hook_callback_timeout`), and never over 10 s: a hook Hermes times
    out is switched off for a minute, and every tool call is blocked meanwhile."""
    if hook_timeout is None or hook_timeout <= 0:
        return PREVIEW_TIMEOUT
    return min(PREVIEW_TIMEOUT, hook_timeout / 2)


_JOINERS = {"\u200c", "\u200d"}  # format characters words need: ZWNJ (Persian, Indic) and ZWJ (emoji, scripts)


def one_line(text: Any, limit: int = LINE) -> str:
    """`text` as one line of at most `limit` characters: no line breaks, control characters, or invisible format
    characters (zero-width spaces, direction overrides, Unicode tags; the two joiners words need stay), so model- or
    project-written text can neither push the runtime's own words out of an approval nor say something a person
    reading it can't see."""
    visible = "".join(char for char in _CONTROL.sub(" ", str(text))
                      if char in _JOINERS or unicodedata.category(char) != "Cf")
    line = " ".join(visible.split())
    return line if len(line) <= limit else line[:limit - 1].rstrip() + "…"


def approve(lines: list[str], rule_key: str, footer: str | None = None) -> dict:
    """`lines[0]` is the agent's title, one line of at most 120 characters (its own words); the rest are Kumi's
    effects, every one shown whole, then the footer. An effect longer than a preview line ever is blocks the approval
    rather than being cut, so nothing the change does is hidden."""
    title, effects = one_line(lines[0]), [one_line(line, limit=STEP_LINE + 1) for line in lines[1:]]
    if any(len(effect) > STEP_LINE for effect in effects):
        return block("Kumi didn't run this: one of its steps can't be described in a line Kumi shows whole.")
    message = [title, *effects, *([footer] if footer else [])]
    return {"action": "approve", "message": "\n".join(message), "rule_key": rule_key}


def block(message: str) -> dict:
    return {"action": "block", "message": message}


def _live(host: Any) -> runtime.Runtime:
    """This profile's running runtime, without waiting for one that's starting (C6: the hook never waits), else
    Unavailable: `busy` while one is still stopping, `not_connected` otherwise."""
    return host.runtimes.require(host.data_dir())


def _ask(host: Any, tool: str, args: dict, kwargs: dict) -> dict:
    """One of the runtime's reads for the hook, within the hook's time."""
    return _live(host).call(tool, args, host.identity(kwargs), timeout=preview_timeout(host.hook_timeout()))


def _reason(failure: Exception) -> str:
    if isinstance(failure, runtime.Unavailable) and failure.code == "timeout":
        return "Kumi didn't answer in time; nothing ran."
    return getattr(failure, "reason", str(failure))


def _listen(host: Any, args: dict, kwargs: dict) -> tuple[dict | None, dict]:
    """Measuring a file, a sound or an artifact is a read. A track or the mix plays the project (CC-2): it's asked in
    the words of the runtime's own preview (which track or the mix, the exact beats, what changes for a moment and is
    put back) and how long it plays, and blocked unasked when the runtime would refuse it. The preview's own deadline
    for the real call goes to the handler with the approval (CC-5's record)."""
    refusal = tools.listen_refusal(args)
    if refusal:
        return block(f"Kumi didn't listen: {refusal}"), {}
    if not tools.is_track_listen(args):
        return None, {}
    try:
        answer = _ask(host, "listen", {**tools.listen_args(args), "preview": True}, kwargs)
    except (runtime.Unavailable, runtime.Refused) as failure:
        return block(f"Kumi didn't play anything: {_reason(failure)}"), {}
    text, deadline = answer.get("text"), tools.track_deadline(answer.get("deadline_seconds"))
    if answer.get("state") != "preview" or not isinstance(text, str) or not text.strip():
        return block(f"Kumi didn't play anything: {answer.get('reason') or 'its runtime refused this listen.'}"), {}
    if deadline is None:  # its own bound for the real call (CC-2); nothing is guessed in its place
        return block("Kumi didn't play anything: its runtime didn't say how long this listen may take."), {}
    seconds = answer.get("estimated_seconds")
    about = (f" (about {seconds:.3g} s)" if isinstance(seconds, (int, float)) and not isinstance(seconds, bool)
             and seconds > 0 else "")
    decision = {"action": "approve", "message": one_line(f"Kumi: {text}{about}", PLAYBACK_LINE),
                "rule_key": "kumi:playback:listen"}
    return decision, {"deadline": deadline}


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
    title = agents_title(answer.get("title") or args.get("title") or "a change")
    return approve([title, *(f"• {step['text']}" for step in steps)], f"kumi:{'+'.join(effects)}:{'+'.join(families)}",
                   footer=FOOTER)


def agents_title(title: Any) -> str:
    """A plan's title as the approval's first line: the model's own words, quoted and labelled as Hermes's, never as
    Kumi's (research-1's review, item 3; copy review P22). Every mark that reads as a quote (#994's list: curly, low,
    prime, CJK corner brackets, guillemets…) becomes ', so none can seem to close the quote early; it's cut so the
    line stays whole."""
    words = _QUOTE_MARKS.sub("'", one_line(title, TITLE))
    return f"{TITLE_LABEL[0]}{words}{TITLE_LABEL[1]}"


# What reads as a double quote (#994's QUOTE_MARKS in the UI): a title's own can't close the agent's.
_QUOTE_MARKS = re.compile('["\u201c\u201d\u201e\u201f\u2033\u2036\u3003\uff02\u00ab\u00bb\u300c\u300d\u300e\u300f'
                          '\u301d\u301e\u301f\uff62\uff63\ufe41\ufe42\ufe43\ufe44]')


# CC-3's reasons a change can't be undone, as an undo's preview gives them, in the runtime's own words for each (serve/
# change.rs). The restart's are kumi-adv's D2, the same on every surface.
UNDO_REFUSED = {
    "restarted": ("Kumi restarted since this change, so it can't undo it safely: undo it in {daw} if it's still the "
                  "latest step there, or ask Hermes to set it back."),
    "expired": "It can't be undone any more.",
    "kept": "That change can't be undone now (see its state in HISTORY).",
}


def _undo(host: Any, args: dict, kwargs: dict) -> dict:
    try:
        answer = _ask(host, "undo", {key: value for key, value in args.items() if key != "preview"} | {"preview": True},
                      kwargs)
    except (runtime.Unavailable, runtime.Refused) as failure:
        return block(f"Kumi didn't undo anything: {_reason(failure)}")
    if answer.get("state") != "preview" or not answer.get("undo_available"):
        reason = answer.get("reason") if isinstance(answer.get("reason"), str) else ""
        words = (UNDO_REFUSED[reason].format(daw=setup.DAWS.get(host.daw(), "the DAW")) if reason in UNDO_REFUSED
                 else reason or "that change can no longer be undone.")
        return block(f"Kumi didn't undo anything: {words}")
    return approve([f"Kumi: {answer.get('text') or 'undo a Kumi change'}"], "kumi:write:undo")


def _library(host: Any, args: dict, kwargs: dict) -> dict | None:
    refusal = tools.library_refusal(args)
    if refusal is not None:
        return block(f"Kumi didn't run: {refusal}")
    action = args["action"]
    if action in ("status", "cancel"):  # reads: a cancel only stops this runtime's own scan (CC-14)
        return None
    try:
        _live(host)
    except runtime.Unavailable as failure:
        return block(f"Kumi didn't read any folder: {_reason(failure)}")
    if action == "add_folder":
        folder, refusal = snapshots.library_folder(args.get("folder"), hermes_home=host.hermes_home())
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


def _setup(host: Any, args: dict) -> tuple[dict | None, dict]:
    """Setup (C8): a plan passes (it records the plan in Kumi's own data). Installing or adopting asks in the words of
    the plan it carries out, under a rule key naming that plan, so "always" never covers another download or path; the
    plan's digest is bound to this call (CC-5), and only that plan runs. Its lines are shown verbatim, uncut: setup
    refuses any it couldn't show as they are (control, bidi, zero-width or tag characters, or too long), since a cut
    or cleaned path isn't the path written."""
    action = args.get("action")
    if action == "plan":
        return None, {}
    if action not in ("install", "adopt"):
        return block("Kumi didn't set anything up: kumi_setup's action is plan, install or adopt."), {}
    asked = setup.approval(args.get("daw") or host.daw(), action=action, data_dir=host.data_dir())
    if asked is None:
        return block("Kumi didn't set anything up: run kumi_setup plan first."), {}
    lines = [asked["title"], *(f"• {line}" for line in asked["effects"])]
    message = "\n".join(lines)
    surface = host.session_surface()
    if not shows_whole(message, surface):
        where = "Hermes Desktop or `hermes kumi setup` in a terminal" if surface not in ("cli", "tui") else \
            "`hermes kumi setup` in a terminal"
        return block(f"Kumi didn't set anything up: this setup step is longer than this window shows, and Kumi asks only "
                     f"with every line of it in view. Use {where}: it shows the whole plan before it asks."), {}
    decision = {"action": "approve", "message": message, "rule_key": f"kumi:setup:{asked['digest']}"}
    return decision, {"digest": asked["digest"]}


def shows_whole(message: str, surface: str) -> bool:
    """Whether Hermes shows `message`, an approval's reason, whole on `surface` (`Host.session_surface()`): Desktop
    and any client without a known limit do; the terminal's panel wraps each line on its own (long words broken) into
    at most 10 rows of 64 columns; Telegram, Slack and Discord cut it at a length (Telegram's after escaping). An
    "unknown" surface (Hermes couldn't say) shows nothing whole."""
    if surface == "unknown":
        return False
    if surface in ("cli", "tui"):
        rows = sum(len(textwrap.wrap(line, PANEL_COLUMNS, replace_whitespace=False, drop_whitespace=False) or [""])
                   for line in message.splitlines())
        return rows <= PANEL_ROWS
    budget = REASON_BUDGETS.get(surface)
    if budget is None:
        return True
    length = sum(2 if char in _TELEGRAM_ESCAPED else 1 for char in message) if surface == "telegram" else len(message)
    return length <= budget
