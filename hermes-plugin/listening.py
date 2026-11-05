"""Optional interpretation of measured audio, through Hermes's own model slot.

No files, credentials, model clients or audio decoders are opened here. The host
owns the opt-in switch and keeps the measured result if interpretation fails.
"""

from __future__ import annotations

import json
import math
import re
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

# Hermes applies this to each attempt; its own retries and provider fallbacks
# can make the complete host call take longer.
_MAX_ATTEMPT_TIMEOUT = 120.0
_MAX_REPLY = 2400
_CELL_SIZE = 12
_CLOCK = re.compile(r"([0-9]{1,9}):([0-5][0-9])")
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
fileSeconds is the whole file's length, NOT the measured window. The analyzed
from/to values are runtime clock labels floored to whole seconds, not precise
sample boundaries; equal labels can mean a sub-second span, not empty audio.
Images cover that analyzed span. If its labels are unavailable, do not infer the
image's duration from the file length.

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


def _clock_seconds(value: Any) -> int | None:
    """Read only the runtime's M:SS format; never forward its clock strings."""
    matched = _CLOCK.fullmatch(value) if isinstance(value, str) else None
    return int(matched[1]) * 60 + int(matched[2]) if matched else None


def _analyzed_window(analysis: dict[str, Any], file_seconds: Any) -> dict[str, Any]:
    window = analysis.get("analyzed")
    start = _clock_seconds(window.get("from")) if isinstance(window, dict) else None
    end = _clock_seconds(window.get("to")) if isinstance(window, dict) else None
    if start is None or end is None or end < start or (file_seconds is not None and end > file_seconds):
        start = end = None
    return {
        "analyzedFromSeconds": start,
        "analyzedToSeconds": end,
        "analyzedClockResolutionSeconds": 1,
        "analyzedClockRounding": "floor",
    }


def _measurements(analysis: Any) -> dict[str, Any]:
    """Project Analysis onto numeric fields and fixed labels; never forward it."""
    if not isinstance(analysis, dict) or analysis.get("kumiAudio") != 1:
        raise InterpretationError("The runtime did not return a supported audio measurement.")
    if not all(name in analysis for name in ("sampleRate", "channels", "seconds", "loudness", "balance", "dynamics")):
        raise InterpretationError("The runtime returned an incomplete measurement.")
    result = _numbers(analysis, ("sampleRate", "channels"))
    result["fileSeconds"] = _number(analysis["seconds"])
    result.update(_analyzed_window(analysis, result["fileSeconds"]))
    mono = result["channels"] == 1
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
        fields = ("db",) if mono else ("db", "width", "correlation")
        measured_band = {"band": name, "hz": _BANDS[name], **_numbers(band, fields)}
        if mono:
            measured_band.update(width=None, correlation=None)
        measured_bands.append(measured_band)
    result["balance"]["bands"] = measured_bands
    result["dynamics"] = _numbers(analysis["dynamics"], ("crestDb", "peakToLoudnessDb", "onsetsPerSecond"))
    result["stereo"] = None if mono else _optional_numbers(analysis.get("stereo"), ("correlation", "width"))
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


def _heatmap(spectrum: Any) -> tuple[bytes, dict[str, Any]]:
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
        "dbPerStep": 3,
        "scale": "peak-relative",
        "black": "below-display-floor-or-silence",
    }


def _rejects_image_input(error: Exception) -> bool:
    """Recognize explicit provider modality refusals, never infer model capabilities.

    Hermes v0.22.7 preserves SDK exceptions at the PluginLlm boundary. This narrow
    subset of its message_sanitization image-rejection cases excludes corrupt
    payloads, invalid URLs and generic content errors: those do not prove that
    the listening model lacks image input.
    """
    if getattr(error, "status_code", None) not in (400, 404, 422):
        return False
    body = getattr(error, "body", None)
    if isinstance(body, dict):
        if isinstance(body.get("error"), dict):
            body = body["error"]
        message = body.get("message")
    else:
        message = body if isinstance(body, str) else getattr(error, "message", None)
    if not isinstance(message, str):
        return False
    message = message[:4096].lower()
    return any(phrase in message for phrase in (
        "only 'text' content type is supported", "only text content type is supported",
        "image_url is not supported", "image content is not supported",
        "multimodal is not supported", "multimodal content is not supported",
        "multimodal input is not supported", "vision is not supported",
        "vision input is not supported", "does not support images",
        "does not support image input", "does not support multimodal",
        "does not support vision", "model does not support image",
        "no endpoints found that support image input",
    ))


def interpret(llm: Any, result: dict[str, Any], *, timeout: float) -> dict[str, Any]:
    """Make one auxiliary call; return interpretation only, leaving result intact.

    Only W2's opted-in handler calls this. A failure must not cause that handler
    to discard the measurements or repeat a render/capture.
    Timeout is per attempt, not an overall deadline: Hermes owns its retries
    and configured provider fallbacks. This helper neither retries nor chooses
    another provider.
    """
    try:
        timeout = float(timeout) if type(timeout) in (int, float) else math.nan
    except OverflowError:
        timeout = math.nan
    if not math.isfinite(timeout) or timeout <= 0:
        raise InterpretationError("The listening interpretation timeout must be a positive finite number.")
    if not isinstance(result, dict):
        raise InterpretationError("The runtime did not return a measured result.")
    if result.get("state") not in (None, "done") or "error" in result:
        raise InterpretationError("The runtime did not complete the measurement; no interpretation was requested.")
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
            image, measured["bandEnergyImage"] = _heatmap(spectrum)
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
            timeout=min(float(timeout), _MAX_ATTEMPT_TIMEOUT),
        )
    except TimeoutError:
        raise InterpretationError("Model interpretation timed out; the measurements remain available.") from None
    except PermissionError:
        raise InterpretationError("Hermes didn't allow Kumi listening interpretation: check that task's model in Hermes's settings. The measurements remain available.") from None
    except Exception as error:
        if any(block["type"] == "image" for block in blocks) and _rejects_image_input(error):
            raise InterpretationError("No interpretation: the model set for Kumi listening interpretation doesn't accept images. Choose one that does in Hermes's model settings. The measurements remain available.") from None
        raise InterpretationError("Model interpretation failed; the measurements remain available.") from None
    parsed = getattr(completed, "parsed", None)
    # Hermes validates JSON Schema only when its optional jsonschema dependency
    # is available. Never substitute an unparsed or malformed model reply.
    if not isinstance(parsed, dict) or set(parsed) != {"text"}:
        raise InterpretationError("The model did not return a valid structured interpretation; the measurements remain available.")
    text = parsed["text"]
    if not isinstance(text, str) or not text.strip() or len(text) > _MAX_REPLY:
        raise InterpretationError("The model returned an empty or oversized interpretation; the measurements remain available.")
    text = "".join(char for char in text if char in "\n\t" or (ord(char) >= 32 and ord(char) != 127)).strip()
    if not text:
        raise InterpretationError("The model returned an empty interpretation; the measurements remain available.")
    interpretation = {
        "text": "Model interpretation of measurements (no raw audio heard):\n" + text,
        "inputs": supplied,
    }
    model = getattr(completed, "model", None)
    if isinstance(model, str) and model.strip() and len(model) <= 200 and not any(ord(char) < 32 or ord(char) == 127 for char in model):
        interpretation["model"] = model.strip()
    return interpretation
