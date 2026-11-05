//! Devices Kumi made before 1.12.1 lived directly in Kumi/, where Live's Browser gives no clue whether they are
//! instruments. Keep the original for saved Sets and load a byte-identical copy from a folder that names its kind.

use super::{existing, tool::kind_folder};
use std::{
    io::{ErrorKind, Write},
    path::{Path, PathBuf},
};

/// A flat Kumi device's kind-qualified Browser item, or None for other items. A file already there is never replaced.
pub(crate) fn prepare(library: &Path, item: &str) -> Result<Option<String>, String> {
    let Some(name) = item.strip_prefix("user_library/Kumi/").filter(|name| !name.contains('/')) else { return Ok(None) };
    // `find` applies the Browser path's traversal, Windows separator/drive and duplicate-id checks, and adds .amxd.
    let source = existing::find(item, library)?;
    if name.is_empty() || !std::fs::symlink_metadata(&source).is_ok_and(|metadata| metadata.is_file()) {
        return Err("Kumi's old device must be a regular file in the User Library, not a link.".into());
    }
    let library = std::fs::canonicalize(library).map_err(|error| format!("Kumi couldn't open Live's User Library: {error}."))?;
    let kumi = directory(&library, "Kumi", false)?;
    let canonical = std::fs::canonicalize(&source).map_err(|error| format!("Kumi couldn't open the old device: {error}."))?;
    if canonical.parent() != Some(kumi.as_path()) {
        return Err("Kumi's old device must stay inside the User Library's Kumi folder.".into());
    }
    // One bounded read, checked through its handle (and O_NOFOLLOW on Unix), identifies the actual device kind. An
    // unreadable or unknown kind never becomes an effect just because that's where it would be convenient to load it.
    let original = existing::open(&canonical, &library)?;
    let folder = kind_folder(original.kind);
    let destination = directory(&kumi, folder, true)?;
    let mut temporary = tempfile::NamedTempFile::new_in(&destination)
        .map_err(|error| format!("Kumi couldn't prepare a copy of the old device: {error}."))?;
    temporary
        .write_all(&original.bytes)
        .and_then(|_| temporary.as_file().sync_all())
        .map_err(|error| format!("Kumi couldn't finish copying the old device: {error}."))?;
    for index in 0..100 {
        let name = match index {
            0 => original.name.clone(),
            1 => format!("{} (legacy {})", original.name, &original.hash[..12]),
            _ => format!("{} (legacy {} {index})", original.name, &original.hash[..12]),
        };
        let file = destination.join(format!("{name}.amxd"));
        // Recheck the destination directory before publishing. persist_noclobber publishes the whole copy atomically,
        // and protects another Kumi's or the producer's file even if it appears after the name was chosen.
        if directory(&kumi, folder, false)? != destination {
            return Err("The User Library's device folder moved while Kumi was copying the old device: try loading it again.".into());
        }
        match temporary.persist_noclobber(&file) {
            Ok(_) => return Ok(Some(format!("user_library/Kumi/{folder}/{name}"))),
            Err(error) => {
                let exists = error.error.kind() == ErrorKind::AlreadyExists;
                temporary = error.file;
                if !exists {
                    return Err(format!("Kumi couldn't save a copy of the old device: {}.", error.error));
                }
                // Repeated loads share the same copy only while its bytes are unchanged. A link or another device
                // keeps its name and is never overwritten; use a different name for the legacy copy instead.
                if std::fs::symlink_metadata(&file)
                    .is_ok_and(|metadata| metadata.is_file() && metadata.len() == original.bytes.len() as u64)
                    && existing::read_limited(&file, original.bytes.len() as u64).is_ok_and(|bytes| bytes == original.bytes)
                {
                    return Ok(Some(format!("user_library/Kumi/{folder}/{name}")));
                }
            }
        }
    }
    Err("The User Library already has too many devices with this old device's name: rename the old device and load it again.".into())
}

/// A real child directory, never a link redirecting the copy elsewhere. The User Library's root itself may be linked.
fn directory(parent: &Path, name: &str, create: bool) -> Result<PathBuf, String> {
    let path = parent.join(name);
    if create {
        match std::fs::create_dir(&path) {
            Ok(()) => {}
            Err(error) if error.kind() == ErrorKind::AlreadyExists => {}
            Err(error) => return Err(format!("Kumi couldn't make the old device's folder: {error}.")),
        }
    }
    let metadata = std::fs::symlink_metadata(&path).map_err(|error| format!("Kumi couldn't open the old device's folder: {error}."))?;
    if !metadata.is_dir() {
        return Err("Kumi's old device needs real Kumi and device-kind folders in the User Library, not links.".into());
    }
    let canonical = std::fs::canonicalize(&path).map_err(|error| format!("Kumi couldn't open the old device's folder: {error}."))?;
    if canonical.parent() != Some(parent) {
        return Err("Kumi's old device folder must stay inside the User Library.".into());
    }
    Ok(canonical)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::devices::amxd::{encode_amxd, DeviceType};
    use serde_json::json;

    fn old(library: &Path, name: &str, kind: DeviceType) -> Vec<u8> {
        std::fs::create_dir_all(library.join("Kumi")).unwrap();
        let bytes = encode_amxd(kind, &json!({"patcher":{"boxes":[],"lines":[]}}));
        std::fs::write(library.join("Kumi").join(format!("{name}.amxd")), &bytes).unwrap();
        bytes
    }

    #[test]
    fn old_devices_load_copies_from_their_actual_kind_and_keep_the_original() {
        let library = tempfile::tempdir().unwrap();
        for kind in DeviceType::ALL {
            let name = format!("Old {kind}");
            let bytes = old(library.path(), &name, kind);
            let item = format!("user_library/Kumi/{name}");
            let expected = format!("user_library/Kumi/{}/{name}", kind_folder(kind));
            assert_eq!(prepare(library.path(), &item).unwrap(), Some(expected.clone()));
            assert_eq!(prepare(library.path(), &format!("{item}#2")).unwrap(), Some(expected.clone()), "same copy on another load");
            assert_eq!(std::fs::read(library.path().join("Kumi").join(format!("{name}.amxd"))).unwrap(), bytes);
            assert_eq!(std::fs::read(existing::find(&expected, library.path()).unwrap()).unwrap(), bytes);
        }
    }

    #[test]
    fn a_producers_device_keeps_its_name_and_its_bytes() {
        let library = tempfile::tempdir().unwrap();
        let bytes = old(library.path(), "Echo", DeviceType::AudioEffect);
        std::fs::create_dir(library.path().join("Kumi/Audio Effects")).unwrap();
        let theirs = library.path().join("Kumi/Audio Effects/Echo.amxd");
        std::fs::write(&theirs, b"the producer's device").unwrap();
        let item = prepare(library.path(), "user_library/Kumi/Echo").unwrap().unwrap();
        assert!(item.starts_with("user_library/Kumi/Audio Effects/Echo (legacy "), "{item}");
        assert_eq!(std::fs::read(theirs).unwrap(), b"the producer's device");
        assert_eq!(std::fs::read(existing::find(&item, library.path()).unwrap()).unwrap(), bytes);
        assert_eq!(prepare(library.path(), "user_library/Kumi/Echo").unwrap(), Some(item));
    }

    #[test]
    fn unknown_files_and_paths_outside_the_flat_folder_are_never_reclassified() {
        let library = tempfile::tempdir().unwrap();
        old(library.path(), "Unknown", DeviceType::MidiEffect);
        std::fs::write(library.path().join("Kumi/Unknown.amxd"), b"not a device").unwrap();
        assert!(prepare(library.path(), "user_library/Kumi/Unknown").is_err());
        for item in ["user_library/Kumi/../secret", "user_library/Kumi/MIDI Effects/Echo", "audio_effects/Reverb"] {
            assert_eq!(prepare(library.path(), item).unwrap(), None, "not a flat Kumi item: {item}");
        }
        for item in ["user_library/Kumi/..\\secret", "user_library/Kumi/C:secret"] {
            assert!(prepare(library.path(), item).is_err(), "unsafe: {item}");
        }
        assert!(!library.path().join("Kumi/MIDI Effects").exists());
    }

    #[cfg(unix)]
    #[test]
    fn links_never_redirect_the_source_or_the_copy_outside_the_library() {
        use std::os::unix::fs::symlink;
        let library = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let bytes = old(library.path(), "Echo", DeviceType::AudioEffect);
        std::fs::write(outside.path().join("Outside.amxd"), &bytes).unwrap();
        symlink(outside.path().join("Outside.amxd"), library.path().join("Kumi/Linked.amxd")).unwrap();
        assert!(prepare(library.path(), "user_library/Kumi/Linked").is_err());
        symlink(outside.path(), library.path().join("Kumi/Audio Effects")).unwrap();
        assert!(prepare(library.path(), "user_library/Kumi/Echo").is_err());
        assert!(!outside.path().join("Echo.amxd").exists());
    }
}
