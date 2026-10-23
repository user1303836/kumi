//! `kumi doctor`'s Installation section: this Kumi and where it is, what `update --rollback` goes back to, how
//! the shell finds `kumi`, whether a newer Kumi is out, free space, and what Live needs from this computer.
//! Files and the system are read; nothing goes over the network: whether a newer Kumi is out comes from
//! the daily check's note, and the bridge's ports are tried on this computer (127.0.0.1) only.
use super::tilde;
use crate::{
    config::{kumi_dir, load_settings_file, read_settings},
    install::{app_version, chown_line, has_app, not_yours, startup_files, PATH_MARKER, UPDATE_SPACE},
    update::newer,
};
use futures::future::LocalBoxFuture;
use kumi_common::time::{now_ms, since};
use kumi_daw::{
    check::Check,
    io::{Answers, DawIo},
    process::repository_dir,
};
use kumi_library::sources::{dirname, join};
use kumi_types::{
    command::KUMI,
    disk::{free_bytes, size},
    system::{self, Env},
    version::KUMI_VERSION,
};
use serde_json::Value;
use std::{path::Path, rc::Rc};

/// What the Installation section reads from the system; tests answer it.
#[derive(Clone, Default)]
pub struct InstallationIo {
    pub platform: Option<String>,
    pub free: Option<Rc<dyn Fn(String) -> LocalBoxFuture<'static, Option<f64>>>>,
    pub now: Option<f64>,
    /// What each DAW's checks of this computer read beyond the environment (`kumi_daw::io::Answers`): a test's answers.
    pub answers: Answers,
}

/// The `kumi` programs on `path`, in the order a shell finds them.
pub fn kumi_on_path(path: &str, windows: bool, extensions: &str) -> Vec<String> {
    let mut found = vec![];
    for folder in path.split(if windows { ';' } else { ':' }).filter(|folder| !folder.is_empty()) {
        if windows {
            for extension in extensions.split(';').filter(|e| !e.is_empty()) {
                let file = join(folder, &format!("kumi{}", extension.to_ascii_lowercase()));
                if Path::new(&file).is_file() {
                    found.push(file);
                    break;
                }
            }
            continue;
        }
        let file = join(folder, "kumi");
        #[cfg(unix)]
        let runs = {
            use std::os::unix::fs::PermissionsExt;
            std::fs::metadata(&file).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        };
        #[cfg(not(unix))]
        let runs = Path::new(&file).is_file();
        if runs {
            found.push(file);
        }
    }
    found
}

/// Whether `file` is Kumi's own launcher, or anything else in Kumi's folder (a link to it counts).
fn kumi_s_own(file: &str, home: &str) -> bool {
    let real = |path: &str| std::fs::canonicalize(path).unwrap_or_else(|_| path.into());
    real(file).starts_with(real(home))
}

fn path_check(env: &Env, home: &str, windows: bool) -> Check {
    let bin = join(home, "bin");
    let found = kumi_on_path(
        env.get("PATH").or_else(|| env.get("Path")).map(String::as_str).unwrap_or(""),
        windows,
        env.get("PATHEXT").map(String::as_str).unwrap_or(".COM;.EXE;.BAT;.CMD"),
    );
    match found.first() {
        Some(first) if kumi_s_own(first, home) => Check::ok(format!("kumi on PATH: {}", tilde(first))),
        Some(first) => Check::fix(
            format!("Another kumi comes first on PATH: {}", tilde(first)),
            Some(format!("Remove it, or put {} before {} in PATH", tilde(&bin), tilde(&dirname(first)))),
        ),
        None if windows => Check::note(
            "kumi isn't on PATH in this window",
            Some(format!("Open a new terminal window; the installer added {} to PATH for new ones", tilde(&bin))),
        ),
        None => {
            let launcher = join(&bin, "kumi");
            let added =
                startup_files(env).into_iter().find(|file| std::fs::read_to_string(file).is_ok_and(|text| text.contains(PATH_MARKER)));
            Check::note(
                "kumi isn't on PATH in this window",
                Some(match added {
                    Some(file) => format!("Open a new terminal window ({} adds it there); until then: {}", tilde(&file), tilde(&launcher)),
                    None => format!("Add {} to PATH in your shell's startup file, or run {}", tilde(&bin), tilde(&launcher)),
                }),
            )
        }
    }
}

fn rollback_check(home: &str) -> Check {
    let previous = join(home, "app.previous");
    match app_version(&previous) {
        Some(version) if version == KUMI_VERSION => {
            Check::ok(format!("{} update --rollback has only this same {version} to go back to (kept by a reinstall)", *KUMI))
        }
        Some(version) => Check::ok(format!("{} update --rollback goes back to {version}", *KUMI)),
        None if has_app(&previous) => Check::ok(format!("{} update --rollback goes back to the Kumi before", *KUMI)),
        None => Check::ok(format!("Nothing kept to go back to yet: the next update keeps {KUMI_VERSION} for {} update --rollback", *KUMI)),
    }
}

fn update_check(env: &Env, now: f64) -> Check {
    let settings = load_settings_file(env).ok();
    let off = if env.get("KUMI_NO_UPDATE_CHECK").is_some_and(|s| !s.is_empty()) {
        Some("KUMI_NO_UPDATE_CHECK")
    } else if settings.as_deref().is_some_and(|file| read_settings(file).update_check == Some(false)) {
        Some("updateCheck in settings.json")
    } else {
        None
    };
    let checked = settings
        .map(|file| join(&dirname(&file), "update-check.json"))
        .and_then(|file| serde_json::from_slice::<Value>(&std::fs::read(file).ok()?).ok())
        .and_then(|cached| Some((cached.get("latest")?.as_str()?.to_string(), cached.get("checkedAt")?.as_f64()?)));
    match (off, checked) {
        (Some(why), _) => Check::ok(format!("Kumi doesn't look for updates by itself ({why}); {} update --check asks now", *KUMI)),
        (None, Some((latest, _))) if newer(&latest, KUMI_VERSION) => {
            Check::note(format!("Kumi {latest} is out (this is {KUMI_VERSION})"), Some(format!("{} update", *KUMI)))
        }
        (None, Some((_, at))) => Check::ok(format!("Up to date when Kumi last looked ({})", since(at, now))),
        (None, None) => Check::ok(format!("Kumi looks for updates once a day while it runs; {} update --check asks now", *KUMI)),
    }
}

fn space_check(free: Option<f64>, home: &str) -> Option<Check> {
    let free = free?;
    Some(if free < UPDATE_SPACE {
        Check::fix(
            format!("Only {} free on the disk with {}", size(free), tilde(home)),
            Some("Free some space there: an update needs about 250 MB, and Kumi's conversations and notes need room too".into()),
        )
    } else {
        Check::ok(format!("{} free on the disk with {}", size(free), tilde(home)))
    })
}

// What the unit tests below drive: Live's check of the bridge's ports.
#[cfg(test)]
use crate::daw::ableton::doctor::ports_check;

pub async fn installation_checks(env: &Env, io: &InstallationIo) -> Vec<Check> {
    let platform = io.platform.clone().unwrap_or_else(|| system::platform().into());
    let home = kumi_dir(env);
    let installed = env.get("KUMI_INSTALLED").is_some_and(|s| s == "1");
    let mut checks = vec![];
    if installed {
        checks.push(Check::ok(format!("Kumi {KUMI_VERSION} in {}", tilde(&home))));
        checks.push(rollback_check(&home));
        // PATH is this system's, whatever platform the Live checks are told.
        checks.push(path_check(env, &home, cfg!(windows)));
    } else {
        // A checkout's build, or Kumi's own executable started without its launcher.
        let repository = repository_dir();
        checks.push(Check::ok(if Path::new(&join(&repository, "Cargo.toml")).is_file() {
            format!("Kumi {KUMI_VERSION}, built in the checkout at {}", tilde(&repository))
        } else {
            let exe = std::env::current_exe().map(|exe| exe.display().to_string()).unwrap_or_default();
            format!("Kumi {KUMI_VERSION} at {}", tilde(&exe))
        }));
    }
    checks.push(update_check(env, io.now.unwrap_or_else(|| now_ms() as f64)));
    if let Some(folder) = not_yours(&home) {
        checks.push(Check::fix(format!("{} belongs to another account (installed with sudo?)", tilde(&folder)), Some(chown_line(&home))));
    }
    let free = match &io.free {
        Some(free) => free(home.clone()).await,
        None => free_bytes(Path::new(&home)).await,
    };
    checks.extend(space_check(free, &home));
    // What each DAW the producer uses needs from this computer.
    let daw = DawIo { answers: io.answers.clone(), ..DawIo::new(env.clone()) };
    for provider in crate::daw::providers().into_iter().filter(|provider| crate::daw::in_use(*provider, env)) {
        checks.extend(provider.installation_checks(&daw, &platform).await);
    }
    checks
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ports_held_by_live_are_fine_and_by_another_program_need_fixing() {
        assert_eq!(ports_check(&[(9765, None), (9766, None)], false).text, "Ports 9765 and 9766 free for Live's bridge");
        assert_eq!(ports_check(&[(9765, None), (9766, None)], true).status, kumi_daw::check::CheckStatus::Ok);
        let live = ports_check(&[(9765, Some(Some("Live".into()))), (9766, None)], true);
        assert_eq!(live, Check::ok("Live's bridge has port 9765 (Live is open)"));
        let windows =
            ports_check(&[(9765, Some(Some("Ableton Live 12 Suite".into()))), (9766, Some(Some("Ableton Live 12 Suite".into())))], true);
        assert_eq!(windows.text, "Live's bridge has ports 9765 and 9766 (Live is open)");
        // Unnamed while Live runs: Live's. Unnamed with Live closed: another program's.
        assert_eq!(ports_check(&[(9765, Some(None))], true).status, kumi_daw::check::CheckStatus::Ok);
        let unnamed = ports_check(&[(9765, Some(None))], false);
        assert_eq!(unnamed.status, kumi_daw::check::CheckStatus::Fix);
        assert!(unnamed.next.unwrap().starts_with("Quit it, then restart Live"));
        let other = ports_check(&[(9765, Some(Some("Live".into()))), (9766, Some(Some("node".into())))], true);
        assert_eq!(other, Check::fix("node holds port 9766, which Live's bridge needs", Some("Quit node, then restart Live".into())));
    }
}
