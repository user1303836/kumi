//! Native Responses transport for the OpenAI SDK surface used by Kumi.
use super::{
    error::{ApiCallError, LanguageModelError},
    http::{post_json, Fetch, Headers},
    sse::{json_stream, safe_json},
    types::*,
};
use crate::kernel::agent::LanguageModel;
use async_trait::async_trait;
use base64::{engine::general_purpose::STANDARD, Engine};
use futures::{Stream, StreamExt};
use kumi_common::js::json::stringify;
use serde_json::{json, Map, Value};
use std::{
    collections::{BTreeMap, HashMap, VecDeque},
    pin::Pin,
    rc::Rc,
    time::Duration,
};

pub struct ResponsesSettings {
    pub model: String,
    pub base_url: String,
    pub api_key: String,
    pub headers: Headers,
    pub fetch: Rc<dyn Fetch>,
}
struct ResponsesModel(ResponsesSettings);
pub fn openai_responses(settings: ResponsesSettings) -> Rc<dyn LanguageModel> {
    Rc::new(ResponsesModel(settings))
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
fn meta(value: &Value) -> &Value {
    &value["providerOptions"]["openai"]
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
struct Capabilities {
    reasoning: bool,
    gpt6: bool,
    nonreasoning: bool,
    flex: bool,
    priority: bool,
    efforts: Option<Vec<&'static str>>,
}
fn capabilities(model: &str) -> Capabilities {
    let gpt = regex::Regex::new(r"^gpt-(\d+)(?:\.(\d+))?(?:-(.+))?$").unwrap();
    let gpt = gpt.captures(model);
    let major = gpt.as_ref().and_then(|m| m[1].parse::<u32>().ok()).unwrap_or(0);
    let minor = gpt.as_ref().and_then(|m| m.get(2)).and_then(|m| m.as_str().parse::<u32>().ok());
    let variant = gpt.as_ref().and_then(|m| m.get(3)).map(|m| m.as_str()).unwrap_or("");
    let chat = minor.is_none() && variant.starts_with("chat");
    let o = regex::Regex::new(r"^o(\d+)(?:-|$)").unwrap();
    let o = o.captures(model).and_then(|m| m[1].parse::<u32>().ok());
    let gpt6 = major >= 6;
    Capabilities {
        reasoning: o.is_some() || major >= 5 && !chat,
        gpt6,
        nonreasoning: major == 5 && minor.unwrap_or(0) >= 1,
        flex: o.is_some_and(|v| v >= 3) || major >= 5 && !chat,
        priority: model.starts_with("gpt-4") || major >= 5 && !variant.starts_with("nano") && !chat || o.is_some_and(|v| v >= 3),
        efforts: if gpt6 {
            Some(if matches!(model, "gpt-6-sol" | "gpt-6-luna") {
                vec!["none", "low", "medium", "high", "xhigh", "max"]
            } else {
                vec!["low", "medium", "high", "xhigh", "max"]
            })
        } else {
            None
        },
    }
}
fn lookaround(pattern: &str) -> bool {
    let bytes = pattern.as_bytes();
    let mut escaped = false;
    let mut class = false;
    for (i, &b) in bytes.iter().enumerate() {
        if escaped {
            escaped = false;
            continue;
        }
        match b {
            b'\\' => escaped = true,
            b'[' => class = true,
            b']' => class = false,
            b'(' if !class => {
                if bytes.get(i + 1) == Some(&b'?')
                    && (matches!(bytes.get(i + 2), Some(b'=' | b'!'))
                        || bytes.get(i + 2) == Some(&b'<') && matches!(bytes.get(i + 3), Some(b'=' | b'!')))
                {
                    return true;
                }
            }
            _ => {}
        }
    }
    false
}
fn schema(value: &Value, warnings: &mut Vec<Value>) -> Result<Value, LanguageModelError> {
    fn convert(value: &Value, names: &mut bool, patterns: &mut bool) -> Result<Value, LanguageModelError> {
        let Some(original) = value.as_object() else {
            return Ok(value.clone());
        };
        let mut result = original.clone();
        if let Some(property) = nonnull(value, "propertyNames") {
            if property.is_boolean() || property["type"] != "string" {
                return Err(unsupported("JSON Schema propertyNames that does not use a string schema"));
            }
            *names = true;
        }
        result.shift_remove("propertyNames");
        if result.get("pattern").and_then(Value::as_str).is_some_and(lookaround) {
            result.shift_remove("pattern");
            *patterns = true;
        }
        for key in ["properties", "patternProperties", "definitions", "$defs", "dependencies"] {
            if let Some(Value::Object(map)) = result.get_mut(key) {
                for value in map.values_mut() {
                    if !value.is_array() {
                        *value = convert(value, names, patterns)?;
                    }
                }
            }
        }
        for key in ["additionalProperties", "additionalItems", "items", "contains", "not", "allOf", "anyOf", "oneOf", "if", "then", "else"]
        {
            if let Some(value) = result.get_mut(key) {
                if let Some(array) = value.as_array_mut() {
                    for v in array {
                        *v = convert(v, names, patterns)?;
                    }
                } else {
                    *value = convert(value, names, patterns)?;
                }
            }
        }
        Ok(Value::Object(result))
    }
    let mut names = false;
    let mut patterns = false;
    let result = convert(value, &mut names, &mut patterns)?;
    if names {
        warnings.push(json!({"type":"compatibility","feature":"JSON Schema propertyNames","details":"OpenAI does not support JSON Schema propertyNames. It was removed before sending the schema, so OpenAI will not enforce property-name constraints."}));
    }
    if patterns {
        warnings.push(json!({"type":"compatibility","feature":"JSON Schema pattern with regex lookaround","details":"OpenAI does not support regex lookaround in JSON Schema patterns. The pattern was removed before sending the schema, so OpenAI will not enforce that constraint."}));
    }
    Ok(result)
}
fn encoded(data: &Value) -> Result<String, LanguageModelError> {
    if let Some(text) = data.as_str() {
        return Ok(text.into());
    }
    let Some(object) = data.as_object() else {
        return Err(LanguageModelError::other("Invalid byte content"));
    };
    let bytes: Option<Vec<u8>> = object.values().map(|v| v.as_u64().filter(|v| *v < 256).map(|v| v as u8)).collect();
    Ok(STANDARD.encode(bytes.ok_or_else(|| LanguageModelError::other("Invalid byte content"))?))
}
fn cache(value: &mut Value, source: &Value) {
    field(value, "prompt_cache_breakpoint", nonnull(meta(source), "promptCacheBreakpoint"));
}
fn content(part: &Value, index: usize, tool: bool, passthrough: bool) -> Result<Value, LanguageModelError> {
    if part["type"] == "text" {
        let mut value = json!({"type":"input_text","text":part["text"]});
        cache(&mut value, part);
        return Ok(value);
    }
    let media = part["mediaType"].as_str().unwrap_or("");
    let image = media.starts_with("image/");
    let data = &part["data"];
    let mut value = json!({"type":if image{"input_image"}else{"input_file"}});
    match data["type"].as_str() {
        Some("reference") => {
            let reference = if data["reference"].is_string() { data["reference"].clone() } else { data["reference"]["openai"].clone() };
            if !reference.is_string() {
                return Err(unsupported("provider reference for openai"));
            }
            value["file_id"] = reference;
        }
        Some("url") => value[if image { "image_url" } else { "file_url" }] = data["url"].clone(),
        Some("data") => {
            let media = if media == "image/*" { "image/jpeg" } else { media };
            if !image && !tool && !passthrough && media != "application/pdf" {
                return Err(unsupported(&format!("file part media type {media}")));
            }
            if !tool && data["data"].as_str().is_some_and(|s| s.starts_with("file-")) {
                value["file_id"] = data["data"].clone();
            } else {
                let bytes = format!("data:{media};base64,{}", encoded(&data["data"])?);
                if image {
                    value["image_url"] = json!(bytes);
                } else {
                    value["filename"] = nonnull(part, "filename").unwrap_or_else(|| {
                        json!(if tool {
                            "data".into()
                        } else if media == "application/pdf" {
                            format!("part-{index}.pdf")
                        } else {
                            format!("part-{index}")
                        })
                    });
                    value["file_data"] = json!(bytes);
                }
            }
        }
        Some("text") => return Err(unsupported("text file parts")),
        _ => return Err(unsupported("file data type")),
    }
    if image {
        field(&mut value, "detail", nonnull(meta(part), "imageDetail"));
    }
    cache(&mut value, part);
    Ok(value)
}
fn caller(value: &Value) -> Option<Value> {
    let caller = nonnull(value, "caller")?;
    Some(if caller["type"] == "program" { json!({"type":"program","caller_id":caller["callerId"]}) } else { caller })
}
fn prompt(call: &CallOptions, opts: &Value, reasoning: bool, warnings: &mut Vec<Value>) -> Result<Vec<Value>, LanguageModelError> {
    let messages = super::types::to_provider_value(&call.prompt);
    let mut input = vec![];
    let store = opts["store"] != false;
    let conversation = !opts["conversation"].is_null();
    let previous = !opts["previousResponseId"].is_null();
    for message in messages.as_array().unwrap() {
        match message["role"].as_str() {
            Some("system") => {
                let mode = opts["systemMessageMode"].as_str().unwrap_or(if reasoning { "developer" } else { "system" });
                if mode == "remove" {
                    warnings.push(other("system messages are removed for this model"));
                    continue;
                }
                let mut words = message["content"].clone();
                if let Some(control) = nonnull(meta(message), "promptCacheBreakpoint") {
                    words = json!([{"type":"input_text","text":words,"prompt_cache_breakpoint":control}]);
                }
                input.push(json!({"role":mode,"content":words}));
            }
            Some("user") => {
                let parts = message["content"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .enumerate()
                    .map(|(i, p)| content(p, i, false, opts["passThroughUnsupportedFiles"] == true))
                    .collect::<Result<Vec<_>, _>>()?;
                input.push(json!({"role":"user","content":parts}));
            }
            Some("assistant") => {
                let mut reasoning_items: HashMap<String, usize> = HashMap::new();
                for part in message["content"].as_array().unwrap() {
                    let options = meta(part);
                    let id = nonnull(options, "itemId");
                    match part["type"].as_str() {
                        Some("text") => {
                            if conversation && id.is_some() {
                                continue;
                            }
                            if store && id.is_some() {
                                input.push(json!({"type":"item_reference","id":id}));
                                continue;
                            }
                            let mut value = json!({"role":"assistant","content":part["text"]});
                            field(&mut value, "phase", nonnull(options, "phase"));
                            input.push(value);
                        }
                        Some("tool-call") => {
                            if conversation && id.is_some() {
                                continue;
                            }
                            if part["providerExecuted"] == true {
                                if store && id.is_some() {
                                    input.push(json!({"type":"item_reference","id":id}));
                                }
                                continue;
                            }
                            let mut value = json!({"type":"function_call","call_id":part["toolCallId"],"name":part["toolName"],"arguments":stringify(&part["input"])});
                            field(&mut value, "async", nonnull(options, "async"));
                            field(&mut value, "namespace", nonnull(options, "namespace"));
                            field(&mut value, "caller", caller(options));
                            input.push(value);
                        }
                        Some("reasoning") => {
                            if (conversation || previous) && id.is_some() {
                                continue;
                            }
                            let encrypted = nonnull(options, "reasoningEncryptedContent");
                            let text = part["text"].as_str().unwrap_or("");
                            let summary = if text.is_empty() { vec![] } else { vec![json!({"type":"summary_text","text":text})] };
                            if let Some(id) = id {
                                let key = id.as_str().unwrap_or("").to_string();
                                let existing = reasoning_items.get(&key).copied();
                                if store {
                                    if existing.is_none() {
                                        reasoning_items.insert(key, input.len());
                                        input.push(json!({"type":"item_reference","id":id}));
                                    }
                                } else if let Some(index) = existing {
                                    if text.is_empty() {
                                        warnings.push(other(format!("Cannot append empty reasoning part to existing reasoning sequence. Skipping reasoning part: {}.",stringify(part))));
                                    }
                                    input[index]["summary"].as_array_mut().unwrap().extend(summary);
                                    field(&mut input[index], "encrypted_content", encrypted);
                                } else {
                                    let mut value = json!({"type":"reasoning","id":id});
                                    field(&mut value, "encrypted_content", encrypted);
                                    value["summary"] = json!(summary);
                                    reasoning_items.insert(key, input.len());
                                    input.push(value);
                                }
                            } else if let Some(encrypted) = encrypted {
                                input.push(json!({"type":"reasoning","encrypted_content":encrypted,"summary":summary}));
                            } else {
                                warnings.push(other(format!(
                                    "Non-OpenAI reasoning parts are not supported. Skipping reasoning part: {}.",
                                    stringify(part)
                                )));
                            }
                        }
                        Some("custom") if part["kind"] == "openai.compaction" => {
                            if conversation && id.is_some() {
                                continue;
                            }
                            if let Some(id) = id {
                                if store {
                                    input.push(json!({"type":"item_reference","id":id}));
                                } else {
                                    let mut value = json!({"type":"compaction","id":id});
                                    field(&mut value, "encrypted_content", nonnull(options, "encryptedContent"));
                                    input.push(value);
                                }
                            }
                        }
                        Some("tool-result") => {
                            if conversation || part["output"]["type"] == "execution-denied" {
                                continue;
                            }
                            if store {
                                input.push(json!({"type":"item_reference","id":id.unwrap_or(part["toolCallId"].clone())}));
                            } else {
                                warnings.push(other(format!(
                                    "Results for OpenAI tool {} are not sent to the API when store is false",
                                    string(part, "toolName")
                                )));
                            }
                        }
                        _ => {}
                    }
                }
            }
            Some("tool") => {
                for part in message["content"].as_array().unwrap() {
                    if part["type"] != "tool-result" {
                        continue;
                    }
                    let output = &part["output"];
                    let words = match output["type"].as_str() {
                        Some("content") => {
                            let mut parts = vec![];
                            for (i, item) in output["value"].as_array().unwrap().iter().enumerate() {
                                if item["type"] == "text" || item["type"] == "file" && item["data"]["type"] != "text" {
                                    parts.push(content(item, i, true, true)?);
                                } else {
                                    warnings.push(other(if item["type"] == "file" {
                                        format!(
                                            "unsupported tool content part type: file with data type: {}",
                                            string(&item["data"], "type")
                                        )
                                    } else {
                                        format!("unsupported tool content part type: {}", string(item, "type"))
                                    }));
                                }
                            }
                            json!(parts)
                        }
                        _ => {
                            let text = match output["type"].as_str() {
                                Some("text" | "error-text") => output["value"].clone(),
                                Some("execution-denied") => nonnull(output, "reason").unwrap_or(json!("Tool call execution denied.")),
                                _ => json!(stringify(&output["value"])),
                            };
                            let control =
                                nonnull(meta(output), "promptCacheBreakpoint").or_else(|| nonnull(meta(part), "promptCacheBreakpoint"));
                            if let Some(control) = control {
                                json!([{"type":"input_text","text":text,"prompt_cache_breakpoint":control}])
                            } else {
                                text
                            }
                        }
                    };
                    let mut value = json!({"type":"function_call_output","call_id":part["toolCallId"],"output":words});
                    field(&mut value, "caller", caller(meta(part)));
                    input.push(value);
                }
            }
            _ => {}
        }
    }
    if !store && input.iter().any(|v| v["type"] == "reasoning" && v["encrypted_content"].is_null()) {
        warnings.push(other("Reasoning parts without encrypted content are not supported when store is false. Skipping reasoning parts."));
        input.retain(|v| v["type"] != "reasoning" || !v["encrypted_content"].is_null());
    }
    Ok(input)
}
impl ResponsesModel {
    fn arguments(&self, call: &CallOptions) -> Result<(Value, Vec<Value>, Value), LanguageModelError> {
        let model = &self.0.model;
        let cap = capabilities(model);
        let opts = call.provider_options.as_ref().and_then(|v| v.get("openai")).cloned().unwrap_or(json!({}));
        let mut warnings = vec![];
        for (name, present) in [
            ("topK", call.top_k.is_some()),
            ("seed", call.seed.is_some()),
            ("presencePenalty", call.presence_penalty.is_some()),
            ("frequencyPenalty", call.frequency_penalty.is_some()),
            ("stopSequences", call.stop_sequences.is_some()),
        ] {
            if present {
                warnings.push(json!({"type":"unsupported","feature":name}));
            }
        }
        let mut effort =
            nonnull(&opts, "reasoningEffort").or_else(|| call.reasoning.filter(|r| *r != Reasoning::ProviderDefault).map(|r| json!(r)));
        if let (Some(effort_value), Some(efforts)) = (&effort, &cap.efforts) {
            if !efforts.contains(&effort_value.as_str().unwrap_or("")) {
                warnings.push(warning(
                    "reasoningEffort",
                    format!("{model} only supports the following reasoning efforts: {}", efforts.join(", ")),
                ));
                effort = None;
            }
        }
        let summary =
            opts.get("reasoningSummary").cloned().or_else(|| effort.as_ref().filter(|e| **e != json!("none")).map(|_| json!("detailed")));
        let reasoning = opts["forceReasoning"].as_bool().unwrap_or(cap.reasoning);
        if !opts["conversation"].is_null() && !opts["previousResponseId"].is_null() {
            warnings.push(warning("conversation", "conversation and previousResponseId cannot be used together"));
        }
        let mut input = prompt(call, &opts, reasoning, &mut warnings)?;
        if opts["compactionTrigger"] == true {
            input.push(json!({"type":"compaction_trigger"}));
        }
        let mut include = opts["include"].as_array().cloned().unwrap_or_default();
        let logprobs = match &opts["logprobs"] {
            Value::Bool(true) => Some(json!(20)),
            Value::Number(_) => Some(opts["logprobs"].clone()),
            _ => None,
        };
        if logprobs.as_ref().is_some_and(|v| v.as_f64() != Some(0.)) && !include.contains(&json!("message.output_text.logprobs")) {
            include.push(json!("message.output_text.logprobs"));
        }
        if opts["store"] == false && reasoning && !include.contains(&json!("reasoning.encrypted_content")) {
            include.push(json!("reasoning.encrypted_content"));
        }
        let mut body = json!({"model":model,"input":input});
        field(&mut body, "temperature", call.temperature.map(|v| json!(v)));
        field(&mut body, "top_p", call.top_p.map(|v| json!(v)));
        field(&mut body, "max_output_tokens", call.max_output_tokens.map(|v| json!(v)));
        let format = call.response_format.as_ref();
        if format.is_some_and(|v| v["type"] == "json") || !opts["textVerbosity"].is_null() {
            let mut text = json!({});
            if let Some(format) = format.filter(|v| v["type"] == "json") {
                text["format"] = if let Some(s) = nonnull(format, "schema") {
                    let mut value = json!({"type":"json_schema","strict":opts["strictJsonSchema"].as_bool().unwrap_or(true),"name":format["name"].as_str().unwrap_or("response")});
                    field(&mut value, "description", nonnull(format, "description"));
                    value["schema"] = schema(&s, &mut warnings)?;
                    value
                } else {
                    json!({"type":"json_object"})
                };
            }
            field(&mut text, "verbosity", nonnull(&opts, "textVerbosity"));
            body["text"] = text;
        }
        for (from, to) in [
            ("conversation", "conversation"),
            ("maxToolCalls", "max_tool_calls"),
            ("metadata", "metadata"),
            ("parallelToolCalls", "parallel_tool_calls"),
            ("previousResponseId", "previous_response_id"),
            ("store", "store"),
            ("user", "user"),
            ("instructions", "instructions"),
            ("serviceTier", "service_tier"),
        ] {
            field(&mut body, to, nonnull(&opts, from));
        }
        if !include.is_empty() || opts.get("include").is_some() {
            body["include"] = json!(include);
        }
        for (from, to) in [
            ("promptCacheKey", "prompt_cache_key"),
            ("promptCacheOptions", "prompt_cache_options"),
            ("promptCacheRetention", "prompt_cache_retention"),
            ("safetyIdentifier", "safety_identifier"),
        ] {
            field(&mut body, to, nonnull(&opts, from));
        }
        field(&mut body, "top_logprobs", logprobs);
        field(&mut body, "truncation", nonnull(&opts, "truncation"));
        if let Some(management) = opts["contextManagement"].as_array() {
            body["context_management"] = json!(management
                .iter()
                .map(|v| {
                    let mut value = json!({"type":v["type"]});
                    field(&mut value, "compact_threshold", nonnull(v, "compactThreshold"));
                    value
                })
                .collect::<Vec<_>>());
        }
        if reasoning && (effort.is_some() || summary.is_some() || !opts["reasoningMode"].is_null() || !opts["reasoningContext"].is_null()) {
            let mut value = json!({});
            field(&mut value, "effort", effort.clone());
            field(&mut value, "summary", summary);
            field(&mut value, "mode", nonnull(&opts, "reasoningMode"));
            field(&mut value, "context", nonnull(&opts, "reasoningContext"));
            body["reasoning"] = value;
        }
        if cap.gpt6 && body.get("prompt_cache_retention").is_some() {
            body.as_object_mut().unwrap().shift_remove("prompt_cache_retention");
            warnings.push(warning(
                "promptCacheRetention",
                "promptCacheRetention is not supported by GPT-6 and later models; use promptCacheOptions instead",
            ));
        }
        if reasoning {
            if !(effort == Some(json!("none")) && cap.nonreasoning) {
                for (key, wire) in [("temperature", "temperature"), ("topP", "top_p")] {
                    if body.as_object_mut().unwrap().shift_remove(wire).is_some() {
                        warnings.push(warning(key, format!("{key} is not supported for reasoning models")));
                    }
                }
                if cap.efforts.is_some()
                    && (body.get("top_logprobs").is_some()
                        || body["include"].as_array().is_some_and(|v| v.contains(&json!("message.output_text.logprobs"))))
                {
                    body.as_object_mut().unwrap().shift_remove("top_logprobs");
                    if let Some(include) = body["include"].as_array_mut() {
                        include.retain(|v| *v != json!("message.output_text.logprobs"));
                        if include.is_empty() {
                            body.as_object_mut().unwrap().shift_remove("include");
                        }
                    }
                    warnings.push(warning("logprobs", "logprobs is not supported for reasoning models"));
                }
            }
        } else {
            for key in ["reasoningEffort", "reasoningSummary", "reasoningMode", "reasoningContext"] {
                if !opts[key].is_null() {
                    warnings.push(warning(key, format!("{key} is not supported for non-reasoning models")));
                }
            }
        }
        if opts["serviceTier"] == "flex" && !cap.flex {
            body.as_object_mut().unwrap().shift_remove("service_tier");
            warnings.push(warning("serviceTier", "flex processing is only available for o3, o4-mini, and gpt-5 models"));
        }
        if matches!(opts["serviceTier"].as_str(), Some("priority" | "fast")) && !cap.priority {
            body.as_object_mut().unwrap().shift_remove("service_tier");
            warnings.push(warning("serviceTier","priority processing is only available for supported models (gpt-4, gpt-5, gpt-5-mini, o3, o4-mini) and requires Enterprise access. gpt-5-nano is not supported"));
        }
        let mut tool_warnings = vec![];
        let mut tools = vec![];
        for tool in call.tools.as_deref().unwrap_or(&[]) {
            let options = tool.provider_options.as_ref().and_then(|v| v.get("openai")).cloned().unwrap_or(json!({}));
            let mut value = json!({"type":"function","name":tool.name});
            field(&mut value, "description", tool.description.as_ref().map(|v| json!(v)));
            let mut schema_warnings = vec![];
            value["parameters"] = schema(&tool.input_schema, &mut schema_warnings)?;
            if options["async"] == true && !cap.gpt6 {
                tool_warnings.push(warning(
                    &format!("async tool calling for \"{}\"", tool.name),
                    "Async tool calling is only supported by GPT-6 and later models.",
                ));
            } else {
                field(&mut value, "async", nonnull(&options, "async"));
            }
            tool_warnings.extend(schema_warnings);
            value["strict"] = json!(tool.strict.unwrap_or(false));
            for (from, to) in [("deferLoading", "defer_loading"), ("allowedCallers", "allowed_callers")] {
                field(&mut value, to, nonnull(&options, from));
            }
            if let Some(output) = nonnull(&options, "outputSchema") {
                value["output_schema"] = schema(&output, &mut tool_warnings)?;
            }
            tools.push(value);
        }
        if !tools.is_empty() {
            body["tools"] = json!(tools);
            field(
                &mut body,
                "tool_choice",
                call.tool_choice.as_ref().map(|choice| match choice {
                    ToolChoice::Auto => json!("auto"),
                    ToolChoice::None => json!("none"),
                    ToolChoice::Required => json!("required"),
                    ToolChoice::Tool { tool_name } => json!({"type":"function","name":tool_name}),
                }),
            );
        }
        warnings.extend(tool_warnings);
        Ok((body, warnings, opts))
    }
}

fn stream_error(frame: &Value) -> Option<Map<String, Value>> {
    let error = if frame["type"] == "response.failed" {
        &frame["response"]["error"]
    } else if frame["error"].is_object() {
        &frame["error"]
    } else {
        frame
    };
    let message = error["message"].as_str()?;
    let kind = if frame["type"] == "response.failed" { Some(json!("response.failed")) } else { nonnull(error, "type") };
    let code = nonnull(error, "code").filter(|v| v.is_string() || v.is_number());
    let explicit = code
        .as_ref()
        .and_then(|v| {
            v.as_u64().or_else(|| v.as_str().filter(|s| s.len() == 3 && s.bytes().all(|b| b.is_ascii_digit())).and_then(|s| s.parse().ok()))
        })
        .filter(|v| (400..=599).contains(v));
    let discriminator = format!(
        "{} {}",
        code.as_ref().map(|v| v.as_str().map(str::to_string).unwrap_or_else(|| stringify(v))).unwrap_or_default(),
        kind.as_ref().and_then(Value::as_str).unwrap_or("")
    )
    .to_lowercase();
    let status = explicit.unwrap_or_else(|| {
        if discriminator.contains("insufficient_quota") || discriminator.contains("rate_limit") {
            429
        } else if discriminator.contains("authentication") {
            401
        } else if discriminator.contains("permission") {
            403
        } else if discriminator.contains("not_found") {
            404
        } else if ["invalid", "bad_request", "context_length"].iter().any(|s| discriminator.contains(s)) {
            400
        } else if discriminator.contains("overload") {
            503
        } else if discriminator.contains("timeout") {
            504
        } else {
            500
        }
    });
    let retry = code != Some(json!("insufficient_quota"))
        && kind != Some(json!("insufficient_quota"))
        && ApiCallError::default_retryable(Some(status as u16));
    let mut value = json!({"message":message});
    field(&mut value, "type", kind);
    field(&mut value, "code", code);
    value["statusCode"] = json!(status);
    value["isRetryable"] = json!(retry);
    value["data"] = frame.clone();
    Some(value.as_object().unwrap().clone())
}
fn select_fields(value: &Value, keys: &[&str]) -> Value {
    let mut result = Map::new();
    for key in keys {
        if let Some(value) = value.get(*key) {
            result.insert((*key).into(), value.clone());
        }
    }
    Value::Object(result)
}
fn normalized_error(value: &Value) -> Value {
    if value["type"] == "response.failed" {
        let mut frame = select_fields(value, &["type", "sequence_number"]);
        frame["response"] = select_fields(&value["response"], &["error", "incomplete_details", "usage", "reasoning", "service_tier"]);
        if frame["response"]["error"].is_object() {
            frame["response"]["error"] = select_fields(&frame["response"]["error"], &["code", "message"]);
        }
        frame
    } else if value["error"].is_object() {
        let mut frame = select_fields(value, &["type", "sequence_number"]);
        frame["error"] = select_fields(&value["error"], &["type", "code", "message", "param"]);
        frame
    } else {
        select_fields(value, &["type", "sequence_number", "code", "message", "param"])
    }
}
fn valid(value: &Value) -> bool {
    let strings = |v: &Value, keys: &[&str]| keys.iter().all(|k| v[*k].is_string());
    match value["type"].as_str() {
        Some("response.created" | "response.in_progress") => {
            strings(&value["response"], &["id", "model"]) && value["response"]["created_at"].is_number()
        }
        Some("response.output_text.delta") => strings(value, &["item_id", "delta"]),
        Some("response.output_item.added" | "response.output_item.done") => {
            value["output_index"].is_number()
                && value["item"].is_object()
                && strings(&value["item"], &["type", "id"])
                && if value["item"]["type"] == "function_call" {
                    strings(&value["item"], &["call_id", "name", "arguments"])
                        && (value["type"] != "response.output_item.done"
                            || matches!(value["item"]["status"].as_str(), Some("in_progress" | "completed" | "incomplete")))
                } else {
                    true
                }
        }
        Some("response.function_call_arguments.delta") => value["output_index"].is_number() && strings(value, &["item_id", "delta"]),
        Some("response.reasoning_summary_text.delta") => strings(value, &["item_id", "delta"]) && value["summary_index"].is_number(),
        Some("response.reasoning_summary_part.added" | "response.reasoning_summary_part.done") => {
            strings(value, &["item_id"]) && value["summary_index"].is_number()
        }
        Some("response.completed" | "response.incomplete" | "response.failed") => {
            value["response"].is_object()
                && (value["response"]["usage"].is_null()
                    || value["response"]["usage"]["input_tokens"].is_number() && value["response"]["usage"]["output_tokens"].is_number())
        }
        Some("error") => value["sequence_number"].is_number() && stream_error(value).is_some(),
        Some(_) => true,
        None => false,
    }
}
fn output_chunk(value: &Value) -> bool {
    matches!(
        value["type"].as_str(),
        Some(
            "response.output_item.added"
                | "response.output_item.done"
                | "response.output_text.delta"
                | "response.function_call_arguments.delta"
                | "response.function_call_arguments.done"
                | "response.custom_tool_call_input.delta"
                | "response.image_generation_call.partial_image"
                | "response.code_interpreter_call_code.delta"
                | "response.code_interpreter_call_code.done"
                | "response.apply_patch_call_operation_diff.delta"
                | "response.apply_patch_call_operation_diff.done"
                | "response.completed"
                | "response.incomplete"
                | "response.reasoning_summary_part.added"
                | "response.reasoning_summary_part.done"
                | "response.reasoning_summary_text.delta"
                | "response.output_text.annotation.added"
        )
    )
}
#[async_trait(?Send)]
impl LanguageModel for ResponsesModel {
    async fn do_stream(&self, call: CallOptions) -> Result<StreamParts, LanguageModelError> {
        let (body, warnings, opts) = self.arguments(&call)?;
        let mut wire = body.clone();
        wire["stream"] = json!(true);
        let mut headers = Headers::from([("authorization".into(), format!("Bearer {}", self.0.api_key))]);
        headers.extend(self.0.headers.iter().map(|(k, v)| (k.to_lowercase(), v.clone())));
        let agent = headers.entry("user-agent".into()).or_default();
        if !agent.is_empty() {
            agent.push(' ');
        }
        agent.push_str("ai-sdk/openai/4.0.78");
        if let Some(call) = &call.headers {
            headers.extend(call.iter().map(|(k, v)| (k.to_lowercase(), v.clone())));
        }
        headers.entry("user-agent".into()).or_default().push_str(" ai-sdk/provider-utils/5.0.49 runtime/node.js/24");
        let url = format!("{}/responses", self.0.base_url.trim_end_matches('/'));
        let response = post_json(self.0.fetch.as_ref(), &url, headers, wire, call.abort_signal).await?;
        let mut input: Pin<Box<dyn Stream<Item = Result<Value, LanguageModelError>>>> = Box::pin(json_stream(response.body.unwrap()));
        let mut initial = vec![];
        let mut accepted = false;
        loop {
            let item = if accepted {
                match tokio::time::timeout(Duration::from_millis(50), input.next()).await {
                    Ok(item) => item,
                    Err(_) => break,
                }
            } else {
                input.next().await
            };
            let Some(item) = item else {
                break;
            };
            if let Ok(value) = &item {
                if !valid(value) {
                    initial.push(item);
                    break;
                }
                if value["type"] == "error" || value["type"] == "response.failed" && !value["response"]["error"].is_null() {
                    let frame = normalized_error(value);
                    let normalized = stream_error(&frame);
                    let mut error = ApiCallError::new(
                        normalized
                            .as_ref()
                            .and_then(|m| m["message"].as_str())
                            .unwrap_or("OpenAI stream failed before any output was generated"),
                        url,
                        Some(body),
                        Some(normalized.as_ref().and_then(|m| m["statusCode"].as_u64()).unwrap_or(500) as u16),
                    );
                    error.response_headers = Some(response.headers);
                    error.response_body = Some(stringify(&frame));
                    error.data = Some(frame);
                    if let Some(normalized) = normalized {
                        error.is_retryable = normalized["isRetryable"].as_bool().unwrap_or(false);
                    }
                    return Err(error.into());
                }
                let output = output_chunk(value);
                accepted |= value["type"] == "response.in_progress";
                initial.push(item);
                if output {
                    break;
                }
            } else {
                initial.push(item);
                break;
            }
        }
        Ok(convert_stream(
            Box::pin(futures::stream::iter(initial).chain(input)),
            warnings,
            call.include_raw_chunks == Some(true),
            opts,
            call.tools.unwrap_or_default(),
        ))
    }
}
struct Tool {
    id: String,
    name: String,
    suppressed: bool,
    deltas: Vec<String>,
    is_async: Option<Value>,
}
struct ReasoningState {
    encrypted: Value,
    parts: BTreeMap<u64, u8>,
}
struct State {
    input: Pin<Box<dyn Stream<Item = Result<Value, LanguageModelError>>>>,
    pending: VecDeque<StreamPart>,
    raw: bool,
    done: bool,
    store: bool,
    logprobs_enabled: bool,
    tools: Vec<FunctionTool>,
    calls: BTreeMap<u64, Tool>,
    reasoning: HashMap<String, ReasoningState>,
    item_ids: HashMap<u64, String>,
    phase: Option<Value>,
    annotations: Vec<Value>,
    has_function: bool,
    failed: bool,
    /// The response said it was done (completed, incomplete or failed), or the stream sent an error.
    ended: bool,
    finish: FinishReason,
    usage: Option<Value>,
    response_id: Value,
    logprobs: Vec<Value>,
    service: Option<Value>,
    context: Option<Value>,
}
fn usage(value: Option<&Value>) -> Usage {
    let Some(v) = value else {
        return Usage::default();
    };
    let input = v["input_tokens"].as_f64().unwrap_or(0.);
    let output = v["output_tokens"].as_f64().unwrap_or(0.);
    let read = v["input_tokens_details"]["cached_tokens"].as_f64().unwrap_or(0.);
    let write = v["input_tokens_details"]["cache_write_tokens"].as_f64();
    let reasoning = v["output_tokens_details"]["reasoning_tokens"].as_f64().unwrap_or(0.);
    Usage {
        input_tokens: InputTokens {
            total: Some(input),
            no_cache: Some(input - read - write.unwrap_or(0.)),
            cache_read: Some(read),
            cache_write: write,
        },
        output_tokens: OutputTokens { total: Some(output), text: Some(output - reasoning), reasoning: Some(reasoning) },
        raw: Some(v.clone()),
    }
}
fn finish(reason: Option<String>, has_function: bool) -> FinishReason {
    let unified = match reason.as_deref() {
        None => {
            if has_function {
                FinishReasonUnified::ToolCalls
            } else {
                FinishReasonUnified::Stop
            }
        }
        Some("max_output_tokens") => FinishReasonUnified::Length,
        Some("content_filter") => FinishReasonUnified::ContentFilter,
        _ => {
            if has_function {
                FinishReasonUnified::ToolCalls
            } else {
                FinishReasonUnified::Other
            }
        }
    };
    FinishReason { unified, raw: reason }
}
impl State {
    fn emit(&mut self, value: Value) {
        match serde_json::from_value(value) {
            Ok(part) => self.pending.push_back(part),
            Err(error) => self.error(LanguageModelError::other(format!("Invalid OpenAI stream part: {error}"))),
        }
    }
    fn error(&mut self, error: LanguageModelError) {
        self.failed = true;
        self.finish = FinishReason { unified: FinishReasonUnified::Error, raw: None };
        self.pending.push_back(StreamPart::Error { error });
    }
    fn resolved(&self, value: &Value) -> String {
        value["output_index"].as_u64().and_then(|i| self.item_ids.get(&i)).cloned().unwrap_or_else(|| string(value, "item_id"))
    }
    fn suppressed(&self, name: &str) -> bool {
        name == "parallel" && !self.tools.iter().any(|t| t.name == "parallel")
    }
    fn expanded(&self, item: &Value) -> Option<Vec<Value>> {
        let input = safe_json(item["arguments"].as_str()?).ok()?;
        let uses = input["tool_uses"].as_array()?;
        if uses.is_empty() {
            return None;
        }
        let mut result = vec![];
        for (index, entry) in uses.iter().enumerate() {
            let name = entry["recipient_name"].as_str()?.strip_prefix("functions.")?;
            if !self.tools.iter().any(|t| t.name == name) || !entry["parameters"].is_object() {
                return None;
            }
            result.push(json!({"type":"tool-call","toolCallId":format!("{}_{index}",string(item,"call_id")),"toolName":name,"input":stringify(&entry["parameters"]),"providerMetadata":{"openai":{"parallelToolCall":{"itemId":item["id"],"toolCallId":item["call_id"],"toolName":item["name"],"input":item["arguments"],"index":index,"count":uses.len()}}}}));
        }
        Some(result)
    }
    // A summary index that isn't a whole number from 0 (a gateway re-serializing it) skips its event.
    fn accept(&mut self, value: Value) {
        if self.raw {
            self.pending.push_back(StreamPart::Raw { raw_value: value.clone() });
        }
        if !valid(&value) {
            self.error(LanguageModelError::other(format!("Invalid OpenAI response data: {}", stringify(&value))));
            return;
        }
        if matches!(value["type"].as_str(), Some("response.completed" | "response.incomplete" | "response.failed" | "error")) {
            self.ended = true;
        }
        let index = value["output_index"].as_u64().unwrap_or(0);
        let item = &value["item"];
        match value["type"].as_str(){
            Some("response.created")=>{self.response_id=value["response"]["id"].clone();self.emit(json!({"type":"response-metadata","id":value["response"]["id"],"timestamp":(value["response"]["created_at"].as_f64().unwrap()*1000.) as i64,"modelId":value["response"]["model"]}));},
            Some("response.output_item.added")=>match item["type"].as_str(){
                Some("message")=>{let id=string(item,"id");self.item_ids.insert(index,id.clone());self.annotations.clear();self.phase=nonnull(item,"phase");let mut meta=json!({"itemId":id});field(&mut meta,"phase",self.phase.clone());self.emit(json!({"type":"text-start","id":id,"providerMetadata":{"openai":meta}}));},
                Some("reasoning")=>{let id=string(item,"id");self.item_ids.insert(index,id.clone());self.reasoning.insert(id.clone(),ReasoningState{encrypted:item["encrypted_content"].clone(),parts:BTreeMap::from([(0,0)])});self.emit(json!({"type":"reasoning-start","id":format!("{id}:0"),"providerMetadata":{"openai":{"itemId":id,"reasoningEncryptedContent":item["encrypted_content"]}}}));},
                Some("function_call")=>{let id=string(item,"call_id");let name=string(item,"name");let suppressed=self.suppressed(&name);if !suppressed{self.emit(json!({"type":"tool-input-start","id":id,"toolName":name}));}self.calls.insert(index,Tool{id,name,suppressed,deltas:vec![],is_async:nonnull(item,"async")});},_=>{},
            },
            Some("response.output_item.done")=>match item["type"].as_str(){
                Some("message")=>{let id=self.item_ids.remove(&index).unwrap_or_else(||string(item,"id"));let mut meta=json!({"itemId":id});field(&mut meta,"phase",nonnull(item,"phase").or(self.phase.take()));if !self.annotations.is_empty(){meta["annotations"]=json!(self.annotations);}self.emit(json!({"type":"text-end","id":id,"providerMetadata":{"openai":meta}}));},
                Some("reasoning")=>{let id=self.item_ids.remove(&index).unwrap_or_else(||string(item,"id"));if let Some(reasoning)=self.reasoning.remove(&id){for(part,status)in reasoning.parts{if status!=2{self.emit(json!({"type":"reasoning-end","id":format!("{id}:{part}"),"providerMetadata":{"openai":{"itemId":id,"reasoningEncryptedContent":item["encrypted_content"]}}}));}}}},
                Some("compaction")=>self.emit(json!({"type":"custom","kind":"openai.compaction","providerMetadata":{"openai":{"type":"compaction","itemId":item["id"],"encryptedContent":item["encrypted_content"]}}})),
                Some("function_call")=>{
                    let call=self.calls.remove(&index);self.has_function=true;let suppressed=call.as_ref().map(|v|v.suppressed).unwrap_or_else(||self.suppressed(&string(item,"name")));
                    if let Some(expanded)=suppressed.then(||self.expanded(item)).flatten(){for part in expanded{let id=&part["toolCallId"];self.emit(json!({"type":"tool-input-start","id":id,"toolName":part["toolName"]}));self.emit(json!({"type":"tool-input-delta","id":id,"delta":part["input"]}));self.emit(json!({"type":"tool-input-end","id":id}));self.emit(part);}return;}
                    let id=string(item,"call_id");if suppressed{self.emit(json!({"type":"tool-input-start","id":id,"toolName":item["name"]}));if let Some(deltas)=call.as_ref().map(|c|&c.deltas).filter(|d|!d.is_empty()){for delta in deltas{self.emit(json!({"type":"tool-input-delta","id":id,"delta":delta}));}}else if item["arguments"]!=""{self.emit(json!({"type":"tool-input-delta","id":id,"delta":item["arguments"]}));}}
                    let mut end=json!({"type":"tool-input-end","id":id});if let Some(namespace)=nonnull(item,"namespace"){end["providerMetadata"]=json!({"openai":{"namespace":namespace}});}self.emit(end);
                    let mut meta=json!({"itemId":item["id"]});field(&mut meta,"async",nonnull(item,"async").or_else(||call.and_then(|c|c.is_async)));field(&mut meta,"namespace",nonnull(item,"namespace"));if let Some(caller)=nonnull(item,"caller"){meta["caller"]=if caller["type"]=="program"{json!({"type":"program","callerId":caller["caller_id"]})}else{caller};}
                    self.emit(json!({"type":"tool-call","toolCallId":id,"toolName":item["name"],"input":item["arguments"],"providerMetadata":{"openai":meta}}));
                },_=>{},
            },
            Some("response.function_call_arguments.delta")=>{if let Some(call)=self.calls.get_mut(&index){if call.suppressed{call.deltas.push(string(&value,"delta"));}else{let id=call.id.clone();self.emit(json!({"type":"tool-input-delta","id":id,"delta":value["delta"]}));}}},
            Some("response.output_text.delta")=>{self.emit(json!({"type":"text-delta","id":self.resolved(&value),"delta":value["delta"]}));if self.logprobs_enabled&&!value["logprobs"].is_null(){self.logprobs.push(value["logprobs"].clone());}},
            Some("response.reasoning_summary_part.added")=>{
                let id=self.resolved(&value);let Some(part)=value["summary_index"].as_f64().filter(|n|*n>=0.0&&n.fract()==0.0).map(|n|n as u64) else{return};if part>0{if let Some(reasoning)=self.reasoning.get_mut(&id){reasoning.parts.insert(part,0);let mut ends=vec![];for(index,status)in &mut reasoning.parts{if *status==1{*status=2;ends.push(*index);}}let encrypted=reasoning.encrypted.clone();for index in ends{self.emit(json!({"type":"reasoning-end","id":format!("{id}:{index}"),"providerMetadata":{"openai":{"itemId":id}}}));}self.emit(json!({"type":"reasoning-start","id":format!("{id}:{part}"),"providerMetadata":{"openai":{"itemId":id,"reasoningEncryptedContent":encrypted}}}));}}
            },
            Some("response.reasoning_summary_text.delta")=>{let id=self.resolved(&value);self.emit(json!({"type":"reasoning-delta","id":format!("{id}:{}",value["summary_index"]),"delta":value["delta"],"providerMetadata":{"openai":{"itemId":id}}}));},
            Some("response.reasoning_summary_part.done")=>{let id=self.resolved(&value);let Some(part)=value["summary_index"].as_f64().filter(|n|*n>=0.0&&n.fract()==0.0).map(|n|n as u64) else{return};if let Some(reasoning)=self.reasoning.get_mut(&id){reasoning.parts.insert(part,if self.store{2}else{1});if self.store{self.emit(json!({"type":"reasoning-end","id":format!("{id}:{part}"),"providerMetadata":{"openai":{"itemId":id}}}));}}},
            Some("response.completed"|"response.incomplete")=>{if !self.failed{self.finish=finish(value["response"]["incomplete_details"]["reason"].as_str().map(str::to_string),self.has_function);}self.usage=nonnull(&value["response"],"usage");self.service=nonnull(&value["response"],"service_tier").or(self.service.take());self.context=nonnull(&value["response"]["reasoning"],"context").or(self.context.take());},
            Some("response.failed")=>{let reason=value["response"]["incomplete_details"]["reason"].as_str().map(str::to_string);self.finish=if reason.is_some(){finish(reason,self.has_function)}else{FinishReason{unified:FinishReasonUnified::Error,raw:Some("error".into())}};self.usage=nonnull(&value["response"],"usage");self.context=nonnull(&value["response"]["reasoning"],"context").or(self.context.take());if !self.failed&&!value["response"]["error"].is_null(){self.failed=true;let mut frame=select_fields(&value,&["type","sequence_number"]);frame["response"]=select_fields(&value["response"],&["error","incomplete_details","service_tier"]);if let Some(error)=stream_error(&frame){self.pending.push_back(StreamPart::Error{error:LanguageModelError::ProviderStream(error)});}}},
            Some("error")=>{self.failed=true;self.finish=FinishReason{unified:FinishReasonUnified::Error,raw:Some("error".into())};if let Some(error)=stream_error(&normalized_error(&value)){self.pending.push_back(StreamPart::Error{error:LanguageModelError::ProviderStream(error)});}},
            Some("response.output_text.annotation.added")=>{
                let annotation=&value["annotation"];
                self.annotations.push(annotation.clone());
                let mut source=json!({"type":"source","sourceType":if annotation["type"]=="url_citation"{"url"}else{"document"},"id":super::generate_id()});
                if annotation["type"]=="url_citation"{source["url"]=annotation["url"].clone();source["title"]=annotation["title"].clone();}
                else{
                    let path=annotation["type"]=="file_path";
                    source["mediaType"]=json!(if path{"application/octet-stream"}else{"text/plain"});
                    source["title"]=annotation[if path{"file_id"}else{"filename"}].clone();source["filename"]=source["title"].clone();
                    let mut metadata=json!({"type":annotation["type"],"fileId":annotation["file_id"]});
                    if annotation["type"]=="container_file_citation"{metadata["containerId"]=annotation["container_id"].clone();}else{metadata["index"]=annotation["index"].clone();}
                    source["providerMetadata"]=json!({"openai":metadata});
                }
                self.emit(source);
            },_=>{},
        }
    }
    fn flush(&mut self) {
        let pending: Vec<_> =
            self.calls.values().filter(|c| c.suppressed).map(|c| (c.id.clone(), c.name.clone(), c.deltas.clone())).collect();
        for (id, name, deltas) in pending {
            self.emit(json!({"type":"tool-input-start","id":id,"toolName":name}));
            for delta in deltas {
                self.emit(json!({"type":"tool-input-delta","id":id,"delta":delta}));
            }
        }
        let mut metadata = json!({"responseId":self.response_id});
        if !self.logprobs.is_empty() {
            metadata["logprobs"] = json!(self.logprobs);
        }
        field(&mut metadata, "serviceTier", self.service.clone());
        field(&mut metadata, "reasoningContext", self.context.clone());
        if !self.ended && !self.failed {
            // The stream closed before the response said it was done: its text is cut short and a call it was
            // writing is lost. The answer broke off, as a dropped connection does, so it can be tried again.
            let mut broke = ApiCallError::new("The response stream ended before the response was done.", "", None, Some(200));
            broke.is_retryable = true;
            self.pending.push_back(StreamPart::Error { error: broke.into() });
        }
        self.pending.push_back(StreamPart::Finish {
            usage: usage(self.usage.as_ref()),
            finish_reason: self.finish.clone(),
            provider_metadata: Some(Map::from_iter([("openai".into(), metadata)])),
        });
    }
}
fn convert_stream(
    input: Pin<Box<dyn Stream<Item = Result<Value, LanguageModelError>>>>,
    warnings: Vec<Value>,
    raw: bool,
    opts: Value,
    tools: Vec<FunctionTool>,
) -> StreamParts {
    let state = State {
        input,
        pending: VecDeque::from([StreamPart::StreamStart { warnings }]),
        raw,
        done: false,
        store: opts["store"] == true,
        logprobs_enabled: opts["logprobs"] == true || opts["logprobs"].as_f64().is_some_and(|n| n != 0.),
        tools,
        calls: BTreeMap::new(),
        reasoning: HashMap::new(),
        item_ids: HashMap::new(),
        phase: None,
        annotations: vec![],
        has_function: false,
        failed: false,
        ended: false,
        finish: FinishReason { unified: FinishReasonUnified::Other, raw: None },
        usage: None,
        response_id: Value::Null,
        logprobs: vec![],
        service: None,
        context: None,
    };
    Box::pin(futures::stream::unfold(state, |mut state| async move {
        loop {
            if let Some(part) = state.pending.pop_front() {
                return Some((part, state));
            }
            if state.done {
                return None;
            }
            match state.input.next().await {
                Some(Ok(value)) => state.accept(value),
                Some(Err(error)) => state.error(error),
                None => {
                    state.done = true;
                    state.flush();
                }
            }
        }
    }))
}
