//! Kumi's listening over recorded scenes of the real Logic Pro, without Logic: `hear`, `audition` and `render` as
//! `examples/record_changes.rs --mcu` recorded them, replayed through the bridge's own code with the Mackie Control's
//! events in the order they came, every answer compared to what the real Logic gave. A recording keeps no audio: the
//! scene's folder is made the test's own, and where a part was cut the replay leaves a synthesized file of its length
//! and format. The alignment scene keeps what its recorder measured in the real audio before deleting it: where each
//! note of the Piano region began in the file, against where the beats say.

mod common;

use std::path::{Path, PathBuf};
use std::time::Duration;

use common::{dialog, hands_showing, tracks_window, Lab, SONG};
use logic_kumi::ax::strings::Strings;
use logic_kumi::ax::Ax;
use logic_kumi::changes::{self, Changes, Ctx};
use logic_kumi::disk;
use logic_kumi::fixtures::{Scene, SceneHands};
use logic_kumi::helper::{Helper, Launch, MockHands};
use logic_kumi::listen;
use logic_kumi::mcu::Mcu;
use logic_kumi::project::Project;
use logic_kumi::serve;
use serde_json::{json, Value};

fn fixtures() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

/// Every listening scene, by its name.
const SCENES: [&str; 5] = ["listen-hear", "listen-audition", "listen-render", "listen-align", "listen-busy"];

/// One step of a scene replayed: what was asked, what the real Logic answered, what the replay answered, and what the
/// recorder measured in the audio.
struct Step {
    step: Value,
    recorded: Value,
    replayed: Value,
    onsets: Value,
}

impl Step {
    fn method(&self) -> &str {
        self.step["method"].as_str().unwrap_or("")
    }
    fn result(&self) -> &Value {
        &self.replayed["result"]
    }
}

struct Replayed {
    steps: Vec<Step>,
    unrecorded: Vec<(String, Value)>,
    /// The folder the scene's files went into, the test's own (named as the recorded one: a bounce's save panel is
    /// checked to stand in a folder of that name), and the temporary folder holding it.
    folder: PathBuf,
    _home: tempfile::TempDir,
}

/// The folder a scene's listening steps named.
fn recorded_folder(scene: &Value) -> Option<String> {
    scene["logic"]["steps"].as_array()?.iter().find_map(|record| record["step"]["params"]["folder"].as_str().map(str::to_string))
}

/// A scene with the folder it recorded into made a folder of the same name under `home`, wherever it is named
/// (requests, replies, looks, answers): that folder, made, and the scene.
fn relocated(name: &str, home: &Path) -> (PathBuf, Scene) {
    let text = logic_kumi::fixtures::scene_text(&fixtures().join(name))
        .unwrap_or_else(|error| panic!("no scene {name}: {error}; record it with record_changes --mcu"));
    let value: Value = serde_json::from_str(&text).unwrap();
    let (folder, text) = match recorded_folder(&value) {
        Some(recorded) => {
            let folder = home.join(Path::new(&recorded).file_name().expect("a folder's name"));
            let text = text.replace(&recorded, &folder.to_string_lossy());
            (folder, text)
        }
        None => (home.to_path_buf(), text),
    };
    std::fs::create_dir_all(&folder).unwrap();
    (folder, Scene::from_json(&serde_json::from_str(&text).unwrap()).unwrap())
}

fn replay(name: &str) -> Replayed {
    let home = tempfile::tempdir().unwrap();
    let (folder, scene) = relocated(name, home.path());
    changes::set_settling(false);
    let looked: Vec<Value> = scene.logic.get("disk").and_then(Value::as_array).cloned().unwrap_or_default();
    disk::set_hook(Some(disk::replaying(&looked)));
    let pid = scene.logic.get("pid").and_then(Value::as_u64).unwrap_or(1) as u32;
    let project = scene.logic.get("project").cloned().unwrap_or(Value::Null);
    let hands = SceneHands::new(&scene);
    let mut helper: Box<dyn Helper> = Launch::launch(&mut hands.clone()).unwrap();
    let mut state = Project::new(Strings::english());
    state.set_identity(
        project.get("id").and_then(Value::as_str),
        project.get("path").and_then(Value::as_str),
        project.get("name").and_then(Value::as_str).unwrap_or_default(),
    );
    // The attach the recorder made before the steps, then the surface's install, event for event.
    let _ = helper.call("apps", json!({"bundles": logic_kumi::logic::BUNDLES}));
    let _ = helper.call("trusted", json!({"prompt": false}));
    let _ = helper.call("windows", json!({"pid": pid}));
    {
        let mut ax = Ax::new(helper.as_mut(), pid);
        state.look(&mut ax, None).expect("the first look");
    }
    let mut mcu = Mcu::default();
    if let Some(installed) = scene.logic.get("installEvents").and_then(Value::as_u64) {
        mcu.open(helper.as_mut()).expect("the recorded ports");
        for _ in 0..installed {
            if let Some(event) = helper.event(Duration::ZERO) {
                mcu.on_event(&event, helper.as_mut());
            }
        }
    }
    let mut changes = Changes::default();
    let mut steps = vec![];
    for record in scene.logic.get("steps").and_then(Value::as_array).into_iter().flatten() {
        let step = record["step"].clone();
        let Some(method) = step.get("method").and_then(Value::as_str) else { continue };
        let params = step["params"].clone();
        let answer = if serve::LISTEN_METHODS.contains(&method) {
            let surface = if mcu.open { Some(&mut mcu) } else { None };
            serve::answer_listen(&mut state, surface, helper.as_mut(), pid, method, &params, &mut || {})
        } else if serve::CHANGE_METHODS.contains(&method) {
            serve::answer_change(&mut state, &mut changes, helper.as_mut(), pid, method, &params)
        } else {
            serve::answer(&mut state, helper.as_mut(), pid, method, &params)
        };
        let replayed = match answer {
            Ok(result) => json!({"ok": true, "result": result}),
            Err((code, message)) => json!({"ok": false, "error": {"code": code, "message": message}}),
        };
        steps.push(Step { step, recorded: record["answer"].clone(), replayed, onsets: record["onsets"].clone() });
    }
    disk::set_hook(None);
    Replayed { steps, unrecorded: hands.unrecorded(), folder, _home: home }
}

fn replayed(name: &str) -> Replayed {
    let replayed = replay(name);
    assert!(
        replayed.unrecorded.is_empty(),
        "{name}: the scene didn't record what listening asks now; record it again: {:?}",
        replayed.unrecorded
    );
    for step in &replayed.steps {
        assert_eq!(step.replayed, step.recorded, "{name}: {} answered otherwise than the real Logic did", step.step);
    }
    replayed
}

fn of<'a>(replayed: &'a Replayed, method: &str) -> Vec<&'a Step> {
    replayed.steps.iter().filter(|step| step.method() == method).collect()
}

/// A take's file is in the scene's folder, and the replay left a stand-in of its length and format there.
fn check_file(take: &Value, folder: &Path) {
    let file = PathBuf::from(take["file"].as_str().expect("a file"));
    assert_eq!(file.parent(), Some(folder), "{take}");
    let sound = disk::read_sound(&file).unwrap_or_else(|| panic!("no stand-in at {}", file.display()));
    assert_eq!(
        (u64::from(sound.rate), u64::from(sound.channels)),
        (take["sampleRate"].as_u64().unwrap(), take["channels"].as_u64().unwrap())
    );
    assert!((sound.frames as f64 / f64::from(sound.rate) - take["seconds"].as_f64().unwrap()).abs() < 0.001, "{take}");
}

#[test]
fn every_listening_scene_replays_as_the_real_logic_answered() {
    for name in SCENES {
        let replayed = replayed(name);
        assert!(!replayed.steps.is_empty(), "{name}");
    }
}

#[test]
fn a_part_is_heard_through_the_tap_cut_by_the_playheads_timecode_and_the_clicks_latency() {
    let replayed = replayed("listen-hear");
    let hears = of(&replayed, "hear");
    assert!(hears.len() >= 2, "the whole mix, and a track soloed");
    for hear in &hears {
        let take = hear.result();
        // Scratch at 160 BPM: four beats are 1.5 s, at the output's 44.1 kHz in stereo, with no lead.
        let beats = hear.step["params"]["beats"].as_f64().unwrap();
        assert!((take["seconds"].as_f64().unwrap() - beats * 0.375).abs() < 0.002, "{take}");
        assert_eq!((take["sampleRate"].as_u64(), take["channels"].as_u64(), take["lead"].as_f64()), (Some(44100), Some(2), Some(0.0)));
        assert_eq!(take["aligned"]["by"], "timecode+click", "{take}");
        assert!(take["aligned"]["errorMs"].as_f64().unwrap() < 10.0, "{take}");
        assert_eq!(take["metronome"], "off for the pass", "Scratch's metronome is on: it is off while Kumi hears");
        assert!(take["peak"].as_f64().unwrap() > 0.05 && take["rms"].as_f64().unwrap() > 0.005, "the Piano is heard: {take}");
        check_file(take, &replayed.folder);
    }
    assert_eq!(hears[1].result()["solo"], json!(["1:track:2"]));
}

#[test]
fn an_audition_hears_each_candidate_alone_in_one_capture_in_their_order() {
    let replayed = replayed("listen-audition");
    let audition = of(&replayed, "audition")[0].result().clone();
    let candidates = audition["candidates"].as_array().unwrap();
    assert_eq!(
        candidates.iter().map(|c| (c["ref"].clone(), c["name"].as_str().unwrap())).collect::<Vec<_>>(),
        [(json!("1:track:1"), "Vocal"), (json!("1:track:2"), "Keys"), (Value::Null, "the whole mix")]
    );
    assert_eq!(candidates[2]["mix"], true);
    // Vocal has no region: silence; the Keys alone and the mix are the Piano.
    assert!(candidates[0]["peak"].as_f64().unwrap() < 0.001, "{}", candidates[0]);
    assert!(candidates[1]["peak"].as_f64().unwrap() > 0.05 && candidates[2]["peak"].as_f64().unwrap() > 0.05);
    let files: std::collections::HashSet<&str> = candidates.iter().map(|c| c["file"].as_str().unwrap()).collect();
    assert_eq!(files.len(), 3);
    for candidate in candidates {
        check_file(candidate, &replayed.folder);
    }
    assert_eq!((audition["aligned"]["by"].as_str(), audition["metronome"].as_str()), (Some("timecode+click"), Some("off for the pass")));
}

#[test]
fn a_render_is_a_bounce_of_the_part_with_the_metronome_off() {
    let replayed = replayed("listen-render");
    let renders = of(&replayed, "render");
    for render in &renders {
        let result = render.result();
        let beats = render.step["params"]["beats"].as_f64().unwrap();
        assert!((result["seconds"].as_f64().unwrap() - beats * 0.375).abs() < 0.01, "{result}");
        assert_eq!((result["sampleRate"].as_u64(), result["channels"].as_u64(), result["bits"].as_u64()), (Some(44100), Some(2), Some(32)));
        assert_eq!(result["metronome"], "off for the pass");
        assert!(result["file"].as_str().unwrap().ends_with(".wav"));
    }
    let soloed = renders.iter().find(|render| render.step["params"].get("ref").is_some()).expect("a track rendered alone");
    assert!(soloed.result()["note"].as_str().unwrap().contains("Stereo Out"));
}

#[test]
fn the_piano_regions_notes_land_where_the_beats_say_within_a_few_milliseconds() {
    let replayed = replayed("listen-align");
    let hears = of(&replayed, "hear");
    assert!(hears.len() >= 4, "several runs");
    let mut errors = vec![];
    for hear in &hears {
        let onsets = hear.onsets.as_array().expect("the recorder's onsets");
        assert_eq!(onsets.len(), 4, "the region's four notes");
        for onset in onsets {
            errors.push(onset["error"].as_f64().unwrap_or_else(|| panic!("a note wasn't found: {onset}")));
        }
        assert!(hear.result()["aligned"]["errorMs"].as_f64().unwrap() < 10.0);
    }
    let worst = errors.iter().fold(0f64, |worst, error| worst.max(error.abs()));
    assert!(worst < 5.0, "the notes landed within {worst} ms of their beats: {errors:?}");
}

#[test]
fn logic_playing_is_busy_and_nothing_is_heard() {
    let replayed = replayed("listen-busy");
    let refused = &of(&replayed, "hear")[0].replayed;
    assert_eq!(refused["error"]["code"], "busy", "{refused}");
    assert!(refused["error"]["message"].as_str().unwrap().contains("playing"));
}

#[test]
fn a_listening_request_is_refused_before_logic_is_touched_when_it_asks_for_what_cant_be() {
    let hands = MockHands::new();
    let mut launcher = hands.clone();
    let mut helper = Launch::launch(&mut launcher).unwrap();
    let mut project = Project::new(Strings::english());
    let mut ax = Ax::new(helper.as_mut(), 7);
    let mut ctx = Ctx { project: &mut project, ax: &mut ax };
    let folder = tempfile::tempdir().unwrap();
    let ask = |extra: Value| {
        let mut params = json!({"fromBeats": 0, "beats": 4, "folder": folder.path().to_string_lossy()});
        for (key, value) in extra.as_object().unwrap() {
            params[key] = value.clone();
        }
        params
    };
    let refused = |ctx: &mut Ctx, method: &str, params: Value| listen::answer(ctx, None, method, &params, &mut || {}).unwrap_err();
    assert!(refused(&mut ctx, "hear", ask(json!({"beats": 0.1}))).1.contains("at least 0.25"));
    assert!(refused(&mut ctx, "hear", ask(json!({"folder": "relative/folder"}))).1.contains("whole path"));
    assert!(refused(&mut ctx, "hear", ask(json!({"folder": folder.path().join("none").to_string_lossy()}))).1.contains("isn't there"));
    assert!(refused(&mut ctx, "hear", ask(json!({"mute": "yes"}))).1.contains("mute"));
    assert!(refused(&mut ctx, "hear", ask(json!({"solo": "1:track:1"}))).1.contains("list"));
    assert_eq!(refused(&mut ctx, "hear", ask(json!({"solo": ["0:track:1"]}))).0, "stale");
    assert_eq!(refused(&mut ctx, "hear", ask(json!({"epoch": "9"}))).0, "stale");
    assert!(refused(&mut ctx, "audition", ask(json!({}))).1.contains("candidates"));
    assert!(refused(&mut ctx, "audition", ask(json!({"candidates": [{"track": 1}]}))).1.contains("mix"));
    assert_eq!(refused(&mut ctx, "listen", ask(json!({}))).0, "unknown-method");
    assert!(hands.calls().is_empty(), "nothing asked of Logic");
    // The wait the bridge gives a listening request: the part and a margin.
    assert_eq!(listen::bound_seconds("hear", &json!({"beats": 16}), 160.0), 6.0 + 28.0);
    assert_eq!(listen::bound_seconds("audition", &json!({"beats": 16, "candidates": [{}, {}]}), 120.0), 2.0 * (8.0 + 8.0) + 20.0);
    assert_eq!(listen::bound_seconds("render", &json!({"beats": 4}), 0.0), 4.0 + 45.0);
}

#[test]
fn hearing_while_a_dialog_is_up_is_busy_through_the_whole_bridge() {
    let behind = json!({"title": "Song - Tracks", "role": "AXWindow", "subrole": "AXStandardWindow", "main": false, "focused": false, "document": format!("file://{SONG}")});
    let hands = hands_showing(json!([dialog("Bounce"), behind]));
    let lab = Lab::start(&hands);
    let mut client = lab.client();
    client.status_when("modal", |status| status["busy"] == "modal");
    let folder = tempfile::tempdir().unwrap();
    for method in ["hear", "audition", "render"] {
        let refused = client.call_with(
            method,
            json!({"fromBeats": 0, "beats": 4, "folder": folder.path().to_string_lossy(), "candidates": [{"mix": true}]}),
        );
        assert_eq!(refused["error"]["code"], "busy", "{method}: {refused}");
    }
    assert_eq!(hands.calls_of("tap.start") + hands.calls_of("action"), 0);
    hands.set_windows(json!([tracks_window("Song - Tracks", Some(&format!("file://{SONG}")))]), &["Song - Tracks"]);
    drop(client);
    lab.finish();
}
