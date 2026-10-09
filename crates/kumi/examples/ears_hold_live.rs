//! Opt-in check on real Live that Kumi Ears passes the sound on while it records, holds a candidate back only while it
//! records one Kumi compares on its own, and lets it pass again by itself when nothing ends the recording (as after a
//! crash). It adds a Drift track playing a melody, loads a copy of this Kumi's device made for the check ("Kumi Ears
//! check", beside the installed one, which stays as it is), plays the Set and reads the track's output meter while the
//! device records: passing, held back, then held back with no one to end it. What the device recorded is heard each
//! time. Then it takes everything back. No model and no sign-in. It works in the open Set (it never opens or closes
//! one): name it, and use a disposable one. It plays about 15 seconds of sound.
//!   cargo build --release -p ableton-mcp-server --bins
//!   cargo run --release -p kumi --example ears_hold_live -- --set "<Set name>"
use futures::FutureExt;
use kumi::config::find_bridge_config;
use kumi_common::{
    abort::{self, Signal},
    js::string::{head, trim},
    time::now_ms,
};
use kumi_runtime::{
    audio::analyze::{analyze_file, AnalyzeOptions},
    core::contracts::ChangeState,
    create_ableton_integration,
    ears::{
        capture::{read_capture, write_capture_wav},
        device::{ears_file, EARS_VERSION},
        link::{open_ears_link, EarsLink, EarsOptions, Tap},
    },
    integrations::ableton::{connection::Connect, samples::user_library, AbletonOptions},
    mcp::client,
    system::Env,
    ChangeRecord, Integration, JsonObject, KernelTool, Observation, RuntimeError, BRIDGE_TOOLS,
};
use serde_json::{json, Value};
use std::{
    cell::RefCell,
    path::{Path, PathBuf},
    rc::Rc,
    time::Duration,
};

/// The check's copy of the device, by its name in the User Library's Kumi folder.
const CHECK: &str = "Kumi Ears check";
/// How long Live's meter takes to fall from full to nothing, in milliseconds.
const SETTLE_MS: u64 = 2500;

fn main() {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let value = |flag: &str| argv.iter().position(|arg| arg == flag).and_then(|at| argv.get(at + 1)).map(|value| trim(value).to_owned());
    let Some(wanted) = value("--set").filter(|name| !name.is_empty()) else {
        eprintln!("Name the open Set, as Live shows it: cargo run --release -p kumi --example ears_hold_live -- --set \"<Set name>\"");
        eprintln!("It adds a track, plays the Set for about 15 seconds and undoes it all, so use a disposable Set.");
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
    std::process::exit(local.block_on(&runtime, ears_hold_live(wanted, bridge_config, bridge)));
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

async fn ears_hold_live(wanted: String, bridge_config: String, bridge: PathBuf) -> i32 {
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
    let run =
        Run { integration: create_ableton_integration(options), observation: RefCell::default(), records, passed: RefCell::default() };
    // The check's own copy of the device: the installed Kumi Ears (another Kumi's, maybe) stays as it is.
    let device = PathBuf::from(user_library(None, None)).join("Kumi").join("Audio Effects").join(format!("{CHECK}.amxd"));
    let written = device.parent().map(std::fs::create_dir_all).transpose().and_then(|_| std::fs::write(&device, ears_file()));
    let code = match written {
        Err(error) => {
            eprintln!("ears-hold-live: couldn't write {}: {error}", device.display());
            1
        }
        Ok(()) => match run.check(&wanted).await {
            Ok(()) => {
                let passed = run.passed.borrow();
                let good = passed.iter().filter(|ok| **ok).count();
                println!("\n{good} of {} passed.", passed.len());
                i32::from(good != passed.len())
            }
            Err(error) => {
                eprintln!("ears-hold-live: {}", head(&error.message(), 300));
                1
            }
        },
    };
    let _ = run.call("play", json!({"action": "stop"})).await;
    println!("\nUndo, newest first");
    run.undo_all().await;
    let _ = run.integration.close().await;
    let _ = std::fs::remove_file(&device);
    code
}

impl Run {
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
        let tool: Option<Rc<dyn KernelTool>> =
            self.observation.borrow().as_ref().and_then(|seen| seen.tools.iter().find(|tool| tool.name() == name).cloned());
        let Some(tool) = tool else { return Err(format!("{name} isn't offered for this Set")) };
        let input = match input {
            Value::Object(input) => input,
            _ => JsonObject::new(),
        };
        match tool.execute(input, signal()).await {
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

    /// Python's answer in Live, for an expression.
    async fn python(&self, code: &str) -> Result<Value, RuntimeError> {
        let done = self.call("run_python", json!({"code": code, "mode": "eval"})).await.map_err(RuntimeError::plain)?;
        let done = if done.get("ok").is_some() { done } else { done["live"].clone() };
        if done["ok"] != true {
            return Err(RuntimeError::plain(format!("Live's Python: {}", head(&done.to_string(), 300))));
        }
        Ok(done["result"].clone())
    }

    /// The loudest the track's output meter reads over `seconds` (read every 100 ms).
    async fn loudest(&self, name: &str, seconds: f64) -> f64 {
        let code = format!("[t for t in song.tracks if t.name == {}][0].output_meter_level", json!(name));
        let mut loudest = 0_f64;
        let began = std::time::Instant::now();
        while began.elapsed().as_secs_f64() < seconds {
            if let Ok(level) = self.python(&code).await {
                loudest = loudest.max(level.as_f64().unwrap_or(0.));
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        loudest
    }

    /// A MIDI track playing a melody through Drift for sixteen bars in the Arrangement from its start.
    async fn part(&self, name: &str) -> Result<(), RuntimeError> {
        let added = self
            .call("add_tracks_and_scenes", json!({"tracks": [{"name": name, "kind": "midi"}], "scenes": []}))
            .await
            .map_err(RuntimeError::plain)?;
        let track = added["live"]["created"].as_array().and_then(|created| created.iter().find(|item| item["kind"] == "track").cloned());
        let track = track.ok_or_else(|| RuntimeError::plain("the new track didn't come back"))?;
        let rows: Vec<Value> = (0..8)
            .map(|step| json!({"pitch": 72 + [0, 2, 4, 7, 9, 7, 4, 2][step], "start": step * 2, "duration": 1.75, "velocity": 110}))
            .collect();
        self.call("write_midi_clip", json!({"trackRef": track["ref"], "sceneIndex": 0, "name": name, "length": 16, "notes": rows}))
            .await
            .map_err(RuntimeError::plain)?;
        let found = self.call("live_browser_search", json!({"category": "instruments", "query": "Drift", "limit": 1})).await;
        let item = found.ok().map(|found| found["live"]["items"][0].clone()).filter(|item| !item.is_null());
        let item = item.ok_or_else(|| RuntimeError::plain("Drift isn't in Live's browser"))?;
        self.call("load_device", json!({"itemId": item["id"], "trackRef": track["ref"]})).await.map_err(RuntimeError::plain)?;
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

    /// The check's device loaded at the end of the track, once Live's browser lists it (a new file takes a moment).
    async fn load_check(&self, name: &str) -> Result<(), RuntimeError> {
        let item = format!("user_library/Kumi/Audio Effects/{CHECK}");
        let began = std::time::Instant::now();
        loop {
            let track = self.track(name).await?;
            match self.call("load_device", json!({"itemId": item, "trackRef": track["ref"]})).await {
                Ok(_) => return Ok(()),
                Err(why) if began.elapsed() < Duration::from_secs(30) => {
                    println!("  (not loaded yet: {})", head(&why, 120));
                    tokio::time::sleep(Duration::from_secs(2)).await;
                    self.observe().await?;
                }
                Err(why) => return Err(RuntimeError::plain(format!("Live didn't load {CHECK}: {why}"))),
            }
        }
    }

    /// One recording: armed (held back when `quiet`), the track's output meter read meanwhile, then written. The
    /// meter's loudest while it records, and the take's peak in dBFS.
    async fn record(&self, link: &Rc<dyn EarsLink>, tap: &Tap, name: &str, seconds: f64, quiet: bool) -> Result<(f64, f64), RuntimeError> {
        link.arm(tap, seconds, quiet, None).await.map_err(|error| RuntimeError::plain(error.to_string()))?;
        // The device's level moves at once; Live's meter takes a couple of seconds to fall from full.
        tokio::time::sleep(Duration::from_millis(if quiet { SETTLE_MS } else { 600 })).await;
        let level = self.loudest(name, 1.).await;
        let folder = std::env::temp_dir().join(format!("kumi-ears-check-{}", now_ms()));
        std::fs::create_dir_all(&folder).map_err(|error| RuntimeError::plain(error.to_string()))?;
        let raw = folder.join("take.raw");
        let written = link.write(tap, &raw.to_string_lossy(), None).await.map_err(|error| RuntimeError::plain(error.to_string()))?;
        let capture =
            read_capture(&raw, written.channels, written.sample_rate).await.map_err(|error| RuntimeError::plain(error.to_string()))?;
        let wav = folder.join("take.wav");
        write_capture_wav(&wav, &capture, 0., capture.frames() as f64).await.map_err(|error| RuntimeError::plain(error.to_string()))?;
        let heard = analyze_file(&wav.to_string_lossy(), AnalyzeOptions::default())
            .await
            .map_err(|error| RuntimeError::plain(error.to_string()))?;
        let _ = std::fs::remove_dir_all(&folder);
        Ok((level, heard.loudness.sample_peak_dbfs))
    }

    async fn check(&self, wanted: &str) -> Result<(), RuntimeError> {
        self.integration.start(signal()).await?;
        let observation = self.observe().await?;
        let context: Value = serde_json::from_str(&observation.context).unwrap_or(Value::Null);
        let set = context["set"]["name"].as_str().unwrap_or("?").to_owned();
        if set != wanted {
            println!("The open Set is “{set}”, not “{wanted}”; nothing was changed.");
            return Err(RuntimeError::plain("not the Set named"));
        }
        let name = format!("Kumi Lead {}", now_ms() % 10_000);
        println!("A track of its own: “{name}” (Drift, a melody over sixteen bars)");
        self.part(&name).await?;
        self.load_check(&name).await?;
        let index = self.python(&format!("[t.name for t in song.tracks].index({})", json!(name))).await?;
        let index = index.as_u64().ok_or_else(|| RuntimeError::plain("the track's place wasn't read"))?;
        let link = open_ears_link(EarsOptions::default()).await.map_err(|error| RuntimeError::plain(error.to_string()))?;
        let place = format!("live_set tracks {index} devices ");
        let tap = link
            .wait_for(Rc::new(move |tap: &Tap| tap.path.starts_with(&place)), 15_000, None)
            .await
            .ok_or_else(|| RuntimeError::plain(format!("{CHECK} on track {} didn't say hello (Kumi Ears {EARS_VERSION})", index + 1)))?;
        println!("{CHECK} {} is on track {} ({})", tap.version, index + 1, tap.path);
        let main = self.rows("main-track", json!({"fields": ["name", "mixer"]})).await;
        let main_level = main.first().map(|main| main["mixer"]["volume"].clone()).unwrap_or(Value::Null);

        println!("\nThe Set plays");
        self.call("set_transport", json!({"position": 0})).await.map_err(RuntimeError::plain)?;
        self.call("play", json!({"action": "start"})).await.map_err(RuntimeError::plain)?;
        tokio::time::sleep(Duration::from_millis(800)).await;
        let playing = self.loudest(&name, 1.).await;
        self.say(playing > 0.05, &format!("the track plays out loud before Kumi listens: its meter reads {playing:.3}"));

        let (level, peak) = self.record(&link, &tap, &name, 20., false).await?;
        self.say(level > 0.05, &format!("while Kumi records it, the sound passes on: the meter reads {level:.3}"));
        self.say(peak > -40., &format!("and Kumi heard it: the take peaks at {peak:.1} dBFS"));

        let (level, peak) = self.record(&link, &tap, &name, 20., true).await?;
        self.say(level < 0.01, &format!("a candidate only Kumi hears is held back while it records: the meter reads {level:.3}"));
        self.say(peak > -40., &format!("and Kumi still heard it: the take peaks at {peak:.1} dBFS"));
        let after = self.loudest(&name, 1.).await;
        self.say(after > 0.05, &format!("once the take is written, the sound passes again: the meter reads {after:.3}"));

        // Held back with no one to end it (Kumi gone mid-listen): the device's own time runs out after 4 s.
        link.arm(&tap, 4., true, None).await.map_err(|error| RuntimeError::plain(error.to_string()))?;
        tokio::time::sleep(Duration::from_millis(SETTLE_MS)).await;
        let held = self.loudest(&name, 1.).await;
        self.say(held < 0.01, &format!("held back for a 4 s recording: the meter reads {held:.3}"));
        tokio::time::sleep(Duration::from_millis(1100)).await;
        let freed = self.loudest(&name, 1.).await;
        self.say(freed > 0.05, &format!("its time up and nothing said, the sound passes again: the meter reads {freed:.3}"));
        link.stop(&tap);
        link.close().await;

        self.call("play", json!({"action": "stop"})).await.map_err(RuntimeError::plain)?;
        let main = self.rows("main-track", json!({"fields": ["name", "mixer"]})).await;
        let now = main.first().map(|main| main["mixer"]["volume"].clone()).unwrap_or(Value::Null);
        self.say(now == main_level && !now.is_null(), &format!("Main's fader stayed where it was: {main_level} → {now}"));
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
