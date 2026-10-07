//! The host's validation, mutation authority and client-facing result helpers.
use crate::{
    live::LiveError,
    registry::{canonical_json, CanonicalError, CanonicalLimits},
};
use kumi_common::js::{json as js_json, number as js_number, string as js_string};
use regex::Regex;
use serde_json::{json, Map, Value};
use std::sync::LazyLock;

pub const REQUEST_ID_MAX_LENGTH: usize = 128;
/// String(value) for JSON input, including array joining and a shadowed Object.toString refusal.
pub fn js_string(value: &Value) -> Result<String, LiveError> {
    Ok(match value {
        Value::String(text) => text.clone(),
        Value::Object(fields) => {
            if fields.contains_key("toString") {
                return Err(LiveError::type_error("Cannot convert object to primitive value"));
            }
            "[object Object]".into()
        }
        Value::Array(items) => items
            .iter()
            .map(|item| if item.is_null() { Ok(String::new()) } else { js_string(item) })
            .collect::<Result<Vec<_>, _>>()?
            .join(","),
        _ => js_json::stringify(value),
    })
}
pub const MUTATION_CANONICAL_LIMITS: CanonicalLimits =
    CanonicalLimits { max_depth: 256, max_string_length: 1_048_576, max_array_length: 10_000_000, max_object_properties: 10_000_000 };
pub fn has_only(value: &Value, allowed: &[&str]) -> bool {
    value.as_object().is_some_and(|fields| fields.keys().all(|key| allowed.contains(&key.as_str())))
}
pub fn is_non_empty_string(value: &Value, max_length: usize) -> bool {
    value.as_str().is_some_and(|text| (1..=max_length).contains(&js_string::utf16_len(text)))
}
pub fn is_finite_at_least(value: &Value, minimum: f64) -> bool {
    value.as_f64().is_some_and(|n| n.is_finite() && n >= minimum)
}
pub fn is_integer_in_range(value: &Value, minimum: f64, maximum: f64) -> bool {
    value.as_f64().is_some_and(|n| n.is_finite() && n.fract() == 0.0 && n >= minimum && n <= maximum)
}
pub fn is_discovery_filter(value: &Value) -> bool {
    value.as_object().is_some_and(|fields| {
        fields.len() <= 8
            && fields.iter().all(|(key, item)| {
                (1..=64).contains(&js_string::utf16_len(key))
                    && match item {
                        Value::Null | Value::Bool(_) => true,
                        Value::String(text) => js_string::utf16_len(text) <= 256,
                        Value::Number(n) => n.as_f64().is_some_and(|n| n.is_finite() && n.abs() <= 9_007_199_254_740_991.0),
                        _ => false,
                    }
            })
    })
}
pub fn is_idempotency_key(value: &Value) -> bool {
    value.as_str().is_some_and(|text| (8..=128).contains(&js_string::utf16_len(text)))
}
pub fn is_id(value: &Value) -> bool {
    is_non_empty_string(value, REQUEST_ID_MAX_LENGTH) || value.as_f64().is_some_and(js_number::is_safe_integer)
}
pub fn request_id(value: Option<&Value>) -> Value {
    value.filter(|value| is_id(value)).cloned().unwrap_or(Value::Null)
}
pub fn utility_params(value: Option<&Value>) -> bool {
    value.is_none_or(|value| value.as_object().is_some_and(Map::is_empty))
}
pub fn canonical_mutation_identity(value: &Value) -> Result<String, LiveError> {
    canonical_json(value, &MUTATION_CANONICAL_LIMITS).map_err(|cause| {
        LiveError::error(match cause {
            CanonicalError::TooDeep => "mutation authority is too deeply nested",
            CanonicalError::StringTooLarge => "mutation authority string is too large",
            CanonicalError::ArrayTooLarge => "mutation authority array is too large",
            CanonicalError::ObjectTooLarge => "mutation authority object is too large",
        })
    })
}
pub fn response(id: &Value, result: Value) -> Value {
    json!({"jsonrpc":"2.0", "id": id, "result": result})
}
pub fn error(id: &Value, code: i64, message: &str, data: Option<Value>) -> Value {
    let mut body = json!({"code":code,"message":message});
    if let Some(data) = data {
        body["data"] = data;
    }
    json!({"jsonrpc":"2.0", "id":id, "error":body})
}
pub fn text_content(text: &str) -> Value {
    json!({"type":"text","text":text})
}
pub fn success_text(id: &Value, value: &Value) -> Value {
    response(id, json!({"content":[text_content(&js_json::stringify(value))],"isError":false}))
}
pub fn reason_error(id: &Value, reason: &str, remediation: &str) -> Value {
    response(id, json!({"content":[text_content(&js_json::stringify(&json!({"reason":reason,"remediation":remediation})))],"isError":true}))
}
pub fn transaction_error(id: &Value, message: &str) -> Value {
    reason_error(id, message, "Preview the change again, then apply the new transaction.")
}
pub fn recovery_finalize_error(id: &Value, reason: &str) -> Value {
    reason_error(id, reason, "Reconcile or manually recover the exact transaction, prove all audible work stopped, then submit the explicit finalization evidence.")
}
pub fn adapter_tool_error(id: &Value, cause: &LiveError, remediation: &str) -> Value {
    let next = if remediation.ends_with("preview requires fresh authoritative state.") {
        "Nothing changed in Live: fix what the reason says (or take another route) and preview again."
    } else {
        remediation
    };
    reason_error(id, &adapter_reason(cause.message()), next)
}
pub fn adapter_reason(raw: &str) -> String {
    static UNSAFE: LazyLock<Regex> = LazyLock::new(|| {
        // JavaScript's \s omits U+0085 but includes U+FEFF.
        let space = r"\t\n\v\f\r \u{00a0}\u{1680}\u{2000}-\u{200a}\u{2028}\u{2029}\u{202f}\u{205f}\u{3000}\u{feff}";
        Regex::new(&format!(
            r#"[\r\n]|(?-u:\b)at [^{space}]+ \(|node:internal|(?:^|[{space}'"(=])(?:/[^/{space}'"\x00]+){{2,}}|[A-Za-z]:\\|\\\\[^{space}]+"#
        ))
        .expect("adapter redaction pattern")
    });
    let line = js_string::trim(raw);
    if line.is_empty() || UNSAFE.is_match(line) {
        return "adapter request failed".into();
    }
    if js_string::utf16_len(line) <= 400 {
        line.into()
    } else {
        format!("{}…", js_string::slice(line, 0, Some(399)))
    }
}
pub fn output_safety_of(value: &Value) -> Value {
    if has_only(value, &["safe", "provenance", "observedAt", "scope"])
        && value["safe"] == true
        && is_non_empty_string(&value["provenance"], 512)
        && value["provenance"] != "unknown"
        && value["provenance"] != "simulator"
    {
        value.clone()
    } else {
        json!({"safe":true,"provenance":"requested","scope":"output"})
    }
}
pub fn nothing_changed(cause: &LiveError) -> bool {
    static PATTERN: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"^request failed: [^\n\r\u{2028}\u{2029}]*; nothing changed(?-u:\b)").unwrap());
    PATTERN.is_match(cause.message())
}
pub fn whole_number_live_kept(observed: &Value, proposed: f64, parameter: &Value) -> Option<f64> {
    let observed = observed.as_f64()?;
    let min = parameter["min"].as_f64()?;
    let max = parameter["max"].as_f64()?;
    (observed.is_finite()
        && observed.fract() == 0.0
        && min.is_finite()
        && min.fract() == 0.0
        && max.is_finite()
        && max.fract() == 0.0
        && (observed - proposed).abs() <= 0.5 + 1e-9)
        .then_some(observed)
}
pub fn same_live_value(observed: Option<&Value>, expected: Option<&Value>) -> bool {
    match (observed, expected) {
        (Some(Value::Number(a)), Some(Value::Number(b))) => {
            let (a, b) = (a.as_f64().unwrap(), b.as_f64().unwrap());
            (a - b).abs() <= 1e-6 * 1.0_f64.max(a.abs()).max(b.abs())
        }
        (Some(Value::Array(a)), Some(Value::Array(b))) => {
            a.len() == b.len() && a.iter().zip(b).all(|(a, b)| same_live_value(Some(a), Some(b)))
        }
        (a, b) => a.map(js_json::stringify) == b.map(js_json::stringify),
    }
}
/// A mixer field as Live holds it, compared as `same_live_value` does, but a `sends` list only over the sends it
/// names: Live sets a shorter list's first sends and leaves the others as they were.
pub fn same_mixer_value(field: &str, observed: Option<&Value>, expected: Option<&Value>) -> bool {
    match (field, observed, expected) {
        ("sends", Some(Value::Array(observed)), Some(Value::Array(expected))) => {
            observed.len() >= expected.len() && observed.iter().zip(expected).all(|(a, b)| same_live_value(Some(a), Some(b)))
        }
        _ => same_live_value(observed, expected),
    }
}
/// The part of a mixer field's value a change named: for `sends`, the first as many sends as `named` lists.
pub fn named_mixer_part(field: &str, value: &Value, named: &Value) -> Value {
    match (field, value.as_array(), named.as_array()) {
        ("sends", Some(value), Some(named)) => Value::Array(value.iter().take(named.len()).cloned().collect()),
        _ => value.clone(),
    }
}
pub fn same_follow_action_value(field: &str, observed: Option<&Value>, expected: Option<&Value>) -> bool {
    if field == "followActionTime" {
        return same_live_value(observed, expected);
    }
    match (observed, expected) {
        (Some(Value::Number(a)), Some(Value::Number(b))) => a.as_f64() == b.as_f64(),
        (Some(a @ (Value::Object(_) | Value::Array(_))), Some(b)) => std::ptr::eq(a, b),
        (a, b) => a == b,
    }
}
pub fn scene_restore_fields(prior: &Map<String, Value>) -> Option<Map<String, Value>> {
    let within = |value: &Value, min: f64, max: f64| value.as_f64().is_some_and(|n| n.is_finite() && n >= min && n <= max);
    let mut restore = Map::new();
    for (field, value) in prior {
        if value.is_null() {
            return None;
        }
        if field == "tempo" && !within(value, 20.0, 999.0) {
            if prior.get("tempoEnabled") == Some(&Value::Bool(true)) {
                return None;
            }
            restore.insert("tempoEnabled".into(), Value::Bool(false));
        } else if ["signatureNumerator", "signatureDenominator"].contains(&field.as_str()) && !is_integer_in_range(value, 1.0, 99.0) {
            if prior.get("timeSignatureEnabled") == Some(&Value::Bool(true)) {
                return None;
            }
            restore.insert("timeSignatureEnabled".into(), Value::Bool(false));
        } else {
            restore.insert(field.clone(), value.clone());
        }
    }
    Some(restore)
}
pub fn scene_field_restored(restored: &Value, field: &str, value: &Value, prior: &Value) -> bool {
    let off = (field == "tempo" && value.as_f64().is_none_or(|n| n < 20.0))
        || (["signatureNumerator", "signatureDenominator"].contains(&field) && value.as_f64().is_none_or(|n| n < 1.0));
    if off {
        let key = if field == "tempo" { "tempoEnabled" } else { "timeSignatureEnabled" };
        restored.get(key) == Some(&Value::Bool(false)) && prior.get(key) != Some(&Value::Bool(true))
    } else {
        same_live_value(restored.get(field), Some(value))
    }
}
pub fn arrangement_clip_end(clip: &Value) -> f64 {
    clip["endTime"]
        .as_f64()
        .filter(|n| n.is_finite())
        .unwrap_or_else(|| clip["start"].as_f64().unwrap_or(f64::NAN) + clip["length"].as_f64().unwrap_or(f64::NAN))
}
pub fn fit_parameter_value(value: f64, parameter: &Value) -> f64 {
    let min = parameter["min"].as_f64().unwrap_or(f64::NAN);
    let max = parameter["max"].as_f64().unwrap_or(f64::NAN);
    let held = if min.is_nan() || max.is_nan() || value.is_nan() { f64::NAN } else { max.min(min.max(value)) };
    let step = parameter["quantization"].as_f64().unwrap_or(0.0);
    if step > 0.0 {
        max.min(min + js_number::round((held - min) / step) * step)
    } else {
        held
    }
}
pub fn valid_transaction_params(params: &Value, confirmation: &str) -> bool {
    has_only(
        params,
        if confirmation == "undo" {
            &["transactionId", "confirmation", "idempotencyKey", "discard"]
        } else {
            &["transactionId", "confirmation", "idempotencyKey"]
        },
    ) && params.get("discard").is_none_or(Value::is_boolean)
        && is_non_empty_string(&params["transactionId"], 128)
        && params["confirmation"] == confirmation
        && is_idempotency_key(&params["idempotencyKey"])
}
