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
    serde_json::from_reader(flate2::read::GzDecoder::new(&include_bytes!("fixtures/host-clip-action-oracle.json.gz")[..])).unwrap()
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
                    if v.as_str().is_some_and(|v| v.starts_with("clipaction_")) {
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
            Value::String(s) if s.starts_with("clipaction_") => *v = json!("$transaction"),
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
        } else if fault == "apply-silent-no-effect" {
            json!({"changed":true})
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
        if !self.fired.get() && ["apply-length-drift", "apply-length-tolerance"].contains(&fault.as_str()) {
            self.fired.set(true);
            let mut s = self.sim.state.borrow_mut();
            let source = source(&mut s, &self.kind);
            source["length"] = json!(source["length"].as_f64().unwrap() + if fault == "apply-length-drift" { 0.01 } else { 0.0000001 });
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
    clip["loopStart"] = json!(0.5);
    clip["loopEnd"] = json!(3);
    if kind.starts_with("audio") {
        clip["kind"] = json!("audio");
        clip["isAudio"] = json!(true);
    }
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
    let action = kind.split(':').nth(1).unwrap_or("crop");
    let mut args = json!({
    "clipRef":if kind.starts_with("arrangement"){
    "arrangement-clip:track-1:0"}
    else{
    "clip:clip-1"}
    ,
    "action":action}
    );
    if action == "duplicate-region" {
        args["regionStart"] = json!(0.25);
        args["regionEnd"] = json!(1.5);
        args["destination"] = json!(4);
    }
    if ["scrub-start", "move-playing-position"].contains(&action) {
        args["offset"] = json!(0.75);
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
async fn clip_action_validation_matches_source() {
    for (index, row) in fixture()["rows"].as_array().unwrap().iter().enumerate() {
        let sim = Rc::new(DeterministicLiveSimulator::new());
        setup(&sim, "session:crop");
        let host = McpHost::new(sim, McpHostOptions::default()).unwrap();
        let name = format!("live_clip_action_{}", row["action"].as_str().unwrap());
        let got = host
            .dispatch_clip_action_tool(&ToolCall { id: json!(1), name, arguments: Some(row["args"].clone()), asynchronous: true }, None)
            .await
            .unwrap()
            .unwrap()
            .unwrap_or(Value::Null);
        same(&clean(got), &row["result"], &format!("{index} {row}"));
    }
}

#[tokio::test]
async fn clip_content_and_playback_workflows_match_source() {
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
            if scenario == "missing-loop" {
                source.as_object_mut().unwrap().remove("loopStart");
                source.as_object_mut().unwrap().remove("loopEnd");
            }
            if scenario == "null-loop" {
                source["loopStart"] = Value::Null;
                source["loopEnd"] = Value::Null;
            }
            if scenario == "empty-loop" {
                source["loopEnd"] = source["loopStart"].clone();
            }
            if scenario == "backwards-loop" {
                source["loopEnd"] = json!(0);
            }
            if scenario == "full-loop" {
                source["loopStart"] = json!(0);
                source["loopEnd"] = json!(4);
            }
        }

        let host = McpHost::new(adapter.clone(), McpHostOptions::default()).unwrap();
        let mut args = params(kind);
        if scenario == "inside-region" && args["action"] == "duplicate-region" {
            args["destination"] = json!(0);
        }
        let preview = host.live_clip_action_preview_async(&json!(1), &args).await;
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
                let source = source(&mut s, kind);
                if scenario == "identity-edit" {
                    source["objectIdentity"] = json!("other");
                }
                if scenario == "source-content" {
                    source["name"] = json!("Manual");
                }
                if scenario == "loop-drift" {
                    source["loopEnd"] = json!(100);
                }
                if scenario == "playback-drift" {
                    source["playingPosition"] = json!(2);
                }
                if scenario == "is-playing-drift" {
                    source["isPlaying"] = json!(true);
                }
                if scenario == "hierarchy-drift" {
                    s["tracks"][0]["objectIdentity"] = json!("other");
                }
            }
            if scenario.starts_with("apply-") {
                adapter.reset(scenario);
            }
            for key in ["apply-key", "other-key", "apply-key"] {
                let args = json!({
                "transactionId":txid,
                "confirmation":"apply",
                "idempotencyKey":key}
                );
                let signal = kumi_common::abort::Signal::new();
                if scenario == "apply-preabort" {
                    signal.cancel();
                }
                let value = host.live_clip_action_apply_async(&json!(results.len() + 1), &args, Some(&signal)).await.unwrap_or(Value::Null);
                results.push(clean(value));
                states.push(clean(record.borrow().clone()));
            }
        }

        let label = format!("{kind} {scenario}");
        same(&json!(results), &row["results"], &format!("{label} results"));
        same(&json!(states), &row["states"], &format!("{label} states"));
        same(&json!(*adapter.calls.borrow()), &row["calls"], &format!("{label} calls"));
        same(&adapter.sim.state.borrow(), &row["state"], &format!("{label} state"));
    }
}
