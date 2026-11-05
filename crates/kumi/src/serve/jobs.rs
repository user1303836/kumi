//! Background jobs: work that outlasts the call that started it (a library scan), as C1's `job` and C3's
//! `jobs/<id>.json` say it. Each runs as a task of its own on the runtime's thread; its state, progress and result are
//! kept for `job` and written to `jobs/<id>.json` for the host's UI.
//!
//! When the runtime stops, every job is cancelled and given the runtime's settling time; one still running then is
//! written `interrupted`. A job a runtime left `running` (it was killed) reads `interrupted` at the next start. Nothing is
//! resumed or replayed by itself: only the operation that owns a job's kind resumes one, by its id ([`Jobs::resume`]).

use super::ctx::{Ctx, OpError};
use super::snapshots::Snapshots;
use futures::FutureExt;
use kumi_common::abort::{self, Signal};
use serde_json::{json, Value};
use std::cell::{Cell, RefCell};
use std::collections::{HashMap, VecDeque};
use std::future::Future;
use std::panic::AssertUnwindSafe;
use std::path::Path;
use std::rc::Rc;
use std::time::{Duration, Instant};
use tokio::sync::Notify;

/// How often a job's progress is written to its file at most (its state changes are written at once).
const WRITE_EVERY: Duration = Duration::from_millis(500);
/// How many recent jobs are kept. Unsettled generation receipts additionally remain on disk.
const KEPT: usize = 100;
/// How many jobs may run at once: another is refused `busy` until one ends.
pub const RUNNING: usize = 8;
/// The most bytes a job's checkpoint takes, as JSON.
pub const CHECKPOINT_MOST: usize = 16 * 1024;

/// Where a job is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    Running,
    Done,
    Failed,
    /// Stopped before it finished: the runtime stopped or the producer cancelled this owned job.
    Cancelled,
    /// A runtime stopped without settling it: found `running` at the next start.
    Interrupted,
}

impl State {
    pub fn as_str(self) -> &'static str {
        match self {
            State::Running => "running",
            State::Done => "done",
            State::Failed => "failed",
            State::Cancelled => "cancelled",
            State::Interrupted => "interrupted",
        }
    }
}

struct Job {
    kind: &'static str,
    state: Cell<State>,
    /// `{done, total?, message?}`.
    progress: RefCell<Value>,
    outcome: RefCell<Option<Result<Value, OpError>>>,
    /// What the job has come to so far ([`JobHandle::checkpoint`]), while it runs and beside an error.
    checkpoint: RefCell<Option<Value>>,
    /// The secrets of the call that started it ([`super::ctx::SECRET_FIELDS`]): never in a checkpoint.
    secrets: Vec<String>,
    signal: Signal,
    written: Cell<Option<Instant>>,
}

impl Job {
    fn view(&self, id: &str) -> Value {
        let mut view =
            json!({"job_id": id, "kind": self.kind, "state": self.state.get().as_str(), "progress": self.progress.borrow().clone()});
        match &*self.outcome.borrow() {
            Some(Ok(result)) => view["result"] = result.clone(),
            Some(Err(error)) => view["error"] = error.to_value()["error"].clone(),
            None => {}
        }
        // What it came to before it failed, was cancelled or interrupted, or so far while it runs.
        if view.get("result").is_none() {
            if let Some(kept) = &*self.checkpoint.borrow() {
                view["result"] = kept.clone();
            }
        }
        view
    }
}

/// The runtime's jobs.
pub struct Jobs {
    jobs: RefCell<HashMap<String, Rc<Job>>>,
    /// Their ids, oldest first.
    order: RefCell<VecDeque<String>>,
    next: Cell<u64>,
    running: Cell<usize>,
    /// Told when the last running job ends.
    settled: Notify,
}

impl Jobs {
    /// The jobs of a runtime starting with `snapshots`: a job an earlier runtime left `running` now reads
    /// `interrupted`. Old ordinary jobs are pruned; unresolved billing and failed delivery are retained.
    /// (CC-6: its parent holds the folder's lock), so no job found here is another live runtime's.
    pub(crate) fn new(snapshots: &Snapshots, state_dir: &Path) -> Jobs {
        settle_earlier(snapshots, state_dir);
        Jobs {
            jobs: RefCell::new(HashMap::new()),
            order: RefCell::new(VecDeque::new()),
            next: Cell::new(0),
            running: Cell::new(0),
            settled: Notify::new(),
        }
    }

    /// Starts `work` as a job of `kind` (`scan`, say): its id at once. `work` gets the runtime and the job's handle,
    /// whose signal fires when the runtime stops; what it returns is the job's result (an [`OpError`]: it failed).
    /// `secrets` are the starting call's, refused in a checkpoint. Refused `busy` while [`RUNNING`] jobs run.
    pub(crate) fn start<W, F>(&self, ctx: Rc<Ctx>, kind: &'static str, secrets: Vec<String>, work: W) -> Result<String, OpError>
    where
        W: FnOnce(Rc<Ctx>, JobHandle) -> F + 'static,
        F: Future<Output = Result<Value, OpError>> + 'static,
    {
        if self.running.get() >= RUNNING {
            let reason = format!("{RUNNING} jobs are running already: wait for one to finish (job tells when)");
            return Err(OpError::Refused { code: "busy", reason, owner: None });
        }
        let n = self.next.get() + 1;
        self.next.set(n);
        let id = format!("j{}-{n}", kumi_common::time::now_ms());
        self.order.borrow_mut().push_back(id.clone());
        self.run(ctx, id.clone(), kind, json!({"done": 0}), None, secrets, work);
        Ok(id)
    }

    /// Runs `work` again as the job `id` of `kind`, which isn't running: one this runtime ran, or one an earlier runtime
    /// left in its file (interrupted, failed, cancelled or done). Whether a settled job may run again is the operation
    /// that owns its kind's to decide (a generation the vendor took, say, fetched by its id again); the job keeps its id,
    /// its progress and what it had come to (its last checkpoint, or its result) until the new run says otherwise.
    /// False when it's running already; refused `busy` while [`RUNNING`] jobs run, and `not_resumable` for no job of
    /// that id and kind. `secrets` as [`Jobs::start`]'s.
    pub(crate) fn resume<W, F>(&self, ctx: Rc<Ctx>, id: &str, kind: &'static str, secrets: Vec<String>, work: W) -> Result<bool, OpError>
    where
        W: FnOnce(Rc<Ctx>, JobHandle) -> F + 'static,
        F: Future<Output = Result<Value, OpError>> + 'static,
    {
        let not_resumable =
            || OpError::Refused { code: "not_resumable", reason: format!("There's no {kind} job by that id to resume."), owner: None };
        let known = self.jobs.borrow().get(id).cloned();
        let (progress, came_to) = match known {
            Some(job) if job.kind != kind => return Err(not_resumable()),
            Some(job) if job.state.get() == State::Running => return Ok(false),
            Some(job) => {
                let came_to = match &*job.outcome.borrow() {
                    Some(Ok(result)) => Some(result.clone()),
                    _ => job.checkpoint.borrow().clone(),
                };
                (job.progress.borrow().clone(), came_to)
            }
            None => {
                let kept = self.get(id, &ctx.paths().state_dir).ok_or_else(not_resumable)?;
                if kept["kind"] != kind {
                    return Err(not_resumable());
                }
                self.order.borrow_mut().push_back(id.to_string());
                (kept["progress"].clone(), kept.get("result").cloned())
            }
        };
        if self.running.get() >= RUNNING {
            let reason = format!("{RUNNING} jobs are running already: wait for one to finish (job tells when)");
            return Err(OpError::Refused { code: "busy", reason, owner: None });
        }
        self.run(ctx, id.to_string(), kind, progress, came_to, secrets, work);
        Ok(true)
    }

    /// Runs `work` as the job `id` (in [`Jobs::order`] already), from `progress` and what it had `come_to`.
    #[allow(clippy::too_many_arguments)]
    fn run<W, F>(
        &self,
        ctx: Rc<Ctx>,
        id: String,
        kind: &'static str,
        progress: Value,
        came_to: Option<Value>,
        secrets: Vec<String>,
        work: W,
    ) where
        W: FnOnce(Rc<Ctx>, JobHandle) -> F + 'static,
        F: Future<Output = Result<Value, OpError>> + 'static,
    {
        let own = Signal::new();
        let job = Rc::new(Job {
            kind,
            state: Cell::new(State::Running),
            progress: RefCell::new(progress),
            outcome: RefCell::new(None),
            checkpoint: RefCell::new(came_to),
            secrets,
            signal: own.clone(),
            written: Cell::new(None),
        });
        self.jobs.borrow_mut().insert(id.clone(), job.clone());
        self.running.set(self.running.get() + 1);
        write(ctx.snapshots(), &id, &job);
        let handle = JobHandle { id: id.clone(), job: job.clone(), ctx: ctx.clone(), signal: abort::any([own, ctx.shutdown().clone()]) };
        let started = id.clone();
        let worker = ctx.clone();
        tokio::task::spawn_local(async move {
            // `work` is called in here, so a panic before its own future starts fails this job too, never the slot.
            let outcome = AssertUnwindSafe(async move { work(worker, handle).await }).catch_unwind().await.unwrap_or_else(|_| {
                super::say(&format!("kumi serve: job {started} ({kind}) failed unexpectedly"));
                Err(OpError::Failed { reason: format!("the {kind} failed unexpectedly; nothing more is known") })
            });
            let state = match &outcome {
                Ok(_) => State::Done,
                Err(_) if job.signal.is_cancelled() || ctx.shutdown().is_cancelled() => State::Cancelled,
                Err(_) => State::Failed,
            };
            job.state.set(state);
            *job.outcome.borrow_mut() = Some(outcome);
            write(ctx.snapshots(), &started, &job);
            let jobs = ctx.jobs();
            jobs.running.set(jobs.running.get() - 1);
            jobs.forget_oldest(&ctx.paths().state_dir).await;
            if jobs.running.get() == 0 {
                jobs.settled.notify_waiters();
            }
        });
    }

    /// Evict old settled jobs from memory. A generation file stays until its ledger accounts for it;
    /// even a charged job must retain its GET-only recovery route if output delivery failed.
    async fn forget_oldest(&self, state_dir: &Path) {
        let mut generations = Vec::new();
        while self.order.borrow().len() > KEPT {
            let oldest = {
                let jobs = self.jobs.borrow();
                let order = self.order.borrow();
                order.iter().position(|id| jobs.get(id).is_some_and(|job| job.state.get() != State::Running))
            };
            let Some(at) = oldest else { break };
            let Some(id) = self.order.borrow_mut().remove(at) else { break };
            let removed = self.jobs.borrow_mut().remove(&id);
            if removed.as_ref().is_some_and(|job| job.kind == "generate") {
                generations.push((id, removed.is_some_and(|job| job.state.get() == State::Done)));
            } else {
                let _ = std::fs::remove_file(state_dir.join("jobs").join(format!("{id}.json")));
            }
        }
        if !generations.is_empty() {
            let state_dir = state_dir.to_owned();
            let _ = tokio::task::spawn_blocking(move || {
                let Ok(authority) = super::generate_ledger::Authority::read(&state_dir) else { return };
                for (id, delivered) in generations {
                    if can_prune_generation(&authority, &id, delivered) {
                        let _ = std::fs::remove_file(state_dir.join("jobs").join(format!("{id}.json")));
                    }
                }
            })
            .await;
        }
    }

    /// The jobs still running when the runtime stops anyway: each written `interrupted`, so no file says `running`
    /// for a job nothing runs.
    pub(crate) fn interrupt_running(&self, snapshots: &Snapshots) {
        for (id, job) in self.jobs.borrow().iter() {
            if job.state.get() == State::Running {
                job.state.set(State::Interrupted);
                write(snapshots, id, job);
            }
        }
    }

    /// Signal only this runtime's matching running job. A repeated request while it settles is acknowledged;
    /// the worker's ordinary completion path records cancellation and releases the running slot.
    pub fn cancel(&self, id: &str, kind: &str) -> bool {
        let jobs = self.jobs.borrow();
        let Some(job) = jobs.get(id).filter(|job| job.kind == kind && job.state.get() == State::Running) else {
            return false;
        };
        job.signal.cancel();
        true
    }

    /// Job `id` as `job` answers: `{job_id, kind, state, progress, result?, error?}`. One from an earlier run is read
    /// from its file. None: no job of that id.
    pub fn get(&self, id: &str, state_dir: &Path) -> Option<Value> {
        if let Some(job) = self.jobs.borrow().get(id) {
            return Some(job.view(id));
        }
        let named = id.len() <= 64 && id.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-');
        if !named {
            return None;
        }
        let mut kept: Value = serde_json::from_slice(&std::fs::read(state_dir.join("jobs").join(format!("{id}.json"))).ok()?).ok()?;
        let fields = kept.as_object_mut()?;
        for envelope in ["schema", "seq", "at", "owner"] {
            fields.remove(envelope);
        }
        Some(kept)
    }

    /// Waits until no job runs.
    pub(crate) async fn all_settled(&self) {
        loop {
            let settled = self.settled.notified();
            if self.running.get() == 0 {
                return;
            }
            settled.await;
        }
    }

    /// The jobs still running.
    pub(crate) fn running(&self) -> usize {
        self.running.get()
    }
}

/// What a job's work is given: its signal, and its progress to report.
pub struct JobHandle {
    id: String,
    job: Rc<Job>,
    ctx: Rc<Ctx>,
    /// Fires when the runtime stops: the work stops and settles, then returns.
    pub signal: Signal,
}

impl JobHandle {
    pub fn id(&self) -> &str {
        &self.id
    }

    /// `done` of `total` (when known), and what's under way; written to the job's file twice a second at most.
    pub fn report(&self, done: f64, total: Option<f64>, message: Option<&str>) {
        let mut progress = json!({"done": done});
        if let Some(total) = total {
            progress["total"] = json!(total);
        }
        if let Some(message) = message {
            progress["message"] = json!(message);
        }
        *self.job.progress.borrow_mut() = progress;
        if self.job.written.get().is_none_or(|at| at.elapsed() >= WRITE_EVERY) {
            write(self.ctx.snapshots(), &self.id, &self.job);
        }
    }

    /// What the job has come to so far (a paid job's reservation, whether it was submitted, the vendor's id): `job`
    /// gives it while the job runs, and beside its error when it fails, is cancelled or is interrupted. It's on disk
    /// before this returns, so a runtime that dies after it leaves it for the next start (CC-13's recovery).
    ///
    /// For the job's boundaries (before it submits, once the vendor accepted), not its progress ([`Self::report`]):
    /// each is a durable write. Refused [`OpError::Invalid`], with nothing kept, when it's over [`CHECKPOINT_MOST`]
    /// bytes as JSON or holds a secret of the call that started the job; a whole path in it is kept as its name.
    pub async fn checkpoint(&self, mut result: Value) -> Result<(), OpError> {
        let text = result.to_string();
        if text.len() > CHECKPOINT_MOST {
            return Err(OpError::Invalid { reason: format!("A job's checkpoint is at most {CHECKPOINT_MOST} bytes as JSON.") });
        }
        // As given and as JSON writes it (a secret with a quote or a backslash in it is escaped there).
        let held = self.job.secrets.iter().any(|secret| {
            let quoted = Value::String(secret.clone()).to_string();
            text.contains(secret.as_str()) || text.contains(&quoted[1..quoted.len() - 1])
        });
        if held {
            return Err(OpError::Invalid { reason: "A job's checkpoint never holds the key the host gave its call.".into() });
        }
        super::connection::names_only(&mut result);
        *self.job.checkpoint.borrow_mut() = Some(result);
        self.job.written.set(Some(Instant::now()));
        let view = self.job.view(&self.id);
        match self.ctx.snapshots().write_durably(&format!("jobs/{}", self.id), view).await {
            Ok(_) => Ok(()),
            Err(error) => Err(OpError::Failed { reason: format!("Kumi couldn't keep the job's state on disk: {error}") }),
        }
    }
}

fn write(snapshots: &Snapshots, id: &str, job: &Job) {
    job.written.set(Some(Instant::now()));
    if let Err(error) = snapshots.write(&format!("jobs/{id}"), &job.view(id)) {
        super::say(&format!("kumi serve: jobs/{id}.json not written: {error}"));
    }
}

fn can_prune_generation(authority: &super::generate_ledger::Authority, id: &str, delivered: bool) -> bool {
    authority
        .for_job(id)
        .ok()
        .flatten()
        .is_some_and(|record| record.settlement == "not_submitted" || (record.settlement == "charged" && delivered))
}

/// Old ordinary jobs are pruned; unaccounted generation receipts and failed deliveries survive.
fn settle_earlier(snapshots: &Snapshots, state_dir: &Path) {
    let folder = state_dir.join("jobs");
    let Ok(entries) = std::fs::read_dir(&folder) else { return };
    let mut files: Vec<(std::time::SystemTime, std::path::PathBuf)> = entries
        .filter_map(Result::ok)
        .filter(|entry| entry.file_name().to_string_lossy().ends_with(".json"))
        .map(|entry| (entry.metadata().and_then(|meta| meta.modified()).unwrap_or(std::time::UNIX_EPOCH), entry.path()))
        .collect();
    files.sort_by(|a, b| b.0.cmp(&a.0));
    let mut authority = None;
    for (n, (_, path)) in files.iter().enumerate() {
        let Some(id) = path.file_stem().map(|stem| stem.to_string_lossy().into_owned()) else { continue };
        let Ok(mut kept) = std::fs::read(path).map_err(|_| ()).and_then(|bytes| serde_json::from_slice::<Value>(&bytes).map_err(|_| ()))
        else {
            continue;
        };
        if n >= KEPT {
            let prune = if kept["kind"] == "generate" {
                let authority = authority.get_or_insert_with(|| super::generate_ledger::Authority::read(state_dir));
                authority.as_ref().is_ok_and(|authority| can_prune_generation(authority, &id, kept["state"] == "done"))
            } else {
                true
            };
            if prune {
                let _ = std::fs::remove_file(path);
                continue;
            }
        }
        if kept["state"] == State::Running.as_str() {
            kept["state"] = json!(State::Interrupted.as_str());
            if let Err(error) = snapshots.write(&format!("jobs/{id}"), &kept) {
                super::say(&format!("kumi serve: jobs/{id}.json not written: {error}"));
            }
        }
    }
    // On disk before `job` can read one of them back.
    snapshots.flush();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::serve::Args;
    use kumi_types::daw::DawId;
    use kumi_types::system::Env;
    use std::collections::BTreeMap;

    fn runtime(state: &Path) -> Rc<Ctx> {
        let args = Args {
            state_dir: state.to_path_buf(),
            kumi_home: state.join("home"),
            hermes_home: None,
            hermes_root: None,
            daw: DawId::Reaper,
            library_roots: vec![],
            media_roots: vec![],
            tools: BTreeMap::new(),
        };
        Ctx::new(args, Env::new(), Default::default()).unwrap()
    }

    /// Job `id`'s file once every write so far is done.
    fn file(ctx: &Ctx, id: &str) -> Value {
        ctx.snapshots().flush();
        serde_json::from_slice(&std::fs::read(ctx.paths().state_dir.join("jobs").join(format!("{id}.json"))).unwrap()).unwrap()
    }

    async fn local<F: Future<Output = ()>>(test: F) {
        tokio::task::LocalSet::new().run_until(test).await;
    }

    #[tokio::test]
    async fn a_job_resumes_under_its_own_id_from_what_it_came_to_once_and_only_as_its_own_kind() {
        local(async {
            let dir = tempfile::tempdir().unwrap();
            // An earlier runtime's generation the vendor took, then the runtime died.
            let earlier = runtime(dir.path());
            let id = earlier
                .start_job("generate", |_ctx, job| async move {
                    job.checkpoint(json!({"submission": "submitted", "provider_job_id": "v-9"})).await?;
                    std::future::pending::<()>().await;
                    Ok(json!({}))
                })
                .unwrap();
            tokio::task::yield_now().await;
            tokio::task::yield_now().await;
            earlier.snapshots().flush();
            let ctx = runtime(dir.path());
            assert_eq!(ctx.jobs().get(&id, dir.path()).unwrap()["state"], "interrupted");
            // As another kind, or a job there isn't: not resumable.
            for (other, kind) in [(id.as_str(), "scan"), ("j1-999", "generate")] {
                let refused = ctx.resume_job(other, kind, |_ctx, _job| async { Ok(json!({})) });
                assert!(matches!(refused, Err(OpError::Refused { code: "not_resumable", .. })), "{other} as {kind}");
            }
            let (go, gone) = tokio::sync::oneshot::channel::<()>();
            let (told, said) = tokio::sync::oneshot::channel::<String>();
            let resumed = ctx
                .resume_job(&id, "generate", move |_ctx, job| async move {
                    let _ = told.send(job.id().to_string());
                    let _ = gone.await;
                    Ok(json!({"submission": "submitted", "provider_job_id": "v-9", "delivered": true}))
                })
                .unwrap();
            assert!(resumed);
            assert_eq!(said.await.unwrap(), id, "under its own id");
            let running = ctx.jobs().get(&id, dir.path()).unwrap();
            assert_eq!(
                (running["state"].clone(), running["result"]["provider_job_id"].clone()),
                (json!("running"), json!("v-9")),
                "running again, from what it had come to"
            );
            // Asked again while it runs: false, and nothing else runs.
            assert!(!ctx.resume_job(&id, "generate", |_ctx, _job| async { Ok(json!({})) }).unwrap());
            go.send(()).unwrap();
            ctx.jobs().all_settled().await;
            let done = file(&ctx, &id);
            assert_eq!((done["state"].clone(), done["result"]["delivered"].clone()), (json!("done"), json!(true)));
            // A settled job runs again too, when its owner asks: a failed one, under its id.
            let failing =
                ctx.start_job("generate", |_ctx, _job| async { Err(OpError::Failed { reason: "the download failed".into() }) }).unwrap();
            ctx.jobs().all_settled().await;
            assert_eq!(ctx.jobs().get(&failing, dir.path()).unwrap()["state"], "failed");
            assert!(ctx.resume_job(&failing, "generate", |_ctx, _job| async { Ok(json!({"delivered": true})) }).unwrap());
            ctx.jobs().all_settled().await;
            let again = ctx.jobs().get(&failing, dir.path()).unwrap();
            assert_eq!((again["state"].clone(), again.get("error")), (json!("done"), None), "the earlier error gone with the new run");
            earlier.jobs().interrupt_running(earlier.snapshots());
        })
        .await;
    }

    #[tokio::test]
    async fn a_job_says_its_progress_while_it_runs_and_its_result_when_done() {
        local(async {
            let dir = tempfile::tempdir().unwrap();
            let ctx = runtime(dir.path());
            let (go, gone) = tokio::sync::oneshot::channel::<()>();
            let id = ctx
                .start_job("scan", |_ctx, job| async move {
                    job.report(1.0, Some(4.0), Some("Drums"));
                    let _ = gone.await;
                    Ok(json!({"sounds": 4}))
                })
                .unwrap();
            assert_eq!(file(&ctx, &id)["state"], "running", "its file is there at once");
            tokio::task::yield_now().await;
            let running = ctx.jobs().get(&id, dir.path()).unwrap();
            assert_eq!((running["state"].clone(), running["kind"].clone()), (json!("running"), json!("scan")));
            assert_eq!(running["progress"], json!({"done": 1.0, "total": 4.0, "message": "Drums"}));
            go.send(()).unwrap();
            ctx.jobs().all_settled().await;
            let done = ctx.jobs().get(&id, dir.path()).unwrap();
            assert_eq!((done["state"].clone(), done["result"].clone()), (json!("done"), json!({"sounds": 4})));
            let written = file(&ctx, &id);
            assert_eq!(
                (written["state"].clone(), written["result"].clone(), written["schema"].clone()),
                (json!("done"), json!({"sounds": 4}), json!(1))
            );
        })
        .await;
    }

    #[tokio::test]
    async fn what_a_job_came_to_is_on_disk_at_once_and_outlasts_its_error_and_its_runtime() {
        local(async {
            let dir = tempfile::tempdir().unwrap();
            let ctx = runtime(dir.path());
            let (go, gone) = tokio::sync::oneshot::channel::<()>();
            let failing = ctx
                .start_job("generate", |_ctx, job| async move {
                    job.checkpoint(json!({"submission": "unknown", "reservation_id": "r-1"})).await?;
                    let _ = gone.await;
                    Err(OpError::Failed { reason: "the download failed".into() })
                })
                .unwrap();
            tokio::task::yield_now().await;
            // On disk before the work goes on, and given while it runs.
            assert_eq!(file(&ctx, &failing)["result"]["submission"], "unknown");
            assert_eq!(ctx.jobs().get(&failing, dir.path()).unwrap()["result"]["reservation_id"], "r-1");
            go.send(()).unwrap();
            ctx.jobs().all_settled().await;
            let failed = ctx.jobs().get(&failing, dir.path()).unwrap();
            assert_eq!(
                (failed["state"].clone(), failed["result"]["reservation_id"].clone()),
                (json!("failed"), json!("r-1")),
                "beside its error"
            );
            assert_eq!(failed["error"]["reason"], "the download failed");
            // A runtime that dies with a job running leaves what it came to for the next.
            let left = ctx
                .start_job("generate", |_ctx, job| async move {
                    job.checkpoint(json!({"submission": "submitted", "provider_job_id": "v-9"})).await?;
                    std::future::pending::<()>().await;
                    Ok(json!({}))
                })
                .unwrap();
            tokio::task::yield_now().await;
            tokio::task::yield_now().await;
            ctx.snapshots().flush();
            let later = runtime(dir.path());
            let found = later.jobs().get(&left, dir.path()).unwrap();
            assert_eq!((found["state"].clone(), found["result"]["provider_job_id"].clone()), (json!("interrupted"), json!("v-9")));
        })
        .await;
    }

    #[tokio::test]
    async fn a_checkpoint_is_bounded_keeps_names_not_paths_and_never_the_calls_key() {
        const KEY: &str = "sk-sentinel-c13";
        local(async {
            let dir = tempfile::tempdir().unwrap();
            let ctx = runtime(dir.path());
            let given = json!({"_mirelo": {"api_key": KEY, "acknowledged": true, "reservation_id": "r-7"}});
            let private = crate::serve::ctx::Private::from(given.as_object().unwrap().clone());
            let (tried, outcomes) = tokio::sync::oneshot::channel();
            // Started inside the call, as the adapter runs it: the job keeps the call's key.
            let id = crate::serve::ctx::CALL_PRIVATE
                .sync_scope(private, || {
                    ctx.start_job("generate", |_ctx, job| async move {
                        let mut said = vec![];
                        for result in [
                            json!({"note": "x".repeat(CHECKPOINT_MOST)}),
                            json!({"submission": "unknown", "echo": format!("Bearer {KEY}")}),
                            json!({"submission": "unknown", "reservation_id": "r-7", "file": "/Users/someone/Music/kick.wav"}),
                        ] {
                            said.push(job.checkpoint(result).await);
                        }
                        let _ = tried.send(said);
                        std::future::pending::<()>().await;
                        Ok(json!({}))
                    })
                })
                .unwrap();
            let said = outcomes.await.unwrap();
            for refused in &said[..2] {
                let Err(OpError::Invalid { reason }) = refused else { panic!("refused invalid: {refused:?}") };
                assert!(!reason.contains(KEY), "{reason}");
            }
            assert!(said[2].is_ok(), "{:?}", said[2]);
            let kept = file(&ctx, &id)["result"].clone();
            assert_eq!(
                kept,
                json!({"submission": "unknown", "reservation_id": "r-7", "file": "kick.wav"}),
                "the last alone, its path a name"
            );
            let bytes = std::fs::read(ctx.paths().state_dir.join("jobs").join(format!("{id}.json"))).unwrap();
            assert!(!String::from_utf8_lossy(&bytes).contains(KEY));
            ctx.jobs().interrupt_running(ctx.snapshots());
        })
        .await;
    }

    #[tokio::test]
    async fn a_refused_checkpoint_writes_nothing() {
        local(async {
            let dir = tempfile::tempdir().unwrap();
            let ctx = runtime(dir.path());
            let given = json!({"_mirelo": {"api_key": "sk-sentinel-c13"}});
            let private = crate::serve::ctx::Private::from(given.as_object().unwrap().clone());
            let (tried, outcome) = tokio::sync::oneshot::channel();
            let id = crate::serve::ctx::CALL_PRIVATE
                .sync_scope(private, || {
                    ctx.start_job("generate", |_ctx, job| async move {
                        let _ = tried.send(job.checkpoint(json!({"key": "sk-sentinel-c13"})).await);
                        std::future::pending::<()>().await;
                        Ok(json!({}))
                    })
                })
                .unwrap();
            assert!(matches!(outcome.await.unwrap(), Err(OpError::Invalid { .. })));
            let written = file(&ctx, &id);
            assert_eq!((written["state"].clone(), written.get("result")), (json!("running"), None), "{written}");
            assert!(ctx.jobs().get(&id, dir.path()).unwrap().get("result").is_none());
            ctx.jobs().interrupt_running(ctx.snapshots());
        })
        .await;
    }

    #[tokio::test]
    async fn a_job_that_fails_says_why_and_one_stopped_with_the_runtime_says_cancelled() {
        local(async {
            let dir = tempfile::tempdir().unwrap();
            let ctx = runtime(dir.path());
            let failed =
                ctx.start_job("scan", |_ctx, _job| async move { Err(OpError::Failed { reason: "the folder went away".into() }) }).unwrap();
            let stopped = ctx
                .start_job("scan", |_ctx, job| async move {
                    job.signal.cancelled().await;
                    Err(OpError::Failed { reason: "stopped".into() })
                })
                .unwrap();
            tokio::task::yield_now().await;
            ctx.shutdown().cancel();
            ctx.jobs().all_settled().await;
            let failed = ctx.jobs().get(&failed, dir.path()).unwrap();
            assert_eq!(
                (failed["state"].clone(), failed["error"].clone()),
                (json!("failed"), json!({"code": "failed", "reason": "the folder went away"}))
            );
            assert_eq!(ctx.jobs().get(&stopped, dir.path()).unwrap()["state"], "cancelled");
            assert_eq!(file(&ctx, &stopped)["state"], "cancelled");
        })
        .await;
    }

    #[tokio::test]
    async fn a_job_an_earlier_runtime_left_running_reads_interrupted_and_nothing_else_is_a_job() {
        local(async {
            let dir = tempfile::tempdir().unwrap();
            let id = {
                let earlier = runtime(dir.path());
                let id = earlier.start_job("scan", |_ctx, _job| std::future::pending::<Result<Value, OpError>>()).unwrap();
                assert_eq!(file(&earlier, &id)["state"], "running");
                id
            };
            let ctx = runtime(dir.path());
            let found = ctx.jobs().get(&id, dir.path()).unwrap();
            assert_eq!((found["state"].clone(), found["kind"].clone()), (json!("interrupted"), json!("scan")));
            assert!(found.get("seq").is_none(), "the answer is the job, not its file's envelope");
            assert_eq!(file(&ctx, &id)["state"], "interrupted");
            assert!(ctx.jobs().get("j0-404", dir.path()).is_none());
            assert!(ctx.jobs().get("../status", dir.path()).is_none());
        })
        .await;
    }

    #[tokio::test]
    async fn a_job_whose_work_panics_before_it_starts_fails_alone_and_frees_its_place() {
        local(async {
            let dir = tempfile::tempdir().unwrap();
            let ctx = runtime(dir.path());
            let broken =
                ctx.start_job("scan", |_ctx, _job| -> std::future::Ready<Result<Value, OpError>> { panic!("before its future") }).unwrap();
            ctx.jobs().all_settled().await;
            assert_eq!(ctx.jobs().get(&broken, dir.path()).unwrap()["state"], "failed");
            assert_eq!(ctx.jobs().running(), 0, "its place is free again");
        })
        .await;
    }

    #[tokio::test]
    async fn past_the_cap_a_job_waits_its_turn_and_one_still_running_at_the_stop_reads_interrupted() {
        local(async {
            let dir = tempfile::tempdir().unwrap();
            let ctx = runtime(dir.path());
            let ids: Vec<String> = (0..RUNNING)
                .map(|_| ctx.start_job("scan", |_ctx, _job| std::future::pending::<Result<Value, OpError>>()).unwrap())
                .collect();
            let Err(OpError::Refused { code, .. }) = ctx.start_job("scan", |_ctx, _job| async { Ok(json!({})) }) else {
                panic!("a job past the cap")
            };
            assert_eq!(code, "busy");
            ctx.jobs().interrupt_running(ctx.snapshots());
            assert_eq!(ctx.jobs().get(&ids[0], dir.path()).unwrap()["state"], "interrupted");
            assert_eq!(file(&ctx, &ids[RUNNING - 1])["state"], "interrupted", "no file says running for a job nothing runs");
        })
        .await;
    }
}
