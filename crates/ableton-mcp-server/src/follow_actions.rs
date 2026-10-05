//! The Follow Action fields and their bounds, as the registry's `clip.follow-actions.set` declares them.

use std::sync::LazyLock;

use serde_json::{Map, Value};

use crate::registry::load_live_registry;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FollowSchemaType {
    Boolean,
    Integer,
    Number,
}

/// One field's schema: `{ type: "boolean" | "integer" | "number"; minimum?: number; maximum?: number }`.
#[derive(Debug, Clone, PartialEq)]
pub struct FollowSchema {
    pub kind: FollowSchemaType,
    pub minimum: Option<f64>,
    pub maximum: Option<f64>,
}

/// The `followAction*` properties of `clip.follow-actions.set`'s request, in registry order.
pub static FOLLOW_ACTION_SCHEMA: LazyLock<Vec<(String, FollowSchema)>> = LazyLock::new(|| {
    let request = &load_live_registry().operation("clip.follow-actions.set").expect("clip.follow-actions.set is in the registry").request;
    let properties = request.get("properties").and_then(Value::as_object).cloned().unwrap_or_default();
    properties
        .into_iter()
        .filter(|(field, _)| field.starts_with("followAction"))
        .map(|(field, schema)| {
            let kind = match schema.get("type").and_then(Value::as_str) {
                Some("boolean") => FollowSchemaType::Boolean,
                Some("integer") => FollowSchemaType::Integer,
                _ => FollowSchemaType::Number,
            };
            let minimum = schema.get("minimum").and_then(Value::as_f64);
            let maximum = schema.get("maximum").and_then(Value::as_f64);
            (field, FollowSchema { kind, minimum, maximum })
        })
        .collect()
});

/// The field names, in registry order.
pub static FOLLOW_ACTION_FIELDS: LazyLock<Vec<String>> =
    LazyLock::new(|| FOLLOW_ACTION_SCHEMA.iter().map(|(field, _)| field.clone()).collect());

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct FollowActionError(pub String);

/// Every field present, typed and within its bounds, and the two chances summing to 100.
pub fn validate_follow_actions(state: &Map<String, Value>) -> Result<(), FollowActionError> {
    for (field, schema) in FOLLOW_ACTION_SCHEMA.iter() {
        let value = state.get(field);
        if schema.kind == FollowSchemaType::Boolean {
            if !matches!(value, Some(Value::Bool(_))) {
                return Err(FollowActionError(format!("{field} must be boolean")));
            }
            continue;
        }
        let number = value.and_then(Value::as_f64);
        let valid = match number {
            Some(n) => {
                n.is_finite()
                    && schema.minimum.map(|min| n >= min).unwrap_or(true)
                    && schema.maximum.map(|max| n <= max).unwrap_or(true)
                    && (schema.kind != FollowSchemaType::Integer || n.fract() == 0.0)
            }
            None => false,
        };
        if !valid {
            return Err(FollowActionError(format!("{field} is unavailable or out of bounds")));
        }
    }
    let chance_a = state.get("followActionChanceA").and_then(Value::as_f64).unwrap_or(f64::NAN);
    let chance_b = state.get("followActionChanceB").and_then(Value::as_f64).unwrap_or(f64::NAN);
    if chance_a + chance_b != 100.0 {
        return Err(FollowActionError("Follow Action probabilities must sum to 100".into()));
    }
    Ok(())
}
