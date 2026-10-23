//! A host held back by credit that the bridge retires, a connection the bridge lost track of, and a grant that could not be
//! written in time: none of them may leave the host waiting for ever.
mod common;

use daw_bridge::auth::Secret;
use daw_bridge::files::ensure_secret;
use daw_bridge::paths::BridgePaths;
use daw_bridge::Daw;
use daw_mcp_server::client::BridgeClient;
use daw_mcp_server::host::{CallError, Host, Pick};
use daw_mcp_server::simulator::renoise::RenoiseScript;
use daw_mcp_server::simulator::Simulator;
use serde_json::json;
use std::time::Duration;

const WAIT: Duration = Duration::from_secs(10);

fn secret() -> Secret {
    Secret::new("simulated-secret-0123456789-abcdefghijklmnop").unwrap()
}

fn big_page() -> serde_json::Value {
    json!({"kind": "line", "parent": "1:pattern:1", "track": 1, "limit": 200})
}

/// The bridge retires a host that paused with an answer held for credit: when the host reads again it gets the line whole,
/// then the bye, and the connection ends.
#[tokio::test]
async fn a_retired_host_is_sent_the_line_it_was_part_way_through_and_then_the_bye() {
    let secret = secret();
    let script = RenoiseScript::with_full_pattern(300);
    let simulator = Simulator::start(script.config(secret.clone(), "/lab/Renoise")).await.unwrap();
    let client = BridgeClient::connect_with(simulator.address(), secret, "daw-mcp-server/test", WAIT, 64 * 1024).await.unwrap();
    client.pause_reading(true);
    let reader = client.clone();
    let page = tokio::spawn(async move { reader.request("read", big_page(), WAIT).await });
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(simulator.flow().waiting > 100_000, "the page is held for credit");
    simulator.retire_with_bye();
    tokio::time::sleep(Duration::from_millis(200)).await;
    client.pause_reading(false);
    let answer = page.await.unwrap().expect("the line it was part way through arrives whole");
    assert_eq!(answer["items"][0]["line"], 1);
    for _ in 0..100 {
        if client.is_closed() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(client.is_closed(), "and then it is told, and the connection ends");
    let last = simulator.transcript().into_iter().rfind(|line| !line.from_host).unwrap();
    assert!(last.text.contains("\"type\":\"bye\""), "{}", last.text);
    simulator.stop();
}

/// A host and a simulated Renoise it finds by discovery; `heartbeat_ms` is short so silence is told quickly.
async fn lab(heartbeat_ms: u64) -> (tempfile::TempDir, Host, Simulator) {
    let home = tempfile::tempdir().unwrap();
    let paths = BridgePaths::new(home.path(), Daw::Renoise);
    let secret = ensure_secret(&paths.secret()).unwrap();
    let mut config = RenoiseScript::new().config(secret, "/lab/Renoise");
    config.discovery_dir = Some(paths.discovery());
    config.offer.heartbeat_ms = heartbeat_ms;
    let simulator = Simulator::start(config).await.unwrap();
    let host = Host::new(Daw::Renoise, home.path());
    (home, host, simulator)
}

fn anyone() -> Pick {
    Pick { pid: None, resource: None }
}

fn connections(simulator: &Simulator) -> usize {
    simulator.transcript().iter().map(|line| line.connection).max().unwrap_or(0)
}

/// A connection the bridge let go of without a word answers nothing for ever and heartbeats nothing, while the DAW
/// answers a new one. The host sees that, replaces the connection, and the call it was making is asked again on the new one.
#[tokio::test]
async fn a_connection_the_bridge_lost_track_of_is_replaced_by_a_fresh_one_that_answers() {
    let (_home, host, simulator) = lab(100).await;
    host.call("snapshot", json!({}), &anyone(), WAIT).await.unwrap();
    assert_eq!(connections(&simulator), 1);
    simulator.go_silent();
    let answer = host.call("snapshot", json!({}), &anyone(), Duration::from_millis(600)).await;
    assert!(answer.is_ok(), "asked again on the new connection: {answer:?}");
    assert_eq!(connections(&simulator), 2, "a fresh handshake, and the old connection let go");
    host.call("snapshot", json!({}), &anyone(), WAIT).await.expect("and the next call uses it");
    assert_eq!(connections(&simulator), 2);
    simulator.stop();
}

/// A write on a connection the bridge lost track of may have been made: the connection is replaced, but the write is never
/// sent again (that could make it twice); its caller is told it timed out, which the journal keeps as uncertain.
#[tokio::test]
async fn a_write_on_a_lost_connection_is_never_sent_again_on_the_fresh_one() {
    let (_home, host, simulator) = lab(100).await;
    host.call("snapshot", json!({}), &anyone(), WAIT).await.unwrap();
    simulator.go_silent();
    let params = json!({"kind": "track-volume", "input": {}, "fence": {}, "desc": "Kumi: Kick volume −6 dB (k0000)", "epoch": "1"});
    let answer = host.call("change_apply", params, &anyone(), Duration::from_millis(600)).await;
    assert!(matches!(answer, Err(CallError::Timeout)), "{answer:?}");
    let sent = simulator.transcript().iter().filter(|line| line.from_host && line.text.contains("\"change_apply\"")).count();
    assert_eq!(sent, 1, "sent once, on the lost connection only");
    simulator.stop();
}

/// Calls still waiting on a connection the host lets go of for a fresh one were made of a DAW that is still there: a read
/// among them is asked again on the fresh connection, and a write is told it timed out (uncertain), never that the DAW
/// closed. Told it closed, Kumi said Logic Pro had quit while it was importing.
#[tokio::test]
async fn calls_waiting_on_a_connection_the_host_let_go_of_are_never_told_the_daw_closed() {
    let (_home, host, simulator) = lab(100).await;
    host.call("snapshot", json!({}), &anyone(), WAIT).await.unwrap();
    simulator.go_silent();
    let pick = anyone();
    let params = json!({"kind": "track-volume", "input": {}, "fence": {}, "desc": "Kumi: Kick volume −6 dB (k0000)", "epoch": "1"});
    let (read, write, replacing) =
        tokio::join!(host.call("snapshot", json!({}), &pick, WAIT), host.call("change_apply", params, &pick, WAIT), async {
            // Both are waiting on the old connection when this one's timeout lets it go.
            tokio::time::sleep(Duration::from_millis(100)).await;
            host.call("snapshot", json!({}), &pick, Duration::from_millis(600)).await
        },);
    assert!(replacing.is_ok(), "asked again on the new connection: {replacing:?}");
    assert_eq!(connections(&simulator), 2, "a fresh handshake, and the old connection let go");
    assert!(read.is_ok(), "the read waiting on the old connection is asked again on the new one: {read:?}");
    assert_eq!(write, Err(CallError::Timeout), "the write may have been made: uncertain, not closed");
    let sent = simulator.transcript().iter().filter(|line| line.from_host && line.text.contains("\"change_apply\"")).count();
    assert_eq!(sent, 1, "sent once, on the old connection only");
    simulator.stop();
}

/// A frozen DAW (a plug-in loading, a long script) answers neither the old connection nor a new handshake: that is busy, not
/// a lost connection. Nothing is dropped, and when it thaws the same connection answers.
#[tokio::test]
async fn a_frozen_bridge_is_busy_and_keeps_its_connection() {
    let (_home, host, simulator) = lab(100).await;
    host.call("snapshot", json!({}), &anyone(), WAIT).await.unwrap();
    simulator.freeze();
    let error = host.call("snapshot", json!({}), &anyone(), Duration::from_millis(600)).await.unwrap_err();
    assert_eq!(error, CallError::Timeout, "told the DAW is busy, not that the connection was lost");
    let said = host.call_error(&error);
    assert_eq!((said["busy"].clone(), said["connected"].clone()), (json!(true), json!(true)), "{said}");
    simulator.thaw();
    host.call("snapshot", json!({}), &anyone(), WAIT).await.expect("the old connection answers again");
    assert_eq!(connections(&simulator), 2, "only the handshake that went unanswered was made besides the first");
    simulator.stop();
}

/// A Renoise behind a dialog is silent too (no heartbeats), but its socket still answers a ping: the connection is kept and
/// the call is told the DAW is busy.
#[tokio::test]
async fn a_bridge_behind_a_dialog_that_still_answers_a_ping_keeps_its_connection() {
    let (_home, host, simulator) = lab(100).await;
    host.call("snapshot", json!({}), &anyone(), WAIT).await.unwrap();
    simulator.set_busy(Some("showing a dialog"));
    simulator.hold("snapshot");
    let error = host.call("snapshot", json!({}), &anyone(), Duration::from_millis(600)).await.unwrap_err();
    assert_eq!(error, CallError::Timeout);
    assert_eq!(host.call_error(&error)["busy"], true);
    simulator.set_busy(None);
    simulator.release();
    host.call("snapshot", json!({}), &anyone(), WAIT).await.unwrap();
    assert_eq!(connections(&simulator), 1, "the same connection throughout");
    simulator.stop();
}

/// A grant that cannot be written for longer than its time is not lost: the answer, which needs several windows, arrives.
#[tokio::test]
async fn a_grant_that_waits_too_long_for_the_writer_is_written_when_it_can_be() {
    let secret = secret();
    let script = RenoiseScript::with_full_pattern(300);
    let simulator = Simulator::start(script.config(secret.clone(), "/lab/Renoise")).await.unwrap();
    let client = BridgeClient::connect_with(simulator.address(), secret, "daw-mcp-server/test", WAIT, 64 * 1024).await.unwrap();
    let reader = client.clone();
    let page = tokio::spawn(async move { reader.request("read", big_page(), WAIT).await });
    // The writer is held past a grant's two seconds, with the answer under way.
    tokio::time::sleep(Duration::from_millis(50)).await;
    let holder = client.clone();
    let held = tokio::spawn(async move { holder.hold_writer(Duration::from_millis(2600)).await });
    let answer = tokio::time::timeout(Duration::from_secs(20), page).await.expect("the answer arrives").unwrap().unwrap();
    assert!(answer["items"].as_array().unwrap().len() > 100);
    held.await.unwrap();
    assert_eq!(client.grant_owed(), 0, "nothing is left owed");
    simulator.stop();
}
