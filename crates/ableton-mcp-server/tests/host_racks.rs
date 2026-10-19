//! Whole source-host rack/view transaction traces.
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
                if ["rack_", "rackview_"].iter().any(|prefix| text.starts_with(prefix)) {
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

/// The simulator, with device rows as the Remote Script sends them: a rack's view state in its `view`, never a
/// `rackView`.
struct RemoteShape(DeterministicLiveSimulator);
fn remote_rows(value: &mut Value) {
    match value {
        Value::Object(row) => {
            if let Some(Value::Object(rack)) = row.remove("rackView") {
                let view = row.entry("view").or_insert_with(|| json!({}));
                view.as_object_mut().unwrap().extend(rack);
            }
            row.values_mut().for_each(remote_rows);
        }
        Value::Array(items) => items.iter_mut().for_each(remote_rows),
        _ => {}
    }
}
impl LiveAdapter for RemoteShape {
    fn status(&self) -> Result<LiveStatus, LiveError> {
        self.0.status()
    }
    fn snapshot(&self) -> Result<LiveSnapshot, LiveError> {
        self.0.snapshot()
    }
    fn get(&self, r: &LiveRef) -> Result<Option<Value>, LiveError> {
        self.0.get(r)
    }
    fn invoke(&self, i: &LiveInvocation) -> Result<Value, LiveError> {
        self.0.invoke(i)
    }
    fn subscribe(&self, l: LiveListener) -> Result<Unsubscribe, LiveError> {
        self.0.subscribe(l)
    }
    fn reconnect(&self) -> Result<LiveStatus, LiveError> {
        self.0.reconnect()
    }
}
#[async_trait::async_trait(?Send)]
impl AsyncLiveAdapter for RemoteShape {
    async fn snapshot_async(&self, c: Option<&LiveOperationContext>, r: Option<&LiveSnapshotRequest>) -> Result<LiveSnapshot, LiveError> {
        let mut rows = serde_json::to_value(self.0.snapshot_async(c, r).await?).unwrap();
        remote_rows(&mut rows);
        Ok(serde_json::from_value(rows).unwrap())
    }
    async fn discover_async(&self, r: &LiveDiscoveryRequest, c: Option<&LiveOperationContext>) -> Result<LiveDiscoveryResult, LiveError> {
        self.0.discover_async(r, c).await
    }
    async fn get_async(&self, r: &LiveRef, _: Option<&LiveOperationContext>) -> Result<Option<Value>, LiveError> {
        self.0.get(r)
    }
    async fn invoke_async(&self, i: &LiveInvocation, _: Option<&LiveOperationContext>) -> Result<Value, LiveError> {
        self.0.invoke(i)
    }
    async fn reconnect_async(&self, _: Option<&LiveOperationContext>) -> Result<LiveStatus, LiveError> {
        self.0.reconnect()
    }
    async fn close(&self) -> Result<(), LiveError> {
        Ok(())
    }
}
#[tokio::test(flavor = "current_thread")]
async fn a_rack_view_is_read_where_the_remote_script_sends_it() {
    let live = Rc::new(RemoteShape(DeterministicLiveSimulator::new()));
    let rack = json!({"ref":"device:rack-1","parentRef":"track:track-1","objectIdentity":"simulator:device:rack-1","name":"Drum Rack","kind":"rack","className":"DrumGroupDevice","canHaveChains":true,"canHaveDrumPads":false,"parameters":[],"chains":[],"view":{"isCollapsed":false},"rackView":{"selectedChainRef":null,"selectedPadIndex":36,"padScrollPosition":3,"showChainDevices":false}});
    live.0.state.borrow_mut()["tracks"][0]["devices"] = json!([rack]);
    let host = McpHost::new(live.clone(), McpHostOptions::default()).unwrap();
    let call = |name: &str, arguments: Value| ToolCall { id: json!(1), name: name.into(), arguments: Some(arguments), asynchronous: true };
    let text = |result: Value| -> Value { serde_json::from_str(result["result"]["content"][0]["text"].as_str().unwrap()).unwrap() };
    let preview =
        host.dispatch_rack_tool(&call("live_rack_view_preview", json!({"rackRef":"device:rack-1","showChainDevices":true})), None).await;
    let preview = text(preview.unwrap().unwrap().unwrap());
    // What the view shows now, read from the rack's `view`.
    assert_eq!(
        preview["prior"],
        json!({"selectedChainRef":null,"selectedPadIndex":36,"padScrollPosition":3,"showChainDevices":false}),
        "{preview}"
    );
    let apply = json!({"transactionId":preview["transactionId"],"confirmation":"apply","idempotencyKey":"apply-key"});
    let applied = text(host.dispatch_rack_tool(&call("live_rack_view_apply", apply), None).await.unwrap().unwrap().unwrap());
    assert_eq!(applied["state"], "applied", "{applied}");
    assert_eq!(live.0.state.borrow()["tracks"][0]["devices"][0]["rackView"]["showChainDevices"], true);
    let undo = json!({"transactionId":preview["transactionId"],"confirmation":"undo","idempotencyKey":"undo-key"});
    let undone = text(host.undo_rack_async(&json!(2), &undo, None).await);
    assert_eq!(undone["state"], "undone", "{undone}");
    assert_eq!(live.0.state.borrow()["tracks"][0]["devices"][0]["rackView"]["showChainDevices"], false);
}

#[tokio::test(flavor = "current_thread")]
async fn racks_and_views_match_source_workflows() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let fixture: Value = serde_json::from_str(include_str!("fixtures/host-racks-oracle.json")).unwrap();
            for case in fixture["cases"].as_array().unwrap() {
                let label = case["label"].as_str().unwrap();
                if std::env::var("KUMI_RACK_CASE").ok().is_some_and(|v| v != label) {
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
                        host.with_undo_watch(&json!(1), &p, async { Ok(host.undo_rack_async(&json!(1), &p, Some(&signal)).await) })
                            .await
                            .unwrap()
                    } else {
                        let call = ToolCall {
                            asynchronous: true,
                            id: json!(1),
                            name: format!("live_{}_{}", case["tool"].as_str().unwrap(), action),
                            arguments: Some(p),
                        };
                        host.dispatch_rack_tool(&call, Some(&signal)).await.unwrap().unwrap().unwrap_or(Value::Null)
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
