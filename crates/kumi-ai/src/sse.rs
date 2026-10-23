//! Part of the `@ai-sdk` replacement (see `ai/mod.rs`).

use super::{error::LanguageModelError, http::ByteStream};
use futures::{Stream, StreamExt};
use serde_json::Value;
use std::collections::VecDeque;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Event {
    pub data: String,
    pub event: Option<String>,
    pub id: Option<String>,
}
/// Incremental eventsource parsing. A final event without a blank line is deliberately not flushed.
#[derive(Default)]
pub struct SseParser {
    line: Vec<u8>,
    skip_lf: bool,
    started: bool,
    data: Vec<String>,
    event: Option<String>,
    id: Option<String>,
}
impl SseParser {
    pub fn push(&mut self, bytes: &[u8]) -> Vec<Event> {
        let mut events = Vec::new();
        for &byte in bytes {
            if self.skip_lf {
                self.skip_lf = false;
                self.finish_line(&mut events);
                if byte == b'\n' {
                    continue;
                }
            }
            match byte {
                b'\r' => self.skip_lf = true,
                b'\n' => self.finish_line(&mut events),
                _ => self.line.push(byte),
            }
        }
        events
    }
    fn finish_line(&mut self, events: &mut Vec<Event>) {
        let bytes = std::mem::take(&mut self.line);
        let text = String::from_utf8_lossy(&bytes);
        let text = if !self.started {
            self.started = true;
            text.strip_prefix('\u{feff}').unwrap_or(&text)
        } else {
            &text
        };
        if text.is_empty() {
            if !self.data.is_empty() {
                events.push(Event { data: self.data.join("\n"), event: self.event.take(), id: self.id.take() });
                self.data.clear();
            }
            self.event = None;
            self.id = None;
            return;
        }
        if text.starts_with(':') {
            return;
        }
        let (field, value) = text.split_once(':').map(|(key, value)| (key, value.strip_prefix(' ').unwrap_or(value))).unwrap_or((text, ""));
        match field {
            "data" => self.data.push(value.into()),
            "event" => self.event = if value.is_empty() { None } else { Some(value.into()) },
            "id" if !value.contains('\0') => self.id = Some(value.into()),
            _ => {}
        }
    }
}
/// secure-json-parse rejects keys that JavaScript could mistake for prototype mutation.
pub fn safe_json(text: &str) -> Result<Value, LanguageModelError> {
    let value = serde_json::from_str::<Value>(text)
        .map_err(|error| LanguageModelError::other(format!("JSON parsing failed: Text: {text}.\nError message: {error}")))?;
    fn forbidden(value: &Value) -> bool {
        match value {
            Value::Object(object) => {
                object.contains_key("__proto__")
                    || object.get("constructor").and_then(Value::as_object).is_some_and(|object| object.contains_key("prototype"))
                    || object.values().any(forbidden)
            }
            Value::Array(array) => array.iter().any(forbidden),
            _ => false,
        }
    }
    if forbidden(&value) {
        return Err(LanguageModelError::other("Object contains forbidden prototype property"));
    }
    Ok(value)
}
pub fn json_stream(body: ByteStream) -> impl Stream<Item = Result<Value, LanguageModelError>> {
    futures::stream::unfold((body, SseParser::default(), VecDeque::new()), |(mut body, mut parser, mut pending)| async move {
        loop {
            if let Some(next) = pending.pop_front() {
                return Some((next, (body, parser, pending)));
            }
            match body.next().await {
                Some(Ok(bytes)) => {
                    for event in parser.push(&bytes) {
                        if event.data != "[DONE]" {
                            pending.push_back(safe_json(&event.data));
                        }
                    }
                }
                Some(Err(error)) => return Some((Err(error), (body, parser, pending))),
                None => return None,
            }
        }
    })
}
