"""Kumi's approval policy (RESEARCH.md C6, CONTRACT-CHANGES CC-3 and CC-5): which Kumi tool calls the producer
approves first, in what words, and under which rule key. Hermes's own approval gate asks (once, this session, always,
deny) and fails closed with no one there; Kumi's HISTORY makes a change reversible, but never replaces an approval
Kumi asks for.

Reads and kumi_connect pass. A change or an undo is described by the runtime's own preview (CC-3: the targets by their
current names, values as just read), and a plan the runtime would refuse is blocked before anyone is asked. By default
the changes the producer asks for in the DAW then run without an approval (UNASKED_BY_DEFAULT); the plugin's "Ask
before each change" asks before each, as these words describe. A folder the producer adds, and a scan of the folders
the runtime reads, are approved by name; setup's install and adopt in the words of the plan they carry out (C8).
Anything else of Kumi's is blocked.

Nobody is there in a scheduled job (Hermes's cron), and Hermes can pass an approval anyway (`approvals.cron_mode:
approve`, or a chat's earlier "Always", which Hermes checks before it looks for a person). So a scheduled job only reads
what Kumi last published: it never starts, connects, sets up or calls the runtime, which a gateway's chats share with
it. Where nobody can answer an approval (a one-shot `hermes -q`, a webhook), nothing that needs one runs.

The hook runs for every tool call in Hermes, and one that's slow or blocked stops them all: a call that isn't Kumi's
returns at once, the runtime is never waited for, and a preview gets at most half of Hermes's hook timeout.
"""
from __future__ import annotations

import collections
import itertools
import json
import logging
import math
from pathlib import Path
import re
import textwrap
import time
from typing import Any
import unicodedata

from . import calls, devices, generate, runtime, scripts, setup, snapshots, tools

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
TOO_LONG = ("Kumi didn't run this: its approval doesn't fit this window, even with like steps grouped, and Kumi asks only "
            "when every step is in view. Split it into smaller changes, each approved on its own (by track, or by kind "
            "of step), or approve it in Hermes Desktop.")
# Where a change's plain approval (one line per step) doesn't show whole, a compact one is tried: adjacent steps of
# one kind share a bullet, every step's own words whole and in plan order. Steps are never grouped across another
# kind's, since the order carries meaning (beastmode). The footer and the chat note share one row. Nothing is dropped
# or cut: if even that doesn't show whole, the plan isn't asked (TOO_LONG).
COMPACT_CLOSING = "(Values read just now. Kumi's approvals cover this chat only.)"
_COMPACT = "_compact"
# A track listen's one line: which track or the mix, the exact beats, and what's put back after. The runtime writes it
# and bounds the names in it, so it's cut only far past what a preview says.
PLAYBACK_LINE = 600
EFFECTS = {"write", "import", "playback"}
PASS = {"kumi_status", "kumi_connect", "kumi_observe", "kumi_read", "kumi_history", "kumi_sounds", "kumi_job",
        "kumi_plugin_knowledge"}
# What the producer approves first: always for setup, and for the rest while "Ask before each change" is on.
# What a scheduled job may call is tools.SCHEDULED_READS.
ASKED = {"kumi_change", "kumi_undo", "kumi_setup", "kumi_script", "kumi_device", "kumi_preset", "kumi_judge",
         "kumi_tune", "kumi_wavetable", "kumi_sound", "kumi_audition", "kumi_groove", "kumi_render", "kumi_form"}
# The owner's decision: by default Kumi makes the changes the producer asks for in the DAW without asking, each in
# HISTORY; the plugin's `ask_before_changes` setting asks before each again. Kumi doesn't approve anything for Hermes
# (catalog rule 12): it just doesn't ask for its own changes, as a plugin whose tools act directly doesn't. Every
# refusal still refuses, unattended and forked turns still don't write, and setup, sample folders and Mirelo's uploads
# and paid jobs are asked either way, as is a script or a device in a chat that read outside content (CC-19).
UNASKED_BY_DEFAULT = {"kumi_change", "kumi_undo", "kumi_preset", "kumi_device", "kumi_script", "kumi_listen",
                      "kumi_transport", "kumi_variations", "kumi_judge", "kumi_tune", "kumi_wavetable", "kumi_sound",
                      "kumi_audition", "kumi_groove", "kumi_render", "kumi_form"}
_ALWAYS_ASKED = "_always_asked"
# Writers asked of nobody, which still need someone in the chat: never unattended (26-w1), nor forked or scheduled.
UNASKED_WRITERS = {"kumi_preset_build", "kumi_design"}
UNATTENDED = ("Nobody is here to approve this (a one-shot query, a webhook or an API call), so Kumi didn't run it. Ask "
              "in a chat.")
UNATTENDED_CANCEL = ("Kumi stops a scan only for someone in a chat, and this call came from a webhook or an API call, so "
                     "the scan goes on.")
FORKED = ("Kumi works only in the producer's own turns, and this call came from Hermes working on this chat in the "
          "background (its review after a turn, or a side question), so it didn't run.")
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
    isn't blocked is recorded for its handler (CC-5), with whether it was cleared to run: put to the producer, or a
    change Kumi makes without asking (`unasked`, UNASKED_BY_DEFAULT)."""
    if not isinstance(tool_name, str) or not tool_name.startswith("kumi_"):
        return None
    args = args if isinstance(args, dict) else {}
    try:
        decision, extra = decide(host, tool_name, args, kwargs)
    except Exception:
        log.exception("Kumi's approval check for %s failed", tool_name)
        return block("Kumi couldn't check this call, so it didn't run. Hermes's log says why.")
    event = decision.pop(_SCRIPT_EVENT, None) if isinstance(decision, dict) else None
    if decision is None or decision["action"] == "approve":
        host.calls.record(tool_name, args, kwargs,
                          {"approved": decision is not None or extra.get("unasked") is True, **extra})
    if event is not None and decision is not None and decision["action"] == "approve":
        _broadcast("script.pending", event)
    return decision


def post_approval_response(host: Any, tool_call_id: Any = None, choice: Any = None, **kwargs: Any) -> None:
    """Hermes's `post_approval_response`: a Kumi call the producer denied, or nobody answered, loses its record."""
    if choice not in APPROVED:
        host.calls.forget(tool_call_id)
        if isinstance(tool_call_id, str) and tool_call_id:
            _DENIED.append(tool_call_id)


def turn_started(host: Any, session_id: Any = None, turn_id: Any = None, **kwargs: Any) -> None:
    """Hermes's `pre_llm_call`, at the start of each of the producer's turns (never a forked one's): that turn is the
    producer's (`calls.Turns`)."""
    host.turns.saw(session_id, turn_id)


def post_tool_call(host: Any, tool_name: str = "", tool_call_id: Any = None, **kwargs: Any) -> None:
    """Hermes's `post_tool_call`, after every call however it ended, a blocked one included (another plugin's veto, or
    no one there to approve): the call's record goes, so no later dispatch of the same arguments can run on it. A
    search or a listen that went well is announced to Desktop (CC-9), and a script's end (CC-19). Any tool's result
    that brings outside content into the chat marks the chat as having read some (CC-19 19a, r2)."""
    try:
        why = scripts.taints(tool_name)
        if why is not None:
            host.trust().taint(kwargs.get("session_id"), why)
    except Exception:
        log.exception("Kumi couldn't note what %s brought into the chat", tool_name)
    if isinstance(tool_name, str) and tool_name.startswith("kumi_"):
        host.calls.forget(tool_call_id)
        tools.announce(host, tool_name, kwargs.get("result"), status=kwargs.get("status"),
                       session_id=kwargs.get("session_id"), tool_call_id=tool_call_id)
        if tool_name in ("kumi_script", "kumi_device"):
            _script_settled(host, tool_name, kwargs.get("result"), session_id=kwargs.get("session_id"),
                            tool_call_id=tool_call_id, args=kwargs.get("args"))


def chat_turn(host: Any, session_id: Any = None, is_first_turn: Any = False, **kwargs: Any) -> None:
    """Hermes's `pre_llm_call`: a chat's first turn, which nothing from outside has reached yet, starts it clean
    (CC-19 r1). A chat Kumi first meets later keeps no record, and counts as having read outside content."""
    if is_first_turn is not True:
        return
    try:
        host.trust().first_turn(session_id, surface=host.session_surface())
    except Exception:
        log.exception("Kumi couldn't note a chat's first turn")


def subagent_started(host: Any, child_session_id: Any = None, **kwargs: Any) -> None:
    """Hermes's `subagent_start`: a delegated subagent's chat, whose task is the model's words, never the producer's,
    so it never runs a script (CC-19)."""
    try:
        host.trust().subagent(child_session_id)
    except Exception:
        log.exception("Kumi couldn't note a delegated subagent's chat")


def decide(host: Any, tool: str, args: dict, kwargs: dict) -> tuple[dict | None, dict]:
    """The decision for one Kumi call. In a scheduled job, only a published read passes, before anything else is
    looked at. A turn Hermes forked in the background gets reads only. Where nobody can answer an approval, a call that
    needs one is blocked whatever Hermes's settings or an earlier "Always" say: before the runtime is asked when it
    always needs one, else after its decision. A change the producer isn't asked about (UNASKED_BY_DEFAULT, with "Ask
    before each change" off) passes after every refusal, with `unasked` and what its approval would have bound."""
    unattended = host.unattended()
    if unattended == "scheduled" and tool not in tools.SCHEDULED_READS:
        return block(tools.SCHEDULED), {}
    read = tool in PASS or palette_read(tool, args)
    if not read and host.turns.forked(kwargs.get("session_id"), kwargs.get("turn_id")):
        return block(FORKED), {}
    if unattended and (tool in ASKED or tool in UNASKED_WRITERS or (tool == "kumi_palette" and not read)):
        return block(UNATTENDED), {}
    if unattended and tool == "kumi_library" and args.get("action") == "cancel":  # CC-14: a chat's, never a webhook's
        return block(UNATTENDED_CANCEL), {}
    decision, extra = _decide(host, tool, args, kwargs)
    compact = decision.pop(_COMPACT, None) if isinstance(decision, dict) else None
    always = decision.pop(_ALWAYS_ASKED, False) if isinstance(decision, dict) else False
    if unattended and decision is not None and decision["action"] == "approve":
        return block(UNATTENDED), {}
    if decision is not None and decision["action"] == "approve" and asks_first(host, tool, always) is False:
        return None, {**extra, "unasked": True}
    if decision is not None and decision["action"] == "approve" and not decision["rule_key"].startswith("kumi:setup:"):
        scoped = not decision["rule_key"].startswith(ONE_THING)
        if scoped:
            decision = chat_scoped(decision, args, kwargs)
        surface = host.session_surface()
        if not shows_whole(decision["message"], surface):
            tight = "\n".join([*compact, COMPACT_CLOSING if scoped else FOOTER]) if compact else None
            if tight is None or not shows_whole(tight, surface):
                return block(TOO_LONG), {}
            decision = {**decision, "message": tight}
    return decision, extra


def asks_first(host: Any, tool: str, always: bool = False) -> bool:
    """Whether the producer is asked before `tool`'s call: always outside UNASKED_BY_DEFAULT, or for a call its
    decision marks always asked (a device in a chat that read outside content); else only with "Ask before each
    change" on."""
    return tool not in UNASKED_BY_DEFAULT or always or host.ask_before_changes()


def palette_read(tool: str, args: dict) -> bool:
    """A kumi_palette read (list, search, neighbours, kit): it passes like kumi_sounds, in forked turns too. Its edits
    change no project, so they're never asked, but they still need someone in the chat."""
    return tool == "kumi_palette" and args.get("action") in tools.PALETTE_READS


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


# A paid call's key names its quote (CC-13's 13-R1): "Always" can never cover another, in this chat or any other, and
# a quote is used once. An upload's key names its file and, like a change's, this chat (amendment A's A-c). A device's
# names its spec (CC-22), and a preset's its file's digest (CC-26's 26-w3): they outlive the chat, so even a clean
# chat's "Always" covers only that one.
ONE_THING = ("kumi:paid:", "kumi:script:", "kumi:device:", "kumi:preset:", "kumi:wavetable:")


def _decide(host: Any, tool: str, args: dict, kwargs: dict) -> tuple[dict | None, dict]:
    if tool in PASS:
        return None, {}
    if tool == "kumi_palette":  # a read, or an edit of the plugin's own file: never asked (decide blocked the rest)
        return None, {}
    if tool == "kumi_listen":
        return _listen(host, args, kwargs)
    if tool == "kumi_change":
        return _change(host, args, kwargs), {}
    if tool == "kumi_undo":
        return _undo(host, args, kwargs)
    if tool == "kumi_library":
        return _library(host, args, kwargs), {}
    if tool == "kumi_setup":
        return _setup(host, args)
    if tool == "kumi_generate":
        return _generate(host, args, kwargs)
    if tool == "kumi_transport":
        return _transport(host, args, kwargs), {}
    if tool == "kumi_script":
        return _script(host, args, kwargs)
    if tool == "kumi_variations":
        return _variations(host, args, kwargs)
    if tool == "kumi_device":
        return _device(host, args, kwargs)
    if tool == "kumi_preset_build":  # Kumi's private copy only: the producer approves at kumi_preset (26-w1)
        return (block(PRESET_BUILD_ONLY_LIVE) if host.daw() != "ableton" else None), {}
    if tool == "kumi_preset":
        return _preset(host, args, kwargs)
    if tool in ("kumi_judge", "kumi_tune"):
        return _judged(host, tool, args), {}
    if tool == "kumi_wavetable":
        return _wavetable(host, args), {}
    if tool == "kumi_design":  # a new sound kept as Kumi's private copy: nothing in the DAW changes
        return None, {}
    if tool in ("kumi_sound", "kumi_audition", "kumi_groove", "kumi_render", "kumi_form"):
        return _live_listening(host, tool[len("kumi_"):], args), {}
    return block(f"{tool} isn't a Kumi tool this plugin knows how to approve, so it didn't run."), {}


def _transport(host: Any, args: dict, kwargs: dict) -> dict | None:
    """Playback (CC-17): stop is never asked; play is asked in the words of the runtime's own preview, under the
    chat-scoped playback class (once, or for this chat), and refused unasked where the runtime would refuse it: while
    Kumi listens or captures, while the DAW records or record is armed, or on a project that changed."""
    refusal = tools.transport_refusal(host, args)
    if refusal is not None:
        return block(refused("Kumi didn't play anything", refusal))
    if args["action"] == "stop":
        return None
    try:
        answer = _ask(host, "transport", {**args, "preview": True}, kwargs)
    except (runtime.Unavailable, runtime.Refused) as failure:
        return block(refused("Kumi didn't play anything", _reason(failure)))
    text = answer.get("text")
    if answer.get("state") != "preview" or not isinstance(text, str) or not text.strip():
        return block(refused("Kumi didn't play anything", answer.get('reason') or 'its runtime refused to play.'))
    return approve([f"Kumi: {one_line(text, LINE)}"], "kumi:playback:transport")


# CC-24: what the plugin never does with takes, in words.
VARIATIONS_REFUSED = {
    "drop": "Kumi for Hermes doesn't drop takes: undo the ones you don't want with kumi_undo and their HISTORY ids.",
    "extract": "Kumi for Hermes doesn't extract takes.",
}


def _variations(host: Any, args: dict, kwargs: dict) -> tuple[dict | None, dict]:
    """CC-24: list is a read. make and change are asked with every take the runtime's preview drew, in its own words,
    and the handler sends that draw's seed and digest, so the producer approves exactly the takes that are written.
    keep is asked with each take's new name. drop and extract are never run here."""
    action = args.get("action")
    if action == "list":
        return None, {}
    if action in VARIATIONS_REFUSED:
        return block(VARIATIONS_REFUSED[action]), {}
    if action not in ("make", "change", "keep"):
        return block("Kumi didn't run this: kumi_variations takes make, change, keep or list."), {}
    asked = {key: value for key, value in args.items() if key != "preview"}
    try:
        answer = _ask(host, "variations", {**asked, "preview": True}, kwargs)
    except (runtime.Unavailable, runtime.Refused) as failure:
        return block(refused("Kumi didn't make any takes", _reason(failure))), {}
    if answer.get("state") != "preview":
        return block(refused("Kumi didn't make any takes", answer.get('reason') or 'its runtime refused them.')), {}
    takes = [take for take in answer.get("takes") or [] if isinstance(take, dict) and isinstance(take.get("take"), str)]
    if not takes:
        return block("Kumi didn't make any takes: its runtime drew none."), {}
    named = answer.get("source")  # serve's label, its name already quoted ("“Bassline” on Bass")
    source = one_line(named, 60) if isinstance(named, str) and named.strip() else "the source"
    if action == "keep":
        lines = [f"Kumi: keep {len(takes)} take{'s' if len(takes) != 1 else ''} of {source}, renamed",
                 *(f"• {one_line(take['take'], 10)} → {quoted(take.get('name_after'), 80)}" for take in takes)]
        return approve(lines, "kumi:write:clip", footer=FOOTER), {}
    if not isinstance(answer.get("seed"), int) or not isinstance(answer.get("takes_digest"), str):
        return block("Kumi didn't make any takes: its runtime didn't say how it drew them."), {}
    verb = "make" if action == "make" else "rewrite"
    lines = [f"Kumi: {verb} {len(takes)} take{'s' if len(takes) != 1 else ''} of {source}, each its own, never over it",
             *(f"• {one_line(_take_line(take), STEP_LINE - 2)}" for take in takes)]
    decision = approve(lines, "kumi:write:clip", footer=FOOTER)
    if decision["action"] == "approve":
        decision[_COMPACT] = [lines[0], "• " + "; ".join(
            f"{one_line(take['take'], 10)} {one_line(take.get('direction') or 'a take', 80)}" for take in takes)]
    return decision, {"seed": answer["seed"], "takes_digest": answer["takes_digest"]}


def _take_line(take: dict) -> str:
    """A drawn take in the runtime's words ("V3 (busier): …", its name and direction first), or by its name and
    direction when the runtime gave no line."""
    line = take.get("line")
    return line if isinstance(line, str) and line.strip() else f"{take['take']} ({take.get('direction') or 'a take'})"


# Judged rounds and tunes (serve's judge and tune): no runtime preview exists for them, so they're asked in words
# built from their arguments, after the plugin's own checks of those arguments.
JUDGED = "Kumi didn't listen"
JUDGED_ONLY_LIVE = f"{JUDGED}. Kumi judges and tunes by ear only in Ableton Live."
JUDGED_PLAYS = ("Live plays that stretch (at most 60 s a pass) through Kumi Ears with Live stopped and not recording, "
                "and puts it back after.")
JUDGED_KEEPS = "Kumi keeps the change only if it helped; otherwise it takes it back itself."
SCRATCH = "It hears trial settings on scratch copies of the run's track in the Set, removed after."


def _judged(host: Any, tool: str, args: dict) -> dict:
    refusal = (tools.judge_refusal if tool == "kumi_judge" else tools.tune_refusal)(args)
    if refusal is not None:
        return block(refused(JUDGED, refusal))
    if host.daw() != "ableton":
        return block(JUDGED_ONLY_LIVE)
    if tool == "kumi_tune":
        knobs = ", ".join(quoted(knob, 40) for knob in args.get("knobs") or [])
        target = quoted(args["target"], 60) if "target" in args else "the run's next target"
        lines = [f"Kumi: tune device {quoted(args['device'], 60)} ({args['how']}{f': {knobs}' if knobs else ''}) "
                 f"toward {target}", JUDGED_PLAYS.replace("that stretch", "the run's stretch"),
                 *([SCRATCH] if args["how"] in ("search", "probe") else []), JUDGED_KEEPS]
        return approve(lines, "kumi:write:tune")
    if "goal" in args:
        start, beats = args["from_beat"], args["beats"]
        heard = f"track {quoted(args['track'], 60)}" if "track" in args else "the mix"
        lines = [f"Kumi: start a judged run on {heard}, beats {start:g}–{start + beats:g}",
                 f"Toward: {_goal_words(args['goal'])}", JUDGED_PLAYS]
    elif "change" in args:
        lines = [f"Kumi: judge this round's change, {quoted(args['change'], 80)}",
                 JUDGED_PLAYS.replace("that stretch", "the run's stretch"), JUDGED_KEEPS]
    else:
        lines = ["Kumi: end the judged run, hearing its stretch once more",
                 JUDGED_PLAYS.replace("that stretch", "the run's stretch")]
    return approve(lines, "kumi:write:judge")


def _goal_words(goal: dict) -> str:
    """A goal in a few words: its loudness, ceiling, reference, focus and targets."""
    parts = []
    if isinstance(goal.get("loudness"), (int, float)):
        parts.append(f"{goal['loudness']:g} LUFS")
    if isinstance(goal.get("true_peak"), (int, float)):
        parts.append(f"true peak at most {goal['true_peak']:g} dBTP")
    reference = goal.get("reference")
    if isinstance(reference, dict):
        named = reference.get("clip_ref") or reference.get("sound_id") or reference.get("artifact_id") or (
            Path(str(reference.get("file", ""))).name)
        parts.append(f"the balance of {quoted(named, 60)}")
    if isinstance(goal.get("focus"), str):
        parts.append(f"{quoted(goal['focus'], 40)} cutting through")
    targets = [t.get("measure") for t in goal.get("targets") or [] if isinstance(t, dict)]
    if targets:
        parts.append(", ".join(one_line(measure, 20) for measure in targets))
    if goal.get("problems", True) is not False:
        parts.append("clearing the problems it finds")
    return "; ".join(parts)


# A wavetable: a new file outside the Set, asked (with "Ask before each change" on) in its arguments' words, under its
# arguments' digest, since the file outlives the chat as a preset does.
WAVETABLE = "Kumi didn't make the wavetable"
WAVETABLE_ONLY_LIVE = f"{WAVETABLE}. Kumi makes wavetables only for Ableton Live's plug-ins."
WAVETABLE_WARNING = ("Writes a new .wav (never over a file) into the plug-in's wavetable folder (Vital's), else Live's "
                     "User Library/Kumi/Wavetables. It stays there after an undo.")


def _wavetable(host: Any, args: dict) -> dict:
    refusal = tools.wavetable_refusal(args)
    if refusal is not None:
        return block(refused(WAVETABLE, refusal))
    if host.daw() != "ableton":
        return block(WAVETABLE_ONLY_LIVE)
    table = args["wavetable"]
    name = quoted(table["name"], 48) if isinstance(table.get("name"), str) else "a wavetable"
    if "from_audio" in table:
        source = table["from_audio"]["source"]
        named = source.get("sound_id") or source.get("artifact_id") or Path(str(source.get("file", ""))).name
        drawn = f"single cycles cut from {quoted(named, 60)}"
    else:
        count = len(table.get("keyframes") or [])
        drawn = f"{count} keyframe{'s' if count != 1 else ''} morphing"
    frames = f", {table['count']} frames" if isinstance(table.get("count"), int) else ""
    lines = [f"Kumi: make {name} for plug-in {quoted(args['device_ref'], 60)}", f"{drawn}{frames}", WAVETABLE_WARNING]
    return approve(lines, f"kumi:wavetable:{calls.digest(args)}")


# Live's other listening tools (serve's sound, audition, groove and render): listens (sound, audition, a render) under
# the playback class kumi_listen's captures use, and a groove round that changes notes as a change; a groove start or
# done only reads the notes, so it's never asked. No runtime preview exists for them: they're asked in words built from
# their arguments, after the plugin's own checks.
LISTENING_ONLY_LIVE = (f"{JUDGED}. Kumi measures, auditions, grooves, renders and reads the form of tracks only in "
                       "Ableton Live.")
LISTENING_PLAYS = ("Live plays it through Kumi Ears with Live stopped and not recording (at most 60 s, a bar before and "
                   "half a bar after included), and puts it back after.")
AUDITION_COPIES = "Their Session clips play from temporary copies in the Arrangement, removed after."
RENDER_KEPT = ("Offline, with no playback: the file is kept in Kumi's own data, and Live's temporary render file is "
               "deleted.")
FORM_PLAYS = ("Live plays it once through Kumi Ears with Live stopped and not recording, as long as the song (up to "
              "{most} here) while you wait; Stop ends it. Everything is put back after.")
FORM_PLAYS_PART = FORM_PLAYS.replace("as long as the song", "that stretch")
GROOVE_KEEPS = "Kumi keeps the change only if the feel moved toward the reference; otherwise it takes it back itself."
GROOVE_APPLY = "A change in HISTORY: kumi_undo takes it back."


def _clock(seconds: float) -> str:
    whole = int(round(seconds))
    return f"{whole // 60}:{whole % 60:02d}"


def _reference_name(reference: dict) -> str:
    return reference.get("clip_ref") or reference.get("sound_id") or reference.get("artifact_id") or (
        Path(str(reference.get("file", ""))).name)


def _beats(args: dict, default: str) -> str:
    start = args.get("from_beat")
    if "beats" in args:
        return f"beats {start:g}–{start + args['beats']:g}"
    return f"{default} from beat {start:g}"


def _live_listening(host: Any, tool: str, args: dict) -> dict | None:
    refusal = tools.LISTENING_REFUSALS[tool](args)
    if refusal is not None:
        return block(refused(JUDGED, refusal))
    if host.daw() != "ableton":
        return block(LISTENING_ONLY_LIVE)
    if tool == "groove":
        if not tools.groove_changes(args):
            return None
        if "change" in args:
            lines = [f"Kumi: judge this groove change, {quoted(args['change'], 80)}", GROOVE_KEEPS]
        else:
            amount = args.get("amount", 1)
            lines = [f"Kumi: move the part's notes toward the reference's feel ({amount:g} of the way)", GROOVE_APPLY]
        return approve(lines, "kumi:write:groove")
    if tool == "sound":
        heard = ", ".join(quoted(track, 40) for track in args["tracks"])
        against = f" against {quoted(args['against'], 40)}" if "against" in args else ""
        lines = [f"Kumi: measure {heard}{against}, {_beats(args, '8 bars')}", LISTENING_PLAYS]
    elif tool == "audition":
        named = [quoted(candidate.get("label") or ("the mix" if candidate.get("mix") else candidate["track"]), 40)
                 for candidate in args["candidates"]]
        clips = all("clip" in candidate for candidate in args["candidates"])
        where = "their Session clips" if clips else _beats(args, "8 beats")
        lines = [f"Kumi: audition {', '.join(named)}, {where}"]
        if "reference" in args:
            name = _reference_name(args["reference"])
            lines.append(f"Against {quoted(name, 60)}, {args.get('reference_seconds', tools.REFERENCE_SECONDS):g} s of it")
        lines.append(LISTENING_PLAYS)
        if any("clip" in candidate for candidate in args["candidates"]):
            lines.append(AUDITION_COPIES)
    elif tool == "form":
        most = _clock(tools.form_budget(host))
        if "beats" in args:
            start = args.get("from_beat", 0)
            lines = [f"Kumi: read the form of beats {start:g}–{start + args['beats']:g}", FORM_PLAYS_PART.format(most=most)]
        else:
            start = args.get("from_beat", 0)
            what = "the whole Arrangement" if not start else f"the Arrangement from beat {start:g} to its end"
            lines = [f"Kumi: read the form of {what}", FORM_PLAYS.format(most=most)]
        if "reference" in args:
            lines.append(f"Beside the form of {quoted(_reference_name(args['reference']), 60)}")
    else:
        start = args["from_beat"]
        lines = [f"Kumi: render track {quoted(args['track'], 60)}'s own clips, beats {start:g}–{start + args['beats']:g}",
                 RENDER_KEPT]
    return approve(lines, f"kumi:playback:{tool}")


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


def refused(head: str, why: Any) -> str:
    """What Kumi didn't do (`head`), then why, as one message. Kumi's own words go on after a colon ("Kumi didn't run
    the script: its runtime refused it."); a sentence of its own (the runtime's words start with a capital) follows a
    full stop, so a message never stacks a second colon ("Kumi didn't undo anything. Kumi can't undo a script: …")."""
    words = str(why).strip()
    return f"{head}. {words}" if words[:1].isupper() else f"{head}: {words}"


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
    put back) and how long it plays, and blocked unasked when the runtime would refuse it. The preview's bounds for
    the real call (CC-2's A2: the window's seconds, getting `compare_to` ready, the native deadline) go to the handler
    with the approval (CC-5's record); a preview without valid ones is blocked."""
    refusal = tools.listen_refusal(args)
    if refusal:
        return block(refused("Kumi didn't listen", refusal)), {}
    unsupported = tools.listen_feature_refusal(host, args) or tools.listen_unsupported(host, args)
    if unsupported:
        return block(refused("Kumi didn't listen", unsupported)), {}
    if not tools.is_track_listen(args):
        return None, {}
    try:
        live = _live(host)
        unsupported = tools.listen_feature_refusal(host, args, live)
        if unsupported:
            return block(refused("Kumi didn't listen", unsupported)), {}
        answer = live.call("listen", {**tools.listen_args(args), "preview": True}, host.identity(kwargs),
                           timeout=preview_timeout(host.hook_timeout()))
    except (runtime.Unavailable, runtime.Refused) as failure:
        return block(refused("Kumi didn't play anything", _reason(failure))), {}
    text = answer.get("text")
    if answer.get("state") != "preview" or not isinstance(text, str) or not text.strip():
        return block(refused("Kumi didn't play anything",
                             answer.get("reason") or "its runtime refused this listen.")), {}
    bounds = tools.track_bounds(answer, args)
    if bounds is None:  # its own bounds for the real call (CC-2); nothing is guessed in their place
        return block("Kumi didn't play anything: its runtime didn't say how long this listen may take."), {}
    decision = {"action": "approve", "message": one_line(f"Kumi: {text} (about {bounds['estimated_seconds']:.3g} s)",
                                                         PLAYBACK_LINE),
                "rule_key": "kumi:playback:listen"}
    if "references" in args or any("from" in reference or "seconds" in reference
                                    for reference in tools.listen_references(args)):
        try:
            lines = _reference_lines(answer.get("reference_windows"), len(tools.listen_references(args)))
        except ValueError:
            return block("Kumi didn't play anything: its runtime didn't identify every prepared reference window."), {}
        decision["message"] += "\n" + "\n".join(lines)
    return decision, {"listen": bounds}


def _reference_lines(value: Any, expected: int) -> list[str]:
    """Names come from authorized native preparation, never request paths; don't hide a last member."""
    if not isinstance(value, list) or len(value) != expected:
        raise ValueError("incomplete reference preview")
    lines = []
    for index, reference in enumerate(value, 1):
        if not isinstance(reference, dict) or not isinstance(reference.get("name"), str) or not reference["name"].strip():
            raise ValueError("missing reference name")
        window = reference.get("window")
        if not isinstance(window, dict):
            raise ValueError("missing measured window")
        start, frames, rate = (window.get(key) for key in ("start_frame", "frame_count", "sample_rate"))
        if (type(start) is not int or start < 0 or type(frames) is not int or frames <= 0
                or type(rate) not in (int, float) or not tools._finite(rate) or rate <= 0):
            raise ValueError("invalid measured window")
        requested = window.get("requested")
        excerpt = f"file frames {start}–{start + frames} at {rate:g} Hz"
        if requested is not None:
            if (not isinstance(requested, dict) or type(requested.get("from")) not in (int, float)
                    or type(requested.get("seconds")) not in (int, float)
                    or not tools._finite(requested["from"]) or not tools._finite(requested["seconds"])
                    or requested["from"] < 0 or not 0 < requested["seconds"] <= 60):
                raise ValueError("invalid requested window")
            excerpt = f"file excerpt from {requested['from']} s for {requested['seconds']} s; {excerpt}"
        name = one_line(reference["name"], 160).replace('"', "'")
        lines.append(f'Reference {index}: "{name}" — {excerpt}.')
    return lines


def _change(host: Any, args: dict, kwargs: dict) -> dict:
    preview = {key: value for key, value in args.items() if key != "preview"}
    if "expected_epoch" in preview:
        preview["expected_epoch"] = str(preview["expected_epoch"])
    try:
        answer = _ask(host, "change", {**preview, "preview": True}, kwargs)
    except (runtime.Unavailable, runtime.Refused) as failure:
        return block(refused("Kumi didn't change anything", _reason(failure)))
    if answer.get("state") != "preview":
        return block(refused("Kumi didn't change anything", answer.get('reason') or 'its runtime refused the plan.'))
    steps = [step for step in answer.get("steps") or [] if isinstance(step, dict)]
    asked = args.get("steps") if isinstance(args.get("steps"), list) else []
    if (not steps or len(steps) != len(asked)
            or any(not isinstance(step.get("text"), str) or not step["text"].strip() for step in steps)
            or any(step.get("effect") not in EFFECTS or not step.get("family") for step in steps)):
        return block("Kumi didn't change anything: its runtime couldn't describe every step of this plan.")
    effects = sorted({one_line(step["effect"], 20) for step in steps})
    families = sorted({one_line(step["family"], 40) for step in steps})
    title = agents_title(answer.get("title") or args.get("title") or "a change")
    rule_key = f"kumi:{'+'.join(effects)}:{'+'.join(families)}"
    warning = daw_undo_lines(answer, steps)
    if warning:
        rule_key += ":daw-undo"  # an "Always" for a change Kumi can undo never covers one it can't, nor the reverse
    decision = approve([title, *warning, *(f"• {step['text']}" for step in steps)], rule_key, footer=FOOTER)
    if decision["action"] == "approve" and (lines := compact_lines(title, steps)):
        decision[_COMPACT] = [lines[0], *warning, *lines[1:]]
    return decision


# CC-20's 20c, when the runtime's preview doesn't give its own words (it does, with the right key for the producer's
# system, and how many presses of its Undo the change takes: 18-r1).
DAW_UNDO = "Kumi can't undo this change: only the DAW's own Undo can take it back."


def daw_undo_lines(answer: dict, steps: list[dict]) -> list[str]:
    """CC-20's 20d: a change with a step only the DAW's own Undo takes back (`undo: "daw"`: a Live rack's added chain,
    a sample loaded onto a pad) says so before its steps, in the runtime's words: 20c's, and how many presses of the
    DAW's Undo it takes (18-r1). They're kept whole, a sentence a line, so none is cut. Serve refuses such a step
    mixed with other kinds (#1137), so the whole change is the DAW's to take back. Empty for a change Kumi can undo."""
    if answer.get("undo") != "daw" and not any(step.get("undo") == "daw" for step in steps):
        return []
    note = answer.get("undo_note")
    words = one_line(note, 1000) if isinstance(note, str) and note.strip() else DAW_UNDO
    return [sentence for sentence in re.split(r"(?<=[.!?])\s+", words) if sentence]


def compact_lines(title: str, steps: list[dict]) -> list[str] | None:
    """A change's approval lines with adjacent steps of one kind sharing a bullet, for a window too small for one line
    per step: `• Set mixer ×3, volumeDb: -4 — on "Drums" · -6 — on "Bass" · …`.
    - Only the repeated step name goes, and a setting's name when every step in the run sets just that one.
    - Each step's own words stay whole, in plan order, as the runtime wrote them.
    - A run is one line, which the window wraps (its steps never cut by Kumi). STEP_LINE bounds what the runtime writes
      for one step; a run's line isn't that, and splitting it at a length would waste the panel's part-filled rows.
    None when no two adjacent steps share a kind: there's nothing to gain."""
    lines, grouped = [title], False
    for tool, run in itertools.groupby(steps, key=lambda step: step.get("tool")):
        texts = [one_line(step["text"], STEP_LINE + 1) for step in run]
        name = tool.replace("_", " ")[:1].upper() + tool.replace("_", " ")[1:] if isinstance(tool, str) else ""
        rests = [_after(text, name) for text in texts] if name else []
        if len(texts) < 2 or not rests or any(rest is None for rest in rests):
            lines.extend(f"• {text}" for text in texts)
            continue
        grouped = True
        head, parts = f"• {name} ×{len(texts)}", rests
        if (shared := _one_setting(rests)) is not None:
            head, parts = f"{head}, {shared}", [rest[len(shared) + 1:] for rest in rests]
        lines.append(f"{head}: {' · '.join(parts)}")
    return lines if grouped else None


def _after(text: str, name: str) -> str | None:
    """What a step's line says after its step name (`volumeDb -4 — on "Drums"`), or None when it doesn't start with
    it as the runtime writes it (then the step keeps its own line)."""
    if not text.startswith(name):
        return None
    rest = text[len(name):]
    if rest.startswith(": ") and rest[2:].strip():
        return rest[2:]
    if rest.startswith(" — ") and rest[3:].strip():
        return rest[1:]
    return None


def _one_setting(rests: list[str]) -> str | None:
    """The setting every step of a run sets, when each sets just that one (`volumeDb`): its name can lead the bullet
    once. None when any step sets more, or another, or none."""
    names = set()
    for rest in rests:
        settings = rest.split(" — on ", 1)[0]
        name, _, value = settings.partition(" ")
        if ", " in settings or not value or rest.startswith("— "):
            return None
        names.add(name)
    return names.pop() if len(names) == 1 else None


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


# How many of a whole change's rows its approval lists one by one; the rest are counted (CC-3 A2, rgzm's A2-b).
UNDO_ROWS = 6


def _undo(host: Any, args: dict, kwargs: dict) -> tuple[dict, dict]:
    """An undo, asked in the runtime's own words (CC-3). A whole change (kumi_undo with no id: this chat's newest; or
    whole_change) lists every row the runtime will undo, newest first, and asks under a key naming the families it
    spans, which no single-row undo's "Always" covers (A2-a). The approval binds the row and the rows' digest, which
    the handler sends back, so a set that moved is refused (A2-c)."""
    asked, refusal = tools.undo_target(host, args, kwargs)
    if refusal is not None:
        return block(refusal), {}
    try:
        answer = _ask(host, "undo", {**asked, "preview": True}, kwargs)
    except (runtime.Unavailable, runtime.Refused) as failure:
        return block(refused("Kumi didn't undo anything", _reason(failure))), {}
    if answer.get("state") != "preview" or not answer.get("undo_available"):
        reason = answer.get("reason") if isinstance(answer.get("reason"), str) else ""
        note = answer.get("note") if isinstance(answer.get("note"), str) and answer["note"].strip() else None
        # CC-3's codes have Kumi's words; any other (CC-20's `daw_undo`, CC-19's `not_undoable`) comes with the
        # runtime's own, which the DAW and the producer's system decide.
        words = (UNDO_REFUSED[reason].format(daw=setup.DAWS.get(host.daw(), "the DAW")) if reason in UNDO_REFUSED
                 else one_line(note, 400) if note else reason or "that change can no longer be undone.")
        return block(refused("Kumi didn't undo anything", words)), {}
    if asked.get("whole_change") is not True:
        return approve([f"Kumi: {answer.get('text') or 'undo a Kumi change'}"], "kumi:write:undo"), {}
    rows = [row for row in answer.get("rows") or [] if isinstance(row, dict)]
    digest = answer.get("rows_digest")
    pending = [row for row in rows if row.get("state") != "undone"]
    # Every row to be undone names its family, so the key names every family undone, and no rule for fewer covers it.
    if (not pending or not isinstance(digest, str) or not digest
            or not all(isinstance(row.get("family"), str) and row["family"].strip() for row in pending)):
        return block("Kumi didn't undo anything: its runtime couldn't say which steps that change made."), {}
    families = sorted({one_line(row["family"], 40) for row in pending})
    # A step's own words, cut to fit its line: HISTORY has them whole, and an undo is never blocked by a long one.
    shown = [f"• {one_line(row.get('text') or row.get('title'), STEP_LINE - 2)}" for row in pending[:UNDO_ROWS]]
    more = len(pending) - len(shown)
    count = "1 step" if len(pending) == 1 else f"{len(pending)} steps, newest first"
    lines = [f"Kumi: undo a whole change, {count}",
             *shown, *([f"• …and {more} more step{'s' if more != 1 else ''}"] if more > 0 else [])]
    bound = {"history_id": asked["history_id"], "rows_digest": digest}
    return approve(lines, f"kumi:write:undo:{'+'.join(families)}"), bound


def _library(host: Any, args: dict, kwargs: dict) -> dict | None:
    refusal = tools.library_refusal(args)
    if refusal is not None:
        return block(refused("Kumi didn't run", refusal))
    action = args["action"]
    if action in ("status", "cancel"):  # reads: a cancel only stops this runtime's own scan (CC-14)
        return None
    try:
        live = _live(host)
    except runtime.Unavailable as failure:
        if failure.code != "not_connected":  # one is still stopping
            return block(refused("Kumi didn't read any folder", _reason(failure)))
        try:  # none runs: the tool starts one, with no DAW needed, once Kumi is set up (a read of its record)
            host.install(host.data_dir())
        except runtime.Unavailable as unset:
            return block(refused("Kumi didn't read any folder", _reason(unset)))
        live = None

    def scanned() -> list[str] | dict:
        """The folders a scan reads, or the block when the runtime can't say: a running runtime's own, then the
        settings' it lacks, which the tool hands it before the scan (`tools.hand_over_folders`); with none running,
        the settings', which the runtime the tool starts is given (its --library-root). A folder the runtime has is
        shown as the settings spell it, when they have it too: the runtime's own spelling is the disk's."""
        kept = host.library_folders()
        if live is None:
            return kept
        try:
            status = _ask(host, "library", {"action": "status"}, kwargs)
        except (runtime.Unavailable, runtime.Refused) as failure:
            return block(refused("Kumi didn't scan anything", _reason(failure)))
        spelled = {snapshots.same_place(folder): folder for folder in kept}
        running = [spelled.get(snapshots.same_place(folder), folder)
                   for folder in status.get("folders") or [] if isinstance(folder, str) and folder]
        return [*running, *(folder for folder in kept if folder not in running)]

    if action == "add_folder":
        folder, refusal = snapshots.library_folder(args.get("folder"), hermes_home=host.hermes_home())
        if refusal is not None:
            return block(refused("Kumi didn't add the folder", refusal))
        if args.get("scan") is not True:
            return approve(["Kumi: read the audio files in this folder and its subfolders, to build your sound map:",
                            *_path_lines(folder), AUDIO_ONLY], f"kumi:folder_read:{folder}")
        folders = scanned()  # one approval for the folder and its first scan, which says both
        if isinstance(folders, dict):
            return folders
        place = snapshots.same_place(folder)
        others = [line for other in folders if snapshots.same_place(other) != place for line in _path_lines(other)]
        return approve(["Kumi: read the audio files in this folder and its subfolders, and scan it now for your sound "
                        "map:", *_path_lines(folder),
                        *(["The scan reads your other sample folders again too:", *others] if others else []),
                        AUDIO_ONLY], f"kumi:folder_read:scan:{folder}")
    folders = scanned()
    if isinstance(folders, dict):
        return folders
    if not folders:
        return block("Kumi didn't scan anything: it has no approved sample folders yet. Add one first.")
    return approve(["Kumi: scan these sample folders and update your sound map, in the background:",
                    *(line for folder in folders for line in _path_lines(folder)), AUDIO_ONLY], "kumi:folder_read:scan")


def _path_lines(path: str) -> list[str]:
    """A folder's path as approval lines, "• " first: whole, since the path is what's approved, so one too long for a
    line goes on over the next ones, each ending after a separator where one falls in its second half."""
    lines, rest, lead = [], path, "• "
    while len(lead) + len(rest) > STEP_LINE:
        room = STEP_LINE - len(lead)
        separator = max(rest.rfind("/", 0, room), rest.rfind("\\", 0, room))
        cut = separator + 1 if separator > room // 2 else room
        lines.append(lead + rest[:cut])
        rest, lead = rest[cut:], ""
    return [*lines, lead + rest]


# Item 7 (the owner's decision): a scan indexes audio files only, and the approvals say so.
AUDIO_ONLY = "Kumi reads only the audio files there, and never changes or moves your files."


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
    daw = args.get("daw") or host.daw()
    asked = setup.approval(daw, action=action, data_dir=host.data_dir())
    if asked is None:
        why = setup.dead_end(daw, data_dir=host.data_dir()) or "run kumi_setup plan first."
        return block(refused("Kumi didn't set anything up", why)), {}
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


def _generate(host: Any, args: dict, kwargs: dict) -> tuple[dict | None, dict]:
    """Paid generation (CC-13). A sound effect's quote is a read: an estimate costs nothing. Sending an audio file to
    Mirelo is asked first, in the words of the runtime's own description of the file it resolved (13-R2), keyed by that
    file's identity. A create is asked from the stored quote alone, never the model's words (13-R1), keyed by its
    quote_id; its approval binds the quote's digest and estimate, which the handler checks again before it reserves.
    Where nobody can answer, both are refused (decide, 13-R6).

    Its worst case stays within 15 s of Hermes's 30 s hook timeout (a timed-out hook blocks every tool for a minute):
    each ledger call waits at most generate.LEDGER_WAIT (2 s) for its lock (amendment B-c). A create makes four (open,
    configure, the quote, the totals): 8 s. An upload makes two, then the runtime's preview, at most PREVIEW_TIMEOUT
    (10 s): 14 s."""
    refusal = generate.refusal(args)
    if refusal is not None:
        return block(refused("Kumi didn't run", refusal)), {}
    if args["action"] == "quote":
        return (None, {}) if args["kind"] == "sfx" else _upload(host, args, kwargs)
    return _paid(host, args)


def _upload(host: Any, args: dict, kwargs: dict) -> tuple[dict | None, dict]:
    """Sending an audio file to Mirelo (13-R2, amendment A). Where nobody can answer, it's refused before the runtime
    looks at the file. The ledger is opened first, so the producer's current policy is in it before the runtime's
    preview, a bounded local read (no network, copy or DSP); the approval names the file as that preview describes it,
    under a key naming its content (A-a), in this chat (A-c)."""
    if host.unattended():
        return block(UNATTENDED), {}
    not_ready = generate.ready(host, "audio_to_midi", host.mirelo_key())
    if not_ready is not None:
        return block(refused("Kumi didn't send anything", not_ready['error']['reason'])), {}
    try:
        host.ledger()
    except generate.LedgerUnavailable as failure:
        return block(refused("Kumi didn't send anything", failure.reason)), {}
    try:
        preview = _ask(host, "generate", {**args, "preview": True}, kwargs)
    except (runtime.Unavailable, runtime.Refused) as failure:
        return block(refused("Kumi didn't send anything", _reason(failure))), {}
    upload = preview.get("upload") if isinstance(preview.get("upload"), dict) else {}
    name, size, identity = upload.get("file"), upload.get("bytes"), upload.get("identity_sha256")
    if (preview.get("state") != "preview" or not isinstance(name, str) or not name.strip() or isinstance(size, bool)
            or not isinstance(size, int) or size < 0 or not isinstance(identity, str)
            or not _SHA256.fullmatch(identity)):
        return block("Kumi didn't send anything: its runtime couldn't describe the file."), {}
    decision = approve([f"Kumi: send {quoted(name)} ({_size(size)}) to Mirelo, to turn it into MIDI notes",
                        "• Mirelo quotes it in credits first; nothing is paid until that quote is approved too."],
                       f"kumi:upload:{identity}")
    return decision, {}


# A-a: a file's identity is its content's SHA-256, as lowercase hex.
_SHA256 = re.compile("[0-9a-f]{64}")


def _size(size: int) -> str:
    """A file's size as people read it: bytes under a kilobyte, kilobytes under a megabyte, else megabytes."""
    if size < 1_000:
        return f"{size} bytes"
    if size < 1_000_000:
        return f"{size / 1_000:.0f} KB"
    return f"{size / 1_000_000:.1f} MB"


def _paid(host: Any, args: dict) -> tuple[dict | None, dict]:
    """A create (13-R1), refused before anyone is asked, and before the ledger is opened where nobody can answer: a
    used or expired quote, a kind switched off or no key, a ledger that doesn't add up, a cap of 0 (which refuses
    every paid generation, a zero estimate's too), a freeze, or an estimate over what's left."""
    if host.unattended():
        return block(UNATTENDED), {}
    quote_id = args["quote_id"]
    try:
        ledger = host.ledger()
        record = ledger.quote(quote_id)
        totals = ledger.snapshot()
    except Exception as failure:
        said = getattr(failure, "message", None) or getattr(failure, "reason", None)
        why = said if isinstance(said, str) and said else "its credit ledger refused this quote."
        return block(refused("Kumi didn't create anything", why)), {}
    if record.get("consumed"):
        return block("Kumi didn't create anything: that quote was already used. Quote again."), {}
    if generate.expired(record):
        return block("Kumi didn't create anything: that quote expired. Quote again."), {}
    not_ready = generate.ready(host, record.get("kind"), host.mirelo_key())
    if not_ready is not None:
        return block(refused("Kumi didn't create anything", not_ready['error']['reason'])), {}
    estimate = record.get("estimate") if isinstance(record.get("estimate"), dict) else {}
    most, left, cap = estimate.get("max_credits"), totals.get("left"), totals.get("cap")
    if not all(isinstance(value, int) and not isinstance(value, bool) and value >= 0 for value in (most, left, cap)):
        return block("Kumi didn't create anything: its quote or credit ledger doesn't add up."), {}
    if cap == 0:
        return block("Kumi didn't create anything: the producer's credit cap is 0, which refuses every paid "
                     "generation. They set one in Kumi's settings in Hermes."), {}
    if totals.get("frozen"):
        return block("Kumi didn't create anything: new paid generations are paused, since a charge came in above its "
                     "estimate. The producer checks Mirelo, then runs hermes kumi credits unfreeze."), {}
    if most > left:
        return block(f"Kumi didn't create anything: its estimate ({most} credits) is more than the {left} left under "
                     "the producer's credit cap."), {}
    sfx = record.get("kind") == "sfx"
    seconds = record.get("seconds")
    lines = [f"Kumi: a paid Mirelo {'sound effect' if sfx else 'audio-to-MIDI job'}, from quote {quote_id}",
             f"• Prompt: {quoted(record.get('prompt'))}" if sfx
             else f"• From {quoted(record.get('file'))}, sent to Mirelo",
             *([f"• {seconds:g} seconds"] if isinstance(seconds, (int, float)) and not isinstance(seconds, bool)
               else []),
             f"• Reserves up to {most} credits now (an estimate, not a price); {left - most} of the {cap} left after"]
    bound = {"quote_id": quote_id, "request_sha256": record.get("request_sha256"), "max_credits": most}
    return approve(lines, f"kumi:paid:{quote_id}"), bound


def quoted(text: Any, limit: int = LINE) -> str:
    """Untrusted words in an approval, as one quoted line: every mark that reads as a quote made plain (#1014's
    `_QUOTE_MARKS`), so none can seem to close it early."""
    return f"\"{_QUOTE_MARKS.sub(chr(39), one_line(text if isinstance(text, str) else '', limit))}\""


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


# CC-19: scripts. What `_script` hands `pre_tool_call` to tell Desktop's pane, once the producer is really asked; the
# calls a producer denied, so the pane's `script.settled` says so (bounded: a denial is told once, soon after).
_SCRIPT_EVENT = "_script_event"
_DENIED: collections.deque[str] = collections.deque(maxlen=256)
SCRIPT_SUBAGENT = ("Kumi runs scripts only in the producer's own chat, never in a delegated subagent, so this didn't "
                   "run.")
SCRIPT_MESSAGING = ("Kumi doesn't run scripts from a messaging chat unless the producer turns on \"Run scripts from "
                    "messaging chats\" in Kumi's settings, so this didn't run.")
SCRIPT_UNREVIEWED = ("Kumi asks before each script in this chat ({why}), and Hermes's approvals are off here (YOLO or "
                     "approvals.mode: off): nobody would review it, so it didn't run. Turn approvals back on to run it.")
SCRIPT_UNKNOWN_APPROVALS = ("Kumi asks before each script in this chat ({why}), and Kumi can't tell whether Hermes's "
                            "approvals are on here, so it didn't run.")
SCRIPT_TOO_LONG = ("Kumi didn't run the script: its approval can't be shown whole here, and Kumi asks only when all of "
                   "it is in view. Ask for it in Hermes Desktop, which shows it whole.")
# The approval's richest form first, then smaller ones, until one shows whole on the producer's surface: the purpose's
# length, how many of the code's first lines, how wide, where the code is (its full path, which Desktop and the
# terminal can open; a short name; or nothing, in a messaging app), and the facts' length.
_SCRIPT_FORMS = ((200, 12, 100, "path", "long"), (120, 6, 80, "path", "long"), (60, 4, 60, "path", "long"),
                 (60, 2, 60, "path", "long"), (48, 1, 60, "path", "long"), (48, 0, 60, "path", "long"),
                 (40, 0, 60, "name", "short"), (40, 0, 60, None, "short"))


def _script(host: Any, args: dict, kwargs: dict) -> tuple[dict, dict]:
    """CC-19: a script runs in a chat with the producer in it, by the same rules as a device: unasked by default while
    the chat has read nothing from outside, else asked under `kumi:script` (this chat's, so "Allow this session" covers
    its next scripts); once it has read outside content, always asked, under the code's SHA-256, with why, and never
    while nobody reviews it (YOLO or approvals off). The runtime checks the code first (the size cap, hidden
    characters, the denylist), so refused code never reaches the producer."""
    language, code, purpose = args.get("language"), args.get("code"), args.get("purpose")
    if language not in scripts.LANGUAGE_NAMES or not isinstance(code, str) or not isinstance(purpose, str):
        return block("Kumi didn't run the script: it needs a language (python or lua), its purpose and its code."), {}
    session = kwargs.get("session_id")
    trust = host.trust()
    state, why = trust.state(session)
    if state == scripts.SUBAGENT:
        return block(SCRIPT_SUBAGENT), {}
    surface = host.session_surface()
    messaging = surface not in scripts.LOCAL_SURFACES
    if messaging and not host.scripts_in_messaging():
        return block(SCRIPT_MESSAGING), {}
    if messaging and state == scripts.CLEAN:
        state, why = scripts.TAINTED, scripts.MESSAGING
    asked = {key: args[key] for key in ("language", "purpose", "code", "expected_project_key") if key in args}
    try:
        answer = _ask(host, "script", {**asked, "preview": True}, kwargs)
    except (runtime.Unavailable, runtime.Refused) as failure:
        return block(refused("Kumi didn't run the script", _reason(failure))), {}
    if answer.get("state") != "preview":
        return block(refused("Kumi didn't run the script", answer.get('reason') or 'its runtime refused it.')), {}
    digest = scripts.sha256(code)
    if answer.get("sha256") != digest:
        return block("Kumi didn't run the script: its runtime read different code than this."), {}
    deadline = answer.get("deadline_seconds")
    if isinstance(deadline, bool) or not isinstance(deadline, (int, float)) or not 0 < deadline <= 60:
        return block("Kumi didn't run the script: its runtime didn't say when it would stop one that runs too "
                     "long."), {}
    tainted = state != scripts.CLEAN
    if tainted:
        bypassed = tools.approvals_bypassed(host)
        if bypassed is not False:
            return block((SCRIPT_UNREVIEWED if bypassed else SCRIPT_UNKNOWN_APPROVALS).format(
                why=scripts.refused_because(why))), {}
    try:
        path = scripts.save(Path(host.data_dir()).joinpath(*scripts.CODE_FOLDER), code, language)
    except OSError:
        log.exception("Kumi couldn't keep the script's code")
        return block("Kumi didn't run the script: it couldn't keep a copy of the code to show you."), {}
    daw = setup.DAWS.get(host.daw(), "the DAW")
    facts = {"daw": daw, "language": language, "purpose": purpose, "why": why if tainted else "", "digest": digest,
             "lines": scripts.lines_of(code), "bytes": len(code.encode("utf-8")), "path": _home_short(path),
             "deadline": f"{deadline:g}"}
    message = _script_message(code, facts, surface, chat_line=not tainted)
    if message is None and asks_first(host, "kumi_script", tainted):  # unasked, there's no window to fit
        return block(SCRIPT_TOO_LONG), {}
    decision = {"action": "approve", "message": message or "",
                "rule_key": f"kumi:script:{digest}" if tainted else "kumi:script"}
    if tainted:
        decision[_ALWAYS_ASKED] = True  # asked by provenance (CC-19), whatever the setting
    decision[_SCRIPT_EVENT] = {"profile": host.profile(), "session_id": session,
                               "tool_call_id": kwargs.get("tool_call_id"), "language": language, "daw": daw,
                               "purpose": one_line(purpose, 200), "warning": scripts.WARNING, "sha256": digest,
                               "lines": facts["lines"], "bytes": facts["bytes"], "tainted": tainted,
                               "at": int(time.time() * 1000)}
    return decision, {}


def _script_message(code: str, facts: dict, surface: str, *, chat_line: bool) -> str | None:
    """The script's approval in the richest form that shows whole on `surface` (with the chat line Kumi's chat
    scoping adds for a clean chat), or None when even the smallest doesn't."""
    code_lines = code[:-1].split("\n") if code.endswith("\n") else code.split("\n")
    for purpose_limit, head, width, where, facts_form in _SCRIPT_FORMS:
        lines = [f"Kumi: run Hermes's {scripts.LANGUAGE_NAMES[facts['language']]} script in {facts['daw']}",
                 f"Hermes says: {quoted(facts['purpose'], purpose_limit)}", scripts.WARNING]
        if facts["why"]:
            lines.append(scripts.asked_because(one_line(facts["why"], 60)))
        count = f"{facts['lines']} line{'s' if facts['lines'] != 1 else ''}"
        stop = f"stopped after {facts['deadline']} s"
        lines.append(f"{count} · {facts['bytes']:,} bytes · SHA-256 {facts['digest'][:16]}… · {stop}"
                     if facts_form == "long" else f"{count} · SHA-256 {facts['digest'][:8]}… · {stop}")
        shown = code_lines[:head]
        lines += [f"  {scripts.code_line(_visible(line), width)}" for line in shown]
        if head and len(code_lines) > len(shown):
            lines.append(f"  … {len(code_lines) - len(shown)} more")
        if where == "path":
            lines.append(f"Full code: {facts['path']}")
        elif where == "name":
            lines.append(f"Code: scripts/{facts['digest'][:12]}… in Kumi's data")
        message = "\n".join(lines)
        if shows_whole(f"{message}\n{CHAT_ONLY}" if chat_line else message, surface):
            return message
    return None


def _visible(text: str) -> str:
    """A line of code as the approval may show it: anything that could hide (control and format characters, which
    the runtime refuses in a script anyway) drawn as U+XXXX, so nothing shown differs from what runs."""
    return "".join(char if char == "\t" or (unicodedata.category(char) not in ("Cc", "Cf", "Zl", "Zp"))
                   else f"<U+{ord(char):04X}>" for char in text)


def _home_short(path: Path) -> str:
    """`path` as a terminal opens it, with `~` for the home folder (a row fewer in the terminal's panel)."""
    try:
        return f"~/{path.relative_to(Path.home()).as_posix()}"
    except ValueError:
        return str(path)


def _script_settled(host: Any, tool: str, result: Any, *, session_id: Any, tool_call_id: Any, args: Any) -> None:
    """CC-19, for Desktop's pane: a script's call ended, as `ran`, `failed`, `unsure`, `cancelled` (the producer's
    Stop), `refused` (by Kumi or its runtime) or `denied` (by the producer); a device's (CC-22) as `built`, or one of
    those. Its `sha256` is the code's, or the device's canonical spec's. A failed broadcast fails nothing."""
    data: Any = None
    if isinstance(result, str):
        try:
            data = json.loads(result)
        except ValueError:
            data = None
    state = data.get("state") if isinstance(data, dict) else None
    if state == "unsure" and data.get("code") in ("stopped", "interrupted"):
        outcome = "cancelled"
    elif state in ("ran", "failed", "unsure", "refused"):
        outcome = state
    elif tool == "kumi_device" and isinstance(data, dict) and "error" not in data and isinstance(data.get("item"), str):
        outcome = "built"
    else:
        outcome = "denied" if tool_call_id in _DENIED else "refused"
    if tool == "kumi_device":
        spec = args.get("spec") if isinstance(args, dict) else None
        digest = scripts.sha256(devices.canonical(spec)) if devices.problem(spec) is None else None
    else:
        code = args.get("code") if isinstance(args, dict) else None
        digest = scripts.sha256(code) if isinstance(code, str) else None
    _broadcast("script.settled", {"profile": host.profile(), "session_id": session_id, "tool_call_id": tool_call_id,
                                  "sha256": digest, "outcome": outcome})


# CC-22: Max for Live devices, asked as CC-19's scripts are, by the chat's provenance, under the spec's own key.
DEVICE = "Kumi didn't build the device"  # a refusal's head: refused() joins its reason
DEVICE_SUBAGENT = ("Kumi builds devices only in the producer's own chat, never in a delegated subagent, so this didn't "
                   "run.")
DEVICE_MESSAGING = ("Kumi doesn't build devices from a messaging chat unless the producer turns on \"Run scripts from "
                    "messaging chats\" in Kumi's settings, so this didn't run.")
DEVICE_UNREVIEWED = ("Kumi asks before each device in this chat ({why}), and Hermes's approvals are off here (YOLO or "
                     "approvals.mode: off): nobody would review it, so it wasn't built. Turn approvals back on to build "
                     "it.")
DEVICE_UNKNOWN_APPROVALS = ("Kumi asks before each device in this chat ({why}), and Kumi can't tell whether Hermes's "
                            "approvals are on here, so it wasn't built.")
DEVICE_ONLY_LIVE = refused(DEVICE, "Kumi builds Max for Live devices only in Ableton Live.")
# The approval's richest form first, then smaller ones, until one shows whole on the producer's surface: the purpose's
# length, how many of the code's first lines and how wide, the controls (None: one line each, with its default; a
# number: one summary line of at most that length), and the facts' length. The warning and the file are in every one:
# when even the smallest doesn't show whole, it isn't asked (TOO_LONG).
_DEVICE_FORMS = ((400, 12, 100, None, "long"), (200, 6, 80, None, "long"), (120, 4, 60, 120, "long"),
                 (60, 2, 60, 120, "long"), (48, 2, 60, 60, "short"), (48, 1, 60, 60, "short"),
                 (40, 0, 60, 60, "short"))


def _device(host: Any, args: dict, kwargs: dict) -> tuple[dict, dict]:
    """CC-22: a device is built only as the producer approved it, in a chat with the producer in it, by CC-19's
    provenance rules. Its key is the canonical spec's SHA-256 even in a clean chat, since the device outlives the chat.
    Serve previews the spec first (pure): one it refuses never reaches the producer, and a preview that hashed other
    bytes than these is refused."""
    spec = args.get("spec")
    problem = devices.problem(spec)
    if problem is not None:
        return block(refused(DEVICE, problem)), {}
    if host.daw() != "ableton":
        return block(DEVICE_ONLY_LIVE), {}
    session = kwargs.get("session_id")
    state, why = host.trust().state(session)
    if state == scripts.SUBAGENT:
        return block(DEVICE_SUBAGENT), {}
    surface = host.session_surface()
    messaging = surface not in scripts.LOCAL_SURFACES
    if messaging and not host.scripts_in_messaging():
        return block(DEVICE_MESSAGING), {}
    if messaging and state == scripts.CLEAN:
        state, why = scripts.TAINTED, scripts.MESSAGING
    spec_json = devices.canonical(spec)
    digest = scripts.sha256(spec_json)
    try:
        answer = _ask(host, "device", {"action": "preview", "spec_json": spec_json}, kwargs)
    except runtime.Refused as refusal:
        return block(refused(DEVICE, _with_findings(refusal.reason, refusal.findings))), {}
    except runtime.Unavailable as failure:
        return block(refused(DEVICE, _reason(failure))), {}
    if answer.get("state") != "preview":
        return block(refused(DEVICE, _with_findings(answer.get("reason") or "its runtime refused it.",
                                                    answer.get("findings")))), {}
    if answer.get("sha256") != digest:
        return block(refused(DEVICE, "its runtime read a different spec than this one.")), {}
    file = answer.get("file")
    if not (isinstance(file, str) and file.startswith(devices.FOLDER) and file.endswith(".amxd")
            and "/" not in file[len(devices.FOLDER):] and file.isprintable()):
        return block(refused(DEVICE, "its runtime didn't name a new file in the User Library's Kumi folder.")), {}
    tainted = state != scripts.CLEAN
    if tainted:
        bypassed = tools.approvals_bypassed(host)
        if bypassed is not False:
            return block((DEVICE_UNREVIEWED if bypassed else DEVICE_UNKNOWN_APPROVALS).format(
                why=scripts.refused_because(why))), {}
    try:
        scripts.save(Path(host.data_dir()).joinpath(*scripts.CODE_FOLDER), spec_json, devices.LANGUAGE)
    except OSError:
        log.exception("Kumi couldn't keep the device's spec")
        return block(refused(DEVICE, "it couldn't keep a copy of the spec to show you.")), {}
    lines = answer.get("lines") if isinstance(answer.get("lines"), int) else scripts.lines_of(spec["code"])
    facts = {"about": spec["about"], "why": why if tainted else "", "digest": digest, "lines": lines,
             "bytes": len(spec["code"].encode("utf-8")), "file": file}
    message = _device_message(spec, facts, surface)
    if message is None and asks_first(host, "kumi_device", tainted):  # unasked, there's no window to fit
        return block(TOO_LONG), {}
    decision = {"action": "approve", "message": message or "", "rule_key": f"kumi:device:{digest}"}
    if tainted:
        decision[_ALWAYS_ASKED] = True  # asked by provenance, as a script is (CC-19), whatever the setting
    decision[_SCRIPT_EVENT] = {"profile": host.profile(), "session_id": session,
                               "tool_call_id": kwargs.get("tool_call_id"), "language": devices.LANGUAGE,
                               "daw": setup.DAWS.get("ableton", "Live"), "name": answer.get("name"), "file": file,
                               "purpose": one_line(spec["about"], 200), "warning": devices.WARNING, "sha256": digest,
                               "lines": lines, "bytes": len(spec_json.encode("utf-8")), "tainted": tainted,
                               "at": int(time.time() * 1000)}
    return decision, {"sha256": digest}


def _with_findings(reason: Any, findings: Any) -> str:
    """A refusal's reason with the builder's findings, one per line, so the model can fix the spec and ask again."""
    found = [one_line(finding, 300) for finding in findings if isinstance(finding, str)] if isinstance(
        findings, list) else []
    return "\n".join([one_line(reason, 300), *(f"• {finding}" for finding in found)])


def _device_message(spec: dict, facts: dict, surface: str) -> str | None:
    """The device's approval in the richest form that shows whole on `surface`, or None when even the smallest
    doesn't: the warning and the file are never dropped."""
    code = spec["code"]
    code_lines = code[:-1].split("\n") if code.endswith("\n") else code.split("\n")
    for about_limit, head, width, controls_limit, facts_form in _DEVICE_FORMS:
        lines = ([f"Kumi: build Hermes's Max for Live audio effect, a new file: {facts['file']}"]
                 if facts_form == "long" else [f"Kumi: build {facts['file']}"])
        lines += [f"Hermes says: {quoted(facts['about'], about_limit)}", devices.WARNING]
        if facts["why"]:
            lines.append(scripts.asked_because(one_line(facts["why"], 60), "device"))
        count = f"{facts['lines']} line{'s' if facts['lines'] != 1 else ''} of GenExpr"
        lines.append(f"{count} · {facts['bytes']:,} bytes · SHA-256 {facts['digest'][:16]}…" if facts_form == "long"
                     else f"{facts['lines']} line{'s' if facts['lines'] != 1 else ''} · SHA-256 {facts['digest'][:8]}…")
        if controls_limit is None:
            lines.append("Controls:" if spec["controls"] else devices.summary([], 0))
            lines += [f"  • {devices.control_line(control)}" for control in spec["controls"]]
        else:
            lines.append(devices.summary(spec["controls"], controls_limit))
        shown = code_lines[:head]
        lines += [f"  {scripts.code_line(_visible(line), width)}" for line in shown]
        if head and len(code_lines) > len(shown):
            lines.append(f"  … {len(code_lines) - len(shown)} more")
        message = "\n".join(lines)
        if shows_whole(message, surface):
            return message
    return None


# CC-26: a verified plug-in preset into Live's User Library, asked in the words of serve's preview (26-w2), under its
# file's digest (26-w3). No code class: it's a verified writer's output (26c). The file stays after an undo (26e).
PRESET = "Kumi didn't put the preset in your User Library"  # a refusal's head: refused() joins its reason
PRESET_ONLY_LIVE = refused(PRESET, "Kumi puts plug-in presets only into Ableton Live's User Library.")
# The owner's call for the hackathon: presets are Live's only for now, so a build elsewhere would make nothing it
# could place.
PRESET_BUILD_ONLY_LIVE = refused("Kumi didn't build the preset",
                                 "Kumi builds plug-in presets only for Ableton Live for now.")
PRESET_WARNING = "Writes a new preset file into your User Library (Kumi folder). It stays there after an undo."
PRESET_VALUES = "Values as the preset file stores them; the plug-in may show them differently."
PRESET_UNEDITED = "• No settings changed: Kumi's initialized template as it is."
PRESET_STAYS = "The file stays after an undo."  # the short form's warning: its first line says what's written
PRESET_UNSHOWN = refused(PRESET, "its runtime's preview has something Kumi can't show as it is, so it isn't asked.")
PRESET_TOO_LONG = refused(PRESET, "its approval doesn't fit this window with every setting in view, and Kumi asks "
                                  "only with all of them shown. Build the preset with fewer changes, or ask in "
                                  "Hermes Desktop.")
PRESET_FOLDERS = ("User Library/Kumi/Instruments/", "User Library/Kumi/Audio Effects/")
MOST_PRESET_SETTINGS = 512  # serve's cap (kumi_library's preset::MAX_EDITS): the writer refuses more, never cuts them
PRESET_PRODUCTS = {"serum-2": "Serum 2", "fabfilter-pro-q-4": "Pro-Q 4", "ozone-12": "Ozone 12"}  # catalog ids


def _preset(host: Any, args: dict, kwargs: dict) -> tuple[dict, dict]:
    """CC-26: a preset kumi_preset_build made goes into the User Library only as the producer approved it, reading
    serve's preview: the plug-in and its version, every setting as the file stores it, and the file. The approval binds
    the preview's digest and file (26-r1), which the handler sends, so serve writes exactly that or nothing."""
    artifact = args.get("artifact_id")
    if not isinstance(artifact, str) or not artifact.strip():
        return block(refused(PRESET, "it needs the artifact_id kumi_preset_build gave.")), {}
    if host.daw() != "ableton":
        return block(PRESET_ONLY_LIVE), {}
    try:
        answer = _ask(host, "preset", {"action": "preview", "artifact_id": artifact}, kwargs)
    except runtime.Refused as refusal:
        return block(refused(PRESET, _with_findings(refusal.reason, refusal.findings))), {}
    except runtime.Unavailable as failure:
        return block(refused(PRESET, _reason(failure))), {}
    if answer.get("state") != "preview":
        return block(refused(PRESET, _with_findings(answer.get("reason") or "its runtime refused it.",
                                                    answer.get("findings")))), {}
    facts = _preset_facts(answer)
    if facts is None:
        return block(PRESET_UNSHOWN), {}
    message = _preset_message(facts, host.session_surface())
    if message is None and asks_first(host, "kumi_preset"):  # unasked, there's no window to fit
        return block(PRESET_TOO_LONG), {}
    decision = {"action": "approve", "message": message or "", "rule_key": f"kumi:preset:{answer['digest']}"}
    return decision, {"artifact_id": artifact, "digest": answer["digest"], "file": answer["file"]}


def _preset_facts(answer: dict) -> dict | None:
    """What the approval shows of serve's preview, every value as it came (26-s1), or None when it can't be shown as
    it is (26-w4): a field missing or of another kind, a file outside the User Library's Kumi folders, more settings
    than the writer allows, a setting line longer than an approval line, or text with a character a person reading it
    couldn't see. Nothing is cut or cleaned: what's shown is what's written."""
    plugin, version, checked = answer.get("plugin"), answer.get("version"), answer.get("version_checked")
    digest, file, name = answer.get("digest"), answer.get("file"), answer.get("name")
    settings, note = answer.get("settings"), answer.get("values_note", PRESET_VALUES)
    texts = (plugin, version, file, name, note)
    if not (all(isinstance(text, str) and _as_is(text) for text in texts) and isinstance(checked, bool)
            and isinstance(digest, str) and re.fullmatch(r"[0-9a-f]{64}", digest)
            and isinstance(settings, list) and len(settings) <= MOST_PRESET_SETTINGS):
        return None
    folder = next((folder for folder in PRESET_FOLDERS if file.startswith(folder)), None)
    if folder is None or "/" in file[len(folder):] or not file.endswith(".vstpreset"):
        return None
    shown = [_preset_setting(setting) for setting in settings]
    if None in shown or any(len(line) + 2 > STEP_LINE for line in shown) or len(note) > STEP_LINE:
        return None
    return {"plugin": PRESET_PRODUCTS.get(plugin, plugin), "version": version, "checked": checked, "file": file,
            "settings": shown, "note": note}


def _preset_message(facts: dict, surface: str) -> str | None:
    """The preset's approval in the richest form that shows whole on `surface`, or None when even the short one
    doesn't. Both carry every setting, the whole file, the values note verbatim and that the file stays after an undo;
    the short one says the rest in fewer words."""
    plugin, version, file = facts["plugin"], facts["version"], facts["file"]
    settings = [f"• {line}" for line in facts["settings"]] or [PRESET_UNEDITED]
    long = [f"Kumi: put a {plugin} preset in your User Library", f"File: {file}",
            (f"{plugin} {version}, the version installed" if facts["checked"]
             else f"Verified with {plugin} {version} (Kumi couldn't read the installed version)"),
            *settings, facts["note"], PRESET_WARNING]
    short = [f"Kumi: write {file}", f"{plugin} {version}" if facts["checked"] else f"Verified with {plugin} {version}",
             *settings, facts["note"], PRESET_STAYS]
    return next((message for message in ("\n".join(long), "\n".join(short)) if shows_whole(message, surface)), None)


def _preset_setting(setting: Any) -> str | None:
    """One setting as its line, "Volume: 0.3 → 0.45, stored value, 0–1": the record's own name, values and unit, a
    number as given (never rounded or converted), the range only when the record has one. None when it can't be shown
    as it is."""
    if not isinstance(setting, dict):
        return None
    name, unit = setting.get("name"), setting.get("unit")
    before, after = _preset_value(setting.get("before")), _preset_value(setting.get("after"))
    labels = all(isinstance(text, str) and _as_is(text) for text in (name, unit))
    if not labels or before is None or after is None:
        return None
    line = f"{name}: {before} → {after}, {unit}"
    if "range" not in setting:
        return line
    bounds = setting["range"]  # W3: exactly two finite numbers
    if not (isinstance(bounds, list) and len(bounds) == 2 and all(_preset_number(bound) for bound in bounds)):
        return None
    return f"{line}, {_preset_value(bounds[0])}–{_preset_value(bounds[1])}"


def _preset_number(value: Any) -> bool:
    return isinstance(value, (int, float)) and not isinstance(value, bool) and math.isfinite(value)


def _preset_value(value: Any) -> str | None:
    """A stored value as the approval shows it, as W3's record holds it: a number exactly as it came (never rounded:
    Pro-Q 4 stores 0.12345679104328156 for a requested 0.123456789), true or false, a label (or "not stored in
    template"), or a list of labels (an Ozone module chain, Serum's effects); None for anything else."""
    if isinstance(value, bool):
        return "true" if value else "false"
    if _preset_number(value):
        return repr(value)
    if isinstance(value, str):
        return value if _as_is(value) else None
    if isinstance(value, list) and value and all(isinstance(item, str) and _as_is(item) for item in value):
        return "[" + ", ".join(value) + "]"
    return None


def _as_is(text: str) -> bool:
    """Whether `text` shows as it is in one line: not empty, and nothing `one_line` would take out (a line break, a
    control or invisible format character) or squeeze (doubled or edge spaces)."""
    return bool(text) and one_line(text, len(text) + 1) == text


def _broadcast(event: str, payload: dict) -> None:
    try:
        tools.broadcast(event, payload)
    except Exception:
        log.exception("Kumi couldn't tell Desktop about a script (%s)", event)
