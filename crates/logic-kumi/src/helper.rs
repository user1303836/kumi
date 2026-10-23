//! The helper, `kumi-logic-hands`: the bridge's child that holds the macOS channels (Accessibility, key events, MIDI).
//! The bridge writes one JSON request per line on its stdin and reads replies and events from its stdout
//! (`docs/en/LOGIC_CONTRACT.md`, "Helper protocol"; the helper's own page is `helper/README.md`): a request is
//! `{"id": n, "op": "…", …}` with an increasing id, its reply `{"id": n, "ok": true, "ms": n, …}` or `{"id": n,
//! "ok": false, "error": "<code>", …}` (`ms` being how long the helper took), an event `{"event": "…", …}`.
//! One request at a time, a reply within 2 s; a helper that exits is started again, with a backoff. [`Helper`] is what
//! the rest of the bridge uses, so the tests run on [`MockHands`] without a helper or a Logic.

use std::collections::{HashMap, VecDeque};
use std::io::{self, BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::log;

/// A reply comes within this, or the request has failed.
pub const REPLY_TIMEOUT: Duration = Duration::from_secs(2);
/// Requests left unanswered in a row before the helper is taken for stuck and started again: a blocked Accessibility
/// call never returns on its own.
pub const STUCK_AFTER: u32 = 5;
/// The wait before a helper that exited is started again: doubled on each exit up to the cap, and back to the first
/// once a helper has run for [`STEADY`].
pub const FIRST_BACKOFF: Duration = Duration::from_millis(500);
pub const MAX_BACKOFF: Duration = Duration::from_secs(10);
pub const STEADY: Duration = Duration::from_secs(10);
/// How long a polite `quit` gets before the child is killed.
const QUIT_GRACE: Duration = Duration::from_millis(500);

/// Why a request to the helper has no reply.
#[derive(Debug, Clone, PartialEq)]
pub enum HelperError {
    /// The helper answered `ok: false`: its error code (`unknown-op`, `bad-request`, `untrusted`, `no-app`, `timeout`,
    /// `busy`, an AXError's name, …) and the whole reply, which may say more (`detail`, `result`, `before`, `after`).
    Refused { code: String, reply: Value },
    /// No reply within [`REPLY_TIMEOUT`].
    Timeout,
    /// No helper is running: it exited, couldn't be started, or its pipe broke. It is started again after a backoff;
    /// until then every request fails this way.
    Gone(String),
}

impl std::fmt::Display for HelperError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HelperError::Refused { code, reply } => match reply.get("detail").and_then(Value::as_str) {
                Some(detail) => write!(f, "the helper refused: {code} ({detail})"),
                None => write!(f, "the helper refused: {code}"),
            },
            HelperError::Timeout => write!(f, "the helper didn't answer within {} ms", REPLY_TIMEOUT.as_millis()),
            HelperError::Gone(why) => f.write_str(why),
        }
    }
}

impl std::error::Error for HelperError {}

/// A helper process, or something that stands in for one.
pub trait Helper: Send {
    /// One request, answered: the reply's fields (`id` and `ok` among them).
    fn call(&mut self, op: &str, params: Value) -> Result<Value, HelperError>;
    /// The next event the helper sent (`{"event": …}`), waiting at most `wait` for one.
    fn event(&mut self, wait: Duration) -> Option<Value>;
    /// Whether the helper process is still there.
    fn alive(&self) -> bool;
    /// Ends the helper: politely (a `quit`), then for good.
    fn quit(&mut self);
}

/// Makes helper processes: the real child, or the tests' mock.
pub trait Launch: Send {
    fn launch(&mut self) -> Result<Box<dyn Helper>, String>;
}

/// Starts the helper binary at a path.
pub struct ChildLauncher {
    pub path: PathBuf,
}

impl Launch for ChildLauncher {
    fn launch(&mut self) -> Result<Box<dyn Helper>, String> {
        match ChildHelper::spawn(&self.path) {
            Ok(child) => Ok(Box::new(child)),
            Err(error) => Err(format!("couldn't start the helper {}: {error}", self.path.display())),
        }
    }
}

/// One helper process: its pipes, a thread sorting its stdout into replies and events, and one passing its stderr to
/// the log.
pub struct ChildHelper {
    child: Child,
    stdin: Option<ChildStdin>,
    replies: Receiver<Value>,
    events: Receiver<Value>,
    next_id: u64,
    alive: Arc<AtomicBool>,
    threads: Vec<JoinHandle<()>>,
}

impl ChildHelper {
    pub fn spawn(path: &Path) -> io::Result<ChildHelper> {
        let mut child = Command::new(path).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn()?;
        let stdin = child.stdin.take();
        let stdout = child.stdout.take().ok_or_else(|| io::Error::other("no stdout pipe"))?;
        let stderr = child.stderr.take().ok_or_else(|| io::Error::other("no stderr pipe"))?;
        let alive = Arc::new(AtomicBool::new(true));
        let (reply_tx, replies) = mpsc::channel();
        let (event_tx, events) = mpsc::channel();
        let reader = {
            let alive = alive.clone();
            thread::Builder::new().name("kumi-hands-read".into()).spawn(move || {
                for line in BufReader::new(stdout).lines() {
                    let Ok(line) = line else { break };
                    match serde_json::from_str::<Value>(&line) {
                        Ok(value) if value.get("id").is_some() => {
                            if reply_tx.send(value).is_err() {
                                break;
                            }
                        }
                        Ok(value) if value.get("event").is_some() => {
                            if event_tx.send(value).is_err() {
                                break;
                            }
                        }
                        _ => log::line(&format!(
                            "the helper wrote a line that is neither a reply nor an event: {}",
                            daw_bridge::handshake::tidy(&line, 120)
                        )),
                    }
                }
                alive.store(false, Ordering::Release);
            })?
        };
        let logger = thread::Builder::new().name("kumi-hands-stderr".into()).spawn(move || {
            for line in BufReader::new(stderr).lines() {
                let Ok(line) = line else { break };
                log::line(&format!("helper: {line}"));
            }
        })?;
        Ok(ChildHelper { child, stdin, replies, events, next_id: 1, alive, threads: vec![reader, logger] })
    }

    pub fn pid(&self) -> u32 {
        self.child.id()
    }

    fn exit_status(&mut self) -> String {
        match self.child.try_wait() {
            Ok(Some(status)) => status.to_string(),
            Ok(None) => "still running".into(),
            Err(error) => error.to_string(),
        }
    }

    fn gone(&mut self) -> HelperError {
        HelperError::Gone(format!("the helper exited ({})", self.exit_status()))
    }

    fn send(&mut self, request: &Value) -> Result<(), HelperError> {
        if !self.alive.load(Ordering::Acquire) {
            return Err(self.gone());
        }
        let Some(stdin) = self.stdin.as_mut() else { return Err(HelperError::Gone("no pipe to the helper".into())) };
        let mut line = serde_json::to_vec(request).expect("requests serialize");
        line.push(b'\n');
        stdin
            .write_all(&line)
            .and_then(|_| stdin.flush())
            .map_err(|error| HelperError::Gone(format!("couldn't write to the helper: {error}")))
    }

    fn kill(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        self.alive.store(false, Ordering::Release);
        drop(self.stdin.take());
        for thread in self.threads.drain(..) {
            let _ = thread.join();
        }
    }
}

impl Helper for ChildHelper {
    fn call(&mut self, op: &str, params: Value) -> Result<Value, HelperError> {
        let id = self.next_id;
        self.next_id += 1;
        let mut request = json!({"id": id, "op": op});
        if let Value::Object(params) = params {
            for (key, value) in params {
                request[key] = value;
            }
        }
        self.send(&request)?;
        let deadline = Instant::now() + REPLY_TIMEOUT;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            match self.replies.recv_timeout(left) {
                Ok(reply) => {
                    let Some(reply_id) = reply.get("id").and_then(Value::as_u64) else { continue };
                    if reply_id < id {
                        // A late reply to a request that timed out.
                        continue;
                    }
                    if reply_id > id {
                        log::line(&format!("the helper answered request {reply_id} before {id}"));
                        continue;
                    }
                    return if reply["ok"] == Value::Bool(true) {
                        Ok(reply)
                    } else {
                        let code = reply["error"].as_str().unwrap_or("no reason given").to_string();
                        Err(HelperError::Refused { code, reply })
                    };
                }
                Err(RecvTimeoutError::Timeout) => return Err(HelperError::Timeout),
                Err(RecvTimeoutError::Disconnected) => return Err(self.gone()),
            }
        }
    }

    fn event(&mut self, wait: Duration) -> Option<Value> {
        self.events.recv_timeout(wait).ok()
    }

    fn alive(&self) -> bool {
        self.alive.load(Ordering::Acquire)
    }

    fn quit(&mut self) {
        if self.alive() {
            let id = self.next_id;
            self.next_id += 1;
            let _ = self.send(&json!({"id": id, "op": "quit"}));
            drop(self.stdin.take());
            let until = Instant::now() + QUIT_GRACE;
            while Instant::now() < until && matches!(self.child.try_wait(), Ok(None)) {
                thread::sleep(Duration::from_millis(10));
            }
        }
        self.kill();
    }
}

impl Drop for ChildHelper {
    fn drop(&mut self) {
        self.kill();
    }
}

/// The helper as the bridge uses it: one process at a time, started again when it exits (or stops answering), with
/// a backoff that doubles while it keeps failing.
pub struct Supervised {
    launcher: Box<dyn Launch>,
    current: Option<Box<dyn Helper>>,
    /// Exits and failed starts in a row, for the backoff.
    failures: u32,
    next_try: Option<Instant>,
    since: Option<Instant>,
    timeouts: u32,
    starts: u32,
    first_backoff: Duration,
    max_backoff: Duration,
    last_said: Option<String>,
}

impl Supervised {
    pub fn new(launcher: Box<dyn Launch>) -> Self {
        Self {
            launcher,
            current: None,
            failures: 0,
            next_try: None,
            since: None,
            timeouts: 0,
            starts: 0,
            first_backoff: FIRST_BACKOFF,
            max_backoff: MAX_BACKOFF,
            last_said: None,
        }
    }

    /// Another backoff than the default, for tests.
    pub fn with_backoff(mut self, first: Duration, max: Duration) -> Self {
        self.first_backoff = first;
        self.max_backoff = max;
        self
    }

    /// How many helper processes were started.
    pub fn starts(&self) -> u32 {
        self.starts
    }

    fn backoff(&self) -> Duration {
        let doublings = self.failures.saturating_sub(1).min(16);
        self.first_backoff.saturating_mul(1 << doublings).min(self.max_backoff)
    }

    /// The running helper, started if there is none and the backoff allows.
    fn ensure(&mut self) -> Result<&mut Box<dyn Helper>, HelperError> {
        if self.current.as_ref().is_some_and(|helper| !helper.alive()) {
            self.lost("the helper exited");
        }
        if self.current.is_none() {
            if let Some(at) = self.next_try {
                let left = at.saturating_duration_since(Instant::now());
                if !left.is_zero() {
                    return Err(HelperError::Gone(format!("the helper is started again in {} ms", left.as_millis())));
                }
            }
            match self.launcher.launch() {
                Ok(helper) => {
                    self.current = Some(helper);
                    self.since = Some(Instant::now());
                    self.starts += 1;
                    self.timeouts = 0;
                    self.next_try = None;
                    self.last_said = None;
                    log::line(&format!("helper started (start {})", self.starts));
                }
                Err(why) => {
                    self.fail(&why);
                    return Err(HelperError::Gone(why));
                }
            }
        }
        Ok(self.current.as_mut().expect("started above"))
    }

    /// The helper went: ended for good, and the next start scheduled.
    fn lost(&mut self, why: &str) {
        if let Some(mut helper) = self.current.take() {
            helper.quit();
        }
        self.fail(why);
    }

    fn fail(&mut self, why: &str) {
        // A helper that ran a while before it went starts the backoff over.
        if self.since.take().is_some_and(|since| since.elapsed() >= STEADY) {
            self.failures = 0;
        }
        self.failures += 1;
        let wait = self.backoff();
        self.next_try = Some(Instant::now() + wait);
        let said = format!("{why}; starting the helper again in {} ms", wait.as_millis());
        if self.last_said.as_deref() != Some(why) {
            log::line(&said);
            self.last_said = Some(why.to_string());
        }
    }
}

impl Helper for Supervised {
    fn call(&mut self, op: &str, params: Value) -> Result<Value, HelperError> {
        let result = self.ensure()?.call(op, params);
        match &result {
            Err(HelperError::Gone(why)) => {
                let why = why.clone();
                self.lost(&why);
            }
            Err(HelperError::Timeout) => {
                self.timeouts += 1;
                if self.timeouts >= STUCK_AFTER {
                    let why = format!("the helper didn't answer {} requests in a row", self.timeouts);
                    self.timeouts = 0;
                    self.lost(&why);
                }
            }
            _ => self.timeouts = 0,
        }
        result
    }

    fn event(&mut self, wait: Duration) -> Option<Value> {
        let (event, alive) = match self.ensure() {
            Ok(helper) => {
                let event = helper.event(wait);
                (event, helper.alive())
            }
            Err(_) => {
                thread::sleep(wait);
                return None;
            }
        };
        if !alive {
            self.lost("the helper exited");
        }
        event
    }

    fn alive(&self) -> bool {
        self.current.as_ref().is_some_and(|helper| helper.alive())
    }

    fn quit(&mut self) {
        if let Some(mut helper) = self.current.take() {
            helper.quit();
        }
    }
}

/// The tests' helper: what each op answers (scripted replies first, then the fixtures), the events it emits, and the
/// calls it saw. One `MockHands` stands for the helper binary: each launch gives a [`MockHelper`] on the same state.
#[derive(Clone, Default)]
pub struct MockHands {
    inner: Arc<(Mutex<MockState>, Condvar)>,
}

#[derive(Default)]
struct MockState {
    trusted: bool,
    apps: Vec<Value>,
    windows: Value,
    listed: Vec<String>,
    tree: Value,
    scripted: HashMap<String, VecDeque<Result<Value, HelperError>>>,
    events: VecDeque<Value>,
    calls: Vec<(String, Value)>,
    gone: bool,
    launches: u32,
    launch_error: Option<String>,
}

impl MockHands {
    /// Trusted, with no apps, windows or tree yet.
    pub fn new() -> Self {
        let hands = Self::default();
        hands.with(|state| {
            state.trusted = true;
            state.windows = Value::Array(vec![]);
            state.tree = Value::Null;
        });
        hands
    }

    fn with<T>(&self, f: impl FnOnce(&mut MockState) -> T) -> T {
        let (lock, condvar) = &*self.inner;
        let mut state = lock.lock().unwrap_or_else(|e| e.into_inner());
        let result = f(&mut state);
        condvar.notify_all();
        result
    }

    /// What `trusted` answers.
    pub fn set_trusted(&self, trusted: bool) {
        self.with(|state| state.trusted = trusted);
    }

    /// What `apps` answers (each `{bundle, pid, name, active, hidden, …}`), filtered by the bundles asked for.
    pub fn set_apps(&self, apps: Vec<Value>) {
        self.with(|state| state.apps = apps);
    }

    /// What `windows` answers: the windows and the Window menu's titles.
    pub fn set_windows(&self, windows: Value, listed: &[&str]) {
        self.with(|state| {
            state.windows = windows;
            state.listed = listed.iter().map(|title| title.to_string()).collect();
        });
    }

    /// What `tree` answers as its `node`.
    pub fn set_tree(&self, tree: Value) {
        self.with(|state| state.tree = tree);
    }

    /// One scripted reply for `op`, used before the fixtures, in the order given.
    pub fn reply(&self, op: &str, reply: Result<Value, HelperError>) {
        self.with(|state| state.scripted.entry(op.to_string()).or_default().push_back(reply));
    }

    /// An event the helper sends.
    pub fn emit(&self, event: Value) {
        self.with(|state| state.events.push_back(event));
    }

    /// The running helper exits: its requests fail until the bridge starts it again.
    pub fn exit(&self) {
        self.with(|state| state.gone = true);
    }

    /// Whether starts fail (with this message) from now on.
    pub fn fail_launches(&self, why: Option<&str>) {
        self.with(|state| state.launch_error = why.map(str::to_string));
    }

    /// How many helper processes were started.
    pub fn launches(&self) -> u32 {
        self.with(|state| state.launches)
    }

    /// Every request made, in order: the op and its parameters.
    pub fn calls(&self) -> Vec<(String, Value)> {
        self.with(|state| state.calls.clone())
    }

    pub fn calls_of(&self, op: &str) -> usize {
        self.with(|state| state.calls.iter().filter(|(called, _)| called == op).count())
    }
}

impl Launch for MockHands {
    fn launch(&mut self) -> Result<Box<dyn Helper>, String> {
        let launched: Result<(), String> = self.with(|state| {
            state.launches += 1;
            if let Some(why) = &state.launch_error {
                return Err(why.clone());
            }
            state.gone = false;
            Ok(())
        });
        launched?;
        Ok(Box::new(MockHelper { hands: self.clone() }))
    }
}

/// One "process" of [`MockHands`].
pub struct MockHelper {
    hands: MockHands,
}

impl Helper for MockHelper {
    fn call(&mut self, op: &str, params: Value) -> Result<Value, HelperError> {
        self.hands.with(|state| {
            if state.gone {
                return Err(HelperError::Gone("the mock helper exited".into()));
            }
            state.calls.push((op.to_string(), params.clone()));
            let id = state.calls.len();
            // As the helper shapes them: the request's id, `ok`, and `ms` (how long it took).
            let answered = |mut reply: Value| {
                reply["id"] = id.into();
                reply["ok"] = true.into();
                reply["ms"] = 0.into();
                reply
            };
            let refused =
                |code: &str| HelperError::Refused { code: code.into(), reply: json!({"id": id, "ok": false, "error": code, "ms": 0}) };
            if let Some(scripted) = state.scripted.get_mut(op).and_then(VecDeque::pop_front) {
                return scripted.map(answered);
            }
            // Every Accessibility, key and menu op checks the grant first.
            if !state.trusted && matches!(op, "windows" | "tree" | "get" | "set" | "action" | "menu" | "keys" | "type") {
                return Err(refused("untrusted"));
            }
            let reply = match op {
                "trusted" => json!({"trusted": state.trusted}),
                "apps" => {
                    let wanted: Vec<&str> =
                        params["bundles"].as_array().map(|b| b.iter().filter_map(Value::as_str).collect()).unwrap_or_default();
                    let apps: Vec<&Value> = state
                        .apps
                        .iter()
                        .filter(|app| wanted.is_empty() || app["bundle"].as_str().is_some_and(|b| wanted.contains(&b)))
                        .collect();
                    json!({"apps": apps})
                }
                "windows" => json!({"windows": state.windows, "listed": state.listed}),
                "tree" => json!({"node": state.tree}),
                "quit" => json!({}),
                _ => return Err(refused("unknown-op")),
            };
            Ok(answered(reply))
        })
    }

    fn event(&mut self, wait: Duration) -> Option<Value> {
        let (lock, condvar) = &*self.hands.inner;
        let mut state = lock.lock().unwrap_or_else(|e| e.into_inner());
        let deadline = Instant::now() + wait;
        loop {
            if let Some(event) = state.events.pop_front() {
                return Some(event);
            }
            if state.gone {
                return None;
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return None;
            }
            state = condvar.wait_timeout(state, left).unwrap_or_else(|e| e.into_inner()).0;
        }
    }

    fn alive(&self) -> bool {
        !self.hands.with(|state| state.gone)
    }

    fn quit(&mut self) {
        self.hands.with(|state| {
            if !state.gone {
                state.calls.push(("quit".into(), json!({})));
            }
            state.gone = true;
        });
    }
}

// The process tap: the helper records a process's output (Logic's) into a WAV file it writes itself, 32-bit float at
// its aggregate device's rate and the tap's channels, and stamps the file with host times (`mach_absolute_time`, the
// clock CoreMIDI's packets carry) so any host time can be placed in it (`tap.start`, `tap.status`, `tap.stop`: the
// contract's helper protocol).

/// What `tap.start` answers: the capture's format and when it started.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TapStarted {
    pub sample_rate: f64,
    pub channels: u32,
    /// The host time right after the tap's device started.
    pub host_time: u64,
}

/// What `tap.status` answers.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TapStatus {
    pub running: bool,
    /// Frames in the file so far, and their length in seconds.
    pub frames: u64,
    pub seconds: f64,
    /// Whether the capture reached its `maxSeconds` and stopped writing.
    pub truncated: bool,
}

/// Host time's unit: a tick is `numer / denom` nanoseconds (`mach_timebase_info`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Timebase {
    pub numer: u32,
    pub denom: u32,
}

impl Timebase {
    /// A number of ticks as seconds (negative ticks for a time before another).
    pub fn seconds(&self, ticks: i128) -> f64 {
        ticks as f64 * f64::from(self.numer) / f64::from(self.denom) / 1e9
    }
}

/// What `tap.stop` answers: the finished file, and how to place a host time in it.
#[derive(Debug, Clone, PartialEq)]
pub struct TapCapture {
    /// The WAV file, as `tap.start` named it.
    pub file: PathBuf,
    /// Frames in the file, and their length in seconds at `sample_rate`.
    pub frames: u64,
    pub seconds: f64,
    pub sample_rate: f64,
    pub channels: u32,
    /// The largest sample and the root mean square of every sample, linear (1.0 is full scale).
    pub peak: f64,
    pub rms: f64,
    /// `(frame, host time)`: where a buffer starts in the file and the host time Core Audio stamped it with. The first
    /// buffer, at least one every quarter second after it, one after any loss, and the last; in order.
    pub anchors: Vec<(u64, u64)>,
    pub timebase: Timebase,
    /// Frames lost on the way (the device skipped some, or the writer fell behind): 0 for a whole capture.
    pub dropped: u64,
    /// Whether the capture reached its `maxSeconds` and stopped writing there.
    pub truncated: bool,
}

impl TapCapture {
    /// The frame of the file a host time falls on, fractional (below 0 or past the end for a host time outside the
    /// capture): between the two anchors around it at their own pace when no frames were lost between them, else from
    /// the anchor before it at the sample rate. None without anchors.
    pub fn frame_at(&self, host_time: u64) -> Option<f64> {
        let first = *self.anchors.first()?;
        let after = self.anchors.partition_point(|&(_, host)| host <= host_time);
        let (frame, host) = if after == 0 { first } else { self.anchors[after - 1] };
        let offset = self.timebase.seconds(i128::from(host_time) - i128::from(host));
        if let (true, Some(&(next_frame, next_host))) = (after > 0, self.anchors.get(after)) {
            let span = self.timebase.seconds(i128::from(next_host) - i128::from(host));
            let frames = next_frame.saturating_sub(frame) as f64;
            if span > 0.0 && (frames / self.sample_rate - span).abs() < 0.001 {
                return Some(frame as f64 + frames * offset / span);
            }
        }
        Some(frame as f64 + offset * self.sample_rate)
    }
}

/// A reply that said `ok` but lacks what its op answers: refused as `bad-reply`, with the reply.
fn bad_reply(op: &str, reply: &Value, field: &str) -> HelperError {
    let mut reply = reply.clone();
    reply["detail"] = format!("the helper's {op} reply has no {field}").into();
    HelperError::Refused { code: "bad-reply".into(), reply }
}

fn field<T>(op: &str, reply: &Value, name: &str, read: impl Fn(&Value) -> Option<T>) -> Result<T, HelperError> {
    reply.get(name).and_then(read).ok_or_else(|| bad_reply(op, reply, name))
}

fn channels(op: &str, reply: &Value) -> Result<u32, HelperError> {
    field(op, reply, "channels", |v| v.as_u64().and_then(|n| u32::try_from(n).ok()))
}

/// Starts a tap on the process `pid`, writing a new WAV file at `path` (a whole path; never over a file that is
/// there), muting the process's own output while it runs when `mute`, for at most `max_seconds` (the helper's 600
/// when None). The helper's refusals come back as they are: `busy` (a tap runs), `denied` (macOS wants the System
/// Audio Recording permission), `unsupported` (before macOS 14.2), `failed` (with `step` and `detail`).
pub fn tap_start(helper: &mut dyn Helper, pid: u32, path: &Path, mute: bool, max_seconds: Option<f64>) -> Result<TapStarted, HelperError> {
    let Some(path) = path.to_str() else {
        let reply = json!({"ok": false, "error": "bad-request", "detail": "the tap's path isn't UTF-8"});
        return Err(HelperError::Refused { code: "bad-request".into(), reply });
    };
    let mut params = json!({"pid": pid, "path": path, "mute": mute});
    if let Some(seconds) = max_seconds {
        params["maxSeconds"] = seconds.into();
    }
    let reply = helper.call("tap.start", params)?;
    Ok(TapStarted {
        sample_rate: field("tap.start", &reply, "sampleRate", Value::as_f64)?,
        channels: channels("tap.start", &reply)?,
        host_time: field("tap.start", &reply, "hostTime", Value::as_u64)?,
    })
}

/// How the running tap is doing (`running` false when none runs).
pub fn tap_status(helper: &mut dyn Helper) -> Result<TapStatus, HelperError> {
    let reply = helper.call("tap.status", json!({}))?;
    Ok(TapStatus {
        running: field("tap.status", &reply, "running", Value::as_bool)?,
        frames: field("tap.status", &reply, "frames", Value::as_u64)?,
        seconds: field("tap.status", &reply, "seconds", Value::as_f64)?,
        truncated: reply.get("truncated").and_then(Value::as_bool).unwrap_or(false),
    })
}

/// Stops the running tap and finishes its file (`failed` when none runs).
pub fn tap_stop(helper: &mut dyn Helper) -> Result<TapCapture, HelperError> {
    const OP: &str = "tap.stop";
    let reply = helper.call(OP, json!({}))?;
    let anchors = field(OP, &reply, "anchors", |anchors| {
        anchors.as_array()?.iter().map(|pair| Some((pair.get(0)?.as_u64()?, pair.get(1)?.as_u64()?))).collect::<Option<Vec<_>>>()
    })?;
    let timebase = field(OP, &reply, "timebase", |timebase| {
        let part = |name: &str| timebase.get(name)?.as_u64().and_then(|n| u32::try_from(n).ok()).filter(|&n| n > 0);
        Some(Timebase { numer: part("numer")?, denom: part("denom")? })
    })?;
    Ok(TapCapture {
        file: PathBuf::from(field(OP, &reply, "file", |v| v.as_str().map(str::to_string))?),
        frames: field(OP, &reply, "frames", Value::as_u64)?,
        seconds: field(OP, &reply, "seconds", Value::as_f64)?,
        sample_rate: field(OP, &reply, "sampleRate", Value::as_f64)?,
        channels: channels(OP, &reply)?,
        peak: field(OP, &reply, "peak", Value::as_f64)?,
        rms: field(OP, &reply, "rms", Value::as_f64)?,
        anchors,
        timebase,
        dropped: field(OP, &reply, "dropped", Value::as_u64)?,
        truncated: field(OP, &reply, "truncated", Value::as_bool)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_mock_answers_its_fixtures_scripted_replies_first_and_records_the_calls() {
        let hands = MockHands::new();
        hands.set_apps(vec![json!({"bundle": "com.apple.logic10", "pid": 7}), json!({"bundle": "com.other", "pid": 8})]);
        hands.set_windows(json!([{"title": "Song - Tracks"}]), &["Song - Tracks"]);
        hands.reply("windows", Ok(json!({"windows": []})));
        let mut helper = MockHands::launch(&mut hands.clone()).unwrap();
        let trusted = helper.call("trusted", json!({})).unwrap();
        assert_eq!(
            (&trusted["trusted"], &trusted["ok"], &trusted["ms"], &trusted["id"]),
            (&json!(true), &json!(true), &json!(0), &json!(1))
        );
        let apps = helper.call("apps", json!({"bundles": ["com.apple.logic10"]})).unwrap();
        assert_eq!(apps["apps"].as_array().unwrap().len(), 1);
        assert_eq!(helper.call("windows", json!({"pid": 7})).unwrap()["windows"], json!([]));
        let windows = helper.call("windows", json!({"pid": 7})).unwrap();
        assert_eq!((&windows["windows"][0]["title"], &windows["listed"][0]), (&json!("Song - Tracks"), &json!("Song - Tracks")));
        assert!(matches!(helper.call("menu", json!({})), Err(HelperError::Refused { code, .. }) if code == "unknown-op"));
        assert_eq!(hands.calls_of("windows"), 2);
        assert_eq!(hands.calls()[0].0, "trusted");
        // Without the grant, the ops that need it are refused as the helper refuses them.
        hands.set_trusted(false);
        match helper.call("windows", json!({"pid": 7})) {
            Err(HelperError::Refused { code, reply }) => {
                assert_eq!((code.as_str(), &reply["ok"], &reply["error"]), ("untrusted", &json!(false), &json!("untrusted")))
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(helper.call("apps", json!({})).unwrap()["ok"], true, "apps needs no grant");
        hands.set_trusted(true);
        hands.emit(json!({"event": "app", "state": "launched"}));
        assert_eq!(helper.event(Duration::from_millis(10)).unwrap()["state"], "launched");
        assert_eq!(helper.event(Duration::from_millis(10)), None);
        hands.exit();
        assert!(!helper.alive());
        assert!(matches!(helper.call("trusted", json!({})), Err(HelperError::Gone(_))));
    }

    #[test]
    fn a_helper_that_exits_is_started_again_after_a_backoff_that_doubles() {
        let hands = MockHands::new();
        // The backoff starts at 200 ms and caps at 500: wide enough for a busy runner to keep its place in the margins.
        let mut helper = Supervised::new(Box::new(hands.clone())).with_backoff(Duration::from_millis(200), Duration::from_millis(500));
        assert!(helper.call("trusted", json!({})).is_ok());
        assert_eq!(hands.launches(), 1);
        hands.exit();
        // The exit is noticed on the next request, which fails; the next start waits for the backoff.
        assert!(matches!(helper.call("trusted", json!({})), Err(HelperError::Gone(_))));
        assert!(matches!(helper.call("trusted", json!({})), Err(HelperError::Gone(_))));
        assert_eq!(hands.launches(), 1);
        thread::sleep(Duration::from_millis(300));
        assert!(helper.call("trusted", json!({})).is_ok());
        assert_eq!((hands.launches(), helper.starts()), (2, 2));
        // A second exit in a row waits twice as long.
        hands.exit();
        assert!(helper.call("trusted", json!({})).is_err());
        thread::sleep(Duration::from_millis(150));
        assert!(helper.call("trusted", json!({})).is_err());
        thread::sleep(Duration::from_millis(350));
        assert!(helper.call("trusted", json!({})).is_ok());
        assert_eq!(hands.launches(), 3);
        // A start that fails is tried again too, and the events wait out the backoff instead of spinning.
        hands.exit();
        hands.fail_launches(Some("no such file"));
        assert!(helper.call("trusted", json!({})).is_err());
        let started = Instant::now();
        assert_eq!(helper.event(Duration::from_millis(30)), None);
        assert!(started.elapsed() >= Duration::from_millis(30));
        hands.fail_launches(None);
        thread::sleep(Duration::from_millis(650));
        assert!(helper.call("trusted", json!({})).is_ok());
        helper.quit();
        assert!(!helper.alive());
    }

    #[test]
    fn a_helper_that_stops_answering_is_started_again() {
        let hands = MockHands::new();
        for _ in 0..STUCK_AFTER {
            hands.reply("trusted", Err(HelperError::Timeout));
        }
        let mut helper = Supervised::new(Box::new(hands.clone())).with_backoff(Duration::from_millis(1), Duration::from_millis(1));
        for _ in 0..STUCK_AFTER {
            assert_eq!(helper.call("trusted", json!({})), Err(HelperError::Timeout));
        }
        thread::sleep(Duration::from_millis(5));
        assert!(helper.call("trusted", json!({})).is_ok());
        assert_eq!(hands.launches(), 2);
    }

    #[test]
    fn the_tap_is_started_followed_and_stopped_through_typed_answers() {
        let hands = MockHands::new();
        hands.reply("tap.start", Ok(json!({"sampleRate": 48000.0, "channels": 2, "hostTime": 1_000_000u64})));
        hands.reply("tap.status", Ok(json!({"running": true, "frames": 24000, "seconds": 0.5, "truncated": false})));
        // A 24 MHz host clock (125/3 ns a tick): a quarter second is 6,000,000 ticks and 12,000 frames at 48 kHz.
        hands.reply(
            "tap.stop",
            Ok(json!({
                "file": "/tmp/kumi-tap.wav", "frames": 36000, "seconds": 0.75, "sampleRate": 48000.0, "channels": 2,
                "peak": 0.0316, "rms": 0.0224, "timebase": {"numer": 125, "denom": 3}, "dropped": 0, "truncated": false,
                "anchors": [[0, 1_000_000u64], [12000, 7_000_000u64], [24000, 13_000_000u64], [35488, 18_744_000u64]],
            })),
        );
        let busy = HelperError::Refused { code: "busy".into(), reply: json!({"ok": false, "error": "busy"}) };
        hands.reply("tap.start", Err(busy.clone()));
        hands.reply("tap.stop", Ok(json!({"file": "/tmp/kumi-tap.wav", "frames": 10})));
        let mut helper = MockHands::launch(&mut hands.clone()).unwrap();

        let started = tap_start(helper.as_mut(), 812, Path::new("/tmp/kumi-tap.wav"), true, Some(30.0)).unwrap();
        assert_eq!(started, TapStarted { sample_rate: 48000.0, channels: 2, host_time: 1_000_000 });
        assert_eq!(
            hands.calls()[0],
            ("tap.start".to_string(), json!({"pid": 812, "path": "/tmp/kumi-tap.wav", "mute": true, "maxSeconds": 30.0}))
        );
        let status = tap_status(helper.as_mut()).unwrap();
        assert_eq!(status, TapStatus { running: true, frames: 24000, seconds: 0.5, truncated: false });
        let capture = tap_stop(helper.as_mut()).unwrap();
        assert_eq!(
            (capture.file.as_path(), capture.frames, capture.channels, capture.dropped),
            (Path::new("/tmp/kumi-tap.wav"), 36000, 2, 0)
        );
        assert_eq!((capture.anchors.len(), capture.timebase), (4, Timebase { numer: 125, denom: 3 }));
        assert!(!capture.truncated && (capture.peak - 0.0316).abs() < 1e-9);

        // A host time is placed between the anchors around it, before the first and after the last at the sample rate.
        let at = |host: u64| capture.frame_at(host).unwrap();
        assert!((at(1_000_000) - 0.0).abs() < 1e-6);
        assert!((at(4_000_000) - 6000.0).abs() < 1e-6, "{}", at(4_000_000));
        assert!((at(19_000_000) - 36000.0).abs() < 1e-6, "{}", at(19_000_000));
        assert!((at(0) + 2000.0).abs() < 1e-6, "{}", at(0));
        // Frames lost between two anchors: a host time before the next one is placed from the one before it.
        let gapped = TapCapture { anchors: vec![(0, 0), (11488, 6_000_000)], ..capture.clone() };
        assert!((gapped.frame_at(3_000_000).unwrap() - 6000.0).abs() < 1e-6);
        assert!((gapped.frame_at(6_000_000).unwrap() - 11488.0).abs() < 1e-6);
        assert_eq!(TapCapture { anchors: vec![], ..capture }.frame_at(5), None);

        // The helper's refusals come back as they are; a reply short of its fields is `bad-reply`.
        assert_eq!(tap_start(helper.as_mut(), 812, Path::new("/tmp/other.wav"), false, None), Err(busy));
        assert_eq!(hands.calls()[3].1, json!({"pid": 812, "path": "/tmp/other.wav", "mute": false}));
        match tap_stop(helper.as_mut()) {
            Err(HelperError::Refused { code, reply }) => {
                assert_eq!((code.as_str(), reply["detail"].as_str()), ("bad-reply", Some("the helper's tap.stop reply has no anchors")))
            }
            other => panic!("{other:?}"),
        }
        assert!(matches!(tap_status(helper.as_mut()), Err(HelperError::Refused { code, .. }) if code == "unknown-op"));
    }

    /// A helper made of a shell script: the real child, its pipes and threads, the reply timeout and the restart.
    #[cfg(unix)]
    mod child {
        use super::*;
        use std::os::unix::fs::PermissionsExt;

        fn script(dir: &Path, body: &str) -> PathBuf {
            let path = dir.join("kumi-logic-hands-test");
            std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
            path
        }

        #[test]
        fn requests_are_lines_with_increasing_ids_and_replies_and_events_are_sorted_apart() {
            let dir = tempfile::tempdir().unwrap();
            // Echoes each request's id back in a reply, an event before the second, and a line of junk, then exits on
            // the third request.
            let path = script(
                dir.path(),
                r#"n=0
while read line; do
  n=$((n+1))
  id=$(printf '%s' "$line" | sed 's/.*"id":\([0-9]*\).*/\1/')
  if [ "$n" = 2 ]; then echo '{"event":"app","state":"launched","pid":5}'; echo 'not json'; fi
  if [ "$n" = 3 ]; then echo '{"id":'"$id"',"ok":false,"error":"nope"}'; exit 3; fi
  echo '{"id":'"$id"',"ok":true,"echo":'"$n"'}'
  echo "line $n" >&2
done"#,
            );
            let mut helper = ChildHelper::spawn(&path).unwrap();
            assert!(helper.pid() > 0);
            assert_eq!(helper.call("trusted", json!({"prompt": false})).unwrap()["echo"], 1);
            assert_eq!(helper.call("apps", json!({})).unwrap()["echo"], 2);
            assert_eq!(helper.event(Duration::from_secs(2)).unwrap()["state"], "launched");
            match helper.call("windows", json!({"pid": 1})) {
                Err(HelperError::Refused { code, reply }) => assert_eq!((code.as_str(), &reply["id"]), ("nope", &json!(3))),
                other => panic!("{other:?}"),
            }
            let until = Instant::now() + Duration::from_secs(5);
            while helper.alive() && Instant::now() < until {
                thread::sleep(Duration::from_millis(10));
            }
            assert!(!helper.alive(), "the script exited");
            assert!(matches!(helper.call("windows", json!({})), Err(HelperError::Gone(_))));
        }

        #[test]
        fn a_reply_that_doesnt_come_in_time_is_a_timeout_and_a_late_one_is_skipped() {
            let dir = tempfile::tempdir().unwrap();
            let path = script(
                dir.path(),
                r#"read line; sleep 2.5; echo '{"id":1,"ok":true,"late":true}'
read line; echo '{"id":2,"ok":true}'
read line"#,
            );
            let mut helper = ChildHelper::spawn(&path).unwrap();
            let started = Instant::now();
            assert_eq!(helper.call("tree", json!({})), Err(HelperError::Timeout));
            assert!(started.elapsed() >= REPLY_TIMEOUT && started.elapsed() < REPLY_TIMEOUT + Duration::from_millis(500));
            let second = helper.call("get", json!({})).unwrap();
            assert_eq!((second["id"].as_u64(), second.get("late")), (Some(2), None));
            helper.quit();
            assert!(!helper.alive());
        }

        #[test]
        fn a_child_that_exits_is_started_again_by_the_supervisor() {
            let dir = tempfile::tempdir().unwrap();
            let path = script(dir.path(), r#"read line; echo '{"id":1,"ok":true,"run":"'"$$"'"}'"#);
            let mut helper =
                Supervised::new(Box::new(ChildLauncher { path })).with_backoff(Duration::from_millis(20), Duration::from_millis(50));
            let first = helper.call("trusted", json!({})).unwrap()["run"].clone();
            let until = Instant::now() + Duration::from_secs(5);
            let second = loop {
                match helper.call("trusted", json!({})) {
                    Ok(reply) => break reply["run"].clone(),
                    Err(HelperError::Gone(_)) if Instant::now() < until => thread::sleep(Duration::from_millis(20)),
                    Err(error) => panic!("{error}"),
                }
            };
            assert_ne!(first, second, "another process answered");
            assert_eq!(helper.starts(), 2);
            helper.quit();
        }
    }
}
