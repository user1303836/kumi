//! Installed Kumi and its native release bundles.
mod migration;
pub use crate::config::kumi_dir as kumi_home;
use crate::{
    config::{json_files, load_db_file},
    input::TerminalInput,
    process::{ask_yes_no, Confirm, TerminalAsk},
    spinner::{step, Terminal},
    tui::tty::TtyOutput,
};
use futures::{future::LocalBoxFuture, StreamExt};
use kumi_agent::core::store_import::write_back;
use kumi_ai::{
    error::LanguageModelError,
    http::{default_fetch, Fetch, FetchInit, Headers},
};
use kumi_common::{
    abort,
    js::{json::stringify, string::trim},
    time::now_ms,
};
use kumi_daw::{files::remove, provider::newer as newer_version};
use kumi_daw::{
    io::{Answers, DawIo},
    process::{default_run, executable_name, Run},
    provider::RemovedBridge,
};
use kumi_library::sources::{basename, dirname, join};
use kumi_net::{
    cause::{unreachable, unreachable_call, NetCause},
    proxy,
};
use kumi_types::{
    command::{KUMI, KUMI_START, REINSTALL},
    disk::{free_bytes, size, MB},
    errors::RuntimeError,
    system::{self, Env, SystemProgram},
    version::KUMI_VERSION,
};
pub use migration::{ensure_native_launcher, has_app, launcher, write_launcher};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
#[cfg(windows)]
use std::process::Stdio;
use std::{fs, path::Path, rc::Rc, time::Duration};
/// What an update needs free: what it writes, the download and the new Kumi unpacked beside it. Kumi and the one kept
/// for a rollback are only renamed, and the one before that goes after the swap, with the download. Measured on a real
/// update (1.16.5 → 1.16.6, macOS arm64): free space fell by 131 MB (41 MB download + 90 MB unpacked). Windows x64's
/// bundle is the biggest, 48 + 125 = 174 MB; 250 MB leaves room for the bundles' growth.
pub const UPDATE_SPACE: f64 = 250. * MB;
pub fn release_base(env: &Env) -> String {
    env.get("KUMI_RELEASES")
        .filter(|s| !s.is_empty())
        .map(String::as_str)
        .unwrap_or("https://github.com/user1303836/kumi/releases/latest/download")
        .trim_end_matches('/')
        .into()
}
/// Where `update --version` finds that release: its own downloads on GitHub, or KUMI_RELEASES when that's set.
pub fn release_base_for(env: &Env, version: Option<&str>) -> String {
    match version {
        Some(version) if env.get("KUMI_RELEASES").is_none_or(|s| s.is_empty()) => {
            format!("https://github.com/user1303836/kumi/releases/download/v{version}")
        }
        _ => release_base(env),
    }
}
/// The place releases come from, as Kumi names it: GitHub, or KUMI_RELEASES's host.
fn place(base: &str) -> String {
    let host = url::Url::parse(base).ok().and_then(|url| {
        url.host_str().map(|host| match url.port() {
            Some(port) => format!("{host}:{port}"),
            None => host.to_string(),
        })
    });
    match host {
        Some(host) if host == "github.com" || host.ends_with(".github.com") => "GitHub".into(),
        Some(host) => host,
        None => base.into(),
    }
}
pub fn native_target() -> String {
    let arch = std::env::consts::ARCH;
    match system::platform() {
        "darwin" => format!("{arch}-apple-darwin"),
        "win32" => format!("{arch}-pc-windows-{}", if cfg!(target_env = "gnu") { "gnu" } else { "msvc" }),
        "linux" => format!("{arch}-unknown-linux-{}", if cfg!(target_env = "musl") { "musl" } else { "gnu" }),
        other => format!("{arch}-unknown-{other}"),
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReleaseManifest {
    pub kumi: String,
    pub bundle: String,
    pub sha256: String,
    /// The same bundle xz-compressed, a third smaller, for a tar that reads xz; the releases that make one name it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bundle_xz: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha256_xz: Option<String>,
    pub runtime: String,
    pub target: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bridge: Option<String>,
}
#[derive(Debug, Clone, PartialEq)]
pub enum AskedRelease {
    Manifest(ReleaseManifest),
    None,
    /// Not reached: why, when that's known, and the call that failed, whose words say more (a proxy that isn't
    /// answering, the proxy settings for that address). No call for Kumi's own wait running out or a cut-off answer.
    Offline(Option<NetCause>, Option<(String, LanguageModelError)>),
    /// Reached, but it answered with this status in place of the release's description.
    Answered(u16),
    Invalid,
    /// A web page came back instead of the release's description: a Wi-Fi sign-in page, or a proxy's.
    Page,
}
pub async fn ask_release(env: &Env, fetch: Option<Rc<dyn Fetch>>) -> AskedRelease {
    ask_release_at(&release_base(env), fetch).await
}
pub async fn ask_release_at(base: &str, fetch: Option<Rc<dyn Fetch>>) -> AskedRelease {
    let fetch = fetch.unwrap_or_else(default_fetch);
    let url = format!("{base}/kumi-release.json");
    let waited = abort::timeout(20_000);
    let response = match fetch.fetch(&url, FetchInit { signal: Some(waited.clone()), ..Default::default() }).await {
        Ok(response) => response,
        // Kumi's own wait running out is a timeout, whatever the call says of it.
        Err(_) if waited.is_cancelled() => return AskedRelease::Offline(Some(NetCause::Timeout), None),
        Err(error) => return AskedRelease::Offline(NetCause::of_call(&error), Some((url, error))),
    };
    if response.status == 404 {
        return AskedRelease::None;
    }
    if !response.ok() {
        return match NetCause::of_status(response.status) {
            Some(cause) => AskedRelease::Offline(Some(cause), None),
            None => AskedRelease::Answered(response.status),
        };
    }
    let Ok(text) = response.text().await else { return AskedRelease::Offline(Some(NetCause::Offline), None) };
    if text.trim_start().starts_with('<') {
        return AskedRelease::Page;
    }
    let Ok(value) = serde_json::from_str::<Value>(&text) else { return AskedRelease::Invalid };
    let value = match value.get("targets") {
        Some(targets) => match targets.get(native_target()) {
            Some(target) => target.clone(),
            None => return AskedRelease::Invalid,
        },
        None => value,
    };
    let Ok(manifest) = serde_json::from_value::<ReleaseManifest>(value) else { return AskedRelease::Invalid };
    let matches = |value: &str, pattern: &str| regex::Regex::new(pattern).unwrap().is_match(value);
    if !matches(&manifest.kumi, r"^[0-9]+\.[0-9]+\.[0-9]+(?:-[A-Za-z0-9_.]+)?$")
        || !matches(&manifest.bundle, r"^[A-Za-z0-9_.-]+\.tar\.gz$")
        || !matches(&manifest.sha256, r"^[0-9a-f]{64}$")
        || manifest.runtime != "rust-native"
        || !matches(&manifest.target, r"^[A-Za-z0-9_]+-[A-Za-z0-9_.-]+$")
    {
        AskedRelease::Invalid
    } else {
        AskedRelease::Manifest(manifest)
    }
}
/// The newest release's own downloads, as GitHub's API names it. For a few minutes after a release,
/// `releases/latest/download` can still lead to the one before, while the API already names the new one. The answer
/// only chooses which release's own description (and checksum) to read. None when the API doesn't say (its rate
/// limit, a network that blocks it). It's asked without a token.
async fn newest_release_base(fetch: &Rc<dyn Fetch>) -> Option<String> {
    let headers =
        Headers::from([("accept".into(), "application/vnd.github+json".into()), ("user-agent".into(), format!("kumi/{KUMI_VERSION}"))]);
    let init = FetchInit { headers, signal: Some(abort::timeout(3_000)), ..Default::default() };
    let response = fetch.fetch("https://api.github.com/repos/user1303836/kumi/releases/latest", init).await.ok()?;
    if !response.ok() {
        return None;
    }
    let tag = response.json().await.ok()?.get("tag_name")?.as_str()?.to_string();
    let version = tag.strip_prefix('v')?;
    (crate::config::release_version(version).as_deref() == Some(version))
        .then(|| format!("https://github.com/user1303836/kumi/releases/download/{tag}"))
}
/// The newest release for `kumi update` and its checks: the one GitHub's API names, read from that release's own
/// description, else the usual way. With KUMI_RELEASES, only there. The daily check doesn't come here, so it
/// asks no more than once a day.
async fn ask_newest(env: &Env, fetch: Rc<dyn Fetch>) -> (String, AskedRelease) {
    let usual = release_base(env);
    if env.get("KUMI_RELEASES").is_none_or(|s| s.is_empty()) {
        if let Some(base) = newest_release_base(&fetch).await {
            match ask_release_at(&base, Some(fetch.clone())).await {
                // The usual way goes over the same network: no second wait for the same failure.
                asked @ (AskedRelease::Manifest(_) | AskedRelease::Offline(..)) => return (base, asked),
                _ => {}
            }
        }
    }
    let asked = ask_release_at(&usual, Some(fetch)).await;
    (usual, asked)
}
pub async fn fetch_manifest(env: &Env, fetch: Option<Rc<dyn Fetch>>) -> Option<ReleaseManifest> {
    match ask_release(env, fetch).await {
        AskedRelease::Manifest(manifest) => Some(manifest),
        _ => None,
    }
}
/// Why there's no release to go on, without the last full stop. `pinned`: `update --version`'s.
fn unasked(asked: &AskedRelease, base: &str, pinned: Option<&str>, env: &Env) -> String {
    match (asked, pinned) {
        (AskedRelease::None, Some(version)) => {
            format!("There's no Kumi {version} to get (github.com/user1303836/kumi/releases lists every release)")
        }
        (AskedRelease::None, None) => {
            format!("There's no Kumi release to get at {} yet; try again later", base.strip_prefix("https://").unwrap_or(base))
        }
        (AskedRelease::Offline(cause, call), _) => {
            let host = place(base);
            call.as_ref()
                .and_then(|(url, error)| unreachable_call(&host, url, error, env, &proxy::current()))
                .unwrap_or_else(|| unreachable(&host, *cause, env))
                .trim_end_matches('.')
                .into()
        }
        (AskedRelease::Answered(status), _) => {
            format!("{} answered {status} in place of the release's description; try again in a moment", place(base))
        }
        (AskedRelease::Page, _) => {
            format!("A Wi-Fi sign-in page or a proxy answered instead of {}: sign in to the network, then try again", place(base))
        }
        (_, Some(version)) => format!("Kumi {version}'s description didn't make sense; try again later"),
        _ => "The latest release's description didn't make sense; try again later".into(),
    }
}
pub(crate) fn error(e: impl std::fmt::Display) -> RuntimeError {
    RuntimeError::plain(e.to_string())
}
#[derive(Clone)]
pub struct InstalledIo {
    pub out: Rc<dyn TtyOutput>,
    pub env: Env,
    pub input: Option<Rc<dyn TerminalInput>>,
    pub run: Option<Run>,
    pub fetcher: Option<Rc<dyn Fetch>>,
    pub confirm: Option<Confirm>,
    /// What each DAW's steps read beyond the environment (`kumi_daw::io::Answers`): a test's answers (whether the DAW
    /// runs, `kumi bridge` after an update).
    pub answers: Answers,
    /// The producer's Ctrl-C while an update downloads: it stops the download, and nothing changes.
    pub cancel: Option<kumi_common::abort::Signal>,
    /// `update --version`: that release, newer or older, in place of the newest.
    pub version: Option<String>,
    /// Free space on the disk holding a folder (tests say how much).
    pub free: Option<Rc<dyn Fn(String) -> LocalBoxFuture<'static, Option<f64>>>>,
    /// How many other Kumi processes run this Kumi (tests say how many).
    pub others: Option<Rc<dyn Fn() -> LocalBoxFuture<'static, usize>>>,
    /// Who runs this Kumi, for what an uninstall may touch (tests say who).
    pub runner: Option<Runner>,
}
impl InstalledIo {
    pub fn new(out: Rc<dyn TtyOutput>, env: Env) -> Self {
        Self {
            out,
            env,
            input: None,
            run: None,
            fetcher: None,
            confirm: None,
            answers: Answers::default(),
            cancel: None,
            version: None,
            free: None,
            others: None,
            runner: None,
        }
    }
    /// What each DAW's steps get: the terminal they say things on and ask through, this run's programs, the answers.
    pub fn daw(&self) -> DawIo {
        DawIo {
            out: Rc::new(Terminal::new(self.out.clone(), &self.env)),
            ask: Rc::new(TerminalAsk { input: self.input.clone(), out: self.out.clone(), confirm: self.confirm.clone() }),
            env: self.env.clone(),
            run: self.run.clone().unwrap_or_else(default_run),
            answers: self.answers.clone(),
        }
    }
}
/// The version of the Kumi in `folder`, from its package.json.
pub fn app_version(folder: &str) -> Option<String> {
    let value: Value = serde_json::from_slice(&fs::read(join(folder, "package.json")).ok()?).ok()?;
    value.get("version").and_then(Value::as_str).filter(|s| !s.is_empty()).map(str::to_string)
}
/// `path` as the producer reads it: their home folder as ~.
fn tilde(path: &str) -> String {
    tilde_in(path, &home::home_dir().unwrap_or_default().display().to_string())
}
/// The part of Kumi's folder that belongs to another account (an install run with sudo), so Kumi can't update
/// it: the folder itself, or one of what an update moves. None for root, who can change anything.
pub fn not_yours(home: &str) -> Option<String> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        // SAFETY: geteuid has no preconditions.
        let me = unsafe { libc::geteuid() };
        if me == 0 {
            return None;
        }
        return std::iter::once(home.to_string())
            .chain(["app", "app.previous", "app.previous.old", "app.new", "app.rollback", "bin", "downloads"].map(|part| join(home, part)))
            .find(|path| fs::symlink_metadata(path).is_ok_and(|m| m.uid() != me));
    }
    #[cfg(not(unix))]
    {
        let _ = home;
        None
    }
}
/// Kumi's folder as the producer reads it, and as they'd type it in a shell.
fn shown_and_typed(home: &str) -> (String, String) {
    let shown = tilde(home);
    let typed = if shown.chars().all(|c| c.is_ascii_alphanumeric() || "~/._-".contains(c)) {
        shown.clone()
    } else {
        format!("'{}'", home.replace('\'', "'\\''"))
    };
    (shown, typed)
}
/// The command that gives Kumi's folder back to this account.
pub fn chown_line(home: &str) -> String {
    format!("sudo chown -R $USER {}", shown_and_typed(home).1)
}
/// How to give Kumi's folder back to this account.
pub fn owner_advice(home: &str) -> String {
    format!("{} isn't yours (installed with sudo?): {}", shown_and_typed(home).0, chown_line(home))
}
/// What to say when the system refused to change something in Kumi's folder (on macOS and Linux): whose it is
/// when that's the reason, or the system's words and the usual way back.
fn refused(home: &str, error: &std::io::Error) -> String {
    if not_yours(home).is_some() {
        return owner_advice(home);
    }
    let (shown, typed) = shown_and_typed(home);
    format!("The system refused to change {shown} ({error}); if something in it was installed with sudo: sudo chown -R $USER {typed}")
}
/// Whether the system refused for want of permission, on macOS and Linux (EACCES, EPERM). On Windows the same
/// errors mean a folder held for a moment, which `rename_in` waits out.
fn denied(error: &std::io::Error) -> bool {
    !cfg!(windows) && (error.kind() == std::io::ErrorKind::PermissionDenied || error.raw_os_error() == Some(1))
}
/// A yes/no question; `ahead` says how to answer it beforehand, where a flag does ("add --yes").
pub(crate) async fn ask(io: &InstalledIo, question: &str, ahead: Option<&str>) -> bool {
    if let Some(ask) = &io.confirm {
        ask(question.into()).await
    } else {
        ask_yes_no(io.input.clone(), io.out.clone(), question, ahead).await
    }
}
/// Each DAW's bridge after Kumi changed (`back_to`: to the earlier Kumi it names): brought up to date where it can be,
/// or how to. A DAW Kumi was never put in isn't spoken of; in none at all, how to set Kumi up. The exit code is the first
/// one that isn't 0.
async fn bridges_after(io: &InstalledIo, home: &str, app: &str, back_to: Option<&str>) -> i32 {
    let mut code = 0;
    let daw = io.daw();
    let none = no_daw_yet(&io.env);
    for provider in crate::daw::providers() {
        let done = provider.after_update(&daw, home, app, back_to).await;
        if code == 0 {
            code = done;
        }
    }
    if let Some(words) = none {
        io.out.write(&format!("{words}\n"));
    }
    code
}
/// After an update, with Kumi in no DAW yet: how to set it up in the producer's, whichever that is. None once it's in
/// any.
pub fn no_daw_yet(env: &Env) -> Option<String> {
    (!crate::daw::providers().into_iter().any(|provider| provider.bridge_installed(env)))
        .then(|| format!("Kumi isn't in a DAW yet: run {}, and it sets itself up for the one you work in.", *KUMI_START))
}
/// What a swap does to Kumi's folders. The tests stand in for Windows holding one.
trait Folders {
    fn rename(&self, from: &str, to: &str) -> std::io::Result<()>;
    fn remove(&self, path: &str) -> std::io::Result<()>;
    /// A move waits for a busy folder.
    fn waiting(&self) {}
}
/// Folders that say, once, when a move first has to wait: holding a folder can keep an update waiting a minute.
struct Saying<'a, F: Folders, S: Fn()> {
    folders: &'a F,
    say: S,
    said: std::cell::Cell<bool>,
}
impl<F: Folders, S: Fn()> Folders for Saying<'_, F, S> {
    fn rename(&self, from: &str, to: &str) -> std::io::Result<()> {
        self.folders.rename(from, to)
    }
    fn remove(&self, path: &str) -> std::io::Result<()> {
        self.folders.remove(path)
    }
    fn waiting(&self) {
        if !self.said.replace(true) {
            (self.say)()
        }
    }
}
struct Disk;
impl Folders for Disk {
    fn rename(&self, from: &str, to: &str) -> std::io::Result<()> {
        fs::rename(from, to)
    }
    fn remove(&self, path: &str) -> std::io::Result<()> {
        remove(path)
    }
}
async fn rename(from: &str, to: &str) -> std::io::Result<()> {
    rename_in(&Disk, from, to).await
}
/// Renames, waiting out a folder held for a moment (a Kumi window, an antivirus scan) for 5 s. On macOS and
/// Linux only a busy mount waits: a folder that isn't this account's (EACCES, EPERM) never frees up by itself.
async fn rename_in(folders: &impl Folders, from: &str, to: &str) -> std::io::Result<()> {
    for attempt in 0..=20 {
        match folders.rename(from, to) {
            Ok(()) => return Ok(()),
            Err(e)
                if attempt < 20
                    && (if cfg!(windows) {
                        e.kind() == std::io::ErrorKind::PermissionDenied || matches!(e.raw_os_error(), Some(5 | 32))
                    } else {
                        e.raw_os_error() == Some(16)
                    }) =>
            {
                folders.waiting();
                tokio::time::sleep(Duration::from_millis(250)).await
            }
            Err(e) => return Err(e),
        }
    }
    unreachable!()
}
/// Puts the Kumi that was there (in `previous`) back in `app`, unless `app` already holds a whole one: `app` can
/// be missing, or hold part of a move, which is cleared first. Waits out a busy folder a few times over; says
/// whether `app` holds a whole Kumi.
async fn put_back(folders: &impl Folders, app: &str, previous: &str) -> bool {
    for attempt in 0..3 {
        if migration::has_app(app) {
            return true;
        }
        if !migration::has_app(previous) {
            return false;
        }
        if (folders.remove(app).is_err() || rename_in(folders, previous, app).await.is_err()) && attempt < 2 {
            tokio::time::sleep(Duration::from_secs(1)).await
        }
    }
    migration::has_app(app)
}
/// Puts the new Kumi (`fresh`) in `app`, and the one that was there in `previous`, where a rollback finds it.
/// Whenever the new one can't go in, `app` gets the one that was there back, and `previous` the one before it.
/// An update that gave up earlier can leave the only whole Kumi in `previous` (or the one before it in
/// `previous.old`): it goes back first, and neither is cleared while it holds the only whole Kumi.
pub async fn swap_in(fresh: &str, app: &str, previous: &str) -> std::io::Result<()> {
    swap_in_with(&Disk, fresh, app, previous).await
}
async fn swap_in_with(folders: &impl Folders, fresh: &str, app: &str, previous: &str) -> std::io::Result<()> {
    let older = format!("{previous}.old");
    settle(folders, app, previous, &older).await;
    if migration::has_app(app) || migration::has_app(previous) || !migration::has_app(&older) {
        folders.remove(&older)?;
    }
    let swapped = async {
        if migration::has_app(app) {
            // The one before waits aside until the new one is in: a failed update still leaves it to roll back to.
            if Path::new(previous).exists() {
                rename_in(folders, previous, &older).await?
            }
            rename_in(folders, app, previous).await?
        } else {
            // No whole Kumi in app (none yet, or one couldn't go back): the new one replaces what's there, and
            // app.previous stays as it is.
            folders.remove(app)?
        }
        match rename_in(folders, fresh, app).await {
            // A move that went through before its error counts.
            Err(_) if !Path::new(fresh).exists() && migration::has_app(app) => Ok(()),
            moved => moved,
        }
    }
    .await;
    if let Err(error) = swapped {
        put_back(folders, app, previous).await;
        if !Path::new(previous).exists() && migration::has_app(&older) {
            let _ = rename_in(folders, &older, previous).await;
        }
        return Err(error);
    }
    // What was in app.previous before is no longer needed; a later swap clears it if this can't.
    let _ = folders.remove(&older);
    Ok(())
}
/// After an update that gave up: `app` gets a whole Kumi back from `previous`, or from the one before it in
/// `older`, and `previous` gets the one before back if it's gone.
async fn settle(folders: &impl Folders, app: &str, previous: &str, older: &str) {
    if !put_back(folders, app, previous).await {
        put_back(folders, app, older).await;
    }
    if !Path::new(previous).exists() && migration::has_app(older) {
        let _ = rename_in(folders, older, previous).await;
    }
}
/// Puts things back as they were after an update that gave up, before anything else: a later run starts
/// from a whole Kumi in app wherever one is left.
async fn settle_installed(home: &str) {
    let previous = join(home, "app.previous");
    settle(&Disk, &join(home, "app"), &previous, &format!("{previous}.old")).await
}
/// What to say when the new Kumi couldn't go in; `kept` says whether the one that was there still works, and
/// `why` is the system's reason (macOS and Linux; Windows names the usual one).
fn swap_failed(windows: bool, kept: bool, why: &str) -> String {
    let what = "Windows kept Kumi's folder busy: a Kumi window, or an antivirus scan of the new files.";
    let installer = format!("run the installer again ({REINSTALL})");
    match (windows, kept) {
        (true, true) => format!("{what} The Kumi you had still works: close every Kumi window, give the scan a minute, then run update again."),
        (true, false) => format!(
            "{what} The Kumi you had couldn't go back in place yet, so kumi won't start: close every Kumi window, give the scan a minute, then {installer}."
        ),
        (false, true) => format!("Couldn't put the new Kumi in place ({why}), so this one stays."),
        (false, false) => format!("Couldn't put the new Kumi in place ({why}), and the one you had couldn't go back, so kumi won't start: {installer}."),
    }
}
/// What an update leaves behind, removed when it ends, best effort: a file a scan still holds can't make a good
/// update fail.
struct Cleanup(Vec<String>);
impl Drop for Cleanup {
    fn drop(&mut self) {
        for path in &self.0 {
            let _ = remove(path);
        }
    }
}
/// Why a download didn't finish.
enum Stopped {
    /// The producer's Ctrl-C.
    Cancelled,
    /// The connection, and why when the error says.
    Network(Option<NetCause>),
    /// What the server answered instead of the bundle.
    Status(u16),
    /// Writing it on this computer.
    Disk(std::io::Error),
}
impl Stopped {
    /// What to say, ending with what that means: nothing was changed.
    fn said(&self, home: &str) -> String {
        match self {
            Self::Cancelled => "The download was stopped, so nothing was changed.".into(),
            Self::Network(Some(cause)) => {
                format!("The download stopped partway: {}, so nothing was changed. {}", cause.words(), cause.fix())
            }
            Self::Network(None) => "The download stopped partway, so nothing was changed. Check the connection, then try again.".into(),
            Self::Status(status) => match NetCause::of_status(*status) {
                Some(cause) => format!("The download stopped: {}, so nothing was changed. {}", cause.words(), cause.fix()),
                None => format!("The download failed (it answered {status}), so nothing was changed. Try again in a moment."),
            },
            Self::Disk(error) if error.kind() == std::io::ErrorKind::StorageFull => {
                "The disk filled up during the download, so nothing was changed. Free some space, then try again.".into()
            }
            Self::Disk(error) if denied(error) => format!("{}. Nothing was changed.", refused(home, error).trim_end_matches('.')),
            Self::Disk(error) => format!("Kumi couldn't save the download ({error}), so nothing was changed."),
        }
    }
}
/// The bundle an update takes, its checksum and how tar unpacks it: the xz one when the release names one whole (a
/// well-formed name and sum; anything else is passed over, as install.sh passes it over, never a reason to refuse the
/// release) and this system's tar reads xz (macOS's bsdtar, GNU tar beside xz); else the gzipped one, which every tar
/// reads. KUMI_BUNDLE_XZ=1|0 says, for tests.
fn pick_bundle(manifest: &ReleaseManifest, env: &Env) -> (String, String, &'static str) {
    let matches = |value: &str, pattern: &str| regex::Regex::new(pattern).unwrap().is_match(value);
    if let (Some(name), Some(sha256)) = (&manifest.bundle_xz, &manifest.sha256_xz) {
        if matches(name, r"^[A-Za-z0-9_.-]+\.tar\.xz$") && matches(sha256, r"^[0-9a-f]{64}$") && tar_reads_xz(env) {
            return (name.clone(), sha256.clone(), "-xf");
        }
    }
    (manifest.bundle.clone(), manifest.sha256.clone(), "-xzf")
}
fn tar_reads_xz(env: &Env) -> bool {
    match env.get("KUMI_BUNDLE_XZ").map(String::as_str) {
        Some("1") => return true,
        Some("0") => return false,
        _ => {}
    }
    match system::platform() {
        "darwin" => true,
        "linux" => env.get("PATH").is_some_and(|path| std::env::split_paths(path).any(|folder| folder.join("xz").is_file())),
        _ => false,
    }
}
/// Downloads `url` into `file` as it arrives, and gives what it got's SHA-256: the bundle is never held whole, nor
/// read back to check it.
async fn download(url: &str, file: &str, fetch: Rc<dyn Fetch>, cancel: Option<abort::Signal>) -> Result<String, Stopped> {
    use tokio::io::AsyncWriteExt;
    let signal = abort::any(std::iter::once(abort::timeout(600000)).chain(cancel.clone()));
    let stopped = |error: &LanguageModelError| {
        if cancel.as_ref().is_some_and(abort::Signal::is_cancelled) {
            Stopped::Cancelled
        } else {
            Stopped::Network(NetCause::of_call(error))
        }
    };
    let response = fetch.fetch(url, FetchInit { signal: Some(signal.clone()), ..Default::default() }).await.map_err(|e| stopped(&e))?;
    if !response.ok() {
        return Err(Stopped::Status(response.status));
    }
    let mut out = tokio::fs::File::create(file).await.map_err(Stopped::Disk)?;
    let mut digest = Sha256::new();
    if let Some(mut body) = response.body {
        loop {
            // The body's chunks race the signal too: a stalled connection stops when it fires.
            let chunk = tokio::select! {
                chunk = body.next() => chunk,
                _ = signal.cancelled() => return Err(if cancel.as_ref().is_some_and(abort::Signal::is_cancelled) {
                    Stopped::Cancelled
                } else {
                    Stopped::Network(Some(NetCause::Timeout))
                }),
            };
            let Some(chunk) = chunk else { break };
            let chunk = chunk.map_err(|e| stopped(&e))?;
            digest.update(&chunk);
            out.write_all(&chunk).await.map_err(Stopped::Disk)?;
        }
    }
    // A tokio file finishes its last write in the background: wait for it, so a full disk is the download's error.
    out.flush().await.map_err(Stopped::Disk)?;
    Ok(hex::encode(digest.finalize()))
}
/// What a step that moves Kumi's folders says when the system refuses: whose folder it is, or why.
fn not_in_place(home: &str, error: &std::io::Error) -> String {
    if denied(error) {
        format!("{}. The Kumi you had stays.", refused(home, error).trim_end_matches('.'))
    } else {
        format!("Couldn't put the new Kumi in place ({error}), so this one stays.")
    }
}
/// The older Kumi reads notes, techniques and lessons from files: what the database keeps is written back for it
/// first. A file that already says it is left as it is; if this fails, going back still goes ahead.
async fn write_back_for_older(io: &InstalledIo) {
    let say = |s: String| io.out.write(&format!("{s}\n"));
    let written = match (load_db_file(&io.env), json_files(&io.env)) {
        (Ok(db), Ok(files)) => write_back(db.into(), files, kumi_common::time::now_ms()).await,
        (Err(why), _) | (_, Err(why)) => Err(why),
    };
    match written {
        Ok(written) => {
            for (file, why) in written.left {
                say(format!("Left {} as it was: {why}.", file.display()));
            }
        }
        Err(why) => say(format!(
            "The older Kumi won't see the notes, techniques or lessons kept since the update ({}); they stay in Kumi's database for when you update again.",
            why.message()
        )),
    }
}
pub async fn update_installed(io: InstalledIo) -> Result<i32, RuntimeError> {
    let say = |s: String| io.out.write(&format!("{s}\n"));
    let home = kumi_home(&io.env);
    let app = join(&home, "app");
    // Another account's folder (an install run with sudo) can't take a new Kumi: said before anything moves.
    if not_yours(&home).is_some() {
        say(format!("{} Nothing was changed.", owner_advice(&home)));
        return Ok(1);
    }
    settle_installed(&home).await;
    let run = io.run.clone().unwrap_or_else(default_run);
    let fetch = io.fetcher.clone().unwrap_or_else(default_fetch);
    let pinned = io.version.as_deref();
    if let Some(version) = pinned.filter(|version| !newer_version(version, "1.7.5")) {
        say(format!("Kumi {version} came before Kumi's native releases, so kumi update can't install it; 1.7.6 is the earliest it can."));
        return Ok(1);
    }
    if pinned == Some(KUMI_VERSION) {
        say(format!("Kumi is {KUMI_VERSION} already."));
        return Ok(bridges_after(&io, &home, &app, None).await);
    }
    let looking = pinned.map(|version| format!("Looking for Kumi {version}…")).unwrap_or_else(|| "Looking for a newer Kumi…".into());
    let asking = async {
        match pinned {
            Some(_) => {
                let base = release_base_for(&io.env, pinned);
                let asked = ask_release_at(&base, Some(fetch.clone())).await;
                (base, asked)
            }
            None => ask_newest(&io.env, fetch.clone()).await,
        }
    };
    let (base, asked) = step(io.out.clone(), &io.env, &looking, asking, true).await;
    let AskedRelease::Manifest(manifest) = asked else {
        say(format!("{}. Nothing was changed.", unasked(&asked, &base, pinned, &io.env)));
        return Ok(1);
    };
    match pinned {
        Some(version) if manifest.kumi != version => {
            say(format!("There's no Kumi {version} at {}; it has {}. Nothing was changed.", place(&base), manifest.kumi));
            return Ok(1);
        }
        None if !newer_version(&manifest.kumi, KUMI_VERSION) => {
            say(format!("Kumi is up to date ({KUMI_VERSION})."));
            return Ok(bridges_after(&io, &home, &app, None).await);
        }
        _ => {}
    }
    if manifest.target != native_target() {
        say(format!(
            "Kumi {} is for {}, but this Kumi is for {}. Run the installer again for this computer ({REINSTALL}).",
            manifest.kumi,
            manifest.target,
            native_target()
        ));
        return Ok(1);
    }
    let free = match &io.free {
        Some(free) => free(home.clone()).await,
        None => free_bytes(Path::new(&home)).await,
    };
    if let Some(free) = free.filter(|free| *free < UPDATE_SPACE) {
        say(format!(
            "Kumi needs about 250 MB free to update, and the disk with {} has {}. Free some space (empty the Trash or Recycle Bin, or move old bounces and videos off that disk), then try again. Nothing was changed.",
            tilde(&home),
            size(free)
        ));
        return Ok(1);
    }
    let older = newer_version(KUMI_VERSION, &manifest.kumi);
    let downloads = join(&home, "downloads");
    // Gone again once emptied, but only when this update made it: never a folder that was there (in a home folder
    // used as Kumi's, on a disk that ignores case, `downloads` is the producer's Downloads).
    let made = fs::symlink_metadata(&downloads).is_err();
    if let Err(error) = tokio::fs::create_dir_all(&downloads).await {
        say(Stopped::Disk(error).said(&home));
        return Ok(1);
    }
    let (asset, sha256, unpack) = pick_bundle(&manifest, &io.env);
    let bundle = join(&downloads, &asset);
    let fresh = join(&home, "app.new");
    let cleanup = Cleanup(vec![fresh.clone(), bundle.clone()]);
    let result: Result<i32, RuntimeError> = async {
        let downloaded = step(
            io.out.clone(),
            &io.env,
            &format!("Downloading Kumi {}…", manifest.kumi),
            download(&format!("{base}/{asset}"), &bundle, fetch, io.cancel.clone()),
            true,
        )
        .await;
        let downloaded = match downloaded {
            Ok(sha256) => sha256,
            Err(stopped) => {
                say(stopped.said(&home));
                return Ok(1);
            }
        };
        if downloaded != sha256 {
            say("The download didn't match its checksum, so nothing was changed. Try again in a moment.".into());
            return Ok(1);
        }
        let unpacked = step(
            io.out.clone(),
            &io.env,
            &format!("Unpacking Kumi {}…", manifest.kumi),
            async {
                if let Err(error) = remove(&fresh).and_then(|()| fs::create_dir_all(&fresh)) {
                    return not_in_place(&home, &error).into();
                }
                // tar.exe reads its arguments in Windows' code page, which can't spell every folder name: run in
                // KUMI_HOME, it's given the paths inside it, which are plain ASCII.
                let inside = |path: &str| {
                    Path::new(path).strip_prefix(&home).map_or_else(|_| path.to_string(), |inside| inside.display().to_string())
                };
                let unpacked = run(
                    system::system_program_default(SystemProgram::Tar),
                    vec![unpack.into(), inside(&bundle), "-C".into(), inside(&fresh)],
                    Some(home.clone()),
                )
                .await;
                if unpacked.code != 0 {
                    let text = if unpacked.stderr.is_empty() { &unpacked.stdout } else { &unpacked.stderr };
                    return Some(format!("Unpacking it failed: {}", trim(text).split('\n').next_back().unwrap_or("tar failed")));
                }
                let probe = run(join(&fresh, &executable_name("kumi")), vec!["--version".into()], None).await;
                if probe.code != 0 || !probe.stdout.contains(&manifest.kumi) {
                    return Some(format!(
                        "The new Kumi didn't start, so this one stays. Try again, or run the installer again ({REINSTALL})."
                    ));
                }
                None
            },
            false,
        )
        .await;
        if let Some(failed) = unpacked {
            say(failed);
            return Ok(1);
        }
        // Going back: what the database keeps is written for the older Kumi just before it goes in, once it's
        // unpacked and has started.
        if older {
            write_back_for_older(&io).await;
        }
        let failed = step(
            io.out.clone(),
            &io.env,
            &format!("Putting Kumi {} in place…", manifest.kumi),
            async {
                if let Err(error) = write_launcher(&home) {
                    return not_in_place(&home, &error).into();
                }
                // Printed above the spinner, which draws its line again below.
                let spinning = crate::spinner::spins(io.out.as_ref(), &io.env);
                let say = || {
                    let line = if cfg!(windows) {
                        "Windows is holding Kumi's folder (a Kumi window, or an antivirus scan); waiting…"
                    } else {
                        "Kumi's folder is busy; waiting…"
                    };
                    io.out.write(&if spinning { format!("\r\u{1b}[2K{line}\n") } else { format!("{line}\n") })
                };
                let folders = Saying { folders: &Disk, say, said: Default::default() };
                if let Err(error) = swap_in_with(&folders, &fresh, &app, &join(&home, "app.previous")).await {
                    let kept = migration::has_app(&app);
                    return Some(if denied(&error) && kept {
                        not_in_place(&home, &error)
                    } else {
                        swap_failed(cfg!(windows), kept, &error.to_string())
                    });
                }
                None
            },
            false,
        )
        .await;
        if let Some(failed) = failed {
            say(failed);
            return Ok(1);
        }
        Ok(0)
    }
    .await;
    drop(cleanup);
    // Only when it's empty: a file a scan still holds stays for the next update to clear.
    if made {
        let _ = fs::remove_dir(&downloads);
    }
    let code = result?;
    if code != 0 {
        return Ok(code);
    }
    say(format!(
        "Kumi {KUMI_VERSION} → {}{}. {} update --rollback goes back to {KUMI_VERSION}.",
        manifest.kumi,
        if older { ", an earlier one" } else { "" },
        *KUMI
    ));
    if !older {
        let changelog = fs::read_to_string(join(&app, "CHANGELOG.md")).unwrap_or_default();
        for line in crate::whats_new::update_lines(&changelog, KUMI_VERSION, &manifest.kumi) {
            say(line);
        }
    }
    if let Some(open) = still_open(&io, &join(&app, &executable_name("kumi"))).await {
        say(open);
    }
    Ok(bridges_after(&io, &home, &app, older.then_some(manifest.kumi.as_str())).await)
}
/// How many other processes run `exe` on this account: Kumi windows, or a `kumi` command in another terminal.
/// None on Windows, where Kumi can't update or roll back while one runs at all.
pub async fn others_running(exe: &str) -> usize {
    if cfg!(windows) {
        return 0;
    }
    let ps = if Path::new("/bin/ps").exists() { "/bin/ps" } else { "ps" };
    let listing = kumi_daw::process::run_program(ps, &["-axo".into(), "uid=,pid=,args=".into()], None).await.stdout;
    #[cfg(unix)]
    // SAFETY: getuid has no preconditions.
    let uid = unsafe { libc::getuid() }.to_string();
    #[cfg(not(unix))]
    let uid = String::new();
    let me = std::process::id().to_string();
    let names: Vec<String> =
        [Some(exe.to_string()), fs::canonicalize(exe).ok().map(|p| p.display().to_string())].into_iter().flatten().collect();
    listing
        .lines()
        .filter_map(|line| {
            let (owner, rest) = line.trim_start().split_once(char::is_whitespace)?;
            let (pid, args) = rest.trim_start().split_once(char::is_whitespace).unwrap_or((rest.trim_start(), ""));
            (owner == uid && pid != me).then_some(args.trim_start())
        })
        .filter(|args| names.iter().any(|name| *args == name.as_str() || args.starts_with(&format!("{name} "))))
        .count()
}
/// After an update: Kumi windows already open still run this Kumi (Windows can't update while one is open). What
/// they keep reaches the new Kumi at its next turn; what it keeps reaches them once restarted. Said only when one is.
pub fn open_windows(others: usize) -> Option<String> {
    match others {
        0 => None,
        1 => Some(format!("A Kumi window opened before the update keeps running {KUMI_VERSION} until you restart it.")),
        n => Some(format!("{n} Kumi windows opened before the update keep running {KUMI_VERSION} until you restart them.")),
    }
}
async fn still_open(io: &InstalledIo, exe: &str) -> Option<String> {
    open_windows(match &io.others {
        Some(others) => others().await,
        None => others_running(exe).await,
    })
}
/// What the farewell adds when the installer started Kumi in a window where `kumi` doesn't work yet, as the installer
/// says in KUMI_FROM_INSTALLER: new terminal windows have it on PATH (`new-window`), or nothing put it there (`no-path`).
pub fn next_time(env: &Env) -> Option<String> {
    let home =
        env.get("HOME").filter(|s| !s.is_empty()).cloned().unwrap_or_else(|| home::home_dir().unwrap_or_default().display().to_string());
    let shown = |path: String| match path.strip_prefix(&home) {
        Some(rest) if !home.is_empty() && rest.starts_with('/') => format!("~{rest}"),
        _ => path,
    };
    let bin = join(&kumi_home(env), "bin");
    match env.get("KUMI_FROM_INSTALLER").map(String::as_str) {
        Some("new-window") => Some("Next time: open a new terminal window and type kumi".into()),
        Some("no-path") => {
            let kumi = shown(join(&bin, "kumi"));
            let typed = if kumi.contains(' ') { format!("\"{}\"", join(&bin, "kumi")) } else { kumi };
            Some(format!("Next time: run {typed}, or add {} to PATH", shown(bin)))
        }
        _ => None,
    }
}

pub async fn rollback_installed(io: InstalledIo) -> Result<i32, RuntimeError> {
    let say = |s: String| io.out.write(&format!("{s}\n"));
    let home = kumi_home(&io.env);
    let app = join(&home, "app");
    let previous = join(&home, "app.previous");
    let hold = join(&home, "app.rollback");
    settle_installed(&home).await;
    if !migration::has_app(&previous) {
        say("There's no earlier Kumi to go back to.".into());
        return Ok(1);
    }
    // Windows can't roll back while a Kumi window is open; elsewhere one keeps running this Kumi.
    let others = match &io.others {
        Some(others) => others().await,
        None => others_running(&join(&app, &executable_name("kumi"))).await,
    };
    match others {
        0 => {}
        1 => say(format!(
            "A Kumi window still open keeps running {KUMI_VERSION} until you close it; what it keeps from now on stays in Kumi's database for your next update."
        )),
        n => say(format!(
            "{n} Kumi windows still open keep running {KUMI_VERSION} until you close them; what they keep from now on stays in Kumi's database for your next update."
        )),
    }
    let legacy = !Path::new(&join(&previous, &executable_name("kumi"))).is_file();
    let version = fs::read(join(&previous, "package.json"))
        .ok()
        .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
        .and_then(|v| v.get("version").and_then(Value::as_str).map(str::to_string))
        .unwrap_or("the one before".into());
    // Each DAW's bridge, readied for the rollback: what goes back before the swap, and after it.
    let mut bridges = vec![];
    let daw = io.daw();
    for provider in crate::daw::providers() {
        bridges.push(provider.prepare_rollback(&daw, &home, &previous, &version, legacy).await?);
    }
    write_back_for_older(&io).await;
    write_launcher(&home).map_err(error)?;
    for bridge in &bridges {
        bridge.before_swap().await?;
    }
    let switched = async {
        remove(&hold)?;
        rename(&app, &hold).await?;
        rename(&previous, &app).await?;
        rename(&hold, &previous).await
    }
    .await;
    if switched.is_err() {
        if Path::new(&hold).exists() {
            if Path::new(&app).exists() && !Path::new(&previous).exists() {
                rename(&app, &previous).await.map_err(error)?;
            }
            if !Path::new(&app).exists() {
                rename(&hold, &app).await.map_err(error)?;
            }
        }
        for bridge in &bridges {
            if let Some(said) = bridge.swap_failed().await {
                say(said);
            }
        }
        say("Couldn't switch back; close every Kumi window and try again.".into());
        return Ok(1);
    }
    say(format!("Kumi is back to {version}. {} update --rollback again returns to {KUMI_VERSION}.", *KUMI));
    // Each DAW's bridge, finished now Kumi is back.
    let native = Path::new(&join(&app, &executable_name("kumi"))).exists();
    let mut code = 0;
    for bridge in &bridges {
        let done = bridge.after_swap(&daw, &home, &app, &version, native).await;
        if code == 0 {
            code = done;
        }
    }
    Ok(code)
}
pub const PATH_MARKER: &str = "# Added by the Kumi installer";
pub fn startup_files(env: &Env) -> Vec<String> {
    let actual = home::home_dir().unwrap_or_default().display().to_string();
    let home = env.get("HOME").filter(|s| !s.is_empty()).map(String::as_str).unwrap_or(&actual);
    let zsh = env.get("ZDOTDIR").filter(|s| !s.is_empty()).map(String::as_str).unwrap_or(home);
    let mut files = vec![];
    for file in [join(zsh, ".zshrc"), join(zsh, ".zprofile")].into_iter().chain(
        [".zshrc", ".bashrc", ".bash_profile", ".bash_login", ".profile", ".config/fish/conf.d/kumi.fish"].map(|name| join(home, name)),
    ) {
        if !files.contains(&file) {
            files.push(file)
        }
    }
    files
}
/// Kumi's PATH lines in one startup file: what an uninstall takes out, and what it leaves alone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PathLines {
    pub file: String,
    /// What the file holds once Kumi's lines are out; None when it goes (fish's kumi.fish with nothing else in it).
    pub after: Option<String>,
    /// Kumi's lines it takes out, with their line numbers (from 1), as they are in the file.
    pub removed: Vec<(usize, String)>,
    /// Kumi's lines it leaves alone because they sit inside an if, for or function there: taking them out could leave
    /// an empty body, which the shell refuses.
    pub left: Vec<(usize, String)>,
}
/// A line that opens a body Kumi's lines could sit in: one ending in `then`, `do`, `{`, `else` or a case's `in`; in
/// fish, one starting with `if`, `for`, `while`, `function`, `begin`, `switch`, `case` or `else` that doesn't end it.
fn opens_a_body(line: &str, fish: bool) -> bool {
    let words: Vec<&str> = line.split_whitespace().collect();
    let last = words.last().copied().unwrap_or("");
    if fish {
        return words.first().is_some_and(|first| ["if", "for", "while", "function", "begin", "switch", "case", "else"].contains(first))
            && last != "end";
    }
    ["then", "do", "{", "else", "in"].contains(&last) || last.ends_with(";then") || last.ends_with(";do")
}
/// A line that closes such a body: `fi`, `done`, `}`, `esac`, `else`, `elif`, `;;`, or fish's `end`.
fn closes_a_body(line: &str) -> bool {
    line.split_whitespace().next().is_some_and(|first| {
        ["fi", "done", "}", "esac", "else", "elif", ";;", "end"].contains(&first.trim_end_matches(';'))
            || first.starts_with('}')
            || first == ";;"
    })
}
/// A line with code on it: not blank, not a comment.
fn code(line: &&str) -> bool {
    let line = line.trim();
    !line.is_empty() && !line.starts_with('#')
}
/// The startup files with Kumi's PATH lines. A marker is found as install.sh finds it, CRLF endings and trailing
/// spaces included. Kumi's line is the one under it naming this Kumi's bin, as written or spelled from $HOME,
/// ${HOME} or ~; the installer's blank line above the marker goes with them, and so does a marker with no PATH line
/// under it. A marker over another Kumi's PATH line, every line under no marker, and a block inside an if, for or
/// function body stay.
pub fn path_lines(env: &Env, home: &str) -> Vec<PathLines> {
    if cfg!(windows) {
        return vec![];
    }
    // One slash between names: an installer given a HOME ending in / wrote …//.kumi/bin.
    let single = |path: &str| {
        let mut out = String::with_capacity(path.len());
        for c in path.chars() {
            if !(c == '/' && out.ends_with('/')) {
                out.push(c);
            }
        }
        out
    };
    let bin = single(&join(home, "bin"));
    let actual = home::home_dir().unwrap_or_default().display().to_string();
    let user = single(env.get("HOME").filter(|s| !s.is_empty()).map(String::as_str).unwrap_or(&actual));
    let relative = bin.strip_prefix(&format!("{}/", user.trim_end_matches('/'))).filter(|rest| !rest.is_empty()).map(str::to_string);
    let ours = |line: &str| {
        let line = single(line);
        line.contains(&bin)
            || relative.as_ref().is_some_and(|rest| ["$HOME/", "${HOME}/", "~/"].iter().any(|from| line.contains(&format!("{from}{rest}"))))
    };
    let mut found = vec![];
    for file in startup_files(env) {
        let Ok(text) = fs::read_to_string(&file) else { continue };
        let lines: Vec<&str> = text.split('\n').collect();
        let mut kept: Vec<&str> = vec![];
        let (mut removed, mut left) = (vec![], vec![]);
        let mut index = 0;
        while index < lines.len() {
            let line = lines[index];
            let next = lines.get(index + 1).copied();
            let other_kumis = next.is_some_and(|next| !ours(next) && next.contains("PATH"));
            if line.trim() != PATH_MARKER || other_kumis {
                kept.push(line);
                index += 1;
                continue;
            }
            let block = if next.is_some_and(ours) { 2 } else { 1 };
            // The code around the block, past blank lines and comments: an if, for or function it may sit in.
            let above = kept.iter().rev().find(|line| code(line)).copied();
            let below = lines[index + block..].iter().find(|line| code(line)).copied();
            let indented = line.starts_with([' ', '\t']) || (block == 2 && next.is_some_and(|next| next.starts_with([' ', '\t'])));
            if indented || above.is_some_and(|above| opens_a_body(above, file.ends_with(".fish"))) || below.is_some_and(closes_a_body) {
                left.extend((index..index + block).map(|at| (at + 1, lines[at].trim_end_matches('\r').to_string())));
                kept.extend(&lines[index..index + block]);
            } else {
                if kept.last().is_some_and(|last| last.trim().is_empty()) {
                    kept.pop();
                }
                removed.extend((index..index + block).map(|at| (at + 1, lines[at].trim_end_matches('\r').to_string())));
            }
            index += block;
        }
        if removed.is_empty() && left.is_empty() {
            continue;
        }
        let after = kept.join("\n");
        let emptied = file.ends_with("kumi.fish") && after.trim().is_empty();
        found.push(PathLines { file, after: (!emptied).then_some(after), removed, left });
    }
    found
}
/// Whether this account can make the change `lines` needs: write the file, or remove it from its folder.
fn can_change(lines: &PathLines) -> bool {
    #[cfg(unix)]
    {
        let target = if lines.after.is_some() { lines.file.clone() } else { dirname(&lines.file) };
        let Ok(path) = std::ffi::CString::new(target) else { return false };
        // SAFETY: `path` is a valid C string.
        unsafe { libc::access(path.as_ptr(), libc::W_OK) == 0 }
    }
    #[cfg(not(unix))]
    {
        let _ = lines;
        true
    }
}
/// Kumi's lines as the producer would take them out by hand.
fn by_hand(lines: &[(usize, String)]) -> String {
    let first = lines.first().map_or(0, |(at, _)| *at);
    let last = lines.last().map_or(0, |(at, _)| *at);
    let span = if first == last { format!("line {first}") } else { format!("lines {first}–{last}") };
    format!("{span}: {}", lines.iter().map(|(_, line)| line.as_str()).collect::<Vec<_>>().join(" / "))
}
/// Takes Kumi's PATH lines out of the startup files, and says what it did and what it couldn't do (with the lines to
/// take out by hand). False when a file couldn't be changed.
pub fn remove_path_lines(io: &InstalledIo, home: &str) -> bool {
    let user = user_home(&io.env);
    let mut changed = true;
    for lines in path_lines(&io.env, home) {
        let shown = tilde_in(&lines.file, &user);
        if !lines.left.is_empty() {
            io.out.write(&format!(
                "Left Kumi's PATH line in {shown} alone ({}): it sits inside an if, for or function there; take it out by hand if you like.\n",
                by_hand(&lines.left)
            ));
        }
        if lines.removed.is_empty() {
            continue;
        }
        let result = match &lines.after {
            Some(text) => fs::write(&lines.file, text).map(|()| format!("Took Kumi out of {shown}.")),
            None => remove(&lines.file).map(|()| format!("Took Kumi out of fish ({shown} is gone).")),
        };
        io.out.write(&match result {
            Ok(said) => format!("{said}\n"),
            Err(error) => {
                changed = false;
                format!("Couldn't take Kumi out of {shown} ({error}): take out {} by hand.\n", by_hand(&lines.removed))
            }
        });
    }
    changed
}
async fn remove_windows_path(run: Run, home: &str) {
    if !cfg!(windows) {
        return;
    }
    let bin = join(home, "bin").replace('\'', "''");
    let script = format!(
        r#"$key = (Get-Item -LiteralPath 'HKCU:\').OpenSubKey('Environment', $true)
$path = $key.GetValue('Path', '', 'DoNotExpandEnvironmentNames')
if ($path) {{
  $kept = ($path -split ';' | Where-Object {{ $_ -and ($_.TrimEnd('\') -ne '{bin}') }}) -join ';'
  if ($kept -ne $path) {{
    $key.SetValue('Path', $kept, $key.GetValueKind('Path'))
    Add-Type -Namespace Kumi -Name Env -MemberDefinition '[DllImport("user32.dll", CharSet = CharSet.Auto)] public static extern System.IntPtr SendMessageTimeout(System.IntPtr hWnd, uint msg, System.UIntPtr wParam, string lParam, uint flags, uint timeout, out System.UIntPtr result);'
    $result = [UIntPtr]::Zero; [void][Kumi.Env]::SendMessageTimeout([IntPtr]0xffff, 0x1a, [UIntPtr]::Zero, 'Environment', 2, 5000, [ref]$result)
  }}
}}"#
    );
    let _ = run(
        system::system_program_default(SystemProgram::Powershell),
        vec!["-NoProfile".into(), "-NonInteractive".into(), "-Command".into(), script],
        None,
    )
    .await;
}
#[derive(Clone, Copy, Default)]
pub struct UninstallOptions {
    pub all: bool,
    pub yes: bool,
    /// Say what would go, and remove nothing.
    pub dry_run: bool,
}
/// What `kumi uninstall` takes out of Kumi's folder, by these exact names: Kumi and the one kept for a rollback, what
/// updates and repairs leave behind (`app.replaced` is the Windows installer's), and what Kumi fetched and fetches
/// again when needed (programs, models, videos; each only while it holds only Kumi's own). Never conversations, notes,
/// recipes, sign-ins, the library or references; the bridge's files stay while Live uses them. REAPER's and Renoise's
/// bridge files (`bridges`) go by what's in them ([`BRIDGES_OWN`]).
pub const KUMI_S_OWN: [&str; 13] = [
    "app",
    "app.previous",
    "app.previous.old",
    "app.new",
    "app.replaced",
    "app.rollback",
    "node",
    "bin",
    "downloads",
    "tools",
    "models",
    "videos",
    "bridge",
];
/// In each DAW's folder in `bridges` (REAPER's and Renoise's bridge files; their DAWs load their own copies), what's
/// Kumi's to take out: kept generations, discovery, logs, renders to listen to, the receipt, the secret, the lock and the
/// journal, and a temporary file one of them left (`.<name>.tmp-…`). Catch-up's baselines (`snapshots`) are Kumi's data,
/// gone only with --all; the copies of Kumi's bridge the producer had changed (`changed`), which the lifecycle kept for
/// them, always stay.
pub const BRIDGES_OWN: [&str; 8] =
    ["generations", "discovery", "logs", "listening", "install-receipt.json", "bridge.secret", "lifecycle.lock", "lifecycle-journal.json"];
/// What only Kumi's bridge lifecycle writes in a DAW's folder in `bridges`: in a folder the producer also uses, a DAW's
/// folder is Kumi's only when it holds one of these.
const BRIDGES_MARKS: [&str; 4] = ["install-receipt.json", "bridge.secret", "lifecycle.lock", "lifecycle-journal.json"];
/// The data in a DAW's folder in `bridges`, and the copies kept for the producer there.
const BRIDGES_DATA: &str = "snapshots";
const BRIDGES_CHANGED: &str = "changed";
/// What the producer keeps unless they say --all.
const KEPT: &str = "conversations, notes, recipes, sign-ins, library and references";
/// Why a folder isn't one of Kumi's own to uninstall from. In the home folder or at a disk's root, the names Kumi
/// uses (bin, downloads, tools…) could be the producer's own; on macOS and Windows `downloads` is also their Downloads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotKumis {
    /// A relative path: where it points depends on the folder Kumi runs in.
    Relative,
    /// There's no such folder.
    Missing,
    /// The home folder itself.
    Home,
    /// A disk's root: /, C:\, or where a disk is mounted.
    Root,
    /// It holds no Kumi launcher (bin/kumi) and app.
    Unmarked,
    /// Root (sudo) on a folder that isn't root's: someone else's Kumi, perhaps in their home folder.
    NotRoots,
}
/// Who runs this Kumi, for what an uninstall may touch: whether as root (sudo, su), and the home folders the account
/// database gives, whatever HOME says. sudo sets HOME to root's own, while Kumi's launcher still finds Kumi's folder
/// beside it: in the home folder of the account that ran sudo, if an earlier installer put it there.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Runner {
    pub root: bool,
    /// This account's home folder, and under sudo, the home folder of the account that ran it.
    pub homes: Vec<String>,
}
impl Runner {
    pub fn current(env: &Env) -> Self {
        #[cfg(unix)]
        {
            // SAFETY: getuid and geteuid have no preconditions.
            let (uid, euid) = unsafe { (libc::getuid(), libc::geteuid()) };
            let mut homes: Vec<String> = account_home_of(uid).into_iter().collect();
            if euid == 0 || uid == 0 {
                homes.extend(env.get("SUDO_USER").filter(|user| !user.is_empty()).and_then(|user| account_home_named(user)));
                homes.extend(env.get("SUDO_UID").and_then(|id| id.parse().ok()).and_then(account_home_of));
            }
            Runner { root: euid == 0, homes }
        }
        #[cfg(not(unix))]
        {
            let _ = env;
            Runner::default()
        }
    }
}
/// A home folder from the account database, by a lookup that fills in the record (getpwuid_r, getpwnam_r).
#[cfg(unix)]
fn account_home(lookup: impl FnOnce(*mut libc::passwd, *mut libc::c_char, usize, *mut *mut libc::passwd) -> libc::c_int) -> Option<String> {
    let mut buffer = vec![0 as libc::c_char; 16 * 1024];
    // SAFETY: passwd is plain data, which the lookup fills in, pointing into `buffer`.
    let mut record: libc::passwd = unsafe { std::mem::zeroed() };
    let mut found = std::ptr::null_mut();
    if lookup(&mut record, buffer.as_mut_ptr(), buffer.len(), &mut found) != 0 || found.is_null() || record.pw_dir.is_null() {
        return None;
    }
    // SAFETY: pw_dir points at a NUL-terminated string in `buffer`, which is still alive.
    let home = unsafe { std::ffi::CStr::from_ptr(record.pw_dir) }.to_string_lossy().into_owned();
    (!home.is_empty()).then_some(home)
}
#[cfg(unix)]
fn account_home_of(uid: libc::uid_t) -> Option<String> {
    // SAFETY: getpwuid_r writes only into the record and buffer it's given.
    account_home(|record, buffer, size, found| unsafe { libc::getpwuid_r(uid, record, buffer, size, found) })
}
#[cfg(unix)]
fn account_home_named(name: &str) -> Option<String> {
    let name = std::ffi::CString::new(name).ok()?;
    // SAFETY: as above; `name` is a valid C string.
    account_home(|record, buffer, size, found| unsafe { libc::getpwnam_r(name.as_ptr(), record, buffer, size, found) })
}
/// Whether `home` is a folder of Kumi's own, which an uninstall may take Kumi's parts out of; why not, when it isn't.
pub fn not_kumis_own(home: &str, env: &Env) -> Option<NotKumis> {
    not_kumis_own_for(home, env, &Runner::current(env))
}
/// As [`not_kumis_own`], for `runner`.
pub fn not_kumis_own_for(home: &str, env: &Env, runner: &Runner) -> Option<NotKumis> {
    let folder = Path::new(home);
    if !folder.is_absolute() {
        return Some(NotKumis::Relative);
    }
    let Ok(real) = fs::canonicalize(folder) else { return Some(NotKumis::Missing) };
    let homes = [env.get("HOME").cloned(), env.get("USERPROFILE").cloned(), home::home_dir().map(|home| home.display().to_string())];
    let homes = homes.into_iter().flatten().chain(runner.homes.iter().cloned());
    if homes.filter(|home| !home.is_empty()).any(|home| same_folder(&real, Path::new(&home))) {
        return Some(NotKumis::Home);
    }
    if real.parent().is_none() || mount_point(&real) {
        return Some(NotKumis::Root);
    }
    let launcher = real.join("bin").join(if cfg!(windows) { "kumi.cmd" } else { "kumi" });
    let names_kumi = |app: &str| {
        let package = fs::read(real.join(app).join("package.json")).ok().and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok());
        package.is_some_and(|package| package["name"] == "kumi")
    };
    if !launcher.is_file() || !(names_kumi("app") || names_kumi("app.previous")) {
        return Some(NotKumis::Unmarked);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if runner.root && fs::metadata(&real).is_ok_and(|meta| meta.uid() != 0) {
            return Some(NotKumis::NotRoots);
        }
    }
    None
}
/// Whether two paths name one folder, however each is spelled (case, links, a trailing separator).
fn same_folder(a: &Path, b: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if let (Ok(a), Ok(b)) = (fs::metadata(a), fs::metadata(b)) {
            return a.dev() == b.dev() && a.ino() == b.ino();
        }
    }
    fs::canonicalize(a).ok().zip(fs::canonicalize(b).ok()).is_some_and(|(a, b)| a == b)
}
/// Whether a disk is mounted at `path`.
fn mount_point(path: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if let (Ok(here), Some(Ok(above))) = (fs::metadata(path), path.parent().map(fs::metadata)) {
            return here.dev() != above.dev();
        }
    }
    let _ = path;
    false
}
/// `path` as the producer reads it, with `home` (their home folder) as ~.
fn tilde_in(path: &str, home: &str) -> String {
    let home = home.trim_end_matches(['/', '\\']);
    match path.strip_prefix(home) {
        Some(rest) if !home.is_empty() && (rest.is_empty() || rest.starts_with(['/', '\\'])) => format!("~{rest}"),
        _ => path.into(),
    }
}
/// The producer's home folder, as `env` has it.
fn user_home(env: &Env) -> String {
    let actual = home::home_dir().unwrap_or_default().display().to_string();
    env.get("HOME").or_else(|| env.get("USERPROFILE")).filter(|s| !s.is_empty()).cloned().unwrap_or(actual)
}
/// Why an uninstall won't take anything out of `home`, and Kumi's own parts there to take out by hand.
fn not_kumis_said(home: &str, why: NotKumis, display: &dyn Fn(&str) -> String) -> String {
    let shown = display(home);
    let names = "so the names Kumi uses there (bin, downloads, tools…) could be your own";
    let mut said = match why {
        // A ~ in KUMI_HOME is only a shell's to turn into the home folder: as Kumi reads it, it's a folder named ~.
        NotKumis::Relative if home.starts_with('~') => {
            return format!(
                "Kumi won't uninstall from \"{home}\": KUMI_HOME starts with ~, which only a shell turns into your home folder. Set KUMI_HOME to the whole path of Kumi's folder and run kumi uninstall again. Nothing was removed."
            )
        }
        NotKumis::Relative => {
            return format!(
                "Kumi won't uninstall from \"{home}\": KUMI_HOME is a relative path, so where it points depends on the folder you're in. Set it to the whole path of Kumi's folder and run kumi uninstall again. Nothing was removed."
            )
        }
        NotKumis::Missing => return format!("There's no {shown} (Kumi's folder) to uninstall Kumi from. Nothing was removed."),
        NotKumis::Home => format!("Kumi won't uninstall from {shown}: it's your home folder, {names}."),
        NotKumis::Root => format!("Kumi won't uninstall from {shown}: it's the root of a disk, {names}."),
        NotKumis::Unmarked => format!("Kumi won't uninstall from {shown}: it holds no Kumi launcher (bin/kumi) and app, {names}."),
        NotKumis::NotRoots => {
            return format!(
                "Kumi won't uninstall from {shown} as root (sudo): it belongs to another account. Run kumi uninstall without sudo. Nothing was removed."
            )
        }
    };
    said.push_str(" Nothing was removed.");
    let mut parts: Vec<String> = ["app", "app.previous", "app.previous.old", "app.new", "app.replaced", "app.rollback"]
        .into_iter()
        .map(|part| join(home, part))
        .filter(|part| has_app(part))
        .collect();
    let launcher = join(&join(home, "bin"), if cfg!(windows) { "kumi.cmd" } else { "kumi" });
    if fs::read_to_string(&launcher).is_ok_and(|text| text.contains("KUMI_INSTALLED")) {
        parts.push(launcher);
    }
    if !parts.is_empty() {
        let shown: Vec<String> = parts.iter().map(|part| display(part)).collect();
        let path = if cfg!(windows) {
            format!("{} from your PATH", display(&join(home, "bin")))
        } else {
            format!("the two lines under \"{PATH_MARKER}\" in your shell's startup file")
        };
        said.push_str(&format!(" Kumi's own there, to take out by hand: {}, and {path}.", listed(&shown)));
    }
    said
}
/// What Kumi keeps in its folder besides itself, by name: the producer's conversations, notes, recipes, sign-ins,
/// library, music and references, and what Kumi remembers between runs. Only --all takes these, and only from a folder of
/// Kumi's alone: elsewhere, names like projects and settings.json could be the producer's.
const KUMI_S_DATA: [&str; 29] = [
    "auth.json",
    "settings.json",
    "kumi.db",
    "kumi.db-wal",
    "kumi.db-shm",
    "kumi.db-journal",
    "memory.json",
    "techniques.json",
    "playbook.json",
    "audition-restore.json",
    "gaps.jsonl",
    "timings.jsonl",
    "input-history",
    "update-check.json",
    "whats-new.json",
    // The producer's answer about their music, beside settings.json.
    "music-answer.json",
    "slots.json",
    "projects",
    "recipes",
    "library",
    // What Kumi learned of the producer's own music (music.db, its style vectors, the map).
    "music",
    "references",
    "goals",
    "attachments",
    "cache",
    "devices",
    "max",
    "probes",
    // The DAW used last, among several bridges.
    "last-daw",
];
/// Kumi's folders whose names the producer may use too (`~/.local/bin`, say): only Kumi's entries in them go, and the
/// folder once nothing else is left in it.
const SHARED_NAMES: [&str; 7] = ["bin", "downloads", "tools", "models", "videos", "node", "bridge"];
/// What a file browser leaves in a folder.
fn litter(name: &str) -> bool {
    [".DS_Store", "Thumbs.db", "desktop.ini"].contains(&name)
}
/// Whether `name` in Kumi's folder `part` is Kumi's: its launcher, a release it downloaded, a program, model or video
/// it fetched, one of the bridge's generations. Nothing in `node` tells Kumi's old Node from the producer's own, so
/// its entries count as Kumi's only in a folder of Kumi's alone.
fn kumis_entry(part: &str, name: &str, path: &Path, alone: bool) -> bool {
    use kumi_ai::models::pinned::{AFX_REP, BASIC_PITCH, CLAP, CLAP_MERGES, CLAP_TEXT, CLAP_VOCAB};
    let inside = |folder: &Path| -> Vec<String> {
        fs::read_dir(folder)
            .into_iter()
            .flatten()
            .filter_map(Result::ok)
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .collect()
    };
    litter(name)
        || match part {
            "bin" => ["kumi", "kumi.cmd"].contains(&name),
            // A release's bundle, by Kumi's own names: kumi-<target>.tar.gz or .zip for the computers it's built for
            // (aarch64-apple-darwin, x86_64-pc-windows-msvc…), kumi.tar.gz before targets.
            "downloads" => {
                name == "kumi.tar.gz"
                    || name
                        .strip_prefix("kumi-")
                        .and_then(|rest| {
                            rest.strip_suffix(".tar.gz").or_else(|| rest.strip_suffix(".tar.xz")).or_else(|| rest.strip_suffix(".zip"))
                        })
                        .is_some_and(|target| {
                            let parts: Vec<&str> = target.split('-').collect();
                            parts.len() >= 3
                                && ["x86_64", "aarch64"].contains(&parts[0])
                                && parts.iter().all(|part| !part.is_empty() && part.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'))
                        })
            }
            "tools" => {
                ["ffmpeg", "whisper", "whisper-models", "yt-dlp", "hands"].contains(&name)
                    || [".ffmpeg-", ".whisper-", ".yt-dlp"].iter().any(|start| name.starts_with(start))
            }
            "models" => {
                [CLAP.name, CLAP_TEXT.name, CLAP_VOCAB.name, CLAP_MERGES.name, AFX_REP.name, BASIC_PITCH.name, "slots"].contains(&name)
                    || name.starts_with("onnxruntime")
                    || name.contains(".partial-")
                    || name.contains(".unpacking-")
            }
            // A video's folder, named as Kumi names them (its site and id, in lowercase), with Kumi's files in it.
            "videos" => {
                path.is_dir()
                    && name.contains('-')
                    && name.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')
                    && inside(path).iter().all(|entry| {
                        litter(entry)
                            || ["meta.json", "cues.json", "captions", "frames", "sound"].contains(&entry.as_str())
                            || entry.starts_with("speech-")
                    })
            }
            // A generation (its version, then when it went in: 1.0.92-1791674943798), and the state beside them.
            "bridge" => {
                name == "state"
                    || (name.starts_with(|c: char| c.is_ascii_digit())
                        && name.contains('-')
                        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-'))
            }
            _ => alone,
        }
}
/// The names in `home`, in KUMI_S_OWN's order, then the rest by name.
fn names_in(home: &str) -> Result<Vec<String>, RuntimeError> {
    let mut names: Vec<String> = match fs::read_dir(home) {
        Ok(entries) => entries
            .map(|entry| entry.map(|entry| entry.file_name().to_string_lossy().into_owned()))
            .collect::<Result<_, _>>()
            .map_err(error)?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => vec![],
        Err(e) => return Err(error(e)),
    };
    names.sort_by_key(|name| (KUMI_S_OWN.iter().position(|own| own == name).unwrap_or(KUMI_S_OWN.len()), name.clone()));
    Ok(names)
}
/// Whether `home` is a folder of Kumi's alone: named as the installer names it (.kumi), once links are followed (a
/// ~/.kumi linking to ~/.local is ~/.local). Then --all takes Kumi's data, and the folder once it's empty. Kumi's
/// names (bin, downloads, projects, settings.json…) are too common to tell a folder of Kumi's by what's in it.
fn kumis_alone(home: &str) -> bool {
    fs::canonicalize(home).ok().is_some_and(|real| real.file_name().is_some_and(|name| name == ".kumi"))
}
/// What an uninstall does in `home`, a folder of Kumi's own.
struct Parts {
    /// What goes: Kumi's parts by their exact names as the folder lists them, what a stopped installer left
    /// (`.install.*`), and in the folders whose names the producer may use too, only Kumi's entries (the folder
    /// itself once nothing else is in it). With --all in a folder of Kumi's alone, Kumi's data too (the bridge's
    /// files excepted while Live uses them). In KUMI_S_OWN's order.
    gone: Vec<String>,
    /// Kumi's folders that stay because they hold more than Kumi put there.
    left: Vec<String>,
    /// Whether `home` is a folder of Kumi's alone.
    alone: bool,
    /// Kumi's data that stays: without --all, or in a folder the producer also uses.
    data: Vec<String>,
    /// The copies of Kumi's DAW bridges the producer had changed, which the lifecycle kept for them: they stay.
    changed: Vec<String>,
    /// What else is in `home`, by names Kumi doesn't make: it stays, and the folder with it.
    others: Vec<String>,
}
impl Parts {
    /// Whether `home` is a folder of Kumi's alone with nothing of anyone else's in it: then it's Kumi's to name whole.
    fn clean(&self) -> bool {
        self.alone && self.only_kumis()
    }
    /// Whether all that's in `home` is Kumi's, whatever its name.
    fn only_kumis(&self) -> bool {
        self.left.is_empty() && self.others.is_empty() && self.changed.is_empty()
    }
}
/// `kept`: the folders of DAWs' bridge files that stay, as their DAWs use them.
fn own_parts(home: &str, all: bool, kept: &[&str]) -> Result<Parts, RuntimeError> {
    let names = names_in(home)?;
    let alone = kumis_alone(home);
    let mut parts = Parts { gone: vec![], left: vec![], alone, data: vec![], changed: vec![], others: vec![] };
    for name in names {
        let path = join(home, &name);
        if kept.contains(&name.as_str()) {
            continue;
        }
        if SHARED_NAMES.contains(&name.as_str()) {
            let Ok(entries) = fs::read_dir(&path) else {
                // Not a folder: Kumi makes none by these names.
                if alone {
                    parts.gone.push(path);
                }
                continue;
            };
            let (kumis, others): (Vec<_>, Vec<_>) = entries
                .filter_map(Result::ok)
                .map(|entry| (entry.file_name().to_string_lossy().into_owned(), entry.path()))
                .partition(|(entry, at)| kumis_entry(&name, entry, at, alone));
            if others.is_empty() {
                parts.gone.push(path);
            } else {
                parts.gone.extend(kumis.into_iter().filter(|(entry, _)| !litter(entry)).map(|(_, at)| at.display().to_string()));
                parts.left.push(path);
            }
        } else if name == "bridges" {
            bridges_parts(&path, all && alone, &mut parts);
        } else if name.starts_with(".install.") || KUMI_S_OWN.contains(&name.as_str()) {
            parts.gone.push(path);
        } else if KUMI_S_DATA.contains(&name.as_str()) {
            if all && alone {
                parts.gone.push(path);
            } else {
                parts.data.push(path);
            }
        } else if litter(&name) {
            if all && alone {
                parts.gone.push(path);
            }
        } else {
            parts.others.push(path);
        }
    }
    Ok(parts)
}
/// What goes of `bridges`, a folder per DAW (`all`: --all, in a folder of Kumi's alone): Kumi's own entries; the
/// baselines too with `all`; never the copies kept for the producer, or what Kumi doesn't make. A DAW's folder goes
/// whole once all in it goes, and so does `bridges`; else their entries go one by one, and the folders stay. Only
/// REAPER's and Renoise's folders are Kumi's, and in a folder the producer also uses only those its lifecycle wrote
/// ([`BRIDGES_MARKS`]): a `bridges` there may be someone else's, and stays unless Kumi's folders were all it held.
fn bridges_parts(bridges: &str, all: bool, parts: &mut Parts) {
    let Ok(daws) = fs::read_dir(bridges) else {
        // Not a folder: Kumi makes none by this name.
        parts.others.push(bridges.to_string());
        return;
    };
    let kumis = |name: &str| {
        BRIDGES_OWN.contains(&name)
            || name.strip_prefix('.').and_then(|rest| rest.split_once(".tmp-")).is_some_and(|(of, _)| BRIDGES_OWN.contains(&of))
    };
    let (mut whole, mut gone) = (true, vec![]);
    for daw in daws.filter_map(Result::ok) {
        let folder = daw.path().display().to_string();
        let name = daw.file_name().to_string_lossy().into_owned();
        let kumis_folder = daw_bridge::daw::Daw::from_id(&name).is_some()
            && (parts.alone || BRIDGES_MARKS.iter().any(|mark| daw.path().join(mark).exists()));
        let entries = fs::read_dir(daw.path()).ok().filter(|_| kumis_folder);
        let Some(entries) = entries else {
            if !litter(&name) {
                parts.others.push(folder);
                whole = false;
            }
            continue;
        };
        let (mut all_of_it, mut entries_gone) = (true, vec![]);
        for entry in entries.filter_map(Result::ok) {
            let name = entry.file_name().to_string_lossy().into_owned();
            let at = entry.path().display().to_string();
            match name.as_str() {
                _ if litter(&name) => {}
                _ if kumis(&name) => entries_gone.push(at),
                BRIDGES_DATA if all => entries_gone.push(at),
                BRIDGES_DATA => {
                    parts.data.push(at);
                    all_of_it = false;
                }
                BRIDGES_CHANGED => {
                    parts.changed.push(at);
                    all_of_it = false;
                }
                _ => {
                    parts.others.push(at);
                    all_of_it = false;
                }
            }
        }
        if all_of_it {
            gone.push(folder);
        } else {
            whole = false;
            gone.extend(entries_gone);
        }
    }
    if whole && (parts.alone || !gone.is_empty()) {
        parts.gone.push(bridges.to_string());
    } else if whole {
        parts.others.push(bridges.to_string());
    } else {
        parts.gone.extend(gone);
    }
}
/// What's said of the copies of Kumi's DAW bridges the producer had changed, which stay.
fn changed_said(parts: &Parts, display: &dyn Fn(&str) -> String, would: bool) -> Option<String> {
    let shown: Vec<String> = parts.changed.iter().map(|path| display(path)).collect();
    let stay = match (would, shown.len()) {
        (_, 0) => return None,
        (true, _) => "would stay",
        (false, _) => "stay",
    };
    Some(format!("The copies of Kumi's bridges you had changed {stay} in {}: delete them yourself if you don't need them.", listed(&shown)))
}
/// What's said of what stays in a folder of Kumi's alone by names Kumi doesn't make.
fn others_said(home: &str, parts: &Parts, display: &dyn Fn(&str) -> String) -> Option<String> {
    let names: Vec<String> = parts.others.iter().map(|path| part_name(home, path)).collect();
    match names.len() {
        _ if !parts.alone => None,
        0 => None,
        1 => Some(format!("{} stays in {}: it isn't Kumi's.", names[0], display(home))),
        _ => Some(format!("{} stay in {}: they aren't Kumi's.", listed(&names), display(home))),
    }
}
/// Whether `folder` holds nothing but what a file browser leaves.
fn emptied(folder: &str) -> bool {
    fs::read_dir(folder).is_ok_and(|entries| entries.filter_map(Result::ok).all(|entry| litter(&entry.file_name().to_string_lossy())))
}
/// `path`, in `home`, as the part of Kumi's folder it is (`app`, `bin/kumi`).
fn part_name(home: &str, path: &str) -> String {
    Path::new(path).strip_prefix(home).map(|part| part.display().to_string()).unwrap_or_else(|_| basename(path))
}
/// The part of Kumi's folder `path` is in: `bin` for `bin/kumi`.
fn top_part(home: &str, path: &str) -> String {
    let part = part_name(home, path);
    Path::new(&part).components().next().map(|first| first.as_os_str().to_string_lossy().into_owned()).unwrap_or(part)
}
/// The bytes in `path`, its folders walked without following links.
fn bytes_in(path: &Path) -> u64 {
    match fs::symlink_metadata(path) {
        Ok(meta) if meta.is_dir() => {
            fs::read_dir(path).into_iter().flatten().filter_map(Result::ok).map(|entry| bytes_in(&entry.path())).sum()
        }
        Ok(meta) => meta.len(),
        Err(_) => 0,
    }
}
fn listed(words: &[String]) -> String {
    match words {
        [] => String::new(),
        [one] => one.clone(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}
/// What's said of the fetched folders that stay because they hold more than Kumi put there.
fn left_said(left: &[String], display: &dyn Fn(&str) -> String) -> Option<String> {
    let shown: Vec<String> = left.iter().map(|path| display(path)).collect();
    match shown.len() {
        0 => None,
        1 => Some(format!("{} stays: it holds more than Kumi put there.", shown[0])),
        _ => Some(format!("{} stay: they hold more than Kumi put there.", listed(&shown))),
    }
}
/// Where Kumi's data stays, by what of it is there: the folder, or in one that also holds what isn't Kumi's, each name.
fn data_said(home: &str, parts: &Parts, display: &dyn Fn(&str) -> String) -> String {
    if parts.clean() {
        return display(home);
    }
    let names: Vec<String> = parts.data.iter().map(|path| part_name(home, path)).collect();
    if parts.only_kumis() {
        return format!("{} ({})", display(home), listed(&names));
    }
    format!("{}, beside what isn't Kumi's ({})", display(home), listed(&names))
}
/// `kumi uninstall --dry-run`: what would go, and what would stay.
fn dry_run(io: &InstalledIo, home: &str, options: UninstallOptions, display: &dyn Fn(&str) -> String) -> Result<i32, RuntimeError> {
    let say = |s: String| io.out.write(&format!("{s}\n"));
    if let Some(folder) = not_yours(home) {
        say(format!(
            "It would stop before removing anything: {} belongs to another account (installed with sudo?). Run this first: {}",
            display(&folder),
            chown_line(home)
        ));
    }
    // The DAWs whose bridge is in them, and the bridge files they use, which stay.
    let bridges: Vec<_> = crate::daw::providers().into_iter().filter(|provider| provider.bridge_installed(&io.env)).collect();
    let kept: Vec<&str> = bridges
        .iter()
        .filter(|provider| provider.bridge_files_in_use(&join(home, provider.bridge_folder()), &io.env))
        .map(|provider| provider.bridge_folder())
        .collect();
    let parts = own_parts(home, options.all, &kept)?;
    let bytes: u64 = parts.gone.iter().map(|part| bytes_in(Path::new(part))).sum();
    if options.all && parts.clean() {
        say(format!("kumi uninstall --all would remove {} ({}), with your {KEPT}.", display(home), size(bytes as f64)));
    } else if options.all && parts.alone {
        say(format!(
            "kumi uninstall --all would remove everything of Kumi's from {} ({}), with your {KEPT}.",
            display(home),
            size(bytes as f64)
        ));
    } else if parts.gone.is_empty() {
        say(format!("kumi uninstall would find nothing of Kumi's own left in {}.", display(home)));
    } else {
        let names: Vec<String> = parts.gone.iter().map(|part| part_name(home, part)).collect();
        say(format!("kumi uninstall would remove from {} ({}): {}.", display(home), size(bytes as f64), names.join(", ")));
    }
    if let Some(said) = left_said(&parts.left, display) {
        say(said);
    }
    if let Some(said) = changed_said(&parts, display, true) {
        say(said);
    }
    if let Some(said) = others_said(home, &parts, display) {
        say(said);
    }
    let lines = path_lines(&io.env, home);
    let files: Vec<String> =
        lines.iter().filter(|lines| !lines.removed.is_empty() && can_change(lines)).map(|lines| display(&lines.file)).collect();
    let mut also = vec![];
    let bin = join(home, "bin");
    if cfg!(windows) && parts.left.contains(&bin) {
        say(format!("{} would stay in your PATH, as it holds more than Kumi put there.", display(&bin)));
    } else if cfg!(windows) {
        also.push("take Kumi out of your PATH".to_string());
    } else if !files.is_empty() {
        also.push(format!("take Kumi out of {}", listed(&files)));
    }
    for provider in &bridges {
        also.push(provider.removal_question().into());
    }
    if !also.is_empty() {
        say(format!("It would also {}.", also.join(", and ")));
    }
    for lines in &lines {
        if !lines.removed.is_empty() && !can_change(lines) {
            say(format!("It couldn't change {} (it isn't writable): take out {} by hand.", display(&lines.file), by_hand(&lines.removed)));
        }
        if !lines.left.is_empty() {
            say(format!(
                "It would leave Kumi's PATH line in {} alone ({}): it sits inside an if, for or function there.",
                display(&lines.file),
                by_hand(&lines.left)
            ));
        }
    }
    if options.all && !parts.alone && !parts.data.is_empty() {
        let names: Vec<String> = parts.data.iter().map(|path| part_name(home, path)).collect();
        let why = if parts.only_kumis() { "isn't named .kumi" } else { "also holds what isn't Kumi's" };
        say(format!("{} {why}, so --all would leave your {KEPT} there too ({}): delete them by hand.", display(home), listed(&names)));
    } else if !options.all && (parts.alone || !parts.data.is_empty()) {
        say(format!("Your {KEPT} would stay in {}.", data_said(home, &parts, display)));
    }
    say("Nothing was removed (--dry-run).".into());
    Ok(0)
}
/// Removes each of `parts`; the ones that stayed, with why.
#[cfg(not(windows))]
fn removing(parts: &[String]) -> Vec<(String, std::io::Error)> {
    parts.iter().filter_map(|path| remove(path).err().map(|error| (path.clone(), error))).collect()
}
pub async fn uninstall_installed(io: InstalledIo, options: UninstallOptions) -> Result<i32, RuntimeError> {
    let say = |s: String| io.out.write(&format!("{s}\n"));
    let home = kumi_home(&io.env);
    let user = user_home(&io.env);
    let display = |path: &str| tilde_in(path, &user);
    // Before any question, any change, or any look inside: a folder that isn't Kumi's own keeps everything in it.
    let runner = io.runner.clone().unwrap_or_else(|| Runner::current(&io.env));
    if let Some(why) = not_kumis_own_for(&home, &io.env, &runner) {
        say(not_kumis_said(&home, why, &display));
        return Ok(1);
    }
    if options.dry_run {
        return dry_run(&io, &home, options, &display);
    }
    // A folder another account owns can't lose its parts: said before anything goes, so Kumi isn't left half removed.
    if not_yours(&home).is_some() {
        say(format!("{} Nothing was removed.", owner_advice(&home)));
        return Ok(1);
    }
    let run = io.run.clone().unwrap_or_else(default_run);
    let before = own_parts(&home, options.all, &[])?;
    let keeps = match (options.all, before.alone) {
        (true, true) if before.only_kumis() => format!("Your {KEPT} go too."),
        (true, true) => format!("Your {KEPT} go too; what isn't Kumi's in it stays."),
        (true, false) if before.only_kumis() => format!("It isn't named .kumi, so your {KEPT} stay there too."),
        (true, false) => format!("It also holds what isn't Kumi's, so your {KEPT} stay there too."),
        (false, true) => format!("Your {KEPT} stay (add --all to remove them too)."),
        (false, false) => format!("Your {KEPT} stay."),
    };
    if !options.yes && !ask(&io, &format!("Remove Kumi from {}? {keeps}", display(&home)), Some("add --yes")).await {
        say("Nothing was removed.".into());
        return Ok(1);
    }
    // Each DAW's bridge, taken out when the producer says so; the files of one that stays stay too, while it uses them.
    let mut staying = vec![];
    let daw = DawIo { run: run.clone(), ..io.daw() };
    for provider in crate::daw::providers() {
        // One DAW's bridge that can't come out stays, said; the rest of Kumi still goes.
        let removed = match provider.remove_bridge(&daw).await {
            Ok(removed) => removed,
            // (whether its files stay is said below, for a DAW that still uses them)
            Err(why) => {
                say(format!("Kumi couldn't take its bridge out of {}: {why}.", provider.profile().words.app));
                RemovedBridge::Kept
            }
        };
        if removed != RemovedBridge::Removed && provider.bridge_files_in_use(&join(&home, provider.bridge_folder()), &io.env) {
            staying.push(provider);
        }
    }
    let kept: Vec<&str> = staying.iter().map(|provider| provider.bridge_folder()).collect();
    let bridge_stays = !staying.is_empty();
    let parts = own_parts(&home, options.all, &kept)?;
    let whole = options.all && parts.alone && !bridge_stays;
    let bytes: u64 = parts.gone.iter().map(|part| bytes_in(Path::new(part))).sum();
    // Every part is tried, Kumi itself last: its app, then its launcher. When another part stays, Kumi does too, and
    // while its app stays, so do the launcher and its PATH lines, so kumi uninstall can run again once that's sorted.
    #[cfg(not(windows))]
    let (stayed, kumi_stays) = {
        let (app, rest): (Vec<String>, Vec<String>) = parts.gone.iter().cloned().partition(|path| top_part(&home, path) == "app");
        let (bin, rest): (Vec<String>, Vec<String>) = rest.into_iter().partition(|path| top_part(&home, path) == "bin");
        let mut stayed = removing(&rest);
        let kumi_stays = !stayed.is_empty();
        if !kumi_stays {
            stayed = removing(&app);
            if stayed.is_empty() {
                stayed = removing(&bin);
            }
            // The folder too, once it's empty: what isn't Kumi's keeps it.
            if stayed.is_empty() && whole && emptied(&home) {
                stayed.extend(remove(&home).err().map(|error| (home.clone(), error)));
            }
        }
        (stayed, kumi_stays)
    };
    #[cfg(windows)]
    let (stayed, kumi_stays): (Vec<(String, std::io::Error)>, bool) = {
        // Windows won't remove a running program: a detached cmd does, once this Kumi has exited.
        use std::os::windows::process::CommandExt;
        let mut list: Vec<String> =
            parts.gone.iter().map(|path| format!("rmdir /s /q \"{path}\" 2>nul & del /f /q \"{path}\" 2>nul")).collect();
        // The folder too, once it's empty (rmdir without /s takes only an empty one).
        if whole {
            list.push(format!("rmdir \"{home}\" 2>nul"));
        }
        let list = list.join(" & ");
        std::process::Command::new(std::env::var("ComSpec").unwrap_or_else(|_| "cmd.exe".into()))
            .raw_arg(format!("/d /s /c \"ping -n 4 127.0.0.1 >nul & {list}\""))
            .creation_flags(0x00000008 | 0x00000200 | 0x08000000)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(error)?;
        (vec![], false)
    };
    // The startup files last, and only once Kumi's launcher has gone: until then its PATH line still finds Kumi.
    let launcher = join(&join(&home, "bin"), if cfg!(windows) { "kumi.cmd" } else { "kumi" });
    let rc_changed = if Path::new(&launcher).exists() && !cfg!(windows) { true } else { remove_path_lines(&io, &home) };
    // On Windows, PATH names Kumi's bin folder itself, which stays while it holds the producer's own (uv's
    // .local\bin, say): so does its PATH entry.
    let bin = join(&home, "bin");
    if cfg!(windows) && parts.left.contains(&bin) {
        say(format!("{} stays in your PATH, as it holds more than Kumi put there.", display(&bin)));
    } else if cfg!(windows) {
        step(io.out.clone(), &io.env, "Taking Kumi out of your PATH…", remove_windows_path(run, &home), false).await;
    }
    for provider in &staying {
        say(provider.files_stay(&display(&join(&home, provider.bridge_folder()))));
    }
    if let Some(said) = left_said(&parts.left, &display) {
        say(said);
    }
    if let Some(said) = changed_said(&parts, &display, false) {
        say(said);
    }
    if let Some(said) = others_said(&home, &parts, &display) {
        say(said);
    }
    if let Some((_, why)) = stayed.first() {
        let names: Vec<String> = stayed.iter().map(|(path, _)| display(path)).collect();
        let them = if names.len() == 1 { "it" } else { "them" };
        let owner = if stayed.iter().any(|(_, error)| error.kind() == std::io::ErrorKind::PermissionDenied) {
            format!(" (if another account owns {them}: {})", chown_line(&home))
        } else {
            String::new()
        };
        let next = if kumi_stays {
            format!("Kumi itself stays, so you can run kumi uninstall again once that's sorted{owner}, or take {them} out by hand.")
        } else if Path::new(&launcher).exists() {
            let lines = format!("the two lines under \"{PATH_MARKER}\" in your shell's startup file");
            let by_hand = format!("take out {}, the launcher and {lines} by hand", listed(&names));
            // kumi uninstall runs again only with Kumi's program still in app, from a folder that still passes as
            // Kumi's own: has_app would count what's left of an older Kumi's files.
            let runs = Path::new(&join(&join(&home, "app"), &executable_name("kumi"))).is_file()
                && not_kumis_own_for(&home, &io.env, &runner).is_none();
            if runs {
                format!(
                    "Kumi's launcher ({}) and PATH lines stay until then: once that's sorted{owner}, run kumi uninstall again, or {by_hand}.",
                    display(&launcher)
                )
            } else {
                // What's left wouldn't run kumi uninstall, or would refuse it: the way on is by hand.
                format!(
                    "Kumi's launcher ({}) and PATH lines stay, but what's left can't uninstall itself: {by_hand}{owner}.",
                    display(&launcher)
                )
            }
        } else {
            format!("Take {them} out by hand{owner}.")
        };
        say(format!("Kumi is partly removed: {} couldn't go ({why}). {next}", listed(&names)));
        return Ok(1);
    }
    let freed = if bytes > 0 { format!(", freeing {}", size(bytes as f64)) } else { String::new() };
    say(if options.all && parts.alone {
        let rest = if parts.only_kumis() { String::new() } else { format!(" {} stays, with what isn't Kumi's in it.", display(&home)) };
        format!("Kumi is removed, with everything it kept{}{freed}.{rest}", if bridge_stays { " but the bridge's files" } else { "" })
    } else if parts.clean() {
        format!("Kumi is removed{freed}. Your {KEPT} are still in {}; delete that folder to remove them too.", display(&home))
    } else if parts.data.is_empty() {
        format!("Kumi is removed{freed}.")
    } else {
        format!(
            "Kumi is removed{freed}. Your {KEPT} are still in {}: delete them by hand to remove them too.",
            data_said(&home, &parts, &display)
        )
    });
    say("Open a new terminal window so the `kumi` command is gone there too.".into());
    Ok(i32::from(!rc_changed))
}
pub async fn check_release(env: &Env, fetcher: Option<Rc<dyn Fetch>>) -> Result<Option<String>, RuntimeError> {
    match ask_newest(env, fetcher.unwrap_or_else(default_fetch)).await {
        (_, AskedRelease::Manifest(manifest)) => Ok(newer_version(&manifest.kumi, KUMI_VERSION).then_some(manifest.kumi)),
        (base, asked) => Err(RuntimeError::plain(unasked(&asked, &base, None, env))),
    }
}
pub async fn newer_release(cache_file: &str, env: &Env, now: Option<f64>, fetcher: Option<Rc<dyn Fetch>>) -> Option<String> {
    let now = now.unwrap_or_else(|| now_ms() as f64);
    if let Some(cached) = fs::read(cache_file).ok().and_then(|b| serde_json::from_slice::<Value>(&b).ok()) {
        if cached.get("checkedAt").and_then(Value::as_f64).is_some_and(|at| now - at < 86400000. && now >= at) {
            return cached.get("latest").and_then(Value::as_str).filter(|latest| newer_version(latest, KUMI_VERSION)).map(str::to_string);
        }
    }
    let manifest = fetch_manifest(env, fetcher).await?;
    let mut options = fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    if let Ok(mut file) = options.open(cache_file) {
        use std::io::Write;
        let _ = file.write_all(stringify(&json!({"checkedAt":now,"latest":manifest.kumi})).as_bytes());
    }
    newer_version(&manifest.kumi, KUMI_VERSION).then_some(manifest.kumi)
}

#[cfg(test)]
mod swap_tests {
    use super::*;
    use std::cell::RefCell;
    use std::path::PathBuf;

    /// What a move does once it's let through.
    #[derive(Clone, Copy)]
    enum Then {
        Moves,
        /// Leaves part of the folder behind and fails, as a move between disks can.
        LeavesPart,
        /// Goes through, then reports an error anyway.
        MovesButFails,
    }
    /// The disk, failing moves the ways Windows can: busy for some tries (or every one) while a Kumi window or an
    /// antivirus scan holds a folder, then what the move does once it's let through.
    #[derive(Default)]
    struct Windows(RefCell<Vec<(&'static str, &'static str, usize, Then)>>);
    impl Windows {
        fn holds(self, from: &'static str, to: &'static str, tries: usize, then: Then) -> Self {
            self.0.borrow_mut().push((from, to, tries, then));
            self
        }
    }
    fn name(path: &str) -> String {
        Path::new(path).file_name().unwrap().to_string_lossy().into()
    }
    impl Folders for Windows {
        fn rename(&self, from: &str, to: &str) -> std::io::Result<()> {
            let mut rules = self.0.borrow_mut();
            let Some(rule) = rules.iter_mut().find(|rule| rule.0 == name(from) && rule.1 == name(to)) else {
                return fs::rename(from, to);
            };
            if rule.2 > 0 {
                rule.2 -= 1;
                // A held folder: Windows' sharing violation, or a busy mount elsewhere.
                return Err(std::io::Error::from_raw_os_error(if cfg!(windows) { 32 } else { 16 }));
            }
            match rule.3 {
                Then::Moves => fs::rename(from, to),
                Then::LeavesPart => {
                    fs::create_dir_all(to)?;
                    fs::write(Path::new(to).join("part"), "")?;
                    Err(std::io::Error::other("the move stopped part of the way"))
                }
                Then::MovesButFails => {
                    fs::rename(from, to)?;
                    Err(std::io::Error::other("reported after the move"))
                }
            }
        }
        fn remove(&self, path: &str) -> std::io::Result<()> {
            remove(path)
        }
    }
    const ALWAYS: usize = usize::MAX;

    struct Home(tempfile::TempDir);
    impl Home {
        fn new() -> Self {
            Home(tempfile::tempdir().unwrap())
        }
        fn path(&self, folder: &str) -> String {
            self.0.path().join(folder).to_string_lossy().into()
        }
        /// A whole native Kumi of `version` in `folder`.
        fn kumi(&self, folder: &str, version: &str) -> &Self {
            let folder = PathBuf::from(self.path(folder));
            fs::create_dir_all(&folder).unwrap();
            fs::write(folder.join(executable_name("kumi")), version).unwrap();
            self
        }
        /// The version of the whole Kumi in `folder`: a native one's, or "node" for an earlier Node Kumi.
        fn version(&self, folder: &str) -> Option<String> {
            let folder = self.path(folder);
            if !migration::has_app(&folder) {
                return None;
            }
            Some(fs::read_to_string(join(&folder, &executable_name("kumi"))).unwrap_or_else(|_| "node".into()))
        }
        async fn swap(&self, windows: &Windows) -> std::io::Result<()> {
            self.swap_with(windows).await
        }
        async fn swap_with(&self, folders: &impl Folders) -> std::io::Result<()> {
            swap_in_with(folders, &self.path("app.new"), &self.path("app"), &self.path("app.previous")).await
        }
    }
    /// app holds 1.2, app.previous 1.1 to roll back to, and 1.3 waits in app.new.
    fn updating() -> Home {
        let home = Home::new();
        home.kumi("app", "1.2").kumi("app.previous", "1.1").kumi("app.new", "1.3");
        home
    }

    #[tokio::test(start_paused = true)]
    async fn a_folder_busy_for_a_moment_is_waited_out() {
        let home = updating();
        home.swap(&Windows::default().holds("app.new", "app", 5, Then::Moves)).await.unwrap();
        assert_eq!(home.version("app").as_deref(), Some("1.3"));
        assert_eq!(home.version("app.previous").as_deref(), Some("1.2"));
        assert!(!Path::new(&home.path("app.previous.old")).exists());
    }

    #[tokio::test(start_paused = true)]
    async fn a_new_kumi_that_cant_go_in_leaves_the_kumi_that_was_there_and_the_one_before() {
        let home = updating();
        assert!(home.swap(&Windows::default().holds("app.new", "app", ALWAYS, Then::Moves)).await.is_err());
        assert_eq!(home.version("app").as_deref(), Some("1.2"));
        assert_eq!(home.version("app.previous").as_deref(), Some("1.1"));
        assert!(!Path::new(&home.path("app.previous.old")).exists());
        // The caller clears app.new; the next update goes through.
        home.swap(&Windows::default()).await.unwrap();
        assert_eq!(home.version("app").as_deref(), Some("1.3"));
    }

    #[tokio::test(start_paused = true)]
    async fn part_of_a_move_is_cleared_and_the_kumi_that_was_there_goes_back() {
        let home = updating();
        assert!(home.swap(&Windows::default().holds("app.new", "app", 0, Then::LeavesPart)).await.is_err());
        assert_eq!(home.version("app").as_deref(), Some("1.2"));
        assert!(!Path::new(&home.path("app")).join("part").exists());
        assert_eq!(home.version("app.previous").as_deref(), Some("1.1"));
    }

    #[tokio::test(start_paused = true)]
    async fn a_put_back_busy_past_its_first_wait_is_tried_again() {
        let home = updating();
        // Each rename waits out 21 tries; the put-back's first goes by, its second goes through.
        let windows = Windows::default().holds("app.new", "app", ALWAYS, Then::Moves).holds("app.previous", "app", 25, Then::Moves);
        assert!(home.swap(&windows).await.is_err());
        assert_eq!(home.version("app").as_deref(), Some("1.2"));
        assert_eq!(home.version("app.previous").as_deref(), Some("1.1"));
    }

    #[tokio::test(start_paused = true)]
    async fn an_earlier_node_kumi_is_a_whole_kumi_too() {
        let home = Home::new();
        home.kumi("app.new", "1.3");
        put_file(&home.path("app/apps/kumi/bin/kumi.mjs"));
        assert!(home.swap(&Windows::default().holds("app.new", "app", ALWAYS, Then::Moves)).await.is_err());
        assert_eq!(home.version("app").as_deref(), Some("node"));
        home.swap(&Windows::default()).await.unwrap();
        assert_eq!(home.version("app").as_deref(), Some("1.3"));
        assert_eq!(home.version("app.previous").as_deref(), Some("node"));
    }

    #[tokio::test(start_paused = true)]
    async fn a_first_install_has_nothing_to_keep() {
        let home = Home::new();
        home.kumi("app.new", "1.3");
        home.swap(&Windows::default()).await.unwrap();
        assert_eq!(home.version("app").as_deref(), Some("1.3"));
        assert!(!Path::new(&home.path("app.previous")).exists());
    }

    #[tokio::test(start_paused = true)]
    async fn a_run_after_one_that_gave_up_puts_the_kumi_back_first() {
        // An update that gave up with app emptied: 1.2 in app.previous, and 1.1 aside in app.previous.old.
        let home = Home::new();
        home.kumi("app.previous", "1.2").kumi("app.previous.old", "1.1");
        settle_installed(home.0.path().to_str().unwrap()).await;
        assert_eq!(home.version("app").as_deref(), Some("1.2"));
        assert_eq!(home.version("app.previous").as_deref(), Some("1.1"));
        home.kumi("app.new", "1.3");
        home.swap(&Windows::default()).await.unwrap();
        assert_eq!(home.version("app").as_deref(), Some("1.3"));
        assert_eq!(home.version("app.previous").as_deref(), Some("1.2"));
    }

    #[tokio::test(start_paused = true)]
    async fn a_run_after_one_that_gave_up_keeps_app_previous_while_its_put_back_stays_busy() {
        let home = Home::new();
        home.kumi("app.previous", "1.2").kumi("app.new", "1.3");
        home.swap(&Windows::default().holds("app.previous", "app", ALWAYS, Then::Moves)).await.unwrap();
        assert_eq!(home.version("app").as_deref(), Some("1.3"));
        assert_eq!(home.version("app.previous").as_deref(), Some("1.2"));
    }

    #[tokio::test(start_paused = true)]
    async fn a_move_that_went_through_before_its_error_counts() {
        let home = updating();
        home.swap(&Windows::default().holds("app.new", "app", 0, Then::MovesButFails)).await.unwrap();
        assert_eq!(home.version("app").as_deref(), Some("1.3"));
        assert_eq!(home.version("app.previous").as_deref(), Some("1.2"));
    }

    #[tokio::test(start_paused = true)]
    async fn when_nothing_can_go_back_app_previous_keeps_the_kumi_that_was_there() {
        let home = updating();
        let windows = Windows::default().holds("app.new", "app", ALWAYS, Then::Moves).holds("app.previous", "app", ALWAYS, Then::Moves);
        assert!(home.swap(&windows).await.is_err());
        assert_eq!(home.version("app"), None);
        assert_eq!(home.version("app.previous").as_deref(), Some("1.2"));
        assert_eq!(home.version("app.previous.old").as_deref(), Some("1.1"));
        // Once Windows lets go, the next run puts 1.2 back before anything else, and 1.1 beside it.
        settle_installed(home.0.path().to_str().unwrap()).await;
        assert_eq!(home.version("app").as_deref(), Some("1.2"));
        assert_eq!(home.version("app.previous").as_deref(), Some("1.1"));
    }

    #[tokio::test(start_paused = true)]
    async fn a_wait_for_a_busy_folder_is_said_once() {
        let home = updating();
        let said = std::cell::Cell::new(0);
        let windows = Windows::default().holds("app.new", "app", ALWAYS, Then::Moves);
        let folders = Saying { folders: &windows, say: || said.set(said.get() + 1), said: Default::default() };
        assert!(swap_in_with(&folders, &home.path("app.new"), &home.path("app"), &home.path("app.previous")).await.is_err());
        assert_eq!(said.get(), 1);
        let quiet = updating();
        let folders = Saying { folders: &Windows::default(), say: || said.set(said.get() + 1), said: Default::default() };
        swap_in_with(&folders, &quiet.path("app.new"), &quiet.path("app"), &quiet.path("app.previous")).await.unwrap();
        assert_eq!(said.get(), 1, "a swap that never waits says nothing");
    }

    #[test]
    fn a_failed_update_says_what_holds_the_folder_and_whether_the_kumi_you_had_works() {
        let kept = swap_failed(true, true, "");
        assert!(kept.contains("a Kumi window, or an antivirus scan") && kept.contains("still works") && kept.contains("run update again"));
        let gone = swap_failed(true, false, "");
        assert!(gone.contains("a Kumi window, or an antivirus scan") && !gone.contains("still works") && gone.contains("installer again"));
        assert_eq!(
            swap_failed(false, true, "Directory not empty"),
            "Couldn't put the new Kumi in place (Directory not empty), so this one stays."
        );
        assert!(swap_failed(false, false, "Directory not empty").contains("installer again"));
    }

    /// A folder another account owns (an install run with sudo) never frees up: no waiting, and the update says whose.
    #[cfg(unix)]
    #[tokio::test(start_paused = true)]
    async fn a_folder_this_account_cant_change_isnt_waited_for() {
        struct Theirs;
        impl Folders for Theirs {
            fn rename(&self, _: &str, _: &str) -> std::io::Result<()> {
                Err(std::io::Error::from_raw_os_error(libc::EACCES))
            }
            fn remove(&self, path: &str) -> std::io::Result<()> {
                remove(path)
            }
            fn waiting(&self) {
                panic!("a folder another account owns isn't waited for")
            }
        }
        let home = updating();
        let error = home.swap_with(&Theirs).await.unwrap_err();
        assert!(denied(&error));
        assert_eq!(home.version("app").as_deref(), Some("1.2"));
        let said = not_in_place(&home.path(""), &error);
        assert!(
            said.starts_with("The system refused to change ")
                && said.contains("if something in it was installed with sudo: sudo chown -R $USER"),
            "{said}"
        );
        assert!(said.ends_with(". The Kumi you had stays."), "{said}");
    }

    fn put_file(path: &str) {
        fs::create_dir_all(Path::new(path).parent().unwrap()).unwrap();
        fs::write(path, "").unwrap();
    }
}
