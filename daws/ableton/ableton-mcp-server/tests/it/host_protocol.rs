use ableton_mcp_server::{
    host::{McpHost, McpHostOptions, RequestDecision},
    live::*,
};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{cell::RefCell, rc::Rc};
/// An adapter that reports the status it holds, which a test may change without telling anyone.
struct StatusAdapter(RefCell<Result<LiveStatus, LiveError>>);
impl LiveAdapter for StatusAdapter {
    fn status(&self) -> Result<LiveStatus, LiveError> {
        self.0.borrow().clone()
    }
    fn snapshot(&self) -> Result<LiveSnapshot, LiveError> {
        UnavailableLiveAdapter.snapshot()
    }
    fn get(&self, r: &LiveRef) -> Result<Option<Value>, LiveError> {
        UnavailableLiveAdapter.get(r)
    }
    fn invoke(&self, i: &LiveInvocation) -> Result<Value, LiveError> {
        UnavailableLiveAdapter.invoke(i)
    }
    fn subscribe(&self, l: LiveListener) -> Result<Unsubscribe, LiveError> {
        UnavailableLiveAdapter.subscribe(l)
    }
    fn reconnect(&self) -> Result<LiveStatus, LiveError> {
        self.status()
    }
}
#[async_trait::async_trait(?Send)]
impl AsyncLiveAdapter for StatusAdapter {
    async fn snapshot_async(&self, _: Option<&LiveOperationContext>, _: Option<&LiveSnapshotRequest>) -> Result<LiveSnapshot, LiveError> {
        self.snapshot()
    }
    async fn discover_async(&self, r: &LiveDiscoveryRequest, c: Option<&LiveOperationContext>) -> Result<LiveDiscoveryResult, LiveError> {
        UnavailableLiveAdapter.discover_async(r, c).await
    }
    async fn get_async(&self, r: &LiveRef, _: Option<&LiveOperationContext>) -> Result<Option<Value>, LiveError> {
        self.get(r)
    }
    async fn invoke_async(&self, i: &LiveInvocation, _: Option<&LiveOperationContext>) -> Result<Value, LiveError> {
        self.invoke(i)
    }
    async fn reconnect_async(&self, _: Option<&LiveOperationContext>) -> Result<LiveStatus, LiveError> {
        self.reconnect()
    }
    async fn close(&self) -> Result<(), LiveError> {
        Ok(())
    }
}
/// The bridge version the golden files were recorded with; pinned so a version bump changes none of them.
const ORACLE_VERSION: &str = "1.0.74";
fn initialize(host: &McpHost) {
    host.begin_request(&json!({"jsonrpc":"2.0","id":"setup","method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"test","version":"1"}}}),false).unwrap().completed();
    host.begin_request(&json!({"jsonrpc":"2.0","method":"notifications/initialized"}), false).unwrap().completed();
}
fn modern(method: &str, params: Value) -> Value {
    let mut params = params;
    params["_meta"] = json!({"io.modelcontextprotocol/protocolVersion":"2026-07-28","io.modelcontextprotocol/clientCapabilities":{}});
    json!({"jsonrpc":"2.0","id":1,"method":method,"params":params})
}
#[tokio::test]
async fn host_lifecycle_resources_status_and_gates_match_source() {
    let data: Value = serde_json::from_str(&kumi_testkit::oracle("ableton-mcp-server/host-protocol-oracle.json")).unwrap();
    for case in data["cases"].as_array().unwrap() {
        let status = if case["statusError"] == true {
            Err(LiveError::error("broken adapter"))
        } else if let Some(status) = case.get("status") {
            serde_json::from_value(status.clone()).map_err(|_| LiveError::error("invalid adapter status"))
        } else {
            UnavailableLiveAdapter.status()
        };
        let host = Rc::new(
            McpHost::new(
                Rc::new(StatusAdapter(RefCell::new(status))),
                McpHostOptions {
                    tool_policy: case.get("policy").cloned(),
                    server_version: Some(ORACLE_VERSION.into()),
                    ..Default::default()
                },
            )
            .unwrap(),
        );
        for (index, step) in case["steps"].as_array().unwrap().iter().enumerate() {
            let request =
                if case["async"] == true { host.handle_async(&step["request"], None).await } else { host.handle(&step["request"]) };
            if let Some(error) = step.get("error") {
                assert_eq!(
                    request.err().expect("source throws").message(),
                    error.as_str().unwrap(),
                    "{} step {index}: {}",
                    case["name"],
                    step["request"]
                );
                continue;
            }
            let result = request.unwrap_or_else(|e| panic!("{} step {index}: {e}", case["name"]));
            let mut result = result.unwrap_or(Value::Null);
            if step["normalizeStatus"] == true {
                let status: Value = serde_json::from_str(result["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
                result["result"]["content"][0]["text"] =
                    json!(ableton_mcp_server::host::helpers::canonical_mutation_identity(&status).unwrap());
            }
            let text = kumi_common::js::json::stringify(&result);
            let hash = hex::encode(Sha256::digest(text.as_bytes()));
            assert_eq!(
                hash,
                step["sha256"].as_str().unwrap(),
                "{} step {index}: {}\nexpected {}\ngot {text}",
                case["name"],
                step["request"],
                step.get("result").unwrap_or(&Value::Null)
            );
        }
    }
}
// The tools a request may call follow Live's status as the adapter reports it now, even a change no listener was told
// of, and the policy as it was last set.
#[test]
fn the_tools_a_request_may_call_follow_a_status_change_no_listener_heard_of() {
    let connected = DeterministicLiveSimulator::new().status().unwrap();
    let mut away = connected.clone();
    away.connected = false;
    let adapter = Rc::new(StatusAdapter(RefCell::new(Ok(connected.clone()))));
    let host = McpHost::new(adapter.clone(), McpHostOptions::default()).unwrap();
    assert!(host.tool_callable("live_discover").unwrap());
    *adapter.0.borrow_mut() = Ok(away);
    assert!(!host.tool_callable("live_discover").unwrap());
    assert!(host.tool_callable("server_status").unwrap());
    *adapter.0.borrow_mut() = Err(LiveError::error("broken adapter"));
    assert!(!host.tool_callable("live_discover").unwrap());
    *adapter.0.borrow_mut() = Ok(connected);
    assert!(host.tool_callable("live_discover").unwrap());
    host.set_tool_policy(&json!({"profile":"full","deny":["live_discover"]})).unwrap();
    assert!(!host.tool_callable("live_discover").unwrap());
    assert!(!host.policy_allows_tool(Some("live_discover")).unwrap());
    assert!(host.policy_allows_tool(Some("live_snapshot")).unwrap());
    assert!(!host.tool_callable("no_such_tool").unwrap());
}

// A status change no listener was told of is announced by the next request that reads the status.
#[tokio::test]
async fn a_status_change_no_listener_heard_of_is_announced_at_the_next_request() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let connected = DeterministicLiveSimulator::new().status().unwrap();
            let adapter = Rc::new(StatusAdapter(RefCell::new(Ok(connected.clone()))));
            let host = Rc::new(McpHost::new(adapter.clone(), McpHostOptions::default()).unwrap());
            initialize(&host);
            let lines = Rc::new(RefCell::new(Vec::new()));
            let output = lines.clone();
            host.set_event_emitter(Rc::new(move |line| {
                output.borrow_mut().push(line);
                Box::pin(async { Ok(()) })
            }))
            .unwrap();
            let call = |id: i64| json!({"jsonrpc":"2.0","id":id,"method":"tools/call","params":{"name":"live_status","arguments":{}}});
            host.begin_request(&call(1), true).unwrap();
            host.begin_request(&call(2), true).unwrap();
            tokio::task::yield_now().await;
            assert!(lines.borrow().is_empty(), "the same status announces nothing");
            let mut moved = connected.clone();
            moved.epoch = Some(connected.epoch.unwrap() + 1);
            *adapter.0.borrow_mut() = Ok(moved);
            host.begin_request(&call(3), true).unwrap();
            host.begin_request(&call(4), true).unwrap();
            tokio::task::yield_now().await;
            let announced: Vec<Value> = lines.borrow().iter().map(|line| serde_json::from_str(line).unwrap()).collect();
            assert_eq!(announced, vec![json!({"jsonrpc":"2.0","method":"notifications/tools/list_changed"})]);
        })
        .await;
}

#[test]
fn modern_ids_are_in_flight_only_and_legacy_ids_use_bounded_history() {
    let host = McpHost::default();
    let first = host.begin_request(&modern("tools/call", json!({"name":"live_status"})), true).unwrap();
    assert!(matches!(first.decision, RequestDecision::Tool(_)));
    let duplicate = host.begin_request(&modern("ping", json!({})), true).unwrap().completed().unwrap();
    assert_eq!(duplicate["error"]["message"], "Duplicate in-flight request identifier");
    assert!(duplicate.get("result").is_none());
    drop(first);
    assert!(host.begin_request(&modern("ping", json!({})), true).unwrap().completed().unwrap().get("result").is_some());
    let host = McpHost::default();
    initialize(&host);
    for id in 0..4097 {
        assert!(host
            .begin_request(&json!({"jsonrpc":"2.0","id":id,"method":"ping"}), false)
            .unwrap()
            .completed()
            .unwrap()
            .get("result")
            .is_some());
    }
    assert!(host
        .begin_request(&json!({"jsonrpc":"2.0","id":0,"method":"ping"}), false)
        .unwrap()
        .completed()
        .unwrap()
        .get("result")
        .is_some());
    assert_eq!(
        host.begin_request(&json!({"jsonrpc":"2.0","id":4096,"method":"ping"}), false).unwrap().completed().unwrap()["error"]["message"],
        "Duplicate request identifier"
    );
}
#[tokio::test]
async fn events_preserve_refs_channels_backpressure_overflow_and_failed_output_recovery() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let sim = Rc::new(DeterministicLiveSimulator::new());
            let host = Rc::new(McpHost::new(sim, McpHostOptions::default()).unwrap());
            initialize(&host);
            let events = Rc::new(RefCell::new(Vec::new()));
            let open = Rc::new(tokio::sync::Notify::new());
            let output = events.clone();
            let gate = open.clone();
            host.set_event_emitter(Rc::new(move |line| {
                let output = output.clone();
                let gate = gate.clone();
                Box::pin(async move {
                    gate.notified().await;
                    output.borrow_mut().push(serde_json::from_str::<Value>(&line).unwrap());
                    Ok(())
                })
            }))
            .unwrap();
            let event = LiveEvent {
                epoch: 1,
                sequence: 1,
                event_type: LiveEventType::Pointed,
                ref_: None,
                payload: json!({
                    "kind":"sample","path":[0,1],"slots":[{"kind":"clip_slot","path":[2,3]},{"kind":"bad-kind","path":[1]}],
                    "lanes":[{"kind":"take_lane","path":[]},{"kind":"clip","path":[1.5]}]
                }),
                channel: None,
                coalesced: None,
            };
            host.on_live_event(&event).unwrap();
            for sequence in 2..=65_540 {
                host.on_live_event(&LiveEvent { sequence, event_type: LiveEventType::Meter, payload: json!({}), ..event.clone() }).unwrap();
            }
            // First event is already removed; 65,536 more fit and the final three overflow.
            for _ in 0..65_538 {
                open.notify_one();
                tokio::task::yield_now().await;
            }
            let output = events.borrow();
            assert_eq!(output.len(), 65_538);
            assert_eq!(output[0]["params"]["channel"], "remote-script");
            assert_eq!(output[0]["params"]["payload"]["ref"], "1:device:0:1");
            assert_eq!(output[0]["params"]["payload"]["slots"][0]["ref"], "1:clip_slot:2:3");
            assert!(output[0]["params"]["payload"]["slots"][1].get("ref").is_none());
            assert_eq!(output[0]["params"]["payload"]["lanes"][0]["ref"], "1:take_lane:");
            assert!(output[0]["params"]["payload"]["lanes"][1].get("ref").is_none());
            assert_eq!(
                output.last().unwrap(),
                &json!({"jsonrpc":"2.0","method":"notifications/live_event_overflow","params":{"epoch":1,"dropped":3,"resnapshot":true}})
            );
            drop(output);
            host.set_event_emitter(Rc::new(|_| Box::pin(async { Err(LiveError::error("closed")) }))).unwrap();
            host.on_live_event(&event).unwrap();
            host.on_live_event(&event).unwrap();
            tokio::task::yield_now().await;
            host.on_live_event(&event).unwrap();
            let recovered = Rc::new(RefCell::new(Vec::new()));
            let output = recovered.clone();
            host.set_event_emitter(Rc::new(move |line| {
                output.borrow_mut().push(line);
                Box::pin(async { Ok(()) })
            }))
            .unwrap();
            host.on_live_event(&event).unwrap();
            tokio::task::yield_now().await;
            assert_eq!(recovered.borrow().len(), 2);
            let before = recovered.borrow().len();
            host.begin_request(&json!({"jsonrpc":"2.0","id":"list","method":"tools/list"}), false).unwrap().completed();
            host.set_tool_policy(&json!({"profile":"full","deny":["live_*"]})).unwrap();
            host.set_tool_policy(&json!({"profile":"full","deny":["live_*"]})).unwrap();
            tokio::task::yield_now().await;
            assert_eq!(recovered.borrow().len(), before + 1);
            assert_eq!(
                serde_json::from_str::<Value>(recovered.borrow().last().unwrap()).unwrap()["method"],
                "notifications/tools/list_changed"
            );
        })
        .await;
}
