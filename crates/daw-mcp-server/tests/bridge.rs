//! The host's bridge client against simulated REAPER, Renoise and Logic bridges: the handshake, requests, events, busy
//! and quitting, with golden NDJSON transcripts of the wire (tests/golden/*-bridge.ndjson; KUMI_BLESS=1 rewrites them).
mod common;

use common::{golden, normalized};
use daw_bridge::auth::Secret;
use daw_bridge::wire::Project;
use daw_mcp_server::client::{BridgeClient, ConnectError, RequestError};
use daw_mcp_server::simulator::{Simulator, SimulatorConfig};
use serde_json::{json, Value};
use std::time::Duration;

const CLIENT: &str = "daw-mcp-server/test";
const WAIT: Duration = Duration::from_secs(5);

fn secret() -> Secret {
    Secret::new("simulated-secret-0123456789-abcdefghijklmnop").unwrap()
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

/// Logic's identity is the welcome's (Logic's pid, its bundle id as the resource, its version); its status is the
/// project, `busy` (false, or `away` when Logic's windows are on another Space) and `permission` for what the bridge
/// lacks.
#[tokio::test]
async fn logic_session_transcript() {
    let simulator = Simulator::start(SimulatorConfig::logic(secret(), "com.apple.logic10")).await.unwrap();
    let client = BridgeClient::connect(simulator.address(), secret(), CLIENT).await.unwrap();
    let identity = &client.welcome().identity;
    assert_eq!(
        (identity.daw.as_str(), identity.daw_version.as_str(), identity.resource.as_str()),
        ("logic", "12.3.1", "com.apple.logic10")
    );
    // The status is the recorded one: the bridge's answer over the real Logic, replayed by the simulator.
    let status = client.request("status", json!({}), WAIT).await.unwrap();
    let vectors: serde_json::Value = serde_json::from_str(include_str!("vectors/logic-reads.json")).unwrap();
    let recorded = vectors["cases"]
        .as_array()
        .unwrap()
        .iter()
        .find(|case| case["request"]["method"] == "status")
        .map(|case| case["response"]["result"].clone())
        .expect("the vectors record a status");
    assert_eq!(status, recorded);
    assert_eq!((status["busy"].clone(), status["permission"].clone()), (json!(false), json!(null)));
    // Without Accessibility the status says so, and reads fail in words, never busy.
    simulator.set_permission(Some("accessibility"));
    assert_eq!(client.request("status", json!({}), WAIT).await.unwrap()["permission"], "accessibility");
    let refused = client.request("read", json!({"kind": "track"}), WAIT).await.unwrap_err();
    assert!(
        matches!(&refused, RequestError::Bridge { code, message } if code == "failed" && message.contains("Accessibility")),
        "{refused:?}"
    );
    simulator.set_permission(None);
    // Logic's windows on another Space: away, which is busy.
    simulator.set_busy(Some("away"));
    assert_eq!(client.request("status", json!({}), WAIT).await.unwrap()["busy"], "away");
    assert!(
        matches!(client.request("read", json!({"kind": "track"}), WAIT).await, Err(RequestError::Bridge { code, .. }) if code == "busy")
    );
    simulator.set_busy(None);
    client.close().await;
    session(&simulator).await;
    golden("logic-bridge.ndjson", &normalized(&simulator.transcript()));
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
