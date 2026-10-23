//! Native Messages transport for the Anthropic SDK surface used by Kumi.

use super::{
    error::{ApiCallError, LanguageModelError},
    http::{post_json, Fetch, Headers},
    sse::json_stream,
    types::*,
};
use crate::kernel::agent::LanguageModel;
use async_trait::async_trait;
use base64::{engine::general_purpose::STANDARD, Engine};
use futures::{Stream, StreamExt};
use kumi_common::js::json::stringify;
use serde_json::{json, Map, Value};
use std::{
    collections::{HashMap, VecDeque},
    pin::Pin,
    rc::Rc,
};

pub struct AnthropicSettings {
    pub model: String,
    pub base_url: String,
    pub api_key: Option<String>,
    pub auth_token: Option<String>,
    pub headers: Headers,
    pub fetch: Rc<dyn Fetch>,
}
struct AnthropicModel(AnthropicSettings);
pub fn anthropic(settings: AnthropicSettings) -> Rc<dyn LanguageModel> {
    Rc::new(AnthropicModel(settings))
}
fn field(value: &mut Value, key: &str, item: Option<Value>) {
    if let Some(item) = item {
        value.as_object_mut().unwrap().insert(key.into(), item);
    }
}
fn nonnull(value: &Value, key: &str) -> Option<Value> {
    value.get(key).filter(|v| !v.is_null()).cloned()
}
fn string(value: &Value, key: &str) -> String {
    value[key].as_str().unwrap_or("").into()
}
fn metadata(value: &Value) -> &Value {
    &value["providerOptions"]["anthropic"]
}
fn warning(feature: &str, details: impl Into<String>) -> Value {
    json!({"type":"unsupported","feature":feature,"details":details.into()})
}
fn other(message: impl Into<String>) -> Value {
    json!({"type":"other","message":message.into()})
}
fn unsupported(feature: &str) -> LanguageModelError {
    LanguageModelError::other(format!("Unsupported functionality: {feature}"))
}
#[derive(Default)]
struct Cache {
    count: usize,
    warnings: Vec<Value>,
}
impl Cache {
    fn get(&mut self, value: &Value, context: &str, can_cache: bool) -> Option<Value> {
        let meta = metadata(value);
        let cache = nonnull(meta, "cacheControl").or_else(|| nonnull(meta, "cache_control"))?;
        if cache == json!(false) || cache == json!("") {
            return None;
        }
        if !can_cache {
            self.warnings.push(warning(
                "cache_control on non-cacheable context",
                format!("cache_control cannot be set on {context}. It will be ignored."),
            ));
            return None;
        }
        self.count += 1;
        if self.count > 4 {
            self.warnings.push(warning(
                "cacheControl breakpoint limit",
                format!("Maximum 4 cache breakpoints exceeded (found {}). This breakpoint will be ignored.", self.count),
            ));
            return None;
        }
        Some(cache)
    }
}
fn bytes(data: &Value) -> Result<Vec<u8>, LanguageModelError> {
    if let Some(text) = data.as_str() {
        STANDARD.decode(text).map_err(|e| LanguageModelError::other(e.to_string()))
    } else if let Some(object) = data.as_object() {
        object
            .values()
            .map(|v| v.as_u64().filter(|v| *v < 256).map(|v| v as u8).ok_or_else(|| LanguageModelError::other("Invalid byte content")))
            .collect()
    } else {
        Err(LanguageModelError::other("Invalid byte content"))
    }
}
fn base64(data: &Value) -> Result<String, LanguageModelError> {
    Ok(if let Some(text) = data.as_str() { text.into() } else { STANDARD.encode(bytes(data)?) })
}
fn file(part: &Value) -> Result<Value, LanguageModelError> {
    let media = part["mediaType"].as_str().unwrap_or("");
    let image = media.starts_with("image/");
    let data = &part["data"];
    let source = match data["type"].as_str() {
        Some("url") => json!({"type":"url","url":data["url"]}),
        Some("reference") => {
            let id = if data["reference"].is_string() { data["reference"].clone() } else { data["reference"]["anthropic"].clone() };
            if !id.is_string() {
                return Err(unsupported("provider reference for anthropic"));
            }
            json!({"type":"file","file_id":id})
        }
        Some("text") => json!({"type":"text","media_type":"text/plain","data":data["text"]}),
        Some("data") if image || media == "application/pdf" => {
            json!({"type":"base64","media_type":if media=="image/*"{"image/jpeg"}else{media},"data":base64(&data["data"])?})
        }
        Some("data") if media == "text/plain" => {
            json!({"type":"text","media_type":"text/plain","data":String::from_utf8_lossy(&bytes(&data["data"])?)})
        }
        _ => return Err(unsupported(&format!("media type: {media}"))),
    };
    let mut result = json!({"type":if image{"image"}else{"document"},"source":source});
    if !image && data["type"] != "reference" {
        field(&mut result, "title", nonnull(metadata(part), "title").or_else(|| nonnull(part, "filename")));
        field(&mut result, "context", nonnull(metadata(part), "context").filter(|v| *v != json!("")));
        if metadata(part)["citations"]["enabled"] == true {
            result["citations"] = json!({"enabled":true});
        }
    }
    Ok(result)
}
fn reordered(parts: Vec<Value>) -> Vec<Value> {
    let mut result = vec![];
    let mut pending = vec![];
    for part in parts {
        if part["type"] == "thinking" || part["type"] == "redacted_thinking" {
            result.append(&mut pending);
            result.push(part);
        } else if part["type"] == "tool_use" {
            pending.push(part);
        } else {
            result.push(part);
        }
    }
    result.append(&mut pending);
    result
}
fn prompt(
    call: &CallOptions,
    cache: &mut Cache,
    warnings: &mut Vec<Value>,
    betas: &mut Vec<String>,
) -> Result<(Option<Value>, Vec<Value>), LanguageModelError> {
    let original = super::types::to_provider_value(&call.prompt);
    let original = original.as_array().unwrap();
    let mut groups: Vec<(String, Vec<&Value>)> = vec![];
    for message in original {
        let role = if message["role"] == "tool" { "user" } else { message["role"].as_str().unwrap() };
        if groups.last().is_some_and(|g| g.0 == role) {
            groups.last_mut().unwrap().1.push(message);
        } else {
            groups.push((role.into(), vec![message]));
        }
    }
    let mut system = None;
    let mut messages = vec![];
    for (group_index, (role, group)) in groups.iter().enumerate() {
        if role == "system" {
            let converted: Vec<Value> = group
                .iter()
                .map(|message| {
                    let mut v = json!({"type":"text","text":message["content"]});
                    field(&mut v, "cache_control", cache.get(message, "system message", true));
                    v
                })
                .collect();
            if system.is_none() {
                system = Some(json!(converted));
            } else {
                betas.push("mid-conversation-system-2026-04-07".into());
                messages.push(json!({"role":"system","content":converted}));
            }
            continue;
        }
        let mut parts = vec![];
        for (message_index, message) in group.iter().enumerate() {
            let content = message["content"].as_array().unwrap();
            for (part_index, part) in content.iter().enumerate() {
                let last = part_index + 1 == content.len();
                let kind = part["type"].as_str().unwrap_or("");
                if kind == "tool-approval-response" {
                    continue;
                }
                let mut control = cache.get(
                    part,
                    if role == "assistant" {
                        "assistant message part"
                    } else if kind == "tool-result" {
                        "tool result part"
                    } else {
                        "user message part"
                    },
                    true,
                );
                if kind == "tool-result" && control.is_none() {
                    control = cache.get(&part["output"], "tool result output", true);
                    if control.is_none() && part["output"]["type"] == "content" {
                        if let Some(p) =
                            part["output"]["value"].as_array().and_then(|v| v.iter().find(|v| v.get("providerOptions").is_some()))
                        {
                            control = cache.get(p, "tool result output", true);
                        }
                    }
                }
                if control.is_none() && last {
                    control = cache.get(
                        message,
                        if role == "assistant" {
                            "assistant message"
                        } else if kind == "tool-result" {
                            "tool result message"
                        } else {
                            "user message"
                        },
                        true,
                    );
                }
                let mut value = match kind {
                    "text" => {
                        if role == "assistant" && metadata(part)["type"] == "compaction" {
                            if part["text"] == "" {
                                continue;
                            }
                            let mut value = json!({"type":"compaction","content":part["text"]});
                            field(&mut value, "signature", nonnull(metadata(part), "signature"));
                            if value.get("signature").is_some() {
                                betas.push("compact-2026-09-04".into());
                            }
                            value
                        } else {
                            let text = part["text"].as_str().unwrap_or("");
                            let mut value = json!({"type":"text","text":if role=="assistant"&&last&&message_index+1==group.len()&&group_index+1==groups.len(){text.trim()}else{text}});
                            if role == "assistant" {
                                field(&mut value, "citations", nonnull(metadata(part), "citations"));
                            }
                            value
                        }
                    }
                    "file" => {
                        if part["data"]["type"] == "reference" {
                            betas.push("files-api-2025-04-14".into());
                        }
                        file(part)?
                    }
                    "reasoning" => {
                        if call.provider_options.as_ref().and_then(|o| o.get("anthropic")).is_some_and(|o| o["sendReasoning"] == false) {
                            warnings.push(other("sending reasoning content is disabled for this model"));
                            continue;
                        }
                        if let Some(signature) = nonnull(metadata(part), "signature") {
                            cache.get(part, "thinking block", false);
                            parts.push(json!({"type":"thinking","thinking":part["text"],"signature":signature}));
                        } else if let Some(data) = nonnull(metadata(part), "redactedData") {
                            cache.get(part, "redacted thinking block", false);
                            parts.push(json!({"type":"redacted_thinking","data":data}));
                        } else {
                            warnings.push(other("unsupported reasoning metadata"));
                        }
                        continue;
                    }
                    "tool-call" => {
                        let input =
                            if part["input"].is_object() { part["input"].clone() } else { json!({"rawInvalidInput":part["input"]}) };
                        let mut value = json!({"type":"tool_use","id":part["toolCallId"],"name":part["toolName"],"input":input});
                        if let Some(caller) = nonnull(metadata(part), "caller") {
                            let mut mapped = json!({"type":caller["type"]});
                            field(&mut mapped, "tool_id", nonnull(&caller, "toolId"));
                            value["caller"] = mapped;
                        }
                        value
                    }
                    "tool-result" => {
                        let output = &part["output"];
                        let words = match output["type"].as_str() {
                            Some("text" | "error-text") => output["value"].clone(),
                            Some("execution-denied") => nonnull(output, "reason").unwrap_or(json!("Tool call execution denied.")),
                            Some("content") => {
                                let mut converted = vec![];
                                for p in output["value"].as_array().unwrap() {
                                    match p["type"].as_str() {
                                        Some("text") => converted.push(json!({"type":"text","text":p["text"]})),
                                        Some("file")
                                            if p["data"]["type"] == "url"
                                                || p["data"]["type"] == "data"
                                                    && (p["mediaType"].as_str().unwrap_or("").starts_with("image/")
                                                        || p["mediaType"] == "application/pdf") =>
                                        {
                                            let mut v = file(p)?;
                                            v.as_object_mut().unwrap().retain(|k, _| k == "type" || k == "source");
                                            if p["data"]["type"] == "data" && p["mediaType"] == "application/pdf" {
                                                betas.push("pdfs-2024-09-25".into());
                                            }
                                            converted.push(v);
                                        }
                                        Some("custom") if metadata(p)["type"] == "tool-reference" => {
                                            converted.push(json!({"type":"tool_reference","tool_name":metadata(p)["toolName"]}))
                                        }
                                        Some("custom") => warnings.push(other("unsupported custom tool content part")),
                                        Some("file") => warnings.push(other(format!(
                                            "unsupported tool content part type: file with {} type: {}",
                                            if p["data"]["type"] == "data" { "media" } else { "data" },
                                            if p["data"]["type"] == "data" { string(p, "mediaType") } else { string(&p["data"], "type") }
                                        ))),
                                        _ => warnings.push(other(format!("unsupported tool content part type: {}", string(p, "type")))),
                                    }
                                }
                                json!(converted)
                            }
                            _ => json!(stringify(&output["value"])),
                        };
                        let mut value = json!({"type":"tool_result","tool_use_id":part["toolCallId"],"content":words});
                        if output["type"] == "error-text" || output["type"] == "error-json" {
                            value["is_error"] = json!(true);
                        }
                        value
                    }
                    _ => continue,
                };
                field(&mut value, "cache_control", control);
                parts.push(value);
            }
        }
        messages.push(json!({"role":role,"content":if role=="assistant"{reordered(parts)}else{parts}}));
    }
    Ok((system, messages))
}
#[derive(Clone, Copy)]
struct Capabilities {
    max: f64,
    structured: bool,
    known: bool,
    rejects_sampling: bool,
    rejects_forced: bool,
}
fn capabilities(model: &str) -> Capabilities {
    let has = |names: &[&str]| names.iter().any(|name| model.contains(name));
    let (max, structured, known, rejects_sampling) =
        if has(&["claude-opus-5", "claude-fable-5", "claude-opus-4-8", "claude-opus-4-7", "claude-sonnet-5"]) {
            (128000., true, true, true)
        } else if has(&["claude-sonnet-4-6", "claude-opus-4-6"]) {
            (128000., true, true, false)
        } else if has(&["claude-sonnet-4-5", "claude-opus-4-5", "claude-haiku-4-5"]) {
            (64000., true, true, false)
        } else if model.contains("claude-opus-4-1") {
            (32000., true, true, false)
        } else if has(&["claude-sonnet-4-", "claude-sonnet-4@"]) {
            (64000., false, true, false)
        } else if has(&["claude-opus-4-", "claude-opus-4@"]) {
            (32000., false, true, false)
        } else if model.contains("claude-3-haiku") {
            (4096., false, true, false)
        } else if regex::Regex::new(r"claude-(?:instant(?:-|$)|v?2(?:$|[-.:])|3(?:$|[-.]))").unwrap().is_match(model) {
            (4096., false, false, false)
        } else if model.contains("claude-") {
            (128000., true, false, true)
        } else {
            (4096., false, false, false)
        };
    Capabilities { max, structured, known, rejects_sampling, rejects_forced: has(&["claude-opus-5-5", "claude-fable-5-1"]) }
}
impl AnthropicModel {
    fn arguments(&self, call: &CallOptions) -> Result<(Value, Vec<Value>, Vec<String>), LanguageModelError> {
        let model = &self.0.model;
        let cap = capabilities(model);
        let mut warnings = vec![];
        let mut betas = vec![];
        let mut cache = Cache::default();
        for (name, value) in [("frequencyPenalty", call.frequency_penalty), ("presencePenalty", call.presence_penalty), ("seed", call.seed)]
        {
            if value.is_some() {
                warnings.push(json!({"type":"unsupported","feature":name}));
            }
        }
        let mut temperature = call.temperature;
        if let Some(t) = temperature {
            if t > 1. {
                warnings.push(warning("temperature", format!("{t} exceeds anthropic maximum of 1.0. clamped to 1.0")));
                temperature = Some(1.);
            } else if t < 0. {
                warnings.push(warning("temperature", format!("{t} is below anthropic minimum of 0. clamped to 0")));
                temperature = Some(0.);
            }
        }
        let opts = call.provider_options.as_ref().and_then(|p| p.get("anthropic")).cloned().unwrap_or(json!({}));
        if !cap.known && call.max_output_tokens.is_none() {
            warnings.push(json!({"type":"compatibility","feature":"maxOutputTokens","details":format!("The model \"{model}\" is unknown. The max output tokens have been limited to {}. Set maxOutputTokens explicitly to override this limit.",cap.max)}));
        }
        let mut top_k = call.top_k;
        let mut top_p = call.top_p;
        if cap.rejects_sampling {
            for (name, value) in [("temperature", &mut temperature), ("topK", &mut top_k), ("topP", &mut top_p)] {
                if value.take().is_some() {
                    warnings.push(warning(name, format!("{name} is not supported by {model} and will be ignored")));
                }
            }
        }
        let (system, messages) = prompt(call, &mut cache, &mut warnings, &mut betas)?;
        let max = call.max_output_tokens.unwrap_or(cap.max);
        let mut body = json!({"model":model,"max_tokens":max});
        field(&mut body, "temperature", temperature.map(|v| json!(v)));
        field(&mut body, "top_k", top_k.map(|v| json!(v)));
        field(&mut body, "top_p", top_p.map(|v| json!(v)));
        field(&mut body, "stop_sequences", call.stop_sequences.as_ref().map(|v| json!(v)));
        if let Some(thinking) = nonnull(&opts, "thinking") {
            let mut value = json!({"type":thinking["type"]});
            field(&mut value, "budget_tokens", nonnull(&thinking, "budgetTokens"));
            field(&mut value, "display", nonnull(&thinking, "display"));
            body["thinking"] = value;
        }
        if let Some(effort) = nonnull(&opts, "effort") {
            body["output_config"] = json!({"effort":effort});
        }
        for (from, to) in
            [("speed", "speed"), ("serviceTier", "service_tier"), ("inferenceGeo", "inference_geo"), ("cacheControl", "cache_control")]
        {
            field(&mut body, to, nonnull(&opts, from));
        }
        if let Some(user) = nonnull(&opts["metadata"], "userId") {
            body["metadata"] = json!({"user_id":user});
        }
        field(&mut body, "system", system);
        body["messages"] = json!(messages);
        let thinking = matches!(opts["thinking"]["type"].as_str(), Some("enabled" | "adaptive"));
        if thinking {
            let budget = if opts["thinking"]["type"] == "enabled" {
                Some(opts["thinking"]["budgetTokens"].as_f64().unwrap_or_else(||{warnings.push(json!({"type":"compatibility","feature":"extended thinking","details":"thinking budget is required when thinking is enabled. using default budget of 1024 tokens."}));body["thinking"]=json!({"type":"enabled","budget_tokens":1024});1024.}))
            } else {
                None
            };
            for (name, wire) in [("temperature", "temperature"), ("topK", "top_k"), ("topP", "top_p")] {
                if body.as_object_mut().unwrap().shift_remove(wire).is_some() {
                    warnings.push(warning(name, format!("{name} is not supported when thinking is enabled")));
                }
            }
            body["max_tokens"] = json!(max + budget.unwrap_or(0.));
        } else if (cap.known || model.contains("claude-")) && top_p.is_some() && temperature.is_some() {
            body.as_object_mut().unwrap().shift_remove("top_p");
            warnings.push(warning("topP", "topP is not supported when temperature is set. topP is ignored."));
        }
        if cap.known && body["max_tokens"].as_f64().unwrap() > cap.max {
            if call.max_output_tokens.is_some() {
                warnings.push(warning("maxOutputTokens",format!("{} (maxOutputTokens + thinkingBudget) is greater than {model} {} max output tokens. The max output tokens have been limited to {}.",body["max_tokens"].as_f64().unwrap(),cap.max,cap.max)));
            }
            body["max_tokens"] = json!(cap.max);
        }
        let mut tools = vec![];
        for tool in call.tools.as_deref().unwrap_or(&[]) {
            let v = serde_json::to_value(tool).unwrap();
            let meta = metadata(&v);
            let mut wire = json!({"name":tool.name});
            field(&mut wire, "description", tool.description.as_ref().map(|v| json!(v)));
            wire["input_schema"] = tool.input_schema.clone();
            field(&mut wire, "cache_control", cache.get(&v, "tool definition", true));
            if meta["eagerInputStreaming"].as_bool().unwrap_or(opts["toolStreaming"] != false) {
                wire["eager_input_streaming"] = json!(true);
            }
            if cap.structured {
                field(&mut wire, "strict", tool.strict.map(|v| json!(v)));
                betas.push("structured-outputs-2025-11-13".into());
            } else if let Some(strict) = tool.strict {
                warnings.push(warning("strict",format!("Tool '{}' has strict: {strict}, but strict mode is not supported by this provider. The strict property will be ignored.",tool.name)));
            }
            field(&mut wire, "defer_loading", nonnull(meta, "deferLoading"));
            field(&mut wire, "allowed_callers", nonnull(meta, "allowedCallers"));
            if let Some(examples) = &tool.input_examples {
                wire["input_examples"] = json!(examples.iter().map(|v| v["input"].clone()).collect::<Vec<_>>());
            }
            if tool.input_examples.is_some() || meta.get("allowedCallers").is_some() {
                betas.push("advanced-tool-use-2025-11-20".into());
            }
            tools.push(wire);
        }
        if !tools.is_empty() && call.tool_choice != Some(ToolChoice::None) {
            let mut choice = match &call.tool_choice {
                None if opts["disableParallelToolUse"] != true => None,
                None | Some(ToolChoice::Auto) => Some(json!({"type":"auto"})),
                Some(ToolChoice::Required) => Some(json!({"type":"any"})),
                Some(ToolChoice::Tool { tool_name }) => Some(json!({"type":"tool","name":tool_name})),
                _ => None,
            };
            if cap.rejects_forced {
                match &call.tool_choice {
                    Some(ToolChoice::Required) => {
                        warnings.push(warning("toolChoice","toolChoice 'required' is not supported by this model because it rejects forced tool use. Using 'auto' instead. Instruct the model to use a tool in the prompt and verify that a tool call was made."));
                        choice = Some(json!({"type":"auto"}));
                    }
                    Some(ToolChoice::Tool { tool_name }) => {
                        warnings.push(warning("toolChoice",format!("toolChoice 'tool' is not supported by this model because it rejects forced tool use. Only the '{tool_name}' tool is sent with 'auto' tool choice. Instruct the model to use the tool in the prompt and verify that a tool call was made.")));
                        tools.retain(|t| t["name"] == *tool_name);
                        choice = Some(json!({"type":"auto"}));
                    }
                    _ => {}
                }
            }
            if let Some(choice) = &mut choice {
                field(choice, "disable_parallel_tool_use", nonnull(&opts, "disableParallelToolUse"));
            }
            body["tools"] = json!(tools);
            field(&mut body, "tool_choice", choice);
        }
        body["stream"] = json!(true);
        warnings.append(&mut cache.warnings);
        if let Some(more) = opts["anthropicBeta"].as_array() {
            betas.extend(more.iter().filter_map(Value::as_str).map(str::to_string));
        }
        Ok((body, warnings, betas))
    }
}
#[async_trait(?Send)]
impl LanguageModel for AnthropicModel {
    async fn do_stream(&self, options: CallOptions) -> Result<StreamParts, LanguageModelError> {
        if self.0.api_key.is_some() && self.0.auth_token.is_some() {
            return Err(LanguageModelError::other("Both apiKey and authToken were provided. Please use only one authentication method."));
        }
        let (body, warnings, mut betas) = self.arguments(&options)?;
        let mut headers = Headers::from([("anthropic-version".into(), "2023-06-01".into())]);
        if let Some(key) = &self.0.api_key {
            headers.insert("x-api-key".into(), key.clone());
        }
        if let Some(token) = &self.0.auth_token {
            headers.insert("authorization".into(), format!("Bearer {token}"));
        }
        headers.extend(self.0.headers.iter().map(|(k, v)| (k.to_lowercase(), v.clone())));
        let agent = headers.entry("user-agent".into()).or_default();
        if !agent.is_empty() {
            agent.push(' ');
        }
        agent.push_str("ai-sdk/anthropic/4.0.65");
        if let Some(call) = &options.headers {
            headers.extend(call.iter().map(|(k, v)| (k.to_lowercase(), v.clone())));
        }
        for map in [&self.0.headers, options.headers.as_ref().unwrap_or(&Headers::new())] {
            if let Some(beta) = map.get("anthropic-beta") {
                betas.extend(beta.to_lowercase().split(',').map(str::trim).filter(|v| !v.is_empty()).map(str::to_string));
            }
        }
        let mut seen = std::collections::HashSet::new();
        betas.retain(|v| seen.insert(v.clone()));
        if !betas.is_empty() {
            headers.insert("anthropic-beta".into(), betas.join(","));
        }
        headers.entry("user-agent".into()).or_default().push_str(" ai-sdk/provider-utils/5.0.49 runtime/node.js/24");
        let url = format!("{}/messages", self.0.base_url.trim_end_matches('/'));
        let response = post_json(self.0.fetch.as_ref(), &url, headers, body.clone(), options.abort_signal).await?;
        let response_headers = response.headers;
        let documents = options
            .prompt
            .iter()
            .filter_map(|m| match m {
                Message::User { content, .. } => Some(content),
                _ => None,
            })
            .flatten()
            .filter_map(|p| match p {
                UserPart::File(part)
                    if matches!(part.media_type.as_str(), "application/pdf" | "text/plain")
                        && part
                            .provider_options
                            .as_ref()
                            .and_then(|v| v.get("anthropic"))
                            .is_some_and(|v| v["citations"]["enabled"] == true) =>
                {
                    Some(serde_json::to_value(part).unwrap())
                }
                _ => None,
            })
            .collect();
        let mut stream =
            convert_stream(Box::pin(json_stream(response.body.unwrap())), warnings, options.include_raw_chunks == Some(true), documents);
        let mut initial = vec![];
        for _ in 0..2 {
            if let Some(part) = stream.next().await {
                initial.push(part);
            } else {
                break;
            }
        }
        if initial.last().is_some_and(|p| matches!(p, StreamPart::Raw { .. })) {
            if let Some(part) = stream.next().await {
                initial.push(part);
            }
        }
        if let Some(StreamPart::Error { error }) = initial.last() {
            if let LanguageModelError::ProviderStream(error) = error {
                let mut api = ApiCallError::new(
                    error["message"].as_str().unwrap_or(""),
                    url,
                    Some(body),
                    Some(error.get("statusCode").and_then(Value::as_u64).unwrap_or(500) as u16),
                );
                api.response_headers = Some(response_headers);
                api.response_body = error.get("data").map(stringify);
                api.is_retryable = error.get("isRetryable").and_then(Value::as_bool).unwrap_or(false);
                return Err(api.into());
            }
            // Any other failure is returned as it came, its retryability and message kept: a body that broke off
            // after the headers is a retryable failed call (see `post_json`).
            if let Some(StreamPart::Error { error }) = initial.pop() {
                return Err(error);
            }
        }
        Ok(Box::pin(futures::stream::iter(initial).chain(stream)))
    }
}

struct Block {
    kind: String,
    id: String,
    name: String,
    input: String,
    caller: Option<Value>,
    citations: Vec<Value>,
}
struct StreamState {
    input: Pin<Box<dyn Stream<Item = Result<Value, LanguageModelError>>>>,
    pending: VecDeque<StreamPart>,
    raw: bool,
    blocks: HashMap<String, Block>,
    block_type: String,
    message_id: Option<String>,
    invalid: bool,
    usage: Value,
    raw_usage: Option<Value>,
    reason: Option<String>,
    stop_sequence: Value,
    stop_details: Option<Value>,
    input_transformations: Option<Value>,
    safeguard_results: Option<Value>,
    container: Value,
    context: Value,
    documents: Vec<Value>,
}
fn caller(value: &Value) -> Option<Value> {
    let v = value.get("caller").filter(|v| !v.is_null())?;
    let mut mapped = json!({"type":v["type"]});
    field(&mut mapped, "toolId", v.get("tool_id").cloned());
    Some(mapped)
}
fn provider_stream_error(value: &Value) -> LanguageModelError {
    let status = match value["type"].as_str() {
        Some("api_error") => Some((500, true)),
        Some("overloaded_error") => Some((529, true)),
        Some("rate_limit_error") => Some((429, true)),
        Some("request_too_large") => Some((413, false)),
        Some("authentication_error") => Some((401, false)),
        Some("permission_error") => Some((403, false)),
        Some("not_found_error") => Some((404, false)),
        Some("billing_error" | "invalid_request_error") => Some((400, false)),
        _ => None,
    };
    let mut error = json!({"message":value["message"],"type":value["type"]});
    field(&mut error, "code", nonnull(value, "code"));
    field(&mut error, "statusCode", nonnull(value, "statusCode").or_else(|| status.map(|v| json!(v.0))));
    field(&mut error, "isRetryable", nonnull(value, "isRetryable").or_else(|| status.map(|v| json!(v.1))));
    error["data"] = value.get("data").unwrap_or(value).clone();
    LanguageModelError::ProviderStream(error.as_object().unwrap().clone())
}
fn mapped_container(value: &Value, start: bool) -> Value {
    if value.is_null() {
        return Value::Null;
    }
    json!({"expiresAt":value["expires_at"],"id":value["id"],"skills":if start{Value::Null}else{value["skills"].as_array().map(|s|json!(s.iter().map(|s|json!({"type":s["type"],"skillId":s["skill_id"],"version":s["version"]})).collect::<Vec<_>>())).unwrap_or(Value::Null)}})
}
fn validate_chunk(value: &Value) -> Result<(), LanguageModelError> {
    let strings = |object: &Value, keys: &[&str]| keys.iter().all(|key| object[*key].is_string());
    let valid = match value["type"].as_str() {
        Some("ping" | "message_stop") => true,
        Some("message_start") => {
            strings(&value["message"], &["id", "model"])
                && value["message"]["usage"]["input_tokens"].is_number()
                && value["message"]["usage"]["output_tokens"].is_number()
        }
        Some("message_delta") => value["delta"].is_object() && value["usage"]["output_tokens"].is_number(),
        Some("content_block_stop") => value["index"].is_number(),
        Some("error") => strings(&value["error"], &["type", "message"]),
        Some("content_block_start") => {
            value["index"].is_number()
                && match value["content_block"]["type"].as_str() {
                    Some("text") => strings(&value["content_block"], &["text"]),
                    Some("thinking") => strings(&value["content_block"], &["thinking", "signature"]),
                    Some("redacted_thinking") => strings(&value["content_block"], &["data"]),
                    Some("tool_use") => strings(&value["content_block"], &["id", "name"]),
                    Some("compaction" | "fallback") => true,
                    _ => false,
                }
        }
        Some("content_block_delta") => {
            value["index"].is_number()
                && match value["delta"]["type"].as_str() {
                    Some("text_delta") => strings(&value["delta"], &["text"]),
                    Some("thinking_delta") => strings(&value["delta"], &["thinking"]),
                    Some("signature_delta") => strings(&value["delta"], &["signature"]),
                    Some("input_json_delta") => strings(&value["delta"], &["partial_json"]),
                    Some("compaction_delta") => value["delta"]["content"].is_null() || value["delta"]["content"].is_string(),
                    Some("citations_delta") => value["delta"]["citation"].is_object(),
                    _ => false,
                }
        }
        _ => false,
    };
    if valid {
        Ok(())
    } else {
        Err(LanguageModelError::other(format!("Invalid Anthropic response data: {}", stringify(value))))
    }
}
fn usage(value: &Value, raw: Option<Value>) -> Usage {
    let number = |v: &Value, key: &str| v[key].as_f64().unwrap_or(0.);
    let cache_write = number(value, "cache_creation_input_tokens");
    let cache_read = number(value, "cache_read_input_tokens");
    let mut input = number(value, "input_tokens");
    let mut output = number(value, "output_tokens");
    if let Some(iterations) = value["iterations"].as_array() {
        if !iterations.iter().any(|v| v["type"] == "fallback_message") {
            let executor: Vec<_> = iterations.iter().filter(|v| v["type"] == "compaction" || v["type"] == "message").collect();
            if !executor.is_empty() {
                input = executor.iter().map(|v| number(v, "input_tokens")).sum();
                output = executor.iter().map(|v| number(v, "output_tokens")).sum();
            }
        }
    }
    let reasoning = value["output_tokens_details"]["thinking_tokens"].as_f64();
    Usage {
        input_tokens: InputTokens {
            total: Some(input + cache_write + cache_read),
            no_cache: Some(input),
            cache_read: Some(cache_read),
            cache_write: Some(cache_write),
        },
        output_tokens: OutputTokens { total: Some(output), text: reasoning.map(|r| output - r), reasoning },
        raw: Some(raw.unwrap_or_else(|| value.clone())),
    }
}
impl StreamState {
    fn emit(&mut self, value: Value) {
        match serde_json::from_value(value) {
            Ok(part) => self.pending.push_back(part),
            Err(error) => self
                .pending
                .push_back(StreamPart::Error { error: LanguageModelError::other(format!("Invalid Anthropic stream part: {error}")) }),
        }
    }
    fn tool(&mut self, id: &str, name: &str, input: &str, caller: Option<Value>) {
        let mut value = json!({"type":"tool-call","toolCallId":id,"toolName":name,"input":input});
        if let Some(caller) = caller {
            value["providerMetadata"] = json!({"anthropic":{"caller":caller}});
        }
        self.emit(value);
    }
    fn accept(&mut self, value: Value) {
        if self.invalid {
            return;
        }
        if self.raw {
            self.pending.push_back(StreamPart::Raw { raw_value: value.clone() });
        }
        if let Err(error) = validate_chunk(&value) {
            self.pending.push_back(StreamPart::Error { error });
            return;
        }
        let kind = value["type"].as_str().unwrap_or("");
        let index = value["index"].as_u64().unwrap_or(0).to_string();
        match kind {
            "ping" => {}
            "message_start" => {
                let message = &value["message"];
                let id = string(message, "id");
                if let Some(active) = &self.message_id {
                    if active == &id {
                        return;
                    }
                    self.invalid = true;
                    self.pending.push_back(StreamPart::Error {
                        error: LanguageModelError::other(format!(
                            "Received message_start for message {} while message {} is still open.",
                            stringify(&json!(id)),
                            stringify(&json!(active))
                        )),
                    });
                    return;
                }
                self.message_id = Some(id.clone());
                self.usage["input_tokens"] = message["usage"]["input_tokens"].clone();
                for key in ["cache_read_input_tokens", "cache_creation_input_tokens"] {
                    self.usage[key] = nonnull(&message["usage"], key).unwrap_or(json!(0));
                }
                self.raw_usage = Some(message["usage"].clone());
                self.input_transformations = nonnull(message, "input_transformations").or(self.input_transformations.take());
                if !message["container"].is_null() {
                    self.container = mapped_container(&message["container"], true);
                }
                if let Some(reason) = message["stop_reason"].as_str() {
                    self.reason = Some(reason.into());
                }
                self.emit(json!({"type":"response-metadata","id":id,"modelId":message["model"]}));
                if let Some(parts) = message["content"].as_array() {
                    for part in parts {
                        if part["type"] == "tool_use" {
                            let id = string(part, "id");
                            let name = string(part, "name");
                            let input = stringify(&nonnull(part, "input").unwrap_or(json!({})));
                            self.emit(json!({"type":"tool-input-start","id":id,"toolName":name}));
                            self.emit(json!({"type":"tool-input-delta","id":id,"delta":input}));
                            self.emit(json!({"type":"tool-input-end","id":id}));
                            self.tool(&id, &name, &input, caller(part));
                        }
                    }
                }
            }
            "content_block_start" => {
                let part = &value["content_block"];
                let kind = string(part, "type");
                if kind == "fallback" {
                    return;
                }
                self.block_type = kind.clone();
                let block = Block {
                    kind: kind.clone(),
                    id: string(part, "id"),
                    name: string(part, "name"),
                    input: if part["input"].as_object().is_some_and(|v| !v.is_empty()) { stringify(&part["input"]) } else { String::new() },
                    caller: caller(part),
                    citations: vec![],
                };
                match kind.as_str() {
                    "text" => self.emit(json!({"type":"text-start","id":index})),
                    "thinking" => self.emit(json!({"type":"reasoning-start","id":index})),
                    "redacted_thinking" => self
                        .emit(json!({"type":"reasoning-start","id":index,"providerMetadata":{"anthropic":{"redactedData":part["data"]}}})),
                    "compaction" => {
                        let mut meta = json!({"type":"compaction"});
                        field(&mut meta, "signature", nonnull(part, "signature"));
                        self.emit(json!({"type":"text-start","id":index,"providerMetadata":{"anthropic":meta}}));
                        if part.get("signature").is_some_and(|v| !v.is_null()) && part["content"].as_str().is_some_and(|s| !s.is_empty()) {
                            self.emit(json!({"type":"text-delta","id":index,"delta":part["content"]}));
                        }
                    }
                    "tool_use" => self.emit(json!({"type":"tool-input-start","id":block.id,"toolName":block.name})),
                    _ => return,
                }
                self.blocks.insert(index, block);
            }
            "content_block_delta" => {
                let delta = &value["delta"];
                match delta["type"].as_str(){
                    Some("text_delta")=>self.emit(json!({"type":"text-delta","id":index,"delta":delta["text"]})),
                    Some("thinking_delta")=>self.emit(json!({"type":"reasoning-delta","id":index,"delta":delta["thinking"]})),
                    Some("signature_delta") if self.block_type=="thinking"=>self.emit(json!({"type":"reasoning-delta","id":index,"delta":"","providerMetadata":{"anthropic":{"signature":delta["signature"]}}})),
                    Some("compaction_delta") if !delta["content"].is_null()=>self.emit(json!({"type":"text-delta","id":index,"delta":delta["content"]})),
                    Some("input_json_delta")=>{
                        let delta=string(delta,"partial_json");if delta.is_empty(){return;}
                        if let Some(block)=self.blocks.get_mut(&index).filter(|b|b.kind=="tool_use"){block.input+=&delta;let id=block.id.clone();self.emit(json!({"type":"tool-input-delta","id":id,"delta":delta}));}
                    },
                    Some("citations_delta")=>{
                        let citation=&delta["citation"];
                        if citation["type"]=="web_search_result_location"{
                            if let Some(block)=self.blocks.get_mut(&index){block.citations.push(citation.clone());}
                            let mut source=json!({"type":"source","sourceType":"url","id":super::generate_id(),"url":citation["url"]});field(&mut source,"title",nonnull(citation,"title"));
                            source["providerMetadata"]=json!({"anthropic":{"citedText":citation["cited_text"],"encryptedIndex":citation["encrypted_index"]}});self.emit(source);
                        }else if matches!(citation["type"].as_str(),Some("page_location"|"char_location")){
                            if let Some(document)=citation["document_index"].as_u64().and_then(|i|self.documents.get(i as usize)){
                                let mut source=json!({"type":"source","sourceType":"document","id":super::generate_id(),"mediaType":document["mediaType"],"title":nonnull(citation,"document_title").or_else(||nonnull(document,"filename")).unwrap_or(json!("Untitled Document"))});field(&mut source,"filename",nonnull(document,"filename"));
                                let mut metadata=json!({"citedText":citation["cited_text"]});
                                for(from,to)in if citation["type"]=="page_location"{[("start_page_number","startPageNumber"),("end_page_number","endPageNumber")]}else{[("start_char_index","startCharIndex"),("end_char_index","endCharIndex")]}{metadata[to]=citation[from].clone();}
                                source["providerMetadata"]=json!({"anthropic":metadata});self.emit(source);
                            }
                        }
                    },
                    _=>{},
                }
            }
            "content_block_stop" => {
                if let Some(block) = self.blocks.remove(&index) {
                    match block.kind.as_str() {
                        "text" | "compaction" => {
                            let mut part = json!({"type":"text-end","id":index});
                            if !block.citations.is_empty() {
                                part["providerMetadata"] = json!({"anthropic":{"citations":block.citations}});
                            }
                            self.emit(part);
                        }
                        "thinking" | "redacted_thinking" => self.emit(json!({"type":"reasoning-end","id":index})),
                        "tool_use" => {
                            self.emit(json!({"type":"tool-input-end","id":block.id}));
                            self.tool(&block.id, &block.name, if block.input.is_empty() { "{}" } else { &block.input }, block.caller);
                        }
                        _ => {}
                    }
                }
                self.block_type.clear();
            }
            "message_delta" => {
                let delta = &value["delta"];
                for key in [
                    "input_tokens",
                    "output_tokens",
                    "output_tokens_details",
                    "cache_read_input_tokens",
                    "cache_creation_input_tokens",
                    "iterations",
                ] {
                    if let Some(v) = nonnull(&value["usage"], key) {
                        self.usage[key] = v;
                    }
                }
                self.reason = delta["stop_reason"].as_str().map(str::to_string);
                self.stop_sequence = delta["stop_sequence"].clone();
                self.stop_details = nonnull(delta, "stop_details").map(|d| {
                    let mut value = json!({"type":d["type"]});
                    for (from, to) in [("category", "category"), ("explanation", "explanation"), ("recommended_model", "recommendedModel")]
                    {
                        field(&mut value, to, nonnull(&d, from));
                    }
                    value
                });
                self.container = mapped_container(&delta["container"], false);
                if let Some(context) = value["context_management"].as_object() {
                    if let Some(edits) = context.get("applied_edits").and_then(Value::as_array) {
                        self.context = json!({"appliedEdits":edits.iter().filter_map(|edit|{let mut result=json!({"type":edit["type"]});match edit["type"].as_str(){Some("clear_tool_uses_20250919")=>{result["clearedToolUses"]=edit["cleared_tool_uses"].clone();result["clearedInputTokens"]=edit["cleared_input_tokens"].clone();},Some("clear_thinking_20251015")=>{result["clearedThinkingTurns"]=edit["cleared_thinking_turns"].clone();result["clearedInputTokens"]=edit["cleared_input_tokens"].clone();},Some("compact_20260112")=>{},_=>return None}Some(result)}).collect::<Vec<_>>()});
                    }
                }
                self.input_transformations = nonnull(&value, "input_transformations").or(self.input_transformations.take());
                self.safeguard_results = nonnull(delta, "safeguard_results").or(self.safeguard_results.take());
                let raw = self.raw_usage.get_or_insert(json!({}));
                if let Some(more) = value["usage"].as_object() {
                    raw.as_object_mut().unwrap().extend(more.clone());
                }
            }
            "message_stop" => {
                self.message_id = None;
                let mut meta = json!({"usage":self.raw_usage,"stopSequence":self.stop_sequence});
                field(&mut meta, "stopDetails", self.stop_details.clone());
                field(&mut meta, "inputTransformations", self.input_transformations.clone());
                field(&mut meta, "safeguardResults", self.safeguard_results.clone());
                meta["iterations"] = self.usage["iterations"]
                    .as_array()
                    .map(|list| {
                        json!(list
                            .iter()
                            .map(|v| {
                                let mut i = json!({"type":v["type"]});
                                field(&mut i, "model", nonnull(v, "model"));
                                i["inputTokens"] = v["input_tokens"].clone();
                                i["outputTokens"] = v["output_tokens"].clone();
                                for (from, to) in [
                                    ("cache_creation_input_tokens", "cacheCreationInputTokens"),
                                    ("cache_read_input_tokens", "cacheReadInputTokens"),
                                ] {
                                    field(&mut i, to, nonnull(v, from).filter(|v| v.as_f64() != Some(0.)));
                                }
                                i
                            })
                            .collect::<Vec<_>>())
                    })
                    .unwrap_or(Value::Null);
                meta["container"] = self.container.clone();
                meta["contextManagement"] = self.context.clone();
                let unified = match self.reason.as_deref() {
                    Some("end_turn" | "stop_sequence" | "pause_turn") => FinishReasonUnified::Stop,
                    Some("refusal") => FinishReasonUnified::ContentFilter,
                    Some("tool_use") => FinishReasonUnified::ToolCalls,
                    Some("max_tokens" | "model_context_window_exceeded") => FinishReasonUnified::Length,
                    _ => FinishReasonUnified::Other,
                };
                self.pending.push_back(StreamPart::Finish {
                    usage: usage(&self.usage, self.raw_usage.clone()),
                    finish_reason: FinishReason { unified, raw: self.reason.clone() },
                    provider_metadata: Some(Map::from_iter([("anthropic".into(), meta)])),
                });
            }
            "error" => self.pending.push_back(StreamPart::Error { error: provider_stream_error(&value["error"]) }),
            _ => self.pending.push_back(StreamPart::Error { error: LanguageModelError::other(format!("Unsupported chunk type: {kind}")) }),
        }
    }
}
fn convert_stream(
    input: Pin<Box<dyn Stream<Item = Result<Value, LanguageModelError>>>>,
    warnings: Vec<Value>,
    raw: bool,
    documents: Vec<Value>,
) -> StreamParts {
    let state = StreamState {
        input,
        pending: VecDeque::from([StreamPart::StreamStart { warnings }]),
        raw,
        blocks: HashMap::new(),
        block_type: String::new(),
        message_id: None,
        invalid: false,
        usage: json!({"input_tokens":0,"output_tokens":0,"cache_creation_input_tokens":0,"cache_read_input_tokens":0,"iterations":null}),
        raw_usage: None,
        reason: None,
        stop_sequence: Value::Null,
        stop_details: None,
        input_transformations: None,
        safeguard_results: None,
        container: Value::Null,
        context: Value::Null,
        documents,
    };
    Box::pin(futures::stream::unfold(state, |mut state| async move {
        loop {
            if let Some(part) = state.pending.pop_front() {
                return Some((part, state));
            }
            match state.input.next().await {
                Some(Ok(value)) => state.accept(value),
                Some(Err(error)) => state.pending.push_back(StreamPart::Error { error }),
                None => return None,
            }
        }
    }))
}
