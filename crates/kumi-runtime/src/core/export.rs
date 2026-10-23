//! A kept conversation as a file to keep or share: Markdown, one web page that needs nothing else, or Kumi's own JSON,
//! which another Kumi imports. What the producer and Kumi said, the steps Kumi took and what it changed; never the DAW's
//! observations, the producer's notes, Kumi's notes to the model or the model's reasoning. Every word passes the
//! redactor on its way out, and an import takes its file as untrusted.

use std::collections::{HashMap, HashSet};
use std::sync::LazyLock;

use chrono::{DateTime, FixedOffset};
use kumi_common::js::json::{file_text, stringify_pretty};
use kumi_common::js::string::{head, trim, trim_end, trim_start, utf16_len};
use serde_json::{json, Map, Value};
use unicode_normalization::UnicodeNormalization;

use super::contracts::{ChangePicture, ChangeRecord, ChangeState, KernelCheckpoint, MixLevels, PictureValue, SavedConversation};
use super::gaps::GAP_TOOL;
use super::goal::goal_setup_for;
use super::loop_run::loop_setup_for;
use super::memory::{FORGET_TOOL, REMEMBER_TOOL};
use super::playbook::PLAYBOOK_OPEN;
use super::recall::SEARCH_CONVERSATIONS_TOOL;
use super::recipes::{FORGET_RECIPE_TOOL, SAVE_RECIPE_TOOL};
use super::taste_log::REACTION_TOOL;
use super::techniques::TECHNIQUE_TOOL;
use crate::ai::types::{AssistantPart, Message, ToolPart, ToolResultContentItem, ToolResultOutput, UserPart};
use crate::kernel::agent::CARRY_ON_NOTE;
use crate::kernel::budget::{split_asks, OBSERVATION_MARKER, SHORTENED};
use crate::kernel::deferred::{shown, FIND_TOOLS};

/// What a Kumi JSON export says it is, and the version of its format.
pub const FORMAT: &str = "kumi-conversation";
pub const VERSION: u64 = 1;
/// A step's result, where the export left it out.
pub const LEFT_OUT: &str = "[left out of the export]";
/// What a trimmed conversation's export says first.
pub const PARTIAL: &str = "The earlier part of this conversation wasn't kept: Kumi keeps the latest part of a long one.";
/// The most an import reads.
pub const IMPORT_MAX_BYTES: usize = 16 * 1024 * 1024;
const MAX_MESSAGES: usize = 20_000;
const MAX_CHANGES: usize = 500;
/// The producer's first words in an imported conversation that starts with Kumi's answer.
const PARTWAY: &str = "(This conversation starts partway.)";
/// The longest a title runs, in UTF-16 units.
const MAX_TITLE: usize = 60;
/// The longest any of an imported change's words run, in UTF-16 units.
const MAX_WORDS: usize = 300;
/// The longest an imported message's words or a step's result run (UTF-16 units), and the most a step's input takes as
/// JSON (bytes): what's past them is cut, or, for an input, left out.
const MAX_PART: usize = 64 * 1024;
const MAX_INPUT: usize = 16 * 1024;
/// How deep an imported step's input may nest. Kumi's own steps nest a few levels; a kept conversation is read back with
/// JSON's usual limit (128 levels), its inputs 7 levels down, so one past it would be kept but never read again.
const MAX_DEPTH: usize = 64;

/// The tools whose calls never leave Kumi: what it keeps of the producer (notes, recipes, techniques, how something went
/// down), its notes for whoever makes Kumi (gaps), and how it finds its own tools.
const NEVER: [&str; 8] =
    [REMEMBER_TOOL, FORGET_TOOL, SAVE_RECIPE_TOOL, FORGET_RECIPE_TOOL, REACTION_TOOL, TECHNIQUE_TOOL, GAP_TOOL, FIND_TOOLS];
/// Tools whose step shows, but not what they found: a search of earlier conversations finds other conversations' words.
const STEP_ONLY: [&str; 1] = [SEARCH_CONVERSATIONS_TOOL];

/// What a conversation is saved as.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Markdown,
    Html,
    Json,
}

impl Format {
    pub fn extension(self) -> &'static str {
        match self {
            Format::Markdown => "md",
            Format::Html => "html",
            Format::Json => "json",
        }
    }
}

/// A kept conversation and what's known of it, to export.
pub struct Source<'a> {
    pub id: &'a str,
    pub conversation: &'a SavedConversation,
    /// What it's called: the producer's name for it, else its first request.
    pub title: &'a str,
    /// Its place's name (the Set's), when Kumi has seen it.
    pub place: Option<&'a str>,
    /// What the producer calls the DAW and a place in it: ("Live", "Set").
    pub words: (&'a str, &'a str),
    /// When it began (ms since the epoch).
    pub created: i64,
}

/// How a conversation is exported.
pub struct Options<'a> {
    /// Each step's input and result too.
    pub full: bool,
    /// What every word passes on its way out: secrets, the home folder and the account name taken out. The identity
    /// when the producer asked for the file as it is.
    pub redact: &'a dyn Fn(&str) -> String,
    /// Whether `redact` takes anything out, for the file to say.
    pub redacted: bool,
    /// A step's words, from its tool's name: "looked at your Set".
    pub label: &'a dyn Fn(&str) -> String,
    /// A change's picture for the web page, drawn from its record (an `<svg>`, or a line naming devices); None draws none.
    pub picture: &'a dyn Fn(&ChangeRecord) -> Option<String>,
    /// Where dates are told: the producer's time zone.
    pub zone: FixedOffset,
    /// When it's exported (ms since the epoch).
    pub now: i64,
    /// The Kumi that exports it.
    pub version: &'a str,
}

/// A conversation as an export tells it: its words, steps and changes, every word through the redactor.
struct Document {
    title: String,
    place: Option<String>,
    model: Option<String>,
    requests: u32,
    partial: bool,
    /// What the producer said in the part a trimmed conversation dropped, oldest first.
    earlier: Vec<String>,
    /// As a model reads them: what was said, the steps and their results (or that they were left out).
    messages: Vec<Value>,
    said: Vec<Said>,
    changes: Vec<ChangeRecord>,
}

enum Said {
    Producer(String),
    Kumi { words: String, steps: Vec<Step> },
}

struct Step {
    id: String,
    label: String,
    input: Option<String>,
    result: Option<String>,
}

/// A call as the export keeps it: its new id, the name it goes by, and whether its result stays out.
struct Call {
    id: String,
    name: String,
    step_only: bool,
}

/// The messages with only what may leave Kumi.
struct Cleaned {
    messages: Vec<Value>,
    earlier: Vec<String>,
    partial: bool,
}

/// The conversation as Markdown: a heading, what's known of it, then each turn and what Kumi changed. It holds no control
/// characters but line breaks and tabs.
pub fn markdown(source: &Source, options: &Options) -> String {
    let doc = read(source, options);
    let mut out = format!("# {}\n\n", markdown_safe(&one_line(&doc.title)));
    for (label, value) in facts(&doc, source, options) {
        out.push_str(&format!("- **{label}:** {}\n", markdown_safe(&value)));
    }
    out.push_str(&format!("\n> {}\n\n", notes(options).join(" ")));
    if doc.partial {
        out.push_str(&format!("> {PARTIAL}\n"));
        if !doc.earlier.is_empty() {
            out.push_str(">\n> What you asked in that part:\n");
            for ask in &doc.earlier {
                out.push_str(&format!("> - {}\n", markdown_safe(&one_line(ask))));
            }
        }
        out.push('\n');
    }
    for said in &doc.said {
        match said {
            Said::Producer(words) => out.push_str(&format!("## You\n\n{}\n\n", markdown_safe(words))),
            Said::Kumi { words, steps } => {
                out.push_str("## Kumi\n\n");
                if !words.is_empty() {
                    out.push_str(&format!("{}\n\n", markdown_safe(words)));
                }
                if steps.is_empty() {
                    continue;
                }
                if options.full {
                    for step in steps {
                        out.push_str(&format!("<details><summary>{}</summary>\n\n", escape(&step.label)));
                        if let Some(input) = &step.input {
                            out.push_str(&fenced(input, "json"));
                        }
                        if let Some(result) = &step.result {
                            out.push_str(&fenced(result, ""));
                        }
                        out.push_str("</details>\n\n");
                    }
                } else {
                    let labels: Vec<&str> = steps.iter().map(|step| step.label.as_str()).collect();
                    out.push_str(&format!("*{}: {}*\n\n", steps_word(steps.len()), markdown_safe(&labels.join(" · "))));
                }
            }
        }
    }
    if !doc.changes.is_empty() {
        out.push_str(&format!("## What Kumi changed in {}\n\n", markdown_safe(source.words.0)));
        for change in &doc.changes {
            out.push_str(&format!("- {}\n", markdown_safe(&change_line(change, source.words.0))));
            for value in change.picture.as_ref().map(values_of).unwrap_or_default() {
                out.push_str(&format!("  - {}\n", markdown_safe(&value)));
            }
        }
        out.push('\n');
    }
    out.push_str(&format!("---\n\n{}\n", footer(options)));
    // Read in a terminal (cat) or pasted from the copy, it moves and sets nothing: no control characters but line breaks
    // and tabs.
    out.chars().filter(|c| !c.is_control() || *c == '\n' || *c == '\t').collect()
}

/// What a page can load and run: nothing from anywhere, and no scripts; only its own styles. Neither a base for its links
/// nor a form's target falls back to `default-src`, so each is named.
const POLICY: &str = "default-src 'none'; style-src 'unsafe-inline'; base-uri 'none'; form-action 'none'";

/// The conversation as one web page: everything it shows is in the file, light or dark as the reader's system is, and
/// readable on a phone. Each answer's steps fold away.
pub fn html(source: &Source, options: &Options) -> String {
    let doc = read(source, options);
    let title = escape(&one_line(&doc.title));
    let mut body = format!("<header>\n<p class=\"kicker\">Kumi conversation</p>\n<h1>{title}</h1>\n<dl>");
    for (label, value) in facts(&doc, source, options) {
        body.push_str(&format!("<div><dt>{}</dt><dd>{}</dd></div>", escape(&label), escape(&value)));
    }
    body.push_str(&format!("</dl>\n<p class=\"notes\">{}</p>\n</header>\n", escape(&notes(options).join(" "))));
    if doc.partial {
        body.push_str(&format!("<aside class=\"partial\"><p>{}</p>", escape(PARTIAL)));
        if !doc.earlier.is_empty() {
            body.push_str("<p>What you asked in that part:</p><ul>");
            for ask in &doc.earlier {
                body.push_str(&format!("<li>{}</li>", escape(&one_line(ask))));
            }
            body.push_str("</ul>");
        }
        body.push_str("</aside>\n");
    }
    body.push_str("<section class=\"conversation\">\n");
    for said in &doc.said {
        match said {
            Said::Producer(words) => {
                body.push_str(&format!("<article class=\"you\"><h2>You</h2><div class=\"words\">{}</div></article>\n", plain_html(words)))
            }
            Said::Kumi { words, steps } => {
                body.push_str("<article class=\"kumi\"><h2>Kumi</h2>");
                if !words.is_empty() {
                    body.push_str(&format!("<div class=\"words\">{}</div>", markdown_html(words)));
                }
                if !steps.is_empty() && options.full {
                    body.push_str("<div class=\"steps\">");
                    for step in steps {
                        body.push_str(&format!("<details><summary>{}</summary>", escape(&step.label)));
                        if let Some(input) = &step.input {
                            body.push_str(&format!("<p class=\"what\">Input</p><pre>{}</pre>", escape(input)));
                        }
                        if let Some(result) = &step.result {
                            body.push_str(&format!("<p class=\"what\">Result</p><pre>{}</pre>", escape(result)));
                        }
                        body.push_str("</details>");
                    }
                    body.push_str("</div>");
                } else if !steps.is_empty() {
                    let items: String = steps.iter().map(|step| format!("<li>{}</li>", escape(&step.label))).collect();
                    body.push_str(&format!(
                        "<details class=\"steps\"><summary>{} {}</summary><ul>{items}</ul></details>",
                        steps.len(),
                        steps_word(steps.len()).to_lowercase()
                    ));
                }
                body.push_str("</article>\n");
            }
        }
    }
    body.push_str("</section>\n");
    if !doc.changes.is_empty() {
        body.push_str(&format!("<section class=\"changes\">\n<h2>What Kumi changed in {}</h2>\n<ul>", escape(source.words.0)));
        for change in &doc.changes {
            body.push_str(&format!("<li><p>{}</p>", escape(&change_line(change, source.words.0))));
            let drawn = (options.picture)(change);
            let values = change.picture.as_ref().map(values_of).unwrap_or_default();
            if drawn.is_some() || !values.is_empty() {
                body.push_str("<figure>");
                if let Some(picture) = drawn {
                    body.push_str(&picture);
                }
                if !values.is_empty() {
                    let lines: Vec<String> = values.iter().map(|line| escape(line)).collect();
                    body.push_str(&format!("<figcaption>{}</figcaption>", lines.join("<br>")));
                }
                body.push_str("</figure>");
            }
            body.push_str("</li>");
        }
        body.push_str("</ul>\n</section>\n");
    }
    format!(
        "<!doctype html>\n<html lang=\"en\">\n<head>\n<meta charset=\"utf-8\">\n<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n<meta http-equiv=\"Content-Security-Policy\" content=\"{POLICY}\">\n<meta name=\"referrer\" content=\"no-referrer\">\n<meta name=\"generator\" content=\"Kumi {}\">\n<title>{}</title>\n<style>\n{STYLE}</style>\n</head>\n<body>\n<main>\n{body}</main>\n<footer>{}</footer>\n</body>\n</html>\n",
        escape(options.version),
        title,
        escape(&footer(options)),
    )
}

/// The conversation as Kumi's own JSON, which `kumi import` reads: what's known of it, its messages as a model reads them
/// (no reasoning), and its changes.
pub fn json(source: &Source, options: &Options) -> String {
    let doc = read(source, options);
    let mut root = Map::new();
    root.insert("format".into(), FORMAT.into());
    root.insert("version".into(), VERSION.into());
    root.insert("id".into(), source.id.into());
    root.insert("title".into(), doc.title.clone().into());
    if let Some(place) = &doc.place {
        root.insert("set".into(), place.clone().into());
    }
    root.insert("daw".into(), source.words.0.into());
    root.insert("noun".into(), source.words.1.into());
    if let Some(model) = &doc.model {
        root.insert("model".into(), model.clone().into());
    }
    root.insert("created".into(), source.created.into());
    root.insert("saved".into(), source.conversation.saved_at.into());
    root.insert("exported".into(), options.now.into());
    root.insert("requests".into(), doc.requests.into());
    root.insert("kumi".into(), options.version.into());
    root.insert("full".into(), options.full.into());
    root.insert("redacted".into(), options.redacted.into());
    if doc.partial {
        root.insert("partial".into(), true.into());
        root.insert("earlier".into(), json!(doc.earlier));
    }
    root.insert("messages".into(), Value::Array(pair(doc.messages)));
    root.insert("changes".into(), serde_json::to_value(&doc.changes).unwrap_or_else(|_| Value::Array(vec![])));
    file_text(&Value::Object(root))
}

/// A name for an export's file: `kumi-<the title's words>-<id>.<ext>`, the title as lowercase ASCII words (none for one
/// written in other letters), at most 40 characters of them.
pub fn file_name(title: &str, id: &str, format: Format) -> String {
    let mut words = String::new();
    for word in title.split(|c: char| !c.is_ascii_alphanumeric()).filter(|word| !word.is_empty()) {
        if words.len() + word.len() + 1 > 40 {
            break;
        }
        if !words.is_empty() {
            words.push('-');
        }
        words.push_str(&word.to_ascii_lowercase());
    }
    let id: String = id.chars().filter(char::is_ascii_alphanumeric).collect();
    if words.is_empty() {
        format!("kumi-{id}.{}", format.extension())
    } else {
        format!("kumi-{words}-{id}.{}", format.extension())
    }
}

fn read(source: &Source, options: &Options) -> Document {
    // Every word leaves as a reader sees it before the redactor takes secrets out of it.
    let words = |text: &str| (options.redact)(&drawn(text));
    let options = &Options { redact: &words, ..*options };
    let redact = options.redact;
    let cleaned = clean(&source.conversation.checkpoint.messages, options);
    let said = said_of(&cleaned.messages, options);
    let producer = said.iter().filter(|said| matches!(said, Said::Producer(_))).count() as u32;
    let changes = source.conversation.changes.iter().flatten().map(|change| redacted_change(change, redact)).collect();
    Document {
        title: redact(source.title),
        place: source.place.map(|place| redact(place)),
        model: source.conversation.checkpoint.origin.as_deref().map(|model| redact(model)),
        requests: source.conversation.turns.unwrap_or(producer).max(producer),
        partial: cleaned.partial,
        earlier: cleaned.earlier,
        messages: cleaned.messages,
        said,
        changes,
    }
}

/// Words as they're drawn: without control characters but line breaks and tabs (a terminal acts on them when a file is
/// read in one, or its copy pasted), and without characters that draw nothing but can hide words or turn them around.
/// Every word an export writes, and every word an import reads, is this.
fn drawn(text: &str) -> String {
    text.chars().filter(|c| (!c.is_control() || *c == '\n' || *c == '\t') && !invisible(*c)).collect()
}

/// The messages with what may leave Kumi: the producer's words (see [`producer_words`]), Kumi's words and steps, and each
/// step's result when the export is full. No reasoning, pictures or provider data. Results the provider ran itself
/// follow their message as the tool's. Call ids are numbered afresh: a redactor could make two long ones alike.
fn clean(messages: &[Value], options: &Options) -> Cleaned {
    let redact = options.redact;
    let mut out: Vec<Value> = Vec::new();
    let mut earlier = Vec::new();
    let mut partial = false;
    let mut calls: HashMap<String, Call> = HashMap::new();
    let mut left_out: HashSet<String> = HashSet::new();
    let mut carried_on = false;
    for message in messages {
        let parts = parts_of(message);
        match message.get("role").and_then(Value::as_str) {
            Some("user") => {
                let pictures = parts.iter().filter(|part| kind(part) == "file").count();
                match producer_words(&texts(&parts), pictures, &mut earlier, &mut partial) {
                    Words::Said(words) => {
                        carried_on = false;
                        out.push(json!({"role": "user", "content": [{"type": "text", "text": words}]}));
                    }
                    Words::CarryOn => carried_on = true,
                    Words::Kumi => carried_on = false,
                }
            }
            Some("assistant") => {
                let mut content: Vec<Value> = Vec::new();
                let mut ran: Vec<Value> = Vec::new();
                for part in &parts {
                    match kind(part) {
                        "text" => {
                            let text = part.get("text").and_then(Value::as_str).unwrap_or("");
                            if !trim(text).is_empty() {
                                content.push(json!({"type": "text", "text": text}));
                            }
                        }
                        "tool-call" => content.extend(call_of(part, options, &mut calls, &mut left_out)),
                        "tool-result" => ran.extend(result_of(part, options, &calls, &left_out)),
                        _ => {}
                    }
                }
                if content.is_empty() {
                    continue;
                }
                // The answer a dropped connection broke off goes on in the same message, as it reads.
                add(&mut out, "assistant", content, std::mem::take(&mut carried_on));
                if !ran.is_empty() {
                    add(&mut out, "tool", ran, false);
                }
            }
            Some("tool") => {
                let content: Vec<Value> = parts
                    .iter()
                    .filter(|part| kind(part) == "tool-result")
                    .filter_map(|part| result_of(part, options, &calls, &left_out))
                    .collect();
                if !content.is_empty() {
                    add(&mut out, "tool", content, true);
                }
            }
            _ => {}
        }
    }
    // Words that run on (an answer a dropped connection broke off) are one text, so the redactor sees a key whole.
    for message in &mut out {
        let Some(Value::Array(parts)) = message.get_mut("content") else { continue };
        let mut joined: Vec<Value> = Vec::with_capacity(parts.len());
        for part in parts.drain(..) {
            if kind(&part) == "text" && joined.last().is_some_and(|last| kind(last) == "text") {
                let last = joined.last_mut().expect("a text before");
                let text = format!("{}{}", last["text"].as_str().unwrap_or(""), part["text"].as_str().unwrap_or(""));
                last["text"] = Value::String(text);
            } else {
                joined.push(part);
            }
        }
        for part in joined.iter_mut().filter(|part| kind(part) == "text") {
            let text = redact(part["text"].as_str().unwrap_or(""));
            part["text"] = Value::String(text);
        }
        *parts = joined;
    }
    Cleaned { messages: out, earlier: earlier.iter().map(|ask| redact(ask)).collect(), partial }
}

/// Adds `content` to the conversation: to its last message when that's `role`'s and `join` holds, else as a message of its
/// own.
fn add(out: &mut Vec<Value>, role: &str, content: Vec<Value>, join: bool) {
    let same = out.last().is_some_and(|last| last.get("role").and_then(Value::as_str) == Some(role));
    if join && same {
        if let Some(Value::Array(before)) = out.last_mut().and_then(|last| last.get_mut("content")) {
            before.extend(content);
            return;
        }
    }
    out.push(json!({"role": role, "content": content}));
}

/// What one of the producer's messages comes to in an export.
enum Words {
    /// What they said.
    Said(String),
    /// Kumi's note asking the model to carry on an answer a dropped connection broke off: the answer reads as one.
    CarryOn,
    /// Kumi's own note to the model ("[Kumi] …": a background job's report, a match run's next round), or nothing.
    Kumi,
}

/// The producer's words in a message of theirs, as the transcript shows them: without the Live observation, the lessons
/// or a removal's list that Kumi sent with them (the list goes to `earlier`), a loop's or a goal's request without Kumi's
/// framing, and the pictures they attached as a count.
fn producer_words(text: &str, pictures: usize, earlier: &mut Vec<String>, partial: &mut bool) -> Words {
    let words = text.split(OBSERVATION_MARKER).next().unwrap_or("");
    // A message with only pictures took the note as a part of its own, without the blank line.
    let unshortened = words.replacen(SHORTENED, "", 1);
    let unshortened = unshortened.strip_prefix(trim(SHORTENED)).map(str::to_string).unwrap_or(unshortened);
    *partial |= unshortened.len() != words.len();
    let (asks, rest) = split_asks(&unshortened);
    *partial |= !asks.is_empty();
    earlier.extend(asks.iter().filter_map(|ask| asked(ask)));
    let rest = trim(rest.split(&format!("\n\n{PLAYBOOK_OPEN}")).next().unwrap_or(""));
    if rest.ends_with(CARRY_ON_NOTE) {
        return Words::CarryOn;
    }
    let Some(mut said) = asked(rest) else { return Words::Kumi };
    if pictures > 0 {
        let note = if pictures == 1 { "[a picture]".to_string() } else { format!("[{pictures} pictures]") };
        said = if said.is_empty() { note } else { format!("{said}\n\n{note}") };
    }
    if said.is_empty() {
        Words::Kumi
    } else {
        Words::Said(said)
    }
}

/// What the producer asked, in words Kumi sends the model: theirs, a loop's or a goal's request without Kumi's framing
/// ("Loop: …", "Goal: …"), or None for Kumi's own words to the model ("[Kumi…": a round's next step, a check, a job's
/// report, the carry-on note). Kumi frames a request in the words of the DAW it ran in, so every registered DAW's framing
/// is tried, Live's first: a request of several lines framed in REAPER's, Renoise's or Logic Pro's words is the
/// producer's too, in an export, an import and the budget's list.
pub(crate) fn asked(words: &str) -> Option<String> {
    let words = trim(words);
    let profiles = crate::integrations::REGISTERED;
    if let Some(request) = profiles.iter().find_map(|profile| framed(words, &loop_setup_for(profile, ""))) {
        return Some(format!("Loop: {request}"));
    }
    if let Some(goal) = profiles.iter().find_map(|profile| framed(words, &goal_setup_for(profile, ""))) {
        return Some(format!("Goal: {goal}"));
    }
    (!words.is_empty() && !kumis(words)).then(|| words.to_string())
}

/// Whether words open with Kumi's mark for its own notes to the model ("[Kumi"), in any case, form or width, with white
/// space and invisible characters aside.
pub(crate) fn kumis(words: &str) -> bool {
    let mut chars = words.chars().filter(|c| !invisible(*c)).skip_while(|c| c.is_whitespace());
    chars.next().is_some_and(|c| folded(c) == "[") && spells_kumi(&chars.take(16).collect::<Vec<_>>())
}

/// A character as a reader takes it in: its compatibility form (fullwidth and styled letters, brackets that read as "["),
/// in lowercase.
fn folded(c: char) -> String {
    if c.is_ascii() {
        return c.to_ascii_lowercase().to_string();
    }
    std::iter::once(c).nfkc().collect::<String>().to_lowercase()
}

/// Whether characters spell "kumi" in any case, form or width, after white space and invisible characters.
fn spells_kumi(chars: &[char]) -> bool {
    let mut spelled = String::new();
    for c in chars.iter().filter(|c| !invisible(**c)).skip_while(|c| c.is_whitespace()) {
        spelled.push_str(&folded(*c));
        if spelled.len() >= 4 {
            break;
        }
    }
    spelled.starts_with("kumi")
}

/// Text from a file with Kumi's mark taken off wherever it stands: "[Kumi", in any case, form or width, becomes "(Kumi", so
/// nothing in it reads as Kumi's own words to the model.
fn neutralized(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    for (at, c) in chars.iter().enumerate() {
        let opens = (*c == '[' || !c.is_ascii()) && folded(*c) == "[";
        out.push(if opens && spells_kumi(&chars[at + 1..chars.len().min(at + 17)]) { '(' } else { *c });
    }
    out
}

/// The producer's request inside Kumi's framing of it (`setup`, made with an empty request: "[Kumi loop] " then what Kumi
/// asks), or None for other words. An older Kumi's framing is a one-line request, a blank line, then what Kumi asked; a
/// later round's note (a line of its own, then the rest) isn't a request.
fn framed<'a>(text: &'a str, setup: &str) -> Option<&'a str> {
    let (opening, ending) = setup.split_at(setup.find("\n\n")?);
    let request = text.strip_prefix(opening)?;
    if let Some(request) = request.strip_suffix(ending) {
        return Some(trim(request));
    }
    let (request, _) = request.split_once("\n\n")?;
    (!request.contains('\n')).then(|| trim(request))
}

/// A call as the export keeps it: under its tool's own name with nothing of its input, or as it was (redacted) in a full
/// export. None for a tool whose calls never leave Kumi.
fn call_of(part: &Value, options: &Options, calls: &mut HashMap<String, Call>, left_out: &mut HashSet<String>) -> Option<Value> {
    let id = part.get("toolCallId").and_then(Value::as_str).unwrap_or("").to_string();
    let name = part.get("toolName").and_then(Value::as_str).unwrap_or("");
    let input = part.get("input").filter(|input| !input.is_null()).or_else(|| part.get("args")).cloned().unwrap_or(Value::Null);
    let (tool, _) = shown(name, &input);
    if NEVER.contains(&tool) {
        left_out.insert(id);
        return None;
    }
    let number = format!("call_{}", calls.len() + 1);
    // The name call_tool ran is the model's words: it passes the redactor too.
    let (kept, input) = if options.full {
        let named = options.redacted.then(|| named_secrets_out(&input));
        (name.to_string(), redact_value(named.as_ref().unwrap_or(&input), options.redact))
    } else {
        ((options.redact)(tool), json!({}))
    };
    calls.insert(id, Call { id: number.clone(), name: kept.clone(), step_only: STEP_ONLY.contains(&tool) });
    Some(json!({"type": "tool-call", "toolCallId": number, "toolName": kept, "input": input}))
}

/// A result as the export keeps it: as words (redacted) in a full export, else that it was left out. None without its
/// call.
fn result_of(part: &Value, options: &Options, calls: &HashMap<String, Call>, left_out: &HashSet<String>) -> Option<Value> {
    let id = part.get("toolCallId").and_then(Value::as_str).unwrap_or("");
    if left_out.contains(id) {
        return None;
    }
    let call = calls.get(id)?;
    let text = if options.full && !call.step_only {
        let output = part.get("output").filter(|output| !output.is_null()).or_else(|| part.get("result")).cloned().unwrap_or(Value::Null);
        let text = (options.redact)(&result_text(&output, options.redacted));
        if trim(&text).is_empty() {
            "(nothing)".to_string()
        } else {
            text
        }
    } else {
        LEFT_OUT.to_string()
    };
    Some(json!({"type": "tool-result", "toolCallId": call.id, "toolName": call.name, "output": {"type": "text", "value": text}}))
}

/// What the producer and Kumi said, and Kumi's steps, from the cleaned messages: each answer's words and its steps' labels
/// (with their inputs and results in a full export). An answer's words read on where they broke off; a step or another
/// message between them starts a new paragraph.
fn said_of(messages: &[Value], options: &Options) -> Vec<Said> {
    let mut said: Vec<Said> = Vec::new();
    for message in messages {
        let parts = parts_of(message);
        match message.get("role").and_then(Value::as_str) {
            Some("user") => said.push(Said::Producer(trim(&texts(&parts)).to_string())),
            Some("assistant") => {
                if !matches!(said.last(), Some(Said::Kumi { .. })) {
                    said.push(Said::Kumi { words: String::new(), steps: Vec::new() });
                }
                let Some(Said::Kumi { words, steps }) = said.last_mut() else { continue };
                let mut paragraph = true;
                for part in &parts {
                    match kind(part) {
                        "text" => {
                            let text = part.get("text").and_then(Value::as_str).unwrap_or("");
                            if paragraph && !trim(words).is_empty() {
                                let end = trim_end(words).len();
                                words.truncate(end);
                                words.push_str("\n\n");
                                words.push_str(trim_start(text));
                            } else {
                                words.push_str(text);
                            }
                            paragraph = false;
                        }
                        "tool-call" => {
                            paragraph = true;
                            let name = part.get("toolName").and_then(Value::as_str).unwrap_or("");
                            let input = part.get("input").cloned().unwrap_or(Value::Null);
                            let (tool, inner) = shown(name, &input);
                            steps.push(Step {
                                id: part.get("toolCallId").and_then(Value::as_str).unwrap_or("").to_string(),
                                label: (options.redact)(&(options.label)(tool)),
                                input: options.full.then(|| stringify_pretty(inner, 2)),
                                result: None,
                            });
                        }
                        _ => {}
                    }
                }
            }
            Some("tool") => {
                let Some(Said::Kumi { steps, .. }) = said.last_mut() else { continue };
                for part in parts.iter().filter(|part| kind(part) == "tool-result") {
                    attach(steps, part, options);
                }
            }
            _ => {}
        }
    }
    for entry in &mut said {
        if let Said::Kumi { words, .. } = entry {
            *words = trim(words).to_string();
        }
    }
    said
}

/// A step's result, beside its call, in a full export.
fn attach(steps: &mut [Step], part: &Value, options: &Options) {
    if !options.full {
        return;
    }
    let id = part.get("toolCallId").and_then(Value::as_str).unwrap_or("");
    if let Some(step) = steps.iter_mut().rev().find(|step| step.id == id) {
        step.result = part.get("output").and_then(|output| output.get("value")).and_then(Value::as_str).map(str::to_string);
    }
}

/// A message's parts: older saves keep a message's words as a string.
fn parts_of(message: &Value) -> Vec<Value> {
    match message.get("content") {
        Some(Value::String(text)) => vec![json!({"type": "text", "text": text})],
        Some(Value::Array(parts)) => parts.clone(),
        _ => Vec::new(),
    }
}

fn kind(part: &Value) -> &str {
    part.get("type").and_then(Value::as_str).unwrap_or("")
}

fn texts(parts: &[Value]) -> String {
    parts.iter().filter(|part| kind(part) == "text").filter_map(|part| part.get("text").and_then(Value::as_str)).collect()
}

/// A tool's result as words: its text, its JSON laid out, an error's words, a picture named. With `redacted`, what its JSON
/// (or JSON its words hold) keeps under a field named for a secret is taken out.
fn result_text(output: &Value, redacted: bool) -> String {
    let value = output.get("value");
    let words = |value: Option<&Value>| {
        let text = value.and_then(Value::as_str).unwrap_or("");
        if redacted {
            named_secrets_out_of_words(text)
        } else {
            text.to_string()
        }
    };
    let laid_out_whole = |value: &Value| stringify_pretty(&if redacted { named_secrets_out(value) } else { value.clone() }, 2);
    let laid_out = |value: Option<&Value>| value.map(laid_out_whole).unwrap_or_default();
    match output.get("type").and_then(Value::as_str) {
        Some("text") => words(value),
        Some("error-text") => format!("Error: {}", words(value)),
        Some("json") => laid_out(value),
        Some("error-json") => format!("Error: {}", laid_out(value)),
        Some("execution-denied") => {
            format!("Not run{}", output.get("reason").and_then(Value::as_str).map(|why| format!(": {why}")).unwrap_or_default())
        }
        Some("content") => value
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(|item| match kind(item) {
                        "text" => item.get("text").filter(|text| text.is_string()).map(|text| words(Some(text))),
                        "file" => Some("[a picture]".to_string()),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join("\n")
            })
            .unwrap_or_default(),
        _ => match output {
            Value::String(_) => words(Some(output)),
            Value::Null => String::new(),
            _ => laid_out_whole(output),
        },
    }
}

/// The words of an object's field name, split at separators and changes of case, in lowercase: "X-Auth-Token" → x, auth,
/// token; "apiKey" → api, key; "AWSSecretKey" → aws, secret, key.
fn name_words(name: &str) -> Vec<String> {
    let chars: Vec<char> = name.chars().collect();
    let mut words: Vec<String> = Vec::new();
    let mut word = String::new();
    for (at, &c) in chars.iter().enumerate() {
        if !c.is_alphanumeric() {
            if !word.is_empty() {
                words.push(std::mem::take(&mut word));
            }
            continue;
        }
        let before = at.checked_sub(1).map(|at| chars[at]).filter(|c| c.is_alphanumeric());
        let after = chars.get(at + 1).copied();
        let starts = before.is_some_and(|before| {
            c.is_uppercase()
                && (before.is_lowercase() || before.is_numeric() || (before.is_uppercase() && after.is_some_and(char::is_lowercase)))
        });
        if starts && !word.is_empty() {
            words.push(std::mem::take(&mut word));
        }
        word.extend(c.to_lowercase());
    }
    if !word.is_empty() {
        words.push(word);
    }
    words
}

/// Whether a field's name says it holds a secret (password, api_key, accessKey, X-Auth-Token, Cookie…), in any case and
/// with any separator. Whole words only: an author or a count of tokens isn't one.
fn names_a_secret(name: &str) -> bool {
    const WORDS: [&str; 16] = [
        "password",
        "passwd",
        "pwd",
        "passphrase",
        "secret",
        "secrets",
        "token",
        "auth",
        "authorization",
        "credential",
        "credentials",
        "cookie",
        "cookies",
        "apikey",
        "accesskey",
        "privatekey",
    ];
    let words = name_words(name);
    words.iter().any(|word| WORDS.contains(&word.as_str()))
        || words.windows(2).any(|pair| matches!((pair[0].as_str(), pair[1].as_str()), ("api" | "access" | "private", "key")))
}

/// A value with every string under a field named for a secret taken out, however short or plain it is.
fn named_secrets_out(value: &Value) -> Value {
    fn hide(value: &Value, under: bool) -> Value {
        match value {
            Value::String(text) if under && !text.is_empty() => Value::String("[secret]".into()),
            Value::Array(items) => Value::Array(items.iter().map(|item| hide(item, under)).collect()),
            Value::Object(fields) => {
                Value::Object(fields.iter().map(|(name, item)| (name.clone(), hide(item, under || names_a_secret(name)))).collect())
            }
            other => other.clone(),
        }
    }
    hide(value, false)
}

/// A result's words with what the JSON they hold keeps under a field named for a secret taken out where it stands, the
/// words otherwise as they were. Words that aren't JSON stay as they are.
fn named_secrets_out_of_words(text: &str) -> String {
    fn named(value: &Value, under: bool, out: &mut Vec<String>) {
        match value {
            Value::String(text) if under && !text.is_empty() => out.push(text.clone()),
            Value::Array(items) => items.iter().for_each(|item| named(item, under, out)),
            Value::Object(fields) => fields.iter().for_each(|(name, item)| named(item, under || names_a_secret(name), out)),
            _ => {}
        }
    }
    let Ok(value) = serde_json::from_str::<Value>(text) else { return text.to_string() };
    let mut secrets = Vec::new();
    named(&value, false, &mut secrets);
    // The longest first, so one inside another goes whole.
    secrets.sort_by_key(|secret| std::cmp::Reverse(secret.len()));
    secrets.dedup();
    let mut out = text.to_string();
    for secret in secrets {
        out = out.replace(&Value::String(secret).to_string(), "\"[secret]\"");
    }
    out
}

/// A value with each of its strings, and its keys, through the redactor.
fn redact_value(value: &Value, redact: &dyn Fn(&str) -> String) -> Value {
    match value {
        Value::String(text) => Value::String(redact(text)),
        Value::Array(items) => Value::Array(items.iter().map(|item| redact_value(item, redact)).collect()),
        Value::Object(fields) => Value::Object(fields.iter().map(|(key, item)| (redact(key), redact_value(item, redact))).collect()),
        other => other.clone(),
    }
}

/// A change with its words through the redactor (its title, its track's name, its devices' names and its picture's names
/// and values), and without the note on why Kumi can or can't undo it.
fn redacted_change(change: &ChangeRecord, redact: &dyn Fn(&str) -> String) -> ChangeRecord {
    let mut change = change.clone();
    change.title = redact(&change.title);
    // A note says why Kumi can or can't undo it here: nothing for a reader elsewhere.
    change.note = None;
    if let Some(track) = &mut change.track {
        track.name = redact(&track.name);
    }
    if let Some(devices) = &mut change.devices {
        if let Some(names) = &mut devices.devices {
            names.iter_mut().for_each(|name| *name = redact(name));
        }
        devices.rack = devices.rack.as_deref().map(|rack| redact(rack));
        if let Some(chains) = &mut devices.chains {
            for chain in chains {
                chain.name = redact(&chain.name);
                chain.devices.iter_mut().for_each(|name| *name = redact(name));
            }
        }
    }
    if let Some(picture) = &mut change.picture {
        let values = |values: &mut Vec<PictureValue>| {
            for value in values {
                value.name = redact(&value.name);
                for text in [&mut value.from_text, &mut value.to_text, &mut value.asked] {
                    *text = text.as_deref().map(|text| redact(text));
                }
            }
        };
        let levels = |levels: &mut MixLevels| {
            for text in [&mut levels.volume_text, &mut levels.pan_text] {
                *text = text.as_deref().map(|text| redact(text));
            }
            for send in &mut levels.sends {
                send.name = redact(&send.name);
                send.text = send.text.as_deref().map(|text| redact(text));
            }
        };
        match picture {
            ChangePicture::Values { values: list }
            | ChangePicture::Eq { values: list, .. }
            | ChangePicture::Dynamics { values: list, .. } => values(list),
            ChangePicture::Automation { parameter, .. } => *parameter = redact(parameter),
            ChangePicture::Mix { before, after } => {
                levels(before);
                levels(after);
            }
            ChangePicture::Notes { .. } => {}
            ChangePicture::Lines { lines, .. } => {
                for line in lines {
                    line.before = redact(&line.before);
                    line.after = redact(&line.after);
                }
            }
            ChangePicture::Routes { routes } => {
                for route in routes {
                    route.from = redact(&route.from);
                    route.to = redact(&route.to);
                    values(&mut route.values);
                }
            }
            ChangePicture::Zones { before, after, values: list } => {
                for zone in before.iter_mut().chain(after.iter_mut()) {
                    zone.name = redact(&zone.name);
                }
                values(list);
            }
            ChangePicture::Items { before, after, values: list } => {
                for item in before.iter_mut().chain(after.iter_mut()) {
                    item.name = redact(&item.name);
                }
                values(list);
            }
        }
    }
    change
}

/// What's known of a conversation, as labels and values: its id, its Set, when it began and was last saved (when it was),
/// when it was exported (by which Kumi), the model and how many requests it holds.
fn facts(doc: &Document, source: &Source, options: &Options) -> Vec<(String, String)> {
    let mut facts = vec![("Conversation".to_string(), source.id.to_string())];
    if let Some(place) = &doc.place {
        facts.push((source.words.1.to_string(), place.clone()));
    }
    if source.created > 0 {
        facts.push(("Started".to_string(), when(source.created, options.zone)));
    }
    if source.conversation.saved_at > 0 {
        facts.push(("Last saved".to_string(), when(source.conversation.saved_at, options.zone)));
    }
    facts.push(("Exported".to_string(), format!("{} by Kumi {}", when(options.now, options.zone), options.version)));
    if let Some(model) = &doc.model {
        facts.push(("Model".to_string(), model.clone()));
    }
    facts.push(("Requests".to_string(), doc.requests.to_string()));
    facts
}

/// What the file holds and what it doesn't: whether secrets were taken out, and whether steps show their inputs and
/// results.
fn notes(options: &Options) -> [&'static str; 2] {
    [
        if options.redacted {
            "Secrets, your home folder and your account name are taken out."
        } else {
            "Nothing was taken out: this file may hold keys, tokens and paths."
        },
        if options.full { "Each step's input and result are included." } else { "Each step shows by name, without its input and result." },
    ]
}

fn footer(options: &Options) -> String {
    format!("Exported from Kumi {} on {}.", options.version, when(options.now, options.zone))
}

/// A time, as the producer reads it where they are.
fn when(ms: i64, zone: FixedOffset) -> String {
    DateTime::from_timestamp_millis(ms).map(|at| at.with_timezone(&zone).format("%Y-%m-%d %H:%M").to_string()).unwrap_or_default()
}

fn steps_word(count: usize) -> &'static str {
    if count == 1 {
        "Step"
    } else {
        "Steps"
    }
}

/// A change in a line: its title, its track and what became of it (undone, kept, unconfirmed, listened to). `daw` is what
/// the producer calls the DAW.
fn change_line(change: &ChangeRecord, daw: &str) -> String {
    let mut line = one_line(&change.title);
    if let Some(track) = &change.track {
        line.push_str(&format!(" · {}", one_line(&track.name)));
    }
    let state = match change.state {
        ChangeState::Applied | ChangeState::Expired => None,
        ChangeState::Undone => Some("undone".to_string()),
        ChangeState::Kept => Some("kept".to_string()),
        ChangeState::Unsure => Some(format!("{daw} didn't confirm it")),
        ChangeState::Heard => {
            Some(change.score.map(|score| format!("listened: {}% close", score.round())).unwrap_or_else(|| "listened".to_string()))
        }
    };
    if let Some(state) = state {
        line.push_str(&format!(" ({state})"));
    }
    line
}

/// Each value a picture shows moving, in words: "Drive: 3.0 dB → 6.0 dB", with what was asked when the DAW set something else.
fn values_of(picture: &ChangePicture) -> Vec<String> {
    let value = |value: &PictureValue| {
        let from = value.from_text.clone().or_else(|| value.from.map(number));
        let to = value.to_text.clone().or_else(|| value.to.map(number));
        let moved = match (from, to) {
            (Some(from), Some(to)) => format!("{from} → {to}"),
            (None, Some(to)) => to,
            (Some(from), None) => from,
            (None, None) => String::new(),
        };
        let asked = value.asked.as_deref().map(|asked| format!(" (asked {asked})")).unwrap_or_default();
        one_line(&format!("{}: {moved}{asked}", value.name))
    };
    let level = |name: &str, from: Option<&String>, to: Option<&String>| match (from, to) {
        (Some(from), Some(to)) if from != to => Some(format!("{name}: {from} → {to}")),
        _ => None,
    };
    match picture {
        ChangePicture::Values { values }
        | ChangePicture::Eq { values, .. }
        | ChangePicture::Dynamics { values, .. }
        | ChangePicture::Zones { values, .. }
        | ChangePicture::Items { values, .. } => values.iter().map(value).collect(),
        ChangePicture::Lines { lines, more } => {
            let mut said: Vec<String> = lines
                .iter()
                .map(|line| {
                    let side = |text: &str| if text.is_empty() { "(empty)".to_string() } else { one_line(text) };
                    format!("{}: {} → {}", line.line, side(&line.before), side(&line.after))
                })
                .collect();
            if *more > 0 {
                said.push(format!("and {more} more {}", if *more == 1 { "line" } else { "lines" }));
            }
            said
        }
        ChangePicture::Routes { routes } => routes
            .iter()
            .flat_map(|route| {
                let what = match route.change {
                    super::picture::RouteChange::Made => " (new)",
                    super::picture::RouteChange::Removed => " (taken away)",
                    super::picture::RouteChange::Changed => "",
                };
                std::iter::once(one_line(&format!("{} → {}{what}", route.from, route.to))).chain(route.values.iter().map(value))
            })
            .collect(),
        ChangePicture::Mix { before, after } => {
            let mut lines: Vec<String> = [
                level("Volume", before.volume_text.as_ref(), after.volume_text.as_ref()),
                level("Pan", before.pan_text.as_ref(), after.pan_text.as_ref()),
            ]
            .into_iter()
            .flatten()
            .collect();
            for send in &after.sends {
                let was = before.sends.iter().find(|old| old.name == send.name).and_then(|old| old.text.as_ref());
                lines.extend(level(&format!("Send {}", send.name), was, send.text.as_ref()));
            }
            lines.into_iter().map(|line| one_line(&line)).collect()
        }
        ChangePicture::Automation { parameter, .. } if !parameter.is_empty() => vec![one_line(parameter)],
        _ => Vec::new(),
    }
}

fn number(value: f64) -> String {
    kumi_common::js::number::to_string((value * 100.).round() / 100.)
}

/// Text on one line: line breaks and runs of white space as single spaces.
fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// `text` in a fenced block whose fence is longer than any run of backticks in it.
fn fenced(text: &str, language: &str) -> String {
    let mut longest = 0;
    let mut run = 0;
    for c in text.chars() {
        run = if c == '`' { run + 1 } else { 0 };
        longest = longest.max(run);
    }
    let fence = "`".repeat((longest + 1).max(3));
    format!("{fence}{language}\n{text}\n{fence}\n\n")
}

/// Words for Markdown as the terminal shows them, with nothing in them a Markdown reader runs or loads: outside a real
/// fenced block every `<` is `&lt;` (whatever backticks are around, so `<user>` shows and no markup runs) and every `![`
/// is `!\[` (no picture is fetched). A block left open is closed, so it can't take in what follows. Each single line
/// break stays one.
fn markdown_safe(text: &str) -> String {
    let lines: Vec<&str> = text.split('\n').collect();
    let mut out = String::with_capacity(text.len());
    let mut fence: Option<String> = None;
    for (index, line) in lines.iter().enumerate() {
        if index > 0 {
            out.push('\n');
        }
        if let Some(open) = &fence {
            if fence_of(line).is_some_and(|(marker, rest)| closes(open, &marker, &rest)) {
                fence = None;
            }
            out.push_str(line);
            continue;
        }
        if let Some((marker, _)) = fence_of(line) {
            fence = Some(marker);
            out.push_str(line);
            continue;
        }
        out.push_str(&line.replace('<', "&lt;").replace("![", "!\\["));
        let next = lines.get(index + 1).copied().unwrap_or("");
        if !trim(line).is_empty() && !trim(next).is_empty() && fence_of(next).is_none() {
            out.push_str("  ");
        }
    }
    if let Some(open) = fence {
        out.push_str(&format!("\n{open}"));
    }
    out
}

/// A fenced block's opening or closing line as CommonMark reads one: at most three spaces in, then three or more
/// backticks (with no backtick after them on the line) or tildes. The fence and what follows it.
fn fence_of(line: &str) -> Option<(String, String)> {
    let found = FENCE.captures(line)?;
    let (marker, rest) = (found[1].to_string(), found[2].to_string());
    (!(marker.starts_with('`') && rest.contains('`'))).then_some((marker, rest))
}

/// Whether a fence closes the block `open` began: the same character, at least as many, and nothing after it.
fn closes(open: &str, marker: &str, rest: &str) -> bool {
    marker.starts_with(&open[..1]) && marker.len() >= open.len() && trim(rest).is_empty()
}

/// Text for the web page: its markup characters as entities, and control characters gone. What draws a change's picture
/// writes names with it.
pub fn html_text(text: &str) -> String {
    escape(text)
}

/// Text for HTML: its markup characters as entities, and control characters gone.
fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            '\n' | '\t' => out.push(c),
            c if c.is_control() => {}
            c => out.push(c),
        }
    }
    out
}

/// The producer's words for the page: paragraphs at blank lines, and each line break kept.
fn plain_html(text: &str) -> String {
    text.split("\n\n")
        .map(trim)
        .filter(|paragraph| !paragraph.is_empty())
        .map(|paragraph| format!("<p>{}</p>", paragraph.lines().map(escape).collect::<Vec<_>>().join("<br>")))
        .collect()
}

// JavaScript's `.`, as the terminal's Markdown reads it: anything but a line terminator.
const DOT: &str = r"[^\n\r\u{2028}\u{2029}]";

static INLINE: LazyLock<fancy_regex::Regex> = LazyLock::new(|| {
    fancy_regex::Regex::new(&format!(
        r"(`+)({DOT}+?)\1|\*\*(?=\S)({DOT}+?)(?<=\S)\*\*|(?<![A-Za-z0-9_*])\*(?=[^\s*])({DOT}+?)(?<=[^\s*])\*(?![A-Za-z0-9_*])|(?<![A-Za-z0-9_])_(?=\S)({DOT}+?)(?<=\S)_(?![A-Za-z0-9_])|\[([^\]\n]+)\]\((\S+?)\)"
    ))
    .unwrap()
});
static OPENING: LazyLock<regex::Regex> = LazyLock::new(|| regex::Regex::new(r"^\s*(`{3,}|~{3,})").unwrap());
static FENCE: LazyLock<regex::Regex> = LazyLock::new(|| regex::Regex::new(&format!(r"^ {{0,3}}(`{{3,}}|~{{3,}})({DOT}*)$")).unwrap());
static HEADING: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(&format!(r"^\s{{0,3}}(#{{1,6}})\s+({DOT}*?)\s*#*\s*$")).unwrap());
static RULE: LazyLock<fancy_regex::Regex> = LazyLock::new(|| fancy_regex::Regex::new(r"^\s{0,3}([-*_])(\s*\1){2,}\s*$").unwrap());
static QUOTE: LazyLock<regex::Regex> = LazyLock::new(|| regex::Regex::new(&format!(r"^\s{{0,3}}>\s?({DOT}*)$")).unwrap());
static ITEM: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(&format!(r"^(\s*)([-*+•]|[0-9]{{1,3}}[.)])\s+({DOT}*)$")).unwrap());
static TABLE: LazyLock<regex::Regex> = LazyLock::new(|| regex::Regex::new(&format!(r"^\s*\|{DOT}*\|\s*$")).unwrap());
static TABLE_RULE: LazyLock<regex::Regex> = LazyLock::new(|| regex::Regex::new(r"^\s*\|?\s*:?-{2,}").unwrap());

/// Kumi's words for the page, as the terminal draws them: the Markdown models write in chat (headings, bold, italic,
/// inline code, fenced code, lists, quotes, links and rules), single line breaks kept. Everything else is text: no markup
/// passes through, and no picture loads.
fn markdown_html(text: &str) -> String {
    fn flush(out: &mut String, paragraph: &mut Vec<String>) {
        if !paragraph.is_empty() {
            out.push_str(&format!("<p>{}</p>", paragraph.join("<br>")));
            paragraph.clear();
        }
    }
    let mut out = String::new();
    let mut paragraph: Vec<String> = Vec::new();
    let mut fence: Option<String> = None;
    let mut code: Vec<&str> = Vec::new();
    for line in text.split('\n') {
        let opening = OPENING.captures(line).map(|found| found[1].to_string());
        if let Some(open) = &fence {
            if opening.as_ref().is_some_and(|marker| marker.starts_with(open.as_str())) {
                out.push_str(&format!("<pre><code>{}</code></pre>", escape(&code.join("\n"))));
                code.clear();
                fence = None;
            } else {
                code.push(line);
            }
            continue;
        }
        if let Some(marker) = opening {
            flush(&mut out, &mut paragraph);
            fence = Some(marker);
            continue;
        }
        if trim(line).is_empty() {
            flush(&mut out, &mut paragraph);
            continue;
        }
        if let Some(heading) = HEADING.captures(line) {
            flush(&mut out, &mut paragraph);
            let level = (heading[1].len() + 2).min(6);
            out.push_str(&format!("<h{level}>{}</h{level}>", inline_html(&heading[2])));
            continue;
        }
        if RULE.is_match(line).unwrap_or(false) {
            flush(&mut out, &mut paragraph);
            out.push_str("<hr>");
            continue;
        }
        if let Some(quote) = QUOTE.captures(line) {
            flush(&mut out, &mut paragraph);
            out.push_str(&format!("<blockquote>{}</blockquote>", inline_html(&quote[1])));
            continue;
        }
        if let Some(item) = ITEM.captures(line) {
            flush(&mut out, &mut paragraph);
            let depth = (item[1].replace('\t', "  ").chars().count() / 2).min(4);
            let marker = if item[2].bytes().any(|byte| byte.is_ascii_digit()) { item[2].to_string() } else { "•".to_string() };
            out.push_str(&format!(
                "<p class=\"item d{depth}\"><span class=\"marker\">{}</span> {}</p>",
                escape(&marker),
                inline_html(&item[3])
            ));
            continue;
        }
        if TABLE.is_match(line) {
            flush(&mut out, &mut paragraph);
            if !TABLE_RULE.is_match(line) {
                out.push_str(&format!("<p class=\"table\">{}</p>", escape(trim(line))));
            }
            continue;
        }
        paragraph.push(inline_html(line));
    }
    if fence.is_some() {
        out.push_str(&format!("<pre><code>{}</code></pre>", escape(&code.join("\n"))));
    }
    flush(&mut out, &mut paragraph);
    out
}

/// A line's inline Markdown as HTML: `code`, **bold**, *italic*, _italic_ and [links](url); a link goes only to a web or
/// mail address, any other stays words.
fn inline_html(text: &str) -> String {
    let mut out = String::new();
    let mut last = 0;
    // A search that hits the backtrack limit errs without moving on: the rest of such a line stays plain text.
    for found in INLINE.captures_iter(text).map_while(Result::ok) {
        let whole = found.get(0).expect("the match");
        if whole.start() > last {
            out.push_str(&escape(&text[last..whole.start()]));
        }
        let group = |number: usize| found.get(number).map(|group| group.as_str());
        if let Some(code) = group(2) {
            out.push_str(&format!("<code>{}</code>", escape(code)));
        } else if let Some(bold) = group(3) {
            out.push_str(&format!("<strong>{}</strong>", escape(bold)));
        } else if let Some(italic) = group(4).or_else(|| group(5)) {
            out.push_str(&format!("<em>{}</em>", escape(italic)));
        } else if let Some(label) = group(6) {
            let url = group(7).unwrap_or("");
            let lower = url.to_ascii_lowercase();
            if lower.starts_with("https://") || lower.starts_with("http://") || lower.starts_with("mailto:") {
                out.push_str(&format!("<a href=\"{}\" rel=\"noopener noreferrer nofollow\">{}</a>", escape(url), escape(label)));
            } else {
                out.push_str(&escape(label));
                if !url.is_empty() && url != label {
                    out.push_str(&format!(" ({})", escape(url)));
                }
            }
        }
        last = whole.end();
    }
    if last < text.len() {
        out.push_str(&escape(&text[last..]));
    }
    out
}

/// The page's look: the reader's system font and colours, light or dark, a column a phone can read, and pictures drawn
/// in the page's own colours.
const STYLE: &str = r#":root {
  color-scheme: light dark;
  --ground: #fafaf8; --card: #ffffff; --text: #1d1d1f; --dim: #5f5f66; --faint: #8e8e96; --rule: #e3e3de;
  --code: #f1f1ec; --accent: #1f8a5c; --you: #eef6f1;
}
@media (prefers-color-scheme: dark) {
  :root { --ground: #141416; --card: #1b1b1f; --text: #ececee; --dim: #a8a8b0; --faint: #74747c; --rule: #2c2c31;
    --code: #232328; --accent: #86e3b5; --you: #18231d; }
}
* { box-sizing: border-box; }
html { -webkit-text-size-adjust: 100%; }
body { margin: 0; background: var(--ground); color: var(--text);
  font: 16px/1.55 system-ui, -apple-system, "Segoe UI", Roboto, "Helvetica Neue", Arial, "Noto Sans", "Hiragino Sans", "Yu Gothic", "Microsoft YaHei", sans-serif; }
main, footer { max-width: 46rem; margin: 0 auto; padding: 0 16px; }
header { padding: 2rem 0 1rem; border-bottom: 1px solid var(--rule); }
.kicker { margin: 0; color: var(--accent); font-size: .8rem; letter-spacing: .08em; text-transform: uppercase; }
h1 { margin: .3rem 0 .8rem; font-size: 1.6rem; line-height: 1.25; overflow-wrap: anywhere; }
dl { display: flex; flex-wrap: wrap; gap: .3rem 1.4rem; margin: 0; font-size: .85rem; }
dl div { display: flex; gap: .4rem; }
dt { color: var(--faint); }
dd { margin: 0; color: var(--dim); overflow-wrap: anywhere; }
.notes { margin: .7rem 0 0; color: var(--faint); font-size: .85rem; }
.partial { margin: 1rem 0; padding: .6rem .9rem; border-left: 3px solid var(--accent); background: var(--card); color: var(--dim); font-size: .9rem; }
.partial p { margin: .2rem 0; }
article { margin: 1.2rem 0; padding: .9rem 1rem; border-radius: 10px; background: var(--card); border: 1px solid var(--rule); }
article.you { background: var(--you); }
article h2 { margin: 0 0 .4rem; font-size: .75rem; letter-spacing: .08em; text-transform: uppercase; color: var(--faint); }
.words { overflow-wrap: anywhere; }
.words p { margin: .45rem 0; }
.words h3, .words h4, .words h5, .words h6 { margin: .9rem 0 .3rem; line-height: 1.3; }
.words blockquote { margin: .4rem 0; padding-left: .8rem; border-left: 3px solid var(--rule); color: var(--dim); }
.words .item { margin: .2rem 0; padding-left: 1.2rem; text-indent: -1.2rem; }
.words .item.d1 { margin-left: 1.2rem; } .words .item.d2 { margin-left: 2.4rem; } .words .item.d3 { margin-left: 3.6rem; } .words .item.d4 { margin-left: 4.8rem; }
.words .marker { color: var(--faint); }
.words .table { margin: 0; font-family: ui-monospace, SFMono-Regular, Menlo, Consolas, monospace; font-size: .85rem; white-space: pre-wrap; }
.words hr { border: 0; border-top: 1px solid var(--rule); margin: .9rem 0; }
a { color: var(--accent); }
code, pre { font-family: ui-monospace, SFMono-Regular, Menlo, Consolas, monospace; font-size: .85rem; }
code { background: var(--code); padding: .05rem .3rem; border-radius: 4px; }
pre { background: var(--code); padding: .7rem .8rem; border-radius: 8px; white-space: pre-wrap; overflow-wrap: anywhere; margin: .5rem 0; }
pre code { background: none; padding: 0; }
details { margin-top: .6rem; color: var(--dim); font-size: .9rem; }
summary { cursor: pointer; color: var(--faint); }
details ul { margin: .3rem 0 0; padding-left: 1.2rem; }
.steps details { margin-top: .3rem; }
.what { margin: .4rem 0 0; font-size: .75rem; letter-spacing: .06em; text-transform: uppercase; color: var(--faint); }
.changes { padding: .4rem 0 1.2rem; border-top: 1px solid var(--rule); }
.changes h2 { font-size: 1.05rem; }
.changes ul { list-style: none; margin: 0; padding: 0; }
.changes li { margin: .8rem 0; }
.changes li p { margin: 0; }
figure { margin: .4rem 0 0; }
figure svg { display: block; width: 100%; max-width: 32rem; height: auto; background: var(--card); border: 1px solid var(--rule); border-radius: 8px; }
figcaption { margin-top: .3rem; color: var(--dim); font-size: .85rem; }
svg polyline { fill: none; stroke-linejoin: round; stroke-linecap: round; }
svg rect.swatch { stroke: var(--rule); stroke-width: 2; }
.chain { margin: .4rem 0 0; color: var(--dim); font-size: .9rem; overflow-wrap: anywhere; }
.chain p { margin: .15rem 0; }
.chain .rack { color: var(--faint); }
.chain strong { color: var(--accent); }
.chain span { color: var(--faint); }
svg polyline.guide { stroke: var(--rule); } svg polyline.ghost { stroke: var(--faint); }
svg polyline.dim { stroke: var(--dim); } svg polyline.now { stroke: var(--accent); }
svg rect.guide { fill: var(--rule); } svg rect.ghost { fill: var(--faint); }
svg rect.dim { fill: var(--dim); } svg rect.now { fill: var(--accent); }
footer { padding: 1.2rem 16px 2.4rem; color: var(--faint); font-size: .8rem; border-top: 1px solid var(--rule); }
"#;

/// A conversation read from a Kumi JSON export, checked and cut to what Kumi keeps.
#[derive(Debug, Clone, PartialEq)]
pub struct Imported {
    /// The file's id for it, when it's one Kumi could use.
    pub id: Option<String>,
    /// What it was called, as one line.
    pub title: String,
    /// The Set it was about, when the file says.
    pub place: Option<String>,
    pub conversation: SavedConversation,
}

/// What a file for `<command> import` is, said with the command the producer runs Kumi with.
pub fn importable(command: &str) -> String {
    format!("{command} import takes a conversation Kumi saved with: {command} export --json")
}

/// A Kumi JSON export, read as untrusted: only the shapes Kumi writes are kept (words, steps and their results, as text),
/// with no reasoning, pictures or provider data, and every call paired with its result. The conversation carries on
/// from these with any model. Err says what's wrong with the file, in plain words naming `command` (how the producer
/// runs Kumi).
pub fn import(text: &str, now: i64, command: &str) -> Result<Imported, String> {
    let how = importable(command);
    if text.len() > IMPORT_MAX_BYTES {
        return Err(format!("That file is too big for a Kumi conversation (16 MB at most). {how}"));
    }
    let value: Value = serde_json::from_str(text).map_err(|_| format!("That file isn't a Kumi conversation. {how}"))?;
    if value.get("format").and_then(Value::as_str) != Some(FORMAT) {
        return Err(format!("That file isn't a Kumi conversation. {how}"));
    }
    match value.get("version").and_then(Value::as_u64) {
        Some(VERSION) => {}
        Some(newer) if newer > VERSION => {
            return Err(format!("That conversation is from a newer Kumi (format {newer}): update Kumi, then import it again."))
        }
        _ => return Err(format!("That Kumi conversation has no format version this Kumi knows. {how}")),
    }
    let Some(listed) = value.get("messages").and_then(Value::as_array) else {
        return Err(format!("That Kumi conversation has no messages. {how}"));
    };
    let mut messages = renumbered(
        listed
            .iter()
            .take(MAX_MESSAGES)
            .filter_map(|message| serde_json::from_value::<Message>(message.clone()).ok())
            .filter_map(kept_message)
            .collect(),
    );
    if messages.first().is_some_and(|first| first.get("role").and_then(Value::as_str) != Some("user")) {
        messages.insert(0, json!({"role": "user", "content": [{"type": "text", "text": PARTWAY}]}));
    }
    let messages = pair(messages);
    let producer: Vec<String> = messages
        .iter()
        .filter(|message| message.get("role").and_then(Value::as_str) == Some("user"))
        .map(|message| trim(&texts(&parts_of(message))).to_string())
        .collect();
    let Some(first) = producer.iter().find(|words| !words.is_empty() && *words != PARTWAY).cloned() else {
        return Err("That Kumi conversation has nothing in it to continue.".to_string());
    };
    let changes: Vec<ChangeRecord> = value
        .get("changes")
        .and_then(Value::as_array)
        .map(|listed| {
            let start = listed.len().saturating_sub(MAX_CHANGES);
            listed[start..].iter().filter_map(kept_change).collect()
        })
        .unwrap_or_default();
    let id = value
        .get("id")
        .and_then(Value::as_str)
        .filter(|id| (6..=24).contains(&id.len()) && id.bytes().all(|b| b.is_ascii_digit() || b.is_ascii_lowercase()))
        .map(str::to_string);
    let title = value
        .get("title")
        .and_then(Value::as_str)
        .map(|title| head(&one_line(&neutralized(&drawn(title))), MAX_TITLE))
        .filter(|title| !title.is_empty())
        .unwrap_or_else(|| head(&one_line(&first), MAX_TITLE));
    let place = value
        .get("set")
        .and_then(Value::as_str)
        .map(|set| head(&one_line(&neutralized(&drawn(set))), MAX_TITLE))
        .filter(|set| !set.is_empty());
    let saved_at = value.get("saved").and_then(Value::as_i64).filter(|at| (1..=now).contains(at)).unwrap_or(now);
    let turns = producer.iter().filter(|words| *words != PARTWAY).count() as u32;
    Ok(Imported {
        id,
        title,
        place,
        conversation: SavedConversation {
            saved_at,
            checkpoint: KernelCheckpoint { version: 1, messages, origin: None, tools: None },
            changes: (!changes.is_empty()).then_some(changes),
            first: Some(head(&first, 300)),
            turns: Some(turns),
        },
    })
}

/// A message read from a file, with only what Kumi writes: words, calls and their results as text. The producer's words
/// are cleaned as an export cleans them, so nothing in them is hidden from the transcript but seen by the model; a message
/// that opens with Kumi's mark goes, and the mark is taken off wherever else it stands, inputs and results included, so
/// none of the file speaks as Kumi. None when nothing's left of it.
fn kept_message(message: Message) -> Option<Value> {
    let words = |text: &str| {
        Some(neutralized(&drawn(&bounded(text)))).filter(|text| !trim(text).is_empty()).map(|text| json!({"type": "text", "text": text}))
    };
    let (role, content): (&str, Vec<Value>) = match message {
        Message::User { content, .. } => {
            let text: String = content
                .iter()
                .filter_map(|part| match part {
                    UserPart::Text(text) => Some(text.text.as_str()),
                    UserPart::File(_) => None,
                })
                .collect();
            match producer_words(&drawn(&bounded(&text)), 0, &mut Vec::new(), &mut false) {
                Words::Said(said) => ("user", vec![json!({"type": "text", "text": neutralized(&said)})]),
                Words::CarryOn | Words::Kumi => return None,
            }
        }
        Message::Assistant { content, .. } => (
            "assistant",
            content
                .into_iter()
                .filter_map(|part| match part {
                    AssistantPart::Text(text) => words(&text.text),
                    AssistantPart::ToolCall(call) => Some(json!({
                        "type": "tool-call",
                        "toolCallId": call.tool_call_id,
                        "toolName": clean_name(&call.tool_name),
                        "input": if call.input.is_object() && depth(&call.input) <= MAX_DEPTH && stringify_pretty(&call.input, 0).len() <= MAX_INPUT { cleaned_value(&call.input) } else { json!({}) },
                    })),
                    _ => None,
                })
                .collect(),
        ),
        Message::Tool { content, .. } => (
            "tool",
            content
                .into_iter()
                .filter_map(|part| match part {
                    ToolPart::ToolResult(result) => {
                        let text = neutralized(&drawn(&bounded(&output_text(&result.output))));
                        Some(json!({
                            "type": "tool-result",
                            "toolCallId": result.tool_call_id,
                            "toolName": clean_name(&result.tool_name),
                            "output": {"type": "text", "value": if trim(&text).is_empty() { LEFT_OUT.to_string() } else { text }},
                        }))
                    }
                    ToolPart::ToolApprovalResponse(_) => None,
                })
                .collect(),
        ),
        Message::System { .. } => return None,
    };
    (!content.is_empty()).then(|| json!({"role": role, "content": content}))
}

/// A change read from a file, for HISTORY: each of its words on one line without control characters, at most
/// [`MAX_WORDS`] long, and its picture within Kumi's bounds. (Resuming marks it as from an earlier session: it can't be
/// undone here.)
fn kept_change(change: &Value) -> Option<ChangeRecord> {
    let mut change: ChangeRecord = serde_json::from_value(one_line_strings(change)).ok()?;
    change.picture = change.picture.and_then(ChangePicture::bounded);
    Some(change)
}

/// A value with each of its strings on one line, without control characters, and at most [`MAX_WORDS`] long.
fn one_line_strings(value: &Value) -> Value {
    match value {
        Value::String(text) => {
            let shown: String = text.chars().map(|c| if c.is_control() { ' ' } else { c }).collect();
            Value::String(head(&one_line(&shown), MAX_WORDS))
        }
        Value::Array(items) => Value::Array(items.iter().map(one_line_strings).collect()),
        Value::Object(fields) => Value::Object(fields.iter().map(|(key, item)| (key.clone(), one_line_strings(item))).collect()),
        other => other.clone(),
    }
}

/// A result's words, from what a file can hold.
fn output_text(output: &ToolResultOutput) -> String {
    match output {
        ToolResultOutput::Text { value, .. } => value.clone(),
        ToolResultOutput::ErrorText { value, .. } => format!("Error: {value}"),
        ToolResultOutput::Json { value, .. } => stringify_pretty(value, 2),
        ToolResultOutput::ErrorJson { value, .. } => format!("Error: {}", stringify_pretty(value, 2)),
        ToolResultOutput::ExecutionDenied { reason, .. } => {
            format!("Not run{}", reason.as_deref().map(|why| format!(": {why}")).unwrap_or_default())
        }
        ToolResultOutput::Content { value, .. } => value
            .iter()
            .filter_map(|item| match item {
                ToolResultContentItem::Text { text, .. } => Some(text.clone()),
                ToolResultContentItem::File { .. } => Some("[a picture]".to_string()),
                ToolResultContentItem::Custom { .. } => None,
            })
            .collect::<Vec<_>>()
            .join("\n"),
    }
}

/// Words from a file, at most [`MAX_PART`] long: past it, cut, and said to be.
fn bounded(text: &str) -> String {
    if utf16_len(text) > MAX_PART {
        format!("{} [cut: it ran longer than Kumi imports]", head(text, MAX_PART))
    } else {
        text.to_string()
    }
}

/// A step's input from a file, each string and key cleaned as words are.
/// How many lists and objects deep a value nests.
fn depth(value: &Value) -> usize {
    match value {
        Value::Array(items) => 1 + items.iter().map(depth).max().unwrap_or(0),
        Value::Object(fields) => 1 + fields.values().map(depth).max().unwrap_or(0),
        _ => 0,
    }
}

fn cleaned_value(value: &Value) -> Value {
    match value {
        Value::String(text) => Value::String(neutralized(&drawn(&bounded(text)))),
        Value::Array(items) => Value::Array(items.iter().map(cleaned_value).collect()),
        Value::Object(fields) => Value::Object(fields.iter().map(|(key, item)| (neutralized(&drawn(key)), cleaned_value(item))).collect()),
        other => other.clone(),
    }
}

/// Whether a character draws nothing and moves nothing a reader sees, but can hide words or turn them around: format
/// characters (zero-width spaces and joiners, direction marks and overrides, the byte-order mark, soft hyphens), tag
/// characters, variation selectors and the other default-ignorable code points.
pub(crate) fn invisible(c: char) -> bool {
    matches!(c,
        '\u{00AD}' | '\u{034F}' | '\u{061C}' | '\u{115F}' | '\u{1160}' | '\u{17B4}' | '\u{17B5}' | '\u{180B}'..='\u{180F}'
        | '\u{200B}'..='\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2060}'..='\u{206F}' | '\u{3164}' | '\u{FE00}'..='\u{FE0F}'
        | '\u{FEFF}' | '\u{FFA0}' | '\u{FFF0}'..='\u{FFF8}' | '\u{1BCA0}'..='\u{1BCA3}' | '\u{1D173}'..='\u{1D17A}'
        | '\u{E0000}'..='\u{E0FFF}')
}

/// A tool's name from a file: letters, digits, `_` and `-`, at most 64 of them.
fn clean_name(text: &str) -> String {
    let kept: String = text.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-').take(64).collect();
    if kept.is_empty() {
        "tool".to_string()
    } else {
        kept
    }
}

/// The calls numbered afresh ("call_1", …), each result taking its call's number: a file's ids may repeat or be anything.
/// A result whose call isn't there gets no number, and [`pair`] drops it.
fn renumbered(mut messages: Vec<Value>) -> Vec<Value> {
    let mut numbers: HashMap<String, String> = HashMap::new();
    let mut count = 0;
    for message in &mut messages {
        let Some(Value::Array(parts)) = message.get_mut("content") else { continue };
        for part in parts.iter_mut() {
            let id = part.get("toolCallId").and_then(Value::as_str).unwrap_or("").to_string();
            let number = match kind(part) {
                "tool-call" => {
                    count += 1;
                    let number = format!("call_{count}");
                    numbers.insert(id, number.clone());
                    number
                }
                "tool-result" => numbers.get(&id).cloned().unwrap_or_default(),
                _ => continue,
            };
            part["toolCallId"] = Value::String(number);
        }
    }
    messages
}

/// Every call followed by its result, as models want them: a call whose result is missing gets one saying it was left
/// out, right after it, and a result without its call goes.
fn pair(messages: Vec<Value>) -> Vec<Value> {
    fn close(out: &mut Vec<Value>, open: &mut Vec<(String, String)>) {
        if open.is_empty() {
            return;
        }
        let results: Vec<Value> = open
            .drain(..)
            .map(|(id, name)| json!({"type": "tool-result", "toolCallId": id, "toolName": name, "output": {"type": "text", "value": LEFT_OUT}}))
            .collect();
        add(out, "tool", results, true);
    }
    let mut out: Vec<Value> = Vec::with_capacity(messages.len());
    let mut open: Vec<(String, String)> = Vec::new();
    for message in messages {
        let role = message.get("role").and_then(Value::as_str).unwrap_or("").to_string();
        if role == "tool" {
            let content: Vec<Value> = parts_of(&message)
                .into_iter()
                .filter(|part| {
                    let id = part.get("toolCallId").and_then(Value::as_str).unwrap_or("");
                    match open.iter().position(|(call, _)| call == id) {
                        Some(at) => {
                            open.remove(at);
                            true
                        }
                        None => false,
                    }
                })
                .collect();
            if !content.is_empty() {
                add(&mut out, "tool", content, true);
            }
            continue;
        }
        close(&mut out, &mut open);
        if role == "assistant" {
            for part in parts_of(&message).iter().filter(|part| kind(part) == "tool-call") {
                let id = part.get("toolCallId").and_then(Value::as_str).unwrap_or("").to_string();
                let name = part.get("toolName").and_then(Value::as_str).unwrap_or("").to_string();
                open.push((id, name));
            }
        }
        out.push(message);
    }
    close(&mut out, &mut open);
    out
}
