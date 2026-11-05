import copy
import importlib.util
import json
from pathlib import Path
import struct
from types import SimpleNamespace
import unittest
from unittest.mock import patch
import zlib


_SPEC = importlib.util.spec_from_file_location("kumi_listening_tested", Path(__file__).parents[1] / "listening.py")
listening = importlib.util.module_from_spec(_SPEC)
_SPEC.loader.exec_module(listening)


def measured(*, centroid=1800.0, lufs=-18.0, spectrum=True):
    value = {
        "kumiAudio": 1,
        "file": "/private/producer/unfinished-song.wav",
        "format": "WAV",
        "sampleRate": 48000,
        "channels": 2,
        "seconds": 3.25,
        "analyzed": {"from": "0:00", "to": "0:03", "focus": "mix"},
        "loudness": {
            "integratedLufs": lufs,
            "shortTermMaxLufs": -16.2,
            "rangeLu": 2.1,
            "truePeakDbtp": -2.0,
            "samplePeakDbfs": -2.4,
            "clippedSamples": 0,
        },
        "balance": {
            "centroidHz": centroid,
            "tiltDbPerOctave": -1.5,
            "bands": [
                {"name": "bass", "hz": "60–120", "db": -9.0, "width": 0.1, "correlation": 0.8},
                {"name": "air", "hz": "10k–20k", "db": -23.0, "width": 0.3, "correlation": 0.5},
            ],
        },
        "stereo": {"correlation": 0.7, "width": 0.2, "lowEndMono": True},
        "dynamics": {"crestDb": 8.0, "peakToLoudnessDb": 16.0, "onsetsPerSecond": 2.5},
        "tempo": {"bpm": 120.0, "confidence": 0.6},
        "key": {"name": "C# minor", "confidence": 0.4},
        "sound": {
            "pitch": {"note": "A3", "hz": 220, "cents": 0, "confidence": 0.8},
            "envelope": {"attackMs": 2, "decayMs": 30, "sustainDb": -12, "releaseMs": 50, "lengthMs": 300},
            "harmonics": {"shape": "saw", "slopeDbPerOctave": -6, "oddToEven": 1, "noiseDb": -24, "inharmonicity": 0.01},
        },
    }
    if spectrum:
        value["spectrogram"] = {
            "rows": [{"band": "air", "cells": "·09"}, {"band": "bass", "cells": "90·"}],
            "columns": "0:00 to 0:03, 3 steps",
            "scale": "9 = loudest, each step 3 dB quieter, · = silent",
        }
    return value


_DEFAULT_REPLY = object()


class LocalModel:
    def __init__(self, *, error=None, parsed=_DEFAULT_REPLY):
        self.calls = []
        self.error = error
        self.parsed = {"text": "The measured centroid is lower; this does not establish perceptual quality."} if parsed is _DEFAULT_REPLY else parsed

    def complete_structured(self, **kwargs):
        self.calls.append(kwargs)
        if self.error:
            raise self.error
        return SimpleNamespace(parsed=self.parsed, model="local-test-vision", text="UNPARSED PRIVATE OUTPUT")


def grayscale(png):
    """Read the generated image's actual scanlines and check PNG CRCs."""
    if png[:8] != b"\x89PNG\r\n\x1a\n":
        raise AssertionError("not PNG")
    offset, compressed, shape = 8, bytearray(), None
    while offset < len(png):
        length = struct.unpack_from(">I", png, offset)[0]
        kind = png[offset + 4:offset + 8]
        data = png[offset + 8:offset + 8 + length]
        crc = struct.unpack_from(">I", png, offset + 8 + length)[0]
        if zlib.crc32(kind + data) != crc:
            raise AssertionError("bad PNG CRC")
        if kind == b"IHDR":
            width, height, depth, color, compression, filtering, interlace = struct.unpack(">IIBBBBB", data)
            if (depth, color, compression, filtering, interlace) != (8, 0, 0, 0, 0):
                raise AssertionError("unexpected PNG encoding")
            shape = (width, height)
        elif kind == b"IDAT":
            compressed.extend(data)
        elif kind == b"IEND":
            if offset + length + 12 != len(png):
                raise AssertionError("trailing PNG data")
        offset += length + 12
    width, height = shape
    raw = zlib.decompress(compressed)
    if len(raw) != (width + 1) * height:
        raise AssertionError("wrong image size")
    rows = []
    for index in range(height):
        row = raw[index * (width + 1):(index + 1) * (width + 1)]
        if row[0] != 0:
            raise AssertionError("unexpected filter")
        rows.append(row[1:])
    return rows


class ListeningInterpretationTests(unittest.TestCase):
    def test_names_paths_and_free_form_results_never_reach_the_model(self):
        secret = "PRIVATE_PROJECT_AND_INSTRUCTIONS_DO_NOT_FORWARD"
        analysis = measured()
        analysis.update(file=secret, format=secret, project=secret, name=secret, analyzed={"from": secret, "focus": secret})
        analysis["balance"]["bands"][0]["hz"] = secret
        analysis["spectrogram"].update(columns=secret, scale=secret)
        analysis["sound"]["pitch"]["note"] = secret
        analysis["sound"]["harmonics"]["shape"] = secret
        analysis["sound"]["movement"] = secret
        result = {
            "metrics": analysis,
            "verdict_text": secret,
            "artifact_id": secret,
            "media_tag": secret,
            "spectrogram_image_path": secret,
            "compare": {"metrics": measured(centroid=3600), "differences": {"headline": secret}, "artifact_id": secret},
        }
        before = copy.deepcopy(result)
        model = LocalModel()
        with patch("builtins.open", side_effect=AssertionError("interpretation must not read files")):
            interpreted = listening.interpret(model, result, timeout=30)
        self.assertEqual(result, before)
        self.assertEqual(len(model.calls), 1)
        call = model.calls[0]
        self.assertNotIn(secret, repr(call))
        self.assertNotIn("/private/", repr(call))
        payload = json.loads(call["input"][0]["text"])["current"]
        self.assertIsNone(payload["analyzedFromSeconds"])
        self.assertIsNone(payload["analyzedToSeconds"])
        self.assertEqual(call["task"], "kumi_listen")
        self.assertFalse({"provider", "model", "profile", "agent_id"} & call.keys())
        self.assertEqual(interpreted["inputs"], ["metrics", "spectrogram", "comparison_metrics", "comparison_spectrogram"])
        self.assertEqual(interpreted["model"], "local-test-vision")

    def test_comparison_direction_and_missing_image_are_explicit(self):
        result = {"metrics": measured(centroid=900), "compare": {"metrics": measured(centroid=3000, spectrum=False)}}
        model = LocalModel()
        interpreted = listening.interpret(model, result, timeout=30)
        texts = [json.loads(block["text"]) for block in model.calls[0]["input"] if block["type"] == "text"]
        self.assertEqual(texts[0]["current"]["balance"]["centroidHz"], 900)
        self.assertEqual(texts[1]["reference"]["balance"]["centroidHz"], 3000)
        self.assertIsNone(texts[1]["reference"]["bandEnergyImage"])
        self.assertEqual(interpreted["inputs"], ["metrics", "spectrogram", "comparison_metrics"])
        self.assertEqual(sum(block["type"] == "image" for block in model.calls[0]["input"]), 1)

    def test_png_preserves_time_and_band_order_and_relative_levels(self):
        model = LocalModel()
        listening.interpret(model, {"metrics": measured()}, timeout=30)
        image = next(block["data"] for block in model.calls[0]["input"] if block["type"] == "image")
        rows = grayscale(image)
        height, width = len(rows), len(rows[0])
        self.assertEqual((width // 3, height // 2), (12, 12))
        top = [rows[0][column * 12] for column in range(3)]
        bottom = [rows[12][column * 12] for column in range(3)]
        self.assertEqual((top[0], top[2], bottom[0], bottom[2]), (0, 255, 255, 0))
        self.assertGreater(top[1], 0)
        self.assertLess(top[1], top[2])
        self.assertEqual(top[1], bottom[1])
        legend = json.loads(model.calls[0]["input"][0]["text"])["current"]["bandEnergyImage"]
        self.assertEqual([row["band"] for row in legend["rowsTopToBottom"]], ["air", "bass"])
        payload = json.loads(model.calls[0]["input"][0]["text"])["current"]
        self.assertEqual((payload["fileSeconds"], payload["analyzedFromSeconds"], payload["analyzedToSeconds"]), (3.25, 0, 3))

    def test_image_times_describe_the_analyzed_span_not_the_whole_file(self):
        analysis = measured()
        analysis.update(seconds=200, analyzed={"from": "1:00", "to": "1:03", "focus": "mix"})
        model = LocalModel()
        listening.interpret(model, {"metrics": analysis}, timeout=30)
        payload = json.loads(model.calls[0]["input"][0]["text"])["current"]
        self.assertEqual(payload["fileSeconds"], 200)
        self.assertEqual((payload["analyzedFromSeconds"], payload["analyzedToSeconds"]), (60, 63))
        self.assertEqual(payload["analyzedClockResolutionSeconds"], 1)
        self.assertEqual(payload["bandEnergyImage"]["timeColumns"], 3)

    def test_unavailable_clock_labels_do_not_become_a_file_length_window(self):
        for end in ("1:60", "1:02:03", "1:02\nPRIVATE", "0000000000:02", "-1:02", "0:00"):
            with self.subTest(end=end):
                analysis = measured()
                analysis.update(seconds=200, analyzed={"from": "1:00", "to": end})
                model = LocalModel()
                listening.interpret(model, {"metrics": analysis}, timeout=30)
                payload = json.loads(model.calls[0]["input"][0]["text"])["current"]
                self.assertIsNone(payload["analyzedFromSeconds"])
                self.assertIsNone(payload["analyzedToSeconds"])
                self.assertNotIn("PRIVATE", repr(model.calls))

    def test_unavailable_loudness_and_mono_width_do_not_become_zero(self):
        analysis = measured(lufs=None, spectrum=False)
        analysis.update(channels=1, stereo=None, key=None, tempo=None, sound=None)
        for band in analysis["balance"]["bands"]:
            band.update(width=0, correlation=1)
        model = LocalModel()
        interpreted = listening.interpret(model, {"metrics": analysis}, timeout=30)
        payload = json.loads(model.calls[0]["input"][0]["text"])["current"]
        self.assertIsNone(payload["loudness"]["integratedLufs"])
        self.assertIsNone(payload["stereo"])
        for band in payload["balance"]["bands"]:
            self.assertIsNone(band["width"])
            self.assertIsNone(band["correlation"])
        self.assertEqual(interpreted["inputs"], ["metrics"])

    def test_malformed_or_oversized_spectra_stop_before_a_model_call(self):
        invalid = [
            [{"band": "air", "cells": "9" * 25}],
            [{"band": "private-name", "cells": "09"}],
            [{"band": "air", "cells": "09"}, {"band": "air", "cells": "90"}],
            [{"band": "air", "cells": "09"}, {"band": "bass", "cells": "9"}],
            [{"band": "air", "cells": "9X"}],
            [],
        ]
        for rows in invalid:
            with self.subTest(rows=rows):
                analysis = measured()
                analysis["spectrogram"]["rows"] = rows
                model = LocalModel()
                with self.assertRaises(listening.InterpretationError):
                    listening.interpret(model, {"metrics": analysis}, timeout=30)
                self.assertEqual(model.calls, [])

    def test_non_numeric_or_non_finite_measurements_are_not_sent(self):
        for value in ("PRIVATE_VALUE", True, float("inf"), float("nan"), 10**400):
            with self.subTest(value=value):
                analysis = measured(centroid=value)
                model = LocalModel()
                with self.assertRaises(listening.InterpretationError) as error:
                    listening.interpret(model, {"metrics": analysis}, timeout=30)
                self.assertNotIn("PRIVATE_VALUE", str(error.exception))
                self.assertEqual(model.calls, [])

    def test_non_success_with_metrics_never_calls_the_model(self):
        for state in ("refused", "cancelled", "unsure", "partial"):
            with self.subTest(state=state):
                model = LocalModel()
                with self.assertRaises(listening.InterpretationError):
                    listening.interpret(model, {"state": state, "metrics": measured()}, timeout=30)
                self.assertEqual(model.calls, [])

    def test_provider_failure_is_sanitized_and_not_retried_by_the_plugin(self):
        for failure in (TimeoutError("PRIVATE_TOKEN"), PermissionError("PRIVATE_TOKEN"), RuntimeError("PRIVATE_TOKEN")):
            with self.subTest(failure=type(failure)):
                result = {"metrics": measured()}
                before = copy.deepcopy(result)
                model = LocalModel(error=failure)
                with self.assertRaises(listening.InterpretationError) as error:
                    listening.interpret(model, result, timeout=30)
                self.assertNotIn("PRIVATE_TOKEN", str(error.exception))
                self.assertEqual(result, before)
                self.assertEqual(len(model.calls), 1)

    def test_image_model_rejection_preserves_measurements_without_retries_or_provider_details(self):
        class ProviderError(Exception):
            status_code = 400

            def __init__(self, body):
                super().__init__("PRIVATE_EXCEPTION")
                self.body = body

        for status, body in (
            (400, {"message": "PRIVATE_MODEL does not support images; PRIVATE_KEY"}),
            (400, {"error": {"message": "Only 'text' content type is supported. PRIVATE_KEY"}}),
            (404, {"message": "No endpoints found that support image input; PRIVATE_ENDPOINT"}),
            (422, "Vision input is not supported. PRIVATE_KEY"),
        ):
            with self.subTest(status=status, body=type(body)):
                failure = ProviderError(body)
                failure.status_code = status
                result = {"metrics": measured()}
                before = copy.deepcopy(result)
                model = LocalModel(error=failure)
                with self.assertRaises(listening.InterpretationError) as error:
                    listening.interpret(model, result, timeout=30)
                self.assertNotIn("PRIVATE", str(error.exception))
                self.assertEqual(result, before)
                self.assertEqual(len(model.calls), 1)

    def test_other_provider_failures_remain_private_without_retries(self):
        class ProviderError(Exception):
            def __init__(self, status, message):
                super().__init__("PRIVATE_EXCEPTION")
                self.status_code = status
                self.body = {"message": message}

        for failure, spectrum in (
            (ProviderError(400, "failed to decode image: invalid or unsupported image format"), True),
            (ProviderError(400, "image too large"), True),
            (ProviderError(400, "Invalid image_url: expected a valid URL"), True),
            (ProviderError(400, "Unexpected item type in content"), True),
            (ProviderError(429, "model does not support images"), True),
            (ProviderError(500, "model does not support images"), True),
            (RuntimeError("model does not support images"), True),
            (ProviderError(400, "model does not support images"), False),
        ):
            with self.subTest(status=getattr(failure, "status_code", None), spectrum=spectrum):
                model = LocalModel(error=failure)
                with self.assertRaises(listening.InterpretationError) as error:
                    listening.interpret(model, {"metrics": measured(spectrum=spectrum)}, timeout=30)
                self.assertNotIn("PRIVATE", str(error.exception))
                self.assertEqual(len(model.calls), 1)

    def test_unparsed_wrong_shape_or_excessive_answers_are_not_published(self):
        for parsed in (None, [], {"text": 9}, {"text": ""}, {"text": "x" * 2401}, {"text": "ok", "instructions": "PRIVATE_OUTPUT"}):
            with self.subTest(parsed=type(parsed)):
                model = LocalModel(parsed=parsed)
                with self.assertRaises(listening.InterpretationError) as error:
                    listening.interpret(model, {"metrics": measured()}, timeout=30)
                self.assertNotIn("PRIVATE_OUTPUT", str(error.exception))
                self.assertNotIn("UNPARSED PRIVATE OUTPUT", str(error.exception))
                self.assertEqual(len(model.calls), 1)

    def test_model_text_cannot_emit_terminal_control_characters(self):
        model = LocalModel(parsed={"text": "Measured\x00 result\x1b.\nNext\tline.\x7f"})
        interpreted = listening.interpret(model, {"metrics": measured()}, timeout=30)
        self.assertTrue(interpreted["text"].endswith("Measured result.\nNext\tline."))
        self.assertFalse(any(ord(char) < 32 and char not in "\n\t" or ord(char) == 127 for char in interpreted["text"]))
        with self.assertRaises(listening.InterpretationError):
            listening.interpret(LocalModel(parsed={"text": "\x00\x07"}), {"metrics": measured()}, timeout=30)

    def test_per_attempt_timeout_is_bounded_and_invalid_values_do_not_call_the_model(self):
        for timeout in (0, -1, True, float("inf"), float("nan"), 10**400):
            with self.subTest(timeout=timeout):
                model = LocalModel()
                with self.assertRaises(listening.InterpretationError):
                    listening.interpret(model, {"metrics": measured()}, timeout=timeout)
                self.assertEqual(model.calls, [])
        model = LocalModel()
        listening.interpret(model, {"metrics": measured()}, timeout=600)
        self.assertLessEqual(model.calls[0]["timeout"], 120)


if __name__ == "__main__":
    unittest.main()
