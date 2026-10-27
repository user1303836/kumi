//! Discover VST3 files without executing third-party code. Presence is not a promise that a host can load it.

use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
};

use serde::Serialize;
use serde_json::Value;

use super::{
    formats::structure::{builtins, PluginFormat},
    templates,
};

#[derive(Debug, Clone, Serialize)]
pub struct InstalledPlugin {
    pub name: String,
    pub path: PathBuf,
    pub plugin: Option<String>,
    pub vendor: Option<String>,
    pub class_id: Option<String>,
    pub version: Option<String>,
    pub roles: Vec<String>,
    pub identity: String,
    pub create_preset: bool,
    pub verified_write_versions: Vec<String>,
    pub installed_version_verified: Option<bool>,
}

#[derive(Debug, Default, Serialize)]
pub struct Inventory {
    pub plugins: Vec<InstalledPlugin>,
    pub warnings: Vec<String>,
}

/// Standard 64-bit VST3 locations. Callers may also supply the host's custom roots to `scan`.
pub fn standard_roots() -> Vec<PathBuf> {
    let mut roots = Vec::new();
    if cfg!(windows) {
        if let Some(common) = std::env::var_os("CommonProgramFiles") {
            roots.push(PathBuf::from(common).join("VST3"));
        }
        if let Some(local) = std::env::var_os("LOCALAPPDATA") {
            roots.push(PathBuf::from(local).join("Programs/Common/VST3"));
        }
    } else if cfg!(target_os = "macos") {
        roots.push(PathBuf::from("/Library/Audio/Plug-Ins/VST3"));
        if let Some(home) = std::env::var_os("HOME") {
            roots.push(PathBuf::from(home).join("Library/Audio/Plug-Ins/VST3"));
        }
    } else {
        roots.extend([PathBuf::from("/usr/lib/vst3"), PathBuf::from("/usr/local/lib/vst3")]);
        if let Some(home) = std::env::var_os("HOME") {
            roots.push(PathBuf::from(home).join(".vst3"));
        }
    }
    roots
}

pub fn scan(roots: &[PathBuf]) -> Inventory {
    let mut out = Inventory::default();
    let mut pending: Vec<_> = roots.iter().cloned().map(|p| (p, 0)).collect();
    let mut visited = BTreeSet::new();
    let mut count = 0usize;
    while let Some((path, depth)) = pending.pop() {
        count += 1;
        if count > 20_000 {
            out.warnings.push("VST3 discovery stopped after 20,000 filesystem entries".into());
            break;
        }
        let canonical = match path.canonicalize() {
            Ok(p) => p,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => {
                out.warnings.push(format!("{}: {e}", path.display()));
                continue;
            }
        };
        if !visited.insert(canonical) {
            continue;
        }
        if path.extension().is_some_and(|e| e.eq_ignore_ascii_case("vst3")) {
            out.plugins.extend(describe(&path));
            continue; // A bundle's architecture-specific binary is the same installation.
        }
        if !path.is_dir() {
            continue;
        }
        if depth >= 8 {
            out.warnings.push(format!("VST3 discovery depth limit: {}", path.display()));
            continue;
        }
        match fs::read_dir(&path) {
            Ok(entries) => {
                for entry in entries {
                    if count + pending.len() >= 20_000 {
                        out.warnings.push(format!("VST3 discovery entry limit at {}", path.display()));
                        break;
                    }
                    match entry {
                        Ok(entry) => pending.push((entry.path(), depth + 1)),
                        Err(e) => out.warnings.push(format!("{}: {e}", path.display())),
                    }
                }
            }
            Err(e) => out.warnings.push(format!("{}: {e}", path.display())),
        }
    }
    out.plugins.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()).then(a.path.cmp(&b.path)));
    if out.warnings.len() > 50 {
        out.warnings.truncate(50);
        out.warnings.push("Further discovery warnings omitted".into());
    }
    out
}

fn key(s: &str) -> String {
    s.chars().filter(|c| c.is_alphanumeric()).flat_map(char::to_lowercase).collect()
}

fn describe(path: &Path) -> Vec<InstalledPlugin> {
    let metadata_path = path.join("Contents/Resources/moduleinfo.json");
    let info: Option<Value> = fs::metadata(&metadata_path)
        .ok()
        .filter(|m| m.len() <= 1024 * 1024)
        .and_then(|_| fs::read(&metadata_path).ok())
        .and_then(|bytes| serde_json::from_slice(&bytes).ok());
    let version = info.as_ref().and_then(|v| v.get("Version")).and_then(Value::as_str).map(str::to_string);
    let classes: Vec<_> = info
        .as_ref()
        .and_then(|v| v.get("Classes"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|c| c.get("Category").and_then(Value::as_str) == Some("Audio Module Class"))
        .filter_map(|c| Some((c.get("Name")?.as_str()?, c.get("CID")?.as_str()?)))
        .collect();
    if !classes.is_empty() {
        return classes
            .into_iter()
            .map(|(name, id)| {
                let found = builtins().iter().find(|p| p.info.ids.vst3.iter().any(|known| known.id.eq_ignore_ascii_case(id)));
                described(path, name, Some(id.to_uppercase()), version.clone(), found, "moduleinfo class ID")
            })
            .collect();
    }
    let filename = path.file_name().and_then(|s| s.to_str()).unwrap_or_default();
    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or(filename);
    let found = builtins().iter().find(|p| {
        p.info.binaries.iter().any(|s| s.eq_ignore_ascii_case(filename))
            || key(&p.info.name) == key(stem)
            || p.info.ids.vst3.iter().any(|id| key(&id.name) == key(stem))
    });
    let class = found.and_then(|p| {
        p.info.ids.vst3.iter().find(|id| key(&id.name) == key(stem)).or_else(|| (p.info.ids.vst3.len() == 1).then(|| &p.info.ids.vst3[0]))
    });
    vec![described(
        path,
        class.map(|id| id.name.as_str()).unwrap_or(stem),
        class.map(|id| id.id.clone()),
        version,
        found,
        "filename; confirm in the DAW",
    )]
}

fn described(
    path: &Path,
    name: &str,
    class_id: Option<String>,
    version: Option<String>,
    format: Option<&PluginFormat>,
    identity: &str,
) -> InstalledPlugin {
    let writes: Vec<_> = format
        .map(|p| {
            p.verified
                .writes
                .iter()
                .filter(|v| v.files.iter().any(|f| matches!(f.as_str(), "state" | "processor" | "vstpreset")))
                .map(|v| v.version.clone())
                .collect()
        })
        .unwrap_or_default();
    let mut roles = format.map(|p| p.info.roles.clone()).unwrap_or_default();
    if format.is_some_and(|p| p.info.kind == "instrument") && !roles.iter().any(|r| r == "instrument") {
        roles.push("instrument".into());
    }
    // Individual modules must not inherit the suite's capabilities.
    if format.is_some_and(|p| p.info.plugin == "ozone-12") && name != "Ozone 12" {
        roles = ozone_roles(name).iter().map(|s| (*s).to_string()).collect();
    }
    let create_preset = version.as_ref().is_none_or(|v| writes.contains(v))
        && format.is_some_and(|p| {
            !writes.is_empty() && (templates::initial(&p.info.plugin).is_some() || (p.info.plugin == "ozone-12" && class_id.is_some()))
        });
    InstalledPlugin {
        name: name.into(),
        path: path.to_path_buf(),
        plugin: format.map(|p| p.info.plugin.clone()),
        vendor: format.map(|p| p.info.vendor.clone()),
        class_id,
        installed_version_verified: version.as_ref().map(|v| writes.contains(v)),
        version,
        roles,
        identity: identity.into(),
        create_preset,
        verified_write_versions: writes,
    }
}

fn ozone_roles(name: &str) -> &'static [&'static str] {
    if name.contains("Maximizer") || name.contains("Limiter") {
        &["mastering", "limiting", "loudness"]
    } else if name.contains("Imager") {
        &["mastering", "stereo"]
    } else if name.contains("Equalizer") || name.contains("EQ") {
        &["mastering", "eq"]
    } else if name.contains("Dynamics") || name.contains("Compressor") {
        &["mastering", "compression"]
    } else if name.contains("Exciter") || name.contains("Tape") {
        &["mastering", "saturation"]
    } else {
        &["mastering"]
    }
}

/// A known limiter already in a host chain. Module names are checked separately from the suite.
pub fn is_limiter_name(name: &str) -> bool {
    let name_key = key(name);
    builtins().iter().any(|p| {
        let exact = p.info.ids.vst3.iter().find(|id| key(&id.name) == name_key);
        if exact.is_none() && key(&p.info.name) != name_key {
            return false;
        }
        if p.info.plugin == "ozone-12" {
            // A suite can have only EQ, or a bypassed Maximizer; its name proves no limiting.
            return name_key != "ozone12" && ozone_roles(exact.map(|id| id.name.as_str()).unwrap_or(name)).contains(&"limiting");
        }
        // A compressor that can also limit may have that stage disabled. Only dedicated
        // loudness/limiter products qualify without reading their internal signal path.
        matches!(p.info.plugin.as_str(), "softube-weiss-mm-1-mastering-maximizer" | "zynaptiq-loud-1")
    }) || matches!(name_key.as_str(), "fabfilterprol2" | "prol2")
}

/// A query is a capability, not a demand to add every matching plug-in.
pub fn matches_role(plugin: &InstalledPlugin, role: &str) -> bool {
    let role = normalize_role(role);
    plugin.roles.iter().any(|r| normalize_role(r) == role)
}

fn normalize_role(s: &str) -> String {
    match s.to_ascii_lowercase().as_str() {
        "louder" | "loudness" | "limiter" | "maximizer" | "limiting" => "limiting",
        "wider" | "width" | "stereo" | "stereo-width" | "imaging" => "stereo",
        "compressor" | "compression" => "compression",
        "equalizer" | "equalization" | "eq" => "eq",
        "bbd" | "bucket brigade" | "bucket-brigade-delay" => "bucket-brigade-delay",
        _ => return s.to_ascii_lowercase(),
    }
    .into()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn scans_bundles_once_and_uses_exact_class_identity() {
        let temp = tempfile::tempdir().unwrap();
        let bundle = temp.path().join("Renamed.vst3");
        fs::create_dir_all(bundle.join("Contents/Resources")).unwrap();
        fs::write(bundle.join("Contents/Resources/moduleinfo.json"), serde_json::to_vec(&serde_json::json!({
            "Version":"3.0.5", "Classes":[{"Name":"ValhallaDelay", "CID":"565354644C617976616C68616C6C6164", "Category":"Audio Module Class"}]
        })).unwrap()).unwrap();
        fs::create_dir_all(bundle.join("Contents/x86_64-win")).unwrap();
        fs::write(bundle.join("Contents/x86_64-win/Renamed.vst3"), b"not executed").unwrap();
        fs::write(temp.path().join("ValhallaRoom.dll"), b"VST2 ignored").unwrap();
        let inventory = scan(&[temp.path().into(), temp.path().into()]);
        assert_eq!(inventory.plugins.len(), 1);
        let p = &inventory.plugins[0];
        assert_eq!(p.plugin.as_deref(), Some("valhalla-delay"));
        assert_eq!(p.installed_version_verified, Some(true));
        assert!(matches_role(p, "bbd"));
        let mut metadata: Value = serde_json::from_slice(&fs::read(bundle.join("Contents/Resources/moduleinfo.json")).unwrap()).unwrap();
        metadata["Version"] = serde_json::json!("99.0.0");
        fs::write(bundle.join("Contents/Resources/moduleinfo.json"), serde_json::to_vec(&metadata).unwrap()).unwrap();
        let newer = scan(&[temp.path().into()]);
        assert_eq!(newer.plugins[0].installed_version_verified, Some(false));
        assert!(!newer.plugins[0].create_preset);
    }
    #[test]
    fn distinguishes_ozone_modules_and_does_not_trust_names_over_class_ids() {
        let temp = tempfile::tempdir().unwrap();
        fs::write(temp.path().join("Ozone 12 Imager.vst3"), []).unwrap();
        let bundle = temp.path().join("ValhallaDelay.vst3");
        fs::create_dir_all(bundle.join("Contents/Resources")).unwrap();
        fs::write(
            bundle.join("Contents/Resources/moduleinfo.json"),
            br#"{"Classes":[{"Name":"ValhallaDelay","CID":"00000000000000000000000000000000","Category":"Audio Module Class"}]}"#,
        )
        .unwrap();
        let inventory = scan(&[temp.path().into()]);
        let imager = inventory.plugins.iter().find(|p| p.name.contains("Imager")).unwrap();
        assert!(matches_role(imager, "wider"));
        assert!(!matches_role(imager, "louder"));
        let other = inventory.plugins.iter().find(|p| p.name == "ValhallaDelay").unwrap();
        assert!(other.plugin.is_none());
        assert!(!other.create_preset);
        assert!(is_limiter_name("Ozone 12 Maximizer"));
        assert!(!is_limiter_name("Ozone 12"));
        assert!(!is_limiter_name("Ozone 12 Imager"));
        assert!(!is_limiter_name("Weiss DS1-MK3"));
        assert!(!is_limiter_name("P11 Abyss"));
        assert!(is_limiter_name("Weiss MM-1 Mastering Maximizer"));
    }
}
