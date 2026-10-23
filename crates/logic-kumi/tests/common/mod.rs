//! The bridge on the mock helper, for the tests that run it whole (`status.rs`, `changes.rs`): a lab that starts a
//! run in a fresh Kumi folder with short intervals, and a client that speaks the wire to it.
#![allow(dead_code)]

use std::collections::VecDeque;
use std::io::{BufRead, BufReader, Write};
use std::net::TcpStream;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use daw_bridge::auth::Secret;
use daw_bridge::files::{self, Discovery};
use daw_bridge::handshake::{ClientHandshake, ClientStep};
use daw_bridge::wire::{Frame, Request};
use daw_bridge::{loopback, Daw};
use logic_kumi::bridge::{self, Config, Outcome};
use logic_kumi::helper::{MockHands, Supervised};
use serde_json::{json, Value};

pub const WAIT: Duration = Duration::from_secs(5);
pub const SONG: &str = "/Users/kumi/Music/Logic/Song.logicx";
pub const OTHER: &str = "/Users/kumi/Music/Logic/Other.logicx";

/// The test's own pid stands for Logic's: the discovery file's pid must be a running process to be found alive.
pub fn me() -> u32 {
    std::process::id()
}

pub fn logic_app(pid: u32) -> Value {
    json!({"bundle": "com.apple.logic10", "pid": pid, "name": "Logic Pro", "active": true, "hidden": false, "version": "12.3.1"})
}

pub fn tracks_window(title: &str, document: Option<&str>) -> Value {
    json!({"title": title, "role": "AXWindow", "subrole": "AXStandardWindow", "main": true, "focused": true, "document": document, "minimized": false, "fullscreen": false, "modal": false})
}

pub fn dialog(title: &str) -> Value {
    json!({"title": title, "role": "AXWindow", "subrole": "AXDialog", "main": true, "focused": true, "minimized": false, "fullscreen": false, "modal": false})
}

pub fn hands_showing(windows: Value) -> MockHands {
    let hands = MockHands::new();
    hands.set_apps(vec![logic_app(me())]);
    hands.set_windows(windows, &["Song - Tracks"]);
    hands
}

pub fn wait_for(what: &str, mut done: impl FnMut() -> bool) {
    let until = Instant::now() + WAIT;
    while !done() {
        assert!(Instant::now() < until, "waited {WAIT:?} for {what}");
        thread::sleep(Duration::from_millis(10));
    }
}

pub struct Lab {
    pub stop: Arc<AtomicBool>,
    pub run: Option<JoinHandle<Outcome>>,
    pub discovery_dir: PathBuf,
    pub secret: Secret,
    _dir: tempfile::TempDir,
}

impl Lab {
    /// The bridge on `hands`, with short intervals, in a fresh Kumi folder.
    pub fn start(hands: &MockHands) -> Lab {
        let dir = tempfile::tempdir().unwrap();
        let config = Config::development(&dir.path().join("kumi"))
            .unwrap()
            .with_timing(Duration::from_millis(40), Duration::from_millis(120))
            .without_mcu();
        let secret = files::read_secret(&config.server.secret_file).unwrap();
        let discovery_dir = config.server.discovery_dir.clone();
        let helper = Supervised::new(Box::new(hands.clone())).with_backoff(Duration::from_millis(40), Duration::from_millis(100));
        let stop = Arc::new(AtomicBool::new(false));
        let flag = stop.clone();
        let run = thread::spawn(move || bridge::run(config, Box::new(helper), flag));
        Lab { stop, run: Some(run), discovery_dir, secret, _dir: dir }
    }

    pub fn discovery(&self) -> Option<Discovery> {
        files::scan_discovery(&self.discovery_dir, Daw::Logic).into_iter().find_map(|found| found.discovery.ok())
    }

    pub fn wait_discovery(&self) -> Discovery {
        let mut found = None;
        wait_for("the discovery file", || {
            found = self.discovery();
            found.is_some()
        });
        found.unwrap()
    }

    pub fn client(&self) -> Client {
        let discovery = self.wait_discovery();
        Client::connect(discovery.port, &self.secret)
    }

    /// Asks the bridge to stop, and how the run ended.
    pub fn finish(mut self) -> Outcome {
        self.stop.store(true, Ordering::Release);
        self.run.take().unwrap().join().unwrap()
    }

    /// How the run ended by itself.
    pub fn outcome(mut self) -> Outcome {
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

pub fn read_line(reader: &mut BufReader<TcpStream>) -> Option<String> {
    let mut line = String::new();
    match reader.read_line(&mut line) {
        Ok(0) | Err(_) => None,
        Ok(_) => Some(line.trim_end().to_string()),
    }
}

/// A host: authenticated on connect, with the events that arrive between answers kept in order.
pub struct Client {
    pub stream: TcpStream,
    pub reader: BufReader<TcpStream>,
    pub welcome: Value,
    pub next_id: u64,
    pub events: VecDeque<Value>,
}

impl Client {
    pub fn connect(port: u16, secret: &Secret) -> Client {
        let mut stream = TcpStream::connect((loopback::BIND_ADDRESS, port)).unwrap();
        stream.set_read_timeout(Some(WAIT)).unwrap();
        let mut reader = BufReader::new(stream.try_clone().unwrap());
        let (mut handshake, hello) = ClientHandshake::start(secret.clone(), "status-test");
        stream.write_all(&hello).unwrap();
        loop {
            match handshake.on_line(&read_line(&mut reader).expect("a handshake line")).unwrap() {
                ClientStep::Send(bytes) => stream.write_all(&bytes).unwrap(),
                ClientStep::Welcome(welcome) => {
                    return Client { stream, reader, welcome: serde_json::to_value(*welcome).unwrap(), next_id: 1, events: VecDeque::new() }
                }
            }
        }
    }

    /// The next frame that isn't a heartbeat; None once the bridge closed.
    pub fn next_frame(&mut self) -> Option<Value> {
        loop {
            let frame: Value = serde_json::from_str(&read_line(&mut self.reader)?).unwrap();
            if frame["type"] != "heartbeat" {
                return Some(frame);
            }
        }
    }

    pub fn call(&mut self, method: &str) -> Value {
        self.call_with(method, json!({}))
    }

    pub fn call_with(&mut self, method: &str, params: Value) -> Value {
        let id = self.next_id;
        self.next_id += 1;
        self.stream.write_all(&Frame::Request(Request { id, method: method.into(), params, deadline_ms: None }).encode()).unwrap();
        loop {
            let frame = self.next_frame().expect("a response");
            if frame["type"] == "response" {
                assert_eq!(frame["id"], id);
                return frame;
            }
            self.events.push_back(frame);
        }
    }

    pub fn status(&mut self) -> Value {
        let response = self.call("status");
        assert_eq!(response["ok"], true, "{response}");
        response["result"].clone()
    }

    /// The status once `done` holds of it.
    pub fn status_when(&mut self, what: &str, done: impl Fn(&Value) -> bool) -> Value {
        let mut status = self.status();
        wait_for(what, || {
            status = self.status();
            done(&status)
        });
        status
    }

    pub fn next_event(&mut self) -> Value {
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
    pub fn bye(&mut self) -> Value {
        loop {
            let frame = self.next_frame().expect("a bye");
            if frame["type"] == "bye" {
                assert_eq!(read_line(&mut self.reader), None, "closed after the bye");
                return frame;
            }
        }
    }
}
