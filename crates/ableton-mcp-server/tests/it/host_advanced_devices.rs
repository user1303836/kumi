//! Whole source-host advanced device/chain transaction traces.
use ableton_mcp_server::{
    host::{helpers::canonical_mutation_identity, McpHost, McpHostOptions, ToolCall},
    live::*,
};
use kumi_common::abort::Signal;
use serde_json::{json, Value};
use std::{cell::RefCell, collections::VecDeque, rc::Rc};
fn same(a: &Value, b: &Value, label: &str) {
    assert_eq!(canonical_mutation_identity(a).unwrap(), canonical_mutation_identity(b).unwrap(), "{label}");
}
fn clean(mut value: Value, root: &str) -> Value {
    if let Some(text) = value["result"]["content"][0]["text"].as_str() {
        if let Ok(body) = serde_json::from_str::<Value>(text) {
            value["result"]["content"][0]["text"] = body;
        }
    }
    fn walk(value: &mut Value, key: &str, root: &str) {
        if key == "expiresAt" && *value != 0 {
            *value = json!("$time");
            return;
        }
        if key == "mtimeMs" {
            *value = json!("$mtime");
            return;
        }
        match value {
            Value::String(text) => {
                if ["devadv_", "chainset_"].iter().any(|prefix| text.starts_with(prefix)) {
                    *text = "$transaction".into();
                    return;
                }
            }
            Value::Array(items) => {
                for item in items {
                    walk(item, "", root)
                }
            }
            Value::Object(items) => {
                for (key, item) in items {
                    walk(item, key, root)
                }
            }
            _ => {}
        }
    }
    walk(&mut value, "", root);
    value
}
fn context(context: Option<&LiveOperationContext>) -> Value {
    let Some(c) = context else { return Value::Null };
    let mut out = json!({"deadline":c.deadline_ms.is_some()});
    if let Some(key) = &c.idempotency_key {
        out["idempotencyKey"] = json!(key)
    }
    if let Some(key) = &c.transaction_id {
        out["transactionId"] = json!(key)
    }
    out
}
struct Replay {
    root: String,
    status: RefCell<LiveStatus>,
    calls: RefCell<VecDeque<Value>>,
    label: RefCell<String>,
}
impl Replay {
    fn call(&self, method: &str, args: Value, c: Option<&LiveOperationContext>) -> Result<Value, LiveError> {
        let want = self.calls.borrow_mut().pop_front().unwrap_or_else(|| panic!("{}: unexpected {method}", self.label.borrow()));
        let actual = clean(json!({"method":method,"args":args,"context":context(c)}), &self.root);
        same(&actual, &json!({"method":want["method"],"args":want["args"],"context":want["context"]}), &self.label.borrow());
        if let Some(error) = want.get("error") {
            let text = error["message"].as_str().unwrap().to_string();
            Err(if error["kind"] == "not-dispatched" { LiveError::MutationNotDispatched(text) } else { LiveError::error(text) })
        } else {
            Ok(want["result"].clone())
        }
    }
}
impl LiveAdapter for Replay {
    fn status(&self) -> Result<LiveStatus, LiveError> {
        Ok(self.status.borrow().clone())
    }
    fn snapshot(&self) -> Result<LiveSnapshot, LiveError> {
        panic!("unexpected sync snapshot")
    }
    fn get(&self, _: &LiveRef) -> Result<Option<Value>, LiveError> {
        panic!("unexpected sync get")
    }
    fn invoke(&self, _: &LiveInvocation) -> Result<Value, LiveError> {
        panic!("unexpected sync invoke")
    }
    fn subscribe(&self, _: LiveListener) -> Result<Unsubscribe, LiveError> {
        Ok(Box::new(|| {}))
    }
    fn reconnect(&self) -> Result<LiveStatus, LiveError> {
        self.status()
    }
}
#[async_trait::async_trait(?Send)]
impl AsyncLiveAdapter for Replay {
    async fn snapshot_async(&self, c: Option<&LiveOperationContext>, r: Option<&LiveSnapshotRequest>) -> Result<LiveSnapshot, LiveError> {
        serde_json::from_value(self.call("snapshot", json!(r), c)?).map_err(|e| LiveError::error(e.to_string()))
    }
    async fn discover_async(&self, r: &LiveDiscoveryRequest, c: Option<&LiveOperationContext>) -> Result<LiveDiscoveryResult, LiveError> {
        serde_json::from_value(self.call("discover", json!(r), c)?).map_err(|e| LiveError::error(e.to_string()))
    }
    async fn get_async(&self, r: &LiveRef, c: Option<&LiveOperationContext>) -> Result<Option<Value>, LiveError> {
        self.call("get", json!(r), c).map(|v| (!v.is_null()).then_some(v))
    }
    async fn invoke_async(&self, i: &LiveInvocation, c: Option<&LiveOperationContext>) -> Result<Value, LiveError> {
        self.call("invoke", json!(i), c)
    }
    async fn reconnect_async(&self, _: Option<&LiveOperationContext>) -> Result<LiveStatus, LiveError> {
        self.status()
    }
    async fn close(&self) -> Result<(), LiveError> {
        Ok(())
    }
    fn has_refresh_status_async(&self) -> bool {
        true
    }
    async fn refresh_status_async(&self, c: Option<&LiveOperationContext>) -> Result<LiveStatus, LiveError> {
        serde_json::from_value(self.call("status", Value::Null, c)?).map_err(|e| LiveError::error(e.to_string()))
    }
}

/// The simulator with a second track, "Bass", holding EQ Eight.
fn two_tracks() -> Rc<DeterministicLiveSimulator> {
    let live = Rc::new(DeterministicLiveSimulator::new());
    {
        let mut state = live.state.borrow_mut();
        let mut bass = state["tracks"][0].clone();
        bass["ref"] = json!("track:track-2");
        bass["objectIdentity"] = json!("simulator:track:track-2");
        bass["name"] = json!("Bass");
        bass["devices"] = json!([{"ref":"device:eq-2","parentRef":"track:track-2","objectIdentity":"simulator:device:eq-2","name":"EQ Eight","kind":"audio-effect","parameters":[],"enabled":true}]);
        bass["clips"] = json!([]);
        bass["clipSlots"] = json!([]);
        bass.as_object_mut().unwrap().remove("mixer");
        state["tracks"].as_array_mut().unwrap().push(bass);
    }
    live
}
#[tokio::test(flavor = "current_thread")]
async fn a_cross_target_move_refuses_its_own_owner_and_a_target_whose_devices_changed() {
    let live = two_tracks();
    let host = McpHost::new(live.clone(), McpHostOptions::default()).unwrap();
    let text = |result: Value| -> Value { serde_json::from_str(result["result"]["content"][0]["text"].as_str().unwrap()).unwrap() };
    // Its own track would only reorder it.
    let own = host
        .live_device_advanced_preview_async(
            &json!(1),
            &json!({"action":"move-cross","ref":"device:utility-1","targetTrackRef":"track:track-1","index":0}),
        )
        .await;
    assert_eq!(own["error"]["code"], -32602, "{own}");
    // After EQ Eight on Bass; then the producer puts another device first, so index 1 would land before EQ Eight.
    let preview = text(
        host.live_device_advanced_preview_async(
            &json!(2),
            &json!({"action":"move-cross","ref":"device:utility-1","targetTrackRef":"track:track-2","index":1}),
        )
        .await,
    );
    live.state.borrow_mut()["tracks"][1]["devices"].as_array_mut().unwrap().insert(0, json!({"ref":"device:comp-2","parentRef":"track:track-2","objectIdentity":"simulator:device:comp-2","name":"Compressor","kind":"audio-effect","parameters":[],"enabled":true}));
    let apply = json!({"transactionId":preview["transactionId"],"confirmation":"apply","idempotencyKey":"apply-key"});
    let refused = text(host.live_device_advanced_apply_async(&json!(3), &apply, None).await.unwrap());
    assert!(refused.to_string().contains("changed since the preview"), "{refused}");
    let names: Vec<_> = live.state.borrow()["tracks"][1]["devices"].as_array().unwrap().iter().map(|d| d["name"].clone()).collect();
    assert_eq!(names, [json!("Compressor"), json!("EQ Eight")], "nothing moved");
    assert_eq!(host.transaction_record(preview["transactionId"].as_str().unwrap()).unwrap().borrow()["state"], "previewed");
}
#[tokio::test(flavor = "current_thread")]
async fn re_enabling_an_overridden_parameters_automation_fences_on_its_own_state() {
    let live = Rc::new(DeterministicLiveSimulator::new());
    // The producer moved an automated knob: Live's automation state 2, overridden, the case re-enabling is for.
    live.state.borrow_mut()["tracks"][0]["devices"][0]["parameters"][0]["automationState"] = json!("2");
    let host = McpHost::new(live.clone(), McpHostOptions::default()).unwrap();
    let text = |result: Value| -> Value { serde_json::from_str(result["result"]["content"][0]["text"].as_str().unwrap()).unwrap() };
    let preview =
        text(host.live_device_advanced_preview_async(&json!(1), &json!({"action":"re-enable-automation","ref":"parameter:gain-1"})).await);
    let apply = json!({"transactionId":preview["transactionId"],"confirmation":"apply","idempotencyKey":"apply-key"});
    let applied = text(host.live_device_advanced_apply_async(&json!(2), &apply, None).await.unwrap());
    assert_eq!(applied["state"], "applied", "{preview} {applied}");
}
#[tokio::test(flavor = "current_thread")]
async fn advanced_devices_and_chains_match_source_workflows() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let fixture: Value = serde_json::from_str(include_str!("fixtures/host-advanced-devices-oracle.json")).unwrap();
            for case in fixture["cases"].as_array().unwrap() {
                let label = case["label"].as_str().unwrap();
                if std::env::var("KUMI_DEVICE_CASE").ok().is_some_and(|v| v != label) {
                    continue;
                }
                let adapter = Rc::new(Replay {
                    root: "$unreachable".into(),
                    status: RefCell::new(serde_json::from_value(case["steps"][0]["status"].clone()).unwrap()),
                    calls: Default::default(),
                    label: RefCell::new(label.into()),
                });
                let host = McpHost::new(adapter.clone(), McpHostOptions::default()).unwrap();
                let mut tx = None::<String>;
                for (index, step) in case["steps"].as_array().unwrap().iter().enumerate() {
                    let at = format!("{label} step {index}");
                    *adapter.label.borrow_mut() = at.clone();
                    *adapter.status.borrow_mut() = serde_json::from_value(step["status"].clone()).unwrap();
                    *adapter.calls.borrow_mut() = step["calls"].as_array().unwrap().clone().into();
                    if step["expire"] == true {
                        host.transaction_record(tx.as_ref().unwrap()).unwrap().borrow_mut()["expiresAt"] = json!(0)
                    }
                    let action = step["action"].as_str().unwrap();
                    let mut p = step["args"].clone();
                    if action != "preview" {
                        p["transactionId"] = json!(tx.as_ref().unwrap())
                    }
                    let signal = Signal::new();
                    if step["abort"] == true {
                        signal.cancel()
                    }
                    let result = if action == "undo" {
                        host.with_undo_watch(&json!(1), &p, async {
                            Ok(if case["tool"] == "chain" {
                                host.undo_chain_async(&json!(1), &p, Some(&signal)).await
                            } else {
                                host.undo_device_advanced_async(&json!(1), &p, Some(&signal)).await
                            })
                        })
                        .await
                        .unwrap()
                    } else {
                        let call = ToolCall {
                            asynchronous: true,
                            id: json!(1),
                            name: format!("live_{}_{}", case["tool"].as_str().unwrap(), action),
                            arguments: Some(p),
                        };
                        host.dispatch_advanced_device_tool(&call, Some(&signal)).await.unwrap().unwrap().unwrap_or(Value::Null)
                    };
                    if action == "preview" {
                        tx = result["result"]["content"][0]["text"]
                            .as_str()
                            .and_then(|v| serde_json::from_str::<Value>(v).ok())
                            .and_then(|v| v["transactionId"].as_str().map(str::to_owned))
                    }
                    same(&clean(result, "$unreachable"), &step["result"], &at);
                    let record = tx.as_ref().and_then(|tx| host.transaction_record(tx)).map(|r| r.borrow().clone()).unwrap_or(Value::Null);
                    same(&clean(record, "$unreachable"), &step["record"], &format!("{at} record"));
                    assert!(adapter.calls.borrow().is_empty(), "{at}: remaining calls");
                }
            }
        })
        .await
}
