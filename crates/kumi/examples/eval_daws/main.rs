//! Opt-in evals of how the configured model works in a real REAPER or Renoise (M7's power-user scenarios). Each scenario
//! builds its fixture in the lab DAW's open project or song, gives Kumi the producer's words, and scores the state Kumi
//! leaves, read back through Kumi's bridge: the scenario's own checks ("Expect"), then the four every scenario has:
//!
//! - no preference changed (REAPER's setup and render settings; Renoise's editing settings), or put back;
//! - no extension called that the setup didn't show (SWS, ReaPack, ReaImGui, js_ReaScriptAPI in REAPER);
//! - the DAW's own words in what Kumi says and in HISTORY, never Live's;
//! - nothing generated unasked (no notes added where the scenario asks for none).
//!
//! It uses your sign-in and model (Kumi's own folder) and Kumi's host in process on the lab's kumi-home. The fixtures
//! rewrite the project or song they find, so it holds the lab's lock for the whole run (`<locks>/<daw>.lock`, made with
//! mkdir; a wrapper that holds it names itself `holder <pid>` in its owner file), runs only on a lab: the kumi-home's
//! parent must hold the `.kumi-eval-lab` marker the lab's setup writes (never Kumi's own folder or the home folder), and
//! refuses any project or song but the lab's scratch one: unsaved and untitled, or saved under that lab folder.
//!
//! cargo run -p kumi --example eval_daws -- --daw reaper|renoise --kumi-home <lab kumi-home> --locks <dir>
//!     [--only P30,R31] [--out <results.md>]
//!
//! EVAL_TRACE=1 prints each tool call and what it returned.
use async_trait::async_trait;
use daw_bridge::Daw;
use daw_mcp_server::host::Host;
use futures::FutureExt;
use kumi::config::{load_inference_config, load_tools_dir, safe_error};
use kumi_common::{
    abort::Signal,
    js::{json::stringify, string::head},
};
use kumi_runtime::{
    ai::{
        error::LanguageModelError,
        types::{CallOptions, StreamPart, StreamParts},
    },
    core::{context::IntegrationContext, contracts::WatchEvent, daw::DawProfile, memory::MemoryStoreOptions, session::VideoDirectories},
    create_agent_kernel, create_memory_store, create_session,
    integrations::{
        own_bridge::{Connect, OwnOptions},
        reaper::create_reaper_integration,
        renoise::create_renoise_integration,
    },
    kernel::agent::LanguageModel,
    mcp::{
        client::McpEndpoint,
        types::{CallToolResult, ContentBlock},
    },
    open_credential_store, resolve_model,
    system::process_env,
    AgentKernel, AgentKernelOptions, ChangeRecord, IntegrationFactory, Kernel, KernelEvent, KernelFactory, KernelOptions, ModelBinding,
    ResolveModelOptions, RuntimeError, Session, SessionController, SessionEvent, SessionOptions,
};
use regex::Regex;
use serde_json::{json, Value};
use std::{
    cell::{Cell, RefCell},
    io::Write,
    path::{Path, PathBuf},
    rc::{Rc, Weak},
    sync::Arc,
    time::Instant,
};

#[path = "../../../kumi-runtime/tests/support/in_process.rs"]
mod in_process;
use in_process::InProcess;

mod guard;
mod media;
mod scenarios;
use scenarios::{Check, Scenario, Seen};

// ---------------------------------------------------------------------------------------------------------------------
// Outcomes

struct Outcome {
    id: &'static str,
    checks: Vec<Check>,
    said: String,
    tools: Vec<String>,
    seconds: f64,
    error: Option<String>,
}

impl Outcome {
    fn passed(&self) -> bool {
        self.error.is_none() && self.checks.iter().all(|check| check.passed)
    }
}

/// Live's words, whole (as the app's own leak tests have them).
static LIVE: std::sync::LazyLock<Regex> = std::sync::LazyLock::new(|| {
    Regex::new(r"\b(Live|Ableton|Sets?|Session|Arrangement|Main|[Ss]cenes?|[Rr]acks?|[Cc]lip slots?|Control Surfaces?|Remote Scripts?|Max for Live|M4L|Operator)\b").unwrap()
});

/// "Set" as the verb, any producer's word: starting a sentence, a line, a list's item or what follows a colon ("Set Bass to
/// -6 dB", "Done: Set tempo"), or before the words a verb takes ("Set up", "Set the"). Live's Set is a noun mid-sentence.
static SET_VERB: std::sync::LazyLock<Regex> =
    std::sync::LazyLock::new(|| Regex::new(r"(?m)(^\s*(?:[-*•]\s+)?|[.!?:;]\s+)Set\b|\bSet (up|the|it|to|a|an|your|my)\b").unwrap());

/// Live's words in what Kumi said, but "Set" as the verb.
fn live_words(text: &str) -> Vec<String> {
    LIVE.find_iter(&SET_VERB.replace_all(text, "${1}")).map(|m| m.as_str().to_owned()).collect()
}

/// What every scenario also checks.
fn every_scenario(scenario: &Scenario, run: &Run) -> Vec<Check> {
    let mut checks = vec![];
    // No preference changed (or each put back).
    let (before, after) = (&run.before["prefs"], &run.after["prefs"]);
    let changed: Vec<String> = before
        .as_object()
        .into_iter()
        .flatten()
        .filter(|(key, value)| after.get(key.as_str()) != Some(value))
        .map(|(key, value)| format!("{key}: {value} → {}", after.get(key.as_str()).unwrap_or(&Value::Null)))
        .collect();
    checks.push(Check::new("no preference changed", changed.is_empty(), changed.join("; ")));
    // No extension called that the setup didn't show.
    let extensions = Regex::new(r"\b(CF|BR|SNM|NF|JS|ImGui|ReaPack|ULT|SWS)_\w+|_SWS_\w+|_BR_\w+").unwrap();
    let present = |name: &str| run.setup.pointer(&format!("/extensions/{name}")) == Some(&Value::Bool(true));
    let unchecked: Vec<String> = run
        .calls
        .iter()
        .filter_map(|call| extensions.find(call).map(|found| (call, found.as_str().to_owned())))
        .filter(|(call, found)| {
            let which = match found.trim_start_matches('_') {
                name if name.starts_with("ImGui") => "ReaImGui",
                name if name.starts_with("ReaPack") => "ReaPack",
                name if name.starts_with("JS") => "js_ReaScriptAPI",
                _ => "SWS",
            };
            !present(which) && !call.contains("APIExists")
        })
        .map(|(_, found)| found)
        .collect();
    checks.push(Check::new("no extension called unchecked", unchecked.is_empty(), unchecked.join(", ")));
    // The DAW's own words in what Kumi said and in HISTORY.
    let heard: Vec<String> =
        std::iter::once(run.said.as_str()).chain(run.changes.iter().map(|change| change.title.as_str())).flat_map(live_words).collect();
    checks.push(Check::new("the DAW's own words, never Live's", heard.is_empty(), heard.join(", ")));
    let named = scenario.names.iter().any(|name| run.said.to_lowercase().contains(&name.to_lowercase()));
    checks.push(Check::new("says what it found or changed, by name", named, head(&run.said.replace('\n', " "), 160)));
    // An outcome stays honest: a turn that ended in an error isn't a pass.
    if let Some(error) = &run.error {
        checks.push(Check::new("the turn ended without an error", false, error.clone()));
    }
    // Nothing generated unasked.
    let notes = |state: &Value| state["notes"].as_u64().unwrap_or(0);
    if !scenario.generates {
        checks.push(Check::new(
            "no notes added unasked",
            notes(&run.after) <= notes(&run.before),
            format!("{} → {}", notes(&run.before), notes(&run.after)),
        ));
    }
    checks
}

/// What one scenario's session left to score.
struct Run {
    before: Value,
    after: Value,
    setup: Value,
    said: String,
    calls: Vec<String>,
    changes: Vec<ChangeRecord>,
    error: Option<String>,
}

// ---------------------------------------------------------------------------------------------------------------------
// The host, the DAW's Lua, the model

fn text_of(result: &CallToolResult) -> String {
    result
        .content
        .iter()
        .filter_map(|block| match block {
            ContentBlock::Text { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect()
}

async fn host_call(host: &InProcess, name: &str, args: Value) -> Result<Value, String> {
    let result = host.call(name, args.as_object().cloned().unwrap_or_default(), Signal::new()).await.map_err(|e| format!("{e:?}"))?;
    let text = text_of(&result);
    let value: Value = serde_json::from_str(&text).unwrap_or(Value::String(text.clone()));
    if result.is_error == Some(true) {
        return Err(value.get("error").and_then(Value::as_str).map(str::to_owned).unwrap_or(text));
    }
    Ok(value)
}

/// Lua run in the DAW through the host as a run-lua change that no session keeps (no HISTORY row), and what it returned.
async fn lua(host: &InProcess, code: &str, timeout_ms: u64) -> Result<Value, String> {
    thread_local!(static NEXT: Cell<u64> = const { Cell::new(0) });
    // (a fixture that replaced every track reads as another project a moment later: the epoch moves, so read it again)
    let mut tries = 0;
    let preview = loop {
        let project = host_call(host, "snapshot", json!({})).await?;
        let input = json!({"kind": "run-lua", "input": {"code": code, "timeoutMs": timeout_ms}, "epoch": project["epoch"]});
        match host_call(host, "change_preview", input).await {
            Err(why) if why.contains("earlier epoch") && tries < 5 => {
                tries += 1;
                tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            }
            answer => break answer?,
        }
    };
    let (Some(id), Some(confirmation)) = (preview["transactionId"].as_str(), preview["confirmation"].as_str()) else {
        return Err(format!("no transaction: {preview}"));
    };
    let key = NEXT.with(|next| {
        next.set(next.get() + 1);
        format!("eval-{}-{}-{}", std::process::id(), kumi_common::time::now_ms(), next.get())
    });
    let applied =
        host_call(host, "change_apply", json!({"transactionId": id, "confirmation": confirmation, "idempotencyKey": key})).await?;
    host_call(host, "change_release", json!({"transactionIds": [id]})).await.ok();
    if let Some(failed) = applied.pointer("/applied/failed") {
        return Err(format!("Lua: {}", failed["message"].as_str().unwrap_or_default()));
    }
    Ok(applied.pointer("/applied/result").cloned().unwrap_or(Value::Null))
}

/// The configured model, shared by each scenario's kernels.
struct Shared {
    model: Rc<dyn LanguageModel>,
    calls: Cell<usize>,
    tokens: Rc<Cell<(f64, f64, f64)>>,
}

#[async_trait(?Send)]
impl LanguageModel for Shared {
    async fn do_stream(&self, options: CallOptions) -> Result<StreamParts, LanguageModelError> {
        self.calls.set(self.calls.get() + 1);
        let parts = self.model.do_stream(options).await?;
        let tokens = self.tokens.clone();
        Ok(Box::pin(futures::StreamExt::inspect(parts, move |part| {
            if let StreamPart::Finish { usage, .. } = part {
                let (input, cached, output) = tokens.get();
                tokens.set((
                    input + usage.input_tokens.total.unwrap_or(0.),
                    cached + usage.input_tokens.cache_read.unwrap_or(0.),
                    output + usage.output_tokens.total.unwrap_or(0.),
                ));
            }
        })))
    }
}

struct Model {
    binding: Rc<ModelBinding>,
    shared: Rc<Shared>,
}

impl Model {
    fn binding(&self) -> ModelBinding {
        let prepare = self.binding.clone();
        let budget = self.binding.clone();
        ModelBinding {
            id: self.binding.id.clone(),
            model: self.shared.clone(),
            prepare: Box::new(move |request| (prepare.prepare)(request)),
            budget: self.binding.budget.as_ref().map(|_| Box::new(move |fixed| (budget.budget.as_ref().unwrap())(fixed)) as Box<_>),
        }
    }
}

// ---------------------------------------------------------------------------------------------------------------------
// One scenario

/// The lab's mkdir lock for one DAW: taken for the run, or already held for it by the wrapper that started this process
/// (its owner file names `holder <our parent's pid>`); refused, naming the owner, when anyone else holds it.
struct LabLock {
    dir: PathBuf,
    ours: bool,
}

impl LabLock {
    fn take(locks: &Path, daw: Daw) -> Result<LabLock, String> {
        let dir = locks.join(format!("{}.lock", daw.name().to_lowercase()));
        if std::fs::create_dir(&dir).is_ok() {
            std::fs::write(dir.join("owner"), format!("eval_daws holder {}\n", std::process::id())).map_err(|e| e.to_string())?;
            return Ok(LabLock { dir, ours: true });
        }
        let owner = std::fs::read_to_string(dir.join("owner")).unwrap_or_default();
        let parent = parent_pid().map(|pid| pid.to_string());
        if owner.split_whitespace().collect::<Vec<_>>().windows(2).any(|w| w[0] == "holder" && Some(w[1].to_owned()) == parent) {
            return Ok(LabLock { dir, ours: false });
        }
        Err(format!("the {} lab is locked ({}): {}", daw.name(), dir.display(), owner.trim()))
    }
}

/// The pid of the process that started this one (a wrapper holding the lock), where the system says.
#[cfg(unix)]
fn parent_pid() -> Option<u32> {
    Some(std::os::unix::process::parent_id())
}
#[cfg(not(unix))]
fn parent_pid() -> Option<u32> {
    None
}

impl Drop for LabLock {
    fn drop(&mut self) {
        if self.ours {
            let _ = std::fs::remove_file(self.dir.join("owner"));
            let _ = std::fs::remove_dir(&self.dir);
        }
    }
}

/// Where a run's files go: the sounds the fixtures load, the songs they save, Kumi's tools (yt-dlp, whisper) and videos.
struct Dirs {
    /// The lab folder (marked, see `guard`).
    lab: PathBuf,
    media: PathBuf,
    songs: PathBuf,
    tools: String,
    videos: PathBuf,
}

async fn run_scenario(model: &Model, scenario: &Scenario, home: &Path, dirs: &Dirs) -> Result<Outcome, RuntimeError> {
    let started = Instant::now();
    let failure = |e: String| RuntimeError::plain(e);
    let host = Rc::new(InProcess { host: Arc::new(Host::new(scenario.daw, home)), id: Cell::new(0) });
    // (a DAW just started takes a while to load its bridge)
    let mut status = Value::Null;
    for _ in 0..60 {
        status = host_call(&host, "status", json!({})).await.map_err(failure)?;
        if status.get("connected") == Some(&Value::Bool(true)) {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    }
    if status.get("connected") != Some(&Value::Bool(true)) {
        return Err(RuntimeError::plain(format!("the lab DAW isn't connected: {status}")));
    }
    // Only the lab's scratch project or song: the fixtures rewrite what's open.
    let project = host_call(&host, "snapshot", json!({})).await.map_err(failure)?["project"].clone();
    guard::scratch(&project, &dirs.lab).map_err(failure)?;
    // (Renoise stops a callback at 800 ms; REAPER's chunks may take longer on a big template)
    let (prelude, state, timeout) = match scenario.daw {
        Daw::Reaper => (scenarios::REAPER_PRELUDE, scenarios::REAPER_STATE, 5_000),
        Daw::Renoise => (scenarios::RENOISE_PRELUDE, scenarios::RENOISE_STATE, 800),
    };
    let places = format!("local MEDIA = {:?}\nlocal SONGS = {:?}\n", dirs.media.to_string_lossy(), dirs.songs.to_string_lossy());
    let reader = format!("{state}\n{}\nreturn out", scenario.read);
    // The first chunk starts from a plain project or song; `before` is what Kumi saw when it last looked (the host keeps
    // its snapshot), the fixture what happened since.
    if let Some(before) = scenario.before {
        lua(&host, &format!("{places}{prelude}\n{before}"), timeout).await.map_err(|e| failure(format!("before: {e}")))?;
        host_call(&host, "catch_up", json!({})).await.map_err(|e| failure(format!("catch_up: {e}")))?;
        lua(&host, &format!("{places}{}", scenario.fixture), timeout).await.map_err(|e| failure(format!("fixture: {e}")))?;
    } else {
        lua(&host, &format!("{places}{prelude}\n{}", scenario.fixture), timeout).await.map_err(|e| failure(format!("fixture: {e}")))?;
    }
    // A fixture that replaced every track reads as another project a moment later (#393): wait for the epoch to settle, so
    // the session's refs aren't retired under it.
    let mut last = Value::Null;
    for _ in 0..20 {
        tokio::time::sleep(std::time::Duration::from_millis(600)).await;
        let epoch = host_call(&host, "snapshot", json!({})).await.map_err(failure)?["epoch"].clone();
        if epoch == last {
            break;
        }
        last = epoch;
    }
    let before = lua(&host, &reader, timeout).await.map_err(|e| failure(format!("state before: {e}")))?;
    // EVAL_DRY=1: the fixture and its state only, no session (no model calls): to check the fixtures in the lab.
    if std::env::var_os("EVAL_DRY").is_some_and(|dry| !dry.is_empty()) {
        if let Some(teardown) = scenario.teardown {
            lua(&host, teardown, timeout).await.ok();
        }
        let mut summary = format!("{} bytes of state: {}", stringify(&before).len(), head(&stringify(&before), 300));
        // (with a `before`: what catch-up tells of the fixture, as the session's first look would)
        if scenario.before.is_some() {
            let told = host_call(&host, "catch_up", json!({})).await.unwrap_or_else(|why| json!({"refused": why}));
            summary.push_str(&format!(" | catch-up: {}", head(&stringify(&told), 600)));
        }
        return Ok(Outcome {
            id: scenario.id,
            checks: vec![Check::new("fixture built and read", true, summary)],
            said: String::new(),
            tools: vec![],
            seconds: started.elapsed().as_secs_f64(),
            error: None,
        });
    }
    let setup = host_call(&host, "snapshot", json!({})).await.map_err(failure)?["setup"].clone();

    let changes = Rc::new(RefCell::new(Vec::<ChangeRecord>::new()));
    let said = Rc::new(RefCell::new(String::new()));
    let tools = Rc::new(RefCell::new(Vec::<String>::new()));
    let kernel = Rc::new(RefCell::new(None::<Rc<AgentKernel>>));
    let controller = Rc::new(RefCell::new(None::<Weak<Session>>));
    let folder = tempfile::Builder::new().prefix("kumi-eval-daws-").tempdir().map_err(|e| failure(e.to_string()))?;
    let kernel_factory: KernelFactory = {
        let kernel = kernel.clone();
        let model = Model { binding: model.binding.clone(), shared: model.shared.clone() };
        Rc::new(move |options: KernelOptions| {
            let made = create_agent_kernel(AgentKernelOptions {
                conversation: None,
                instructions: options.instructions,
                tools: options.tools,
                signal: options.signal,
                checkpoint: options.checkpoint,
                binding: model.binding(),
                max_steps: None,
                budget: None,
            })
            .map(Rc::new);
            if let Ok(made) = &made {
                *kernel.borrow_mut() = Some(made.clone());
            }
            async move { made.map(|made| made as Rc<dyn Kernel>) }.boxed_local()
        })
    };
    let integration_factory: IntegrationFactory = {
        let (host, changes, controller, home, daw) = (host.clone(), changes.clone(), controller.clone(), home.to_path_buf(), scenario.daw);
        Box::new(move |on_connection| {
            let endpoint = host.clone();
            let connect: Connect = Rc::new(move |_| {
                let endpoint = endpoint.clone();
                async move { Ok(endpoint as Rc<dyn McpEndpoint>) }.boxed_local()
            });
            let mut context = IntegrationContext::new(on_connection);
            let (changes, controller) = (changes.clone(), controller.clone());
            context.on_change = Some(Rc::new(move |change: ChangeRecord| {
                {
                    let mut changes = changes.borrow_mut();
                    match changes.iter_mut().find(|kept| kept.id == change.id) {
                        Some(kept) => *kept = change.clone(),
                        None => changes.push(change.clone()),
                    }
                }
                if let Some(session) = controller.borrow().as_ref().and_then(Weak::upgrade) {
                    session.watch(WatchEvent::Change(change));
                }
            }));
            let options = OwnOptions { context, kumi_home: home.clone(), connect: Some(connect) };
            match daw {
                Daw::Reaper => create_reaper_integration(options),
                Daw::Renoise => create_renoise_integration(options),
            }
        })
    };
    let on_event: Rc<dyn Fn(SessionEvent)> = {
        let (said, tools) = (said.clone(), tools.clone());
        Rc::new(move |event: SessionEvent| match event {
            SessionEvent::Kernel(KernelEvent::ToolStart { name, .. }) => tools.borrow_mut().push(name),
            SessionEvent::Kernel(KernelEvent::Text { text }) => said.borrow_mut().push_str(&text),
            _ => {}
        })
    };
    let mut options = SessionOptions::new(kernel_factory, integration_factory, on_event);
    options.profile = match scenario.daw {
        Daw::Reaper => DawProfile::REAPER,
        Daw::Renoise => DawProfile::RENOISE,
    };
    // (a tutorial is fetched and transcribed inside the turn)
    options.timeout_ms = Some(900_000);
    options.listen = true;
    options.watch = Some(VideoDirectories { videos_dir: dirs.videos.to_string_lossy().into_owned(), tools_dir: dirs.tools.clone() });
    options.memory = Some(create_memory_store(MemoryStoreOptions {
        projects_dir: folder.path().join("projects"),
        producer_file: folder.path().join("memory.json"),
    }));
    let session = Rc::new(create_session(options)?);
    *controller.borrow_mut() = Some(Rc::downgrade(&session));
    let turns = async {
        session.start().await?;
        for words in scenario.says {
            session.submit(words, None).await?;
        }
        Ok::<_, RuntimeError>(())
    }
    .await;
    // A turn that failed still leaves its state to score, and the failure is a failed check.
    let error = turns.err().map(|error| safe_error(Some(&error), &[]));
    let held = kernel.borrow().clone();
    let messages =
        held.and_then(|kernel| Kernel::checkpoint(kernel.as_ref()).ok()).map(|checkpoint| checkpoint.messages).unwrap_or_default();
    session.close().await.ok();
    // Each tool call's input, as text (a plan's steps inside it), for the extension check.
    let mut calls = vec![];
    for part in messages.iter().filter_map(|message| message.get("content").and_then(Value::as_array)).flatten() {
        let either = |first: &str, second: &str| part.get(first).filter(|value| !value.is_null()).or(part.get(second)).cloned();
        match part.get("type").and_then(Value::as_str) {
            Some("tool-call") => {
                let input = stringify(&either("input", "args").unwrap_or(Value::Null));
                if std::env::var_os("EVAL_TRACE").is_some_and(|trace| !trace.is_empty()) {
                    println!("   → {} {}", part["toolName"].as_str().unwrap_or_default(), head(&input, 700));
                }
                calls.push(input);
            }
            Some("tool-result") if std::env::var_os("EVAL_TRACE").is_some_and(|trace| !trace.is_empty()) => {
                println!("   ← {}", head(&stringify(&either("output", "result").unwrap_or(Value::Null)), 500));
            }
            _ => {}
        }
    }
    let after = lua(&host, &reader, timeout).await.map_err(|e| failure(format!("state after: {e}")))?;
    if let Some(teardown) = scenario.teardown {
        lua(&host, teardown, timeout).await.ok();
    }
    let tools_used = tools.borrow().clone();
    let run = Run { before, after, setup, said: said.borrow().clone(), calls, changes: changes.borrow().clone(), error };
    let mut checks = (scenario.expect)(&Seen {
        before: &run.before,
        after: &run.after,
        said: &run.said,
        tools: &tools_used,
        changes: run.changes.len(),
    });
    checks.extend(every_scenario(scenario, &run));
    folder.close().ok();
    Ok(Outcome { id: scenario.id, checks, said: run.said, tools: tools_used, seconds: started.elapsed().as_secs_f64(), error: None })
}

// ---------------------------------------------------------------------------------------------------------------------
// The run

fn report(outcome: &Outcome) -> String {
    let mut text = format!("## {} — {} ({:.0} s)\n\n", outcome.id, if outcome.passed() { "passed" } else { "failed" }, outcome.seconds);
    if let Some(error) = &outcome.error {
        text.push_str(&format!("- error: {error}\n"));
    }
    for check in &outcome.checks {
        text.push_str(&format!(
            "- {} {}{}\n",
            if check.passed { "✓" } else { "✗" },
            check.name,
            if check.detail.is_empty() { String::new() } else { format!(": {}", check.detail) }
        ));
    }
    text.push_str(&format!("- tools: {}\n- said: {}\n\n", outcome.tools.join(", "), head(&outcome.said.replace('\n', " "), 600)));
    text
}

async fn run() -> Result<i32, RuntimeError> {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let flag = |name: &str| argv.iter().position(|a| a == name).and_then(|k| argv.get(k + 1)).cloned();
    let usage = "eval_daws --daw reaper|renoise --kumi-home <lab kumi-home> --locks <dir> [--only P30,R31] [--out <results.md>]";
    let daw = match flag("--daw").as_deref() {
        Some("reaper") => Daw::Reaper,
        Some("renoise") => Daw::Renoise,
        _ => return Err(RuntimeError::plain(usage)),
    };
    let home = PathBuf::from(flag("--kumi-home").ok_or_else(|| RuntimeError::plain(usage))?);
    // Only a marked lab, never Kumi's own folder or the home folder; then the lab's lock, for the whole run (released as
    // `_lock` goes, before the process ends).
    let user_home = std::env::var_os("HOME").map(PathBuf::from).or_else(::home::home_dir).unwrap_or_default();
    let lab = guard::lab_of(&home, &user_home).map_err(RuntimeError::plain)?;
    let locks = PathBuf::from(flag("--locks").ok_or_else(|| RuntimeError::plain(usage))?);
    let _lock = LabLock::take(&locks, daw).map_err(RuntimeError::plain)?;
    let only: Vec<String> = flag("--only").map(|list| list.split(',').map(str::to_owned).collect()).unwrap_or_default();
    let env = process_env();
    let config = load_inference_config(&env)?;
    let binding = resolve_model(ResolveModelOptions {
        model: config.model.clone().unwrap_or_default(),
        store: Rc::new(open_credential_store(&config.auth_file)),
        env: Some(env.clone()),
        fetch: None,
        effort: None,
        service_tier: None,
    })
    .await?;
    let shared = Rc::new(Shared { model: binding.model.clone(), calls: Cell::new(0), tokens: Rc::new(Cell::new((0., 0., 0.))) });
    let model = Model { binding: Rc::new(binding), shared };
    let mut outcomes = vec![];
    let mut text = format!("# eval_daws: {} with {}\n\n", daw.name(), config.model.as_deref().unwrap_or("undefined"));
    // The fixtures' sounds and songs beside the lab's kumi-home; Kumi's own tools (yt-dlp, whisper) where Kumi keeps them.
    let files = lab.join("m7g");
    let videos = tempfile::Builder::new().prefix("kumi-eval-videos-").tempdir().map_err(|e| RuntimeError::plain(e.to_string()))?;
    let dirs = Dirs {
        media: files.join("media"),
        songs: files.join("songs"),
        tools: load_tools_dir(&env)?,
        videos: videos.path().to_path_buf(),
        lab,
    };
    media::write_all(&dirs.media).map_err(|e| RuntimeError::plain(format!("the fixtures' sounds: {e}")))?;
    std::fs::create_dir_all(&dirs.songs).map_err(|e| RuntimeError::plain(e.to_string()))?;
    for scenario in scenarios::all().iter().filter(|s| s.daw == daw && (only.is_empty() || only.iter().any(|o| o == s.id))) {
        println!("== {} {}", scenario.id, scenario.says.join(" / "));
        let outcome = match run_scenario(&model, scenario, &home, &dirs).await {
            Ok(outcome) => outcome,
            Err(error) => Outcome {
                id: scenario.id,
                checks: vec![],
                said: String::new(),
                tools: vec![],
                seconds: 0.0,
                error: Some(safe_error(Some(&error), &[])),
            },
        };
        let part = report(&outcome);
        print!("{part}");
        text.push_str(&part);
        outcomes.push(outcome);
    }
    let passed = outcomes.iter().filter(|outcome| outcome.passed()).count();
    let (input, cached, output) = model.shared.tokens.get();
    let summary = format!(
        "{passed} of {} passed; {} model calls; tokens {input} in ({cached} cached), {output} out.\n",
        outcomes.len(),
        model.shared.calls.get()
    );
    print!("{summary}");
    text.push_str(&summary);
    if let Some(out) = flag("--out") {
        std::fs::write(&out, text).map_err(|e| RuntimeError::plain(e.to_string()))?;
    }
    Ok(if passed == outcomes.len() { 0 } else { 1 })
}

fn main() {
    let runtime = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
        Ok(runtime) => runtime,
        Err(error) => {
            eprintln!("eval_daws: {error}");
            std::process::exit(1);
        }
    };
    let code = tokio::task::LocalSet::new().block_on(&runtime, async {
        match run().await {
            Ok(code) => code,
            Err(error) => {
                eprintln!("eval_daws: {}", safe_error(Some(&error), &[]));
                1
            }
        }
    });
    let _ = std::io::stdout().flush();
    std::process::exit(code);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// #401 (PR #400 review D20): "Set" starting a sentence is the verb, not Live's Set, so the check passes it; Live's Set
    /// mid-sentence is still caught, so the check can fail.
    #[test]
    fn set_starting_a_sentence_is_the_verb_and_lives_set_is_caught() {
        let none: Vec<String> = vec![];
        assert_eq!(live_words("Set the reverb to 30%. Set Bass to -6 dB.\n- Set Kick's send to -12 dB"), none);
        assert_eq!(live_words("Done: Set tempo to 124. I'll set it up; Set up the bus"), none);
        assert_eq!(live_words("I saved your Set. Your Sets folder has 3"), ["Set", "Sets"]);
        assert_eq!(live_words("Added a Rack to the Session view"), ["Rack", "Session"]);
    }

    /// The names a scenario expects in Kumi's answer aren't the producer's own words, so saying them proves something and
    /// the check can fail.
    #[test]
    fn no_scenarios_names_are_in_what_the_producer_says() {
        for scenario in scenarios::all() {
            assert!(!scenario.names.is_empty(), "{}", scenario.id);
            let says = scenario.says.join(" ").to_lowercase();
            for name in scenario.names {
                assert!(!says.contains(&name.to_lowercase()), "{}: {name} is in {says:?}", scenario.id);
            }
        }
    }
}
