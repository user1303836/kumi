//! A small client for trying the extension in a real REAPER: finds the bridge through its discovery file, proves the
//! secret, prints the welcome and status, runs actions, prints the events that follow, and can quit REAPER.
//!
//! cargo run -p reaper-kumi --example lab_client -- <kumi home> [--action <id or name>]... [--watch <seconds>]
//!     [--call <method> <json params>]... [--digest <json params>]... [--quit [--save]]
//!
//! `--digest` reads a whole digest, page after page, and prints what it came to and how long it took (its pages aren't
//! printed: they're big).

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

fn main() {
    let mut args = std::env::args().skip(1);
    let home = PathBuf::from(
        args.next().expect("usage: lab_client <kumi home> [--action N]... [--call M P]... [--digest P]... [--watch S] [--quit [--save]]"),
    );
    let (mut actions, mut calls, mut digests, mut watch, mut quit, mut save) = (vec![], vec![], vec![], 0.0, false, false);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--action" => actions.push(args.next().expect("--action <id or name>")),
            "--call" => calls.push((args.next().expect("--call <method> <json>"), args.next().expect("--call <method> <json>"))),
            "--digest" => digests.push(args.next().expect("--digest <json params>")),
            "--watch" => watch = args.next().and_then(|s| s.parse().ok()).expect("--watch <seconds>"),
            "--quit" => quit = true,
            "--save" => save = true,
            other => panic!("unknown argument {other}"),
        }
    }
    let paths = BridgePaths::new(&home, Daw::Reaper);
    let found = files::scan_discovery(&paths.discovery(), Daw::Reaper);
    let discovery = found.iter().find(|f| f.alive).and_then(|f| f.discovery.as_ref().ok()).expect("no running REAPER bridge found");
    println!("discovery {}", serde_json::to_string(discovery).unwrap());
    let secret = files::read_secret(&paths.secret()).expect("the secret");

    let started = Instant::now();
    let mut stream = TcpStream::connect((discovery.host.as_str(), discovery.port)).expect("connect");
    let lines = reader(stream.try_clone().unwrap());
    let (mut handshake, hello) = ClientHandshake::start(secret, "lab_client");
    stream.write_all(&hello).unwrap();
    loop {
        let line = lines.recv_timeout(Duration::from_secs(5)).expect("a handshake line");
        match handshake.on_line(&line).expect("handshake") {
            ClientStep::Send(bytes) => stream.write_all(&bytes).unwrap(),
            ClientStep::Welcome(welcome) => {
                println!("welcome after {} ms {}", started.elapsed().as_millis(), serde_json::to_string(&welcome).unwrap());
                break;
            }
        }
    }

    let mut client = Client { stream, lines, next_id: 1 };
    client.call("status", json!({}));
    for action in actions {
        let command = action.parse::<i64>().map(Value::from).unwrap_or_else(|_| Value::from(action.clone()));
        client.call("action", json!({"command": command}));
        client.watch(Duration::from_millis(400));
    }
    for (method, params) in calls {
        client.call(&method, serde_json::from_str(&params).expect("the params as JSON"));
    }
    for params in digests {
        client.digest(serde_json::from_str(&params).expect("the digest's params as JSON"));
    }
    client.watch(Duration::from_secs_f64(watch));
    if quit {
        client.call("quit", json!({"save": save}));
        client.watch(Duration::from_secs(10));
    }
}

struct Client {
    stream: TcpStream,
    lines: Receiver<String>,
    next_id: u64,
}

impl Client {
    fn call(&mut self, method: &str, params: Value) -> Value {
        let (frame, line, took) = self.exchange(method, params);
        println!("{method} in {} ms {line}", took.as_millis());
        frame
    }

    /// The response to a request, its line and how long it took; what arrives meanwhile is printed.
    fn exchange(&mut self, method: &str, params: Value) -> (Value, String, Duration) {
        let id = self.next_id;
        self.next_id += 1;
        let sent = Instant::now();
        self.stream.write_all(&Frame::Request(Request { id, method: method.into(), params, deadline_ms: None }).encode()).unwrap();
        loop {
            let line = self.lines.recv_timeout(Duration::from_secs(15)).expect("a response");
            let frame: Value = serde_json::from_str(&line).unwrap();
            if frame["type"] == "response" && frame["id"] == id {
                return (frame, line, sent.elapsed());
            }
            print_frame(&frame, &line);
        }
    }

    /// Reads a whole digest, page after page, and prints what it came to: pages, records (and a fingerprint of them, to
    /// tell two digests apart), bytes on the wire, the project's change count, and the time, with each page's.
    fn digest(&mut self, params: Value) {
        let began = Instant::now();
        let (mut pages, mut records, mut bytes, mut times) = (0, 0, 0, vec![]);
        let mut print: u64 = 0xcbf2_9ce4_8422_2325;
        let mut cursor: Option<String> = None;
        let mut first: Option<Value> = None;
        loop {
            let mut asked = params.clone();
            if let Some(cursor) = &cursor {
                asked["cursor"] = cursor.clone().into();
            }
            let (frame, line, took) = self.exchange("digest", asked);
            if frame["ok"] != true {
                println!("digest {params} failed after {pages} pages: {line}");
                return;
            }
            let result = &frame["result"];
            pages += 1;
            for record in result["records"].as_array().into_iter().flatten() {
                records += 1;
                for byte in record.to_string().bytes() {
                    print = (print ^ byte as u64).wrapping_mul(0x0100_0000_01b3);
                }
            }
            bytes += line.len();
            times.push(took.as_millis());
            first.get_or_insert_with(|| json!({"project": result["project"], "changed": result["changed"], "bounded": result["bounded"]}));
            cursor = result["nextCursor"].as_str().map(str::to_owned);
            if cursor.is_none() || pages >= 1000 {
                break;
            }
        }
        println!(
            "digest {params}: {pages} pages, {records} records (print {print:016x}), {bytes} bytes in {} ms (pages {} ms) {}",
            began.elapsed().as_millis(),
            times.iter().map(|ms| ms.to_string()).collect::<Vec<_>>().join(" "),
            first.unwrap_or(Value::Null)
        );
    }

    // Prints what arrives until `time` passes or the bridge closes.
    fn watch(&mut self, time: Duration) {
        let until = Instant::now() + time;
        while let Some(left) = until.checked_duration_since(Instant::now()) {
            match self.lines.recv_timeout(left) {
                Ok(line) => print_frame(&serde_json::from_str(&line).unwrap(), &line),
                Err(mpsc::RecvTimeoutError::Timeout) => return,
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    println!("closed");
                    return;
                }
            }
        }
    }
}

fn print_frame(frame: &Value, line: &str) {
    if frame["type"] != "heartbeat" {
        println!("{line}");
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
