"""palettes.py and kumi_palette: the palette the map UI and the agent share, with native Kumi's semantics (its words,
duplicates by file, excluded sounds, the gather cap, `Palette N`, names without regard to case, moved files re-found),
plus the plugin's order and shared current palette; the tool's reads and edits, what policy lets through, and the
turn's note."""
from __future__ import annotations

import json
import os
from pathlib import Path
import tempfile
import threading
import unittest
from unittest import mock

import support

palettes = support.module("palettes")
snapshots = support.module("snapshots")
policy = support.module("policy")
tools = support.module("tools")
context = support.module("context")

# (name, class, folder, bytes): a small map's sounds; the index is the sound's number and its group.
SOUNDS = [("Boom Kick 01", "kick", "Drums/Kicks", 100), ("Tight Kick 02", "kick", "Drums/Kicks", 120),
          ("Dusty Snare 01", "snare", "Drums/Snares", 140), ("Crack Snare 02", "snare", "Drums/Snares", 160),
          ("Closed Hat 01", "hat", "Drums/Hats", 180), ("Warm Pad 01", "pad", "Pads", 200)]
GROUPS = ["kick", "snare", "hat", "pad"]


def publish_map(state: Path, library: Path, revision: str, *, folder_of: dict | None = None,
                missing: tuple[str, ...] = ()) -> list[str]:
    """A map revision as `kumi serve` publishes it (the private file, with each sound's path), its files written into
    `library` unless `missing`; `folder_of` moves a sound's file. Returns its sound ids in SOUNDS's order."""
    points, groups, sounds, ids = [], {}, {}, []
    for number, (name, kind, folder, size) in enumerate(SOUNDS, 1):
        sound_id = f"s-{revision}-{number:04d}"
        ids.append(sound_id)
        folder = (folder_of or {}).get(name, folder)
        path = library / folder / f"{name}.wav"
        if name not in missing:
            path.parent.mkdir(parents=True, exist_ok=True)
            if not path.exists():
                path.write_bytes(b"\0" * size)
        group = GROUPS.index(kind)
        groups[str(group)] = {"name": kind, "color": [1, 2, 3], "count": 1}
        points.append([sound_id, number / 10, number / 10, group])
        sounds[sound_id] = {"name": name, "kind": "one-shot", "class": kind, "folder": folder, "duration": 0.5,
                            "path": str(path)}
    (state / "map").mkdir(parents=True, exist_ok=True)
    (state / "map" / f"{revision}.json").write_text(json.dumps(
        {"schema": 1, "seq": 1, "at": 0, "owner": {}, "revision": revision, "made": 0, "points": points,
         "groups": groups, "sounds": sounds}), encoding="utf-8")
    (state / "map" / "current").write_text(f"{revision}\n", encoding="utf-8")
    return ids


class Semantics(unittest.TestCase):
    def setUp(self):
        folder = tempfile.TemporaryDirectory()
        self.addCleanup(folder.cleanup)
        self.state = Path(folder.name) / "state"
        self.library = Path(folder.name) / "samples"
        self.ids = publish_map(self.state, self.library, "r1")

    def map(self):
        return palettes.current_map(self.state, snapshots.map_current(self.state),
                                    lambda revision: snapshots.map_revision(self.state, revision))

    def gather(self, *ids: str, artifacts=()) -> str:
        return palettes.gather(self.state, self.map(), list(ids), list(artifacts))

    def view(self) -> dict:
        return palettes.view(self.state, self.map())

    def test_a_gather_makes_palette_1_and_says_what_it_had_in_natives_words(self):
        self.assertEqual(self.gather(*self.ids[:3]), "Added 3 sounds to Palette 1.")
        self.assertEqual(self.gather(self.ids[0]), "Palette 1 has that sound already.")
        self.assertEqual(self.gather(*self.ids[:2]), "Palette 1 has those 2 sounds already.")
        self.assertEqual(self.gather(self.ids[0], self.ids[3]), "Added 1 sound to Palette 1 (1 it had already).")
        view = self.view()
        self.assertEqual([item["sound_id"] for item in view["current"]["items"]], self.ids[:4])
        self.assertEqual([item["group"] for item in view["current"]["items"]], [0, 0, 1, 1])
        self.assertEqual(view["palettes"], [{"id": view["current"]["id"], "name": "Palette 1", "items": 4,
                                             "current": True}])
        self.assertEqual((view["seq"], view["map_revision"]), (4, "r1"), "every write bumps seq")
        self.assertNotIn(str(self.library), json.dumps(view), "never a path")
        self.assertEqual(snapshots.status(self.state)["revisions"]["palette"], 4)

    def test_the_same_path_with_another_size_is_another_file_as_native_keys_it(self):
        self.gather(self.ids[0])
        (self.library / "Drums/Kicks/Boom Kick 01.wav").write_bytes(b"\1" * 999)
        self.assertEqual(self.gather(self.ids[0]), "Added 1 sound to Palette 1.")
        self.assertEqual(len(palettes.load(self.state)["palettes"][0]["items"]), 2)

    def test_an_excluded_sound_leaves_the_items_and_a_gather_brings_it_back_in_its_place(self):
        self.gather(*self.ids[:3])
        self.assertEqual(palettes.exclude_sounds(self.state, self.map(), [self.ids[0], self.ids[5]]), 2)
        current = self.view()["current"]
        self.assertEqual(([item["sound_id"] for item in current["items"]], current["excluded"]), (self.ids[1:3], 2))
        self.assertEqual(self.gather(self.ids[0]), "Added 1 sound to Palette 1.")
        self.assertEqual([item["sound_id"] for item in self.view()["current"]["items"]], self.ids[:3])

    def test_one_gather_takes_the_first_10000_picked(self):
        many = {"sounds": {f"s-r9-{n}": {"name": f"Hit {n}", "class": "perc", "path": f"/nowhere/{n}.wav"}
                           for n in range(palettes.MOST_GATHERED + 1)}, "points": []}
        the_map = palettes.Map("r9", many)
        said = palettes.gather(self.state, the_map, list(many["sounds"]))
        self.assertEqual(said, "Added 10000 sounds to Palette 1. The first 10,000 of 10,001 picked went in.")
        with self.assertRaises(palettes.Invalid):
            palettes.gather(self.state, the_map, [], [("a", "x")] * (palettes.MOST_ARTIFACTS + 1))

    def test_ids_off_the_current_map_are_refused_whole(self):
        with self.assertRaises(palettes.OffMap):
            self.gather(self.ids[0], "s-r0-0001")
        self.assertEqual(palettes.load(self.state)["seq"], 0, "nothing written")

    def test_names_are_trimmed_unique_without_regard_to_case_and_palette_n_skips_taken_ones(self):
        palettes.new(self.state, "PALETTE 1")
        self.assertEqual(palettes.new(self.state, "  Drums "), "Drums is the palette now: gather sounds on the map "
                                                               "to add them.")
        with self.assertRaisesRegex(palettes.Invalid, "There's a palette called Drums already"):
            palettes.new(self.state, "drums")
        for bad in ("", "   ", "x" * 81, "two\nlines"):
            with self.subTest(name=bad), self.assertRaises(palettes.Invalid):
                palettes.new(self.state, bad)
        with self.assertRaisesRegex(palettes.Invalid, "There's a palette called PALETTE 1 already"):
            palettes.rename(self.state, "palette 1")
        self.assertEqual(palettes.rename(self.state, "drums kit"), "The palette is called drums kit now.")
        self.assertEqual(palettes.forget(self.state, "DRUMS KIT"), "Forgot the palette drums kit and its 0 sounds.")
        self.assertIsNone(self.view()["current"], "forgetting the current palette leaves none")
        self.assertEqual(self.gather(self.ids[0]), "Added 1 sound to Palette 2.")
        first = next(p for p in self.view()["palettes"] if p["name"] == "PALETTE 1")
        self.assertEqual(palettes.use(self.state, first["id"]), "PALETTE 1 (0 sounds) is the palette now.")
        self.assertEqual(palettes.use(self.state, "palette 2"), "Palette 2 (1 sound) is the palette now.")
        with self.assertRaisesRegex(palettes.Invalid, "There's no palette called Bass"):
            palettes.use(self.state, "Bass")

    def test_edits_of_the_current_palette_need_one(self):
        for edit in (lambda: palettes.rename(self.state, "X"), lambda: palettes.reach(self.state, 3),
                     lambda: palettes.move(self.state, "it-000000000000", 0),
                     lambda: palettes.remove_keys(self.state, ["it-000000000000"])):
            with self.assertRaisesRegex(palettes.NoPalette, "There's no palette yet"):
                edit()

    def test_reach_is_strict_or_loose_up_to_50_and_says_its_mode(self):
        self.gather(self.ids[0])
        self.assertEqual(palettes.reach(self.state, 5), "Loose: each of the palette's sounds brings its 5 nearest, "
                                                        "and Kumi says when it uses one of those.")
        self.assertEqual((self.view()["current"]["reach"], self.view()["current"]["mode"]), (5, "loose, 5 nearest"))
        self.assertEqual(palettes.reach(self.state, 0), "Strict: Kumi keeps to the palette's own sounds.")
        self.assertEqual(self.view()["current"]["mode"], "strict")
        for bad in (51, -1, True, "3"):
            with self.subTest(reach=bad), self.assertRaises(palettes.Invalid):
                palettes.reach(self.state, bad)

    def test_items_move_to_a_place_among_those_shown_and_remove_by_key(self):
        self.gather(*self.ids[:4])
        palettes.exclude_sounds(self.state, self.map(), [self.ids[1]])
        keys = [item["key"] for item in self.view()["current"]["items"]]  # sounds 1, 3, 4
        self.assertEqual(palettes.move(self.state, keys[2], 0), "Moved Crack Snare 02 to place 1 in Palette 1.")
        self.assertEqual([i["key"] for i in self.view()["current"]["items"]], [keys[2], keys[0], keys[1]])
        palettes.move(self.state, keys[2], 99)
        self.assertEqual([i["key"] for i in self.view()["current"]["items"]], [keys[0], keys[1], keys[2]])
        with self.assertRaises(palettes.Invalid):
            palettes.move(self.state, "it-ffffffffffff", 0)
        self.assertEqual(palettes.remove_keys(self.state, [keys[0], keys[1]]), "Took 2 sounds out of Palette 1.")
        with self.assertRaisesRegex(palettes.Invalid, "has none of those"):
            palettes.remove_keys(self.state, [keys[0]])
        self.assertEqual(palettes.remove_sounds(self.state, self.map(), [self.ids[3], self.ids[5]]), 1)
        self.assertEqual((self.view()["current"]["items"], self.view()["current"]["excluded"]), ([], 1))

    def test_a_moved_file_is_found_again_by_name_and_size_and_nothing_is_ever_pruned(self):
        self.gather(*self.ids[:3])
        old = self.library / "Drums/Kicks/Boom Kick 01.wav"
        moved = self.library / "Moved/Boom Kick 01.wav"
        moved.parent.mkdir(parents=True)
        os.replace(old, moved)
        (self.library / "Drums/Kicks/Tight Kick 02.wav").unlink()  # gone, and the new map's file differs in size
        (self.library / "Other/Tight Kick 02.wav").parent.mkdir()
        (self.library / "Other/Tight Kick 02.wav").write_bytes(b"\0" * 7)
        ids = publish_map(self.state, self.library, "r2",
                          folder_of={"Boom Kick 01": "Moved", "Tight Kick 02": "Other"})
        items = self.view()["current"]["items"]
        self.assertEqual([item["sound_id"] for item in items], [ids[0], None, ids[2]])
        self.assertEqual([item["name"] for item in items], ["Boom Kick 01", "Tight Kick 02", "Dusty Snare 01"])
        kept = palettes.load(self.state)["palettes"][0]["items"]
        self.assertEqual(kept[0]["path"], str(moved))
        self.assertTrue(kept[1]["path"].endswith(os.path.join("Kicks", "Tight Kick 02.wav")), "stays as it was")

    def test_artifacts_go_in_by_id_once(self):
        self.assertEqual(self.gather(artifacts=[("a-1", "dusty layer")]), "Added 1 sound to Palette 1.")
        self.assertEqual(self.gather(self.ids[0], artifacts=[("a-1", "dusty layer")]),
                         "Added 1 sound to Palette 1 (1 it had already).")
        [artifact, sound] = self.view()["current"]["items"]
        self.assertEqual({k: artifact[k] for k in ("source", "sound_id", "artifact_id", "name", "class", "group")},
                         {"source": "artifact", "sound_id": None, "artifact_id": "a-1", "name": "dusty layer",
                          "class": None, "group": None})
        self.assertEqual((sound["source"], sound["artifact_id"]), ("sound", None))

    def test_writes_at_once_from_two_places_all_land_under_the_lock(self):
        failures = []

        def one(sound_id):
            try:
                self.gather(sound_id)
            except Exception as failure:  # noqa: BLE001 - reported below
                failures.append(failure)

        threads = [threading.Thread(target=one, args=(sound_id,)) for sound_id in self.ids]
        for thread in threads:
            thread.start()
        for thread in threads:
            thread.join()
        self.assertEqual(failures, [])
        data = palettes.load(self.state)
        self.assertEqual((len(data["palettes"]), len(data["palettes"][0]["items"]), data["seq"]), (1, 6, 6))

    def test_a_file_it_cant_read_is_kept_aside_not_written_over(self):
        self.state.mkdir(parents=True, exist_ok=True)
        (self.state / "palettes.json").write_text("{not json", encoding="utf-8")
        with self.assertLogs(palettes.log, "WARNING"):
            self.assertIsNone(self.view()["current"])
            self.gather(self.ids[0])
        self.assertEqual((self.state / "palettes.json.unreadable").read_text(encoding="utf-8"), "{not json")

    def test_the_brief_names_the_palette_its_classes_mode_and_excluded_count(self):
        self.assertIsNone(palettes.brief(self.state))
        palettes.new(self.state, "Drums")
        self.assertIsNone(palettes.brief(self.state), "nothing in it")
        self.gather(*self.ids[:5])
        palettes.exclude_sounds(self.state, self.map(), [self.ids[4]])
        name, said = palettes.brief(self.state)
        self.assertEqual(name, "Drums")
        self.assertTrue(said.startswith("4 sounds (2 kick, 2 snare). It's strict:"), said)
        self.assertTrue(said.endswith(" Never use the sound it excludes."), said)
        palettes.reach(self.state, 3)
        self.assertIn("It's loose: these, and each one's 3 nearest", palettes.brief(self.state)[1])


class Tool(unittest.TestCase):
    def setUp(self):
        self.host, self.ctx, self.data = support.host(self, ask_before_changes=True)
        self.ctx.profile_name = "studio"
        self.state = self.data / "state"
        self.library = self.data / "samples"
        self.ids = publish_map(self.state, self.library, "r7")
        self.sent: list[tuple[str, dict]] = []
        patcher = mock.patch.object(tools, "broadcast", lambda event, payload: self.sent.append((event, payload)))
        patcher.start()
        self.addCleanup(patcher.stop)

    def palette(self, **args) -> dict:
        return support.call(self.host, "kumi_palette", args, session_id="s1")

    def test_with_no_palette_reads_say_how_to_make_one(self):
        self.assertEqual(self.palette(action="list"), {"palette": None, "say": tools.PALETTE_NONE})
        self.assertEqual(self.palette(action="strict"), {"palette": None, "say": tools.PALETTE_NONE})

    def test_add_list_search_exclude_and_remove_speak_in_sound_ids_never_paths(self):
        self.assertEqual(self.palette(action="add", sound_ids=self.ids[:4]), {"said": "Added 4 sounds to Palette 1."})
        self.assertEqual(self.sent, [("palette.changed", {"seq": 1, "profile": "studio"})])
        self.assertEqual(self.palette(action="add", sound_ids=["s-r6-0001"])["error"]["code"], "not_on_map")
        self.assertEqual(self.palette(action="exclude", sound_ids=[self.ids[3]]), {"excluded": 1})
        listed = self.palette(action="list", limit=2)
        self.assertEqual(listed["palette"], {"name": "Palette 1", "mode": "strict", "items": 3})
        self.assertEqual(listed["map_revision"], "r7")
        self.assertEqual([(s["sound_id"], s["name"], s["class"]) for s in listed["sounds"]],
                         [(self.ids[0], "Boom Kick 01", "kick"), (self.ids[1], "Tight Kick 02", "kick")])
        self.assertEqual((listed["more"], [s["sound_id"] for s in listed["excluded"]]), (1, [self.ids[3]]))
        self.assertEqual(listed["palettes"], [{"name": "Palette 1", "items": 3, "current": True}])
        self.assertNotIn(str(self.library), json.dumps(listed))
        found = self.palette(action="search", words="Snares dusty")  # the map's folder and the name
        self.assertEqual([s["sound_id"] for s in found["sounds"]], [self.ids[2]])
        self.assertEqual(self.palette(action="search")["error"]["code"], "invalid")
        self.assertEqual(self.palette(action="remove", sound_ids=[self.ids[0], self.ids[5]]), {"removed": 1})
        key = self.palette(action="list")["sounds"][0]["key"]
        self.assertEqual(self.palette(action="remove", keys=[key]), {"said": "Took 1 sound out of Palette 1."})
        self.assertEqual([event for event, _ in self.sent], ["palette.changed"] * 4, "every edit, never a read")

    def test_palettes_are_made_used_renamed_moved_and_forgotten_by_name(self):
        self.assertEqual(self.palette(action="new", name="Drums")["said"],
                         "Drums is the palette now: gather sounds on the map to add them.")
        self.palette(action="add", sound_ids=self.ids[:2])
        self.palette(action="new", name="Pads")
        self.assertEqual(self.palette(action="use", name="drums"), {"said": "Drums (2 sounds) is the palette now."})
        self.assertEqual(self.palette(action="rename", name="Kit"), {"said": "The palette is called Kit now."})
        keys = [s["key"] for s in self.palette(action="list")["sounds"]]
        self.palette(action="move", key=keys[1], to=0)
        self.assertEqual([s["key"] for s in self.palette(action="list")["sounds"]], [keys[1], keys[0]])
        self.assertEqual(self.palette(action="loose")["said"][:24], "Loose: each of the palet")
        self.assertEqual(self.palette(action="list")["palette"]["mode"], "loose, 5 nearest")
        self.assertEqual(self.palette(action="loose", reach=51)["error"]["code"], "invalid")
        self.assertEqual(self.palette(action="forget", name="pads"), {"said": "Forgot the palette Pads and its 0 sounds."})
        self.assertEqual(self.palette(action="new", name="kit")["error"]["code"], "invalid")
        self.assertEqual(self.palette(action="new")["error"]["code"], "invalid")
        self.assertEqual(self.palette(action="dance")["error"]["code"], "invalid")

    def test_kit_lists_this_maps_sounds_in_order_with_the_dAWs_way_to_load_them(self):
        self.palette(action="add", sound_ids=self.ids[:3], artifact_ids=None)
        self.palette(action="exclude", sound_ids=[self.ids[1]])
        publish_map(self.state, self.library, "r8", missing=("Dusty Snare 01",))  # a rescan: new ids
        kit = self.palette(action="kit")
        self.assertEqual(kit["map_revision"], "r8")
        self.assertEqual(kit["sounds"], [{"sound_id": "s-r8-0001", "name": "Boom Kick 01", "class": "kick"},
                                         {"sound_id": "s-r8-0003", "name": "Dusty Snare 01", "class": "snare"}])
        self.assertEqual((kit["left_out"], kit["say"]), (0, tools.KIT_SAY["ableton"]))
        self.assertIn("load_sample_to_pad", kit["say"])
        self.ctx.settings["daw"] = "renoise"
        self.assertEqual(self.palette(action="kit")["say"], tools.KIT_SAY["renoise"])
        self.ctx.settings["daw"] = "reaper"
        self.assertIn("item_import", self.palette(action="kit")["say"])

    def test_neighbours_of_a_loose_palette_leave_out_its_own_and_rank_by_how_many_they_are_near(self):
        self.palette(action="add", sound_ids=self.ids[:2])
        self.assertEqual(self.palette(action="neighbours")["say"], tools.PALETTE_STRICT)
        self.palette(action="exclude", sound_ids=[self.ids[2]])
        self.palette(action="loose", reach=2)
        support.call(self.host, "kumi_connect", {}, session_id="s1")
        near = self.palette(action="neighbours")
        # Points sit on a line: 1's nearest not its own or excluded are 4 and 5; 2's are 4 and 5 too.
        self.assertEqual([s["sound_id"] for s in near["sounds"]], [self.ids[3], self.ids[4]])
        self.assertEqual(near["sounds"][0]["near"], ["Boom Kick 01", "Tight Kick 02"])
        asked = [r for r in support.requests(self.state) if r.get("params", {}).get("name") == "sounds"]
        self.assertEqual([r["params"]["arguments"]["near"] for r in asked], self.ids[:2])
        self.assertEqual(asked[0]["params"]["arguments"]["limit"], 4)

    def test_reads_pass_like_kumi_sounds_and_edits_are_never_asked_but_need_someone_in_the_chat(self):
        hook = {"session_id": "s1", "tool_call_id": "c1", "task_id": "t1", "turn_id": "u1"}
        policy.turn_started(self.host, session_id="s1", turn_id="u1")
        for action in ("list", "search", "neighbours", "kit", "add", "move", "forget"):
            with self.subTest(action=action):  # "Ask before each change" is on: still never asked
                self.assertIsNone(policy.pre_tool_call(self.host, tool_name="kumi_palette",
                                                       args={"action": action}, **hook))
        forked = {**hook, "turn_id": "s1:t2:9f3a1c2e"}
        self.assertIsNone(policy.pre_tool_call(self.host, tool_name="kumi_palette", args={"action": "list"}, **forked))
        for action in ("add", "remove", "exclude", "new", "loose"):
            with self.subTest(action=action):
                self.assertEqual(policy.pre_tool_call(self.host, tool_name="kumi_palette", args={"action": action},
                                                      **forked), {"action": "block", "message": policy.FORKED})
        with mock.patch.dict(os.environ, {"HERMES_SINGLE_QUERY_SESSION": "1"}):
            self.assertEqual(policy.pre_tool_call(self.host, tool_name="kumi_palette", args={"action": "add"},
                                                  **hook)["message"], policy.UNATTENDED)
            self.assertIsNone(policy.pre_tool_call(self.host, tool_name="kumi_palette", args={"action": "list"},
                                                   **hook))
        self.ctx.cron_execution = object()
        self.assertEqual(policy.pre_tool_call(self.host, tool_name="kumi_palette", args={"action": "add"},
                                              **hook)["message"], tools.SCHEDULED)
        self.assertEqual(support.handle(self.host, "kumi_palette", {"action": "add", "sound_ids": self.ids[:1]},
                                        session_id="s1")["error"]["code"], "scheduled")
        self.assertEqual(palettes.load(self.state)["seq"], 0, "nothing was written")

    def test_the_turns_note_names_the_palette_and_comes_again_when_it_changes(self):
        def note():
            added = context.pre_llm_call(self.host, session_id="s1", task_id="t1", user_message="hi",
                                         conversation_history=[], is_first_turn=False, model="m", platform="cli")
            return added["context"] if added else None

        support.call(self.host, "kumi_connect", {}, session_id="s1")
        self.assertNotIn("palette", note())
        self.palette(action="new", name="<Drums>")
        self.assertIsNone(note(), "an empty palette adds nothing")
        self.palette(action="add", sound_ids=self.ids[:3])
        line = note().splitlines()[-2]
        self.assertEqual(line, 'The producer\'s palette, "\\u003cDrums\\u003e": 3 sounds (2 kick, 1 snare). It\'s '
                               "strict: when they ask for sounds, use these (kumi_palette lists and searches them), "
                               "and look further only when they ask you to.")
        self.assertIsNone(note())
        self.palette(action="exclude", sound_ids=[self.ids[0]])
        self.assertIn("2 sounds (1 kick, 1 snare)", note())


if __name__ == "__main__":
    unittest.main()
