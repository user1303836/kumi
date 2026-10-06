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
    serde_json::from_str(include_str!("fixtures/host-arrangement-clip-oracle.json")).unwrap()
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
                    if v.as_str().is_some_and(|v| v.starts_with("arrclip_")) {
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
            Value::String(s) if s.starts_with("arrclip_") => *v = json!("$transaction"),
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
        let mut result = if no_effect {
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
        if !self.fired.get() && ["apply-invalid-identity", "apply-invalid-fingerprint"].contains(&fault.as_str()) {
            self.fired.set(true);
            let row = &mut result;
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
fn setup(_: &DeterministicLiveSimulator, _: &str) {}
fn params(kind: &str) -> Value {
    match kind {
        "audio" => {
            json!({"action":"create","kind":"audio","trackRef":"track:track-1","position":8,"filePath":"/mock/sample.wav","name":"New Audio"})
        }
        "take-lane" => json!({"action":"create","takeLaneRef":"take-lane:track-1:0","position":8,"length":4,"name":"New Take"}),
        _ => json!({"action":"create","trackRef":"track:track-1","position":8,"length":4,"name":"New Clip"}),
    }
}
#[tokio::test]
async fn arrangement_clip_validation_matches_source() {
    for (index, row) in fixture()["rows"].as_array().unwrap().iter().enumerate() {
        let sim = Rc::new(DeterministicLiveSimulator::new());
        setup(&sim, "session-midi");
        let host = McpHost::new(sim, McpHostOptions::default()).unwrap();
        let name = format!("live_arrangement_clip_{}", row["action"].as_str().unwrap());
        let got = host
            .dispatch_arrangement_clip_tool(
                &ToolCall { id: json!(1), name, arguments: Some(row["args"].clone()), asynchronous: true },
                None,
            )
            .await
            .unwrap()
            .unwrap()
            .unwrap_or(Value::Null);
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
    preabort: bool,
) {
    let id = json!(results.len() + 1);
    let args = json!({"transactionId":txid,"confirmation":action,"idempotencyKey":key});
    let signal = kumi_common::abort::Signal::new();
    if preabort {
        signal.cancel();
    }
    let result = if action == "apply" {
        host.live_arrangement_clip_apply_async(&id, &args, Some(&signal)).await.unwrap_or(Value::Null)
    } else {
        host.with_undo_watch(&id, &args, async { Ok(host.undo_arrangement_clip_async(&id, &args, None).await) }).await.unwrap()
    };
    results.push(clean(result));
    states.push(clean(record.borrow().clone()));
}
#[tokio::test]
async fn arrangement_clip_apply_and_exact_key_undo_match_source() {
    for row in fixture()["workflows"].as_array().unwrap() {
        let kind = row["kind"].as_str().unwrap();
        let scenario = row["scenario"].as_str().unwrap();
        let adapter = Rc::new(Adapter::new());
        setup(&adapter.sim, kind);
        if scenario == "missing-identity" {
            let mut s = adapter.sim.state.borrow_mut();
            let row = if kind == "take-lane" { &mut s["tracks"][0]["takeLanes"][0] } else { &mut s["tracks"][0] };
            row.as_object_mut().unwrap().remove("objectIdentity");
        }
        let host = McpHost::new(adapter.clone(), McpHostOptions::default()).unwrap();
        let args = params(kind);
        let preview = host.live_arrangement_clip_preview_async(&json!(1), &args).await;
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
                    "source-content" => s["tracks"][0]["clips"][0]["name"] = json!("Manual"),
                    "target-identity" => s["tracks"][0]["takeLanes"][0]["objectIdentity"] = json!("other"),
                    "target-occupied" => {
                        let mut clip = s["tracks"][0]["clips"][0].clone();
                        clip["ref"] = json!("arrangement-clip:external");
                        s["arrangementClips"].as_array_mut().unwrap().push(json!({"clip":clip,"trackRef":"track:track-1","start":0}));
                    }
                    "lane-siblings" => {
                        let mut clip = s["tracks"][0]["clips"][0].clone();
                        clip["ref"] = json!("clip:take-external");
                        clip["objectIdentity"] = json!("take-external");
                        s["tracks"][0]["takeLanes"][0]["clips"].as_array_mut().unwrap().push(clip);
                    }
                    _ => {}
                }
            }
            if scenario.starts_with("apply-") {
                adapter.reset(scenario);
            }
            for key in ["apply-key", "other-key", "apply-key"] {
                perform(&host, "apply", key, txid, &mut results, &mut states, &record, scenario == "apply-preabort").await;
            }
            adapter.reset("");
            {
                let t = record.borrow();
                let mut s = adapter.sim.state.borrow_mut();
                let reference = &t["created"]["ref"];
                let clips = if kind == "take-lane" { &mut s["tracks"][0]["takeLanes"][0]["clips"] } else { &mut s["arrangementClips"] };
                let created = clips
                    .as_array_mut()
                    .unwrap()
                    .iter_mut()
                    .map(|c| if kind == "take-lane" { c } else { &mut c["clip"] })
                    .find(|c| c["ref"] == *reference);
                if let Some(created) = created {
                    if scenario == "undo-other-identity" {
                        created["objectIdentity"] = json!("other");
                    }
                    if scenario == "undo-other-content" {
                        created["name"] = json!("Manual");
                    }
                }
            }
            if scenario == "undo-epoch" {
                adapter.sim.reconnect().unwrap();
            }
            if scenario.starts_with("undo-") && !["undo-other-identity", "undo-other-content", "undo-epoch"].contains(&scenario) {
                adapter.reset(scenario);
            }
            for key in ["undo-key", "other-undo-key", "undo-key"] {
                perform(&host, "undo", key, txid, &mut results, &mut states, &record, false).await;
            }
            adapter.reset("");
            perform(&host, "undo", "new-undo-key", txid, &mut results, &mut states, &record, false).await;
        }
        let label = format!("{kind} {scenario}");
        same(&json!(results), &row["results"], &format!("{label} results"));
        same(&json!(states), &row["states"], &format!("{label} states"));
        same(&json!(*adapter.calls.borrow()), &row["calls"], &format!("{label} calls"));
        same(&adapter.sim.state.borrow(), &row["state"], &format!("{label} state"));
    }
}

#[tokio::test]
async fn a_take_lane_is_created_on_a_track_and_only_lives_undo_removes_it() {
    let sim = Rc::new(DeterministicLiveSimulator::new());
    let host = McpHost::new(sim, McpHostOptions::default()).unwrap();
    let text = |reply: Value| -> Value { serde_json::from_str(reply["result"]["content"][0]["text"].as_str().unwrap()).unwrap() };
    let preview = host
        .dispatch_arrangement_clip_tool(
            &ToolCall {
                id: json!(1),
                name: "live_arrangement_clip_preview".into(),
                arguments: Some(json!({"action":"create-lane","trackRef":"track:track-1","name":"Comp"})),
                asynchronous: true,
            },
            None,
        )
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let preview = text(preview);
    assert_eq!((preview["action"].clone(), preview["impact"].clone()), (json!("create-lane"), json!("creates-take-lane-live-undo-only")));
    assert_eq!(preview["payload"]["name"], json!("Comp"));
    let transaction = preview["transactionId"].clone();
    let applied = host
        .live_arrangement_clip_apply_async(
            &json!(2),
            &json!({"transactionId":transaction,"confirmation":"apply","idempotencyKey":"lane-apply-0001"}),
            None,
        )
        .await
        .unwrap();
    let applied = text(applied);
    assert_eq!(applied["state"], json!("applied"));
    assert_eq!(applied["result"]["name"], json!("Comp"));
    assert!(applied["result"]["ref"].as_str().unwrap().starts_with("take-lane:"));
    // Live's API deletes no take lane: Kumi's undo says Live's own undo takes it back.
    let undone = host
        .undo_arrangement_clip_async(
            &json!(3),
            &json!({"transactionId":transaction,"confirmation":"undo","idempotencyKey":"lane-undo-0001"}),
            None,
        )
        .await;
    let said = kumi_common::js::json::stringify(&undone);
    assert!(said.contains("Live's own undo takes it back") && said.contains("Undo it in Live (Cmd-Z, or live_song_undo)"), "{undone}");
    assert!(!said.contains("Preview the change again"), "a lane isn't previewed again to undo it: {undone}");
    // A track that isn't there, and a bad name, are refused before Live is asked.
    for args in
        [json!({"action":"create-lane","trackRef":"track:nope"}), json!({"action":"create-lane","trackRef":"track:track-1","name":""})]
    {
        let reply = host
            .dispatch_arrangement_clip_tool(
                &ToolCall { id: json!(4), name: "live_arrangement_clip_preview".into(), arguments: Some(args), asynchronous: true },
                None,
            )
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert!(reply.get("error").is_some() || reply["result"]["isError"] == true, "{reply}");
    }
}
