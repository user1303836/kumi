//! Each job runs in a disposable child process: the `ableton-mcp-analysis-worker` binary beside
//! this one (`analysis_job_worker`), fed the job as JSON on stdin.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::path::PathBuf;
use std::process::Stdio;
use std::rc::Rc;
use std::time::Duration;

use kumi_common::abort::Signal;
use kumi_common::js::json;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
use tokio::process::Command;
use tokio::sync::oneshot;

use crate::audio_standards::ConventionalChannelLabel;
use crate::reference_analysis::AlignmentOptions;

pub const MAX_CONCURRENT_ANALYSIS_JOBS: usize = 2;
pub const MAX_QUEUED_ANALYSIS_JOBS: usize = 4;
pub const MAX_ANALYSIS_JOB_OUTPUT_BYTES: usize = 2 * 1024 * 1024;
pub const MAX_ANALYSIS_JOB_STDERR_BYTES: usize = 16 * 1024;
pub const MAX_ANALYSIS_JOB_REQUEST_BYTES: usize = 64 * 1024 * 1024;
pub const ANALYSIS_JOB_TIMEOUT_MS: u64 = 30_000;

/// The worker binary's name, found beside the running executable.
pub const ANALYSIS_WORKER_EXECUTABLE: &str = "ableton-mcp-analysis-worker";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EncodedAnalysisSource {
    pub pcm_base64: String,
    pub sample_rate: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channels: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channel_layout: Option<Vec<ConventionalChannelLabel>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub frame_size: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "lowercase")]
pub enum AnalysisJob {
    Analyze {
        source: EncodedAnalysisSource,
    },
    Compare {
        project: EncodedAnalysisSource,
        reference: EncodedAnalysisSource,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        alignment: Option<AlignmentOptions>,
    },
}

/// What a job throws: its message.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct AnalysisJobError(pub String);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AnalysisRunnerStatus {
    pub active: usize,
    pub queued: usize,
    pub max_concurrent: usize,
    pub max_queued: usize,
}

struct QueueItem {
    id: u64,
    sender: oneshot::Sender<Result<(), AnalysisJobError>>,
    signal: Option<Signal>,
}

#[derive(Default)]
struct RunnerState {
    active: usize,
    queue: VecDeque<QueueItem>,
    next_id: u64,
}

#[derive(Clone, Default)]
pub struct AnalysisRunner {
    state: Rc<RefCell<RunnerState>>,
}

/// Holds a concurrency slot until the job's process is confirmed closed.
struct Slot(AnalysisRunner);

impl Drop for Slot {
    fn drop(&mut self) {
        self.0.release();
    }
}

impl AnalysisRunner {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn status(&self) -> AnalysisRunnerStatus {
        let state = self.state.borrow();
        AnalysisRunnerStatus {
            active: state.active,
            queued: state.queue.len(),
            max_concurrent: MAX_CONCURRENT_ANALYSIS_JOBS,
            max_queued: MAX_QUEUED_ANALYSIS_JOBS,
        }
    }

    async fn acquire(&self, signal: Option<&Signal>) -> Result<(), AnalysisJobError> {
        if signal.is_some_and(Signal::is_cancelled) {
            return Err(AnalysisJobError("analysis job cancelled before queueing".to_string()));
        }
        let (mut receiver, id) = {
            let mut state = self.state.borrow_mut();
            if state.active < MAX_CONCURRENT_ANALYSIS_JOBS {
                state.active += 1;
                return Ok(());
            }
            if state.queue.len() >= MAX_QUEUED_ANALYSIS_JOBS {
                return Err(AnalysisJobError("analysis job queue is full".to_string()));
            }
            let (sender, receiver) = oneshot::channel();
            let id = state.next_id;
            state.next_id += 1;
            state.queue.push_back(QueueItem { id, sender, signal: signal.cloned() });
            (receiver, id)
        };
        let cancelled_while_queued = || AnalysisJobError("analysis job cancelled while queued".to_string());
        let Some(signal) = signal else {
            return receiver.await.unwrap_or_else(|_| Err(cancelled_while_queued()));
        };
        tokio::select! {
            biased;
            outcome = &mut receiver => outcome.unwrap_or_else(|_| Err(cancelled_while_queued())),
            _ = signal.cancelled() => {
                let removed = {
                    let mut state = self.state.borrow_mut();
                    let before = state.queue.len();
                    state.queue.retain(|item| item.id != id);
                    state.queue.len() != before
                };
                if removed {
                    Err(cancelled_while_queued())
                } else {
                    // The slot was handed over as the cancellation came: it is ours to release.
                    receiver.await.unwrap_or_else(|_| Err(cancelled_while_queued()))
                }
            }
        }
    }

    fn release(&self) {
        let mut state = self.state.borrow_mut();
        state.active = state.active.saturating_sub(1);
        while let Some(next) = state.queue.pop_front() {
            if next.signal.as_ref().is_some_and(Signal::is_cancelled) {
                let _ = next.sender.send(Err(AnalysisJobError("analysis job cancelled while queued".to_string())));
                continue;
            }
            state.active += 1;
            if next.sender.send(Ok(())).is_err() {
                // The waiter went away: the slot stays free for the next one.
                state.active -= 1;
                continue;
            }
            break;
        }
    }

    /// `timeoutMs` defaults to [`ANALYSIS_JOB_TIMEOUT_MS`].
    pub async fn run(&self, job: &AnalysisJob, signal: Option<Signal>, timeout_ms: Option<u64>) -> Result<Value, AnalysisJobError> {
        self.run_value(&serde_json::to_value(job).map_err(|cause| AnalysisJobError(cause.to_string()))?, signal, timeout_ms).await
    }

    /// The host validates the caller's JSON independently; the worker remains the final job-schema boundary.
    pub async fn run_value(&self, job: &Value, signal: Option<Signal>, timeout_ms: Option<u64>) -> Result<Value, AnalysisJobError> {
        self.acquire(signal.as_ref()).await?;
        let slot = Slot(self.clone());
        let result = spawn_job(job, signal.as_ref(), timeout_ms.unwrap_or(ANALYSIS_JOB_TIMEOUT_MS)).await;
        drop(slot);
        result
    }
}

/// The worker binary beside this executable. A test binary runs from `target/debug/deps` while the
/// worker is built one folder up, so that folder is tried next.
pub fn analysis_worker_executable() -> PathBuf {
    let name = format!("{ANALYSIS_WORKER_EXECUTABLE}{}", std::env::consts::EXE_SUFFIX);
    let Some(directory) = std::env::current_exe().ok().and_then(|path| path.parent().map(|parent| parent.to_path_buf())) else {
        return PathBuf::from(name);
    };
    let sibling = directory.join(&name);
    if sibling.exists() {
        return sibling;
    }
    if let Some(above) = directory.parent().map(|parent| parent.join(&name)) {
        if above.exists() {
            return above;
        }
    }
    sibling
}

async fn read_bounded<R: AsyncRead + Unpin>(mut reader: R, maximum: usize, keep: bool, message: &'static str) -> Result<Vec<u8>, String> {
    let mut total = 0usize;
    let mut output = Vec::new();
    let mut buffer = vec![0u8; 64 * 1024];
    loop {
        let count = match reader.read(&mut buffer).await {
            Ok(0) | Err(_) => return Ok(output),
            Ok(count) => count,
        };
        total += count;
        if total > maximum {
            return Err(message.to_string());
        }
        if keep {
            output.extend_from_slice(&buffer[..count]);
        }
    }
}

async fn cancelled(signal: Option<&Signal>) {
    match signal {
        Some(signal) => signal.cancelled().await,
        None => std::future::pending().await,
    }
}

async fn spawn_job(job: &Value, signal: Option<&Signal>, timeout_ms: u64) -> Result<Value, AnalysisJobError> {
    let payload = json::stringify(job);
    if payload.len() > MAX_ANALYSIS_JOB_REQUEST_BYTES {
        return Err(AnalysisJobError("analysis job request exceeds the worker input limit".to_string()));
    }
    let mut command = Command::new(analysis_worker_executable());
    command.env_clear().env("ABLETON_MCP_ANALYSIS_WORKER", "1");
    // Windows process creation and temporary-directory resolution can need
    // these platform variables; application secrets are deliberately not
    // inherited by the disposable DSP worker.
    for name in ["SystemRoot", "WINDIR", "TEMP", "TMP", "TMPDIR"] {
        if let Ok(value) = std::env::var(name) {
            command.env(name, value);
        }
    }
    command.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).kill_on_drop(true);
    let mut child = command.spawn().map_err(|cause| AnalysisJobError(cause.to_string()))?;
    let stdin = child.stdin.take();
    let stdout = child.stdout.take().expect("piped stdout");
    let stderr = child.stderr.take().expect("piped stderr");

    // The first failure wins; the process is then killed and its close awaited before settling, so
    // the concurrency slot stays occupied until no worker is resident.
    let mut requested: Option<AnalysisJobError> = None;
    let mut exited = false;
    if signal.is_some_and(Signal::is_cancelled) {
        requested = Some(AnalysisJobError("analysis job cancelled".to_string()));
        let _ = child.start_kill();
    }
    let write_payload = requested.is_none();
    let mut stdin_task = Box::pin(async move {
        match stdin {
            Some(mut stdin) if write_payload => {
                stdin.write_all(payload.as_bytes()).await?;
                stdin.shutdown().await
            }
            _ => Ok(()),
        }
    });
    let mut stdout_task = Box::pin(read_bounded(stdout, MAX_ANALYSIS_JOB_OUTPUT_BYTES, true, "analysis worker output exceeded its bound"));
    let mut stderr_task =
        Box::pin(read_bounded(stderr, MAX_ANALYSIS_JOB_STDERR_BYTES, false, "analysis worker diagnostics exceeded its bound"));
    let timeout = tokio::time::sleep(Duration::from_millis(timeout_ms));
    tokio::pin!(timeout);
    let mut stdin_done = false;
    let mut stdout_result: Option<Vec<u8>> = None;
    let mut stderr_done = false;
    let mut timed_out = false;
    loop {
        if exited && stdout_result.is_some() && stderr_done {
            break;
        }
        let mut finish: Option<String> = None;
        tokio::select! {
            biased;
            _ = &mut timeout, if !timed_out && requested.is_none() => {
                timed_out = true;
                finish = Some(format!("analysis job exceeded {timeout_ms} ms"));
            }
            _ = cancelled(signal), if requested.is_none() => {
                finish = Some("analysis job cancelled".to_string());
            }
            result = &mut stdin_task, if !stdin_done => {
                stdin_done = true;
                if let Err(cause) = result {
                    finish = Some(cause.to_string());
                }
            }
            result = &mut stdout_task, if stdout_result.is_none() => {
                match result {
                    Ok(bytes) => stdout_result = Some(bytes),
                    Err(message) => {
                        stdout_result = Some(Vec::new());
                        finish = Some(message);
                    }
                }
            }
            result = &mut stderr_task, if !stderr_done => {
                stderr_done = true;
                if let Err(message) = result {
                    finish = Some(message);
                }
            }
            _ = child.wait(), if !exited => {
                exited = true;
            }
        }
        if let Some(message) = finish {
            if requested.is_none() {
                requested = Some(AnalysisJobError(message));
            }
            if !exited {
                let _ = child.start_kill();
            }
        }
    }
    if let Some(cause) = requested {
        return Err(cause);
    }
    let output = stdout_result.unwrap_or_default();
    let envelope: Value = serde_json::from_slice::<Value>(&output)
        .ok()
        .filter(|value| !value.is_null())
        .ok_or_else(|| AnalysisJobError("analysis worker returned invalid bounded JSON".to_string()))?;
    if envelope.get("ok") != Some(&Value::Bool(true)) {
        let message = envelope.get("error").and_then(Value::as_str).unwrap_or("analysis worker failed");
        return Err(AnalysisJobError(message.to_string()));
    }
    Ok(envelope.get("result").cloned().unwrap_or(Value::Null))
}
