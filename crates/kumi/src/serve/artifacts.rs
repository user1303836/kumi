//! Approved media becomes a stable private copy before it is measured. Only validated copies enter C3's manifest.

use super::ctx::{make_private_dir, Ctx, OpError, Scratch};
use kumi_common::{abort::Signal, time::now_ms};
use kumi_library::sources::canonical_local_path;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    cell::RefCell,
    collections::BTreeMap,
    fs::{File, OpenOptions},
    io::{self, Read, Write},
    path::{Path, PathBuf},
    sync::Arc,
};
use tempfile::NamedTempFile;
use tokio::sync::Mutex;

/// The media route's limit. Refuse before copying a file the producer could not play from the card.
pub const MAX_BYTES: u64 = 50 * 1024 * 1024;
const MAX_MANIFEST_BYTES: u64 = 16 * 1024 * 1024;

#[derive(Clone, Serialize, Deserialize)]
struct Record {
    path: PathBuf,
    kind: String,
    mime: String,
    tool: String,
    at: i64,
}
type Records = BTreeMap<String, Record>;

pub struct SavedArtifact {
    pub id: String,
    pub path: PathBuf,
}

/// No manifest entry exists yet. Dropping this removes the owned copy, never its user-owned source.
pub struct StagedArtifact {
    file: NamedTempFile,
    id: String,
}
impl StagedArtifact {
    pub fn path(&self) -> &Path {
        self.file.path()
    }
}

pub struct Artifacts {
    root: PathBuf,
    records: RefCell<Arc<Records>>,
    publishing: Mutex<()>,
}
impl Artifacts {
    pub fn new(state_dir: &Path) -> Result<Self, OpError> {
        let state_dir = std::fs::canonicalize(state_dir).map_err(failed)?;
        let root = state_dir.join("artifacts");
        make_private_dir(&root).map_err(failed)?;
        let checked = local(&root)?;
        if checked != root || !checked.is_dir() {
            return Err(refused("outside_root", "The artifact folder must stay inside this profile's state folder."));
        }
        let records = match open_regular(&state_dir.join("artifacts.json")) {
            Ok(file) => {
                if file.metadata().map_err(failed)?.len() > MAX_MANIFEST_BYTES {
                    return Err(refused("too_large", "The artifact manifest is too large to read safely."));
                }
                let mut bytes = Vec::new();
                file.take(MAX_MANIFEST_BYTES + 1).read_to_end(&mut bytes).map_err(failed)?;
                if bytes.len() as u64 > MAX_MANIFEST_BYTES {
                    return Err(refused("too_large", "The artifact manifest changed while it was read."));
                }
                let mut document: serde_json::Map<String, Value> = serde_json::from_slice(&bytes).map_err(failed)?;
                if document.remove("schema") != Some(Value::from(super::snapshots::SCHEMA)) {
                    return Err(refused("artifact_schema", "The artifact manifest has an unsupported schema."));
                }
                for name in ["seq", "at", "owner"] {
                    document.remove(name);
                }
                let mut records = Records::new();
                for (id, value) in document {
                    if !artifact_id(&id) {
                        return Err(refused("artifact_schema", "The artifact manifest contains an invalid id."));
                    }
                    records.insert(id, serde_json::from_value(value).map_err(failed)?);
                }
                records
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => Records::new(),
            Err(error) => return Err(failed(error)),
        };
        Ok(Self { root, records: RefCell::new(Arc::new(records)), publishing: Mutex::new(()) })
    }

    /// Copy through an opened regular file, bounded by its initial length. A growing/changing file is not a snapshot.
    pub async fn stage(&self, ctx: &Ctx, source: &Path, signal: &Signal) -> Result<StagedArtifact, OpError> {
        check(signal)?;
        let source = ctx.approved(source)?;
        self.copy(source, signal).await
    }

    /// Native-only promotion of one call's owned scratch. A model's `file` source cannot use this grant.
    pub async fn stage_scratch(&self, ctx: &Ctx, scratch: &Scratch, source: &Path, signal: &Signal) -> Result<StagedArtifact, OpError> {
        check(signal)?;
        let root = local(scratch.path())?;
        let source = local(source)?;
        if root != scratch.path() || !root.starts_with(ctx.paths().state_dir.join("scratch")) || !source.starts_with(&root) {
            return Err(refused("outside_root", "This file does not belong to this call's native scratch."));
        }
        self.copy(source, signal).await
    }

    async fn copy(&self, source: PathBuf, signal: &Signal) -> Result<StagedArtifact, OpError> {
        let root = self.checked_root()?;
        let signal = signal.clone();
        tokio::task::spawn_blocking(move || {
            check(&signal)?;
            let mut input = open_regular(&source).map_err(failed)?;
            let before = input.metadata().map_err(failed)?;
            if before.len() > MAX_BYTES {
                return Err(refused("too_large", "This file exceeds the 50 MiB media limit; choose a shorter excerpt."));
            }
            let extension = source.extension().and_then(|part| part.to_str()).filter(|extension| {
                !extension.is_empty() && extension.len() <= 12 && extension.bytes().all(|byte| byte.is_ascii_alphanumeric())
            });
            let suffix = format!(".{}", extension.unwrap_or("bin"));
            let mut file = tempfile::Builder::new().prefix("a-").rand_bytes(24).suffix(&suffix).tempfile_in(root).map_err(failed)?;
            let mut left = before.len();
            let mut buffer = [0u8; 64 * 1024];
            while left != 0 {
                check(&signal)?;
                let wanted = left.min(buffer.len() as u64) as usize;
                let got = input.read(&mut buffer[..wanted]).map_err(failed)?;
                if got == 0 {
                    return Err(refused("changed", "The source file changed while Kumi copied it; export it again first."));
                }
                file.write_all(&buffer[..got]).map_err(failed)?;
                left -= got as u64;
            }
            let after = input.metadata().map_err(failed)?;
            if before.len() != after.len() || before.modified().ok() != after.modified().ok() {
                return Err(refused("changed", "The source file changed while Kumi copied it; export it again first."));
            }
            check(&signal)?;
            file.as_file().sync_all().map_err(failed)?;
            let id = file.path().file_stem().and_then(|name| name.to_str()).ok_or_else(|| failed("invalid artifact name"))?.to_owned();
            Ok(StagedArtifact { file, id })
        })
        .await
        .map_err(failed)?
    }

    pub async fn add(
        &self,
        ctx: &Ctx,
        source: &Path,
        kind: &str,
        mime: &str,
        tool: &str,
        signal: &Signal,
    ) -> Result<SavedArtifact, OpError> {
        self.publish(ctx, self.stage(ctx, source, signal).await?, kind, mime, tool, signal).await
    }

    pub async fn publish(
        &self,
        ctx: &Ctx,
        file: StagedArtifact,
        kind: &str,
        mime: &str,
        tool: &str,
        signal: &Signal,
    ) -> Result<SavedArtifact, OpError> {
        let [saved] = self.publish_files(ctx, [(file, kind, mime)], tool, signal).await?;
        Ok(saved)
    }

    /// Current and reference audio share one manifest commit: no half-published comparison on an error.
    pub async fn publish_pair(
        &self,
        ctx: &Ctx,
        source: StagedArtifact,
        source_mime: &str,
        reference: Option<(StagedArtifact, &str)>,
        tool: &str,
        signal: &Signal,
    ) -> Result<(SavedArtifact, Option<SavedArtifact>), OpError> {
        match reference {
            Some((reference, reference_mime)) => {
                let [source, reference] =
                    self.publish_files(ctx, [(source, "audio", source_mime), (reference, "audio", reference_mime)], tool, signal).await?;
                Ok((source, Some(reference)))
            }
            None => Ok((self.publish(ctx, source, "audio", source_mime, tool, signal).await?, None)),
        }
    }

    async fn publish_files<const N: usize>(
        &self,
        ctx: &Ctx,
        files: [(StagedArtifact, &str, &str); N],
        tool: &str,
        signal: &Signal,
    ) -> Result<[SavedArtifact; N], OpError> {
        let _publishing = tokio::select! {
            guard = self.publishing.lock() => guard,
            _ = signal.cancelled() => return Err(cancelled()),
        };
        check(signal)?;
        let root = self.checked_root()?;
        for (file, _, _) in &files {
            if file.path().parent() != Some(root.as_path()) || local(file.path())? != file.path() {
                return Err(refused("outside_root", "This staged file does not belong to this artifact store."));
            }
            if self.records.borrow().contains_key(&file.id) {
                return Err(refused("artifact_exists", "An artifact already has this id."));
            }
        }
        let at = now_ms();
        let added: [(String, Record); N] = std::array::from_fn(|i| {
            let (file, kind, mime) = &files[i];
            (file.id.clone(), Record { path: file.path().to_path_buf(), kind: (*kind).into(), mime: (*mime).into(), tool: tool.into(), at })
        });
        let existing = self.records.borrow().clone();
        let (document, added) = tokio::task::spawn_blocking(move || -> Result<_, OpError> {
            let mut document = serde_json::to_value(existing.as_ref()).map_err(failed)?;
            let fields = document.as_object_mut().ok_or_else(|| failed("invalid artifact manifest"))?;
            for (id, record) in &added {
                fields.insert(id.clone(), serde_json::to_value(record).map_err(failed)?);
            }
            Ok((document, added))
        })
        .await
        .map_err(failed)??;
        check(signal)?;
        // `keep` happens before the manifest can refer to a file. A failed commit drops only these owned copies.
        let mut retained = Vec::with_capacity(N);
        for (file, _, _) in files {
            let (handle, path) = file.file.keep().map_err(|error| failed(error.error))?;
            retained.push(PendingFile { handle: Some(handle), path, committed: false });
        }
        // Once queued, a forcibly dropped caller cannot know whether the writer committed. Keep those copies rather
        // than deleting files a manifest may now name; a known write failure can still roll the whole group back.
        for file in &mut retained {
            file.committed = true;
        }
        if let Err(error) = ctx.snapshots().write_durably("artifacts", document).await {
            for file in &mut retained {
                file.committed = false;
            }
            return Err(failed(error));
        }
        let mut records = self.records.borrow_mut();
        let records = Arc::make_mut(&mut records);
        let saved = added.map(|(id, record)| {
            let saved = SavedArtifact { id: id.clone(), path: record.path.clone() };
            records.insert(id, record);
            saved
        });
        Ok(saved)
    }

    pub fn resolve(&self, _ctx: &Ctx, id: &str) -> Result<PathBuf, OpError> {
        if !artifact_id(id) {
            return Err(OpError::Invalid { reason: "Use an artifact_id returned by Kumi.".into() });
        }
        let records = self.records.borrow();
        let record = records.get(id).ok_or_else(|| refused("missing_artifact", "This artifact is not registered in this profile."))?;
        let root = self.checked_root()?;
        let path = local(&record.path)?;
        if path.parent() != Some(root.as_path()) || path.file_stem().and_then(|name| name.to_str()) != Some(id) || !path.is_file() {
            return Err(refused("outside_root", "The artifact is no longer a regular file in this profile's artifact store."));
        }
        Ok(path)
    }

    fn checked_root(&self) -> Result<PathBuf, OpError> {
        let root = local(&self.root)?;
        if root != self.root || !root.is_dir() {
            return Err(refused("outside_root", "The artifact folder changed since the runtime started."));
        }
        Ok(root)
    }

    fn registered(&self, path: &Path) -> bool {
        if path.parent() != Some(self.root.as_path()) {
            return false;
        }
        path.file_stem()
            .and_then(|name| name.to_str())
            .and_then(|id| self.records.borrow().get(id).map(|record| record.path == path))
            .unwrap_or(false)
    }
}

struct PendingFile {
    handle: Option<File>,
    path: PathBuf,
    committed: bool,
}
impl Drop for PendingFile {
    fn drop(&mut self) {
        self.handle.take();
        if !self.committed {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

/// Ctx borrows its roots for this check; a file resolution does not allocate a copy of the root list.
pub fn approved(path: &Path, library: &[PathBuf], media: &[PathBuf], artifacts: &Artifacts) -> Result<PathBuf, OpError> {
    let path = local(path)?;
    if !path.is_file() || !(library.iter().chain(media).any(|root| path.starts_with(root)) || artifacts.registered(&path)) {
        return Err(refused("outside_root", "Use a regular file inside an approved library/media folder, or a registered artifact."));
    }
    Ok(path)
}

fn local(path: &Path) -> Result<PathBuf, OpError> {
    path.to_str().and_then(canonical_local_path).ok_or_else(|| {
        refused("outside_root", "Use a local absolute path without traversal, control characters, symlinks or network/device namespaces.")
    })
}
fn artifact_id(id: &str) -> bool {
    id.starts_with("a-")
        && (3..=80).contains(&id.len())
        && id.bytes().all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
}
fn open_regular(path: &Path) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        use windows_sys::Win32::Storage::FileSystem::{FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_READ};
        options.custom_flags(FILE_FLAG_OPEN_REPARSE_POINT).share_mode(FILE_SHARE_READ);
    }
    let file = options.open(path)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "not a regular file"));
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if metadata.file_attributes() & 0x400 != 0 {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "a reparse point is not a media file"));
        }
    }
    Ok(file)
}
fn refused(code: &'static str, reason: &str) -> OpError {
    OpError::Refused { code, reason: reason.into(), owner: None }
}
fn cancelled() -> OpError {
    refused("cancelled", "The operation was cancelled before artifact publication.")
}
fn check(signal: &Signal) -> Result<(), OpError> {
    if signal.is_cancelled() {
        Err(cancelled())
    } else {
        Ok(())
    }
}
fn failed(error: impl std::fmt::Display) -> OpError {
    OpError::Failed { reason: format!("Could not retain the media artifact: {error}") }
}

#[cfg(test)]
mod tests {
    use super::super::Args;
    use super::*;
    use kumi_types::{daw::DawId, system::Env};
    use std::rc::Rc;

    struct Fixture {
        ctx: Rc<Ctx>,
        root: PathBuf,
        _dir: tempfile::TempDir,
    }

    impl Fixture {
        fn new() -> Self {
            let dir = tempfile::tempdir().unwrap();
            let root = dir.path().join("approved");
            std::fs::create_dir(&root).unwrap();
            let root = root.canonicalize().unwrap();
            let ctx = Ctx::new(
                Args {
                    state_dir: dir.path().join("state"),
                    kumi_home: dir.path().join("home"),
                    daw: DawId::Reaper,
                    library_roots: vec![root.clone()],
                    media_roots: Vec::new(),
                    tools: BTreeMap::new(),
                },
                Env::new(),
                Default::default(),
            )
            .unwrap();
            Self { ctx, root, _dir: dir }
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            self.ctx.snapshots().flush();
        }
    }

    #[tokio::test]
    async fn failed_manifest_commit_removes_both_copies_not_user_files() {
        let fixture = Fixture::new();
        let ctx = &fixture.ctx;
        let source = fixture.root.join("current.wav");
        let reference = fixture.root.join("reference.wav");
        std::fs::write(&source, b"current audio").unwrap();
        std::fs::write(&reference, b"reference audio").unwrap();
        let signal = Signal::new();
        let current = ctx.artifacts().stage(ctx, &source, &signal).await.unwrap();
        let other = ctx.artifacts().stage(ctx, &reference, &signal).await.unwrap();
        let paths = [current.path().to_path_buf(), other.path().to_path_buf()];
        // A directory cannot be replaced by the manifest's regular file, on any supported platform.
        std::fs::create_dir(ctx.paths().state_dir.join("artifacts.json")).unwrap();
        let result = ctx.artifacts().publish_pair(ctx, current, "audio/wav", Some((other, "audio/wav")), "listen", &signal).await;
        assert!(matches!(result, Err(OpError::Failed { .. })));
        assert!(paths.iter().all(|path| !path.exists()));
        assert!(ctx.artifacts().records.borrow().is_empty());
        assert_eq!(std::fs::read(source).unwrap(), b"current audio");
        assert_eq!(std::fs::read(reference).unwrap(), b"reference audio");
    }

    #[tokio::test]
    async fn scratch_is_a_native_capability_not_a_model_file_grant() {
        let fixture = Fixture::new();
        let ctx = &fixture.ctx;
        let own = ctx.scratch("listen").unwrap();
        let other = ctx.scratch("listen").unwrap();
        let source = own.path().join("capture.wav");
        std::fs::write(&source, b"verified capture").unwrap();
        let signal = Signal::new();
        assert_eq!(ctx.approved(&source).unwrap_err().code(), "outside_root");
        let wrong = ctx.artifacts().stage_scratch(ctx, &other, &source, &signal).await;
        assert!(matches!(wrong, Err(OpError::Refused { code: "outside_root", .. })));
        let staged = ctx.artifacts().stage_scratch(ctx, &own, &source, &signal).await.unwrap();
        assert_eq!(std::fs::read(staged.path()).unwrap(), b"verified capture");
        assert_eq!(std::fs::read(source).unwrap(), b"verified capture");
        let copied = staged.path().to_path_buf();
        drop(staged);
        assert!(!copied.exists(), "unpublished capture is removed on refusal or cancellation");
    }

    #[tokio::test]
    async fn over_limit_media_is_refused_before_copying() {
        let fixture = Fixture::new();
        let source = fixture.root.join("too-large.wav");
        File::create(&source).unwrap().set_len(MAX_BYTES + 1).unwrap();
        let result = fixture.ctx.artifacts().stage(&fixture.ctx, &source, &Signal::new()).await;
        assert!(matches!(result, Err(OpError::Refused { code: "too_large", .. })));
        assert_eq!(std::fs::read_dir(&fixture.ctx.artifacts().root).unwrap().count(), 0);
        assert_eq!(std::fs::metadata(source).unwrap().len(), MAX_BYTES + 1);
    }

    #[test]
    fn a_manifest_entry_cannot_grant_files_outside_the_artifact_store() {
        let fixture = Fixture::new();
        let source = fixture.root.join("a-untrusted.wav");
        std::fs::write(&source, b"user-owned").unwrap();
        let manifest = serde_json::json!({
            "schema": 1,
            "a-untrusted": {"path": source, "kind": "audio", "mime": "audio/wav", "tool": "listen", "at": 1}
        });
        std::fs::write(fixture.ctx.paths().state_dir.join("artifacts.json"), serde_json::to_vec(&manifest).unwrap()).unwrap();
        let store = Artifacts::new(&fixture.ctx.paths().state_dir).unwrap();
        assert_eq!(store.resolve(&fixture.ctx, "a-untrusted").unwrap_err().code(), "outside_root");
        assert_eq!(std::fs::read(source).unwrap(), b"user-owned");
    }
}
