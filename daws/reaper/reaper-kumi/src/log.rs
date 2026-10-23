//! The bridge's own log (REAPER keeps none for extensions): one file in the log folder the reference names, kept
//! under 1 MB by moving the old one aside, and the logs of launches over a week old removed. Until a folder is known,
//! lines go to stderr.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const MAX_BYTES: u64 = 1024 * 1024;
const KEEP: Duration = Duration::from_secs(7 * 24 * 60 * 60);

static FILE: Mutex<Option<PathBuf>> = Mutex::new(None);

pub fn set_dir(dir: Option<PathBuf>) {
    if let Some(dir) = &dir {
        prune(dir);
    }
    let path = dir.and_then(|dir| fs::create_dir_all(&dir).ok().map(|_| dir.join(format!("reaper-{}.log", std::process::id()))));
    *FILE.lock().unwrap_or_else(|e| e.into_inner()) = path;
}

// Each launch of REAPER logs to a file of its own (named by pid): the ones from over a week ago go.
fn prune(dir: &std::path::Path) {
    let Ok(entries) = fs::read_dir(dir) else { return };
    let mine = format!("reaper-{}.", std::process::id());
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let ours = name.starts_with("reaper-") && (name.ends_with(".log") || name.ends_with(".log.1"));
        let old = entry.metadata().and_then(|m| m.modified()).ok().and_then(|t| t.elapsed().ok()).is_some_and(|age| age > KEEP);
        if ours && old && !name.starts_with(&mine) {
            let _ = fs::remove_file(entry.path());
        }
    }
}

pub fn line(message: &str) {
    let secs = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.0);
    let text = format!("{secs:.3} {message}\n");
    let file = FILE.lock().unwrap_or_else(|e| e.into_inner());
    let Some(path) = file.as_ref() else {
        // Not eprint!: it panics when stderr is closed, and REAPER on Windows has none.
        let _ = std::io::stderr().write_all(format!("reaper_kumi: {text}").as_bytes());
        return;
    };
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
    fn logs_of_earlier_launches_go_after_a_week() {
        let dir = tempfile::tempdir().unwrap();
        let old = SystemTime::now() - Duration::from_secs(8 * 24 * 60 * 60);
        for name in ["reaper-1.log", "reaper-1.log.1", "reaper-2.log", "notes.log"] {
            let file = fs::File::create(dir.path().join(name)).unwrap();
            if name != "reaper-2.log" {
                file.set_modified(old).unwrap();
            }
        }
        let mine = dir.path().join(format!("reaper-{}.log", std::process::id()));
        fs::File::create(&mine).unwrap().set_modified(old).unwrap();
        prune(dir.path());
        let mut left: Vec<String> =
            fs::read_dir(dir.path()).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
        left.sort();
        let mut want = vec!["notes.log".to_string(), "reaper-2.log".to_string(), mine.file_name().unwrap().to_string_lossy().into_owned()];
        want.sort();
        assert_eq!(left, want);
    }
}
