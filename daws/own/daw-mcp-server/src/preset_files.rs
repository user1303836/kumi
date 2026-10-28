//! Preset files are read on the host's blocking workers, never on a DAW's main thread. The bridge receives a bounded
//! snapshot of checked bytes; changing or unmounting the producer's source afterwards cannot change the apply.
use base64::Engine;
use daw_bridge::Daw;
use serde_json::Value;
use std::{fs, io::Read, path::Path};

pub fn has_suffix(name: &str) -> bool {
    name.to_ascii_lowercase().ends_with(".vstpreset")
}

/// A named program must not smuggle a filesystem path into the native preset loader. Windows also
/// interprets drive-relative names and strips trailing dots/spaces before opening a filename.
pub fn file_program(name: &str) -> bool {
    let name = name.trim();
    name.starts_with('/')
        || ["./", "../"].iter().any(|prefix| name.starts_with(prefix))
        || (cfg!(windows)
            && (name.starts_with('\\')
                || (name.as_bytes().get(1) == Some(&b':') && name.as_bytes()[0].is_ascii_alphabetic())
                || [".\\", "..\\"].iter().any(|prefix| name.starts_with(prefix))))
        || has_suffix(name.trim_end_matches([' ', '.']))
}

pub fn with_file(daw: Daw, kind: &str, input: &Value) -> Option<Result<Value, String>> {
    if daw != Daw::Reaper || kind != "fx-preset" {
        return None;
    }
    if input.get("vst3Data").is_some() || input.get("presetName").is_some() {
        return Some(Err("Give a preset path; Kumi's host validates and snapshots its bytes.".into()));
    }
    let path =
        input.get("path").or_else(|| input.get("preset").filter(|value| value.as_str().is_some_and(|name| has_suffix(name.trim()))))?;
    Some((|| {
        if input.get("path").is_some() && input.get("preset").is_some() || ["index", "step"].iter().any(|key| input.get(*key).is_some()) {
            return Err("Give a preset file alone, without another preset, index or step.".into());
        }
        snapshot(daw, input, path.as_str().ok_or("Preset path must be text.")?)
    })())
}

pub fn snapshot(daw: Daw, input: &Value, path: &str) -> Result<Value, String> {
    if path.chars().any(char::is_control) || kumi_common::path::network_or_device(path) || kumi_common::path::reserved_device(path) {
        return Err("Preset paths must name local files without control characters, network shares or reserved devices.".into());
    }
    let file = Path::new(path);
    if !file.is_absolute() || !has_suffix(path) {
        return Err("Give an absolute .vstpreset file path.".into());
    }
    if ["presetData", "presetName", "vst3Data"].iter().any(|key| input.get(*key).is_some()) {
        return Err("Give the file path alone; the host reads and validates its state.".into());
    }
    let limit = if daw == Daw::Reaper { 4 << 20 } else { 8 << 20 };
    let most = (limit - 65536) / 4 * 3;
    let metadata = fs::symlink_metadata(file).map_err(|e| format!("Couldn't inspect the preset file: {e}"))?;
    if !metadata.is_file() || metadata.len() > most as u64 {
        return Err("The preset must be a regular file within the DAW's undo state limit.".into());
    }
    let mut options = fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NONBLOCK | libc::O_NOFOLLOW);
    }
    let opened = options.open(file).map_err(|e| format!("Couldn't open the preset file: {e}"))?;
    let metadata = opened.metadata().map_err(|e| format!("Couldn't inspect the opened preset: {e}"))?;
    if !metadata.is_file() || metadata.len() > most as u64 {
        return Err("The preset changed into a non-regular or oversized file.".into());
    }
    let mut bytes = Vec::new();
    opened.take(most as u64 + 1).read_to_end(&mut bytes).map_err(|e| format!("Couldn't read the preset file: {e}"))?;
    if bytes.len() > most {
        return Err("The preset exceeds the DAW's undo state limit.".into());
    }
    daw_bridge::vst3::read(&bytes)?;
    let mut output = input.clone();
    let fields = output.as_object_mut().ok_or("Preset input must be an object.")?;
    fields.remove("path");
    fields.remove("preset");
    fields.insert("vst3Data".into(), base64::engine::general_purpose::STANDARD.encode(bytes).into());
    let mut name = if daw == Daw::Reaper { file.file_name() } else { file.file_stem() }.unwrap_or_default().to_string_lossy().into_owned();
    if daw == Daw::Reaper {
        name = name.chars().map(|c| if r#"<>:"/\|?*"#.contains(c) { '_' } else { c }).collect();
        if kumi_common::path::windows_reserved_device(&name) {
            name.insert(0, '_');
        }
    }
    fields.insert("presetName".into(), name.into());
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn controls_reserved_devices_and_non_files_are_refused_before_reading() {
        for path in ["\0\\\\host\\share\\x.vstpreset\0", "C:\\P\\x.vstpreset\0", "/tmp/x\n.vstpreset", "C:\\x\\COM3.vstpreset"] {
            assert!(snapshot(Daw::Reaper, &json!({}), path).is_err());
        }
        let temp = tempfile::tempdir().unwrap();
        let folder = temp.path().join("folder.vstpreset");
        fs::create_dir(&folder).unwrap();
        assert!(snapshot(Daw::Reaper, &json!({}), folder.to_str().unwrap()).is_err());
        let large = temp.path().join("large.vstpreset");
        fs::File::create(&large).unwrap().set_len(4 << 20).unwrap();
        assert!(snapshot(Daw::Reaper, &json!({}), large.to_str().unwrap()).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn unix_colon_files_and_program_names_are_supported() {
        for name in ["Aux: Hall", "A: Bright Pad"] {
            assert!(!file_program(name));
        }
        let temp = tempfile::tempdir().unwrap();
        let folder = temp.path().join("Aux");
        fs::create_dir(&folder).unwrap();
        let source = folder.join("A: Bright Pad.vstpreset");
        let bytes = base64::engine::general_purpose::STANDARD
            .decode("VlNUMwEAAAAwMDExMjIzMzQ0NTU2Njc3ODg5OUFBQkJDQ0RERUVGRjMAAAAAAAAAYWJjTGlzdAEAAABDb21wMAAAAAAAAAADAAAAAAAAAA==")
            .unwrap();
        fs::write(&source, &bytes).unwrap();
        let made = snapshot(Daw::Reaper, &json!({}), source.to_str().unwrap()).unwrap();
        assert_eq!(made["presetName"], "A_ Bright Pad.vstpreset");
        assert_eq!(base64::engine::general_purpose::STANDARD.decode(made["vst3Data"].as_str().unwrap()).unwrap(), bytes);
    }

    #[cfg(unix)]
    #[test]
    fn special_files_are_refused_without_opening_them() {
        use std::os::unix::{ffi::OsStrExt, fs::symlink};
        let temp = tempfile::tempdir().unwrap();
        let fifo = temp.path().join("fifo.vstpreset");
        let name = std::ffi::CString::new(fifo.as_os_str().as_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
        assert!(snapshot(Daw::Reaper, &json!({}), fifo.to_str().unwrap()).is_err());
        let link = temp.path().join("link.vstpreset");
        symlink(&fifo, &link).unwrap();
        assert!(snapshot(Daw::Reaper, &json!({}), link.to_str().unwrap()).is_err());
    }
}
