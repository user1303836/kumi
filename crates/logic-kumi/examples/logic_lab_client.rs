//! A small client for trying the bridge against a real Logic Pro: finds the bridge through its discovery file, proves
//! the secret, prints the welcome and `status`, then the events for a while. The M1 gate on a Mac: start `logic-kumi`
//! with Logic open, run this, and open or switch a project in Logic to see the `project` event.
//!
//! cargo run -p logic-kumi --example logic_lab_client -- [--kumi-home <dir>] [--watch <seconds>] [--call <method> <json params>]... [--bench <method> <json params> <n>]
//!
//! `--bench` calls a method n times and prints the median and the worst round trip: the lab's numbers for `focus`.

use std::io::{BufRead, BufReader, Write};
use std::net::TcpStream;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::{Duration, Instant};

use daw_bridge::files;
use daw_bridge::handshake::{ClientHandshake, ClientStep};
use daw_bridge::paths::{self, BridgePaths};
use daw_bridge::wire::{Frame, Request};
use daw_bridge::Daw;
use serde_json::{json, Value};

fn main() {
    let mut args = std::env::args().skip(1);
    let (mut home, mut calls, mut watch, mut benches) = (None, vec![], 5.0, vec![]);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--kumi-home" => home = Some(PathBuf::from(args.next().expect("--kumi-home <dir>"))),
            "--call" => calls.push((args.next().expect("--call <method> <json>"), args.next().expect("--call <method> <json>"))),
            "--bench" => benches.push((
                args.next().expect("--bench <method> <json> <n>"),
                args.next().expect("--bench <method> <json> <n>"),
                args.next().and_then(|n| n.parse::<usize>().ok()).expect("--bench <method> <json> <n>"),
            )),
            "--watch" => watch = args.next().and_then(|s| s.parse().ok()).expect("--watch <seconds>"),
            other => {
                panic!("unknown argument {other}; usage: lab_client [--kumi-home <dir>] [--watch <seconds>] [--call <method> <json>]... [--bench <method> <json> <n>]")
            }
        }
    }
    let home = home.or_else(paths::kumi_home).expect("Kumi's folder");
    let bridge_paths = BridgePaths::new(&home, Daw::Logic);
    let found = files::scan_discovery(&bridge_paths.discovery(), Daw::Logic);
    let discovery = found
        .iter()
        .find(|f| f.alive)
        .and_then(|f| f.discovery.as_ref().ok())
        .unwrap_or_else(|| panic!("no running Logic bridge found in {}", bridge_paths.discovery().display()));
    println!("discovery {}", serde_json::to_string(discovery).unwrap());
    let secret = files::read_secret(&bridge_paths.secret()).expect("the secret");

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
    client.call("ping", json!({}));
    client.call("status", json!({}));
    for (method, params) in calls {
        client.call(&method, serde_json::from_str(&params).expect("the params as JSON"));
    }
    for (method, params, n) in benches {
        let params: Value = serde_json::from_str(&params).expect("the params as JSON");
        let mut times: Vec<Duration> = (0..n).map(|_| client.timed(&method, params.clone())).collect();
        times.sort();
        let median = times[times.len() / 2];
        println!(
            "{method} x{n}: median {:.2} ms, worst {:.2} ms, best {:.2} ms",
            median.as_secs_f64() * 1000.0,
            times[times.len() - 1].as_secs_f64() * 1000.0,
            times[0].as_secs_f64() * 1000.0
        );
    }
    client.watch(Duration::from_secs_f64(watch));
}

struct Client {
    stream: TcpStream,
    lines: Receiver<String>,
    next_id: u64,
}

impl Client {
    /// One call, silent: how long its round trip took.
    fn timed(&mut self, method: &str, params: Value) -> Duration {
        let id = self.next_id;
        self.next_id += 1;
        let sent = Instant::now();
        self.stream.write_all(&Frame::Request(Request { id, method: method.into(), params, deadline_ms: None }).encode()).unwrap();
        loop {
            let line = self.lines.recv_timeout(Duration::from_secs(15)).expect("a response");
            let frame: Value = serde_json::from_str(&line).unwrap();
            if frame["type"] == "response" && frame["id"] == id {
                return sent.elapsed();
            }
        }
    }

    fn call(&mut self, method: &str, params: Value) -> Value {
        let id = self.next_id;
        self.next_id += 1;
        let sent = Instant::now();
        self.stream.write_all(&Frame::Request(Request { id, method: method.into(), params, deadline_ms: None }).encode()).unwrap();
        loop {
            let line = self.lines.recv_timeout(Duration::from_secs(15)).expect("a response");
            let frame: Value = serde_json::from_str(&line).unwrap();
            if frame["type"] == "response" && frame["id"] == id {
                println!("{method} in {} ms {line}", sent.elapsed().as_millis());
                return frame;
            }
            print_frame(&frame, &line);
        }
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
