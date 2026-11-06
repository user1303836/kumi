//! Attach to LYNX Remote Control; opening an app requires an explicit project.
use crate::{native, unix_ms, Error, Result};
use daw_bridge::{detect, owner, wire::code, Daw};
use serde_json::{json, Value};
use std::{
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, Instant},
};

pub fn native_path() -> Option<PathBuf> {
    std::env::var_os("LYNX_CONTROL_DISCOVERY").map(PathBuf::from).or_else(|| {
        if let Some(profile) = std::env::var_os("LYNX_PROFILE_DIR") {
            return Some(PathBuf::from(profile).join("LYNX/control/connection.json"));
        }
        #[cfg(windows)]
        {
            return std::env::var_os("APPDATA").map(PathBuf::from).map(|folder| folder.join("LYNX/control/connection.json"));
        }
        #[cfg(not(windows))]
        std::env::var_os("HOME").map(PathBuf::from).map(|home| {
            if cfg!(target_os = "macos") {
                home.join("Library/Application Support/LYNX/control/connection.json")
            } else {
                std::env::var_os("XDG_CONFIG_HOME")
                    .map(PathBuf::from)
                    .filter(|folder| folder.is_absolute())
                    .unwrap_or_else(|| home.join(".config"))
                    .join("LYNX/control/connection.json")
            }
        })
    })
}

/// The host has a scrubbed environment. An attach keeps the native path privately
/// so its companion can find the same endpoint without forwarding launch variables.
pub fn serving_path(home: &Path) -> Result<Option<PathBuf>> {
    let record = home.join("bridges/lynx/launched.json");
    match std::fs::symlink_metadata(&record) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(native_path()),
        Err(_) => return Err(Error::bad("Cannot inspect the private LYNX attachment record")),
        Ok(_) => {}
    }
    let bytes = native::private_record(&record)?;
    let value: Value = serde_json::from_slice(&bytes).map_err(|_| Error::bad("Invalid LYNX attachment record"))?;
    if value["schema"] != "lynx-kumi-launch/1" {
        return Err(Error::bad("Incompatible LYNX attachment record; run kumi --daw lynx bridge"));
    }
    let path = value["nativeConfig"]
        .as_str()
        .filter(|path| !path.is_empty())
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .ok_or_else(|| Error::bad("The LYNX attachment record must name an absolute native discovery path"))?;
    Ok(Some(path))
}

pub async fn run(home: &Path, path: &Path, session: Option<&Path>, background: bool) -> Result<Value> {
    if !path.is_absolute() {
        return Err(Error::bad("Native discovery path must be absolute"));
    }
    let folder = home.join("bridges/lynx");
    private_directory(&folder)?;
    let _lock = launch_lock(&folder)?;
    let app = find_app();
    if let Ok(found) = native::discovery(path) {
        let inspected = native::Client::for_pid(path.into(), found.pid).call("session.inspect", json!({})).await?;
        let record = record(&folder, path, app.as_deref(), found.pid, &found.app_version, &inspected, false)?;
        return Ok(record);
    }
    let var = |name: &str| std::env::var(name).ok();
    if detect::list_processes(&var).iter().any(|(_, name)| detect::process_is(Daw::Lynx, name)) {
        return Err(Error::new(code::FAILED,"LYNX is already open without a compatible Remote Control endpoint. Enable LYNX Remote Control in General settings; if this build has no such setting, save and reopen a compatible build. No second copy was started"));
    }
    let session = session.ok_or_else(|| Error::new(
        code::FAILED,
        "Open LYNX and enable Remote Control in General settings, then run kumi --daw lynx bridge. Connecting does not open another app or enable control automatically",
    ))?;
    let app =
        app.ok_or_else(|| Error::bad("A compatible custom LYNX build was not found; set KUMI_LYNX_APP to its absolute application path"))?;
    if !app.is_absolute() {
        return Err(Error::bad("KUMI_LYNX_APP must be an absolute application path"));
    }
    let executable = executable(&app)?;
    if let Some(parent) = path.parent() {
        private_directory(parent)?;
    }
    if !session.is_absolute() || !session.is_file() {
        return Err(Error::bad("The session must be an existing absolute local file"));
    }
    let mut child = start_apart(&app, &executable, path, Some(session), background)?;
    let until = Instant::now() + Duration::from_secs(30);
    while Instant::now() < until {
        if let Some(child) = child.as_mut() {
            if child.try_wait().map_err(|_| Error::new(code::FAILED, "Cannot inspect the started LYNX process"))?.is_some() {
                return Err(Error::new(code::FAILED, "The selected LYNX application ended before its native endpoint became ready"));
            }
        }
        if let Ok(found) = native::discovery(path) {
            if !process_matches(found.pid, &executable) {
                return Err(Error::new(
                    code::FAILED,
                    "A different LYNX process published discovery during startup; no request was sent to it",
                ));
            }
            if let Ok(inspected) = native::Client::for_pid(path.into(), found.pid).call("session.inspect", json!({})).await {
                return record(&folder, path, Some(&app), found.pid, &found.app_version, &inspected, true);
            }
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    Err(Error::new(
        code::FAILED,
        "LYNX opened; enable Remote Control in its General settings, then connect again. The application was left open; no project was closed",
    ))
}

/// Refuse an existing shared directory instead of changing somebody's permissions.
pub fn private_directory(path: &Path) -> Result<()> {
    #[cfg(windows)]
    let private = |path: &Path| crate::windows::private_directory(path);
    #[cfg(not(windows))]
    let private = |path: &Path| owner::is_owner_only(path);
    match std::fs::symlink_metadata(path) {
        Ok(meta) if meta.is_dir() && !meta.file_type().is_symlink() && private(path) => Ok(()),
        Ok(_) => Err(Error::bad("The LYNX control/bridge directory must already be private and cannot be a link")),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            owner::create_owner_only_dir(path).map_err(|_| Error::bad("Cannot create the private LYNX control directory"))?;
            if private(path) {
                Ok(())
            } else {
                Err(Error::bad("The new LYNX directory failed its privacy check"))
            }
        }
        Err(_) => Err(Error::bad("Cannot inspect the LYNX control directory")),
    }
}

fn start_apart(
    app: &Path,
    executable: &Path,
    discovery: &Path,
    session: Option<&Path>,
    background: bool,
) -> Result<Option<std::process::Child>> {
    #[cfg(target_os = "macos")]
    if app.extension().is_some_and(|ext| ext == "app") {
        let mut command = Command::new("/usr/bin/open");
        command.arg("-n");
        if background {
            command.arg("-g");
        }
        command.arg("-a").arg(app).arg("--args");
        if let Some(session) = session {
            command.arg(session);
        }
        command
            .env_remove("LYNX_CONTROL_ENABLED")
            .env("LYNX_CONTROL_DISCOVERY", discovery)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let status = command.status().map_err(|_| Error::new(code::FAILED, "LaunchServices could not start LYNX"))?;
        if !status.success() {
            return Err(Error::new(code::FAILED, "LaunchServices refused the selected LYNX application"));
        }
        return Ok(None);
    }
    #[cfg(not(target_os = "macos"))]
    let _ = (app, background);
    let mut command = Command::new(executable);
    command
        .env_remove("LYNX_CONTROL_ENABLED")
        .env("LYNX_CONTROL_DISCOVERY", discovery)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    if let Some(session) = session {
        command.arg(session);
    }
    #[cfg(unix)]
    std::os::unix::process::CommandExt::process_group(&mut command, 0);
    #[cfg(windows)]
    let child = {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0000_0008 | 0x0000_0200 | 0x0100_0000);
        command.spawn().or_else(|_| command.creation_flags(0x0000_0008 | 0x0000_0200).spawn())
    };
    #[cfg(not(windows))]
    let child = command.spawn();
    child.map(Some).map_err(|_| Error::new(code::FAILED, "The selected LYNX application could not start"))
}

fn process_matches(pid: u32, expected: &Path) -> bool {
    #[cfg(target_os = "macos")]
    let actual = {
        #[link(name = "proc")]
        unsafe extern "C" {
            fn proc_pidpath(pid: libc::c_int, buffer: *mut std::ffi::c_void, size: u32) -> libc::c_int;
        }
        let mut bytes = vec![0u8; 4096];
        // SAFETY: the mutable buffer is valid for its full size; proc_pidpath writes a terminated path.
        let size = unsafe { proc_pidpath(pid as libc::c_int, bytes.as_mut_ptr().cast(), bytes.len() as u32) };
        if size <= 0 {
            None
        } else {
            bytes.truncate(bytes.iter().position(|b| *b == 0).unwrap_or(size as usize));
            String::from_utf8(bytes).ok().map(PathBuf::from)
        }
    };
    #[cfg(not(target_os = "macos"))]
    let actual = detect::program_of(pid);
    let Some((actual, expected)) =
        actual.and_then(|actual| Some((std::fs::canonicalize(actual).ok()?, std::fs::canonicalize(expected).ok()?)))
    else {
        return false;
    };
    #[cfg(windows)]
    {
        actual.to_string_lossy().eq_ignore_ascii_case(&expected.to_string_lossy())
    }
    #[cfg(not(windows))]
    {
        actual == expected
    }
}
fn record(folder: &Path, path: &Path, app: Option<&Path>, pid: u32, version: &str, inspected: &Value, started: bool) -> Result<Value> {
    let value = json!({"schema":"lynx-kumi-launch/1","pid":pid,"app":app,"version":version,"nativeConfig":path,
        "path":inspected.pointer("/project/path"),"projectId":inspected.pointer("/project/id"),"started":started,"startedMs":unix_ms()});
    let mut bytes = serde_json::to_vec_pretty(&value).map_err(|_| Error::bad("Cannot encode the launch record"))?;
    bytes.push(b'\n');
    owner::write_owner_only(&folder.join("launched.json"), &bytes)
        .map_err(|_| Error::new(code::FAILED, "Cannot keep the private LYNX launch record"))?;
    Ok(value)
}
fn find_app() -> Option<PathBuf> {
    let var = |name: &str| std::env::var(name).ok();
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default();
    detect::detect_lynx(&detect::Env { os: detect::Os::current(), home, var: &var, root: None }).map(|app| app.path)
}
fn executable(app: &Path) -> Result<PathBuf> {
    if app.is_file() {
        return Ok(app.into());
    }
    #[cfg(target_os = "macos")]
    {
        if app.extension().is_some_and(|e| e == "app") {
            let output = Command::new("/usr/bin/plutil")
                .args(["-extract", "CFBundleExecutable", "raw", "-o", "-"])
                .arg(app.join("Contents/Info.plist"))
                .output()
                .map_err(|_| Error::bad("Cannot inspect the selected LYNX application bundle"))?;
            if !output.status.success() {
                return Err(Error::bad("Cannot find the selected bundle's executable"));
            }
            let name = String::from_utf8(output.stdout).map_err(|_| Error::bad("Invalid app executable name"))?;
            let name = name.trim();
            if name.is_empty() || name.contains(['/', '\\']) || name == "." || name == ".." {
                return Err(Error::bad("Invalid app executable name"));
            }
            let executable = app.join("Contents/MacOS").join(name);
            if executable.is_file() {
                return Ok(executable);
            }
        }
    }
    Err(Error::bad("KUMI_LYNX_APP must name an executable or a valid macOS application bundle"))
}
fn launch_lock(folder: &Path) -> Result<std::fs::File> {
    let mut options = std::fs::OpenOptions::new();
    options.read(true).write(true).create(true).truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    }
    let file = options.open(folder.join("launch.lock")).map_err(|_| Error::bad("Cannot open the private launch lock"))?;
    file.try_lock().map_err(|_| Error::new(code::BUSY, "Another LYNX launch is being prepared; try again once it finishes"))?;
    Ok(file)
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::{MetadataExt, PermissionsExt};

    #[test]
    fn an_app_is_started_in_a_separate_process_group_from_kumis_terminal() {
        let dir = tempfile::tempdir().unwrap();
        let shell = std::fs::canonicalize("/bin/sh").unwrap();
        let mut child = start_apart(&shell, &shell, &dir.path().join("control.json"), None, true).unwrap().unwrap();
        // SAFETY: getpgid/getpgrp inspect existing process IDs and mutate no process.
        let (group, parent) = unsafe { (libc::getpgid(child.id() as libc::pid_t), libc::getpgrp()) };
        child.wait().unwrap();
        assert_eq!(group, child.id() as libc::pid_t);
        assert_ne!(group, parent);
    }

    #[test]
    fn an_existing_shared_control_directory_is_refused_without_changing_its_mode() {
        let dir = tempfile::tempdir().unwrap();
        let control = dir.path().join("control");
        std::fs::create_dir(&control).unwrap();
        std::fs::set_permissions(&control, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(private_directory(&control).is_err());
        assert_eq!(std::fs::metadata(control).unwrap().mode() & 0o777, 0o755);
    }
}
