//! A host whose receive buffer is small, and which stops reading in the middle of an answer. The bridge (the Renoise
//! tool) must be held back by credit and never sent into a full socket, no line is torn, nothing is retired, and the
//! answer is whole once the host reads again.
#[path = "../common/mod.rs"]
mod common;

use daw_bridge::auth::Secret;
use daw_mcp_server::client::BridgeClient;
use daw_mcp_server::simulator::renoise::RenoiseScript;
use daw_mcp_server::simulator::Simulator;
use serde_json::{json, Value};
use std::time::Duration;

const WAIT: Duration = Duration::from_secs(10);

#[tokio::test]
async fn a_host_with_a_small_receive_buffer_that_pauses_mid_answer_holds_the_bridge_back_and_loses_nothing() {
    let secret = Secret::new("simulated-secret-0123456789-abcdefghijklmnop").unwrap();
    let script = RenoiseScript::with_full_pattern(300);
    let simulator = Simulator::start(script.config(secret.clone(), "/lab/Renoise")).await.unwrap();
    let client = BridgeClient::connect_with(simulator.address(), secret, "daw-mcp-server/test", WAIT, 64 * 1024).await.unwrap();
    let window = client.credit_window().unwrap();
    // The system may give more than was asked for, but not the 1 MiB a default host has.
    assert!(window <= 512 * 1024, "{window}");

    // The host stops reading with a page of about 250 KB on its way.
    client.pause_reading(true);
    let reader = client.clone();
    let page = tokio::spawn(async move {
        reader.request("read", json!({"kind": "line", "parent": "1:pattern:1", "track": 1, "limit": 200}), WAIT).await
    });
    tokio::time::sleep(Duration::from_millis(700)).await;
    let flow = simulator.flow();
    assert!(flow.sent > 0, "something was sent");
    assert!(flow.sent <= window, "{} bytes sent into a host that granted a window of {window}", flow.sent);
    assert!(flow.waiting > 100_000, "the rest of the page waits for credit: {}", flow.waiting);
    assert!(!client.is_closed(), "a host that pauses is not retired");

    // It reads again: the page arrives whole, and so do the rest.
    client.pause_reading(false);
    let mut lines = Vec::new();
    let mut answer = page.await.unwrap().unwrap();
    loop {
        lines.extend(answer["items"].as_array().unwrap().iter().map(|row| row["line"].as_u64().unwrap()));
        match answer.get("nextCursor").cloned() {
            Some(cursor) => {
                answer = client
                    .request("read", json!({"kind": "line", "parent": "1:pattern:1", "track": 1, "limit": 200, "cursor": cursor}), WAIT)
                    .await
                    .unwrap();
            }
            None => break,
        }
    }
    assert_eq!(lines, (1..=300).collect::<Vec<u64>>());
    assert!(!client.is_closed());
    assert!(client.max_unread_credit() <= window, "{} over {window}", client.max_unread_credit());
    let _: Value = client.request("ping", json!({}), WAIT).await.unwrap();
    simulator.stop();
}
