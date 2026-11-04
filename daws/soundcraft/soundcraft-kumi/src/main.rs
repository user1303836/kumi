//! `soundcraft-kumi`, Kumi's bridge beside SoundCraft.
//!
//! `soundcraft-kumi [--kumi-home <folder>]` serves `kumi-daw/1` for the copy of SoundCraft Kumi started, until its stdin
//! ends (the host that started it has gone) or SoundCraft quits. Kumi's host starts it so, with its stdin a pipe of the
//! host's. One bridge serves one copy: a second one for the same copy says so and ends.
//!
//! `soundcraft-kumi launch [--kumi-home <folder>] [--background] [<session>]` starts SoundCraft with its control channel
//! on, opening the session if one is named, or finds the copy an earlier start left running. It prints that copy as JSON
//! on stdout (its pid, port, app and version, whether it started now, and whether its channel answers yet). It starts no
//! second copy beside one opened another way, which has no channel: it says so and fails.
use daw_bridge::daw::Daw;
use daw_bridge::detect;
use daw_bridge::files::Launched;
use daw_bridge::paths::BridgePaths;
use soundcraft_kumi::{launch, server};
use std::io::Write;
use std::path::PathBuf;
use std::process::ExitCode;

const USAGE: &str =
    "usage: soundcraft-kumi [--kumi-home <folder>]\n       soundcraft-kumi launch [--kumi-home <folder>] [--background] [<session>]";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("launch") => launch_command(&args[1..]),
        Some("--version") => {
            println!("soundcraft-kumi {}", env!("CARGO_PKG_VERSION"));
            ExitCode::SUCCESS
        }
        _ => serve_command(&args),
    }
}

/// Words on stderr, without the panic `eprintln!` gives when the host that reads them has gone.
fn say(words: &str) {
    let _ = writeln!(std::io::stderr(), "soundcraft-kumi: {words}");
}

fn failed(why: &str) -> ExitCode {
    say(why);
    ExitCode::FAILURE
}

fn usage() -> ExitCode {
    say(USAGE);
    ExitCode::from(2)
}

/// The bridge's folder in Kumi's: `<Kumi's folder>/bridges/soundcraft`, from `--kumi-home` or KUMI_HOME.
fn paths(kumi_home: Option<PathBuf>) -> Option<BridgePaths> {
    kumi_home.or_else(daw_bridge::paths::kumi_home).map(|home| BridgePaths::new(&home, Daw::SoundCraft))
}

fn serve_command(args: &[String]) -> ExitCode {
    let kumi_home = match args {
        [] => None,
        [flag, folder] if flag == "--kumi-home" => Some(PathBuf::from(folder)),
        _ => return usage(),
    };
    let Some(paths) = paths(kumi_home) else {
        return failed("no home folder: set KUMI_HOME or pass --kumi-home");
    };
    // (read now to say at once when it can't be; the server reads it again for each connection)
    if let Err(error) = daw_bridge::files::read_secret(&paths.secret()) {
        return failed(&format!("the secret Kumi's host makes for this bridge can't be read: {error}"));
    }
    let var = |name: &str| std::env::var(name).ok();
    let runtime = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().expect("a runtime");
    let Some(found) = runtime.block_on(launch::attach(&paths, launch::Waits::USUAL.answer)) else {
        let recorded = Launched::read(&paths.launch_record());
        let app = recorded.as_ref().map(|launched| launched.app.clone()).or_else(|| launch::find(&var).map(|app| app.path));
        return failed(&match launch::running(&var, app.as_deref()).first().copied() {
            Some(pid) if recorded.as_ref().is_some_and(|launched| launched.pid == pid) => format!(
                "the SoundCraft Kumi started (pid {pid}) no longer takes Kumi's requests on its control channel: save, quit it, and \
                 Kumi starts it again"
            ),
            Some(pid) => {
                format!("SoundCraft is open without its control channel (pid {pid}): save, quit it, and Kumi opens your session again")
            }
            None => "SoundCraft isn't running with its control channel: Kumi opens it with `kumi --daw soundcraft`".into(),
        });
    };
    let launched = found.launched;
    // One bridge for one copy: the one that holds the lock serves it. (The lock goes with the bridge's discovery file, or
    // with this process, however it ends.)
    let serving = match serving_lock(&paths, launched.pid) {
        Ok(Some(lock)) => lock,
        Ok(None) => return failed(&format!("another bridge serves this SoundCraft (pid {}) already", launched.pid)),
        Err(error) => {
            return failed(&format!("the bridge can't take {}: {error}", paths.serving_lock(launched.pid).display()));
        }
    };
    let listener = match server::bind() {
        Ok(listener) => listener,
        Err(error) => return failed(&format!("the bridge can't listen on loopback: {error}")),
    };
    let settings = detect::soundcraft_settings(&detect::Env::current(&var), Some(&launched.app));
    let resource = settings.map(|folder| folder.display().to_string()).unwrap_or_default();
    let config = server::Config { serving: Some(serving), ..server::Config::new(paths.secret(), paths.discovery(), launched, resource) };
    let ended = runtime.block_on(server::serve(listener, config, stdin_ends()));
    // (stdin's reader is a blocking read, which a runtime's own shutdown would wait for: ended by SoundCraft quitting, the
    // bridge doesn't wait for its host to let go of its stdin)
    runtime.shutdown_background();
    match ended {
        Ok(server::End::Stopped) => ExitCode::SUCCESS,
        Ok(server::End::AppQuit) => {
            say("SoundCraft quit");
            ExitCode::SUCCESS
        }
        Err(why) => failed(&why),
    }
}

/// The lock on the bridge's serving lock for the copy whose process is `pid`: None while another bridge holds it, and an
/// error when it can't be taken at all (a file system without locks), which is no other bridge's doing. It waits a moment
/// for a bridge that is letting go (Windows lets a dead process's lock go late) before saying another one serves.
fn serving_lock(paths: &BridgePaths, pid: u32) -> std::io::Result<Option<std::fs::File>> {
    let file = daw_bridge::lifecycle::lock_file(&paths.serving_lock(pid))?;
    let until = std::time::Instant::now() + std::time::Duration::from_secs(1);
    loop {
        match file.try_lock() {
            Ok(()) => return Ok(Some(file)),
            Err(std::fs::TryLockError::WouldBlock) if std::time::Instant::now() < until => {
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            Err(std::fs::TryLockError::WouldBlock) => return Ok(None),
            Err(std::fs::TryLockError::Error(error)) => return Err(error),
        }
    }
}

/// When the bridge's stdin ends: the host that started it has gone, killed or not, or let go of it.
async fn stdin_ends() {
    use tokio::io::AsyncReadExt;
    let mut stdin = tokio::io::stdin();
    let mut chunk = [0u8; 256];
    while matches!(stdin.read(&mut chunk).await, Ok(1..)) {}
}

fn launch_command(args: &[String]) -> ExitCode {
    let (mut kumi_home, mut background, mut session) = (None, false, None);
    let mut rest = args.iter();
    while let Some(arg) = rest.next() {
        match arg.as_str() {
            "--kumi-home" => match rest.next() {
                Some(folder) => kumi_home = Some(PathBuf::from(folder)),
                None => return usage(),
            },
            "--background" => background = true,
            other if other.starts_with("--") || session.is_some() => return usage(),
            other => session = Some(PathBuf::from(other)),
        }
    }
    let var = |name: &str| std::env::var(name).ok();
    let Some(paths) = paths(kumi_home) else {
        return failed("no home folder: set KUMI_HOME or pass --kumi-home");
    };
    // Held until the copy is found or recorded: a second start waits, then finds the first one's copy.
    let _lock = match launch::lock(&paths) {
        Ok(lock) => lock,
        Err(why) => return failed(&why),
    };
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().expect("a runtime");
    if let Some(found) = runtime.block_on(launch::attach(&paths, launch::Waits::USUAL.answer)) {
        return said(&found, false);
    }
    // (found first: a program KUMI_SOUNDCRAFT_APP names under another name is known by that name too)
    let app = launch::find(&var);
    if let Some(pid) = launch::running(&var, app.as_ref().map(|app| app.path.as_path())).first().copied() {
        if Launched::read(&paths.launch_record()).is_some_and(|launched| launched.pid == pid) {
            return failed(&format!(
                "The SoundCraft Kumi started (pid {pid}) no longer takes Kumi's requests on its control channel. Save, quit \
                 it, and Kumi starts it again."
            ));
        }
        return failed(&format!(
            "SoundCraft is open without its control channel (pid {pid}): it was opened from the Dock or the Start menu, or \
             restarted by its own update. Save, quit it, and Kumi opens your session again."
        ));
    }
    let Some(app) = app else {
        return failed(&format!("SoundCraft isn't installed here (or {} names a program that isn't there)", detect::SOUNDCRAFT_APP));
    };
    if let Some(settings) = detect::soundcraft_settings(&detect::Env::current(&var), Some(&app.path)) {
        let waiting = launch::recovery_waiting(&settings);
        if !waiting.is_empty() {
            say(&format!("SoundCraft will offer to recover what it was doing when it last stopped: {}", waiting.join(", ")));
        }
    }
    match runtime.block_on(launch::launch(&app, session.as_deref(), &paths, background)) {
        Ok(found) => said(&found, true),
        Err(why) => failed(&why),
    }
}

/// The copy on stdout, its token left out: the bridge reads that from launched.json, owner-only.
fn said(found: &launch::Found, started: bool) -> ExitCode {
    let launched = &found.launched;
    let shown = serde_json::json!({
        "started": started,
        "answering": found.answering,
        "pid": launched.pid,
        "port": launched.port,
        "app": launched.app,
        "version": launched.version,
        "startedMs": launched.started_ms,
    });
    println!("{shown}");
    ExitCode::SUCCESS
}
