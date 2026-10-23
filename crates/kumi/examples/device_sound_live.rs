//! Opt-in check on real Live that the devices Kumi makes load where they belong and sound. It adds a Drift track playing
//! a melody and reads its output meter dry. Then, each made with make_device: a MIDI effect loads before Drift and the
//! melody still plays; an instrument plays on a MIDI track of its own and is refused on Drift's track (Live would replace
//! Drift); each audio effect (as code, and as a patch of Max's own objects) loads after Drift and lets the sound
//! through. Each make says whether Live's Browser listed the device before make_device answered, and each load whether
//! it had to wait for the Browser. A device file can be checked too (--file <path>.amxd): a copy goes in its kind's
//! folder. No model and no sign-in. It works in the open Set (it never opens or closes one): name it, and use a
//! disposable one. It plays about 5 seconds of sound for each device, and takes everything back, the kind folders it
//! made in the User Library included, so each run makes them as a producer's first device does.
//!   cargo build -p ableton-mcp-server -p kumi-daw-ableton --bins
//!   cargo run -p kumi --example device_sound_live -- --set "<Set name>" [--file <path>.amxd]…
use futures::FutureExt;
use kumi_agent::{mcp::client, Integration, Observation};
use kumi_common::{
    abort::{self, Signal},
    js::string::{head, trim},
    time::now_ms,
};
use kumi_daw_ableton::app::folders::find_bridge_config;
use kumi_daw_ableton::{
    connection::Connect,
    create_ableton_integration,
    devices::{amxd::decode_amxd, tool::kind_folder},
    samples::user_library,
    AbletonOptions, BRIDGE_TOOLS,
};
use kumi_types::{
    contracts::{ChangeRecord, ChangeState, JsonObject, KernelTool},
    errors::RuntimeError,
    system::Env,
};
use serde_json::{json, Value};
use std::{
    cell::RefCell,
    path::{Path, PathBuf},
    rc::Rc,
    time::{Duration, Instant},
};

/// How long Live's meter takes to rise to a new level and fall from full, in milliseconds.
const SETTLE_MS: u64 = 2500;

fn main() {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let values = |flag: &str| -> Vec<String> {
        argv.windows(2).filter(|pair| pair[0] == flag).map(|pair| trim(&pair[1]).to_owned()).filter(|value| !value.is_empty()).collect()
    };
    let Some(wanted) = values("--set").into_iter().next() else {
        eprintln!("Name the open Set, as Live shows it: cargo run -p kumi --example device_sound_live -- --set \"<Set name>\"");
        eprintln!("It adds tracks, plays the Set and undoes it all, so use a disposable Set.");
        std::process::exit(2);
    };
    let env: Env = std::env::vars().collect();
    let Some(bridge_config) = find_bridge_config(&env) else {
        eprintln!("The Ableton bridge isn't installed; run Kumi's bridge setup first.");
        std::process::exit(2);
    };
    let bridge = bridge_program();
    // Kumi's check of a MIDI effect's code runs in a program of its own, beside Kumi's (here, beside the bridge).
    let harness = bridge.with_file_name(if cfg!(windows) { "kumi-harness.exe" } else { "kumi-harness" });
    if !bridge.is_file() || !harness.is_file() {
        eprintln!(
            "The Ableton bridge or Kumi's check isn't built ({}, {}); build them first: cargo build -p ableton-mcp-server -p kumi-daw-ableton --bins",
            bridge.display(),
            harness.display()
        );
        std::process::exit(2);
    }
    if std::env::var_os("KUMI_HARNESS_BIN").is_none() {
        std::env::set_var("KUMI_HARNESS_BIN", &harness);
    }
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().expect("a runtime");
    let local = tokio::task::LocalSet::new();
    std::process::exit(local.block_on(&runtime, device_sound_live(wanted, values("--file"), bridge_config, bridge)));
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

/// A device as the model would ask for it, named for the check.
fn device(kind: &str, label: &str, stamp: u128, spec: Value) -> (String, Value) {
    let mut spec = spec;
    spec["type"] = json!(kind);
    spec["name"] = json!(format!("Sound Check {label} {stamp}"));
    spec["about"] = json!(format!("Kumi's sound check: {label}."));
    (format!("make_device {label}"), spec)
}

/// The audio effects Kumi makes for the check.
fn effects(stamp: u128) -> Vec<(String, Value)> {
    let knob =
        |name: &str, min: f64, max: f64, default: f64| json!({"name": name, "type": "number", "min": min, "max": max, "default": default});
    let effect = |label: &str, spec: Value| device("audio_effect", label, stamp, spec);
    vec![
        effect("code thru", json!({"controls": [], "code": "out1 = in1;\nout2 = in2;"})),
        effect(
            "code drive",
            json!({"controls": [knob("Drive", 1.0, 10.0, 2.0)], "code": "out1 = tanh(in1 * drive);\nout2 = tanh(in2 * drive);"}),
        ),
        effect(
            "code gain",
            json!({"controls": [knob("Gain", -24.0, 24.0, 0.0)], "code": "g = dbtoa(gain);\nout1 = in1 * g;\nout2 = in2 * g;"}),
        ),
        effect(
            "patch objects",
            json!({"controls": [knob("Gain", -24.0, 24.0, 0.0)], "patch": concat!(
                "left = [*~ 1.]\n",
                "right = [*~ 1.]\n",
                "in.0 -> left -> out.0\n",
                "in.1 -> right -> out.1\n",
                "\"Gain\" -> [dbtoa] -> both\n",
                "both = [t f f]\n",
                "both.0 -> left.1\n",
                "both.1 -> right.1\n",
            )}),
        ),
        effect(
            "patch gen",
            json!({"controls": [knob("Drive", 1.0, 10.0, 2.0)], "patch": concat!(
                "fx = [gen~] {\n",
                "  Param drive(2);\n",
                "  out1 = tanh(in1 * drive);\n",
                "  out2 = tanh(in2 * drive);\n",
                "}\n",
                "in.0 -> fx\n",
                "in.1 -> fx.1\n",
                "fx -> out.0\n",
                "fx.1 -> out.1\n",
                "\"Drive\" -> [prepend drive] -> fx\n",
            )}),
        ),
    ]
}

/// A device make_device made: its Browser item, and whether the Browser listed it before make_device answered.
struct Made {
    item: String,
    listed: bool,
}

struct Run {
    integration: Rc<dyn Integration>,
    observation: RefCell<Option<Observation>>,
    records: Rc<RefCell<Vec<ChangeRecord>>>,
    made: RefCell<Vec<PathBuf>>,
    failed: RefCell<Vec<String>>,
    /// The kind folders the User Library didn't have before the run.
    fresh: RefCell<Vec<PathBuf>>,
}

async fn device_sound_live(wanted: String, files: Vec<String>, bridge_config: String, bridge: PathBuf) -> i32 {
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
        fresh: RefCell::default(),
    };
    let code = match run.check(&wanted, &files).await {
        Ok(()) => {
            let failed = run.failed.borrow();
            println!("\n{} failed{}", failed.len(), if failed.is_empty() { String::new() } else { format!(": {}", failed.join("; ")) });
            i32::from(!failed.is_empty())
        }
        Err(error) => {
            eprintln!("device-sound-live: {}", head(&error.message(), 300));
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
    // A kind folder the run made goes too, once empty (remove_dir leaves one that holds anything).
    for folder in run.fresh.borrow().iter() {
        let _ = std::fs::remove_dir(folder);
    }
    code
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

    /// Python's answer in Live, for an expression.
    async fn python(&self, code: &str) -> Result<Value, RuntimeError> {
        let done = self.call("run_python", json!({"code": code, "mode": "eval"})).await.map_err(RuntimeError::plain)?;
        let done = if done.get("ok").is_some() { done } else { done["live"].clone() };
        if done["ok"] != true {
            return Err(RuntimeError::plain(format!("Live's Python: {}", head(&done.to_string(), 300))));
        }
        Ok(done["result"].clone())
    }

    /// The loudest the track's output meter reads over 1.5 s, from the Arrangement's start once the meter settles.
    async fn level(&self, name: &str) -> Result<f64, RuntimeError> {
        self.call("set_transport", json!({"position": 0})).await.map_err(RuntimeError::plain)?;
        tokio::time::sleep(Duration::from_millis(SETTLE_MS)).await;
        let code = format!("[t for t in song.tracks if t.name == {}][0].output_meter_level", json!(name));
        let mut loudest = 0_f64;
        let began = Instant::now();
        while began.elapsed().as_secs_f64() < 1.5 {
            if let Ok(level) = self.python(&code).await {
                loudest = loudest.max(level.as_f64().unwrap_or(0.));
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        Ok(loudest)
    }

    /// The devices on the track, in order: name, class, on.
    async fn devices(&self, name: &str) -> Value {
        let code =
            format!("[[d.name, d.class_name, d.is_active] for d in [t for t in song.tracks if t.name == {}][0].devices]", json!(name));
        self.python(&code).await.unwrap_or(Value::Null)
    }

    /// A MIDI track playing a melody for sixteen bars in the Arrangement from its start, through Drift when `drift`.
    async fn part(&self, name: &str, drift: bool) -> Result<(), RuntimeError> {
        let added = self
            .call("add_tracks_and_scenes", json!({"tracks": [{"name": name, "kind": "midi"}], "scenes": []}))
            .await
            .map_err(RuntimeError::plain)?;
        let track = added["live"]["created"].as_array().and_then(|created| created.iter().find(|item| item["kind"] == "track").cloned());
        let track = track.ok_or_else(|| RuntimeError::plain("the new track didn't come back"))?;
        let rows: Vec<Value> = (0..8)
            .map(|step| json!({"pitch": 60 + [0, 2, 4, 7, 9, 7, 4, 2][step], "start": step * 2, "duration": 1.75, "velocity": 110}))
            .collect();
        self.call("write_midi_clip", json!({"trackRef": track["ref"], "sceneIndex": 0, "name": name, "length": 16, "notes": rows}))
            .await
            .map_err(RuntimeError::plain)?;
        if drift {
            let found = self.call("live_browser_search", json!({"category": "instruments", "query": "Drift", "limit": 1})).await;
            let item = found.ok().map(|found| found["live"]["items"][0].clone()).filter(|item| !item.is_null());
            let item = item.ok_or_else(|| RuntimeError::plain("Drift isn't in Live's browser"))?;
            self.call("load_device", json!({"itemId": item["id"], "trackRef": track["ref"]})).await.map_err(RuntimeError::plain)?;
        }
        self.observe().await?;
        let track = self.track(name).await?;
        let slots = self.rows("clip-slot", json!({"parent": track["ref"], "fields": ["clipRef"]})).await;
        let clip = slots.first().map(|slot| slot["clipRef"].clone()).filter(Value::is_string);
        let clip = clip.ok_or_else(|| RuntimeError::plain("the clip didn't come back"))?;
        for copy in 0..4 {
            self.call("duplicate_clip", json!({"clipRef": clip, "arrangementPosition": copy * 16})).await.map_err(RuntimeError::plain)?;
        }
        Ok(())
    }

    async fn track(&self, name: &str) -> Result<Value, RuntimeError> {
        let tracks = self.rows("track", json!({"fields": ["name"]})).await;
        tracks.iter().rfind(|row| row["name"] == name).cloned().ok_or_else(|| RuntimeError::plain("the track is gone"))
    }

    /// A device made with make_device, its findings said, and whether Live's Browser listed it before make_device
    /// answered (the `browser` key says it didn't).
    async fn make(&self, spec: Value) -> Option<Made> {
        let began = Instant::now();
        match self.call("make_device", spec).await {
            Ok(made) => {
                for key in ["wrong", "changed", "notes"] {
                    if let Some(said) = made.get(key) {
                        println!("    make_device {key}: {said}");
                    }
                }
                if let Some(file) = made["file"].as_str() {
                    self.made.borrow_mut().push(PathBuf::from(file));
                }
                let listed = made.get("browser").is_none();
                let browser = made.get("browser").map_or("absent".to_string(), Value::to_string);
                println!("    made {} in {:.1} s; browser key: {browser}", made["itemId"], began.elapsed().as_secs_f64());
                Some(Made { item: made["itemId"].as_str().unwrap_or_default().to_string(), listed })
            }
            Err(why) => {
                println!("    make_device refused: {}", head(&why, 300));
                None
            }
        }
    }

    /// The device loaded at the end of the track, at once or (while Live's Browser catches up) after up to 30 s of
    /// tries: the changes the load made, to take it back.
    async fn load(&self, name: &str, item: &str) -> Result<Vec<String>, String> {
        let began = Instant::now();
        let mut tries = 0;
        loop {
            let track = self.track(name).await.map_err(|error| error.message())?;
            tries += 1;
            let before: Vec<String> = self.records.borrow().iter().map(|record| record.id.clone()).collect();
            match self.call("load_device", json!({"itemId": item, "trackRef": track["ref"]})).await {
                Ok(_) => {
                    println!("    loaded on {name} {}", if tries == 1 { "at once".to_string() } else { format!("after {tries} tries") });
                    let made = self
                        .records
                        .borrow()
                        .iter()
                        .filter(|record| !before.contains(&record.id))
                        .map(|record| record.id.clone())
                        .collect();
                    return Ok(made);
                }
                Err(why) if began.elapsed() < Duration::from_secs(30) && !why.contains("already has an instrument") => {
                    println!("    (not loaded yet: {})", head(&why, 160));
                    tokio::time::sleep(Duration::from_secs(2)).await;
                    self.observe().await.map_err(|error| error.message())?;
                }
                Err(why) => return Err(why),
            }
        }
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

    /// The changes a load made, taken back, newest first.
    async fn take_back(&self, changes: &[String]) {
        for id in changes.iter().rev() {
            if let Err(error) = self.integration.undo(Some(id), signal()).await {
                println!("    couldn't take it away: {}", head(&error.message(), 200));
            }
        }
    }

    /// One effect loaded on the Drift track and heard against Drift alone, then taken away.
    async fn effect(&self, label: &str, track: &str, item: &str, dry: f64) -> Result<(), RuntimeError> {
        let changes = match self.load(track, item).await {
            Ok(changes) => changes,
            Err(why) => {
                println!("    FAIL  not loaded: {}", head(&why, 400));
                self.fail(label);
                return Ok(());
            }
        };
        let level = self.level(track).await?;
        let heard = level > dry * 0.1;
        if !heard {
            self.fail(label);
        }
        println!("    {}  the meter reads {level:.3} (dry {dry:.3})", if heard { "ok  " } else { "SILENT" });
        println!("    devices: {}", self.devices(track).await);
        self.take_back(&changes).await;
        self.observe().await?;
        Ok(())
    }

    async fn check(&self, wanted: &str, files: &[String]) -> Result<(), RuntimeError> {
        self.integration.start(signal()).await?;
        let observation = self.observe().await?;
        let context: Value = serde_json::from_str(&observation.context).unwrap_or(Value::Null);
        let set = context["set"]["name"].as_str().unwrap_or("?").to_owned();
        if set != wanted {
            println!("The open Set is “{set}”, not “{wanted}”; nothing was changed.");
            return Err(RuntimeError::plain("not the Set named"));
        }
        let library = PathBuf::from(user_library(None, None)).join("Kumi");
        let folders = ["MIDI Effects", "Audio Effects", "Instruments"].map(|folder| library.join(folder));
        self.fresh.borrow_mut().extend(folders.iter().filter(|folder| !folder.is_dir()).cloned());
        let there: Vec<String> = folders.iter().map(|folder| format!("{}: {}", folder.display(), folder.is_dir())).collect();
        println!("Kumi's folders before the run: {}", there.join(", "));
        let stamp = now_ms() % 10_000;
        let drift = format!("Kumi Sound {stamp}");
        println!("A track of its own: “{drift}” (Drift, a melody over sixteen bars)");
        self.part(&drift, true).await?;
        self.call("play", json!({"action": "start"})).await.map_err(RuntimeError::plain)?;
        let dry = self.level(&drift).await?;
        println!("dry (Drift alone): the meter reads {dry:.3}");
        if dry < 0.05 {
            return Err(RuntimeError::plain("Drift alone plays nothing here, so nothing can be checked"));
        }

        // A MIDI effect goes before Drift, and the melody still plays through it.
        let (label, spec) = device(
            "midi_effect",
            "midi thru",
            stamp as u128,
            json!({"controls": [], "code": "function midi(event) { pass(event); }", "tests": []}),
        );
        println!("\n{label}");
        match self.make(spec).await {
            Some(made) => {
                if !made.listed {
                    self.fail(&format!("{label}: Live's Browser hadn't listed it when make_device answered"));
                }
                self.effect(&label, &drift, &made.item, dry).await?;
            }
            None => self.fail(&label),
        }

        // An instrument plays on a MIDI track of its own, and isn't loaded over Drift.
        let voice = "History level(0);\nlevel = mix(level, velocity > 0, 0.002);\nout1 = cycle(mtof(note)) * level * 0.3;\nout2 = out1;";
        let (label, spec) = device("instrument", "synth", stamp as u128, json!({"controls": [], "code": voice, "voices": 4}));
        println!("\n{label}");
        match self.make(spec).await {
            Some(made) => {
                if !made.listed {
                    self.fail(&format!("{label}: Live's Browser hadn't listed it when make_device answered"));
                }
                let own = format!("Kumi Synth {stamp}");
                self.part(&own, false).await?;
                match self.load(&own, &made.item).await {
                    Ok(_) => {
                        let level = self.level(&own).await?;
                        let heard = level > 0.05;
                        if !heard {
                            self.fail(&format!("{label} on its own track"));
                        }
                        println!("    {}  on its own track the meter reads {level:.3}", if heard { "ok  " } else { "SILENT" });
                        println!("    devices: {}", self.devices(&own).await);
                    }
                    Err(why) => {
                        println!("    FAIL  not loaded on its own track: {}", head(&why, 400));
                        self.fail(&format!("{label} on its own track"));
                    }
                }
                match self.load(&drift, &made.item).await {
                    Ok(changes) => {
                        println!("    FAIL  loaded over Drift: {}", self.devices(&drift).await);
                        self.fail(&format!("{label} over Drift"));
                        self.take_back(&changes).await;
                    }
                    Err(why) => println!("    ok    refused on Drift's track: {why}"),
                }
                self.observe().await?;
            }
            None => self.fail(&label),
        }

        // Audio effects go after Drift and let the sound through.
        for (label, spec) in effects(stamp as u128) {
            println!("\n{label}");
            match self.make(spec).await {
                Some(made) => {
                    if !made.listed {
                        self.fail(&format!("{label}: Live's Browser hadn't listed it when make_device answered"));
                    }
                    self.effect(&label, &drift, &made.item, dry).await?;
                }
                None => self.fail(&label),
            }
        }
        for (at, file) in files.iter().enumerate() {
            let label = format!("file {file}");
            println!("\n{label}");
            match self.copy(Path::new(file), &format!("Sound Check {stamp} {}", at + 1)) {
                Ok(item) => self.effect(&label, &drift, &item, dry).await?,
                Err(why) => {
                    println!("    {why}");
                    self.fail(&label);
                }
            }
        }
        self.call("play", json!({"action": "stop"})).await.map_err(RuntimeError::plain)?;
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
