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
    serde_json::from_str(include_str!("fixtures/host-clip-launch-oracle.json")).unwrap()
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
                    if v.as_str().is_some_and(|v| v.starts_with("cliplaunch_")) {
                        *v = json!("$transaction");
                    } else if ["confirmation", "stopConfirmation"].contains(&k.as_str()) && v.as_str().is_some_and(|s| s.len() == 43) {
                        *v = json!(format!("${k}"));
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
            Value::String(s) if s.starts_with("cliplaunch_") => *v = json!("$transaction"),
            _ => {}
        }
    }
    walk(&mut v);
    v
}
struct Adapter {
    sim: DeterministicLiveSimulator,
    status_patch: RefCell<Value>,
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
            status_patch: RefCell::new(json!({})),
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
        if !self.fired.get() && fault == "apply-ended" {
            self.fired.set(true);
            let keys = active_keys(&self.sim.state.borrow());
            self.sim
                .invoke(&LiveInvocation::new("session.emergency-stop", json!({"expectedTargets":keys,"expectedRecording":"stopped"})))?;
            return Err(LiveError::error("injected operation failure"));
        }
        if !self.fired.get() && fault.ends_with("after") {
            self.fired.set(true);
            return Err(LiveError::error("injected operation failure"));
        }
        if !self.fired.get() && fault == "apply-invalid-result" {
            self.fired.set(true);
            result = json!({"launched":"other","targets":[]});
        }
        if !self.fired.get() && fault == "apply-external" {
            self.fired.set(true);
            self.sim.state.borrow_mut()["playback"]["playingTargets"][0]["sceneRef"] = json!("scene:external");
            self.sim.state.borrow_mut()["playback"]["firedTargets"][0]["sceneRef"] = json!("scene:external");
            result["targets"][0]["sceneRef"] = json!("scene:external");
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
        let mut value = serde_json::to_value(self.sim.status()?).unwrap();
        for (k, v) in self.status_patch.borrow().as_object().unwrap() {
            value[k] = v.clone();
        }
        Ok(serde_json::from_value(value).unwrap())
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
        self.status()
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
fn preview_args() -> Value {
    json!({"slotRef":"clip-slot:track-1:0","outputSafety":{"safe":true,"provenance":"operator observation"}})
}
#[tokio::test]
async fn cliplaunch_validation_matches_source() {
    tokio::task::LocalSet::new()
        .run_until(async {
            for (index, row) in fixture()["rows"].as_array().unwrap().iter().enumerate() {
                let host = Rc::new(McpHost::new(Rc::new(DeterministicLiveSimulator::new()), McpHostOptions::default()).unwrap());
                let name = match row["tool"].as_str().unwrap() {
                    "liveClipLaunchPreviewAsync" => "live_clip_launch_preview",
                    "liveClipLaunchApplyAsync" => "live_clip_launch_apply",
                    "liveClipLaunchStopAsync" => "live_clip_launch_stop",
                    _ => "live_session_emergency_stop",
                };
                let got = match host
                    .dispatch_clip_launch_tool(
                        &ToolCall { id: json!(1), name: name.into(), arguments: Some(row["args"].clone()), asynchronous: true },
                        None,
                    )
                    .await
                    .unwrap()
                {
                    Ok(v) => v.unwrap_or(Value::Null),
                    Err(e) => json!({"error":e.message()}),
                };
                same(&clean(got), &row["result"], &format!("{index} {row}"));
            }
        })
        .await;
}
async fn perform(
    host: &Rc<McpHost>,
    action: &str,
    key: &str,
    body: &Value,
    results: &mut Vec<Value>,
    states: &mut Vec<Value>,
    record: &Rc<RefCell<Value>>,
    aborted: bool,
    concurrent: bool,
) {
    let args = json!({"transactionId":body["transactionId"],"confirmation":body[if action=="apply"{"confirmation"}else{"stopConfirmation"}],"idempotencyKey":key});
    let id = json!(results.len() + 1);
    let signal = kumi_common::abort::Signal::new();
    if aborted {
        signal.cancel();
    }
    let signal = if aborted { Some(&signal) } else { None };
    if concurrent {
        let other_id = json!(results.len() + 2);
        let joined_id = json!(results.len() + 3);
        let mut other = args.clone();
        other["idempotencyKey"] = json!("different");
        let (first, different, joined) = if action == "apply" {
            let (a, b, c) = futures::join!(
                host.live_clip_launch_apply_async(&id, &args, None),
                host.live_clip_launch_apply_async(&other_id, &other, None),
                host.live_clip_launch_apply_async(&joined_id, &args, None)
            );
            (a.unwrap(), b.unwrap(), c.unwrap())
        } else {
            futures::join!(
                host.live_clip_launch_stop_async(&id, &args, None),
                host.live_clip_launch_stop_async(&other_id, &other, None),
                host.live_clip_launch_stop_async(&joined_id, &args, None)
            )
        };
        for v in [first, different, joined] {
            results.push(clean(v.unwrap_or(Value::Null)));
        }
    } else {
        let result = if action == "apply" {
            host.live_clip_launch_apply_async(&id, &args, signal).await.unwrap()
        } else {
            host.live_clip_launch_stop_async(&id, &args, signal).await
        };
        results.push(clean(result.unwrap_or(Value::Null)));
    }
    states.push(clean(record.borrow().clone()));
}
#[tokio::test]
async fn cliplaunch_lifecycle_matches_source() {
    tokio::task::LocalSet::new()
        .run_until(async {
            for row in fixture()["workflows"].as_array().unwrap() {
                let scenario = row["scenario"].as_str().unwrap();
                let adapter = Rc::new(Adapter::new());
                if scenario == "empty-scene" {
                    adapter.sim.state.borrow_mut()["tracks"][0]["clipSlots"][0].as_object_mut().unwrap().remove("clipRef");
                }
                if scenario == "missing-identity" {
                    adapter.sim.state.borrow_mut()["tracks"][0].as_object_mut().unwrap().remove("objectIdentity");
                }
                let host = Rc::new(McpHost::new(adapter.clone(), McpHostOptions::default()).unwrap());
                let preview = host.live_clip_launch_preview_async(&json!(1), &preview_args()).await;
                let body: Value = serde_json::from_str(preview["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
                let mut results = vec![clean(preview)];
                let mut states = vec![];
                if let Some(txid) = body["transactionId"].as_str() {
                    let record = host.transaction_record(txid).unwrap();
                    if scenario == "expire" {
                        record.borrow_mut()["expiresAt"] = json!(0);
                    }
                    if scenario == "epoch" {
                        adapter.sim.reconnect().unwrap();
                    }
                    {
                        let mut s = adapter.sim.state.borrow_mut();
                        match scenario {
                            "scene-edit" => s["scenes"][0]["name"] = json!("Manual"),
                            "identity-edit" => s["scenes"][0]["objectIdentity"] = json!("other"),
                            "slot-edit" => s["tracks"][0]["clipSlots"][0]["objectIdentity"] = json!("other"),
                            "clip-edit" => s["tracks"][0]["clips"][0]["objectIdentity"] = json!("other"),
                            "set-edit" => s["set"]["objectIdentity"] = json!("other"),
                            "armed" => s["tracks"][0]["armed"] = json!(true),
                            "monitoring" => s["tracks"][0]["monitoringState"] = json!("in"),
                            "quantization" => s["playback"]["transport"]["launchQuantization"]["normalized"] = json!("none"),
                            "playing" => s["playback"]["transport"]["playing"] = json!(true),
                            "recording" => s["playback"]["transport"]["sessionRecord"] = json!(true),
                            _ => {}
                        }
                    }

                    if scenario.starts_with("apply-") {
                        adapter.reset(scenario);
                    }
                    for key in ["apply-key", "other-key", "apply-key"] {
                        let concurrent = scenario == "apply-concurrent" && key == "apply-key" && results.len() == 1;
                        perform(&host, "apply", key, &body, &mut results, &mut states, &record, scenario == "apply-preabort", concurrent)
                            .await;
                    }
                    adapter.reset("");
                    {
                        let mut s = adapter.sim.state.borrow_mut();
                        match scenario {
                            "stop-other-scene" => {
                                s["playback"]["playingTargets"][0]["sceneRef"] = json!("scene:external");
                                s["playback"]["firedTargets"][0]["sceneRef"] = json!("scene:external");
                            }
                            "stop-identity-edit" => s["scenes"][0]["objectIdentity"] = json!("other"),
                            "stop-recording" => s["playback"]["transport"]["sessionRecord"] = json!(true),
                            "stop-armed" => s["tracks"][0]["armed"] = json!(true),
                            _ => {}
                        }
                    }
                    if scenario == "stop-already-stopped" {
                        let keys = active_keys(&adapter.sim.state.borrow());
                        adapter
                            .sim
                            .invoke(&LiveInvocation::new(
                                "session.emergency-stop",
                                json!({"expectedTargets":keys,"expectedRecording":"stopped"}),
                            ))
                            .unwrap();
                    }
                    if scenario.starts_with("stop-")
                        && ![
                            "stop-other-scene",
                            "stop-identity-edit",
                            "stop-recording",
                            "stop-armed",
                            "stop-already-stopped",
                            "stop-preabort",
                            "stop-concurrent",
                        ]
                        .contains(&scenario)
                    {
                        adapter.reset(scenario);
                    }
                    for key in ["stop-key", "other-stop-key", "stop-key"] {
                        let concurrent = scenario == "stop-concurrent" && key == "stop-key" && states.len() == 3;
                        perform(&host, "stop", key, &body, &mut results, &mut states, &record, scenario == "stop-preabort", concurrent)
                            .await;
                        if key == "stop-key" {
                            perform(&host, "apply", "apply-key", &body, &mut results, &mut states, &record, false, false).await;
                        }
                    }
                    adapter.reset("");
                    perform(&host, "stop", "new-stop-key", &body, &mut results, &mut states, &record, false, false).await;
                    perform(&host, "apply", "apply-key", &body, &mut results, &mut states, &record, false, false).await;
                }

                same(&json!(results), &row["results"], &format!("{scenario} results"));
                same(&json!(states), &row["states"], &format!("{scenario} states"));
                same(&json!(*adapter.calls.borrow()), &row["calls"], &format!("{scenario} calls"));
                same(&adapter.sim.state.borrow(), &row["state"], &format!("{scenario} state"));
            }
        })
        .await;
}
fn active_keys(state: &Value) -> Vec<String> {
    let mut keys = vec![];
    for t in state["playback"]["firedTargets"].as_array().unwrap().iter().chain(state["playback"]["playingTargets"].as_array().unwrap()) {
        let key = format!("{}|{}|{}", t["trackRef"].as_str().unwrap(), t["clipSlotRef"].as_str().unwrap(), t["sceneRef"].as_str().unwrap());
        if !keys.contains(&key) {
            keys.push(key);
        }
    }
    keys
}
