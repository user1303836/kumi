//! The canonical Live operation registry (`protocol/ableton-live-v1.operations.json`): its hash,
//! which the Remote Script must match before Live connects, and validation of production wire
//! values against the exact schema subset the registry uses.

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};

use kumi_common::js::{json, number as js_number, string as js_string};
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};

/// The wire method an operation is carried by.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LiveRegistryMethod {
    Status,
    Snapshot,
    Discover,
    Get,
    Preflight,
    Prepare,
    Invoke,
    Subscribe,
    Reconnect,
    Retire,
}

impl LiveRegistryMethod {
    pub const ALL: [LiveRegistryMethod; 10] = [
        LiveRegistryMethod::Status,
        LiveRegistryMethod::Snapshot,
        LiveRegistryMethod::Discover,
        LiveRegistryMethod::Get,
        LiveRegistryMethod::Preflight,
        LiveRegistryMethod::Prepare,
        LiveRegistryMethod::Invoke,
        LiveRegistryMethod::Subscribe,
        LiveRegistryMethod::Reconnect,
        LiveRegistryMethod::Retire,
    ];

    pub fn as_str(&self) -> &'static str {
        match self {
            LiveRegistryMethod::Status => "status",
            LiveRegistryMethod::Snapshot => "snapshot",
            LiveRegistryMethod::Discover => "discover",
            LiveRegistryMethod::Get => "get",
            LiveRegistryMethod::Preflight => "preflight",
            LiveRegistryMethod::Prepare => "prepare",
            LiveRegistryMethod::Invoke => "invoke",
            LiveRegistryMethod::Subscribe => "subscribe",
            LiveRegistryMethod::Reconnect => "reconnect",
            LiveRegistryMethod::Retire => "retire",
        }
    }

    pub fn parse(text: &str) -> Option<LiveRegistryMethod> {
        LiveRegistryMethod::ALL.iter().copied().find(|method| method.as_str() == text)
    }
}

impl std::fmt::Display for LiveRegistryMethod {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LiveRegistryOperation {
    pub id: String,
    pub method: LiveRegistryMethod,
    pub request: Map<String, Value>,
    pub result: Map<String, Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LiveRegistry {
    pub version: u64,
    pub protocol: String,
    pub operations: Vec<LiveRegistryOperation>,
}

impl LiveRegistry {
    /// The operation with `id`, if the registry holds it.
    pub fn operation(&self, id: &str) -> Option<&LiveRegistryOperation> {
        self.operations.iter().find(|item| item.id == id)
    }
}

/// What went wrong with the registry or a value checked against it; the text is the TypeScript's.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct RegistryError(pub String);

const SCHEMA_KEYS: [&str; 16] = [
    "type",
    "properties",
    "required",
    "additionalProperties",
    "items",
    "enum",
    "const",
    "minLength",
    "maxLength",
    "minimum",
    "maximum",
    "minItems",
    "maxItems",
    "uniqueItems",
    "maxProperties",
    "pattern",
];
const SCHEMA_TYPES: [&str; 7] = ["object", "array", "string", "number", "integer", "boolean", "null"];
const MAX_SAFE_INTEGER: f64 = 9_007_199_254_740_991.0;

/// Bounds on the canonical JSON of a value: the registry's own, the Live wire's, or none.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CanonicalLimits {
    pub max_depth: usize,
    /// In UTF-16 code units, as JavaScript measures a string.
    pub max_string_length: usize,
    pub max_array_length: usize,
    pub max_object_properties: usize,
}

/// The registry's canonical form: nesting bounded at 32.
pub const REGISTRY_CANONICAL_LIMITS: CanonicalLimits =
    CanonicalLimits { max_depth: 32, max_string_length: usize::MAX, max_array_length: usize::MAX, max_object_properties: usize::MAX };
/// The Remote Script's wire bounds (MAX_WIRE_DEPTH and the rest in ableton_mcp_remote_script.py): both
/// ends of the wire sign the same text.
pub const WIRE_CANONICAL_LIMITS: CanonicalLimits =
    CanonicalLimits { max_depth: 256, max_string_length: 1_048_576, max_array_length: 10_000_000, max_object_properties: 1_000_000 };
/// No bounds: the simulator's authority digests.
pub const UNBOUNDED_CANONICAL_LIMITS: CanonicalLimits = CanonicalLimits {
    max_depth: usize::MAX,
    max_string_length: usize::MAX,
    max_array_length: usize::MAX,
    max_object_properties: usize::MAX,
};

/// Why a value has no canonical form within its limits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CanonicalError {
    TooDeep,
    StringTooLarge,
    ArrayTooLarge,
    ObjectTooLarge,
}

/// Canonical JSON: sorted keys (by UTF-16 code unit order, as `Object.keys(x).sort()` sorts them),
/// `JSON.stringify` for scalars, no spaces. `ableton-live-v1.operations.json` is hashed this way, and
/// so is every signed wire frame.
pub fn canonical_json(value: &Value, limits: &CanonicalLimits) -> Result<String, CanonicalError> {
    let mut out = String::new();
    canonical_into(value, limits, 0, &mut out)?;
    Ok(out)
}

fn canonical_into(value: &Value, limits: &CanonicalLimits, depth: usize, out: &mut String) -> Result<(), CanonicalError> {
    if depth > limits.max_depth {
        return Err(CanonicalError::TooDeep);
    }
    match value {
        Value::Null | Value::Bool(_) | Value::Number(_) => json::write_into(value, out),
        Value::String(text) => {
            // UTF-16 never takes more units than UTF-8 takes bytes: only a long text needs counting.
            if text.len() > limits.max_string_length && js_string::utf16_len(text) > limits.max_string_length {
                return Err(CanonicalError::StringTooLarge);
            }
            json::escape(text, out);
        }
        Value::Array(items) => {
            if items.len() > limits.max_array_length {
                return Err(CanonicalError::ArrayTooLarge);
            }
            out.push('[');
            for (index, item) in items.iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                canonical_into(item, limits, depth + 1, out)?;
            }
            out.push(']');
        }
        Value::Object(object) => {
            if object.len() > limits.max_object_properties {
                return Err(CanonicalError::ObjectTooLarge);
            }
            let mut keys: Vec<&String> = object.keys().collect();
            // JavaScript orders by UTF-16 code units, which is byte order for ASCII keys (nearly all).
            if keys.iter().all(|key| key.is_ascii()) {
                keys.sort_unstable();
            } else {
                keys.sort_by(|left, right| left.encode_utf16().cmp(right.encode_utf16()));
            }
            out.push('{');
            for (index, key) in keys.into_iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                json::escape(key, out);
                out.push(':');
                canonical_into(&object[key], limits, depth + 1, out)?;
            }
            out.push('}');
        }
    }
    Ok(())
}

/// The registry's `canonical()`: bounded at 32 levels, with its own error text.
fn canonical(value: &Value) -> Result<String, RegistryError> {
    canonical_json(value, &REGISTRY_CANONICAL_LIMITS).map_err(|_| RegistryError("registry is too deeply nested".into()))
}

/// SHA-256 hex of a registry file's canonical JSON, as its release manifest names it: one a retained release
/// carries, which needn't be this code's.
pub fn registry_text_hash(text: &str) -> Result<String, RegistryError> {
    let value: Value = serde_json::from_str(text).map_err(|error| RegistryError(error.to_string()))?;
    Ok(sha256_hex(&canonical(&value)?))
}

/// SHA-256 of a text, as lowercase hex.
pub fn sha256_hex(text: &str) -> String {
    hex::encode(Sha256::digest(text.as_bytes()))
}

/// The registry this code was built with: the repository's `protocol/ableton-live-v1.operations.json`,
/// embedded at build time. Never one found from the working directory: a bridge started from a checkout
/// of another version would take that one's registry and no longer agree with its own Remote Script in
/// Live.
pub const LIVE_REGISTRY_TEXT: &str = include_str!("../../../protocol/ableton-live-v1.operations.json");

static REGISTRY: LazyLock<LiveRegistry> =
    LazyLock::new(|| parse_live_registry(LIVE_REGISTRY_TEXT).unwrap_or_else(|error| panic!("{error}")));
static REGISTRY_HASH: LazyLock<String> =
    LazyLock::new(|| live_registry_hash_of(load_live_registry()).unwrap_or_else(|error| panic!("{error}")));
static REGISTRY_OPERATIONS: LazyLock<Vec<String>> =
    LazyLock::new(|| load_live_registry().operations.iter().map(|operation| operation.id.clone()).collect());

/// The registry, parsed and validated once.
pub fn load_live_registry() -> &'static LiveRegistry {
    &REGISTRY
}

/// JavaScript truthiness, where the TypeScript tested a parsed value with `!`.
fn truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(flag) => *flag,
        Value::Number(number) => number.as_f64().map(|n| n != 0.0 && !n.is_nan()).unwrap_or(false),
        Value::String(text) => !text.is_empty(),
        Value::Array(_) | Value::Object(_) => true,
    }
}

/// Parses and validates a registry's JSON text, as `loadLiveRegistry()` does.
pub fn parse_live_registry(text: &str) -> Result<LiveRegistry, RegistryError> {
    let parsed: Value = serde_json::from_str(text).map_err(|error| RegistryError(error.to_string()))?;
    let invalid = || RegistryError("invalid Live operation registry".into());
    let object = match &parsed {
        Value::Object(object) if truthy(&parsed) => object,
        _ => return Err(invalid()),
    };
    let version_ok = object.get("version").and_then(Value::as_f64) == Some(1.0);
    let protocol_ok = object.get("protocol").and_then(Value::as_str) == Some("ableton-live/v1");
    let operations = match object.get("operations") {
        Some(Value::Array(items)) if version_ok && protocol_ok && !items.is_empty() && items.len() <= 4096 => items,
        _ => return Err(invalid()),
    };
    let ids: Vec<Option<&str>> =
        operations.iter().map(|operation| operation.as_object().and_then(|row| row.get("id")).and_then(Value::as_str)).collect();
    let sorted_unique = {
        let mut ok = true;
        for (index, id) in ids.iter().enumerate() {
            let Some(id) = id else {
                ok = false;
                break;
            };
            let length = js_string::utf16_len(id);
            if length < 1 || length > 128 {
                ok = false;
                break;
            }
            if index > 0 {
                let previous = ids[index - 1].unwrap_or("");
                if previous.encode_utf16().cmp(id.encode_utf16()) != std::cmp::Ordering::Less {
                    ok = false;
                    break;
                }
            }
        }
        ok && ids.iter().collect::<std::collections::HashSet<_>>().len() == ids.len()
    };
    if !sorted_unique {
        return Err(RegistryError("registry operation identifiers must be unique and sorted".into()));
    }
    for operation in operations {
        let row = operation.as_object().filter(|_| truthy(operation));
        let method = row.and_then(|row| row.get("method")).and_then(Value::as_str).and_then(LiveRegistryMethod::parse);
        let request = row.and_then(|row| row.get("request")).filter(|value| truthy(value));
        let result = row.and_then(|row| row.get("result")).filter(|value| truthy(value));
        let (Some(_), Some(request), Some(result)) = (method, request, result) else {
            let id = row
                .and_then(|row| row.get("id"))
                .map(|id| match id {
                    Value::String(text) => text.clone(),
                    other => json::stringify(other),
                })
                .unwrap_or_else(|| "unknown".to_string());
            return Err(RegistryError(format!("invalid registry operation: {id}")));
        };
        validate_schema(request, 0)?;
        validate_schema(result, 0)?;
    }
    serde_json::from_value(parsed).map_err(|error| RegistryError(error.to_string()))
}

fn is_bound(value: &Value) -> bool {
    matches!(value.as_f64(), Some(n) if js_number::is_safe_integer(n) && (0.0..=MAX_SAFE_INTEGER).contains(&n))
}

fn is_limit(value: &Value) -> bool {
    matches!(value.as_f64(), Some(n) if n.is_finite() && (-MAX_SAFE_INTEGER..=MAX_SAFE_INTEGER).contains(&n))
}

fn validate_schema(schema: &Value, depth: usize) -> Result<(), RegistryError> {
    let err = |text: &str| Err(RegistryError(text.to_string()));
    let value = match schema {
        Value::Object(value) if depth <= 8 => value,
        _ => return err("invalid registry schema"),
    };
    if value.keys().any(|key| !SCHEMA_KEYS.contains(&key.as_str())) {
        return err("unknown registry schema keyword");
    }
    let types: Vec<&Value> = match value.get("type") {
        None => Vec::new(),
        Some(Value::Array(items)) => items.iter().collect(),
        Some(single) => vec![single],
    };
    if types.is_empty()
        || types.len() > 4
        || types.iter().any(|item| !matches!(item, Value::String(text) if SCHEMA_TYPES.contains(&text.as_str())))
    {
        return err("registry schema type is invalid");
    }
    let type_names: Vec<&str> = types.iter().filter_map(|item| item.as_str()).collect();
    for key in ["minLength", "maxLength", "minItems", "maxItems", "maxProperties"] {
        if let Some(bound) = value.get(key) {
            if !is_bound(bound) {
                return err("registry schema bound is invalid");
            }
        }
    }
    for key in ["minimum", "maximum"] {
        if let Some(limit) = value.get(key) {
            if !is_limit(limit) {
                return err("registry schema bound is invalid");
            }
        }
    }
    if (value.contains_key("minItems") || value.contains_key("maxItems") || value.contains_key("uniqueItems"))
        && !type_names.contains(&"array")
    {
        return err("array constraint on non-array schema");
    }
    if let Some(unique) = value.get("uniqueItems") {
        if !unique.is_boolean() {
            return err("uniqueItems is invalid");
        }
    }
    if let (Some(min), Some(max)) = (value.get("minItems").and_then(Value::as_f64), value.get("maxItems").and_then(Value::as_f64)) {
        if min > max {
            return err("array bounds are invalid");
        }
    }
    if value.contains_key("maxProperties") && !type_names.contains(&"object") {
        return err("object bound on non-object schema");
    }
    if type_names.contains(&"object") {
        let additional = value.get("additionalProperties");
        if !matches!(additional, Some(Value::Bool(_)) | Some(Value::Object(_))) {
            return err("object schema must bound additional properties");
        }
        if additional != Some(&Value::Bool(false)) && !value.contains_key("maxProperties") {
            return err("additional properties must be bounded");
        }
        if let Some(child @ Value::Object(_)) = additional {
            validate_schema(child, depth + 1)?;
        }
        if let Some(properties) = value.get("properties") {
            match properties {
                Value::Object(map) if map.len() <= 64 => {}
                _ => return err("object properties are invalid"),
            }
        }
        if let Some(required) = value.get("required") {
            match required {
                Value::Array(items) if items.len() <= 64 && items.iter().all(Value::is_string) => {}
                _ => return err("required fields are invalid"),
            }
        }
        if let Some(Value::Object(properties)) = value.get("properties") {
            for child in properties.values() {
                validate_schema(child, depth + 1)?;
            }
        }
    }
    if type_names.contains(&"array") {
        match value.get("items") {
            Some(items) if truthy(items) && (items.is_object() || items.is_array()) => validate_schema(items, depth + 1)?,
            _ => return err("array items are required"),
        }
    }
    if let Some(items) = value.get("enum") {
        match items {
            Value::Array(items) if !items.is_empty() && items.len() <= 32 => {}
            _ => return err("enum is invalid"),
        }
    }
    if let Some(constant) = value.get("const") {
        // `typeof null` is "object" too.
        if constant.is_object() || constant.is_array() || constant.is_null() {
            return err("const is invalid");
        }
    }
    Ok(())
}

/// SHA-256 hex of the registry's canonical JSON.
pub fn live_registry_hash_of(registry: &LiveRegistry) -> Result<String, RegistryError> {
    let value = serde_json::to_value(registry).map_err(|error| RegistryError(error.to_string()))?;
    Ok(sha256_hex(&canonical(&value)?))
}

/// SHA-256 hex of the embedded registry's canonical JSON, computed once.
pub fn live_registry_hash() -> &'static str {
    REGISTRY_HASH.as_str()
}

/// The embedded registry's operation ids, in its (sorted) order.
pub fn live_registry_operations() -> &'static [String] {
    REGISTRY_OPERATIONS.as_slice()
}

/// `liveRegistryOperations(registry)` for a registry other than the embedded one.
pub fn live_registry_operations_of(registry: &LiveRegistry) -> Vec<String> {
    registry.operations.iter().map(|operation| operation.id.clone()).collect()
}

fn matches_type(value: &Value, kind: &str) -> bool {
    match kind {
        "null" => value.is_null(),
        "object" => value.is_object(),
        "array" => value.is_array(),
        "integer" => matches!(value.as_f64(), Some(n) if js_number::is_safe_integer(n)),
        "number" => matches!(value.as_f64(), Some(n) if n.is_finite()),
        "boolean" => value.is_boolean(),
        "string" => value.is_string(),
        _ => false,
    }
}

/// JavaScript's `===` between a wire value and a registry constant (primitives only).
fn strictly_equal(left: &Value, right: &Value) -> bool {
    match (left, right) {
        (Value::Null, Value::Null) => true,
        (Value::Bool(a), Value::Bool(b)) => a == b,
        (Value::Number(a), Value::Number(b)) => a.as_f64() == b.as_f64(),
        (Value::String(a), Value::String(b)) => a == b,
        _ => false,
    }
}

static PATTERNS: LazyLock<Mutex<HashMap<String, Regex>>> = LazyLock::new(|| Mutex::new(HashMap::new()));

/// `new RegExp(pattern).test(value)`; the registry's patterns are plain character classes and groups.
fn pattern_matches(pattern: &str, value: &str) -> Result<bool, RegistryError> {
    let mut cache = PATTERNS.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    if !cache.contains_key(pattern) {
        let compiled = Regex::new(pattern).map_err(|error| RegistryError(error.to_string()))?;
        cache.insert(pattern.to_string(), compiled);
    }
    Ok(cache[pattern].is_match(value))
}

/// Validate production wire values against the exact canonical registry subset.
pub fn validate_registry_value(schema: &Map<String, Value>, value: &Value, path: &str) -> Result<(), RegistryError> {
    let fail = |text: String| Err(RegistryError(text));
    let declared: Vec<&Value> = match schema.get("type") {
        Some(Value::Array(items)) => items.iter().collect(),
        Some(single) => vec![single],
        None => vec![&Value::Null],
    };
    if !declared.iter().any(|kind| kind.as_str().map(|kind| matches_type(value, kind)).unwrap_or(false)) {
        return fail(format!("{path} does not match registry type"));
    }
    if let Some(constant) = schema.get("const") {
        if !strictly_equal(value, constant) {
            return fail(format!("{path} does not match registry constant"));
        }
    }
    if let Some(Value::Array(items)) = schema.get("enum") {
        if !items.iter().any(|item| strictly_equal(item, value)) {
            return fail(format!("{path} is outside registry enum"));
        }
    }
    if let Value::String(text) = value {
        let length = js_string::utf16_len(text) as f64;
        if let Some(min) = schema.get("minLength").and_then(Value::as_f64) {
            if length < min {
                return fail(format!("{path} is shorter than registry minimum"));
            }
        }
        if let Some(max) = schema.get("maxLength").and_then(Value::as_f64) {
            if length > max {
                return fail(format!("{path} exceeds registry maximum"));
            }
        }
        if let Some(pattern) = schema.get("pattern").and_then(Value::as_str) {
            if !pattern_matches(pattern, text)? {
                return fail(format!("{path} does not match registry pattern"));
            }
        }
    }
    if let Some(number) = value.as_f64() {
        let below = schema.get("minimum").and_then(Value::as_f64).map(|min| number < min).unwrap_or(false);
        let above = schema.get("maximum").and_then(Value::as_f64).map(|max| number > max).unwrap_or(false);
        if !number.is_finite() || below || above {
            return fail(format!("{path} is outside registry numeric bounds"));
        }
    }
    if let Value::Array(items) = value {
        let count = items.len() as f64;
        if let Some(min) = schema.get("minItems").and_then(Value::as_f64) {
            if count < min {
                return fail(format!("{path} is below registry item bound"));
            }
        }
        if let Some(max) = schema.get("maxItems").and_then(Value::as_f64) {
            if count > max {
                return fail(format!("{path} exceeds registry item bound"));
            }
        }
        if schema.get("uniqueItems") == Some(&Value::Bool(true)) {
            let mut seen = std::collections::HashSet::new();
            for item in items {
                seen.insert(canonical(item)?);
            }
            if seen.len() != items.len() {
                return fail(format!("{path} contains duplicate registry items"));
            }
        }
        let item_schema = schema.get("items").and_then(Value::as_object).cloned().unwrap_or_default();
        for (index, item) in items.iter().enumerate() {
            validate_registry_value(&item_schema, item, &format!("{path}[{index}]"))?;
        }
    }
    if let Value::Object(object) = value {
        if let Some(max) = schema.get("maxProperties").and_then(Value::as_f64) {
            if object.len() as f64 > max {
                return fail(format!("{path} exceeds registry property bound"));
            }
        }
        let empty = Map::new();
        let properties = schema.get("properties").and_then(Value::as_object).unwrap_or(&empty);
        if let Some(Value::Array(required)) = schema.get("required") {
            for name in required.iter().filter_map(Value::as_str) {
                if !object.contains_key(name) {
                    return fail(format!("{path}.{name} is required by registry"));
                }
            }
        }
        match schema.get("additionalProperties") {
            Some(Value::Bool(false)) => {
                for key in object.keys() {
                    if !properties.contains_key(key) {
                        return fail(format!("{path}.{key} is not allowed by registry"));
                    }
                }
            }
            Some(Value::Object(additional)) => {
                for (key, child) in object {
                    if !properties.contains_key(key) {
                        validate_registry_value(additional, child, &format!("{path}.{key}"))?;
                    }
                }
            }
            _ => {}
        }
        for (key, child) in properties {
            if let (Some(value), Some(child)) = (object.get(key), child.as_object()) {
                validate_registry_value(child, value, &format!("{path}.{key}"))?;
            }
        }
    }
    Ok(())
}

/// `validateLiveOperationRequest` against a given registry.
pub fn validate_live_operation_request_in(registry: &LiveRegistry, operation_id: &str, value: &Value) -> Result<(), RegistryError> {
    let operation =
        registry.operation(operation_id).ok_or_else(|| RegistryError(format!("operation is not in canonical registry: {operation_id}")))?;
    validate_registry_value(&operation.request, value, &format!("{operation_id}.request"))
}

/// `validateLiveOperationResult` against a given registry.
pub fn validate_live_operation_result_in(registry: &LiveRegistry, operation_id: &str, value: &Value) -> Result<(), RegistryError> {
    let operation =
        registry.operation(operation_id).ok_or_else(|| RegistryError(format!("operation is not in canonical registry: {operation_id}")))?;
    validate_registry_value(&operation.result, value, &format!("{operation_id}.result"))
}

/// A request's arguments against the embedded registry.
pub fn validate_live_operation_request(operation_id: &str, value: &Value) -> Result<(), RegistryError> {
    validate_live_operation_request_in(load_live_registry(), operation_id, value)
}

/// An operation's result against the embedded registry.
pub fn validate_live_operation_result(operation_id: &str, value: &Value) -> Result<(), RegistryError> {
    validate_live_operation_result_in(load_live_registry(), operation_id, value)
}
