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

pub(crate) const MANUAL_TRACK_ADD: &str =
    "Kumi can't safely remove a track it added because Logic Pro doesn't expose all of its contents. Use Logic Pro's own undo manually.";
pub(crate) const MANUAL_MARKER: &str = "Kumi can't verify the marker's complete original contents because Logic Pro didn't expose every required field. Use Logic Pro's own undo manually; showing a field now can't recover its earlier value.";

fn manual_hint(reason: &'static str) -> Value {
    json!({"mode":"manual", "reason":reason, "guidance":"Use Logic Pro's own undo manually."})
}

/// The opaque material stays in the host journal. A positive restore capability requires both the native hint
/// and complete captured original/applied identity; display text or title equality never grants permission.
pub(crate) fn restore_hint(kind: &str, reply: &Value) -> Option<Value> {
    if kind == "track-add" {
        return Some(manual_hint(MANUAL_TRACK_ADD));
    }
    if !["marker-add", "marker-move"].contains(&kind) {
        return None;
    }
    let material = &reply["material"];

    let prior = material["prior"].as_array();
    let applied = material["applied"].as_array();
    let marker = &material["marker"];
    let known = material["undo"] == json!({"mode":"value"})
        && complete_marker_identity(marker)
        && prior.is_some_and(|rows| rows.iter().all(complete_marker_identity))
        && applied.is_some_and(|rows| rows.iter().all(complete_marker_identity) && rows.iter().filter(|row| *row == marker).count() == 1)
        && (kind != "marker-move"
            || (complete_marker_identity(&material["priorMarker"])
                && prior.is_some_and(|rows| rows.iter().filter(|row| **row == material["priorMarker"]).count() == 1)));
    Some(if known { json!({"mode":"value"}) } else { manual_hint(MANUAL_MARKER) })
}

pub(crate) fn complete_marker_identity(row: &Value) -> bool {
    let Some(columns) = row["columns"].as_array() else { return false };
    let names: Vec<&str> = columns.iter().filter_map(Value::as_str).collect();
    let Some(other) = row["other"].as_object() else { return false };
    let extras: Vec<&str> = names.iter().copied().filter(|column| !["Position", "Name", "Length", "Text"].contains(column)).collect();
    ["bar", "beat", "division", "tick"].iter().all(|field| row["position"][*field].as_u64().is_some_and(|n| n > 0))
        && row["name"].is_string()
        && row["text"].is_string()
        && (row["length"].as_f64().is_some_and(|n| n.is_finite() && n >= 0.0) || row["length"] == "infinite")
        && columns.len() == names.len()
        && ["Position", "Name", "Length", "Text"].iter().all(|column| names.contains(column))
        && names.iter().enumerate().all(|(i, column)| !names[..i].contains(column))
        && row["ax"]["role"] == "AXRow"
        && ["id", "subrole"].iter().all(|field| row["ax"].get(*field).is_none_or(Value::is_string))
        && other.len() == extras.len()
        && extras.iter().all(|column| other.get(*column).is_some_and(complete_cell))
}

fn complete_cell(cell: &Value) -> bool {
    cell["role"].as_str().is_some_and(|role| !role.is_empty())
        && cell["children"].as_array().is_some_and(|children| children.iter().all(complete_cell))
        && cell.get("more").is_none_or(|more| more.as_u64() == Some(0))
        && known_cell_value(cell)
}

fn known_cell_value(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Array(values) => values.iter().all(known_cell_value),
        Value::Object(fields) => fields.values().all(known_cell_value),
        _ => true,
    }
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
            "description":"Restores reversible values after checking their identity. Added tracks, deleted tracks and deleted markers require manual Logic Undo History. Marker additions and moves also require manual undo when any original identity field was unavailable; showing it later cannot recover its earlier value. Discard never bypasses unknown or changed marker identity. An uncertain write is never undone blindly.",
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

#[cfg(test)]
mod restore_tests {
    use super::*;

    #[test]
    fn incomplete_or_unrecognized_native_evidence_never_grants_marker_restore() {
        let row = json!({"position":{"bar":9,"beat":1,"division":1,"tick":1},"name":"Marker 1","length":32.0,"text":"",
            "columns":["Lock","Position","Name","Length","Text"],"ax":{"role":"AXRow","subrole":"AXTableRow"},
            "other":{"Lock":{"role":"AXCell","children":[]}}});
        let complete = json!({"material":{"marker":row,"priorMarker":row,"prior":[row],"applied":[row],"undo":{"mode":"value"}}});
        for kind in ["marker-add", "marker-move"] {
            assert_eq!(restore_hint(kind, &complete), Some(json!({"mode":"value"})));
            for path in [
                "/material/undo",
                "/material/marker/text",
                "/material/prior/0/text",
                "/material/applied/0/name",
                "/material/marker/length",
                "/material/marker/columns",
                "/material/marker/ax/role",
                "/material/marker/other/Lock/children",
            ] {
                let mut reply = complete.clone();
                *reply.pointer_mut(path).unwrap() = Value::Null;
                assert_eq!(restore_hint(kind, &reply).unwrap()["mode"], "manual", "{kind}: {path}");
            }
            let mut missing = complete.clone();
            missing["material"].as_object_mut().unwrap().remove("undo");
            assert_eq!(restore_hint(kind, &missing).unwrap()["mode"], "manual");
            missing["material"]["undo"] = json!({"mode":"value", "unknown":true});
            assert_eq!(restore_hint(kind, &missing).unwrap()["mode"], "manual");
        }
        assert_eq!(restore_hint("track-add", &complete).unwrap()["mode"], "manual");
        assert!(restore_hint("track-volume", &complete).is_none(), "other kinds do not interpret material.undo");
    }
}
