//! Records a scene from the real Logic Pro: every helper reply the bridge's readers ask for over the fixed request
//! set, written as `scene.json.gz` under the folder given, for `tests/reads.rs` to replay without Logic. Opt-in: it
//! drives the running Logic through the helper, as the bridge does. The scene to record is whatever Logic shows:
//! `scratch` is Scratch.logicx idle with track 1 selected; `scratch-keys` has track 2 and its region selected with
//! the Piano Roll open; `scratch-playing` plays; `scratch-dialog` has a dialog up.
//!
//! cargo run -p logic-kumi --example record_fixtures -- --out crates/logic-kumi/tests/fixtures/<scene> [--helper <path>] [--select <track>]
//!
//! `--select <track>` first selects that track (its place in the Tracks area, from 1) as a producer would, with a
//! click on its header's name field through the helper; `--region <place>` selects a region (its place through the
//! project) through its item; `--editors` opens the editors pane (the Piano Roll for a
//! MIDI region) first, `--play` presses Play, `--dialog` brings the "Choose a Project" dialog up. The scene starts
//! after that; `--restore` puts the panes, the transport and the dialog back after it. `--set m4b` asks the second
//! slice's reads (`scenes::m4b_requests`: a plug-in's parameters, Smart Controls, automation) instead of the fixed set.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use logic_kumi::ax::paths;
use logic_kumi::ax::strings::{self, Strings};
use logic_kumi::ax::{Ax, Root};
use logic_kumi::disk;
use logic_kumi::fixtures::{Recorder, Scene, SCENE_FILE};
use logic_kumi::helper::{ChildLauncher, Helper, Launch};
use logic_kumi::logic::{self, Model, BUNDLES};
use logic_kumi::project::Project;
use logic_kumi::scenes;
use serde_json::json;

fn main() {
    let mut args = std::env::args().skip(1);
    let (mut out, mut helper_path, mut select, mut region) = (None, None, None, None);
    let (mut editors, mut play, mut dialog, mut restore) = (false, false, false, false);
    let mut set_m4b = false;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--out" => out = Some(PathBuf::from(args.next().expect("--out <dir>"))),
            "--helper" => helper_path = Some(PathBuf::from(args.next().expect("--helper <path>"))),
            "--select" => select = Some(args.next().and_then(|n| n.parse::<usize>().ok()).expect("--select <track>")),
            "--region" => region = Some(args.next().and_then(|n| n.parse::<usize>().ok()).expect("--region <place>")),
            "--editors" => editors = true,
            "--play" => play = true,
            "--dialog" => dialog = true,
            "--restore" => restore = true,
            "--set" => {
                set_m4b = match args.next().as_deref() {
                    Some("m4b") => true,
                    Some("m2") => false,
                    _ => panic!("--set m2 (the fixed set) or m4b (the second slice's reads)"),
                }
            }
            other => panic!("unknown argument {other}; usage: record_fixtures --out <dir> [--helper <path>] [--select <track>] [--editors] [--play] [--dialog] [--restore]"),
        }
    }
    let out = out.expect("--out <dir>: where to write scene.json.gz");
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
    // What the scene needs shown is set up on a helper of its own, before the recording, and put back after it.
    let mut setup = launcher.launch().expect("the helper");
    let apps = logic::parse_apps(&setup.call("apps", json!({"bundles": BUNDLES})).expect("apps"));
    let app = logic::newest(&apps).expect("a running Logic Pro").clone();
    let mut stage = Project::new(Strings::english());
    {
        let mut ax = Ax::new(setup.as_mut(), app.pid);
        if let Some(place) = select {
            stage.select_track(&mut ax, place).expect("select the track");
            std::thread::sleep(Duration::from_millis(400));
            eprintln!("track {place} selected with a click");
        }
        if let Some(place) = region {
            stage.select_region(&mut ax, place).expect("select the region");
            eprintln!("region {place} selected");
        }
        for (label, on) in [("Editors", editors)] {
            if on {
                stage.find_layout(&mut ax).expect("the window");
                let bar = stage.control_bar(&mut ax).expect("the control bar");
                let button = paths::view_button(&bar, &stage.strings, label).expect("the view button");
                if !button.is_on() {
                    ax.action(&Root::Main, &button.path, "AXPress", 600).expect("open the pane");
                }
                eprintln!("{label} pane open");
            }
        }
        if play {
            stage.find_layout(&mut ax).expect("the window");
            let bar = stage.control_bar(&mut ax).expect("the control bar");
            let button = paths::transport_button(&bar, &stage.strings, "Play").expect("Play");
            ax.action(&Root::Main, &button.path, "AXPress", 800).expect("play");
            eprintln!("playing");
        }
        if dialog {
            ax.set(&Root::Title("Choose a Project".into()), "", "AXMinimized", json!(false), 800).expect("the project chooser");
            eprintln!("the project chooser is up");
        }
    }
    setup.quit();
    std::thread::sleep(Duration::from_millis(300));
    let scene = Arc::new(Mutex::new(Scene::new(&name)));
    let mut helper: Box<dyn Helper> = Box::new(Recorder::new(launcher.launch().expect("the helper"), scene.clone()));

    // As the bridge attaches: the apps, the grant, the windows.
    let apps = logic::parse_apps(&helper.call("apps", json!({"bundles": BUNDLES})).expect("apps"));
    let app = logic::newest(&apps).expect("a running Logic Pro").clone();
    let version = logic::app_version(&app);
    let trusted = helper.call("trusted", json!({"prompt": false})).expect("trusted")["trusted"] == true;
    assert!(trusted, "the helper isn't trusted under Accessibility");
    let mut model = Model::attach(&app, &version);
    let windows = helper.call("windows", json!({"pid": app.pid})).expect("windows");
    model.on_windows(&windows);
    let status = model.status();
    let project = model.project.clone();
    let (id, path, pname) =
        project.as_ref().map(|p| (p.id.clone(), p.path.clone(), p.name.clone().unwrap_or_default())).unwrap_or_default();
    eprintln!("recording {name} from Logic Pro {version} (pid {}), {}", app.pid, pname);
    let language = strings::language_of(&strings::preferred_languages(&app.bundle));
    let bundle = app.path.clone().map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/Applications/Logic Pro.app"));
    let strings = Strings::load(&bundle, &language);

    let requests = if set_m4b { scenes::m4b_requests() } else { scenes::fixed_requests() };
    // What the readers look at outside Logic (its preferences, Spotlight), kept for the replay as the change recorder keeps it.
    let looked = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    disk::set_hook(Some(disk::recording(looked.clone())));
    let cases = scenes::run_set(helper.as_mut(), app.pid, strings, status, (id.as_deref(), path.as_deref(), &pname), &requests);
    disk::set_hook(None);
    // The events that came meanwhile (none expected: the Mackie ports aren't opened here).
    while helper.event(Duration::from_millis(50)).is_some() {}
    helper.quit();
    {
        let mut scene = scene.lock().unwrap();
        let mut logic = json!({"version": version, "bundle": app.bundle, "pid": app.pid, "language": language, "project": project});
        let looked = looked.borrow().clone();
        if !looked.is_empty() {
            logic["disk"] = serde_json::Value::Array(looked);
        }
        scene.logic = logic.as_object().cloned().unwrap_or_default();
    }
    let scene = scene.lock().unwrap().clone();
    scene.save(&out).expect("write the scene");
    if restore {
        let mut after = launcher.launch().expect("the helper");
        let mut ax = Ax::new(after.as_mut(), app.pid);
        if dialog {
            let _ = ax.set(&Root::Title("Choose a Project".into()), "", "AXMinimized", json!(true), 600);
        }
        if play {
            if let Ok(bar) = stage.find_layout(&mut ax).and_then(|_| stage.control_bar(&mut ax)) {
                if let Some(button) = paths::transport_button(&bar, &stage.strings, "Stop") {
                    let _ = ax.action(&Root::Main, &button.path, "AXPress", 600);
                }
            }
        }
        if editors {
            if let Ok(bar) = stage.find_layout(&mut ax).and_then(|_| stage.control_bar(&mut ax)) {
                if let Some(button) = paths::view_button(&bar, &stage.strings, "Editors").filter(|b| b.is_on()) {
                    let _ = ax.action(&Root::Main, &button.path, "AXPress", 600);
                }
            }
        }
        after.quit();
        eprintln!("put back");
    }
    eprintln!("{} helper calls recorded to {}: {:?}", scene.calls.len(), out.join(SCENE_FILE).display(), scene.counts());
    for case in &cases {
        let ok = case["response"]["ok"] == true;
        eprintln!(
            "{} {} -> {}",
            case["request"]["method"],
            case["request"]["params"],
            if ok { "ok".to_string() } else { case["response"]["error"]["code"].to_string() }
        );
    }
}
