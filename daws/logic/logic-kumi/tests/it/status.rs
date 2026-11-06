//! The bridge on the mock helper, without a Logic: what `status`, the welcome and the discovery file say of a saved,
//! an unsaved and a loading project, of a dialog, of Logic's windows away and of a missing permission; the discovery
//! file through attach, a project switch and the stop; a helper that exits mid-run; and Logic quitting. The reads
//! over a recorded Logic are `tests/reads.rs`.

use std::collections::VecDeque;
use std::io::{BufRead, BufReader, Write};
use std::net::TcpStream;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use daw_bridge::auth::Secret;
use daw_bridge::files::{self, Discovery, DISCOVERY_SCHEMA};
use daw_bridge::handshake::{ClientHandshake, ClientStep};
use daw_bridge::paths::BridgePaths;
use daw_bridge::wire::{Frame, Project, Request, PROTOCOL};
use daw_bridge::{loopback, Daw};
use logic_kumi::bridge::{self, Config, Outcome};
use logic_kumi::helper::{MockHands, Supervised};
use logic_kumi::{logic, BRIDGE_VERSION};
use serde_json::{json, Value};

const WAIT: Duration = Duration::from_secs(5);
const SONG: &str = "/Users/kumi/Music/Logic/Song.logicx";
const OTHER: &str = "/Users/kumi/Music/Logic/Other.logicx";

/// The test's own pid stands for Logic's: the discovery file's pid must be a running process to be found alive.
fn me() -> u32 {
    std::process::id()
}

fn logic_app(pid: u32) -> Value {
    json!({"bundle": "com.apple.logic10", "pid": pid, "name": "Logic Pro", "active": true, "hidden": false, "version": "12.3.1"})
}

fn tracks_window(title: &str, document: Option<&str>) -> Value {
    json!({"title": title, "role": "AXWindow", "subrole": "AXStandardWindow", "main": true, "focused": true, "document": document, "minimized": false, "fullscreen": false, "modal": false})
}

fn dialog(title: &str) -> Value {
    json!({"title": title, "role": "AXWindow", "subrole": "AXDialog", "main": true, "focused": true, "minimized": false, "fullscreen": false, "modal": false})
}

fn hands_showing(windows: Value) -> MockHands {
    let hands = MockHands::new();
    hands.set_apps(vec![logic_app(me())]);
    hands.set_windows(windows, &["Song - Tracks"]);
    hands
}

fn wait_for(what: &str, mut done: impl FnMut() -> bool) {
    let until = Instant::now() + WAIT;
    while !done() {
        assert!(Instant::now() < until, "waited {WAIT:?} for {what}");
        thread::sleep(Duration::from_millis(10));
    }
}

struct Lab {
    stop: Arc<AtomicBool>,
    run: Option<JoinHandle<Outcome>>,
    discovery_dir: PathBuf,
    secret: Secret,
    _dir: tempfile::TempDir,
}

impl Lab {
    /// The bridge on `hands`, with short intervals, in a fresh Kumi folder.
    fn start(hands: &MockHands) -> Lab {
        Self::start_with_reference(hands, false)
    }

    fn start_with_reference(hands: &MockHands, reference: bool) -> Lab {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("kumi");
        let mut config = Config::development(&home).unwrap();
        if reference {
            config = Config::from_reference(&files::Reference::new(Daw::Logic, &BridgePaths::new(&home, Daw::Logic)));
        }
        config = config.with_timing(Duration::from_millis(40), Duration::from_millis(120));
        config.bundle = Some(dir.path().join("No Logic.app"));
        let secret = files::read_secret(&config.server.secret_file).unwrap();
        let discovery_dir = config.server.discovery_dir.clone();
        let helper = Supervised::new(Box::new(hands.clone())).with_backoff(Duration::from_millis(40), Duration::from_millis(100));
        let stop = Arc::new(AtomicBool::new(false));
        let flag = stop.clone();
        let run = thread::spawn(move || bridge::run(config, Box::new(helper), flag));
        Lab { stop, run: Some(run), discovery_dir, secret, _dir: dir }
    }

    fn discovery(&self) -> Option<Discovery> {
        files::scan_discovery(&self.discovery_dir, Daw::Logic).into_iter().find_map(|found| found.discovery.ok())
    }

    fn wait_discovery(&self) -> Discovery {
        let mut found = None;
        wait_for("the discovery file", || {
            found = self.discovery();
            found.is_some()
        });
        found.unwrap()
    }

    fn client(&self) -> Client {
        let discovery = self.wait_discovery();
        Client::connect(discovery.port, &self.secret)
    }

    /// Asks the bridge to stop, and how the run ended.
    fn finish(mut self) -> Outcome {
        self.stop.store(true, Ordering::Release);
        self.run.take().unwrap().join().unwrap()
    }

    /// How the run ended by itself.
    fn outcome(mut self) -> Outcome {
        wait_for("the run to end", || self.run.as_ref().unwrap().is_finished());
        self.run.take().unwrap().join().unwrap()
    }
}

impl Drop for Lab {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(run) = self.run.take() {
            let _ = run.join();
        }
    }
}

fn read_line(reader: &mut BufReader<TcpStream>) -> Option<String> {
    let mut line = String::new();
    match reader.read_line(&mut line) {
        Ok(0) | Err(_) => None,
        Ok(_) => Some(line.trim_end().to_string()),
    }
}

/// A host: authenticated on connect, with the events that arrive between answers kept in order.
struct Client {
    stream: TcpStream,
    reader: BufReader<TcpStream>,
    welcome: Value,
    next_id: u64,
    events: VecDeque<Value>,
}

impl Client {
    fn connect(port: u16, secret: &Secret) -> Client {
        let mut stream = TcpStream::connect((loopback::BIND_ADDRESS, port)).unwrap();
        stream.set_read_timeout(Some(WAIT)).unwrap();
        let mut reader = BufReader::new(stream.try_clone().unwrap());
        let (mut handshake, hello) = ClientHandshake::start(secret.clone(), "status-test");
        stream.write_all(&hello).unwrap();
        loop {
            match handshake.on_line(&read_line(&mut reader).expect("a handshake line")).unwrap() {
                ClientStep::Send(bytes) => stream.write_all(&bytes).unwrap(),
                ClientStep::Welcome(welcome) => {
                    return Client {
                        stream,
                        reader,
                        welcome: serde_json::to_value(*welcome).unwrap(),
                        next_id: 1,
                        events: VecDeque::new(),
                    };
                }
            }
        }
    }

    /// The next frame that isn't a heartbeat; None once the bridge closed.
    fn next_frame(&mut self) -> Option<Value> {
        loop {
            let frame: Value = serde_json::from_str(&read_line(&mut self.reader)?).unwrap();
            if frame["type"] != "heartbeat" {
                return Some(frame);
            }
        }
    }

    fn call(&mut self, method: &str) -> Value {
        let id = self.next_id;
        self.next_id += 1;
        self.stream
            .write_all(&Frame::Request(Request { id, method: method.into(), params: json!({}), deadline_ms: None }).encode())
            .unwrap();
        loop {
            let frame = self.next_frame().expect("a response");
            if frame["type"] == "response" {
                assert_eq!(frame["id"], id);
                return frame;
            }
            self.events.push_back(frame);
        }
    }

    fn status(&mut self) -> Value {
        let response = self.call("status");
        assert_eq!(response["ok"], true, "{response}");
        response["result"].clone()
    }

    /// The status once `done` holds of it.
    fn status_when(&mut self, what: &str, done: impl Fn(&Value) -> bool) -> Value {
        let mut status = self.status();
        wait_for(what, || {
            status = self.status();
            done(&status)
        });
        status
    }

    fn next_event(&mut self) -> Value {
        if let Some(event) = self.events.pop_front() {
            return event;
        }
        loop {
            let frame = self.next_frame().expect("an event");
            if frame["type"] == "event" {
                return frame;
            }
        }
    }

    /// The bye, past any events before it, then the close.
    fn bye(&mut self) -> Value {
        loop {
            let frame = self.next_frame().expect("a bye");
            if frame["type"] == "bye" {
                assert_eq!(read_line(&mut self.reader), None, "closed after the bye");
                return frame;
            }
        }
    }
}

#[test]
fn observing_a_project_never_opens_midi_or_answers_control_surface_queries_without_opt_in() {
    for reference in [false, true] {
        let hands = hands_showing(json!([tracks_window("Song - Tracks", Some(&format!("file://{SONG}")))]));
        // MockHands returns this tree at every path. Its selected item makes a completed focus refresh observable;
        // its empty Tracks area supplies the two groups the reader locates before that refresh.
        hands.set_tree(json!({"role": "AXWindow", "c": [
            {"i": "0", "role": "AXGroup", "desc": "Control Bar"},
            {"i": "1", "role": "AXGroup", "desc": "Tracks", "c": [
                {"i": "1.0", "role": "AXSplitGroup", "c": [
                    {"role": "AXScrollArea", "c": [{"i": "1.0.0.0", "role": "AXGroup"}]},
                    {"role": "AXScrollArea", "c": [{"i": "1.0.1.0", "role": "AXGroup"}]}
                ]}
            ]},
            {"role": "AXLayoutItem", "desc": "Track 1 “Fixture”", "selected": true}
        ]}));
        let lab = Lab::start_with_reference(&hands, reference);
        let mut client = lab.client();
        wait_for("a completed focus refresh", || client.call("focus")["result"]["selection"]["trackRef"] == "1:track:1");
        let before = hands.calls_of("apps");
        hands.emit(json!({"event": "midi", "hex": "F0 00 00 66 14 00 F7"}));
        // The app listing happens after the run loop drains helper events, so this proves the unsolicited query
        // was processed without relying on how long a native tick takes.
        wait_for("the next app listing after the MIDI query", || hands.calls_of("apps") > before);
        assert_eq!(lab.finish(), Outcome::Stopped);
        let midi: Vec<_> = hands.calls().into_iter().filter(|(op, _)| op.starts_with("midi.")).collect();
        assert!(midi.is_empty(), "ordinary observation called the MIDI helper: {midi:?}");
    }
}

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
    assert_eq!(client.welcome["methods"], json!(logic_kumi::server::METHODS));
    assert_eq!(client.welcome["events"], json!(logic_kumi::server::EVENTS));
    assert_eq!(client.welcome["project"], serde_json::to_value(&project).unwrap());
    assert_eq!(client.call("ping")["result"], json!({}));
    let status = client.status();
    assert_eq!(status, json!({"project": project, "busy": false, "permission": null}));
    assert_eq!(status.as_object().unwrap().keys().collect::<Vec<_>>(), ["project", "busy", "permission"]);
    // A read goes to the run loop, which reads the request before Logic: no kind is refused as the simulator refuses it.
    let read = client.call("read");
    assert_eq!((read["error"]["code"].as_str(), read["error"]["message"].as_str()), (Some("bad-params"), Some("kind: one of the kinds")));
    assert_eq!(client.call("bogus")["error"]["code"], "unknown-method");

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
    wait_for("two app listings while Logic is absent", || hands.calls_of("apps") >= 2);
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
