//! Records a change scene from the real Logic Pro: a run of change requests (a preview, its apply, its restore, a
//! transport call, a poll of the undo watch) through the bridge's own change side, with every helper reply the run
//! asked for, written as `scene.json.gz` under the folder given, for `tests/changes.rs` to replay without Logic. Opt-in:
//! it changes the running Logic's project through the helper, as the bridge does, and puts it back as the steps say.
//!
//! cargo run -p logic-kumi --example record_changes -- --out crates/logic-kumi/tests/fixtures/<scene> --steps '<json>' [--helper <path>] [--mcu]
//!
//! `--steps` is a list: `{"preview": {"kind", "input"}}`, `{"apply": {"desc"}}` (the last preview's), `{"restore":
//! {"desc", "discard"?, "of"?}}` (the last preview's and apply's, or with `of` the apply of that place in the run, from
//! 0, as a host's journal undoes an earlier change), `{"call": {"method", "params"}}` for any other method,
//! `{"poll": true}` (one focus refresh and the undo watch's poll, its events kept), `{"wait": ms}`, and `{"undo":
//! true}` or `{"redo": true}`, which press Cmd-Z or Shift-Cmd-Z in Logic through a helper of their own, unrecorded, as
//! the producer would. `{epoch}` in a step's text stands for the epoch at that step. Each step's answer is kept in
//! the scene under `logic.steps`, and printed with its time; what the bridge looked at on disk (a bounce's file, an
//! export's folder) under `logic.disk`, which a replay answers from.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use logic_kumi::ax::strings::{self, Strings};
use logic_kumi::ax::Ax;
use logic_kumi::changes::Changes;
use logic_kumi::disk;
use logic_kumi::fixtures::{Recorder, Scene, SCENE_FILE};
use logic_kumi::helper::{ChildLauncher, Helper, Launch};
use logic_kumi::listen::audio;
use logic_kumi::logic::{self, Model, BUNDLES};
use logic_kumi::mcu::Mcu;
use logic_kumi::project::Project;
use logic_kumi::serve;
use serde_json::{json, Value};

/// The step with `{epoch}` in its strings replaced by the epoch.
fn with_epoch(step: &Value, epoch: u32) -> Value {
    match step {
        Value::String(text) => Value::String(text.replace("{epoch}", &epoch.to_string())),
        Value::Array(items) => Value::Array(items.iter().map(|item| with_epoch(item, epoch)).collect()),
        Value::Object(fields) => Value::Object(fields.iter().map(|(key, value)| (key.clone(), with_epoch(value, epoch))).collect()),
        other => other.clone(),
    }
}

fn main() {
    let mut args = std::env::args().skip(1);
    let (mut out, mut helper_path, mut steps, mut with_mcu) = (None, None, None, false);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--out" => out = Some(PathBuf::from(args.next().expect("--out <dir>"))),
            "--helper" => helper_path = Some(PathBuf::from(args.next().expect("--helper <path>"))),
            "--steps" => steps = Some(args.next().expect("--steps <json>")),
            "--mcu" => with_mcu = true,
            other => panic!("unknown argument {other}; usage: record_changes --out <dir> --steps <json> [--helper <path>] [--mcu]"),
        }
    }
    let out = out.expect("--out <dir>: where to write scene.json.gz");
    let steps: Vec<Value> = serde_json::from_str(&steps.expect("--steps <json>")).expect("steps are a JSON list");
    let name = out.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "scene".into());
    let helper_path = helper_path
        .or_else(|| std::env::var("KUMI_LOGIC_HANDS").ok().map(PathBuf::from))
        .or_else(|| {
            let dir = kumi_testkit::workspace_path("target/logic-hands");
            std::fs::read_dir(dir)
                .ok()?
                .flatten()
                .map(|e| e.path())
                .find(|p| p.file_name().is_some_and(|n| n.to_string_lossy().starts_with("kumi-logic-hands-")))
        })
        .expect("the helper: build it with python3 scripts/build-logic-hands.py, or name it with --helper");
    let mut launcher = ChildLauncher { path: helper_path };
    let scene = Arc::new(Mutex::new(Scene::new(&name)));
    let mut helper: Box<dyn Helper> = Box::new(Recorder::new(launcher.launch().expect("the helper"), scene.clone()));

    // As the bridge attaches: the apps, the grant, the windows.
    let apps = logic::parse_apps(&helper.call("apps", json!({"bundles": BUNDLES})).expect("apps"));
    let app = logic::newest(&apps).expect("a running Logic Pro").clone();
    let version = logic::app_version(&app);
    assert!(
        helper.call("trusted", json!({"prompt": false})).expect("trusted")["trusted"] == true,
        "the helper isn't trusted under Accessibility"
    );
    let mut model = Model::attach(&app, &version);
    model.on_windows(&helper.call("windows", json!({"pid": app.pid})).expect("windows"));
    if let Some(busy) = model.busy {
        panic!("Logic Pro can't take work: {} (bring its window to the current Space, close the dialog)", busy.reason());
    }
    let project_json = model.project.clone();
    let (id, path, pname) =
        project_json.as_ref().map(|p| (p.id.clone(), p.path.clone(), p.name.clone().unwrap_or_default())).unwrap_or_default();
    eprintln!("recording {name} from Logic Pro {version} (pid {}), {}", app.pid, pname);
    let language = strings::language_of(&strings::preferred_languages(&app.bundle));
    let bundle = app.path.clone().map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/Applications/Logic Pro.app"));
    let strings = Strings::load(&bundle, &language);
    let mut project = Project::new(strings);
    project.set_identity(id.as_deref(), path.as_deref(), &pname);
    let mut changes = Changes::default();
    let looked = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    disk::set_hook(Some(disk::recording(looked.clone())));
    // The first look, as the run loop's first refresh: the shape the epoch moves from.
    {
        let mut ax = Ax::new(helper.as_mut(), app.pid);
        project.look(&mut ax, None).expect("a first look at the project");
    }
    // The Mackie Control, as the bridge opens it once Logic can be read: Logic installs the surface meanwhile.
    let mut mcu = Mcu::default();
    let mut install_events = 0;
    if with_mcu {
        mcu.open(helper.as_mut()).expect("the Mackie Control's ports");
        mcu.pump(helper.as_mut(), Duration::from_millis(1500));
        install_events = scene.lock().unwrap().events.len();
        eprintln!("Mackie Control installed {} ({install_events} events): {:?}", mcu.surface.state.installed, mcu.surface.state.names());
    }

    let (mut preview, mut applied): (Option<(Value, Value)>, Option<Value>) = (None, None);
    // Every apply of the run with its preview, for a restore that names an earlier one.
    let mut journal: Vec<((Value, Value), Value)> = Vec::new();
    let mut recorded: Vec<Value> = Vec::new();
    let mut events: Vec<Value> = Vec::new();
    for step in steps {
        let step = with_epoch(&step, project.epoch);
        let started = Instant::now();
        let mut checked: Option<Value> = None;
        let (what, answer): (Value, Option<Result<Value, (String, String)>>) = if let Some(wait) = step.get("wait").and_then(Value::as_u64)
        {
            std::thread::sleep(Duration::from_millis(wait));
            (step.clone(), None)
        } else if let Some(asked) = step.get("preview") {
            let params = json!({"kind": asked["kind"], "input": asked["input"], "epoch": project.epoch.to_string()});
            let answer = serve::answer_change(&mut project, &mut changes, helper.as_mut(), app.pid, "change_preview", &params);
            if let Ok(answer) = &answer {
                preview = Some((params.clone(), answer.clone()));
            }
            (json!({"method": "change_preview", "params": params}), Some(answer))
        } else if let Some(asked) = step.get("apply") {
            let (params, answer) = preview.clone().expect("a preview before the apply");
            let params = json!({"kind": params["kind"], "input": params["input"], "fence": answer["fence"], "desc": asked["desc"], "epoch": project.epoch.to_string()});
            let answer = serve::answer_change(&mut project, &mut changes, helper.as_mut(), app.pid, "change_apply", &params);
            if let Ok(answer) = &answer {
                applied = Some(answer.clone());
                journal.push((preview.clone().expect("the preview"), answer.clone()));
            }
            (json!({"method": "change_apply", "params": params}), Some(answer))
        } else if let Some(asked) = step.get("restore") {
            let ((params, answer), done) = match asked.get("of").and_then(Value::as_u64) {
                Some(of) => journal.get(of as usize).cloned().expect("the apply a restore names"),
                None => (preview.clone().expect("a preview before the restore"), applied.clone().expect("an apply before the restore")),
            };
            // The target where the apply left it when the apply said (a region moved or renamed), as the host's journal keeps it.
            let target = done.get("target").filter(|t| !t.is_null()).cloned().unwrap_or_else(|| answer["target"].clone());
            let mut params = json!({"kind": params["kind"], "target": target, "prior": answer["prior"], "applied": done["applied"], "desc": asked["desc"], "epoch": project.epoch.to_string()});
            for key in ["created", "material"] {
                if let Some(value) = done.get(key) {
                    params[key] = value.clone();
                }
            }
            if asked["discard"] == true {
                params["discard"] = true.into();
            }
            let answer = serve::answer_change(&mut project, &mut changes, helper.as_mut(), app.pid, "change_restore", &params);
            (json!({"method": "change_restore", "params": params}), Some(answer))
        } else if let Some(asked) = step.get("call") {
            let method = asked["method"].as_str().expect("call.method");
            let mut params = asked["params"].clone();
            if params.is_null() {
                params = json!({});
            }
            if serve::CHANGE_METHODS.contains(&method) && params.get("epoch").is_none() {
                params["epoch"] = project.epoch.to_string().into();
            }
            let answer = if serve::CHANGE_METHODS.contains(&method) {
                serve::answer_change(&mut project, &mut changes, helper.as_mut(), app.pid, method, &params)
            } else if serve::LISTEN_METHODS.contains(&method) {
                let surface = if mcu.open { Some(&mut mcu) } else { None };
                serve::answer_listen(&mut project, surface, helper.as_mut(), app.pid, method, &params, &mut || {})
            } else {
                serve::answer(&mut project, helper.as_mut(), app.pid, method, &params)
            };
            if let (Ok(answered), true) = (&answer, serve::LISTEN_METHODS.contains(&method)) {
                checked = Some(check_files(answered, &params, step.get("onsets"), step.get("keep") == Some(&json!(true))));
            }
            (json!({"method": method, "params": params}), Some(answer))
        } else if step.get("poll").is_some() {
            let mut ax = Ax::new(helper.as_mut(), app.pid);
            let (look, headers, _) = project.look(&mut ax, None).expect("a look");
            let mut got = Vec::new();
            let carry = |from: u32, place: usize| project.carry_track(from, place);
            changes.watch.poll(&mut ax, &project.strings, &look, &headers, &mut |data| got.push(data), &carry);
            events.extend(got.iter().cloned());
            (json!({"poll": true}), Some(Ok(json!({"epoch": look.epoch.to_string(), "events": got}))))
        } else if step.get("undo").is_some() || step.get("redo").is_some() {
            // The producer's own undo: through a helper of its own, so the recording holds none of it.
            let mut hands = launcher.launch().expect("a helper for the keys");
            let mods: Vec<&str> = if step.get("redo").is_some() { vec!["cmd", "shift"] } else { vec!["cmd"] };
            hands.call("activate", json!({"pid": app.pid})).expect("activate");
            std::thread::sleep(Duration::from_millis(200));
            let reply = hands.call("keys", json!({"pid": app.pid, "key": "z", "mods": mods})).expect("keys");
            std::thread::sleep(Duration::from_millis(600));
            hands.quit();
            (step.clone(), Some(Ok(json!({"pressed": reply["active"]}))))
        } else {
            panic!("a step is preview, apply, restore, call, poll, wait, undo or redo: {step}");
        };
        let ms = started.elapsed().as_millis();
        let kept = match &answer {
            Some(Ok(result)) => json!({"ok": true, "result": result}),
            Some(Err((code, message))) => json!({"ok": false, "error": {"code": code, "message": message}}),
            None => Value::Null,
        };
        eprintln!("{} in {ms} ms -> {}", what, if kept.is_null() { "-".to_string() } else { kept.to_string() });
        if !kept.is_null() {
            let mut record = json!({"step": what, "answer": kept, "ms": ms});
            if let Some(onsets) = checked.filter(|onsets| !onsets.is_null()) {
                eprintln!("  onsets: {onsets}");
                record["onsets"] = onsets;
            }
            recorded.push(record);
        }
    }
    // The surface's ports closed first: Logic streams the Mackie Control's meters and timecode while they are open.
    if mcu.open {
        mcu.close(helper.as_mut());
    }
    let until = Instant::now() + Duration::from_secs(2);
    while Instant::now() < until && helper.event(Duration::from_millis(50)).is_some() {}
    helper.quit();
    disk::set_hook(None);
    {
        let mut scene = scene.lock().unwrap();
        let mut logic = json!({"version": version, "bundle": app.bundle, "pid": app.pid, "language": language, "project": project_json, "steps": recorded, "undoneInDaw": events});
        if with_mcu {
            logic["installEvents"] = install_events.into();
        }
        let looked = looked.borrow().clone();
        if !looked.is_empty() {
            logic["disk"] = Value::Array(looked);
        }
        scene.logic = logic.as_object().cloned().unwrap_or_default();
    }
    let scene = scene.lock().unwrap().clone();
    scene.save(&out).expect("write the scene");
    eprintln!("{} helper calls recorded to {}: {:?}", scene.calls.len(), out.join(SCENE_FILE).display(), scene.counts());
}

/// A listening answer's files: each one's onsets placed against the beats they were expected on (`expected`, beats
/// from the project's start: where the beats say each lands in the file, at the tempo the file's length and the part's
/// beats give), then the files deleted unless kept. The onsets as `[{file, beat, ms, error}]` (`error` the onset's
/// distance from where the beat says, in milliseconds), or null when none were expected.
fn check_files(answer: &Value, params: &Value, expected: Option<&Value>, keep: bool) -> Value {
    let files: Vec<&Value> = match answer.get("candidates").and_then(Value::as_array) {
        Some(candidates) => candidates.iter().collect(),
        None => vec![answer],
    };
    let from = params.get("fromBeats").and_then(Value::as_f64).unwrap_or(0.0);
    let beats = params.get("beats").and_then(Value::as_f64).unwrap_or(1.0);
    let wanted: Vec<f64> =
        expected.and_then(Value::as_array).map(|beats| beats.iter().filter_map(Value::as_f64).collect()).unwrap_or_default();
    let mut found = vec![];
    for take in files {
        let Some(file) = take.get("file").and_then(Value::as_str).map(PathBuf::from) else { continue };
        if !wanted.is_empty() {
            let seconds = take.get("seconds").and_then(Value::as_f64).unwrap_or(0.0);
            let per_beat = if beats > 0.0 { seconds / beats } else { 0.0 };
            let rate = take.get("sampleRate").and_then(Value::as_f64).unwrap_or(44100.0);
            // Read with 20 ms of silence before the file, so a note right at its start is found too.
            let onsets = audio::open(&file)
                .and_then(|(mut opened, wav)| {
                    let pad = (rate * 0.02) as usize;
                    let mut samples = vec![0f32; pad * usize::from(wav.channels)];
                    samples.extend(audio::read_frames(&mut opened, &wav, 0, wav.frames)?);
                    Ok(audio::onsets(&samples, usize::from(wav.channels), rate)
                        .into_iter()
                        .map(|frame| frame - pad as f64)
                        .collect::<Vec<f64>>())
                })
                .unwrap_or_default();
            for beat in &wanted {
                let at = (beat - from) * per_beat;
                let nearest = onsets.iter().map(|frame| frame / rate).min_by(|a, b| (a - at).abs().total_cmp(&(b - at).abs()));
                found.push(json!({
                    "file": file.file_name().map(|n| n.to_string_lossy().into_owned()),
                    "beat": beat,
                    "ms": nearest.map(|onset| (onset * 1000.0 * 100.0).round() / 100.0),
                    "error": nearest.map(|onset| ((onset - at) * 1000.0 * 100.0).round() / 100.0),
                }));
            }
        }
        if !keep {
            let _ = std::fs::remove_file(&file);
        }
    }
    if wanted.is_empty() {
        Value::Null
    } else {
        Value::Array(found)
    }
}
