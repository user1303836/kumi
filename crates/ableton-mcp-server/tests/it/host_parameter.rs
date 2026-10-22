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
    serde_json::from_str(include_str!("fixtures/host-parameter-oracle.json")).unwrap()
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
                    if v.as_str().is_some_and(|v| v.starts_with("parameter_") || v.starts_with("parameters_")) {
                        *v = json!("$transaction");
                    } else if k == "confirmation" && v.as_str().is_some_and(|s| s.len() == 32) {
                        *v = json!("$confirmation");
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
            Value::String(s) if (s.starts_with("parameter_") || s.starts_with("parameters_")) => *v = json!("$transaction"),
            _ => {}
        }
    }
    walk(&mut v);
    v
}
struct Adapter {
    scenario: String,
    sim: DeterministicLiveSimulator,
    calls: RefCell<Vec<Value>>,
    cache: RefCell<HashMap<String, Value>>,
    fault: RefCell<String>,
    fired: Cell<bool>,
    after_invoke: Cell<bool>,
}
impl Adapter {
    fn new(scenario: &str) -> Self {
        Self {
            scenario: scenario.into(),
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
        let result = if no_effect {
            self.fired.set(true);
            json!({"ok":true})
        } else {
            self.sim.invoke(i)?
        };
        if c.is_some() && !no_effect {
            self.cache.borrow_mut().insert(key, result.clone());
        }
        if self.scenario.starts_with("rounded") && !self.after_invoke.get() {
            for parameter in self.sim.state.borrow_mut()["tracks"][0]["devices"][0]["parameters"].as_array_mut().unwrap() {
                parameter["value"] = json!(3);
                parameter["displayValue"] = json!("3");
            }
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
        self.calls.borrow_mut().push(json!({"method":"snapshotSync"}));
        self.read_fault()?;
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
async fn parameter_validation_matches_source() {
    for (index, row) in fixture()["rows"].as_array().unwrap().iter().enumerate() {
        let host = McpHost::new(Rc::new(DeterministicLiveSimulator::new()), McpHostOptions::default()).unwrap();
        let name =
            if row["tool"].as_str().unwrap().contains("Preview") { "live_device_parameter_preview" } else { "live_device_parameter_apply" };
        let got = host
            .dispatch_device_parameter_tool(
                &ToolCall {
                    id: json!(1),
                    name: name.into(),
                    arguments: Some(row["args"].clone()),
                    asynchronous: row["tool"].as_str().unwrap().ends_with("Async"),
                },
                None,
            )
            .await
            .unwrap()
            .unwrap();
        same(&clean(got), &row["result"], &format!("{index} {row}"));
    }
}
async fn perform(
    host: &McpHost,
    mode: &str,
    action: &str,
    key: &str,
    txid: &Value,
    confirmation: &Value,
    results: &mut Vec<Value>,
    states: &mut Vec<Value>,
    record: &Rc<RefCell<Value>>,
) {
    let id = json!(results.len() + 1);
    let args =
        json!({"transactionId":txid,"confirmation":if action=="apply"{confirmation.clone()}else{json!("undo")},"idempotencyKey":key});
    let result = if action == "apply" {
        if mode == "sync" {
            host.live_device_parameter_apply(&id, &args)
        } else {
            host.live_device_parameter_apply_async(&id, &args, None).await
        }
    } else if mode == "sync" {
        host.undo_device_parameter(&id, &args)
    } else {
        host.with_undo_watch(&id, &args, async {
            Ok(if mode == "single" {
                host.undo_device_parameter_async(&id, &args, None).await
            } else {
                host.undo_device_parameters_async(&id, &args, None).await
            })
        })
        .await
        .unwrap()
    };
    results.push(clean(result));
    states.push(clean(record.borrow().clone()));
}
#[tokio::test]
async fn parameter_apply_and_undo_match_source() {
    for row in fixture()["workflows"].as_array().unwrap() {
        let mode = row["mode"].as_str().unwrap();
        let scenario = row["scenario"].as_str().unwrap();
        let adapter = Rc::new(Adapter::new(scenario));
        let count = match mode {
            "multi" => 3,
            "listed" => 5,
            "paged" => 1025,
            _ => 1,
        };
        let original = adapter.sim.state.borrow()["tracks"][0]["devices"][0]["parameters"][0].clone();
        {
            let mut state = adapter.sim.state.borrow_mut();
            let device = &mut state["tracks"][0]["devices"][0];
            device["parameters"] = json!((0..count)
                .map(|i| {
                    let mut parameter = original.clone();
                    if i > 0 {
                        parameter["ref"] = json!(format!("parameter:p{i}"));
                        parameter["objectIdentity"] = json!(format!("parameter-identity-{i}"));
                        parameter["name"] = json!(format!("Knob {i}"));
                    }
                    parameter
                })
                .collect::<Vec<_>>());
            if scenario == "disabled" {
                device["parameters"][0]["enabled"] = json!(false);
            }
            if scenario == "device-disabled" {
                device["enabled"] = json!(false);
            }
            if scenario == "not-automatable" {
                device["parameters"][0]["automatable"] = json!(false);
            }
            if scenario == "quantize" {
                device["parameters"][0]["quantization"] = json!(0.25);
            }
            if scenario.starts_with("rounded") {
                for p in device["parameters"].as_array_mut().unwrap() {
                    p["max"] = json!(8);
                }
            }
        }
        let value = match scenario {
            "clamp" => 8.0,
            "quantize" => 0.6,
            "rounded" => 2.7,
            "rounded-too-far" => 2.2,
            _ => 0.7,
        };
        let args = if count == 1 {
            json!({"deviceRef":"device:utility-1","parameterRef":"parameter:gain-1","value":value})
        } else {
            json!({"deviceRef":"device:utility-1","values":adapter.sim.state.borrow()["tracks"][0]["devices"][0]["parameters"].as_array().unwrap().iter().map(|p|json!({"parameterRef":p["ref"],"value":value})).collect::<Vec<_>>()})
        };
        let host = McpHost::new(adapter.clone(), McpHostOptions::default()).unwrap();
        let preview = if mode == "sync" {
            host.live_device_parameter_preview(&json!(1), &args)
        } else {
            host.live_device_parameter_preview_async(&json!(1), &args).await
        };
        let body: Value = serde_json::from_str(preview["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
        let mut results = vec![clean(preview)];
        let mut states = vec![];
        if let Some(txid) = body.get("transactionId").filter(|_| mode != "paged") {
            let record = host.transaction_record(txid.as_str().unwrap()).unwrap();
            if scenario == "expire" {
                record.borrow_mut()["expiresAt"] = json!(0);
            }
            if scenario == "epoch" {
                adapter.sim.reconnect().unwrap();
            }
            {
                let mut state = adapter.sim.state.borrow_mut();
                if scenario == "track-edit" {
                    state["tracks"][0]["objectIdentity"] = json!("other");
                }
                let device = &mut state["tracks"][0]["devices"][0];
                if scenario == "value-edit" {
                    device["parameters"][0]["value"] = json!(0.3);
                }
                if scenario == "identity-edit" {
                    device["parameters"][0]["objectIdentity"] = json!("other");
                }
                if scenario == "owner-edit" {
                    device["objectIdentity"] = json!("other");
                }
                if scenario == "sibling-edit" {
                    let mut extra = original.clone();
                    extra["ref"] = json!("parameter:extra");
                    extra["objectIdentity"] = json!("extra");
                    device["parameters"].as_array_mut().unwrap().push(extra);
                }
            }
            if scenario.starts_with("apply-") {
                adapter.reset(scenario);
            }
            for key in ["apply-key", "other-key", "apply-key"] {
                perform(&host, mode, "apply", key, txid, &body["confirmation"], &mut results, &mut states, &record).await;
            }
            adapter.reset("");
            {
                let mut state = adapter.sim.state.borrow_mut();
                let parameter = &mut state["tracks"][0]["devices"][0]["parameters"][0];
                if scenario == "undo-value-edit" {
                    parameter["value"] = json!(0.3);
                    parameter["revision"] = json!(20);
                }
                if scenario == "undo-identity-edit" {
                    parameter["objectIdentity"] = json!("other");
                }
            }
            if scenario.starts_with("undo-") && !["undo-value-edit", "undo-identity-edit"].contains(&scenario) {
                adapter.reset(scenario);
            }
            for key in ["undo-key", "other-undo-key", "undo-key"] {
                perform(&host, mode, "undo", key, txid, &body["confirmation"], &mut results, &mut states, &record).await;
            }
            adapter.reset("");
            perform(&host, mode, "undo", "new-undo-key", txid, &body["confirmation"], &mut results, &mut states, &record).await;
        }
        let label = format!("{mode} {scenario}");
        same(&json!(results), &row["results"], &format!("{label} results"));
        same(&json!(states), &row["states"], &format!("{label} states"));
        same(&json!(*adapter.calls.borrow()), &row["calls"], &format!("{label} calls"));
        same(&adapter.sim.state.borrow()["tracks"][0]["devices"][0], &row["device"], &format!("{label} device"));
    }
}
