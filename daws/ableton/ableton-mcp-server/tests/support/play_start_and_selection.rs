use serde_json::Value;

/// Keep replaying the recorded source for the tool descriptors from before Live's track listening: where play starts
/// (live_transport_preview's startTime) and a Browser load whose undo keeps the selection (live_browser_load_preview's
/// and live_undo's preserveSelection). The catalog's own tests pin them as they are; this projection takes out only
/// these properties and puts back the words they changed, each pair (now, then).
const ADDED: &[(&str, &str)] =
    &[("live_transport_preview", "startTime"), ("live_browser_load_preview", "preserveSelection"), ("live_undo", "preserveSelection")];
const SINCE: &[(&str, &str)] = &[(
    "Read-only preflight for one bounded transport change (position, start time, loop, punch, metronome, count-in) with a playback-revision fence.",
    "Read-only preflight for one bounded transport change (position, loop, punch, metronome, count-in) with a playback-revision fence.",
)];

/// Tool descriptors (an array of them, as tools/list and the catalog give them) as the source recorded them.
pub fn as_recorded(tools: &mut Value) {
    for tool in tools.as_array_mut().into_iter().flatten() {
        let name = tool["name"].as_str().unwrap_or_default().to_owned();
        for (_, property) in ADDED.iter().filter(|(owner, _)| *owner == name) {
            if let Some(properties) = tool.pointer_mut("/inputSchema/properties").and_then(Value::as_object_mut) {
                // shift_remove, not remove: the others keep their order, which the recorded hashes need.
                properties.shift_remove(*property);
            }
        }
        if let Some(description) = tool.get_mut("description") {
            if let Some((_, then)) = SINCE.iter().find(|(now, _)| description.as_str() == Some(*now)) {
                *description = Value::from(*then);
            }
        }
    }
}
