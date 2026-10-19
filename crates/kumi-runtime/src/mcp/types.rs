//! The MCP types the TypeScript took from `@modelcontextprotocol/sdk/types.js` (Tool, CallToolResult, JSON-RPC messages).
//!
//! Validation follows the SDK's zod schemas: unknown keys are dropped (kept where the SDK kept
//! them), an optional field must be absent rather than `null`, unions take their first match, and
//! an id or token is a string or a safe integer.

use std::fmt;
use std::sync::LazyLock;

use regex::Regex;
use serde::de::{self, Deserializer};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::core::contracts::JsonObject;

pub const JSONRPC_VERSION: &str = "2.0";
pub const LATEST_PROTOCOL_VERSION: &str = "2025-11-25";
pub const SUPPORTED_PROTOCOL_VERSIONS: [&str; 5] = ["2025-11-25", "2025-06-18", "2025-03-26", "2024-11-05", "2024-10-07"];

/// The SDK's `ErrorCode` values.
pub struct ErrorCode;

impl ErrorCode {
    pub const CONNECTION_CLOSED: i64 = -32000;
    pub const REQUEST_TIMEOUT: i64 = -32001;
    pub const PARSE_ERROR: i64 = -32700;
    pub const INVALID_REQUEST: i64 = -32600;
    pub const METHOD_NOT_FOUND: i64 = -32601;
    pub const INVALID_PARAMS: i64 = -32602;
    pub const INTERNAL_ERROR: i64 = -32603;
    pub const URL_ELICITATION_REQUIRED: i64 = -32042;
}

/// The SDK's `McpError`: `message` is already "MCP error <code>: <message>".
#[derive(Debug, Clone, PartialEq)]
pub struct McpError {
    pub code: i64,
    pub message: String,
    pub data: Option<Value>,
}

impl McpError {
    pub const NAME: &'static str = "McpError";

    pub fn new(code: i64, message: &str, data: Option<Value>) -> Self {
        Self { code, message: format!("MCP error {code}: {message}"), data }
    }
}

impl fmt::Display for McpError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for McpError {}

/// An optional field that must be absent rather than `null`.
fn present<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    T::deserialize(deserializer).map(Some)
}

const MAX_SAFE_INTEGER: f64 = 9_007_199_254_740_991.0;

fn safe_integer(number: f64) -> Option<i64> {
    (number.fract() == 0.0 && number.abs() <= MAX_SAFE_INTEGER).then_some(number as i64)
}

/// `z.number().int()`: a whole number within the safe range (`1.0` and `1e2` pass).
fn int<'de, D: Deserializer<'de>>(deserializer: D) -> Result<i64, D::Error> {
    let number = f64::deserialize(deserializer)?;
    safe_integer(number).ok_or_else(|| de::Error::custom("expected an integer"))
}

/// A request id or progress token: a string or a safe integer.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum RequestId {
    Number(i64),
    String(String),
}

impl RequestId {
    /// `Number(id)`: how the SDK keys responses (`"5"` is 5, `""` is 0, other words are NaN).
    pub fn to_js_number(&self) -> f64 {
        match self {
            Self::Number(number) => *number as f64,
            Self::String(text) => kumi_common::js::number::parse(text).unwrap_or(f64::NAN),
        }
    }
}

impl fmt::Display for RequestId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Number(number) => write!(f, "{number}"),
            Self::String(text) => f.write_str(text),
        }
    }
}

impl Serialize for RequestId {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Number(number) => serializer.serialize_i64(*number),
            Self::String(text) => serializer.serialize_str(text),
        }
    }
}

impl<'de> Deserialize<'de> for RequestId {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        match Value::deserialize(deserializer)? {
            Value::String(text) => Ok(Self::String(text)),
            Value::Number(number) => number
                .as_f64()
                .and_then(safe_integer)
                .map(Self::Number)
                .ok_or_else(|| de::Error::custom("expected a string or a safe integer")),
            _ => Err(de::Error::custom("expected a string or a safe integer")),
        }
    }
}

pub type ProgressToken = RequestId;

/// `"jsonrpc": "2.0"`, and nothing else.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct JsonRpcVersion;

impl Serialize for JsonRpcVersion {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(JSONRPC_VERSION)
    }
}

impl<'de> Deserialize<'de> for JsonRpcVersion {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        if text == JSONRPC_VERSION {
            Ok(Self)
        } else {
            Err(de::Error::custom("expected jsonrpc \"2.0\""))
        }
    }
}

/// A task a request or result relates to.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RelatedTask {
    pub task_id: String,
}

/// `_meta` on params or a result: its known fields checked, other keys kept.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Meta {
    #[serde(default, rename = "progressToken", deserialize_with = "present", skip_serializing_if = "Option::is_none")]
    pub progress_token: Option<ProgressToken>,
    #[serde(
        default,
        rename = "io.modelcontextprotocol/related-task",
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub related_task: Option<RelatedTask>,
    #[serde(flatten)]
    pub rest: JsonObject,
}

/// A request's `params` or a response's `result`: an object (never an array) whose `_meta` must be
/// well formed; every other key is kept.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Payload {
    #[serde(default, rename = "_meta", deserialize_with = "present", skip_serializing_if = "Option::is_none")]
    pub meta: Option<Meta>,
    #[serde(flatten)]
    pub rest: JsonObject,
}

impl Payload {
    /// A payload of these keys, with no `_meta`.
    pub fn from_object(rest: JsonObject) -> Self {
        Self { meta: None, rest }
    }
}

/// `{ method, params?, jsonrpc, id }`: keys in the order the SDK writes them.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JsonRpcRequest {
    pub method: String,
    #[serde(default, deserialize_with = "present", skip_serializing_if = "Option::is_none")]
    pub params: Option<Payload>,
    pub jsonrpc: JsonRpcVersion,
    pub id: RequestId,
}

/// `{ method, params?, jsonrpc }`; an `id` key (even null) makes it no notification.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JsonRpcNotification {
    pub method: String,
    #[serde(default, deserialize_with = "present", skip_serializing_if = "Option::is_none")]
    pub params: Option<Payload>,
    pub jsonrpc: JsonRpcVersion,
}

/// `{ result, jsonrpc, id }`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JsonRpcResponse {
    pub result: Payload,
    pub jsonrpc: JsonRpcVersion,
    pub id: RequestId,
}

/// An error response's `error` (extra keys dropped).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JsonRpcErrorBody {
    #[serde(deserialize_with = "int")]
    pub code: i64,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

/// `{ jsonrpc, id?, error }`; `id` may be left out but not null.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JsonRpcErrorResponse {
    pub jsonrpc: JsonRpcVersion,
    #[serde(default, deserialize_with = "present", skip_serializing_if = "Option::is_none")]
    pub id: Option<RequestId>,
    pub error: JsonRpcErrorBody,
}

/// `JSONRPCMessageSchema`: a request, a notification, a result or an error, tried in that order.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum JsonRpcMessage {
    Request(JsonRpcRequest),
    Notification(JsonRpcNotification),
    Response(JsonRpcResponse),
    Error(JsonRpcErrorResponse),
}

/// `{ name, title?, version, websiteUrl?, description?, icons? }`: a client or server naming itself.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Implementation {
    pub name: String,
    #[serde(default, deserialize_with = "present", skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, deserialize_with = "present", skip_serializing_if = "Option::is_none")]
    pub icons: Option<Vec<Icon>>,
    pub version: String,
    #[serde(default, deserialize_with = "present", skip_serializing_if = "Option::is_none")]
    pub website_url: Option<String>,
    #[serde(default, deserialize_with = "present", skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Icon {
    pub src: String,
    #[serde(default, deserialize_with = "present", skip_serializing_if = "Option::is_none")]
    pub mime_type: Option<String>,
    #[serde(default, deserialize_with = "present", skip_serializing_if = "Option::is_none")]
    pub sizes: Option<Vec<String>>,
    #[serde(default, deserialize_with = "present", skip_serializing_if = "Option::is_none")]
    pub theme: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListChangedCapability {
    #[serde(default, deserialize_with = "present", skip_serializing_if = "Option::is_none")]
    pub list_changed: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResourcesCapability {
    #[serde(default, deserialize_with = "present", skip_serializing_if = "Option::is_none")]
    pub subscribe: Option<bool>,
    #[serde(default, deserialize_with = "present", skip_serializing_if = "Option::is_none")]
    pub list_changed: Option<bool>,
}

/// What a server says it can do (unknown keys dropped).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerCapabilities {
    #[serde(default, deserialize_with = "present", skip_serializing_if = "Option::is_none")]
    pub experimental: Option<JsonObject>,
    #[serde(default, deserialize_with = "present", skip_serializing_if = "Option::is_none")]
    pub logging: Option<JsonObject>,
    #[serde(default, deserialize_with = "present", skip_serializing_if = "Option::is_none")]
    pub completions: Option<JsonObject>,
    #[serde(default, deserialize_with = "present", skip_serializing_if = "Option::is_none")]
    pub prompts: Option<ListChangedCapability>,
    #[serde(default, deserialize_with = "present", skip_serializing_if = "Option::is_none")]
    pub resources: Option<ResourcesCapability>,
    #[serde(default, deserialize_with = "present", skip_serializing_if = "Option::is_none")]
    pub tools: Option<ListChangedCapability>,
    #[serde(default, deserialize_with = "present", skip_serializing_if = "Option::is_none")]
    pub extensions: Option<JsonObject>,
    #[serde(default, deserialize_with = "present", skip_serializing_if = "Option::is_none")]
    pub tasks: Option<JsonObject>,
}

/// The server's answer to `initialize` (extra top-level keys kept).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InitializeResult {
    #[serde(default, rename = "_meta", deserialize_with = "present", skip_serializing_if = "Option::is_none")]
    pub meta: Option<Meta>,
    pub protocol_version: String,
    pub capabilities: ServerCapabilities,
    pub server_info: Implementation,
    #[serde(default, deserialize_with = "present", skip_serializing_if = "Option::is_none")]
    pub instructions: Option<String>,
    #[serde(flatten)]
    pub rest: JsonObject,
}

/// `"type": "object"`, and nothing else.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ObjectType {
    #[default]
    Object,
}

/// Each property's schema must be an object or an array.
fn properties<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<JsonObject>, D::Error> {
    let map = JsonObject::deserialize(deserializer)?;
    if map.values().all(|value| value.is_object() || value.is_array()) {
        Ok(Some(map))
    } else {
        Err(de::Error::custom("each property must be an object or an array"))
    }
}

/// A tool's input or output schema: `{ type: "object", properties?, required?, …others kept }`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolSchema {
    #[serde(rename = "type")]
    pub schema_type: ObjectType,
    #[serde(default, deserialize_with = "properties", skip_serializing_if = "Option::is_none")]
    pub properties: Option<JsonObject>,
    #[serde(default, deserialize_with = "present", skip_serializing_if = "Option::is_none")]
    pub required: Option<Vec<String>>,
    #[serde(flatten)]
    pub rest: JsonObject,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolAnnotations {
    #[serde(default, deserialize_with = "present", skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, deserialize_with = "present", skip_serializing_if = "Option::is_none")]
    pub read_only_hint: Option<bool>,
    #[serde(default, deserialize_with = "present", skip_serializing_if = "Option::is_none")]
    pub destructive_hint: Option<bool>,
    #[serde(default, deserialize_with = "present", skip_serializing_if = "Option::is_none")]
    pub idempotent_hint: Option<bool>,
    #[serde(default, deserialize_with = "present", skip_serializing_if = "Option::is_none")]
    pub open_world_hint: Option<bool>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TaskSupport {
    Required,
    Optional,
    Forbidden,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolExecution {
    #[serde(default, deserialize_with = "present", skip_serializing_if = "Option::is_none")]
    pub task_support: Option<TaskSupport>,
}

/// A tool as `tools/list` returns it (unknown keys dropped).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Tool {
    pub name: String,
    #[serde(default, deserialize_with = "present", skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, deserialize_with = "present", skip_serializing_if = "Option::is_none")]
    pub icons: Option<Vec<Icon>>,
    #[serde(default, deserialize_with = "present", skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub input_schema: ToolSchema,
    #[serde(default, deserialize_with = "present", skip_serializing_if = "Option::is_none")]
    pub output_schema: Option<ToolSchema>,
    #[serde(default, deserialize_with = "present", skip_serializing_if = "Option::is_none")]
    pub annotations: Option<ToolAnnotations>,
    #[serde(default, deserialize_with = "present", skip_serializing_if = "Option::is_none")]
    pub execution: Option<ToolExecution>,
    #[serde(default, rename = "_meta", deserialize_with = "present", skip_serializing_if = "Option::is_none")]
    pub meta: Option<JsonObject>,
}

/// `tools/list`'s result (extra top-level keys kept).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListToolsResult {
    #[serde(default, rename = "_meta", deserialize_with = "present", skip_serializing_if = "Option::is_none")]
    pub meta: Option<Meta>,
    #[serde(default, deserialize_with = "present", skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
    pub tools: Vec<Tool>,
    #[serde(flatten)]
    pub rest: JsonObject,
}

/// Who a content block is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    User,
    Assistant,
}

/// zod's `z.iso.datetime({ offset: true })`: a calendar date, a time with seconds, and `Z` or an offset.
static DATETIME: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(concat!(
        r"^(?:(?:[0-9][0-9][2468][048]|[0-9][0-9][13579][26]|[0-9][0-9]0[48]|[02468][048]00|[13579][26]00)-02-29|[0-9]{4}-(?:(?:0[13578]|1[02])-(?:0[1-9]|[12][0-9]|3[01])|(?:0[469]|11)-(?:0[1-9]|[12][0-9]|30)|(?:02)-(?:0[1-9]|1[0-9]|2[0-8])))",
        r"T(?:(?:[01][0-9]|2[0-3]):[0-5][0-9]:[0-5][0-9](?:\.[0-9]+)?(?:Z|(?:[+-](?:[01][0-9]|2[0-3]):[0-5][0-9])))$"
    ))
    .expect("datetime regex")
});

fn datetime<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<String>, D::Error> {
    let text = String::deserialize(deserializer)?;
    if DATETIME.is_match(&text) {
        Ok(Some(text))
    } else {
        Err(de::Error::custom("Invalid ISO datetime"))
    }
}

fn priority<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<f64>, D::Error> {
    let number = f64::deserialize(deserializer)?;
    if (0.0..=1.0).contains(&number) {
        Ok(Some(number))
    } else {
        Err(de::Error::custom("priority must be between 0 and 1"))
    }
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Annotations {
    #[serde(default, deserialize_with = "present", skip_serializing_if = "Option::is_none")]
    pub audience: Option<Vec<Role>>,
    #[serde(default, deserialize_with = "priority", skip_serializing_if = "Option::is_none")]
    pub priority: Option<f64>,
    #[serde(default, deserialize_with = "datetime", skip_serializing_if = "Option::is_none")]
    pub last_modified: Option<String>,
}

/// Whether `atob` would take the text: the forgiving-base64 decode of the HTML standard.
pub fn forgiving_base64(text: &str) -> bool {
    let mut cleaned: Vec<u8> = text.bytes().filter(|byte| !matches!(byte, b'\t' | b'\n' | b'\x0c' | b'\r' | b' ')).collect();
    if cleaned.len() % 4 == 0 {
        if cleaned.ends_with(b"==") {
            cleaned.truncate(cleaned.len() - 2);
        } else if cleaned.ends_with(b"=") {
            cleaned.truncate(cleaned.len() - 1);
        }
    }
    if cleaned.len() % 4 == 1 {
        return false;
    }
    cleaned.iter().all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'+' | b'/'))
}

/// Base64 text as the SDK accepts it (`atob` must not throw).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
pub struct Base64Text(pub String);

impl<'de> Deserialize<'de> for Base64Text {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        if forgiving_base64(&text) {
            Ok(Self(text))
        } else {
            Err(de::Error::custom("Invalid Base64 string"))
        }
    }
}

/// A resource's contents: text, or a blob (tried in that order).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ResourceContents {
    Text {
        uri: String,
        #[serde(default, rename = "mimeType", deserialize_with = "present", skip_serializing_if = "Option::is_none")]
        mime_type: Option<String>,
        #[serde(default, rename = "_meta", deserialize_with = "present", skip_serializing_if = "Option::is_none")]
        meta: Option<JsonObject>,
        text: String,
    },
    Blob {
        uri: String,
        #[serde(default, rename = "mimeType", deserialize_with = "present", skip_serializing_if = "Option::is_none")]
        mime_type: Option<String>,
        #[serde(default, rename = "_meta", deserialize_with = "present", skip_serializing_if = "Option::is_none")]
        meta: Option<JsonObject>,
        blob: Base64Text,
    },
}

/// A block of a tool result's content; an unknown `type` fails.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", rename_all_fields = "camelCase")]
pub enum ContentBlock {
    Text {
        text: String,
        #[serde(default, deserialize_with = "present", skip_serializing_if = "Option::is_none")]
        annotations: Option<Annotations>,
        #[serde(default, rename = "_meta", deserialize_with = "present", skip_serializing_if = "Option::is_none")]
        meta: Option<JsonObject>,
    },
    Image {
        data: Base64Text,
        mime_type: String,
        #[serde(default, deserialize_with = "present", skip_serializing_if = "Option::is_none")]
        annotations: Option<Annotations>,
        #[serde(default, rename = "_meta", deserialize_with = "present", skip_serializing_if = "Option::is_none")]
        meta: Option<JsonObject>,
    },
    Audio {
        data: Base64Text,
        mime_type: String,
        #[serde(default, deserialize_with = "present", skip_serializing_if = "Option::is_none")]
        annotations: Option<Annotations>,
        #[serde(default, rename = "_meta", deserialize_with = "present", skip_serializing_if = "Option::is_none")]
        meta: Option<JsonObject>,
    },
    ResourceLink {
        name: String,
        #[serde(default, deserialize_with = "present", skip_serializing_if = "Option::is_none")]
        title: Option<String>,
        #[serde(default, deserialize_with = "present", skip_serializing_if = "Option::is_none")]
        icons: Option<Vec<Icon>>,
        uri: String,
        #[serde(default, deserialize_with = "present", skip_serializing_if = "Option::is_none")]
        description: Option<String>,
        #[serde(default, deserialize_with = "present", skip_serializing_if = "Option::is_none")]
        mime_type: Option<String>,
        #[serde(default, deserialize_with = "present", skip_serializing_if = "Option::is_none")]
        size: Option<f64>,
        #[serde(default, deserialize_with = "present", skip_serializing_if = "Option::is_none")]
        annotations: Option<Annotations>,
        #[serde(default, rename = "_meta", deserialize_with = "present", skip_serializing_if = "Option::is_none")]
        meta: Option<JsonObject>,
    },
    Resource {
        resource: ResourceContents,
        #[serde(default, deserialize_with = "present", skip_serializing_if = "Option::is_none")]
        annotations: Option<Annotations>,
        #[serde(default, rename = "_meta", deserialize_with = "present", skip_serializing_if = "Option::is_none")]
        meta: Option<JsonObject>,
    },
}

impl ContentBlock {
    pub fn text(text: impl Into<String>) -> Self {
        Self::Text { text: text.into(), annotations: None, meta: None }
    }
}

/// `tools/call`'s result (extra top-level keys kept); `content` is `[]` only when absent.
/// Input field order is retained because an unwrapped/error result is shown as JSON to the model.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct CallToolResult {
    pub meta: Option<Meta>,
    pub content: Vec<ContentBlock>,
    pub structured_content: Option<JsonObject>,
    pub is_error: Option<bool>,
    pub rest: JsonObject,
    pub field_order: Vec<String>,
}
impl<'de> Deserialize<'de> for CallToolResult {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct Fields {
            #[serde(default, rename = "_meta", deserialize_with = "present")]
            meta: Option<Meta>,
            #[serde(default)]
            content: Vec<ContentBlock>,
            #[serde(default, deserialize_with = "present")]
            structured_content: Option<JsonObject>,
            #[serde(default, deserialize_with = "present")]
            is_error: Option<bool>,
            #[serde(flatten)]
            rest: JsonObject,
        }
        let value = JsonObject::deserialize(deserializer)?;
        let field_order = value.keys().cloned().collect();
        let fields: Fields = serde_json::from_value(Value::Object(value)).map_err(serde::de::Error::custom)?;
        Ok(Self {
            meta: fields.meta,
            content: fields.content,
            structured_content: fields.structured_content,
            is_error: fields.is_error,
            rest: fields.rest,
            field_order,
        })
    }
}
impl Serialize for CallToolResult {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeMap;
        let mut keys: Vec<&str> = self.field_order.iter().map(String::as_str).collect();
        for key in ["_meta", "content", "structuredContent", "isError"].into_iter().chain(self.rest.keys().map(String::as_str)) {
            if !keys.contains(&key) {
                keys.push(key);
            }
        }
        let mut out = serializer.serialize_map(None)?;
        for key in keys {
            match key {
                "_meta" => {
                    if let Some(value) = &self.meta {
                        out.serialize_entry(key, value)?;
                    }
                }
                "content" => out.serialize_entry(key, &self.content)?,
                "structuredContent" => {
                    if let Some(value) = &self.structured_content {
                        out.serialize_entry(key, value)?;
                    }
                }
                "isError" => {
                    if let Some(value) = self.is_error {
                        out.serialize_entry(key, &value)?;
                    }
                }
                _ => {
                    if let Some(value) = self.rest.get(key) {
                        out.serialize_entry(key, value)?;
                    }
                }
            }
        }
        out.end()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn ids_are_strings_or_safe_integers() {
        assert_eq!(serde_json::from_value::<RequestId>(json!(1.0)).unwrap(), RequestId::Number(1));
        assert_eq!(serde_json::from_value::<RequestId>(json!(1e2)).unwrap(), RequestId::Number(100));
        assert_eq!(serde_json::from_value::<RequestId>(json!("5")).unwrap(), RequestId::String("5".into()));
        for bad in [json!(1.5), json!(9007199254740992.0), json!(null), json!(true)] {
            assert!(serde_json::from_value::<RequestId>(bad).is_err());
        }
        assert_eq!(RequestId::String("5".into()).to_js_number(), 5.0);
        assert_eq!(RequestId::String(String::new()).to_js_number(), 0.0);
        assert!(RequestId::String("x".into()).to_js_number().is_nan());
    }

    #[test]
    fn envelopes_are_strict() {
        let request: JsonRpcMessage = serde_json::from_value(json!({"jsonrpc": "2.0", "id": 0, "method": "ping"})).unwrap();
        assert!(matches!(request, JsonRpcMessage::Request(_)));
        let notification: JsonRpcMessage =
            serde_json::from_value(json!({"jsonrpc": "2.0", "method": "notifications/initialized"})).unwrap();
        assert!(matches!(notification, JsonRpcMessage::Notification(_)));
        let response: JsonRpcMessage = serde_json::from_value(json!({"jsonrpc": "2.0", "id": "a", "result": {"x": 1}})).unwrap();
        assert!(matches!(response, JsonRpcMessage::Response(_)));
        let error: JsonRpcMessage =
            serde_json::from_value(json!({"jsonrpc": "2.0", "error": {"code": -32700, "message": "Parse error", "extra": 1}})).unwrap();
        assert!(matches!(error, JsonRpcMessage::Error(JsonRpcErrorResponse { id: None, .. })));
        for bad in [
            json!({"jsonrpc": "2.0", "id": null, "method": "ping"}),
            json!({"jsonrpc": "2.0", "method": "x", "id": null}),
            json!({"jsonrpc": "2.0", "id": 1, "result": [], }),
            json!({"jsonrpc": "2.0", "id": 1, "result": {}, "extra": true}),
            json!({"jsonrpc": "2.0", "id": null, "error": {"code": -32700, "message": "Parse error"}}),
            json!({"jsonrpc": "2.0", "id": 1, "result": {}, "error": {"code": 1, "message": "both"}}),
            json!({"jsonrpc": "1.0", "id": 1, "method": "ping"}),
            json!({"jsonrpc": "2.0", "id": 1, "method": "ping", "params": []}),
            json!({"jsonrpc": "2.0", "id": 1, "method": "ping", "params": {"_meta": {"progressToken": 1.5}}}),
            json!([]),
            json!("ping"),
        ] {
            assert!(serde_json::from_value::<JsonRpcMessage>(bad.clone()).is_err(), "{bad}");
        }
        let request = JsonRpcRequest {
            method: "tools/call".into(),
            params: Some(Payload::from_object(serde_json::from_value(json!({"name": "t", "arguments": {}})).unwrap())),
            jsonrpc: JsonRpcVersion,
            id: RequestId::Number(3),
        };
        assert_eq!(
            kumi_common::js::json::stringify(&serde_json::to_value(&request).unwrap()),
            r#"{"method":"tools/call","params":{"name":"t","arguments":{}},"jsonrpc":"2.0","id":3}"#
        );
        let initialized = JsonRpcNotification { method: "notifications/initialized".into(), params: None, jsonrpc: JsonRpcVersion };
        assert_eq!(
            kumi_common::js::json::stringify(&serde_json::to_value(&initialized).unwrap()),
            r#"{"method":"notifications/initialized","jsonrpc":"2.0"}"#
        );
    }

    #[test]
    fn tool_results_validate_as_the_sdk_did() {
        let result: CallToolResult =
            serde_json::from_value(json!({"content": [{"type": "text", "text": "hi", "junk": 1}], "other": 2})).unwrap();
        assert_eq!(result.content, vec![ContentBlock::text("hi")]);
        assert_eq!(result.rest.get("other"), Some(&json!(2)));
        let empty: CallToolResult = serde_json::from_value(json!({})).unwrap();
        assert!(empty.content.is_empty());
        for bad in [
            json!({"content": null}),
            json!({"content": [{"type": "video", "data": ""}]}),
            json!({"content": [], "isError": null}),
            json!({"content": [{"type": "image", "data": "!!", "mimeType": "image/png"}]}),
            json!({"content": [{"type": "text", "text": "x", "annotations": {"priority": 2}}]}),
            json!({"content": [{"type": "text", "text": "x", "annotations": {"lastModified": "2025-02-30T00:00:00Z"}}]}),
        ] {
            assert!(serde_json::from_value::<CallToolResult>(bad.clone()).is_err(), "{bad}");
        }
        let ok: CallToolResult = serde_json::from_value(json!({"content": [
            {"type": "image", "data": "aGk=", "mimeType": "image/png"},
            {"type": "resource", "resource": {"uri": "x://y", "text": "t"}},
            {"type": "resource_link", "name": "n", "uri": "x://y"},
            {"type": "text", "text": "x", "annotations": {"lastModified": "2025-02-28T10:00:00.5+02:00", "audience": ["user"]}},
        ]}))
        .unwrap();
        assert_eq!(ok.content.len(), 4);
        let tools: ListToolsResult = serde_json::from_value(
            json!({"tools": [{"name": "t", "inputSchema": {"type": "object", "properties": {"a": {"type": "string"}}, "x": 1}}]}),
        )
        .unwrap();
        assert_eq!(tools.tools[0].input_schema.rest.get("x"), Some(&json!(1)));
        assert!(serde_json::from_value::<ListToolsResult>(
            json!({"tools": [{"name": "t", "inputSchema": {"type": "object", "properties": {"a": 1}}}]})
        )
        .is_err());
        assert!(serde_json::from_value::<ListToolsResult>(json!({"tools": [{"name": "t", "inputSchema": {"type": "array"}}]})).is_err());
    }

    #[test]
    fn base64_is_forgiving_like_atob() {
        assert!(forgiving_base64("aGVsbG8="));
        assert!(forgiving_base64("aGVs bG8=\n"));
        assert!(forgiving_base64(""));
        assert!(!forgiving_base64("a"));
        assert!(!forgiving_base64("!!!!"));
        assert!(!forgiving_base64("aGVsbG8=="));
    }
}
