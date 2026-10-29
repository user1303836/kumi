//! Words a source can hide from whoever reads it, as the model reads them. Unicode's tag characters spell ASCII that
//! draws nothing, so a page, a transcript, or a sample's or a track's name can carry instructions nobody sees; a run of
//! variation selectors carries a byte each, and a long run of joiners and marks a few bits each. Each such run becomes
//! [`HIDDEN`]: the model knows something was hidden there, and never reads it.
//!
//! What hides nothing stays exactly as it was, because names and files have it and Kumi finds things by their exact
//! names: a joiner inside an emoji (👩‍🎤), a non-joiner in a Persian word, a direction mark in a Hebrew one, a file's
//! byte-order mark, a variation selector straight after a character (❤️), and the England, Scotland and Wales flags,
//! which tags make. A name the model repeats is still its track's.
//!
//! A name that hides words reaches the model marked ("Drums[hidden text removed]"), and the model gives it back so.
//! What the model read is remembered ([`remember`]), so each such name is put back as the DAW has it before a tool or a
//! plan's step gets it ([`named`]): the track is found by the name the model read, in every DAW, with no integration
//! comparing names any differently (#641).
use crate::core::export::invisible;
use serde_json::Value;
use std::borrow::Cow;
use std::cell::RefCell;
use std::collections::HashMap;

/// What the model reads where a source hid words.
pub const HIDDEN: &str = "[hidden text removed]";

/// The most characters that draw nothing that real text has side by side, such as the selector and the joiner in ❤️‍🔥. A
/// longer run can carry bits.
const LONGEST: usize = 4;

/// The most names [`remember`] keeps: past it, it starts again with the ones read since.
const KEPT: usize = 4096;

/// The longest a string can be, in characters, and be remembered as a name: longer ones are pages and transcripts.
const NAME_LONGEST: usize = 256;

thread_local! {
    /// Names that hide words, by what the model was shown of them; none where two different names read alike.
    static SHOWN: RefCell<HashMap<String, Option<String>>> = RefCell::new(HashMap::new());
}

/// The flags tag characters make after U+1F3F4 (England, Scotland, Wales), as the tags spell them before the cancel tag.
const FLAGS: [&str; 3] = ["gbeng", "gbsct", "gbwls"];

/// Unicode's variation selectors. One after a character chooses its form (❤️, a kanji's in a name); real text never has
/// two in a row, but a run of them behind one emoji can carry a hidden message, a byte for each.
pub fn selector(c: char) -> bool {
    matches!(c, '\u{180B}'..='\u{180D}' | '\u{180F}' | '\u{FE00}'..='\u{FE0F}' | '\u{E0100}'..='\u{E01EF}')
}

/// The tag characters and the unassigned code points around them, which draw nothing either.
fn tag(c: char) -> bool {
    matches!(c, '\u{E0000}'..='\u{E0FFF}') && !selector(c)
}

/// `text` as the model reads it. A run of characters that draw nothing becomes [`HIDDEN`] when it has a tag in it, a
/// second variation selector (the first, straight after a character, stays), or more than [`LONGEST`] characters; the
/// flags tags make stay whole. The marker is written once what was taken out covers its length, so the words never grow
/// and a cap on them still holds: a short run before that is taken out without one. That's counted afresh after each
/// quote, so a JSON string or a quoted name reads the same within a tool's whole result as alone, as [`remember`] keeps
/// it. Words with nothing hidden come back as they were, not copied.
pub fn unhidden(text: &str) -> Cow<'_, str> {
    let mut out = String::new();
    // `text` up to here is in `out`; `owed` bytes were taken out and no marker said so yet.
    let (mut copied, mut owed, mut at) = (0, 0, 0);
    while let Some(start) = text[at..].find(invisible).map(|found| at + found) {
        if text[at..start].contains(['"', '“', '”']) {
            owed = 0;
        }
        let end = text[start..].find(|c: char| !invisible(c)).map_or(text.len(), |found| start + found);
        let run = &text[start..end];
        at = end;
        let hides = run.chars().any(tag) || run.chars().filter(|c| selector(*c)).nth(1).is_some() || run.chars().nth(LONGEST).is_some();
        if !hides || (text[..start].ends_with('\u{1F3F4}') && flag(run)) {
            continue;
        }
        out.push_str(&text[copied..start]);
        let kept = run.chars().next().filter(|c| start > 0 && selector(*c)).map_or(0, char::len_utf8);
        out.push_str(&run[..kept]);
        owed += run.len() - kept;
        if owed >= HIDDEN.len() {
            out.push_str(HIDDEN);
            owed -= HIDDEN.len();
        }
        copied = end;
    }
    if copied == 0 {
        return Cow::Borrowed(text);
    }
    out.push_str(&text[copied..]);
    Cow::Owned(out)
}

/// [`unhidden`], for words already owned: they come back themselves when nothing in them is hidden.
pub fn unhidden_owned(text: String) -> String {
    let clean = match unhidden(&text) {
        Cow::Owned(clean) => Some(clean),
        Cow::Borrowed(_) => None,
    };
    clean.unwrap_or(text)
}

/// Whether `run` is one of [`FLAGS`]' tags and the cancel tag.
fn flag(run: &str) -> bool {
    let Some(code) = run.strip_suffix('\u{E007F}') else { return false };
    let spelled: String = code.chars().map(|c| (c as u32).checked_sub(0xE0000).and_then(char::from_u32).unwrap_or('\0')).collect();
    FLAGS.contains(&spelled.as_str())
}

/// Keeps each string in `value` (a tool's result, the observation) that hides words, by what the model is shown of it.
/// Only one shown with [`HIDDEN`] (one whose short run was taken out without it reads as a plain name, which another
/// thing may truly have), and at most [`NAME_LONGEST`] characters long.
pub fn remember(value: &Value) {
    match value {
        Value::String(text) => remember_one(text),
        Value::Array(items) => items.iter().for_each(remember),
        Value::Object(fields) => fields.values().for_each(remember),
        _ => {}
    }
}

/// [`remember`], for words that are JSON; other words keep only the names quoted in them. Words that hide nothing cost
/// a look and no more.
pub fn remember_text(text: &str) {
    if let Cow::Owned(_) = unhidden(text) {
        match serde_json::from_str::<Value>(text) {
            Ok(value) => remember(&value),
            Err(_) => quoted(text),
        }
    }
}

/// The string, and the names quoted in it.
fn remember_one(text: &str) {
    keep(text);
    quoted(text);
}

/// Each name quoted in `text` as Kumi's own lines quote names ("Renamed track “Bass” → “Sub”").
fn quoted(text: &str) {
    for (at, _) in text.match_indices('“') {
        let rest = &text[at + '“'.len_utf8()..];
        if let Some(end) = rest.find('”') {
            keep(&rest[..end]);
        }
    }
}

fn keep(text: &str) {
    if text.chars().nth(NAME_LONGEST).is_some() {
        return;
    }
    let Cow::Owned(shown) = unhidden(text) else { return };
    if !shown.contains(HIDDEN) {
        return;
    }
    SHOWN.with(|names| {
        let mut names = names.borrow_mut();
        if names.len() >= KEPT && !names.contains_key(&shown) {
            names.clear();
        }
        let alike = names.get(&shown).is_some_and(|known| known.as_deref() != Some(text));
        names.insert(shown, if alike { None } else { Some(text.to_owned()) });
    });
}

/// `text` with each name the model was shown in place of one that hides words put back as the name: the whole of it, or
/// where it holds one (a script naming a track). A name two things read alike as stays as the model gave it.
pub fn named(text: &str) -> Cow<'_, str> {
    if !text.contains(HIDDEN) {
        return Cow::Borrowed(text);
    }
    SHOWN.with(|names| {
        let names = names.borrow();
        if let Some(found) = names.get(text) {
            return found.clone().map_or(Cow::Borrowed(text), Cow::Owned);
        }
        // Longest first, so a name inside another's shown words is put back with the whole of it.
        let mut held: Vec<(&String, &String)> = names
            .iter()
            .filter_map(|(shown, name)| name.as_ref().map(|name| (shown, name)))
            .filter(|(shown, _)| text.contains(shown.as_str()))
            .collect();
        if held.is_empty() {
            return Cow::Borrowed(text);
        }
        held.sort_by_key(|(shown, _)| std::cmp::Reverse(shown.len()));
        let mut out = text.to_owned();
        for (shown, name) in held {
            out = out.replace(shown.as_str(), name);
        }
        Cow::Owned(out)
    })
}

/// `value` with [`named`] on each string in it (not its keys): what a tool is given, the names in it as the DAW has them.
pub fn named_value(value: &Value) -> Value {
    match value {
        Value::String(text) => Value::String(named(text).into_owned()),
        Value::Array(items) => Value::Array(items.iter().map(named_value).collect()),
        Value::Object(fields) => Value::Object(fields.iter().map(|(key, item)| (key.clone(), named_value(item))).collect()),
        other => other.clone(),
    }
}
