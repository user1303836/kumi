//! A low-priority process that learns, accepts pause/resume/stop, and reports progress over JSON lines.
use super::{
    learn::{learn, LearnOptions, LearnPhase, LearnProgress},
    plan::{plan_learning, PlanOptions},
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
    io,
    rc::Rc,
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
fn low_priority() {
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
/// Must run in a LocalSet: pause callbacks share the learner's single event loop.
pub async fn main() -> io::Result<()> {
    low_priority();
    let stop = Signal::new();
    let paused = Rc::new(Cell::new(false));
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
    let (input_stop, input_paused, input_wake) = (stop.clone(), paused.clone(), wake.clone());
    let input = tokio::task::spawn_local(async move {
        let mut lines = BufReader::new(tokio::io::stdin()).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            match serde_json::from_str::<LearnerMessage>(&line) {
                Ok(LearnerMessage::Pause) => input_paused.set(true),
                Ok(LearnerMessage::Resume) => {
                    input_paused.set(false);
                    input_wake.notify_waiters();
                }
                Ok(LearnerMessage::Stop) => {
                    input_stop.cancel();
                    input_paused.set(false);
                    input_wake.notify_waiters();
                }
                Ok(LearnerMessage::Learn { options }) => {
                    input_paused.set(options.paused);
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
            let (report_latest, report_last, report_output, report_dir) = (latest.clone(), last.clone(), output.clone(), dir.clone());
            let mut learning = LearnOptions::new(plan_learning(options.plan).await, stop.clone());
            learning.rebuild = options.rebuild;
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
                    tokio::task::spawn_local(async move {
                        let _ = write_state(&dir, &progress, false).await;
                    });
                }
            }));
            match learn(learning).await {
                Ok(progress) => {
                    *latest.borrow_mut() = Some(progress.clone());
                    let _ = write_state(&dir, &progress, false).await;
                    send(&output, LearnerReply::Done { progress });
                }
                Err(error) => {
                    if !stop.is_cancelled() {
                        send(&output, LearnerReply::Failed { message: head(&error.to_string(), 300) });
                    }
                }
            }
            let latest = latest.borrow().clone();
            if let Some(latest) = latest.filter(|p| p.phase != LearnPhase::Done) {
                let _ = write_state(&dir, &latest, true).await;
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
