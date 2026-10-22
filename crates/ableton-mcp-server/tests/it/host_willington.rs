use ableton_mcp_server::{
    host::{helpers::canonical_mutation_identity, McpHost, McpHostOptions},
    live::*,
};
use kumi_common::{abort::Signal, time::now_ms_f64};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    cell::{Cell, RefCell},
    collections::HashMap,
    rc::Rc,
};
fn fixture() -> Value {
    serde_json::from_str(include_str!("fixtures/host-willington-oracle.json")).unwrap()
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
                    if v.as_str().is_some_and(|s| s.starts_with("willington_")) {
                        let raw = v.as_str().unwrap();
                        let prefix = ["recording", "realtime", "mixer", "view", "locjump", "clipset", "willington"]
                            .iter()
                            .find(|p| raw.starts_with(&format!("{p}_")))
                            .unwrap();
                        let suffix = raw.strip_prefix(&format!("{prefix}_")).unwrap();
                        assert_eq!(suffix.len(), 24, "exact transaction ID width: {raw}");
                        assert!(
                            suffix.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_'),
                            "base64url transaction ID: {raw}"
                        );
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
            _ => {}
        }
    }
    walk(&mut v);
    v
}
fn change(root: &mut Value, patch: &Value) {
    let path = patch["path"].as_array().unwrap();
    let mut at = root;
    for key in &path[..path.len() - 1] {
        at = if let Some(k) = key.as_str() { &mut at[k] } else { &mut at[key.as_u64().unwrap() as usize] };
    }
    let key = path.last().unwrap();
    if patch["remove"] == true {
        at.as_object_mut().unwrap().remove(key.as_str().unwrap());
    } else if let Some(key) = key.as_str() {
        at[key] = patch["value"].clone();
    } else {
        at[key.as_u64().unwrap() as usize] = patch["value"].clone();
    }
}
fn context(c: Option<&LiveOperationContext>) -> Value {
    let Some(c) = c else {
        return Value::Null;
    };
    let mut v = json!({"deadline":c.deadline_ms.is_some()});
    if let Some(k) = &c.idempotency_key {
        v["idempotencyKey"] = json!(k);
    }
    if let Some(k) = &c.transaction_id {
        v["transactionId"] = json!(k);
    }
    v
}
struct Adapter {
    sim: DeterministicLiveSimulator,
    calls: RefCell<Vec<Value>>,
    cache: RefCell<HashMap<String, Value>>,
    overrides: RefCell<Value>,
    fault: RefCell<String>,
    fired: Cell<bool>,
    armed: Cell<bool>,
    pending: RefCell<Option<Value>>,
    controller: RefCell<Option<Signal>>,
    endpoint: Value,
    read_override: Option<Value>,
}
impl Adapter {
    fn new(config: &Value) -> Self {
        let sim = DeterministicLiveSimulator::new();
        {
            let mut state = sim.state.borrow_mut();
            let mut other = state["tracks"][0].clone();
            other["ref"] = json!("track:other");
            other["name"] = json!("Other");
            other["objectIdentity"] = json!("simulator:track:other");
            other["devices"] = json!([]);
            for key in ["volumeRef", "panRef", "cueRef", "volumeIdentity", "panIdentity", "cueIdentity"] {
                other["mixer"][key] = Value::Null;
            }
            for key in ["sendRefs", "sendIdentities", "sends"] {
                other["mixer"][key] = json!([]);
            }
            state["tracks"].as_array_mut().unwrap().push(other);
            for t in state["tracks"].as_array_mut().unwrap() {
                t["armed"] = json!(true);
            }
            state["willington"] = json!({"names":{"macro-name":"Macro 1","variation-name":"Variation 1"},"mapping":null,"zone":{"minimum":12,"maximum":104,"fadeMinimum":24,"fadeMaximum":88}});
            for p in config["patches"].as_array().into_iter().flatten() {
                change(&mut state, p);
            }
        }
        let mut overrides = json!({"provenance":"real-live","registryHash":"a".repeat(64)});
        if let Some(o) = config["status"].as_object() {
            overrides.as_object_mut().unwrap().extend(o.clone());
        }
        Self {
            sim,
            calls: Default::default(),
            cache: Default::default(),
            overrides: RefCell::new(overrides),
            fault: Default::default(),
            fired: Cell::new(false),
            armed: Cell::new(false),
            pending: Default::default(),
            controller: Default::default(),
            endpoint: config["endpoint"].clone(),
            read_override: config.get("readOverride").cloned(),
        }
    }
    fn once(&self) -> bool {
        !self.fired.replace(true)
    }
    async fn invoke_impl(&self, i: &LiveInvocation, c: Option<&LiveOperationContext>) -> Result<Value, LiveError> {
        self.calls.borrow_mut().push(clean(json!({"method":"invoke","invocation":i,"context":context(c)})));
        let key =
            canonical_mutation_identity(&json!([c.and_then(|c| c.transaction_id.as_ref()), c.and_then(|c| c.idempotency_key.as_ref()), i]))
                .unwrap();
        if let Some(v) = self.cache.borrow().get(&key).filter(|_| c.is_some_and(|c| c.transaction_id.is_some())) {
            return Ok(v.clone());
        }
        let fault = self.fault.borrow().clone();
        if ["before", "cancel", "refusal"].contains(&fault.as_str()) && self.once() {
            return Err(if fault == "refusal" {
                LiveError::MutationNotDispatched("mutation was not dispatched: ownership changed".into())
            } else {
                LiveError::error(if fault == "cancel" { "operation cancelled before dispatch" } else { "injected operation failure" })
            });
        }
        if fault == "during-invoke" {
            if let Some(s) = self.controller.borrow().as_ref() {
                s.cancel();
            }
        }
        if fault == "no-effect" && self.once() {
            return Ok(json!({"changed":true}));
        }
        let mut result = if i.operation == "realtime.arm" {
            self.armed.set(true);
            tokio::time::sleep(std::time::Duration::from_millis(1)).await;
            let mut result = json!({"host":"127.0.0.1","port":9766,"token":"t".repeat(32),"expiresAt":now_ms_f64()+i.args["ttlMs"].as_f64().unwrap(),"channels":i.args["channels"],"parameterRefs":i.args["parameterRefs"],"packetLimitBytes":512,"ratePerSecond":64,"burst":16});
            if let Some(e) = self.endpoint.as_object() {
                result.as_object_mut().unwrap().extend(e.clone());
            }
            result
        } else if i.operation == "realtime.disarm" {
            self.armed.set(false);
            json!({"armed":false})
        } else if i.operation == "realtime.stats" {
            json!({"armed":self.armed.get(),"accepted":2,"pending":0})
        } else if i.operation.starts_with("willington.device.") {
            fn read(mock: &Value, kind: &Value) -> Value {
                let key = ableton_mcp_server::host::helpers::js_string(kind).unwrap();
                let state = if key.ends_with("-zone") {
                    let mut state = mock["zone"].clone();
                    state["lowerBound"] = json!(if kind == "velocity-zone" { 1 } else { 0 });
                    state["upperBound"] = json!(127);
                    state["deviceIdentity"] = json!("rack");
                    state["targetIdentity"] = json!("chain");
                    state["rackClass"] = json!("InstrumentGroupDevice");
                    state
                } else if kind == "macro-mapping" {
                    json!({"mapping":kumi_common::js::json::stringify(&mock["mapping"]),"parameterValue":0.5,"parameterMin":0,"parameterMax":1,"macroValues":"[0]","targetIdentity":"target","deviceIdentity":"rack"})
                } else {
                    let mut state = json!({});
                    if let Some(name) = mock["names"].get(&key) {
                        state["name"] = name.clone();
                    }
                    state["deviceIdentity"] = json!("rack");
                    state
                };
                let revision = hex::encode(Sha256::digest(kumi_common::js::json::stringify(&state)));
                json!({"state":state,"stateRevision":revision})
            }
            let kind = &i.args["kind"];
            let mut state = self.sim.state.borrow_mut();
            let mock = &mut state["willington"];
            if i.operation == "willington.device.read" {
                self.read_override.clone().unwrap_or_else(|| read(mock, kind))
            } else {
                if i.args.get("expectedStateRevision") != read(mock, kind).get("stateRevision") {
                    return Err(LiveError::error("Willington target changed since preview or after apply; undo refused"));
                }
                let key = ableton_mcp_server::host::helpers::js_string(kind).unwrap();
                if key.ends_with("-zone") {
                    mock["zone"] = i.args["next"].clone();
                } else if kind == "macro-mapping" {
                    mock["mapping"] = i.args["next"]["mapping"].clone();
                } else {
                    if let Some(name) = i.args["next"].get("name") {
                        mock["names"][key] = name.clone();
                    } else {
                        mock["names"].as_object_mut().unwrap().remove(&key);
                    }
                }
                let mut result = json!({"changed":true,"revision":1});
                result.as_object_mut().unwrap().extend(read(mock, kind).as_object().unwrap().clone());
                result
            }
        } else {
            if fault == "delayed" && !self.fired.get() {
                *self.pending.borrow_mut() = Some(self.sim.state.borrow()["playback"].clone());
            }
            self.sim.invoke(i)?
        };
        if fault == "null" && self.once() {
            result = Value::Null;
        }
        if fault == "false" && self.once() {
            result = json!({"recording":"yes","armed":"no"});
        }
        if fault == "no-effect" && self.once() {
            self.sim.state.borrow_mut()["playback"]["transport"]["sessionRecord"] = json!(false);
            self.sim.state.borrow_mut()["playback"]["transport"]["arrangementRecord"] = json!(false);
        }
        if c.is_some_and(|c| c.transaction_id.is_some()) {
            self.cache.borrow_mut().insert(key, result.clone());
        }
        if fault == "after" && self.once() {
            return Err(LiveError::error("injected operation failure"));
        }
        Ok(result)
    }
}
impl LiveAdapter for Adapter {
    fn status(&self) -> Result<LiveStatus, LiveError> {
        let base = self.sim.status()?;
        let mut value = serde_json::to_value(&base).unwrap();
        value.as_object_mut().unwrap().extend(self.overrides.borrow().as_object().unwrap().clone());
        if self.overrides.borrow()["operations"].is_null() {
            let mut operations = base.operations.unwrap();
            operations.extend(
                ["realtime.arm", "realtime.disarm", "realtime.stats", "willington.device.read", "willington.device.set"].map(String::from),
            );
            value["operations"] = json!(operations);
        }
        serde_json::from_value(value).map_err(|e| LiveError::error(e.to_string()))
    }
    fn snapshot(&self) -> Result<LiveSnapshot, LiveError> {
        self.sim.snapshot()
    }
    fn get(&self, r: &LiveRef) -> Result<Option<Value>, LiveError> {
        self.sim.get(r)
    }
    fn invoke(&self, _: &LiveInvocation) -> Result<Value, LiveError> {
        Err(LiveError::error("unexpected sync invoke"))
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
        if *self.fault.borrow() == "read" && self.once() {
            return Err(LiveError::error("injected authoritative read failure"));
        }
        if let Some(playback) = self.pending.borrow_mut().take() {
            let mut v = self.sim.state.borrow().clone();
            v["playback"] = playback;
            self.fired.set(true);
            return serde_json::from_value(v).map_err(|e| LiveError::error(e.to_string()));
        }
        self.sim.snapshot_async(c, r).await
    }
    async fn discover_async(&self, r: &LiveDiscoveryRequest, c: Option<&LiveOperationContext>) -> Result<LiveDiscoveryResult, LiveError> {
        self.calls.borrow_mut().push(clean(json!({"method":"discover","request":r,"context":context(c)})));
        if r.kind == LiveDiscoveryKind::SessionPlayback && *self.fault.borrow() == "playback-read" && self.once() {
            return Err(LiveError::error("injected authoritative read failure"));
        }
        let mut result = self.sim.discover_async(r, c).await?;
        if r.kind == LiveDiscoveryKind::SessionPlayback {
            if let Some(pending) = self.pending.borrow_mut().take() {
                result.items = vec![pending.as_object().unwrap().clone()];
                self.fired.set(true);
            }
        }
        Ok(result)
    }
    async fn get_async(&self, r: &LiveRef, c: Option<&LiveOperationContext>) -> Result<Option<Value>, LiveError> {
        self.calls.borrow_mut().push(clean(json!({"method":"get","reference":r,"context":context(c)})));
        self.sim.get(r)
    }
    async fn invoke_async(&self, i: &LiveInvocation, c: Option<&LiveOperationContext>) -> Result<Value, LiveError> {
        self.invoke_impl(i, c).await
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
        let fault = self.fault.borrow().clone();
        if fault == "status" && self.once() {
            return Err(LiveError::error("injected status failure"));
        }
        if fault == "during-status" {
            if let Some(s) = self.controller.borrow().as_ref() {
                s.cancel();
            }
        }
        self.status()
    }
}
async fn call(host: &Rc<McpHost>, tool: &str, id: usize, args: &Value, signal: Option<&Signal>) -> Value {
    let id = json!(id);
    match tool {
        "willingtonPreview" => host.live_willington_preview_async(&id, args).await.unwrap_or_else(|e| json!({"thrown":e.message()})),
        "willingtonApply" => host.live_willington_apply_async(&id, args, signal).await.unwrap_or(Value::Null),
        "undo" => {
            if args["transactionId"].is_null() {
                ableton_mcp_server::host::helpers::error(
                    &id,
                    -32602,
                    "transactionId, confirmation=undo, and idempotencyKey are required",
                    None,
                )
            } else {
                host.with_undo_watch(&id, args, async { Ok(host.undo_willington_async(&id, args, signal).await) }).await.unwrap()
            }
        }
        _ => panic!("unknown tool {tool}"),
    }
}
#[tokio::test]
async fn willington_validation_matches_source() {
    tokio::task::LocalSet::new()
        .run_until(async {
            for (index, row) in fixture()["rows"].as_array().unwrap().iter().enumerate() {
                let host = Rc::new(McpHost::new(Rc::new(Adapter::new(&json!({}))), McpHostOptions::default()).unwrap());
                let result = call(&host, row["tool"].as_str().unwrap(), 1, &row["args"], None).await;
                same(&clean(result), &row["result"], &format!("validation {index}: {}", row["tool"]));
            }
        })
        .await;
}
#[tokio::test]
async fn willington_workflows_match_source() {
    tokio::task::LocalSet::new()
        .run_until(async {
            for row in fixture()["workflows"].as_array().unwrap() {
                let adapter = Rc::new(Adapter::new(&row["config"]));
                let host = Rc::new(McpHost::new(adapter.clone(), McpHostOptions::default()).unwrap());
                let tx = RefCell::new(None::<String>);
                let mut results = vec![];
                let mut states = vec![];
                async fn step(host: &Rc<McpHost>, adapter: &Rc<Adapter>, step: &Value, id: usize, tx: &RefCell<Option<String>>) -> Value {
                    let mut args = step["args"].clone();
                    if args["transactionId"] == "$transaction" {
                        args["transactionId"] = json!(*tx.borrow());
                    }
                    let signal = Signal::new();
                    *adapter.controller.borrow_mut() = Some(signal.clone());
                    if step["abort"] == true {
                        signal.cancel();
                    }
                    let result = call(host, step["tool"].as_str().unwrap(), id, &args, Some(&signal)).await;
                    if step["tool"].as_str().unwrap().ends_with("Preview") {
                        if let Some(text) = result["result"]["content"][0]["text"].as_str() {
                            if let Ok(v) = serde_json::from_str::<Value>(text) {
                                *tx.borrow_mut() = v["transactionId"].as_str().map(str::to_owned);
                            }
                        }
                    }
                    clean(result)
                }
                for s in row["steps"].as_array().unwrap() {
                    if s.get("patch").is_some() {
                        change(&mut adapter.sim.state.borrow_mut(), &s["patch"]);
                        continue;
                    }
                    if let Some(status) = s["status"].as_object() {
                        adapter.overrides.borrow_mut().as_object_mut().unwrap().extend(status.clone());
                        continue;
                    }
                    if let Some(change) = s["record"].as_object() {
                        if let Some(tx) = tx.borrow().as_ref() {
                            host.transaction_record(tx).unwrap().borrow_mut().as_object_mut().unwrap().extend(change.clone());
                        }
                        continue;
                    }
                    if let Some(fault) = s["fault"].as_str() {
                        *adapter.fault.borrow_mut() = fault.into();
                        adapter.fired.set(false);
                        continue;
                    }
                    if let Some(calls) = s["concurrent"].as_array() {
                        let offset = results.len();
                        results.extend(
                            futures::future::join_all(calls.iter().enumerate().map(|(i, s)| step(&host, &adapter, s, offset + i + 1, &tx)))
                                .await,
                        );
                    } else {
                        results.push(step(&host, &adapter, s, results.len() + 1, &tx).await);
                    }
                    states.push(
                        tx.borrow()
                            .as_ref()
                            .and_then(|tx| host.transaction_record(tx))
                            .map(|r| clean(r.borrow().clone()))
                            .unwrap_or(Value::Null),
                    );
                }
                let label = row["label"].as_str().unwrap();
                same(&json!(results), &row["results"], &format!("{label} results"));
                same(&json!(states), &row["states"], &format!("{label} states"));
                same(&json!(*adapter.calls.borrow()), &row["calls"], &format!("{label} calls"));
                same(&adapter.sim.state.borrow()["willington"], &row["target"], &format!("{label} target"));
            }
        })
        .await;
}
