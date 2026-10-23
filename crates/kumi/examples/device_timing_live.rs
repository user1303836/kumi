//! Opt-in check on real Live that the devices Kumi makes keep Live's time. It records a device's MIDI into the
//! Arrangement, through a second track that takes the device's track's output. While it records, it moves the
//! device's rate to a slower division and back. Then it reads the notes and says, for each run:
//! - how far each note starts from the grid line it belongs to, in milliseconds: the mean and the worst, and how many
//!   are off the grid of either division (past a tolerance);
//! - doubled notes (two starts within a millisecond);
//! - notes held past their step (a stuck or missed note-off).
//!
//! It checks a step sequencer made with make_device from Kumi's [kumi.clock] and [kumi.notes], and any device file
//! given (--file <path>.amxd, with --rate "<its rate parameter>", --from <index> --to <index> for its two divisions).
//! Then scenes, each note against where Live's song was: Live's loop (3 and 3.5 beats), 7/8 and 6/8, a jump while
//! playing, a tempo ramp, tempos typed in, and a stop then Space (--scenarios: the scenes only).
//! --record <file>.json records Live's own transport instead, for the clock's replay test: a device writes down what
//! plugphasor~ and plugsync~ say at every sample through eight scenes (stops and plays, loops, jumps, tempo changes).
//! --encode <folder> <file>.json makes that file from recordings already made (no Live).
//! No model and no sign-in. It works in the open Set (it never opens or closes one): name it, and use a disposable
//! one. It sets the tempo to 120 and takes everything back after.
//!   cargo build -p ableton-mcp-server -p kumi-runtime --bins
//!   cargo run -p kumi --example device_timing_live -- --set "<Set name>" [--file <path>.amxd --rate Rate --from 4 --to 3]
use futures::FutureExt;
use kumi::config::find_bridge_config;
use kumi_common::{
    abort::{self, Signal},
    js::string::{head, trim},
    time::now_ms,
};
use kumi_daw_ableton::{
    connection::Connect,
    create_ableton_integration,
    devices::{amxd::decode_amxd, tool::kind_folder},
    samples::user_library,
    AbletonOptions, BRIDGE_TOOLS,
};
use kumi_runtime::{
    core::contracts::ChangeState, mcp::client, ChangeRecord, Integration, JsonObject, KernelTool, Observation, RuntimeError,
};
use serde_json::{json, Value};
use std::{
    cell::RefCell,
    path::{Path, PathBuf},
    rc::Rc,
    time::Duration,
};

/// The tempo the check plays at, and how far a note may start from its line and still be on it.
const BPM: f64 = 120.0;
const ON_GRID_MS: f64 = 1.5;
/// How long it records, in beats, and when it moves the rate and back (seconds after the recording starts).
const BEATS: f64 = 16.0;
const SLOWER_AT: f64 = 3.3;
const BACK_AT: f64 = 5.7;

fn main() {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let values = |flag: &str| -> Vec<String> {
        argv.windows(2).filter(|pair| pair[0] == flag).map(|pair| trim(&pair[1]).to_owned()).filter(|value| !value.is_empty()).collect()
    };
    if let Some(at) = argv.iter().position(|arg| arg == "--encode") {
        let (Some(folder), Some(out)) = (argv.get(at + 1), argv.get(at + 2)) else {
            eprintln!("--encode <folder of recordings> <file>.json");
            std::process::exit(2);
        };
        match encode_folder(Path::new(folder)).and_then(|text| std::fs::write(out, text).map_err(|error| error.to_string())) {
            Ok(()) => println!("{out}"),
            Err(why) => {
                eprintln!("{why}");
                std::process::exit(1);
            }
        }
        return;
    }
    let Some(wanted) = values("--set").into_iter().next() else {
        eprintln!("Name the open Set, as Live shows it: cargo run -p kumi --example device_timing_live -- --set \"<Set name>\"");
        eprintln!("It adds tracks, records in the Arrangement and undoes it all, so use a disposable Set.");
        std::process::exit(2);
    };
    let scenarios_only = argv.iter().any(|arg| arg == "--scenarios");
    let record = values("--record").into_iter().next();
    let file = values("--file").into_iter().next().map(|file| Subject {
        file: PathBuf::from(file),
        rate: values("--rate").into_iter().next().unwrap_or_else(|| "Rate".into()),
        from: values("--from").into_iter().next().and_then(|value| value.parse().ok()).unwrap_or(4.0),
        to: values("--to").into_iter().next().and_then(|value| value.parse().ok()).unwrap_or(3.0),
    });
    let env = std::env::vars().collect();
    let Some(bridge_config) = find_bridge_config(&env) else {
        eprintln!("The Ableton bridge isn't installed; run Kumi's bridge setup first.");
        std::process::exit(2);
    };
    let bridge = bridge_program();
    let harness = bridge.with_file_name(if cfg!(windows) { "kumi-harness.exe" } else { "kumi-harness" });
    if !bridge.is_file() || !harness.is_file() {
        eprintln!("Build the bridge and Kumi's check first: cargo build -p ableton-mcp-server -p kumi-runtime --bins");
        std::process::exit(2);
    }
    if std::env::var_os("KUMI_HARNESS_BIN").is_none() {
        std::env::set_var("KUMI_HARNESS_BIN", &harness);
    }
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().expect("a runtime");
    let local = tokio::task::LocalSet::new();
    std::process::exit(local.block_on(&runtime, device_timing_live(wanted, file, scenarios_only, record, bridge_config, bridge)));
}

/// A device file to check: its rate parameter and the two values it's moved between.
struct Subject {
    file: PathBuf,
    rate: String,
    from: f64,
    to: f64,
}

fn bridge_program() -> PathBuf {
    let name = if cfg!(windows) { "ableton-mcp-server.exe" } else { "ableton-mcp-server" };
    let mut folder = std::env::current_exe().ok().and_then(|exe| exe.parent().map(Path::to_path_buf)).unwrap_or_default();
    if folder.file_name().is_some_and(|name| name == "examples") {
        folder.pop();
    }
    folder.join(name)
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

struct Run {
    integration: Rc<dyn Integration>,
    observation: RefCell<Option<Observation>>,
    records: Rc<RefCell<Vec<ChangeRecord>>>,
    made: RefCell<Vec<PathBuf>>,
    failed: RefCell<Vec<String>>,
}

async fn device_timing_live(
    wanted: String,
    file: Option<Subject>,
    scenarios_only: bool,
    record: Option<String>,
    bridge_config: String,
    bridge: PathBuf,
) -> i32 {
    let records = Rc::new(RefCell::new(Vec::<ChangeRecord>::new()));
    let mut options = AbletonOptions::new(Rc::new(|_, _| {}));
    options.bridge_config = Some(bridge_config.clone());
    options.connect = Some(connect_to(bridge, bridge_config));
    options.on_change = Some({
        let records = records.clone();
        Rc::new(move |change: ChangeRecord| {
            let mut records = records.borrow_mut();
            match records.iter_mut().find(|record| record.id == change.id) {
                Some(record) => *record = change,
                None => records.push(change),
            }
        })
    });
    options.change_timeout_ms = Some(30_000);
    let run = Run {
        integration: create_ableton_integration(options),
        observation: RefCell::default(),
        records,
        made: RefCell::default(),
        failed: RefCell::default(),
    };
    let code = match run.check(&wanted, file, scenarios_only, record).await {
        Ok(()) => {
            let failed = run.failed.borrow();
            println!("\n{} failed{}", failed.len(), if failed.is_empty() { String::new() } else { format!(": {}", failed.join("; ")) });
            i32::from(!failed.is_empty())
        }
        Err(error) => {
            eprintln!("device-timing-live: {}", head(&error.message(), 300));
            1
        }
    };
    let _ = run.call("play", json!({"action": "stop"})).await;
    println!("\nUndo, newest first");
    run.undo_all().await;
    let _ = run.integration.close().await;
    for file in run.made.borrow().iter() {
        let _ = std::fs::remove_file(file);
    }
    code
}

/// What the notes of one take say about its timing.
#[derive(Debug, Default)]
struct Timing {
    notes: usize,
    mean_ms: f64,
    worst_ms: f64,
    off_grid: Vec<String>,
    doubled: Vec<String>,
    held: Vec<String>,
    /// The notes furthest from their lines, worst first.
    worst: Vec<String>,
    /// How many start within 0.1, 0.5, 1 and 1.5 ms of their lines, and further.
    spread: [usize; 5],
}

/// The notes' timing against two grids, `fine` and `coarse` (step lengths in beats): each note belongs to the nearer.
fn timing(notes: &[Value], fine: f64, coarse: f64) -> Timing {
    let ms = |beats: f64| beats * 60_000.0 / BPM;
    let mut starts: Vec<(f64, f64, i64)> =
        notes.iter().filter_map(|note| Some((note["start"].as_f64()?, note["duration"].as_f64()?, note["pitch"].as_i64()?))).collect();
    starts.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut found = Timing { notes: starts.len(), ..Timing::default() };
    let mut total = 0.0;
    let mut offs: Vec<(f64, String)> = Vec::new();
    for (start, duration, pitch) in &starts {
        let off = |grid: f64| ms(start - (start / grid).round() * grid);
        let (a, b) = (off(fine), off(coarse));
        let off = if a.abs() <= b.abs() { a } else { b };
        offs.push((off.abs(), format!("{pitch} at beat {start:.4} ({off:+.2} ms)")));
        let bin = [0.1, 0.5, 1.0, 1.5].iter().position(|edge| off.abs() < *edge).unwrap_or(4);
        found.spread[bin] += 1;
        total += off;
        found.worst_ms = found.worst_ms.max(off.abs());
        if off.abs() > ON_GRID_MS {
            found.off_grid.push(format!("{pitch} at beat {start:.4} ({off:+.2} ms)"));
        }
        if *duration > coarse * 1.05 {
            found.held.push(format!("{pitch} at beat {start:.4} held {duration:.3} beats"));
        }
    }
    for pair in starts.windows(2) {
        if ms(pair[1].0 - pair[0].0) < 1.0 {
            found.doubled.push(format!("{} and {} at beat {:.4}", pair[0].2, pair[1].2, pair[0].0));
        }
    }
    found.mean_ms = if starts.is_empty() { 0.0 } else { total / starts.len() as f64 };
    offs.sort_by(|a, b| b.0.total_cmp(&a.0));
    found.worst = offs.into_iter().take(3).map(|(_, said)| said).collect();
    found
}

impl Run {
    fn fail(&self, what: &str) {
        self.failed.borrow_mut().push(what.to_string());
    }

    async fn observe(&self) -> Result<Observation, RuntimeError> {
        let observation = self.integration.observe(signal(), None).await?;
        *self.observation.borrow_mut() = Some(observation.clone());
        Ok(observation)
    }

    async fn call(&self, name: &str, input: Value) -> Result<Value, String> {
        let tool: Option<Rc<dyn KernelTool>> =
            self.observation.borrow().as_ref().and_then(|seen| seen.tools.iter().find(|tool| tool.name() == name).cloned());
        let Some(tool) = tool else { return Err(format!("{name} isn't offered for this Set")) };
        let input = match input {
            Value::Object(input) => input,
            _ => JsonObject::new(),
        };
        match tool.execute(input, signal()).await {
            Ok(result) if !result.is_error => Ok(serde_json::from_str(&result.text).unwrap_or(Value::String(result.text))),
            Ok(result) => Err(result.text),
            Err(error) => Err(error.message()),
        }
    }

    async fn rows(&self, kind: &str, extra: Value) -> Vec<Value> {
        let mut input = json!({"kind": kind, "limit": 100});
        input.as_object_mut().unwrap().extend(extra.as_object().cloned().unwrap_or_default());
        self.call("live_discover", input)
            .await
            .map(|body| body["live"]["items"].as_array().cloned().unwrap_or_default())
            .unwrap_or_default()
    }

    async fn python(&self, code: &str) -> Result<Value, RuntimeError> {
        let done = self.call("run_python", json!({"code": code, "mode": "eval"})).await.map_err(RuntimeError::plain)?;
        let done = if done.get("ok").is_some() { done } else { done["live"].clone() };
        if done["ok"] != true {
            return Err(RuntimeError::plain(format!("Live's Python: {}", head(&done.to_string(), 300))));
        }
        Ok(done["result"].clone())
    }

    async fn track(&self, name: &str) -> Result<Value, RuntimeError> {
        let tracks = self.rows("track", json!({"fields": ["name"]})).await;
        tracks.iter().rfind(|row| row["name"] == name).cloned().ok_or_else(|| RuntimeError::plain(format!("the track {name} is gone")))
    }

    async fn add_track(&self, name: &str) -> Result<(), RuntimeError> {
        self.call("add_tracks_and_scenes", json!({"tracks": [{"name": name, "kind": "midi"}], "scenes": []}))
            .await
            .map_err(RuntimeError::plain)?;
        self.observe().await?;
        Ok(())
    }

    /// A copy of a device file in its kind's folder, as `name`: its Browser item.
    fn copy(&self, file: &Path, name: &str) -> Result<String, String> {
        let bytes = std::fs::read(file).map_err(|error| error.to_string())?;
        let kind = decode_amxd(&bytes).ok_or("not a Max for Live device file")?.kind;
        let folder = PathBuf::from(user_library(None, None)).join("Kumi").join(kind_folder(kind));
        std::fs::create_dir_all(&folder).map_err(|error| error.to_string())?;
        let copy = folder.join(format!("{name}.amxd"));
        std::fs::write(&copy, bytes).map_err(|error| error.to_string())?;
        self.made.borrow_mut().push(copy);
        Ok(format!("user_library/Kumi/{}/{name}", kind_folder(kind)))
    }

    /// The device loaded on the track, waiting for Live's Browser to list it.
    async fn load(&self, name: &str, item: &str) -> Result<(), String> {
        for attempt in 0..15 {
            let track = self.track(name).await.map_err(|error| error.message())?;
            match self.call("load_device", json!({"itemId": item, "trackRef": track["ref"]})).await {
                Ok(_) => return Ok(()),
                Err(why) if attempt < 14 => {
                    println!("    (not loaded yet: {})", head(&why, 120));
                    tokio::time::sleep(Duration::from_secs(2)).await;
                    self.observe().await.map_err(|error| error.message())?;
                }
                Err(why) => return Err(why),
            }
        }
        Err("not loaded".into())
    }

    /// One take: the device on its own track, its MIDI recorded on another for BEATS beats while its rate moves from
    /// `from` to `to` and back. The notes recorded.
    async fn take(&self, label: &str, item: &str, rate: &str, from: f64, to: f64, stamp: u128) -> Result<Vec<Value>, RuntimeError> {
        let device_track = format!("Timing {label} {stamp}");
        let take_track = format!("Timing Take {label} {stamp}");
        self.add_track(&device_track).await?;
        self.load(&device_track, item).await.map_err(RuntimeError::plain)?;
        self.add_track(&take_track).await?;
        let take = self.track(&take_track).await?;
        self.call(
            "set_routing",
            json!({"trackRef": take["ref"], "inputType": device_track, "inputSubRouting": "Post FX", "outputType": "No Output", "arm": true, "monitoring": "off"}),
        )
        .await
        .map_err(RuntimeError::plain)?;
        let set = |value: f64| {
            format!(
            "[p for p in [t for t in song.tracks if t.name == {}][0].devices[0].parameters if p.name == {}][0].__setattr__('value', {value})",
            json!(device_track),
            json!(rate)
        )
        };
        self.python(&set(from)).await?;
        self.python(&format!("song.__setattr__('tempo', {BPM:.1})")).await?;
        self.call("set_transport", json!({"position": 0, "loopEnabled": false})).await.map_err(RuntimeError::plain)?;
        // A change makes refs found before it stale: the take's track is found again.
        self.observe().await?;
        let take = self.track(&take_track).await?;
        self.call("record", json!({"action": "start", "lane": "arrangement", "destinationTrackRef": take["ref"]}))
            .await
            .map_err(RuntimeError::plain)?;
        let began = std::time::Instant::now();
        let at = |seconds: f64| async move {
            let left = seconds - began.elapsed().as_secs_f64();
            if left > 0.0 {
                tokio::time::sleep(Duration::from_secs_f64(left)).await;
            }
        };
        at(SLOWER_AT).await;
        self.python(&set(to)).await?;
        at(BACK_AT).await;
        self.python(&set(from)).await?;
        at(BEATS * 60.0 / BPM).await;
        self.call("play", json!({"action": "stop"})).await.map_err(RuntimeError::plain)?;
        let _ = self.call("record", json!({"action": "stop", "lane": "arrangement"})).await;
        self.observe().await?;
        let take = self.track(&take_track).await?;
        self.call("set_routing", json!({"trackRef": take["ref"], "arm": false})).await.map_err(RuntimeError::plain)?;
        self.observe().await?;
        let take = self.track(&take_track).await?;
        let clips = self.rows("arrangement-clip", json!({"parent": take["ref"]})).await;
        let Some(clip) = clips.first() else { return Ok(Vec::new()) };
        let read = self.call("read_notes", json!({"clipRef": clip["ref"], "format": "json"})).await.map_err(RuntimeError::plain)?;
        Ok(read["clips"][0]["notes"].as_array().cloned().unwrap_or_default())
    }

    /// A device holding a long note, turned off (`delete` false) or deleted two beats in: the notes recorded. Its note
    /// should end there, not at the end of its length.
    async fn held(&self, item: &str, delete: bool, stamp: u128) -> Result<Vec<Value>, RuntimeError> {
        let label = if delete { "deleted" } else { "off" };
        let device_track = format!("Timing {label} {stamp}");
        let take_track = format!("Timing Take {label} {stamp}");
        self.add_track(&device_track).await?;
        self.load(&device_track, item).await.map_err(RuntimeError::plain)?;
        self.add_track(&take_track).await?;
        let take = self.track(&take_track).await?;
        self.call(
            "set_routing",
            json!({"trackRef": take["ref"], "inputType": device_track, "inputSubRouting": "Post FX", "outputType": "No Output", "arm": true, "monitoring": "off"}),
        )
        .await
        .map_err(RuntimeError::plain)?;
        self.python(&format!("song.__setattr__('tempo', {BPM:.1})")).await?;
        self.call("set_transport", json!({"position": 0, "loopEnabled": false})).await.map_err(RuntimeError::plain)?;
        self.observe().await?;
        let take = self.track(&take_track).await?;
        self.call("record", json!({"action": "start", "lane": "arrangement", "destinationTrackRef": take["ref"]}))
            .await
            .map_err(RuntimeError::plain)?;
        tokio::time::sleep(Duration::from_secs_f64(2.0 * 60.0 / BPM)).await;
        let track = format!("[t for t in song.tracks if t.name == {}][0]", json!(device_track));
        if delete {
            self.python(&format!("{track}.delete_device(0)")).await?;
        } else {
            self.python(&format!("{track}.devices[0].parameters[0].__setattr__('value', 0)")).await?;
        }
        tokio::time::sleep(Duration::from_secs_f64(3.0 * 60.0 / BPM)).await;
        self.call("play", json!({"action": "stop"})).await.map_err(RuntimeError::plain)?;
        let _ = self.call("record", json!({"action": "stop", "lane": "arrangement"})).await;
        self.observe().await?;
        let take = self.track(&take_track).await?;
        self.call("set_routing", json!({"trackRef": take["ref"], "arm": false})).await.map_err(RuntimeError::plain)?;
        self.observe().await?;
        let take = self.track(&take_track).await?;
        let clips = self.rows("arrangement-clip", json!({"parent": take["ref"]})).await;
        let Some(clip) = clips.first() else { return Ok(Vec::new()) };
        let read = self.call("read_notes", json!({"clipRef": clip["ref"], "format": "json"})).await.map_err(RuntimeError::plain)?;
        Ok(read["clips"][0]["notes"].as_array().cloned().unwrap_or_default())
    }

    fn say(&self, label: &str, notes: &[Value], fine: f64, coarse: f64) {
        let found = timing(notes, fine, coarse);
        println!(
            "  {label}: {} notes, mean {:+.2} ms from the grid, worst {:.2} ms; {} off the grid, {} doubled, {} held past a step",
            found.notes,
            found.mean_ms,
            found.worst_ms,
            found.off_grid.len(),
            found.doubled.len(),
            found.held.len()
        );
        println!("    worst: {}", found.worst.join(", "));
        let [a, b, c, d, e] = found.spread;
        println!("    from their lines: {a} under 0.1 ms, {b} under 0.5, {c} under 1, {d} under 1.5, {e} further");
        for line in found.off_grid.iter().take(80).chain(found.doubled.iter().take(8)).chain(found.held.iter().take(8)) {
            println!("    {line}");
        }
        if found.notes == 0 || !found.off_grid.is_empty() || !found.doubled.is_empty() || !found.held.is_empty() {
            self.fail(label);
        }
    }

    async fn check(&self, wanted: &str, file: Option<Subject>, scenarios_only: bool, record: Option<String>) -> Result<(), RuntimeError> {
        self.integration.start(signal()).await?;
        let observation = self.observe().await?;
        let context: Value = serde_json::from_str(&observation.context).unwrap_or(Value::Null);
        let set = context["set"]["name"].as_str().unwrap_or("?").to_owned();
        if set != wanted {
            println!("The open Set is “{set}”, not “{wanted}”; nothing was changed.");
            return Err(RuntimeError::plain("not the Set named"));
        }
        let stamp = (now_ms() % 10_000) as u128;
        if let Some(out) = record {
            return self.record(Path::new(&out), stamp).await;
        }
        if scenarios_only {
            return self.scenarios(stamp).await;
        }
        // A step sequencer made the way the guide says: every step on, its pitch the step's index.
        let spec = json!({
            "type": "midi_effect",
            "name": format!("Timing Check {stamp}"),
            "about": "Kumi's timing check: a note on every step.",
            "controls": [{"name": "Rate", "type": "choice", "options": ["1/4", "1/8", "1/16", "1/32"], "default": "1/16", "group": "Clock"}],
            "patch": concat!(
                "clock = [kumi.clock 4n 8n 16n 32n]\n",
                "\"Rate\" -> clock\n",
                "notes = [kumi.notes]\n",
                "clock -> [+ 60] -> notes\n",
                "clock.1 -> [* 0.5] -> notes.2\n",
                "notes -> out\n",
            ),
        });
        let made = self.call("make_device", spec).await.map_err(RuntimeError::plain)?;
        for key in ["wrong", "changed", "notes"] {
            if let Some(said) = made.get(key) {
                println!("  make_device {key}: {said}");
            }
        }
        if let Some(file) = made["file"].as_str() {
            self.made.borrow_mut().push(PathBuf::from(file));
        }
        let item = made["itemId"].as_str().unwrap_or_default().to_string();
        println!("\nKumi's clock, 1/16 moved to 1/8 and back");
        let notes = self.take("clock", &item, "Rate", 2.0, 1.0, stamp).await?;
        self.say("Kumi's clock", &notes, 0.25, 0.5);
        // The guide's step sequencer: each step's gate and note read from its lane by name. Odd steps are on, each
        // playing 60 plus its index, so a note's pitch and place say whether the right step played.
        let mut controls = vec![json!({"name": "Rate", "type": "choice", "options": ["1/8", "1/16"], "default": "1/16", "group": "Clock"})];
        for step in 1..=16 {
            controls.push(json!({"name": format!("Gate {step}"), "type": "switch", "default": step % 2 == 1}));
        }
        for step in 1..=16 {
            controls.push(json!({"name": format!("Note {step}"), "type": "integer", "min": 0, "max": 127, "default": 59 + step}));
        }
        let lanes = json!({
            "type": "midi_effect",
            "name": format!("Timing Lanes {stamp}"),
            "about": "Kumi's timing check: a step sequencer read by lane.",
            "controls": controls,
            "patch": concat!(
                "clock = [kumi.clock 8n 16n]\n",
                "\"Rate\" -> clock\n",
                "clock -> step = [t i i i]\n",
                "step.2 -> \"Gate\" -> open = [gate]\n",
                "step.1 -> playhead\n",
                "step -> \"Note\" -> open.1\n",
                "open -> notes = [kumi.notes]\n",
                "clock.1 -> [* 0.5] -> notes.2\n",
                "notes -> out\n",
            ),
        });
        let made = self.call("make_device", lanes).await.map_err(RuntimeError::plain)?;
        for key in ["wrong", "changed", "notes"] {
            if let Some(said) = made.get(key) {
                println!("  make_device {key}: {said}");
            }
        }
        if let Some(file) = made["file"].as_str() {
            self.made.borrow_mut().push(PathBuf::from(file));
        }
        let item = made["itemId"].as_str().unwrap_or_default().to_string();
        println!("\nThe guide's step sequencer, read by lane");
        let notes = self.take("lanes", &item, "Rate", 1.0, 1.0, stamp).await?;
        self.say("the lanes", &notes, 0.25, 0.5);
        let wrong: Vec<String> = notes
            .iter()
            .filter_map(|note| {
                let (start, pitch) = (note["start"].as_f64()?, note["pitch"].as_i64()?);
                let index = ((start / 0.25).round() as i64).rem_euclid(16);
                (index % 2 == 1 || pitch != 60 + index).then(|| format!("{pitch} at beat {start:.3} (step {})", index + 1))
            })
            .collect();
        println!(
            "  {} notes; on a step that's off or with another step's note: {}",
            notes.len(),
            if wrong.is_empty() { "none".into() } else { wrong.join(", ") }
        );
        if notes.len() < 28 || !wrong.is_empty() {
            self.fail("the lanes' steps");
        }
        // Long notes (a whole note a step, 90% of it held), ended by turning the device off or deleting it.
        let long = json!({
            "type": "midi_effect",
            "name": format!("Timing Held {stamp}"),
            "about": "Kumi's timing check: long notes.",
            "controls": [],
            "patch": concat!(
                "clock = [kumi.clock 1n]\n",
                "notes = [kumi.notes]\n",
                "clock -> [+ 48] -> notes\n",
                "clock.1 -> [* 0.9] -> notes.2\n",
                "notes -> out\n",
            ),
        });
        let made = self.call("make_device", long).await.map_err(RuntimeError::plain)?;
        if let Some(file) = made["file"].as_str() {
            self.made.borrow_mut().push(PathBuf::from(file));
        }
        let long_item = made["itemId"].as_str().unwrap_or_default().to_string();
        for delete in [false, true] {
            let label = if delete { "deleted" } else { "turned off" };
            let notes = self.held(&long_item, delete, stamp).await?;
            let ends: Vec<String> = notes
                .iter()
                .filter_map(|note| {
                    Some(format!(
                        "{} from beat {:.3} to {:.3}",
                        note["pitch"],
                        note["start"].as_f64()?,
                        note["start"].as_f64()? + note["duration"].as_f64()?
                    ))
                })
                .collect();
            // Its note started at beat 0 and would run to 3.6: it should end within half a beat of 2.
            let ended = notes.iter().all(|note| {
                let end = note["start"].as_f64().unwrap_or(0.0) + note["duration"].as_f64().unwrap_or(9.0);
                end < 2.6
            });
            println!("\nA long note, its device {label} at beat 2: {}", if ends.is_empty() { "no notes".into() } else { ends.join("; ") });
            if notes.is_empty() || !ended {
                self.fail(&format!("a held note when the device was {label}"));
            }
        }
        // A modulator: an LFO in time with Live's bars, sent to mod, so it has a Map page.
        let lfo = json!({
            "type": "audio_effect",
            "name": format!("Timing LFO {stamp}"),
            "about": "Kumi's timing check: an LFO with a Map page.",
            "controls": [{"name": "Bars", "type": "integer", "min": 1, "max": 8, "default": 1, "group": "LFO"}],
            "map": {"slots": 8},
            "patch": concat!(
                "song = [kumi.transport]\n",
                "wave = [gen~] { Param bars(1); out1 = 0.5 + 0.5 * sin(twopi * in1 / (4 * bars)); }\n",
                "song -> wave\n",
                "\"Bars\" -> [prepend bars] -> wave\n",
                "wave -> mod\n",
                "in.0 -> out.0\n",
                "in.1 -> out.1\n",
            ),
        });
        let made = self.call("make_device", lfo).await.map_err(RuntimeError::plain)?;
        for key in ["wrong", "changed", "notes"] {
            if let Some(said) = made.get(key) {
                println!("  make_device {key}: {said}");
            }
        }
        if let Some(file) = made["file"].as_str() {
            self.made.borrow_mut().push(PathBuf::from(file));
        }
        let lfo_track = format!("Timing LFO {stamp}");
        self.add_track(&lfo_track).await?;
        self.load(&lfo_track, made["itemId"].as_str().unwrap_or_default()).await.map_err(RuntimeError::plain)?;
        let names = self
            .python(&format!("[p.name for p in [t for t in song.tracks if t.name == {}][0].devices[0].parameters]", json!(lfo_track)))
            .await?;
        // The Map page's own parameters are stored with the Set only, as Live's own multimap's are, so Live's list has
        // just the device's own.
        println!("\nA Map page's device: Live sees {names}");
        if !names.as_array().is_some_and(|names| names.iter().any(|name| name == "Bars") && !names.iter().any(|name| name == "Map 1 Min")) {
            self.fail("the Map page's device");
        }
        self.scenarios(stamp).await?;
        if let Some(subject) = file {
            println!("\n{}, its {} moved from {} to {} and back", subject.file.display(), subject.rate, subject.from, subject.to);
            let item = self.copy(&subject.file, &format!("Timing File {stamp}")).map_err(RuntimeError::plain)?;
            self.observe().await?;
            let notes = self.take("file", &item, &subject.rate, subject.from, subject.to, stamp).await?;
            self.say("the file", &notes, 0.25, 0.5);
        }
        Ok(())
    }

    /// Python statements in Live, with `result` as the answer.
    async fn python_exec(&self, code: &str) -> Result<Value, RuntimeError> {
        let done = self.call("run_python", json!({"code": code})).await.map_err(RuntimeError::plain)?;
        let done = if done.get("ok").is_some() { done } else { done["live"].clone() };
        if done["ok"] != true {
            return Err(RuntimeError::plain(format!("Live's Python: {}", head(&done.to_string(), 300))));
        }
        Ok(done["result"].clone())
    }

    /// The device on a track of its own, its MIDI recorded into a Session clip on another (the clip's time is linear,
    /// whatever Live's song position does) for `beats`, with `setup` run first and each of `actions` at its second. The
    /// notes: pitch, start and length in beats from the recording's start.
    async fn session_take(
        &self,
        label: &str,
        item: &str,
        setup: &str,
        actions: &[(f64, String)],
        beats: f64,
        stamp: u128,
    ) -> Result<Vec<(i64, f64, f64)>, RuntimeError> {
        let device_track = format!("Timing {label} {stamp}");
        let take_track = format!("Timing Take {label} {stamp}");
        self.add_track(&device_track).await?;
        self.load(&device_track, item).await.map_err(RuntimeError::plain)?;
        self.add_track(&take_track).await?;
        let take = self.track(&take_track).await?;
        self.call(
            "set_routing",
            json!({"trackRef": take["ref"], "inputType": device_track, "inputSubRouting": "Post FX", "outputType": "No Output", "arm": true, "monitoring": "off"}),
        )
        .await
        .map_err(RuntimeError::plain)?;
        let track = format!("[t for t in song.tracks if t.name == {}][0]", json!(take_track));
        self.python_exec(&format!(
            "song.tempo = {BPM:.1}\nsong.clip_trigger_quantization = 0\nsong.stop_playing()\nsong.current_song_time = 0.0\n{setup}\n{track}.clip_slots[0].fire()\nresult = True"
        ))
        .await?;
        let began = std::time::Instant::now();
        for (at, code) in actions {
            let left = at - began.elapsed().as_secs_f64();
            if left > 0.0 {
                tokio::time::sleep(Duration::from_secs_f64(left)).await;
            }
            self.python_exec(&format!("{code}\nresult = True")).await?;
        }
        let left = beats * 60.0 / BPM - began.elapsed().as_secs_f64();
        if left > 0.0 {
            tokio::time::sleep(Duration::from_secs_f64(left)).await;
        }
        self.python_exec("song.stop_playing()\nresult = True").await?;
        tokio::time::sleep(Duration::from_millis(500)).await;
        let notes = self
            .python_exec(&format!(
                "clip = {track}.clip_slots[0].clip\nresult = [[n.pitch, n.start_time, n.duration] for n in clip.get_notes_extended(0, 128, 0.0, 100000.0)] if clip else []\nsong.loop = False\nsong.signature_numerator = 4\nsong.signature_denominator = 4\nsong.tempo = {BPM:.1}"
            ))
            .await?;
        self.observe().await?;
        let mut notes: Vec<(i64, f64, f64)> = notes
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|note| Some((note[0].as_i64()?, note[1].as_f64()?, note[2].as_f64()?)))
            .collect();
        notes.sort_by(|a, b| a.1.total_cmp(&b.1));
        Ok(notes)
    }

    /// Played from the start, stopped `stop` seconds in, then (after `between`, Python run while stopped) played again
    /// with Space's start_playing, from Live's insert marker, while Live records the arrangement: where Live started
    /// again (beats) and the notes recorded, at their song positions.
    async fn restart_take(
        &self,
        label: &str,
        item: &str,
        stop: f64,
        between: &str,
        stamp: u128,
    ) -> Result<(f64, Vec<(i64, f64, f64)>), RuntimeError> {
        let device_track = format!("Timing {label} {stamp}");
        let take_track = format!("Timing Take {label} {stamp}");
        self.add_track(&device_track).await?;
        self.load(&device_track, item).await.map_err(RuntimeError::plain)?;
        self.add_track(&take_track).await?;
        let take = self.track(&take_track).await?;
        self.call(
            "set_routing",
            json!({"trackRef": take["ref"], "inputType": device_track, "inputSubRouting": "Post FX", "outputType": "No Output", "arm": true, "monitoring": "off"}),
        )
        .await
        .map_err(RuntimeError::plain)?;
        let track = format!("[t for t in song.tracks if t.name == {}][0]", json!(take_track));
        self.python_exec(&format!(
            "song.tempo = {BPM:.1}\nsong.loop = False\nsong.record_mode = False\nsong.stop_playing()\nsong.stop_playing()\nsong.current_song_time = 0.0\nsong.start_playing()\nresult = True"
        ))
        .await?;
        tokio::time::sleep(Duration::from_secs_f64(stop)).await;
        self.python_exec("song.stop_playing()\nresult = True").await?;
        tokio::time::sleep(Duration::from_millis(600)).await;
        if !between.is_empty() {
            self.python_exec(&format!("{between}\nresult = True")).await?;
            tokio::time::sleep(Duration::from_millis(600)).await;
        }
        self.python_exec("song.record_mode = True\nsong.start_playing()\nresult = True").await?;
        tokio::time::sleep(Duration::from_secs(3)).await;
        self.python_exec("song.stop_playing()\nsong.record_mode = False\nresult = True").await?;
        tokio::time::sleep(Duration::from_millis(500)).await;
        let read = self
            .python_exec(&format!(
                "clips = list({track}.arrangement_clips)\nresult = [min([c.start_time for c in clips] or [-1]), [[n.pitch, c.start_time + n.start_time, n.duration] for c in clips for n in c.get_notes_extended(0, 128, 0.0, 100000.0)]]"
            ))
            .await?;
        self.observe().await?;
        let take = self.track(&take_track).await?;
        self.call("set_routing", json!({"trackRef": take["ref"], "arm": false})).await.map_err(RuntimeError::plain)?;
        self.observe().await?;
        let mut notes: Vec<(i64, f64, f64)> = read[1]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|note| Some((note[0].as_i64()?, note[1].as_f64()?, note[2].as_f64()?)))
            .collect();
        notes.sort_by(|a, b| a.1.total_cmp(&b.1));
        Ok((read[0].as_f64().unwrap_or(-1.0), notes))
    }

    /// Notes against where Live's song was: `song` gives the song position (beats) for a time in the clip. From the
    /// first note to the last there should be one on every sixteenth and no two within half a step, each on its line
    /// and playing 60 plus its step (16 steps of a sixteenth). Says what's wrong, if anything. `bpm`: the take's fastest
    /// tempo, for its milliseconds.
    fn followed(&self, label: &str, notes: &[(i64, f64, f64)], song: impl Fn(f64) -> f64, bpm: f64) {
        let (Some(first), Some(last)) = (notes.first().copied(), notes.last().copied()) else {
            println!("  {label}: no notes");
            self.fail(label);
            return;
        };
        let mut wrong = Vec::new();
        let mut each = vec![0; ((last.1 - first.1) / 0.25).round() as usize + 1];
        for (pitch, start, _) in notes {
            each[((start - first.1) / 0.25).round() as usize] += 1;
            let position = song(*start);
            let line = (position / 0.25).round() * 0.25;
            let off_ms = (position - line) * 60_000.0 / bpm;
            let index = ((line / 0.25).round() as i64).rem_euclid(16);
            if off_ms.abs() > ON_GRID_MS || *pitch != 60 + index {
                wrong.push(format!("{pitch} at {start:.4} (song {position:.4}, {off_ms:+.2} ms, step {})", index + 1));
            }
        }
        for (at, count) in each.iter().enumerate() {
            if *count != 1 {
                wrong.push(format!("{count} notes at {:.4}", first.1 + at as f64 * 0.25));
            }
        }
        println!(
            "  {label}: {} notes, {}@{:.4} to {}@{:.4}; {}",
            notes.len(),
            first.0,
            first.1,
            last.0,
            last.1,
            if wrong.is_empty() {
                "one on every sixteenth between, each on its line, playing its own step".to_string()
            } else {
                format!("{} wrong: {}", wrong.len(), wrong.iter().take(12).cloned().collect::<Vec<_>>().join("; "))
            }
        );
        if !wrong.is_empty() {
            self.fail(label);
            let all: Vec<String> = notes.iter().map(|(pitch, start, _)| format!("{pitch}@{start:.3}")).collect();
            println!("    notes: {}", all.join(" "));
        }
    }

    /// Live's own transport through each of the scenes, recorded by a device that writes it down, as the clock's replay
    /// test reads it.
    async fn record(&self, out: &Path, stamp: u128) -> Result<(), RuntimeError> {
        let folder = std::env::temp_dir().join(format!("kumi-live-transport-{stamp}"));
        std::fs::create_dir_all(&folder).map_err(|error| RuntimeError::plain(error.to_string()))?;
        let wav = folder.join("current.wav");
        let made = self.call("make_device", recorder(&wav, stamp)?).await.map_err(RuntimeError::plain)?;
        if let Some(file) = made["file"].as_str() {
            self.made.borrow_mut().push(PathBuf::from(file));
        }
        let item = made["itemId"].as_str().unwrap_or_default().to_string();
        println!("\nLive's transport, recorded at every sample");
        for scene in scenes() {
            let _ = std::fs::remove_file(&wav);
            let track = format!("Recorder {} {stamp}", scene.name);
            self.add_track(&track).await?;
            self.load(&track, &item).await.map_err(RuntimeError::plain)?;
            self.python_exec(&format!(
                "song.tempo = {BPM:.1}\nsong.clip_trigger_quantization = 0\nsong.stop_playing()\nsong.current_song_time = 0.0\n{}\nresult = True",
                scene.setup
            ))
            .await?;
            let began = std::time::Instant::now();
            for (at, code) in &scene.actions {
                let left = at - began.elapsed().as_secs_f64();
                if left > 0.0 {
                    tokio::time::sleep(Duration::from_secs_f64(left)).await;
                }
                self.python_exec(&format!("{code}\nresult = True")).await?;
            }
            // The recorder writes 12 s from its load.
            let mut waited = 0;
            while !wav.exists() && waited < 200 {
                tokio::time::sleep(Duration::from_millis(100)).await;
                waited += 1;
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
            let kept = folder.join(format!("{}.wav", scene.name));
            std::fs::rename(&wav, &kept).map_err(|error| RuntimeError::plain(format!("{}: no recording ({error})", scene.name)))?;
            println!("  {}: {}", scene.name, kept.display());
            self.python_exec(&format!("song.stop_playing()\nsong.loop = False\nsong.tempo = {BPM:.1}\nresult = True")).await?;
            self.observe().await?;
        }
        let text = encode_folder(&folder).map_err(RuntimeError::plain)?;
        std::fs::write(out, text).map_err(|error| RuntimeError::plain(error.to_string()))?;
        println!("  written: {}", out.display());
        Ok(())
    }

    /// Live's loop, an odd meter, a jump while playing, a tempo that falls, and a stop with a note held.
    async fn scenarios(&self, stamp: u128) -> Result<(), RuntimeError> {
        let spec = json!({
            "type": "midi_effect",
            "name": format!("Timing Scenes {stamp}"),
            "about": "Kumi's timing check: 60 plus the step's index on every sixteenth.",
            "controls": [],
            "patch": concat!(
                "clock = [kumi.clock 16n]\n",
                "notes = [kumi.notes]\n",
                "clock -> [+ 60] -> notes\n",
                "clock.1 -> [* 0.5] -> notes.2\n",
                "notes -> out\n",
            ),
        });
        let made = self.call("make_device", spec).await.map_err(RuntimeError::plain)?;
        if let Some(file) = made["file"].as_str() {
            self.made.borrow_mut().push(PathBuf::from(file));
        }
        let item = made["itemId"].as_str().unwrap_or_default().to_string();
        println!("\nScenes: each note against where Live's song was");
        for length in [3.0, 3.5] {
            let setup = format!("song.loop_start = 0.0\nsong.loop_length = {length}\nsong.loop = True");
            let notes = self.session_take(&format!("loop {length}"), &item, &setup, &[], 12.0, stamp).await?;
            self.followed(&format!("a loop of {length} beats, four times round"), &notes, |t| t % length, BPM);
        }
        let notes = self
            .session_take("seven eight", &item, "song.signature_numerator = 7\nsong.signature_denominator = 8", &[], 8.0, stamp)
            .await?;
        self.followed("7/8", &notes, |t| t, BPM);
        let notes =
            self.session_take("six eight", &item, "song.signature_numerator = 6\nsong.signature_denominator = 8", &[], 8.0, stamp).await?;
        self.followed("6/8", &notes, |t| t, BPM);
        // A jump while playing, to beat 8, about 1.6 s in: the steps after it follow the song from there. Live jumps on
        // its audio buffer's first sample and takes a device's MIDI per buffer, and a note due in a buffer's last
        // signal vector goes out at the next buffer's start: so the step just before the jump can go out with the
        // landing's step, playing its own pitch.
        let notes = self.session_take("jump", &item, "", &[(1.6, "song.current_song_time = 8.0".to_string())], 8.0, stamp).await?;
        let jumped_at = notes.windows(2).find(|pair| pair[1].0 == 60 && pair[0].0 != 75).map(|pair| pair[1].1);
        match jumped_at {
            Some(at) => {
                let landing = |note: &(i64, f64, f64)| (note.1 - at).abs() < 0.002;
                let before_pitch = |note: &(i64, f64, f64)| note.0 == 60 + (((note.1 / 0.25).floor()) as i64).rem_euclid(16);
                let before: Vec<(i64, f64, f64)> = notes
                    .iter()
                    .copied()
                    .filter(|note| note.1 < at - 0.002 || (landing(note) && note.0 != 60 && before_pitch(note)))
                    .collect();
                let after: Vec<(i64, f64, f64)> = notes.iter().copied().filter(|note| !before.contains(note)).collect();
                let together = before.iter().filter(|note| landing(note)).count();
                if together > 0 {
                    println!("  the step before the jump went out with the landing's (due in the last vector of Live's buffer)");
                }
                self.followed("before the jump", &before, |t| t, BPM);
                self.followed("after the jump", &after, |t| t - at + 8.0, BPM);
            }
            None => {
                let all: Vec<String> = notes.iter().map(|(pitch, start, _)| format!("{pitch}@{start:.3}")).collect();
                println!("  a jump: no step 1 after the jump: {}", all.join(" "));
                self.fail("a jump");
            }
        }
        // A tempo that falls from 160 to 100 BPM over 4 s while it plays: Live's clip keeps beats.
        let ramp: Vec<(f64, String)> =
            (1..=40).map(|tick| (tick as f64 * 0.1, format!("song.tempo = {:.2}", 160.0 - 1.5 * tick as f64))).collect();
        let notes = self.session_take("ramp", &item, "song.tempo = 160.0", &ramp, 12.0, stamp).await?;
        self.followed("a tempo falling from 160 to 100", &notes, |t| t, 160.0);
        // Tempos typed in while it plays: halved, quadrupled, and down and up again.
        let typed: Vec<(f64, String)> = [(1.0, 60.0), (2.5, 240.0), (3.5, 70.0), (4.5, 140.0), (5.5, 20.0)]
            .iter()
            .map(|(at, bpm)| (*at, format!("song.tempo = {bpm:.1}")))
            .collect();
        let notes = self.session_take("typed", &item, "", &typed, 13.0, stamp).await?;
        self.followed("tempos typed in: 120, 60, 240, 70, 140, 20", &notes, |t| t, 240.0);
        // Stopped mid-bar, then Space: Live plays again from its insert marker, the first note that step's, on time.
        for (label, stop, between) in [("restart", 3.185, ""), ("restart at six", 3.0, "song.current_song_time = 6.0")] {
            let (from, notes) = self.restart_take(label, &item, stop, between, stamp).await?;
            let first = notes.first().copied();
            let on_time = first.is_some_and(|(pitch, start, _)| {
                let line = (from / 0.25).ceil() * 0.25;
                (start - line).abs() * 60_000.0 / BPM <= ON_GRID_MS && pitch == 60 + ((line / 0.25).round() as i64).rem_euclid(16)
            });
            println!(
                "  stopped {stop} s in{}, then Space: Live played from {from:.4}; first notes {}",
                if between.is_empty() { String::new() } else { format!(" ({between})") },
                notes.iter().take(4).map(|(pitch, start, _)| format!("{pitch}@{start:.4}")).collect::<Vec<_>>().join(" ")
            );
            let label = format!("{label}: the first step on time");
            if !on_time {
                self.fail(&label);
            }
            self.followed(&label, &notes, |t| t, BPM);
        }
        Ok(())
    }

    async fn undo_all(&self) {
        let records = self.records.borrow().clone();
        for record in records.iter().rev().filter(|record| record.state == ChangeState::Applied) {
            match self.integration.undo(Some(&record.id), signal()).await {
                Ok(after) => println!("  {:?} · {}", after.state, record.title),
                Err(error) => println!("  FAIL {}: {}", record.title, head(&error.message(), 200)),
            }
        }
    }
}

fn signal() -> Signal {
    abort::timeout(120_000)
}

/// A scene the recorder plays Live through: Python run at the start (from the downbeat, stopped, at 120 BPM) and at
/// its seconds after.
struct Scene {
    name: &'static str,
    what: &'static str,
    looped: Option<(f64, f64)>,
    setup: String,
    actions: Vec<(f64, String)>,
}

fn scenes() -> Vec<Scene> {
    let at = |list: &[(f64, &str)]| list.iter().map(|(at, code)| (*at, code.to_string())).collect::<Vec<_>>();
    let ramp = (1..=40).map(|step| (5.0 + step as f64 * 0.1, format!("song.tempo = {:.2}", 160.0 - 1.5 * step as f64)));
    let looped = |length: f64| format!("song.loop_start = 0.0\nsong.loop_length = {length}\nsong.loop = True\nsong.start_playing()");
    vec![
        Scene {
            name: "restart",
            what: "played from the start, stopped 3.185 s in, played again (from the start) and stopped",
            looped: None,
            setup: "song.start_playing()".into(),
            actions: at(&[(3.185, "song.stop_playing()"), (4.5, "song.start_playing()"), (7.0, "song.stop_playing()")]),
        },
        Scene {
            name: "restart-six",
            what: "stopped 3 s in and moved to beat 6 while stopped, then played again (from the start)",
            looped: None,
            setup: "song.start_playing()".into(),
            actions: at(&[
                (3.0, "song.stop_playing()"),
                (3.3, "song.current_song_time = 6.0"),
                (4.5, "song.start_playing()"),
                (7.0, "song.stop_playing()"),
            ]),
        },
        Scene {
            name: "loop-3.5",
            what: "Live's loop of 3.5 beats from the downbeat",
            looped: Some((0.0, 3.5)),
            setup: looped(3.5),
            actions: Vec::new(),
        },
        Scene {
            name: "loop-3",
            what: "Live's loop of 3 beats from the downbeat",
            looped: Some((0.0, 3.0)),
            setup: looped(3.0),
            actions: Vec::new(),
        },
        Scene {
            name: "jump",
            what: "jumps while playing: to beat 8, to 2.37 and back to the downbeat",
            looped: None,
            setup: "song.start_playing()".into(),
            actions: at(&[
                (1.6, "song.current_song_time = 8.0"),
                (4.0, "song.current_song_time = 2.37"),
                (6.0, "song.current_song_time = 0.0"),
            ]),
        },
        Scene {
            name: "tempo-big",
            what: "tempos typed in while playing: 120, 60, 120, 70, 140, 20, 999, 20, 120",
            looped: None,
            setup: "song.start_playing()".into(),
            actions: at(&[
                (1.5, "song.tempo = 60.0"),
                (3.0, "song.tempo = 120.0"),
                (4.5, "song.tempo = 70.0"),
                (5.5, "song.tempo = 140.0"),
                (6.5, "song.tempo = 20.0"),
                (8.0, "song.tempo = 999.0"),
                (9.0, "song.tempo = 20.0"),
                (10.5, "song.tempo = 120.0"),
            ]),
        },
        Scene {
            name: "continue-mid",
            what: "stopped mid-beat, moved while stopped, and played on (from where it stopped), twice",
            looped: None,
            setup: "song.start_playing()".into(),
            actions: at(&[
                (1.0, "song.stop_playing()"),
                (1.5, "song.current_song_time = 2.37"),
                (2.5, "song.continue_playing()"),
                (5.0, "song.stop_playing()"),
                (5.5, "song.current_song_time = 6.0"),
                (6.5, "song.continue_playing()"),
            ]),
        },
        Scene {
            name: "tempo",
            what: "120 to 59 and 160 typed in, then a ramp from 160 to 100 in steps of a tenth of a second",
            looped: None,
            setup: "song.start_playing()".into(),
            actions: at(&[(2.0, "song.tempo = 59.0"), (3.5, "song.tempo = 160.0")]).into_iter().chain(ramp).collect(),
        },
    ]
}

/// A device that writes down, at every sample for 12 s from its load, plugphasor~ and plugsync~'s position (each as a
/// float32 and what float32 left of it), play flag and tempo, then saves them to `wav`.
fn recorder(wav: &Path, stamp: u128) -> Result<Value, RuntimeError> {
    let path = wav.to_string_lossy();
    if path.contains(char::is_whitespace) {
        return Err(RuntimeError::plain(format!("the recording's folder has a space in it: {path}")));
    }
    let code = concat!(
        "Buffer b(\"kumi.transport.recording\");\n",
        "History n(0);\n",
        "k = n;\n",
        "if (k < dim(b)) {\n",
        "  poke(b, in1, k, 0);\n",
        "  phase = peek(b, k, 0);\n",
        "  poke(b, in1 - phase, k, 1);\n",
        "  poke(b, in2, k, 2);\n",
        "  heard = peek(b, k, 2);\n",
        "  poke(b, in2 - heard, k, 3);\n",
        "  poke(b, in3, k, 4);\n",
        "  poke(b, in4, k, 5);\n",
        "  k = k + 1;\n",
        "}\n",
        "n = k;\n",
        "out1 = k >= dim(b);\n",
    );
    let patch = format!(
        concat!(
            "beat = [plugphasor~]\nsync = [plugsync~]\nheard = [sig~]\nflag = [sig~]\nbpm = [sig~]\n",
            "sync.6 -> heard\nsync -> flag\nsync.5 -> bpm\n",
            "memory = [buffer~ kumi.transport.recording 12000 6]\n",
            "write = [gen~] {{\n{code}}}\n",
            "beat -> write\nheard -> write.1\nflag -> write.2\nbpm -> write.3\n",
            "tick = [metro 100 @active 1]\ndone = [snapshot~]\ntick -> done\nwrite -> done\n",
            "save = (samptype float32, writewave {path})\n",
            "done -> [change] -> [sel 1] -> save\nsave -> memory\n",
        ),
        code = code,
        path = path
    );
    Ok(json!({
        "type": "midi_effect",
        "name": format!("Transport Recorder {stamp}"),
        "about": "Writes down Live's transport at every sample for Kumi's timing tests.",
        "controls": [],
        "patch": patch,
    }))
}

/// A recording: the sample rate, and at each sample the phase, plugsync~'s position, its play flag and its tempo.
fn read_recording(path: &Path) -> Result<(f64, Vec<[f64; 4]>), String> {
    let data = std::fs::read(path).map_err(|error| format!("{}: {error}", path.display()))?;
    let (mut at, mut channels, mut rate) = (12, 0, 0.0);
    while at + 8 <= data.len() {
        let size = u32::from_le_bytes(data[at + 4..at + 8].try_into().unwrap()) as usize;
        let body = data.get(at + 8..at + 8 + size).ok_or_else(|| format!("{}: cut short", path.display()))?;
        match &data[at..at + 4] {
            b"fmt " => {
                channels = u16::from_le_bytes(body[2..4].try_into().unwrap()) as usize;
                rate = u32::from_le_bytes(body[4..8].try_into().unwrap()) as f64;
            }
            b"data" if channels == 6 => {
                let floats: Vec<f64> = body.chunks_exact(4).map(|bytes| f32::from_le_bytes(bytes.try_into().unwrap()) as f64).collect();
                return Ok((rate, floats.chunks_exact(6).map(|c| [c[0] + c[1], c[2] + c[3], c[4], c[5]]).collect()));
            }
            _ => {}
        }
        at += 8 + size + (size & 1);
    }
    Err(format!("{}: not a recording of 6 channels", path.display()))
}

/// The scenes' recordings in `folder`, as the clock's replay test reads them: plugphasor~ as runs (from, value, step
/// per sample, the value as it is on the run's first sample and from there on its fraction; a step of 0 holds the value),
/// and plugsync~'s position, play flag and tempo on each vector they aren't where the vector before puts them.
fn encode_folder(folder: &Path) -> Result<String, String> {
    const VECTOR: usize = 64;
    let fraction = |x: f64| x - x.floor();
    let mut written = Vec::new();
    let mut sample_rate = 0.0;
    for scene in scenes() {
        let (rate, samples) = read_recording(&folder.join(format!("{}.wav", scene.name)))?;
        sample_rate = rate;
        let mut runs = Vec::new();
        let mut from = 0;
        while from < samples.len() {
            let value = samples[from][0];
            let step = samples.get(from + 1).map_or(0.0, |next| {
                let step = next[0] - value;
                step - step.round()
            });
            // plugphasor~ can read 1 on a beat's line: a run's first sample is its value as it is.
            let at = |k: usize| if step == 0.0 || k == 0 { value } else { fraction(value + k as f64 * step) };
            let mut to = from + 1;
            while to < samples.len() && (at(to - from) - samples[to][0]).abs() < 1e-9 {
                to += 1;
            }
            runs.push(serde_json::to_string(&json!([from, value, step])).unwrap());
            from = to;
        }
        let mut sync = Vec::new();
        let (mut heard, mut flag, mut bpm) = (0.0, 0.0, 0.0);
        for (vector, block) in samples.chunks(VECTOR).enumerate() {
            let [_, said, playing, tempo] = block[0];
            if block.iter().any(|sample| sample[1] != said || sample[2] != playing || sample[3] != tempo) {
                return Err(format!("{}: plugsync~ changed inside vector {vector}", scene.name));
            }
            if flag > 0.5 {
                heard += VECTOR as f64 * bpm / 60.0 / rate;
            }
            if (said - heard).abs() > 1e-9 || playing != flag || tempo != bpm {
                sync.push(serde_json::to_string(&json!([vector, said, playing, tempo])).unwrap());
                (heard, flag, bpm) = (said, playing, tempo);
            }
        }
        written.push(format!(
            "  {{\n   \"name\": {},\n   \"what\": {},\n   \"loop\": {},\n   \"samples\": {},\n   \"phase\": [\n    {}\n   ],\n   \"sync\": [\n    {}\n   ]\n  }}",
            json!(scene.name),
            json!(scene.what),
            scene.looped.map_or(Value::Null, |(start, length)| json!([start, length])),
            samples.len(),
            runs.join(",\n    "),
            sync.join(",\n    ")
        ));
    }
    Ok(format!(
        "{{\n \"about\": {},\n \"rate\": {sample_rate},\n \"vector\": {VECTOR},\n \"scenes\": [\n{}\n ]\n}}\n",
        json!("Live's transport as a Max for Live device reads it at every sample, recorded in Live 12.4 by device_timing_live --record. phase: plugphasor~ as runs of [from sample, value, step per sample]: the value as it is on the run's first sample, then the fraction of value plus step times samples since (a step of 0 holds the value). sync: plugsync~'s [vector, song position in beats, play flag, tempo] on each signal vector where they aren't what the vector before puts them (the same, the position on by a vector at the tempo while playing)."),
        written.join(",\n")
    ))
}
