use base64::{engine::general_purpose::STANDARD, Engine};
use kumi_ai::{
    audio::{Config, Endpoint, LocalProtocol, Selection, Service, ServiceOptions, Task, TaskConfig},
    auth::store::open_credential_store,
};
use kumi_common::abort::Signal;
use serde_json::{json, Value};
use std::{
    cell::RefCell,
    collections::BTreeMap,
    rc::Rc,
    sync::{Arc, Mutex},
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

#[derive(Clone, Debug)]
struct Request {
    path: String,
    headers: String,
    body: Vec<u8>,
    at: tokio::time::Instant,
}
struct Server {
    url: String,
    requests: Arc<Mutex<Vec<Request>>>,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for Server {
    fn drop(&mut self) {
        self.task.abort();
    }
}
struct Reply {
    status: u16,
    mime: &'static str,
    body: Vec<u8>,
    headers: Vec<(&'static str, String)>,
}
fn answer(value: Value) -> Option<Reply> {
    Some(Reply { status: 200, mime: "application/json", body: serde_json::to_vec(&value).unwrap(), headers: vec![] })
}
async fn server(handler: impl Fn(&Request, &str) -> Option<Reply> + Send + Sync + 'static) -> Server {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let base = url.clone();
    let requests = Arc::new(Mutex::new(Vec::new()));
    let recorded = Arc::clone(&requests);
    let handler = Arc::new(handler);
    let task = tokio::spawn(async move {
        loop {
            let (mut stream, _) = listener.accept().await.unwrap();
            let handler = Arc::clone(&handler);
            let recorded = Arc::clone(&recorded);
            let base = base.clone();
            tokio::spawn(async move {
                let mut bytes = Vec::new();
                let mut chunk = [0; 8192];
                let end = loop {
                    let n = stream.read(&mut chunk).await.unwrap();
                    if n == 0 {
                        return;
                    }
                    bytes.extend_from_slice(&chunk[..n]);
                    if let Some(end) = bytes.windows(4).position(|bytes| bytes == b"\r\n\r\n") {
                        break end + 4;
                    }
                };
                let headers = String::from_utf8_lossy(&bytes[..end]).into_owned();
                let length = headers
                    .lines()
                    .filter_map(|line| line.split_once(':'))
                    .find(|(name, _)| name.eq_ignore_ascii_case("content-length"))
                    .map(|(_, value)| value.trim().parse::<usize>().unwrap())
                    .unwrap_or(0);
                while bytes.len() < end + length {
                    let n = stream.read(&mut chunk).await.unwrap();
                    if n == 0 {
                        return;
                    }
                    bytes.extend_from_slice(&chunk[..n]);
                }
                let request = Request {
                    path: headers.split_whitespace().nth(1).unwrap().to_owned(),
                    headers,
                    body: bytes[end..].to_vec(),
                    at: tokio::time::Instant::now(),
                };
                recorded.lock().unwrap().push(request.clone());
                let Some(reply) = handler(&request, &base) else {
                    return;
                };
                let header = format!(
                    "HTTP/1.1 {} Test\r\nContent-Type: {}\r\nContent-Length: {}\r\n{}Connection: close\r\n\r\n",
                    reply.status,
                    reply.mime,
                    reply.body.len(),
                    reply.headers.iter().map(|(name, value)| format!("{name}: {value}\r\n")).collect::<String>()
                );
                stream.write_all(header.as_bytes()).await.unwrap();
                stream.write_all(&reply.body).await.unwrap();
            });
        }
    });
    Server { url, requests, task }
}
fn config(url: &str, task: Task, protocol: LocalProtocol) -> Config {
    let model = match task {
        Task::AudioToMidi => None,
        Task::Extend | Task::Inpaint => Some("sfx-1.6".into()),
        _ => Some("sfx-2.0".into()),
    };
    Config {
        tasks: BTreeMap::from([(
            task,
            TaskConfig { auto_mirelo: false, local: Some(Endpoint { url: url.into(), protocol, model }), fallback: true },
        )]),
        acknowledged: false,
    }
}
fn service(dir: &tempfile::TempDir, config: Config) -> Rc<Service> {
    service_with_base(dir, config, None)
}
fn service_with_base(dir: &tempfile::TempDir, config: Config, mirelo_base: Option<String>) -> Rc<Service> {
    let config = Rc::new(RefCell::new(config));
    let read = Rc::clone(&config);
    Service::new(ServiceOptions {
        store: Rc::new(open_credential_store(dir.path().join("auth.json"))),
        env: Default::default(),
        read_config: Rc::new(move || read.borrow().clone()),
        write_config: Rc::new(move |value| {
            *config.borrow_mut() = value.clone();
            Ok(())
        }),
        output_dir: dir.path().join("audio"),
        mirelo_base,
    })
}
fn models() -> Value {
    json!({"data":[{"id":"sfx-2.0","max_prompt_chars":200,"formats":["wav"],"operations":{"text-to-sfx":{"duration_ms":{"min":100,"max":30000}},"video-to-sfx":{"duration_ms":{"min":1000,"max":30000}}}},
        {"id":"sfx-1.6","max_prompt_chars":200,"formats":["wav"],"operations":{"extend":{"append_duration_ms":{"min":1000,"max":60000}},"inpaint":{"region_start_ms":{"min":1000},"region_width_ms":{"min":100,"max":30000}}}}]})
}
fn completed(base: &str, partial: bool) -> Value {
    json!({"id":"job-1","model":"sfx-2.0","status":if partial {"partially_succeeded"} else {"succeeded"},"credits":10,"errors":[],"result":{"output":{"index":0,"status":if partial {"partially_succeeded"} else {"succeeded"},"variants":[{"index":0,"status":"succeeded","files":{"audio":{"url":format!("{base}/download"),"format":"wav","sample_rate":48000}}},{"index":1,"status":if partial {"failed"} else {"succeeded"},"files":if partial {Value::Null} else {json!({"audio":{"url":format!("{base}/download"),"format":"wav"}})}}]}}})
}
fn fixture(request: &Request, base: &str) -> Option<Reply> {
    if request.path == "/v3/models" {
        answer(models())
    } else if request.path.ends_with("/preflight") {
        answer(json!({"credits":10,"estimated_ms":20}))
    } else if request.path == "/v3/assets" {
        answer(json!({"id":"asset1","upload_url":format!("{base}/upload"),"max_bytes":100000,"fields":{"policy":"signed-policy"}}))
    } else if request.path == "/upload" {
        Some(Reply { status: 204, mime: "text/plain", body: vec![], headers: vec![] })
    } else if request.path == "/download" {
        Some(Reply { status: 200, mime: "audio/wav", body: b"RIFF0000WAVEfmt ".to_vec(), headers: vec![] })
    } else {
        answer(completed(base, false))
    }
}
#[test]
fn task_configuration_is_independent_and_malformed_disabled_settings_do_not_default_on() {
    let saved: Config = serde_json::from_value(json!({"tasks":{"extend":{"autoMirelo":false,"fallback":false}}})).unwrap();
    assert!(!saved.task(Task::Extend).auto_mirelo);
    assert!(saved.task(Task::Inpaint).auto_mirelo);
    assert_eq!(serde_json::to_value(&saved).unwrap()["tasks"]["extend"]["autoMirelo"], false);
    assert!(serde_json::from_value::<Config>(json!({"tasks":{"extend":{"autoMirelo":false,"local":{"url":42}}}})).is_err());
    assert!(config("http://192.168.1.4:8000", Task::TextToSfx, LocalProtocol::Mirelo).validate().is_err());
    assert!(config("http://127.0.0.1:8000", Task::TextToSfx, LocalProtocol::Muscriptor).validate().is_err());
}
#[tokio::test]
async fn every_sfx_operation_quotes_exact_body_uploads_without_auth_and_saves_outputs() {
    let server = server(fixture).await;
    let dir = tempfile::tempdir().unwrap();
    let audio = dir.path().join("Unreleased_feat_X_v3.wav");
    let video = dir.path().join("input.mp4");
    tokio::fs::write(&audio, b"RIFF0000WAVEfmt ").await.unwrap();
    tokio::fs::write(&video, b"0000ftypisom0000").await.unwrap();
    for (task, input, family, model) in [
        (Task::TextToSfx, json!({"prompt":"rain","duration_ms":1000}), "generations", "sfx-2.0"),
        (Task::VideoToSfx, json!({"video":video,"audio":audio,"duration_ms":1000}), "generations", "sfx-2.0"),
        (Task::Extend, json!({"audio":audio,"append_duration_ms":1000}), "edits", "sfx-1.6"),
        (Task::Inpaint, json!({"audio":audio,"region_start_ms":1000,"region_end_ms":2000}), "edits", "sfx-1.6"),
    ] {
        let service = service(&dir, config(&server.url, task, LocalProtocol::Mirelo));
        service.save_local_key(task, "local-secret").await.unwrap();
        let result = service.execute(task, input, Selection::Auto, &Signal::new(), &|_| {}).await.unwrap();
        assert_eq!(result["provider"], "local");
        assert!(std::path::Path::new(result["files"][0]["path"].as_str().unwrap()).is_file());
        let requests = server.requests.lock().unwrap();
        let prefix = format!("/v3/{}/{family}", task.as_str());
        let quote = requests.iter().rev().find(|r| r.path == format!("{prefix}/preflight")).unwrap();
        let create = requests.iter().rev().find(|r| r.path == format!("{prefix}?wait=25")).unwrap();
        assert_eq!(quote.body, create.body);
        assert_eq!(serde_json::from_slice::<Value>(&create.body).unwrap()["model"], model);
        if task == Task::VideoToSfx {
            let body: Value = serde_json::from_slice(&create.body).unwrap();
            assert_eq!(body["controls"]["preserve_speech"], false);
            assert_eq!(body["input"]["audio"]["type"], "asset");
        }
        assert!(create.headers.to_lowercase().contains("idempotency-key:"));
        assert!(create.headers.contains("Bearer local-secret"));
    }
    let requests = server.requests.lock().unwrap();
    for request in requests.iter().filter(|request| request.path == "/upload" || request.path == "/download") {
        assert!(!request.headers.to_lowercase().contains("authorization:"));
    }
    let upload = requests.iter().find(|request| request.path == "/upload").unwrap();
    let body = String::from_utf8_lossy(&upload.body);
    assert!(body.find("name=\"policy\"").unwrap() < body.find("name=\"file\"").unwrap());
    assert!(body.contains("filename=\"input.wav\""));
    assert!(!body.contains("Unreleased_feat_X_v3"));
}
#[tokio::test]
async fn partial_output_is_saved_but_is_not_reported_as_complete_or_submitted_twice() {
    let server =
        server(|request, base| if request.path.contains("?wait=") { answer(completed(base, true)) } else { fixture(request, base) }).await;
    let dir = tempfile::tempdir().unwrap();
    let service = service(&dir, config(&server.url, Task::TextToSfx, LocalProtocol::Mirelo));
    let error = service
        .execute(Task::TextToSfx, json!({"prompt":"rain","duration_ms":1000}), Selection::Auto, &Signal::new(), &|_| {})
        .await
        .unwrap_err()
        .to_string();
    assert!(error.contains("partially_succeeded") && error.contains("job-1") && error.contains("may have been billed"));
    assert_eq!(server.requests.lock().unwrap().iter().filter(|request| request.path.contains("?wait=")).count(), 1);
    assert!(std::fs::read_dir(dir.path().join("audio")).unwrap().next().is_some());
}
#[tokio::test]
async fn lost_create_is_never_retried() {
    let server = server(|request, base| if request.path.contains("?wait=") { None } else { fixture(request, base) }).await;
    let dir = tempfile::tempdir().unwrap();
    let service = service(&dir, config(&server.url, Task::TextToSfx, LocalProtocol::Mirelo));
    let error = service
        .execute(Task::TextToSfx, json!({"prompt":"rain","duration_ms":1000}), Selection::Auto, &Signal::new(), &|_| {})
        .await
        .unwrap_err()
        .to_string();
    assert!(error.contains("no retry or fallback"));
    use sha2::{Digest, Sha256};
    let requests = server.requests.lock().unwrap();
    let create = requests.iter().find(|request| request.path.contains("?wait=")).unwrap();
    let key = create
        .headers
        .lines()
        .find_map(|line| {
            line.split_once(':').filter(|(name, _)| name.eq_ignore_ascii_case("idempotency-key")).map(|(_, value)| value.trim())
        })
        .unwrap();
    assert!(error.contains(key));
    assert!(error.contains(&hex::encode(Sha256::digest(&create.body))));
    drop(requests);
    assert_eq!(server.requests.lock().unwrap().iter().filter(|request| request.path.contains("?wait=")).count(), 1);
}
#[tokio::test]
async fn foreign_poll_url_is_rejected_and_endpoint_change_does_not_forward_previous_key() {
    let other = server(fixture).await;
    let target = other.url.clone();
    let server = server(move |request, base| {
        if request.path.contains("?wait=") {
            answer(json!({"id":"job1","status":"queued","urls":{"self":format!("{target}/poll")}}))
        } else {
            fixture(request, base)
        }
    })
    .await;
    let dir = tempfile::tempdir().unwrap();
    let service = service(&dir, config(&server.url, Task::TextToSfx, LocalProtocol::Mirelo));
    service.save_local_key(Task::TextToSfx, "origin-A-secret").await.unwrap();
    let error = service
        .execute(Task::TextToSfx, json!({"prompt":"rain","duration_ms":1000}), Selection::Local, &Signal::new(), &|_| {})
        .await
        .unwrap_err()
        .to_string();
    assert!(error.contains("configured API origin"));
    assert!(other.requests.lock().unwrap().is_empty());
    service.configure(config(&other.url, Task::TextToSfx, LocalProtocol::Mirelo)).await.unwrap();
    service
        .execute(Task::TextToSfx, json!({"prompt":"rain","duration_ms":1000,"quote":true}), Selection::Local, &Signal::new(), &|_| {})
        .await
        .unwrap();
    assert!(other.requests.lock().unwrap().iter().all(|request| !request.headers.to_lowercase().contains("authorization:")));
}
#[tokio::test]
async fn midi_uses_v2_asset_put_async_job_and_authoritative_notes() {
    let server = server(|request, base| {
        if request.path.contains("/preflight?") { answer(json!({"credits":3,"billing_mode":"unmetered"})) }
        else if request.path == "/v2/assets" { answer(json!({"asset_id":"midi-asset","upload_url":format!("{base}/put")})) }
        else if request.path == "/put" { Some(Reply { status:200,mime:"text/plain",body:vec![],headers:vec![] }) }
        else if request.path.ends_with("/jobs") { answer(json!({"job_id":"midi1","job_url":"/v2/audio-to-midi/v1.0/jobs/midi1"})) }
        else if request.path.ends_with("/midi1") { answer(json!({"job_id":"midi1","status":"succeeded","result":{"model":"audio-to-midi-1.0","midi_url":format!("{base}/midi"),"notes":[{"pitch":60.0,"start":0.0,"end":0.5,"velocity":100.5,"instrument":"acoustic_piano"}]}})) }
        else { Some(Reply { status:200,mime:"audio/midi",body:b"MThd0000".to_vec(),headers:vec![] }) }
    }).await;
    let dir = tempfile::tempdir().unwrap();
    let audio = dir.path().join("input.wav");
    tokio::fs::write(&audio, b"RIFF0000WAVEfmt ").await.unwrap();
    let service = service(&dir, config(&server.url, Task::AudioToMidi, LocalProtocol::Mirelo));
    service.save_local_key(Task::AudioToMidi, "midi-secret").await.unwrap();
    let result = service
        .execute(Task::AudioToMidi, json!({"audio":audio,"duration_ms":1000}), Selection::Auto, &Signal::new(), &|_| {})
        .await
        .unwrap();
    assert_eq!(result["notes"][0]["end"], 0.5);
    assert_eq!(result["notes"][0]["pitch"], 60);
    assert_eq!(result["notes"][0]["velocity"], 101);
    let saved: Value = serde_json::from_slice(&tokio::fs::read(result["notes_path"].as_str().unwrap()).await.unwrap()).unwrap();
    assert_eq!(saved, result["notes"]);
    assert_eq!(result["estimated_credits"], 3);
    assert!(std::path::Path::new(result["notes_path"].as_str().unwrap()).is_file());
    let requests = server.requests.lock().unwrap();
    let put = requests.iter().find(|request| request.path == "/put").unwrap();
    assert!(put.headers.starts_with("PUT ") && !put.headers.to_lowercase().contains("authorization:"));
    let create = requests.iter().find(|request| request.path.ends_with("/jobs")).unwrap();
    assert!(!create.headers.to_lowercase().contains("idempotency-key:"));
    assert_eq!(serde_json::from_slice::<Value>(&create.body).unwrap()["audio"]["asset_id"], "midi-asset");
}
#[tokio::test]
async fn muscriptor_native_stream_retains_midi_corrects_onset_and_rejects_truncation() {
    for complete in [true, false] {
        let server = server(move |request, _| {
            assert_eq!(request.path, "/transcribe");
            let mut events = vec![
                json!({"type":"start","index":1,"pitch":64,"start_time":0.5,"instrument":"acoustic_piano"}),
                json!({"type":"end","start_event_index":1,"end_time":1.0}),
            ];
            if complete {
                events.push(json!({"type":"transcription_complete","data":STANDARD.encode(b"MThd0000"),"beat_grid":{"onset_delay":0.1}}));
            }
            Some(Reply {
                status: 200,
                mime: "text/event-stream",
                body: events.into_iter().map(|event| format!("data: {event}\n\n")).collect::<String>().into_bytes(),
                headers: vec![],
            })
        })
        .await;
        let dir = tempfile::tempdir().unwrap();
        let audio = dir.path().join("input.wav");
        tokio::fs::write(&audio, b"RIFF0000WAVEfmt ").await.unwrap();
        let service = service(&dir, config(&server.url, Task::AudioToMidi, LocalProtocol::Muscriptor));
        let result = service.execute(Task::AudioToMidi, json!({"audio":audio}), Selection::Local, &Signal::new(), &|_| {}).await;
        if complete {
            let result = result.unwrap();
            assert_eq!(result["notes"][0]["start"], 0.4);
            assert_eq!(result["notes"][0]["velocity"], 100);
            assert!(std::path::Path::new(result["midi_path"].as_str().unwrap()).is_file());
        } else {
            assert!(result.unwrap_err().to_string().contains("without transcription_complete"));
        }
    }
}
#[tokio::test]
async fn quote_and_pre_cancel_never_create_and_credit_recovery_blocks_even_without_shortfall() {
    let server = server(|request, base| {
        if request.path.ends_with("/preflight") {
            answer(json!({"credits":10,"credit_recovery":{"provisioning_state":"pending","credit_shortfall":0,"recovery_action":null}}))
        } else {
            fixture(request, base)
        }
    })
    .await;
    let dir = tempfile::tempdir().unwrap();
    let service = service(&dir, config(&server.url, Task::TextToSfx, LocalProtocol::Mirelo));
    let input = json!({"prompt":"rain","duration_ms":1000});
    assert!(service
        .execute(Task::TextToSfx, input.clone(), Selection::Local, &Signal::new(), &|_| {})
        .await
        .unwrap_err()
        .to_string()
        .contains("cannot fund"));
    let mut quote = input.clone();
    quote["quote"] = json!(true);
    assert_eq!(service.execute(Task::TextToSfx, quote, Selection::Local, &Signal::new(), &|_| {}).await.unwrap()["submitted"], false);
    let signal = Signal::new();
    signal.cancel();
    assert!(service
        .execute(Task::TextToSfx, input, Selection::Local, &signal, &|_| {})
        .await
        .unwrap_err()
        .to_string()
        .contains("cancelled"));
    assert!(server.requests.lock().unwrap().iter().all(|request| !request.path.contains("?wait=")));
}

#[tokio::test]
async fn terminal_failure_states_never_download_or_resubmit() {
    for status in ["failed", "canceled", "expired"] {
        let server = server(move |request, base| {
            if request.path.contains("?wait=") {
                answer(json!({"id":"failed-job","status":status,"credits":10,"errors":[{"code":"result_unreadable","retryable":false}]}))
            } else {
                fixture(request, base)
            }
        })
        .await;
        let dir = tempfile::tempdir().unwrap();
        let service = service(&dir, config(&server.url, Task::TextToSfx, LocalProtocol::Mirelo));
        let error = service
            .execute(Task::TextToSfx, json!({"prompt":"rain","duration_ms":1000}), Selection::Local, &Signal::new(), &|_| {})
            .await
            .unwrap_err()
            .to_string();
        assert!(error.contains(status) && error.contains("failed-job") && error.contains("result_unreadable"));
        let requests = server.requests.lock().unwrap();
        assert_eq!(requests.iter().filter(|request| request.path.contains("?wait=")).count(), 1);
        assert!(!requests.iter().any(|request| request.path == "/download"));
    }
}

#[tokio::test]
async fn preference_is_independent_and_removing_saved_mirelo_keeps_environment_fallback() {
    use kumi_ai::auth::store::{Credential, CredentialStore};
    let dir = tempfile::tempdir().unwrap();
    let store = Rc::new(open_credential_store(dir.path().join("auth.json")));
    store.update("mirelo", Box::new(|_| Box::pin(async { Ok(Some(Credential::ApiKey { key: "saved-secret".into() })) }))).await.unwrap();
    let config = config("http://127.0.0.1:8000", Task::AudioToMidi, LocalProtocol::Muscriptor);
    let service = Service::new(ServiceOptions {
        store: store.clone(),
        env: std::collections::HashMap::from([("MIRELO_API_KEY".into(), "env-secret".into())]),
        read_config: Rc::new(move || config.clone()),
        write_config: Rc::new(|_| Ok(())),
        output_dir: dir.path().join("audio"),
        mirelo_base: None,
    });
    assert_eq!(service.preferred(Task::AudioToMidi).await.unwrap(), Some(Selection::Local));
    assert_eq!(service.preferred(Task::TextToSfx).await.unwrap(), None);
    assert_eq!(service.status().await.unwrap().key_source.as_deref(), Some("saved"));
    service.remove_mirelo_key().await.unwrap();
    assert!(store.get("mirelo").await.unwrap().is_none());
    let status = service.status().await.unwrap();
    assert!(status.mirelo_configured);
    assert_eq!(status.key_source.as_deref(), Some("MIRELO_API_KEY"));
    let displayed = serde_json::to_string(&status).unwrap();
    assert!(!displayed.contains("env-secret") && !displayed.contains("saved-secret"));
}

#[tokio::test]
async fn local_key_removal_and_invalid_media_do_not_send_secrets_or_requests() {
    let server = server(fixture).await;
    let dir = tempfile::tempdir().unwrap();
    let service = service(&dir, config(&server.url, Task::TextToSfx, LocalProtocol::Mirelo));
    service.save_local_key(Task::TextToSfx, "temporary-secret").await.unwrap();
    service.save_local_key(Task::TextToSfx, "").await.unwrap();
    service
        .execute(Task::TextToSfx, json!({"prompt":"rain","duration_ms":1000,"quote":true}), Selection::Local, &Signal::new(), &|_| {})
        .await
        .unwrap();
    assert!(server.requests.lock().unwrap().iter().all(|request| !request.headers.to_lowercase().contains("authorization:")));
    service.configure(config(&server.url, Task::Extend, LocalProtocol::Mirelo)).await.unwrap();
    let before = server.requests.lock().unwrap().len();
    assert!(service
        .execute(Task::Extend, json!({"audio":dir.path(),"append_duration_ms":1000}), Selection::Local, &Signal::new(), &|_| {})
        .await
        .is_err());
    assert_eq!(server.requests.lock().unwrap().len(), before);
}

#[tokio::test]
async fn unsafe_paths_are_rejected_before_any_protocol_or_filesystem_access() {
    let server = server(fixture).await;
    let dir = tempfile::tempdir().unwrap();
    let service = service(&dir, config(&server.url, Task::Extend, LocalProtocol::Mirelo));
    for path in [r"\\host\share\a.wav", "//host/share/a.wav", r"\\?\C:\a.wav", "/tmp/a\n.wav", "/tmp/a\u{1b}.wav"] {
        let error = service
            .execute(Task::Extend, json!({"audio":path,"append_duration_ms":1000}), Selection::Local, &Signal::new(), &|_| {})
            .await
            .unwrap_err()
            .to_string();
        assert!(error.contains("network, device or control"), "{error}");
    }
    #[cfg(windows)]
    for path in [r"C:\NUL.wav", r"C:\con", r"C:\COM1.wav"] {
        assert!(service
            .execute(Task::Extend, json!({"audio":path,"append_duration_ms":1000}), Selection::Local, &Signal::new(), &|_| {})
            .await
            .unwrap_err()
            .to_string()
            .contains("network, device or control"));
    }
    assert!(server.requests.lock().unwrap().is_empty());
}

#[tokio::test]
async fn download_redirects_are_bounded_credential_free_and_cannot_escape_loopback_origin() {
    for mode in ["relative", "foreign", "cycle"] {
        let other = server(fixture).await;
        let foreign = other.url.clone();
        let server = server(move |request, base| {
            if request.path == "/download" {
                Some(Reply {
                    status: 302,
                    mime: "text/plain",
                    body: vec![],
                    headers: vec![(
                        "Location",
                        match mode {
                            "relative" => "/final".into(),
                            "foreign" => format!("{foreign}/stolen"),
                            _ => "/download".into(),
                        },
                    )],
                })
            } else if request.path == "/final" {
                Some(Reply { status: 200, mime: "audio/wav", body: b"RIFF0000WAVEfmt ".to_vec(), headers: vec![] })
            } else {
                fixture(request, base)
            }
        })
        .await;
        let dir = tempfile::tempdir().unwrap();
        let service = service(&dir, config(&server.url, Task::TextToSfx, LocalProtocol::Mirelo));
        service.save_local_key(Task::TextToSfx, "redirect-secret").await.unwrap();
        let result = service.execute(Task::TextToSfx, json!({"job_id":"job-1"}), Selection::Local, &Signal::new(), &|_| {}).await;
        if mode == "relative" {
            assert_eq!(result.unwrap()["files"].as_array().unwrap().len(), 2);
        } else {
            let error = result.unwrap_err().to_string();
            assert!(error.contains(if mode == "cycle" { "five redirects" } else { "configured loopback" }), "{error}");
        }
        assert!(other.requests.lock().unwrap().is_empty());
        let requests = server.requests.lock().unwrap();
        assert!(requests.iter().all(|request| request.headers.starts_with("GET ")));
        assert!(requests
            .iter()
            .filter(|request| matches!(request.path.as_str(), "/download" | "/final"))
            .all(|request| !request.headers.to_ascii_lowercase().contains("authorization:")));
        assert!(requests.iter().filter(|request| request.path == "/download").count() <= 12);
    }
}

#[tokio::test]
async fn recovery_is_get_only_and_retries_only_transient_polls() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    for status in [0, 429, 503] {
        let polls = Arc::new(AtomicUsize::new(0));
        let count = polls.clone();
        let server = server(move |request, base| {
            if request.path == "/v3/text-to-sfx/generations/job-1" && count.fetch_add(1, Ordering::SeqCst) == 0 {
                if status == 0 {
                    return None;
                }
                let mut reply = answer(json!({"error":{"code":"temporarily_unavailable","retryable":true}})).unwrap();
                reply.status = status;
                reply.headers.push(("Retry-After", if status == 429 { "1" } else { "0" }.into()));
                return Some(reply);
            }
            fixture(request, base)
        })
        .await;
        let dir = tempfile::tempdir().unwrap();
        let service = service(&dir, config(&server.url, Task::TextToSfx, LocalProtocol::Mirelo));
        let result = service.execute(Task::TextToSfx, json!({"job_id":"job-1"}), Selection::Local, &Signal::new(), &|_| {}).await.unwrap();
        assert_eq!(result["job_id"], "job-1");
        assert_eq!(polls.load(Ordering::SeqCst), 2);
        if status == 429 {
            let requests = server.requests.lock().unwrap();
            assert!(requests[1].at - requests[0].at >= std::time::Duration::from_secs(1), "Retry-After must be honored");
        }
        assert!(server.requests.lock().unwrap().iter().all(|request| request.headers.starts_with("GET ")));
        let before = server.requests.lock().unwrap().len();
        for input in [json!({"job_id":"job-1","prompt":"rain"}), json!({"job_id":"../job"}), json!({"job_id":""})] {
            assert!(service.execute(Task::TextToSfx, input, Selection::Local, &Signal::new(), &|_| {}).await.is_err());
        }
        assert_eq!(server.requests.lock().unwrap().len(), before);
    }
}

#[tokio::test]
async fn retry_after_wait_is_cancellable_and_definite_poll_refusal_is_not_retried() {
    for status in [429, 422] {
        let signal = Signal::new();
        let cancel = signal.clone();
        let seen = Arc::new(tokio::sync::Notify::new());
        let notify = seen.clone();
        let server = server(move |_, _| {
            notify.notify_one();
            let mut reply = answer(json!({"error":{"code":"blocked","retryable":status == 429}})).unwrap();
            reply.status = status;
            reply.headers.push(("Retry-After", "3600".into()));
            Some(reply)
        })
        .await;
        let dir = tempfile::tempdir().unwrap();
        let service = service(&dir, config(&server.url, Task::TextToSfx, LocalProtocol::Mirelo));
        let execution = service.execute(Task::TextToSfx, json!({"job_id":"job-1"}), Selection::Local, &signal, &|_| {});
        let cancellation = async {
            seen.notified().await;
            if status == 429 {
                // Give the response reader time to enter its Retry-After wait.
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                cancel.cancel();
            }
        };
        let (result, ()) = tokio::join!(execution, cancellation);
        let error = result.unwrap_err().to_string();
        assert!(error.contains(if status == 429 { "cancelled" } else { "HTTP 422" }), "{error}");
        assert_eq!(server.requests.lock().unwrap().len(), 1);
    }
}

#[tokio::test]
async fn cloud_preflight_can_fallback_but_accepted_lost_and_refused_creates_cannot() {
    for mode in ["preflight", "accepted", "lost", "refused"] {
        let local = server(fixture).await;
        let cloud = server(move |request, base| {
            if request.path == "/v3/me" {
                answer(json!({"credits_available":12,"billing_mode":"metered","provisioning_state":"ready","spend_capacity":99,
                    "key":"cloud-secret","email":"private@example.test","untrusted":"cloud-secret"}))
            } else if mode == "preflight" && request.path.ends_with("/preflight") {
                let mut reply = answer(json!({"error":{"code":"temporarily_unavailable","retryable":true}})).unwrap();
                reply.status = 503;
                Some(reply)
            } else if request.path.contains("?wait=") {
                match mode {
                    "lost" => None,
                    "accepted" => answer(json!({"id":"job-1","status":"failed","credits":10})),
                    "refused" => {
                        let mut reply =
                            answer(json!({"error":{"code":"insufficient_credits","message":"cloud-secret cannot fund","param":"audio",
                            "retryable":false,"credit_recovery":{"credit_shortfall":10,"recovery_action":"view_plans"}}}))
                            .unwrap();
                        reply.status = 402;
                        Some(reply)
                    }
                    _ => fixture(request, base),
                }
            } else {
                fixture(request, base)
            }
        })
        .await;
        let dir = tempfile::tempdir().unwrap();
        let mut configuration = config(&local.url, Task::TextToSfx, LocalProtocol::Mirelo);
        configuration.tasks.get_mut(&Task::TextToSfx).unwrap().auto_mirelo = true;
        let service = service_with_base(&dir, configuration, Some(cloud.url.clone()));
        let account = service.save_mirelo_key("cloud-secret", &Signal::new()).await.unwrap();
        assert_eq!(
            account,
            json!({"configured":true,"credits_available":12,"spend_capacity":99,"billing_mode":"metered","provisioning_state":"ready"})
        );
        let notices = RefCell::new(Vec::new());
        let result = service
            .execute(Task::TextToSfx, json!({"prompt":"rain","duration_ms":1000}), Selection::Auto, &Signal::new(), &|message| {
                notices.borrow_mut().push(message)
            })
            .await;
        assert_eq!(notices.borrow().len(), usize::from(mode != "preflight"));
        if mode == "lost" {
            assert!(notices.borrow()[0].contains("credits unknown"));
        }
        if mode == "accepted" {
            assert!(notices.borrow()[0].contains("credits 10"));
        }
        assert!(notices.borrow().iter().all(|notice| !notice.contains("cloud-secret")));
        if mode == "preflight" {
            let result = result.unwrap();
            assert_eq!(result["provider"], "local");
            assert_eq!(result["fallback"]["from"], "mirelo");
            assert_eq!(local.requests.lock().unwrap().iter().filter(|request| request.path.contains("?wait=")).count(), 1);
        } else {
            let error = result.unwrap_err().to_string();
            assert!(local.requests.lock().unwrap().is_empty());
            assert!(!error.contains("cloud-secret"));
            if mode == "refused" {
                assert!(
                    error.contains("not submitted")
                        && error.contains("insufficient_credits")
                        && error.contains("view_plans")
                        && error.contains("retryable")
                );
                assert!(!error.contains("may have been billed"));
            } else {
                assert!(error.contains("may have been billed"));
            }
            assert_eq!(cloud.requests.lock().unwrap().iter().filter(|request| request.path.contains("?wait=")).count(), 1);
        }
    }
}

#[tokio::test]
async fn account_validation_does_not_return_arbitrary_allowlisted_field_values() {
    let server = server(|_, _| {
        answer(json!({"credits_available":"account-secret","spend_capacity":{"secret":"account-secret"},
        "billing_mode":"account-secret","provisioning_state":["account-secret"]}))
    })
    .await;
    let dir = tempfile::tempdir().unwrap();
    let service = service_with_base(&dir, Config::default(), Some(server.url.clone()));
    assert_eq!(service.save_mirelo_key("account-secret", &Signal::new()).await.unwrap(), json!({"configured":true}));
    let requests = server.requests.lock().unwrap();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].path, "/v3/me");
    assert!(requests[0].headers.starts_with("GET ") && requests[0].headers.contains("Bearer account-secret"));
}

#[tokio::test]
async fn local_credentials_survive_same_origin_changes_but_are_retired_on_move_or_removal() {
    use kumi_ai::auth::store::CredentialStore;
    let first = server(fixture).await;
    let second = server(fixture).await;
    let dir = tempfile::tempdir().unwrap();
    let service = service(&dir, config(&first.url, Task::TextToSfx, LocalProtocol::Mirelo));
    service.save_local_key(Task::TextToSfx, "first-secret").await.unwrap();
    service.configure(config(&format!("{}/nested", first.url), Task::TextToSfx, LocalProtocol::Mirelo)).await.unwrap();
    service.configure(config(&first.url, Task::TextToSfx, LocalProtocol::Mirelo)).await.unwrap();
    service
        .execute(Task::TextToSfx, json!({"prompt":"rain","duration_ms":1000,"quote":true}), Selection::Local, &Signal::new(), &|_| {})
        .await
        .unwrap();
    assert!(first.requests.lock().unwrap().iter().all(|request| request.headers.contains("Bearer first-secret")));
    service.configure(config(&second.url, Task::TextToSfx, LocalProtocol::Mirelo)).await.unwrap();
    let store = open_credential_store(dir.path().join("auth.json"));
    assert!(!std::fs::read_to_string(dir.path().join("auth.json")).unwrap().contains("first-secret"));
    service.save_local_key(Task::TextToSfx, "second-secret").await.unwrap();
    service.configure(Config::default()).await.unwrap();
    assert!(!std::fs::read_to_string(dir.path().join("auth.json")).unwrap().contains("second-secret"));
    assert!(store.get("mirelo").await.unwrap().is_none());
}

#[tokio::test]
async fn unsupported_midi_type_and_oversized_output_sets_stop_before_requests_or_downloads() {
    let server = server(|request, base| {
        if request.path.ends_with("/many") {
            let mut job = completed(base, false);
            let variant = job["result"]["output"]["variants"][0].clone();
            job["result"]["output"]["variants"] = json!(vec![variant; 33]);
            answer(job)
        } else {
            fixture(request, base)
        }
    })
    .await;
    let dir = tempfile::tempdir().unwrap();
    let audio = dir.path().join("input.m4a");
    tokio::fs::write(&audio, b"0000ftypisom0000").await.unwrap();
    let service = service(&dir, config(&server.url, Task::AudioToMidi, LocalProtocol::Mirelo));
    assert!(service
        .execute(Task::AudioToMidi, json!({"audio":audio,"duration_ms":1000}), Selection::Local, &Signal::new(), &|_| {})
        .await
        .unwrap_err()
        .to_string()
        .contains("supports WAV"));
    assert!(server.requests.lock().unwrap().is_empty());
    service.configure(config(&server.url, Task::TextToSfx, LocalProtocol::Mirelo)).await.unwrap();
    let error = service
        .execute(Task::TextToSfx, json!({"job_id":"many"}), Selection::Local, &Signal::new(), &|_| {})
        .await
        .unwrap_err()
        .to_string();
    assert!(error.contains("32 downloadable") && error.contains("no files downloaded"));
    assert_eq!(server.requests.lock().unwrap().len(), 1);
    assert!(!dir.path().join("audio").exists());
}

#[tokio::test]
async fn inpaint_start_limit_is_optional_but_enforced_when_advertised() {
    for advertised in [false, true] {
        let server = server(move |request, base| {
            if request.path == "/v3/models" {
                let mut value = models();
                if !advertised {
                    value["data"][1]["operations"]["inpaint"].as_object_mut().unwrap().remove("region_start_ms");
                }
                answer(value)
            } else {
                fixture(request, base)
            }
        })
        .await;
        let dir = tempfile::tempdir().unwrap();
        let service = service(&dir, config(&server.url, Task::Inpaint, LocalProtocol::Mirelo));
        let result = service
            .execute(
                Task::Inpaint,
                json!({"audio":"https://example.com/input.wav","region_start_ms":0,"region_end_ms":1000,"quote":true}),
                Selection::Local,
                &Signal::new(),
                &|_| {},
            )
            .await;
        assert_eq!(result.is_ok(), !advertised);
        assert_eq!(
            server.requests.lock().unwrap().iter().filter(|request| request.path.ends_with("/preflight")).count(),
            usize::from(!advertised)
        );
    }
}

#[tokio::test]
async fn environment_key_requires_onboarding_off_blocks_explicit_and_logout_revokes_acknowledgement() {
    use kumi_ai::auth::store::CredentialStore;
    let server =
        server(|request, base| if request.path == "/v3/me" { answer(json!({"credits_available":100})) } else { fixture(request, base) })
            .await;
    let dir = tempfile::tempdir().unwrap();
    let initial = config(&server.url, Task::AudioToMidi, LocalProtocol::Muscriptor);
    let state = Rc::new(RefCell::new(initial.clone()));
    let read = state.clone();
    let write = state.clone();
    let store = Rc::new(open_credential_store(dir.path().join("auth.json")));
    let service = Service::new(ServiceOptions {
        store: store.clone(),
        env: std::collections::HashMap::from([("MIRELO_API_KEY".into(), "env-only-secret".into())]),
        read_config: Rc::new(move || read.borrow().clone()),
        write_config: Rc::new(move |config| {
            *write.borrow_mut() = config.clone();
            Ok(())
        }),
        output_dir: dir.path().join("audio"),
        mirelo_base: Some(server.url.clone()),
    });
    assert_eq!(service.preferred(Task::TextToSfx).await.unwrap(), None);
    assert_eq!(service.preferred(Task::AudioToMidi).await.unwrap(), Some(Selection::Local));
    for selection in [Selection::Auto, Selection::Mirelo] {
        assert!(service
            .execute(Task::TextToSfx, json!({"prompt":"rain","duration_ms":1000}), selection, &Signal::new(), &|_| panic!(
                "unsubmitted job notice"
            ))
            .await
            .is_err());
    }
    assert!(server.requests.lock().unwrap().is_empty());
    service.acknowledge_mirelo(&Signal::new()).await.unwrap();
    assert!(state.borrow().acknowledged);
    assert_eq!(state.borrow().tasks, initial.tasks);
    assert!(store.get("mirelo").await.unwrap().is_none(), "environment onboarding must not copy the key into auth");
    assert_eq!(service.preferred(Task::TextToSfx).await.unwrap(), Some(Selection::Mirelo));
    assert_eq!(service.preferred(Task::AudioToMidi).await.unwrap(), Some(Selection::Local));
    let before = server.requests.lock().unwrap().len();
    assert!(service
        .execute(Task::AudioToMidi, json!({"job_id":"existing"}), Selection::Mirelo, &Signal::new(), &|_| {})
        .await
        .unwrap_err()
        .to_string()
        .contains("disabled"));
    assert_eq!(server.requests.lock().unwrap().len(), before);
    let notices = RefCell::new(Vec::new());
    let notice = |message| notices.borrow_mut().push(message);
    service
        .execute(Task::TextToSfx, json!({"prompt":"rain","duration_ms":1000,"quote":true}), Selection::Auto, &Signal::new(), &notice)
        .await
        .unwrap();
    assert!(notices.borrow().is_empty(), "quote never produces a charge notice");
    service.execute(Task::TextToSfx, json!({"prompt":"rain","duration_ms":1000}), Selection::Auto, &Signal::new(), &notice).await.unwrap();
    assert_eq!(*notices.borrow(), vec!["Mirelo job job-1: credits 10."]);
    service.remove_mirelo_key().await.unwrap();
    assert!(!state.borrow().acknowledged);
    assert_eq!(service.preferred(Task::TextToSfx).await.unwrap(), None);
    assert!(service.status().await.unwrap().mirelo_configured, "key presence is not consent");
    let before = server.requests.lock().unwrap().len();
    assert!(service.execute(Task::TextToSfx, json!({"job_id":"job-1"}), Selection::Mirelo, &Signal::new(), &notice).await.is_err());
    assert_eq!(server.requests.lock().unwrap().len(), before);
}

#[tokio::test]
async fn failed_account_validation_never_acknowledges_or_saves_key() {
    let server = server(|_, _| {
        let mut reply = answer(json!({"detail":"invalid supplied-secret"})).unwrap();
        reply.status = 401;
        Some(reply)
    })
    .await;
    let dir = tempfile::tempdir().unwrap();
    let service = service_with_base(&dir, Config::default(), Some(server.url.clone()));
    let error = service.save_mirelo_key("supplied-secret", &Signal::new()).await.unwrap_err().to_string();
    assert!(error.contains("invalid") && !error.contains("supplied-secret"));
    assert!(!service.config().acknowledged);
    assert!(!service.status().await.unwrap().mirelo_configured);
}

#[tokio::test]
async fn every_operation_recovers_using_its_documented_get_path_without_preflight_or_upload() {
    let server = server(|request, base| {
        if request.path == "/v2/audio-to-midi/v1.0/jobs/existing" {
            answer(json!({"job_id":"existing","status":"succeeded","result":{"midi_url":format!("{base}/midi"),
                "notes":[{"pitch":60.0,"velocity":99.5,"start":0.0,"end":0.1,"instrument":"acoustic_piano"}]}}))
        } else if request.path == "/midi" {
            Some(Reply { status: 200, mime: "audio/midi", body: b"MThd0000".to_vec(), headers: vec![] })
        } else {
            fixture(request, base)
        }
    })
    .await;
    let dir = tempfile::tempdir().unwrap();
    for task in Task::ALL {
        let service = service(&dir, config(&server.url, task, LocalProtocol::Mirelo));
        service.execute(task, json!({"job_id":"existing"}), Selection::Local, &Signal::new(), &|_| {}).await.unwrap();
    }
    let requests = server.requests.lock().unwrap();
    assert!(requests.iter().all(|request| request.headers.starts_with("GET ")));
    for path in [
        "/v3/text-to-sfx/generations/existing",
        "/v3/video-to-sfx/generations/existing",
        "/v3/extend/edits/existing",
        "/v3/inpaint/edits/existing",
        "/v2/audio-to-midi/v1.0/jobs/existing",
    ] {
        assert_eq!(requests.iter().filter(|request| request.path == path).count(), 1);
    }
    assert!(!requests
        .iter()
        .any(|request| request.path.contains("preflight") || request.path.contains("assets") || request.path.contains("models")));
}

#[tokio::test]
async fn http_error_details_are_bounded_redacted_and_do_not_retry_post_requests() {
    for status in [400, 402, 409, 422, 429, 503] {
        let server = server(move |request, base| {
            if request.path.contains("?wait=") {
                let mut reply = answer(json!({"error":{"code":"refused","message":format!("{} error-secret", "x".repeat(3000)),
                    "detail":{"reason":"error-secret"},"retryable":true,"credit_recovery":{"credit_shortfall":1}}}))
                .unwrap();
                reply.status = status;
                reply.headers.push(("Retry-After", "0".into()));
                Some(reply)
            } else {
                fixture(request, base)
            }
        })
        .await;
        let dir = tempfile::tempdir().unwrap();
        let service = service(&dir, config(&server.url, Task::TextToSfx, LocalProtocol::Mirelo));
        service.save_local_key(Task::TextToSfx, "error-secret").await.unwrap();
        let error = service
            .execute(Task::TextToSfx, json!({"prompt":"rain","duration_ms":1000}), Selection::Local, &Signal::new(), &|_| {})
            .await
            .unwrap_err()
            .to_string();
        assert!(!error.contains("error-secret"));
        assert!(error.len() < 2048, "{error}");
        assert_eq!(error.contains("not submitted"), status < 500);
        assert_eq!(error.contains("may have been billed"), status >= 500);
        assert_eq!(server.requests.lock().unwrap().iter().filter(|request| request.path.contains("?wait=")).count(), 1);
    }
}

#[tokio::test]
async fn phased_mirelo_prepares_without_create_and_submits_exact_body_once() {
    use kumi_ai::audio::mirelo::{Client, OutputKind};
    use sha2::{Digest, Sha256};
    let server = server(fixture).await;
    let client = Client::with_base(&server.url, "phased-secret").unwrap();
    let signal = Signal::new();
    let prepared = client.prepare(Task::TextToSfx, &json!({"prompt":"rain","duration_ms":1000}), &signal).await.unwrap();
    assert_eq!(prepared.credits(), 10);
    let digest = prepared.request_sha256().to_owned();
    {
        let requests = server.requests.lock().unwrap();
        assert_eq!(requests.len(), 2);
        assert!(requests[0].headers.starts_with("GET /v3/models "));
        assert!(requests[1].path.ends_with("/preflight"));
        assert_eq!(digest, hex::encode(Sha256::digest(&requests[1].body)));
    }
    let job = client.submit(prepared, &signal).await.unwrap();
    assert_eq!(job.id(), Some("job-1"));
    assert_eq!(job.actual_credits(), Some(10));
    assert!(job.succeeded());
    let job = client.wait(job, &signal).await.unwrap();
    assert_eq!(server.requests.lock().unwrap().len(), 3, "terminal wait does not fetch or download");
    let dir = tempfile::tempdir().unwrap();
    let delivery = client.deliver(job, dir.path(), 1024, &signal).await.unwrap();
    assert_eq!(delivery.outputs.len(), 2);
    assert!(delivery.notes.is_none());
    assert!(delivery.outputs.iter().all(|file| file.kind == OutputKind::Audio && file.mime == "audio/wav" && file.path.is_file()));
    let requests = server.requests.lock().unwrap();
    let creates: Vec<_> = requests.iter().filter(|request| request.path.ends_with("?wait=25")).collect();
    assert_eq!(creates.len(), 1);
    assert_eq!(requests[1].body, creates[0].body);
    assert!(creates[0].headers.to_lowercase().contains("idempotency-key:"));
    assert!(requests
        .iter()
        .filter(|request| request.path == "/download")
        .all(|request| !request.headers.to_lowercase().contains("authorization:")));
}

#[tokio::test]
async fn phased_mirelo_lost_or_refused_submission_is_unknown_and_never_replayed() {
    use kumi_ai::audio::mirelo::Client;
    for status in [0, 400, 429, 503, 200] {
        let server = server(move |request, base| {
            if request.path.ends_with("?wait=25") {
                return match status {
                    0 => None,
                    200 => Some(Reply { status, mime: "application/json", body: b"not-json".to_vec(), headers: vec![] }),
                    _ => {
                        let mut reply = answer(json!({"error":{"message":"key-sentinel /private/source.wav https://signed.example/result?secret=1","retryable":true}})).unwrap();
                        reply.status = status;
                        Some(reply)
                    }
                };
            }
            fixture(request, base)
        }).await;
        let client = Client::with_base(&server.url, "key-sentinel").unwrap();
        let signal = Signal::new();
        let prepared = client.prepare(Task::TextToSfx, &json!({"prompt":"rain","duration_ms":1000}), &signal).await.unwrap();
        let error = client.submit(prepared, &signal).await.err().unwrap();
        assert!(!error.not_submitted(), "{status}");
        let diagnostic = format!("{error} {error:?}");
        for secret in ["key-sentinel", "/private/", "https://", "signed.example"] {
            assert!(!diagnostic.contains(secret));
        }
        assert_eq!(server.requests.lock().unwrap().iter().filter(|request| request.path.ends_with("?wait=25")).count(), 1);
    }
}

#[tokio::test]
async fn phased_mirelo_predispatch_cancel_proves_no_submission() {
    use kumi_ai::audio::mirelo::Client;
    let server = server(fixture).await;
    let client = Client::with_base(&server.url, "secret").unwrap();
    let signal = Signal::new();
    let prepared = client.prepare(Task::TextToSfx, &json!({"prompt":"rain","duration_ms":1000}), &signal).await.unwrap();
    signal.cancel();
    let error = client.submit(prepared, &signal).await.err().unwrap();
    assert!(error.not_submitted());
    assert_eq!(server.requests.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn phased_mirelo_get_and_wait_recover_without_post_or_download() {
    use kumi_ai::audio::mirelo::Client;
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let count = Arc::clone(&calls);
    let server = server(move |request, base| {
        assert!(request.headers.starts_with("GET "));
        assert_eq!(request.path, "/v3/text-to-sfx/generations/job-1");
        if count.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 0 {
            answer(json!({"id":"job-1","status":"queued","credits":null}))
        } else {
            answer(completed(base, false))
        }
    })
    .await;
    let client = Client::with_base(&server.url, "secret").unwrap();
    let signal = Signal::new();
    assert!(client.get(Task::TextToSfx, "../escape", &signal).await.is_err());
    assert!(server.requests.lock().unwrap().is_empty());
    let job = client.get(Task::TextToSfx, "job-1", &signal).await.unwrap();
    assert_eq!(job.status(), "queued");
    assert_eq!(job.actual_credits(), None);
    let job = client.wait(job, &signal).await.unwrap();
    assert!(job.succeeded());
    assert_eq!(job.actual_credits(), Some(10));
    assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 2);
    assert_eq!(server.requests.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn phased_mirelo_terminal_charge_is_available_before_failed_bounded_delivery() {
    use kumi_ai::audio::mirelo::Client;
    let server = server(fixture).await;
    let client = Client::with_base(&server.url, "key-sentinel").unwrap();
    let signal = Signal::new();
    let job = client.get(Task::TextToSfx, "job-1", &signal).await.unwrap();
    let actual_checkpoint = job.actual_credits();
    assert_eq!(actual_checkpoint, Some(10));
    assert_eq!(server.requests.lock().unwrap().len(), 1, "receipt retrieval has not downloaded");
    let dir = tempfile::tempdir().unwrap();
    let error = client.deliver(job, dir.path(), 1, &signal).await.err().unwrap();
    assert_eq!(actual_checkpoint, Some(10));
    assert!(!error.to_string().contains("key-sentinel"));
    assert!(!error.to_string().contains(&server.url));
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
    assert!(server.requests.lock().unwrap().iter().all(|request| request.headers.starts_with("GET ")));
}

#[tokio::test]
async fn phased_mirelo_notes_only_are_normalized_real_bounded_outputs() {
    use kumi_ai::audio::mirelo::{Client, OutputKind};
    let server = server(|_, _| {
        answer(json!({"job_id":"notes1","status":"succeeded","credits":3,"result":{"notes":[
            {"pitch":60.0,"velocity":100.5,"start":0.0,"end":0.5,"instrument":"acoustic_piano",
            "provider_private":"key-sentinel https://signed.example/private /private/source.wav"}
        ]}}))
    })
    .await;
    let client = Client::with_base(&server.url, "key-sentinel").unwrap();
    let signal = Signal::new();
    let dir = tempfile::tempdir().unwrap();
    let job = client.get(Task::AudioToMidi, "notes1", &signal).await.unwrap();
    let delivery = client.deliver(job, dir.path(), 4096, &signal).await.unwrap();
    assert_eq!(delivery.outputs.len(), 1);
    let output = &delivery.outputs[0];
    assert_eq!(output.kind, OutputKind::Notes);
    assert_eq!(output.mime, "application/json");
    assert_eq!(output.path.extension().unwrap(), "json");
    let saved: Value = serde_json::from_slice(&tokio::fs::read(&output.path).await.unwrap()).unwrap();
    assert_eq!(Some(&saved), delivery.notes.as_ref());
    assert_eq!(saved, json!([{"pitch":60,"velocity":101,"start":0.0,"end":0.5,"instrument":"acoustic_piano"}]));
    let job = client.get(Task::AudioToMidi, "notes1", &signal).await.unwrap();
    let small = tempfile::tempdir().unwrap();
    assert!(client.deliver(job, small.path(), 1, &signal).await.is_err());
    assert_eq!(std::fs::read_dir(small.path()).unwrap().count(), 0);
    assert!(server.requests.lock().unwrap().iter().all(|request| request.headers.starts_with("GET ")));
}

#[tokio::test]
async fn phased_mirelo_rejects_quote_flags_and_invalid_estimates_without_create() {
    use kumi_ai::audio::mirelo::Client;
    let signal = Signal::new();
    for amount in [Value::Null, json!(true), json!(-1), json!(1.5), json!(1.0), json!(9007199254740992u64)] {
        let server = server(
            move |request, base| {
                if request.path.ends_with("/preflight") {
                    answer(json!({"credits":amount}))
                } else {
                    fixture(request, base)
                }
            },
        )
        .await;
        let client = Client::with_base(&server.url, "secret").unwrap();
        assert!(client.prepare(Task::TextToSfx, &json!({"prompt":"rain","duration_ms":1000}), &signal).await.is_err());
        assert_eq!(server.requests.lock().unwrap().len(), 2);
    }
    let server = server(fixture).await;
    let client = Client::with_base(&server.url, "secret").unwrap();
    for input in [json!({"job_id":"known"}), json!({"prompt":"rain","duration_ms":1000,"quote":false})] {
        assert!(client.prepare(Task::TextToSfx, &input, &signal).await.is_err());
    }
    assert!(server.requests.lock().unwrap().is_empty());
}

#[tokio::test]
async fn phased_mirelo_receipt_amounts_status_and_errors_are_strict_and_private() {
    use kumi_ai::audio::mirelo::Client;
    let signal = Signal::new();
    for amount in [Value::Null, json!(false), json!(-1), json!(1.0), json!(9007199254740992u64), json!("key-sentinel")] {
        let server = server(move |_, _| {
            answer(json!({"id":"known","status":"key-sentinel /private/source.wav https://signed.example","credits":amount}))
        })
        .await;
        let client = Client::with_base(&server.url, "key-sentinel").unwrap();
        let job = client.get(Task::TextToSfx, "known", &signal).await.unwrap();
        assert_eq!(job.actual_credits(), None);
        assert_eq!(job.status(), "unknown");
        let dir = tempfile::tempdir().unwrap();
        let error = client.deliver(job, dir.path(), 1024, &signal).await.err().unwrap();
        assert!(!format!("{error} {error:?}").contains("key-sentinel"));
    }
    let server = server(|_, _| {
        let mut reply =
            answer(json!({"error":{"message":"key-sentinel /private/source.wav https://signed.example","retryable":false}})).unwrap();
        reply.status = 400;
        Some(reply)
    })
    .await;
    let client = Client::with_base(&server.url, "key-sentinel").unwrap();
    let error = client.prepare(Task::TextToSfx, &json!({"prompt":"rain","duration_ms":1000}), &signal).await.err().unwrap();
    let get_error = client.get(Task::TextToSfx, "known", &signal).await.err().unwrap();
    for error in [error, get_error] {
        let diagnostic = format!("{error} {error:?}");
        for secret in ["key-sentinel", "/private/", "https://", "signed.example"] {
            assert!(!diagnostic.contains(secret));
        }
    }
}

#[tokio::test]
async fn phased_mirelo_midi_prepare_uploads_but_standalone_quote_stays_preflight_only() {
    use kumi_ai::audio::mirelo::Client;
    let server = server(|request, base| match request.path.as_str() {
        "/v2/audio-to-midi/v1.0/preflight?duration_ms=1000" => answer(json!({"credits":3,"billing_mode":"unmetered"})),
        "/v2/assets" => answer(json!({"asset_id":"asset1","upload_url":format!("{base}/put")})),
        "/put" => Some(Reply { status: 200, mime: "text/plain", body: vec![], headers: vec![] }),
        "/v2/audio-to-midi/v1.0/jobs" => answer(json!({"job_id":"midi1","status":"queued","credits":null})),
        _ => panic!("unexpected MIDI request"),
    })
    .await;
    let dir = tempfile::tempdir().unwrap();
    let audio = dir.path().join("source.wav");
    tokio::fs::write(&audio, b"RIFF0000WAVEfmt ").await.unwrap();
    let service = service(&dir, config(&server.url, Task::AudioToMidi, LocalProtocol::Mirelo));
    let signal = Signal::new();
    service
        .execute(Task::AudioToMidi, json!({"audio":audio,"duration_ms":1000,"quote":true}), Selection::Local, &signal, &|_| {})
        .await
        .unwrap();
    assert_eq!(server.requests.lock().unwrap().len(), 1);
    let client = Client::with_base(&server.url, "secret").unwrap();
    let prepared = client.prepare(Task::AudioToMidi, &json!({"audio":audio,"duration_ms":1000}), &signal).await.unwrap();
    assert_eq!(prepared.credits(), 3);
    {
        let requests = server.requests.lock().unwrap();
        assert_eq!(requests.len(), 4);
        assert!(requests.iter().all(|request| !request.path.ends_with("/jobs")));
        assert!(requests[3].headers.starts_with("PUT "));
        assert!(!requests[3].headers.to_lowercase().contains("authorization:"));
    }
    let job = client.submit(prepared, &signal).await.unwrap();
    assert_eq!(job.id(), Some("midi1"));
    assert_eq!(job.actual_credits(), None);
    assert_eq!(server.requests.lock().unwrap().iter().filter(|request| request.path.ends_with("/jobs")).count(), 1);
}

#[tokio::test]
async fn phased_mirelo_failed_job_keeps_charge_without_attempting_delivery() {
    use kumi_ai::audio::mirelo::Client;
    let server = server(|_, base| {
        answer(json!({"id":"failed1","status":"failed","credits":7,"error":"key-sentinel",
        "result":{"output":{"variants":[{"status":"succeeded","files":{"audio":{"url":format!("{base}/secret-download")}}}]}}}))
    })
    .await;
    let client = Client::with_base(&server.url, "key-sentinel").unwrap();
    let signal = Signal::new();
    let job = client.get(Task::TextToSfx, "failed1", &signal).await.unwrap();
    let job = client.wait(job, &signal).await.unwrap();
    assert_eq!(job.status(), "failed");
    assert_eq!(job.actual_credits(), Some(7));
    let dir = tempfile::tempdir().unwrap();
    let error = client.deliver(job, dir.path(), 1024, &signal).await.err().unwrap();
    assert!(!error.to_string().contains("key-sentinel"));
    assert_eq!(server.requests.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn phased_mirelo_credit_boundaries_and_recovery_identity_are_exact() {
    use kumi_ai::audio::mirelo::Client;
    let signal = Signal::new();
    for amount in [0, 9007199254740991u64] {
        let server = server(move |request, base| {
            if request.path.ends_with("/preflight") {
                answer(json!({"credits":amount}))
            } else if request.path.ends_with("?wait=25") {
                let mut job = completed(base, false);
                job["credits"] = json!(amount);
                answer(job)
            } else {
                fixture(request, base)
            }
        })
        .await;
        let client = Client::with_base(&server.url, "secret").unwrap();
        let prepared = client.prepare(Task::TextToSfx, &json!({"prompt":"rain","duration_ms":1000}), &signal).await.unwrap();
        assert_eq!(prepared.credits(), amount);
        let job = client.submit(prepared, &signal).await.unwrap();
        assert_eq!(job.actual_credits(), Some(amount));
    }
    let server = server(|_, _| answer(json!({"id":"different","status":"succeeded","credits":1}))).await;
    let client = Client::with_base(&server.url, "secret").unwrap();
    assert!(client.get(Task::TextToSfx, "known", &signal).await.is_err());
    assert_eq!(server.requests.lock().unwrap().len(), 1);
}
