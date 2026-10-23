//! Catching up on a saved project, and watching the producer work, for a DAW whose bridge is Kumi's own: the project's
//! snapshot, read whole through Kumi's host (in one request where the bridge reads every row of the kinds at once,
//! `snapshot {rows}`; else `read`, a page at a time, every page of one epoch; a snapshot an edit tore is read again),
//! kept as the baseline in the project store under the project's place, and compared with the project
//! as it is when it's opened or switched to: what changed goes to the app (`on_catch_up`) and into the model's context.
//! The baseline is saved again every five minutes and soon after Kumi's own changes, as Live's is. `watch_me` compares
//! the project before and after the producer works. What a snapshot holds and how two differ is the DAW's
//! (`OwnProject`); the words are core's describers' (`core/describe.rs`) with the DAW's names for what a project holds.
use super::own_bridge::{answer_of, Refused, CALL_MS};
use crate::core::{
    context::IntegrationContext,
    contracts::{CatchUp, JsonObject, KernelTool, Reach, ToolResult},
    daw::{DawId, DawProfile},
    describe::{describe_changes, describe_watch_changes, DescribedDiff, ProjectChange, ProjectWords},
    errors::RuntimeError,
    place::is_project_id,
    project::{project_key_of, Baseline, ProjectRules, ProjectStore},
};
use crate::mcp::client::McpEndpoint;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use futures::{
    future::{LocalBoxFuture, Shared},
    FutureExt,
};
use kumi_common::{
    abort::{self, Signal, SignalExt},
    js::{
        json::stringify,
        number::{round, to_string},
    },
    time::since,
};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    cell::{Cell, RefCell},
    panic::{catch_unwind, AssertUnwindSafe},
    rc::Rc,
    time::Duration,
};

/// The rows of one kind a snapshot read, by the kind's name.
pub type KindRows = (&'static str, Vec<Value>);

/// What a DAW gives the shared integration to catch up on its saved projects and to watch the producer work.
pub struct OwnProject {
    /// How the DAW tells its templates (never caught up on or kept: new songs start from them) and a song's folder.
    pub rules: &'static dyn ProjectRules,
    /// The DAW's names for what a project holds.
    pub words: &'static ProjectWords,
    /// The kinds the snapshot reads whole through the host's `read`, in order: the first is the project's own row.
    pub reads: &'static [&'static str],
    /// The snapshot kept as the baseline, from every row of each kind read (in `reads`' order); why not, when the rows
    /// make none.
    pub snapshot: fn(&[KindRows]) -> Result<JsonObject, String>,
    /// What changed from one snapshot to another; `watching` adds what watch_me tells beside catch-up (the track each
    /// thing is on, what an added thing is made of, each detail's values).
    pub changes: fn(&JsonObject, &JsonObject, bool) -> Vec<ProjectChange>,
    /// watch_me's description, in the DAW's words.
    pub watch_description: &'static str,
    /// (M6) Whether the DAW's bridge reads every row of the kinds in one request, from one look at the project
    /// (`snapshot {rows}`: Logic's, which shows its Mixer and List Editors once rather than once a kind). The snapshot is
    /// read so, and a page at a time when it can't be (an older bridge, a kind past what one answer holds).
    pub bulk: bool,
}

/// The tool that watches the producer work.
pub const WATCH_TOOL: &str = "watch_me";
/// The rows a snapshot's page asks for, and the most pages one kind may take: past them the project is too big to keep.
const PAGE_ROWS: u64 = 200;
const MOST_PAGES: usize = 50;
/// A snapshot an edit tore is read again, this many times in all, this far apart.
const TRIES: usize = 3;
const TORN_GAP_MS: u64 = 250;
/// How long catching up may take, a save, and a save as Kumi closes (with its wait), as Live's.
const CATCH_UP_MS: u64 = 60_000;
const SAVE_MS: u64 = 30_000;
const CLOSE_SAVE_MS: u64 = 2_000;
const CLOSE_WAIT_MS: u64 = 2_500;
/// The baseline is saved again this often while the project is open, and this long after Kumi's own changes.
const SAVE_EVERY_MS: i64 = 5 * 60_000;
const AFTER_CHANGES_MS: u64 = 20_000;
/// A catch-up the DAW couldn't answer (a dialog, a busy moment) is tried again with the looks after it, this many times.
const CATCH_UP_TRIES: u32 = 5;
/// How long watch_me may take to read the project.
const WATCH_MS: u64 = 60_000;

/// A snapshot as read: what the DAW keeps of it, the project row it was read with, and what reading it took.
pub struct Read {
    pub snapshot: JsonObject,
    pub project: Value,
    pub pages: usize,
    pub rows: usize,
}

/// Why a snapshot couldn't be read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Unread {
    /// The DAW didn't answer (busy, away, gone), or answered something else: its words.
    Later(String),
    /// More rows of a kind than a snapshot keeps.
    TooBig(&'static str),
}

enum Torn {
    Again(String),
    Not(Unread),
}

/// The project's snapshot, read through the host: each kind of `project.reads` whole, a page at a time, every page of
/// one epoch. One an edit tore (the epoch moved between pages, or the bridge refused a cursor or ref as stale) is read
/// again from the start, a few times.
pub async fn read_snapshot(endpoint: &Rc<dyn McpEndpoint>, project: &OwnProject, signal: &Signal) -> Result<Read, Unread> {
    let mut why = String::new();
    for attempt in 0..TRIES {
        if attempt > 0 {
            tokio::time::sleep(Duration::from_millis(TORN_GAP_MS)).await;
        }
        if signal.is_cancelled() {
            return Err(Unread::Later("Kumi stopped reading".into()));
        }
        match read_once(endpoint, project, signal).await {
            Ok(read) => return Ok(read),
            Err(Torn::Again(said)) => why = said,
            Err(Torn::Not(unread)) => return Err(unread),
        }
    }
    Err(Unread::Later(why))
}

async fn read_once(endpoint: &Rc<dyn McpEndpoint>, project: &OwnProject, signal: &Signal) -> Result<Read, Torn> {
    if project.bulk {
        if let Some(read) = read_bulk(endpoint, project, signal).await? {
            return Ok(read);
        }
    }
    let mut epoch: Option<String> = None;
    let mut kinds: Vec<KindRows> = vec![];
    let mut pages = 0;
    for kind in project.reads {
        let (mut rows, mut cursor) = (vec![], None::<String>);
        for page_number in 0.. {
            if page_number >= MOST_PAGES {
                return Err(Torn::Not(Unread::TooBig(kind)));
            }
            let mut args = JsonObject::new();
            args.insert("kind".into(), json!(kind));
            args.insert("limit".into(), json!(PAGE_ROWS));
            if let Some(cursor) = &cursor {
                args.insert("cursor".into(), json!(cursor));
            }
            let bounded = abort::any([signal.clone(), abort::timeout(CALL_MS)]);
            let result = endpoint.call("read", args, bounded).await.map_err(|error| Torn::Not(Unread::Later(error.to_string())))?;
            let page = match answer_of(&result) {
                Ok(page) => page,
                Err(Refused { code: Some(code), why, .. }) if matches!(code.as_str(), "stale" | "changed" | "epoch-changed") => {
                    return Err(Torn::Again(why))
                }
                Err(refused) => return Err(Torn::Not(Unread::Later(refused.why))),
            };
            pages += 1;
            match (&epoch, page.get("epoch").and_then(Value::as_str)) {
                (Some(before), Some(now)) if before != now => {
                    return Err(Torn::Again(format!("the project changed while Kumi read it (epoch {before}, then {now})")))
                }
                (None, now) => epoch = now.map(str::to_owned),
                _ => {}
            }
            rows.extend(page.get("items").and_then(Value::as_array).into_iter().flatten().cloned());
            cursor = page.get("nextCursor").and_then(Value::as_str).filter(|next| !next.is_empty()).map(str::to_owned);
            if cursor.is_none() {
                break;
            }
        }
        kinds.push((kind, rows));
    }
    let rows = kinds.iter().map(|(_, rows)| rows.len()).sum();
    let project_row = kinds.first().and_then(|(_, rows)| rows.first().cloned()).unwrap_or(Value::Null);
    let snapshot = (project.snapshot)(&kinds).map_err(|why| Torn::Not(Unread::Later(why)))?;
    Ok(Read { snapshot, project: project_row, pages, rows })
}

/// (M6) The snapshot in one request (`snapshot {rows}`): every row of each kind from one look at the project, in one
/// epoch. None when it can't be read so, and pages are read instead: a bridge that answers the one screen (an older one,
/// which doesn't know `rows`), a refusal, or a kind past what one answer holds (`truncated`, or the host's bound).
async fn read_bulk(endpoint: &Rc<dyn McpEndpoint>, project: &OwnProject, signal: &Signal) -> Result<Option<Read>, Torn> {
    let mut args = JsonObject::new();
    args.insert("rows".into(), json!(project.reads));
    let bounded = abort::any([signal.clone(), abort::timeout(CALL_MS)]);
    let result = endpoint.call("snapshot", args, bounded).await.map_err(|error| Torn::Not(Unread::Later(error.to_string())))?;
    let answer = match answer_of(&result) {
        Ok(answer) => answer,
        Err(Refused { code: Some(code), why, .. }) if matches!(code.as_str(), "stale" | "changed" | "epoch-changed") => {
            return Err(Torn::Again(why))
        }
        Err(Refused { busy: true, why, .. }) => return Err(Torn::Not(Unread::Later(why))),
        Err(_) => return Ok(None),
    };
    let Some(rows) = answer.get("rows").and_then(Value::as_object) else { return Ok(None) };
    if answer.get("truncated").and_then(Value::as_array).is_some_and(|kinds| !kinds.is_empty()) {
        return Ok(None);
    }
    let kinds: Vec<KindRows> =
        project.reads.iter().map(|kind| (*kind, rows.get(*kind).and_then(Value::as_array).cloned().unwrap_or_default())).collect();
    let count = kinds.iter().map(|(_, rows)| rows.len()).sum();
    let project_row = kinds.first().and_then(|(_, rows)| rows.first().cloned()).unwrap_or(Value::Null);
    let snapshot = (project.snapshot)(&kinds).map_err(|why| Torn::Not(Unread::Later(why)))?;
    Ok(Some(Read { snapshot, project: project_row, pages: 1, rows: count }))
}

/// A snapshot's fingerprint: two alike are the same project as it was.
pub fn artifact_of(snapshot: &JsonObject) -> String {
    hex::encode(Sha256::digest(stringify(&Value::Object(snapshot.clone())).as_bytes()))[..32].to_owned()
}

/// The project a snapshot's row names: its id when saved, else the identity an unsaved one carries.
fn identity_of(project: &Value) -> Option<String> {
    project
        .get("id")
        .and_then(Value::as_str)
        .filter(|id| is_project_id(id))
        .or_else(|| project.get("identity").and_then(Value::as_str).filter(|identity| !identity.is_empty()))
        .map(str::to_owned)
}

/// The saved project open as last seen.
struct Current {
    key: String,
    id: String,
    path: String,
    name: String,
    /// Not a template: its baseline is kept, and it's caught up on.
    kept: bool,
    /// Caught up on since it was opened (or came back), so saving it again loses nothing untold.
    caught_up: Cell<bool>,
    /// A catch-up of it waits or runs.
    pending: Cell<bool>,
    /// Catch-ups begun for it.
    tries: Cell<u32>,
}

/// What a look saw of the project, for catching up.
pub struct Seen<'a> {
    /// The saved project's id (none while it's unsaved), where it is and its name.
    pub id: Option<&'a str>,
    pub path: Option<&'a str>,
    pub name: &'a str,
    /// The DAW went away since the look before (it quit, or its bridge did) and is back.
    pub came_back: bool,
    /// It plays or records: a save waits, since the bridge's reads may open the DAW's panes.
    pub playing: bool,
}

struct Watching {
    snapshot: JsonObject,
    project: Option<String>,
    at: i64,
}

type Saving = Shared<LocalBoxFuture<'static, ()>>;

/// What Kumi keeps of a DAW's saved project as it works: the baseline it catches up from, saved again as it goes, and
/// what watch_me saw when it started.
pub struct Remember {
    project: &'static OwnProject,
    profile: &'static DawProfile,
    daw: DawId,
    store: Option<Rc<dyn ProjectStore>>,
    endpoint: Rc<RefCell<Option<Rc<dyn McpEndpoint>>>>,
    connected: Rc<Cell<bool>>,
    lifetime: Signal,
    on_catch_up: Option<Rc<dyn Fn(CatchUp)>>,
    on_watch: Option<Rc<dyn Fn(bool)>>,
    now: Option<Rc<dyn Fn() -> DateTime<Utc>>>,
    current: RefCell<Option<Rc<Current>>>,
    /// What the model is told changed since it was last here, until another project is open.
    context: RefCell<Option<JsonObject>>,
    last_saved: Cell<i64>,
    queue: RefCell<Saving>,
    timer: RefCell<Option<Signal>>,
    watching: RefCell<Option<Watching>>,
}

impl Remember {
    pub fn new(
        project: &'static OwnProject,
        profile: &'static DawProfile,
        store: Option<Rc<dyn ProjectStore>>,
        endpoint: Rc<RefCell<Option<Rc<dyn McpEndpoint>>>>,
        connected: Rc<Cell<bool>>,
        lifetime: Signal,
        context: &IntegrationContext,
    ) -> Rc<Self> {
        Rc::new(Remember {
            project,
            profile,
            daw: profile.id.expect("a DAW with a bridge of Kumi's own is known"),
            store,
            endpoint,
            connected,
            lifetime,
            on_catch_up: context.on_catch_up.clone(),
            on_watch: context.on_watch.clone(),
            now: context.now.clone(),
            current: RefCell::default(),
            context: RefCell::default(),
            last_saved: Cell::new(0),
            queue: RefCell::new(async {}.boxed_local().shared()),
            timer: RefCell::default(),
            watching: RefCell::default(),
        })
    }

    fn now_ms(&self) -> i64 {
        self.now.as_ref().map(|now| now().timestamp_millis()).unwrap_or_else(kumi_common::time::now_ms)
    }

    fn current(&self) -> Option<Rc<Current>> {
        self.current.borrow().clone()
    }

    /// Whether `current` is still the project open.
    fn still(&self, current: &Rc<Current>) -> bool {
        self.current().is_some_and(|now| Rc::ptr_eq(&now, current))
    }

    /// What the model is told changed since it was last here: `{lastSeen, changes, more}`.
    pub fn context(&self) -> Option<JsonObject> {
        self.context.borrow().clone()
    }

    /// A look saw the project: one opened or switched to (or back after the DAW went away) is caught up on; the same one
    /// is saved again every five minutes, once caught up on, while it doesn't play.
    pub fn seen(self: &Rc<Self>, seen: Seen) {
        let before = self.current();
        let (Some(id), Some(path)) = (seen.id.filter(|id| is_project_id(id)), seen.path.filter(|path| !path.is_empty())) else {
            // Not saved: nothing to keep, and nothing to tell of another project.
            if before.is_some() {
                self.forget();
            }
            return;
        };
        let key = project_key_of(self.daw, id);
        let same = before.as_ref().is_some_and(|current| current.key == key && current.path == path);
        if let (true, false, Some(current)) = (same, seen.came_back, before) {
            if !current.kept {
                return;
            }
            if !current.caught_up.get() {
                // The DAW couldn't answer the catch-up before: it's tried again.
                if !current.pending.get() && current.tries.get() < CATCH_UP_TRIES {
                    self.catch_up(current, false);
                }
            } else if !seen.playing && kumi_common::time::now_ms() - self.last_saved.get() > SAVE_EVERY_MS {
                self.schedule_save(1_000);
            }
            return;
        }
        self.forget();
        let kept = !self.project.rules.template_location(path);
        let current = Rc::new(Current {
            key,
            id: id.to_owned(),
            path: path.to_owned(),
            name: seen.name.to_owned(),
            kept,
            caught_up: Cell::new(false),
            pending: Cell::new(false),
            tries: Cell::new(0),
        });
        *self.current.borrow_mut() = Some(current.clone());
        if kept {
            self.catch_up(current, same && seen.came_back);
        }
    }

    /// Another project (or none) is open: what was told of the one before goes.
    fn forget(&self) {
        self.current.borrow_mut().take();
        self.context.borrow_mut().take();
        self.cancel_timer();
    }

    fn enqueue(self: &Rc<Self>, work: impl FnOnce(Rc<Self>) -> LocalBoxFuture<'static, ()> + 'static) -> Saving {
        let before = self.queue.borrow().clone();
        let this = self.clone();
        let pending = async move {
            before.await;
            work(this).await;
        }
        .boxed_local()
        .shared();
        *self.queue.borrow_mut() = pending.clone();
        tokio::task::spawn_local(pending.clone());
        pending
    }

    /// Compares the project with its baseline (this place's, or the project's latest when it moved here), tells what
    /// changed, and keeps the project as it is now as the baseline.
    fn catch_up(self: &Rc<Self>, current: Rc<Current>, after_reconnect: bool) {
        let Some(store) = self.store.clone() else { return };
        current.tries.set(current.tries.get() + 1);
        current.pending.set(true);
        drop(self.enqueue(move |this| {
            async move {
                let done = this.compare(&store, &current, after_reconnect).await;
                current.pending.set(false);
                current.caught_up.set(done);
            }
            .boxed_local()
        }));
    }

    /// Whether the catch-up is over (told, or nothing to tell, or the project isn't open any more); false when the DAW
    /// couldn't be read, to try again.
    async fn compare(self: &Rc<Self>, store: &Rc<dyn ProjectStore>, current: &Rc<Current>, after_reconnect: bool) -> bool {
        if !self.still(current) {
            return true;
        }
        let Some(endpoint) = self.endpoint.borrow().clone() else { return false };
        let signal = abort::any([self.lifetime.clone(), abort::timeout(CATCH_UP_MS)]);
        let read = match read_snapshot(&endpoint, self.project, &signal).await {
            Ok(read) => read,
            // Too big to keep: there's no catch-up for it, as for Live's.
            Err(Unread::TooBig(_)) => return true,
            Err(Unread::Later(_)) => return false,
        };
        // Read from another project than the one asked about (it was switched meanwhile): that one's own look catches up.
        if !self.still(current) || identity_of(&read.project).as_deref() != Some(current.id.as_str()) {
            return true;
        }
        let artifact = artifact_of(&read.snapshot);
        let Ok(baseline) = store.load_set(&current.key, &current.path).await else { return false };
        if let Some(baseline) = baseline {
            // The same as last time: nothing changed. Otherwise what changed, if the DAW tells anything of it.
            let described = match baseline.pages.first() {
                _ if baseline.artifact_id == artifact => Some(DescribedDiff { lines: vec![], more: 0 }),
                Some(before) => Some(describe_changes(&(self.project.changes)(before, &read.snapshot, false), self.project.words, None))
                    .filter(|described| !described.lines.is_empty()),
                None => None,
            };
            if let Some(described) = described.filter(|_| self.still(current)) {
                let summary = CatchUp {
                    set: current.name.clone(),
                    last_seen_at: baseline.saved_at,
                    lines: described.lines,
                    more: described.more,
                    after_reconnect: after_reconnect.then_some(true),
                };
                let mut context = JsonObject::new();
                context.insert("lastSeen".into(), json!(since(baseline.saved_at as f64, self.now_ms() as f64)));
                context.insert("changes".into(), json!(summary.lines));
                if summary.more != 0 {
                    context.insert("more".into(), json!(summary.more));
                }
                *self.context.borrow_mut() = Some(context);
                if let Some(callback) = &self.on_catch_up {
                    let _ = catch_unwind(AssertUnwindSafe(|| callback(summary)));
                }
            }
        }
        if self.still(current) {
            self.keep(store, current, read.snapshot, artifact).await;
        }
        true
    }

    /// Keeps the project as it is now as its baseline.
    async fn keep(&self, store: &Rc<dyn ProjectStore>, current: &Current, snapshot: JsonObject, artifact: String) {
        let baseline = Baseline {
            version: 1,
            path: current.path.clone(),
            name: current.name.clone(),
            saved_at: self.now_ms(),
            artifact_id: artifact,
            pages: vec![snapshot],
        };
        if store.save(&current.key, &baseline).await.is_ok() {
            self.last_saved.set(kumi_common::time::now_ms());
        }
    }

    /// Saves the project open as its baseline, once it's been caught up on (so nothing untold is lost), when the DAW
    /// answers within `bound_ms`.
    fn save_now(self: &Rc<Self>, bound_ms: u64) -> Saving {
        self.enqueue(move |this| {
            async move {
                let (Some(current), Some(store)) = (this.current(), this.store.clone()) else { return };
                if !current.kept || !current.caught_up.get() || !this.connected.get() {
                    return;
                }
                let Some(endpoint) = this.endpoint.borrow().clone() else { return };
                let signal = abort::any([this.lifetime.clone(), abort::timeout(bound_ms)]);
                let Ok(read) = read_snapshot(&endpoint, this.project, &signal).await else { return };
                if this.still(&current) && identity_of(&read.project).as_deref() == Some(current.id.as_str()) {
                    let artifact = artifact_of(&read.snapshot);
                    this.keep(&store, &current, read.snapshot, artifact).await;
                }
            }
            .boxed_local()
        })
    }

    fn cancel_timer(&self) {
        if let Some(timer) = self.timer.borrow_mut().take() {
            timer.cancel();
        }
    }

    /// Saves the baseline `delay` ms from now (a later call moves it).
    fn schedule_save(self: &Rc<Self>, delay: u64) {
        if self.store.is_none() || self.lifetime.is_cancelled() {
            return;
        }
        self.cancel_timer();
        let stop = Signal::new();
        *self.timer.borrow_mut() = Some(stop.clone());
        let weak = Rc::downgrade(self);
        tokio::task::spawn_local(async move {
            tokio::select! {
                biased;
                _ = stop.cancelled() => {}
                _ = tokio::time::sleep(Duration::from_millis(delay)) => {
                    if let Some(this) = weak.upgrade() {
                        drop(this.save_now(SAVE_MS));
                    }
                }
            }
        });
    }

    /// Kumi changed the project: the baseline is saved soon, so the next catch-up doesn't tell Kumi's own changes as
    /// the producer's.
    pub fn keep_soon(self: &Rc<Self>) {
        if self.current().is_some_and(|current| current.kept) {
            self.schedule_save(AFTER_CHANGES_MS);
        }
    }

    /// Kumi closes: the baseline is saved if the DAW answers at once.
    pub async fn close(self: &Rc<Self>) {
        self.cancel_timer();
        if self.current().is_some_and(|current| current.kept && current.caught_up.get()) && self.connected.get() {
            let _ = tokio::time::timeout(Duration::from_millis(CLOSE_WAIT_MS), self.save_now(CLOSE_SAVE_MS)).await;
        }
    }

    fn watching_now(&self, on: bool) {
        if let Some(listener) = &self.on_watch {
            let _ = catch_unwind(AssertUnwindSafe(|| listener(on)));
        }
    }

    /// watch_me, offered with the DAW's other tools.
    pub fn watch_tool(self: &Rc<Self>) -> Rc<dyn KernelTool> {
        Rc::new(WatchTool { remember: self.clone() })
    }

    /// watch_me's start (the project as it is) or stop (what changed since): its answer, or why not.
    async fn watch(&self, start: bool, signal: &Signal) -> Result<String, String> {
        let (app, place) = (self.profile.words.app, self.profile.words.place);
        let endpoint = self.endpoint.borrow().clone().filter(|_| self.connected.get());
        let Some(endpoint) = endpoint else { return Err(format!("{app} isn't connected, so Kumi can't watch it.")) };
        if !start && self.watching.borrow().is_none() {
            return Err("Kumi isn't watching yet: start first, before the producer does it.".into());
        }
        let read = read_snapshot(&endpoint, self.project, signal).await.map_err(|unread| match unread {
            Unread::TooBig(kind) => {
                format!("The {place} holds more of {kind} than Kumi can compare yet, so it can't learn routines by watching it.")
            }
            Unread::Later(why) => format!("Kumi couldn't read the {place} just now ({why}); try again."),
        })?;
        let project = identity_of(&read.project);
        if start {
            *self.watching.borrow_mut() = Some(Watching { snapshot: read.snapshot, project, at: self.now_ms() });
            self.watching_now(true);
            return Ok(stringify(
                &json!({"watching": true, "note": format!("Tell the producer to go ahead in {app} and to say when they're done.")}),
            ));
        }
        let Some(before) = self.watching.borrow_mut().take() else {
            return Err("Kumi isn't watching yet: start first, before the producer does it.".into());
        };
        self.watching_now(false);
        if before.project != project {
            return Err(format!("A different {place} is open now, so there's nothing to compare; start again."));
        }
        let described = describe_watch_changes(&(self.project.changes)(&before.snapshot, &read.snapshot, true), self.project.words, None);
        let seconds = to_string(round((self.now_ms() - before.at) as f64 / 1000.0));
        let mut reply = json!({"watched": format!("{seconds} s"), "changes": described.changes});
        if described.more > 0 {
            reply["more"] = json!(described.more);
        }
        if described.changes.is_empty() {
            reply["note"] = json!(format!("Nothing in the {place} changed while Kumi watched."));
        }
        Ok(stringify(&reply))
    }
}

/// watch_me: the producer's routine, learned by comparing the project before and after they do it.
struct WatchTool {
    remember: Rc<Remember>,
}

#[async_trait(?Send)]
impl KernelTool for WatchTool {
    fn name(&self) -> &str {
        WATCH_TOOL
    }
    fn description(&self) -> &str {
        self.remember.project.watch_description
    }
    fn input_schema(&self) -> JsonObject {
        json!({"type": "object", "additionalProperties": false, "required": ["action"], "properties": {"action": {"type": "string", "enum": ["start", "stop"]}}})
            .as_object()
            .cloned()
            .unwrap_or_default()
    }
    async fn execute(&self, input: JsonObject, signal: Signal) -> Result<ToolResult, RuntimeError> {
        signal.check()?;
        let bounded = abort::any([signal.clone(), self.remember.lifetime.clone(), abort::timeout(WATCH_MS)]);
        let start = input.get("action").and_then(Value::as_str) == Some("start");
        let outcome = self.remember.watch(start, &bounded).await;
        signal.check()?;
        Ok(match outcome {
            Ok(text) => ToolResult::text(text),
            Err(why) => ToolResult::error(why),
        })
    }
    fn reach(&self) -> Reach {
        Reach::DawRead
    }
    fn deferred(&self) -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::{daw::DawWords, describe::ProjectItem, project::create_project_store};
    use crate::mcp::{
        client::StderrStatus,
        types::{CallToolResult, Implementation, ListToolsResult},
    };
    use std::path::PathBuf;

    const ID: &str = "0123456789abcdef0123456789abcdef";
    const PATH: &str = "/songs/Beat.song";

    struct Rules;
    impl ProjectRules for Rules {
        fn template_location(&self, path: &str) -> bool {
            path.contains("/Templates/")
        }
        fn project_folder(&self, _path: &str) -> Option<PathBuf> {
            None
        }
    }
    const WORDS: ProjectWords = ProjectWords { kinds: &[("track", "tracks")], project: "song", rank: &["song", "track"], opaque: &[] };
    /// A song's tracks by name.
    fn names(rows: &[KindRows]) -> Result<JsonObject, String> {
        let tracks: Vec<Value> =
            rows.iter().filter(|(kind, _)| *kind == "track").flat_map(|(_, rows)| rows.iter()).map(|row| row["name"].clone()).collect();
        Ok(json!({"tracks": tracks}).as_object().cloned().unwrap_or_default())
    }
    /// The tracks added, by name.
    fn added(before: &JsonObject, after: &JsonObject, _watching: bool) -> Vec<ProjectChange> {
        let was = before["tracks"].as_array().cloned().unwrap_or_default();
        after["tracks"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|name| !was.contains(name))
            .map(|name| ProjectChange::Added {
                kind: "track".into(),
                item: ProjectItem { name: Some(name.clone()), order: None, on: None },
                made_of: JsonObject::new(),
            })
            .collect()
    }
    const SONG: OwnProject = OwnProject {
        rules: &Rules,
        words: &WORDS,
        reads: &["project", "track"],
        snapshot: names,
        changes: added,
        watch_description: "Watch.",
        bulk: false,
    };
    const FAKE: DawProfile = DawProfile {
        id: Some(DawId::Reaper),
        words: DawWords { app: "Fakeloop", place: "song", places: "songs", master: "the master track", timeline: "timeline" },
        ..DawProfile::REAPER
    };

    /// A bridge that answers a song's rows, or that it's busy.
    #[derive(Default)]
    struct Bridge {
        tracks: RefCell<Vec<&'static str>>,
        busy: Cell<bool>,
        reads: Cell<usize>,
    }
    #[async_trait(?Send)]
    impl McpEndpoint for Bridge {
        fn pid(&self) -> Option<u32> {
            None
        }
        fn server_info(&self) -> Option<Implementation> {
            None
        }
        async fn list(&self, _: Option<&str>, _: Signal) -> Result<ListToolsResult, RuntimeError> {
            Ok(serde_json::from_value(json!({"tools": []})).unwrap())
        }
        async fn call(&self, _name: &str, args: JsonObject, _: Signal) -> Result<CallToolResult, RuntimeError> {
            self.reads.set(self.reads.get() + 1);
            let (text, error) = if self.busy.get() {
                (json!({"error": "a dialog is open", "code": "busy"}), true)
            } else {
                let items = match args["kind"].as_str() {
                    Some("project") => json!([{"id": ID, "identity": "Beat", "path": PATH}]),
                    _ => json!(self.tracks.borrow().iter().map(|name| json!({"name": name})).collect::<Vec<_>>()),
                };
                (json!({"epoch": "1", "items": items}), false)
            };
            Ok(serde_json::from_value(json!({"content": [{"type": "text", "text": text.to_string()}], "isError": error})).unwrap())
        }
        fn on_catalog_changed(&self, _: Rc<dyn Fn()>) -> Box<dyn Fn()> {
            Box::new(|| {})
        }
        fn on_disconnect(&self, _: Rc<dyn Fn()>) -> Box<dyn Fn()> {
            Box::new(|| {})
        }
        fn stderr_status(&self) -> StderrStatus {
            StderrStatus { bytes: 0, truncated: false }
        }
        async fn close(&self) -> Result<(), RuntimeError> {
            Ok(())
        }
    }

    /// What a session keeps of the song, over `bridge`, with `store`; and what it told.
    fn remember(bridge: &Rc<Bridge>, store: &Rc<dyn ProjectStore>) -> (Rc<Remember>, Rc<RefCell<Vec<CatchUp>>>) {
        let told: Rc<RefCell<Vec<CatchUp>>> = Rc::default();
        let mut context = IntegrationContext::new(Rc::new(|_, _| {}));
        let into = told.clone();
        context.on_catch_up = Some(Rc::new(move |catch_up| into.borrow_mut().push(catch_up)));
        let endpoint: Rc<dyn McpEndpoint> = bridge.clone();
        let remember = Remember::new(
            &SONG,
            &FAKE,
            Some(store.clone()),
            Rc::new(RefCell::new(Some(endpoint))),
            Rc::new(Cell::new(true)),
            Signal::new(),
            &context,
        );
        (remember, told)
    }

    fn seen(path: &str, came_back: bool) -> Seen<'_> {
        Seen { id: Some(ID), path: Some(path), name: "Beat", came_back, playing: false }
    }

    async fn settle(remember: &Rc<Remember>) {
        let pending = remember.queue.borrow().clone();
        pending.await;
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_song_is_caught_up_on_as_it_opens_again_when_its_daw_was_busy_and_when_the_daw_comes_back() {
        tokio::task::LocalSet::new()
            .run_until(async {
                let dir = tempfile::tempdir().unwrap();
                let store: Rc<dyn ProjectStore> = create_project_store(dir.path());
                let bridge = Rc::new(Bridge::default());
                bridge.tracks.borrow_mut().push("Kick");
                let key = format!("reaper-{ID}");
                // First seen: kept, nothing told.
                let (first, told) = remember(&bridge, &store);
                first.seen(seen(PATH, true));
                settle(&first).await;
                assert!(store.load(&key).await.unwrap().is_some() && told.borrow().is_empty());
                // The next session, the DAW busy as it opens: nothing kept or told yet, and tried again with the next look.
                bridge.tracks.borrow_mut().push("Bass");
                let (second, told) = remember(&bridge, &store);
                bridge.busy.set(true);
                second.seen(seen(PATH, false));
                settle(&second).await;
                assert!(told.borrow().is_empty() && second.context().is_none());
                bridge.busy.set(false);
                second.seen(seen(PATH, false));
                settle(&second).await;
                assert_eq!(told.borrow().iter().map(|caught| caught.lines.clone()).collect::<Vec<_>>(), [["Added track “Bass”"]]);
                assert_eq!(told.borrow()[0].after_reconnect, None);
                assert_eq!(second.context().map(|context| context["changes"].clone()), Some(json!(["Added track “Bass”"])));
                // Seen again: nothing more to tell, nor read until a save is due.
                let reads = bridge.reads.get();
                second.seen(seen(PATH, false));
                settle(&second).await;
                assert_eq!((told.borrow().len(), bridge.reads.get()), (1, reads));
                // The DAW went away and came back on the same song: what changed meanwhile is told as such.
                bridge.tracks.borrow_mut().push("Pad");
                second.seen(seen(PATH, true));
                settle(&second).await;
                assert_eq!(told.borrow().len(), 2);
                assert_eq!(
                    (told.borrow()[1].lines.clone(), told.borrow()[1].after_reconnect),
                    (vec!["Added track “Pad”".to_string()], Some(true))
                );
                // An unsaved song: what was told of the saved one goes.
                second.seen(Seen { id: None, path: None, name: "untitled", came_back: false, playing: false });
                assert!(second.context().is_none());
            })
            .await;
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_template_is_neither_caught_up_on_nor_kept() {
        tokio::task::LocalSet::new()
            .run_until(async {
                let dir = tempfile::tempdir().unwrap();
                let store: Rc<dyn ProjectStore> = create_project_store(dir.path());
                let bridge = Rc::new(Bridge::default());
                let (remember, told) = remember(&bridge, &store);
                remember.seen(seen("/songs/Templates/Band.song", false));
                settle(&remember).await;
                assert_eq!(store.load(&format!("reaper-{ID}")).await.unwrap(), None);
                assert!(told.borrow().is_empty());
                assert_eq!(bridge.reads.get(), 0, "not even read");
            })
            .await;
    }

    #[test]
    fn a_snapshots_fingerprint_is_its_content() {
        let one = json!({"tracks": ["Kick"]}).as_object().unwrap().clone();
        let two = json!({"tracks": ["Kick", "Bass"]}).as_object().unwrap().clone();
        assert_eq!(artifact_of(&one), artifact_of(&one.clone()));
        assert_ne!(artifact_of(&one), artifact_of(&two));
        assert_eq!(artifact_of(&one).len(), 32);
    }
}
