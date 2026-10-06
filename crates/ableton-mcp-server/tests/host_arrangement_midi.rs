use ableton_mcp_server::{
    host::{helpers::canonical_mutation_identity, McpHost, McpHostOptions, ToolCall},
    live::*,
};
use serde_json::{json, Value};
use std::{
    cell::{Cell, RefCell},
    collections::HashMap,
    rc::Rc,
    time::Duration,
};
fn array(v: &Value) -> Vec<Value> {
    v.as_array().cloned().unwrap_or_default()
}
fn fixture() -> Value {
    serde_json::from_str(include_str!("fixtures/host-arrangement-midi-oracle.json")).unwrap()
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
                    if v.as_str().is_some_and(|v| v.starts_with("arrmidi_") || v.starts_with("clearrange_")) {
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
            Value::String(s) if s.starts_with("arrmidi_") || s.starts_with("clearrange_") => *v = json!("$transaction"),
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
    /// "apply-late" and "apply-unfilled": when the clip was made, the clips there before it, and how long
    /// Live shows the new clip unnamed, then without its notes.
    late: RefCell<Option<(tokio::time::Instant, Vec<Value>, Duration, Duration)>>,
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
            late: Default::default(),
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
        if !self.fired.get()
            && i.operation == "transaction.group"
            && ["apply-partial", "apply-partial-timeout", "apply-unreported-partial"].contains(&fault.as_str())
        {
            self.fired.set(true);
            self.after_invoke.set(true);
            self.sim.invoke(&serde_json::from_value(i.args["ops"][0].clone()).unwrap())?;
            if fault == "apply-unreported-partial" {
                return Ok(json!({"ok":true}));
            }
            return Err(LiveError::error(if fault == "apply-partial" { "step 2 failed (refused)" } else { "injected operation failure" }));
        }
        if ["apply-late", "apply-unfilled"].contains(&fault.as_str())
            && i.operation == "arrangement.midi-clip.create"
            && self.late.borrow().is_none()
        {
            let before =
                array(&self.sim.state.borrow()["arrangementClips"]).iter().map(|row| row["clip"]["objectIdentity"].clone()).collect();
            let result = self.sim.invoke(i)?;
            let (unnamed, empty) = if fault == "apply-late" { (60, 120) } else { (u64::MAX, u64::MAX) };
            *self.late.borrow_mut() =
                Some((tokio::time::Instant::now(), before, Duration::from_millis(unnamed), Duration::from_millis(empty)));
            self.after_invoke.set(true);
            return Ok(result);
        }
        let no_effect = !self.fired.get() && fault.ends_with("no-effect");
        let result = if no_effect {
            self.fired.set(true);
            json!({"ok":true})
        } else {
            self.sim.invoke(i)?
        };
        if c.is_some() && !no_effect && i.operation == "arrangement.clip.delete" {
            self.cache.borrow_mut().insert(key, result.clone());
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
        if !self.fired.get() && ((fault.ends_with("-read") && self.after_invoke.get()) || fault.ends_with("-current-read")) {
            self.fired.set(true);
            return Err(LiveError::error("injected authoritative read failure"));
        }
        Ok(())
    }
}
impl Adapter {
    /// As real Live shows a clip Kumi's extension made: unnamed at first, then without its notes, then whole.
    fn as_live_shows(&self, r: &LiveDiscoveryRequest, result: &mut LiveDiscoveryResult) {
        let Some((at, before, unnamed, empty)) = self.late.borrow().clone() else { return };
        let state = self.sim.state.borrow();
        let made = |identity: &Value| !before.contains(identity);
        match r.kind {
            LiveDiscoveryKind::ArrangementClip if at.elapsed() < unnamed => {
                for item in result.items.iter_mut().filter(|item| made(&item["objectIdentity"])) {
                    item.insert("name".into(), json!(""));
                }
            }
            LiveDiscoveryKind::Note if at.elapsed() < empty => {
                let parent = array(&state["arrangementClips"]).into_iter().find(|row| r.parent.as_deref() == row["clip"]["ref"].as_str());
                if parent.is_some_and(|row| made(&row["clip"]["objectIdentity"])) {
                    result.items.clear();
                }
            }
            _ => {}
        }
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
        let mut result = self.sim.discover_async(r, c).await?;
        self.as_live_shows(r, &mut result);
        Ok(result)
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
async fn arrangement_midi_and_clear_validation_match_source() {
    for (index, row) in fixture()["rows"].as_array().unwrap().iter().enumerate() {
        let host = McpHost::new(Rc::new(DeterministicLiveSimulator::new()), McpHostOptions::default()).unwrap();
        let name = format!(
            "live_{}_{}",
            if row["kind"] == "single" { "arrangement_midi_clip" } else { "clip_clear_range" },
            row["action"].as_str().unwrap()
        );
        let result = host
            .dispatch_arrangement_midi_tool(
                &ToolCall { id: json!(1), name, arguments: Some(row["args"].clone()), asynchronous: true },
                None,
            )
            .await
            .unwrap()
            .unwrap()
            .unwrap_or(Value::Null);
        same(&clean(result), &row["result"], &format!("{index} {row}"));
    }
}
async fn perform(
    host: &McpHost,
    clear: bool,
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
        if clear {
            host.live_clip_clear_range_apply_async(&id, &args, Some(&signal)).await
        } else {
            host.live_arrangement_midi_clip_apply_async(&id, &args, Some(&signal)).await
        }
        .unwrap_or(Value::Null)
    } else {
        host.with_undo_watch(&id, &args, async {
            if clear {
                Ok(ableton_mcp_server::host::helpers::reason_error(
                    &id,
                    "Kumi can't bring this back; Live's undo can.",
                    "If the producer wants it back, Live's own undo can bring it (Cmd-Z in Live).",
                ))
            } else {
                Ok(host.undo_arrangement_midi_async(&id, &args, None).await)
            }
        })
        .await
        .unwrap()
    };
    results.push(clean(result));
    states.push(clean(record.borrow().clone()));
}
// Paused time: an apply that waits for Live to show its clips waits the same on every run.
#[tokio::test(start_paused = true)]
async fn arrangement_midi_partial_recovery_and_clear_flows_match_source() {
    let data = fixture();
    for row in data["workflows"].as_array().unwrap() {
        let kind = row["kind"].as_str().unwrap();
        let scenario = row["scenario"].as_str().unwrap();
        let clear = ["clear", "cut"].contains(&kind);
        let adapter = Rc::new(Adapter::new());
        *adapter.sim.state.borrow_mut() = data["seeds"][kind].clone();
        if scenario == "missing-identity" {
            adapter.sim.state.borrow_mut()["tracks"][0]["objectIdentity"] = json!("");
        }
        if scenario == "group-track" {
            adapter.sim.state.borrow_mut()["tracks"][0]["kind"] = json!("group");
        }
        if scenario == "audio-track" {
            adapter.sim.state.borrow_mut()["tracks"][0]["mediaKind"] = json!("audio");
        }
        let host = McpHost::new(adapter.clone(), McpHostOptions::default()).unwrap();
        let args = &data["variants"][kind];
        let preview = if clear {
            host.live_clip_clear_range_preview_async(&json!(1), args).await
        } else {
            host.live_arrangement_midi_clip_preview_async(&json!(1), args).await
        };
        let body: Value = serde_json::from_str(preview["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
        let mut results = vec![clean(preview)];
        let mut states = vec![];
        if let Some(txid) = body.get("transactionId") {
            let raw = txid.as_str().unwrap();
            let prefix = if clear { "clearrange_" } else { "arrmidi_" };
            assert!(raw.starts_with(prefix));
            let suffix = &raw[prefix.len()..];
            assert_eq!(suffix.len(), 24);
            assert!(suffix.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-'));
            let record = host.transaction_record(txid.as_str().unwrap()).unwrap();
            if scenario == "expire" {
                record.borrow_mut()["expiresAt"] = json!(0);
            }
            if scenario == "epoch" {
                adapter.sim.reconnect().unwrap();
            }
            if scenario == "identity-edit" {
                adapter.sim.state.borrow_mut()["tracks"][0]["objectIdentity"] = json!("other");
            }
            if scenario == "clips-edit" {
                let mut args = data["variants"]["single"].clone();
                args["start"] = json!(40);
                adapter.sim.invoke(&LiveInvocation::new("arrangement.midi-clip.create", args)).unwrap();
            }
            if scenario.starts_with("apply-") {
                adapter.reset(scenario);
            }
            for key in ["apply-key", "other-key", "apply-key"] {
                perform(&host, clear, "apply", key, txid, &mut results, &mut states, &record, scenario == "apply-preabort").await;
            }
            adapter.reset("");
            if let Some(rows) = adapter.sim.state.borrow_mut().get_mut("arrangementClips").and_then(Value::as_array_mut) {
                if let Some(row) =
                    rows.iter_mut().find(|r| r["clip"]["objectIdentity"] == record.borrow()["created"]["clips"][0]["objectIdentity"])
                {
                    let c = &mut row["clip"];
                    if scenario == "undo-other-identity" {
                        c["objectIdentity"] = json!("other");
                    }
                    if scenario == "undo-name" {
                        c["name"] = json!("Manual");
                    }
                    if scenario == "undo-start" {
                        c["start"] = json!(c["start"].as_f64().unwrap() + 1.);
                    }
                    if scenario == "undo-length" {
                        c["length"] = json!(c["length"].as_f64().unwrap() + 1.);
                    }
                    if scenario == "undo-notes" {
                        if c["notes"].as_array().unwrap().is_empty() {
                            c["notes"]
                                .as_array_mut()
                                .unwrap()
                                .push(json!({"pitch":72,"start":0,"duration":1,"velocity":100,"mute":false,"channel":1,"id":700}));
                        } else {
                            c["notes"][0]["pitch"] = json!(72);
                        }
                    }
                }
            }
            if scenario == "undo-epoch" {
                adapter.sim.reconnect().unwrap();
            }
            if scenario.starts_with("undo-")
                && !["undo-other-identity", "undo-name", "undo-start", "undo-length", "undo-notes", "undo-epoch"].contains(&scenario)
            {
                adapter.reset(scenario);
            }
            for key in ["undo-key", "other-undo-key", "undo-key"] {
                perform(&host, clear, "undo", key, txid, &mut results, &mut states, &record, false).await;
            }
            adapter.reset("");
            perform(&host, clear, "undo", "new-undo-key", txid, &mut results, &mut states, &record, false).await;
        }
        let label = format!("{kind} {scenario}");
        same(&json!(results), &row["results"], &format!("{label} results"));
        same(&json!(states), &row["states"], &format!("{label} states"));
        same(&json!(*adapter.calls.borrow()), &row["calls"], &format!("{label} calls"));
        same(&adapter.sim.state.borrow(), &row["state"], &format!("{label} state"));
    }
}

/// Live shows a clip Kumi's extension made a moment late (#166): the apply looks again until Live shows it
/// whole, and the clip's fence has its notes, so undoing it isn't refused as changed since.
#[tokio::test(start_paused = true)]
async fn an_apply_waits_for_live_to_show_its_clip_whole() {
    let data = fixture();
    let adapter = Rc::new(Adapter::new());
    *adapter.sim.state.borrow_mut() = data["seeds"]["single"].clone();
    let host = McpHost::new(adapter.clone(), McpHostOptions::default()).unwrap();
    let mut args = data["variants"]["single"].clone();
    args["name"] = json!("Reese");
    let preview = host.live_arrangement_midi_clip_preview_async(&json!(1), &args).await;
    let txid = serde_json::from_str::<Value>(preview["result"]["content"][0]["text"].as_str().unwrap()).unwrap()["transactionId"].clone();
    adapter.reset("apply-late");
    let began = tokio::time::Instant::now();
    let applied = host
        .live_arrangement_midi_clip_apply_async(
            &json!(2),
            &json!({"transactionId":txid,"confirmation":"apply","idempotencyKey":"late-apply"}),
            None,
        )
        .await
        .unwrap();
    let applied = clean(applied);
    assert_eq!(applied["result"]["content"][0]["text"]["state"], "applied", "{applied}");
    assert!(began.elapsed() >= Duration::from_millis(120), "it waited until Live showed the clip whole");
    let undo = json!({"transactionId":txid,"confirmation":"undo","idempotencyKey":"undo-late"});
    let undone = clean(
        host.with_undo_watch(&json!(3), &undo, async { Ok(host.undo_arrangement_midi_async(&json!(3), &undo, None).await) }).await.unwrap(),
    );
    assert_eq!(undone["result"]["content"][0]["text"]["state"], "undone", "{undone}");
}
/// When Live still doesn't show the clip whole after half a second, the apply says what Live shows there.
#[tokio::test(start_paused = true)]
async fn an_apply_that_cant_find_its_clip_says_what_live_shows() {
    let data = fixture();
    let adapter = Rc::new(Adapter::new());
    *adapter.sim.state.borrow_mut() = data["seeds"]["single"].clone();
    let host = McpHost::new(adapter.clone(), McpHostOptions::default()).unwrap();
    let mut args = data["variants"]["single"].clone();
    args["name"] = json!("Reese");
    let preview = host.live_arrangement_midi_clip_preview_async(&json!(1), &args).await;
    let txid = serde_json::from_str::<Value>(preview["result"]["content"][0]["text"].as_str().unwrap()).unwrap()["transactionId"].clone();
    adapter.reset("apply-unfilled");
    let began = tokio::time::Instant::now();
    let applied = host
        .live_arrangement_midi_clip_apply_async(
            &json!(2),
            &json!({"transactionId":txid,"confirmation":"apply","idempotencyKey":"unfilled"}),
            None,
        )
        .await
        .unwrap();
    let applied = clean(applied);
    assert_eq!(began.elapsed(), Duration::from_millis(500), "it waited half a second");
    assert_eq!(
        applied["result"]["content"][0]["text"]["reason"],
        "Live made the clips, but the Arrangement doesn't show them where they were asked; Live shows an unnamed clip at beat 8, 4 beats long, with 0 of 1 notes"
    );
}
/// A cancelled apply stops waiting for Live at once and goes on with what it last found.
#[tokio::test(start_paused = true)]
async fn a_cancelled_apply_stops_waiting_for_live() {
    let data = fixture();
    let adapter = Rc::new(Adapter::new());
    *adapter.sim.state.borrow_mut() = data["seeds"]["single"].clone();
    let host = McpHost::new(adapter.clone(), McpHostOptions::default()).unwrap();
    let preview = host.live_arrangement_midi_clip_preview_async(&json!(1), &data["variants"]["single"]).await;
    let txid = serde_json::from_str::<Value>(preview["result"]["content"][0]["text"].as_str().unwrap()).unwrap()["transactionId"].clone();
    adapter.reset("apply-unfilled");
    let signal = kumi_common::abort::Signal::new();
    let began = tokio::time::Instant::now();
    let args = json!({"transactionId":txid,"confirmation":"apply","idempotencyKey":"cancelled"});
    let cancel = async {
        tokio::time::sleep(Duration::from_millis(100)).await;
        signal.cancel();
    };
    let id = json!(2);
    let (applied, ()) = tokio::join!(host.live_arrangement_midi_clip_apply_async(&id, &args, Some(&signal)), cancel);
    assert_eq!(began.elapsed(), Duration::from_millis(100), "it stopped waiting when cancelled");
    let applied = clean(applied.unwrap());
    let text = &applied["result"]["content"][0]["text"];
    assert_eq!(text["state"], "applied", "{applied}");
    assert_eq!(text["partial"]["reason"], "Live shows 0 of the 1 notes asked for in the clip at beat 8", "{applied}");
}
