//! Live's vocabulary, for the tests that a REAPER or Renoise producer never meets it (daw_words, terminal).
use regex::Regex;
use std::sync::LazyLock;

/// Live's words, whole.
pub static LIVE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
            r"\b(Live|Ableton|Sets?|Session|Arrangement|Main|[Ss]cenes?|[Rr]acks?|[Cc]lip slots?|Control Surfaces?|Remote Scripts?|Max for Live|M4L|Operator)\b|\[live\]|Untitled clip",
        )
        .unwrap()
});

/// Where a Live word is right for any producer, and why.
pub const ALLOWED: &[(&str, &str)] = &[
    // An example request a producer might type: "Set" is the verb.
    ("Set the tempo to 124", "a verb"),
    // A Live Set dropped into the box is one: its chip says so.
    ("Live Set ·", "an .als file is Live's"),
    // "Set it to the whole path": the verb, in uninstall's refusals.
    ("Set KUMI_HOME", "a verb"),
    ("Set it to", "a verb"),
    // The taste Kumi learned is from the producer's Live Sets, the only projects it learns taste from: /memory names
    // them so outside Live.
    ("your Live Sets", "the taste's own source"),
    ("your own Live Sets", "the taste's own source"),
];

/// The lines in `text` that speak Live's words, outside what's allowed.
pub fn leaks(text: &str) -> Vec<String> {
    text.lines()
        .filter(|line| {
            let mut line = line.to_string();
            for (allowed, _) in ALLOWED {
                line = line.replace(allowed, "");
            }
            LIVE.is_match(&line)
        })
        .map(|line| line.trim().to_string())
        .collect()
}
