//! `soundcraft-kumi`, Kumi's bridge beside SoundCraft. So far, one command:
//!
//! `soundcraft-kumi launch [--kumi-home <folder>] [--background] [<session>]` starts SoundCraft with its control channel
//! on, opening the session if one is named, or finds the copy an earlier start left running. It prints that copy as JSON
//! on stdout (its pid, port, app and version, whether it started now, and whether its channel answers yet). It starts no
//! second copy beside one opened another way, which has no channel: it says so and fails.
use soundcraft_kumi::launch;
use std::path::PathBuf;
use std::process::ExitCode;

const USAGE: &str = "usage: soundcraft-kumi launch [--kumi-home <folder>] [--background] [<session>]";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("launch") => launch_command(&args[1..]),
        Some("--version") => {
            println!("soundcraft-kumi {}", env!("CARGO_PKG_VERSION"));
            ExitCode::SUCCESS
        }
        _ => {
            eprintln!("{USAGE}");
            ExitCode::from(2)
        }
    }
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
    let Some(kumi_home) = kumi_home.or_else(daw_bridge::paths::kumi_home) else {
        eprintln!("no home folder: set KUMI_HOME or pass --kumi-home");
        return ExitCode::FAILURE;
    };
    let folder = kumi_home.join("bridges").join("soundcraft");
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
            eprintln!("SoundCraft will offer to recover what it was doing when it last stopped: {}", waiting.join(", "));
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

fn failed(why: &str) -> ExitCode {
    eprintln!("{why}");
    ExitCode::FAILURE
}

fn usage() -> ExitCode {
    eprintln!("{USAGE}");
    ExitCode::from(2)
}
