//! Discover VST3 files without executing third-party code. Presence is not a promise that a host can load it.

use std::{
    collections::BTreeSet,
    fs,
    io::Read,
    path::{Path, PathBuf},
};

use serde::Serialize;
use serde_json::Value;
use sha2::{Digest, Sha256};

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

#[derive(Debug, Clone, Default, Serialize)]
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
        if kumi_common::path::network_or_device(&path.to_string_lossy()) {
            out.warnings.push("Skipped a network or device VST3 location".into());
            continue;
        }
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

fn same_version(a: &str, b: &str) -> bool {
    fn parts(s: &str) -> Vec<&str> {
        let mut parts: Vec<_> = s.split('.').collect();
        while parts.len() > 2 && parts.last() == Some(&"0") {
            parts.pop();
        }
        parts
    }
    parts(a) == parts(b)
}

fn agreed_version(module: Option<&str>, binary: Option<&str>) -> Option<String> {
    match (module, binary) {
        (Some(a), Some(b)) if !same_version(a, b) => None,
        (Some(a), _) => Some(a.to_string()),
        (_, Some(b)) => Some(b.to_string()),
        _ => None,
    }
}

fn binary_file(path: &Path) -> Option<PathBuf> {
    if path.is_file() {
        return Some(path.to_path_buf());
    }
    #[cfg(windows)]
    {
        let architecture = if cfg!(target_arch = "aarch64") { "arm64ec-win" } else { "x86_64-win" };
        return fs::read_dir(path.join("Contents").join(architecture)).ok().and_then(|entries| {
            entries.take(16).flatten().map(|entry| entry.path()).find(|p| p.extension().is_some_and(|ext| ext.eq_ignore_ascii_case("vst3")))
        });
    }
    #[cfg(not(windows))]
    None
}

fn metadata_bytes(path: &Path) -> Option<Vec<u8>> {
    if kumi_common::path::network_or_device(&path.to_string_lossy()) {
        return None;
    }
    let metadata = fs::metadata(path).ok()?;
    if !metadata.is_file() || metadata.len() > 256 * 1024 {
        return None;
    }
    let mut options = fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NONBLOCK | libc::O_NOFOLLOW);
    }
    let file = options.open(path).ok()?;
    if !file.metadata().ok()?.is_file() {
        return None;
    }
    let mut bytes = Vec::new();
    file.take(256 * 1024 + 1).read_to_end(&mut bytes).ok()?;
    (bytes.len() <= 256 * 1024).then_some(bytes)
}

fn read_metadata(path: &Path) -> Option<String> {
    String::from_utf8(metadata_bytes(path)?).ok()
}

/// Bound the JSON5 grammar before its parser allocates a tree. Quotes and comments may
/// contain brackets; only real container syntax contributes to depth and item counts.
fn module_info(text: &str) -> Option<Value> {
    if text.len() > 256 * 1024 {
        return None;
    }
    let bytes = text.as_bytes();
    let (mut at, mut depth, mut items) = (0, 0usize, 0usize);
    while at < bytes.len() {
        match bytes[at] {
            b'"' | b'\'' => {
                let quote = bytes[at];
                at += 1;
                while at < bytes.len() && bytes[at] != quote {
                    if bytes[at] == b'\\' {
                        at += 1;
                    }
                    at += 1;
                }
            }
            b'/' if bytes.get(at + 1) == Some(&b'/') => {
                at += 2;
                while at < bytes.len()
                    && !matches!(bytes[at], b'\n' | b'\r')
                    && !bytes.get(at..at + 3).is_some_and(|b| b == b"\xe2\x80\xa8" || b == b"\xe2\x80\xa9")
                {
                    at += 1;
                }
            }
            b'/' if bytes.get(at + 1) == Some(&b'*') => {
                at += 2;
                while at + 1 < bytes.len() && &bytes[at..at + 2] != b"*/" {
                    at += 1;
                }
                at += 1;
            }
            b'{' | b'[' => {
                depth += 1;
                items += 1;
                if depth > 32 {
                    return None;
                }
            }
            b'}' | b']' => {
                depth = depth.checked_sub(1)?;
            }
            b',' | b':' => {
                items += 1;
            }
            _ => {}
        }
        if items > 16_384 {
            return None;
        }
        at += 1;
    }
    json5::from_str(text).ok()
}

fn bundle_version(path: &Path) -> Option<String> {
    // An XML plist can be read on every platform, including hermetic fixture tests.
    // Ignore the standard external DTD declaration; the XML parser resolves no entities.
    if let Some(text) = read_metadata(&path.join("Contents/Info.plist")) {
        if let Some(start) = text.find("<plist") {
            if let Ok(document) = super::formats::xml::parse(&text[start..]) {
                if let Some(dict) = document.root.child("dict") {
                    for pair in dict.children.windows(2) {
                        if pair[0].name == "key" && pair[0].text == "CFBundleShortVersionString" && pair[1].name == "string" {
                            return Some(pair[1].text.clone());
                        }
                    }
                }
            }
        }
    }
    #[cfg(target_os = "macos")]
    if let Some(bytes) = metadata_bytes(&path.join("Contents/Info.plist")).filter(|b| b.starts_with(b"bplist00")) {
        if let Some(version) = binary_plist_version(&bytes) {
            return Some(version);
        }
    }
    #[cfg(windows)]
    {
        let binary = binary_file(path);
        if let Some(binary) = binary {
            return windows_version(&binary);
        }
    }
    None
}

#[cfg(target_os = "macos")]
fn binary_plist_version(bytes: &[u8]) -> Option<String> {
    use std::{
        ffi::{c_char, c_void, CStr},
        ptr,
    };
    type Object = *const c_void;
    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        fn CFDataCreate(allocator: Object, bytes: *const u8, length: isize) -> Object;
        fn CFPropertyListCreateWithData(allocator: Object, data: Object, options: usize, format: *mut isize, error: *mut Object) -> Object;
        fn CFGetTypeID(object: Object) -> usize;
        fn CFDictionaryGetTypeID() -> usize;
        fn CFStringGetTypeID() -> usize;
        fn CFDictionaryGetValue(dictionary: Object, key: Object) -> Object;
        fn CFStringCreateWithCString(allocator: Object, value: *const c_char, encoding: u32) -> Object;
        fn CFStringGetCString(string: Object, buffer: *mut c_char, size: isize, encoding: u32) -> u8;
        fn CFRelease(object: Object);
    }
    const UTF8: u32 = 0x08000100;
    // CoreFoundation parses the already bounded bytes; no bundle or plug-in is loaded.
    unsafe {
        let data = CFDataCreate(ptr::null(), bytes.as_ptr(), bytes.len() as isize);
        if data.is_null() {
            return None;
        }
        let plist = CFPropertyListCreateWithData(ptr::null(), data, 0, ptr::null_mut(), ptr::null_mut());
        CFRelease(data);
        if plist.is_null() {
            return None;
        }
        let key = CFStringCreateWithCString(ptr::null(), c"CFBundleShortVersionString".as_ptr(), UTF8);
        let mut result = None;
        if !key.is_null() && CFGetTypeID(plist) == CFDictionaryGetTypeID() {
            let value = CFDictionaryGetValue(plist, key);
            let mut buffer = [0i8; 256];
            if !value.is_null()
                && CFGetTypeID(value) == CFStringGetTypeID()
                && CFStringGetCString(value, buffer.as_mut_ptr(), buffer.len() as isize, UTF8) != 0
            {
                result = CStr::from_ptr(buffer.as_ptr()).to_str().ok().map(str::to_string);
            }
        }
        if !key.is_null() {
            CFRelease(key);
        }
        CFRelease(plist);
        result
    }
}

#[cfg(windows)]
fn windows_version(path: &Path) -> Option<String> {
    use std::{ffi::c_void, os::windows::ffi::OsStrExt};
    #[link(name = "version")]
    unsafe extern "system" {
        fn GetFileVersionInfoSizeW(name: *const u16, handle: *mut u32) -> u32;
        fn GetFileVersionInfoW(name: *const u16, handle: u32, length: u32, data: *mut c_void) -> i32;
        fn VerQueryValueW(data: *const c_void, block: *const u16, output: *mut *mut c_void, length: *mut u32) -> i32;
    }
    if kumi_common::path::network_or_device(&path.to_string_lossy()) || !fs::metadata(path).ok()?.is_file() {
        return None;
    }
    let name: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    if name[..name.len() - 1].contains(&0) {
        return None;
    }
    // These APIs inspect a version resource, without loading or executing the plug-in.
    unsafe {
        let size = GetFileVersionInfoSizeW(name.as_ptr(), std::ptr::null_mut());
        if size == 0 || size > 1024 * 1024 {
            return None;
        }
        let mut data = vec![0u32; (size as usize).div_ceil(4)];
        if GetFileVersionInfoW(name.as_ptr(), 0, size, data.as_mut_ptr().cast()) == 0 {
            return None;
        }
        // The fixed file version may carry a vendor build number (SD3's 44977)
        // while ProductVersion names the actual preset-compatible release.
        let mut translations = std::ptr::null_mut();
        let mut translated_bytes = 0;
        let translation: Vec<u16> = "\\VarFileInfo\\Translation".encode_utf16().chain(Some(0)).collect();
        if VerQueryValueW(data.as_ptr().cast(), translation.as_ptr(), &mut translations, &mut translated_bytes) != 0
            && !translations.is_null()
            && translated_bytes >= 4
            && translated_bytes <= 256
        {
            for pair in std::slice::from_raw_parts(translations.cast::<u16>(), translated_bytes as usize / 2).chunks_exact(2) {
                let query: Vec<u16> =
                    format!("\\StringFileInfo\\{:04x}{:04x}\\ProductVersion", pair[0], pair[1]).encode_utf16().chain(Some(0)).collect();
                let mut value = std::ptr::null_mut();
                let mut chars = 0;
                if VerQueryValueW(data.as_ptr().cast(), query.as_ptr(), &mut value, &mut chars) != 0
                    && !value.is_null()
                    && chars > 1
                    && chars <= 128
                {
                    let version = String::from_utf16_lossy(std::slice::from_raw_parts(value.cast::<u16>(), chars as usize));
                    let version = version.trim_matches('\0').trim();
                    if !version.is_empty()
                        && version.chars().all(|c| c.is_ascii_digit() || c == '.')
                        && version.chars().any(|c| matches!(c, '1'..='9'))
                    {
                        return Some(version.to_string());
                    }
                }
            }
        }
        let mut output = std::ptr::null_mut();
        let mut length = 0;
        if VerQueryValueW(data.as_ptr().cast(), [92u16, 0].as_ptr(), &mut output, &mut length) == 0 || output.is_null() || length < 52 {
            return None;
        }
        let words = std::slice::from_raw_parts(output.cast::<u32>(), 13);
        if words[0] != 0xfeef04bd {
            return None;
        }
        let (high, low) = (words[2], words[3]);
        if high == 0 && low == 0 {
            return None;
        }
        Some(format!("{}.{}.{}.{}", high >> 16, high & 65535, low >> 16, low & 65535))
    }
}

fn describe(path: &Path) -> Vec<InstalledPlugin> {
    let info: Option<Value> = ["Contents/Resources/moduleinfo.json", "Contents/moduleinfo.json"]
        .iter()
        .find_map(|name| read_metadata(&path.join(name)).and_then(|text| module_info(&text)));
    let version = info.as_ref().and_then(|v| v.get("Version")).and_then(Value::as_str);
    let binary_version = bundle_version(path);
    let classes: Vec<_> = info
        .as_ref()
        .and_then(|v| v.get("Classes"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|c| c.get("Category").and_then(Value::as_str) == Some("Audio Module Class"))
        .filter_map(|c| Some((c.get("Name")?.as_str()?, c.get("CID")?.as_str()?, c.get("Version").and_then(Value::as_str))))
        .collect();
    if !classes.is_empty() {
        return classes
            .into_iter()
            .map(|(name, id, class_version)| {
                let found = builtins().iter().find(|p| p.info.ids.vst3.iter().any(|known| known.id.eq_ignore_ascii_case(id)));
                described(
                    path,
                    name,
                    Some(id.to_uppercase()),
                    agreed_version(class_version.or(version), binary_version.as_deref()),
                    found,
                    "moduleinfo class ID",
                )
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
        agreed_version(version, binary_version.as_deref()),
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
    let version = version.filter(|v| !v.trim().is_empty()).or_else(|| format.and_then(|f| fingerprint_version(path, f)));
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
    // Windows resources commonly add a zero build component (3.0.5.0 for 3.0.5).
    // Normalize only trailing zero components, never a different nonzero build.
    let version = version.map(|v| writes.iter().find(|known| same_version(known, &v)).cloned().unwrap_or(v));
    let create_preset = version.as_ref().is_some_and(|v| writes.contains(v))
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

/// A small number of verified Windows products omit all version resources.
/// Only an exact captured binary may use its public factory's recorded version.
fn fingerprint_version(path: &Path, format: &PluginFormat) -> Option<String> {
    let proofs: Vec<_> = format.verified.writes.iter().filter_map(|v| Some((v.binary_sha256.as_deref()?, v.version.as_str()))).collect();
    if proofs.is_empty() || kumi_common::path::network_or_device(&path.to_string_lossy()) {
        return None;
    }
    let path = binary_file(path)?;
    let metadata = fs::metadata(&path).ok()?;
    if !metadata.is_file() || metadata.len() > 256 * 1024 * 1024 {
        return None;
    }
    let mut options = fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NONBLOCK | libc::O_NOFOLLOW);
    }
    let file = options.open(&path).ok()?;
    if !file.metadata().ok()?.is_file() {
        return None;
    }
    let mut file = file.take(256 * 1024 * 1024 + 1);
    let mut hash = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    let mut total = 0usize;
    loop {
        let n = file.read(&mut buffer).ok()?;
        if n == 0 {
            break;
        }
        total += n;
        if total > 256 * 1024 * 1024 {
            return None;
        }
        hash.update(&buffer[..n]);
    }
    let digest = hex::encode(hash.finalize());
    proofs.into_iter().find(|(known, _)| known.eq_ignore_ascii_case(&digest)).map(|(_, version)| version.to_string())
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

pub fn is_limiter_class_id(class_id: &str) -> bool {
    builtins()
        .iter()
        .flat_map(|p| p.info.ids.vst3.iter())
        .any(|id| id.id.eq_ignore_ascii_case(class_id.trim_matches(['{', '}'])) && is_limiter_name(&id.name))
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
    fn only_exact_binary_evidence_supplies_a_missing_installed_version() {
        let temp = tempfile::tempdir().unwrap();
        let binary = temp.path().join("test.vst3");
        fs::write(&binary, b"synthetic binary identity").unwrap();
        let mut format = builtins().iter().find(|f| f.info.plugin == "valhalla-delay").unwrap().clone();
        format.verified.writes[0].binary_sha256 = Some(hex::encode(Sha256::digest(b"synthetic binary identity")));
        assert_eq!(fingerprint_version(&binary, &format).as_deref(), Some("3.0.5"));
        fs::write(&binary, b"another build").unwrap();
        assert_eq!(fingerprint_version(&binary, &format), None);
    }
    #[test]
    fn json5_class_versions_plists_and_unknown_versions_are_distinct() {
        let temp = tempfile::tempdir().unwrap();
        let bundle = temp.path().join("ValhallaDelay.vst3");
        fs::create_dir_all(bundle.join("Contents/Resources")).unwrap();
        let metadata = bundle.join("Contents/Resources/moduleinfo.json");
        fs::write(
            &metadata,
            r#"{
            // The class version is authoritative for a multi-product module.
            Version: '99.0.0', Classes: [{Name: 'ValhallaDelay',
            CID: '565354644C617976616C68616C6C6164', Version: '3.0.5.0',
            Category: 'Audio Module Class',},],
        }"#,
        )
        .unwrap();
        let found = scan(&[temp.path().into()]);
        assert_eq!(found.plugins[0].version.as_deref(), Some("3.0.5"));
        assert!(found.plugins[0].create_preset);
        fs::remove_file(metadata).unwrap();
        let unknown = scan(&[temp.path().into()]);
        assert_eq!(unknown.plugins[0].installed_version_verified, None);
        assert!(!unknown.plugins[0].create_preset);
        fs::write(
            bundle.join("Contents/Info.plist"),
            r#"<?xml version="1.0"?>
            <!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
            <plist version="1.0"><dict><key>CFBundleShortVersionString</key><string>3.0.5</string></dict></plist>"#,
        )
        .unwrap();
        assert!(scan(&[temp.path().into()]).plugins[0].create_preset);
        assert!(module_info(&format!("{}{}", "[".repeat(33), "]".repeat(33))).is_none());
        assert!(module_info(&format!("[{}0]", "0,".repeat(16_385))).is_none());
        assert!(module_info(&format!("// comment\u{2028}{}{}", "[".repeat(200), "]".repeat(200))).is_none());
    }
    #[cfg(target_os = "macos")]
    #[test]
    fn binary_plists_use_the_short_product_version_without_loading_a_bundle() {
        // Python plistlib's binary encoding of {CFBundleShortVersionString: "3.0.5"}.
        let bytes = [
            98, 112, 108, 105, 115, 116, 48, 48, 209, 1, 2, 95, 16, 26, 67, 70, 66, 117, 110, 100, 108, 101, 83, 104, 111, 114, 116, 86,
            101, 114, 115, 105, 111, 110, 83, 116, 114, 105, 110, 103, 85, 51, 46, 48, 46, 53, 8, 11, 40, 0, 0, 0, 0, 0, 0, 1, 1, 0, 0, 0,
            0, 0, 0, 0, 3, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 46,
        ];
        assert_eq!(binary_plist_version(&bytes).as_deref(), Some("3.0.5"));
        assert!(binary_plist_version(b"bplist00invalid").is_none());
    }
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
