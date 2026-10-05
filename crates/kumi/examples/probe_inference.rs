//! Opt-in authenticated Gate A: a conversation with the configured model and its sign-in, using only a
//! harmless nonce tool; never connects to Live. Reports JSON lines.
//!
//! Run: cargo run --release -p kumi --example probe_inference

use std::{
    cell::{Cell, RefCell},
    future::Future,
    io::Write,
    rc::Rc,
    sync::atomic::{AtomicUsize, Ordering},
    time::Duration,
};

use async_trait::async_trait;
use kumi::config::{load_inference_config, safe_error};
use kumi_common::{
    abort::{Controller, Signal, SignalExt},
    js::{
        json::stringify,
        number::round,
        string::{trim, utf16_len},
    },
    time::{iso_string, now_ms, perf_now},
};
use kumi_runtime::{
    core::contracts::{KernelEmit, StopReason},
    create_agent_kernel, open_credential_store, resolve_model, system, AgentKernel, AgentKernelOptions, JsonObject, KernelEvent,
    KernelTool, ResolveModelOptions, RuntimeError, ToolResult, KUMI, KUMI_VERSION,
};
use serde_json::{json, Value};

const INSTRUCTIONS: &str = "You are a concise integration diagnostic. Follow output instructions exactly. Call diagnostic_nonce only when explicitly requested, exactly once, then report the actual returned nonce. Never invent a tool result.";

/// Panics anywhere in the probe: what the script counted as rejections nobody handled.
static UNHANDLED: AtomicUsize = AtomicUsize::new(0);

/// What the probe shares with its interrupt handlers and its watchdog.
#[derive(Default)]
struct Probe {
    stage: Cell<&'static str>,
    kernel: RefCell<Option<AgentKernel>>,
    active: RefCell<Option<Controller>>,
    interrupted: Cell<bool>,
    watchdog_expired: Cell<bool>,
}

/// What a turn's listener saw: its words, when they began, and its tool events.
#[derive(Default)]
struct Heard {
    first_text_ms: Option<f64>,
    cancellation_at: Option<f64>,
    text: String,
    tools: Vec<KernelEvent>,
}

impl Probe {
    /// SIGINT or SIGTERM: the turn under way stops, and the probe fails at its next step.
    fn interrupt(&self) {
        self.interrupted.set(true);
        self.abort_active();
    }

    fn abort_active(&self) {
        if let Some(controller) = self.active.borrow().as_ref() {
            controller.abort();
        }
    }

    /// One turn, its words streamed to stdout and its tool events reported, checked as the gate requires.
    async fn prompt(&self, kernel: &AgentKernel, label: &'static str, input: &str, cancel_on_text: bool) -> Result<Heard, RuntimeError> {
        if self.interrupted.get() {
            return Err(RuntimeError::plain("Probe interrupted"));
        }
        self.stage.set(label);
        let controller = Controller::new();
        *self.active.borrow_mut() = Some(controller.clone());
        let started = perf_now();
        let heard = Rc::new(RefCell::new(Heard::default()));
        let emit: KernelEmit = {
            let heard = heard.clone();
            let controller = controller.clone();
            Rc::new(move |event| {
                if controller.signal.is_cancelled() {
                    return Ok(());
                }
                let mut heard = heard.borrow_mut();
                match &event {
                    KernelEvent::Text { text } if !text.is_empty() => {
                        heard.first_text_ms.get_or_insert(perf_now() - started);
                        heard.text.push_str(text);
                        if heard.text.len() > 64 * 1024 {
                            return Err(RuntimeError::plain("Probe response exceeded 64 KiB"));
                        }
                        out(&printable(text));
                        if cancel_on_text {
                            heard.cancellation_at = Some(perf_now());
                            controller.abort();
                        }
                    }
                    KernelEvent::ToolStart { name, .. } => {
                        report(json!({ "stage": label, "event": "tool-start", "tool": name }));
                        heard.tools.push(event.clone());
                    }
                    KernelEvent::ToolEnd { name, is_error, elapsed_ms, .. } => {
                        report(json!({ "stage": label, "event": "tool-end", "tool": name, "isError": is_error, "elapsedMs": elapsed_ms }));
                        heard.tools.push(event.clone());
                    }
                    _ => {}
                }
                Ok(())
            })
        };
        let outcome = async {
            let result =
                deadline(kernel.run(input, controller.signal.clone(), emit), 120_000, "Inference turn timed out", || controller.abort())
                    .await??;
            out("\n");
            if self.interrupted.get() {
                return Err(RuntimeError::plain("Probe interrupted"));
            }
            let heard = heard.take();
            let first_text_ms = heard.first_text_ms.filter(|_| !trim(&heard.text).is_empty());
            let first_text_ms = first_text_ms.ok_or_else(|| RuntimeError::plain("Model must stream nonempty text"))?;
            if cancel_on_text {
                let at = heard.cancellation_at.ok_or_else(|| RuntimeError::plain("Cancellation must occur during active streaming"))?;
                equal(json!(result.stop_reason), json!(StopReason::Cancelled))?;
                check(perf_now() - at < 5_000.0, "Cancellation exceeded five seconds")?;
            } else {
                equal(json!(result.stop_reason), json!(StopReason::Completed))?;
            }
            let mut record = json!({
                "stage": label,
                "passed": true,
                "firstTextMs": round(first_text_ms),
                "totalMs": round(perf_now() - started),
                "stopReason": result.stop_reason,
                "usage": result.usage.map_or(json!("unavailable"), |usage| json!(usage)),
            });
            if let Some(at) = heard.cancellation_at {
                record["cancellationMs"] = json!(round(perf_now() - at));
            }
            report(record);
            Ok(heard)
        }
        .await;
        controller.abort();
        self.active.borrow_mut().take();
        outcome
    }
}

/// The one tool: an unpredictable nonce, so an answer holding it used the actual result.
struct NonceTool {
    calls: Rc<Cell<u32>>,
    nonce: Rc<RefCell<Option<String>>>,
}

#[async_trait(?Send)]
impl KernelTool for NonceTool {
    fn name(&self) -> &str {
        "diagnostic_nonce"
    }
    fn description(&self) -> &str {
        "Return an unpredictable diagnostic nonce. No side effects or external data."
    }
    fn input_schema(&self) -> JsonObject {
        json!({ "type": "object", "properties": {}, "additionalProperties": false }).as_object().unwrap().clone()
    }
    async fn execute(&self, _input: JsonObject, signal: Signal) -> Result<ToolResult, RuntimeError> {
        signal.check()?;
        self.calls.set(self.calls.get() + 1);
        let nonce = hex::encode(rand::random::<[u8; 16]>());
        *self.nonce.borrow_mut() = Some(nonce.clone());
        Ok(ToolResult::text(stringify(&json!({ "nonce": nonce }))))
    }
}

async fn gate_a(probe: &Probe) -> Result<(), RuntimeError> {
    if std::env::args_os().len() != 1 {
        return Err(RuntimeError::plain("The inference probe takes no arguments; configure KUMI_MODEL and sign in locally."));
    }
    let arch = match std::env::consts::ARCH {
        "x86_64" => "x64",
        "aarch64" => "arm64",
        other => other,
    };
    // Natively there's no Node or AI SDK: Kumi's version stands for Node's, and its providers are kumi-runtime's own.
    report(json!({
        "timestamp": iso_string(now_ms()),
        "kumi": KUMI_VERSION,
        "platform": format!("{}-{arch}", system::platform()),
        "kernel": "kumi",
        "providers": { "kumi-runtime": KUMI_VERSION },
    }));
    probe.stage.set("configuration");
    let env = system::process_env();
    let config = load_inference_config(&env)?;
    let model = config.model.ok_or_else(|| {
        RuntimeError::plain(format!("No model is chosen: set KUMI_MODEL, or choose one with: {} model <provider>/<model>", *KUMI))
    })?;
    let authentication =
        if model.split('/').next() == Some("openai-codex") { "ChatGPT OAuth; credentials not logged" } else { "API key; not logged" };
    report(json!({ "stage": "configuration", "model": model, "authentication": authentication }));
    probe.stage.set("agent-create");
    let binding = resolve_model(ResolveModelOptions {
        model: model.clone(),
        store: Rc::new(open_credential_store(&config.auth_file)),
        env: Some(env),
        fetch: None,
        effort: None,
    })
    .await?;
    let calls = Rc::new(Cell::new(0));
    let nonce = Rc::new(RefCell::new(None));
    let kernel = create_agent_kernel(AgentKernelOptions {
        instructions: INSTRUCTIONS.into(),
        tools: vec![Rc::new(NonceTool { calls: calls.clone(), nonce: nonce.clone() })],
        signal: Signal::new(),
        checkpoint: None,
        binding,
        max_steps: None,
        budget: None,
    })?;
    *probe.kernel.borrow_mut() = Some(kernel.clone());
    let marker = format!("memory_{}", hex::encode(rand::random::<[u8; 12]>()));
    let remember = format!("Remember this marker for my next question: {marker}. Reply with a brief acknowledgement, without using tools.");
    probe.prompt(&kernel, "authenticated-stream", &remember, false).await?;
    let ask = "What marker did I just ask you to remember? Reply with only the exact marker; do not use tools.";
    let followup = probe.prompt(&kernel, "same-agent-followup", ask, false).await?;
    check(followup.text.contains(&marker), "Follow-up did not retain the conversation marker")?;
    let call = "Call diagnostic_nonce now, exactly once. Then reply with only the actual nonce it returned.";
    let diagnostic = probe.prompt(&kernel, "nonce-tool", call, false).await?;
    equal(json!(calls.get()), json!(1))?;
    let used = nonce.borrow().as_ref().is_some_and(|nonce| diagnostic.text.contains(nonce.as_str()));
    check(used, "Answer did not use the actual random tool result")?;
    let start = diagnostic.tools.iter().find_map(|event| match event {
        KernelEvent::ToolStart { id, name } if name == "diagnostic_nonce" => Some(id),
        _ => None,
    });
    let matched = start.is_some_and(|start| {
        diagnostic.tools.iter().any(|event| matches!(event, KernelEvent::ToolEnd { id, is_error: false, .. } if id == start))
    });
    check(matched, "Missing matching successful tool start/end events")?;
    let count = "Write a numbered list from 1 to 10000, one number per line. Do not summarize or use tools.";
    probe.prompt(&kernel, "active-cancellation", count, true).await?;
    let marker_after_cancel = format!("recovered_{}", hex::encode(rand::random::<[u8; 12]>()));
    let recover = format!("Reply with only {marker_after_cancel}. Do not use tools.");
    let recovery = probe.prompt(&kernel, "post-cancel-recovery", &recover, false).await?;
    equal(json!(trim(&recovery.text)), json!(marker_after_cancel))?;
    let settled = kernel.checkpoint()?.messages;
    check(!stringify(&json!(settled)).contains("10000, one number"), "Cancelled turn leaked into settled history")?;
    deadline(kernel.close(), 5_000, "Kernel close timed out", || {}).await?;
    probe.kernel.borrow_mut().take();
    // `setImmediate`: whatever was about to fail in the background has its turn first.
    tokio::task::yield_now().await;
    equal(json!(UNHANDLED.load(Ordering::Relaxed)), json!(0))?;
    report(json!({
        "gateA": "passed",
        "kernel": "kumi",
        "model": model,
        "toolCalls": calls.get(),
        "settledMessages": settled.len(),
        "unhandledRejections": 0,
        "kernelClosed": true,
        "cost": "unavailable; no price estimated",
    }));
    Ok(())
}

async fn run() -> i32 {
    let show_panic = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        UNHANDLED.fetch_add(1, Ordering::Relaxed);
        show_panic(info);
    }));
    let probe = Rc::new(Probe::default());
    probe.stage.set("arguments");
    let interrupts = listen_for_interrupts(&probe);
    let watchdog = tokio::task::spawn_local({
        let probe = probe.clone();
        async move {
            tokio::time::sleep(Duration::from_secs(10 * 60)).await;
            eprintln!("Gate A failed: watchdog expired; bounded shutdown could not be verified.");
            probe.watchdog_expired.set(true);
            probe.abort_active();
            let kernel = probe.kernel.borrow().clone();
            if let Some(kernel) = kernel {
                tokio::task::spawn_local(async move { kernel.close().await });
            }
            tokio::time::sleep(Duration::from_secs(5)).await;
            std::process::exit(1);
        }
    });
    // The signal handlers are in place before anything starts.
    tokio::task::yield_now().await;
    let mut code = 0;
    if let Err(error) = gate_a(&probe).await {
        code = 1;
        eprintln!("Gate A failed at {}: {}", probe.stage.get(), safe_error(Some(&error), &[]));
    }
    probe.abort_active();
    let kernel = probe.kernel.borrow().clone();
    if let Some(kernel) = kernel {
        if let Err(error) = deadline(kernel.close(), 5_000, "Kernel close timed out", || {}).await {
            code = 1;
            eprintln!("Gate A cleanup failed: {}", safe_error(Some(&error), &[]));
        }
    }
    // An expired watchdog ends the probe with 1 whatever came after it, as its exit timer did.
    if probe.watchdog_expired.get() {
        code = 1;
    }
    watchdog.abort();
    for task in interrupts {
        task.abort();
    }
    code
}

/// SIGINT and SIGTERM interrupt the probe instead of ending it.
fn listen_for_interrupts(probe: &Rc<Probe>) -> Vec<tokio::task::JoinHandle<()>> {
    #[cfg_attr(not(unix), allow(unused_mut))]
    let mut tasks = vec![tokio::task::spawn_local({
        let probe = probe.clone();
        async move {
            while tokio::signal::ctrl_c().await.is_ok() {
                probe.interrupt();
            }
        }
    })];
    #[cfg(unix)]
    {
        if let Ok(mut terminate) = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            let probe = probe.clone();
            tasks.push(tokio::task::spawn_local(async move {
                while terminate.recv().await.is_some() {
                    probe.interrupt();
                }
            }));
        }
    }
    tasks
}

/// `deadline(work, ms, message, onTimeout)`: the work's outcome, or `message` once `ms` pass (after `on_timeout`).
async fn deadline<T>(work: impl Future<Output = T>, ms: u64, message: &str, on_timeout: impl FnOnce()) -> Result<T, RuntimeError> {
    tokio::time::timeout(Duration::from_millis(ms), work).await.map_err(|_| {
        on_timeout();
        RuntimeError::plain(message)
    })
}

/// `assert(ok, message)`.
fn check(ok: bool, message: &str) -> Result<(), RuntimeError> {
    if ok {
        Ok(())
    } else {
        Err(RuntimeError::plain(message))
    }
}

/// `assert.equal(actual, expected)` (strict), with Node's message: both values on one line when they're
/// short (measured without quotes), else one line each.
fn equal(actual: Value, expected: Value) -> Result<(), RuntimeError> {
    if actual == expected {
        return Ok(());
    }
    let (shown, wanted) = (inspect(&actual), inspect(&expected));
    let quotes = [&actual, &expected].iter().filter(|value| value.is_string()).count() * 2;
    Err(RuntimeError::plain(if utf16_len(&shown) + utf16_len(&wanted) - quotes <= 12 {
        format!("Expected values to be strictly equal:\n\n{shown} !== {wanted}\n")
    } else {
        format!("Expected values to be strictly equal:\n+ actual - expected\n\n+ {shown}\n- {wanted}\n")
    }))
}

/// `util.inspect` of a string or a number, as Node's assertion messages show it.
fn inspect(value: &Value) -> String {
    let Value::String(text) = value else { return stringify(value) };
    let quote = if !text.contains('\'') {
        '\''
    } else if !text.contains('"') {
        '"'
    } else if !text.contains('`') && !text.contains("${") {
        '`'
    } else {
        '\''
    };
    let mut shown = String::from(quote);
    for c in text.chars() {
        match c {
            '\n' => shown.push_str("\\n"),
            '\t' => shown.push_str("\\t"),
            '\r' => shown.push_str("\\r"),
            '\u{8}' => shown.push_str("\\b"),
            '\u{c}' => shown.push_str("\\f"),
            '\\' => shown.push_str("\\\\"),
            '\'' if quote == '\'' => shown.push_str("\\'"),
            '\0'..='\u{1f}' | '\u{7f}'..='\u{9f}' => shown.push_str(&format!("\\x{:02X}", c as u32)),
            c => shown.push(c),
        }
    }
    shown.push(quote);
    shown
}

/// Streamed words as shown: control characters left out, newlines kept.
fn printable(text: &str) -> String {
    text.chars().filter(|c| !matches!(c, '\0'..='\u{9}' | '\u{b}'..='\u{1f}' | '\u{7f}'..='\u{9f}')).collect()
}

/// `process.stdout.write(text)`.
fn out(text: &str) {
    let mut stdout = std::io::stdout().lock();
    let _ = stdout.write_all(text.as_bytes()).and_then(|()| stdout.flush());
}

/// One JSON line on stdout.
fn report(record: Value) {
    out(&format!("{}\n", stringify(&record)));
}

fn main() {
    let runtime = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
        Ok(runtime) => runtime,
        Err(error) => {
            eprintln!("Gate A failed: {error}");
            std::process::exit(1);
        }
    };
    let code = tokio::task::LocalSet::new().block_on(&runtime, run());
    std::process::exit(code);
}
