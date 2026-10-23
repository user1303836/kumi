//! The bridge's changes over recorded scenes of the real Logic Pro, without Logic: each kind's scene (a preview, its
//! apply, a readback, its restore, as `examples/record_changes.rs` recorded them) replayed through the bridge's own
//! change side, with every answer compared to what the real Logic gave; a readback that doesn't show the value
//! answered `uncertain`; a dialog answered `busy` through the whole bridge; a restore whose ref is from an earlier
//! epoch carried to the track's place now; and the undo watch naming the producer's own Cmd-Z back, over a plan of
//! four changes undone by hand after the recording. The M2 scenes and `reads.rs` stand as they are.

mod common;

use std::path::PathBuf;

use common::{dialog, hands_showing, tracks_window, Lab, SONG};
use logic_kumi::ax::strings::Strings;
use logic_kumi::ax::Ax;
use logic_kumi::changes::{self, Changes};
use logic_kumi::disk;
use logic_kumi::fixtures::{Scene, SceneHands};
use logic_kumi::helper::{Helper, Launch};
use logic_kumi::project::Project;
use logic_kumi::serve;
use serde_json::{json, Value};

fn fixtures() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

/// Every change scene, by its name.
const SCENES: [&str; 61] = [
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
    "change-key-command",
    "change-region-rename",
    "change-region-move",
    "change-region-resize",
    "change-region-split",
    "change-region-delete",
    "change-region-loop",
    "change-midi-import",
    "change-loop-range",
    "change-notes-quantize",
    "change-tempo-marker-set",
    "change-tempo-marker-delete",
    "change-signature-set",
    "change-track-stack",
    "change-track-stack-flatten",
    "change-project-save",
    "change-plan-m4",
    "change-fx-add",
    "change-fx-delete",
    "change-fx-params",
    "change-fx-output",
    "change-fx-preset",
    "change-send-add",
    "change-send-volume",
    "change-send-remove",
    "change-smart-control-set",
    "change-patch-load",
    "change-automation-add",
    "change-automation-edit",
    "change-bounce",
    "change-export-tracks",
    "change-undo-m4b",
    "read-smart-none",
    "change-fx-param-exact",
    "change-bounce-undo",
    "change-playhead-exact",
    "change-metronome-set",
    "read-recent",
    "read-paged",
    "read-bulk",
    "change-sample-load",
    "read-param-compressor",
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
struct Replayed {
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
    // What the recording saw on disk (a bounce's file, an export's folder), answered back in order.
    let looked: Vec<Value> = scene.logic.get("disk").and_then(Value::as_array).cloned().unwrap_or_default();
    disk::set_hook(Some(disk::replaying(&looked)));
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
    for record in scene.logic.get("steps").and_then(Value::as_array).into_iter().flatten() {
        let step = record["step"].clone();
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
        steps.push(Step { step, recorded: record["answer"].clone(), replayed });
    }
    disk::set_hook(None);
    Replayed { steps, unrecorded: hands.unrecorded(), events }
}

/// Whether `recorded` is all in `replayed`: every field it has answered the same, an array as long; `replayed` may add
/// fields.
fn within(recorded: &Value, replayed: &Value) -> bool {
    match (recorded, replayed) {
        (Value::Object(was), Value::Object(now)) => was.iter().all(|(key, value)| now.get(key).is_some_and(|got| within(value, got))),
        (Value::Array(was), Value::Array(now)) => was.len() == now.len() && was.iter().zip(now).all(|(a, b)| within(a, b)),
        (was, now) => was == now,
    }
}

/// `KUMI_BLESS=1`: a scene's recorded answers take what the replay adds to them, a field the bridge answers now that it
/// didn't when the scene was recorded (derived from the same recorded replies of Logic: the tempo row's `tempos` from
/// the Tempo List), and only where every recorded field is answered as it was; anything else stays a failure. As the
/// vectors are blessed, and run one test at a time (`-- --test-threads=1`).
fn bless_additions(name: &str) {
    let replayed = replay_scene(&load(name));
    let dir = fixtures().join(name);
    let mut raw: Value = serde_json::from_str(&logic_kumi::fixtures::scene_text(&dir).unwrap()).unwrap();
    let mut blessed = 0;
    let steps = raw["logic"]["steps"].as_array_mut().unwrap();
    let mut kept = steps.iter_mut().filter(|record| record["step"].get("method").is_some() || record["step"].get("poll").is_some());
    for step in &replayed.steps {
        let record = kept.next().unwrap();
        if step.replayed != step.recorded && within(&step.recorded, &step.replayed) {
            record["answer"] = step.replayed.clone();
            blessed += 1;
        }
    }
    if blessed > 0 {
        let mut text = serde_json::to_string(&raw).unwrap();
        text.push('\n');
        logic_kumi::fixtures::write_scene_text(&dir, &text).unwrap();
        eprintln!("{name}: {blessed} recorded answers took the fields the replay adds");
    }
}

fn replay(name: &str) -> Replayed {
    if std::env::var("KUMI_BLESS").is_ok_and(|v| v == "1") {
        bless_additions(name);
    }
    let replayed = replay_scene(&load(name));
    assert!(
        replayed.unrecorded.is_empty(),
        "{name}: the scene didn't record what the changes ask now; record it again: {:?}",
        replayed.unrecorded
    );
    for step in &replayed.steps {
        assert_eq!(step.replayed, step.recorded, "{name}: {} answered otherwise than the real Logic did", step.step);
    }
    replayed
}

/// The steps of a method, in order.
fn of<'a>(replayed: &'a Replayed, method: &str) -> Vec<&'a Step> {
    replayed.steps.iter().filter(|step| step.method() == method).collect()
}

#[test]
fn every_change_scene_replays_as_the_real_logic_answered() {
    for name in SCENES {
        let replayed = replay(name);
        assert!(!replayed.steps.is_empty(), "{name}");
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
fn a_track_added_below_a_track_answers_the_new_epoch_and_its_restore_deletes_it() {
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
    let restored = of(&replayed, "change_restore")[0].result().clone();
    assert_eq!((restored["restored"]["deleted"].as_str(), restored["epoch"].as_str()), (Some("2:track:2"), Some("3")));
    assert_eq!(restored["display"]["title"], "“Bass” taken away again");
}

#[test]
fn a_deleted_track_is_put_back_only_by_logics_own_undo_while_its_step_is_on_top() {
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
    let restored = of(&replayed, "change_restore")[0].result().clone();
    // The restore is Logic's own undo (Cmd-Z): a new step, so entry false, with how "undo"; the epoch moved again.
    assert_eq!((restored["undo"]["entry"].as_bool(), restored["undo"]["how"].as_str()), (Some(false), Some("undo")));
    assert!(restored["restored"]["ref"].as_str().unwrap().ends_with(":track:3"), "{}", restored["restored"]["ref"]);
    assert_eq!(restored["restored"]["was"], "2:track:3");
    assert_eq!(restored["display"]["note"], "Put back with Logic Pro's own undo.");
    let reads = of(&replayed, "read");
    assert_eq!(reads[0].result()["items"].as_array().unwrap().len(), 2);
    assert_eq!(reads[1].result()["items"].as_array().unwrap().len(), 3);
    assert_eq!(reads[2].result()["items"].as_array().unwrap().len(), 2);
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
    assert_eq!(of(&added, "change_restore")[0].result()["restored"], json!({"deleted": "1:marker:2"}));
    assert_eq!(of(&added, "read")[1].result()["items"].as_array().unwrap().len(), 1);

    let moved = replay("change-marker-move");
    let applied = of(&moved, "change_apply")[0].result().clone();
    assert_eq!(applied["applied"]["position"]["bar"], 10);
    assert_eq!(applied["undo"]["title"], "Undo Value Change");
    assert_eq!(of(&moved, "change_restore")[0].result()["restored"]["position"]["bar"], 9);

    let deleted = replay("change-marker-delete");
    let applies = of(&deleted, "change_apply");
    assert_eq!(applies[1].result()["material"]["position"]["bar"], 17);
    assert_eq!(applies[1].result()["undo"]["title"], "Undo Delete");
    let restored = of(&deleted, "change_restore")[0].result().clone();
    assert_eq!(restored["undo"]["how"], "undo", "{restored}");
    assert_eq!(of(&deleted, "read")[1].result()["items"].as_array().unwrap().len(), 2);
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
fn a_restore_from_an_earlier_epoch_is_carried_to_where_its_track_is_now() {
    let replayed = replay("change-carry");
    // A fader set in epoch 1, a track added (epoch 2), deleted (3), put back (4) and deleted again (5): the fader's
    // restore names epoch 1's place, which is still track 1.
    let restores = of(&replayed, "change_restore");
    let fader = restores.last().unwrap().result().clone();
    assert_eq!((fader["epoch"].as_str(), fader["restored"]["volumeDb"].as_f64()), (Some("5"), Some(0.0)));
    assert_eq!(of(&replayed, "read")[0].result()["items"][0]["volumeDb"], 0.0);
}

#[test]
fn the_producers_own_undo_of_a_plan_of_four_changes_is_named_back() {
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
    assert!(named.iter().any(|d| d.contains("Added audio track")), "the add wasn't named back: {named:?}");
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

#[test]
fn a_key_command_runs_as_an_action_with_a_row_only_when_logic_made_a_step_and_is_logics_own_to_undo() {
    let replayed = replay("change-key-command");
    let previews = of(&replayed, "change_preview");
    // Select All: Logic's history took no step, so no row (REAPER's run_action shape).
    assert_eq!(previews[0].result()["target"], json!({"kind": "action", "command": 766, "name": "Select All"}));
    assert_eq!(previews[0].result()["display"]["title"], "Ran key command “Select All” (⌘A)");
    let applies = of(&replayed, "change_apply");
    let select_all = applies[0].result().clone();
    assert_eq!((select_all["applied"]["changed"].as_bool(), select_all["display"]["unchanged"].as_bool()), (Some(false), Some(true)));
    assert_eq!(select_all["undo"], json!({"desc": "Kumi: Ran key command “Select All” (k0001)", "entry": false}));
    // Create Marker (Opt-apostrophe, the producer's own binding): a step, named by the Edit menu's validated title.
    let marker = applies[1].result().clone();
    assert_eq!(marker["applied"], json!({"command": 513, "name": "Create Marker", "key": "⌥'", "step": "Create Marker"}));
    assert_eq!(
        marker["undo"],
        json!({"desc": "Kumi: Ran key command “Create Marker” (k0002)", "entry": true, "title": "Undo Create Marker"})
    );
    assert_eq!(of(&replayed, "read")[0].result()["items"].as_array().unwrap().len(), 2, "a second marker");
    // A restore is Logic's own to do.
    let refused = &of(&replayed, "change_restore")[0].replayed;
    assert_eq!(refused["error"]["code"], "daw-only");
    // The producer's Cmd-Z after it is named back to Kumi by the watch.
    let named: Vec<&Value> = replayed.events.iter().filter(|event| event["how"] == "undo").collect();
    assert_eq!(named.len(), 1);
    assert_eq!(named[0]["descs"], json!(["Kumi: Ran key command “Create Marker” (k0002)"]));
    // What Kumi won't run, and what it doesn't know, is refused with the closest names.
    let refusals: Vec<String> = previews[2..].iter().map(|step| step.replayed["error"]["message"].as_str().unwrap().to_string()).collect();
    assert!(refusals[0].contains("no key command named “Save”"), "{}", refusals[0]);
    assert!(refusals[1].contains("1 to 4999"), "{}", refusals[1]);
    assert!(refusals[2].contains("“Split Regions/Events at Playhead Position”"), "{}", refusals[2]);
    assert!(previews[2..].iter().all(|step| step.replayed["error"]["code"] == "bad-params"));
}

#[test]
fn a_region_is_renamed_through_the_region_inspector_and_answers_its_ref_in_the_new_epoch() {
    let replayed = replay("change-region-rename");
    let preview = of(&replayed, "change_preview")[0].result().clone();
    assert_eq!((&preview["prior"], &preview["fence"]), (&json!({"name": "Keys"}), &json!({"name": "Keys"})));
    let applied = of(&replayed, "change_apply")[0].result().clone();
    assert_eq!(applied["applied"], json!({"name": "Keys (Kumi)"}));
    // A renamed region moves the epoch (M2's rule): the apply answers where the region is now, for the restore.
    assert_eq!((applied["epoch"].as_str(), &applied["target"]), (Some("2"), &json!({"ref": "2:region:1", "kind": "region"})));
    assert_eq!(applied["undo"]["title"], "Undo Renaming");
    assert_eq!(of(&replayed, "read")[0].result()["items"][0]["name"], "Keys (Kumi)");
    let restored = of(&replayed, "change_restore")[0].result().clone();
    assert_eq!(
        (restored["restored"]["name"].as_str(), restored["epoch"].as_str(), restored["undo"]["entry"].as_bool()),
        (Some("Keys"), Some("3"), Some(false))
    );
    assert_eq!(of(&replayed, "read")[1].result()["items"][0]["name"], "Keys");
}

#[test]
fn a_region_is_moved_through_the_event_lists_region_level_and_moved_back_by_value() {
    let replayed = replay("change-region-move");
    let preview = of(&replayed, "change_preview")[0].result().clone();
    assert_eq!(preview["fence"], json!({"startBeats": 0.0, "endBeats": 16.0, "name": "Keys", "track": 2, "looped": false}));
    assert_eq!(preview["display"]["title"], "Moved region “Keys” to bar 5");
    let applied = of(&replayed, "change_apply")[0].result().clone();
    assert_eq!((applied["applied"]["startBeats"].as_f64(), applied["applied"]["endBeats"].as_f64()), (Some(16.0), Some(32.0)));
    // A moved region moves the epoch: the apply says where it is now; Logic's step is a Value Change.
    assert_eq!((applied["epoch"].as_str(), applied["target"]["ref"].as_str()), (Some("2"), Some("2:region:1")));
    assert_eq!(applied["undo"]["entry"], true);
    let reads = of(&replayed, "read");
    assert_eq!(reads[0].result()["items"][0]["startBeats"], 16.0);
    let restored = of(&replayed, "change_restore")[0].result().clone();
    assert_eq!((restored["restored"]["startBeats"].as_f64(), restored["undo"]["entry"].as_bool()), (Some(0.0), Some(false)));
    assert_eq!(reads[1].result()["items"][0]["startBeats"], 0.0);
}

#[test]
fn a_regions_edges_are_set_to_the_playhead_with_logics_own_commands_and_put_back_by_value() {
    let replayed = replay("change-region-resize");
    let applies = of(&replayed, "change_apply");
    // The end first, then the start: each a step of Logic's, the notes staying where they are.
    assert_eq!(
        (applies[0].result()["applied"]["startBeats"].as_f64(), applies[0].result()["applied"]["endBeats"].as_f64()),
        (Some(0.0), Some(8.0))
    );
    assert_eq!(applies[0].result()["target"]["ref"], "2:region:1");
    assert_eq!(applies[0].result()["undo"]["entry"], true);
    assert_eq!(
        (applies[1].result()["applied"]["startBeats"].as_f64(), applies[1].result()["applied"]["endBeats"].as_f64()),
        (Some(4.0), Some(16.0))
    );
    let restores = of(&replayed, "change_restore");
    assert!(restores.iter().all(|step| step.result()["restored"]["startBeats"] == 0.0 && step.result()["restored"]["endBeats"] == 16.0));
    let reads = of(&replayed, "read");
    assert_eq!(
        (reads[0].result()["items"][0]["endBeats"].as_f64(), reads[2].result()["items"][0]["startBeats"].as_f64()),
        (Some(8.0), Some(4.0))
    );
    assert_eq!(reads[3].result()["items"][0]["startBeats"], 0.0);
}

#[test]
fn a_split_region_answers_its_right_part_as_created_and_comes_back_whole_through_logics_own_undo() {
    let replayed = replay("change-region-split");
    let applied = of(&replayed, "change_apply")[0].result().clone();
    assert_eq!(applied["applied"]["left"]["endBeats"], 8.0);
    assert_eq!(applied["applied"]["right"]["startBeats"], 8.0);
    assert_eq!(applied["created"], json!({"ref": "2:region:2", "kind": "region"}));
    assert_eq!(applied["target"]["ref"], "2:region:1");
    assert_eq!(of(&replayed, "read")[0].result()["items"].as_array().unwrap().len(), 2);
    let restored = of(&replayed, "change_restore")[0].result().clone();
    assert_eq!(restored["undo"]["how"], "undo");
    assert_eq!(restored["display"]["note"], "Put back with Logic Pro's own undo.");
    assert_eq!(of(&replayed, "read")[1].result()["items"].as_array().unwrap().len(), 1);
}

#[test]
fn a_deleted_region_comes_back_only_through_logics_own_undo_while_its_step_is_on_top() {
    let replayed = replay("change-region-delete");
    let applied = of(&replayed, "change_apply")[0].result().clone();
    assert_eq!(applied["applied"], json!({"deleted": true}));
    for key in ["startBeats", "endBeats", "name", "track", "kind", "step"] {
        assert!(applied["material"].get(key).is_some(), "material lacks {key}");
    }
    assert_eq!(applied["undo"]["title"], "Undo Delete");
    assert_eq!(of(&replayed, "read")[0].result()["total"], 0);
    let restored = of(&replayed, "change_restore")[0].result().clone();
    assert_eq!((restored["undo"]["how"].as_str(), restored["restored"]["was"].as_str()), (Some("undo"), Some("1:region:1")));
    assert_eq!(of(&replayed, "read")[1].result()["items"][0]["name"], "Keys");
}

#[test]
fn a_regions_loop_is_pressed_in_the_region_inspector_and_read_back_from_it() {
    let replayed = replay("change-region-loop");
    let applied = of(&replayed, "change_apply")[0].result().clone();
    assert_eq!(applied["applied"], json!({"loop": true}));
    // Logic keeps a step for it, under its own name.
    assert_eq!(applied["undo"]["title"], "Undo Loop Parameter Change");
    let restored = of(&replayed, "change_restore")[0].result().clone();
    assert_eq!((restored["restored"]["loop"].as_bool(), restored["undo"]["entry"].as_bool()), (Some(false), Some(false)));
}

#[test]
fn notes_are_imported_in_bulk_from_a_midi_file_logic_takes_through_its_panel_and_undone_by_logics_own_undo() {
    let replayed = replay("change-midi-import");
    let preview = of(&replayed, "change_preview")[0].result().clone();
    assert_eq!(preview["fence"], json!({"tracks": 2, "names": ["Vocal", "Keys"], "regions": 1}));
    let applied = of(&replayed, "change_apply")[0].result().clone();
    // Logic made a track for the file and put the region at the playhead: the apply says both, in the new epoch.
    assert_eq!(
        (applied["applied"]["notes"].as_u64(), applied["applied"]["startBeats"].as_f64(), applied["applied"]["endBeats"].as_f64()),
        (Some(4), Some(32.0), Some(36.0))
    );
    assert_eq!(applied["created"], json!({"ref": "2:region:2", "kind": "region", "track": "2:track:3", "trackAdded": true}));
    assert_eq!(applied["epoch"], "2");
    assert!(applied["applied"]["file"].as_str().unwrap().starts_with("/tmp/kumi-logic/kumi-"));
    assert_eq!(applied["undo"]["title"], "Undo Import");
    let reads = of(&replayed, "read");
    assert_eq!(reads[0].result()["items"][1]["name"], "MIDI Region");
    assert_eq!(reads[1].result()["items"].as_array().unwrap().len(), 3);
    let restored = of(&replayed, "change_restore")[0].result().clone();
    assert_eq!((restored["undo"]["how"].as_str(), restored["restored"]["pressed"].as_str()), (Some("undo"), Some("Undo Import")));
    assert_eq!(reads[2].result()["total"], 1);
}

#[test]
fn the_cycle_is_set_through_the_lcds_locators_and_cycle_button_and_put_back_with_its_locators() {
    let replayed = replay("change-loop-range");
    let previews = of(&replayed, "change_preview");
    assert_eq!(previews[0].result()["prior"], json!({"on": false, "startBeats": 0.0, "endBeats": 16.0}));
    let applies = of(&replayed, "change_apply");
    assert_eq!(applies[0].result()["applied"], json!({"on": true, "startBeats": 16.0, "endBeats": 32.0}));
    // Logic keeps no step for the cycle: nothing for its undo.
    assert_eq!(applies[0].result()["undo"]["entry"], false);
    let restores = of(&replayed, "change_restore");
    // An off cycle still has its locators: the restore puts them back too.
    assert_eq!(restores[0].result()["restored"], json!({"on": false, "startBeats": 0.0, "endBeats": 16.0}));
    // Off asked of an off cycle: unchanged, and nothing moves.
    assert_eq!(previews[1].result()["display"]["unchanged"], true);
    assert_eq!(applies[1].result()["applied"]["on"], false);
}

#[test]
fn notes_are_quantized_through_the_event_lists_quantize_parameter_and_read_as_recorded_once_it_is_off_again() {
    let replayed = replay("change-notes-quantize");
    let applies = of(&replayed, "change_apply");
    // A note nudged 96 ticks late first, so the quantize has something to move.
    assert_eq!(applies[0].result()["applied"]["added"][0]["position"]["tick"], 97);
    let quantized = applies[1].result();
    assert_eq!((quantized["applied"]["moved"].as_u64(), quantized["applied"]["grid"].as_str()), (Some(1), Some("1/16 Note")));
    assert_eq!(quantized["applied"]["notes"][0]["position"]["tick"], 1);
    assert_eq!(quantized["material"], json!({"was": "Off"}));
    // Logic made a step of it (its title wasn't validated at the moment of the read).
    assert_eq!(quantized["undo"]["entry"], true);
    let reads = of(&replayed, "read");
    assert_eq!(reads[0].result()["items"][0]["position"]["tick"], 1);
    let restored = of(&replayed, "change_restore")[0].result().clone();
    assert_eq!(restored["restored"], json!({"notes": 4, "quantize": "Off"}));
    // The recorded position is back exactly: the quantize was a parameter of the events.
    assert_eq!(reads[1].result()["items"][0]["position"]["tick"], 97);
    assert_eq!(reads[2].result()["items"][0]["position"]["tick"], 1);
}

#[test]
fn a_tempo_event_is_made_in_the_tempo_list_and_stepped_to_its_bpm_and_an_existing_one_is_put_back_by_value() {
    let replayed = replay("change-tempo-marker-set");
    let applies = of(&replayed, "change_apply");
    assert_eq!(applies[0].result()["applied"], json!({"exists": true, "bpm": 128.0, "made": true}));
    assert_eq!(applies[0].result()["undo"]["title"], "Undo Value Change");
    let restores = of(&replayed, "change_restore");
    // A row Kumi made is deleted again.
    assert_eq!(restores[0].result()["restored"], json!({"exists": false}));
    // The project's first tempo, by value and back.
    assert_eq!(applies[1].result()["applied"], json!({"exists": true, "bpm": 100.0, "made": false}));
    assert_eq!(restores[1].result()["restored"], json!({"exists": true, "bpm": 160.0}));
}

#[test]
fn a_deleted_tempo_event_comes_back_through_logics_own_undo_with_its_bpm_as_material() {
    let replayed = replay("change-tempo-marker-delete");
    let applies = of(&replayed, "change_apply");
    let deleted = applies[1].result();
    assert_eq!(deleted["applied"], json!({"deleted": true}));
    assert_eq!(deleted["material"], json!({"positionBeats": 16.0, "bpm": 128.0}));
    assert_eq!(deleted["undo"]["title"], "Undo Delete");
    let restored = of(&replayed, "change_restore")[0].result().clone();
    assert_eq!(restored["restored"], json!({"exists": true, "bpm": 128.0}));
    assert_eq!(restored["undo"]["how"], "undo");
}

#[test]
fn a_time_signature_is_made_in_the_signature_list_and_taken_away_again_through_the_lists_edit_menu() {
    let replayed = replay("change-signature-set");
    let applied = of(&replayed, "change_apply")[0].result().clone();
    assert_eq!(applied["applied"], json!({"exists": true, "timeSignature": [3, 4], "made": true}));
    assert_eq!(applied["undo"]["title"], "Undo Value Change");
    let reads = of(&replayed, "read");
    assert_eq!(reads[0].result()["items"][0]["signatures"][1]["time"], "3/4");
    let restored = of(&replayed, "change_restore")[0].result().clone();
    assert_eq!(restored["restored"], json!({"exists": false}));
    assert_eq!(reads[1].result()["items"][0]["signatures"].as_array().unwrap().len(), 1);
}

#[test]
fn tracks_are_stacked_by_logics_own_key_commands_and_a_stack_comes_apart_by_undo_or_by_flattening() {
    let replayed = replay("change-track-stack");
    let applies = of(&replayed, "change_apply");
    // A summing stack of two tracks: its header first, the subtracks after it, all in the new epoch.
    let summing = applies[0].result();
    assert_eq!(summing["applied"], json!({"index": 1, "name": "Sum 1", "kind": "summing", "tracks": ["2:track:2", "2:track:3"]}));
    assert_eq!(summing["created"], json!({"ref": "2:track:1", "kind": "track", "stack": "summing"}));
    assert_eq!(summing["undo"]["title"], "Undo Create Track Stack");
    let reads = of(&replayed, "read");
    let rows = reads[0].result()["items"].as_array().unwrap().clone();
    assert_eq!((rows[0]["stack"].as_bool(), rows[0]["kind"].as_str(), rows[1].get("stack")), (Some(true), Some("aux"), None));
    let restores = of(&replayed, "change_restore");
    assert_eq!(restores[0].result()["restored"], json!({"tracks": 2, "pressed": "Undo Create Track Stack"}));
    assert_eq!(reads[1].result()["total"], 2);
    // A folder stack of one track, named: Logic shows it closed, so it is opened to be read; the rename on top of
    // the creation means Logic's undo can't take it back, and it is flattened by value instead.
    let folder = applies[1].result();
    assert_eq!(folder["applied"], json!({"index": 2, "name": "Keys Folder", "kind": "folder", "tracks": ["4:track:3"]}));
    assert_eq!(folder["created"]["stack"], "folder");
    assert_eq!(restores[1].result()["restored"], json!({"tracks": 2}));
    assert_eq!(restores[1].result()["display"]["title"], "Stack “Keys Folder” flattened again");
}

#[test]
fn a_flattened_stack_comes_back_only_through_logics_own_undo() {
    let replayed = replay("change-track-stack-flatten");
    let applies = of(&replayed, "change_apply");
    let flattened = applies[1].result();
    assert_eq!(flattened["applied"], json!({"name": "Sum 1", "tracks": 2}));
    assert_eq!(flattened["material"], json!({"name": "Sum 1", "index": 1, "tracks": 3}));
    assert_eq!(flattened["undo"]["title"], "Undo Flatten Stack");
    let restored = of(&replayed, "change_restore")[0].result().clone();
    assert_eq!(restored["restored"], json!({"tracks": 3, "pressed": "Undo Flatten Stack", "ref": "4:track:1"}));
    assert_eq!(restored["undo"]["how"], "undo");
    let reads = of(&replayed, "read");
    assert_eq!(reads[2].result()["items"][0]["stack"], true);
    assert_eq!(reads[3].result()["total"], 2);
}

#[test]
fn the_project_is_saved_as_a_copy_through_the_save_panel_and_in_place_with_cmd_s_and_never_over_a_file_or_into_no_folder() {
    let replayed = replay("change-project-save");
    let saves = of(&replayed, "project_save");
    // Save As to a .logicx that isn't there: the window's document is that file from then on, and clean.
    assert_eq!(
        saves[0].result(),
        &json!({"epoch": "1", "saved": "/Users/me/programs/kumi-lanes/m4-files/Scratch copy.logicx", "dirty": false})
    );
    // A plain save after a change (Cmd-S), then one with nothing to save: no key posted, answered at once.
    assert_eq!(saves[1].result()["dirty"], false);
    assert_eq!(saves[2].result()["dirty"], false);
    // Refused: the project's own file as a path, REAPER's version, a folder that isn't there (the panel's own
    // "where" pop-up says where it stands; nothing is saved).
    let error = |step: &Step| step.replayed["error"].clone();
    assert!(error(saves[3])["message"].as_str().unwrap().contains("the project's own file"));
    assert!(error(saves[4])["message"].as_str().unwrap().contains("keeps no versions"));
    assert!(error(saves[5])["message"].as_str().unwrap().contains("isn't there"));
    assert_eq!(error(saves[5])["code"], "bad-params");
}

#[test]
fn a_plan_of_a_split_a_cycle_a_tempo_event_an_import_and_a_save_as_runs_as_one_step_each_of_logics_own_undo() {
    let replayed = replay("change-plan-m4");
    let applies = of(&replayed, "change_apply");
    assert_eq!(applies[0].result()["created"], json!({"ref": "2:region:2", "kind": "region"}));
    assert_eq!(applies[1].result()["applied"], json!({"on": true, "startBeats": 16.0, "endBeats": 32.0}));
    assert_eq!(applies[2].result()["applied"], json!({"exists": true, "bpm": 128.0, "made": true}));
    let import = applies[3].result();
    assert_eq!(
        (import["applied"]["notes"].as_u64(), import["applied"]["startBeats"].as_f64(), import["applied"]["endBeats"].as_f64()),
        (Some(64), Some(32.0), Some(48.0))
    );
    assert_eq!(import["created"]["trackAdded"], true);
    let saved = of(&replayed, "project_save")[0].result().clone();
    assert_eq!(saved["dirty"], false);
    assert_eq!(of(&replayed, "read")[0].result()["total"], 3);
}

#[test]
fn a_plug_in_is_added_through_its_slots_menu_search_and_taken_off_again_by_value() {
    let replayed = replay("change-fx-add");
    let preview = of(&replayed, "change_preview")[0].result().clone();
    assert_eq!(preview["fence"], json!({"plugins": ["Channel EQ"], "instrument": null}));
    let applied = of(&replayed, "change_apply")[0].result().clone();
    // The empty slot under Channel EQ: the plug-in menu's search took "gain", Down and Return its first result.
    assert_eq!(applied["applied"], json!({"index": 1, "name": "Gain", "slot": 2, "plugins": ["Channel EQ", "Gain"]}));
    assert_eq!(applied["created"], json!({"ref": "1:plugin:1.2", "kind": "plugin"}));
    assert_eq!(applied["undo"]["title"], "Undo Insert Plug-in in Channel Strip");
    let reads = of(&replayed, "read");
    assert_eq!(reads[0].result()["items"][1]["name"], "Gain");
    let restored = of(&replayed, "change_restore")[0].result().clone();
    assert_eq!(restored["restored"], json!({"deleted": "1:plugin:1.2", "plugins": ["Channel EQ"]}));
    assert_eq!(reads[1].result()["total"], 1);
}

#[test]
fn a_gain_on_the_stereo_out_is_added_set_in_its_controls_view_read_back_and_taken_off() {
    // What M5's listening needs: a Gain on the mix's output, its gain set and read, all of it taken back.
    let replayed = replay("change-fx-output");
    let applies = of(&replayed, "change_apply");
    assert_eq!(applies[0].result()["created"], json!({"ref": "1:plugin:0.1.1", "kind": "plugin"}));
    assert_eq!(applies[0].result()["undo"]["title"], "Undo Insert Plug-in in Channel Strip");
    let reads = of(&replayed, "read");
    assert_eq!(
        reads[0].result()["items"][0],
        json!({"ref": "1:plugin:0.1.1", "trackRef": "1:output:1", "slot": 1, "name": "Gain", "bypassed": false, "kind": "effect"})
    );
    let params = reads[1].result()["items"].as_array().unwrap().clone();
    let gain = params.iter().find(|p| p["name"] == "Gain").unwrap();
    assert_eq!(
        gain,
        &json!({"ref": "1:param:0.1.1.2", "pluginRef": "1:plugin:0.1.1", "index": 2, "name": "Gain", "value": 960.0, "text": "0.0 dB", "min": 0.0, "max": 1200.0, "normalized": 0.8})
    );
    // −6 found by reading the slider's text: Logic's Gain runs −96 to +24 dB a tenth a step.
    assert_eq!(
        applies[1].result()["applied"],
        json!({"plugin": "Gain", "params": [{"index": 2, "name": "Gain", "value": 900.0, "text": "-6.0 dB"}]})
    );
    assert_eq!(applies[1].result()["undo"]["entry"], false, "a parameter is no step of the project's undo");
    assert_eq!(reads[2].result()["items"][0]["text"], "-6.0 dB");
    let restores = of(&replayed, "change_restore");
    assert_eq!(restores[0].result()["restored"]["params"][0]["text"], "0.0 dB");
    assert_eq!(reads[3].result()["items"][0]["value"], 960.0);
    assert_eq!(restores[1].result()["restored"], json!({"deleted": "1:plugin:0.1.1", "plugins": []}));
    assert_eq!(reads[4].result()["total"], 0);
}

#[test]
fn a_deleted_plug_in_comes_back_through_logics_own_undo_and_an_instrument_isnt_taken_off() {
    let replayed = replay("change-fx-delete");
    let applied = of(&replayed, "change_apply")[0].result().clone();
    assert_eq!(applied["applied"], json!({"deleted": true, "plugins": ["Channel EQ", "Compressor"]}));
    assert_eq!(applied["material"]["name"], "ChromaVerb");
    assert_eq!(applied["undo"]["title"], "Undo Remove Plug-in in Channel Strip");
    let restored = of(&replayed, "change_restore")[0].result().clone();
    assert_eq!(restored["restored"], json!({"ref": "1:plugin:2.4", "pressed": "Undo Remove Plug-in in Channel Strip"}));
    assert_eq!(restored["undo"]["how"], "undo");
    assert_eq!(of(&replayed, "read")[1].result()["items"][3]["name"], "ChromaVerb");
    let refused = &of(&replayed, "change_preview")[1].replayed;
    assert_eq!(refused["error"]["code"], "bad-params");
    assert!(refused["error"]["message"].as_str().unwrap().contains("instrument"));
}

#[test]
fn a_plug_ins_parameters_are_set_together_in_its_controls_view_and_put_back_by_value() {
    let replayed = replay("change-fx-params");
    let applied = of(&replayed, "change_apply")[0].result().clone();
    assert_eq!(
        applied["applied"]["params"],
        json!([{"index": 7, "name": "Low Shelf Gain", "value": 270.0, "text": "+3.0 dB"}, {"index": 9, "name": "Peak 1 On/Off", "value": 0.0, "text": "off"}])
    );
    assert_eq!(applied["display"]["lines"], json!(["Low Shelf Gain → +3.0 dB", "Peak 1 On/Off → off"]));
    let reads = of(&replayed, "read");
    assert_eq!(reads[0].result()["items"][0]["text"], "+3.0 dB");
    let restored = of(&replayed, "change_restore")[0].result().clone();
    assert_eq!(restored["restored"]["params"][1]["text"], "on");
    assert_eq!(reads[1].result()["items"][0]["text"], "0.0 dB");
    let previews = of(&replayed, "change_preview");
    assert_eq!(previews[1].replayed["error"]["message"], "normalized must be a number from 0 to 1");
    assert!(previews[2].replayed["error"]["message"].as_str().unwrap().contains("no parameter named \"Drive\""));
}

#[test]
fn a_preset_is_stepped_with_the_plug_in_windows_setting_buttons_and_put_back_with_its_own_undo() {
    let replayed = replay("change-fx-preset");
    let preview = of(&replayed, "change_preview")[0].result().clone();
    assert_eq!(preview["fence"], json!({"plugin": "Channel EQ", "preset": "Default Preset"}));
    let applied = of(&replayed, "change_apply")[0].result().clone();
    assert_eq!(applied["applied"], json!({"plugin": "Channel EQ", "preset": "Brighten Hi Hat", "steps": 1}));
    assert_eq!(applied["material"], json!({"steps": 1}));
    assert_eq!(applied["undo"]["entry"], false, "a setting is a step of the plug-in window's own undo, not the project's");
    // "Default Preset" is on no setting button's way: the window's Undo (in its editor) brings it back.
    let restored = of(&replayed, "change_restore")[0].result().clone();
    assert_eq!(restored["restored"], json!({"preset": "Default Preset"}));
    assert_eq!(restored["display"]["note"], "Put back with the plug-in window's own undo.");
    assert_eq!(of(&replayed, "change_preview")[1].replayed["error"]["code"], "bad-params");
}

#[test]
fn a_send_is_made_by_walking_its_slots_menu_its_level_stepped_and_taken_back_with_its_aux_by_logics_own_undo() {
    let replayed = replay("change-send-add");
    let applied = of(&replayed, "change_apply")[0].result().clone();
    assert_eq!(applied["applied"], json!({"destination": "Bus 1", "slot": 1, "level": -6.0}));
    // Logic made an aux for the bus nothing used yet: the answer names it.
    assert_eq!(applied["created"], json!({"ref": "1:send:1.1", "kind": "send", "bus": 1, "aux": "Aux 1"}));
    assert_eq!(applied["undo"]["title"], "Undo Change Send in Channel Strip");
    let reads = of(&replayed, "read");
    assert_eq!(
        reads[0].result()["items"][0],
        json!({"ref": "1:send:1.1", "trackRef": "1:track:1", "slot": 1, "destination": "Bus 1", "level": -6.0})
    );
    let restored = of(&replayed, "change_restore")[0].result().clone();
    assert_eq!(restored["restored"], json!({"removed": "1:send:1.1", "pressed": "Undo Change Send in Channel Strip"}));
    assert_eq!(restored["undo"]["how"], "undo");
    assert_eq!(reads[1].result()["total"], 0);
    let refused = &of(&replayed, "change_preview")[1].replayed;
    assert_eq!(refused["error"]["code"], "bad-params");
    assert!(refused["error"]["message"].as_str().unwrap().contains("mode"));
}

#[test]
fn a_sends_level_is_stepped_on_its_knob_and_put_back_by_value() {
    let replayed = replay("change-send-volume");
    let applies = of(&replayed, "change_apply");
    // A new send starts at −∞, which the rows give as −96 dB.
    assert_eq!(applies[0].result()["applied"]["level"], -96.0);
    let preview = of(&replayed, "change_preview")[1].result().clone();
    assert_eq!(preview["fence"], json!({"send": "Bus 2", "level": -96.0}));
    assert_eq!(applies[1].result()["applied"], json!({"level": -12.0}));
    assert_eq!(applies[1].result()["undo"]["entry"], false, "a send's level is no step of Logic's undo");
    let reads = of(&replayed, "read");
    assert_eq!(reads[0].result()["items"][0]["level"], -12.0);
    let restores = of(&replayed, "change_restore");
    assert_eq!(restores[0].result()["restored"], json!({"level": -96.0}));
    assert_eq!(reads[1].result()["items"][0]["level"], -96.0);
    assert_eq!(restores[1].result()["undo"]["how"], "undo");
    assert_eq!(reads[2].result()["total"], 0);
}

#[test]
fn a_removed_send_comes_back_through_logics_own_undo_while_its_step_is_on_top() {
    let replayed = replay("change-send-remove");
    let applies = of(&replayed, "change_apply");
    let removed = applies[1].result().clone();
    assert_eq!(removed["applied"], json!({"removed": true, "sends": []}));
    assert_eq!(removed["material"]["destination"], "Bus 2");
    assert_eq!(removed["material"]["step"], removed["undo"]["desc"]);
    assert_eq!(removed["undo"]["title"], "Undo Change Send in Channel Strip");
    let reads = of(&replayed, "read");
    assert_eq!(reads[0].result()["total"], 0);
    let restores = of(&replayed, "change_restore");
    assert_eq!(restores[0].result()["restored"], json!({"destination": "Bus 2", "pressed": "Undo Change Send in Channel Strip"}));
    assert_eq!(reads[1].result()["items"][0]["destination"], "Bus 2");
    // Then the send Kumi added goes the same way, its aux with it.
    assert_eq!(restores[1].result()["restored"]["removed"], "1:send:1.1");
    assert_eq!(reads[2].result()["total"], 0);
}

#[test]
fn a_smart_control_is_stepped_in_the_smart_controls_pane_of_its_selected_track_and_put_back_by_value() {
    let replayed = replay("change-smart-control-set");
    let preview = of(&replayed, "change_preview")[0].result().clone();
    assert_eq!(preview["target"], json!({"ref": "1:smart:2.1", "kind": "smart", "track": "1:track:2"}));
    assert_eq!(preview["fence"], json!({"name": "Tone", "value": 0.0, "text": "0%"}));
    let applied = of(&replayed, "change_apply")[0].result().clone();
    assert_eq!(applied["applied"], json!({"name": "Tone", "value": 0.6, "text": "60%"}));
    assert_eq!(applied["undo"]["entry"], false);
    let reads = of(&replayed, "read");
    let names: Vec<&str> = reads[0].result()["items"].as_array().unwrap().iter().map(|c| c["name"].as_str().unwrap()).collect();
    assert_eq!(names, ["Tone", "Low Cut", "Comp", "Ratio"], "the pane's order, by rows then from the left");
    let restored = of(&replayed, "change_restore")[0].result().clone();
    assert_eq!(restored["restored"], json!({"name": "Tone", "value": 0.0, "text": "0%"}));
    assert_eq!(reads[1].result()["items"][0]["value"], 0.0);
    let refused = &of(&replayed, "change_preview")[1].replayed;
    assert!(refused["error"]["message"].as_str().unwrap().contains("“Tone”, “Low Cut”, “Comp”, “Ratio”"));
}

#[test]
fn a_patch_is_loaded_from_the_librarys_search_and_taken_back_only_by_logics_own_undo() {
    let replayed = replay("change-patch-load");
    let preview = of(&replayed, "change_preview")[0].result().clone();
    assert_eq!(preview["fence"], json!({"patch": "", "plugins": ["Channel EQ"]}));
    let applied = of(&replayed, "change_apply")[0].result().clone();
    assert_eq!(
        applied["applied"],
        json!({"patch": "Classic Vocal", "plugins": ["Channel EQ", "Compressor", "Channel EQ", "Tape Delay", "Spring Box"]})
    );
    assert_eq!(applied["undo"]["title"], "Undo Load Patch");
    let reads = of(&replayed, "read");
    assert_eq!(reads[0].result()["total"], 5);
    let restored = of(&replayed, "change_restore")[0].result().clone();
    assert_eq!(restored["restored"], json!({"patch": "", "plugins": ["Channel EQ"], "pressed": "Undo Load Patch"}));
    assert_eq!(restored["undo"]["how"], "undo");
    assert_eq!(reads[1].result()["total"], 1, "the strip read again after Logic's undo, not from what the bridge kept");
    let missing = &of(&replayed, "change_apply")[1].replayed;
    assert_eq!(missing["error"]["code"], "failed");
    assert!(missing["error"]["message"].as_str().unwrap().contains("finds no patch named “No Such Patch Here”"));
}

#[test]
fn automation_is_written_in_the_track_automation_event_list_read_back_as_rows_per_parameter_and_taken_out_by_value() {
    let replayed = replay("change-automation-add");
    let applied = of(&replayed, "change_apply")[0].result().clone();
    let made = json!([
        {"positionBeats": 0.0, "value": 90, "number": 7, "parameter": "Volume"},
        {"positionBeats": 4.0, "value": 60, "number": 7, "parameter": "Volume"}
    ]);
    assert_eq!(applied["applied"]["made"], made);
    assert_eq!(applied["applied"]["events"], made);
    assert_eq!(applied["undo"]["entry"], true);
    let reads = of(&replayed, "read");
    assert_eq!(
        reads[0].result()["items"][0],
        json!({"ref": "1:automation:1.1", "trackRef": "1:track:1", "index": 1, "parameter": "Volume", "number": 7, "events": [{"positionBeats": 0.0, "value": 90}, {"positionBeats": 4.0, "value": 60}]})
    );
    let restored = of(&replayed, "change_restore")[0].result().clone();
    assert_eq!(restored["restored"], json!({"events": []}));
    assert_eq!(reads[1].result()["total"], 0);
    let refused = &of(&replayed, "change_preview")[1].replayed;
    assert_eq!(refused["error"]["message"], "Logic Pro's playhead display places an automation event on a beat: give whole beats");
}

#[test]
fn an_automation_event_is_changed_and_deleted_by_its_place_and_each_is_put_back_by_value() {
    let replayed = replay("change-automation-edit");
    let applies = of(&replayed, "change_apply");
    // The second event moved to beat 8 at 100: taken out and made again there.
    assert_eq!(applies[1].result()["applied"]["removed"], json!([{"positionBeats": 4.0, "value": 60, "number": 7, "parameter": "Volume"}]));
    assert_eq!(applies[1].result()["applied"]["made"], json!([{"positionBeats": 8.0, "value": 100, "number": 7, "parameter": "Volume"}]));
    let reads = of(&replayed, "read");
    assert_eq!(reads[0].result()["items"][0]["events"], json!([{"positionBeats": 0.0, "value": 90}, {"positionBeats": 8.0, "value": 100}]));
    let restores = of(&replayed, "change_restore");
    let both = json!([{"positionBeats": 0.0, "value": 90}, {"positionBeats": 4.0, "value": 60}]);
    assert_eq!(reads[1].result()["items"][0]["events"], both);
    assert_eq!(applies[2].result()["applied"]["removed"], json!([{"positionBeats": 0.0, "value": 90, "number": 7, "parameter": "Volume"}]));
    assert_eq!(reads[2].result()["items"][0]["events"], json!([{"positionBeats": 4.0, "value": 60}]));
    assert_eq!(reads[3].result()["items"][0]["events"], both);
    assert_eq!(restores[2].result()["restored"], json!({"events": []}));
    assert_eq!(reads[4].result()["total"], 0);
}

#[test]
fn the_mix_is_bounced_through_logics_bounce_dialog_and_found_on_disk_and_never_over_a_file() {
    let replayed = replay("change-bounce");
    let bounces = of(&replayed, "bounce");
    let bounced = bounces[0].result().clone();
    assert_eq!(bounced["bounced"], "/Users/me/programs/kumi-lanes/m5-files/m4b/Scratch bounce.wav");
    assert_eq!(
        (bounced["format"].as_str(), bounced["rate"].as_u64(), bounced["channels"].as_u64(), bounced["bits"].as_u64()),
        (Some("wav24"), Some(44100), Some(2), Some(24))
    );
    // Eight beats at Scratch's 160 BPM.
    assert_eq!(bounced["seconds"], 3.0);
    assert!(bounces[1].replayed["error"]["message"].as_str().unwrap().contains("is there already"));
    assert_eq!(bounces[2].replayed["error"]["message"], "path must end in .wav for wav24");
    assert!(bounces[3].replayed["error"]["message"].as_str().unwrap().contains("Kumi makes no folders"));
}

#[test]
fn every_track_is_exported_through_the_export_panel_and_the_new_files_are_answered() {
    let replayed = replay("change-export-tracks");
    let exports = of(&replayed, "export_tracks");
    assert_eq!(
        exports[0].result()["files"],
        json!([{"name": "01.wav", "bytes": 985522, "seconds": 3.715}]),
        "the producer's naming (Increment) and Scratch's one track with sound"
    );
    assert_eq!(exports[1].replayed["error"]["code"], "bad-params");
}

#[test]
fn the_producers_own_undo_of_a_plug_in_and_a_send_kumi_added_is_named_back() {
    let replayed = replay("change-undo-m4b");
    let polls = of(&replayed, "");
    // Each Cmd-Z takes Kumi's latest step off: the send first, then the plug-in, each named by its desc.
    let named: Vec<Value> = replayed.events.iter().map(|event| event["descs"].clone()).collect();
    assert_eq!(named, [json!(["Kumi: Added send from “Vocal” to Bus 2 (k0002)"]), json!(["Kumi: Added “Gain” to “Vocal” (k0001)"])]);
    assert!(polls.iter().all(|poll| poll.replayed["ok"] == true));
    let reads = of(&replayed, "read");
    assert_eq!(reads[0].result()["total"], 1);
    assert_eq!(reads[1].result()["total"], 0);
}

#[test]
fn a_track_whose_strip_has_nothing_smart_controls_map_reads_none_of_them() {
    // Vocal's strip holds a Channel EQ alone: the pane draws its header (the Controls and EQ views' buttons) over no
    // controls, which is no rows, not a failure.
    let replayed = replay("read-smart-none");
    let reads = of(&replayed, "read");
    assert_eq!(reads[0].result(), &json!({"items": [], "total": 0, "epoch": "1"}));
    assert_eq!(reads[1].result()["items"][0]["name"], "Channel EQ");
}

#[test]
fn a_parameter_lands_on_the_number_asked_when_its_steps_allow() {
    // The Stereo Out's Gain asked for -3 dB: its bisection used to stop at the first reading within a quarter of a dB
    // (-3.1 dB), and goes on now until one reads -3 exactly, which raw 930 does.
    let replayed = replay("change-fx-param-exact");
    let applies = of(&replayed, "change_apply");
    let set = applies[1].result();
    assert_eq!(set["applied"]["params"][0], json!({"index": 2, "name": "Gain", "value": 930.0, "text": "-3.0 dB"}));
    assert!(set["display"].get("note").is_none(), "the number was reached: no resolution to say");
    assert_eq!(of(&replayed, "read")[0].result()["items"][0]["text"], "-3.0 dB");
    let restores = of(&replayed, "change_restore");
    assert_eq!(restores[0].result()["restored"]["params"][0]["text"], "0.0 dB");
    assert_eq!(restores[1].result()["restored"]["deleted"], "1:plugin:0.1.1");
    assert_eq!(of(&replayed, "read")[1].result()["items"], json!([]));
}

#[test]
fn a_bounce_adds_nothing_to_the_project_so_logics_own_undo_still_takes_back_kumis_step_under_it() {
    // The Bounce dialog's Add to Project put the file in the project's audio, a step of Logic's undo ("Bounce
    // Project") on top of Kumi's: Kumi unchecks it (and Add to Apple Music Library) before OK.
    let bounce = replay("change-bounce");
    let unchecked = bounce_scene_presses("change-bounce");
    assert!(unchecked.iter().any(|(path, before, after)| path == "3.4" && *before == 1 && *after == 0), "{unchecked:?}");
    assert_eq!(of(&bounce, "bounce")[0].result()["seconds"], 3.0);
    // A split, a bounce, then the split taken back through Logic's own undo, which still has the split on top.
    let replayed = replay("change-bounce-undo");
    assert_eq!(of(&replayed, "bounce")[0].result()["seconds"], 1.5);
    let restored = of(&replayed, "change_restore")[0].result().clone();
    assert_eq!((restored["restored"]["pressed"].as_str(), restored["undo"]["how"].as_str()), (Some("Undo Split Regions"), Some("undo")));
    assert_eq!(of(&replayed, "read")[0].result()["items"].as_array().unwrap().len(), 1, "the region whole again");
}

/// The presses a scene made in the Bounce dialog: each one's path and its value before and after.
fn bounce_scene_presses(name: &str) -> Vec<(String, i64, i64)> {
    load(name)
        .calls
        .iter()
        .filter(|call| {
            call.op == "action"
                && call.params.get("root").and_then(|r| r.get("title")).and_then(Value::as_str).is_some_and(|t| t.starts_with("Bounce"))
        })
        .filter_map(|call| {
            Some((call.params.get("path")?.as_str()?.to_string(), call.reply.get("before")?.as_i64()?, call.reply.get("after")?.as_i64()?))
        })
        .collect()
}

#[test]
fn a_split_after_logic_played_lands_exactly_on_the_bar_asked() {
    // Logic played and stopped, so the playhead's division and tick are wherever the stop left them, and the LCD's
    // bar and beat sliders keep them: the playhead goes to the project's start first ("Go to Beginning"), then to
    // bar 3, and the split lands there exactly.
    let replayed = replay("change-playhead-exact");
    let presses: Vec<String> = load("change-playhead-exact")
        .calls
        .iter()
        .filter(|call| call.op == "action")
        .filter_map(|call| call.params.get("path").and_then(Value::as_str).map(str::to_string))
        .collect();
    assert_eq!(presses, ["5.5", "5.6", "5.6"], "Play, Stop, then Go to Beginning before the playhead is placed");
    let applied = of(&replayed, "change_apply")[0].result().clone();
    assert_eq!(
        (applied["applied"]["left"]["endBeats"].as_f64(), applied["applied"]["right"]["startBeats"].as_f64()),
        (Some(8.0), Some(8.0))
    );
    let reads = of(&replayed, "read");
    assert_eq!(reads[0].result()["items"][1]["startBeats"], 8.0);
    assert_eq!(reads[1].result()["items"].as_array().unwrap().len(), 1, "the region whole again");
}

#[test]
fn the_metronome_is_turned_off_and_on_by_the_control_bars_metronome_click_and_makes_no_step_of_logics_undo() {
    let replayed = replay("change-metronome-set");
    let previews = of(&replayed, "change_preview");
    // Scratch's click is on: the preview fences it as read, and writes nothing.
    assert_eq!((&previews[0].result()["prior"], &previews[0].result()["fence"]), (&json!({"on": true}), &json!({"on": true})));
    assert_eq!(previews[0].result()["target"], json!({"ref": null, "kind": "project"}));
    let applied = of(&replayed, "change_apply")[0].result().clone();
    assert_eq!(applied["applied"], json!({"on": false}));
    assert_eq!((applied["display"]["title"].as_str(), applied["undo"]["entry"].as_bool()), (Some("Metronome off"), Some(false)));
    let presses: Vec<(i64, i64)> = load("change-metronome-set")
        .calls
        .iter()
        .filter(|call| call.op == "action")
        .filter_map(|call| Some((call.reply.get("before")?.as_i64()?, call.reply.get("after")?.as_i64()?)))
        .collect();
    assert_eq!(presses, [(1, 0), (0, 1)], "one press off, one press back on, each read back");
    let restored = of(&replayed, "change_restore")[0].result().clone();
    assert_eq!((&restored["restored"], restored["undo"]["entry"].as_bool()), (&json!({"on": true}), Some(false)));
    // On asked of an on click: nothing to do, so no step for the history either.
    assert_eq!(previews[1].result()["display"]["unchanged"], true);
    assert_eq!(previews[2].replayed["error"]["code"], "bad-params");
}

#[test]
fn the_recent_projects_are_logics_open_recent_menu_read_without_opening_it_with_a_path_where_one_is_known() {
    let replayed = replay("read-recent");
    let reads = of(&replayed, "read");
    let items = reads[0].result()["items"].as_array().unwrap().clone();
    assert_eq!(reads[0].result()["total"], 8);
    // The open project first, its path from its window; the others' from Spotlight, which finds the gate's copies
    // outside the Music folder.
    assert_eq!(items[0], json!({"index": 1, "name": "Scratch", "path": "/Users/me/Music/Logic/Scratch.logicx", "open": true}));
    assert_eq!(items[1]["name"], "Scratch Kumi M4 20261021-142756");
    assert!(items[1]["path"].as_str().unwrap().ends_with("/Scratch Kumi M4 20261021-142756.logicx"));
    assert!(items[1..].iter().all(|item| item["open"] == false && item["path"].is_string()));
    assert_eq!((reads[1].result()["items"].as_array().unwrap().len(), reads[1].result()["nextCursor"].as_str()), (2, Some("1.2")));
    // Only reads of the menu bar: no menu was opened.
    let scene = load("read-recent");
    assert!(scene.calls.iter().all(|call| call.op != "action" && call.op != "menu" && call.op != "keys"));
}

#[test]
fn the_bulk_read_answers_the_paged_reads_rows_in_one_request_showing_each_pane_once() {
    let paged = replay("read-paged");
    let bulk = replay("read-bulk");
    let answer = of(&bulk, "snapshot")[0].result().clone();
    let pages = of(&paged, "read");
    for (at, kind) in ["project", "track", "region", "plugin", "marker", "tempo"].iter().enumerate() {
        assert_eq!(answer["rows"][*kind], pages[at].result()["items"], "{kind}: the bulk read's rows are the page's");
        assert_eq!(answer["totals"][*kind], pages[at].result()["total"], "{kind}");
    }
    assert_eq!((answer["epoch"].as_str(), answer.get("truncated")), (Some("1"), None));
    assert_eq!(answer["rows"]["tempo"][0]["tempos"][0]["bpm"], 160.0);
    // The List Editors shown once for the three lists (their button pressed to open and to close), against once per
    // list for the pages; the Mixer once in each.
    let presses = |name: &str, label: &str| -> usize {
        let scene = load(name);
        let buttons: Vec<String> = scene
            .calls
            .iter()
            .filter(|call| call.op == "tree")
            .flat_map(|call| {
                let mut found = vec![];
                collect_buttons(&call.reply["node"], label, &mut found);
                found
            })
            .collect();
        scene.calls.iter().filter(|call| call.op == "action" && buttons.iter().any(|b| call.params["path"] == b.as_str())).count()
    };
    assert_eq!((presses("read-bulk", "List Editors"), presses("read-paged", "List Editors")), (2, 6));
    assert_eq!((presses("read-bulk", "Mixer"), presses("read-paged", "Mixer")), (2, 2));
    // Measured on Scratch: the six pages took 4.4 to 4.5 s, the bulk read 2.7 s.
    let ms = |name: &str| -> u64 { load(name).logic["steps"].as_array().unwrap().iter().filter_map(|step| step["ms"].as_u64()).sum() };
    assert!(ms("read-bulk") < ms("read-paged"), "{} ms against {} ms", ms("read-bulk"), ms("read-paged"));
}

/// The paths of the control bar's view buttons of a label in a recorded tree.
fn collect_buttons(node: &Value, label: &str, found: &mut Vec<String>) {
    if node["role"] == "AXCheckBox" && (node["desc"] == label || node["title"] == label) {
        if let Some(path) = node["i"].as_str() {
            found.push(path.to_string());
        }
    }
    for child in node["c"].as_array().into_iter().flatten() {
        collect_buttons(child, label, found);
    }
}

#[test]
fn a_sample_comes_in_on_an_audio_track_and_logic_converts_it_into_a_new_sampler_track_and_its_undo_takes_both_back() {
    // Recorded on a copy of Scratch: Logic copies the file into the project's package at once.
    let replayed = replay("change-sample-load");
    let previews = of(&replayed, "change_preview");
    assert_eq!(previews[0].replayed["error"]["code"], "bad-params", "Quick Sampler takes a sample by a drag only");
    assert!(previews[0].replayed["error"]["message"].as_str().unwrap().contains("Quick Sampler"));
    assert_eq!(previews[1].replayed["error"]["code"], "bad-params", "a file that isn't there");
    assert_eq!(previews[2].replayed["error"]["code"], "bad-params", "Keys is an instrument track");
    let preview = previews[3].result();
    assert_eq!(preview["fence"], json!({"tracks": 2, "names": ["Vocal", "Keys"], "regions": 1}));
    let applied = of(&replayed, "change_apply")[0].result().clone();
    assert_eq!(
        (applied["applied"]["name"].as_str(), applied["applied"]["instrument"].as_str(), applied["applied"]["track"].as_u64()),
        (Some("kumi-tone"), Some("Sampler"), Some(2))
    );
    // The new track's trigger region where the audio region came in (bar 3), and the audio region left muted on Vocal.
    assert_eq!(applied["applied"]["startBeats"], 8.0);
    assert_eq!(applied["created"]["ref"], format!("{}:track:2", applied["epoch"].as_str().unwrap()));
    assert!(applied["created"]["region"].is_string() && applied["created"]["source"].is_string());
    assert_eq!(applied["undo"]["entry"], true);
    let reads = of(&replayed, "read");
    let names: Vec<&str> = reads[0].result()["items"].as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap()).collect();
    assert_eq!(names, ["Vocal", "kumi-tone", "Keys"]);
    assert_eq!(reads[0].result()["items"][1]["kind"], "instrument");
    let restored = of(&replayed, "change_restore")[0].result().clone();
    assert_eq!(restored["undo"]["how"], "undo");
    assert_eq!(restored["restored"]["pressed"], json!(["Undo Convert Regions to New Sampler Track", "Undo Insert"]));
    assert_eq!(reads[2].result()["total"], 2, "the Sampler track gone");
    assert_eq!(reads[3].result()["total"], 1, "the audio region gone too");
}

#[test]
fn the_compressors_window_is_found_past_its_editors_controls_and_its_controls_view_reads_as_channel_eqs_does() {
    // Scratch's Keys holds a Compressor, whose editor puts its own controls at the window's top level with the header's
    // (44 in all), the texts naming the plug-in and the strip last: the header read cut at 40 is made again whole, so
    // the window is found, its view switched to Controls, its rows read, its editor put back and the window closed.
    let replayed = replay("read-param-compressor");
    let reads = of(&replayed, "read");
    let page = reads[0].result();
    assert_eq!(page["total"], 25);
    let items = page["items"].as_array().unwrap();
    let names: Vec<&str> = items.iter().map(|row| row["name"].as_str().unwrap()).collect();
    assert_eq!(names[..6], ["Threshold", "Ratio", "Attack", "Release", "Make Up", "Knee"]);
    assert_eq!(
        items[0],
        json!({"ref": "1:param:2.3.1", "pluginRef": "1:plugin:2.3", "index": 1, "name": "Threshold", "value": 65.0, "text": "-17.5 dB", "min": 0.0, "max": 100.0, "normalized": 0.65})
    );
    let shown = |name: &str| items.iter().find(|row| row["name"] == name).map(|row| row["text"].clone()).unwrap();
    assert_eq!((shown("Ratio"), shown("Make Up"), shown("Mix")), (json!("3.9:1"), json!("0.0 dB"), json!("59.0 %")));
    // A value the view shows with no control (its slider disabled, with no range) reads as its text alone, as a menu's.
    let knee = items.iter().find(|row| row["name"] == "Knee").unwrap();
    assert_eq!((knee["text"].as_str(), knee["value"].is_null(), knee["min"].is_null()), (Some("0.7"), true, true));
    assert_eq!(reads[1].result()["items"][0], items[0], "the Threshold by its ref");
    let previews = of(&replayed, "change_preview");
    assert_eq!(previews[0].replayed["error"]["code"], "bad-params");
    assert!(previews[0].replayed["error"]["message"].as_str().unwrap().contains("Knee with no control"));
    // The setting is the header's last pop-up: the Compressor lists its Side Chain pop-up ("Internal") before it.
    assert_eq!(previews[1].result()["fence"], json!({"plugin": "Compressor", "preset": "Custom"}));
    let scene = load("read-param-compressor");
    let headers: Vec<(u64, usize)> = scene
        .calls
        .iter()
        .filter(|call| call.op == "tree" && call.params["root"] == json!({"title": "Keys"}) && call.params["depth"] == 1)
        .map(|call| (call.params["children"].as_u64().unwrap(), call.reply["node"]["c"].as_array().map_or(0, Vec::len)))
        .collect();
    assert!(headers.contains(&(40, 40)) && headers.contains(&(256, 44)), "{headers:?}");
    // Each read opened the window and closed it after, its view put back to the editor.
    let presses = |path: &str| {
        scene
            .calls
            .iter()
            .filter(|call| call.op == "action" && call.params["root"] == json!({"title": "Keys"}) && call.params["path"] == path)
            .count()
    };
    assert_eq!((presses("3.0.0"), presses("3.0.1"), presses("0")), (3, 3, 4), "Controls, Editor, close");
}

#[test]
fn a_sample_loads_restore_reads_back_that_the_imports_region_went_too_and_says_which_undo_didnt_land() {
    // The sample-load scene with the Tracks area read after both undos as the restore read it before them: the Sampler
    // track's header is gone, but the audio region the import made still shows, so the restore answers `uncertain`
    // and names the import's undo. (Replayed as recorded, the region is gone and the restore answers as Logic did.)
    let mut scene = load("change-sample-load");
    let pressed = |titles: Value| scene.calls.iter().position(|call| call.op == "menu" && call.params["titles"] == titles).unwrap();
    let (conversion, import) =
        (pressed(json!(["Edit", "Undo Convert Regions to New Sampler Track"])), pressed(json!(["Edit", "Undo Insert"])));
    let regions = |call: &logic_kumi::fixtures::Call| call.op == "tree" && call.params["depth"] == 3 && call.params["children"] == 256;
    let before = (0..conversion).rev().find(|at| regions(&scene.calls[*at])).unwrap();
    let after = (import..scene.calls.len()).find(|at| scene.calls[*at].params == scene.calls[before].params).unwrap();
    scene.calls[after].reply = scene.calls[before].reply.clone();
    let replayed = replay_scene(&scene);
    let restore = &of(&replayed, "change_restore")[0].replayed;
    assert_eq!(restore["error"]["code"], "uncertain", "{restore}");
    assert_eq!(
        restore["error"]["message"],
        "Logic Pro took the Sampler track back, but its “Undo Insert” didn't land: the audio region of “kumi-tone” is still on “Vocal”"
    );
    // The headers read after both undos as before them: the conversion's undo is the step named.
    let mut scene = load("change-sample-load");
    let headers = |call: &logic_kumi::fixtures::Call| call.op == "tree" && call.params["depth"] == 2 && call.params["children"] == 256;
    let before = (0..conversion).rev().find(|at| headers(&scene.calls[*at])).unwrap();
    let later: Vec<usize> =
        (import..scene.calls.len()).filter(|at| scene.calls[*at].params == scene.calls[before].params).take(2).collect();
    for at in later {
        scene.calls[at].reply = scene.calls[before].reply.clone();
    }
    let replayed = replay_scene(&scene);
    let restore = &of(&replayed, "change_restore")[0].replayed;
    assert_eq!(
        (restore["error"]["code"].as_str(), restore["error"]["message"].as_str()),
        (
            Some("uncertain"),
            Some("Logic Pro's “Undo Convert Regions to New Sampler Track” was pressed, but the Sampler track is still there")
        )
    );
}
