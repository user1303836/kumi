//! Where Live loads Kumi's extension and keeps its endpoint and secret.
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    path::{Component, Path, PathBuf},
};
pub const KUMI_EXTENSION_ID: &str = "kumi.kumi";
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KumiExtensionFolders {
    pub code: PathBuf,
    pub data: PathBuf,
}
pub fn kumi_extension_folders(
    env: Option<&HashMap<String, String>>,
    platform: Option<&str>,
    home: Option<&Path>,
) -> Option<KumiExtensionFolders> {
    let process_env;
    let env = match env {
        Some(env) => env,
        None => {
            process_env = kumi_common::env::vars();
            &process_env
        }
    };
    let home_fallback;
    let home = match home {
        Some(home) => home,
        None => {
            home_fallback = home::home_dir().unwrap_or_default();
            &home_fallback
        }
    };
    let platform = platform.unwrap_or_else(|| crate::platform::current_platform());
    let extensions =
        env.get("ABLETON_MCP_LIVE_EXTENSIONS_DIR").filter(|v| !v.is_empty()).map(PathBuf::from).or_else(|| match platform {
            "darwin" => Some(home.join("Library/Application Support/Ableton/Extensions")),
            "win32" => Some(
                env.get("LOCALAPPDATA")
                    .filter(|v| !v.is_empty())
                    .map(PathBuf::from)
                    .unwrap_or_else(|| home.join("AppData/Local"))
                    .join("Ableton/Extensions"),
            ),
            _ => None,
        })?;
    Some(KumiExtensionFolders {
        code: normalize(&extensions.join(KUMI_EXTENSION_ID)),
        data: normalize(&extensions.parent().unwrap_or(Path::new(".")).join("Extensions Data").join(KUMI_EXTENSION_ID)),
    })
}
fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for part in path.components() {
        match part {
            Component::CurDir => {}
            Component::ParentDir => {
                if out.file_name().is_some_and(|name| name != "..") {
                    out.pop();
                } else if !out.has_root() {
                    out.push("..");
                }
            }
            part => out.push(part.as_os_str()),
        }
    }
    if out.as_os_str().is_empty() {
        out.push(".");
    }
    out
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn folders_match_live_platform_conventions_and_override() {
        let mut env = HashMap::new();
        let folders = kumi_extension_folders(Some(&env), Some("darwin"), Some(Path::new("/Users/p"))).unwrap();
        assert_eq!(folders.code, PathBuf::from("/Users/p/Library/Application Support/Ableton/Extensions/kumi.kumi"));
        assert_eq!(folders.data, PathBuf::from("/Users/p/Library/Application Support/Ableton/Extensions Data/kumi.kumi"));
        assert!(kumi_extension_folders(Some(&env), Some("linux"), Some(Path::new("/home/p"))).is_none());
        env.insert("ABLETON_MCP_LIVE_EXTENSIONS_DIR".into(), "/x/Extensions".into());
        let folders = kumi_extension_folders(Some(&env), Some("linux"), None).unwrap();
        assert_eq!(folders.code, PathBuf::from("/x/Extensions/kumi.kumi"));
        assert_eq!(folders.data, PathBuf::from("/x/Extensions Data/kumi.kumi"));
        env.clear();
        env.insert("LOCALAPPDATA".into(), "C:/Users/p/AppData/Local".into());
        env.insert("APPDATA".into(), "C:/Users/p/AppData/Roaming".into());
        let folders = kumi_extension_folders(Some(&env), Some("win32"), Some(Path::new("C:/Users/p"))).unwrap();
        assert_eq!(folders.data, PathBuf::from("C:/Users/p/AppData/Local/Ableton/Extensions Data/kumi.kumi"));
    }
}
