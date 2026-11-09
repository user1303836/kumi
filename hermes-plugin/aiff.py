"""AIFF and AIFF-C as WAV, for `/audio`: Hermes Desktop is Electron, and Chromium plays no AIFF ("no supported source
was found"), while the same samples as WAV play. Only the bytes `/audio` hands out are converted; the file stays.

PCM (8 to 32 bits, either byte order), 32- and 64-bit float (64-bit comes out as 32-bit float, which WAV players take
everywhere), µ-law and A-law. A compressed AIFF-C (ima4, MAC3, …) or a file that doesn't parse is left as it is: the
browser then fails on it and the UI hides the sound as one that can't play here.

Standard library only, and fast enough for the 50 MiB a media route allows: 16- and 32-bit samples are byte-swapped
by `array`, 24-bit ones by extended-slice assignment, 8-bit ones by `bytes.translate`. `dashboard/plugin_api.py` loads
this file by path, so it imports nothing else of the plugin.
"""

from __future__ import annotations

import math
import struct
import sys
from array import array

# A sample rate past this (or under the lowest) isn't a recording's: the COMM chunk is garbage.
_RATE_MIN = 1_000
_RATE_MAX = 1_536_000
_CHANNELS_MAX = 64

_PCM, _FLOAT, _ALAW, _ULAW, _EXTENSIBLE = 1, 3, 6, 7, 0xFFFE
# KSDATAFORMAT_SUBTYPE_*: the format tag followed by this fixed GUID tail.
_GUID_TAIL = b"\x00\x00\x00\x00\x10\x00\x80\x00\x00\xaa\x00\x38\x9b\x71"
# WAV's 8-bit PCM is unsigned; AIFF's is signed.
_SIGNED_TO_UNSIGNED = bytes((i + 128) & 0xFF for i in range(256))
_LITTLE = sys.byteorder == "little"

# AIFF-C compression type → (how the samples are stored, bytes per sample or None for COMM's bits).
_KINDS: dict[bytes, tuple[str, int | None]] = {
    b"NONE": ("be", None),
    b"twos": ("be", None),
    b"sowt": ("le", None),
    b"raw ": ("offset", 1),
    b"in24": ("be", 3),
    b"in32": ("be", 4),
    b"23ni": ("le", 3),
    b"42ni": ("le", 4),
    b"fl32": ("f32", 4),
    b"FL32": ("f32", 4),
    b"fl64": ("f64", 8),
    b"FL64": ("f64", 8),
    b"ulaw": ("ulaw", 1),
    b"ULAW": ("ulaw", 1),
    b"alaw": ("alaw", 1),
    b"ALAW": ("alaw", 1),
}


def is_aiff(data: bytes) -> bool:
    return len(data) >= 12 and data[:4] == b"FORM" and data[8:12] in (b"AIFF", b"AIFC")


def _extended(raw: bytes) -> float:
    """An 80-bit IEEE 754 extended float (COMM's sample rate); NaN for an infinity or NaN."""
    exponent = ((raw[0] & 0x7F) << 8) | raw[1]
    mantissa = int.from_bytes(raw[2:10], "big")
    if exponent == 0x7FFF:
        return math.nan
    if exponent == 0 and mantissa == 0:
        return 0.0
    value = math.ldexp(mantissa, exponent - 16383 - 63)
    return -value if raw[0] & 0x80 else value


def _chunks(data: bytes) -> dict[bytes, tuple[int, int]]:
    """The first COMM and SSND chunks' bodies as `(start, end)`, `end` clamped to the bytes present (a truncated file
    still plays what it has). Chunks are padded to an even length."""
    form_end = min(len(data), 8 + int.from_bytes(data[4:8], "big"))
    found: dict[bytes, tuple[int, int]] = {}
    at = 12
    while at + 8 <= form_end and len(found) < 2:
        kind = data[at:at + 4]
        size = int.from_bytes(data[at + 4:at + 8], "big")
        start = at + 8
        if kind in (b"COMM", b"SSND") and kind not in found:
            found[kind] = (start, min(start + size, len(data)))
        at = start + size + (size & 1)
    return found


def to_wav(data: bytes) -> bytes | None:
    """`data` (an AIFF or AIFF-C file) as a WAV file, or None when it isn't one or isn't one this converts."""
    try:
        return _convert(data) if is_aiff(data) else None
    except (ValueError, OverflowError, struct.error, MemoryError):
        return None


def _convert(data: bytes) -> bytes | None:
    chunks = _chunks(data)
    if b"COMM" not in chunks or b"SSND" not in chunks:
        return None
    start, end = chunks[b"COMM"]
    aifc = data[8:12] == b"AIFC"
    if end - start < (22 if aifc else 18):
        return None
    channels, frames, bits = struct.unpack(">hIh", data[start:start + 8])
    rate = _extended(data[start + 8:start + 18])
    compression = data[start + 18:start + 22] if aifc else b"NONE"
    if compression not in _KINDS or not 0 < channels <= _CHANNELS_MAX or not _RATE_MIN <= rate <= _RATE_MAX:
        return None
    kind, width = _KINDS[compression]
    if width is None:
        if not 0 < bits <= 32:
            return None
        width = (bits + 7) // 8
    start, end = chunks[b"SSND"]
    if end - start < 8:
        return None
    offset = int.from_bytes(data[start:start + 4], "big")
    sound_start = start + 8 + offset
    block = channels * width
    frames = min(frames, max(0, end - sound_start) // block)
    samples = data[sound_start:sound_start + frames * block]

    tag, out_bits = _PCM, width * 8
    if kind == "be" or kind == "le":
        samples = _pcm(samples, width, little=kind == "le")
    elif kind == "offset":
        pass  # already WAV's unsigned 8-bit
    elif kind == "f32":
        samples, tag = _swapped(samples, "f"), _FLOAT
    elif kind == "f64":
        doubles = array("d")
        doubles.frombytes(samples)
        if _LITTLE:
            doubles.byteswap()
        floats = array("f", doubles)
        if not _LITTLE:
            floats.byteswap()
        samples, tag, out_bits = floats.tobytes(), _FLOAT, 32
    else:
        tag = _ULAW if kind == "ulaw" else _ALAW
    return _wav(samples, tag, channels, round(rate), out_bits)


def _swapped(samples: bytes, code: str) -> bytes:
    """Big-endian samples of an `array` type as little-endian bytes."""
    values = array(code)
    values.frombytes(samples)
    if _LITTLE:
        values.byteswap()
        return values.tobytes()
    return samples


def _pcm(samples: bytes, width: int, little: bool) -> bytes:
    """Signed PCM in either byte order as WAV's: little-endian, and unsigned at 8 bits."""
    if width == 1:
        return samples.translate(_SIGNED_TO_UNSIGNED)
    if little:
        return samples
    if width == 2:
        return _swapped(samples, "h")
    if width == 4:
        return _swapped(samples, "i" if array("i").itemsize == 4 else "l")
    if width == 3:
        out = bytearray(samples)
        out[0::3] = samples[2::3]
        out[2::3] = samples[0::3]
        return bytes(out)
    raise ValueError("unsupported sample width")


def _wav(samples: bytes, tag: int, channels: int, rate: int, bits: int) -> bytes:
    block = channels * bits // 8
    head = struct.pack("<HHIIHH", tag, channels, rate, rate * block, block, bits)
    if tag in (_PCM, _FLOAT) and (channels > 2 or bits > 16):
        # WAVE_FORMAT_EXTENSIBLE: what players expect past stereo or 16 bits. Speakers named for mono and stereo only.
        mask = {1: 0x4, 2: 0x3}.get(channels, 0)
        head = struct.pack("<HHIIHH", _EXTENSIBLE, channels, rate, rate * block, block, bits)
        head += struct.pack("<HHI", 22, bits, mask) + struct.pack("<H", tag) + _GUID_TAIL
    elif tag != _PCM:
        head += struct.pack("<H", 0)
    pad = b"\x00" if len(samples) & 1 else b""
    fmt = b"fmt " + struct.pack("<I", len(head)) + head
    size = 4 + len(fmt) + 8 + len(samples) + len(pad)
    return b"".join((b"RIFF", struct.pack("<I", size), b"WAVE", fmt, b"data", struct.pack("<I", len(samples)), samples, pad))
