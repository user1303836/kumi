"""CC-22: Max for Live audio effects that Hermes writes in GenExpr, built into the User Library, then loaded and placed.

A device is code too: it runs in Live whenever its Set is open, so it's approved as CC-19's scripts are (`policy._device`,
in `scripts`' provenance), with three differences:
- the approval names the whole spec (its purpose, its controls with their ranges and defaults, and its code), and the
  file the build writes;
- its key is `kumi:device:<sha256 of the spec>`, unscoped even in a clean chat, since a device outlives the chat;
- serve, not the plugin, checks the spec (22b, 22-r1, 22-r2) and refuses it with the builder's findings.

One set of bytes, hashed once (the lead's canonical form): the plugin writes the spec as `canonical()` gives it, hashes
those UTF-8 bytes, keeps them as `state/scripts/<sha256>.device.json` and hands serve the same string as `spec_json`.
Serve hashes what it received and never re-encodes, so the hash serve answers, the approval's key and the saved file's
name are one SHA by construction (Python and Rust write some floats differently: `1e-07` against `1e-7`).
"""
from __future__ import annotations

import json
import math
from typing import Any

from . import scripts

# 22e, word for word: the approval, Desktop's pane (through `script.pending`) and the docs all say it this way.
WARNING = ("Writes a new Max for Live device into your User Library (Kumi folder); it stays in your Set and runs "
           "whenever the Set is open. Its output is held under −0.3 dBFS.")
# Where serve's build writes (22d), as its preview names the file.
FOLDER = "User Library/Kumi/Audio Effects/"
LANGUAGE = "device"  # in `scripts.LANGUAGES` and `script.pending`'s language
MOST_NAME, MOST_ABOUT, MOST_CONTROLS = 32, 400, 16
KEYS = ("name", "about", "controls", "code")
CONTROL_KEYS = ("name", "type", "min", "max", "default", "unit", "options")
TYPES = ("number", "integer", "choice", "switch")
# The standalone's units (devices/spec.rs's UNITS), none left out.
UNITS = ("ms", "s", "Hz", "dB", "%", "st", "note", "pan", "beats", "bpm", "x")


def canonical(spec: Any) -> str:
    """The spec's one JSON form (W2's): sorted keys, no spaces, characters as they are. Raises ValueError for a number
    JSON can't carry (NaN, ±Infinity)."""
    return json.dumps(spec, sort_keys=True, separators=(",", ":"), ensure_ascii=False, allow_nan=False)


def problem(spec: Any) -> str | None:
    """Why the plugin can't take `spec` as 22b's shape, or None. Only the shape and the caps: what the device does
    (ranges, defaults, GenExpr, loops, memory) is serve's to check, with its findings."""
    if not isinstance(spec, dict):
        return "spec is an object with name, about, controls and code."
    other = sorted(str(key) for key in spec if key not in KEYS)
    if other:
        return (f"spec has {', '.join(other)}, which a device built from Hermes doesn't take: only name, about, controls "
                "and code.")
    name, about, controls, code = (spec.get(key) for key in KEYS)
    if not isinstance(name, str) or not name.strip() or len(name) > MOST_NAME:
        return f"name is the device's name, 1 to {MOST_NAME} characters."
    if not isinstance(about, str) or not about.strip() or len(about) > MOST_ABOUT:
        return f"about says what the device does, 1 to {MOST_ABOUT} characters."
    if not isinstance(code, str) or not code.strip():
        return "code is the device's GenExpr."
    if scripts.lines_of(code) > scripts.MOST_LINES or len(code.encode("utf-8", "surrogatepass")) > scripts.MOST_BYTES:
        return (f"its code is over {scripts.MOST_LINES} lines or {scripts.MOST_BYTES // 1024} KB, so the producer "
                "couldn't review it whole: make it smaller.")
    if not isinstance(controls, list) or len(controls) > MOST_CONTROLS:
        return f"controls is a list of at most {MOST_CONTROLS}."
    for number, control in enumerate(controls, 1):
        if not isinstance(control, dict):
            return f"control {number} is an object."
        other = sorted(str(key) for key in control if key not in CONTROL_KEYS)
        if other:
            return f"control {number} has {', '.join(other)}: a control takes only {', '.join(CONTROL_KEYS)}."
        if not isinstance(control.get("name"), str) or not control["name"].strip():
            return f"control {number} needs a name."
        if control.get("type") not in TYPES:
            return f"control {number}'s type is one of {', '.join(TYPES)}."
    try:
        canonical(spec).encode("utf-8")
    except ValueError:  # NaN or Infinity, or text that isn't Unicode (UnicodeEncodeError is a ValueError)
        return "every number in it is finite and every text is Unicode."
    return None


def _number(value: Any) -> str:
    if isinstance(value, bool) or not isinstance(value, (int, float)):
        return "?"
    if isinstance(value, float) and value.is_integer() and math.fabs(value) < 1e15:
        return str(int(value))
    return str(value)


def _text(value: Any, limit: int = 32) -> str:
    """A name or an option from the spec as one line of the approval: the model's words, nothing that could hide."""
    words = " ".join(str(value).split()) if isinstance(value, (str, int, float)) else ""
    words = "".join(char for char in words if char.isprintable())
    return words if len(words) <= limit else words[:limit - 1] + "…"


def control_range(control: dict) -> str:
    """A control's name and range as the summary line has them: `Tone 200–8000 Hz`, `Mode Soft/Hard`, `Bypass on/off`."""
    name, kind = _text(control.get("name")), control.get("type")
    if kind == "choice":
        options = control.get("options") if isinstance(control.get("options"), list) else []
        return f"{name} {'/'.join(_text(option, 16) for option in options) or '(no options)'}"
    if kind == "switch":
        return f"{name} on/off"
    unit = f" {_text(control.get('unit'), 8)}" if control.get("unit") else ""
    return f"{name} {_number(control.get('min'))}–{_number(control.get('max'))}{unit}"


def control_line(control: dict) -> str:
    """A control's whole line for a surface that shows the full list: its range and its default."""
    default = control.get("default")
    if control.get("type") == "switch":
        shown = "on" if default is True else "off" if default is False else _text(default, 16) or "?"
    elif control.get("type") == "choice":
        shown = _text(default, 16) if default is not None else "?"
    else:
        unit = f" {_text(control.get('unit'), 8)}" if control.get("unit") else ""
        shown = f"{_number(default)}{unit}"
    return f"{control_range(control)}, default {shown}"


def summary(controls: list[dict], limit: int) -> str:
    """Every control in one line of at most `limit` characters, as compact approvals show them (W2): `6 controls:
    Drive 0–1, Tone 200–8000 Hz, …`. Desktop's pane has the full list."""
    if not controls:
        return "No controls of its own (Kumi adds Mix and Output)."
    line = f"{len(controls)} control{'s' if len(controls) != 1 else ''}: "
    shown = []
    for index, control in enumerate(controls):
        part = control_range(control)
        rest = len(controls) - index - 1
        tail = ", …" if rest else ""
        if len(line) + len(", ".join([*shown, part])) + len(tail) > limit:
            return f"{line}{', '.join(shown)}{', …' if shown else '…'}"
        shown.append(part)
    return line + ", ".join(shown)


def next_step(built: dict) -> str:
    """What the model does with a built device (22f): observe, then load and place it in one ordinary approved change."""
    step = ("The device is in the User Library, not yet in the Set. To use it, first kumi_observe for the track's ref "
            "and the project key and epoch, then call kumi_change with a load_device step "
            f"{{\"itemId\": {json.dumps(built.get('item'), ensure_ascii=False)}, \"trackRef\": …}} and a move_device "
            "step to place it (\"before the Saturator\"), in that one change: the producer approves it on its own, "
            "and Kumi can undo it. The file stays in the User Library after an undo.")
    if built.get("listed") is False:
        step += " Live's Browser hasn't listed it yet; the load waits for it."
    return step
