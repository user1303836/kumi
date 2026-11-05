use ableton_mcp_server::{
    host::{McpHost, McpHostOptions},
    live::DeterministicLiveSimulator,
};
use serde_json::{json, Value};
use std::rc::Rc;

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
