//! The jobs and the preview, as the socket threads see them: `listen_status`, `listen_cancel` and `audition_stop` are
//! answered from here at once, even while the main thread is inside REAPER's render. The main thread runs the jobs and
//! writes their state here; the lock is held only to read or write that state, never across a REAPER call.

use std::path::PathBuf;
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use serde_json::{json, Map, Value};

use super::audition::Slot;
use crate::queue::Answer;

/// Jobs remembered after they end, for a status asked late.
const KEPT: usize = 16;
/// How long `listen_cancel` waits for a running job to stop before it answers what the job is doing then.
pub const CANCEL_WAIT: Duration = Duration::from_millis(1500);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Render,
    Capture,
}

impl Kind {
    pub fn word(self) -> &'static str {
        match self {
            Kind::Render => "render",
            Kind::Capture => "capture",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    Queued,
    Running,
    Done,
    Failed,
    Cancelled,
}

impl State {
    pub fn word(self) -> &'static str {
        match self {
            State::Queued => "queued",
            State::Running => "running",
            State::Done => "done",
            State::Failed => "failed",
            State::Cancelled => "cancelled",
        }
    }

    pub fn ended(self) -> bool {
        matches!(self, State::Done | State::Failed | State::Cancelled)
    }
}

/// The files a render pass is writing now, with the bytes each will have, and the bytes of the passes before it: the
/// socket threads measure a render's progress by them.
#[derive(Debug, Clone, Default)]
pub struct Watch {
    pub files: Vec<(PathBuf, u64)>,
    pub before: u64,
    pub total: u64,
}

#[derive(Debug, Clone)]
pub struct Job {
    pub id: String,
    pub kind: Kind,
    pub state: State,
    /// Every other request is answered busy while it lasts (a render holds REAPER's main thread).
    pub blocks: bool,
    /// Kumi asked it to stop.
    pub cancel: bool,
    /// 0..1, for the jobs the main thread measures itself (a capture, a track's items).
    pub progress: f64,
    pub watch: Option<Watch>,
    /// What the status says once the job has ended: files, error and code, notes, marks.
    pub result: Map<String, Value>,
    pub ended: Option<Instant>,
}

impl Job {
    pub fn new(id: &str, kind: Kind, blocks: bool) -> Job {
        Job {
            id: id.into(),
            kind,
            state: State::Queued,
            blocks,
            cancel: false,
            progress: 0.0,
            watch: None,
            result: Map::new(),
            ended: None,
        }
    }

    fn progress(&self) -> f64 {
        match (self.state, &self.watch) {
            (State::Done, _) => 1.0,
            (State::Running, Some(watch)) if watch.total > 0 => {
                let written: u64 =
                    watch.files.iter().map(|(file, planned)| std::fs::metadata(file).map_or(0, |meta| meta.len()).min(*planned)).sum();
                ((watch.before + written) as f64 / watch.total as f64).clamp(0.0, 0.99)
            }
            _ => self.progress.clamp(0.0, 1.0),
        }
    }

    fn answer(&self) -> Value {
        let mut answer = json!({
            "jobId": self.id,
            "kind": self.kind.word(),
            "state": self.state.word(),
            "progress": (self.progress() * 1000.0).round() / 1000.0,
        });
        for (key, value) in &self.result {
            answer[key] = value.clone();
        }
        answer
    }
}

/// The preview playing out loud now.
pub struct Playing {
    pub id: String,
    pub slot: Arc<Slot>,
    /// Kumi asked it to stop: it is silent already, and the main thread stops it on its next tick.
    pub stop: bool,
}

#[derive(Default)]
struct Inner {
    jobs: Vec<Job>,
    playing: Option<Playing>,
}

#[derive(Default)]
pub struct Board {
    inner: Mutex<Inner>,
    /// Woken whenever a job's state changes, for a cancel waiting to see it stop.
    changed: Condvar,
}

impl std::fmt::Debug for Board {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Board")
    }
}

fn bad(message: impl Into<String>) -> (String, String) {
    ("bad-params".into(), message.into())
}

impl Board {
    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    // ----- the main thread's side -----

    /// A job that hasn't ended, if one has (one job at a time).
    pub fn active(&self) -> Option<(String, Kind)> {
        self.lock().jobs.iter().find(|job| !job.state.ended()).map(|job| (job.id.clone(), job.kind))
    }

    pub fn knows(&self, id: &str) -> bool {
        self.lock().jobs.iter().any(|job| job.id == id)
    }

    /// Adds a job, forgetting the oldest ended ones past what is kept.
    pub fn add(&self, job: Job) {
        let mut inner = self.lock();
        inner.jobs.push(job);
        while inner.jobs.len() > KEPT {
            match inner.jobs.iter().position(|job| job.state.ended()) {
                Some(at) => {
                    inner.jobs.remove(at);
                }
                None => break,
            }
        }
        self.changed.notify_all();
    }

    /// Changes a job; false when there is no such job.
    pub fn update(&self, id: &str, change: impl FnOnce(&mut Job)) -> bool {
        let mut inner = self.lock();
        let Some(job) = inner.jobs.iter_mut().find(|job| job.id == id) else { return false };
        change(job);
        if job.state.ended() && job.ended.is_none() {
            job.ended = Some(Instant::now());
            job.watch = None;
        }
        self.changed.notify_all();
        true
    }

    /// A copy of a job as it is now.
    pub fn job(&self, id: &str) -> Option<Job> {
        self.lock().jobs.iter().find(|job| job.id == id).cloned()
    }

    /// Whether Kumi asked this job to stop.
    pub fn cancelled(&self, id: &str) -> bool {
        self.lock().jobs.iter().any(|job| job.id == id && job.cancel)
    }

    /// Takes a queued job to run it: false when it was cancelled first (or is gone).
    pub fn start(&self, id: &str) -> bool {
        let mut inner = self.lock();
        let Some(job) = inner.jobs.iter_mut().find(|job| job.id == id) else { return false };
        if job.state != State::Queued {
            return false;
        }
        job.state = State::Running;
        self.changed.notify_all();
        true
    }

    /// Whether a job that holds every other request is queued or running.
    pub fn holding(&self) -> bool {
        self.lock().jobs.iter().any(|job| job.blocks && !job.state.ended())
    }

    pub fn set_playing(&self, playing: Option<Playing>) {
        self.lock().playing = playing;
    }

    /// The preview playing now: its id, its slot and whether Kumi asked it to stop.
    pub fn playing(&self) -> Option<(String, Arc<Slot>, bool)> {
        self.lock().playing.as_ref().map(|p| (p.id.clone(), p.slot.clone(), p.stop))
    }

    // ----- the socket threads' side -----

    /// `listen_status {jobId}`.
    pub fn status(&self, params: &Value) -> Answer {
        let id = params.get("jobId").and_then(Value::as_str).ok_or_else(|| bad("jobId: the job render_start or capture_start began"))?;
        let inner = self.lock();
        let job = inner.jobs.iter().find(|job| job.id == id).ok_or_else(|| bad(format!("REAPER has no job {id:?}")))?;
        Ok(job.answer())
    }

    /// `listen_cancel {jobId}`: a queued job is cancelled at once; a running one is asked to stop and waited for a moment,
    /// and the answer says what it is doing then (`stopping` while REAPER still finishes it, as Renoise words it).
    pub fn cancel(&self, params: &Value, wait: Duration) -> Answer {
        let id = params.get("jobId").and_then(Value::as_str).ok_or_else(|| bad("jobId: the job to stop"))?;
        let mut inner = self.lock();
        let job = inner.jobs.iter_mut().find(|job| job.id == id).ok_or_else(|| bad(format!("REAPER has no job {id:?}")))?;
        match job.state {
            State::Queued => {
                job.cancel = true;
                job.state = State::Cancelled;
                job.ended = Some(Instant::now());
                self.changed.notify_all();
            }
            State::Running => job.cancel = true,
            _ => {}
        }
        let until = Instant::now() + wait;
        loop {
            let job = inner.jobs.iter().find(|job| job.id == id).ok_or_else(|| bad(format!("REAPER has no job {id:?}")))?;
            if job.state.ended() {
                return Ok(json!({"jobId": id, "state": job.state.word()}));
            }
            let left = until.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Ok(json!({"jobId": id, "state": job.state.word(), "stopping": true}));
            }
            inner = self.changed.wait_timeout(inner, left).unwrap_or_else(|e| e.into_inner()).0;
        }
    }

    /// `audition_stop {previewId?}`: the preview goes silent at once (its volume, under REAPER's own lock for it), and the
    /// main thread stops it on its next tick.
    pub fn stop_preview(&self, params: &Value) -> Answer {
        let wanted = params.get("previewId").and_then(Value::as_str);
        let mut inner = self.lock();
        let Some(playing) = inner.playing.as_mut().filter(|p| !p.stop && wanted.is_none_or(|id| id == p.id)) else {
            return Ok(json!({"stopped": false}));
        };
        playing.slot.silence();
        playing.stop = true;
        Ok(json!({"stopped": true}))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;

    #[test]
    fn a_queued_job_cancels_at_once_and_a_running_one_is_waited_for() {
        let board = Arc::new(Board::default());
        board.add(Job::new("j1", Kind::Render, true));
        assert!(board.holding());
        assert_eq!(board.cancel(&json!({"jobId": "j1"}), CANCEL_WAIT).unwrap(), json!({"jobId": "j1", "state": "cancelled"}));
        assert!(!board.holding());
        assert!(!board.start("j1"), "a cancelled job never runs");

        board.add(Job::new("j2", Kind::Render, true));
        assert!(board.start("j2"));
        let main = {
            let board = board.clone();
            thread::spawn(move || {
                while !board.cancelled("j2") {
                    thread::sleep(Duration::from_millis(5));
                }
                thread::sleep(Duration::from_millis(30));
                board.update("j2", |job| job.state = State::Cancelled);
            })
        };
        let started = Instant::now();
        assert_eq!(board.cancel(&json!({"jobId": "j2"}), CANCEL_WAIT).unwrap(), json!({"jobId": "j2", "state": "cancelled"}));
        assert!(started.elapsed() < CANCEL_WAIT);
        main.join().unwrap();

        // One that doesn't stop in time says so.
        board.add(Job::new("j3", Kind::Capture, false));
        assert!(board.start("j3"));
        let answer = board.cancel(&json!({"jobId": "j3"}), Duration::from_millis(20)).unwrap();
        assert_eq!(answer, json!({"jobId": "j3", "state": "running", "stopping": true}));
        assert_eq!(board.cancel(&json!({"jobId": "nope"}), CANCEL_WAIT).unwrap_err().0, "bad-params");
    }

    #[test]
    fn a_running_renders_progress_is_the_bytes_its_files_have() {
        let dir = tempfile::tempdir().unwrap();
        let board = Board::default();
        board.add(Job::new("j1", Kind::Render, true));
        board.start("j1");
        let (a, b) = (dir.path().join("a.wav"), dir.path().join("b.wav"));
        std::fs::write(&a, vec![0u8; 500]).unwrap();
        board.update("j1", |job| job.watch = Some(Watch { files: vec![(a, 1000), (b, 1000)], before: 1000, total: 3000 }));
        let status = board.status(&json!({"jobId": "j1"})).unwrap();
        assert_eq!((status["state"].as_str(), status["progress"].as_f64()), (Some("running"), Some(0.5)));
        board.update("j1", |job| {
            job.state = State::Done;
            job.result.insert("files".into(), json!([]));
        });
        let status = board.status(&json!({"jobId": "j1"})).unwrap();
        assert_eq!(status, json!({"jobId": "j1", "kind": "render", "state": "done", "progress": 1.0, "files": []}));
        assert_eq!(board.status(&json!({})).unwrap_err().0, "bad-params");
    }

    #[test]
    fn ended_jobs_are_kept_up_to_a_limit_and_the_oldest_go_first() {
        let board = Board::default();
        for n in 0..KEPT + 4 {
            let mut job = Job::new(&format!("j{n}"), Kind::Render, true);
            job.state = State::Done;
            board.add(job);
        }
        assert!(!board.knows("j0") && !board.knows("j3"));
        assert!(board.knows("j4") && board.knows(&format!("j{}", KEPT + 3)));
    }
}
