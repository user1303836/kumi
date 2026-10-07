//! Launch and share Live's Extension Host, without installing a second host into Live.
use super::extension_channel::read_extension_endpoint;
use crate::{
    live::LiveError,
    platform::{current_platform, windows_powershell},
};
use base64::{
    alphabet,
    engine::{
        general_purpose::{GeneralPurpose, GeneralPurposeConfig, URL_SAFE_NO_PAD},
        DecodePaddingMode,
    },
    Engine,
};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::{
    io::Write,
    path::{Path, PathBuf},
    rc::Rc,
    time::{Duration, SystemTime},
};
use tokio::{io::AsyncReadExt, process::Command};
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtensionHostBinary {
    pub node: PathBuf,
    pub module: PathBuf,
}
fn host_in(directory: &Path) -> Option<ExtensionHostBinary> {
    let node = directory.join(if cfg!(windows) { "node.exe" } else { "node" });
    let module = directory.join("ExtensionHostNodeModule.node");
    (node.exists() && module.exists()).then_some(ExtensionHostBinary { node, module })
}
/// `ps` by its path: Kumi starts the bridge with PATH holding only the bridge's own folder.
fn ps() -> &'static str {
    if Path::new("/bin/ps").exists() {
        "/bin/ps"
    } else {
        "ps"
    }
}
/// This user's processes only, from `ps -axo uid=,<field>=` lines: another user's Live, host or folders aren't Kumi's.
#[cfg(unix)]
fn own_lines(listing: &str) -> String {
    // SAFETY: getuid has no preconditions.
    let uid = unsafe { libc::getuid() }.to_string();
    listing
        .lines()
        .filter_map(|line| line.trim_start().split_once(' ').filter(|(owner, _)| *owner == uid).map(|(_, rest)| rest.trim_start()))
        .collect::<Vec<_>>()
        .join("\n")
}
fn running_live_app() -> Option<PathBuf> {
    if current_platform() != "darwin" {
        return None;
    }
    let output = std::process::Command::new(ps()).args(["-axo", "uid=,comm="]).output().ok()?;
    let listing = String::from_utf8_lossy(&output.stdout);
    #[cfg(unix)]
    let listing = own_lines(&listing);
    listing
        .lines()
        .map(kumi_common::js::string::trim)
        .find(|line| line.ends_with(".app/Contents/MacOS/Live"))
        .map(|line| PathBuf::from(line.strip_suffix("/Contents/MacOS/Live").unwrap()))
}
pub fn find_extension_host(live_app: Option<&Path>) -> Option<ExtensionHostBinary> {
    let mut candidates = vec![];
    let mut add = |path: PathBuf| {
        let text = path.to_string_lossy();
        if text.is_empty() {
            return;
        }
        candidates.push(if text.ends_with(".app") {
            path.join("Contents/Helpers/ExtensionHost")
        } else if text.ends_with(".exe") {
            path.parent().unwrap_or(Path::new(".")).join("ExtensionHost")
        } else {
            path
        });
    };
    if let Some(path) = live_app {
        add(path.into());
    }
    if let Some(path) = running_live_app() {
        add(path);
    }
    if cfg!(target_os = "macos") {
        let mut names: Vec<_> =
            std::fs::read_dir("/Applications").into_iter().flatten().filter_map(Result::ok).map(|entry| entry.file_name()).collect();
        names.sort();
        for name in names {
            let text = name.to_string_lossy();
            if text.starts_with("Ableton Live 12") && text.ends_with(".app") {
                add(Path::new("/Applications").join(name));
            }
        }
    } else if cfg!(windows) {
        // An older Kumi doesn't pass ProgramData on: it's on the system drive, which may not be C:.
        let program_data = std::env::var("ProgramData")
            .or_else(|_| std::env::var("SystemDrive").map(|drive| format!("{drive}\\ProgramData")))
            .unwrap_or_else(|_| "C:\\ProgramData".into());
        let root = PathBuf::from(program_data).join("Ableton");
        let mut names: Vec<_> =
            std::fs::read_dir(&root).into_iter().flatten().filter_map(Result::ok).map(|entry| entry.file_name()).collect();
        names.sort();
        for name in names {
            if name.to_string_lossy().starts_with("Live 12") {
                add(root.join(name).join("Program/ExtensionHost"));
            }
        }
    }
    candidates.iter().find_map(|directory| host_in(directory))
}
fn is_extension(path: &Path) -> bool {
    path.join("manifest.json").exists() && path.join("package.json").exists() && path.join("dist/extension.js").exists()
}
/// A bundle staged beside the native bridge, or, for a checkout's build, the checkout's extension.
pub fn find_extension_bundle() -> Option<PathBuf> {
    let mut candidates = vec![];
    if let Ok(executable) = std::env::current_exe() {
        if let Some(directory) = executable.parent() {
            candidates.push(directory.join("live-extension"));
            candidates.push(directory.join("../live-extension"));
        }
    }
    candidates.push(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../apps/live-extension"));
    candidates.into_iter().find(|path| is_extension(path)).map(|path| std::fs::canonicalize(&path).unwrap_or(path))
}
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExtensionHosts {
    pub kumi: Vec<PathBuf>,
    pub live: bool,
}
pub fn parse_extension_hosts(listing: &str) -> ExtensionHosts {
    static HOST: std::sync::LazyLock<regex::Regex> =
        std::sync::LazyLock::new(|| regex::Regex::new(r"ExtensionHost[\\/]node(\.exe)?\b").unwrap());
    static STORAGE: std::sync::LazyLock<regex::Regex> =
        std::sync::LazyLock::new(|| regex::Regex::new(r"kumi-storage:([A-Za-z0-9_-]+)").unwrap());
    let mut result = ExtensionHosts::default();
    let decoder = GeneralPurpose::new(
        &alphabet::URL_SAFE,
        GeneralPurposeConfig::new().with_decode_padding_mode(DecodePaddingMode::Indifferent).with_decode_allow_trailing_bits(true),
    );
    for line in listing.lines() {
        if !HOST.is_match(line) {
            continue;
        }
        if !line.contains("__kumiLaunchedHost") {
            result.live = true;
            continue;
        }
        if let Some(found) = STORAGE.captures(line) {
            let mut encoded = &found[1];
            if encoded.len() % 4 == 1 {
                encoded = &encoded[..encoded.len() - 1];
            }
            let bytes = decoder.decode(encoded).unwrap_or_default();
            result.kumi.push(PathBuf::from(String::from_utf8_lossy(&bytes).into_owned()));
        }
    }
    result
}
pub async fn running_extension_hosts() -> ExtensionHosts {
    let mut command = if cfg!(windows) {
        let mut command = Command::new(windows_powershell(None));
        command.args([
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            "Get-CimInstance Win32_Process -Filter \"Name='node.exe'\" | ForEach-Object { $_.CommandLine }",
        ]);
        command
    } else {
        let mut command = Command::new(ps());
        command.args(["-axo", "uid=,command="]);
        command
    };
    #[cfg(windows)]
    command.creation_flags(0x08000000);
    // Never the bridge's stdin, which is Kumi's request pipe: Windows PowerShell reads a redirected stdin to its end.
    command.stdin(std::process::Stdio::null()).stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::null()).kill_on_drop(true);
    let Ok(mut child) = command.spawn() else {
        return ExtensionHosts::default();
    };
    let Some(stdout) = child.stdout.take() else {
        return ExtensionHosts::default();
    };
    let mut output = vec![];
    let result = tokio::time::timeout(Duration::from_secs(20), async {
        let _ = stdout.take(16 * 1024 * 1024 + 1).read_to_end(&mut output).await;
        if output.len() > 16 * 1024 * 1024 {
            output.truncate(16 * 1024 * 1024);
            let _ = child.start_kill();
        }
        let _ = child.wait().await;
    })
    .await;
    if result.is_err() {
        let _ = child.start_kill();
    }
    let listing = String::from_utf8_lossy(&output);
    #[cfg(unix)]
    let listing = own_lines(&listing);
    parse_extension_hosts(&listing)
}
#[derive(Clone, Default)]
pub enum ExtensionScan {
    #[default]
    Enabled,
    Disabled,
    Function(Rc<dyn Fn() -> ExtensionHosts>),
}
#[derive(Clone)]
pub struct LaunchOptions {
    pub storage_directory: PathBuf,
    pub extension: Option<PathBuf>,
    pub live_app: Option<PathBuf>,
    pub wait_ms: Option<f64>,
    pub log: Option<Rc<dyn Fn(&str)>>,
    pub scan: ExtensionScan,
    pub on_shared: Option<Rc<dyn Fn(&Path)>>,
    pub lock_path: Option<PathBuf>,
}
impl LaunchOptions {
    pub fn new(storage_directory: impl Into<PathBuf>) -> Self {
        Self {
            storage_directory: storage_directory.into(),
            extension: None,
            live_app: None,
            wait_ms: None,
            log: None,
            scan: ExtensionScan::default(),
            on_shared: None,
            lock_path: None,
        }
    }
    async fn look(&self) -> ExtensionHosts {
        match &self.scan {
            ExtensionScan::Enabled => running_extension_hosts().await,
            ExtensionScan::Disabled => ExtensionHosts::default(),
            ExtensionScan::Function(scan) => scan(),
        }
    }
    fn log(&self, line: &str) {
        if let Some(log) = &self.log {
            log(line);
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum LaunchOutcome {
    Answering,
    Shared,
    LiveHost,
    Unavailable,
    Started,
    Failed,
}
fn mkdir(path: &Path) -> std::io::Result<()> {
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(path)
}
fn open_options() -> std::fs::OpenOptions {
    let mut options = std::fs::OpenOptions::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options
}
fn ensure_secret(storage: &Path) -> std::io::Result<()> {
    let path = storage.join("secret");
    if path.exists() {
        let bytes = std::fs::read(&path)?;
        if kumi_common::js::string::utf16_len(kumi_common::js::string::trim(&String::from_utf8_lossy(&bytes))) >= 32 {
            return Ok(());
        }
    }
    let mut random = [0; 32];
    rand::rng().fill_bytes(&mut random);
    open_options().write(true).create(true).truncate(true).open(path)?.write_all(format!("{}\n", URL_SAFE_NO_PAD.encode(random)).as_bytes())
}
struct LaunchLock(PathBuf);
impl Drop for LaunchLock {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}
fn take_lock(path: &Path) -> Option<LaunchLock> {
    if std::fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|mtime| SystemTime::now().duration_since(mtime).ok())
        .is_some_and(|elapsed| elapsed > Duration::from_secs(30))
    {
        let _ = std::fs::remove_file(path);
    }
    open_options().write(true).create_new(true).open(path).ok()?;
    Some(LaunchLock(path.into()))
}
fn find_shared(options: &LaunchOptions, hosts: &ExtensionHosts) -> Option<PathBuf> {
    hosts.kumi.iter().find(|folder| *folder != &options.storage_directory && read_extension_endpoint(folder).is_some()).cloned()
}
fn shared(options: &LaunchOptions, path: &Path) -> LaunchOutcome {
    if let Some(on_shared) = &options.on_shared {
        on_shared(path);
    }
    LaunchOutcome::Shared
}
/// The bridge's stdin, stdout and stderr made non-inheritable. Kumi made them inheritable pipes, and Windows hands
/// every inheritable handle to a child: the long-lived Extension Host would keep Kumi's pipes open after the bridge
/// exits, so Kumi would never see it close. A child given one of them explicitly still gets its own copy.
#[cfg(windows)]
fn keep_std_handles_from_children() {
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetStdHandle(which: u32) -> *mut std::ffi::c_void;
        fn SetHandleInformation(handle: *mut std::ffi::c_void, mask: u32, flags: u32) -> i32;
    }
    const HANDLE_FLAG_INHERIT: u32 = 1;
    // STD_INPUT_HANDLE, STD_OUTPUT_HANDLE, STD_ERROR_HANDLE.
    for which in [-10i32 as u32, -11i32 as u32, -12i32 as u32] {
        // SAFETY: plain handle queries and flag changes on this process's own standard handles.
        unsafe {
            let handle = GetStdHandle(which);
            if !handle.is_null() && handle as isize != -1 {
                SetHandleInformation(handle, HANDLE_FLAG_INHERIT, 0);
            }
        }
    }
}
/// Start only one Extension Host across bridge instances and wait for this process's endpoint.
pub async fn launch_extension(options: LaunchOptions) -> Result<LaunchOutcome, LiveError> {
    let io = |error: std::io::Error| LiveError::error(error.to_string());
    mkdir(&options.storage_directory).map_err(io)?;
    if read_extension_endpoint(&options.storage_directory).is_some() {
        return Ok(LaunchOutcome::Answering);
    }
    let running = options.look().await;
    if let Some(other) = find_shared(&options, &running) {
        return Ok(shared(&options, &other));
    }
    if running.live {
        options.log("extension channel: Live runs its own Extension Host; Kumi's extension runs there once installed (kumi bridge, then restart Live)");
        return Ok(LaunchOutcome::LiveHost);
    }
    let extension = options.extension.clone().or_else(find_extension_bundle);
    let host = find_extension_host(options.live_app.as_deref());
    let Some(extension) = extension.filter(|path| is_extension(path)) else {
        options.log("extension channel: Kumi's Live extension isn't with this bridge");
        return Ok(LaunchOutcome::Unavailable);
    };
    let Some(host) = host else {
        options.log("extension channel: this Live has no Extension Host (Live 12.4 or later has one)");
        return Ok(LaunchOutcome::Unavailable);
    };
    let lock = take_lock(&options.lock_path.clone().unwrap_or_else(|| std::env::temp_dir().join("kumi-extension-launch.lock")));
    let deadline = kumi_common::time::now_ms() as f64 + options.wait_ms.unwrap_or(15000.0);
    let Some(_lock) = lock else {
        while (kumi_common::time::now_ms() as f64) < deadline {
            if read_extension_endpoint(&options.storage_directory).is_some() {
                return Ok(LaunchOutcome::Answering);
            }
            if let Some(other) = find_shared(&options, &options.look().await) {
                return Ok(shared(&options, &other));
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        return Ok(LaunchOutcome::Failed);
    };
    ensure_secret(&options.storage_directory).map_err(io)?;
    let temp = options.storage_directory.join("tmp");
    mkdir(&temp).map_err(io)?;
    let slash = |path: &Path| path.to_string_lossy().replace('\\', "/");
    let config = json!({"extensions":[{"path":slash(&extension),"storageDirectory":slash(&options.storage_directory),"tempDirectory":slash(&temp)}]});
    let log = open_options().create(true).append(true).open(options.storage_directory.join("extension-host.log")).map_err(io)?;
    let script=["globalThis.__kumiLaunchedHost = true;","const config = JSON.parse(process.argv[1]); const endpoint = require('path').join(config.extensions[0].storageDirectory, 'endpoint.json');","setTimeout(() => { try { if (JSON.parse(require('fs').readFileSync(endpoint, 'utf8')).pid === process.pid) return; } catch {} process.exit(3); }, 20000).unref();","require(process.argv[2]).initialize(config);"].join(" ");
    let mut command = Command::new(&host.node);
    command
        .arg("-e")
        .arg(script)
        .arg(kumi_common::js::json::stringify(&config))
        .arg(&host.module)
        .arg(format!("kumi-storage:{}", URL_SAFE_NO_PAD.encode(options.storage_directory.to_string_lossy().as_bytes())))
        .env("KUMI_LAUNCHED_HOST", "1")
        .stdin(std::process::Stdio::null())
        .stdout(log.try_clone().map_err(io)?)
        .stderr(log);
    #[cfg(unix)]
    {
        unsafe {
            command.pre_exec(|| {
                if libc::setsid() < 0 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
    }
    #[cfg(windows)]
    {
        command.creation_flags(0x00000008 | 0x00000200 | 0x08000000);
        keep_std_handles_from_children();
    }
    let mut child = command.spawn().map_err(io)?;
    let pid = child.id();
    options.log(&format!(
        "extension channel: started Live's Extension Host (pid {}) with {}",
        pid.map(|v| v.to_string()).unwrap_or_else(|| "undefined".into()),
        extension.display()
    ));
    while (kumi_common::time::now_ms() as f64) < deadline {
        if read_extension_endpoint(&options.storage_directory).is_some_and(|endpoint| endpoint["pid"].as_u64() == pid.map(u64::from)) {
            return Ok(LaunchOutcome::Started);
        }
        // Stopped by a signal too (no exit code): it's gone, and its pid may be reaped and reused.
        if let Some(status) = child.try_wait().map_err(io)? {
            options.log(&format!("extension channel: the Extension Host stopped ({status})"));
            return Ok(LaunchOutcome::Failed);
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    options.log("extension channel: the Extension Host didn't reach Live in time; stopping it");
    #[cfg(unix)]
    if let Some(pid) = pid {
        unsafe {
            libc::kill(pid as i32, libc::SIGTERM);
        }
    }
    #[cfg(not(unix))]
    let _ = child.start_kill();
    Ok(LaunchOutcome::Failed)
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    #[test]
    fn ps_is_named_by_its_path_and_only_this_users_processes_count() {
        assert_eq!(ps(), "/bin/ps");
        // SAFETY: getuid has no preconditions.
        let uid = unsafe { libc::getuid() };
        let listing = format!(
            "  {uid} /Applications/Live.app/Contents/MacOS/Live\n  {} /Users/other/Live.app/Contents/MacOS/Live\n{uid}   ExtensionHost/node -e x\n",
            uid + 1
        );
        assert_eq!(own_lines(&listing), "/Applications/Live.app/Contents/MacOS/Live\nExtensionHost/node -e x");
    }
}
