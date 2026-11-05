//! MCP on stdio for `kumi serve`: newline-delimited JSON-RPC 2.0, MCP 2025-11-25, as Kumi's own DAW host speaks it.
//! It answers `initialize`, `ping`, `tools/list` and `tools/call`, and takes `notifications/cancelled`.
//!
//! Each call runs as a task of its own on the runtime's one thread, and its answer goes out when it's done. A call the
//! host cancels gets its signal, settles, and gets no answer (MCP's rule). One host call runs once (C2's outer dedup,
//! CC-5): a call that repeats `_host.tool_call_id` with the same arguments gets the first one's answer. When the
//! host's input ends, or the runtime is told to stop (SIGTERM), every call in flight is cancelled and given [`SETTLE`]
//! (or, while a call holds a settle guard, its own bound, 360 s at most)
//! to settle, `status.json` says the runtime stopped, and it returns.

use super::core;
use super::ctx::{Call, Ctx, Effect, Host, OpError, Operation, Out, Outbox, Progress};
use super::history::CallIdentity;
use super::{registry, say, MCP_PROTOCOL};
use futures::FutureExt;
use kumi_common::abort::{self, Signal};
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use std::cell::{Cell, RefCell};
use std::collections::{HashMap, VecDeque};
use std::future::Future;
use std::panic::AssertUnwindSafe;
use std::rc::Rc;
use std::time::Duration;
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::sync::mpsc::UnboundedReceiver;
use tokio::sync::Notify;

/// The longest message taken. A longer one is refused whole: its id can't be read, so its error has none.
pub const MAX_MESSAGE: usize = 16 * 1024 * 1024;
/// How long the calls in flight when the runtime stops get to settle before it stops anyway.
pub const SETTLE: Duration = Duration::from_secs(20);
/// How many finished host calls the dedup remembers, newest kept.
const REMEMBERED: usize = 256;
/// How often `owner.json`'s heartbeat is written: a reader takes one older than 10 s for a runtime gone.
const HEARTBEAT: Duration = Duration::from_secs(3);

const PARSE_ERROR: i64 = -32700;
const INVALID_REQUEST: i64 = -32600;
const METHOD_NOT_FOUND: i64 = -32601;
const INVALID_PARAMS: i64 = -32602;

/// Serves the registry's operations on `input` and `output` until `input` ends, the host stops reading, or the
/// process is told to stop.
pub async fn serve<I, O>(ctx: Rc<Ctx>, input: I, output: O) -> std::io::Result<()>
where
    I: AsyncRead + Unpin,
    O: AsyncWrite + Unpin + 'static,
{
    serve_with(ctx, registry::OPERATIONS, input, output, told_to_stop()).await
}

/// [`serve`] with these operations, stopping as well when `stop` is done.
pub async fn serve_with<I, O, S>(
    ctx: Rc<Ctx>,
    operations: &'static [&'static Operation],
    input: I,
    output: O,
    stop: S,
) -> std::io::Result<()>
where
    I: AsyncRead + Unpin,
    O: AsyncWrite + Unpin + 'static,
    S: Future<Output = ()>,
{
    ctx.offer(operations);
    let (outbox, outgoing) = tokio::sync::mpsc::unbounded_channel();
    let mut writer = Some(tokio::task::spawn_local(write_out(output, outgoing)));
    core::publish_status(&ctx, false).await;
    let heartbeat = tokio::task::spawn_local(beat(ctx.clone()));
    let server = Rc::new(Server {
        ctx,
        operations,
        outbox,
        initialized: Cell::new(false),
        calls: RefCell::new(HashMap::new()),
        settled: Notify::new(),
        seen: RefCell::new(Seen::default()),
    });
    let mut input = BufReader::new(input);
    tokio::pin!(stop);
    let read = loop {
        let Some(written) = writer.as_mut() else { break Ok(()) };
        tokio::select! {
            line = read_line(&mut input, MAX_MESSAGE) => match line {
                Ok(Some(Line::Message(bytes))) => server.take(&bytes),
                Ok(Some(Line::TooLong)) => server.send_error(Value::Null, INVALID_REQUEST, "a message is at most 16 MiB"),
                Ok(None) => break Ok(()),
                Err(error) => break Err(error),
            },
            done = written => {
                // The host stopped reading: nothing more can be answered.
                writer = None;
                if let Ok(Err(error)) = done {
                    say(&format!("kumi serve: the host stopped reading: {error}"));
                }
                break Ok(());
            }
            () = &mut stop => {
                say("kumi serve: told to stop");
                break Ok(());
            }
        }
    };
    // Every call's signal has the runtime's shutdown in it.
    server.ctx.shutdown().cancel();
    let settling = async {
        server.all_settled().await;
        server.ctx.jobs().all_settled().await;
    };
    if !settle(&server.ctx, settling).await {
        let (calls, jobs) = (server.calls.borrow().len(), server.ctx.jobs().running());
        say(&format!("kumi serve: {calls} call(s) and {jobs} job(s) still settling at the stop's bound; stopping"));
    }
    server.ctx.set_settling_until(None);
    server.ctx.jobs().interrupt_running(server.ctx.snapshots());
    heartbeat.abort();
    server.ctx.link().close().await;
    core::publish_stopped(&server.ctx).await;
    let _ = server.outbox.send(Out::Close);
    if let Some(written) = writer {
        let _ = tokio::time::timeout(Duration::from_secs(5), written).await;
    }
    read
}

/// Fires when the process is told to stop: SIGTERM or SIGHUP (on Windows, the console closing or the system shutting
/// down). Ctrl-C (SIGINT) is the host's: the runtime keeps working through it.
async fn told_to_stop() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};
        let (Ok(mut term), Ok(mut hangup), Ok(mut interrupt)) =
            (signal(SignalKind::terminate()), signal(SignalKind::hangup()), signal(SignalKind::interrupt()))
        else {
            say("kumi serve: no signal handling; it stops when its input ends");
            return std::future::pending().await;
        };
        loop {
            tokio::select! {
                _ = term.recv() => return,
                _ = hangup.recv() => return,
                _ = interrupt.recv() => {}
            }
        }
    }
    #[cfg(windows)]
    {
        use tokio::signal::windows::{ctrl_break, ctrl_c, ctrl_close, ctrl_shutdown};
        let (Ok(mut close), Ok(mut shutdown), Ok(mut interrupt), Ok(mut brk)) = (ctrl_close(), ctrl_shutdown(), ctrl_c(), ctrl_break())
        else {
            say("kumi serve: no console signal handling; it stops when its input ends");
            return std::future::pending().await;
        };
        loop {
            tokio::select! {
                _ = close.recv() => return,
                _ = shutdown.recv() => return,
                _ = interrupt.recv() => {}
                _ = brk.recv() => {}
            }
        }
    }
}

/// `owner.json`'s heartbeat, from the start until the runtime stops.
async fn beat(ctx: Rc<Ctx>) {
    loop {
        core::publish_owner(&ctx, true);
        tokio::time::sleep(HEARTBEAT).await;
    }
}

/// The runtime's side of the session.
struct Server {
    ctx: Rc<Ctx>,
    operations: &'static [&'static Operation],
    outbox: Outbox,
    initialized: Cell<bool>,
    /// The calls running, by their request id (as JSON, so `7` and `"7"` stay apart).
    calls: RefCell<HashMap<String, Rc<InFlight>>>,
    /// Told when the last call running ends.
    settled: Notify,
    seen: RefCell<Seen>,
}

struct InFlight {
    signal: Signal,
    progress: Progress,
    /// The host cancelled it: it gets no answer.
    cancelled: Cell<bool>,
}

/// The host calls seen (`_host.tool_call_id`): what each asked (its operation and arguments, digested) and its answer
/// once it has one.
#[derive(Default)]
struct Seen {
    calls: HashMap<String, Rc<SeenCall>>,
    /// The finished ones, oldest first: past [`REMEMBERED`], the oldest is forgotten.
    finished: VecDeque<String>,
}

struct SeenCall {
    digest: String,
    answer: RefCell<Option<Value>>,
    answered: Notify,
}

/// A `tools/call` the runtime can run.
struct Prepared {
    operation: &'static Operation,
    args: Map<String, Value>,
    host: Host,
    token: Option<Value>,
}

/// Why a `tools/call` won't run: the request is wrong (a JSON-RPC error), or the call is (the tool's own error, so
/// the model can fix it).
enum Refusal {
    Request(i64, String),
    Call(OpError),
}

impl Server {
    /// One message from the host.
    fn take(self: &Rc<Self>, bytes: &[u8]) {
        let bytes = bytes.trim_ascii();
        if bytes.is_empty() {
            return;
        }
        let message = match serde_json::from_slice::<Value>(bytes) {
            Ok(Value::Object(message)) => message,
            Ok(_) => return self.send_error(Value::Null, INVALID_REQUEST, "a message is one JSON-RPC object"),
            Err(error) => return self.send_error(Value::Null, PARSE_ERROR, &format!("not JSON: {error}")),
        };
        let id = message.get("id").cloned();
        if id.as_ref().is_some_and(|id| !(id.is_string() || id.is_i64() || id.is_u64())) {
            return self.send_error(Value::Null, INVALID_REQUEST, "an id is a string or a whole number");
        }
        if message.get("jsonrpc").and_then(Value::as_str) != Some("2.0") {
            return self.send_error(id.unwrap_or(Value::Null), INVALID_REQUEST, "jsonrpc is \"2.0\"");
        }
        let params = message.get("params").unwrap_or(&Value::Null);
        match (message.get("method").and_then(Value::as_str), id) {
            (Some(method), Some(id)) => self.request(method, id, params),
            (Some(method), None) => self.notification(method, params),
            // An answer: the runtime asks the host nothing.
            (None, Some(_)) if message.contains_key("result") || message.contains_key("error") => {}
            (None, id) => self.send_error(id.unwrap_or(Value::Null), INVALID_REQUEST, "a request names its method"),
        }
    }

    fn request(self: &Rc<Self>, method: &str, id: Value, params: &Value) {
        match method {
            "initialize" => {
                self.initialized.set(true);
                self.send_result(
                    id,
                    json!({
                        "protocolVersion": MCP_PROTOCOL,
                        "capabilities": {"tools": {"listChanged": false}},
                        "serverInfo": {"name": "kumi-serve", "version": env!("CARGO_PKG_VERSION")},
                    }),
                );
            }
            "ping" => self.send_result(id, json!({})),
            _ if !self.initialized.get() => self.send_error(id, INVALID_REQUEST, "initialize first"),
            "tools/list" => {
                let tools: Vec<Value> = self
                    .operations
                    .iter()
                    .map(|operation| {
                        json!({"name": operation.name, "description": operation.description, "inputSchema": (operation.schema)()})
                    })
                    .collect();
                self.send_result(id, json!({"tools": tools}));
            }
            "tools/call" => self.call(id, params),
            _ => self.send_error(id, METHOD_NOT_FOUND, &format!("no method \"{method}\"")),
        }
    }

    fn notification(&self, method: &str, params: &Value) {
        if method != "notifications/cancelled" {
            return;
        }
        let Some(id) = params.get("requestId") else { return };
        if let Some(flight) = self.calls.borrow().get(&id.to_string()) {
            flight.cancelled.set(true);
            flight.progress.end();
            flight.signal.cancel();
        }
    }

    /// A `tools/call`: checked now (so a cancel right behind it finds it), run as a task of its own.
    fn call(self: &Rc<Self>, id: Value, params: &Value) {
        let key = id.to_string();
        if self.calls.borrow().contains_key(&key) {
            return self.send_error(id, INVALID_REQUEST, "a call with that id is still running");
        }
        let prepared = match self.prepare(params) {
            Ok(prepared) => prepared,
            Err(Refusal::Request(code, message)) => return self.send_error(id, code, &message),
            Err(Refusal::Call(error)) => return self.send_result(id, tool_result(Err(error))),
        };
        let effect = (prepared.operation.effect)(&prepared.args);
        let asked = digest(prepared.operation.name, &prepared.args);
        // A call that only reads is run again; any other is run once per host call.
        let seen = match (effect, &prepared.host.tool_call_id) {
            (Effect::Read, _) | (_, None) => None,
            (_, Some(host_call)) => match self.seen(host_call, &asked) {
                Ok(seen) => Some(seen),
                Err(refused) => return self.send_result(id, tool_result(Err(refused))),
            },
        };
        let own = Signal::new();
        let progress = Progress::new(prepared.token.clone(), self.outbox.clone());
        let flight = Rc::new(InFlight { signal: own.clone(), progress: progress.clone(), cancelled: Cell::new(false) });
        self.calls.borrow_mut().insert(key.clone(), flight.clone());
        self.ctx.snapshots().note_host(&prepared.host);
        let server = self.clone();
        let operation = prepared.operation;
        let host = prepared.host.clone();
        let title = prepared.args.get("title").and_then(Value::as_str).map(str::to_string);
        let call = Call { args: prepared.args, host: prepared.host, signal: abort::any([own, self.ctx.shutdown().clone()]), progress };
        tokio::task::spawn_local(async move {
            let answer = match &seen {
                Some((earlier, false)) => {
                    // The same host call is running, or has run: its answer is this one's.
                    loop {
                        let answered = earlier.answered.notified();
                        if let Some(answer) = earlier.answer.borrow().clone() {
                            break answer;
                        }
                        answered.await;
                    }
                }
                _ if effect.writes() => server.write(operation, call, &host, &asked, title.as_deref()).await,
                _ => {
                    let ran = AssertUnwindSafe((operation.run)(&server.ctx, call)).catch_unwind().await;
                    tool_result(ran.unwrap_or_else(|_| {
                        say(&format!("kumi serve: {} failed unexpectedly", operation.name));
                        Err(OpError::Failed { reason: format!("{} failed unexpectedly; nothing more is known", operation.name) })
                    }))
                }
            };
            server.finish(&seen, &flight, &key, id, answer);
        });
    }

    /// A call that writes, plays or imports: under the endpoint's lease, held until it has settled, and on disk while
    /// it runs (R4), so a runtime that dies in the middle leaves word of it.
    async fn write(&self, operation: &'static Operation, call: Call, host: &Host, asked: &str, title: Option<&str>) -> Value {
        let held = match self.ctx.lease().take(host).await {
            Ok(held) => held,
            Err(refused) => return tool_result(Err(refused)),
        };
        super::core::publish_owner(&self.ctx, true);
        let identity = CallIdentity { tool_call_id: host.tool_call_id.as_deref(), digest: asked };
        let answer = match super::inflight::begin(&self.ctx, operation.name, title, identity) {
            Err(error) => tool_result(Err(error)),
            Ok(record) => {
                let ran = AssertUnwindSafe((operation.run)(&self.ctx, call)).catch_unwind().await;
                let answer = match ran {
                    Ok(outcome) => tool_result(outcome),
                    Err(_) => tool_result(Ok(self.broke(operation.name, title, identity).await)),
                };
                record.settle();
                answer
            }
        };
        drop(held);
        super::core::publish_owner(&self.ctx, true);
        answer
    }

    /// A write that failed unexpectedly part way may have changed the DAW: it's unsure (CC-2's W1-b), never "failed".
    /// Its rows so far and an audit row, the epoch moved on, and `status.unsure` until the next look.
    async fn broke(&self, op: &str, title: Option<&str>, identity: CallIdentity<'_>) -> Value {
        say(&format!("kumi serve: {op} failed unexpectedly"));
        let history = self.ctx.history();
        let rows = history.end_call(identity, self.ctx.snapshots());
        let reason = format!("{op} failed unexpectedly part way, so the DAW may have changed: look at the project before another change.");
        let project = self.ctx.link().project().map(|project| project.key).unwrap_or_default();
        let audit = history.audit(title.unwrap_or(op), &project, &reason, identity, self.ctx.snapshots());
        self.ctx.link().note_unsure(op, &reason, "unsure");
        self.ctx.link().advance();
        super::core::publish_status(&self.ctx, false).await;
        let ids: Vec<String> = rows.into_iter().map(|row| row.id).chain([audit.id]).collect();
        history.call_came_to(&ids, "unsure", self.ctx.snapshots());
        json!({"state": "unsure", "reason": reason, "history_ids": ids, "undo_available": false})
    }

    /// A call is done with `answer`: a host call's first remembers it for its repeats, the call leaves the running ones,
    /// and its answer goes out unless the host cancelled it.
    fn finish(&self, seen: &Option<(Rc<SeenCall>, bool)>, flight: &InFlight, key: &str, id: Value, answer: Value) {
        if let Some((first, true)) = seen {
            *first.answer.borrow_mut() = Some(answer.clone());
            first.answered.notify_waiters();
            // A refusal about the moment (the lease held elsewhere, too many jobs, the DAW away) isn't the call's
            // answer for good: the same call asked again later runs.
            if momentary(&answer) {
                self.forget(first);
            } else {
                self.remember(first);
            }
        }
        flight.progress.end();
        self.calls.borrow_mut().remove(key);
        if self.calls.borrow().is_empty() {
            self.settled.notify_waiters();
        }
        if !flight.cancelled.get() {
            self.send_result(id, answer);
        }
    }

    /// The host call `host_call` asking `digest`: (it, true) when it's new and this call runs it; (it, false) when
    /// it's running or has run, and this call takes its answer. Refused when it asked something else. One the
    /// in-memory table doesn't know (an earlier runtime's, or one long forgotten) is looked for in HISTORY (C5-3).
    fn seen(&self, host_call: &str, digest: &str) -> Result<(Rc<SeenCall>, bool), OpError> {
        let duplicate = |reason: String| OpError::Refused { code: "duplicate", reason, owner: None };
        if let Some(earlier) = self.seen.borrow().calls.get(host_call) {
            if earlier.digest != digest {
                return Err(duplicate("This host call already ran with other arguments: a call is run once, as first asked.".into()));
            }
            return Ok((earlier.clone(), false));
        }
        let new = Rc::new(SeenCall { digest: digest.to_string(), answer: RefCell::new(None), answered: Notify::new() });
        match self.ctx.history().ran(host_call, digest) {
            Some(Err(reason)) => Err(duplicate(reason)),
            Some(Ok(outcome)) => {
                *new.answer.borrow_mut() = Some(tool_result(Ok(outcome)));
                self.seen.borrow_mut().calls.insert(host_call.to_string(), new.clone());
                self.remember(&new);
                Ok((new, false))
            }
            None => {
                self.seen.borrow_mut().calls.insert(host_call.to_string(), new.clone());
                Ok((new, true))
            }
        }
    }

    /// A host call whose answer isn't kept: asked again, it runs.
    fn forget(&self, finished: &Rc<SeenCall>) {
        self.seen.borrow_mut().calls.retain(|_, call| !Rc::ptr_eq(call, finished));
    }

    /// A host call that has its answer joins the remembered ones; past [`REMEMBERED`], the oldest is forgotten.
    fn remember(&self, finished: &Rc<SeenCall>) {
        let mut seen = self.seen.borrow_mut();
        let Some(host_call) = seen.calls.iter().find(|(_, call)| Rc::ptr_eq(call, finished)).map(|(id, _)| id.clone()) else { return };
        seen.finished.push_back(host_call);
        while seen.finished.len() > REMEMBERED {
            if let Some(oldest) = seen.finished.pop_front() {
                seen.calls.remove(&oldest);
            }
        }
    }

    fn prepare(&self, params: &Value) -> Result<Prepared, Refusal> {
        let Value::Object(params) = params else {
            return Err(Refusal::Request(INVALID_PARAMS, "tools/call takes {name, arguments}".into()));
        };
        let name = params
            .get("name")
            .and_then(Value::as_str)
            .ok_or_else(|| Refusal::Request(INVALID_PARAMS, "tools/call names its tool".into()))?;
        let operation = self
            .operations
            .iter()
            .copied()
            .find(|operation| operation.name == name)
            .ok_or_else(|| Refusal::Request(INVALID_PARAMS, format!("Unknown tool: {name}")))?;
        let mut args = match params.get("arguments") {
            None | Some(Value::Null) => Map::new(),
            Some(Value::Object(args)) => args.clone(),
            Some(_) => return Err(Refusal::Request(INVALID_PARAMS, "arguments is an object".into())),
        };
        let host = match args.remove("_host") {
            None => Host::default(),
            Some(host) => Host::from_value(&host).map_err(|reason| Refusal::Call(OpError::Invalid { reason }))?,
        };
        let args = Value::Object(args);
        if let Some(reason) = kumi_agent::kernel::schema::problem(&(operation.schema)(), &args) {
            return Err(Refusal::Call(OpError::Invalid { reason }));
        }
        let Value::Object(args) = args else { return Err(Refusal::Request(INVALID_PARAMS, "arguments is an object".into())) };
        let token = params
            .get("_meta")
            .and_then(|meta| meta.get("progressToken"))
            .filter(|token| token.is_string() || token.is_i64() || token.is_u64())
            .cloned();
        Ok(Prepared { operation, args, host, token })
    }

    async fn all_settled(&self) {
        loop {
            let settled = self.settled.notified();
            if self.calls.borrow().is_empty() {
                return;
            }
            settled.await;
        }
    }

    fn send(&self, message: Value) {
        let _ = self.outbox.send(Out::Message(message));
    }

    fn send_result(&self, id: Value, result: Value) {
        self.send(json!({"jsonrpc": "2.0", "id": id, "result": result}));
    }

    fn send_error(&self, id: Value, code: i64, message: &str) {
        self.send(json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}}));
    }
}

/// Waits for `settled` up to [`SETTLE`], or while a call's guard lives up to the largest bound it holds
/// ([`super::ctx::SETTLE_MOST`] at most), read again each second, since a guard taken as the stop began counts too.
/// While it waits on a guard, `owner.json` says until when. Whether everything settled.
async fn settle(ctx: &Ctx, settled: impl Future<Output = ()>) -> bool {
    let started = tokio::time::Instant::now();
    tokio::pin!(settled);
    loop {
        let guarded = ctx.settle_bound();
        let deadline = started + guarded.map_or(SETTLE, |bound| bound.max(SETTLE));
        let left = deadline.saturating_duration_since(tokio::time::Instant::now());
        let until = guarded.map(|_| kumi_common::time::now_ms() + left.as_millis() as i64);
        if until.is_some() != ctx.settling_until().is_some() {
            ctx.set_settling_until(until);
            core::publish_owner(ctx, true);
        }
        let tick = deadline.min(tokio::time::Instant::now() + Duration::from_secs(1));
        tokio::select! {
            () = &mut settled => return true,
            () = tokio::time::sleep_until(tick) => {}
        }
        if tokio::time::Instant::now() >= deadline {
            return false;
        }
    }
}

/// What a call asks, digested (CC-5): its operation's name and its arguments (`_host` left out), written with every
/// object's keys sorted, so the same question digests the same however its fields were ordered. As HISTORY keeps it.
pub(crate) fn digest(operation: &str, args: &Map<String, Value>) -> String {
    let mut text = operation.as_bytes().to_vec();
    text.push(0);
    canonical(&Value::Object(args.clone()), &mut text);
    Sha256::digest(&text).iter().map(|byte| format!("{byte:02x}")).collect()
}

fn canonical(value: &Value, out: &mut Vec<u8>) {
    match value {
        Value::Object(fields) => {
            let mut keys: Vec<&String> = fields.keys().collect();
            keys.sort_unstable();
            out.push(b'{');
            for (n, key) in keys.into_iter().enumerate() {
                if n > 0 {
                    out.push(b',');
                }
                out.extend(serde_json::to_vec(key).unwrap_or_default());
                out.push(b':');
                canonical(&fields[key], out);
            }
            out.push(b'}');
        }
        Value::Array(items) => {
            out.push(b'[');
            for (n, item) in items.iter().enumerate() {
                if n > 0 {
                    out.push(b',');
                }
                canonical(item, out);
            }
            out.push(b']');
        }
        other => out.extend(serde_json::to_vec(other).unwrap_or_default()),
    }
}

/// A call's outcome as MCP's tool result: the value as text and as `structuredContent`; an [`OpError`] with `isError`.
pub fn tool_result(outcome: Result<Value, OpError>) -> Value {
    let (value, is_error) = match outcome {
        Ok(Value::Object(fields)) => (Value::Object(fields), false),
        Ok(other) => (json!({"value": other}), false),
        Err(error) => (error.to_value(), true),
    };
    json!({"content": [{"type": "text", "text": value.to_string()}], "structuredContent": value, "isError": is_error})
}

/// A refusal about the moment, not the call: another runtime holds the DAW's lease, the jobs are full, the DAW isn't
/// there, or the project moved on since the plan was made. Whether it comes as an error or as an operation's own
/// `refused` answer, the same call asked again later may run.
fn momentary(answer: &Value) -> bool {
    let content = &answer["structuredContent"];
    let code = if answer["isError"] == true {
        content["error"]["code"].as_str()
    } else {
        content["code"].as_str().filter(|_| content["state"] == "refused")
    };
    matches!(code, Some("leased" | "busy" | "not_connected" | "stale"))
}

/// Writes the messages out, one line each, until told to close (or the host stops reading).
async fn write_out<O: AsyncWrite + Unpin>(mut output: O, mut outgoing: UnboundedReceiver<Out>) -> std::io::Result<()> {
    while let Some(Out::Message(message)) = outgoing.recv().await {
        let mut line = serde_json::to_vec(&message).map_err(std::io::Error::other)?;
        line.push(b'\n');
        output.write_all(&line).await?;
        output.flush().await?;
    }
    output.flush().await
}

#[derive(Debug, PartialEq, Eq)]
enum Line {
    Message(Vec<u8>),
    TooLong,
}

/// The next line of input without its newline (None at the end); a line over `most` bytes is read through but not
/// kept.
async fn read_line<R: AsyncBufRead + Unpin>(input: &mut R, most: usize) -> std::io::Result<Option<Line>> {
    let mut line = Vec::new();
    let mut too_long = false;
    loop {
        let available = input.fill_buf().await?;
        if available.is_empty() {
            return Ok(match (too_long, line.is_empty()) {
                (true, _) => Some(Line::TooLong),
                (false, true) => None,
                (false, false) => Some(Line::Message(line)),
            });
        }
        let (taken, ended) = match memchr::memchr(b'\n', available) {
            Some(at) => (at, true),
            None => (available.len(), false),
        };
        if !too_long {
            if line.len() + taken > most {
                too_long = true;
                line = Vec::new();
            } else {
                line.extend_from_slice(&available[..taken]);
            }
        }
        input.consume(taken + usize::from(ended));
        if ended {
            return Ok(Some(if too_long { Line::TooLong } else { Line::Message(line) }));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::serve::ctx::{reads, JsonObject, OpFuture};
    use crate::serve::{environment, Args};
    use kumi_types::daw::DawId;
    use kumi_types::system::Env;
    use std::collections::BTreeMap;
    use tokio::io::DuplexStream;

    thread_local! {
        static SETTLED: Cell<bool> = const { Cell::new(false) };
        static RUNS: Cell<u32> = const { Cell::new(0) };
    }

    fn object() -> Value {
        json!({"type": "object", "additionalProperties": false, "properties": {
            "word": {"type": "string", "maxLength": 8}, "hold": {"type": "boolean"}
        }})
    }

    /// Waits for its signal, then takes a moment to put things back.
    static WAIT: Operation = Operation { name: "wait", description: "Waits to be cancelled.", schema: object, effect: reads, run: wait };
    fn wait(_ctx: &Ctx, call: Call) -> OpFuture<'_> {
        Box::pin(async move {
            call.signal.cancelled().await;
            tokio::time::sleep(Duration::from_millis(20)).await;
            SETTLED.with(|settled| settled.set(true));
            Ok(json!({"state": "cancelled"}))
        })
    }

    /// Reports progress and answers with its arguments and who asked.
    static ECHO: Operation = Operation { name: "echo", description: "Says it back.", schema: object, effect: reads, run: echo };
    fn echo(_ctx: &Ctx, call: Call) -> OpFuture<'_> {
        Box::pin(async move {
            call.progress.report(1.0, Some(2.0), Some("halfway"));
            Ok(json!({"args": call.args, "host": call.host}))
        })
    }

    static BOOM: Operation = Operation { name: "boom", description: "Fails unexpectedly.", schema: object, effect: reads, run: boom };
    fn boom(_ctx: &Ctx, _call: Call) -> OpFuture<'_> {
        Box::pin(async move { panic!("boom") })
    }

    /// Reads a folder (an effect, so run once per host call): counts its runs; with `hold`, until it's cancelled.
    static SCAN: Operation = Operation { name: "scan", description: "Scans.", schema: object, effect: folder_read, run: scan };
    fn folder_read(_: &JsonObject) -> Effect {
        Effect::FolderRead
    }
    fn scan(_ctx: &Ctx, call: Call) -> OpFuture<'_> {
        Box::pin(async move {
            let run = RUNS.with(|runs| {
                runs.set(runs.get() + 1);
                runs.get()
            });
            if call.args.get("hold") == Some(&json!(true)) {
                call.signal.cancelled().await;
                return Ok(json!({"run": run, "state": "cancelled"}));
            }
            tokio::time::sleep(Duration::from_millis(30)).await;
            Ok(json!({"run": run}))
        })
    }

    /// Writes to the DAW, under the lease: counts its runs.
    static PLACE: Operation = Operation { name: "place", description: "Places.", schema: object, effect: writes, run: place };
    fn writes(_: &JsonObject) -> Effect {
        Effect::Write
    }
    fn place(_ctx: &Ctx, _call: Call) -> OpFuture<'_> {
        Box::pin(async move {
            RUNS.with(|runs| runs.set(runs.get() + 1));
            Ok(json!({"state": "applied"}))
        })
    }

    /// Writes to the DAW, and fails unexpectedly part way.
    static SMASH: Operation = Operation { name: "smash", description: "Fails part way.", schema: object, effect: writes, run: boom };

    /// Writes to the DAW until it's cancelled, then says so.
    static HOLD: Operation = Operation { name: "hold", description: "Writes until cancelled.", schema: object, effect: writes, run: hold };
    fn hold(_ctx: &Ctx, call: Call) -> OpFuture<'_> {
        Box::pin(async move {
            RUNS.with(|runs| runs.set(runs.get() + 1));
            call.signal.cancelled().await;
            Ok(json!({"state": "unsure", "reason": "stopped part way"}))
        })
    }

    /// Renders until it's cancelled, then puts the DAW back for `restore` seconds, under a guard of `bound` seconds.
    static RESTORE: Operation =
        Operation { name: "restore", description: "Renders, then restores.", schema: object, effect: reads, run: restore };
    fn restore(ctx: &Ctx, call: Call) -> OpFuture<'_> {
        Box::pin(async move {
            let seconds = |name: &str| {
                call.args
                    .get("word")
                    .and_then(Value::as_str)
                    .and_then(|word| word.split(',').find_map(|part| part.strip_prefix(name)?.parse::<u64>().ok()))
            };
            let _guard = ctx.settling(Duration::from_secs(seconds("b").unwrap_or(0)));
            call.signal.cancelled().await;
            tokio::time::sleep(Duration::from_secs(seconds("r").unwrap_or(0))).await;
            SETTLED.with(|settled| settled.set(true));
            Ok(json!({"state": "restored"}))
        })
    }

    static OPERATIONS: &[&Operation] = &[&WAIT, &ECHO, &BOOM, &SCAN, &PLACE, &SMASH, &HOLD, &RESTORE];

    struct Session {
        to: Option<DuplexStream>,
        from: BufReader<DuplexStream>,
        served: tokio::task::JoinHandle<std::io::Result<()>>,
        stop: Option<tokio::sync::oneshot::Sender<()>>,
        ctx: Rc<Ctx>,
        _state: tempfile::TempDir,
    }

    impl Session {
        fn start() -> Session {
            Session::at(tempfile::tempdir().unwrap())
        }

        /// A runtime on `state`'s folders (another runtime's, once it stopped).
        fn at(state: tempfile::TempDir) -> Session {
            let args = Args {
                state_dir: state.path().join("state"),
                kumi_home: state.path().join("home"),
                daw: DawId::Reaper,
                library_roots: vec![],
                media_roots: vec![],
                tools: BTreeMap::new(),
            };
            let given = Env::from([("HOME".into(), state.path().display().to_string()), ("KUMI_PROCESSES".into(), "none".into())]);
            let env = environment(&args, given);
            let ctx = Ctx::new(args, env, Default::default()).unwrap();
            // A pipe each way: dropping the host's end of its input ends the runtime's.
            let (to, server_in) = tokio::io::duplex(1 << 16);
            let (server_out, from) = tokio::io::duplex(1 << 16);
            let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
            let stopped = async move {
                let _ = stopped.await;
            };
            let served = tokio::task::spawn_local(serve_with(ctx.clone(), OPERATIONS, server_in, server_out, stopped));
            Session { to: Some(to), from: BufReader::new(from), served, stop: Some(stop), ctx, _state: state }
        }

        /// Stops the runtime, keeping its folders for the next.
        async fn stopped(mut self) -> tempfile::TempDir {
            self.end().await;
            self._state
        }

        async fn send(&mut self, message: Value) {
            self.send_line(&message.to_string()).await;
        }

        async fn send_line(&mut self, line: &str) {
            let to = self.to.as_mut().unwrap();
            to.write_all(format!("{line}\n").as_bytes()).await.unwrap();
            to.flush().await.unwrap();
        }

        async fn next(&mut self) -> Option<Value> {
            let mut line = String::new();
            let read = tokio::time::timeout(Duration::from_secs(10), self.from.read_line(&mut line)).await.expect("an answer in time");
            (read.unwrap() > 0).then(|| serde_json::from_str(&line).unwrap())
        }

        async fn initialized(mut self) -> Session {
            self.send(json!({"jsonrpc": "2.0", "id": 0, "method": "initialize", "params": {"protocolVersion": MCP_PROTOCOL}})).await;
            self.next().await.unwrap();
            self.send(json!({"jsonrpc": "2.0", "method": "notifications/initialized"})).await;
            self
        }

        /// What else came out before the runtime stopped. Its state folder stays until the session goes.
        async fn rest(&mut self) -> Vec<Value> {
            let mut rest = vec![];
            while let Some(message) = self.next().await {
                rest.push(message);
            }
            tokio::time::timeout(Duration::from_secs(10), &mut self.served).await.unwrap().unwrap().unwrap();
            rest
        }

        /// Ends the input; what else came out before the runtime stopped.
        async fn end(&mut self) -> Vec<Value> {
            drop(self.to.take());
            self.rest().await
        }

        /// A snapshot as it is on disk once every write so far is done.
        fn state(&self, file: &str) -> Value {
            self.ctx.snapshots().flush();
            serde_json::from_slice(&std::fs::read(self._state.path().join("state").join(file)).unwrap()).unwrap()
        }
    }

    fn call(id: Value, name: &str, arguments: Value) -> Value {
        json!({"jsonrpc": "2.0", "id": id, "method": "tools/call", "params": {"name": name, "arguments": arguments}})
    }

    async fn local<F: Future<Output = ()>>(test: F) {
        tokio::task::LocalSet::new().run_until(test).await;
    }

    #[tokio::test]
    async fn a_session_lists_its_tools_and_calls_one_with_progress() {
        local(async {
            let mut session = Session::start();
            session.send(json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {"protocolVersion": "2025-06-18"}})).await;
            let initialized = session.next().await.unwrap();
            assert_eq!(initialized["result"]["protocolVersion"], MCP_PROTOCOL, "the one version it speaks");
            assert_eq!(initialized["result"]["serverInfo"], json!({"name": "kumi-serve", "version": env!("CARGO_PKG_VERSION")}));
            assert_eq!(initialized["result"]["capabilities"], json!({"tools": {"listChanged": false}}));
            session.send(json!({"jsonrpc": "2.0", "method": "notifications/initialized"})).await;
            session.send(json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list"})).await;
            let listed = session.next().await.unwrap();
            let names: Vec<&str> = listed["result"]["tools"].as_array().unwrap().iter().filter_map(|tool| tool["name"].as_str()).collect();
            assert_eq!(names, ["wait", "echo", "boom", "scan", "place", "smash", "hold", "restore"]);
            assert_eq!(listed["result"]["tools"][1]["inputSchema"], object());
            let mut asked = call(json!("c3"), "echo", json!({"word": "hi", "_host": {"profile_id": "default", "tool_call_id": "t9"}}));
            asked["params"]["_meta"] = json!({"progressToken": "p3"});
            session.send(asked).await;
            let progress = session.next().await.unwrap();
            assert_eq!(progress["method"], "notifications/progress");
            assert_eq!(progress["params"], json!({"progressToken": "p3", "progress": 1.0, "total": 2.0, "message": "halfway"}));
            let answered = session.next().await.unwrap();
            assert_eq!(answered["id"], "c3");
            assert_eq!(answered["result"]["isError"], false);
            assert_eq!(
                answered["result"]["structuredContent"],
                json!({"args": {"word": "hi"}, "host": {"profile_id": "default", "tool_call_id": "t9"}}),
                "_host is taken out of the arguments"
            );
            let text: Value = serde_json::from_str(answered["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
            assert_eq!(text, answered["result"]["structuredContent"]);
            assert!(session.end().await.is_empty());
        })
        .await;
    }

    #[tokio::test]
    async fn a_wrong_message_gets_its_error_and_the_session_goes_on() {
        local(async {
            let mut session = Session::start();
            session.send(json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list"})).await;
            assert_eq!(session.next().await.unwrap()["error"]["message"], "initialize first");
            let mut session = session.initialized().await;
            session.send_line("{not json").await;
            let parse = session.next().await.unwrap();
            assert_eq!((parse["id"].clone(), parse["error"]["code"].clone()), (Value::Null, json!(PARSE_ERROR)));
            session.send_line("[1, 2]").await;
            assert_eq!(session.next().await.unwrap()["error"]["code"], INVALID_REQUEST);
            session.send(json!({"jsonrpc": "2.0", "id": 2, "method": "resources/list"})).await;
            assert_eq!(session.next().await.unwrap()["error"]["code"], METHOD_NOT_FOUND);
            session.send(call(json!(3), "observe", json!({}))).await;
            let unknown = session.next().await.unwrap();
            assert_eq!((unknown["id"].clone(), unknown["error"]["code"].clone()), (json!(3), json!(INVALID_PARAMS)));
            session.send(call(json!(4), "echo", json!({"word": "hi", "yes": true}))).await;
            let invalid = session.next().await.unwrap();
            assert_eq!(invalid["result"]["isError"], true, "the model can fix its arguments");
            assert_eq!(invalid["result"]["structuredContent"]["error"]["code"], "invalid");
            assert!(invalid["result"]["structuredContent"]["error"]["reason"].as_str().unwrap().contains("yes"));
            session.send(call(json!(5), "echo", json!({"_host": {"approved": true}}))).await;
            assert_eq!(session.next().await.unwrap()["result"]["structuredContent"]["error"]["code"], "invalid");
            session.send(call(json!(6), "boom", json!({}))).await;
            let failed = session.next().await.unwrap();
            assert_eq!((failed["id"].clone(), failed["result"]["isError"].clone()), (json!(6), json!(true)));
            assert_eq!(failed["result"]["structuredContent"]["error"]["code"], "failed");
            session.send(json!({"jsonrpc": "2.0", "id": 7, "method": "ping"})).await;
            assert_eq!(session.next().await.unwrap(), json!({"jsonrpc": "2.0", "id": 7, "result": {}}));
            assert!(session.end().await.is_empty());
        })
        .await;
    }

    #[tokio::test]
    async fn a_cancelled_call_settles_and_gets_no_answer() {
        local(async {
            let mut session = Session::start().initialized().await;
            session.send(call(json!(5), "wait", json!({}))).await;
            session.send(call(json!(5), "wait", json!({"yes": true}))).await;
            let again = session.next().await.unwrap();
            assert_eq!(
                (again["id"].clone(), again["error"]["code"].clone()),
                (json!(5), json!(INVALID_REQUEST)),
                "an id in use, whatever else is wrong with the call"
            );
            session.send(json!({"jsonrpc": "2.0", "method": "notifications/cancelled", "params": {"requestId": "5"}})).await;
            session
                .send(json!({"jsonrpc": "2.0", "method": "notifications/cancelled", "params": {"requestId": 5, "reason": "stop"}}))
                .await;
            tokio::time::sleep(Duration::from_millis(100)).await;
            assert!(SETTLED.with(Cell::get), "the call was told and settled");
            session.send(json!({"jsonrpc": "2.0", "id": 6, "method": "ping"})).await;
            assert_eq!(session.next().await.unwrap()["id"], 6, "nothing for the cancelled call came first");
            assert!(session.end().await.is_empty(), "nor after");
        })
        .await;
    }

    #[tokio::test]
    async fn the_end_of_input_stops_each_call_and_waits_for_it_to_settle() {
        local(async {
            let mut session = Session::start().initialized().await;
            session.send(call(json!(9), "wait", json!({}))).await;
            tokio::time::sleep(Duration::from_millis(20)).await;
            let rest = session.end().await;
            assert!(SETTLED.with(Cell::get));
            assert_eq!(rest.len(), 1);
            assert_eq!((rest[0]["id"].clone(), rest[0]["result"]["structuredContent"]["state"].clone()), (json!(9), json!("cancelled")));
            let status = session.state("status.json");
            assert_eq!((status["stale"].clone(), status["connected"].clone()), (json!(true), json!(false)), "the runtime says it stopped");
            assert_eq!(session.state("owner.json")["running"], false);
        })
        .await;
    }

    #[tokio::test]
    async fn told_to_stop_it_settles_its_calls_as_at_the_end_of_input() {
        local(async {
            let mut session = Session::start().initialized().await;
            session.send(call(json!(9), "wait", json!({}))).await;
            tokio::time::sleep(Duration::from_millis(20)).await;
            let owner = session.state("owner.json");
            assert_eq!(
                (owner["running"].clone(), owner["lease"].clone()),
                (json!(true), json!("free")),
                "its heartbeat is there from the start"
            );
            assert!(owner["heartbeat"].as_i64().is_some() && owner["since"].as_i64().is_some());
            session.stop.take().unwrap().send(()).unwrap();
            let rest = session.rest().await;
            assert!(SETTLED.with(Cell::get));
            assert_eq!(rest[0]["result"]["structuredContent"]["state"], "cancelled");
            assert_eq!(session.state("status.json")["stale"], true);
        })
        .await;
    }

    #[tokio::test]
    async fn one_host_call_runs_once_and_its_retry_gets_its_answer() {
        local(async {
            let mut session = Session::start().initialized().await;
            let asked = |id: u64, word: &str| call(json!(id), "scan", json!({"word": word, "_host": {"tool_call_id": "tc-1"}}));
            // A retry while the first runs, and one after it answered: both get its answer, and it ran once.
            session.send(asked(1, "drums")).await;
            session.send(asked(2, "drums")).await;
            let (first, second) = (session.next().await.unwrap(), session.next().await.unwrap());
            assert_eq!(first["result"], second["result"]);
            assert_eq!(first["result"]["structuredContent"], json!({"run": 1}));
            session.send(asked(3, "drums")).await;
            assert_eq!(session.next().await.unwrap()["result"]["structuredContent"], json!({"run": 1}));
            session.send(asked(4, "keys")).await;
            let other = session.next().await.unwrap();
            assert_eq!(
                (other["result"]["isError"].clone(), other["result"]["structuredContent"]["error"]["code"].clone()),
                (json!(true), json!("duplicate"))
            );
            // Fields in another order are the same question.
            session.send(call(json!(5), "scan", json!({"_host": {"tool_call_id": "tc-2"}, "word": "a", "hold": false}))).await;
            session.send(call(json!(6), "scan", json!({"hold": false, "word": "a", "_host": {"tool_call_id": "tc-2"}}))).await;
            let (one, two) = (session.next().await.unwrap(), session.next().await.unwrap());
            assert_eq!(one["result"], two["result"]);
            assert_eq!(RUNS.with(Cell::get), 2);
            // No host call, no dedup; and a call that only reads runs every time.
            session.send(call(json!(7), "scan", json!({}))).await;
            session.send(call(json!(8), "scan", json!({}))).await;
            session.next().await.unwrap();
            session.next().await.unwrap();
            assert_eq!(RUNS.with(Cell::get), 4);
            session.send(call(json!(9), "echo", json!({"word": "a", "_host": {"tool_call_id": "tc-1"}}))).await;
            assert_eq!(session.next().await.unwrap()["result"]["isError"], false, "a read isn't a host call's repeat");
            assert!(session.end().await.is_empty());
        })
        .await;
    }

    #[tokio::test]
    async fn calls_with_a_blank_host_call_id_are_each_their_own() {
        local(async {
            let mut session = Session::start().initialized().await;
            for (id, word, blank) in [(1, "drums", ""), (2, "keys", ""), (3, "keys", "  ")] {
                session.send(call(json!(id), "scan", json!({"word": word, "_host": {"tool_call_id": blank}}))).await;
                let answered = session.next().await.unwrap();
                assert_eq!(answered["result"]["isError"], false, "{answered}");
            }
            assert_eq!(RUNS.with(Cell::get), 3, "neither refused as another's repeat nor answered with another's result");
            assert!(session.end().await.is_empty());
        })
        .await;
    }

    #[tokio::test]
    async fn a_cancelled_host_call_isnt_run_again_by_its_retry() {
        local(async {
            let mut session = Session::start().initialized().await;
            session.send(call(json!(1), "scan", json!({"hold": true, "_host": {"tool_call_id": "tc-9"}}))).await;
            tokio::time::sleep(Duration::from_millis(20)).await;
            session.send(json!({"jsonrpc": "2.0", "method": "notifications/cancelled", "params": {"requestId": 1}})).await;
            tokio::time::sleep(Duration::from_millis(20)).await;
            session.send(call(json!(2), "scan", json!({"hold": true, "_host": {"tool_call_id": "tc-9"}}))).await;
            let retried = session.next().await.unwrap();
            assert_eq!(
                (retried["id"].clone(), retried["result"]["structuredContent"].clone()),
                (json!(2), json!({"run": 1, "state": "cancelled"}))
            );
            assert_eq!(RUNS.with(Cell::get), 1);
            assert!(session.end().await.is_empty());
        })
        .await;
    }

    #[tokio::test]
    async fn a_write_runs_under_the_daws_lease_and_waits_for_none_another_runtime_holds() {
        local(async {
            let mut session = Session::start().initialized().await;
            session.send(call(json!(1), "place", json!({"_host": {"tool_call_id": "tc-3"}}))).await;
            let placed = session.next().await.unwrap();
            assert_eq!(placed["result"]["structuredContent"], json!({"state": "applied"}));
            assert_eq!(RUNS.with(Cell::get), 1);
            assert!(!session.ctx.lease().held(), "freed once it settled");
            // Another runtime on the same DAW holds it: refused at once, with who holds it.
            let other = crate::serve::lease::Lease::new(&session.ctx.paths().kumi_home, "reaper");
            let host = Host { profile_id: Some("work".into()), ..Host::default() };
            let held = other.take(&host).await.unwrap();
            session.send(call(json!(2), "place", json!({"_host": {"tool_call_id": "tc-4"}}))).await;
            let refused = session.next().await.unwrap();
            assert_eq!(refused["result"]["structuredContent"]["error"]["code"], "leased");
            assert_eq!(refused["result"]["structuredContent"]["error"]["owner"]["profile"], "work");
            assert_eq!(RUNS.with(Cell::get), 1, "it never ran");
            drop(held);
            // That refusal was about the moment: the same host call asked again runs.
            session.send(call(json!(3), "place", json!({"_host": {"tool_call_id": "tc-4"}}))).await;
            assert_eq!(session.next().await.unwrap()["result"]["structuredContent"], json!({"state": "applied"}));
            assert_eq!(RUNS.with(Cell::get), 2);
            assert!(session.end().await.is_empty());
        })
        .await;
    }

    #[tokio::test]
    async fn a_write_the_host_cancels_part_way_settles_frees_the_lease_and_isnt_run_again_by_its_retry() {
        local(async {
            let mut session = Session::start().initialized().await;
            session.send(call(json!(1), "hold", json!({"_host": {"tool_call_id": "tc-6"}}))).await;
            while RUNS.with(Cell::get) == 0 {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
            assert!(session.ctx.lease().held(), "held while it writes");
            session.send(json!({"jsonrpc": "2.0", "method": "notifications/cancelled", "params": {"requestId": 1}})).await;
            while session.ctx.lease().held() {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
            assert!(std::fs::read_dir(session.ctx.paths().state_dir.join("scratch/hold")).unwrap().next().is_none(), "its record went");
            // The host's retry of the call it cancelled gets what came of it, and nothing runs again.
            session.send(call(json!(2), "hold", json!({"_host": {"tool_call_id": "tc-6"}}))).await;
            assert_eq!(session.next().await.unwrap()["result"]["structuredContent"]["state"], "unsure");
            assert_eq!(RUNS.with(Cell::get), 1);
            session.end().await;
        })
        .await;
    }

    #[tokio::test]
    async fn a_write_that_fails_part_way_is_unsure_with_an_audit_row_never_failed() {
        local(async {
            let mut session = Session::start().initialized().await;
            session.send(call(json!(1), "smash", json!({"word": "kick", "_host": {"tool_call_id": "tc-5"}}))).await;
            let answer = session.next().await.unwrap()["result"].clone();
            assert_eq!((answer["isError"].clone(), answer["structuredContent"]["state"].clone()), (json!(false), json!("unsure")));
            let history = session.state("history.json");
            let row = &history["items"][0];
            assert_eq!(
                (row["state"].clone(), row["undo_available"].clone(), row["tool_call_id"].clone()),
                (json!("unsure"), json!(false), json!("tc-5"))
            );
            assert_eq!(row["digest"], json!(digest("smash", json!({"word": "kick"}).as_object().unwrap())));
            assert_eq!(answer["structuredContent"]["history_ids"], json!([row["id"].clone()]));
            assert_eq!(session.state("status.json")["unsure"]["op"], "smash", "said until the next look");
            assert!(!session.ctx.lease().held(), "freed");
            assert!(std::fs::read_dir(session.ctx.paths().state_dir.join("scratch/smash")).unwrap().next().is_none(), "its record went");
            session.end().await;
        })
        .await;
    }

    #[tokio::test]
    async fn a_write_a_runtime_died_in_is_unsure_at_the_next_start_and_its_retry_isnt_run_again() {
        local(async {
            let session = Session::start().initialized().await;
            // A place the runtime dispatched and never saw settle (killed, say): its record stays.
            let asked = json!({"word": "snare"});
            let identity = CallIdentity { tool_call_id: Some("tc-7"), digest: &digest("place", asked.as_object().unwrap()) };
            drop(crate::serve::inflight::begin(&session.ctx, "place", Some("Place the snare"), identity).unwrap());
            let state = session.stopped().await;
            let mut session = Session::at(state).initialized().await;
            let status = session.state("status.json");
            assert_eq!((status["unsure"]["op"].clone(), status["unsure"]["cleanup"].clone()), (json!("place"), json!("interrupted")));
            let history = session.state("history.json");
            assert_eq!(
                (history["items"][0]["title"].clone(), history["items"][0]["tool_call_id"].clone()),
                (json!("Place the snare"), json!("tc-7"))
            );
            // The host retries it (its answer never came): what came of it, from HISTORY, not run again.
            session.send(call(json!(1), "place", json!({"word": "snare", "_host": {"tool_call_id": "tc-7"}}))).await;
            let again = session.next().await.unwrap()["result"]["structuredContent"].clone();
            assert_eq!((again["state"].clone(), again["replayed"].clone()), (json!("unsure"), json!(true)));
            session.send(call(json!(2), "place", json!({"word": "kick", "_host": {"tool_call_id": "tc-7"}}))).await;
            assert_eq!(session.next().await.unwrap()["result"]["structuredContent"]["error"]["code"], "duplicate");
            assert_eq!(RUNS.with(Cell::get), 0, "never run again");
            session.send(call(json!(3), "place", json!({"word": "snare", "_host": {"tool_call_id": "tc-8"}}))).await;
            assert_eq!(session.next().await.unwrap()["result"]["structuredContent"]["state"], "applied", "another call runs");
            session.end().await;
        })
        .await;
    }

    #[tokio::test(start_paused = true)]
    async fn a_stop_waits_for_a_calls_own_cleanup_bound_and_no_longer() {
        local(async {
            // A render that needs 90 s to put the DAW back, under a 120 s guard: the stop waits for it, past 20 s.
            let mut session = Session::start().initialized().await;
            session.send(call(json!(1), "restore", json!({"word": "b120,r90"}))).await;
            tokio::time::sleep(Duration::from_millis(10)).await;
            assert_eq!(session.ctx.settle_bound(), Some(Duration::from_secs(120)));
            drop(session.to.take());
            tokio::time::sleep(Duration::from_secs(2)).await;
            assert!(session.state("owner.json")["settling_until_ms"].is_i64(), "the host can tell a slow stop from a hung one");
            tokio::time::timeout(Duration::from_secs(400), &mut session.served).await.unwrap().unwrap().unwrap();
            assert!(SETTLED.with(Cell::get), "its cleanup finished");
            assert_eq!(session.ctx.settle_bound(), None, "the guard went with the call");
            assert!(session.state("owner.json").get("settling_until_ms").is_none());
        })
        .await;
    }

    #[tokio::test(start_paused = true)]
    async fn a_guard_holds_a_stop_only_to_its_own_bound() {
        local(async {
            let mut session = Session::start().initialized().await;
            session.send(call(json!(1), "restore", json!({"word": "b30,r200"}))).await;
            tokio::time::sleep(Duration::from_millis(10)).await;
            drop(session.to.take());
            let started = tokio::time::Instant::now();
            tokio::time::timeout(Duration::from_secs(400), &mut session.served).await.unwrap().unwrap().unwrap();
            let waited = started.elapsed();
            assert!(waited >= Duration::from_secs(30) && waited < Duration::from_secs(40), "{waited:?}");
            assert!(!SETTLED.with(Cell::get), "stopped before its cleanup could finish");
            let long = session.ctx.settling(Duration::from_secs(1000));
            assert_eq!(session.ctx.settle_bound(), Some(Duration::from_secs(360)), "never past 360 s");
            drop(long);
        })
        .await;
    }

    #[tokio::test]
    async fn an_overlong_line_is_refused_whole_and_the_next_is_read() {
        let mut input: &[u8] = b"abcdef\n{\"ok\":1}\r\nlast";
        assert_eq!(read_line(&mut input, 4).await.unwrap(), Some(Line::TooLong));
        assert_eq!(read_line(&mut input, 16).await.unwrap(), Some(Line::Message(b"{\"ok\":1}\r".to_vec())));
        assert_eq!(read_line(&mut input, 16).await.unwrap(), Some(Line::Message(b"last".to_vec())));
        assert_eq!(read_line(&mut input, 16).await.unwrap(), None);
    }

    #[test]
    fn a_result_is_its_value_as_text_and_structure() {
        let ok = tool_result(Ok(json!({"state": "applied"})));
        assert_eq!((ok["isError"].clone(), ok["structuredContent"]["state"].clone()), (json!(false), json!("applied")));
        assert_eq!(tool_result(Ok(json!(3)))["structuredContent"], json!({"value": 3}), "structured content is an object");
        let failed = tool_result(Err(OpError::Invalid { reason: "no".into() }));
        assert_eq!(
            (failed["isError"].clone(), failed["content"][0]["text"].clone()),
            (json!(true), json!(r#"{"error":{"code":"invalid","reason":"no"}}"#))
        );
    }
}
