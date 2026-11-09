"""Sound design: `POST /design` and `GET /designs` (designs.py) for the dashboard, and the agent's `kumi_design`. Ids in
and out, never a path; at most two renders at once; the 200 newest results kept unless a palette keeps an older one;
a result served by the one media guard (`snapshots.media`), so it plays, goes in a palette and feeds the next design."""

from __future__ import annotations

import base64
import importlib.util
import json
import os
import re
import subprocess
import sys
import tempfile
import threading
import unittest
from pathlib import Path
from unittest import mock

import support

HERE = Path(__file__).resolve().parent
DESIGN_ID = re.compile(r"a-d[A-Za-z0-9]{22}")

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
fixture_runtime = _load("kumi_test_fixture_runtime", HERE / "fixture_runtime.py")


def listed(state: Path, *answers: dict) -> None:
    """`<state>/designs.json` listing `answers` (newest first), each with its WAV in `<state>/designs/`."""
    (state / "designs").mkdir(parents=True, exist_ok=True)
    for answer in answers:
        (state / "designs" / f"{answer['artifact_id']}.wav").write_bytes(fixture_runtime.WAV)
    (state / "designs.json").write_text(json.dumps({"schema": 1, "seq": len(answers), "designs": list(answers)}),
                                        encoding="utf-8")


@unittest.skipUnless(HAVE_FASTAPI and TestClient is not None, "plugin_api.py runs inside Hermes, with FastAPI and httpx")
class Routes(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        cls.api = _load("hermes_dashboard_plugin_kumi", HERE.parent / "dashboard" / "plugin_api.py")
        cls.designs = cls.api.designs

    def setUp(self) -> None:
        from fastapi import FastAPI

        self.tmp = tempfile.TemporaryDirectory()
        root = Path(self.tmp.name)
        self.data = root / "plugin-data" / "kumi"
        self.library = root / "samples"
        self.state = fixtures.write_state(self.data, self.library, audio="some")
        api = self.api
        self.saved = (api.snapshots.state_dir, api._library_folders)
        api.snapshots.state_dir = lambda: self.state
        api._library_folders = lambda: [str(self.library)]
        support.install(self.data, version=api._load("setup", "setup.py").pinned_version())
        self.kick = self._sound_file("s-r7-0002", "Kick.wav")
        self.tom = self._sound_file("s-r7-0003", "Tom.wav")
        app = FastAPI()
        app.include_router(api.router, prefix="/api/plugins/kumi")
        self.client = TestClient(app)

    def tearDown(self) -> None:
        self.api.snapshots.state_dir, self.api._library_folders = self.saved
        self.tmp.cleanup()

    def _sound_file(self, sound_id: str, name: str) -> str:
        """Point a current-map sound at a WAV in the approved folder; its real path."""
        current = self.state / "map" / "r7.json"
        sound_map = json.loads(current.read_text(encoding="utf-8"))
        path = self.library / "made" / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(fixture_runtime.WAV)
        sound_map["sounds"][sound_id]["path"] = str(path)
        current.write_text(json.dumps(sound_map), encoding="utf-8")
        return os.path.realpath(path)

    def post(self, body):
        return self.client.post("/api/plugins/kumi/design", json=body)

    def made(self) -> list[str]:
        folder = self.state / "designs"
        return sorted(path.name for path in folder.glob("*.wav")) if folder.is_dir() else []

    def engine_got(self, artifact_id: str) -> dict:
        """The request the fixture's `kumi design` was handed (it keeps it beside the WAV)."""
        return json.loads((self.state / "designs" / f"{artifact_id}.request.json").read_text(encoding="utf-8"))

    def test_a_design_takes_ids_answers_ids_and_its_result_plays_and_feeds_the_next_design(self) -> None:
        sources = [{"sound_id": "s-r7-0002", "map_revision": "r7"},
                   {"sound_id": "s-r7-0003", "map_revision": "r7", "name": "Low Tom"}]
        reply = self.post({"op": "cross", "mode": "blend", "sources": sources, "seed": 5})
        self.assertEqual(reply.status_code, 200, reply.text)
        first = reply.json()
        self.assertRegex(first["artifact_id"], DESIGN_ID)
        self.assertEqual((first["op"], first["mode"], first["seed"], first["sources"]), ("cross", "blend", 5, sources))
        self.assertEqual(len(first["peaks"]), 256)
        self.assertNotIn(str(self.library), reply.text)
        self.assertNotIn(str(self.state), reply.text)
        engine = self.engine_got(first["artifact_id"])
        map_name = json.loads((self.state / "map" / "r7.json").read_text(encoding="utf-8"))["sounds"]["s-r7-0002"]["name"]
        self.assertEqual(engine["sources"], [{"file": self.kick, "name": map_name}, {"file": self.tom, "name": "Low Tom"}],
                         "the engine reads the files the media guard approved, by the names the producer sees")
        self.assertEqual((engine["op"], engine["mode"], engine["seed"]), ("cross", "blend", 5))

        audio = self.client.get("/api/plugins/kumi/audio", params={"id": first["artifact_id"]}).json()
        self.assertTrue(audio["data_url"].startswith("data:audio/wav;base64,"))
        self.assertEqual(base64.b64decode(audio["data_url"].split(",", 1)[1]),
                         (self.state / "designs" / f"{first['artifact_id']}.wav").read_bytes())

        again = self.post({"op": "stack", "sources": [{"artifact_id": first["artifact_id"]}, sources[0]]})
        self.assertEqual(again.status_code, 200, again.text)
        second = again.json()
        self.assertEqual(self.engine_got(second["artifact_id"])["sources"][0]["file"],
                         os.path.realpath(self.state / "designs" / f"{first['artifact_id']}.wav"))
        self.assertNotIn("mode", self.engine_got(second["artifact_id"]))
        history = self.client.get("/api/plugins/kumi/designs").json()["designs"]
        self.assertEqual([d["artifact_id"] for d in history], [second["artifact_id"], first["artifact_id"]],
                         "newest first, the history surviving a reload")
        self.assertEqual(history[1], first)

    def test_a_request_no_design_can_take_is_refused_with_its_reason_and_makes_nothing(self) -> None:
        two = [{"sound_id": "s-r7-0002", "map_revision": "r7"}, {"sound_id": "s-r7-0003", "map_revision": "r7"}]
        refused = {
            "not an object": [two],
            "unknown field": {"op": "stack", "sources": two, "out": "/tmp/x.wav"},
            "unknown op": {"op": "morph", "sources": two},
            "cross without a mode": {"op": "cross", "sources": two},
            "fuse with cross's mode": {"op": "fuse", "mode": "blend", "sources": two},
            "stack with a mode": {"op": "stack", "mode": "loop", "sources": two},
            "one sound": {"op": "stack", "sources": two[:1]},
            "grow from three": {"op": "grow", "sources": two + two[:1]},
            "grow with a mode": {"op": "grow", "mode": "blend", "sources": two},
            "a steer that isn't choices": {"op": "stack", "sources": two, "options": {"steer": 3}},
            "a steer with a nested object": {"op": "stack", "sources": two, "options": {"steer": {"bright": {"a": 1}}}},
            "a file": {"op": "stack", "sources": [two[0], {"file": self.tom}]},
            "both ids": {"op": "stack", "sources": [two[0], {"sound_id": "s-r7-0003", "artifact_id": "a-7f32"}]},
            "a revision without a sound": {"op": "stack", "sources": [two[0], {"artifact_id": "a-7f32", "map_revision": "r7"}]},
            "a seed that isn't whole": {"op": "stack", "sources": two, "seed": 1.5},
            "a seed past 2^53": {"op": "stack", "sources": two, "seed": 2**53},
            "a true seed": {"op": "stack", "sources": two, "seed": True},
            "an unknown option": {"op": "stack", "sources": two, "options": {"gain": 2}},
            "a split of words": {"op": "cross", "mode": "resonate", "sources": two, "options": {"split": ["a"]}},
            "an older map's sound": {"op": "stack", "sources": [two[0], {"sound_id": "s-r7-0003", "map_revision": "r6"}]},
            "a sound not on the map": {"op": "stack", "sources": [two[0], {"sound_id": "s-r7-9999", "map_revision": "r7"}]},
            "a sound whose file is gone": {"op": "stack", "sources": [two[0], {"sound_id": "s-r7-0001", "map_revision": "r7"}]},
            "a design never made": {"op": "stack", "sources": [two[0], {"artifact_id": "a-d" + "x" * 22}]},
            "a map sound as an artifact": {"op": "stack", "sources": [two[0], {"artifact_id": "s-r7-0003"}]},
            "an artifact as a map sound": {"op": "stack", "sources": [two[0], {"sound_id": "a-7f32"}]},
        }
        for why, body in refused.items():
            with self.subTest(why):
                reply = self.post(body)
                self.assertEqual(reply.status_code, 400, reply.text)
                self.assertTrue(reply.json()["detail"])
                self.assertNotIn(str(self.library), reply.text)
        self.assertEqual(self.client.post("/api/plugins/kumi/design", content=b"{not json").status_code, 400)
        self.assertEqual(self.client.post("/api/plugins/kumi/design", content=b" " * (9 * 1024 * 1024)).status_code, 413)
        self.assertEqual(self.client.get("/api/plugins/kumi/design").status_code, 405)
        self.assertEqual(self.made(), [], "nothing was made")
        self.assertEqual(self.client.get("/api/plugins/kumi/designs").json(), {"designs": []})

    def test_without_the_pinned_runtime_set_up_its_503(self) -> None:
        body = {"op": "stack", "sources": [{"sound_id": "s-r7-0002"}, {"sound_id": "s-r7-0003"}]}
        receipt = self.data / "runtime" / "current.json"
        record = json.loads(receipt.read_text(encoding="utf-8"))
        receipt.write_text(json.dumps({**record, "version": "1.0.0"}), encoding="utf-8")
        self.assertEqual(self.post(body).status_code, 503, "another version than the pin")
        receipt.unlink()
        reply = self.post(body)
        self.assertEqual(reply.status_code, 503)
        self.assertIn("set up", reply.json()["detail"])

    def test_the_engines_refusal_names_the_sound_never_its_path(self) -> None:
        kumi = str(support.install(self.data, version=self.api._load("setup", "setup.py").pinned_version()))
        gone = str(self.library / "made" / "Gone Kick.wav")
        request = self.designs.parse({"op": "stack", "sources": [{"sound_id": "s-r7-0002"}, {"sound_id": "s-r7-0003"}]})
        with self.assertRaises(self.designs.Invalid) as refused:
            self.designs.render(self.state, request, [(gone, "Gone Kick"), (self.tom, "Tom")], kumi, None)
        self.assertIn("Gone Kick", str(refused.exception))
        self.assertNotIn(gone, str(refused.exception))
        self.assertEqual(self.made(), [], "a refused render leaves no file")

    def _quick_render(self, gate: threading.Event | None = None):
        """A stand-in for running `kumi design`: writes the WAV and answers as the engine does, after `gate` opens."""
        running, most = [0], [0]
        lock = threading.Lock()

        def run(argv, input, **_):
            with lock:
                running[0] += 1
                most[0] = max(most[0], running[0])
            try:
                if gate is not None:
                    gate.wait(10)
                request = json.loads(input)
                Path(argv[argv.index("--out") + 1]).write_bytes(fixture_runtime.WAV)
                answer = fixture_runtime.answer_of({**request, "sources": [{"name": s["name"]} for s in request["sources"]]}, None)
                return subprocess.CompletedProcess(argv, 0, json.dumps(answer) + "\n", "")
            finally:
                with lock:
                    running[0] -= 1

        return run, most

    def test_at_most_two_renders_run_at_once_and_a_third_is_told_to_wait(self) -> None:
        gate = threading.Event()
        run, most = self._quick_render(gate)
        request = self.designs.parse({"op": "stack", "sources": [{"sound_id": "s-r7-0002"}, {"sound_id": "s-r7-0003"}]})
        files = [(self.kick, "Kick"), (self.tom, "Tom")]
        answers: list = []
        with mock.patch.object(self.designs.subprocess, "run", run):
            threads = [threading.Thread(target=lambda: answers.append(self.designs.render(self.state, request, files, "kumi", None)))
                       for _ in range(2)]
            for thread in threads:
                thread.start()
            self.assertTrue(support.eventually(lambda: most[0] == 2))
            busy = self.post({"op": "stack", "sources": [{"sound_id": "s-r7-0002"}, {"sound_id": "s-r7-0003"}]})
            self.assertEqual(busy.status_code, 503)
            self.assertIn("two", busy.json()["detail"])
            gate.set()
            for thread in threads:
                thread.join(10)
            self.assertEqual(len(answers), 2)
            self.assertEqual(most[0], 2)
            self.assertEqual(self.post({"op": "stack", "sources": [{"sound_id": "s-r7-0002"}, {"sound_id": "s-r7-0003"}]}).status_code,
                             200, "a slot frees when a render ends")

    def test_the_newest_stay_and_an_older_one_goes_unless_a_palette_keeps_it(self) -> None:
        run, _ = self._quick_render()
        body = {"op": "cross", "mode": "blend", "sources": [{"sound_id": "s-r7-0002"}, {"sound_id": "s-r7-0003"}]}
        with mock.patch.object(self.designs.subprocess, "run", run), mock.patch.object(self.designs, "KEEP", 3):
            made = []
            for seed in range(5):
                made.append(self.post({**body, "seed": seed}).json()["artifact_id"])
                if seed == 0:  # "Keep": the first goes in the palette, through gr's palette route
                    kept = self.client.post("/api/plugins/kumi/palette", json={"action": "gather", "artifact_ids": [made[0]]})
                    self.assertEqual(kept.status_code, 200, kept.text)
                    [item] = kept.json()["palette"]["current"]["items"]
                    self.assertEqual((item["source"], item["artifact_id"], item["name"]),
                                     ("artifact", made[0], "Blend of 2, seed 0"))
        history = [d["artifact_id"] for d in self.client.get("/api/plugins/kumi/designs").json()["designs"]]
        self.assertEqual(history, [made[4], made[3], made[2], made[0]], "the three newest, and the one the palette keeps")
        self.assertEqual(self.made(), sorted(f"{each}.wav" for each in history), "a result that goes takes its file")
        self.assertEqual(self.client.get("/api/plugins/kumi/audio", params={"id": made[1]}).status_code, 404)
        self.assertEqual(self.client.get("/api/plugins/kumi/audio", params={"id": made[0]}).status_code, 200)


class Media(unittest.TestCase):
    """`snapshots.media`, the one guard `/audio`, `/sound`, the palette and a design's sources ask, knows a design."""

    def setUp(self) -> None:
        self.snapshots = support.module("snapshots")
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        self.state = Path(tmp.name) / "state"
        self.state.mkdir()
        self.id = "a-d" + "A" * 22
        listed(self.state, {"artifact_id": self.id, "op": "stack", "seed": 3, "recipe": "Stack of 2: Kick, Tom"})

    def test_a_listed_design_is_served_from_its_own_file_only(self) -> None:
        found = self.snapshots.media(self.state, self.id)
        self.assertEqual((found["path"], found["mime"]),
                         (os.path.realpath(self.state / "designs" / f"{self.id}.wav"), "audio/wav"))
        unlisted = "a-d" + "B" * 22
        (self.state / "designs" / f"{unlisted}.wav").write_bytes(fixture_runtime.WAV)
        self.assertIsNone(self.snapshots.media(self.state, unlisted), "a file nobody listed isn't served")
        self.assertIsNone(self.snapshots.media(self.state, "a-d" + "A" * 21 + "/"), "nor an id of another shape")

    @unittest.skipIf(os.name == "nt", "a symbolic link needs privileges on Windows")
    def test_a_design_whose_file_leads_elsewhere_isnt_served(self) -> None:
        outside = self.state.parent / "secret.wav"
        outside.write_bytes(fixture_runtime.WAV)
        kept = self.state / "designs" / f"{self.id}.wav"
        kept.unlink()
        kept.symlink_to(outside)
        self.assertIsNone(self.snapshots.media(self.state, self.id))

    def test_a_palette_keeps_a_design_by_its_recipe_and_seed(self) -> None:
        self.assertEqual(self.snapshots.palette_artifact(self.state, self.id), "Stack of 2, seed 3")
        self.assertIsNone(self.snapshots.palette_artifact(self.state, "a-d" + "B" * 22))


class Tool(unittest.TestCase):
    """The agent's kumi_design: unasked in the producer's own turn, never unattended, forked or scheduled; sources by
    id only, a dashboard design handed to serve as its kept file and answered as its id."""

    def setUp(self) -> None:
        self.policy, self.tools = support.module("policy"), support.module("tools")
        self.host, self.ctx, self.data = support.host(self)
        support.call(self.host, "kumi_connect", {}, session_id="s1")
        self.args = {"op": "cross", "mode": "attack",
                     "sources": [{"sound_id": "s-r1-1", "map_revision": "r1"}, {"sound_id": "s-r1-2", "map_revision": "r1"}]}

    def hook(self, **context) -> dict | None:
        return self.policy.pre_tool_call(self.host, tool_name="kumi_design", args=self.args,
                                         **{"session_id": "s1", "tool_call_id": "d1", "turn_id": "u1", **context})

    def designed(self) -> list[dict]:
        return [m["params"]["arguments"] for m in support.requests(self.data / "state")
                if m.get("params", {}).get("name") == "design"]

    def test_its_never_asked_in_the_producers_turn_and_never_runs_with_nobody_there(self) -> None:
        self.policy.turn_started(self.host, session_id="s1", turn_id="u1")
        self.assertIsNone(self.hook(), "it changes no project, so even Ask before each change doesn't ask")
        with mock.patch.dict(os.environ, {"HERMES_SINGLE_QUERY_SESSION": "1"}):
            self.assertEqual(self.hook(), {"action": "block", "message": self.policy.UNATTENDED})
        self.assertEqual(self.hook(task_id="t2", turn_id="s1:t2:9f3a1c2e"),
                         {"action": "block", "message": self.policy.FORKED})
        self.ctx.cron_execution = object()
        self.assertEqual(self.hook(), {"action": "block", "message": self.tools.SCHEDULED})
        self.ctx.cron_execution = None
        for surface in ("api_server", "webhook"):
            with self.subTest(surface=surface):
                self.ctx.surface = surface
                self.assertEqual(self.hook(), {"action": "block", "message": self.policy.UNATTENDED})

    def test_it_makes_a_sound_from_ids_and_refuses_a_file_or_a_field_it_doesnt_take(self) -> None:
        made = support.call(self.host, "kumi_design", {**self.args, "seed": 11}, session_id="s1")
        self.assertTrue(made["artifact_id"].startswith("a-design"))
        self.assertEqual((made["seed"], made["sources"], made["mode"]), (11, self.args["sources"], "attack"))
        for bad in ({**self.args, "sources": [self.args["sources"][0], {"file": "/Samples/Drums/kick.wav"}]},
                    {**self.args, "out": "/tmp/x.wav"}):
            with self.subTest(bad=bad):
                self.assertEqual(support.call(self.host, "kumi_design", bad, session_id="s1")["error"],
                                 {"code": "invalid", "reason": self.tools.DESIGN_FIELDS_ONLY})
        self.assertEqual(len(self.designed()), 1, "serve was asked once, for the good call")

    def test_a_design_made_in_the_dashboard_reaches_serve_as_its_kept_file_and_comes_back_as_its_id(self) -> None:
        state = self.data / "state"
        ui_made, never = "a-d" + "C" * 22, "a-d" + "D" * 22
        listed(state, {"artifact_id": ui_made, "op": "stack", "seed": 1, "recipe": "Stack of 2"})
        sources = [{"artifact_id": ui_made, "name": "Made"}, {"artifact_id": never}]
        made = support.call(self.host, "kumi_design", {"op": "stack", "sources": sources}, session_id="s1")
        [sent] = self.designed()
        self.assertEqual(sent["sources"], [{"name": "Made", "file": os.path.realpath(state / "designs" / f"{ui_made}.wav")},
                                           {"artifact_id": never}],
                         "only a listed design becomes its file; anything else is left for serve to refuse")
        self.assertEqual(made["sources"], sources, "the model sees ids, never the file")
        self.assertNotIn(str(state), json.dumps(made).replace(made.get("media_tag", ""), ""))


if __name__ == "__main__":
    unittest.main()
