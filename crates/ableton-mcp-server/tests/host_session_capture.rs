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
    serde_json::from_str(include_str!("fixtures/host-session-capture-oracle.json")).unwrap()
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
                    if v.as_str().is_some_and(|v| v.starts_with("capturemidi_") || v.starts_with("scenecapture_")) {
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
            Value::String(s) if s.starts_with("capturemidi_") || s.starts_with("scenecapture_") => *v = json!("$transaction"),
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
    kind: String,
    fired: Cell<bool>,
    after_invoke: Cell<bool>,
    /// Live's scene refs are positional (`{epoch}:scene:{index}`): once one is deleted, the next scene takes its ref.
    positional: Cell<bool>,
}
impl Adapter {
    fn new(kind: &str) -> Self {
        Self {
            sim: DeterministicLiveSimulator::new(),
            kind: kind.into(),
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
        let deleted = (self.positional.get() && i.operation == "scene.delete")
            .then(|| self.sim.state.borrow()["scenes"].as_array().unwrap().iter().position(|scene| scene["ref"] == i.args["ref"]))
            .flatten();
        let mut result = if no_effect {
            self.fired.set(true);
            json!({"ok":true})
        } else {
            self.sim.invoke(i)?
        };
        if let Some(index) = deleted {
            if let Some(next) = self.sim.state.borrow_mut()["scenes"].get_mut(index) {
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
        if !self.fired.get() && ["apply-invalid-identity", "apply-invalid-fingerprint"].contains(&fault.as_str()) {
            self.fired.set(true);
            let row = if self.kind == "capture-midi" { &mut result["clipIdentities"][0] } else { &mut result };
            row[if fault.ends_with("identity") { "objectIdentity" } else { "createdFingerprint" }] = json!("");
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
async fn session_capture_validation_matches_source() {
    for (index, row) in fixture()["rows"].as_array().unwrap().iter().enumerate() {
        let host = McpHost::new(Rc::new(DeterministicLiveSimulator::new()), McpHostOptions::default()).unwrap();
        let name = format!(
            "live_{}_{}",
            if row["kind"] == "capture-midi" { "capture_midi" } else { "scene_capture" },
            row["action"].as_str().unwrap()
        );
        let got = host
            .dispatch_session_capture_tool(&ToolCall { id: json!(1), name, arguments: Some(row["args"].clone()), asynchronous: true }, None)
            .await
            .unwrap()
            .unwrap()
            .unwrap_or(Value::Null);
        same(&clean(got), &row["result"], &format!("{index} {row}"));
    }
}

async fn perform(
    host: &McpHost,
    kind: &str,
    action: &str,
    key: &str,
    txid: &Value,
    results: &mut Vec<Value>,
    states: &mut Vec<Value>,
    record: &Rc<RefCell<Value>>,
    preabort: bool,
) {
    let id = json!(results.len() + 1);
    let args = json!({
    "transactionId":txid,
    "confirmation":action,
    "idempotencyKey":key}
    );
    let signal = kumi_common::abort::Signal::new();
    if preabort {
        signal.cancel();
    }
    let result = if action == "apply" {
        host.live_capture_apply_async(&id, &args, kind, Some(&signal)).await.unwrap_or(Value::Null)
    } else {
        host.with_undo_watch(&id, &args, async { Ok(host.undo_session_capture_async(&id, &args, None).await) }).await.unwrap()
    };
    results.push(clean(result));
    states.push(clean(record.borrow().clone()));
}

#[tokio::test]
async fn undoing_a_scene_capture_with_a_scene_after_it_is_confirmed_by_identity() {
    // The read after the delete failing first leaves the undo uncertain, for a retry with the same key to settle.
    for fault in ["", "undo-read"] {
        let adapter = Rc::new(Adapter::new("scene-capture"));
        let host = McpHost::new(adapter.clone(), McpHostOptions::default()).unwrap();
        let preview = host.live_capture_preview_async(&json!(1), &json!({}), "scene-capture").await;
        let body: Value = serde_json::from_str(preview["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
        let apply = json!({"transactionId":body["transactionId"],"confirmation":"apply","idempotencyKey":"apply-key"});
        host.live_capture_apply_async(&json!(2), &apply, "scene-capture", None).await.unwrap();
        let record = host.transaction_record(body["transactionId"].as_str().unwrap()).unwrap();
        assert_eq!(record.borrow()["state"], "applied");
        {
            // The producer adds "Later" after the captured scene.
            let mut state = adapter.sim.state.borrow_mut();
            let index = state["scenes"].as_array().unwrap().len();
            state["scenes"]
                .as_array_mut()
                .unwrap()
                .push(json!({"ref":"scene:later","objectIdentity":"sim-object:scene:later","name":"Later","index":index}));
            for track in state["tracks"].as_array_mut().unwrap() {
                let slot = json!({"ref":format!("clip-slot:{}:later", track["ref"].as_str().unwrap()),"parentRef":track["ref"],"objectIdentity":format!("sim-object:clip-slot:{}:later", track["ref"].as_str().unwrap()),"sceneIndex":index,"clipRef":null,"empty":true});
                track["clipSlots"].as_array_mut().unwrap().push(slot);
            }
        }
        adapter.positional.set(true);
        adapter.reset(fault);
        let undo = json!({"transactionId":body["transactionId"],"confirmation":"undo","idempotencyKey":"undo-key"});
        let first = host
            .with_undo_watch(&json!(3), &undo, async { Ok(host.undo_session_capture_async(&json!(3), &undo, None).await) })
            .await
            .unwrap();
        if fault.is_empty() {
            // "Later" has the captured scene's ref now; the captured scene is gone by its identity.
            assert_eq!(record.borrow()["state"], "undone", "{first}");
        } else {
            assert_eq!(record.borrow()["state"], "uncertain", "{first}");
            host.with_undo_watch(&json!(4), &undo, async { Ok(host.undo_session_capture_async(&json!(4), &undo, None).await) })
                .await
                .unwrap();
            assert_eq!(record.borrow()["state"], "undone", "the retry settles it");
        }
        let names: Vec<_> = adapter.sim.state.borrow()["scenes"].as_array().unwrap().iter().map(|s| s["name"].clone()).collect();
        assert_eq!(names.last(), Some(&json!("Later")), "{fault}");
        assert!(!names.contains(&json!("Captured")), "{fault}");
    }
}
#[tokio::test]
async fn a_capture_retry_after_live_restarts_keeps_its_recovery_marker() {
    let adapter = Rc::new(Adapter::new("scene-capture"));
    let host = McpHost::new(adapter.clone(), McpHostOptions::default()).unwrap();
    let preview = host.live_capture_preview_async(&json!(1), &json!({}), "scene-capture").await;
    let body: Value = serde_json::from_str(preview["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    let record = host.transaction_record(body["transactionId"].as_str().unwrap()).unwrap();
    // Live captured the scene, but its answer never came: the apply is uncertain.
    adapter.reset("apply-after");
    let apply = json!({"transactionId":body["transactionId"],"confirmation":"apply","idempotencyKey":"apply-key"});
    host.live_capture_apply_async(&json!(2), &apply, "scene-capture", None).await.unwrap();
    assert_eq!(record.borrow()["state"], "uncertain");
    adapter.reset("");
    adapter.sim.reconnect().unwrap();
    // The retry can't reconcile across the new epoch, and it leaves the record as the marker, with its key.
    let retry = host.live_capture_apply_async(&json!(3), &apply, "scene-capture", None).await.unwrap();
    assert!(retry.to_string().contains("epoch changed"), "{retry}");
    assert_eq!(record.borrow()["state"], "uncertain");
    assert_eq!(record.borrow()["applyKey"], "apply-key");
}
#[tokio::test]
async fn session_capture_apply_and_exact_key_undo_match_source() {
    for row in fixture()["workflows"].as_array().unwrap() {
        let kind = row["kind"].as_str().unwrap();
        let scenario = row["scenario"].as_str().unwrap();
        let adapter = Rc::new(Adapter::new(kind));
        if scenario == "missing-identity" {
            adapter.sim.state.borrow_mut()["tracks"][0]["clips"][0].as_object_mut().unwrap().remove("objectIdentity");
        }
        let host = McpHost::new(adapter.clone(), McpHostOptions::default()).unwrap();
        let preview = host.live_capture_preview_async(&json!(1), &json!({}), kind).await;
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
            {
                let mut s = adapter.sim.state.borrow_mut();
                match scenario {
                    "identity-edit" => s["tracks"][0]["objectIdentity"] = json!("other"),
                    "note-edit" => s["tracks"][0]["clips"][0]["notes"][0]["velocity"] = json!(60),
                    "playing" => s["playback"]["transport"]["playing"] = json!(true),
                    "position" => s["playback"]["transport"]["position"] = json!(2),
                    _ => {}
                }
            }
            if scenario.starts_with("apply-") {
                adapter.reset(scenario);
            }
            for key in ["apply-key", "other-key", "apply-key"] {
                perform(&host, kind, "apply", key, txid, &mut results, &mut states, &record, scenario == "apply-preabort").await;
            }
            adapter.reset("");

            {
                let t = record.borrow();
                let mut s = adapter.sim.state.borrow_mut();
                let reference = if kind == "capture-midi" { &t["created"]["clips"][0]["ref"] } else { &t["created"]["sceneRef"] };
                let created = if kind == "capture-midi" {
                    s["tracks"]
                        .as_array_mut()
                        .unwrap()
                        .iter_mut()
                        .flat_map(|t| t["clips"].as_array_mut().unwrap())
                        .find(|c| c["ref"] == *reference)
                } else {
                    s["scenes"].as_array_mut().unwrap().iter_mut().find(|c| c["ref"] == *reference)
                };
                let index = created.as_ref().map(|c| c["index"].clone());
                if let Some(created) = created {
                    if scenario == "undo-other-identity" {
                        created["objectIdentity"] = json!("other");
                    }
                    if scenario == "undo-other-content" {
                        created["name"] = json!("Manual");
                    }
                    if scenario == "undo-already-absent" {
                        if kind == "capture-midi" {
                            for track in s["tracks"].as_array_mut().unwrap() {
                                track["clips"].as_array_mut().unwrap().retain(|clip| clip["ref"] != *reference);
                                for slot in track["clipSlots"].as_array_mut().unwrap() {
                                    if slot["clipRef"] == *reference {
                                        slot["clipRef"] = Value::Null;
                                        slot["empty"] = json!(true);
                                    }
                                }
                            }
                        } else {
                            s["scenes"].as_array_mut().unwrap().retain(|scene| scene["ref"] != *reference);
                            for track in s["tracks"].as_array_mut().unwrap() {
                                track["clipSlots"].as_array_mut().unwrap().retain(|slot| Some(slot["sceneIndex"].clone()) != index);
                            }
                        }
                    }
                }
            }

            if scenario == "undo-epoch" {
                adapter.sim.reconnect().unwrap();
            }
            if scenario.starts_with("undo-")
                && !["undo-other-identity", "undo-other-content", "undo-epoch", "undo-already-absent"].contains(&scenario)
            {
                adapter.reset(scenario);
            }
            for key in ["undo-key", "other-undo-key", "undo-key"] {
                perform(&host, kind, "undo", key, txid, &mut results, &mut states, &record, false).await;
            }
            adapter.reset("");
            perform(&host, kind, "undo", "new-undo-key", txid, &mut results, &mut states, &record, false).await;
        }

        let label = format!("{kind} {scenario}");
        same(&json!(results), &row["results"], &format!("{label} results"));
        same(&json!(states), &row["states"], &format!("{label} states"));
        same(&json!(*adapter.calls.borrow()), &row["calls"], &format!("{label} calls"));
        same(&adapter.sim.state.borrow(), &row["state"], &format!("{label} state"));
    }
}
