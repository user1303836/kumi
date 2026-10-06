//! `kumi bridge`: installs and updates the native bridge.
use crate::{input::TerminalInput, tui::tty::TtyOutput};
use futures::{future::LocalBoxFuture, FutureExt};
use kumi_common::{abort::Signal, js::string::trim};
use kumi_runtime::system::{self, Env, SystemProgram};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    rc::Rc,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::io::AsyncReadExt;
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Ran {
    pub code: i32,
    pub stdout: String,
    pub stderr: String,
}
pub type Run = Rc<dyn Fn(String, Vec<String>, Option<String>) -> LocalBoxFuture<'static, Ran>>;
pub fn default_run() -> Run {
    Rc::new(|command, args, cwd| async move { run_program(&command, &args, cwd.as_deref()).await }.boxed_local())
}
/// Run one command, preserving partial output on timeout and limiting each output stream to 16 MiB.
pub async fn run_program(command: &str, args: &[String], cwd: Option<&str>) -> Ran {
    let mut process = tokio::process::Command::new(command);
    process.args(args);
    if let Some(cwd) = cwd {
        process.current_dir(cwd);
    }
    process.stdin(std::process::Stdio::null()).stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::piped()).kill_on_drop(true);
    #[cfg(windows)]
    process.creation_flags(0x0800_0000);
    let Ok(mut child) = process.spawn() else { return Ran { code: 1, ..Default::default() } };
    let stdout = Arc::new(Mutex::new(vec![]));
    let stderr = Arc::new(Mutex::new(vec![]));
    let full = Signal::new();
    async fn read(mut source: impl tokio::io::AsyncRead + Unpin, data: Arc<Mutex<Vec<u8>>>, full: Signal) {
        let mut buffer = [0; 65536];
        loop {
            let Ok(n) = source.read(&mut buffer).await else { return };
            if n == 0 {
                return;
            }
            let mut data = data.lock().unwrap();
            let left = (16 * 1024 * 1024usize).saturating_sub(data.len());
            data.extend_from_slice(&buffer[..n.min(left)]);
            if n > left {
                full.cancel();
                return;
            }
        }
    }
    let out = tokio::spawn(read(child.stdout.take().unwrap(), stdout.clone(), full.clone()));
    let err = tokio::spawn(read(child.stderr.take().unwrap(), stderr.clone(), full.clone()));
    let status = tokio::select! {status=child.wait()=>status.ok(),_=full.cancelled()=>{let _=child.kill().await;None},_=tokio::time::sleep(Duration::from_secs(600))=>{let _=child.kill().await;None}};
    let _ = tokio::join!(out, err);
    let result = Ran {
        code: status.and_then(|s| s.code()).unwrap_or(1),
        stdout: String::from_utf8_lossy(&stdout.lock().unwrap()).into_owned(),
        stderr: String::from_utf8_lossy(&stderr.lock().unwrap()).into_owned(),
    };
    result
}
pub async fn remote_script_answers(config_path: &str) -> bool {
    let Some(value) = tokio::fs::read(config_path).await.ok().and_then(|b| serde_json::from_slice::<Value>(&b).ok()) else { return true };
    let Some(host) = value["bridge"]["host"].as_str().filter(|host| ["127.0.0.1", "::1"].contains(host)) else { return true };
    let Some(port) = value["bridge"]["port"].as_f64().filter(|n| n.fract() == 0.) else { return true };
    if !(0.0..=65535.0).contains(&port) {
        return false;
    }
    tokio::time::timeout(Duration::from_millis(1500), tokio::net::TcpStream::connect((host, port as u16))).await.is_ok_and(|r| r.is_ok())
}
pub async fn is_live_running(run: Run) -> bool {
    is_live_running_on(run, system::platform(), &system::process_env()).await
}
pub async fn is_live_running_on(run: Run, platform: &str, env: &Env) -> bool {
    if platform == "darwin" {
        return run("pgrep".into(), vec!["-x".into(), "Live".into()], None).await.code == 0;
    }
    if platform == "win32" {
        let asked = run(
            system::system_program(SystemProgram::Powershell, env, platform),
            [
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                "Get-Process -Name 'Ableton Live*' -ErrorAction SilentlyContinue | Select-Object -First 1 -ExpandProperty Id",
            ]
            .map(str::to_string)
            .into(),
            None,
        )
        .await;
        if asked.code == 0 {
            return asked.stdout.bytes().any(|c| c.is_ascii_digit());
        }
        return run(
            system::system_program(SystemProgram::Tasklist, env, platform),
            ["/FI", "IMAGENAME eq Ableton Live*", "/NH"].map(str::to_string).into(),
            None,
        )
        .await
        .stdout
        .to_ascii_lowercase()
        .contains("ableton live");
    }
    false
}
struct InputGuard(Option<Rc<dyn TerminalInput>>);
impl Drop for InputGuard {
    fn drop(&mut self) {
        if let Some(input) = &self.0 {
            input.pause();
        }
    }
}
/// Read a yes/no line in the terminal's normal line mode.
pub async fn ask_yes_no(input: Option<Rc<dyn TerminalInput>>, out: Rc<dyn TtyOutput>, question: &str) -> bool {
    let Some(input) = input.filter(|input| input.is_tty()) else { return false };
    out.write(&format!("{question} [y/N] "));
    let _guard = InputGuard(Some(input.clone()));
    let (send, receive) = tokio::sync::oneshot::channel();
    let send = Rc::new(std::cell::RefCell::new(Some(send)));
    let bytes = Rc::new(std::cell::RefCell::new(vec![]));
    input.on_end(Rc::new({
        let send = send.clone();
        let bytes = bytes.clone();
        let input = Rc::downgrade(&input);
        move || {
            if let Some(send) = send.borrow_mut().take() {
                if let Some(input) = input.upgrade() {
                    input.pause();
                }
                let _ = send.send(String::from_utf8_lossy(&bytes.borrow()).into_owned());
            }
        }
    }));
    input.resume(Rc::new({
        let send = send.clone();
        let input = Rc::downgrade(&input);
        move |chunk| {
            let end = chunk.iter().position(|b| matches!(b, b'\n' | b'\r'));
            bytes.borrow_mut().extend_from_slice(&chunk[..end.unwrap_or(chunk.len())]);
            if end.is_some() {
                if let Some(send) = send.borrow_mut().take() {
                    if let Some(input) = input.upgrade() {
                        input.pause();
                    }
                    let _ = send.send(String::from_utf8_lossy(&bytes.borrow()).into_owned());
                }
            }
        }
    }));
    receive.await.is_ok_and(|answer| matches!(trim(&answer).to_ascii_lowercase().as_str(), "y" | "yes"))
}
use crate::{
    config::{find_bridge_config, kumi_dir, remote_scripts_dir},
    doctor::read_bridge_server,
    live_extension::{extension_source, install_extension, live_extensions_dir, remove_former_extension},
    spinner::{spin, step},
};
use kumi_common::{js::string::head, time::now_ms};
use kumi_runtime::{
    core::errors::RuntimeError,
    ears::device::{install_ears, EARS_NAME},
    library::sources::{dirname, join},
    KUMI, KUMI_REPAIR, KUMI_START,
};
use sha2::{Digest, Sha256};
use std::{fs, path::Path};
pub type AsyncBool = Rc<dyn Fn() -> LocalBoxFuture<'static, bool>>;
pub type Confirm = Rc<dyn Fn(String) -> LocalBoxFuture<'static, bool>>;
#[derive(Clone)]
pub struct BridgeSetupIo {
    pub out: Rc<dyn TtyOutput>,
    pub env: Env,
    pub input: Option<Rc<dyn TerminalInput>>,
    pub yes: bool,
    pub allow_dirty: bool,
    pub wait_ms: Option<u64>,
    pub run: Option<Run>,
    pub live_running: Option<AsyncBool>,
    pub remote_script_answers: Option<Rc<dyn Fn(String) -> LocalBoxFuture<'static, bool>>>,
    pub confirm: Option<Confirm>,
    pub sleep: Option<Rc<dyn Fn(u64) -> LocalBoxFuture<'static, ()>>>,
    pub bridge_dir: Option<String>,
    pub home: Option<String>,
    pub prepared: Option<String>,
}
impl BridgeSetupIo {
    pub fn new(out: Rc<dyn TtyOutput>, env: Env) -> Self {
        Self {
            out,
            env,
            input: None,
            yes: false,
            allow_dirty: false,
            wait_ms: None,
            run: None,
            live_running: None,
            remote_script_answers: None,
            confirm: None,
            sleep: None,
            bridge_dir: None,
            home: None,
            prepared: None,
        }
    }
    /// For `install_quietly`, which keeps what's said to itself.
    pub fn quiet(env: Env) -> Self {
        Self::new(Rc::new(Captured::default()), env)
    }
}
fn error(e: impl std::fmt::Display) -> RuntimeError {
    RuntimeError::plain(e.to_string())
}
pub fn executable_name(name: &str) -> String {
    format!("{name}{}", if cfg!(windows) { ".exe" } else { "" })
}
pub fn executable_dir() -> String {
    std::env::current_exe().ok().and_then(|p| p.parent().map(|p| p.display().to_string())).unwrap_or(".".into())
}
pub fn repository_dir() -> String {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap_or_else(|_| Path::new(env!("CARGO_MANIFEST_DIR")).join("../.."))
        .display()
        .to_string()
}
/// The folder of the bridge this Kumi brings: an installed app's (its package.json is beside the
/// executable), or, for a checkout's build, the checkout's apps folder, whose live-extension it places.
pub fn bundled_bridge_dir() -> String {
    let folder = executable_dir();
    if Path::new(&join(&folder, "package.json")).exists() {
        folder
    } else {
        join(&repository_dir(), "apps")
    }
}
/// The version of the bridge this Kumi brings: an installed app names it in its package.json; a
/// checkout's build carries the bridge crate it was built with.
pub fn bundled_bridge_version() -> Option<String> {
    let folder = executable_dir();
    if Path::new(&join(&folder, "package.json")).exists() {
        bridge_version(&folder)
    } else {
        Some(ableton_mcp_server::delivery::PACKAGE_VERSION.into())
    }
}
pub fn bridge_version(root: &str) -> Option<String> {
    let value: Value = serde_json::from_slice(&fs::read(join(root, "package.json")).ok()?).ok()?;
    value.get("bridge").and_then(Value::as_str).or_else(|| value.get("version").and_then(Value::as_str)).map(str::to_string)
}
struct Prepared {
    artifact: String,
    sha256: String,
    root: String,
}
fn prepared_bridge(dir: &str) -> Option<Prepared> {
    let manifest: Value = serde_json::from_slice(&fs::read(join(dir, "prepared.json")).ok()?).ok()?;
    let artifact = manifest.get("artifact")?.as_str()?;
    let sha256 = manifest.get("sha256")?.as_str()?;
    if Path::new(artifact).file_name()?.to_str()? != artifact {
        return None;
    }
    let root = join(dir, "package");
    (Path::new(&join(dir, artifact)).exists() && Path::new(&join(&root, &executable_name("ableton-mcp-server"))).exists())
        .then(|| Prepared { artifact: artifact.into(), sha256: sha256.into(), root })
}
pub async fn copy_tree(from: &str, to: &str) -> std::io::Result<()> {
    let from = from.to_string();
    let to = to.to_string();
    tokio::task::spawn_blocking(move || {
        let mut stack = vec![(std::path::PathBuf::from(from), std::path::PathBuf::from(to))];
        while let Some((from, to)) = stack.pop() {
            let metadata = fs::symlink_metadata(&from)?;
            if metadata.is_symlink() {
                return Err(std::io::Error::other("native bridge packages cannot contain symbolic links"));
            }
            if metadata.is_dir() {
                fs::create_dir_all(&to)?;
                for entry in fs::read_dir(&from)? {
                    let entry = entry?;
                    stack.push((entry.path(), to.join(entry.file_name())));
                }
            } else if metadata.is_file() {
                if let Some(parent) = to.parent() {
                    fs::create_dir_all(parent)?;
                }
                fs::copy(&from, &to)?;
            } else {
                return Err(std::io::Error::other("native bridge packages cannot contain special files"));
            }
        }
        Ok(())
    })
    .await
    .map_err(std::io::Error::other)?
}
fn digest(file: &str) -> std::io::Result<String> {
    Ok(hex::encode(Sha256::digest(fs::read(file)?)))
}
fn last(ran: &Ran, most: usize) -> String {
    head(trim(if ran.stderr.is_empty() { &ran.stdout } else { &ran.stderr }).split('\n').next_back().unwrap_or(""), most)
}
pub(crate) fn lifecycle_answer(ran: Ran) -> Result<Value, String> {
    let json = |text: &str| serde_json::from_str::<Value>(trim(text).split('\n').filter(|s| !s.is_empty()).next_back().unwrap_or("")).ok();
    let Some(value) = json(&ran.stdout).or_else(|| json(&ran.stderr)).filter(|v| !v.is_null() && v != &Value::Bool(false)) else {
        let reason = last(&ran, 300);
        return Err(if reason.is_empty() { "the bridge's installer failed".into() } else { reason });
    };
    if value.get("version").and_then(Value::as_str).is_some_and(|s| s.contains("error")) || ran.code != 0 {
        Err(head(value.get("reason").and_then(Value::as_str).unwrap_or("the bridge's installer refused"), 400))
    } else {
        Ok(value)
    }
}
fn activated(value: &Value) -> bool {
    (value["state"] == "completed" && value["verification"]["liveConnected"] == true)
        || value["state"] == "activated"
        || value["verification"]["receipt"]["effectiveStatus"] == "activated"
}
fn tilde(path: &str) -> String {
    let home = home::home_dir().unwrap_or_default().display().to_string();
    path.strip_prefix(&home).map(|s| format!("~{s}")).unwrap_or(path.into())
}
fn place_extension(io: &BridgeSetupIo, roots: &[String], live_open: bool) {
    let Some(folder) = live_extensions_dir(&io.env, system::platform()) else { return };
    let Some(source) = roots.iter().find_map(|root| extension_source(root)) else { return };
    let result = (|| {
        let placed = install_extension(&source, &folder)?;
        remove_former_extension(&io.env, system::platform())?;
        Ok::<_, std::io::Error>(placed)
    })();
    match result {
        Ok(placed) if placed.changed => {
            let next = if live_open { " It starts the next time you open Live." } else { "" };
            io.out.write(&if placed.replaced{format!("Updated Kumi's extension in Live.{next}\n")}else{format!("Added Kumi's extension to Live: it renders tracks without playing them, writes MIDI clips in the Arrangement, and adds \"Ask Kumi about this\" to Live's right-click menu.{next}\n")});
        }
        Err(e) => io.out.write(&format!(
            "Kumi couldn't add its extension to Live ({e}); everything else works. Run {} bridge again to retry.\n",
            *KUMI
        )),
        _ => {}
    }
}
async fn place_ears(out: &dyn TtyOutput, scripts: &str) {
    let library = dirname(scripts);
    if Path::new(scripts).file_name().is_none_or(|n| n != "Remote Scripts") || !Path::new(&library).exists() {
        return;
    }
    if let Ok(placed) = install_ears(&library).await {
        if placed.written {
            out.write(&format!("Added Kumi's listening device to your User Library (Kumi › {EARS_NAME}): Kumi puts it on a track when it needs to hear it, and takes it away after.\n"));
        }
    }
}
struct StopOnKey {
    signal: Signal,
    input: Option<Rc<dyn TerminalInput>>,
}
impl StopOnKey {
    fn new(input: Option<Rc<dyn TerminalInput>>) -> Self {
        let signal = Signal::new();
        let input = input.filter(|i| i.is_tty());
        if let Some(input) = &input {
            let _ = input.set_raw_mode(true);
            input.resume(Rc::new({
                let signal = signal.clone();
                move |chunk| {
                    if chunk.iter().any(|b| matches!(b, 3 | 27 | b'\r' | b'\n')) {
                        signal.cancel();
                    }
                }
            }));
        }
        Self { signal, input }
    }
}
impl Drop for StopOnKey {
    fn drop(&mut self) {
        if let Some(input) = &self.input {
            let _ = input.set_raw_mode(false);
            input.pause();
        }
    }
}
async fn live_open(io: &BridgeSetupIo, run: Run) -> bool {
    step(
        io.out.clone(),
        &io.env,
        "Checking whether Live is open…",
        async {
            if let Some(live) = &io.live_running {
                live().await
            } else {
                is_live_running(run).await
            }
        },
        false,
    )
    .await
}
/// Locate existing ownership without inventing paths for custom configurations.
/// The lifecycle executable performs complete receipt/hash validation before applying anything.
pub(crate) fn owner_paths(config: &str, package: &str, home: &str) -> Option<(String, String, String)> {
    let candidates = [join(&dirname(config), "install-receipt.json"), join(home, "bridge/state/install-receipt.json")];
    for file in candidates {
        let Ok(metadata) = fs::symlink_metadata(&file) else { continue };
        if !metadata.is_file() || metadata.is_symlink() {
            continue;
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if metadata.permissions().mode() & 0o077 != 0 {
                continue;
            }
        }
        let Some(receipt) = fs::read(&file).ok().and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok()) else { continue };
        let field = |name: &str| receipt.get(name).and_then(Value::as_str);
        if receipt.get("version").and_then(Value::as_u64) != Some(1)
            || field("configPath") != Some(config)
            || field("packageRoot") != Some(package)
        {
            continue;
        }
        let (Some(state), Some(secret), Some(scripts)) = (field("stateDirectory"), field("secretPath"), field("remoteScriptsDirectory"))
        else {
            continue;
        };
        if [state, secret, scripts].iter().any(|path| !Path::new(path).is_absolute()) || join(state, "install-receipt.json") != file {
            continue;
        }
        return Some((state.into(), secret.into(), scripts.into()));
    }
    None
}
/// The native generation an installed bridge's receipt keeps to go back to (in the owner's `state`): its package
/// root and its version.
pub(crate) fn kept_generation(state: &str) -> Option<(String, String)> {
    let receipt: Value = serde_json::from_slice(&fs::read(join(state, "install-receipt.json")).ok()?).ok()?;
    let root = receipt["previous"]["packageRoot"].as_str().filter(|path| Path::new(path).is_absolute())?;
    let manifest: Value = serde_json::from_slice(&fs::read(join(root, "release-manifest.json")).ok()?).ok()?;
    if manifest["schema"] != "ableton-mcp-native-release/v1" {
        return None;
    }
    Some((root.into(), bridge_version(root)?))
}
/// The installed bridge's own rollback to the generation its receipt keeps (`lifecycle rollback`), and that
/// generation's Live extension put back after it. Applied again, it goes forward to the one it left.
pub(crate) struct BridgeRollback {
    command: String,
    args: Vec<String>,
    run: Run,
    /// The retained generation the rollback restores.
    previous: String,
}
impl BridgeRollback {
    /// The installed bridge (`command`, in `package`) going back to `previous`, with its owner's state, secret and
    /// Remote Scripts folder.
    pub(crate) fn new(
        command: String,
        package: String,
        owner: (String, String, String),
        config: String,
        previous: String,
        run: Run,
    ) -> Self {
        let (state, secret, scripts) = owner;
        let args = [
            "lifecycle",
            "rollback",
            "--package-root",
            package.as_str(),
            "--state-dir",
            state.as_str(),
            "--config",
            config.as_str(),
            "--secret",
            secret.as_str(),
            "--remote-scripts-dir",
            scripts.as_str(),
            "--apply",
            "--confirm-live-stopped",
        ]
        .map(str::to_string)
        .into();
        Self { command, args, run, previous }
    }
    pub(crate) async fn apply(&self) -> Result<(), RuntimeError> {
        let result = (self.run)(self.command.clone(), self.args.clone(), None).await;
        lifecycle_answer(result).map(|_| ()).map_err(RuntimeError::plain)
    }
    /// Put the restored generation's Live extension in Live, in place of this bridge's: each bridge checks that its
    /// extension carries its own registry. What to tell the producer, if anything.
    pub(crate) fn place_extension(&self, env: &Env) -> Option<String> {
        let folder = live_extensions_dir(env, system::platform())?;
        let again = format!("run {} bridge once to put it back", *KUMI);
        let Some(source) = extension_source(&self.previous) else {
            return crate::live_extension::installed_extension(&folder)
                .map(|_| format!("Kumi's extension in Live is this version's, which that bridge won't use: {again}."));
        };
        Some(match install_extension(&source, &folder) {
            Ok(placed) if placed.changed => "Kumi's extension in Live went back with it.".into(),
            Ok(_) => return None,
            Err(e) => format!("Couldn't put back the Live extension that goes with that bridge ({e}): {again}."),
        })
    }
}
/// Whether every file of the Remote Script in a bridge package (`<package>/remote-script/AbletonMcpBridge`)
/// is, byte for byte, in the installed one (`<scripts>/AbletonMcpBridge`). Files the install adds
/// (manifest, bridge reference) don't count: Live runs what it loaded, which these files are.
pub fn remote_script_unchanged(package: &str, scripts: &str) -> bool {
    fn same(source: &Path, installed: &Path) -> bool {
        let Ok(entries) = fs::read_dir(source) else { return false };
        let mut any = false;
        for entry in entries.flatten() {
            let (from, to) = (entry.path(), installed.join(entry.file_name()));
            let matches = match entry.file_type() {
                Ok(kind) if kind.is_dir() => same(&from, &to),
                Ok(kind) if kind.is_file() => {
                    fs::read(&from).ok().is_some_and(|ours| fs::read(&to).ok().is_some_and(|theirs| ours == theirs))
                }
                _ => false,
            };
            if !matches {
                return false;
            }
            any = true;
        }
        any
    }
    same(&Path::new(package).join("remote-script/AbletonMcpBridge"), &Path::new(scripts).join("AbletonMcpBridge"))
}
/// A bridge folder made for an install that hasn't finished: removed when the install stops early.
struct Unfinished(Option<String>);
impl Unfinished {
    fn keep(mut self) {
        self.0 = None;
    }
}
impl Drop for Unfinished {
    fn drop(&mut self) {
        if let Some(folder) = self.0.take() {
            let _ = fs::remove_dir_all(folder);
        }
    }
}
/// What `setup_bridge` says, kept for the in-app setup to show what went wrong.
#[derive(Default)]
struct Captured(std::cell::RefCell<String>);
impl TtyOutput for Captured {
    fn is_tty(&self) -> bool {
        false
    }
    fn columns(&self) -> Option<i32> {
        None
    }
    fn rows(&self) -> Option<i32> {
        None
    }
    fn write(&self, data: &str) {
        self.0.borrow_mut().push_str(data);
    }
}

/// The bridge put in place from Kumi's own setup, with Live closed: nothing printed, no question asked
/// and no wait for Live, which the setup opens itself. The bridge's version, or what went wrong in a
/// sentence; Live found open (it opened again) is the setup's to fix, so it says so.
pub async fn install_quietly(mut io: BridgeSetupIo) -> Result<String, String> {
    let said = Rc::new(Captured::default());
    io.out = said.clone();
    io.input = None;
    io.yes = true;
    io.wait_ms = Some(0);
    // Each look for Live is noted: a refusal right after one that found it open is Live's.
    let open = Rc::new(std::cell::Cell::new(false));
    let look = io.live_running.clone();
    let run = io.run.clone().unwrap_or_else(default_run);
    io.live_running = Some({
        let open = open.clone();
        Rc::new(move || {
            let (look, run, open) = (look.clone(), run.clone(), open.clone());
            async move {
                let running = match look {
                    Some(look) => look().await,
                    None => is_live_running(run).await,
                };
                open.set(running);
                running
            }
            .boxed_local()
        })
    });
    let version = match &io.bridge_dir {
        Some(dir) => bridge_version(dir),
        None => bundled_bridge_version(),
    };
    match setup_bridge(io).await {
        Ok(0) => Ok(version.unwrap_or_default()),
        Ok(_) if open.get() => Err("Live is open again: quit it, then Try again.".into()),
        Ok(_) => {
            let said = said.0.borrow();
            Err(said.lines().map(trim).rfind(|line| !line.is_empty()).unwrap_or("The bridge didn't go in place.").to_string())
        }
        Err(error) => Err(error.message()),
    }
}

pub async fn setup_bridge(io: BridgeSetupIo) -> Result<i32, RuntimeError> {
    let say = |line: &str| io.out.write(&format!("{line}\n"));
    let run = io.run.clone().unwrap_or_else(default_run);
    let bridge_dir = io.bridge_dir.clone().unwrap_or_else(bundled_bridge_dir);
    let mut scripts = remote_scripts_dir(&io.env);
    let Some(bundled) = (match &io.bridge_dir {
        Some(dir) => bridge_version(dir),
        None => bundled_bridge_version(),
    }) else {
        say(&format!("Kumi's copy of the bridge is missing. Run {}.", *KUMI_REPAIR));
        return Ok(1);
    };
    let native = join(&bridge_dir, &executable_name("ableton-mcp-server"));
    let native = if Path::new(&native).exists() { native } else { join(&executable_dir(), &executable_name("ableton-mcp-server")) };
    if !Path::new(&native).exists() {
        say(&format!("The bridge isn't built yet. Run {}.", *KUMI_REPAIR));
        return Ok(1);
    }
    let config = find_bridge_config(&io.env);
    let home = io.home.clone().unwrap_or_else(|| kumi_dir(&io.env));
    let installed = config.as_ref().and_then(|c| read_bridge_server(c).ok());
    let owner = config
        .as_deref()
        .zip(installed.as_ref().and_then(|s| s.package_root()))
        .and_then(|(config, package)| owner_paths(config, &package, &home));
    let mut state = config.as_ref().map(|c| dirname(c)).unwrap_or_else(|| join(&home, "bridge/state"));
    let mut secret = None;
    if let Some((owned_state, owned_secret, owned_scripts)) = owner {
        state = owned_state;
        secret = Some(owned_secret);
        scripts = owned_scripts;
    }
    if installed.as_ref().is_some_and(|s| s.native() && s.version.as_ref() == Some(&bundled)) {
        say(&format!("The Ableton bridge {bundled} is installed, the same as Kumi's."));
        let mut roots: Vec<_> = installed.as_ref().and_then(|s| s.package_root()).into_iter().collect();
        roots.push(bridge_dir);
        place_extension(&io, &roots, live_open(&io, run).await);
        place_ears(io.out.as_ref(), &scripts).await;
        return Ok(0);
    }
    // An app rollback can leave a newer Kumi's bridge in Live: it serves this Kumi too, and the lifecycle never
    // updates to an older bridge. Its own rollback puts this Kumi's back when that's the generation it kept.
    if let Some(newer) =
        installed.as_ref().filter(|s| s.native()).and_then(|s| s.version.clone()).filter(|v| crate::update::newer(v, &bundled))
    {
        let package = installed.as_ref().and_then(|s| s.package_root());
        let kept = secret.as_ref().and_then(|_| kept_generation(&state)).filter(|(_, version)| *version == bundled);
        let (Some(config), Some(command), Some(package), Some(secret), Some((kept, _))) =
            (config.clone(), installed.as_ref().and_then(|s| s.command.clone()), package.clone(), secret.clone(), kept)
        else {
            say(&format!("The bridge in Live is {newer}, from a newer Kumi. It works with this one, so it stays."));
            place_extension(&io, &package.into_iter().collect::<Vec<_>>(), live_open(&io, run).await);
            place_ears(io.out.as_ref(), &scripts).await;
            return Ok(0);
        };
        say(&format!("The bridge in Live is {newer}, from a newer Kumi. It works with this one; this Kumi's own, {bundled}, is kept and can go back."));
        let after_update = io.env.get("KUMI_BRIDGE_AFTER").is_some_and(|value| value == "1");
        let later = |what: &str| {
            say(&format!("The bridge {newer} stays for now: {what}, then run: {} bridge", *KUMI));
            Ok(0)
        };
        if live_open(&io, run.clone()).await {
            if after_update {
                return later("to put back its own, save your work, quit Live");
            }
            say(&format!("Live is open. To put back {bundled}, save your work, quit Live, then run this again: {} bridge", *KUMI));
            return Ok(1);
        }
        if !io.yes
            && !(if let Some(confirm) = &io.confirm {
                confirm("Is Live closed, with your work saved?".into()).await
            } else {
                ask_yes_no(io.input.clone(), io.out.clone(), "Is Live closed, with your work saved?").await
            })
        {
            if after_update {
                return later("to put back its own, quit Live");
            }
            say(&format!("Nothing was changed. To put back {bundled}, quit Live, then run: {} bridge", *KUMI));
            return Ok(1);
        }
        let rollback = BridgeRollback::new(
            command,
            package,
            (state.clone(), secret.clone(), scripts.clone()),
            config.clone(),
            kept.clone(),
            run.clone(),
        );
        if let Err(reason) =
            step(io.out.clone(), &io.env, "Putting back Live's Remote Script and the bridge…", rollback.apply(), true).await
        {
            say(&format!("The bridge's rollback stopped, and put back what was there: {}", reason.message()));
            return Ok(1);
        }
        say(&format!("Done: the Ableton bridge {bundled} is back ({}).", tilde(&scripts)));
        if let Some(said) = rollback.place_extension(&io.env) {
            say(&said);
        }
        place_ears(io.out.as_ref(), &scripts).await;
        say("");
        say("Now open Live. Kumi connects on its own.");
        let activate = || {
            let args = [
                "lifecycle",
                "activate",
                "--remote-scripts-dir",
                scripts.as_str(),
                "--state-dir",
                state.as_str(),
                "--package-root",
                kept.as_str(),
            ];
            let mut args: Vec<String> = args.map(str::to_string).into();
            args.extend(["--config".into(), config.clone(), "--secret".into(), secret.clone()]);
            run(join(&kept, &executable_name("ableton-mcp-server")), args, None)
        };
        let connected = format!("Live is connected through this Kumi's bridge again. Run: {}", *KUMI_START);
        return Ok(wait_for_live(&io, config.clone(), &activate, &connected).await);
    }
    say(&if config.is_some() {
        format!(
            "Kumi's bridge is {bundled}; the one Live uses is {}. Updating it takes a minute.",
            installed.as_ref().and_then(|s| s.version.as_deref()).unwrap_or("older")
        )
    } else {
        format!("Kumi will install the Ableton bridge {bundled}: the Remote Script Live loads, and the local server Kumi talks to.")
    });
    // An update's or a rollback's last step (Kumi's updater, or an older Kumi's through the migration
    // shim): Kumi itself has changed, so a bridge left for later isn't the command failing.
    let after_update = io.env.get("KUMI_BRIDGE_AFTER").is_some_and(|value| value == "1");
    let later = |what: &str| {
        say(&format!("The bridge wasn't updated yet: {what}, then run: {} bridge", *KUMI));
        Ok(0)
    };
    if live_open(&io, run.clone()).await {
        if after_update {
            return later("save your work, quit Live");
        }
        say(&format!("Live is open. Save your work, quit Live, then run this again: {} bridge", *KUMI));
        return Ok(1);
    }
    if !io.yes
        && !(if let Some(confirm) = &io.confirm {
            confirm("Is Live closed, with your work saved?".into()).await
        } else {
            ask_yes_no(io.input.clone(), io.out.clone(), "Is Live closed, with your work saved?").await
        })
    {
        if after_update {
            return later("quit Live");
        }
        say(&format!("Nothing was changed. Quit Live, then run: {} bridge", *KUMI));
        return Ok(1);
    }
    if !Path::new(&scripts).exists() {
        if Path::new(&scripts).file_name().is_none_or(|n| n != "Remote Scripts") || !Path::new(&dirname(&scripts)).exists() {
            say(&format!("Kumi couldn't find Live's User Library (it looked for {}). Open Live once so it makes one, or set KUMI_REMOTE_SCRIPTS_DIR to your User Library's Remote Scripts folder (Live's Settings → Library shows where it is).",tilde(&dirname(&scripts))));
            return Ok(1);
        }
        fs::create_dir(&scripts).map_err(error)?;
    }
    let folder = join(io.home.as_deref().unwrap_or(&kumi_dir(&io.env)), &format!("bridge/{bundled}-{}", now_ms()));
    let mut dirs = fs::DirBuilder::new();
    dirs.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        dirs.mode(0o700);
    }
    dirs.create(&folder).map_err(error)?;
    // Until the lifecycle has put this version in place, nothing refers to its folder.
    let unfinished = Unfinished(Some(folder.clone()));
    let prepared = io.prepared.clone().unwrap_or_else(|| join(&executable_dir(), "bridge"));
    let ready = prepared_bridge(&prepared);
    let artifact;
    let sha;
    let root = join(&folder, "package");
    if let Some(ready) = ready {
        artifact = join(&folder, &ready.artifact);
        let copied = step(
            io.out.clone(),
            &io.env,
            "Copying the bridge…",
            async {
                tokio::fs::copy(join(&prepared, &ready.artifact), &artifact).await.map_err(error)?;
                if digest(&artifact).map_err(error)? != ready.sha256 {
                    return Ok::<_, RuntimeError>(false);
                }
                copy_tree(&ready.root, &root).await.map_err(error)?;
                Ok(true)
            },
            true,
        )
        .await?;
        if !copied {
            say(&format!("Kumi's copy of the bridge is damaged. Run {}.", *KUMI_REPAIR));
            return Ok(1);
        }
        sha = ready.sha256;
    } else {
        let producer = join(&repository_dir(), "scripts/build-native-release.py");
        let built = step(
            io.out.clone(),
            &io.env,
            "Preparing the bridge…",
            run(
                if cfg!(windows) { "python".into() } else { "python3".into() },
                vec![producer, "--bridge-only".into(), "--binaries-dir".into(), dirname(&native), "--out".into(), folder.clone()],
                Some(repository_dir()),
            ),
            true,
        )
        .await;
        if built.code != 0 {
            say(&format!("Packing the bridge failed: {}", last(&built, 300)));
            return Ok(1);
        }
        let manifest: Value = serde_json::from_slice(&fs::read(join(&folder, "prepared.json")).map_err(error)?).map_err(error)?;
        let Some(name) = manifest.get("artifact").and_then(Value::as_str).filter(|n| Path::new(n).file_name().is_some_and(|p| p == *n))
        else {
            return Err(RuntimeError::plain("The bridge producer returned no artifact."));
        };
        artifact = join(&folder, name);
        sha = digest(&artifact).map_err(error)?;
        let unpacked = run(
            system::system_program_default(SystemProgram::Tar),
            vec!["-xzf".into(), artifact.clone(), "-C".into(), folder.clone()],
            None,
        )
        .await;
        if unpacked.code != 0 {
            say(&format!("Installing the bridge's package failed: {}", last(&unpacked, 300)));
            return Ok(1);
        }
    }
    let lifecycle = |action: &str, extra: Vec<String>| {
        let mut args = vec![
            "lifecycle".into(),
            action.into(),
            "--remote-scripts-dir".into(),
            scripts.clone(),
            "--state-dir".into(),
            state.clone(),
            "--package-root".into(),
            root.clone(),
        ];
        if let Some(config) = &config {
            args.extend(["--config".into(), config.clone()]);
        }
        if let Some(secret) = &secret {
            args.extend(["--secret".into(), secret.clone()]);
        }
        args.extend(extra);
        if io.allow_dirty {
            args.push("--allow-dirty-private-build".into())
        }
        run(join(&root, &executable_name("ableton-mcp-server")), args, None)
    };
    let action = if config.is_some() { "upgrade" } else { "install" };
    let artifact_args = vec!["--artifact".into(), artifact, "--artifact-sha256".into(), sha];
    let plan =
        lifecycle_answer(step(io.out.clone(), &io.env, "Checking what changes…", lifecycle(action, artifact_args.clone()), false).await);
    if let Err(reason) = plan {
        say(&format!("The bridge's installer refused: {reason}"));
        if reason.to_ascii_lowercase().contains("dirty") {
            say("This checkout has uncommitted changes; to install it anyway (developers only), add --allow-dirty.");
        }
        return Ok(1);
    }
    // Live may have opened since the first look (quit, then opened again): the switch runs with it closed.
    if live_open(&io, run.clone()).await {
        say(&format!("Live is open again, so nothing was changed. Save your work, quit Live, then run this again: {} bridge", *KUMI));
        return Ok(1);
    }
    let mut apply_args = artifact_args;
    apply_args.extend(["--apply".into(), "--confirm-live-stopped".into()]);
    let applied = lifecycle_answer(
        step(
            io.out.clone(),
            &io.env,
            if action == "upgrade" {
                "Updating Live's Remote Script and the bridge…"
            } else {
                "Installing Live's Remote Script and the bridge…"
            },
            lifecycle(action, apply_args),
            true,
        )
        .await,
    );
    if let Err(reason) = applied {
        say(&format!("The bridge's installer stopped, and put back what was there: {reason}"));
        return Ok(1);
    }
    unfinished.keep();
    say(&format!("Done: the Ableton bridge {bundled} is installed ({}).", tilde(&scripts)));
    place_extension(&io, std::slice::from_ref(&root), false);
    place_ears(io.out.as_ref(), &scripts).await;
    say("");
    say(if config.is_some() {
        "Now open Live. Kumi connects on its own."
    } else {
        "Now open Live, and in Settings → Link, Tempo & MIDI choose AbletonMcpBridge as a Control Surface. Kumi connects on its own."
    });
    let config_path = find_bridge_config(&io.env).unwrap_or_else(|| join(&state, "bridge-config.json"));
    let activate = || lifecycle("activate", vec![]);
    Ok(wait_for_live(&io, config_path, &activate, &format!("Live is connected through the new bridge. Run: {}", *KUMI_START)).await)
}

/// After a bridge went in place: wait for Live to load it, and record the connection (`activate`).
async fn wait_for_live(
    io: &BridgeSetupIo,
    config_path: String,
    activate: &dyn Fn() -> LocalBoxFuture<'static, Ran>,
    connected_line: &str,
) -> i32 {
    let say = |line: &str| io.out.write(&format!("{line}\n"));
    let wait_ms = io.wait_ms.unwrap_or(600000);
    if wait_ms == 0 {
        return 0;
    }
    let stop = StopOnKey::new(io.input.clone());
    let waiting = spin(io.out.clone(), &io.env, "Waiting for Live… (Enter or Ctrl-C stops waiting; nothing else depends on it)", true);
    let mut connected = false;
    let attempts = wait_ms.div_ceil(2000).max(1);
    for attempt in 0..attempts {
        if stop.signal.is_cancelled() || connected {
            break;
        }
        let answers = if let Some(answers) = &io.remote_script_answers {
            answers(config_path.clone()).await
        } else {
            remote_script_answers(&config_path).await
        };
        if answers {
            connected = lifecycle_answer(activate().await).is_ok_and(|v| activated(&v))
        }
        if attempt < attempts - 1 && !stop.signal.is_cancelled() && !connected {
            let sleep = async {
                if let Some(sleep) = &io.sleep {
                    sleep(2000).await
                } else {
                    tokio::time::sleep(Duration::from_millis(2000)).await
                }
            };
            tokio::select! {_=sleep=>{},_=stop.signal.cancelled()=>{}}
        }
    }
    waiting.stop();
    let stopped = stop.signal.is_cancelled();
    drop(stop);
    if connected {
        say(connected_line);
        return 0;
    }
    say(&if stopped {
        format!(
            "Stopped waiting. Kumi connects on its own once Live has AbletonMcpBridge as a Control Surface; {} doctor says how it stands.",
            *KUMI
        )
    } else {
        format!("Live didn't connect yet; Kumi will connect when it does. If it doesn't, run: {} doctor", *KUMI)
    });
    0
}
