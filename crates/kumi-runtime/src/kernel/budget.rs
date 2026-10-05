//! Keeps a conversation within what a model can take. Live reads go stale as the producer works, so
//! they go first: earlier turns' larger tool results shrink to their opening and a note, and the Live
//! observation attached to earlier requests is dropped (the producer's words stay). Only when that
//! isn't enough do the earliest exchanges go. Clearing happens past thresholds, not on every request,
//! so providers' prompt caches keep working in between.

use std::borrow::Cow;

use kumi_common::js::json::stringify;
use kumi_common::js::string::{head, trim};
use serde_json::Value;

use crate::ai::types::{
    AssistantPart, DataContent, FileData, Message, Role, TextPart, ToolPart, ToolResultContentItem, ToolResultOutput, UserPart,
};
use crate::core::contracts::{TranscriptLine, TranscriptRole};

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ContextBudget {
    /// Bytes of conversation (as JSON, roughly 3 per token) past which earlier turns are cleared.
    pub clear_at: f64,
    /// Bytes never sent: past this, all earlier reads and then this turn's older ones are cleared, then the earliest exchanges dropped.
    pub limit: f64,
}

/// Roughly 50k and 130k tokens, leaving room for instructions and tools in every supported model's window.
pub const DEFAULT_BUDGET: ContextBudget = ContextBudget { clear_at: 160.0 * 1024.0, limit: 400.0 * 1024.0 };

/// Bytes a token stands for, as the budget counts them: JSON runs about 3, words more, so this errs toward room.
pub const BYTES_PER_TOKEN: f64 = 3.0;

/// The budget for a model that reads `window` tokens at once: what's left once the instructions and
/// tools (`fixed` bytes) and the answer (`answer` tokens) have their room, cleared from 40% of it as
/// the default budget is. Never less than a few exchanges, nor more than the default.
pub fn budget_for(window: f64, fixed: f64, answer: f64) -> ContextBudget {
    let limit = DEFAULT_BUDGET.limit.min((16.0_f64 * 1024.0).max((window - answer) * BYTES_PER_TOKEN - fixed));
    ContextBudget { clear_at: (limit * 0.4).floor(), limit }
}

/// How the session attaches each turn's Live observation to the producer's words.
pub const OBSERVATION_MARKER: &str = "\n\n<current_observation_untrusted>";
/// Starts the first kept message once the earliest exchanges are gone.
pub const SHORTENED: &str = "[Kumi removed the earlier part of this conversation to save room.]\n\n";

/// A conversation's words, for showing it: what the producer said and Kumi's answers. The host
/// appends each turn's Live observation to the producer's words, and the budget may note that
/// earlier exchanges are gone; neither is theirs. Saved conversations are data from disk, so any shape is taken.
pub fn transcript_of(messages: &[Value]) -> Vec<TranscriptLine> {
    messages
        .iter()
        .filter_map(|raw| {
            let role = match raw.get("role").and_then(Value::as_str) {
                Some("user") => TranscriptRole::User,
                Some("assistant") => TranscriptRole::Assistant,
                _ => return None,
            };
            let content = raw.get("content");
            let parts: &[Value] = content.and_then(Value::as_array).map(Vec::as_slice).unwrap_or(&[]);
            let text = match content.and_then(Value::as_str) {
                Some(text) => text.to_string(),
                None => parts
                    .iter()
                    .map(|part| {
                        if part.get("type").and_then(Value::as_str) == Some("text") {
                            part.get("text").and_then(Value::as_str).unwrap_or("")
                        } else {
                            ""
                        }
                    })
                    .collect(),
            };
            let words = match role {
                TranscriptRole::User => text.split(OBSERVATION_MARKER).next().unwrap_or("").replacen(SHORTENED, "", 1),
                TranscriptRole::Assistant => text,
            };
            // An answer's steps come back too: the tools it called, in order.
            let tools: Vec<String> = match role {
                TranscriptRole::Assistant => parts
                    .iter()
                    .filter(|part| part.get("type").and_then(Value::as_str) == Some("tool-call"))
                    .filter_map(|part| part.get("toolName").and_then(Value::as_str).map(str::to_string))
                    .collect(),
                TranscriptRole::User => Vec::new(),
            };
            if trim(&words).is_empty() && tools.is_empty() {
                return None;
            }
            Some(TranscriptLine { role, text: trim(&words).to_string(), tools: (!tools.is_empty()).then_some(tools) })
        })
        .collect()
}

const CLEARED: &str = " … [Kumi cleared the rest of this earlier result to save room; read Live again if you need it.]";
/// Results this small stay whole: change confirmations, refusals, short answers.
const SMALL: usize = 1024;
/// What's kept of a cleared result; change results start with what changed.
const HEAD: usize = 200;
/// What an image costs, as bytes of conversation: a 1280×720 frame is about 1.2k tokens.
const IMAGE_BYTES: usize = 4 * 1024;
/// The most images a request carries; past this, this turn's earliest are put away.
pub const MAX_IMAGES: usize = 40;

fn put_away(count: usize) -> String {
    format!(
        "[{} shown here; {} no longer attached (the tool shows {} again when asked).]",
        if count == 1 { "An image was".to_string() } else { format!("{count} images were") },
        if count == 1 { "it's" } else { "they're" },
        if count == 1 { "it" } else { "them" }
    )
}

/// Raw image bytes measure as what they cost a model, not their size: each counts once, as "" in the JSON.
fn strip_file(data: &mut FileData) -> usize {
    if let FileData::Data { data: DataContent::Bytes(_) } = data {
        *data = FileData::Data { data: DataContent::Base64(String::new()) };
        return 1;
    }
    0
}

fn strip_output(output: &mut ToolResultOutput) -> usize {
    match output {
        ToolResultOutput::Content { value, .. } => {
            value.iter_mut().map(|item| if let ToolResultContentItem::File { data, .. } = item { strip_file(data) } else { 0 }).sum()
        }
        _ => 0,
    }
}

fn strip_bytes(message: &mut Message) -> usize {
    match message {
        Message::System { .. } => 0,
        Message::User { content, .. } => {
            content.iter_mut().map(|part| if let UserPart::File(file) = part { strip_file(&mut file.data) } else { 0 }).sum()
        }
        Message::Assistant { content, .. } => content
            .iter_mut()
            .map(|part| match part {
                AssistantPart::File(file) => strip_file(&mut file.data),
                AssistantPart::ReasoningFile(file) => strip_file(&mut file.data),
                AssistantPart::ToolResult(result) => strip_output(&mut result.output),
                _ => 0,
            })
            .sum(),
        Message::Tool { content, .. } => {
            content.iter_mut().map(|part| if let ToolPart::ToolResult(result) = part { strip_output(&mut result.output) } else { 0 }).sum()
        }
    }
}

/// Bytes as sent, with each image counted at what it costs a model rather than its size.
fn bytes(messages: &[Message]) -> usize {
    let mut images = 0;
    let measured: Vec<Message> = messages
        .iter()
        .map(|message| {
            let mut message = message.clone();
            images += strip_bytes(&mut message);
            message
        })
        .collect();
    stringify(&serde_json::to_value(&measured).expect("messages serialize")).len() + images * IMAGE_BYTES
}

fn bytes_of(message: &Message) -> usize {
    let mut measured = message.clone();
    let images = strip_bytes(&mut measured);
    stringify(&serde_json::to_value(&measured).expect("message serializes")).len() + images * IMAGE_BYTES
}

fn image_count(output: &ToolResultOutput) -> usize {
    match output {
        ToolResultOutput::Content { value, .. } => value.iter().filter(|item| matches!(item, ToolResultContentItem::File { .. })).count(),
        _ => 0,
    }
}

/// A result's words, with a line where its images were.
fn words_of(output: &ToolResultOutput) -> String {
    let ToolResultOutput::Content { value, .. } = output else { return String::new() };
    let images = image_count(output);
    let mut lines: Vec<String> = value
        .iter()
        .filter_map(|item| if let ToolResultContentItem::Text { text, .. } = item { Some(text.clone()) } else { None })
        .collect();
    if images > 0 {
        lines.push(put_away(images));
    }
    lines.join("\n")
}

fn result_images(message: &Message) -> usize {
    match message {
        Message::Tool { content, .. } => {
            content.iter().map(|part| if let ToolPart::ToolResult(result) = part { image_count(&result.output) } else { 0 }).sum()
        }
        _ => 0,
    }
}

/// The messages with the images tool results showed put away, all but the latest `keep`: a result
/// keeps its words and says how many images it had. Borrowed (the same messages) when nothing changed.
pub fn put_away_images(messages: &[Message], keep: usize) -> Cow<'_, [Message]> {
    let total: usize = messages.iter().map(result_images).sum();
    if total <= keep {
        return Cow::Borrowed(messages);
    }
    let mut excess = total - keep;
    let cleared = messages
        .iter()
        .map(|message| {
            if excess == 0 || result_images(message) == 0 {
                return message.clone();
            }
            let Message::Tool { content, provider_options } = message else { return message.clone() };
            let content = content
                .iter()
                .map(|part| {
                    let ToolPart::ToolResult(result) = part else { return part.clone() };
                    let images = image_count(&result.output);
                    if excess == 0 || images == 0 {
                        return part.clone();
                    }
                    excess = excess.saturating_sub(images);
                    let mut result = result.clone();
                    result.output = ToolResultOutput::Text { value: words_of(&part_output(part)), provider_options: None };
                    ToolPart::ToolResult(result)
                })
                .collect();
            Message::Tool { content, provider_options: provider_options.clone() }
        })
        .collect();
    Cow::Owned(cleared)
}

fn part_output(part: &ToolPart) -> ToolResultOutput {
    match part {
        ToolPart::ToolResult(result) => result.output.clone(),
        ToolPart::ToolApprovalResponse(_) => ToolResultOutput::Text { value: String::new(), provider_options: None },
    }
}

fn last_index(messages: &[Message], role: Role) -> Option<usize> {
    messages.iter().rposition(|message| message.role() == role)
}

fn has_marker(message: &Message) -> bool {
    matches!(message, Message::User { content, .. } if content.iter().any(|part| matches!(part, UserPart::Text(text) if text.text.contains(OBSERVATION_MARKER))))
}

/// Where the turn before this one starts: its opening message carries the Live observation, which steering doesn't.
fn previous_turn(messages: &[Message]) -> Option<usize> {
    messages.iter().rposition(has_marker).or_else(|| last_index(messages, Role::User))
}

/// A copy of messages[0, end) with large tool results cut down and, if asked, observations removed; None when nothing changed.
fn clear_until(messages: &[Message], end: usize, observations: bool) -> Option<Vec<Message>> {
    let mut changed = false;
    let cleared: Vec<Message> = messages
        .iter()
        .enumerate()
        .map(|(index, message)| {
            if index >= end {
                return message.clone();
            }
            match message {
                Message::Tool { content, provider_options } => {
                    let mut touched = false;
                    let content = content
                        .iter()
                        .map(|part| {
                            let ToolPart::ToolResult(result) = part else { return part.clone() };
                            let output = &result.output;
                            // Words and pictures: the pictures go with the rest.
                            let value = match output {
                                ToolResultOutput::Text { value, .. } | ToolResultOutput::ErrorText { value, .. } => value.clone(),
                                ToolResultOutput::Content { .. } => words_of(output),
                                _ => return part.clone(),
                            };
                            let content_output = matches!(output, ToolResultOutput::Content { .. });
                            let small = value.ends_with(CLEARED) || value.len() <= SMALL;
                            if !content_output && small {
                                return part.clone();
                            }
                            touched = true;
                            let kept = if small { value } else { format!("{}{CLEARED}", head(&value, HEAD)) };
                            let mut result = result.clone();
                            result.output = match output {
                                ToolResultOutput::Content { .. } => ToolResultOutput::Text { value: kept, provider_options: None },
                                ToolResultOutput::Text { provider_options, .. } => {
                                    ToolResultOutput::Text { value: kept, provider_options: provider_options.clone() }
                                }
                                ToolResultOutput::ErrorText { provider_options, .. } => {
                                    ToolResultOutput::ErrorText { value: kept, provider_options: provider_options.clone() }
                                }
                                other => other.clone(),
                            };
                            ToolPart::ToolResult(result)
                        })
                        .collect();
                    if !touched {
                        return message.clone();
                    }
                    changed = true;
                    Message::Tool { content, provider_options: provider_options.clone() }
                }
                Message::User { content, provider_options } if observations => {
                    let mut touched = false;
                    let content = content
                        .iter()
                        .map(|part| match part {
                            UserPart::Text(text) => match text.text.find(OBSERVATION_MARKER) {
                                Some(at) => {
                                    touched = true;
                                    UserPart::Text(TextPart {
                                        text: text.text[..at].to_string(),
                                        provider_options: text.provider_options.clone(),
                                    })
                                }
                                None => part.clone(),
                            },
                            UserPart::File(_) => part.clone(),
                        })
                        .collect();
                    if !touched {
                        return message.clone();
                    }
                    changed = true;
                    Message::User { content, provider_options: provider_options.clone() }
                }
                _ => message.clone(),
            }
        })
        .collect();
    changed.then_some(cleared)
}

/// The earliest whole exchanges removed until what's left takes at most `room` bytes; what's kept
/// starts where the producer spoke. Nothing is kept when even the last exchange doesn't fit.
pub fn drop_earliest(messages: &[Message], room: usize) -> &[Message] {
    let sizes: Vec<usize> = messages.iter().map(|message| bytes_of(message) + 1).collect();
    let mut total: usize = sizes.iter().sum::<usize>() + 1;
    let mut start = 0;
    while start < messages.len() && total > room {
        loop {
            total -= sizes[start];
            start += 1;
            if !(start < messages.len() && messages[start].role() != Role::User) {
                break;
            }
        }
    }
    &messages[start..]
}

/// The conversation's first message, marked as following a removed part (once).
pub fn note_shortened(mut messages: Vec<Message>) -> Vec<Message> {
    // Saved conversations are data from disk: anything unexpected stays as it is.
    let Some(Message::User { content, .. }) = messages.first_mut() else { return messages };
    let at = content.iter().position(|part| matches!(part, UserPart::Text(_)));
    match at {
        Some(at) => {
            let UserPart::Text(text) = &mut content[at] else { unreachable!() };
            if !text.text.starts_with(SHORTENED) {
                text.text = format!("{SHORTENED}{}", text.text);
            }
        }
        None => content.insert(0, UserPart::Text(TextPart::new(trim(SHORTENED)))),
    }
    messages
}

/// What `fit` made of the settled history and the running turn: each borrowed when nothing had to go.
#[derive(Debug, Clone, PartialEq)]
pub struct Fitted<'a> {
    pub history: Cow<'a, [Message]>,
    pub turn: Cow<'a, [Message]>,
}

impl Fitted<'_> {
    /// Whether either came back changed (the caller then keeps the result, since the clearing now belongs to the conversation).
    pub fn changed(&self) -> bool {
        matches!(self.history, Cow::Owned(_)) || matches!(self.turn, Cow::Owned(_))
    }
}

/// The settled history and the running turn, within the budget. Both come back borrowed when
/// nothing had to go; otherwise the caller keeps the result, since the clearing now belongs to the conversation.
pub fn fit<'a>(history: &'a [Message], turn: &'a [Message], budget: &ContextBudget) -> Fitted<'a> {
    // A request carries only so many images: this turn's earliest go first.
    let mut turn: Cow<'a, [Message]> = put_away_images(turn, MAX_IMAGES);
    let mut history: Cow<'a, [Message]> = Cow::Borrowed(history);
    let within = |history: &[Message], turn: &[Message], bound: f64| (bytes(history) + bytes(turn)) as f64 <= bound;
    if within(&history, &turn, budget.clear_at) {
        return Fitted { history, turn };
    }
    // Earlier turns first, keeping the one just before this whole: the producer may refer back to it.
    if let Some(cleared) = clear_until(&history, previous_turn(&history).unwrap_or(0), true) {
        history = Cow::Owned(cleared);
    }
    if within(&history, &turn, budget.limit) {
        return Fitted { history, turn };
    }
    if let Some(cleared) = clear_until(&history, history.len(), true) {
        history = Cow::Owned(cleared);
    }
    if within(&history, &turn, budget.limit) {
        return Fitted { history, turn };
    }
    // This turn's older reads, keeping its latest results and the observation it started with.
    if let Some(cleared) = clear_until(&turn, last_index(&turn, Role::Tool).unwrap_or(0), false) {
        turn = Cow::Owned(cleared);
    }
    if within(&history, &turn, budget.limit) {
        return Fitted { history, turn };
    }
    // Then whole exchanges from the front, down to three quarters of the limit so this doesn't recur on every request.
    let room = ((budget.limit * 0.75).floor() - bytes(&turn) as f64).max(0.0) as usize;
    let kept = drop_earliest(&history, room);
    if kept.len() == history.len() {
        return Fitted { history, turn };
    }
    if kept.is_empty() {
        Fitted { history: Cow::Owned(Vec::new()), turn: Cow::Owned(note_shortened(turn.into_owned())) }
    } else {
        Fitted { history: Cow::Owned(note_shortened(kept.to_vec())), turn }
    }
}
