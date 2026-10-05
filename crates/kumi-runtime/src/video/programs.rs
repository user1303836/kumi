//! The programs watching a video needs: yt-dlp (what a video page holds: captions, chapters, its
//! streams), ffmpeg (a frame, or a stretch of sound, from a stream) and, for a video without
//! captions, whisper.cpp (its speech, transcribed on this computer) with a speech model. yt-dlp,
//! the speech model and (on Windows and Linux) whisper.cpp and ffmpeg are fetched into Kumi's own
//! folder the first time they're needed, each checked against the checksum its publisher lists; on a
//! Mac, ffmpeg and whisper.cpp are the producer's (Homebrew), and audio formats are read with afconvert.

use std::collections::HashMap;
use std::ffi::OsStr;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, UNIX_EPOCH};

use futures::future::{BoxFuture, Shared};
use futures::{FutureExt, StreamExt};
use kumi_common::abort::{Aborted, Signal, SignalExt};
use kumi_common::js::number::{parse, round, to_string};
use kumi_common::js::string::{head, trim};
use kumi_common::time::now_ms;
use regex::Regex;
use serde_json::Value;
use sha2::{Digest, Sha256};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
use tokio::process::Command;

use crate::core::disk::{low_disk, low_disk_with, MB};
use crate::system::{platform, process_env, system_program_default, Env, SystemProgram};

/// A failure watching a video, said so the producer knows what to do.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct VideoError(pub String);

/// What a step with these programs fails with: a `VideoError` (in Kumi's words), any other error
/// (its message, as `error.message` read), or the signal's stop.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum VideoFailure {
    #[error("{0}")]
    Video(VideoError),
    #[error("{0}")]
    Other(String),
    #[error("This operation was aborted")]
    Aborted,
}

impl VideoFailure {
    pub fn video(message: impl Into<String>) -> Self {
        Self::Video(VideoError(message.into()))
    }

    pub fn other(message: impl Into<String>) -> Self {
        Self::Other(message.into())
    }

    /// `error.message`.
    pub fn message(&self) -> String {
        self.to_string()
    }

    /// `error instanceof VideoError`.
    pub fn video_error(&self) -> Option<&VideoError> {
        match self {
            Self::Video(error) => Some(error),
            _ => None,
        }
    }

    pub fn is_aborted(&self) -> bool {
        matches!(self, Self::Aborted)
    }
}

impl From<VideoError> for VideoFailure {
    fn from(error: VideoError) -> Self {
        Self::Video(error)
    }
}

impl From<Aborted> for VideoFailure {
    fn from(_: Aborted) -> Self {
        Self::Aborted
    }
}

impl From<std::io::Error> for VideoFailure {
    fn from(error: std::io::Error) -> Self {
        Self::Other(error.to_string())
    }
}

impl From<reqwest::Error> for VideoFailure {
    fn from(error: reqwest::Error) -> Self {
        Self::Other(error.to_string())
    }
}

/// `(message: string) => void`: told once when Kumi fetches a program.
pub type OnFetch = Arc<dyn Fn(&str) + Send + Sync>;
/// `(fraction: number) => void`: how far a fetch is, 0–1.
pub type OnProgress = Arc<dyn Fn(f64) + Send + Sync>;
/// `(url, signal) => Promise<Uint8Array>`: for tests, fetch instead of the network.
pub type Download = Arc<dyn Fn(String, Option<Signal>) -> BoxFuture<'static, Result<Vec<u8>, VideoFailure>> + Send + Sync>;
/// `(path) => Promise<number | undefined>`: for tests, free space on a disk.
pub type Free = Arc<dyn Fn(String) -> BoxFuture<'static, Option<f64>> + Send + Sync>;

#[derive(Debug, Clone, Default)]
pub struct RunOptions {
    pub signal: Option<Signal>,
    pub timeout_ms: Option<u64>,
    pub max_buffer: Option<usize>,
}

/// What a program wrote: its stdout as bytes (text when it was text), its stderr as text.
// TS: `encoding: "utf8" | "buffer"` chose the stdout type; here stdout is always bytes, `stdout_text()` the utf8 reading.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RunOutput {
    pub stdout: Vec<u8>,
    pub stderr: String,
}

impl RunOutput {
    /// stdout as utf8 (`encoding: "utf8"`).
    pub fn stdout_text(&self) -> String {
        String::from_utf8_lossy(&self.stdout).into_owned()
    }
}

enum Outcome {
    Done(Result<std::process::ExitStatus, String>),
    TimedOut,
    Aborted,
}

async fn cancelled(signal: Option<Signal>) {
    match signal {
        Some(signal) => signal.cancelled().await,
        None => std::future::pending::<()>().await,
    }
}

/// `future`, unless the signal fires first.
async fn with_signal<T>(signal: &Option<Signal>, future: impl Future<Output = T>) -> Result<T, VideoFailure> {
    tokio::select! {
        value = future => Ok(value),
        _ = cancelled(signal.clone()) => Err(VideoFailure::Aborted),
    }
}

async fn read_some<R: AsyncRead + Unpin>(reader: &mut Option<R>, buffer: &mut [u8]) -> Option<usize> {
    match reader {
        Some(reader) => reader.read(buffer).await.ok(),
        None => None,
    }
}

enum Read {
    Out(Option<usize>),
    Err(Option<usize>),
}

/// Both pipes read until they close; `Err` when one grows past `max` (Node's maxBuffer).
async fn collect<O: AsyncRead + Unpin, E: AsyncRead + Unpin>(
    stdout: &mut Option<O>,
    stderr: &mut Option<E>,
    out: &mut Vec<u8>,
    err: &mut Vec<u8>,
    max: usize,
) -> Result<(), String> {
    let mut buffer_out = vec![0u8; 16384];
    let mut buffer_err = vec![0u8; 16384];
    loop {
        let out_open = stdout.is_some();
        let err_open = stderr.is_some();
        if !out_open && !err_open {
            return Ok(());
        }
        let read = tokio::select! {
            read = read_some(stdout, &mut buffer_out), if out_open => Read::Out(read),
            read = read_some(stderr, &mut buffer_err), if err_open => Read::Err(read),
        };
        match read {
            Read::Out(Some(count)) if count > 0 => {
                out.extend_from_slice(&buffer_out[..count]);
                if out.len() > max {
                    return Err("stdout maxBuffer length exceeded".to_string());
                }
            }
            Read::Out(_) => *stdout = None,
            Read::Err(Some(count)) if count > 0 => {
                err.extend_from_slice(&buffer_err[..count]);
                if err.len() > max {
                    return Err("stderr maxBuffer length exceeded".to_string());
                }
            }
            Read::Err(_) => *stderr = None,
        }
    }
}

/// `spawn <file> ENOENT`: how Node says a program couldn't start.
fn spawn_message(command: &str, error: &std::io::Error) -> String {
    let code = match error.kind() {
        std::io::ErrorKind::NotFound => "ENOENT".to_string(),
        std::io::ErrorKind::PermissionDenied => "EACCES".to_string(),
        _ => error.to_string(),
    };
    format!("spawn {command} {code}")
}

/// Run a program without a shell, bounded; rejects with its last line of stderr.
pub async fn run<S: AsRef<OsStr>>(command: &str, args: &[S], options: RunOptions) -> Result<RunOutput, VideoFailure> {
    let timeout = Duration::from_millis(options.timeout_ms.unwrap_or(120_000));
    let max_buffer = options.max_buffer.unwrap_or(64 * 1024 * 1024);
    let mut shown = command.to_string();
    for arg in args {
        shown.push(' ');
        shown.push_str(&arg.as_ref().to_string_lossy());
    }
    let mut spawning = Command::new(command);
    spawning.args(args).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).kill_on_drop(true);
    #[cfg(windows)]
    {
        // windowsHide: no console window flashes up for the program.
        spawning.creation_flags(0x0800_0000);
    }
    let mut child = spawning.spawn().map_err(|error| VideoFailure::other(spawn_message(command, &error)))?;
    let mut stdout = child.stdout.take();
    let mut stderr = child.stderr.take();
    let mut out = Vec::new();
    let mut err = Vec::new();
    let outcome = {
        let work = async {
            match collect(&mut stdout, &mut stderr, &mut out, &mut err, max_buffer).await {
                Err(exceeded) => Err(exceeded),
                Ok(()) => child.wait().await.map_err(|error| error.to_string()),
            }
        };
        tokio::pin!(work);
        tokio::select! {
            result = &mut work => Outcome::Done(result),
            _ = tokio::time::sleep(timeout) => Outcome::TimedOut,
            _ = cancelled(options.signal.clone()) => Outcome::Aborted,
        }
    };
    let stderr_text = String::from_utf8_lossy(&err).into_owned();
    let failed = |message: String| {
        let last = trim(&stderr_text).split('\n').filter(|line| !line.is_empty()).next_back();
        VideoFailure::other(match last {
            Some(last) => head(last, 400),
            None => message,
        })
    };
    match outcome {
        Outcome::Done(Ok(status)) if status.success() => Ok(RunOutput { stdout: out, stderr: stderr_text }),
        Outcome::Done(Ok(_)) => Err(failed(format!("Command failed: {shown}\n{stderr_text}"))),
        Outcome::Done(Err(message)) => {
            let _ = child.start_kill();
            let _ = child.wait().await;
            Err(failed(message))
        }
        Outcome::TimedOut => {
            let _ = child.start_kill();
            let _ = child.wait().await;
            Err(failed(format!("Command failed: {shown}\n{stderr_text}")))
        }
        Outcome::Aborted => {
            let _ = child.start_kill();
            let _ = child.wait().await;
            Err(VideoFailure::Aborted)
        }
    }
}

/// `process.arch`: Node's name for this computer's processor ("arm64", "x64", "ia32").
pub fn node_arch() -> &'static str {
    match std::env::consts::ARCH {
        "aarch64" => "arm64",
        "x86_64" => "x64",
        "x86" => "ia32",
        other => other,
    }
}

/// The release of yt-dlp for this computer; None where yt-dlp publishes none. A Mac and Windows
/// get the unpacked build, a zip, which starts in a moment (the single file unpacks itself each run).
pub fn yt_dlp_asset(platform: &str, arch: &str) -> Option<&'static str> {
    match platform {
        "darwin" => Some("yt-dlp_macos.zip"),
        "win32" => Some(match arch {
            "arm64" => "yt-dlp_win_arm64.zip",
            "ia32" => "yt-dlp_win_x86.zip",
            _ => "yt-dlp_win.zip",
        }),
        "linux" => match arch {
            "arm64" => Some("yt-dlp_linux_aarch64"),
            "x64" => Some("yt-dlp_linux"),
            _ => None,
        },
        _ => None,
    }
}

const RELEASES: &str = "https://github.com/yt-dlp/yt-dlp/releases/latest/download";
/// YouTube changes often, and yt-dlp with it: Kumi's copy is fetched afresh once it's this old.
const YTDLP_FRESH_MS: f64 = 30.0 * 24.0 * 60.0 * 60_000.0;

/// Whether `command` runs (asked for its version).
async fn runs(command: &str, version_arg: &str, signal: Option<Signal>) -> bool {
    run(command, &[version_arg], RunOptions { timeout_ms: Some(20_000), signal, max_buffer: None }).await.is_ok()
}

#[derive(Clone, Default)]
pub struct ProgramOptions {
    pub env: Option<Env>,
    /// Where Kumi keeps programs it fetched itself (~/.kumi/tools).
    pub tools_dir: String,
    pub signal: Option<Signal>,
    /// Told once when Kumi fetches yt-dlp, so the wait is explained.
    pub on_fetch: Option<OnFetch>,
    /// How far a fetch is, 0–1, as it downloads.
    pub on_progress: Option<OnProgress>,
    /// What the program is for, in the fetch's notice ("to write down what you say"); a video's use when left out.
    pub purpose: Option<String>,
    /// For tests: fetch, instead of the network.
    pub download: Option<Download>,
    /// Only look for what's there: fetch nothing (for the doctor).
    pub installed_only: bool,
    /// For tests: free space on a disk.
    pub free: Option<Free>,
}

// GitHub's API answers 403 to a request without a User-Agent (Node's fetch sent "node").
static CLIENT: LazyLock<reqwest::Client> =
    LazyLock::new(|| reqwest::Client::builder().user_agent(format!("kumi/{}", crate::KUMI_VERSION)).build().expect("an HTTP client"));

fn last_segment(url: &str) -> &str {
    url.rsplit('/').next().unwrap_or(url)
}

async fn download(url: &str, signal: Option<Signal>) -> Result<Vec<u8>, VideoFailure> {
    let response = with_signal(&signal, CLIENT.get(url).send()).await??;
    if !response.status().is_success() {
        return Err(VideoFailure::video(format!("Downloading {} failed ({}).", last_segment(url), response.status().as_u16())));
    }
    let bytes = with_signal(&signal, response.bytes()).await??;
    Ok(bytes.to_vec())
}

/// `(options.download ?? download)(url, signal)`.
async fn fetch_bytes(download_with: &Option<Download>, url: &str, signal: &Option<Signal>) -> Result<Vec<u8>, VideoFailure> {
    match download_with {
        Some(download_with) => download_with(url.to_string(), signal.clone()).await,
        None => download(url, signal.clone()).await,
    }
}

/// `new TextDecoder().decode(bytes)`: utf8, a byte order mark dropped.
fn decode_text(bytes: &[u8]) -> String {
    let text = String::from_utf8_lossy(bytes);
    text.strip_prefix('\u{FEFF}').map(str::to_string).unwrap_or_else(|| text.into_owned())
}

fn exists(path: impl AsRef<Path>) -> bool {
    path.as_ref().exists()
}

fn join(base: impl AsRef<Path>, parts: &[&str]) -> String {
    let mut path = base.as_ref().to_path_buf();
    for part in parts {
        path.push(part);
    }
    path.to_string_lossy().into_owned()
}

fn dirname(path: &str) -> PathBuf {
    Path::new(path).parent().map(Path::to_path_buf).unwrap_or_else(|| PathBuf::from("."))
}

async fn mkdir_700(path: impl AsRef<Path>) -> std::io::Result<()> {
    let mut builder = tokio::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    builder.mode(0o700);
    builder.create(path).await
}

async fn chmod_755(path: impl AsRef<Path>) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        tokio::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).await
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        Ok(())
    }
}

/// `rm(path, { force: true })`.
async fn remove_file_force(path: impl AsRef<Path>) {
    let _ = tokio::fs::remove_file(path).await;
}

/// `rm(path, { recursive: true, force: true })`.
async fn remove_dir_force(path: impl AsRef<Path>) -> std::io::Result<()> {
    match tokio::fs::remove_dir_all(path).await {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

/// A file opened for writing with mode 0o600.
async fn create_600(path: impl AsRef<Path>) -> std::io::Result<tokio::fs::File> {
    let mut options = tokio::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    options.mode(0o600);
    options.open(path).await
}

/// `readdir(folder).catch(() => [])`: the names in a folder, none when it can't be read.
async fn names_in(folder: impl AsRef<Path>) -> Vec<String> {
    let mut names = Vec::new();
    if let Ok(mut entries) = tokio::fs::read_dir(folder).await {
        while let Ok(Some(entry)) = entries.next_entry().await {
            names.push(entry.file_name().to_string_lossy().into_owned());
        }
    }
    names
}

/// `readdir(folder, { recursive: true })`: every path under `folder`, relative to it.
async fn names_under(folder: &Path) -> std::io::Result<Vec<String>> {
    let mut found = Vec::new();
    let mut pending = vec![PathBuf::new()];
    while let Some(relative) = pending.pop() {
        let mut entries = tokio::fs::read_dir(folder.join(&relative)).await?;
        let mut here = Vec::new();
        while let Some(entry) = entries.next_entry().await? {
            let path = relative.join(entry.file_name());
            if entry.file_type().await.map(|kind| kind.is_dir()).unwrap_or(false) {
                here.push(path.clone());
            }
            found.push(path.to_string_lossy().into_owned());
        }
        here.reverse();
        pending.extend(here);
    }
    Ok(found)
}

fn path_parts(name: &str) -> Vec<&str> {
    name.split(['\\', '/']).collect()
}

/// The yt-dlp program in Kumi's folder, when it fetched one.
async fn own_yt_dlp(folder: &str) -> Option<String> {
    static NAME: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^yt-dlp[\w.-]*$").unwrap());
    let names = names_in(folder).await;
    let name = names
        .into_iter()
        .find(|entry| NAME.is_match(entry) && !entry.ends_with(".zip") && (platform() != "win32" || entry.ends_with(".exe")))?;
    Some(join(folder, &[&name]))
}

fn mtime_ms(path: &str) -> f64 {
    std::fs::metadata(path)
        .and_then(|meta| meta.modified())
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map(|since| since.as_secs_f64() * 1000.0)
        .unwrap_or(0.0)
}

/// Where the yt-dlp fetch leaves its pieces; `None` for a download the release doesn't list.
async fn fetch_yt_dlp(asset: &str, folder: &str, options: &ProgramOptions) -> Result<String, VideoFailure> {
    let sums = fetch_bytes(&options.download, &format!("{RELEASES}/SHA2-256SUMS"), &options.signal).await?;
    let expected = decode_text(&sums).split('\n').find_map(|line| {
        let parts: Vec<&str> = trim(line).split_whitespace().collect();
        (parts.get(1) == Some(&asset)).then(|| parts.first().map(|sum| sum.to_string())).flatten()
    });
    mkdir_700(&options.tools_dir).await?;
    let fetched = join(&options.tools_dir, &[&format!(".{asset}-{}", uuid::Uuid::new_v4())]);
    let unpacked = join(&options.tools_dir, &[&format!(".yt-dlp-{}", uuid::Uuid::new_v4())]);
    let result = async {
        let matched = match &expected {
            Some(expected) => {
                download_to(
                    &format!("{RELEASES}/{asset}"),
                    Path::new(&fetched),
                    Fetching {
                        download: options.download.as_ref(),
                        signal: options.signal.clone(),
                        on_progress: options.on_progress.as_ref(),
                    },
                    None,
                )
                .await?
                    == expected.to_lowercase()
            }
            None => false,
        };
        if !matched {
            return Err(VideoFailure::video("The yt-dlp Kumi downloaded didn't match its release's checksum, so it wasn't kept."));
        }
        mkdir_700(&unpacked).await?;
        // tar reads zip archives too (Windows has had it since 2018).
        if asset.ends_with(".zip") {
            run(
                &system_program_default(SystemProgram::Tar),
                &["-xf", &fetched, "-C", &unpacked],
                RunOptions { timeout_ms: Some(120_000), signal: options.signal.clone(), max_buffer: None },
            )
            .await?;
        } else {
            tokio::fs::rename(&fetched, join(&unpacked, &[if platform() == "win32" { "yt-dlp.exe" } else { "yt-dlp" }])).await?;
        }
        let program = own_yt_dlp(&unpacked).await.ok_or_else(|| VideoFailure::video("The yt-dlp Kumi downloaded had no program in it."))?;
        if platform() != "win32" {
            chmod_755(&program).await?;
        }
        remove_dir_force(folder).await?;
        tokio::fs::rename(&unpacked, folder).await?;
        Ok(own_yt_dlp(folder).await.expect("the program just unpacked"))
    }
    .await;
    remove_file_force(&fetched).await;
    let _ = remove_dir_force(&unpacked).await;
    result
}

/// yt-dlp: KUMI_YTDLP, then the copy Kumi fetched (fetched afresh once a month), then one on the
/// PATH; failing those, the release for this computer is fetched into `tools_dir` (checked against the
/// release's SHA2-256SUMS).
pub async fn find_yt_dlp(options: &ProgramOptions) -> Result<String, VideoFailure> {
    let env = options.env.clone().unwrap_or_else(process_env);
    if let Some(named) = env.get("KUMI_YTDLP").filter(|value| !value.is_empty()) {
        if !exists(named) {
            return Err(VideoFailure::video(format!("KUMI_YTDLP names {named}, which isn't there.")));
        }
        return Ok(named.clone());
    }
    let folder = join(&options.tools_dir, &["yt-dlp"]);
    let own = own_yt_dlp(&folder).await;
    if let Some(own) = &own {
        if now_ms() as f64 - mtime_ms(&folder) < YTDLP_FRESH_MS {
            return Ok(own.clone());
        }
    }
    let installed = if own.is_some() { None } else { on_path(if platform() == "win32" { "yt-dlp.exe" } else { "yt-dlp" }, &env) };
    if let Some(installed) = installed {
        return Ok(installed);
    }
    let Some(asset) = yt_dlp_asset(platform(), node_arch()) else {
        if let Some(own) = own {
            return Ok(own);
        }
        return Err(VideoFailure::video(
            "Kumi can't fetch yt-dlp for this computer; install it (https://github.com/yt-dlp/yt-dlp) and try again.",
        ));
    };
    if let Some(on_fetch) = &options.on_fetch {
        on_fetch(if own.is_some() {
            "Kumi is updating yt-dlp, the program it reads videos with (once a month)."
        } else {
            "Kumi is fetching yt-dlp, the program it reads videos with (once, about 35 MB)."
        });
    }
    match fetch_yt_dlp(asset, &folder, options).await {
        Ok(program) => Ok(program),
        Err(error) => {
            // Offline, say: the copy Kumi has still works for most videos.
            if let Some(signal) = &options.signal {
                signal.check()?;
            }
            if let Some(own) = own {
                return Ok(own);
            }
            Err(error)
        }
    }
}

mod node_runtime;

type Extras = Shared<BoxFuture<'static, Vec<String>>>;
static RUNTIMES: LazyLock<Mutex<HashMap<String, Extras>>> = LazyLock::new(|| Mutex::new(HashMap::new()));

async fn probe_runtimes(ytdlp: String, signal: Option<Signal>) -> Vec<String> {
    let Ok(output) = run(&ytdlp, &["--version"], RunOptions { timeout_ms: Some(60_000), signal, max_buffer: None }).await else {
        return Vec::new();
    };
    let text = output.stdout_text();
    let mut parts = trim(&text).split('.').map(|part| parse(part).unwrap_or(f64::NAN));
    let year = parts.next().unwrap_or(0.0);
    let month = parts.next().unwrap_or(0.0);
    if !(year > 2025.0 || (year == 2025.0 && month >= 11.0)) {
        return Vec::new();
    }
    // Preserve the old installed app’s process.execPath before considering a system runtime.
    match node_runtime::find_node(&process_env(), &home::home_dir().unwrap_or_default(), platform()) {
        Some(node) => vec!["--js-runtimes".to_string(), format!("node:{node}")],
        None => Vec::new(),
    }
}

/// What yt-dlp is told besides: YouTube's pages need JavaScript run to give their streams, and
/// yt-dlp (from 2025.11) can run it with Node.
pub async fn yt_dlp_extras(ytdlp: &str, signal: Option<Signal>) -> Vec<String> {
    let extras = {
        let mut runtimes = RUNTIMES.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        runtimes.entry(ytdlp.to_string()).or_insert_with(|| probe_runtimes(ytdlp.to_string(), signal).boxed().shared()).clone()
    };
    extras.await
}

type Pieces = Shared<BoxFuture<'static, bool>>;
static PIECES: LazyLock<Mutex<HashMap<String, Pieces>>> = LazyLock::new(|| Mutex::new(HashMap::new()));

async fn probe_pieces(ffmpeg: String) -> bool {
    match run(&ffmpeg, &["-hide_banner", "-h", "protocol=http"], RunOptions { timeout_ms: Some(20_000), ..Default::default() }).await {
        Ok(output) => output.stdout_text().contains("-request_size"),
        Err(_) => false,
    }
}

/// Whether this ffmpeg can ask for a stream a piece at a time (`-request_size`, from ffmpeg 8.1).
/// YouTube wants its streams asked for that way: one asked for whole slows to a trickle or is refused.
pub async fn ffmpeg_reads_in_pieces(ffmpeg: &str, signal: Option<Signal>) -> Result<bool, VideoFailure> {
    let pieces = {
        let mut known = PIECES.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        known.entry(ffmpeg.to_string()).or_insert_with(|| probe_pieces(ffmpeg.to_string()).boxed().shared()).clone()
    };
    with_signal(&signal, pieces).await
}

/// Where Kumi keeps the programs it fetches, and who's told when it fetches one: set once as Kumi starts.
#[derive(Clone, Default)]
pub struct ProgramDefaults {
    pub tools_dir: Option<String>,
    pub on_fetch: Option<OnFetch>,
}

static PROGRAM_DEFAULTS: LazyLock<Mutex<ProgramDefaults>> = LazyLock::new(|| Mutex::new(ProgramDefaults::default()));

pub fn configure_programs(options: ProgramDefaults) {
    *PROGRAM_DEFAULTS.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = options;
}

fn program_defaults() -> ProgramDefaults {
    PROGRAM_DEFAULTS.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).clone()
}

/// The ffmpeg build for this computer among a release's files: the newest numbered LGPL one; a Mac has none.
pub fn ffmpeg_asset(names: &[String], platform: &str, arch: &str) -> Option<String> {
    let target = match platform {
        "win32" => match arch {
            "arm64" => Some("winarm64"),
            "x64" => Some("win64"),
            _ => None,
        },
        "linux" => match arch {
            "arm64" => Some("linuxarm64"),
            "x64" => Some("linux64"),
            _ => None,
        },
        _ => None,
    }?;
    let pattern = Regex::new(&format!(r"^ffmpeg-n([0-9]+)\.([0-9]+)-latest-{target}-lgpl-[0-9]+\.[0-9]+\.(zip|tar\.xz)$")).unwrap();
    let mut versions: Vec<(&String, f64, f64)> = names
        .iter()
        .filter_map(|name| {
            let found = pattern.captures(name)?;
            Some((name, parse(&found[1]).unwrap_or(f64::NAN), parse(&found[2]).unwrap_or(f64::NAN)))
        })
        .collect();
    versions.sort_by(|a, b| {
        b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal).then(b.2.partial_cmp(&a.2).unwrap_or(std::cmp::Ordering::Equal))
    });
    versions.first().map(|(name, _, _)| (*name).clone())
}

const FFMPEG_RELEASE: &str = "https://api.github.com/repos/BtbN/FFmpeg-Builds/releases/latest";

#[derive(Clone, Default)]
pub struct FfmpegOptions {
    pub env: Option<Env>,
    pub signal: Option<Signal>,
    /// Where Kumi keeps programs it fetched (~/.kumi/tools); set by configure_programs when left out.
    pub tools_dir: Option<String>,
    pub on_fetch: Option<OnFetch>,
    /// How far a fetch is, 0–1, as it downloads.
    pub on_progress: Option<OnProgress>,
    /// What ffmpeg is for, in the fetch's notice; reading audio and videos when left out.
    pub purpose: Option<String>,
    /// Only look for what's there: fetch nothing (for the doctor).
    pub installed_only: bool,
    /// For tests: the network, the computer, and its free disk.
    pub download: Option<Download>,
    pub platform: Option<String>,
    pub arch: Option<String>,
    pub free: Option<Free>,
}

/// `lowDisk(path, needed, what, options.free)`.
async fn low_disk_for(path: &str, needed: f64, what: &str, free: &Option<Free>) -> Option<String> {
    match free {
        Some(free) => {
            let free = free.clone();
            low_disk_with(path, needed, what, move |at| free(at)).await
        }
        None => low_disk(path, needed, what).await,
    }
}

fn string_of(value: &Value, key: &str) -> Option<String> {
    value.get(key).and_then(Value::as_str).map(str::to_string)
}

static DIGEST: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)^sha256:([0-9a-f]{64})$").unwrap());

/// ffmpeg: KUMI_FFMPEG, one on the PATH, where Homebrew and the usual installers put it, or the copy
/// Kumi fetched; failing those, on Windows and Linux, a release build is fetched into `tools_dir`
/// (checked against the SHA-256 GitHub lists for it). A Mac reads audio with afconvert; its ffmpeg
/// (for a video's frames) comes from Homebrew. None when there's none.
pub async fn find_ffmpeg(options: FfmpegOptions) -> Result<Option<String>, VideoFailure> {
    let env = options.env.clone().unwrap_or_else(process_env);
    if let Some(named) = env.get("KUMI_FFMPEG").filter(|value| !value.is_empty()) {
        return Ok(if exists(named) { Some(named.clone()) } else { None });
    }
    let platform_name = options.platform.clone().unwrap_or_else(|| platform().to_string());
    let program = if platform_name == "win32" { "ffmpeg.exe" } else { "ffmpeg" };
    let tools_dir = options.tools_dir.clone().or_else(|| program_defaults().tools_dir);
    let own = tools_dir.as_ref().map(|dir| join(dir, &["ffmpeg", program]));
    if let Some(own) = &own {
        if exists(own) {
            return Ok(Some(own.clone()));
        }
    }
    if options.platform.is_none() {
        if runs("ffmpeg", "-version", options.signal.clone()).await {
            return Ok(Some("ffmpeg".to_string()));
        }
        let candidates: Vec<String> = if platform_name == "win32" {
            vec![
                join(env.get("ProgramFiles").map(String::as_str).unwrap_or("C:\\Program Files"), &["ffmpeg", "bin", "ffmpeg.exe"]),
                join(env.get("LOCALAPPDATA").map(String::as_str).unwrap_or(""), &["Microsoft", "WinGet", "Links", "ffmpeg.exe"]),
            ]
        } else {
            vec!["/opt/homebrew/bin/ffmpeg".to_string(), "/usr/local/bin/ffmpeg".to_string(), "/usr/bin/ffmpeg".to_string()]
        };
        for candidate in candidates {
            if !candidate.is_empty() && exists(&candidate) && runs(&candidate, "-version", options.signal.clone()).await {
                return Ok(Some(candidate));
            }
        }
    }
    let (Some(tools_dir), Some(own)) = (tools_dir, own) else { return Ok(None) };
    if options.installed_only {
        return Ok(None);
    }
    let release: Value = match fetch_bytes(&options.download, FFMPEG_RELEASE, &options.signal).await {
        Ok(bytes) => match serde_json::from_str(&decode_text(&bytes)) {
            Ok(release) => release,
            Err(_) => {
                if let Some(signal) = &options.signal {
                    signal.check()?;
                }
                return Ok(None);
            }
        },
        Err(_) => {
            if let Some(signal) = &options.signal {
                signal.check()?;
            }
            return Ok(None);
        }
    };
    let assets: Vec<&Value> = release
        .get("assets")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter(|item| string_of(item, "browser_download_url").is_some_and(|url| url.starts_with("https://github.com/")))
                .collect()
        })
        .unwrap_or_default();
    let names: Vec<String> = assets.iter().map(|item| string_of(item, "name").unwrap_or_default()).collect();
    let asset = ffmpeg_asset(&names, &platform_name, options.arch.as_deref().unwrap_or(node_arch()));
    let published = asset.as_ref().and_then(|asset| assets.iter().find(|item| string_of(item, "name").as_ref() == Some(asset)));
    let expected =
        published.and_then(|item| string_of(item, "digest")).and_then(|digest| DIGEST.captures(&digest).map(|found| found[1].to_string()));
    let (Some(asset), Some(published), Some(expected)) = (asset, published, expected) else { return Ok(None) };
    let size = published.get("size").and_then(Value::as_f64);
    // The archive, and the program unpacked from it, side by side for a moment.
    let full = low_disk_for(&tools_dir, 2.0 * size.unwrap_or(200.0 * MB) + 100.0 * MB, "Kumi keeps its programs on", &options.free).await;
    if let Some(full) = full {
        return Err(VideoFailure::video(format!("Kumi needs ffmpeg for this, and would fetch it. {full}")));
    }
    if let Some(on_fetch) = options.on_fetch.clone().or_else(|| program_defaults().on_fetch) {
        on_fetch(&format!(
            "Kumi is fetching ffmpeg, {} (once, about {} MB).",
            options.purpose.as_deref().unwrap_or("which it reads audio formats and videos with"),
            to_string(round(size.unwrap_or(0.0) / 1e6))
        ));
    }
    let url = string_of(published, "browser_download_url").expect("a GitHub address");
    let archive =
        join(&tools_dir, &[&format!(".ffmpeg-{}{}", uuid::Uuid::new_v4(), if asset.ends_with(".zip") { ".zip" } else { ".tar.xz" })]);
    let unpacked = join(&tools_dir, &[&format!(".ffmpeg-{}", uuid::Uuid::new_v4())]);
    let result = async {
        let digest = download_to(
            &url,
            Path::new(&archive),
            Fetching { download: options.download.as_ref(), signal: options.signal.clone(), on_progress: options.on_progress.as_ref() },
            size,
        )
        .await?;
        if digest != expected.to_lowercase() {
            return Err(VideoFailure::video("The ffmpeg Kumi downloaded didn't match its release's checksum, so it wasn't kept."));
        }
        mkdir_700(&unpacked).await?;
        // tar reads zip archives too (Windows has had it since 2018), and xz ones.
        run(
            &system_program_default(SystemProgram::Tar),
            &["-xf", &archive, "-C", &unpacked],
            RunOptions { timeout_ms: Some(300_000), signal: options.signal.clone(), max_buffer: None },
        )
        .await?;
        let inside = names_under(Path::new(&unpacked)).await?.into_iter().find(|name| {
            let parts = path_parts(name);
            parts.last() == Some(&program) && parts.len() >= 2 && parts[parts.len() - 2] == "bin"
        });
        let Some(inside) = inside else { return Ok(None) };
        // Only the program: the build is static, and the rest (ffprobe, ffplay, docs) isn't needed.
        mkdir_700(dirname(&own)).await?;
        tokio::fs::rename(join(&unpacked, &[&inside]), &own).await?;
        if platform_name != "win32" {
            chmod_755(&own).await?;
        }
        Ok(Some(own.clone()))
    }
    .await;
    remove_file_force(&archive).await;
    let _ = remove_dir_force(&unpacked).await;
    result
}

/// How to get ffmpeg on this computer, for the one line that says frames and sound need it.
pub fn ffmpeg_hint() -> &'static str {
    match platform() {
        "darwin" => "brew install ffmpeg",
        "win32" => "Kumi fetches it the first time it's needed; or winget install ffmpeg",
        _ => "Kumi fetches it the first time it's needed; or your package manager (ffmpeg)",
    }
}

/// A program by name on the PATH, as a full path; None when it isn't there.
fn on_path(name: &str, env: &Env) -> Option<String> {
    let delimiter = if cfg!(windows) { ';' } else { ':' };
    let path = env.get("PATH").or_else(|| env.get("Path")).map(String::as_str).unwrap_or("");
    for folder in path.split(delimiter).filter(|folder| !folder.is_empty()) {
        let candidate = join(folder, &[name]);
        if exists(&candidate) {
            return Some(candidate);
        }
    }
    None
}

/// What a download is fetched with: a test's `download`, the signal, and who's told how far it is.
struct Fetching<'a> {
    download: Option<&'a Download>,
    signal: Option<Signal>,
    on_progress: Option<&'a OnProgress>,
}

/// A download streamed to `path`, with its SHA-256 (hex); nothing is left there on failure. How far it
/// is goes to on_progress, against its length (or `size`, the length its publisher lists).
async fn download_to(url: &str, path: &Path, fetching: Fetching<'_>, size: Option<f64>) -> Result<String, VideoFailure> {
    if let Some(parent) = path.parent() {
        mkdir_700(parent).await?;
    }
    match write_download(url, path, &fetching, size).await {
        Ok(digest) => Ok(digest),
        Err(error) => {
            remove_file_force(path).await;
            Err(error)
        }
    }
}

async fn write_download(url: &str, path: &Path, fetching: &Fetching<'_>, size: Option<f64>) -> Result<String, VideoFailure> {
    let mut hash = Sha256::new();
    if let Some(download_with) = fetching.download {
        let data = download_with(url.to_string(), fetching.signal.clone()).await?;
        hash.update(&data);
        let mut file = create_600(path).await?;
        file.write_all(&data).await?;
        file.flush().await?;
        if let Some(on_progress) = fetching.on_progress {
            on_progress(1.0);
        }
    } else {
        let response = with_signal(&fetching.signal, CLIENT.get(url).send()).await??;
        if !response.status().is_success() {
            return Err(VideoFailure::video(format!("Downloading {} failed ({}).", last_segment(url), response.status().as_u16())));
        }
        let header = response
            .headers()
            .get(reqwest::header::CONTENT_LENGTH)
            .and_then(|value| value.to_str().ok())
            .and_then(parse)
            .filter(|length| *length != 0.0);
        let total = header.or(size.filter(|length| *length != 0.0)).unwrap_or(0.0);
        let mut received = 0.0;
        let mut told = -1.0;
        let mut file = create_600(path).await?;
        let mut stream = response.bytes_stream();
        loop {
            let Some(chunk) = with_signal(&fetching.signal, stream.next()).await? else { break };
            let chunk = chunk?;
            hash.update(&chunk);
            received += chunk.len() as f64;
            // A whole percent at a time: the screen needn't redraw for every packet.
            let percent = if total != 0.0 { (received / total * 100.0).floor().min(100.0) } else { -1.0 };
            if percent > told {
                told = percent;
                if let Some(on_progress) = fetching.on_progress {
                    on_progress(percent / 100.0);
                }
            }
            file.write_all(&chunk).await?;
        }
        file.flush().await?;
    }
    Ok(hex::encode(hash.finalize()))
}

/// whisper.cpp's build for this computer, where its releases have one (a Mac gets it from Homebrew).
pub fn whisper_asset(platform: &str, arch: &str) -> Option<&'static str> {
    match platform {
        "win32" => match arch {
            "arm64" => Some("whisper-bin-win-cpu-arm64.zip"),
            "ia32" => Some("whisper-bin-Win32.zip"),
            "x64" => Some("whisper-bin-x64.zip"),
            _ => None,
        },
        "linux" => match arch {
            "arm64" => Some("whisper-bin-ubuntu-arm64.tar.gz"),
            "x64" => Some("whisper-bin-ubuntu-x64.tar.gz"),
            _ => None,
        },
        _ => None,
    }
}

/// How to get whisper.cpp on this computer, for the line that says transcribing needs it.
pub fn whisper_hint() -> &'static str {
    if platform() == "darwin" {
        "brew install whisper-cpp"
    } else {
        "https://github.com/ggml-org/whisper.cpp"
    }
}

const WHISPER_RELEASES: &str = "https://api.github.com/repos/ggml-org/whisper.cpp/releases?per_page=20";

/// whisper.cpp's command line (whisper-cli): KUMI_WHISPER, the copy Kumi fetched, one on the PATH
/// or where Homebrew puts it; failing those, on Windows and Linux, the latest release's build is
/// fetched into `tools_dir` (checked against the SHA-256 GitHub lists for it). None when there's none.
pub async fn find_whisper(options: &ProgramOptions) -> Result<Option<String>, VideoFailure> {
    let env = options.env.clone().unwrap_or_else(process_env);
    if let Some(named) = env.get("KUMI_WHISPER").filter(|value| !value.is_empty()) {
        return Ok(if exists(named) { Some(named.clone()) } else { None });
    }
    let program = if platform() == "win32" { "whisper-cli.exe" } else { "whisper-cli" };
    let own = join(&options.tools_dir, &["whisper", program]);
    if exists(&own) {
        return Ok(Some(own));
    }
    let found = on_path(program, &env).or_else(|| {
        if platform() == "darwin" {
            ["/opt/homebrew/bin/whisper-cli", "/usr/local/bin/whisper-cli"].into_iter().find(|path| exists(path)).map(str::to_string)
        } else {
            None
        }
    });
    if let Some(found) = found {
        return Ok(Some(found));
    }
    let Some(asset) = whisper_asset(platform(), node_arch()) else { return Ok(None) };
    if options.installed_only {
        return Ok(None);
    }
    let releases: Value = serde_json::from_str(&decode_text(&fetch_bytes(&options.download, WHISPER_RELEASES, &options.signal).await?))
        .map_err(|error| VideoFailure::other(error.to_string()))?;
    let published = releases
        .as_array()
        .map(|releases| {
            releases.iter().flat_map(|release| release.get("assets").and_then(Value::as_array).cloned().unwrap_or_default()).find(|item| {
                string_of(item, "name").as_deref() == Some(asset)
                    && string_of(item, "browser_download_url").is_some_and(|url| url.starts_with("https://github.com/"))
            })
        })
        .unwrap_or(None);
    let expected = published
        .as_ref()
        .and_then(|item| string_of(item, "digest"))
        .and_then(|digest| DIGEST.captures(&digest).map(|found| found[1].to_string()));
    let (Some(published), Some(expected)) = (published, expected) else { return Ok(None) };
    if let Some(on_fetch) = &options.on_fetch {
        on_fetch(&format!(
            "Kumi is fetching whisper.cpp, {} (once).",
            options.purpose.as_deref().unwrap_or("which transcribes a video's speech when it has no captions")
        ));
    }
    let url = string_of(&published, "browser_download_url").expect("a GitHub address");
    let archive = join(
        &options.tools_dir,
        &[&format!(".whisper-{}{}", uuid::Uuid::new_v4(), if asset.ends_with(".zip") { ".zip" } else { ".tar.gz" })],
    );
    let unpacked = join(&options.tools_dir, &[&format!(".whisper-{}", uuid::Uuid::new_v4())]);
    let result = async {
        let digest = download_to(
            &url,
            Path::new(&archive),
            Fetching { download: options.download.as_ref(), signal: options.signal.clone(), on_progress: options.on_progress.as_ref() },
            None,
        )
        .await?;
        if digest != expected.to_lowercase() {
            return Err(VideoFailure::video("The whisper.cpp Kumi downloaded didn't match its release's checksum, so it wasn't kept."));
        }
        mkdir_700(&unpacked).await?;
        // tar reads zip archives too (Windows has had it since 2018).
        run(
            &system_program_default(SystemProgram::Tar),
            &["-xf", &archive, "-C", &unpacked],
            RunOptions { timeout_ms: Some(120_000), signal: options.signal.clone(), max_buffer: None },
        )
        .await?;
        let inside = names_under(Path::new(&unpacked)).await?.into_iter().find(|name| path_parts(name).last() == Some(&program));
        let Some(inside) = inside else { return Ok(None) };
        // The program and its libraries, as they came.
        remove_dir_force(join(&options.tools_dir, &["whisper"])).await?;
        tokio::fs::rename(dirname(&join(&unpacked, &[&inside])), join(&options.tools_dir, &["whisper"])).await?;
        if platform() != "win32" {
            chmod_755(&own).await?;
        }
        Ok(if exists(&own) { Some(own.clone()) } else { None })
    }
    .await;
    remove_file_force(&archive).await;
    let _ = remove_dir_force(&unpacked).await;
    result
}

/// Where whisper.cpp's models are published (Hugging Face): its speech models, and its voice activity model.
const WHISPER_MODELS: &str = "ggerganov/whisper.cpp";
const VAD_MODELS: &str = "ggml-org/whisper-vad";
/// whisper.cpp's voice activity model (Silero, under a megabyte).
pub const VAD_MODEL: &str = "ggml-silero-v6.2.0.bin";

/// A whisper.cpp speech model: KUMI_WHISPER_MODEL, or `name` (such as ggml-small.en-q5_1.bin)
/// fetched once into `tools_dir`, checked against the SHA-256 Hugging Face lists for it.
pub async fn whisper_model(name: &str, options: &ProgramOptions) -> Result<String, VideoFailure> {
    let env = options.env.clone().unwrap_or_else(process_env);
    if let Some(named) = env.get("KUMI_WHISPER_MODEL").filter(|value| !value.is_empty()) {
        if !exists(named) {
            return Err(VideoFailure::video(format!("KUMI_WHISPER_MODEL names {named}, which isn't there.")));
        }
        return Ok(named.clone());
    }
    published_model(WHISPER_MODELS, name, options).await
}

/// whisper.cpp's voice activity model, fetched once beside the speech models, without a word (it's
/// under a megabyte): with it, only stretches of speech are written down, never music or noise.
pub async fn vad_model(options: &ProgramOptions) -> Result<String, VideoFailure> {
    let quiet = ProgramOptions { on_fetch: None, on_progress: None, ..options.clone() };
    published_model(VAD_MODELS, VAD_MODEL, &quiet).await
}

/// `name` from the Hugging Face repository `repo`, fetched once into `tools_dir`, checked against the SHA-256 it lists.
async fn published_model(repo: &str, name: &str, options: &ProgramOptions) -> Result<String, VideoFailure> {
    static MODEL_NAME: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^ggml-[a-z0-9._-]+\.bin$").unwrap());
    static OID: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)^[0-9a-f]{64}$").unwrap());
    if !MODEL_NAME.is_match(name) {
        return Err(VideoFailure::video(format!("{name} isn't a whisper.cpp model's name.")));
    }
    let path = join(&options.tools_dir, &["whisper-models", name]);
    if exists(&path) && std::fs::metadata(&path).map(|meta| meta.len() > 0).unwrap_or(false) {
        return Ok(path);
    }
    let files: Value = serde_json::from_str(&decode_text(
        &fetch_bytes(&options.download, &format!("https://huggingface.co/api/models/{repo}/tree/main"), &options.signal).await?,
    ))
    .map_err(|error| VideoFailure::other(error.to_string()))?;
    let listed = files.as_array().and_then(|files| files.iter().find(|file| string_of(file, "path").as_deref() == Some(name)));
    let expected = listed.and_then(|file| file.get("lfs")).and_then(|lfs| string_of(lfs, "oid")).filter(|oid| OID.is_match(oid));
    let Some(expected) = expected else {
        return Err(VideoFailure::video(format!("Kumi couldn't find the speech model {name} to fetch.")));
    };
    let size = listed.and_then(|file| file.get("size")).and_then(Value::as_f64);
    let full = low_disk_for(&options.tools_dir, size.unwrap_or(200.0 * MB) + 100.0 * MB, "Kumi keeps its programs on", &options.free).await;
    if let Some(full) = full {
        return Err(VideoFailure::video(format!("Transcribing needs a speech model, which Kumi would fetch. {full}")));
    }
    if let Some(on_fetch) = &options.on_fetch {
        on_fetch(&format!(
            "Kumi is fetching a speech model, {} (once, about {} MB).",
            options.purpose.as_deref().unwrap_or("to transcribe videos without captions"),
            to_string(round(size.unwrap_or(0.0) / 1e6))
        ));
    }
    let temporary = dirname(&path).join(format!(".{name}-{}", uuid::Uuid::new_v4()));
    let result = async {
        let digest = download_to(
            &format!("https://huggingface.co/{repo}/resolve/main/{name}"),
            &temporary,
            Fetching { download: options.download.as_ref(), signal: options.signal.clone(), on_progress: options.on_progress.as_ref() },
            size,
        )
        .await?;
        if digest != expected.to_lowercase() {
            return Err(VideoFailure::video("The speech model Kumi downloaded didn't match its checksum, so it wasn't kept."));
        }
        tokio::fs::rename(&temporary, &path).await?;
        Ok(())
    }
    .await;
    remove_file_force(&temporary).await;
    result?;
    Ok(path)
}

#[cfg(test)]
mod user_agent_tests {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    #[tokio::test]
    async fn downloads_say_who_is_asking() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/repos/BtbN/FFmpeg-Builds/releases/latest", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = vec![0; 4096];
            let read = socket.read(&mut request).await.unwrap();
            socket.write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 2\r\nconnection: close\r\n\r\n{}").await.unwrap();
            String::from_utf8_lossy(&request[..read]).to_lowercase()
        });
        super::CLIENT.get(&url).send().await.unwrap();
        let request = server.await.unwrap();
        assert!(request.contains(&format!("user-agent: kumi/{}\r\n", crate::KUMI_VERSION)), "{request}");
    }
}
