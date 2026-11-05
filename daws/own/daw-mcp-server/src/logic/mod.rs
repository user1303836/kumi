//! Logic's native M3 capabilities. The shared host owns the journal; only the methods implemented by the native
//! bridge are exposed here. Logic has no native undo block, so a plan's step counts changes without grouping them.
use serde_json::{json, Value};

/// The bridge's implemented kinds, from the original M3 native change contract.
pub const CHANGE_KINDS: [&str; 16] = [
    "track-volume",
    "track-pan",
    "track-mute",
    "track-solo",
    "track-arm",
    "track-rename",
    "track-add",
    "track-delete",
    "marker-add",
    "marker-move",
    "marker-delete",
    "tempo-set",
    "notes-add",
    "notes-change",
    "notes-delete",
    "fx-bypass",
];

/// These native writes have no entry in Logic's own undo history and are restored by value.
pub const OUTSIDE_UNDO: &[&str] = &["track-volume", "track-pan", "track-mute", "track-solo", "track-arm", "tempo-set", "fx-bypass"];

/// The current simulator location, also exported beside the DAW's host capabilities for integration tests.
pub mod simulator {
    pub use crate::simulator::logic::*;
}

fn picked(pick: &Value, more: Value) -> Value {
    let mut properties = pick.as_object().cloned().unwrap_or_default();
    properties.extend(more.as_object().into_iter().flat_map(|fields| fields.iter()).map(|(name, value)| (name.clone(), value.clone())));
    Value::Object(properties)
}

pub(crate) fn change_tools(pick: &Value) -> Vec<Value> {
    vec![
        json!({
            "name":"change_preview", "title":"Preview a Logic Pro change",
            "description":"What one change would do in Logic Pro, without making it: {transactionId, confirmation, prior, display, target, epoch}. Apply it with change_apply. Only the listed M3 kinds are implemented. A preview reports unavailable controls; an apply may be refused or uncertain.",
            "inputSchema":{"type":"object","properties":picked(pick,json!({
                "kind":{"type":"string","enum":CHANGE_KINDS},
                "input":{"type":"object","description":"The change kind's input, with refs from the current epoch."},
                "epoch":{"type":"string","description":"The epoch the input refs came from."}
            })),"required":["kind","input"],"additionalProperties":false},
            "annotations":{"readOnlyHint":true,"idempotentHint":false,"openWorldHint":false}
        }),
        json!({
            "name":"change_apply", "title":"Apply a Logic Pro change",
            "description":"Applies a preview after checking its fence again, and returns the values read back. The same idempotencyKey returns the first answer. An unanswered write is uncertain and is never sent again. Mixer controls, tempo and bypass have no native undo entry; other changes keep their own Logic undo steps.",
            "inputSchema":{"type":"object","properties":{
                "transactionId":{"type":"string"},"confirmation":{"type":"string"},"idempotencyKey":{"type":"string"},"title":{"type":"string"}
            },"required":["transactionId","confirmation","idempotencyKey"],"additionalProperties":false},
            "annotations":{"readOnlyHint":false,"destructiveHint":false,"idempotentHint":true,"openWorldHint":false}
        }),
        json!({
            "name":"change_undo", "title":"Undo a Logic Pro change",
            "description":"Restores exactly what a change set, after checking those values still match. A producer edit is refused unless discard is true. A deleted track can return only while its native undo step is still on top; an uncertain write is never undone blindly.",
            "inputSchema":{"type":"object","properties":{
                "transactionId":{"type":"string"},"confirmation":{"const":"undo"},"idempotencyKey":{"type":"string"},"discard":{"type":"boolean"}
            },"required":["transactionId","confirmation","idempotencyKey"],"additionalProperties":false},
            "annotations":{"readOnlyHint":false,"destructiveHint":true,"idempotentHint":true,"openWorldHint":false}
        }),
        json!({
            "name":"transport_stop", "title":"Stop Logic Pro", "description":"Stops Logic Pro's playback and recording.",
            "inputSchema":{"type":"object","properties":pick,"additionalProperties":false},
            "annotations":{"readOnlyHint":false,"destructiveHint":false,"idempotentHint":true,"openWorldHint":false}
        }),
        json!({
            "name":"transport_play", "title":"Play Logic Pro", "description":"Starts Logic Pro's playback from the playhead.",
            "inputSchema":{"type":"object","properties":picked(pick,json!({"epoch":{"type":"string"}})),"additionalProperties":false},
            "annotations":{"readOnlyHint":false,"destructiveHint":false,"idempotentHint":true,"openWorldHint":false}
        }),
        json!({
            "name":"cursor_set", "title":"Move Logic Pro's playhead", "description":"Moves Logic Pro's playhead by positionBeats (quarter notes from the start) or position (seconds), to the bar and beat the native display can set.",
            "inputSchema":{"type":"object","properties":picked(pick,json!({"positionBeats":{"type":"number","minimum":0},"position":{"type":"number","minimum":0},"epoch":{"type":"string"}})),"additionalProperties":false},
            "annotations":{"readOnlyHint":false,"destructiveHint":false,"idempotentHint":true,"openWorldHint":false}
        }),
        json!({
            "name":"undo_step_begin", "title":"Begin counting a Logic Pro plan", "description":"Begins counting changes in a plan: {stepId}. Logic Pro has no native undo block, so each change retains its own native history behavior. Close it with undo_step_end.",
            "inputSchema":{"type":"object","properties":{"label":{"type":"string"}},"required":["label"],"additionalProperties":false},
            "annotations":{"readOnlyHint":false,"destructiveHint":false,"idempotentHint":false,"openWorldHint":false}
        }),
        json!({
            "name":"undo_step_end", "title":"End counting a Logic Pro plan", "description":"Ends the counted plan and returns its description and change count, with entry:false. It creates no combined native undo step.",
            "inputSchema":{"type":"object","properties":{"stepId":{"type":"string"},"label":{"type":"string","maxLength":200}},"required":["stepId"],"additionalProperties":false},
            "annotations":{"readOnlyHint":false,"destructiveHint":false,"idempotentHint":true,"openWorldHint":false}
        }),
        json!({
            "name":"change_release", "title":"Forget changes", "description":"Forgets changes that will no longer be undone, at most 64 at a time. This changes only the host's journal.",
            "inputSchema":{"type":"object","properties":{"transactionIds":{"type":"array","items":{"type":"string"},"minItems":1,"maxItems":64}},"required":["transactionIds"],"additionalProperties":false},
            "annotations":{"readOnlyHint":false,"destructiveHint":false,"idempotentHint":true,"openWorldHint":false}
        }),
    ]
}
