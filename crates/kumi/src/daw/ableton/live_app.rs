//! Live, the app: which one is open, asking it to quit the way its own menu does (unsaved work makes
//! it ask to save first), opening it again, and which Live is on this computer at all.

use crate::bridge_setup::{is_live_running_on, Run};
use kumi_common::abort::Signal;
use kumi_runtime::{
    library::sources::join,
    system::{self, Env, SystemProgram},
};
use regex::Regex;
use std::{path::Path, sync::LazyLock, time::Duration};

/// The Live that's open: its app, a `.app` on macOS and an `.exe` on Windows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenLive {
    pub app: String,
}

/// "/Applications/Ableton Live 12 Suite.app/Contents/MacOS/Live", as `ps` names it, is that app.
pub fn live_app_of(line: &str) -> Option<OpenLive> {
    let app = line.trim().strip_suffix("/Contents/MacOS/Live")?;
    app.ends_with(".app").then(|| OpenLive { app: app.into() })
}

fn powershell(env: &Env, platform: &str) -> String {
    system::system_program(SystemProgram::Powershell, env, platform)
}
fn args(words: &[&str]) -> Vec<String> {
    words.iter().map(|word| word.to_string()).collect()
}
/// A PowerShell single-quoted string.
fn quoted(text: &str) -> String {
    format!("'{}'", text.replace('\'', "''"))
}

/// Which Live is open, if one is.
pub async fn open_live(run: &Run, platform: &str, env: &Env) -> Option<OpenLive> {
    match platform {
        "darwin" => run("ps".into(), args(&["-axo", "comm="]), None).await.stdout.lines().find_map(live_app_of),
        "win32" => {
            let path = "Get-Process -Name 'Ableton Live*' -ErrorAction SilentlyContinue | Select-Object -First 1 -ExpandProperty Path";
            let ran = run(powershell(env, platform), args(&["-NoProfile", "-NonInteractive", "-Command", path]), None).await;
            let app = ran.stdout.trim();
            (!app.is_empty()).then(|| OpenLive { app: app.into() })
        }
        _ => None,
    }
}

/// Ask Live to quit, as its own menu does: with unsaved work it asks the producer to save first. This
/// returns once asked, not when Live has answered: the producer may still be deciding about saving. False
/// when the system didn't let Kumi ask: macOS's Automation is off for this terminal, so the producer quits
/// Live instead (asking again can't help).
pub async fn ask_to_quit(run: &Run, platform: &str, env: &Env) -> bool {
    match platform {
        "darwin" => {
            // Without `ignoring application responses`, osascript waits for Live's answer, which comes
            // only once the save dialog is answered (or after AppleScript's two-minute timeout).
            let quit = ["ignoring application responses", "tell application id \"com.ableton.live\" to quit", "end ignoring"];
            let ran = run("osascript".into(), args(&["-e", quit[0], "-e", quit[1], "-e", quit[2]]), None).await;
            ran.code == 0 || !not_allowed(&ran.stderr)
        }
        "win32" => {
            let close = "Get-Process -Name 'Ableton Live*' -ErrorAction SilentlyContinue | ForEach-Object { [void]$_.CloseMainWindow() }";
            run(powershell(env, platform), args(&["-NoProfile", "-NonInteractive", "-Command", close]), None).await;
            true
        }
        _ => true,
    }
}

/// What osascript says when macOS doesn't let it send Live an Apple Event: turned down (-1743), or it would have
/// to ask and can't (-1744).
pub fn not_allowed(said: &str) -> bool {
    said.contains("-1743") || said.contains("-1744") || said.contains("Not authorized to send Apple events")
}

/// Wait until Live has closed: true once it has, false when `stop` comes first.
pub async fn closed(run: &Run, platform: &str, env: &Env, stop: &Signal) -> bool {
    loop {
        if !is_live_running_on(run.clone(), platform, env).await {
            return true;
        }
        if stop.is_cancelled() {
            return false;
        }
        tokio::select! {
            _ = tokio::time::sleep(Duration::from_millis(500)) => {}
            _ = stop.cancelled() => return false,
        }
    }
}

/// Open Live: the app that was open before, or else the newest one installed; with `set`, that Set in it.
pub async fn start(run: &Run, platform: &str, env: &Env, app: Option<&str>, set: Option<&str>) -> bool {
    match platform {
        "darwin" => {
            let mut target = match (app, set) {
                (Some(app), Some(_)) => args(&["-a", app]),
                (Some(app), None) => args(&[app]),
                (None, _) => args(&["-b", "com.ableton.live"]),
            };
            target.extend(set.map(str::to_string));
            run("open".into(), target, None).await.code == 0
        }
        "win32" => {
            let found = "Get-ChildItem -Path (Join-Path $env:ProgramData 'Ableton\\Live*\\Program\\Ableton Live*.exe') -ErrorAction SilentlyContinue | Sort-Object LastWriteTime -Descending | Select-Object -First 1 -ExpandProperty FullName";
            let app = app.map(quoted).unwrap_or_else(|| format!("({found})"));
            // One argument, quoted for the command line Live reads, spaces and all.
            let set = set.map(|set| format!(" -ArgumentList {}", quoted(&format!("\"{set}\"")))).unwrap_or_default();
            let script = format!("$app = {app}; if ($app) {{ Start-Process -FilePath $app{set}; 'started' }}");
            run(powershell(env, platform), args(&["-NoProfile", "-NonInteractive", "-Command", &script]), None)
                .await
                .stdout
                .contains("started")
        }
        _ => false,
    }
}

/// Open a Set's file in the Live that's running, by that Live's own app (a computer can have several Lives), as a
/// double-click on the file does there: Live asks about its open Set itself, and Kumi never answers for it. False when
/// no Live is running, or the system didn't take the file.
pub async fn open_set_file(run: &Run, platform: &str, env: &Env, set: &str) -> bool {
    let Some(live) = open_live(run, platform, env).await else { return false };
    start(run, platform, env, Some(&live.app), Some(set)).await
}

/// The oldest Live Kumi works with, by its major version: setup, the doctor and what's installed here read it.
pub const MIN_LIVE_MAJOR: u32 = 12;

/// What of Live is on this computer, read from its files without opening it: whether it can take the bridge yet.
pub use crate::daw::provider::Installed as LiveInstalled;

/// Which Live is here, from files alone: the Lives installed where their installer puts them, the settings folder
/// each version makes as it first starts ("Live 12.4.15"), and the User Library the bridge goes in.
pub fn installed(env: &Env, platform: &str) -> LiveInstalled {
    installed_with(env, platform, &installed_lives(env, platform))
}

/// `installed`, with the Lives installed here read already (the doctor lists them too).
pub fn installed_with(env: &Env, platform: &str, lives: &[InstalledLive]) -> LiveInstalled {
    let var = |name: &str| system::env_var(env, name, platform).unwrap_or_default().to_string();
    // The producer said where Live's Remote Scripts are: that's Live's to have made.
    if !var("KUMI_REMOTE_SCRIPTS_DIR").is_empty() {
        return LiveInstalled::Ready;
    }
    let libraries = match platform {
        "darwin" => vec![join(&var("HOME"), "Music/Ableton/User Library")],
        "win32" => ["Documents", "OneDrive/Documents"]
            .map(|documents| join(&var("USERPROFILE"), &format!("{documents}/Ableton/User Library")))
            .into(),
        _ => return LiveInstalled::Missing,
    };
    decide(lives, &opened_versions(&crate::config::live_preferences_dir(env)), &libraries)
}

/// The versions Live has been opened as here, from its settings folders ("Live 12.4.15b6"): newest major first.
fn opened_versions(settings: &str) -> Vec<(u32, String)> {
    static OPENED: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^Live ((\d+)[0-9A-Za-z.]*)$").unwrap());
    let mut opened: Vec<(u32, String)> = std::fs::read_dir(settings)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            let found = OPENED.captures(&name)?;
            Some((found[2].parse().ok()?, found[1].to_string()))
        })
        .collect();
    opened.sort_by(|a, b| b.0.cmp(&a.0));
    opened
}

/// A Live's major version: from its version, else its name ("Ableton Live 12 Suite").
fn major(live: &InstalledLive) -> Option<u32> {
    let version = live.version.as_deref().and_then(|version| version.split('.').next()?.parse().ok());
    version.or_else(|| live.name.split_whitespace().find_map(|word| word.parse().ok()))
}

/// What setup can do with the Lives installed and opened here, and the User Libraries Live makes.
fn decide(lives: &[InstalledLive], opened: &[(u32, String)], libraries: &[String]) -> LiveInstalled {
    if opened.iter().any(|(major, _)| *major >= MIN_LIVE_MAJOR) {
        return LiveInstalled::Ready;
    }
    let newest = lives.iter().filter_map(|live| Some((major(live)?, live))).max_by_key(|(major, _)| *major);
    if let Some((_, live)) = newest.filter(|(major, _)| *major >= MIN_LIVE_MAJOR) {
        return LiveInstalled::NeverOpened(live.app.clone());
    }
    if let Some((_, version)) = opened.first() {
        return LiveInstalled::Older(version.clone());
    }
    if let Some((major, live)) = newest {
        return LiveInstalled::Older(live.version.clone().unwrap_or_else(|| major.to_string()));
    }
    // Live somewhere Kumi doesn't look, that has made its User Library where Live puts it: the bridge can go in.
    if libraries.iter().any(|library| Path::new(library).is_dir()) {
        return LiveInstalled::Ready;
    }
    LiveInstalled::Missing
}

/// Whether Live runs on this platform: macOS and Windows. Elsewhere (Linux) Kumi chats without it.
pub fn runs_on(platform: &str) -> bool {
    matches!(platform, "darwin" | "win32")
}

/// Where Live keeps its Settings, for the Control Surface step's second look.
pub fn settings_menu(platform: &str) -> Option<String> {
    match platform {
        "darwin" => Some("Settings is in Live's Live menu (⌘,).".into()),
        "win32" => Some("Settings is in Live's Options menu (Ctrl+,).".into()),
        _ => None,
    }
}

/// "12.4" from a version such as "12.4.15b5" or "12.4b15": its first two numbers.
pub fn major_minor(version: &str) -> Option<String> {
    let mut parts = version.trim().split(|c: char| !c.is_ascii_digit());
    let major = parts.next().filter(|p| !p.is_empty())?;
    let minor = parts.next().filter(|p| !p.is_empty());
    Some(minor.map(|minor| format!("{major}.{minor}")).unwrap_or_else(|| major.into()))
}

/// Live's version ("12.4"), read from its app: `app`, or else the Live that's open.
pub async fn version(run: &Run, platform: &str, env: &Env, app: Option<&str>) -> Option<String> {
    let app = match app {
        Some(app) => app.to_string(),
        None => open_live(run, platform, env).await?.app,
    };
    let read = match platform {
        "darwin" => run("defaults".into(), args(&["read", &format!("{app}/Contents/Info"), "CFBundleShortVersionString"]), None).await,
        "win32" => {
            let script = format!("(Get-Item -LiteralPath {}).VersionInfo.ProductVersion", quoted(&app));
            run(powershell(env, platform), args(&["-NoProfile", "-NonInteractive", "-Command", &script]), None).await
        }
        _ => return None,
    };
    (read.code == 0).then(|| major_minor(&read.stdout)).flatten()
}

/// A Live installed where its installer puts it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstalledLive {
    /// The app: a `.app` on macOS, the `.exe` on Windows.
    pub app: String,
    /// "Ableton Live 12 Suite", the app's own name.
    pub name: String,
    /// "12.4.15b6" as the app has it on macOS; on Windows, its major from its folder ("12").
    pub version: Option<String>,
    /// Whether a Live of its major has been opened on this account: Live keeps a settings folder for each version.
    pub opened: bool,
}

/// The Lives installed in /Applications and ~/Applications on macOS, or under %ProgramData%\Ableton on
/// Windows; none on Linux, where Live doesn't run. File reads only.
pub fn installed_lives(env: &Env, platform: &str) -> Vec<InstalledLive> {
    let home =
        env.get("HOME").filter(|s| !s.is_empty()).cloned().unwrap_or_else(|| home::home_dir().unwrap_or_default().display().to_string());
    let roots = match platform {
        "darwin" => vec!["/Applications".to_string(), join(&home, "Applications")],
        "win32" => {
            let data = env.get("ProgramData").or_else(|| env.get("PROGRAMDATA")).filter(|s| !s.is_empty()).cloned();
            vec![join(&data.unwrap_or_else(|| "C:\\ProgramData".into()), "Ableton")]
        }
        _ => vec![],
    };
    installed_lives_in(&roots, &crate::config::live_preferences_dir(env), platform)
}

/// `installed_lives` in these folders, with Live's settings in `preferences`.
pub fn installed_lives_in(roots: &[String], preferences: &str, platform: &str) -> Vec<InstalledLive> {
    let names = |folder: &str| -> Vec<String> {
        let mut names: Vec<_> = std::fs::read_dir(folder)
            .into_iter()
            .flatten()
            .filter_map(Result::ok)
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    };
    let opened = names(preferences);
    let mut found = vec![];
    for root in roots {
        for name in names(root) {
            let (app, version) = match platform {
                "darwin" if name.starts_with("Ableton Live") && name.ends_with(".app") => {
                    let app = join(root, &name);
                    let plist = std::fs::read_to_string(join(&app, "Contents/Info.plist")).unwrap_or_default();
                    (app, bundle_version(&plist))
                }
                "win32" if name.starts_with("Live ") => {
                    let program = join(&join(root, &name), "Program");
                    let Some(exe) =
                        names(&program).into_iter().find(|n| n.starts_with("Ableton Live") && n.to_ascii_lowercase().ends_with(".exe"))
                    else {
                        continue;
                    };
                    let major = name.split_whitespace().nth(1).filter(|word| word.bytes().all(|b| b.is_ascii_digit())).map(str::to_string);
                    (join(&program, &exe), major)
                }
                _ => continue,
            };
            let file = std::path::Path::new(&app).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            let name = file.strip_suffix(".app").or_else(|| file.strip_suffix(".exe")).unwrap_or(&file).to_string();
            let major = version.as_deref().and_then(|v| v.split('.').next()).map(str::to_string);
            let opened = opened.iter().any(|folder| {
                folder
                    .strip_prefix("Live ")
                    .is_some_and(|v| major.as_ref().is_none_or(|major| v == major || v.starts_with(&format!("{major}."))))
            });
            found.push(InstalledLive { app, name, version, opened });
        }
    }
    found
}

/// "12.4.15b6" from an app's Info.plist, as XML: its CFBundleShortVersionString, without the build after it.
fn bundle_version(plist: &str) -> Option<String> {
    let after = plist.split("<key>CFBundleShortVersionString</key>").nth(1)?;
    let value = after.trim_start().strip_prefix("<string>")?.split("</string>").next()?;
    value.split_whitespace().next().map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bridge_setup::Ran;
    use futures::FutureExt;
    use std::{cell::RefCell, rc::Rc};

    #[test]
    fn the_open_live_is_named_by_its_app() {
        assert_eq!(
            live_app_of("/Applications/Ableton Live 12 Suite.app/Contents/MacOS/Live"),
            Some(OpenLive { app: "/Applications/Ableton Live 12 Suite.app".into() })
        );
        for other in ["/Applications/Ableton Live 12 Suite.app/Contents/Helpers/ExtensionHost/node", "/usr/bin/Live", ""] {
            assert_eq!(live_app_of(other), None, "{other}");
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn live_is_found_quit_waited_for_and_opened_again() {
        let calls = Rc::new(RefCell::new(Vec::<String>::new()));
        let running = Rc::new(RefCell::new(2u32));
        let run: Run = {
            let calls = calls.clone();
            let running = running.clone();
            Rc::new(move |command, args, _| {
                calls.borrow_mut().push(format!("{command} {}", args.join(" ")));
                let ran = match command.as_str() {
                    "ps" => Ran {
                        code: 0,
                        stdout: "/sbin/launchd\n/Applications/Ableton Live 12 Beta.app/Contents/MacOS/Live\n".into(),
                        stderr: String::new(),
                    },
                    // pgrep: running for two more looks, then closed.
                    "pgrep" => {
                        let left = *running.borrow();
                        *running.borrow_mut() = left.saturating_sub(1);
                        Ran { code: if left > 0 { 0 } else { 1 }, stdout: String::new(), stderr: String::new() }
                    }
                    _ => Ran { code: 0, stdout: String::new(), stderr: String::new() },
                };
                async move { ran }.boxed_local()
            })
        };
        let env = Env::new();
        let live = open_live(&run, "darwin", &env).await.unwrap();
        assert_eq!(live.app, "/Applications/Ableton Live 12 Beta.app");
        assert!(ask_to_quit(&run, "darwin", &env).await);
        assert!(closed(&run, "darwin", &env, &Signal::new()).await);
        assert!(start(&run, "darwin", &env, Some(&live.app), None).await);
        assert!(start(&run, "darwin", &env, None, None).await);
        {
            let calls = calls.borrow();
            assert_eq!(
                calls[1],
                "osascript -e ignoring application responses -e tell application id \"com.ableton.live\" to quit -e end ignoring"
            );
            assert_eq!(calls.iter().filter(|c| c.starts_with("pgrep")).count(), 3);
            assert_eq!(&calls[calls.len() - 2..], ["open /Applications/Ableton Live 12 Beta.app", "open -b com.ableton.live"]);
        }
        // Waiting stops when asked to.
        *running.borrow_mut() = 100;
        let stop = Signal::new();
        stop.cancel();
        assert!(!closed(&run, "darwin", &env, &stop).await);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn on_windows_live_is_found_closed_opened_and_read_through_powershell() {
        let calls = Rc::new(RefCell::new(Vec::<(String, Vec<String>)>::new()));
        let run: Run = {
            let calls = calls.clone();
            Rc::new(move |command, args, _| {
                let script = args.last().cloned().unwrap_or_default();
                calls.borrow_mut().push((command, args));
                let stdout = if script.contains("Select-Object -First 1 -ExpandProperty Path") {
                    "C:\\ProgramData\\Ableton\\Live 12 Suite\\Program\\Ableton Live 12 Suite.exe\r\n"
                } else if script.contains("Start-Process") {
                    "started\r\n"
                } else if script.contains("ProductVersion") {
                    "12.4.15.0\r\n"
                } else {
                    ""
                };
                let ran = Ran { code: 0, stdout: stdout.into(), stderr: String::new() };
                async move { ran }.boxed_local()
            })
        };
        let env = Env::from([("SystemRoot".to_string(), "C:\\Windows".to_string())]);
        let powershell = "C:\\Windows\\System32\\WindowsPowerShell\\v1.0\\powershell.exe";
        let live = open_live(&run, "win32", &env).await.unwrap();
        assert_eq!(live.app, "C:\\ProgramData\\Ableton\\Live 12 Suite\\Program\\Ableton Live 12 Suite.exe");
        assert!(ask_to_quit(&run, "win32", &env).await);
        // The same Live again, by its path; a path with a quote in it stays one string.
        assert!(start(&run, "win32", &env, Some(&live.app), None).await);
        assert!(start(&run, "win32", &env, Some("C:\\Live's\\Ableton Live 12.exe"), None).await);
        // Without one, the newest Live under ProgramData.
        assert!(start(&run, "win32", &env, None, None).await);
        assert_eq!(version(&run, "win32", &env, Some(&live.app)).await.as_deref(), Some("12.4"));
        let calls = calls.borrow();
        for (command, args) in calls.iter() {
            assert_eq!(command, powershell);
            assert_eq!(&args[..3], ["-NoProfile", "-NonInteractive", "-Command"]);
        }
        let scripts: Vec<&str> = calls.iter().map(|(_, args)| args[3].as_str()).collect();
        assert_eq!(
            scripts[0],
            "Get-Process -Name 'Ableton Live*' -ErrorAction SilentlyContinue | Select-Object -First 1 -ExpandProperty Path"
        );
        assert_eq!(
            scripts[1],
            "Get-Process -Name 'Ableton Live*' -ErrorAction SilentlyContinue | ForEach-Object { [void]$_.CloseMainWindow() }"
        );
        assert_eq!(
            scripts[2],
            "$app = 'C:\\ProgramData\\Ableton\\Live 12 Suite\\Program\\Ableton Live 12 Suite.exe'; if ($app) { Start-Process -FilePath $app; 'started' }"
        );
        assert_eq!(scripts[3], "$app = 'C:\\Live''s\\Ableton Live 12.exe'; if ($app) { Start-Process -FilePath $app; 'started' }");
        assert_eq!(
            scripts[4],
            "$app = (Get-ChildItem -Path (Join-Path $env:ProgramData 'Ableton\\Live*\\Program\\Ableton Live*.exe') -ErrorAction SilentlyContinue | Sort-Object LastWriteTime -Descending | Select-Object -First 1 -ExpandProperty FullName); if ($app) { Start-Process -FilePath $app; 'started' }"
        );
        assert_eq!(scripts[5], "(Get-Item -LiteralPath 'C:\\ProgramData\\Ableton\\Live 12 Suite\\Program\\Ableton Live 12 Suite.exe').VersionInfo.ProductVersion");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_sets_file_opens_in_the_live_thats_running_by_that_lives_own_app() {
        let calls = Rc::new(RefCell::new(Vec::<String>::new()));
        let running = Rc::new(RefCell::new(true));
        let run: Run = {
            let (calls, running) = (calls.clone(), running.clone());
            Rc::new(move |command, args, _| {
                calls.borrow_mut().push(format!("{command} {}", args.join(" ")));
                let script = args.last().cloned().unwrap_or_default();
                let stdout = match command.as_str() {
                    "ps" if *running.borrow() => "/Applications/Ableton Live 12 Beta.app/Contents/MacOS/Live\n".into(),
                    _ if script.contains("Select-Object -First 1 -ExpandProperty Path") && *running.borrow() => {
                        "C:\\ProgramData\\Ableton\\Live 12 Beta\\Program\\Ableton Live 12 Beta.exe\r\n".into()
                    }
                    _ if script.contains("Start-Process") => "started\r\n".into(),
                    _ => String::new(),
                };
                async move { Ran { code: 0, stdout, stderr: String::new() } }.boxed_local()
            })
        };
        let env = Env::from([("SystemRoot".to_string(), "C:\\Windows".to_string())]);
        assert!(open_set_file(&run, "darwin", &env, "/Music/Song A Project/Song A.als").await);
        assert!(open_set_file(&run, "win32", &env, "C:\\Music\\Bob's Song.als").await);
        {
            let calls = calls.borrow();
            assert_eq!(calls[1], "open -a /Applications/Ableton Live 12 Beta.app /Music/Song A Project/Song A.als");
            assert!(
                calls[3].ends_with(
                    "$app = 'C:\\ProgramData\\Ableton\\Live 12 Beta\\Program\\Ableton Live 12 Beta.exe'; if ($app) { Start-Process -FilePath $app -ArgumentList '\"C:\\Music\\Bob''s Song.als\"'; 'started' }"
                ),
                "{}",
                calls[3]
            );
        }
        // No Live running: nothing is opened with whatever app .als files go to.
        *running.borrow_mut() = false;
        calls.borrow_mut().clear();
        assert!(!open_set_file(&run, "darwin", &env, "/Music/Song A Project/Song A.als").await);
        assert!(!open_set_file(&run, "win32", &env, "C:\\Music\\Song.als").await);
        assert!(!open_set_file(&run, "linux", &env, "/x.als").await);
        assert!(calls.borrow().iter().all(|call| !call.starts_with("open ") && !call.contains("Start-Process")), "{:?}", calls.borrow());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn on_windows_live_not_found_or_not_started_says_so() {
        let run: Run = Rc::new(|_, _, _| async { Ran { code: 0, stdout: String::new(), stderr: String::new() } }.boxed_local());
        let env = Env::new();
        assert_eq!(open_live(&run, "win32", &env).await, None);
        assert!(!start(&run, "win32", &env, None, None).await, "no Live under ProgramData: nothing started");
        assert_eq!(version(&run, "win32", &env, Some("C:\\x.exe")).await, None);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn lives_version_is_read_from_its_app() {
        for (raw, want) in [("12.4.15b5", Some("12.4")), ("12.4b15", Some("12.4")), ("12", Some("12")), ("", None), ("beta", None)] {
            assert_eq!(major_minor(raw).as_deref(), want, "{raw}");
        }
        let calls = Rc::new(RefCell::new(Vec::<String>::new()));
        let run: Run = {
            let calls = calls.clone();
            Rc::new(move |command, args, _| {
                calls.borrow_mut().push(format!("{command} {}", args.join(" ")));
                let stdout = match command.as_str() {
                    "ps" => "/Applications/Ableton Live 12 Beta.app/Contents/MacOS/Live\n",
                    "defaults" => "12.4b15\n",
                    _ => "",
                };
                let ran = Ran { code: 0, stdout: stdout.into(), stderr: String::new() };
                async move { ran }.boxed_local()
            })
        };
        let env = Env::new();
        assert_eq!(version(&run, "darwin", &env, None).await.as_deref(), Some("12.4"));
        assert_eq!(calls.borrow()[1], "defaults read /Applications/Ableton Live 12 Beta.app/Contents/Info CFBundleShortVersionString");
        assert_eq!(version(&run, "linux", &env, Some("/x")).await, None);
    }

    /// What osascript and `open` did, and what osascript answers.
    fn darwin(said: Ran) -> (Run, Rc<RefCell<Vec<String>>>) {
        let calls = Rc::new(RefCell::new(Vec::<String>::new()));
        let run: Run = {
            let calls = calls.clone();
            Rc::new(move |command, args, _| {
                calls.borrow_mut().push(format!("{command} {}", args.join(" ")));
                let ran = if command == "osascript" { said.clone() } else { Ran::default() };
                async move { ran }.boxed_local()
            })
        };
        (run, calls)
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_mac_that_doesnt_let_kumi_ask_live_to_quit_is_told_apart() {
        let env = Env::new();
        let refused = |code: &str| Ran {
            code: 1,
            stdout: String::new(),
            stderr: format!("29:63: execution error: Not authorized to send Apple events to Ableton Live 12 Suite. ({code})\n"),
        };
        for code in ["-1743", "-1744"] {
            let (run, calls) = darwin(refused(code));
            assert!(!ask_to_quit(&run, "darwin", &env).await, "{code}");
            assert_eq!(calls.borrow().len(), 1, "asked once, never again by itself");
        }
        // Asked: osascript is done once the request is sent. Any other trouble (Live already gone, say) isn't the
        // system's refusal, so Kumi watches for Live to close as it always has.
        let (run, _) = darwin(Ran::default());
        assert!(ask_to_quit(&run, "darwin", &env).await);
        let (run, _) = darwin(Ran { code: 1, stdout: String::new(), stderr: "execution error: Application isn't running. (-600)".into() });
        assert!(ask_to_quit(&run, "darwin", &env).await);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn live_opens_again_with_the_set_it_had_open() {
        let env = Env::new();
        let (run, calls) = darwin(Ran::default());
        let set = "/Users/me/Music/Night Drive Project/Night Drive.als";
        assert!(start(&run, "darwin", &env, Some("/Applications/Ableton Live 12 Suite.app"), Some(set)).await);
        assert!(start(&run, "darwin", &env, None, Some(set)).await);
        assert_eq!(
            *calls.borrow(),
            [format!("open -a /Applications/Ableton Live 12 Suite.app {set}"), format!("open -b com.ableton.live {set}"),]
        );
        let scripts = Rc::new(RefCell::new(Vec::<String>::new()));
        let run: Run = {
            let scripts = scripts.clone();
            Rc::new(move |_, args, _| {
                scripts.borrow_mut().push(args.last().cloned().unwrap_or_default());
                async { Ran { code: 0, stdout: "started\r\n".into(), stderr: String::new() } }.boxed_local()
            })
        };
        let app = "C:\\ProgramData\\Ableton\\Live 12 Suite\\Program\\Ableton Live 12 Suite.exe";
        assert!(start(&run, "win32", &env, Some(app), Some("C:\\Users\\me\\Rock 'n' Roll.als")).await);
        assert_eq!(
            scripts.borrow()[0],
            format!("$app = '{app}'; if ($app) {{ Start-Process -FilePath $app -ArgumentList '\"C:\\Users\\me\\Rock ''n'' Roll.als\"'; 'started' }}")
        );
    }

    #[test]
    fn which_live_is_here_is_read_from_its_folders() {
        let root = tempfile::tempdir().unwrap();
        let path = |name: &str| root.path().join(name).to_string_lossy().into_owned();
        let make = |name: &str| std::fs::create_dir_all(root.path().join(name)).unwrap();
        let live = |name: &str, version: Option<&str>| InstalledLive {
            app: format!("/Applications/{name}.app"),
            name: name.into(),
            version: version.map(str::to_string),
            opened: false,
        };
        let here = |lives: &[InstalledLive]| decide(lives, &opened_versions(&path("Preferences")), &[path("User Library")]);
        assert_eq!(here(&[]), LiveInstalled::Missing, "nothing here");
        let eleven = live("Ableton Live 11 Suite", Some("11.3.21"));
        assert_eq!(here(std::slice::from_ref(&eleven)), LiveInstalled::Older("11.3.21".into()), "installed, never opened");
        make("Preferences/Live 11.3.13");
        assert_eq!(here(std::slice::from_ref(&eleven)), LiveInstalled::Older("11.3.13".into()), "opened: the version it ran as");
        // Live 12 here, never opened: the one to open, by its app (a version unread from its name).
        let twelve = live("Ableton Live 12 Beta", None);
        assert_eq!(here(&[eleven.clone(), twelve.clone()]), LiveInstalled::NeverOpened("/Applications/Ableton Live 12 Beta.app".into()));
        make("Preferences/Live 12.4.15b6");
        make("Preferences/Live Reports");
        assert_eq!(here(&[eleven, twelve]), LiveInstalled::Ready);
        // Live installed where Kumi doesn't look, that has made its User Library.
        let elsewhere = decide(&[], &[], &[path("Library")]);
        assert_eq!(elsewhere, LiveInstalled::Missing);
        make("Library");
        assert_eq!(decide(&[], &[], &[path("Library")]), LiveInstalled::Ready);
        // Remote Scripts named by the producer (in any case on Windows), and Linux, where Live doesn't run.
        let named = Env::from([("KUMI_REMOTE_SCRIPTS_DIR".to_string(), path("Remote Scripts"))]);
        assert_eq!(installed(&named, "darwin"), LiveInstalled::Ready);
        let windows = Env::from([("kumi_remote_scripts_dir".to_string(), path("Remote Scripts"))]);
        assert_eq!(installed(&windows, "win32"), LiveInstalled::Ready);
        assert_eq!(installed(&Env::new(), "linux"), LiveInstalled::Missing);
        assert_eq!(settings_menu("darwin").as_deref(), Some("Settings is in Live's Live menu (⌘,)."));
        assert_eq!(settings_menu("linux"), None);
    }

    #[test]
    fn installed_lives_are_read_from_their_folders_with_their_versions_and_whether_they_were_opened() {
        let dir = tempfile::tempdir().unwrap();
        let root = |path: &str| dir.path().join(path).to_string_lossy().into_owned();
        let put = |path: &str, text: &str| {
            let path = dir.path().join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        };
        let plist = |version: &str| {
            format!("<?xml version=\"1.0\"?>\n<plist><dict>\n\t<key>CFBundleIdentifier</key>\n\t<string>com.ableton.live</string>\n\t<key>CFBundleShortVersionString</key>\n\t<string>{version}</string>\n</dict></plist>\n")
        };
        put("Applications/Ableton Live 12 Beta.app/Contents/Info.plist", &plist("12.4.15b6 (2026-10-01_8378f48a19)"));
        put("Applications/Ableton Live 11 Suite.app/Contents/Info.plist", &plist("11.3.21"));
        put("Applications/Logic Pro.app/Contents/Info.plist", &plist("11.0"));
        put("Preferences/Live 12.4.15b4/Library.cfg", "");
        let lives = installed_lives_in(&[root("Applications"), root("nowhere")], &root("Preferences"), "darwin");
        let seen: Vec<_> = lives.iter().map(|live| (live.name.as_str(), live.version.as_deref(), live.opened)).collect();
        assert_eq!(seen, [("Ableton Live 11 Suite", Some("11.3.21"), false), ("Ableton Live 12 Beta", Some("12.4.15b6"), true)]);
        assert!(lives[1].app.ends_with("Ableton Live 12 Beta.app"));
        // Windows: each Live in its own folder under ProgramData\Ableton, named with its major.
        put("Ableton/Live 12 Suite/Program/Ableton Live 12 Suite.exe", "");
        put("Ableton/Live 12 Suite/Program/Ableton Index.exe", "");
        put("Ableton/Live 12 Trial/Resources/readme.txt", "");
        let lives = installed_lives_in(&[root("Ableton")], &root("nowhere"), "win32");
        assert_eq!(lives.len(), 1);
        assert_eq!((lives[0].name.as_str(), lives[0].version.as_deref(), lives[0].opened), ("Ableton Live 12 Suite", Some("12"), false));
        assert!(installed_lives_in(&[root("Applications")], &root("Preferences"), "linux").is_empty());
    }
}
