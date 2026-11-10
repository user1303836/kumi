"""`hermes kumi doctor` and `/kumi doctor`: what Kumi needs in this Hermes profile, item by item, each with what to do.

Read only. Hermes's settings come through the host (`Host.hermes_config`, Hermes's documented config reader, and the
plugin's own settings); Kumi's state from its plugin data, found without creating it (`Host.data_path`): setup's
records, what the runtime last published, and its job records. It never opens Hermes's .env or auth.json, starts no
runtime, writes nothing and asks nothing. (Reading Hermes's settings is Hermes's own: on a cache miss it keeps a backup
of a config.yaml it hasn't seen, as every `hermes` command does.) Hermes Desktop keeps its plugin switches itself, so
they're the one item it can only describe. Nothing it prints names a folder on this computer: `/kumi doctor`'s answer
stays in a chat's transcript.
"""
from __future__ import annotations

from pathlib import Path
import re
from typing import Any

from . import listening, policy, schemas, setup, snapshots, tools

OK, FIX, CHECK, OFF = "ok", "fix", "check", "off"
_MARKS = {OK: "OK   ", FIX: "FIX  ", CHECK: "CHECK", OFF: "OFF  "}
_WIDTH = 400  # one item's words, as one line


def checks(host: tools.Host, **chat: Any) -> list[dict]:
    """Every item, in the order the report shows them. `chat` is a chat's call context (`/kumi doctor`): a chat's own
    /yolo is then read too, which a terminal can't see."""
    data = host.data_path()
    config = host.hermes_config()
    items = [_approvals(host, config, chat=bool(chat.get("session_id"))), _isolation(config), _hook(host),
             _listening(host, config), _hermes()]
    if data is None:
        missing = _item("Kumi's data", FIX, "Kumi can't find this profile's Hermes home, so it can't read its own data.",
                        "Check that Hermes runs with this profile, then try again.")
        return [*items, missing, _desktop()]
    state = data / "state"
    daw = host.daw()
    app = schemas.DAW_NAMES.get(daw, daw)
    report = setup.status(daw=daw, data_dir=data, plugin_dir=host.plugin_dir)
    shown = setup.for_model(report)  # setup's words naming no path on this computer
    status = snapshots.status(state)
    items += [_runtime(report, shown), _part(report, shown, status, daw, app), _connection(status, report, daw),
              _project(status), _library(host, state, live=not status.get("stale")), _audio(shown)]
    items += [_mirelo(host.mirelo_status()), _desktop()]
    return items


def report(items: list[dict]) -> str:
    """The items as the terminal and the chat show them, then what's left to do."""
    lines = ["Kumi doctor: what Kumi needs in this profile (read only: Kumi changed nothing)."]
    for item in items:
        lines.append(f"{_MARKS.get(item['state'], 'CHECK')} {_said(item['name'], 60)}: {_said(item['detail'])}")
        if item.get("fix"):
            lines.append(f"      To do: {_said(item['fix'])}")
    fix = sum(item["state"] == FIX for item in items)
    check = sum(item["state"] == CHECK for item in items)
    if not fix and not check:
        lines.append("Everything Kumi needs is in place.")
    else:
        lines.append(f"{fix} to fix, {check} to check.")
    return "\n".join(lines)


def needs_fixing(items: list[dict]) -> bool:
    return any(item["state"] == FIX for item in items)


def _item(name: str, state: str, detail: str, fix: str = "") -> dict:
    return {"name": name, "state": state, "detail": detail, "fix": fix}


def _said(text: Any, limit: int = _WIDTH) -> str:
    return policy.one_line(text, limit) if text is not None else ""


def _section(config: dict, name: str) -> dict:
    value = config.get(name)
    return value if isinstance(value, dict) else {}


# ---- Hermes's settings ----------------------------------------------------------------------------------------------

def _approvals(host: tools.Host, config: dict, *, chat: bool) -> dict:
    name = "Approvals"
    bypassed = tools.approvals_bypassed(host, chat=chat)
    if bypassed:
        return _item(name, FIX, tools.NO_APPROVALS,
                     "Set approvals.mode: manual in Hermes's config.yaml, run Hermes without --yolo, and turn /yolo off.")
    gaps = tools.approval_gaps(host)
    if gaps:
        return _item(name, FIX, " ".join(gaps),
                     "Set approvals.single_query_mode: deny and delegation.subagent_auto_approve: false in config.yaml.")
    if bypassed is None:
        return _item(name, CHECK, tools.UNKNOWN_APPROVALS, "Check that /yolo is off in this chat.")
    seen = "" if chat else " A terminal can't see a chat's /yolo: check that it's off where you use Kumi."
    # Hermes's default, approvals.mode: smart, puts a guardian model in front of terminal commands and code only: a
    # plugin's approval goes through Hermes's action gate, which never asks it (0.22.7's tools/approval.py calls
    # _human_decision(_ACTION_GATE, …) without smart). So Hermes still asks before each Kumi change; research-1's
    # first-hour baseline saw every change asked under smart.
    mode = _section(config, "approvals").get("mode", "manual")
    smart = isinstance(mode, str) and mode.strip().lower() == "smart"
    how = ("approvals.mode: smart, whose guardian reviews commands and code, never Kumi's changes" if smart
           else "approvals.mode: manual")
    if not host.ask_before_changes():
        return _item(name, OK, f"{tools.CHANGES_UNASKED}, through Hermes ({how}).{seen}")
    return _item(name, OK, f"Hermes asks before each Kumi change (Ask before each change: on; {how}).{seen}")


def _isolation(config: dict) -> dict:
    isolation = _section(config, "plugins").get("isolation", "in_process")
    if isinstance(isolation, str) and isolation.strip().lower() == "host":
        return _item("Plugin isolation", FIX,
                     "Hermes runs plugins in its plugin host (plugins.isolation: host): the hermes kumi commands aren't "
                     "there (/kumi is), and Kumi can't see a chat's /yolo.",
                     "Set plugins.isolation: in_process (Hermes's default) in config.yaml, then restart Hermes.")
    return _item("Plugin isolation", OK, "Kumi runs in Hermes's own process (plugins.isolation: in_process).")


def _hook(host: tools.Host) -> dict:
    timeout = host.hook_timeout()
    budget = policy.preview_timeout(timeout)
    if budget < policy.PREVIEW_TIMEOUT:
        return _item("Hook time", FIX, f"plugins.hook_callback_timeout is {timeout:g} s, so Kumi has only {budget:g} s "
                                       "to read the project and prepare a change's approval.",
                     "Set plugins.hook_callback_timeout to 30 (Hermes's default) or more in config.yaml.")
    setting = "no limit" if timeout is None or timeout <= 0 else f"{timeout:g} s"
    return _item("Hook time", OK, f"Kumi has {budget:g} s to prepare each approval (plugins.hook_callback_timeout: "
                                  f"{setting}).")


def _listening(host: tools.Host, config: dict) -> dict:
    name = "Listening model"
    if not host.listen_interpretation():
        return _item(name, OFF, "Interpret measurements with a model is off: Kumi measures audio itself, and no model "
                                "reads the measurements.")
    auxiliary = _section(config, "auxiliary")
    task = auxiliary.get(listening.AUX_TASK_KEY) if isinstance(auxiliary.get(listening.AUX_TASK_KEY), dict) else {}
    vision = auxiliary.get("vision") if isinstance(auxiliary.get("vision"), dict) else {}
    model = task.get("model") or vision.get("model")
    which = f"{_said(model, 80)}" if model else "the model Hermes's Vision slot uses"
    return _item(name, CHECK, f"Interpret measurements with a model is on, with {which}. Kumi can't tell from here "
                              "whether it takes images: if it doesn't, the reading fails and the measurements still "
                              "come back.",
                 "In Hermes's model settings, choose an image-capable model for Kumi's listening task.")


def _hermes() -> dict:
    return _item("Hermes", OK, "Hermes loaded Kumi, so it's 0.22.7 or later (what Kumi requires).")


# ---- Kumi's setup and runtime ---------------------------------------------------------------------------------------

_SET_UP = "Run hermes kumi setup in a terminal, or ask Hermes to set Kumi up."


def _runtime(report: dict, shown: dict) -> dict:
    name = "Kumi's runtime"
    state = report.get("state")
    if state in ("no_pin", "unsupported_platform"):
        return _item(name, FIX, shown.get("message") or "This copy of the plugin can't set Kumi up here.",
                     "Update the plugin (hermes plugins update kumi).")
    if state in ("needs_runtime", "outdated_runtime", "damaged_runtime"):
        return _item(name, FIX, shown.get("message") or "Kumi's runtime isn't set up.", _SET_UP)
    version = (report.get("runtime") or {}).get("version")
    return _item(name, OK, f"Kumi {_said(version, 40)}'s runtime is installed, the version this plugin pins.")


def _version(text: Any) -> tuple[int, ...] | None:
    """A dotted release number, or None for anything else (missing, `dev`, a pre-release): only those compare."""
    text = str(text or "")
    return tuple(int(part) for part in text.split(".")) if re.fullmatch(r"\d+(?:\.\d+)*", text) else None


def _part(report: dict, shown: dict, status: dict, daw: str, app: str) -> dict:
    """Kumi's part in the DAW. Setup's record names its sub-parts (Kumi Ears beside Live's Remote Script); the
    runtime's own reading of the part's version (`status.setup.component`, when it runs for this DAW) comes before the
    record's, which survives a runtime upgrade. Only two dotted versions that compare equal make a match. A part
    another installation put in (adopted) is that installation's to update: this profile's setup refuses it."""
    name = f"Kumi's part in {app}"
    if report.get("state") in ("no_pin", "unsupported_platform", "needs_runtime", "outdated_runtime",
                               "damaged_runtime"):
        return _item(name, OFF, "Set up Kumi's runtime first.")
    component = shown.get("daw_component") or {}
    existing = shown.get("existing_kumi") or {}
    if component.get("state") == "not_installed":
        return _item(name, FIX, f"Kumi's part in {app} isn't set up in this profile.", _SET_UP)
    adopted = component.get("state") == "unknown" or existing.get("adopted") is True
    if adopted and daw in ("reaper", "renoise"):  # the runtime refuses to connect through one (serve/connection.rs)
        return _item(name, FIX, f"Kumi for Hermes can't use {app} through an adopted installation yet, so connecting "
                                f"to {app} is refused.", f"Set {app} up for Hermes instead.")
    part = f"the part another Kumi installation put in {app}" if adopted else f"Kumi's part in {app}"
    update = (f"Update it with the Kumi that put it in: kumi --daw {daw} bridge for a standalone Kumi, or setup in "
              "that Kumi's own Hermes profile.") if adopted else _SET_UP
    parts = [entry for entry in component.get("parts") or [] if isinstance(entry, dict)]
    behind = [_said(entry.get("name"), 60) for entry in parts if entry.get("state") not in (None, "same")]
    if behind:
        return _item(name, FIX, f"{_cap(part)} ({_said(component.get('version'), 40)}) isn't complete: "
                                f"{', '.join(behind)}.", update)
    setup_now = status.get("setup") if isinstance(status.get("setup"), dict) else {}
    live = setup_now.get("component") if not status.get("stale") and status.get("daw") == daw else None
    if isinstance(live, dict) and live.get("installed"):
        have, bundled = _version(live.get("version")), _version(live.get("bundled"))
        said, ours = _said(live.get("version") or "unknown", 40), _said(live.get("bundled") or "unknown", 40)
        if have is None or bundled is None:
            return _item(name, CHECK, f"Kumi can't compare the version of {part} ({said}) with this Kumi's ({ours}).",
                         f"If Kumi says the part is too old: {update}")
        if have < bundled:
            return _item(name, FIX, f"{_cap(part)} is {said}, older than this Kumi's {ours}.", update)
        if have > bundled:
            return _item(name, CHECK, f"{_cap(part)} is {said}, newer than this Kumi's {ours}.",
                         "Update the Kumi plugin (hermes plugins update kumi).")
        return _item(name, OK, f"{_cap(part)} is {said}, the same as this Kumi's, as its runtime read it just now.")
    if adopted:
        return _item(name, OK, f"Kumi uses another Kumi installation's part in {app}: set up and updated there.")
    return _item(name, OK, f"Kumi's part in {app} is set up ({_said(component.get('name'), 60)} "
                           f"{_said(component.get('version'), 40)}), as setup last recorded it; Kumi's runtime checks "
                           "it against its own when it runs.")


def _cap(text: str) -> str:
    return text[:1].upper() + text[1:]


def _connection(status: dict, report: dict, daw: str) -> dict:
    """The connection the runtime last published, named by the DAW it runs for (a chat can connect another DAW
    without changing the setting)."""
    name = "Connection"
    setting = schemas.DAW_NAMES.get(daw, daw)
    running_for = status.get("daw") if status.get("daw") in schemas.DAW_NAMES else daw
    app = schemas.DAW_NAMES[running_for] if running_for in schemas.DAW_NAMES else setting
    if status.get("connected"):
        project = status.get("project") if isinstance(status.get("project"), dict) else {}
        connected = f"Connected to {app}: {_said(project.get('name') or 'a project', 80)}."
        if running_for != daw:
            return _item(name, CHECK, f"{connected} Kumi's DAW setting is {setting}.",
                         f"To make {app} Kumi's DAW, run hermes kumi setup --daw {running_for} --use.")
        return _item(name, OK, connected)
    if report.get("state") != "ready":
        return _item(name, OFF, f"Set Kumi up for {setting} first.")
    if report.get("next_step") == "choose_control_surface":
        return _item(name, FIX, "Live doesn't run Kumi's Remote Script yet.",
                     "In Live, choose AbletonMcpBridge as a Control Surface (Settings → Link, Tempo & MIDI).")
    running = "Kumi's runtime isn't running now (it runs while a chat uses Kumi)." if status.get("stale") else \
        f"Kumi's runtime is running, but not connected to {app}."
    return _item(name, CHECK, running, f"Open {app}, then ask Hermes to connect to it.")


def _project(status: dict) -> dict:
    name = "Project"
    app = schemas.DAW_NAMES.get(status.get("daw"), "the DAW")
    if not status.get("connected"):
        return _item(name, OFF, "Connect first: then the open project's key and epoch show here.")
    project = status.get("project") if isinstance(status.get("project"), dict) else {}
    if not project.get("key"):
        return _item(name, FIX, "The open project has no key yet, so Kumi can't keep its changes apart from another "
                                "project's.",
                     f"Save the project in {app} (an unsaved Live Set has no key), then ask Hermes to look at it.")
    return _item(name, OK, f"{_said(project.get('name') or 'The project', 80)} has a key and epoch "
                           f"{_said(project.get('epoch'), 40)}.")


def _library(host: tools.Host, state: Path, *, live: bool) -> dict:
    """The sound map: its approved folders, and its last scan as the runtime's job record says, taken with the
    runtime's liveness (a scan left "running" by a runtime that was stopped is marked interrupted only at its next
    start)."""
    name = "Sound map"
    folders = host.library_folders()
    if not folders:
        return _item(name, CHECK, "No sample folders are approved, so the sound map is empty.",
                     "Add one with `hermes kumi samples add <folder>`, or ask Hermes to, then ask Hermes to scan it.")
    scan = _last_scan(state)
    progress = scan.get("progress") if isinstance(scan.get("progress"), dict) else {}
    sounds = (snapshots.map_revision(state) or {}).get("sounds")
    count = len(sounds) if isinstance(sounds, dict) else 0
    approved = f"{len(folders)} approved folder{'s' if len(folders) != 1 else ''}"
    if scan.get("state") == "running" and not live:
        return _item(name, FIX, "The last scan stopped when Kumi's runtime did (its record still says running).",
                     "Ask Hermes to scan the folders again.")
    if scan.get("state") == "running":
        done, total = progress.get("done"), progress.get("total")
        so_far = f" ({_said(done, 12)} of {_said(total, 12)})" if done is not None and total is not None else ""
        return _item(name, CHECK, f"A scan of the {approved} is running{so_far}.", "Wait until it shows done.")
    if scan.get("state") in ("failed", "cancelled", "interrupted"):
        return _item(name, FIX, f"The last scan was {_said(scan['state'], 20)}; the map has {count} sounds.",
                     "Ask Hermes to scan the folders again.")
    if not count and scan.get("state") == "done":
        return _item(name, FIX, f"The last scan of the {approved} found no audio Kumi can read: it reads WAV and "
                                "AIFF, and more with an ffmpeg allowed at setup.",
                     "Approve a folder that holds WAV or AIFF files (or allow ffmpeg with hermes kumi setup for MP3 "
                     "and FLAC), then scan it.")
    if not count:
        return _item(name, FIX, f"The {approved} haven't been scanned yet.", "Ask Hermes to scan them.")
    return _item(name, OK, f"{count} sounds on the map, from the {approved}.")


def _last_scan(state: Path) -> dict:
    """The newest scan job's record, from the runtime's own job files (`jobs/<id>.json`)."""
    newest: dict = {}
    folder = state / "jobs"
    for path in folder.glob("*.json") if folder.is_dir() else ():
        record = snapshots.read(state, f"jobs/{path.stem}")
        if not isinstance(record, dict) or record.get("kind") != "scan":
            continue
        if not newest or _at(record) >= _at(newest):
            newest = record
    return newest


def _at(record: dict) -> float:
    at = record.get("at")
    return float(at) if isinstance(at, (int, float)) and not isinstance(at, bool) else 0.0


def _audio(shown: dict) -> dict:
    name = "Compressed audio"
    audio = shown.get("compressed_audio") or {}
    message = _said(str(audio.get("message") or "").removeprefix("Compressed audio (MP3, FLAC…): "))
    if audio.get("state") == "available":
        return _item(name, OK, message)
    if audio.get("state") in ("changed", "gone"):
        return _item(name, FIX, message, "Run hermes kumi setup to allow an ffmpeg again.")
    offer = "" if "hermes kumi setup" in message else \
        "Only if your sample folders hold MP3, FLAC or other compressed audio: install ffmpeg, then run hermes kumi setup."
    return _item(name, OFF, message, offer)


def _mirelo(ready: Any) -> dict:
    """Mirelo's readiness as the host reports it: the key held (never shown), the onboarding acknowledged, a task on,
    and a credit cap above 0."""
    name = "Mirelo"
    ready = ready if isinstance(ready, dict) else {}
    if not ready.get("key"):
        return _item(name, OFF, "No Mirelo key is set, so Kumi never contacts Mirelo.",
                     "Only if you use Mirelo: enter its key in Kumi's plugin settings.")
    missing = [words for flag, words in (("acknowledged", "acknowledge Mirelo's onboarding"),
                                         ("task", "switch on the task you'll use (sound effects, or audio to MIDI)"),
                                         ("cap", "set mirelo_credit_cap above 0"))
               if not ready.get(flag)]
    if missing:
        return _item(name, FIX, "Mirelo's key is set, but paid generations aren't ready.",
                     "In Kumi's plugin settings: " + "; ".join(missing) + ".")
    return _item(name, OK, "Mirelo is ready: key set, onboarding acknowledged, a task on, and a credit cap above 0.")


def _desktop() -> dict:
    return _item("Hermes Desktop", CHECK, "Kumi can't see Hermes Desktop's plugin switches from here.",
                 "In Hermes Desktop, turn on Kumi's Agent and Desktop switches under Capabilities → Plugins.")
