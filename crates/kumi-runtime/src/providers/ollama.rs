use super::compat::{think_splitter, ThinkSplitter};
use crate::{
    ai::{
        error::{ApiCallError, LanguageModelError},
        http::{ByteStream, Fetch, FetchInit},
        types::{
            AssistantPart, CallOptions, DataContent, FileData, FinishReason, FinishReasonUnified, InputTokens, Message, OutputTokens,
            StreamPart, StreamParts, ToolCall, ToolPart, ToolResultContentItem, ToolResultOutput, Usage, UserPart,
        },
    },
    core::errors::KumiError,
    kernel::agent::LanguageModel,
};
use async_trait::async_trait;
use base64::{engine::general_purpose::STANDARD, Engine};
use futures::{future::LocalBoxFuture, StreamExt};
use kumi_common::{
    abort::Signal,
    js::{json::stringify, string::trim},
};
use serde_json::{json, Value};
use std::{collections::VecDeque, rc::Rc};

pub struct OllamaShape {
    pub num_ctx: f64,
    pub think: Option<Value>,
    pub options: CallOptions,
    pub images: bool,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailurePhase {
    Request,
    Answer,
}
pub struct OllamaChatSettings {
    pub base_url: String,
    pub model: String,
    pub fetch: Rc<dyn Fetch>,
    pub shape: Rc<dyn Fn(CallOptions) -> LocalBoxFuture<'static, Result<OllamaShape, LanguageModelError>>>,
    pub failure: Rc<dyn Fn(LanguageModelError, FailurePhase) -> KumiError>,
}
struct OllamaChat(Rc<OllamaChatSettings>);
pub fn ollama_chat(settings: OllamaChatSettings) -> Rc<dyn LanguageModel> {
    Rc::new(OllamaChat(Rc::new(settings)))
}
#[async_trait(?Send)]
impl LanguageModel for OllamaChat {
    async fn do_stream(&self, call: CallOptions) -> Result<StreamParts, LanguageModelError> {
        let signal = call.abort_signal.clone();
        let shape = (self.0.shape)(call).await?;
        let tools: Vec<_> = shape.options.tools.as_deref().unwrap_or(&[]).iter().map(|tool| json!({"type":"function","function":{"name":tool.name,"description":tool.description.as_deref().unwrap_or(""),"parameters":tool.input_schema}})).collect();
        let mut body = json!({"model":self.0.model,"messages":messages(&shape.options.prompt, shape.images),"stream":true});
        let object = body.as_object_mut().unwrap();
        if !tools.is_empty() {
            object.insert("tools".into(), Value::Array(tools));
        }
        if let Some(think) = shape.think {
            object.insert("think".into(), think);
        }
        object.insert("options".into(), json!({"num_ctx":shape.num_ctx}));
        object.insert("truncate".into(), json!(false));
        object.insert("shift".into(), json!(false));
        let url = format!("{}/api/chat", self.0.base_url);
        let response = match self
            .0
            .fetch
            .fetch(
                &url,
                FetchInit {
                    method: "POST".into(),
                    headers: [("content-type".into(), "application/json".into())].into(),
                    body: Some(stringify(&body)),
                    signal: signal.clone(),
                },
            )
            .await
        {
            Ok(response) => response,
            Err(error) => {
                return Err(if signal.as_ref().is_some_and(Signal::is_cancelled) {
                    error
                } else {
                    (self.0.failure)(error, FailurePhase::Request).into()
                })
            }
        };
        if !response.ok() || response.body.is_none() {
            let status = response.status;
            let text = response.text().await.unwrap_or_default();
            let mut error = ApiCallError::new(format!("HTTP {status}"), url, Some(json!({})), Some(status));
            error.response_body = Some(text);
            error.is_retryable = false;
            return Err((self.0.failure)(error.into(), FailurePhase::Request).into());
        }
        Ok(answer(response.body.unwrap(), signal, self.0.clone()))
    }
}
const NO_IMAGE: &str = "[An image this model can't be shown.]";
fn encoded(data: &DataContent) -> String {
    match data {
        DataContent::Bytes(data) => STANDARD.encode(data),
        DataContent::Base64(data) => data.clone(),
    }
}
fn parsed(input: &Value) -> Value {
    match input {
        Value::String(text) => serde_json::from_str(text).unwrap_or(json!({})),
        Value::Null => json!({}),
        _ => input.clone(),
    }
}
/// Kumi's conversation as Ollama's messages: a tool's result as words, each tied to its call.
pub fn messages(prompt: &[Message], images: bool) -> Vec<Value> {
    prompt.iter().flat_map(|message| match message {
        Message::System { content, .. } => vec![json!({"role":"system","content":content})],
        Message::User { content, .. } => {
            let words = content.iter().filter_map(|part| match part { UserPart::Text(text) => Some(text.text.as_str()), _ => None }).collect::<String>();
            let pictures: Vec<_> = content.iter().filter_map(|part| match part {
                UserPart::File(file) if file.media_type.starts_with("image") => match &file.data { FileData::Data { data } => Some(encoded(data)), _ => None }, _ => None,
            }).collect();
            vec![if pictures.is_empty() { json!({"role":"user","content":words}) } else if images { json!({"role":"user","content":words,"images":pictures}) } else { json!({"role":"user","content":format!("{words}\n{NO_IMAGE}")}) }]
        }
        Message::Assistant { content, .. } => {
            let words = content.iter().filter_map(|part| match part { AssistantPart::Text(text) => Some(text.text.as_str()), _ => None }).collect::<String>();
            let thinking = content.iter().filter_map(|part| match part { AssistantPart::Reasoning(text) => Some(text.text.as_str()), _ => None }).collect::<String>();
            let calls: Vec<_> = content.iter().filter_map(|part| match part { AssistantPart::ToolCall(call) => Some(json!({"id":call.tool_call_id,"function":{"name":call.tool_name,"arguments":parsed(&call.input)}})), _ => None }).collect();
            let mut message = json!({"role":"assistant","content":words});
            if !thinking.is_empty() { message.as_object_mut().unwrap().insert("thinking".into(), json!(thinking)); }
            if !calls.is_empty() { message.as_object_mut().unwrap().insert("tool_calls".into(), json!(calls)); } vec![message]
        }
        Message::Tool { content, .. } => content.iter().filter_map(|part| match part {
            ToolPart::ToolResult(result) => Some(json!({"role":"tool","content":words(&result.output),"tool_name":result.tool_name,"tool_call_id":result.tool_call_id})), _ => None,
        }).collect(),
    }).collect()
}
fn words(output: &ToolResultOutput) -> String {
    match output {
        ToolResultOutput::Text { value, .. } | ToolResultOutput::ErrorText { value, .. } => value.clone(),
        ToolResultOutput::Json { value, .. } | ToolResultOutput::ErrorJson { value, .. } => stringify(value),
        ToolResultOutput::ExecutionDenied { reason, .. } => reason.as_deref().unwrap_or("Not run.").into(),
        ToolResultOutput::Content { value, .. } => value
            .iter()
            .filter_map(|part| match part {
                ToolResultContentItem::Text { text, .. } if !text.is_empty() => Some(text.as_str()),
                ToolResultContentItem::File { .. } => Some(NO_IMAGE),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n"),
    }
}
struct Answer {
    body: ByteStream,
    signal: Option<Signal>,
    settings: Rc<OllamaChatSettings>,
    buffer: Vec<u8>,
    think: ThinkSplitter,
    open: Option<bool>,
    called: bool,
    finished: bool,
    started: bool,
    first_line: bool,
    pending: VecDeque<StreamPart>,
}
impl Answer {
    fn close(&mut self) {
        if let Some(reasoning) = self.open.take() {
            self.pending.push_back(if reasoning {
                StreamPart::ReasoningEnd { id: "reasoning-0".into(), provider_metadata: None }
            } else {
                StreamPart::TextEnd { id: "txt-0".into(), provider_metadata: None }
            });
        }
    }
    fn say(&mut self, reasoning: bool, delta: String) {
        if delta.is_empty() {
            return;
        }
        if self.open != Some(reasoning) {
            self.close();
            self.open = Some(reasoning);
            self.pending.push_back(if reasoning {
                StreamPart::ReasoningStart { id: "reasoning-0".into(), provider_metadata: None }
            } else {
                StreamPart::TextStart { id: "txt-0".into(), provider_metadata: None }
            });
        }
        self.pending.push_back(if reasoning {
            StreamPart::ReasoningDelta { id: "reasoning-0".into(), delta, provider_metadata: None }
        } else {
            StreamPart::TextDelta { id: "txt-0".into(), delta, provider_metadata: None }
        });
    }
    fn line(&mut self, raw: &[u8]) -> Result<bool, LanguageModelError> {
        let text = String::from_utf8_lossy(raw);
        let text = if !self.first_line {
            self.first_line = true;
            text.strip_prefix('\u{feff}').unwrap_or(&text)
        } else {
            &text
        };
        let Ok(chunk) = serde_json::from_str::<Value>(trim(text)) else {
            return Ok(false);
        };
        if !chunk.is_object() {
            return Ok(false);
        }
        if let Some(error) = chunk.get("error").and_then(Value::as_str) {
            return Err(LanguageModelError::other(error));
        }
        let message = &chunk["message"];
        if let Some(thinking) = message.get("thinking").and_then(Value::as_str) {
            self.say(true, thinking.into());
        }
        if let Some(content) = message.get("content").and_then(Value::as_str).filter(|s| !s.is_empty()) {
            let split = self.think.push(content);
            self.say(true, split.reasoning);
            self.say(false, split.text);
        }
        if let Some(calls) = message.get("tool_calls").and_then(Value::as_array) {
            for call in calls {
                let function = &call["function"];
                let Some(name) = function.get("name").and_then(Value::as_str).filter(|s| !s.is_empty()) else {
                    continue;
                };
                let id = call
                    .get("id")
                    .and_then(Value::as_str)
                    .filter(|s| !s.is_empty())
                    .map(str::to_string)
                    .unwrap_or_else(|| format!("call_{}", &uuid::Uuid::new_v4().simple().to_string()[..16]));
                let args = function.get("arguments").filter(|value| !value.is_null()).cloned().unwrap_or(json!({}));
                let input = args.as_str().map(str::to_string).unwrap_or_else(|| stringify(&args));
                self.close();
                self.pending.push_back(StreamPart::ToolInputStart {
                    id: id.clone(),
                    tool_name: name.into(),
                    provider_metadata: None,
                    provider_executed: None,
                    dynamic: None,
                    title: None,
                });
                self.pending.push_back(StreamPart::ToolInputDelta { id: id.clone(), delta: input.clone(), provider_metadata: None });
                self.pending.push_back(StreamPart::ToolInputEnd { id: id.clone(), provider_metadata: None });
                self.pending.push_back(StreamPart::ToolCall(ToolCall {
                    tool_call_id: id,
                    tool_name: name.into(),
                    input,
                    provider_executed: None,
                    dynamic: None,
                    provider_metadata: None,
                }));
                self.called = true;
            }
        }
        if chunk.get("done") != Some(&Value::Bool(true)) {
            return Ok(false);
        }
        let split = self.think.flush();
        self.say(true, split.reasoning);
        self.say(false, split.text);
        self.close();
        let count = |key| chunk.get(key).and_then(Value::as_f64).filter(|n| n.is_finite() && *n >= 0.0).unwrap_or(0.0);
        let evaluated = count("prompt_eval_count");
        let cached = count("prompt_eval_cached_count");
        let raw = chunk.get("done_reason").and_then(Value::as_str).map(str::to_string);
        let unified = if self.called {
            FinishReasonUnified::ToolCalls
        } else {
            match raw.as_deref() {
                Some("length") => FinishReasonUnified::Length,
                None | Some("" | "stop") => FinishReasonUnified::Stop,
                _ => FinishReasonUnified::Other,
            }
        };
        self.pending.push_back(StreamPart::Finish {
            finish_reason: FinishReason { unified, raw },
            usage: Usage {
                input_tokens: InputTokens {
                    total: Some(evaluated + cached),
                    no_cache: Some(evaluated),
                    cache_read: Some(cached),
                    cache_write: None,
                },
                output_tokens: OutputTokens { total: Some(count("eval_count")), text: None, reasoning: None },
                raw: None,
            },
            provider_metadata: None,
        });
        Ok(true)
    }
    fn fail(&mut self, error: LanguageModelError) {
        self.finished = true;
        if self.signal.as_ref().is_some_and(Signal::is_cancelled) {
            self.pending.push_back(StreamPart::Error { error });
            return;
        }
        self.close();
        self.pending.push_back(StreamPart::Error { error: (self.settings.failure)(error, FailurePhase::Answer).into() });
    }
}
fn answer(body: ByteStream, signal: Option<Signal>, settings: Rc<OllamaChatSettings>) -> StreamParts {
    let state = Answer {
        body,
        signal,
        settings,
        buffer: Vec::new(),
        think: think_splitter(),
        open: None,
        called: false,
        finished: false,
        started: false,
        first_line: false,
        pending: VecDeque::new(),
    };
    Box::pin(futures::stream::unfold(state, |mut state| async move {
        loop {
            if let Some(part) = state.pending.pop_front() {
                return Some((part, state));
            }
            if state.finished {
                return None;
            }
            if !state.started {
                state.started = true;
                return Some((StreamPart::StreamStart { warnings: vec![] }, state));
            }
            if let Some(at) = memchr::memchr(b'\n', &state.buffer) {
                let remaining = state.buffer.split_off(at + 1);
                let mut raw = std::mem::replace(&mut state.buffer, remaining);
                raw.truncate(at);
                match state.line(&raw) {
                    Ok(done) => state.finished = done,
                    Err(error) => state.fail(error),
                }
                continue;
            }
            match state.body.next().await {
                Some(Ok(bytes)) => state.buffer.extend_from_slice(&bytes),
                Some(Err(error)) => state.fail(error),
                None => {
                    let raw = std::mem::take(&mut state.buffer);
                    match state.line(&raw) {
                        Ok(true) => state.finished = true,
                        Ok(false) => state.fail(LanguageModelError::other("terminated")),
                        Err(error) => state.fail(error),
                    }
                }
            }
        }
    }))
}
