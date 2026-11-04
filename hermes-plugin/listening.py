"""Optional interpretation of measured audio, through Hermes's own model slot.

No files, credentials, model clients or audio decoders are opened here. The host
owns the opt-in switch and keeps the measured result if interpretation fails.
"""

from __future__ import annotations

import json
import math
import struct
import zlib
from typing import Any

AUX_TASK_KEY = "kumi_listen"
AUX_TASK = {
    "display_name": "Kumi listening interpretation",
    "description": (
        "Interpret measured audio and a coarse band-energy image, not raw audio. "
        "Choose an image-capable model; follows the Vision slot until overridden."
    ),
    "inherit_from": "vision",
    "defaults": {"timeout": 60},
}

_MAX_TIMEOUT = 120.0
_MAX_REPLY = 2400
_CELL_SIZE = 12
# kumi_media::audio::analyze::BANDS. Names below are fixed measurement labels,
# never a file, track, sound or project name supplied by the runtime.
_BANDS = {
    "sub": (20, 60),
    "bass": (60, 120),
    "upper bass": (120, 250),
    "low mids": (250, 500),
    "mids": (500, 1000),
    "upper mids": (1000, 2000),
    "presence": (2000, 4000),
    "bite": (4000, 6000),
    "brilliance": (6000, 10000),
    "air": (10000, 20000),
}
_NOTES = ("C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B")
_KEYS = frozenset(f"{note} {mode}" for note in _NOTES for mode in ("major", "minor"))
_SCHEMA = {
    "type": "object",
    "properties": {"text": {"type": "string", "minLength": 1, "maxLength": _MAX_REPLY}},
    "required": ["text"],
    "additionalProperties": False,
}
_INSTRUCTIONS = """Interpret only the supplied measurements and band-energy images.
You have NOT heard the audio. Do not claim to have listened, recognize instruments,
identify a genre, or assess performance or musical quality from these inputs.
Give a short, useful reading grounded in measured values and units, with limits.

The input labels current and reference identify the two measurements, not names.
If both are present, compare current against reference. A louder signal is not
inherently better. Use LUFS and numeric band shares for level-aware comparisons;
do not compare image brightness between files: each image is peak-normalized.
LUFS is integrated loudness, dBTP is true peak, and centroid is in Hz. Mono has no
stereo-width evidence. Nulls mean unavailable, not zero. Tempo/key confidence is
uncertainty, not proof. Do not invent missing measurements or perceptual facts.

Images are coarse band-energy heatmaps, NOT high-resolution FFT spectrograms.
Their fixed band labels and time window are in the text. Left to right is time;
row order is given top to bottom. White is the loudest displayed band/time cell;
each level is 3 dB lower. Black can mean below the display floor, not just silence.
Never infer exact frequencies, phase or transients smaller than this resolution.
If an image is absent, explicitly limit the reading to the measured numbers.
Do not propose or execute DAW edits. Return the requested JSON object only.
"""


class InterpretationError(RuntimeError):
    """A safe, user-facing failure; never contains a provider exception's text."""


def _number(value: Any) -> int | float | None:
    if value is None:
        return None
    if type(value) not in (int, float):
        raise InterpretationError("The runtime returned a non-numeric measurement.")
    try:
        finite = math.isfinite(value)
    except OverflowError:
        finite = False
    if not finite:
        raise InterpretationError("The runtime returned a non-finite measurement.")
    return value


def _numbers(value: Any, fields: tuple[str, ...]) -> dict[str, Any]:
    if not isinstance(value, dict):
        raise InterpretationError("The runtime returned an incomplete measurement.")
    return {name: _number(value[name]) for name in fields if name in value}


def _optional_numbers(value: Any, fields: tuple[str, ...]) -> dict[str, Any] | None:
    return None if value is None else _numbers(value, fields)


def _measurements(analysis: Any) -> dict[str, Any]:
    """Project Analysis onto numeric fields and fixed labels; never forward it."""
    if not isinstance(analysis, dict) or analysis.get("kumiAudio") != 1:
        raise InterpretationError("The runtime did not return a supported audio measurement.")
    if not all(name in analysis for name in ("sampleRate", "channels", "seconds", "loudness", "balance", "dynamics")):
        raise InterpretationError("The runtime returned an incomplete measurement.")
    result = _numbers(analysis, ("sampleRate", "channels", "seconds"))
    result["loudness"] = _numbers(
        analysis["loudness"],
        ("integratedLufs", "shortTermMaxLufs", "rangeLu", "truePeakDbtp", "samplePeakDbfs", "clippedSamples"),
    )
    balance = analysis["balance"]
    result["balance"] = _numbers(balance, ("tiltDbPerOctave", "centroidHz"))
    bands = balance.get("bands", [])
    if not isinstance(bands, list) or len(bands) > len(_BANDS):
        raise InterpretationError("The runtime returned an invalid band measurement.")
    measured_bands = []
    seen = set()
    for band in bands:
        if not isinstance(band, dict):
            raise InterpretationError("The runtime returned an invalid band measurement.")
        name = band.get("name")
        if not isinstance(name, str) or name not in _BANDS or name in seen:
            raise InterpretationError("The runtime returned an unknown or repeated frequency band.")
        seen.add(name)
        measured_bands.append({"band": name, "hz": _BANDS[name], **_numbers(band, ("db", "width", "correlation"))})
    result["balance"]["bands"] = measured_bands
    result["dynamics"] = _numbers(analysis["dynamics"], ("crestDb", "peakToLoudnessDb", "onsetsPerSecond"))
    result["stereo"] = _optional_numbers(analysis.get("stereo"), ("correlation", "width"))
    result["tempo"] = _optional_numbers(analysis.get("tempo"), ("bpm", "confidence"))
    key = analysis.get("key")
    result["key"] = _optional_numbers(key, ("confidence",))
    if key is not None:
        name = key.get("name")
        if not isinstance(name, str) or name not in _KEYS:
            raise InterpretationError("The runtime returned an unknown measured key.")
        result["key"]["name"] = name
    sound = analysis.get("sound")
    if sound is not None:
        if not isinstance(sound, dict):
            raise InterpretationError("The runtime returned an invalid sound measurement.")
        result["sound"] = {
            "pitch": _optional_numbers(sound.get("pitch"), ("hz", "cents", "confidence")),
            "envelope": _numbers(sound.get("envelope"), ("attackMs", "decayMs", "sustainDb", "releaseMs", "lengthMs")),
            "harmonics": _optional_numbers(
                sound.get("harmonics"), ("slopeDbPerOctave", "oddToEven", "noiseDb", "inharmonicity")
            ),
        }
    return result


def _chunk(kind: bytes, data: bytes) -> bytes:
    body = kind + data
    return struct.pack(">I", len(data)) + body + struct.pack(">I", zlib.crc32(body))


def _heatmap(spectrum: Any, seconds: Any) -> tuple[bytes, dict[str, Any]]:
    """Rasterize the existing quantized grid, without decoding or reading audio."""
    if not isinstance(spectrum, dict):
        raise InterpretationError("The runtime returned an invalid measured spectrogram.")
    rows = spectrum.get("rows")
    if not isinstance(rows, list) or not 1 <= len(rows) <= len(_BANDS):
        raise InterpretationError("The measured spectrogram has an invalid row count.")
    columns = None
    seen = set()
    labels = []
    pixels = bytearray()
    for row in rows:
        if not isinstance(row, dict):
            raise InterpretationError("The runtime returned an invalid measured spectrogram.")
        band, cells = row.get("band"), row.get("cells")
        if not isinstance(band, str) or band not in _BANDS or band in seen:
            raise InterpretationError("The measured spectrogram has an unknown or repeated frequency band.")
        if not isinstance(cells, str) or not 1 <= len(cells) <= 24 or any(cell not in "0123456789·" for cell in cells):
            raise InterpretationError("The measured spectrogram has an invalid energy grid.")
        if columns is not None and len(cells) != columns:
            raise InterpretationError("The measured spectrogram rows have different time grids.")
        columns = len(cells)
        seen.add(band)
        labels.append({"band": band, "hz": _BANDS[band]})
        # Grayscale: silent/below-floor black; the ten displayed levels span
        # 26..255. No raw metadata or arbitrary strings enter the PNG.
        scanline = b"\x00" + b"".join(bytes([0 if cell == "·" else round((int(cell) + 1) * 25.5)]) * _CELL_SIZE for cell in cells)
        pixels.extend(scanline * _CELL_SIZE)
    width, height = columns * _CELL_SIZE, len(rows) * _CELL_SIZE
    png = (
        b"\x89PNG\r\n\x1a\n"
        + _chunk(b"IHDR", struct.pack(">IIBBBBB", width, height, 8, 0, 0, 0, 0))
        + _chunk(b"IDAT", zlib.compress(pixels))
        + _chunk(b"IEND", b"")
    )
    return png, {
        "rowsTopToBottom": labels,
        "timeColumns": columns,
        "relativeWindowSeconds": _number(seconds),
        "dbPerStep": 3,
        "scale": "peak-relative",
        "black": "below-display-floor-or-silence",
    }


def interpret(llm: Any, result: dict[str, Any], *, timeout: float) -> dict[str, Any]:
    """Make one auxiliary call; return interpretation only, leaving result intact.

    Only W2's opted-in handler calls this. A failure must not cause that handler
    to discard the measurements or repeat a render/capture.
    """
    try:
        timeout = float(timeout) if type(timeout) in (int, float) else math.nan
    except OverflowError:
        timeout = math.nan
    if not math.isfinite(timeout) or timeout <= 0:
        raise InterpretationError("The listening interpretation timeout must be a positive finite number.")
    if not isinstance(result, dict):
        raise InterpretationError("The runtime did not return a measured result.")
    analyses = [("current", "metrics", "spectrogram", result.get("metrics"))]
    comparison = result.get("compare")
    if comparison is not None:
        if not isinstance(comparison, dict):
            raise InterpretationError("The runtime returned an invalid comparison.")
        analyses.append(("reference", "comparison_metrics", "comparison_spectrogram", comparison.get("metrics")))
    supplied = []
    blocks = []
    for label, metrics_input, spectrum_input, analysis in analyses:
        measured = _measurements(analysis)
        supplied.append(metrics_input)
        spectrum = analysis.get("spectrogram")
        image = None
        if spectrum is not None:
            image, measured["bandEnergyImage"] = _heatmap(spectrum, measured["seconds"])
            supplied.append(spectrum_input)
        else:
            measured["bandEnergyImage"] = None
        blocks.append({"type": "text", "text": json.dumps({label: measured}, allow_nan=False, separators=(",", ":"))})
        if image is not None:
            blocks.append({"type": "image", "data": image, "mime_type": "image/png", "file_name": f"{label}-band-energy.png"})
    try:
        completed = llm.complete_structured(
            task=AUX_TASK_KEY,
            purpose="kumi.listen.interpret",
            instructions=_INSTRUCTIONS,
            input=blocks,
            json_schema=_SCHEMA,
            schema_name="kumi_audio_measurement_interpretation",
            max_tokens=700,
            timeout=min(float(timeout), _MAX_TIMEOUT),
        )
    except TimeoutError:
        raise InterpretationError("Model interpretation timed out; the measured results remain available.") from None
    except PermissionError:
        raise InterpretationError("Hermes refused the listening auxiliary task; check its model settings. The measurements remain available.") from None
    except Exception:
        raise InterpretationError(
            "Model interpretation failed; choose an image-capable model for Kumi listening in Hermes. The measurements remain available."
        ) from None
    parsed = getattr(completed, "parsed", None)
    # Hermes validates JSON Schema only when its optional jsonschema dependency
    # is available. Never substitute an unparsed or malformed model reply.
    if not isinstance(parsed, dict) or set(parsed) != {"text"}:
        raise InterpretationError("The model did not return a valid structured interpretation; the measurements remain available.")
    text = parsed["text"]
    if not isinstance(text, str) or not text.strip() or len(text) > _MAX_REPLY:
        raise InterpretationError("The model returned an empty or oversized interpretation; the measurements remain available.")
    interpretation = {
        "text": "Model interpretation of measurements (no raw audio heard):\n" + text.strip(),
        "inputs": supplied,
    }
    model = getattr(completed, "model", None)
    if isinstance(model, str) and model.strip() and len(model) <= 200 and not any(ord(char) < 32 for char in model):
        interpretation["model"] = model.strip()
    return interpretation
