"""aiff.py: AIFF and AIFF-C as WAV, so `/audio`'s AIFF sounds play in Hermes Desktop (Chromium plays no AIFF)."""

from __future__ import annotations

import importlib.util
import struct
import sys
import unittest
from pathlib import Path

HERE = Path(__file__).resolve().parent


def _load(name: str, path: Path):
    spec = importlib.util.spec_from_file_location(name, path)
    module = importlib.util.module_from_spec(spec)
    sys.modules[name] = module
    spec.loader.exec_module(module)
    return module


aiff = _load("kumi_test_aiff", HERE.parent / "aiff.py")


def extended(rate: int) -> bytes:
    """A whole sample rate as COMM's 80-bit extended float."""
    if rate == 0:
        return bytes(10)
    exponent = rate.bit_length() - 1
    return struct.pack(">HQ", 16383 + exponent, rate << (63 - exponent))


def chunk(kind: bytes, body: bytes) -> bytes:
    return kind + struct.pack(">I", len(body)) + body + (b"\x00" if len(body) & 1 else b"")


def make(sound: bytes, *, channels: int, bits: int, frames: int, rate: int = 44100, compression: bytes | None = None,
         offset: int = 0, before: bytes = b"") -> bytes:
    """An AIFF file (AIFF-C when `compression` is given), with `before` chunks ahead of COMM."""
    comm = struct.pack(">hIh", channels, frames, bits) + extended(rate)
    if compression is not None:
        comm += compression + b"\x04none"  # a Pascal-string name, padded to even
    ssnd = struct.pack(">II", offset, 0) + bytes(offset) + sound
    body = (b"AIFC" if compression is not None else b"AIFF") + before + chunk(b"COMM", comm) + chunk(b"SSND", ssnd)
    return b"FORM" + struct.pack(">I", len(body)) + body


def wav(data: bytes) -> tuple[dict, bytes]:
    """A WAV file's format fields and sample bytes."""
    assert data[:4] == b"RIFF" and data[8:12] == b"WAVE", data[:12]
    assert struct.unpack("<I", data[4:8])[0] == len(data) - 8
    at, fmt, samples = 12, {}, None
    while at + 8 <= len(data):
        kind, size = data[at:at + 4], struct.unpack("<I", data[at + 4:at + 8])[0]
        body = data[at + 8:at + 8 + size]
        if kind == b"fmt ":
            tag, channels, rate, byte_rate, block, bits = struct.unpack("<HHIIHH", body[:16])
            fmt = {"tag": tag, "channels": channels, "rate": rate, "byte_rate": byte_rate, "block": block, "bits": bits}
            if tag == 0xFFFE:
                fmt["valid"], fmt["mask"], fmt["sub"] = struct.unpack("<xxHIH", body[16:26])
                fmt["guid_tail"] = body[26:40]
            elif len(body) > 16:
                fmt["cb"] = struct.unpack("<H", body[16:18])[0]
        elif kind == b"data":
            samples = body
        at += 8 + size + (size & 1)
    return fmt, samples


class Converts(unittest.TestCase):
    def test_16_bit_pcm_comes_out_as_little_endian_pcm_with_its_rate_and_channels(self) -> None:
        values = [0, 1, -1, 32767, -32768, 1234]
        out = aiff.to_wav(make(struct.pack(">6h", *values), channels=2, bits=16, frames=3, rate=48000))
        fmt, samples = wav(out)
        self.assertEqual(fmt, {"tag": 1, "channels": 2, "rate": 48000, "byte_rate": 48000 * 4, "block": 4, "bits": 16})
        self.assertEqual(samples, struct.pack("<6h", *values))

    def test_24_bit_is_extensible_with_its_bytes_reversed(self) -> None:
        values = [0, 1, -1, 8388607, -8388608]
        sound = b"".join(v.to_bytes(3, "big", signed=True) for v in values)
        fmt, samples = wav(aiff.to_wav(make(sound, channels=1, bits=24, frames=5)))
        self.assertEqual((fmt["tag"], fmt["bits"], fmt["valid"], fmt["sub"], fmt["block"], fmt["mask"]), (0xFFFE, 24, 24, 1, 3, 0x4))
        self.assertEqual(fmt["guid_tail"], aiff._GUID_TAIL)
        self.assertEqual(samples, b"".join(v.to_bytes(3, "little", signed=True) for v in values))

    def test_8_bit_signed_becomes_wavs_unsigned_and_an_odd_data_chunk_is_padded(self) -> None:
        out = aiff.to_wav(make(struct.pack(">5b", -128, -1, 0, 1, 127), channels=1, bits=8, frames=5))
        fmt, samples = wav(out)
        self.assertEqual((fmt["tag"], fmt["bits"]), (1, 8))
        self.assertEqual(samples, bytes([0, 127, 128, 129, 255]))
        self.assertEqual(len(out) % 2, 0)

    def test_aiff_c_little_endian_and_float_and_mu_law(self) -> None:
        values = [5, -5, 300, -300]
        _, samples = wav(aiff.to_wav(make(struct.pack("<4h", *values), channels=2, bits=16, frames=2, compression=b"sowt")))
        self.assertEqual(samples, struct.pack("<4h", *values))

        floats = [0.0, 0.5, -0.25, 1.0]
        fmt, samples = wav(aiff.to_wav(make(struct.pack(">4f", *floats), channels=1, bits=32, frames=4, compression=b"fl32")))
        self.assertEqual((fmt["tag"], fmt["sub"], fmt["bits"]), (0xFFFE, 3, 32))
        self.assertEqual(samples, struct.pack("<4f", *floats))

        fmt, samples = wav(aiff.to_wav(make(struct.pack(">4d", *floats), channels=2, bits=64, frames=2, compression=b"FL64")))
        self.assertEqual((fmt["sub"], fmt["bits"], fmt["block"]), (3, 32, 8))
        self.assertEqual(samples, struct.pack("<4f", *floats))

        law = bytes([0xFF, 0x7F, 0x00, 0x80])
        fmt, samples = wav(aiff.to_wav(make(law, channels=1, bits=16, frames=4, compression=b"ulaw")))
        self.assertEqual((fmt["tag"], fmt["bits"], fmt["block"], fmt["cb"]), (7, 8, 1, 0))
        self.assertEqual(samples, law)

    def test_the_chunk_walk_pads_odd_chunks_honours_the_offset_and_clamps_frames(self) -> None:
        values = [1, 2, 3, 4]
        # An odd-length chunk ahead of COMM, an SSND offset, and COMM claiming more frames than the file holds.
        data = make(struct.pack(">4h", *values), channels=1, bits=16, frames=1000, offset=6, before=chunk(b"NAME", b"abc"))
        _, samples = wav(aiff.to_wav(data))
        self.assertEqual(samples, struct.pack("<4h", *values))
        # A file cut off mid-sample keeps the whole frames it has.
        _, samples = wav(aiff.to_wav(data[:-3]))
        self.assertEqual(samples, struct.pack("<2h", 1, 2))

    def test_what_it_cant_convert_is_none(self) -> None:
        sound = struct.pack(">2h", 1, 2)
        good = make(sound, channels=1, bits=16, frames=2)
        self.assertIsNotNone(aiff.to_wav(good))
        for broken in (
            b"RIFF" + good[4:],  # not AIFF at all
            good[:30],  # COMM cut short
            make(sound, channels=1, bits=16, frames=2, rate=0),
            make(sound, channels=1, bits=16, frames=2, rate=10_000_000),
            good[:28] + b"\x7f\xff" + bytes(8) + good[38:],  # COMM's rate is NaN
            make(sound, channels=0, bits=16, frames=2),
            make(sound, channels=1, bits=40, frames=2),
            make(sound, channels=1, bits=16, frames=2, compression=b"ima4"),
        ):
            self.assertIsNone(aiff.to_wav(broken), broken[:40])


if __name__ == "__main__":
    unittest.main()
