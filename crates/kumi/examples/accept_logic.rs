//! Opt-in acceptance run on the real Logic Pro: Kumi's host started beside this binary, as Kumi starts it, the bridge
//! it puts beside Logic, and the M1 and M2 checks of Logic's plan against the project Logic has open. M1: the status
//! and discovery. M2: the focus's latency (median and worst over 50 calls, under 150 ms), the first look and every kind
//! read through logic_read, the four bodies, and, with an operator at Logic, the selection following, a dialog
//! counting as busy, a project switched and refs held across a track moved. It reads only: nothing in the project
//! changes. It prints a Markdown summary for docs/evidence/ at the end (and writes it to --report, when given). The
//! bridge must be set up in the home (`kumi --daw logic bridge`) and Accessibility allowed; build the host first:
//!   cargo build --release -p daw-mcp-server --bins
//!   cargo run --release -p kumi --example accept_logic -- [--kumi-home <dir>] [--report <file>] [--no-operator]
use futures::FutureExt;
use kumi_agent::{
    core::{
        context::IntegrationContext,
        contracts::{Integration, Observation},
        view::BodyRequest,
    },
    mcp::{
        client::{connect_mcp, McpEndpoint, Options},
        types::{CallToolResult, ContentBlock},
    },
};
use kumi_common::{
    abort::{self, Signal},
    time::perf_now,
};
use kumi_daw_logic::create_logic_integration;
use kumi_daw_own::bridge::{Connect, OwnOptions};
use kumi_types::{contracts::JsonObject, errors::RuntimeError, system::Env};
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

fn main() {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let after = |flag: &str| argv.iter().position(|arg| arg == flag).and_then(|at| argv.get(at + 1)).cloned();
    if argv.iter().any(|arg| arg == "--help" || arg == "-h") {
        eprintln!("cargo run --release -p kumi --example accept_logic -- [--kumi-home <dir>] [--report <file>] [--no-operator]");
        std::process::exit(2);
    }
    let env: Env = std::env::vars().collect();
    let kumi_home = after("--kumi-home").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(kumi::config::kumi_dir(&env)));
    let report = after("--report").map(PathBuf::from);
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
    let code = local.block_on(&runtime, accept_logic(kumi_home, host, operator, report));
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
}

fn signal() -> Signal {
    abort::timeout(60_000)
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

async fn accept_logic(kumi_home: PathBuf, host: PathBuf, operator: bool, report: Option<PathBuf>) -> i32 {
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
        bridge: Some(kumi_daw_logic::DAW.descriptor),
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
    let context = IntegrationContext::new(Rc::new(|_, _| {}));
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
        let heard = self.events.borrow().len();
        let t0 = perf_now();
        let seen = self
            .wait_for(|| async move {
                let focus = self.tool("focus", json!({})).await;
                let now = focus.body["selection"]["trackRef"].as_str().map(str::to_owned);
                (focus.ok && now.is_some() && now.as_deref() != before).then_some(now.unwrap_or_default())
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
        let mut out = format!("# Logic Pro M2 acceptance, {today}\n\nStatus: {} of {} checks passed.\n\n", passed, rows.len());
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
