//! Starting SoundCraft with its control channel on, and finding that copy again.
//!
//! SoundCraft opens its channel only when it's started with `--control` (or with SOUNDCRAFT_CONTROL_PORT set). A copy
//! opened from the Dock or the Start menu has none, and its settings have no switch for it. So Kumi starts SoundCraft
//! itself, on port 0: the app takes a free port and prints it on its stdout, which goes to a file of Kumi's, and only then
//! opens the session it was given. The app's environment carries a token of Kumi's (SOUNDCRAFT_CONTROL_TOKEN), never its
//! command line: SoundCraft 0.17.0 ignores it, and #1319 upstream makes it require it.
//!
//! What a start left is kept, owner-only, in `launched.json` in the bridge's folder as soon as the app's process and port
//! are known, so a later session (Kumi restarted, or a second Kumi) finds the same copy: one whose process still runs as
//! SoundCraft, listening on that port and taking that token. A copy Kumi started and can't use (no port in time, a
//! channel that won't take its token) is closed, so it never stands in the next start's way. One look-then-start runs at a
//! time ([`lock`]).
use crate::control::{Control, Error};
use daw_bridge::daw::Daw;
use daw_bridge::detect::{self, SoundCraftApp};
use daw_bridge::files::Launched;
use daw_bridge::owner;
use daw_bridge::paths::BridgePaths;
use serde_json::json;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// What the app prints once its channel listens.
const PORT_LINE: &str = "SOUNDCRAFT_CONTROL_PORT=";
/// How many starts' output folders are kept, to say why one failed.
const RUNS_KEPT: usize = 5;
/// How long after `open` returns the app's process gets to show up, holding its stdout's file.
const HOLDER_WAIT: Duration = Duration::from_secs(2);

/// How long a start waits for the app's port, and a look for the copy waits for its channel's answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Waits {
    pub port: Duration,
    pub answer: Duration,
}

impl Waits {
    /// The port comes within a second of a cold start in the lab. The answer comes from the app's window, which takes no
    /// request while it opens a session: a big one takes longer than this, and the copy is busy meanwhile, not gone.
    pub const USUAL: Waits = Waits { port: Duration::from_secs(30), answer: Duration::from_secs(5) };
}

/// Where SoundCraft is installed, as Kumi's detection finds it: the program [`detect::SOUNDCRAFT_APP`] names (the
/// program itself, not a wrapper that starts it: Kumi finds its copy again by the program the copy's process runs), else
/// the system's usual places. Its version is its bundle's on macOS; elsewhere what its `--version` prints
/// (`soundcraft 0.17.0 (…)`), which detection, launching nothing, leaves out.
pub fn find(var: &dyn Fn(&str) -> Option<String>) -> Option<SoundCraftApp> {
    let mut app = detect::detect_soundcraft(&detect::Env::current(var))?;
    if app.version.is_none() && !cfg!(target_os = "macos") {
        app.version = version_said(&app.path);
    }
    Some(app)
}

/// The version a program's `--version` prints, its second word.
fn version_said(program: &Path) -> Option<String> {
    let said = Command::new(program).arg("--version").stdin(Stdio::null()).stderr(Stdio::null()).output().ok()?;
    Some(String::from_utf8_lossy(&said.stdout).split_whitespace().nth(1)?.to_string()).filter(|version| !version.is_empty())
}

/// The sessions SoundCraft will offer to recover when it next starts, by its own rule (autosave.rs's `leftovers`): an
/// autosave folder (`<session> <stamp>-<pid>`) whose copy of the app has gone, holding a session saved after the session
/// file it came from (named in its `origin`; or with no such file), or a recording that never stopped. The app deletes
/// the rest. Only names and times are read: the app's own check holds each folder's lock file, which this never opens,
/// so it can't make the app pass a session by.
pub fn recovery_waiting(settings: &Path) -> Vec<String> {
    let Ok(folders) = std::fs::read_dir(settings.join("Autosave")) else { return vec![] };
    let modified = |path: &Path| std::fs::metadata(path).and_then(|meta| meta.modified()).ok();
    let mut sessions: Vec<String> = folders
        .flatten()
        .filter_map(|folder| {
            let name = folder.file_name().to_string_lossy().into_owned();
            let (session, stamp) = name.rsplit_once(' ')?;
            let pid: u32 = stamp.split('-').nth(1)?.parse().ok()?;
            let dir = folder.path();
            if daw_bridge::files::process_alive(pid) || !dir.join("lock").is_file() {
                return None;
            }
            let saved = modified(&dir.join("session.scraft"))?;
            let origin = std::fs::read_to_string(dir.join("origin")).ok().filter(|origin| !origin.is_empty());
            let origin_saved = origin.and_then(|origin| modified(Path::new(&origin)));
            let offered = dir.join("recording.json").is_file() || origin_saved.is_none_or(|origin_saved| saved > origin_saved);
            offered.then(|| session.to_string())
        })
        .collect();
    sessions.sort();
    sessions.dedup();
    sessions
}

/// A client of the control channel of the copy Kumi started.
pub fn control(launched: &Launched) -> Control {
    Control::new(launched.port, Some(launched.token.clone()))
}

/// The copy Kumi started, as a look finds it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Found {
    pub launched: Launched,
    /// Whether its channel answered within the look's wait. One that didn't is busy (a session opening, a dialog in its
    /// window): Kumi's copy all the same.
    pub answering: bool,
}

/// Waits for any other look-then-start in the bridge's folder (this process's or another's) to finish, then holds the
/// folder's launch lock until the returned file is dropped: a second Kumi finds the first one's copy instead of starting
/// another beside it, or calling it one opened another way.
pub fn lock(paths: &BridgePaths) -> Result<std::fs::File, String> {
    let failed = |error: std::io::Error| format!("Kumi's SoundCraft folder {} can't be locked: {error}", paths.root.display());
    owner::create_owner_only_dir(&paths.root).map_err(failed)?;
    let file = daw_bridge::lifecycle::lock_file(&paths.launch_lock()).map_err(failed)?;
    file.lock().map_err(failed)?;
    Ok(file)
}

/// The copy an earlier start left, if it's still there: its process runs SoundCraft's program, listens on its port, and
/// its channel takes its token within `answer` (or is busy).
pub async fn attach(paths: &BridgePaths, answer: Duration) -> Option<Found> {
    let launched = Launched::read(&paths.launch_record())?;
    let there = daw_bridge::files::process_alive(launched.pid)
        && runs_soundcraft(launched.pid, &launched.app)
        && listens(launched.port, launched.pid);
    if !there {
        return None;
    }
    let answering = probe(&launched, answer).await.ok()?;
    Some(Found { launched, answering })
}

/// Whether `pid`'s program is SoundCraft's (or `app`'s own, under whatever name), where the system says: a pid another
/// program got since isn't the copy. Where it won't say, the listener check names the pid.
fn runs_soundcraft(pid: u32, app: &Path) -> bool {
    detect::program_of(pid).is_none_or(|program| detect::process_is_app(Daw::SoundCraft, &program.to_string_lossy(), Some(app)))
}

/// Whether `pid` listens on `port`: on macOS by the socket's owner (`lsof`). Elsewhere only whether something accepts a
/// connection there; whose it is, the program check and the token's probe say.
fn listens(port: u16, pid: u32) -> bool {
    if cfg!(target_os = "macos") {
        return listener(port) == Some(pid);
    }
    daw_bridge::probe::listening(port, Duration::from_millis(500))
}

/// macOS's own `lsof`, never another one on PATH.
const LSOF: &str = "/usr/sbin/lsof";

/// The process listening on a loopback port, by `lsof`.
fn listener(port: u16) -> Option<u32> {
    let said = Command::new(LSOF).args(["-nP", &format!("-iTCP:{port}"), "-sTCP:LISTEN", "-Fp"]).output().ok()?;
    String::from_utf8_lossy(&said.stdout).lines().find_map(|line| line.strip_prefix('p')?.parse().ok())
}

/// What a request carrying the copy's token says of its channel: Ok(true) when the app answers (its words, a refusal, a
/// method it hasn't got), Ok(false) while it's busy (its window took no request in time, or its queue or its connections
/// are full), an error when it isn't the copy's channel (nothing listens, the token is refused, or what answers isn't
/// SoundCraft's).
async fn probe(launched: &Launched, answer: Duration) -> Result<bool, Error> {
    match control(launched).call("ui.inspect", json!({}), answer).await {
        Ok(_) | Err(Error::Refused(_) | Error::Broken(_) | Error::Protocol(_)) => Ok(true),
        Err(Error::Deadline | Error::Stalled | Error::Busy | Error::Crowded) => Ok(false),
        Err(error) => Err(error),
    }
}

/// What a start is given: the app, the session to open (an absolute path), the token, and the files its stdout and
/// stderr go to.
pub struct Start<'a> {
    pub app: &'a SoundCraftApp,
    pub session: Option<&'a Path>,
    pub token: &'a str,
    pub out: &'a Path,
    pub err: &'a Path,
    /// Started behind the window in front (macOS's `open -g`).
    pub background: bool,
}

/// Starts SoundCraft with its channel on, opening `session` if one is named, and waits for its port. The copy is kept in
/// the bridge's launch record before Kumi's first request to it, which says whether it answers yet.
pub async fn launch(app: &SoundCraftApp, session: Option<&Path>, paths: &BridgePaths, background: bool) -> Result<Found, String> {
    launch_with(app, session, paths, background, Waits::USUAL, start_app).await
}

/// [`launch`], with the app started by `start` (a test's stand-in). The start gives back the app's process when it has it
/// (a copy Kumi can't use is closed through it, never by a pid another program may have by then), or None (macOS's
/// `open`): then the app is the process holding its stdout's file, and its port's listener once there is one.
pub async fn launch_with(
    app: &SoundCraftApp,
    session: Option<&Path>,
    paths: &BridgePaths,
    background: bool,
    waits: Waits,
    start: impl FnOnce(&Start) -> Result<Option<Child>, String>,
) -> Result<Found, String> {
    let token = daw_bridge::auth::Secret::generate();
    let started_ms = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |since| since.as_millis() as u64);
    let made = |error: std::io::Error| format!("Kumi's SoundCraft folder {} can't be written: {error}", paths.root.display());
    owner::create_owner_only_dir(&paths.root).map_err(made)?;
    let runs = paths.runs();
    let run = new_run(&runs, started_ms).map_err(made)?;
    prune_runs(&runs);
    let (out, err) = (run.join("launch.out"), run.join("launch.err"));
    // Owner-only before the app writes to it: it gets the port, which is open to any local program until the app checks
    // tokens.
    owner::write_atomic(&out, b"").map_err(made)?;
    owner::write_atomic(&err, b"").map_err(made)?;
    // (the app opens a relative path against its own working folder, which is `/` when `open` starts it)
    let session = session.map(std::path::absolute).transpose().map_err(|error| format!("the session's path can't be read: {error}"))?;
    let mut copy = Copy {
        child: start(&Start { app, session: session.as_deref(), token: token.expose(), out: &out, err: &err, background })?,
        out: &out,
    };
    let process = match &copy.child {
        Some(child) => Process::Known(child.id()),
        None => match holder(&out, &app.path).await {
            Some(pid) => Process::Known(pid),
            // (no copy of SoundCraft holds it: the app ended at once; one runs, but which can't be told yet)
            None if running(&|_: &str| None, Some(&app.path)).is_empty() => Process::Ended,
            None => Process::Unknown,
        },
    };
    let printed = port_from(&out, &err, &mut || copy.ended(process), waits.port).await;
    let port = match printed {
        Ok(port) => port,
        Err((ended, said)) => {
            let how = if ended {
                copy.reap();
                "SoundCraft ended before it opened its control channel".to_string()
            } else if copy.stop(process.pid()) {
                // (running with no channel: of no use to Kumi, and in the next start's way)
                format!("SoundCraft didn't open its control channel within {} s, so Kumi closed the copy it started", waits.port.as_secs())
            } else {
                format!("SoundCraft didn't open its control channel within {} s", waits.port.as_secs())
            };
            return Err(if said.is_empty() { how } else { format!("{how}: {said}") });
        }
    };
    // (on macOS the port's listener is the app: it printed that port itself)
    let pid = match (&copy.child, process) {
        (Some(child), _) => Some(child.id()),
        (None, _) => listener(port),
    };
    let Some(pid) = pid else {
        copy.stop(process.pid());
        return Err(format!("SoundCraft opened its channel on port {port}, and no process listens there now"));
    };
    let launched =
        Launched { pid, started_ms, port, token: token.expose().to_string(), app: app.path.clone(), version: app.version.clone() };
    // Kept before the first request: the window takes none while it opens the session, which can outlast any wait here,
    // and a running copy no record names would look like one opened from the Dock.
    let written = serde_json::to_vec_pretty(&launched).expect("launched.json is JSON");
    if let Err(error) = owner::write_owner_only(&paths.launch_record(), &written) {
        copy.stop(Some(pid));
        return Err(format!("{}, so Kumi closed the copy it started", made(error)));
    }
    match probe(&launched, waits.answer).await {
        Ok(answering) => {
            copy.keep();
            Ok(Found { launched, answering })
        }
        Err(error) => {
            copy.stop(Some(pid));
            let _ = std::fs::remove_file(paths.launch_record());
            Err(format!("SoundCraft opened its channel on port {port}, but it didn't take Kumi's request ({error}), so Kumi closed the copy it started"))
        }
    }
}

/// What a start knows of the app's process.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Process {
    Known(u32),
    /// It ended (or never showed up).
    Ended,
    /// It runs, but which process it is can't be told before its port's listener can.
    Unknown,
}

impl Process {
    fn pid(self) -> Option<u32> {
        match self {
            Process::Known(pid) => Some(pid),
            Process::Ended | Process::Unknown => None,
        }
    }
}

/// The copy a start made: its process when the start gave it, and its stdout's file.
struct Copy<'a> {
    child: Option<Child>,
    out: &'a Path,
}

impl Copy<'_> {
    /// Ends a copy Kumi started and can't use, so it doesn't stand in the next start's way: through its process, else (no
    /// process given) by `pid` while that process still holds the app's stdout's file. It has only just started: at most it
    /// opened the session it was given, which it hasn't changed. Whether anything was ended.
    fn stop(&mut self, pid: Option<u32>) -> bool {
        if let Some(mut child) = self.child.take() {
            let ended = child.kill().is_ok();
            let _ = child.wait();
            return ended;
        }
        #[cfg(unix)]
        if let Some(pid) = pid.filter(|pid| holders(self.out).contains(pid)) {
            // SAFETY: sends a signal to a process; nothing of this one's is read or written.
            return unsafe { libc::kill(pid as libc::pid_t, libc::SIGTERM) } == 0;
        }
        let _ = pid;
        false
    }

    /// Whether the app's process has ended: asked of its process when the start gave it (which waits for one that ended, as
    /// a pid alone can't tell an ended child that nobody waited for), else of its pid.
    fn ended(&mut self, process: Process) -> bool {
        match (&mut self.child, process) {
            (Some(child), _) => !matches!(child.try_wait(), Ok(None)),
            (None, Process::Known(pid)) => !daw_bridge::files::process_alive(pid),
            (None, Process::Ended) => true,
            (None, Process::Unknown) => false,
        }
    }

    /// A copy that ended by itself: its process is waited for, so it leaves no zombie.
    fn reap(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.wait();
        }
    }

    /// The copy Kumi keeps: waited for on a thread of its own, so it leaves no zombie when it quits before the bridge does.
    fn keep(&mut self) {
        if let Some(mut child) = self.child.take() {
            std::thread::spawn(move || child.wait());
        }
    }
}

/// The pids of the processes holding a file, by macOS's `lsof`.
fn holders(file: &Path) -> Vec<u32> {
    let said = Command::new(LSOF).arg("-t").arg(file).stdin(Stdio::null()).stderr(Stdio::null()).output();
    said.map(|said| String::from_utf8_lossy(&said.stdout).lines().filter_map(|line| line.trim().parse().ok()).collect()).unwrap_or_default()
}

/// The app's process when the start can't say it (macOS's `open`): the one holding its stdout's file whose program is
/// SoundCraft's (or `app`'s own), its first if several (a helper it starts later holds the file too, with a later pid).
/// None once none holds it.
async fn holder(out: &Path, app: &Path) -> Option<u32> {
    let deadline = Instant::now() + HOLDER_WAIT;
    loop {
        let held = holders(out)
            .into_iter()
            .filter(|pid| {
                detect::program_of(*pid)
                    .is_some_and(|program| detect::process_is_app(Daw::SoundCraft, &program.to_string_lossy(), Some(app)))
            })
            .min();
        if held.is_some() || Instant::now() >= deadline {
            return held;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// The port the app printed, once it has. Else whether its process ended first (or never showed up), and the last lines
/// it wrote to its stderr.
async fn port_from(out: &Path, err: &Path, ended: &mut dyn FnMut() -> bool, wait: Duration) -> Result<u16, (bool, String)> {
    let deadline = Instant::now() + wait;
    loop {
        let printed = std::fs::read_to_string(out).unwrap_or_default();
        if let Some(port) = printed.lines().find_map(|line| line.trim().strip_prefix(PORT_LINE)?.parse().ok()) {
            return Ok(port);
        }
        let ended = ended();
        if ended || Instant::now() >= deadline {
            let said = std::fs::read_to_string(err).unwrap_or_default();
            let last: Vec<&str> = said.lines().map(str::trim).filter(|line| !line.is_empty()).collect();
            return Err((ended, last[last.len().saturating_sub(3)..].join(" / ")));
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// A new folder for one start's output, named by when it began (a millisecond on, when another start has that one). The
/// bridge's folder above it is owner-only already, so this one is a plain folder.
fn new_run(runs: &Path, started_ms: u64) -> std::io::Result<PathBuf> {
    std::fs::create_dir_all(runs)?;
    let mut ms = started_ms;
    loop {
        let run = runs.join(ms.to_string());
        match std::fs::create_dir(&run) {
            Ok(()) => return Ok(run),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => ms += 1,
            Err(error) => return Err(error),
        }
    }
}

/// The oldest starts' output folders go, past the last few.
fn prune_runs(runs: &Path) {
    let Ok(entries) = std::fs::read_dir(runs) else { return };
    let mut started: Vec<(u64, PathBuf)> =
        entries.flatten().filter_map(|entry| Some((entry.file_name().to_str()?.parse().ok()?, entry.path()))).collect();
    started.sort();
    for (_, old) in started.iter().rev().skip(RUNS_KEPT) {
        let _ = std::fs::remove_dir_all(old);
    }
}

/// SOUNDCRAFT_CONFIG_DIR as the app should get it: absolute, since the app resolves a relative one against its own
/// working folder (`/` when `open` starts it) and Kumi against the bridge's. None when it's unset or empty, which the app
/// inherits as it is.
fn config_dir() -> Option<PathBuf> {
    let named = std::env::var_os("SOUNDCRAFT_CONFIG_DIR").filter(|named| !named.is_empty())?;
    std::path::absolute(named).ok()
}

/// Starts SoundCraft as the system starts apps: on macOS through `open` (a new copy, `-n`, which LaunchServices starts on
/// its own, so it outlives Kumi), elsewhere as a process of its own group, detached from Kumi's. Either way the app
/// inherits this process's environment, where the token goes: never on a command line, which other programs can read.
fn start_app(start: &Start) -> Result<Option<Child>, String> {
    let app = start.app.path.display();
    if cfg!(target_os = "macos") {
        let mut open = Command::new("open");
        open.arg("-n");
        if start.background {
            open.arg("-g");
        }
        open.env("SOUNDCRAFT_CONTROL_TOKEN", start.token);
        if let Some(config) = config_dir() {
            open.env("SOUNDCRAFT_CONFIG_DIR", config);
        }
        open.arg("-a").arg(&start.app.path).arg("--stdout").arg(start.out).arg("--stderr").arg(start.err);
        open.args(["--args", "--control", "0"]);
        if let Some(session) = start.session {
            open.arg("--").arg(session);
        }
        let done = open.output().map_err(|error| format!("`open` couldn't start {app}: {error}"))?;
        if !done.status.success() {
            return Err(format!("`open` couldn't start {app}: {}", String::from_utf8_lossy(&done.stderr).trim()));
        }
        return Ok(None);
    }
    let file = |path: &Path| std::fs::OpenOptions::new().append(true).open(path).map_err(|error| format!("{}: {error}", path.display()));
    let mut command = Command::new(&start.app.path);
    command.args(["--control", "0"]);
    if let Some(session) = start.session {
        command.arg("--").arg(session);
    }
    command.env("SOUNDCRAFT_CONTROL_TOKEN", start.token);
    if let Some(config) = config_dir() {
        command.env("SOUNDCRAFT_CONFIG_DIR", config);
    }
    command.stdin(Stdio::null()).stdout(file(start.out)?).stderr(file(start.err)?);
    #[cfg(unix)]
    std::os::unix::process::CommandExt::process_group(&mut command, 0);
    #[cfg(windows)]
    let child = {
        use std::os::windows::process::CommandExt;
        // DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP, and out of Kumi's job when the job allows it, so the app outlives Kumi.
        command.creation_flags(0x0000_0008 | 0x0000_0200 | 0x0100_0000);
        command.spawn().or_else(|_| command.creation_flags(0x0000_0008 | 0x0000_0200).spawn())
    };
    #[cfg(not(windows))]
    let child = command.spawn();
    child.map(Some).map_err(|error| format!("{app} couldn't start: {error}"))
}

/// The pids of this user's copies of SoundCraft running now, by the process listing (KUMI_PROCESSES stands in for it):
/// SoundCraft's program, or `app`'s own under another name. Another account's copy (fast user switching, another remote
/// session) isn't this user's to quit, so it's left out.
pub fn running(var: &dyn Fn(&str) -> Option<String>, app: Option<&Path>) -> Vec<u32> {
    detect::processes(var)
        .iter()
        .filter(|(pid, program)| detect::process_is_app(Daw::SoundCraft, program, app) && daw_bridge::files::process_alive(*pid))
        .map(|(pid, _)| *pid)
        .collect()
}
