//! The M2 acceptance run against a real REAPER (the lab copy, announced to the reviewer first): timings of the reads on a
//! big project (the lab's 200 tracks and 2,000 items), how fast the selection is followed, a modal dialog (S5) and a new
//! project in the same tab (S10).
//!
//! cargo run --release -p reaper-kumi --example lab_m2 -- <kumi home> perf|select|s5 [seconds]|s10 new|s10 open <path>|refs|watch [seconds]

use std::io::{BufRead, BufReader, Write};
use std::net::TcpStream;
use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::{Duration, Instant};

use daw_bridge::files;
use daw_bridge::handshake::{ClientHandshake, ClientStep};
use daw_bridge::paths::BridgePaths;
use daw_bridge::wire::{Frame, Request};
use daw_bridge::Daw;
use serde_json::{json, Value};

type Answer = Result<Value, (String, String)>;

struct Client {
    stream: TcpStream,
    lines: Receiver<String>,
    next_id: u64,
}

fn connect(home: &str) -> Client {
    let paths = BridgePaths::new(std::path::Path::new(home), Daw::Reaper);
    let found = files::scan_discovery(&paths.discovery(), Daw::Reaper);
    let discovery = found.iter().find(|f| f.alive).and_then(|f| f.discovery.as_ref().ok()).expect("no running REAPER bridge found");
    let secret = files::read_secret(&paths.secret()).expect("the secret");
    let mut stream = TcpStream::connect((discovery.host.as_str(), discovery.port)).expect("connect");
    let lines = reader(stream.try_clone().unwrap());
    let (mut handshake, hello) = ClientHandshake::start(secret, "lab_m2");
    stream.write_all(&hello).unwrap();
    loop {
        let line = lines.recv_timeout(Duration::from_secs(5)).expect("a handshake line");
        match handshake.on_line(&line).expect("handshake") {
            ClientStep::Send(bytes) => stream.write_all(&bytes).unwrap(),
            ClientStep::Welcome(welcome) => {
                println!(
                    "welcome: REAPER {} bridge {} methods {:?}",
                    welcome.identity.daw_version, welcome.identity.bridge, welcome.methods
                );
                break;
            }
        }
    }
    Client { stream, lines, next_id: 1 }
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

impl Client {
    /// Sends a request; the answer with how long it took and how many bytes it was.
    fn ask(&mut self, method: &str, params: Value, wait: Duration) -> (Answer, Duration, usize) {
        let id = self.next_id;
        self.next_id += 1;
        let sent = Instant::now();
        let deadline_ms =
            Some(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() as u64 + wait.as_millis() as u64);
        self.stream.write_all(&Frame::Request(Request { id, method: method.into(), params, deadline_ms }).encode()).unwrap();
        loop {
            let Ok(line) = self.lines.recv_timeout(wait) else { return (Err(("timeout".into(), "no answer".into())), sent.elapsed(), 0) };
            let frame: Value = serde_json::from_str(&line).unwrap();
            if frame["type"] == "response" && frame["id"] == id {
                let took = sent.elapsed();
                let answer = if frame["ok"] == true {
                    Ok(frame["result"].clone())
                } else {
                    Err((
                        frame["error"]["code"].as_str().unwrap_or("").to_string(),
                        frame["error"]["message"].as_str().unwrap_or("").to_string(),
                    ))
                };
                return (answer, took, line.len());
            }
        }
    }

    fn call(&mut self, method: &str, params: Value) -> Value {
        match self.ask(method, params, Duration::from_secs(20)).0 {
            Ok(value) => value,
            Err(error) => panic!("{method}: {error:?}"),
        }
    }
}

fn ms(duration: Duration) -> String {
    format!("{:.1} ms", duration.as_secs_f64() * 1000.0)
}

/// Every page of a read, with the number of pages, rows, the longest page and the total time.
fn read_all(client: &mut Client, label: &str, params: Value) -> Vec<Value> {
    let started = Instant::now();
    let (mut rows, mut pages, mut biggest, mut slowest) = (Vec::new(), 0, 0, Duration::ZERO);
    let mut params = params;
    loop {
        let (answer, took, bytes) = client.ask("read", params.clone(), Duration::from_secs(20));
        let page = answer.unwrap_or_else(|e| panic!("{label}: {e:?}"));
        pages += 1;
        biggest = biggest.max(bytes);
        slowest = slowest.max(took);
        rows.extend(page["items"].as_array().unwrap().iter().cloned());
        match page.get("nextCursor") {
            Some(cursor) => params["cursor"] = cursor.clone(),
            None => break,
        }
    }
    println!(
        "{label}: {} rows in {pages} pages, {} total, slowest page {}, biggest page {} bytes",
        rows.len(),
        ms(started.elapsed()),
        ms(slowest),
        biggest
    );
    rows
}

fn timed(client: &mut Client, label: &str, method: &str, params: Value) -> Value {
    let (answer, took, bytes) = client.ask(method, params, Duration::from_secs(20));
    let value = answer.unwrap_or_else(|e| panic!("{label}: {e:?}"));
    println!("{label}: {} {bytes} bytes", ms(took));
    value
}

fn perf(client: &mut Client) {
    println!("status: {}", client.call("status", json!({}))["busy"]);
    for round in 1..=3 {
        let snapshot = timed(client, &format!("snapshot #{round}"), "snapshot", json!({}));
        if round == 1 {
            println!(
                "  tracks {} markers {} truncated {} project {} selection track {} focus view n/a",
                snapshot["tracks"].as_array().unwrap().len(),
                snapshot["markers"].as_array().unwrap().len(),
                snapshot.get("truncated").map_or("no".to_string(), |t| t.to_string()),
                snapshot["project"]["name"],
                snapshot["selection"]["trackRef"],
            );
        }
    }
    let tracks = read_all(client, "read track (limit 200)", json!({"kind": "track", "limit": 200}));
    read_all(client, "read item, whole project (limit 200)", json!({"kind": "item", "limit": 200}));
    let track = |n: usize| tracks[n]["ref"].as_str().unwrap().to_string();
    let items = read_all(client, "read item of Track 005", json!({"kind": "item", "parent": track(5)}));
    println!("  first item {}", items[0]);
    // Item lookups by ref: the first walks the project once, the rest don't.
    let reference = items[3]["ref"].as_str().unwrap().to_string();
    let first = timed(client, "read ref (item), cold", "read", json!({"ref": reference}));
    println!("  {}", first["items"][0]["name"]);
    let started = Instant::now();
    for _ in 0..50 {
        client.call("read", json!({"ref": reference}));
    }
    println!("read ref (item) warm: {} per read over 50", ms(started.elapsed() / 50));
    // FX, containers, parameters.
    let fx = read_all(client, "read fx of Track 002", json!({"kind": "fx", "parent": track(2)}));
    for row in &fx {
        println!("  fx {} {} container={} childRefs={}", row["ref"], row["name"], row["container"], row["childRefs"]);
    }
    if let Some(container) = fx.iter().find(|r| r["container"] == true) {
        let children = read_all(client, "read fx children", json!({"kind": "fx", "parent": container["ref"]}));
        for child in &children {
            println!("  child {} {} container={}", child["ref"], child["name"], child["container"]);
        }
        if let Some(first) = children.first() {
            let params = read_all(client, "read param of first child", json!({"kind": "param", "parent": first["ref"]}));
            println!("  {} params, first {}", params.len(), params.first().cloned().unwrap_or(Value::Null));
        }
    }
    let body = timed(client, "focus_body fx Track 002", "focus_body", json!({"kind": "fx", "ref": track(2)}));
    println!("  nodes {}", body["nodes"]);
    // Sends, envelopes, notes.
    let sends = read_all(client, "read send of Track 200", json!({"kind": "send", "parent": track(200)}));
    println!("  {} routes, first {}", sends.len(), sends.first().cloned().unwrap_or(Value::Null));
    let envelopes = read_all(client, "read envelope of Track 003", json!({"kind": "envelope", "parent": track(3)}));
    println!("  {}", envelopes.first().cloned().unwrap_or(Value::Null));
    let fx_envelopes = read_all(client, "read envelope of Track 001", json!({"kind": "envelope", "parent": track(1)}));
    println!("  {}", fx_envelopes.first().cloned().unwrap_or(Value::Null));
    let take = items[0]["takeRef"].as_str().unwrap().to_string();
    let notes = read_all(client, "read note", json!({"kind": "note", "parent": take}));
    println!("  {} notes, first {}", notes.len(), notes.first().cloned().unwrap_or(Value::Null));
    // The bodies.
    let mut cursor = Value::Null;
    let (mut pages, started, mut biggest) = (0, Instant::now(), 0);
    loop {
        let mut params = json!({"kind": "arrange"});
        if !cursor.is_null() {
            params["cursor"] = cursor.clone();
        }
        let (answer, _, bytes) = client.ask("focus_body", params, Duration::from_secs(20));
        let page = answer.unwrap();
        pages += 1;
        biggest = biggest.max(bytes);
        if pages == 1 {
            println!(
                "  arrange first page: window {} tracks {} items on track 1: {} markers {} tempo {}",
                page["window"],
                page["tracks"].as_array().unwrap().len(),
                page["tracks"][0]["items"].as_array().map_or(0, Vec::len),
                page["markers"],
                page["tempoMarkers"]
            );
        }
        match page.get("nextCursor") {
            Some(next) => cursor = next.clone(),
            None => break,
        }
    }
    println!("focus_body arrange (whole project): {pages} pages, {}, biggest {biggest} bytes", ms(started.elapsed()));
    let arrange =
        timed(client, "focus_body arrange 0..16 beats", "focus_body", json!({"kind": "arrange", "window": {"from": 0, "to": 16}}));
    println!("  tracks {} bytes (window {})", arrange["tracks"].as_array().unwrap().len(), arrange["window"]);
    let midi = timed(client, "focus_body midi", "focus_body", json!({"kind": "midi", "ref": take}));
    println!(
        "  notes {} lanes {} length {} beatsPerBar {}",
        midi["notes"].as_array().unwrap().len(),
        midi["ccLanes"],
        midi["length"],
        midi["beatsPerBar"]
    );
    let routing = timed(client, "focus_body routing Track 200", "focus_body", json!({"kind": "routing", "ref": track(200)}));
    println!("  routes {} parentSend {}", routing["routes"].as_array().unwrap().len(), routing["parentSend"]);
    let focus = timed(client, "focus", "focus", json!({}));
    println!("  {}", focus);
}

/// Changes the selection with an action and polls `focus` until it shows; the time from asking to seeing.
fn select(client: &mut Client) {
    let rounds = 8;
    for (name, select, unselect, key, expect) in
        [("tracks", 40296, 40297, "trackCount", 200u64), ("items", 40182, 40289, "itemCount", 2000u64)]
    {
        let mut latencies = Vec::new();
        for _ in 0..rounds {
            for (action, wanted) in [(select, true), (unselect, false)] {
                let started = Instant::now();
                let (answer, _, _) = client.ask("action", json!({"command": action}), Duration::from_secs(10));
                answer.unwrap();
                let acted = started.elapsed();
                let seen = loop {
                    let focus = client.call("focus", json!({}));
                    let count = focus["selection"][key].as_u64().unwrap_or(0);
                    // All of them selected, or none.
                    if (wanted && count >= expect) || (!wanted && count == 0) {
                        break (started.elapsed(), count, focus["ageMs"].as_u64().unwrap_or(0));
                    }
                    if started.elapsed() > Duration::from_secs(5) {
                        panic!("selection {name} not followed in 5 s (count {count})");
                    }
                    thread::sleep(Duration::from_millis(5));
                };
                latencies.push((seen.0, acted));
                thread::sleep(Duration::from_millis(120));
            }
        }
        latencies.sort_by_key(|(seen, _)| *seen);
        let list: Vec<String> = latencies.iter().map(|(seen, _)| format!("{:.0}", seen.as_secs_f64() * 1000.0)).collect();
        println!("selection {name}: seen after (ms, incl. the action, polled every 5 ms) {}", list.join(" "));
        println!("  worst {} median {}", ms(latencies.last().unwrap().0), ms(latencies[latencies.len() / 2].0));
    }
}

/// Opens a project in the tab (the lab's script does it, as a user would: `new` runs File: New project, `open <path>`
/// opens that file without prompting) and waits for the bridge to notice: a new epoch and a new identity.
fn s10(client: &mut Client, home: &str, how: &str, path: Option<String>) {
    let project = |client: &mut Client| {
        let read = client.call("read", json!({"kind": "project"}));
        let focus = client.call("focus", json!({}));
        (read["epoch"].clone(), read["items"][0].clone(), focus["epoch"].clone())
    };
    let (epoch, row, focus_epoch) = project(client);
    println!("before: epoch {epoch} identity {} name {} tabs {} focus epoch {focus_epoch}", row["identity"], row["name"], row["tabCount"]);
    let trigger = std::path::Path::new(home).parent().unwrap().join("m2").join(if how == "new" { "trigger-new" } else { "trigger-open" });
    std::fs::write(&trigger, path.unwrap_or_default()).unwrap();
    let started = Instant::now();
    let noticed = loop {
        let (now, row, focus_epoch) = project(client);
        if now != epoch {
            break Some((started.elapsed(), now, row, focus_epoch));
        }
        if started.elapsed() > Duration::from_secs(10) {
            break None;
        }
        thread::sleep(Duration::from_millis(20));
    };
    match noticed {
        Some((took, now, row, focus_epoch)) => {
            println!(
                "noticed after {}: epoch {now} identity {} name {} path {} tabs {} focus epoch {focus_epoch}",
                ms(took),
                row["identity"],
                row["name"],
                row["path"],
                row["tabCount"]
            );
            let snapshot = client.call("snapshot", json!({}));
            println!("snapshot: {} tracks, project {}", snapshot["tracks"].as_array().unwrap().len(), snapshot["project"]["name"]);
        }
        None => {
            let (now, row, _) = project(client);
            println!(
                "NOT noticed in 10 s: epoch {now} identity {} name {} path {} tabs {}",
                row["identity"], row["name"], row["path"], row["tabCount"]
            );
        }
    }
}

/// With a dialog open (the lab's script opens one): what the bridge says, and how fast.
fn s5(client: &mut Client, seconds: f64) {
    let until = Instant::now() + Duration::from_secs_f64(seconds);
    let mut last = String::new();
    while Instant::now() < until {
        let status = client.call("status", json!({}));
        let (read, took, _) = client.ask("read", json!({"kind": "track", "limit": 1}), Duration::from_secs(6));
        let (focus, focus_took, _) = client.ask("focus", json!({}), Duration::from_secs(6));
        let line = format!(
            "status busy={} lastTickMs={} ticks={} | read {} in {} | focus {} in {}",
            status["busy"],
            status["lastTickMs"],
            status["ticksPerSecond"],
            match &read {
                Ok(_) => "ok".to_string(),
                Err((code, message)) => format!("{code}: {message}"),
            },
            ms(took),
            match &focus {
                Ok(f) => format!("ok busy={} ageMs={}", f["busy"], f["ageMs"]),
                Err((code, message)) => format!("{code}: {message}"),
            },
            ms(focus_took),
        );
        if line != last {
            println!("{line}");
            last = line;
        }
        thread::sleep(Duration::from_millis(250));
    }
}

/// The refs of sends and markers before and after the lab's script removes a send between two and renumbers a marker to
/// a number another has: each ref that should survive reads back to the same object; the removed one is gone.
fn refs(client: &mut Client, home: &str) {
    let m2 = std::path::Path::new(home).parent().unwrap().join("m2");
    let tracks = read_all(client, "read track", json!({"kind": "track", "limit": 200}));
    let track = |n: usize| tracks[n]["ref"].as_str().unwrap().to_string();
    std::fs::write(m2.join("trigger-refs-setup"), "").unwrap();
    thread::sleep(Duration::from_millis(1500));
    let describe = |client: &mut Client| {
        let sends = read_all(client, "  sends of Track 001", json!({"kind": "send", "parent": track(1)}));
        let markers = read_all(client, "  markers", json!({"kind": "marker"}));
        let regions = read_all(client, "  regions", json!({"kind": "region"}));
        (sends, markers, regions)
    };
    let (sends, markers, regions) = describe(client);
    for row in &sends {
        println!("  send {} dir={} volumeDb={} destRef={}", row["ref"], row["direction"], row["volumeDb"], row["destRef"]);
    }
    for row in markers.iter().chain(&regions) {
        println!("  {} number={} name={}", row["ref"], row["number"], row["name"]);
    }
    let before: Vec<(String, Value)> =
        sends.iter().chain(&markers).chain(&regions).map(|r| (r["ref"].as_str().unwrap().to_string(), r["volumeDb"].clone())).collect();
    std::fs::write(m2.join("trigger-refs-change"), "").unwrap();
    thread::sleep(Duration::from_millis(1500));
    println!("script: {}", std::fs::read_to_string(m2.join("refs-change.log")).unwrap_or_default().trim());
    for (reference, volume) in &before {
        let (answer, _, _) = client.ask("read", json!({"ref": reference}), Duration::from_secs(10));
        match answer {
            Ok(page) => {
                let row = &page["items"][0];
                println!(
                    "  after: {reference} -> volumeDb {} (was {volume}) number {} name {}",
                    row["volumeDb"], row["number"], row["name"]
                );
            }
            Err((code, message)) => println!("  after: {reference} -> {code}: {message}"),
        }
    }
    let (sends, _, _) = describe(client);
    println!("  sends now: {}", sends.iter().map(|r| r["ref"].to_string()).collect::<Vec<_>>().join(" "));
}

/// A send to another track removed from among sends to one track leaves those sends' refs on their own sends.
fn sends(client: &mut Client, home: &str) {
    let m2 = std::path::Path::new(home).parent().unwrap().join("m2");
    let tracks = read_all(client, "read track", json!({"kind": "track", "limit": 200}));
    let first = tracks[1]["ref"].as_str().unwrap().to_string();
    std::fs::write(m2.join("trigger-refs-setup"), "").unwrap();
    thread::sleep(Duration::from_millis(1500));
    let list = |client: &mut Client| -> Vec<(String, Value)> {
        read_all(client, "  sends of Track 001", json!({"kind": "send", "parent": first}))
            .iter()
            .map(|r| (r["ref"].to_string(), r["volumeDb"].clone()))
            .collect()
    };
    let before = list(client);
    for (reference, volume) in &before {
        println!("  before {reference} {volume} dB");
    }
    std::fs::write(m2.join("trigger-remove100"), "").unwrap();
    thread::sleep(Duration::from_millis(1500));
    println!("script: {}", std::fs::read_to_string(m2.join("remove100.log")).unwrap_or_default().trim());
    let after = list(client);
    for (reference, volume) in &after {
        println!("  after  {reference} {volume} dB");
    }
    let survivors_same = before.iter().filter(|b| after.iter().any(|a| a.0 == b.0)).all(|b| after.contains(b));
    println!("every ref that is still there reads back to its own send: {survivors_same}; {} -> {} sends", before.len(), after.len());
}

/// The colours the bridge reports: tracks, the first track's items and the markers. A colour REAPER keeps but doesn't use
/// (no 0x1000000 flag) must read as null.
fn colors(client: &mut Client) {
    let tracks = read_all(client, "read track", json!({"kind": "track", "limit": 200}));
    let coloured = tracks.iter().filter(|t| !t["color"].is_null()).count();
    println!("  {} tracks, {} with a colour, {} without", tracks.len(), coloured, tracks.len() - coloured);
    for track in tracks.iter().take(6) {
        println!("  {} -> {}", track["name"], track["color"]);
    }
    if let Some(track) = tracks.get(1) {
        let items = read_all(client, "read item of the first track", json!({"kind": "item", "parent": track["ref"], "limit": 5}));
        println!("  first items' colours: {}", items.iter().map(|i| i["color"].to_string()).collect::<Vec<_>>().join(" "));
    }
    let focus = client.call("focus", json!({}));
    println!("  focus header track: {}", focus["selection"]["track"]);
}

/// The state change count and dirty flag around the moves that tell a first save from another file opened.
fn counts(client: &mut Client, home: &str, steps: &[&str]) {
    let m2 = std::path::Path::new(home).parent().unwrap().join("m2");
    let _ = std::fs::remove_file(m2.join("id.log"));
    let show = |client: &mut Client, label: &str| {
        let status = client.call("status", json!({}));
        let page = client.call("read", json!({"kind": "project"}));
        let row = &page["items"][0];
        println!(
            "{label}: count {} dirty {} path {} id {} epoch {} identity {}",
            status["stateChangeCount"], row["dirty"], row["path"], row["id"], page["epoch"], row["identity"]
        );
    };
    show(client, "start");
    for step in steps {
        std::fs::write(m2.join(format!("trigger-id-{step}")), "").unwrap();
        thread::sleep(Duration::from_millis(1500));
        show(client, step);
    }
    println!("{}", std::fs::read_to_string(m2.join("id.log")).unwrap_or_default().trim());
}

/// What the bridge reports for the project's id, step by step through Save As, a template and a copy opened from disk,
/// with the lab script's own reading of REAPER's ProjExtState beside it (m2/id.log).
fn ids(client: &mut Client, home: &str) {
    let m2 = std::path::Path::new(home).parent().unwrap().join("m2");
    let _ = std::fs::remove_file(m2.join("id.log"));
    let row = |client: &mut Client| {
        let page = client.call("read", json!({"kind": "project"}));
        let row = page["items"][0].clone();
        println!(
            "  bridge: path {} saved {} id {} identity {} epoch {}",
            row["path"], row["saved"], row["id"], row["identity"], page["epoch"]
        );
    };
    println!("start");
    row(client);
    for step in ["set", "saveas", "template", "copy"] {
        std::fs::write(m2.join(format!("trigger-id-{step}")), "").unwrap();
        thread::sleep(Duration::from_millis(1500));
        println!("{step}:");
        row(client);
    }
    println!("{}", std::fs::read_to_string(m2.join("id.log")).unwrap_or_default().trim());
}

fn watch(client: &mut Client, seconds: f64) {
    let until = Instant::now() + Duration::from_secs_f64(seconds);
    let mut last = Value::Null;
    while Instant::now() < until {
        let focus = client.call("focus", json!({}));
        let shown = json!({"view": focus["view"], "sel": focus["selection"], "changed": focus["changed"], "epoch": focus["epoch"]});
        if shown != last {
            println!("{shown}");
            last = shown;
        }
        thread::sleep(Duration::from_millis(150));
    }
}

fn main() {
    let mut args = std::env::args().skip(1);
    let home = args.next().expect("usage: lab_m2 <kumi home> perf|select|s5|s10|watch [seconds]");
    let scenario = args.next().expect("a scenario");
    let extra = args.next();
    let seconds: f64 = extra.as_deref().and_then(|s| s.parse().ok()).unwrap_or(10.0);
    let mut client = connect(&home);
    match scenario.as_str() {
        "perf" => perf(&mut client),
        "select" => select(&mut client),
        "s10" => s10(&mut client, &home, &extra.clone().unwrap_or_else(|| "new".into()), args.next()),
        "s5" => s5(&mut client, seconds),
        "watch" => watch(&mut client, seconds),
        "refs" => refs(&mut client, &home),
        "colors" => colors(&mut client),
        "ids" => ids(&mut client, &home),
        "counts" => {
            let steps: Vec<&str> = extra.as_deref().unwrap_or("").split(',').filter(|s| !s.is_empty()).collect();
            counts(&mut client, &home, &steps)
        }
        "sends" => sends(&mut client, &home),
        other => panic!("no scenario {other}"),
    }
    if std::env::var("QUIT").is_ok() {
        println!("quit: {:?}", client.ask("quit", json!({"save": false}), Duration::from_secs(10)).0);
    }
}
