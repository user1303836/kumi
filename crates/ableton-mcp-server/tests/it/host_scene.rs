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
    serde_json::from_reader(flate2::read::GzDecoder::new(&include_bytes!("fixtures/host-scene-oracle.json.gz")[..])).unwrap()
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
                    if v.as_str().is_some_and(|v| v.starts_with("sceneset_") || v.starts_with("scenefire_")) {
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
            Value::String(s) if s.starts_with("sceneset_") || s.starts_with("scenefire_") => *v = json!("$transaction"),
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
    scenario: String,
    fire_reads: Cell<usize>,
}
impl Adapter {
    fn new(kind: &str, scenario: &str) -> Self {
        Self {
            sim: DeterministicLiveSimulator::new(),
            kind: kind.into(),
            scenario: scenario.into(),
            fire_reads: Cell::new(0),
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
        } else {
            self.sim.invoke(i)?
        };
        if c.is_some() && !no_effect {
            self.cache.borrow_mut().insert(key, result.clone());
        }
        self.after_invoke.set(true);
        self.mutate_fire();
        if !self.fired.get() && fault.ends_with("after") {
            self.fired.set(true);
            return Err(LiveError::error("injected operation failure"));
        }
        if !self.fired.get() && fault == "apply-clamped" {
            self.fired.set(true);
            alter(&mut self.sim.state.borrow_mut(), &self.kind);
        }
        if !self.fired.get() && fault == "apply-float32" {
            self.fired.set(true);
            if let Some(tempo) = i.args.get("tempo").and_then(Value::as_f64) {
                self.sim.state.borrow_mut()["scenes"][0]["tempo"] = json!((tempo as f32) as f64);
            }
        }

        if !self.fired.get() && fault.ends_with("disappear") {
            self.fired.set(true);
            self.sim.state.borrow_mut()["scenes"] = json!([]);
        }
        Ok(result)
    }
    fn mutate_fire(&self) {
        if self.kind != "fire" || !self.scenario.starts_with("fire-") {
            return;
        }
        let mut s = self.sim.state.borrow_mut();
        if let Some(scene) = s["scenes"].as_array_mut().unwrap().first_mut() {
            scene["isTriggered"] = json!(false);
        }
        s["playback"]["transport"]["playing"] = json!(false);
        s["playback"]["firedTargets"] = json!([]);
        s["playback"]["playingTargets"] = json!([]);
        match self.scenario.as_str() {
            "fire-trigger" => s["scenes"][0]["isTriggered"] = json!(true),
            "fire-transport" => s["playback"]["transport"]["playing"] = json!(true),
            "fire-target" => {
                s["playback"]["playingTargets"] = json!([{
                "sceneRef":"scene:scene-1",
                "trackRef":"track:other",
                "clipSlotRef":"clip-slot:other",
                "sceneIndex":0,
                "clipRef":null}
                ])
            }
            "fire-disappear" => {
                s["scenes"] = json!([]);
                s["playback"]["transport"]["playing"] = json!(true);
            }
            _ => {}
        }
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
        if self.after_invoke.get() && self.scenario == "fire-delayed" {
            self.fire_reads.set(self.fire_reads.get() + 1);
            if self.fire_reads.get() == 3 {
                self.sim.state.borrow_mut()["playback"]["transport"]["playing"] = json!(true);
            }
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

fn params(kind: &str) -> Value {
    match kind {
        "all" => json!({
        "ref":"scene:scene-1",
        "colorIndex":20,
        "tempo":130.1,
        "tempoEnabled":true,
        "signatureNumerator":3,
        "signatureDenominator":8,
        "timeSignatureEnabled":true}
        ),
        "tempo" => json!({
        "ref":"scene:scene-1",
        "tempo":140}
        ),
        "signature" => json!({
        "ref":"scene:scene-1",
        "signatureNumerator":3,
        "signatureDenominator":8}
        ),
        "switches" => json!({
        "ref":"scene:scene-1",
        "tempoEnabled":true,
        "timeSignatureEnabled":true}
        ),
        "fire" => json!({
        "ref":"scene:scene-1"}
        ),
        _ => json!({
        "ref":"scene:scene-1",
        "colorIndex":20}
        ),
    }
}

fn alter(s: &mut Value, kind: &str) {
    let scene = &mut s["scenes"][0];
    match kind {
        "tempo" | "all" => scene["tempo"] = json!(180),
        "signature" => scene["signatureNumerator"] = json!(7),
        "switches" => scene["tempoEnabled"] = json!(false),
        _ => scene["colorIndex"] = json!(22),
    }
}

#[tokio::test]
async fn scene_validation_matches_source() {
    for (index, row) in fixture()["rows"].as_array().unwrap().iter().enumerate() {
        let host = McpHost::new(Rc::new(DeterministicLiveSimulator::new()), McpHostOptions::default()).unwrap();
        let name = format!("live_scene_{}{}", if row["family"] == "fire" { "fire_" } else { "" }, row["action"].as_str().unwrap());
        let got = host
            .dispatch_scene_tool(&ToolCall { id: json!(1), name, arguments: Some(row["args"].clone()), asynchronous: true }, None)
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
        if kind == "fire" {
            host.live_scene_fire_apply_async(&id, &args, Some(&signal)).await
        } else {
            host.live_scene_apply_async(&id, &args, Some(&signal)).await
        }
        .unwrap_or(Value::Null)
    } else {
        host.with_undo_watch(&id, &args, async { Ok(host.undo_scene_async(&id, &args, None).await) }).await.unwrap()
    };
    results.push(clean(result));
    states.push(clean(record.borrow().clone()));
}

#[tokio::test]
async fn scene_edit_restore_and_audible_fire_match_source() {
    for row in fixture()["workflows"].as_array().unwrap() {
        let kind = row["kind"].as_str().unwrap();
        let scenario = row["scenario"].as_str().unwrap();
        let adapter = Rc::new(Adapter::new(kind, scenario));
        {
            let mut s = adapter.sim.state.borrow_mut();
            let scene = &mut s["scenes"][0];
            if scenario == "missing-identity" {
                scene.as_object_mut().unwrap().remove("objectIdentity");
            }
            if scenario == "empty" {
                scene["isEmpty"] = json!(true);
            }
            if scenario == "null-prior" || scenario == "missing-prior" {
                for f in ["colorIndex", "tempo", "tempoEnabled", "signatureNumerator", "signatureDenominator", "timeSignatureEnabled"] {
                    if scenario == "null-prior" {
                        scene[f] = Value::Null;
                    } else {
                        scene.as_object_mut().unwrap().remove(f);
                    }
                }
            }
            if ["disabled-prior", "bad-enabled-prior", "high-prior"].contains(&scenario) {
                scene["tempo"] = json!(if scenario == "high-prior" { 1000 } else { -1 });
                scene["signatureNumerator"] = json!(if scenario == "high-prior" { 100 } else { -1 });
                scene["signatureDenominator"] = scene["signatureNumerator"].clone();
                scene["tempoEnabled"] = json!(scenario == "bad-enabled-prior");
                scene["timeSignatureEnabled"] = json!(scenario == "bad-enabled-prior");
            }
            if scenario == "sibling-identity" {
                let mut second = scene.clone();
                second["ref"] = json!("scene:scene-2");
                second["objectIdentity"] = json!("");
                s["scenes"].as_array_mut().unwrap().push(second);
            }
            if scenario == "missing-scene" {
                s["scenes"] = json!([]);
            }
        }

        let host = McpHost::new(adapter.clone(), McpHostOptions::default()).unwrap();
        let args = params(kind);
        let preview = if kind == "fire" {
            host.live_scene_fire_preview_async(&json!(1), &args).await
        } else {
            host.live_scene_preview_async(&json!(1), &args).await
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
            {
                let mut s = adapter.sim.state.borrow_mut();
                if scenario == "identity-edit" {
                    s["scenes"][0]["objectIdentity"] = json!("other");
                }
                if scenario == "source-content" {
                    if kind == "fire" {
                        s["playback"]["transport"]["playing"] = json!(true);
                    } else {
                        alter(&mut s, kind);
                    }
                }
                if scenario == "unrequested-content" {
                    s["scenes"][0]["name"] = json!("Manual");
                }
            }
            if scenario.starts_with("apply-") {
                adapter.reset(scenario);
            }
            for key in ["apply-key", "other-key", "apply-key"] {
                perform(&host, kind, "apply", key, txid, &mut results, &mut states, &record, scenario == "apply-preabort").await;
            }
            adapter.reset("");
            if kind != "fire" {
                {
                    let mut s = adapter.sim.state.borrow_mut();
                    if scenario == "undo-other-identity" {
                        s["scenes"][0]["objectIdentity"] = json!("other");
                    }
                    if scenario == "undo-other-content" {
                        alter(&mut s, kind);
                    }
                    if scenario == "undo-unrequested-content" {
                        s["scenes"][0]["name"] = json!("Manual");
                    }
                }
                if scenario == "undo-epoch" {
                    adapter.sim.reconnect().unwrap();
                }
                if scenario.starts_with("undo-")
                    && !["undo-other-identity", "undo-other-content", "undo-unrequested-content", "undo-epoch"].contains(&scenario)
                {
                    adapter.reset(scenario);
                }
                for key in ["undo-key", "other-undo-key", "undo-key"] {
                    perform(&host, kind, "undo", key, txid, &mut results, &mut states, &record, false).await;
                }
                adapter.reset("");
                perform(&host, kind, "undo", "new-undo-key", txid, &mut results, &mut states, &record, false).await;
            }
        }

        let label = format!("{kind} {scenario}");
        same(&json!(results), &row["results"], &format!("{label} results"));
        same(&json!(states), &row["states"], &format!("{label} states"));
        same(&json!(*adapter.calls.borrow()), &row["calls"], &format!("{label} calls"));
        same(&adapter.sim.state.borrow(), &row["state"], &format!("{label} state"));
    }
}
