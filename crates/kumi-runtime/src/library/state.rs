use super::{
    learn::{Counts, LearnPhase, LearnProgress},
    store::{read_json, write_json},
};
use kumi_common::{js::json::stringify, time::now_ms};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    io,
    path::{Path, PathBuf},
};
use tokio::io::AsyncWriteExt;
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Learning {
    pub pid: i64,
    pub started_at: i64,
    pub phase: LearnPhase,
    pub sounds: Counts,
    pub presets: Counts,
    pub sets: Counts,
    pub updated_at: i64,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LastRun {
    pub started_at: i64,
    pub finished_at: i64,
    pub sounds: usize,
    pub presets: usize,
    pub sets: usize,
    pub failed: usize,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LibraryState {
    pub version: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last: Option<LastRun>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub learning: Option<Learning>,
}
/// `process.kill(pid, 0)`, including a process that exists but belongs to another user.
pub fn alive(pid: f64) -> bool {
    if !pid.is_finite() || pid.fract() != 0. || pid < i32::MIN as f64 || pid > i32::MAX as f64 {
        return false;
    }
    #[cfg(unix)]
    {
        unsafe { libc::kill(pid as libc::pid_t, 0) == 0 || io::Error::last_os_error().raw_os_error() == Some(libc::EPERM) }
    }
    #[cfg(windows)]
    {
        use std::ffi::c_void;
        #[link(name = "kernel32")]
        unsafe extern "system" {
            fn OpenProcess(access: u32, inherit: i32, pid: u32) -> *mut c_void;
            fn GetExitCodeProcess(handle: *mut c_void, code: *mut u32) -> i32;
            fn CloseHandle(handle: *mut c_void) -> i32;
            fn GetLastError() -> u32;
        }
        if pid <= 0. {
            return false;
        }
        unsafe {
            let handle = OpenProcess(0x1000, 0, pid as u32);
            if handle.is_null() {
                return GetLastError() == 5;
            }
            let mut code = 0;
            let alive = GetExitCodeProcess(handle, &mut code) != 0 && code == 259;
            CloseHandle(handle);
            alive
        }
    }
    #[cfg(not(any(unix, windows)))]
    {
        pid == std::process::id() as f64
    }
}
/// Whether the process that wrote a record at `since` (ms) is still running: `pid` alive, and not gone to a process
/// started after the record (a second's slack for how finely a system keeps start times). Where the system can't say
/// when it started, alive is enough.
pub fn still_running(pid: f64, since: f64) -> bool {
    if !alive(pid) {
        return false;
    }
    match u32::try_from(pid as i64).ok().and_then(kumi_common::process::started_at_ms) {
        Some(started) => started as f64 <= since + 1000.,
        None => true,
    }
}
pub async fn read_state(dir: &str) -> Option<LibraryState> {
    let mut state = read_json::<LibraryState>(&Path::new(dir).join("state.json")).await?;
    if state.version != 1 {
        return None;
    }
    if state.learning.as_ref().is_some_and(|l| !still_running(l.pid as f64, l.updated_at as f64)) {
        state.learning = None;
    }
    Some(state)
}
pub async fn write_state(dir: &str, progress: &LearnProgress, stopped: bool) -> io::Result<()> {
    let file = Path::new(dir).join("state.json");
    let before = read_json::<LibraryState>(&file).await;
    let mut state = LibraryState { version: 1, last: before.filter(|s| s.version == 1).and_then(|s| s.last), learning: None };
    if progress.phase == LearnPhase::Done {
        state.last = Some(LastRun {
            started_at: progress.started_at,
            finished_at: progress.finished_at.unwrap_or_else(now_ms),
            sounds: progress.sounds.known,
            presets: progress.presets.known,
            sets: progress.sets.known,
            failed: progress.failed,
        });
    } else if !stopped {
        state.learning = Some(Learning {
            pid: std::process::id() as i64,
            started_at: progress.started_at,
            phase: progress.phase,
            sounds: progress.sounds.clone(),
            presets: progress.presets.clone(),
            sets: progress.sets.clone(),
            updated_at: now_ms(),
        });
    }
    write_json(&file, &state).await
}
pub struct LibraryLock {
    file: PathBuf,
}
impl LibraryLock {
    pub async fn release(self) -> io::Result<()> {
        match tokio::fs::remove_file(&self.file).await {
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
            other => other,
        }
    }
}
/// The logs being written afresh when a learner was stopped dead (`.sounds-<uuid>` and the like): only a lock's holder
/// writes them, so once it's taken any left are a gone learner's.
async fn remove_left_logs(dir: &Path) {
    let Ok(mut entries) = tokio::fs::read_dir(dir).await else { return };
    while let Ok(Some(entry)) = entries.next_entry().await {
        let name = entry.file_name();
        if [".sounds-", ".presets-", ".sets-"].iter().any(|kind| name.to_string_lossy().starts_with(kind)) {
            let _ = tokio::fs::remove_file(entry.path()).await;
        }
    }
}
/// A same-process or day-old lock is replaced, exactly as the source learner does, and one whose pid has gone to a
/// process started after it was taken (the learner that took it was killed).
pub async fn acquire_lock(dir: &str) -> io::Result<Option<LibraryLock>> {
    let mut builder = tokio::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    builder.mode(0o700);
    builder.create(dir).await?;
    let file = Path::new(dir).join("learning.lock");
    for _ in 0..2 {
        let mut options = tokio::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        options.mode(0o600);
        match options.open(&file).await {
            Ok(mut handle) => {
                handle.write_all(stringify(&json!({"pid":std::process::id(),"at":now_ms()})).as_bytes()).await?;
                handle.flush().await?;
                drop(handle);
                remove_left_logs(Path::new(dir)).await;
                return Ok(Some(LibraryLock { file }));
            }
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
                let holder = read_json::<Value>(&file).await.unwrap_or(Value::Null);
                if let (Some(pid), Some(at)) = (holder["pid"].as_f64(), holder["at"].as_f64()) {
                    if pid != std::process::id() as f64 && still_running(pid, at) && now_ms() as f64 - at < 86400000. {
                        return Ok(None);
                    }
                }
                match tokio::fs::remove_file(&file).await {
                    Err(e) if e.kind() == io::ErrorKind::NotFound => {}
                    result => result?,
                }
            }
            Err(e) => return Err(e),
        }
    }
    Ok(None)
}
