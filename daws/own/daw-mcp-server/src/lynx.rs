//! LYNX's host surface. The companion program owns the native protocol; the host
//! uses the same authenticated bridge, journal and MCP handlers as other DAWs.
use serde_json::{json, Value};

pub const READ_KINDS: &[&str] = &[
    "project",
    "document",
    "track",
    "channel",
    "mixer",
    "pattern",
    "note",
    "device",
    "param",
    "parameter",
    "instrument",
    "effect",
    "playlist",
    "clip",
    "call",
];
pub const BODIES: &[&str] = &["pattern", "playlist", "mixer", "fx"];

/// Offer only methods the native endpoint and companion implement. In particular
/// there is no save, capture/render, arbitrary file import or grouped native undo.
pub fn offered_tools(mut tools: Value) -> Value {
    let Some(list) = tools.as_array_mut() else { return json!([]) };
    list.retain(|tool| {
        matches!(
            tool["name"].as_str(),
            Some(
                "status"
                    | "discover"
                    | "read"
                    | "snapshot"
                    | "focus"
                    | "focus_body"
                    | "change_preview"
                    | "change_apply"
                    | "change_undo"
                    | "change_release"
                    | "transport_stop"
                    | "transport_play"
                    | "catch_up"
                    | "baseline_save"
                    | "watch_start"
                    | "watch_stop"
            )
        )
    });
    for tool in list {
        match tool["name"].as_str() {
            Some("read") => {
                tool["inputSchema"]["properties"]["epoch"] = json!({"type":"string", "description":"The LYNX state epoch returned by the read that supplied a ref, parent or cursor. Stale positional references are refused."});
                tool["inputSchema"]["properties"]["fn"] = json!({"type":"string", "description":"For kind call: one of the native read-only operations advertised by this connection, such as instrument.catalog or plugin.inspect. Mutations are refused."});
                tool["inputSchema"]["properties"]["args"] = json!({"type":"object", "description":"For kind call: the operation's bounded arguments. Exact catalog IDs and indices are data, never file paths to load."});
            }
            Some("change_undo") => {
                tool["description"] = "Undo a journalled Kumi edit only while it is the most recent owned native undo step and its observed state is intact. Refused after unrelated producer edits; discard cannot bypass this protection.".into();
            }
            Some("change_apply") => {
                tool["description"] = "Apply a previewed LYNX change as one native undo step. The host journals idempotency keys; an uncertain write is never automatically repeated. Several plan steps are separate native undo steps.".into();
            }
            _ => {}
        }
    }
    tools
}
