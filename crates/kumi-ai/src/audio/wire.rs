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
pub(super) struct Failure {
    pub message: String,
    pub availability: bool,
}
impl Failure {
    pub fn new(message: impl Into<String>) -> Self {
        Self { message: message.into(), availability: false }
    }
    pub fn runtime(self) -> RuntimeError {
        RuntimeError::plain(self.message)
    }
    fn submitted(mut self, job: &Value) -> Self {
        self.availability = false;
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
}
impl Client {
    pub fn new(base: &str, key: Option<&str>, local: bool) -> Result<Self, Failure> {
        let base = base_url(base).map_err(Failure::new)?;
        let builder = if local { kumi_net::model_servers() } else { kumi_net::client() };
        let http = builder
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(20))
            .timeout(Duration::from_secs(3600))
            .user_agent("Kumi native audio")
            .build()
            .map_err(|_| Failure::new("Could not construct audio HTTP client"))?;
        let transfers = kumi_net::web::guarded_http()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(20))
            .timeout(Duration::from_secs(300))
            .build()
            .map_err(|_| Failure::new("Could not construct guarded media client"))?;
        Ok(Self { http, transfers, base, key: key.map(str::to_owned), local })
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
        tokio::select! { biased;
            _ = signal.cancelled() => Err(Failure::new("Operation cancelled")),
            // Deliberately bypass kumi_net::send's proxy retry for possibly billed POSTs.
            response = request.send() => {
                let response = response.map_err(|_| Failure { message: "Audio server connection failed or timed out".into(), availability: true })?;
                let status = response.status();
                if !status.is_success() {
                    return Err(Failure { message: format!("Audio server returned HTTP {}", status.as_u16()), availability: status.is_server_error() || status.as_u16() == 429 });
                }
                Ok(response)
            }
        }
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
        if url.scheme() == "http" && (!self.local || url.origin() != self.base.origin()) {
            return Err(Failure::new("Unencrypted media transfer is allowed only to the configured loopback server"));
        }
        if !(self.local && url.origin() == self.base.origin()) {
            public_source(&url)?;
        }
        Ok(url)
    }
    fn transfer(&self, method: Method, url: Url) -> RequestBuilder {
        let client = if self.local && url.origin() == self.base.origin() { &self.http } else { &self.transfers };
        client.request(method, url)
    }
    pub async fn execute(&self, task: Task, input: &Value, model: Option<&str>, output: &Path, signal: &Signal) -> Result<Value, Failure> {
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
                return Ok(self.quote(task, quote));
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
                return Ok(self.quote(task, quote));
            }
            affordable(&quote, false)?;
            (body, path, quote)
        };
        if signal.is_cancelled() {
            return Err(Failure::new("Operation cancelled before submission"));
        }
        let midi = task == Task::AudioToMidi;
        let mut request =
            self.api(Method::POST, &if midi { path } else { format!("{path}?wait=25") })?.timeout(Duration::from_secs(120)).json(&body);
        if !midi {
            request = request.header("Idempotency-Key", uuid::Uuid::new_v4().to_string());
        }
        // Once this send is polled we cannot know whether a disconnected server accepted the work.
        let response = self.send(request, signal).await.map_err(|error| error.submitted(&Value::Null))?;
        let mut job = decode(response, signal).await.map_err(|error| error.submitted(&Value::Null))?;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(3600);
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
                    if midi {
                        job["job_id"]
                            .as_str()
                            .filter(|id| id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-'))
                            .map(|id| format!("/v2/audio-to-midi/v1.0/jobs/{id}"))
                    } else {
                        None
                    }
                })
                .ok_or_else(|| Failure::new("Audio job omitted its polling URL").submitted(&job))?;
            tokio::select! { biased;
                _ = signal.cancelled() => return Err(Failure::new("Polling cancelled; this API has no documented remote cancellation endpoint").submitted(&job)),
                _ = tokio::time::sleep(Duration::from_secs(2)) => {}
            }
            let next = self.json(Method::GET, &path, None, signal).await.map_err(|error| error.submitted(&job))?;
            job = next;
        }
        self.finish(task, job, quote, output, signal).await
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
        self.send(request, signal).await?;
        let id = required_string(&ticket, if midi { "asset_id" } else { "id" })?;
        Ok(if midi { json!({"type":"asset","asset_id":id}) } else { json!({"type":"asset","id":id}) })
    }
    async fn finish(&self, task: Task, job: Value, quote: Value, output: &Path, signal: &Signal) -> Result<Value, Failure> {
        let mut files = Vec::new();
        let mut failures = Vec::new();
        let mut result = json!({"provider":if self.local {"local"} else {"mirelo"},"task":task,
            "job_id":job.get("id").or_else(|| job.get("job_id")),"status":job["status"],"credits":job["credits"],
            "model":job.get("model").or_else(|| job.pointer("/result/model")),"errors":job.get("errors").or_else(|| job.get("error")),
            "attribution":if self.local {Value::Null} else {json!("Powered by Mirelo")}});
        if task == Task::AudioToMidi {
            result["estimated_credits"] = quote["credits"].clone();
            result["cost_basis"] = json!("Final charge uses server-measured duration, not the supplied quote duration");
            result["notes"] = job.pointer("/result/notes").cloned().unwrap_or(Value::Null);
            result["timing"] = job.pointer("/result/timing").cloned().unwrap_or(Value::Null);
            for (field, extension) in [("midi_url", "mid"), ("musicxml_url", "musicxml")] {
                if let Some(url) = job["result"][field].as_str() {
                    match self.download(url, extension, output, signal).await {
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
                                    Some(url) => self.download(url, extension, output, signal).await,
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
            match save_notes(output, &result["notes"], signal).await {
                Ok(path) => result["notes_path"] = json!(path),
                Err(error) => failures.push(error.message),
            }
        }
        result["files"] = json!(files);
        if files.is_empty()
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
    pub async fn download(&self, url: &str, extension: &str, output: &Path, signal: &Signal) -> Result<PathBuf, Failure> {
        let mut response = self.send(self.transfer(Method::GET, self.external(url)?), signal).await?;
        if response.content_length().is_some_and(|bytes| bytes > MAX_FILE) {
            return Err(Failure::new("Audio download exceeds 512 MiB"));
        }
        let (path, mut file) = output_file(output, extension).await?;
        let saved: Result<(), Failure> = async {
            let mut size = 0u64;
            let mut header = Vec::with_capacity(4);
            loop {
                let chunk = tokio::select! { biased;
                    _ = signal.cancelled() => return Err(Failure::new("Operation cancelled")),
                    chunk = response.chunk() => chunk.map_err(|_| Failure::new("Audio download was interrupted"))?
                };
                let Some(chunk) = chunk else { break };
                size += chunk.len() as u64;
                if size > MAX_FILE {
                    return Err(Failure::new("Audio download exceeds 512 MiB"));
                }
                if header.len() < 4 {
                    header.extend_from_slice(&chunk[..chunk.len().min(4 - header.len())]);
                }
                file.write_all(&chunk).await.map_err(|_| Failure::new("Could not write audio output"))?;
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
}
pub(super) async fn bounded(mut response: Response, limit: u64, signal: &Signal) -> Result<Vec<u8>, Failure> {
    if response.content_length().is_some_and(|bytes| bytes > limit) {
        return Err(Failure::new("Audio server response exceeds size limit"));
    }
    let mut bytes = Vec::new();
    loop {
        let chunk = tokio::select! { biased;
            _ = signal.cancelled() => return Err(Failure::new("Operation cancelled")),
            chunk = response.chunk() => chunk.map_err(|_| Failure::new("Audio response stream was interrupted"))?
        };
        let Some(chunk) = chunk else { break };
        if bytes.len() as u64 + chunk.len() as u64 > limit {
            return Err(Failure::new("Audio server response exceeds size limit"));
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}
async fn decode(response: Response, signal: &Signal) -> Result<Value, Failure> {
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
    Ok(Media {
        path: path.to_path_buf(),
        name: path.file_name().unwrap_or_default().to_string_lossy().into_owned(),
        mime,
        bytes: metadata.len(),
    })
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
            "audio" => matches!(task, Task::Extend | Task::Inpaint | Task::AudioToMidi),
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
        if input.get(field).is_some() {
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
    let notes = notes.as_array().ok_or_else(|| Failure::new("Audio-to-MIDI result omitted its authoritative notes"))?;
    for note in notes {
        let valid = note["pitch"].as_u64().is_some_and(|pitch| pitch <= 127)
            && note["velocity"].as_u64().is_some_and(|velocity| velocity <= 127)
            && note["start"].as_f64().is_some_and(|start| start.is_finite() && start >= 0.0)
            && note["end"].as_f64().is_some_and(|end| end.is_finite() && end > note["start"].as_f64().unwrap_or(f64::INFINITY))
            && note["instrument"].as_str().is_some();
        if !valid {
            return Err(Failure::new("Audio-to-MIDI returned malformed note data"));
        }
    }
    let bytes = serde_json::to_vec(notes).map_err(|_| Failure::new("Could not encode transcription notes"))?;
    save(output, "json", &bytes, signal).await
}
