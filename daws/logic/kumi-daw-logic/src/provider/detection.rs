//! Logic app detection from the original M2 adapter, kept in Logic's own crate. Read bounded app metadata; do not
//! launch an app or invoke plutil while finding a provider. Both XML and App Store binary plists are supported.
use daw_bridge::detect::{Arch, Version};
use kumi_types::system::Env;
use std::path::{Path, PathBuf};

pub const LOGIC_BUNDLES: [&str; 2] = ["com.apple.logic10", "com.apple.mobilelogic"];
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogicInstall {
    pub app: PathBuf,
    pub bundle: String,
    pub version: Option<Version>,
    pub archs: Vec<Arch>,
}

pub fn detect_logic(env: &Env) -> Vec<LogicInstall> {
    if let Some(app) = env.get("KUMI_LOGIC_APP").filter(|app| !app.is_empty()) {
        return install(Path::new(app)).into_iter().collect();
    }
    if !cfg!(target_os = "macos") {
        return Vec::new();
    }
    let home = env.get("HOME").or_else(|| env.get("USERPROFILE")).map(PathBuf::from).unwrap_or_default();
    let mut apps = vec![PathBuf::from("/Applications/Logic Pro.app")];
    for directory in [PathBuf::from("/Applications"), home.join("Applications")] {
        let mut found: Vec<PathBuf> = std::fs::read_dir(directory)
            .into_iter()
            .flatten()
            .flatten()
            .map(|entry| entry.path())
            .filter(|app| app.extension().is_some_and(|extension| extension == "app"))
            .collect();
        found.sort();
        apps.extend(found);
    }
    let mut found = Vec::new();
    for app in apps {
        if found.iter().any(|found: &LogicInstall| found.app == app) {
            continue;
        }
        if let Some(install) = install(&app) {
            found.push(install);
        }
    }
    found
}

fn install(app: &Path) -> Option<LogicInstall> {
    if !app.is_dir() {
        return None;
    }
    let bundle = plist_value(app, "CFBundleIdentifier")?;
    if !LOGIC_BUNDLES.contains(&bundle.as_str()) {
        return None;
    }
    let version = plist_value(app, "CFBundleShortVersionString").and_then(|version| Version::parse(&version));
    let executable = plist_value(app, "CFBundleExecutable").unwrap_or_else(|| "Logic Pro".into());
    let executable = Path::new(&executable);
    if executable.components().count() != 1 || executable.file_name().is_none() {
        return None;
    }
    let archs = daw_bridge::detect::binary_archs(&app.join("Contents/MacOS").join(executable));
    Some(LogicInstall { app: app.into(), bundle, version, archs })
}

fn plist_value(app: &Path, key: &str) -> Option<String> {
    use std::io::Read;
    let mut bytes = Vec::new();
    std::fs::File::open(app.join("Contents/Info.plist")).ok()?.take(1024 * 1024 + 1).read_to_end(&mut bytes).ok()?;
    if bytes.len() > 1024 * 1024 {
        return None;
    }
    if bytes.starts_with(b"bplist") {
        return bplist_string(&bytes, key);
    }
    let text = String::from_utf8(bytes).ok()?;
    let at = text.find(&format!("<key>{key}</key>"))?;
    let rest = &text[at..];
    let start = rest.find("<string>")? + "<string>".len();
    let end = rest[start..].find("</string>")?;
    Some(rest[start..start + end].trim().to_owned())
}

/// The string under `key` in a binary plist's top dictionary (`bplist00`). Only what detection needs: the trailer, the
/// offset table, one dictionary, and ASCII or UTF-16 strings; anything else answers None.
fn bplist_string(bytes: &[u8], key: &str) -> Option<String> {
    if bytes.len() < 40 || !bytes.starts_with(b"bplist0") {
        return None;
    }
    let trailer = &bytes[bytes.len() - 32..];
    let offset_size = trailer[6] as usize;
    let ref_size = trailer[7] as usize;
    let be64 = |at: usize| u64::from_be_bytes(trailer[at..at + 8].try_into().expect("8 bytes"));
    let count = usize::try_from(be64(8)).ok()?;
    let top = usize::try_from(be64(16)).ok()?;
    let table = usize::try_from(be64(24)).ok()?;
    if !(1..=8).contains(&offset_size) || !(1..=8).contains(&ref_size) || count > 1 << 20 || top >= count {
        return None;
    }
    if table.checked_add(count.checked_mul(offset_size)?)? > bytes.len() - 32 {
        return None;
    }
    let uint = |at: usize, size: usize| -> Option<usize> {
        if size > 8 {
            return None;
        }
        let value = bytes.get(at..at.checked_add(size)?)?.iter().fold(0u64, |n, byte| (n << 8) | *byte as u64);
        usize::try_from(value).ok()
    };
    let offset = |index: usize| -> Option<usize> {
        (index < count).then(|| uint(table.checked_add(index.checked_mul(offset_size)?)?, offset_size)).flatten()
    };
    // An object's type (its marker's high nibble), count (the low one, or the int object after it when that is 0xF), and
    // where its contents start.
    let header = |at: usize| -> Option<(u8, usize, usize)> {
        let marker = *bytes.get(at)?;
        let (kind, small) = (marker >> 4, (marker & 0x0F) as usize);
        if small == 0x0F && matches!(kind, 0x4 | 0x5 | 0x6 | 0xA | 0xC | 0xD) {
            let int_marker = *bytes.get(at.checked_add(1)?)?;
            if int_marker >> 4 != 0x1 {
                return None;
            }
            if int_marker & 0x0F > 3 {
                return None;
            }
            let size = 1usize << (int_marker & 0x0F);
            return Some((kind, uint(at.checked_add(2)?, size)?, at.checked_add(2)?.checked_add(size)?));
        }
        Some((kind, small, at.checked_add(1)?))
    };
    let string_at = |index: usize| -> Option<String> {
        let (kind, n, start) = header(offset(index)?)?;
        match kind {
            0x5 => String::from_utf8(bytes.get(start..start.checked_add(n)?)?.to_vec()).ok(),
            0x6 => {
                let units: Vec<u16> = bytes
                    .get(start..start.checked_add(n.checked_mul(2)?)?)?
                    .chunks(2)
                    .map(|pair| u16::from_be_bytes([pair[0], pair[1]]))
                    .collect();
                String::from_utf16(&units).ok()
            }
            _ => None,
        }
    };
    let (kind, n, start) = header(offset(top)?)?;
    if kind != 0xD || start.checked_add(n.checked_mul(2)?.checked_mul(ref_size)?)? > table {
        return None;
    }
    (0..n).find_map(|i| {
        let key_ref = uint(start.checked_add(i.checked_mul(ref_size)?)?, ref_size)?;
        (string_at(key_ref).as_deref() == Some(key))
            .then(|| string_at(uint(start.checked_add(n.checked_add(i)?.checked_mul(ref_size)?)?, ref_size)?))
            .flatten()
    })
}
