//! Keeps a conversation within what a model can take. Live reads go stale as the producer works, so
//! they go first: earlier turns' larger tool results shrink to their opening and a note, and the Live
//! observation attached to earlier requests is dropped (the producer's words stay). Only when that
//! isn't enough do the earliest exchanges go, and even then the producer's own words in them stay, as
//! a short list where the conversation now starts. Clearing happens past thresholds, not on every
//! request, so providers' prompt caches keep working in between.

use crate::core::daw::DawProfile;
use std::borrow::Cow;

use kumi_common::js::json::stringify;
use kumi_common::js::string::{head, trim, utf16_len};
use serde_json::Value;

use super::agent::CARRY_ON_NOTE;
use crate::ai::types::{
    AssistantPart, DataContent, FileData, Message, Role, TextPart, ToolPart, ToolResultContentItem, ToolResultOutput, UserPart,
};
use crate::core::contracts::{TranscriptLine, TranscriptRole};
use crate::core::playbook::PLAYBOOK_OPEN;

// How much conversation a model takes (kumi-ai, where a model's binding names it).
pub use kumi_ai::binding::{budget_for, ContextBudget, BYTES_PER_TOKEN, DEFAULT_BUDGET};

/// How the session attaches each turn's Live observation to the producer's words.
pub const OBSERVATION_MARKER: &str = "\n\n<current_observation_untrusted>";
/// A background job's report in a turn's note (after its observation): cleared with the observation, it stays as a stub
/// that says how to read it again.
const JOB_REPORT_OPEN: &str = "<job_report_untrusted id=\"";
const JOB_REPORT_CLOSE: &str = "</job_report_untrusted>";
/// What an observation cleared with reports beside it leaves before their stubs.
const CLEARED_OBSERVATION: &str =
    "\n[Kumi cleared this observation to save room; the background jobs' reports below are cut to their opening.]\n</current_observation_untrusted>";
/// How much of a report its stub keeps.
const REPORT_STUB: usize = 300;

/// A job's report as a turn's note carries it: what a worker read is data, not the producer's words, and its tags can't be
/// faked from inside it.
pub fn job_report_block(id: u32, report: &str) -> String {
    let report = report.replace(JOB_REPORT_OPEN, "<job report id=\"").replace(JOB_REPORT_CLOSE, "</job report untrusted>");
    format!("{JOB_REPORT_OPEN}{id}\">\n{report}\n{JOB_REPORT_CLOSE}")
}

/// The stubs of the job reports in a cleared observation's tail: each one's opening and how to read it whole.
fn job_report_stubs(tail: &str) -> String {
    let mut stubs = String::new();
    let mut rest = tail;
    while let Some(start) = rest.find(JOB_REPORT_OPEN) {
        let after = &rest[start + JOB_REPORT_OPEN.len()..];
        let Some(quote) = after.find("\">") else { break };
        let id = &after[..quote];
        let body = &after[quote + 2..];
        let end = body.find(JOB_REPORT_CLOSE).unwrap_or(body.len());
        if !id.is_empty() && id.bytes().all(|b| b.is_ascii_digit()) {
            let report = trim(&body[..end]);
            let cut = opening(report, REPORT_STUB);
            let more = if cut.len() < report.len() { "…" } else { "" };
            stubs.push_str(&format!(
                "\n\n[Kumi] Background job {id}'s report, cut to its opening: {}{more} The whole report: jobs, action result, id {id}.",
                cut.split_whitespace().collect::<Vec<_>>().join(" ")
            ));
        }
        rest = &body[end..];
    }
    stubs
}
/// Starts the first kept message once the earliest exchanges are gone.
pub const SHORTENED: &str = "[Kumi removed the earlier part of this conversation to save room.]\n\n";
/// After `SHORTENED`: what the producer said in the removed part, one "- " line each, then a blank line.
pub const ASKED: &str = "[What the producer said in that part, oldest first:]\n";
/// Longest one of the producer's messages runs in that list (UTF-16 units), and the most its lines take (bytes, as sent).
const ASK: usize = 300;
const ASKS: usize = 3 * 1024;

/// A conversation's words, for showing it: what the producer said and Kumi's answers. The host
/// appends each turn's Live observation to the producer's words, and the budget may note that
/// earlier exchanges are gone (listing what the producer said in them); none of that is theirs. Nor is
/// the note asking the model to carry on an answer that broke off: that answer reads as one. Saved conversations are data from disk, so any shape is taken.
pub fn transcript_of(messages: &[Value]) -> Vec<TranscriptLine> {
    let mut said: Vec<(TranscriptRole, String, Vec<String>)> = Vec::new();
    let mut carried_on = false;
    for raw in messages {
        let role = match raw.get("role").and_then(Value::as_str) {
            Some("user") => TranscriptRole::User,
            Some("assistant") => TranscriptRole::Assistant,
            _ => continue,
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
            TranscriptRole::User => {
                split_asks(&text.split(OBSERVATION_MARKER).next().unwrap_or("").replacen(SHORTENED, "", 1)).1.to_string()
            }
            TranscriptRole::Assistant => text,
        };
        if role == TranscriptRole::User && trim(&words).ends_with(CARRY_ON_NOTE) {
            carried_on = true;
            continue;
        }
        // An answer's steps come back too: the tools it called, in order (the one call_tool ran, not call_tool).
        let tools: Vec<String> = match role {
            TranscriptRole::Assistant => parts
                .iter()
                .filter(|part| part.get("type").and_then(Value::as_str) == Some("tool-call"))
                .filter_map(|part| {
                    let name = part.get("toolName").and_then(Value::as_str)?;
                    Some(super::deferred::shown(name, part.get("input").unwrap_or(&Value::Null)).0.to_string())
                })
                .collect(),
            TranscriptRole::User => Vec::new(),
        };
        let carried = std::mem::take(&mut carried_on);
        match said.last_mut() {
            Some((TranscriptRole::Assistant, before, called)) if carried && role == TranscriptRole::Assistant => {
                before.push_str(&words);
                called.extend(tools);
            }
            _ => said.push((role, words, tools)),
        }
    }
    said.into_iter()
        .filter(|(_, words, tools)| !trim(words).is_empty() || !tools.is_empty())
        .map(|(role, words, tools)| TranscriptLine { role, text: trim(&words).to_string(), tools: (!tools.is_empty()).then_some(tools) })
        .collect()
}

/// Messages as kept on disk, with each request's Live observation taken out (background jobs' reports stay, as their
/// stubs): a conversation carried on in another Set keeps the words, not what Kumi read of the first one.
pub fn without_observations(messages: &[Value]) -> Vec<Value> {
    let cleared = |text: &str| match text.find(OBSERVATION_MARKER) {
        Some(at) if text[at + OBSERVATION_MARKER.len()..].starts_with(CLEARED_OBSERVATION) => text.to_string(),
        Some(at) => {
            let stubs = job_report_stubs(&text[at..]);
            if stubs.is_empty() {
                text[..at].to_string()
            } else {
                format!("{}{OBSERVATION_MARKER}{CLEARED_OBSERVATION}{stubs}", &text[..at])
            }
        }
        None => text.to_string(),
    };
    messages
        .iter()
        .map(|message| {
            if message.get("role").and_then(Value::as_str) != Some("user") {
                return message.clone();
            }
            let mut message = message.clone();
            match message.get_mut("content") {
                Some(Value::String(text)) => *text = cleared(text),
                Some(Value::Array(parts)) => {
                    for part in parts.iter_mut().filter(|part| part.get("type").and_then(Value::as_str) == Some("text")) {
                        if let Some(Value::String(text)) = part.get_mut("text") {
                            *text = cleared(text);
                        }
                    }
                }
                _ => {}
            }
            message
        })
        .collect()
}

crate::daw_text!(#[allow(dead_code)] CLEARED, cleared: " … [Kumi cleared the rest of this earlier result to save room; read ", app, " again if you need it.]");
/// Results this small stay whole: change confirmations, refusals, short answers.
const SMALL: usize = 1024;
/// What's kept of a cleared result; change results start with what changed.
const HEAD: usize = 200;
/// What an image costs, as bytes of conversation: a 1280×720 frame is about 1.2k tokens.
const IMAGE_BYTES: usize = 4 * 1024;
/// The most images a request carries; past this, this turn's earliest are put away.
pub const MAX_IMAGES: usize = 40;

/// Why a result's images were put away.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PutAway {
    /// Past the most a request carries, during the turn: asking for the same pictures again only puts
    /// away others, so the note says to keep what was read instead (#193: a tutorial was watched 205
    /// times in one answer, each look evicting the frames the next one asked for again).
    Room,
    /// The turn they served is over: a later request can ask for them again.
    TurnEnded,
}

fn put_away(count: usize, why: PutAway) -> String {
    let (shown, it) = if count == 1 { ("An image was".to_string(), "it") } else { (format!("{count} images were"), "them") };
    match why {
        PutAway::Room => format!(
            "[{shown} shown here and put away to make room: a request carries the latest {MAX_IMAGES}. Keep what you read from pictures in your own words as they come. Asking for the same moments again only puts away others.]"
        ),
        PutAway::TurnEnded => {
            format!("[{shown} shown here; {} no longer attached (the tool shows {it} again when asked).]", if count == 1 { "it's" } else { "they're" })
        }
    }
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
fn words_of(output: &ToolResultOutput, why: PutAway) -> String {
    let ToolResultOutput::Content { value, .. } = output else { return String::new() };
    let images = image_count(output);
    let mut lines: Vec<String> = value
        .iter()
        .filter_map(|item| if let ToolResultContentItem::Text { text, .. } = item { Some(text.clone()) } else { None })
        .collect();
    if images > 0 {
        lines.push(put_away(images, why));
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
/// keeps its words and says how many images it had, and why they went. Borrowed (the same messages)
/// when nothing changed.
pub fn put_away_images(messages: &[Message], keep: usize, why: PutAway) -> Cow<'_, [Message]> {
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
                    result.output = ToolResultOutput::Text { value: words_of(&part_output(part), why), provider_options: None };
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
    // One cleared to its jobs' report stubs no longer carries its observation.
    let observed = |text: &str| {
        text.find(OBSERVATION_MARKER).is_some_and(|at| !text[at + OBSERVATION_MARKER.len()..].starts_with(CLEARED_OBSERVATION))
    };
    matches!(message, Message::User { content, .. } if content.iter().any(|part| matches!(part, UserPart::Text(text) if observed(&text.text))))
}

/// Where the turn before this one starts: its opening message carries the Live observation, which steering doesn't.
fn previous_turn(messages: &[Message]) -> Option<usize> {
    messages.iter().rposition(has_marker).or_else(|| last_index(messages, Role::User))
}

/// A copy of messages[0, end) with large tool results cut down and, if asked, observations removed; None when nothing changed.
fn clear_until(messages: &[Message], end: usize, observations: bool, cleared_note: &str) -> Option<Vec<Message>> {
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
                                ToolResultOutput::Content { .. } => words_of(output, PutAway::TurnEnded),
                                _ => return part.clone(),
                            };
                            let content_output = matches!(output, ToolResultOutput::Content { .. });
                            let small = value.ends_with(cleared_note) || value.len() <= SMALL;
                            if !content_output && small {
                                return part.clone();
                            }
                            touched = true;
                            let kept = if small { value } else { format!("{}{cleared_note}", head(&value, HEAD)) };
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
                                // Cleared already, its jobs' reports kept as stubs.
                                Some(at) if text.text[at + OBSERVATION_MARKER.len()..].starts_with(CLEARED_OBSERVATION) => part.clone(),
                                Some(at) => {
                                    touched = true;
                                    let stubs = job_report_stubs(&text.text[at..]);
                                    let kept = if stubs.is_empty() {
                                        text.text[..at].to_string()
                                    } else {
                                        format!("{}{OBSERVATION_MARKER}{CLEARED_OBSERVATION}{stubs}", &text.text[..at])
                                    };
                                    UserPart::Text(TextPart { text: kept, provider_options: text.provider_options.clone() })
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

/// The conversation's first message, marked as following a removed part (once), with what the
/// producer said there (`asks`, oldest first) ahead of any it already listed.
pub fn note_shortened(mut messages: Vec<Message>, asks: Vec<String>) -> Vec<Message> {
    // Saved conversations are data from disk: anything unexpected stays as it is.
    let Some(Message::User { content, .. }) = messages.first_mut() else { return messages };
    let at = content.iter().position(|part| matches!(part, UserPart::Text(_)));
    match at {
        Some(at) => {
            let UserPart::Text(text) = &mut content[at] else { unreachable!() };
            let (listed, rest) = split_asks(&text.text);
            let asks = bounded([asks, listed].concat());
            text.text = format!("{SHORTENED}{}{rest}", list(&asks));
        }
        None => content.insert(0, UserPart::Text(TextPart::new(trim(&format!("{SHORTENED}{}", list(&bounded(asks))))))),
    }
    messages
}

/// What the producer said in these messages, oldest first: each of their messages' own words (not
/// the lessons or the Live observation Kumi attached, nor Kumi's own "[Kumi] …" prompts), with the
/// list an earlier removal kept in their place.
fn asks_in(messages: &[Message]) -> Vec<String> {
    let mut asks = Vec::new();
    for message in messages {
        let Message::User { content, .. } = message else { continue };
        for part in content {
            let UserPart::Text(text) = part else { continue };
            let (listed, rest) = split_asks(&text.text);
            asks.extend(listed);
            let words = rest.split(OBSERVATION_MARKER).next().unwrap_or_default();
            let words = trim(words.split(&format!("\n\n{PLAYBOOK_OPEN}")).next().unwrap_or_default());
            // A loop's or goal's request without Kumi's framing; Kumi's own notes ("[Kumi…") aren't the producer's.
            if let Some(said) = crate::core::export::asked(words) {
                let line = said.split_whitespace().collect::<Vec<_>>().join(" ");
                asks.push(if utf16_len(&line) > ASK { format!("{}…", head(&line, ASK)) } else { line });
            }
        }
    }
    asks
}

/// A message's text split into the list a removal put at its start and the rest.
pub(crate) fn split_asks(text: &str) -> (Vec<String>, &str) {
    let text = text.strip_prefix(SHORTENED).unwrap_or(text);
    let Some(listed) = text.strip_prefix(ASKED) else { return (Vec::new(), text) };
    let (block, rest) = listed.split_once("\n\n").unwrap_or((listed, ""));
    (block.lines().filter_map(|line| line.strip_prefix("- ")).map(str::to_string).collect(), rest)
}

/// The list within `ASKS` bytes: the earliest few (often the ground rules) and the latest stay, the ones
/// between go first. Two always fit (each is at most about 900 bytes).
fn bounded(mut asks: Vec<String>) -> Vec<String> {
    asks.dedup();
    while asks.len() > 2 && asks.iter().map(|ask| sent(ask) + 4).sum::<usize>() > ASKS {
        asks.remove(3.min(asks.len() - 2));
    }
    asks
}

/// Bytes a text takes in a request (as a JSON string, without its quotes).
fn sent(text: &str) -> usize {
    stringify(&Value::String(text.to_string())).len() - 2
}

fn list(asks: &[String]) -> String {
    if asks.is_empty() {
        String::new()
    } else {
        format!("{ASKED}{}\n\n", asks.iter().map(|ask| format!("- {ask}")).collect::<Vec<_>>().join("\n"))
    }
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
    fit_for(&DawProfile::ABLETON, history, turn, budget)
}

/// `fit`, with what a cleared result says in the DAW's words.
pub fn fit_for<'a>(profile: &DawProfile, history: &'a [Message], turn: &'a [Message], budget: &ContextBudget) -> Fitted<'a> {
    let cleared_note = cleared(profile);
    // A request carries only so many images: this turn's earliest go first.
    let mut turn: Cow<'a, [Message]> = put_away_images(turn, MAX_IMAGES, PutAway::Room);
    let mut history: Cow<'a, [Message]> = Cow::Borrowed(history);
    let within = |history: &[Message], turn: &[Message], bound: f64| (bytes(history) + bytes(turn)) as f64 <= bound;
    if within(&history, &turn, budget.clear_at) {
        return Fitted { history, turn };
    }
    // Earlier turns first, keeping the one just before this whole: the producer may refer back to it.
    if let Some(cleared) = clear_until(&history, previous_turn(&history).unwrap_or(0), true, &cleared_note) {
        history = Cow::Owned(cleared);
    }
    if within(&history, &turn, budget.limit) {
        return Fitted { history, turn };
    }
    if let Some(cleared) = clear_until(&history, history.len(), true, &cleared_note) {
        history = Cow::Owned(cleared);
    }
    if within(&history, &turn, budget.limit) {
        return Fitted { history, turn };
    }
    // This turn's older reads, keeping its latest results and the observation it started with.
    if let Some(cleared) = clear_until(&turn, last_index(&turn, Role::Tool).unwrap_or(0), false, &cleared_note) {
        turn = Cow::Owned(cleared);
    }
    if within(&history, &turn, budget.limit) {
        return Fitted { history, turn };
    }
    // Then whole exchanges from the front, down to three quarters of the limit so this doesn't recur on every
    // request, with room kept for the note and the list of the producer's words from them.
    let listed = sent(SHORTENED) + sent(&list(&asks_in(&history))).min(ASKS + sent(ASKED) + 4);
    let room = ((budget.limit * 0.75).floor() - (bytes(&turn) + listed) as f64).max(0.0) as usize;
    let kept = drop_earliest(&history, room);
    let asks = asks_in(&history[..history.len() - kept.len()]);
    let fitted = if kept.len() == history.len() {
        Fitted { history, turn }
    } else if kept.is_empty() {
        Fitted { history: Cow::Owned(Vec::new()), turn: Cow::Owned(note_shortened(turn.into_owned(), asks)) }
    } else {
        Fitted { history: Cow::Owned(note_shortened(kept.to_vec(), asks)), turn }
    };
    // Last, when this turn alone is past the limit: its latest results, each cut to a share of the room left.
    let total = bytes(&fitted.history) + bytes(&fitted.turn);
    if total as f64 <= budget.limit {
        return fitted;
    }
    let Some(at) = last_index(&fitted.turn, Role::Tool) else { return fitted };
    let room = (budget.limit as usize).saturating_sub(total - bytes_of(&fitted.turn[at]));
    match cut_latest(&fitted.turn, at, room) {
        Some(turn) => Fitted { history: fitted.history, turn: Cow::Owned(turn) },
        None => fitted,
    }
}

const CUT: &str = " … [Kumi cut the rest of this result: the request had no more room. Ask for less of it (a narrower read, a page, a filter) to see more.]";

/// The start of `text`, at most `bytes` long, ending between characters.
pub fn opening(text: &str, bytes: usize) -> &str {
    if text.len() <= bytes {
        return text;
    }
    let mut end = bytes;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

/// The results at `at`, each cut to an even share of `room` bytes (halving the share until they fit),
/// never below an opening of `HEAD` bytes; None when there are none.
fn cut_latest(turn: &[Message], at: usize, room: usize) -> Option<Vec<Message>> {
    let Message::Tool { content, provider_options } = &turn[at] else { return None };
    let count = content.iter().filter(|part| matches!(part, ToolPart::ToolResult(_))).count();
    if count == 0 {
        return None;
    }
    let floor = HEAD + CUT.len();
    let mut share = (room / count).max(floor);
    loop {
        let cut: Vec<ToolPart> = content
            .iter()
            .map(|part| {
                let ToolPart::ToolResult(result) = part else { return part.clone() };
                let pictures = matches!(result.output, ToolResultOutput::Content { .. });
                let value = match &result.output {
                    ToolResultOutput::Text { value, .. } | ToolResultOutput::ErrorText { value, .. } => value.clone(),
                    ToolResultOutput::Content { .. } => words_of(&result.output, PutAway::Room),
                    _ => return part.clone(),
                };
                if value.len() <= share && !pictures {
                    return part.clone();
                }
                let kept = if value.len() <= share { value } else { format!("{}{CUT}", opening(&value, share - CUT.len())) };
                let mut result = result.clone();
                result.output = match &result.output {
                    ToolResultOutput::ErrorText { provider_options, .. } => {
                        ToolResultOutput::ErrorText { value: kept, provider_options: provider_options.clone() }
                    }
                    ToolResultOutput::Text { provider_options, .. } => {
                        ToolResultOutput::Text { value: kept, provider_options: provider_options.clone() }
                    }
                    _ => ToolResultOutput::Text { value: kept, provider_options: None },
                };
                ToolPart::ToolResult(result)
            })
            .collect();
        let message = Message::Tool { content: cut, provider_options: provider_options.clone() };
        if bytes_of(&message) <= room || share == floor {
            let mut turn = turn.to_vec();
            turn[at] = message;
            return Some(turn);
        }
        share = (share / 2).max(floor);
    }
}
