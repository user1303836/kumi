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
    serde_json::from_reader(flate2::read::GzDecoder::new(&include_bytes!("fixtures/host-automation-oracle.json.gz")[..])).unwrap()
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
                    if v.as_str().is_some_and(|v| v.starts_with("automation_")) {
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
            Value::String(s) if s.starts_with("automation_") => *v = json!("$transaction"),
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
    mutations: Cell<usize>,
    read_override: RefCell<Option<Value>>,
}
impl Adapter {
    fn new(_: &str) -> Self {
        Self {
            sim: DeterministicLiveSimulator::new(),
            mutations: Cell::new(0),
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
        self.mutations.set(0);
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
        if i.operation == "automation.envelope.read" {
            self.read_fault()?;
            if let Some(read) = self.read_override.borrow().as_ref() {
                return Ok(read.clone());
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
        self.mutations.set(self.mutations.get() + 1);
        let selected = !fault.contains("second") || self.mutations.get() == 2;
        if selected && !self.fired.get() && ["before", "cancel", "refusal"].iter().any(|s| fault.ends_with(s)) {
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
        let no_effect = selected && !self.fired.get() && fault.ends_with("no-effect");
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
        if selected && !self.fired.get() && fault.ends_with("after") {
            self.fired.set(true);
            return Err(LiveError::error("injected operation failure"));
        }
        if !self.fired.get() && fault == "apply-corrupt-result" {
            self.fired.set(true);
            result["cleared"] = json!(999);
            result["envelopesRevision"] = json!("wrong");
            result["inserted"] = json!(0);
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
const PREF: &str = "parameter:gain-1";
fn setup(sim: &DeterministicLiveSimulator, kind: &str) {
    let mut s = sim.state.borrow_mut();
    let clip = &mut s["tracks"][0]["clips"][0];
    if !kind.ends_with("absent") && !kind.ends_with("empty") {
        clip["envelopes"] = json!({
        PREF:[{
        "value":0.2,
        "time":0.5}
        ,
        {
        "time":1,
        "value":0.7}
        ,
        {
        "time":2,
        "value":0.3}
        ,
        {
        "time":3,
        "value":0.8}
        ]}
        );
    }
    if kind == "step-identical" {
        clip["envelopes"] = json!({
        PREF:[{
        "time":1,
        "value":0.6}
        ,
        {
        "time":2,
        "value":0.6}
        ]}
        );
    }
    if kind == "clear-existing" {
        clip["envelopes"]["parameter:volume:track-1"] = json!([{
        "time":0,
        "value":0.5}
        ]);
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
    let action = if kind.starts_with("create") {
        "create-envelope"
    } else if kind.starts_with("delete") {
        "delete-envelope"
    } else if kind.starts_with("step") {
        "insert-step"
    } else if kind.starts_with("range") {
        "delete-range"
    } else if kind.starts_with("clear") {
        "clear-envelopes"
    } else {
        "insert"
    };
    let mut args = json!({
    "action":action,
    "clipRef":if kind.starts_with("arrangement"){
    "arrangement-clip:track-1:0"}
    else{
    "clip:clip-1"}
    }
    );
    if action != "clear-envelopes" {
        args["parameterRef"] = json!(PREF);
    }
    match action {
        "insert" => {
            args["points"] = json!([{
            "value":0.6,
            "time":1}
            ,
            {
            "time":2,
            "value":0.4}
            ])
        }
        "insert-step" => {
            args["start"] = json!(1);
            args["length"] = json!(1);
            args["value"] = json!(0.6);
        }
        "delete-range" => {
            args["from"] = json!(0.5);
            args["to"] = json!(2);
        }
        _ => {}
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

fn alter(s: &mut Value, kind: &str) {
    let clip = source(s, kind);
    if clip.get("envelopes").is_none_or(Value::is_null) {
        clip["envelopes"] = json!({});
    }
    clip["envelopes"][PREF] = json!([{
    "time":1,
    "value":0.99}
    ]);
}

#[tokio::test]
async fn automation_validation_matches_source() {
    for (index, row) in fixture()["rows"].as_array().unwrap().iter().enumerate() {
        let sim = Rc::new(DeterministicLiveSimulator::new());
        setup(&sim, row["kind"].as_str().unwrap());
        let host = McpHost::new(sim, McpHostOptions::default()).unwrap();
        let name = format!("live_automation_{}", row["action"].as_str().unwrap());
        let got = host
            .dispatch_automation_tool(&ToolCall { id: json!(1), name, arguments: Some(row["args"].clone()), asynchronous: true }, None)
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
        host.live_automation_apply_async(&id, &args, Some(&signal)).await.unwrap_or(Value::Null)
    } else {
        host.with_undo_watch(&id, &args, async { Ok(host.undo_automation_async(&id, &args, None).await) }).await.unwrap()
    };
    results.push(clean(result));
    states.push(clean(record.borrow().clone()));
}
/// The simulator, with clip rows as the Remote Script sends them: whether a clip has envelopes (`hasEnvelopes`), never
/// which parameters they're on.
struct RemoteShape(DeterministicLiveSimulator);
impl LiveAdapter for RemoteShape {
    fn status(&self) -> Result<LiveStatus, LiveError> {
        self.0.status()
    }
    fn snapshot(&self) -> Result<LiveSnapshot, LiveError> {
        self.0.snapshot()
    }
    fn get(&self, r: &LiveRef) -> Result<Option<Value>, LiveError> {
        self.0.get(r)
    }
    fn invoke(&self, i: &LiveInvocation) -> Result<Value, LiveError> {
        self.0.invoke(i)
    }
    fn subscribe(&self, l: LiveListener) -> Result<Unsubscribe, LiveError> {
        self.0.subscribe(l)
    }
    fn reconnect(&self) -> Result<LiveStatus, LiveError> {
        self.0.reconnect()
    }
}
#[async_trait::async_trait(?Send)]
impl AsyncLiveAdapter for RemoteShape {
    async fn snapshot_async(&self, c: Option<&LiveOperationContext>, r: Option<&LiveSnapshotRequest>) -> Result<LiveSnapshot, LiveError> {
        let mut rows = serde_json::to_value(self.0.snapshot_async(c, r).await?).unwrap();
        for track in rows["tracks"].as_array_mut().into_iter().flatten() {
            for clip in track["clips"].as_array_mut().into_iter().flatten() {
                let has = clip["envelopes"].as_object().is_some_and(|envelopes| !envelopes.is_empty());
                clip.as_object_mut().unwrap().remove("envelopes");
                clip["hasEnvelopes"] = json!(has);
            }
        }
        Ok(serde_json::from_value(rows).unwrap())
    }
    async fn discover_async(&self, r: &LiveDiscoveryRequest, c: Option<&LiveOperationContext>) -> Result<LiveDiscoveryResult, LiveError> {
        self.0.discover_async(r, c).await
    }
    async fn get_async(&self, r: &LiveRef, _: Option<&LiveOperationContext>) -> Result<Option<Value>, LiveError> {
        self.0.get(r)
    }
    async fn invoke_async(&self, i: &LiveInvocation, _: Option<&LiveOperationContext>) -> Result<Value, LiveError> {
        self.0.invoke(i)
    }
    async fn reconnect_async(&self, _: Option<&LiveOperationContext>) -> Result<LiveStatus, LiveError> {
        self.0.reconnect()
    }
    async fn close(&self) -> Result<(), LiveError> {
        Ok(())
    }
}
#[tokio::test]
async fn clearing_envelopes_fences_on_what_live_reports_not_the_snapshot() {
    let live = Rc::new(RemoteShape(DeterministicLiveSimulator::new()));
    setup(&live.0, "clear-existing");
    let host = McpHost::new(live.clone(), McpHostOptions::default()).unwrap();
    let text = |result: Value| -> Value { serde_json::from_str(result["result"]["content"][0]["text"].as_str().unwrap()).unwrap() };
    let preview = text(host.live_automation_preview_async(&json!(1), &json!({"action":"clear-envelopes","clipRef":"clip:clip-1"})).await);
    // The snapshot's rows say only that the clip has envelopes; Live says which, and the preview counts them.
    assert_eq!(preview["envelopes"], 1, "{preview}");
    let apply = json!({"transactionId":preview["transactionId"],"confirmation":"apply","idempotencyKey":"apply-key"});
    let applied = text(host.live_automation_apply_async(&json!(2), &apply, None).await.unwrap());
    assert_eq!((applied["state"].clone(), applied["cleared"].clone()), (json!("applied"), json!(1)), "{applied}");
    assert_eq!(live.0.state.borrow()["tracks"][0]["clips"][0]["envelopes"], json!({}));
}
#[tokio::test]
async fn automation_apply_and_exact_key_undo_match_source() {
    for row in fixture()["workflows"].as_array().unwrap() {
        let kind = row["kind"].as_str().unwrap();
        let scenario = row["scenario"].as_str().unwrap();
        let adapter = Rc::new(Adapter::new(kind));
        setup(&adapter.sim, kind);
        if scenario == "missing-identity" {
            source(&mut adapter.sim.state.borrow_mut(), kind).as_object_mut().unwrap().remove("objectIdentity");
        }
        let host = McpHost::new(adapter.clone(), McpHostOptions::default()).unwrap();
        let args = params(kind);
        let preview = host.live_automation_preview_async(&json!(1), &args).await;
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
                    "parameter-identity" => s["tracks"][0]["devices"][0]["parameters"][0]["objectIdentity"] = json!("other"),
                    "unrelated-content" => source(&mut s, kind)["name"] = json!("Manual"),
                    "source-content" => alter(&mut s, kind),
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
            perform(&host, "undo", "undo-key", txid, &mut results, &mut states, &record, false).await;
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
async fn automation_evidence_and_nested_authority_match_source() {
    let data = fixture();
    for category in ["evidence", "authorities"] {
        for row in data[category].as_array().unwrap() {
            let adapter = Rc::new(Adapter::new(""));
            *adapter.sim.state.borrow_mut() = row["state"].clone();
            if category == "evidence" {
                *adapter.read_override.borrow_mut() = Some(row["read"].clone());
            }
            let host = McpHost::new(adapter, McpHostOptions::default()).unwrap();
            let got = host.live_automation_preview_async(&json!(1), &row["args"]).await;
            same(&clean(got), &row["result"], &format!("{category} {} {}", row["variant"], row["read"]));
        }
    }
}
