//! Audio diagnosis, on the Live simulator's seed Set: one track, "Drums",
//! with a Utility device and one published parameter.

use std::f64::consts::PI;

use ableton_mcp_server::analysis::{analyze_pcm, PcmAnalysisInput};
use ableton_mcp_server::audio_diagnosis::{
    diagnose_audio_with_live_context, AudioSourceKind, AudioSourceProvenance, CaptureTap, Device, LiveSet, LiveSnapshot, MixerState,
    Parameter, RelationshipToLive, Track,
};

fn snapshot() -> LiveSnapshot {
    LiveSnapshot {
        set: LiveSet { r#ref: "set:set-1".to_string(), name: "Simulator Set".to_string() },
        tracks: vec![Track {
            r#ref: "track:track-1".to_string(),
            name: "Drums".to_string(),
            kind: "regular".to_string(),
            mixer: Some(MixerState {
                volume: Some(0.85),
                volume_ref: Some("parameter:mixer-volume-1".to_string()),
                pan_ref: Some("parameter:mixer-pan-1".to_string()),
                cue_ref: None,
                send_refs: vec!["parameter:send-a-1".to_string()],
            }),
            routing: None,
            devices: vec![Device {
                r#ref: "device:utility-1".to_string(),
                name: "Utility".to_string(),
                kind: "audio-effect".to_string(),
                enabled: Some(true),
                parameters: vec![Parameter {
                    r#ref: "parameter:gain-1".to_string(),
                    name: "Gain".to_string(),
                    value: 0.5,
                    display_value: Some("0.5".to_string()),
                }],
                chains: None,
                drum_pads: None,
            }],
        }],
    }
}

fn source(kind: AudioSourceKind) -> AudioSourceProvenance {
    AudioSourceProvenance {
        kind,
        observed_at: "2026-01-01T00:00:00.000Z".to_string(),
        description: "bounded test source".to_string(),
        capture_id: (kind == AudioSourceKind::VerifiedLiveResamplingCapture).then(|| "capture-test-00000001".to_string()),
    }
}

fn analysis(samples: &[f64]) -> ableton_mcp_server::analysis::PcmAnalysis {
    analyze_pcm(&PcmAnalysisInput { samples, sample_rate: 48_000.0, channels: None, channel_layout: None, frame_size: None }).unwrap()
}

fn is_hex_digest(text: &str) -> bool {
    text.len() == 64 && text.chars().all(|character| character.is_ascii_hexdigit() && !character.is_ascii_uppercase())
}

#[test]
fn links_measurements_to_exact_live_refs_without_asserting_device_causality() {
    let snapshot = snapshot();
    let track = &snapshot.tracks[0];
    let samples: Vec<f64> = (0..48_000).map(|frame| if frame % 2 == 0 { 1.0 } else { -1.0 }).collect();
    let diagnosis = diagnose_audio_with_live_context(
        &analysis(&samples),
        &snapshot,
        1,
        &track.r#ref,
        &source(AudioSourceKind::CallerSuppliedPcm),
        Some("2026-01-01T00:00:01.000Z".to_string()),
    )
    .unwrap();
    assert_eq!(diagnosis.source.relationship_to_live, RelationshipToLive::DeclaredByCallerNotVerified);
    assert_eq!(diagnosis.context.track.r#ref, track.r#ref);
    assert!(is_hex_digest(&diagnosis.context.context_revision));
    assert!(is_hex_digest(&diagnosis.diagnosis_id));
    assert!(!diagnosis.causality.claimed);
    let clipping =
        diagnosis.findings.iter().find(|finding| finding.finding_id == "sample-full-scale-boundary").expect("a clipping finding");
    assert!(clipping.project_refs.contains(&track.r#ref));
    let preview = clipping.suggested_preview.as_ref().expect("the mixer has a volume");
    assert_eq!(preview.tool, "live_mixer_preview");
    assert_eq!(preview.arguments.track_ref, track.r#ref);
    assert_eq!(preview.arguments.volume, 0.765);
    assert!(diagnosis.findings.iter().all(|finding| !finding.hypothesis.to_lowercase().contains("caused by")));
    assert!(!diagnosis.privacy.raw_audio_returned);
    assert_eq!(diagnosis.context.captured_at, "2026-01-01T00:00:01.000Z");
    // Mixer refs are listed once each, after the track, in the order the mixer names them.
    assert_eq!(clipping.project_refs, vec!["track:track-1", "parameter:mixer-volume-1", "parameter:mixer-pan-1", "parameter:send-a-1"]);
    let context = diagnosis.findings.iter().find(|finding| finding.finding_id == "signal-chain-context-observed").expect("device context");
    assert_eq!(context.project_refs, vec!["track:track-1", "device:utility-1"]);
    assert_eq!(context.measured_evidence["orderedDeviceCount"], 1);
    assert_eq!(context.measured_evidence["publishedParameterCount"], 1);
    // The same measurement of the same Live state names the same diagnosis.
    let again = diagnose_audio_with_live_context(
        &analysis(&samples),
        &snapshot,
        1,
        &track.r#ref,
        &source(AudioSourceKind::CallerSuppliedPcm),
        Some("2026-01-01T00:00:02.000Z".to_string()),
    )
    .unwrap();
    assert_eq!(again.diagnosis_id, diagnosis.diagnosis_id);
    assert_eq!(again.context.context_revision, diagnosis.context.context_revision);
}

#[test]
fn marks_mapper_owned_capture_provenance_as_verified_and_reports_unavailable_latency() {
    let snapshot = snapshot();
    let samples: Vec<f64> = (0..48_000).map(|frame| (0.1 * (2.0 * PI * 440.0 * frame as f64 / 48_000.0).sin()) as f32 as f64).collect();
    let diagnosis = diagnose_audio_with_live_context(
        &analysis(&samples),
        &snapshot,
        1,
        &snapshot.tracks[0].r#ref,
        &source(AudioSourceKind::VerifiedLiveResamplingCapture),
        None,
    )
    .unwrap();
    assert_eq!(diagnosis.source.relationship_to_live, RelationshipToLive::VerifiedByCaptureLifecycle);
    assert_eq!(diagnosis.context.capture_tap, CaptureTap::SessionResampling);
    assert!(!diagnosis.context.latency_available);
    assert!(!diagnosis.findings.is_empty());
    assert_eq!(diagnosis.source.capture_id.as_deref(), Some("capture-test-00000001"));
    assert!(diagnosis.context.captured_at.ends_with('Z'));
}

#[test]
fn refuses_a_stale_or_fabricated_track_ref() {
    let samples = vec![0.0f64; 48_000];
    let failure = diagnose_audio_with_live_context(
        &analysis(&samples),
        &snapshot(),
        1,
        "track:missing",
        &source(AudioSourceKind::CallerSuppliedPcm),
        None,
    )
    .unwrap_err();
    assert!(failure.0.contains("not present"), "{failure}");
}

#[test]
fn serializes_as_the_typescript_did() {
    let samples = vec![0.0f64; 48_000];
    let diagnosis = diagnose_audio_with_live_context(
        &analysis(&samples),
        &snapshot(),
        1,
        "track:track-1",
        &source(AudioSourceKind::CallerSuppliedPcm),
        Some("2026-01-01T00:00:01.000Z".to_string()),
    )
    .unwrap();
    let json = kumi_common::js::json::stringify(&serde_json::to_value(&diagnosis).unwrap());
    assert!(json.starts_with("{\"version\":\"audio-diagnosis/v1\",\"diagnosisId\":\""), "{json}");
    assert!(json.contains("\"source\":{\"kind\":\"caller-supplied-pcm\",\"observedAt\":\"2026-01-01T00:00:00.000Z\",\"description\":\"bounded test source\",\"relationshipToLive\":\"declared-by-caller-not-verified\"}"), "{json}");
    assert!(json.contains("\"context\":{\"epoch\":1,\"set\":{\"ref\":\"set:set-1\",\"name\":\"Simulator Set\"},\"track\":{\"ref\":\"track:track-1\",\"name\":\"Drums\",\"kind\":\"regular\"},\"mixer\":{"), "{json}");
    assert!(json.contains("\"routing\":null,\"devices\":[{\"ref\":\"device:utility-1\",\"name\":\"Utility\",\"kind\":\"audio-effect\",\"enabled\":true,\"parentRef\":\"track:track-1\",\"parameters\":[{\"ref\":\"parameter:gain-1\",\"name\":\"Gain\",\"value\":0.5,\"displayValue\":\"0.5\"}]}],\"capturedAt\":\"2026-01-01T00:00:01.000Z\",\"contextRevision\":\""), "{json}");
    assert!(json.contains("\"latencyAvailable\":false,\"captureTap\":\"caller-declared-unknown\"}"), "{json}");
    assert!(json.contains("\"confidence\":\"low-context-only\""), "{json}");
    assert!(json.ends_with("\"privacy\":{\"rawAudioRetained\":false,\"rawAudioReturned\":false}}"), "{json}");
}
