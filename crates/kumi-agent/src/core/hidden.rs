//! Words a source can hide from whoever reads it, as the model reads them. Unicode's tag characters spell ASCII that
//! draws nothing, so a page, a transcript, or a sample's or a track's name can carry instructions nobody sees; and a
//! run of variation selectors carries a byte each. Each such run becomes [`HIDDEN`]: the model knows something was
//! hidden there, and never reads it.
//!
//! What hides nothing stays exactly as it was, because names and files have it and Kumi finds things by their exact
//! names: a joiner inside an emoji (👩‍🎤), a non-joiner in a Persian word, a direction mark in a Hebrew one, a file's
//! byte-order mark, a variation selector straight after a character (❤️), and the England, Scotland and Wales flags,
//! which tags make. A name the model repeats is still its track's.
use crate::core::export::invisible;
use std::borrow::Cow;

/// What the model reads where a source hid words.
pub const HIDDEN: &str = "[hidden text removed]";

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

/// `text` as the model reads it. A run of characters that draw nothing becomes [`HIDDEN`] when it has a tag in it or a
/// second variation selector (the first, straight after a character, stays); the flags tags make stay whole. The marker
/// is written once what was taken out covers its length, so the words never grow and a cap on them still holds: a
/// short run before that is taken out without one. Words with nothing hidden come back as they were, not copied.
pub fn unhidden(text: &str) -> Cow<'_, str> {
    let mut out = String::new();
    // `text` up to here is in `out`; `owed` bytes were taken out and no marker said so yet.
    let (mut copied, mut owed, mut at) = (0, 0, 0);
    while let Some(start) = text[at..].find(invisible).map(|found| at + found) {
        let end = text[start..].find(|c: char| !invisible(c)).map_or(text.len(), |found| start + found);
        let run = &text[start..end];
        at = end;
        let hides = run.chars().any(tag) || run.chars().filter(|c| selector(*c)).nth(1).is_some();
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
