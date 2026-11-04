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
use daw_bridge::owner;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// The file in the bridge's folder that names the copy of SoundCraft Kumi started last.
pub const LAUNCHED: &str = "launched.json";
/// The bridge folder's lock, held from a look for the copy through a start's record of it.
pub const LOCK: &str = "launch.lock";
/// The variable that names SoundCraft's program (its `.app` on macOS), in place of the usual places. It names the program
/// itself, not a wrapper that starts it: Kumi finds its copy again by the program the copy's process runs.
pub const APP_VARIABLE: &str = "KUMI_SOUNDCRAFT_APP";
/// What the app prints once its channel listens.
const PORT_LINE: &str = "SOUNDCRAFT_CONTROL_PORT=";
/// How many starts' output folders are kept, to say why one failed.
const RUNS_KEPT: usize = 5;
/// The biggest `launched.json` read.
const MAX_LAUNCHED: u64 = 64 * 1024;
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

/// A SoundCraft installed here: its program (the `.app` on macOS), and its version when it says one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct App {
    pub path: PathBuf,
    pub version: Option<String>,
}

/// Where SoundCraft is installed: the program `KUMI_SOUNDCRAFT_APP` names, else the system's usual places.
pub fn find(var: &dyn Fn(&str) -> Option<String>) -> Option<App> {
    let path = match var(APP_VARIABLE).filter(|named| !named.is_empty()) {
        Some(named) => Some(PathBuf::from(named)).filter(|path| path.exists())?,
        None => installed(var)?,
    };
    Some(App { version: version_of(&path), path })
}

/// macOS: /Applications, ~/Applications, else wherever Spotlight knows the app's bundle id to be.
#[cfg(target_os = "macos")]
fn installed(var: &dyn Fn(&str) -> Option<String>) -> Option<PathBuf> {
    let home = var("HOME").map(|home| PathBuf::from(home).join("Applications/SoundCraft.app"));
    if let Some(usual) = [Some(PathBuf::from("/Applications/SoundCraft.app")), home].into_iter().flatten().find(|path| path.exists()) {
        return Some(usual);
    }
    let found = Command::new("mdfind").arg("kMDItemCFBundleIdentifier == 'ai.storyteller.soundcraft'").output().ok()?;
    String::from_utf8_lossy(&found.stdout)
        .lines()
        .map(PathBuf::from)
        .find(|path| path.extension().is_some_and(|ext| ext == "app") && path.exists())
}

/// Windows: the folder SoundCraft's installer recorded (for one user, or for everyone), else its two default folders.
#[cfg(windows)]
fn installed(var: &dyn Fn(&str) -> Option<String>) -> Option<PathBuf> {
    let recorded = [registry::CURRENT_USER, registry::LOCAL_MACHINE]
        .into_iter()
        .find_map(|root| registry::text(root, r"Software\SoundCraft", "InstallDir"))
        .map(|folder| PathBuf::from(folder).join("soundcraft.exe"));
    let defaults = [
        var("LOCALAPPDATA").map(|data| PathBuf::from(data).join(r"Programs\ArtCraft\SoundCraft\soundcraft.exe")),
        var("ProgramFiles").map(|files| PathBuf::from(files).join(r"ArtCraft\SoundCraft\soundcraft.exe")),
    ];
    recorded.into_iter().chain(defaults.into_iter().flatten()).find(|path| path.exists())
}

/// Text values in Windows' registry, read in UTF-16: `reg.exe` writes in the console's code page, which loses what it
/// can't show, such as an install folder's name in another script.
#[cfg(windows)]
mod registry {
    use std::ffi::OsString;
    use std::os::windows::ffi::OsStringExt;

    pub const CURRENT_USER: isize = 0x8000_0001_u32 as i32 as isize;
    pub const LOCAL_MACHINE: isize = 0x8000_0002_u32 as i32 as isize;

    #[link(name = "advapi32")]
    unsafe extern "system" {
        fn RegGetValueW(
            key: isize,
            subkey: *const u16,
            value: *const u16,
            flags: u32,
            kind: *mut u32,
            data: *mut std::ffi::c_void,
            size: *mut u32,
        ) -> i32;
    }

    /// The value `name` of `key` under `root`, when it's text (REG_SZ) and not empty.
    pub fn text(root: isize, key: &str, name: &str) -> Option<OsString> {
        const RRF_RT_REG_SZ: u32 = 0x2;
        let wide = |text: &str| text.encode_utf16().chain(std::iter::once(0)).collect::<Vec<u16>>();
        let (key, name) = (wide(key), wide(name));
        let mut size = 0u32;
        // SAFETY: both names end in NUL; without a buffer, the call only says the size the value needs, in bytes.
        let status = unsafe {
            RegGetValueW(root, key.as_ptr(), name.as_ptr(), RRF_RT_REG_SZ, std::ptr::null_mut(), std::ptr::null_mut(), &mut size)
        };
        if status != 0 {
            return None;
        }
        let mut buffer = vec![0u16; size as usize / 2 + 1];
        size = (buffer.len() * 2) as u32;
        // SAFETY: the buffer has the `size` bytes of room it's said to have.
        let status = unsafe {
            RegGetValueW(root, key.as_ptr(), name.as_ptr(), RRF_RT_REG_SZ, std::ptr::null_mut(), buffer.as_mut_ptr().cast(), &mut size)
        };
        if status != 0 {
            return None;
        }
        let end = buffer.iter().position(|&unit| unit == 0).unwrap_or(buffer.len());
        Some(OsString::from_wide(&buffer[..end])).filter(|text| !text.is_empty())
    }
}

/// Linux and the rest: `soundcraft` on PATH.
#[cfg(all(not(target_os = "macos"), not(windows)))]
fn installed(var: &dyn Fn(&str) -> Option<String>) -> Option<PathBuf> {
    std::env::split_paths(&var("PATH")?).map(|folder| folder.join("soundcraft")).find(|path| path.is_file())
}

/// The app's version: its bundle's on macOS, else what its `--version` prints (`soundcraft 0.17.0 (…)`).
fn version_of(path: &Path) -> Option<String> {
    let version = if path.extension().is_some_and(|ext| ext == "app") {
        let said = Command::new("plutil")
            .args(["-extract", "CFBundleShortVersionString", "raw"])
            .arg(path.join("Contents/Info.plist"))
            .output()
            .ok()?;
        String::from_utf8_lossy(&said.stdout).trim().to_string()
    } else {
        let said = Command::new(path).arg("--version").stdin(Stdio::null()).stderr(Stdio::null()).output().ok()?;
        String::from_utf8_lossy(&said.stdout).split_whitespace().nth(1)?.to_string()
    };
    Some(version).filter(|version| !version.is_empty())
}

/// SoundCraft's settings folder, where it keeps its autosaves, as the app at `app` finds it (config.rs there):
/// SOUNDCRAFT_CONFIG_DIR when it's set (set empty: no folder at all, and no autosave), else `SoundCraftData` beside the
/// program of a portable copy (one with `portable.txt` beside it), else the system's place for it. (A portable copy that
/// can't write beside itself uses the system's place; that's left out.)
pub fn settings_folder(var: &dyn Fn(&str) -> Option<String>, app: Option<&Path>) -> Option<PathBuf> {
    if let Some(named) = var("SOUNDCRAFT_CONFIG_DIR") {
        let named = PathBuf::from(named);
        return (!named.as_os_str().is_empty()).then(|| std::path::absolute(&named).unwrap_or(named));
    }
    if let Some(beside) = app.map(program_folder).filter(|folder| folder.join("portable.txt").is_file()) {
        return Some(beside.join("SoundCraftData"));
    }
    if cfg!(target_os = "macos") {
        var("HOME").map(|home| PathBuf::from(home).join("Library/Application Support/SoundCraft"))
    } else if cfg!(windows) {
        var("APPDATA").map(|data| PathBuf::from(data).join("SoundCraft"))
    } else {
        let config = var("XDG_CONFIG_HOME").filter(|config| !config.is_empty()).map(PathBuf::from);
        config.or_else(|| var("HOME").map(|home| PathBuf::from(home).join(".config"))).map(|config| config.join("soundcraft"))
    }
}

/// The folder SoundCraft's program is in: inside the bundle on macOS.
fn program_folder(app: &Path) -> PathBuf {
    if app.extension().is_some_and(|ext| ext == "app") {
        app.join("Contents/MacOS")
    } else {
        app.parent().map(Path::to_path_buf).unwrap_or_default()
    }
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

/// A copy of SoundCraft Kumi started. (Its Debug leaves the token out.)
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Launched {
    /// The app's process.
    pub pid: u32,
    pub started_ms: u64,
    /// Its control channel's port, and the token its requests carry.
    pub port: u16,
    pub token: String,
    pub app: PathBuf,
    pub version: Option<String>,
}

impl Launched {
    /// A client of its control channel.
    pub fn control(&self) -> Control {
        Control::new(self.port, Some(self.token.clone()))
    }
}

impl std::fmt::Debug for Launched {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Launched")
            .field("pid", &self.pid)
            .field("started_ms", &self.started_ms)
            .field("port", &self.port)
            .field("app", &self.app)
            .field("version", &self.version)
            .finish_non_exhaustive()
    }
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
pub fn lock(folder: &Path) -> Result<std::fs::File, String> {
    let failed = |error: std::io::Error| format!("Kumi's SoundCraft folder {} can't be locked: {error}", folder.display());
    owner::create_owner_only_dir(folder).map_err(failed)?;
    let mut options = std::fs::OpenOptions::new();
    options.read(true).write(true).create(true).truncate(false);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    // (opened in place, never replaced: a lock is on the file itself)
    let file = options.open(folder.join(LOCK)).map_err(failed)?;
    file.lock().map_err(failed)?;
    Ok(file)
}

/// What `launched.json` says, when it's a plain file of this user's alone in a folder of this user's alone (on unix; on
/// Windows that check is a PowerShell run, and the folder Kumi made owner-only stands for it, as for the bridge's secret):
/// another account that can write the folder can't plant a port and a token.
pub fn recorded(folder: &Path) -> Option<Launched> {
    let path = folder.join(LAUNCHED);
    let meta = std::fs::symlink_metadata(&path).ok()?;
    let owned = !cfg!(unix) || (owner::is_owner_only(folder) && owner::is_owner_only(&path));
    if !meta.is_file() || meta.len() > MAX_LAUNCHED || !owned {
        return None;
    }
    serde_json::from_slice(&std::fs::read(&path).ok()?).ok()
}

/// The copy an earlier start left, if it's still there: its process runs SoundCraft's program, listens on its port, and
/// its channel takes its token within `answer` (or is busy).
pub async fn attach(folder: &Path, answer: Duration) -> Option<Found> {
    let launched = recorded(folder)?;
    let there = daw_bridge::files::process_alive(launched.pid)
        && runs_soundcraft(launched.pid, &launched.app)
        && listens(launched.port, launched.pid);
    if !there {
        return None;
    }
    let answering = probe(&launched, answer).await.ok()?;
    Some(Found { launched, answering })
}

/// Whether `pid`'s program is SoundCraft's (or `app`'s own, under whatever name), where the system says (Linux's /proc,
/// Windows' image name): a pid another program got since isn't the copy. macOS doesn't say, and there the listener check
/// names the pid.
fn runs_soundcraft(pid: u32, app: &Path) -> bool {
    daw_bridge::detect::program_of(pid).is_none_or(|program| program_is(&program.to_string_lossy(), app))
}

/// Whether a process's program (as the system names it) is SoundCraft's, or the program at `app` (one
/// `KUMI_SOUNDCRAFT_APP` names under another name). Linux names a program whose file was replaced since it started (a
/// package upgrade) `<path> (deleted)`: still that program.
pub fn program_is(program: &str, app: &Path) -> bool {
    let program = program.strip_suffix(" (deleted)").unwrap_or(program);
    let name = program.rsplit(['/', '\\']).next().unwrap_or(program);
    let app_name = app.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
    is_soundcraft(program) || (!app_name.is_empty() && name.eq_ignore_ascii_case(&app_name))
}

/// Whether `pid` listens on `port`: on macOS by the socket's owner (`lsof`). Elsewhere only whether something accepts a
/// connection there; whose it is, the program check and the token's probe say.
fn listens(port: u16, pid: u32) -> bool {
    if cfg!(target_os = "macos") {
        return listener(port) == Some(pid);
    }
    let address = std::net::SocketAddr::from(([127, 0, 0, 1], port));
    std::net::TcpStream::connect_timeout(&address, Duration::from_millis(500)).is_ok()
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
    match launched.control().call("ui.inspect", json!({}), answer).await {
        Ok(_) | Err(Error::Refused(_) | Error::Broken(_) | Error::Protocol(_)) => Ok(true),
        Err(Error::Deadline | Error::Stalled | Error::Busy | Error::Crowded) => Ok(false),
        Err(error) => Err(error),
    }
}

/// What a start is given: the app, the session to open (an absolute path), the token, and the files its stdout and
/// stderr go to.
pub struct Start<'a> {
    pub app: &'a App,
    pub session: Option<&'a Path>,
    pub token: &'a str,
    pub out: &'a Path,
    pub err: &'a Path,
    /// Started behind the window in front (macOS's `open -g`).
    pub background: bool,
}

/// Starts SoundCraft with its channel on, opening `session` if one is named, and waits for its port. The copy is kept in
/// `launched.json` in `folder`, the bridge's, before Kumi's first request to it, which says whether it answers yet.
pub async fn launch(app: &App, session: Option<&Path>, folder: &Path, background: bool) -> Result<Found, String> {
    launch_with(app, session, folder, background, Waits::USUAL, start_app).await
}

/// [`launch`], with the app started by `start` (a test's stand-in). The start gives back the app's process when it has it
/// (a copy Kumi can't use is closed through it, never by a pid another program may have by then), or None (macOS's
/// `open`): then the app is the process holding its stdout's file, and its port's listener once there is one.
pub async fn launch_with(
    app: &App,
    session: Option<&Path>,
    folder: &Path,
    background: bool,
    waits: Waits,
    start: impl FnOnce(&Start) -> Result<Option<Child>, String>,
) -> Result<Found, String> {
    let token = daw_bridge::auth::Secret::generate();
    let started_ms = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |since| since.as_millis() as u64);
    let made = |error: std::io::Error| format!("Kumi's SoundCraft folder {} can't be written: {error}", folder.display());
    owner::create_owner_only_dir(folder).map_err(made)?;
    let runs = folder.join("runs");
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
    if let Err(error) = owner::write_owner_only(&folder.join(LAUNCHED), &written) {
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
            let _ = std::fs::remove_file(folder.join(LAUNCHED));
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

/// The program of a process, as macOS's `ps` names it (a path).
fn program_of_pid(pid: u32) -> Option<String> {
    let said = Command::new("/bin/ps").args(["-o", "comm=", "-p", &pid.to_string()]).stderr(Stdio::null()).output().ok()?;
    Some(String::from_utf8_lossy(&said.stdout).trim().to_string()).filter(|program| !program.is_empty())
}

/// The app's process when the start can't say it (macOS's `open`): the one holding its stdout's file whose program is
/// SoundCraft's (or `app`'s own), its first if several (a helper it starts later holds the file too, with a later pid).
/// None once none holds it.
async fn holder(out: &Path, app: &Path) -> Option<u32> {
    let deadline = Instant::now() + HOLDER_WAIT;
    loop {
        let held = holders(out).into_iter().filter(|pid| program_of_pid(*pid).is_some_and(|program| program_is(&program, app))).min();
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

/// Whether a process's program (a path, or an image name) is SoundCraft's app: `SoundCraft` in macOS's bundle,
/// `soundcraft.exe`, `soundcraft`. Its command-line tool, `soundcraft-cli`, isn't.
pub fn is_soundcraft(program: &str) -> bool {
    let base = program.rsplit(['/', '\\']).next().unwrap_or(program);
    let base = base.strip_suffix(".exe").or_else(|| base.strip_suffix(".EXE")).unwrap_or(base);
    base.eq_ignore_ascii_case("soundcraft")
}

/// The pids of this user's copies of SoundCraft running now, by the process listing (KUMI_PROCESSES stands in for it):
/// SoundCraft's program, or `app`'s own under another name. Another account's copy (fast user switching, another remote
/// session) isn't this user's to quit, so it's left out.
pub fn running(var: &dyn Fn(&str) -> Option<String>, app: Option<&Path>) -> Vec<u32> {
    daw_bridge::detect::processes(var)
        .iter()
        .filter(|(pid, program)| {
            app.map_or(is_soundcraft(program), |app| program_is(program, app)) && daw_bridge::files::process_alive(*pid)
        })
        .map(|(pid, _)| *pid)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn soundcrafts_app_is_told_from_its_command_line_tool() {
        for program in
            ["/Applications/SoundCraft.app/Contents/MacOS/SoundCraft", "SoundCraft", r"C:\ArtCraft\SoundCraft\soundcraft.exe", "soundcraft"]
        {
            assert!(is_soundcraft(program), "{program}");
        }
        for program in ["soundcraft-cli", "/usr/bin/soundcraft-cli", "SoundCraftHelper", "Renoise"] {
            assert!(!is_soundcraft(program), "{program}");
        }
    }

    #[test]
    fn kumis_copy_is_known_by_its_program_after_an_upgrade_or_under_another_name() {
        let app = Path::new("/usr/bin/soundcraft");
        assert!(program_is("/usr/bin/soundcraft (deleted)", app), "Linux's name for a program replaced since it started");
        let renamed = Path::new("/opt/studio/SoundCraft-0.17.0");
        assert!(program_is("/opt/studio/SoundCraft-0.17.0", renamed), "the program KUMI_SOUNDCRAFT_APP names");
        assert!(program_is(r"C:\ArtCraft\SoundCraft\SOUNDCRAFT.EXE", Path::new(r"C:\ArtCraft\SoundCraft\soundcraft.exe")));
        assert!(!program_is("/usr/bin/bwrap", renamed));
        assert!(!program_is("/usr/local/bin/soundcraft-cli", app));
    }
}
