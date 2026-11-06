//! A bridge the host starts and stops itself, for a DAW nothing of Kumi's can be loaded into: Kumi's own program beside
//! the DAW. The host starts it for a running DAW when no bridge answers, one at a time. The start runs in a task of its
//! own (the secret made off the async threads, then the program), and a call waits for a starting bridge no longer than
//! its own deadline: the bridge goes on starting while the calls go on. Its stdin is the host's pipe. Closing it is how
//! the host stops it, and a host that dies (killed, or crashed) closes it too, so the bridge ends by itself and removes its
//! discovery file either way. Which DAWs have such a bridge: [`bridge_program`] (SoundCraft and Logic Pro).
use daw_bridge::paths::BridgePaths;
use daw_bridge::Daw;
use std::ffi::OsString;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// The program a DAW's bridge is, when the host starts it; None for a DAW that loads its bridge itself
/// (REAPER's extension, Renoise's tool). Logic may use its installed owned folder; SoundCraft ships beside the host.
pub fn bridge_program(daw: Daw) -> Option<&'static str> {
    match daw {
        Daw::Reaper | Daw::Renoise => None,
        Daw::Logic => Some("logic-kumi"),
        Daw::SoundCraft => Some("soundcraft-kumi"),
    }
}

/// Which program to start. Once Logic has an owned installation, an incomplete or unreadable copy must be reported
/// at that path instead of silently running a different sibling. Without that layout a development bundle can still
/// provide the sibling program, as SoundCraft does.
fn bridge_executable(daw: Daw, kumi_home: &Path, host_executable: Option<&Path>) -> Option<PathBuf> {
    let name = bridge_program(daw)?;
    if daw == Daw::Logic {
        let target = daw_bridge::install::logic_target(kumi_home);
        let present = |path: &Path| match std::fs::symlink_metadata(path) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
            _ => true,
        };
        if present(&target.dir) || present(&BridgePaths::new(kumi_home, daw).receipt()) {
            return Some(target.path().join(daw_bridge::install::LOGIC_BINARY));
        }
    }
    let file = if cfg!(windows) { format!("{name}.exe") } else { name.to_string() };
    Some(host_executable.and_then(Path::parent).map_or_else(|| PathBuf::from(&file), |folder| folder.join(&file)))
}

/// How long the host waits on a bridge it starts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Times {
    /// The longest a call waits for a bridge that is starting. A call that waits that long is told the bridge is
    /// starting, and the bridge goes on starting.
    pub call: Duration,
    /// How long a bridge gets to serve (its discovery file appears). One that hasn't by then is stopped as failed, by the
    /// first call after (or the session's end).
    pub serve: Duration,
    /// How long after a bridge failed before another is started. The calls in between are told why at once.
    pub retry: Duration,
    /// How long a bridge asked to end gets before it's killed.
    pub stop: Duration,
}

impl Default for Times {
    fn default() -> Self {
        Self { call: Duration::from_secs(2), serve: Duration::from_secs(10), retry: Duration::from_secs(5), stop: Duration::from_secs(1) }
    }
}

/// How often a call waiting for a starting bridge looks for its discovery file.
const POLL: Duration = Duration::from_millis(50);

/// A bridge the host started, or what a test runs in its place.
pub trait SpawnedBridge: Send {
    /// Why it ended, once it has (the last line it wrote, else its exit status); None while it runs.
    fn exited(&mut self) -> Option<String>;
    /// Asks it to end: its stdin closes, as it does when the host dies. The bridge removes its discovery file first and
    /// then ends. From then on its stderr may be a pipe nobody reads, so a bridge ignores a write there that fails
    /// (`eprintln!` panics on one).
    fn stop(&mut self);
    /// Ends it now.
    fn kill(&mut self);
}

/// Starting a bridge: the bridge, or why it couldn't start.
pub type Starting = Pin<Box<dyn Future<Output = Result<Box<dyn SpawnedBridge>, String>> + Send>>;

type Start = Arc<dyn Fn() -> Starting + Send + Sync>;

/// How the host starts its DAW's bridge, and the bridge it started.
pub struct Spawn {
    /// Whether the DAW runs now. It may list the processes, so it's asked off the async threads.
    running: Arc<dyn Fn() -> bool + Send + Sync>,
    start: Start,
    times: Times,
    state: Arc<tokio::sync::Mutex<State>>,
}

#[derive(Default)]
enum State {
    #[default]
    Idle,
    /// A task of its own is getting a bridge going: making the secret, then starting the program.
    Starting,
    Started {
        bridge: Box<dyn SpawnedBridge>,
        at: Instant,
        /// A bridge has served since it started (its own discovery file, or another's).
        served: bool,
    },
    Failed {
        at: Instant,
        why: String,
    },
}

impl Spawn {
    /// The DAW table's: its bridge program, started with `--kumi-home` while the DAW runs. Logic's owned installation
    /// takes precedence over a bundled sibling. None for a DAW that loads its bridge itself.
    pub fn of(daw: Daw, kumi_home: &Path) -> Option<Self> {
        bridge_program(daw)?;
        let host_executable = std::env::current_exe().ok();
        let mut args = vec!["--kumi-home".into(), kumi_home.into()];
        if daw == Daw::Logic {
            args.push("--hosted".into());
        }
        let kumi_home = kumi_home.to_path_buf();
        let start = move || {
            // Setup may install the bridge after this Host was made. Resolve at each start, not at construction.
            let file = bridge_executable(daw, &kumi_home, host_executable.as_deref()).expect("a DAW with a bridge program");
            program(file, args.clone())()
        };
        Some(Self::new(move || daw_running(daw), start))
    }

    /// Starts bridges with `start`, for a DAW that runs while `running` says so.
    pub fn new(running: impl Fn() -> bool + Send + Sync + 'static, start: impl Fn() -> Starting + Send + Sync + 'static) -> Self {
        Self { running: Arc::new(running), start: Arc::new(start), times: Times::default(), state: Arc::default() }
    }

    pub fn with_times(mut self, times: Times) -> Self {
        self.times = times;
        self
    }

    /// The deadline of a call that begins now: all the call's waits for its bridge share it.
    pub(crate) fn deadline(&self) -> Instant {
        Instant::now() + self.times.call
    }

    /// Makes sure a bridge serves the DAW, if it runs: one that serves already (a discovery file that answers, whoever
    /// started its bridge), else the one the host is starting, else one it starts now. `serving` says whether a discovery
    /// file answers. Ok once a bridge serves, by `deadline`. Otherwise it says why none does: at once when the DAW isn't
    /// running or a bridge failed a moment ago, and that the bridge is starting when the deadline comes first.
    pub(crate) async fn bring_up(
        &self,
        daw: Daw,
        paths: &BridgePaths,
        serving: impl Fn() -> bool,
        deadline: Instant,
    ) -> Result<(), String> {
        let starting = || format!("Kumi's {daw} bridge is starting: ask again in a moment");
        loop {
            let up = serving();
            // (another call may be looking at the process listing: waited for no longer than this call's deadline)
            let Ok(mut state) = tokio::time::timeout_at(deadline.into(), self.state.lock()).await else { return Err(starting()) };
            if let State::Started { bridge, at, served } = &mut *state {
                *served |= up;
                let (served, at) = (*served, *at);
                if let Some(how) = bridge.exited() {
                    // One that served and ended (its DAW quit, or it lost it) is started again if the DAW runs. One that never
                    // served failed.
                    *state = if served {
                        State::Idle
                    } else {
                        State::Failed { at: Instant::now(), why: format!("Kumi's {daw} bridge ended before it served: {how}") }
                    };
                } else if !up && (served || at.elapsed() >= self.times.serve) {
                    // Its discovery file went after it served (its DAW quit), or it never served in time. It's stopped beside
                    // the call (one that ignores its stdin's end is killed after `Times::stop`), and one that never served
                    // failed.
                    let next = if served {
                        State::Idle
                    } else {
                        let why = format!("Kumi's {daw} bridge didn't start serving within {} s", self.times.serve.as_secs_f64());
                        State::Failed { at: Instant::now(), why }
                    };
                    if let State::Started { bridge, .. } = std::mem::replace(&mut *state, next) {
                        tokio::spawn(stop_bridge(bridge, self.times.stop));
                    }
                    continue;
                }
            }
            if up {
                return Ok(());
            }
            match &*state {
                // Starting: waited for below.
                State::Starting | State::Started { .. } => {}
                State::Failed { at, why } if at.elapsed() < self.times.retry => return Err(why.clone()),
                State::Idle | State::Failed { .. } => {
                    let running = self.running.clone();
                    if !tokio::task::spawn_blocking(move || running()).await.unwrap_or(false) {
                        *state = State::Idle;
                        return Err(format!("no {daw} is running: open {daw}"));
                    }
                    *state = State::Starting;
                    tokio::spawn(start(self.state.clone(), self.start.clone(), paths.secret(), daw, self.times.stop));
                }
            }
            drop(state);
            if Instant::now() >= deadline {
                return Err(starting());
            }
            tokio::time::sleep(POLL).await;
        }
    }

    /// Stops the bridge the host started, if one runs: the session ends. A start under way stops its bridge once it has
    /// one.
    pub(crate) async fn stop(&self) {
        let stopping = std::mem::replace(&mut *self.state.lock().await, State::Idle);
        if let State::Started { bridge, .. } = stopping {
            stop_bridge(bridge, self.times.stop).await;
        }
    }
}

/// Gets a bridge going beside the calls. First the secret the bridge and the host both prove they know: the host makes
/// it, off the async threads, since on Windows its owner-only permissions are a PowerShell run, seconds each. Then the
/// program. A session that ended meanwhile (the state is no longer `Starting`) has the new bridge stopped.
async fn start(state: Arc<tokio::sync::Mutex<State>>, start: Start, secret: PathBuf, daw: Daw, stop: Duration) {
    let made = tokio::task::spawn_blocking(move || daw_bridge::files::ensure_secret(&secret).map_err(|error| error.to_string())).await;
    let started = match made.map_err(|error| error.to_string()).and_then(|made| made) {
        Ok(_) => start().await.map_err(|why| format!("Kumi's {daw} bridge {why}")),
        Err(error) => Err(format!("the secret for Kumi's {daw} bridge can't be made: {error}")),
    };
    let mut state = state.lock().await;
    if !matches!(*state, State::Starting) {
        drop(state);
        if let Ok(bridge) = started {
            stop_bridge(bridge, stop).await;
        }
        return;
    }
    *state = match started {
        Ok(bridge) => State::Started { bridge, at: Instant::now(), served: false },
        Err(why) => State::Failed { at: Instant::now(), why },
    };
}

/// Whether one of the DAW's programs runs now, by the process listing (KUMI_PROCESSES stands in for it).
fn daw_running(daw: Daw) -> bool {
    daw_bridge::detect::processes(&|name| std::env::var(name).ok()).iter().any(|(_, name)| daw_bridge::detect::process_is(daw, name))
}

/// Asks a bridge to end, and kills it if it hasn't within `wait`.
async fn stop_bridge(mut bridge: Box<dyn SpawnedBridge>, wait: Duration) {
    bridge.stop();
    let deadline = Instant::now() + wait;
    while bridge.exited().is_none() && Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    if bridge.exited().is_none() {
        bridge.kill();
    }
}

/// Starts `program` with `args` as a bridge the host holds.
/// - Its stdin is a pipe the host keeps open while it wants the bridge.
/// - Its stdout is closed, because the host's own stdout is the MCP channel.
/// - Its stderr is passed on to the host's, and the end of it is kept to say why the bridge ended. Once the host is
///   gone, nobody reads that pipe, so the bridge must ignore a stderr write that fails and remove its discovery file
///   before it writes anything ([`SpawnedBridge::stop`]).
pub fn program(program: PathBuf, args: Vec<OsString>) -> impl Fn() -> Starting + Send + Sync + 'static {
    move || {
        let (program, args) = (program.clone(), args.clone());
        Box::pin(async move { start_program(&program, &args) })
    }
}

fn start_program(program: &Path, args: &[OsString]) -> Result<Box<dyn SpawnedBridge>, String> {
    let mut command = tokio::process::Command::new(program);
    command.args(args).stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::piped());
    // CREATE_NO_WINDOW: Kumi starts the host without a console, so the bridge would get a window of its own.
    #[cfg(windows)]
    command.creation_flags(0x0800_0000);
    let mut child = command.spawn().map_err(|error| format!("at {} couldn't start: {error}", program.display()))?;
    let stdin = child.stdin.take();
    let said = Arc::new(Mutex::new(Tail::default()));
    let reader = child.stderr.take().map(|stderr| tokio::spawn(pass_on(stderr, said.clone())));
    Ok(Box::new(Program { child, stdin, said, reader, ended: None }))
}

/// A bridge program the host started.
struct Program {
    child: tokio::process::Child,
    /// The host's end of the bridge's stdin, held while the host wants the bridge.
    stdin: Option<tokio::process::ChildStdin>,
    said: Arc<Mutex<Tail>>,
    reader: Option<tokio::task::JoinHandle<()>>,
    /// When it was first seen ended, while the rest of its stderr was still being read.
    ended: Option<Instant>,
}

/// How long after a bridge ends the host waits for the rest of its stderr, to say why it ended.
const LAST_WORDS: Duration = Duration::from_millis(500);

impl SpawnedBridge for Program {
    fn exited(&mut self) -> Option<String> {
        let status = match self.child.try_wait() {
            Ok(None) => return None,
            Ok(Some(status)) => status.to_string(),
            Err(error) => error.to_string(),
        };
        let reading = self.reader.as_ref().is_some_and(|reader| !reader.is_finished());
        if reading && self.ended.get_or_insert_with(Instant::now).elapsed() < LAST_WORDS {
            return None;
        }
        Some(self.said.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).last_line().unwrap_or(status))
    }

    fn stop(&mut self) {
        self.stdin.take();
    }

    fn kill(&mut self) {
        let _ = self.child.start_kill();
    }
}

/// The end of what a bridge wrote to its stderr: enough for its last line.
#[derive(Default)]
struct Tail(Vec<u8>);

const TAIL_BYTES: usize = 4096;

impl Tail {
    fn push(&mut self, bytes: &[u8]) {
        self.0.extend_from_slice(bytes);
        if let Some(over) = self.0.len().checked_sub(TAIL_BYTES) {
            self.0.drain(..over);
        }
    }

    /// The last line with something on it.
    fn last_line(&self) -> Option<String> {
        String::from_utf8_lossy(&self.0).lines().rev().map(str::trim).find(|line| !line.is_empty()).map(str::to_string)
    }
}

/// Passes a bridge's stderr on to the host's, keeping the end of it.
async fn pass_on(mut stderr: tokio::process::ChildStderr, said: Arc<Mutex<Tail>>) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let mut host = tokio::io::stderr();
    let mut chunk = vec![0u8; 8192];
    while let Ok(read @ 1..) = stderr.read(&mut chunk).await {
        said.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).push(&chunk[..read]);
        let _ = host.write_all(&chunk[..read]).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn logic_uses_its_owned_installation_and_never_hides_an_incomplete_one_with_a_sibling() {
        let root = tempfile::tempdir().unwrap();
        let home = root.path().join("kumi");
        let host = root.path().join("bundle/daw-mcp-server");
        let sibling_name = if cfg!(windows) { "logic-kumi.exe" } else { "logic-kumi" };
        let sibling = host.parent().unwrap().join(sibling_name);
        std::fs::create_dir_all(host.parent().unwrap()).unwrap();
        std::fs::write(&sibling, "bundled program").unwrap();
        let resolve = || bridge_executable(Daw::Logic, &home, Some(&host)).unwrap();
        assert_eq!(resolve(), sibling, "no installed layout: a bundle can provide the bridge");
        let target = daw_bridge::install::logic_target(&home);
        let installed = target.path().join(daw_bridge::install::LOGIC_BINARY);
        // A setup begun after the Host was created is observed on the next start, even before its copy is complete.
        std::fs::create_dir_all(&target.dir).unwrap();
        assert_eq!(resolve(), installed, "an incomplete owned layout must fail at its own path");
        std::fs::create_dir_all(target.path()).unwrap();
        std::fs::write(&installed, "installed program").unwrap();
        assert_eq!(resolve(), installed, "the installed copy wins over a different bundled one");
        std::fs::remove_file(&installed).unwrap();
        assert_eq!(resolve(), installed, "a missing owned binary is reported rather than replaced by the sibling");
        std::fs::remove_dir_all(&target.dir).unwrap();
        let receipt = BridgePaths::new(&home, Daw::Logic).receipt();
        std::fs::write(&receipt, "an unreadable receipt must not switch program copies").unwrap();
        assert_eq!(resolve(), installed, "a receipt with a missing folder still names an owned installation");
        std::fs::remove_file(receipt).unwrap();
        assert_eq!(resolve(), sibling);
    }

    #[test]
    fn soundcraft_keeps_its_sibling_program_and_loaded_bridges_have_no_program() {
        let root = tempfile::tempdir().unwrap();
        let host = root.path().join("bundle/daw-mcp-server");
        let name = if cfg!(windows) { "soundcraft-kumi.exe" } else { "soundcraft-kumi" };
        let owned = BridgePaths::new(root.path(), Daw::SoundCraft).bridge();
        std::fs::create_dir_all(owned).unwrap();
        assert_eq!(bridge_executable(Daw::SoundCraft, root.path(), Some(&host)), Some(host.parent().unwrap().join(name)));
        assert_eq!(bridge_executable(Daw::Reaper, root.path(), Some(&host)), None);
        assert_eq!(bridge_executable(Daw::Renoise, root.path(), Some(&host)), None);
    }

    #[test]
    fn what_a_bridge_said_last_is_its_last_line_with_words_and_the_tail_stays_bounded() {
        let mut tail = Tail::default();
        assert_eq!(tail.last_line(), None);
        tail.push(b"starting\nno control channel: open it through Kumi\n\n  \n");
        assert_eq!(tail.last_line().as_deref(), Some("no control channel: open it through Kumi"));
        tail.push(&[b'x'; 3 * TAIL_BYTES]);
        assert_eq!(tail.0.len(), TAIL_BYTES);
    }
}
