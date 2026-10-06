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
    serde_json::from_str(include_str!("fixtures/host-note-edit-oracle.json")).unwrap()
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
                    if v.as_str().is_some_and(|v| v.starts_with("noteupdate_") || v.starts_with("notedelete_")) {
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
            Value::String(s) if s.starts_with("noteupdate_") || s.starts_with("notedelete_") => *v = json!("$transaction"),
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
    kind: String,
    fired: Cell<bool>,
    after_invoke: Cell<bool>,
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
        let mut actual = i.clone();
        if !self.fired.get() && fault == "recover-partial" {
            let field = if self.kind == "delete" { "noteIds" } else { "notes" };
            actual.args[field] = json!([actual.args[field][0].clone()]);
        }
        let result = if no_effect {
            self.fired.set(true);
            json!({"ok":true})
        } else {
            self.sim.invoke(&actual)?
        };
        if c.is_some() && !no_effect {
            self.cache.borrow_mut().insert(key, result.clone());
        }
        self.after_invoke.set(true);
        if !self.fired.get() && fault == "recover-external" {
            self.fired.set(true);
            self.sim.state.borrow_mut()["tracks"][0]["clips"][0]["notes"]
                .as_array_mut()
                .unwrap()
                .push(json!({"id":100,"pitch":80,"start":2,"duration":1,"velocity":90,"channel":1}));
            return Err(LiveError::error("injected operation failure"));
        }
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
async fn note_edit_validation_matches_source() {
    for (index, row) in fixture()["rows"].as_array().unwrap().iter().enumerate() {
        let host = McpHost::new(Rc::new(DeterministicLiveSimulator::new()), McpHostOptions::default()).unwrap();
        let name = format!("live_note_{}_{}", row["kind"].as_str().unwrap(), row["action"].as_str().unwrap());
        let got = host
            .dispatch_note_edit_tool(&ToolCall { id: json!(1), name, arguments: Some(row["args"].clone()), asynchronous: true }, None)
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
    let args = json!({"transactionId":txid,"confirmation":action,"idempotencyKey":key});
    let signal = kumi_common::abort::Signal::new();
    if preabort {
        signal.cancel();
    }
    let result = if action == "apply" {
        host.live_note_edit_apply_async(&id, &args, kind, Some(&signal)).await.unwrap_or(Value::Null)
    } else {
        host.with_undo_watch(&id, &args, async { Ok(host.undo_note_edit_async(&id, &args, None).await) }).await.unwrap()
    };
    results.push(clean(result));
    states.push(clean(record.borrow().clone()));
}
#[tokio::test]
async fn note_edit_apply_and_exact_key_undo_match_source() {
    for row in fixture()["workflows"].as_array().unwrap() {
        let kind = row["kind"].as_str().unwrap();
        let scenario = row["scenario"].as_str().unwrap();
        let adapter = Rc::new(Adapter::new(kind));
        {
            let mut s = adapter.sim.state.borrow_mut();
            let clip = &mut s["tracks"][0]["clips"][0];
            if scenario == "missing-identity" {
                clip.as_object_mut().unwrap().remove("objectIdentity");
            }
            if scenario == "missing-optionals" {
                for field in ["mute", "probability", "velocityDeviation", "releaseVelocity"] {
                    clip["notes"][0].as_object_mut().unwrap().remove(field);
                }
            }
            if ["multiple", "recover-partial", "recover-external"].contains(&scenario) {
                let mut note = clip["notes"][0].clone();
                note["id"] = json!(2);
                note["start"] = json!(1);
                note["pitch"] = json!(40);
                clip["notes"].as_array_mut().unwrap().push(note);
            }
        }
        let host = McpHost::new(adapter.clone(), McpHostOptions::default()).unwrap();
        let ids: Vec<_> = adapter.sim.state.borrow()["tracks"][0]["clips"][0]["notes"]
            .as_array()
            .unwrap()
            .iter()
            .map(|note| note["id"].clone())
            .collect();
        let mut args = json!({"clipRef":"clip:clip-1"});
        if kind == "update" {
            args["notes"] = json!(ids.iter().map(|id| json!({"id":id,"pitch":48,"velocity":96,"mute":true})).collect::<Vec<_>>());
        } else {
            args["noteIds"] = json!(ids);
        }
        let preview = host.live_note_edit_preview_async(&json!(1), &args, kind).await;
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
                let clip = &mut s["tracks"][0]["clips"][0];
                match scenario {
                    "identity-edit" => clip["objectIdentity"] = json!("other"),
                    "note-edit" => clip["notes"][0]["velocity"] = json!(60),
                    "revision-edit" => clip["notesRevision"] = json!("manual"),
                    _ => {}
                }
            }
            if scenario.starts_with("apply-") || scenario.starts_with("recover-") {
                adapter.reset(scenario);
            }
            let keys = if scenario.starts_with("recover-") { vec!["apply-key"] } else { vec!["apply-key", "other-key", "apply-key"] };
            for key in keys {
                perform(&host, kind, "apply", key, txid, &mut results, &mut states, &record, scenario == "apply-preabort").await;
            }
            adapter.reset("");
            {
                let mut s = adapter.sim.state.borrow_mut();
                let clip = &mut s["tracks"][0]["clips"][0];
                match scenario {
                    "undo-other-identity" => clip["objectIdentity"] = json!("other"),
                    "undo-other-note" => clip["notes"]
                        .as_array_mut()
                        .unwrap()
                        .push(json!({"id":100,"pitch":80,"start":2,"duration":1,"velocity":90,"channel":1})),
                    _ => {}
                }
            }
            if scenario == "undo-epoch" {
                adapter.sim.reconnect().unwrap();
            }
            if scenario.starts_with("undo-") && !["undo-other-identity", "undo-other-note", "undo-epoch"].contains(&scenario) {
                adapter.reset(scenario);
            }
            for key in ["undo-key", "other-undo-key", "undo-key"] {
                perform(&host, kind, "undo", key, txid, &mut results, &mut states, &record, false).await;
            }
            adapter.reset("");
            perform(&host, kind, "undo", "new-undo-key", txid, &mut results, &mut states, &record, false).await;
        }
        let label = format!("{kind} {scenario}");
        same(&json!(results), &row["results"], &format!("{label} results"));
        same(&json!(states), &row["states"], &format!("{label} states"));
        same(&json!(*adapter.calls.borrow()), &row["calls"], &format!("{label} calls"));
        same(&adapter.sim.state.borrow(), &row["state"], &format!("{label} state"));
    }
}
#[tokio::test]
async fn integer_note_id_written_as_decimal_keeps_js_number_semantics() {
    let host = McpHost::new(Rc::new(DeterministicLiveSimulator::new()), McpHostOptions::default()).unwrap();
    let args: Value = serde_json::from_str("{\"clipRef\":\"clip:clip-1\",\"notes\":[{\"id\":1.0,\"pitch\":48.0}]}").unwrap();
    let preview = host.live_note_edit_preview_async(&json!(1), &args, "update").await;
    let body: Value = serde_json::from_str(preview["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    let result = host
        .live_note_edit_apply_async(
            &json!(2),
            &json!({"transactionId":body["transactionId"],"confirmation":"apply","idempotencyKey":"apply-key"}),
            "update",
            None,
        )
        .await
        .unwrap();
    assert_eq!(result["result"]["isError"], false, "{result}");
    let body: Value = serde_json::from_str(result["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(body["updated"], 1);
}

#[tokio::test]
async fn an_arrangement_clips_notes_are_edited_fenced_by_its_track() {
    // An Arrangement clip has no slot or scene: its notes are read on their own, and it's fenced by its track.
    let sim = Rc::new(DeterministicLiveSimulator::new());
    {
        let mut s = sim.state.borrow_mut();
        let mut clip = s["tracks"][0]["clips"][0].clone();
        clip["ref"] = json!("arrangement-clip:track-1:4");
        clip["objectIdentity"] = json!("simulator:arrangement-clip:0");
        clip["start"] = json!(4);
        s["arrangementClips"].as_array_mut().unwrap().push(json!({"trackRef":"track:track-1","clip":clip}));
    }
    let host = McpHost::new(sim.clone(), McpHostOptions::default()).unwrap();
    let text = |reply: &Value| -> Value { serde_json::from_str(reply["result"]["content"][0]["text"].as_str().unwrap()).unwrap() };
    let args = json!({"clipRef":"arrangement-clip:track-1:4","notes":[{"id":1,"pitch":48}]});
    let preview = host.live_note_edit_preview_async(&json!(1), &args, "update").await;
    let body = text(&preview);
    let record = host.transaction_record(body["transactionId"].as_str().unwrap()).unwrap().borrow().clone();
    assert_eq!(
        record["authority"],
        json!({"expectedObjectIdentity":"simulator:arrangement-clip:0","expectedTrackRef":"track:track-1","expectedTrackIdentity":"simulator:track:track-1"}),
        "{preview}"
    );
    let applied = host
        .live_note_edit_apply_async(
            &json!(2),
            &json!({"transactionId":body["transactionId"],"confirmation":"apply","idempotencyKey":"arrangement-apply"}),
            "update",
            None,
        )
        .await
        .unwrap();
    assert_eq!(applied["result"]["isError"], false, "{applied}");
    let pitch = || sim.state.borrow()["arrangementClips"][0]["clip"]["notes"][0]["pitch"].clone();
    assert_eq!(pitch(), json!(48), "the Arrangement clip's note changed");
    assert_eq!(sim.state.borrow()["tracks"][0]["clips"][0]["notes"][0]["pitch"], json!(36), "the Session clip's didn't");
    // The track changed since the preview: refused, nothing written.
    let preview = host
        .live_note_edit_preview_async(&json!(3), &json!({"clipRef":"arrangement-clip:track-1:4","notes":[{"id":1,"pitch":50}]}), "update")
        .await;
    sim.state.borrow_mut()["tracks"][0]["objectIdentity"] = json!("simulator:track:other");
    let refused = host
        .live_note_edit_apply_async(
            &json!(4),
            &json!({"transactionId":text(&preview)["transactionId"],"confirmation":"apply","idempotencyKey":"arrangement-stale"}),
            "update",
            None,
        )
        .await
        .unwrap();
    assert_eq!(refused["result"]["isError"], true, "{refused}");
    assert_eq!(pitch(), json!(48));
}
#[tokio::test]
async fn a_split_arrangement_clips_notes_are_edited_up_to_its_own_end() {
    // The right half of an 8-beat clip split at beat 4: start marker 4, end marker 8, so Live's length is 4, but its
    // notes are in its own time, at 4 to 8.
    let sim = Rc::new(DeterministicLiveSimulator::new());
    {
        let mut s = sim.state.borrow_mut();
        let mut clip = s["tracks"][0]["clips"][0].clone();
        clip["ref"] = json!("arrangement-clip:track-1:4");
        clip["objectIdentity"] = json!("simulator:arrangement-clip:0");
        clip["name"] = json!("Verse B");
        clip["start"] = json!(20);
        clip["length"] = json!(4);
        clip["looping"] = json!(false);
        clip["startMarker"] = json!(4);
        clip["endMarker"] = json!(8);
        clip["notes"][0]["start"] = json!(5);
        s["arrangementClips"].as_array_mut().unwrap().push(json!({"trackRef":"track:track-1","clip":clip}));
    }
    let host = McpHost::new(sim.clone(), McpHostOptions::default()).unwrap();
    let preview = |patch: Value| {
        let host = &host;
        async move {
            host.live_note_edit_preview_async(&json!(1), &json!({"clipRef":"arrangement-clip:track-1:4","notes":[patch]}), "update").await
        }
    };
    // A velocity-only patch, and a move to the clip's own end, both preview.
    for patch in [json!({"id":1,"velocity":90}), json!({"id":1,"start":7.75})] {
        let reply = preview(patch).await;
        assert!(reply["result"]["content"][0]["text"].as_str().is_some_and(|t| t.contains("transactionId")), "{reply}");
    }
    // Past it: refused, naming the clip's span.
    let reply = preview(json!({"id":1,"start":7.9})).await;
    assert_eq!(
        reply["error"]["message"],
        json!("note patch runs past the clip: \"Verse B\" holds notes from beat 0 to 8 in its own time"),
        "{reply}"
    );
    // An audio clip takes no notes, and the refusal says so.
    sim.state.borrow_mut()["arrangementClips"][0]["clip"]["kind"] = json!("audio");
    let reply = preview(json!({"id":1,"velocity":90})).await;
    let text = reply["result"]["content"][0]["text"].as_str().unwrap_or_default();
    assert!(text.contains(r#"\"Verse B\" is an audio clip; notes are only in MIDI clips"#), "{reply}");
}
