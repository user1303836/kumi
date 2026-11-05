//! CC-13 upload approval: content hash without copying, then exactly that private copy.

use super::artifacts::{file_identity, open_regular, StagedArtifact, MAX_BYTES};
use super::ctx::{Ctx, OpError};
use kumi_common::abort::{self, Signal};
use kumi_media::audio::decode::{open_audio_to_with_options, DecodeOptions};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::Duration;

const PREVIEW_LIMIT: Duration = Duration::from_secs(5);

pub struct Pin {
    source: Value,
    path: PathBuf,
    identity: Value,
    pub digest: String,
    pub bytes: u64,
    pub name: String,
}

impl Pin {
    pub fn source(&self) -> &Value {
        &self.source
    }

    pub fn preview(&self) -> Value {
        json!({"state": "preview", "upload": {"file": self.name, "bytes": self.bytes, "identity_sha256": self.digest}})
    }

    /// Recheck permission, revision, identity and content. Never upload a newly substituted file.
    pub async fn current(&self, ctx: &Ctx, signal: &Signal) -> Result<(), OpError> {
        let found = super::import::find(ctx, &self.source, signal).await.map_err(safe_error)?;
        if found.path() != self.path || found.named != self.identity {
            return Err(stale());
        }
        let (digest, bytes) = hash(self.path.clone(), Some(self.identity.clone()), signal.clone()).await?;
        if digest != self.digest || bytes != self.bytes {
            return Err(stale());
        }
        ctx.approved(&self.path).map_err(safe_error)?;
        Ok(())
    }

    /// All source kinds are copied for upload, including registered artifacts. The SDK sees only
    /// this still-unpublished, call-owned stage; the producer's original is never moved or changed.
    pub async fn stage(&self, ctx: &Ctx, signal: &Signal) -> Result<StagedArtifact, OpError> {
        let found = super::import::find(ctx, &self.source, signal).await.map_err(safe_error)?;
        if found.path() != self.path || found.named != self.identity {
            return Err(stale());
        }
        let stage = ctx.artifacts().stage(ctx, &self.path, Some(&self.identity), signal).await.map_err(safe_error)?;
        let (digest, bytes) = hash(stage.path().to_path_buf(), None, signal.clone()).await?;
        if digest != self.digest || bytes != self.bytes {
            return Err(stale());
        }
        stage.verify().map_err(safe_error)?;
        ctx.approved(&self.path).map_err(safe_error)?;
        Ok(stage)
    }
}

/// Five seconds including resolution/hash, inside the host hook's total fifteen-second budget.
/// Timeout also signals a blocking reader to stop at its next bounded chunk; it never starts a copy.
pub async fn preview(ctx: &Ctx, source: &Value, signal: &Signal) -> Result<Pin, OpError> {
    let deadline = Signal::new();
    let combined = abort::any([signal.clone(), deadline.clone()]);
    let result = tokio::time::timeout(PREVIEW_LIMIT, async {
        check(&combined)?;
        let found = super::import::find(ctx, source, &combined).await.map_err(safe_error)?;
        let (digest, bytes) = hash(found.path().to_path_buf(), Some(found.named.clone()), combined.clone()).await?;
        ctx.approved(found.path()).map_err(safe_error)?;
        // Display only. Authorization is the original structured source, open-file identity and hash.
        let name = super::change::one_line(&found.name, 120).replace(['/', '\\', ':'], "_");
        if name.is_empty() {
            return Err(invalid("The selected audio has no displayable file name."));
        }
        Ok(Pin { source: source.clone(), path: found.path().to_path_buf(), identity: found.named, digest, bytes, name })
    })
    .await;
    deadline.cancel();
    result.map_err(|_| invalid("Reading the selected audio exceeded the upload-preview time limit."))?
}

async fn hash(path: PathBuf, expected: Option<Value>, signal: Signal) -> Result<(String, u64), OpError> {
    tokio::task::spawn_blocking(move || {
        check(&signal)?;
        let mut file = open_regular(&path).map_err(|_| invalid("The selected audio could not be opened safely."))?;
        let before = file_identity(&file).map_err(|_| invalid("The selected audio identity could not be read."))?;
        if expected.as_ref().is_some_and(|expected| expected != &before) {
            return Err(stale());
        }
        let size = file.metadata().map_err(|_| invalid("The selected audio size could not be read."))?.len();
        if size == 0 || size > MAX_BYTES {
            return Err(invalid("Choose a nonempty audio file no larger than 50 MiB."));
        }
        let mut digest = Sha256::new();
        let mut total = 0_u64;
        let mut buffer = [0_u8; 64 * 1024];
        loop {
            check(&signal)?;
            let count = file.read(&mut buffer).map_err(|_| invalid("The selected audio could not be read."))?;
            if count == 0 {
                break;
            }
            total += count as u64;
            if total > MAX_BYTES {
                return Err(stale());
            }
            digest.update(&buffer[..count]);
        }
        if total != size || file_identity(&file).map_err(|_| stale())? != before {
            return Err(stale());
        }
        Ok((format!("{:x}", digest.finalize()), total))
    })
    .await
    .map_err(|_| invalid("The upload-preview reader failed."))?
}

/// Read duration from the approved bytes, not a model argument. No FFT or model is needed. The
/// decoder's 721-second fence must never become a shorter estimate for a longer uploaded file.
pub async fn seconds(ctx: &Ctx, source: &StagedArtifact, signal: &Signal) -> Result<f64, OpError> {
    check(signal)?;
    source.verify().map_err(safe_error)?;
    let mut audio = open_audio_to_with_options(
        source.path(),
        Some(signal.clone()),
        Some(721.0),
        &DecodeOptions { installed_only: true, ffmpeg: ctx.media().tool("ffmpeg").map(Path::to_path_buf) },
    )
    .await
    .map_err(|_| invalid("The selected audio could not be measured; compressed audio needs the approved decoder."))?;
    let measured = audio.frames as f64 / audio.sample_rate;
    // A full PCM length is authoritative even beyond the decoder fence. For a converted copy
    // without a known full length, accept only EOF clearly before that fence, never the prefix.
    let seconds = audio.length().or_else(|| (measured <= 720.0).then_some(measured));
    audio.close().await.map_err(|_| invalid("The selected audio could not be closed after measurement."))?;
    source.verify().map_err(safe_error)?;
    check(signal)?;
    let seconds = seconds.ok_or_else(|| {
        invalid("The approved decoder could not measure the entire file; export a WAV or a shorter clip before uploading.")
    })?;
    if !seconds.is_finite() || seconds <= 0.0 || seconds > 1800.0 {
        return Err(invalid("Mirelo audio-to-MIDI accepts audio up to thirty minutes; choose a supported clip."));
    }
    Ok(seconds)
}

fn check(signal: &Signal) -> Result<(), OpError> {
    if signal.is_cancelled() {
        Err(OpError::Refused { code: "cancelled", reason: "Audio upload preparation was cancelled.".into(), owner: None })
    } else {
        Ok(())
    }
}
fn stale() -> OpError {
    OpError::Refused {
        code: "stale",
        reason: "The selected audio changed or its approval no longer applies; approve a new quote.".into(),
        owner: None,
    }
}
fn invalid(reason: &str) -> OpError {
    OpError::Refused { code: "invalid_source", reason: reason.into(), owner: None }
}
fn safe_error(error: OpError) -> OpError {
    match error.code() {
        "stale" => stale(),
        "cancelled" => OpError::Refused { code: "cancelled", reason: "Audio upload preparation was cancelled.".into(), owner: None },
        "outside_root" => OpError::Refused {
            code: "outside_root",
            reason: "Choose audio in a currently approved folder or a registered artifact.".into(),
            owner: None,
        },
        _ => invalid("The selected audio is not an available approved CC-11 source."),
    }
}
