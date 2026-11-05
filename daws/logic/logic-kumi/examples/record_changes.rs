//! Records a change scene from the real Logic Pro: a run of change requests (a preview, its apply, its restore, a
//! transport call, a poll of the undo watch) through the bridge's own change side, with every helper reply the run
//! asked for, written as `scene.json.gz` under the folder given, for `tests/it/changes.rs` to replay without Logic. Opt-in:
//! it changes the running Logic's project through the helper, as the bridge does, and puts it back as the steps say.
//!
//! cargo run -p logic-kumi --example record_changes -- --out daws/logic/logic-kumi/tests/fixtures/<scene> --steps '<json>' [--helper <path>]
//!
//! `--steps` is a list: `{"preview": {"kind", "input"}}`, `{"apply": {"desc"}}` (the last preview's), `{"restore":
//! {"desc", "discard"?}}` (the last preview's and apply's), `{"call": {"method", "params"}}` for any other method,
//! `{"poll": true}` (one focus refresh and the undo watch's poll, its events kept), `{"wait": ms}`, and `{"undo":
//! true}` or `{"redo": true}`, which press Cmd-Z or Shift-Cmd-Z in Logic through a helper of their own, unrecorded, as
//! the producer would. `{epoch}` in a step's text stands for the epoch at that step. Each step's answer is kept in
//! the scene under `logic.steps`, and printed with its time.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use logic_kumi::ax::strings::{self, Strings};
use logic_kumi::ax::Ax;
use logic_kumi::changes::Changes;
use logic_kumi::fixtures::{Recorder, Scene, SCENE_FILE};
use logic_kumi::helper::{ChildLauncher, Helper, Launch};
use logic_kumi::logic::{self, Model, BUNDLES};
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
    let (mut out, mut helper_path, mut steps) = (None, None, None);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--out" => out = Some(PathBuf::from(args.next().expect("--out <dir>"))),
            "--helper" => helper_path = Some(PathBuf::from(args.next().expect("--helper <path>"))),
            "--steps" => steps = Some(args.next().expect("--steps <json>")),
            other => panic!("unknown argument {other}; usage: record_changes --out <dir> --steps <json> [--helper <path>]"),
        }
    }
    let out = out.expect("--out <dir>: where to write scene.json.gz");
    let steps: Vec<Value> = serde_json::from_str(&steps.expect("--steps <json>")).expect("steps are a JSON list");
    let name = out.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "scene".into());
    let helper_path = helper_path
        .or_else(|| std::env::var("KUMI_LOGIC_HANDS").ok().map(PathBuf::from))
        .or_else(|| {
            let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../target/logic-hands");
            std::fs::read_dir(dir)
                .ok()?
                .flatten()
                .map(|e| e.path())
                .find(|p| p.file_name().is_some_and(|n| n.to_string_lossy().starts_with("kumi-logic-hands-")))
        })
        .expect("the helper: build it with python3 scripts/build-logic-hands.py, or name it with --helper");
    let mut launcher = ChildLauncher { path: helper_path, stop: None };
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
    // The first look, as the run loop's first refresh: the shape the epoch moves from.
    {
        let mut ax = Ax::new(helper.as_mut(), app.pid);
        project.look(&mut ax, None).expect("a first look at the project");
    }

    let (mut preview, mut applied): (Option<(Value, Value)>, Option<Value>) = (None, None);
    let mut recorded: Vec<Value> = Vec::new();
    let mut events: Vec<Value> = Vec::new();
    for step in steps {
        let step = with_epoch(&step, project.epoch);
        let started = Instant::now();
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
            }
            (json!({"method": "change_apply", "params": params}), Some(answer))
        } else if let Some(asked) = step.get("restore") {
            let (params, answer) = preview.clone().expect("a preview before the restore");
            let done = applied.clone().expect("an apply before the restore");
            let mut params = json!({"kind": params["kind"], "target": answer["target"], "prior": answer["prior"], "applied": done["applied"], "desc": asked["desc"], "epoch": project.epoch.to_string()});
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
            } else {
                serve::answer(&mut project, helper.as_mut(), app.pid, method, &params)
            };
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
            recorded.push(json!({"step": what, "answer": kept, "ms": ms}));
        }
    }
    while helper.event(Duration::from_millis(50)).is_some() {}
    helper.quit();
    {
        let mut scene = scene.lock().unwrap();
        scene.logic = json!({"version": version, "bundle": app.bundle, "pid": app.pid, "language": language, "project": project_json, "steps": recorded, "undoneInDaw": events})
            .as_object()
            .cloned()
            .unwrap_or_default();
    }
    let scene = scene.lock().unwrap().clone();
    scene.save(&out).expect("write the scene");
    eprintln!("{} helper calls recorded to {}: {:?}", scene.calls.len(), out.join(SCENE_FILE).display(), scene.counts());
}
