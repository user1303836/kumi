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
    serde_json::from_str(include_str!("fixtures/host-arrangement-oracle.json")).unwrap()
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
                    if v.as_str().is_some_and(|v| v.starts_with("arrangement_")) {
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
            Value::String(s) if s.starts_with("arrangement_") => *v = json!("$transaction"),
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
    fault_step: Cell<usize>,
    count: Cell<usize>,
    fired: Cell<bool>,
    read_fail: Cell<bool>,
    compensate_fail: Cell<bool>,
    /// Live's locator refs are positional (`{epoch}:locator:{index}`, in time order): adding or deleting one renumbers
    /// those after it.
    positional: Cell<bool>,
}
impl Adapter {
    fn new() -> Self {
        Self {
            sim: DeterministicLiveSimulator::new(),
            calls: Default::default(),
            cache: Default::default(),
            fault: Default::default(),
            fault_step: Cell::new(0),
            count: Cell::new(0),
            fired: Cell::new(false),
            read_fail: Cell::new(false),
            compensate_fail: Cell::new(false),
            positional: Cell::new(false),
        }
    }
    /// Renumbers the locators by place, as Live does, and gives `made` (a created locator's result) its ref and
    /// fingerprint there.
    fn renumber(&self, made: Option<&mut Value>) {
        use ableton_mcp_server::registry::{canonical_json, sha256_hex, UNBOUNDED_CANONICAL_LIMITS};
        let hash = |value: &Value| sha256_hex(&canonical_json(value, &UNBOUNDED_CANONICAL_LIMITS).unwrap());
        let mut state = self.sim.state.borrow_mut();
        let locators = state["arrangement"]["locators"].as_array_mut().unwrap();
        locators.sort_by(|a, b| a["position"].as_f64().unwrap().total_cmp(&b["position"].as_f64().unwrap()));
        for (index, locator) in locators.iter_mut().enumerate() {
            locator["ref"] = json!(format!("locator:at-{index}"));
        }
        if let Some(made) = made {
            let row = locators.iter().find(|row| row["objectIdentity"] == made["objectIdentity"]).unwrap();
            made["ref"] = row["ref"].clone();
            made["createdFingerprint"] = json!(hash(row));
        }
        let revision = hash(&state["arrangement"]["locators"]);
        state["arrangement"]["locatorRevision"] = json!(revision);
    }
    fn reset(&self, fault: &str) {
        *self.fault.borrow_mut() = fault.into();
        self.count.set(0);
        self.fired.set(false);
        self.read_fail.set(false);
        self.compensate_fail.set(false);
    }
    fn read(&self) -> Result<(), LiveError> {
        if self.read_fail.replace(false) {
            Err(LiveError::error("injected authoritative read failure"))
        } else {
            Ok(())
        }
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
        self.count.set(self.count.get() + 1);
        let key =
            canonical_mutation_identity(&json!([c.and_then(|c| c.transaction_id.as_ref()), c.and_then(|c| c.idempotency_key.as_ref()), i]))
                .unwrap();
        if c.is_some() {
            if let Some(v) = self.cache.borrow().get(&key) {
                return Ok(v.clone());
            }
        }
        let fault = self.fault.borrow().clone();
        if self.compensate_fail.get() && i.operation == "locator.delete" {
            self.compensate_fail.set(false);
            if fault != "compensate-before" {
                let result = self.sim.invoke(i)?;
                if c.is_some() {
                    self.cache.borrow_mut().insert(key, result);
                }
            }
            return Err(LiveError::error("injected compensation failure"));
        }
        let trigger = !self.fired.get() && self.count.get() == self.fault_step.get();
        if trigger {
            self.fired.set(true);
        }
        if trigger && ["before", "nothing", "refusal"].contains(&fault.as_str()) {
            return Err(if fault == "refusal" {
                LiveError::MutationNotDispatched("mutation was not dispatched: ownership changed".into())
            } else {
                LiveError::error(if fault == "nothing" {
                    "Nothing changed in Live: rejected locator"
                } else {
                    "injected operation failure"
                })
            });
        }
        let mut result = if trigger && fault == "no-effect" { json!({"ok":true}) } else { self.sim.invoke(i)? };
        if self.positional.get() && ["locator.add", "locator.delete"].contains(&i.operation.as_str()) {
            self.renumber((i.operation == "locator.add").then_some(&mut result));
        }
        if c.is_some() && !(trigger && fault == "no-effect") {
            self.cache.borrow_mut().insert(key, result.clone());
        }
        if trigger {
            if fault == "after" {
                return Err(LiveError::error("injected operation failure"));
            }
            if fault == "read" {
                self.read_fail.set(true);
            }
            if ["wrong-name", "compensate-before", "compensate-after"].contains(&fault.as_str()) {
                result["name"] = json!("Wrong");
                if fault.starts_with("compensate-") {
                    self.compensate_fail.set(true);
                }
            }
            if fault == "bad-fingerprint" {
                result["createdFingerprint"] = json!("bad");
            }
            if fault == "bad-identity" {
                result["objectIdentity"] = json!("changed");
            }
            if fault == "missing-ref" {
                result.as_object_mut().unwrap().remove("ref");
            }
            if fault == "null-result" {
                result = Value::Null;
            }
            if fault == "changed-owned" {
                let mut state = self.sim.state.borrow_mut();
                state["arrangement"]["locators"].as_array_mut().unwrap().iter_mut().find(|r| r["ref"] == result["ref"]).unwrap()["name"] =
                    json!("Changed");
            }
        }
        Ok(result)
    }
}
impl LiveAdapter for Adapter {
    fn status(&self) -> Result<LiveStatus, LiveError> {
        self.sim.status()
    }
    fn snapshot(&self) -> Result<LiveSnapshot, LiveError> {
        self.read()?;
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
        self.read()?;
        self.sim.snapshot_async(c, r).await
    }
    async fn discover_async(&self, r: &LiveDiscoveryRequest, c: Option<&LiveOperationContext>) -> Result<LiveDiscoveryResult, LiveError> {
        self.sim.discover_async(r, c).await
    }
    async fn get_async(&self, r: &LiveRef, c: Option<&LiveOperationContext>) -> Result<Option<Value>, LiveError> {
        self.sim.get_async(r, c).await
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
            host.live_arrangement_apply_async(&id, &args, None).await
        } else {
            host.live_arrangement_apply(&id, &args)
        }
    } else if asynchronous {
        host.with_undo_watch(&id, &args, async { Ok(host.undo_arrangement_async(&id, &args, None).await) }).await.unwrap()
    } else {
        host.undo_arrangement(&id, &args)
    };
    results.push(clean(result));
    states.push(clean(record.borrow().clone()));
}
#[tokio::test]
async fn arrangement_section_validation_matches_source() {
    for (index, row) in fixture()["rows"].as_array().unwrap().iter().enumerate() {
        let host = McpHost::new(Rc::new(DeterministicLiveSimulator::new()), McpHostOptions::default()).unwrap();
        let tool = row["tool"].as_str().unwrap();
        let name = if tool.contains("Preview") { "live_arrangement_section_preview" } else { "live_arrangement_section_apply" };
        let got = host
            .dispatch_arrangement_tool(
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
async fn a_section_before_another_locator_is_undone_and_compensated_by_identity() {
    // "Outro" sits after the section, so each locator the section adds or deletes renumbers it.
    for scenario in ["undo", "compensate"] {
        let adapter = Rc::new(Adapter::new());
        adapter.positional.set(true);
        let revision = adapter.sim.state.borrow()["arrangement"]["locatorRevision"].clone();
        adapter
            .sim
            .invoke(&LiveInvocation::new("locator.add", json!({"name":"Outro","position":64,"expectedCollectionRevision":revision})))
            .unwrap();
        adapter.renumber(None);
        let host = McpHost::new(adapter.clone(), McpHostOptions::default()).unwrap();
        let params = json!({"start":16,"end":32,"startName":"Verse","endName":"End Verse"});
        let preview = host.live_arrangement_preview_async(&json!(1), &params).await;
        let body: Value = serde_json::from_str(preview["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
        let record = host.transaction_record(body["transactionId"].as_str().unwrap()).unwrap();
        let apply = json!({"transactionId":body["transactionId"],"confirmation":"apply","idempotencyKey":"apply-key"});
        if scenario == "compensate" {
            // Live makes "End Verse" but names it wrongly: both are taken back.
            adapter.fault_step.set(2);
            adapter.reset("wrong-name");
            host.live_arrangement_apply_async(&json!(2), &apply, None).await;
            assert_eq!(record.borrow()["state"], "undone", "{}", record.borrow());
        } else {
            host.live_arrangement_apply_async(&json!(2), &apply, None).await;
            assert_eq!(record.borrow()["state"], "applied");
            let undo = json!({"transactionId":body["transactionId"],"confirmation":"undo","idempotencyKey":"undo-key"});
            let result = host
                .with_undo_watch(&json!(3), &undo, async { Ok(host.undo_arrangement_async(&json!(3), &undo, None).await) })
                .await
                .unwrap();
            assert_eq!(record.borrow()["state"], "undone", "{result}");
        }
        let left: Vec<_> =
            adapter.sim.state.borrow()["arrangement"]["locators"].as_array().unwrap().iter().map(|l| l["name"].clone()).collect();
        assert_eq!(left, [json!("Intro"), json!("Outro")], "{scenario}");
    }
}
#[tokio::test]
async fn arrangement_apply_compensation_replay_and_undo_workflows_match_source() {
    for row in fixture()["workflows"].as_array().unwrap() {
        let asynchronous = row["async"].as_bool().unwrap();
        let scenario = row["scenario"].as_str().unwrap();
        let adapter = Rc::new(Adapter::new());
        let host = McpHost::new(adapter.clone(), McpHostOptions::default()).unwrap();
        let params = json!({"start":16,"end":32,"startName":"Verse","endName":"End Verse"});
        let preview = if asynchronous {
            host.live_arrangement_preview_async(&json!(1), &params).await
        } else {
            host.live_arrangement_preview(&json!(1), &params)
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
        if scenario == "revision" {
            let revision = adapter.sim.state.borrow()["arrangement"]["locatorRevision"].clone();
            adapter
                .sim
                .invoke(&LiveInvocation::new("locator.add", json!({"name":"External","position":60,"expectedCollectionRevision":revision})))
                .unwrap();
        }
        if scenario.starts_with("apply/") {
            let p: Vec<_> = scenario.split('/').collect();
            adapter.fault_step.set(p[1].parse().unwrap());
            adapter.reset(p[2]);
        }
        for key in ["apply-key", "wrong-key", "apply-key"] {
            perform(&host, asynchronous, "apply", key, &txid, &mut results, &mut states, &record).await;
        }
        adapter.reset("");
        if scenario == "undo-epoch" {
            adapter.sim.reconnect().unwrap();
        }
        if scenario == "undo-missing" {
            adapter.sim.state.borrow_mut()["arrangement"]["locators"].as_array_mut().unwrap().pop();
        }
        if scenario == "undo-changed" {
            let mut state = adapter.sim.state.borrow_mut();
            state["arrangement"]["locators"].as_array_mut().unwrap().last_mut().unwrap()["name"] = json!("Other");
        }
        if scenario.starts_with("undo/") {
            let p: Vec<_> = scenario.split('/').collect();
            adapter.fault_step.set(p[1].parse().unwrap());
            adapter.reset(p[2]);
        }
        for key in ["undo-key", "wrong-undo-key", "undo-key"] {
            perform(&host, asynchronous, "undo", key, &txid, &mut results, &mut states, &record).await;
        }
        adapter.reset("");
        perform(&host, asynchronous, "undo", "new-undo-key", &txid, &mut results, &mut states, &record).await;
        let label = format!("async={asynchronous} {scenario}");
        same(&json!(results), &row["results"], &format!("{label} results"));
        same(&json!(states), &row["states"], &format!("{label} states"));
        same(&json!(*adapter.calls.borrow()), &row["calls"], &format!("{label} calls"));
        same(&adapter.sim.state.borrow()["arrangement"], &row["arrangement"], &format!("{label} arrangement"));
    }
}
