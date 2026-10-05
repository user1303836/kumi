//! Opt-in timing on real Live: what a tutorial's rebuild costs in Live, without a model. A MIDI track
//! with Drift and four effects in one plan, each device's parameters read, 30 of them set by name (one
//! change per device, then one change each), and Kumi's looks at the Set in between: each timed, with
//! the bridge requests it took. It adds one track, named for the run, and deletes it at the end.
//!   cargo build --release -p ableton-mcp-server --bins
//!   cargo run --release -p kumi --example time_rebuild
use futures::FutureExt;
use kumi::config::find_bridge_config;
use kumi_common::{
    abort::{self, Signal},
    js::string::head,
    time::{now_ms, perf_now},
};
use kumi_runtime::{
    core::timing,
    create_ableton_integration,
    integrations::ableton::{connection::Connect, AbletonOptions},
    mcp::client,
    system::Env,
    ChangeRecord, Integration, JsonObject, KernelTool, Observation, BRIDGE_TOOLS,
};
use serde_json::{json, Value};
use std::{
    cell::RefCell,
    path::{Path, PathBuf},
    rc::Rc,
};

const DEVICES: [(&str, &str); 5] = [
    ("drift", "instruments/Drift"),
    ("sat", "audio_effects/Saturator"),
    ("filter", "audio_effects/Auto Filter"),
    ("verb", "audio_effects/Reverb"),
    ("comp", "audio_effects/Compressor"),
];
const PER_DEVICE: usize = 6;

fn main() {
    let env: Env = std::env::vars().collect();
    let bridge_config = find_bridge_config(&env).expect("the bridge is installed");
    let mut folder = std::env::current_exe().unwrap().parent().unwrap().to_path_buf();
    if folder.file_name().is_some_and(|name| name == "examples") {
        folder.pop();
    }
    let bridge = folder.join("ableton-mcp-server");
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    let local = tokio::task::LocalSet::new();
    local.block_on(&runtime, run(bridge_config, bridge));
}

fn connect_to(bridge: PathBuf, config: String) -> Connect {
    Rc::new(move |signal| {
        client::connect_mcp(client::Options {
            signal,
            bridge_config: Some(PathBuf::from(&config)),
            entry: Some(bridge.clone()),
            cwd: bridge.parent().map(Path::to_path_buf),
            allow_tools: BRIDGE_TOOLS.clone(),
            ..Default::default()
        })
        .boxed_local()
    })
}

fn signal() -> Signal {
    abort::timeout(300_000)
}

struct Driver {
    integration: Rc<dyn Integration>,
    observation: RefCell<Option<Observation>>,
    records: Rc<RefCell<Vec<(f64, ChangeRecord)>>>,
}

struct Ran {
    ok: bool,
    ms: f64,
    requests: u32,
    body: Value,
    text: String,
}

impl Driver {
    async fn observe(&self) -> Ran {
        let recorder = timing::begin();
        let began = perf_now();
        let observation = self.integration.observe(signal(), None).await.expect("observe");
        let ms = perf_now() - began;
        *self.observation.borrow_mut() = Some(observation);
        Ran { ok: true, ms, requests: recorder.finish().live_requests, body: Value::Null, text: String::new() }
    }
    fn tool(&self, name: &str) -> Rc<dyn KernelTool> {
        self.observation
            .borrow()
            .as_ref()
            .unwrap()
            .tools
            .iter()
            .find(|tool| tool.name() == name)
            .cloned()
            .unwrap_or_else(|| panic!("no {name}"))
    }
    async fn run(&self, name: &str, input: Value) -> Ran {
        let input: JsonObject = input.as_object().cloned().unwrap();
        let tool = self.tool(name);
        let recorder = timing::begin();
        let began = perf_now();
        let result = tool.execute(input, signal()).await;
        let ms = perf_now() - began;
        let requests = recorder.finish().live_requests;
        match result {
            Ok(result) => Ran {
                ok: !result.is_error,
                ms,
                requests,
                body: serde_json::from_str(&result.text).unwrap_or(Value::Null),
                text: result.text,
            },
            Err(error) => Ran { ok: false, ms, requests, body: Value::Null, text: error.message() },
        }
    }
}

fn line(what: &str, ran: &Ran, changes: usize) {
    let each = if changes > 0 { format!("  {:6.0} ms a change", ran.ms / changes as f64) } else { String::new() };
    println!("  {} {:58} {:8.0} ms  {:4} requests{each}", if ran.ok { "ok  " } else { "FAIL" }, what, ran.ms, ran.requests);
    if !ran.ok {
        let stopped = &ran.body["stopped"];
        println!("       {}", if stopped.is_null() { head(&ran.text, 600) } else { head(&stopped.to_string(), 1500) });
    }
}

async fn run(bridge_config: String, bridge: PathBuf) {
    let records = Rc::new(RefCell::new(Vec::new()));
    let mut options = AbletonOptions::new(Rc::new(|_, _| {}));
    options.bridge_config = Some(bridge_config.clone());
    options.connect = Some(connect_to(bridge, bridge_config));
    options.on_change = Some({
        let records = records.clone();
        Rc::new(move |change: ChangeRecord| records.borrow_mut().push((perf_now(), change)))
    });
    options.change_timeout_ms = Some(60_000);
    let driver = Driver { integration: create_ableton_integration(options), observation: RefCell::new(None), records };
    let name = format!("Kumi Timing {}", now_ms() % 100_000);
    println!("\nA tutorial's rebuild on real Live, track \"{name}\":");
    let began = perf_now();
    driver.integration.start(signal()).await.expect("start");
    println!("  ok   {:58} {:8.0} ms", "connect (start the bridge, reach Live)", perf_now() - began);
    line("look at the Set (observe)", &driver.observe().await, 0);
    line("look again (observe, warm)", &driver.observe().await, 0);

    // 1. The track and its chain, as one plan (how the model sends it since 1.7.4).
    let mut steps = vec![json!({"tool":"add_tracks_and_scenes","input":{"tracks":[{"name":name,"kind":"midi"}],"scenes":[]},"as":"t"})];
    for (alias, item) in DEVICES {
        steps.push(json!({"tool":"load_device","input":{"trackRef":"@t","itemId":item},"as":alias}));
    }
    let before = driver.records.borrow().len();
    let chain = driver.run("make_changes", json!({"steps": steps})).await;
    line("plan: a MIDI track, Drift and 4 effects (6 changes)", &chain, 6);
    print_steps(&driver.records.borrow()[before..]);
    if !chain.ok {
        return;
    }

    // 2. Each device's parameters, as the model reads them without a plan.
    line("look at the Set after the plan", &driver.observe().await, 0);
    let tracks = driver.run("live_discover", json!({"kind":"track","limit":100})).await;
    let track =
        tracks.body["live"]["items"].as_array().and_then(|items| items.iter().rev().find(|row| row["name"] == name.as_str())).cloned();
    let Some(track) = track else {
        println!("  (couldn't find the track again: {})", head(&tracks.text, 300));
        return;
    };
    let devices = driver.run("live_discover", json!({"kind":"device","parent":track["ref"],"limit":100})).await;
    line("discover the track's devices", &devices, 0);
    let device_rows: Vec<Value> = devices.body["live"]["items"].as_array().cloned().unwrap_or_default();
    let mut chosen: Vec<(String, String, Vec<(String, Value)>)> = Vec::new();
    for device in &device_rows {
        let parameters = driver.run("live_discover", json!({"kind":"parameter","parent":device["ref"],"limit":100})).await;
        let rows = parameters.body["live"]["items"].as_array().cloned().unwrap_or_default();
        line(&format!("discover {}'s parameters ({})", device["name"].as_str().unwrap_or("?"), rows.len()), &parameters, 0);
        if chosen.is_empty() && !rows.is_empty() {
            println!("       a parameter row: {}", head(&rows[0].to_string(), 400));
        }
        let picks: Vec<(String, Value)> = rows
            .iter()
            .filter(|row| row["name"] != "Device On")
            .filter_map(|row| {
                let (min, max) = (row["min"].as_f64()?, row["max"].as_f64()?);
                let quantized = row["isQuantized"].as_bool().unwrap_or(false) || row["quantized"].as_bool().unwrap_or(false);
                (max > min).then(|| {
                    let value = if quantized { (min + 1.0).min(max) } else { min + (max - min) * 0.37 };
                    (row["name"].as_str().unwrap_or_default().to_string(), json!(value))
                })
            })
            .take(PER_DEVICE)
            .collect();
        chosen.push((
            device["ref"].as_str().unwrap_or_default().to_string(),
            device["name"].as_str().unwrap_or_default().to_string(),
            picks,
        ));
    }
    let total: usize = chosen.iter().map(|(_, _, picks)| picks.len()).sum();

    // 3. The parameters as one change per device, then the same as one change per parameter.
    let batched: Vec<Value> = chosen
        .iter()
        .filter(|(_, _, picks)| !picks.is_empty())
        .map(|(device, _, picks)| {
            json!({"tool":"set_device_parameter","input":{"deviceRef":device,"values":picks.iter().map(|(name, value)| json!({"parameter":name,"value":value})).collect::<Vec<_>>()}})
        })
        .collect();
    let before = driver.records.borrow().len();
    let ran = driver.run("make_changes", json!({"steps": batched})).await;
    line(&format!("plan: {total} parameters by name, one change per device ({})", batched.len()), &ran, batched.len());
    print_steps(&driver.records.borrow()[before..]);
    let single: Vec<Value> = chosen
        .iter()
        .flat_map(|(device, _, picks)| {
            picks.iter().map(move |(name, value)| {
                let nudged = value.as_f64().map(|v| json!(v * 0.9)).unwrap_or(value.clone());
                json!({"tool":"set_device_parameter","input":{"deviceRef":device,"parameter":name,"value":nudged}})
            })
        })
        .collect();
    let before = driver.records.borrow().len();
    let ran = driver.run("make_changes", json!({"steps": single})).await;
    line(&format!("plan: the same {total} parameters, one change each"), &ran, single.len());
    print_steps(&driver.records.borrow()[before..]);

    // 4. With TIME_REBUILD_TRACKS=N, a longer plan: N more tracks, each with Drift and the effects.
    let more: usize = std::env::var("TIME_REBUILD_TRACKS").ok().and_then(|n| n.parse().ok()).unwrap_or(0);
    if more > 0 {
        let mut steps = Vec::new();
        for track in 1..=more {
            steps.push(json!({"tool":"add_tracks_and_scenes","input":{"tracks":[{"name":format!("{name} {track}"),"kind":"midi"}],"scenes":[]},"as":format!("t{track}")}));
            for (_, item) in DEVICES {
                steps.push(json!({"tool":"load_device","input":{"trackRef":format!("@t{track}"),"itemId":item}}));
            }
        }
        let count = steps.len();
        let ran = driver.run("make_changes", json!({"steps": steps})).await;
        line(&format!("plan: {more} more tracks with Drift and 4 effects ({count} changes)"), &ran, count);
        // One at a time, looked up again each time: a deletion moves the tracks after it.
        let began = perf_now();
        let mut deleted = 0;
        loop {
            line("look at the Set", &driver.observe().await, 0);
            let tracks = driver.run("live_discover", json!({"kind":"track","limit":100})).await;
            let found = tracks.body["live"]["items"]
                .as_array()
                .and_then(|rows| rows.iter().find(|row| row["name"].as_str().is_some_and(|n| n.starts_with(&format!("{name} ")))).cloned());
            let Some(found) = found else { break };
            let ran = driver.run("make_changes", json!({"steps":[{"tool":"delete_track","input":{"trackRef":found["ref"]}}]})).await;
            if !ran.ok {
                line("delete a track", &ran, 1);
                break;
            }
            deleted += 1;
        }
        println!("  ok   {:58} {:8.0} ms", format!("deleted those {deleted} tracks, one at a time"), perf_now() - began);
    }

    // 5. Put the Set back.
    line("look at the Set", &driver.observe().await, 0);
    let tracks = driver.run("live_discover", json!({"kind":"track","limit":100})).await;
    let track = tracks.body["live"]["items"]
        .as_array()
        .and_then(|items| items.iter().rev().find(|row| row["name"] == name.as_str()))
        .cloned()
        .unwrap_or(track);
    let ran = driver.run("make_changes", json!({"steps":[{"tool":"delete_track","input":{"trackRef":track["ref"]}}]})).await;
    line("delete the run's track", &ran, 1);
    let _ = driver.integration.close().await;
}

/// Each change's time in a plan, from one change record's arrival to the next.
fn print_steps(records: &[(f64, ChangeRecord)]) {
    let mut last: Option<f64> = None;
    let mut seen = Vec::new();
    for (at, record) in records {
        if seen.contains(&record.id) {
            continue;
        }
        seen.push(record.id.clone());
        let gap = last.map(|last| at - last);
        last = Some(*at);
        println!("       {:>7}  {}", gap.map(|gap| format!("+{gap:.0} ms")).unwrap_or_else(|| "first".into()), head(&record.title, 70));
    }
}
