//! The wire of the read methods (`read`, `snapshot`, `focus`, `focus_body`) between the host's client and a bridge that
//! answers like the Renoise tool's, as a golden transcript (tests/golden/renoise-reads-bridge.ndjson), and what the host's
//! client does as it reads pages bigger than the bridge's credit.
mod common;

use common::{golden, normalized};
use daw_bridge::auth::Secret;
use daw_mcp_server::client::{BridgeClient, RequestError};
use daw_mcp_server::simulator::renoise::RenoiseScript;
use daw_mcp_server::simulator::Simulator;
use serde_json::{json, Value};
use std::time::Duration;

const WAIT: Duration = Duration::from_secs(5);
const FRAME_BOUND: usize = 256 * 1024;

fn secret() -> Secret {
    Secret::new("simulated-secret-0123456789-abcdefghijklmnop").unwrap()
}

#[tokio::test]
async fn the_read_methods_on_the_wire() {
    let secret = secret();
    let script = RenoiseScript::new();
    let simulator = Simulator::start(script.config(secret.clone(), "/lab/Renoise")).await.unwrap();
    let client = BridgeClient::connect(simulator.address(), secret, "daw-mcp-server/test").await.unwrap();
    let mut methods = vec!["ping", "status", "read", "snapshot", "focus", "focus_body", "show_status", "digest"];
    methods.extend(daw_mcp_server::simulator::reaper::LISTENING_METHODS);
    assert_eq!(client.welcome().methods, methods);
    assert_eq!(client.welcome().credit_bytes, Some(64 * 1024), "the tool starts with a floor of credit");

    let song = client.request("read", json!({"kind": "song"}), WAIT).await.unwrap();
    assert_eq!(song["items"][0]["name"], "Demo");
    let page = client.request("read", json!({"kind": "track", "limit": 2}), WAIT).await.unwrap();
    assert_eq!(page["items"].as_array().unwrap().len(), 2);
    let rest = client.request("read", json!({"kind": "track", "limit": 2, "cursor": page["nextCursor"]}), WAIT).await.unwrap();
    assert_eq!(rest["items"].as_array().unwrap().len(), 2);
    let one = client.request("read", json!({"ref": "1:track:3"}), WAIT).await.unwrap();
    assert_eq!(one["items"][0]["name"], "Drums");
    let lines = client.request("read", json!({"kind": "line", "parent": "1:pattern:1", "track": 1}), WAIT).await.unwrap();
    assert_eq!(lines["items"][1]["notes"][0]["note"], "D#4");
    let snapshot = client.request("snapshot", json!({}), WAIT).await.unwrap();
    assert_eq!(snapshot["tracks"].as_array().unwrap().len(), 7);
    let focus = client.request("focus", json!({}), WAIT).await.unwrap();
    assert_eq!(focus["selection"]["trackRef"], "1:track:1");
    for (kind, reference) in
        [("matrix", None), ("phrase", Some("1:phrase:1.1")), ("instrument", Some("1:instrument:1")), ("dsp", Some("1:track:1"))]
    {
        let mut params = json!({"kind": kind});
        if let Some(reference) = reference {
            params["ref"] = reference.into();
        }
        client.request("focus_body", params, WAIT).await.unwrap();
    }
    client.request("focus_body", json!({"kind": "pattern", "window": {"from": 3, "to": 6}}), WAIT).await.unwrap();

    // What the tool refuses, in its words and codes.
    for (params, code) in [
        (json!({"kind": "nonsense"}), "bad-params"),
        (json!({"ref": "0:track:1"}), "stale"),
        (json!({"kind": "track", "cursor": "0.2"}), "stale"),
    ] {
        let error = client.request("read", params, WAIT).await.unwrap_err();
        assert!(matches!(&error, RequestError::Bridge { code: got, .. } if got == code), "{error:?}");
    }
    assert_eq!(script.asked().len(), 15);

    golden("renoise-reads-bridge.ndjson", &normalized(&simulator.transcript()));
    simulator.stop();
}

/// A pattern of 300 full lines is about 480 KB, and each page most of a frame. The host asked for a receive buffer of only
/// 64 KiB, so the bridge could send no more than that ahead of the host's reading, and the pages arrive only because the
/// host gives credit back as it reads, partial lines too.
#[tokio::test]
async fn pages_bigger_than_the_credit_window_arrive_as_the_host_reads() {
    let secret = secret();
    let script = RenoiseScript::with_full_pattern(300);
    let simulator = Simulator::start(script.config(secret.clone(), "/lab/Renoise")).await.unwrap();
    let client = BridgeClient::connect_with(simulator.address(), secret, "daw-mcp-server/test", WAIT, 64 * 1024).await.unwrap();
    let window = client.credit_window().unwrap();

    let mut seen = Vec::new();
    let mut cursor: Option<Value> = None;
    let mut pages = 0;
    loop {
        let mut params = json!({"kind": "line", "parent": "1:pattern:1", "track": 1, "limit": 200});
        if let Some(cursor) = &cursor {
            params["cursor"] = cursor.clone();
        }
        let page = client.request("read", params, WAIT).await.unwrap();
        assert!(page.to_string().len() <= FRAME_BOUND, "page {pages} is {} bytes", page.to_string().len());
        seen.extend(page["items"].as_array().unwrap().iter().map(|row| row["line"].as_u64().unwrap()));
        pages += 1;
        match page.get("nextCursor") {
            Some(next) => cursor = Some(next.clone()),
            None => break,
        }
    }
    assert!(pages >= 2, "{pages} pages");
    assert_eq!(seen, (1..=300).collect::<Vec<u64>>(), "every line once, in order");
    let granted: u64 = simulator
        .transcript()
        .iter()
        .filter(|line| line.from_host && line.text.contains("\"type\":\"credit\""))
        .map(|line| serde_json::from_str::<Value>(&line.text).unwrap()["bytes"].as_u64().unwrap())
        .sum();
    assert!(granted > 300_000, "the pages went out on {granted} bytes of credit, past a window of {window}");
    assert!(client.max_unread_credit() <= window);
    simulator.stop();
}

/// Another song is opened: the epoch moves, refs and cursors from before are refused as stale, and the answers carry the
/// new epoch, which reads the same.
#[tokio::test]
async fn a_song_switched_retires_the_refs_and_cursors_from_before() {
    let secret = secret();
    let script = RenoiseScript::new();
    let simulator = Simulator::start(script.config(secret.clone(), "/lab/Renoise")).await.unwrap();
    let client = BridgeClient::connect(simulator.address(), secret, "daw-mcp-server/test").await.unwrap();
    let page = client.request("read", json!({"kind": "track", "limit": 2}), WAIT).await.unwrap();
    assert_eq!(page["epoch"], "1");
    script.switch_song();
    let stale = |error: RequestError| matches!(error, RequestError::Bridge { code, .. } if code == "stale");
    assert!(stale(client.request("read", json!({"kind": "track", "limit": 2, "cursor": page["nextCursor"]}), WAIT).await.unwrap_err()));
    assert!(stale(client.request("read", json!({"ref": "1:track:3"}), WAIT).await.unwrap_err()));
    assert!(stale(client.request("focus_body", json!({"kind": "dsp", "ref": "1:track:1"}), WAIT).await.unwrap_err()));
    // The new epoch is in every answer, and what is asked with it reads as before.
    let again = client.request("read", json!({"kind": "track", "limit": 2}), WAIT).await.unwrap();
    assert_eq!(again["epoch"], "2");
    assert_eq!(again["items"][0]["ref"], "2:track:1");
    assert_eq!(again["nextCursor"], "2.0.3");
    let rest = client.request("read", json!({"kind": "track", "limit": 2, "cursor": again["nextCursor"]}), WAIT).await.unwrap();
    assert_eq!(rest["items"][0]["ref"], "2:track:3");
    let one = client.request("read", json!({"ref": "2:track:3"}), WAIT).await.unwrap();
    assert_eq!(one["items"][0]["name"], "Drums");
    assert_eq!(one["items"][0]["groupRef"], "2:track:5");
    let focus = client.request("focus", json!({}), WAIT).await.unwrap();
    assert_eq!(focus["epoch"], "2");
    assert_eq!(focus["selection"]["trackRef"], "2:track:1");
    simulator.stop();
}
