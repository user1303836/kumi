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
    serde_json::from_str(include_str!("fixtures/host-data-oracle.json")).unwrap()
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
                    if v.as_str().is_some_and(|v| v.starts_with("data_")) {
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
            Value::String(s) if s.starts_with("data_") => *v = json!("$transaction"),
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
    /// A track put in the second track's place just before the next write (after the host's own check).
    shift_before_write: Cell<bool>,
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
            shift_before_write: Cell::new(false),
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
        if i.operation == "data.set" && self.shift_before_write.replace(false) {
            self.sim.state.borrow_mut()["tracks"][1]["objectIdentity"] = json!("simulator:track:other");
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
            row[if fault.ends_with("identity") { "objectIdentity" } else { "createdFingerprint" }] =
                json!(if fault == "apply-settle" { "settling" } else { "" });
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
#[tokio::test]
async fn saved_text_validation_matches_source() {
    for (index, row) in fixture()["rows"].as_array().unwrap().iter().enumerate() {
        let host = McpHost::new(Rc::new(DeterministicLiveSimulator::new()), McpHostOptions::default()).unwrap();
        let name = format!("live_data_{}", row["action"].as_str().unwrap());
        let got = host
            .dispatch_data_tool(&ToolCall { id: json!(1), name, arguments: Some(row["args"].clone()), asynchronous: true }, None)
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
        host.live_data_apply_async(&id, &args, Some(&signal)).await.unwrap_or(Value::Null)
    } else {
        host.with_undo_watch(&id, &args, async { Ok(host.undo_data_async(&id, &args, None).await) }).await.unwrap()
    };
    results.push(clean(result));
    states.push(clean(record.borrow().clone()));
}
#[tokio::test]
async fn saved_text_apply_and_exact_undo_match_source() {
    let data = fixture();
    for row in data["workflows"].as_array().unwrap() {
        let kind = row["kind"].as_str().unwrap();
        let scenario = row["scenario"].as_str().unwrap();
        let adapter = Rc::new(Adapter::new());
        let owner = if kind.ends_with("track") { "track:track-1" } else { "set:set-1" };
        adapter.sim.invoke(&LiveInvocation::new("data.set", json!({"ref":owner,"key":"kumi.test","value":"before"}))).unwrap();
        if scenario == "missing-identity" {
            adapter.sim.state.borrow_mut()["tracks"][0]["objectIdentity"] = json!("");
        }
        let host = McpHost::new(adapter.clone(), McpHostOptions::default()).unwrap();
        let mut args = json!({"key":"kumi.test","value":if kind.starts_with("clear"){Value::Null}else{json!("new text")}});
        if kind.ends_with("track") {
            args["trackRef"] = json!("track:track-1");
        }
        let preview = host.live_data_preview_async(&json!(1), &args).await;
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
                    s["tracks"][0]["objectIdentity"] = json!("other");
                }
            }
            if scenario == "data-edit" {
                adapter.sim.invoke(&LiveInvocation::new("data.set", json!({"ref":owner,"key":"kumi.test","value":"manual"}))).unwrap();
            }
            if scenario.starts_with("apply-") {
                adapter.reset(scenario);
            }
            for key in ["apply-key", "other-key", "apply-key"] {
                perform(&host, "apply", key, txid, &mut results, &mut states, &record, scenario == "apply-preabort").await;
            }
            adapter.reset("");
            if scenario == "undo-other-identity" {
                adapter.sim.state.borrow_mut()["tracks"][0]["objectIdentity"] = json!("other");
            }
            if scenario == "undo-other-content" {
                adapter.sim.invoke(&LiveInvocation::new("data.set", json!({"ref":owner,"key":"kumi.test","value":"manual"}))).unwrap();
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
        same(
            &adapter.sim.invoke(&LiveInvocation::new("data.get", json!({"ref":owner,"key":"kumi.test"}))).unwrap(),
            &row["saved"],
            &format!("{label} saved"),
        );
    }
}
async fn apply_batch(host: &McpHost, preview: &Value, key: &str) -> Value {
    let body: Value = serde_json::from_str(preview["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    let args = json!({"transactionId":body["transactionId"],"confirmation":"apply","idempotencyKey":key});
    host.live_data_apply_async(&json!(2), &args, None).await.unwrap()
}
#[tokio::test]
async fn several_tracks_take_text_in_one_apply_each_track_checked_first() {
    // Kumi's own ids for many tracks at once: every track checked to be the one read before any is written,
    // then each one's text, sent together.
    let setup = || {
        let adapter = Rc::new(Adapter::new());
        {
            let mut state = adapter.sim.state.borrow_mut();
            let mut bass = state["tracks"][0].clone();
            bass["ref"] = json!("track:track-2");
            bass["objectIdentity"] = json!("simulator:track:track-2");
            bass["name"] = json!("Bass");
            state["tracks"].as_array_mut().unwrap().push(bass);
        }
        let host = McpHost::new(adapter.clone(), McpHostOptions::default()).unwrap();
        (adapter, host)
    };
    let batch = |second_value: Value, second_identity: &str| {
        json!({"key":"kumi.track","value":"01J00000000000000000000001","trackRef":"track:track-1","expectedValue":null,
            "expectedIdentity":"simulator:track:track-1","entries":[{"trackRef":"track:track-2","value":"01J00000000000000000000002",
            "expectedValue":second_value,"expectedIdentity":second_identity}]})
    };
    let saved = |adapter: &Adapter, track: &str| {
        adapter.sim.invoke(&LiveInvocation::new("data.get", json!({"ref":track,"key":"kumi.track"}))).unwrap()["value"].clone()
    };
    // Both tracks, in one call to Live.
    let (adapter, host) = setup();
    let preview = host.live_data_preview_async(&json!(1), &batch(Value::Null, "simulator:track:track-2")).await;
    let applied = apply_batch(&host, &preview, "batch-key").await;
    assert!(applied.to_string().contains(r#"\"state\":\"applied\""#), "{applied}");
    assert_eq!(
        (saved(&adapter, "track:track-1"), saved(&adapter, "track:track-2")),
        (json!("01J00000000000000000000001"), json!("01J00000000000000000000002"))
    );
    let writes: Vec<Value> = adapter.calls.borrow().iter().filter(|c| c["invocation"]["operation"] == "data.set").cloned().collect();
    assert_eq!(writes.len(), 1, "both tracks' text in one call to Live: {writes:?}");
    assert_eq!(writes[0]["invocation"]["args"]["entries"].as_array().map(Vec::len), Some(2), "{writes:?}");
    // Each place names the track read there: Live writes by place.
    let identities: Vec<&Value> =
        writes[0]["invocation"]["args"]["entries"].as_array().unwrap().iter().map(|e| &e["expectedObjectIdentity"]).collect();
    assert_eq!(identities, [&json!("simulator:track:track-1"), &json!("simulator:track:track-2")]);
    let body: Value = serde_json::from_str(preview["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    let undo = host
        .undo_data_async(&json!(3), &json!({"transactionId":body["transactionId"],"confirmation":"undo","idempotencyKey":"undo-key"}), None)
        .await;
    assert!(undo.to_string().contains("isn't taken back"), "{undo}");
    // Another track there now: nothing is written.
    let (adapter, host) = setup();
    let preview = host.live_data_preview_async(&json!(1), &batch(Value::Null, "simulator:track:other")).await;
    let applied = apply_batch(&host, &preview, "batch-key").await;
    assert!(applied.to_string().contains("nothing was saved"), "{applied}");
    assert_eq!((saved(&adapter, "track:track-1"), saved(&adapter, "track:track-2")), (Value::Null, Value::Null));
    // Text that changed on one track since it was read: all or none, so nothing is saved, and the apply says so.
    let (adapter, host) = setup();
    let preview = host.live_data_preview_async(&json!(1), &batch(json!("01J0000000000000000000000X"), "simulator:track:track-2")).await;
    let applied = apply_batch(&host, &preview, "batch-key").await;
    assert!(applied.to_string().contains("nothing was saved"), "{applied}");
    assert_eq!((saved(&adapter, "track:track-1"), saved(&adapter, "track:track-2")), (Value::Null, Value::Null));
    // A track named twice, or a place without what was read there, is refused before anything is asked of Live.
    let (_, host) = setup();
    let mut twice = batch(Value::Null, "simulator:track:track-2");
    twice["entries"][0]["trackRef"] = json!("track:track-1");
    assert!(host.live_data_preview_async(&json!(1), &twice).await.to_string().contains("only once"));
    let mut unread = batch(Value::Null, "simulator:track:track-2");
    unread["entries"][0].as_object_mut().unwrap().remove("expectedIdentity");
    assert!(host.live_data_preview_async(&json!(1), &unread).await.to_string().contains("-32602"));
    // A batch is for ids: its text is short.
    let mut long = batch(Value::Null, "simulator:track:track-2");
    long["entries"][0]["value"] = json!("x".repeat(257));
    assert!(host.live_data_preview_async(&json!(1), &long).await.to_string().contains("at most 256 characters in a batch"));
}
#[tokio::test]
async fn a_track_moved_in_after_the_check_gets_nothing_and_a_retry_learns_what_was_saved() {
    let setup = || {
        let adapter = Rc::new(Adapter::new());
        {
            let mut state = adapter.sim.state.borrow_mut();
            let mut bass = state["tracks"][0].clone();
            bass["ref"] = json!("track:track-2");
            bass["objectIdentity"] = json!("simulator:track:track-2");
            state["tracks"].as_array_mut().unwrap().push(bass);
        }
        let host = McpHost::new(adapter.clone(), McpHostOptions::default()).unwrap();
        (adapter, host)
    };
    let batch = json!({"key":"kumi.track","value":"01J00000000000000000000001","trackRef":"track:track-1","expectedValue":null,
        "expectedIdentity":"simulator:track:track-1","entries":[{"trackRef":"track:track-2","value":"01J00000000000000000000002",
        "expectedValue":null,"expectedIdentity":"simulator:track:track-2"}]});
    let saved = |adapter: &Adapter, track: &str| {
        adapter.sim.invoke(&LiveInvocation::new("data.get", json!({"ref":track,"key":"kumi.track"}))).unwrap()["value"].clone()
    };
    let state = |host: &McpHost, preview: &Value| {
        let body: Value = serde_json::from_str(preview["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
        host.transaction_record(body["transactionId"].as_str().unwrap()).unwrap().borrow()["state"].clone()
    };
    // Another track lands in the second place between the host's check and the write: Live refuses the whole batch.
    let (adapter, host) = setup();
    let preview = host.live_data_preview_async(&json!(1), &batch).await;
    adapter.shift_before_write.set(true);
    let applied = apply_batch(&host, &preview, "batch-key").await;
    assert!(applied.to_string().contains("isn't the one that was read any more, so nothing was saved"), "{applied}");
    assert_eq!((saved(&adapter, "track:track-1"), saved(&adapter, "track:track-2")), (Value::Null, Value::Null));
    assert_eq!(state(&host, &preview), "previewed");
    // An apply that saved everything but never said so: its retry (the same key) finds the text there.
    let (adapter, host) = setup();
    let preview = host.live_data_preview_async(&json!(1), &batch).await;
    adapter.reset("apply-after");
    let first = apply_batch(&host, &preview, "batch-key").await;
    assert_eq!(state(&host, &preview), "uncertain", "{first}");
    // Live's replay of that key is gone (as after a lost answer it never recorded), so it checks the text afresh.
    adapter.cache.borrow_mut().clear();
    let retried = apply_batch(&host, &preview, "batch-key").await;
    assert!(
        retried.to_string().contains(r#"\"state\":\"applied\""#) && retried.to_string().contains(r#"\"idempotent\":true"#),
        "{retried}"
    );
    assert_eq!(state(&host, &preview), "applied");
    // The same, but the text was changed again before the retry: whether Kumi's write held isn't known, so it stays
    // uncertain rather than saying nothing was saved.
    let (adapter, host) = setup();
    let preview = host.live_data_preview_async(&json!(1), &batch).await;
    adapter.reset("apply-after");
    apply_batch(&host, &preview, "batch-key").await;
    adapter.reset("");
    adapter.cache.borrow_mut().clear();
    adapter.sim.invoke(&LiveInvocation::new("data.set", json!({"ref":"track:track-2","key":"kumi.track","value":"manual"}))).unwrap();
    let retried = apply_batch(&host, &preview, "batch-key").await;
    assert!(!retried.to_string().contains("nothing was saved"), "{retried}");
    assert_eq!(state(&host, &preview), "uncertain");
}
