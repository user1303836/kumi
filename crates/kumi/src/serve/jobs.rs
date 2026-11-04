//! Background jobs: work that outlasts the call that started it (a library scan), as C1's `job` and C3's
//! `jobs/<id>.json` say it. Each runs as a task of its own on the runtime's thread; its state, progress and result are
//! kept for `job` and written to `jobs/<id>.json` for the host's UI.
//!
//! When the runtime stops, every job is cancelled and given the runtime's settling time; one still running then is
//! written `interrupted`. A job a runtime left `running` (it was killed) reads `interrupted` at the next start. Nothing is
//! resumed or replayed.

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
/// How many jobs are kept, newest first: in memory and on disk.
const KEPT: usize = 100;
/// How many jobs may run at once: another is refused `busy` until one ends.
pub const RUNNING: usize = 8;

/// Where a job is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    Running,
    Done,
    Failed,
    /// Stopped before it finished: the runtime stopped.
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
    /// `interrupted`, and only the newest [`KEPT`] jobs' files stay. Only one runtime works in a state folder at a time
    /// (CC-6: its parent holds the folder's lock), so no job found here is another live runtime's.
    pub(crate) fn new(snapshots: &Snapshots, state_dir: &Path) -> Jobs {
        settle_earlier(snapshots, &state_dir.join("jobs"));
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
    /// Refused `busy` while [`RUNNING`] jobs run.
    pub(crate) fn start<W, F>(&self, ctx: Rc<Ctx>, kind: &'static str, work: W) -> Result<String, OpError>
    where
        W: FnOnce(Rc<Ctx>, JobHandle) -> F,
        F: Future<Output = Result<Value, OpError>> + 'static,
    {
        if self.running.get() >= RUNNING {
            let reason = format!("{RUNNING} jobs are running already: wait for one to finish (job tells when)");
            return Err(OpError::Refused { code: "busy", reason, owner: None });
        }
        let n = self.next.get() + 1;
        self.next.set(n);
        let id = format!("j{}-{n}", kumi_common::time::now_ms());
        let own = Signal::new();
        let job = Rc::new(Job {
            kind,
            state: Cell::new(State::Running),
            progress: RefCell::new(json!({"done": 0})),
            outcome: RefCell::new(None),
            signal: own.clone(),
            written: Cell::new(None),
        });
        self.jobs.borrow_mut().insert(id.clone(), job.clone());
        self.order.borrow_mut().push_back(id.clone());
        self.running.set(self.running.get() + 1);
        write(ctx.snapshots(), &id, &job);
        let handle = JobHandle { id: id.clone(), job: job.clone(), ctx: ctx.clone(), signal: abort::any([own, ctx.shutdown().clone()]) };
        let future = work(ctx.clone(), handle);
        let started = id.clone();
        tokio::task::spawn_local(async move {
            let outcome = AssertUnwindSafe(future).catch_unwind().await.unwrap_or_else(|_| {
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
            jobs.forget_oldest(&ctx.paths().state_dir);
            if jobs.running.get() == 0 {
                jobs.settled.notify_waiters();
            }
        });
        Ok(id)
    }

    /// Past [`KEPT`] jobs, the oldest finished ones are forgotten, their files with them.
    fn forget_oldest(&self, state_dir: &Path) {
        while self.order.borrow().len() > KEPT {
            let oldest = {
                let jobs = self.jobs.borrow();
                let order = self.order.borrow();
                order.iter().position(|id| jobs.get(id).is_some_and(|job| job.state.get() != State::Running))
            };
            let Some(at) = oldest else { return };
            let Some(id) = self.order.borrow_mut().remove(at) else { return };
            self.jobs.borrow_mut().remove(&id);
            let _ = std::fs::remove_file(state_dir.join("jobs").join(format!("{id}.json")));
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
}

fn write(snapshots: &Snapshots, id: &str, job: &Job) {
    job.written.set(Some(Instant::now()));
    if let Err(error) = snapshots.write(&format!("jobs/{id}"), &job.view(id)) {
        super::say(&format!("kumi serve: jobs/{id}.json not written: {error}"));
    }
}

/// The jobs an earlier runtime left: `running` ones now `interrupted`, the oldest past [`KEPT`] deleted.
fn settle_earlier(snapshots: &Snapshots, folder: &Path) {
    let Ok(entries) = std::fs::read_dir(folder) else { return };
    let mut files: Vec<(std::time::SystemTime, std::path::PathBuf)> = entries
        .filter_map(Result::ok)
        .filter(|entry| entry.file_name().to_string_lossy().ends_with(".json"))
        .map(|entry| (entry.metadata().and_then(|meta| meta.modified()).unwrap_or(std::time::UNIX_EPOCH), entry.path()))
        .collect();
    files.sort_by(|a, b| b.0.cmp(&a.0));
    for (n, (_, path)) in files.iter().enumerate() {
        if n >= KEPT {
            let _ = std::fs::remove_file(path);
            continue;
        }
        let Some(id) = path.file_stem().map(|stem| stem.to_string_lossy().into_owned()) else { continue };
        let Ok(mut kept) = std::fs::read(path).map_err(|_| ()).and_then(|bytes| serde_json::from_slice::<Value>(&bytes).map_err(|_| ()))
        else {
            continue;
        };
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
            daw: DawId::Reaper,
            library_roots: vec![],
            media_roots: vec![],
            tools: BTreeMap::new(),
        };
        Ctx::new(args, Env::new()).unwrap()
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
