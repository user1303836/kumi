//! Opt-in acceptance run on the real Logic Pro: Kumi's host started beside this binary, as Kumi starts it, the bridge
//! it puts beside Logic, and the M1, M2 and M3 checks of Logic's plan against the project Logic has open. M1: the
//! status and discovery. M2: the focus's latency (median and worst over 50 calls, under 150 ms), the first look and
//! every kind read through logic_read, the four bodies, and, with an operator at Logic, the selection following, a
//! dialog counting as busy, a project switched and refs held across a track moved. M3: a plan of several changes (a
//! fader, a rename, a track added, the new track muted) through Kumi's plan tool, each in HISTORY, then each undone
//! through Kumi (the fader and the mute put back by value, the rename as a fresh step of Logic's undo, the added track
//! by its deletion); and, with the operator, a rename made through Kumi and a Cmd-Z in Logic named back by the bridge.
//! M4 (the first slice): a save as to a copy under the folder `--files` names (the project is that copy from then on,
//! so the recorded one stays as it was), a key command Logic keeps no step of (no row) and one it does (a marker, whose
//! row only Logic's own undo takes back: taken away through Kumi by its deletion), a region split and its undo, a MIDI
//! import of 64 notes read back and undone, the cycle set and cleared, a tempo change set and deleted in the Tempo
//! List, each read back, and everything it made undone through Kumi at the end, with the times. M4 (the second slice):
//! the Mixer's strips, a plug-in's parameters, a track's Smart Controls and automation read; a Gain inserted on the
//! Stereo Out and its gain set through the plug-in window's Controls View, read back as a parameter; a send from track 1
//! to a bus nothing takes (Logic makes an aux for it); a patch from the Library loaded on a software instrument track
//! (`--patch` names it, "Piano/Steinway Grand Piano" by default); a bounce of bars 1 to 5 under the `--files` folder,
//! found on disk with its length and deleted after; each undone or taken away, and the project read as before. M5,
//! listening: the listening tools offered; bars 1 to 5 heard (Logic plays them while the bridge's helper taps what it
//! outputs: the file's length and loudness); a judged run toward a loudness 3 LU above what was heard, the first
//! software instrument track up 1 dB, and the round that brings the level back with a Gain added at the end of the
//! Stereo Out and set (read back as a parameter), then each of the run's changes undone through Kumi; tracks 1 and 2
//! auditioned, a pass each; a render of bars 1 to 5 of the instrument track found on disk with its length, then
//! deleted; and the project read as before (its tracks, their faders and solos, the Stereo Out's plug-ins).
//! M6: the fixed phrases (the tempo 4 BPM up, the metronome turned and turned back, a track muted, the cycle on bars 1
//! to 4 and off), each through the integration's instant path with no model, one HISTORY row each, read back and undone
//! through Kumi; the metronome set as a make_changes step and undone; a 1 s sine the run writes under the `--files`
//! folder loaded into a new Sampler track (only into the copy the save as made, since Logic copies the file into the
//! project's package), read back and undone through both of Logic's undo steps, and Quick Sampler refused; the projects
//! Logic opened lately, the copy among them; middle C on the project row and the notes named in its octave; the bulk
//! read beside the paged reads (the same tracks and markers, both timed); watch_me over a marker added through Kumi;
//! Channel EQ's and the Compressor's parameter changes drawn as curves in HISTORY, each undone; Logic's library folders
//! on this Mac; and the project read as before (its tracks, markers, tempo, metronome and cycle).
//! It leaves the project as it found it: every change it makes is undone before it ends (the operator's own steps of
//! M2 aside). It prints a Markdown summary for docs/evidence/ at the end (and writes it to --report, when given). The
//! bridge must be set up in the home (`kumi --daw logic bridge`) and Accessibility allowed; build the host first:
//!   cargo build --release -p daw-mcp-server --bins
//!   cargo run --release -p kumi --example accept_logic -- [--kumi-home <dir>] [--files <dir>] [--patch <path>] [--report <file>] [--no-operator]
use futures::FutureExt;
use kumi_common::{
    abort::{self, Signal},
    time::perf_now,
};
use kumi_runtime::{
    core::{
        context::IntegrationContext,
        contracts::{ChangeRecord, ChangeState, HearRequest, KernelTool},
        daw::DawId,
        phrases::{self, Intent},
        picture::ChangePicture,
        view::BodyRequest,
    },
    integrations::{
        self,
        logic::create_logic_integration,
        logic_project,
        own_bridge::{Connect, OwnOptions},
    },
    library::sources::{SourceKind, SourceOptions},
    listening::measure::{measure_file, MeasureOptions},
    mcp::{
        client::{connect_mcp, BridgeDescriptor, McpEndpoint, Options},
        types::{CallToolResult, ContentBlock},
    },
    system::Env,
    Integration, JsonObject, Observation, RuntimeError,
};
use serde_json::{json, Value};
use std::{
    cell::{Cell, RefCell},
    future::Future,
    path::{Path, PathBuf},
    rc::Rc,
    time::Duration,
};

/// The focus answers from a cache: this is the most a call may take, median and worst alike.
const FOCUS_LIMIT_MS: f64 = 150.0;
const FOCUS_CALLS: usize = 50;
/// How long each step with the operator waits for them.
const OPERATOR_MS: u64 = 90_000;
/// (M4b) The Library's patch the run loads on a software instrument track, unless `--patch` names another.
const PATCH: &str = "Piano/Steinway Grand Piano";
/// (M6) The sine the run writes under the `--files` folder and loads into a Sampler track, which Logic names after it.
const SINE: &str = "Kumi M6 Sine.wav";
/// (M6) The most a change's curve may take to be drawn in HISTORY: the follow-up read waits for the changes to settle
/// (0.3 s, at most 3 s), then reads the plug-in's Controls View.
const CURVE_MS: f64 = 20_000.0;
/// (M6) The pause before the paged reads and the bulk read are each timed: past the bridge's caches (the Mixer's strips
/// are kept 5 s, the List Editors' rows 2 s), so each reads Logic afresh.
const COLD_MS: u64 = 5_500;

fn main() {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let after = |flag: &str| argv.iter().position(|arg| arg == flag).and_then(|at| argv.get(at + 1)).cloned();
    if argv.iter().any(|arg| arg == "--help" || arg == "-h") {
        eprintln!(
            "cargo run --release -p kumi --example accept_logic -- [--kumi-home <dir>] [--files <dir>] [--patch <path>] [--report <file>] [--no-operator]"
        );
        std::process::exit(2);
    }
    let env: Env = std::env::vars().collect();
    let kumi_home = after("--kumi-home").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(kumi::config::kumi_dir(&env)));
    let report = after("--report").map(PathBuf::from);
    // The folder the save as goes under (M4): never the recorded project's own folder.
    let files = after("--files").map(PathBuf::from);
    // The Library's patch the second slice loads (M4b).
    let patch = after("--patch").unwrap_or_else(|| PATCH.to_string());
    let operator = !argv.iter().any(|arg| arg == "--no-operator");
    let (host, build) = host_program();
    if !host.is_file() {
        eprintln!("Kumi's host isn't built ({}); build it first: {build}", host.display());
        std::process::exit(2);
    }
    let runtime = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
        Ok(runtime) => runtime,
        Err(error) => {
            eprintln!("accept-logic: {error}");
            std::process::exit(1);
        }
    };
    let local = tokio::task::LocalSet::new();
    let code = local.block_on(&runtime, accept_logic(kumi_home, host, operator, report, files, patch));
    std::process::exit(code);
}

/// The host this run starts, and the command that builds it. Kumi starts `daw-mcp-server` beside its own binary;
/// Cargo puts an example one folder further down, in `examples`.
fn host_program() -> (PathBuf, String) {
    let name = if cfg!(windows) { "daw-mcp-server.exe" } else { "daw-mcp-server" };
    let mut folder = std::env::current_exe().ok().and_then(|exe| exe.parent().map(Path::to_path_buf)).unwrap_or_default();
    if folder.file_name().is_some_and(|name| name == "examples") {
        folder.pop();
    }
    let profile = match folder.file_name().and_then(|name| name.to_str()) {
        Some("release") => " --release".to_owned(),
        Some(name) if name != "debug" => format!(" --profile {name}"),
        _ => String::new(),
    };
    (folder.join(name), format!("cargo build{profile} -p daw-mcp-server --bins"))
}

/// One check's row in the summary.
struct Row {
    ok: bool,
    ms: Option<f64>,
    what: String,
}

/// A host tool's answer: its JSON (null when it isn't JSON), whether it was an error, and the error's code.
struct Answer {
    ok: bool,
    ms: f64,
    body: Value,
}

impl Answer {
    fn code(&self) -> Option<&str> {
        self.body.get("code").and_then(Value::as_str)
    }
    fn error(&self) -> String {
        self.body.get("error").and_then(Value::as_str).map(str::to_owned).unwrap_or_else(|| short(&self.body.to_string(), 200))
    }
}

/// The bridge's events as the host forwards them, in order: their names and data.
type Events = Rc<RefCell<Vec<(String, Value)>>>;

/// One run: the host's endpoint (Kumi's integration shares it), the integration, the events heard, and the rows.
struct Accept {
    endpoint: Rc<dyn McpEndpoint>,
    integration: Rc<dyn Integration>,
    events: Events,
    rows: RefCell<Vec<Row>>,
    observation: RefCell<Option<Observation>>,
    stopping: Rc<Cell<bool>>,
    operator: bool,
    /// What the status said, for the summary's header.
    status: RefCell<Value>,
    /// Kumi's changes as HISTORY hears them: each record as it is first told, and again as its state moves.
    changes: Rc<RefCell<Vec<ChangeRecord>>>,
    /// Where the save as of M4 writes its copy, when the run was given a folder.
    files: Option<PathBuf>,
    /// (M4b) The Library's patch the run loads.
    patch: String,
}

fn signal() -> Signal {
    abort::timeout(60_000)
}

/// (M5) A listening tool's time: its passes play in real time, a few after one another.
fn listening() -> Signal {
    abort::timeout(300_000)
}

fn short(text: &str, most: usize) -> String {
    let text: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if text.chars().count() <= most {
        text
    } else {
        format!("{}…", text.chars().take(most).collect::<String>())
    }
}

fn seconds(ms: f64) -> String {
    if ms < 1000.0 {
        format!("{} ms", ms.round())
    } else {
        format!("{:.1} s", ms / 1000.0)
    }
}

/// One of an observation's tools, by name.
fn tool_named(observation: &Observation, name: &str) -> Option<Rc<dyn kumi_runtime::core::contracts::KernelTool>> {
    observation.tools.iter().find(|tool| tool.name() == name).cloned()
}

/// A change's state as HISTORY words it.
fn state_word(state: ChangeState) -> &'static str {
    match state {
        ChangeState::Applied => "applied",
        ChangeState::Undone => "undone",
        ChangeState::Kept => "kept",
        ChangeState::Unsure => "unsure",
        ChangeState::Expired => "expired",
        ChangeState::Heard => "heard",
    }
}

/// (M4b) The ref a change made, from its tool's text: a change tool's own answer names it as `ref`, a plan's as its first
/// step's.
fn made_ref(text: &str) -> Option<String> {
    let answer: Value = serde_json::from_str(text).ok()?;
    answer["ref"].as_str().or_else(|| answer["done"][0]["ref"].as_str()).map(str::to_owned)
}

/// (M4b) A WAV file's length in seconds, from its header (its format chunk and the size of its data chunk).
fn wav_seconds(path: &Path) -> Option<f64> {
    let bytes = std::fs::read(path).ok()?;
    if bytes.get(..4)? != b"RIFF" || bytes.get(8..12)? != b"WAVE" {
        return None;
    }
    let (mut at, mut rate, mut block) = (12usize, 0u32, 0u16);
    while at + 8 <= bytes.len() {
        let id = &bytes[at..at + 4];
        let size = u32::from_le_bytes(bytes[at + 4..at + 8].try_into().ok()?) as usize;
        let body = at + 8;
        if id == b"fmt " && body + 14 <= bytes.len() {
            rate = u32::from_le_bytes(bytes[body + 4..body + 8].try_into().ok()?);
            block = u16::from_le_bytes(bytes[body + 12..body + 14].try_into().ok()?);
        }
        if id == b"data" {
            return (rate > 0 && block > 0).then(|| size as f64 / f64::from(rate) / f64::from(block));
        }
        at = body + size + size % 2;
    }
    None
}

/// The JSON a tool answered with, from its text blocks.
fn body_of(result: &CallToolResult) -> Value {
    let text = result
        .content
        .iter()
        .filter_map(|block| match block {
            ContentBlock::Text { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n");
    serde_json::from_str(&text).unwrap_or(Value::String(text))
}

/// (M6) A second of a 440 Hz sine at half scale, 44.1 kHz, 16-bit and mono, as a WAV file: the sample the run loads.
fn write_sine(path: &Path) -> std::io::Result<()> {
    const RATE: u32 = 44_100;
    let data: Vec<u8> = (0..RATE)
        .flat_map(|n| {
            let phase = f64::from(n) * 440.0 * std::f64::consts::TAU / f64::from(RATE);
            ((phase.sin() * 0.5 * f64::from(i16::MAX)).round() as i16).to_le_bytes()
        })
        .collect();
    let mut bytes = Vec::with_capacity(44 + data.len());
    bytes.extend_from_slice(b"RIFF");
    bytes.extend_from_slice(&(36 + data.len() as u32).to_le_bytes());
    bytes.extend_from_slice(b"WAVEfmt ");
    bytes.extend_from_slice(&16u32.to_le_bytes());
    // PCM, one channel, the rate, the bytes a second, the bytes a frame and the bits a sample.
    bytes.extend_from_slice(&1u16.to_le_bytes());
    bytes.extend_from_slice(&1u16.to_le_bytes());
    bytes.extend_from_slice(&RATE.to_le_bytes());
    bytes.extend_from_slice(&(RATE * 2).to_le_bytes());
    bytes.extend_from_slice(&2u16.to_le_bytes());
    bytes.extend_from_slice(&16u16.to_le_bytes());
    bytes.extend_from_slice(b"data");
    bytes.extend_from_slice(&(data.len() as u32).to_le_bytes());
    bytes.extend_from_slice(&data);
    std::fs::write(path, bytes)
}

/// (M6) The first figure of a value as Logic shows it: "+3.0 dB" is 3, "−20.0 dB" -20.
fn number_of(text: &str) -> Option<f64> {
    let text = text.trim().replace('\u{2212}', "-");
    let figure: String = text.trim_start_matches('+').chars().take_while(|c| c.is_ascii_digit() || matches!(c, '-' | '.')).collect();
    figure.parse().ok()
}

/// (M6) Two readings alike: numbers within a hundredth, objects and lists field by field, anything else equal.
fn alike(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => x.as_f64().zip(y.as_f64()).is_some_and(|(x, y)| (x - y).abs() < 0.01),
        (Value::Object(x), Value::Object(y)) => {
            x.len() == y.len() && x.iter().all(|(key, value)| y.get(key).is_some_and(|other| alike(value, other)))
        }
        (Value::Array(x), Value::Array(y)) => x.len() == y.len() && x.iter().zip(y).all(|(value, other)| alike(value, other)),
        _ => a == b,
    }
}

/// (M6) A path with the home folder written `~`, so the record names no one's home.
fn tilde(path: &str) -> String {
    let home = kumi_runtime::library::sources::homedir();
    match path.strip_prefix(home.as_str()) {
        Some(rest) if !home.is_empty() && (rest.is_empty() || rest.starts_with('/')) => format!("~{rest}"),
        _ => path.to_owned(),
    }
}

/// (M6) A path as the disk has it, to tell whether one is inside another; as given when it isn't there.
fn whole(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

/// (M6) Rows with their refs read without the epoch they carry (`7:track:2` as `track:2`), so two reads of the same rows
/// in different epochs compare.
fn without_epoch(value: &Value, epoch: &str) -> Value {
    match value {
        Value::Array(items) => Value::Array(items.iter().map(|item| without_epoch(item, epoch)).collect()),
        Value::Object(fields) => Value::Object(
            fields
                .iter()
                .map(|(key, item)| {
                    let item = match item.as_str() {
                        Some(text) if key == "ref" || key.ends_with("Ref") => {
                            json!(text.strip_prefix(&format!("{epoch}:")).unwrap_or(text))
                        }
                        _ => without_epoch(item, epoch),
                    };
                    (key.clone(), item)
                })
                .collect(),
        ),
        other => other.clone(),
    }
}

/// (M6) What HISTORY draws of a change, in words.
fn picture_words(picture: Option<&ChangePicture>) -> String {
    match picture {
        Some(ChangePicture::Eq { before, after, .. }) => {
            let moved = before
                .iter()
                .zip(after)
                .position(|(was, now)| (was.gain_db - now.gain_db).abs() > 0.05 || (was.freq - now.freq).abs() > 0.5 || was.on != now.on);
            match moved {
                Some(at) => format!(
                    "an EQ curve of {} bands, band {} ({:?}) {:+.1} → {:+.1} dB",
                    after.len(),
                    at + 1,
                    after[at].shape,
                    before[at].gain_db,
                    after[at].gain_db
                ),
                None => format!("an EQ curve of {} bands, none of them moved", after.len()),
            }
        }
        Some(ChangePicture::Dynamics { before, after, .. }) => {
            format!("a compressor's curve, its threshold {:.1} → {:.1} dB at {:.1}:1", before.threshold_db, after.threshold_db, after.ratio)
        }
        Some(ChangePicture::Values { values }) => format!(
            "its values only ({})",
            values
                .iter()
                .map(|value| format!(
                    "{} {} → {}",
                    value.name,
                    value.from_text.as_deref().unwrap_or("?"),
                    value.to_text.as_deref().unwrap_or("?")
                ))
                .collect::<Vec<_>>()
                .join(", ")
        ),
        Some(_) => "another kind of picture".into(),
        None => "no picture".into(),
    }
}

/// (M6) What a fixed phrase or a step changes, read back through the bridge.
enum Reading {
    /// The project's tempo, from its row.
    Tempo,
    /// The metronome's click (true for on), from a metronome-set previewed and never applied.
    Click,
    /// A track's mute, from its row, by the track's name.
    Mute(String),
    /// The cycle as a loop-range previewed (never applied) fences it, `{on, startBeats, endBeats}`, with the focus's
    /// transport's `cycle` beside it as `focus`.
    Cycle,
}

/// (M6) A reading in words.
fn reading_words(what: &Reading, value: &Value) -> String {
    let on = |value: &Value, yes: &str, no: &str, none: &str| match value.as_bool() {
        Some(true) => yes.to_owned(),
        Some(false) => no.to_owned(),
        None => none.to_owned(),
    };
    match what {
        Reading::Tempo => format!("{value} BPM"),
        Reading::Click => on(value, "the click on", "the click off", "no click"),
        Reading::Mute(name) => on(value, &format!("“{name}” muted"), &format!("“{name}” not muted"), &format!("no track “{name}”")),
        Reading::Cycle if value.get("on").is_none() => format!("no cycle (the focus: cycle {})", value["focus"]),
        Reading::Cycle => format!(
            "the cycle {}, beats {} to {} (the focus: cycle {})",
            on(&value["on"], "on", "off", "?"),
            value["startBeats"],
            value["endBeats"],
            value["focus"]
        ),
    }
}

/// (M6) What M6 leaves as it found, in words.
fn state_words(state: &Value) -> String {
    let tracks: Vec<String> = state["tracks"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|track| format!("{}{}", track["name"].as_str().unwrap_or("?"), if track["mute"] == true { " (muted)" } else { "" }))
        .collect();
    format!(
        "{} tracks ({}), {} markers, {}, {}, {}",
        tracks.len(),
        tracks.join(", "),
        state["markers"].as_array().map_or(0, Vec::len),
        reading_words(&Reading::Tempo, &state["tempo"]),
        reading_words(&Reading::Click, &state["click"]),
        reading_words(&Reading::Cycle, &state["cycle"])
    )
}

async fn accept_logic(
    kumi_home: PathBuf,
    host: PathBuf,
    operator: bool,
    report: Option<PathBuf>,
    files: Option<PathBuf>,
    patch: String,
) -> i32 {
    let stopping = Rc::new(Cell::new(false));
    tokio::task::spawn_local({
        let stopping = stopping.clone();
        async move {
            if tokio::signal::ctrl_c().await.is_ok() {
                stopping.set(true);
                println!("\nStopping after this check…");
                if tokio::signal::ctrl_c().await.is_ok() {
                    std::process::exit(130);
                }
            }
        }
    });
    println!("Kumi's host for Logic Pro, from {} with the home {}", host.display(), kumi_home.display());
    let t0 = perf_now();
    let endpoint = match connect_mcp(Options {
        signal: signal(),
        entry: Some(host.clone()),
        args: vec!["--daw".into(), "logic".into(), "--kumi-home".into(), kumi_home.to_string_lossy().into_owned()],
        timeout_ms: Some(30_000),
        cwd: host.parent().map(Path::to_path_buf),
        bridge: Some(BridgeDescriptor::LOGIC),
        ..Default::default()
    })
    .await
    {
        Ok(endpoint) => endpoint,
        Err(error) => {
            eprintln!("accept-logic: the host didn't start: {}", short(&error.message(), 300));
            return 1;
        }
    };
    println!("  ok   {:>8}  the host started", seconds(perf_now() - t0));
    let events: Events = Rc::default();
    let _listening = endpoint.on_live_event(Rc::new({
        let events = events.clone();
        move |event| {
            let name = event.get("name").and_then(Value::as_str).unwrap_or_default().to_owned();
            let data = event.get("data").cloned().unwrap_or(Value::Null);
            events.borrow_mut().push((name, data));
        }
    }));
    // Kumi's integration over the same host, as the app makes it: its looks, read tool and bodies are the real ones.
    // Nothing draws the focus here, so the integration doesn't poll it: the latency below is this run's alone.
    let shared = endpoint.clone();
    let connect: Connect = Rc::new(move |_| {
        let endpoint = shared.clone();
        async move { Ok(endpoint) }.boxed_local()
    });
    let mut context = IntegrationContext::new(Rc::new(|_, _| {}));
    let changes: Rc<RefCell<Vec<ChangeRecord>>> = Rc::default();
    let into = changes.clone();
    context.on_change = Some(Rc::new(move |record| into.borrow_mut().push(record)));
    let integration = create_logic_integration(OwnOptions { context, kumi_home, connect: Some(connect), open_file: None });
    let accept = Accept {
        endpoint,
        integration,
        events,
        rows: RefCell::default(),
        observation: RefCell::default(),
        stopping,
        operator,
        status: RefCell::new(Value::Null),
        changes,
        files,
        patch,
    };
    let code = accept.accept().await.unwrap_or_else(|error| {
        eprintln!("accept-logic: {}", short(&error.message(), 300));
        1
    });
    let summary = accept.summary();
    println!("\n{summary}");
    if let Some(report) = report {
        match std::fs::write(&report, &summary) {
            Ok(()) => println!("Written to {}", report.display()),
            Err(error) => eprintln!("accept-logic: couldn't write {}: {error}", report.display()),
        }
    }
    let _ = accept.integration.close().await;
    code
}

impl Accept {
    fn say(&self, ok: bool, ms: Option<f64>, what: &str) {
        println!("  {} {:>8}  {what}", if ok { "ok  " } else { "FAIL" }, ms.map(seconds).unwrap_or_default());
        self.rows.borrow_mut().push(Row { ok, ms, what: what.into() });
    }

    /// A host tool, asked directly: what Kumi's integration asks of the host, without the turn's short refs.
    async fn tool(&self, name: &str, arguments: Value) -> Answer {
        let args: JsonObject = arguments.as_object().cloned().unwrap_or_default();
        let t0 = perf_now();
        match self.endpoint.call(name, args, signal()).await {
            Ok(result) => Answer { ok: result.is_error != Some(true), ms: perf_now() - t0, body: body_of(&result) },
            Err(error) => Answer { ok: false, ms: perf_now() - t0, body: json!({"error": error.message()}) },
        }
    }

    /// The tempo at `beats` in Logic's Tempo List, read back through the bridge: a delete previewed there (never
    /// applied) finds the event and its BPM, or says there is none; `Err` when the preview failed otherwise.
    async fn tempo_listed(&self, beats: f64) -> Result<Option<f64>, String> {
        let epoch = self.tool("read", json!({"kind": "project"})).await.body["epoch"].clone();
        let preview =
            self.tool("change_preview", json!({"kind": "tempo-marker-delete", "input": {"positionBeats": beats}, "epoch": epoch})).await;
        if preview.ok {
            return Ok(preview.body["prior"]["bpm"].as_f64());
        }
        let error = preview.error();
        if error.contains("there is no tempo event") {
            Ok(None)
        } else {
            Err(short(&error, 160))
        }
    }

    async fn observe(&self) -> Result<Observation, RuntimeError> {
        let observation = self.integration.observe(signal(), None).await?;
        *self.observation.borrow_mut() = Some(observation.clone());
        Ok(observation)
    }

    /// Waits for the operator (or the bridge) until `done` answers, or the time is up: what `done` answered.
    async fn wait_for<T, F, Fut>(&self, mut done: F) -> Option<T>
    where
        F: FnMut() -> Fut,
        Fut: Future<Output = Option<T>>,
    {
        let deadline = tokio::time::Instant::now() + Duration::from_millis(OPERATOR_MS);
        while tokio::time::Instant::now() < deadline && !self.stopping.get() {
            if let Some(found) = done().await {
                return Some(found);
            }
            tokio::time::sleep(Duration::from_millis(150)).await;
        }
        done().await
    }

    /// The events heard since `from`, by name.
    fn events_since(&self, from: usize, name: &str) -> Vec<Value> {
        self.events.borrow().iter().skip(from).filter(|(heard, _)| heard == name).map(|(_, data)| data.clone()).collect()
    }

    async fn accept(&self) -> Result<i32, RuntimeError> {
        println!("\nM1: the status and discovery");
        let status = self.tool("status", json!({})).await;
        *self.status.borrow_mut() = status.body.clone();
        let connected = status.ok && status.body["connected"] == true;
        let project = status.body["project"]["name"].as_str().unwrap_or("none");
        let what = if connected {
            format!(
                "status: Logic Pro {} (pid {}, {}), bridge {}, project “{project}”, busy {}, permission {}",
                status.body["dawVersion"].as_str().unwrap_or("?"),
                status.body["pid"],
                status.body["resource"].as_str().unwrap_or("?"),
                status.body["bridge"].as_str().unwrap_or("?"),
                status.body["busy"],
                status.body["permission"]
            )
        } else {
            format!("status: not connected: {}", status.error())
        };
        self.say(connected, Some(status.ms), &what);
        if !connected {
            return Ok(1);
        }
        if status.body["permission"].as_str().is_some() {
            self.say(false, None, "Accessibility isn't allowed to Kumi's helper: the reads below will fail in words");
        }
        let discover = self.tool("discover", json!({})).await;
        let instances = discover.body["instances"].as_array().map_or(0, Vec::len);
        self.say(discover.ok && instances >= 1, Some(discover.ms), &format!("discover: {instances} Logic Pro instance(s)"));

        println!("\nM2: the focus's latency");
        // The bridge fills its focus cache on its first pass over the project after attaching: the latency is measured
        // from the first answer on, not against that pass.
        let warm = perf_now();
        loop {
            let focus = self.tool("focus", json!({})).await;
            if focus.ok || perf_now() - warm > 10_000.0 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        let mut times = Vec::with_capacity(FOCUS_CALLS);
        let mut last = Value::Null;
        for _ in 0..FOCUS_CALLS {
            let focus = self.tool("focus", json!({})).await;
            if !focus.ok {
                self.say(false, Some(focus.ms), &format!("focus: {}", focus.error()));
                break;
            }
            times.push(focus.ms);
            last = focus.body;
        }
        if times.len() == FOCUS_CALLS {
            times.sort_by(|a, b| a.total_cmp(b));
            let (median, worst) = (times[FOCUS_CALLS / 2], times[FOCUS_CALLS - 1]);
            let what = format!(
                "focus over {FOCUS_CALLS} calls: median {}, worst {} (under {} ms); selection {}, editor {}, ageMs {}",
                seconds(median),
                seconds(worst),
                FOCUS_LIMIT_MS,
                last["selection"]["trackRef"].as_str().unwrap_or("none"),
                last["editor"].as_str().unwrap_or("none"),
                last["ageMs"]
            );
            self.say(median < FOCUS_LIMIT_MS && worst < FOCUS_LIMIT_MS, Some(median), &what);
        }

        println!("\nM2: the first look, every kind, the bodies");
        let t0 = perf_now();
        let observation = self.observe().await?;
        let context: Value = serde_json::from_str(&observation.context).unwrap_or(Value::Null);
        let tracks = context["tracks"].as_array().map_or(0, Vec::len);
        let markers = context["markers"].as_array().map_or(0, Vec::len);
        let looked = observation.tools.iter().any(|tool| tool.name() == "logic_read");
        self.say(
            looked && tracks > 0,
            Some(perf_now() - t0),
            &format!(
                "first look: “{}”, {tracks} tracks, {markers} markers, selection {}, place {}",
                observation.label,
                context["selection"]["trackRef"].as_str().unwrap_or("none"),
                observation.project.as_ref().map(|project| project.id.as_str()).unwrap_or("unsaved")
            ),
        );
        let open = self.integration.open_set();
        self.say(open.is_some(), None, &format!("open_set: {}", open.as_deref().unwrap_or("none (the project isn't saved)")));
        let recording = self.integration.recording(signal()).await?;
        self.say(recording.is_some(), None, &format!("recording: {recording:?} (from the focus's transport)"));
        let selected_track = context["selection"]["trackRef"].as_str().map(str::to_owned);
        let selected_region = context["selection"]["regionRef"].as_str().map(str::to_owned);
        // The look's refs are the model's short ones; FOCUS's bodies and the selection check take the focus's long ones.
        let focus_track = last["selection"]["trackRef"].as_str().map(str::to_owned);
        let focus_region = last["selection"]["regionRef"].as_str().map(str::to_owned);
        let read = observation.tools.iter().find(|tool| tool.name() == "logic_read").cloned();
        if let Some(read) = read {
            let kinds: Vec<(&str, Option<&str>)> = vec![
                ("project", None),
                ("track", None),
                ("region", None),
                ("note", selected_region.as_deref()),
                ("plugin", selected_track.as_deref()),
                ("send", selected_track.as_deref()),
                ("marker", None),
                ("tempo", None),
            ];
            for (kind, parent) in kinds {
                let mut input = json!({"kind": kind});
                match parent {
                    Some(parent) => input["parent"] = json!(parent),
                    None if matches!(kind, "note") => {
                        self.say(false, None, "read note: no region is selected to read the notes of");
                        continue;
                    }
                    None => {}
                }
                let t0 = perf_now();
                let result = read.execute(input.as_object().cloned().unwrap_or_default(), signal()).await?;
                let page: Value = serde_json::from_str(&result.text).unwrap_or(Value::Null);
                let rows = page["items"].as_array().map_or(0, Vec::len);
                let what = if result.is_error {
                    format!("read {kind}: {}", short(&result.text, 200))
                } else {
                    format!("read {kind}: {rows} rows (total {}, epoch {})", page["total"], page["epoch"])
                };
                self.say(!result.is_error, Some(perf_now() - t0), &what);
            }
        }
        let bodies: Vec<(&str, Option<BodyRequest>)> = vec![
            ("tree", focus_track.clone().map(|track_ref| BodyRequest::Tree { track_ref })),
            ("notes", focus_region.clone().map(|item_ref| BodyRequest::Notes { item_ref })),
            ("timeline", Some(BodyRequest::Timeline)),
            ("routing", focus_track.clone().map(|track_ref| BodyRequest::Routing { track_ref })),
        ];
        for (name, request) in bodies {
            let Some(request) = request else {
                self.say(false, None, &format!("body {name}: nothing is selected to show it for"));
                continue;
            };
            let t0 = perf_now();
            let body = self.integration.focus_body(&request, signal()).await?;
            let what = match &body {
                Some(body) => format!("body {name}: {}", short(&format!("{body:?}"), 160)),
                None => format!("body {name}: nothing came back"),
            };
            self.say(body.is_some(), Some(perf_now() - t0), &what);
        }
        if self.stopping.get() {
            return Ok(self.exit_code());
        }

        println!("\nM3: a plan of changes, each undone through Kumi");
        self.plan_and_undo().await?;
        if self.stopping.get() {
            return Ok(self.exit_code());
        }
        println!("\nM4: a save as, key commands, a region split, a MIDI import, the cycle and the Tempo List, each undone");
        self.m4_kinds().await?;
        if self.stopping.get() {
            return Ok(self.exit_code());
        }
        println!("\nM4, the second slice: a Gain on the Stereo Out, a send to a new bus, a patch, a bounce, each undone");
        self.m4b_kinds().await?;
        if self.stopping.get() {
            return Ok(self.exit_code());
        }
        println!("\nM5: bars 1 to 5 heard, a judged round's gain on the Stereo Out, an audition, a render, each undone");
        self.m5_listening().await?;
        if self.stopping.get() {
            return Ok(self.exit_code());
        }
        println!("\nM6: the fixed phrases, the metronome, a sample in a Sampler track, the new reads, watch_me, curves, the library");
        self.m6_kinds().await?;
        if !self.operator || self.stopping.get() {
            println!("\n(the steps with an operator at Logic Pro were skipped)");
            return Ok(self.exit_code());
        }

        println!("\nM2: with you at Logic Pro");
        self.select_follows(focus_track.as_deref()).await;
        if self.stopping.get() {
            return Ok(self.exit_code());
        }
        self.modal_busy().await;
        if self.stopping.get() {
            return Ok(self.exit_code());
        }
        println!("\nM3: with you at Logic Pro");
        self.cmd_z_named().await?;
        if self.stopping.get() {
            return Ok(self.exit_code());
        }
        println!("\nM2: with you at Logic Pro, the project changed");
        self.refs_hold_across_a_track_move().await;
        if self.stopping.get() {
            return Ok(self.exit_code());
        }
        self.project_switch(observation.project.as_ref().map(|project| project.id.clone())).await?;
        Ok(self.exit_code())
    }

    /// The operator selects another track: the focus follows, and a `selection` event comes.
    async fn select_follows(&self, before: Option<&str>) {
        println!("  → In Logic Pro, select another track (within {} s)…", OPERATOR_MS / 1000);
        // The selection as it stands now, by its place: refs carry the epoch, which the plan before this moved.
        let place = |track_ref: &str| track_ref.splitn(2, ':').nth(1).unwrap_or(track_ref).to_owned();
        let fresh = self.tool("focus", json!({})).await;
        let before: Option<String> = fresh.body["selection"]["trackRef"].as_str().map(str::to_owned).or_else(|| before.map(str::to_owned));
        let before_place = before.as_deref().map(place);
        let before = before.as_deref();
        let heard = self.events.borrow().len();
        let t0 = perf_now();
        let seen = self
            .wait_for(|| {
                let before_place = before_place.clone();
                async move {
                    let focus = self.tool("focus", json!({})).await;
                    let now = focus.body["selection"]["trackRef"].as_str().map(str::to_owned);
                    (focus.ok && now.is_some() && now.as_deref().map(place) != before_place).then_some(now.unwrap_or_default())
                }
            })
            .await;
        let events = self.events_since(heard, "selection");
        match seen {
            Some(now) => self.say(true, Some(perf_now() - t0), &format!("the focus follows: {} → {now}", before.unwrap_or("none"))),
            None => self.say(false, Some(perf_now() - t0), "the focus didn't follow a selection (none made, or not seen)"),
        }
        self.say(!events.is_empty(), None, &format!("selection events: {} ({})", events.len(), short(&json!(events).to_string(), 120)));
    }

    /// The operator opens a dialog (Cmd-Shift-S) and closes it (Escape): the status says modal, a look is refused as
    /// busy, and the next one succeeds.
    async fn modal_busy(&self) {
        println!("  → In Logic Pro, press Cmd-Shift-S (Save As…), wait for the ok below, then press Escape…");
        let t0 = perf_now();
        let modal = self
            .wait_for(|| async move {
                let status = self.tool("status", json!({})).await;
                (status.body["busy"] == "modal").then_some(())
            })
            .await;
        self.say(modal.is_some(), Some(perf_now() - t0), "status says busy: modal while the dialog is up");
        if modal.is_none() {
            return;
        }
        let refused = self.tool("snapshot", json!({})).await;
        self.say(
            !refused.ok && refused.code() == Some("busy"),
            Some(refused.ms),
            &format!("a look is refused as busy: {}", refused.error()),
        );
        println!("  → now press Escape in Logic Pro…");
        let t0 = perf_now();
        let free = self
            .wait_for(|| async move {
                let status = self.tool("status", json!({})).await;
                (status.body["busy"] == false).then_some(())
            })
            .await;
        self.say(free.is_some(), Some(perf_now() - t0), "status says free again once the dialog is closed");
        let look = self.tool("snapshot", json!({})).await;
        self.say(look.ok, Some(look.ms), &format!("the next look succeeds: {} tracks", look.body["tracks"].as_array().map_or(0, Vec::len)));
    }

    /// The operator adds or moves a track: the epoch moves, a ref from before is refused as stale, and a fresh read
    /// carries the new epoch.
    async fn refs_hold_across_a_track_move(&self) {
        let before = self.tool("read", json!({"kind": "track"})).await;
        let Some(reference) = before.body["items"][0]["ref"].as_str().map(str::to_owned) else {
            self.say(false, Some(before.ms), &format!("read track before the move: {}", before.error()));
            return;
        };
        let epoch = before.body["epoch"].as_str().unwrap_or_default().to_owned();
        println!("  → In Logic Pro, add a track (Option-Cmd-N) or drag one to another place…");
        let heard = self.events.borrow().len();
        let t0 = perf_now();
        let epoch = &epoch;
        let moved = self
            .wait_for(|| async move {
                let page = self.tool("read", json!({"kind": "track"})).await;
                let now = page.body["epoch"].as_str().unwrap_or_default().to_owned();
                (page.ok && now != *epoch).then_some(now)
            })
            .await;
        let tracks_events = self.events_since(heard, "tracks").len();
        match moved {
            Some(now) => {
                self.say(true, Some(perf_now() - t0), &format!("the epoch moved: {epoch} → {now} ({tracks_events} tracks events)"))
            }
            None => {
                self.say(false, Some(perf_now() - t0), "the epoch didn't move (no track added or moved, or not seen)");
                return;
            }
        }
        let stale = self.tool("read", json!({"ref": reference})).await;
        self.say(
            !stale.ok && stale.code() == Some("stale"),
            Some(stale.ms),
            &format!("a ref from before ({reference}) is refused as stale: {}", stale.error()),
        );
        let fresh = self.tool("read", json!({"kind": "track"})).await;
        let first = fresh.body["items"][0]["ref"].as_str().unwrap_or("none");
        self.say(
            fresh.ok && first.starts_with(&format!("{}:", fresh.body["epoch"].as_str().unwrap_or("?"))),
            Some(fresh.ms),
            &format!("a fresh read carries the new epoch: {first}"),
        );
    }

    /// The operator opens another project: a `project` event comes, and the next look is of another place.
    async fn project_switch(&self, place: Option<String>) -> Result<(), RuntimeError> {
        println!("  → In Logic Pro, open another project (File > Open Recent…)…");
        let heard = self.events.borrow().len();
        let t0 = perf_now();
        let event = self.wait_for(|| async move { self.events_since(heard, "project").into_iter().next() }).await;
        match &event {
            Some(data) => self.say(true, Some(perf_now() - t0), &format!("project event: {}", short(&data.to_string(), 160))),
            None => {
                self.say(false, Some(perf_now() - t0), "no project event (no project opened, or not seen)");
                return Ok(());
            }
        }
        // The bridge may still be loading: the look waits for it as Kumi's does.
        tokio::time::sleep(Duration::from_millis(1_000)).await;
        let t0 = perf_now();
        let observation = self.observe().await?;
        let now = observation.project.as_ref().map(|project| project.id.clone());
        self.say(
            now != place && observation.tools.iter().any(|tool| tool.name() == "logic_read"),
            Some(perf_now() - t0),
            &format!(
                "the look after is of another place: {} → {}; “{}”",
                place.as_deref().unwrap_or("unsaved"),
                now.as_deref().unwrap_or("unsaved"),
                observation.label
            ),
        );
        let open = self.integration.open_set();
        self.say(true, None, &format!("open_set now: {}", open.as_deref().unwrap_or("none")));
        Ok(())
    }

    /// A change as HISTORY last told of it.
    fn change(&self, id: &str) -> Option<ChangeRecord> {
        self.changes.borrow().iter().rev().find(|record| record.id == id).cloned()
    }

    /// M3: a plan of several changes through Kumi's own plan tool (the first track's fader, the second track renamed,
    /// a track added below it, that track muted), each in HISTORY with undo; then each undone through Kumi, last first:
    /// the mute and the fader put back by value, the added track by its deletion, the rename by value as a fresh step
    /// of Logic's undo. The project reads as before when it's done.
    async fn plan_and_undo(&self) -> Result<(), RuntimeError> {
        let observation = self.observe().await?;
        let context: Value = serde_json::from_str(&observation.context).unwrap_or(Value::Null);
        let tracks = context["tracks"].as_array().cloned().unwrap_or_default();
        let (Some(first), Some(second)) = (tracks.first().cloned(), tracks.get(1).or(tracks.first()).cloned()) else {
            self.say(false, None, "plan: the project has no track to change");
            return Ok(());
        };
        let (Some(plan), Some(undo)) = (tool_named(&observation, "make_changes"), tool_named(&observation, "undo_change")) else {
            self.say(false, None, "plan: no make_changes and undo_change tools (Logic Pro's change set is empty)");
            return Ok(());
        };
        let volume = first["volumeDb"].as_f64().unwrap_or(0.0);
        // The fader moved down 6 dB (up 3 when it is low already), the second track's name marked, a track added below
        // the last one and muted.
        let target = if volume > -60.0 { volume - 6.0 } else { volume + 3.0 };
        let (first_name, second_name) =
            (first["name"].as_str().unwrap_or("?").to_owned(), second["name"].as_str().unwrap_or("?").to_owned());
        let steps = json!({"steps": [
            {"tool": "track_volume", "input": {"ref": first["ref"], "volumeDb": target}},
            {"tool": "track_rename", "input": {"ref": second["ref"], "name": format!("{second_name} (Kumi)")}},
            {"tool": "track_add", "as": "pad", "input": {"name": "Kumi Pad", "kind": "instrument", "index": tracks.len().max(1)}},
            {"tool": "track_mute", "input": {"ref": "@pad", "mute": true}},
        ]});
        let heard = self.changes.borrow().len();
        let t0 = perf_now();
        let done = plan.execute(steps.as_object().cloned().unwrap_or_default(), signal()).await?;
        let ms = perf_now() - t0;
        // The changes HISTORY was told of by this plan, in order, each as it stands now.
        let mut ids: Vec<String> = Vec::new();
        for record in self.changes.borrow()[heard..].iter() {
            if !ids.contains(&record.id) {
                ids.push(record.id.clone());
            }
        }
        let rows: Vec<ChangeRecord> = ids.iter().filter_map(|id| self.change(id)).collect();
        let applied = rows.iter().filter(|row| row.state == ChangeState::Applied).count();
        let listed: Vec<String> = rows.iter().map(|row| format!("“{}” {}", row.title, state_word(row.state))).collect();
        let stopped = if done.is_error { format!("; the plan stopped: {}", short(&done.text, 1500)) } else { String::new() };
        self.say(
            !done.is_error && applied == 4,
            Some(ms),
            &format!("a plan of 4 changes: {} in HISTORY ({}){stopped}", rows.len(), listed.join("; ")),
        );
        // Each undone through Kumi, last first: the mute and the fader put back by value, the added track by its deletion,
        // the rename by value as a fresh step of Logic's undo.
        for row in rows.iter().rev().filter(|row| matches!(row.state, ChangeState::Applied | ChangeState::Unsure)) {
            let t0 = perf_now();
            let result = undo.execute(json!({"change": row.id}).as_object().cloned().unwrap_or_default(), signal()).await?;
            let undone = !result.is_error && self.change(&row.id).is_some_and(|now| now.state == ChangeState::Undone);
            let what = if undone { "undone".to_string() } else { short(&result.text, 200) };
            self.say(undone, Some(perf_now() - t0), &format!("undo “{}”: {what}", row.title));
        }
        // The project as it was, read again. Right after a structural change the bridge may still be settling its epoch
        // and refuse the look as busy, which Kumi's session answers by looking again once it's free: so does this.
        let t0 = perf_now();
        let mut context = Value::Null;
        for _ in 0..20 {
            let after = self.observe().await?;
            context = serde_json::from_str(&after.context).unwrap_or(Value::Null);
            if context["tracks"].as_array().is_some_and(|tracks| !tracks.is_empty()) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
        let waited = perf_now() - t0;
        let now = context["tracks"].as_array().cloned().unwrap_or_default();
        let volume_now = now.first().and_then(|track| track["volumeDb"].as_f64());
        let second_now = now.get(1).and_then(|track| track["name"].as_str()).unwrap_or("?").to_owned();
        let back = now.len() == tracks.len() && volume_now.is_some_and(|db| (db - volume).abs() < 0.05) && second_now == second_name;
        self.say(
            back,
            Some(waited),
            &format!(
                "the project reads as before the plan: {} tracks, “{first_name}” at {} dB, “{second_now}”",
                now.len(),
                volume_now.map(|db| db.to_string()).unwrap_or_else(|| "?".into())
            ),
        );
        Ok(())
    }

    /// A change tool of the observation run with `input`, by its name, or, for a kind that is a make_changes step and no
    /// tool of its own (the key command, the cycle), a plan of that one step: whether it answered an error, and its text.
    async fn run_tool(&self, observation: &Observation, name: &str, input: Value) -> Result<(bool, String), RuntimeError> {
        let (tool, arguments) = match tool_named(observation, name) {
            Some(tool) => (tool, input),
            None => match tool_named(observation, "make_changes") {
                Some(plan) => (plan, json!({"steps": [{"tool": name, "input": input}]})),
                None => return Ok((true, format!("no {name} tool, and no make_changes to run it as a step"))),
            },
        };
        let result = tool.execute(arguments.as_object().cloned().unwrap_or_default(), signal()).await?;
        Ok((result.is_error, result.text))
    }

    /// How many rows a kind has, read through the host (0 when the read fails).
    async fn count(&self, kind: &str) -> usize {
        self.tool("read", json!({"kind": kind, "limit": 1})).await.body["total"].as_u64().unwrap_or(0) as usize
    }

    /// The latest change HISTORY heard of since `from`, by its title's start.
    fn change_since(&self, from: usize, title_start: &str) -> Option<ChangeRecord> {
        let heard = self.changes.borrow();
        heard[from.min(heard.len())..].iter().rev().find(|record| record.title.starts_with(title_start)).cloned()
    }

    /// M4, the first slice: a save as to a copy under the folder the run was given (the project is that copy from then
    /// on, so the recorded one stays as it was), a key command Logic keeps no step of (no row) and one it does (a marker
    /// at the playhead, whose row only Logic's own undo takes back: Kumi's undo is refused in Logic's words, and the
    /// marker is taken away through Kumi by its deletion), a region split and its undo, a MIDI import of 64 notes read
    /// back and undone, the cycle set and cleared, a tempo change set and deleted in the Tempo List, each read back;
    /// then everything it made undone through Kumi, last first, and the project read as before.
    async fn m4_kinds(&self) -> Result<(), RuntimeError> {
        let observation = self.observe().await?;
        let (Some(plan), Some(_)) = (tool_named(&observation, "make_changes"), tool_named(&observation, "undo_change")) else {
            self.say(false, None, "M4: no make_changes and undo_change tools");
            return Ok(());
        };
        let context: Value = serde_json::from_str(&observation.context).unwrap_or(Value::Null);
        let tracks = context["tracks"].as_array().cloned().unwrap_or_default();
        let before = (tracks.len(), self.count("region").await, self.count("marker").await);
        let heard = self.changes.borrow().len();
        // The changes that only tidy up after a key command: not undone at the end.
        let mut tidied: Vec<String> = Vec::new();
        let run_plan = |steps: Value| plan.execute(json!({"steps": steps}).as_object().cloned().unwrap_or_default(), signal());

        // A save as under the folder given: the project is the copy from then on, read back from the host.
        match &self.files {
            None => self.say(true, None, "save as: skipped (no --files folder was given)"),
            Some(folder) => {
                let _ = std::fs::create_dir_all(folder);
                let name = self.status.borrow()["project"]["name"].as_str().unwrap_or("Project").to_owned();
                let copy = folder.join(format!("{name} Kumi M4 {}.logicx", chrono::Utc::now().format("%Y%m%d-%H%M%S")));
                let copy = copy.to_string_lossy().into_owned();
                let t0 = perf_now();
                let done = run_plan(json!([{"tool": "save_project", "input": {"path": copy}}])).await?;
                let ms = perf_now() - t0;
                let status = self.tool("status", json!({})).await;
                let project = self.tool("read", json!({"kind": "project"})).await;
                let paths = [status.body["project"]["path"].as_str(), project.body["items"][0]["path"].as_str()];
                let named = paths.into_iter().flatten().any(|path| path == copy);
                let what = if done.is_error {
                    format!("save as {copy}: {}", short(&done.text, 200))
                } else if named {
                    format!("save as {copy}: the project is the copy now (status and read)")
                } else {
                    format!(
                        "save as {copy}: saved, but the project reads as {} (status), {} (read)",
                        paths[0].unwrap_or("?"),
                        paths[1].unwrap_or("?")
                    )
                };
                self.say(!done.is_error && named, Some(ms), &what);
            }
        }
        if self.stopping.get() {
            return Ok(());
        }
        // The save as made the copy the project: the tools from here on are a fresh observation's, since a plan from
        // the one before is refused as made for the project before it changed (the M4 gate's Select All).
        let observation = self.observe().await?;
        let (Some(plan), Some(undo)) = (tool_named(&observation, "make_changes"), tool_named(&observation, "undo_change")) else {
            self.say(false, None, "M4: no make_changes and undo_change tools after the save as");
            return Ok(());
        };
        let run_plan = |steps: Value| plan.execute(json!({"steps": steps}).as_object().cloned().unwrap_or_default(), signal());

        // A key command Logic keeps no step of: no row.
        let rows = self.changes.borrow().len();
        let t0 = perf_now();
        let (is_error, text) = self.run_tool(&observation, "key_command", json!({"command": "Select All"})).await?;
        let no_row = self.changes.borrow().len() == rows;
        let what = if is_error {
            format!("key command Select All: {}", short(&text, 200))
        } else if no_row {
            "key command Select All: no row (Logic Pro kept no step of it)".into()
        } else {
            format!("key command Select All: a row was made: {}", short(&text, 200))
        };
        self.say(!is_error && no_row, Some(perf_now() - t0), &what);

        // One it does: a marker at the playhead, a bar past every region; its row is Logic's own undo's, so Kumi's undo
        // of it is refused and the marker is taken away through Kumi by its deletion.
        let regions = self.tool("read", json!({"kind": "region", "limit": 200})).await;
        let last_end =
            regions.body["items"].as_array().into_iter().flatten().filter_map(|region| region["endBeats"].as_f64()).fold(0.0, f64::max);
        let at = ((last_end / 4.0).floor() + 2.0) * 4.0;
        let moved = run_plan(json!([{"tool": "set_cursor", "input": {"positionBeats": at}}])).await?;
        let markers = self.count("marker").await;
        let rows = self.changes.borrow().len();
        let t0 = perf_now();
        let (is_error, text) = self.run_tool(&observation, "key_command", json!({"command": "Create Marker"})).await?;
        let ms = perf_now() - t0;
        let markers_now = self.count("marker").await;
        let made = self.change_since(rows, "Ran key command");
        let what = if is_error || moved.is_error {
            format!("key command Create Marker at beat {at}: {}", short(if is_error { &text } else { &moved.text }, 200))
        } else {
            format!(
                "key command Create Marker at beat {at}: {markers} → {markers_now} markers, row {}",
                made.as_ref().map(|row| format!("“{}” {}", row.title, state_word(row.state))).unwrap_or_else(|| "missing".into())
            )
        };
        self.say(!is_error && !moved.is_error && markers_now == markers + 1 && made.is_some(), Some(ms), &what);
        if let Some(made) = made {
            let t0 = perf_now();
            let refused = undo.execute(json!({"change": made.id}).as_object().cloned().unwrap_or_default(), signal()).await?;
            let kept = self.change(&made.id).is_some_and(|row| row.state == ChangeState::Kept);
            self.say(
                refused.is_error && refused.text.contains("Logic Pro's own undo") && kept,
                Some(perf_now() - t0),
                &format!("Kumi's undo of the key command is refused, the row kept: {}", short(&refused.text, 200)),
            );
            // The marker it made, found by its place, taken away through Kumi. The look names it by a short ref of its own
            // (counted over the session, not the marker's place), so it is found there by its place too.
            let at_beat = |row: &Value| {
                let bar = row["position"]["bar"].as_f64().unwrap_or(0.0);
                let beat = row["position"]["beat"].as_f64().unwrap_or(0.0);
                ((bar - 1.0) * 4.0 + (beat - 1.0) - at).abs() < 0.5
            };
            let page = self.tool("read", json!({"kind": "marker", "limit": 200})).await;
            let marker = page.body["items"].as_array().into_iter().flatten().find(|row| at_beat(row));
            match marker.and_then(|row| row["ref"].as_str().map(str::to_owned)) {
                Some(_) => {
                    let look = self.observe().await?;
                    let marker_ref = serde_json::from_str::<Value>(&look.context).ok().and_then(|context| {
                        context["markers"]
                            .as_array()
                            .into_iter()
                            .flatten()
                            .find(|row| at_beat(row))
                            .and_then(|row| row["ref"].as_str().map(str::to_owned))
                    });
                    let rows = self.changes.borrow().len();
                    let t0 = perf_now();
                    let done = match marker_ref {
                        Some(short) => Some(
                            tool_named(&look, "make_changes")
                                .unwrap_or(plan.clone())
                                .execute(
                                    json!({"steps": [{"tool": "marker_delete", "input": {"ref": short}}]})
                                        .as_object()
                                        .cloned()
                                        .unwrap_or_default(),
                                    signal(),
                                )
                                .await?,
                        ),
                        None => None,
                    };
                    let deleted = done.as_ref().is_some_and(|done| !done.is_error) && self.count("marker").await == markers;
                    for record in self.changes.borrow()[rows..].iter() {
                        if !tidied.contains(&record.id) {
                            tidied.push(record.id.clone());
                        }
                    }
                    let what = match &done {
                        Some(done) if !done.is_error => {
                            format!("the marker taken away through Kumi (marker_delete, not undone at the end): {markers} markers again")
                        }
                        Some(done) => format!("the marker's deletion through Kumi: {}", short(&done.text, 200)),
                        None => "the marker isn't in the look's markers to delete it through Kumi: left for Cmd-Z in Logic Pro".into(),
                    };
                    self.say(deleted, Some(perf_now() - t0), &what);
                }
                None => self.say(false, None, &format!("no marker at beat {at} to take away: left for Cmd-Z in Logic Pro")),
            }
        }
        if self.stopping.get() {
            return Ok(());
        }

        // A region split at its middle, the right part's ref named; whole again through Kumi's undo.
        let look = self.observe().await?;
        let page = match tool_named(&look, "logic_read") {
            Some(read) => serde_json::from_str::<Value>(
                &read.execute(json!({"kind": "region", "limit": 200}).as_object().cloned().unwrap_or_default(), signal()).await?.text,
            )
            .unwrap_or(Value::Null),
            None => Value::Null,
        };
        let region = page["items"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|row| row["endBeats"].as_f64().unwrap_or(0.0) - row["startBeats"].as_f64().unwrap_or(0.0) >= 2.0)
            .cloned();
        match region {
            None => self.say(false, None, "region split: no region of two beats or more to split"),
            Some(region) => {
                let (start, end) = (region["startBeats"].as_f64().unwrap_or(0.0), region["endBeats"].as_f64().unwrap_or(0.0));
                let cut = ((start + end) / 2.0).floor().clamp(start + 1.0, end - 1.0);
                let name = region["name"].as_str().unwrap_or("?").to_owned();
                let regions = self.count("region").await;
                let rows = self.changes.borrow().len();
                let t0 = perf_now();
                let (is_error, text) = self.run_tool(&look, "region_split", json!({"ref": region["ref"], "positionBeats": cut})).await?;
                let ms = perf_now() - t0;
                let created = serde_json::from_str::<Value>(&text).ok().and_then(|answer| answer["ref"].as_str().map(str::to_owned));
                let regions_now = self.count("region").await;
                let what = if is_error {
                    format!("region split “{name}” at beat {cut}: {}", short(&text, 200))
                } else {
                    format!(
                        "region split “{name}” at beat {cut}: {regions} → {regions_now} regions, the right part {}",
                        created.as_deref().unwrap_or("unnamed")
                    )
                };
                self.say(!is_error && regions_now == regions + 1 && created.is_some(), Some(ms), &what);
                if let Some(row) = self.change_since(rows, "Split region") {
                    let t0 = perf_now();
                    let result = undo.execute(json!({"change": row.id}).as_object().cloned().unwrap_or_default(), signal()).await?;
                    let whole = !result.is_error && self.count("region").await == regions;
                    let note = self.change(&row.id).and_then(|row| row.note).map(|note| format!(" ({note})")).unwrap_or_default();
                    self.say(
                        whole,
                        Some(perf_now() - t0),
                        &format!(
                            "undo the split: {}{note}",
                            if whole { format!("{regions} regions again") } else { short(&result.text, 200) }
                        ),
                    );
                }
            }
        }
        if self.stopping.get() {
            return Ok(());
        }

        // A MIDI import of 64 sixteenths onto a software instrument track, a bar past every region, read back; undone.
        // Logic lands the file on a new track below the one named. The bridge's note read lists only the Event List rows
        // Logic shows (33 of 64 at the M4 gate), and its apply says so in the change's note ("lists its first N rows"), so
        // the import counts when its region spans the 64 sixteenths (16 beats) and either the read holds the 64 notes or
        // the answer says the Event List listed fewer rows.
        let look = self.observe().await?;
        let context: Value = serde_json::from_str(&look.context).unwrap_or(Value::Null);
        let target = context["tracks"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|track| track["kind"] == "instrument")
            .or_else(|| context["tracks"].as_array().into_iter().flatten().find(|track| track["kind"] == "external"))
            .cloned();
        match target {
            None => self.say(false, None, "MIDI import: no software instrument or external MIDI track to import onto"),
            Some(track) => {
                let notes: Vec<Value> =
                    (0..64).map(|n| json!({"pitch": 48 + n % 12, "start": n as f64 * 0.25, "length": 0.25, "velocity": 96})).collect();
                let name = track["name"].as_str().unwrap_or("?").to_owned();
                let regions = self.count("region").await;
                let rows = self.changes.borrow().len();
                let t0 = perf_now();
                let (is_error, text) =
                    self.run_tool(&look, "midi_import", json!({"ref": track["ref"], "positionBeats": at, "notes": notes})).await?;
                let ms = perf_now() - t0;
                // The change tool's text: {"changed": "<title> (<the apply's note>)", "change": <its id>, "ref": <the region,
                // short>}; the note names the Event List's rows when it listed fewer than the file's notes.
                let created = serde_json::from_str::<Value>(&text).ok().and_then(|answer| answer["ref"].as_str().map(str::to_owned));
                let listed = text.split("lists its first ").nth(1).and_then(|rest| rest.split(' ').next()?.parse::<u64>().ok());
                let t1 = perf_now();
                let (held, span) = match (&created, tool_named(&look, "logic_read")) {
                    (Some(created), Some(read)) => {
                        let page = |input: Value| read.execute(input.as_object().cloned().unwrap_or_default(), signal());
                        let notes = page(json!({"kind": "note", "parent": created, "limit": 200})).await?;
                        let held = serde_json::from_str::<Value>(&notes.text).ok().and_then(|page| page["total"].as_u64()).unwrap_or(0);
                        let made = page(json!({"kind": "region", "limit": 200})).await?;
                        let span = serde_json::from_str::<Value>(&made.text)
                            .ok()
                            .and_then(|page| {
                                let row =
                                    page["items"].as_array()?.iter().find(|row| row["ref"].as_str() == Some(created.as_str()))?.clone();
                                Some(row["endBeats"].as_f64()? - row["startBeats"].as_f64()?)
                            })
                            .unwrap_or(0.0);
                        (held, span)
                    }
                    _ => (0, 0.0),
                };
                let read_ms = perf_now() - t1;
                let spans = span + 1e-6 >= 16.0;
                let shown = match listed {
                    _ if held == 64 => " holds 64 notes".to_string(),
                    Some(shown_rows) => format!(": the Event List listed {shown_rows} of 64 rows; the region spans {span} beats"),
                    None => format!(" holds {held} notes; the region spans {span} beats"),
                };
                let what = if is_error {
                    format!("MIDI import of 64 notes on “{name}” at beat {at}: {}", short(&text, 200))
                } else {
                    format!(
                        "MIDI import of 64 notes on “{name}” at beat {at}: region {}{shown} (read back in {}), {regions} → {} regions",
                        created.as_deref().unwrap_or("unnamed"),
                        seconds(read_ms),
                        self.count("region").await
                    )
                };
                self.say(!is_error && spans && (held == 64 || listed.is_some_and(|shown_rows| shown_rows < 64)), Some(ms), &what);
                if let Some(row) = self.change_since(rows, "Imported") {
                    let t0 = perf_now();
                    let result = undo.execute(json!({"change": row.id}).as_object().cloned().unwrap_or_default(), signal()).await?;
                    let gone = !result.is_error && self.count("region").await == regions;
                    let note = self.change(&row.id).and_then(|row| row.note).map(|note| format!(" ({note})")).unwrap_or_default();
                    self.say(
                        gone,
                        Some(perf_now() - t0),
                        &format!(
                            "undo the import: {}{note}",
                            if gone { format!("{regions} regions again") } else { short(&result.text, 200) }
                        ),
                    );
                }
            }
        }
        if self.stopping.get() {
            return Ok(());
        }

        // The cycle set, then cleared, each read back from the focus's transport.
        let look = self.observe().await?;
        let t0 = perf_now();
        let (is_error, text) = self.run_tool(&look, "loop_range", json!({"startBeats": 0, "endBeats": 16})).await?;
        let cycle = self.tool("focus", json!({})).await.body["transport"]["cycle"].clone();
        self.say(
            !is_error && cycle == true,
            Some(perf_now() - t0),
            &format!(
                "cycle set, bar 1 to bar 5: the focus's transport says cycle {cycle}{}",
                if is_error { format!(": {}", short(&text, 200)) } else { String::new() }
            ),
        );
        let t0 = perf_now();
        let (is_error, text) = self.run_tool(&look, "loop_range", json!({"startBeats": 0, "endBeats": 0})).await?;
        let cycle = self.tool("focus", json!({})).await.body["transport"]["cycle"].clone();
        self.say(
            !is_error && cycle == false,
            Some(perf_now() - t0),
            &format!(
                "cycle cleared: the focus's transport says cycle {cycle}{}",
                if is_error { format!(": {}", short(&text, 200)) } else { String::new() }
            ),
        );

        // A tempo change in the Tempo List, set and deleted, each read back from the list itself (the tempo row holds
        // the project's tempo and signatures only).
        let bpm = (context["project"]["tempo"].as_f64().unwrap_or(120.0).round() + 8.0).min(990.0);
        let t0 = perf_now();
        let done = run_plan(json!([{"tool": "tempo_marker_set", "input": {"positionBeats": at, "bpm": bpm}}])).await?;
        let listed = self.tempo_listed(at).await;
        self.say(
            !done.is_error && listed.as_ref().is_ok_and(|shown| *shown == Some(bpm)),
            Some(perf_now() - t0),
            &format!(
                "tempo {bpm} BPM at beat {at} in the Tempo List: {}; the list {}",
                if done.is_error { short(&done.text, 200) } else { "set".into() },
                match &listed {
                    Ok(Some(shown)) => format!("shows {shown} BPM there"),
                    Ok(None) => "has no tempo event there".into(),
                    Err(why) => format!("couldn't be read ({why})"),
                }
            ),
        );
        let t0 = perf_now();
        let done = run_plan(json!([{"tool": "tempo_marker_delete", "input": {"positionBeats": at}}])).await?;
        let listed = self.tempo_listed(at).await;
        self.say(
            !done.is_error && matches!(listed, Ok(None)),
            Some(perf_now() - t0),
            &format!(
                "the tempo change at beat {at} deleted: {}; the list {}",
                if done.is_error { short(&done.text, 200) } else { "deleted".into() },
                match &listed {
                    Ok(Some(shown)) => format!("still shows {shown} BPM there"),
                    Ok(None) => "has no tempo event there".into(),
                    Err(why) => format!("couldn't be read ({why})"),
                }
            ),
        );

        // Everything M4 made, undone through Kumi, last first: the cycle's two changes and the Tempo List's two (the
        // split and the import are undone already, the key command's row is Logic's, the tidy-ups stay).
        let mut ids: Vec<String> = Vec::new();
        for record in self.changes.borrow()[heard..].iter() {
            if !ids.contains(&record.id) && !tidied.contains(&record.id) {
                ids.push(record.id.clone());
            }
        }
        for id in ids.iter().rev() {
            let Some(row) = self.change(id) else { continue };
            if !matches!(row.state, ChangeState::Applied | ChangeState::Unsure) {
                continue;
            }
            let t0 = perf_now();
            let result = undo.execute(json!({"change": id}).as_object().cloned().unwrap_or_default(), signal()).await?;
            let undone = !result.is_error && self.change(id).is_some_and(|now| now.state == ChangeState::Undone);
            let what = if undone { "undone".to_string() } else { short(&result.text, 200) };
            self.say(undone, Some(perf_now() - t0), &format!("undo “{}”: {what}", row.title));
        }
        let after = (self.observe().await?.context, self.count("region").await, self.count("marker").await);
        let tracks_now =
            serde_json::from_str::<Value>(&after.0).ok().and_then(|context| context["tracks"].as_array().map(Vec::len)).unwrap_or(0);
        let same = (tracks_now, after.1, after.2) == before;
        self.say(
            same,
            None,
            &format!(
                "the project reads as before M4: {tracks_now} tracks, {} regions, {} markers (before: {} tracks, {} regions, {} markers)",
                after.1, after.2, before.0, before.1, before.2
            ),
        );
        Ok(())
    }

    /// The rows of a read through the turn's read tool (short refs), or none when it failed.
    async fn rows(&self, read: &Rc<dyn kumi_runtime::core::contracts::KernelTool>, input: Value) -> Result<Vec<Value>, RuntimeError> {
        let result = read.execute(input.as_object().cloned().unwrap_or_default(), signal()).await?;
        let page: Value = serde_json::from_str(&result.text).unwrap_or(Value::Null);
        Ok(if result.is_error { vec![] } else { page["items"].as_array().cloned().unwrap_or_default() })
    }

    /// M4, the second slice: a track's Smart Controls and automation read; a Gain inserted on the Stereo Out (which the
    /// observation's outputs name: no read lists it) and its gain set through the plug-in window's Controls View, read
    /// back as a parameter; a send from track 1 to a bus nothing takes (Logic makes an aux for it, which the answer
    /// names); a patch from the Library on the first software instrument track; a bounce of bars 1 to 5 under the folder
    /// the run was given, found on disk with its length and deleted after; then everything it made undone through Kumi,
    /// last first, and the project read as before.
    async fn m4b_kinds(&self) -> Result<(), RuntimeError> {
        let observation = self.observe().await?;
        let (Some(undo), Some(read)) = (tool_named(&observation, "undo_change"), tool_named(&observation, "logic_read")) else {
            self.say(false, None, "M4b: no undo_change and logic_read tools");
            return Ok(());
        };
        let context: Value = serde_json::from_str(&observation.context).unwrap_or(Value::Null);
        let tracks = context["tracks"].as_array().cloned().unwrap_or_default();
        let heard = self.changes.borrow().len();

        // The new reads: a software instrument track's Smart Controls (an audio track with nothing on its strip has
        // none) and the first track's automation.
        let instrument = tracks.iter().position(|track| track["kind"] == "instrument");
        let reads = [("smart", instrument), ("automation", (!tracks.is_empty()).then_some(0))];
        for (kind, at) in reads {
            let Some((place, parent)) = at.and_then(|at| tracks[at]["ref"].as_str().map(|parent| (at + 1, parent.to_owned()))) else {
                self.say(false, None, &format!("read {kind}: no track to read it of"));
                continue;
            };
            let t0 = perf_now();
            let result = read.execute(json!({"kind": kind, "parent": parent}).as_object().cloned().unwrap_or_default(), signal()).await?;
            let page: Value = serde_json::from_str(&result.text).unwrap_or(Value::Null);
            let what = if result.is_error {
                format!("read {kind}: {}", short(&result.text, 200))
            } else {
                format!("read {kind} of track {place}: {} rows", page["total"])
            };
            self.say(!result.is_error, Some(perf_now() - t0), &what);
        }

        // A Gain at the end of the Stereo Out, its gain set to -3 dB through the Controls View and read back as a
        // parameter.
        let stereo_out = context["outputs"].as_array().into_iter().flatten().find(|output| output["name"] == "Stereo Out").cloned();
        let mut output_plugins = 0;
        match stereo_out.as_ref().and_then(|output| output["ref"].as_str().map(str::to_owned)) {
            None => self.say(
                false,
                None,
                &format!("a Gain on the Stereo Out: the observation names no Stereo Out ({})", short(&context["outputs"].to_string(), 120)),
            ),
            Some(stereo_out) => {
                output_plugins = self.rows(&read, json!({"kind": "plugin", "parent": stereo_out, "limit": 200})).await?.len();
                let t0 = perf_now();
                let (is_error, text) = self.run_tool(&observation, "fx_add", json!({"ref": stereo_out, "name": "Gain"})).await?;
                let gain = made_ref(&text);
                self.say(
                    !is_error && gain.is_some(),
                    Some(perf_now() - t0),
                    &if is_error {
                        format!("a Gain on the Stereo Out ({stereo_out}): {}", short(&text, 200))
                    } else {
                        format!("a Gain inserted on the Stereo Out ({stereo_out}): {}", gain.as_deref().unwrap_or("no ref"))
                    },
                );
                if let Some(gain) = gain {
                    let t0 = perf_now();
                    let (is_error, text) =
                        self.run_tool(&observation, "fx_param", json!({"ref": gain, "param": "Gain", "value": -3})).await?;
                    let ms = perf_now() - t0;
                    let params = self.rows(&read, json!({"kind": "param", "parent": gain, "limit": 200})).await?;
                    let row = params.iter().find(|row| row["name"] == "Gain").cloned().unwrap_or(Value::Null);
                    let reads = row["text"].as_str().map(|text| text.replace('\u{2212}', "-"));
                    let db = reads
                        .as_deref()
                        .and_then(|text| text.split_whitespace().next())
                        .and_then(|n| n.trim_start_matches('+').parse::<f64>().ok());
                    let what = if is_error {
                        format!("its Gain set to -3 dB: {}", short(&text, 200))
                    } else {
                        format!(
                            "its Gain set to -3 dB: the Controls View reads {} ({} parameters read)",
                            reads.as_deref().unwrap_or("nothing"),
                            params.len()
                        )
                    };
                    self.say(!is_error && db.is_some_and(|db| (db + 3.0).abs() < 0.05), Some(ms), &what);
                }
            }
        }
        if self.stopping.get() {
            return Ok(());
        }

        // A send from track 1 to the lowest bus nothing on the tracks takes (a summing stack's, track 1's sends): Logic
        // makes an aux for a bus nothing takes, which the answer names.
        match tracks.first().and_then(|track| track["ref"].as_str().map(str::to_owned)) {
            None => self.say(false, None, "a send: the project has no track"),
            Some(first) => {
                let sends_before = self.rows(&read, json!({"kind": "send", "parent": first})).await?;
                let taken: Vec<String> = tracks
                    .iter()
                    .flat_map(|row| row["inputs"].as_array().cloned().unwrap_or_default())
                    .filter_map(|input| input.as_str().map(str::to_owned))
                    .chain(sends_before.iter().filter_map(|send| send["destination"].as_str().map(str::to_owned)))
                    .collect();
                let bus = (1..=64).find(|bus| !taken.contains(&format!("Bus {bus}"))).unwrap_or(1);
                let t0 = perf_now();
                let (is_error, text) = self.run_tool(&observation, "send_add", json!({"ref": first, "bus": bus, "volumeDb": -12})).await?;
                let ms = perf_now() - t0;
                let look = self.observe().await?;
                let first_now = serde_json::from_str::<Value>(&look.context)
                    .ok()
                    .and_then(|context| context["tracks"][0]["ref"].as_str().map(str::to_owned));
                let sends = match (&first_now, tool_named(&look, "logic_read")) {
                    (Some(first_now), Some(read)) => self.rows(&read, json!({"kind": "send", "parent": first_now})).await?,
                    _ => vec![],
                };
                let made = sends.iter().find(|send| send["destination"] == format!("Bus {bus}").as_str()).cloned();
                let aux = text.contains("made the aux");
                let what = if is_error {
                    format!("a send from track 1 to Bus {bus}: {}", short(&text, 200))
                } else {
                    format!(
                        "a send from track 1 to Bus {bus} at -12 dB: {} send(s) on track 1, this one at {} dB{}",
                        sends.len(),
                        made.as_ref().map_or("?".into(), |send| send["level"].to_string()),
                        if aux { " (Logic Pro made an aux for the bus)" } else { "" }
                    )
                };
                self.say(
                    !is_error && made.is_some_and(|send| send["level"].as_f64().is_some_and(|db| (db + 12.0).abs() < 0.55)),
                    Some(ms),
                    &what,
                );
            }
        }
        if self.stopping.get() {
            return Ok(());
        }

        // A patch from the Library on the first software instrument track, its strip read back.
        let look = self.observe().await?;
        let context: Value = serde_json::from_str(&look.context).unwrap_or(Value::Null);
        let instrument = context["tracks"].as_array().into_iter().flatten().find(|track| track["kind"] == "instrument").cloned();
        match instrument {
            None => self.say(false, None, "a patch: no software instrument track to load it on"),
            Some(track) => {
                let name = track["name"].as_str().unwrap_or("?").to_owned();
                let t0 = perf_now();
                let (is_error, text) = self.run_tool(&look, "patch_load", json!({"ref": track["ref"], "patch": self.patch})).await?;
                let ms = perf_now() - t0;
                let plugins = match tool_named(&look, "logic_read") {
                    Some(read) => self.rows(&read, json!({"kind": "plugin", "parent": track["ref"]})).await?,
                    None => vec![],
                };
                let names: Vec<&str> = plugins.iter().filter_map(|row| row["name"].as_str()).collect();
                let what = if is_error {
                    format!("patch {} on “{name}”: {}", self.patch, short(&text, 200))
                } else {
                    format!(
                        "patch {} on “{name}”: the strip holds {}",
                        self.patch,
                        if names.is_empty() { "nothing".into() } else { names.join(", ") }
                    )
                };
                self.say(!is_error && !names.is_empty(), Some(ms), &what);
            }
        }
        if self.stopping.get() {
            return Ok(());
        }

        // A bounce of bars 1 to 5 under the folder given: found on disk, its length the range's, then deleted.
        match &self.files {
            None => self.say(true, None, "bounce: skipped (no --files folder was given)"),
            Some(folder) => {
                let _ = std::fs::create_dir_all(folder);
                let name = self.status.borrow()["project"]["name"].as_str().unwrap_or("Project").to_owned();
                let path = folder.join(format!("{name} Kumi M4b bars 1 to 5 {}.wav", chrono::Utc::now().format("%Y%m%d-%H%M%S")));
                let bpm = context["project"]["tempo"].as_f64().unwrap_or(120.0);
                let expected = 16.0 * 60.0 / bpm;
                let rows = self.changes.borrow().len();
                let t0 = perf_now();
                let (is_error, text) =
                    self.run_tool(&look, "bounce", json!({"startBeats": 0, "endBeats": 16, "path": path.to_string_lossy()})).await?;
                let ms = perf_now() - t0;
                let seconds = wav_seconds(&path);
                let no_row = self.changes.borrow().len() == rows;
                let what = if is_error {
                    format!("bounce of bars 1 to 5: {}", short(&text, 200))
                } else {
                    format!(
                        "bounce of bars 1 to 5: {} found, {} s long ({expected:.2} s at {bpm} BPM), {}",
                        path.display(),
                        seconds.map(|s| format!("{s:.2}")).unwrap_or_else(|| "not".into()),
                        if no_row { "no row" } else { "a row was made" }
                    )
                };
                self.say(!is_error && no_row && seconds.is_some_and(|s| (s - expected).abs() < 0.05), Some(ms), &what);
                if std::fs::remove_file(&path).is_ok() {
                    println!("        (the bounce is deleted, checked)");
                }
            }
        }

        // Everything the second slice made, undone through Kumi, last first: the patch (through Logic's own undo, its
        // step on top), the send (its aux with it, through Logic's own undo once the patch's step is off), the gain and
        // the Gain.
        let mut ids: Vec<String> = Vec::new();
        for record in self.changes.borrow()[heard..].iter() {
            if !ids.contains(&record.id) {
                ids.push(record.id.clone());
            }
        }
        for id in ids.iter().rev() {
            let Some(row) = self.change(id) else { continue };
            if !matches!(row.state, ChangeState::Applied | ChangeState::Unsure) {
                continue;
            }
            let t0 = perf_now();
            let result = undo.execute(json!({"change": id}).as_object().cloned().unwrap_or_default(), signal()).await?;
            let undone = !result.is_error && self.change(id).is_some_and(|now| now.state == ChangeState::Undone);
            let what = if undone { "undone".to_string() } else { short(&result.text, 200) };
            self.say(undone, Some(perf_now() - t0), &format!("undo “{}”: {what}", row.title));
        }
        let look = self.observe().await?;
        let context: Value = serde_json::from_str(&look.context).unwrap_or(Value::Null);
        let tracks_now = context["tracks"].as_array().map(Vec::len).unwrap_or(0);
        let output_plugins_now = match (context["outputs"][0]["ref"].as_str(), tool_named(&look, "logic_read")) {
            (Some(stereo_out), Some(read)) => self.rows(&read, json!({"kind": "plugin", "parent": stereo_out, "limit": 200})).await?.len(),
            _ => 0,
        };
        self.say(
            output_plugins_now == output_plugins && tracks_now == tracks.len(),
            None,
            &format!(
                "the project reads as before the second slice: {tracks_now} tracks, {output_plugins_now} plug-ins on the Stereo Out (before: {} tracks, {output_plugins})",
                tracks.len()
            ),
        );
        Ok(())
    }

    /// M5, listening: the listening tools offered; bars 1 to 5 of the mix heard (the file's length, and its loudness
    /// measured); a judged run toward 3 LU above what was heard (the loudness alone its target), the first software
    /// instrument track up 1 dB, and the round that brings the level back with a Gain at the end of the Stereo Out, read
    /// back as a parameter, then each of the run's changes undone through Kumi, last first; tracks 1 and 2 auditioned, a
    /// pass each; a render of bars 1 to 5 of the instrument track found on disk with its length, then deleted; and the
    /// project read as before (its tracks, their faders and solos, the Stereo Out's plug-ins).
    async fn m5_listening(&self) -> Result<(), RuntimeError> {
        let observation = self.observe().await?;
        let (Some(read), Some(undo)) = (tool_named(&observation, "logic_read"), tool_named(&observation, "undo_change")) else {
            self.say(false, None, "M5: no logic_read and undo_change tools");
            return Ok(());
        };
        let context: Value = serde_json::from_str(&observation.context).unwrap_or(Value::Null);
        let tracks = context["tracks"].as_array().cloned().unwrap_or_default();
        let bpm = context["project"]["tempo"].as_f64().unwrap_or(120.0);
        let expected = 16.0 * 60.0 / bpm;
        let stereo_out = |context: &Value| {
            context["outputs"]
                .as_array()
                .into_iter()
                .flatten()
                .find(|output| output["name"] == "Stereo Out")
                .and_then(|output| output["ref"].as_str().map(str::to_owned))
        };
        let output_plugins = match stereo_out(&context) {
            Some(strip) => self.rows(&read, json!({"kind": "plugin", "parent": strip, "limit": 200})).await?.len(),
            None => 0,
        };
        // Each track's name, fader and solo, as a look reads them.
        let mixer = |tracks: &[Value]| -> Vec<(String, f64, bool)> {
            tracks
                .iter()
                .map(|track| {
                    (
                        track["name"].as_str().unwrap_or_default().to_owned(),
                        track["volumeDb"].as_f64().unwrap_or(0.0),
                        track["solo"] == true,
                    )
                })
                .collect()
        };
        let before = mixer(&tracks);
        let heard = self.changes.borrow().len();
        // A round's reply as its log's lines.
        let log = |text: &str| {
            serde_json::from_str::<Value>(text)
                .ok()
                .and_then(|reply| {
                    reply["log"].as_array().map(|lines| lines.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(" / "))
                })
                .unwrap_or_else(|| short(text, 300))
        };

        // The listening tools, and what hears for the listen tool.
        let names = ["audition", "judge", "tune", "groove", "form", "sound", "render"];
        let offered: Vec<&str> = names.iter().copied().filter(|name| tool_named(&observation, name).is_some()).collect();
        let (hears, auditions) = (self.integration.has_hear(), self.integration.has_audition());
        self.say(
            offered.len() == names.len() && hears && auditions,
            None,
            &format!("the listening tools: {} (hear {hears}, audition {auditions})", offered.join(", ")),
        );

        // Bars 1 to 5 of the mix: Logic plays them while the helper taps what it outputs, cut to the part.
        let t0 = perf_now();
        let mix = HearRequest { mix: Some(true), from_beat: Some(0.0), beats: Some(16.0), ..HearRequest::default() };
        let mut loudness = None;
        match self.integration.hear(&mix, listening()).await? {
            Ok(takes) => {
                let ms = perf_now() - t0;
                match takes.first() {
                    Some(take) => {
                        let file = wav_seconds(Path::new(&take.file));
                        let measured =
                            measure_file(&take.file, MeasureOptions { start: Some(take.start), seconds: take.seconds, signal: None }).await;
                        loudness = measured.as_ref().ok().and_then(|heard| heard.measures.integrated);
                        let part = take.seconds.unwrap_or(0.0);
                        let ok = (part - expected).abs() < 0.05
                            && file.is_some_and(|file| (file - take.start - part).abs() < 0.05)
                            && loudness.is_some_and(|loudness| loudness > -70.0);
                        self.say(
                            ok,
                            Some(ms),
                            &format!(
                                "hear bars 1 to 5: {part:.3} s of the mix ({expected:.2} s at {bpm} BPM; the file {}, {:.3} s before the part), {}",
                                file.map(|file| format!("{file:.3} s")).unwrap_or_else(|| "unread".into()),
                                take.start,
                                loudness.map(|loudness| format!("{loudness:.1} LUFS")).unwrap_or_else(|| "no loudness measured".into())
                            ),
                        );
                    }
                    None => self.say(false, Some(ms), "hear bars 1 to 5: no take came back"),
                }
            }
            Err(why) => self.say(false, Some(perf_now() - t0), &format!("hear bars 1 to 5: {}", short(&why, 200))),
        }
        if self.stopping.get() {
            return Ok(());
        }

        // A judged run toward 3 LU above what was heard, the first software instrument track up 1 dB, and the round that
        // brings the level back to the goal with a Gain at the end of the Stereo Out.
        let instrument = tracks.iter().find(|track| track["kind"] == "instrument").cloned();
        match (loudness, tool_named(&observation, "judge"), instrument) {
            (Some(loudness), Some(judge), Some(track)) => {
                let goal = ((loudness + 3.0) * 2.0).round() / 2.0;
                let t0 = perf_now();
                let start = json!({"goal": {"loudness": goal, "problems": false}, "from_beat": 0, "beats": 16});
                let started = judge.execute(start.as_object().cloned().unwrap_or_default(), listening()).await?;
                self.say(
                    !started.is_error,
                    Some(perf_now() - t0),
                    &format!("judge: a run toward {goal} LUFS starts: {}", short(&log(&started.text), 240)),
                );
                if !started.is_error {
                    let name = track["name"].as_str().unwrap_or("?").to_owned();
                    let fader = track["volumeDb"].as_f64().unwrap_or(0.0);
                    let (is_error, text) =
                        self.run_tool(&observation, "track_volume", json!({"ref": track["ref"], "volumeDb": fader + 1.0})).await?;
                    self.say(!is_error, None, &format!("“{name}” up 1 dB: {}", short(&text, 160)));
                    let t0 = perf_now();
                    let change = json!({"change": format!("{name} up 1 dB")});
                    let round = judge.execute(change.as_object().cloned().unwrap_or_default(), listening()).await?;
                    let said = log(&round.text);
                    self.say(
                        !round.is_error && said.contains("the Stereo Out"),
                        Some(perf_now() - t0),
                        &format!("the round: {}", short(&said, 320)),
                    );
                    // The Gain it put at the end of the Stereo Out, read back as a parameter.
                    let look = self.observe().await?;
                    let context: Value = serde_json::from_str(&look.context).unwrap_or(Value::Null);
                    let reads = match (stereo_out(&context), tool_named(&look, "logic_read")) {
                        (Some(strip), Some(read)) => {
                            let plugins = self.rows(&read, json!({"kind": "plugin", "parent": strip, "limit": 200})).await?;
                            match plugins.last().filter(|row| row["name"] == "Gain").and_then(|row| row["ref"].as_str()) {
                                Some(gain) => self
                                    .rows(&read, json!({"kind": "param", "parent": gain, "limit": 200}))
                                    .await?
                                    .iter()
                                    .find(|row| row["name"] == "Gain")
                                    .and_then(|row| row["text"].as_str().map(str::to_owned)),
                                None => None,
                            }
                        }
                        _ => None,
                    };
                    self.say(
                        reads.is_some(),
                        None,
                        &format!(
                            "a Gain at the end of the Stereo Out: its Gain reads {}",
                            reads.as_deref().unwrap_or("nothing (no Gain there)")
                        ),
                    );
                }
            }
            (None, ..) => self.say(false, None, "judge: no loudness was heard to aim above"),
            (_, None, _) => self.say(false, None, "judge: no judge tool"),
            (_, _, None) => self.say(false, None, "judge: no software instrument track to turn up"),
        }
        // Each of the run's changes still standing, undone through Kumi, last first (a round not kept took its own back).
        let mut ids: Vec<String> = Vec::new();
        for record in self.changes.borrow()[heard..].iter() {
            if !ids.contains(&record.id) {
                ids.push(record.id.clone());
            }
        }
        for id in ids.iter().rev() {
            let Some(row) = self.change(id) else { continue };
            if !matches!(row.state, ChangeState::Applied | ChangeState::Unsure) {
                continue;
            }
            let t0 = perf_now();
            let result = undo.execute(json!({"change": id}).as_object().cloned().unwrap_or_default(), signal()).await?;
            let undone = !result.is_error && self.change(id).is_some_and(|now| now.state == ChangeState::Undone);
            let what = if undone { "undone".to_string() } else { short(&result.text, 200) };
            self.say(undone, Some(perf_now() - t0), &format!("undo “{}”: {what}", row.title));
        }
        if self.stopping.get() {
            return Ok(());
        }

        // Tracks 1 and 2 auditioned: a pass each, the producer hearing the first and Logic silent for the second.
        let look = self.observe().await?;
        let context: Value = serde_json::from_str(&look.context).unwrap_or(Value::Null);
        let now = context["tracks"].as_array().cloned().unwrap_or_default();
        match (tool_named(&look, "audition"), now.first(), now.get(1)) {
            (Some(audition), Some(first), Some(second)) => {
                let t0 = perf_now();
                let candidates = json!({"candidates": [{"track": first["ref"], "label": "track 1"}, {"track": second["ref"], "label": "track 2"}], "from_beat": 0, "beats": 16});
                let result = audition.execute(candidates.as_object().cloned().unwrap_or_default(), listening()).await?;
                let reply: Value = serde_json::from_str(&result.text).unwrap_or(Value::Null);
                let takes: Vec<String> = reply["takes"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(|take| {
                        let heard = if take["silent"] == true {
                            "silent".to_string()
                        } else {
                            take["heard"].as_str().unwrap_or("unheard").to_owned()
                        };
                        format!("{}: {heard}", take["label"].as_str().unwrap_or("?"))
                    })
                    .collect();
                self.say(
                    !result.is_error && takes.len() == 2,
                    Some(perf_now() - t0),
                    &format!("audition of tracks 1 and 2: {}", if takes.is_empty() { short(&result.text, 200) } else { takes.join("; ") }),
                );
            }
            _ => self.say(false, None, "audition: no audition tool, or fewer than two tracks"),
        }
        if self.stopping.get() {
            return Ok(());
        }

        // Bars 1 to 5 of the instrument track rendered offline (soloed for the bounce): found on disk, its length the
        // part's, then deleted.
        match (tool_named(&look, "render"), now.iter().find(|track| track["kind"] == "instrument")) {
            (Some(render), Some(track)) => {
                let name = track["name"].as_str().unwrap_or("?").to_owned();
                let t0 = perf_now();
                let asked = json!({"track": track["ref"], "from_beat": 0, "beats": 16});
                let result = render.execute(asked.as_object().cloned().unwrap_or_default(), listening()).await?;
                let ms = perf_now() - t0;
                let reply: Value = serde_json::from_str(&result.text).unwrap_or(Value::Null);
                let file = reply["file"].as_str().map(str::to_owned);
                let seconds = file.as_deref().and_then(|file| wav_seconds(Path::new(file)));
                let what = match (&file, result.is_error) {
                    (Some(file), false) => format!(
                        "render of bars 1 to 5 of “{name}”: {} found, {} s long ({expected:.2} s at {bpm} BPM)",
                        Path::new(file).file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default(),
                        seconds.map(|seconds| format!("{seconds:.2}")).unwrap_or_else(|| "not".into())
                    ),
                    _ => format!("render of bars 1 to 5 of “{name}”: {}", short(&result.text, 200)),
                };
                self.say(!result.is_error && seconds.is_some_and(|seconds| (seconds - expected).abs() < 0.05), Some(ms), &what);
                if file.is_some_and(|file| std::fs::remove_file(file).is_ok()) {
                    println!("        (the render is deleted, checked)");
                }
            }
            _ => self.say(false, None, "render: no render tool, or no software instrument track"),
        }

        // The project as M5 found it.
        let look = self.observe().await?;
        let context: Value = serde_json::from_str(&look.context).unwrap_or(Value::Null);
        let after = mixer(&context["tracks"].as_array().cloned().unwrap_or_default());
        let output_plugins_now = match (stereo_out(&context), tool_named(&look, "logic_read")) {
            (Some(strip), Some(read)) => self.rows(&read, json!({"kind": "plugin", "parent": strip, "limit": 200})).await?.len(),
            _ => 0,
        };
        let same = after.len() == before.len()
            && after.iter().zip(&before).all(|(now, was)| now.0 == was.0 && (now.1 - was.1).abs() < 0.05 && now.2 == was.2);
        self.say(
            same && output_plugins_now == output_plugins,
            None,
            &format!(
                "the project reads as before M5: {} tracks, faders {}, {} soloed, {output_plugins_now} plug-ins on the Stereo Out (before: {output_plugins})",
                after.len(),
                after.iter().map(|(name, db, _)| format!("{name} {db:+.1} dB")).collect::<Vec<_>>().join(", "),
                after.iter().filter(|(_, _, solo)| *solo).count()
            ),
        );
        Ok(())
    }

    /// M3, with the operator: a rename made through Kumi (a step of Logic's undo, where a fader move is none), then
    /// Cmd-Z in Logic: the bridge names it back (`undone_in_daw`), and HISTORY marks the change undone with Logic's
    /// own undo. Without the Cmd-Z, Kumi undoes it, so nothing of the run is left in the project.
    async fn cmd_z_named(&self) -> Result<(), RuntimeError> {
        let observation = self.observe().await?;
        let context: Value = serde_json::from_str(&observation.context).unwrap_or(Value::Null);
        let Some(first) = context["tracks"].as_array().and_then(|tracks| tracks.first().cloned()) else {
            self.say(false, None, "Cmd-Z: the project has no track to change");
            return Ok(());
        };
        let (Some(plan), Some(undo)) = (tool_named(&observation, "make_changes"), tool_named(&observation, "undo_change")) else {
            self.say(false, None, "Cmd-Z: no make_changes and undo_change tools");
            return Ok(());
        };
        let name = first["name"].as_str().unwrap_or("Track 1").to_owned();
        let heard = self.changes.borrow().len();
        let steps = json!({"steps": [{"tool": "track_rename", "input": {"ref": first["ref"], "name": format!("{name} (Kumi)")}}]});
        let made = plan.execute(steps.as_object().cloned().unwrap_or_default(), signal()).await?;
        let Some(record) = self.changes.borrow()[heard..].first().cloned() else {
            self.say(false, None, &format!("Cmd-Z: the change wasn't made: {}", short(&made.text, 200)));
            return Ok(());
        };
        self.say(!made.is_error, None, &format!("a change to take back in Logic Pro: “{}”", record.title));
        println!("  → In Logic Pro, press Cmd-Z once (within {} s)…", OPERATOR_MS / 1000);
        let events_before = self.events.borrow().len();
        let t0 = perf_now();
        let named = self
            .wait_for(|| async move {
                self.events_since(events_before, "undone_in_daw")
                    .into_iter()
                    .find(|data| data["transactionIds"].as_array().is_some_and(|ids| !ids.is_empty()))
            })
            .await;
        match &named {
            Some(data) => {
                self.say(true, Some(perf_now() - t0), &format!("Cmd-Z named back: undone_in_daw {}", short(&data.to_string(), 160)))
            }
            None => self.say(false, Some(perf_now() - t0), "no undone_in_daw named the change (no Cmd-Z, or the bridge didn't see it)"),
        }
        // HISTORY follows a moment after the event.
        let mut now = self.change(&record.id);
        for _ in 0..20 {
            if now.as_ref().is_some_and(|record| record.state == ChangeState::Undone) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
            now = self.change(&record.id);
        }
        let undone = now.as_ref().is_some_and(|record| record.state == ChangeState::Undone);
        let note = now.as_ref().and_then(|record| record.note.clone()).map(|note| format!(" ({note})")).unwrap_or_default();
        self.say(
            undone,
            None,
            &format!("HISTORY: “{}” {}{note}", record.title, now.as_ref().map(|r| state_word(r.state)).unwrap_or("gone")),
        );
        if !undone {
            let result = undo.execute(json!({"change": record.id}).as_object().cloned().unwrap_or_default(), signal()).await?;
            let back = !result.is_error && self.change(&record.id).is_some_and(|now| now.state == ChangeState::Undone);
            let what = if back { "undone".to_string() } else { short(&result.text, 200) };
            self.say(back, None, &format!("undone through Kumi instead, so nothing of the run is left: {what}"));
        }
        Ok(())
    }

    /// M6: the fixed phrases through the integration's instant path (no model), one HISTORY row each, read back and undone
    /// through Kumi; metronome_set as a make_changes step, undone; a sample into a new Sampler track (only into the copy
    /// the save as made), undone through both of Logic's undo steps, and Quick Sampler refused; the recent projects, middle
    /// C, and the bulk read beside the paged reads; watch_me over a marker added through Kumi, undone; Channel EQ's and the
    /// Compressor's changes drawn as curves in HISTORY, undone; Logic's library folders; and the project read as before
    /// (its tracks, markers, tempo, metronome and cycle). Each part undoes what it made before the next begins.
    async fn m6_kinds(&self) -> Result<(), RuntimeError> {
        let observation = self.observe().await?;
        let Some(undo) = tool_named(&observation, "undo_change") else {
            self.say(false, None, "M6: no undo_change tool");
            return Ok(());
        };
        let before = self.m6_state().await;
        self.m6_phrases(&undo).await?;
        if self.stopping.get() {
            return Ok(());
        }
        self.m6_metronome(&undo).await?;
        if self.stopping.get() {
            return Ok(());
        }
        self.m6_sample(&undo).await?;
        if self.stopping.get() {
            return Ok(());
        }
        let (recent, open) = self.m6_recent().await;
        self.m6_middle_c().await?;
        if self.stopping.get() {
            return Ok(());
        }
        self.m6_bulk().await;
        if self.stopping.get() {
            return Ok(());
        }
        self.m6_watch(&undo).await?;
        if self.stopping.get() {
            return Ok(());
        }
        self.m6_curves(&undo).await?;
        self.m6_library(&recent, open.as_deref());
        let after = self.m6_state().await;
        let same = alike(&before, &after);
        self.say(
            same,
            None,
            &format!(
                "the project reads as before M6: {}{}",
                state_words(&after),
                if same { String::new() } else { format!(" (before: {})", state_words(&before)) }
            ),
        );
        Ok(())
    }

    /// (M6) What M6 leaves as it found: the tracks (name, kind, mute), the markers (name, position, length), the tempo, the
    /// metronome's click and the cycle.
    async fn m6_state(&self) -> Value {
        let fields_of = |page: Answer, fields: &[&str]| -> Vec<Value> {
            page.body["items"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|row| Value::Object(fields.iter().map(|field| (field.to_string(), row[*field].clone())).collect()))
                .collect()
        };
        let tracks = fields_of(self.tool("read", json!({"kind": "track", "limit": 200})).await, &["name", "kind", "mute"]);
        let markers = fields_of(self.tool("read", json!({"kind": "marker", "limit": 200})).await, &["name", "position", "length"]);
        let tempo = self.reading(&Reading::Tempo).await;
        let click = self.reading(&Reading::Click).await;
        let cycle = self.reading(&Reading::Cycle).await;
        json!({"tracks": tracks, "markers": markers, "tempo": tempo, "click": click, "cycle": cycle})
    }

    /// (M6) The project row as the bridge reads it now.
    async fn project_row(&self) -> Value {
        self.tool("read", json!({"kind": "project"})).await.body["items"][0].clone()
    }

    /// (M6) The beats in a bar of the project's time signature ("4/4" has 4).
    async fn beats_per_bar(&self) -> f64 {
        self.project_row().await["timeSignature"]
            .as_str()
            .and_then(|time| time.split('/').next()?.trim().parse::<f64>().ok())
            .filter(|beats| *beats > 0.0)
            .unwrap_or(4.0)
    }

    /// (M6) The start of the bar after the bar past every region (as M4's marker and MIDI import are placed).
    async fn past_regions(&self, beats_per_bar: f64) -> f64 {
        let regions = self.tool("read", json!({"kind": "region", "limit": 200})).await;
        let end =
            regions.body["items"].as_array().into_iter().flatten().filter_map(|region| region["endBeats"].as_f64()).fold(0.0, f64::max);
        ((end / beats_per_bar).floor() + 2.0) * beats_per_bar
    }

    /// (M6) What a change of `kind` would change from, read through the bridge: a preview (never applied) names the state
    /// it fences, as M4's read of the Tempo List does.
    async fn prior_of(&self, kind: &str, input: Value) -> Result<Value, String> {
        let epoch = self.tool("read", json!({"kind": "project"})).await.body["epoch"].clone();
        let preview = self.tool("change_preview", json!({"kind": kind, "input": input, "epoch": epoch})).await;
        if preview.ok && preview.body["prior"].is_object() {
            Ok(preview.body["prior"].clone())
        } else {
            Err(short(&preview.error(), 160))
        }
    }

    /// (M6) What a phrase or a step changes, as the bridge reads it now (null when it can't be read).
    async fn reading(&self, what: &Reading) -> Value {
        match what {
            Reading::Tempo => self.project_row().await["tempo"].clone(),
            Reading::Click => {
                self.prior_of("metronome-set", json!({"on": true})).await.map(|prior| prior["on"].clone()).unwrap_or(Value::Null)
            }
            Reading::Mute(name) => {
                let page = self.tool("read", json!({"kind": "track", "limit": 200})).await;
                page.body["items"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .find(|row| row["name"] == name.as_str())
                    .map(|row| row["mute"].clone())
                    .unwrap_or(Value::Null)
            }
            Reading::Cycle => {
                let focus = self.tool("focus", json!({})).await.body["transport"]["cycle"].clone();
                match self.prior_of("loop-range", json!({"startBeats": 0, "endBeats": 0})).await {
                    Ok(prior) => {
                        json!({"on": prior["on"], "startBeats": prior["startBeats"], "endBeats": prior["endBeats"], "focus": focus})
                    }
                    Err(_) => json!({"focus": focus}),
                }
            }
        }
    }

    /// (M6) A reading once it is `wanted`, or as it stands after two seconds (the focus is a cache the bridge refreshes
    /// every 150 ms).
    async fn settled(&self, what: &Reading, wanted: &Value) -> Value {
        let mut now = self.reading(what).await;
        for _ in 0..10 {
            if alike(&now, wanted) || self.stopping.get() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
            now = self.reading(what).await;
        }
        now
    }

    /// (M6) The changes HISTORY heard of since `from`, each once, in the order they came.
    fn made_since(&self, from: usize) -> Vec<String> {
        let heard = self.changes.borrow();
        let mut ids: Vec<String> = Vec::new();
        for record in heard[from.min(heard.len())..].iter() {
            if !ids.contains(&record.id) {
                ids.push(record.id.clone());
            }
        }
        ids
    }

    /// (M6) Changes' titles as HISTORY says them.
    fn titles(&self, ids: &[String]) -> String {
        let titles: Vec<String> = ids.iter().filter_map(|id| self.change(id)).map(|row| format!("“{}”", row.title)).collect();
        if titles.is_empty() {
            "none".into()
        } else {
            titles.join(", ")
        }
    }

    /// (M6) A fixed phrase as the session runs one: a fresh look, then the integration's instant path, with no model. It
    /// counts when it makes one HISTORY row and what it changes reads `wanted`. The rows it made, for Kumi's undo.
    async fn phrase_row(&self, said: &str, what: &Reading, wanted: &Value) -> Result<Vec<String>, RuntimeError> {
        let Some(intent) = phrases::parse(said) else {
            self.say(false, None, &format!("“{said}”: not one of Kumi's fixed phrases"));
            return Ok(vec![]);
        };
        self.observe().await?;
        let heard = self.changes.borrow().len();
        let t0 = perf_now();
        let outcome = self.integration.instant(&intent, signal()).await?;
        let ms = perf_now() - t0;
        let ids = self.made_since(heard);
        let Some(outcome) = outcome else {
            self.say(false, Some(ms), &format!("“{said}”: the instant path didn't take it, so it would go to the model"));
            return Ok(ids);
        };
        let now = self.settled(what, wanted).await;
        self.say(
            !outcome.is_error && ids.len() == 1 && alike(&now, wanted),
            Some(ms),
            &format!(
                "“{said}” through the instant path, no model: {} HISTORY row(s) ({}); reads {}; Kumi said: {}",
                ids.len(),
                self.titles(&ids),
                reading_words(what, &now),
                short(&outcome.text, 160)
            ),
        );
        Ok(ids)
    }

    /// (M6) Changes undone through Kumi, the last made first, each read back as it was before it (`made`: the rows of a
    /// phrase or a step, with the reading from before it).
    async fn undo_rows(&self, undo: &Rc<dyn KernelTool>, made: &[(Vec<String>, Value)], what: &Reading) -> Result<(), RuntimeError> {
        for (ids, before) in made.iter().rev() {
            for id in ids.iter().rev() {
                let title = self.change(id).map(|row| row.title).unwrap_or_else(|| id.clone());
                let t0 = perf_now();
                let result = undo.execute(json!({"change": id}).as_object().cloned().unwrap_or_default(), signal()).await?;
                let ms = perf_now() - t0;
                let undone = !result.is_error && self.change(id).is_some_and(|now| now.state == ChangeState::Undone);
                let now = self.settled(what, before).await;
                self.say(
                    undone && alike(&now, before),
                    Some(ms),
                    &format!(
                        "undo “{title}” through Kumi: {}; reads {}",
                        if undone { "undone".to_string() } else { short(&result.text, 200) },
                        reading_words(what, &now)
                    ),
                );
            }
        }
        Ok(())
    }

    /// (M6) The fixed phrases, each through the integration's instant path with no model, one HISTORY row each, read back
    /// and undone through Kumi: the tempo 4 BPM up (in whole BPM), the metronome turned and turned back, a track muted by
    /// its name (unmuted when it is muted), the cycle on bars 1 to 4 and off.
    async fn m6_phrases(&self, undo: &Rc<dyn KernelTool>) -> Result<(), RuntimeError> {
        if !self.integration.has_instant() {
            self.say(false, None, "the fixed phrases: Logic Pro's integration has no instant path, so they would go to the model");
            return Ok(());
        }
        let tempo = self.reading(&Reading::Tempo).await;
        match tempo.as_f64() {
            None => self.say(false, None, "“tempo”: the project row reads no tempo"),
            Some(bpm) => {
                let target = if bpm.round() + 4.0 <= 990.0 { bpm.round() + 4.0 } else { bpm.round() - 4.0 };
                let made = self.phrase_row(&format!("tempo {target}"), &Reading::Tempo, &json!(target)).await?;
                self.undo_rows(undo, &[(made, tempo)], &Reading::Tempo).await?;
            }
        }
        if self.stopping.get() {
            return Ok(());
        }
        // The metronome turned, then turned back as it began: a row each, undone last first.
        let click = self.reading(&Reading::Click).await;
        match click.as_bool() {
            None => self.say(false, None, "“metronome”: the click can't be read (a metronome-set previewed named none)"),
            Some(on) => {
                let (first, second) = if on { ("metronome off", "metronome on") } else { ("metronome on", "metronome off") };
                let mut made = vec![(self.phrase_row(first, &Reading::Click, &json!(!on)).await?, json!(on))];
                made.push((self.phrase_row(second, &Reading::Click, &json!(on)).await?, json!(!on)));
                self.undo_rows(undo, &made, &Reading::Click).await?;
            }
        }
        if self.stopping.get() {
            return Ok(());
        }
        // A track by its name: the first of the producer's own whose name a phrase can say, and no other track has.
        let page = self.tool("read", json!({"kind": "track", "limit": 200})).await;
        let rows = page.body["items"].as_array().cloned().unwrap_or_default();
        let names: Vec<String> = rows.iter().filter_map(|row| row["name"].as_str()).map(str::to_lowercase).collect();
        let pick = rows.iter().find(|row| {
            let name = row["name"].as_str().unwrap_or_default();
            matches!(row["kind"].as_str(), Some("audio" | "instrument" | "external"))
                && names.iter().filter(|other| **other == name.to_lowercase()).count() == 1
                && matches!(phrases::parse(&format!("mute {name}")), Some(Intent::Track { .. }))
        });
        match pick {
            None => self.say(false, None, "“mute”: no track of the producer's whose name a phrase can say"),
            Some(row) => {
                let name = row["name"].as_str().unwrap_or_default().to_owned();
                let muted = row["mute"] == true;
                let what = Reading::Mute(name.clone());
                let said = format!("{} {name}", if muted { "unmute" } else { "mute" });
                let made = self.phrase_row(&said, &what, &json!(!muted)).await?;
                self.undo_rows(undo, &[(made, json!(muted))], &what).await?;
            }
        }
        if self.stopping.get() {
            return Ok(());
        }
        // The cycle on bars 1 to 4, then off: a row each, undone last first, the cycle as it began.
        let cycle = self.reading(&Reading::Cycle).await;
        if cycle.get("on").is_none() {
            self.say(false, None, "“loop”: the cycle can't be read (a loop-range previewed named none)");
            return Ok(());
        }
        // (bars 2 to 5 when the cycle stands on bars 1 to 4 already, which a phrase for them wouldn't change)
        let bar = self.beats_per_bar().await;
        let there = cycle["on"] == true && alike(&cycle["startBeats"], &json!(0.0)) && alike(&cycle["endBeats"], &json!(4.0 * bar));
        let (said, start) = if there { ("loop 2-5", bar) } else { ("loop 1-4", 0.0) };
        let on = json!({"on": true, "startBeats": start, "endBeats": start + 4.0 * bar, "focus": true});
        let off = json!({"on": false, "startBeats": start, "endBeats": start + 4.0 * bar, "focus": false});
        let mut made = vec![(self.phrase_row(said, &Reading::Cycle, &on).await?, cycle)];
        made.push((self.phrase_row("loop off", &Reading::Cycle, &off).await?, on));
        self.undo_rows(undo, &made, &Reading::Cycle).await
    }

    /// (M6) metronome_set as a make_changes step: the click turned, read back, and undone through Kumi.
    async fn m6_metronome(&self, undo: &Rc<dyn KernelTool>) -> Result<(), RuntimeError> {
        let click = self.reading(&Reading::Click).await;
        let Some(on) = click.as_bool() else {
            self.say(false, None, "metronome_set: the click can't be read (a metronome-set previewed named none)");
            return Ok(());
        };
        let look = self.observe().await?;
        let heard = self.changes.borrow().len();
        let t0 = perf_now();
        let (is_error, text) = self.run_tool(&look, "metronome_set", json!({"on": !on})).await?;
        let ms = perf_now() - t0;
        let made = self.made_since(heard);
        let now = self.settled(&Reading::Click, &json!(!on)).await;
        self.say(
            !is_error && made.len() == 1 && alike(&now, &json!(!on)),
            Some(ms),
            &format!(
                "metronome_set {{on: {}}} as a make_changes step: {} HISTORY row(s) ({}); reads {}{}",
                !on,
                made.len(),
                self.titles(&made),
                reading_words(&Reading::Click, &now),
                if is_error { format!(": {}", short(&text, 200)) } else { String::new() }
            ),
        );
        self.undo_rows(undo, &[(made, click)], &Reading::Click).await
    }

    /// (M6) A sample into a new Sampler track: a 1 s sine the run writes under the --files folder, loaded through
    /// sample_load onto the first audio track, a bar past every region (Logic imports it, then converts its region), read
    /// back (a new track named after the file, its instrument the Sampler) and undone through Kumi (Logic's own undo of the
    /// conversion and of the import under it: the tracks and regions as before); Quick Sampler refused first. Only into the
    /// copy the save as made: Logic copies the file into the open project's package, and its undo leaves the copy there.
    async fn m6_sample(&self, undo: &Rc<dyn KernelTool>) -> Result<(), RuntimeError> {
        let Some(folder) = &self.files else {
            self.say(
                true,
                None,
                "sample_load: skipped (no --files folder was given, and the run loads a sample only into the copy it saves there)",
            );
            return Ok(());
        };
        let folder = whole(folder);
        let open = self.project_row().await["path"].as_str().unwrap_or_default().trim_end_matches('/').to_owned();
        if open.is_empty() || !whole(Path::new(&open)).starts_with(&folder) {
            self.say(
                false,
                None,
                &format!(
                    "sample_load: not run, since the project open ({}) isn't the copy the save as made under {} (Logic Pro copies a sample into the open project's package)",
                    if open.is_empty() { "unsaved".to_string() } else { tilde(&open) },
                    tilde(&folder.to_string_lossy())
                ),
            );
            return Ok(());
        }
        let wav = folder.join(SINE);
        if let Err(error) = write_sine(&wav) {
            self.say(false, None, &format!("sample_load: the sine couldn't be written to {}: {error}", tilde(&wav.to_string_lossy())));
            return Ok(());
        }
        let loaded = self.sample_kinds(undo, &wav).await;
        if std::fs::remove_file(&wav).is_ok() {
            println!("        (the sine is deleted; the copy's package keeps Logic Pro's copies of it)");
        }
        loaded
    }

    /// (M6) Quick Sampler refused and the Sampler loaded and undone, with the sine at `wav`.
    async fn sample_kinds(&self, undo: &Rc<dyn KernelTool>, wav: &Path) -> Result<(), RuntimeError> {
        let file = wav.to_string_lossy().into_owned();
        let stem = SINE.trim_end_matches(".wav");
        let look = self.observe().await?;
        // Quick Sampler: refused in Logic's words, nothing made.
        let heard = self.changes.borrow().len();
        let t0 = perf_now();
        let (is_error, text) = self.run_tool(&look, "sample_load", json!({"kind": "quick-sampler", "file": file})).await?;
        let why = serde_json::from_str::<Value>(&text)
            .ok()
            .and_then(|answer| answer["refused"][0]["error"].as_str().map(str::to_owned))
            .unwrap_or_else(|| text.clone());
        self.say(
            is_error && self.made_since(heard).is_empty() && why.contains("Quick Sampler"),
            Some(perf_now() - t0),
            &format!("sample_load kind quick-sampler refused, nothing made: {}", short(&why, 320)),
        );
        // The Sampler, on the first audio track, a bar past every region.
        let context: Value = serde_json::from_str(&look.context).unwrap_or(Value::Null);
        let Some(audio) = context["tracks"].as_array().into_iter().flatten().find(|track| track["kind"] == "audio").cloned() else {
            self.say(false, None, "sample_load: the project has no audio track for the sample to come in on");
            return Ok(());
        };
        let audio_name = audio["name"].as_str().unwrap_or("?").to_owned();
        let at = self.past_regions(self.beats_per_bar().await).await;
        let named = |page: &Answer| -> Vec<String> {
            page.body["items"].as_array().into_iter().flatten().filter_map(|row| row["name"].as_str().map(str::to_owned)).collect()
        };
        let names = named(&self.tool("read", json!({"kind": "track", "limit": 200})).await);
        let (tracks, regions) = (self.count("track").await, self.count("region").await);
        let heard = self.changes.borrow().len();
        let t0 = perf_now();
        let (is_error, text) =
            self.run_tool(&look, "sample_load", json!({"kind": "sampler", "file": file, "ref": audio["ref"], "positionBeats": at})).await?;
        let ms = perf_now() - t0;
        let made = self.made_since(heard);
        // The track the load made: the one named after the file, else the one whose name wasn't there before.
        let page = self.tool("read", json!({"kind": "track", "limit": 200})).await;
        let tracks_now = page.body["total"].as_u64().unwrap_or(0) as usize;
        let rows = page.body["items"].as_array().cloned().unwrap_or_default();
        let track = rows
            .iter()
            .find(|row| row["name"] == stem)
            .or_else(|| rows.iter().find(|row| row["name"].as_str().is_some_and(|name| !names.iter().any(|was| was == name))))
            .cloned();
        let instrument = match track.as_ref().and_then(|row| row["ref"].as_str()) {
            Some(parent) => {
                self.tool("read", json!({"kind": "plugin", "parent": parent})).await.body["items"][0]["name"].as_str().map(str::to_owned)
            }
            None => None,
        };
        let regions_now = self.count("region").await;
        let what = if is_error {
            format!("sample_load of {SINE} on “{audio_name}”: {}", short(&text, 300))
        } else {
            format!(
                "sample_load of a 1 s sine ({SINE}: 44.1 kHz, 16-bit, {} s) on “{audio_name}” at beat {at}: {}, its instrument {}; {tracks} → {tracks_now} tracks, {regions} → {regions_now} regions; {} HISTORY row(s) ({})",
                wav_seconds(wav).map_or("unread".to_string(), |seconds| format!("{seconds:.2}")),
                match &track {
                    Some(row) => format!("a new {} track “{}”", row["kind"].as_str().unwrap_or("?"), row["name"].as_str().unwrap_or("?")),
                    None => format!("no new track (none named “{stem}”)"),
                },
                instrument.as_deref().unwrap_or("unread"),
                made.len(),
                self.titles(&made)
            )
        };
        self.say(
            !is_error
                && made.len() == 1
                && tracks_now == tracks + 1
                && track.as_ref().is_some_and(|row| row["kind"] == "instrument" && row["name"] == stem)
                && instrument.as_deref() == Some("Sampler"),
            Some(ms),
            &what,
        );
        // Undone through Kumi: Logic's own undo of the conversion, then of the import under it (the audio region with it).
        for id in made.iter().rev() {
            let title = self.change(id).map(|row| row.title).unwrap_or_else(|| id.clone());
            let t0 = perf_now();
            let result = undo.execute(json!({"change": id}).as_object().cloned().unwrap_or_default(), signal()).await?;
            let ms = perf_now() - t0;
            let undone = !result.is_error && self.change(id).is_some_and(|now| now.state == ChangeState::Undone);
            let (tracks_back, regions_back) = (self.count("track").await, self.count("region").await);
            let note = self.change(id).and_then(|row| row.note).map(|note| format!(" ({note})")).unwrap_or_default();
            self.say(
                undone && tracks_back == tracks && regions_back == regions,
                Some(ms),
                &format!(
                    "undo “{title}” through Kumi, both of Logic Pro's undo steps (the conversion, then the import): {}; {tracks_back} tracks, {regions_back} regions (before the load: {tracks}, {regions}){note}",
                    if undone { "undone".to_string() } else { short(&result.text, 200) }
                ),
            );
        }
        Ok(())
    }

    /// (M6) The recent projects as the bridge reads them (File > Open Recent): rows, the open project among them with its
    /// path (the copy the save as made, when the run made one). The paths the rows name, and the open project's.
    async fn m6_recent(&self) -> (Vec<String>, Option<String>) {
        let open =
            self.project_row().await["path"].as_str().map(|path| path.trim_end_matches('/').to_owned()).filter(|path| !path.is_empty());
        let recent = self.tool("read", json!({"kind": "recent", "limit": 50})).await;
        let rows = recent.body["items"].as_array().cloned().unwrap_or_default();
        let paths: Vec<String> =
            rows.iter().filter_map(|row| row["path"].as_str()).map(|path| path.trim_end_matches('/').to_owned()).collect();
        let open_row = rows.iter().find(|row| row["open"] == true);
        let named = open_row.and_then(|row| row["path"].as_str()).map(|path| path.trim_end_matches('/').to_owned());
        let the_copy = match (&self.files, &named) {
            (None, _) => true,
            (Some(folder), Some(path)) => whole(Path::new(path)).starts_with(whole(folder)),
            (Some(_), None) => false,
        };
        let what = if recent.ok {
            format!(
                "read recent: {} projects Logic Pro opened lately, {} with a path; the open one {}",
                rows.len(),
                paths.len(),
                match open_row {
                    Some(row) => format!(
                        "“{}” (item {}) at {}{}",
                        row["name"].as_str().unwrap_or("?"),
                        row["index"],
                        named.as_deref().map_or("no path".to_string(), tilde),
                        match (self.files.is_some(), the_copy) {
                            (true, true) => ", the copy the save as made",
                            (true, false) => ", which isn't a copy under the --files folder",
                            _ => "",
                        }
                    ),
                    None => format!("isn't among them ({})", open.as_deref().map_or("unsaved".to_string(), tilde)),
                }
            )
        } else {
            format!("read recent: {}", recent.error())
        };
        self.say(recent.ok && named.is_some() && named == open && the_copy, Some(recent.ms), &what);
        (paths, open)
    }

    /// (M6) Middle C: the project row's middleC (3 or 4: Logic's Display Middle C As, which the run reads and never
    /// changes), the integration's octave from a look, and MIDI 60 named in it as the session's profile names notes.
    async fn m6_middle_c(&self) -> Result<(), RuntimeError> {
        let middle_c = self.project_row().await["middleC"].as_i64();
        self.observe().await?;
        let octave = self.integration.middle_c_octave();
        let named = self.integration.profile().with_middle_c(octave).note_name(60);
        self.say(
            matches!(middle_c, Some(3 | 4)) && octave.map(i64::from) == middle_c && middle_c.is_some_and(|c| named == format!("C{c}")),
            None,
            &format!(
                "middle C: the project row says middleC {} (Logic Pro's Display Middle C As, read and never changed); the integration takes octave {}, so the session names MIDI 60 {named}",
                middle_c.map_or("none".to_string(), |c| c.to_string()),
                octave.map_or("none".to_string(), |o| o.to_string())
            ),
        );
        Ok(())
    }

    /// (M6) Every row of a kind, read a page (200 rows) at a time through the host, as catch-up's paged reads take them:
    /// the rows and their epoch, or why not.
    async fn paged(&self, kind: &str) -> Result<(Vec<Value>, String), String> {
        let (mut rows, mut cursor, mut epoch) = (Vec::new(), None::<String>, String::new());
        for _ in 0..50 {
            let mut args = json!({"kind": kind, "limit": 200});
            if let Some(cursor) = &cursor {
                args["cursor"] = json!(cursor);
            }
            let page = self.tool("read", args).await;
            if !page.ok {
                return Err(page.error());
            }
            epoch = page.body["epoch"].as_str().unwrap_or_default().to_owned();
            rows.extend(page.body["items"].as_array().cloned().unwrap_or_default());
            cursor = page.body["nextCursor"].as_str().filter(|next| !next.is_empty()).map(str::to_owned);
            if cursor.is_none() {
                break;
            }
        }
        Ok((rows, epoch))
    }

    /// (M6) The bulk read beside the paged reads: the six kinds catch-up reads, a page at a time, then in one
    /// `snapshot {rows}` (each after a pause past the bridge's caches), both timed; the tracks and the markers alike.
    async fn m6_bulk(&self) {
        let kinds = logic_project::READS;
        tokio::time::sleep(Duration::from_millis(COLD_MS)).await;
        let t0 = perf_now();
        let mut paged = Vec::new();
        for kind in kinds {
            let t1 = perf_now();
            let read = self.paged(kind).await;
            paged.push((*kind, read, perf_now() - t1));
        }
        let paged_ms = perf_now() - t0;
        tokio::time::sleep(Duration::from_millis(COLD_MS)).await;
        let bulk = self.tool("snapshot", json!({"rows": kinds})).await;
        let epoch = bulk.body["epoch"].as_str().unwrap_or_default().to_owned();
        let same = |kind: &str| {
            paged.iter().find(|(read, ..)| *read == kind).and_then(|(_, read, _)| read.as_ref().ok()).is_some_and(|(rows, paged_epoch)| {
                without_epoch(&Value::Array(rows.clone()), paged_epoch) == without_epoch(&bulk.body["rows"][kind], &epoch)
            })
        };
        let (tracks, markers) = (same("track"), same("marker"));
        let others: Vec<&str> = kinds.iter().copied().filter(|kind| !matches!(*kind, "track" | "marker") && !same(kind)).collect();
        let count = |kind: &str| bulk.body["rows"][kind].as_array().map_or(0, Vec::len);
        let truncated = bulk.body.get("truncated").filter(|kinds| !kinds.is_null()).map(Value::to_string);
        let times: Vec<String> = paged
            .iter()
            .map(|(kind, read, ms)| match read {
                Ok((rows, _)) => format!("{kind} {} ({} rows)", seconds(*ms), rows.len()),
                Err(why) => format!("{kind} failed ({})", short(why, 80)),
            })
            .collect();
        let what = if bulk.ok {
            format!(
                "the bulk read, snapshot {{rows: [{}]}}: {} rows in one answer in {} (epoch {epoch}), the six paged reads {} ({}); the tracks ({}) {} and the markers ({}) {} the paged reads'{}{}",
                kinds.join(", "),
                kinds.iter().map(|kind| count(kind)).sum::<usize>(),
                seconds(bulk.ms),
                seconds(paged_ms),
                times.join(", "),
                count("track"),
                if tracks { "are" } else { "aren't" },
                count("marker"),
                if markers { "are" } else { "aren't" },
                if others.is_empty() { ", as are the other kinds'".to_string() } else { format!("; the {} rows differ", others.join(", ")) },
                truncated.as_deref().map(|kinds| format!("; cut: {kinds}")).unwrap_or_default()
            )
        } else {
            format!("the bulk read, snapshot {{rows}}: {}", bulk.error())
        };
        self.say(bulk.ok && truncated.is_none() && tracks && markers, Some(bulk.ms), &what);
    }

    /// (M6) watch_me: started, a marker added through Kumi (a bar past every region), stopped: what it tells names the
    /// marker added there in Logic's words (a marker, never Live's locator); then the marker undone through Kumi.
    async fn m6_watch(&self, undo: &Rc<dyn KernelTool>) -> Result<(), RuntimeError> {
        let look = self.observe().await?;
        let Some(watch) = tool_named(&look, "watch_me") else {
            self.say(false, None, "watch_me: Logic Pro's integration offers no watch_me tool");
            return Ok(());
        };
        let beats_per_bar = self.beats_per_bar().await;
        let at = self.past_regions(beats_per_bar).await;
        let position = format!("bar {}", (at / beats_per_bar).round() as i64 + 1);
        let markers = self.count("marker").await;
        let action = |action: &str| json!({"action": action}).as_object().cloned().unwrap_or_default();
        let t0 = perf_now();
        let started = watch.execute(action("start"), signal()).await?;
        let heard = self.changes.borrow().len();
        let (is_error, text) = self.run_tool(&look, "marker_add", json!({"positionBeats": at})).await?;
        let made = self.made_since(heard);
        let stopped = watch.execute(action("stop"), signal()).await?;
        let ms = perf_now() - t0;
        let told =
            serde_json::from_str::<Value>(&stopped.text).ok().and_then(|reply| reply["changes"].as_array().cloned()).unwrap_or_default();
        let named = told.iter().any(|change| change["added"] == "marker" && change["position"] == position.as_str());
        let said = stopped.text.to_lowercase();
        let lives: Vec<&str> = ["locator", "scene", "clip", "the set"].into_iter().filter(|word| said.contains(word)).collect();
        let what = if started.is_error || is_error || stopped.is_error {
            let failed = if started.is_error {
                &started.text
            } else if is_error {
                &text
            } else {
                &stopped.text
            };
            format!("watch_me around a marker added at {position}: {}", short(failed, 240))
        } else {
            format!(
                "watch_me: started, a marker added through Kumi at {position}, stopped: {}{}",
                short(&stopped.text, 400),
                if lives.is_empty() { String::new() } else { format!(" (Live's words: {})", lives.join(", ")) }
            )
        };
        self.say(
            !started.is_error
                && !is_error
                && made.len() == 1
                && !stopped.is_error
                && named
                && lives.is_empty()
                && watch.description().contains("Logic Pro"),
            Some(ms),
            &what,
        );
        for id in made.iter().rev() {
            let title = self.change(id).map(|row| row.title).unwrap_or_else(|| id.clone());
            let t0 = perf_now();
            let result = undo.execute(json!({"change": id}).as_object().cloned().unwrap_or_default(), signal()).await?;
            let ms = perf_now() - t0;
            let undone = !result.is_error && self.change(id).is_some_and(|now| now.state == ChangeState::Undone);
            let markers_back = self.count("marker").await;
            self.say(
                undone && markers_back == markers,
                Some(ms),
                &format!(
                    "undo “{title}” through Kumi: {}; {markers_back} markers (before: {markers})",
                    if undone { "undone".to_string() } else { short(&result.text, 200) }
                ),
            );
        }
        Ok(())
    }

    /// (M6) Curves in HISTORY: a band's gain of the first Channel EQ (Scratch's Vocal holds one) and the threshold of a
    /// Compressor (on the EQ's strip, else the first strip that holds one; skipped when none does), each changed through
    /// fx_param, its record drawn as the plug-in's curve once the follow-up read lands, and undone.
    async fn m6_curves(&self, undo: &Rc<dyn KernelTool>) -> Result<(), RuntimeError> {
        let look = self.observe().await?;
        let Some(read) = tool_named(&look, "logic_read") else {
            self.say(false, None, "curves: no logic_read tool");
            return Ok(());
        };
        let context: Value = serde_json::from_str(&look.context).unwrap_or(Value::Null);
        let mut strips: Vec<(String, Vec<Value>)> = Vec::new();
        for track in context["tracks"].as_array().cloned().unwrap_or_default() {
            let plugins = self.rows(&read, json!({"kind": "plugin", "parent": track["ref"], "limit": 200})).await?;
            strips.push((track["name"].as_str().unwrap_or("?").to_owned(), plugins));
        }
        let find = |plugin: &str, first: Option<&str>| -> Option<(String, Value)> {
            let on =
                |strip: &(String, Vec<Value>)| strip.1.iter().find(|row| row["name"] == plugin).map(|row| (strip.0.clone(), row.clone()));
            first.and_then(|first| strips.iter().filter(|strip| strip.0 == first).find_map(on)).or_else(|| strips.iter().find_map(on))
        };
        let eq = find("Channel EQ", None);
        match &eq {
            None => self.say(false, None, "curves: no track's strip holds a Channel EQ"),
            Some((strip, plugin)) => self.curve_row(&look, &read, undo, strip, plugin, "Low Shelf Gain", true).await?,
        }
        if self.stopping.get() {
            return Ok(());
        }
        match find("Compressor", eq.as_ref().map(|(strip, _)| strip.as_str())) {
            None => self.say(true, None, "curves: the Compressor's threshold skipped, since no track's strip holds a Compressor"),
            Some((strip, plugin)) => self.curve_row(&look, &read, undo, &strip, &plugin, "Threshold", false).await?,
        }
        Ok(())
    }

    /// (M6) A parameter of a plug-in's curve changed through Kumi's fx_param (an EQ band's gain 3 dB, a compressor's
    /// threshold 6 dB), its HISTORY record then drawn as the plug-in's curve once the follow-up read of the plug-in lands
    /// (an EQ's bands, or a compressor's curve, before and after; waited for up to 20 s), and undone through Kumi.
    #[allow(clippy::too_many_arguments)]
    async fn curve_row(
        &self,
        look: &Observation,
        read: &Rc<dyn KernelTool>,
        undo: &Rc<dyn KernelTool>,
        strip: &str,
        plugin: &Value,
        param: &str,
        eq: bool,
    ) -> Result<(), RuntimeError> {
        let name = plugin["name"].as_str().unwrap_or("?").to_owned();
        let shown_in =
            |rows: &[Value]| rows.iter().find(|row| row["name"] == param).and_then(|row| row["text"].as_str().map(str::to_owned));
        let params = self.rows(read, json!({"kind": "param", "parent": plugin["ref"], "limit": 200})).await?;
        let Some((was, from)) = shown_in(&params).and_then(|was| number_of(&was).map(|from| (was, from))) else {
            let names: Vec<&str> = params.iter().filter_map(|row| row["name"].as_str()).take(30).collect();
            self.say(
                false,
                None,
                &format!(
                    "{name}'s {param} on “{strip}”: its Controls View shows no {param} with a number ({})",
                    if names.is_empty() { "nothing read".to_string() } else { names.join(", ") }
                ),
            );
            return Ok(());
        };
        let by = if eq {
            if from <= 20.0 {
                3.0
            } else {
                -3.0
            }
        } else if from - 6.0 >= -48.0 {
            -6.0
        } else {
            6.0
        };
        let target = from.round() + by;
        let heard = self.changes.borrow().len();
        let t0 = perf_now();
        let (is_error, text) = self.run_tool(look, "fx_param", json!({"ref": plugin["ref"], "param": param, "value": target})).await?;
        let ms = perf_now() - t0;
        let made = self.made_since(heard);
        // The record's picture: the values it moved at once, the curve once the follow-up read of the plug-in lands.
        let curve = |picture: Option<&ChangePicture>| {
            if eq {
                matches!(picture, Some(ChangePicture::Eq { .. }))
            } else {
                matches!(picture, Some(ChangePicture::Dynamics { .. }))
            }
        };
        let t1 = perf_now();
        let mut picture = None;
        if let Some(id) = made.last().filter(|_| !is_error) {
            loop {
                picture = self.change(id).and_then(|row| row.picture);
                if curve(picture.as_ref()) || perf_now() - t1 > CURVE_MS || self.stopping.get() {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(200)).await;
            }
        }
        let drawn_in = perf_now() - t1;
        let reads = shown_in(&self.rows(read, json!({"kind": "param", "parent": plugin["ref"], "limit": 200})).await?);
        let set = reads.as_deref().and_then(number_of).is_some_and(|now| (now - target).abs() < 0.5);
        let what = if is_error {
            format!("{name}'s {param} on “{strip}” to {target} through fx_param: {}", short(&text, 200))
        } else {
            format!(
                "{name}'s {param} on “{strip}” {was} → {} through fx_param, {} HISTORY row(s): its record draws {} ({} after the change)",
                reads.as_deref().unwrap_or("unread"),
                made.len(),
                picture_words(picture.as_ref()),
                seconds(drawn_in)
            )
        };
        self.say(!is_error && made.len() == 1 && set && curve(picture.as_ref()), Some(ms), &what);
        for id in made.iter().rev() {
            let title = self.change(id).map(|row| row.title).unwrap_or_else(|| id.clone());
            let t0 = perf_now();
            let result = undo.execute(json!({"change": id}).as_object().cloned().unwrap_or_default(), signal()).await?;
            let ms = perf_now() - t0;
            let undone = !result.is_error && self.change(id).is_some_and(|now| now.state == ChangeState::Undone);
            let back = shown_in(&self.rows(read, json!({"kind": "param", "parent": plugin["ref"], "limit": 200})).await?);
            self.say(
                undone && back.as_deref().and_then(number_of).is_some_and(|now| (now - from).abs() < 0.05),
                Some(ms),
                &format!(
                    "undo “{title}” through Kumi: {}; {param} reads {} (before: {was})",
                    if undone { "undone".to_string() } else { short(&result.text, 200) },
                    back.as_deref().unwrap_or("unread")
                ),
            );
        }
        Ok(())
    }

    /// (M6) Logic's library: integrations::library_sources for Logic gives only folders there on this Mac and none of
    /// Live's (Splice's folder is both DAWs'); and the library's recent Logic projects are those the bridge named that are
    /// still there, the open one among them.
    fn m6_library(&self, recent: &[String], open: Option<&str>) {
        let logic = integrations::library_sources(&SourceOptions { daw: Some(DawId::Logic), ..SourceOptions::default() });
        let live: Vec<String> = integrations::library_sources(&SourceOptions::default())
            .into_iter()
            .filter(|source| source.kind != SourceKind::Splice)
            .map(|source| source.path)
            .collect();
        let missing: Vec<String> =
            logic.iter().filter(|source| !Path::new(&source.path).is_dir()).map(|source| tilde(&source.path)).collect();
        let lives: Vec<String> = logic
            .iter()
            .filter(|source| live.contains(&source.path) || source.path.to_lowercase().contains("ableton"))
            .map(|source| tilde(&source.path))
            .collect();
        let listed: Vec<String> = logic.iter().map(|source| format!("{} ({})", source.label, tilde(&source.path))).collect();
        self.say(
            !logic.is_empty() && missing.is_empty() && lives.is_empty(),
            None,
            &format!(
                "Logic Pro's library sources: {} folder(s){}{}: {}",
                logic.len(),
                if missing.is_empty() {
                    ", each there on this Mac".to_string()
                } else {
                    format!(", {} not there ({})", missing.len(), missing.join(", "))
                },
                if lives.is_empty() { ", none of Live's".to_string() } else { format!(", Live's among them ({})", lives.join(", ")) },
                if listed.is_empty() { "none".to_string() } else { listed.join("; ") }
            ),
        );
        let kept = integrations::recent_projects(&SourceOptions {
            daw: Some(DawId::Logic),
            recent: Some(recent.to_vec()),
            ..SourceOptions::default()
        });
        let among = open.is_some_and(|open| kept.iter().any(|path| path == open));
        self.say(
            among,
            None,
            &format!(
                "the library's recent Logic Pro projects: {} of the {} the bridge named with a path (those still there), the open one {}",
                kept.len(),
                recent.len(),
                if among { "among them" } else { "not among them" }
            ),
        );
    }

    fn exit_code(&self) -> i32 {
        let rows = self.rows.borrow();
        let passed = rows.iter().filter(|row| row.ok).count();
        let stopping = self.stopping.get();
        println!("\n{passed} of {} passed{}.", rows.len(), if stopping { " (stopped early)" } else { "" });
        if passed == rows.len() && !stopping {
            0
        } else {
            1
        }
    }

    /// The run as a Markdown record for docs/evidence/.
    fn summary(&self) -> String {
        let status = self.status.borrow();
        let rows = self.rows.borrow();
        let passed = rows.iter().filter(|row| row.ok).count();
        let today = chrono::Utc::now().format("%Y-%m-%d");
        let mut out = format!("# Logic Pro M2 to M6 acceptance, {today}\n\nStatus: {} of {} checks passed.\n\n", passed, rows.len());
        out.push_str("| | |\n| --- | --- |\n");
        out.push_str(&format!(
            "| Logic Pro | {} (pid {}, `{}`) |\n| Bridge | {} |\n| Project | {} |\n| Host | `daw-mcp-server --daw logic` from this build |\n\n",
            status["dawVersion"].as_str().unwrap_or("?"),
            status["pid"],
            status["resource"].as_str().unwrap_or("?"),
            status["bridge"].as_str().unwrap_or("?"),
            status["project"]["path"].as_str().unwrap_or("none")
        ));
        out.push_str("| Check | Result | Time |\n| --- | --- | --- |\n");
        for row in rows.iter() {
            let what = row.what.replace('|', "\\|");
            out.push_str(&format!("| {what} | {} | {} |\n", if row.ok { "ok" } else { "failed" }, row.ms.map(seconds).unwrap_or_default()));
        }
        out
    }
}
