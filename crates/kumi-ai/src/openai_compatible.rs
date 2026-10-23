//! Part of the `@ai-sdk` replacement (see `ai/mod.rs`).

use super::{
    error::LanguageModelError,
    http::{post_json, Fetch, Headers},
    sse::json_stream,
    types::{
        AssistantPart, CallOptions, DataContent, FileData, FinishReason, FinishReasonUnified, InputTokens, Message, OutputTokens,
        ProviderMetadata, Reasoning, StreamPart, StreamParts, ToolCall, ToolChoice, ToolPart, ToolResultOutput, Usage, UserPart,
    },
};
use crate::kernel::agent::LanguageModel;
use async_trait::async_trait;
use base64::{engine::general_purpose::STANDARD, Engine};
use futures::{Stream, StreamExt};
use indexmap::IndexMap;
use kumi_common::js::json::stringify;
use serde_json::{json, Map, Value};
use std::{
    collections::{HashMap, HashSet, VecDeque},
    pin::Pin,
    rc::Rc,
};

pub struct CompatibleSettings {
    pub name: String,
    pub model: String,
    pub base_url: String,
    pub api_key: Option<String>,
    pub headers: Headers,
    pub fetch: Rc<dyn Fetch>,
    pub include_usage: bool,
}
struct CompatibleModel(CompatibleSettings);
pub fn openai_compatible(settings: CompatibleSettings) -> Rc<dyn LanguageModel> {
    Rc::new(CompatibleModel(settings))
}
fn camel(name: &str) -> String {
    let mut text = String::new();
    let mut chars = name.chars().peekable();
    while let Some(c) = chars.next() {
        if (c == '-' || c == '_') && chars.peek().is_some_and(char::is_ascii_lowercase) {
            text.push(chars.next().unwrap().to_ascii_uppercase());
        } else {
            text.push(c);
        }
    }
    text
}
fn metadata(value: &Option<Map<String, Value>>) -> Map<String, Value> {
    value.as_ref().and_then(|options| options.get("openaiCompatible")).and_then(Value::as_object).cloned().unwrap_or_default()
}
fn merge(value: &mut Value, more: Map<String, Value>) {
    value.as_object_mut().unwrap().extend(more);
}
fn encoded(data: &DataContent) -> String {
    match data {
        DataContent::Bytes(data) => STANDARD.encode(data),
        DataContent::Base64(data) => data.clone(),
    }
}
fn unsupported(feature: &str) -> LanguageModelError {
    LanguageModelError::other(format!("Unsupported functionality: {feature}"))
}
fn messages(prompt: &[Message], key: &str) -> Result<Vec<Value>, LanguageModelError> {
    let mut messages = vec![];
    for message in prompt {
        let message_metadata = metadata(&message.provider_options().cloned());
        match message {
            Message::System { content, .. } => {
                let mut value = json!({"role":"system","content":content});
                merge(&mut value, message_metadata);
                messages.push(value);
            }
            Message::User { content, .. } => {
                if let [UserPart::Text(part)] = content.as_slice() {
                    let mut value = json!({"role":"user","content":part.text});
                    merge(&mut value, metadata(&part.provider_options));
                    messages.push(value);
                    continue;
                }
                let mut parts = vec![];
                for part in content {
                    let (mut value, options) = match part {
                        UserPart::Text(part) => (json!({"type":"text","text":part.text}), &part.provider_options),
                        UserPart::File(part) => {
                            let url = match &part.data {
                                FileData::Data { data } => format!("data:{};base64,{}", part.media_type, encoded(data)),
                                FileData::Url { url, .. } => url.clone(),
                                FileData::Reference { .. } => return Err(unsupported("file parts with provider references")),
                                FileData::Text { .. } => return Err(unsupported("text file parts")),
                            };
                            let value = match part.media_type.split('/').next().unwrap_or("") {
                                "image" => json!({"type":"image_url","image_url":{"url":url}}),
                                "video" => json!({"type":"video_url","video_url":{"url":url}}),
                                "audio" => {
                                    let FileData::Data { data } = &part.data else {
                                        return Err(unsupported("audio file parts with URLs"));
                                    };
                                    let format = match part.media_type.as_str() {
                                        "audio/wav" => "wav",
                                        "audio/mpeg" | "audio/mp3" => "mp3",
                                        _ => return Err(unsupported(&format!("audio media type {}", part.media_type))),
                                    };
                                    json!({"type":"input_audio","input_audio":{"data":encoded(data),"format":format}})
                                }
                                "application" if part.media_type == "application/pdf" => {
                                    if matches!(part.data, FileData::Url { .. }) {
                                        return Err(unsupported("PDF file parts with URLs"));
                                    }
                                    json!({"type":"file","file":{"filename":part.filename.as_deref().unwrap_or("document.pdf"),"file_data":url}})
                                }
                                "text" => {
                                    let text = match &part.data {
                                        FileData::Url { url, .. } => url.clone(),
                                        FileData::Data { data: DataContent::Bytes(bytes) } => String::from_utf8_lossy(bytes).into_owned(),
                                        FileData::Data { data: DataContent::Base64(data) } => String::from_utf8_lossy(
                                            &STANDARD.decode(data).map_err(|e| LanguageModelError::other(e.to_string()))?,
                                        )
                                        .into_owned(),
                                        _ => unreachable!(),
                                    };
                                    json!({"type":"text","text":text})
                                }
                                _ => return Err(unsupported(&format!("file part media type {}", part.media_type))),
                            };
                            (value, &part.provider_options)
                        }
                    };
                    merge(&mut value, metadata(options));
                    parts.push(value);
                }
                let mut value = json!({"role":"user","content":parts});
                merge(&mut value, message_metadata);
                messages.push(value);
            }
            Message::Assistant { content, .. } => {
                let mut text = String::new();
                let mut reasoning = String::new();
                let mut calls = vec![];
                for part in content {
                    match part {
                        AssistantPart::Text(part) => text += &part.text,
                        AssistantPart::Reasoning(part) => reasoning += &part.text,
                        AssistantPart::ToolCall(call) => {
                            let mut value = json!({"id":call.tool_call_id,"type":"function","function":{"name":call.tool_name,"arguments":stringify(&call.input)}});
                            merge(&mut value, metadata(&call.provider_options));
                            if let Some(signature) = call
                                .provider_options
                                .as_ref()
                                .and_then(|options| {
                                    options
                                        .get(key)
                                        .and_then(|p| p.get("thoughtSignature"))
                                        .or_else(|| options.get("google").and_then(|p| p.get("thoughtSignature")))
                                })
                                .filter(|v| !v.is_null() && **v != json!(false) && **v != json!(""))
                            {
                                value.as_object_mut().unwrap().insert("extra_content".into(),json!({"google":{"thought_signature":signature.as_str().map(str::to_string).unwrap_or_else(||stringify(signature))}}));
                            }
                            calls.push(value);
                        }
                        _ => {}
                    }
                }
                let mut value = json!({"role":"assistant","content":if !calls.is_empty()&&text.is_empty(){Value::Null}else{json!(text)}});
                if !reasoning.is_empty() {
                    value.as_object_mut().unwrap().insert("reasoning_content".into(), json!(reasoning));
                }
                if !calls.is_empty() {
                    value.as_object_mut().unwrap().insert("tool_calls".into(), json!(calls));
                }
                merge(&mut value, message_metadata);
                messages.push(value);
            }
            Message::Tool { content, .. } => {
                for part in content {
                    if let ToolPart::ToolResult(result) = part {
                        let words = match &result.output {
                            ToolResultOutput::Text { value, .. } | ToolResultOutput::ErrorText { value, .. } => value.clone(),
                            ToolResultOutput::ExecutionDenied { reason, .. } => {
                                reason.as_deref().unwrap_or("Tool call execution denied.").into()
                            }
                            ToolResultOutput::Json { value, .. } | ToolResultOutput::ErrorJson { value, .. } => stringify(value),
                            ToolResultOutput::Content { value, .. } => stringify(&serde_json::to_value(value).unwrap()),
                        };
                        let mut value = json!({"role":"tool","tool_call_id":result.tool_call_id,"content":words});
                        merge(&mut value, metadata(&result.provider_options));
                        messages.push(value);
                    }
                }
            }
        }
    }
    Ok(messages)
}
#[async_trait(?Send)]
impl LanguageModel for CompatibleModel {
    async fn do_stream(&self, options: CallOptions) -> Result<StreamParts, LanguageModelError> {
        let (body, warnings, key) = self.arguments(&options)?;
        let mut headers = Headers::new();
        if let Some(key) = &self.0.api_key {
            if !key.is_empty() {
                headers.insert("authorization".into(), format!("Bearer {key}"));
            }
        }
        headers.extend(self.0.headers.iter().map(|(k, v)| (k.to_lowercase(), v.clone())));
        let user_agent = headers.entry("user-agent".into()).or_default();
        if !user_agent.is_empty() {
            user_agent.push(' ');
        }
        user_agent.push_str("ai-sdk/openai-compatible/3.0.57");
        if let Some(call) = &options.headers {
            headers.extend(call.iter().map(|(k, v)| (k.to_lowercase(), v.clone())));
        }
        headers.entry("user-agent".into()).or_default().push_str(" ai-sdk/provider-utils/5.0.49 runtime/node.js/24");
        let url = format!("{}/chat/completions", self.0.base_url.trim_end_matches('/'));
        let response = post_json(self.0.fetch.as_ref(), &url, headers, body, options.abort_signal.clone()).await?;
        Ok(convert_stream(Box::pin(json_stream(response.body.unwrap())), key, warnings, options.include_raw_chunks == Some(true)))
    }
}
impl CompatibleModel {
    fn arguments(&self, call: &CallOptions) -> Result<(Value, Vec<Value>, String), LanguageModelError> {
        let mut settings = Map::new();
        let mut passthrough = Map::new();
        let mut warnings = vec![];
        let camel_name = camel(&self.0.name);
        if let Some(options) = &call.provider_options {
            for key in ["openai-compatible", "openaiCompatible", self.0.name.as_str(), camel_name.as_str()] {
                if let Some(value) = options.get(key).filter(|v| !v.is_null()) {
                    let object = value.as_object().ok_or_else(|| LanguageModelError::other(format!("invalid {key} provider options")))?;
                    for field in ["user", "reasoningEffort", "textVerbosity"] {
                        if object.get(field).is_some_and(|v| !v.is_string()) {
                            return Err(LanguageModelError::other(format!("invalid {key} provider options")));
                        }
                    }
                    if object.get("strictJsonSchema").is_some_and(|value| !value.is_boolean()) {
                        return Err(LanguageModelError::other(format!("invalid {key} provider options")));
                    }
                    settings.extend(object.clone());
                }
            }
            for key in [self.0.name.as_str(), camel_name.as_str()] {
                if let Some(object) = options.get(key).and_then(Value::as_object) {
                    for (key, value) in object {
                        if !["user", "reasoningEffort", "textVerbosity", "strictJsonSchema"].contains(&key.as_str()) {
                            passthrough.insert(key.clone(), value.clone());
                        }
                    }
                }
            }
        }
        if call.top_k.is_some() {
            warnings.push(json!({"type":"unsupported","feature":"topK"}));
        }
        let key = if camel_name != self.0.name
            && call.provider_options.as_ref().is_some_and(|options| options.get(&camel_name).is_some_and(|value| !value.is_null()))
        {
            camel_name.clone()
        } else {
            self.0.name.clone()
        };
        if call.provider_options.as_ref().is_some_and(|options| options.get("openai-compatible").is_some_and(|v| !v.is_null())) {
            warnings.insert(0, json!({"type":"deprecated","setting":"providerOptions key 'openai-compatible'","message":"Use 'openaiCompatible' instead."}));
        }
        if camel_name != self.0.name
            && call.provider_options.as_ref().is_some_and(|options| options.get(&self.0.name).is_some_and(|v| !v.is_null()))
        {
            warnings.insert(usize::from(warnings.first().is_some_and(|warning| warning["setting"] == "providerOptions key 'openai-compatible'")), json!({"type":"deprecated","setting":format!("providerOptions key '{}'",self.0.name),"message":format!("Use '{camel_name}' instead.")}));
        }
        let mut body = Map::new();
        body.insert("model".into(), json!(self.0.model));
        if let Some(user) = settings.get("user") {
            body.insert("user".into(), user.clone());
        }
        for (key, value) in [
            ("max_tokens", call.max_output_tokens),
            ("temperature", call.temperature),
            ("top_p", call.top_p),
            ("frequency_penalty", call.frequency_penalty),
            ("presence_penalty", call.presence_penalty),
        ] {
            if let Some(value) = value {
                body.insert(key.into(), json!(value));
            }
        }
        if let Some(format) = &call.response_format {
            let value = serde_json::to_value(format).unwrap();
            if value["type"] == "json" {
                if value.get("schema").is_some_and(|schema| !schema.is_null()) {
                    warnings.push(json!({"type":"unsupported","feature":"responseFormat","details":"JSON response format schema is only supported with structuredOutputs"}));
                }
                body.insert("response_format".into(), json!({"type":"json_object"}));
            }
        }
        if let Some(stop) = &call.stop_sequences {
            body.insert("stop".into(), json!(stop));
        }
        if let Some(seed) = call.seed {
            body.insert("seed".into(), json!(seed));
        }
        body.extend(passthrough);
        if let Some(effort) = settings
            .get("reasoningEffort")
            .filter(|v| !v.is_null())
            .cloned()
            .or_else(|| call.reasoning.filter(|v| *v != Reasoning::ProviderDefault).map(|v| serde_json::to_value(v).unwrap()))
        {
            body.insert("reasoning_effort".into(), effort);
        }
        if let Some(verbosity) = settings.get("textVerbosity") {
            body.insert("verbosity".into(), verbosity.clone());
        }
        body.insert("messages".into(), json!(messages(&call.prompt, &key)?));
        if let Some(tools) = call.tools.as_ref().filter(|v| !v.is_empty()) {
            body.insert(
                "tools".into(),
                Value::Array(
                    tools
                        .iter()
                        .map(|tool| {
                            let mut function = json!({"name":tool.name});
                            let map = function.as_object_mut().unwrap();
                            if let Some(description) = &tool.description {
                                map.insert("description".into(), json!(description));
                            }
                            map.insert("parameters".into(), tool.input_schema.clone());
                            if let Some(strict) = tool.strict {
                                map.insert("strict".into(), json!(strict));
                            }
                            json!({"type":"function","function":function})
                        })
                        .collect(),
                ),
            );
            if let Some(choice) = &call.tool_choice {
                body.insert(
                    "tool_choice".into(),
                    match choice {
                        ToolChoice::Auto => json!("auto"),
                        ToolChoice::None => json!("none"),
                        ToolChoice::Required => json!("required"),
                        ToolChoice::Tool { tool_name } => json!({"type":"function","function":{"name":tool_name}}),
                    },
                );
            }
        }
        body.insert("stream".into(), json!(true));
        if self.0.include_usage {
            body.insert("stream_options".into(), json!({"include_usage":true}));
        }
        Ok((Value::Object(body), warnings, key))
    }
}

// The SDK validates chunks before forwarding any text or tool arguments.
fn nullable(value: Option<&Value>, valid: impl Fn(&Value) -> bool) -> bool {
    value.is_none_or(|value| value.is_null() || valid(value))
}
fn valid_chunk(chunk: &Value) -> bool {
    let error = chunk.get("error").is_some_and(|error| {
        error.is_object()
            && error.get("message").is_some_and(Value::is_string)
            && nullable(error.get("type"), Value::is_string)
            && nullable(error.get("code"), |value| value.is_string() || value.is_number())
    });
    if error {
        return true;
    }
    if !chunk.is_object()
        || !nullable(chunk.get("id"), Value::is_string)
        || !nullable(chunk.get("model"), Value::is_string)
        || !nullable(chunk.get("created"), Value::is_number)
    {
        return false;
    }
    let Some(choices) = chunk.get("choices").and_then(Value::as_array) else {
        return false;
    };
    if !choices.iter().all(|choice| {
        choice.is_object()
            && nullable(choice.get("finish_reason"), Value::is_string)
            && nullable(choice.get("delta"), |delta| {
                delta.is_object()
                    && nullable(delta.get("role"), |role| matches!(role.as_str(), Some("assistant" | "")))
                    && nullable(delta.get("reasoning_content"), Value::is_string)
                    && nullable(delta.get("reasoning"), Value::is_string)
                    && nullable(delta.get("content"), |content| {
                        content.is_string()
                            || content.as_array().is_some_and(|parts| {
                                parts.iter().all(|part| part.is_object() && part.get("type").is_some_and(Value::is_string))
                            })
                    })
                    && nullable(delta.get("tool_calls"), |calls| {
                        calls.as_array().is_some_and(|calls| {
                            calls.iter().all(|call| {
                                call.is_object()
                                    && nullable(call.get("index"), Value::is_number)
                                    && nullable(call.get("id"), Value::is_string)
                                    && call.get("function").is_some_and(|function| {
                                        function.is_object()
                                            && nullable(function.get("name"), Value::is_string)
                                            && nullable(function.get("arguments"), Value::is_string)
                                    })
                                    && nullable(call.get("extra_content"), |extra| {
                                        extra.is_object()
                                            && nullable(extra.get("google"), |google| {
                                                google.is_object() && nullable(google.get("thought_signature"), Value::is_string)
                                            })
                                    })
                            })
                        })
                    })
            })
    }) {
        return false;
    }
    nullable(chunk.get("usage"), |usage| {
        usage.is_object()
            && ["prompt_tokens", "completion_tokens", "total_tokens"].iter().all(|key| nullable(usage.get(key), Value::is_number))
            && nullable(usage.get("prompt_tokens_details"), |details| {
                details.is_object() && nullable(details.get("cached_tokens"), Value::is_number)
            })
            && nullable(usage.get("completion_tokens_details"), |details| {
                details.is_object()
                    && ["reasoning_tokens", "accepted_prediction_tokens", "rejected_prediction_tokens"]
                        .iter()
                        .all(|key| nullable(details.get(key), Value::is_number))
            })
    })
}

type JsonStream = Pin<Box<dyn Stream<Item = Result<Value, LanguageModelError>>>>;
struct Call {
    id: String,
    name: String,
    input: String,
    metadata: Option<ProviderMetadata>,
}
struct ChatStream {
    source: JsonStream,
    key: String,
    queue: VecDeque<StreamPart>,
    ended: bool,
    text: bool,
    reasoning: bool,
    metadata_sent: bool,
    raw: bool,
    finish: Option<FinishReason>,
    usage: Option<Value>,
    calls: Vec<Call>,
    by_id: HashMap<String, usize>,
    by_index: HashMap<u64, usize>,
    latest: Option<usize>,
    pending: IndexMap<u64, Value>,
    forwarded: HashSet<u64>,
}
impl ChatStream {
    fn end_text(&mut self) {
        if self.text {
            self.text = false;
            self.queue.push_back(StreamPart::TextEnd { id: "txt-0".into(), provider_metadata: None });
        }
    }
    fn end_reasoning(&mut self) {
        if self.reasoning {
            self.reasoning = false;
            self.queue.push_back(StreamPart::ReasoningEnd { id: "reasoning-0".into(), provider_metadata: None });
        }
    }
    fn say(&mut self, reasoning: bool, text: &str) {
        if text.is_empty() {
            return;
        }
        if reasoning {
            self.end_text();
            if !self.reasoning {
                self.reasoning = true;
                self.queue.push_back(StreamPart::ReasoningStart { id: "reasoning-0".into(), provider_metadata: None });
            }
            self.queue.push_back(StreamPart::ReasoningDelta { id: "reasoning-0".into(), delta: text.into(), provider_metadata: None });
        } else {
            self.end_reasoning();
            if !self.text {
                self.text = true;
                self.queue.push_back(StreamPart::TextStart { id: "txt-0".into(), provider_metadata: None });
            }
            self.queue.push_back(StreamPart::TextDelta { id: "txt-0".into(), delta: text.into(), provider_metadata: None });
        }
    }
    fn error(&mut self, error: LanguageModelError) {
        self.finish = Some(FinishReason { unified: FinishReasonUnified::Error, raw: None });
        self.queue.push_back(StreamPart::Error { error });
    }
    fn process(&mut self, chunk: Value) -> Result<(), LanguageModelError> {
        if self.raw {
            self.queue.push_back(StreamPart::Raw { raw_value: chunk.clone() });
        }
        if !valid_chunk(&chunk) {
            return Err(LanguageModelError::other("Type validation failed: invalid OpenAI-compatible stream chunk."));
        }
        if let Some(error) = chunk.get("error") {
            // The provider's own status when its error has one (a numeric code); a server error otherwise, which is
            // worth trying again.
            let status = error
                .get("code")
                .and_then(|code| code.as_u64().or_else(|| code.as_str().and_then(|code| code.parse().ok())))
                .filter(|status| (400..=599).contains(status))
                .unwrap_or(500);
            let mut fields = serde_json::Map::new();
            fields.insert("message".into(), json!(error.get("message").and_then(Value::as_str).unwrap_or("Provider stream error")));
            fields.insert("statusCode".into(), json!(status));
            fields.insert("isRetryable".into(), json!(super::error::ApiCallError::default_retryable(Some(status as u16))));
            fields.insert("data".into(), chunk.clone());
            self.error(LanguageModelError::ProviderStream(fields));
            return Ok(());
        }
        let choices = chunk
            .get("choices")
            .and_then(Value::as_array)
            .ok_or_else(|| LanguageModelError::other("Type validation failed: choices must be an array."))?;
        if !self.metadata_sent {
            let id = chunk.get("id").and_then(Value::as_str).map(str::to_string);
            let model_id = chunk.get("model").and_then(Value::as_str).map(str::to_string);
            let timestamp = chunk.get("created").and_then(Value::as_f64).filter(|v| *v != 0.0).map(|v| (v * 1000.0) as i64);
            if id.as_ref().is_some_and(|v| !v.is_empty()) || model_id.as_ref().is_some_and(|v| !v.is_empty()) || timestamp.is_some() {
                self.metadata_sent = true;
                self.queue.push_back(StreamPart::ResponseMetadata { id, timestamp, model_id });
            }
        }
        if let Some(usage) = chunk.get("usage").filter(|v| !v.is_null()) {
            self.usage = Some(usage.clone());
        }
        let Some(choice) = choices.first() else {
            return Ok(());
        };
        if let Some(reason) = choice.get("finish_reason").and_then(Value::as_str) {
            self.finish = Some(FinishReason {
                unified: match reason {
                    "stop" => FinishReasonUnified::Stop,
                    "length" => FinishReasonUnified::Length,
                    "content_filter" => FinishReasonUnified::ContentFilter,
                    "tool_calls" | "function_call" => FinishReasonUnified::ToolCalls,
                    _ => FinishReasonUnified::Other,
                },
                raw: Some(reason.into()),
            });
        }
        let Some(delta) = choice.get("delta").filter(|v| !v.is_null()) else {
            return Ok(());
        };
        if let Some(reasoning) =
            delta.get("reasoning_content").filter(|v| !v.is_null()).or_else(|| delta.get("reasoning")).and_then(Value::as_str)
        {
            self.say(true, reasoning);
        }
        match delta.get("content") {
            Some(Value::String(text)) => self.say(false, text),
            Some(Value::Array(parts)) => {
                for part in parts {
                    match part.get("type").and_then(Value::as_str) {
                        Some("text") => {
                            if let Some(text) = part.get("text").and_then(Value::as_str) {
                                self.say(false, text);
                            }
                        }
                        Some("thinking") => {
                            if let Some(chunks) = part.get("thinking").and_then(Value::as_array) {
                                let text = chunks
                                    .iter()
                                    .filter(|v| v["type"] == "text")
                                    .filter_map(|v| v.get("text").and_then(Value::as_str))
                                    .collect::<String>();
                                self.say(true, &text);
                            }
                        }
                        _ => {}
                    }
                }
            }
            _ => {}
        }
        if let Some(calls) = delta.get("tool_calls").and_then(Value::as_array).filter(|v| !v.is_empty()) {
            self.end_reasoning();
            for call in calls {
                self.process_call(call.clone())?;
            }
        }
        Ok(())
    }
    fn process_call(&mut self, call: Value) -> Result<(), LanguageModelError> {
        let index = call.get("index").and_then(Value::as_f64).map(|n| if n == 0.0 { 0 } else { n.to_bits() });
        if let Some(index) = index.filter(|index| !self.forwarded.contains(index)) {
            let pending=self.pending.entry(index).or_insert_with(||json!({"index":call["index"],"id":call.get("id").cloned().unwrap_or(Value::Null),"function":{"arguments":""},"extra_content":call.get("extra_content").cloned().unwrap_or(Value::Null)}));
            if pending["id"].is_null() {
                pending["id"] = call.get("id").cloned().unwrap_or(Value::Null);
            }
            if pending["extra_content"].is_null() {
                pending["extra_content"] = call.get("extra_content").cloned().unwrap_or(Value::Null);
            }
            if let Some(args) = call["function"].get("arguments").and_then(Value::as_str) {
                let mut text = pending["function"]["arguments"].as_str().unwrap().to_string();
                text += args;
                pending["function"]["arguments"] = json!(text);
            }
            if let Some(name) = call["function"].get("name").filter(|v| !v.is_null()) {
                pending["function"]["name"] = name.clone();
                let pending = self.pending.shift_remove(&index).unwrap();
                self.track(pending)?;
                self.forwarded.insert(index);
            }
            return Ok(());
        }
        self.track(call)
    }
    fn track(&mut self, call: Value) -> Result<(), LanguageModelError> {
        let id = call.get("id").and_then(Value::as_str);
        let index = call.get("index").and_then(Value::as_f64).map(|n| if n == 0.0 { 0 } else { n.to_bits() });
        let found = if let Some(id) = id.filter(|id| !id.is_empty()) {
            self.by_id.get(id).copied()
        } else if let Some(index) = index {
            self.by_index.get(&index).copied()
        } else {
            self.latest
        };
        let at = if let Some(at) = found {
            if let Some(args) = call["function"].get("arguments").and_then(Value::as_str) {
                self.calls[at].input += args;
                self.queue.push_back(StreamPart::ToolInputDelta {
                    id: self.calls[at].id.clone(),
                    delta: args.into(),
                    provider_metadata: None,
                });
            }
            at
        } else {
            let id = id.ok_or_else(|| LanguageModelError::other("Expected 'id' to be a string."))?;
            let name = call["function"]
                .get("name")
                .and_then(Value::as_str)
                .ok_or_else(|| LanguageModelError::other("Expected 'function.name' to be a string."))?;
            self.queue.push_back(StreamPart::ToolInputStart {
                id: id.into(),
                tool_name: name.into(),
                provider_metadata: None,
                provider_executed: None,
                dynamic: None,
                title: None,
            });
            let input = call["function"].get("arguments").and_then(Value::as_str).unwrap_or("").to_string();
            if !input.is_empty() {
                self.queue.push_back(StreamPart::ToolInputDelta { id: id.into(), delta: input.clone(), provider_metadata: None });
            }
            let metadata = call
                .get("extra_content")
                .and_then(|v| v.get("google"))
                .and_then(|v| v.get("thought_signature"))
                .and_then(Value::as_str)
                .filter(|v| !v.is_empty())
                .map(|signature| Map::from_iter([(self.key.clone(), json!({"thoughtSignature":signature}))]));
            let at = self.calls.len();
            self.calls.push(Call { id: id.into(), name: name.into(), input, metadata });
            if !id.is_empty() {
                self.by_id.insert(id.into(), at);
            }
            at
        };
        if let Some(index) = index {
            self.by_index.insert(index, at);
        }
        self.latest = Some(at);
        Ok(())
    }
    fn flush(&mut self) -> Result<(), LanguageModelError> {
        self.end_reasoning();
        self.end_text();
        for (_, pending) in std::mem::take(&mut self.pending) {
            self.track(pending)?;
        }
        for call in &self.calls {
            self.queue.push_back(StreamPart::ToolInputEnd { id: call.id.clone(), provider_metadata: None });
            self.queue.push_back(StreamPart::ToolCall(ToolCall {
                tool_call_id: call.id.clone(),
                tool_name: call.name.clone(),
                input: call.input.clone(),
                provider_executed: None,
                dynamic: None,
                provider_metadata: call.metadata.clone(),
            }));
        }
        if self.finish.is_none() {
            self.error(LanguageModelError::other("Response stream ended without a finish reason."));
        }
        let mut metadata = Map::new();
        let usage = if let Some(raw) = &self.usage {
            let input = raw.get("prompt_tokens").and_then(Value::as_f64).unwrap_or(0.0);
            let cached = raw["prompt_tokens_details"].get("cached_tokens").and_then(Value::as_f64).unwrap_or(0.0);
            let output = raw.get("completion_tokens").and_then(Value::as_f64).unwrap_or(0.0);
            let reasoning = raw["completion_tokens_details"].get("reasoning_tokens").and_then(Value::as_f64).unwrap_or(0.0);
            for (field, key) in
                [("accepted_prediction_tokens", "acceptedPredictionTokens"), ("rejected_prediction_tokens", "rejectedPredictionTokens")]
            {
                if let Some(value) = raw["completion_tokens_details"].get(field).filter(|v| !v.is_null()) {
                    metadata.insert(key.into(), value.clone());
                }
            }
            Usage {
                input_tokens: InputTokens {
                    total: Some(input),
                    no_cache: Some(input - cached),
                    cache_read: Some(cached),
                    cache_write: None,
                },
                output_tokens: OutputTokens { total: Some(output), text: Some((output - reasoning).max(0.0)), reasoning: Some(reasoning) },
                raw: Some(raw.clone()),
            }
        } else {
            Usage::default()
        };
        self.queue.push_back(StreamPart::Finish {
            finish_reason: self.finish.clone().unwrap(),
            usage,
            provider_metadata: Some(Map::from_iter([(self.key.clone(), Value::Object(metadata))])),
        });
        Ok(())
    }
}
fn convert_stream(source: JsonStream, key: String, warnings: Vec<Value>, raw: bool) -> StreamParts {
    let state = ChatStream {
        source,
        key,
        queue: VecDeque::from([StreamPart::StreamStart { warnings }]),
        ended: false,
        text: false,
        reasoning: false,
        metadata_sent: false,
        raw,
        finish: None,
        usage: None,
        calls: vec![],
        by_id: HashMap::new(),
        by_index: HashMap::new(),
        latest: None,
        pending: IndexMap::new(),
        forwarded: HashSet::new(),
    };
    Box::pin(futures::stream::unfold(state, |mut state| async move {
        loop {
            if let Some(part) = state.queue.pop_front() {
                return Some((part, state));
            }
            if state.ended {
                return None;
            }
            match state.source.next().await {
                Some(Ok(chunk)) => {
                    if let Err(error) = state.process(chunk) {
                        state.error(error);
                    }
                }
                Some(Err(error)) => state.error(error),
                None => {
                    state.ended = true;
                    if let Err(error) = state.flush() {
                        state.error(error);
                    }
                }
            }
        }
    }))
}
