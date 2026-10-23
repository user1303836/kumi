//! Where Live keeps what Kumi's bridge needs: its preferences (a folder for each version it's been opened as), the
//! producer's User Library and the Remote Scripts folder in it, the bridge's configuration that the Remote Script's
//! reference names, and the server that configuration starts.
use kumi_library::sources::{dirname, is_absolute, join};
use kumi_types::{errors::RuntimeError, system::Env};
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{fs, path::Path, sync::LazyLock};

fn home() -> String {
    home::home_dir().unwrap_or_default().to_string_lossy().into()
}

/// Where Live keeps its settings, a folder for each version it's been opened as ("Live 12.4.15").
pub fn live_preferences_dir(env: &Env) -> String {
    let home = env.get("HOME").cloned().unwrap_or_else(home);
    if cfg!(windows) {
        join(&env.get("APPDATA").cloned().unwrap_or_else(|| join(&home, "AppData/Roaming")), "Ableton")
    } else {
        join(&home, "Library/Preferences/Ableton")
    }
}
pub fn live_user_library(env: &Env) -> Option<String> {
    let mut configs = Vec::new();
    for entry in std::fs::read_dir(live_preferences_dir(env)).ok()? {
        let entry = entry.ok()?;
        if !entry.file_name().to_string_lossy().starts_with("Live ") {
            continue;
        }
        let file = entry.path().join(if cfg!(windows) { "Preferences/Library.cfg" } else { "Library.cfg" });
        if file.exists() {
            let mtime = std::fs::metadata(&file).ok()?.modified().ok()?;
            configs.push((file, mtime));
        }
    }
    configs.sort_by(|a, b| b.1.cmp(&a.1));
    static USER: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?s)<UserLibrary>(.*?)</UserLibrary>").unwrap());
    for (file, _) in configs {
        let text = std::fs::read_to_string(file).ok()?;
        let block = USER.captures(&text).and_then(|c| c.get(1)).map(|m| m.as_str()).unwrap_or("");
        let value = |field: &str| {
            Regex::new(&format!(r#"<{field} Value="([^"]*)""#))
                .unwrap()
                .captures(block)
                .map(|c| c[1].replace("&quot;", "\"").replace("&lt;", "<").replace("&gt;", ">").replace("&amp;", "&"))
        };
        let name = value("ProjectName").filter(|s| !s.is_empty()).unwrap_or("User Library".into());
        if let Some(folder) = value("ProjectPath").filter(|s| !s.is_empty() && is_absolute(s)) {
            return Some(join(&folder, &name));
        }
    }
    None
}
pub fn remote_scripts_dir(env: &Env) -> String {
    if let Some(folder) = env.get("KUMI_REMOTE_SCRIPTS_DIR").filter(|s| !s.is_empty()) {
        return folder.clone();
    }
    if let Some(library) = live_user_library(env) {
        return join(&library, "Remote Scripts");
    }
    let home = home();
    let documents = if cfg!(windows) {
        [join(&home, "OneDrive/Documents"), join(&home, "Documents")]
            .into_iter()
            .find(|p| Path::new(&join(p, "Ableton")).exists())
            .unwrap_or_else(|| join(&home, "Documents"))
    } else {
        join(&home, "Music")
    };
    join(&documents, "Ableton/User Library/Remote Scripts")
}
pub fn find_bridge_config(env: &Env) -> Option<String> {
    let file = join(&remote_scripts_dir(env), "AbletonMcpBridge/bridge-reference.json");
    let value: Value = serde_json::from_str(&String::from_utf8_lossy(&std::fs::read(file).ok()?)).ok()?;
    let config = value["config"].as_str()?;
    (is_absolute(config) && Path::new(config).is_file()).then(|| config.into())
}
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct BridgeServer {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub entry: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
}
impl BridgeServer {
    pub fn native(&self) -> bool {
        self.command.as_ref().is_some_and(|c| Path::new(c).file_stem().is_some_and(|s| s == "ableton-mcp-server"))
    }
    pub fn package_root(&self) -> Option<String> {
        if self.native() {
            self.command.as_ref().map(|c| dirname(c))
        } else {
            self.entry.as_ref().map(|entry| dirname(&dirname(&dirname(entry))))
        }
    }
}
/// Read legacy Node entry metadata or the native server's sibling package metadata.
pub fn read_bridge_server(config_path: &str) -> Result<BridgeServer, RuntimeError> {
    let config: Value = serde_json::from_slice(&fs::read(config_path).map_err(|e| RuntimeError::plain(e.to_string()))?)
        .map_err(|e| RuntimeError::plain(e.to_string()))?;
    let nonempty = |v: &Value| v.as_str().filter(|s| !s.is_empty()).map(str::to_string);
    let mut server = BridgeServer {
        command: nonempty(&config["server"]["command"]),
        entry: config["server"]["args"].as_array().and_then(|a| a.first()).and_then(nonempty),
        version: None,
    };
    if server.native() {
        server.entry = server.command.clone();
    }
    if let Some(root) = server.package_root() {
        server.version =
            fs::read(join(&root, "package.json")).ok().and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok()).and_then(|v| {
                let version =
                    if server.native() { v.get("bridge").filter(|v| v.is_string()).or_else(|| v.get("version")) } else { v.get("version") };
                version.and_then(Value::as_str).filter(|s| !s.is_empty()).map(str::to_string)
            });
    }
    Ok(server)
}
