//! Kumi's own files are the producer's alone: the folders it makes are 0700 and the files it writes 0600 where the
//! system has modes, as every store under ~/.kumi keeps them (Windows keeps a profile's folders to their user already).
//! A folder that's already there keeps its mode: it may be one the producer pointed Kumi at.

use std::{io::Write, path::Path};

/// `folder`, and any folders above it that aren't there yet, readable only by the producer.
pub fn create_dir_all(folder: &Path) -> std::io::Result<()> {
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);
    builder.create(folder)
}

/// A file made new (or emptied) for writing, readable only by the producer.
pub fn create(file: &Path) -> std::io::Result<std::fs::File> {
    let mut options = std::fs::OpenOptions::new();
    options.create(true).truncate(true).write(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    options.open(file)
}

/// `bytes` written to `file` whole and only the producer's: through a temporary file of this write's own beside it,
/// renamed into place, so a reader never sees half of it and the file comes out 0600 whatever it was before.
pub fn write(file: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let folder = file.parent().filter(|folder| !folder.as_os_str().is_empty()).unwrap_or(Path::new("."));
    create_dir_all(folder)?;
    let tag = uuid::Uuid::new_v4().simple().to_string();
    let name = file.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
    let temporary = folder.join(format!(".{name}.{}-{}.partial", std::process::id(), &tag[..8]));
    let written = (|| {
        let mut out = create(&temporary)?;
        out.write_all(bytes)?;
        out.flush()?;
        drop(out);
        std::fs::rename(&temporary, file)
    })();
    if written.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    written
}

#[cfg(all(test, unix))]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn what_kumi_writes_is_the_producers_alone() {
        let root = tempfile::tempdir().unwrap();
        let file = root.path().join("new").join("deeper").join("store.json");
        // A file that was there, readable by anyone, comes out the producer's alone once it's written again.
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, "old").unwrap();
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o644)).unwrap();
        super::write(&file, b"new").unwrap();
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "new");
        assert_eq!(std::fs::metadata(&file).unwrap().permissions().mode() & 0o777, 0o600);
        // Folders it makes are 0700; nothing's left beside the file.
        let made = root.path().join("made").join("store.json");
        super::write(&made, b"{}").unwrap();
        assert_eq!(std::fs::metadata(made.parent().unwrap()).unwrap().permissions().mode() & 0o777, 0o700);
        let left: Vec<_> = std::fs::read_dir(made.parent().unwrap()).unwrap().flatten().map(|entry| entry.file_name()).collect();
        assert_eq!(left, vec![std::ffi::OsString::from("store.json")]);
    }
}
