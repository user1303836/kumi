//! Renoise library files Kumi takes for the tool (M4 n-devices): `device-preset-data` and `phrase-script` with a `path`
//! get the file's text here (an .xrdp's preset data, a .lua phrase script), and the loaders (`device-chain-load`,
//! `phrase-load`, `modset-load`) get their file checked here and its path sent on as `file`. Only from Renoise's own
//! library or the producer's User Library, once every link is resolved; only files of the kind's type and size, and not of
//! a newer Renoise than the one whose library is here (a file Renoise can't load shows a dialog that holds Renoise).

use crate::renoise::presets::{Refusal, MAX_PRESET_BYTES};
use serde_json::Value;
use std::collections::BTreeMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// A file kind Kumi takes from Renoise's libraries, by the change kind that takes it: its extension, its document's root
/// element (none for a script), the library folder Renoise keeps them in, what it is in words, its most bytes, and for a
/// kind whose file's text the tool gets, the input fields of the text and of the name the file gives it.
pub struct FileKind {
    pub kind: &'static str,
    pub extension: &'static str,
    pub root: Option<&'static str>,
    pub folder: &'static str,
    pub noun: &'static str,
    pub most: usize,
    pub text: Option<(&'static str, &'static str)>,
}

/// The most bytes of a phrase script (as the tool's `MAX_SCRIPT_BYTES`).
pub const MAX_SCRIPT_BYTES: usize = 65536;

pub const FILE_KINDS: [FileKind; 5] = [
    FileKind {
        kind: "device-preset-data",
        extension: "xrdp",
        root: Some("FilterDevicePreset"),
        folder: "Effect Presets",
        noun: "device preset",
        most: MAX_PRESET_BYTES,
        text: Some(("presetData", "presetName")),
    },
    FileKind {
        kind: "phrase-script",
        extension: "lua",
        root: None,
        folder: "Phrase Scripts",
        noun: "phrase script",
        most: MAX_SCRIPT_BYTES,
        text: Some(("code", "codeName")),
    },
    FileKind {
        kind: "device-chain-load",
        extension: "xrnt",
        root: Some("RenoiseDeviceChain"),
        folder: "Effect Chains",
        noun: "device chain",
        most: MAX_PRESET_BYTES,
        text: None,
    },
    FileKind {
        kind: "phrase-load",
        extension: "xrnz",
        root: Some("InstrumentPhrase"),
        folder: "Phrases",
        noun: "phrase",
        most: MAX_PRESET_BYTES,
        text: None,
    },
    FileKind {
        kind: "modset-load",
        extension: "xrno",
        root: Some("SampleModulationSet"),
        folder: "Modulation Sets",
        noun: "modulation set",
        most: MAX_PRESET_BYTES,
        text: None,
    },
];

/// The kind that reads a preset file's text.
pub const KIND: &str = "device-preset-data";

/// Renoise's library folders: the ones Renoise copies out of the app for each of its versions (`own`: the version's
/// preferences folder, or on macOS its Application Support folder), and the producer's User Library.
#[derive(Debug, Default)]
pub struct Libraries {
    pub own: Vec<PathBuf>,
    pub user: Vec<PathBuf>,
}

impl Libraries {
    fn all(&self) -> impl Iterator<Item = &PathBuf> {
        self.own.iter().chain(self.user.iter())
    }
}

pub fn library_roots() -> Libraries {
    let Some(home) =
        std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).filter(|home| !home.is_empty()).map(PathBuf::from)
    else {
        return Libraries::default();
    };
    let var = |name: &str| std::env::var(name).ok();
    let env = daw_bridge::detect::Env { os: daw_bridge::detect::Os::current(), home: home.clone(), var: &var, root: None };
    let mut own = vec![];
    for prefs in daw_bridge::detect::detect_renoise(&env).prefs {
        own.push(prefs.dir.join("Library"));
        if let Some(version) = prefs.dir.file_name() {
            own.push(home.join("Library").join("Application Support").join("Renoise").join(version).join("Library"));
        }
    }
    Libraries { own, user: vec![home.join("Documents").join("Renoise").join("User Library")] }
}

/// A document's root element and its `doc_version` (after the XML declaration and comments).
fn document_root(text: &str) -> Option<(&str, Option<u32>)> {
    let mut rest = text.trim_start_matches('\u{feff}');
    loop {
        rest = rest.trim_start();
        if let Some(after) = rest.strip_prefix("<?") {
            rest = &after[after.find("?>")? + 2..];
        } else if let Some(after) = rest.strip_prefix("<!--") {
            rest = &after[after.find("-->")? + 3..];
        } else {
            break;
        }
    }
    let tag = rest.strip_prefix('<')?;
    let tag = &tag[..tag.find('>').unwrap_or(tag.len())];
    let name = &tag[..tag.find(|c: char| c.is_whitespace() || c == '/').unwrap_or(tag.len())];
    let version = tag.split_once("doc_version=\"").and_then(|(_, after)| after.split('"').next()).and_then(|n| n.parse().ok());
    (!name.is_empty()).then_some((name, version))
}

/// The newest version found in Renoise's own library folders, by those folders and the kind: read once a run (another
/// Renoise version's library is other folders).
type Newest = BTreeMap<(Vec<PathBuf>, &'static str), Option<u32>>;
static NEWEST: Mutex<Newest> = Mutex::new(BTreeMap::new());

/// The newest document version among Renoise's own library files of this kind (Renoise wrote them: it loads up to that).
fn newest_version(own: &[PathBuf], spec: &FileKind) -> Option<u32> {
    let key = (own.to_vec(), spec.kind);
    if let Some(newest) = NEWEST.lock().unwrap_or_else(|e| e.into_inner()).get(&key) {
        return *newest;
    }
    let newest = scan_newest(own, spec);
    NEWEST.lock().unwrap_or_else(|e| e.into_inner()).insert(key, newest);
    newest
}

/// Up to 2,000 of the library's files of the kind, each read to its root element.
fn scan_newest(own: &[PathBuf], spec: &FileKind) -> Option<u32> {
    let mut newest = None;
    let mut seen = 0;
    for root in own {
        let mut folders = vec![(root.join(spec.folder), 0)];
        while let Some((folder, depth)) = folders.pop() {
            let Ok(entries) = std::fs::read_dir(&folder) else { continue };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() && depth < 6 {
                    folders.push((path, depth + 1));
                } else if path.extension().is_some_and(|extension| extension.eq_ignore_ascii_case(spec.extension)) && seen < 2000 {
                    seen += 1;
                    let mut head = [0u8; 512];
                    let read = std::fs::File::open(&path).and_then(|mut file| file.read(&mut head)).unwrap_or(0);
                    let text = String::from_utf8_lossy(&head[..read]);
                    if let Some((name, Some(version))) = document_root(&text) {
                        if Some(name) == spec.root {
                            newest = newest.max(Some(version));
                        }
                    }
                }
            }
        }
    }
    newest
}

/// A library file, checked: where it is once every link is resolved, its text, and the name it gives what it holds (the
/// file's name without its extension).
pub struct Checked {
    pub real: PathBuf,
    pub text: String,
    pub name: String,
}

pub fn check_file(path: &str, spec: &FileKind, libraries: &Libraries) -> Result<Checked, Refusal> {
    let asked = Path::new(path);
    if !asked.is_absolute() {
        return Err(Refusal::Bad(format!("path: the whole path of a .{} file in Renoise's library", spec.extension)));
    }
    if !asked.extension().is_some_and(|extension| extension.eq_ignore_ascii_case(spec.extension)) {
        return Err(Refusal::Bad(format!("{path} isn't a {} (.{}) file", spec.noun, spec.extension)));
    }
    let real = std::fs::canonicalize(asked).map_err(|_| Refusal::Bad(format!("there's no {} file at {path}", spec.noun)))?;
    let inside = libraries.all().filter_map(|root| std::fs::canonicalize(root).ok()).any(|root| real.starts_with(&root));
    if !inside {
        return Err(Refusal::Bad(format!(
            "{path} isn't in Renoise's library or the User Library (Kumi takes library files only from there)"
        )));
    }
    let size = std::fs::metadata(&real).map(|meta| meta.len()).unwrap_or(u64::MAX);
    if !real.is_file() || size > spec.most as u64 {
        let most = if spec.most >= 1_048_576 { format!("{} MB", spec.most / 1_048_576) } else { format!("{} KB", spec.most / 1024) };
        return Err(Refusal::Bad(format!("{path} isn't a {} file of at most {most}", spec.noun)));
    }
    let text = std::fs::read_to_string(&real).map_err(|_| Refusal::Bad(format!("{path} isn't a text file Kumi can read")))?;
    let mut version = None;
    if let Some(root) = spec.root {
        let Some((_, found)) = document_root(&text).filter(|(found, _)| *found == root) else {
            return Err(Refusal::Bad(format!("{path} isn't a {} (no <{root}> document)", spec.noun)));
        };
        version = found;
    }
    if spec.kind == KIND && !text.contains("<DeviceSlot") {
        return Err(Refusal::Bad(format!("{path} isn't a device preset (no <DeviceSlot> in it)")));
    }
    // (a loader's file is loaded by Renoise itself; a preset's version is checked by the tool against the device's own data)
    if spec.text.is_none() {
        if let (Some(version), Some(newest)) = (version, newest_version(&libraries.own, spec)) {
            if version > newest {
                return Err(Refusal::Bad(format!(
                    "{path} was saved by a newer Renoise (document version {version}; this one's library has {newest}): Renoise would show an error"
                )));
            }
        }
    }
    let name = real.file_stem().map(|stem| stem.to_string_lossy().into_owned()).unwrap_or_default();
    Ok(Checked { real, text, name })
}

/// The input the tool gets for a change that takes a library file (Renoise only): a file's text with the name it gives it
/// (`presetData`/`presetName`, `code`/`codeName`), or a loader's file checked (`file`, `fileName`). None when the change
/// isn't one (a preset's data or a script's code given in full).
pub fn with_file(daw: daw_bridge::Daw, kind: &str, input: &Value, libraries: &Libraries) -> Option<Result<Value, Refusal>> {
    if daw != daw_bridge::Daw::Renoise {
        return None;
    }
    let spec = FILE_KINDS.iter().find(|spec| spec.kind == kind)?;
    let path = input.get("path");
    if spec.text.is_some() && path.is_none() {
        return None;
    }
    Some((|| {
        let named = ["file", "fileName"].into_iter().chain(spec.text.map(|(_, name)| name));
        if named.into_iter().any(|field| input.get(field).is_some()) {
            return Err(Refusal::Bad("give path: Kumi's host checks the file and names it to Renoise".into()));
        }
        let Some(path) = path.and_then(Value::as_str) else {
            return Err(Refusal::Bad(format!("path: the whole path of a .{} file in Renoise's library", spec.extension)));
        };
        if let Some((data, _)) = spec.text.filter(|(data, _)| input.get(*data).is_some()) {
            return Err(Refusal::Bad(format!("give {data} or path, not both")));
        }
        let checked = check_file(path, spec, libraries)?;
        let mut input = input.clone();
        if let Some(fields) = input.as_object_mut() {
            fields.remove("path");
            match spec.text {
                Some((data, name)) => {
                    fields.insert(data.into(), checked.text.into());
                    fields.insert(name.into(), checked.name.into());
                }
                None => {
                    fields.insert("file".into(), checked.real.to_string_lossy().into_owned().into());
                    fields.insert("fileName".into(), checked.name.into());
                }
            }
        }
        Ok(input)
    })())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const PRESET: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<FilterDevicePreset doc_version=\"14\">\n  <DeviceSlot type=\"CompressorDevice\">\n    <IsMaximized>true</IsMaximized>\n  </DeviceSlot>\n</FilterDevicePreset>\n";
    const CHAIN: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<RenoiseDeviceChain doc_version=\"21\">\n  <Devices>\n  </Devices>\n</RenoiseDeviceChain>\n";

    /// A User Library with a Compressor preset, and Renoise's own library with a chain of document version 21.
    fn libraries() -> (tempfile::TempDir, Libraries, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let presets = dir.path().join("User Library").join("Effect Presets").join("Compressor");
        std::fs::create_dir_all(&presets).unwrap();
        std::fs::write(presets.join("Drums Hard.xrdp"), PRESET).unwrap();
        let chains = dir.path().join("Own").join("Effect Chains").join("Pad");
        std::fs::create_dir_all(&chains).unwrap();
        std::fs::write(chains.join("Bright Vox.xrnt"), CHAIN).unwrap();
        let libraries = Libraries { own: vec![dir.path().join("Own")], user: vec![dir.path().join("User Library")] };
        (dir, libraries, presets)
    }

    fn preset(path: &Path, libraries: &Libraries) -> Result<Checked, Refusal> {
        check_file(&path.to_string_lossy(), &FILE_KINDS[0], libraries)
    }

    #[test]
    fn a_preset_file_in_the_library_becomes_the_preset_data_the_tool_writes() {
        let (_dir, libraries, presets) = libraries();
        let input = json!({"ref": "e1:device:1.2", "path": presets.join("Drums Hard.xrdp").to_string_lossy()});
        let made = with_file(daw_bridge::Daw::Renoise, KIND, &input, &libraries).unwrap().unwrap();
        assert_eq!(made["presetData"], PRESET);
        assert_eq!(made["presetName"], "Drums Hard");
        assert!(made.get("path").is_none());
        // Not a kind that takes a file, not Renoise, a preset given in full: nothing to read.
        assert!(with_file(daw_bridge::Daw::Renoise, "device-preset", &input, &libraries).is_none());
        assert!(with_file(daw_bridge::Daw::Reaper, KIND, &input, &libraries).is_none());
        assert!(with_file(daw_bridge::Daw::Renoise, KIND, &json!({"ref": "e1:device:1.2", "presetData": PRESET}), &libraries).is_none());
    }

    #[test]
    fn a_loaders_file_is_checked_and_named_to_the_tool_by_its_resolved_path() {
        let (dir, libraries, _) = libraries();
        let chain = dir.path().join("Own").join("Effect Chains").join("Pad").join("Bright Vox.xrnt");
        let input = json!({"trackRef": "e1:track:1", "path": chain.to_string_lossy(), "replace": true});
        let made = with_file(daw_bridge::Daw::Renoise, "device-chain-load", &input, &libraries).unwrap().unwrap();
        assert_eq!(made["file"], std::fs::canonicalize(&chain).unwrap().to_string_lossy().as_ref());
        assert_eq!(made["fileName"], "Bright Vox");
        assert_eq!(made["replace"], true);
        assert!(made.get("path").is_none());
        // A loader always takes its file from Kumi's host: a file named by the caller, or none, is refused.
        let named = json!({"trackRef": "e1:track:1", "file": "/etc/passwd"});
        assert!(
            matches!(with_file(daw_bridge::Daw::Renoise, "device-chain-load", &named, &libraries), Some(Err(Refusal::Bad(m))) if m.contains("give path"))
        );
        let none = json!({"trackRef": "e1:track:1"});
        assert!(
            matches!(with_file(daw_bridge::Daw::Renoise, "phrase-load", &none, &libraries), Some(Err(Refusal::Bad(m))) if m.contains(".xrnz"))
        );
        // A chain of a newer Renoise than the library's own: refused (Renoise would show an error).
        let newer = dir.path().join("User Library").join("Newer.xrnt");
        std::fs::write(&newer, CHAIN.replace("doc_version=\"21\"", "doc_version=\"22\"")).unwrap();
        let refused = check_file(&newer.to_string_lossy(), &FILE_KINDS[2], &libraries);
        assert!(matches!(refused, Err(Refusal::Bad(m)) if m.contains("newer Renoise")));
        // (review H11) Renoise's own library is read once a run: gone from here on, its newest version still holds.
        std::fs::remove_dir_all(dir.path().join("Own")).unwrap();
        let refused = check_file(&newer.to_string_lossy(), &FILE_KINDS[2], &libraries);
        assert!(matches!(refused, Err(Refusal::Bad(m)) if m.contains("this one's library has 21")));
        // A file of another kind under the loader's extension.
        let wrong = dir.path().join("User Library").join("Wrong.xrnz");
        std::fs::write(&wrong, CHAIN).unwrap();
        assert!(
            matches!(check_file(&wrong.to_string_lossy(), &FILE_KINDS[3], &libraries), Err(Refusal::Bad(m)) if m.contains("isn't a phrase"))
        );
    }

    #[test]
    fn a_library_file_is_refused_outside_the_libraries_through_a_link_too_or_when_it_is_not_one() {
        let (dir, libraries, presets) = libraries();
        let refused = |path: &Path| match preset(path, &libraries) {
            Err(Refusal::Bad(message)) => message,
            Ok(_) => panic!("{path:?} not refused"),
            Err(other) => panic!("{path:?}: {other:?}"),
        };
        let outside = dir.path().join("Elsewhere.xrdp");
        std::fs::write(&outside, PRESET).unwrap();
        assert!(refused(&outside).contains("isn't in Renoise's library"));
        #[cfg(unix)]
        {
            // A file link out of the library, and a folder link out of it with the file under it.
            let link = presets.join("Escape.xrdp");
            std::os::unix::fs::symlink(&outside, &link).unwrap();
            assert!(refused(&link).contains("isn't in Renoise's library"));
            let away = dir.path().join("away");
            std::fs::create_dir_all(&away).unwrap();
            std::fs::write(away.join("Far.xrdp"), PRESET).unwrap();
            std::os::unix::fs::symlink(&away, presets.join("Away")).unwrap();
            assert!(refused(&presets.join("Away").join("Far.xrdp")).contains("isn't in Renoise's library"));
        }
        std::fs::write(presets.join("Notes.txt"), "hello").unwrap();
        assert!(refused(&presets.join("Notes.txt")).contains(".xrdp"));
        assert!(
            matches!(check_file("Effect Presets/Compressor/Drums Hard.xrdp", &FILE_KINDS[0], &libraries), Err(Refusal::Bad(m)) if m.contains("whole path"))
        );
        assert!(refused(&presets.join("Missing.xrdp")).contains("no device preset file"));
        std::fs::write(presets.join("Chain.xrdp"), CHAIN).unwrap();
        assert!(refused(&presets.join("Chain.xrdp")).contains("isn't a device preset"));
        let big = presets.join("Big.xrdp");
        std::fs::write(&big, format!("{PRESET}{}", " ".repeat(MAX_PRESET_BYTES))).unwrap();
        assert!(refused(&big).contains("at most"));
        let both = json!({"ref": "e1:device:1.2", "path": presets.join("Drums Hard.xrdp").to_string_lossy(), "presetData": PRESET});
        assert!(
            matches!(with_file(daw_bridge::Daw::Renoise, KIND, &both, &libraries), Some(Err(Refusal::Bad(m))) if m.contains("not both"))
        );
    }

    #[test]
    fn a_phrase_script_file_becomes_the_code_the_tool_writes() {
        let (dir, libraries, _) = libraries();
        let scripts = dir.path().join("Own").join("Phrase Scripts");
        std::fs::create_dir_all(&scripts).unwrap();
        let script = "return cycle(\"c4 e4 g4\")\n";
        std::fs::write(scripts.join("Arp.lua"), script).unwrap();
        let input = json!({"ref": "e1:phrase:1.1", "path": scripts.join("Arp.lua").to_string_lossy(), "commit": false});
        let made = with_file(daw_bridge::Daw::Renoise, "phrase-script", &input, &libraries).unwrap().unwrap();
        assert_eq!(made["code"], script);
        assert_eq!(made["codeName"], "Arp");
        assert_eq!(made["commit"], false);
        assert!(made.get("path").is_none());
        // Code given in full: nothing to read; both, or a name of the caller's: refused; too big: refused.
        assert!(
            with_file(daw_bridge::Daw::Renoise, "phrase-script", &json!({"ref": "e1:phrase:1.1", "code": script}), &libraries).is_none()
        );
        let both = json!({"ref": "e1:phrase:1.1", "path": scripts.join("Arp.lua").to_string_lossy(), "code": script});
        assert!(
            matches!(with_file(daw_bridge::Daw::Renoise, "phrase-script", &both, &libraries), Some(Err(Refusal::Bad(m))) if m.contains("code or path"))
        );
        let named = json!({"ref": "e1:phrase:1.1", "path": scripts.join("Arp.lua").to_string_lossy(), "codeName": "x"});
        assert!(
            matches!(with_file(daw_bridge::Daw::Renoise, "phrase-script", &named, &libraries), Some(Err(Refusal::Bad(m))) if m.contains("give path"))
        );
        std::fs::write(scripts.join("Big.lua"), "-".repeat(MAX_SCRIPT_BYTES + 1)).unwrap();
        let big = check_file(&scripts.join("Big.lua").to_string_lossy(), &FILE_KINDS[1], &libraries);
        assert!(matches!(big, Err(Refusal::Bad(m)) if m.contains("64 KB")));
    }

    #[test]
    fn a_documents_root_and_version_are_read_past_its_declaration_and_comments() {
        assert_eq!(document_root(CHAIN), Some(("RenoiseDeviceChain", Some(21))));
        assert_eq!(document_root("<!-- a --><InstrumentPhrase doc_version=\"1\">"), Some(("InstrumentPhrase", Some(1))));
        assert_eq!(document_root("<SampleModulationSet>"), Some(("SampleModulationSet", None)));
        assert_eq!(document_root("no xml"), None);
    }

    #[test]
    fn the_libraries_are_the_user_library_and_renoises_own() {
        let libraries = library_roots();
        assert!(libraries.user.iter().any(|root| root.ends_with(Path::new("Documents/Renoise/User Library"))), "{libraries:?}");
    }
}
