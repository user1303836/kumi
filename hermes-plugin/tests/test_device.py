"""CC-22: Max for Live audio effects from GenExpr (22e, 22-r3, W2's notes): the spec's one canonical form and its hash,
the saved spec, the approval's key and words on each surface, CC-19's trust, serve's preview and build through the
fixture runtime's `device` op, and Desktop's events."""
from __future__ import annotations

import hashlib
import os
from pathlib import Path
import stat
import unittest
from unittest import mock

import support

devices = support.module("devices")
policy = support.module("policy")
scripts = support.module("scripts")
tools = support.module("tools")

CODE = ("Param drive(4, min=1, max=20);\nParam tone(1000, min=200, max=8000);\n"
        "History lp(0);\nlp = mix(lp, tanh(in1 * drive), tone / 20000);\nout1 = lp;\nout2 = lp;\n")
SPEC = {"name": "Warm Fold", "about": "A soft saturator with a tone control, for the bass.",
        "controls": [{"name": "Drive", "type": "number", "min": 1, "max": 20, "default": 4, "unit": "x"},
                     {"name": "Tone", "type": "number", "min": 200, "max": 8000, "default": 1000, "unit": "Hz"},
                     {"name": "Mode", "type": "choice", "options": ["Soft", "Hard"], "default": "Soft"},
                     {"name": "Bypass", "type": "switch", "default": False}],
        "code": CODE}
FILE = "User Library/Kumi/Audio Effects/Warm Fold.amxd"


def digest(spec: dict = SPEC) -> str:
    return hashlib.sha256(devices.canonical(spec).encode("utf-8")).hexdigest()


class Devices(unittest.TestCase):
    def setUp(self):
        self.use()

    def use(self, *fixture_args: str) -> None:
        self.host, self.ctx, self.data = support.host(self, None, *fixture_args)
        self.events: list[tuple[str, dict]] = []
        patched = mock.patch.object(tools, "broadcast", lambda event, payload: self.events.append((event, payload)))
        patched.start()
        self.addCleanup(patched.stop)
        self.bypassed = False
        patched = mock.patch.object(tools, "approvals_bypassed", lambda host, **kw: self.bypassed)
        patched.start()
        self.addCleanup(patched.stop)
        support.call(self.host, "kumi_connect", {}, session_id="s1")

    def start(self, session: str = "s1") -> None:
        policy.chat_turn(self.host, session_id=session, is_first_turn=True)

    def hook(self, spec: dict = SPEC, session: str = "s1") -> dict:
        return policy.pre_tool_call(self.host, tool_name="kumi_device", args={"spec": spec}, session_id=session,
                                    tool_call_id="c1", task_id="t1", turn_id="u1", api_request_id="r1",
                                    middleware_trace=[])

    def sent(self, action: str) -> list[dict]:
        return [m["params"]["arguments"] for m in support.requests(self.data / "state")
                if m.get("params", {}).get("name") == "device" and m["params"]["arguments"].get("action") == action]

    def saved(self, spec: dict = SPEC) -> Path:
        return self.data / "state" / "scripts" / f"{digest(spec)}.device.json"

    def test_the_spec_is_one_set_of_bytes_sorted_and_unescaped_and_its_hash_is_theirs(self):
        spec = {"code": "out1 = in1 * 1e-07;", "name": "Café", "controls": [{"type": "switch", "name": "On"}],
                "about": "Ü"}
        text = devices.canonical(spec)
        self.assertEqual(text, '{"about":"Ü","code":"out1 = in1 * 1e-07;","controls":[{"name":"On","type":"switch"}],'
                               '"name":"Café"}')
        self.assertEqual(scripts.sha256(text), hashlib.sha256(text.encode("utf-8")).hexdigest())
        self.assertEqual(devices.canonical(dict(reversed(list(spec.items())))), text, "key order never matters")
        self.assertIsNotNone(devices.problem({**SPEC, "controls": [{**SPEC["controls"][0], "max": float("nan")}]}),
                             "JSON can't carry NaN, so no hash is made of it")
        self.assertEqual(scripts.LANGUAGES["device"], ".device.json")

    def test_the_spec_is_kept_once_as_its_canonical_bytes_named_by_their_hash(self):
        self.start()
        self.assertEqual(self.hook()["action"], "approve")
        saved = self.saved()
        self.assertEqual(saved.read_bytes(), devices.canonical(SPEC).encode("utf-8"))
        if os.name == "posix":
            self.assertEqual(stat.S_IMODE(saved.stat().st_mode), 0o600)
        self.assertEqual(self.hook()["action"], "approve", "asking again finds the same file and keeps it")
        saved.write_text("{}", encoding="utf-8")
        self.assertEqual(self.hook()["message"], policy.DEVICE + "it couldn't keep a copy of the spec to show you.",
                         "a file that isn't the spec it's named for is never used")

    def test_the_approval_is_keyed_by_the_spec_and_shows_the_warning_file_controls_and_code(self):
        self.start()
        terminal = self.hook()
        self.assertEqual(terminal["rule_key"], f"kumi:device:{digest()}", "unscoped: never this chat's id")
        self.assertNotIn(policy.CHAT_ONLY, terminal["message"])
        self.assertTrue(policy.shows_whole(terminal["message"], "tui"))
        for words in (devices.WARNING, FILE, 'Hermes says: "A soft saturator', f"SHA-256 {digest()[:8]}",
                      "4 controls: Drive 1–20 x, Tone 200–8000 Hz", "  Param drive(4, min=1, max=20);"):
            self.assertIn(words, terminal["message"])
        self.ctx.surface = "desktop"
        desktop = self.hook()["message"]
        for words in ("  • Drive 1–20 x, default 4 x", "  • Tone 200–8000 Hz, default 1000 Hz",
                      "  • Mode Soft/Hard, default Soft", "  • Bypass on/off, default off", "  out2 = lp;",
                      "6 lines of GenExpr", f"SHA-256 {digest()[:16]}", devices.WARNING, FILE):
            self.assertIn(words, desktop)
        other = self.hook({**SPEC, "controls": [{**SPEC["controls"][0], "max": 40}, *SPEC["controls"][1:]]})
        self.assertNotEqual(other["rule_key"], terminal["rule_key"], "a control's range is part of what's approved")

    def test_where_the_warning_or_the_file_dont_fit_it_isnt_asked(self):
        self.ctx.settings["scripts_in_messaging"] = True
        for surface in ("discord", "telegram", "slack"):
            with self.subTest(surface=surface):
                self.ctx.surface = surface
                self.start(f"chat-{surface}")
                decision = self.hook(session=f"chat-{surface}")
                if decision["action"] == "block":
                    self.assertEqual(decision["message"], policy.TOO_LONG)
                else:
                    self.assertIn(devices.WARNING, decision["message"])
                    self.assertIn(FILE, decision["message"])
                    self.assertTrue(policy.shows_whole(decision["message"], surface))
        self.ctx.surface = "discord"
        self.assertEqual(self.hook(session="chat-discord"), {"action": "block", "message": policy.TOO_LONG},
                         "Discord's 300 characters can't hold the warning and the file with the rest")
        self.assertEqual([payload for event, payload in self.events
                          if event == "script.pending" and payload["session_id"] == "chat-discord"], [],
                         "nothing pending for a device that wasn't asked")

    def test_a_subagent_or_a_tainted_chat_with_approvals_off_is_refused_and_a_clean_chat_asks(self):
        policy.subagent_started(self.host, parent_session_id="s1", child_session_id="child")
        self.assertEqual(self.hook(session="child"), {"action": "block", "message": policy.DEVICE_SUBAGENT})
        self.start()
        self.bypassed = True
        self.assertEqual(self.hook()["action"], "approve", "a clean chat's device goes to Hermes, as a script's does")
        self.bypassed = False
        self.assertEqual(self.hook()["action"], "approve")
        policy.post_tool_call(self.host, tool_name="web_extract", tool_call_id="x1", args={}, result="{}", status="ok",
                              session_id="s1")
        asked = self.hook()
        self.assertEqual(asked["rule_key"], f"kumi:device:{digest()}")
        self.ctx.surface = "desktop"
        self.assertIn("This chat read outside content (web_extract), so each device is asked.", self.hook()["message"])
        self.bypassed = True
        refused = self.hook()
        self.assertEqual(refused["action"], "block")
        self.assertIn("nobody would review it", refused["message"])
        self.bypassed = None
        self.assertIn("can't tell whether Hermes's approvals are on", self.hook()["message"])
        self.ctx.surface = "webhook"
        self.assertEqual(self.hook()["message"], policy.UNATTENDED)

    def test_a_spec_serve_hashed_differently_or_refused_is_never_put_to_the_producer(self):
        self.start()
        refused = self.hook({**SPEC, "code": "while (1) { out1 = in1; }\nout2 = in2;\n"})
        self.assertEqual(refused["action"], "block")
        self.assertIn("• code: `while` and `do` aren't allowed", refused["message"])
        self.assertIn("spec has map, which a device built from Hermes doesn't take",
                      self.hook({**SPEC, "map": {}})["message"], "the plugin refuses what v1's spec doesn't take")
        self.use("--device-wrong-sha")
        self.start()
        self.assertEqual(self.hook(), {"action": "block",
                                       "message": policy.DEVICE + "its runtime read a different spec than this one."})
        self.assertFalse(self.saved().exists(), "nothing is kept for a spec that wasn't asked")
        self.assertEqual(self.sent("preview")[0]["spec_json"], devices.canonical(SPEC))

    def test_the_build_gets_the_approved_bytes_and_their_hash_and_its_answer_comes_back_with_the_next_step(self):
        self.start()
        built = support.call(self.host, "kumi_device", {"spec": SPEC}, session_id="s1")
        self.assertEqual({key: built[key] for key in ("name", "file", "sha256", "listed")},
                         {"name": "Warm Fold", "file": FILE, "sha256": digest(), "listed": True})
        self.assertEqual(built["artifact_id"], "a-device-1")
        self.assertIn("load_device", built["next"])
        self.assertIn(built["item"], built["next"])
        self.assertIn("first kumi_observe", built["next"])
        self.assertIn("move_device", built["next"])
        [build] = self.sent("build")
        self.assertEqual((build["spec_json"], build["sha256"]), (devices.canonical(SPEC), digest()))
        self.assertEqual(self.sent("preview")[0]["spec_json"], build["spec_json"], "one string, never re-encoded")
        self.assertEqual(support.handle(self.host, "kumi_device", {"spec": SPEC}, session_id="s1")["error"]["code"],
                         "not_approved", "a call no approval saw never builds")

    def test_a_stale_build_comes_back_as_serve_gave_it(self):
        self.use("--device-file-appears")
        self.start()
        stale = support.call(self.host, "kumi_device", {"spec": SPEC}, session_id="s1")
        self.assertEqual(stale["error"]["code"], "stale")
        self.assertIn("took the name the approval showed", stale["error"]["reason"])

    def test_desktop_is_told_of_the_device_by_its_spec_hash_and_how_it_ended(self):
        self.start()
        support.call(self.host, "kumi_device", {"spec": SPEC}, session_id="s1")
        pending = next(payload for event, payload in self.events if event == "script.pending")
        self.assertEqual({key: pending[key] for key in ("language", "sha256", "file", "warning", "lines")},
                         {"language": "device", "sha256": digest(), "file": FILE, "warning": devices.WARNING,
                          "lines": 6})
        self.assertNotIn("code", pending, "the code never travels in an event")
        policy.post_tool_call(self.host, tool_name="kumi_device", tool_call_id="c9", args={"spec": SPEC},
                              result='{"name": "Warm Fold", "item": "query:x", "sha256": "s"}', status="ok",
                              session_id="s1")
        policy.post_tool_call(self.host, tool_name="kumi_device", tool_call_id="c10", args={"spec": SPEC},
                              result='{"error": {"code": "stale", "reason": "r"}}', status="ok", session_id="s1")
        settled = [(payload["sha256"], payload["outcome"]) for event, payload in self.events
                   if event == "script.settled"]
        self.assertEqual(settled[-2:], [(digest(), "built"), (digest(), "refused")])


if __name__ == "__main__":
    unittest.main()
