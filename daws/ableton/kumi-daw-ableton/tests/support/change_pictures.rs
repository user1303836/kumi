//! Oracle comparisons with each change record's picture set aside. Pictures came after the port, and
//! `changes/pictures.rs` tests them: only that key is dropped, and only on a change record (an object with an id, a
//! family and a state). Everything else compares exactly.
use serde_json::Value;

pub fn without_change_pictures(value: &Value) -> Value {
    match value {
        Value::Object(map) => {
            let record = ["id", "family", "state"].iter().all(|key| map.contains_key(*key));
            Value::Object(
                map.iter()
                    .filter(|(key, _)| !(record && key.as_str() == "picture"))
                    .map(|(key, value)| (key.clone(), without_change_pictures(value)))
                    .collect(),
            )
        }
        Value::Array(items) => Value::Array(items.iter().map(without_change_pictures).collect()),
        other => other.clone(),
    }
}
