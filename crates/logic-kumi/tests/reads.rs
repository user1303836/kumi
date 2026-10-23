//! The bridge over recorded scenes of the real Logic Pro, without Logic: the fixed request set asked in order over
//! the `scratch` scene, the shapes checked against the host simulator's, and the answers blessed into
//! `crates/daw-mcp-server/tests/vectors/logic-reads.json` with `KUMI_BLESS=1` (the test fails when the file differs
//! without it). The other scenes check what changes between them: the selection, the transport, a dialog.

use std::path::PathBuf;

use logic_kumi::ax::strings::Strings;
use logic_kumi::fixtures::{Scene, SceneHands};
use logic_kumi::helper::Launch;
use logic_kumi::scenes;
use serde_json::{json, Value};

fn fixtures() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

fn vectors_path() -> PathBuf {
    kumi_testkit::workspace_path("crates/daw-mcp-server/tests/vectors/logic-reads.json")
}

/// The second slice's reads, blessed beside this crate's scenes: the host's replay of them is the wiring side's.
fn m4b_vectors_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/vectors/logic-reads-m4b.json")
}

/// The fixed request set over a scene: the cases, and the requests the scene couldn't answer.
fn run(name: &str) -> (Vec<Value>, Vec<(String, Value)>) {
    run_with(name, &scenes::fixed_requests())
}

/// A request set over a scene.
fn run_with(name: &str, requests: &[(&'static str, Value)]) -> (Vec<Value>, Vec<(String, Value)>) {
    let dir = fixtures().join(name);
    let scene = Scene::load(&dir).unwrap_or_else(|error| panic!("no scene {name}: {error}; record it with the record_fixtures example"));
    let pid = scene.logic.get("pid").and_then(Value::as_u64).unwrap_or(1) as u32;
    let language = scene.logic.get("language").and_then(Value::as_str).unwrap_or("en");
    assert_eq!(language, "en", "the scenes are recorded from Logic in English");
    let project = scene.logic.get("project").cloned().unwrap_or(Value::Null);
    let (id, path, pname) = (
        project.get("id").and_then(Value::as_str).map(str::to_string),
        project.get("path").and_then(Value::as_str).map(str::to_string),
        project.get("name").and_then(Value::as_str).unwrap_or_default().to_string(),
    );
    let status = json!({"project": project, "busy": false, "permission": null});
    let hands = SceneHands::new(&scene);
    let mut helper = Launch::launch(&mut hands.clone()).unwrap();
    logic_kumi::changes::set_settling(false);
    // What the recording saw outside Logic (its preferences), answered back; a scene recorded before kept none.
    let looked: Vec<Value> = scene.logic.get("disk").and_then(Value::as_array).cloned().unwrap_or_default();
    logic_kumi::disk::set_hook(Some(logic_kumi::disk::replaying(&looked)));
    let cases = scenes::run_set(helper.as_mut(), pid, Strings::english(), status, (id.as_deref(), path.as_deref(), &pname), requests);
    logic_kumi::disk::set_hook(None);
    (cases, hands.unrecorded())
}

fn result(case: &Value) -> &Value {
    &case["response"]["result"]
}

#[test]
fn the_fixed_request_set_over_scratch_answers_in_the_simulators_shapes_and_is_the_vectors() {
    let (cases, unrecorded) = run("scratch");
    assert!(unrecorded.is_empty(), "the scene didn't record what the readers ask now; record it again: {unrecorded:?}");
    assert_eq!(cases.len(), 26);
    let by_request: Vec<(String, Value, Value)> = cases
        .iter()
        .map(|c| (c["request"]["method"].as_str().unwrap().to_string(), c["request"]["params"].clone(), c["response"].clone()))
        .collect();
    let find = |method: &str, params: Value| -> Value {
        by_request
            .iter()
            .find(|(m, p, _)| m == method && *p == params)
            .map(|(_, _, r)| r.clone())
            .unwrap_or_else(|| panic!("no case {method} {params}"))
    };

    let status = find("status", json!({}));
    assert_eq!(status["result"].as_object().unwrap().keys().collect::<Vec<_>>(), ["project", "busy", "permission"]);

    let project = find("read", json!({"kind": "project"}));
    let row = &project["result"]["items"][0];
    for key in ["id", "identity", "path", "tempo", "timeSignature", "keySignature", "sampleRate", "trackCount"] {
        assert!(row.get(key).is_some(), "project row lacks {key}");
    }
    assert_eq!(
        (row["identity"].as_str(), row["tempo"].as_f64(), row["timeSignature"].as_str()),
        (Some("Scratch"), Some(160.0), Some("4/4"))
    );
    assert_eq!((project["result"]["total"].as_u64(), project["result"]["epoch"].as_str()), (Some(1), Some("1")));

    let tracks = find("read", json!({"kind": "track"}));
    let items = tracks["result"]["items"].as_array().unwrap();
    assert_eq!(items.len(), 2);
    for key in
        ["ref", "index", "name", "kind", "mute", "solo", "armed", "monitoring", "volumeDb", "pan", "color", "output", "inputs", "selected"]
    {
        assert!(items[0].get(key).is_some(), "track row lacks {key}");
    }
    assert_eq!(
        (items[0]["ref"].as_str(), items[0]["name"].as_str(), items[0]["kind"].as_str(), items[0]["selected"].as_bool()),
        (Some("1:track:1"), Some("Vocal"), Some("audio"), Some(true))
    );
    assert_eq!(
        (items[1]["ref"].as_str(), items[1]["kind"].as_str(), items[1]["selected"].as_bool()),
        (Some("1:track:2"), Some("instrument"), Some(false))
    );
    let first = find("read", json!({"kind": "track", "limit": 1}));
    assert_eq!((first["result"]["items"].as_array().unwrap().len(), first["result"]["nextCursor"].as_str()), (1, Some("1.1")));
    let second = find("read", json!({"kind": "track", "cursor": "1.1"}));
    assert_eq!((second["result"]["items"][0]["ref"].as_str(), second["result"].get("nextCursor")), (Some("1:track:2"), None));

    let regions = find("read", json!({"kind": "region"}));
    let region = &regions["result"]["items"][0];
    for key in ["ref", "trackRef", "name", "startBeats", "endBeats", "kind", "selected", "looped"] {
        assert!(region.get(key).is_some(), "region row lacks {key}");
    }
    assert_eq!(
        (region["ref"].as_str(), region["trackRef"].as_str(), region["name"].as_str(), region["kind"].as_str()),
        (Some("1:region:1"), Some("1:track:2"), Some("Keys"), Some("midi"))
    );
    assert_eq!((region["startBeats"].as_f64(), region["endBeats"].as_f64()), (Some(0.0), Some(16.0)));
    assert_eq!(find("read", json!({"kind": "region", "parent": "1:track:2"}))["result"]["total"], 1);

    let refused = find("read", json!({"kind": "note"}));
    assert_eq!(refused, json!({"ok": false, "error": {"code": "bad-params", "message": "note: pass the parent region"}}));
    let notes = find("read", json!({"kind": "note", "parent": "1:region:1"}));
    let notes = notes["result"]["items"].as_array().unwrap();
    assert_eq!(notes.len(), 4);
    for key in ["ref", "regionRef", "position", "pitch", "velocity", "lengthTicks", "channel"] {
        assert!(notes[0].get(key).is_some(), "note row lacks {key}");
    }
    let seen: Vec<(u64, u64, u64, u64)> = notes
        .iter()
        .map(|n| {
            (
                n["position"]["beat"].as_u64().unwrap(),
                n["pitch"].as_u64().unwrap(),
                n["velocity"].as_u64().unwrap(),
                n["lengthTicks"].as_u64().unwrap(),
            )
        })
        .collect();
    assert_eq!(seen, [(1, 60, 100, 240), (2, 64, 96, 240), (3, 67, 92, 240), (4, 72, 110, 480)]);
    assert_eq!(notes[0]["ref"], "1:note:1.1");

    let plugins = find("read", json!({"kind": "plugin", "parent": "1:track:1"}));
    let plugin = &plugins["result"]["items"][0];
    assert_eq!(
        (plugin["ref"].as_str(), plugin["slot"].as_u64(), plugin["name"].as_str(), plugin["bypassed"].as_bool(), plugin["kind"].as_str()),
        (Some("1:plugin:1.1"), Some(1), Some("Channel EQ"), Some(false), Some("effect"))
    );
    let sends = find("read", json!({"kind": "send", "parent": "1:track:1"}));
    assert!(sends["result"]["items"].is_array());

    let markers = find("read", json!({"kind": "marker"}));
    let marker = &markers["result"]["items"][0];
    for key in ["ref", "position", "name", "length", "text"] {
        assert!(marker.get(key).is_some(), "marker row lacks {key}");
    }
    assert_eq!((marker["position"]["bar"].as_u64(), marker["length"].as_f64()), (Some(9), Some(32.0)));

    let tempo = find("read", json!({"kind": "tempo"}));
    assert_eq!(tempo["result"]["items"][0]["bpm"], 160.0);
    assert_eq!(tempo["result"]["items"][0]["signatures"][0]["time"], "4/4");

    assert_eq!(find("read", json!({"ref": "1:track:2"}))["result"]["items"][0]["name"], "Keys");
    assert_eq!(
        find("read", json!({"ref": "1:track:9"})),
        json!({"ok": false, "error": {"code": "failed", "message": "1:track:9 isn't in the project (deleted, or another project)"}})
    );
    assert_eq!(find("read", json!({"kind": "bogus"}))["error"]["message"], "Logic Pro has no kind \"bogus\" to read");
    assert_eq!(
        find("read", json!({"kind": "region", "parent": "0:track:2"})),
        json!({"ok": false, "error": {"code": "stale", "message": "that ref is from an earlier epoch (0; now 1): read again"}})
    );

    let snapshot = find("snapshot", json!({}));
    assert_eq!(
        snapshot["result"].as_object().unwrap().keys().collect::<Vec<_>>(),
        ["epoch", "project", "transport", "tracks", "selection", "markers"]
    );
    assert_eq!(snapshot["result"]["selection"]["trackRef"], "1:track:1");
    let focus = find("focus", json!({}));
    for key in ["epoch", "view", "editor", "selection", "playhead", "transport", "changed", "ageMs"] {
        assert!(focus["result"].get(key).is_some(), "focus lacks {key}");
    }
    assert_eq!(
        (focus["result"]["view"].as_str(), focus["result"]["selection"]["trackRef"].as_str(), focus["result"]["ageMs"].as_u64()),
        (Some("tracks"), Some("1:track:1"), Some(0))
    );

    let tree = find("focus_body", json!({"kind": "tree", "ref": "1:track:1"}));
    assert_eq!(tree["result"]["strip"]["inserts"][0]["name"], "Channel EQ");
    assert_eq!(find("focus_body", json!({"kind": "tree"}))["result"]["trackRef"], "1:track:1");
    let body_notes = find("focus_body", json!({"kind": "notes", "ref": "1:region:1"}));
    assert_eq!((body_notes["result"]["total"].as_u64(), body_notes["result"]["notes"].as_array().unwrap().len()), (Some(4), 4));
    assert_eq!(find("focus_body", json!({"kind": "notes"}))["result"]["regionRef"], "1:region:1");
    let timeline = find("focus_body", json!({"kind": "timeline"}));
    assert_eq!(timeline["result"]["lanes"].as_array().unwrap().len(), 2);
    let around = find("focus_body", json!({"kind": "timeline", "around": "1:track:2", "window": {"from": 0, "to": 16}, "limit": 1}));
    assert_eq!((around["result"]["lanes"].as_array().unwrap().len(), around["result"]["lanes"][0]["name"].as_str()), (1, Some("Keys")));
    let routing = find("focus_body", json!({"kind": "routing", "ref": "1:track:1"}));
    assert_eq!(routing["result"]["inputs"], json!(["Input 1"]));

    // The vectors: blessed from these answers, or checked against the file.
    let text = scenes::vectors_text(&cases);
    let path = vectors_path();
    if std::env::var("KUMI_BLESS").is_ok_and(|v| v == "1") {
        std::fs::write(&path, &text).unwrap();
    } else {
        let current =
            std::fs::read_to_string(&path).unwrap_or_else(|_| panic!("no vectors at {}: bless them with KUMI_BLESS=1", path.display()));
        assert_eq!(current, text, "the vectors differ from what the bridge answers over the scratch scene: bless them with KUMI_BLESS=1");
    }
}

#[test]
fn the_other_scenes_change_the_selection_the_transport_and_the_busy_state() {
    for (scene, check) in [
        (
            "scratch-keys",
            (|focus: &Value| {
                assert_eq!(focus["selection"]["trackRef"], "1:track:2", "{focus}");
                assert_eq!(focus["selection"]["regionRef"], "1:region:1", "{focus}");
                assert_eq!(focus["editor"], "pianoRoll", "{focus}");
            }) as fn(&Value),
        ),
        ("scratch-playing", |focus: &Value| {
            assert_eq!(focus["transport"]["playing"], true, "{focus}");
        }),
    ] {
        if !fixtures().join(scene).join(logic_kumi::fixtures::SCENE_FILE).is_file() {
            eprintln!("no {scene} scene recorded; skipped");
            continue;
        }
        let (cases, _) = run(scene);
        let focus = cases.iter().find(|c| c["request"]["method"] == "focus").map(result).unwrap().clone();
        check(&focus);
    }
}

#[test]
fn the_scenes_name_no_home_of_the_mac_they_were_recorded_on() {
    // The recorder saves every home as /Users/me (fixtures::redacted); a scene edited or written another way mustn't
    // bring one back.
    let mut scenes = 0;
    for entry in std::fs::read_dir(fixtures()).unwrap() {
        let dir = entry.unwrap().path();
        assert!(!dir.join("scene.json").exists(), "{} holds its scene uncompressed: gzip it", dir.display());
        let file = dir.join(logic_kumi::fixtures::SCENE_FILE);
        if !file.is_file() {
            continue;
        }
        let homes = logic_kumi::fixtures::homes_named(&logic_kumi::fixtures::scene_text(&dir).unwrap());
        assert!(homes.is_empty(), "{} names a home: /Users/{}", file.display(), homes.join(", /Users/"));
        scenes += 1;
    }
    assert!(scenes >= 4, "{scenes} scenes");
}

#[test]
fn the_second_slices_reads_over_scratch_are_a_plug_ins_parameters_its_smart_controls_and_automation() {
    let (cases, unrecorded) = run_with("scratch-m4b", &scenes::m4b_requests());
    assert!(unrecorded.is_empty(), "the scene didn't record what the readers ask now; record it again: {unrecorded:?}");
    let answer = |index: usize| cases[index]["response"].clone();
    let text = scenes::vectors_text(&cases);
    let path = m4b_vectors_path();
    if std::env::var("KUMI_BLESS").is_ok_and(|v| v == "1") {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, &text).unwrap();
    } else {
        let current =
            std::fs::read_to_string(&path).unwrap_or_else(|_| panic!("no vectors at {}: bless them with KUMI_BLESS=1", path.display()));
        assert_eq!(current, text, "the second slice's vectors differ from what the bridge answers: bless them with KUMI_BLESS=1");
    }
    // Keys' Piano, from its window's Controls View: a row per parameter with its range and text.
    let params = answer(0)["result"]["items"].as_array().unwrap().clone();
    assert_eq!(params.len(), 6);
    for key in ["ref", "pluginRef", "index", "name", "value", "text", "min", "max", "normalized"] {
        assert!(params[1].get(key).is_some(), "param row lacks {key}");
    }
    assert_eq!(
        (params[1]["ref"].as_str(), params[1]["pluginRef"].as_str(), params[1]["name"].as_str()),
        (Some("1:param:2.1.2"), Some("1:plugin:2.1"), Some("Main Volume"))
    );
    assert_eq!(params[0]["value"], Value::Null, "a menu parameter has its text alone");
    assert_eq!(answer(1)["result"]["nextCursor"], "1.2");
    assert_eq!(answer(2)["result"]["items"][0]["name"], "Main Volume");
    // Keys' Smart Controls: shares of their ranges with their texts.
    let smart = answer(3)["result"]["items"].as_array().unwrap().clone();
    assert!(!smart.is_empty());
    for key in ["ref", "trackRef", "index", "name", "value", "text"] {
        assert!(smart[0].get(key).is_some(), "smart row lacks {key}");
    }
    assert_eq!(smart[0]["trackRef"], "1:track:2");
    // Vocal has no automation; the Stereo Out no plug-ins.
    assert_eq!(answer(4)["result"]["total"], 0);
    assert_eq!(answer(5)["result"]["total"], 0);
    assert_eq!(answer(6)["error"]["message"], "param: pass the parent plug-in");
    assert_eq!(answer(7)["error"]["message"], "smart: pass the parent track");
}

#[test]
fn the_project_row_says_which_octave_logic_names_middle_c_in_from_its_display_setting() {
    // Recorded with Logic's "Display Middle C As" on C3 (Yamaha), its default: the preferences' MiddleC switch off.
    let (cases, _) = run_with("scratch", &[("read", json!({"kind": "project"}))]);
    assert_eq!(result(&cases[0])["items"][0]["middleC"], 3);
    // The same scene with the switch on (C4, Roland), as the lab's toggle of the setting wrote it.
    let scene = Scene::load(&fixtures().join("scratch")).unwrap();
    let pid = scene.logic.get("pid").and_then(Value::as_u64).unwrap_or(1) as u32;
    let project = scene.logic["project"].clone();
    let hands = SceneHands::new(&scene);
    let mut helper = Launch::launch(&mut hands.clone()).unwrap();
    logic_kumi::changes::set_settling(false);
    logic_kumi::disk::set_hook(Some(logic_kumi::disk::replaying(&[json!({"kind": "preference", "path": "MiddleC", "answer": "1"})])));
    let cases = scenes::run_set(
        helper.as_mut(),
        pid,
        Strings::english(),
        json!({"project": project, "busy": false, "permission": null}),
        (project["id"].as_str(), project["path"].as_str(), project["name"].as_str().unwrap_or_default()),
        &[("read", json!({"kind": "project"}))],
    );
    logic_kumi::disk::set_hook(None);
    assert_eq!(result(&cases[0])["items"][0]["middleC"], 4);
}
