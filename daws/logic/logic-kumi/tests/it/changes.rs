//! The bridge's changes over recorded scenes of the real Logic Pro, without Logic: each kind's scene (a preview, its
//! apply, a readback, its restore, as `examples/record_changes.rs` recorded them) replayed through the bridge's own
//! change side, with every answer compared to what the real Logic gave; a readback that doesn't show the value
//! answered `uncertain`; a dialog answered `busy` through the whole bridge; a restore whose ref is from an earlier
//! epoch carried to the track's place now; and the undo watch naming the producer's own Cmd-Z back, over a plan of
//! four changes undone by hand after the recording. The M2 scenes and `reads.rs` stand as they are.

use std::path::PathBuf;

use super::common::{dialog, hands_showing, tracks_window, Lab, SONG};
use logic_kumi::ax::strings::Strings;
use logic_kumi::ax::Ax;
use logic_kumi::changes::{self, Changes};
use logic_kumi::fixtures::{Scene, SceneHands};
use logic_kumi::helper::{Helper, Launch};
use logic_kumi::project::Project;
use logic_kumi::serve;
use serde_json::{json, Value};

fn fixtures() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

/// The stronger opaque identity expected from this original recording's one Vocal audio effect. These are
/// recorded observable facts, not the production extractor: the source blob and its old wire answers stay intact.
fn recorded_fx_identity() -> Value {
    json!({
        "owner": {"name": "Vocal", "kind": "audio", "ax": {"role": "AXLayoutItem"}},
        "slot": 1,
        "chain": [{"slot": 1, "name": "Channel EQ", "kind": "effect", "ax": {"role": "AXGroup"}, "bypass": {"role": "AXCheckBox"}}],
    })
}

/// Full material newly retained by the port, derived from the original Marker List's four visible columns.
/// The old public read row says text:""; identity explicitly records that no Text column was exposed.
fn recorded_marker_identity(name: &str, bar: u32, length: impl Into<Value>) -> Value {
    let length: Value = length.into();
    json!({"position": {"bar": bar, "beat": 1, "division": 1, "tick": 1}, "name": name, "length": length,
        "text": null, "columns": ["Lock", "Position", "Name", "Length"], "ax": {"role": "AXRow", "subrole": "AXTableRow"},
        "other": {"Lock": {"role": "AXCell", "children": []}}})
}

fn recorded_marker_before(kind: &str) -> Value {
    let first = recorded_marker_identity("Marker 1", 9, 32.0);
    if kind == "marker-delete" {
        json!([first, recorded_marker_identity("Marker 2", 17, "infinite")])
    } else {
        json!([first])
    }
}

fn recorded_marker_material(kind: &str) -> Value {
    let prior = recorded_marker_before(kind);
    let mut material = if kind == "marker-add" {
        let marker = recorded_marker_identity("Marker 2", 17, "infinite");
        json!({"marker": marker, "prior": prior, "applied": [recorded_marker_identity("Marker 1", 9, 32.0), marker]})
    } else {
        let marker = recorded_marker_identity("Marker 1", 10, 32.0);
        json!({"marker": marker, "priorMarker": recorded_marker_identity("Marker 1", 9, 32.0), "prior": prior, "applied": [marker]})
    };
    material["undo"] = manual_hint(marker_unknown_reason());
    material
}

fn marker_unknown_reason() -> &'static str {
    "Kumi can't verify the marker's complete original contents because Logic Pro didn't expose every required field. Use Logic Pro's own undo manually; showing a field now can't recover its earlier value."
}

fn track_add_reason() -> &'static str {
    "Kumi can't safely remove a track it added because Logic Pro doesn't expose all of its contents. Use Logic Pro's own undo manually."
}

fn manual_hint(reason: &str) -> Value {
    json!({"mode": "manual", "reason": reason, "guidance": "Use Logic Pro's own undo manually."})
}

/// Every change scene, by its name.
const SCENES: [&str; 19] = [
    "change-track-volume",
    "change-track-pan",
    "change-track-mute",
    "change-track-solo",
    "change-track-arm",
    "change-track-rename",
    "change-track-add",
    "change-track-delete",
    "change-marker-add",
    "change-marker-move",
    "change-marker-delete",
    "change-tempo-set",
    "change-cursor",
    "change-transport",
    "change-notes-add",
    "change-notes-change",
    "change-notes-delete",
    "change-fx-bypass",
    "change-plan",
];

/// One step of a scene replayed: what was asked, what the real Logic answered, what the replay answered.
struct Step {
    step: Value,
    recorded: Value,
    replayed: Value,
}

impl Step {
    fn method(&self) -> &str {
        self.step["method"].as_str().unwrap_or("")
    }
    fn result(&self) -> &Value {
        &self.replayed["result"]
    }
}

/// A scene replayed: its steps, the requests it couldn't answer, and the undo watch's events.
struct RestoreBoundary {
    index: usize,
    /// Kept explicitly: these recorded requests depend on native Undo actually having run, so are not replay coverage.
    unconsumed: Vec<Value>,
    helper_calls: usize,
    refusal: String,
}

struct Replayed {
    boundary: Option<RestoreBoundary>,
    calls: Vec<(String, Value)>,
    steps: Vec<Step>,
    unrecorded: Vec<(String, Value)>,
    events: Vec<Value>,
}

fn load(name: &str) -> Scene {
    Scene::load(&fixtures().join(name))
        .unwrap_or_else(|error| panic!("no scene {name}: {error}; record it with the record_changes example"))
}

/// Replays a scene's steps through the change side over its recorded helper replies, as the recorder ran them.
fn replay_scene(scene: &Scene) -> Replayed {
    changes::set_settling(false);
    let pid = scene.logic.get("pid").and_then(Value::as_u64).unwrap_or(1) as u32;
    let project = scene.logic.get("project").cloned().unwrap_or(Value::Null);
    let hands = SceneHands::new(scene);
    let mut helper: Box<dyn Helper> = Launch::launch(&mut hands.clone()).unwrap();
    let mut state = Project::new(Strings::english());
    state.set_identity(
        project.get("id").and_then(Value::as_str),
        project.get("path").and_then(Value::as_str),
        project.get("name").and_then(Value::as_str).unwrap_or_default(),
    );
    // The attach the recorder made before the steps, so the helper's reply queues line up.
    let _ = helper.call("apps", json!({"bundles": logic_kumi::logic::BUNDLES}));
    let _ = helper.call("trusted", json!({"prompt": false}));
    let _ = helper.call("windows", json!({"pid": pid}));
    {
        let mut ax = Ax::new(helper.as_mut(), pid);
        state.look(&mut ax, None).expect("the first look");
    }
    let mut changes = Changes::default();
    let (mut steps, mut events) = (Vec::new(), Vec::new());
    let records = scene.logic.get("steps").and_then(Value::as_array).unwrap();
    let mut boundary = None;
    for (index, record) in records.iter().enumerate() {
        let calls_before = hands.calls().len();
        let mut step = record["step"].clone();
        if scene.name == "change-fx-bypass" && step["params"]["kind"] == "fx-bypass" {
            let method = step["method"].as_str().unwrap_or_default().to_string();
            match method.as_str() {
                "change_apply" => step["params"]["fence"]["identity"] = recorded_fx_identity(),
                "change_restore" => step["params"]["target"]["identity"] = recorded_fx_identity(),
                _ => {}
            }
        }
        let kind = step["params"]["kind"].as_str().unwrap_or_default().to_string();
        let method = step["method"].as_str().unwrap_or_default().to_string();
        if matches!(kind.as_str(), "marker-add" | "marker-move" | "marker-delete") {
            if method == "change_apply" {
                step["params"]["fence"]["identity"] = recorded_marker_before(&kind);
            } else if method == "change_restore" && kind != "marker-delete" {
                step["params"]["material"] = recorded_marker_material(&kind);
                if kind == "marker-move" {
                    step["params"]["prior"]["identity"] = recorded_marker_before(&kind);
                }
            }
        }
        let answer = if let Some(method) = step.get("method").and_then(Value::as_str) {
            let params = step["params"].clone();
            if serve::CHANGE_METHODS.contains(&method) {
                serve::answer_change(&mut state, &mut changes, helper.as_mut(), pid, method, &params)
            } else {
                serve::answer(&mut state, helper.as_mut(), pid, method, &params)
            }
        } else if step.get("poll").is_some() {
            let mut ax = Ax::new(helper.as_mut(), pid);
            let (look, headers, _) = state.look(&mut ax, None).expect("a look for the poll");
            let mut got = Vec::new();
            let carry = |from: u32, place: usize| state.carry_track(from, place);
            changes.watch.poll(&mut ax, &state.strings, &look, &headers, &mut |data| got.push(data), &carry);
            events.extend(got.iter().cloned());
            Ok(json!({"epoch": look.epoch.to_string(), "events": got}))
        } else {
            // The producer's own Cmd-Z, or a wait: nothing of the helper's.
            continue;
        };
        let replayed = match answer {
            Ok(result) => json!({"ok": true, "result": result}),
            Err((code, message)) => json!({"ok": false, "error": {"code": code, "message": message}}),
        };
        let native_undo = record["answer"]["result"]["undo"]["how"] == "undo";
        let refusal = if native_undo {
            Some(changes::NATIVE_UNDO_REFUSAL)
        } else if method == "change_restore" && kind == "track-add" {
            Some(track_add_reason())
        } else if method == "change_restore" && matches!(kind.as_str(), "marker-add" | "marker-move") {
            Some(marker_unknown_reason())
        } else {
            None
        };
        steps.push(Step { step, recorded: record["answer"].clone(), replayed });
        if let Some(refusal) = refusal {
            boundary = Some(RestoreBoundary {
                index,
                unconsumed: records[index + 1..].to_vec(),
                helper_calls: hands.calls().len() - calls_before,
                refusal: refusal.into(),
            });
            break;
        }
    }
    Replayed { boundary, calls: hands.calls(), steps, unrecorded: hands.unrecorded(), events }
}

fn replay(name: &str) -> Replayed {
    let replayed = replay_scene(&load(name));
    assert!(
        replayed.unrecorded.is_empty(),
        "{name}: the scene didn't record what the changes ask now; record it again: {:?}",
        replayed.unrecorded
    );
    let expected_boundary = match name {
        "change-track-delete" => Some((5, 4)),
        "change-marker-delete" => Some((5, 3)),
        "change-carry" => Some((6, 4)),
        "change-track-add" => Some((3, 0)),
        "change-marker-add" => Some((3, 1)),
        "change-marker-move" => Some((2, 0)),
        _ => None,
    };
    assert_eq!(
        replayed.boundary.as_ref().map(|b| (b.index, b.unconsumed.len())),
        expected_boundary,
        "{name}: the unsupported restore boundary and its unconsumed recorded suffix are explicit"
    );
    for (index, step) in replayed.steps.iter().enumerate() {
        if replayed.boundary.as_ref().is_some_and(|b| b.index == index) {
            assert_eq!(step.recorded["ok"], true, "the historical recording really claimed a successful restore");
            assert_eq!(
                step.replayed,
                json!({"ok": false, "error": {"code": "failed", "message": replayed.boundary.as_ref().unwrap().refusal}})
            );
            assert_eq!(replayed.boundary.as_ref().unwrap().helper_calls, 0, "a refused restore consumes no helper request");
            continue;
        }
        let mut expected = step.recorded.clone();
        if name == "change-fx-bypass" && step.method() == "change_preview" {
            // Only the intentionally extended opaque target/fence differ from the original wire recording.
            expected["result"]["target"]["identity"] = recorded_fx_identity();
            expected["result"]["fence"]["identity"] = recorded_fx_identity();
        }
        let kind = step.step["params"]["kind"].as_str().unwrap_or_default();
        if matches!(kind, "marker-add" | "marker-move" | "marker-delete") && step.method() == "change_preview" {
            expected["result"]["fence"]["identity"] = recorded_marker_before(kind);
            if kind != "marker-add" {
                expected["result"]["prior"]["identity"] = recorded_marker_before(kind);
            }
        }
        if matches!(kind, "marker-add" | "marker-move") && step.method() == "change_apply" {
            expected["result"]["material"] = recorded_marker_material(kind);
            expected["result"]["display"]["note"] = marker_unknown_reason().into();
        }
        if kind == "track-add" && step.method() == "change_apply" {
            expected["result"]["material"] = json!({"undo": manual_hint(track_add_reason())});
            expected["result"]["display"]["note"] = track_add_reason().into();
        }
        if name == "change-plan" && step.step["poll"] == true {
            // The historical scene attributes a track deletion by names/title alone. That evidence cannot prove
            // the removed material was Kumi's track, so the port intentionally does not attribute that event.
            if let Some(events) = expected.get_mut("result").and_then(|result| result.get_mut("events")).and_then(Value::as_array_mut) {
                events.retain(|event| event["descs"] != json!(["Kumi: Added audio track (k0003)"]));
            }
        }
        assert_eq!(step.replayed, expected, "{name}: {} answered otherwise than the real Logic did", step.step);
    }
    replayed
}

/// The steps of a method, in order.
fn of<'a>(replayed: &'a Replayed, method: &str) -> Vec<&'a Step> {
    replayed.steps.iter().filter(|step| step.method() == method).collect()
}

#[test]
fn original_change_scenes_replay_with_explicit_unsupported_restore_boundaries() {
    let sources: Value = serde_json::from_str(include_str!("../fixtures/change-sources.json")).unwrap();
    assert_eq!(sources["commit"], "8d609d6a051e4a6f040431bf64c4b9712214094e");
    let files = sources["files"].as_array().unwrap();
    assert_eq!(files.len(), 20);
    for file in files {
        let bytes = std::fs::read(fixtures().join(file["path"].as_str().unwrap())).unwrap();
        assert_eq!(daw_bridge::auth::sha256_hex(&bytes), file["sha256"], "{}", file["source"]);
    }
    for name in SCENES {
        let replayed = replay(name);
        assert!(!replayed.steps.is_empty(), "{name}");
    }
}

#[test]
fn the_original_cycle_scene_is_preserved_and_its_incomplete_setup_refuses_without_a_write() {
    let source: Value = serde_json::from_str(include_str!("../fixtures/cycle-source.json")).unwrap();
    assert_eq!(source["commit"], "6f8f6e9bbc3666159ce68be1ce7466aa30e82670");
    let files = source["files"].as_array().unwrap();
    assert_eq!(files.len(), 1);
    let bytes = std::fs::read(fixtures().join(files[0]["path"].as_str().unwrap())).unwrap();
    assert_eq!(daw_bridge::auth::sha256_hex(&bytes), files[0]["sha256"]);
    let scene = load("change-loop-range");
    let records = scene.logic["steps"].as_array().unwrap();
    assert_eq!(records.len(), 7);
    assert_eq!(records[0]["step"]["method"], "change_preview");
    assert_eq!(records[0]["answer"]["ok"], true, "the historical answer remains evidence, not the new expected answer");
    // These two observed replies supply only a read-only prefix. The stronger request asks for writable facts
    // absent from the recording; this test doesn't invent them or pretend to replay the original mutation suffix.
    let windows = scene.calls.iter().find(|call| call.op == "windows").unwrap().reply.clone();
    let bar =
        scene.calls.iter().find(|call| call.op == "tree" && call.params["path"] == "5" && call.params["depth"] == 3).unwrap().reply.clone();
    assert_eq!(bar["node"]["c"][0]["c"][0]["value"], "Beats & Project");
    let hands = logic_kumi::helper::MockHands::new();
    hands.reply("windows", Ok(windows));
    hands.reply("tree", Ok(bar));
    let mut helper = hands.clone().launch().unwrap();
    let saved = &scene.logic["project"];
    let mut state = Project::new(Strings::english());
    state.set_identity(saved["id"].as_str(), saved["path"].as_str(), saved["name"].as_str().unwrap());
    state.layout.control_bar = Some("5".into());
    state.layout.headers = Some("7.1.0.0.0".into());
    let refused =
        serve::answer_change(&mut state, &mut Changes::default(), helper.as_mut(), 1, "change_preview", &records[0]["step"]["params"])
            .unwrap_err();
    assert_eq!(refused.0, "failed");
    assert!(refused.1.contains("LCD must already show Custom"), "{}", refused.1);
    assert_eq!(hands.calls().iter().map(|(op, _)| op.as_str()).collect::<Vec<_>>(), ["windows", "tree"]);
    assert_eq!(records[1..].len(), 6, "all six dependent recorded requests remain explicitly unconsumed");
    assert_eq!(records[1]["step"]["method"], "change_apply");
    assert_eq!(records[6]["step"]["method"], "change_restore");
}

#[test]
fn original_region_value_scenes_keep_their_bytes_and_refuse_at_an_explicit_incomplete_setup_prefix() {
    let source: Value = serde_json::from_str(include_str!("../fixtures/region-values-sources.json")).unwrap();
    assert_eq!(source["commit"], "6f8f6e9bbc3666159ce68be1ce7466aa30e82670");
    let files = source["files"].as_array().unwrap();
    assert_eq!(files.len(), 2);
    for file in files {
        let bytes = std::fs::read(fixtures().join(file["path"].as_str().unwrap())).unwrap();
        assert_eq!(daw_bridge::auth::sha256_hex(&bytes), file["sha256"]);
        let scene = load(file["path"].as_str().unwrap().split('/').next().unwrap());
        let records = scene.logic["steps"].as_array().unwrap();
        assert_eq!(records.len(), 5);
        assert_eq!(records[0]["step"]["method"], "change_preview");
        assert_eq!(records[0]["answer"]["ok"], true, "original answers remain unchanged historical evidence");
        // Raw recorded replies are sufficient for this read-only refusal prefix. We don't fabricate missing
        // Mixer/Inspector/action facts, replay mutation requests, or count the unconsumed suffix as coverage.
        let hands = logic_kumi::helper::MockHands::new();
        hands.reply("windows", Ok(scene.calls.iter().find(|c| c.op == "windows").unwrap().reply.clone()));
        hands.reply(
            "tree",
            Ok(scene.calls.iter().find(|c| c.op == "tree" && c.params["path"] == "5" && c.params["depth"] == 3).unwrap().reply.clone()),
        );
        let mut helper = hands.clone().launch().unwrap();
        let saved = &scene.logic["project"];
        let mut state = Project::new(Strings::english());
        state.set_identity(saved["id"].as_str(), saved["path"].as_str(), saved["name"].as_str().unwrap());
        state.layout.control_bar = Some("5".into());
        state.layout.headers = Some("7.1.0.0.0".into());
        let error =
            serve::answer_change(&mut state, &mut Changes::default(), helper.as_mut(), 1, "change_preview", &records[0]["step"]["params"])
                .unwrap_err();
        assert_eq!(error.0, "failed");
        assert!(error.1.contains("Inspector and Mixer must already be visible"), "{}", error.1);
        assert_eq!(hands.calls().iter().map(|(op, _)| op.as_str()).collect::<Vec<_>>(), ["windows", "tree"]);
        assert_eq!(records[1..].len(), 4, "four dependent recorded requests remain explicitly unconsumed");
        assert_eq!(records[1]["step"]["method"], "change_apply");
    }
}

#[test]
fn a_tracks_volume_pan_mute_solo_and_arm_go_through_its_header_and_come_back_by_value() {
    for (name, field, wanted, prior, step) in [
        ("change-track-volume", "volumeDb", json!(-6.0), json!(0.0), "Vocal volume −6 dB"),
        ("change-track-pan", "pan", json!(-0.5), json!(0.0), "Vocal pan 50% left"),
        ("change-track-mute", "mute", json!(true), json!(false), "Vocal muted"),
        ("change-track-solo", "solo", json!(true), json!(false), "Keys soloed"),
        ("change-track-arm", "armed", json!(true), json!(false), "Vocal armed"),
    ] {
        let replayed = replay(name);
        let preview = of(&replayed, "change_preview")[0].result().clone();
        assert_eq!(
            (&preview["prior"][field], &preview["fence"][field], preview["display"]["title"].as_str()),
            (&prior, &prior, Some(step)),
            "{name}"
        );
        assert_eq!(preview["target"]["kind"], "track");
        let applied = of(&replayed, "change_apply")[0].result().clone();
        assert_eq!(applied["applied"][field], wanted, "{name}");
        // Logic keeps no undo step for a header's control: Kumi's undo is the way back.
        assert_eq!(applied["undo"], json!({"desc": format!("Kumi: {step} (k0001)"), "entry": false}), "{name}");
        let restored = of(&replayed, "change_restore")[0].result().clone();
        assert_eq!(restored["restored"][field], prior, "{name}");
        assert!(restored["display"]["title"].as_str().unwrap().ends_with("(put back)"), "{name}: {}", restored["display"]["title"]);
        assert_eq!(restored["undo"]["entry"], false);
    }
    // The pan carries Logic's own steps beside the share: −32 of 64 is half left.
    let pan = replay("change-track-pan");
    assert_eq!(of(&pan, "change_apply")[0].result()["applied"], json!({"pan": -0.5, "steps": -32}));
}

#[test]
fn a_rename_goes_through_the_inspector_moves_the_epoch_and_is_a_step_of_logics_undo() {
    let replayed = replay("change-track-rename");
    let applied = of(&replayed, "change_apply")[0].result().clone();
    assert_eq!(applied["applied"], json!({"name": "Vox"}));
    assert_eq!(applied["display"]["title"], "Renamed track “Vocal” → “Vox”");
    assert_eq!(applied["undo"], json!({"desc": "Kumi: Renamed track “Vocal” → “Vox” (k0001)", "entry": true, "title": "Undo Renaming"}));
    // A renamed header moves the epoch, as the M2 observers count it: the answer carries the new one.
    assert_eq!(applied["epoch"], "2");
    let read = of(&replayed, "read")[0].result().clone();
    assert_eq!((read["items"][0]["name"].as_str(), read["items"][0]["ref"].as_str()), (Some("Vox"), Some("2:track:1")));
    let restored = of(&replayed, "change_restore")[0].result().clone();
    assert_eq!((restored["restored"]["name"].as_str(), restored["epoch"].as_str()), (Some("Vocal"), Some("3")));
}

#[test]
fn a_track_added_below_a_track_answers_the_new_epoch_and_its_restore_requires_manual_undo() {
    let replayed = replay("change-track-add");
    let preview = of(&replayed, "change_preview")[0].result().clone();
    assert_eq!(preview["fence"], json!({"tracks": 2, "names": ["Vocal", "Keys"]}));
    let applied = of(&replayed, "change_apply")[0].result().clone();
    assert_eq!(applied["epoch"], "2");
    assert_eq!(applied["created"], json!({"ref": "2:track:2", "kind": "track"}));
    assert_eq!(applied["applied"], json!({"index": 2, "name": "Bass", "kind": "audio"}));
    assert_eq!(applied["display"]["title"], "Added audio track “Bass”");
    assert_eq!((applied["undo"]["entry"].as_bool(), applied["undo"]["title"].as_str()), (Some(true), Some("Undo Renaming")));
    let read = of(&replayed, "read")[0].result().clone();
    assert_eq!(
        read["items"].as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap()).collect::<Vec<_>>(),
        ["Vocal", "Bass", "Keys"]
    );
    assert_eq!(of(&replayed, "change_restore")[0].replayed["error"]["message"], track_add_reason());
    assert!(replayed.boundary.as_ref().unwrap().unconsumed.is_empty());
}

#[test]
fn a_deleted_tracks_restore_refuses_the_unowned_native_undo_and_preserves_its_recorded_suffix() {
    let replayed = replay("change-track-delete");
    let applies = of(&replayed, "change_apply");
    // The scene adds an instrument track, then deletes it: the delete is the second apply.
    let deleted = applies[1].result().clone();
    assert_eq!(deleted["applied"], json!({"deleted": true}));
    let material = &deleted["material"];
    for key in ["name", "kind", "index", "volumeDb", "pan", "mute", "solo", "arm", "step"] {
        assert!(material.get(key).is_some(), "material lacks {key}: {material}");
    }
    assert_eq!((material["name"].as_str(), material["index"].as_u64()), (Some("Inst 2"), Some(3)));
    assert_eq!(material["step"], deleted["undo"]["desc"]);
    assert_eq!(deleted["undo"]["title"], "Undo Delete Tracks");
    let refused = &of(&replayed, "change_restore")[0].replayed;
    assert_eq!(refused["error"]["code"], "failed");
    assert_eq!(refused["error"]["message"], changes::NATIVE_UNDO_REFUSAL);
    assert_eq!(of(&replayed, "read")[0].result()["items"].as_array().unwrap().len(), 2);
    assert_eq!(replayed.boundary.as_ref().unwrap().unconsumed.len(), 4);
}

#[test]
fn markers_are_made_at_the_playhead_moved_by_their_segments_and_deleted_by_their_row() {
    let added = replay("change-marker-add");
    let applied = of(&added, "change_apply")[0].result().clone();
    assert_eq!(applied["created"], json!({"ref": "1:marker:2", "kind": "marker"}));
    assert_eq!(applied["applied"]["position"], json!({"bar": 17, "beat": 1, "division": 1, "tick": 1}));
    assert_eq!(applied["applied"]["name"], "Marker 2");
    assert_eq!(applied["undo"]["title"], "Undo Create Marker");
    assert_eq!(of(&added, "read")[0].result()["items"].as_array().unwrap().len(), 2);
    assert_eq!(of(&added, "change_restore")[0].replayed["error"]["message"], marker_unknown_reason());
    assert_eq!(added.boundary.as_ref().unwrap().unconsumed.len(), 1);

    let moved = replay("change-marker-move");
    let applied = of(&moved, "change_apply")[0].result().clone();
    assert_eq!(applied["applied"]["position"]["bar"], 10);
    assert_eq!(applied["undo"]["title"], "Undo Value Change");
    assert_eq!(of(&moved, "change_restore")[0].replayed["error"]["message"], marker_unknown_reason());
    assert!(moved.boundary.as_ref().unwrap().unconsumed.is_empty());

    let deleted = replay("change-marker-delete");
    let applies = of(&deleted, "change_apply");
    assert_eq!(applies[1].result()["material"]["position"]["bar"], 17);
    assert_eq!(applies[1].result()["undo"]["title"], "Undo Delete");
    assert_eq!(of(&deleted, "change_restore")[0].replayed["error"]["message"], changes::NATIVE_UNDO_REFUSAL);
    assert_eq!(deleted.boundary.as_ref().unwrap().unconsumed.len(), 3);
}

#[test]
fn the_tempo_the_playhead_and_the_transport_go_through_the_control_bar() {
    let tempo = replay("change-tempo-set");
    let applied = of(&tempo, "change_apply")[0].result().clone();
    assert_eq!((applied["applied"]["bpm"].as_f64(), applied["undo"]["entry"].as_bool()), (Some(120.0), Some(false)));
    assert_eq!(of(&tempo, "change_restore")[0].result()["restored"]["bpm"], 160.0);

    let cursor = replay("change-cursor");
    let moved = of(&cursor, "cursor_set");
    assert_eq!(moved[0].result()["playhead"], json!({"bar": 5, "beat": 1}));
    assert_eq!((moved[0].result()["positionBeats"].as_f64(), moved[0].result()["position"].as_f64()), (Some(16.0), Some(6.0)));
    assert_eq!(moved[1].result()["positionBeats"], 0.0);
    assert_eq!(moved[2].result()["playhead"], json!({"bar": 3, "beat": 1}), "seconds at the tempo");

    let transport = replay("change-transport");
    let (stops, plays) = (of(&transport, "transport_stop"), of(&transport, "transport_play"));
    assert_eq!(stops[0].result(), &json!({"stopped": false, "wasRecording": false}));
    assert_eq!(plays[0].result(), &json!({"started": true, "playing": true}));
    assert_eq!(stops[1].result(), &json!({"stopped": true, "wasRecording": false}));
}

#[test]
fn notes_are_written_changed_and_deleted_in_the_event_list_and_put_back_by_their_values() {
    let added = replay("change-notes-add");
    let preview = of(&added, "change_preview")[0].result().clone();
    assert_eq!(preview["fence"]["count"], 4);
    assert!(preview["fence"]["hash"].is_string());
    let applied = of(&added, "change_apply")[0].result().clone();
    assert_eq!(
        applied["applied"]["added"],
        json!([{"position": {"bar": 1, "beat": 3, "division": 3, "tick": 1}, "pitch": 62, "velocity": 90, "lengthTicks": 480, "channel": 1}])
    );
    assert_eq!(applied["applied"]["notes"].as_u64(), Some(5));
    assert_eq!(applied["undo"]["entry"], true);
    assert!(applied["undo"]["title"].as_str().unwrap().starts_with("Undo "), "{}", applied["undo"]["title"]);
    assert_eq!(of(&added, "read")[0].result()["items"].as_array().unwrap().len(), 5);
    assert_eq!(of(&added, "change_restore")[0].result()["restored"], json!({"notes": 4}));

    let changed = replay("change-notes-change");
    let applied = of(&changed, "change_apply")[0].result().clone();
    assert_eq!(applied["applied"]["added"][0]["pitch"], 61);
    assert_eq!(applied["applied"]["removed"][0]["pitch"], 60);
    assert_eq!(applied["undo"]["title"], "Undo Value Change");
    assert_eq!(of(&changed, "read")[0].result()["items"][0]["pitch"], 60, "put back in place");

    let deleted = replay("change-notes-delete");
    let applied = of(&deleted, "change_apply")[0].result().clone();
    assert_eq!(applied["applied"]["removed"][0]["pitch"], 72);
    assert_eq!(applied["undo"]["title"], "Undo Delete");
    assert_eq!(of(&deleted, "read")[0].result()["items"][3]["pitch"], 72, "written again");
}

#[test]
fn a_plug_ins_bypass_is_pressed_on_its_strip_in_the_mixer() {
    let replayed = replay("change-fx-bypass");
    let applied = of(&replayed, "change_apply")[0].result().clone();
    assert_eq!(applied["applied"], json!({"bypassed": true}));
    assert_eq!(applied["display"]["title"], "Bypassed “Channel EQ” on “Vocal”");
    assert_eq!(applied["undo"]["entry"], false);
    assert_eq!(of(&replayed, "read")[0].result()["items"][0]["bypassed"], true);
    assert_eq!(of(&replayed, "change_restore")[0].result()["restored"], json!({"bypassed": false}));

    // The normal recording starts with Mixer closed. Derive a replacement after its preview without changing
    // the source blob; reading the replacement may only navigate that known pane, never press its bypass.
    let mut replaced = load("change-fx-bypass");
    let bar = replaced.calls.iter().find(|call| call.op == "tree" && call.reply["node"]["desc"] == "Control Bar").unwrap();
    let bar = logic_kumi::ax::Node::from_json(&bar.reply["node"]);
    let mixer_button = bar.find(&|node| node.role == "AXCheckBox" && node.desc == "Mixer").unwrap().path.clone();
    fn replace_plugin(node: &mut Value) {
        if node["role"] == "AXGroup" && node["desc"] == "Channel EQ" {
            node["desc"] = "Compressor".into();
        }
        if let Some(children) = node.get_mut("c").and_then(Value::as_array_mut) {
            for child in children {
                replace_plugin(child);
            }
        }
    }
    let mut mixer_snapshots = 0;
    for call in &mut replaced.calls {
        if call.op == "tree" && call.reply["node"]["role"] == "AXLayoutArea" && call.reply["node"]["desc"] == "Mixer" {
            mixer_snapshots += 1;
            if mixer_snapshots > 1 {
                replace_plugin(&mut call.reply["node"]);
            }
        }
    }
    assert!(mixer_snapshots >= 2);
    replaced.logic.get_mut("steps").and_then(Value::as_array_mut).unwrap().truncate(2);
    let refused = replay_scene(&replaced);
    assert_eq!(of(&refused, "change_apply")[0].replayed["error"]["code"], "changed");
    assert!(refused.unrecorded.is_empty(), "the closed-pane read uses only the recorded navigation");
    for (op, params) in &refused.calls {
        match op.as_str() {
            "action" => {
                assert_eq!(params["root"], "main");
                assert_eq!(params["path"], mixer_button);
                assert_eq!(params["name"], "AXPress");
            }
            "set" | "click" | "keys" | "menu" => panic!("the replaced plug-in received a write: {op} {params}"),
            _ => {}
        }
    }
}

#[test]
fn a_readback_that_doesnt_show_the_value_is_uncertain_and_names_no_step() {
    // The mute scene with its header reads frozen at the before-state: the click posts, but Logic never shows mute
    // on, so the apply answers `uncertain` and names no step. The header read is the tree of the headers' group,
    // which the reader asks for with 256 children (the window and the Tracks area ask for fewer).
    let mut scene = load("change-track-mute");
    let header_calls: Vec<usize> = scene
        .calls
        .iter()
        .enumerate()
        .filter(|(_, call)| call.op == "tree" && call.params.get("children") == Some(&json!(256)))
        .map(|(i, _)| i)
        .collect();
    assert!(header_calls.len() >= 2, "the mute scene reads the headers more than once");
    let before = scene.calls[header_calls[0]].reply.clone();
    for &i in &header_calls {
        scene.calls[i].reply = before.clone();
    }
    let replayed = replay_scene(&scene);
    let apply = &of(&replayed, "change_apply")[0].replayed;
    assert_eq!(apply["ok"], false, "{apply}");
    assert_eq!(apply["error"]["code"], "uncertain");
    assert!(apply["error"]["message"].as_str().unwrap().contains("after Kumi asked"));

    // A helper may have dispatched the click before losing the reply. Neither apply nor restore is retried.
    for restoring in [false, true] {
        let mut scene = load("change-track-mute");
        let clicks: Vec<_> = scene.calls.iter().enumerate().filter(|(_, call)| call.op == "click").map(|(index, _)| index).collect();
        assert_eq!(clicks.len(), 2, "one recorded apply and one restore click");
        scene.calls[clicks[usize::from(restoring)]].reply = json!({"ok": false, "error": "timeout"});
        scene.logic.get_mut("steps").and_then(Value::as_array_mut).unwrap().truncate(if restoring { 4 } else { 2 });
        let lost = replay_scene(&scene);
        let method = if restoring { "change_restore" } else { "change_apply" };
        let answer = &of(&lost, method)[0].replayed;
        assert_eq!(answer["error"]["code"], "uncertain", "{answer}");
        assert_eq!(lost.calls.iter().filter(|(op, _)| op == "click").count(), if restoring { 2 } else { 1 });
    }
}

#[test]
fn a_change_while_a_dialog_is_up_is_busy_and_writes_nothing() {
    let behind = json!({"title": "Song - Tracks", "role": "AXWindow", "subrole": "AXStandardWindow", "main": false, "focused": false, "document": format!("file://{SONG}")});
    let hands = hands_showing(json!([dialog("Import"), behind]));
    let lab = Lab::start(&hands);
    let mut client = lab.client();
    client.status_when("modal", |status| status["busy"] == "modal");
    let refused =
        client.call_with("change_preview", json!({"kind": "track-mute", "input": {"ref": "1:track:1", "mute": true}, "epoch": "1"}));
    assert_eq!(
        (refused["error"]["code"].as_str(), refused["error"]["message"].as_str()),
        (Some("busy"), Some("Logic Pro is showing a dialog; nothing changed"))
    );
    assert_eq!(hands.calls_of("set") + hands.calls_of("action") + hands.calls_of("click"), 0);
    // Without the Accessibility grant: failed, in words, never busy.
    hands.set_windows(json!([tracks_window("Song - Tracks", Some(&format!("file://{SONG}")))]), &["Song - Tracks"]);
    hands.set_trusted(false);
    client.status_when("the permission", |status| status["permission"] == "accessibility");
    let refused = client.call_with("change_apply", json!({"kind": "track-mute", "input": {}, "fence": {}, "desc": "x", "epoch": "1"}));
    assert_eq!(refused["error"]["code"], "failed");
    assert!(refused["error"]["message"].as_str().unwrap().contains("Accessibility"));
}

#[test]
fn an_earlier_epoch_recording_stops_at_the_unowned_native_undo_without_replaying_its_dependent_suffix() {
    let replayed = replay("change-carry");
    assert_eq!(of(&replayed, "change_apply").len(), 3, "fader, track addition and deletion replay unchanged");
    assert_eq!(of(&replayed, "change_restore")[0].replayed["error"]["message"], changes::NATIVE_UNDO_REFUSAL);
    let boundary = replayed.boundary.unwrap();
    assert_eq!(boundary.index, 6);
    assert_eq!(boundary.unconsumed.len(), 4);
    assert_eq!(boundary.unconsumed[0]["step"]["method"], "change_preview");
    assert_eq!(boundary.unconsumed[2]["step"]["params"]["kind"], "track-volume");
}

#[test]
fn a_recorded_plan_names_value_proven_manual_reversions_and_restores_values_without_title_only_attribution() {
    let replayed = replay("change-plan");
    let applies = of(&replayed, "change_apply");
    assert_eq!(applies.len(), 4, "volume, rename, add, mute");
    // Two Cmd-Z presses after the plan: Logic's own undo took back the track add and the rename, named back to Kumi
    // by the Edit menu's titles. The mute and the fader, which Logic keeps no step for, are then put back by value.
    let named: Vec<String> = replayed
        .events
        .iter()
        .filter(|event| event["how"] == "undo")
        .flat_map(|event| event["descs"].as_array().unwrap().iter().map(|d| d.as_str().unwrap().to_string()))
        .collect();
    assert!(!named.iter().any(|d| d.contains("Added audio track")), "a title/header-only structural undo is not attributed: {named:?}");
    assert!(named.iter().any(|d| d.contains("Renamed track")), "the rename wasn't named back: {named:?}");
    let restores = of(&replayed, "change_restore");
    assert_eq!(restores.len(), 2);
    assert!(restores.iter().all(|step| step.result()["undo"]["entry"] == false));
    // The project ends as it began: the fader at 0 dB, no mute, the name Vocal, two tracks.
    let read = of(&replayed, "read").last().unwrap().result().clone();
    let items = read["items"].as_array().unwrap();
    assert_eq!(items.len(), 2);
    assert_eq!(
        (items[0]["name"].as_str(), items[0]["volumeDb"].as_f64(), items[0]["mute"].as_bool()),
        (Some("Vocal"), Some(0.0), Some(false))
    );
}
