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
    serde_json::from_reader(flate2::read::GzDecoder::new(&include_bytes!("fixtures/host-tuning-oracle.json.gz")[..])).unwrap()
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
                    if v.as_str().is_some_and(|v| v.starts_with("tuning_")) {
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
            Value::String(s) if s.starts_with("tuning_") => *v = json!("$transaction"),
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
        if i.operation == "tuning.read" {
            self.read_fault()?;
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

        if !self.fired.get() && fault == "apply-clamped" {
            self.fired.set(true);
            alter(&mut self.sim.state.borrow_mut(), &self.kind);
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
/// The simulator's tuning as Live 12.4 reports one (#206): its lowest and highest notes are places (a step of
/// the pseudo-octave and an octave), its reference pitch a frequency on one, its note tunings a row a step.
fn setup(sim: &DeterministicLiveSimulator, _: &str) {
    let mut state = sim.state.borrow_mut();
    let system = &mut state["tuning"]["system"];
    system["lowestNote"] = json!({"indexInOctave":0,"octave":-2});
    system["highestNote"] = json!({"indexInOctave":7,"octave":8});
    system["referencePitch"] = json!({"frequency":440,"indexInOctave":9,"octave":3});
    system["noteTunings"] = json!((0..12).map(|note| json!({"note":note,"deviation":0})).collect::<Vec<_>>());
}
fn params(kind: &str) -> Value {
    match kind {
        "scale" => json!({"rootNote":7,"scaleName":"Minor","scaleMode":false}),
        "system" => json!({
            "name":"Custom",
            "lowestNote":{"indexInOctave":2,"octave":-1},
            "highestNote":{"octave":7,"indexInOctave":11},
            "referencePitch":{"frequency":442,"indexInOctave":9,"octave":3}
        }),
        _ => json!({"name":"Custom"}),
    }
}
fn alter(s: &mut Value, kind: &str) {
    if kind == "scale" {
        s["tuning"]["scale"]["rootNote"] = json!(5);
    } else {
        s["tuning"]["system"]["name"] = json!("Manual");
    }
}

#[tokio::test]
async fn note_tunings_are_read_never_set_and_notes_take_lives_shapes() {
    // #206: Live keeps one value a step of the loaded tuning's pseudo-octave, so 128 MIDI-note rows failed in
    // Live on every write. They're refused before anything is read, and so is a note or a reference pitch in
    // any shape but Live's.
    let adapter = Rc::new(Adapter::new("system"));
    setup(&adapter.sim, "system");
    let host = McpHost::new(adapter.clone(), McpHostOptions::default()).unwrap();
    let rows: Vec<_> = (0..128).map(|note| json!({"note":note,"deviation":0})).collect();
    let refused = host.live_tuning_preview_async(&json!(1), &json!({"noteTunings":rows})).await;
    assert_eq!(refused["error"]["message"], NOTE_TUNINGS_FIXED, "{refused}");
    for (args, said) in [
        (json!({"lowestNote":{"note":2,"deviation":5}}), "lowestNote is {indexInOctave, octave}"),
        (json!({"highestNote":{"indexInOctave":2,"octave":1,"cents":0}}), "highestNote is {indexInOctave, octave}"),
        (json!({"referencePitch":{"frequency":442,"note":69}}), "referencePitch is {frequency, indexInOctave, octave}"),
        (json!({"referencePitch":{"frequency":0,"indexInOctave":9,"octave":3}}), "referencePitch is {frequency, indexInOctave, octave}"),
    ] {
        let refused = host.live_tuning_preview_async(&json!(1), &args).await;
        assert!(refused["error"]["message"].as_str().unwrap().starts_with(said), "{args}: {refused}");
    }
    assert!(adapter.calls.borrow().is_empty(), "nothing read or written: {:?}", adapter.calls.borrow());
    // Live's shapes go through, in Live's order, and the read after confirms them exactly.
    let preview = host
        .live_tuning_preview_async(
            &json!(1),
            &json!({"highestNote":{"octave":7,"indexInOctave":11},"referencePitch":{"octave":3,"indexInOctave":9,"frequency":432}}),
        )
        .await;
    let body: Value = serde_json::from_str(preview["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(body["proposed"]["highestNote"].to_string(), r#"{"indexInOctave":11,"octave":7}"#);
    let applied = host
        .live_tuning_apply_async(
            &json!(2),
            &json!({"transactionId":body["transactionId"],"confirmation":"apply","idempotencyKey":"apply-key"}),
            None,
        )
        .await
        .unwrap();
    assert!(applied["result"]["isError"] != true, "{applied}");
    assert_eq!(adapter.sim.state.borrow()["tuning"]["system"]["referencePitch"], json!({"frequency":432,"indexInOctave":9,"octave":3}));
}
#[tokio::test]
async fn tuning_validation_matches_source() {
    for (index, row) in fixture()["rows"].as_array().unwrap().iter().enumerate() {
        let sim = Rc::new(DeterministicLiveSimulator::new());
        setup(&sim, "session-midi");
        let host = McpHost::new(sim, McpHostOptions::default()).unwrap();
        let name = format!("live_tuning_{}", row["action"].as_str().unwrap());
        let got = host
            .dispatch_tuning_tool(&ToolCall { id: json!(1), name, arguments: Some(row["args"].clone()), asynchronous: true }, None)
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
        host.live_tuning_apply_async(&id, &args, Some(&signal)).await.unwrap_or(Value::Null)
    } else {
        host.with_undo_watch(&id, &args, async { Ok(host.undo_tuning_async(&id, &args, None).await) }).await.unwrap()
    };
    results.push(clean(result));
    states.push(clean(record.borrow().clone()));
}
#[tokio::test]
async fn tuning_apply_and_exact_key_undo_match_source() {
    for row in fixture()["workflows"].as_array().unwrap() {
        let kind = row["kind"].as_str().unwrap();
        let scenario = row["scenario"].as_str().unwrap();
        let adapter = Rc::new(Adapter::new(kind));
        setup(&adapter.sim, kind);
        if scenario == "missing-identity" {
            adapter.sim.state.borrow_mut()["set"].as_object_mut().unwrap().remove("objectIdentity");
        }
        if scenario == "null-prior" {
            let mut s = adapter.sim.state.borrow_mut();
            s["tuning"]["system"]["referencePitch"] = Value::Null;
            s["tuning"]["scale"]["rootNote"] = Value::Null;
        }
        if scenario == "missing-prior" {
            let mut s = adapter.sim.state.borrow_mut();
            s["tuning"]["system"].as_object_mut().unwrap().remove("referencePitch");
            s["tuning"]["scale"].as_object_mut().unwrap().remove("rootNote");
        }
        let host = McpHost::new(adapter.clone(), McpHostOptions::default()).unwrap();
        let args = params(kind);
        let preview = host.live_tuning_preview_async(&json!(1), &args).await;
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
                    "identity-edit" => s["set"]["objectIdentity"] = json!("other"),
                    "source-content" => s["tuning"]["system"]["name"] = json!("Manual"),
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
                    s["set"]["objectIdentity"] = json!("other");
                }
                if scenario == "undo-other-content" {
                    alter(&mut s, kind);
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
