//! Attach to a compatible running app, or start the explicitly selected custom build.
use crate::{native, unix_ms, Error, Result};
use daw_bridge::{detect, owner, wire::code, Daw};
use serde_json::{json, Value};
use std::{
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, Instant},
};

pub fn native_path() -> Option<PathBuf> {
    std::env::var_os("KUMI_LYNX_CONFIG").or_else(|| std::env::var_os("LYNX_KUMI_DISCOVERY")).map(PathBuf::from).or_else(|| {
        std::env::var_os("HOME").map(PathBuf::from).map(|home| {
            if cfg!(target_os = "macos") {
                home.join("Library/Application Support/LYNX/kumi/connection.json")
            } else {
                std::env::var_os("XDG_CONFIG_HOME")
                    .map(PathBuf::from)
                    .unwrap_or_else(|| home.join(".config"))
                    .join("LYNX/kumi/connection.json")
            }
        })
    })
}

pub async fn run(home: &Path, path: &Path, session: Option<&Path>) -> Result<Value> {
    if !path.is_absolute() {
        return Err(Error::bad("Native discovery path must be absolute"));
    }
    let folder = home.join("bridges/lynx");
    owner::create_owner_only_dir(&folder).map_err(|_| Error::bad("Cannot create the private LYNX bridge folder"))?;
    let _lock = launch_lock(&folder)?;
    let app = find_app();
    if let Ok(found) = native::discovery(path) {
        let inspected = native::Client::for_pid(path.into(), found.pid).call("session.inspect", json!({})).await?;
        let record = record(&folder, path, app.as_deref(), found.pid, &found.app_version, &inspected, false)?;
        return Ok(record);
    }
    let var = |name: &str| std::env::var(name).ok();
    if detect::list_processes(&var).iter().any(|(_, name)| detect::process_is(Daw::Lynx, name)) {
        return Err(Error::new(code::FAILED,"LYNX is already open without a compatible authenticated endpoint. Save the project and reopen the custom native-control build; no second copy was started"));
    }
    let app =
        app.ok_or_else(|| Error::bad("A compatible custom LYNX build was not found; set KUMI_LYNX_APP to its absolute application path"))?;
    if !app.is_absolute() {
        return Err(Error::bad("KUMI_LYNX_APP must be an absolute application path"));
    }
    let executable = executable(&app)?;
    if let Some(parent) = path.parent() {
        owner::create_owner_only_dir(parent).map_err(|_| Error::bad("Cannot prepare LYNX's private native discovery directory"))?;
    }
    let mut command = Command::new(executable);
    command.env("LYNX_KUMI_DISCOVERY", path).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    if let Some(session) = session {
        if !session.is_absolute() || !session.is_file() {
            return Err(Error::bad("The session must be an existing absolute local file"));
        }
        command.arg(session);
    }
    let mut child = command.spawn().map_err(|_| Error::new(code::FAILED, "The selected LYNX application could not start"))?;
    let until = Instant::now() + Duration::from_secs(30);
    while Instant::now() < until {
        if child.try_wait().map_err(|_| Error::new(code::FAILED, "Cannot inspect the started LYNX process"))?.is_some() {
            return Err(Error::new(code::FAILED, "The selected LYNX application ended before its native endpoint became ready"));
        }
        if let Ok(found) = native::discovery(path) {
            if found.pid != child.id() {
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
        "LYNX opened but its compatible native endpoint did not become ready. The application was left open; no project was closed",
    ))
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
