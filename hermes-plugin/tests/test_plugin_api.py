"""dashboard/plugin_api.py: the routes both UIs read, and the POST routes that keep their own state (contract C4)."""

from __future__ import annotations

import base64
import importlib.util
import json
import sys
import tempfile
import unittest
from pathlib import Path

HERE = Path(__file__).resolve().parent

try:
    import fastapi  # noqa: F401

    HAVE_FASTAPI = True
except ImportError:
    HAVE_FASTAPI = False

try:
    from fastapi.testclient import TestClient
except ImportError:  # the test client needs httpx
    TestClient = None


def _load(name: str, path: Path):
    spec = importlib.util.spec_from_file_location(name, path)
    module = importlib.util.module_from_spec(spec)
    sys.modules[name] = module
    spec.loader.exec_module(module)
    return module


fixtures = _load("kumi_test_ui_fixtures", HERE / "ui_fixtures.py")


@unittest.skipUnless(HAVE_FASTAPI, "plugin_api.py runs inside Hermes, which has FastAPI")
class Routes(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        # The module name Hermes gives it when it mounts the routes.
        cls.api = _load("hermes_dashboard_plugin_kumi", HERE.parent / "dashboard" / "plugin_api.py")

    def setUp(self) -> None:
        self.tmp = tempfile.TemporaryDirectory()
        root = Path(self.tmp.name)
        self.data = root / "plugin-data" / "kumi"
        self.library = root / "samples"
        self.state = fixtures.write_state(self.data, self.library, audio="some")
        api = self.api
        self.saved = (api.snapshots.state_dir, api._library_folders)
        api.snapshots.state_dir = lambda: self.state
        api._library_folders = lambda: [str(self.library)]

    def tearDown(self) -> None:
        self.api.snapshots.state_dir, self.api._library_folders = self.saved
        self.tmp.cleanup()

    def test_every_route_is_a_get_but_the_contracts_post_routes(self) -> None:
        methods = {(route.path, method) for route in self.api.router.routes for method in route.methods}
        reads = {"/status", "/focus", "/history", "/map", "/sound", "/audio", "/setup", "/job", "/script", "/palette"}
        self.assertEqual(methods, {(path, "GET") for path in reads} | {("/map/hidden", "POST"), ("/palette", "POST")})

    def test_status_focus_and_history_read_this_profiles_state_on_every_request(self) -> None:
        self.assertTrue(self.api.status()["connected"])
        self.assertEqual(self.api.focus()["transport"]["tempo"], 124.0)
        self.assertEqual([item["label"] for item in self.api.history(limit=2)["items"]], ["c14", "c13"])
        # Another profile's state, resolved on the next request: nothing is cached.
        other = Path(self.tmp.name) / "other" / "state"
        self.api.snapshots.state_dir = lambda: other
        self.assertEqual((self.api.status()["connected"], self.api.status()["stale"]), (False, True))

    def test_the_map_and_a_sound_never_carry_a_file_path(self) -> None:
        current = self.api.sound_map(revision=None)
        self.assertEqual((current["revision"], current["current"]), ("r7", "r7"))
        self.assertTrue(current["sounds"])
        self.assertFalse(any("path" in sound for sound in current["sounds"].values()))
        self.assertNotIn("schema", current)
        sound = self.api.sound(id="s-r7-0091")
        self.assertNotIn("path", sound)
        self.assertTrue(sound["playable"])
        self.assertFalse(self.api.sound(id="s-r7-0001")["playable"])

    def test_no_map_yet_is_an_empty_map_not_an_error(self) -> None:
        (self.state / "map" / "current").unlink()
        empty = self.api.sound_map(revision=None)
        self.assertEqual((empty["revision"], empty["points"], empty["sounds"]), (None, [], {}))

    def test_audio_is_a_data_url_of_the_files_bytes(self) -> None:
        reply = self.api.audio(id="a-7f32")
        self.assertTrue(reply["data_url"].startswith("data:audio/wav;base64,"))
        payload = base64.b64decode(reply["data_url"].split(",", 1)[1])
        self.assertEqual(payload, (self.state / "artifacts" / "a-7f32-dusty.wav").read_bytes())
        self.assertEqual(reply["bytes"], len(payload))
        self.assertTrue(self.api.audio(id="s-r7-0091")["data_url"].startswith("data:audio/wav;base64,"))

    def test_audio_refuses_an_unknown_id_and_a_file_outside_the_approved_folders(self) -> None:
        from fastapi import HTTPException

        with self.assertRaises(HTTPException) as unknown:
            self.api.audio(id="a-nothing")
        self.assertEqual(unknown.exception.status_code, 404)
        self.api._library_folders = lambda: []
        with self.assertRaises(HTTPException):
            self.api.audio(id="s-r7-0091")

    def _sound_file(self, sound_id: str, name: str, data: bytes) -> None:
        """Point a current-map sound at a file of `data` in the approved folder."""
        current = self.state / "map" / "r7.json"
        sound_map = json.loads(current.read_text(encoding="utf-8"))
        path = self.library / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(data)
        sound_map["sounds"][sound_id]["path"] = str(path)
        current.write_text(json.dumps(sound_map), encoding="utf-8")

    def test_aiff_goes_out_as_wav_and_one_it_cant_convert_goes_out_as_it_is(self) -> None:
        import struct

        aiff_tests = _load("kumi_test_aiff_cases", HERE / "test_aiff.py")
        values = [0, 1000, -1000, 32767]
        self._sound_file("s-r7-0091", "Snare.aif", aiff_tests.make(struct.pack(">4h", *values), channels=1, bits=16, frames=4))
        reply = self.api.audio(id="s-r7-0091")
        self.assertEqual(reply["mime"], "audio/wav")
        self.assertTrue(reply["data_url"].startswith("data:audio/wav;base64,"))
        fmt, samples = aiff_tests.wav(base64.b64decode(reply["data_url"].split(",", 1)[1]))
        self.assertEqual((fmt["tag"], fmt["rate"], samples), (1, 44100, struct.pack("<4h", *values)))
        broken = b"FORM\x00\x00\x00\x04AIFF"
        self._sound_file("s-r7-0091", "Broken.aiff", broken)
        reply = self.api.audio(id="s-r7-0091")
        self.assertEqual((reply["mime"], base64.b64decode(reply["data_url"].split(",", 1)[1])), ("audio/aiff", broken))

    def test_a_saved_script_is_served_whole_only_while_it_is_the_code_the_approval_named(self) -> None:
        import hashlib
        import os

        from fastapi import HTTPException

        code = b"import Live\nsong = Live.Application.get_application().get_document()\nsong.tempo = 120\n"
        sha = hashlib.sha256(code).hexdigest()
        folder = self.state / "scripts"
        folder.mkdir()
        (folder / f"{sha}.py").write_bytes(code)
        self.assertEqual(self.api.script(sha=sha, language="python"), {"sha256": sha, "language": "python", "code": code.decode(), "lines": 3})

        def refused(name: str, language: str) -> None:
            with self.assertRaises(HTTPException) as caught:
                self.api.script(sha=name, language=language)
            self.assertEqual(caught.exception.status_code, 404)

        # The language fixes the extension, and only an exact lowercase SHA-256 names a file.
        refused(sha, "lua")
        refused(sha.upper(), "python")
        refused(f"../{sha[3:]}", "python")
        # Code that no longer hashes to its name (changed after the ask) is never shown as the approved code.
        (folder / f"{sha}.py").write_bytes(code + b"song.tempo = 90\n")
        refused(sha, "python")
        # A link isn't followed, even to the right bytes; nor is a scripts folder that's a link.
        elsewhere = Path(self.tmp.name) / "elsewhere"
        elsewhere.mkdir()
        (elsewhere / f"{sha}.py").write_bytes(code)
        (folder / f"{sha}.py").unlink()
        os.symlink(elsewhere / f"{sha}.py", folder / f"{sha}.py")
        refused(sha, "python")
        (folder / f"{sha}.py").unlink()
        folder.rmdir()
        os.symlink(elsewhere, folder)
        refused(sha, "python")
        # Over CC-19's cap of 400 lines, even when it hashes right.
        folder.unlink()
        folder.mkdir()
        long = b"x = 1\n" * 401
        (folder / f"{hashlib.sha256(long).hexdigest()}.py").write_bytes(long)
        refused(hashlib.sha256(long).hexdigest(), "python")

    def test_a_devices_saved_spec_is_served_by_the_hash_its_approval_keyed_on(self) -> None:
        import hashlib
        import json as json_module

        from fastapi import HTTPException

        spec = {"name": "Soft Clip", "about": "Gentle saturation", "controls": [{"name": "Drive", "type": "number", "min": 0, "max": 1, "default": 0.3}],
                "code": "out1 = tanh(in1 * (1 + Drive * 4));\nout2 = tanh(in2 * (1 + Drive * 4));\n"}
        # CC-22: the plugin's canonical bytes, named by their own hash.
        data = json_module.dumps(spec, sort_keys=True, separators=(",", ":"), ensure_ascii=False).encode("utf-8")
        sha = hashlib.sha256(data).hexdigest()
        folder = self.state / "scripts"
        folder.mkdir()
        (folder / f"{sha}.device.json").write_bytes(data)
        served = self.api.script(sha=sha, language="device")
        self.assertEqual((served["sha256"], served["language"], served["lines"]), (sha, "device", 2))
        self.assertEqual(json_module.loads(served["code"]), spec)
        # The same file isn't a Python script, and JSON without code isn't a device's spec.
        with self.assertRaises(HTTPException):
            self.api.script(sha=sha, language="python")
        bare = json_module.dumps({"name": "x"}).encode("utf-8")
        (folder / f"{hashlib.sha256(bare).hexdigest()}.device.json").write_bytes(bare)
        with self.assertRaises(HTTPException):
            self.api.script(sha=hashlib.sha256(bare).hexdigest(), language="device")

    def test_only_audio_or_png_is_served_and_sound_says_what_can_play_the_same_way(self) -> None:
        from fastapi import HTTPException

        listing = self.state / "artifacts.json"
        artifacts = json.loads(listing.read_text(encoding="utf-8"))
        for entry in artifacts["artifacts"].values():
            entry["mime"] = "text/html"
        listing.write_text(json.dumps(artifacts), encoding="utf-8")
        current = self.state / "map" / "r7.json"
        sound_map = json.loads(current.read_text(encoding="utf-8"))
        sound_map["sounds"]["s-r7-0091"]["mime"] = "text/html"
        current.write_text(json.dumps(sound_map), encoding="utf-8")
        for media_id in ("a-7f32", "s-r7-0091"):
            with self.assertRaises(HTTPException) as refused:
                self.api.audio(id=media_id)
            self.assertEqual(refused.exception.status_code, 404)
        self.assertFalse(self.api.sound(id="s-r7-0091")["playable"])

    def test_an_unknown_sound_is_404(self) -> None:
        from fastapi import HTTPException

        with self.assertRaises(HTTPException) as unknown:
            self.api.sound(id="s-r7-9999")
        self.assertEqual(unknown.exception.status_code, 404)

    def test_settings_come_from_the_plugins_entry_its_legacy_config_else_nothing(self) -> None:
        import types

        config = {"plugins": {"entries": {"kumi": {"settings": {"daw": "reaper", "library_folders": ["/a", 7, "/b"]}, "config": {"daw": "renoise", "other": 1}}}}}
        saved = sys.modules.get("hermes_cli.config")
        self.api._library_folders = self.saved[1]
        try:
            sys.modules["hermes_cli.config"] = types.SimpleNamespace(load_config_readonly=lambda: config)
            self.assertEqual((self.api._setting("daw"), self.api._setting("other"), self.api._setting("nothing")), ("reaper", 1, None))
            self.assertEqual(self.api._library_folders(), ["/a", "/b"])
            config["plugins"] = {}
            self.assertEqual(self.api._library_folders(), [])
        finally:
            if saved is None:
                sys.modules.pop("hermes_cli.config", None)
            else:
                sys.modules["hermes_cli.config"] = saved

    def test_setup_answers_what_setup_pys_status_says_for_the_daw_setting_and_unknown_without_it(self) -> None:
        import types

        name = "hermes_dashboard_plugin_kumi_setup"
        saved = (sys.modules.get(name), self.api._setting)
        asked = []
        ready = {"state": "ready", "runtime": {"installed": True, "version": "1.24.70"}, "next_step": None}

        def status(*, daw=None, **_):
            asked.append(daw)
            return ready

        def broken(*, daw=None, **_):
            raise OSError("[Errno 2] No such file or directory: '/var/folders/x/plugin-data/kumi/runtime/missing.json'")

        try:
            self.api._setting = lambda key: "reaper" if key == "daw" else None
            sys.modules[name] = types.SimpleNamespace(status=status)
            self.assertEqual((self.api.setup(), asked), ({**ready, "daw_chosen": True}, ["reaper"]))
            self.api._setting = lambda key: None
            self.assertEqual((self.api.setup()["daw_chosen"], asked[-1]), (False, None),
                             "with no DAW chosen, setup reads as its default and the page offers each DAW")
            sys.modules[name] = types.SimpleNamespace(status=broken)
            with self.assertLogs(self.api._log.name, "WARNING"):
                failed = self.api.setup()
            self.assertEqual(failed, {"state": "unknown", "message": "Kumi couldn't read its setup state."})
            sys.modules[name] = types.SimpleNamespace(status=lambda **_: "ready")
            with self.assertLogs(self.api._log.name, "WARNING"):
                self.assertEqual(self.api.setup()["state"], "unknown")
            sys.modules[name] = types.SimpleNamespace()
            self.assertEqual(self.api.setup()["state"], "unknown")
        finally:
            module, self.api._setting = saved
            if module is None:
                sys.modules.pop(name, None)
            else:
                sys.modules[name] = module

    def test_before_a_runtime_names_its_daw_status_names_the_one_chosen_in_kumis_settings(self) -> None:
        saved = self.api._setting
        try:
            self.api._setting = lambda key: "reaper" if key == "daw" else None
            self.assertEqual(self.api.status()["daw"], "ableton", "a runtime's own word comes first")
            fresh = Path(self.tmp.name) / "fresh" / "state"
            self.api.snapshots.state_dir = lambda: fresh
            self.assertEqual(self.api.status()["daw"], "reaper")
            for unset in (None, "logic"):
                self.api._setting = lambda key, unset=unset: unset if key == "daw" else None
                self.assertIsNone(self.api.status().get("daw"), unset)
        finally:
            self.api._setting = saved

    def test_a_job_is_read_by_its_id_and_an_unknown_one_is_404(self) -> None:
        from fastapi import HTTPException

        self.assertEqual(self.api.job(id="j-scan-1")["progress"]["done"], 412)
        with self.assertRaises(HTTPException):
            self.api.job(id="../owner")

    @unittest.skipIf(TestClient is None, "the FastAPI test client needs httpx")
    def test_over_http_queries_are_validated_and_only_the_contracts_posts_are_taken(self) -> None:
        from fastapi import FastAPI

        app = FastAPI()
        app.include_router(self.api.router, prefix="/api/plugins/kumi")
        client = TestClient(app)
        self.assertEqual(client.get("/api/plugins/kumi/history", params={"limit": 500}).status_code, 422)
        self.assertEqual(client.get("/api/plugins/kumi/history", params={"limit": 3}).json()["items"][0]["label"], "c14")
        self.assertEqual(client.post("/api/plugins/kumi/status").status_code, 405)
        self.assertEqual(client.get("/api/plugins/kumi/audio", params={"id": "a-nothing"}).status_code, 404)
        self.assertNotIn(str(self.library), json.dumps(client.get("/api/plugins/kumi/map").json()))

    @unittest.skipIf(TestClient is None, "the FastAPI test client needs httpx")
    def test_over_http_hidden_sounds_leave_the_map_and_clear_brings_them_back(self) -> None:
        from fastapi import FastAPI

        app = FastAPI()
        app.include_router(self.api.router, prefix="/api/plugins/kumi")
        client = TestClient(app)
        route = "/api/plugins/kumi/map/hidden"
        whole = client.get("/api/plugins/kumi/map").json()
        self.assertEqual((whole["hidden"], client.get("/api/plugins/kumi/status").json()["revisions"]["hidden"]), (0, 0))

        # s-r7-0001's file is missing; s-r7-0091's plays, so its "gone" is the client's mistake.
        reply = client.post(route, json={"revision": "r7", "ids": [{"id": "s-r7-0001", "why": "gone"}, {"id": "s-r7-0091", "why": "gone"}]})
        self.assertEqual((reply.status_code, reply.json()), (200, {"hidden": 1, "ignored": 1, "seq": 1}))
        shown = client.get("/api/plugins/kumi/map").json()
        self.assertEqual((shown["hidden"], len(shown["points"])), (1, len(whole["points"]) - 1))
        self.assertNotIn("s-r7-0001", shown["sounds"])
        self.assertEqual(client.get("/api/plugins/kumi/status").json()["revisions"]["hidden"], 1)
        self.assertNotIn(str(self.library), reply.text + json.dumps(shown))

        for bad in ({"revision": "r7", "ids": []}, {"action": "forget"}, {"ids": [{"id": "s-r7-0002", "why": "gone"}]}):
            refused = client.post(route, json=bad)
            self.assertEqual(refused.status_code, 400, bad)
            self.assertTrue(refused.json()["detail"])
        self.assertEqual(client.post(route, content=b"{not json", headers={"content-type": "application/json"}).status_code, 400)
        self.assertEqual(client.post(route, content=b" " * (300 * 1024)).status_code, 413)
        self.assertEqual(client.get(route).status_code, 405)

        self.assertEqual(client.post(route, json={"action": "clear"}).json(), {"hidden": 0, "ignored": 0, "seq": 2})
        self.assertEqual(client.get("/api/plugins/kumi/map").json()["hidden"], 0)

    def test_the_palette_round_trips_over_http_in_sound_ids_and_never_a_path(self) -> None:
        from fastapi import FastAPI

        app = FastAPI()
        app.include_router(self.api.router, prefix="/api/plugins/kumi")
        client = TestClient(app)
        url = "/api/plugins/kumi/palette"

        def edit(**body):
            answer = client.post(url, json=body)
            self.assertEqual(answer.status_code, 200, answer.text)
            return answer.json()

        def refused(body) -> str:
            answer = client.post(url, json=body)
            self.assertEqual(answer.status_code, 400, answer.text)
            return answer.json()["detail"]

        self.assertEqual(client.get(url).json(), {"seq": 0, "map_revision": "r7", "current": None, "palettes": []})
        gathered = edit(action="gather", sound_ids=["s-r7-0091", "s-r7-0092", "s-r7-0001"], artifact_ids=["a-7f32"])
        self.assertEqual(gathered["said"], "Added 4 sounds to Palette 1.")
        current = gathered["palette"]["current"]
        self.assertEqual((current["name"], current["reach"], current["mode"], current["excluded"]),
                         ("Palette 1", 0, "strict", 0))
        self.assertEqual([(i["source"], i["sound_id"], i["artifact_id"], i["group"]) for i in current["items"]],
                         [("sound", "s-r7-0091", None, 1), ("sound", "s-r7-0092", None, 1),
                          ("sound", "s-r7-0001", None, 0), ("artifact", None, "a-7f32", None)])
        self.assertEqual(current["items"][3]["name"], "a-7f32-dusty")
        self.assertEqual(refused({"action": "gather", "sound_ids": ["s-r6-0001"]}),
                         "Those sounds aren't on the current map: read it again.")
        self.assertIn("can't go in the palette", refused({"action": "gather", "artifact_ids": ["s-r7-0091"]}))
        for body in ({"action": "gather"}, {"action": "gather", "sound_ids": ["x"] * 10_001}, {"action": "fly"},
                     ["gather"], {"action": "reach", "reach": 51}, {"action": "move", "key": "it-1", "to": 0},
                     {"action": "new", "name": "a\u2028b"}, {"action": "use", "id": "../palettes"}):
            with self.subTest(body=str(body)[:60]):
                self.assertEqual(len(refused(body).splitlines()), 1)
        keys = [item["key"] for item in current["items"]]
        self.assertEqual(edit(action="move", key=keys[2], to=0)["said"], "Moved Boom Kick 01 to place 1 in Palette 1.")
        self.assertEqual(edit(action="reach", reach=4)["palette"]["current"]["mode"], "loose, 4 nearest")
        self.assertEqual(edit(action="rename", name="Snares")["said"], "The palette is called Snares now.")
        removed = edit(action="remove", keys=[keys[3]])
        self.assertEqual([i["key"] for i in removed["palette"]["current"]["items"]], [keys[2], keys[0], keys[1]])
        made = edit(action="new", name="Pads")["palette"]
        self.assertEqual([(p["name"], p["items"], p["current"]) for p in made["palettes"]],
                         [("Snares", 3, False), ("Pads", 0, True)])
        snares = made["palettes"][0]["id"]
        self.assertEqual(edit(action="use", id=snares)["palette"]["current"]["name"], "Snares")
        self.assertEqual(refused({"action": "new", "name": "pads"}), "There's a palette called Pads already: use it "
                                                                     "to work with it.")
        forgot = edit(action="forget", id=snares)
        self.assertEqual((forgot["said"], forgot["palette"]["current"]), ("Forgot the palette Snares and its 3 sounds.",
                                                                          None))
        seq = client.get(url).json()["seq"]
        self.assertEqual((seq, client.get("/api/plugins/kumi/status").json()["revisions"]["palette"]), (8, 8))
        self.assertNotIn(str(self.library), json.dumps(client.get(url).json()))
        self.assertNotIn(str(self.state), json.dumps(gathered))


if __name__ == "__main__":
    unittest.main()
