//! Approved media becomes a stable private copy before it is measured. Only validated copies enter C3's manifest.

use super::ctx::{Ctx, OpError, Scratch};
use kumi_common::{abort::Signal, time::now_ms};
use kumi_library::plugins::preset::review::ReviewError;
pub use kumi_library::plugins::preset::review::{PresetMetadata, ReviewedPreset as VerifiedPreset};
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    audio_origin: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    preset: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    identity: Option<Value>,
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
    source_identity: FileIdentity,
    audio_origin: Option<Value>,
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

    pub(crate) fn source_identity(&self) -> FileIdentity {
        self.source_identity
    }

    pub(crate) fn audio_origin(&self) -> Option<&Value> {
        self.audio_origin.as_ref()
    }

    /// Replayed artifacts keep their original provenance, even when measured again.
    pub(crate) fn set_audio_origin(&mut self, origin: Value) {
        if matches!(&self.file, StagedFile::Owned(_)) {
            self.audio_origin = Some(origin);
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

/// An unpublished output owned by this store. DSP receives only a cloned handle, never an output pathname.
pub(crate) struct AudioStage {
    file: NamedTempFile,
}

impl AudioStage {
    pub(crate) fn writer(&self) -> Result<File, OpError> {
        self.file.as_file().try_clone().map_err(failed)
    }

    /// The DSP writer has already flushed and synced the completed WAV.
    pub(crate) fn finish(self, source_identity: FileIdentity) -> Result<StagedArtifact, OpError> {
        let identity = FileIdentity::of(self.file.as_file()).map_err(failed)?;
        if identity.size > MAX_BYTES {
            return Err(refused("too_large", "This excerpt exceeds the 50 MiB media limit; choose a shorter window."));
        }
        let id = self.file.path().file_stem().and_then(|name| name.to_str()).ok_or_else(|| failed("invalid artifact name"))?.to_owned();
        Ok(StagedArtifact { file: StagedFile::Owned(self.file), id, identity, source_identity, audio_origin: None })
    }
}

pub struct Artifacts {
    root: PathBuf,
    records: RefCell<Arc<Records>>,
    publishing: Mutex<()>,
    #[cfg(test)]
    before_enqueue: RefCell<Option<futures::future::LocalBoxFuture<'static, ()>>>,
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
            root,
            records: RefCell::new(Arc::new(records)),
            publishing: Mutex::new(()),
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
        let audio_origin = self.records.borrow().get(id).and_then(|record| record.audio_origin.clone());
        Ok(StagedArtifact {
            file: StagedFile::Registered { path, handle },
            id: id.to_owned(),
            identity,
            source_identity: identity,
            audio_origin,
        })
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

    pub(crate) fn audio_stage(&self) -> Result<AudioStage, OpError> {
        let file = tempfile::Builder::new().prefix("a-").rand_bytes(24).suffix(".wav").tempfile_in(self.checked_root()?).map_err(failed)?;
        Ok(AudioStage { file })
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
            Ok(StagedArtifact { file: StagedFile::Owned(file), id, identity, source_identity, audio_origin: None })
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

    /// Only the initialized, verified writer uses this publication path. Ordinary media cannot attach preset metadata.
    pub async fn publish_preset(
        &self,
        ctx: &Ctx,
        preset: VerifiedPreset,
        signal: &Signal,
    ) -> Result<(SavedArtifact, PresetMetadata), OpError> {
        check(signal)?;
        if preset.bytes.len() as u64 > MAX_BYTES {
            return Err(refused("too_large", "The preset exceeds the 50 MiB artifact limit."));
        }
        let root = self.checked_root()?;
        let own_signal = signal.clone();
        let (staged, metadata) = tokio::task::spawn_blocking(move || {
            check(&own_signal)?;
            preset.metadata.verify(&preset.bytes).map_err(preset_error)?;
            let mut file = tempfile::Builder::new()
                .prefix("a-")
                .rand_bytes(24)
                .suffix(".vstpreset")
                .make_in(root, private_preset_file)
                .map_err(failed)?;
            file.write_all(&preset.bytes).map_err(failed)?;
            file.as_file().sync_all().map_err(failed)?;
            check(&own_signal)?;
            let id = file.path().file_stem().and_then(|name| name.to_str()).ok_or_else(|| failed("invalid artifact name"))?.to_owned();
            let identity = FileIdentity::of(file.as_file()).map_err(failed)?;
            Ok::<_, OpError>((
                StagedArtifact { file: StagedFile::Owned(file), id, identity, source_identity: identity, audio_origin: None },
                preset.metadata,
            ))
        })
        .await
        .map_err(failed)??;
        let receipt = serde_json::to_value(&metadata).map_err(failed)?;
        let [saved] =
            self.publish_files(ctx, [(staged, "preset", "application/x-vstpreset", Some(receipt))], "preset_build", signal, None).await?;
        Ok((saved, metadata))
    }

    /// Return the exact verified bytes, never a path that a later caller would reopen.
    pub async fn verified_preset(&self, ctx: &Ctx, id: &str, signal: &Signal) -> Result<VerifiedPreset, OpError> {
        check(signal)?;
        let path = self.resolve(ctx, id)?;
        let (metadata, expected) = {
            let records = self.records.borrow();
            let record = records.get(id).ok_or_else(|| refused("missing_artifact", "The preset artifact is no longer registered."))?;
            if record.kind != "preset" || record.tool != "preset_build" || record.mime != "application/x-vstpreset" {
                return Err(refused("invalid_source", "Use a preset artifact made by Kumi's verified preset writer."));
            }
            let metadata =
                record.preset.as_ref().ok_or_else(|| refused("invalid_source", "This artifact has no verified preset receipt."))?;
            let metadata =
                PresetMetadata::deserialize(metadata).map_err(|_| refused("invalid_source", "The preset receipt is invalid."))?;
            let expected = record.identity.clone().ok_or_else(|| refused("invalid_source", "The preset has no retained file identity."))?;
            (metadata, expected)
        };
        let signal = signal.clone();
        tokio::task::spawn_blocking(move || {
            check(&signal)?;
            let mut file = open_regular(&path).map_err(|_| refused("changed", "The preset file changed; build it again."))?;
            check_private_preset(&file)?;
            let identity = FileIdentity::of(&file).map_err(failed)?;
            if !identity.matches(&expected) || identity.size > MAX_BYTES {
                return Err(refused("changed", "The preset file changed; build it again."));
            }
            let mut bytes = Vec::with_capacity(identity.size as usize);
            let mut left = identity.size;
            let mut buffer = [0u8; 64 * 1024];
            while left > 0 {
                check(&signal)?;
                let wanted = left.min(buffer.len() as u64) as usize;
                let got = file.read(&mut buffer[..wanted]).map_err(failed)?;
                if got == 0 {
                    return Err(refused("changed", "The preset changed while Kumi read it; build it again."));
                }
                bytes.extend_from_slice(&buffer[..got]);
                left -= got as u64;
            }
            if FileIdentity::of(&file).map_err(failed)? != identity || FileIdentity::at(&path).map_err(failed)? != identity {
                return Err(refused("changed", "The preset changed while Kumi read it; build it again."));
            }
            metadata.verify(&bytes).map_err(|error| match error {
                ReviewError::TooLong => preset_error(error),
                _ => refused("changed", "The preset bytes or their receipt changed; build it again."),
            })?;
            check(&signal)?;
            Ok(VerifiedPreset { metadata, bytes })
        })
        .await
        .map_err(failed)?
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
        let [saved] = self.publish_files(ctx, [(file, kind, mime, None)], tool, signal, None).await?;
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
                let [source, reference] = self
                    .publish_files(
                        ctx,
                        [(source, "audio", source_mime, None), (reference, "audio", reference_mime, None)],
                        "listen",
                        signal,
                        deadline,
                    )
                    .await?;
                Ok((source, Some(reference)))
            }
            None => {
                let [source] = self.publish_files(ctx, [(source, "audio", source_mime, None)], "listen", signal, deadline).await?;
                Ok((source, None))
            }
        }
    }

    /// One current and up to five references use the same transaction engine as ordinary media.
    /// The old fixed-size paths keep their allocation and serialized-record behavior.
    pub(crate) async fn publish_audio_group(
        &self,
        ctx: &Ctx,
        files: Vec<(StagedArtifact, &str)>,
        signal: &Signal,
        deadline: Option<tokio::time::Instant>,
    ) -> Result<Vec<SavedArtifact>, OpError> {
        macro_rules! publish {
            ($count:literal) => {{
                let files: [(StagedArtifact, &str); $count] = files.try_into().map_err(|_| failed("invalid audio group"))?;
                let saved =
                    self.publish_files(ctx, files.map(|(file, mime)| (file, "audio", mime, None)), "listen", signal, deadline).await?;
                Ok(saved.into_iter().collect())
            }};
        }
        match files.len() {
            1 => publish!(1),
            2 => publish!(2),
            3 => publish!(3),
            4 => publish!(4),
            5 => publish!(5),
            6 => publish!(6),
            _ => Err(failed("Listening retains one current and at most five references.")),
        }
    }

    async fn publish_files<const N: usize>(
        &self,
        ctx: &Ctx,
        mut files: [(StagedArtifact, &str, &str, Option<Value>); N],
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
            for (file, kind, mime, preset) in &mut files {
                if file.path().parent() != Some(root.as_path()) {
                    return Err(refused("outside_root", "This staged file does not belong to this artifact store."));
                }
                file.verify()?;
                match &file.file {
                    StagedFile::Owned(_) => {
                        if self.records.borrow().contains_key(&file.id) {
                            return Err(refused("artifact_exists", "An artifact already has this id."));
                        }
                        let identity = preset.as_ref().map(|_| file.identity.value());
                        added.push((
                            file.id.clone(),
                            Record {
                                path: file.path().to_path_buf(),
                                kind: (*kind).into(),
                                mime: (*mime).into(),
                                tool: tool.into(),
                                at,
                                audio_origin: file.audio_origin.take(),
                                preset: preset.take(),
                                identity,
                            },
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
            })
            .await
            .map_err(failed)??;
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
        for (file, _, _, _) in files {
            if let StagedFile::Owned(file) = file.file {
                let (handle, path) = file.keep().map_err(|error| failed(error.error))?;
                retained.push(PendingFile { handle: Some(handle), path, committed: false });
            }
        }
        #[cfg(test)]
        {
            let before = self.before_enqueue.borrow_mut().take();
            if let Some(before) = before {
                before.await;
            }
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

fn preset_error(error: ReviewError) -> OpError {
    let code = if matches!(error, ReviewError::TooLong) { "too_long" } else { "invalid_preset" };
    OpError::Refused { code, reason: error.to_string(), owner: None }
}

fn private_preset_file(path: &Path) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.read(true).write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600).custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        // Generic read/write plus WRITE_DAC, so the newly created handle can protect its own ACL without a path reopen.
        options.access_mode(0xc004_0000).share_mode(windows_sys::Win32::Storage::FileSystem::FILE_SHARE_READ);
    }
    let file = options.open(path)?;
    #[cfg(windows)]
    if let Err(error) = daw_bridge::lifecycle::native_acl::protect_created_file(&file, &file) {
        drop(file);
        let _ = std::fs::remove_file(path);
        return Err(io::Error::other(error));
    }
    Ok(file)
}

fn check_private_preset(file: &File) -> Result<(), OpError> {
    #[cfg(unix)]
    let private = {
        use std::os::unix::fs::MetadataExt;
        let metadata = file.metadata().map_err(failed)?;
        metadata.mode() & 0o7777 == 0o600 && metadata.uid() == unsafe { libc::geteuid() } && metadata.nlink() == 1
    };
    #[cfg(windows)]
    let private =
        daw_bridge::lifecycle::native_acl::is_kumis_shape_handle(file, false) && handle_info(file).map_err(failed)?.nNumberOfLinks == 1;
    if !private {
        return Err(refused("changed", "The preset isn't an owner-only, unlinked artifact anymore; build it again."));
    }
    Ok(())
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

    async fn staged_group(fixture: &Fixture) -> (Vec<(StagedArtifact, &'static str)>, Vec<PathBuf>, Vec<PathBuf>) {
        let mut files = Vec::with_capacity(6);
        let mut copies = Vec::with_capacity(6);
        let mut originals = Vec::with_capacity(6);
        for index in 0..6 {
            let path = fixture.root.join(format!("audio-{index}.wav"));
            std::fs::write(&path, format!("audio {index}")).unwrap();
            let file = fixture.ctx.artifacts().stage(&fixture.ctx, &path, None, &Signal::new()).await.unwrap();
            copies.push(file.path().to_path_buf());
            originals.push(path);
            files.push((file, "audio/wav"));
        }
        (files, copies, originals)
    }

    fn reviewed() -> VerifiedPreset {
        use kumi_library::plugins::{
            formats::structure,
            preset::{review, Edit, WriteOptions},
        };
        let plugin = "serum-2";
        let version = structure::builtin(plugin).unwrap().verified.writes[0].version.clone();
        review::build(
            plugin,
            &WriteOptions { version, class_id: None, name: Some("Private patch".into()) },
            &[Edit { path: "Oscillator1/plainParams/kParamVolume".into(), value: serde_json::json!(0.45) }],
        )
        .unwrap()
    }

    #[tokio::test]
    async fn a_reference_group_preserves_verified_presets_and_cannot_claim_their_receipts() {
        let fixture = Fixture::new();
        let ctx = &fixture.ctx;
        let signal = Signal::new();
        let (preset, receipt) = ctx.artifacts().publish_preset(ctx, reviewed(), &signal).await.unwrap();
        let bytes = std::fs::read(&preset.path).unwrap();
        let (mut files, _, _) = staged_group(&fixture).await;
        let origin = serde_json::json!({
            "source": {"kind": "file", "name": "Reference.wav"},
            "window": {"start_frame": 48000, "frame_count": 48000, "sample_rate": 48000, "channels": 2, "replay_from": 0}
        });
        files[0].0.audio_origin = Some(origin.clone());
        let audio = ctx.artifacts().publish_audio_group(ctx, files, &signal, None).await.unwrap();
        let reopened = Artifacts::new(&ctx.paths().state_dir).unwrap();
        let verified = reopened.verified_preset(ctx, &preset.id, &signal).await.unwrap();
        assert_eq!(verified.metadata, receipt);
        assert_eq!(verified.bytes, bytes);
        let records: Value = serde_json::from_slice(&std::fs::read(ctx.paths().state_dir.join("artifacts.json")).unwrap()).unwrap();
        for (index, saved) in audio.iter().enumerate() {
            let record = &records[&saved.id];
            assert!(record.get("preset").is_none());
            assert!(record.get("identity").is_none());
            assert_eq!(record.get("audio_origin"), (index == 0).then_some(&origin));
            assert_eq!(reopened.verified_preset(ctx, &saved.id, &signal).await.unwrap_err().code(), "invalid_source");
        }
        let replay = reopened.stage_registered(ctx, &audio[0].id, &signal).unwrap();
        assert_eq!(replay.audio_origin, Some(origin));
        assert!(records[&preset.id].get("audio_origin").is_none());
    }

    #[tokio::test]
    async fn verified_presets_survive_reload_but_an_import_cannot_claim_the_writer() {
        let fixture = Fixture::new();
        let ctx = &fixture.ctx;
        let signal = Signal::new();
        let (saved, receipt) = ctx.artifacts().publish_preset(ctx, reviewed(), &signal).await.unwrap();
        let reopened = Artifacts::new(&ctx.paths().state_dir).unwrap();
        let verified = reopened.verified_preset(ctx, &saved.id, &signal).await.unwrap();
        assert_eq!(verified.metadata, receipt);
        assert_eq!(verified.bytes, std::fs::read(&saved.path).unwrap());
        assert_eq!(verified.metadata.settings[0].after, serde_json::json!(0.45));
        let source = fixture.root.join("foreign.vstpreset");
        std::fs::write(&source, &verified.bytes).unwrap();
        let fake = ctx.artifacts().add(ctx, &source, None, "preset", "application/x-vstpreset", "preset_build", &signal).await.unwrap();
        assert_eq!(ctx.artifacts().verified_preset(ctx, &fake.id, &signal).await.unwrap_err().code(), "invalid_source");
        assert_eq!(std::fs::read(source).unwrap(), verified.bytes);
    }

    #[tokio::test]
    async fn preset_consumers_refuse_replaced_modified_or_multiply_linked_files() {
        let fixture = Fixture::new();
        let ctx = &fixture.ctx;
        let signal = Signal::new();
        let (saved, _) = ctx.artifacts().publish_preset(ctx, reviewed(), &signal).await.unwrap();
        let original = std::fs::read(&saved.path).unwrap();
        let stamp = File::open(&saved.path).unwrap().metadata().unwrap().modified().unwrap();
        let replacement = ctx.artifacts().root.join("replacement");
        std::fs::write(&replacement, &original).unwrap();
        File::options().write(true).open(&replacement).unwrap().set_times(std::fs::FileTimes::new().set_modified(stamp)).unwrap();
        std::fs::remove_file(&saved.path).unwrap();
        std::fs::rename(replacement, &saved.path).unwrap();
        assert_eq!(ctx.artifacts().verified_preset(ctx, &saved.id, &signal).await.unwrap_err().code(), "changed");

        let (saved, _) = ctx.artifacts().publish_preset(ctx, reviewed(), &signal).await.unwrap();
        OpenOptions::new().append(true).open(&saved.path).unwrap().write_all(b"changed").unwrap();
        assert_eq!(ctx.artifacts().verified_preset(ctx, &saved.id, &signal).await.unwrap_err().code(), "changed");

        let (saved, _) = ctx.artifacts().publish_preset(ctx, reviewed(), &signal).await.unwrap();
        std::fs::hard_link(&saved.path, fixture.root.join("linked.vstpreset")).unwrap();
        assert_eq!(ctx.artifacts().verified_preset(ctx, &saved.id, &signal).await.unwrap_err().code(), "changed");
    }

    #[tokio::test]
    async fn preset_publication_failure_leaves_no_unregistered_file() {
        let fixture = Fixture::new();
        let ctx = &fixture.ctx;
        std::fs::create_dir(ctx.paths().state_dir.join("artifacts.json")).unwrap();
        assert!(matches!(ctx.artifacts().publish_preset(ctx, reviewed(), &Signal::new()).await, Err(OpError::Failed { .. })));
        assert!(ctx.artifacts().records.borrow().is_empty());
        assert_eq!(std::fs::read_dir(&ctx.artifacts().root).unwrap().count(), 0);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_preset_with_public_permissions_is_not_a_verified_source() {
        use std::os::unix::fs::PermissionsExt;
        let fixture = Fixture::new();
        let ctx = &fixture.ctx;
        let signal = Signal::new();
        let (saved, _) = ctx.artifacts().publish_preset(ctx, reviewed(), &signal).await.unwrap();
        std::fs::set_permissions(&saved.path, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert_eq!(ctx.artifacts().verified_preset(ctx, &saved.id, &signal).await.unwrap_err().code(), "changed");
    }

    #[tokio::test]
    async fn failed_manifest_commit_removes_the_whole_reference_group_not_user_files() {
        let fixture = Fixture::new();
        let ctx = &fixture.ctx;
        let (files, copies, originals) = staged_group(&fixture).await;
        // A directory cannot be replaced by the manifest's regular file, on any supported platform.
        std::fs::create_dir(ctx.paths().state_dir.join("artifacts.json")).unwrap();
        let result = ctx.artifacts().publish_audio_group(ctx, files, &Signal::new(), None).await;
        assert!(matches!(result, Err(OpError::Failed { .. })));
        assert!(copies.iter().all(|path| !path.exists()));
        assert!(ctx.artifacts().records.borrow().is_empty());
        for (index, original) in originals.iter().enumerate() {
            assert_eq!(std::fs::read(original).unwrap(), format!("audio {index}").as_bytes());
        }
    }

    #[tokio::test]
    async fn expiry_at_the_commit_boundary_removes_every_unpublished_copy() {
        let fixture = Fixture::new();
        let ctx = &fixture.ctx;
        let (files, copies, originals) = staged_group(&fixture).await;
        let signal = Signal::new();
        let (entered, at_boundary) = tokio::sync::oneshot::channel();
        let (release, proceed) = tokio::sync::oneshot::channel();
        *ctx.artifacts().before_enqueue.borrow_mut() = Some(Box::pin(async move {
            entered.send(()).unwrap();
            proceed.await.unwrap();
        }));
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(60);
        let (result, ()) = tokio::join!(ctx.artifacts().publish_audio_group(ctx, files, &signal, Some(deadline)), async {
            at_boundary.await.unwrap();
            tokio::time::pause();
            tokio::time::advance(std::time::Duration::from_secs(61)).await;
            release.send(()).unwrap();
        });
        tokio::time::resume();
        assert!(matches!(result, Err(OpError::Refused { code: "retention_timeout", .. })));
        ctx.snapshots().flush();
        assert!(copies.iter().all(|path| !path.exists()));
        assert!(!ctx.paths().state_dir.join("artifacts.json").exists());
        assert!(ctx.artifacts().records.borrow().is_empty());
        for (index, original) in originals.iter().enumerate() {
            assert_eq!(std::fs::read(original).unwrap(), format!("audio {index}").as_bytes());
        }
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
        let (result, ()) = tokio::join!(ctx.artifacts().publish_pair(ctx, staged, "audio/wav", None, &signal, Some(deadline)), async {
            entered.await.unwrap();
            signal.cancel();
            tokio::task::yield_now().await;
            assert!(!ctx.paths().state_dir.join("artifacts.json").exists());
            release.send(()).unwrap();
        });
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
