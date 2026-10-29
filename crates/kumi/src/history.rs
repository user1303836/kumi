//! Input history kept across conversations and restarts, with secrets and key-shaped text removed.
use crate::text::sanitize_text;
use fancy_regex::Regex;
use kumi_common::js::{json, string};
use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    sync::LazyLock,
};
const MAX_ENTRIES: usize = 500;
const MAX_ENTRY: usize = 4096;
static KEYS: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    [
        r"(?<![A-Za-z0-9_])(?:sk|pk|rk)-[A-Za-z0-9_-]{16,}",
        r"(?<![A-Za-z0-9_])AIza[0-9A-Za-z_-]{20,}",
        r"(?<![A-Za-z0-9_])gh[pousr]_[A-Za-z0-9]{20,}",
        r"(?<![A-Za-z0-9_])xox[abprs]-[A-Za-z0-9-]{10,}",
        r"(?<![A-Za-z0-9_])eyJ[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{8,}",
    ]
    .into_iter()
    .map(|p| Regex::new(p).unwrap())
    .collect()
});
static LABELLED: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)(?<![A-Za-z0-9_])(api[_-]?key|token|secret|password)([\s\u{feff}]*[:=][\s\u{feff}]*)[^\s\u{feff}]+").unwrap()
});
/// The characters of a key without a known shape: letters and digits, and in what the producer types `_` and `-` too.
fn joined(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_' || c == '-'
}
/// `text` with each run of `key` characters 32 or more long that holds a digit and a letter taken out whole: a key
/// without a known shape. In one pass, as whole runs: a run's later starts see only part of it, so none of them is one
/// where the run isn't.
fn long_runs_out(text: &str, key: fn(char) -> bool) -> String {
    let mut out = String::with_capacity(text.len());
    let mut start = None;
    let ended = |out: &mut String, run: &str| {
        let shaped = run.len() >= 32 && run.bytes().any(|b| b.is_ascii_digit()) && run.bytes().any(|b| b.is_ascii_alphabetic());
        out.push_str(if shaped { "[redacted]" } else { run });
    };
    for (at, c) in text.char_indices() {
        match (key(c), start) {
            (true, None) => start = Some(at),
            (true, Some(_)) => {}
            (false, Some(from)) => {
                ended(&mut out, &text[from..at]);
                start = None;
                out.push(c);
            }
            (false, None) => out.push(c),
        }
    }
    if let Some(from) = start {
        ended(&mut out, &text[from..]);
    }
    out
}
/// `text` without anything shaped like a key, or set after a word that names one ("token: …"); a key without a known
/// shape is a long run of `key` characters.
fn without_keys(mut kept: String, key: fn(char) -> bool) -> String {
    for pattern in KEYS.iter() {
        kept = pattern.replace_all(&kept, "[redacted]").into_owned();
    }
    LABELLED.replace_all(&long_runs_out(&kept, key), "${1}${2}[redacted]").into_owned()
}
pub fn history_text(text: &str, secrets: &[String]) -> String {
    string::head(&redacted(text, secrets), MAX_ENTRY)
}
fn redacted(text: &str, secrets: &[String]) -> String {
    string::trim(&without_keys(sanitize_text(text, secrets), joined)).to_string()
}
/// What a conversation's log keeps of its words: the input history's redaction, `secrets` (the keys this computer knows)
/// then anything shaped like a key, with the words otherwise as they were (the screen cleans them as it draws them). A
/// known key under 8 characters (a local model server's "none") is a word, not a secret, and a key without a known shape
/// is a long run of letters and digits alone: names joined with `_` or `-` (a sample's, a preset's, a bounce's) are what
/// the conversation is about.
pub fn log_redactor(secrets: Vec<String>) -> impl Fn(&str) -> String {
    let mut secrets: Vec<String> = secrets.into_iter().filter(|secret| string::utf16_len(secret) >= 8).collect();
    // A longer key that holds a shorter one goes first, whole.
    secrets.sort_by_key(|secret| std::cmp::Reverse(string::utf16_len(secret)));
    secrets.dedup();
    move |text| {
        let mut kept = text.to_owned();
        for secret in &secrets {
            if kept.contains(secret.as_str()) {
                kept = kept.replace(secret.as_str(), "[redacted]");
            }
        }
        without_keys(kept, |c| c.is_ascii_alphanumeric())
    }
}
pub struct InputHistory {
    entries: Vec<String>,
    /// Whether each entry is in the file: a draft cleared with ctrl+c is kept only while Kumi runs.
    saved: Vec<bool>,
    file: Option<PathBuf>,
    secrets: Vec<String>,
    lines: usize,
}
pub fn open_input_history(file: Option<PathBuf>, secrets: Vec<String>) -> InputHistory {
    let mut entries: Vec<_> = file
        .as_ref()
        .and_then(|p| fs::read(p).ok())
        .map(|data| {
            String::from_utf8_lossy(&data)
                .split('\n')
                .filter_map(|line| serde_json::from_str::<String>(line).ok())
                .map(|s| history_text(&s, &secrets))
                .filter(|s| !s.is_empty())
                .collect()
        })
        .unwrap_or_default();
    if entries.len() > MAX_ENTRIES {
        entries.drain(..entries.len() - MAX_ENTRIES);
    }
    let lines = entries.len();
    InputHistory { saved: vec![true; entries.len()], entries, file, secrets, lines }
}
impl InputHistory {
    pub fn entries(&self) -> &[String] {
        &self.entries
    }
    pub fn add(&mut self, text: &str) {
        let kept = history_text(text, &self.secrets);
        if !self.push(&kept, true) {
            return;
        }
        let Some(file) = self.file.clone() else {
            return;
        };
        let _ = self.write(&file, &kept);
    }
    /// A draft cleared without being sent: among the entries while Kumi runs, whole, but never written to the file.
    pub fn keep(&mut self, text: &str) {
        let kept = redacted(text, &self.secrets);
        self.push(&kept, false);
    }
    fn push(&mut self, kept: &str, saved: bool) -> bool {
        if kept.is_empty() {
            return false;
        }
        if self.entries.last().is_some_and(|last| last == kept) {
            // A draft kept from ctrl+c, now sent: it stays one entry, and is written now.
            let unsaved = saved && self.saved.last() == Some(&false);
            if unsaved {
                *self.saved.last_mut().expect("an entry") = true;
            }
            return unsaved;
        }
        self.entries.push(kept.to_string());
        self.saved.push(saved);
        if self.entries.len() > MAX_ENTRIES {
            let over = self.entries.len() - MAX_ENTRIES;
            self.entries.drain(..over);
            self.saved.drain(..over);
        }
        true
    }
    fn write(&mut self, file: &Path, kept: &str) -> std::io::Result<()> {
        let folder = file.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or(Path::new("."));
        let mut builder = fs::DirBuilder::new();
        builder.recursive(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        builder.create(folder)?;
        self.lines += 1;
        let compact = self.lines > 2 * MAX_ENTRIES;
        let mut options = OpenOptions::new();
        options.create(true).write(true);
        if compact {
            options.truncate(true);
        } else {
            options.append(true);
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut handle = options.open(file)?;
        if compact {
            let saved: Vec<_> = self.entries.iter().zip(&self.saved).filter(|(_, saved)| **saved).map(|(s, _)| json::quote(s)).collect();
            handle.write_all(format!("{}\n", saved.join("\n")).as_bytes())?;
            self.lines = saved.len();
        } else {
            handle.write_all(format!("{}\n", json::quote(kept)).as_bytes())?;
        }
        Ok(())
    }
}
