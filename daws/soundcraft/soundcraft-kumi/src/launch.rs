//! Starting SoundCraft with its control channel on, and finding that copy again.
//!
//! SoundCraft opens its channel only when it's started with `--control` (or with SOUNDCRAFT_CONTROL_PORT set). A copy
//! opened from the Dock or the Start menu has none, and its settings have no switch for it. So Kumi starts SoundCraft
//! itself, on port 0: the app takes a free port and prints it on its stdout, which goes to a file of Kumi's. The app's
//! environment carries a token of Kumi's (SOUNDCRAFT_CONTROL_TOKEN), which SoundCraft 0.17.0 ignores and #1319 upstream
//! makes it require. What the start left is kept, owner-only, in `launched.json` in the bridge's folder, so a later
//! session (Kumi restarted, or a second Kumi) finds the same copy: one whose process still runs, listening on that port.
use crate::control::Control;
use daw_bridge::owner;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// The file in the bridge's folder that names the copy of SoundCraft Kumi started last.
pub const LAUNCHED: &str = "launched.json";
/// The variable that names SoundCraft's program (its `.app` on macOS), in place of the usual places.
pub const APP_VARIABLE: &str = "KUMI_SOUNDCRAFT_APP";
/// How long a start gets for the app to print its port. A cold start took about a second in the lab.
pub const PORT_WAIT: Duration = Duration::from_secs(30);
/// What the app prints once its channel listens.
const PORT_LINE: &str = "SOUNDCRAFT_CONTROL_PORT=";
/// How many starts' output folders are kept, to say why one failed.
const RUNS_KEPT: usize = 5;
/// What reaches the app from Kumi's environment besides the token. On macOS `open` starts it with the system's
/// environment, not Kumi's, so a settings folder Kumi's session names is passed on by name.
const PASSED_ON: [&str; 1] = ["SOUNDCRAFT_CONFIG_DIR"];

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
    let recorded = ["HKCU", "HKLM"].into_iter().find_map(|root| {
        let said = Command::new("reg").args(["query", &format!(r"{root}\Software\SoundCraft"), "/v", "InstallDir"]).output().ok()?;
        let text = String::from_utf8_lossy(&said.stdout).into_owned();
        let folder = text.lines().find_map(|line| line.split_once("REG_SZ").map(|(_, folder)| folder.trim().to_string()))?;
        Some(PathBuf::from(folder).join("soundcraft.exe"))
    });
    let defaults = [
        var("LOCALAPPDATA").map(|data| PathBuf::from(data).join(r"Programs\ArtCraft\SoundCraft\soundcraft.exe")),
        var("ProgramFiles").map(|files| PathBuf::from(files).join(r"ArtCraft\SoundCraft\soundcraft.exe")),
    ];
    recorded.into_iter().chain(defaults.into_iter().flatten()).find(|path| path.exists())
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

/// SoundCraft's settings folder, where it keeps its autosaves: SOUNDCRAFT_CONFIG_DIR when it's set (set empty: no
/// folder at all, and no autosave), else the system's place for it.
pub fn settings_folder(var: &dyn Fn(&str) -> Option<String>) -> Option<PathBuf> {
    if let Some(named) = var("SOUNDCRAFT_CONFIG_DIR") {
        return Some(PathBuf::from(named)).filter(|named| !named.as_os_str().is_empty());
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

/// The sessions SoundCraft will offer to recover when it next starts: autosave folders (`<session> <stamp>-<pid>`) that
/// hold a session and whose copy of the app has gone. Only names are read. The app's own check at its start holds each
/// folder's lock file, which this never opens, so it can't make the app pass a session by.
pub fn recovery_waiting(settings: &Path) -> Vec<String> {
    let Ok(folders) = std::fs::read_dir(settings.join("Autosave")) else { return vec![] };
    let mut sessions: Vec<String> = folders
        .flatten()
        .filter_map(|folder| {
            let name = folder.file_name().to_string_lossy().into_owned();
            let (session, stamp) = name.rsplit_once(' ')?;
            let pid: u32 = stamp.split('-').nth(1)?.parse().ok()?;
            let gone = !daw_bridge::files::process_alive(pid) && folder.path().join("session.scraft").is_file();
            gone.then(|| session.to_string())
        })
        .collect();
    sessions.sort();
    sessions.dedup();
    sessions
}

/// A copy of SoundCraft Kumi started.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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

/// The copy an earlier start left (`launched.json` in the bridge's folder), if it still runs and listens on its port.
pub fn attach(folder: &Path) -> Option<Launched> {
    let launched: Launched = serde_json::from_slice(&std::fs::read(folder.join(LAUNCHED)).ok()?).ok()?;
    (daw_bridge::files::process_alive(launched.pid) && listens(launched.port, launched.pid)).then_some(launched)
}

/// Whether `pid` listens on `port`: on macOS by the socket's owner (`lsof`), elsewhere by a connection it accepts.
fn listens(port: u16, pid: u32) -> bool {
    if cfg!(target_os = "macos") {
        return listener(port) == Some(pid);
    }
    let address = std::net::SocketAddr::from(([127, 0, 0, 1], port));
    std::net::TcpStream::connect_timeout(&address, Duration::from_millis(500)).is_ok()
}

/// The process listening on a loopback port, by `lsof`: what `open` started can't be asked for its pid otherwise.
fn listener(port: u16) -> Option<u32> {
    let said = Command::new("lsof").args(["-nP", &format!("-iTCP:{port}"), "-sTCP:LISTEN", "-Fp"]).output().ok()?;
    String::from_utf8_lossy(&said.stdout).lines().find_map(|line| line.strip_prefix('p')?.parse().ok())
}

/// What a start is given: the app, the session to open, the token, and the files its stdout and stderr go to.
pub struct Start<'a> {
    pub app: &'a App,
    pub session: Option<&'a Path>,
    pub token: &'a str,
    pub out: &'a Path,
    pub err: &'a Path,
    /// Started behind the window in front (macOS's `open -g`).
    pub background: bool,
}

/// Starts SoundCraft with its channel on, opening `session` if one is named, and waits until its channel answers with
/// the start's token. What the start left is written to `launched.json` in `folder`, the bridge's.
pub async fn launch(app: &App, session: Option<&Path>, folder: &Path, background: bool) -> Result<Launched, String> {
    launch_with(app, session, folder, background, PORT_WAIT, start_app).await
}

/// [`launch`], with the app started by `start` (a test's stand-in), which answers its process when it knows it.
pub async fn launch_with(
    app: &App,
    session: Option<&Path>,
    folder: &Path,
    background: bool,
    wait: Duration,
    start: impl FnOnce(&Start) -> Result<Option<u32>, String>,
) -> Result<Launched, String> {
    let token = daw_bridge::auth::Secret::generate().expose().to_string();
    let started_ms = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |since| since.as_millis() as u64);
    let run = folder.join("runs").join(started_ms.to_string());
    let made = |error: std::io::Error| format!("Kumi's SoundCraft folder {} can't be written: {error}", folder.display());
    owner::create_owner_only_dir(folder).map_err(made)?;
    owner::create_owner_only_dir(&run).map_err(made)?;
    prune_runs(&folder.join("runs"));
    let (out, err) = (run.join("launch.out"), run.join("launch.err"));
    // Owner-only before the app writes to it: it gets the port, which is open to any local program until the app checks
    // tokens.
    owner::write_atomic(&out, b"").map_err(made)?;
    owner::write_atomic(&err, b"").map_err(made)?;
    let pid = start(&Start { app, session, token: &token, out: &out, err: &err, background })?;
    let port = port_from(&out, &err, pid, wait).await?;
    let pid = match pid {
        Some(pid) => pid,
        None => listener(port).ok_or_else(|| format!("SoundCraft opened its channel on port {port}, and no process listens there now"))?,
    };
    let launched = Launched { pid, started_ms, port, token, app: app.path.clone(), version: app.version.clone() };
    if let Err(error) = launched.control().call("ui.inspect", json!({}), Duration::from_secs(10)).await {
        return Err(format!("SoundCraft opened its channel on port {port}, but it didn't answer: {error}"));
    }
    let written = serde_json::to_vec_pretty(&launched).expect("launched.json is JSON");
    owner::write_owner_only(&folder.join(LAUNCHED), &written).map_err(made)?;
    Ok(launched)
}

/// The port the app printed, once it has. A process that ends first, or a wait that runs out, is told with the last
/// lines the app wrote to its stderr.
async fn port_from(out: &Path, err: &Path, pid: Option<u32>, wait: Duration) -> Result<u16, String> {
    let deadline = Instant::now() + wait;
    loop {
        let printed = std::fs::read_to_string(out).unwrap_or_default();
        if let Some(port) = printed.lines().find_map(|line| line.trim().strip_prefix(PORT_LINE)?.parse().ok()) {
            return Ok(port);
        }
        let ended = pid.is_some_and(|pid| !daw_bridge::files::process_alive(pid));
        if ended || Instant::now() >= deadline {
            let said = std::fs::read_to_string(err).unwrap_or_default();
            let last: Vec<&str> = said.lines().map(str::trim).filter(|line| !line.is_empty()).collect();
            let last = last[last.len().saturating_sub(3)..].join(" / ");
            let how = if ended {
                "SoundCraft ended before it opened its control channel".to_string()
            } else {
                format!("SoundCraft didn't open its control channel within {} s", wait.as_secs())
            };
            return Err(if last.is_empty() { how } else { format!("{how}: {last}") });
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
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

/// Starts SoundCraft as the system starts apps: on macOS through `open` (a new copy, `-n`, which LaunchServices starts on
/// its own, so it outlives Kumi), elsewhere as a process of its own group, detached from Kumi's.
fn start_app(start: &Start) -> Result<Option<u32>, String> {
    let app = start.app.path.display();
    if cfg!(target_os = "macos") {
        let mut open = Command::new("open");
        open.arg("-n");
        if start.background {
            open.arg("-g");
        }
        open.arg("-a").arg(&start.app.path).arg("--env").arg(format!("SOUNDCRAFT_CONTROL_TOKEN={}", start.token));
        for name in PASSED_ON {
            if let Ok(value) = std::env::var(name) {
                open.arg("--env").arg(format!("{name}={value}"));
            }
        }
        open.arg("--stdout").arg(start.out).arg("--stderr").arg(start.err).args(["--args", "--control", "0"]);
        open.args(start.session);
        let done = open.output().map_err(|error| format!("`open` couldn't start {app}: {error}"))?;
        if !done.status.success() {
            return Err(format!("`open` couldn't start {app}: {}", String::from_utf8_lossy(&done.stderr).trim()));
        }
        return Ok(None);
    }
    let file = |path: &Path| std::fs::OpenOptions::new().append(true).open(path).map_err(|error| format!("{}: {error}", path.display()));
    let mut command = Command::new(&start.app.path);
    command.args(["--control", "0"]).args(start.session).env("SOUNDCRAFT_CONTROL_TOKEN", start.token);
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
    let mut child = child.map_err(|error| format!("{app} couldn't start: {error}"))?;
    let pid = child.id();
    // Waited for on a thread of its own, so the app leaves no zombie when it quits before the bridge does.
    std::thread::spawn(move || child.wait());
    Ok(Some(pid))
}

/// Whether a process's program (a path, or an image name) is SoundCraft's app: `SoundCraft` in macOS's bundle,
/// `soundcraft.exe`, `soundcraft`. Its command-line tool, `soundcraft-cli`, isn't.
pub fn is_soundcraft(program: &str) -> bool {
    let base = program.rsplit(['/', '\\']).next().unwrap_or(program);
    let base = base.strip_suffix(".exe").or_else(|| base.strip_suffix(".EXE")).unwrap_or(base);
    base.eq_ignore_ascii_case("soundcraft")
}

/// The pids of the copies of SoundCraft running now, by the process listing (KUMI_PROCESSES stands in for it).
pub fn running(var: &dyn Fn(&str) -> Option<String>) -> Vec<u32> {
    daw_bridge::detect::processes(var).iter().filter(|(_, program)| is_soundcraft(program)).map(|(pid, _)| *pid).collect()
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
}
