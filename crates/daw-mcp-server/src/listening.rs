//! The host's listening tools (M5 contract §2): offline renders and live captures as jobs the bridge runs while the host
//! answers at once (`render`, `hear`, then `listen_job` and `listen_cancel`), a file played out loud (`audition`,
//! `audition_stop`), the audio behind a ref (`audio_file`) and scratch copies of a track (`scratch`). One implementation
//! for both DAWs: each checks its arguments, asks the bridge and keeps the jobs; what a job renders is the bridge's.
use crate::host::{Host, Pick};
use serde_json::{json, Map, Value};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime};

/// Starting a job: the bridge only checks and queues it, so it answers quickly.
const START_TIMEOUT: Duration = Duration::from_secs(5);
/// A job's status, a cancel or a stop: the bridge answers them at once, even mid-render.
const STATUS_TIMEOUT: Duration = Duration::from_secs(3);
/// Copies of a heavy track take a while to make (Renoise makes them in slices).
const SCRATCH_TIMEOUT: Duration = Duration::from_secs(30);
/// Opening a file to play it, or saving a sample's audio.
const FILE_TIMEOUT: Duration = Duration::from_secs(10);
/// How often `listen_job` asks the bridge while it waits, and the longest it waits.
const POLL: Duration = Duration::from_millis(150);
pub const MAX_WAIT_MS: u64 = 10_000;
/// The most sources one render takes (8 candidates, a focus, the mix and the mix before its chain, or a search's copies).
pub const MAX_SOURCES: usize = 18;
/// The most jobs the host keeps, and how long a finished one stays once its end has been read.
pub const MAX_JOBS: usize = 32;
const FORGET_AFTER: Duration = Duration::from_secs(60);
/// A job nobody has asked about for this long is asked about at the next start: its end is learnt, so it neither counts
/// toward `MAX_JOBS` nor keeps its files from pruning once it's over.
const ASK_AFTER: Duration = Duration::from_secs(60);
/// The listening folder loses files older than this, then the oldest while it holds more than `MAX_FOLDER_BYTES`.
const PRUNE_AGE: Duration = Duration::from_secs(2 * 3600);
pub const MAX_FOLDER_BYTES: u64 = 2 * 1024 * 1024 * 1024;
/// The longest live capture, in seconds, and the most a render may start before its part, in beats.
pub const MAX_CAPTURE_SECONDS: f64 = 60.;
pub const MAX_PRE_ROLL_BEATS: f64 = 64.;
pub const SAMPLE_RATES: [u64; 4] = [44_100, 48_000, 88_200, 96_000];
/// The most copies one scratch make takes, and the most values one set takes.
pub const MAX_COPIES: u64 = 16;
pub const MAX_SCRATCH_VALUES: usize = 256;
/// A job's states that end it.
const TERMINAL: [&str; 3] = ["done", "failed", "cancelled"];

/// One job the host started: what kind, the files it will leave, and whether its end has been read.
#[derive(Debug, Clone)]
struct Job {
    kind: &'static str,
    files: Vec<PathBuf>,
    ended: bool,
    /// When `listen_job` first answered its end: it's forgotten `FORGET_AFTER` later.
    read_end_at: Option<Instant>,
    /// When its state was last learnt (its start, or an answer about it).
    asked_at: Instant,
}

#[derive(Default)]
struct JobsInner {
    jobs: HashMap<String, Job>,
    /// Oldest first.
    order: Vec<String>,
    next: u64,
}

/// The jobs the host has started (renders and captures), shared by every call it serves at once.
#[derive(Default)]
pub struct Jobs {
    inner: Mutex<JobsInner>,
}

/// Four base-36 characters from a counter and the clock: ids unique among what the host keeps.
fn token(seed: u64) -> String {
    const DIGITS: &[u8] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    let mut n = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ (seed >> 7);
    (0..4)
        .map(|_| {
            let digit = DIGITS[(n % 36) as usize] as char;
            n /= 36;
            digit
        })
        .collect()
}

fn now_ms() -> u64 {
    SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

impl Jobs {
    /// A new id for what the host hands the bridge: `j…` a job, `p…` a preview, `s…` a set of scratch copies.
    pub fn id(&self, prefix: char) -> String {
        let mut inner = self.inner.lock().expect("jobs lock");
        inner.next += 1;
        format!("{prefix}{}{}", inner.next, token(inner.next ^ now_ms()))
    }

    /// Forgets the jobs whose end was read a while ago, then keeps this one. Refused when the host already keeps
    /// `MAX_JOBS` that haven't ended.
    fn add(&self, id: &str, kind: &'static str, files: Vec<PathBuf>) -> Result<(), String> {
        let mut inner = self.inner.lock().expect("jobs lock");
        let stale: Vec<String> = inner
            .order
            .iter()
            .filter(|id| inner.jobs.get(*id).and_then(|job| job.read_end_at).is_some_and(|at| at.elapsed() >= FORGET_AFTER))
            .cloned()
            .collect();
        for id in stale {
            inner.jobs.remove(&id);
            inner.order.retain(|kept| *kept != id);
        }
        while inner.order.len() >= MAX_JOBS {
            let Some(at) = inner.order.iter().position(|id| inner.jobs.get(id).is_some_and(|job| job.ended)) else {
                return Err(format!("{MAX_JOBS} listening jobs are still running: wait for one to end"));
            };
            let id = inner.order.remove(at);
            inner.jobs.remove(&id);
        }
        inner.jobs.insert(id.to_string(), Job { kind, files, ended: false, read_end_at: None, asked_at: Instant::now() });
        inner.order.push(id.to_string());
        Ok(())
    }

    fn get(&self, id: &str) -> Option<Job> {
        self.inner.lock().expect("jobs lock").jobs.get(id).cloned()
    }

    /// The job has ended (a `listen_job` answer said so).
    fn ended(&self, id: &str) {
        if let Some(job) = self.inner.lock().expect("jobs lock").jobs.get_mut(id) {
            job.ended = true;
            job.read_end_at.get_or_insert_with(Instant::now);
        }
    }

    /// Its state was just learnt (still running).
    fn asked(&self, id: &str) {
        if let Some(job) = self.inner.lock().expect("jobs lock").jobs.get_mut(id) {
            job.asked_at = Instant::now();
        }
    }

    /// The jobs that haven't ended and nobody has asked about for `ASK_AFTER` (all of them when `all`), oldest first.
    fn unasked(&self, all: bool) -> Vec<String> {
        let inner = self.inner.lock().expect("jobs lock");
        inner
            .order
            .iter()
            .filter(|id| inner.jobs.get(*id).is_some_and(|job| !job.ended && (all || job.asked_at.elapsed() >= ASK_AFTER)))
            .cloned()
            .collect()
    }

    /// The jobs that haven't ended, oldest first.
    pub fn running(&self) -> Vec<String> {
        let inner = self.inner.lock().expect("jobs lock");
        inner.order.iter().filter(|id| inner.jobs.get(*id).is_some_and(|job| !job.ended)).cloned().collect()
    }

    /// The files of the jobs that haven't ended: pruning never takes them.
    fn running_files(&self) -> Vec<PathBuf> {
        let inner = self.inner.lock().expect("jobs lock");
        inner.jobs.values().filter(|job| !job.ended).flat_map(|job| job.files.iter().cloned()).collect()
    }
}

/// Free space on the disk holding `path` (its nearest folder that exists), in bytes; None when the system won't say.
pub fn free_bytes(path: &Path) -> Option<u64> {
    let mut at = path.to_path_buf();
    while !at.exists() {
        at = at.parent()?.to_path_buf();
    }
    free_on(&at)
}

#[cfg(unix)]
#[allow(clippy::unnecessary_cast)] // (statvfs's field types differ by platform)
fn free_on(path: &Path) -> Option<u64> {
    use std::os::unix::ffi::OsStrExt;
    let name = std::ffi::CString::new(path.as_os_str().as_bytes()).ok()?;
    // SAFETY: an all-zero statvfs is a valid value to be written over.
    let mut stats: libc::statvfs = unsafe { std::mem::zeroed() };
    // SAFETY: `name` is a valid C string and `stats` is a writable statvfs struct.
    if unsafe { libc::statvfs(name.as_ptr(), &mut stats) } != 0 {
        return None;
    }
    Some((stats.f_bavail as u64).saturating_mul(stats.f_frsize as u64))
}

#[cfg(windows)]
fn free_on(path: &Path) -> Option<u64> {
    use std::os::windows::ffi::OsStrExt;
    #[link(name = "kernel32")]
    extern "system" {
        fn GetDiskFreeSpaceExW(directory: *const u16, free_to_caller: *mut u64, total: *mut u64, total_free: *mut u64) -> i32;
    }
    let wide: Vec<u16> = path.as_os_str().encode_wide().chain(std::iter::once(0)).collect();
    let (mut free, mut total, mut total_free) = (0u64, 0u64, 0u64);
    // SAFETY: `wide` is NUL-terminated and the three out-pointers are valid for writes.
    let ok = unsafe { GetDiskFreeSpaceExW(wide.as_ptr(), &mut free, &mut total, &mut total_free) };
    (ok != 0).then_some(free)
}

#[cfg(not(any(unix, windows)))]
fn free_on(_path: &Path) -> Option<u64> {
    None
}

/// The listening folder without what's old (older than `PRUNE_AGE`), then without the oldest while it holds more than
/// `most` bytes. Only the audio the bridges wrote there: never a folder in it (Kumi's `kept/`), never one of `keep` (a
/// running job's), and never the bridges' own files (the scratch journal, which they keep for weeks to sweep copies a
/// crash left behind). How many files went.
pub fn prune(folder: &Path, keep: &[PathBuf], age: Duration, most: u64) -> usize {
    let Ok(entries) = std::fs::read_dir(folder) else { return 0 };
    let audio = |path: &Path| path.extension().and_then(|ext| ext.to_str()).is_some_and(|ext| ext.eq_ignore_ascii_case("wav"));
    let mut files: Vec<(PathBuf, SystemTime, u64)> = entries
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let meta = entry.metadata().ok()?;
            meta.is_file().then(|| (entry.path(), meta.modified().unwrap_or(SystemTime::UNIX_EPOCH), meta.len()))
        })
        .filter(|(path, _, _)| audio(path) && !keep.contains(path))
        .collect();
    files.sort_by_key(|(_, modified, _)| *modified);
    let now = SystemTime::now();
    let mut removed = 0;
    let mut total: u64 = files.iter().map(|(_, _, size)| size).sum();
    for (path, modified, size) in files {
        let old = now.duration_since(modified).is_ok_and(|elapsed| elapsed >= age);
        if !(old || total > most) {
            continue;
        }
        if std::fs::remove_file(&path).is_ok() {
            removed += 1;
            total -= size;
        }
    }
    removed
}

fn bad(why: impl Into<String>) -> (Value, bool) {
    (json!({"error": why.into(), "code": "bad-params"}), true)
}

/// A job id the host doesn't keep (never started here, or ended and forgotten): its own code, not a malformed request.
fn unknown_job(id: &str) -> (Value, bool) {
    (
        json!({"error": format!("{id} isn't a job this host keeps (it never started here, or it ended a while ago)"), "code": "unknown-job"}),
        true,
    )
}

fn number(arguments: &Value, name: &str) -> Result<Option<f64>, String> {
    match arguments.get(name) {
        None | Some(Value::Null) => Ok(None),
        Some(value) => value.as_f64().filter(|n| n.is_finite()).map(Some).ok_or_else(|| format!("{name} must be a number")),
    }
}

fn text<'a>(arguments: &'a Value, name: &str) -> Result<Option<&'a str>, String> {
    match arguments.get(name) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(text)) if !text.is_empty() => Ok(Some(text)),
        Some(_) => Err(format!("{name} must be a non-empty string")),
    }
}

fn required<'a>(arguments: &'a Value, name: &str) -> Result<&'a str, String> {
    text(arguments, name)?.ok_or_else(|| format!("{name} must be a non-empty string"))
}

/// The epoch and layout the request's refs came from, copied as given (the bridge refuses others as stale).
fn place(arguments: &Value, params: &mut Map<String, Value>) -> Result<(), String> {
    if let Some(epoch) = text(arguments, "epoch")? {
        params.insert("epoch".into(), epoch.into());
    }
    match arguments.get("layout") {
        None | Some(Value::Null) => {}
        Some(Value::Number(n)) if n.as_u64().is_some() => {
            params.insert("layout".into(), n.clone().into());
        }
        Some(_) => return Err("layout must be a whole number from 0".into()),
    }
    Ok(())
}

/// A source id: Kumi's name for what a file holds.
fn source_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 32 && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

/// The sources a render takes, each checked and with only the fields it knows.
fn sources(arguments: &Value) -> Result<Vec<Value>, String> {
    let given = arguments.get("sources").and_then(Value::as_array).ok_or("sources must be a list")?;
    if given.is_empty() || given.len() > MAX_SOURCES {
        return Err(format!("sources: 1 to {MAX_SOURCES} of them"));
    }
    let mut ids = vec![];
    let mut out = vec![];
    for source in given {
        let id = text(source, "id")?.filter(|id| source_id(id)).ok_or("each source's id: up to 32 letters, digits, _ or -")?;
        if ids.contains(&id) {
            return Err(format!("source id {id} is given twice"));
        }
        ids.push(id);
        let mut row = Map::new();
        row.insert("id".into(), id.into());
        match text(source, "what")? {
            Some("master") => {
                row.insert("what".into(), "master".into());
                match source.get("beforeChain") {
                    None | Some(Value::Null) | Some(Value::Bool(false)) => {}
                    Some(Value::Bool(true)) => {
                        row.insert("beforeChain".into(), true.into());
                    }
                    Some(_) => return Err("beforeChain must be true or false".into()),
                }
                if source.get("ref").is_some_and(|r| !r.is_null()) {
                    return Err("the master source takes no ref".into());
                }
            }
            Some("track") => {
                row.insert("what".into(), "track".into());
                row.insert("ref".into(), required(source, "ref")?.into());
                match text(source, "stage")? {
                    None => {}
                    Some(stage @ ("pre-fader" | "post-fader" | "items")) => {
                        row.insert("stage".into(), stage.into());
                    }
                    Some(_) => return Err("stage is pre-fader, post-fader or items".into()),
                }
            }
            _ => return Err("each source's what: master or track".into()),
        }
        out.push(Value::Object(row));
    }
    // A track's own items (before its FX) render on their own, from an audio accessor, not in a render with others.
    if out.len() > 1 && out.iter().any(|source| source["stage"] == "items") {
        return Err("a track's items (stage items) render alone: one source".into());
    }
    Ok(out)
}

/// A stretch in beats (quarter notes from the project's start).
fn range(arguments: &Value, name: &str) -> Result<Value, String> {
    let range = arguments.get(name).filter(|r| r.is_object()).ok_or_else(|| format!("{name} must be {{fromBeat, beats}}"))?;
    let from = number(range, "fromBeat")?.filter(|from| *from >= 0.);
    let beats = number(range, "beats")?.filter(|beats| *beats > 0.);
    match (from, beats) {
        (Some(from), Some(beats)) => Ok(json!({"fromBeat": from, "beats": beats})),
        _ => Err(format!("{name}: fromBeat from 0, and beats more than 0")),
    }
}

/// The paths a bridge's answer names under `files` (`[{id, file}]`).
fn files_of(answer: &Value) -> Vec<PathBuf> {
    answer
        .get("files")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|file| file.get("file").and_then(Value::as_str))
        .map(PathBuf::from)
        .collect()
}

impl Host {
    /// Where the bridges leave what they render: `<Kumi's folder>/bridges/<daw>/listening`.
    pub fn listening_folder(&self) -> PathBuf {
        self.paths().root.join("listening")
    }

    /// The listening folder pruned, and the bytes free on its disk (None when the system won't say).
    async fn tidy_folder(&self) -> Option<u64> {
        let folder = self.listening_folder();
        let keep = self.jobs().running_files();
        tokio::task::spawn_blocking(move || {
            prune(&folder, &keep, PRUNE_AGE, MAX_FOLDER_BYTES);
            free_bytes(&folder)
        })
        .await
        .ok()
        .flatten()
    }

    /// Asks the bridge, and answers with its JSON or the call's error. A bridge without the method is older than M5.
    async fn listening_call(&self, method: &str, params: Value, pick: &Pick, timeout: Duration) -> Result<Value, (Value, bool)> {
        match self.call(method, params, pick, timeout).await {
            Ok(answer) => Ok(answer),
            Err(crate::host::CallError::Bridge { code, .. }) if code == daw_bridge::wire::code::UNKNOWN_METHOD => {
                let daw = self.daw();
                Err((
                    json!({"error": format!("this {daw} bridge can't listen yet (it's older than Kumi): update Kumi's bridge and restart {daw}"), "code": "failed", "connected": true, "daw": daw.id()}),
                    true,
                ))
            }
            Err(error) => Err((self.call_error(&error), true)),
        }
    }

    /// `render`: an offline render of a stretch, started as a job.
    pub async fn render(&self, arguments: &Value) -> (Value, bool) {
        let pick = match Pick::from_arguments(arguments) {
            Ok(pick) => pick,
            Err(why) => return bad(why),
        };
        let mut params = Map::new();
        let checked = (|| {
            params.insert("sources".into(), Value::Array(sources(arguments)?));
            params.insert("range".into(), range(arguments, "range")?);
            let pre_roll = number(arguments, "preRollBeats")?.unwrap_or(0.);
            if !(0. ..=MAX_PRE_ROLL_BEATS).contains(&pre_roll) {
                return Err(format!("preRollBeats: from 0 to {MAX_PRE_ROLL_BEATS}"));
            }
            params.insert("preRollBeats".into(), pre_roll.into());
            match arguments.get("sampleRate") {
                None | Some(Value::Null) => {}
                Some(rate) => match rate.as_u64().filter(|rate| SAMPLE_RATES.contains(rate)) {
                    Some(rate) => {
                        params.insert("sampleRate".into(), rate.into());
                    }
                    None => return Err("sampleRate is 44100, 48000, 88200 or 96000".into()),
                },
            }
            place(arguments, &mut params)
        })();
        if let Err(why) = checked {
            return bad(why);
        }
        self.start_job("render_start", "render", params, &pick).await
    }

    /// `hear`: what the DAW is playing, captured live as a job (REAPER).
    pub async fn hear(&self, arguments: &Value) -> (Value, bool) {
        let pick = match Pick::from_arguments(arguments) {
            Ok(pick) => pick,
            Err(why) => return bad(why),
        };
        let seconds = match number(arguments, "seconds") {
            Ok(Some(seconds)) if (1. ..=MAX_CAPTURE_SECONDS).contains(&seconds) => seconds,
            Ok(_) => return bad(format!("seconds: from 1 to {MAX_CAPTURE_SECONDS}")),
            Err(why) => return bad(why),
        };
        let mut params = Map::new();
        params.insert("seconds".into(), seconds.into());
        self.start_job("capture_start", "capture", params, &pick).await
    }

    /// Asks the bridge where the jobs nobody followed stand (every one that hasn't ended when `all`): those it says ended,
    /// or no longer has, are ended here too.
    async fn learn_ends(&self, pick: &Pick, all: bool) {
        for id in self.jobs().unasked(all) {
            match self.call("listen_status", json!({"jobId": id}), pick, STATUS_TIMEOUT).await {
                Ok(answer) if answer.get("state").and_then(Value::as_str).is_some_and(|state| TERMINAL.contains(&state)) => {
                    self.jobs().ended(&id)
                }
                Ok(_) => self.jobs().asked(&id),
                Err(crate::host::CallError::Bridge { code, .. }) if code == daw_bridge::wire::code::BAD_PARAMS => self.jobs().ended(&id),
                Err(_) => {}
            }
        }
    }

    /// Starts a job on the bridge under an id of the host's, with the free room it may fill, and keeps it.
    async fn start_job(&self, method: &str, kind: &'static str, mut params: Map<String, Value>, pick: &Pick) -> (Value, bool) {
        // (jobs nobody followed to their end: their ends are learnt before they count against this one or its room)
        self.learn_ends(pick, self.jobs().running().len() >= MAX_JOBS).await;
        let id = self.jobs().id('j');
        params.insert("jobId".into(), id.clone().into());
        if let Some(free) = self.tidy_folder().await {
            params.insert("freeBytes".into(), free.into());
        }
        let mut answer = match self.listening_call(method, Value::Object(params), pick, START_TIMEOUT).await {
            Ok(answer) => answer,
            Err(refused) => return refused,
        };
        if let Err(why) = self.jobs().add(&id, kind, files_of(&answer)) {
            // (the bridge has it running: it's let go of, so it doesn't render into a job nobody follows)
            let _ = self.call("listen_cancel", json!({"jobId": id}), pick, STATUS_TIMEOUT).await;
            return (json!({"error": why, "code": "busy", "busy": true, "connected": true, "daw": self.daw().id()}), true);
        }
        answer["jobId"] = id.into();
        answer["kind"] = kind.into();
        (answer, false)
    }

    /// `listen_job`: where a job stands, waiting up to `waitMs` for it to end.
    pub async fn listen_job(&self, arguments: &Value) -> (Value, bool) {
        let pick = match Pick::from_arguments(arguments) {
            Ok(pick) => pick,
            Err(why) => return bad(why),
        };
        let id = match required(arguments, "jobId") {
            Ok(id) => id.to_string(),
            Err(why) => return bad(why),
        };
        let wait = match arguments.get("waitMs") {
            None | Some(Value::Null) => 0,
            Some(value) => match value.as_u64().filter(|ms| *ms <= MAX_WAIT_MS) {
                Some(ms) => ms,
                None => return bad(format!("waitMs: a whole number from 0 to {MAX_WAIT_MS}")),
            },
        };
        let Some(job) = self.jobs().get(&id) else { return unknown_job(&id) };
        let began = Instant::now();
        loop {
            let answer = match self.call("listen_status", json!({"jobId": id}), &pick, STATUS_TIMEOUT).await {
                Ok(mut answer) => {
                    answer["jobId"] = id.clone().into();
                    answer["kind"] = job.kind.into();
                    answer
                }
                // The bridge doesn't know it any more (reloaded, or REAPER restarted): it has ended for good.
                Err(crate::host::CallError::Bridge { code, message }) if code == daw_bridge::wire::code::BAD_PARAMS => json!({
                    "jobId": id,
                    "kind": job.kind,
                    "state": "failed",
                    "progress": 0,
                    "error": format!("{} no longer has this job ({message}); its bridge may have been reloaded", self.daw()),
                    "code": "failed",
                }),
                Err(error) => return (self.call_error(&error), true),
            };
            let ended = answer.get("state").and_then(Value::as_str).is_some_and(|state| TERMINAL.contains(&state));
            if ended {
                self.jobs().ended(&id);
            } else {
                self.jobs().asked(&id);
            }
            if ended || began.elapsed() >= Duration::from_millis(wait) {
                return (answer, false);
            }
            tokio::time::sleep(POLL.min(Duration::from_millis(wait).saturating_sub(began.elapsed())).max(Duration::from_millis(1))).await;
        }
    }

    /// `listen_cancel`: the job stopped as soon as the bridge can, everything it changed put back.
    pub async fn listen_cancel(&self, arguments: &Value) -> (Value, bool) {
        let pick = match Pick::from_arguments(arguments) {
            Ok(pick) => pick,
            Err(why) => return bad(why),
        };
        let id = match required(arguments, "jobId") {
            Ok(id) => id.to_string(),
            Err(why) => return bad(why),
        };
        if self.jobs().get(&id).is_none() {
            return unknown_job(&id);
        }
        match self.listening_call("listen_cancel", json!({"jobId": id}), &pick, STATUS_TIMEOUT).await {
            Ok(mut answer) => {
                if answer.get("state").and_then(Value::as_str).is_some_and(|state| TERMINAL.contains(&state)) {
                    self.jobs().ended(&id);
                }
                answer["jobId"] = id.into();
                (answer, false)
            }
            Err(refused) => refused,
        }
    }

    /// `audition`: a file played out loud, once, where the producer listens.
    pub async fn audition(&self, arguments: &Value) -> (Value, bool) {
        let pick = match Pick::from_arguments(arguments) {
            Ok(pick) => pick,
            Err(why) => return bad(why),
        };
        let mut params = Map::new();
        let checked = (|| {
            let file = required(arguments, "file")?;
            if !Path::new(file).is_absolute() {
                return Err("file must be an absolute path".to_string());
            }
            if !Path::new(file).is_file() {
                return Err(format!("there's no file at {file}"));
            }
            params.insert("file".into(), file.into());
            let from = number(arguments, "fromSeconds")?.unwrap_or(0.);
            if from < 0. {
                return Err("fromSeconds: from 0".into());
            }
            params.insert("fromSeconds".into(), from.into());
            if let Some(seconds) = number(arguments, "seconds")? {
                if seconds <= 0. {
                    return Err("seconds: more than 0".into());
                }
                params.insert("seconds".into(), seconds.into());
            }
            let gain = number(arguments, "gainDb")?.unwrap_or(0.);
            if !(-60. ..=12.).contains(&gain) {
                return Err("gainDb: from -60 to 12".into());
            }
            params.insert("gainDb".into(), gain.into());
            Ok(())
        })();
        if let Err(why) = checked {
            return bad(why);
        }
        let id = self.jobs().id('p');
        params.insert("previewId".into(), id.clone().into());
        match self.listening_call("audition", Value::Object(params), &pick, FILE_TIMEOUT).await {
            Ok(mut answer) => {
                answer["previewId"] = id.into();
                (answer, false)
            }
            Err(refused) => refused,
        }
    }

    /// `audition_stop`: the preview playing stops (the one named, or whichever).
    pub async fn audition_stop(&self, arguments: &Value) -> (Value, bool) {
        let pick = match Pick::from_arguments(arguments) {
            Ok(pick) => pick,
            Err(why) => return bad(why),
        };
        let mut params = Map::new();
        match text(arguments, "previewId") {
            Ok(Some(id)) => {
                params.insert("previewId".into(), id.into());
            }
            Ok(None) => {}
            Err(why) => return bad(why),
        }
        match self.listening_call("audition_stop", Value::Object(params), &pick, STATUS_TIMEOUT).await {
            Ok(answer) => (answer, false),
            Err(refused) => refused,
        }
    }

    /// `audio_file`: the audio a ref stands for (an item's source file, a sample's audio, a pattern rendered).
    pub async fn audio_file(&self, arguments: &Value) -> (Value, bool) {
        let pick = match Pick::from_arguments(arguments) {
            Ok(pick) => pick,
            Err(why) => return bad(why),
        };
        let mut params = Map::new();
        let checked = (|| {
            params.insert("ref".into(), required(arguments, "ref")?.into());
            place(arguments, &mut params)
        })();
        if let Err(why) = checked {
            return bad(why);
        }
        // (the job's id, should the bridge have to render it: a Renoise pattern)
        let id = self.jobs().id('j');
        params.insert("jobId".into(), id.clone().into());
        if let Some(free) = self.tidy_folder().await {
            params.insert("freeBytes".into(), free.into());
        }
        match self.listening_call("audio_file", Value::Object(params), &pick, FILE_TIMEOUT).await {
            Ok(mut answer) => {
                if answer.get("jobId").is_some() {
                    if let Err(why) = self.jobs().add(&id, "render", files_of(&answer)) {
                        let _ = self.call("listen_cancel", json!({"jobId": id}), &pick, STATUS_TIMEOUT).await;
                        return (json!({"error": why, "code": "busy", "busy": true, "connected": true, "daw": self.daw().id()}), true);
                    }
                    answer["jobId"] = id.into();
                    answer["kind"] = "render".into();
                }
                (answer, false)
            }
            Err(refused) => refused,
        }
    }

    /// (#401) What a delivery is asked to be, each key passed on as given: the bridge checks them.
    fn delivery_params(arguments: &Value) -> Result<Map<String, Value>, String> {
        let mut params = Map::new();
        for key in [
            "source",
            "bounds",
            "trackRefs",
            "format",
            "bits",
            "sampleRate",
            "channels",
            "tailMs",
            "pattern",
            "folder",
            "overwrite",
            "expect",
        ] {
            if let Some(value) = arguments.get(key).filter(|value| !value.is_null()) {
                params.insert(key.into(), value.clone());
            }
        }
        if !params.contains_key("pattern") {
            return Err("pattern: the files' names, with the DAW's wildcards".into());
        }
        place(arguments, &mut params)?;
        Ok(params)
    }

    /// `deliver_targets` (#401): the files a delivery would write, every render setting put back before the answer.
    pub async fn deliver_targets(&self, arguments: &Value) -> (Value, bool) {
        let pick = match Pick::from_arguments(arguments) {
            Ok(pick) => pick,
            Err(why) => return bad(why),
        };
        let params = match Self::delivery_params(arguments) {
            Ok(params) => params,
            Err(why) => return bad(why),
        };
        match self.listening_call("deliver_targets", Value::Object(params), &pick, START_TIMEOUT).await {
            Ok(answer) => (answer, false),
            Err(refused) => refused,
        }
    }

    /// `deliver` (#401): a delivery rendered as a job, into the producer's folder (follow it with listen_job).
    pub async fn deliver(&self, arguments: &Value) -> (Value, bool) {
        let pick = match Pick::from_arguments(arguments) {
            Ok(pick) => pick,
            Err(why) => return bad(why),
        };
        let params = match Self::delivery_params(arguments) {
            Ok(params) => params,
            Err(why) => return bad(why),
        };
        self.start_job("deliver_start", "render", params, &pick).await
    }

    /// `scratch`: copies of a track made, set or dropped, for a search, a probe or a goal to hear.
    pub async fn scratch(&self, arguments: &Value) -> (Value, bool) {
        let pick = match Pick::from_arguments(arguments) {
            Ok(pick) => pick,
            Err(why) => return bad(why),
        };
        let mut params = Map::new();
        let checked = (|| {
            match text(arguments, "action")? {
                Some("make") => {
                    params.insert("action".into(), "make".into());
                    // (M7: players of an instrument's phrases, for Kumi to hear phrase takes, instead of copies of a track)
                    match arguments.get("phrases").filter(|phrases| !phrases.is_null()) {
                        Some(phrases) => {
                            let instrument = phrases
                                .get("instrument")
                                .and_then(Value::as_str)
                                .ok_or("phrases.instrument: the instrument's ref".to_string())?;
                            let list: Vec<u64> =
                                phrases.get("phrases").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_u64).collect();
                            if list.is_empty() || list.len() as u64 > MAX_COPIES {
                                return Err(format!("phrases.phrases: 1 to {MAX_COPIES} phrase numbers"));
                            }
                            let mut players = Map::new();
                            players.insert("instrument".into(), instrument.into());
                            players.insert("phrases".into(), list.into());
                            if let Some(note) = phrases.get("note").and_then(Value::as_u64) {
                                players.insert("note".into(), note.into());
                            }
                            params.insert("phrases".into(), Value::Object(players));
                        }
                        None => {
                            params.insert("ref".into(), required(arguments, "ref")?.into());
                        }
                    }
                    let count = arguments.get("count").and_then(Value::as_u64).filter(|n| (1..=MAX_COPIES).contains(n));
                    params.insert("count".into(), count.ok_or(format!("count: 1 to {MAX_COPIES}"))?.into());
                    let label = required(arguments, "label")?;
                    if label.chars().count() > 80 {
                        return Err("label: at most 80 characters".into());
                    }
                    params.insert("label".into(), label.into());
                    if arguments.get("range").is_some_and(|r| !r.is_null()) {
                        params.insert("range".into(), range(arguments, "range")?);
                    }
                    place(arguments, &mut params)?;
                }
                Some("set") => {
                    params.insert("action".into(), "set".into());
                    params.insert("scratchId".into(), required(arguments, "scratchId")?.into());
                    let values = arguments.get("values").and_then(Value::as_array).ok_or("values must be a list")?;
                    if values.is_empty() || values.len() > MAX_SCRATCH_VALUES {
                        return Err(format!("values: 1 to {MAX_SCRATCH_VALUES} of them"));
                    }
                    let mut out = vec![];
                    for value in values {
                        let mut row = Map::new();
                        row.insert("ref".into(), required(value, "ref")?.into());
                        for place in ["device", "param"] {
                            let at =
                                value.get(place).and_then(Value::as_u64).ok_or(format!("each value's {place}: a whole number from 0"))?;
                            row.insert(place.into(), at.into());
                        }
                        match (number(value, "normalized")?, number(value, "value")?) {
                            (Some(normalized), None) if (0. ..=1.).contains(&normalized) => {
                                row.insert("normalized".into(), normalized.into());
                            }
                            (None, Some(raw)) => {
                                row.insert("value".into(), raw.into());
                            }
                            _ => return Err("each value: normalized (0 to 1) or value, not both".into()),
                        }
                        out.push(Value::Object(row));
                    }
                    params.insert("values".into(), Value::Array(out));
                }
                Some("drop") => {
                    params.insert("action".into(), "drop".into());
                    params.insert("scratchId".into(), required(arguments, "scratchId")?.into());
                }
                _ => return Err("action is make, set or drop".into()),
            }
            Ok(())
        })();
        if let Err(why) = checked {
            return bad(why);
        }
        let making = params["action"] == "make";
        let id = making.then(|| self.jobs().id('s'));
        if let Some(id) = &id {
            params.insert("scratchId".into(), id.clone().into());
        }
        match self.listening_call("scratch", Value::Object(params), &pick, SCRATCH_TIMEOUT).await {
            Ok(mut answer) => {
                if let Some(id) = id {
                    answer["scratchId"] = id.into();
                }
                (answer, false)
            }
            Err(refused) => refused,
        }
    }

    /// The jobs still running when the host goes: the bridge stops them and puts back what they changed.
    pub(crate) async fn cancel_running_jobs(&self) {
        for id in self.jobs().running() {
            let answer = self.call("listen_cancel", json!({"jobId": id}), &Pick::default(), STATUS_TIMEOUT).await;
            if answer.is_ok_and(|answer| answer.get("state").and_then(Value::as_str).is_some_and(|state| TERMINAL.contains(&state))) {
                self.jobs().ended(&id);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_render_takes_only_sources_it_knows_each_checked() {
        let given = json!({"sources": [
            {"id": "mix", "what": "master", "extra": 1},
            {"id": "pad-1", "what": "track", "ref": "r:track:{A}", "stage": "post-fader"},
            {"id": "pre", "what": "master", "beforeChain": true},
        ]});
        assert_eq!(
            sources(&given).unwrap(),
            vec![
                json!({"id": "mix", "what": "master"}),
                json!({"id": "pad-1", "what": "track", "ref": "r:track:{A}", "stage": "post-fader"}),
                json!({"id": "pre", "what": "master", "beforeChain": true}),
            ]
        );
        for bad in [
            json!({}),
            json!({"sources": []}),
            json!({"sources": [{"id": "a", "what": "bus"}]}),
            json!({"sources": [{"id": "a b", "what": "master"}]}),
            json!({"sources": [{"id": "a", "what": "master"}, {"id": "a", "what": "master"}]}),
            json!({"sources": [{"id": "a", "what": "track"}]}),
            json!({"sources": [{"id": "a", "what": "track", "ref": "r:track:{A}", "stage": "solo"}]}),
            json!({"sources": [{"id": "a", "what": "master", "ref": "r:track:{A}"}]}),
            json!({"sources": [{"id": "a", "what": "master", "beforeChain": "yes"}]}),
            json!({"sources": [{"id": "a", "what": "track", "ref": "r:track:{A}", "stage": "items"}, {"id": "b", "what": "master"}]}),
            json!({"sources": (0..=MAX_SOURCES).map(|n| json!({"id": format!("s{n}"), "what": "master"})).collect::<Vec<_>>()}),
        ] {
            assert!(sources(&bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn a_range_is_beats_from_the_start_and_more_than_none() {
        assert_eq!(range(&json!({"range": {"fromBeat": 16, "beats": 8}}), "range").unwrap(), json!({"fromBeat": 16.0, "beats": 8.0}));
        for bad in [
            json!({}),
            json!({"range": {"fromBeat": -1, "beats": 8}}),
            json!({"range": {"fromBeat": 0, "beats": 0}}),
            json!({"range": [0, 8]}),
        ] {
            assert!(range(&bad, "range").is_err(), "{bad}");
        }
    }

    #[test]
    fn pruning_takes_old_files_then_the_oldest_past_the_bound_never_a_running_jobs_or_a_folder() {
        let dir = tempfile::tempdir().unwrap();
        let file = |name: &str, bytes: usize, age_s: u64| {
            let path = dir.path().join(name);
            std::fs::write(&path, vec![0u8; bytes]).unwrap();
            let when = SystemTime::now() - Duration::from_secs(age_s);
            std::fs::File::options().write(true).open(&path).unwrap().set_modified(when).unwrap();
            path
        };
        let old = file("old.wav", 10, 3 * 3600);
        let older_running = file("running.wav", 10, 4 * 3600);
        let a = file("a.wav", 100, 30);
        let b = file("b.wav", 100, 20);
        let c = file("c.wav", 100, 10);
        let journal = file("scratch.json", 10, 30 * 24 * 3600);
        std::fs::create_dir(dir.path().join("kept")).unwrap();
        let removed = prune(dir.path(), std::slice::from_ref(&older_running), Duration::from_secs(2 * 3600), 250);
        assert_eq!(removed, 2, "the old one, then the oldest while over 250 bytes");
        assert!(!old.exists() && !a.exists());
        assert!(older_running.exists() && b.exists() && c.exists() && dir.path().join("kept").is_dir());
        assert!(journal.exists(), "the bridges' scratch journal stays, however old");
    }

    #[test]
    fn jobs_are_kept_until_their_end_was_read_and_never_more_than_the_bound_running() {
        let jobs = Jobs::default();
        let first = jobs.id('j');
        assert!(first.starts_with('j') && first != jobs.id('j'));
        for n in 0..MAX_JOBS {
            jobs.add(&format!("j{n}"), "render", vec![]).unwrap();
        }
        assert!(jobs.add("one-more", "render", vec![]).is_err(), "all running");
        jobs.ended("j3");
        jobs.add("one-more", "render", vec![]).unwrap();
        assert!(jobs.get("j3").is_none(), "an ended job makes room");
        assert_eq!(jobs.running().len(), MAX_JOBS);
    }
}
