//! Oracle comparisons with each change record's picture, and the plan's run it's part of, set aside. Both came after
//! the port, and have tests of their own (`changes/pictures.rs`, the TUI's HISTORY runs): only those keys are dropped,
//! and only on a change record (an object with an id, a family and a state). Everything else compares exactly.
use serde_json::Value;

pub fn without_change_pictures(value: &Value) -> Value {
    match value {
        Value::Object(map) => {
            let record = ["id", "family", "state"].iter().all(|key| map.contains_key(*key));
            Value::Object(
                map.iter()
                    .filter(|(key, _)| !(record && matches!(key.as_str(), "picture" | "batch")))
                    .map(|(key, value)| (key.clone(), without_change_pictures(value)))
                    .collect(),
            )
        }
        Value::Array(items) => Value::Array(items.iter().map(without_change_pictures).collect()),
        other => other.clone(),
    }
}
