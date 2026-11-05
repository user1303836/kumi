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
use super::ctx::{Call, Ctx, Effect, Host, OpError, Operation, Out, Outbox, Private, Progress, CALL_PRIVATE, PRIVATE_ARGS, PRIVATE_MOST};
use super::history::CallIdentity;
use super::{registry, say, MCP_PROTOCOL};
use futures::FutureExt;
use kumi_common::abort::{self, Signal};
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet, VecDeque};
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
/// MCP's "Resource not found".
const RESOURCE_NOT_FOUND: i64 = -32002;
/// A resource bigger than one answer carries (a server error of the runtime's own).
const RESOURCE_TOO_LARGE: i64 = -32003;
/// The most of a snapshot a `resources/read` gives. Put in a JSON string, a JSON text at most doubles (each quote or
/// backslash is escaped), so the answer stays a line the host takes (as [`MAX_MESSAGE`] is).
const RESOURCE_MOST: usize = MAX_MESSAGE / 2 - 64 * 1024;

/// C2's resources: each the snapshot (C3) the runtime keeps of it, read as it's on disk, and the snapshot's name, whose
/// landing tells a subscriber the resource changed. `kumi://map` is the map's current revision, by its pointer.
const RESOURCES: [(&str, &str, &str); 4] = [
    ("kumi://status", "status", "Setup and connection: whether Kumi is connected to the DAW, and to which project."),
    ("kumi://focus", "focus", "What the producer is looking at in the DAW, the transport, and what changed since the last look."),
    ("kumi://history", "history", "Kumi's HISTORY for the open project, newest first."),
    ("kumi://map", "map/current", "The sound map's current revision: its sounds and where each sits."),
];
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
    // What an earlier runtime left on disk is published again as this one has it: its HISTORY rows can't be undone
    // here, and nothing is connected yet.
    core::publish_status(&ctx, false).await;
    ctx.history().publish(ctx.snapshots());
    super::connection::publish_focus(&ctx.handle());
    let heartbeat = tokio::task::spawn_local(beat(ctx.clone()));
    let mut landed = ctx.snapshots().landed();
    let server = Rc::new(Server {
        ctx,
        operations,
        outbox,
        initialized: Cell::new(false),
        calls: RefCell::new(HashMap::new()),
        settled: Notify::new(),
        seen: RefCell::new(Seen::default()),
        subscribed: RefCell::new(HashSet::new()),
    });
    // A subscribed resource whose snapshot is on disk anew: the host hears it changed, and reads it again.
    let updates = {
        let server = Rc::downgrade(&server);
        tokio::task::spawn_local(async move {
            while let Some(name) = landed.recv().await {
                // What landed meanwhile goes with it: one word for each resource, however often it was written.
                let mut names = vec![name];
                while let Ok(more) = landed.try_recv() {
                    if !names.contains(&more) {
                        names.push(more);
                    }
                }
                let Some(server) = server.upgrade() else { break };
                for name in names {
                    let Some((uri, _, _)) = RESOURCES.iter().find(|(_, snapshot, _)| *snapshot == name) else { continue };
                    if server.subscribed.borrow().contains(*uri) {
                        server.send(json!({"jsonrpc": "2.0", "method": "notifications/resources/updated", "params": {"uri": uri}}));
                    }
                }
            }
        })
    };
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
    if let Err(waited) = settle(&server.ctx, settling).await {
        let (calls, jobs) = (server.calls.borrow().len(), server.ctx.jobs().running());
        say(&format!(
            "kumi serve: {calls} call(s) and {jobs} job(s) still settling after {} s, the stop's bound; stopping",
            waited.as_secs()
        ));
    }
    server.ctx.set_settling_until(None);
    server.ctx.jobs().interrupt_running(server.ctx.snapshots());
    heartbeat.abort();
    updates.abort();
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
    /// The resources the host subscribed to (`resources/subscribe`), by URI.
    subscribed: RefCell<HashSet<String>>,
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
                        "capabilities": {"tools": {"listChanged": false}, "resources": {"subscribe": true, "listChanged": false}},
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
            "resources/list" => {
                let resources: Vec<Value> = RESOURCES
                    .iter()
                    .map(|(uri, name, description)| json!({"uri": uri, "name": name.split('/').next(), "description": description, "mimeType": "application/json"}))
                    .collect();
                self.send_result(id, json!({"resources": resources}));
            }
            "resources/read" => match self.resource(params) {
                Ok((uri, _)) => {
                    // Read off the runtime's thread: the map's revision can be tens of MiB.
                    let (server, state_dir) = (self.clone(), self.ctx.paths().state_dir.clone());
                    tokio::task::spawn_local(async move {
                        match tokio::task::spawn_blocking(move || read_resource(&state_dir, uri, RESOURCE_MOST)).await {
                            Ok(Ok(Some(text))) => {
                                server.send_result(id, json!({"contents": [{"uri": uri, "mimeType": "application/json", "text": text}]}));
                            }
                            Ok(Ok(None)) => server.send_error(id, RESOURCE_NOT_FOUND, &format!("{uri} has no snapshot yet")),
                            Ok(Err(TooLarge)) => {
                                let reason = format!(
                                    "{uri} is more than one answer carries ({} MiB): search the sound map with sounds instead",
                                    RESOURCE_MOST >> 20
                                );
                                server.send_error(id, RESOURCE_TOO_LARGE, &reason);
                            }
                            Err(_) => server.send_error(id, RESOURCE_NOT_FOUND, &format!("{uri} couldn't be read")),
                        }
                    });
                }
                Err(message) => self.send_error(id, RESOURCE_NOT_FOUND, &message),
            },
            "resources/subscribe" | "resources/unsubscribe" => match self.resource(params) {
                Ok((uri, _)) => {
                    let mut subscribed = self.subscribed.borrow_mut();
                    if method == "resources/subscribe" {
                        subscribed.insert(uri.to_owned());
                    } else {
                        subscribed.remove(uri);
                    }
                    drop(subscribed);
                    self.send_result(id, json!({}));
                }
                Err(message) => self.send_error(id, RESOURCE_NOT_FOUND, &message),
            },
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
                    let run = CALL_PRIVATE.scope(host.private.clone(), (operation.run)(&server.ctx, call));
                    let ran = AssertUnwindSafe(run).catch_unwind().await;
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
                let run = CALL_PRIVATE.scope(host.private.clone(), (operation.run)(&self.ctx, call));
                let ran = AssertUnwindSafe(run).catch_unwind().await;
                let answer = match ran {
                    // An unsure outcome the operation didn't keep in HISTORY itself (CC-2's W1-b, W1-c), whichever
                    // operation answered it: audited, said in status, and the epoch moved on.
                    Ok(Ok(outcome)) if outcome["state"] == "unsure" && outcome.get("history_ids").is_none() => {
                        tool_result(Ok(self.unsure(operation.name, title, identity, outcome).await))
                    }
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
    /// A write, playback or import that answered `unsure` with no HISTORY of its own (a track listen whose cleanup Kumi
    /// can't confirm, say): its audit row, `status.unsure` until the next look, and the epoch moved on, as `change` does
    /// for its own, with the row's id in the answer.
    async fn unsure(&self, op: &str, title: Option<&str>, identity: CallIdentity<'_>, mut outcome: Value) -> Value {
        let reason = outcome["reason"].as_str().map(str::to_string).unwrap_or_else(|| {
            format!("{op} didn't confirm it put the DAW back, so it may have changed: look at the project before another change.")
        });
        let cleanup = outcome["cleanup"].as_str().filter(|cleanup| !cleanup.is_empty()).unwrap_or("unsure").to_string();
        let history = self.ctx.history();
        let project = self.ctx.link().project().map(|project| project.key).unwrap_or_default();
        let audit = history.audit(title.unwrap_or(op), &project, &reason, identity, self.ctx.snapshots());
        history.call_came_to(std::slice::from_ref(&audit.id), "unsure", self.ctx.snapshots());
        self.ctx.link().note_unsure(op, &reason, &cleanup);
        self.ctx.link().advance();
        super::connection::publish_focus(&self.ctx.handle());
        super::core::publish_status(&self.ctx, false).await;
        outcome["history_ids"] = json!([audit.id]);
        outcome
    }

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
        let hosted = args.get("_host").is_some();
        let mut host = match args.remove("_host") {
            None => Host::default(),
            Some(host) => Host::from_value(&host).map_err(|reason| Refusal::Call(OpError::Invalid { reason }))?,
        };
        // The host's private arguments leave the call's own before anything reads them (13-R3). A refusal names the
        // argument, never what it held.
        let mut private = Map::new();
        for name in PRIVATE_ARGS {
            let Some(value) = args.remove(name) else { continue };
            if !hosted {
                let reason = format!("{name} comes from the host alone, with _host");
                return Err(Refusal::Call(OpError::Invalid { reason }));
            }
            if !value.is_object() || value.to_string().len() > PRIVATE_MOST {
                let reason = format!("{name} is an object of at most {PRIVATE_MOST} bytes");
                return Err(Refusal::Call(OpError::Invalid { reason }));
            }
            private.insert(name.to_string(), value);
        }
        host.private = Private::from(private);
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

    /// The resource a `resources/*` request names: its URI and its snapshot's name.
    fn resource(&self, params: &Value) -> Result<(&'static str, &'static str), String> {
        let asked = params.get("uri").and_then(Value::as_str).unwrap_or_default();
        RESOURCES.iter().find(|(uri, _, _)| *uri == asked).map(|(uri, name, _)| (*uri, *name)).ok_or_else(|| {
            format!(
                "no resource \"{}\": kumi://status, kumi://focus, kumi://history and kumi://map are",
                super::change::one_line(asked, 64)
            )
        })
    }

    fn send_result(&self, id: Value, result: Value) {
        self.send(json!({"jsonrpc": "2.0", "id": id, "result": result}));
    }

    fn send_error(&self, id: Value, code: i64, message: &str) {
        self.send(json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}}));
    }
}

/// Waits for `settled` up to [`SETTLE`], or while a call's guard lives up to the largest bound it holds
/// ([`super::ctx::SETTLE_MOST`] at most), counted from the stop's start and read again each second: a guard taken as
/// the stop began counts, and one dropped stops counting. While it waits on a guard, `owner.json` says until when, and
/// says it again whenever that moves. Everything settled, or the bound it gave up at.
async fn settle(ctx: &Ctx, settled: impl Future<Output = ()>) -> Result<(), Duration> {
    let (started, started_ms) = (tokio::time::Instant::now(), kumi_common::time::now_ms());
    tokio::pin!(settled);
    loop {
        let guarded = ctx.settle_bound();
        let bound = guarded.map_or(SETTLE, |bound| bound.max(SETTLE));
        let deadline = started + bound;
        let until = guarded.map(|_| started_ms + bound.as_millis() as i64);
        if until != ctx.settling_until() {
            ctx.set_settling_until(until);
            core::publish_owner(ctx, true);
        }
        let tick = deadline.min(tokio::time::Instant::now() + Duration::from_secs(1));
        tokio::select! {
            () = &mut settled => return Ok(()),
            () = tokio::time::sleep_until(tick) => {}
        }
        if tokio::time::Instant::now() >= deadline {
            return Err(bound);
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

/// A snapshot bigger than a resource read gives.
struct TooLarge;

/// A resource's snapshot as it's on disk, at most `most` bytes: `kumi://map`'s by the revision its pointer names. None
/// before there's one.
fn read_resource(state_dir: &std::path::Path, uri: &str, most: usize) -> Result<Option<String>, TooLarge> {
    use std::io::Read;
    let name = match uri {
        "kumi://map" => {
            let Ok(revision) = std::fs::read_to_string(state_dir.join("map/current")) else { return Ok(None) };
            let revision = revision.trim();
            if revision.is_empty() || !revision.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_') {
                return Ok(None);
            }
            format!("map/{revision}")
        }
        _ => match RESOURCES.iter().find(|(known, _, _)| *known == uri) {
            Some((_, name, _)) => (*name).to_owned(),
            None => return Ok(None),
        },
    };
    let Ok(file) = std::fs::File::open(state_dir.join(format!("{name}.json"))) else { return Ok(None) };
    // One byte past the most tells a bigger one, whatever the file grew to since it was opened.
    let mut text = String::new();
    if file.take(most as u64 + 1).read_to_string(&mut text).is_err() {
        return Ok(None);
    }
    if text.len() > most {
        return Err(TooLarge);
    }
    Ok(Some(text))
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
            let bound = u16::try_from(seconds("b").unwrap_or(0)).unwrap_or(u16::MAX);
            let _guard = ctx.settling(bound)?;
            call.signal.cancelled().await;
            // The DAW is put back on its own time, not on the call's signal, which the stop has cancelled.
            tokio::time::sleep(Duration::from_secs(seconds("r").unwrap_or(0))).await;
            SETTLED.with(|settled| settled.set(true));
            Ok(json!({"state": "restored"}))
        })
    }

    /// [`RESTORE`] as a track listen is: it plays the DAW, so under the write lease and with its in-flight record.
    static PLAY: Operation =
        Operation { name: "play", description: "Plays, then restores.", schema: object, effect: playback, run: restore };

    /// Plays and can't confirm it put the DAW back; with `hold`, it says so having kept HISTORY itself.
    static SPILL: Operation =
        Operation { name: "spill", description: "Plays, unsure after.", schema: object, effect: playback, run: spill };
    fn spill(_ctx: &Ctx, call: Call) -> OpFuture<'_> {
        Box::pin(async move {
            RUNS.with(|runs| runs.set(runs.get() + 1));
            if call.args.get("word").and_then(Value::as_str) == Some("fine") {
                return Ok(json!({"state": "applied", "cleanup": "restored"}));
            }
            let mut outcome = json!({"state": "unsure", "reason": "The render ran past its window, and Live's routing may not be back.", "cleanup": "unsure"});
            if call.args.get("hold").and_then(Value::as_bool) == Some(true) {
                outcome["history_ids"] = json!(["h-own-1"]);
            }
            Ok(outcome)
        })
    }
    fn playback(_: &JsonObject) -> Effect {
        Effect::Playback
    }

    /// Spends the producer's credits with the key the host gives it privately: says whether it had one, never the key.
    /// With `hold`, it then writes until cancelled; with the word "smash", it fails unexpectedly.
    static PAY: Operation = Operation { name: "pay", description: "Pays.", schema: object, effect: writes, run: pay };
    fn pay(_ctx: &Ctx, call: Call) -> OpFuture<'_> {
        Box::pin(async move {
            RUNS.with(|runs| runs.set(runs.get() + 1));
            let keyed = call.host.private.get("_mirelo").and_then(|given| given["api_key"].as_str()).is_some();
            if call.args.get("word").and_then(Value::as_str) == Some("smash") {
                panic!("a payment went wrong for {:?}", call.host);
            }
            if call.args.get("hold").and_then(Value::as_bool) == Some(true) {
                call.signal.cancelled().await;
            }
            Ok(json!({"state": "applied", "keyed": keyed, "bounded": call.host.private.get("_listen").is_some()}))
        })
    }

    static OPERATIONS: &[&Operation] = &[&WAIT, &ECHO, &BOOM, &SCAN, &PLACE, &SMASH, &HOLD, &RESTORE, &PLAY, &PAY, &SPILL];

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
                hermes_home: None,
                hermes_root: None,
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

    /// C2's resources: listed, read as their snapshots are on disk, and a subscriber told each time one lands anew
    /// (the map when its pointer flips), until it unsubscribes.
    #[tokio::test]
    async fn a_subscriber_hears_a_resource_changed_once_its_snapshot_is_on_disk() {
        local(async {
            let mut session = Session::start().initialized().await;
            session.send(json!({"jsonrpc": "2.0", "id": 1, "method": "resources/list"})).await;
            let listed = session.next().await.unwrap();
            let uris: Vec<&str> = listed["result"]["resources"].as_array().unwrap().iter().filter_map(|r| r["uri"].as_str()).collect();
            assert_eq!(uris, ["kumi://status", "kumi://focus", "kumi://history", "kumi://map"]);
            // status is on disk from the start: read as it is there.
            session.ctx.snapshots().flush();
            session.send(json!({"jsonrpc": "2.0", "id": 2, "method": "resources/read", "params": {"uri": "kumi://status"}})).await;
            let read = session.next().await.unwrap();
            let first: Value = serde_json::from_str(read["result"]["contents"][0]["text"].as_str().unwrap()).unwrap();
            assert_eq!(first["seq"], session.state("status.json")["seq"], "{read}");
            // A map not made yet, or a URI that isn't one: not found.
            for (id, uri) in [(3, "kumi://map"), (4, "kumi://elsewhere")] {
                session.send(json!({"jsonrpc": "2.0", "id": id, "method": "resources/read", "params": {"uri": uri}})).await;
                assert_eq!(session.next().await.unwrap()["error"]["code"], RESOURCE_NOT_FOUND, "{uri}");
            }
            for (id, uri) in [(5, "kumi://status"), (6, "kumi://map")] {
                session.send(json!({"jsonrpc": "2.0", "id": id, "method": "resources/subscribe", "params": {"uri": uri}})).await;
                assert_eq!(session.next().await.unwrap()["result"], json!({}));
            }
            // A new status: its subscriber hears of it once it's on disk, and reads the new one.
            session.ctx.snapshots().write("status", &json!({"connected": false})).unwrap();
            let updated = session.next().await.unwrap();
            assert_eq!(updated, json!({"jsonrpc": "2.0", "method": "notifications/resources/updated", "params": {"uri": "kumi://status"}}));
            session.send(json!({"jsonrpc": "2.0", "id": 7, "method": "resources/read", "params": {"uri": "kumi://status"}})).await;
            let read = session.next().await.unwrap();
            let now: Value = serde_json::from_str(read["result"]["contents"][0]["text"].as_str().unwrap()).unwrap();
            assert!(now["seq"].as_u64() > first["seq"].as_u64(), "{now}");
            // A map's revision is written, then its pointer flips: the subscriber hears once, and reads that revision.
            session.ctx.snapshots().write_durably("map/r7", json!({"revision": "r7"})).await.unwrap();
            crate::serve::snapshots::replace(&session.ctx.paths().state_dir.join("map/current"), b"r7\n").unwrap();
            session.ctx.snapshots().touched("map/current");
            let updated = session.next().await.unwrap();
            assert_eq!(updated["params"]["uri"], "kumi://map", "only when the pointer flips: {updated}");
            session.send(json!({"jsonrpc": "2.0", "id": 8, "method": "resources/read", "params": {"uri": "kumi://map"}})).await;
            let read = session.next().await.unwrap();
            assert_eq!(serde_json::from_str::<Value>(read["result"]["contents"][0]["text"].as_str().unwrap()).unwrap()["revision"], "r7");
            // Unsubscribed, or never subscribed (history): nothing more is said.
            session.send(json!({"jsonrpc": "2.0", "id": 9, "method": "resources/unsubscribe", "params": {"uri": "kumi://status"}})).await;
            assert_eq!(session.next().await.unwrap()["result"], json!({}));
            session.ctx.snapshots().write("status", &json!({"connected": false})).unwrap();
            session.ctx.snapshots().write("history", &json!({"items": []})).unwrap();
            session.ctx.snapshots().flush();
            session.send(json!({"jsonrpc": "2.0", "id": 10, "method": "ping"})).await;
            assert_eq!(session.next().await.unwrap()["id"], 10, "no notification came before the ping's answer");
            session.end().await;
        })
        .await;
    }

    /// After a restart, focus and HISTORY are this runtime's from the start, not as the last one left them: nothing is
    /// connected, and the earlier runtime's rows can't be undone here.
    #[tokio::test]
    async fn a_new_runtime_publishes_focus_and_history_as_it_has_them() {
        local(async {
            let first = Session::start().initialized().await;
            let folders = first.stopped().await;
            let state = folders.path().join("state");
            let row = json!({"id": "h-old-c1", "label": "c1", "project_key": "p-1", "runtime_generation": "old", "title": "Drums",
                "family": "mixer", "at": 1, "state": "applied", "undo_available": true});
            std::fs::write(state.join("history.json"), json!({"schema": 1, "seq": 9, "items": [row]}).to_string()).unwrap();
            std::fs::write(state.join("focus.json"), json!({"schema": 1, "seq": 9, "connected": true}).to_string()).unwrap();
            let mut session = Session::at(folders).initialized().await;
            session.ctx.snapshots().flush();
            let read = |uri: &str| json!({"jsonrpc": "2.0", "id": uri, "method": "resources/read", "params": {"uri": uri}});
            let mut text =
                |answer: Value| -> Value { serde_json::from_str(answer["result"]["contents"][0]["text"].as_str().unwrap()).unwrap() };
            session.send(read("kumi://focus")).await;
            assert_eq!(text(session.next().await.unwrap())["connected"], false);
            session.send(read("kumi://history")).await;
            let history = text(session.next().await.unwrap());
            session.send(read("kumi://status")).await;
            let status = text(session.next().await.unwrap());
            assert_eq!(
                (history["items"][0]["id"].clone(), history["items"][0]["undo_available"].clone()),
                (json!("h-old-c1"), json!(false))
            );
            assert_eq!(history["generation"], status["runtime"]["generation"]);
            assert_ne!(history["generation"], history["items"][0]["runtime_generation"], "the row is an earlier runtime's");
            session.end().await;
        })
        .await;
    }

    #[test]
    fn a_resource_read_gives_at_most_what_one_answer_carries() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("map")).unwrap();
        std::fs::write(dir.path().join("map/current"), "r1\n").unwrap();
        std::fs::write(dir.path().join("map/r1.json"), "[0123456789]").unwrap();
        assert!(matches!(read_resource(dir.path(), "kumi://map", 12), Ok(Some(text)) if text == "[0123456789]"));
        assert!(matches!(read_resource(dir.path(), "kumi://map", 11), Err(TooLarge)), "one byte over");
        assert!(matches!(read_resource(dir.path(), "kumi://status", 12), Ok(None)), "none yet");
    }

    #[tokio::test]
    async fn a_session_lists_its_tools_and_calls_one_with_progress() {
        local(async {
            let mut session = Session::start();
            session.send(json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {"protocolVersion": "2025-06-18"}})).await;
            let initialized = session.next().await.unwrap();
            assert_eq!(initialized["result"]["protocolVersion"], MCP_PROTOCOL, "the one version it speaks");
            assert_eq!(initialized["result"]["serverInfo"], json!({"name": "kumi-serve", "version": env!("CARGO_PKG_VERSION")}));
            assert_eq!(
                initialized["result"]["capabilities"],
                json!({"tools": {"listChanged": false}, "resources": {"subscribe": true, "listChanged": false}})
            );
            session.send(json!({"jsonrpc": "2.0", "method": "notifications/initialized"})).await;
            session.send(json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list"})).await;
            let listed = session.next().await.unwrap();
            let names: Vec<&str> = listed["result"]["tools"].as_array().unwrap().iter().filter_map(|tool| tool["name"].as_str()).collect();
            assert_eq!(names, ["wait", "echo", "boom", "scan", "place", "smash", "hold", "restore", "play", "pay", "spill"]);
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
            session.send(json!({"jsonrpc": "2.0", "id": 2, "method": "prompts/list"})).await;
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

    #[tokio::test]
    async fn a_key_the_host_gives_privately_reaches_the_call_and_nothing_else() {
        const SENTINEL: &str = "sk-sentinel-5f2c9e";
        let mirelo = |key: &str| json!({"api_key": key, "acknowledged": true, "enabled": true, "reservation_id": "r-1"});
        local(async {
            // A runtime of its own (its own local task set), killed with a payment still running.
            let runtime = tokio::task::LocalSet::new();
            let (state, mut said) = runtime
                .run_until(async {
                    let mut session = Session::start().initialized().await;
                    let mut said = vec![];
                    // The call has it; its answer, its digest and its deduplication don't.
                    let paying = json!({"word": "kick", "_host": {"tool_call_id": "tc-1"}, "_mirelo": mirelo(SENTINEL)});
                    session.send(call(json!(1), "pay", paying)).await;
                    let paid = session.next().await.unwrap();
                    assert_eq!(paid["result"]["structuredContent"]["keyed"], true, "{paid}");
                    said.push(paid);
                    // The same call with another key is the same call: answered as before, not run again.
                    let rekeyed = json!({"word": "kick", "_host": {"tool_call_id": "tc-1"}, "_mirelo": mirelo("sk-another")});
                    session.send(call(json!(2), "pay", rekeyed)).await;
                    let again = session.next().await.unwrap();
                    assert_eq!(again["result"]["structuredContent"]["keyed"], true, "{again}");
                    assert_eq!(RUNS.with(Cell::get), 1);
                    said.push(again);
                    // Not an object, too big, or without `_host`: refused by its name alone.
                    for given in [
                        json!({"word": "kick", "_host": {}, "_mirelo": SENTINEL}),
                        json!({"word": "kick", "_host": {}, "_mirelo": {"api_key": SENTINEL.repeat(400)}}),
                        json!({"word": "kick", "_mirelo": mirelo(SENTINEL)}),
                    ] {
                        session.send(call(json!(3), "pay", given)).await;
                        let refused = session.next().await.unwrap();
                        assert_eq!(refused["result"]["structuredContent"]["error"]["code"], "invalid", "{refused}");
                        said.push(refused);
                    }
                    // A payment that fails unexpectedly: its audit row and its answer.
                    let smashing = json!({"word": "smash", "_host": {"tool_call_id": "tc-2"}, "_mirelo": mirelo(SENTINEL)});
                    session.send(call(json!(4), "pay", smashing)).await;
                    said.push(session.next().await.unwrap());
                    // One the runtime is killed in the middle of: on disk once it runs.
                    let holding = json!({"word": "snare", "hold": true, "_host": {"tool_call_id": "tc-3"}, "_mirelo": mirelo(SENTINEL)});
                    session.send(call(json!(5), "pay", holding)).await;
                    let state_dir = session.ctx.paths().state_dir.clone();
                    for _ in 0..1000 {
                        if !crate::serve::inflight::left_over(&state_dir).is_empty() {
                            break;
                        }
                        tokio::time::sleep(Duration::from_millis(10)).await;
                    }
                    // What it wrote before, on disk as a killed process's would be: its writer thread outlives this
                    // set, and a write of its landing late would hide the next start's.
                    session.ctx.snapshots().flush();
                    (session._state, said)
                })
                .await;
            // The process ends with the payment running: its record stays, unsettled, without the key.
            drop(runtime);
            let left = crate::serve::inflight::left_over(&state.path().join("state"));
            assert_eq!(left.len(), 1, "the killed payment's record");
            assert_eq!(left[0].0["tool_call_id"], "tc-3");
            let record = std::fs::read(left[0].1.join("inflight.json")).unwrap();
            assert!(!String::from_utf8_lossy(&record).contains(SENTINEL), "its record keeps the key");
            // The next start turns it into a HISTORY row, still without the key.
            let mut next = Session::at(state).initialized().await;
            assert!(crate::serve::inflight::left_over(&next.ctx.paths().state_dir).is_empty(), "recovered");
            let history = next.state("history.json");
            let rows = history["items"].as_array().unwrap();
            assert!(rows.iter().any(|row| row["tool_call_id"] == "tc-3"), "{history}");
            assert!(!history.to_string().contains(SENTINEL));
            said.extend(next.end().await);
            let state = next._state;
            for message in &said {
                assert!(!message.to_string().contains(SENTINEL), "said: {message}");
            }
            let mut folders = vec![state.path().to_path_buf()];
            let mut files = 0;
            while let Some(folder) = folders.pop() {
                for entry in std::fs::read_dir(&folder).unwrap().filter_map(Result::ok) {
                    let path = entry.path();
                    if path.is_dir() {
                        folders.push(path);
                    } else {
                        files += 1;
                        let bytes = std::fs::read(&path).unwrap();
                        assert!(!String::from_utf8_lossy(&bytes).contains(SENTINEL), "{} keeps the key", path.display());
                    }
                }
            }
            assert!(files > 3, "the state folder was looked through ({files} files)");
            let host = Host { private: Private::from(mirelo(SENTINEL).as_object().unwrap().clone()), ..Host::default() };
            assert!(!format!("{host:?}").contains(SENTINEL));
        })
        .await;
    }

    #[tokio::test]
    async fn a_private_argument_without_host_is_refused_by_name_and_never_runs() {
        local(async {
            let mut session = Session::start().initialized().await;
            let asked = json!({"word": "kick", "_mirelo": {"api_key": "sk-sentinel-77", "reservation_id": "r-1"}});
            session.send(call(json!(1), "pay", asked)).await;
            let refused = session.next().await.unwrap();
            let error = &refused["result"]["structuredContent"]["error"];
            assert_eq!(error["code"], "invalid", "{refused}");
            let reason = error["reason"].as_str().unwrap();
            assert!(reason.contains("_mirelo") && reason.contains("_host") && !reason.contains("sk-sentinel"), "{reason}");
            assert_eq!(RUNS.with(Cell::get), 0, "never run");
            // With `_host`, the same call runs.
            let hosted = json!({"word": "kick", "_host": {}, "_mirelo": {"api_key": "sk-sentinel-77"}});
            session.send(call(json!(2), "pay", hosted)).await;
            assert_eq!(session.next().await.unwrap()["result"]["structuredContent"]["keyed"], true);
            // A2's `_listen` (a track listen's approved bounds) is the host's the same way: refused without `_host`;
            // with it, given to the call and outside what the call asked, so a retry with other bounds is the same call.
            let bounds = |deadline: u64| json!({"estimated_seconds": 8, "prepare_seconds": 0, "deadline_seconds": deadline});
            session.send(call(json!(3), "pay", json!({"word": "snare", "_listen": bounds(120)}))).await;
            let refused = session.next().await.unwrap();
            assert_eq!(refused["result"]["structuredContent"]["error"]["code"], "invalid", "{refused}");
            let timed = json!({"word": "snare", "_host": {"tool_call_id": "tc-l"}, "_listen": bounds(120)});
            session.send(call(json!(4), "pay", timed)).await;
            let first = session.next().await.unwrap()["result"]["structuredContent"].clone();
            assert_eq!((first["state"].clone(), first["bounded"].clone()), (json!("applied"), json!(true)), "{first}");
            let again = json!({"word": "snare", "_host": {"tool_call_id": "tc-l"}, "_listen": bounds(300)});
            session.send(call(json!(5), "pay", again)).await;
            assert_eq!(session.next().await.unwrap()["result"]["structuredContent"], first, "the same call, answered again");
            assert_eq!(RUNS.with(Cell::get), 2, "the snare ran once");
            session.end().await;
        })
        .await;
    }

    #[tokio::test]
    async fn an_unsure_playback_is_audited_said_and_moves_the_epoch_on_once() {
        local(async {
            let mut session = Session::start().initialized().await;
            let before = session.ctx.link().epoch();
            session.send(call(json!(1), "spill", json!({"_host": {"tool_call_id": "tc-1"}}))).await;
            let answered = session.next().await.unwrap()["result"]["structuredContent"].clone();
            let ids = answered["history_ids"].as_array().cloned().unwrap_or_default();
            assert_eq!((answered["state"].clone(), ids.len()), (json!("unsure"), 1), "{answered}");
            let history = session.state("history.json");
            let row = &history["items"][0];
            assert_eq!(
                (row["id"].clone(), row["state"].clone(), row["undo_available"].clone()),
                (ids[0].clone(), json!("unsure"), json!(false))
            );
            assert_eq!(row["tool_call_id"], "tc-1");
            let status = session.state("status.json");
            assert_eq!((status["unsure"]["op"].clone(), status["unsure"]["cleanup"].clone()), (json!("spill"), json!("unsure")));
            assert_eq!(session.ctx.link().epoch(), before + 1, "refs from before it no longer hold");
            // Its retry is the same call: answered as before, with no second row and no second move.
            session.send(call(json!(2), "spill", json!({"_host": {"tool_call_id": "tc-1"}}))).await;
            let again = session.next().await.unwrap()["result"]["structuredContent"].clone();
            assert_eq!(again, answered, "the stored answer");
            // One that kept its own HISTORY (as change and undo do) isn't audited again, nor its epoch moved twice.
            session.send(call(json!(3), "spill", json!({"hold": true, "_host": {"tool_call_id": "tc-2"}}))).await;
            let own = session.next().await.unwrap()["result"]["structuredContent"].clone();
            assert_eq!(own["history_ids"], json!(["h-own-1"]));
            // One that came to anything but unsure is the operation's own to record: no row, no move.
            session.send(call(json!(4), "spill", json!({"word": "fine", "_host": {"tool_call_id": "tc-3"}}))).await;
            assert_eq!(session.next().await.unwrap()["result"]["structuredContent"]["state"], "applied");
            assert_eq!(session.state("history.json")["items"].as_array().map(Vec::len), Some(1));
            assert_eq!(session.ctx.link().epoch(), before + 1);
            assert_eq!(RUNS.with(Cell::get), 3, "the retry wasn't run");
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
            // Only an approved deadline holds a stop: 1 to 360 s.
            for refused in [0, 361, u16::MAX] {
                assert!(matches!(session.ctx.settling(refused), Err(OpError::Failed { .. })), "{refused}");
            }
            let longest = session.ctx.settling(360).unwrap();
            assert_eq!(session.ctx.settle_bound(), Some(Duration::from_secs(360)));
            drop(longest);
            assert_eq!(session.ctx.settle_bound(), Some(Duration::from_secs(30)), "the cut-off call's own guard is still there");
        })
        .await;
    }

    #[tokio::test(start_paused = true)]
    async fn a_guard_taken_mid_stop_moves_its_deadline_and_owner_json_says_so() {
        local(async {
            let mut session = Session::start().initialized().await;
            session.send(call(json!(1), "restore", json!({"word": "b30,r200"}))).await;
            tokio::time::sleep(Duration::from_millis(10)).await;
            drop(session.to.take());
            let started = tokio::time::Instant::now();
            tokio::time::sleep(Duration::from_secs(2)).await;
            let first = session.state("owner.json")["settling_until_ms"].as_i64().unwrap();
            // A second call's guard, larger, taken as the stop goes on: the deadline moves, and the host is told.
            let larger = session.ctx.settling(120).unwrap();
            tokio::time::sleep(Duration::from_secs(2)).await;
            let moved = session.state("owner.json")["settling_until_ms"].as_i64().unwrap();
            assert_eq!(moved - first, 90_000, "counted from the stop's start, as the first was");
            // It goes at 50 s: the stop's bound is the first guard's 30 s again, long past, so it ends.
            tokio::time::sleep_until(started + Duration::from_secs(50)).await;
            drop(larger);
            tokio::time::timeout(Duration::from_secs(400), &mut session.served).await.unwrap().unwrap().unwrap();
            let waited = started.elapsed();
            assert!(waited >= Duration::from_secs(50) && waited < Duration::from_secs(52), "{waited:?}");
            assert!(!SETTLED.with(Cell::get), "stopped before its cleanup could finish");
            assert!(session.state("owner.json").get("settling_until_ms").is_none());
        })
        .await;
    }

    #[tokio::test(start_paused = true)]
    async fn a_playback_cut_off_at_its_bound_holds_the_lease_to_the_end_and_is_unsure_at_the_next_start() {
        local(async {
            let host = Host { profile_id: Some("work".into()), ..Host::default() };
            let ask = json!({"word": "b30,r200", "_host": {"tool_call_id": "tc-9"}});
            // A runtime of its own (its own local task set): when it's done, its calls go as a process's would.
            let runtime = tokio::task::LocalSet::new();
            let (state, home) = runtime
                .run_until(async {
                    let mut session = Session::start().initialized().await;
                    session.send(call(json!(1), "play", ask.clone())).await;
                    tokio::time::sleep(Duration::from_millis(10)).await;
                    let home = session.ctx.paths().kumi_home.clone();
                    let other = crate::serve::lease::Lease::new(&home, "reaper");
                    assert!(matches!(other.take(&host).await, Err(OpError::Refused { code: "leased", .. })));
                    // The host goes from both pipes at once.
                    drop(session.to.take());
                    drop(std::mem::replace(&mut session.from, BufReader::new(tokio::io::duplex(1).0)));
                    let started = tokio::time::Instant::now();
                    tokio::time::sleep(Duration::from_secs(5)).await;
                    assert!(session.ctx.lease().held(), "held while it settles");
                    assert!(matches!(other.take(&host).await, Err(OpError::Refused { code: "leased", .. })));
                    tokio::time::timeout(Duration::from_secs(400), &mut session.served).await.unwrap().unwrap().unwrap();
                    let waited = started.elapsed();
                    assert!(waited >= Duration::from_secs(30) && waited < Duration::from_secs(40), "{waited:?}");
                    assert!(!SETTLED.with(Cell::get), "cut off at its bound, still putting the DAW back");
                    let left = crate::serve::inflight::left_over(&session.ctx.paths().state_dir);
                    assert_eq!(left.len(), 1, "word of it is on disk");
                    (session._state, home)
                })
                .await;
            // The process ends: the call still putting the DAW back goes with it, and so does its lease. Its record stays.
            drop(runtime);
            assert_eq!(crate::serve::inflight::left_over(&state.path().join("state")).len(), 1);
            let other = crate::serve::lease::Lease::new(&home, "reaper");
            drop(other.take(&host).await.expect("free once the runtime's gone"));
            // The next start finds it unsettled: its retry is answered unsure from HISTORY, never run again.
            let mut next = Session::at(state).initialized().await;
            next.send(call(json!(2), "play", ask)).await;
            let again = next.next().await.unwrap()["result"]["structuredContent"].clone();
            assert_eq!((again["state"].clone(), again["replayed"].clone()), (json!("unsure"), json!(true)), "{again}");
            assert!(crate::serve::inflight::left_over(&next.ctx.paths().state_dir).is_empty(), "recovered once");
            next.end().await;
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
