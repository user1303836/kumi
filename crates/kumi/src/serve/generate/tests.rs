use super::*;
use crate::serve::Args;
use crate::serve::ctx::{CALL_PRIVATE, Host, Private, Progress};
use kumi_types::{daw::DawId, system::Env};
use std::collections::BTreeMap;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicU64, Ordering},
};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const KEY: &str = "mirelo-private-sentinel-54b7a0";

#[derive(Clone)]
struct Request {
    method: String,
    path: String,
    body: Vec<u8>,
}
struct Server {
    url: String,
    requests: Arc<Mutex<Vec<Request>>>,
    download_fails: Arc<AtomicBool>,
    lose_create: Arc<AtomicBool>,
    actual_credits: Arc<AtomicU64>,
    midi_file: Arc<AtomicBool>,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for Server {
    fn drop(&mut self) {
        self.task.abort();
    }
}
impl Server {
    async fn new() -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let base = url.clone();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let recorded = requests.clone();
        let download_fails = Arc::new(AtomicBool::new(false));
        let failing = download_fails.clone();
        let lose_create = Arc::new(AtomicBool::new(false));
        let lost = lose_create.clone();
        let actual_credits = Arc::new(AtomicU64::new(12));
        let charged = actual_credits.clone();
        let midi_file = Arc::new(AtomicBool::new(false));
        let has_midi = midi_file.clone();
        let task = tokio::spawn(async move {
            loop {
                let (mut stream, _) = listener.accept().await.unwrap();
                let mut bytes = Vec::new();
                let mut buffer = [0_u8; 8192];
                let end = loop {
                    let n = stream.read(&mut buffer).await.unwrap();
                    if n == 0 {
                        break None;
                    }
                    bytes.extend_from_slice(&buffer[..n]);
                    if let Some(end) = bytes.windows(4).position(|bytes| bytes == b"\r\n\r\n") {
                        break Some(end + 4);
                    }
                };
                let Some(end) = end else {
                    continue;
                };
                let header = String::from_utf8_lossy(&bytes[..end]).into_owned();
                let mut words = header.split_whitespace();
                let method = words.next().unwrap().to_owned();
                let path = words.next().unwrap().to_owned();
                let length = header
                    .lines()
                    .filter_map(|line| line.split_once(':'))
                    .find(|(key, _)| key.eq_ignore_ascii_case("content-length"))
                    .map(|(_, value)| value.trim().parse::<usize>().unwrap())
                    .unwrap_or(0);
                while bytes.len() < end + length {
                    let n = stream.read(&mut buffer).await.unwrap();
                    if n == 0 {
                        break;
                    }
                    bytes.extend_from_slice(&buffer[..n]);
                }
                recorded.lock().unwrap().push(Request { method: method.clone(), path: path.clone(), body: bytes[end..].to_vec() });
                if method == "POST" && path.ends_with("?wait=25") && lost.load(Ordering::SeqCst) {
                    continue;
                }
                let mut status = 200;
                let mut mime = "application/json";
                let value = match path.as_str() {
                    "/v3/models" => json!({"data": [{"id": "sfx-2.0", "max_prompt_chars": 500, "formats": ["wav"],
                        "operations": {"text-to-sfx": {"duration_ms": {"min": 100, "max": 30000}}}}]}),
                    "/v3/text-to-sfx/generations/preflight" => json!({"credits": 15}),
                    "/v2/audio-to-midi/v1.0/preflight?duration_ms=1000" => json!({"credits": 3, "billing_mode": "unmetered"}),
                    "/v2/assets" => json!({"asset_id": "approved-audio", "upload_url": format!("{base}/put")}),
                    "/put" => json!({}),
                    "/v2/audio-to-midi/v1.0/jobs" | "/v2/audio-to-midi/v1.0/jobs/midi-job" => json!({
                        "job_id": "midi-job", "status": "succeeded", "credits": 3,
                        "result": {"midi_url": if has_midi.load(Ordering::SeqCst) { json!(format!("{base}/midi")) } else { Value::Null },
                            "notes": [{"pitch": 60, "velocity": 101, "start": 0.0, "end": 0.5, "instrument": "acoustic_piano"}]}
                    }),
                    "/download" | "/midi" => Value::Null,
                    _ => json!({"id": "sfx-job", "status": "succeeded", "credits": charged.load(Ordering::SeqCst), "result": {"output": {
                        "status": "succeeded", "variants": [{"status": "succeeded", "files": {
                            "audio": {"url": format!("{base}/download"), "format": "wav", "sample_rate": 8000}
                        }}]
                    }}}),
                };
                let body = if path == "/midi" {
                    mime = "audio/midi";
                    midi().to_vec()
                } else if path == "/download" {
                    if failing.load(Ordering::SeqCst) {
                        status = 404;
                        b"missing output".to_vec()
                    } else {
                        mime = "audio/wav";
                        wav()
                    }
                } else {
                    serde_json::to_vec(&value).unwrap()
                };
                let response = format!(
                    "HTTP/1.1 {status} Fixture\r\nContent-Type: {mime}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                if stream.write_all(response.as_bytes()).await.is_ok() {
                    let _ = stream.write_all(&body).await;
                }
            }
        });
        Self { url, requests, download_fails, lose_create, actual_credits, midi_file, task }
    }
    fn creates(&self) -> usize {
        self.requests
            .lock()
            .unwrap()
            .iter()
            .filter(|request| {
                request.method == "POST" && (request.path.ends_with("?wait=25") || request.path == "/v2/audio-to-midi/v1.0/jobs")
            })
            .count()
    }
}

fn wav() -> Vec<u8> {
    let data = 16_000_u32;
    let mut out = Vec::with_capacity(data as usize + 44);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(data + 36).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16_u32.to_le_bytes());
    out.extend_from_slice(&1_u16.to_le_bytes());
    out.extend_from_slice(&1_u16.to_le_bytes());
    out.extend_from_slice(&8000_u32.to_le_bytes());
    out.extend_from_slice(&16000_u32.to_le_bytes());
    out.extend_from_slice(&2_u16.to_le_bytes());
    out.extend_from_slice(&16_u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data.to_le_bytes());
    out.resize(data as usize + 44, 0);
    out
}

fn midi() -> &'static [u8] {
    b"MThd\0\0\0\x06\0\0\0\x01\0\x60MTrk\0\0\0\x0c\0\x90\x3c\x65\x60\x80\x3c\0\0\xff\x2f\0"
}

struct Fixture {
    _dir: tempfile::TempDir,
    data: PathBuf,
    source: PathBuf,
    ctx: Rc<Ctx>,
    ledger: Value,
}
impl Fixture {
    fn new(server: &Server) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path().canonicalize().unwrap();
        let source = data.join("samples");
        std::fs::create_dir(&source).unwrap();
        let ctx = Self::context(&data, &source, server);
        let ledger = json!({"schema": 2, "policy": {"acknowledged": true, "enabled": {"sfx": true, "audio_to_midi": true}, "cap": 100},
            "frozen": false, "freeze_reason": null, "quotes": {}, "reservations": {}, "archived_count": 0, "archived_spent": 0});
        let fixture = Self { _dir: dir, data, source, ctx, ledger };
        fixture.save();
        fixture
    }
    fn context(data: &Path, source: &Path, server: &Server) -> Rc<Ctx> {
        let args = Args {
            state_dir: data.join("kumi/state"),
            kumi_home: data.join("kumi/home"),
            hermes_home: None,
            hermes_root: None,
            daw: DawId::Reaper,
            library_roots: vec![source.to_owned()],
            media_roots: vec![],
            tools: BTreeMap::new(),
        };
        std::fs::create_dir_all(args.state_dir.parent().unwrap()).unwrap();
        let answers = kumi_daw::io::Answers::default().with(FixtureEndpoint(server.url.clone()));
        Ctx::new(args, Env::new(), answers).unwrap()
    }
    fn restart(self, server: &Server) -> Self {
        let Self { _dir, data, source, ctx, ledger } = self;
        ctx.snapshots().flush();
        drop(ctx);
        let ctx = Self::context(&data, &source, server);
        Self { _dir, data, source, ctx, ledger }
    }
    fn save(&self) {
        for (name, bytes) in [("ledger.lock", Vec::new()), ("ledger.json", serde_json::to_vec(&self.ledger).unwrap())] {
            let mut options = OpenOptions::new();
            options.create(true).write(true);
            if name == "ledger.json" {
                options.truncate(true);
            }
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            options.open(self.data.join(name)).unwrap().write_all(&bytes).unwrap();
        }
    }
    fn reserve(&mut self, quote: &Value, id: &str, create_call: &str) {
        let mut stored = quote.clone();
        stored["host_call_id"] = json!("quote-call");
        stored["consumed"] = json!(true);
        self.ledger["quotes"][quote["quote_id"].as_str().unwrap()] = stored;
        self.ledger["reservations"][id] = json!({
            "reservation_id": id, "quote_id": quote["quote_id"], "request_sha256": quote["request_sha256"],
            "host_call_id": create_call, "kind": quote["kind"], "reserved_credits": quote["estimate"]["max_credits"],
            "expires_at": quote["expires_at"], "created_at": now(), "job_id": null, "provider_job_id": null,
            "submission": "unknown", "settlement": "pending", "actual_credits": null
        });
        self.save();
    }
    async fn call(&self, args: Value, id: &str, reservation: Option<(&str, u64)>) -> Result<Value, OpError> {
        self.call_private(args, id, reservation, true, true).await
    }
    async fn call_private(
        &self,
        args: Value,
        id: &str,
        reservation: Option<(&str, u64)>,
        ack: bool,
        enabled: bool,
    ) -> Result<Value, OpError> {
        if let Some(reason) = kumi_agent::kernel::schema::problem(&schema(), &args) {
            return Err(OpError::Invalid { reason });
        }
        let mut sealed = json!({"api_key": KEY, "acknowledged": ack, "enabled": enabled});
        if let Some((id, amount)) = reservation {
            sealed["reservation_id"] = json!(id);
            sealed["reserved_credits"] = json!(amount);
        }
        let private = Private::from(serde_json::Map::from_iter([("_mirelo".into(), sealed)]));
        let call = Call {
            args: args.as_object().unwrap().clone(),
            host: Host {
                profile_id: Some("profile".into()),
                session_id: Some("session".into()),
                tool_call_id: Some(id.into()),
                private: private.clone(),
                ..Default::default()
            },
            signal: Signal::new(),
            progress: Progress::none(),
        };
        CALL_PRIVATE.scope(private, run(&self.ctx, call)).await
    }
    async fn done(&self, id: &str) -> Value {
        tokio::time::timeout(Duration::from_secs(10), self.ctx.jobs().all_settled()).await.unwrap();
        self.ctx.snapshots().flush();
        self.ctx.jobs().get(id, &self.ctx.paths().state_dir).unwrap()
    }
    fn assert_private(&self) {
        fn visit(path: &Path) {
            for entry in std::fs::read_dir(path).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    visit(&path);
                } else {
                    assert!(
                        !std::fs::read(&path).unwrap().windows(KEY.len()).any(|bytes| bytes == KEY.as_bytes()),
                        "private key in {}",
                        path.display()
                    );
                }
            }
        }
        visit(&self.data);
    }
}
fn sfx() -> Value {
    json!({"action": "quote", "kind": "sfx", "prompt": "a metallic impact", "seconds": 1})
}

#[tokio::test]
async fn a_reserved_native_quote_posts_once_retains_playable_bytes_and_exposes_real_billing() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let server = Server::new().await;
            let mut fixture = Fixture::new(&server);
            let quote = fixture.call(sfx(), "quote-call", None).await.unwrap();
            assert_eq!(server.creates(), 0);
            fixture.reserve(&quote, "r-one", "create-call");
            let created =
                fixture.call(json!({"action": "create", "quote_id": quote["quote_id"]}), "create-call", Some(("r-one", 15))).await.unwrap();
            let id = created["job_id"].as_str().unwrap();
            let job = fixture.done(id).await;
            assert_eq!(job["state"], "done", "{job}");
            assert_eq!(job["result"]["credits"], json!({"estimate": 15, "reserved": 15, "actual": 12}));
            assert_eq!(job["result"]["submission"], "submitted");
            let path = fixture.ctx.artifacts().resolve(&fixture.ctx, job["result"]["artifact_id"].as_str().unwrap()).unwrap();
            assert_eq!(std::fs::read(&path).unwrap(), wav());
            assert_eq!(job["result"]["media_tag"], format!("MEDIA:{}", path.display()));
            assert_eq!(
                fixture
                    .call(json!({"action": "create", "quote_id": quote["quote_id"]}), "another-call", Some(("r-one", 15)))
                    .await
                    .unwrap_err()
                    .code(),
                "quote_consumed"
            );
            assert_eq!(server.creates(), 1);
            let requests = server.requests.lock().unwrap();
            let preflight = requests.iter().find(|request| request.path.ends_with("/preflight")).unwrap();
            let create = requests.iter().find(|request| request.path.ends_with("?wait=25")).unwrap();
            assert_eq!(preflight.body, create.body);
            drop(requests);
            fixture.assert_private();
        })
        .await;
}

#[tokio::test]
async fn stored_off_and_forged_reservations_never_authorize_or_refund_another_generation() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let server = Server::new().await;
            let mut fixture = Fixture::new(&server);
            fixture.ledger["policy"]["enabled"]["sfx"] = json!(false);
            fixture.save();
            assert_eq!(fixture.call(sfx(), "off-call", None).await.unwrap_err().code(), "not_enabled");
            assert!(server.requests.lock().unwrap().is_empty());
            fixture.ledger["policy"]["enabled"]["sfx"] = json!(true);
            fixture.save();
            let quote = fixture.call(sfx(), "quote-call", None).await.unwrap();
            let error = fixture
                .call(json!({"action": "create", "quote_id": quote["quote_id"]}), "create-call", Some(("forged", 15)))
                .await
                .unwrap_err();
            assert_ne!(error.code(), "not_submitted", "a forged reservation is not proof about any real unknown job");
            assert_eq!(server.creates(), 0);
        })
        .await;
}

#[tokio::test]
async fn changed_approval_text_or_digest_is_refused_before_the_paid_post() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let server = Server::new().await;
            let mut fixture = Fixture::new(&server);
            let quote = fixture.call(sfx(), "quote-call", None).await.unwrap();
            fixture.reserve(&quote, "r-one", "create-call");
            fixture.ledger["quotes"][quote["quote_id"].as_str().unwrap()]["summary"] = json!("A different job");
            fixture.save();
            let error = fixture
                .call(json!({"action": "create", "quote_id": quote["quote_id"]}), "create-call", Some(("r-one", 15)))
                .await
                .unwrap_err();
            assert_eq!(error.code(), "not_submitted");
            assert_eq!(server.creates(), 0);
        })
        .await;
}

#[tokio::test]
async fn a_lost_submission_stays_unknown_and_never_becomes_a_retry() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let server = Server::new().await;
            server.lose_create.store(true, Ordering::SeqCst);
            let mut fixture = Fixture::new(&server);
            let quote = fixture.call(sfx(), "quote-call", None).await.unwrap();
            fixture.reserve(&quote, "r-one", "create-call");
            let created =
                fixture.call(json!({"action": "create", "quote_id": quote["quote_id"]}), "create-call", Some(("r-one", 15))).await.unwrap();
            let id = created["job_id"].as_str().unwrap();
            let job = fixture.done(id).await;
            assert_eq!(job["state"], "failed");
            assert_eq!(job["result"]["quote_id"], quote["quote_id"]);
            assert_eq!(job["result"]["submission"], "unknown");
            assert_eq!(job["result"]["credits"]["actual"], Value::Null);
            let read = fixture.call(json!({"action": "job", "job_id": id}), "job-call", None).await.unwrap();
            assert_eq!(read, job);
            assert_eq!(server.creates(), 1);
            fixture.assert_private();
        })
        .await;
}

#[tokio::test]
async fn a_charged_download_failure_recovers_by_get_under_the_same_id_even_when_creation_is_off() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let server = Server::new().await;
            server.download_fails.store(true, Ordering::SeqCst);
            let mut fixture = Fixture::new(&server);
            let quote = fixture.call(sfx(), "quote-call", None).await.unwrap();
            fixture.reserve(&quote, "r-one", "create-call");
            let created =
                fixture.call(json!({"action": "create", "quote_id": quote["quote_id"]}), "create-call", Some(("r-one", 15))).await.unwrap();
            let id = created["job_id"].as_str().unwrap();
            let failed = fixture.done(id).await;
            assert_eq!(failed["state"], "failed", "{failed}");
            assert_eq!(failed["result"]["credits"]["actual"], 12);
            fixture.ledger["reservations"]["r-one"]["job_id"] = json!(id);
            fixture.ledger["reservations"]["r-one"]["provider_job_id"] = json!("sfx-job");
            fixture.ledger["reservations"]["r-one"]["submission"] = json!("submitted");
            fixture.ledger["reservations"]["r-one"]["settlement"] = json!("charged");
            fixture.ledger["reservations"]["r-one"]["actual_credits"] = json!(12);
            fixture.ledger["policy"]["enabled"]["sfx"] = json!(false);
            fixture.ledger["policy"]["acknowledged"] = json!(false);
            fixture.save();
            server.download_fails.store(false, Ordering::SeqCst);
            fixture.call_private(json!({"action": "job", "job_id": id}), "recover-call", None, false, false).await.unwrap();
            let recovered = fixture.done(id).await;
            assert_eq!(recovered["state"], "done", "{recovered}");
            assert_eq!(recovered["job_id"], id);
            assert_eq!(recovered["result"]["credits"]["actual"], 12);
            assert_eq!(server.creates(), 1);
            assert!(server.requests.lock().unwrap().iter().any(|request| request.method == "GET" && request.path.contains("sfx-job")));
            fixture.assert_private();
        })
        .await;
}

#[tokio::test]
async fn recovery_keeps_retained_outputs_when_a_later_delivery_fails() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let server = Server::new().await;
            let mut fixture = Fixture::new(&server);
            let quote = fixture.call(sfx(), "quote-call", None).await.unwrap();
            fixture.reserve(&quote, "r-one", "create-call");
            let created =
                fixture.call(json!({"action": "create", "quote_id": quote["quote_id"]}), "create-call", Some(("r-one", 15))).await.unwrap();
            let id = created["job_id"].as_str().unwrap();
            let completed = fixture.done(id).await;
            let retained = completed["result"]["outputs"].clone();
            let artifact = completed["result"]["artifact_id"].as_str().unwrap();
            fixture.ledger["reservations"]["r-one"]["job_id"] = json!(id);
            fixture.ledger["reservations"]["r-one"]["provider_job_id"] = json!("sfx-job");
            fixture.ledger["reservations"]["r-one"]["submission"] = json!("submitted");
            fixture.save();
            // A settled operation can lose its final successful answer after its files were kept.
            fixture
                .ctx
                .resume_job(id, "generate", |_, _| async {
                    Err(refused("interrupted", "The final generation answer was lost after retention."))
                })
                .unwrap();
            assert_eq!(fixture.done(id).await["state"], "failed");
            server.download_fails.store(true, Ordering::SeqCst);
            fixture.call(json!({"action": "job", "job_id": id}), "recover-fails", None).await.unwrap();
            let failed = fixture.done(id).await;
            assert_eq!(failed["state"], "failed");
            assert_eq!(failed["result"]["outputs"], retained);
            assert_eq!(failed["result"]["artifact_id"], artifact);
            assert_eq!(failed["result"]["credits"]["actual"], 12);
            assert_eq!(std::fs::read(fixture.ctx.artifacts().resolve(&fixture.ctx, artifact).unwrap()).unwrap(), wav());
            server.download_fails.store(false, Ordering::SeqCst);
            fixture.call(json!({"action": "job", "job_id": id}), "recover-works", None).await.unwrap();
            assert_eq!(fixture.done(id).await["state"], "done");
            assert_eq!(server.creates(), 1);
        })
        .await;
}

#[tokio::test]
async fn midi_approval_hashes_without_copying_then_uploads_exact_bytes_and_keeps_authoritative_notes() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let server = Server::new().await;
            let mut fixture = Fixture::new(&server);
            let source = fixture.source.join("melody.wav");
            std::fs::write(&source, wav()).unwrap();
            let request =
                json!({"action": "quote", "kind": "audio_to_midi", "source": {"file": source, "provenance": {"host_tool": "fixture"}}});
            let mut preview = request.clone();
            preview["preview"] = json!(true);
            let before = std::fs::read_dir(fixture.ctx.paths().state_dir.join("artifacts")).unwrap().count();
            let approved = fixture.call(preview, "quote-call", None).await.unwrap();
            assert_eq!(approved["upload"]["identity_sha256"], format!("{:x}", Sha256::digest(wav())));
            assert_eq!(approved["upload"]["bytes"], wav().len());
            assert!(server.requests.lock().unwrap().is_empty());
            assert_eq!(std::fs::read_dir(fixture.ctx.paths().state_dir.join("artifacts")).unwrap().count(), before);
            let quote = fixture.call(request, "quote-call", None).await.unwrap();
            assert_eq!(quote["seconds"], 1.0);
            assert_eq!(server.creates(), 0);
            assert_eq!(server.requests.lock().unwrap().iter().find(|request| request.path == "/put").unwrap().body, wav());
            fixture.reserve(&quote, "r-midi", "create-call");
            let created =
                fixture.call(json!({"action": "create", "quote_id": quote["quote_id"]}), "create-call", Some(("r-midi", 3))).await.unwrap();
            let job = fixture.done(created["job_id"].as_str().unwrap()).await;
            assert_eq!(job["state"], "done", "{job}");
            assert_eq!(job["result"]["output_type"], "notes");
            assert_eq!(job["result"]["note_count"], 1);
            assert_eq!(
                job["result"]["notes"][0],
                json!({"pitch": 60, "velocity": 101, "start": 0.0, "end": 0.5, "instrument": "acoustic_piano"})
            );
            let output = fixture.ctx.artifacts().resolve(&fixture.ctx, job["result"]["artifact_id"].as_str().unwrap()).unwrap();
            assert_eq!(serde_json::from_slice::<Value>(&std::fs::read(output).unwrap()).unwrap(), job["result"]["notes"]);
            assert_eq!(std::fs::read(source).unwrap(), wav());
            fixture.assert_private();
        })
        .await;
}

#[tokio::test]
async fn changing_content_at_the_same_size_and_mtime_invalidates_upload_approval_before_any_network() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let server = Server::new().await;
            let fixture = Fixture::new(&server);
            let source = fixture.source.join("melody.wav");
            std::fs::write(&source, wav()).unwrap();
            let request =
                json!({"action": "quote", "kind": "audio_to_midi", "source": {"file": source, "provenance": {"host_tool": "fixture"}}});
            let mut preview = request.clone();
            preview["preview"] = json!(true);
            fixture.call(preview, "quote-call", None).await.unwrap();
            let time = std::fs::metadata(&source).unwrap().modified().unwrap();
            let mut changed = wav();
            *changed.last_mut().unwrap() = 1;
            std::fs::write(&source, changed).unwrap();
            File::options().write(true).open(&source).unwrap().set_times(std::fs::FileTimes::new().set_modified(time)).unwrap();
            let error = fixture.call(request, "quote-call", None).await.unwrap_err();
            assert_eq!(error.code(), "stale");
            assert!(server.requests.lock().unwrap().is_empty());
        })
        .await;
}

#[tokio::test]
async fn pcm_duration_uses_the_entire_source_and_enforces_mirelos_thirty_minute_limit() {
    let server = Server::new().await;
    let fixture = Fixture::new(&server);
    let source = fixture.source.join("long.wav");
    for seconds in [900_u32, 1800, 1801] {
        let data = seconds * 16000;
        let mut header = wav();
        header.truncate(44);
        header[4..8].copy_from_slice(&(data + 36).to_le_bytes());
        header[40..44].copy_from_slice(&data.to_le_bytes());
        let mut file = std::fs::File::create(&source).unwrap();
        file.write_all(&header).unwrap();
        file.set_len(u64::from(data) + 44).unwrap();
        drop(file);
        let signal = Signal::new();
        let staged = fixture.ctx.artifacts().stage(&fixture.ctx, &source, None, &signal).await.unwrap();
        let result = generate_source::seconds(&fixture.ctx, &staged, &signal).await;
        if seconds <= 1800 {
            assert_eq!(result.unwrap(), f64::from(seconds));
        } else {
            assert!(result.is_err(), "a longer upload must not be priced from a truncated duration");
        }
    }
    assert!(server.requests.lock().unwrap().is_empty());
}

#[path = "review_tests.rs"]
mod review;
