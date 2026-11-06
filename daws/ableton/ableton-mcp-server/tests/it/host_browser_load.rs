use ableton_mcp_server::{
    host::{McpHost, McpHostOptions},
    live::*,
};
use serde_json::{json, Value};
use std::rc::Rc;

/// The simulator as the bridge hears the Remote Script through its socket, where a refusal reads
/// "request failed: …; nothing changed".
struct OverTheSocket(Rc<DeterministicLiveSimulator>);
fn as_the_socket_says(error: LiveError) -> LiveError {
    LiveError::error(format!("request failed: {}", error.message()))
}
impl LiveAdapter for OverTheSocket {
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
        self.0.invoke(i).map_err(as_the_socket_says)
    }
    fn subscribe(&self, l: LiveListener) -> Result<Unsubscribe, LiveError> {
        self.0.subscribe(l)
    }
    fn reconnect(&self) -> Result<LiveStatus, LiveError> {
        self.0.reconnect()
    }
}
#[async_trait::async_trait(?Send)]
impl AsyncLiveAdapter for OverTheSocket {
    async fn snapshot_async(&self, c: Option<&LiveOperationContext>, r: Option<&LiveSnapshotRequest>) -> Result<LiveSnapshot, LiveError> {
        self.0.snapshot_async(c, r).await
    }
    async fn discover_async(&self, r: &LiveDiscoveryRequest, c: Option<&LiveOperationContext>) -> Result<LiveDiscoveryResult, LiveError> {
        self.0.discover_async(r, c).await
    }
    async fn get_async(&self, r: &LiveRef, _: Option<&LiveOperationContext>) -> Result<Option<Value>, LiveError> {
        self.0.get(r)
    }
    async fn invoke_async(&self, i: &LiveInvocation, _: Option<&LiveOperationContext>) -> Result<Value, LiveError> {
        self.0.invoke(i).map_err(as_the_socket_says)
    }
    async fn reconnect_async(&self, _: Option<&LiveOperationContext>) -> Result<LiveStatus, LiveError> {
        self.0.reconnect()
    }
    async fn close(&self) -> Result<(), LiveError> {
        Ok(())
    }
}

fn text(answer: &Value) -> Value {
    serde_json::from_str(answer["result"]["content"][0]["text"].as_str().unwrap_or("null")).unwrap_or(Value::Null)
}

/// Loads and previews a Browser load that keeps the selection (listening's Kumi Ears), with no parameter selected.
async fn load_keeping_the_selection(host: &McpHost, sim: &DeterministicLiveSimulator) -> (Value, Value) {
    sim.state.borrow_mut()["selection"]["parameterRef"] = Value::Null;
    let selection = sim.state.borrow()["selection"].clone();
    let args = json!({"itemId":"audio_effects/Utility","trackRef":"track:track-1","preserveSelection":true});
    let preview = text(&host.live_browser_load_preview_async(&json!(1), &args).await);
    assert_eq!(preview["selection"], selection, "{preview}");
    let tx = preview["transactionId"].clone();
    let apply = json!({"transactionId":tx,"confirmation":"apply","idempotencyKey":"keep-apply-1"});
    assert_eq!(text(&host.live_browser_load_apply_async(&json!(2), &apply, None).await.unwrap())["state"], "applied");
    (tx, selection)
}
async fn undo_keeping_the_selection(host: &McpHost, tx: &Value, call: u64) -> Value {
    let undo = json!({"transactionId":tx,"confirmation":"undo","idempotencyKey":"keep-undo-1","preserveSelection":true});
    text(
        &host
            .with_undo_watch(&json!(call), &undo, async { Ok(host.undo_browser_load_async(&json!(call), &undo, None).await) })
            .await
            .unwrap(),
    )
}

/// The preview reads what's selected before anything loads and refuses a parameter (no script can select one); with
/// the selection still what the load left, the undo brings back what was selected before it.
#[tokio::test]
async fn a_browser_load_previewed_to_keep_the_selection_undoes_with_it_and_refuses_one_it_cant_keep() {
    // The simulator's Set has its gain parameter selected: nothing loads.
    let sim = Rc::new(DeterministicLiveSimulator::new());
    let host = McpHost::new(sim.clone(), McpHostOptions::default()).unwrap();
    let devices = sim.state.borrow()["tracks"][0]["devices"].clone();
    assert!(sim.state.borrow()["selection"]["parameterRef"].is_string());
    let args = json!({"itemId":"audio_effects/Utility","trackRef":"track:track-1","preserveSelection":true});
    let refused = host.live_browser_load_preview_async(&json!(1), &args).await;
    assert!(refused.to_string().contains("parameter is selected"), "{refused}");
    assert_eq!(sim.state.borrow()["tracks"][0]["devices"], devices, "nothing loaded");
    let (tx, selection) = load_keeping_the_selection(&host, &sim).await;
    let undone = undo_keeping_the_selection(&host, &tx, 3).await;
    assert_eq!((undone["state"].clone(), undone["deleted"].clone()), (json!("undone"), json!(true)), "{undone}");
    assert_eq!(undone["selection"], json!({"before":selection,"after":selection,"restored":true,"previous":true}));
    assert_eq!(sim.state.borrow()["selection"], selection);
    assert_eq!(sim.state.borrow()["tracks"][0]["devices"], devices, "the loaded device is gone");
    // Asked again with the same key, it says the same.
    let again = undo_keeping_the_selection(&host, &tx, 4).await;
    assert_eq!((again["idempotent"].clone(), again["selection"].clone()), (json!(true), undone["selection"].clone()), "{again}");
}

/// What the producer chooses while Ears listens is theirs: the undo keeps it, not what was selected before the load.
#[tokio::test]
async fn a_selection_the_producer_makes_during_the_capture_stays_theirs() {
    let sim = Rc::new(DeterministicLiveSimulator::new());
    let host = McpHost::new(sim.clone(), McpHostOptions::default()).unwrap();
    let (tx, before) = load_keeping_the_selection(&host, &sim).await;
    sim.state.borrow_mut()["selection"]["detailClipRef"] = Value::Null;
    sim.state.borrow_mut()["selection"]["deviceRef"] = Value::Null;
    let chosen = sim.state.borrow()["selection"].clone();
    assert_ne!(chosen, before);
    let undone = undo_keeping_the_selection(&host, &tx, 3).await;
    assert_eq!(undone["deleted"], true, "{undone}");
    assert_eq!(undone["selection"], json!({"before":chosen,"after":chosen,"restored":true,"previous":false}));
    assert_eq!(sim.state.borrow()["selection"], chosen, "still the producer's");
}

/// Live moves the selection to the deleted device's track on its next tick, and the Remote Script selects what the delete
/// kept on its display tick after that: the undo reads the selection until it holds, and only then says it's kept.
#[tokio::test]
async fn the_undo_waits_for_the_selection_live_moved_to_come_back() {
    let sim = Rc::new(DeterministicLiveSimulator::new());
    let host = McpHost::new(sim.clone(), McpHostOptions::default()).unwrap();
    let (tx, selection) = load_keeping_the_selection(&host, &sim).await;
    sim.state.borrow_mut()["liveMovesTheSelection"] = json!({"reads": 2});
    let undone = undo_keeping_the_selection(&host, &tx, 3).await;
    assert_eq!(undone["selection"], json!({"before":selection,"after":selection,"restored":true,"previous":true}), "{undone}");
    assert_eq!(sim.state.borrow()["selection"], selection);
}

/// A selection Live won't take back: the device still goes, and the undo says the selection wasn't kept, and where it is.
#[tokio::test]
async fn a_selection_live_wont_take_back_is_said_and_the_device_still_goes() {
    let sim = Rc::new(DeterministicLiveSimulator::new());
    let host = McpHost::new(sim.clone(), McpHostOptions::default()).unwrap();
    let (tx, selection) = load_keeping_the_selection(&host, &sim).await;
    let devices = sim.state.borrow()["tracks"][0]["devices"].clone();
    sim.state.borrow_mut()["liveMovesTheSelection"] = json!({});
    let undone = undo_keeping_the_selection(&host, &tx, 3).await;
    assert_eq!((undone["deleted"].clone(), undone["selection"]["restored"].clone()), (json!(true), json!(false)), "{undone}");
    assert_eq!(undone["selection"]["after"], sim.state.borrow()["selection"], "where Live left it");
    assert_ne!(undone["selection"]["after"], selection);
    assert_eq!(array_len(&sim.state.borrow()["tracks"][0]["devices"]) + 1, array_len(&devices), "the loaded device is gone");
}

fn array_len(value: &Value) -> usize {
    value.as_array().map_or(0, Vec::len)
}

/// Keeping the selection is only for a Browser load previewed to keep it; refused, the load stays undoable.
#[tokio::test]
async fn keeping_the_selection_is_refused_for_a_load_not_previewed_to_keep_it() {
    let sim = Rc::new(DeterministicLiveSimulator::new());
    let host = McpHost::new(sim.clone(), McpHostOptions::default()).unwrap();
    let devices = sim.state.borrow()["tracks"][0]["devices"].clone();
    let preview =
        text(&host.live_browser_load_preview_async(&json!(1), &json!({"itemId":"audio_effects/Utility","trackRef":"track:track-1"})).await);
    let tx = preview["transactionId"].clone();
    let apply = json!({"transactionId":tx,"confirmation":"apply","idempotencyKey":"plain-apply-1"});
    host.live_browser_load_apply_async(&json!(2), &apply, None).await.unwrap();
    let keep = json!({"transactionId":tx,"confirmation":"undo","idempotencyKey":"plain-undo-1","preserveSelection":true});
    let refused =
        host.with_undo_watch(&json!(3), &keep, async { Ok(host.undo_browser_load_async(&json!(3), &keep, None).await) }).await.unwrap();
    assert!(refused.to_string().contains("wasn't previewed with preserveSelection"), "{refused}");
    assert_ne!(sim.state.borrow()["tracks"][0]["devices"], devices, "still loaded");
    let plain = json!({"transactionId":tx,"confirmation":"undo","idempotencyKey":"plain-undo-2"});
    let undone = text(
        &host.with_undo_watch(&json!(4), &plain, async { Ok(host.undo_browser_load_async(&json!(4), &plain, None).await) }).await.unwrap(),
    );
    assert_eq!(undone["state"], "undone", "{undone}");
    assert_eq!(sim.state.borrow()["tracks"][0]["devices"], devices);
}

/// The selection the undo keeps is the one the load read in its own step: one the producer makes between preview and
/// apply is what comes back, never the preview's older reading.
#[tokio::test]
async fn what_the_producer_selects_between_preview_and_apply_is_what_the_undo_keeps() {
    let sim = Rc::new(DeterministicLiveSimulator::new());
    let host = McpHost::new(sim.clone(), McpHostOptions::default()).unwrap();
    sim.state.borrow_mut()["selection"]["parameterRef"] = Value::Null;
    let args = json!({"itemId":"audio_effects/Utility","trackRef":"track:track-1","preserveSelection":true});
    let preview = text(&host.live_browser_load_preview_async(&json!(1), &args).await);
    sim.state.borrow_mut()["selection"]["detailClipRef"] = Value::Null;
    let chosen = sim.state.borrow()["selection"].clone();
    assert_ne!(preview["selection"], chosen);
    let tx = preview["transactionId"].clone();
    let apply = json!({"transactionId":tx,"confirmation":"apply","idempotencyKey":"keep-apply-1"});
    assert_eq!(text(&host.live_browser_load_apply_async(&json!(2), &apply, None).await.unwrap())["state"], "applied");
    let undone = undo_keeping_the_selection(&host, &tx, 3).await;
    assert_eq!(undone["selection"], json!({"before":chosen,"after":chosen,"restored":true,"previous":true}));
}

/// A selection the delete can't keep (here, the loaded device itself) refuses with nothing deleted, says why in a field,
/// and a plain undo still removes the device.
#[tokio::test]
async fn a_selection_the_undo_cant_keep_refuses_and_a_plain_undo_still_removes_the_device() {
    let sim = Rc::new(DeterministicLiveSimulator::new());
    let host = McpHost::new(Rc::new(OverTheSocket(sim.clone())), McpHostOptions::default()).unwrap();
    let devices = sim.state.borrow()["tracks"][0]["devices"].clone();
    let (tx, _) = load_keeping_the_selection(&host, &sim).await;
    let created = host.transaction_record(tx.as_str().unwrap()).unwrap().borrow()["created"]["deviceRef"].clone();
    sim.state.borrow_mut()["selection"]["deviceRef"] = created;
    let keep = json!({"transactionId":tx,"confirmation":"undo","idempotencyKey":"keep-undo-1","preserveSelection":true});
    let refused =
        host.with_undo_watch(&json!(3), &keep, async { Ok(host.undo_browser_load_async(&json!(3), &keep, None).await) }).await.unwrap();
    assert_eq!(text(&refused)["code"], "selection_not_kept", "{refused}");
    assert_ne!(sim.state.borrow()["tracks"][0]["devices"], devices, "nothing deleted");
    let plain = json!({"transactionId":tx,"confirmation":"undo","idempotencyKey":"plain-undo-2"});
    let undone = text(
        &host.with_undo_watch(&json!(4), &plain, async { Ok(host.undo_browser_load_async(&json!(4), &plain, None).await) }).await.unwrap(),
    );
    assert_eq!(undone["state"], "undone", "{undone}");
    assert_eq!(sim.state.borrow()["tracks"][0]["devices"], devices);
}

/// A loaded device the producer has removed already can't be removed keeping the selection: nothing is selected or
/// claimed, and the undo doesn't say it's undone.
#[tokio::test]
async fn a_loaded_device_already_gone_keeps_the_selection_as_it_is_and_claims_nothing() {
    let sim = Rc::new(DeterministicLiveSimulator::new());
    let host = McpHost::new(sim.clone(), McpHostOptions::default()).unwrap();
    let devices = sim.state.borrow()["tracks"][0]["devices"].clone();
    let (tx, _) = load_keeping_the_selection(&host, &sim).await;
    sim.state.borrow_mut()["tracks"][0]["devices"] = devices;
    sim.state.borrow_mut()["selection"]["sceneRef"] = Value::Null;
    let chosen = sim.state.borrow()["selection"].clone();
    let undone = undo_keeping_the_selection(&host, &tx, 3).await;
    assert_ne!(undone["state"], "undone", "{undone}");
    assert!(undone.get("selection").is_none(), "{undone}");
    assert_eq!(sim.state.borrow()["selection"], chosen);
}
