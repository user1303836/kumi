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
    serde_json::from_str(include_str!("fixtures/host-structure-oracle.json")).unwrap()
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
                    if v.as_str().is_some_and(|v| v.starts_with("structure_")) {
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
            Value::String(s) if s.starts_with("structure_") => *v = json!("$transaction"),
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
    fault_step: Cell<usize>,
    count: Cell<usize>,
    fired: Cell<bool>,
    read_fail: Cell<bool>,
    compensate_fail: Cell<bool>,
    /// How many reads after a track is made see Live still setting it up, a value changing each time (#203).
    settle_reads: Cell<usize>,
    settling: RefCell<Vec<(Value, usize)>>,
    /// What the first read after the injected fault finds changed on the first track made: "value" (Live's
    /// late setup), or a "clip" or "device" the producer put there.
    after_fault: RefCell<Option<&'static str>>,
    first_created: RefCell<Value>,
}
impl Adapter {
    fn new() -> Self {
        Self {
            sim: DeterministicLiveSimulator::new(),
            calls: Default::default(),
            cache: Default::default(),
            fault: Default::default(),
            fault_step: Cell::new(0),
            count: Cell::new(0),
            fired: Cell::new(false),
            read_fail: Cell::new(false),
            compensate_fail: Cell::new(false),
            settle_reads: Cell::new(0),
            settling: Default::default(),
            after_fault: Default::default(),
            first_created: Default::default(),
        }
    }
    fn reset(&self, fault: &str) {
        *self.fault.borrow_mut() = fault.into();
        self.count.set(0);
        self.fired.set(false);
        self.read_fail.set(false);
        self.compensate_fail.set(false);
    }
    fn read(&self) -> Result<(), LiveError> {
        if self.read_fail.replace(false) {
            Err(LiveError::error("injected authoritative read failure"))
        } else {
            Ok(())
        }
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
        self.calls.borrow_mut().push(clean(json!({"invocation":i,"context":context})));
        self.count.set(self.count.get() + 1);
        let key =
            canonical_mutation_identity(&json!([c.and_then(|c| c.transaction_id.as_ref()), c.and_then(|c| c.idempotency_key.as_ref()), i]))
                .unwrap();
        if c.is_some() {
            if let Some(v) = self.cache.borrow().get(&key) {
                return Ok(v.clone());
            }
        }
        let fault = self.fault.borrow().clone();
        if self.compensate_fail.get() && ["track.delete", "scene.delete"].contains(&i.operation.as_str()) {
            self.compensate_fail.set(false);
            if fault != "compensate-before" {
                let result = self.sim.invoke(i)?;
                if c.is_some() {
                    self.cache.borrow_mut().insert(key, result);
                }
            }
            return Err(LiveError::error("injected compensation failure"));
        }
        let trigger = !self.fired.get() && self.count.get() == self.fault_step.get();
        if trigger {
            self.fired.set(true);
        }
        if trigger && ["before", "nothing", "refusal"].contains(&fault.as_str()) {
            return Err(if fault == "refusal" {
                LiveError::MutationNotDispatched("mutation was not dispatched: ownership changed".into())
            } else {
                LiveError::error(if fault == "nothing" {
                    "Nothing changed in Live: rejected locator"
                } else {
                    "injected operation failure"
                })
            });
        }
        let mut result = if trigger && fault == "no-effect" { json!({"ok":true}) } else { self.sim.invoke(i)? };
        if i.operation == "track.create" {
            if self.first_created.borrow().is_null() {
                *self.first_created.borrow_mut() = result["ref"].clone();
            }
            if self.settle_reads.get() > 0 {
                self.settling.borrow_mut().push((result["ref"].clone(), self.settle_reads.get()));
            }
        }
        if c.is_some() && !(trigger && fault == "no-effect") {
            self.cache.borrow_mut().insert(key, result.clone());
        }
        if trigger {
            if fault == "after" {
                return Err(LiveError::error("injected operation failure"));
            }
            if fault == "read" {
                self.read_fail.set(true);
            }
            if ["wrong-name", "compensate-before", "compensate-after"].contains(&fault.as_str()) {
                result["name"] = json!("Wrong");
                if fault.starts_with("compensate-") {
                    self.compensate_fail.set(true);
                }
            }
            if fault == "bad-fingerprint" {
                result["createdFingerprint"] = json!("bad");
            }
            if fault == "bad-identity" {
                result["objectIdentity"] = json!("changed");
            }
            if fault == "missing-ref" {
                result.as_object_mut().unwrap().remove("ref");
            }
            if fault == "missing-name" {
                result.as_object_mut().unwrap().remove("name");
            }
            if fault == "missing-index" {
                result.as_object_mut().unwrap().remove("index");
            }
            if fault == "null-index" {
                result["index"] = Value::Null;
            }
            if fault == "null-result" {
                result = Value::Null;
            }
            if fault == "changed-owned" {
                let mut state = self.sim.state.borrow_mut();
                for kind in ["tracks", "scenes"] {
                    if let Some(row) = state[kind].as_array_mut().unwrap().iter_mut().find(|r| r["ref"] == result["ref"]) {
                        row["name"] = json!("Changed");
                        break;
                    }
                }
            }
        }
        Ok(result)
    }
}
impl LiveAdapter for Adapter {
    fn status(&self) -> Result<LiveStatus, LiveError> {
        self.sim.status()
    }
    fn snapshot(&self) -> Result<LiveSnapshot, LiveError> {
        self.read()?;
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
        self.read()?;
        {
            let mut state = self.sim.state.borrow_mut();
            for (reference, left) in self.settling.borrow_mut().iter_mut().filter(|(_, left)| *left > 0) {
                *left -= 1;
                if let Some(row) = state["tracks"].as_array_mut().unwrap().iter_mut().find(|row| row["ref"] == *reference) {
                    row["volume"] = json!(row["volume"].as_f64().unwrap_or(0.85) - 0.1);
                }
            }
            if self.fired.get() {
                let change = self.after_fault.borrow_mut().take();
                let drums = state["tracks"].as_array().unwrap().iter().find(|row| row["ref"] == "track:track-1").unwrap().clone();
                let first = self.first_created.borrow().clone();
                if let Some(row) = state["tracks"].as_array_mut().unwrap().iter_mut().find(|row| row["ref"] == first) {
                    match change {
                        Some("value") => row["volume"] = json!(0.25),
                        Some("clip") => {
                            let mut clip = drums["clips"][0].clone();
                            clip["ref"] = json!("clip:producers");
                            clip["objectIdentity"] = json!("simulator:clip:producers");
                            row["clips"] = json!([clip]);
                        }
                        Some("device") => {
                            let mut device = drums["devices"][0].clone();
                            device["ref"] = json!("device:producers");
                            device["parentRef"] = row["ref"].clone();
                            row["devices"].as_array_mut().unwrap().push(device);
                        }
                        _ => {}
                    }
                }
            }
        }
        self.sim.snapshot_async(c, r).await
    }
    async fn discover_async(&self, r: &LiveDiscoveryRequest, c: Option<&LiveOperationContext>) -> Result<LiveDiscoveryResult, LiveError> {
        self.sim.discover_async(r, c).await
    }
    async fn get_async(&self, r: &LiveRef, c: Option<&LiveOperationContext>) -> Result<Option<Value>, LiveError> {
        self.sim.get_async(r, c).await
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
    async fn refresh_status_async(&self, _: Option<&LiveOperationContext>) -> Result<LiveStatus, LiveError> {
        self.sim.status()
    }
}
async fn perform(
    host: &McpHost,
    asynchronous: bool,
    kind: &str,
    key: &str,
    txid: &Value,
    results: &mut Vec<Value>,
    states: &mut Vec<Value>,
    record: &Rc<RefCell<Value>>,
) {
    let id = json!(results.len() + 1);
    let args = json!({"transactionId":txid,"confirmation":kind,"idempotencyKey":key});
    let result = if kind == "apply" {
        if asynchronous {
            host.live_session_structure_apply_async(&id, &args, None).await
        } else {
            host.live_session_structure_apply(&id, &args)
        }
    } else if asynchronous {
        host.with_undo_watch(&id, &args, host.undo_structure_async(&id, &args, None)).await.unwrap_or_else(|e| json!({"error":e.message()}))
    } else {
        host.undo_structure(&id, &args)
    };
    results.push(clean(result));
    states.push(clean(record.borrow().clone()));
}
#[tokio::test]
async fn structure_section_validation_matches_source() {
    for (index, row) in fixture()["rows"].as_array().unwrap().iter().enumerate() {
        let host = McpHost::new(Rc::new(DeterministicLiveSimulator::new()), McpHostOptions::default()).unwrap();
        let tool = row["tool"].as_str().unwrap();
        let name = if tool.contains("Preview") { "live_session_structure_preview" } else { "live_session_structure_apply" };
        let got = host
            .dispatch_structure_tool(
                &ToolCall { id: json!(1), name: name.into(), arguments: Some(row["args"].clone()), asynchronous: tool.ends_with("Async") },
                None,
            )
            .await
            .unwrap()
            .unwrap_or_else(|e| json!({"error":e.message()}));
        same(&clean(got), &row["result"], &format!("row {index} {row}"));
    }
}
#[tokio::test]
async fn structure_apply_compensation_replay_and_undo_workflows_match_source() {
    for row in fixture()["workflows"].as_array().unwrap() {
        let asynchronous = row["async"].as_bool().unwrap();
        let scenario = row["scenario"].as_str().unwrap();
        let adapter = Rc::new(Adapter::new());
        let host = McpHost::new(adapter.clone(), McpHostOptions::default()).unwrap();
        let params = row["preview"].clone();
        let preview = if asynchronous {
            host.live_session_structure_preview_async(&json!(1), &params).await
        } else {
            host.live_session_structure_preview(&json!(1), &params)
        };
        let body: Value = serde_json::from_str(preview["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
        let txid = body["transactionId"].clone();
        let record = host.transaction_record(txid.as_str().unwrap()).unwrap();
        let mut results = vec![clean(preview)];
        let mut states = vec![];
        if scenario == "expire" {
            record.borrow_mut()["expiresAt"] = json!(0);
        }
        if scenario == "epoch" {
            adapter.sim.reconnect().unwrap();
        }
        if scenario == "revision" {
            adapter.sim.state.borrow_mut()["tracks"][0]["name"] = json!("External");
        }
        if scenario.starts_with("apply/") {
            let p: Vec<_> = scenario.split('/').collect();
            adapter.fault_step.set(p[1].parse().unwrap());
            adapter.reset(p[2]);
        }
        for key in ["apply-key", "wrong-key", "apply-key"] {
            perform(&host, asynchronous, "apply", key, &txid, &mut results, &mut states, &record).await;
        }
        adapter.reset("");
        if scenario == "undo-epoch" {
            adapter.sim.reconnect().unwrap();
        }
        if scenario == "undo-missing" {
            let item = record.borrow()["created"].as_array().unwrap().last().unwrap().clone();
            let kind = if item["kind"] == "track" { "tracks" } else { "scenes" };
            adapter.sim.state.borrow_mut()[kind].as_array_mut().unwrap().retain(|r| r["ref"] != item["ref"]);
        }
        if scenario == "undo-changed" {
            let mut state = adapter.sim.state.borrow_mut();
            let item = record.borrow()["created"].as_array().unwrap().last().unwrap().clone();
            let kind = if item["kind"] == "track" { "tracks" } else { "scenes" };
            state[kind].as_array_mut().unwrap().iter_mut().find(|r| r["ref"] == item["ref"]).unwrap()["name"] = json!("Other");
        }
        if scenario == "undo-shifted" || scenario == "undo-ambiguous" {
            let item = record.borrow()["created"].as_array().unwrap().last().unwrap().clone();
            let kind = if item["kind"] == "track" { "tracks" } else { "scenes" };
            let mut state = adapter.sim.state.borrow_mut();
            let rows = state[kind].as_array_mut().unwrap();
            let index = rows.iter().position(|r| r["ref"] == item["ref"]).unwrap();
            if scenario == "undo-shifted" {
                rows[index]["ref"] = json!(format!("{}-shifted", rows[index]["ref"].as_str().unwrap()));
            } else {
                let mut duplicate = rows[index].clone();
                duplicate["ref"] = json!(format!("{}-duplicated", duplicate["ref"].as_str().unwrap()));
                rows.push(duplicate);
            }
        }
        if scenario == "undo-read-before" {
            adapter.read_fail.set(true);
        }
        if scenario.starts_with("undo/") {
            let p: Vec<_> = scenario.split('/').collect();
            adapter.fault_step.set(p[1].parse().unwrap());
            adapter.reset(p[2]);
        }
        for key in ["undo-key", "wrong-undo-key", "undo-key"] {
            perform(&host, asynchronous, "undo", key, &txid, &mut results, &mut states, &record).await;
        }
        adapter.reset("");
        perform(&host, asynchronous, "undo", "new-undo-key", &txid, &mut results, &mut states, &record).await;
        let label = format!("type={} async={asynchronous} {scenario}", row["type"]);
        same(&json!(results), &row["results"], &format!("{label} results"));
        same(&json!(states), &row["states"], &format!("{label} states"));
        same(&json!(*adapter.calls.borrow()), &row["calls"], &format!("{label} calls"));
        same(&adapter.sim.state.borrow(), &row["state"], &format!("{label} state"));
    }
}

async fn preview(host: &McpHost, tracks: Value) -> Value {
    let preview = host.live_session_structure_preview_async(&json!(1), &json!({"tracks":tracks,"scenes":[]})).await;
    let text = preview["result"]["content"][0]["text"].as_str().unwrap_or_else(|| panic!("{preview}"));
    serde_json::from_str::<Value>(text).unwrap()["transactionId"].clone()
}
fn body(result: &Value) -> Value {
    serde_json::from_str(result["result"]["content"][0]["text"].as_str().unwrap_or_else(|| panic!("{result}"))).unwrap()
}
fn track_count(adapter: &Adapter) -> usize {
    adapter.sim.state.borrow()["tracks"].as_array().unwrap().len()
}

#[tokio::test]
async fn a_track_live_goes_on_setting_up_after_making_it_is_kept_and_its_undo_still_removes_it() {
    // #203: Live sets a new track up after making it (its routing, a default track's devices with their
    // saved values), so the Remote Script's fingerprint from the moment it made the track was gone by the
    // bridge's first read: the apply failed, and its cleanup refused too, leaving stray tracks.
    for reads in [1, 2, 4] {
        let adapter = Rc::new(Adapter::new());
        adapter.settle_reads.set(reads);
        let host = McpHost::new(adapter.clone(), McpHostOptions::default()).unwrap();
        let tracks = track_count(&adapter);
        let txid = preview(&host, json!([{"name":"CAP A","kind":"audio"},{"name":"CAP B","kind":"audio"}])).await;
        let applied = host
            .live_session_structure_apply_async(
                &json!(2),
                &json!({"transactionId":txid,"confirmation":"apply","idempotencyKey":"apply-key"}),
                None,
            )
            .await;
        assert_eq!(body(&applied)["state"], "applied", "{reads} reads: {applied}");
        assert_eq!(track_count(&adapter), tracks + 2);
        let undone = host
            .undo_structure_async(&json!(3), &json!({"transactionId":txid,"confirmation":"undo","idempotencyKey":"undo-key"}), None)
            .await
            .unwrap();
        assert_eq!(body(&undone)["state"], "undone", "{reads} reads: {undone}");
        assert_eq!(track_count(&adapter), tracks, "{reads} reads: both tracks are gone again");
    }
}

#[tokio::test]
async fn a_failed_apply_still_removes_a_track_live_changed_and_says_both_causes_when_it_cant() {
    // The second track comes back unconfirmed (another name), so both are cleaned up. The first, which
    // Live changed after its baseline (a late template value), still goes, since nothing was put on it.
    let failed = |after: &'static str| {
        let adapter = Rc::new(Adapter::new());
        adapter.settle_reads.set(1);
        adapter.reset("wrong-name");
        adapter.fault_step.set(2);
        *adapter.after_fault.borrow_mut() = Some(after);
        adapter
    };
    let adapter = failed("value");
    let host = McpHost::new(adapter.clone(), McpHostOptions::default()).unwrap();
    let tracks = track_count(&adapter);
    let txid = preview(&host, json!([{"name":"CAP A","kind":"audio"},{"name":"CAP B","kind":"audio"}])).await;
    let applied = host
        .live_session_structure_apply_async(
            &json!(2),
            &json!({"transactionId":txid,"confirmation":"apply","idempotencyKey":"apply-key"}),
            None,
        )
        .await;
    assert_eq!(body(&applied)["reason"], "Live did not confirm created track", "{applied}");
    assert_eq!(
        track_count(&adapter),
        tracks,
        "the track made first is gone: {applied} {} {:?}",
        host.transaction_record(txid.as_str().unwrap()).unwrap().borrow(),
        adapter.calls.borrow()
    );
    assert_eq!(host.transaction_record(txid.as_str().unwrap()).unwrap().borrow()["state"], "undone");
    // A clip or a device put on the track made first keeps it. The error says why the apply failed, that a
    // retry won't remove what's left, and what's left in Live.
    for change in ["clip", "device"] {
        let adapter = failed(change);
        let host = McpHost::new(adapter.clone(), McpHostOptions::default()).unwrap();
        let txid = preview(&host, json!([{"name":"CAP A","kind":"audio"},{"name":"CAP B","kind":"audio"}])).await;
        let applied = host
            .live_session_structure_apply_async(
                &json!(2),
                &json!({"transactionId":txid,"confirmation":"apply","idempotencyKey":"apply-key"}),
                None,
            )
            .await;
        let reason = body(&applied)["reason"].as_str().unwrap().to_string();
        assert!(reason.starts_with("Session-structure apply failed (Live did not confirm created track)."), "{change}: {reason}");
        assert!(
            reason.contains("changed in Live since (a new name, a clip or a device), so Kumi stopped cleaning up."),
            "{change}: {reason}"
        );
        assert!(reason.contains("Left in Live: track:track-2 \"CAP A\"."), "only CAP A is left: {reason}");
        assert!(reason.contains("A retry won't remove them; ask the producer before deleting any of them."), "{reason}");
        assert_eq!(track_count(&adapter), track_count(&Adapter::new()) + 1, "{change}: CAP A stays, with the producer's {change}");
    }
}
