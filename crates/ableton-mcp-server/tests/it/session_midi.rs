use ableton_mcp_server::{live::*, transactions::session_midi::*};
use serde_json::{json, Value};
use std::{
    cell::{Cell, RefCell},
    collections::HashMap,
    rc::Rc,
};
fn request() -> Value {
    json!({"trackRef":"track:track-1","sceneIndex":1,"name":"Bounded Beat","length":4,"notes":[{"pitch":36,"start":0,"duration":0.25,"velocity":100,"channel":1},{"pitch":38,"start":1,"duration":0.25,"velocity":90,"channel":1}]})
}
fn simulator() -> Rc<DeterministicLiveSimulator> {
    let sim = Rc::new(DeterministicLiveSimulator::new());
    {
        let mut state = sim.state.borrow_mut();
        for index in 1..=3 {
            state["scenes"].as_array_mut().unwrap().push(json!({"ref":format!("scene:scene-{}",index+1),"objectIdentity":format!("simulator:scene:scene-{}",index+1),"name":format!("Scene {}",index+1),"index":index}));
            state["tracks"][0]["clipSlots"].as_array_mut().unwrap().push(json!({"ref":format!("clip-slot:track-1:{index}"),"parentRef":"track:track-1","objectIdentity":format!("simulator:clip-slot:track-1:{index}"),"sceneIndex":index,"clipRef":null,"empty":true}));
        }
    }
    sim
}
#[test]
fn synchronous_creation_replay_discovery_and_exact_undo() {
    let sim = simulator();
    let manager = SessionMidiTransactionManager::new(sim.clone(), None);
    let mut bad = request();
    bad["notes"][0]["pitch"] = 128.into();
    assert!(manager.preview(&mut bad).unwrap_err().message().contains("invalid MIDI note"));
    let preview = manager.preview(&mut request()).unwrap();
    let id = preview["transactionId"].as_str().unwrap();
    assert_eq!(preview["prior"]["occupied"], false);
    assert_eq!(preview["confirmation"], "apply");
    assert!(manager.apply(id, &json!("wrong"), "apply-1").unwrap_err().message().contains("confirmation=apply"));
    let applied = manager.apply(id, &json!("apply"), "apply-1").unwrap();
    assert_eq!(applied["state"], "applied");
    assert_eq!(applied["notes"].as_array().unwrap().len(), 2);
    assert_eq!(manager.apply(id, &json!("apply"), "apply-1").unwrap()["idempotent"], true);
    assert!(manager.apply("midi_other", &json!("apply"), "apply-1").unwrap_err().message().contains("idempotency key conflicts"));
    for kind in ["track", "scene", "clip", "note"] {
        assert_eq!(discover_session(&*sim, kind, 100.0, None).unwrap()["epoch"], 1);
    }
    let page = discover_session(&*sim, "note", 1.0, None).unwrap();
    assert_eq!(page["truncated"], true);
    assert_eq!(discover_session(&*sim, "note", 10.0, page["nextCursor"].as_str()).unwrap()["items"].as_array().unwrap().len(), 2);
    assert!(discover_session(&*sim, "track", 0.0, None).is_err());
    assert_eq!(manager.undo(id, &json!("undo"), "undo-1").unwrap()["state"], "undone");
    assert_eq!(manager.undo(id, &json!("undo"), "undo-1").unwrap()["idempotent"], true);
    assert!(manager.is_finalizable(id));
    assert_eq!(manager.finalize(id).unwrap()["priorState"], "undone");
    assert!(!manager.is_finalizable(id));
}
#[test]
fn synchronous_undo_refusal_keeps_applied_authority_for_a_later_check() {
    let sim = simulator();
    let manager = SessionMidiTransactionManager::new(sim.clone(), None);
    let preview = manager.preview(&mut request()).unwrap();
    let id = preview["transactionId"].as_str().unwrap();
    let applied = manager.apply(id, &json!("apply"), "apply").unwrap();
    let reference = applied["clipRef"].as_str().unwrap();
    let edit = |name: &str| {
        sim.state.borrow_mut()["tracks"][0]["clips"].as_array_mut().unwrap().iter_mut().find(|clip| clip["ref"] == reference).unwrap()
            ["name"] = name.into();
    };
    edit("Edited by hand");
    assert!(manager.undo(id, &json!("undo"), "undo-1").unwrap_err().message().contains("changed"));
    edit("Bounded Beat");
    assert_eq!(manager.undo(id, &json!("undo"), "undo-2").unwrap()["state"], "undone");
}
#[derive(Default)]
struct Faults {
    lose_delete: Cell<bool>,
    fail_read: Cell<bool>,
    lose_notes: Cell<bool>,
    partial_notes: Cell<bool>,
    strip_expression: Cell<bool>,
    cache_create: Cell<bool>,
    cache: RefCell<HashMap<String, Value>>,
    operations: RefCell<Vec<String>>,
}
struct Adapter {
    sim: Rc<DeterministicLiveSimulator>,
    faults: Rc<Faults>,
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
        self.sim.invoke(i)
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
    async fn snapshot_async(&self, _: Option<&LiveOperationContext>, _: Option<&LiveSnapshotRequest>) -> Result<LiveSnapshot, LiveError> {
        self.sim.snapshot()
    }
    async fn discover_async(&self, _: &LiveDiscoveryRequest, _: Option<&LiveOperationContext>) -> Result<LiveDiscoveryResult, LiveError> {
        Err(LiveError::error("not used by this test adapter"))
    }
    async fn get_async(&self, r: &LiveRef, _: Option<&LiveOperationContext>) -> Result<Option<Value>, LiveError> {
        if self.faults.fail_read.replace(false) {
            return Err(LiveError::error("remote adapter request timed out"));
        }
        self.sim.get(r)
    }
    async fn invoke_async(&self, i: &LiveInvocation, _: Option<&LiveOperationContext>) -> Result<Value, LiveError> {
        self.faults.operations.borrow_mut().push(i.operation.clone());
        if let Some(value) = self.faults.cache.borrow().get(&i.operation) {
            return Ok(value.clone());
        }
        let mut invocation = i.clone();
        if i.operation == "note.add-batch" && self.faults.strip_expression.get() {
            for note in invocation.args["notes"].as_array_mut().unwrap() {
                for key in ["probability", "velocityDeviation", "releaseVelocity", "mute"] {
                    note.as_object_mut().unwrap().remove(key);
                }
            }
        }
        let mut result = self.sim.invoke(&invocation)?;
        if i.operation == "clip.create" && self.faults.cache_create.get() {
            self.faults.cache.borrow_mut().insert(i.operation.clone(), result.clone());
        }
        if (i.operation == "clip.delete" && self.faults.lose_delete.replace(false))
            || (i.operation == "note.add-batch" && self.faults.lose_notes.replace(false))
        {
            self.faults.cache.borrow_mut().insert(i.operation.clone(), result);
            return Err(LiveError::error("remote adapter request state uncertain after dispatch timeout"));
        }
        if i.operation == "note.add-batch" && self.faults.partial_notes.get() {
            result["added"] = 0.into();
        }
        Ok(result)
    }
    async fn reconnect_async(&self, _: Option<&LiveOperationContext>) -> Result<LiveStatus, LiveError> {
        self.sim.reconnect()
    }
    async fn close(&self) -> Result<(), LiveError> {
        Ok(())
    }
}
fn adapter() -> (Rc<Adapter>, Rc<Faults>) {
    let faults = Rc::new(Faults::default());
    (Rc::new(Adapter { sim: simulator(), faults: faults.clone() }), faults)
}
fn context(id: &str, key: &str) -> LiveOperationContext {
    LiveOperationContext {
        deadline_ms: Some(kumi_common::time::now_ms() as f64 + 5000.0),
        idempotency_key: Some(key.into()),
        transaction_id: Some(id.into()),
        ..Default::default()
    }
}
#[tokio::test]
async fn asynchronous_lifecycle_uses_batch_notes_and_windowed_pagination() {
    let (adapter, faults) = adapter();
    let manager = SessionMidiTransactionManager::new(adapter.clone(), None);
    let preview = manager.preview_async(&mut request()).await.unwrap();
    let id = preview["transactionId"].as_str().unwrap();
    assert_eq!(manager.apply_async(id, &json!("apply"), "async-apply", None).await.unwrap()["state"], "applied");
    assert_eq!(manager.apply_async(id, &json!("apply"), "async-apply", None).await.unwrap()["idempotent"], true);
    assert_eq!(*faults.operations.borrow(), vec!["clip.create", "note.add-batch"]);
    let page = discover_session_async(adapter.clone(), "note", 1.0, None).await.unwrap();
    assert_eq!(page["truncated"], true);
    assert_eq!(
        discover_session_async(adapter, "note", 10.0, page["nextCursor"].as_str()).await.unwrap()["items"].as_array().unwrap().len(),
        2
    );
    assert_eq!(manager.undo_async(id, &json!("undo"), "undo", None).await.unwrap()["state"], "undone");
    assert_eq!(manager.undo_async(id, &json!("undo"), "undo", None).await.unwrap()["idempotent"], true);
}
#[tokio::test]
async fn lost_deletion_acknowledgement_only_reconciles_with_exact_key() {
    let (adapter, faults) = adapter();
    let manager = SessionMidiTransactionManager::new(adapter, None);
    let preview = manager.preview_async(&mut request()).await.unwrap();
    let id = preview["transactionId"].as_str().unwrap();
    manager.apply_async(id, &json!("apply"), "apply", Some(&context(id, "apply"))).await.unwrap();
    faults.lose_delete.set(true);
    assert!(manager.undo_async(id, &json!("undo"), "undo", Some(&context(id, "undo"))).await.unwrap_err().message().contains("uncertain"));
    assert!(manager
        .undo_async(id, &json!("undo"), "wrong", Some(&context(id, "wrong")))
        .await
        .unwrap_err()
        .message()
        .contains("exact-key"));
    assert_eq!(manager.undo_async(id, &json!("undo"), "undo", Some(&context(id, "undo"))).await.unwrap()["state"], "undone");
}
#[tokio::test]
async fn failed_undo_read_dispatches_nothing_and_can_retry() {
    let (adapter, faults) = adapter();
    let manager = SessionMidiTransactionManager::new(adapter, None);
    let preview = manager.preview_async(&mut request()).await.unwrap();
    let id = preview["transactionId"].as_str().unwrap();
    manager.apply_async(id, &json!("apply"), "apply", None).await.unwrap();
    faults.fail_read.set(true);
    faults.operations.borrow_mut().clear();
    assert!(manager.undo_async(id, &json!("undo"), "undo", None).await.unwrap_err().message().contains("timed out"));
    assert!(faults.operations.borrow().is_empty());
    assert_eq!(manager.undo_async(id, &json!("undo"), "undo", None).await.unwrap()["state"], "undone");
}
#[tokio::test]
async fn partial_note_batch_compensation_with_lost_acknowledgement_reconciles_without_clip() {
    let (adapter, faults) = adapter();
    faults.partial_notes.set(true);
    faults.lose_delete.set(true);
    let manager = SessionMidiTransactionManager::new(adapter.clone(), None);
    let preview = manager.preview_async(&mut request()).await.unwrap();
    let id = preview["transactionId"].as_str().unwrap();
    assert!(manager
        .apply_async(id, &json!("apply"), "apply", Some(&context(id, "apply")))
        .await
        .unwrap_err()
        .message()
        .contains("compensation failed"));
    assert_eq!(manager.apply_async(id, &json!("apply"), "apply", Some(&context(id, "apply"))).await.unwrap()["state"], "compensated");
    assert_eq!(adapter.sim.state.borrow()["tracks"][0]["clipSlots"][1]["empty"], true);
    assert_eq!(faults.operations.borrow().iter().filter(|op| *op == "clip.delete").count(), 1);
}
#[tokio::test]
async fn changed_owned_clip_after_lost_note_acknowledgement_cannot_be_compensated() {
    let (adapter, faults) = adapter();
    faults.lose_notes.set(true);
    faults.cache_create.set(true);
    let manager = SessionMidiTransactionManager::new(adapter.clone(), None);
    let preview = manager.preview_async(&mut request()).await.unwrap();
    let id = preview["transactionId"].as_str().unwrap();
    assert!(manager
        .apply_async(id, &json!("apply"), "apply", Some(&context(id, "apply")))
        .await
        .unwrap_err()
        .message()
        .contains("uncertain"));
    adapter.sim.state.borrow_mut()["tracks"][0]["clips"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|clip| clip["name"] == "Bounded Beat")
        .unwrap()["name"] = "Human edit".into();
    let error = manager.apply_async(id, &json!("apply"), "apply", Some(&context(id, "apply"))).await.unwrap_err();
    assert!(error.message().contains("compensation failed"), "{error}");
    assert_eq!(faults.operations.borrow().iter().filter(|op| *op == "clip.delete").count(), 0);
    assert!(manager.is_finalizable(id));
    assert_eq!(manager.finalize(id).unwrap()["priorState"], "uncertain");
}
#[tokio::test]
async fn lossy_expression_write_is_verified_and_compensated() {
    let (adapter, faults) = adapter();
    faults.strip_expression.set(true);
    let manager = SessionMidiTransactionManager::new(adapter.clone(), None);
    let mut requested = request();
    requested["notes"] = json!([{"pitch":36,"start":0,"duration":0.25,"velocity":100,"probability":0.5,"velocityDeviation":8,"releaseVelocity":32,"mute":true}]);
    let preview = manager.preview_async(&mut requested).await.unwrap();
    assert_eq!(requested["notes"][0]["channel"], 1);
    let id = preview["transactionId"].as_str().unwrap();
    assert!(manager
        .apply_async(id, &json!("apply"), "apply", None)
        .await
        .unwrap_err()
        .message()
        .contains("confirm exact MIDI clip contents"));
    assert_eq!(adapter.sim.state.borrow()["tracks"][0]["clipSlots"][1]["empty"], true);
}
#[test]
fn pagination_retains_node_base64_and_parse_int_cursor_behavior() {
    use base64::Engine;
    let sim = simulator();
    for decoded in ["1suffix", " +1", "01", "1.9", "\u{feff}1", "1e9"] {
        let cursor = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(decoded);
        assert_eq!(discover_session(&*sim, "scene", 1.0, Some(&format!("!{cursor}="))).unwrap()["items"][0]["index"], 1);
    }
    for decoded in ["-1", "5", "no", "", "Infinity"] {
        let cursor = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(decoded);
        if cursor.is_empty() {
            continue;
        }
        assert!(discover_session(&*sim, "scene", 1.0, Some(&cursor)).unwrap_err().message().contains("invalid cursor"));
    }
}
#[test]
fn validation_and_session_pages_match_typescript_oracle() {
    let fixture: Value = serde_json::from_str(include_str!("fixtures/session-midi-oracle.json")).unwrap();
    for row in fixture["rows"].as_array().unwrap() {
        let manager = SessionMidiTransactionManager::new(simulator(), None);
        let mut request = row["input"].clone();
        let result = manager.preview(&mut request);
        if let Some(error) = row.get("error") {
            let error_actual = result.unwrap_err();
            assert_eq!(error_actual.name(), error["name"].as_str().unwrap(), "{}", row["name"]);
            assert_eq!(error_actual.message(), error["message"].as_str().unwrap(), "{}", row["name"]);
        } else {
            let mut result = result.unwrap();
            result.as_object_mut().unwrap().shift_remove("transactionId");
            result.as_object_mut().unwrap().shift_remove("expiresAt");
            assert_eq!(kumi_common::js::json::stringify(&result), kumi_common::js::json::stringify(&row["result"]), "{}", row["name"]);
        }
        assert_eq!(request, row["mutated"], "mutation {}", row["name"]);
    }
    for row in fixture["pages"].as_array().unwrap() {
        let result = discover_session(
            &*simulator(),
            row["kind"].as_str().unwrap(),
            row["limit"].as_f64().unwrap(),
            row.get("cursor").and_then(Value::as_str),
        );
        if let Some(error) = row.get("error") {
            assert_eq!(result.unwrap_err().message(), error.as_str().unwrap(), "{row}");
        } else {
            let actual = result.unwrap();
            let canonical = |v: &Value| {
                ableton_mcp_server::registry::canonical_json(v, &ableton_mcp_server::registry::UNBOUNDED_CANONICAL_LIMITS).unwrap()
            };
            assert_eq!(canonical(&actual), canonical(&row["result"]), "{row}");
        }
    }
}
#[tokio::test]
async fn empty_clip_retains_authoritative_note_revision_and_preview_fences_epoch_and_occupancy() {
    let sim = simulator();
    let manager = SessionMidiTransactionManager::new(sim.clone(), None);
    let mut empty = request();
    empty["notes"] = json!([]);
    let preview = manager.preview_async(&mut empty).await.unwrap();
    let id = preview["transactionId"].as_str().unwrap();
    let result = manager.apply_async(id, &json!("apply"), "empty", None).await.unwrap();
    assert_eq!(result["notes"], json!([]));
    assert!(manager.preview_async(&mut request()).await.unwrap_err().message().contains("occupied"));
    manager.undo_async(id, &json!("undo"), "empty-undo", None).await.unwrap();
    let preview = manager.preview_async(&mut request()).await.unwrap();
    let id = preview["transactionId"].as_str().unwrap();
    sim.reconnect().unwrap();
    assert!(manager.apply_async(id, &json!("apply"), "old-epoch", None).await.unwrap_err().message().contains("epoch changed"));
}
