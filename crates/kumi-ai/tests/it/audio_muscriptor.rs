use base64::{engine::general_purpose::STANDARD, Engine};
use kumi_ai::{
    audio::{Config, Endpoint, LocalProtocol, Selection, Service, ServiceOptions, Task, TaskConfig},
    auth::store::open_credential_store,
};
use kumi_common::abort::Signal;
use serde_json::{json, Value};
use std::{collections::BTreeMap, rc::Rc, time::Duration};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const MIDI: &[u8] = b"MThd\0\0\0\x06\0\0\0\x01\x01\xe0MTrk\0\0\0\x04\0\xff\x2f\0";

struct Server {
    url: String,
    task: tokio::task::JoinHandle<Vec<u8>>,
}
impl Drop for Server {
    fn drop(&mut self) {
        self.task.abort();
    }
}
async fn server(body: String) -> Server {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let task = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut request = Vec::new();
        let mut chunk = [0; 8192];
        let end = loop {
            let n = stream.read(&mut chunk).await.unwrap();
            assert_ne!(n, 0, "client closed before sending HTTP headers");
            request.extend_from_slice(&chunk[..n]);
            if let Some(end) = request.windows(4).position(|bytes| bytes == b"\r\n\r\n") {
                break end + 4;
            }
        };
        let headers = String::from_utf8_lossy(&request[..end]);
        assert!(headers.starts_with("POST /transcribe HTTP/1.1\r\n"));
        let length = headers
            .lines()
            .filter_map(|line| line.split_once(':'))
            .find(|(name, _)| name.eq_ignore_ascii_case("content-length"))
            .unwrap()
            .1
            .trim()
            .parse::<usize>()
            .unwrap();
        while request.len() < end + length {
            let n = stream.read(&mut chunk).await.unwrap();
            assert_ne!(n, 0, "client closed before sending multipart audio");
            request.extend_from_slice(&chunk[..n]);
        }
        let header =
            format!("HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len());
        stream.write_all(header.as_bytes()).await.unwrap();
        for chunk in body.as_bytes().chunks(7) {
            // A rejected stream may close its socket before the fixture has finished.
            if stream.write_all(chunk).await.is_err() {
                break;
            }
        }
        request
    });
    Server { url, task }
}
fn service(dir: &tempfile::TempDir, url: &str) -> Rc<Service> {
    let config = Config {
        acknowledged: false,
        tasks: BTreeMap::from([(
            Task::AudioToMidi,
            TaskConfig {
                auto_mirelo: false,
                local: Some(Endpoint { url: url.into(), protocol: LocalProtocol::Muscriptor, model: None }),
                fallback: false,
            },
        )]),
        ..Default::default()
    };
    Service::new(ServiceOptions {
        store: Rc::new(open_credential_store(dir.path().join("auth.json"))),
        env: Default::default(),
        read_config: Rc::new(move || config.clone()),
        write_config: Rc::new(|_| Ok(())),
        output_dir: dir.path().join("audio"),
        mirelo_base: None,
    })
}
async fn audio(dir: &tempfile::TempDir) -> std::path::PathBuf {
    let path = dir.path().join("Unreleased_feat_Private_v3.wav");
    tokio::fs::write(&path, b"RIFF0000WAVEfmt ").await.unwrap();
    path
}
fn frame(event: Value) -> String {
    format!("data: {event}\n\n")
}
fn start(index: u64, pitch: u8, time: f64) -> String {
    frame(json!({"type":"start","index":index,"pitch":pitch,"start_time":time,"instrument":"acoustic_piano"}))
}
fn end(index: u64, time: f64) -> String {
    frame(json!({"type":"end","start_event_index":index,"end_time":time}))
}
fn complete() -> String {
    frame(json!({"type":"transcription_complete","data":STANDARD.encode(MIDI),"beat_grid":{"onset_delay":0.25}}))
}
async fn assert_saved(result: &Value, expected: Value) {
    assert_eq!(result["status"], "succeeded");
    assert_eq!(result["protocol"], "muscriptor");
    assert_eq!(result["notes"], expected);
    assert_eq!(tokio::fs::read(result["midi_path"].as_str().unwrap()).await.unwrap(), MIDI);
    let notes: Value = serde_json::from_slice(&tokio::fs::read(result["notes_path"].as_str().unwrap()).await.unwrap()).unwrap();
    assert_eq!(notes, expected);
}

#[tokio::test]
async fn reversed_note_is_repaired_before_onset_correction_and_matching_notes_survive() {
    let mut server = server(start(7, 64, 0.5) + &start(2, 67, 0.75) + &end(2, 1.25) + &end(7, 0.4) + &complete()).await;
    let dir = tempfile::tempdir().unwrap();
    let service = service(&dir, &server.url);
    let result = service
        .execute(
            Task::AudioToMidi,
            json!({"audio":audio(&dir).await,"instruments":["acoustic_piano"]}),
            Selection::Local,
            &Signal::new(),
            &|_| {},
        )
        .await
        .unwrap();
    assert_saved(
        &result,
        json!([
            {"pitch":64,"start":0.25,"end":0.26,"instrument":"acoustic_piano","velocity":100},
            {"pitch":67,"start":0.5,"end":1.0,"instrument":"acoustic_piano","velocity":100}
        ]),
    )
    .await;
    let request = String::from_utf8((&mut server.task).await.unwrap()).unwrap();
    assert!(request.contains("filename=\"input.wav\""));
    assert!(!request.contains("Unreleased_feat_Private_v3"));
    assert!(request.contains("name=\"detect_tempo\"\r\n\r\nbest-effort"));
    assert!(request.contains("name=\"instruments\"\r\n\r\nacoustic_piano"));
    assert!(request.contains("RIFF0000WAVEfmt "));
}

#[tokio::test]
async fn progress_and_unknown_extensions_preserve_completed_notes_and_overlap_trimming() {
    let body = frame(json!({"type":"progress","completed":0,"total":1}))
        + &start(1, 60, 0.5)
        + "event: model_status\ndata: {\"type\":\"warming_up\",\"message\":\"ready\"}\n\n"
        + &start(2, 60, 0.75)
        + &end(1, 1.5)
        + "event: server_metrics\ndata: {\"queue_depth\":0}\n\n"
        + &end(2, 1.25)
        + &frame(json!({"type":"progress","completed":1,"total":1}))
        + &complete();
    let mut server = server(body).await;
    let dir = tempfile::tempdir().unwrap();
    let result = service(&dir, &server.url)
        .execute(Task::AudioToMidi, json!({"audio":audio(&dir).await}), Selection::Local, &Signal::new(), &|_| {})
        .await
        .unwrap();
    assert_saved(
        &result,
        json!([
            {"pitch":60,"start":0.25,"end":0.5,"instrument":"acoustic_piano","velocity":100},
            {"pitch":60,"start":0.5,"end":1.0,"instrument":"acoustic_piano","velocity":100}
        ]),
    )
    .await;
    (&mut server.task).await.unwrap();
}

#[tokio::test]
async fn native_named_and_json_errors_prevent_completion_with_safe_diagnostics() {
    let detail = format!("decoder failed local-secret\n\u{1b}[31m{}", "x".repeat(4096));
    for error_frame in [
        format!("event: error\ndata: {}\n\n", json!({"error":detail})),
        frame(json!({"type":"error","message":detail})),
        format!("event: error\ndata: {}\n\n", json!({"type":"transcription_complete","data":STANDARD.encode(MIDI),"error":detail})),
    ] {
        let mut server = server(start(1, 60, 0.5) + &end(1, 1.0) + &error_frame + &complete()).await;
        let dir = tempfile::tempdir().unwrap();
        let service = service(&dir, &server.url);
        service.save_local_key(Task::AudioToMidi, "local-secret").await.unwrap();
        let error = service
            .execute(Task::AudioToMidi, json!({"audio":audio(&dir).await}), Selection::Local, &Signal::new(), &|_| {})
            .await
            .unwrap_err()
            .to_string();
        assert!(error.contains("MuScriptor reported a transcription error"), "{error}");
        assert!(error.contains("decoder failed"), "{error}");
        assert!(!error.contains("local-secret"), "{error}");
        assert!(!error.contains('\n') && !error.contains('\u{1b}'), "{error}");
        assert!(error.len() < 2048, "diagnostic was not bounded: {} bytes", error.len());
        assert!(!dir.path().join("audio").exists(), "failed streams must not save completed outputs");
        (&mut server.task).await.unwrap();
    }
}

#[tokio::test]
async fn malformed_notes_payloads_and_midi_cannot_be_hidden_by_completion() {
    for (body, diagnostic) in [
        (frame(Value::Null) + &complete(), "Invalid MuScriptor event envelope"),
        (frame(json!({"type":42})) + &complete(), "Invalid MuScriptor event envelope"),
        (frame(json!({"data":"missing type"})) + &complete(), "Invalid MuScriptor event envelope"),
        (end(99, 1.0) + &complete(), "no matching start"),
        (start(1, 60, 0.5) + &start(1, 62, 0.75) + &complete(), "Duplicate open"),
        (start(1, 60, 0.5) + &complete(), "unclosed notes"),
        ("data: {invalid json}\n\n".to_owned() + &complete(), "Invalid JSON"),
        (frame(json!({"type":"transcription_complete","data":"!bad!"})), "Invalid base64 MIDI"),
        (frame(json!({"type":"transcription_complete","data":STANDARD.encode(b"not MIDI")})), "did not contain MIDI"),
        (
            frame(json!({"type":"transcription_complete","data":STANDARD.encode(MIDI),"beat_grid":{"onset_delay":"bad"}})),
            "Invalid MuScriptor onset delay",
        ),
        ("event: error\ndata: decoder crashed\n\n".to_owned() + &complete(), "reported a transcription error"),
    ] {
        let mut server = server(body).await;
        let dir = tempfile::tempdir().unwrap();
        let error = service(&dir, &server.url)
            .execute(Task::AudioToMidi, json!({"audio":audio(&dir).await}), Selection::Local, &Signal::new(), &|_| {})
            .await
            .unwrap_err()
            .to_string();
        assert!(error.contains(diagnostic), "expected {diagnostic}, got {error}");
        assert!(!dir.path().join("audio").exists(), "invalid streams must not save completed outputs");
        (&mut server.task).await.unwrap();
    }
}

#[tokio::test]
async fn extensions_and_unframed_completion_cannot_turn_truncated_streams_into_success() {
    for tail in [String::new(), frame(json!({"type":"future_extension","finished":true})), complete().trim_end().to_owned()] {
        let mut server = server(start(1, 60, 0.5) + &end(1, 1.0) + &tail).await;
        let dir = tempfile::tempdir().unwrap();
        let error = service(&dir, &server.url)
            .execute(Task::AudioToMidi, json!({"audio":audio(&dir).await}), Selection::Local, &Signal::new(), &|_| {})
            .await
            .unwrap_err()
            .to_string();
        assert!(error.contains("without transcription_complete"), "{error}");
        assert!(!dir.path().join("audio").exists(), "incomplete streams must not save completed outputs");
        (&mut server.task).await.unwrap();
    }
}

#[tokio::test]
async fn m4a_and_aac_are_rejected_before_any_http_connection() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let service = service(&dir, &format!("http://{}", listener.local_addr().unwrap()));
    for (extension, bytes) in [("m4a", &b"\0\0\0\x10ftypM4A "[..]), ("aac", &b"\xff\xf1\0\0"[..])] {
        let path = dir.path().join(format!("input.{extension}"));
        tokio::fs::write(&path, bytes).await.unwrap();
        let error = tokio::time::timeout(
            Duration::from_secs(2),
            service.execute(Task::AudioToMidi, json!({"audio":path}), Selection::Local, &Signal::new(), &|_| {}),
        )
        .await
        .expect("unsupported inputs must not wait for HTTP")
        .unwrap_err()
        .to_string();
        assert!(error.contains("M4A and AAC are not supported"), "{error}");
        assert_eq!(listener.accept().unwrap_err().kind(), std::io::ErrorKind::WouldBlock);
    }
}
