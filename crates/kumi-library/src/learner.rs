//! A low-priority process that learns, accepts pause/resume/stop, and reports progress over JSON lines.
use super::{
    daws::Daws,
    learn::{learn, LearnOptions, LearnPhase, LearnProgress},
    plan::{plan_learning, PlanOptions},
    sounds_map::lay_out_sounds,
    state::{acquire_lock, write_state},
};
use kumi_common::{
    abort::Signal,
    js::{json::stringify, string::head},
    time::now_ms,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    cell::{Cell, RefCell},
    future::Future,
    io,
    rc::Rc,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::Duration,
};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    sync::{mpsc, Notify},
};
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LearnerOptions {
    #[serde(flatten)]
    pub plan: PlanOptions,
    #[serde(default)]
    pub paused: bool,
    #[serde(default)]
    pub rebuild: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum LearnerMessage {
    Learn { options: LearnerOptions },
    Pause,
    Resume,
    Stop,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum LearnerReply {
    Progress { progress: LearnProgress },
    Done { progress: LearnProgress },
    Busy,
    Failed { message: String },
}
/// The learner's priority, the system's lowest: the producer's work (and their DAW) always comes first. The music
/// scan, a job of the same worker, takes it too.
pub fn low_priority() {
    #[cfg(unix)]
    unsafe {
        libc::setpriority(libc::PRIO_PROCESS, 0, 19);
    }
    #[cfg(windows)]
    unsafe {
        use std::ffi::c_void;
        #[link(name = "kernel32")]
        unsafe extern "system" {
            fn GetCurrentProcess() -> *mut c_void;
            fn SetPriorityClass(handle: *mut c_void, priority: u32) -> i32;
        }
        SetPriorityClass(GetCurrentProcess(), 0x40);
    }
}
fn send(output: &mpsc::UnboundedSender<Value>, reply: LearnerReply) {
    let _ = output.send(serde_json::to_value(reply).unwrap());
}
/// The learner's progress writes to state.json, each after the one before: a write reads the file, keeps its `last`
/// and writes it back whole, so one still going when another lands puts back what that one replaced (#552).
#[derive(Default)]
struct StateWrites(RefCell<Option<tokio::task::JoinHandle<()>>>);
impl StateWrites {
    /// `write`, once the writes asked before it are done.
    fn then(&self, write: impl Future<Output = ()> + 'static) {
        let before = self.0.borrow_mut().take();
        let next = tokio::task::spawn_local(async move {
            if let Some(before) = before {
                let _ = before.await;
            }
            write.await;
        });
        *self.0.borrow_mut() = Some(next);
    }
    /// The run's own write, its summary or (stopped) its state with no run going, after every progress write asked so
    /// far: none of them can put back what it replaces.
    async fn last(&self, dir: &str, progress: &LearnProgress, stopped: bool) -> io::Result<()> {
        let going = self.0.borrow_mut().take();
        if let Some(going) = going {
            let _ = going.await;
        }
        write_state(dir, progress, stopped).await
    }
}
/// Must run in a LocalSet: pause callbacks share the learner's single event loop. `daws` are what the DAWs the app
/// registers tell the library, as in the app.
pub async fn main(daws: Daws) -> io::Result<()> {
    low_priority();
    let stop = Signal::new();
    let paused = Rc::new(Cell::new(false));
    // The same, for the sounds' map laid out off this thread.
    let held = Arc::new(AtomicBool::new(false));
    let wake = Rc::new(Notify::new());
    let (jobs, mut incoming) = mpsc::unbounded_channel();
    let (output, mut outgoing) = mpsc::unbounded_channel::<Value>();
    let write = tokio::task::spawn_local(async move {
        let mut stdout = tokio::io::stdout();
        while let Some(reply) = outgoing.recv().await {
            stdout.write_all(format!("{}\n", stringify(&reply)).as_bytes()).await?;
            stdout.flush().await?;
        }
        Ok::<_, io::Error>(())
    });
    let (input_stop, input_paused, input_wake, input_held) = (stop.clone(), paused.clone(), wake.clone(), held.clone());
    let input = tokio::task::spawn_local(async move {
        let mut lines = BufReader::new(tokio::io::stdin()).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            match serde_json::from_str::<LearnerMessage>(&line) {
                Ok(LearnerMessage::Pause) => {
                    input_paused.set(true);
                    input_held.store(true, Ordering::Relaxed);
                }
                Ok(LearnerMessage::Resume) => {
                    input_paused.set(false);
                    input_held.store(false, Ordering::Relaxed);
                    input_wake.notify_waiters();
                }
                Ok(LearnerMessage::Stop) => {
                    input_stop.cancel();
                    input_paused.set(false);
                    input_wake.notify_waiters();
                }
                Ok(LearnerMessage::Learn { options }) => {
                    input_paused.set(options.paused);
                    input_held.store(options.paused, Ordering::Relaxed);
                    let _ = jobs.send(options);
                }
                Err(_) => {}
            }
        }
        input_stop.cancel();
        // A disconnected parent cannot keep an uncooperative decoder alive indefinitely.
        tokio::time::sleep(Duration::from_secs(2)).await;
        std::process::exit(0);
    });
    if let Some(options) = incoming.recv().await {
        let dir = options.plan.dir.clone();
        let release = acquire_lock(&dir).await?;
        if let Some(release) = release {
            let latest = Rc::new(RefCell::new(None::<LearnProgress>));
            let last = Rc::new(Cell::new(0));
            let writes = Rc::new(StateWrites::default());
            let (report_latest, report_last, report_output, report_dir, report_writes) =
                (latest.clone(), last.clone(), output.clone(), dir.clone(), writes.clone());
            let plan = plan_learning(PlanOptions { daws, ..options.plan }).await;
            let sources = plan.sources.clone();
            let mut learning = LearnOptions::new(plan, stop.clone());
            learning.rebuild = options.rebuild;
            learning.daws = daws;
            learning.gate = Some(Rc::new(move || {
                let (paused, wake) = (paused.clone(), wake.clone());
                Box::pin(async move {
                    if paused.get() {
                        wake.notified().await;
                    }
                })
            }));
            learning.on_progress = Some(Rc::new(move |progress| {
                *report_latest.borrow_mut() = Some(progress.clone());
                send(&report_output, LearnerReply::Progress { progress: progress.clone() });
                if now_ms() - report_last.get() > 2000 || progress.phase == LearnPhase::Done {
                    report_last.set(now_ms());
                    let dir = report_dir.clone();
                    report_writes.then(async move {
                        let _ = write_state(&dir, &progress, false).await;
                    });
                }
            }));
            match learn(learning).await {
                Ok(progress) => {
                    *latest.borrow_mut() = Some(progress.clone());
                    let _ = writes.last(&dir, &progress, false).await;
                    send(&output, LearnerReply::Done { progress });
                    // Then the sounds' map, when it's due, while the library is still this learner's: it waits while
                    // the learner's paused (the DAW plays), and ends when it's stopped.
                    if !stop.is_cancelled() {
                        let (held, stopped) = (held.clone(), stop.clone());
                        let go_on = Arc::new(move || {
                            while held.load(Ordering::Relaxed) && !stopped.is_cancelled() {
                                std::thread::sleep(Duration::from_millis(100));
                            }
                            !stopped.is_cancelled()
                        });
                        let how = kumi_layout::Layout::default();
                        if let Err(error) = lay_out_sounds(std::path::Path::new(&dir), &sources, &how, go_on).await {
                            send(
                                &output,
                                LearnerReply::Failed { message: head(&format!("Kumi couldn't make the sounds' map: {error}"), 300) },
                            );
                        }
                    }
                }
                Err(error) => {
                    if !stop.is_cancelled() {
                        send(&output, LearnerReply::Failed { message: head(&error.to_string(), 300) });
                    }
                }
            }
            let latest = latest.borrow().clone();
            if let Some(latest) = latest.filter(|p| p.phase != LearnPhase::Done) {
                let _ = writes.last(&dir, &latest, true).await;
            }
            release.release().await?;
        } else {
            send(&output, LearnerReply::Busy);
        }
    }
    drop(output);
    input.abort();
    write.await.map_err(io::Error::other)??;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        state::{read_state, Learning, LibraryState},
        store::{read_json, write_json},
    };

    #[tokio::test(flavor = "current_thread")]
    async fn a_runs_summary_lands_after_a_slow_progress_write_still_going() {
        tokio::task::LocalSet::new()
            .run_until(async {
                let folder = tempfile::tempdir().unwrap();
                let dir = folder.path().to_string_lossy().into_owned();
                let writes = StateWrites::default();
                // A progress write on a slow disk, as write_state makes it: it reads state.json (no summary yet: the
                // library's first run), takes a while, then writes it back whole, with a run going.
                let (read, was_read) = tokio::sync::oneshot::channel();
                let (wrote, was_written) = tokio::sync::oneshot::channel();
                let file = folder.path().join("state.json");
                writes.then(async move {
                    let before = read_json::<LibraryState>(&file).await;
                    let _ = read.send(());
                    tokio::time::sleep(Duration::from_millis(200)).await;
                    let going = Learning {
                        pid: std::process::id() as i64,
                        started_at: 1,
                        phase: LearnPhase::Sounds,
                        sounds: Default::default(),
                        presets: Default::default(),
                        sets: Default::default(),
                        updated_at: now_ms(),
                    };
                    let state = LibraryState { version: 1, last: before.and_then(|state| state.last), learning: Some(going) };
                    write_json(&file, &state).await.unwrap();
                    let _ = wrote.send(());
                });
                was_read.await.unwrap();
                // The run ends meanwhile, and its summary is written. The learner goes on (the sounds' map), and the
                // progress write lands too, whenever it does.
                let done = LearnProgress { phase: LearnPhase::Done, started_at: 1, finished_at: Some(2), ..Default::default() };
                writes.last(&dir, &done, false).await.unwrap();
                was_written.await.unwrap();
                let state = read_state(&dir).await.unwrap();
                assert_eq!(state.last.map(|last| last.finished_at), Some(2), "the run's summary is kept");
                assert_eq!(state.learning, None, "and no run is going");
            })
            .await;
    }
}
