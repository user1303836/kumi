//! Installed Kumi and its native release bundles.
mod migration;
pub use crate::config::kumi_dir as kumi_home;
pub use crate::update::newer as newer_version;
use crate::{
    bridge_setup::{ask_yes_no, bridge_version, default_run, executable_dir, executable_name, is_live_running, AsyncBool, Confirm, Run},
    config::{find_bridge_config, json_files, load_db_file, remote_scripts_dir},
    doctor::read_bridge_server,
    input::TerminalInput,
    live_extension::{extension_data_dir, live_extensions_dir, remove_extension, remove_former_extension, KUMI_EXTENSION_ID},
    spinner::step,
    tui::tty::TtyOutput,
};
use futures::{future::LocalBoxFuture, StreamExt};
use kumi_common::{
    abort,
    js::{json::stringify, string::trim},
    time::now_ms,
};
use kumi_runtime::{
    ai::http::{default_fetch, Fetch, FetchInit},
    core::{errors::RuntimeError, store_import::write_back},
    ears::device::EARS_NAME,
    library::sources::{basename, dirname, join, resolve},
    system::{self, Env, SystemProgram},
    KUMI, KUMI_VERSION,
};
pub use migration::{ensure_native_launcher, finish_legacy_transition, launcher, only_the_host_differs, write_launcher};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{fs, path::Path, process::Stdio, rc::Rc, time::Duration};
pub fn release_base(env: &Env) -> String {
    env.get("KUMI_RELEASES")
        .filter(|s| !s.is_empty())
        .map(String::as_str)
        .unwrap_or("https://github.com/user1303836/kumi/releases/latest/download")
        .trim_end_matches('/')
        .into()
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
    pub runtime: String,
    pub target: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bridge: Option<String>,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AskedRelease {
    Manifest(ReleaseManifest),
    None,
    Offline,
    Invalid,
}
pub async fn ask_release(env: &Env, fetch: Option<Rc<dyn Fetch>>) -> AskedRelease {
    let fetch = fetch.unwrap_or_else(default_fetch);
    let response = match fetch
        .fetch(&format!("{}/kumi-release.json", release_base(env)), FetchInit { signal: Some(abort::timeout(20000)), ..Default::default() })
        .await
    {
        Ok(response) => response,
        Err(_) => return AskedRelease::Offline,
    };
    if response.status == 404 {
        return AskedRelease::None;
    }
    if !response.ok() {
        return AskedRelease::Offline;
    }
    let Ok(value) = response.json().await else { return AskedRelease::Invalid };
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
pub async fn fetch_manifest(env: &Env, fetch: Option<Rc<dyn Fetch>>) -> Option<ReleaseManifest> {
    match ask_release(env, fetch).await {
        AskedRelease::Manifest(manifest) => Some(manifest),
        _ => None,
    }
}
fn unasked(asked: &AskedRelease, env: &Env) -> String {
    match asked {
        AskedRelease::None => format!(
            "There's no Kumi release to get at {} yet; try again later",
            release_base(env).strip_prefix("https://").unwrap_or(&release_base(env))
        ),
        AskedRelease::Offline => "Kumi couldn't reach GitHub to ask; check your internet connection".into(),
        _ => "The latest release's description didn't make sense; try again later".into(),
    }
}
fn error(e: impl std::fmt::Display) -> RuntimeError {
    RuntimeError::plain(e.to_string())
}
#[derive(Clone)]
pub struct InstalledIo {
    pub out: Rc<dyn TtyOutput>,
    pub env: Env,
    pub input: Option<Rc<dyn TerminalInput>>,
    pub run: Option<Run>,
    pub fetcher: Option<Rc<dyn Fetch>>,
    pub live_running: Option<AsyncBool>,
    pub confirm: Option<Confirm>,
    pub update_bridge: Option<Rc<dyn Fn(String) -> LocalBoxFuture<'static, i32>>>,
    /// The producer's Ctrl-C while an update downloads: it stops the download, and nothing changes.
    pub cancel: Option<kumi_common::abort::Signal>,
}
impl InstalledIo {
    pub fn new(out: Rc<dyn TtyOutput>, env: Env) -> Self {
        Self { out, env, input: None, run: None, fetcher: None, live_running: None, confirm: None, update_bridge: None, cancel: None }
    }
}
async fn ask(io: &InstalledIo, question: &str) -> bool {
    if let Some(ask) = &io.confirm {
        ask(question.into()).await
    } else {
        ask_yes_no(io.input.clone(), io.out.clone(), question).await
    }
}
async fn live_open(io: &InstalledIo, run: Run) -> bool {
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
async fn bridge_after(io: &InstalledIo, home: &str, app: &str) -> i32 {
    let say = |text: String| io.out.write(&format!("{text}\n"));
    let Some(config) = find_bridge_config(&io.env) else {
        say(format!("To connect Live, quit Live, then run: {} bridge", *KUMI));
        return 0;
    };
    let server = read_bridge_server(&config).ok();
    let runtime_migration = server.as_ref().is_some_and(|s| !s.native());
    let installed = server.and_then(|s| s.version);
    let bundled = bridge_version(app);
    let Some((installed, bundled)) = installed
        .zip(bundled)
        .filter(|(installed, bundled)| newer_version(bundled, installed) || (runtime_migration && bundled == installed))
    else {
        say("The bridge in Live is up to date.".into());
        return 0;
    };
    say(if runtime_migration {
        format!("The bridge in Live uses JavaScript ({installed}); this Kumi includes the native bridge ({bundled}).")
    } else {
        format!("The bridge in Live is {installed}; this Kumi's is {bundled}.")
    });
    if live_open(io, io.run.clone().unwrap_or_else(default_run)).await {
        say(format!("Quit Live (save your work first), then run: {} bridge", *KUMI));
        return 0;
    }
    if let Some(update) = &io.update_bridge {
        return update(app.into()).await;
    }
    let mut command = tokio::process::Command::new(join(app, &executable_name("kumi")));
    command
        .arg("bridge")
        .current_dir(home)
        .env("KUMI_INSTALLED", "1")
        .env("KUMI_BRIDGE_AFTER", "1")
        .env("KUMI_HOME", home)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());
    command.status().await.ok().and_then(|s| s.code()).unwrap_or(1)
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
/// Renames, waiting out a folder held for a moment (a Kumi window, an antivirus scan) for 5 s.
async fn rename_in(folders: &impl Folders, from: &str, to: &str) -> std::io::Result<()> {
    for attempt in 0..=20 {
        match folders.rename(from, to) {
            Ok(()) => return Ok(()),
            Err(e)
                if attempt < 20
                    && (e.kind() == std::io::ErrorKind::PermissionDenied
                        || (if cfg!(windows) {
                            matches!(e.raw_os_error(), Some(5 | 32))
                        } else {
                            matches!(e.raw_os_error(), Some(1 | 16))
                        })) =>
            {
                folders.waiting();
                tokio::time::sleep(Duration::from_millis(250)).await
            }
            Err(e) => return Err(e),
        }
    }
    unreachable!()
}
pub fn remove(path: &str) -> std::io::Result<()> {
    match fs::symlink_metadata(path) {
        Ok(m) if m.is_dir() => fs::remove_dir_all(path),
        Ok(_) => fs::remove_file(path),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
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
/// What to say when the new Kumi couldn't go in; `kept` says whether the one that was there still works.
fn swap_failed(windows: bool, kept: bool) -> String {
    let what = "Windows kept Kumi's folder busy: a Kumi window, or an antivirus scan of the new files.";
    let installer = "run the installer again (github.com/user1303836/kumi)";
    match (windows, kept) {
        (true, true) => format!("{what} The Kumi you had still works: close every Kumi window, give the scan a minute, then run update again."),
        (true, false) => format!(
            "{what} The Kumi you had couldn't go back in place yet, so kumi won't start: close every Kumi window, give the scan a minute, then {installer}."
        ),
        (false, true) => "Couldn't put the new Kumi in place, so this one stays.".into(),
        (false, false) => format!("Couldn't put the new Kumi in place, and the one you had couldn't go back, so kumi won't start: {installer}."),
    }
}
struct Cleanup(Vec<String>);
impl Cleanup {
    fn finish(&mut self) -> std::io::Result<()> {
        for path in &self.0 {
            remove(path)?
        }
        self.0.clear();
        Ok(())
    }
}
impl Drop for Cleanup {
    fn drop(&mut self) {
        for path in &self.0 {
            let _ = remove(path);
        }
    }
}
async fn download(url: &str, file: &str, fetch: Rc<dyn Fetch>, cancel: Option<abort::Signal>) -> Result<(), RuntimeError> {
    let signal = abort::any(std::iter::once(abort::timeout(600000)).chain(cancel));
    let response = fetch.fetch(url, FetchInit { signal: Some(signal.clone()), ..Default::default() }).await.map_err(error)?;
    if !response.ok() {
        return Err(RuntimeError::plain(format!("the download failed ({})", response.status)));
    }
    let mut bytes = vec![];
    if let Some(mut body) = response.body {
        loop {
            // The body's chunks race the signal too: a stalled connection stops when it fires.
            let chunk = tokio::select! {
                chunk = body.next() => chunk,
                _ = signal.cancelled() => return Err(RuntimeError::plain("the download was stopped")),
            };
            let Some(chunk) = chunk else { break };
            bytes.extend(chunk.map_err(error)?);
        }
    }
    fs::write(file, bytes).map_err(error)
}
pub async fn update_installed(io: InstalledIo) -> Result<i32, RuntimeError> {
    let say = |s: String| io.out.write(&format!("{s}\n"));
    let home = kumi_home(&io.env);
    let app = join(&home, "app");
    settle_installed(&home).await;
    let run = io.run.clone().unwrap_or_else(default_run);
    let fetch = io.fetcher.clone().unwrap_or_else(default_fetch);
    let asked = step(io.out.clone(), &io.env, "Looking for a newer Kumi…", ask_release(&io.env, Some(fetch.clone())), true).await;
    let AskedRelease::Manifest(manifest) = asked else {
        say(format!("{}.", unasked(&asked, &io.env)));
        return Ok(1);
    };
    if !newer_version(&manifest.kumi, KUMI_VERSION) {
        say(format!("Kumi is up to date ({KUMI_VERSION})."));
        return Ok(bridge_after(&io, &home, &app).await);
    }
    if manifest.target != native_target() {
        say(format!(
            "Kumi {} is for {}, but this Kumi is for {}. Run the installer again for this computer (github.com/user1303836/kumi).",
            manifest.kumi,
            manifest.target,
            native_target()
        ));
        return Ok(1);
    }
    let downloads = join(&home, "downloads");
    tokio::fs::create_dir_all(&downloads).await.map_err(error)?;
    let bundle = join(&downloads, &manifest.bundle);
    let fresh = join(&home, "app.new");
    let mut cleanup = Cleanup(vec![fresh.clone(), bundle.clone()]);
    let result: Result<i32, RuntimeError> = async {
        let downloaded = step(
            io.out.clone(),
            &io.env,
            &format!("Downloading Kumi {}…", manifest.kumi),
            download(&format!("{}/{}", release_base(&io.env), manifest.bundle), &bundle, fetch, io.cancel.clone()),
            true,
        )
        .await;
        if downloaded.is_err() && io.cancel.as_ref().is_some_and(|cancel| cancel.is_cancelled()) {
            say("The download was stopped, so nothing was changed.".into());
            return Ok(1);
        }
        downloaded?;
        if hex::encode(Sha256::digest(fs::read(&bundle).map_err(error)?)) != manifest.sha256 {
            say("The download didn't match its checksum, so nothing was changed. Try again in a moment.".into());
            return Ok(1);
        }
        let failed = step(
            io.out.clone(),
            &io.env,
            &format!("Putting Kumi {} in place…", manifest.kumi),
            async {
                remove(&fresh).map_err(error)?;
                tokio::fs::create_dir_all(&fresh).await.map_err(error)?;
                let unpacked = run(
                    system::system_program_default(SystemProgram::Tar),
                    vec!["-xzf".into(), bundle.clone(), "-C".into(), fresh.clone()],
                    None,
                )
                .await;
                if unpacked.code != 0 {
                    let text = if unpacked.stderr.is_empty() { &unpacked.stdout } else { &unpacked.stderr };
                    return Ok::<_, RuntimeError>(Some(format!(
                        "Unpacking it failed: {}",
                        trim(text).split('\n').next_back().unwrap_or("tar failed")
                    )));
                }
                let probe = run(join(&fresh, &executable_name("kumi")), vec!["--version".into()], None).await;
                if probe.code != 0 || !probe.stdout.contains(&manifest.kumi) {
                    return Ok(Some("The new Kumi didn't start, so this one stays. Try again, or run the installer again.".into()));
                }
                write_launcher(&home).map_err(error)?;
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
                if swap_in_with(&folders, &fresh, &app, &join(&home, "app.previous")).await.is_err() {
                    return Ok(Some(swap_failed(cfg!(windows), migration::has_app(&app))));
                }
                Ok(None)
            },
            false,
        )
        .await?;
        if let Some(failed) = failed {
            say(failed);
            return Ok(1);
        }
        Ok(0)
    }
    .await;
    cleanup.finish().map_err(error)?;
    let code = result?;
    if code != 0 {
        return Ok(code);
    }
    say(format!("Kumi is now {} ({} update --rollback goes back to {KUMI_VERSION}).", manifest.kumi, *KUMI));
    if let Some(open) = still_open() {
        say(open);
    }
    Ok(bridge_after(&io, &home, &app).await)
}
/// After an update: Kumi windows already open still run this Kumi (Windows can't update while one is
/// open). What they keep reaches the new Kumi at its next turn; what it keeps reaches them once restarted.
pub fn still_open() -> Option<String> {
    (!cfg!(windows)).then(|| format!("Kumi windows opened before the update keep running {KUMI_VERSION} until you restart them."))
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
    // The older Kumi reads notes, techniques and lessons from files: what the database keeps is written
    // back for it first. If that fails, the rollback still goes ahead.
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
    // Windows can't roll back while a Kumi window is open; elsewhere one keeps running this Kumi.
    if !cfg!(windows) {
        say(format!(
            "Kumi windows still open keep running {KUMI_VERSION} until you close them; what they keep from now on stays in Kumi's database for your next update."
        ));
    }
    let legacy = !Path::new(&join(&previous, &executable_name("kumi"))).is_file();
    let bridge_rollback = if legacy { migration::prepare_legacy_rollback(&io, &home).await? } else { None };
    write_launcher(&home).map_err(error)?;
    if let Some(rollback) = &bridge_rollback {
        rollback.apply().await?;
    }
    let version = fs::read(join(&previous, "package.json"))
        .ok()
        .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
        .and_then(|v| v.get("version").and_then(Value::as_str).map(str::to_string))
        .unwrap_or("the one before".into());
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
        if let Some(rollback) = &bridge_rollback {
            if let Err(error) = rollback.apply().await {
                say(format!(
                    "The application swap failed, and restoring its bridge also failed: {}. Keep Live closed and run: kumi bridge",
                    error.message()
                ));
            }
        }
        say("Couldn't switch back; close every Kumi window and try again.".into());
        return Ok(1);
    }
    say(format!("Kumi is back to {version}. {} update --rollback again returns to {KUMI_VERSION}.", *KUMI));
    if Path::new(&join(&app, &executable_name("kumi"))).exists() {
        Ok(bridge_after(&io, &home, &app).await)
    } else {
        // The receipt-bound bridge rollback above restored the legacy command/config as well.
        Ok(0)
    }
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
pub fn remove_path_lines(io: &InstalledIo, home: &str) {
    if cfg!(windows) {
        return;
    }
    let bin = join(home, "bin");
    let actual = home::home_dir().unwrap_or_default().display().to_string();
    let home = io.env.get("HOME").filter(|s| !s.is_empty()).map(String::as_str).unwrap_or(&actual);
    for file in startup_files(&io.env) {
        let result = (|| -> std::io::Result<()> {
            let text = fs::read_to_string(&file)?;
            if !text.contains(PATH_MARKER) {
                return Ok(());
            }
            if file.ends_with("kumi.fish") {
                return remove(&file);
            }
            let lines: Vec<_> = text.split('\n').collect();
            let mut kept = vec![];
            let mut index = 0;
            while index < lines.len() {
                if lines[index] == PATH_MARKER {
                    if lines.get(index + 1).is_some_and(|s| s.contains(&bin)) {
                        index += 1
                    }
                } else {
                    kept.push(lines[index])
                }
                index += 1
            }
            fs::write(&file, kept.join("\n"))?;
            io.out.write(&format!("Took Kumi out of {}.\n", file.replacen(home, "~", 1)));
            Ok(())
        })();
        let _ = result;
    }
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
#[derive(PartialEq, Eq)]
enum RemovedBridge {
    Removed,
    Kept,
    None,
}
async fn remove_bridge(io: &InstalledIo, run: Run) -> Result<RemovedBridge, RuntimeError> {
    let say = |s: String| io.out.write(&format!("{s}\n"));
    let Some(config) = find_bridge_config(&io.env) else { return Ok(RemovedBridge::None) };
    let extensions = live_extensions_dir(&io.env, system::platform());
    let by_hand = format!(
        "AbletonMcpBridge from Live's Remote Scripts folder{}",
        extensions
            .as_ref()
            .map(|ext| format!(", and {KUMI_EXTENSION_ID} from {ext} and from {}", dirname(&extension_data_dir(ext))))
            .unwrap_or_default()
    );
    if !ask(io, "Remove the Ableton bridge from Live too?").await {
        say(format!("The bridge stays in Live, and so do its files. To take it out later, remove {by_hand}."));
        return Ok(RemovedBridge::Kept);
    }
    if live_open(io, run.clone()).await {
        say(format!("Live is open, so the bridge stays, and so do its files. Quit Live, then remove {by_hand}."));
        return Ok(RemovedBridge::Kept);
    }
    let root = read_bridge_server(&config).ok().and_then(|server| server.package_root());
    let lifecycle =
        root.as_ref().map(|root| join(root, &executable_name("ableton-mcp-server"))).filter(|file| Path::new(file).is_file()).or_else(
            || {
                let file = join(&executable_dir(), &executable_name("ableton-mcp-server"));
                Path::new(&file).is_file().then_some(file)
            },
        );
    let Some((root, lifecycle)) = root.zip(lifecycle) else {
        say(format!("Kumi couldn't find the bridge's own uninstaller; remove {by_hand} by hand."));
        return Ok(RemovedBridge::Kept);
    };
    let ran = step(
        io.out.clone(),
        &io.env,
        "Taking the bridge out of Live…",
        run(
            lifecycle,
            vec![
                "lifecycle".into(),
                "uninstall".into(),
                "--remote-scripts-dir".into(),
                remote_scripts_dir(&io.env),
                "--state-dir".into(),
                dirname(&config),
                "--package-root".into(),
                root,
                "--apply".into(),
                "--confirm-live-stopped".into(),
            ],
            None,
        ),
        false,
    )
    .await;
    if ran.code != 0 {
        say(format!("The bridge's uninstaller refused; remove {by_hand} by hand."));
        return Ok(RemovedBridge::Kept);
    }
    let removed_extension = extensions.as_ref().map(|ext| remove_extension(ext)).transpose().map_err(error)?.unwrap_or(false);
    let removed_former = remove_former_extension(&io.env, system::platform()).map_err(error)?;
    say(if removed_extension || removed_former {
        "The bridge and Kumi's extension are out of Live."
    } else {
        "The bridge is out of Live."
    }
    .into());
    let scripts = remote_scripts_dir(&io.env);
    if basename(&scripts) == "Remote Scripts" {
        remove(&join(&join(&dirname(&scripts), "Kumi"), &format!("{EARS_NAME}.amxd"))).map_err(error)?;
    }
    Ok(RemovedBridge::Removed)
}
fn bridge_uses(folder: &str, env: &Env) -> bool {
    let Some(config) = find_bridge_config(env) else { return false };
    let inside = |path: &str| {
        let mut folder = resolve(folder);
        let mut path = resolve(path);
        if cfg!(windows) {
            folder = folder.to_lowercase();
            path = path.to_lowercase();
        }
        Path::new(&path)
            .strip_prefix(&folder)
            .is_ok_and(|relative| !relative.as_os_str().is_empty() && !relative.to_string_lossy().starts_with(".."))
    };
    inside(&config) || read_bridge_server(&config).ok().and_then(|s| s.entry).is_some_and(|entry| inside(&entry))
}
#[derive(Clone, Copy, Default)]
pub struct UninstallOptions {
    pub all: bool,
    pub yes: bool,
}
pub async fn uninstall_installed(io: InstalledIo, options: UninstallOptions) -> Result<i32, RuntimeError> {
    let say = |s: String| io.out.write(&format!("{s}\n"));
    let home = kumi_home(&io.env);
    let display = |path: &str| path.replacen(&home::home_dir().unwrap_or_default().display().to_string(), "~", 1);
    let run = io.run.clone().unwrap_or_else(default_run);
    let keeps = if options.all {
        "Everything in it goes too: your conversations, notes, recipes and sign-ins."
    } else {
        "Your conversations, notes, recipes and sign-ins stay (add --all to remove them too)."
    };
    if !options.yes && !ask(&io, &format!("Remove Kumi from {}? {keeps}", display(&home))).await {
        say("Nothing was removed.".into());
        return Ok(1);
    }
    let bridge = remove_bridge(&io, run.clone()).await?;
    remove_path_lines(&io, &home);
    if cfg!(windows) {
        step(io.out.clone(), &io.env, "Taking Kumi out of your PATH…", remove_windows_path(run, &home), false).await;
    }
    let bridge_stays = bridge != RemovedBridge::Removed && bridge_uses(&join(&home, "bridge"), &io.env);
    let parts: Vec<String> = if options.all {
        if bridge_stays {
            fs::read_dir(&home)
                .map_err(error)?
                .map(|entry| entry.map(|entry| entry.path().display().to_string()))
                .collect::<Result<Vec<_>, _>>()
                .map_err(error)?
                .into_iter()
                .filter(|path| basename(path) != "bridge")
                .collect()
        } else {
            vec![home.clone()]
        }
    } else {
        ["app", "app.previous", "app.new", "node", "bin", "downloads", "bridge"]
            .into_iter()
            .filter(|part| *part != "bridge" || !bridge_stays)
            .map(|part| join(&home, part))
            .collect()
    };
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let list =
            parts.iter().map(|path| format!("rmdir /s /q \"{path}\" 2>nul & del /f /q \"{path}\" 2>nul")).collect::<Vec<_>>().join(" & ");
        std::process::Command::new(std::env::var("ComSpec").unwrap_or_else(|_| "cmd.exe".into()))
            .raw_arg(format!("/d /s /c \"ping -n 4 127.0.0.1 >nul & {list}\""))
            .creation_flags(0x00000008 | 0x00000200 | 0x08000000)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(error)?;
    }
    #[cfg(not(windows))]
    for path in &parts {
        remove(path).map_err(error)?;
    }
    if bridge_stays {
        say(format!("The bridge's files stay in {} while Live uses it.", display(&join(&home, "bridge"))));
    }
    say(if options.all {
        format!("Kumi is removed, with everything it kept{}.", if bridge_stays { " but the bridge's files" } else { "" })
    } else {
        format!("Kumi is removed. Your files are still in {}; delete that folder to remove them too.", display(&home))
    });
    say("Open a new terminal window so the `kumi` command is gone there too.".into());
    Ok(0)
}
pub async fn check_release(env: &Env, fetcher: Option<Rc<dyn Fetch>>) -> Result<Option<String>, RuntimeError> {
    match ask_release(env, fetcher).await {
        AskedRelease::Manifest(manifest) => Ok(newer_version(&manifest.kumi, KUMI_VERSION).then_some(manifest.kumi)),
        asked => Err(RuntimeError::plain(unasked(&asked, env))),
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
                return Err(std::io::ErrorKind::PermissionDenied.into());
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
            swap_in_with(windows, &self.path("app.new"), &self.path("app"), &self.path("app.previous")).await
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
        let kept = swap_failed(true, true);
        assert!(kept.contains("a Kumi window, or an antivirus scan") && kept.contains("still works") && kept.contains("run update again"));
        let gone = swap_failed(true, false);
        assert!(gone.contains("a Kumi window, or an antivirus scan") && !gone.contains("still works") && gone.contains("installer again"));
        assert_eq!(swap_failed(false, true), "Couldn't put the new Kumi in place, so this one stays.");
        assert!(swap_failed(false, false).contains("installer again"));
    }

    fn put_file(path: &str) {
        fs::create_dir_all(Path::new(path).parent().unwrap()).unwrap();
        fs::write(path, "").unwrap();
    }
}
