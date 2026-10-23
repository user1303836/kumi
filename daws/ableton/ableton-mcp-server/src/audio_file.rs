//! Reading and removing the one WAV file a capture owns, with the identity fences the TypeScript
//! kept: a fresh regular file with one link, opened without following symlinks, re-checked by
//! device and inode after every step, and quarantined before it is unlinked.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use kumi_common::js::string;
use kumi_common::time::now_ms;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::fs::{self, File, OpenOptions};
use tokio::io::AsyncReadExt;

pub const MAX_CAPTURE_FILE_BYTES: u64 = 32 * 1024 * 1024;
pub const MAX_CAPTURE_SECONDS: f64 = 12.0;
pub const MAX_CAPTURE_CHANNELS: usize = 2;

const COMPANION_MAX_BYTES: u64 = 4 * 1024 * 1024;

/// What the capture code throws: its message, or the file system's error (whose `NotFound` is the
/// `ENOENT` the TypeScript let pass).
#[derive(Debug, thiserror::Error)]
pub enum AudioFileError {
    #[error("{0}")]
    Message(String),
    #[error("{0}")]
    Io(#[from] std::io::Error),
}

impl AudioFileError {
    fn message(text: &str) -> Self {
        AudioFileError::Message(text.to_string())
    }

    /// `(cause as NodeJS.ErrnoException).code === "ENOENT"`.
    pub fn is_not_found(&self) -> bool {
        matches!(self, AudioFileError::Io(cause) if cause.kind() == std::io::ErrorKind::NotFound)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileIdentity {
    pub size: u64,
    pub mtime_ms: f64,
    pub birthtime_ms: f64,
    pub dev: u64,
    pub ino: u64,
    pub nlink: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OwnedFile {
    pub real_path: String,
    pub sha256: String,
    pub stat: FileIdentity,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CaptureFormat {
    WavPcm,
    WavFloat,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DecodedCaptureFile {
    pub samples: Vec<f32>,
    pub sample_rate: f64,
    pub channels: usize,
    pub duration_seconds: f64,
    pub format: CaptureFormat,
    pub bits_per_sample: u32,
    pub sha256: String,
    pub byte_length: usize,
    pub basename: String,
    pub real_path: String,
    pub stat: FileIdentity,
    pub companions: Vec<OwnedFile>,
}

/// The part of a decoded capture its removal needs (`Pick<DecodedCaptureFile, ...>`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OwnedCaptureFile {
    pub real_path: String,
    pub sha256: String,
    pub stat: FileIdentity,
    pub companions: Vec<OwnedFile>,
}

impl DecodedCaptureFile {
    pub fn owned(&self) -> OwnedCaptureFile {
        OwnedCaptureFile {
            real_path: self.real_path.clone(),
            sha256: self.sha256.clone(),
            stat: self.stat.clone(),
            companions: self.companions.clone(),
        }
    }
}

struct OpenCandidate {
    original_path: PathBuf,
    handle: File,
    stat: FileIdentity,
}

fn system_time_ms(time: Option<SystemTime>) -> f64 {
    time.and_then(|time| time.duration_since(UNIX_EPOCH).ok()).map_or(0.0, |duration| duration.as_secs_f64() * 1000.0)
}

#[cfg(unix)]
fn identity(metadata: &std::fs::Metadata) -> FileIdentity {
    use std::os::unix::fs::MetadataExt;
    FileIdentity {
        size: metadata.len(),
        mtime_ms: metadata.mtime() as f64 * 1000.0 + metadata.mtime_nsec() as f64 / 1e6,
        birthtime_ms: system_time_ms(metadata.created().ok()),
        dev: metadata.dev(),
        ino: metadata.ino(),
        nlink: metadata.nlink(),
    }
}

#[cfg(not(unix))]
fn identity(metadata: &std::fs::Metadata) -> FileIdentity {
    // TS: Node reports dev, ino and nlink on Windows too; stable Rust does not, so a file there is
    // identified by size and times alone.
    FileIdentity {
        size: metadata.len(),
        mtime_ms: system_time_ms(metadata.modified().ok()),
        birthtime_ms: system_time_ms(metadata.created().ok()),
        dev: 0,
        ino: 0,
        nlink: 1,
    }
}

fn same_identity(left: &FileIdentity, right: &FileIdentity, include_times: bool) -> bool {
    left.dev == right.dev
        && left.ino == right.ino
        && left.nlink == right.nlink
        && left.size == right.size
        && (!include_times || left.mtime_ms == right.mtime_ms)
}

/// `path.extname`: the last extension of the base name, or nothing for a name that is only dots or
/// starts with its only dot.
fn extname(path: &str) -> String {
    let base = Path::new(path).file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
    if base.chars().all(|character| character == '.') {
        return String::new();
    }
    match base.rfind('.') {
        Some(index) if index > 0 => base[index..].to_string(),
        _ => String::new(),
    }
}

fn within(root: &Path, candidate: &Path) -> bool {
    candidate.starts_with(root)
}

fn with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut name: OsString = path.as_os_str().to_owned();
    name.push(suffix);
    PathBuf::from(name)
}

fn path_text(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

fn sha256_hex(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

async fn allowed_roots(project_file_path: &Path) -> Result<Vec<PathBuf>, AudioFileError> {
    let project_directory = fs::canonicalize(project_file_path.parent().unwrap_or(Path::new("."))).await?;
    let mut roots = vec![project_directory.clone()];
    // Live Sets directly under User Library/Projects conventionally place new
    // recordings under the narrow Samples/Recorded subtree. Never authorize the
    // entire User Library namespace.
    let directory_name = project_directory.file_name().map(|name| name.to_string_lossy().to_lowercase()).unwrap_or_default();
    if directory_name == "projects" {
        let recorded = project_directory.parent().unwrap_or(&project_directory).join("Samples").join("Recorded");
        match fs::canonicalize(&recorded).await {
            Ok(path) => roots.push(path),
            Err(cause) if cause.kind() == std::io::ErrorKind::NotFound => {}
            Err(cause) => return Err(cause.into()),
        }
    }
    Ok(roots)
}

async fn open_verified_candidate(
    file_path: &Path,
    maximum_bytes: u64,
    capture_started_at_ms: f64,
    label: &str,
    writable: bool,
) -> Result<OpenCandidate, AudioFileError> {
    let before = fs::symlink_metadata(file_path).await?;
    let before_identity = identity(&before);
    if !before.is_file()
        || before.file_type().is_symlink()
        || before_identity.nlink != 1
        || before_identity.size < (if label == "media" { 44 } else { 0 })
        || before_identity.size > maximum_bytes
    {
        return Err(AudioFileError::Message(format!("capture {label} must be one fresh bounded regular file")));
    }
    // mtime is the cross-platform write freshness signal. Birth time cannot be
    // rewritten by utimes on Windows and would let an old untouched file appear
    // fresh merely because it was copied/created recently.
    if before_identity.mtime_ms < capture_started_at_ms - 5_000.0 {
        return Err(AudioFileError::Message(format!("capture {label} predates the authorized capture lifecycle")));
    }
    let mut options = OpenOptions::new();
    options.read(true);
    if writable {
        options.write(true);
    }
    #[cfg(unix)]
    {
        options.custom_flags(libc::O_NOFOLLOW);
    }
    let handle = options.open(file_path).await?;
    let opened = handle.metadata().await?;
    let path_stat = fs::symlink_metadata(file_path).await?;
    let opened_identity = identity(&opened);
    if !opened.is_file() || opened.file_type().is_symlink() || !same_identity(&opened_identity, &identity(&path_stat), true) {
        return Err(AudioFileError::Message(format!("capture {label} identity changed while it was opened")));
    }
    Ok(OpenCandidate { original_path: file_path.to_path_buf(), handle, stat: opened_identity })
}

/// `bytes.toString("ascii", start, start + expected.length) === expected`: Node's ASCII decoding
/// drops each byte's high bit.
fn ascii_equals(bytes: &[u8], start: usize, expected: &[u8]) -> bool {
    bytes.get(start..start + expected.len()).is_some_and(|slice| slice.iter().zip(expected).all(|(byte, want)| byte & 0x7f == *want))
}

fn u16_le(bytes: &[u8], offset: usize) -> u32 {
    u16::from_le_bytes([bytes[offset], bytes[offset + 1]]) as u32
}

fn u32_le(bytes: &[u8], offset: usize) -> u64 {
    u32::from_le_bytes([bytes[offset], bytes[offset + 1], bytes[offset + 2], bytes[offset + 3]]) as u64
}

fn int24_le(bytes: &[u8], offset: usize) -> i32 {
    let unsigned = bytes[offset] as i32 | (bytes[offset + 1] as i32) << 8 | (bytes[offset + 2] as i32) << 16;
    if unsigned & 0x80_0000 != 0 {
        unsigned - 0x100_0000
    } else {
        unsigned
    }
}

async fn read_all(handle: &mut File) -> Result<Vec<u8>, AudioFileError> {
    let mut bytes = Vec::new();
    handle.read_to_end(&mut bytes).await?;
    Ok(bytes)
}

pub async fn decode_owned_wave_file(
    file_path: &str,
    project_file_path: &str,
    capture_started_at_ms: f64,
) -> Result<DecodedCaptureFile, AudioFileError> {
    let path = Path::new(file_path);
    if !path.is_absolute() || string::utf16_len(file_path) > 4_096 || extname(file_path).to_lowercase() != ".wav" {
        return Err(AudioFileError::message("capture provider currently accepts only one absolute bounded WAV media file"));
    }
    let requested = fs::symlink_metadata(path).await?;
    if !requested.is_file() || requested.file_type().is_symlink() {
        return Err(AudioFileError::message("capture media must be one fresh bounded regular file"));
    }
    let actual_path = fs::canonicalize(path).await?;
    let roots = allowed_roots(Path::new(project_file_path)).await?;
    if !roots.iter().any(|root| within(root, &actual_path)) {
        return Err(AudioFileError::message("capture media path is outside the saved Live project boundary"));
    }

    let mut media = open_verified_candidate(&actual_path, MAX_CAPTURE_FILE_BYTES, capture_started_at_ms, "media", false).await?;
    let mut companion =
        match open_verified_candidate(&with_suffix(&actual_path, ".asd"), COMPANION_MAX_BYTES, capture_started_at_ms, "companion", false)
            .await
        {
            Ok(candidate) => Some(candidate),
            Err(cause) if cause.is_not_found() => None,
            Err(cause) => return Err(cause),
        };
    let bytes = read_all(&mut media.handle).await?;
    let after = identity(&media.handle.metadata().await?);
    if !same_identity(&media.stat, &after, true) {
        return Err(AudioFileError::message("capture media changed while it was acquired"));
    }
    if !ascii_equals(&bytes, 0, b"RIFF") || !ascii_equals(&bytes, 8, b"WAVE") {
        return Err(AudioFileError::message("capture media is not a supported RIFF/WAVE file"));
    }

    let mut format: Option<u32> = None;
    let mut channels: Option<u32> = None;
    let mut sample_rate: Option<u64> = None;
    let mut block_align: Option<u32> = None;
    let mut bits_per_sample: Option<u32> = None;
    let mut data_offset: Option<usize> = None;
    let mut data_length: Option<u64> = None;
    let mut offset = 12usize;
    while offset + 8 <= bytes.len() {
        let length = u32_le(&bytes, offset + 4);
        let start = offset + 8;
        let end = start as u64 + length;
        if end > bytes.len() as u64 {
            return Err(AudioFileError::message("capture WAV contains a truncated chunk"));
        }
        if ascii_equals(&bytes, offset, b"fmt ") {
            if length < 16 {
                return Err(AudioFileError::message("capture WAV format chunk is too short"));
            }
            format = Some(u16_le(&bytes, start));
            channels = Some(u16_le(&bytes, start + 2));
            sample_rate = Some(u32_le(&bytes, start + 4));
            block_align = Some(u16_le(&bytes, start + 12));
            bits_per_sample = Some(u16_le(&bytes, start + 14));
            if format == Some(0xfffe) && length >= 40 {
                format = Some(u16_le(&bytes, start + 24));
            }
        } else if ascii_equals(&bytes, offset, b"data") && data_offset.is_none() {
            data_offset = Some(start);
            data_length = Some(length);
        }
        offset = (end + length % 2) as usize;
    }
    let (Some(format), Some(channels), Some(sample_rate), Some(block_align), Some(bits_per_sample), Some(data_offset), Some(data_length)) =
        (format, channels, sample_rate, block_align, bits_per_sample, data_offset, data_length)
    else {
        return Err(AudioFileError::message("capture WAV format or data is unsupported"));
    };
    if format != 1 && format != 3 {
        return Err(AudioFileError::message("capture WAV format or data is unsupported"));
    }
    if channels < 1 || channels as usize > MAX_CAPTURE_CHANNELS || !(8_000..=384_000).contains(&sample_rate) {
        return Err(AudioFileError::message("capture WAV channel count or sample rate is outside bounds"));
    }
    if bits_per_sample % 8 != 0 {
        return Err(AudioFileError::message("capture WAV sample packing is unsupported"));
    }
    let expected_bytes = bits_per_sample / 8;
    if ![2, 3, 4].contains(&expected_bytes) || block_align != channels * expected_bytes || data_length % block_align as u64 != 0 {
        return Err(AudioFileError::message("capture WAV sample packing is unsupported"));
    }
    if format == 3 && bits_per_sample != 32 {
        return Err(AudioFileError::message("capture WAV float format must be 32-bit"));
    }
    if format == 1 && ![16, 24, 32].contains(&bits_per_sample) {
        return Err(AudioFileError::message("capture WAV PCM width is unsupported"));
    }
    let frame_count = data_length / block_align as u64;
    let duration_seconds = frame_count as f64 / sample_rate as f64;
    if !(duration_seconds > 0.0) || duration_seconds > MAX_CAPTURE_SECONDS || frame_count * channels as u64 > 10_000_000 {
        return Err(AudioFileError::message("capture WAV duration exceeds the ephemeral analysis bound"));
    }
    let sample_count = (frame_count * channels as u64) as usize;
    let mut samples = Vec::with_capacity(sample_count);
    for index in 0..sample_count {
        let offset = data_offset + index * expected_bytes as usize;
        let value: f64 = if format == 3 {
            f32::from_le_bytes([bytes[offset], bytes[offset + 1], bytes[offset + 2], bytes[offset + 3]]) as f64
        } else if bits_per_sample == 16 {
            i16::from_le_bytes([bytes[offset], bytes[offset + 1]]) as f64 / 0x8000 as f64
        } else if bits_per_sample == 24 {
            int24_le(&bytes, offset) as f64 / 0x80_0000 as f64
        } else {
            i32::from_le_bytes([bytes[offset], bytes[offset + 1], bytes[offset + 2], bytes[offset + 3]]) as f64 / 0x8000_0000u32 as f64
        };
        if !value.is_finite() || !(-1.0..=1.0).contains(&value) {
            return Err(AudioFileError::message("capture WAV contains non-finite or non-normalized samples"));
        }
        samples.push(value as f32);
    }
    let mut companions = Vec::new();
    if let Some(companion) = companion.as_mut() {
        let companion_bytes = read_all(&mut companion.handle).await?;
        let companion_after = identity(&companion.handle.metadata().await?);
        if !same_identity(&companion.stat, &companion_after, true) {
            return Err(AudioFileError::message("capture companion changed while it was acquired"));
        }
        companions.push(OwnedFile {
            real_path: path_text(&companion.original_path),
            sha256: sha256_hex(&companion_bytes),
            stat: companion_after,
        });
    }
    Ok(DecodedCaptureFile {
        samples,
        sample_rate: sample_rate as f64,
        channels: channels as usize,
        duration_seconds,
        format: if format == 3 { CaptureFormat::WavFloat } else { CaptureFormat::WavPcm },
        bits_per_sample,
        sha256: sha256_hex(&bytes),
        byte_length: bytes.len(),
        basename: actual_path.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default(),
        real_path: path_text(&actual_path),
        stat: after,
        companions,
    })
}

pub async fn capture_media_is_absent(file_path: &str, project_file_path: &str) -> Result<bool, AudioFileError> {
    let path = Path::new(file_path);
    if !path.is_absolute() || string::utf16_len(file_path) > 4_096 || extname(file_path).to_lowercase() != ".wav" {
        return Err(AudioFileError::message("capture absence check requires one absolute WAV path"));
    }
    let parent = fs::canonicalize(path.parent().unwrap_or(Path::new("."))).await?;
    let candidate = parent.join(path.file_name().unwrap_or_default());
    let roots = allowed_roots(Path::new(project_file_path)).await?;
    if !roots.iter().any(|root| within(root, &candidate)) {
        return Err(AudioFileError::message("capture absence path is outside the saved Live project boundary"));
    }
    for path in [candidate.clone(), with_suffix(&candidate, ".asd")] {
        match fs::symlink_metadata(&path).await {
            Ok(_) => return Ok(false),
            Err(cause) if cause.kind() == std::io::ErrorKind::NotFound => {}
            Err(cause) => return Err(cause.into()),
        }
    }
    // A process killed between quarantine rename and unlink leaves this private
    // marker directory. Treat it as residual, never as proof that raw media is
    // gone merely because the original pathname is absent.
    let mut entries = fs::read_dir(&parent).await?;
    while let Some(entry) = entries.next_entry().await? {
        if entry.file_type().await?.is_dir() && entry.file_name().to_string_lossy().starts_with(".ableton-mcp-capture-") {
            return Ok(false);
        }
    }
    Ok(true)
}

/// `mkdtemp(prefix)`: a private folder with six random characters after the prefix.
async fn make_temporary_directory(prefix: &Path) -> Result<PathBuf, AudioFileError> {
    const ALPHABET: &[u8] = b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";
    loop {
        let suffix: String = (0..6).map(|_| ALPHABET[rand::random_range(0..ALPHABET.len())] as char).collect();
        let path = with_suffix(prefix, &suffix);
        let mut builder = fs::DirBuilder::new();
        #[cfg(unix)]
        builder.mode(0o700);
        match builder.create(&path).await {
            Ok(()) => return Ok(path),
            Err(cause) if cause.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(cause) => return Err(cause.into()),
        }
    }
}

struct QuarantinedEntry {
    target: OwnedFile,
    handle: File,
    quarantine_path: Option<PathBuf>,
}

async fn quarantine_and_unlink_targets(targets: &[OwnedFile], anchor_path: &Path) -> Result<(), AudioFileError> {
    if targets.is_empty() || !targets.iter().all(|target| Path::new(&target.real_path).parent() == anchor_path.parent()) {
        return Err(AudioFileError::message("capture companion directory identity is invalid"));
    }
    let mut opened: Vec<QuarantinedEntry> = Vec::new();
    let mut quarantine_directory: Option<PathBuf> = None;
    let outcome = quarantine_targets(targets, anchor_path, &mut opened, &mut quarantine_directory).await;
    if outcome.is_err() {
        for entry in opened.iter().rev() {
            let Some(quarantine_path) = &entry.quarantine_path else { continue };
            // Preserve uncertain objects rather than deleting them.
            let Ok(moved) = fs::symlink_metadata(quarantine_path).await.as_ref().map(identity) else { continue };
            let Ok(descriptor) = entry.handle.metadata().await.as_ref().map(identity) else { continue };
            match fs::symlink_metadata(&entry.target.real_path).await {
                Ok(_) => continue,
                Err(missing) if missing.kind() == std::io::ErrorKind::NotFound => {}
                Err(_) => continue,
            }
            if moved.dev == descriptor.dev && moved.ino == descriptor.ino && descriptor.size > 0 {
                let _ = fs::rename(quarantine_path, &entry.target.real_path).await;
            }
        }
    }
    drop(opened);
    if let Some(directory) = quarantine_directory {
        // Preserve a non-empty uncertain quarantine.
        let _ = fs::remove_dir(&directory).await;
    }
    outcome
}

async fn quarantine_targets(
    targets: &[OwnedFile],
    anchor_path: &Path,
    opened: &mut Vec<QuarantinedEntry>,
    quarantine_directory: &mut Option<PathBuf>,
) -> Result<(), AudioFileError> {
    for target in targets {
        let companion = extname(&target.real_path).to_lowercase() == ".asd";
        let candidate = open_verified_candidate(
            Path::new(&target.real_path),
            if companion { COMPANION_MAX_BYTES } else { MAX_CAPTURE_FILE_BYTES },
            (target.stat.birthtime_ms - 1.0).max(0.0),
            if companion { "companion" } else { "media" },
            true,
        )
        .await?;
        opened.push(QuarantinedEntry { target: target.clone(), handle: candidate.handle, quarantine_path: None });
        let entry = opened.last_mut().expect("the entry just pushed");
        if !same_identity(&identity(&entry.handle.metadata().await?), &target.stat, true) {
            return Err(AudioFileError::message("capture media identity changed before unlink"));
        }
        let bytes = read_all(&mut entry.handle).await?;
        if sha256_hex(&bytes) != target.sha256 {
            return Err(AudioFileError::message("capture media digest changed before unlink"));
        }
    }
    let directory = make_temporary_directory(&anchor_path.parent().unwrap_or(Path::new(".")).join(".ableton-mcp-capture-")).await?;
    *quarantine_directory = Some(directory.clone());
    #[cfg(unix)]
    fs::set_permissions(&directory, std::os::unix::fs::PermissionsExt::from_mode(0o700)).await?;
    for (index, entry) in opened.iter_mut().enumerate() {
        let extension = if extname(&entry.target.real_path).to_lowercase() == ".asd" { ".asd" } else { ".wav" };
        let quarantine_path = directory.join(format!("capture-{index}{extension}"));
        fs::rename(&entry.target.real_path, &quarantine_path).await?;
        entry.quarantine_path = Some(quarantine_path.clone());
        if !same_identity(&identity(&entry.handle.metadata().await?), &identity(&fs::symlink_metadata(&quarantine_path).await?), true) {
            return Err(AudioFileError::message("capture media identity changed during private quarantine"));
        }
    }
    for entry in opened.iter() {
        entry.handle.set_len(0).await?;
    }
    for entry in opened.iter() {
        let Some(quarantine_path) = &entry.quarantine_path else {
            return Err(AudioFileError::message("capture quarantine is incomplete"));
        };
        let descriptor = identity(&entry.handle.metadata().await?);
        let path_stat = identity(&fs::symlink_metadata(quarantine_path).await?);
        if descriptor.dev != path_stat.dev || descriptor.ino != path_stat.ino || path_stat.nlink != 1 {
            return Err(AudioFileError::message("capture media path changed before unlink"));
        }
        fs::remove_file(quarantine_path).await?;
    }
    Ok(())
}

pub async fn unlink_owned_capture_file(file: &OwnedCaptureFile) -> Result<(), AudioFileError> {
    let mut targets: Vec<OwnedFile> =
        vec![OwnedFile { real_path: file.real_path.clone(), sha256: file.sha256.clone(), stat: file.stat.clone() }];
    targets.extend(file.companions.iter().cloned());
    let companion_path = with_suffix(Path::new(&file.real_path), ".asd");
    if !file.companions.iter().any(|companion| Path::new(&companion.real_path) == companion_path) {
        // Live may finish its analysis sidecar while the isolated DSP worker runs.
        // Discover it again immediately before the all-target identity fence.
        match open_verified_candidate(&companion_path, COMPANION_MAX_BYTES, (file.stat.birthtime_ms - 5_000.0).max(0.0), "companion", false)
            .await
        {
            Ok(mut late) => {
                let bytes = read_all(&mut late.handle).await?;
                let late_stat = identity(&late.handle.metadata().await?);
                if !same_identity(&late.stat, &late_stat, true) {
                    return Err(AudioFileError::message("capture companion changed during late acquisition"));
                }
                targets.push(OwnedFile { real_path: path_text(&late.original_path), sha256: sha256_hex(&bytes), stat: late_stat });
            }
            Err(cause) if cause.is_not_found() => {}
            Err(cause) => return Err(cause),
        }
    }
    quarantine_and_unlink_targets(&targets, Path::new(&file.real_path)).await
}

/// Removes the analysis sidecar Live may still write after the capture itself is gone, watching the
/// whole two-second window; the count removed is returned.
pub async fn unlink_late_capture_companions(file: &OwnedCaptureFile) -> Result<usize, AudioFileError> {
    let companion_path = with_suffix(Path::new(&file.real_path), ".asd");
    let mut removed = 0usize;
    let deadline = now_ms() + 2_000;
    // Observe the complete declared window. Three early ENOENT checks are not a
    // stable absence proof because Live can publish an ASD hundreds of
    // milliseconds after clip deletion.
    while now_ms() < deadline {
        match open_verified_candidate(&companion_path, COMPANION_MAX_BYTES, (file.stat.birthtime_ms - 5_000.0).max(0.0), "companion", false)
            .await
        {
            Ok(mut candidate) => {
                let bytes = read_all(&mut candidate.handle).await?;
                let current = identity(&candidate.handle.metadata().await?);
                if !same_identity(&candidate.stat, &current, true) {
                    return Err(AudioFileError::message("late capture companion changed while acquired"));
                }
                let target = OwnedFile { real_path: path_text(&companion_path), sha256: sha256_hex(&bytes), stat: current };
                drop(candidate);
                quarantine_and_unlink_targets(&[target], Path::new(&file.real_path)).await?;
                removed += 1;
            }
            Err(cause) if cause.is_not_found() => {}
            Err(cause) => return Err(cause),
        }
        let wait = (deadline - now_ms()).clamp(1, 100);
        tokio::time::sleep(Duration::from_millis(wait as u64)).await;
    }
    match fs::symlink_metadata(&companion_path).await {
        Ok(_) => Err(AudioFileError::message("capture companion appeared at the stable-absence deadline")),
        Err(cause) if cause.kind() == std::io::ErrorKind::NotFound => Ok(removed),
        Err(cause) => Err(cause.into()),
    }
}
