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
use soundcraft_kumi::{launch, server};
use std::io::Write;
use std::path::{Path, PathBuf};
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
fn folder(kumi_home: Option<PathBuf>) -> Option<PathBuf> {
    kumi_home.or_else(daw_bridge::paths::kumi_home).map(|home| home.join("bridges").join(server::DAW))
}

fn serve_command(args: &[String]) -> ExitCode {
    let kumi_home = match args {
        [] => None,
        [flag, folder] if flag == "--kumi-home" => Some(PathBuf::from(folder)),
        _ => return usage(),
    };
    let Some(folder) = folder(kumi_home) else {
        return failed("no home folder: set KUMI_HOME or pass --kumi-home");
    };
    // (read now to say at once when it can't be; the server reads it again for each connection)
    let secret_file = folder.join("bridge.secret");
    if let Err(error) = daw_bridge::files::read_secret(&secret_file) {
        return failed(&format!("the secret Kumi's host makes for this bridge can't be read: {error}"));
    }
    let var = |name: &str| std::env::var(name).ok();
    let runtime = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().expect("a runtime");
    let Some(found) = runtime.block_on(launch::attach(&folder, launch::Waits::USUAL.answer)) else {
        return failed(&match launch::running(&var).first() {
            Some(pid) => {
                format!("SoundCraft is open without its control channel (pid {pid}): save, quit it, and Kumi opens your session again")
            }
            None => "SoundCraft isn't running with its control channel: Kumi opens it with `kumi --daw soundcraft`".into(),
        });
    };
    let launched = found.launched;
    // One bridge for one copy: the one that holds the lock serves it. (The lock goes with this process, however it ends.)
    let _serving = match serving_lock(&folder, launched.pid) {
        Ok(Some(lock)) => lock,
        Ok(None) => return failed(&format!("another bridge serves this SoundCraft (pid {}) already", launched.pid)),
        Err(error) => {
            return failed(&format!("the bridge can't take serving-{}.lock in {}: {error}", launched.pid, folder.display()));
        }
    };
    let resource = launch::settings_folder(&var, Some(&launched.app)).map(|folder| folder.display().to_string()).unwrap_or_default();
    let config = server::Config::new(secret_file, folder.join("discovery"), launched, resource);
    let ended = runtime.block_on(server::serve(config, stdin_ends()));
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

/// The lock on `serving-<pid>.lock` in the bridge's folder: None while another bridge holds it, and an error when it
/// can't be taken at all (a file system without locks), which is no other bridge's doing.
fn serving_lock(folder: &Path, pid: u32) -> std::io::Result<Option<std::fs::File>> {
    let mut options = std::fs::OpenOptions::new();
    options.create(true).truncate(false).write(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    let file = options.open(folder.join(format!("serving-{pid}.lock")))?;
    match file.try_lock() {
        Ok(()) => Ok(Some(file)),
        Err(std::fs::TryLockError::WouldBlock) => Ok(None),
        Err(std::fs::TryLockError::Error(error)) => Err(error),
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
    let Some(folder) = folder(kumi_home) else {
        return failed("no home folder: set KUMI_HOME or pass --kumi-home");
    };
    // Held until the copy is found or recorded: a second start waits, then finds the first one's copy.
    let _lock = match launch::lock(&folder) {
        Ok(lock) => lock,
        Err(why) => return failed(&why),
    };
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().expect("a runtime");
    if let Some(found) = runtime.block_on(launch::attach(&folder, launch::Waits::USUAL.answer)) {
        return said(&found, false);
    }
    // (found first: a program KUMI_SOUNDCRAFT_APP names under another name is known by that name too)
    let app = launch::find(&var);
    if let Some(pid) = launch::running(&var, app.as_ref().map(|app| app.path.as_path())).first().copied() {
        if launch::recorded(&folder).is_some_and(|launched| launched.pid == pid) {
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
        return failed(&format!("SoundCraft isn't installed here (or {} names a program that isn't there)", launch::APP_VARIABLE));
    };
    if let Some(settings) = launch::settings_folder(&var, Some(&app.path)) {
        let waiting = launch::recovery_waiting(&settings);
        if !waiting.is_empty() {
            say(&format!("SoundCraft will offer to recover what it was doing when it last stopped: {}", waiting.join(", ")));
        }
    }
    match runtime.block_on(launch::launch(&app, session.as_deref(), &folder, background)) {
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
