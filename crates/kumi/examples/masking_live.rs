//! Opt-in check on real Live that masking reads through processing on Main (#289): it adds a "vocal" (Drift, a
//! melody) under a louder bed in the same register, puts a rack holding an EQ Eight and a Limiter with 12 dB into its
//! ceiling on Main, and judges the vocal cutting through over the mix. On every listen masking stays on the checklist
//! (heard against the mix as it comes into Main's chain, by a second Kumi Ears device first on Main), it reads within
//! 5 points of the same run with the Limiter switched off, and that device is gone from Main after each listen, and
//! after one stopped while it was first there. Then it takes everything back with Kumi's undo. No model and no
//! sign-in. It works in the open Set (it never opens or closes one): name it, and use a disposable one.
//! Refs to Main's devices read before the listens still work after them. KUMI_TIMING=1 also prints how long the
//! device's placing and taking off take.
//!   cargo build --release -p ableton-mcp-server --bins
//!   cargo run --release -p kumi --example masking_live -- --set "<Set name>"
use futures::FutureExt;
use kumi_agent::{core::contracts::ActionEvent, mcp::client, Integration, Observation};
use kumi_common::{
    abort::{self, Signal},
    js::string::{head, trim},
    time::now_ms,
};
use kumi_daw_ableton::app::folders::find_bridge_config;
use kumi_daw_ableton::{connection::Connect, create_ableton_integration, AbletonOptions, BRIDGE_TOOLS};
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
};

fn main() {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let value = |flag: &str| argv.iter().position(|arg| arg == flag).and_then(|at| argv.get(at + 1)).map(|value| trim(value).to_owned());
    let Some(wanted) = value("--set").filter(|name| !name.is_empty()) else {
        eprintln!("Name the open Set, as Live shows it: cargo run --release -p kumi --example masking_live -- --set \"<Set name>\"");
        eprintln!("It adds two tracks and a Limiter on Main, judges, and undoes it all, so use a disposable Set.");
        std::process::exit(2);
    };
    let env: Env = std::env::vars().collect();
    let Some(bridge_config) = find_bridge_config(&env) else {
        eprintln!("The Ableton bridge isn't installed; run Kumi's bridge setup first.");
        std::process::exit(2);
    };
    let bridge = bridge_program();
    if !bridge.is_file() {
        eprintln!(
            "The Ableton bridge isn't built ({}); build it first: cargo build --release -p ableton-mcp-server --bins",
            bridge.display()
        );
        std::process::exit(2);
    }
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().expect("a runtime");
    let local = tokio::task::LocalSet::new();
    std::process::exit(local.block_on(&runtime, masking_live(wanted, bridge_config, bridge)));
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
    passed: RefCell<Vec<bool>>,
}

fn bridge_options(bridge: &Path, bridge_config: &str) -> AbletonOptions {
    let mut options = AbletonOptions::new(Rc::new(|_, _| {}));
    options.bridge_config = Some(bridge_config.into());
    options.connect = Some(connect_to(bridge.into(), bridge_config.into()));
    options
}

async fn masking_live(wanted: String, bridge_config: String, bridge: PathBuf) -> i32 {
    let records = Rc::new(RefCell::new(Vec::<ChangeRecord>::new()));
    let mut options = bridge_options(&bridge, &bridge_config);
    // What Kumi says beside its progress (a listen's notes) is printed.
    options.on_action = Some(Rc::new(|event: ActionEvent| {
        if !event.title.starts_with("Listen") {
            println!("  · {}", event.title);
        }
    }));
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
    let run = Run::new(create_ableton_integration(options), records);
    // Another connection to Live, to read Main's chain while a judge call holds the first one's tools.
    let watcher = Run::new(create_ableton_integration(bridge_options(&bridge, &bridge_config)), Rc::default());
    let code = match run.check(&wanted, &watcher).await {
        Ok(()) => {
            let passed = run.passed.borrow();
            let good = passed.iter().filter(|ok| **ok).count();
            println!("\n{good} of {} passed.", passed.len());
            i32::from(good != passed.len())
        }
        Err(error) => {
            eprintln!("masking-live: {}", head(&error.message(), 300));
            1
        }
    };
    println!("\nUndo, newest first");
    run.undo_all().await;
    let _ = run.integration.close().await;
    let _ = watcher.integration.close().await;
    code
}

impl Run {
    fn new(integration: Rc<dyn Integration>, records: Rc<RefCell<Vec<ChangeRecord>>>) -> Self {
        Run { integration, observation: RefCell::new(None), records, passed: RefCell::new(vec![]) }
    }

    fn say(&self, ok: bool, what: &str) {
        self.passed.borrow_mut().push(ok);
        println!("  {}  {what}", if ok { "ok  " } else { "FAIL" });
    }

    async fn observe(&self) -> Result<Observation, RuntimeError> {
        let observation = self.integration.observe(signal(), None).await?;
        *self.observation.borrow_mut() = Some(observation.clone());
        Ok(observation)
    }

    async fn call(&self, name: &str, input: Value) -> Result<Value, String> {
        self.call_until(name, input, signal()).await
    }

    async fn call_until(&self, name: &str, input: Value, signal: Signal) -> Result<Value, String> {
        let tool: Option<Rc<dyn KernelTool>> =
            self.observation.borrow().as_ref().and_then(|seen| seen.tools.iter().find(|tool| tool.name() == name).cloned());
        let Some(tool) = tool else { return Err(format!("{name} isn't offered for this Set")) };
        let input = match input {
            Value::Object(input) => input,
            _ => JsonObject::new(),
        };
        match tool.execute(input, signal).await {
            Ok(result) if !result.is_error => Ok(serde_json::from_str(&result.text).unwrap_or(Value::String(result.text))),
            Ok(result) => Err(head(&result.text, 400)),
            Err(error) => Err(head(&error.message(), 400)),
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

    /// A MIDI track playing `notes` (pitch and velocity, a quarter note each, two beats apart) through Drift, for
    /// eight bars in the Arrangement from its start, its fader at `volume` when one is given (0.85 is 0 dB).
    async fn part(&self, name: &str, notes: &[(i64, i64)], volume: Option<f64>) -> Result<(), RuntimeError> {
        let added = self
            .call("add_tracks_and_scenes", json!({"tracks": [{"name": name, "kind": "midi"}], "scenes": []}))
            .await
            .map_err(RuntimeError::plain)?;
        let track = added["live"]["created"].as_array().and_then(|created| created.iter().find(|item| item["kind"] == "track").cloned());
        let track = track.ok_or_else(|| RuntimeError::plain("the new track didn't come back"))?;
        let rows: Vec<Value> = (0..8)
            .flat_map(|step| notes.iter().map(move |(pitch, velocity)| json!({"pitch": pitch + [0, 2, 4, 7, 9, 7, 4, 2][step], "start": step * 2, "duration": 1.75, "velocity": velocity})))
            .collect();
        self.call("write_midi_clip", json!({"trackRef": track["ref"], "sceneIndex": 0, "name": name, "length": 16, "notes": rows}))
            .await
            .map_err(RuntimeError::plain)?;
        let found = self.call("live_browser_search", json!({"category": "instruments", "query": "Drift", "limit": 1})).await;
        let item = found.ok().map(|found| found["live"]["items"][0].clone()).filter(|item| !item.is_null());
        let item = item.ok_or_else(|| RuntimeError::plain("Drift isn't in Live's browser"))?;
        self.call("load_device", json!({"itemId": item["id"], "trackRef": track["ref"]})).await.map_err(RuntimeError::plain)?;
        self.observe().await?;
        let tracks = self.rows("track", json!({"fields": ["name"]})).await;
        let track = tracks.iter().rfind(|row| row["name"] == name).cloned().ok_or_else(|| RuntimeError::plain("track gone"))?;
        let slots = self.rows("clip-slot", json!({"parent": track["ref"], "fields": ["clipRef"]})).await;
        let clip = slots.first().map(|slot| slot["clipRef"].clone()).filter(Value::is_string);
        let clip = clip.ok_or_else(|| RuntimeError::plain("the clip didn't come back"))?;
        for copy in 0..2 {
            self.call("duplicate_clip", json!({"clipRef": clip, "arrangementPosition": copy * 16})).await.map_err(RuntimeError::plain)?;
        }
        if let Some(volume) = volume {
            self.call("set_mixer", json!({"trackRef": track["ref"], "volume": volume})).await.map_err(RuntimeError::plain)?;
        }
        Ok(())
    }

    /// Main's devices now, by name.
    async fn main_devices(&self) -> Vec<String> {
        let main = self.rows("main-track", json!({"fields": ["name"]})).await;
        let Some(main) = main.first() else { return vec!["(Main not read)".into()] };
        let devices = self.rows("device", json!({"parent": main["ref"], "fields": ["name", "className"]})).await;
        devices.iter().map(|row| row["name"].as_str().unwrap_or("?").to_owned()).collect()
    }

    /// The Limiter on Main, read fresh: refs from an earlier look are retired.
    async fn main_limiter(&self) -> Result<Value, RuntimeError> {
        self.observe().await?;
        let main = self.rows("main-track", json!({"fields": ["name"]})).await;
        let main = main.first().cloned().ok_or_else(|| RuntimeError::plain("Main wasn't read"))?;
        let devices = self.rows("device", json!({"parent": main["ref"], "fields": ["name", "className"]})).await;
        let limiter = devices.iter().rfind(|row| row["className"] == "Limiter").map(|row| row["ref"].clone());
        limiter.ok_or_else(|| RuntimeError::plain("the Limiter on Main wasn't read"))
    }

    /// One knob of the Limiter on Main, set by its name and the value as Live shows it.
    async fn set_limiter(&self, names: &[&str], value: Value) -> Result<(), RuntimeError> {
        let limiter = self.main_limiter().await?;
        let mut why = String::new();
        for name in names {
            match self.call("set_device_parameter", json!({"deviceRef": limiter, "parameter": name, "value": value})).await {
                Ok(_) => return Ok(()),
                Err(error) => why = error,
            }
        }
        Err(RuntimeError::plain(why))
    }

    /// A run over the mix, the vocal cutting through, ended at once. On both its listens masking is on the checklist
    /// with no note that it can't be read through Main, and Main's chain is `chain` again after each. Masking as read
    /// at the start, when it was.
    async fn judged(&self, goal: &Value, chain: &[String]) -> Option<f64> {
        let began = std::time::Instant::now();
        let started = self.call_until("judge", goal.clone(), abort::timeout(600_000)).await;
        println!("  (its first listen took {:.1} s)", began.elapsed().as_secs_f64());
        let masked = self.heard_before_main(&started, "the first listen");
        let after = self.main_devices().await;
        self.say(after == chain, &format!("Main's chain after the first listen: {}", after.join(", ")));
        let began = std::time::Instant::now();
        let done = self.call_until("judge", json!({"done": true}), abort::timeout(600_000)).await;
        println!("  (its last listen took {:.1} s)", began.elapsed().as_secs_f64());
        self.heard_before_main(&done, "the last listen");
        let after = self.main_devices().await;
        self.say(after == chain, &format!("Main's chain after the last listen: {}", after.join(", ")));
        masked
    }

    /// A round's masking, read against the mix as it comes into Main's chain: on the checklist, and no note that it
    /// can't be read through Main. The masked share, when it was read.
    fn heard_before_main(&self, round: &Result<Value, String>, listen: &str) -> Option<f64> {
        let round = match round {
            Ok(round) => round,
            Err(why) => {
                self.say(false, &format!("{listen}: {why}"));
                return None;
            }
        };
        let text = round.to_string();
        let masked = masking(round);
        let found: Vec<&str> = round["problems"]
            .as_array()
            .map(|found| found.iter().filter_map(|problem| problem["id"].as_str()).collect())
            .unwrap_or_default();
        let found = if found.is_empty() { String::new() } else { format!(" (problems found: {})", found.join(", ")) };
        self.say(masked.is_some(), &format!("{listen}: masking is on the checklist, {masked:?}%{found}"));
        self.say(
            !text.contains("can't be read through Main's chain"),
            &format!("{listen}: no note that masking can't be read through Main"),
        );
        masked
    }

    /// The browser item Live finds first for `query` among its `category`.
    async fn item(&self, category: &str, query: &str) -> Result<Value, RuntimeError> {
        let found = self.call("live_browser_search", json!({"category": category, "query": query, "limit": 1})).await;
        let item = found.ok().map(|found| found["live"]["items"][0]["id"].clone()).filter(|item| !item.is_null());
        item.ok_or_else(|| RuntimeError::plain(format!("{query} isn't in Live's browser")))
    }

    async fn check(&self, wanted: &str, watcher: &Run) -> Result<(), RuntimeError> {
        self.integration.start(signal()).await?;
        let observation = self.observe().await?;
        let context: Value = serde_json::from_str(&observation.context).unwrap_or(Value::Null);
        let set = context["set"]["name"].as_str().unwrap_or("?").to_owned();
        if set != wanted {
            println!("The open Set is “{set}”, not “{wanted}”; nothing was changed.");
            return Err(RuntimeError::plain("not the Set named"));
        }
        watcher.integration.start(signal()).await?;
        watcher.observe().await?;
        let tag = now_ms() % 10_000;
        let (vocal, bed) = (format!("Kumi Vocal {tag}"), format!("Kumi Bed {tag}"));
        println!("Two tracks of its own: “{vocal}” (a melody, its fader down) and “{bed}” (louder chords in its register), Drift each");
        // The vocal 2.8 dB down: buried about a quarter of the time. Not all of it, so the reading can tell where it was
        // heard: Kumi's device left after the Limiter read 100 % through the limiting and 26 % with it off.
        self.part(&vocal, &[(84, 90)], Some(0.78)).await?;
        self.part(&bed, &[(83, 127), (86, 127), (88, 127), (91, 127)], None).await?;
        let before = self.main_devices().await;
        let main = self.rows("main-track", json!({"fields": ["name"]})).await;
        let main = main.first().cloned().ok_or_else(|| RuntimeError::plain("Main wasn't read"))?;
        // A rack holding an EQ Eight, then a Limiter: Live's walk of Main lists the rack's devices right after it, so a
        // row's place there isn't its place on Main's chain.
        let (rack, eq, limiter) = (
            self.item("audio_effects", "Audio Effect Rack").await?,
            self.item("audio_effects", "EQ Eight").await?,
            self.item("audio_effects", "Limiter").await?,
        );
        let steps = json!([
            {"tool": "load_device", "input": {"itemId": rack, "trackRef": main["ref"]}, "as": "rack"},
            {"tool": "edit_rack", "input": {"rackRef": "@rack", "action": "add-chain"}, "as": "chain"},
            {"tool": "load_device", "input": {"itemId": eq, "chainRef": "@chain"}},
            {"tool": "load_device", "input": {"itemId": limiter, "trackRef": main["ref"]}},
        ]);
        self.call("make_changes", json!({"steps": steps})).await.map_err(RuntimeError::plain)?;
        self.observe().await?;
        let chain = self.main_devices().await;
        println!("Main's chain: {} → {}", before.join(", "), chain.join(", "));
        // 12 dB into the ceiling: the mix after Main's chain is louder and limited, the mix as it comes in isn't.
        self.set_limiter(&["Input Gain", "Gain"], json!("12 dB")).await?;
        // The Limiter's switch, discovered as the model would before a listen: it still works after the listens.
        let limiter = self.main_limiter().await?;
        let parameters = self.rows("parameter", json!({"parent": limiter, "fields": ["name"]})).await;
        let switch = parameters.iter().find(|row| row["name"] == "Device On").map(|row| row["ref"].clone());
        let switch = switch.ok_or_else(|| RuntimeError::plain("the Limiter's Device On wasn't read"))?;
        let goal = json!({"goal": {"focus": vocal, "problems": true}, "from_beat": 0, "beats": 32});

        println!("\nJudged over the mix, the vocal cutting through, with 12 dB into the Limiter on Main");
        let limiting = self.judged(&goal, &chain).await;

        println!("\nThe same with the Limiter switched off");
        let switched = self.call("set_device_parameter", json!({"deviceRef": limiter, "parameterRef": switch, "value": 0})).await;
        self.say(
            switched.is_ok(),
            &format!("the Limiter switched off by refs read before the listens: {}", head(&format!("{switched:?}"), 160)),
        );
        if switched.is_err() {
            self.set_limiter(&["Device On"], json!(0)).await?;
        }
        let off = self.judged(&goal, &chain).await;
        match (limiting, off) {
            (Some(limiting), Some(off)) => self.say(
                (limiting - off).abs() <= 5.,
                &format!("masked {limiting}% through the limiting, {off}% with it off: within 5 points"),
            ),
            _ => self.say(false, "masking read both ways, to compare"),
        }

        println!("\nThe same, stopped once Kumi Ears is first on Main");
        let stop = Signal::new();
        let watch = async {
            let began = std::time::Instant::now();
            let mut seen = vec![];
            while !stop.is_cancelled() && began.elapsed() < std::time::Duration::from_secs(30) {
                let now = watcher.main_devices().await;
                if now.first().is_some_and(|name| name == "Kumi Ears") && now.len() > chain.len() {
                    seen = now;
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(250)).await;
            }
            stop.cancel();
            (seen, began.elapsed())
        };
        let (stopped, (seen, at)) = tokio::join!(self.call_until("judge", goal, stop.clone()), watch);
        self.say(!seen.is_empty(), &format!("stopped {:.1} s in, with Main's chain: {}", at.as_secs_f64(), seen.join(", ")));
        self.say(stopped.is_err(), &format!("the judge stopped partway: {}", head(&format!("{stopped:?}"), 200)));
        self.observe().await?;
        let after = self.main_devices().await;
        self.say(after == chain, &format!("Main's chain after the stopped listen: {}", after.join(", ")));
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

/// The masked share a round's checklist reads (its masking item's id is "masking <focus>"), when masking is on it.
fn masking(round: &Value) -> Option<f64> {
    let rows = round["checklist"].as_array()?;
    let row = rows.iter().find(|row| row["id"].as_str().is_some_and(|id| id.starts_with("masking ")))?;
    row["after"].as_f64().or_else(|| row["before"].as_f64())
}
