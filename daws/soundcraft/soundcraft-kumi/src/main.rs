//! `soundcraft-kumi`, Kumi's bridge beside SoundCraft. So far, one command:
//!
//! `soundcraft-kumi launch [--kumi-home <folder>] [--background] [<session>]` starts SoundCraft with its control channel
//! on, opening the session if one is named, or finds the copy an earlier start left running. It prints that copy as JSON
//! on stdout (its pid, port, app and version, and whether it started now). It starts no second copy beside one opened
//! another way, which has no channel: it says so and fails.
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
    if let Some(running) = launch::attach(&folder) {
        return said(&running, false);
    }
    if let Some(pid) = launch::running(&var).first() {
        eprintln!(
            "SoundCraft is open without its control channel (pid {pid}): it was opened from the Dock or the Start menu, or \
             restarted by its own update. Save, quit it, and Kumi opens your session again."
        );
        return ExitCode::FAILURE;
    }
    let Some(app) = launch::find(&var) else {
        eprintln!("SoundCraft isn't installed here (or {} names a program that isn't there)", launch::APP_VARIABLE);
        return ExitCode::FAILURE;
    };
    if let Some(settings) = launch::settings_folder(&var) {
        let waiting = launch::recovery_waiting(&settings);
        if !waiting.is_empty() {
            eprintln!("SoundCraft will offer to recover what it was doing when it last stopped: {}", waiting.join(", "));
        }
    }
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().expect("a runtime");
    match runtime.block_on(launch::launch(&app, session.as_deref(), &folder, background)) {
        Ok(launched) => said(&launched, true),
        Err(why) => {
            eprintln!("{why}");
            ExitCode::FAILURE
        }
    }
}

/// The copy on stdout, its token left out: the bridge reads that from launched.json, owner-only.
fn said(launched: &launch::Launched, started: bool) -> ExitCode {
    let shown = serde_json::json!({
        "started": started,
        "pid": launched.pid,
        "port": launched.port,
        "app": launched.app,
        "version": launched.version,
        "startedMs": launched.started_ms,
    });
    println!("{shown}");
    ExitCode::SUCCESS
}

fn usage() -> ExitCode {
    eprintln!("{USAGE}");
    ExitCode::from(2)
}
