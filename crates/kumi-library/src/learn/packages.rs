//! Atomic directory presets. Read content, not just the parent directory's modification time: a child can change
//! without changing that time (or its own size/time). No links are traversed, and every walk/read is bounded.

use std::io;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use kumi_common::abort::{Signal, SignalExt};
use sha2::{Digest, Sha256};
use tokio::io::AsyncReadExt;

const MOST_ENTRIES: usize = 4096;
const MOST_BYTES: u64 = 64 * 1024 * 1024;
const MOST_DEPTH: usize = 16;

#[derive(Debug)]
pub(super) struct Signature {
    pub size: u64,
    pub modified: SystemTime,
    pub fingerprint: String,
}

fn stopped(signal: &Signal) -> io::Result<()> {
    signal.check().map_err(|_| io::Error::new(io::ErrorKind::Interrupted, "package scan cancelled"))
}

fn invalid(reason: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, reason)
}

async fn local(path: &Path) -> io::Result<()> {
    let path = path.to_string_lossy().into_owned();
    let valid =
        tokio::task::spawn_blocking(move || crate::sources::canonical_local_path(&path).is_some()).await.map_err(io::Error::other)?;
    if valid {
        Ok(())
    } else {
        Err(invalid("package path contains a link or is not local"))
    }
}

pub(super) async fn signature(path: &str, signal: &Signal) -> io::Result<Signature> {
    stopped(signal)?;
    let root = PathBuf::from(path);
    local(&root).await?;
    let metadata = tokio::fs::symlink_metadata(&root).await?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(invalid("a preset package must be a real directory"));
    }
    let mut modified = metadata.modified()?;
    let mut queue = vec![(root.clone(), 0)];
    let mut records = Vec::new();
    let mut folders = Vec::new();
    let mut total = 0u64;
    let mut count = 0usize;
    while let Some((folder, depth)) = queue.pop() {
        stopped(signal)?;
        local(&folder).await?;
        let before = tokio::fs::symlink_metadata(&folder).await?;
        if !before.is_dir() || before.file_type().is_symlink() {
            return Err(invalid("preset package changed while scanning"));
        }
        folders.push((folder.clone(), before));
        let mut entries = tokio::fs::read_dir(&folder).await?;
        while let Some(entry) = entries.next_entry().await? {
            stopped(signal)?;
            count += 1;
            if count > MOST_ENTRIES {
                return Err(invalid("preset package exceeds 4096 entries"));
            }
            let path = entry.path();
            let metadata = tokio::fs::symlink_metadata(&path).await?;
            // A link makes the package incomplete: keeping its old entry is safer than claiming a coherent snapshot
            // while silently omitting content. In particular, a link to a huge external tree is never followed.
            if metadata.file_type().is_symlink() {
                return Err(invalid("preset package contains a link"));
            }
            #[cfg(windows)]
            {
                use std::os::windows::fs::MetadataExt;
                if metadata.file_attributes() & 0x400 != 0 {
                    return Err(invalid("preset package contains a reparse point"));
                }
            }
            let relative = path.strip_prefix(&root).map_err(io::Error::other)?.as_os_str().as_encoded_bytes().to_vec();
            modified = modified.max(metadata.modified()?);
            if metadata.is_dir() {
                if depth >= MOST_DEPTH {
                    return Err(invalid("preset package exceeds 16 directory levels"));
                }
                records.push((relative, vec![0]));
                queue.push((path, depth + 1));
            } else if metadata.is_file() {
                total = total
                    .checked_add(metadata.len())
                    .filter(|size| *size <= MOST_BYTES)
                    .ok_or_else(|| invalid("preset package exceeds 64 MiB"))?;
                local(&path).await?;
                let mut options = tokio::fs::OpenOptions::new();
                options.read(true);
                #[cfg(unix)]
                options.custom_flags(libc::O_NOFOLLOW);
                let mut input = options.open(&path).await?;
                let opened = input.metadata().await?;
                if !opened.is_file() || opened.len() != metadata.len() || opened.modified()? != metadata.modified()? {
                    return Err(invalid("preset package changed while scanning"));
                }
                let mut bytes = 0u64;
                let mut hash = Sha256::new();
                // This buffer lives across reads: keep its bytes out of the enclosing learn/walk futures' stack.
                let mut buffer = vec![0u8; 64 * 1024];
                loop {
                    stopped(signal)?;
                    let read = input.read(&mut buffer).await?;
                    if read == 0 {
                        break;
                    }
                    bytes += read as u64;
                    if bytes > metadata.len() || bytes > MOST_BYTES {
                        return Err(invalid("preset package changed while scanning"));
                    }
                    hash.update(&buffer[..read]);
                }
                let after = input.metadata().await?;
                if bytes != metadata.len() || after.len() != metadata.len() || after.modified()? != metadata.modified()? {
                    return Err(invalid("preset package changed while scanning"));
                }
                let mut record = vec![1];
                record.extend(metadata.len().to_le_bytes());
                let time = metadata.modified()?.duration_since(UNIX_EPOCH).unwrap_or_default();
                record.extend(time.as_nanos().to_le_bytes());
                record.extend(hash.finalize());
                records.push((relative, record));
            } else {
                return Err(invalid("preset package contains a non-file entry"));
            }
        }
    }
    // A new/deleted/renamed child changes its containing directory even if the package root's mtime is unchanged.
    // Reject an incomplete snapshot rather than publishing it and erasing the previously learned preset.
    for (folder, before) in folders {
        stopped(signal)?;
        local(&folder).await?;
        let after = tokio::fs::symlink_metadata(folder).await?;
        if !after.is_dir() || after.file_type().is_symlink() || after.modified()? != before.modified()? || after.len() != before.len() {
            return Err(invalid("preset package changed while scanning"));
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            if after.dev() != before.dev() || after.ino() != before.ino() {
                return Err(invalid("preset package changed while scanning"));
            }
        }
    }
    records.sort_by(|a, b| a.0.cmp(&b.0));
    let mut hash = Sha256::new();
    for (name, value) in records {
        hash.update((name.len() as u64).to_le_bytes());
        hash.update(name);
        hash.update((value.len() as u64).to_le_bytes());
        hash.update(value);
    }
    Ok(Signature { size: total, modified, fingerprint: hex::encode(hash.finalize()) })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn child_content_and_renames_change_the_package_identity_without_parent_time_coherence() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap().join("Voice.patch");
        std::fs::create_dir(&root).unwrap();
        let child = root.join("Voice.cst");
        std::fs::write(&child, b"first").unwrap();
        let signal = Signal::new();
        let first = signature(root.to_str().unwrap(), &signal).await.unwrap();
        let old = std::fs::metadata(&child).unwrap();
        std::fs::write(&child, b"other").unwrap();
        std::fs::File::options()
            .write(true)
            .open(&child)
            .unwrap()
            .set_times(std::fs::FileTimes::new().set_modified(old.modified().unwrap()))
            .unwrap();
        let second = signature(root.to_str().unwrap(), &signal).await.unwrap();
        assert_eq!(first.size, second.size);
        assert_ne!(first.fingerprint, second.fingerprint, "same-size and same-mtime child edits must invalidate");
        std::fs::rename(&child, root.join("Renamed.cst")).unwrap();
        assert_ne!(second.fingerprint, signature(root.to_str().unwrap(), &signal).await.unwrap().fingerprint);
        signal.cancel();
        assert_eq!(signature(root.to_str().unwrap(), &signal).await.unwrap_err().kind(), io::ErrorKind::Interrupted);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn links_and_oversized_packages_are_incomplete_without_reading_their_targets() {
        use std::os::unix::fs::symlink;
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap().join("Voice.patch");
        std::fs::create_dir(&root).unwrap();
        symlink("/no/such/external", root.join("linked.cst")).unwrap();
        let signal = Signal::new();
        assert!(signature(root.to_str().unwrap(), &signal).await.is_err());
        std::fs::remove_file(root.join("linked.cst")).unwrap();
        std::fs::File::create(root.join("huge.cst")).unwrap().set_len(MOST_BYTES + 1).unwrap();
        assert!(signature(root.to_str().unwrap(), &signal).await.is_err());
    }
}
