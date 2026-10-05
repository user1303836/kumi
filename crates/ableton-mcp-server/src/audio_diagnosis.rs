use kumi_common::js::{json, number};
use kumi_common::time::{iso_string, now_ms};
use serde::{Deserialize, Serialize};
use serde_json::{json as json_value, Map, Value};
use sha2::{Digest, Sha256};

use crate::analysis::PcmAnalysis;

pub const AUDIO_DIAGNOSIS_VERSION: &str = "audio-diagnosis/v1";

// Only the fields of Live's rows that the diagnosis reads: the context revision hashes over these.
pub type LiveRef = String;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Parameter {
    pub r#ref: LiveRef,
    pub name: String,
    pub value: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_value: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceChain {
    pub r#ref: LiveRef,
    pub devices: Vec<Device>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DrumPad {
    pub r#ref: LiveRef,
    pub chains: Vec<DeviceChain>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Device {
    pub r#ref: LiveRef,
    pub name: String,
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    pub parameters: Vec<Parameter>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chains: Option<Vec<DeviceChain>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub drum_pads: Option<Vec<DrumPad>>,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MixerState {
    pub volume: Option<f64>,
    pub volume_ref: Option<LiveRef>,
    pub pan_ref: Option<LiveRef>,
    pub cue_ref: Option<LiveRef>,
    #[serde(default)]
    pub send_refs: Vec<LiveRef>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Track {
    pub r#ref: LiveRef,
    pub name: String,
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mixer: Option<MixerState>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub routing: Option<Value>,
    pub devices: Vec<Device>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LiveSet {
    pub r#ref: LiveRef,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LiveSnapshot {
    pub set: LiveSet,
    pub tracks: Vec<Track>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AudioSourceKind {
    CallerSuppliedPcm,
    VerifiedLiveResamplingCapture,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AudioSourceProvenance {
    pub kind: AudioSourceKind,
    pub observed_at: String,
    pub description: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capture_id: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FindingSeverity {
    Info,
    Warning,
    Critical,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum FindingConfidence {
    HighMeasurement,
    Medium,
    LowContextOnly,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MixerPreviewArguments {
    pub track_ref: LiveRef,
    pub volume: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SuggestedPreview {
    pub tool: String,
    pub arguments: MixerPreviewArguments,
    pub verification: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AudioDiagnosticFinding {
    pub finding_id: String,
    pub severity: FindingSeverity,
    pub measured_evidence: Map<String, Value>,
    pub project_refs: Vec<LiveRef>,
    pub hypothesis: String,
    pub confidence: FindingConfidence,
    pub missing_evidence: Vec<String>,
    pub suggested_preview: Option<SuggestedPreview>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RelationshipToLive {
    VerifiedByCaptureLifecycle,
    DeclaredByCallerNotVerified,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiagnosisSource {
    pub kind: AudioSourceKind,
    pub observed_at: String,
    pub description: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capture_id: Option<String>,
    pub relationship_to_live: RelationshipToLive,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CaptureTap {
    SessionResampling,
    CallerDeclaredUnknown,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContextSet {
    pub r#ref: LiveRef,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContextTrack {
    pub r#ref: LiveRef,
    pub name: String,
    pub kind: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContextParameter {
    pub r#ref: LiveRef,
    pub name: String,
    pub value: f64,
    pub display_value: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContextDevice {
    pub r#ref: LiveRef,
    pub name: String,
    pub kind: String,
    pub enabled: Option<bool>,
    pub parent_ref: LiveRef,
    pub parameters: Vec<ContextParameter>,
}

/// What `contextRevision` hashes: the Live state the measurement is linked to.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ContextPayload {
    epoch: i64,
    set: ContextSet,
    track: ContextTrack,
    mixer: Option<MixerState>,
    routing: Option<Value>,
    devices: Vec<ContextDevice>,
}

/// Keys in the order the TypeScript wrote them: the hashed payload first, then the capture facts.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiagnosisContext {
    pub epoch: i64,
    pub set: ContextSet,
    pub track: ContextTrack,
    pub mixer: Option<MixerState>,
    pub routing: Option<Value>,
    pub devices: Vec<ContextDevice>,
    pub captured_at: String,
    pub context_revision: String,
    pub latency_available: bool,
    pub capture_tap: CaptureTap,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Causality {
    pub claimed: bool,
    pub statement: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiagnosisPrivacy {
    pub raw_audio_retained: bool,
    pub raw_audio_returned: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AudioDiagnosis {
    pub version: String,
    pub diagnosis_id: String,
    pub source: DiagnosisSource,
    pub context: DiagnosisContext,
    pub findings: Vec<AudioDiagnosticFinding>,
    pub causality: Causality,
    pub privacy: DiagnosisPrivacy,
}

/// What the diagnosis throws: its message.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct DiagnosisError(pub String);

fn flatten_devices(devices: &[Device], parent_ref: &str, output: &mut Vec<ContextDevice>) {
    for device in devices {
        if output.len() >= 64 {
            return;
        }
        output.push(ContextDevice {
            r#ref: device.r#ref.clone(),
            name: device.name.clone(),
            kind: device.kind.clone(),
            enabled: device.enabled,
            parent_ref: parent_ref.to_string(),
            parameters: device
                .parameters
                .iter()
                .take(32)
                .map(|parameter| ContextParameter {
                    r#ref: parameter.r#ref.clone(),
                    name: parameter.name.clone(),
                    value: parameter.value,
                    display_value: parameter.display_value.clone(),
                })
                .collect(),
        });
        for chain in device.chains.as_deref().unwrap_or_default() {
            flatten_devices(&chain.devices, &chain.r#ref, output);
        }
        for pad in device.drum_pads.as_deref().unwrap_or_default() {
            for chain in &pad.chains {
                flatten_devices(&chain.devices, &chain.r#ref, output);
            }
        }
    }
}

fn refs(track: &Track) -> Vec<LiveRef> {
    let mut result: Vec<LiveRef> = vec![track.r#ref.clone()];
    if let Some(mixer) = &track.mixer {
        for reference in [&mixer.volume_ref, &mixer.pan_ref, &mixer.cue_ref].into_iter().flatten().chain(mixer.send_refs.iter()) {
            if !reference.is_empty() {
                result.push(reference.clone());
            }
        }
    }
    let mut unique: Vec<LiveRef> = Vec::new();
    for reference in result {
        if !unique.contains(&reference) {
            unique.push(reference);
        }
    }
    unique
}

fn mixer_suggestion(track: &Track) -> Option<SuggestedPreview> {
    let volume = track.mixer.as_ref().and_then(|mixer| mixer.volume)?;
    if !volume.is_finite() || volume <= 0.0 {
        return None;
    }
    Some(SuggestedPreview {
        tool: "live_mixer_preview".to_string(),
        arguments: MixerPreviewArguments {
            track_ref: track.r#ref.clone(),
            volume: (number::round(volume * 0.9 * 1_000_000.0) / 1_000_000.0).max(0.0),
        },
        verification: "This is a reversible 10% normalized-control intervention, not a promised dB change. Confirm explicitly, recapture the identical scope, and compare before/after measurements.".to_string(),
    })
}

fn sha256_hex(text: &str) -> String {
    hex::encode(Sha256::digest(text.as_bytes()))
}

fn evidence(value: Value) -> Map<String, Value> {
    value.as_object().cloned().unwrap_or_default()
}

/// `capturedAt` defaults to now, as an ISO string.
pub fn diagnose_audio_with_live_context(
    analysis: &PcmAnalysis,
    snapshot: &LiveSnapshot,
    epoch: i64,
    track_ref: &str,
    source: &AudioSourceProvenance,
    captured_at: Option<String>,
) -> Result<AudioDiagnosis, DiagnosisError> {
    let captured_at = captured_at.unwrap_or_else(|| iso_string(now_ms()));
    let track = snapshot
        .tracks
        .iter()
        .find(|item| item.r#ref == track_ref)
        .ok_or_else(|| DiagnosisError("diagnosis trackRef is not present in the authoritative snapshot".to_string()))?;
    let mut devices: Vec<ContextDevice> = Vec::new();
    flatten_devices(&track.devices, &track.r#ref, &mut devices);
    let context_payload = ContextPayload {
        epoch,
        set: ContextSet { r#ref: snapshot.set.r#ref.clone(), name: snapshot.set.name.clone() },
        track: ContextTrack { r#ref: track.r#ref.clone(), name: track.name.clone(), kind: track.kind.clone() },
        mixer: track.mixer.clone(),
        routing: track.routing.clone(),
        devices,
    };
    let context_revision = sha256_hex(&json::stringify(&serde_json::to_value(&context_payload).expect("context payload serializes")));
    let project_refs = refs(track);
    let mut findings: Vec<AudioDiagnosticFinding> = Vec::new();
    let true_peak = analysis.standards_audio.true_peak.aggregate_dbtp;

    if analysis.clipping.count > 0 {
        findings.push(AudioDiagnosticFinding {
            finding_id: "sample-full-scale-boundary".to_string(),
            severity: FindingSeverity::Critical,
            measured_evidence: evidence(json_value!({ "clippingSamples": analysis.clipping.count, "clippingRatio": analysis.clipping.ratio, "samplePeakDbfs": analysis.peak_dbfs })),
            project_refs: project_refs.clone(),
            hypothesis: "The analyzed programme reaches the normalized sample boundary; gain staging or limiting in the measured path may need inspection.".to_string(),
            confidence: FindingConfidence::HighMeasurement,
            missing_evidence: vec!["No Live meter history or per-device gain-reduction telemetry is available, so no device is identified as causal.".to_string()],
            suggested_preview: mixer_suggestion(track),
        });
    } else if true_peak.is_some_and(|value| value > -1.0) {
        findings.push(AudioDiagnosticFinding {
            finding_id: "limited-true-peak-headroom".to_string(),
            severity: FindingSeverity::Warning,
            measured_evidence: evidence(
                json_value!({ "truePeakDbtp": true_peak, "samplePeakDbfs": analysis.peak_dbfs, "thresholdDbtp": -1 }),
            ),
            project_refs: project_refs.clone(),
            hypothesis: "The measured signal has less than 1 dB of true-peak headroom at this capture point.".to_string(),
            confidence: FindingConfidence::HighMeasurement,
            missing_evidence: vec![
                "Delivery target and downstream codec behavior were not provided.".to_string(),
                "Ordered devices are context only; their presence does not prove causation.".to_string(),
            ],
            suggested_preview: mixer_suggestion(track),
        });
    }

    let dc_maximum = analysis.channels_detail.iter().fold(f64::NEG_INFINITY, |maximum, channel| maximum.max(channel.dc_offset.abs()));
    if dc_maximum > 0.01 {
        findings.push(AudioDiagnosticFinding {
            finding_id: "dc-offset-observed".to_string(),
            severity: FindingSeverity::Warning,
            measured_evidence: evidence(json_value!({ "maximumAbsoluteDcOffset": dc_maximum })),
            project_refs: project_refs.clone(),
            hypothesis: "The analyzed selection contains a measurable DC component or an asymmetric short selection.".to_string(),
            confidence: FindingConfidence::HighMeasurement,
            missing_evidence: vec![
                "A longer, silence-trimmed capture may be needed to distinguish sustained DC from selection bias.".to_string()
            ],
            suggested_preview: None,
        });
    }

    if analysis.stereo.phase_correlation.is_some_and(|value| value < -0.2) {
        findings.push(AudioDiagnosticFinding {
            finding_id: "negative-stereo-correlation".to_string(),
            severity: FindingSeverity::Warning,
            measured_evidence: evidence(json_value!({ "phaseCorrelation": analysis.stereo.phase_correlation })),
            project_refs: project_refs.clone(),
            hypothesis: "The measured stereo programme has substantial anti-correlated energy and may lose level when collapsed to mono."
                .to_string(),
            confidence: FindingConfidence::HighMeasurement,
            missing_evidence: vec!["No mono fold-down audition or downstream playback topology was observed.".to_string()],
            suggested_preview: None,
        });
    }

    if !context_payload.devices.is_empty() {
        let mut device_refs: Vec<LiveRef> = vec![track.r#ref.clone()];
        device_refs.extend(context_payload.devices.iter().map(|device| device.r#ref.clone()));
        findings.push(AudioDiagnosticFinding {
            finding_id: "signal-chain-context-observed".to_string(),
            severity: FindingSeverity::Info,
            measured_evidence: evidence(json_value!({
                "orderedDeviceCount": context_payload.devices.len(),
                "publishedParameterCount": context_payload.devices.iter().map(|device| device.parameters.len()).sum::<usize>(),
            })),
            project_refs: device_refs,
            hypothesis: "The ordered device and published-parameter rows are available for operator inspection, but they are not attributed as causes of any measured delta.".to_string(),
            confidence: FindingConfidence::LowContextOnly,
            missing_evidence: vec!["Device latency, sidechain topology, hidden parameters, gain reduction, and exact capture position inside the device chain are unavailable.".to_string()],
            suggested_preview: None,
        });
    }

    if findings.is_empty() {
        findings.push(AudioDiagnosticFinding {
            finding_id: "no-threshold-finding".to_string(),
            severity: FindingSeverity::Info,
            measured_evidence: evidence(json_value!({ "clippingSamples": 0, "truePeakDbtp": true_peak, "phaseCorrelation": analysis.stereo.phase_correlation })),
            project_refs: project_refs.clone(),
            hypothesis: "No configured clipping, true-peak-headroom, DC-offset, or stereo-correlation threshold was crossed by this bounded analysis.".to_string(),
            confidence: FindingConfidence::HighMeasurement,
            missing_evidence: vec!["This is not a mastering, compliance, or perceptual-quality verdict.".to_string()],
            suggested_preview: None,
        });
    }

    let verified = source.kind == AudioSourceKind::VerifiedLiveResamplingCapture;
    let relationship_to_live =
        if verified { RelationshipToLive::VerifiedByCaptureLifecycle } else { RelationshipToLive::DeclaredByCallerNotVerified };
    let diagnosis_id = sha256_hex(&json::stringify(&json_value!({
        "source": source,
        "contextRevision": context_revision,
        "analysisVersion": analysis.version,
        "integrated": analysis.standards_audio.loudness.integrated_lufs,
        "truePeak": true_peak,
    })));
    let ContextPayload { epoch, set, track: context_track, mixer, routing, devices } = context_payload;
    Ok(AudioDiagnosis {
        version: AUDIO_DIAGNOSIS_VERSION.to_string(),
        diagnosis_id,
        source: DiagnosisSource {
            kind: source.kind,
            observed_at: source.observed_at.clone(),
            description: source.description.clone(),
            capture_id: source.capture_id.clone(),
            relationship_to_live,
        },
        context: DiagnosisContext {
            epoch,
            set,
            track: context_track,
            mixer,
            routing,
            devices,
            captured_at,
            context_revision,
            latency_available: false,
            capture_tap: if verified { CaptureTap::SessionResampling } else { CaptureTap::CallerDeclaredUnknown },
        },
        findings,
        causality: Causality {
            claimed: false,
            statement: "Measured audio and observed Live state are linked by the stated source provenance; device presence and parameter values are never treated as proof of cause.".to_string(),
        },
        privacy: DiagnosisPrivacy { raw_audio_retained: false, raw_audio_returned: false },
    })
}

/// Preserve the complete observed mixer/routing objects at the actual Live boundary.
/// The compact calculation types above retain the numeric and reference fields used by findings;
/// the evidence and its digests must include every field the authoritative snapshot supplied.
pub fn diagnose_audio_with_live_context_value(
    analysis: &PcmAnalysis,
    snapshot: &Value,
    epoch: i64,
    track_ref: &str,
    source: &AudioSourceProvenance,
    captured_at: Option<String>,
) -> Result<Value, DiagnosisError> {
    let track = snapshot["tracks"]
        .as_array()
        .and_then(|rows| rows.iter().find(|row| row["ref"] == track_ref))
        .ok_or_else(|| DiagnosisError("diagnosis trackRef is not present in the authoritative snapshot".into()))?;
    let selected = json_value!({"set":snapshot["set"],"tracks":[track]});
    let typed: LiveSnapshot = serde_json::from_value(selected).map_err(|cause| DiagnosisError(cause.to_string()))?;
    let diagnosis = diagnose_audio_with_live_context(analysis, &typed, epoch, track_ref, source, captured_at)?;
    let mut result = serde_json::to_value(diagnosis).expect("diagnosis JSON");
    let payload = json_value!({
        "epoch":epoch,"set":result["context"]["set"],"track":result["context"]["track"],
        "mixer":track.get("mixer").unwrap_or(&Value::Null),"routing":track.get("routing").unwrap_or(&Value::Null),
        "devices":result["context"]["devices"]
    });
    let context_revision = sha256_hex(&json::stringify(&payload));
    result["context"]["mixer"] = payload["mixer"].clone();
    result["context"]["routing"] = payload["routing"].clone();
    result["context"]["contextRevision"] = json_value!(context_revision);
    result["diagnosisId"] = json_value!(sha256_hex(&json::stringify(&json_value!({
        "source":source,"contextRevision":context_revision,"analysisVersion":analysis.version,
        "integrated":analysis.standards_audio.loudness.integrated_lufs,"truePeak":analysis.standards_audio.true_peak.aggregate_dbtp
    }))));
    Ok(result)
}
