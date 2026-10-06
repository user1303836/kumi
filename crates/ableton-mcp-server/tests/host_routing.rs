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
    serde_json::from_str(include_str!("fixtures/host-routing-oracle.json")).unwrap()
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
                    if v.as_str().is_some_and(|v| v.starts_with("routing_")) {
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
            Value::String(s) if s.starts_with("routing_") => *v = json!("$transaction"),
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
    pending_snapshot: RefCell<Option<Value>>,
}
impl Adapter {
    fn new() -> Self {
        let sim = DeterministicLiveSimulator::new();
        {
            let mut state = sim.state.borrow_mut();
            let mut other = state["tracks"][0].clone();
            other["ref"] = json!("track:other");
            other["name"] = json!("Other");
            other["objectIdentity"] = json!("simulator:track:other");
            state["tracks"].as_array_mut().unwrap().push(other);
        }
        Self {
            sim,
            calls: Default::default(),
            cache: Default::default(),
            fault: Default::default(),
            fired: Cell::new(false),
            after_invoke: Cell::new(false),
            pending_snapshot: RefCell::new(None),
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
        let null_result = !self.fired.get() && fault.ends_with("-null");
        if fault == "apply-delayed" && !self.fired.get() {
            *self.pending_snapshot.borrow_mut() = Some(self.sim.state.borrow().clone());
        }
        let result = if no_effect { json!({"changed":false}) } else { self.sim.invoke(i)? };
        if no_effect || null_result {
            self.fired.set(true);
        }
        let result = if null_result { Value::Null } else { result };
        self.cache.borrow_mut().insert(key, result.clone());
        if !self.fired.get() && fault == "apply-identity" {
            self.sim.state.borrow_mut()["tracks"][0]["objectIdentity"] = json!("other");
            self.fired.set(true);
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
        if let Some(snapshot) = self.pending_snapshot.borrow_mut().take() {
            self.fired.set(true);
            return serde_json::from_value(snapshot).map_err(|e| LiveError::error(e.to_string()));
        }
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
async fn routing_validation_matches_source() {
    for (index, row) in fixture()["rows"].as_array().unwrap().iter().enumerate() {
        let host = McpHost::new(Rc::new(DeterministicLiveSimulator::new()), McpHostOptions::default()).unwrap();
        let result = host
            .dispatch_routing_tool(
                &ToolCall {
                    id: json!(1),
                    name: if row["tool"] == "liveRoutingPreviewAsync" { "live_routing_preview" } else { "live_routing_apply" }.into(),
                    arguments: Some(row["args"].clone()),
                    asynchronous: true,
                },
                None,
            )
            .await
            .unwrap()
            .map(|v| v.unwrap_or(Value::Null))
            .unwrap_or_else(|e| json!({"error":e.message()}));
        same(&clean(result), &row["result"], &format!("validation {index} {}", row["args"]));
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
        host.live_routing_apply_async(&id, &args, None).await.unwrap_or(Value::Null)
    } else {
        host.with_undo_watch(&id, &args, async { Ok(host.undo_routing_async(&id, &args, None).await) }).await.unwrap()
    };
    results.push(clean(result));
    states.push(clean(record.borrow().clone()));
}
#[tokio::test]
async fn routing_apply_identity_based_undo_and_recovery_match_source() {
    let f = fixture();
    for row in f["workflows"].as_array().unwrap() {
        let mode = row["mode"].as_str().unwrap();
        let scenario = row["scenario"].as_str().unwrap();
        let proposed = &f["modes"][mode];
        let adapter = Rc::new(Adapter::new());
        {
            let mut state = adapter.sim.state.borrow_mut();
            let track = &mut state["tracks"][0];
            if scenario == "preview-cycle" {
                track["routing"]["outputType"] = track["name"].clone();
            }
            if scenario == "missing-identity" {
                track.as_object_mut().unwrap().remove("objectIdentity");
            }
            if scenario == "no-input" {
                track["routing"]["inputType"] = json!("No Input");
                track["routing"]["inputSubRouting"] = Value::Null;
            }
        }
        let host = McpHost::new(adapter.clone(), McpHostOptions::default()).unwrap();
        let mut args = json!({"trackRef":"track:track-1"});
        for (k, v) in proposed.as_object().unwrap() {
            args[k] = v.clone();
        }
        let preview = host.live_routing_preview_async(&json!(1), &args).await.unwrap();
        let body: Value = serde_json::from_str(preview["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
        let mut results = vec![clean(preview)];
        let mut states = vec![];
        if let Some(txid) = body.get("transactionId") {
            let record = host.transaction_record(txid.as_str().unwrap()).unwrap();
            if scenario == "apply-cycle" {
                let mut state = adapter.sim.state.borrow_mut();
                state["tracks"][1]["routing"]["outputType"] = state["tracks"][1]["name"].clone();
            }
            if scenario == "expire" {
                record.borrow_mut()["expiresAt"] = json!(0);
            }
            if scenario == "epoch" {
                adapter.sim.reconnect().unwrap();
            }
            {
                let mut state = adapter.sim.state.borrow_mut();
                let track = &mut state["tracks"][0];
                match scenario {
                    "identity-edit" => track["objectIdentity"] = json!("other"),
                    "routing-edit" => track["routing"]["outputType"] = json!("other"),
                    "arm-edit" => track["armed"] = json!(true),
                    "monitoring-edit" => track["monitoringState"] = json!("in"),
                    _ => {}
                }
            }
            if scenario.starts_with("apply-") {
                adapter.reset(scenario);
            }
            for key in ["apply-key", "other-key", "apply-key"] {
                perform(&host, "apply", key, txid, &mut results, &mut states, &record).await;
            }
            adapter.reset("");
            {
                let mut state = adapter.sim.state.borrow_mut();
                if scenario == "undo-cycle" {
                    state["tracks"][1]["routing"]["outputType"] = state["tracks"][1]["name"].clone();
                }
                let track = &mut state["tracks"][0];
                if scenario == "undo-edit" {
                    let key = proposed.as_object().unwrap().keys().next().unwrap();
                    match key.as_str() {
                        "arm" => track["armed"] = json!(track["armed"] != true),
                        "monitoring" => track["monitoringState"] = json!("in"),
                        _ => track["routing"][key] = json!("Manual"),
                    }
                }
                if scenario == "undo-identity" {
                    track["objectIdentity"] = json!("other");
                }
                if scenario == "undo-moved" {
                    track["ref"] = json!("track:moved");
                    let tracks = state["tracks"].as_array_mut().unwrap();
                    let moved = tracks.remove(0);
                    tracks.push(moved);
                } else if scenario == "undo-ambiguous" {
                    let mut extra = track.clone();
                    extra["ref"] = json!("track:duplicate");
                    state["tracks"].as_array_mut().unwrap().push(extra);
                }
            }
            if scenario.starts_with("undo-")
                && !["undo-edit", "undo-identity", "undo-moved", "undo-ambiguous", "undo-cycle"].contains(&scenario)
            {
                adapter.reset(scenario);
            }
            for (index, key) in ["undo-key", "other-undo-key", "undo-key"].iter().enumerate() {
                let started = std::time::Instant::now();
                perform(&host, "undo", key, txid, &mut results, &mut states, &record).await;
                if mode == "sub" && scenario == "no-input" && index == 0 {
                    assert!(
                        started.elapsed() >= std::time::Duration::from_millis(14500),
                        "routing restoration must observe the readback deadline"
                    );
                }
            }
            adapter.reset("");
            perform(&host, "undo", "new-undo-key", txid, &mut results, &mut states, &record).await;
        }
        let label = format!("{mode} {scenario}");
        same(&json!(results), &row["results"], &format!("{label} results"));
        same(&json!(states), &row["states"], &format!("{label} states"));
        same(&deadline_polls(&json!(*adapter.calls.borrow())), &deadline_polls(&row["calls"]), &format!("{label} calls"));
        let state = adapter.sim.state.borrow();
        let target = if scenario == "undo-moved" { state["tracks"].as_array().unwrap().last().unwrap() } else { &state["tracks"][0] };
        same(target, &row["target"], &format!("{label} target"));
    }
}

// Only repeated identical readback polls vary with wall-clock scheduling. Keep every other call exact.
fn deadline_polls(value: &Value) -> Value {
    let calls = value.as_array().unwrap();
    let mut out = vec![];
    let mut at = 0;
    while at < calls.len() {
        let mut end = at + 1;
        while end < calls.len() && calls[at]["method"] == "snapshot" && calls[end] == calls[at] {
            end += 1;
        }
        if end - at > 3 {
            let mut call = calls[at].clone();
            call["repeatedUntilDeadline"] = json!(true);
            out.push(call);
        } else {
            out.extend_from_slice(&calls[at..end]);
        }
        at = end;
    }
    json!(out)
}

#[tokio::test]
async fn an_input_from_main_is_refused_since_it_crashes_live() {
    // #195: Live 12.4 crashed, losing the producer's unsaved work, when an audio track's input was set to Main.
    let host = McpHost::new(Rc::new(DeterministicLiveSimulator::new()), McpHostOptions::default()).unwrap();
    for input in ["Main", "Master", " Main "] {
        let result = host.live_routing_preview_async(&json!(1), &json!({"trackRef":"track:track-1","inputType":input})).await.unwrap();
        assert_eq!(result["result"]["isError"], true, "{result}");
        let body: Value = serde_json::from_str(result["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
        assert!(body["reason"].as_str().unwrap().starts_with("Live crashes when a track's input is set to Main"), "{body}");
        assert_eq!(body["remediation"], "Record the mix with inputType \"Resampling\" instead.");
    }
    // An output to Main is a track's usual output, and fine.
    let output = host.live_routing_preview_async(&json!(1), &json!({"trackRef":"track:track-1","outputType":"Main"})).await.unwrap();
    assert!(!output.to_string().contains("crashes"), "{output}");
}
