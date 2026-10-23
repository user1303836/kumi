//! Kumi's runtime never reaches into Live's integration (kumi-daw-ableton, which depends on it): nothing in its sources
//! names it. Only the app's registry does.

use std::path::{Path, PathBuf};

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

#[test]
fn nothing_outside_lives_integration_and_the_registry_names_it() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = vec![];
    sources(&root, &mut files);
    let mut found = vec![];
    for file in files {
        let name = file.strip_prefix(&root).unwrap().to_string_lossy().replace('\\', "/");
        let text = std::fs::read_to_string(&file).unwrap();
        let mut using: Option<usize> = None;
        for (n, line) in text.lines().enumerate() {
            let code = code(line);
            // A `use` names Live's integration however it's written: `integrations::{ableton as live}` too, across
            // lines.
            if using.is_none() && code.trim_start().trim_start_matches("pub ").trim_start_matches("pub(crate) ").starts_with("use ") {
                using = Some(n);
            }
            let named = code.split(|c: char| !(c.is_alphanumeric() || c == '_')).any(|word| word == "ableton");
            if code.contains("ableton::") || code.contains("mod ableton") || (using.is_some() && named) {
                found.push(format!("{name}:{}: {}", n + 1, line.trim()));
            }
            if code.contains(';') {
                using = None;
            }
        }
    }
    assert!(found.is_empty(), "these reach into Live's integration; take what they need from a neutral module:\n{}", found.join("\n"));
}
