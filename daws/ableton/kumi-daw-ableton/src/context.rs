//! Validate the bounded Live observations the integration gives the model.
use kumi_agent::mcp::types::{CallToolResult, ContentBlock};
use kumi_common::js::{json::stringify, string::utf16_len};
use kumi_types::contracts::JsonObject;
use serde_json::{json, Value};
use std::sync::LazyLock;
pub const INSTRUCTIONS: &str = include_str!("assets/instructions.txt");
pub static FIELDS: LazyLock<JsonObject> = LazyLock::new(|| serde_json::from_str(include_str!("assets/fields.json")).unwrap());
pub static PARENTS: LazyLock<JsonObject> = LazyLock::new(|| serde_json::from_str(include_str!("assets/parents.json")).unwrap());
pub use kumi_agent::core::refs::{query_key, ObservationError};
fn error(message: &str) -> ObservationError {
    ObservationError(message.into())
}
pub fn object(value: &Value) -> Result<JsonObject, ObservationError> {
    value.as_object().cloned().ok_or_else(|| error("Malformed Live observation; refresh before continuing"))
}
pub fn payload(result: &CallToolResult) -> Result<JsonObject, ObservationError> {
    if result.is_error == Some(true) {
        return Err(error("Live read returned an error; no old observation is current"));
    }
    if let Some(value) = &result.structured_content {
        return Ok(value.clone());
    }
    let text = result
        .content
        .iter()
        .filter_map(|block| if let ContentBlock::Text { text, .. } = block { Some(text.as_str()) } else { None })
        .collect::<Vec<_>>()
        .join("\n");
    object(&serde_json::from_str(&text).map_err(|_| error("Malformed Live observation; refresh before continuing"))?)
}
pub fn status_payload(result: &CallToolResult) -> Result<JsonObject, ObservationError> {
    let value = payload(result)?;
    let valid_epoch = value.get("epoch").and_then(Value::as_f64).is_some_and(|n| n >= 0.0 && n <= 9007199254740991.0 && n.fract() == 0.0);
    if !value.get("connected").is_some_and(Value::is_boolean)
        || !value.get("adapter").is_some_and(Value::is_string)
        || (value.get("connected") == Some(&Value::Bool(true)) && !valid_epoch)
    {
        return Err(error("Malformed Live status; access cannot be verified"));
    }
    Ok(value)
}
pub fn discovery_payload(result: &CallToolResult, kind: &str, epoch: f64) -> Result<JsonObject, ObservationError> {
    let value = payload(result)?;
    if value.get("epoch").and_then(Value::as_f64) != Some(epoch) {
        return Err(error("Live epoch changed; result discarded, refresh before continuing"));
    }
    if value.get("kind").and_then(Value::as_str) != Some(kind)
        || !value.get("items").is_some_and(Value::is_array)
        || !value.get("truncated").is_some_and(Value::is_boolean)
        || !value.get("revision").is_some_and(Value::is_string)
        || value.get("nextCursor").is_some_and(|cursor| !cursor.as_str().is_some_and(|s| !s.is_empty() && utf16_len(s) <= 1024))
    {
        return Err(error("Malformed discovery result"));
    }
    for row in value["items"].as_array().unwrap() {
        object(row)?;
    }
    Ok(value)
}
pub fn set_identity(row: &JsonObject) -> Result<String, ObservationError> {
    let reference = row
        .get("ref")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty() && utf16_len(s) <= 256)
        .ok_or_else(|| error("Current Set reference is unavailable"))?;
    Ok(stringify(&json!([reference, row.get("objectIdentity").and_then(Value::as_str)])))
}
pub fn discovery_args(input: &JsonObject) -> Result<JsonObject, ObservationError> {
    let kind = input
        .get("kind")
        .and_then(Value::as_str)
        .filter(|kind| FIELDS.contains_key(*kind))
        .ok_or_else(|| error("Unsupported discovery kind"))?;
    let mut args = json!({"limit":25,"budget":1000,"fields":FIELDS[kind]}).as_object().unwrap().clone();
    args.extend(input.clone());
    if let Some(fields) = args.get("fields").and_then(Value::as_array) {
        let mut needed = vec!["ref"];
        if input.contains_key("parent") {
            needed.push("parentRef");
        }
        if kind == "set" {
            needed.push("objectIdentity");
        }
        if kind.ends_with("track") {
            needed.extend(["name", "color"]);
        }
        if kind == "clip-slot" {
            needed.push("clipRef");
        }
        for field in fields.iter().filter_map(Value::as_str) {
            if !needed.contains(&field) {
                needed.push(field);
            }
        }
        let fields = needed.into_iter().map(|s| Value::String(s.into())).collect();
        args.insert("fields".into(), Value::Array(fields));
    }
    Ok(args)
}
