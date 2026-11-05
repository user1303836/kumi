//! Approved media becomes a stable private copy before it is measured. Only validated copies enter C3's manifest.

use super::ctx::{Ctx, OpError, Scratch};
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

/// Approval pins and map selections use the same open-handle identity, including the containing device.
/// Nanoseconds are a decimal string so a JSON round trip cannot round the timestamp.
pub fn file_identity(file: &File) -> io::Result<Value> {
    Ok(FileIdentity::of(file)?.value())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct FileIdentity {
    dev: u64,
    ino: u64,
    size: u64,
    mtime_ns: i128,
}

impl FileIdentity {
    fn of(file: &File) -> io::Result<Self> {
        #[cfg(unix)]
        {
            Ok(Self::from_metadata(&file.metadata()?))
        }
        #[cfg(windows)]
        {
            Ok(Self::from_windows_info(handle_info(file)?))
        }
    }

    pub(crate) fn at(path: &Path) -> io::Result<Self> {
        #[cfg(unix)]
        {
            let metadata = std::fs::symlink_metadata(path)?;
            if !metadata.is_file() {
                return Err(io::Error::new(io::ErrorKind::InvalidInput, "not a regular file"));
            }
            Ok(Self::from_metadata(&metadata))
        }
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            use windows_sys::Win32::Storage::FileSystem::{
                FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_REPARSE_POINT, FILE_FLAG_OPEN_REPARSE_POINT, FILE_READ_ATTRIBUTES,
                FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
            };
            // Attribute-only access can validate a NamedTempFile whose existing handle also has write access.
            let file = OpenOptions::new()
                .access_mode(FILE_READ_ATTRIBUTES)
                .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
                .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
                .open(path)?;
            let info = handle_info(&file)?;
            if info.dwFileAttributes & (FILE_ATTRIBUTE_DIRECTORY | FILE_ATTRIBUTE_REPARSE_POINT) != 0 {
                return Err(io::Error::new(io::ErrorKind::InvalidInput, "not a regular local file"));
            }
            Ok(Self::from_windows_info(info))
        }
    }

    #[cfg(unix)]
    fn from_metadata(metadata: &std::fs::Metadata) -> Self {
        use std::os::unix::fs::MetadataExt;
        Self {
            dev: metadata.dev(),
            ino: metadata.ino(),
            size: metadata.len(),
            mtime_ns: i128::from(metadata.mtime()) * 1_000_000_000 + i128::from(metadata.mtime_nsec()),
        }
    }

    #[cfg(windows)]
    fn from_windows_info(info: windows_sys::Win32::Storage::FileSystem::BY_HANDLE_FILE_INFORMATION) -> Self {
        let ticks = (u64::from(info.ftLastWriteTime.dwHighDateTime) << 32) | u64::from(info.ftLastWriteTime.dwLowDateTime);
        Self {
            dev: u64::from(info.dwVolumeSerialNumber),
            ino: (u64::from(info.nFileIndexHigh) << 32) | u64::from(info.nFileIndexLow),
            size: (u64::from(info.nFileSizeHigh) << 32) | u64::from(info.nFileSizeLow),
            mtime_ns: (i128::from(ticks) - 116_444_736_000_000_000) * 100,
        }
    }

    pub(crate) fn value(&self) -> Value {
        serde_json::json!({"dev":self.dev,"ino":self.ino,"size":self.size,"mtime_ns":self.mtime_ns.to_string()})
    }

    fn matches(&self, value: &Value) -> bool {
        value.as_object().is_some_and(|object| {
            object.len() == 4
                && value.get("dev").and_then(Value::as_u64) == Some(self.dev)
                && value.get("ino").and_then(Value::as_u64) == Some(self.ino)
                && value.get("size").and_then(Value::as_u64) == Some(self.size)
                && value.get("mtime_ns").and_then(Value::as_str).and_then(|value| value.parse::<i128>().ok()) == Some(self.mtime_ns)
        })
    }
}

#[cfg(windows)]
fn handle_info(file: &File) -> io::Result<windows_sys::Win32::Storage::FileSystem::BY_HANDLE_FILE_INFORMATION> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::GetFileInformationByHandle;
    let mut info = std::mem::MaybeUninit::uninit();
    // SAFETY: File owns a live handle and the API initializes the output structure on success.
    if unsafe { GetFileInformationByHandle(file.as_raw_handle() as _, info.as_mut_ptr()) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(unsafe { info.assume_init() })
}

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

/// An unpublished private copy, or a read handle for an already registered immutable artifact. Only the former
/// is removed on drop; replaying a retained artifact neither duplicates it nor changes its provenance.
pub struct StagedArtifact {
    file: StagedFile,
    id: String,
    identity: FileIdentity,
}

enum StagedFile {
    Owned(NamedTempFile),
    Registered { path: PathBuf, handle: File },
}

impl StagedArtifact {
    pub fn path(&self) -> &Path {
        match &self.file {
            StagedFile::Owned(file) => file.path(),
            StagedFile::Registered { path, .. } => path,
        }
    }

    /// The already-owned file. Read it in place instead of reopening a writable Windows temp file.
    pub(crate) fn opened(&self) -> &File {
        match &self.file {
            StagedFile::Owned(file) => file.as_file(),
            StagedFile::Registered { handle, .. } => handle,
        }
    }

    pub(crate) fn verify(&self) -> Result<(), OpError> {
        let path = local(self.path())?;
        if path != self.path()
            || FileIdentity::at(&path).map_err(failed)? != self.identity
            || FileIdentity::of(self.opened()).map_err(failed)? != self.identity
        {
            return Err(refused("stale", "The retained audio changed; choose the source again."));
        }
        Ok(())
    }
}

pub struct Artifacts {
    root: PathBuf,
    records: RefCell<Arc<Records>>,
    publishing: Mutex<()>,
    #[cfg(test)]
    before_enqueue: RefCell<Option<Box<dyn FnOnce()>>>,
}
impl Artifacts {
    pub fn new(state_dir: &Path) -> Result<Self, OpError> {
        let state_dir = std::fs::canonicalize(state_dir).map_err(failed)?;
        let root = state_dir.join("artifacts");
        // Creating a missing directory is safe; repairing an existing one before validation could chmod a
        // symlink's outside target. The enclosing state folder is already owner-only.
        let builder = std::fs::DirBuilder::new();
        #[cfg(unix)]
        let builder = {
            use std::os::unix::fs::DirBuilderExt;
            let mut builder = builder;
            builder.mode(0o700);
            builder
        };
        match builder.create(&root) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(failed(error)),
        }
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
        Ok(Self {
            root, records: RefCell::new(Arc::new(records)), publishing: Mutex::new(()),
            #[cfg(test)]
            before_enqueue: RefCell::new(None),
        })
    }

    /// Copy through an opened regular file, bounded by its initial length. A growing/changing file is not a snapshot.
    pub(crate) async fn stage(
        &self,
        ctx: &Ctx,
        source: &Path,
        expected: Option<&Value>,
        signal: &Signal,
    ) -> Result<StagedArtifact, OpError> {
        check(signal)?;
        let source = ctx.approved(source)?;
        self.copy(source, expected.cloned(), signal).await
    }

    pub(crate) fn stage_registered(&self, ctx: &Ctx, id: &str, signal: &Signal) -> Result<StagedArtifact, OpError> {
        check(signal)?;
        let path = self.resolve(ctx, id)?;
        let handle = open_regular(&path).map_err(failed)?;
        let identity = FileIdentity::of(&handle).map_err(failed)?;
        if identity.size > MAX_BYTES {
            return Err(refused("too_large", "This artifact exceeds the 50 MiB media limit."));
        }
        Ok(StagedArtifact { file: StagedFile::Registered { path, handle }, id: id.to_owned(), identity })
    }

    /// Native-only promotion of one call's owned scratch. A model's `file` source cannot use this grant.
    pub async fn stage_scratch(&self, ctx: &Ctx, scratch: &Scratch, source: &Path, signal: &Signal) -> Result<StagedArtifact, OpError> {
        check(signal)?;
        let root = local(scratch.path())?;
        let source = local(source)?;
        if root != scratch.path() || !root.starts_with(ctx.paths().state_dir.join("scratch")) || !source.starts_with(&root) {
            return Err(refused("outside_root", "This file does not belong to this call's native scratch."));
        }
        self.copy(source, None, signal).await
    }

    async fn copy(&self, source: PathBuf, expected: Option<Value>, signal: &Signal) -> Result<StagedArtifact, OpError> {
        let root = self.checked_root()?;
        let signal = signal.clone();
        tokio::task::spawn_blocking(move || {
            check(&signal)?;
            let mut input = open_regular(&source).map_err(failed)?;
            let source_identity = FileIdentity::of(&input).map_err(failed)?;
            if expected.as_ref().is_some_and(|expected| !source_identity.matches(expected)) {
                return Err(refused("stale", "The selected file changed before it could be copied; choose it again."));
            }
            if source_identity.size > MAX_BYTES {
                return Err(refused("too_large", "This file exceeds the 50 MiB media limit; choose a shorter excerpt."));
            }
            let extension = source.extension().and_then(|part| part.to_str()).filter(|extension| {
                !extension.is_empty() && extension.len() <= 12 && extension.bytes().all(|byte| byte.is_ascii_alphanumeric())
            });
            let suffix = format!(".{}", extension.unwrap_or("bin"));
            let mut file = tempfile::Builder::new().prefix("a-").rand_bytes(24).suffix(&suffix).tempfile_in(root).map_err(failed)?;
            let mut left = source_identity.size;
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
            if source_identity != FileIdentity::of(&input).map_err(failed)? {
                return Err(refused("changed", "The source file changed while Kumi copied it; export it again first."));
            }
            check(&signal)?;
            file.as_file().sync_all().map_err(failed)?;
            let id = file.path().file_stem().and_then(|name| name.to_str()).ok_or_else(|| failed("invalid artifact name"))?.to_owned();
            let identity = FileIdentity::of(file.as_file()).map_err(failed)?;
            Ok(StagedArtifact { file: StagedFile::Owned(file), id, identity })
        })
        .await
        .map_err(failed)?
    }

    pub async fn add(
        &self,
        ctx: &Ctx,
        source: &Path,
        expected: Option<&Value>,
        kind: &str,
        mime: &str,
        tool: &str,
        signal: &Signal,
    ) -> Result<SavedArtifact, OpError> {
        self.publish(ctx, self.stage(ctx, source, expected, signal).await?, kind, mime, tool, signal).await
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
        let [saved] = self.publish_files(ctx, [(file, kind, mime)], tool, signal, None).await?;
        Ok(saved)
    }

    /// Current and reference audio share one manifest commit: no half-published comparison on an error.
    pub async fn publish_pair(
        &self,
        ctx: &Ctx,
        source: StagedArtifact,
        source_mime: &str,
        reference: Option<(StagedArtifact, &str)>,
        signal: &Signal,
        deadline: Option<tokio::time::Instant>,
    ) -> Result<(SavedArtifact, Option<SavedArtifact>), OpError> {
        match reference {
            Some((reference, reference_mime)) => {
                let [source, reference] = self.publish_files(
                    ctx, [(source, "audio", source_mime), (reference, "audio", reference_mime)], "listen", signal, deadline,
                ).await?;
                Ok((source, Some(reference)))
            }
            None => {
                let [source] = self.publish_files(ctx, [(source, "audio", source_mime)], "listen", signal, deadline).await?;
                Ok((source, None))
            }
        }
    }

    async fn publish_files<const N: usize>(
        &self,
        ctx: &Ctx,
        files: [(StagedArtifact, &str, &str); N],
        tool: &str,
        signal: &Signal,
        deadline: Option<tokio::time::Instant>,
    ) -> Result<[SavedArtifact; N], OpError> {
        let preparation = async {
            let publishing = tokio::select! {
                guard = self.publishing.lock() => guard,
                _ = signal.cancelled() => return Err(cancelled()),
            };
            check(signal)?;
            let root = self.checked_root()?;
            let at = now_ms();
            let mut added = Vec::with_capacity(N);
            for (file, kind, mime) in &files {
                if file.path().parent() != Some(root.as_path()) {
                    return Err(refused("outside_root", "This staged file does not belong to this artifact store."));
                }
                file.verify()?;
                match &file.file {
                    StagedFile::Owned(_) => {
                        if self.records.borrow().contains_key(&file.id) {
                            return Err(refused("artifact_exists", "An artifact already has this id."));
                        }
                        added.push((
                            file.id.clone(),
                            Record { path: file.path().to_path_buf(), kind: (*kind).into(), mime: (*mime).into(), tool: tool.into(), at },
                        ));
                    }
                    StagedFile::Registered { .. } => {
                        if self.resolve(ctx, &file.id)? != file.path() {
                            return Err(refused("stale", "The retained audio is no longer the registered artifact."));
                        }
                    }
                }
            }
            let saved = std::array::from_fn(|i| SavedArtifact { id: files[i].0.id.clone(), path: files[i].0.path().to_path_buf() });
            if added.is_empty() {
                return Ok((publishing, saved, None));
            }
            let existing = self.records.borrow().clone();
            let (document, added) = tokio::task::spawn_blocking(move || -> Result<_, OpError> {
                let mut document = serde_json::to_value(existing.as_ref()).map_err(failed)?;
                let fields = document.as_object_mut().ok_or_else(|| failed("invalid artifact manifest"))?;
                for (id, record) in &added {
                    fields.insert(id.clone(), serde_json::to_value(record).map_err(failed)?);
                }
                Ok((document, added))
            }).await.map_err(failed)??;
            Ok((publishing, saved, Some((document, added))))
        };
        let expired = || refused("retention_timeout", "The capture deadline expired before retaining its audio.");
        let (_publishing, saved, prepared) = match deadline {
            Some(deadline) => tokio::time::timeout_at(deadline, preparation).await.map_err(|_| expired())??,
            None => preparation.await?,
        };
        check(signal)?;
        if deadline.is_some_and(|deadline| tokio::time::Instant::now() >= deadline) {
            return Err(expired());
        }
        let Some((document, added)) = prepared else { return Ok(saved) };
        // `keep` happens before the manifest can refer to a file. A failed commit drops only these owned copies.
        let mut retained = Vec::with_capacity(N);
        for (file, _, _) in files {
            if let StagedFile::Owned(file) = file.file {
                let (handle, path) = file.keep().map_err(|error| failed(error.error))?;
                retained.push(PendingFile { handle: Some(handle), path, committed: false });
            }
        }
        #[cfg(test)]
        if let Some(before) = self.before_enqueue.borrow_mut().take() {
            before();
        }
        // This is the commit boundary: no await from these checks through enqueue.
        check(signal)?;
        if deadline.is_some_and(|deadline| tokio::time::Instant::now() >= deadline) {
            return Err(expired());
        }
        let queued = ctx.snapshots().enqueue("artifacts", document, Some(MAX_MANIFEST_BYTES as usize)).map_err(failed)?;
        // Once queued, a forcibly dropped caller cannot know whether the writer committed. Keep those copies rather
        // than deleting files a manifest may now name; a known write failure can still roll the whole group back.
        for file in &mut retained {
            file.committed = true;
        }
        if let Err(error) = queued.wait().await {
            for file in &mut retained {
                file.committed = false;
            }
            return Err(failed(error));
        }
        let mut records = self.records.borrow_mut();
        let records = Arc::make_mut(&mut records);
        for (id, record) in added {
            records.insert(id, record);
        }
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
pub fn approved<'a>(
    path: &Path,
    library: &[PathBuf],
    mut media: impl Iterator<Item = &'a PathBuf>,
    artifacts: &Artifacts,
) -> Result<PathBuf, OpError> {
    let path = local(path)?;
    if !path.is_file()
        || !(library.iter().any(|root| path.starts_with(root)) || media.any(|root| path.starts_with(root)) || artifacts.registered(&path))
    {
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
pub(crate) fn open_regular(path: &Path) -> io::Result<File> {
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
                    hermes_home: None,
                    hermes_root: None,
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
        let current = ctx.artifacts().stage(ctx, &source, None, &signal).await.unwrap();
        let other = ctx.artifacts().stage(ctx, &reference, None, &signal).await.unwrap();
        let paths = [current.path().to_path_buf(), other.path().to_path_buf()];
        // A directory cannot be replaced by the manifest's regular file, on any supported platform.
        std::fs::create_dir(ctx.paths().state_dir.join("artifacts.json")).unwrap();
        let result = ctx.artifacts().publish_pair(ctx, current, "audio/wav", Some((other, "audio/wav")), &signal, None).await;
        assert!(matches!(result, Err(OpError::Failed { .. })));
        assert!(paths.iter().all(|path| !path.exists()));
        assert!(ctx.artifacts().records.borrow().is_empty());
        assert_eq!(std::fs::read(source).unwrap(), b"current audio");
        assert_eq!(std::fs::read(reference).unwrap(), b"reference audio");
    }

    #[tokio::test(start_paused = true)]
    async fn expiry_at_the_commit_boundary_removes_every_unpublished_copy() {
        let fixture = Fixture::new();
        let ctx = &fixture.ctx;
        let source = fixture.root.join("capture.wav");
        std::fs::write(&source, b"captured audio").unwrap();
        let signal = Signal::new();
        let staged = ctx.artifacts().stage(ctx, &source, None, &signal).await.unwrap();
        let copy = staged.path().to_path_buf();
        // Keep paused time explicit while the manifest is prepared on a worker.
        let clock = tokio::spawn(async { loop { tokio::task::yield_now().await; } });
        let reached = Rc::new(std::cell::Cell::new(false));
        let marked = reached.clone();
        *ctx.artifacts().before_enqueue.borrow_mut() = Some(Box::new(move || {
            marked.set(true);
            futures::executor::block_on(tokio::time::advance(std::time::Duration::from_secs(2)));
        }));
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(1);
        let result = ctx.artifacts().publish_pair(ctx, staged, "audio/wav", None, &signal, Some(deadline)).await;
        clock.abort();
        assert!(reached.get(), "expiry must occur after preparation, at the final enqueue gate");
        assert!(matches!(result, Err(OpError::Refused { code: "retention_timeout", .. })));
        ctx.snapshots().flush();
        assert!(!copy.exists());
        assert!(!ctx.paths().state_dir.join("artifacts.json").exists());
        assert!(ctx.artifacts().records.borrow().is_empty());
        assert_eq!(std::fs::read(source).unwrap(), b"captured audio");
    }

    #[tokio::test]
    async fn cancellation_after_enqueue_waits_for_one_real_committed_artifact() {
        let fixture = Fixture::new();
        let ctx = &fixture.ctx;
        let source = fixture.root.join("capture.wav");
        std::fs::write(&source, b"captured audio").unwrap();
        let signal = Signal::new();
        let staged = ctx.artifacts().stage(ctx, &source, None, &signal).await.unwrap();
        let (entered, release) = ctx.snapshots().pause_next_commit("artifacts");
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(30);
        let (result, ()) = tokio::join!(
            ctx.artifacts().publish_pair(ctx, staged, "audio/wav", None, &signal, Some(deadline)),
            async {
                entered.await.unwrap();
                signal.cancel();
                tokio::task::yield_now().await;
                assert!(!ctx.paths().state_dir.join("artifacts.json").exists());
                release.send(()).unwrap();
            }
        );
        let (saved, reference) = result.unwrap();
        assert!(reference.is_none());
        assert_eq!(ctx.artifacts().resolve(ctx, &saved.id).unwrap(), saved.path);
        assert_eq!(std::fs::read(&saved.path).unwrap(), b"captured audio");
        assert_eq!(ctx.artifacts().records.borrow().len(), 1);
        assert_eq!(std::fs::read_dir(&ctx.artifacts().root).unwrap().count(), 1);
        let committed = std::fs::read(ctx.paths().state_dir.join("artifacts.json")).unwrap();
        ctx.snapshots().flush();
        assert_eq!(std::fs::read(ctx.paths().state_dir.join("artifacts.json")).unwrap(), committed);
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
        let result = fixture.ctx.artifacts().stage(&fixture.ctx, &source, None, &Signal::new()).await;
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

    #[cfg(unix)]
    #[test]
    fn a_planted_artifact_directory_link_never_repairs_its_outside_targets_permissions() {
        use std::os::unix::fs::PermissionsExt;
        let fixture = Fixture::new();
        let outside = fixture.root.join("outside");
        std::fs::create_dir(&outside).unwrap();
        std::fs::set_permissions(&outside, std::fs::Permissions::from_mode(0o751)).unwrap();
        std::fs::write(outside.join("keep.wav"), b"outside data").unwrap();
        let root = fixture.ctx.paths().state_dir.join("artifacts");
        std::fs::remove_dir(&root).unwrap();
        std::os::unix::fs::symlink(&outside, &root).unwrap();
        assert!(matches!(Artifacts::new(&fixture.ctx.paths().state_dir), Err(OpError::Refused { code: "outside_root", .. })));
        assert_eq!(std::fs::metadata(&outside).unwrap().permissions().mode() & 0o777, 0o751);
        assert_eq!(std::fs::read(outside.join("keep.wav")).unwrap(), b"outside data");
    }

    #[tokio::test]
    async fn opened_source_identity_must_match_the_selected_sound() {
        let fixture = Fixture::new();
        let source = fixture.root.join("selected.wav");
        std::fs::write(&source, b"selected audio").unwrap();
        let approved = File::open(&source).unwrap();
        let modified = approved.metadata().unwrap().modified().unwrap();
        let expected = file_identity(&approved).unwrap();
        drop(approved);
        let selected = fixture.ctx.artifacts().stage(&fixture.ctx, &source, Some(&expected), &Signal::new()).await.unwrap();
        assert_eq!(std::fs::read(selected.path()).unwrap(), b"selected audio");
        drop(selected);
        let mut other_device = expected.clone();
        other_device["dev"] = Value::from(expected["dev"].as_u64().unwrap() ^ 1);
        assert!(matches!(
            fixture.ctx.artifacts().stage(&fixture.ctx, &source, Some(&other_device), &Signal::new()).await,
            Err(OpError::Refused { code: "stale", .. })
        ));
        std::fs::rename(&source, fixture.root.join("old.wav")).unwrap();
        std::fs::write(&source, b"replaced audio").unwrap();
        let replacement = OpenOptions::new().write(true).open(&source).unwrap();
        replacement.set_times(std::fs::FileTimes::new().set_modified(modified)).unwrap();
        drop(replacement);
        let result =
            fixture.ctx.artifacts().add(&fixture.ctx, &source, Some(&expected), "audio", "audio/wav", "change", &Signal::new()).await;
        assert!(matches!(result, Err(OpError::Refused { code: "stale", .. })));
        assert!(fixture.ctx.artifacts().records.borrow().is_empty());
        assert_eq!(std::fs::read(source).unwrap(), b"replaced audio");
        assert_eq!(std::fs::read_dir(&fixture.ctx.artifacts().root).unwrap().count(), 0);
    }
}
