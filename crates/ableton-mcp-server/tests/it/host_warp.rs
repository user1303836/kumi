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
    serde_json::from_reader(flate2::read::GzDecoder::new(&include_bytes!("fixtures/host-warp-oracle.json.gz")[..])).unwrap()
}
fn same(a: &Value, b: &Value, label: &str) {
    if canonical_mutation_identity(a).unwrap() == canonical_mutation_identity(b).unwrap() {
        return;
    }
    fn diff(a: &Value, b: &Value, path: &str) -> Option<String> {
        if let (Some(a), Some(b)) = (a.as_object(), b.as_object()) {
            for (k, v) in a {
                if let Some(e) = b.get(k) {
                    if let Some(d) = diff(v, e, &format!("{path}.{k}")) {
                        return Some(d);
                    }
                } else {
                    return Some(format!("{path}.{k} missing expected"));
                }
            }
            for k in b.keys() {
                if !a.contains_key(k) {
                    return Some(format!("{path}.{k} missing actual"));
                }
            }
            return None;
        }
        if let (Some(a), Some(b)) = (a.as_array(), b.as_array()) {
            if a.len() != b.len() {
                return Some(format!("{path} lengths {} != {}", a.len(), b.len()));
            }
            for (i, (a, b)) in a.iter().zip(b).enumerate() {
                if let Some(d) = diff(a, b, &format!("{path}[{i}]")) {
                    return Some(d);
                }
            }
            return None;
        }
        if canonical_mutation_identity(a).unwrap() != canonical_mutation_identity(b).unwrap() {
            let av = a.to_string();
            let bv = b.to_string();
            return Some(format!("{path}: {} != {}", av.chars().take(600).collect::<String>(), bv.chars().take(600).collect::<String>()));
        }
        None
    }
    panic!("{label}: {}", diff(a, b, "$").unwrap_or_default());
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
                    if v.as_str().is_some_and(|v| v.starts_with("warp_")) {
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
            Value::String(s) if s.starts_with("warp_") => *v = json!("$transaction"),
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
    kind: String,
    read_override: RefCell<Option<Value>>,
}
impl Adapter {
    fn new(kind: &str) -> Self {
        Self {
            sim: DeterministicLiveSimulator::new(),
            kind: kind.into(),
            read_override: Default::default(),
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
        if i.operation == "audio.warp-marker.read" {
            self.read_fault()?;
            if let Some(value) = self.read_override.borrow().as_ref() {
                return Ok(value.clone());
            }
            return self.sim.invoke(i);
        }
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
        if !self.fired.get() && ["apply-beat-drift", "apply-sample-drift", "apply-duplicate-marker"].contains(&fault.as_str()) {
            self.fired.set(true);
            let mut s = self.sim.state.borrow_mut();
            let source = source(&mut s, &self.kind);
            if fault == "apply-beat-drift" {
                source["warpMarkers"][0]["beatTime"] = json!(0.25);
            } else if fault == "apply-sample-drift" {
                source["warpMarkers"][0]["sampleTime"] = json!(9);
            } else {
                let marker = source["warpMarkers"][0].clone();
                source["warpMarkers"].as_array_mut().unwrap().push(marker);
            }
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
fn setup(sim: &DeterministicLiveSimulator, kind: &str) {
    let mut s = sim.state.borrow_mut();
    let clip = &mut s["tracks"][0]["clips"][0];
    clip["kind"] = json!("audio");
    clip["isAudio"] = json!(true);
    clip["warpMarkers"] = json!([{
    "sampleTime":0,
    "beatTime":0}
    ,
    {
    "sampleTime":100000,
    "beatTime":2}
    ,
    {
    "sampleTime":176400,
    "beatTime":4}
    ]);
    if kind.starts_with("arrangement") {
        let mut clip = clip.clone();
        clip["ref"] = json!("arrangement-clip:track-1:0");
        clip["objectIdentity"] = json!("simulator:arrangement-clip:0");
        s["arrangementClips"].as_array_mut().unwrap().push(json!({
        "trackRef":"track:track-1",
        "clip":clip}
        ));
    }
}

fn params(kind: &str) -> Value {
    let action = kind.split('-').nth(1).unwrap_or("add");
    let mut args = json!({
    "clipRef":if kind.starts_with("arrangement"){
    "arrangement-clip:track-1:0"}
    else{
    "clip:clip-1"}
    ,
    "action":action,
    "beatTime":if action=="add"{
    1}
    else{
    2}
    }
    );
    if action == "move" {
        args["distance"] = json!(0.5);
    }
    args
}

fn source<'a>(s: &'a mut Value, kind: &str) -> &'a mut Value {
    if kind.starts_with("arrangement") {
        &mut s["arrangementClips"][0]["clip"]
    } else {
        &mut s["tracks"][0]["clips"][0]
    }
}

#[tokio::test]
async fn a_marker_moved_by_a_fraction_is_undone() {
    let sim = Rc::new(DeterministicLiveSimulator::new());
    setup(&sim, "session-move");
    sim.state.borrow_mut()["tracks"][0]["clips"][0]["warpMarkers"]
        .as_array_mut()
        .unwrap()
        .insert(1, json!({"sampleTime":4410,"beatTime":0.1}));
    let host = McpHost::new(sim.clone(), McpHostOptions::default()).unwrap();
    let text = |result: Value| -> Value { serde_json::from_str(result["result"]["content"][0]["text"].as_str().unwrap()).unwrap() };
    let preview = text(
        host.live_warp_marker_preview_async(&json!(2), &json!({"clipRef":"clip:clip-1","action":"move","beatTime":0.1,"distance":0.2}))
            .await,
    );
    let apply = json!({"transactionId":preview["transactionId"],"confirmation":"apply","idempotencyKey":"apply-key"});
    let applied = text(host.live_warp_marker_apply_async(&json!(3), &apply, None).await.unwrap());
    assert_eq!(applied["state"], "applied", "{preview} {applied}");
    // Moved back by -0.2, the marker sits at 0.30000000000000004 - 0.2 = 0.10000000000000003: the same beat.
    let undo = json!({"transactionId":preview["transactionId"],"confirmation":"undo","idempotencyKey":"undo-key"});
    let undone = text(host.undo_warp_marker_async(&json!(4), &undo, None).await);
    assert_eq!(undone["state"], "undone", "{undone}");
}
#[tokio::test]
async fn a_move_too_small_to_change_the_beat_is_refused_at_preview() {
    let sim = Rc::new(DeterministicLiveSimulator::new());
    setup(&sim, "session-move");
    let host = McpHost::new(sim, McpHostOptions::default()).unwrap();
    // The Remote Script can never confirm a move that leaves the marker in place: 2 + 1e-20 is 2, and 2 + 1e-12 is
    // within the tolerance beat times compare with.
    for distance in [0.0, 1e-20, 1e-12] {
        let refused = host
            .live_warp_marker_preview_async(&json!(1), &json!({"clipRef":"clip:clip-1","action":"move","beatTime":2,"distance":distance}))
            .await;
        assert_eq!(refused["error"]["code"], -32602, "{distance}: {refused}");
        assert!(refused["error"]["message"].as_str().unwrap().starts_with("distance"), "{distance}: {refused}");
    }
}
#[tokio::test]
async fn warp_marker_validation_matches_source() {
    for (index, row) in fixture()["rows"].as_array().unwrap().iter().enumerate() {
        let sim = Rc::new(DeterministicLiveSimulator::new());
        setup(&sim, "session-midi");
        let host = McpHost::new(sim, McpHostOptions::default()).unwrap();
        let name = format!("live_warp_marker_{}", row["action"].as_str().unwrap());
        let got = host
            .dispatch_warp_marker_tool(&ToolCall { id: json!(1), name, arguments: Some(row["args"].clone()), asynchronous: true }, None)
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
        host.live_warp_marker_apply_async(&id, &args, Some(&signal)).await.unwrap_or(Value::Null)
    } else {
        host.with_undo_watch(&id, &args, async { Ok(host.undo_warp_marker_async(&id, &args, None).await) }).await.unwrap()
    };
    results.push(clean(result));
    states.push(clean(record.borrow().clone()));
}
#[tokio::test]
async fn warp_marker_apply_and_exact_key_undo_match_source() {
    for row in fixture()["workflows"].as_array().unwrap() {
        let kind = row["kind"].as_str().unwrap();
        let scenario = row["scenario"].as_str().unwrap();
        let adapter = Rc::new(Adapter::new(kind));
        setup(&adapter.sim, kind);
        {
            let mut s = adapter.sim.state.borrow_mut();
            let source = source(&mut s, kind);
            if scenario == "missing-identity" {
                source.as_object_mut().unwrap().remove("objectIdentity");
            }
            if scenario == "missing-markers" {
                source.as_object_mut().unwrap().remove("warpMarkers");
            }
            if scenario == "empty-markers" {
                source["warpMarkers"] = json!([]);
            }
            if scenario == "not-audio" {
                source["isAudio"] = json!(false);
                source["kind"] = json!("midi");
            }
        }

        let host = McpHost::new(adapter.clone(), McpHostOptions::default()).unwrap();
        let args = params(kind);
        let preview = host.live_warp_marker_preview_async(&json!(1), &args).await;
        let body: Value = preview["result"]["content"][0]["text"].as_str().map(|s| serde_json::from_str(s).unwrap()).unwrap_or(json!({}));
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
                    "identity-edit" => source(&mut s, kind)["objectIdentity"] = json!("other"),
                    "source-content" => source(&mut s, kind)["warpMarkers"][0]["beatTime"] = json!(0.25),
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
                let mut s = adapter.sim.state.borrow_mut();
                if scenario == "undo-other-identity" {
                    source(&mut s, kind)["objectIdentity"] = json!("other");
                }
                if scenario == "undo-other-content" {
                    source(&mut s, kind)["warpMarkers"][0]["beatTime"] = json!(0.25);
                }
            }
            if scenario == "undo-sample-drift" {
                source(&mut adapter.sim.state.borrow_mut(), kind)["warpMarkers"][0]["sampleTime"] = json!(123);
            }
            if scenario == "undo-epoch" {
                adapter.sim.reconnect().unwrap();
            }
            if scenario.starts_with("undo-")
                && !["undo-other-identity", "undo-other-content", "undo-sample-drift", "undo-epoch"].contains(&scenario)
            {
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
async fn warp_preview_evidence_matches_source() {
    for row in fixture()["evidence"].as_array().unwrap() {
        let adapter = Rc::new(Adapter::new("session-add"));
        setup(&adapter.sim, "session-add");
        *adapter.read_override.borrow_mut() = Some(row["read"].clone());
        let host = McpHost::new(adapter, McpHostOptions::default()).unwrap();
        let got = host.live_warp_marker_preview_async(&json!(1), &params("session-add")).await;
        same(&clean(got), &row["result"], &format!("read {}", row["read"]));
    }
}
