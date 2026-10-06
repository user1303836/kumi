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
    serde_json::from_reader(flate2::read::GzDecoder::new(&include_bytes!("fixtures/host-midi-transform-oracle.json.gz")[..])).unwrap()
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
                    if v.as_str().is_some_and(|v| v.starts_with("miditransform_")) {
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
            Value::String(s) if s.starts_with("miditransform_") => *v = json!("$transaction"),
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
            {
                let mut actual = i.clone();
                if !self.fired.get() && fault == "apply-partial" && i.operation.starts_with("note.") {
                    self.fired.set(true);
                    let field = if i.operation == "note.delete" { "noteIds" } else { "notes" };
                    actual.args[field] = json!(actual.args[field].as_array().unwrap().iter().take(1).cloned().collect::<Vec<_>>());
                }
                self.sim.invoke(&actual)?
            }
        };
        if c.is_some() && !no_effect {
            self.cache.borrow_mut().insert(key, result.clone());
        }
        self.after_invoke.set(true);
        if !self.fired.get()
            && ((fault.ends_with("after") && fault != "apply-note-after")
                || (fault == "apply-note-after" && i.operation.starts_with("note.")))
        {
            self.fired.set(true);
            return Err(LiveError::error("injected operation failure"));
        }
        if !self.fired.get() && ["apply-invalid-identity", "apply-invalid-fingerprint"].contains(&fault.as_str()) {
            self.fired.set(true);
            let row = &mut result;
            row[if fault.ends_with("identity") { "objectIdentity" } else { "createdFingerprint" }] = json!("");
        }

        if !self.fired.get() && fault == "apply-external" && i.operation.starts_with("note.") {
            self.fired.set(true);
            let mut s = self.sim.state.borrow_mut();
            let clip = s["tracks"][0]["clips"].as_array_mut().unwrap().iter_mut().find(|c| c["ref"] == i.args["ref"]).unwrap();
            clip["notes"].as_array_mut().unwrap().push(json!({"id":100000,"pitch":80,"start":2,"duration":1,"velocity":90,"channel":1}));
            return Err(LiveError::error("injected operation failure"));
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
    let mut scene = s["scenes"][0].clone();
    scene["ref"] = json!("scene:scene-2");
    scene["objectIdentity"] = json!("simulator:scene:scene-2");
    scene["index"] = json!(1);
    scene["name"] = json!("Scene 2");
    s["scenes"].as_array_mut().unwrap().push(scene);
    let mut slot = s["tracks"][0]["clipSlots"][0].clone();
    slot["ref"] = json!("clip-slot:track-1:1");
    slot["objectIdentity"] = json!("simulator:clip-slot:track-1:1");
    slot["sceneIndex"] = json!(1);
    slot["clipRef"] = Value::Null;
    slot["empty"] = json!(true);
    s["tracks"][0]["clipSlots"].as_array_mut().unwrap().push(slot);
    let _ = kind;
}
fn input(sim: &DeterministicLiveSimulator, input: &Value) {
    if input.is_object() {
        let mut s = sim.state.borrow_mut();
        s["tracks"][0]["clips"][0]["notes"] = input["notes"].clone();
        s["tracks"][0]["clips"][0]["length"] = input.get("clipLength").cloned().unwrap_or(json!(16));
        if let Some(scale) = input.get("scale") {
            s["tuning"]["scale"] = scale.clone();
        }
        if let Some(chains) = input.get("chains") {
            s["tracks"][0]["devices"][0]["chains"] = chains.clone();
        }
    }
}
fn params(row: &Value) -> Value {
    let mut v = json!({"clipRef":"clip:clip-1","transform":row["spec"]["type"],"params":row["spec"]["params"],"target":{"trackRef":"track:track-1","sceneIndex":1}});
    if row["scope"] != "default" {
        v["scope"] = row["scope"].clone();
    }
    v
}
#[tokio::test]
async fn midi_transform_validation_matches_source() {
    for (index, row) in fixture()["rows"].as_array().unwrap().iter().enumerate() {
        let sim = Rc::new(DeterministicLiveSimulator::new());
        setup(&sim, "session-midi");
        input(&sim, &row["input"]);
        let host = McpHost::new(sim, McpHostOptions::default()).unwrap();
        let name = format!("live_midi_transform_{}", row["action"].as_str().unwrap());
        let got = host
            .dispatch_midi_transform_tool(&ToolCall { id: json!(1), name, arguments: Some(row["args"].clone()), asynchronous: true }, None)
            .await
            .unwrap()
            .unwrap_or_else(|e| Some(json!({"error":e.message()})))
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
        host.live_midi_transform_apply_async(&id, &args, Some(&signal)).await.unwrap_or(Value::Null)
    } else {
        host.with_undo_watch(&id, &args, async { Ok(host.undo_midi_transform_async(&id, &args, None).await) }).await.unwrap()
    };
    results.push(clean(result));
    states.push(clean(record.borrow().clone()));
}
#[tokio::test]
async fn midi_transform_apply_and_exact_key_undo_match_source() {
    for row in fixture()["workflows"].as_array().unwrap() {
        let kind = row["scope"].as_str().unwrap();
        let scenario = row["scenario"].as_str().unwrap();
        let adapter = Rc::new(Adapter::new());
        setup(&adapter.sim, kind);
        input(&adapter.sim, &row["input"]);
        if scenario == "missing-identity" {
            adapter.sim.state.borrow_mut()["tracks"][0]["clips"][0].as_object_mut().unwrap().remove("objectIdentity");
        }
        let host = McpHost::new(adapter.clone(), McpHostOptions::default()).unwrap();
        let args = params(row);
        let preview = host.live_midi_transform_preview_async(&json!(1), &args).await.unwrap();
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
                    "identity-edit" => s["tracks"][0]["clips"][0]["objectIdentity"] = json!("other"),
                    "note-edit" => s["tracks"][0]["clips"][0]["notes"][0]["velocity"] = json!(60),
                    "target-identity" => s["tracks"][0]["clipSlots"][1]["objectIdentity"] = json!("other"),
                    "target-occupied" => s["tracks"][0]["clipSlots"][1]["clipRef"] = json!("clip:external"),
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
                let created = if kind == "duplicate" {
                    s["tracks"][0]["clips"].as_array_mut().unwrap().iter_mut().find(|c| c["ref"] == *reference)
                } else {
                    Some(&mut s["tracks"][0]["clips"][0])
                };
                if let Some(created) = created {
                    if scenario == "undo-other-identity" {
                        created["objectIdentity"] = json!("other");
                    }
                    if scenario == "undo-other-content" {
                        created["notes"][0]["velocity"] = json!(51);
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
async fn an_arrangement_clips_notes_transform_in_place_and_never_into_a_copy() {
    let sim = Rc::new(DeterministicLiveSimulator::new());
    {
        let mut s = sim.state.borrow_mut();
        let mut clip = s["tracks"][0]["clips"][0].clone();
        clip["ref"] = json!("arrangement-clip:track-1:4");
        clip["objectIdentity"] = json!("simulator:arrangement-clip:0");
        clip["name"] = json!("Verse");
        clip["start"] = json!(16);
        s["arrangementClips"].as_array_mut().unwrap().push(json!({"trackRef":"track:track-1","clip":clip}));
    }
    let host = McpHost::new(sim.clone(), McpHostOptions::default()).unwrap();
    let preview = |args: Value| {
        let host = &host;
        async move {
            let mut args = args;
            args["clipRef"] = json!("arrangement-clip:track-1:4");
            let reply = host.live_midi_transform_preview_async(&json!(1), &args).await.unwrap();
            reply["result"]["content"][0]["text"].as_str().unwrap_or_default().to_owned()
        }
    };
    // In place: a transpose previews.
    let text = preview(json!({"transform":"transpose","params":{"semitones":2},"scope":"in-place"})).await;
    assert!(text.contains("transactionId"), "{text}");
    // Into a copy: refused at preview, saying what works, rather than failing at apply.
    let text = preview(
        json!({"transform":"transpose","params":{"semitones":2},"scope":"duplicate","target":{"trackRef":"track:track-1","sceneIndex":1}}),
    )
    .await;
    assert!(text.contains(r#"\"Verse\" is an Arrangement clip: its notes change in place (scope in-place)"#), "{text}");
    // A generative transform writes into a copy by default: refused, saying to run it on a Session clip.
    let text = preview(json!({"transform":"repeat","params":{"times":2}})).await;
    assert!(
        text.contains("a generative transform (repeat) writes into a copy in a Session slot") && text.contains("run it on a Session clip"),
        "{text}"
    );
}
