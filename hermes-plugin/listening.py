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

_PROFILE_REASONS = frozenset(("not_requested", "zero_energy", "too_short_for_profile_frames", "no_usable_profile_energy",
                              "too_short_for_loudness", "below_loudness_gate", "too_short_for_range", "mono",
                              "no_low_band_energy", "not_measured", "no_reference_measurement",
                              "above_nyquist", "partial_frequency_coverage"))
_PROFILE_QUANTITIES = {
    **{f"region/{name}": ("dB", "Heard 31-third-octave relative band power / Profile::regions")
       for name in ("sub", "bass", "low mids", "mids", "upper mids", "presence", "brilliance", "air")},
    "integrated": ("LUFS", "Heard BS.1770-4 integrated / Profile::integrated; distinct from canonical Analysis"),
    "plr": ("dB", "Heard true peak minus Heard integrated / Profile::plr"),
    "crest": ("dB", "Heard median 400 ms crest of the louder half / Profile::crest"),
    "low_width": ("dB", "Heard side/mid power below 120 Hz / Profile::low_width; not canonical full-band width"),
    "tilt": ("dB/octave", "Heard 100 Hz–10 kHz third-octave slope / Profile::tilt"),
    "range": ("LU", "Heard EBU 3342 loudness range / Profile::range"),
    "centroid": ("Hz", "Heard median loud-frame centroid / Profile::centroid; not canonical energy-weighted centroid"),
    "noise": ("dB", "Heard loud-frame spectral flatness / Profile::noise"),
    "attack": ("ms", "Heard median per-hit 10–90 percent attack / Profile::attack"),
    "decay": ("ms", "Heard median per-hit peak to -20 dB decay / Profile::decay"),
    "sustain": ("dB", "Heard per-hit level a quarter second after peak / Profile::sustain"),
}
_AVAILABILITY_QUANTITIES = frozenset(_PROFILE_QUANTITIES) | {
    "regions", "tempo", "key", "busyness", "groove", "sound.width", "sound.pitch_drop", "sound.modulation",
    "sound.bandwidth", "sound.noise_floor", "sound.warmth", "sound.tail", "sound.crackle", "sound.decay_time",
    "sound.darkening", "sound.echo_time", "sound.echo_falls", "sound.swing", "sound.sweep", "sound.tail_share",
    "sound.pumping", "sound.hit_crest",
}
_WINDOW_INSTRUCTIONS = """
Exact sample windows supersede rounded analyzed clock labels. Retained excerpts replay at zero;
original offsets describe provenance, never a second playback offset.
"""
_PROFILE_INSTRUCTIONS = """
Numbered references are separate canonical Analysis measurements, not an averaged recording.
The reference_profile quantities use real Heard/Profile on BOTH sides. Never subtract a canonical
Analysis value from a Profile target, even when names or units match. Deviations are source minus
target.mid. References have equal weight, not duration weight. Spreads include minimum tolerances;
fewer than five references use member-center min/max. These are NOT confidence intervals or purely
observed variation. A short-frame fixed region tolerance is not measured variability.
Report null values with their reasons and actual contributor/reference counts; silence is not a tonal
target. There is no Profile true-peak target and no scalar better score: keep each canonical peak and
the producer's ceiling separate.
Each member's profile_quantities is its Heard/Profile measurement, distinct from its canonical metrics.
Null member values are excluded only from that quantity's target and contributor count. A low sample rate
can leave above-Nyquist or partially covered regions unavailable; never replace them with a floor or zero,
and never discard the member's other usable quantities or substitute canonical band measurements.
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


def _count(value: Any, *, maximum: int | None = None) -> int:
    if type(value) is not int or value < 0 or (maximum is not None and value > maximum):
        raise InterpretationError("The runtime returned an invalid measurement count.")
    return value


def _reason(value: Any) -> str | None:
    if value is not None and (not isinstance(value, str) or value not in _PROFILE_REASONS):
        raise InterpretationError("The runtime returned an unknown availability reason.")
    return value


def _nyquist(value: Any, reason: str | None) -> int | float | None:
    number = _number(value)
    if (number is not None and number <= 0) or (
            reason in ("above_nyquist", "partial_frequency_coverage") and number is None):
        raise InterpretationError("The runtime returned invalid frequency coverage.")
    return number


def _sample_window(value: Any) -> dict:
    if not isinstance(value, dict):
        raise InterpretationError("The runtime returned an invalid exact audio window.")
    result = _numbers(value, ("original_seconds", "sample_rate", "replay_from"))
    for key in ("start_frame", "frame_count", "channels"):
        result[key] = _count(value.get(key))
    total = value.get("original_total_frames")
    result["original_total_frames"] = None if total is None else _count(total)
    exact = value.get("original_duration_exact")
    if (type(exact) is not bool or result.get("sample_rate") is None or result["sample_rate"] <= 0
            or result["frame_count"] <= 0 or result["channels"] <= 0 or result.get("replay_from") != 0):
        raise InterpretationError("The runtime returned an invalid exact audio window.")
    result["original_duration_exact"] = exact
    requested = value.get("requested")
    result["requested"] = None if requested is None else _numbers(requested, ("from", "seconds"))
    if requested is not None:
        window = result["requested"]
        if (set(window) != {"from", "seconds"} or window["from"] is None or window["seconds"] is None
                or window["from"] < 0 or not 0 < window["seconds"] <= 60
                or not math.isfinite(window["from"] + window["seconds"])):
            raise InterpretationError("The runtime returned an invalid requested audio window.")
    return result


def _availability(value: Any) -> dict:
    if not isinstance(value, dict) or any(type(value.get(key)) is not bool
                                          for key in ("energy_zero", "fixed_region_tolerance")):
        raise InterpretationError("The runtime returned invalid reference availability.")
    missing = value.get("unavailable")
    if not isinstance(missing, list) or len(missing) > len(_AVAILABILITY_QUANTITIES):
        raise InterpretationError("The runtime returned invalid reference availability.")
    rows = []
    for row in missing:
        key = row.get("quantity") if isinstance(row, dict) else None
        if not isinstance(key, str) or key not in _AVAILABILITY_QUANTITIES:
            raise InterpretationError("The runtime returned an unknown unavailable quantity.")
        rows.append({"quantity": key, "reason": _reason(row.get("reason"))})
    return {"energy_zero": value["energy_zero"], "fixed_region_tolerance": value["fixed_region_tolerance"],
            "loud_frame_count": _count(value.get("loud_frame_count")),
            "profile_reason": _reason(value.get("profile_reason")), "unavailable": rows}


def _member_profile_facts(value: Any) -> list[dict]:
    if not isinstance(value, list) or not value or len(value) > len(_PROFILE_QUANTITIES):
        raise InterpretationError("The runtime returned invalid member profile quantities.")
    rows, seen = [], set()
    for row in value:
        key = row.get("quantity") if isinstance(row, dict) else None
        if not isinstance(key, str) or key not in _PROFILE_QUANTITIES or key in seen:
            raise InterpretationError("The runtime returned an unknown or repeated member profile quantity.")
        seen.add(key)
        unit, basis = _PROFILE_QUANTITIES[key]
        if row.get("unit") != unit or row.get("basis") != basis:
            raise InterpretationError("The runtime returned an unsupported member profile basis.")
        measured, reason = _number(row.get("value")), _reason(row.get("reason_code"))
        nyquist = _nyquist(row.get("nyquist_hz"), reason)
        if (measured is None) != (reason is not None):
            raise InterpretationError("The runtime returned inconsistent member profile availability.")
        rows.append({"quantity": key, "unit": unit, "basis": basis, "value": measured,
                     "reason_code": reason, "nyquist_hz": nyquist})
    return rows


def _profile_facts(value: Any, references: int) -> dict:
    if not isinstance(value, dict) or _count(value.get("references"), maximum=5) != references:
        raise InterpretationError("The runtime returned an incomplete reference profile.")
    quantities = value.get("quantities")
    if not isinstance(quantities, list) or not quantities or len(quantities) > len(_PROFILE_QUANTITIES):
        raise InterpretationError("The runtime returned invalid profile quantities.")
    rows, seen = [], set()
    usable = _count(value.get("usable_references"), maximum=references)
    for row in quantities:
        key = row.get("quantity") if isinstance(row, dict) else None
        if not isinstance(key, str) or key not in _PROFILE_QUANTITIES or key in seen:
            raise InterpretationError("The runtime returned an unknown or repeated profile quantity.")
        seen.add(key)
        unit, basis = _PROFILE_QUANTITIES[key]
        if row.get("unit") != unit or row.get("basis") != basis or _count(row.get("references")) != references:
            raise InterpretationError("The runtime returned an unsupported profile measurement basis.")
        target = row.get("target")
        target = None if target is None else _numbers(target, ("mid", "low", "high"))
        source, deviation = _number(row.get("source")), _number(row.get("deviation"))
        contributors = _count(row.get("contributors"), maximum=usable)
        source_reason, target_reason = _reason(row.get("source_reason")), _reason(row.get("target_reason"))
        source_nyquist = _nyquist(row.get("source_nyquist_hz"), source_reason)
        inside = row.get("inside_spread")
        if (target is not None and (set(target) != {"mid", "low", "high"} or any(v is None for v in target.values())
                                   or not target["low"] <= target["mid"] <= target["high"])):
            raise InterpretationError("The runtime returned an invalid profile spread.")
        available = source is not None and target is not None
        if ((source is None) != (source_reason is not None) or (target is None) != (target_reason is not None)
                or (target is None) != (contributors == 0) or (deviation is not None) != available
                or (type(inside) is bool) != available or (not available and inside is not None)):
            raise InterpretationError("The runtime returned inconsistent profile availability.")
        rows.append({"quantity": key, "unit": unit, "basis": basis, "source": source, "source_reason": source_reason,
                     "source_nyquist_hz": source_nyquist, "target": target, "target_reason": target_reason,
                     "contributors": contributors, "references": references, "deviation": deviation, "inside_spread": inside})
    return {"basis": "Real Heard/Profile on both sides; equal per-original-reference weighting, not canonical Analysis",
            "references": references, "usable_references": usable, "source_window": _sample_window(value.get("source_window")),
            "quantities": rows}


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
    references = result.get("references")
    if references is not None:
        if (comparison is not None or not isinstance(references, list) or not 1 <= len(references) <= 5
                or any(not isinstance(reference, dict) for reference in references)):
            raise InterpretationError("The runtime returned an invalid reference set.")
        for index, reference in enumerate(references, 1):
            analyses.append((f"reference_{index}", f"references[{index - 1}].metrics",
                             f"references[{index - 1}].spectrogram", reference.get("metrics")))
    profile = result.get("reference_profile")
    if (profile is None) != (references is None):
        raise InterpretationError("The runtime returned an incomplete reference profile.")
    exact_windows = profile is not None or (isinstance(comparison, dict) and comparison.get("window") is not None)
    supplied = []
    blocks = []
    for label, metrics_input, spectrum_input, analysis in analyses:
        measured = _measurements(analysis)
        supplied.append(metrics_input)
        reference = comparison if label == "reference" else (
            references[int(label.removeprefix("reference_")) - 1] if label.startswith("reference_") else None)
        if reference is not None:
            if reference.get("window") is not None:
                measured["exactWindow"] = _sample_window(reference["window"])
            if "availability" in reference:
                measured["availability"] = _availability(reference["availability"])
            if label.startswith("reference_"):
                measured["profile_quantities"] = _member_profile_facts(reference.get("profile_quantities"))
                supplied.append(f"references[{int(label.removeprefix('reference_')) - 1}].profile_quantities")
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
    if profile is not None:
        facts = _profile_facts(profile, len(references))
        supplied.append("reference_profile")
        blocks.append({"type": "text", "text": json.dumps({"reference_profile": facts},
                                                           allow_nan=False, separators=(",", ":"))})
    try:
        completed = llm.complete_structured(
            task=AUX_TASK_KEY,
            purpose="kumi.listen.interpret",
            instructions=_INSTRUCTIONS + (_WINDOW_INSTRUCTIONS if exact_windows else "")
                         + (_PROFILE_INSTRUCTIONS if profile is not None else ""),
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
