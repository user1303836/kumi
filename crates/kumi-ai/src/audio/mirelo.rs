//! Phased Mirelo generation for durable billing checkpoints.
//!
//! Prepare does not create a billed job. Persist an unknown-submission checkpoint
//! before consuming `Prepared` in `submit`, then checkpoint the job and its actual
//! credits before delivery. Recovery is GET-only; this API never retries a create.
use super::{wire, Task};
use kumi_common::abort::Signal;
use reqwest::{Method, Request, RequestBuilder};
use serde_json::Value;
use std::{
    fmt,
    fs::File,
    path::{Path, PathBuf},
};

const MAX_CREDITS: u64 = (1 << 53) - 1;
const MAX_DELIVERY: u64 = 50 * 1024 * 1024;

/// Safe stage diagnostic; provider text, credentials and paths are never exposed.
#[derive(Debug)]
pub struct Error(&'static str);
impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.0)
    }
}
impl std::error::Error for Error {}

#[derive(Debug)]
pub struct SubmitError {
    error: Error,
    not_submitted: bool,
}
impl SubmitError {
    /// True only when dispatch was provably never attempted.
    pub fn not_submitted(&self) -> bool {
        self.not_submitted
    }
}
impl fmt::Display for SubmitError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.error, f)
    }
}
impl std::error::Error for SubmitError {}

/// Immutable, single-use prepared request. Deliberately neither Clone nor Debug.
pub struct Prepared {
    task: Task,
    request: Request,
    credits: u64,
    request_sha256: String,
}
impl Prepared {
    pub fn credits(&self) -> u64 {
        self.credits
    }
    pub fn request_sha256(&self) -> &str {
        &self.request_sha256
    }
}

/// Receipt facts only: the underlying response and its transfer URLs stay private.
pub struct Job {
    task: Task,
    raw: Value,
}
impl Job {
    pub fn id(&self) -> Option<&str> {
        self.raw.get("id").or_else(|| self.raw.get("job_id")).and_then(Value::as_str).filter(|id| wire::valid_job_id(id))
    }
    pub fn status(&self) -> &str {
        match self.raw["status"].as_str() {
            Some(
                status @ ("queued"
                | "pending"
                | "running"
                | "processing"
                | "succeeded"
                | "partially_succeeded"
                | "failed"
                | "canceled"
                | "cancelled"
                | "expired"
                | "errored"),
            ) => status,
            _ => "unknown",
        }
    }
    pub fn actual_credits(&self) -> Option<u64> {
        credits(&self.raw["credits"])
    }
    pub fn succeeded(&self) -> bool {
        self.status() == "succeeded"
    }
}

pub struct Delivery {
    pub outputs: Vec<Output>,
    pub notes: Option<Value>,
}
pub struct Output {
    pub path: PathBuf,
    pub kind: OutputKind,
    pub mime: &'static str,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OutputKind {
    Audio,
    Midi,
    Notes,
    MusicXml,
}
impl OutputKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Audio => "audio",
            Self::Midi => "midi",
            Self::Notes => "notes",
            Self::MusicXml => "musicxml",
        }
    }
}
/// An already authorized, stable source. Inspection and transfer consume this handle, not its label.
/// A cloned handle shares its cursor; do not use that cursor concurrently with preparation.
pub struct Upload<'a> {
    pub file: File,
    pub extension: &'a str,
}
#[derive(Default)]
pub struct Uploads<'a> {
    pub audio: Option<Upload<'a>>,
    pub video: Option<Upload<'a>>,
}

/// Fixed-provider client. No credential-bearing state implements Debug.
pub struct Client {
    wire: wire::Client,
}
impl Client {
    pub fn new(key: &str) -> Result<Self, Error> {
        Self::with_base("https://api.mirelo.ai", key)
    }
    /// Protocol-fixture injection only, never a native or model setting.
    #[doc(hidden)]
    pub fn with_base(base: &str, key: &str) -> Result<Self, Error> {
        if key.trim().is_empty() {
            return Err(Error("Mirelo credentials are required"));
        }
        Ok(Self { wire: wire::Client::new(base, Some(key), false).map_err(|_| Error("Could not initialize Mirelo client"))? })
    }
    pub async fn prepare(&self, task: Task, input: &Value, uploads: Uploads<'_>, signal: &Signal) -> Result<Prepared, Error> {
        if input.get("job_id").is_some() || input.get("quote").is_some() {
            return Err(Error("Phased generation does not accept job_id or quote inputs"));
        }
        if signal.is_cancelled() {
            return Err(Error("Mirelo preparation cancelled"));
        }
        let (body, path, quote) = self.wire.prepare_request(task, input, None, uploads, signal).await.map_err(|error| {
            eprintln!("[warn] Mirelo preparation failed: {}", error.diagnostic());
            Error("Mirelo preparation failed; no generation submitted")
        })?;
        let credits = credits(&quote["credits"]).ok_or(Error("Mirelo preflight omitted a valid bounded credit estimate"))?;
        let (request, request_sha256, _) =
            self.wire.submission(task, body, path).map_err(|_| Error("Could not prepare Mirelo submission"))?;
        let request = request.build().map_err(|_| Error("Could not prepare Mirelo submission"))?;
        Ok(Prepared { task, request, credits, request_sha256 })
    }
    pub async fn submit(&self, prepared: Prepared, signal: &Signal) -> Result<Job, SubmitError> {
        if signal.is_cancelled() {
            return Err(SubmitError { error: Error("Mirelo submission cancelled before dispatch"), not_submitted: true });
        }
        let unknown = |_| SubmitError { error: Error("Mirelo submission outcome is unknown; do not submit again"), not_submitted: false };
        // Once this boundary is entered, even a refusal or a lost response is unknown.
        let request = RequestBuilder::from_parts(self.wire.http.clone(), prepared.request);
        let response = self.wire.send(request, signal).await.map_err(unknown)?;
        let raw = wire::decode(response, signal).await.map_err(unknown)?;
        Ok(Job { task: prepared.task, raw })
    }
    pub async fn wait(&self, job: Job, signal: &Signal) -> Result<Job, Error> {
        let id = job.id().map(str::to_owned);
        let raw = self
            .wire
            .wait_job(job.task, job.raw, signal, false)
            .await
            .map_err(|_| Error("Mirelo job polling failed; no generation was resubmitted"))?;
        let mut job = Job { task: job.task, raw };
        if let Some(id) = id {
            bind_id(&mut job, &id)?;
        }
        Ok(job)
    }
    /// Fetch one known job. Never uploads, creates, or retries a request.
    pub async fn get(&self, task: Task, id: &str, signal: &Signal) -> Result<Job, Error> {
        if !wire::valid_job_id(id) {
            return Err(Error("Invalid Mirelo job ID"));
        }
        let raw = self
            .wire
            .json(Method::GET, &wire::job_path(task, id), None, signal)
            .await
            .map_err(|_| Error("Mirelo job recovery failed; no generation was resubmitted"))?;
        let mut job = Job { task, raw };
        bind_id(&mut job, id)?;
        Ok(job)
    }
    /// Deliver into caller-owned scratch, bounded across all files including notes.
    /// The caller must checkpoint `actual_credits` before consuming this job.
    pub async fn deliver(&self, job: Job, output: &Path, max_bytes: u64, signal: &Signal) -> Result<Delivery, Error> {
        if !job.succeeded() {
            return Err(Error("Mirelo job has not succeeded; outputs were not delivered"));
        }
        let mut result = self
            .wire
            .finish(job.task, job.raw, Value::Null, output, signal, max_bytes.min(MAX_DELIVERY), true)
            .await
            .map_err(|_| Error("Mirelo output delivery failed; the recorded charge is unchanged"))?;
        let mut outputs = Vec::new();
        for file in result["files"].as_array().into_iter().flatten() {
            let path = file["path"].as_str().ok_or(Error("Mirelo output delivery returned an invalid file"))?;
            let path = PathBuf::from(path);
            let (kind, mime) = match file["type"].as_str() {
                Some("mid") => (OutputKind::Midi, "audio/midi"),
                Some("musicxml") => (OutputKind::MusicXml, "application/vnd.recordare.musicxml+xml"),
                Some("audio") => (
                    OutputKind::Audio,
                    match path.extension().and_then(|ext| ext.to_str()) {
                        Some("wav") => "audio/wav",
                        Some("flac") => "audio/flac",
                        Some("mp3") => "audio/mpeg",
                        Some("m4a") => "audio/mp4",
                        _ => return Err(Error("Mirelo returned an unsupported audio format")),
                    },
                ),
                _ => return Err(Error("Mirelo returned an unsupported output type")),
            };
            outputs.push(Output { path, kind, mime });
        }
        if let Some(path) = result["notes_path"].as_str() {
            outputs.push(Output { path: PathBuf::from(path), kind: OutputKind::Notes, mime: "application/json" });
        }
        let notes = result.get_mut("notes").filter(|notes| notes.is_array()).map(Value::take);
        Ok(Delivery { outputs, notes })
    }
}

fn credits(value: &Value) -> Option<u64> {
    value.as_u64().filter(|value| *value <= MAX_CREDITS)
}
fn bind_id(job: &mut Job, id: &str) -> Result<(), Error> {
    let object = job.raw.as_object_mut().ok_or(Error("Mirelo returned an invalid job receipt"))?;
    for field in ["id", "job_id"] {
        if object.get(field).is_some_and(|value| value.as_str() != Some(id)) {
            return Err(Error("Mirelo returned a different job identity"));
        }
    }
    object.insert("id".into(), Value::String(id.into()));
    Ok(())
}
