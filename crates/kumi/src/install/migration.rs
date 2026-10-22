//! Compatibility with the install layout used before native releases.
use super::*;
/// The kumi command the installers write, and Kumi repairs from its active app. It runs the native app; one rolled
/// back to its JavaScript release, on its own Node; with no app, the Kumi before it, saying so; with neither, it says
/// how to repair Kumi. install.sh and install.ps1 write the same, and tests keep the three, and the one-liner in them
/// ([`kumi_runtime::INSTALL_SH`], [`kumi_runtime::INSTALL_PS1`]), the same.
pub fn launcher(windows: bool) -> &'static str {
    if windows {
        WINDOWS_LAUNCHER
    } else {
        UNIX_LAUNCHER
    }
}
/// cmd reads a running batch file from where it stopped, by byte: one replaced while it runs goes on from the same
/// byte of the new one. So this is [`PREVIOUS_WINDOWS_LAUNCHER`] byte for byte up to its end but for its Node line,
/// whose bytes two lines take, and the rest comes after.
const WINDOWS_LAUNCHER: &str = "@echo off\r\ngoto start\r\n::::::::::::::::::::::::::::::::::::::::::::::::::::::::::::::::\r\n::::::::::::::::::::::::::::::::::::::::::::::::::::::::::::::::\r\n::::::::::::::::::::::::::::::::::::::::::::::::::::::::::::::::\r\n::::::::::::::::::::::::::::::::::::::::::::::::::::::::::::::::\r\n::::::::::::::::::::::::::::::::::::::::::::::::::::::::::::::::\r\nexit /b %errorlevel%\r\n:start\r\nrem Kumi's launcher, written by its installer. cmd reads a running batch file from where it stopped:\r\nrem one replaced while it runs resumes in the colons above (labels) and exits.\r\nsetlocal\r\nif not defined KUMI_HOME for %%I in (\"%~dp0..\") do set \"KUMI_HOME=%%~fI\"\r\nset \"KUMI_INSTALLED=1\"\r\nif exist \"%KUMI_HOME%\\app\\kumi.exe\" goto native\r\ngoto other\r\nrem With the line above, as many bytes as the old Node line\r\nexit /b %errorlevel%\r\n:native\r\n\"%KUMI_HOME%\\app\\kumi.exe\" %*\r\nexit /b %errorlevel%\r\n:other\r\nif exist \"%KUMI_HOME%\\node\\node.exe\" if exist \"%KUMI_HOME%\\app\\apps\\kumi\\bin\\kumi.mjs\" goto node\r\nif exist \"%KUMI_HOME%\\app.previous\\kumi.exe\" goto previous\r\n>&2 echo Kumi's app is missing: run the installer again: irm https://github.com/user1303836/kumi/releases/latest/download/install.ps1 ^| iex\r\nexit /b 1\r\n:node\r\n\"%KUMI_HOME%\\node\\node.exe\" \"%KUMI_HOME%\\app\\apps\\kumi\\bin\\kumi.mjs\" %*\r\nexit /b %errorlevel%\r\n:previous\r\n>&2 echo Kumi's app is missing, so the Kumi before it starts. To repair Kumi, run the installer again: irm https://github.com/user1303836/kumi/releases/latest/download/install.ps1 ^| iex\r\n\"%KUMI_HOME%\\app.previous\\kumi.exe\" %*\r\nexit /b %errorlevel%\r\n";
const UNIX_LAUNCHER: &str = "#!/bin/sh\nKUMI_HOME=\"${KUMI_HOME:-$(cd \"$(dirname \"$0\")/..\" && pwd)}\"\nexport KUMI_HOME KUMI_INSTALLED=1\nif [ -x \"$KUMI_HOME/app/kumi\" ]; then\n  exec \"$KUMI_HOME/app/kumi\" \"$@\"\nfi\n# Rolled back to a JavaScript release: it runs on its own Node.\nif [ -x \"$KUMI_HOME/node/bin/node\" ] && [ -f \"$KUMI_HOME/app/apps/kumi/bin/kumi.mjs\" ]; then\n  exec \"$KUMI_HOME/node/bin/node\" \"$KUMI_HOME/app/apps/kumi/bin/kumi.mjs\" \"$@\"\nfi\nif [ -x \"$KUMI_HOME/app.previous/kumi\" ]; then\n  echo \"Kumi's app is missing, so the Kumi before it starts. To repair Kumi, run the installer again: curl -fsSL https://github.com/user1303836/kumi/releases/latest/download/install.sh | sh\" >&2\n  exec \"$KUMI_HOME/app.previous/kumi\" \"$@\"\nfi\necho \"Kumi's app is missing: run the installer again: curl -fsSL https://github.com/user1303836/kumi/releases/latest/download/install.sh | sh\" >&2\nexit 1\n";
/// The native launcher before this one, from 1.7.6 on.
const PREVIOUS_WINDOWS_LAUNCHER: &str = "@echo off\r\ngoto start\r\n::::::::::::::::::::::::::::::::::::::::::::::::::::::::::::::::\r\n::::::::::::::::::::::::::::::::::::::::::::::::::::::::::::::::\r\n::::::::::::::::::::::::::::::::::::::::::::::::::::::::::::::::\r\n::::::::::::::::::::::::::::::::::::::::::::::::::::::::::::::::\r\n::::::::::::::::::::::::::::::::::::::::::::::::::::::::::::::::\r\nexit /b %errorlevel%\r\n:start\r\nrem Kumi's launcher, written by its installer. cmd reads a running batch file from where it stopped:\r\nrem one replaced while it runs resumes in the colons above (labels) and exits.\r\nsetlocal\r\nif not defined KUMI_HOME for %%I in (\"%~dp0..\") do set \"KUMI_HOME=%%~fI\"\r\nset \"KUMI_INSTALLED=1\"\r\nif exist \"%KUMI_HOME%\\app\\kumi.exe\" goto native\r\n\"%KUMI_HOME%\\node\\node.exe\" \"%KUMI_HOME%\\app\\apps\\kumi\\bin\\kumi.mjs\" %*\r\nexit /b %errorlevel%\r\n:native\r\n\"%KUMI_HOME%\\app\\kumi.exe\" %*\r\nexit /b %errorlevel%\r\n";
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

/// A launcher left as it is: the native template, with either line ending the installer may have written, and the one
/// before it written with other line endings than its own (a cmd running it would resume mid-line in the new one). Any
/// other content is replaced: a cmd still running the legacy Node launcher resumes in the colons and exits, and one
/// running the launcher before resumes where it would have.
fn compatible_windows_launcher(current: &str) -> bool {
    let lines = |text: &str| text.replace("\r\n", "\n").trim_end_matches('\n').to_string();
    lines(current) == lines(launcher(true)) || (lines(current) == lines(PREVIOUS_WINDOWS_LAUNCHER) && current != PREVIOUS_WINDOWS_LAUNCHER)
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
        // launcher is left as it is (even LF for CRLF would move its offsets). The template puts
        // the legacy launcher's resume offsets in its padding, and keeps the previous native
        // launcher's as its installers wrote it (CRLF), so those two are replaced.
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
        let script = include_str!(concat!(env!("KUMI_WORKSPACE"), "/install.ps1")).replace("\r\n", "\n");
        let template = script.split_once("$launcher = @'\n").unwrap().1.split_once("\n'@").unwrap().0;
        assert_eq!(template.replace('\n', "\r\n") + "\r\n", launcher(true));
    }

    #[test]
    fn shell_installer_and_native_unix_launcher_have_the_same_template() {
        let script = include_str!(concat!(env!("KUMI_WORKSPACE"), "/install.sh"));
        let template = script.split_once("<<'LAUNCHER'\n").unwrap().1.split_once("\nLAUNCHER\n").unwrap().0;
        assert_eq!(format!("{template}\n"), launcher(false));
    }

    #[test]
    fn every_copy_of_the_one_liners_is_kumis_own() {
        use kumi_runtime::{INSTALL_PS1, INSTALL_SH};
        let sh = include_str!(concat!(env!("KUMI_WORKSPACE"), "/install.sh"));
        let ps1 = include_str!(concat!(env!("KUMI_WORKSPACE"), "/install.ps1")).replace("\r\n", "\n");
        // Each script's own header, install.sh's word to Git Bash, and the launchers' word when Kumi's app is missing.
        assert!(sh.contains(&format!("\n#   {INSTALL_SH}\n")));
        assert!(sh.contains(&format!("open PowerShell and run: {INSTALL_PS1}\"")));
        assert!(ps1.contains(&format!("\n#   {INSTALL_PS1}\n")));
        assert_eq!(launcher(false).matches(&format!("run the installer again: {INSTALL_SH}\"")).count(), 2);
        assert_eq!(launcher(true).matches(&format!("run the installer again: {}\r\n", INSTALL_PS1.replace('|', "^|"))).count(), 2);
        let readmes = [
            include_str!(concat!(env!("KUMI_WORKSPACE"), "/README.md")),
            include_str!(concat!(env!("KUMI_WORKSPACE"), "/README.ja.md")),
            include_str!(concat!(env!("KUMI_WORKSPACE"), "/README.zh-CN.md")),
        ];
        for readme in readmes {
            assert!(readme.contains(&format!("\n{INSTALL_SH}\n")) && readme.contains(&format!("\n{INSTALL_PS1}\n")));
        }
        // No other address for them, on main or a release, but a release's own (for one before native Kumi).
        let address = regex::Regex::new(
            r"https://(?:raw\.githubusercontent\.com/user1303836/kumi/[^/\s]+|github\.com/user1303836/kumi/releases/[^\s]+?)/install\.(?:sh|ps1)",
        )
        .unwrap();
        for text in [sh, &ps1, launcher(false), launcher(true)].into_iter().chain(readmes) {
            for found in address.find_iter(text).map(|found| found.as_str()) {
                assert!(INSTALL_SH.contains(found) || INSTALL_PS1.contains(found) || found.contains("/v$"), "{found}");
            }
        }
    }

    #[test]
    fn what_windows_reads_is_ascii() {
        // Windows PowerShell reads a script file without a byte order mark, and a download without a charset (a
        // release asset), in another code page than UTF-8: install.ps1 builds its glyphs from [char]s. cmd reads
        // kumi.cmd in the console's code page.
        assert!(include_str!(concat!(env!("KUMI_WORKSPACE"), "/install.ps1")).is_ascii());
        assert!(launcher(true).is_ascii());
    }

    #[test]
    fn a_cmd_still_running_the_launcher_before_goes_on_as_it_would_have() {
        let (before, now) = (PREVIOUS_WINDOWS_LAUNCHER, launcher(true));
        // Each line the one before has starts a line here too, the same one but for the Node line, whose bytes
        // `goto other` and a comment take.
        let node = before.find("\"%KUMI_HOME%\\node").unwrap();
        let line = |text: &str, at: usize| text[at..].split("\r\n").next().unwrap().to_string();
        let starts = std::iter::once(0).chain(before.match_indices("\r\n").map(|(at, _)| at + 2)).filter(|at| *at < before.len());
        for at in starts {
            assert_eq!(line(now, at), if at == node { "goto other".into() } else { line(before, at) }, "at {at}");
        }
        assert_eq!(&now[..node], &before[..node]);
        assert_eq!(now[node..].find("\r\nexit /b"), before[node..].find("\r\nexit /b"));
        // Once Node, or the native Kumi, exits, it goes on to exit.
        for after in [before[node..].find("\r\n").unwrap() + node + 2, before.rfind("exit /b %errorlevel%").unwrap()] {
            assert_eq!(resumed_at(now, after), "exit /b %errorlevel%", "offset {after}");
        }
    }

    #[test]
    fn the_launcher_before_is_replaced_only_as_its_installers_wrote_it() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bin/kumi.cmd");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, PREVIOUS_WINDOWS_LAUNCHER).unwrap();
        write_launcher_for(dir.path().to_str().unwrap(), true).unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), launcher(true));
        // Written with LF, its offsets aren't this one's: it stays.
        let lf = PREVIOUS_WINDOWS_LAUNCHER.replace("\r\n", "\n");
        for contents in [lf.clone(), lf + "\r\n"] {
            fs::write(&path, &contents).unwrap();
            write_launcher_for(dir.path().to_str().unwrap(), true).unwrap();
            assert_eq!(fs::read_to_string(&path).unwrap(), contents);
        }
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
