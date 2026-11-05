//! What an operation of `kumi serve` is and is given: its MCP tool ([`Operation`]), the call it answers ([`Call`]: the
//! arguments, the host's identity for it, its cancel signal and its progress) and the runtime it runs in ([`Ctx`]).
//! Every stream's operations are written against these; each module adds its own to [`super::registry`].

use super::Args;
use super::connection::Link;
use super::jobs::{JobHandle, Jobs};
use super::snapshots::Snapshots;
use futures::future::LocalBoxFuture;
use kumi_common::abort::Signal;
use kumi_types::daw::DawId;
use kumi_types::system::Env;
use serde::Serialize;
use serde_json::{Map, Value, json};
use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::rc::{Rc, Weak};
use tokio::sync::mpsc::UnboundedSender;

pub type JsonObject = Map<String, Value>;
pub type OpFuture<'a> = LocalBoxFuture<'a, Result<Value, OpError>>;

/// One MCP tool of `kumi serve`: a C1 tool without its `kumi_` prefix.
pub struct Operation {
    pub name: &'static str,
    pub description: &'static str,
    /// The arguments' JSON schema (C1's, without `_host`): a call that doesn't fit is refused before `run`.
    pub schema: fn() -> Value,
    /// What a call with these arguments does: `listen` reads a file but plays the Set for a track. A call that writes,
    /// plays or imports runs only under the DAW endpoint's write lease; until the runtime has the lease, the adapter
    /// refuses such a call (`no_lease`).
    pub effect: fn(&JsonObject) -> Effect,
    pub run: for<'a> fn(&'a Ctx, Call) -> OpFuture<'a>,
}

/// What a call does, by C1's effect classes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Effect {
    /// Reads the DAW, the runtime's state or an approved file.
    Read,
    /// Starts or ends the runtime's connection to the DAW; changes nothing in it.
    Connect,
    /// Changes the project.
    Write,
    /// Plays or captures the project (a track listen), changing settings while it does.
    Playback,
    /// Places a file in the project.
    Import,
    /// Reads a folder the producer approved.
    FolderRead,
    /// Installs Kumi's part in the DAW.
    Setup,
    /// Sends a file to a service.
    Upload,
    /// Spends the producer's credits.
    Paid,
}

impl Effect {
    /// Whether a call with this effect needs the DAW endpoint's write lease.
    pub fn writes(self) -> bool {
        matches!(self, Effect::Write | Effect::Playback | Effect::Import)
    }
}

/// [`Operation::effect`] for an operation that only ever reads.
pub fn reads(_: &JsonObject) -> Effect {
    Effect::Read
}

/// The schema of an epoch a call names (`expected_epoch`): a whole number, or its decimal digits as a string, which is
/// how the host sends it (CC-2).
pub fn epoch_schema() -> Value {
    json!({"type": ["integer", "string"], "minimum": 0, "pattern": "^[0-9]{1,20}$"})
}

/// The epoch a call names in `field`, in either form; None when it's missing or neither.
pub fn epoch_of(args: &JsonObject, field: &str) -> Option<u64> {
    match args.get(field)? {
        Value::Number(number) => number.as_u64(),
        Value::String(digits) => digits.parse().ok(),
        _ => None,
    }
}

/// Who asked, as the host says it (`_host` in the arguments, CC-5): the profile, its session, the turn's task, the
/// tool call and the request. Identity for deduplication, ownership and the snapshots' envelope; never permission.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Host {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub profile_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub task_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
    /// What the host gives the call privately, after its approval: never written or said anywhere.
    #[serde(skip)]
    pub private: Private,
}

/// The arguments the host adds to a call after its approval, for the call alone, each an object, and only with
/// `_host`: CC-13's `_mirelo` (a service's key, the producer's acknowledgment and the credits reserved) and CC-2 A2's
/// `_listen` (the approved track listen's `estimated_seconds`, `prepare_seconds` and `deadline_seconds`, which the
/// listen checks itself).
///
/// W2's plugin handler is their only writer: it adds them after its approval, and strips every `_`-prefixed key the
/// model wrote before then, so one that reaches the runtime is the host's.
pub const PRIVATE_ARGS: [&str; 2] = ["_mirelo", "_listen"];

/// The fields of [`PRIVATE_ARGS`] that are secrets, as (argument, field): a job's checkpoint never holds one
/// ([`JobHandle::checkpoint`]). The rest (the acknowledgment, the reservation) may be kept: CC-13 checkpoints the
/// reservation.
pub const SECRET_FIELDS: [(&str, &str); 1] = [("_mirelo", "api_key")];

/// The most bytes one of [`PRIVATE_ARGS`] takes, as JSON.
pub const PRIVATE_MOST: usize = 4096;

/// What the host passes a call privately ([`PRIVATE_ARGS`], by name), taken out of its arguments before anything
/// reads them: so never in what the call asked (its digest and deduplication), its in-flight record, HISTORY, a
/// snapshot, a log or an error. Its `Debug` shows only how many there are.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct Private(Map<String, Value>);

impl From<Map<String, Value>> for Private {
    fn from(arguments: Map<String, Value>) -> Private {
        Private(arguments)
    }
}

impl Private {
    /// The private argument `name` (`"_mirelo"`), if the host gave it.
    pub fn get(&self, name: &str) -> Option<&Value> {
        self.0.get(name)
    }

    /// The secrets in it ([`SECRET_FIELDS`]), each non-empty.
    pub fn secrets(&self) -> Vec<String> {
        SECRET_FIELDS
            .iter()
            .filter_map(|(name, field)| self.0.get(*name)?.get(*field)?.as_str())
            .filter(|secret| !secret.is_empty())
            .map(str::to_string)
            .collect()
    }
}

tokio::task_local! {
    /// The private arguments of the call whose operation is running (the adapter sets it around each run): a job the
    /// call starts keeps their secrets, so its checkpoints are refused any that hold one.
    pub static CALL_PRIVATE: Private;
}

impl std::fmt::Debug for Private {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Private({} argument(s))", self.0.len())
    }
}

impl Host {
    /// `_host` as the host sends it: an object of these five strings, any of them left out. A blank one (`""`, spaces)
    /// is left out too: a host that has no id for a call sends no identity, never one shared by every such call.
    pub fn from_value(value: &Value) -> Result<Host, String> {
        let Value::Object(fields) = value else { return Err("_host is an object".into()) };
        let mut host = Host::default();
        for (key, field) in fields {
            let slot = match key.as_str() {
                "profile_id" => &mut host.profile_id,
                "session_id" => &mut host.session_id,
                "tool_call_id" => &mut host.tool_call_id,
                "request_id" => &mut host.request_id,
                "task_id" => &mut host.task_id,
                other => return Err(format!("_host has no \"{other}\"")),
            };
            match field {
                Value::String(text) if text.trim().is_empty() => {}
                Value::String(text) if text.len() <= 256 => *slot = Some(text.clone()),
                Value::Null => {}
                _ => return Err(format!("_host.{key} is a string of at most 256 bytes")),
            }
        }
        Ok(host)
    }
}

/// A message the runtime sends the host.
#[derive(Debug, Clone, PartialEq)]
pub enum Out {
    Message(Value),
    /// The runtime is done: what was sent before goes out, then the output closes.
    Close,
}

pub type Outbox = UnboundedSender<Out>;

/// A call's progress, for long work: `notifications/progress` when the host asked for it (a `progressToken`), nothing
/// otherwise, and nothing once the call has answered or been cancelled (MCP's rule), whoever still holds a copy.
#[derive(Debug, Clone)]
pub struct Progress {
    to: Option<(Value, Outbox)>,
    ended: Rc<Cell<bool>>,
}

impl Progress {
    pub fn new(token: Option<Value>, outbox: Outbox) -> Progress {
        Progress { to: token.map(|token| (token, outbox)), ended: Rc::new(Cell::new(false)) }
    }

    /// Progress no one asked for: reported nowhere.
    pub fn none() -> Progress {
        Progress { to: None, ended: Rc::new(Cell::new(true)) }
    }

    /// `done` of `total` (when known), and what's under way.
    pub fn report(&self, done: f64, total: Option<f64>, message: Option<&str>) {
        let Some((token, outbox)) = &self.to else { return };
        if self.ended.get() {
            return;
        }
        let mut params = json!({"progressToken": token, "progress": done});
        if let Some(total) = total {
            params["total"] = json!(total);
        }
        if let Some(message) = message {
            params["message"] = json!(message);
        }
        let _ = outbox.send(Out::Message(json!({"jsonrpc": "2.0", "method": "notifications/progress", "params": params})));
    }

    /// The call answered or was cancelled: no more progress from it, or from any copy of it.
    pub(crate) fn end(&self) {
        self.ended.set(true);
    }
}

/// One call to an operation.
pub struct Call {
    /// The arguments, already checked against the operation's schema, `_host` taken out.
    pub args: JsonObject,
    pub host: Host,
    /// Fires when the host cancels the call (`notifications/cancelled`) or its input ends. An operation stops at it,
    /// but settles first: puts back what it changed for the while, deletes its scratch, and records `unsure` when it
    /// can't tell what happened.
    pub signal: Signal,
    pub progress: Progress,
}

/// Why a call didn't run: the call never reached the DAW. What a call that ran did, refused steps and `unsure` writes
/// included, is its result, not one of these.
#[derive(Debug, Clone, PartialEq)]
pub enum OpError {
    /// The arguments don't fit.
    Invalid { reason: String },
    /// There's no connection to the DAW yet: `connect` first.
    NotConnected { reason: String },
    /// The runtime won't: `code` says why (`stale`, `leased`, `unsupported`…), `owner` who holds the DAW when it's held.
    Refused { code: &'static str, reason: String, owner: Option<Value> },
    /// It failed before changing anything.
    Failed { reason: String },
}

impl OpError {
    pub fn code(&self) -> &str {
        match self {
            OpError::Invalid { .. } => "invalid",
            OpError::NotConnected { .. } => "not_connected",
            OpError::Refused { code, .. } => code,
            OpError::Failed { .. } => "failed",
        }
    }

    pub fn reason(&self) -> &str {
        match self {
            OpError::Invalid { reason } | OpError::NotConnected { reason } | OpError::Failed { reason } => reason,
            OpError::Refused { reason, .. } => reason,
        }
    }

    /// The error as the call's result says it: `{"error": {"code", "reason", "owner"?}}`.
    pub fn to_value(&self) -> Value {
        let mut error = json!({"code": self.code(), "reason": self.reason()});
        if let OpError::Refused { owner: Some(owner), .. } = self {
            error["owner"] = owner.clone();
        }
        json!({"error": error})
    }
}

/// The folders the runtime works in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Paths {
    /// Everything the runtime writes: snapshots, HISTORY, jobs, artifacts, scratch.
    pub state_dir: PathBuf,
    /// The Kumi home the DAW bridge's transport files are in.
    pub kumi_home: PathBuf,
}

/// The programs optional features may run, as setup selected them. The runtime fetches nothing: a feature that needs a
/// program not here is unavailable, and says so.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Media {
    pub tools: BTreeMap<String, PathBuf>,
}

impl Media {
    /// Always: in `kumi serve`, only what's installed and given is used.
    pub fn installed_only(&self) -> bool {
        true
    }

    pub fn tool(&self, name: &str) -> Option<&Path> {
        self.tools.get(name).map(PathBuf::as_path)
    }
}

/// A folder of one call's own for its temporary files, removed with all that's in it when this drops. A file that must
/// outlast the call is moved out first (the artifact store promotes it).
#[derive(Debug)]
pub struct Scratch {
    path: PathBuf,
}

impl Scratch {
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Kept where it is, not removed: for what the next start reads (an unsettled call's record, R4).
    pub fn keep(self) {
        std::mem::forget(self);
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        if let Err(error) = std::fs::remove_dir_all(&self.path) {
            if error.kind() != std::io::ErrorKind::NotFound {
                super::say(&format!("kumi serve: scratch {} not removed: {error}", self.path.display()));
            }
        }
    }
}

/// The runtime every operation runs in. One per process, shared by its calls (on one thread).
pub struct Ctx {
    /// Itself, for work that outlasts a call (a job).
    me: Weak<Ctx>,
    daw: DawId,
    paths: Paths,
    env: Env,
    library_roots: RefCell<Vec<PathBuf>>,
    media_roots: Vec<PathBuf>,
    private_roots: ProtectedRoots,
    media: Media,
    artifacts: super::artifacts::Artifacts,
    maps: super::map::MapStore,
    library: super::library::LibraryState,
    generation: super::generate::Generation,
    snapshots: Snapshots,
    jobs: Jobs,
    shutdown: Signal,
    /// The operations the runtime offers (`status`'s capabilities).
    offered: Cell<&'static [&'static Operation]>,
    scratches: Cell<u64>,
    /// When the runtime started (ms).
    started: i64,
    /// The DAW's stand-ins a test gives (as the app's `run` takes them); none otherwise.
    answers: kumi_daw::io::Answers,
    link: Link,
    lease: super::lease::Lease,
    history: super::history::History,
    /// The calls' own settle bounds (a track listen's render and restoration), each while its guard lives, and the
    /// last guard's number.
    settles: Rc<RefCell<Vec<(u64, std::time::Duration)>>>,
    settle_ids: Cell<u64>,
    /// When a stop that waits on a call's own bound gives up (ms), while it waits.
    settling_until: Cell<Option<i64>>,
}

/// The most a stop waits for a call's own settle bound, in seconds: CC-2's largest approved deadline. #989's host waits
/// 390 s for a stopping runtime, which this, the DAW's close and the exit fit in.
pub const SETTLE_MOST_SECONDS: u16 = 360;
pub const SETTLE_MOST: std::time::Duration = std::time::Duration::from_secs(SETTLE_MOST_SECONDS as u64);

/// A call's own bound for settling when the runtime stops (a track listen's render, then the DAW put back): while it
/// lives, a stop waits up to the largest live bound for the calls to finish, not just the runtime's own few seconds.
/// The bound counts from the stop's start, so it bounds the whole stop.
pub struct Settling {
    settles: Rc<RefCell<Vec<(u64, std::time::Duration)>>>,
    id: u64,
}

impl Drop for Settling {
    fn drop(&mut self) {
        self.settles.borrow_mut().retain(|(id, _)| *id != self.id);
    }
}

impl Ctx {
    /// The runtime for these arguments: its state folder made when it isn't there and the owner's alone either way,
    /// and resolved on disk, so every path under it (scratch included) compares as the library's roots do. `answers`
    /// are the DAW's stand-ins a test gives; the runtime itself gives none.
    pub fn new(args: Args, env: Env, answers: kumi_daw::io::Answers) -> Result<Rc<Ctx>, String> {
        let unmade = |error: std::io::Error| format!("can't make {}: {error}", args.state_dir.display());
        make_private_state_root(&args.state_dir).map_err(unmade)?;
        let state_dir = std::fs::canonicalize(&args.state_dir).map_err(unmade)?;
        let snapshots = Snapshots::new(state_dir.clone());
        let jobs = Jobs::new(&snapshots, &state_dir);
        let started = kumi_common::time::now_ms();
        // This runtime's own, among every runtime's: HISTORY ids and the epochs it gives out start from it.
        let generation = format!("{:x}", started.max(0));
        let history = super::history::History::new(&state_dir, generation);
        let lease = super::lease::Lease::new(&args.kumi_home, args.daw.as_str());
        let artifacts = super::artifacts::Artifacts::new(&state_dir).map_err(|error| error.reason().to_owned())?;
        let maps = super::map::MapStore::new(&state_dir).map_err(|error| error.reason().to_owned())?;
        let private_roots = protected_roots(&args, &state_dir)?;
        let library_roots = granted("library", args.library_roots, &state_dir, &private_roots);
        let media_roots = media_granted(args.media_roots, &state_dir, &private_roots);
        let ctx = Rc::new_cyclic(|me| Ctx {
            me: me.clone(),
            daw: args.daw,
            snapshots,
            jobs,
            artifacts,
            maps,
            library: super::library::LibraryState::default(),
            generation: super::generate::Generation::default(),
            paths: Paths { state_dir, kumi_home: args.kumi_home },
            env,
            library_roots: RefCell::new(library_roots),
            media_roots,
            private_roots,
            media: Media { tools: args.tools },
            shutdown: Signal::new(),
            offered: Cell::new(super::registry::OPERATIONS),
            scratches: Cell::new(0),
            settles: Rc::new(RefCell::new(Vec::new())),
            settle_ids: Cell::new(0),
            settling_until: Cell::new(None),
            started,
            answers,
            link: Link::new(started.max(0) as u64),
            lease,
            history,
        });
        ctx.project_library_roots();
        ctx.recover_unsettled();
        Ok(ctx)
    }

    /// Each call an earlier runtime didn't see settle (R4) becomes an audit row in HISTORY (W1-b), in that runtime's
    /// generation, and the runtime's `unsure` (R6) until the next look. A record left half written was a call that
    /// never ran: its row says so, and nothing is unsure. Each record goes once HISTORY has it on disk.
    fn recover_unsettled(&self) {
        let left = super::inflight::left_over(&self.paths.state_dir);
        if left.is_empty() {
            return;
        }
        for (record, _) in &left {
            let op = record["op"].as_str().unwrap_or("a call");
            let title = record["title"].as_str().map_or_else(|| format!("{op} (interrupted)"), str::to_string);
            let identity = super::history::CallIdentity {
                profile_id: record["profile_id"].as_str(),
                session_id: record["session_id"].as_str(),
                tool_call_id: record["tool_call_id"].as_str(),
                digest: record["digest"].as_str().unwrap_or_default(),
            };
            let project = record["project_key"].as_str().unwrap_or_default();
            let generation = record["generation"].as_str().unwrap_or(self.history.generation()).to_string();
            if record["unread"] == true {
                let reason = "Kumi's runtime stopped as this call was about to run: it was interrupted before it ran.";
                self.history.audit_in(&generation, &title, project, reason, identity, &self.snapshots);
                continue;
            }
            let reason = "Kumi's runtime stopped while this was running, so the DAW may have changed: look at the project \
                          before another change.";
            self.history.audit_in(&generation, &title, project, reason, identity, &self.snapshots);
            self.link.note_unsure(op, reason, "interrupted");
        }
        self.snapshots.flush();
        super::inflight::clear(&left.into_iter().map(|(_, folder)| folder).collect::<Vec<_>>());
    }

    pub fn daw(&self) -> DawId {
        self.daw
    }

    pub fn paths(&self) -> &Paths {
        &self.paths
    }

    /// The environment the DAW code reads (KUMI_HOME is `--kumi-home`).
    pub fn env(&self) -> &Env {
        &self.env
    }

    /// The sample folders the producer approved: the only ones the library reads. `--library-root` is the whole grant
    /// at each start (a folder taken out of the host's settings is gone); folders approved while it runs join them.
    /// Each as it was on disk when granted (links resolved then, never again), so a check against them is a check
    /// against what the producer approved.
    pub fn library_roots(&self) -> Vec<PathBuf> {
        self.library_roots.borrow().clone()
    }

    /// Add an already checked canonical folder. The library operation validates it off the runtime thread;
    /// re-resolving it here could follow a link substituted after that validation. Saved projections grant nothing.
    pub(crate) fn add_checked_library_root(&self, root: PathBuf) -> Result<PathBuf, OpError> {
        if !safe_grant(&root, &self.paths.state_dir, &self.private_roots, false) {
            return Err(OpError::Refused {
                code: "unsafe_folder",
                reason: "Choose a dedicated sample folder, not a filesystem root, home, Hermes profile or runtime state folder.".into(),
                owner: None,
            });
        }
        let added = {
            let mut roots = self.library_roots.borrow_mut();
            let new = !roots.contains(&root);
            if new {
                roots.push(root.clone());
            }
            new
        };
        if added {
            self.project_library_roots();
        }
        Ok(root)
    }

    /// Currently existing media roots. Missing roots retain their frozen ancestor and lexical suffix, not permission
    /// to follow a later symlink or junction. The host may create its generated-media folder after this runtime starts.
    pub fn media_roots(&self) -> Vec<PathBuf> {
        self.media_roots
            .iter()
            .filter_map(|root| {
                let found = root.to_str().and_then(kumi_library::sources::canonical_local_path)?;
                (found == *root && found.is_dir() && safe_grant(&found, &self.paths.state_dir, &self.private_roots, true)).then_some(found)
            })
            .collect()
    }

    pub fn media(&self) -> &Media {
        &self.media
    }

    pub fn artifacts(&self) -> &super::artifacts::Artifacts {
        &self.artifacts
    }

    pub fn maps(&self) -> &super::map::MapStore {
        &self.maps
    }

    pub(crate) fn library_state(&self) -> &super::library::LibraryState {
        &self.library
    }

    pub(crate) fn generation(&self) -> &super::generate::Generation {
        &self.generation
    }

    pub fn approved(&self, path: &Path) -> Result<PathBuf, OpError> {
        let media = self.media_roots.iter().filter(|root| safe_grant(root, &self.paths.state_dir, &self.private_roots, true));
        super::artifacts::approved(path, &self.library_roots.borrow(), media, &self.artifacts)
    }

    pub fn snapshots(&self) -> &Snapshots {
        &self.snapshots
    }

    pub fn jobs(&self) -> &Jobs {
        &self.jobs
    }

    /// Starts `work` as a background job of `kind` (`scan`, say): its id at once, for the call to answer with. `work`
    /// gets the runtime and the job's handle (its signal fires when the runtime stops; its progress goes to
    /// `jobs/<id>.json`); what it returns is the job's result. Refused `busy` while [`super::jobs::RUNNING`] jobs run.
    /// The job keeps the secrets of the call it's started in ([`CALL_PRIVATE`]), so its checkpoints never hold them.
    pub fn start_job<W, F>(&self, kind: &'static str, work: W) -> Result<String, OpError>
    where
        W: FnOnce(Rc<Ctx>, JobHandle) -> F + 'static,
        F: Future<Output = Result<Value, OpError>> + 'static,
    {
        let ctx = self.me.upgrade().ok_or_else(|| OpError::Failed { reason: "the runtime is stopping".into() })?;
        let secrets = CALL_PRIVATE.try_with(Private::secrets).unwrap_or_default();
        self.jobs.start(ctx, kind, secrets, work)
    }

    /// Runs `work` again as the job `id` of `kind` ([`super::jobs::Jobs::resume`]), with the calling call's secrets kept
    /// as [`Ctx::start_job`] keeps them: true when it runs, false when it was running already.
    pub fn resume_job<W, F>(&self, id: &str, kind: &'static str, work: W) -> Result<bool, OpError>
    where
        W: FnOnce(Rc<Ctx>, JobHandle) -> F + 'static,
        F: Future<Output = Result<Value, OpError>> + 'static,
    {
        let ctx = self.me.upgrade().ok_or_else(|| OpError::Failed { reason: "the runtime is stopping".into() })?;
        let secrets = CALL_PRIVATE.try_with(Private::secrets).unwrap_or_default();
        self.jobs.resume(ctx, id, kind, secrets, work)
    }

    /// A scratch folder for one call of `op`: `<state>/scratch/<op>/<id>/`, the id the runtime's own (never anything
    /// the host sent), made the owner's alone; removed when the returned [`Scratch`] drops.
    pub fn scratch(&self, op: &str) -> Result<Scratch, OpError> {
        let named = !op.is_empty() && op.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_');
        if !named {
            return Err(OpError::Failed { reason: format!("no scratch for \"{op}\"") });
        }
        let n = self.scratches.get() + 1;
        self.scratches.set(n);
        let path = self.paths.state_dir.join("scratch").join(op).join(format!("{}-{n}", kumi_common::time::now_ms()));
        #[cfg(windows)]
        let made = (|| {
            // These are exact runtime-owned targets, not ancestors supplied by the host. An old scratch tree may
            // need its own ACL tightened too; the per-call leaf must never adopt a collision that Drop would delete.
            make_private_dir(&self.paths.state_dir.join("scratch"))?;
            make_private_dir(path.parent().unwrap())?;
            make_private_windows_dir(&path, WindowsDirectoryPolicy::Fresh)
        })();
        #[cfg(not(windows))]
        let made = make_private_dir(&path);
        made.map_err(|error| OpError::Failed { reason: format!("no scratch folder: {error}") })?;
        Ok(Scratch { path })
    }

    /// A guard holding a stop to `deadline_seconds` from the stop's start while it lives: take it just before the work
    /// whose cleanup must finish (a render and the DAW put back), and drop it once that's done.
    ///
    /// - The bound is the call's approved deadline (CC-2's `deadline_seconds`, 1 to [`SETTLE_MOST_SECONDS`]), never
    ///   a number the model gave: anything else is refused.
    /// - The cleanup it covers must run on a signal the stop doesn't cancel. A stop cancels `ctx.shutdown()` first, and
    ///   every call's `signal` has it in it, so a restoration awaited on `call.signal` would be cut off as it began.
    pub fn settling(&self, deadline_seconds: u16) -> Result<Settling, OpError> {
        if !(1..=SETTLE_MOST_SECONDS).contains(&deadline_seconds) {
            let reason = format!("A call's settle bound is 1 to {SETTLE_MOST_SECONDS} seconds, not {deadline_seconds}.");
            return Err(OpError::Failed { reason });
        }
        let id = self.settle_ids.get() + 1;
        self.settle_ids.set(id);
        self.settles.borrow_mut().push((id, std::time::Duration::from_secs(deadline_seconds.into())));
        Ok(Settling { settles: self.settles.clone(), id })
    }

    /// The largest bound a live guard holds a stop to, if any.
    pub fn settle_bound(&self) -> Option<std::time::Duration> {
        self.settles.borrow().iter().map(|(_, bound)| *bound).max()
    }

    /// When a stop waiting on a call's bound gives up (ms), while it waits: `status` says it.
    pub fn settling_until(&self) -> Option<i64> {
        self.settling_until.get()
    }

    pub(super) fn set_settling_until(&self, until: Option<i64>) {
        self.settling_until.set(until);
    }

    /// When the runtime started (ms since the epoch).
    pub fn started(&self) -> i64 {
        self.started
    }

    /// The runtime itself, for what outlasts a call (a callback the DAW's integration keeps).
    pub fn handle(&self) -> Weak<Ctx> {
        self.me.clone()
    }

    pub fn answers(&self) -> &kumi_daw::io::Answers {
        &self.answers
    }

    /// The connection to the DAW and the project last seen (none before `connect`).
    pub fn link(&self) -> &Link {
        &self.link
    }

    /// The DAW endpoint's write lease: a call that writes, plays or imports holds it while it runs.
    pub fn lease(&self) -> &super::lease::Lease {
        &self.lease
    }

    /// The runtime's HISTORY.
    pub fn history(&self) -> &super::history::History {
        &self.history
    }

    /// The connection to the DAW: `connect` first.
    pub fn connection(&self) -> Result<Rc<super::connection::Connection>, OpError> {
        self.link.connection().ok_or_else(|| OpError::NotConnected { reason: "Kumi isn't connected to the DAW: connect first.".into() })
    }

    /// The operations the runtime offers.
    pub fn offered(&self) -> &'static [&'static Operation] {
        self.offered.get()
    }

    pub(crate) fn offer(&self, operations: &'static [&'static Operation]) {
        self.offered.set(operations);
    }

    /// Fires when the host's input ends or the runtime is told to stop: every call in flight stops and settles, then
    /// the runtime exits.
    pub fn shutdown(&self) -> &Signal {
        &self.shutdown
    }

    fn project_library_roots(&self) {
        let roots: Vec<String> = self.library_roots.borrow().iter().map(|root| root.display().to_string()).collect();
        if let Err(error) = self.snapshots.write("library-roots", &json!({"roots": roots})) {
            super::say(&format!("kumi serve: library-roots.json not written: {error}"));
        }
    }
}

/// The folders granted at start, each resolved on disk now. One that isn't a folder there now grants nothing this run.
fn granted(kind: &str, roots: Vec<PathBuf>, state: &Path, private: &ProtectedRoots) -> Vec<PathBuf> {
    let mut granted: Vec<PathBuf> = Vec::with_capacity(roots.len());
    for root in roots {
        let found = pending_media_root(&root).filter(|root| root.is_dir() && safe_grant(root, state, private, false));
        match found {
            Some(found) if !granted.contains(&found) => granted.push(found),
            Some(_) => {}
            None => super::say(&format!("kumi serve: a {kind} root is not a safe local folder; left out")),
        }
    }
    granted
}

fn media_granted(roots: Vec<PathBuf>, state: &Path, private: &ProtectedRoots) -> Vec<PathBuf> {
    let mut granted = Vec::with_capacity(roots.len());
    for root in roots {
        match pending_media_root(&root).filter(|root| safe_grant(root, state, private, true)) {
            Some(found) if !granted.contains(&found) => granted.push(found),
            Some(_) => {}
            None => super::say("kumi serve: a media root is not a safe local or pending folder; left out"),
        }
    }
    granted
}

struct ProtectedRoots {
    ancestors: Vec<PathBuf>,
    subtrees: Vec<PathBuf>,
    hermes: Vec<PathBuf>,
    generated_audio: Option<PathBuf>,
}

fn safe_grant(root: &Path, state: &Path, private: &ProtectedRoots, media: bool) -> bool {
    let Some(parent) = root.parent() else { return false };
    let overlaps = |excluded: &Path| excluded.starts_with(root) || root.starts_with(excluded);
    if overlaps(state)
        || private.ancestors.iter().any(|excluded| excluded.starts_with(root))
        || private.subtrees.iter().any(|excluded| overlaps(excluded))
    {
        return false;
    }
    // Only the host can grant this exact active-profile folder as media. A library grant, its parent,
    // a sibling or another profile never receives this exception.
    let generated_audio = media && private.generated_audio.as_deref() == Some(root);
    if !generated_audio && private.hermes.iter().any(|excluded| overlaps(excluded)) {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let metadata = match std::fs::symlink_metadata(root) {
            Ok(metadata) if metadata.is_dir() => Some(metadata),
            Ok(_) => return false,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(_) => return false,
        };
        if let Some(metadata) = metadata {
            let Ok(parent_metadata) = std::fs::metadata(parent) else { return false };
            if metadata.dev() != parent_metadata.dev() {
                return false; // A mounted disk/volume's root is too broad, even if it holds no home.
            }
            // APFS firmlinks can preserve a different path spelling. Check protected ancestors by identity,
            // including a volume alias exposing the protected tree at its root-relative path.
            for protected in
                private.ancestors.iter().chain(&private.subtrees).chain(&private.hermes).map(PathBuf::as_path).chain(std::iter::once(state))
            {
                for ancestor in protected.ancestors() {
                    match std::fs::metadata(ancestor) {
                        Ok(found) if found.dev() == metadata.dev() && found.ino() == metadata.ino() => return false,
                        Ok(found) if ancestor == protected => {
                            if let Ok(relative) = protected.strip_prefix("/") {
                                let below = root.join(relative);
                                if let Some(below) = below.to_str().and_then(kumi_library::sources::canonical_local_path) {
                                    if std::fs::metadata(below).is_ok_and(|alias| alias.dev() == found.dev() && alias.ino() == found.ino())
                                    {
                                        return false;
                                    }
                                }
                            }
                        }
                        Ok(_) => {}
                        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                        Err(_) => return false,
                    }
                }
            }
        }
        // Refuse descendants reached through those aliases even when the final folder is still missing.
        // Home descendants remain valid sample folders.
        for ancestor in root.ancestors() {
            let found = match std::fs::symlink_metadata(ancestor) {
                Ok(found) => found,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(_) => return false,
            };
            for excluded in private
                .subtrees
                .iter()
                .chain(private.hermes.iter().filter(|_| !generated_audio))
                .map(PathBuf::as_path)
                .chain(std::iter::once(state))
            {
                match std::fs::metadata(excluded) {
                    Ok(blocked) if found.dev() == blocked.dev() && found.ino() == blocked.ino() => return false,
                    Ok(_) => {}
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    Err(_) => return false,
                }
            }
        }
    }
    #[cfg(not(unix))]
    let _ = parent;
    true
}

fn protected_roots(args: &Args, state: &Path) -> Result<ProtectedRoots, String> {
    let freeze = |path: &Path| {
        pending_media_root(path)
            .ok_or_else(|| "private folders must be local absolute paths without traversal or redirected components".to_owned())
    };
    let mut ancestors = Vec::new();
    for name in ["HOME", "USERPROFILE"] {
        if let Some(home) = std::env::var_os(name).and_then(|home| pending_media_root(Path::new(&home))) {
            if !ancestors.contains(&home) {
                ancestors.push(home);
            }
        }
    }
    let default = ancestors.first().map(|home| home.join(".hermes")).map(|home| freeze(&home)).transpose()?;
    let active = args.hermes_home.as_deref().map(freeze).transpose()?.or_else(|| default.clone());
    let root = args.hermes_root.as_deref().map(freeze).transpose()?.or_else(|| active.clone());
    let mut hermes = Vec::new();
    for path in default.into_iter().chain(root).chain(active.iter().cloned()) {
        if let Some(parent) = path.parent() {
            if !ancestors.iter().any(|path| path == parent) {
                ancestors.push(parent.to_path_buf());
            }
        }
        if !hermes.contains(&path) {
            hermes.push(path);
        }
    }
    let mut subtrees = vec![freeze(&args.kumi_home)?];
    if let Some(parent) = state.parent().filter(|parent| parent.file_name().is_some_and(|name| name == "kumi")) {
        subtrees.push(parent.to_path_buf());
    }
    Ok(ProtectedRoots { ancestors, subtrees, hermes, generated_audio: active.map(|home| home.join("cache/generated/audio")) })
}

/// Resolve the deepest existing ancestor once, then retain the missing suffix literally. A later resolution of a
/// source checks every component without following links, so creating a folder works but retargeting it cannot grant
/// its new destination. Explicit startup aliases such as macOS /var are frozen to their canonical local ancestor.
fn pending_media_root(root: &Path) -> Option<PathBuf> {
    use kumi_library::sources::{canonical_local_path, local_absolute_path};
    if !root.to_str().is_some_and(local_absolute_path) {
        return None;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        // Reject redirection before even looking below an existing component: an apparent local drive path can
        // otherwise cause network authentication through a junction.
        let mut prefix = PathBuf::new();
        for component in root.components() {
            prefix.push(component);
            if matches!(component, std::path::Component::Prefix(_)) {
                continue;
            }
            match std::fs::symlink_metadata(&prefix) {
                Ok(meta) if meta.is_dir() && !meta.file_type().is_symlink() && meta.file_attributes() & 0x400 == 0 => {}
                Ok(_) => return None,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => break,
                Err(_) => return None,
            }
        }
    }
    for ancestor in root.ancestors() {
        match std::fs::symlink_metadata(ancestor) {
            Ok(_) => {
                let frozen = std::fs::canonicalize(ancestor).ok()?;
                if !frozen.is_dir() || canonical_local_path(frozen.to_str()?)? != frozen {
                    return None;
                }
                let suffix = root.strip_prefix(ancestor).ok()?;
                return Some(if suffix.as_os_str().is_empty() { frozen } else { frozen.join(suffix) });
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => return None,
        }
    }
    None
}

/// Startup alone authorizes tightening the explicitly selected runtime root, which the host may have created
/// for runtime.lock. This is not authority to take ownership or repair any other directory.
fn make_private_state_root(path: &Path) -> std::io::Result<()> {
    #[cfg(windows)]
    {
        make_private_windows_dir(path, WindowsDirectoryPolicy::BootstrapStateRoot).map_err(|error| {
            std::io::Error::other(format!(
                "cannot secure the runtime state root (it must belong to the current account's token user or default owner, and contain no reparse components): {error}"
            ))
        })
    }
    #[cfg(not(windows))]
    {
        make_private_dir(path)
    }
}

#[cfg(windows)]
#[derive(Clone, Copy)]
enum WindowsDirectoryPolicy {
    BootstrapStateRoot,
    ExistingUnderPrivateParent,
    Fresh,
}

/// A folder only its owner can open, made with its parents when it isn't there.
/// On Windows every new component is born private. An existing private target needs no repair; a nonprivate target
/// may be tightened only beneath a pinned private parent. Links, foreign owners and nonlocal paths are refused.
pub(crate) fn make_private_dir(path: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
        if !path.is_dir() {
            std::fs::DirBuilder::new().recursive(true).mode(0o700).create(path)?;
        }
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))
    }
    #[cfg(windows)]
    {
        make_private_windows_dir(path, WindowsDirectoryPolicy::ExistingUnderPrivateParent)
    }
    #[cfg(not(any(unix, windows)))]
    {
        std::fs::create_dir_all(path)
    }
}

/// Pins each directory against replacement and opens the next component relative to that handle, refusing reparses.
/// Ancestors allow ordinary child writes (including snapshot renames); only the exact ACL target excludes writers.
#[cfg(windows)]
fn make_private_windows_dir(path: &Path, policy: WindowsDirectoryPolicy) -> std::io::Result<()> {
    use daw_bridge::lifecycle::native_acl;
    use std::{
        fs::OpenOptions,
        os::windows::{
            ffi::OsStrExt,
            fs::{MetadataExt, OpenOptionsExt},
        },
        path::{Component, Prefix},
    };
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_ATTRIBUTE_REPARSE_POINT, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_LIST_DIRECTORY, FILE_READ_ATTRIBUTES,
        FILE_SHARE_READ, FILE_SHARE_WRITE, GetDriveTypeW,
    };

    let unsafe_path = |check: &str| std::io::Error::other(format!("private runtime directory check failed: {check}"));
    let fresh = matches!(policy, WindowsDirectoryPolicy::Fresh);
    let path = if path.is_absolute() { path.to_path_buf() } else { std::env::current_dir()?.join(path) };
    if path.as_os_str().encode_wide().any(|unit| unit == 0)
        || path.components().any(|component| matches!(component, Component::ParentDir))
        || path.file_name().is_none()
        || !path.has_root()
    {
        return Err(unsafe_path("absolute directory path, no NUL or parent component"));
    }
    let drive = match path.components().next() {
        Some(Component::Prefix(prefix)) => match prefix.kind() {
            Prefix::Disk(drive) | Prefix::VerbatimDisk(drive) => drive,
            _ => return Err(unsafe_path("plain local drive prefix")),
        },
        _ => return Err(unsafe_path("drive prefix missing")),
    };
    let root: Vec<u16> = format!("{}:\\", char::from(drive)).encode_utf16().chain([0]).collect();
    // SAFETY: the local drive root is NUL-terminated; reject remote/unknown drives before any filesystem lookup.
    let drive_type = unsafe { GetDriveTypeW(root.as_ptr()) };
    #[cfg(debug_assertions)]
    eprintln!("WINDOWS_STATE path={path:?} drive_type={drive_type} fixed_local={}", drive_type == 3);
    if drive_type != 3 {
        return Err(unsafe_path("volume is not a fixed local drive"));
    }
    let mut components = path.components().peekable();
    let mut at = PathBuf::new();
    let mut pins = Vec::new();
    while let Some(component) = components.next() {
        at.push(component);
        if matches!(component, Component::Prefix(_) | Component::CurDir) {
            continue;
        }
        let target = components.peek().is_none();
        #[cfg(debug_assertions)]
        eprintln!("WINDOWS_STATE component={at:?} target={target}");
        let (created, file) = if matches!(component, Component::RootDir) {
            let file = OpenOptions::new()
                .access_mode(FILE_LIST_DIRECTORY | FILE_READ_ATTRIBUTES)
                .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
                .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
                .open(&at)?;
            (false, file)
        } else {
            let parent = pins.last().ok_or_else(|| unsafe_path("pinned ancestor missing"))?;
            match native_acl::open_directory_at(parent, component.as_os_str(), target && fresh, target) {
                Ok(file) => (target && fresh, file),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound && !(target && fresh) => {
                    #[cfg(debug_assertions)]
                    eprintln!("WINDOWS_STATE component_open=not_found component={at:?} target={target}");
                    if !target && matches!(policy, WindowsDirectoryPolicy::BootstrapStateRoot) {
                        // Startup authorizes the selected root, not creating missing ancestors.
                        return Err(unsafe_path("state-root ancestor does not exist; bootstrap cannot create ancestors"));
                    }
                    (true, native_acl::open_directory_at(parent, component.as_os_str(), true, target)?)
                }
                Err(error) => {
                    #[cfg(debug_assertions)]
                    eprintln!("WINDOWS_STATE component_open_error component={at:?} error={error}");
                    return Err(error);
                }
            }
        };
        #[cfg(debug_assertions)]
        native_acl::diagnose_handle(&file);
        let metadata = file.metadata()?;
        if !metadata.is_dir() || metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return Err(unsafe_path("pinned component is not a directory or has FILE_ATTRIBUTE_REPARSE_POINT"));
        }
        if created {
            if !native_acl::is_kumis_shape_handle(&file, true) {
                return Err(unsafe_path("new directory owner or DACL is not Kumi's exact private shape"));
            }
        } else if target && !native_acl::is_kumis_shape_handle(&file, true) {
            // Only startup's explicit state-root selection authorizes bootstrapping an existing root.
            // Generic targets, like the owning DAW caches, need an independently verified private parent.
            if !matches!(policy, WindowsDirectoryPolicy::BootstrapStateRoot) {
                let parent = pins.last().ok_or_else(|| unsafe_path("private parent pin missing"))?;
                if !native_acl::is_kumis_shape_handle(parent, true) {
                    return Err(unsafe_path("existing target's parent does not have Kumi's exact private shape"));
                }
            }
            // Relative opens bind the target to that parent. Change only this pinned object's DACL, without
            // ownership takeover or inheritance propagation to old children. Adopted source paths never come here.
            native_acl::protect_owned_directory(&file, &file).map_err(std::io::Error::other)?;
        }
        pins.push(file);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(windows)]
    #[test]
    fn windows_startup_bootstraps_only_the_explicit_owned_state_root() {
        use daw_bridge::lifecycle::native_acl::{is_kumis_shape, shapes};
        let dir = tempfile::tempdir().unwrap();
        let parent = dir.path().join("host");
        std::fs::create_dir(&parent).unwrap();
        shapes::own_mine(&parent);
        shapes::write(
            &parent,
            Some(&[
                shapes::Ace { allow: true, flags: shapes::INHERITABLE, mask: shapes::FULL, whose: shapes::Whose::Me },
                shapes::Ace { allow: true, flags: shapes::INHERITABLE, mask: 0x0012_0089, whose: shapes::Whose::Sid("S-1-1-0") },
            ]),
            true,
        );
        // The host creates state/ and holds runtime.lock before it spawns kumi serve.
        let state = parent.join("state");
        std::fs::create_dir(&state).unwrap();
        assert!(shapes::has_default_owner(&state), "exercise the owner Windows actually assigns");
        if std::env::var_os("CI").is_some() {
            assert!(!shapes::default_owner_is_user(), "elevated Windows CI must exercise the distinct TokenOwner case");
        }
        let lock = state.join("runtime.lock");
        let _held_lock = std::fs::File::create(&lock).unwrap();
        assert!(!is_kumis_shape(&state), "the host's inherited ACL needs bootstrap");
        let untouched = [parent.as_path(), lock.as_path()];
        let before: Vec<_> = untouched.iter().map(|path| shapes::stored_descriptor(path)).collect();
        let state_before = shapes::stored_descriptor(&state);
        assert!(make_private_dir(&state).is_err(), "generic creation cannot authorize root bootstrap");
        assert_eq!(shapes::stored_descriptor(&state), state_before);
        make_private_state_root(&state).unwrap();
        assert!(is_kumis_shape(&state));
        assert!(shapes::has_default_owner(&state), "bootstrap must not take ownership");
        let secured = shapes::stored_descriptor(&state);
        make_private_state_root(&state).unwrap();
        assert_eq!(shapes::stored_descriptor(&state), secured, "an already-private root needs no repair");
        for (path, descriptor) in untouched.into_iter().zip(before) {
            assert_eq!(shapes::stored_descriptor(path), descriptor, "bootstrap changed {}", path.display());
        }
        let fresh = parent.join("fresh");
        make_private_state_root(&fresh).unwrap();
        assert!(is_kumis_shape(&fresh), "a missing explicit root is born private");
        let outside = parent.join("unmade");
        assert!(make_private_state_root(&outside.join("state")).is_err());
        assert!(!outside.exists(), "bootstrap must not create an ancestor of the selected root");
    }

    #[cfg(windows)]
    #[test]
    fn a_windows_scratch_collision_is_refused_without_adopting_or_deleting_it() {
        use daw_bridge::lifecycle::native_acl::{is_kumis_shape, shapes};
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state/scratch/listen/call");
        make_private_windows_dir(&path, WindowsDirectoryPolicy::Fresh).unwrap();
        assert!(is_kumis_shape(&path));
        std::fs::write(path.join("kept"), b"not this call's").unwrap();
        let before = shapes::descriptor(&path);
        assert_eq!(make_private_windows_dir(&path, WindowsDirectoryPolicy::Fresh).unwrap_err().kind(), std::io::ErrorKind::AlreadyExists);
        assert_eq!(shapes::descriptor(&path), before);
        assert_eq!(std::fs::read(path.join("kept")).unwrap(), b"not this call's");
    }

    #[cfg(windows)]
    #[test]
    fn windows_scratch_creation_coexists_with_writes_in_its_state_ancestor() {
        use daw_bridge::lifecycle::native_acl::is_kumis_shape;
        use std::os::windows::fs::OpenOptionsExt;
        let dir = tempfile::tempdir().unwrap();
        let state = dir.path().join("state");
        make_private_dir(&state).unwrap();
        let state = std::fs::canonicalize(state).unwrap();
        // Atomic snapshot rename opens its destination directory for adding an entry.
        // Such access is not permission to replace an ancestor with a reparse point.
        let _writer = std::fs::OpenOptions::new()
            .access_mode(2 | 4) // FILE_ADD_FILE | FILE_ADD_SUBDIRECTORY
            .share_mode(7)
            .custom_flags(0x0220_0000)
            .open(&state)
            .unwrap();
        let scratch = state.join("scratch/listen/call");
        make_private_windows_dir(&scratch, WindowsDirectoryPolicy::Fresh).unwrap();
        assert!(is_kumis_shape(&scratch));
        std::fs::write(scratch.join("take.wav"), b"private capture").unwrap();
        let queued = state.join("status.pending");
        std::fs::write(&queued, b"new snapshot").unwrap();
        std::fs::rename(queued, state.join("status.json")).unwrap();
        assert_eq!(std::fs::read(state.join("status.json")).unwrap(), b"new snapshot");
        assert_eq!(std::fs::read(scratch.join("take.wav")).unwrap(), b"private capture");
    }

    #[cfg(windows)]
    #[test]
    fn securing_an_owned_windows_state_changes_only_that_exact_directory() {
        use daw_bridge::lifecycle::native_acl::{is_kumis_shape, shapes};
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("private");
        make_private_dir(&root).unwrap();
        let state = root.join("state");
        let neighbor = root.join("neighbor");
        std::fs::create_dir(&state).unwrap();
        std::fs::create_dir(&neighbor).unwrap();
        shapes::own_mine(&state);
        shapes::write(
            &state,
            Some(&[
                shapes::Ace { allow: true, flags: shapes::INHERITABLE, mask: shapes::FULL, whose: shapes::Whose::Me },
                shapes::Ace { allow: true, flags: shapes::INHERITABLE, mask: 0x0012_0089, whose: shapes::Whose::Sid("S-1-1-0") },
            ]),
            true,
        );
        assert!(!is_kumis_shape(&state), "the fixture must require tightening");
        let child = state.join("old");
        std::fs::create_dir(&child).unwrap();
        let file = child.join("kept");
        std::fs::write(&file, b"unchanged").unwrap();
        let untouched = [dir.path(), root.as_path(), neighbor.as_path(), child.as_path(), file.as_path()];
        let before: Vec<_> = untouched.iter().map(|path| shapes::stored_descriptor(path)).collect();
        make_private_dir(&state).unwrap();
        assert!(is_kumis_shape(&state));
        make_private_dir(&state).unwrap();
        for (path, descriptor) in untouched.into_iter().zip(before) {
            assert_eq!(shapes::stored_descriptor(path), descriptor, "changed {}", path.display());
        }
        assert_eq!(std::fs::read(file).unwrap(), b"unchanged");
    }

    #[cfg(windows)]
    #[test]
    fn windows_existing_targets_need_a_verified_private_parent_before_tightening() {
        use daw_bridge::lifecycle::native_acl::{is_kumis_shape, shapes};
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("root");
        std::fs::create_dir(&root).unwrap();
        shapes::own_mine(&root);
        shapes::write(
            &root,
            Some(&[
                shapes::Ace { allow: true, flags: shapes::INHERITABLE, mask: shapes::FULL, whose: shapes::Whose::Me },
                shapes::Ace { allow: true, flags: shapes::INHERITABLE, mask: 0x0012_0089, whose: shapes::Whose::Sid("S-1-1-0") },
            ]),
            true,
        );
        let target = root.join("existing");
        std::fs::create_dir(&target).unwrap();
        shapes::own_mine(&target);
        assert!(!is_kumis_shape(&root) && !is_kumis_shape(&target));
        let before = [shapes::stored_descriptor(&root), shapes::stored_descriptor(&target)];
        assert!(make_private_dir(&target).is_err());
        assert_eq!([shapes::stored_descriptor(&root), shapes::stored_descriptor(&target)], before);
    }

    #[cfg(windows)]
    #[test]
    fn windows_existing_targets_refuse_foreign_private_parents_unchanged() {
        use daw_bridge::lifecycle::native_acl::shapes;
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("root");
        make_private_dir(&root).unwrap();
        let target = root.join("existing");
        std::fs::create_dir(&target).unwrap();
        shapes::own_mine(&target);
        shapes::write(
            &target,
            Some(&[
                shapes::Ace { allow: true, flags: shapes::INHERITABLE, mask: shapes::FULL, whose: shapes::Whose::Me },
                shapes::Ace { allow: true, flags: 0, mask: 0x0012_0089, whose: shapes::Whose::Sid("S-1-1-0") },
            ]),
            true,
        );
        if !shapes::own_foreign_user(&root) {
            assert!(std::env::var_os("CI").is_none(), "CI must exercise foreign-root refusal");
            return;
        }
        let before = [shapes::stored_descriptor(&root), shapes::stored_descriptor(&target)];
        let refused = make_private_dir(&target).is_err();
        let after = [shapes::stored_descriptor(&root), shapes::stored_descriptor(&target)];
        shapes::own_mine(&root);
        assert!(refused);
        assert_eq!(after, before);
    }

    #[cfg(windows)]
    #[test]
    fn windows_private_directories_refuse_foreign_owners_without_taking_them_over() {
        use daw_bridge::lifecycle::native_acl::shapes;
        let dir = tempfile::tempdir().unwrap();
        let state = dir.path().join("state");
        make_private_dir(&state).unwrap();
        if !shapes::own_foreign_user(&state) {
            assert!(std::env::var_os("CI").is_none(), "CI must exercise foreign-owner refusal");
            return; // A non-elevated developer account may lack SeRestorePrivilege.
        }
        let before = shapes::descriptor(&state);
        assert!(!daw_bridge::lifecycle::native_acl::is_kumis_shape(&state));
        let result = make_private_dir(&state);
        let bootstrap = make_private_state_root(&state);
        let after = shapes::descriptor(&state);
        shapes::own_mine(&state);
        assert!(result.is_err());
        assert!(bootstrap.unwrap_err().to_string().contains("must belong to the current account"));
        assert_eq!(after, before);
    }

    #[cfg(windows)]
    #[test]
    fn windows_private_directories_refuse_files_and_junctions_without_touching_their_targets() {
        use daw_bridge::lifecycle::native_acl::shapes;
        let dir = tempfile::tempdir().unwrap();
        let outside = dir.path().join("outside");
        make_private_dir(&outside).unwrap();
        let file = outside.join("kept");
        std::fs::write(&file, b"outside").unwrap();
        let target = outside.join("existing");
        std::fs::create_dir(&target).unwrap();
        shapes::own_mine(&target);
        shapes::write(
            &target,
            Some(&[
                shapes::Ace { allow: true, flags: shapes::INHERITABLE, mask: shapes::FULL, whose: shapes::Whose::Me },
                shapes::Ace { allow: true, flags: 0, mask: 0x0012_0089, whose: shapes::Whose::Sid("S-1-1-0") },
            ]),
            true,
        );
        let target_before = shapes::stored_descriptor(&target);
        let before = [shapes::descriptor(&outside), shapes::descriptor(&file)];
        let link = dir.path().join("redirect");
        directory_link(&outside, &link);
        assert!(make_private_dir(&link).is_err());
        assert!(make_private_state_root(&link).is_err());
        assert!(make_private_state_root(&link.join("existing")).is_err());
        assert!(make_private_dir(&link.join("existing")).is_err());
        assert_eq!(shapes::stored_descriptor(&target), target_before);
        assert!(make_private_dir(&link.join("new/child")).is_err());
        assert!(make_private_windows_dir(&link.join("call"), WindowsDirectoryPolicy::Fresh).is_err());
        assert!(make_private_dir(&file).is_err());
        assert!(!outside.join("new").exists() && !outside.join("call").exists());
        assert_eq!([shapes::descriptor(&outside), shapes::descriptor(&file)], before);
        assert_eq!(std::fs::read(&file).unwrap(), b"outside");
        std::fs::remove_dir(link).unwrap();
    }

    #[test]
    fn the_host_says_who_asked_and_nothing_else() {
        let host = Host::from_value(
            &json!({"profile_id": "default", "session_id": "s1", "tool_call_id": "t1", "request_id": null, "task_id": " "}),
        )
        .unwrap();
        assert_eq!(
            (host.profile_id.as_deref(), host.session_id.as_deref(), host.tool_call_id.as_deref()),
            (Some("default"), Some("s1"), Some("t1"))
        );
        assert_eq!(host.task_id, None, "a blank id is no id");
        assert_eq!(Host::from_value(&json!({"tool_call_id": ""})).unwrap(), Host::default());
        assert_eq!(host.request_id, None);
        assert!(Host::from_value(&json!({"profile_id": "default", "approved": true})).unwrap_err().contains("approved"));
        assert!(Host::from_value(&json!({"session_id": 7})).unwrap_err().contains("string"));
        assert!(Host::from_value(&json!("default")).is_err());
    }

    #[test]
    fn an_error_says_its_code_reason_and_holder() {
        let refused = OpError::Refused { code: "leased", reason: "another session has Live".into(), owner: Some(json!({"pid": 7})) };
        assert_eq!(refused.to_value(), json!({"error": {"code": "leased", "reason": "another session has Live", "owner": {"pid": 7}}}));
        assert_eq!(OpError::NotConnected { reason: "connect first".into() }.to_value()["error"]["code"], "not_connected");
    }

    #[test]
    fn only_writes_plays_and_imports_take_the_lease() {
        let writing: Vec<Effect> = [
            Effect::Read,
            Effect::Connect,
            Effect::Write,
            Effect::Playback,
            Effect::Import,
            Effect::FolderRead,
            Effect::Setup,
            Effect::Upload,
            Effect::Paid,
        ]
        .into_iter()
        .filter(|effect| effect.writes())
        .collect();
        assert_eq!(writing, [Effect::Write, Effect::Playback, Effect::Import]);
    }

    #[test]
    fn progress_goes_out_only_when_asked_for_and_until_the_call_ends() {
        let (outbox, mut sent) = tokio::sync::mpsc::unbounded_channel();
        let progress = Progress::new(Some(json!("p1")), outbox.clone());
        let kept = progress.clone();
        progress.report(2.0, Some(5.0), Some("scanning"));
        Progress::new(None, outbox).report(1.0, None, None);
        let Ok(Out::Message(message)) = sent.try_recv() else { panic!("progress") };
        assert_eq!(message["params"], json!({"progressToken": "p1", "progress": 2.0, "total": 5.0, "message": "scanning"}));
        assert!(sent.try_recv().is_err(), "nothing for a call that didn't ask");
        progress.end();
        kept.report(3.0, Some(5.0), None);
        assert!(sent.try_recv().is_err(), "nothing after the call ended, from a copy kept elsewhere either");
    }

    #[test]
    fn a_calls_scratch_is_its_own_named_by_the_runtime_and_goes_with_it() {
        let dir = tempfile::tempdir().unwrap();
        let args = Args {
            state_dir: dir.path().join("state"),
            kumi_home: dir.path().join("home"),
            hermes_home: None,
            hermes_root: None,
            daw: DawId::Reaper,
            library_roots: vec![],
            media_roots: vec![],
            tools: BTreeMap::new(),
        };
        let ctx = Ctx::new(args, Env::new(), Default::default()).unwrap();
        let (one, two) = (ctx.scratch("listen").unwrap(), ctx.scratch("listen").unwrap());
        assert_ne!(one.path(), two.path());
        let under = std::fs::canonicalize(dir.path()).unwrap().join("state/scratch/listen");
        assert!(one.path().starts_with(&under) && one.path().is_dir(), "{}", one.path().display());
        #[cfg(windows)]
        for folder in [ctx.paths().state_dir.clone(), under.parent().unwrap().to_path_buf(), under.clone(), one.path().to_path_buf()] {
            assert!(daw_bridge::lifecycle::native_acl::is_kumis_shape(&folder), "not private from creation: {}", folder.display());
        }
        std::fs::create_dir(one.path().join("takes")).unwrap();
        std::fs::write(one.path().join("takes/take.wav"), b"RIFF").unwrap();
        let gone = one.path().to_path_buf();
        drop(one);
        assert!(!gone.exists() && two.path().is_dir(), "only its own folder goes");
        assert!(ctx.scratch("../status").is_err());
    }

    #[test]
    fn the_library_reads_only_the_roots_granted_as_they_were_then() {
        let dir = tempfile::tempdir().unwrap();
        for folder in ["Samples", "Drums", "media"] {
            std::fs::create_dir_all(dir.path().join(folder)).unwrap();
        }
        std::fs::write(dir.path().join("kick.wav"), b"RIFF").unwrap();
        let args = Args {
            state_dir: dir.path().join("state"),
            kumi_home: dir.path().join("home"),
            hermes_home: None,
            hermes_root: None,
            daw: DawId::Reaper,
            library_roots: vec![dir.path().join("Samples"), dir.path().join("Gone"), dir.path().join("Samples/../Samples")],
            media_roots: vec![dir.path().join("media"), dir.path().join("kick.wav")],
            tools: BTreeMap::new(),
        };
        let ctx = Ctx::new(args, Env::new(), Default::default()).unwrap();
        let resolved = |name: &str| std::fs::canonicalize(dir.path().join(name)).unwrap();
        assert_eq!(ctx.library_roots(), [resolved("Samples")], "a root not there grants nothing; one root named twice is one");
        assert_eq!(ctx.media_roots(), [resolved("media")], "a file is no root");
        assert_eq!(ctx.add_checked_library_root(resolved("Drums")).unwrap(), resolved("Drums"));
        ctx.add_checked_library_root(resolved("Drums")).unwrap();
        assert_eq!(ctx.library_roots(), [resolved("Samples"), resolved("Drums")]);
        ctx.snapshots().flush();
        let shown: Value = serde_json::from_slice(&std::fs::read(dir.path().join("state/library-roots.json")).unwrap()).unwrap();
        assert_eq!(shown["roots"].as_array().map(Vec::len), Some(2));
        assert!(ctx.media().installed_only() && ctx.media().tool("ffmpeg").is_none());
    }

    fn media_context(dir: &Path, roots: Vec<PathBuf>) -> Rc<Ctx> {
        Ctx::new(
            Args {
                state_dir: dir.join("state"),
                kumi_home: dir.join("home"),
                hermes_home: None,
                hermes_root: None,
                daw: DawId::Reaper,
                library_roots: Vec::new(),
                media_roots: roots,
                tools: BTreeMap::new(),
            },
            Env::new(),
            Default::default(),
        )
        .unwrap()
    }

    #[test]
    fn pending_media_folders_can_appear_but_traversal_and_wide_roots_grant_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path().canonicalize().unwrap();
        let root = base.join("media/generated/audio");
        let ctx = media_context(&base, vec![root.clone(), base.clone(), base.join("bad/../outside")]);
        assert!(ctx.media_roots().is_empty());
        std::fs::create_dir_all(&root).unwrap();
        let file = root.join("generated.wav");
        std::fs::write(&file, b"generated audio").unwrap();
        assert_eq!(ctx.approved(&file).unwrap(), file.canonicalize().unwrap());
        assert_eq!(ctx.media_roots(), [root.canonicalize().unwrap()]);
        let outside = base.join("outside.wav");
        std::fs::write(&outside, b"not approved").unwrap();
        assert_eq!(ctx.approved(&outside).unwrap_err().code(), "outside_root");
        assert_eq!(ctx.add_checked_library_root(dir.path().canonicalize().unwrap()).unwrap_err().code(), "unsafe_folder");
        assert_eq!(ctx.add_checked_library_root(ctx.paths().state_dir.clone()).unwrap_err().code(), "unsafe_folder");
        ctx.snapshots().flush();
    }

    #[cfg(any(unix, windows))]
    fn directory_link(target: &Path, link: &Path) {
        #[cfg(unix)]
        std::os::unix::fs::symlink(target, link).unwrap();
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            fn plain(path: &Path) -> std::borrow::Cow<'_, Path> {
                let mut components = path.components();
                if let Some(std::path::Component::Prefix(prefix)) = components.next() {
                    if let std::path::Prefix::VerbatimDisk(drive) = prefix.kind() {
                        let mut plain = PathBuf::from(format!("{}:", char::from(drive)));
                        plain.push(components.as_path());
                        return std::borrow::Cow::Owned(plain);
                    }
                }
                std::borrow::Cow::Borrowed(path)
            }
            let link = plain(link);
            let target = plain(target);
            // cmd parses its command after /c itself; CRT argument quoting is not its quoting.
            // /s removes the outer pair, leaving one explicitly quoted path for each mklink operand.
            let mut command = std::ffi::OsString::from("\"mklink /J \"");
            command.push(link.as_os_str());
            command.push("\" \"");
            command.push(target.as_os_str());
            command.push("\"\"");
            // cmd parses switches even in argv[0]; use native separators, not "System32/cmd.exe".
            let cmd = PathBuf::from(std::env::var_os("SystemRoot").unwrap()).join("System32").join("cmd.exe");
            let output = std::process::Command::new(cmd).args(["/d", "/s", "/c"]).raw_arg(&command).output().unwrap();
            assert!(
                output.status.success(),
                "junction link={link:?}, target={target:?}, command={command:?}\nstdout: {}\nstderr: {}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
        }
    }

    #[cfg(any(unix, windows))]
    #[test]
    fn a_pending_media_descendant_cannot_become_a_link_or_junction_to_another_folder() {
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path().canonicalize().unwrap().join("media with spaces");
        std::fs::create_dir(&base).unwrap();
        let ctx = media_context(dir.path(), vec![base.join("generated/audio")]);
        let outside = dir.path().canonicalize().unwrap().join("outside with spaces");
        std::fs::create_dir_all(outside.join("audio")).unwrap();
        let escaped = outside.join("audio/private.wav");
        std::fs::write(&escaped, b"outside").unwrap();
        directory_link(&outside, &base.join("generated"));
        assert_eq!(ctx.approved(&base.join("generated/audio/private.wav")).unwrap_err().code(), "outside_root");
        assert_eq!(ctx.approved(&escaped).unwrap_err().code(), "outside_root");
        assert!(ctx.media_roots().is_empty());
        assert_eq!(std::fs::read(escaped).unwrap(), b"outside");
        ctx.snapshots().flush();
    }

    #[cfg(unix)]
    #[test]
    fn a_pending_grant_freezes_the_existing_ancestor_not_its_retargetable_alias() {
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path().canonicalize().unwrap();
        let original = base.join("original");
        let outside = base.join("outside");
        std::fs::create_dir(&original).unwrap();
        std::fs::create_dir(&outside).unwrap();
        let alias = base.join("alias");
        directory_link(&original, &alias);
        let ctx = media_context(&base, vec![alias.join("generated")]);
        std::fs::remove_file(&alias).unwrap();
        directory_link(&outside, &alias);
        for root in [&original, &outside] {
            std::fs::create_dir(root.join("generated")).unwrap();
            std::fs::write(root.join("generated/take.wav"), b"audio").unwrap();
        }
        let approved = original.join("generated/take.wav");
        assert_eq!(ctx.approved(&approved).unwrap(), approved.canonicalize().unwrap());
        assert_eq!(ctx.approved(&outside.join("generated/take.wav")).unwrap_err().code(), "outside_root");
        assert_eq!(ctx.approved(&alias.join("generated/take.wav")).unwrap_err().code(), "outside_root");
        ctx.snapshots().flush();
    }

    #[test]
    fn startup_grants_refuse_canonical_homes_profiles_and_state_ancestors() {
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path().canonicalize().unwrap();
        let home = base.join("people/producer");
        let hermes = base.join("profile-parent/hermes");
        let state = hermes.join("plugin/state");
        let samples = base.join("Samples");
        for folder in [&home, &state, &samples] {
            std::fs::create_dir_all(folder).unwrap();
        }
        let private = ProtectedRoots {
            ancestors: vec![home.clone(), hermes.parent().unwrap().to_path_buf()],
            subtrees: Vec::new(),
            hermes: vec![hermes.clone()],
            generated_audio: None,
        };
        let roots = vec![
            base.ancestors().last().unwrap().to_path_buf(),
            home.parent().unwrap().to_path_buf(),
            home,
            hermes,
            state.clone(),
            state.parent().unwrap().to_path_buf(),
            samples.clone(),
        ];
        assert_eq!(granted("library", roots.clone(), &state, &private), [samples.clone()]);
        assert_eq!(media_granted(roots, &state, &private), [samples]);
    }

    #[test]
    fn protected_descendants_refuse_library_grants_and_only_active_generated_audio_is_host_media() {
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path().canonicalize().unwrap();
        let hermes = base.join("HermesData");
        let active = hermes.join("profiles/work");
        let state = active.join("plugin-data/kumi/state");
        let transport = active.join("plugin-data/kumi/home");
        let generated = active.join("cache/generated/audio");
        let samples = base.join("Samples");
        let refused = [
            active.join("cache"),
            active.join("cache/other"),
            hermes.join("profiles/other/cache/generated/audio"),
            transport.join("x"),
            state.join("artifacts"),
            state.join("library"),
            active.join("plugin-data/kumi/runtime"),
            active.join("plugin-data/kumi/setup"),
            generated.join("child"),
        ];
        // The runtime owns its state even on an elevated Windows runner, whose default creator owner is Administrators.
        make_private_dir(&state).unwrap();
        for root in refused.iter().chain([&samples, &generated]) {
            std::fs::create_dir_all(root).unwrap();
            std::fs::write(root.join("take.wav"), b"not granted by its extension").unwrap();
        }
        let roots: Vec<_> = refused.iter().cloned().chain([generated.clone(), samples.clone()]).collect();
        let ctx = Ctx::new(
            Args {
                state_dir: state,
                kumi_home: transport,
                hermes_home: Some(active),
                hermes_root: Some(hermes),
                daw: DawId::Reaper,
                library_roots: roots.clone(),
                media_roots: roots,
                tools: BTreeMap::new(),
            },
            Env::new(),
            Default::default(),
        )
        .unwrap();
        assert_eq!(ctx.library_roots(), [samples.clone()]);
        assert_eq!(ctx.media_roots(), [generated.clone(), samples]);
        assert_eq!(ctx.add_checked_library_root(generated.clone()).unwrap_err().code(), "unsafe_folder");
        assert_eq!(ctx.approved(&generated.join("take.wav")).unwrap(), generated.join("take.wav"));
        assert_eq!(ctx.approved(&generated.join("child/take.wav")).unwrap(), generated.join("child/take.wav"));
        for root in &refused[..refused.len() - 1] {
            assert_eq!(ctx.add_checked_library_root(root.clone()).unwrap_err().code(), "unsafe_folder", "{root:?}");
            assert_eq!(ctx.approved(&root.join("take.wav")).unwrap_err().code(), "outside_root", "{root:?}");
        }
        ctx.snapshots().flush();
    }

    #[test]
    fn a_missing_active_generated_media_folder_is_allowed_but_a_private_pending_folder_is_not() {
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path().canonicalize().unwrap();
        let home = base.join("custom-hermes");
        let generated = home.join("cache/generated/audio");
        let private = home.join("cache/other/later");
        let transport = home.join("plugin-data/kumi/home");
        std::fs::create_dir_all(transport.parent().unwrap()).unwrap();
        let ctx = Ctx::new(
            Args {
                state_dir: home.join("plugin-data/kumi/state"),
                kumi_home: transport.clone(),
                hermes_home: Some(home.clone()),
                hermes_root: Some(home),
                daw: DawId::Reaper,
                library_roots: Vec::new(),
                media_roots: vec![generated.clone(), private.clone(), transport.join("later")],
                tools: BTreeMap::new(),
            },
            Env::new(),
            Default::default(),
        )
        .unwrap();
        assert!(ctx.media_roots().is_empty());
        for root in [&generated, &private, &transport.join("later")] {
            std::fs::create_dir_all(root).unwrap();
            std::fs::write(root.join("take.wav"), b"later").unwrap();
        }
        assert_eq!(ctx.media_roots(), [generated.clone()]);
        assert_eq!(ctx.approved(&generated.join("take.wav")).unwrap(), generated.join("take.wav"));
        assert_eq!(ctx.approved(&private.join("take.wav")).unwrap_err().code(), "outside_root");
        assert_eq!(ctx.approved(&transport.join("later/take.wav")).unwrap_err().code(), "outside_root");
        ctx.snapshots().flush();
    }
}
