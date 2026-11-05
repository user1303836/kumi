"""dashboard/plugin_api.py: the read-only routes both UIs read (contract C4)."""

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

    def test_every_route_is_a_get(self) -> None:
        methods = {route.path: route.methods for route in self.api.router.routes}
        self.assertEqual(set(methods), {"/status", "/focus", "/history", "/map", "/sound", "/audio", "/setup", "/job"})
        self.assertTrue(all(found == {"GET"} for found in methods.values()), methods)

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
            self.assertEqual((self.api.setup(), asked), (ready, ["reaper"]))
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

    def test_a_job_is_read_by_its_id_and_an_unknown_one_is_404(self) -> None:
        from fastapi import HTTPException

        self.assertEqual(self.api.job(id="j-scan-1")["progress"]["done"], 412)
        with self.assertRaises(HTTPException):
            self.api.job(id="../owner")

    @unittest.skipIf(TestClient is None, "the FastAPI test client needs httpx")
    def test_over_http_queries_are_validated_and_a_post_is_refused(self) -> None:
        from fastapi import FastAPI

        app = FastAPI()
        app.include_router(self.api.router, prefix="/api/plugins/kumi")
        client = TestClient(app)
        self.assertEqual(client.get("/api/plugins/kumi/history", params={"limit": 500}).status_code, 422)
        self.assertEqual(client.get("/api/plugins/kumi/history", params={"limit": 3}).json()["items"][0]["label"], "c14")
        self.assertEqual(client.post("/api/plugins/kumi/status").status_code, 405)
        self.assertEqual(client.get("/api/plugins/kumi/audio", params={"id": "a-nothing"}).status_code, 404)
        self.assertNotIn(str(self.library), json.dumps(client.get("/api/plugins/kumi/map").json()))


if __name__ == "__main__":
    unittest.main()
