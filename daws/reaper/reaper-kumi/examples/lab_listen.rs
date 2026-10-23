//! A client for trying listening in a real REAPER (M5): runs steps from a JSON file against the bridge and prints each
//! answer with its time. A step is `{"call": method, "params": {…}}`, `{"poll": jobId, "everyMs": 100, "maxMs": 60000,
//! "status": true}` (asks `listen_status` until the job ends, and `status` beside it when asked), `{"sleepMs": n}`, or
//! `{"cancelAfterMs": n, "job": jobId}` (asks `listen_cancel` on a second connection after n ms, while the steps go on).
//! A string `"$epoch"` in params is the epoch the last answer named; `"$ref:<name>"` a track's ref by its name,
//! `"$fx:<name>"` the ref of that track's first FX, `"$prev:<pointer>"` a value of the previous answer (a JSON pointer; a
//! poll's answer is its last `listen_status`).
//!
//! cargo run -p reaper-kumi --example lab_listen -- <kumi home> <steps.json>

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::net::TcpStream;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::{Duration, Instant};

use daw_bridge::files;
use daw_bridge::handshake::{ClientHandshake, ClientStep};
use daw_bridge::paths::BridgePaths;
use daw_bridge::wire::{Frame, Request};
use daw_bridge::Daw;
use serde_json::{json, Value};

struct Client {
    stream: TcpStream,
    lines: Receiver<String>,
    next_id: u64,
    started: Instant,
}

fn connect(home: &std::path::Path, started: Instant) -> Client {
    let paths = BridgePaths::new(home, Daw::Reaper);
    let found = files::scan_discovery(&paths.discovery(), Daw::Reaper);
    let discovery = found.iter().find(|f| f.alive).and_then(|f| f.discovery.as_ref().ok()).expect("no running REAPER bridge found");
    let secret = files::read_secret(&paths.secret()).expect("the secret");
    let mut stream = TcpStream::connect((discovery.host.as_str(), discovery.port)).expect("connect");
    let lines = reader(stream.try_clone().unwrap());
    let (mut handshake, hello) = ClientHandshake::start(secret, "lab_listen");
    stream.write_all(&hello).unwrap();
    loop {
        let line = lines.recv_timeout(Duration::from_secs(5)).expect("a handshake line");
        match handshake.on_line(&line).expect("handshake") {
            ClientStep::Send(bytes) => stream.write_all(&bytes).unwrap(),
            ClientStep::Welcome(_) => break,
        }
    }
    Client { stream, lines, next_id: 1, started }
}

impl Client {
    fn call(&mut self, method: &str, params: Value) -> Value {
        let id = self.next_id;
        self.next_id += 1;
        let sent = Instant::now();
        self.stream.write_all(&Frame::Request(Request { id, method: method.into(), params, deadline_ms: None }).encode()).unwrap();
        loop {
            let line = self.lines.recv_timeout(Duration::from_secs(120)).expect("a response");
            let frame: Value = serde_json::from_str(&line).unwrap();
            if frame["type"] == "event" {
                println!("{:>7} event {line}", self.started.elapsed().as_millis());
            }
            if frame["type"] == "response" && frame["id"] == id {
                println!("{:>7} {method} in {} ms {line}", self.started.elapsed().as_millis(), sent.elapsed().as_millis());
                return frame;
            }
        }
    }
}

fn reader(stream: TcpStream) -> Receiver<String> {
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        for line in BufReader::new(stream).lines() {
            let Ok(line) = line else { break };
            if tx.send(line).is_err() {
                break;
            }
        }
    });
    rx
}

/// `$epoch`, `$ref:<name>`, `$fx:<name>` and `$prev:<pointer>` filled in.
fn fill(value: &Value, epoch: &str, refs: &HashMap<String, String>, prev: &Value) -> Value {
    match value {
        Value::String(text) if text.starts_with("$prev:") => prev.pointer(&text[6..]).cloned().unwrap_or(Value::Null),
        Value::String(text) if text == "$epoch" => epoch.into(),
        Value::String(text) if text.starts_with("$ref:") => refs.get(&text[5..]).cloned().map(Value::from).unwrap_or(Value::Null),
        Value::String(text) if text.starts_with("$fx:") => refs.get(&text[1..]).cloned().map(Value::from).unwrap_or(Value::Null),
        Value::Array(items) => Value::Array(items.iter().map(|v| fill(v, epoch, refs, prev)).collect()),
        Value::Object(map) => Value::Object(map.iter().map(|(k, v)| (k.clone(), fill(v, epoch, refs, prev))).collect()),
        other => other.clone(),
    }
}

fn main() {
    let mut args = std::env::args().skip(1);
    let home = PathBuf::from(args.next().expect("usage: lab_listen <kumi home> <steps.json>"));
    let steps: Vec<Value> = serde_json::from_str(&std::fs::read_to_string(args.next().expect("steps.json")).unwrap()).expect("steps");
    let started = Instant::now();
    let mut client = connect(&home, started);
    let mut epoch = String::new();
    let mut refs: HashMap<String, String> = HashMap::new();
    let tracks = client.call("read", json!({"kind": "track", "limit": 200}));
    for row in tracks["result"]["items"].as_array().into_iter().flatten() {
        let (name, reference) = (row["name"].as_str().unwrap_or_default().to_string(), row["ref"].as_str().unwrap_or_default().to_string());
        if row["fxCount"].as_i64().unwrap_or(0) > 0 {
            let fx = client.call("read", json!({"kind": "fx", "parent": reference}));
            if let Some(first) = fx["result"]["items"][0]["ref"].as_str() {
                refs.insert(format!("fx:{name}"), first.to_string());
            }
        }
        refs.insert(name, reference);
    }
    if let Some(e) = tracks["result"]["epoch"].as_str() {
        epoch = e.to_string();
    }
    let mut cancels = Vec::new();
    let mut prev = Value::Null;
    for step in steps {
        if let Some(method) = step["call"].as_str() {
            let answer = client.call(method, fill(&step["params"], &epoch, &refs, &prev));
            if let Some(e) = answer["result"]["epoch"].as_str() {
                epoch = e.to_string();
            }
            prev = answer;
        } else if let Some(job) = step["poll"].as_str() {
            let every = Duration::from_millis(step["everyMs"].as_u64().unwrap_or(100));
            let until = Instant::now() + Duration::from_millis(step["maxMs"].as_u64().unwrap_or(60_000));
            loop {
                let answer = client.call("listen_status", json!({"jobId": job}));
                if step["status"] == true {
                    client.call("status", json!({}));
                }
                let state = answer["result"]["state"].as_str().unwrap_or("?").to_string();
                prev = answer;
                if matches!(state.as_str(), "done" | "failed" | "cancelled" | "?") || Instant::now() > until {
                    break;
                }
                thread::sleep(every);
            }
        } else if let Some(ms) = step["sleepMs"].as_u64() {
            thread::sleep(Duration::from_millis(ms));
        } else if let Some(ms) = step["cancelAfterMs"].as_u64() {
            let (home, job) = (home.clone(), step["job"].as_str().unwrap_or_default().to_string());
            cancels.push(thread::spawn(move || {
                thread::sleep(Duration::from_millis(ms));
                let mut second = connect(&home, started);
                second.call("listen_cancel", json!({"jobId": job}));
            }));
        }
    }
    for cancel in cancels {
        let _ = cancel.join();
    }
    // Events that came while nothing was asked.
    while let Ok(line) = client.lines.try_recv() {
        if line.contains("\"type\":\"event\"") {
            println!("{:>7} event {line}", started.elapsed().as_millis());
        }
    }
}
