//! Compatibility with the install layout used before native releases.
use super::*;
/// The fallback is used only when the user explicitly rolls back to a JavaScript release.
pub fn launcher(windows: bool) -> &'static str {
    if windows {
        "@echo off\r\ngoto start\r\n::::::::::::::::::::::::::::::::::::::::::::::::::::::::::::::::\r\n::::::::::::::::::::::::::::::::::::::::::::::::::::::::::::::::\r\n::::::::::::::::::::::::::::::::::::::::::::::::::::::::::::::::\r\n::::::::::::::::::::::::::::::::::::::::::::::::::::::::::::::::\r\n::::::::::::::::::::::::::::::::::::::::::::::::::::::::::::::::\r\nexit /b %errorlevel%\r\n:start\r\nrem Kumi's launcher, written by its installer. cmd reads a running batch file from where it stopped:\r\nrem one replaced while it runs resumes in the colons above (labels) and exits.\r\nsetlocal\r\nif not defined KUMI_HOME for %%I in (\"%~dp0..\") do set \"KUMI_HOME=%%~fI\"\r\nset \"KUMI_INSTALLED=1\"\r\nif exist \"%KUMI_HOME%\\app\\kumi.exe\" goto native\r\n\"%KUMI_HOME%\\node\\node.exe\" \"%KUMI_HOME%\\app\\apps\\kumi\\bin\\kumi.mjs\" %*\r\nexit /b %errorlevel%\r\n:native\r\n\"%KUMI_HOME%\\app\\kumi.exe\" %*\r\nexit /b %errorlevel%\r\n"
    } else {
        "#!/bin/sh\nKUMI_HOME=\"${KUMI_HOME:-$(cd \"$(dirname \"$0\")/..\" && pwd)}\"\nexport KUMI_HOME KUMI_INSTALLED=1\nif [ -x \"$KUMI_HOME/app/kumi\" ]; then\n  exec \"$KUMI_HOME/app/kumi\" \"$@\"\nfi\nexec \"$KUMI_HOME/node/bin/node\" \"$KUMI_HOME/app/apps/kumi/bin/kumi.mjs\" \"$@\"\n"
    }
}
/// Repair the installed launcher only from its active app, never an update probe or checkout.
pub fn ensure_native_launcher(env: &Env, executable: &Path) -> std::io::Result<bool> {
    if env.get("KUMI_INSTALLED").is_none_or(|s| s != "1") {
        return Ok(false);
    }
    let home = kumi_home(env);
    let app = Path::new(&home).join("app");
    if app.canonicalize().ok().zip(executable.parent().and_then(|p| p.canonicalize().ok())).is_none_or(|(app, folder)| app != folder) {
        return Ok(false);
    }
    write_launcher(&home)?;
    Ok(true)
}
pub fn write_launcher(home: &str) -> std::io::Result<()> {
    write_launcher_for(home, cfg!(windows))
}

// The 1.7.4 and 1.7.5 installers' launcher, which the first native start replaces.
#[cfg(test)]
const LEGACY_WINDOWS_LAUNCHER: &str = "@echo off\nrem Kumi's launcher, written by its installer: Kumi runs on its own Node, whatever Node this computer has.\nsetlocal\nfor %%I in (\"%~dp0..\") do set \"KUMI_HOME=%%~fI\"\nset \"KUMI_INSTALLED=1\"\n\"%KUMI_HOME%\\node\\node.exe\" \"%KUMI_HOME%\\app\\apps\\kumi\\bin\\kumi.mjs\" %*\n";

/// The native template, with either line ending the installer may have written. Any other content (the
/// legacy Node launcher among them) is replaced: a cmd still running it resumes in the colons and exits.
fn compatible_windows_launcher(current: &str) -> bool {
    current.replace("\r\n", "\n").trim_end_matches('\n') == launcher(true).replace("\r\n", "\n").trim_end_matches('\n')
}

fn write_launcher_for(home: &str, windows: bool) -> std::io::Result<()> {
    use std::io::Write;
    let home = Path::new(home);
    let path = home.join("bin").join(if windows { "kumi.cmd" } else { "kumi" });
    let contents = launcher(windows);
    if let Ok(current) = fs::read_to_string(&path) {
        if current == contents {
            return Ok(());
        }
        // cmd.exe resumes a batch file at its old byte offset when the child exits. A native
        // launcher is left as it is (even LF for CRLF would move its offsets); the template puts
        // the legacy launcher's resume offsets in its padding, so that one is replaced.
        if windows && compatible_windows_launcher(&current) {
            return Ok(());
        }
    }
    fs::create_dir_all(path.parent().unwrap())?;
    let temporary = path.with_extension(format!("new-{}", std::process::id()));
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o755);
    }
    let result = (|| {
        let mut file = options.open(&temporary)?;
        file.write_all(contents.as_bytes())?;
        file.sync_all()?;
        drop(file);
        fs::rename(&temporary, &path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn powershell_installer_and_native_windows_launcher_have_the_same_template() {
        let script = include_str!("../../../../install.ps1").replace("\r\n", "\n");
        let template = script.split_once("$launcher = @'\n").unwrap().1.split_once("\n'@").unwrap().0;
        assert_eq!(template.replace('\n', "\r\n") + "\r\n", launcher(true));
    }

    #[test]
    fn native_windows_launcher_keeps_its_bytes_with_each_installer_line_ending() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bin/kumi.cmd");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        for contents in [launcher(true).into(), launcher(true).replace("\r\n", "\n"), launcher(true).replace("\r\n", "\n") + "\r\n"] {
            fs::write(&path, &contents).unwrap();
            write_launcher_for(dir.path().to_str().unwrap(), true).unwrap();
            assert_eq!(fs::read_to_string(&path).unwrap(), contents);
        }
    }

    #[test]
    fn the_first_native_start_replaces_the_legacy_windows_launcher() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bin/kumi.cmd");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        for contents in [
            LEGACY_WINDOWS_LAUNCHER.into(),
            LEGACY_WINDOWS_LAUNCHER.replace('\n', "\r\n"),
            LEGACY_WINDOWS_LAUNCHER.trim_end_matches('\n').to_string() + "\r\n",
        ] {
            fs::write(&path, &contents).unwrap();
            write_launcher_for(dir.path().to_str().unwrap(), true).unwrap();
            assert_eq!(fs::read_to_string(&path).unwrap(), launcher(true));
        }
    }

    /// What cmd.exe runs next when it resumes a batch file at `offset`: lines read from there, labels
    /// (a first non-blank `:`) and empty lines skipped.
    fn resumed_at(file: &str, offset: usize) -> &str {
        let mut rest = &file[offset..];
        loop {
            let (line, after) = rest.split_once('\n').unwrap_or((rest, ""));
            let line = line.trim_end_matches('\r').trim_start();
            if !(line.is_empty() || line.starts_with(':')) || after.is_empty() {
                return line;
            }
            rest = after;
        }
    }

    #[test]
    fn a_cmd_still_running_a_replaced_launcher_resumes_onto_exit() {
        let native = launcher(true);
        // The legacy launcher, in each line ending its installers wrote, resumes after its Node line.
        for legacy in [
            LEGACY_WINDOWS_LAUNCHER.to_string(),
            LEGACY_WINDOWS_LAUNCHER.replace('\n', "\r\n"),
            LEGACY_WINDOWS_LAUNCHER.trim_end_matches('\n').to_string() + "\r\n",
        ] {
            assert_eq!(resumed_at(native, legacy.len()), "exit /b %errorlevel%", "{} bytes", legacy.len());
        }
        // So does any offset in the padding, with room for a longer old launcher.
        let start = native.find("goto start\r\n").unwrap() + "goto start\r\n".len();
        let end = native.find("exit /b %errorlevel%").unwrap();
        assert!(end - start >= 300, "{start}..{end}");
        for offset in start..end {
            assert_eq!(resumed_at(native, offset), "exit /b %errorlevel%", "offset {offset}");
        }
        // Run from the top, it jumps over the padding; KUMI_HOME is normalized and stays local.
        assert!(native.starts_with("@echo off\r\ngoto start\r\n"));
        assert!(native.contains("\r\n:start\r\n"));
        assert!(native.contains("\r\nsetlocal\r\n"));
        assert!(native.contains("for %%I in (\"%~dp0..\") do set \"KUMI_HOME=%%~fI\""));
    }
}
/// Legacy probes require this file; native updates retain it so old rollback can return here.
pub fn legacy_entry(app: &str) -> String {
    join(app, "apps/kumi/bin/kumi.mjs")
}
pub fn has_app(app: &str) -> bool {
    Path::new(&join(app, &executable_name("kumi"))).is_file() || Path::new(&legacy_entry(app)).is_file()
}
/// Complete a receipt-bound bridge transition after the old updater's application swap. Best effort:
/// Kumi opens whatever happens here, and keeps chatting through the existing bridge until it switches.
pub async fn finish_legacy_transition(io: &InstalledIo) {
    let home = kumi_home(&io.env);
    let app = join(&home, "app");
    let native = fs::read(join(&app, "package.json"))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
        .is_some_and(|metadata| metadata["runtime"] == "rust-native");
    if !native {
        return;
    }
    let Some(config) = find_bridge_config(&io.env) else { return };
    let Some(server) = read_bridge_server(&config).ok().filter(|s| !s.native()) else { return };
    let Some(bundled) = bridge_version(&app) else { return };
    if server.version.as_deref().is_none_or(|version| version != bundled && !newer_version(&bundled, version)) {
        return;
    }
    let unchanged = remote_script_unchanged(&io.env, &app, &config, &server);
    // Live has the existing Remote Script loaded: wait, quietly; the folder Live loaded is never moved
    // under it. A connect is instant; asking the system about Live is not (Windows). With the same
    // Remote Script nothing is missing meanwhile (Kumi runs its own native host), so the session
    // doesn't mention it either.
    if crate::bridge_setup::remote_script_answers(&config).await || live_open(io, io.run.clone().unwrap_or_else(default_run)).await {
        return;
    }
    // Byte for byte the same Remote Script (1.7.5's 1.0.74 for this release's): only the host and its
    // receipt change, so the switch is quiet, failing included (a later start tries again).
    let said = Rc::new(Said::default());
    let mut setup = crate::bridge_setup::BridgeSetupIo::new(if unchanged { said.clone() } else { io.out.clone() }, io.env.clone());
    setup.input = io.input.clone();
    setup.bridge_dir = Some(app.clone());
    setup.prepared = Some(join(&app, "bridge"));
    setup.yes = true;
    setup.wait_ms = Some(0);
    setup.run = io.run.clone();
    setup.live_running = io.live_running.clone();
    match crate::bridge_setup::setup_bridge(setup).await {
        Ok(0) => {}
        _ if unchanged => {}
        Ok(_) => io.out.write("Kumi can still open. To finish switching the bridge, close Live and run: kumi bridge\n"),
        Err(reason) => io.out.write(&format!(
            "Kumi can still open; the bridge couldn't switch ({reason}). To finish switching it, close Live and run: kumi bridge\n"
        )),
    }
}

/// Whether the Remote Script installed for a JavaScript bridge is, byte for byte, the one the app at
/// `app` brings: then switching changes only the host, which native Kumi already runs.
fn remote_script_unchanged(env: &Env, app: &str, config: &str, server: &crate::doctor::BridgeServer) -> bool {
    let scripts = server
        .package_root()
        .and_then(|package| crate::bridge_setup::owner_paths(config, &package, &kumi_home(env)))
        .map_or_else(|| crate::config::remote_scripts_dir(env), |(_, _, scripts)| scripts);
    crate::bridge_setup::remote_script_unchanged(&join(app, "bridge/package"), &scripts)
}

/// Whether a pending switch from a JavaScript bridge would change only its host (the installed Remote
/// Script is the app's own); the session then has nothing to ask of the producer.
pub fn only_the_host_differs(env: &Env) -> bool {
    let app = join(&kumi_home(env), "app");
    let Some(config) = find_bridge_config(env) else { return false };
    let Some(server) = read_bridge_server(&config).ok().filter(|s| !s.native()) else { return false };
    remote_script_unchanged(env, &app, &config, &server)
}

/// What a quiet bridge switch said, kept for when it fails.
#[derive(Default)]
struct Said(std::cell::RefCell<String>);
impl crate::tui::tty::TtyOutput for Said {
    fn is_tty(&self) -> bool {
        false
    }
    fn columns(&self) -> Option<i32> {
        None
    }
    fn rows(&self) -> Option<i32> {
        None
    }
    fn write(&self, data: &str) {
        self.0.borrow_mut().push_str(data);
    }
}

pub(super) struct BridgeRollback {
    command: String,
    args: Vec<String>,
    run: Run,
    /// The retained generation the rollback restores.
    previous: String,
}
impl BridgeRollback {
    pub(super) async fn apply(&self) -> Result<(), RuntimeError> {
        let result = (self.run)(self.command.clone(), self.args.clone(), None).await;
        crate::bridge_setup::lifecycle_answer(result).map(|_| ()).map_err(RuntimeError::plain)
    }
    /// Put the restored generation's Live extension in Live, in place of this bridge's: each bridge checks that its
    /// extension carries its own registry. What to tell the producer, if anything.
    pub(super) fn place_extension(&self, env: &Env) -> Option<String> {
        let folder = crate::live_extension::live_extensions_dir(env, kumi_runtime::system::platform())?;
        let again = format!("run {} bridge once to put it back", *KUMI);
        let Some(source) = crate::live_extension::extension_source(&self.previous) else {
            return crate::live_extension::installed_extension(&folder)
                .map(|_| format!("Kumi's extension in Live is this version's, which that bridge won't use: {again}."));
        };
        Some(match crate::live_extension::install_extension(&source, &folder) {
            Ok(placed) if placed.changed => "Kumi's extension in Live went back with it.".into(),
            Ok(_) => return None,
            Err(e) => format!("Couldn't put back the Live extension that goes with that bridge ({e}): {again}."),
        })
    }
}
pub(super) async fn prepare_legacy_rollback(io: &InstalledIo, home: &str) -> Result<Option<BridgeRollback>, RuntimeError> {
    let Some(config) = find_bridge_config(&io.env) else { return Ok(None) };
    let server = read_bridge_server(&config)?;
    if !server.native() {
        return Ok(None);
    }
    let package = server.package_root().ok_or_else(|| RuntimeError::plain("The bridge's package could not be found."))?;
    let Some((state, secret, scripts)) = crate::bridge_setup::owner_paths(&config, &package, home) else {
        return Err(RuntimeError::plain(
            "The earlier Kumi needs its JavaScript bridge, but its owner receipt could not be found. Nothing was changed.",
        ));
    };
    let receipt: Value = serde_json::from_slice(&fs::read(join(&state, "install-receipt.json")).map_err(error)?).map_err(error)?;
    let previous = receipt["previous"]["packageRoot"].as_str().filter(|path| Path::new(path).is_absolute());
    let schema = |root: &str| {
        fs::read(join(root, "release-manifest.json"))
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
            .and_then(|value| value["schema"].as_str().map(str::to_string))
    };
    if schema(&package).as_deref() != Some("ableton-mcp-native-release/v1")
        || !previous.and_then(schema).is_some_and(|schema| matches!(schema.as_str(), "ableton-mcp-release/v1" | "ableton-mcp-release/v2"))
    {
        return Err(RuntimeError::plain(
            "The earlier Kumi needs its JavaScript bridge, but no retained JavaScript bridge generation is available. Nothing was changed.",
        ));
    }
    let run = io.run.clone().unwrap_or_else(default_run);
    if live_open(io, run.clone()).await {
        return Err(RuntimeError::plain("Quit Live (save your work first), then run: kumi update --rollback. Restoring this Kumi also restores its previous bridge; nothing was changed."));
    }
    let args = vec![
        "lifecycle".into(),
        "rollback".into(),
        "--package-root".into(),
        package,
        "--state-dir".into(),
        state,
        "--config".into(),
        config,
        "--secret".into(),
        secret,
        "--remote-scripts-dir".into(),
        scripts,
        "--apply".into(),
        "--confirm-live-stopped".into(),
    ];
    Ok(Some(BridgeRollback { command: server.command.unwrap(), args, run, previous: previous.unwrap().into() }))
}
