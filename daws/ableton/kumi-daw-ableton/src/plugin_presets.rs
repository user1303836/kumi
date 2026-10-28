//! Put a complete VST3 preset in Live's User Library. `load_device` then performs the normal
//! preview/apply/history/undo transaction; preparing a file itself changes no Live device.

use kumi_common::path::network_or_device;
use kumi_library::plugins::formats::{structure, vstpreset::VstPreset, MAX_DECODED_BYTES};
use sha2::{Digest, Sha256};
use std::{
    io::{Read, Write},
    path::{Path, PathBuf},
};

#[derive(Debug)]
pub struct PreparedPreset {
    pub file: PathBuf,
    pub item: String,
    pub name: String,
    pub plugin: String,
    pub kind: String,
    pub class_id: String,
}

pub fn prepare(input: &Path, library: &Path, name: &str) -> Result<PreparedPreset, String> {
    if [input, library]
        .iter()
        .any(|path| path.to_string_lossy().chars().any(char::is_control) || kumi_common::path::reserved_device(&path.to_string_lossy()))
    {
        return Err("Use local preset and User Library paths without control characters or reserved device names.".into());
    }
    if !input.is_absolute() || !library.is_absolute() {
        return Err("Preset and User Library paths must be absolute".into());
    }
    if network_or_device(&input.to_string_lossy()) || network_or_device(&library.to_string_lossy()) {
        return Err("Use local preset and User Library paths; network and device paths are not supported".into());
    }
    if !library.is_dir() {
        return Err("Live's configured User Library is missing. Open Live, check its Library location, then restart Kumi.".into());
    }
    if !input.extension().is_some_and(|ext| ext.eq_ignore_ascii_case("vstpreset")) {
        return Err("Give a .vstpreset produced by vst3 create/edit".into());
    }
    if name.trim().is_empty()
        || name.chars().count() > 64
        || name.chars().any(|c| c.is_control() || r#"<>:"/\|?*"#.contains(c))
        || name.ends_with(['.', ' '])
    {
        return Err("Preset name must be 1–64 characters without filename separators or control characters".into());
    }
    let metadata = std::fs::symlink_metadata(input).map_err(|e| e.to_string())?;
    if !metadata.is_file() || metadata.len() > MAX_DECODED_BYTES as u64 {
        return Err("Preset input must be a regular file within the supported size limit".into());
    }
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NONBLOCK | libc::O_NOFOLLOW);
    }
    let source = options.open(input).map_err(|e| e.to_string())?;
    let metadata = source.metadata().map_err(|e| e.to_string())?;
    if !metadata.is_file() || metadata.len() > MAX_DECODED_BYTES as u64 {
        return Err("Preset input must be a regular file within the supported size limit".into());
    }
    let mut bytes = Vec::new();
    source.take(MAX_DECODED_BYTES as u64 + 1).read_to_end(&mut bytes).map_err(|e| e.to_string())?;
    if bytes.len() > MAX_DECODED_BYTES {
        return Err("VST3 preset exceeds the supported size limit".into());
    }
    let preset = VstPreset::read(&bytes).map_err(|e| e.to_string())?;
    if preset.version != 1 {
        return Err("Unsupported VST3 preset container version".into());
    }
    if preset.component().is_none_or(|c| c.is_empty()) {
        return Err("VST3 preset has no processor state".into());
    }
    let format = structure::builtins()
        .iter()
        .find(|p| p.info.ids.vst3.iter().any(|id| id.id.eq_ignore_ascii_case(&preset.class_id)))
        .ok_or("VST3 preset class is not in Kumi's built-in catalog")?;
    let product = format.info.ids.vst3.iter().find(|id| id.id.eq_ignore_ascii_case(&preset.class_id)).unwrap();
    // The folder also tells Live's existing loader whether an instrument could replace another.
    let folder = if format.info.kind == "instrument" { "Instruments" } else { "Audio Effects" };
    let digest = hex::encode(Sha256::digest(&bytes));
    let label = format!("{name} {}", &digest[..12]);
    let directory = library.join("Kumi").join(folder);
    std::fs::create_dir_all(&directory)
        .map_err(|e| format!("Kumi couldn't write Live's User Library; check its location and folder permissions: {e}"))?;
    let file = directory.join(format!("{label}.vstpreset"));
    match std::fs::OpenOptions::new().write(true).create_new(true).open(&file) {
        Ok(mut out) => {
            if let Err(e) = out.write_all(&bytes).and_then(|_| out.sync_all()) {
                drop(out);
                let _ = std::fs::remove_file(&file);
                return Err(format!(
                    "Kumi couldn't finish saving the preset in Live's User Library; check free disk space and folder permissions: {e}"
                ));
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            // A concurrent copy or a repeated request is safe only if its bytes are still ours.
            let metadata = std::fs::symlink_metadata(&file).map_err(|e| e.to_string())?;
            if !metadata.is_file() || metadata.len() != bytes.len() as u64 {
                return Err("Preset destination already exists with different bytes".into());
            }
            let mut existing = Vec::new();
            options
                .open(&file)
                .map_err(|e| e.to_string())?
                .take(bytes.len() as u64 + 1)
                .read_to_end(&mut existing)
                .map_err(|e| e.to_string())?;
            if existing != bytes {
                return Err("Preset destination already exists with different bytes".into());
            }
        }
        Err(e) => return Err(format!("Kumi couldn't write Live's User Library; check its folder permissions: {e}")),
    }
    Ok(PreparedPreset {
        file,
        // Live keeps the extension in a VST3 preset's Browser name (unlike its own device presets).
        item: format!("user_library/Kumi/{folder}/{label}.vstpreset"),
        name: label,
        plugin: product.name.clone(),
        kind: format.info.kind.clone(),
        class_id: preset.class_id,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn stages_exact_vst3_state_in_the_right_live_folder_and_reuses_only_identical_bytes() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("input.vstpreset");
        let bytes = include_bytes!(concat!(env!("KUMI_WORKSPACE"), "/plugin-formats/serum-2/format-1/fixtures/init.vstpreset"));
        std::fs::write(&source, bytes).unwrap();
        let made = prepare(&source, temp.path(), "Kumi synth").unwrap();
        assert!(made.item.starts_with("user_library/Kumi/Instruments/"));
        assert_eq!(made.item, format!("user_library/Kumi/Instruments/{}.vstpreset", made.name));
        assert_eq!(std::fs::read(&made.file).unwrap(), bytes);
        assert_eq!(prepare(&source, temp.path(), "Kumi synth").unwrap().file, made.file);
        std::fs::write(&made.file, b"someone else's preset").unwrap();
        assert!(prepare(&source, temp.path(), "Kumi synth").is_err());
        assert!(prepare(&source, temp.path(), "../escape").is_err());
        assert!(prepare(&source, &temp.path().join("missing"), "valid").unwrap_err().contains("Library is missing"));
        assert!(!temp.path().join("missing").exists());
        let blocked = tempfile::tempdir().unwrap();
        std::fs::write(blocked.path().join("Kumi"), b"not a folder").unwrap();
        let error = prepare(&source, blocked.path(), "valid").unwrap_err();
        assert!(error.contains("User Library") && error.contains("permissions"), "{error}");
        assert!(prepare(Path::new("//host/share/test.vstpreset"), temp.path(), "valid").is_err());
        for path in ["\0//host/share/x.vstpreset", "/tmp/x\n.vstpreset"] {
            assert!(prepare(Path::new(path), temp.path(), "valid").unwrap_err().contains("local"));
            assert!(prepare(&source, Path::new(path), "valid").unwrap_err().contains("local"));
        }
        #[cfg(windows)]
        assert!(prepare(Path::new("C:/P/CON.vstpreset"), temp.path(), "valid").unwrap_err().contains("local"));
    }

    #[cfg(unix)]
    #[test]
    fn unix_preset_paths_allow_colons_and_dos_device_directory_names() {
        let temp = tempfile::tempdir().unwrap();
        let directory = temp.path().join("Aux");
        std::fs::create_dir(&directory).unwrap();
        let source = directory.join("X:Y.vstpreset");
        let bytes = include_bytes!(concat!(env!("KUMI_WORKSPACE"), "/plugin-formats/serum-2/format-1/fixtures/init.vstpreset"));
        std::fs::write(&source, bytes).unwrap();
        let made = prepare(&source, &directory, "Kumi synth").unwrap();
        assert_eq!(std::fs::read(made.file).unwrap(), bytes);
    }
}
