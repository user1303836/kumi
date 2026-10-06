//! Opt-in acceptance run on real Live: every kind of change Kumi can make, each undone through
//! Kumi's undo, on the Set you name; Kumi 1.0's playing, bouncing, listening and watching; and how
//! long the reads a big Set depends on take. It changes the open Set (and plays it briefly), then
//! puts it back, so run it on a disposable copy. No model and no sign-in. It starts the bridge from
//! the same build, so build that first:
//!   cargo build --release -p ableton-mcp-server --bins
//!   cargo run --release -p kumi --example accept_live -- --set "Kumi Focus Demo"
use async_trait::async_trait;
use futures::FutureExt;
use kumi::config::find_bridge_config;
use kumi_common::{
    abort::{self, Signal},
    js::{
        json::{byte_length, stringify},
        number::{round, to_fixed, to_string},
        string::{head, pad_start, trim},
    },
    time::{now_ms, perf_now},
};
use kumi_runtime::{
    audio::AnalyzeOptions,
    core::contracts::ChangeState,
    create_ableton_integration, create_project_store, hear,
    integrations::ableton::{connection::Connect, AbletonOptions},
    mcp::client,
    system::Env,
    Baseline, ChangeRecord, Integration, JsonObject, KernelTool, Observation, ProjectStore, RuntimeError, BRIDGE_TOOLS, KUMI,
};
use regex::Regex;
use serde_json::{json, Value};
use std::{
    cell::{Cell, RefCell},
    collections::HashSet,
    path::{Path, PathBuf},
    rc::Rc,
    sync::LazyLock,
    time::Duration,
};

/// Where the Arrangement changes go (beat 16, bar 5): inside any Set, since Live refuses a spot past its end.
const SPOT: i64 = 16;
static SPACES: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\s+").unwrap());

fn main() {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let wanted = argv.iter().position(|arg| arg == "--set").and_then(|at| argv.get(at + 1)).map(|name| trim(name).to_owned());
    let Some(wanted) = wanted.filter(|name| !name.is_empty()) else {
        eprintln!("Name the Set to change, as Live shows it: cargo run --release -p kumi --example accept_live -- --set \"<Set name>\"");
        eprintln!("It changes that Set and undoes every change, so use a disposable copy.");
        std::process::exit(2);
    };
    let env: Env = std::env::vars().collect();
    let Some(bridge_config) = find_bridge_config(&env) else {
        eprintln!("The Ableton bridge isn't installed; run: {} doctor", *KUMI);
        std::process::exit(2);
    };
    let (bridge, build) = bridge_program();
    if !bridge.is_file() {
        eprintln!("The Ableton bridge isn't built ({}); build it first: {build}", bridge.display());
        std::process::exit(2);
    }
    // Cargo doesn't rebuild a dependency's binaries for an example, so an older build could answer instead.
    let reported = std::process::Command::new(&bridge).arg("--version").output().ok();
    let reported = reported.map(|output| String::from_utf8_lossy(&output.stdout).trim().to_owned()).unwrap_or_default();
    let expected = format!("ableton-mcp-server {}", ableton_mcp_server::delivery::PACKAGE_VERSION);
    if reported != expected {
        eprintln!("The Ableton bridge at {} says \"{reported}\", not \"{expected}\"; rebuild it: {build}", bridge.display());
        std::process::exit(2);
    }
    let runtime = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
        Ok(runtime) => runtime,
        Err(error) => {
            eprintln!("accept-live: {error}");
            std::process::exit(1);
        }
    };
    let local = tokio::task::LocalSet::new();
    let code = local.block_on(&runtime, accept_live(wanted, bridge_config, bridge));
    std::process::exit(code);
}

/// The bridge this run starts, and the command that builds it. Kumi starts the `ableton-mcp-server`
/// beside its own binary; Cargo puts an example one folder further down, in `examples`.
fn bridge_program() -> (PathBuf, String) {
    let name = if cfg!(windows) { "ableton-mcp-server.exe" } else { "ableton-mcp-server" };
    let mut folder = std::env::current_exe().ok().and_then(|exe| exe.parent().map(Path::to_path_buf)).unwrap_or_default();
    if folder.file_name().is_some_and(|name| name == "examples") {
        folder.pop();
    }
    let profile = match folder.file_name().and_then(|name| name.to_str()) {
        Some("release") => " --release".to_owned(),
        Some(name) if name != "debug" => format!(" --profile {name}"),
        _ => String::new(),
    };
    (folder.join(name), format!("cargo build{profile} -p ableton-mcp-server --bins"))
}

/// Kumi's own connection to the bridge (its configuration and tools), started from `bridge`.
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

async fn accept_live(wanted: String, bridge_config: String, bridge: PathBuf) -> i32 {
    let stopping = Rc::new(Cell::new(false));
    tokio::task::spawn_local({
        let stopping = stopping.clone();
        async move {
            if tokio::signal::ctrl_c().await.is_ok() {
                stopping.set(true);
                println!("\nStopping: undoing what was changed so far…");
                // Only the first Ctrl-C is caught: another one quits at once.
                if tokio::signal::ctrl_c().await.is_ok() {
                    std::process::exit(130);
                }
            }
        }
    });
    // Catch-up runs against a throwaway folder, so ~/.kumi isn't touched; its first save is the Set's snapshot export.
    let scratch = match tempfile::Builder::new().prefix("kumi-accept-").tempdir() {
        Ok(scratch) => scratch,
        Err(error) => {
            eprintln!("accept-live: {error}");
            return 1;
        }
    };
    let looked = Rc::new(Cell::new(0.0));
    let exported = Rc::new(Cell::new(None));
    let records = Rc::new(RefCell::new(Vec::<ChangeRecord>::new()));
    let mut options = AbletonOptions::new(Rc::new(|_, _| {}));
    options.bridge_config = Some(bridge_config.clone());
    options.connect = Some(connect_to(bridge, bridge_config));
    options.project_store =
        Some(Rc::new(TimedStore { store: create_project_store(scratch.path()), looked: looked.clone(), exported: exported.clone() }));
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
    // Its own tracks, named for this run: an earlier run's may still be in the Set.
    let run = run_id();
    let accept = Accept {
        integration: create_ableton_integration(options),
        observation: RefCell::new(None),
        rows: RefCell::new(Vec::new()),
        records,
        expected_kept: RefCell::new(HashSet::new()),
        stopping,
        looked,
        exported,
        pad: format!("Kumi Pad {run}"),
        bounce: format!("Kumi Bounce {run}"),
    };
    let code = accept.accept(&wanted).await.unwrap_or_else(|error| {
        eprintln!("accept-live: {}", head(&error.message(), 300));
        1
    });
    let _ = accept.integration.close().await;
    let _ = scratch.close();
    code
}

/// `Date.now().toString(36).slice(-4)`.
fn run_id() -> String {
    let mut number = now_ms().unsigned_abs();
    let mut digits = Vec::new();
    while number > 0 && digits.len() < 4 {
        digits.push(char::from_digit((number % 36) as u32, 36).unwrap());
        number /= 36;
    }
    digits.iter().rev().collect()
}

#[derive(Clone, Copy)]
struct Exported {
    ms: f64,
    pages: usize,
    bytes: usize,
}

/// The catch-up store, noting when the Set's snapshot export first arrives (its first save).
struct TimedStore {
    store: Rc<dyn ProjectStore>,
    looked: Rc<Cell<f64>>,
    exported: Rc<Cell<Option<Exported>>>,
}
#[async_trait(?Send)]
impl ProjectStore for TimedStore {
    async fn load(&self, project: &str) -> Result<Option<Baseline>, RuntimeError> {
        self.store.load(project).await
    }
    async fn save(&self, project: &str, baseline: &Baseline) -> Result<(), RuntimeError> {
        if self.exported.get().is_none() {
            let bytes = byte_length(&json!(baseline.pages));
            self.exported.set(Some(Exported { ms: perf_now() - self.looked.get(), pages: baseline.pages.len(), bytes }));
        }
        self.store.save(project, baseline).await
    }
}

/// A tool call's outcome: its reply read as JSON (null when it isn't) and, when it failed, why.
struct Outcome {
    ok: bool,
    ms: f64,
    body: Value,
    error: Option<String>,
}

/// Every row of one kind, page by page.
struct Listing {
    items: Vec<Value>,
    pages: u32,
    ms: f64,
    error: Option<String>,
}

/// The Set as the run compares it, before and after.
struct State {
    tempo: Option<Value>,
    tracks: Vec<Value>,
    scenes: usize,
    locators: Vec<String>,
    locator_positions: Vec<f64>,
    track_read: Listing,
}

/// One run: the latest look at the Set, each check's outcome, and Kumi's changes as HISTORY records them.
struct Accept {
    integration: Rc<dyn Integration>,
    observation: RefCell<Option<Observation>>,
    rows: RefCell<Vec<bool>>,
    records: Rc<RefCell<Vec<ChangeRecord>>>,
    /// Changes the run itself took away afterwards (a clip it cleared and deleted): their own undo finds nothing, as it should.
    expected_kept: RefCell<HashSet<String>>,
    stopping: Rc<Cell<bool>>,
    looked: Rc<Cell<f64>>,
    exported: Rc<Cell<Option<Exported>>>,
    pad: String,
    bounce: String,
}

impl Accept {
    fn say(&self, ok: bool, ms: Option<f64>, what: &str) {
        self.rows.borrow_mut().push(ok);
        println!("  {} {}  {what}", if ok { "ok  " } else { "FAIL" }, pad_start(&ms.map(seconds).unwrap_or_default(), 8, ' '));
    }

    async fn observe(&self) -> Result<Observation, RuntimeError> {
        let observation = self.integration.observe(signal(), None).await?;
        *self.observation.borrow_mut() = Some(observation.clone());
        Ok(observation)
    }

    fn tool(&self, name: &str) -> Option<Rc<dyn KernelTool>> {
        self.observation.borrow().as_ref()?.tools.iter().find(|tool| tool.name() == name).cloned()
    }

    async fn run(&self, name: &str, input: Value) -> Outcome {
        let Some(found) = self.tool(name) else {
            return Outcome { ok: false, ms: 0.0, body: Value::Null, error: Some("not offered for this Set".into()) };
        };
        let input = match input {
            Value::Object(input) => input,
            _ => JsonObject::new(),
        };
        let t0 = perf_now();
        match found.execute(input, signal()).await {
            Ok(result) => Outcome {
                ok: !result.is_error,
                ms: perf_now() - t0,
                body: serde_json::from_str(&result.text).unwrap_or(Value::Null),
                error: result.is_error.then(|| head(&SPACES.replace_all(&result.text, " "), 240)),
            },
            Err(error) => Outcome { ok: false, ms: perf_now() - t0, body: Value::Null, error: Some(head(&error.message(), 240)) },
        }
    }

    /// Every row of one kind, page by page.
    async fn all(&self, kind: &str, extra: Value) -> Listing {
        let mut listing = Listing { items: Vec::new(), pages: 0, ms: 0.0, error: None };
        let mut cursor = Value::Null;
        loop {
            let mut input = JsonObject::new();
            input.insert("kind".into(), json!(kind));
            input.insert("limit".into(), json!(100));
            input.extend(extra.as_object().cloned().unwrap_or_default());
            if truthy(&cursor) {
                input.insert("cursor".into(), cursor.clone());
            }
            let read = self.run("live_discover", Value::Object(input)).await;
            listing.ms += read.ms;
            listing.pages += 1;
            if !read.ok {
                listing.error = read.error;
                return listing;
            }
            let content = content_of(&read.body);
            listing.items.extend(content["items"].as_array().into_iter().flatten().cloned());
            cursor = content["nextCursor"].clone();
            if !truthy(&cursor) || listing.pages >= 64 {
                return listing;
            }
        }
    }

    /// The Set's tracks, with their names.
    async fn tracks(&self) -> Vec<Value> {
        self.all("track", json!({"fields": ["name"]})).await.items
    }

    /// Every row of one kind inside `parent`; none without a parent.
    async fn under(&self, kind: &str, parent: Option<&Value>, fields: &[&str]) -> Vec<Value> {
        match parent {
            Some(parent) => self.all(kind, json!({"parent": parent["ref"], "fields": fields})).await.items,
            None => Vec::new(),
        }
    }

    async fn state(&self) -> State {
        let set = self.all("set", json!({"fields": ["tempo"]})).await;
        let tracks = self.all("track", json!({"fields": ["name"]})).await;
        let scenes = self.all("scene", json!({"fields": ["name"]})).await;
        let locators = self.all("locator", json!({"fields": ["name", "position"]})).await;
        State {
            tempo: set.items.first().and_then(|row| row.get("tempo")).cloned(),
            tracks: tracks.items.iter().map(|row| row["name"].clone()).collect(),
            scenes: scenes.items.len(),
            locators: locators
                .items
                .iter()
                .map(|row| format!("{}@{}", js_string(row.get("name")), js_string(row.get("position"))))
                .collect(),
            locator_positions: locators.items.iter().filter_map(|row| row["position"].as_f64()).collect(),
            track_read: tracks,
        }
    }

    async fn change(&self, name: &str, input: Value) -> Option<Outcome> {
        if self.stopping.get() {
            return None;
        }
        let outcome = self.run(name, input).await;
        let untitled = format!("{name}: {}", outcome.error.as_deref().unwrap_or("no title"));
        self.say(outcome.ok, Some(outcome.ms), &text_or(&outcome.body["changed"], &untitled));
        Some(outcome)
    }

    async fn accept(&self, wanted: &str) -> Result<i32, RuntimeError> {
        println!("Reads");
        let t0 = perf_now();
        self.integration.start(signal()).await?;
        self.say(true, Some(perf_now() - t0), "connect to the bridge and Live");
        let t0 = perf_now();
        self.looked.set(t0);
        let observation = self.observe().await?;
        let context: Value = serde_json::from_str(&observation.context).map_err(|error| RuntimeError::plain(error.to_string()))?;
        let tools = observation.tools.len();
        let set = &context["set"]["name"];
        let live =
            if truthy(&context["liveVersion"]) { format!(", Live {}", js_string(Some(&context["liveVersion"]))) } else { String::new() };
        self.say(tools > 0, Some(perf_now() - t0), &format!("first look at the Set: “{}”{live}, {tools} tools", text_or(set, "?")));
        if set.as_str() != Some(wanted) {
            println!("\nThe open Set is “{}”, not “{wanted}”; nothing was changed.", text_or(set, "unknown"));
            return Ok(2);
        }
        let before = self.state().await;
        let read = &before.track_read;
        let error = read.error.as_deref().filter(|error| !error.is_empty());
        let failure = error.map(|error| format!(": {error}")).unwrap_or_default();
        let pages = if read.pages == 1 { "page" } else { "pages" };
        self.say(error.is_none(), Some(read.ms), &format!("{} tracks in {} {pages}{failure}", before.tracks.len(), read.pages));
        let mut waited = 0;
        while self.exported.get().is_none() && waited < 60_000 {
            tokio::time::sleep(Duration::from_millis(250)).await;
            waited += 250;
        }
        match self.exported.get() {
            Some(exported) => {
                let pages = if exported.pages == 1 { "page" } else { "pages" };
                let kb = to_string(round(exported.bytes as f64 / 1024.0));
                let what =
                    format!("Set snapshot for catching up: {} {pages}, {kb} KB (ready this long after the first look)", exported.pages);
                self.say(true, Some(exported.ms), &what);
            }
            None => self.say(false, None, "no Set snapshot for catching up within 60 s (an unsaved Set, or one too big for the bridge)"),
        }

        println!("\nChanges");
        // A panic stops the changes like an error does, so the undo below still runs.
        match std::panic::AssertUnwindSafe(self.changes(&before)).catch_unwind().await {
            Ok(Ok(())) => {}
            Ok(Err(error)) => self.say(false, None, &format!("stopped making changes: {}", head(&error.message(), 200))),
            Err(_) => self.say(false, None, "stopped making changes: a panic (see above)"),
        }

        // Whatever stopped the changes, what was changed goes back.
        println!("\nUndo, newest first");
        self.undo_all().await;

        println!();
        if let Err(error) = self.check(&before).await {
            self.say(false, None, &format!("couldn't read the Set afterwards to check it: {}", head(&error.message(), 200)));
        }
        let rows = self.rows.borrow();
        let passed = rows.iter().filter(|ok| **ok).count();
        let stopping = self.stopping.get();
        println!("\n{passed} of {} passed{}.", rows.len(), if stopping { " (stopped early)" } else { "" });
        Ok(if passed == rows.len() && !stopping { 0 } else { 1 })
    }

    async fn changes(&self, before: &State) -> Result<(), RuntimeError> {
        let Some(target) = before.track_read.items.first() else {
            self.say(false, None, "the Set has no track to change");
            return Ok(());
        };
        let tempo = if before.tempo.as_ref().and_then(Value::as_f64) == Some(124.0) { 125 } else { 124 };
        self.change("set_tempo", json!({"tempo": tempo})).await;
        self.change("set_mixer", json!({"trackRef": target["ref"], "volume": 0.7, "pan": -0.25})).await;
        let renamed = format!("{} (Kumi)", head(&js_string(target.get("name")), 110));
        self.change("rename", json!({"kind": "track", "ref": target["ref"], "name": renamed})).await;
        self.change("set_track_color", json!({"ref": target["ref"], "colorIndex": 12})).await;
        // Live keeps the playhead, loop and locators inside the Set's arrangement, so they go near its start, clear of the
        // Set's own locators.
        let taken = |beat: i64| before.locator_positions.contains(&(beat as f64));
        let mut start = SPOT;
        while taken(start) || taken(start + 16) {
            start += 1;
        }
        self.change("set_locators", json!({"start": start, "end": start + 16, "startName": "Kumi Start", "endName": "Kumi End"})).await;
        let added = self.change("add_tracks_and_scenes", json!({"tracks": [{"name": self.pad, "kind": "midi"}], "scenes": []})).await;
        let pad = added.and_then(|added| added.body["live"]["created"].as_array()?.iter().find(|item| item["kind"] == "track").cloned());
        let Some(pad) = pad.filter(|_| !self.stopping.get()) else { return Ok(()) };
        let notes: Vec<Value> =
            [60, 64, 67].iter().map(|pitch| json!({"pitch": pitch, "start": 0, "duration": 4, "velocity": 96})).collect();
        self.change("write_midi_clip", json!({"trackRef": pad["ref"], "sceneIndex": 0, "name": "Kumi Chord", "length": 4, "notes": notes}))
            .await;
        let found = self.run("live_browser_search", json!({"category": "instruments", "query": "Drift", "limit": 1})).await;
        let item = &content_of(&found.body)["items"][0];
        let loaded = if truthy(item) {
            self.change("load_device", json!({"itemId": item["id"], "trackRef": pad["ref"]})).await
        } else {
            self.say(false, Some(found.ms), "Drift not found in the browser");
            None
        };
        if loaded.is_some_and(|loaded| loaded.ok) && !self.stopping.get() {
            // A device brings its parameter tools; a new look retires earlier references, so find the pad again.
            self.observe().await?;
            let device = self.under("device", last_named(&self.tracks().await, &self.pad), &["name"]).await.into_iter().next();
            let parameters = self.under("parameter", device.as_ref(), &["name", "value", "min", "max", "displayValue"]).await;
            let knob = parameters.iter().find(|row| row["name"] == "LP Freq" || row["name"] == "Filter Freq").or_else(|| {
                parameters.iter().find(|row| {
                    matches!((row["max"].as_f64(), row["min"].as_f64()), (Some(max), Some(min)) if max > min) && row["name"] != "Device On"
                })
            });
            match (knob, &device) {
                (Some(knob), Some(device)) => {
                    let (min, max) = (knob["min"].as_f64().unwrap_or(f64::NAN), knob["max"].as_f64().unwrap_or(f64::NAN));
                    let value = min + (max - min) * 0.3;
                    self.change("set_device_parameter", json!({"deviceRef": device["ref"], "parameterRef": knob["ref"], "value": value}))
                        .await;
                }
                _ => self.say(false, None, "no Drift parameter to change"),
            }
        }
        if !self.stopping.get() {
            self.accept_one_dot_zero(target).await?;
        }
        if !self.stopping.get() {
            println!("\nFull control");
            self.accept_full_control().await?;
        }
        Ok(())
    }

    /// Kumi 1.0: the song and scenes, clips and notes, a device off and on, a bounce heard back, playing and showing, watching.
    async fn accept_one_dot_zero(&self, target: &Value) -> Result<(), RuntimeError> {
        let skip = |name: &str| println!("  skip           {name} isn't offered by this bridge");
        let maybe = move |name: &'static str, input: Value| async move {
            if self.tool(name).is_some() {
                self.change(name, input).await
            } else {
                skip(name);
                None
            }
        };
        let fresh = move || async move {
            self.observe().await?;
            Ok::<_, RuntimeError>(self.tracks().await)
        };
        println!("\n1.0: the song, clips, notes and devices");
        let tracks = fresh().await?;
        maybe("set_transport", json!({"loopEnabled": true, "loopStart": SPOT, "loopLength": 16})).await;
        maybe("set_song", json!({"swingAmount": 0.2})).await;
        if let Some(scene) = self.all("scene", json!({"fields": ["name"]})).await.items.first() {
            maybe("set_scene", json!({"ref": scene["ref"], "tempo": 121, "tempoEnabled": true})).await;
        }
        let slots = self.under("clip-slot", last_named(&tracks, &self.pad), &["clipRef"]).await;
        if let Some(slot) = slots.iter().find(|row| truthy(&row["clipRef"])) {
            let clip = &slot["clipRef"];
            maybe("set_clip", json!({"clipRef": clip, "looping": true, "loopStart": 0, "loopEnd": 4})).await;
            let notes = self.all("note", json!({"parent": clip})).await.items;
            if let Some(note) = notes.first() {
                maybe("change_notes", json!({"clipRef": clip, "notes": [{"id": note["id"], "velocity": 80}]})).await;
            }
            maybe("transform_midi", json!({"clipRef": clip, "transform": "transpose", "params": {"semitones": 2}, "scope": "in-place"}))
                .await;
            // The chord goes into the Arrangement too, where the bounce below records it from.
            maybe("duplicate_clip", json!({"clipRef": clip, "arrangementPosition": SPOT})).await;
        }
        let tracks = fresh().await?;
        let pad_now = last_named(&tracks, &self.pad);
        if let Some(drift) = self.under("device", pad_now, &["name"]).await.first() {
            maybe("switch_device", json!({"deviceRef": drift["ref"], "enabled": false})).await;
            maybe("switch_device", json!({"deviceRef": drift["ref"], "enabled": true})).await;
        }
        if self.stopping.get() || pad_now.is_none() {
            return Ok(());
        }

        println!("\n1.0: a bounce, heard back");
        if self.tool("record").is_none() || self.tool("set_routing").is_none() {
            skip("record");
        } else {
            let steps = json!([
                {
                    "tool": "add_tracks_and_scenes",
                    "input": {"tracks": [{"name": self.bounce, "kind": "audio"}], "scenes": []},
                    "as": "bounce"
                },
                {
                    "tool": "set_routing",
                    "input": {"trackRef": "@bounce", "inputType": self.pad, "inputSubRouting": "Post FX", "arm": true, "monitoring": "off"}
                },
                {"tool": "set_transport", "input": {"position": SPOT, "loopEnabled": false}},
                {"tool": "record", "input": {"action": "start", "lane": "arrangement", "destinationTrackRef": "@bounce"}},
                {"tool": "play", "input": {"action": "continue"}},
                {"tool": "wait", "input": {"beats": 4}},
                {"tool": "play", "input": {"action": "stop"}},
                {"tool": "record", "input": {"action": "stop", "lane": "arrangement"}},
                {"tool": "set_routing", "input": {"trackRef": "@bounce", "arm": false}},
            ]);
            let plan = self.run("make_changes", json!({"steps": steps})).await;
            let stopped = &plan.body["stopped"];
            let what = if plan.ok {
                let done = plan.body["done"].as_array().map_or_else(|| "?".to_owned(), |done| done.len().to_string());
                format!("resampled the pad onto a new audio track, in {done} steps")
            } else if truthy(stopped) {
                let (step, tool) = (js_string(stopped.get("step")), js_string(stopped.get("tool")));
                format!("resampling: stopped at step {step} ({tool}): {}", head(&stringify(stopped), 400))
            } else {
                format!("resampling: {}", plan.error.as_deref().unwrap_or_default())
            };
            self.say(plan.ok, Some(plan.ms), &what);
            if plan.ok {
                let tracks = fresh().await?;
                let clips = self.under("arrangement-clip", last_named(&tracks, &self.bounce), &["isAudio", "length"]).await;
                let clip = clips.iter().find(|row| truthy(&row["isAudio"]));
                let t0 = perf_now();
                let file = match clip {
                    Some(clip) => self.integration.audio_file(clip["ref"].as_str().unwrap_or_default(), signal()).await,
                    None => Ok(None),
                };
                let heard = match file {
                    Ok(Some(file)) if !file.is_empty() => hear(&file, listening()).await.map(Some).map_err(|error| error.to_string()),
                    Ok(_) => Ok(None),
                    Err(error) => Err(error.message()),
                };
                match heard {
                    Ok(Some(heard)) => {
                        let lufs = heard.loudness.integrated_lufs;
                        let key = heard.key.as_ref().map(|key| format!(", {}", key.name)).unwrap_or_default();
                        let what = format!(
                            "heard the bounce: {} s at {} LUFS{key}",
                            to_string(round(heard.seconds * 10.0) / 10.0),
                            lufs.map_or_else(|| "null".to_owned(), to_string)
                        );
                        self.say(lufs.is_some(), Some(perf_now() - t0), &what);
                    }
                    Ok(None) => self.say(false, Some(perf_now() - t0), "no recorded audio to hear"),
                    Err(error) => self.say(false, Some(perf_now() - t0), &format!("listening: {}", head(&error, 200))),
                }
            }
        }
        if self.stopping.get() {
            return Ok(());
        }

        println!("\n1.0: playing and showing (no HISTORY; nothing to undo)");
        for (name, input) in [
            ("select", json!({"trackRef": target["ref"]})),
            ("show", json!({"action": "focus-view", "view": "Arranger"})),
            ("play", json!({"action": "start"})),
            ("play", json!({"action": "stop"})),
        ] {
            if self.tool(name).is_none() {
                skip(name);
                continue;
            }
            let starts = name == "play" && input["action"] == "start";
            let outcome = self.run(name, input).await;
            let what = if outcome.ok {
                text_or(&outcome.body["done"], name)
            } else {
                format!("{name}: {}", outcome.error.as_deref().unwrap_or_default())
            };
            self.say(outcome.ok, Some(outcome.ms), &what);
            if starts {
                tokio::time::sleep(Duration::from_millis(1_000)).await;
            }
        }
        if self.stopping.get() {
            return Ok(());
        }

        println!("\n1.0: watching a change by hand");
        if self.tool("watch_me").is_none() {
            skip("watch_me");
            return Ok(());
        }
        let started = self.run("watch_me", json!({"action": "start"})).await;
        let what = if started.ok {
            "watching the Set".to_owned()
        } else {
            format!("watch_me start: {}", started.error.as_deref().unwrap_or_default())
        };
        self.say(started.ok, Some(started.ms), &what);
        if !started.ok {
            return Ok(());
        }
        maybe("set_mixer", json!({"trackRef": target["ref"], "volume": 0.6})).await;
        self.observe().await?;
        let seen = self.run("watch_me", json!({"action": "stop"})).await;
        let changes = if seen.body["changes"].is_null() { json!([]) } else { seen.body["changes"].clone() };
        let count = changes.as_array().map_or(0, Vec::len);
        let what = if seen.ok {
            format!("saw {count} {}: {}", if count == 1 { "change" } else { "changes" }, head(&stringify(&changes), 160))
        } else {
            format!("watch_me stop: {}", seen.error.as_deref().unwrap_or_default())
        };
        self.say(seen.ok && count > 0, Some(seen.ms), &what);
        Ok(())
    }

    /// Bridge 1.0.58 and Kumi's Live extension: MIDI with notes straight into the Arrangement, part of it
    /// cleared, the rest deleted (the pad track is empty again), a device copied, and the bounce rendered
    /// offline and heard back.
    async fn accept_full_control(&self) -> Result<(), RuntimeError> {
        let skip = |name: &str| println!("  skip           {name} isn't offered by this bridge (or Kumi's Live extension isn't running)");
        self.observe().await?;
        let Some(pad) = last_named(&self.tracks().await, &self.pad).cloned() else {
            self.say(false, None, "the pad track is gone");
            return Ok(());
        };
        if self.tool("write_arrangement_clip").is_some() {
            // At the start of the pad track: the run copied the pad's clip to the Arrangement at SPOT already.
            let notes: Vec<Value> = [60, 63, 67]
                .iter()
                .enumerate()
                .map(|(index, pitch)| json!({"pitch": pitch, "start": index, "duration": 2, "velocity": 90}))
                .collect();
            let clip = json!({"trackRef": pad["ref"], "start": 0, "length": 8, "name": "Kumi Arrangement", "notes": notes});
            let written = self.change("write_arrangement_clip", clip).await;
            if let Some(written) = written.filter(|written| written.ok && !self.stopping.get()) {
                if self.tool("clear_range").is_some() {
                    self.change("clear_range", json!({"trackRef": pad["ref"], "fromBeat": 6, "toBeat": 8})).await;
                } else {
                    skip("clear_range");
                }
                if self.tool("delete_clip").is_some() {
                    self.observe().await?;
                    let clips = self.under("arrangement-clip", last_named(&self.tracks().await, &self.pad), &["name"]).await;
                    for clip in clips.iter().filter(|clip| clip["name"] == "Kumi Arrangement") {
                        if !self.stopping.get() {
                            self.change("delete_clip", json!({"clipRef": clip["ref"]})).await;
                        }
                    }
                    if let Some(changed) = written.body["changed"].as_str().filter(|changed| !changed.is_empty()) {
                        self.expected_kept.borrow_mut().insert(changed.to_owned());
                    }
                } else {
                    skip("delete_clip");
                }
            }
        } else {
            skip("write_arrangement_clip");
        }
        if self.tool("duplicate_device").is_some() && !self.stopping.get() {
            self.observe().await?;
            // A chain holds one instrument: copying the pad's is refused, plainly, and nothing changes. An effect is copied.
            let devices = self.under("device", last_named(&self.tracks().await, &self.pad), &["name", "deviceType"]).await;
            if let Some(instrument) = devices.iter().find(|row| row["deviceType"] == "instrument") {
                let refused = self.run("duplicate_device", json!({"deviceRef": instrument["ref"]})).await;
                let plainly = !refused.ok && refused.error.as_deref().unwrap_or_default().contains("one instrument");
                let what = format!("an instrument isn't copied beside itself{}", if refused.ok { ", but it was" } else { "" });
                self.say(plainly, Some(refused.ms), &what);
            }
            // Devices are discovered a track at a time: the first effect on the Set's first tracks.
            let mut effect = None;
            for track in self.tracks().await.iter().take(40) {
                let devices = self.under("device", Some(track), &["name", "deviceType"]).await;
                effect = devices.into_iter().find(|row| row["deviceType"] == "audio_effect" || row["deviceType"] == "midi_effect");
                if effect.is_some() || self.stopping.get() {
                    break;
                }
            }
            match effect {
                Some(effect) => {
                    self.change("duplicate_device", json!({"deviceRef": effect["ref"]})).await;
                }
                None => self.say(false, None, "no effect on the Set's first tracks to copy"),
            }
        } else if self.tool("duplicate_device").is_none() {
            skip("duplicate_device");
        }
        if self.tool("render").is_some() && !self.stopping.get() {
            let Some(bounce) = last_named(&self.tracks().await, &self.bounce).cloned() else {
                self.say(false, None, "no bounce to render");
                return Ok(());
            };
            let rendered = self.run("render", json!({"track": bounce["ref"], "from_beat": SPOT, "beats": 8})).await;
            let what = if rendered.ok {
                let seconds = rendered.body["seconds"].as_f64().unwrap_or(0.0);
                format!("rendered the bounce offline: {} s of audio", to_string(round(seconds * 10.0) / 10.0))
            } else {
                format!("render: {}", rendered.error.as_deref().unwrap_or_default())
            };
            self.say(rendered.ok, Some(rendered.ms), &what);
            if let Some(file) = rendered.body["file"].as_str().filter(|file| rendered.ok && !file.is_empty()) {
                let t0 = perf_now();
                match hear(file, listening()).await {
                    Ok(heard) => {
                        let lufs = heard.loudness.integrated_lufs;
                        let what = lufs.map_or_else(|| "silent".to_owned(), |lufs| format!("{} LUFS", to_string(round(lufs))));
                        self.say(lufs.is_some(), Some(perf_now() - t0), &format!("heard the render: {what}"));
                    }
                    Err(error) => {
                        self.say(false, Some(perf_now() - t0), &format!("listening to the render: {}", head(&error.to_string(), 200)))
                    }
                }
            }
        } else if self.tool("render").is_none() {
            skip("render");
        }
        Ok(())
    }

    async fn undo_all(&self) {
        let records = self.records.borrow().clone();
        for record in records.iter().rev() {
            if record.state != ChangeState::Applied {
                continue;
            }
            let t1 = perf_now();
            match self.integration.undo(Some(&record.id), signal()).await {
                Ok(after) => {
                    // A bounce that recorded keeps its track, and the track it recorded from, as a producer would want.
                    // Its input can't go back to Ext. In when Live has no audio input to offer.
                    let added = [format!("Added audio track “{}”", self.bounce), format!("Added MIDI track “{}”", self.pad)];
                    let note = after.note.as_deref().unwrap_or_default();
                    let keeps = after.state == ChangeState::Kept
                        && (added.contains(&record.title)
                            || self.expected_kept.borrow().contains(&record.title)
                            || (record.title.starts_with(&format!("{}: input from", self.bounce))
                                && note.starts_with("Live doesn't offer")));
                    let state = json!(after.state);
                    let why = if note.is_empty() { String::new() } else { format!(" ({note})") };
                    let what = format!("{} · {}{why}", state.as_str().unwrap_or_default(), record.title);
                    self.say(after.state == ChangeState::Undone || keeps, Some(perf_now() - t1), &what);
                }
                Err(error) => self.say(false, Some(perf_now() - t1), &format!("{}: {}", record.title, head(&error.message(), 200))),
            }
        }
        for record in self.records.borrow().iter() {
            if record.state == ChangeState::Unsure {
                self.say(false, None, &format!("unsure, check Live: {}", record.title));
            }
        }
        if self.integration.has_stop_live() {
            let _ = self.integration.stop_live(signal()).await;
        }
    }

    async fn check(&self, before: &State) -> Result<(), RuntimeError> {
        self.observe().await?;
        let mut after = self.state().await;
        after.tracks.retain(|name| name.as_str() != Some(&self.bounce) && name.as_str() != Some(&self.pad));
        let restored = before.tempo.as_ref().map(stringify) == after.tempo.as_ref().map(stringify)
            && stringify(&json!(before.tracks)) == stringify(&json!(after.tracks))
            && before.scenes == after.scenes
            && before.locators == after.locators;
        let what = if restored {
            let (tracks, scenes, locators) = (after.tracks.len(), after.scenes, after.locators.len());
            format!("Set as it was: tempo {}, {tracks} tracks, {scenes} scenes, {locators} locators", js_string(after.tempo.as_ref()))
        } else {
            let both = json!({"before": summary(before), "after": summary(&after)});
            format!("Set differs from before: {}", head(&stringify(&both), 600))
        };
        self.say(restored, None, &what);
        Ok(())
    }
}

fn signal() -> Signal {
    abort::timeout(120_000)
}

fn listening() -> AnalyzeOptions {
    AnalyzeOptions { signal: Some(signal()), ..Default::default() }
}

fn seconds(ms: f64) -> String {
    if ms < 1000.0 {
        format!("{} ms", to_string(round(ms)))
    } else {
        format!("{} s", to_fixed(ms / 1000.0, 1))
    }
}

/// A read's payload, as Kumi gives it to the model.
fn content_of(body: &Value) -> &Value {
    &body["live"]
}

/// The last row with this name.
fn last_named<'a>(rows: &'a [Value], name: &str) -> Option<&'a Value> {
    rows.iter().rfind(|row| row["name"] == name)
}

/// The Set's state as the report shows it, without what was only read to compare.
fn summary(state: &State) -> Value {
    let mut summary = JsonObject::new();
    if let Some(tempo) = &state.tempo {
        summary.insert("tempo".into(), tempo.clone());
    }
    summary.insert("tracks".into(), json!(state.tracks));
    summary.insert("scenes".into(), json!(state.scenes));
    summary.insert("locators".into(), json!(state.locators));
    Value::Object(summary)
}

/// Whether JavaScript counts this as true.
fn truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(value) => *value,
        Value::Number(number) => number.as_f64().is_some_and(|number| number != 0.0),
        Value::String(text) => !text.is_empty(),
        Value::Array(_) | Value::Object(_) => true,
    }
}

/// A value as JavaScript writes it into text (`${value}`).
fn js_string(value: Option<&Value>) -> String {
    match value {
        None => "undefined".into(),
        Some(Value::Null) => "null".into(),
        Some(Value::String(text)) => text.clone(),
        Some(Value::Object(_)) => "[object Object]".into(),
        Some(Value::Array(items)) => {
            items.iter().map(|item| if item.is_null() { String::new() } else { js_string(Some(item)) }).collect::<Vec<_>>().join(",")
        }
        Some(value) => stringify(value),
    }
}

/// `value ?? fallback`, as text.
fn text_or(value: &Value, fallback: &str) -> String {
    if value.is_null() {
        fallback.into()
    } else {
        js_string(Some(value))
    }
}
