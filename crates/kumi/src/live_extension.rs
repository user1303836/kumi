use kumi_common::js::json::file_text;
use kumi_runtime::{
    library::sources::{dirname, join},
    system::Env,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{fs, path::Path, time::Duration};
use tokio::io::AsyncReadExt;
pub const KUMI_EXTENSION_ID: &str = "kumi.kumi";
pub fn live_extensions_dir(env: &Env, platform: &str) -> Option<String> {
    let value = |key| env.get(key).filter(|s| !s.is_empty());
    if let Some(folder) = value("KUMI_LIVE_EXTENSIONS_DIR") {
        return Some(folder.clone());
    }
    match platform {
        "darwin" => value("HOME").map(|home| join(home, "Library/Application Support/Ableton/Extensions")),
        "win32" => value("LOCALAPPDATA").map(|home| join(home, "Ableton/Extensions")),
        _ => None,
    }
}
pub fn former_extensions_dir(env: &Env, platform: &str) -> Option<String> {
    if platform == "win32" && !env.get("KUMI_LIVE_EXTENSIONS_DIR").is_some_and(|s| !s.is_empty()) {
        env.get("APPDATA").filter(|s| !s.is_empty()).map(|root| join(root, "Ableton/Extensions"))
    } else {
        None
    }
}
pub fn remove_former_extension(env: &Env, platform: &str) -> std::io::Result<bool> {
    let Some(folder) = former_extensions_dir(env, platform).filter(|folder| Path::new(&join(folder, KUMI_EXTENSION_ID)).exists()) else {
        return Ok(false);
    };
    remove_extension(&folder)?;
    for left in [&folder, &join(&dirname(&folder), "Extensions Data")] {
        if fs::read_dir(left).is_ok_and(|mut items| items.next().is_none()) {
            let _ = remove(left);
        }
    }
    Ok(true)
}
pub fn extension_data_dir(extensions_dir: &str) -> String {
    join(&dirname(extensions_dir), &format!("Extensions Data/{KUMI_EXTENSION_ID}"))
}
pub fn extension_source(bridge_root: &str) -> Option<String> {
    [join(bridge_root, "live-extension"), join(bridge_root, "../live-extension")]
        .into_iter()
        .find(|folder| Path::new(&join(folder, "manifest.json")).exists() && Path::new(&join(folder, "dist/extension.js")).exists())
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExtensionCopy {
    pub path: String,
    pub version: String,
    pub digest: String,
}
pub fn read_extension(folder: &str) -> Option<ExtensionCopy> {
    let manifest: Value = serde_json::from_slice(&fs::read(join(folder, "manifest.json")).ok()?).ok()?;
    let code = fs::read(join(folder, "dist/extension.js")).ok()?;
    Some(ExtensionCopy {
        path: folder.into(),
        version: manifest.get("version").and_then(Value::as_str).unwrap_or("?").into(),
        digest: hex::encode(Sha256::digest(code)),
    })
}
pub fn installed_extension(extensions_dir: &str) -> Option<ExtensionCopy> {
    read_extension(&join(extensions_dir, KUMI_EXTENSION_ID))
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstalledExtension {
    pub path: String,
    pub version: String,
    pub changed: bool,
    pub replaced: bool,
}
/// Assemble beside Live's extension folder, then replace the old copy, restoring it after a failed swap.
pub fn install_extension(source: &str, extensions_dir: &str) -> std::io::Result<InstalledExtension> {
    let target = join(extensions_dir, KUMI_EXTENSION_ID);
    let wanted = read_extension(source).ok_or_else(|| std::io::Error::other("Kumi's copy of its Live extension is incomplete"))?;
    let current = read_extension(&target);
    if current.as_ref().is_some_and(|c| c.digest == wanted.digest && c.version == wanted.version) {
        return Ok(InstalledExtension { path: target, version: wanted.version, changed: false, replaced: false });
    }
    let parent = dirname(extensions_dir);
    if !Path::new(&parent).exists() {
        return Err(std::io::Error::other(format!("Live's folder isn't there ({parent}); open Live once")));
    }
    fs::create_dir_all(extensions_dir)?;
    let staging = join(&parent, &format!(".kumi-extension-{}", std::process::id()));
    let retired = format!("{staging}-old");
    remove(&staging)?;
    remove(&retired)?;
    let result = (|| -> std::io::Result<()> {
        fs::create_dir_all(join(&staging, "dist"))?;
        for file in ["manifest.json", "dist/extension.js"] {
            fs::copy(join(source, file), join(&staging, file))?;
        }
        let package = join(source, "package.json");
        if Path::new(&package).exists() {
            fs::copy(package, join(&staging, "package.json"))?;
        } else {
            fs::write(
                join(&staging, "package.json"),
                file_text(&json!({"name":"kumi","version":wanted.version,"private":true,"main":"dist/extension.js"})),
            )?;
        }
        if Path::new(&target).exists() {
            fs::rename(&target, &retired)?;
        }
        fs::rename(&staging, &target)?;
        Ok(())
    })();
    if let Err(error) = result {
        if !Path::new(&target).exists() && Path::new(&retired).exists() {
            fs::rename(&retired, &target)?;
        }
        remove(&staging)?;
        return Err(error);
    }
    remove(&retired)?;
    Ok(InstalledExtension { path: target, version: wanted.version, changed: true, replaced: current.is_some() })
}
pub fn remove_extension(extensions_dir: &str) -> std::io::Result<bool> {
    let code = join(extensions_dir, KUMI_EXTENSION_ID);
    let data = extension_data_dir(extensions_dir);
    let had = Path::new(&code).exists() || Path::new(&data).exists();
    remove(&code)?;
    remove(&data)?;
    Ok(had)
}
fn remove(path: &str) -> std::io::Result<()> {
    match fs::symlink_metadata(path) {
        Ok(m) if m.is_dir() => fs::remove_dir_all(path),
        Ok(_) => fs::remove_file(path),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RunningExtension {
    pub folder: String,
    pub port: f64,
    pub pid: f64,
}
pub fn running_extension(folder: &str) -> Option<RunningExtension> {
    let endpoint: Value = serde_json::from_slice(&fs::read(join(folder, "endpoint.json")).ok()?).ok()?;
    if endpoint.get("host")?.as_str()? != "127.0.0.1" {
        return None;
    }
    let port = endpoint.get("port")?.as_f64()?;
    let pid = endpoint.get("pid")?.as_f64()?;
    if port.fract() != 0. || pid.fract() != 0. || !process_exists(pid) {
        return None;
    }
    Some(RunningExtension { folder: folder.into(), port, pid })
}
fn process_exists(pid: f64) -> bool {
    // Node's process.kill accepts a signed 32-bit pid; zero/negative retain their process-group meaning.
    if !(i32::MIN as f64..=i32::MAX as f64).contains(&pid) {
        return false;
    }
    #[cfg(unix)]
    {
        unsafe { libc::kill(pid as i32, 0) == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM) }
    }
    #[cfg(windows)]
    {
        #[link(name = "kernel32")]
        extern "system" {
            fn OpenProcess(access: u32, inherit: i32, pid: u32) -> *mut std::ffi::c_void;
            fn GetExitCodeProcess(handle: *mut std::ffi::c_void, code: *mut u32) -> i32;
            fn CloseHandle(handle: *mut std::ffi::c_void) -> i32;
            fn GetLastError() -> u32;
        }
        unsafe {
            let h = OpenProcess(0x1000, 0, pid as u32);
            if h.is_null() {
                return GetLastError() == 5;
            }
            let mut code = 0;
            let ok = GetExitCodeProcess(h, &mut code) != 0;
            CloseHandle(h);
            ok && code == 259
        }
    }
    #[cfg(not(any(unix, windows)))]
    {
        pid == std::process::id() as f64
    }
}
/// Read the first signed-hello-shaped line within the source deadline.
pub async fn extension_answers(port: f64, ms: u64) -> bool {
    if !(0.0..=65535.0).contains(&port) || port.fract() != 0. {
        return false;
    }
    tokio::time::timeout(Duration::from_millis(ms), async {
        let Ok(mut socket) = tokio::net::TcpStream::connect(("127.0.0.1", port as u16)).await else { return false };
        let mut text = String::new();
        let mut buffer = [0; 4096];
        loop {
            match socket.read(&mut buffer).await {
                Ok(0) => {
                    std::future::pending::<()>().await;
                    return false;
                }
                Ok(n) => {
                    text.push_str(&String::from_utf8_lossy(&buffer[..n]));
                    if let Some(end) = text.find('\n') {
                        return serde_json::from_str::<Value>(&text[..end])
                            .is_ok_and(|v| v.get("id") == Some(&json!("hello")) && v.get("ok") == Some(&json!(true)));
                    }
                }
                Err(_) => return false,
            }
        }
    })
    .await
    .unwrap_or(false)
}
