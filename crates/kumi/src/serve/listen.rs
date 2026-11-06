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
        "properties": {"source": source_schema(), "compare_to": super::listen_reference::reference_schema(),
            "references": super::listen_reference::references_schema()},
        "not": {"required": ["compare_to", "references"]}
    })
}

pub(super) enum Source<'a> {
    File(&'a str),
    Sound { id: &'a str, revision: &'a str },
    Artifact(&'a str),
}

impl<'a> Source<'a> {
    pub(super) fn parse(value: &'a Value) -> Result<Self, OpError> {
        Self::parse_with_extra(value, 0)
    }

    pub(super) fn parse_with_extra(value: &'a Value, extra: usize) -> Result<Self, OpError> {
        let fields = value.as_object().ok_or_else(|| OpError::Invalid { reason: "An audio source must be an object.".into() })?;
        let string = |key| fields.get(key).and_then(Value::as_str).filter(|value| !value.trim().is_empty());
        match (fields.len().saturating_sub(extra), string("file"), string("sound_id"), string("map_revision"), string("artifact_id")) {
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
pub(super) async fn resolve_source(ctx: &Ctx, source: &Source<'_>, signal: &Signal) -> Result<PathBuf, OpError> {
    source.resolve(ctx, signal).await
}

/// Stage a structured source, retaining map identity checks and reusing registered files.
/// Guarded capture uses this for its authorized comparison before touching the DAW.
pub(super) async fn stage_source(ctx: &Ctx, source: &Source<'_>, signal: &Signal) -> Result<StagedArtifact, OpError> {
    source.stage(ctx, signal).await
}

fn run(ctx: &Ctx, call: Call) -> OpFuture<'_> {
    if super::listen_track::is_track(&call.args) {
        return super::listen_track::run(ctx, call);
    }
    if super::listen_reference::is_new_request(&call.args) {
        return super::listen_reference::run_file(ctx, call);
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
        let source_path = resolve_source(ctx, &source, &call.signal).await?;
        let reference_path = match &reference {
            Some(reference) => Some(resolve_source(ctx, reference, &call.signal).await?),
            None => None,
        };
        let staged = stage_source(ctx, &source, &call.signal).await?;
        let staged_reference = match &reference {
            Some(reference) => Some(stage_source(ctx, reference, &call.signal).await?),
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

pub(super) fn check(signal: &Signal) -> Result<(), OpError> {
    if signal.is_cancelled() {
        Err(OpError::Refused { code: "cancelled", reason: "Listening was cancelled.".into(), owner: None })
    } else {
        Ok(())
    }
}

/// A successful measurement owns the still-unpublished audio it describes. Dropping it cleans the copy.
/// Capture callers can use this same path after staging their approved scratch audio.
pub struct MeasuredAudio {
    pub(super) file: StagedArtifact,
    pub metrics: Analysis,
    pub(super) mime: &'static str,
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
    let mime = audio_mime(&metrics.format)?;
    Ok(MeasuredAudio { file, metrics, mime })
}

pub(super) fn audio_mime(format: &str) -> Result<&'static str, OpError> {
    Ok(match format {
        "wav" | "wave" => "audio/wav",
        "aif" | "aiff" | "aifc" => "audio/aiff",
        "mp3" => "audio/mpeg",
        "m4a" | "mp4" => "audio/mp4",
        "aac" => "audio/aac",
        "flac" => "audio/flac",
        "ogg" | "oga" | "opus" => "audio/ogg",
        "caf" => "audio/x-caf",
        format => return Err(OpError::Failed { reason: format!("No retained audio MIME type for validated format {format}.") }),
    })
}

/// Retain measured audio and its optional reference together, then produce the C1 result.
/// Differences always mean current minus reference; MEDIA always names the retained current audio.
pub async fn retain(
    ctx: &Ctx,
    source: MeasuredAudio,
    reference: Option<MeasuredAudio>,
    signal: &Signal,
    deadline: Option<tokio::time::Instant>,
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
    async fn reference_metrics_artifact_against_file_preserves_stereo_and_dynamics() {
        fn stereo_pcm(invert_right: bool) -> Vec<u8> {
            let mono = pcm(750.0, 0.2);
            let size = ((mono.len() - 44) * 2) as u32;
            let mut bytes = mono[..44].to_vec();
            bytes[4..8].copy_from_slice(&(36 + size).to_le_bytes());
            bytes[22..24].copy_from_slice(&2u16.to_le_bytes());
            bytes[28..32].copy_from_slice(&(48_000u32 * 4).to_le_bytes());
            bytes[32..34].copy_from_slice(&4u16.to_le_bytes());
            bytes[40..44].copy_from_slice(&size.to_le_bytes());
            for sample in mono[44..].chunks_exact(2) {
                let left = i16::from_le_bytes([sample[0], sample[1]]);
                bytes.extend_from_slice(&left.to_le_bytes());
                bytes.extend_from_slice(&(if invert_right { -left } else { left }).to_le_bytes());
            }
            bytes
        }
        let fixture = Fixture::new();
        let source = fixture.root.join("side.wav");
        let reference = fixture.root.join("mid.wav");
        std::fs::write(&source, stereo_pcm(true)).unwrap();
        std::fs::write(&reference, stereo_pcm(false)).unwrap();
        let captured = fixture.listen(json!({"source": {"file": source}})).await.unwrap();
        std::fs::remove_file(source).unwrap();
        let result =
            fixture.listen(json!({"source": {"artifact_id": captured["artifact_id"]}, "compare_to": {"file": reference}})).await.unwrap();
        assert_eq!(result["artifact_id"], captured["artifact_id"]);
        let current = &result["metrics"];
        let reference = &result["compare"]["metrics"];
        assert_eq!(current["stereo"]["correlation"], -1.0);
        assert_eq!(reference["stereo"]["correlation"], 1.0);
        assert_eq!(current["stereo"]["width"], 1.0);
        assert_eq!(reference["stereo"]["width"], 0.0);
        assert_eq!(current["loudness"], reference["loudness"]);
        assert!(current["loudness"]["integratedLufs"].is_number());
        assert!(current["loudness"]["truePeakDbtp"].is_number());
        assert!(current["loudness"]["rangeLu"].is_null());
        assert_eq!(current["dynamics"]["crestDb"], reference["dynamics"]["crestDb"]);
        assert!((current["dynamics"]["crestDb"].as_f64().unwrap() - 3.0).abs() < 0.15);
        assert!(current["dynamics"]["peakToLoudnessDb"].is_number());
        let differences = &result["compare"]["differences"];
        assert_eq!(differences["stereo"]["correlation"], json!([-1.0, 1.0]));
        assert!(differences["balance"].as_array().unwrap().iter().all(|band| band["difference"] == 0.0));
        assert_eq!(differences["dynamics"]["crestDb"], json!([3.0, 3.0]));
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
        let result = retain(&fixture.ctx, measured, None, &signal, None).await.unwrap();
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
        assert_eq!(retain(&fixture.ctx, measured, None, &signal, None).await.unwrap_err().code(), "cancelled");
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
    #[tokio::test]
    async fn an_excerpt_retains_its_measured_float_samples_and_replays_once_at_zero() {
        let fixture = Fixture::new();
        let (source, _) = fixture.tone("current.wav", 1800., 0.2);
        let (reference, _) = fixture.tone("reference.wav", 110., 0.1);
        let mut bytes = std::fs::read(&reference).unwrap();
        bytes[44 + 96_000..].copy_from_slice(&pcm(1800., 0.2)[44..44 + 96_000]);
        std::fs::write(&reference, &bytes).unwrap();
        let heard = fixture.listen(json!({"source":{"file":source},"compare_to":{"file":reference,"from":1.,"seconds":0.75}})).await.unwrap();
        let compared = &heard["compare"];
        assert_eq!(compared["name"], "reference.wav");
        assert_eq!(compared["window"]["requested"], json!({"from":1.,"seconds":0.75}));
        assert_eq!(compared["window"]["start_frame"], 48_000);
        assert_eq!(compared["window"]["frame_count"], 36_000);
        assert_eq!(compared["window"]["replay_from"], 0.);
        assert!(!compared.to_string().contains(fixture.root.to_str().unwrap()));
        let id = compared["artifact_id"].as_str().unwrap();
        let kept = fixture.ctx.artifacts().resolve(&fixture.ctx, id).unwrap();
        let kept_bytes = std::fs::read(&kept).unwrap();
        assert_eq!(u16::from_le_bytes(kept_bytes[20..22].try_into().unwrap()), 3, "float32, not post-measure PCM16");
        assert_eq!(kept_bytes.len(), 44 + 36_000 * 4);
        let before: Value = serde_json::from_slice(&std::fs::read(fixture.ctx.paths().state_dir.join("artifacts.json")).unwrap()).unwrap();
        let replay = fixture.listen(json!({"source":{"file":source},"compare_to":{"artifact_id":id}})).await.unwrap();
        assert_eq!(replay["compare"]["artifact_id"], id);
        assert_eq!(replay["compare"]["metrics"]["seconds"], 0.75);
        let first = compared["metrics"]["loudness"]["integratedLufs"].as_f64().unwrap();
        let again = replay["compare"]["metrics"]["loudness"]["integratedLufs"].as_f64().unwrap();
        assert!((first - again).abs() < 0.01, "{first} vs {again}");
        let after: Value = serde_json::from_slice(&std::fs::read(fixture.ctx.paths().state_dir.join("artifacts.json")).unwrap()).unwrap();
        assert_eq!(before[id], after[id], "replay must not rewrite the excerpt's origin or registration");
        assert_eq!(std::fs::read(reference).unwrap(), bytes);
        assert_eq!(std::fs::read(kept).unwrap(), kept_bytes);
    }

    #[tokio::test]
    async fn three_reference_targets_use_real_profile_values_and_report_mono_coverage() {
        let fixture = Fixture::new();
        let (source, _) = fixture.tone("current.wav", 750., 0.2);
        let (quiet, _) = fixture.tone("quiet.wav", 750., 0.1);
        let (middle, _) = fixture.tone("middle.wav", 750., 0.2);
        let (loud, _) = fixture.tone("loud.wav", 750., 0.4);
        let mut mixed_rate = std::fs::read(&quiet).unwrap();
        mixed_rate[24..28].copy_from_slice(&24_000u32.to_le_bytes());
        mixed_rate[28..32].copy_from_slice(&48_000u32.to_le_bytes());
        std::fs::write(&quiet, mixed_rate).unwrap();
        let heard = fixture.listen(json!({"source":{"file":source},"references":[{"file":quiet},{"file":middle},{"file":loud}]})).await.unwrap();
        assert_eq!(heard["references"][0]["window"]["sample_rate"], 24_000.);
        assert_eq!(heard["references"][1]["window"]["sample_rate"], 48_000.);
        assert_eq!(heard["reference_profile"]["usable_references"], 3);
        let quantities = heard["reference_profile"]["quantities"].as_array().unwrap();
        let loudness = quantities.iter().find(|row| row["quantity"] == "integrated").unwrap();
        assert_eq!((loudness["contributors"].as_u64(), loudness["references"].as_u64()), (Some(3), Some(3)));
        assert!(loudness["deviation"].as_f64().unwrap().abs() < 0.1, "{loudness}");
        assert!(loudness["target"]["high"].as_f64().unwrap() - loudness["target"]["low"].as_f64().unwrap() > 10.);
        assert!(loudness["basis"].as_str().unwrap().contains("Heard"));
        let width = quantities.iter().find(|row| row["quantity"] == "low_width").unwrap();
        assert_eq!(width["contributors"], 0);
        assert!(width["source"].is_null() && width["target"].is_null() && width["deviation"].is_null(), "{width}");
        assert_eq!(width["source_reason"], "mono");
        assert_eq!(width["target_reason"], "no_reference_measurement");
        assert!(!quantities.iter().any(|row| row["quantity"] == "true_peak"), "a Profile has no peak target");
        assert!(!heard["references"].to_string().contains(fixture.root.to_str().unwrap()));
        assert!(fixture.ctx.history().rows(10).is_empty());
        let manifest: Value = serde_json::from_slice(&std::fs::read(fixture.ctx.paths().state_dir.join("artifacts.json")).unwrap()).unwrap();
        assert_eq!(manifest.as_object().unwrap().keys().filter(|key| key.starts_with("a-")).count(), 4);
    }

    #[tokio::test]
    async fn silent_references_never_form_floor_only_targets() {
        let fixture = Fixture::new();
        let (source, _) = fixture.tone("current.wav", 750., 0.2);
        let (silent, _) = fixture.tone("silent.wav", 750., 0.);
        let heard = fixture.listen(json!({"source":{"file":source},"references":[{"file":silent}]})).await.unwrap();
        assert_eq!(heard["references"][0]["availability"]["energy_zero"], true);
        assert_eq!(heard["references"][0]["availability"]["profile_reason"], "zero_energy");
        assert_eq!(heard["reference_profile"]["usable_references"], 0);
        for quantity in heard["reference_profile"]["quantities"].as_array().unwrap() {
            assert!(quantity["target"].is_null() && quantity["deviation"].is_null(), "{quantity}");
            assert_eq!(quantity["contributors"], 0);
            assert_eq!(quantity["target_reason"], "no_reference_measurement");
        }
    }

    #[tokio::test]
    async fn invalid_or_failed_last_references_leave_no_artifact_prefix() {
        let fixture = Fixture::new();
        let (source, original) = fixture.tone("current.wav", 750., 0.2);
        let (reference, reference_bytes) = fixture.tone("reference.wav", 750., 0.1);
        let broken = fixture.root.join("broken.wav");
        std::fs::write(&broken, b"not audio").unwrap();
        let before = fixture.artifact_files();
        for args in [
            json!({"source":{"file":source},"references":[]}),
            json!({"source":{"file":source},"references":[{"file":reference}],"compare_to":{"file":reference}}),
            json!({"source":{"file":source},"references":[{"file":reference},{"file":reference}]}),
            json!({"source":{"file":source},"references":[{"file":reference},{"file":reference,"from":0.,"seconds":2.}]}),
            json!({"source":{"file":source},"references":[{"file":reference,"from":0.000001,"seconds":0.5},{"file":reference,"from":0.000002,"seconds":0.5}]}),
            json!({"source":{"file":source},"references":[{"file":reference,"from":0.}]}),
            json!({"source":{"file":source},"references":[{"file":reference,"from":0.,"seconds":60.001}]}),
            json!({"source":{"file":source},"references":[{"file":reference},{"file":reference,"from":1.5,"seconds":1.}]}),
            json!({"source":{"file":source},"references":[{"file":reference},{"file":broken}]}),
        ] {
            assert!(fixture.listen(args.clone()).await.is_err(), "{args}");
            assert_eq!(fixture.artifact_files(), before, "{args}");
            assert!(!fixture.ctx.paths().state_dir.join("artifacts.json").exists(), "{args}");
        }
        assert_eq!(std::fs::read(source).unwrap(), original);
        assert_eq!(std::fs::read(reference).unwrap(), reference_bytes);
    }

    #[tokio::test]
    async fn cancellation_after_the_last_reference_discards_every_measured_stage() {
        let fixture = Fixture::new();
        let (source, original) = fixture.tone("current.wav", 750., 0.2);
        let (first, _) = fixture.tone("first.wav", 250., 0.1);
        let (last, _) = fixture.tone("last.wav", 750., 0.2);
        let signal = Signal::new();
        let args = json!({"references":[{"file":first},{"file":last}]});
        let selected = super::super::listen_reference::ReferenceSet::parse(args.as_object().unwrap()).unwrap();
        let references = selected.prepare(&fixture.ctx, &signal, 120).await.unwrap();
        let file = fixture.ctx.artifacts().stage(&fixture.ctx, &source, None, &signal).await.unwrap();
        let current = super::super::listen_reference::measure_current(&fixture.ctx, file, &source, true, &signal).await.unwrap();
        signal.cancel();
        let refused = super::super::listen_reference::retain(&fixture.ctx, current, references, &signal, None).await.unwrap_err();
        assert_eq!(refused.code(), "cancelled");
        assert!(fixture.artifact_files().is_empty());
        assert!(!fixture.ctx.paths().state_dir.join("artifacts.json").exists());
        assert_eq!(std::fs::read(source).unwrap(), original);
    }

    #[tokio::test]
    async fn low_rate_references_never_widen_air_targets_with_nyquist_floors() {
        let fixture = Fixture::new();
        let make = |name: &str, rate: u32, air_db: Option<f64>| {
            let high = air_db.map(|db| {
                let share = 10f64.powf(db / 10.);
                0.25 * (share / (1. - share)).sqrt()
            }).unwrap_or(0.);
            let samples: Vec<f32> = (0..rate).map(|frame| {
                let time = frame as f64 / rate as f64 * std::f64::consts::TAU;
                (0.25 * (750. * time).sin() + high * (16_000. * time).sin()) as f32
            }).collect();
            let path = fixture.root.join(name);
            std::fs::write(&path, audio::decode::float_wav(&[samples], rate)).unwrap();
            path
        };
        let current = make("dull-current.wav", 48_000, Some(-40.));
        let low = make("low-rate.wav", 22_050, None);
        let first = make("first-full.wav", 48_000, Some(-22.));
        let second = make("second-full.wav", 48_000, Some(-24.));
        let heard = fixture.listen(json!({"source":{"file":current},
            "references":[{"file":low},{"file":first},{"file":second}]})).await.unwrap();
        let quantities = heard["reference_profile"]["quantities"].as_array().unwrap();
        let air = quantities.iter().find(|row| row["quantity"] == "region/air").unwrap();
        assert_eq!((air["contributors"].as_u64(), air["references"].as_u64()), (Some(2), Some(3)));
        assert!(air["target"]["low"].as_f64().unwrap() > -30., "{air}");
        assert!(air["source"].as_f64().unwrap() < -35., "{air}");
        assert_eq!(air["inside_spread"], false, "the unavailable low-rate floor must not make a dull source fit");
        let low_quantities = heard["references"][0]["profile_quantities"].as_array().unwrap();
        let unavailable_air = low_quantities.iter().find(|row| row["quantity"] == "region/air").unwrap();
        assert!(unavailable_air["value"].is_null());
        assert_eq!(unavailable_air["reason_code"], "above_nyquist");
        assert_eq!(unavailable_air["nyquist_hz"], 11_025.);
        let partial = low_quantities.iter().find(|row| row["quantity"] == "region/brilliance").unwrap();
        assert!(partial["value"].is_null());
        assert_eq!(partial["reason_code"], "partial_frequency_coverage");
        assert!(low_quantities.iter().find(|row| row["quantity"] == "region/bass").unwrap()["value"].is_number());
        let heard = fixture.listen(json!({"source":{"file":low},"references":[{"file":first},{"file":second}]})).await.unwrap();
        let air = heard["reference_profile"]["quantities"].as_array().unwrap().iter().find(|row| row["quantity"] == "region/air").unwrap();
        assert!(air["source"].is_null() && air["deviation"].is_null() && air["inside_spread"].is_null());
        assert_eq!(air["source_reason"], "above_nyquist");
        assert_eq!(air["source_nyquist_hz"], 11_025.);
        let heard = fixture.listen(json!({"source":{"file":current},"references":[{"file":low}]})).await.unwrap();
        let air = heard["reference_profile"]["quantities"].as_array().unwrap().iter().find(|row| row["quantity"] == "region/air").unwrap();
        assert!(air["target"].is_null() && air["deviation"].is_null());
        assert_eq!(air["contributors"], 0);
        assert_eq!(air["target_reason"], "no_reference_measurement");
    }
}
