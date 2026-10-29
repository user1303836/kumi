//! Input history persistence, plus redaction and retention boundaries.
use kumi::history::*;
#[test]
fn history_persists_between_conversations_without_secrets() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("input-history");
    let mut history = open_input_history(Some(file.clone()), vec!["private-token".into()]);
    history.add("make the bass wider");
    history.add("make the bass wider");
    history.add("/new");
    history.add("use my key private-token and sk-abcdefghijklmnopqrstuvwxyz012345");
    assert_eq!(
        open_input_history(Some(file.clone()), vec![]).entries(),
        ["make the bass wider", "/new", "use my key [redacted] and [redacted]"]
    );
    let saved = std::fs::read_to_string(&file).unwrap();
    assert!(!saved.contains("private-token"));
    assert!(!saved.contains("sk-"));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(file).unwrap().permissions().mode() & 0o777, 0o600);
    }
}
#[test]
fn key_formats_and_labels_are_redacted_and_terminal_controls_removed() {
    for key in [
        "sk-1234567890abcdefgh",
        "pk-1234567890abcdefgh",
        "rk-1234567890abcdefgh",
        "AIza1234567890abcdefghij12345",
        "ghp_1234567890abcdefghij12345",
        "xoxb-1234567890abcdefgh",
        "eyJabcdefgh123.abcdefgh12345.abcdefgh12345",
        "abcdefghijklmnop1234567890abcdefgh",
    ] {
        assert_eq!(history_text(&format!("use {key}"), &[]), "use [redacted]", "{key}");
    }
    assert_eq!(
        history_text("api_key = abc token: xyz password=q secret: hidden", &[]),
        "api_key = [redacted] token: [redacted] password=[redacted] secret: [redacted]"
    );
    assert_eq!(history_text("\x1b[31mred\x1b[0m", &[]), "red");
    assert_eq!(history_text(&"x".repeat(5000), &[]).len(), 4096);
}
#[test]
fn history_recovers_valid_lines_compacts_and_survives_unwritable_storage() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("history");
    std::fs::write(&file, "\"first\"\n{broken\n42\n\"token: secret\"\n").unwrap();
    let mut history = open_input_history(Some(file.clone()), vec![]);
    assert_eq!(history.entries(), ["first", "token: [redacted]"]);
    for i in 0..999 {
        history.add(&format!("request {i}"));
    }
    assert_eq!(history.entries().len(), 500);
    assert_eq!(std::fs::read_to_string(&file).unwrap().lines().count(), 500);
    assert_eq!(history.entries()[0], "request 499");
    let mut broken = open_input_history(Some(file.join("not-a-directory")), vec![]);
    broken.add("still usable");
    assert_eq!(broken.entries(), ["still usable"]);
}
#[test]
fn a_draft_cleared_with_ctrl_c_is_kept_whole_while_kumi_runs_and_never_written() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("input-history");
    let mut history = open_input_history(Some(file.clone()), vec!["private-token".into()]);
    history.add("sent once");
    // Longer than a sent message is kept, and redacted as one is.
    let draft = format!("{} with private-token", "a long draft ".repeat(400));
    history.keep(&draft);
    assert_eq!(history.entries().len(), 2);
    assert_eq!(history.entries()[1], draft.replace("private-token", "[redacted]"));
    history.add("sent twice");
    assert_eq!(open_input_history(Some(file.clone()), vec![]).entries(), ["sent once", "sent twice"]);
    // When the file is written again whole, it still has only what was sent.
    for i in 0..998 {
        history.add(&format!("message {i}"));
    }
    history.keep("cleared just before the file is compacted");
    history.add("the last one");
    let saved = std::fs::read_to_string(&file).unwrap();
    assert!(!saved.contains("cleared just before") && !saved.contains("a long draft"));
    assert_eq!(saved.lines().count(), 499);
    assert_eq!(history.entries().len(), 500);
    assert_eq!(history.entries()[498], "cleared just before the file is compacted");
    // A draft cleared, then brought back with ↑ and sent, is written then, and stays one entry.
    history.keep("cleared, then sent");
    history.add("cleared, then sent");
    assert_eq!(history.entries().iter().filter(|entry| *entry == "cleared, then sent").count(), 1);
    assert_eq!(open_input_history(Some(file.clone()), vec![]).entries().last().map(String::as_str), Some("cleared, then sent"));
}
#[test]
fn a_conversations_log_takes_keys_out_of_its_words_as_the_history_does() {
    let redact = log_redactor(vec!["private-token-1234".into(), "private-token".into(), "none".into(), String::new()]);
    for key in [
        "sk-1234567890abcdefgh",
        "AIza1234567890abcdefghij12345",
        "ghp_1234567890abcdefghij12345",
        "xoxb-1234567890abcdefgh",
        "eyJabcdefgh123.abcdefgh12345.abcdefgh12345",
        "abcdefghijklmnop1234567890abcdefgh",
    ] {
        assert_eq!(redact(&format!("use {key}")), "use [redacted]", "{key}");
        assert_eq!(history_text(&format!("use {key}"), &[]), redact(&format!("use {key}")), "{key}");
    }
    assert_eq!(redact("api_key = abc token: xyz"), "api_key = [redacted] token: [redacted]");
    // The keys this computer knows, a longer one before one it holds.
    assert_eq!(redact("my private-token-1234 and private-token"), "my [redacted] and [redacted]");
    // The words are otherwise as they were: lines, spacing, length and what the screen cleans as it draws them. A "key"
    // too short to be one is a word.
    let long = format!("  one\n\ttwo \x1b[31mred\x1b[0m {}\n", "x".repeat(5000));
    assert_eq!(redact(&long), long);
    assert_eq!(redact("none of the tracks"), "none of the tracks");
}
