//! The bridge's own log: one file per run (`logic-<pid>.log`) in the log folder, kept under 1 MB by moving the old one
//! aside, the logs of runs over a week old removed; and every line on stderr as well, for a terminal that runs it.
//! The folder is Kumi's log folder for the bridge, never the folder the bridge's own files are installed in, whose
//! hash the lifecycle checks.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const MAX_BYTES: u64 = 1024 * 1024;
const KEEP: Duration = Duration::from_secs(7 * 24 * 60 * 60);
const PREFIX: &str = "logic-";

static FILE: Mutex<Option<PathBuf>> = Mutex::new(None);

/// Logs to `<dir>/logic-<pid>.log` from now on (None: stderr only); the logs of runs over a week old go.
pub fn set_dir(dir: Option<PathBuf>) {
    if let Some(dir) = &dir {
        prune(dir);
    }
    let path = dir.and_then(|dir| fs::create_dir_all(&dir).ok().map(|_| dir.join(format!("{PREFIX}{}.log", std::process::id()))));
    *FILE.lock().unwrap_or_else(|e| e.into_inner()) = path;
}

/// The file lines go to, if one is set.
pub fn file() -> Option<PathBuf> {
    FILE.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

// Each run logs to a file of its own (named by pid): the ones from over a week ago go.
fn prune(dir: &Path) {
    let Ok(entries) = fs::read_dir(dir) else { return };
    let mine = format!("{PREFIX}{}.", std::process::id());
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let ours = name.starts_with(PREFIX) && (name.ends_with(".log") || name.ends_with(".log.1"));
        let old = entry.metadata().and_then(|m| m.modified()).ok().and_then(|t| t.elapsed().ok()).is_some_and(|age| age > KEEP);
        if ours && old && !name.starts_with(&mine) {
            let _ = fs::remove_file(entry.path());
        }
    }
}

pub fn line(message: &str) {
    let secs = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.0);
    let text = format!("{secs:.3} {message}\n");
    // Not eprint!: it panics when stderr is closed, and a bridge started by Kumi may have none.
    let _ = std::io::stderr().write_all(format!("logic-kumi: {text}").as_bytes());
    let file = FILE.lock().unwrap_or_else(|e| e.into_inner());
    let Some(path) = file.as_ref() else { return };
    if fs::metadata(path).is_ok_and(|m| m.len() > MAX_BYTES) {
        let _ = fs::rename(path, path.with_extension("log.1"));
    }
    if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(path) {
        let _ = f.write_all(text.as_bytes());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn logs_of_earlier_runs_go_after_a_week() {
        let dir = tempfile::tempdir().unwrap();
        let old = SystemTime::now() - Duration::from_secs(8 * 24 * 60 * 60);
        for name in ["logic-1.log", "logic-1.log.1", "logic-2.log", "notes.log"] {
            let file = fs::File::create(dir.path().join(name)).unwrap();
            if name != "logic-2.log" {
                file.set_modified(old).unwrap();
            }
        }
        let mine = dir.path().join(format!("logic-{}.log", std::process::id()));
        fs::File::create(&mine).unwrap().set_modified(old).unwrap();
        prune(dir.path());
        let mut left: Vec<String> =
            fs::read_dir(dir.path()).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
        left.sort();
        let mut want = vec!["notes.log".to_string(), "logic-2.log".to_string(), mine.file_name().unwrap().to_string_lossy().into_owned()];
        want.sort();
        assert_eq!(left, want);
    }
}
