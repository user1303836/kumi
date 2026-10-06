use ableton_mcp_server::{
    host::{helpers::canonical_mutation_identity, McpHost, McpHostOptions},
    live::*,
};
use serde_json::{json, Value};
use std::{
    cell::{Cell, RefCell},
    collections::HashMap,
    rc::Rc,
};
fn fixture() -> Value {
    serde_json::from_str(include_str!("fixtures/host-deletion-oracle.json")).unwrap()
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
                    if v.as_str()
                        .is_some_and(|v| ["devdel_", "clipdel_", "trackdel_", "scenedel_", "locatordel_"].iter().any(|p| v.starts_with(p)))
                    {
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
            Value::String(s) if ["devdel_", "clipdel_", "trackdel_", "scenedel_", "locatordel_"].iter().any(|p| s.starts_with(p)) => {
                *v = json!("$transaction")
            }
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
    /// Live's device refs are positional (`{track}:{index}`): once one is deleted, the next device takes its ref.
    positional: Cell<bool>,
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
            positional: Cell::new(false),
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
        let mut result = if no_effect {
            self.fired.set(true);
            json!({"ok":true})
        } else {
            self.sim.invoke(i)?
        };
        if self.positional.get() && i.operation == "device.delete" {
            if let Some(next) = self.sim.state.borrow_mut()["tracks"][0]["devices"].get_mut(0) {
                next["ref"] = i.args["ref"].clone();
            }
        }
        if c.is_some() && !no_effect {
            self.cache.borrow_mut().insert(key, result.clone());
        }
        self.after_invoke.set(true);
        if !self.fired.get() && fault.ends_with("after") {
            self.fired.set(true);
            return Err(LiveError::error("injected operation failure"));
        }
        if !self.fired.get() && ["apply-invalid-identity", "apply-invalid-fingerprint", "apply-no-fingerprint"].contains(&fault.as_str()) {
            self.fired.set(true);
            let row = &mut result;
            row[if fault.ends_with("identity") { "objectIdentity" } else { "createdFingerprint" }] = json!("");
        }

        if fault == "apply-no-fingerprint" {
            result.as_object_mut().unwrap().remove("createdFingerprint");
        }
        Ok(result)
    }
    fn read_fault(&self) -> Result<(), LiveError> {
        let fault = self.fault.borrow();
        if !self.fired.get() && ((fault.ends_with("-read") && self.after_invoke.get()) || fault.ends_with("-current-read")) {
            self.fired.set(true);
            return Err(LiveError::error("injected authoritative read failure"));
        }
        Ok(())
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
#[tokio::test]
async fn deletion_validation_matches_source() {
    let data = fixture();
    for (i, row) in data["rows"].as_array().unwrap().iter().enumerate() {
        let sim = Rc::new(DeterministicLiveSimulator::new());
        let kind = row["kind"].as_str().unwrap();
        *sim.state.borrow_mut() = data["seeds"][kind].clone();
        let host = McpHost::new(sim, McpHostOptions::default()).unwrap();
        let result = match (kind, row["action"].as_str().unwrap()) {
            ("device", "preview") => host.live_device_delete_preview_async(&json!(1), &row["args"]).await,
            ("device", _) => host.live_device_delete_apply_async(&json!(1), &row["args"], None).await.unwrap(),
            (_, "preview") => host.live_deletion_preview_async(&json!(1), &row["args"], kind).await,
            _ => host.live_deletion_apply_async(&json!(1), &row["args"], kind, None).await.unwrap(),
        };
        same(&clean(result), &row["result"], &format!("{i} {row}"));
    }
}
#[tokio::test]
async fn deleting_a_device_with_one_after_it_is_confirmed_by_identity() {
    let data = fixture();
    // The read after the delete failing first leaves it uncertain, for a retry with the same key to settle.
    for fault in ["", "apply-read"] {
        let adapter = Rc::new(Adapter::new());
        *adapter.sim.state.borrow_mut() = data["seeds"]["device"].clone();
        let next = json!({"ref":"device:eq-1","parentRef":"track:track-1","name":"EQ Eight","kind":"audio-effect","parameters":[],"objectIdentity":"simulator:device:eq-1","enabled":true});
        adapter.sim.state.borrow_mut()["tracks"][0]["devices"].as_array_mut().unwrap().push(next);
        adapter.positional.set(true);
        let host = McpHost::new(adapter.clone(), McpHostOptions::default()).unwrap();
        let preview = host.live_device_delete_preview_async(&json!(1), &json!({"ref":"device:utility-1"})).await;
        let body: Value = serde_json::from_str(preview["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
        let record = host.transaction_record(body["transactionId"].as_str().unwrap()).unwrap();
        adapter.reset(fault);
        let apply = json!({"transactionId":body["transactionId"],"confirmation":"apply","idempotencyKey":"apply-key"});
        let first = host.live_device_delete_apply_async(&json!(2), &apply, None).await.unwrap();
        if fault.is_empty() {
            // EQ Eight has the deleted device's ref now; Utility is gone by its identity.
            assert_eq!(record.borrow()["state"], "applied", "{first}");
        } else {
            assert_eq!(record.borrow()["state"], "uncertain", "{first}");
            host.live_device_delete_apply_async(&json!(3), &apply, None).await.unwrap();
            assert_eq!(record.borrow()["state"], "applied", "the retry settles it");
        }
        let left: Vec<_> =
            adapter.sim.state.borrow()["tracks"][0]["devices"].as_array().unwrap().iter().map(|d| d["name"].clone()).collect();
        assert_eq!(left, [json!("EQ Eight")], "{fault}");
    }
}
#[tokio::test]
async fn explicit_deletions_and_retained_undo_refusals_match_source() {
    let data = fixture();
    for row in data["workflows"].as_array().unwrap() {
        let kind = row["kind"].as_str().unwrap();
        let scenario = row["scenario"].as_str().unwrap();
        let typ = data["variants"][kind][0].as_str().unwrap();
        let adapter = Rc::new(Adapter::new());
        *adapter.sim.state.borrow_mut() = data["seeds"][kind].clone();
        if scenario == "missing-identity" {
            adapter.sim.state.borrow_mut()["tracks"][0]["objectIdentity"] = json!("");
        }
        let host = McpHost::new(adapter.clone(), McpHostOptions::default()).unwrap();
        let mut args = json!({});
        args[if typ == "device" { "ref".into() } else { format!("{typ}Ref") }] = data["variants"][kind][1].clone();
        let preview = if typ == "device" {
            host.live_device_delete_preview_async(&json!(1), &args).await
        } else {
            host.live_deletion_preview_async(&json!(1), &args, typ).await
        };
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
            if scenario == "identity-edit" {
                adapter.sim.state.borrow_mut()["tracks"][0]["objectIdentity"] = json!("other");
            }
            if scenario == "content-edit" {
                adapter.sim.state.borrow_mut()["tracks"][0]["devices"][0]["name"] = json!("Manual");
            }
            if scenario == "structure-edit" {
                adapter.sim.state.borrow_mut()["scenes"][0]["name"] = json!("Changed");
            }
            if scenario.starts_with("apply-") {
                adapter.reset(scenario);
            }
            for key in ["apply-key", "other-key", "apply-key"] {
                let signal = kumi_common::abort::Signal::new();
                if scenario == "apply-preabort" {
                    signal.cancel();
                }
                let id = json!(results.len() + 1);
                let p = json!({"transactionId":txid,"confirmation":"apply","idempotencyKey":key});
                let result = if typ == "device" {
                    host.live_device_delete_apply_async(&id, &p, Some(&signal)).await
                } else {
                    host.live_deletion_apply_async(&id, &p, typ, Some(&signal)).await
                }
                .unwrap_or(Value::Null);
                results.push(clean(result));
                states.push(clean(record.borrow().clone()));
            }
            adapter.reset("");
            let id = json!(results.len() + 1);
            let p = json!({"transactionId":txid,"confirmation":"undo","idempotencyKey":"undo-key"});
            results.push(clean(
                host.with_undo_watch(&id, &p, async {
                    Ok(ableton_mcp_server::host::helpers::reason_error(
                        &id,
                        "Kumi can't bring this back; Live's undo can.",
                        "If the producer wants it back, Live's own undo can bring it (Cmd-Z in Live).",
                    ))
                })
                .await
                .unwrap(),
            ));
            states.push(clean(record.borrow().clone()));
        }
        let label = format!("{kind} {scenario}");
        same(&json!(results), &row["results"], &format!("{label} results"));
        same(&json!(states), &row["states"], &format!("{label} states"));
        same(&json!(*adapter.calls.borrow()), &row["calls"], &format!("{label} calls"));
        same(&adapter.sim.state.borrow(), &row["state"], &format!("{label} state"));
    }
}
