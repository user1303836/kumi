//! The host's bridge client against simulated REAPER and Renoise bridges: the handshake, requests, events, busy and
//! quitting, with golden NDJSON transcripts of the wire (tests/golden/*-bridge.ndjson; KUMI_BLESS=1 rewrites them).
use daw_bridge::auth::Secret;
use daw_bridge::wire::Project;
use daw_mcp_server::client::{BridgeClient, ConnectError, RequestError};
use daw_mcp_server::simulator::{Line, Simulator, SimulatorConfig};
use serde_json::{Value, json};
use std::path::PathBuf;
use std::time::Duration;

const CLIENT: &str = "daw-mcp-server/test";
const WAIT: Duration = Duration::from_secs(5);

fn secret() -> Secret {
    Secret::new("simulated-secret-0123456789-abcdefghijklmnop").unwrap()
}

/// The transcript with what changes from run to run (nonces, proofs, sessions, the pid) replaced by names.
pub fn normalized(lines: &[Line]) -> String {
    let mut out = String::new();
    for line in lines {
        let mut value: Value = serde_json::from_str(&line.text).unwrap_or_else(|_| Value::String(line.text.clone()));
        // Credit frames are left out, like heartbeats: how many bytes a host grants depends on the receive buffer
        // its system gave it and on how the bytes arrived.
        if value.get("type").and_then(Value::as_str) == Some("credit") {
            continue;
        }
        if let Some(object) = value.as_object_mut() {
            for key in ["nonce", "proof", "session"] {
                if object.contains_key(key) {
                    object.insert(key.into(), format!("<{key}>").into());
                }
            }
            if object.contains_key("pid") {
                object.insert("pid".into(), "<pid>".into());
            }
            if object.contains_key("deadlineMs") {
                object.insert("deadlineMs".into(), "<deadline>".into());
            }
            if let Some(result) = object.get_mut("result").and_then(Value::as_object_mut) {
                if result.contains_key("pid") {
                    result.insert("pid".into(), "<pid>".into());
                }
            }
        }
        out.push_str(&format!("{} {} {}\n", line.connection, if line.from_host { ">" } else { "<" }, value));
    }
    out
}

pub fn golden(name: &str, made: &str) {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests").join("golden").join(name);
    if std::env::var("KUMI_BLESS").is_ok_and(|value| value == "1") {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, made).unwrap();
        return;
    }
    let kept = std::fs::read_to_string(&path).unwrap_or_else(|_| panic!("{} is missing; run with KUMI_BLESS=1", path.display()));
    assert_eq!(made, kept, "{name} changed; bless it if that's intended");
}

async fn session(simulator: &Simulator) {
    let client = BridgeClient::connect(simulator.address(), secret(), CLIENT).await.unwrap();
    let mut events = client.events();
    assert_eq!(client.request("ping", json!({}), WAIT).await.unwrap(), json!({}));
    let status = client.request("status", json!({}), WAIT).await.unwrap();
    assert_eq!(status["busy"], false);
    assert_eq!(
        client.request("render", json!({}), WAIT).await,
        Err(RequestError::Bridge { code: "unknown-method".into(), message: "no method render".into() })
    );
    simulator.open_project(Some(Project { name: Some("Next".into()), ..Project::default() })).unwrap();
    let event = tokio::time::timeout(WAIT, events.recv()).await.unwrap().unwrap();
    assert_eq!((event.seq, event.name.as_str(), &event.data), (1, "project", &json!({"name": "Next"})));
    assert_eq!(client.request("status", json!({}), WAIT).await.unwrap()["project"], json!({"name": "Next"}));
    // Busy, as the bridges are: status still answers (saying why), work is refused.
    simulator.set_busy(Some("rendering"));
    assert_eq!(client.request("status", json!({}), WAIT).await.unwrap()["busy"], "rendering");
    assert!(matches!(client.request("render", json!({}), WAIT).await, Err(RequestError::Bridge { code, .. }) if code == "busy"));
    simulator.set_busy(None);
    simulator.stop();
    for _ in 0..100 {
        if client.is_closed() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(client.is_closed());
    assert_eq!(client.request("ping", json!({}), WAIT).await, Err(RequestError::Closed));
}

#[tokio::test]
async fn reaper_session_transcript() {
    let simulator = Simulator::start(SimulatorConfig::reaper(secret(), "/lab/REAPER")).await.unwrap();
    session(&simulator).await;
    golden("reaper-bridge.ndjson", &normalized(&simulator.transcript()));
}

#[tokio::test]
async fn renoise_session_transcript() {
    let simulator = Simulator::start(SimulatorConfig::renoise(secret(), "/lab/Renoise/V3.5.4")).await.unwrap();
    session(&simulator).await;
    let transcript = normalized(&simulator.transcript());
    assert!(transcript.contains("\"demo\":false"));
    golden("renoise-bridge.ndjson", &transcript);
}

#[tokio::test]
async fn a_wrong_secret_or_a_non_loopback_address_never_connects() {
    let simulator = Simulator::start(SimulatorConfig::reaper(secret(), "/lab/REAPER")).await.unwrap();
    let other = Secret::new("another-secret-0123456789-abcdefghijklmnopq").unwrap();
    let refused = BridgeClient::connect(simulator.address(), other, CLIENT).await;
    assert!(matches!(refused, Err(ConnectError::Handshake(daw_bridge::handshake::HandshakeError::BadProof))));
    // The host never sent its own proof.
    assert!(simulator.transcript().iter().all(|line| !line.text.contains("\"auth\"")));
    let remote = "192.0.2.1:9770".parse().unwrap();
    assert!(matches!(BridgeClient::connect(remote, secret(), CLIENT).await, Err(ConnectError::NotLoopback(_))));
}

#[tokio::test]
async fn silence_reads_as_busy_and_heartbeats_clear_it() {
    let mut config = SimulatorConfig::renoise(secret(), "/lab/Renoise/V3.5.4");
    config.offer.heartbeat_ms = 40;
    let simulator = Simulator::start(config).await.unwrap();
    let client = BridgeClient::connect(simulator.address(), secret(), CLIENT).await.unwrap();
    assert_eq!(client.welcome().heartbeat_ms, 40);
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(!client.busy());
    simulator.set_busy(Some("showing a dialog"));
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(client.busy());
    simulator.set_busy(None);
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(!client.busy());
}

#[tokio::test]
async fn several_connections_each_number_their_own_events() {
    let simulator = Simulator::start(SimulatorConfig::reaper(secret(), "/lab/REAPER")).await.unwrap();
    let first = BridgeClient::connect(simulator.address(), secret(), CLIENT).await.unwrap();
    let mut first_events = first.events();
    simulator.emit("tracks", json!({"count": 3}));
    assert_eq!(tokio::time::timeout(WAIT, first_events.recv()).await.unwrap().unwrap().seq, 1);
    let second = BridgeClient::connect(simulator.address(), secret(), CLIENT).await.unwrap();
    let mut second_events = second.events();
    assert_ne!(first.welcome().session, second.welcome().session);
    simulator.emit("tracks", json!({"count": 4}));
    assert_eq!(tokio::time::timeout(WAIT, first_events.recv()).await.unwrap().unwrap().seq, 2);
    let event = tokio::time::timeout(WAIT, second_events.recv()).await.unwrap().unwrap();
    assert_eq!((event.seq, event.data), (1, json!({"count": 4})));
}

/// A listener that answers the hello with `reply` and closes.
async fn one_reply(reply: &'static str) -> std::net::SocketAddr {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut chunk = [0u8; 1024];
        let _ = stream.read(&mut chunk).await;
        let _ = stream.write_all(reply.as_bytes()).await;
    });
    address
}

#[tokio::test]
async fn refusals_early_closes_and_garbage_during_the_handshake() {
    let refused = BridgeClient::connect(
        one_reply("{\"type\":\"refused\",\"reason\":\"protocol\",\"protocol\":\"kumi-daw/2\"}\n").await,
        secret(),
        CLIENT,
    )
    .await;
    assert!(matches!(
        refused,
        Err(ConnectError::Handshake(daw_bridge::handshake::HandshakeError::Refused { ref reason, protocol: Some(ref protocol) })) if reason == "protocol" && protocol == "kumi-daw/2"
    ));
    assert!(matches!(BridgeClient::connect(one_reply("").await, secret(), CLIENT).await, Err(ConnectError::Closed)));
    assert!(matches!(
        BridgeClient::connect(one_reply("HTTP/1.1 400 Bad Request\r\n\r\n").await, secret(), CLIENT).await,
        Err(ConnectError::Handshake(_))
    ));
    let welcome_first = "{\"type\":\"welcome\",\"session\":\"00000000000000000000000000000000\",\"daw\":\"reaper\",\"bridge\":\"0\",\"dawVersion\":\"0\",\"pid\":1,\"resource\":\"/\",\"project\":null,\"maxFrameBytes\":1,\"heartbeatMs\":1}\n";
    assert!(matches!(BridgeClient::connect(one_reply(welcome_first).await, secret(), CLIENT).await, Err(ConnectError::Handshake(_))));
    // Nothing listening.
    let closed = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await.unwrap().local_addr().unwrap();
    assert!(matches!(BridgeClient::connect(closed, secret(), CLIENT).await, Err(ConnectError::Io(_))));
}

#[tokio::test]
async fn a_bridge_that_never_speaks_is_a_handshake_timeout() {
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let address = listener.local_addr().unwrap();
    let silent = tokio::spawn(async move {
        let (_stream, _) = listener.accept().await.unwrap();
        tokio::time::sleep(Duration::from_secs(5)).await;
    });
    assert!(matches!(
        BridgeClient::connect_within(address, secret(), CLIENT, Duration::from_millis(150)).await,
        Err(ConnectError::Timeout)
    ));
    silent.abort();
}

#[tokio::test]
async fn params_nested_past_the_limit_are_refused_before_sending() {
    let simulator = Simulator::start(SimulatorConfig::reaper(secret(), "/lab/REAPER")).await.unwrap();
    let client = BridgeClient::connect(simulator.address(), secret(), CLIENT).await.unwrap();
    let nest = |levels: usize| (1..levels).fold(json!({}), |inner, _| json!({"a": inner}));
    assert_eq!(client.request("ping", nest(daw_bridge::wire::MAX_PAYLOAD_DEPTH), WAIT).await.unwrap(), json!({}));
    let refused = client.request("ping", nest(daw_bridge::wire::MAX_PAYLOAD_DEPTH + 1), WAIT).await;
    assert!(matches!(refused, Err(RequestError::Bridge { ref code, .. }) if code == "bad-params"), "{refused:?}");
    assert!(!client.is_closed());
    assert_eq!(client.request("ping", json!({}), WAIT).await.unwrap(), json!({}));
    // Two pings reached the bridge, not three.
    assert_eq!(simulator.transcript().iter().filter(|line| line.from_host && line.text.contains("\"ping\"")).count(), 2);
}

#[tokio::test]
async fn an_answer_nested_past_the_limit_costs_that_answer_not_the_connection() {
    use daw_bridge::handshake::{ServerAction, ServerConnection};
    use daw_bridge::wire::Frame;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let address = listener.local_addr().unwrap();
    // A bridge that answers the first request with a line nested 70 levels deep, and the rest properly.
    tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let config = SimulatorConfig::reaper(secret(), "/lab/REAPER");
        let mut connection = ServerConnection::new(secret(), 0);
        let mut chunk = vec![0u8; 4096];
        loop {
            let read = stream.read(&mut chunk).await.unwrap_or(0);
            if read == 0 {
                return;
            }
            for action in connection.on_bytes(&chunk[..read], 0, &mut || (config.identity.clone(), config.offer.clone())) {
                match action {
                    ServerAction::Send(bytes) => stream.write_all(&bytes).await.unwrap(),
                    ServerAction::Request(request) if request.id == 1 => {
                        let line =
                            format!("{{\"type\":\"response\",\"id\":1,\"ok\":true,\"result\":{}{}}}\n", "[".repeat(70), "]".repeat(70));
                        stream.write_all(line.as_bytes()).await.unwrap();
                    }
                    ServerAction::Request(request) => {
                        stream.write_all(&Frame::ok(request.id, json!({"n": request.id})).encode()).await.unwrap()
                    }
                    _ => {}
                }
            }
        }
    });
    let client = BridgeClient::connect(address, secret(), CLIENT).await.unwrap();
    // The too-deep answer is dropped alone: that request gets no answer.
    assert!(matches!(client.request("ping", json!({}), Duration::from_millis(300)).await, Err(RequestError::Timeout)));
    assert!(!client.is_closed());
    assert_eq!(client.request("ping", json!({}), WAIT).await.unwrap(), json!({"n": 2}));
}

/// A bridge that welcomes the host and then never reads another byte (a Lua script that never returns, a dialog).
async fn a_bridge_that_stops_reading() -> std::net::SocketAddr {
    use daw_bridge::handshake::{ServerAction, ServerConnection};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let config = SimulatorConfig::reaper(secret(), "/lab/REAPER");
        let mut connection = ServerConnection::new(secret(), 0);
        let mut chunk = vec![0u8; 4096];
        loop {
            let read = stream.read(&mut chunk).await.unwrap_or(0);
            if read == 0 {
                return;
            }
            for action in connection.on_bytes(&chunk[..read], 0, &mut || (config.identity.clone(), config.offer.clone())) {
                match action {
                    ServerAction::Send(bytes) => stream.write_all(&bytes).await.unwrap(),
                    ServerAction::Authenticated => {
                        // Held open, unread.
                        tokio::time::sleep(Duration::from_secs(60)).await;
                        return;
                    }
                    _ => {}
                }
            }
        }
    });
    address
}

#[tokio::test]
async fn a_bridge_that_stopped_reading_cant_hold_a_request_past_its_timeout() {
    let client = BridgeClient::connect(a_bridge_that_stops_reading().await, secret(), CLIENT).await.unwrap();
    let started = std::time::Instant::now();
    // Far more than the socket buffers take: the write blocks for room that never comes.
    let big = client.clone();
    let writing = tokio::spawn(async move { big.request("big", json!({"pad": "x".repeat(32 << 20)}), Duration::from_secs(1)).await });
    tokio::time::sleep(Duration::from_millis(200)).await;
    // Behind it, a small request has its own, shorter timeout.
    // (A request stuck behind the write would never return: the outer timeout is only so a failure is a failure, not a hang.)
    let behind = tokio::time::timeout(Duration::from_secs(20), client.request("status", json!({}), Duration::from_millis(300)))
        .await
        .expect("a request waiting behind the write is stuck");
    assert!(matches!(behind, Err(RequestError::Timeout)), "{behind:?}");
    // (Building the frame is CPU on this single thread, before the write starts: slow runners need room.)
    assert!(started.elapsed() < Duration::from_secs(6), "{:?}", started.elapsed());
    let first = tokio::time::timeout(Duration::from_secs(20), writing).await.expect("the big request is stuck in its write").unwrap();
    assert!(matches!(first, Err(RequestError::Timeout)), "{first:?}");
    assert!(started.elapsed() < Duration::from_secs(12), "{:?}", started.elapsed());
    // Part of a frame went out, so the connection is gone rather than carrying a corrupt stream. (Windows' loopback
    // sockets buffer tens of MiB, so a write there may complete: it was the answer that never came.)
    if cfg!(not(windows)) {
        assert!(client.is_closed());
        assert!(matches!(client.request("ping", json!({}), WAIT).await, Err(RequestError::Closed)));
    }
}

#[tokio::test]
async fn a_host_that_only_listens_tells_the_bridge_it_is_reading() {
    let simulator = Simulator::start(SimulatorConfig::renoise(secret(), "/lab/Renoise/V3.5.4")).await.unwrap();
    let client = BridgeClient::connect(simulator.address(), secret(), CLIENT).await.unwrap();
    let mut events = client.events();
    let heartbeats = || simulator.transcript().iter().filter(|line| line.from_host && line.text.contains("\"heartbeat\"")).count();
    assert_eq!(heartbeats(), 0);
    // Not enough yet for the host to say anything.
    simulator.emit("tracks", json!({"pad": "x".repeat(4000)}));
    events.recv().await.unwrap();
    assert_eq!(heartbeats(), 0);
    for _ in 0..17 {
        simulator.emit("tracks", json!({"pad": "x".repeat(4000)}));
        events.recv().await.unwrap();
    }
    // About 72 KB read: one heartbeat each time 16 KiB came in without the host writing.
    for _ in 0..100 {
        if heartbeats() >= 3 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!((3..=4).contains(&heartbeats()), "{}", heartbeats());
    // Once the host has written, the count starts over.
    assert_eq!(client.request("ping", json!({}), WAIT).await.unwrap(), json!({}));
    assert!(!client.is_closed());
}

#[tokio::test]
async fn a_request_carries_the_time_the_host_gives_up() {
    let simulator = Simulator::start(SimulatorConfig::reaper(secret(), "/lab/REAPER")).await.unwrap();
    let client = BridgeClient::connect(simulator.address(), secret(), CLIENT).await.unwrap();
    let before = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() as u64;
    client.request("ping", json!({}), Duration::from_secs(3)).await.unwrap();
    let transcript = simulator.transcript();
    let line = transcript.iter().find(|line| line.from_host && line.text.contains("\"ping\"")).unwrap();
    let deadline = serde_json::from_str::<Value>(&line.text).unwrap()["deadlineMs"].as_u64().unwrap();
    assert!((before + 3000..=before + 3000 + 500).contains(&deadline), "{deadline} vs {before}");
}

#[tokio::test]
async fn a_host_tops_a_bridge_up_to_its_receive_buffer_then_gives_back_what_it_reads() {
    let simulator = Simulator::start(SimulatorConfig::renoise(secret(), "/lab/Renoise/V3.5.4")).await.unwrap();
    let client = BridgeClient::connect(simulator.address(), secret(), CLIENT).await.unwrap();
    let floor = daw_bridge::wire::CREDIT_FLOOR_BYTES;
    assert_eq!(client.welcome().credit_bytes, Some(floor));
    let window = client.credit_window().expect("the bridge sends within credit");
    assert!(window >= floor, "{window}");
    let mut events = client.events();
    let credit = || -> u64 {
        simulator
            .transcript()
            .iter()
            .filter(|line| line.from_host && line.text.contains("\"credit\""))
            .map(|line| serde_json::from_str::<Value>(&line.text).unwrap()["bytes"].as_u64().unwrap())
            .sum()
    };
    // The first grant, before anything is read: what tops the floor up to the buffer.
    for _ in 0..100 {
        if credit() >= window - floor {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(credit(), window - floor, "the floor topped up to the window");
    let first = credit();
    // Events of about 20 KB until a quarter of the window has been read.
    let quarter = window / 4;
    for i in 0..(quarter / 20_000 + 2) {
        simulator.emit("tracks", json!({"pad": "x".repeat(20_000), "i": i}));
        events.recv().await.unwrap();
    }
    let mut back = 0;
    for _ in 0..100 {
        back = credit() - first;
        if back >= quarter {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(back >= quarter, "granted back {back}: a quarter of the window ({quarter}) is read");
    // Never more than it has read (heartbeats are read too, and left out of the transcript).
    let read: u64 =
        simulator.transcript().iter().filter(|l| !l.from_host && l.text.contains("\"event\"")).map(|l| l.text.len() as u64 + 1).sum();
    assert!(back <= read + 4096, "granted back {back} of {read} read");
    assert!(client.max_unread_credit() <= window, "{} over the window {window}", client.max_unread_credit());
    // A bridge that doesn't ask for credit is sent none.
    let reaper = Simulator::start(SimulatorConfig::reaper(secret(), "/lab/REAPER")).await.unwrap();
    let plain = BridgeClient::connect(reaper.address(), secret(), CLIENT).await.unwrap();
    let mut plain_events = plain.events();
    for _ in 0..12 {
        reaper.emit("tracks", json!({"pad": "x".repeat(20_000)}));
        plain_events.recv().await.unwrap();
    }
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(reaper.transcript().iter().all(|line| !line.from_host || !line.text.contains("\"credit\"")));
}

/// These production-host Windows boundaries are selected by ci-os as well as Extended.
#[cfg(windows)]
mod restricted_renoise_staging {
    use super::*;
    use daw_bridge::{
        Daw,
        lifecycle::native_acl::{self, shapes},
        paths::BridgePaths,
    };
    use daw_mcp_server::host::{Host, ListeningMode};
    use std::os::windows::fs::OpenOptionsExt;
    use std::{
        path::Path,
        sync::{Arc, Mutex},
    };

    fn private_home() -> tempfile::TempDir {
        let home = tempfile::tempdir().unwrap();
        native_acl::create_kumis_folder(&home.path().join("bridges")).unwrap();
        native_acl::create_kumis_folder(&BridgePaths::new(home.path(), Daw::Renoise).root).unwrap();
        home
    }

    fn own_full(flags: u32) -> shapes::Ace {
        shapes::Ace { allow: true, flags, mask: shapes::FULL, whose: shapes::Whose::Me }
    }

    fn request() -> Value {
        json!({"sources": [{"id": "bounded", "what": "master"}], "range": {"fromBeat": 0, "beats": 1},
            "maxSeconds": 2, "epoch": "sim-1"})
    }

    struct Lab {
        host: Host,
        _simulator: Simulator,
        reply: Arc<Mutex<Value>>,
        fail_status: Arc<std::sync::atomic::AtomicBool>,
        // Dropped last, after the host releases its directory identity pins.
        home: tempfile::TempDir,
    }

    impl Lab {
        async fn start(home: tempfile::TempDir, mode: ListeningMode) -> Self {
            Self::start_with_state(home, mode, false).await
        }

        async fn start_with_state(home: tempfile::TempDir, mode: ListeningMode, complete: bool) -> Self {
            let paths = BridgePaths::new(home.path(), Daw::Renoise);
            let secret = daw_bridge::files::ensure_secret(&paths.secret()).unwrap();
            let reply = Arc::new(Mutex::new(Value::Null));
            let kept = reply.clone();
            let fail_status = Arc::new(std::sync::atomic::AtomicBool::new(false));
            let fail_next = fail_status.clone();
            let folder = paths.root.join("listening");
            let mut config = SimulatorConfig::renoise(secret, "/lab/Renoise");
            config.discovery_dir = Some(paths.discovery());
            config.respond = Some(Arc::new(move |method, params| match method {
                "render_start" | "audio_file" => {
                    std::fs::create_dir_all(&folder).unwrap();
                    let file = folder.join(format!("{}-bounded.wav", params["jobId"].as_str().unwrap()));
                    std::fs::write(&file, b"owned render boundary fixture").unwrap();
                    // Elevated CI's default token owner is Administrators; model normal
                    // current-user output without weakening the production owner check.
                    shapes::own_mine(&file);
                    let answer = json!({"jobId": params["jobId"], "state": if complete { "done" } else { "running" },
                        "files": [{"id": "bounded", "file": file}], "seconds": 0.5});
                    *kept.lock().unwrap() = answer.clone();
                    Some(Ok(answer))
                }
                "listen_status" | "listen_cancel" => {
                    if fail_next.swap(false, std::sync::atomic::Ordering::SeqCst) {
                        return Some(Err(("failed".into(), "A temporary status read failure.".into())));
                    }
                    let mut answer = kept.lock().unwrap().clone();
                    answer["state"] = "done".into();
                    Some(Ok(answer))
                }
                _ => None,
            }));
            let simulator = Simulator::start(config).await.unwrap();
            let host = Host::new(Daw::Renoise, home.path(), mode);
            Self { host, _simulator: simulator, reply, fail_status, home }
        }

        fn file(&self) -> PathBuf {
            PathBuf::from(self.reply.lock().unwrap()["files"][0]["file"].as_str().unwrap())
        }
    }

    #[test]
    fn restricted_windows_renoise_never_repairs_an_untrusted_root() {
        let home = private_home();
        let root = BridgePaths::new(home.path(), Daw::Renoise).root;
        shapes::write(
            &root,
            Some(&[
                own_full(shapes::INHERITABLE),
                shapes::Ace { allow: true, flags: shapes::INHERITABLE, mask: 0x0012_0089, whose: shapes::Whose::Sid("S-1-1-0") },
            ]),
            true,
        );
        let before = shapes::descriptor(&root);
        let runs = daw_bridge::owner::acl_runs_on_this_thread();
        let _host = Host::new(Daw::Renoise, home.path(), ListeningMode::Restricted);
        assert!(!root.join("listening").exists(), "no cache may be created beneath an unverified root");
        assert_eq!(shapes::descriptor(&root), before, "startup has no authority to repair its root");
        assert_eq!(daw_bridge::owner::acl_runs_on_this_thread(), runs);
    }

    #[test]
    fn restricted_windows_renoise_upgrades_only_owned_cache_and_pins_it() {
        let home = private_home();
        let root = BridgePaths::new(home.path(), Daw::Renoise).root;
        let folder = root.join("listening");
        std::fs::create_dir(&folder).unwrap();
        shapes::own_mine(&folder);
        shapes::write(
            &folder,
            Some(&[
                own_full(shapes::INHERITABLE),
                shapes::Ace { allow: true, flags: shapes::INHERITABLE, mask: shapes::FULL, whose: shapes::Whose::Sid("S-1-5-18") },
                shapes::Ace { allow: true, flags: shapes::INHERITABLE, mask: shapes::FULL, whose: shapes::Whose::Sid("S-1-5-32-544") },
            ]),
            false,
        );
        let old = folder.join("previous.wav");
        std::fs::write(&old, b"previous audio").unwrap();
        let old_folder = folder.join("older");
        std::fs::create_dir(&old_folder).unwrap();
        let nested_old = old_folder.join("previous.wav");
        std::fs::write(&nested_old, b"nested previous audio").unwrap();
        let neighbor = root.join("other-cache");
        std::fs::create_dir(&neighbor).unwrap();
        let before = [&root, &old, &old_folder, &nested_old, &neighbor].map(|path| shapes::descriptor(path));
        let runs = daw_bridge::owner::acl_runs_on_this_thread();
        let host = Host::new(Daw::Renoise, home.path(), ListeningMode::Restricted);
        assert!(native_acl::is_kumis_shape(&folder));
        assert_eq!(
            [&root, &old, &old_folder, &nested_old, &neighbor].map(|path| shapes::descriptor(path)),
            before,
            "startup changes only the exact cache directory, not old children, neighbors or its root"
        );
        assert_eq!(std::fs::read(&old).unwrap(), b"previous audio");
        assert!(std::fs::rename(&folder, root.join("replaced")).is_err(), "the host pins its approved folder identity");
        assert!(std::fs::rename(&root, root.with_file_name("renoise-replaced")).is_err(), "the owning root is pinned too");
        for path in [&root, &folder] {
            assert!(
                std::fs::OpenOptions::new().access_mode(2).share_mode(7).custom_flags(0x0220_0000).open(path).is_err(),
                "a pinned directory must exclude data writes that could replace its reparse metadata"
            );
        }
        assert_eq!(daw_bridge::owner::acl_runs_on_this_thread(), runs);
        drop(host);
        std::fs::rename(&folder, root.join("replaced")).unwrap();
    }

    #[tokio::test]
    async fn restricted_windows_renoise_refuses_foreign_or_reparse_cache_unchanged() {
        for foreign in [true, false] {
            let home = private_home();
            let root = BridgePaths::new(home.path(), Daw::Renoise).root;
            let folder = root.join("listening");
            let target = root.join("other-cache");
            native_acl::create_kumis_folder(&target).unwrap();
            if foreign {
                native_acl::create_kumis_folder(&folder).unwrap();
                if !shapes::own_foreign_user(&folder) {
                    assert!(std::env::var_os("CI").is_none(), "Windows CI must exercise a genuinely foreign owner");
                    continue;
                }
            } else if let Err(error) = std::os::windows::fs::symlink_dir(&target, &folder) {
                assert_eq!(error.raw_os_error(), Some(1314), "unexpected symlink fixture failure: {error}");
                continue; // This case needs Windows' symbolic-link privilege.
            }
            #[cfg(debug_assertions)]
            shapes::diagnose(&folder);
            let observed = if foreign { &folder } else { &target };
            let before = shapes::descriptor(observed);
            let lab = Lab::start(home, ListeningMode::Restricted).await;
            let runs = daw_bridge::owner::acl_runs_on_this_thread();
            let (answer, error) = lab.host.render(&request()).await;
            assert!(error && answer["code"] == "failed", "{answer}");
            assert!(lab.reply.lock().unwrap().is_null(), "unsafe startup must refuse before native render dispatch");
            assert_eq!(shapes::descriptor(observed), before);
            assert_eq!(daw_bridge::owner::acl_runs_on_this_thread(), runs);
            if foreign {
                shapes::own_mine(&folder);
            }
        }
    }

    #[tokio::test]
    async fn restricted_windows_renoise_protects_completed_owned_render_before_exposure() {
        for mode in [ListeningMode::Restricted, ListeningMode::Standalone] {
            let lab = Lab::start(private_home(), mode).await;
            let runs = daw_bridge::owner::acl_runs_on_this_thread();
            let (started, error) = lab.host.render(&request()).await;
            assert!(!error && started["state"] == "running", "{started}");
            assert_eq!(
                started.get("files").is_none(),
                mode == ListeningMode::Restricted,
                "only the restricted host hides planned, unprotected paths"
            );
            let file = lab.file();
            assert!(!native_acl::is_kumis_shape(&file), "the renderer initially inherits, not protects, its owner rule");
            let before = shapes::descriptor(&file);
            for cancel_reply in [false, true] {
                let args = json!({"jobId": started["jobId"]});
                let (done, error) = if cancel_reply { lab.host.listen_cancel(&args).await } else { lab.host.listen_job(&args).await };
                assert!(!error && done["state"] == "done", "{done}");
                assert_eq!(Path::new(done["files"][0]["file"].as_str().unwrap()), file.as_path());
                assert_eq!(std::fs::read(&file).unwrap(), b"owned render boundary fixture");
                assert_eq!(native_acl::is_kumis_shape(&file), mode == ListeningMode::Restricted);
            }
            if mode == ListeningMode::Standalone {
                assert_eq!(shapes::descriptor(&file), before);
            }
            if mode == ListeningMode::Restricted {
                assert_eq!(daw_bridge::owner::acl_runs_on_this_thread(), runs);
            }
        }
    }

    #[tokio::test]
    async fn restricted_windows_renoise_protects_immediately_completed_start() {
        for pattern in [false, true] {
            let lab = Lab::start_with_state(private_home(), ListeningMode::Restricted, true).await;
            let (done, error) =
                if pattern { lab.host.audio_file(&json!({"ref": "1:pattern:1"})).await } else { lab.host.render(&request()).await };
            assert!(!error && done["state"] == "done", "{done}");
            let path = Path::new(done["files"][0]["file"].as_str().unwrap());
            assert_eq!(path, lab.file().as_path());
            assert!(native_acl::is_kumis_shape(path), "even an immediately terminal start must pass the file boundary");
        }
    }

    #[tokio::test]
    async fn restricted_windows_renoise_refuses_broad_hardlinked_foreign_reparse_or_unowned_output() {
        for hazard in ["broad", "hardlink", "foreign", "reparse", "unowned"] {
            let lab = Lab::start(private_home(), ListeningMode::Restricted).await;
            let (started, error) = lab.host.render(&request()).await;
            assert!(!error, "{started}");
            let file = lab.file();
            let outside = if hazard == "reparse" { file.with_file_name("neighbor.wav") } else { lab.home.path().join("outside.wav") };
            std::fs::write(&outside, b"not this render").unwrap();
            shapes::own_mine(&outside);
            match hazard {
                "broad" => shapes::write(
                    &file,
                    Some(&[own_full(0), shapes::Ace { allow: true, flags: 0, mask: 0x0012_0089, whose: shapes::Whose::Sid("S-1-1-0") }]),
                    true,
                ),
                // Linking into the pinned cache needs a writable directory handle,
                // which the host intentionally excludes. A link outside it still
                // raises this exact returned file's link count and must be refused.
                "hardlink" => std::fs::hard_link(&file, lab.home.path().join("second-link.wav")).unwrap(),
                "foreign" => {
                    if !shapes::own_foreign_user(&file) {
                        assert!(std::env::var_os("CI").is_none(), "Windows CI must exercise a genuinely foreign owner");
                        continue;
                    }
                }
                "reparse" => {
                    std::fs::remove_file(&file).unwrap();
                    if let Err(error) = std::os::windows::fs::symlink_file(&outside, &file) {
                        assert_eq!(error.raw_os_error(), Some(1314), "unexpected symlink fixture failure: {error}");
                        continue;
                    }
                }
                "unowned" => lab.reply.lock().unwrap()["files"][0]["file"] = json!(outside),
                _ => unreachable!(),
            }
            let observed = if matches!(hazard, "reparse" | "unowned") { &outside } else { &file };
            #[cfg(debug_assertions)]
            shapes::diagnose(&file);
            let before = shapes::descriptor(observed);
            let runs = daw_bridge::owner::acl_runs_on_this_thread();
            let (done, error) = lab.host.listen_job(&json!({"jobId": started["jobId"]})).await;
            assert!(!error && done["state"] == "failed", "{hazard}: {done}");
            assert!(done.get("files").is_none(), "{hazard}: unsafe files are never exposed");
            assert!(!done["error"].as_str().unwrap().contains(&lab.home.path().to_string_lossy().to_string()));
            assert_eq!(shapes::descriptor(observed), before, "{hazard}: a refusal must not change the ACL or owner");
            assert_eq!(std::fs::read(&outside).unwrap(), b"not this render");
            assert_eq!(daw_bridge::owner::acl_runs_on_this_thread(), runs);
            if hazard == "foreign" {
                shapes::own_mine(&file);
            }
        }
    }

    #[tokio::test]
    async fn restricted_windows_renoise_refuses_an_output_with_an_open_writer() {
        let lab = Lab::start(private_home(), ListeningMode::Restricted).await;
        let (started, error) = lab.host.render(&request()).await;
        assert!(!error, "{started}");
        let file = lab.file();
        let writer = std::fs::OpenOptions::new().write(true).share_mode(7).open(&file).unwrap();
        let before = shapes::descriptor(&file);
        let (done, error) = lab.host.listen_job(&json!({"jobId": started["jobId"]})).await;
        assert!(!error && done["state"] == "failed" && done.get("files").is_none(), "{done}");
        assert_eq!(shapes::descriptor(&file), before, "the exclusive file pin must precede any ACL change");
        drop(writer);
        let (later, error) = lab.host.listen_job(&json!({"jobId": started["jobId"]})).await;
        assert!(!error && later["state"] == "done" && native_acl::is_kumis_shape(&file), "{later}");
    }

    #[tokio::test]
    async fn restricted_windows_renoise_keeps_owned_paths_across_a_temporary_status_failure() {
        let lab = Lab::start(private_home(), ListeningMode::Restricted).await;
        let (started, error) = lab.host.render(&request()).await;
        assert!(!error, "{started}");
        lab.fail_status.store(true, std::sync::atomic::Ordering::SeqCst);
        let (failed, error) = lab.host.listen_job(&json!({"jobId": started["jobId"]})).await;
        assert!(error && failed.get("files").is_none(), "{failed}");
        assert!(!native_acl::is_kumis_shape(&lab.file()));
        let (later, error) = lab.host.listen_job(&json!({"jobId": started["jobId"]})).await;
        assert!(!error && later["state"] == "done" && later["jobId"] == started["jobId"], "{later}");
        assert_eq!(Path::new(later["files"][0]["file"].as_str().unwrap()), lab.file().as_path());
        assert!(native_acl::is_kumis_shape(&lab.file()), "a later terminal observation can still authorize exact cleanup");
    }

    #[tokio::test]
    async fn restricted_windows_renoise_blocks_swaps_through_pinned_file_protection() {
        let lab = Lab::start(private_home(), ListeningMode::Restricted).await;
        let (started, error) = lab.host.render(&request()).await;
        assert!(!error, "{started}");
        let file = lab.file();
        let other = file.with_file_name("swapped.wav");
        let attempts = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let observed = attempts.clone();
        let expected = file.clone();
        let swapped = other.clone();
        let _probe = shapes::probe_next_render(move |pinned, protected| {
            // This is the host's actual open, before the first ACL check and after
            // protection/read-back, not a test-created handle with copied flags.
            assert_eq!(native_acl::file_path(pinned).unwrap(), std::fs::canonicalize(&expected).unwrap());
            assert_eq!(native_acl::is_kumis_shape_handle(pinned, false), protected);
            assert!(std::fs::rename(&expected, &swapped).is_err());
            assert!(std::fs::write(&expected, b"replacement").is_err());
            assert!(std::fs::OpenOptions::new().append(true).share_mode(7).open(&expected).is_err());
            assert!(std::fs::remove_file(&expected).is_err());
            assert_eq!(std::fs::read(&expected).unwrap(), b"owned render boundary fixture");
            observed.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        });
        let (done, error) = lab.host.listen_job(&json!({"jobId": started["jobId"]})).await;
        assert!(!error && done["state"] == "done", "{done}");
        assert_eq!(attempts.load(std::sync::atomic::Ordering::SeqCst), 2, "both ends of the real pinned interval were exercised");
        assert_eq!(Path::new(done["files"][0]["file"].as_str().unwrap()), file.as_path());
        assert!(native_acl::is_kumis_shape(&file));
        assert_eq!(std::fs::read(&file).unwrap(), b"owned render boundary fixture");
        // The file pin ended, but the host still pins the cache directory. Ordinary
        // owner writes/deletion work; a rename would also need a cache-directory writer.
        std::fs::write(&file, b"owner changed the exposed file").unwrap();
        std::fs::remove_file(&file).unwrap();
        assert!(!file.exists());
    }
}
