"""CC-27 consumer boundaries: live availability, approval scope, retained media and numeric-only interpretation."""
from __future__ import annotations

import copy
import json
import threading
from types import SimpleNamespace
import unittest
from unittest import mock

import support
from test_listening import LocalModel, measured

runtime = support.module("runtime")
tools = support.module("tools")
policy = support.module("policy")
listening = support.module("listening")

SOURCE = {"artifact_id": "current"}
REFERENCE = {"artifact_id": "reference"}
PROJECT = {"expected_project_key": "live:night-drive", "expected_epoch": "1"}
FEATURES = {"kumi/listen_features": ["window", "references"]}


def window(start=42, seconds=2):
    return {"original_total_frames": 9_600_000, "original_seconds": 200,
            "original_duration_exact": True, "requested": {"from": start, "seconds": seconds},
            "start_frame": start * 48_000, "frame_count": seconds * 48_000,
            "sample_rate": 48_000, "channels": 1, "replay_from": 0}


def capture(**fields):
    return {"source": {"track": {"mix": True}, "from_beat": 8, "beats": 4}, **PROJECT, **fields}


class ReferenceCaller(unittest.TestCase):
    def connected(self, metadata=None, *options):
        extra = ("--listen-meta", json.dumps(metadata)) if metadata is not None else ()
        host, ctx, data = support.host(self, None, *extra, *options)
        connected = support.call(host, "kumi_connect", {}, session_id="s1")
        self.assertTrue(connected.get("connected"), connected)
        return host, ctx, data

    def test_handshake_requires_valid_metadata_and_never_infers_from_a_version(self):
        for metadata, extra, expected in (
            (FEATURES, (), {"window", "references"}),
            ({"kumi/listen_features": ["window", "unknown"]}, (), {"window"}),
            ({"kumi/listen_features": "references"}, (), set()),
            ({"kumi/listen_features": ["window", 1]}, (), set()),
            (["window", "references"], (), set()),
            (None, (), set()), (None, ("--no-tools-list",), set()),
        ):
            with self.subTest(metadata=metadata, extra=extra):
                host, _, data = self.connected(metadata, *extra)
                self.assertEqual(host.runtimes.get(data).listen_features, expected)
                messages = support.requests(data / "state")
                methods = [message["method"] for message in messages]
                self.assertLess(methods.index("notifications/initialized"), methods.index("tools/list"))
                host.runtimes.stop_all()
        duplicate = {"tools": [{"name": "listen", "_meta": FEATURES}] * 2}
        self.assertEqual(runtime.listen_features(duplicate), set())

    def test_hook_and_handler_reject_unadvertised_fields_before_any_native_call(self):
        host, _, data = self.connected()
        live = host.runtimes.get(data)
        cases = [{"source": SOURCE, "references": [REFERENCE]},
                 {"source": SOURCE, "compare_to": {**REFERENCE, "from": 1, "seconds": 2}},
                 capture(references=[REFERENCE])]
        with mock.patch.object(live, "call", side_effect=AssertionError("no native request is allowed")):
            for args in cases:
                with self.subTest(args=args):
                    decision = policy.pre_tool_call(host, tool_name="kumi_listen", args=args, session_id="s1")
                    self.assertEqual(decision["action"], "block")
                    self.assertIn("doesn't support", decision["message"])
                    self.assertEqual(tools.kumi_listen(host, args)["error"]["code"], "unsupported")
        host.runtimes.stop_all()
        self.assertEqual(support.handle(host, "kumi_listen", cases[0])["error"]["code"], "not_connected")

    def test_each_feature_is_independently_required_and_scheduled_calls_stay_offline(self):
        host, ctx, data = self.connected({"kumi/listen_features": ["references"]})
        args = {"source": SOURCE, "references": [{**REFERENCE, "from": 0, "seconds": 1}]}
        with mock.patch.object(host.runtimes.get(data), "call", side_effect=AssertionError("must refuse locally")):
            self.assertIn("reference excerpts", support.call(host, "kumi_listen", args)["blocked"])
            self.assertEqual(support.handle(host, "kumi_listen", args)["error"]["code"], "unsupported")
        offline, scheduled, state = support.host(self)
        scheduled.cron_execution = {"job_id": "nightly"}
        self.assertIn("blocked", support.call(offline, "kumi_listen", args))
        self.assertEqual(support.handle(offline, "kumi_listen", args)["error"]["code"], "scheduled")
        self.assertEqual(support.requests(state / "state"), [])

    def test_invalid_reference_sets_cannot_reach_a_capture_preview(self):
        host, _, data = self.connected(FEATURES)
        bad = [[], [REFERENCE] * 6, [REFERENCE, dict(REFERENCE)], [None], [{"track": {"mix": True}}],
               [{"file": "x", "artifact_id": "a"}], [{"sound_id": "s"}],
               [{**REFERENCE, "from": 0}], [{**REFERENCE, "seconds": 1}],
               [{**REFERENCE, "from": True, "seconds": 1}], [{**REFERENCE, "from": "0", "seconds": 1}],
               [{**REFERENCE, "from": -1, "seconds": 1}], [{**REFERENCE, "from": 0, "seconds": 0}],
               [{**REFERENCE, "from": 0, "seconds": 61}], [{**REFERENCE, "from": 0, "seconds": float("nan")}],
               [{**REFERENCE, "from": float("inf"), "seconds": 1}],
               [{**REFERENCE, "from": 1e308, "seconds": 1}],
               [{**REFERENCE, "from": 10 ** 1000, "seconds": 1}]]
        cases = [capture(references=references) for references in bad]
        cases += [capture(references=[REFERENCE], compare_to=REFERENCE),
                  capture(references=[REFERENCE], output="/private/not-an-output"),
                  {"source": {**SOURCE, "from": 0, "seconds": 1}, "references": [REFERENCE]}]
        with mock.patch.object(host.runtimes.get(data), "call", side_effect=AssertionError("invalid before preview")):
            for args in cases:
                with self.subTest(args=args):
                    self.assertEqual(policy.pre_tool_call(host, tool_name="kumi_listen", args=args)["action"], "block")
                    self.assertEqual(tools.kumi_listen(host, args)["error"]["code"], "invalid")
        self.assertIsNone(tools.listen_refusal(capture(references=[{**REFERENCE, "from": 0, "seconds": 60},
                                                                  {**REFERENCE, "from": 60, "seconds": 60}])))

    def test_capture_approval_names_all_excerpts_separately_and_keeps_one_preparation_budget(self):
        host, ctx, data = self.connected(FEATURES)
        ctx.surface = "desktop"
        args = capture(references=[{"artifact_id": f"r{index}", "from": index, "seconds": 2} for index in range(3)])
        preview = {"state": "preview", "text": "play the mix from beat 8 to beat 12; restore the transport",
                   "estimated_seconds": 2, "prepare_seconds": 120, "deadline_seconds": 75,
                   "reference_windows": [{"name": f"Reference {index}.wav", "window": window(index)} for index in range(3)]}
        live = host.runtimes.get(data)
        with mock.patch.object(live, "call", return_value=preview):
            decision = policy.pre_tool_call(host, tool_name="kumi_listen", args=args, session_id="s1", tool_call_id="p10")
        self.assertEqual(decision["action"], "approve")
        self.assertIn("beat 8 to beat 12", decision["message"].splitlines()[0])
        for index in range(3):
            self.assertIn(f'"Reference {index}.wav" — file excerpt from {index} s for 2 s', decision["message"])
        with mock.patch.object(live, "call", return_value={"state": "cancelled", "reason": "stopped"}) as called:
            support.handle(host, "kumi_listen", args, session_id="s1")
        self.assertEqual(called.call_count, 1)
        self.assertEqual(called.call_args.kwargs["timeout"], 120 + 75 + runtime.TRACK_MARGIN)
        self.assertEqual(called.call_args.kwargs["private"]["_listen"]["prepare_seconds"], 120)
        with mock.patch.object(live, "call", return_value={**preview, "reference_windows": preview["reference_windows"][:-1]}):
            self.assertEqual(policy.pre_tool_call(host, tool_name="kumi_listen", args=args)["action"], "block")

    def test_failed_file_profile_strips_all_measurement_and_media_facts(self):
        host, _, data = self.connected(FEATURES)
        failure = {"state": "cancelled", "reason": "stopped", **{key: "MEDIA:/private/secret" for key in tools.MEASURED}}
        with mock.patch.object(host.runtimes.get(data), "call", return_value=failure):
            answer = support.handle(host, "kumi_listen", {"source": SOURCE, "references": [REFERENCE]})
        self.assertEqual(answer, {"state": "cancelled", "reason": "stopped"})

    def test_reference_media_uses_its_own_retained_artifact_proof(self):
        host, _, _ = self.connected()
        kept = support.call(host, "kumi_listen", {"source": {"sound_id": "s-r1-1", "map_revision": "r1"}})
        result = {"references": [{"artifact_id": kept["artifact_id"], "media_tag": kept["media_tag"]},
                                 {"artifact_id": kept["artifact_id"], "media_tag": "MEDIA:/private/secret"}]}
        answer = tools.kept_media(host, result)
        self.assertEqual(answer["references"][0]["media_tag"], kept["media_tag"])
        self.assertNotIn("media_tag", answer["references"][1])
        self.assertIn("media_tag", result["references"][1], "the native result was not mutated")


class ReferenceLifecycle(unittest.TestCase):
    def test_started_closing_retired_and_late_callbacks_cannot_leave_an_extended_schema(self):
        host, ctx, data = support.host(self, None, "--listen-meta", json.dumps(FEATURES))
        callback = tools.handler("kumi_listen", tools.kumi_listen, host)
        check = lambda: True
        host.register_listen(callback, check, "meter")
        connected = support.call(host, "kumi_connect", {}, session_id="s1")
        self.assertTrue(connected.get("connected"), connected)
        extended = ctx.tools["kumi_listen"]
        self.assertIn("references", extended["schema"]["parameters"]["properties"])
        self.assertIs(extended["handler"], callback)
        self.assertIs(extended["check_fn"], check)
        self.assertEqual(extended["emoji"], "meter")
        live = host.runtimes.get(data)
        live.in_flight = 1  # settling is a distinct boundary: schema must reset before the runtime exits
        host.runtimes.stop(data)
        self.assertNotIn("references", ctx.tools["kumi_listen"]["schema"]["parameters"]["properties"])
        host.unload()
        before = len(ctx.tool_registrations)
        host.refresh_listen_schema(data)
        live.gone.wait(10)
        self.assertEqual(len(ctx.tool_registrations), before)

    def test_unload_disposes_a_refresh_overtaken_by_the_sdk_unload_snapshot(self):
        host, ctx, data = support.host(self, installed=False)
        entered, release = threading.Event(), threading.Event()
        handles = []
        class Handle:
            active = True
            def dispose(self):
                self.active = False
        original = ctx.register_tool
        def register(**kwargs):
            handle = Handle()
            handles.append(handle)
            if "references" in kwargs["schema"]["parameters"]["properties"]:
                entered.set()
                if not release.wait(10):
                    raise AssertionError("refresh did not get released")
            original(**kwargs)
            return handle
        ctx.register_tool = register
        host.register_listen(lambda *_: "", None, "meter")
        with mock.patch.object(host.runtimes, "get", return_value=SimpleNamespace(listen_features=frozenset(FEATURES["kumi/listen_features"]))):
            refresh = threading.Thread(target=host.refresh_listen_schema, args=(data,))
            refresh.start()
            self.assertTrue(entered.wait(10))
            unload = threading.Thread(target=host.unload)
            unload.start()
            release.set()
            refresh.join(10)
            unload.join(10)
        self.assertFalse(refresh.is_alive() or unload.is_alive())
        self.assertTrue(all(not handle.active for handle in handles))
        before = len(handles)
        host.refresh_listen_schema(data)
        self.assertEqual(len(handles), before)


def profile_result():
    availability = {"energy_zero": False, "profile_reason": None, "loud_frame_count": 4,
                    "fixed_region_tolerance": True,
                    "unavailable": [{"quantity": "sound.width", "reason": "mono"}]}
    refs = [{"metrics": measured(spectrum=False), "window": window(), "availability": copy.deepcopy(availability),
             "name": "PRIVATE_NAME", "identity": {"file": "/PRIVATE_SOURCE"}, "origin": {"name": "PRIVATE_ORIGIN"}}
            for _ in range(2)]
    members = [
        {"quantity": "integrated", "unit": "LUFS",
         "basis": "Heard BS.1770-4 integrated / Profile::integrated; distinct from canonical Analysis",
         "value": -16, "reason_code": None, "reason": "PRIVATE_MEMBER_DETAIL", "nyquist_hz": None},
        {"quantity": "low_width", "unit": "dB",
         "basis": "Heard side/mid power below 120 Hz / Profile::low_width; not canonical full-band width",
         "value": None, "reason_code": "mono", "reason": "PRIVATE_MEMBER_DETAIL", "nyquist_hz": None},
    ]
    for reference in refs:
        reference["profile_quantities"] = copy.deepcopy(members)
    refs[1]["profile_quantities"][0].update(value=None, reason_code="too_short_for_loudness")
    refs[1]["metrics"]["loudness"]["integratedLufs"] = None
    refs[1]["availability"]["unavailable"].append({"quantity": "integrated", "reason": "too_short_for_loudness"})
    return {"metrics": measured(spectrum=False), "references": refs,
            "reference_profile": {"basis": "PRIVATE_BASIS", "range_note": "PRIVATE_NOTE", "peak_note": "PRIVATE_PEAK",
                "references": 2, "usable_references": 2, "source_window": window(0), "quantities": [
                    {"quantity": "integrated", "unit": "LUFS",
                     "basis": "Heard BS.1770-4 integrated / Profile::integrated; distinct from canonical Analysis",
                     "source": -14, "source_reason": None, "target": {"mid": -16, "low": -17, "high": -15},
                     "target_reason": None, "contributors": 1, "references": 2, "deviation": 2, "inside_spread": False},
                    {"quantity": "low_width", "unit": "dB",
                     "basis": "Heard side/mid power below 120 Hz / Profile::low_width; not canonical full-band width",
                     "source": None, "source_reason": "mono", "target": None,
                     "target_reason": "no_reference_measurement", "contributors": 0, "references": 2,
                     "deviation": None, "inside_spread": None}]}}


class ProfileInterpretation(unittest.TestCase):
    def test_only_numeric_profile_facts_reach_the_existing_single_auxiliary_call(self):
        result, model = profile_result(), LocalModel()
        result["metrics"]["channels"] = 1
        for reference in result["references"]:
            reference["metrics"]["channels"] = 1
        before = copy.deepcopy(result)
        with mock.patch("builtins.open", side_effect=AssertionError("no source audio/file reads")):
            reading = listening.interpret(model, result, timeout=30)
        self.assertEqual(len(model.calls), 1)
        self.assertNotIn("PRIVATE", repr(model.calls))
        facts = json.loads(model.calls[0]["input"][-1]["text"])["reference_profile"]
        lufs, width = facts["quantities"]
        self.assertEqual((lufs["source"], lufs["contributors"], lufs["references"]), (-14, 1, 2))
        self.assertNotEqual(lufs["source"], result["metrics"]["loudness"]["integratedLufs"])
        self.assertEqual((width["source"], width["target"], width["deviation"], width["contributors"]), (None, None, None, 0))
        self.assertEqual(width["source_reason"], "mono")
        reference = json.loads(model.calls[0]["input"][1]["text"])["reference_1"]
        self.assertIsNone(reference["stereo"], "mono never acquires width from another profile")
        self.assertEqual(reference["profile_quantities"][0]["value"], -16)
        self.assertNotEqual(reference["profile_quantities"][0]["value"], reference["loudness"]["integratedLufs"])
        self.assertIn("NOT confidence intervals", model.calls[0]["instructions"])
        self.assertIn("reference_profile", reading["inputs"])
        self.assertEqual(result, before)

    def test_single_excerpt_keeps_exact_window_and_replay_zero_without_names_or_origin(self):
        model = LocalModel()
        result = {"metrics": measured(spectrum=False), "compare": {
            "metrics": measured(spectrum=False), "name": "PRIVATE", "window": window(), "origin": {"file": "PRIVATE"}}}
        listening.interpret(model, result, timeout=30)
        reference = json.loads(model.calls[0]["input"][1]["text"])["reference"]
        self.assertEqual(reference["exactWindow"]["requested"], {"from": 42, "seconds": 2})
        self.assertEqual(reference["exactWindow"]["replay_from"], 0)
        self.assertNotIn("PRIVATE", repr(model.calls))

    def test_silent_reference_remains_unusable_with_reason_and_reduced_coverage(self):
        result, model = profile_result(), LocalModel()
        result["references"][1]["availability"].update(
            energy_zero=True, profile_reason="zero_energy", fixed_region_tolerance=False,
            unavailable=[{"quantity": "regions", "reason": "zero_energy"}])
        for quantity in result["references"][1]["profile_quantities"]:
            quantity.update(value=None, reason_code="zero_energy")
        result["reference_profile"]["usable_references"] = 1
        listening.interpret(model, result, timeout=30)
        silent = json.loads(model.calls[0]["input"][2]["text"])["reference_2"]["availability"]
        self.assertEqual((silent["energy_zero"], silent["profile_reason"]), (True, "zero_energy"))
        profile = json.loads(model.calls[0]["input"][-1]["text"])["reference_profile"]
        self.assertEqual((profile["usable_references"], profile["references"]), (1, 2))
        self.assertEqual(profile["quantities"][0]["contributors"], 1)

    def test_malformed_coverage_or_a_peak_target_never_reaches_the_model(self):
        for change in (lambda p: p["quantities"][0].update(contributors=3),
                       lambda p: p["quantities"][1].update(source=0),
                       lambda p: p["quantities"][0].update(basis="PRIVATE"),
                       lambda p: p["quantities"][0].update(quantity="true_peak"),
                       lambda p: p["quantities"][0]["target"].update(mid=float("nan")),
                       lambda p: p["quantities"][0].update(source=None, source_reason="above_nyquist",
                                                           source_nyquist_hz=float("nan"), deviation=None,
                                                           inside_spread=None)):
            result, model = profile_result(), LocalModel()
            change(result["reference_profile"])
            with self.assertRaises(listening.InterpretationError):
                listening.interpret(model, result, timeout=30)
            self.assertEqual(model.calls, [])

    def test_missing_or_inconsistent_member_profile_values_never_reach_the_model(self):
        for change in (lambda ref: ref.pop("profile_quantities"),
                       lambda ref: ref["profile_quantities"][0].update(value=None),
                       lambda ref: ref["profile_quantities"][1].update(value=0),
                       lambda ref: ref["profile_quantities"][0].update(quantity="true_peak"),
                       lambda ref: ref["profile_quantities"][0].update(reason_code="PRIVATE_INSTRUCTIONS"),
                       lambda ref: ref["profile_quantities"][0].update(value=None, reason_code="above_nyquist"),
                       lambda ref: ref["profile_quantities"][0].update(value=None, reason_code="partial_frequency_coverage",
                                                                      nyquist_hz=-1)):
            result, model = profile_result(), LocalModel()
            change(result["references"][0])
            with self.assertRaises(listening.InterpretationError):
                listening.interpret(model, result, timeout=30)
            self.assertEqual(model.calls, [])

    def test_nyquist_mask_preserves_null_members_other_usable_bands_and_actual_contributors(self):
        for reason in ("above_nyquist", "partial_frequency_coverage"):
            with self.subTest(reason=reason):
                result, model = profile_result(), LocalModel()
                for index, reference in enumerate(result["references"]):
                    reference["profile_quantities"].extend([
                        {"quantity": "region/air", "unit": "dB",
                         "basis": "Heard 31-third-octave relative band power / Profile::regions",
                         "value": -20 if index == 0 else None, "reason_code": None if index == 0 else reason,
                         "reason": "PRIVATE_NYQUIST_DETAIL", "nyquist_hz": None if index == 0 else 11025},
                        {"quantity": "region/bass", "unit": "dB",
                         "basis": "Heard 31-third-octave relative band power / Profile::regions",
                         "value": -6 - index, "reason_code": None, "reason": None, "nyquist_hz": None},
                    ])
                low = result["references"][1]
                low["window"].update(sample_rate=22050, start_frame=42 * 22050, frame_count=2 * 22050)
                low["metrics"]["sampleRate"] = 22050
                low["availability"]["unavailable"].append({"quantity": "region/air", "reason": reason})
                profile = result["reference_profile"]
                profile["source_window"]["sample_rate"] = 22050
                profile["quantities"].extend([
                    {"quantity": "region/air", "unit": "dB",
                     "basis": "Heard 31-third-octave relative band power / Profile::regions",
                     "source": None, "source_reason": reason, "source_reason_detail": "PRIVATE_SOURCE_DETAIL",
                     "source_nyquist_hz": 11025, "target": {"mid": -20, "low": -21.5, "high": -18.5},
                     "target_reason": None, "contributors": 1, "references": 2,
                     "deviation": None, "inside_spread": None},
                    {"quantity": "region/bass", "unit": "dB",
                     "basis": "Heard 31-third-octave relative band power / Profile::regions",
                     "source": -6, "source_reason": None, "source_nyquist_hz": None,
                     "target": {"mid": -6.5, "low": -8, "high": -5}, "target_reason": None,
                     "contributors": 2, "references": 2, "deviation": 0.5, "inside_spread": True},
                ])
                listening.interpret(model, result, timeout=30)
                self.assertEqual(len(model.calls), 1)
                self.assertNotIn("PRIVATE", repr(model.calls))
                member = json.loads(model.calls[0]["input"][2]["text"])["reference_2"]
                air, bass = member["profile_quantities"][-2:]
                self.assertEqual((air["value"], air["reason_code"], air["nyquist_hz"]), (None, reason, 11025))
                self.assertEqual(bass["value"], -7, "the low-rate member retains real low-band evidence")
                self.assertEqual(member["availability"]["unavailable"][-1]["reason"], reason)
                combined = json.loads(model.calls[0]["input"][-1]["text"])["reference_profile"]
                air_target, bass_target = combined["quantities"][-2:]
                self.assertEqual((air_target["source"], air_target["source_reason"], air_target["source_nyquist_hz"]),
                                 (None, reason, 11025))
                self.assertEqual((air_target["contributors"], bass_target["contributors"]), (1, 2))
                self.assertEqual(air_target["target"]["mid"], -20)


if __name__ == "__main__":
    unittest.main()
