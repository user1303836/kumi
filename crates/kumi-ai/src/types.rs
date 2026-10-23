//! Part of the `@ai-sdk` replacement (see `ai/mod.rs`): the `LanguageModelV4` types, as
//! `@ai-sdk/provider` 4.0 declared them. Messages are the checkpoint format on disk, so they
//! round-trip through JSON byte for byte: the same keys in the same order, optionals left out.

use std::collections::BTreeMap;
use std::pin::Pin;

use futures::Stream;
use kumi_common::abort::Signal;
use serde::de::{self, MapAccess, Visitor};
use serde::ser::SerializeMap;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::Value;

use super::error::LanguageModelError;

/// `SharedV4ProviderOptions` and `SharedV4ProviderMetadata`: `Record<providerName, JSONObject>`.
pub type ProviderOptions = serde_json::Map<String, Value>;
pub type ProviderMetadata = ProviderOptions;

/// `LanguageModelV4Prompt`.
pub type Prompt = Vec<Message>;

/// The parts a model streams, as `doStream` returns them.
pub type StreamParts = Pin<Box<dyn Stream<Item = StreamPart>>>;

/// A file's bytes: raw (`Uint8Array`) or base64 text. Raw bytes serialize as JavaScript serializes a
/// `Uint8Array` (an object of indices), which Kumi never lets reach a file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DataContent {
    Bytes(Vec<u8>),
    Base64(String),
}

thread_local! {
    /// Set while a provider's request body is built (see [`to_provider_value`]).
    static BYTES_AS_BASE64: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// `serde_json::to_value` for a provider's request body: file bytes become base64 text, as the body
/// carries them, rather than `JSON.stringify(Uint8Array)`'s object with a key per byte, which saved
/// conversations keep. A turn's images (megabytes) were rebuilt from that object on every step.
pub fn to_provider_value<T: Serialize + ?Sized>(value: &T) -> Value {
    struct Restore(bool);
    impl Drop for Restore {
        fn drop(&mut self) {
            BYTES_AS_BASE64.with(|flag| flag.set(self.0));
        }
    }
    let _restore = Restore(BYTES_AS_BASE64.with(|flag| flag.replace(true)));
    serde_json::to_value(value).expect("a prompt serializes")
}

impl Serialize for DataContent {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Base64(text) => serializer.serialize_str(text),
            Self::Bytes(bytes) if BYTES_AS_BASE64.with(std::cell::Cell::get) => {
                use base64::{engine::general_purpose::STANDARD, Engine};
                serializer.serialize_str(&STANDARD.encode(bytes))
            }
            Self::Bytes(bytes) => {
                let mut map = serializer.serialize_map(Some(bytes.len()))?;
                for (index, byte) in bytes.iter().enumerate() {
                    map.serialize_entry(&index.to_string(), byte)?;
                }
                map.end()
            }
        }
    }
}

impl<'de> Deserialize<'de> for DataContent {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct DataVisitor;
        impl<'de> Visitor<'de> for DataVisitor {
            type Value = DataContent;
            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("base64 text or an object of byte indices")
            }
            fn visit_str<E: de::Error>(self, text: &str) -> Result<DataContent, E> {
                Ok(DataContent::Base64(text.to_string()))
            }
            fn visit_string<E: de::Error>(self, text: String) -> Result<DataContent, E> {
                Ok(DataContent::Base64(text))
            }
            fn visit_bytes<E: de::Error>(self, bytes: &[u8]) -> Result<DataContent, E> {
                Ok(DataContent::Bytes(bytes.to_vec()))
            }
            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<DataContent, A::Error> {
                let mut bytes = Vec::new();
                while let Some((key, value)) = map.next_entry::<String, u8>()? {
                    if key != bytes.len().to_string() {
                        return Err(de::Error::custom("byte indices must run from 0"));
                    }
                    bytes.push(value);
                }
                Ok(DataContent::Bytes(bytes))
            }
        }
        deserializer.deserialize_any(DataVisitor)
    }
}

/// `SharedV4FileData`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case", rename_all_fields = "camelCase")]
pub enum FileData {
    Data {
        data: DataContent,
    },
    Url {
        url: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        original_url: Option<String>,
    },
    Reference {
        reference: Value,
    },
    Text {
        text: String,
    },
}

/// `LanguageModelV4Message`: one of a prompt's messages. Each may carry `providerOptions`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "role", rename_all = "lowercase", rename_all_fields = "camelCase")]
pub enum Message {
    System {
        content: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        provider_options: Option<ProviderOptions>,
    },
    User {
        content: Vec<UserPart>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        provider_options: Option<ProviderOptions>,
    },
    Assistant {
        content: Vec<AssistantPart>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        provider_options: Option<ProviderOptions>,
    },
    Tool {
        content: Vec<ToolPart>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        provider_options: Option<ProviderOptions>,
    },
}

/// A message's role, as its `role` key spells it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    System,
    User,
    Assistant,
    Tool,
}

impl Message {
    pub fn role(&self) -> Role {
        match self {
            Self::System { .. } => Role::System,
            Self::User { .. } => Role::User,
            Self::Assistant { .. } => Role::Assistant,
            Self::Tool { .. } => Role::Tool,
        }
    }

    /// `{ role: "user", content: [{ type: "text", text }] }`.
    pub fn user_text(text: impl Into<String>) -> Self {
        Self::User { content: vec![UserPart::Text(TextPart::new(text))], provider_options: None }
    }

    /// `{ role: "assistant", content: [{ type: "text", text }] }`.
    pub fn assistant_text(text: impl Into<String>) -> Self {
        Self::Assistant { content: vec![AssistantPart::Text(TextPart::new(text))], provider_options: None }
    }

    pub fn provider_options(&self) -> Option<&ProviderOptions> {
        match self {
            Self::System { provider_options, .. }
            | Self::User { provider_options, .. }
            | Self::Assistant { provider_options, .. }
            | Self::Tool { provider_options, .. } => provider_options.as_ref(),
        }
    }

    pub fn provider_options_mut(&mut self) -> &mut Option<ProviderOptions> {
        match self {
            Self::System { provider_options, .. }
            | Self::User { provider_options, .. }
            | Self::Assistant { provider_options, .. }
            | Self::Tool { provider_options, .. } => provider_options,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TextPart {
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_options: Option<ProviderOptions>,
}

impl TextPart {
    pub fn new(text: impl Into<String>) -> Self {
        Self { text: text.into(), provider_options: None }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReasoningPart {
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_options: Option<ProviderOptions>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FilePart {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filename: Option<String>,
    pub data: FileData,
    pub media_type: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_options: Option<ProviderOptions>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReasoningFilePart {
    pub data: FileData,
    pub media_type: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_options: Option<ProviderOptions>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CustomPart {
    /// `{provider}.{provider-type}`.
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_options: Option<ProviderOptions>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolCallPart {
    pub tool_call_id: String,
    pub tool_name: String,
    /// A JSON value (an object) matching the tool's input schema.
    pub input: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_executed: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_options: Option<ProviderOptions>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolResultPart {
    pub tool_call_id: String,
    pub tool_name: String,
    pub output: ToolResultOutput,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_options: Option<ProviderOptions>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolApprovalResponsePart {
    pub approval_id: String,
    pub approved: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_options: Option<ProviderOptions>,
}

/// `LanguageModelV4ToolResultOutput`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case", rename_all_fields = "camelCase")]
pub enum ToolResultOutput {
    Text {
        value: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        provider_options: Option<ProviderOptions>,
    },
    Json {
        value: Value,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        provider_options: Option<ProviderOptions>,
    },
    ExecutionDenied {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        provider_options: Option<ProviderOptions>,
    },
    ErrorText {
        value: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        provider_options: Option<ProviderOptions>,
    },
    ErrorJson {
        value: Value,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        provider_options: Option<ProviderOptions>,
    },
    Content {
        value: Vec<ToolResultContentItem>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        provider_options: Option<ProviderOptions>,
    },
}

impl ToolResultOutput {
    pub fn text(value: impl Into<String>) -> Self {
        Self::Text { value: value.into(), provider_options: None }
    }

    pub fn error_text(value: impl Into<String>) -> Self {
        Self::ErrorText { value: value.into(), provider_options: None }
    }
}

/// An item of a `content` tool output: words, a file, or a provider's own part.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case", rename_all_fields = "camelCase")]
pub enum ToolResultContentItem {
    Text {
        text: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        provider_options: Option<ProviderOptions>,
    },
    File {
        data: FileData,
        media_type: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        filename: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        provider_options: Option<ProviderOptions>,
    },
    Custom {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        provider_options: Option<ProviderOptions>,
    },
}

/// A user message's part.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum UserPart {
    Text(TextPart),
    File(FilePart),
}

/// An assistant message's part.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum AssistantPart {
    Text(TextPart),
    File(FilePart),
    Custom(CustomPart),
    Reasoning(ReasoningPart),
    ReasoningFile(ReasoningFilePart),
    ToolCall(ToolCallPart),
    ToolResult(ToolResultPart),
}

/// A tool message's part.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum ToolPart {
    ToolResult(ToolResultPart),
    ToolApprovalResponse(ToolApprovalResponsePart),
}

impl UserPart {
    pub fn provider_options_mut(&mut self) -> &mut Option<ProviderOptions> {
        match self {
            Self::Text(part) => &mut part.provider_options,
            Self::File(part) => &mut part.provider_options,
        }
    }
}

impl AssistantPart {
    pub fn provider_options_mut(&mut self) -> &mut Option<ProviderOptions> {
        match self {
            Self::Text(part) => &mut part.provider_options,
            Self::File(part) => &mut part.provider_options,
            Self::Custom(part) => &mut part.provider_options,
            Self::Reasoning(part) => &mut part.provider_options,
            Self::ReasoningFile(part) => &mut part.provider_options,
            Self::ToolCall(part) => &mut part.provider_options,
            Self::ToolResult(part) => &mut part.provider_options,
        }
    }
}

impl ToolPart {
    pub fn provider_options_mut(&mut self) -> &mut Option<ProviderOptions> {
        match self {
            Self::ToolResult(part) => &mut part.provider_options,
            Self::ToolApprovalResponse(part) => &mut part.provider_options,
        }
    }
}

/// The `"type": "function"` every function tool starts with.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FunctionToolType {
    #[default]
    Function,
}

/// `LanguageModelV4FunctionTool`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FunctionTool {
    #[serde(rename = "type", default)]
    pub tool_type: FunctionToolType,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// JSON Schema (draft 7), verbatim.
    pub input_schema: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_examples: Option<Vec<Value>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub strict: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_options: Option<ProviderOptions>,
}

impl FunctionTool {
    pub fn new(name: impl Into<String>, description: impl Into<String>, input_schema: Value) -> Self {
        Self {
            tool_type: FunctionToolType::Function,
            name: name.into(),
            description: Some(description.into()),
            input_schema,
            input_examples: None,
            strict: None,
            provider_options: None,
        }
    }
}

/// `LanguageModelV4ToolChoice`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case", rename_all_fields = "camelCase")]
pub enum ToolChoice {
    Auto,
    None,
    Required,
    Tool { tool_name: String },
}

/// The `reasoning` effort a call asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Reasoning {
    ProviderDefault,
    None,
    Minimal,
    Low,
    Medium,
    High,
    Xhigh,
}

/// `LanguageModelV4CallOptions`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CallOptions {
    pub prompt: Prompt,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_output_tokens: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stop_sequences: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub top_k: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub presence_penalty: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub frequency_penalty: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub response_format: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seed: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<FunctionTool>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_choice: Option<ToolChoice>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub include_raw_chunks: Option<bool>,
    #[serde(skip)]
    pub abort_signal: Option<Signal>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub headers: Option<BTreeMap<String, String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<Reasoning>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_options: Option<ProviderOptions>,
}

/// `LanguageModelV4FinishReason["unified"]`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum FinishReasonUnified {
    Stop,
    Length,
    ContentFilter,
    ToolCalls,
    Error,
    Other,
}

/// `LanguageModelV4FinishReason`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FinishReason {
    pub unified: FinishReasonUnified,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub raw: Option<String>,
}

/// `LanguageModelV4Usage["inputTokens"]`; every field `number | undefined`.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InputTokens {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub no_cache: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_read: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_write: Option<f64>,
}

/// `LanguageModelV4Usage["outputTokens"]`.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OutputTokens {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<f64>,
}

/// `LanguageModelV4Usage`.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Usage {
    pub input_tokens: InputTokens,
    pub output_tokens: OutputTokens,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub raw: Option<Value>,
}

/// `LanguageModelV4ToolCall`: a call as the model made it, its input still JSON text.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolCall {
    pub tool_call_id: String,
    pub tool_name: String,
    /// Stringified JSON object with the tool call arguments.
    pub input: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_executed: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dynamic: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_metadata: Option<ProviderMetadata>,
}

/// `LanguageModelV4ToolResult`: a result of a tool the provider ran itself.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderToolResult {
    pub tool_call_id: String,
    pub tool_name: String,
    pub result: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub is_error: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preliminary: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dynamic: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_metadata: Option<ProviderMetadata>,
}

/// `LanguageModelV4StreamPart`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case", rename_all_fields = "camelCase")]
pub enum StreamPart {
    TextStart {
        id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        provider_metadata: Option<ProviderMetadata>,
    },
    TextDelta {
        id: String,
        delta: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        provider_metadata: Option<ProviderMetadata>,
    },
    TextEnd {
        id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        provider_metadata: Option<ProviderMetadata>,
    },
    ReasoningStart {
        id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        provider_metadata: Option<ProviderMetadata>,
    },
    ReasoningDelta {
        id: String,
        delta: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        provider_metadata: Option<ProviderMetadata>,
    },
    ReasoningEnd {
        id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        provider_metadata: Option<ProviderMetadata>,
    },
    ToolInputStart {
        id: String,
        tool_name: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        provider_metadata: Option<ProviderMetadata>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        provider_executed: Option<bool>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        dynamic: Option<bool>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        title: Option<String>,
    },
    ToolInputDelta {
        id: String,
        delta: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        provider_metadata: Option<ProviderMetadata>,
    },
    ToolInputEnd {
        id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        provider_metadata: Option<ProviderMetadata>,
    },
    ToolApprovalRequest {
        approval_id: String,
        tool_call_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        provider_metadata: Option<ProviderMetadata>,
    },
    ToolCall(ToolCall),
    ToolResult(ProviderToolResult),
    Custom {
        kind: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        provider_metadata: Option<ProviderMetadata>,
    },
    File {
        media_type: String,
        data: FileData,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        provider_metadata: Option<ProviderMetadata>,
    },
    ReasoningFile {
        media_type: String,
        data: FileData,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        provider_metadata: Option<ProviderMetadata>,
    },
    /// A source (`sourceType` "url" or "document") with its own fields.
    Source(serde_json::Map<String, Value>),
    StreamStart {
        warnings: Vec<Value>,
    },
    ResponseMetadata {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        id: Option<String>,
        /// Milliseconds since the epoch (a `Date` in the SDK).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        timestamp: Option<i64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        model_id: Option<String>,
    },
    Finish {
        usage: Usage,
        finish_reason: FinishReason,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        provider_metadata: Option<ProviderMetadata>,
    },
    Raw {
        raw_value: Value,
    },
    Error {
        error: LanguageModelError,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use kumi_common::js::json::stringify;
    use serde_json::json;

    /// Parsed and written again, a message's JSON is the same text: the checkpoint format holds.
    fn round_trip(text: &str) -> Message {
        let message: Message = serde_json::from_str(text).unwrap();
        assert_eq!(stringify(&serde_json::to_value(&message).unwrap()), text);
        message
    }

    #[test]
    fn checkpoint_messages_round_trip_byte_for_byte() {
        let user = round_trip(r#"{"role":"user","content":[{"type":"text","text":"hi"}]}"#);
        assert_eq!(user, Message::user_text("hi"));
        round_trip(r#"{"role":"assistant","content":[{"type":"text","text":"hello"}]}"#);
        round_trip(
            r#"{"role":"assistant","content":[{"type":"tool-call","toolCallId":"c1","toolName":"lookup","input":{"key":"tempo"}}]}"#,
        );
        round_trip(
            r#"{"role":"tool","content":[{"type":"tool-result","toolCallId":"c1","toolName":"lookup","output":{"type":"text","value":"{\"tempo\":120}"}}]}"#,
        );
        round_trip(
            r#"{"role":"tool","content":[{"type":"tool-result","toolCallId":"c1","toolName":"flaky","output":{"type":"error-text","value":"Live read failed"}}]}"#,
        );
        round_trip(
            r#"{"role":"assistant","content":[{"type":"reasoning","text":"","providerOptions":{"openai":{"itemId":"rs_1","reasoningEncryptedContent":"enc"},"anthropic":{"signature":"sig"}}},{"type":"text","text":"first","providerOptions":{"openai":{"itemId":"msg_1"}}}]}"#,
        );
        round_trip(
            r#"{"role":"assistant","content":[{"type":"reasoning","text":"","providerOptions":{"openai":{"itemId":"rs_c","reasoningEncryptedContent":null}}},{"type":"tool-call","toolCallId":"c","toolName":"live_discover","input":{"kind":"track"},"providerOptions":{"openai":{"itemId":"fc_c"}}}]}"#,
        );
        round_trip(r#"{"role":"system","content":"You are Kumi.","providerOptions":{"anthropic":{"cacheControl":{"type":"ephemeral"}}}}"#);
        round_trip(
            r#"{"role":"user","content":[{"type":"text","text":"a"}],"providerOptions":{"anthropic":{"cacheControl":{"type":"ephemeral"}}}}"#,
        );
        round_trip(
            r#"{"role":"tool","content":[{"type":"tool-result","toolCallId":"c1","toolName":"watch","output":{"type":"content","value":[{"type":"text","text":"Video"},{"type":"file","data":{"type":"data","data":"/9g="},"mediaType":"image/jpeg"}]}}]}"#,
        );
        round_trip(r#"{"role":"tool","content":[{"type":"tool-approval-response","approvalId":"a1","approved":true,"reason":"ok"}]}"#);
        round_trip(
            r#"{"role":"user","content":[{"type":"file","filename":"ref.wav","data":{"type":"url","url":"https://x/y"},"mediaType":"audio/wav"}]}"#,
        );
    }

    #[test]
    fn raw_bytes_serialize_as_javascript_serializes_a_uint8array() {
        let part = ToolResultContentItem::File {
            data: FileData::Data { data: DataContent::Bytes(vec![0xff, 0xd8]) },
            media_type: "image/jpeg".into(),
            filename: None,
            provider_options: None,
        };
        let text = stringify(&serde_json::to_value(&part).unwrap());
        assert_eq!(text, r#"{"type":"file","data":{"type":"data","data":{"0":255,"1":216}},"mediaType":"image/jpeg"}"#);
        assert_eq!(serde_json::from_str::<ToolResultContentItem>(&text).unwrap(), part);
    }

    #[test]
    fn stream_parts_tools_and_options_keep_their_shapes() {
        let finish: StreamPart = serde_json::from_value(json!({
            "type": "finish", "usage": {"inputTokens": {"total": 3, "noCache": 2, "cacheRead": 1, "cacheWrite": 0}, "outputTokens": {"total": 2, "text": 2, "reasoning": 0}},
            "finishReason": {"unified": "tool-calls", "raw": "tool_use"},
        }))
        .unwrap();
        match &finish {
            StreamPart::Finish { usage, finish_reason, .. } => {
                assert_eq!(usage.input_tokens.total, Some(3.0));
                assert_eq!(finish_reason.unified, FinishReasonUnified::ToolCalls);
            }
            other => panic!("{other:?}"),
        }
        let call: StreamPart =
            serde_json::from_value(json!({"type": "tool-call", "toolCallId": "c1", "toolName": "lookup", "input": "{}"})).unwrap();
        assert!(matches!(call, StreamPart::ToolCall(ToolCall { ref input, .. }) if input == "{}"));
        let tool = FunctionTool::new("lookup", "lookup fixture", json!({"type": "object", "properties": {}}));
        assert_eq!(
            stringify(&serde_json::to_value(&tool).unwrap()),
            r#"{"type":"function","name":"lookup","description":"lookup fixture","inputSchema":{"type":"object","properties":{}}}"#
        );
        assert_eq!(stringify(&serde_json::to_value(ToolChoice::Auto).unwrap()), r#"{"type":"auto"}"#);
        assert_eq!(
            stringify(&serde_json::to_value(ToolChoice::Tool { tool_name: "x".into() }).unwrap()),
            r#"{"type":"tool","toolName":"x"}"#
        );
        let options = CallOptions { prompt: vec![Message::user_text("hi")], tools: Some(vec![tool]), ..CallOptions::default() };
        let json = serde_json::to_value(&options).unwrap();
        assert_eq!(json.as_object().unwrap().keys().collect::<Vec<_>>(), ["prompt", "tools"]);
    }
}

#[cfg(test)]
mod provider_value_tests {
    use super::*;

    #[test]
    fn provider_bodies_carry_base64_and_saved_conversations_keep_byte_indices() {
        let bytes: Vec<u8> = (0..=255).cycle().take(1_800_000).collect();
        let data = DataContent::Bytes(bytes.clone());
        use base64::{engine::general_purpose::STANDARD, Engine};
        assert_eq!(to_provider_value(&data), Value::String(STANDARD.encode(&bytes)));
        // Saved as JSON.stringify(Uint8Array) wrote it, before and after a provider body is built.
        let small = DataContent::Bytes(vec![7, 0, 255]);
        let saved = serde_json::json!({"0":7,"1":0,"2":255});
        assert_eq!(serde_json::to_value(&small).unwrap(), saved);
        assert_eq!(to_provider_value(&small), Value::String("BwD/".into()));
        assert_eq!(serde_json::to_value(&small).unwrap(), saved);
        // Base64 text stays as it is either way.
        assert_eq!(to_provider_value(&DataContent::Base64("BwD/".into())), Value::String("BwD/".into()));
    }
}
