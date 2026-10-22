use ableton_mcp_server::{
    host::{helpers::canonical_mutation_identity, McpHost, McpHostOptions, ToolCall},
    live::*,
};
use serde_json::{json, Value};
use std::{
    cell::{Cell, RefCell},
    collections::HashMap,
    rc::Rc,
};
fn fixture() -> Value {
    serde_json::from_str(include_str!("fixtures/host-rename-oracle.json")).unwrap()
}
fn same(a: &Value, b: &Value, label: &str) {
    assert_eq!(canonical_mutation_identity(a).unwrap(), canonical_mutation_identity(b).unwrap(), "{label}");
}
fn clean(mut v: Value) -> Value {
    if let Some(s) = v["result"]["content"][0]["text"].as_str() {
        if let Ok(body) = serde_json::from_str::<Value>(s) {
            v["result"]["content"][0]["text"] = body;
        }
    }
    fn walk(v: &mut Value) {
        match v {
            Value::Object(o) => {
                for (k, v) in o {
                    if v.as_str().is_some_and(|v| v.starts_with("rename_")) {
                        *v = json!("$transaction");
                    } else if k == "expiresAt" {
                        *v = json!("$time");
                    } else {
                        walk(v)
                    }
                }
            }
            Value::Array(a) => {
                for v in a {
                    walk(v)
                }
            }
            Value::String(s) if s.starts_with("rename_") => *v = json!("$transaction"),
            _ => {}
        }
    }
    walk(&mut v);
    v
}
struct Adapter {
    sim: DeterministicLiveSimulator,
    calls: RefCell<Vec<Value>>,
    cache: RefCell<HashMap<String, Value>>,
    fault: RefCell<String>,
    fired: Cell<bool>,
    after_invoke: Cell<bool>,
}
impl Adapter {
    fn new() -> Self {
        Self {
            sim: DeterministicLiveSimulator::new(),
            calls: Default::default(),
            cache: Default::default(),
            fault: Default::default(),
            fired: Cell::new(false),
            after_invoke: Cell::new(false),
        }
    }
    fn reset(&self, fault: &str) {
        *self.fault.borrow_mut() = fault.into();
        self.fired.set(false);
        self.after_invoke.set(false);
    }
    fn invoke_impl(&self, i: &LiveInvocation, c: Option<&LiveOperationContext>) -> Result<Value, LiveError> {
        let mut context = Value::Null;
        if let Some(c) = c {
            context = json!({"deadline":c.deadline_ms.is_some()});
            if let Some(k) = &c.idempotency_key {
                context["idempotencyKey"] = json!(k);
            }
            if let Some(k) = &c.transaction_id {
                context["transactionId"] = json!(k);
            }
        }
        self.calls.borrow_mut().push(clean(json!({"method":"invoke","invocation":i,"context":context})));
        let key =
            canonical_mutation_identity(&json!([c.and_then(|c| c.transaction_id.as_ref()), c.and_then(|c| c.idempotency_key.as_ref()), i]))
                .unwrap();
        if c.is_some() {
            if let Some(v) = self.cache.borrow().get(&key) {
                return Ok(v.clone());
            }
        }
        let fault = self.fault.borrow().clone();
        if !self.fired.get() && ["before", "cancel", "refusal"].iter().any(|s| fault.ends_with(s)) {
            self.fired.set(true);
            return Err(if fault.ends_with("refusal") {
                LiveError::MutationNotDispatched("mutation was not dispatched: ownership changed".into())
            } else {
                LiveError::error(if fault.ends_with("cancel") {
                    "operation cancelled before dispatch"
                } else {
                    "injected operation failure"
                })
            });
        }
        let no_effect = !self.fired.get() && fault.ends_with("no-effect");
        let result = if no_effect {
            self.fired.set(true);
            json!({"ok":true})
        } else {
            self.sim.invoke(i)?
        };
        if c.is_some() && !no_effect {
            self.cache.borrow_mut().insert(key, result.clone());
        }
        self.after_invoke.set(true);
        if !self.fired.get() && fault.ends_with("after") {
            self.fired.set(true);
            return Err(LiveError::error("injected operation failure"));
        }
        Ok(result)
    }
    fn read_fault(&self) -> Result<(), LiveError> {
        let fault = self.fault.borrow();
        if !self.fired.get() && ((fault.ends_with("-read") && self.after_invoke.get()) || *fault == "undo-current-read") {
            self.fired.set(true);
            return Err(LiveError::error("injected authoritative read failure"));
        }
        Ok(())
    }
    fn edit(&self, kind: &str, key: &str, value: Value) {
        let mut state = self.sim.state.borrow_mut();
        target_mut(&mut state, kind).as_object_mut().unwrap().insert(key.into(), value);
    }
}
impl LiveAdapter for Adapter {
    fn status(&self) -> Result<LiveStatus, LiveError> {
        self.sim.status()
    }
    fn snapshot(&self) -> Result<LiveSnapshot, LiveError> {
        self.sim.snapshot()
    }
    fn get(&self, r: &LiveRef) -> Result<Option<Value>, LiveError> {
        self.sim.get(r)
    }
    fn invoke(&self, i: &LiveInvocation) -> Result<Value, LiveError> {
        self.invoke_impl(i, None)
    }
    fn subscribe(&self, l: LiveListener) -> Result<Unsubscribe, LiveError> {
        self.sim.subscribe(l)
    }
    fn reconnect(&self) -> Result<LiveStatus, LiveError> {
        self.sim.reconnect()
    }
}
#[async_trait::async_trait(?Send)]
impl AsyncLiveAdapter for Adapter {
    async fn snapshot_async(&self, c: Option<&LiveOperationContext>, r: Option<&LiveSnapshotRequest>) -> Result<LiveSnapshot, LiveError> {
        self.calls.borrow_mut().push(clean(json!({"method":"snapshot","request":r,"context":context(c)})));
        self.read_fault()?;
        self.sim.snapshot_async(c, r).await
    }
    async fn discover_async(&self, r: &LiveDiscoveryRequest, c: Option<&LiveOperationContext>) -> Result<LiveDiscoveryResult, LiveError> {
        self.calls.borrow_mut().push(clean(json!({"method":"discover","request":r,"context":context(c)})));
        self.read_fault()?;
        self.sim.discover_async(r, c).await
    }
    async fn get_async(&self, r: &LiveRef, c: Option<&LiveOperationContext>) -> Result<Option<Value>, LiveError> {
        self.calls.borrow_mut().push(clean(json!({"method":"get","reference":r,"context":context(c)})));
        self.read_fault()?;
        self.sim.get(r)
    }
    async fn invoke_async(&self, i: &LiveInvocation, c: Option<&LiveOperationContext>) -> Result<Value, LiveError> {
        self.invoke_impl(i, c)
    }
    async fn reconnect_async(&self, _: Option<&LiveOperationContext>) -> Result<LiveStatus, LiveError> {
        self.sim.reconnect()
    }
    async fn close(&self) -> Result<(), LiveError> {
        Ok(())
    }
    fn has_refresh_status_async(&self) -> bool {
        true
    }
    async fn refresh_status_async(&self, c: Option<&LiveOperationContext>) -> Result<LiveStatus, LiveError> {
        self.calls.borrow_mut().push(clean(json!({"method":"status","context":context(c)})));
        self.sim.status()
    }
}

fn context(c: Option<&LiveOperationContext>) -> Value {
    match c {
        None => Value::Null,
        Some(c) => {
            let mut value = json!({"deadline":c.deadline_ms.is_some()});
            if let Some(key) = &c.idempotency_key {
                value["idempotencyKey"] = json!(key);
            }
            if let Some(key) = &c.transaction_id {
                value["transactionId"] = json!(key);
            }
            value
        }
    }
}
fn target_mut<'a>(state: &'a mut Value, kind: &str) -> &'a mut Value {
    match kind {
        "track" => &mut state["tracks"][0],
        "scene" => &mut state["scenes"][0],
        "clip" => &mut state["tracks"][0]["clips"][0],
        "device" => &mut state["tracks"][0]["devices"][0],
        "locator" => &mut state["arrangement"]["locators"][0],
        "takeLane" => &mut state["tracks"][0]["takeLanes"][0],
        _ => unreachable!(),
    }
}
fn reference(kind: &str) -> &str {
    match kind {
        "track" => "track:track-1",
        "scene" => "scene:scene-1",
        "clip" => "clip:clip-1",
        "device" => "device:utility-1",
        "locator" => "locator:locator-1",
        "takeLane" => "take-lane:track-1:0",
        _ => unreachable!(),
    }
}
#[tokio::test]
async fn rename_validation_matches_source() {
    for (index, row) in fixture()["rows"].as_array().unwrap().iter().enumerate() {
        let host = McpHost::new(Rc::new(DeterministicLiveSimulator::new()), McpHostOptions::default()).unwrap();
        let name =
            if row["tool"].as_str().unwrap().contains("Preview") { "live_object_rename_preview" } else { "live_object_rename_apply" };
        let got = host
            .dispatch_rename_tool(
                &ToolCall { id: json!(1), name: name.into(), arguments: Some(row["args"].clone()), asynchronous: true },
                None,
            )
            .await
            .unwrap()
            .unwrap();
        same(&clean(got), &row["result"], &format!("{index} {row}"));
    }
}
async fn perform(
    host: &McpHost,
    action: &str,
    key: &str,
    txid: &Value,
    results: &mut Vec<Value>,
    states: &mut Vec<Value>,
    record: &Rc<RefCell<Value>>,
) {
    let id = json!(results.len() + 1);
    let args = json!({"transactionId":txid,"confirmation":action,"idempotencyKey":key});
    let result = if action == "apply" {
        host.live_object_rename_apply_async(&id, &args, None).await
    } else {
        host.with_undo_watch(&id, &args, async { Ok(host.undo_rename_async(&id, &args, None).await) }).await.unwrap()
    };
    results.push(clean(result));
    states.push(clean(record.borrow().clone()));
}
#[tokio::test]
async fn rename_apply_and_exact_key_undo_match_source() {
    for row in fixture()["workflows"].as_array().unwrap() {
        let kind = row["kind"].as_str().unwrap();
        let scenario = row["scenario"].as_str().unwrap();
        let adapter = Rc::new(Adapter::new());
        let original = target_mut(&mut adapter.sim.state.borrow_mut(), kind)["name"].clone();
        if scenario.starts_with("return-") {
            adapter.edit(kind, "kind", json!("return"));
            adapter.edit(kind, "name", json!("A-Reverb"));
        }
        if scenario == "missing-identity" {
            target_mut(&mut adapter.sim.state.borrow_mut(), kind).as_object_mut().unwrap().remove("objectIdentity");
        }
        let host = McpHost::new(adapter.clone(), McpHostOptions::default()).unwrap();
        let name = if scenario == "same-name" {
            original.clone()
        } else if scenario == "return-prefixed" {
            json!("A-Delay")
        } else {
            json!("New Name")
        };
        let preview = host.live_object_rename_preview_async(&json!(1), &json!({"kind":kind,"ref":reference(kind),"name":name})).await;
        let body: Value = serde_json::from_str(preview["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
        let mut results = vec![clean(preview)];
        let mut states = vec![];
        if let Some(txid) = body.get("transactionId") {
            let record = host.transaction_record(txid.as_str().unwrap()).unwrap();
            if scenario == "expire" {
                record.borrow_mut()["expiresAt"] = json!(0);
            }
            if scenario == "epoch" {
                adapter.sim.reconnect().unwrap();
            }
            if scenario == "name-edit" {
                adapter.edit(kind, "name", json!("Manual"));
            }
            if scenario == "identity-edit" {
                adapter.edit(kind, "objectIdentity", json!("other"));
            }
            if scenario.starts_with("apply-") {
                adapter.reset(scenario);
            }
            for key in ["apply-key", "other-key", "apply-key"] {
                perform(&host, "apply", key, txid, &mut results, &mut states, &record).await;
            }
            adapter.reset("");
            if scenario == "undo-other-name" {
                adapter.edit(kind, "name", json!("Manual"));
            }
            if scenario == "undo-prior-name" {
                adapter.edit(kind, "name", original);
            }
            if scenario == "undo-other-identity" {
                adapter.edit(kind, "objectIdentity", json!("other"));
            }
            if scenario.starts_with("undo-") && !["undo-other-name", "undo-prior-name", "undo-other-identity"].contains(&scenario) {
                adapter.reset(scenario);
            }
            for key in ["undo-key", "other-undo-key", "undo-key"] {
                perform(&host, "undo", key, txid, &mut results, &mut states, &record).await;
            }
            adapter.reset("");
            perform(&host, "undo", "new-undo-key", txid, &mut results, &mut states, &record).await;
        }
        let label = format!("{kind} {scenario}");
        same(&json!(results), &row["results"], &format!("{label} results"));
        same(&json!(states), &row["states"], &format!("{label} states"));
        same(&json!(*adapter.calls.borrow()), &row["calls"], &format!("{label} calls"));
        same(target_mut(&mut adapter.sim.state.borrow_mut(), kind), &row["target"], &format!("{label} target"));
    }
}
