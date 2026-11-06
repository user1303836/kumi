"""CC-26: a verified plug-in preset into Live's User Library through serve's `preset` op (26-w1…w5, W2-CC26-PLUGIN):
the approval in the preview's own words on each surface, its key, the place bound to what was approved, and what
"undo that" does after it."""
from __future__ import annotations

import hashlib
import unittest

import support

policy = support.module("policy")
tools = support.module("tools")

DIGEST = hashlib.sha256(b"Night Bass").hexdigest()
FILE = f"User Library/Kumi/Instruments/Night Bass {DIGEST[:12]}.vstpreset"
SETTINGS = ["• Osc B: not stored in template → true, stored value",
            "• Volume: 0.3 → 0.45, stored value, 0–1",
            "• Gain: 0 → 0.12345679104328156, dB",
            "• FX Order: [Reverb] → [Distortion, Reverb], chain"]
NOTE = "Values as the preset file stores them; the plug-in may show them differently."


class Presets(unittest.TestCase):
    def use(self, *fixture_args: str, daw: str = "ableton", surface: str = "desktop") -> None:
        self.host, self.ctx, self.data = support.host(self, {"daw": daw}, *fixture_args)
        self.ctx.surface = surface
        support.call(self.host, "kumi_connect", {}, session_id="s1")

    def hook(self) -> dict:
        return policy.pre_tool_call(self.host, tool_name="kumi_preset", args={"artifact_id": "a-preset-1"},
                                    session_id="s1", tool_call_id="p1", task_id="t1", turn_id="u1",
                                    api_request_id="r1", middleware_trace=[])

    def sent(self, action: str) -> list[dict]:
        return [m["params"]["arguments"] for m in support.requests(self.data / "state")
                if m.get("params", {}).get("name") == "preset" and m["params"]["arguments"].get("action") == action]

    def test_the_approval_shows_every_setting_as_stored_and_the_place_writes_only_what_was_approved(self):
        self.use()
        build = {"plugin": "serum-2", "name": "Night Bass", "edits": [{"path": "global/volume", "value": 0.45}]}
        self.assertIsNone(policy.pre_tool_call(self.host, tool_name="kumi_preset_build", args=build, session_id="s1",
                                               tool_call_id="b1", turn_id="u1"), "Kumi's private copy isn't asked")
        built = support.call(self.host, "kumi_preset_build", build, session_id="s1")
        self.assertEqual((built["state"], built["artifact_id"], built["values_note"]), ("built", "a-preset-1", NOTE))
        asked = self.hook()
        self.assertEqual(asked["message"], "\n".join([
            "Kumi: put a Serum 2 preset in your User Library", f"File: {FILE}", "Serum 2 2.0.18, the version installed",
            *SETTINGS, NOTE, policy.PRESET_WARNING]))
        self.assertEqual(asked["rule_key"], f"kumi:preset:{DIGEST}", "an Always covers only this file")
        placed = support.call(self.host, "kumi_preset", {"artifact_id": "a-preset-1"}, session_id="s1")
        self.assertEqual((placed["file"], placed["indexed"]), (FILE, True))
        [place] = self.sent("place")
        self.assertEqual((place["digest"], place["file"]), (DIGEST, FILE), "the preview's digest and file, bound")
        self.assertEqual(support.call(self.host, "kumi_undo", {}, session_id="s1"),
                         {"blocked": tools.FILE_STAYS.format(file=FILE)}, "the file stays, as the approval said")
        self.assertEqual(support.handle(self.host, "kumi_preset", {"artifact_id": "a-preset-1"},
                                        session_id="s1")["error"]["code"], "not_approved",
                         "a call no approval saw places nothing")

    def test_where_the_installed_version_couldnt_be_read_it_says_which_version_the_writer_verified(self):
        self.use("--preset-unchecked")
        self.assertIn("\nVerified with Serum 2 2.0.18 (Kumi couldn't read the installed version)\n",
                      self.hook()["message"])

    def test_a_small_window_gets_every_setting_in_fewer_words_and_a_preset_too_long_for_it_isnt_asked(self):
        self.use(surface="tui")
        self.assertEqual(self.hook()["message"], "\n".join([
            f"Kumi: write {FILE}", "Serum 2 2.0.18", *SETTINGS, NOTE, policy.PRESET_STAYS]))
        self.use("--preset-settings", "12", surface="tui")
        self.assertEqual(self.hook(), policy.block(policy.PRESET_TOO_LONG), "never a form that hides settings")
        self.use("--preset-settings", "12")
        self.assertEqual(self.hook()["message"].count("\n• Macro "), 12, "Desktop shows all twelve")

    def test_a_preview_kumi_cant_show_as_it_is_or_a_place_that_finds_it_changed_writes_nothing(self):
        self.use("--preset-hidden")
        self.assertEqual(self.hook(), policy.block(policy.PRESET_UNSHOWN), "a direction override is never stripped")
        self.use("--preset-changed")
        changed = support.call(self.host, "kumi_preset", {"artifact_id": "a-preset-1"}, session_id="s1")
        self.assertEqual(changed["error"]["code"], "changed")
        self.assertEqual(support.call(self.host, "kumi_undo", {}, session_id="s1"), {"blocked": tools.NOTHING_TO_UNDO},
                         "nothing was written, so nothing is this chat's newest write")

    def test_nobody_there_or_another_daw_places_nothing_and_never_asks_serve(self):
        self.use(surface="webhook")
        self.assertEqual(self.hook(), policy.block(policy.UNATTENDED))
        self.use(daw="renoise")
        self.assertEqual(self.hook(), policy.block(policy.PRESET_ONLY_LIVE))
        self.assertEqual(self.sent("preview"), [])


if __name__ == "__main__":
    unittest.main()
