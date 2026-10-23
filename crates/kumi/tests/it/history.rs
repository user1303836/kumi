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
