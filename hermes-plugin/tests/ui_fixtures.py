"""A Kumi state folder (contract C3) made from nothing, for the UI's tests and screenshots.

`write_state(plugin_data, library)` writes what `kumi serve` would publish while connected to a Live Set: owner,
status, FOCUS, HISTORY, a sound map with its samples, two measured files and a scan job. Everything is deterministic
but the timestamps. From a terminal, `python3 tests/ui_fixtures.py <plugin-data>/kumi <library> --live` keeps the
owner's heartbeat fresh (and the transport moving) until stopped, so the UI shows a connected runtime.
"""

from __future__ import annotations

import argparse
import json
import math
import os
import random
import struct
import sys
import time
import wave
from pathlib import Path

OWNER = {"profile": "default", "session_id": "20261105_101500_k7d2a1", "pid": 4242}
PROJECT = {"name": "Night Drive", "key": "p-6f1c9e", "epoch": 3}

# group: (name, colour, centre, spread, count, folder, names, kind)
GROUPS = [
    ("kick", [232, 93, 74], (0.18, 0.72), 0.06, 90, "Drums/Kicks", ["Boom Kick", "Tight Kick", "Sub Kick", "Punch Kick"], "one-shot"),
    ("snare", [245, 166, 35], (0.36, 0.34), 0.07, 110, "Drums/Snares", ["Dusty Snare", "Crack Snare", "Tape Snare", "Brush Snare"], "one-shot"),
    ("clap", [229, 196, 64], (0.48, 0.22), 0.05, 50, "Drums/Claps", ["Room Clap", "Snap Clap", "Stack Clap"], "one-shot"),
    ("hat", [80, 200, 180], (0.70, 0.30), 0.06, 120, "Drums/Hats", ["Closed Hat", "Open Hat", "Shaker Hat", "Tick Hat"], "one-shot"),
    ("perc", [126, 211, 33], (0.62, 0.55), 0.07, 80, "Drums/Perc", ["Wood Block", "Conga Hit", "Rim Shot", "Tom Hit"], "one-shot"),
    ("bass", [74, 144, 226], (0.24, 0.40), 0.05, 60, "Bass", ["Reese Bass", "Sub Bass", "808 Glide"], "one-shot"),
    ("pad", [155, 89, 182], (0.80, 0.74), 0.07, 70, "Pads", ["Warm Pad", "Air Pad", "Glass Pad"], "loop"),
    ("fx", [236, 112, 200], (0.50, 0.84), 0.08, 60, "FX", ["Riser", "Impact", "Downlifter", "Sweep"], "one-shot"),
]


def _now_ms() -> int:
    return int(time.time() * 1000)


def _envelope(seq: int, at: int) -> dict:
    return {"schema": 1, "seq": seq, "at": at, "owner": dict(OWNER)}


def _write_json(path: Path, data: dict) -> None:
    """Atomic replace, as the runtime does it."""
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary = path.with_name(f".{path.name}.{os.getpid()}.tmp")
    temporary.write_text(json.dumps(data, ensure_ascii=False), encoding="utf-8")
    os.replace(temporary, path)


def _tone(path: Path, seconds: float, kind: str, seed: int) -> None:
    """A short mono WAV: a drum-ish burst, a tone or a swell, so auditions are audible and waveforms differ."""
    rate = 22050
    rng = random.Random(seed)
    count = max(1, int(seconds * rate))
    pitch = 50 + (seed % 37) * 9
    frames = bytearray()
    for i in range(count):
        t = i / rate
        if kind == "loop":
            level = 0.35 * min(1.0, t / 0.4) * (0.7 + 0.3 * math.sin(2 * math.pi * 0.5 * t))
            value = level * (math.sin(2 * math.pi * pitch * 2 * t) + 0.4 * math.sin(2 * math.pi * pitch * 3.01 * t))
        else:
            decay = math.exp(-t * (8 + seed % 11))
            noise = rng.uniform(-1.0, 1.0)
            body = math.sin(2 * math.pi * pitch * t * (1 - 0.5 * min(1.0, t * 8)))
            value = decay * (0.6 * body + 0.4 * noise)
        frames += struct.pack("<h", int(max(-1.0, min(1.0, value)) * 26000))
    path.parent.mkdir(parents=True, exist_ok=True)
    with wave.open(str(path), "wb") as file:
        file.setnchannels(1)
        file.setsampwidth(2)
        file.setframerate(rate)
        file.writeframes(bytes(frames))


def map_points(revision: str = "r7") -> tuple[list, dict, dict]:
    """The sound map's points, groups and sounds, without paths."""
    rng = random.Random(7)
    points: list = []
    groups: dict = {}
    sounds: dict = {}
    number = 0
    for index, (name, colour, (cx, cy), spread, count, folder, stems, kind) in enumerate(GROUPS):
        groups[str(index)] = {"name": name, "color": colour, "count": count}
        for n in range(count):
            number += 1
            sound_id = f"s-{revision}-{number:04d}"
            x = min(1.0, max(0.0, rng.gauss(cx, spread)))
            y = min(1.0, max(0.0, rng.gauss(cy, spread)))
            points.append([sound_id, round(x, 4), round(y, 4), index])
            stem = stems[n % len(stems)]
            duration = round(rng.uniform(2.0, 8.0), 2) if kind == "loop" else round(rng.uniform(0.15, 1.2), 2)
            sounds[sound_id] = {
                "name": f"{stem} {n // len(stems) + 1:02d}",
                "kind": kind,
                "class": name,
                "folder": folder,
                "duration": duration,
            }
    return points, groups, sounds


def write_state(plugin_data: Path, library: Path, *, audio: str = "some", now_ms: int | None = None) -> Path:
    """Write the whole state folder under `plugin_data` (the plugin's `plugin-data/kumi`). `audio`: `all` writes a
    WAV for every map sound, `some` for the snares and the measured files, `none` for none. Returns the state folder."""
    now = _now_ms() if now_ms is None else now_ms
    state = plugin_data / "state"
    # As `kumi serve` writes it (W1, #945): from its start, its heartbeat every 3 s, `running: false` at its exit.
    owner = {**_envelope(41, now), "profile": OWNER["profile"], "session_id": OWNER["session_id"], "pid": OWNER["pid"],
             "since": now - 25 * 60_000, "heartbeat": now, "running": True, "lease": "free"}
    _write_json(state / "owner.json", owner)
    _write_json(
        state / "status.json",
        {
            **_envelope(12, now),
            "connected": True,
            "daw": "ableton",
            # What `kumi serve` sees of Live's part (W1); `/setup` answers from setup.py's `status()` instead.
            "setup": {"component": {"installed": True, "version": "1.0.104", "bundled": "1.0.104"}, "daw_open": True},
            "stale": False,
            "runtime": {"version": "1.24.70", "protocol": "kumi-serve/1"},
            "project": dict(PROJECT),
            "capabilities": ["observe", "read", "change", "undo", "history", "listen", "sounds", "library", "map"],
        },
    )
    _write_json(
        state / "focus.json",
        {
            **_envelope(88, now),
            "project": dict(PROJECT),
            "focus": {
                "track": {"name": "Drums", "color": "#f59a3c", "kind": "midi"},
                "device": {"name": "Drum Rack"},
                "clip": None,
                "view": "Session",
            },
            "transport": {"playing": True, "tempo": 124.0, "position": "17.1.1"},
            "catch_up": ["Drums: Drum Rack's pad D1 changed", "Tempo 120 → 124 BPM"],
        },
    )
    minute = 60_000
    _write_json(
        state / "history.json",
        {
            **_envelope(9, now),
            "items": [
                {
                    "id": "h-p6f1c9e-g2-c14",
                    "label": "c14",
                    "project_key": PROJECT["key"],
                    "runtime_generation": 2,
                    "title": "Drums volume −6.0 → −4.0 dB",
                    "family": "mixer",
                    "at": now - 1 * minute,
                    "state": "applied",
                    "undo_available": True,
                    "track": {"name": "Drums", "color": "#f59a3c"},
                },
                {
                    "id": "h-p6f1c9e-g2-c13",
                    "label": "c13",
                    "project_key": PROJECT["key"],
                    "runtime_generation": 2,
                    "title": "Drum Rack pad D1: Snare 01.wav → Dusty Snare 04.wav",
                    "family": "sample",
                    "at": now - 3 * minute,
                    "state": "applied",
                    "undo_available": True,
                    "track": {"name": "Drums", "color": "#f59a3c"},
                },
                {
                    "id": "h-p6f1c9e-g2-c12",
                    "label": "c12",
                    "project_key": PROJECT["key"],
                    "runtime_generation": 2,
                    "title": "Tempo 120 → 124 BPM",
                    "family": "tempo",
                    "at": now - 9 * minute,
                    "state": "undone",
                    "undo_available": False,
                },
                {
                    "id": "h-p6f1c9e-g1-c11",
                    "label": "c11",
                    "project_key": PROJECT["key"],
                    "runtime_generation": 1,
                    "title": "Bass: Saturator added after Operator",
                    "family": "device",
                    "at": now - 42 * minute,
                    "state": "applied",
                    "undo_available": False,
                    "track": {"name": "Bass", "color": "#5ec1f7"},
                },
            ],
        },
    )
    points, groups, sounds = map_points("r7")
    # `some`: the first snares only (the ones a test or a screenshot auditions), so writing the state stays quick.
    first_snares = {sound_id for sound_id, sound in sounds.items() if sound["class"] == "snare"}
    first_snares = set(sorted(first_snares)[:4])
    for sound_id, sound in sounds.items():
        sound["path"] = str(library / sound["folder"] / f"{sound['name']}.wav")
        if audio == "all" or (audio == "some" and sound_id in first_snares):
            _tone(Path(sound["path"]), min(sound["duration"], 1.5), sound["kind"], int(sound_id[-4:]))
    _write_json(state / "map" / "r7.json", {**_envelope(3, now), "revision": "r7", "made": now - 3600_000, "points": points, "groups": groups, "sounds": sounds})
    (state / "map" / "current").write_text("r7\n", encoding="utf-8")
    artifacts_dir = state / "artifacts"
    original = artifacts_dir / "a-7f31-original.wav"
    replaced = artifacts_dir / "a-7f32-dusty.wav"
    if audio != "none":
        _tone(original, 0.6, "one-shot", 11)
        _tone(replaced, 0.6, "one-shot", 23)
    _write_json(
        state / "artifacts.json",
        {
            **_envelope(5, now),
            "artifacts": {
                "a-7f31": {"path": str(original), "kind": "audio", "mime": "audio/wav", "tool": "kumi_listen", "at": now - 2 * minute},
                "a-7f32": {"path": str(replaced), "kind": "audio", "mime": "audio/wav", "tool": "kumi_listen", "at": now - 2 * minute},
            },
        },
    )
    _write_json(state / "jobs" / "j-scan-1.json", {**_envelope(30, now), "state": "running", "progress": {"done": 412, "total": 1200, "message": "Scanning Drums"}})
    return state


_BANDS = [("sub", "20–60"), ("bass", "60–120"), ("upper bass", "120–250"), ("low mids", "250–500"), ("mids", "500–1k"),
          ("upper mids", "1k–2k"), ("presence", "2k–4k"), ("bite", "4k–6k"), ("brilliance", "6k–10k"), ("air", "10k–20k")]


def analysis(file: str, *, lufs: float, peak: float, centroid: float, levels: list[float], seconds: float = 0.6) -> dict:
    """A `kumi_media::audio::Analysis` as `kumi_listen` serializes it (camelCase), with nulls where Kumi leaves them."""
    return {
        "kumiAudio": 1,
        "file": file,
        "format": "wav",
        "sampleRate": 44100.0,
        "channels": 1,
        "seconds": seconds,
        "analyzed": {"from": "0:00", "to": "0:00.6", "focus": "sound"},
        "loudness": {"integratedLufs": lufs, "shortTermMaxLufs": None, "rangeLu": None, "truePeakDbtp": peak, "samplePeakDbfs": peak - 0.2, "clippedSamples": 0},
        "balance": {
            "bands": [{"name": name, "hz": hz, "db": db, "width": 0.0, "correlation": 1.0} for (name, hz), db in zip(_BANDS, levels)],
            "tiltDbPerOctave": -3.1,
            "centroidHz": centroid,
        },
        "stereo": None,
        "dynamics": {"crestDb": 14.2, "peakToLoudnessDb": 13.4, "onsetsPerSecond": 1.7},
        "tempo": None,
        "key": None,
        "overTime": {"every": "0.1 s", "lufs": [None, lufs, lufs - 4.0]},
        "spectrogram": {"rows": [{"band": name, "cells": "█▆▃▁▁▁"} for name, _ in _BANDS], "columns": "0:00 → 0:00.6", "scale": "relative"},
    }


def listen_result() -> dict:
    """What `kumi_listen` returns when it compares the new snare (measured) with the old one (reference)."""
    mine_levels = [-30, -22, -14, -10, -9, -11, -14, -18, -24, -33]
    reference_levels = [-32, -25, -16, -11, -8, -8, -9, -11, -15, -22]
    mine = analysis("Dusty Snare 04.wav", lufs=-14.2, peak=-0.8, centroid=2380.0, levels=mine_levels)
    reference = analysis("Snare 01.wav", lufs=-12.9, peak=-0.3, centroid=3520.0, levels=reference_levels)
    return {
        "metrics": mine,
        "artifact_id": "a-7f32",
        "verdict_text": "Dusty Snare 04 is darker than Snare 01: its spectral centroid is 1,140 Hz lower, at 1.3 LU less loudness.",
        "media_tag": "MEDIA:/tmp/kumi/a-7f32-dusty.wav",
        "compare": {
            "artifact_id": "a-7f31",
            "metrics": reference,
            "differences": {
                "kumiAudioComparison": 1,
                "mine": "Dusty Snare 04.wav",
                "reference": "Snare 01.wav",
                "headlines": ["1.3 LU quieter than the reference", "Darker: less presence, bite and brilliance"],
                "loudness": {"mineLufs": -14.2, "referenceLufs": -12.9, "differenceLu": -1.3, "truePeak": [-0.8, -0.3], "range": [None, None]},
                "balance": [{"band": name, "hz": hz, "difference": float(a - b)} for (name, hz), a, b in zip(_BANDS, mine_levels, reference_levels)],
                "tilt": [-3.1, -2.2],
                "width": [],
                "stereo": None,
                "dynamics": {"crestDb": [14.2, 13.1], "peakToLoudnessDb": [13.4, 12.6], "onsetsPerSecond": [1.7, 1.7]},
                "tempo": [None, None],
                "key": [None, None],
            },
        },
    }


def _live(plugin_data: Path) -> None:
    state = plugin_data / "state"
    beat = 0
    while True:
        now = _now_ms()
        owner = json.loads((state / "owner.json").read_text(encoding="utf-8"))
        owner.update(seq=owner.get("seq", 0) + 1, at=now, heartbeat=now)
        _write_json(state / "owner.json", owner)
        beat += 1
        if beat % 2 == 0:
            focus = json.loads((state / "focus.json").read_text(encoding="utf-8"))
            bar = 17 + (beat // 2) % 16
            focus["transport"]["position"] = f"{bar}.{1 + beat % 4}.1"
            _write_json(state / "focus.json", focus)
        time.sleep(2)


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("plugin_data", type=Path, help="the plugin's data folder: <HERMES_HOME>/plugin-data/kumi")
    parser.add_argument("library", type=Path, help="a folder for the map's samples (an approved sample folder)")
    parser.add_argument("--audio", choices=["all", "some", "none"], default="all")
    parser.add_argument("--live", action="store_true", help="keep the heartbeat fresh until stopped")
    parser.add_argument("--session-id", help="the stored id of the chat that owns the runtime (default: a made-up one)")
    args = parser.parse_args(argv)
    if args.session_id:
        OWNER["session_id"] = args.session_id
    state = write_state(args.plugin_data, args.library, audio=args.audio)
    print(state)
    if args.live:
        try:
            _live(args.plugin_data)
        except KeyboardInterrupt:
            return 0
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
