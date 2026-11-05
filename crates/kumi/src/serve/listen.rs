//! File listening measures an approved immutable copy, then retains exactly those bytes for playback.
//! Both sides of a comparison must measure successfully before either becomes a registered artifact.

use super::artifacts::StagedArtifact;
use super::ctx::{Call, Ctx, OpError, OpFuture, Operation};
use kumi_common::abort::Signal;
use kumi_media::audio::{self, Analysis, AnalyzeOptions, DecodeOptions};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

pub const LISTEN: Operation = Operation {
    name: "listen",
    description: "Measure approved audio by file, revision-scoped sound, retained artifact, or an explicitly approved bounded track window. Optionally compare it with a reference. Track captures restore their temporary DAW state before returning measurements.",
    schema: super::listen_track::schema,
    effect: super::listen_track::effect,
    run,
};

pub fn source_schema() -> Value {
    json!({
        "type": "object", "additionalProperties": false,
        "properties": {
            "file": {"type": "string", "minLength": 1},
            "sound_id": {"type": "string", "minLength": 1},
            "map_revision": {"type": "string", "minLength": 1},
            "artifact_id": {"type": "string", "minLength": 1}
        },
        "oneOf": [
            {"required": ["file"], "not": {"anyOf": [{"required": ["sound_id"]}, {"required": ["map_revision"]}, {"required": ["artifact_id"]}]}},
            {"required": ["sound_id", "map_revision"], "not": {"anyOf": [{"required": ["file"]}, {"required": ["artifact_id"]}]}},
            {"required": ["artifact_id"], "not": {"anyOf": [{"required": ["file"]}, {"required": ["sound_id"]}, {"required": ["map_revision"]}]}}
        ]
    })
}

pub fn file_schema() -> Value {
    json!({
        "type": "object", "additionalProperties": false, "required": ["source"],
        "properties": {"source": source_schema(), "compare_to": source_schema()}
    })
}

enum Source<'a> {
    File(&'a str),
    Sound { id: &'a str, revision: &'a str },
    Artifact(&'a str),
}

impl<'a> Source<'a> {
    fn parse(value: &'a Value) -> Result<Self, OpError> {
        let fields = value.as_object().ok_or_else(|| OpError::Invalid { reason: "An audio source must be an object.".into() })?;
        let string = |key| fields.get(key).and_then(Value::as_str).filter(|value| !value.trim().is_empty());
        match (fields.len(), string("file"), string("sound_id"), string("map_revision"), string("artifact_id")) {
            (1, Some(file), None, None, None) => Ok(Self::File(file)),
            (2, None, Some(id), Some(revision), None) => Ok(Self::Sound { id, revision }),
            (1, None, None, None, Some(id)) => Ok(Self::Artifact(id)),
            _ => Err(OpError::Invalid {
                reason: "Choose exactly one audio source: {file}, {sound_id, map_revision}, or {artifact_id}; no other source fields are accepted.".into(),
            }),
        }
    }

    async fn resolve(&self, ctx: &Ctx, signal: &Signal) -> Result<PathBuf, OpError> {
        check(signal)?;
        let path = match self {
            Self::File(file) => ctx.approved(Path::new(file))?,
            Self::Sound { id, revision } => ctx.maps().resolve(ctx, id, revision, signal).await?,
            Self::Artifact(id) => ctx.artifacts().resolve(ctx, id)?,
        };
        // Neither a saved artifact nor a map snapshot is itself a permission grant.
        ctx.approved(&path)
    }

    async fn stage(&self, ctx: &Ctx, signal: &Signal) -> Result<StagedArtifact, OpError> {
        match self {
            Self::File(file) => ctx.artifacts().stage(ctx, Path::new(file), None, signal).await,
            Self::Sound { id, revision } => ctx.maps().stage(ctx, id, revision, signal).await,
            Self::Artifact(id) => ctx.artifacts().stage_registered(ctx, id, signal),
        }
    }
}

/// Authorize a file, current map sound or registered artifact without staging it or running DSP.
pub async fn resolve_source(ctx: &Ctx, value: &Value, signal: &Signal) -> Result<PathBuf, OpError> {
    Source::parse(value)?.resolve(ctx, signal).await
}

/// Stage a structured source, retaining map identity checks and reusing registered files.
/// Guarded capture uses this for its authorized comparison before touching the DAW.
pub async fn stage_source(ctx: &Ctx, value: &Value, signal: &Signal) -> Result<StagedArtifact, OpError> {
    Source::parse(value)?.stage(ctx, signal).await
}

fn run(ctx: &Ctx, call: Call) -> OpFuture<'_> {
    if super::listen_track::is_track(&call.args) {
        return super::listen_track::run(ctx, call);
    }
    Box::pin(async move {
        check(&call.signal)?;
        if call.args.keys().any(|key| !matches!(key.as_str(), "source" | "compare_to")) {
            return Err(OpError::Invalid { reason: "listen accepts only source and optional compare_to.".into() });
        }
        let source = Source::parse(
            call.args.get("source").ok_or_else(|| OpError::Invalid { reason: "listen requires an audio source object.".into() })?,
        )?;
        let reference = call.args.get("compare_to").map(Source::parse).transpose()?;
        // Resolve both before making any copy or starting any DSP.
        let source_path = source.resolve(ctx, &call.signal).await?;
        let reference_path = match &reference {
            Some(reference) => Some(reference.resolve(ctx, &call.signal).await?),
            None => None,
        };
        let staged = source.stage(ctx, &call.signal).await?;
        let staged_reference = match &reference {
            Some(reference) => Some(reference.stage(ctx, &call.signal).await?),
            None => None,
        };
        let source = measure(ctx, staged, &source_path, &call.signal).await?;
        let reference = match (staged_reference, reference_path) {
            (Some(staged), Some(path)) => Some(measure(ctx, staged, &path, &call.signal).await?),
            _ => None,
        };
        retain(ctx, source, reference, &call.signal, None).await
    })
}

fn check(signal: &Signal) -> Result<(), OpError> {
    if signal.is_cancelled() {
        Err(OpError::Refused { code: "cancelled", reason: "Listening was cancelled.".into(), owner: None })
    } else {
        Ok(())
    }
}

/// A successful measurement owns the still-unpublished audio it describes. Dropping it cleans the copy.
/// Capture callers can use this same path after staging their approved scratch audio.
pub struct MeasuredAudio {
    file: StagedArtifact,
    pub metrics: Analysis,
    mime: &'static str,
}

/// Measure an owned immutable stage with the installed-only decoder policy. No model or DAW is involved.
/// `original_name` contributes only its basename to the canonical Analysis, never an absolute input path.
pub async fn measure(ctx: &Ctx, file: StagedArtifact, original_name: &Path, signal: &Signal) -> Result<MeasuredAudio, OpError> {
    check(signal)?;
    file.verify()?;
    let path = file.path().to_str().ok_or_else(|| OpError::Invalid { reason: "The staged audio path is not valid UTF-8.".into() })?;
    let measured = audio::hear(
        path,
        AnalyzeOptions {
            signal: Some(signal.clone()),
            decode: DecodeOptions { installed_only: true, ffmpeg: ctx.media().tool("ffmpeg").map(Path::to_path_buf) },
            ..Default::default()
        },
    )
    .await;
    check(signal)?;
    file.verify()?;
    let mut metrics = measured.map_err(|error| OpError::Failed { reason: error.to_string() })?;
    metrics.file = original_name
        .file_name()
        .ok_or_else(|| OpError::Invalid { reason: "Audio must have a file name.".into() })?
        .to_string_lossy()
        .into_owned();
    let mime = match metrics.format.as_str() {
        "wav" | "wave" => "audio/wav",
        "aif" | "aiff" | "aifc" => "audio/aiff",
        "mp3" => "audio/mpeg",
        "m4a" | "mp4" => "audio/mp4",
        "aac" => "audio/aac",
        "flac" => "audio/flac",
        "ogg" | "oga" | "opus" => "audio/ogg",
        "caf" => "audio/x-caf",
        format => return Err(OpError::Failed { reason: format!("No retained audio MIME type for validated format {format}.") }),
    };
    Ok(MeasuredAudio { file, metrics, mime })
}

/// Retain measured audio and its optional reference together, then produce the C1 result.
/// Differences always mean current minus reference; MEDIA always names the retained current audio.
pub async fn retain(
    ctx: &Ctx, source: MeasuredAudio, reference: Option<MeasuredAudio>, signal: &Signal, deadline: Option<tokio::time::Instant>,
) -> Result<Value, OpError> {
    check(signal)?;
    let differences = reference.as_ref().map(|reference| audio::compare(&source.metrics, &reference.metrics));
    let mut verdict_text = audio::tools::summary(&source.metrics);
    if let Some(differences) = &differences {
        for headline in &differences.headlines {
            verdict_text.push('\n');
            verdict_text.push_str(headline);
        }
    }
    let (reference_file, reference_metrics) = match reference {
        Some(reference) => (Some((reference.file, reference.mime)), Some(reference.metrics)),
        None => (None, None),
    };
    let (saved, saved_reference) = ctx.artifacts().publish_pair(ctx, source.file, source.mime, reference_file, signal, deadline).await?;
    let mut result = json!({
        "metrics": source.metrics,
        "artifact_id": saved.id,
        "verdict_text": verdict_text,
        "media_tag": format!("MEDIA:{}", saved.path.display()),
    });
    if let (Some(saved), Some(metrics), Some(differences)) = (saved_reference, reference_metrics, differences) {
        result["compare"] = json!({"artifact_id": saved.id, "metrics": metrics, "differences": differences});
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::super::ctx::{Host, Progress};
    use super::super::Args;
    use super::*;
    use kumi_media::audio::Comparison;
    use kumi_types::{daw::DawId, system::Env};
    use std::{collections::BTreeMap, rc::Rc};

    struct Fixture {
        _dir: tempfile::TempDir,
        root: PathBuf,
        ctx: Rc<Ctx>,
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
            Self { _dir: dir, root, ctx }
        }

        fn tone(&self, name: &str, hz: f64, gain: f64) -> (PathBuf, Vec<u8>) {
            let bytes = pcm(hz, gain);
            let path = self.root.join(name);
            std::fs::write(&path, &bytes).unwrap();
            (path, bytes)
        }

        async fn listen(&self, args: Value) -> Result<Value, OpError> {
            self.call(args, Signal::new()).await
        }

        async fn call(&self, args: Value, signal: Signal) -> Result<Value, OpError> {
            (LISTEN.run)(
                &self.ctx,
                Call { args: args.as_object().unwrap().clone(), host: Host::default(), signal, progress: Progress::none() },
            )
            .await
        }

        fn retained(&self, result: &Value) -> PathBuf {
            self.ctx.artifacts().resolve(&self.ctx, result["artifact_id"].as_str().unwrap()).unwrap()
        }

        fn artifact_files(&self) -> BTreeMap<PathBuf, Vec<u8>> {
            fn collect(path: &Path, files: &mut BTreeMap<PathBuf, Vec<u8>>) {
                if !path.exists() {
                    return;
                }
                for entry in std::fs::read_dir(path).unwrap() {
                    let path = entry.unwrap().path();
                    if path.is_dir() {
                        collect(&path, files);
                    } else {
                        files.insert(path.clone(), std::fs::read(path).unwrap());
                    }
                }
            }
            let mut files = BTreeMap::new();
            collect(&self.ctx.paths().state_dir.join("artifacts"), &mut files);
            files
        }
    }

    /// Two seconds of deterministic mono PCM16, with no decoder or fixture download involved.
    fn pcm(hz: f64, gain: f64) -> Vec<u8> {
        let rate = 48_000u32;
        let samples = rate * 2;
        let size = samples * 2;
        let mut bytes = Vec::with_capacity(size as usize + 44);
        bytes.extend_from_slice(b"RIFF");
        bytes.extend_from_slice(&(36 + size).to_le_bytes());
        bytes.extend_from_slice(b"WAVEfmt ");
        bytes.extend_from_slice(&16u32.to_le_bytes());
        bytes.extend_from_slice(&1u16.to_le_bytes());
        bytes.extend_from_slice(&1u16.to_le_bytes());
        bytes.extend_from_slice(&rate.to_le_bytes());
        bytes.extend_from_slice(&(rate * 2).to_le_bytes());
        bytes.extend_from_slice(&2u16.to_le_bytes());
        bytes.extend_from_slice(&16u16.to_le_bytes());
        bytes.extend_from_slice(b"data");
        bytes.extend_from_slice(&size.to_le_bytes());
        for frame in 0..samples {
            let value = (gain * (std::f64::consts::TAU * hz * frame as f64 / rate as f64).sin() * i16::MAX as f64).round() as i16;
            bytes.extend_from_slice(&value.to_le_bytes());
        }
        bytes
    }

    #[tokio::test]
    async fn comparison_measures_current_minus_reference_and_retains_both_originals() {
        let fixture = Fixture::new();
        let (source, source_bytes) = fixture.tone("current.wav", 750.0, 0.2);
        let (reference, reference_bytes) = fixture.tone("reference.wav", 750.0, 0.1);
        let result = fixture.listen(json!({"source": {"file": source}, "compare_to": {"file": reference}})).await.unwrap();
        let metrics: Analysis = serde_json::from_value(result["metrics"].clone()).unwrap();
        let reference_metrics: Analysis = serde_json::from_value(result["compare"]["metrics"].clone()).unwrap();
        let differences: Comparison = serde_json::from_value(result["compare"]["differences"].clone()).unwrap();
        let current_lufs = metrics.loudness.integrated_lufs.unwrap();
        let reference_lufs = reference_metrics.loudness.integrated_lufs.unwrap();
        assert!((current_lufs - reference_lufs - 6.02).abs() < 0.15, "{current_lufs} vs {reference_lufs}");
        assert!((differences.loudness.difference_lu.unwrap() - 6.0).abs() < 0.15);
        assert_eq!(differences.loudness.mine_lufs, Some(current_lufs));
        assert_eq!(differences.loudness.reference_lufs, Some(reference_lufs));
        assert_eq!((&*differences.mine, &*differences.reference), ("current.wav", "reference.wav"));
        assert!(result["verdict_text"].as_str().unwrap().contains("louder"));
        assert_eq!(metrics.sample_rate, 48_000.0);
        assert_eq!(metrics.channels, 1);
        assert_eq!(metrics.seconds, 2.0);
        assert_eq!(metrics.spectrogram.rows.len(), 10);
        assert!(result["metrics"]["stereo"].is_null());
        assert!(result["metrics"].get("sample_rate").is_none());
        assert_eq!(result.as_object().unwrap().len(), 5);
        assert_eq!(result["compare"].as_object().unwrap().len(), 3);
        assert!(result.get("comparison").is_none());
        let retained = fixture.retained(&result);
        let retained_reference = fixture.retained(&result["compare"]);
        assert_ne!(retained, source);
        assert_ne!(retained_reference, reference);
        assert_eq!(result["media_tag"], format!("MEDIA:{}", retained.display()));
        assert_eq!(std::fs::read(retained).unwrap(), source_bytes);
        assert_eq!(std::fs::read(retained_reference).unwrap(), reference_bytes);
        assert_eq!(std::fs::read(&source).unwrap(), source_bytes);
        assert_eq!(std::fs::read(&reference).unwrap(), reference_bytes);

        // Artifacts remain usable when the user's original disappears, and compare_to uses the same union.
        std::fs::remove_file(&source).unwrap();
        std::fs::remove_file(&reference).unwrap();
        let before_replay = fixture.artifact_files();
        let replay = fixture
            .listen(json!({
                "source": {"artifact_id": result["compare"]["artifact_id"]},
                "compare_to": {"artifact_id": result["artifact_id"]}
            }))
            .await
            .unwrap();
        assert!(replay["compare"]["differences"]["loudness"]["differenceLu"].as_f64().unwrap() < -5.8);
        assert_eq!(std::fs::read(fixture.retained(&replay)).unwrap(), reference_bytes);
        assert_eq!(replay["artifact_id"], result["compare"]["artifact_id"]);
        assert_eq!(replay["compare"]["artifact_id"], result["artifact_id"]);
        assert_eq!(fixture.artifact_files(), before_replay, "artifact replay must not create more retained copies");
    }

    #[tokio::test]
    async fn spectral_differences_describe_the_audio_not_the_argument_order_or_names() {
        let fixture = Fixture::new();
        let (low, _) = fixture.tone("bright-name.wav", 93.75, 0.2);
        let (high, _) = fixture.tone("bass-name.wav", 3000.0, 0.2);
        let result = fixture.listen(json!({"source": {"file": low}, "compare_to": {"file": high}})).await.unwrap();
        let differences: Comparison = serde_json::from_value(result["compare"]["differences"].clone()).unwrap();
        let bass = differences.balance.iter().find(|band| band.band == "bass").unwrap().difference;
        let presence = differences.balance.iter().find(|band| band.band == "presence").unwrap().difference;
        assert!(bass > 20.0, "bass difference: {bass}");
        assert!(presence < -20.0, "presence difference: {presence}");
        assert!(result["metrics"]["balance"]["centroidHz"].as_f64().unwrap() < 200.0);
        assert!(result["compare"]["metrics"]["balance"]["centroidHz"].as_f64().unwrap() > 2500.0);
    }

    #[tokio::test]
    async fn measurement_and_playback_share_the_staged_bytes_when_the_original_changes() {
        let fixture = Fixture::new();
        let (path, original) = fixture.tone("immutable.wav", 750.0, 0.2);
        let signal = Signal::new();
        let staged = fixture.ctx.artifacts().stage(&fixture.ctx, &path, None, &signal).await.unwrap();
        let replacement = pcm(3000.0, 0.8);
        std::fs::write(&path, &replacement).unwrap();
        let measured = measure(&fixture.ctx, staged, &path, &signal).await.unwrap();
        let result = retain(&fixture.ctx, measured, None, &signal).await.unwrap();
        assert!((result["metrics"]["loudness"]["samplePeakDbfs"].as_f64().unwrap() + 14.0).abs() < 0.2);
        assert_eq!(result["metrics"]["file"], "immutable.wav");
        assert!(result.get("compare").is_none());
        assert_eq!(result.as_object().unwrap().len(), 4);
        assert_eq!(std::fs::read(fixture.retained(&result)).unwrap(), original);
        assert_eq!(std::fs::read(path).unwrap(), replacement);
    }

    #[tokio::test]
    async fn invalid_unions_and_unapproved_paths_publish_nothing() {
        tokio::task::LocalSet::new()
            .run_until(async {
                let fixture = Fixture::new();
                let (path, original) = fixture.tone("valid.wav", 750.0, 0.2);
                let before = fixture.artifact_files();
                for args in [
                    json!({}),
                    json!({"file": path}),
                    json!({"source": null}),
                    json!({"source": {}}),
                    json!({"source": {"file": path, "artifact_id": "a"}}),
                    json!({"source": {"file": path, "sound_id": "s", "map_revision": "r"}}),
                    json!({"source": {"sound_id": "s"}}),
                    json!({"source": {"artifact_id": "a", "map_revision": "r"}}),
                    json!({"source": {"file": path, "extra": true}}),
                    json!({"source": {"file": path}, "extra": true}),
                    json!({"source": {"file": path}, "compare_to": null}),
                    json!({"source": {"file": path}, "compare_to": {"file": path, "artifact_id": "a"}}),
                    json!({"source": {"file": path}, "compare_to": {"sound_id": "s"}}),
                    json!({"source": {"file": path}, "compare_to": {"artifact_id": ""}}),
                    json!({"source": {"track": "1"}}),
                ] {
                    assert_eq!(fixture.listen(args.clone()).await.unwrap_err().code(), "invalid", "{args}");
                }
                let outside = fixture._dir.path().join("outside.wav");
                std::fs::write(&outside, &original).unwrap();
                assert!(fixture.listen(json!({"source": {"file": outside}})).await.is_err());
                assert!(fixture.listen(json!({"source": {"file": path}, "compare_to": {"file": outside}})).await.is_err());
                assert!(fixture.listen(json!({"source": {"artifact_id": "../outside.wav"}})).await.is_err());
                assert!(fixture.listen(json!({"source": {"sound_id": "no-such-sound", "map_revision": "old"}})).await.is_err());
                assert_eq!(fixture.artifact_files(), before);
                assert_eq!(std::fs::read(path).unwrap(), original);
                assert_eq!(std::fs::read(outside).unwrap(), original);
            })
            .await;
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_symlink_inside_a_root_does_not_grant_the_outside_audio() {
        let fixture = Fixture::new();
        let outside = fixture._dir.path().join("outside.wav");
        std::fs::write(&outside, pcm(750.0, 0.2)).unwrap();
        let link = fixture.root.join("escape.wav");
        std::os::unix::fs::symlink(&outside, &link).unwrap();
        let before = fixture.artifact_files();
        assert!(fixture.listen(json!({"source": {"file": link}})).await.is_err());
        assert_eq!(fixture.artifact_files(), before);
    }

    #[tokio::test]
    async fn a_failed_reference_discards_the_successfully_measured_source() {
        let fixture = Fixture::new();
        let (source, original) = fixture.tone("valid.wav", 750.0, 0.2);
        let broken = fixture.root.join("broken.wav");
        std::fs::write(&broken, b"not a wave file").unwrap();
        let before = fixture.artifact_files();
        let error = fixture.listen(json!({"source": {"file": source}, "compare_to": {"file": broken}})).await.unwrap_err();
        assert_eq!(error.code(), "failed");
        assert_eq!(fixture.artifact_files(), before);
        assert_eq!(std::fs::read(source).unwrap(), original);
        assert_eq!(std::fs::read(broken).unwrap(), b"not a wave file");
    }

    #[tokio::test]
    async fn cancellation_cleans_owned_stages_and_never_publishes_measured_audio() {
        let fixture = Fixture::new();
        let (path, original) = fixture.tone("cancel.wav", 750.0, 0.2);
        let before = fixture.artifact_files();
        let signal = Signal::new();
        signal.cancel();
        assert_eq!(fixture.call(json!({"source": {"file": path}}), signal).await.unwrap_err().code(), "cancelled");
        let signal = Signal::new();
        let staged = fixture.ctx.artifacts().stage(&fixture.ctx, &path, None, &signal).await.unwrap();
        let staged_path = staged.path().to_owned();
        signal.cancel();
        let error = match measure(&fixture.ctx, staged, &path, &signal).await {
            Ok(_) => panic!("cancelled measurement succeeded"),
            Err(error) => error,
        };
        assert_eq!(error.code(), "cancelled");
        assert!(!staged_path.exists());
        let signal = Signal::new();
        let staged = fixture.ctx.artifacts().stage(&fixture.ctx, &path, None, &signal).await.unwrap();
        let staged_path = staged.path().to_owned();
        let measured = measure(&fixture.ctx, staged, &path, &signal).await.unwrap();
        signal.cancel();
        assert_eq!(retain(&fixture.ctx, measured, None, &signal).await.unwrap_err().code(), "cancelled");
        assert!(!staged_path.exists());
        assert_eq!(fixture.artifact_files(), before);
        assert_eq!(std::fs::read(path).unwrap(), original);
    }

    #[tokio::test]
    async fn compressed_media_requires_an_explicitly_approved_decoder() {
        let fixture = Fixture::new();
        let path = fixture.root.join("reference.mp3");
        // The decoder policy must refuse before any implicit discovery or decoding, even with native PCM bytes.
        let original = pcm(750.0, 0.2);
        std::fs::write(&path, &original).unwrap();
        let before = fixture.artifact_files();
        let error = fixture.listen(json!({"source": {"file": path}})).await.unwrap_err();
        assert!(error.reason().contains("approved ffmpeg dependency"), "{}", error.reason());
        assert_eq!(fixture.artifact_files(), before);
        assert_eq!(std::fs::read(path).unwrap(), original);
    }

    #[tokio::test]
    async fn a_host_media_folder_created_after_startup_can_be_listened_to() {
        let dir = tempfile::tempdir().unwrap();
        let media = dir.path().canonicalize().unwrap().join("generated/audio");
        let ctx = Ctx::new(
            Args {
                state_dir: dir.path().join("state"),
                kumi_home: dir.path().join("home"),
                hermes_home: None,
                hermes_root: None,
                daw: DawId::Reaper,
                library_roots: Vec::new(),
                media_roots: vec![media.clone()],
                tools: BTreeMap::new(),
            },
            Env::new(),
            Default::default(),
        )
        .unwrap();
        std::fs::create_dir_all(&media).unwrap();
        let source = media.join("host-generated.wav");
        let bytes = pcm(750.0, 0.2);
        std::fs::write(&source, &bytes).unwrap();
        let result = (LISTEN.run)(
            &ctx,
            Call {
                args: json!({"source":{"file":source}}).as_object().unwrap().clone(),
                host: Host::default(),
                signal: Signal::new(),
                progress: Progress::none(),
            },
        )
        .await
        .unwrap();
        assert_eq!(result["metrics"]["seconds"], 2.0);
        assert_eq!(result["metrics"]["sampleRate"], 48_000.0);
        let retained = ctx.artifacts().resolve(&ctx, result["artifact_id"].as_str().unwrap()).unwrap();
        assert_eq!(std::fs::read(retained).unwrap(), bytes);
        assert_eq!(std::fs::read(source).unwrap(), bytes);
        ctx.snapshots().flush();
    }
}
