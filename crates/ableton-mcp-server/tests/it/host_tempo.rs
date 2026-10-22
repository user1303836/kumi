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
    serde_json::from_str(include_str!("fixtures/host-tempo-oracle.json")).unwrap()
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
                    if v.as_str().is_some_and(|v| v.starts_with("tempo_")) {
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
            Value::String(s) if s.starts_with("tempo_") => *v = json!("$transaction"),
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
        self.calls.borrow_mut().push(clean(json!({"invocation":i,"context":context})));
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
    fn get_impl(&self, r: &LiveRef) -> Result<Option<Value>, LiveError> {
        let fault = self.fault.borrow();
        if !self.fired.get() && ((fault.ends_with("-read") && self.after_invoke.get()) || *fault == "undo-current-read") {
            self.fired.set(true);
            return Err(LiveError::error("injected authoritative read failure"));
        }
        self.sim.get(r)
    }
    fn edit(&self, key: &str, value: Value) {
        self.sim.state.borrow_mut()["set"][key] = value;
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
        self.get_impl(r)
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
        self.sim.snapshot_async(c, r).await
    }
    async fn discover_async(&self, r: &LiveDiscoveryRequest, c: Option<&LiveOperationContext>) -> Result<LiveDiscoveryResult, LiveError> {
        self.sim.discover_async(r, c).await
    }
    async fn get_async(&self, r: &LiveRef, _: Option<&LiveOperationContext>) -> Result<Option<Value>, LiveError> {
        self.get_impl(r)
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
    async fn refresh_status_async(&self, _: Option<&LiveOperationContext>) -> Result<LiveStatus, LiveError> {
        self.sim.status()
    }
}
async fn perform(
    host: &McpHost,
    asynchronous: bool,
    kind: &str,
    key: &str,
    txid: &Value,
    results: &mut Vec<Value>,
    states: &mut Vec<Value>,
    record: &Rc<RefCell<Value>>,
) {
    let id = json!(results.len() + 1);
    let args = json!({"transactionId":txid,"confirmation":kind,"idempotencyKey":key});
    let result = if kind == "apply" {
        if asynchronous {
            host.live_tempo_apply_async(&id, &args, None).await
        } else {
            host.live_tempo_apply(&id, &args)
        }
    } else if asynchronous {
        host.with_undo_watch(&id, &args, async { Ok(host.undo_tempo_async(&id, &args, None).await) }).await.unwrap()
    } else {
        host.undo_tempo(&id, &args)
    };
    results.push(clean(result));
    states.push(clean(record.borrow().clone()));
}
#[tokio::test]
async fn tempo_validation_and_release_match_source() {
    for (index, row) in fixture()["rows"].as_array().unwrap().iter().enumerate() {
        let host = McpHost::new(Rc::new(DeterministicLiveSimulator::new()), McpHostOptions::default()).unwrap();
        let tool = row["tool"].as_str().unwrap();
        let name = if tool.contains("Preview") {
            "live_tempo_preview"
        } else if tool.contains("Apply") {
            "live_tempo_apply"
        } else {
            "live_transaction_release"
        };
        let got = host
            .dispatch_tempo_tool(
                &ToolCall { id: json!(1), name: name.into(), arguments: Some(row["args"].clone()), asynchronous: tool.ends_with("Async") },
                None,
            )
            .await
            .unwrap()
            .unwrap_or_else(|e| json!({"error":e.message()}));
        same(&clean(got), &row["result"], &format!("row {index} {row}"));
    }
}
#[tokio::test]
async fn tempo_apply_undo_replay_and_refusal_workflows_match_source() {
    for row in fixture()["workflows"].as_array().unwrap() {
        let asynchronous = row["async"].as_bool().unwrap();
        let scenario = row["scenario"].as_str().unwrap();
        let adapter = Rc::new(Adapter::new());
        let host = McpHost::new(adapter.clone(), McpHostOptions::default()).unwrap();
        let preview = if asynchronous {
            host.live_tempo_preview_async(&json!(1), &json!({"tempo":124})).await.unwrap()
        } else {
            host.live_tempo_preview(&json!(1), &json!({"tempo":124}))
        };
        let body: Value = serde_json::from_str(preview["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
        let txid = body["transactionId"].clone();
        let record = host.transaction_record(txid.as_str().unwrap()).unwrap();
        let mut results = vec![clean(preview)];
        let mut states = vec![];
        if scenario == "expire" {
            record.borrow_mut()["expiresAt"] = json!(0);
        }
        if scenario == "epoch" {
            adapter.sim.reconnect().unwrap();
        }
        if scenario == "tempo-edit" {
            adapter.edit("tempo", json!(130));
        }
        if scenario == "identity-edit" {
            adapter.edit("objectIdentity", json!("other:set"));
        }
        if scenario.starts_with("apply-") {
            adapter.reset(scenario);
        }
        for key in ["apply-key", "other-key", "apply-key"] {
            perform(&host, asynchronous, "apply", key, &txid, &mut results, &mut states, &record).await;
        }
        adapter.reset("");
        if scenario == "undo-other-tempo" {
            adapter.edit("tempo", json!(130));
        }
        if scenario == "undo-prior-tempo" {
            adapter.edit("tempo", json!(120));
        }
        if scenario == "undo-other-set" {
            adapter.edit("objectIdentity", json!("other:set"));
        }
        if scenario.starts_with("undo-") && !scenario.contains("tempo") && !scenario.contains("set") {
            adapter.reset(scenario);
        }
        for key in ["undo-key", "other-undo-key", "undo-key"] {
            perform(&host, asynchronous, "undo", key, &txid, &mut results, &mut states, &record).await;
        }
        adapter.reset("");
        perform(&host, asynchronous, "undo", "new-undo-key", &txid, &mut results, &mut states, &record).await;
        results.push(clean(
            host.live_transaction_release(&json!(results.len() + 1), &json!({"transactionIds":[txid,txid,"missing","batch_missing"]})),
        ));
        let label = format!("async={asynchronous} {scenario}");
        same(&json!(results), &row["results"], &format!("{label} results"));
        same(&json!(states), &row["states"], &format!("{label} states"));
        same(&json!(*adapter.calls.borrow()), &row["calls"], &format!("{label} calls"));
        same(&adapter.sim.state.borrow()["set"]["tempo"], &row["tempo"], &format!("{label} tempo"));
    }
}
