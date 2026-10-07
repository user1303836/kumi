//! The TypeScript spoke through `@modelcontextprotocol/sdk`'s `Client` over its `StdioClientTransport`;
//! here the same wire behaviour is written out: the child and its environment, newline-delimited
//! JSON-RPC, the `initialize` handshake, one deadline and a `notifications/cancelled` per request,
//! the server's `ping`, and the close sequence (stdin end → 2 s → SIGTERM → 2 s → SIGKILL).

use std::cell::{Cell, RefCell};
use std::collections::{BTreeSet, HashMap};
use std::future::Future;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::rc::{Rc, Weak};
use std::sync::LazyLock;
use std::time::Duration;

use async_trait::async_trait;
use futures::future::{LocalBoxFuture, Shared};
use futures::FutureExt;
use kumi_common::abort::{Signal, SignalExt};
use kumi_common::js::json::stringify;
use kumi_common::js::string::head;
use regex::Regex;
use serde::Serialize;
use serde_json::{json, Value};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::{Child, ChildStderr, ChildStdin, ChildStdout, Command};
use tokio::sync::{mpsc, oneshot, watch};
use tokio::task::spawn_local;
use tokio::time::sleep;

use super::types::{
    CallToolResult, ContentBlock, ErrorCode, Implementation, InitializeResult, JsonRpcMessage, JsonRpcNotification, JsonRpcRequest,
    ListToolsResult, McpError, Payload, RequestId, LATEST_PROTOCOL_VERSION, SUPPORTED_PROTOCOL_VERSIONS,
};
use crate::core::contracts::JsonObject;
use crate::core::errors::RuntimeError;
use crate::version::KUMI_VERSION;

/// The most one message from the bridge may hold: a big Set's reads come in pages well under it.
pub const MAX_BRIDGE_MESSAGE_BYTES: usize = 64 * 1024 * 1024;

/// The SDK's `deserializeMessage`: `JSONRPCMessageSchema.parse(JSON.parse(line))`.
pub fn deserialize_message(line: &str) -> Result<JsonRpcMessage, RuntimeError> {
    serde_json::from_str(line).map_err(|error| RuntimeError::plain(error.to_string()))
}

/// The bridge's messages, read in linear time: the SDK's own buffer joins everything so far with each
/// chunk the pipe delivers, which for a message of megabytes is quadratic. Over the limit, the message
/// is refused (and the link closes, as the SDK's does).
pub struct LinearReadBuffer {
    pieces: Vec<Vec<u8>>,
    size: usize,
    /// How many pieces were searched for a line end and had none: the SDK asks after every chunk.
    scanned: usize,
    limit: usize,
}

impl LinearReadBuffer {
    pub fn new(limit: usize) -> Self {
        Self { pieces: Vec::new(), size: 0, scanned: 0, limit }
    }

    pub fn append(&mut self, chunk: &[u8]) -> Result<(), RuntimeError> {
        if self.size + chunk.len() > self.limit {
            self.clear();
            return Err(RuntimeError::plain(format!("ReadBuffer exceeded maximum size of {} bytes", self.limit)));
        }
        self.pieces.push(chunk.to_vec());
        self.size += chunk.len();
        Ok(())
    }

    /// The next message, or None until a whole line is in; a line that isn't a message is an error (and consumed).
    pub fn read_message(&mut self) -> Result<Option<JsonRpcMessage>, RuntimeError> {
        for index in self.scanned..self.pieces.len() {
            let Some(at) = memchr::memchr(b'\n', &self.pieces[index]) else { continue };
            let mut head: Vec<u8> = Vec::with_capacity(self.pieces[..index].iter().map(Vec::len).sum::<usize>() + at);
            for piece in &self.pieces[..index] {
                head.extend_from_slice(piece);
            }
            head.extend_from_slice(&self.pieces[index][..at]);
            let rest = self.pieces[index][at + 1..].to_vec();
            let tail = self.pieces.split_off(index + 1);
            self.pieces = if rest.is_empty() { tail } else { std::iter::once(rest).chain(tail).collect() };
            self.size -= head.len() + 1;
            self.scanned = 0;
            let text = String::from_utf8_lossy(&head);
            let line = text.strip_suffix('\r').unwrap_or(&text);
            return deserialize_message(line).map(Some);
        }
        self.scanned = self.pieces.len();
        Ok(None)
    }

    pub fn clear(&mut self) {
        self.pieces = Vec::new();
        self.size = 0;
        self.scanned = 0;
    }
}

/// The bridge Kumi starts: the `ableton-mcp-server` binary beside Kumi's own.
pub fn bridge_entry() -> PathBuf {
    let name = if cfg!(windows) { "ableton-mcp-server.exe" } else { "ableton-mcp-server" };
    match std::env::current_exe().ok().and_then(|exe| exe.parent().map(|folder| folder.join(name))) {
        Some(path) => path,
        None => PathBuf::from(name),
    }
}

/// `dirname(path)`: "." for a bare name, as Node answers.
fn dirname(path: &Path) -> PathBuf {
    match path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent.to_path_buf(),
        _ => PathBuf::from("."),
    }
}

/// How much output the bridge wrote to stderr (at most 64 KiB counted) and whether there was more.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct StderrStatus {
    pub bytes: usize,
    pub truncated: bool,
}

/// A connection to the bridge: Kumi's tool catalog and calls go through it.
#[async_trait(?Send)]
pub trait McpEndpoint {
    fn pid(&self) -> Option<u32>;
    fn server_info(&self) -> Option<Implementation>;
    async fn list(&self, cursor: Option<&str>, signal: Signal) -> Result<ListToolsResult, RuntimeError>;
    async fn call(&self, name: &str, args: JsonObject, signal: Signal) -> Result<CallToolResult, RuntimeError>;
    /// The returned closure removes the listener.
    fn on_catalog_changed(&self, listener: Rc<dyn Fn()>) -> Box<dyn Fn()>;
    fn on_disconnect(&self, listener: Rc<dyn Fn()>) -> Box<dyn Fn()>;
    /// Whether `on_live_event` is offered (`onLiveEvent?` was optional).
    fn has_on_live_event(&self) -> bool {
        false
    }
    /// What happens in Live as it happens (the bridge's notifications/live_event, once subscribed; pointed events always).
    fn on_live_event(&self, listener: Rc<dyn Fn(JsonObject)>) -> Box<dyn Fn()> {
        let _ = listener;
        Box::new(|| {})
    }
    fn stderr_status(&self) -> StderrStatus;
    async fn close(&self) -> Result<(), RuntimeError>;
}

#[derive(Default)]
pub struct Options {
    pub signal: Signal,
    pub bridge_config: Option<PathBuf>,
    /// Host-only injection for protocol tests. The CLI never accepts executable/entry overrides.
    pub entry: Option<PathBuf>,
    pub args: Vec<String>,
    /// Per request.
    pub timeout_ms: Option<u64>,
    /// Starting the child and the MCP handshake; defaults to the request timeout.
    pub connect_timeout_ms: Option<u64>,
    pub on_dispatch: Option<Rc<dyn Fn(&str)>>,
    /// Ask the bridge to expose exactly these tools (the ones Kumi's reads, changes and actions use);
    /// it refuses every other one, so audio capture, files, projects and realtime control stay off
    /// even if Kumi's own checks were bypassed. Without it the bridge runs read-only.
    pub allow_tools: Vec<String>,
    /// Where the bridge runs; the bridge binary's folder by default. TS: the repository root, where it finds the protocol registry.
    pub cwd: Option<PathBuf>,
}

/// How long the transport waits, after the bridge exits, for its output to end before it counts as closed anyway.
const EXIT_GRACE: Duration = Duration::from_millis(1000);

/// `Number.isSafeInteger(ms) && ms >= 1`.
fn valid_timeout(ms: u64) -> bool {
    (1..=9_007_199_254_740_991).contains(&ms)
}

/// The SDK's default inheritance on Windows (undefined values and ones starting with "()" skipped); Kumi's
/// own variables override it. On POSIX the SDK's defaults (HOME LOGNAME PATH SHELL TERM USER) are all overridden.
#[cfg(windows)]
const DEFAULT_INHERITED_ENV_VARS: [&str; 12] = [
    "APPDATA",
    "HOMEDRIVE",
    "HOMEPATH",
    "LOCALAPPDATA",
    "PATH",
    "PROCESSOR_ARCHITECTURE",
    "SYSTEMDRIVE",
    "SYSTEMROOT",
    "TEMP",
    "USERNAME",
    "USERPROFILE",
    "PROGRAMFILES",
];

/// The child's whole environment, in the order it is built (a later key replaces an earlier one in place).
fn child_environment(entry: &Path, allow_tools: &[String]) -> Vec<(String, String)> {
    let mut environment: Vec<(String, String)> = Vec::new();
    let mut set = |key: &str, value: String| match environment.iter_mut().find(|(name, _)| name == key) {
        Some(entry) => entry.1 = value,
        None => environment.push((key.to_string(), value)),
    };
    #[cfg(windows)]
    for key in DEFAULT_INHERITED_ENV_VARS {
        if let Ok(value) = std::env::var(key) {
            if !value.starts_with("()") {
                set(key, value);
            }
        }
    }
    // Override the SDK's automatic defaults; no model keys, auth paths, NODE_OPTIONS or loader hooks.
    set("HOME", std::env::var("HOME").unwrap_or_default());
    set("LOGNAME", String::new());
    set("USER", String::new());
    set("SHELL", String::new());
    set("TERM", "dumb".into());
    set("PATH", dirname(entry).to_string_lossy().into_owned());
    if allow_tools.is_empty() {
        set("ABLETON_MCP_TOOL_POLICY", "read-only".into());
    } else {
        let unique: BTreeSet<&str> = allow_tools.iter().map(String::as_str).collect();
        set("ABLETON_MCP_TOOL_POLICY", "full".into());
        set("ABLETON_MCP_TOOL_ALLOW", unique.into_iter().collect::<Vec<_>>().join(","));
    }
    // Windows runtime variables are not inference credentials. No complete process.env inheritance. PROGRAMDATA is
    // where the bridge finds Live's Extension Host, on whichever drive Windows is.
    for key in ["SYSTEMROOT", "SYSTEMDRIVE", "TEMP", "TMP", "PROGRAMDATA"] {
        if let Some(value) = std::env::var(key).ok().filter(|value| !value.is_empty()) {
            set(key, value);
        }
    }
    environment
}

pub async fn connect_mcp(options: Options) -> Result<Rc<dyn McpEndpoint>, RuntimeError> {
    options.signal.check()?;
    // Past the bridge's own longest deadline (60 s): a long render or a big Set's read finishes.
    let timeout = options.timeout_ms.unwrap_or(65_000);
    let connect_timeout = options.connect_timeout_ms.unwrap_or(timeout);
    if !valid_timeout(timeout) || !valid_timeout(connect_timeout) {
        return Err(RuntimeError::plain("Invalid MCP timeout"));
    }
    let entry = options.entry.clone().unwrap_or_else(bridge_entry);
    let mut command = Command::new(&entry);
    if let Some(config) = &options.bridge_config {
        command.arg("--config").arg(config);
    }
    command.args(&options.args);
    command.env_clear().envs(child_environment(&entry, &options.allow_tools));
    command.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).kill_on_drop(true);
    // The standalone bridge resolves protocol assets from the repository root.
    command.current_dir(options.cwd.clone().unwrap_or_else(|| dirname(&bridge_entry())));
    #[cfg(windows)]
    command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW: the SDK's windowsHide.
    let endpoint = Endpoint::new(timeout, options.on_dispatch.clone());
    match endpoint.connect(command, connect_timeout, &options.signal).await {
        Ok(()) => Ok(endpoint),
        Err(_) => {
            endpoint.close().await?;
            // The bridge's own words when it stopped (Live running another version of the bridge, say), else
            // where to look.
            Err(RuntimeError::plain(match endpoint.bridge_said() {
                Some(said) => format!("Kumi's bridge didn't start: {said}"),
                None => "MCP connection failed; check the built bridge, config and Live setup".into(),
            }))
        }
    }
}

/// A request still awaiting its answer when its future goes (an outer timeout, a batch that stopped early): its entry
/// goes and the server is told, as at the request's own deadline, so the bridge stops working on it. The notice is
/// queued at once, with no task: the runtime may be ending.
struct Abandoned<'a> {
    pending: &'a RefCell<HashMap<i64, oneshot::Sender<Result<Payload, McpError>>>>,
    transport: Rc<Transport>,
    id: i64,
}
impl Drop for Abandoned<'_> {
    fn drop(&mut self) {
        // Answered, timed out or cancelled, the request took its entry already.
        let waiting = self.pending.try_borrow_mut().ok().and_then(|mut pending| pending.remove(&self.id)).is_some();
        if waiting {
            let notice = json!({
                "jsonrpc": "2.0",
                "method": "notifications/cancelled",
                "params": { "requestId": self.id, "reason": "AbortError: This operation was aborted" },
            });
            drop(self.transport.send(line(&notice)));
        }
    }
}
/// `JSON.stringify(message) + "\n"`: one message on the wire.
fn line(message: &Value) -> String {
    let mut text = stringify(message);
    text.push('\n');
    text
}

/// A `_meta`-checked payload back as the plain object it came from.
fn payload_object(payload: &Payload) -> JsonObject {
    match serde_json::to_value(payload) {
        Ok(Value::Object(object)) => object,
        _ => JsonObject::new(),
    }
}

/// `RequestIdSchema` / `ProgressTokenSchema`: a string or a safe integer.
fn is_request_id(value: &Value) -> bool {
    serde_json::from_value::<RequestId>(value.clone()).is_ok()
}

/// What the writer task is asked: a line to write, or the end of stdin.
enum Write {
    Line(Vec<u8>, oneshot::Sender<Result<(), String>>),
    End,
}

enum Kill {
    Term,
    Kill,
}

/// The SDK's `StdioClientTransport` with Kumi's `OwnedStdioTransport` on top: the child, its pipes, the
/// read buffer, and ONE shared close (SDK 1.30.1 starts an unawaited close on initialize failure, and its
/// stdio close clears pid before exit; Kumi retains the owned identity).
struct Transport {
    owner: Weak<Endpoint>,
    pid: u32,
    /// The SDK's `_process`: cleared when `close()` begins or the child closes (`pid` is null then).
    attached: Cell<bool>,
    writes: mpsc::UnboundedSender<Write>,
    kills: mpsc::UnboundedSender<Kill>,
    /// `exitCode !== null`: the child was seen to exit.
    exited: Cell<bool>,
    /// The child's `close` event: exited, with stdout and stderr closed.
    closed: watch::Sender<bool>,
    closes_needed: Cell<u8>,
    read_buffer: RefCell<LinearReadBuffer>,
    shutdown: RefCell<Option<Shared<LocalBoxFuture<'static, ()>>>>,
    stderr_bytes: Cell<usize>,
    stderr_truncated: Cell<bool>,
    /// The bridge's own last line ("mcp-host: …", why it stopped), the only stderr text kept; and the line being
    /// read, at most a KiB of it, dropped once read.
    stderr_said: RefCell<Option<String>>,
    stderr_line: RefCell<Vec<u8>>,
}

impl Transport {
    /// `spawn` plus `start()`: the child running, its pipes read by tasks.
    fn spawn(owner: Weak<Endpoint>, mut command: Command) -> Result<Rc<Self>, String> {
        let mut child = command.spawn().map_err(|error| error.to_string())?;
        let pid = child.id().unwrap_or(0);
        let stdin = child.stdin.take();
        let stdout = child.stdout.take();
        let stderr = child.stderr.take();
        let (writes, write_queue) = mpsc::unbounded_channel();
        let (kills, kill_queue) = mpsc::unbounded_channel();
        let (closed, _) = watch::channel(false);
        let transport = Rc::new(Self {
            owner,
            pid,
            attached: Cell::new(true),
            writes,
            kills,
            exited: Cell::new(false),
            closed,
            closes_needed: Cell::new(3),
            read_buffer: RefCell::new(LinearReadBuffer::new(MAX_BRIDGE_MESSAGE_BYTES)),
            shutdown: RefCell::new(None),
            stderr_bytes: Cell::new(0),
            stderr_truncated: Cell::new(false),
            stderr_said: RefCell::new(None),
            stderr_line: RefCell::new(Vec::new()),
        });
        spawn_local(Self::write_loop(Rc::downgrade(&transport), stdin, write_queue));
        spawn_local(transport.clone().read_stdout(stdout));
        spawn_local(transport.clone().drain_stderr(stderr));
        spawn_local(transport.clone().wait_exit(child, kill_queue));
        Ok(transport)
    }

    fn on_error(&self, message: &str) {
        if let Some(owner) = self.owner.upgrade() {
            owner.on_error(message);
        }
    }

    /// Writes go out in the order they were asked, each resolving once written; a failed write is the
    /// stdin stream's `error` (onerror), and none is possible once stdin ended.
    async fn write_loop(transport: Weak<Self>, mut stdin: Option<ChildStdin>, mut queue: mpsc::UnboundedReceiver<Write>) {
        while let Some(write) = queue.recv().await {
            match write {
                Write::Line(bytes, done) => {
                    let result = match stdin.as_mut() {
                        Some(stdin) => stdin.write_all(&bytes).await.map_err(|error| error.to_string()),
                        None => Err("Not connected".to_string()),
                    };
                    if let (Err(error), true) = (&result, stdin.is_some()) {
                        if let Some(transport) = transport.upgrade() {
                            transport.on_error(error);
                        }
                    }
                    let _ = done.send(result);
                }
                Write::End => stdin = None,
            }
        }
    }

    async fn read_stdout(self: Rc<Self>, stdout: Option<ChildStdout>) {
        if let Some(mut stdout) = stdout {
            let mut chunk = vec![0u8; 64 * 1024];
            loop {
                let read = match stdout.read(&mut chunk).await {
                    Ok(0) | Err(_) => break,
                    Ok(read) => read,
                };
                let appended = self.read_buffer.borrow_mut().append(&chunk[..read]);
                match appended {
                    Ok(()) => self.process_read_buffer(),
                    Err(error) => {
                        self.on_error(&error.message());
                        spawn_local(self.close());
                    }
                }
            }
        }
        self.one_closed();
    }

    fn process_read_buffer(&self) {
        loop {
            let next = self.read_buffer.borrow_mut().read_message();
            match next {
                Ok(Some(message)) => {
                    if let Some(owner) = self.owner.upgrade() {
                        owner.on_message(message);
                    }
                }
                Ok(None) => break,
                Err(error) => self.on_error(&error.message()),
            }
        }
    }

    // Drain early output to avoid child backpressure, retaining only bounded byte-count metadata.
    async fn drain_stderr(self: Rc<Self>, stderr: Option<ChildStderr>) {
        if let Some(mut stderr) = stderr {
            let mut chunk = vec![0u8; 64 * 1024];
            loop {
                match stderr.read(&mut chunk).await {
                    Ok(0) | Err(_) => break,
                    Ok(read) => {
                        let bytes = self.stderr_bytes.get();
                        if bytes + read > 64 * 1024 {
                            self.stderr_truncated.set(true);
                        }
                        self.stderr_bytes.set((bytes + read).min(64 * 1024));
                        for &byte in &chunk[..read] {
                            if byte == b'\n' {
                                self.end_stderr_line();
                            } else if self.stderr_line.borrow().len() < 1024 {
                                self.stderr_line.borrow_mut().push(byte);
                            }
                        }
                    }
                }
            }
        }
        self.end_stderr_line();
        self.one_closed();
    }

    /// A line of the bridge's stderr, read: kept only when it's the bridge's own ("mcp-host: …").
    fn end_stderr_line(&self) {
        let line = std::mem::take(&mut *self.stderr_line.borrow_mut());
        let text = String::from_utf8_lossy(&line);
        if let Some(said) = text.trim().strip_prefix("mcp-host: ") {
            *self.stderr_said.borrow_mut() = Some(said.chars().take(300).collect());
        }
    }

    /// Owns the child: waits for its exit, and sends the signals `close()` asks for meanwhile.
    async fn wait_exit(self: Rc<Self>, mut child: Child, mut kills: mpsc::UnboundedReceiver<Kill>) {
        let mut open = true;
        loop {
            tokio::select! {
                _ = child.wait() => break,
                kill = kills.recv(), if open => match kill {
                    Some(Kill::Term) => self.terminate(&mut child),
                    Some(Kill::Kill) => {
                        let _ = child.start_kill();
                    }
                    None => open = false,
                },
            }
        }
        self.exited.set(true);
        self.one_closed();
        // A process the bridge started may hold its pipes open (on Windows it can inherit them), so their ends may
        // never come: once the bridge is gone, what it wrote gets a moment to be read, then the transport is closed.
        // The moment ends as soon as the pipes have: a task left waiting holds up Kumi's own exit.
        let until = tokio::time::Instant::now() + EXIT_GRACE;
        while self.closes_needed.get() > 0 && tokio::time::Instant::now() < until {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        if self.closes_needed.get() > 0 {
            self.closes_needed.set(1);
            self.one_closed();
        }
    }

    #[cfg(unix)]
    fn terminate(&self, _child: &mut Child) {
        // SAFETY: a plain kill(2) of the pid this transport spawned; it has no memory-safety preconditions.
        unsafe {
            libc::kill(self.pid as libc::pid_t, libc::SIGTERM);
        }
    }

    /// On Windows both signals are TerminateProcess.
    #[cfg(not(unix))]
    fn terminate(&self, child: &mut Child) {
        let _ = child.start_kill();
    }

    /// One of exit, stdout's end and stderr's end; the third is the child's `close` event (once only).
    fn one_closed(&self) {
        let Some(left) = self.closes_needed.get().checked_sub(1) else { return };
        self.closes_needed.set(left);
        if left == 0 {
            self.attached.set(false);
            let _ = self.closed.send(true);
            if let Some(owner) = self.owner.upgrade() {
                owner.on_transport_close();
            }
        }
    }

    /// The line is queued now (writes keep their order); the future settles once it is written.
    fn send(&self, line: String) -> impl Future<Output = Result<(), String>> + 'static {
        let queued = if self.attached.get() {
            let (done, result) = oneshot::channel();
            self.writes.send(Write::Line(line.into_bytes(), done)).ok().map(|_| result)
        } else {
            None
        };
        async move {
            match queued {
                Some(result) => result.await.unwrap_or_else(|_| Err("Not connected".to_string())),
                None => Err("Not connected".to_string()),
            }
        }
    }

    /// One close for everyone who asks: stdin's end, then after 2 s SIGTERM, then after 2 s SIGKILL (not waited for).
    fn close(self: &Rc<Self>) -> Shared<LocalBoxFuture<'static, ()>> {
        let mut shutdown = self.shutdown.borrow_mut();
        shutdown
            .get_or_insert_with(|| {
                let (done, result) = oneshot::channel::<()>();
                let this = self.clone();
                spawn_local(async move {
                    this.close_sequence().await;
                    let _ = done.send(());
                });
                result.map(|_| ()).boxed_local().shared()
            })
            .clone()
    }

    async fn close_sequence(&self) {
        if self.attached.replace(false) {
            let mut closed = self.closed.subscribe();
            let _ = self.writes.send(Write::End);
            tokio::select! {
                _ = closed.wait_for(|closed| *closed) => {}
                _ = sleep(Duration::from_millis(2000)) => {}
            }
            if !self.exited.get() {
                let _ = self.kills.send(Kill::Term);
                tokio::select! {
                    _ = closed.wait_for(|closed| *closed) => {}
                    _ = sleep(Duration::from_millis(2000)) => {}
                }
            }
            if !self.exited.get() {
                let _ = self.kills.send(Kill::Kill);
            }
        }
        self.read_buffer.borrow_mut().clear();
    }
}

/// A set of listeners that can be removed by the handle `add` returns.
struct Listeners<F: ?Sized> {
    next: Cell<u64>,
    items: RefCell<Vec<(u64, Rc<F>)>>,
}

impl<F: ?Sized> Listeners<F> {
    fn new() -> Self {
        Self { next: Cell::new(0), items: RefCell::new(Vec::new()) }
    }

    fn add(&self, listener: Rc<F>) -> u64 {
        let id = self.next.get();
        self.next.set(id + 1);
        self.items.borrow_mut().push((id, listener));
        id
    }

    fn remove(&self, id: u64) {
        self.items.borrow_mut().retain(|(known, _)| *known != id);
    }

    /// `[...listeners]`: a copy to call, so a listener may add or remove one meanwhile.
    fn snapshot(&self) -> Vec<Rc<F>> {
        self.items.borrow().iter().map(|(_, listener)| listener.clone()).collect()
    }
}

/// Why a request didn't answer: the SDK's rejections, as Kumi told them apart.
#[derive(Debug, thiserror::Error)]
enum RequestFailure {
    #[error("Not connected")]
    NotConnected,
    #[error("This operation was aborted")]
    Aborted,
    #[error("{0}")]
    Mcp(McpError),
    /// The write failed (its error text).
    #[error("{0}")]
    Send(String),
    /// The answer wasn't the shape the schema asked for (a ZodError).
    #[error("{0}")]
    Invalid(String),
    #[error("{0}")]
    Plain(String),
}

static IGNORED_ERRORS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^Received a (response for an unknown message ID|progress notification for an unknown token)").expect("regex")
});
static MCP_ERROR_PREFIX: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^(?:MCP error -?[0-9]+:\s*)+").expect("regex"));

/// The SDK `Client` (request ids, pending answers, the server's notifications and pings) and Kumi's
/// endpoint on top of it, in one object.
struct Endpoint {
    me: Weak<Endpoint>,
    timeout: u64,
    on_dispatch: Option<Rc<dyn Fn(&str)>>,
    transport: RefCell<Option<Rc<Transport>>>,
    next_id: Cell<i64>,
    pending: RefCell<HashMap<i64, oneshot::Sender<Result<Payload, McpError>>>>,
    server_version: RefCell<Option<Implementation>>,
    /// The child this endpoint started, kept past the link's end (its pid, and whether it exited).
    owned: RefCell<Option<Rc<Transport>>>,
    catalog_listeners: Listeners<dyn Fn()>,
    disconnect_listeners: Listeners<dyn Fn()>,
    live_event_listeners: Listeners<dyn Fn(JsonObject)>,
    ready: Cell<bool>,
    disconnected: Cell<bool>,
    closing: RefCell<Option<Shared<LocalBoxFuture<'static, Result<(), RuntimeError>>>>>,
}

impl Endpoint {
    fn new(timeout: u64, on_dispatch: Option<Rc<dyn Fn(&str)>>) -> Rc<Self> {
        Rc::new_cyclic(|me| Self {
            me: me.clone(),
            timeout,
            on_dispatch,
            transport: RefCell::new(None),
            next_id: Cell::new(0),
            pending: RefCell::new(HashMap::new()),
            server_version: RefCell::new(None),
            owned: RefCell::new(None),
            catalog_listeners: Listeners::new(),
            disconnect_listeners: Listeners::new(),
            live_event_listeners: Listeners::new(),
            ready: Cell::new(false),
            disconnected: Cell::new(false),
            closing: RefCell::new(None),
        })
    }

    /// Why the bridge stopped, as it said it: its own last line ("mcp-host: …") on stderr, if it wrote one.
    fn bridge_said(&self) -> Option<String> {
        self.owned.borrow().as_ref()?.stderr_said.borrow().clone()
    }

    /// `client.connect(transport, …)`, then Kumi's own checks: the endpoint is ready, or the reason it isn't.
    async fn connect(self: &Rc<Self>, command: Command, connect_timeout: u64, signal: &Signal) -> Result<(), RequestFailure> {
        let transport = Transport::spawn(Rc::downgrade(self), command).map_err(RequestFailure::Send)?;
        *self.transport.borrow_mut() = Some(transport.clone());
        *self.owned.borrow_mut() = Some(transport.clone());
        let handshake = async {
            let params = json!({
                "protocolVersion": LATEST_PROTOCOL_VERSION,
                "capabilities": {},
                "clientInfo": { "name": "kumi", "version": KUMI_VERSION },
            });
            let result = self.request("initialize", Some(params), connect_timeout, signal).await?;
            let result: InitializeResult = serde_json::from_value(Value::Object(payload_object(&result)))
                .map_err(|error| RequestFailure::Invalid(error.to_string()))?;
            if !SUPPORTED_PROTOCOL_VERSIONS.contains(&result.protocol_version.as_str()) {
                return Err(RequestFailure::Plain(format!("Server's protocol version is not supported: {}", result.protocol_version)));
            }
            *self.server_version.borrow_mut() = Some(result.server_info);
            self.notification("notifications/initialized").await
        }
        .await;
        if let Err(error) = handshake {
            // Disconnect if initialization fails (the SDK's unawaited close; Kumi's own close joins it).
            spawn_local(transport.close());
            return Err(error);
        }
        if signal.aborted() {
            return Err(RequestFailure::Aborted);
        }
        if self.disconnected.get() {
            return Err(RequestFailure::Plain("MCP disconnected during initialization".into()));
        }
        self.ready.set(true);
        Ok(())
    }

    /// `Protocol.request`: one id, one deadline, a `notifications/cancelled` on timeout or abort.
    async fn request(&self, method: &str, params: Option<Value>, timeout_ms: u64, signal: &Signal) -> Result<Payload, RequestFailure> {
        let transport = self.transport.borrow().clone().ok_or(RequestFailure::NotConnected)?;
        if signal.aborted() {
            return Err(RequestFailure::Aborted);
        }
        let message_id = self.next_id.get();
        self.next_id.set(message_id + 1);
        let mut envelope = JsonObject::new();
        envelope.insert("method".into(), Value::String(method.into()));
        if let Some(params) = params {
            envelope.insert("params".into(), params);
        }
        envelope.insert("jsonrpc".into(), Value::String("2.0".into()));
        envelope.insert("id".into(), Value::from(message_id));
        let (answer, answered) = oneshot::channel();
        self.pending.borrow_mut().insert(message_id, answer);
        let _abandoned = Abandoned { pending: &self.pending, transport: transport.clone(), id: message_id };
        let (failed, failure) = oneshot::channel::<String>();
        let send = transport.send(line(&Value::Object(envelope)));
        spawn_local(async move {
            if let Err(error) = send.await {
                let _ = failed.send(error);
            }
        });
        let cancel = |reason: &str| {
            self.pending.borrow_mut().remove(&message_id);
            let notice = json!({
                "jsonrpc": "2.0",
                "method": "notifications/cancelled",
                "params": { "requestId": message_id, "reason": reason },
            });
            self.send_detached(&transport, line(&notice), "Failed to send cancellation");
        };
        tokio::select! {
            biased;
            answer = answered => match answer {
                Ok(Ok(result)) => Ok(result),
                Ok(Err(error)) => Err(RequestFailure::Mcp(error)),
                Err(_) => Err(RequestFailure::Mcp(McpError::new(ErrorCode::CONNECTION_CLOSED, "Connection closed", None))),
            },
            _ = signal.cancelled() => {
                cancel("AbortError: This operation was aborted");
                Err(RequestFailure::Mcp(McpError::new(ErrorCode::REQUEST_TIMEOUT, "AbortError: This operation was aborted", None)))
            }
            _ = sleep(Duration::from_millis(timeout_ms)) => {
                cancel("McpError: MCP error -32001: Request timed out");
                Err(RequestFailure::Mcp(McpError::new(ErrorCode::REQUEST_TIMEOUT, "Request timed out", Some(json!({ "timeout": timeout_ms })))))
            }
            Ok(error) = failure => {
                self.pending.borrow_mut().remove(&message_id);
                Err(RequestFailure::Send(error))
            }
        }
    }

    /// `Protocol.notification` for the one Kumi sends without params.
    async fn notification(&self, method: &str) -> Result<(), RequestFailure> {
        let transport = self.transport.borrow().clone().ok_or(RequestFailure::NotConnected)?;
        transport.send(line(&json!({ "method": method, "jsonrpc": "2.0" }))).await.map_err(RequestFailure::Send)
    }

    /// A write nobody waits for; its failure is reported as `<failure>: <error>`.
    fn send_detached(&self, transport: &Rc<Transport>, line: String, failure: &'static str) {
        let send = transport.send(line);
        let me = self.me.clone();
        spawn_local(async move {
            if let Err(error) = send.await {
                if let Some(me) = me.upgrade() {
                    me.on_error(&format!("{failure}: {error}"));
                }
            }
        });
    }

    fn on_message(&self, message: JsonRpcMessage) {
        match message {
            JsonRpcMessage::Response(response) => {
                let id = response.id.clone();
                self.on_response(Some(&id), Ok(response.result.clone()), || {
                    stringify(&serde_json::to_value(&response).unwrap_or(Value::Null))
                });
            }
            JsonRpcMessage::Error(error) => {
                let id = error.id.clone();
                let failure = McpError::new(error.error.code, &error.error.message, error.error.data.clone());
                self.on_response(id.as_ref(), Err(failure), || stringify(&serde_json::to_value(&error).unwrap_or(Value::Null)));
            }
            JsonRpcMessage::Request(request) => self.on_request(request),
            JsonRpcMessage::Notification(notification) => self.on_notification(notification),
        }
    }

    /// Answers are matched by `Number(response.id)`; one for no request in flight is reported, not fatal.
    fn on_response(&self, id: Option<&RequestId>, result: Result<Payload, McpError>, text: impl FnOnce() -> String) {
        let key = id.map(RequestId::to_js_number).filter(|number| number.fract() == 0.0 && number.abs() <= 9_007_199_254_740_991.0);
        let handler = key.and_then(|key| self.pending.borrow_mut().remove(&(key as i64)));
        match handler {
            Some(handler) => {
                let _ = handler.send(result);
            }
            None => self.on_error(&format!("Received a response for an unknown message ID: {}", text())),
        }
    }

    /// Only `ping` is answered; any other request is -32601.
    fn on_request(&self, request: JsonRpcRequest) {
        let Some(transport) = self.transport.borrow().clone() else { return };
        if request.method == "ping" {
            let response = json!({ "result": {}, "jsonrpc": "2.0", "id": request.id });
            self.send_detached(&transport, line(&response), "Failed to send response");
        } else {
            let response = json!({
                "jsonrpc": "2.0",
                "id": request.id,
                "error": { "code": ErrorCode::METHOD_NOT_FOUND, "message": "Method not found" },
            });
            self.send_detached(&transport, line(&response), "Failed to send an error response");
        }
    }

    fn on_notification(&self, notification: JsonRpcNotification) {
        match notification.method.as_str() {
            // CancelledNotificationSchema: params required; requestId (an id) and reason (a string) optional.
            // A falsy requestId is ignored, and Kumi has no server-sent request in flight to abort.
            "notifications/cancelled" => {
                let valid = notification.params.as_ref().is_some_and(|params| {
                    params.rest.get("requestId").is_none_or(is_request_id) && params.rest.get("reason").is_none_or(Value::is_string)
                });
                if !valid {
                    self.on_error("Uncaught error in notification handler: ZodError: invalid notifications/cancelled");
                }
            }
            // ProgressNotificationSchema; Kumi never asks for progress, so a valid one has no token to match.
            "notifications/progress" => {
                let valid = notification.params.as_ref().is_some_and(|params| {
                    params.rest.get("progressToken").is_some_and(is_request_id)
                        && params.rest.get("progress").is_some_and(Value::is_number)
                        && params.rest.get("total").is_none_or(Value::is_number)
                        && params.rest.get("message").is_none_or(Value::is_string)
                });
                if valid {
                    let text = stringify(&serde_json::to_value(&notification).unwrap_or(Value::Null));
                    self.on_error(&format!("Received a progress notification for an unknown token: {text}"));
                } else {
                    self.on_error("Uncaught error in notification handler: ZodError: invalid notifications/progress");
                }
            }
            "notifications/tools/list_changed" => {
                for listener in self.catalog_listeners.snapshot() {
                    listener();
                }
            }
            // Live's events: a method the SDK has no schema for, so it comes through the fallback handler.
            "notifications/live_event" => {
                if let Some(params) = &notification.params {
                    let event = payload_object(params);
                    for listener in self.live_event_listeners.snapshot() {
                        // A listener failure must not affect the link.
                        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| listener(event.clone())));
                    }
                }
            }
            _ => {}
        }
    }

    /// The transport's `onerror`. An answer (or progress) for a request Kumi stopped waiting for can cross
    /// the cancel on its way: Live quitting mid-request does this. The connection is fine; anything else ends it.
    fn on_error(&self, message: &str) {
        if IGNORED_ERRORS.is_match(message) {
            return;
        }
        self.disconnected_once();
        let close = self.close();
        spawn_local(async move {
            let _ = close.await;
        });
    }

    /// The child's `close` event (`Protocol._onclose`): the link is gone, and every pending request hears so.
    fn on_transport_close(&self) {
        let handlers: Vec<_> = self.pending.borrow_mut().drain().map(|(_, handler)| handler).collect();
        *self.transport.borrow_mut() = None;
        self.disconnected_once();
        for handler in handlers {
            let _ = handler.send(Err(McpError::new(ErrorCode::CONNECTION_CLOSED, "Connection closed", None)));
        }
    }

    fn disconnected_once(&self) {
        self.ready.set(false);
        if self.disconnected.replace(true) {
            return;
        }
        for listener in self.disconnect_listeners.snapshot() {
            listener();
        }
    }

    fn require_ready(&self, signal: &Signal) -> Result<(), RuntimeError> {
        signal.check()?;
        if !self.ready.get() || self.closing.borrow().is_some() {
            return Err(RuntimeError::plain("Kumi's link to Live is down; it reconnects when Live is back."));
        }
        Ok(())
    }

    /// One close for everyone who asks (it starts at once, awaited or not).
    fn close(&self) -> Shared<LocalBoxFuture<'static, Result<(), RuntimeError>>> {
        let mut closing = self.closing.borrow_mut();
        closing
            .get_or_insert_with(|| {
                let (done, result) = oneshot::channel();
                match self.me.upgrade() {
                    Some(me) => {
                        spawn_local(async move {
                            let _ = done.send(me.close_work().await);
                        });
                    }
                    None => {
                        let _ = done.send(Ok(()));
                    }
                };
                result.map(|result| result.unwrap_or(Ok(()))).boxed_local().shared()
            })
            .clone()
    }

    async fn close_work(&self) -> Result<(), RuntimeError> {
        self.ready.set(false);
        let transport = self.transport.borrow().clone();
        if let Some(transport) = transport {
            transport.close().await; // SDK alone owns stdin close → SIGTERM → SIGKILL for this child.
        }
        self.disconnected_once();
        let owned = self.owned.borrow().clone();
        if let Some(owned) = owned {
            for _ in 0..20 {
                if owned.exited.get() {
                    return Ok(());
                }
                sleep(Duration::from_millis(10)).await;
            }
            return Err(RuntimeError::plain("Owned MCP child exit could not be verified"));
        }
        Ok(())
    }

    /// `(error) => new Error(signal.aborted ? "MCP request cancelled" : "MCP request failed or timed out")`.
    fn request_error(signal: &Signal) -> RuntimeError {
        RuntimeError::plain(if signal.aborted() { "MCP request cancelled" } else { "MCP request failed or timed out" })
    }

    fn unsubscribe(&self, remove: impl Fn(&Endpoint) + 'static) -> Box<dyn Fn()> {
        let me = self.me.clone();
        Box::new(move || {
            if let Some(me) = me.upgrade() {
                remove(&me);
            }
        })
    }
}

#[async_trait(?Send)]
impl McpEndpoint for Endpoint {
    fn pid(&self) -> Option<u32> {
        self.owned.borrow().as_ref().map(|owned| owned.pid)
    }

    fn server_info(&self) -> Option<Implementation> {
        self.server_version.borrow().clone()
    }

    async fn list(&self, cursor: Option<&str>, signal: Signal) -> Result<ListToolsResult, RuntimeError> {
        self.require_ready(&signal)?;
        let params = match cursor {
            None => json!({}),
            Some(cursor) => json!({ "cursor": cursor }),
        };
        let listed = async {
            let result = self.request("tools/list", Some(params), self.timeout, &signal).await?;
            serde_json::from_value::<ListToolsResult>(Value::Object(payload_object(&result)))
                .map_err(|error| RequestFailure::Invalid(error.to_string()))
        }
        .await;
        listed.map_err(|_| Self::request_error(&signal))
    }

    async fn call(&self, name: &str, args: JsonObject, signal: Signal) -> Result<CallToolResult, RuntimeError> {
        self.require_ready(&signal)?;
        if let Some(on_dispatch) = &self.on_dispatch {
            on_dispatch(name);
        }
        let called = async {
            let params = json!({ "name": name, "arguments": args });
            let result = self.request("tools/call", Some(params), self.timeout, &signal).await?;
            serde_json::from_value::<CallToolResult>(Value::Object(payload_object(&result)))
                .map(|mut result| {
                    // The SDK's CallToolResultSchema emits known keys in schema order.
                    // Direct test/custom endpoints retain their own object insertion order.
                    result.field_order.clear();
                    result
                })
                .map_err(|error| RequestFailure::Invalid(error.to_string()))
        }
        .await;
        match called {
            Ok(result) => Ok(result),
            // The bridge's own word on bad arguments ("trackRef is required") helps the caller correct them.
            Err(RequestFailure::Mcp(error)) if !signal.aborted() && error.code == ErrorCode::INVALID_PARAMS => {
                let stripped = MCP_ERROR_PREFIX.replace(&error.message, "");
                let cleaned: String = stripped.chars().map(|c| if (c as u32) <= 0x1f || c == '\x7f' { ' ' } else { c }).collect();
                let reason = head(&cleaned, 300);
                Ok(CallToolResult {
                    is_error: Some(true),
                    content: vec![ContentBlock::text(format!("The bridge rejected the arguments: {reason}"))],
                    field_order: vec!["isError".into(), "content".into()],
                    ..Default::default()
                })
            }
            Err(_) => Err(Self::request_error(&signal)),
        }
    }

    fn on_catalog_changed(&self, listener: Rc<dyn Fn()>) -> Box<dyn Fn()> {
        let id = self.catalog_listeners.add(listener);
        self.unsubscribe(move |endpoint| endpoint.catalog_listeners.remove(id))
    }

    fn on_disconnect(&self, listener: Rc<dyn Fn()>) -> Box<dyn Fn()> {
        let id = self.disconnect_listeners.add(listener);
        self.unsubscribe(move |endpoint| endpoint.disconnect_listeners.remove(id))
    }

    fn has_on_live_event(&self) -> bool {
        true
    }

    fn on_live_event(&self, listener: Rc<dyn Fn(JsonObject)>) -> Box<dyn Fn()> {
        let id = self.live_event_listeners.add(listener);
        self.unsubscribe(move |endpoint| endpoint.live_event_listeners.remove(id))
    }

    fn stderr_status(&self) -> StderrStatus {
        match self.owned.borrow().as_ref() {
            Some(owned) => StderrStatus { bytes: owned.stderr_bytes.get(), truncated: owned.stderr_truncated.get() },
            None => StderrStatus { bytes: 0, truncated: false },
        }
    }

    async fn close(&self) -> Result<(), RuntimeError> {
        Endpoint::close(self).await
    }
}
