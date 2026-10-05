//! The library's measurement worker and its pool.
//! Persistent native child processes retain worker isolation and can be killed on decoder timeouts.
use super::{
    features::MeasureOptions,
    learn::{learn_sound, SoundEntry},
    store::Entry,
};
use kumi_common::js::{json::stringify, string::head};
use serde::{Deserialize, Serialize};
use std::{cell::Cell, io, path::PathBuf, process::Stdio, time::Duration};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines},
    process::{Child, ChildStdin, ChildStdout, Command},
    sync::Mutex,
};
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MeasureJob {
    pub id: u64,
    pub path: String,
    pub relative: String,
    pub size: u64,
    pub mtime: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seconds: Option<f64>,
}
#[derive(Debug, Serialize, Deserialize)]
pub struct MeasureReply {
    pub id: u64,
    pub entry: SoundEntry,
}
impl MeasureJob {
    pub fn failed(&self, why: &str) -> SoundEntry {
        SoundEntry {
            file: Entry { path: self.path.clone(), size: self.size, mtime: self.mtime, gone: None },
            error: Some(head(why, 160)),
            ..Default::default()
        }
    }
    pub async fn measure(&self) -> SoundEntry {
        learn_sound(
            &self.path,
            &self.relative,
            self.size,
            self.mtime,
            MeasureOptions { start: self.start, seconds: self.seconds, signal: None },
        )
        .await
        .unwrap_or_else(|error| self.failed(&error.to_string()))
    }
}
/// Workers are installed beside the main binary; the override also supports embedders.
pub fn worker_binary(name: &str, variable: &str) -> PathBuf {
    if let Some(path) = std::env::var_os(variable) {
        return path.into();
    }
    let filename = format!("{name}{}", std::env::consts::EXE_SUFFIX);
    std::env::current_exe()
        .ok()
        .and_then(|exe| {
            exe.parent().map(|folder| {
                // Cargo's integration tests are one directory below the binaries they exercise.
                let folder = if folder.file_name().is_some_and(|n| n == "deps") { folder.parent().unwrap_or(folder) } else { folder };
                folder.join(&filename)
            })
        })
        .unwrap_or_else(|| filename.into())
}
struct Worker {
    child: Child,
    input: ChildStdin,
    lines: Lines<BufReader<ChildStdout>>,
}
impl Worker {
    fn spawn(binary: &std::path::Path) -> io::Result<Self> {
        let mut child =
            Command::new(binary).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::inherit()).kill_on_drop(true).spawn()?;
        let input = child.stdin.take().unwrap();
        let lines = BufReader::new(child.stdout.take().unwrap()).lines();
        Ok(Self { child, input, lines })
    }
    async fn run(&mut self, job: &MeasureJob) -> io::Result<SoundEntry> {
        self.input.write_all(format!("{}\n", stringify(&serde_json::to_value(job)?)).as_bytes()).await?;
        self.input.flush().await?;
        while let Some(line) = self.lines.next_line().await? {
            let reply: MeasureReply = serde_json::from_str(&line)?;
            if reply.id == job.id {
                return Ok(reply.entry);
            }
        }
        Err(io::Error::new(io::ErrorKind::UnexpectedEof, "measurement worker exited"))
    }
    async fn stop(&mut self) {
        let _ = self.child.start_kill();
        let _ = self.child.wait().await;
    }
}
pub struct MeasurePool {
    slots: Vec<Mutex<Option<Worker>>>,
    timeout: Duration,
    binary: PathBuf,
    next: Cell<u64>,
}
impl MeasurePool {
    pub fn new(size: usize) -> Self {
        Self::with_worker(size, worker_binary("kumi-library-measure", "KUMI_LIBRARY_MEASURE_BIN"), Duration::from_millis(90000))
    }
    pub fn with_worker(size: usize, binary: PathBuf, timeout: Duration) -> Self {
        Self { slots: (0..size).map(|_| Mutex::new(None)).collect(), timeout, binary, next: Cell::new(0) }
    }
    pub async fn run(&self, slot: usize, mut job: MeasureJob) -> SoundEntry {
        if self.slots.is_empty() {
            return job.measure().await;
        }
        job.id = self.next.get();
        self.next.set(job.id.wrapping_add(1));
        let mut held = self.slots[slot % self.slots.len()].lock().await;
        if held.is_none() {
            match Worker::spawn(&self.binary) {
                Ok(worker) => *held = Some(worker),
                Err(_) => return job.failed("Kumi couldn't read it"),
            }
        }
        let worker = held.as_mut().unwrap();
        let result = tokio::time::timeout(self.timeout, worker.run(&job)).await;
        match result {
            Ok(Ok(entry)) => entry,
            result => {
                let why = if result.is_err() { "it took too long to read" } else { "Kumi couldn't read it" };
                worker.stop().await;
                *held = None;
                job.failed(why)
            }
        }
    }
    pub async fn close(&self) {
        for slot in &self.slots {
            if let Some(mut worker) = slot.lock().await.take() {
                worker.stop().await;
            }
        }
    }
}
pub async fn main() -> io::Result<()> {
    let mut lines = BufReader::new(tokio::io::stdin()).lines();
    let mut output = tokio::io::stdout();
    while let Some(line) = lines.next_line().await? {
        let job: MeasureJob = serde_json::from_str(&line)?;
        let reply = MeasureReply { id: job.id, entry: job.measure().await };
        output.write_all(format!("{}\n", stringify(&serde_json::to_value(reply)?)).as_bytes()).await?;
        output.flush().await?;
    }
    Ok(())
}

/// One reference file, with immediate cancellation and no learner's per-file timeout.
pub async fn measure_reference(
    job: MeasureJob,
    signal: kumi_common::abort::Signal,
) -> Result<SoundEntry, crate::core::errors::RuntimeError> {
    use crate::core::errors::RuntimeError;
    let mut worker = Worker::spawn(&worker_binary("kumi-library-measure", "KUMI_LIBRARY_MEASURE_BIN"))
        .map_err(|e| RuntimeError::plain(e.to_string()))?;
    let result = tokio::select! {biased;_=signal.cancelled()=>Err(RuntimeError::Aborted),result=worker.run(&job)=>result.map_err(|e|RuntimeError::plain(e.to_string()))};
    worker.stop().await;
    result
}
