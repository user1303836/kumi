//! Devices Kumi made before 1.12.1 lived directly in Kumi/, where Live's Browser gives no clue whether they are
//! instruments. Keep the original for saved Sets and load a byte-identical copy from a folder that names its kind.

mod directory;

use super::{amxd::DeviceType, patch::frozen::read_device, tool::kind_folder};
use directory::Directory;
use sha2::{Digest, Sha256};
use std::{io::ErrorKind, path::Path};

const MOST_BYTES: u64 = 256 * 1024 * 1024;

/// A flat Kumi device's kind-qualified Browser item, or None for other items. A file already there is never replaced.
pub(crate) fn prepare(library: &Path, item: &str) -> Result<Option<String>, String> {
    prepare_with(library, item, || {})
}

fn prepare_with(library: &Path, item: &str, after_open: impl FnOnce()) -> Result<Option<String>, String> {
    let Some(source) = source_name(item)? else { return Ok(None) };
    if kumi_common::path::network_or_device(&library.to_string_lossy()) {
        return Err("Kumi's old device needs a local User Library, not a network or device path.".into());
    }
    let root = Directory::root(library).map_err(|error| format!("Kumi couldn't open Live's User Library: {error}."))?;
    let kumi = root.child("Kumi", false).map_err(|error| format!("Kumi couldn't open its old device folder: {error}."))?;
    // Read through the pinned Kumi directory, never through a path a concurrent rename could redirect. The handle is
    // checked to be a regular file, and reading stops at the limit even if another process grows the file.
    let bytes = kumi.read(&source, MOST_BYTES).map_err(|error| format!("Kumi couldn't read the old device: {error}."))?;
    let device = read_device(&bytes).map_err(|_| "The old file isn't a Max for Live device Kumi can read.".to_string())?;
    let kind = DeviceType::ALL
        .into_iter()
        .find(|kind| kind.code() == device.code)
        .ok_or_else(|| "The old file isn't an audio effect, MIDI effect or instrument Kumi can identify.".to_string())?;
    drop(device);
    let name = &source[..source.len() - ".amxd".len()];
    let hash = hex::encode(Sha256::digest(&bytes));
    let folder = kind_folder(kind);
    let destination = kumi.child(folder, true).map_err(|error| format!("Kumi couldn't open the old device's new folder: {error}."))?;
    // Tests can replace a directory here, at exactly the point a path-based temporary creation would be redirected.
    after_open();
    let mut temporary =
        destination.temporary(&bytes).map_err(|error| format!("Kumi couldn't prepare a copy of the old device: {error}."))?;
    let current = || {
        if root.is_at(library) && root.contains("Kumi", &kumi) && kumi.contains(folder, &destination) {
            Ok(())
        } else {
            Err("The User Library's device folder moved while Kumi was copying the old device: try loading it again.".to_string())
        }
    };
    for index in 0..100 {
        let name = match index {
            0 => name.to_string(),
            1 => format!("{name} (legacy {})", &hash[..12]),
            _ => format!("{name} (legacy {} {index})", &hash[..12]),
        };
        let file = format!("{name}.amxd");
        // Unix publishes relative to the held directory descriptor; Windows keeps every containing directory open
        // without delete sharing. No path check/use gap can redirect bytes, and publication never replaces a file.
        match temporary.persist(&file) {
            Ok(()) => {
                current()?;
                return Ok(Some(format!("user_library/Kumi/{folder}/{name}")));
            }
            Err(error) if error.kind() == ErrorKind::AlreadyExists => {
                // Repeated loads share a copy only while its bytes are unchanged. A link or another device keeps its
                // name and is never overwritten; use a different name for the legacy copy instead.
                if destination.read(&file, bytes.len() as u64).is_ok_and(|kept| kept == bytes) {
                    current()?;
                    return Ok(Some(format!("user_library/Kumi/{folder}/{name}")));
                }
            }
            Err(error) => return Err(format!("Kumi couldn't save a copy of the old device without replacing another file: {error}.")),
        }
    }
    Err("The User Library already has too many devices with this old device's name: rename the old device and load it again.".into())
}

/// A single filename, with the same suffix and traversal rules as a User Library Browser item. No filesystem access.
fn source_name(item: &str) -> Result<Option<String>, String> {
    let Some(name) = item.strip_prefix("user_library/Kumi/").filter(|name| !name.contains('/')) else { return Ok(None) };
    let name = name.rsplit_once('#').filter(|(_, n)| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit())).map_or(name, |(name, _)| name);
    if name.is_empty() || matches!(name, "." | "..") || name.contains(['\\', ':']) || name.chars().any(char::is_control) {
        return Err("That isn't a device's item in the User Library's Kumi folder.".into());
    }
    let file = if name.to_lowercase().ends_with(".amxd") { name.to_string() } else { format!("{name}.amxd") };
    if file.len() == ".amxd".len() || kumi_common::path::reserved_device(&file) {
        return Err("That isn't a device's filename.".into());
    }
    Ok(Some(file))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::devices::{
        amxd::{encode_amxd, DeviceType},
        existing,
    };
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

    #[cfg(unix)]
    #[test]
    fn replacing_a_directory_after_it_is_opened_never_redirects_the_copy() {
        use std::os::unix::fs::symlink;
        for replaced in ["Kumi/Audio Effects", "Kumi"] {
            let library = tempfile::tempdir().unwrap();
            let outside = tempfile::tempdir().unwrap();
            let bytes = old(library.path(), "Echo", DeviceType::AudioEffect);
            std::fs::create_dir(library.path().join("Kumi/Audio Effects")).unwrap();
            let path = library.path().join(replaced);
            let moved = library.path().join("held");
            let result = prepare_with(library.path(), "user_library/Kumi/Echo", || {
                std::fs::rename(&path, &moved).unwrap();
                symlink(outside.path(), &path).unwrap();
            });
            assert!(result.is_err(), "a Browser item now naming another directory must not be loaded: {result:?}");
            assert_eq!(std::fs::read_dir(outside.path()).unwrap().count(), 0, "nothing written through the replacement link");
            let copy = if replaced == "Kumi" { moved.join("Audio Effects/Echo.amxd") } else { moved.join("Echo.amxd") };
            assert_eq!(std::fs::read(copy).unwrap(), bytes, "writes used the opened directory's identity");
            let temporary_folder = if replaced == "Kumi" { moved.join("Audio Effects") } else { moved.clone() };
            assert_eq!(std::fs::read_dir(temporary_folder).unwrap().count(), 1, "temporary cleanup used the same held directory");
        }
    }

    #[cfg(windows)]
    #[test]
    fn containing_directories_cannot_be_replaced_while_the_copy_is_written() {
        let library = tempfile::tempdir().unwrap();
        let bytes = old(library.path(), "Echo", DeviceType::AudioEffect);
        let result = prepare_with(library.path(), "user_library/Kumi/Echo", || {
            for path in [library.path().join("Kumi/Audio Effects"), library.path().join("Kumi")] {
                assert!(std::fs::rename(path, library.path().join("replaced")).is_err(), "the directory is pinned without delete sharing");
            }
        });
        assert_eq!(result.unwrap(), Some("user_library/Kumi/Audio Effects/Echo".into()));
        assert_eq!(std::fs::read(library.path().join("Kumi/Audio Effects/Echo.amxd")).unwrap(), bytes);
    }
}
