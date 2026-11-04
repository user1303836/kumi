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
}
fn answer(value: Value) -> Option<Reply> {
    Some(Reply { status: 200, mime: "application/json", body: serde_json::to_vec(&value).unwrap() })
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
                let request = Request { path: headers.split_whitespace().nth(1).unwrap().to_owned(), headers, body: bytes[end..].to_vec() };
                recorded.lock().unwrap().push(request.clone());
                let Some(reply) = handler(&request, &base) else {
                    return;
                };
                let header = format!(
                    "HTTP/1.1 {} Test\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    reply.status,
                    reply.mime,
                    reply.body.len()
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
    }
}
fn service(dir: &tempfile::TempDir, config: Config) -> Rc<Service> {
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
        Some(Reply { status: 204, mime: "text/plain", body: vec![] })
    } else if request.path == "/download" {
        Some(Reply { status: 200, mime: "audio/wav", body: b"RIFF0000WAVEfmt ".to_vec() })
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
    let audio = dir.path().join("input.wav");
    let video = dir.path().join("input.mp4");
    tokio::fs::write(&audio, b"RIFF0000WAVEfmt ").await.unwrap();
    tokio::fs::write(&video, b"0000ftypisom0000").await.unwrap();
    for (task, input, family, model) in [
        (Task::TextToSfx, json!({"prompt":"rain","duration_ms":1000}), "generations", "sfx-2.0"),
        (Task::VideoToSfx, json!({"video":video,"duration_ms":1000}), "generations", "sfx-2.0"),
        (Task::Extend, json!({"audio":audio,"append_duration_ms":1000}), "edits", "sfx-1.6"),
        (Task::Inpaint, json!({"audio":audio,"region_start_ms":1000,"region_end_ms":2000}), "edits", "sfx-1.6"),
    ] {
        let service = service(&dir, config(&server.url, task, LocalProtocol::Mirelo));
        service.save_local_key(task, "local-secret").await.unwrap();
        let result = service.execute(task, input, Selection::Auto, &Signal::new()).await.unwrap();
        assert_eq!(result["provider"], "local");
        assert!(std::path::Path::new(result["files"][0]["path"].as_str().unwrap()).is_file());
        let requests = server.requests.lock().unwrap();
        let prefix = format!("/v3/{}/{family}", task.as_str());
        let quote = requests.iter().rev().find(|r| r.path == format!("{prefix}/preflight")).unwrap();
        let create = requests.iter().rev().find(|r| r.path == format!("{prefix}?wait=25")).unwrap();
        assert_eq!(quote.body, create.body);
        assert_eq!(serde_json::from_slice::<Value>(&create.body).unwrap()["model"], model);
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
}
#[tokio::test]
async fn partial_output_is_saved_but_is_not_reported_as_complete_or_submitted_twice() {
    let server =
        server(|request, base| if request.path.contains("?wait=") { answer(completed(base, true)) } else { fixture(request, base) }).await;
    let dir = tempfile::tempdir().unwrap();
    let service = service(&dir, config(&server.url, Task::TextToSfx, LocalProtocol::Mirelo));
    let error = service
        .execute(Task::TextToSfx, json!({"prompt":"rain","duration_ms":1000}), Selection::Auto, &Signal::new())
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
        .execute(Task::TextToSfx, json!({"prompt":"rain","duration_ms":1000}), Selection::Auto, &Signal::new())
        .await
        .unwrap_err()
        .to_string();
    assert!(error.contains("no retry or fallback"));
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
        .execute(Task::TextToSfx, json!({"prompt":"rain","duration_ms":1000}), Selection::Local, &Signal::new())
        .await
        .unwrap_err()
        .to_string();
    assert!(error.contains("configured API origin"));
    assert!(other.requests.lock().unwrap().is_empty());
    service.configure(config(&other.url, Task::TextToSfx, LocalProtocol::Mirelo)).unwrap();
    service
        .execute(Task::TextToSfx, json!({"prompt":"rain","duration_ms":1000,"quote":true}), Selection::Local, &Signal::new())
        .await
        .unwrap();
    assert!(other.requests.lock().unwrap().iter().all(|request| !request.headers.to_lowercase().contains("authorization:")));
}
#[tokio::test]
async fn midi_uses_v2_asset_put_async_job_and_authoritative_notes() {
    let server = server(|request, base| {
        if request.path.contains("/preflight?") { answer(json!({"credits":3,"billing_mode":"unmetered"})) }
        else if request.path == "/v2/assets" { answer(json!({"asset_id":"midi-asset","upload_url":format!("{base}/put")})) }
        else if request.path == "/put" { Some(Reply { status:200,mime:"text/plain",body:vec![] }) }
        else if request.path.ends_with("/jobs") { answer(json!({"job_id":"midi1","job_url":"/v2/audio-to-midi/v1.0/jobs/midi1"})) }
        else if request.path.ends_with("/midi1") { answer(json!({"job_id":"midi1","status":"succeeded","result":{"model":"audio-to-midi-1.0","midi_url":format!("{base}/midi"),"notes":[{"pitch":60,"start":0.0,"end":0.5,"velocity":100,"instrument":"acoustic_piano"}]}})) }
        else { Some(Reply { status:200,mime:"audio/midi",body:b"MThd0000".to_vec() }) }
    }).await;
    let dir = tempfile::tempdir().unwrap();
    let audio = dir.path().join("input.wav");
    tokio::fs::write(&audio, b"RIFF0000WAVEfmt ").await.unwrap();
    let service = service(&dir, config(&server.url, Task::AudioToMidi, LocalProtocol::Mirelo));
    service.save_local_key(Task::AudioToMidi, "midi-secret").await.unwrap();
    let result =
        service.execute(Task::AudioToMidi, json!({"audio":audio,"duration_ms":1000}), Selection::Auto, &Signal::new()).await.unwrap();
    assert_eq!(result["notes"][0]["end"], 0.5);
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
            })
        })
        .await;
        let dir = tempfile::tempdir().unwrap();
        let audio = dir.path().join("input.wav");
        tokio::fs::write(&audio, b"RIFF0000WAVEfmt ").await.unwrap();
        let service = service(&dir, config(&server.url, Task::AudioToMidi, LocalProtocol::Muscriptor));
        let result = service.execute(Task::AudioToMidi, json!({"audio":audio}), Selection::Local, &Signal::new()).await;
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
        .execute(Task::TextToSfx, input.clone(), Selection::Local, &Signal::new())
        .await
        .unwrap_err()
        .to_string()
        .contains("cannot fund"));
    let mut quote = input.clone();
    quote["quote"] = json!(true);
    assert_eq!(service.execute(Task::TextToSfx, quote, Selection::Local, &Signal::new()).await.unwrap()["submitted"], false);
    let signal = Signal::new();
    signal.cancel();
    assert!(service.execute(Task::TextToSfx, input, Selection::Local, &signal).await.unwrap_err().to_string().contains("cancelled"));
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
            .execute(Task::TextToSfx, json!({"prompt":"rain","duration_ms":1000}), Selection::Local, &Signal::new())
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
    });
    assert_eq!(service.preferred(Task::AudioToMidi).await.unwrap(), Some(Selection::Local));
    assert_eq!(service.preferred(Task::TextToSfx).await.unwrap(), Some(Selection::Mirelo));
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
        .execute(Task::TextToSfx, json!({"prompt":"rain","duration_ms":1000,"quote":true}), Selection::Local, &Signal::new())
        .await
        .unwrap();
    assert!(server.requests.lock().unwrap().iter().all(|request| !request.headers.to_lowercase().contains("authorization:")));
    service.configure(config(&server.url, Task::Extend, LocalProtocol::Mirelo)).unwrap();
    let before = server.requests.lock().unwrap().len();
    assert!(service
        .execute(Task::Extend, json!({"audio":dir.path(),"append_duration_ms":1000}), Selection::Local, &Signal::new())
        .await
        .is_err());
    assert_eq!(server.requests.lock().unwrap().len(), before);
}
