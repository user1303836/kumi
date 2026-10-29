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
        r"(?<![A-Za-z0-9_])(?=[A-Za-z0-9_-]*[0-9])(?=[A-Za-z0-9_-]*[A-Za-z])[A-Za-z0-9_-]{32,}",
    ]
    .into_iter()
    .map(|p| Regex::new(p).unwrap())
    .collect()
});
static LABELLED: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)(?<![A-Za-z0-9_])(api[_-]?key|token|secret|password)([\s\u{feff}]*[:=][\s\u{feff}]*)[^\s\u{feff}]+").unwrap()
});
pub fn history_text(text: &str, secrets: &[String]) -> String {
    string::head(&redacted(text, secrets), MAX_ENTRY)
}
fn redacted(text: &str, secrets: &[String]) -> String {
    let mut kept = sanitize_text(text, secrets);
    for pattern in KEYS.iter() {
        kept = pattern.replace_all(&kept, "[redacted]").into_owned();
    }
    string::trim(&LABELLED.replace_all(&kept, "${1}${2}[redacted]")).to_string()
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
        if kept.is_empty() || self.entries.last().is_some_and(|last| last == kept) {
            return false;
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
