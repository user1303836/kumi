//! MCP wire semantics only: never creates Live authority or transaction state.

use std::sync::LazyLock;

use kumi_common::js::{number, string};
use regex::Regex;
use serde_json::{json, Map, Value};

pub const LEGACY_PROTOCOL_VERSION: &str = "2025-11-25";
pub const MODERN_PROTOCOL_VERSION: &str = "2026-07-28";
pub const SUPPORTED_PROTOCOL_VERSIONS: [&str; 2] = [MODERN_PROTOCOL_VERSION, LEGACY_PROTOCOL_VERSION];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProtocolEra {
    Legacy,
    Modern,
}

const PREFIX: &str = "io.modelcontextprotocol/";
const VERSION_KEY: &str = "io.modelcontextprotocol/protocolVersion";
const CAPABILITIES_KEY: &str = "io.modelcontextprotocol/clientCapabilities";
const IDENTITY_KEY: &str = "io.modelcontextprotocol/clientInfo";
const LOG_LEVEL_KEY: &str = "io.modelcontextprotocol/logLevel";

static META_KEY: LazyLock<Regex> = LazyLock::new(|| {
    let label = "[A-Za-z](?:[A-Za-z0-9-]*[A-Za-z0-9])?";
    Regex::new(&format!("^(?:(?:{label}\\.)*{label}/)?(?:[A-Za-z0-9](?:[A-Za-z0-9._-]*[A-Za-z0-9])?)?$")).expect("metadata key pattern")
});

const CACHEABLE: [&str; 6] =
    ["server/discover", "tools/list", "prompts/list", "resources/list", "resources/templates/list", "resources/read"];

/// Legacy push notifications have no modern subscriptions/listen binding yet.
/// Explicit observe/poll tools and authoritative snapshots remain available.
pub const MODERN_UNAVAILABLE_TOOLS: [&str; 2] = ["live_subscribe", "live_unsubscribe"];

/// `MODERN_UNAVAILABLE_TOOLS.has(name)`.
pub fn is_modern_unavailable_tool(name: &str) -> bool {
    MODERN_UNAVAILABLE_TOOLS.contains(&name)
}

/// What `prepareMcpRequest` returns: the input to dispatch (its `params` without `_meta` when
/// modern), whether the request is modern, and the error frame that ends it instead, if any.
#[derive(Debug, Clone, PartialEq)]
pub struct PreparedMcpRequest {
    pub input: Value,
    pub modern: bool,
    pub error: Option<Value>,
}

fn is_object(value: &Value) -> bool {
    value.is_object()
}

fn failure(input: &Map<String, Value>, code: i64, message: &str, data: Option<Value>) -> Value {
    let id = match input.get("id") {
        Some(Value::String(text)) if (1..=128).contains(&string::utf16_len(text)) => Value::String(text.clone()),
        Some(Value::Number(n)) if n.as_f64().is_some_and(number::is_safe_integer) => Value::Number(n.clone()),
        _ => Value::Null,
    };
    let mut error = json!({ "code": code, "message": message });
    if let Some(data) = data {
        error["data"] = data;
    }
    json!({ "jsonrpc": "2.0", "id": id, "error": error })
}

pub fn prepare_mcp_request(input: &Value, era: Option<ProtocolEra>) -> PreparedMcpRequest {
    let unchanged = |modern: bool| PreparedMcpRequest { input: input.clone(), modern, error: None };
    // The core retains envelope validation and notification handling. Metadata
    // must not turn malformed envelopes or notifications into method dispatches.
    let Some(envelope) = input.as_object() else { return unchanged(false) };
    let Some(method) = envelope.get("method").and_then(Value::as_str) else { return unchanged(false) };
    if envelope.get("jsonrpc").and_then(Value::as_str) != Some("2.0") || !envelope.contains_key("id") {
        return unchanged(false);
    }
    let params = envelope.get("params").and_then(Value::as_object);
    let meta = params.and_then(|params| params.get("_meta")).and_then(Value::as_object);
    let modern = era == Some(ProtocolEra::Modern)
        || method == "server/discover"
        || meta.is_some_and(|meta| [VERSION_KEY, CAPABILITIES_KEY, IDENTITY_KEY, LOG_LEVEL_KEY].iter().any(|key| meta.contains_key(*key)));
    if !modern {
        return unchanged(false);
    }
    let fail = |code: i64, message: &str, data: Option<Value>| PreparedMcpRequest {
        input: input.clone(),
        modern: true,
        error: Some(failure(envelope, code, message, data)),
    };
    let invalid = |message: &str| fail(-32602, message, None);
    if envelope.keys().any(|key| !["jsonrpc", "id", "method", "params"].contains(&key.as_str())) {
        return fail(-32600, "Invalid modern request envelope", None);
    }
    let (Some(meta), Some(version)) = (meta, meta.and_then(|meta| meta.get(VERSION_KEY)).and_then(Value::as_str)) else {
        return invalid("Modern requests require protocolVersion and clientCapabilities in params._meta");
    };
    let Some(capabilities) = meta.get(CAPABILITIES_KEY).and_then(Value::as_object) else {
        return invalid("Modern requests require protocolVersion and clientCapabilities in params._meta");
    };
    let version_length = string::utf16_len(version);
    if !(1..=64).contains(&version_length) {
        return invalid("Invalid protocol version metadata");
    }
    if version != MODERN_PROTOCOL_VERSION {
        if version == LEGACY_PROTOCOL_VERSION {
            return invalid("Legacy 2025-11-25 requires initialize on a legacy stdio process");
        }
        return fail(
            -32022,
            "Unsupported protocol version",
            Some(json!({ "requested": version, "supported": SUPPORTED_PROTOCOL_VERSIONS })),
        );
    }
    if meta.len() > 128 || meta.keys().any(|key| string::utf16_len(key) > 256 || !META_KEY.is_match(key)) {
        return invalid("Invalid or oversized request metadata");
    }
    if let Some(identity) = meta.get(IDENTITY_KEY) {
        let valid = identity.as_object().is_some_and(|identity| {
            identity.get("name").and_then(Value::as_str).is_some_and(|name| (1..=256).contains(&string::utf16_len(name)))
                && identity.get("version").and_then(Value::as_str).is_some_and(|version| (1..=64).contains(&string::utf16_len(version)))
        });
        if !valid {
            return invalid("Invalid clientInfo metadata");
        }
    }
    if capabilities.len() > 128
        || ["experimental", "roots", "sampling", "elicitation", "extensions"]
            .iter()
            .any(|key| capabilities.get(*key).is_some_and(|value| !is_object(value)))
    {
        return invalid("Invalid clientCapabilities metadata");
    }
    if let Some(experimental) = capabilities.get("experimental").and_then(Value::as_object) {
        if experimental.values().any(|value| !is_object(value)) {
            return invalid("Invalid experimental capabilities");
        }
    }
    for (name, fields) in [("sampling", ["context", "tools"]), ("elicitation", ["form", "url"])] {
        if let Some(capability) = capabilities.get(name).and_then(Value::as_object) {
            if fields.iter().any(|field| capability.get(*field).is_some_and(|value| !is_object(value))) {
                return invalid("Invalid client capability settings");
            }
        }
    }
    if let Some(extensions) = capabilities.get("extensions").and_then(Value::as_object) {
        if extensions.iter().any(|(key, value)| !key.contains('/') || !META_KEY.is_match(key) || !is_object(value)) {
            return invalid("Invalid extension capabilities");
        }
    }
    if let Some(token) = meta.get("progressToken") {
        let valid = match token {
            Value::String(_) => true,
            Value::Number(n) => n.as_f64().is_some_and(f64::is_finite),
            _ => false,
        };
        if !valid {
            return invalid("Invalid progress token");
        }
    }
    if let Some(level) = meta.get(LOG_LEVEL_KEY) {
        let levels = ["debug", "info", "notice", "warning", "error", "critical", "alert", "emergency"];
        if !level.as_str().is_some_and(|level| levels.contains(&level)) {
            return invalid("Invalid log level");
        }
    }
    if method == "initialize" {
        return invalid("Modern requests do not use initialize");
    }
    if era == Some(ProtocolEra::Legacy) && method != "server/discover" {
        return invalid("Do not mix protocol eras after legacy initialization; start a new stdio process");
    }
    let params = params.expect("metadata came from params");
    if method == "tools/call"
        && (!matches!(params.get("name"), Some(Value::String(_)))
            || params.keys().any(|key| !["name", "arguments", "_meta"].contains(&key.as_str()))
            || params.get("arguments").is_some_and(|arguments| !is_object(arguments)))
    {
        return invalid("Invalid tools/call parameters");
    }
    if method == "tools/call" && params.get("name").and_then(Value::as_str).is_some_and(is_modern_unavailable_tool) {
        return invalid("Legacy push subscriptions are unavailable in modern mode; use snapshot or observe/poll");
    }
    let mut arguments_object = params.clone();
    arguments_object.remove("_meta");
    // No client metadata is stored or treated as consent, policy, or capabilities
    // of Live itself. Each request has to supply its own protocol metadata.
    let mut prepared = envelope.clone();
    prepared.insert("params".to_string(), Value::Object(arguments_object));
    PreparedMcpRequest { input: Value::Object(prepared), modern, error: None }
}

pub fn format_mcp_response(frame: Option<Value>, input: &Value, modern: bool, server_info: &Value) -> Option<Value> {
    let frame = frame?;
    if !modern {
        return Some(frame);
    }
    let Some(frame_object) = frame.as_object() else { return Some(frame) };
    let method = input.as_object().and_then(|input| input.get("method")).and_then(Value::as_str);
    if let Some(error) = frame_object.get("error").and_then(Value::as_object) {
        let code = error.get("code").and_then(Value::as_f64);
        let obsolete = code == Some(-32002.0) || code == Some(-32042.0);
        let unknown_tool = method == Some("tools/call") && code == Some(-32601.0);
        if !(obsolete || unknown_tool) {
            return Some(frame);
        }
        let mut error = error.clone();
        error.insert("code".to_string(), json!(-32602));
        let mut formatted = frame_object.clone();
        formatted.insert("error".to_string(), Value::Object(error));
        return Some(Value::Object(formatted));
    }
    let Some(original) = frame_object.get("result").and_then(Value::as_object) else { return Some(frame) };
    let mut result = original.clone();
    result.insert("resultType".to_string(), json!("complete"));
    let mut meta = original.get("_meta").and_then(Value::as_object).cloned().unwrap_or_default();
    meta.insert(format!("{PREFIX}serverInfo"), server_info.clone());
    result.insert("_meta".to_string(), Value::Object(meta));
    if method.is_some_and(|method| CACHEABLE.contains(&method)) {
        result.insert("ttlMs".to_string(), json!(0));
        result.insert("cacheScope".to_string(), json!("private"));
    }
    if method == Some("tools/list") {
        if let Some(Value::Array(tools)) = result.get("tools") {
            let tools: Vec<Value> = tools
                .iter()
                .filter(|tool| {
                    tool.as_object().is_some_and(|tool| !tool.get("name").and_then(Value::as_str).is_some_and(is_modern_unavailable_tool))
                })
                .cloned()
                .collect();
            result.insert("tools".to_string(), Value::Array(tools));
        }
    }
    // Promote the existing redacted JSON text, after coalesced replay formatting,
    // so programmatic clients never need to scrape text or receive stale flags.
    if method == Some("tools/call") {
        let text = match result.get("content") {
            Some(Value::Array(content)) if content.len() == 1 => content[0]
                .as_object()
                .filter(|item| item.get("type").and_then(Value::as_str) == Some("text"))
                .and_then(|item| item.get("text"))
                .and_then(Value::as_str),
            _ => None,
        };
        if let Some(structured) = text.and_then(|text| serde_json::from_str::<Value>(text).ok()) {
            result.insert("structuredContent".to_string(), structured);
        }
        // Non-JSON prose remains text.
    }
    let mut formatted = frame_object.clone();
    formatted.insert("result".to_string(), Value::Object(result));
    Some(Value::Object(formatted))
}
