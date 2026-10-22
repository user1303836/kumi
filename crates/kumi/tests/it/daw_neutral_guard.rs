//! The app reaches Live's own parts only through its DAW providers: outside src/daw/, nothing names Live's integration,
//! Live's bridge crate, or the app's Live modules (live_app, live_extension, bridge_setup, willington). Allowed: the
//! re-exports that keep their old paths (`pub use crate::daw::ableton::…`), imports only unit tests use
//! (`#[cfg(test)] use …`), and the few uses listed below, each with why.

use std::path::{Path, PathBuf};

/// What names Live's parts.
const LIVE: [&str; 6] = ["ableton", "ableton_mcp_server", "live_app", "live_extension", "bridge_setup", "willington"];

/// Uses allowed by file and phrase, with why.
const ALLOWED: &[(&str, &str, &str)] = &[
    ("doctor/installation.rs", "live_app::InstalledLive", "InstallationIo.lives, which the doctor's tests answer, lists Lives"),
    (
        "cli/native/session.rs",
        "crate::daw::ableton::provider::Ableton.watch_words()",
        "the watch with Live's words, as its unit tests drive it",
    ),
];

fn sources(folder: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(folder).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            sources(&path, out);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            out.push(path);
        }
    }
}

/// A line's code, without a comment after it (a `//` inside a string, as in a URL, ends it early: that only hides
/// more of the line, never adds to it).
fn code(line: &str) -> &str {
    line.split("//").next().unwrap_or("")
}

/// The identifiers in a line's code, each with whether a path joins it to the next or the last (`a::b`).
fn words(code: &str) -> Vec<(String, bool)> {
    let mut found = vec![];
    let chars: Vec<char> = code.chars().collect();
    let mut at = 0;
    while at < chars.len() {
        if chars[at].is_alphanumeric() || chars[at] == '_' {
            let start = at;
            while at < chars.len() && (chars[at].is_alphanumeric() || chars[at] == '_') {
                at += 1;
            }
            let word: String = chars[start..at].iter().collect();
            let before = start >= 2 && chars[start - 2] == ':' && chars[start - 1] == ':';
            let after = at + 1 < chars.len() && chars[at] == ':' && chars[at + 1] == ':';
            found.push((word, before || after));
        } else {
            at += 1;
        }
    }
    found
}

#[test]
fn nothing_outside_the_daw_providers_names_lives_own_parts() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = vec![];
    sources(&root, &mut files);
    let mut found = vec![];
    for file in files {
        let name = file.strip_prefix(&root).unwrap().to_string_lossy().replace('\\', "/");
        if name.starts_with("daw/") || name == "lib.rs" {
            continue;
        }
        let text = std::fs::read_to_string(&file).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        // The `use` statement the line is in, and whether it's allowed whole (a re-export shim, a unit test's import).
        let mut using: Option<bool> = None;
        let mut test_only = false;
        for (n, line) in lines.iter().enumerate() {
            // A file's unit tests (`#[cfg(test)] mod …`) come last: they drive Live's parts directly.
            let next = lines[n + 1..].iter().map(|line| code(line).trim()).find(|line| !line.is_empty());
            if code(line).trim() == "#[cfg(test)]" && next.is_some_and(|next| next.starts_with("mod ")) {
                break;
            }
            let code = code(line);
            let trimmed = code.trim_start();
            if using.is_none() && trimmed.trim_start_matches("pub ").trim_start_matches("pub(crate) ").starts_with("use ") {
                using = Some(test_only || trimmed.starts_with("pub use crate::daw::ableton::"));
            }
            if !trimmed.is_empty() {
                test_only = trimmed.starts_with("#[cfg(test)]");
            }
            let named = words(code).into_iter().any(|(word, path)| LIVE.contains(&word.as_str()) && (path || using.is_some()));
            let allowed = using == Some(true) || ALLOWED.iter().any(|(file, phrase, _)| name == *file && code.contains(phrase));
            if named && !allowed {
                found.push(format!("{name}:{}: {}", n + 1, line.trim()));
            }
            if code.contains(';') {
                using = None;
            }
        }
    }
    assert!(found.is_empty(), "these reach Live's own parts outside src/daw/; go through the DAW's provider:\n{}", found.join("\n"));
}

#[test]
fn each_allowed_use_is_still_there() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    for (file, phrase, why) in ALLOWED {
        let text = std::fs::read_to_string(root.join(file)).unwrap();
        assert!(text.contains(phrase), "{file} no longer has {phrase} ({why}): take it off the list");
    }
}
