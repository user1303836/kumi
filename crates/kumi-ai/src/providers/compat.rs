use crate::ai::{
    error::LanguageModelError,
    http::{ByteStream, Fetch, FetchInit, Response},
};
use async_trait::async_trait;
use futures::StreamExt;
use indexmap::IndexMap;
use kumi_common::js::{
    json::stringify,
    string::{trim, trim_start},
};
use regex::Regex;
use serde_json::{json, Value};
use std::{collections::HashMap, rc::Rc, sync::LazyLock};
const OPEN: &str = "<think>";
const CLOSE: &str = "</think>";
#[derive(Default, Clone, Copy, PartialEq, Eq)]
enum State {
    #[default]
    Start,
    Thinking,
    Gap,
    Words,
}
#[derive(Default)]
pub struct ThinkSplitter {
    state: State,
    held: String,
}
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Split {
    pub reasoning: String,
    pub text: String,
}
pub fn think_splitter() -> ThinkSplitter {
    ThinkSplitter::default()
}
impl ThinkSplitter {
    /// Split only an answer opening with a thinking tag, however its tags arrive in chunks.
    pub fn push(&mut self, chunk: &str) -> Split {
        let mut output = Split::default();
        let all = std::mem::take(&mut self.held) + chunk;
        let mut input = all.as_str();
        while !input.is_empty() {
            if self.state == State::Start {
                let trimmed = trim_start(input);
                if let Some(rest) = trimmed.strip_prefix(OPEN) {
                    self.state = State::Thinking;
                    input = rest;
                    continue;
                }
                if trimmed.is_empty() || OPEN.starts_with(trimmed) {
                    self.held = input.into();
                    break;
                }
                self.state = State::Words;
            }
            if self.state == State::Thinking {
                if let Some(end) = input.find(CLOSE) {
                    output.reasoning.push_str(&input[..end]);
                    input = &input[end + CLOSE.len()..];
                    self.state = State::Gap;
                    continue;
                }
                let keep = (1..=input.len().min(CLOSE.len() - 1))
                    .rev()
                    .find(|&size| CLOSE.as_bytes().starts_with(&input.as_bytes()[input.len() - size..]))
                    .unwrap_or(0);
                output.reasoning.push_str(&input[..input.len() - keep]);
                self.held = input[input.len() - keep..].into();
                break;
            }
            if self.state == State::Gap {
                input = trim_start(input);
                if input.is_empty() {
                    break;
                }
                self.state = State::Words;
            }
            output.text.push_str(input);
            break;
        }
        output
    }
    pub fn flush(&mut self) -> Split {
        let rest = std::mem::take(&mut self.held);
        if self.state == State::Thinking {
            Split { reasoning: rest, text: String::new() }
        } else {
            Split { reasoning: String::new(), text: if self.state == State::Gap { String::new() } else { rest } }
        }
    }
}
#[derive(Default)]
struct Mender {
    think: ThinkSplitter,
    ids: IndexMap<u64, String>,
    by_id: HashMap<String, f64>,
    latest: Option<f64>,
    buffer: Vec<u8>,
    finished: bool,
    ended: bool,
    called: bool,
    separated: bool,
    started: bool,
}
fn chunk(delta: Value, finish: Option<&str>) -> String {
    format!(
        "data: {}\n\n",
        stringify(&json!({"object":"chat.completion.chunk","choices":[{"index":0,"delta":delta,"finish_reason":finish}]}))
    )
}
impl Mender {
    fn mend_call(&mut self, call: &mut serde_json::Map<String, Value>) {
        if !call.get("function").is_some_and(Value::is_object) {
            call.insert("function".into(), json!({}));
        }
        let function = call.get_mut("function").unwrap().as_object_mut().unwrap();
        if let Some(args) = function.get_mut("arguments").filter(|args| !args.is_null() && !args.is_string()) {
            *args = Value::String(stringify(args));
        }
        let named = function.get("name").and_then(Value::as_str).is_some_and(|s| !s.is_empty());
        let given_id = call.get("id").and_then(Value::as_str).filter(|s| !s.is_empty()).map(str::to_string);
        let index = match call.get("index").and_then(Value::as_f64) {
            Some(index) => index,
            None => {
                let index = given_id.as_ref().and_then(|id| self.by_id.get(id).copied()).unwrap_or_else(|| match self.latest {
                    Some(latest) if !named => latest,
                    _ => self.ids.len() as f64,
                });
                call.insert("index".into(), json!(index));
                index
            }
        };
        let key = if index == 0.0 { 0 } else { index.to_bits() };
        let id = self.ids.entry(key).or_insert_with(|| {
            let id = given_id.unwrap_or_else(|| format!("call_{}", &uuid::Uuid::new_v4().simple().to_string()[..16]));
            self.by_id.insert(id.clone(), index);
            id
        });
        call.insert("id".into(), json!(id));
        self.latest = Some(index);
        self.called = true;
    }
    fn mend(&mut self, data: &str) -> String {
        let Ok(mut value) = serde_json::from_str::<Value>(data) else {
            return data.into();
        };
        let Some(choice) =
            value.get_mut("choices").and_then(Value::as_array_mut).and_then(|choices| choices.first_mut()).and_then(Value::as_object_mut)
        else {
            return data.into();
        };
        if choice.get("finish_reason").is_some_and(|reason| !reason.is_null()) {
            self.finished = true;
        }
        if let Some(delta) = choice.get_mut("delta").and_then(Value::as_object_mut) {
            if delta.get("reasoning_content").is_some_and(Value::is_string) || delta.get("reasoning").is_some_and(Value::is_string) {
                self.separated = true;
            }
            if let Some(content) = delta.get("content").and_then(Value::as_str).filter(|s| !s.is_empty() && !self.separated) {
                let split = self.think.push(content);
                delta.insert("content".into(), json!(split.text));
                if !split.reasoning.is_empty() {
                    delta.insert("reasoning_content".into(), json!(split.reasoning));
                }
            }
            if let Some(calls) = delta.get_mut("tool_calls").and_then(Value::as_array_mut) {
                for call in calls {
                    if let Some(call) = call.as_object_mut() {
                        self.mend_call(call);
                    }
                }
            }
        }
        stringify(&value)
    }
    fn ending(&mut self) -> String {
        if self.ended {
            return String::new();
        }
        self.ended = true;
        let split = self.think.flush();
        let mut out = String::new();
        if !split.reasoning.is_empty() || !split.text.is_empty() {
            let mut delta = serde_json::Map::new();
            if !split.text.is_empty() {
                delta.insert("content".into(), json!(split.text));
            }
            if !split.reasoning.is_empty() {
                delta.insert("reasoning_content".into(), json!(split.reasoning));
            }
            out += &chunk(Value::Object(delta), None);
        }
        if !self.finished {
            self.finished = true;
            out += &chunk(json!({}), Some(if self.called { "tool_calls" } else { "stop" }));
        }
        if out.is_empty() {
            out
        } else {
            format!("\n{out}")
        }
    }
    fn line(&mut self, raw: &str) -> String {
        static DATA: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^data:\s?(.*?)(\r?)$").unwrap());
        let Some(found) = DATA.captures(raw) else {
            return format!("{raw}\n");
        };
        if trim(&found[1]) == "[DONE]" {
            return format!("{}{raw}\n", self.ending());
        }
        format!("data: {}{}\n", self.mend(&found[1]), &found[2])
    }
    fn push(&mut self, bytes: &[u8], flush: bool) -> Vec<u8> {
        self.buffer.extend_from_slice(bytes);
        let mut output = String::new();
        while let Some(at) = memchr::memchr(b'\n', &self.buffer) {
            let rest = self.buffer.split_off(at + 1);
            let mut line = std::mem::replace(&mut self.buffer, rest);
            line.truncate(at);
            let decoded = String::from_utf8_lossy(&line);
            let text = if !self.started {
                self.started = true;
                decoded.strip_prefix('\u{feff}').unwrap_or(&decoded)
            } else {
                &decoded
            };
            output += &self.line(text);
        }
        if flush {
            let bytes = std::mem::take(&mut self.buffer);
            let text = String::from_utf8_lossy(&bytes);
            let text = if !self.started { text.strip_prefix('\u{feff}').unwrap_or(&text) } else { &text };
            if !text.is_empty() {
                output += &self.line(text);
            }
            output += &self.ending();
        }
        output.into_bytes()
    }
}
/// Normalize local chat streams' missing ids/indices, object arguments, inline thinking and finishes.
pub fn mend_stream(body: ByteStream) -> ByteStream {
    Box::pin(futures::stream::unfold((body, Mender::default(), false), |(mut body, mut mender, ended)| async move {
        if ended {
            return None;
        }
        loop {
            match body.next().await {
                Some(Ok(bytes)) => {
                    let out = mender.push(&bytes, false);
                    if !out.is_empty() {
                        return Some((Ok(out), (body, mender, false)));
                    }
                }
                Some(Err(error)) => return Some((Err(error), (body, mender, true))),
                None => {
                    let out = mender.push(&[], true);
                    return if out.is_empty() { None } else { Some((Ok(out), (body, mender, true))) };
                }
            }
        }
    }))
}
struct MendingFetch(Rc<dyn Fetch>);
#[async_trait(?Send)]
impl Fetch for MendingFetch {
    async fn fetch(&self, url: &str, init: FetchInit) -> Result<Response, LanguageModelError> {
        static STREAMED: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#""stream"\s*:\s*true"#).unwrap());
        let streamed = init.body.as_deref().is_some_and(|body| STREAMED.is_match(body));
        let mut response = self.0.fetch(url, init).await?;
        if response.ok() && streamed && url::Url::parse(url).is_ok_and(|url| url.path().ends_with("/chat/completions")) {
            response.body = response.body.map(mend_stream);
        }
        Ok(response)
    }
}
pub fn mending_fetch(base: Rc<dyn Fetch>) -> Rc<dyn Fetch> {
    Rc::new(MendingFetch(base))
}
