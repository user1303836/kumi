//! The bridge on the mock helper, without a Logic: what `status`, the welcome and the discovery file say of a saved,
//! an unsaved and a loading project, of a dialog, of Logic's windows away and of a missing permission; the discovery
//! file through attach, a project switch and the stop; a helper that exits mid-run; and Logic quitting. The reads
//! over a recorded Logic are `tests/reads.rs`.

mod common;

use std::thread;
use std::time::Duration;

use common::{dialog, hands_showing, logic_app, me, tracks_window, wait_for, Lab, OTHER, SONG};
use daw_bridge::files::{self, DISCOVERY_SCHEMA};
use daw_bridge::wire::{Project, PROTOCOL};
use daw_bridge::Daw;
use logic_kumi::bridge::Outcome;
use logic_kumi::helper::MockHands;
use logic_kumi::{logic, BRIDGE_VERSION};
use serde_json::{json, Value};

#[test]
fn a_saved_project_is_identified_by_its_path_in_status_the_welcome_and_the_discovery_file() {
    let hands = hands_showing(json!([tracks_window("Song - Tracks", Some(&format!("file://{SONG}")))]));
    let lab = Lab::start(&hands);
    let id = logic::project_id(SONG);
    let project = Project { id: Some(id.clone()), name: Some("Song".into()), path: Some(SONG.into()), ..Project::default() };

    let discovery = lab.wait_discovery();
    assert_eq!(discovery.schema, DISCOVERY_SCHEMA);
    assert_eq!((discovery.daw.as_str(), discovery.pid, discovery.resource.as_str()), ("logic", me(), "com.apple.logic10"));
    assert_eq!((discovery.host.as_str(), discovery.protocol.as_str()), ("127.0.0.1", PROTOCOL));
    assert!(Daw::Logic.ports().contains(&discovery.port), "port {}", discovery.port);
    assert_eq!((discovery.version.as_str(), discovery.daw_version.as_str()), (BRIDGE_VERSION, "12.3.1"));
    assert_eq!(discovery.project, Some(project.clone()));
    assert!(discovery.started_ms > 0);
    assert_eq!(discovery.file_name(), files::discovery_file_name("logic", me(), "com.apple.logic10"));

    let mut client = lab.client();
    assert_eq!(client.welcome["daw"], "logic");
    assert_eq!(client.welcome["pid"], me());
    assert_eq!(client.welcome["resource"], "com.apple.logic10");
    assert_eq!((client.welcome["bridge"].as_str(), client.welcome["dawVersion"].as_str()), (Some(BRIDGE_VERSION), Some("12.3.1")));
    assert_eq!(
        client.welcome["methods"],
        json!([
            "ping",
            "status",
            "focus",
            "read",
            "snapshot",
            "focus_body",
            "change_preview",
            "change_apply",
            "change_restore",
            "transport_stop",
            "transport_play",
            "cursor_set",
            "undo_step_begin",
            "undo_step_end",
            "project_save",
            "bounce",
            "export_tracks",
            "hear",
            "audition",
            "render"
        ])
    );
    assert_eq!(client.welcome["events"], json!(["project", "transport", "tracks", "selection", "undone_in_daw"]));
    assert_eq!(client.welcome["project"], serde_json::to_value(&project).unwrap());
    assert_eq!(client.call("ping")["result"], json!({}));
    let status = client.status();
    assert_eq!(status, json!({"project": project, "busy": false, "permission": null}));
    assert_eq!(status.as_object().unwrap().keys().collect::<Vec<_>>(), ["project", "busy", "permission"]);
    // A read goes to the run loop, which reads the request before Logic: no kind is refused as the simulator refuses it.
    let read = client.call("read");
    assert_eq!((read["error"]["code"].as_str(), read["error"]["message"].as_str()), (Some("bad-params"), Some("kind: one of the kinds")));
    assert_eq!(client.call("bogus")["error"]["code"], "unknown-method");
    // A change without its epoch is refused by the change side, on the run loop, before anything is read.
    assert_eq!(client.call("change_preview")["error"]["message"], "epoch: the epoch the refs were read in");

    // The helper was asked as the contract says: the apps by both bundle ids, then the permission and the windows.
    let calls = hands.calls();
    assert_eq!(calls[0], ("apps".into(), json!({"bundles": ["com.apple.logic10", "com.apple.mobilelogic"]})));
    assert_eq!(calls[1], ("trusted".into(), json!({"prompt": true})));
    assert_eq!(calls[2], ("windows".into(), json!({"pid": me()})));
    assert!(calls.iter().skip(2).filter(|(op, _)| op == "trusted").all(|(_, params)| params == &json!({"prompt": false})));
}

#[test]
fn an_unsaved_project_has_an_identity_and_no_id() {
    let hands = hands_showing(json!([tracks_window("Untitled - Tracks", None)]));
    let lab = Lab::start(&hands);
    let discovery = lab.wait_discovery();
    let project = discovery.project.unwrap();
    assert_eq!((project.id, project.path), (None, None));
    assert_eq!(project.name.as_deref(), Some("Untitled"));
    assert_eq!(project.extra.get("identity"), Some(&json!("Untitled")));
    let mut client = lab.client();
    let status = client.status();
    assert_eq!(status["project"], json!({"name": "Untitled", "identity": "Untitled"}));
    assert_eq!(status["project"].get("id"), None);
}

#[test]
fn no_window_is_away_a_dialog_is_modal_and_no_title_is_loading() {
    let song = tracks_window("Song - Tracks", Some(&format!("file://{SONG}")));
    let hands = hands_showing(json!([song]));
    let lab = Lab::start(&hands);
    let mut client = lab.client();
    assert_eq!(client.status()["busy"], false);
    let project = client.status()["project"].clone();

    // Logic's Space isn't the current one: no window at all, though the Window menu still lists it.
    hands.set_windows(json!([]), &["Song - Tracks"]);
    let status = client.status_when("away", |status| status["busy"] == "away");
    assert_eq!(status["project"], project, "the project stays while away");

    // An Import panel as the main window, the project window behind it.
    let behind = json!({"title": "Song - Tracks", "role": "AXWindow", "subrole": "AXStandardWindow", "main": false, "focused": false, "document": format!("file://{SONG}")});
    hands.set_windows(json!([dialog("Import"), behind]), &["Song - Tracks"]);
    let status = client.status_when("modal", |status| status["busy"] == "modal");
    assert_eq!(status["project"], project);
    // Free again, then a sheet on the project window: modal, and still the project.
    let song = tracks_window("Song - Tracks", Some(&format!("file://{SONG}")));
    hands.set_windows(json!([song]), &["Song - Tracks"]);
    client.status_when("free", |status| status["busy"] == false);
    let mut sheet = tracks_window("Song - Tracks", Some(&format!("file://{SONG}")));
    sheet["modal"] = true.into();
    hands.set_windows(json!([sheet]), &["Song - Tracks"]);
    let status = client.status_when("a sheet", |status| status["busy"] == "modal");
    assert_eq!(status["project"], project);

    // A project loading: the main window has no title yet.
    hands.set_windows(json!([tracks_window("", None)]), &[""]);
    let status = client.status_when("loading", |status| status["busy"] == "loading");
    assert_eq!(status["project"], project);

    // Back to the project: free.
    hands.set_windows(json!([tracks_window("Song - Tracks", Some(&format!("file://{SONG}")))]), &["Song - Tracks"]);
    client.status_when("free", |status| status["busy"] == false);
    assert!(client.events.is_empty(), "the same project again isn't a project event: {:?}", client.events);
}

#[test]
fn without_accessibility_status_names_the_permission_and_is_never_busy() {
    let hands = MockHands::new();
    hands.set_apps(vec![logic_app(me())]);
    hands.set_trusted(false);
    let lab = Lab::start(&hands);
    let mut client = lab.client();
    let status = client.status();
    assert_eq!((&status["permission"], &status["busy"]), (&json!("accessibility"), &json!(false)));
    assert_eq!(status["project"], Value::Null);
    assert_eq!(client.welcome["pid"], me());
    assert_eq!(hands.calls_of("windows"), 0, "nothing is read without the permission");
    // Granted: the windows are read from then on.
    hands.set_windows(json!([tracks_window("Song - Tracks", Some(&format!("file://{SONG}")))]), &["Song - Tracks"]);
    hands.set_trusted(true);
    let status = client.status_when("the permission", |status| status["permission"].is_null());
    assert_eq!(status["project"]["id"], logic::project_id(SONG));
    assert_eq!(client.next_event()["name"], "project");
}

#[test]
fn the_discovery_file_follows_the_project_and_goes_with_the_bridge() {
    let hands = hands_showing(json!([tracks_window("Song - Tracks", Some(&format!("file://{SONG}")))]));
    let lab = Lab::start(&hands);
    let mut client = lab.client();
    assert_eq!(lab.discovery().unwrap().project.unwrap().id.as_deref(), Some(logic::project_id(SONG).as_str()));

    hands.set_windows(json!([tracks_window("Other - Tracks", Some(&format!("file://{OTHER}")))]), &["Other - Tracks"]);
    let event = client.next_event();
    assert_eq!((event["seq"].as_u64(), event["name"].as_str()), (Some(1), Some("project")));
    assert_eq!(event["data"], json!({"id": logic::project_id(OTHER), "name": "Other", "path": OTHER}));
    // The discovery file was rewritten before the event went out.
    assert_eq!(lab.discovery().unwrap().project.unwrap().id.as_deref(), Some(logic::project_id(OTHER).as_str()));
    assert_eq!(client.status()["project"]["path"], OTHER);

    // A project closed: no project, said once.
    hands.set_windows(json!([dialog("Choose a Project")]), &[]);
    let event = client.next_event();
    assert_eq!((event["seq"].as_u64(), &event["data"]), (Some(2), &Value::Null));
    assert_eq!(lab.discovery().unwrap().project, None);

    let discovery_dir = lab.discovery_dir.clone();
    assert_eq!(lab.finish(), Outcome::Stopped);
    assert_eq!(client.bye(), json!({"type": "bye", "reason": "shutdown"}));
    assert!(files::scan_discovery(&discovery_dir, Daw::Logic).is_empty(), "the discovery file is removed on stop");
}

#[test]
fn a_helper_that_exits_is_started_again_and_the_bridge_keeps_answering() {
    let hands = hands_showing(json!([tracks_window("Song - Tracks", Some(&format!("file://{SONG}")))]));
    let lab = Lab::start(&hands);
    let mut client = lab.client();
    assert_eq!(hands.launches(), 1);
    hands.exit();
    wait_for("the helper to be started again", || hands.launches() == 2);
    let status = client.status();
    assert_eq!(status["project"]["id"], logic::project_id(SONG), "the last look is kept meanwhile");
    assert_eq!(lab.discovery().unwrap().project.unwrap().id.as_deref(), Some(logic::project_id(SONG).as_str()));
    // The new helper is the one asked: a project switch is followed.
    hands.set_windows(json!([tracks_window("Other - Tracks", Some(&format!("file://{OTHER}")))]), &["Other - Tracks"]);
    assert_eq!(client.next_event()["data"]["name"], "Other");
    assert_eq!(hands.launches(), 2);
}

#[test]
fn logic_quitting_ends_the_bridge_with_a_bye() {
    let hands = hands_showing(json!([tracks_window("Song - Tracks", Some(&format!("file://{SONG}")))]));
    let lab = Lab::start(&hands);
    let mut client = lab.client();
    // Another app's end, and a launch, change nothing.
    hands.emit(json!({"event": "app", "state": "terminated", "pid": me() + 1, "bundle": "com.apple.logic10"}));
    hands.emit(json!({"event": "app", "state": "launched", "pid": me() + 2, "bundle": "com.apple.mobilelogic"}));
    thread::sleep(Duration::from_millis(100));
    assert_eq!(client.status()["project"]["name"], "Song");
    hands.emit(json!({"event": "app", "state": "terminated", "pid": me(), "bundle": "com.apple.logic10"}));
    assert_eq!(client.bye(), json!({"type": "bye", "reason": "quit"}));
    let discovery_dir = lab.discovery_dir.clone();
    assert_eq!(lab.outcome(), Outcome::LogicQuit);
    assert!(files::scan_discovery(&discovery_dir, Daw::Logic).is_empty());
    assert_eq!(hands.calls().last().map(|(op, _)| op.as_str()), Some("quit"), "the helper is told to quit");
}

#[test]
fn logic_missing_from_the_running_apps_ends_the_bridge_too() {
    let hands = hands_showing(json!([tracks_window("Song - Tracks", Some(&format!("file://{SONG}")))]));
    let lab = Lab::start(&hands);
    let mut client = lab.client();
    hands.set_apps(vec![]);
    assert_eq!(client.bye(), json!({"type": "bye", "reason": "quit"}));
    assert_eq!(lab.outcome(), Outcome::LogicQuit);
}

#[test]
fn the_bridge_waits_for_logic_and_attaches_when_it_launches() {
    let hands = MockHands::new();
    hands.set_windows(json!([tracks_window("Song - Tracks", Some(&format!("file://{SONG}")))]), &["Song - Tracks"]);
    let lab = Lab::start(&hands);
    thread::sleep(Duration::from_millis(400));
    assert_eq!(lab.discovery(), None, "nothing to serve for yet");
    assert!(hands.calls_of("apps") >= 2, "asked again while waiting");
    assert_eq!(hands.calls_of("windows"), 0);
    // The newest Logic is the one attached to.
    hands.set_apps(vec![
        logic_app(me()),
        json!({"bundle": "com.apple.mobilelogic", "pid": 1, "name": "Logic Pro", "active": false, "hidden": false}),
    ]);
    hands.emit(json!({"event": "app", "state": "launched", "pid": me(), "bundle": "com.apple.logic10"}));
    let discovery = lab.wait_discovery();
    assert_eq!((discovery.pid, discovery.resource.as_str()), (me(), "com.apple.logic10"));
    let mut client = lab.client();
    assert_eq!(client.status()["project"]["name"], "Song");
}
