//! What Kumi keeps of words it didn't write (a note to remember, a recipe's or a technique's words, a name read from a
//! Set): never words that read like an instruction to it or carry what looks like a secret.

use std::sync::LazyLock;

use regex::Regex;

static SUSPECT: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    [
        r"(?i)\b(ignore|disregard|override|forget)\b[^.]{0,40}\b(rules|instructions|prompt|guidelines)\b",
        r"(?i)\bsystem prompt\b",
        r"(?i)\b(reveal|print|show|send|share|leak|post)\b[^.]{0,40}\b(api ?keys?|tokens?|passwords?|credentials?|secrets?|auth\w*)\b",
        r"(?i)\b(api[_-]?key|token|secret|password)\s*[:=]",
        r"[A-Za-z0-9+/_=\-]{32,}",
    ]
    .into_iter()
    .map(|pattern| Regex::new(pattern).unwrap())
    .collect()
});

/// Whether `text` reads like an instruction to Kumi or carries what looks like a secret.
pub fn suspect_note(text: &str) -> bool {
    SUSPECT.iter().any(|pattern| pattern.is_match(text))
}
