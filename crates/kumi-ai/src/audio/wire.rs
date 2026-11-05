use super::Task;
use kumi_common::abort::Signal;
use kumi_types::errors::RuntimeError;
use reqwest::{multipart, Method, RequestBuilder, Response};
use serde_json::{json, Value};
use std::{
    path::{Path, PathBuf},
    time::Duration,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use url::Url;

const MAX_FILE: u64 = 512 * 1024 * 1024;
const MAX_JSON: u64 = 32 * 1024 * 1024;
const MAX_ERROR: u64 = 16 * 1024;
const MAX_OUTPUTS: usize = 32;
const DOWNLOAD_IDLE: Duration = Duration::from_secs(300);
const JOB_WAIT: Duration = Duration::from_secs(3600);
pub(super) struct Failure {
    pub message: String,
    pub availability: bool,
    status: Option<reqwest::StatusCode>,
    retry_after: Option<Duration>,
    billing: Option<String>,
}
impl Failure {
    pub fn new(message: impl Into<String>) -> Self {
        Self { message: message.into(), availability: false, status: None, retry_after: None, billing: None }
    }
    pub fn runtime(self) -> RuntimeError {
        RuntimeError::plain(self.message)
    }
    fn submitted(mut self, job: &Value) -> Self {
        self.availability = false;
        self.billing = Some(billing_notice(job));
        let id = job.get("id").or_else(|| job.get("job_id")).and_then(Value::as_str).unwrap_or("unknown (submission response lost)");
        self.message = format!("{}; job={id}, credits={}. Submission may have been billed or may still be running. Do not submit again automatically; no retry or fallback was attempted.", self.message, job.get("credits").unwrap_or(&Value::Null));
        self
    }
}
pub(super) fn base_url(value: &str) -> Result<Url, String> {
    let url = Url::parse(value).map_err(|_| "Invalid audio endpoint URL".to_string())?;
    secure_url(&url)?;
    if url.query().is_some() || url.fragment().is_some() {
        return Err("Audio endpoint must not contain a query or fragment".into());
    }
    Ok(url)
}
fn loopback(url: &Url) -> bool {
    url.host_str().is_some_and(|host| {
        host == "localhost" || host.trim_matches(['[', ']']).parse::<std::net::IpAddr>().is_ok_and(|ip| ip.is_loopback())
    })
}
fn secure_url(url: &Url) -> Result<(), String> {
    if url.host_str().is_none() || !url.username().is_empty() || url.password().is_some() {
        return Err("Audio URLs need a host and must not contain credentials".into());
    }
    if url.scheme() != "https" && !(url.scheme() == "http" && loopback(url)) {
        return Err("Remote audio endpoints require HTTPS; HTTP is allowed only on loopback".into());
    }
    Ok(())
}
pub(super) struct Client {
    pub http: reqwest::Client,
    transfers: reqwest::Client,
    base: Url,
    key: Option<String>,
    local: bool,
    #[cfg(test)]
    downloaded_bytes: Option<tokio::sync::watch::Sender<u64>>,
}
impl Client {
    pub fn new(base: &str, key: Option<&str>, local: bool) -> Result<Self, Failure> {
        let base = base_url(base).map_err(Failure::new)?;
        let builder = if local { kumi_net::model_servers() } else { kumi_net::client() };
        let http = builder
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(20))
            .user_agent("Kumi native audio")
            .build()
            .map_err(|_| Failure::new("Could not construct audio HTTP client"))?;
        let transfers = kumi_net::web::guarded_http()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(20))
            .build()
            .map_err(|_| Failure::new("Could not construct guarded media client"))?;
        Ok(Self {
            http,
            transfers,
            base,
            key: key.map(str::to_owned),
            local,
            #[cfg(test)]
            downloaded_bytes: None,
        })
    }
    pub fn api(&self, method: Method, path: &str) -> Result<RequestBuilder, Failure> {
        let url = if path.starts_with("https://") || path.starts_with("http://") {
            Url::parse(path).map_err(|_| Failure::new("Invalid audio polling URL"))?
        } else {
            let mut base = self.base.clone();
            if !base.path().ends_with('/') {
                base.set_path(&format!("{}/", base.path()));
            }
            base.join(path.trim_start_matches('/')).map_err(|_| Failure::new("Invalid audio API path"))?
        };
        let prefix = self.base.path().trim_end_matches('/');
        if url.origin() != self.base.origin()
            || !url.username().is_empty()
            || url.password().is_some()
            || (!prefix.is_empty() && url.path() != prefix && !url.path().starts_with(&format!("{prefix}/")))
        {
            return Err(Failure::new("Refusing to send audio credentials outside the configured API origin/path"));
        }
        let mut request = self.http.request(method, url).header("Mirelo-Version", "2026-08-28");
        if let Some(key) = &self.key {
            request = request.bearer_auth(key);
        }
        Ok(request)
    }
    pub async fn send(&self, request: RequestBuilder, signal: &Signal) -> Result<Response, Failure> {
        let response = tokio::select! { biased;
            _ = signal.cancelled() => return Err(Failure::new("Operation cancelled")),
            // Never replay POSTs/PUTs, including after a lost response.
            response = request.send() => response.map_err(|_| {
                let mut error = Failure::new("Audio server connection failed or timed out");
                error.availability = true;
                error
            })?
        };
        self.check_response(response, signal).await
    }
    async fn check_response(&self, response: Response, signal: &Signal) -> Result<Response, Failure> {
        let status = response.status();
        if status.is_success() {
            return Ok(response);
        }
        let retry_after = response_retry_after(&response);
        let body = bounded(response, MAX_ERROR, signal).await.ok().and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok());
        let mut error = Failure::new(format!(
            "Audio server returned HTTP {}{}",
            status.as_u16(),
            body.as_ref().map(|value| format!(": {}", self.diagnostic(value))).unwrap_or_default()
        ));
        error.status = Some(status);
        error.retry_after = retry_after;
        error.availability = (status.is_server_error() || status.as_u16() == 429)
            && body.as_ref().and_then(|body| body.pointer("/error/retryable")).and_then(Value::as_bool) != Some(false);
        Err(error)
    }
    pub fn diagnostic(&self, value: &Value) -> String {
        let source = value.get("error").filter(|value| value.is_object()).unwrap_or(value);
        let mut safe = serde_json::Map::new();
        // Keep machine-readable recovery fields ahead of possibly long human details.
        for field in ["code", "retryable", "credit_recovery", "param", "message", "detail", "error"] {
            if let Some(value) = source.get(field) {
                let encoded = self.redact(value.to_string());
                let bounded: String = encoded.chars().take(256).collect();
                safe.insert(field.into(), serde_json::from_str(&bounded).unwrap_or(Value::String(bounded)));
            }
        }
        self.redact(Value::Object(safe).to_string()).chars().take(1024).collect()
    }
    fn redact(&self, mut text: String) -> String {
        if let Some(key) = &self.key {
            text = text.replace(key, "[REDACTED]");
            // JSON escapes quotes/control characters in an echoed credential.
            if let Ok(encoded) = serde_json::to_string(key) {
                text = text.replace(&encoded[1..encoded.len() - 1], "[REDACTED]");
            }
        }
        text.chars().filter(|c| !c.is_control()).collect()
    }
    pub async fn json(&self, method: Method, path: &str, body: Option<&Value>, signal: &Signal) -> Result<Value, Failure> {
        let mut request = self.api(method, path)?.timeout(Duration::from_secs(120));
        if let Some(body) = body {
            request = request.json(body);
        }
        let response = self.send(request, signal).await?;
        decode(response, signal).await
    }
    pub fn external(&self, value: &str) -> Result<Url, Failure> {
        let url = Url::parse(value).map_err(|_| Failure::new("Invalid media transfer URL"))?;
        secure_url(&url).map_err(Failure::new)?;
        if url.scheme() == "http" && (!self.trusted_transfer(&url) || !loopback(&url)) {
            return Err(Failure::new("Unencrypted media transfer is allowed only to the configured loopback server"));
        }
        if !self.trusted_transfer(&url) {
            public_source(&url)?;
        }
        Ok(url)
    }
    fn trusted_transfer(&self, url: &Url) -> bool {
        url.origin() == self.base.origin() && (self.local || loopback(&self.base))
    }
    fn transfer(&self, method: Method, url: Url) -> RequestBuilder {
        let client = if self.trusted_transfer(&url) { &self.http } else { &self.transfers };
        client.request(method, url)
    }
    pub async fn execute(
        &self,
        task: Task,
        input: &Value,
        model: Option<&str>,
        output: &Path,
        signal: &Signal,
        notice: &dyn Fn(String),
    ) -> Result<Value, Failure> {
        let mut result = self.run(task, input, model, output, signal).await;
        match &mut result {
            Ok(value) if !self.local && value["submitted"] != false => notice(self.redact(billing_notice(value))),
            Err(error) => {
                error.message = self.redact(std::mem::take(&mut error.message));
                if !self.local {
                    if let Some(billing) = &error.billing {
                        notice(self.redact(billing.clone()));
                    }
                }
            }
            _ => {}
        }
        result
    }
    pub(super) async fn prepare_request(
        &self,
        task: Task,
        input: &Value,
        model: Option<&str>,
        signal: &Signal,
    ) -> Result<(Value, String, Value), Failure> {
        validate_input(task, input)?;
        if self.local && task != Task::AudioToMidi && model.is_none() && input.get("model").and_then(Value::as_str).is_none() {
            return Err(Failure::new("Configure an explicit release for this Mirelo-compatible SFX endpoint, or provide model"));
        }
        // Reject invalid local files before any upload, quote, or generation request.
        for (field, video) in [("audio", false), ("video", true)] {
            if let Some(value) = input.get(field) {
                let value = value.as_str().ok_or_else(|| Failure::new("Media must be an absolute file path or public URL"))?;
                if value.starts_with("https://") || value.starts_with("http://") {
                    public_source(&Url::parse(value).map_err(|_| Failure::new("Invalid media URL"))?)?;
                } else {
                    let file = media_file(value, video).await?;
                    if task == Task::AudioToMidi
                        && !matches!(file.mime, "audio/wav" | "audio/flac" | "audio/mpeg" | "audio/ogg" | "audio/aac")
                    {
                        return Err(Failure::new("Mirelo MIDI supports WAV, FLAC, MP3, OGG and AAC input only"));
                    }
                    if task == Task::AudioToMidi && file.bytes > 100 * 1024 * 1024 {
                        return Err(Failure::new("Mirelo MIDI input exceeds 100 MiB"));
                    }
                }
            }
        }
        let (body, path, quote) = if task == Task::AudioToMidi {
            let duration = integer(input, "duration_ms")?;
            if duration == 0 || duration > 1_800_000 {
                return Err(Failure::new("Audio-to-MIDI duration_ms must be 1..1800000 (up to 30 minutes)"));
            }
            let quote = self.json(Method::GET, &format!("/v2/audio-to-midi/v1.0/preflight?duration_ms={duration}"), None, signal).await?;
            if input["quote"] == true {
                return Ok((Value::Null, String::new(), quote));
            }
            affordable(&quote, true)?;
            let audio = self.source(required_string(input, "audio")?, false, true, signal).await?;
            let mut body = json!({"audio":audio});
            if let Some(instruments) = input.get("instruments") {
                body["instruments"] = instruments.clone();
            }
            (body, "/v2/audio-to-midi/v1.0/jobs".to_string(), quote)
        } else {
            let release = input.get("model").and_then(Value::as_str).or(model).unwrap_or(match task {
                Task::Extend | Task::Inpaint => "sfx-1.6",
                _ => "sfx-2.0",
            });
            if release == "latest" || release.is_empty() {
                return Err(Failure::new("Choose an explicit audio model release"));
            }
            let models = self.json(Method::GET, "/v3/models", None, signal).await?;
            let model = models["data"]
                .as_array()
                .and_then(|models| models.iter().find(|model| model["id"] == release))
                .ok_or_else(|| Failure::new(format!("Audio model release {release} is not available")))?;
            validate_model(task, input, model)?;
            let mut content = json!({});
            if let Some(prompt) = input.get("prompt") {
                content["prompt"] = prompt.clone();
            }
            for (field, video) in [("audio", false), ("video", true)] {
                if let Some(value) = input.get(field) {
                    content[field] = self
                        .source(
                            value.as_str().ok_or_else(|| Failure::new("Media must be a file path or public URL"))?,
                            video,
                            false,
                            signal,
                        )
                        .await?;
                }
            }
            let mut body = json!({"model":release,"input":content,"output":{"format":"wav"}});
            if task == Task::VideoToSfx {
                body["controls"] = json!({"preserve_speech":false});
            }
            match task {
                Task::TextToSfx | Task::VideoToSfx => {
                    if let Some(duration) = input.get("duration_ms") {
                        body["duration_ms"] = duration.clone();
                    }
                }
                Task::Extend => body["append_duration_ms"] = input["append_duration_ms"].clone(),
                Task::Inpaint => body["region"] = json!({"start_ms":input["region_start_ms"],"end_ms":input["region_end_ms"]}),
                Task::AudioToMidi => unreachable!(),
            }
            let family = match task {
                Task::Extend | Task::Inpaint => "edits",
                _ => "generations",
            };
            let path = format!("/v3/{}/{family}", task.as_str());
            let quote = self.json(Method::POST, &format!("{path}/preflight"), Some(&body), signal).await?;
            if input["quote"] == true {
                return Ok((Value::Null, String::new(), quote));
            }
            affordable(&quote, false)?;
            (body, path, quote)
        };
        Ok((body, path, quote))
    }
    async fn run(&self, task: Task, input: &Value, model: Option<&str>, output: &Path, signal: &Signal) -> Result<Value, Failure> {
        if let Some(id) = input.get("job_id") {
            if input.as_object().is_none_or(|object| object.len() != 1) {
                return Err(Failure::new("job_id recovery cannot be mixed with generation inputs"));
            }
            let id = id.as_str().filter(|id| valid_job_id(id)).ok_or_else(|| Failure::new("Invalid audio job_id"))?;
            let path = job_path(task, id);
            let job = json!({"id":id,"job_id":id,"status":"queued","urls":{"self":path}});
            return self.poll(task, job, Value::Null, output, signal, true).await;
        }
        let (body, path, quote) = self.prepare_request(task, input, model, signal).await?;
        if input["quote"] == true {
            return Ok(self.quote(task, quote));
        }
        if signal.is_cancelled() {
            return Err(Failure::new("Operation cancelled before submission"));
        }
        let (request, body_hash, idempotency_key) = self.submission(task, body, path)?;
        let lost = |error: Failure| {
            let mut error = error.submitted(&Value::Null);
            error.message.push_str(&format!(
                " Recovery: idempotency_key={}, body_sha256={body_hash}. Create was not replayed.",
                idempotency_key.as_deref().unwrap_or("unsupported by MIDI API")
            ));
            error
        };
        let response = self.send(request, signal).await.map_err(|mut error| {
            if error.status.is_some_and(|status| status.is_client_error()) {
                error.availability = false;
                error.billing = Some(format!("Mirelo create refused (HTTP {}); no job submitted.", error.status.unwrap().as_u16()));
                error.message.push_str("; create refused; not submitted. No automatic fallback was attempted.");
                error
            } else {
                lost(error)
            }
        })?;
        let job = decode(response, signal).await.map_err(lost)?;
        self.poll(task, job, quote, output, signal, false).await
    }
    pub(super) fn submission(&self, task: Task, body: Value, path: String) -> Result<(RequestBuilder, String, Option<String>), Failure> {
        use sha2::{Digest, Sha256};
        let midi = task == Task::AudioToMidi;
        let idempotency_key = (!midi).then(|| uuid::Uuid::new_v4().to_string());
        let encoded = serde_json::to_vec(&body).map_err(|_| Failure::new("Could not encode audio request"))?;
        let body_hash = hex::encode(Sha256::digest(&encoded));
        let mut request = self
            .api(Method::POST, &if midi { path } else { format!("{path}?wait=25") })?
            .timeout(Duration::from_secs(120))
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .body(encoded);
        if let Some(key) = &idempotency_key {
            request = request.header("Idempotency-Key", key);
        }
        Ok((request, body_hash, idempotency_key))
    }
    async fn poll(&self, task: Task, job: Value, quote: Value, output: &Path, signal: &Signal, immediate: bool) -> Result<Value, Failure> {
        let job = self.wait_job(task, job, signal, immediate).await?;
        self.finish(task, job, quote, output, signal, MAX_FILE, false).await
    }
    pub(super) async fn wait_job(&self, task: Task, mut job: Value, signal: &Signal, mut immediate: bool) -> Result<Value, Failure> {
        let deadline = tokio::time::Instant::now() + JOB_WAIT;
        let mut delay = Duration::from_secs(2);
        loop {
            let status = job["status"].as_str().unwrap_or("");
            if matches!(status, "succeeded" | "partially_succeeded" | "failed" | "canceled" | "cancelled" | "expired" | "errored") {
                break;
            }
            if tokio::time::Instant::now() >= deadline {
                return Err(Failure::new("Audio job polling timed out").submitted(&job));
            }
            let path = job
                .pointer("/urls/self")
                .or_else(|| job.get("job_url"))
                .and_then(Value::as_str)
                .map(str::to_owned)
                .or_else(|| {
                    job.get("id")
                        .or_else(|| job.get("job_id"))
                        .and_then(Value::as_str)
                        .filter(|id| valid_job_id(id))
                        .map(|id| job_path(task, id))
                })
                .ok_or_else(|| Failure::new("Audio job omitted its polling URL").submitted(&job))?;
            if !immediate {
                poll_wait(delay, deadline, signal).await.map_err(|error| error.submitted(&job))?;
            }
            immediate = false;
            let mut backoff = Duration::from_secs(1);
            loop {
                let attempt = async {
                    let response = self.send(self.api(Method::GET, &path)?.timeout(Duration::from_secs(120)), signal).await?;
                    let delay = response_retry_after(&response).unwrap_or(Duration::from_secs(2));
                    let next = decode(response, signal).await?;
                    Ok::<_, Failure>((next, delay))
                };
                let next = tokio::select! { biased;
                    _ = signal.cancelled() => return Err(Failure::new("Polling cancelled; remote job was not cancelled").submitted(&job)),
                    _ = tokio::time::sleep_until(deadline) => return Err(Failure::new("Audio job polling timed out").submitted(&job)),
                    next = attempt => next
                };
                match next {
                    Ok((next, next_delay)) => {
                        job = next;
                        delay = next_delay;
                        break;
                    }
                    Err(error) if error.availability => {
                        poll_wait(error.retry_after.unwrap_or(backoff), deadline, signal).await.map_err(|error| error.submitted(&job))?;
                        backoff = (backoff * 2).min(Duration::from_secs(30));
                    }
                    Err(error) => return Err(error.submitted(&job)),
                }
            }
        }
        Ok(job)
    }
    fn quote(&self, task: Task, quote: Value) -> Value {
        json!({"provider":if self.local {"local"} else {"mirelo"},"task":task,"quote":quote,"submitted":false,
            "attribution":if self.local {Value::Null} else {json!("Powered by Mirelo")},
            "cost_basis":if task == Task::AudioToMidi {"Estimate from supplied duration_ms; final credits use server-measured media duration"} else {"Preflight for this exact request"}})
    }
    async fn source(&self, value: &str, video: bool, midi: bool, signal: &Signal) -> Result<Value, Failure> {
        if value.starts_with("https://") || value.starts_with("http://") {
            let url = Url::parse(value).map_err(|_| Failure::new("Invalid media URL"))?;
            public_source(&url)?;
            return Ok(if midi { json!({"type":"url","audio_url":value}) } else { json!({"type":"url","url":value}) });
        }
        let media = media_file(value, video).await?;
        if midi && !matches!(media.mime, "audio/wav" | "audio/flac" | "audio/mpeg" | "audio/ogg" | "audio/aac") {
            return Err(Failure::new("Mirelo MIDI supports WAV, FLAC, MP3, OGG and AAC input only"));
        }
        if midi && media.bytes > 100 * 1024 * 1024 {
            return Err(Failure::new("Mirelo MIDI input exceeds 100 MiB"));
        }
        let ticket = self
            .json(Method::POST, if midi { "/v2/assets" } else { "/v3/assets" }, Some(&json!({"content_type":media.mime})), signal)
            .await?;
        let upload = self.external(required_string(&ticket, "upload_url")?)?;
        let file = tokio::fs::File::open(&media.path).await.map_err(|_| Failure::new("Could not open media file"))?;
        let request = if midi {
            self.transfer(Method::PUT, upload)
                .header(reqwest::header::CONTENT_TYPE, media.mime)
                .header(reqwest::header::CONTENT_LENGTH, media.bytes)
                .body(file)
        } else {
            let maximum = ticket["max_bytes"].as_u64().ok_or_else(|| Failure::new("Upload ticket omitted max_bytes"))?;
            if media.bytes > maximum {
                return Err(Failure::new(format!("Media exceeds this upload ticket's {maximum}-byte limit")));
            }
            let fields = ticket["fields"].as_object().ok_or_else(|| Failure::new("Upload ticket omitted form fields"))?;
            let mut form = multipart::Form::new();
            for (name, value) in fields {
                form = form.text(name.clone(), value.as_str().ok_or_else(|| Failure::new("Invalid upload form field"))?.to_owned());
            }
            let part = multipart::Part::stream_with_length(file, media.bytes)
                .file_name(media.name)
                .mime_str(media.mime)
                .map_err(|_| Failure::new("Invalid media MIME type"))?;
            self.transfer(Method::POST, upload).multipart(form.part("file", part))
        };
        self.send(request.timeout(DOWNLOAD_IDLE), signal).await?;
        let id = required_string(&ticket, if midi { "asset_id" } else { "id" })?;
        Ok(if midi { json!({"type":"asset","asset_id":id}) } else { json!({"type":"asset","id":id}) })
    }
    pub(super) async fn finish(
        &self,
        task: Task,
        job: Value,
        quote: Value,
        output: &Path,
        signal: &Signal,
        max_bytes: u64,
        native: bool,
    ) -> Result<Value, Failure> {
        if output_count(&job) > MAX_OUTPUTS {
            return Err(Failure::new("Audio job exceeds Kumi's limit of 32 downloadable files; no files downloaded. Recover this job after arranging a smaller result; do not recreate it.").submitted(&job));
        }
        let mut files = Vec::new();
        let mut remaining_bytes = max_bytes.min(MAX_FILE);
        let mut failures = Vec::new();
        let mut result = json!({"provider":if self.local {"local"} else {"mirelo"},"task":task,
            "job_id":job.get("id").or_else(|| job.get("job_id")),"status":job["status"],"credits":job["credits"],
            "model":job.get("model").or_else(|| job.pointer("/result/model")),"errors":job.get("errors").or_else(|| job.get("error")),
            "attribution":if self.local {Value::Null} else {json!("Powered by Mirelo")}});
        if task == Task::AudioToMidi {
            result["estimated_credits"] = quote["credits"].clone();
            result["cost_basis"] = json!("Final charge uses server-measured duration, not the supplied quote duration");
            result["notes"] = job.pointer("/result/notes").cloned().unwrap_or(Value::Null);
            if result["notes"].is_array() {
                normalize_notes(&mut result["notes"]).map_err(|error| error.submitted(&job))?;
                if native {
                    for note in result["notes"].as_array_mut().unwrap() {
                        note.as_object_mut()
                            .unwrap()
                            .retain(|field, _| matches!(field.as_str(), "pitch" | "velocity" | "start" | "end" | "instrument"));
                    }
                }
            }
            result["timing"] = job.pointer("/result/timing").cloned().unwrap_or(Value::Null);
            for (field, extension) in [("midi_url", "mid"), ("musicxml_url", "musicxml")] {
                if let Some(url) = job["result"][field].as_str() {
                    match self.download(url, extension, output, signal, &mut remaining_bytes).await {
                        Ok(path) => {
                            if field == "midi_url" {
                                result["midi_path"] = json!(path);
                            }
                            files.push(json!({"path":path,"type":extension}));
                        }
                        Err(error) => failures.push(error.message),
                    }
                }
            }
        } else {
            let outputs: Vec<&Value> = if let Some(output) = job.pointer("/result/output") {
                vec![output]
            } else {
                job.pointer("/result/outputs").and_then(Value::as_array).map(|values| values.iter().collect()).unwrap_or_default()
            };
            for item in outputs {
                if item["status"] != "succeeded" {
                    failures.push("An output did not fully succeed".into());
                }
                if let Some(variants) = item["variants"].as_array() {
                    for variant in variants {
                        if variant["status"] != "succeeded" {
                            failures.push("A variant failed".into());
                            continue;
                        }
                        let mut found = false;
                        for kind in ["audio", "video"] {
                            if let Some(file) = variant["files"].get(kind) {
                                found = true;
                                if native && kind == "video" {
                                    failures.push("Native generation does not support video output".into());
                                    continue;
                                }
                                let extension = if kind == "video" {
                                    "mp4"
                                } else {
                                    match file["format"].as_str() {
                                        Some("flac") => "flac",
                                        Some(value) if value.starts_with("mp3_") => "mp3",
                                        Some(value) if value.starts_with("m4a_") => "m4a",
                                        _ => "wav",
                                    }
                                };
                                let saved = match file["url"].as_str() {
                                    Some(url) => self.download(url, extension, output, signal, &mut remaining_bytes).await,
                                    None => Err(Failure::new("Succeeded output omitted its URL")),
                                };
                                match saved { Ok(path) => files.push(json!({"path":path,"type":kind,"output_index":item["index"],"variant_index":variant["index"],"start_ms":item["start_ms"],"end_ms":item["end_ms"],"label":item["label"],"sample_rate":file["sample_rate"]})), Err(error) => failures.push(error.message) }
                            }
                        }
                        if !found {
                            failures.push("Succeeded variant had no downloadable files".into());
                        }
                    }
                }
            }
        }
        if task == Task::AudioToMidi && result["notes"].is_array() {
            match save_notes_bounded(output, &result["notes"], signal, if native { remaining_bytes } else { u64::MAX }).await {
                Ok(path) => result["notes_path"] = json!(path),
                Err(error) => failures.push(error.message),
            }
        }
        result["files"] = json!(files);
        if (files.is_empty() && !(native && result["notes_path"].is_string()))
            || !matches!(job["status"].as_str(), Some("succeeded"))
            || !failures.is_empty()
            || (task == Task::AudioToMidi && !result["notes"].is_array())
        {
            result["download_errors"] = json!(failures);
            if let Some(notes) = result.as_object_mut().and_then(|result| result.remove("notes")) {
                result["note_count"] = json!(notes.as_array().map(Vec::len));
            }
            return Err(
                Failure::new(format!("Audio job failed or only partially completed. Saved outputs and status: {result}")).submitted(&job)
            );
        }
        Ok(result)
    }
    async fn download(
        &self,
        url: &str,
        extension: &str,
        output: &Path,
        signal: &Signal,
        remaining_bytes: &mut u64,
    ) -> Result<PathBuf, Failure> {
        if *remaining_bytes == 0 {
            return Err(Failure::new("Audio job exhausted its 512 MiB aggregate download budget; previously saved outputs were retained"));
        }
        let mut url = self.external(url)?;
        let mut response = self.download_response(&mut url, signal).await?;
        if response.content_length().is_some_and(|bytes| bytes > *remaining_bytes) {
            *remaining_bytes = 0;
            return Err(Failure::new(
                "Audio job exceeds its remaining 512 MiB aggregate download budget; previously saved outputs were retained",
            ));
        }
        let (path, mut file) = output_file(output, extension).await?;
        let saved: Result<(), Failure> = async {
            let mut size = 0u64;
            let mut header = Vec::with_capacity(4);
            loop {
                let chunk = tokio::select! { biased;
                    _ = signal.cancelled() => return Err(Failure::new("Operation cancelled")),
                    chunk = tokio::time::timeout(DOWNLOAD_IDLE, response.chunk()) => chunk
                        .map_err(|_| Failure::new("Audio download exceeded 300 seconds without data"))?
                        .map_err(|_| Failure::new("Audio download was interrupted"))?
                };
                let Some(chunk) = chunk else { break };
                size += chunk.len() as u64;
                if chunk.len() as u64 > *remaining_bytes {
                    *remaining_bytes = 0;
                    return Err(Failure::new(
                        "Audio job exceeded its 512 MiB aggregate download budget while streaming; previously saved outputs were retained",
                    ));
                }
                // Count transferred bytes even if this incomplete file is later removed.
                *remaining_bytes -= chunk.len() as u64;
                if header.len() < 4 {
                    header.extend_from_slice(&chunk[..chunk.len().min(4 - header.len())]);
                }
                file.write_all(&chunk).await.map_err(|_| Failure::new("Could not write audio output"))?;
                #[cfg(test)]
                if let Some(written) = &self.downloaded_bytes {
                    written.send_replace(size);
                }
            }
            if size == 0 || (extension == "mid" && header != b"MThd") {
                return Err(Failure::new("Audio server returned an empty or invalid output file"));
            }
            file.sync_all().await.map_err(|_| Failure::new("Could not persist audio output"))?;
            Ok(())
        }
        .await;
        if let Err(error) = saved {
            drop(file);
            let _ = tokio::fs::remove_file(&path).await;
            return Err(error);
        }
        Ok(path)
    }
    async fn download_response(&self, url: &mut Url, signal: &Signal) -> Result<Response, Failure> {
        let mut left_origin = !self.trusted_transfer(url);
        for hop in 0..=5 {
            let response = tokio::select! { biased;
                _ = signal.cancelled() => return Err(Failure::new("Operation cancelled")),
                response = tokio::time::timeout(DOWNLOAD_IDLE, self.transfer(Method::GET, url.clone()).send()) =>
                    response.map_err(|_| Failure::new("Audio download headers timed out"))?
                        .map_err(|_| Failure::new("Audio download connection failed"))?
            };
            if !response.status().is_redirection() {
                return self.check_response(response, signal).await;
            }
            if hop == 5 {
                return Err(Failure::new("Audio download exceeded five redirects"));
            }
            let location = response
                .headers()
                .get(reqwest::header::LOCATION)
                .and_then(|value| value.to_str().ok())
                .ok_or_else(|| Failure::new("Audio redirect omitted a valid Location"))?;
            let next = self.download_redirect(url, location, left_origin)?;
            left_origin |= next.origin() != self.base.origin();
            *url = next;
        }
        unreachable!()
    }
    fn download_redirect(&self, current: &Url, location: &str, left_origin: bool) -> Result<Url, Failure> {
        let next = current.join(location).map_err(|_| Failure::new("Invalid audio redirect URL"))?;
        if current.scheme() == "https" && next.scheme() != "https" {
            return Err(Failure::new("Refusing HTTPS downgrade in audio redirect"));
        }
        self.external(next.as_str())?;
        if left_origin {
            public_source(&next)?;
        }
        Ok(next)
    }
}
fn billing_notice(job: &Value) -> String {
    let id = job.get("id").or_else(|| job.get("job_id")).and_then(Value::as_str).filter(|id| valid_job_id(id)).unwrap_or("unknown");
    let credits = job["credits"]
        .as_f64()
        .filter(|credits| credits.is_finite() && *credits >= 0.0)
        .map(|credits| credits.to_string())
        .unwrap_or_else(|| "unknown; the job may still be running or billed".into());
    format!("Mirelo job {id}: credits {credits}.")
}
fn response_retry_after(response: &Response) -> Option<Duration> {
    let value = response.headers().get(reqwest::header::RETRY_AFTER)?.to_str().ok()?;
    let headers = std::collections::BTreeMap::from([("retry-after".into(), value.to_owned())]);
    let milliseconds = kumi_net::web::retry_after(&headers, None, None)?;
    // Longer waits still hit the job deadline; avoid overflow on untrusted header numbers.
    Some(Duration::from_secs_f64((milliseconds / 1000.0).clamp(0.0, JOB_WAIT.as_secs_f64())))
}
async fn poll_wait(delay: Duration, deadline: tokio::time::Instant, signal: &Signal) -> Result<(), Failure> {
    tokio::select! { biased;
        _ = signal.cancelled() => Err(Failure::new("Polling cancelled; remote job was not cancelled")),
        _ = tokio::time::sleep_until(deadline) => Err(Failure::new("Audio job polling timed out")),
        _ = tokio::time::sleep(delay) => Ok(())
    }
}
pub(super) fn valid_job_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 128 && id.bytes().all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}
pub(super) fn job_path(task: Task, id: &str) -> String {
    if task == Task::AudioToMidi {
        format!("/v2/audio-to-midi/v1.0/jobs/{id}")
    } else {
        let family = if matches!(task, Task::Extend | Task::Inpaint) { "edits" } else { "generations" };
        format!("/v3/{}/{family}/{id}", task.as_str())
    }
}
fn output_count(job: &Value) -> usize {
    let outputs =
        job.pointer("/result/output").into_iter().chain(job.pointer("/result/outputs").and_then(Value::as_array).into_iter().flatten());
    let mut files = 0;
    let mut variants = 0;
    for (index, output) in outputs.enumerate() {
        if index >= MAX_OUTPUTS {
            return MAX_OUTPUTS + 1;
        }
        for variant in output["variants"].as_array().into_iter().flatten() {
            variants += 1;
            files += ["audio", "video"].iter().filter(|kind| variant["files"].get(**kind).is_some()).count();
            if files > MAX_OUTPUTS || variants > MAX_OUTPUTS {
                return MAX_OUTPUTS + 1;
            }
        }
    }
    files
}
fn normalize_notes(notes: &mut Value) -> Result<(), Failure> {
    for note in notes.as_array_mut().ok_or_else(|| Failure::new("Audio-to-MIDI omitted notes"))? {
        let pitch = note["pitch"]
            .as_f64()
            .filter(|pitch| pitch.is_finite() && (0.0..=127.0).contains(pitch) && pitch.fract() == 0.0)
            .ok_or_else(|| Failure::new("Audio-to-MIDI returned an invalid MIDI pitch"))?;
        let velocity = note["velocity"]
            .as_f64()
            .filter(|velocity| velocity.is_finite() && (0.0..=127.0).contains(velocity))
            .ok_or_else(|| Failure::new("Audio-to-MIDI returned an invalid MIDI velocity"))?;
        note["pitch"] = json!(pitch as u8);
        note["velocity"] = json!(velocity.round() as u8);
    }
    Ok(())
}
pub(super) async fn bounded(mut response: Response, limit: u64, signal: &Signal) -> Result<Vec<u8>, Failure> {
    if response.content_length().is_some_and(|bytes| bytes > limit) {
        return Err(Failure::new("Audio server response exceeds size limit"));
    }
    let mut bytes = Vec::new();
    loop {
        let chunk = tokio::select! { biased;
            _ = signal.cancelled() => return Err(Failure::new("Operation cancelled")),
            chunk = tokio::time::timeout(DOWNLOAD_IDLE, response.chunk()) => chunk
                .map_err(|_| Failure::new("Audio response stream timed out"))?
                .map_err(|_| {
                    let mut error = Failure::new("Audio response stream was interrupted");
                    error.availability = true;
                    error
                })?
        };
        let Some(chunk) = chunk else { break };
        if bytes.len() as u64 + chunk.len() as u64 > limit {
            return Err(Failure::new("Audio server response exceeds size limit"));
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}
pub(super) async fn decode(response: Response, signal: &Signal) -> Result<Value, Failure> {
    serde_json::from_slice(&bounded(response, MAX_JSON, signal).await?).map_err(|_| Failure::new("Audio server returned invalid JSON"))
}
pub(super) async fn save(output: &Path, extension: &str, bytes: &[u8], signal: &Signal) -> Result<PathBuf, Failure> {
    if signal.is_cancelled() {
        return Err(Failure::new("Operation cancelled"));
    }
    let (path, mut file) = output_file(output, extension).await?;
    let saved = async {
        file.write_all(bytes).await?;
        file.sync_all().await
    }
    .await;
    if saved.is_err() {
        let _ = tokio::fs::remove_file(&path).await;
        return Err(Failure::new("Could not save complete audio output"));
    }
    Ok(path)
}
async fn output_file(output: &Path, extension: &str) -> Result<(PathBuf, tokio::fs::File), Failure> {
    if !output.is_absolute() {
        return Err(Failure::new("Kumi audio output directory must be absolute"));
    }
    tokio::fs::create_dir_all(output).await.map_err(|_| Failure::new("Could not create Kumi audio output directory"))?;
    let path = output.join(format!("{}.{}", uuid::Uuid::new_v4(), extension));
    let mut options = tokio::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    options.mode(0o600);
    let file = options.open(&path).await.map_err(|_| Failure::new("Could not create Kumi audio output file"))?;
    Ok((path, file))
}
pub(super) struct Media {
    pub path: PathBuf,
    pub name: String,
    pub mime: &'static str,
    pub bytes: u64,
}
pub(super) async fn media_file(value: &str, video: bool) -> Result<Media, Failure> {
    if kumi_common::path::network_or_device(value) || kumi_common::path::reserved_device(value) || value.chars().any(char::is_control) {
        return Err(Failure::new("Media must be a local path without network, device or control characters"));
    }
    let path = Path::new(value);
    if !path.is_absolute() {
        return Err(Failure::new("Media must be an absolute local file path or public HTTPS URL"));
    }
    let metadata = tokio::fs::metadata(path).await.map_err(|_| Failure::new("Media file is missing or unreadable"))?;
    if !metadata.is_file() || metadata.len() == 0 || metadata.len() > MAX_FILE {
        return Err(Failure::new("Media must be a nonempty regular file no larger than 512 MiB"));
    }
    let extension = path.extension().and_then(|value| value.to_str()).unwrap_or("").to_ascii_lowercase();
    let mime = match (video, extension.as_str()) {
        (false, "wav" | "wave") => "audio/wav",
        (false, "flac") => "audio/flac",
        (false, "mp3") => "audio/mpeg",
        (false, "ogg" | "oga") => "audio/ogg",
        (false, "aac") => "audio/aac",
        (false, "m4a") => "audio/mp4",
        (true, "mp4" | "m4v") => "video/mp4",
        (true, "mov") => "video/quicktime",
        (true, "webm") => "video/webm",
        _ => return Err(Failure::new("Unsupported media file extension")),
    };
    let mut file = tokio::fs::File::open(path).await.map_err(|_| Failure::new("Could not read media file"))?;
    let mut header = [0; 16];
    let len = file.read(&mut header).await.map_err(|_| Failure::new("Could not read media header"))?;
    let header = &header[..len];
    let recognized = match mime {
        "audio/wav" => header.starts_with(b"RIFF") && header.get(8..12) == Some(b"WAVE"),
        "audio/flac" => header.starts_with(b"fLaC"),
        "audio/ogg" => header.starts_with(b"OggS"),
        "audio/mpeg" | "audio/aac" => header.starts_with(b"ID3") || (header.len() >= 2 && header[0] == 0xff && header[1] & 0xe0 == 0xe0),
        "video/webm" => header.starts_with(&[0x1a, 0x45, 0xdf, 0xa3]),
        _ => header.get(4..8) == Some(b"ftyp"),
    };
    if !recognized {
        return Err(Failure::new("Media header does not match its file type"));
    }
    Ok(Media { path: path.to_path_buf(), name: format!("input.{extension}"), mime, bytes: metadata.len() })
}
fn public_source(url: &Url) -> Result<(), Failure> {
    secure_url(url).map_err(Failure::new)?;
    if !kumi_net::web::public_host(url.host_str().unwrap_or(""), None) {
        return Err(Failure::new("Media URL must be publicly accessible, not a private or local host"));
    }
    Ok(())
}
pub(super) fn required_string<'a>(value: &'a Value, field: &str) -> Result<&'a str, Failure> {
    value[field].as_str().filter(|value| !value.trim().is_empty()).ok_or_else(|| Failure::new(format!("Missing or empty {field}")))
}
fn integer(value: &Value, field: &str) -> Result<u64, Failure> {
    value[field].as_u64().ok_or_else(|| Failure::new(format!("{field} must be a nonnegative integer")))
}
fn validate_input(task: Task, input: &Value) -> Result<(), Failure> {
    if !input.is_object() {
        return Err(Failure::new("Audio input must be an object"));
    }
    for field in input.as_object().into_iter().flat_map(|value| value.keys()) {
        let allowed = match field.as_str() {
            "quote" | "model" => true,
            "prompt" => task != Task::AudioToMidi,
            "audio" => matches!(task, Task::VideoToSfx | Task::Extend | Task::Inpaint | Task::AudioToMidi),
            "video" => matches!(task, Task::VideoToSfx | Task::Extend | Task::Inpaint),
            "duration_ms" => matches!(task, Task::TextToSfx | Task::VideoToSfx | Task::AudioToMidi),
            "append_duration_ms" => task == Task::Extend,
            "region_start_ms" | "region_end_ms" => task == Task::Inpaint,
            "instruments" => task == Task::AudioToMidi,
            _ => false,
        };
        if !allowed {
            return Err(Failure::new(format!("Unsupported {} input field: {field}", task.as_str())));
        }
    }
    if let Some(quote) = input.get("quote") {
        if !quote.is_boolean() {
            return Err(Failure::new("quote must be boolean"));
        }
    }
    if task == Task::TextToSfx {
        required_string(input, "prompt")?;
        integer(input, "duration_ms")?;
    }
    if let Some(prompt) = input.get("prompt") {
        if !prompt.is_string() || prompt.as_str().is_none_or(|value| value.trim().is_empty()) {
            return Err(Failure::new("prompt must be a nonempty string"));
        }
    }
    match task {
        Task::VideoToSfx => {
            required_string(input, "video")?;
        }
        Task::Extend => {
            required_string(input, "audio")?;
            integer(input, "append_duration_ms")?;
        }
        Task::Inpaint => {
            required_string(input, "audio")?;
            if integer(input, "region_end_ms")? <= integer(input, "region_start_ms")? {
                return Err(Failure::new("Inpaint end must be after start"));
            }
        }
        Task::AudioToMidi => {
            required_string(input, "audio")?;
        }
        _ => {}
    }
    if let Some(instruments) = input.get("instruments") {
        if !instruments
            .as_array()
            .is_some_and(|values| values.len() <= 35 && values.iter().all(|value| value.as_str().is_some_and(|value| !value.is_empty())))
        {
            return Err(Failure::new("instruments must be an array of up to 35 instrument names"));
        }
    }
    Ok(())
}
fn validate_model(task: Task, input: &Value, model: &Value) -> Result<(), Failure> {
    let operation = model["operations"]
        .get(task.as_str())
        .filter(|value| value.is_object())
        .ok_or_else(|| Failure::new("Selected model does not support this operation"))?;
    if !model["formats"].as_array().is_some_and(|formats| formats.iter().any(|format| format == "wav")) {
        return Err(Failure::new("Selected model does not support WAV output"));
    }
    if let Some(prompt) = input["prompt"].as_str() {
        if prompt.chars().count() as u64
            > model["max_prompt_chars"].as_u64().ok_or_else(|| Failure::new("Model omitted its prompt limit"))?
        {
            return Err(Failure::new("Prompt exceeds this model's limit"));
        }
    }
    let check = |field: &str, value: u64| -> Result<(), Failure> {
        let limit = operation.get(field).ok_or_else(|| Failure::new(format!("Selected model does not support {field}")))?;
        if limit["min"].as_u64().is_some_and(|min| value < min) || limit["max"].as_u64().is_some_and(|max| value > max) {
            return Err(Failure::new(format!("{field} is outside this model's limits")));
        }
        Ok(())
    };
    for field in ["duration_ms", "append_duration_ms", "region_start_ms"] {
        if input.get(field).is_some() && (field != "region_start_ms" || operation.get(field).is_some()) {
            check(field, integer(input, field)?)?;
        }
    }
    if task == Task::Inpaint {
        check("region_width_ms", integer(input, "region_end_ms")? - integer(input, "region_start_ms")?)?;
    }
    Ok(())
}
fn affordable(quote: &Value, midi: bool) -> Result<(), Failure> {
    if quote["credits"].as_u64().is_none() {
        return Err(Failure::new("Preflight omitted its credit quote; nothing submitted"));
    }
    let recovery = if midi {
        match quote["billing_mode"].as_str() {
            Some("unmetered") => return Ok(()),
            Some("metered") => Some(quote),
            _ => return Err(Failure::new("MIDI preflight omitted billing_mode; nothing submitted")),
        }
    } else {
        quote.get("credit_recovery")
    };
    if let Some(recovery) = recovery {
        if recovery["provisioning_state"] != "ready"
            || !recovery["recovery_action"].is_null()
            || recovery["credit_shortfall"].as_u64() != Some(0)
        {
            return Err(Failure::new(format!("Preflight cannot fund this request; no generation submitted. credits={}, provisioning_state={}, credit_shortfall={}, recovery_action={}", quote["credits"], recovery["provisioning_state"], recovery["credit_shortfall"], recovery["recovery_action"])));
        }
    }
    Ok(())
}

pub(super) async fn save_notes(output: &Path, notes: &Value, signal: &Signal) -> Result<PathBuf, Failure> {
    save_notes_bounded(output, notes, signal, u64::MAX).await
}
async fn save_notes_bounded(output: &Path, notes: &Value, signal: &Signal, max_bytes: u64) -> Result<PathBuf, Failure> {
    let notes = notes.as_array().ok_or_else(|| Failure::new("Audio-to-MIDI result omitted its authoritative notes"))?;
    for note in notes {
        let valid = note["pitch"].as_f64().is_some_and(|pitch| pitch.is_finite() && (0.0..=127.0).contains(&pitch) && pitch.fract() == 0.0)
            && note["velocity"].as_f64().is_some_and(|velocity| velocity.is_finite() && (0.0..=127.0).contains(&velocity))
            && note["start"].as_f64().is_some_and(|start| start.is_finite() && start >= 0.0)
            && note["end"].as_f64().is_some_and(|end| end.is_finite() && end > note["start"].as_f64().unwrap_or(f64::INFINITY))
            && note["instrument"].as_str().is_some();
        if !valid {
            return Err(Failure::new("Audio-to-MIDI returned malformed note data"));
        }
    }
    let bytes = serde_json::to_vec(notes).map_err(|_| Failure::new("Could not encode transcription notes"))?;
    if bytes.len() as u64 > max_bytes {
        return Err(Failure::new("Audio job exceeds its remaining output budget"));
    }
    save(output, "json", &bytes, signal).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redirect_policy_checks_each_hop_without_reopening_loopback_exception() {
        let client = Client::new("https://127.0.0.1:7443", Some("secret"), false).ok().unwrap();
        let public = Url::parse("https://example.com/result").unwrap();
        let configured = Url::parse("https://127.0.0.1:7443/result").unwrap();
        assert!(client.download_redirect(&configured, "/next", false).is_ok());
        assert!(client.download_redirect(&configured, public.as_str(), false).is_ok());
        assert!(client.download_redirect(&public, configured.as_str(), true).is_err());
        assert!(client.download_redirect(&public, "https://192.168.1.2/file", true).is_err());
        assert!(client.download_redirect(&public, "http://example.com/file", true).is_err());
        assert!(client.download_redirect(&configured, "http://127.0.0.1:7443/file", false).is_err());
        assert!(client.download_redirect(&public, "https://user:secret@example.com/file", true).is_err());
        assert!(client.download_redirect(&public, "//example.org/file", true).is_ok());
    }

    #[tokio::test(start_paused = true)]
    async fn polling_wait_honors_deadline_and_cancellation() {
        let start = tokio::time::Instant::now();
        let error = poll_wait(Duration::from_secs(9000), start + Duration::from_secs(3600), &Signal::new()).await.err().unwrap();
        assert!(error.message.contains("timed out"));
        assert_eq!(tokio::time::Instant::now() - start, Duration::from_secs(3600));
        let signal = Signal::new();
        signal.cancel();
        assert!(poll_wait(Duration::from_secs(300), tokio::time::Instant::now() + JOB_WAIT, &signal)
            .await
            .err()
            .unwrap()
            .message
            .contains("cancelled"));
    }

    #[test]
    fn midi_numbers_require_whole_pitch_and_bounded_velocity() {
        for (pitch, velocity) in [(60.5, 100.0), (-1.0, 100.0), (128.0, 100.0), (60.0, -0.1), (60.0, 127.1)] {
            assert!(normalize_notes(&mut json!([{"pitch":pitch,"velocity":velocity}])).is_err());
        }
        let mut notes = json!([{"pitch":60.0,"velocity":100.6}]);
        normalize_notes(&mut notes).ok().unwrap();
        assert_eq!(notes[0], json!({"pitch":60,"velocity":101}));
    }

    #[tokio::test]
    async fn download_timeout_is_per_chunk_not_total_and_partial_files_are_removed() {
        async fn advance_clock(seconds: u64) {
            tokio::time::pause();
            tokio::time::advance(Duration::from_secs(seconds)).await;
            tokio::time::resume();
        }

        for stalls in [false, true] {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let base = format!("http://{}", listener.local_addr().unwrap());
            let (send, mut receive) = tokio::sync::mpsc::channel::<bool>(1);
            let writer = tokio::spawn(async move {
                let (mut stream, _) = listener.accept().await.unwrap();
                let mut request = [0; 2048];
                stream.read(&mut request).await.unwrap();
                stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 12\r\nConnection: close\r\n\r\nRIFF").await.unwrap();
                while let Some(last) = receive.recv().await {
                    stream.write_all(if last { b"WAVE" } else { b"0000" }).await.unwrap();
                    if last {
                        break;
                    }
                }
            });
            let (written, mut progress) = tokio::sync::watch::channel(0u64);
            let mut client = Client::new(&base, None, true).ok().unwrap();
            client.downloaded_bytes = Some(written);
            let directory = tempfile::tempdir().unwrap();
            let output = directory.path().join("output");
            let download_output = output.clone();
            let download = tokio::spawn(async move {
                let signal = Signal::new();
                let url = format!("{base}/file");
                let mut remaining_bytes = MAX_FILE;
                client.download(&url, "wav", &download_output, &signal, &mut remaining_bytes).await
            });

            // Observe completed writes, not OS-dependent visibility of a buffered file's length.
            // Keep time running during real I/O; pause only for the explicit timeout advances.
            assert_eq!(
                *tokio::time::timeout(Duration::from_secs(10), progress.wait_for(|&bytes| bytes >= 4))
                    .await
                    .expect("first chunk was not written")
                    .expect("download ended before the first chunk"),
                4
            );
            if stalls {
                advance_clock(301).await;
            } else {
                advance_clock(200).await;
                send.send(false).await.unwrap();
                assert_eq!(
                    *tokio::time::timeout(Duration::from_secs(10), progress.wait_for(|&bytes| bytes >= 8))
                        .await
                        .expect("second chunk was not written")
                        .expect("download ended before the second chunk"),
                    8
                );
                advance_clock(200).await;
                send.send(true).await.unwrap();
            }
            let result = tokio::time::timeout(Duration::from_secs(10), download)
                .await
                .expect("download did not finish after driving the chunks")
                .unwrap();
            writer.abort();
            if stalls {
                assert!(result.err().unwrap().message.contains("without data"));
                assert!(tokio::fs::read_dir(&output).await.unwrap().next_entry().await.unwrap().is_none());
            } else {
                assert_eq!(tokio::fs::read(result.ok().unwrap()).await.unwrap(), b"RIFF0000WAVE");
            }
        }
    }

    #[tokio::test]
    async fn aggregate_budget_rejects_declared_and_streamed_overruns_retaining_prior_files() {
        for chunked in [false, true] {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let base = format!("http://{}", listener.local_addr().unwrap());
            let requests = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
            let recorded = requests.clone();
            let server = tokio::spawn(async move {
                loop {
                    let (mut stream, _) = listener.accept().await.unwrap();
                    recorded.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    let mut request = [0; 2048];
                    stream.read(&mut request).await.unwrap();
                    let response = if chunked {
                        "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n8\r\nRIFF0000\r\n0\r\n\r\n"
                    } else {
                        "HTTP/1.1 200 OK\r\nContent-Length: 8\r\nConnection: close\r\n\r\nRIFF0000"
                    };
                    let _ = stream.write_all(response.as_bytes()).await;
                }
            });
            let client = Client::new(&base, None, true).ok().unwrap();
            let directory = tempfile::tempdir().unwrap();
            let signal = Signal::new();
            // Exercise the same production budget arithmetic with a tiny allowance.
            let mut remaining_bytes = 12;
            let first = client.download(&base, "wav", directory.path(), &signal, &mut remaining_bytes).await.ok().unwrap();
            assert_eq!(remaining_bytes, 4);
            let error = client.download(&base, "wav", directory.path(), &signal, &mut remaining_bytes).await.err().unwrap();
            assert!(
                error.message.contains("aggregate download budget") && error.message.contains("previously saved outputs were retained")
            );
            assert_eq!(error.message.contains("while streaming"), chunked);
            assert_eq!(remaining_bytes, 0);
            assert_eq!(tokio::fs::read(&first).await.unwrap(), b"RIFF0000");
            let mut entries = tokio::fs::read_dir(directory.path()).await.unwrap();
            assert!(entries.next_entry().await.unwrap().is_some());
            assert!(entries.next_entry().await.unwrap().is_none(), "only the failed partial file is removed");
            assert!(client.download(&base, "wav", directory.path(), &signal, &mut remaining_bytes).await.is_err());
            assert_eq!(requests.load(std::sync::atomic::Ordering::SeqCst), 2, "an exhausted job must not transfer another file");
            server.abort();
        }
    }
}
