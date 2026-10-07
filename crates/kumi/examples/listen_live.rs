//! Opt-in check on real Live that quiet listens come through on their first pass at the tempos producers use
//! (#252), and that a whole song is heard in one listen taking about its length. It adds a track of its own
//! (Drift playing a note on every beat, through the Arrangement), listens to it and to the mix at each tempo,
//! then takes it all back with Kumi's undo. No model and no sign-in. Run it on a disposable Set:
//!   cargo build --release -p ableton-mcp-server --bins
//!   KUMI_TIMING=1 cargo run --release -p kumi --example listen_live -- --set "<Set name>" [--bars 8] [--song-bars 96]
use futures::FutureExt;
use kumi::config::find_bridge_config;
use kumi_common::{
    abort::{self, Signal},
    js::{
        number::{round, to_fixed, to_string},
        string::{head, trim},
    },
    time::{now_ms, perf_now},
};
use kumi_runtime::{
    audio::AnalyzeOptions,
    core::contracts::{ChangeState, HearRequest},
    create_ableton_integration, hear,
    integrations::ableton::{connection::Connect, AbletonOptions},
    mcp::client,
    system::Env,
    ChangeRecord, Integration, JsonObject, KernelTool, Observation, RuntimeError, BRIDGE_TOOLS,
};
use serde_json::{json, Value};
use std::{
    cell::RefCell,
    path::{Path, PathBuf},
    rc::Rc,
};

/// The tempos of #252: where beats land on Max's signal vectors at 44.1 kHz (and 48), and the ones around them.
const TEMPOS: [f64; 11] = [90.0, 100.0, 120.0, 125.0, 126.0, 128.0, 130.0, 135.0, 140.0, 145.0, 150.0];

fn main() {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let value = |flag: &str| argv.iter().position(|arg| arg == flag).and_then(|at| argv.get(at + 1)).map(|value| trim(value).to_owned());
    let Some(wanted) = value("--set").filter(|name| !name.is_empty()) else {
        eprintln!("Name the Set to use, as Live shows it: cargo run --release -p kumi --example listen_live -- --set \"<Set name>\"");
        eprintln!("It adds a track, listens, and undoes it all, so use a disposable Set.");
        std::process::exit(2);
    };
    let bars = value("--bars").and_then(|bars| bars.parse::<f64>().ok()).unwrap_or(8.0);
    let song_bars = value("--song-bars").and_then(|bars| bars.parse::<f64>().ok()).unwrap_or(96.0);
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
    std::process::exit(local.block_on(&runtime, listen_live(wanted, bars, song_bars, bridge_config, bridge)));
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

async fn listen_live(wanted: String, bars: f64, song_bars: f64, bridge_config: String, bridge: PathBuf) -> i32 {
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
        Run { integration: create_ableton_integration(options), observation: RefCell::new(None), records, passed: RefCell::new(vec![]) };
    let code = match run.check(&wanted, bars, song_bars).await {
        Ok(()) => {
            let passed = run.passed.borrow();
            let good = passed.iter().filter(|ok| **ok).count();
            println!("\n{good} of {} passed.", passed.len());
            i32::from(good != passed.len())
        }
        Err(error) => {
            eprintln!("listen-live: {}", head(&error.message(), 300));
            1
        }
    };
    println!("\nUndo, newest first");
    run.undo_all().await;
    let _ = run.integration.close().await;
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
            Ok(result) if !result.is_error => Ok(serde_json::from_str(&result.text).unwrap_or(Value::Null)),
            Ok(result) => Err(head(&result.text, 300)),
            Err(error) => Err(head(&error.message(), 300)),
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

    async fn check(&self, wanted: &str, bars: f64, song_bars: f64) -> Result<(), RuntimeError> {
        self.integration.start(signal()).await?;
        let observation = self.observe().await?;
        let context: Value = serde_json::from_str(&observation.context).unwrap_or(Value::Null);
        let set = context["set"]["name"].as_str().unwrap_or("?").to_owned();
        if set != wanted {
            println!("The open Set is “{set}”, not “{wanted}”; nothing was changed.");
            return Err(RuntimeError::plain("not the Set named"));
        }
        let name = format!("Kumi Listen {}", now_ms() % 10_000);
        println!("A track of its own: “{name}”, Drift on every beat for {} bars", to_string(song_bars));
        let added = self
            .call("add_tracks_and_scenes", json!({"tracks": [{"name": name, "kind": "midi"}], "scenes": []}))
            .await
            .map_err(RuntimeError::plain)?;
        let track = added["live"]["created"].as_array().and_then(|created| created.iter().find(|item| item["kind"] == "track").cloned());
        let track = track.ok_or_else(|| RuntimeError::plain("the new track didn't come back"))?;
        // Four bars of a low note and a fifth on every beat, so every beat sounds.
        let notes: Vec<Value> = (0..16)
            .flat_map(|beat| [48, 55].map(|pitch| json!({"pitch": pitch, "start": beat, "duration": 0.5, "velocity": 100})))
            .collect();
        self.call("write_midi_clip", json!({"trackRef": track["ref"], "sceneIndex": 0, "name": "Pulse", "length": 16, "notes": notes}))
            .await
            .map_err(RuntimeError::plain)?;
        let found = self.call("live_browser_search", json!({"category": "instruments", "query": "Drift", "limit": 1})).await;
        let item = found.ok().map(|found| found["live"]["items"][0].clone()).filter(|item| !item.is_null());
        let item = item.ok_or_else(|| RuntimeError::plain("Drift isn't in Live's browser"))?;
        self.call("load_device", json!({"itemId": item["id"], "trackRef": track["ref"]})).await.map_err(RuntimeError::plain)?;
        self.observe().await?;
        let tracks = self.rows("track", json!({"fields": ["name"]})).await;
        let track = tracks.iter().rfind(|row| row["name"] == name.as_str()).cloned().ok_or_else(|| RuntimeError::plain("track gone"))?;
        let slots = self.rows("clip-slot", json!({"parent": track["ref"], "fields": ["clipRef"]})).await;
        let clip = slots.first().map(|slot| slot["clipRef"].clone()).filter(Value::is_string);
        let clip = clip.ok_or_else(|| RuntimeError::plain("the clip didn't come back"))?;
        let copies = (song_bars / 4.0).ceil() as usize;
        for copy in 0..copies {
            self.call("duplicate_clip", json!({"clipRef": clip, "arrangementPosition": copy * 16})).await.map_err(RuntimeError::plain)?;
        }
        println!("\nQuiet listens of {} bars from bar 2, the track and the mix", to_string(bars));
        for tempo in TEMPOS {
            self.call("set_tempo", json!({"tempo": tempo})).await.map_err(RuntimeError::plain)?;
            for mix in [false, true] {
                let request = HearRequest {
                    tracks: if mix { vec![] } else { vec![name.clone()] },
                    mix: mix.then_some(true),
                    from_beat: Some(4.0),
                    beats: Some(bars * 4.0),
                    ..Default::default()
                };
                // One pass plays a bar ahead, the part and half a bar after; a second would play it all again.
                let part = bars * 4.0 * 60.0 / tempo;
                let played = (1.0 + bars + 0.5) * 4.0 * 60.0 / tempo;
                let what = format!("{} BPM, {}", to_string(tempo), if mix { "the mix" } else { "the track" });
                self.listen(&request, &what, part, played, played + 6.0 + part * 0.5).await;
            }
        }
        println!("\nThe whole song, at 128 BPM");
        self.call("set_tempo", json!({"tempo": 128.0})).await.map_err(RuntimeError::plain)?;
        let song = song_bars * 4.0 * 60.0 / 128.0;
        let played = song + 1.5 * 4.0 * 60.0 / 128.0;
        let request = HearRequest { mix: Some(true), whole: Some(true), ..Default::default() };
        self.listen(&request, "the whole song, the mix", song, played, played * 1.1 + 10.0).await;
        Ok(())
    }

    /// One listen: it must come back with the whole part, not silent, within `allowed` seconds (one pass, where a
    /// second would take as long again).
    async fn listen(&self, request: &HearRequest, what: &str, seconds: f64, played: f64, allowed: f64) {
        let t0 = perf_now();
        let heard = self.integration.hear(request, abort::timeout(3_600_000)).await;
        let took = (perf_now() - t0) / 1000.0;
        let takes = match heard {
            Ok(Ok(takes)) => takes,
            Ok(Err(message)) => return self.say(false, &format!("{what}: {} ({} s)", head(&message, 300), to_fixed(took, 1))),
            Err(error) => return self.say(false, &format!("{what}: {} ({} s)", head(&error.message(), 300), to_fixed(took, 1))),
        };
        let Some(take) = takes.first() else { return self.say(false, &format!("{what}: no take")) };
        let options = AnalyzeOptions { start: Some(take.start), seconds: take.seconds, signal: Some(signal()), ..Default::default() };
        let analysis = hear(&take.file, options).await;
        let (lufs, length) = match &analysis {
            Ok(analysis) => (analysis.loudness.integrated_lufs, analysis.seconds - take.start),
            Err(_) => (None, 0.0),
        };
        let whole = length >= seconds - 0.05 && take.note.is_none();
        let sounded = lufs.is_some_and(|lufs| lufs > -50.0);
        let once = took <= allowed;
        let lufs = lufs.map(|lufs| format!("{} LUFS", to_string(round(lufs * 10.0) / 10.0))).unwrap_or_else(|| "silent".into());
        let note = take.note.as_ref().map(|note| format!(" · {note}")).unwrap_or_default();
        self.say(
            whole && sounded && once,
            &format!(
                "{what}: {} s heard of {} s, {lufs}, in {} s (one pass plays {} s){note}",
                to_fixed(length, 1),
                to_fixed(seconds, 1),
                to_fixed(took, 1),
                to_fixed(played, 1)
            ),
        );
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
