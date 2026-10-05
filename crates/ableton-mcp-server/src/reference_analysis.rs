use std::f64::consts::PI;

use kumi_common::js::number;
use serde::{Deserialize, Serialize};

use crate::analysis::{analyze_pcm, analyze_reconstructed_pcm, BoundaryCount, PcmAnalysis, PcmAnalysisInput, Privacy, RangeError};
use crate::audio_standards::{is_integer, ConventionalChannelLabel};

pub const REFERENCE_ANALYSIS_VERSION: &str = "reference-analysis/v2";
pub const MAX_COMPARISON_TOTAL_SAMPLES: usize = 4_000_000;
pub const MAX_COMPARISON_SECONDS_PER_SOURCE: f64 = 30.0;
pub const MAX_COMPARISON_CHANNELS: usize = 2;
pub const MIN_COMPARISON_SAMPLE_RATE: f64 = 32_000.0;
pub const MAX_COMPARISON_SAMPLE_RATE: f64 = 96_000.0;
pub const MAX_ALIGNMENT_LAG_SECONDS: f64 = 10.0;
pub const COMPARISON_ANALYSIS_RATE: f64 = 48_000.0;
const ALIGNMENT_RATE: f64 = 1_000.0;
const RESAMPLER_RADIUS: i64 = 16;
const EPSILON: f64 = 1e-12;

/// A source as the TypeScript took it: `sampleRate` and `channels` are numbers the comparison
/// itself checks to be integers in range.
#[derive(Debug, Clone, Copy)]
pub struct ReferencePcmSource<'a> {
    pub samples: &'a [f64],
    pub sample_rate: f64,
    pub channels: f64,
    pub channel_layout: Option<&'a [ConventionalChannelLabel]>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AlignmentMode {
    Auto,
    Manual,
    Disabled,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AlignmentOptions {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<AlignmentMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_lag_seconds: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub manual_offset_seconds: Option<f64>,
}

#[derive(Debug, Clone, Copy)]
pub struct ReferenceComparisonInput<'a> {
    pub project: ReferencePcmSource<'a>,
    pub reference: ReferencePcmSource<'a>,
    pub alignment: Option<AlignmentOptions>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResampledSource {
    pub source_rate: f64,
    pub target_rate: f64,
    pub resampled: bool,
    pub source_clipping: BoundaryCount,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Resampling {
    pub method: String,
    pub project: ResampledSource,
    pub reference: ResampledSource,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Alignment {
    pub available: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    pub mode: AlignmentMode,
    pub reference_offset_seconds: Option<f64>,
    pub correlation: Option<f64>,
    pub confidence: Option<f64>,
    pub ambiguous: bool,
    pub overlap_seconds: f64,
    pub max_lag_seconds: f64,
    pub resolution_seconds: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LevelMatch {
    pub available: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    pub project_integrated_lufs: Option<f64>,
    pub reference_integrated_lufs: Option<f64>,
    pub project_gain_to_reference_db: Option<f64>,
    pub bounded_suggested_gain_db: Option<f64>,
    pub suggestion_limit_db: f64,
    pub changes_audio: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectMinusReference {
    pub integrated_loudness_lu: Option<f64>,
    pub true_peak_db: Option<f64>,
    pub sample_peak_db: Option<f64>,
    pub rms_db: Option<f64>,
    pub crest_factor_db: Option<f64>,
    pub dynamic_range_db: Option<f64>,
    pub spectral_centroid_hz: Option<f64>,
    pub dominant_frequency_hz: Option<f64>,
    pub transient_density_per_second: Option<f64>,
}

impl ProjectMinusReference {
    /// `Object.values(deltas.projectMinusReference)`.
    pub fn values(&self) -> [Option<f64>; 9] {
        [
            self.integrated_loudness_lu,
            self.true_peak_db,
            self.sample_peak_db,
            self.rms_db,
            self.crest_factor_db,
            self.dynamic_range_db,
            self.spectral_centroid_hz,
            self.dominant_frequency_hz,
            self.transient_density_per_second,
        ]
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Deltas {
    pub project_minus_reference: ProjectMinusReference,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ComparisonPerformance {
    pub bounded: bool,
    pub max_total_input_samples: usize,
    pub max_seconds_per_source: f64,
    pub max_channels: usize,
    pub alignment_rate: f64,
    pub max_alignment_lag_seconds: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReferenceComparison {
    pub version: String,
    pub analysis_rate: f64,
    pub resampling: Resampling,
    pub alignment: Alignment,
    pub level_match: LevelMatch,
    pub deltas: Deltas,
    pub project: PcmAnalysis,
    pub reference: PcmAnalysis,
    pub privacy: Privacy,
    pub performance: ComparisonPerformance,
}

struct ValidatedSource {
    samples: Vec<f64>,
    sample_rate: f64,
    channels: usize,
    channel_layout: Option<Vec<ConventionalChannelLabel>>,
}

fn validate_source(source: &ReferencePcmSource, label: &str) -> Result<ValidatedSource, RangeError> {
    if !is_integer(source.sample_rate) || source.sample_rate < MIN_COMPARISON_SAMPLE_RATE || source.sample_rate > MAX_COMPARISON_SAMPLE_RATE
    {
        return Err(RangeError(format!(
            "{label}.sampleRate must be an integer from {} to {} for the validated fixed-kernel comparison resampler",
            number::to_string(MIN_COMPARISON_SAMPLE_RATE),
            number::to_string(MAX_COMPARISON_SAMPLE_RATE)
        )));
    }
    if !is_integer(source.channels) || source.channels < 1.0 || source.channels > MAX_COMPARISON_CHANNELS as f64 {
        return Err(RangeError(format!("{label}.channels must be an integer from 1 to {MAX_COMPARISON_CHANNELS}")));
    }
    let channels = source.channels as usize;
    let sample_length = source.samples.len();
    if sample_length == 0 || sample_length % channels != 0 {
        return Err(RangeError(format!("{label}.samples must contain complete channel frames")));
    }
    if sample_length as f64 / channels as f64 / source.sample_rate > MAX_COMPARISON_SECONDS_PER_SOURCE {
        return Err(RangeError(format!("{label} duration exceeds {} seconds", number::to_string(MAX_COMPARISON_SECONDS_PER_SOURCE))));
    }
    let channel_layout = source.channel_layout.map(|layout| layout.to_vec());
    if channel_layout.as_ref().is_some_and(|layout| layout.len() != channels) {
        return Err(RangeError(format!("{label}.channelLayout must match channels")));
    }
    let mut samples = Vec::with_capacity(sample_length);
    for (index, &value) in source.samples.iter().enumerate() {
        if !value.is_finite() || !(-1.0..=1.0).contains(&value) {
            return Err(RangeError(format!("{label}.samples[{index}] must be finite normalized PCM")));
        }
        samples.push(value);
    }
    Ok(ValidatedSource { samples, sample_rate: source.sample_rate, channels, channel_layout })
}

fn source_clipping(samples: &[f64]) -> BoundaryCount {
    let count = samples.iter().filter(|value| value.abs() >= 0.999999).count();
    BoundaryCount { count, ratio: count as f64 / samples.len() as f64 }
}

fn sinc(value: f64) -> f64 {
    if value.abs() < 1e-12 {
        return 1.0;
    }
    let angle = PI * value;
    angle.sin() / angle
}

fn blackman(distance: f64) -> f64 {
    let normalized = (distance + RESAMPLER_RADIUS as f64) / (2.0 * RESAMPLER_RADIUS as f64);
    0.42 - 0.5 * (2.0 * PI * normalized).cos() + 0.08 * (4.0 * PI * normalized).cos()
}

/// Deterministic band-limited resampling with a fixed, bounded 32-tap kernel.
fn resample_validated_pcm(source: &ValidatedSource, target_rate: f64) -> Result<Vec<f64>, RangeError> {
    if target_rate != COMPARISON_ANALYSIS_RATE {
        return Err(RangeError(format!("targetRate must be the fixed {} Hz comparison rate", number::to_string(COMPARISON_ANALYSIS_RATE))));
    }
    let channels = source.channels;
    let input_frames = source.samples.len() / channels;
    let output_frames = (number::round(input_frames as f64 * target_rate / source.sample_rate) as usize).max(1);
    if output_frames * channels > MAX_COMPARISON_TOTAL_SAMPLES {
        return Err(RangeError("resampled output exceeds the bounded comparison work limit".to_string()));
    }
    let mut output = vec![0.0f64; output_frames * channels];
    if target_rate == source.sample_rate {
        for (index, slot) in output.iter_mut().enumerate() {
            *slot = source.samples.get(index).copied().unwrap_or(0.0);
        }
        return Ok(output);
    }
    let cutoff = (target_rate / source.sample_rate).min(1.0) * 0.94;
    for output_frame in 0..output_frames {
        let source_position = output_frame as f64 * source.sample_rate / target_rate;
        let center = source_position.floor() as i64;
        for channel in 0..channels {
            let mut value = 0.0;
            let mut normalization = 0.0;
            for tap in (center - RESAMPLER_RADIUS + 1)..=(center + RESAMPLER_RADIUS) {
                let distance = source_position - tap as f64;
                let coefficient = cutoff * sinc(distance * cutoff) * blackman(distance);
                // A complete kernel with constant edge extension avoids dividing a
                // truncated near-zero boundary sum and cannot turn normalized short
                // programmes into unbounded reconstructed peaks.
                let source_frame = tap.clamp(0, input_frames as i64 - 1) as usize;
                value += coefficient * source.samples[source_frame * channels + channel];
                normalization += coefficient;
            }
            output[output_frame * channels + channel] = if normalization.abs() > EPSILON { value / normalization } else { 0.0 };
        }
    }
    Ok(output)
}

/// `targetRate` defaults to the fixed comparison rate.
pub fn resample_pcm(source: &ReferencePcmSource, target_rate: Option<f64>) -> Result<Vec<f64>, RangeError> {
    resample_validated_pcm(&validate_source(source, "source")?, target_rate.unwrap_or(COMPARISON_ANALYSIS_RATE))
}

fn alignment_envelope(samples: &[f64], channels: usize) -> Vec<f64> {
    let frames = samples.len() / channels;
    let block_frames = (COMPARISON_ANALYSIS_RATE / ALIGNMENT_RATE) as usize;
    let output_frames = frames / block_frames;
    (0..output_frames)
        .map(|block| {
            let mut sum = 0.0;
            for frame in 0..block_frames {
                let mut mono = 0.0;
                for channel in 0..channels {
                    mono += samples[(block * block_frames + frame) * channels + channel];
                }
                mono /= channels as f64;
                sum += mono * mono;
            }
            (sum / block_frames as f64).sqrt()
        })
        .collect()
}

#[derive(Debug, Clone, Copy)]
struct AlignmentCandidate {
    lag: i64,
    correlation: f64,
}

fn correlation_at(project: &[f64], reference: &[f64], lag: i64, minimum_count: usize) -> Option<f64> {
    let project_start = (-lag).max(0) as usize;
    let reference_start = lag.max(0) as usize;
    let count = (project.len() as i64 - project_start as i64).min(reference.len() as i64 - reference_start as i64);
    if count < minimum_count as i64 {
        return None;
    }
    let count = count as usize;
    let mut project_mean = 0.0;
    let mut reference_mean = 0.0;
    for index in 0..count {
        project_mean += project[project_start + index];
        reference_mean += reference[reference_start + index];
    }
    project_mean /= count as f64;
    reference_mean /= count as f64;
    let mut product = 0.0;
    let mut project_squares = 0.0;
    let mut reference_squares = 0.0;
    for index in 0..count {
        let project_value = project[project_start + index] - project_mean;
        let reference_value = reference[reference_start + index] - reference_mean;
        product += project_value * reference_value;
        project_squares += project_value * project_value;
        reference_squares += reference_value * reference_value;
    }
    if project_squares <= EPSILON || reference_squares <= EPSILON {
        return None;
    }
    Some((product / (project_squares * reference_squares).sqrt()).clamp(-1.0, 1.0))
}

fn downsample_envelope(input: &[f64], factor: usize) -> Vec<f64> {
    let length = input.len() / factor;
    (0..length)
        .map(|index| {
            let mut sum = 0.0;
            for offset in 0..factor {
                sum += input[index * factor + offset];
            }
            sum / factor as f64
        })
        .collect()
}

struct AutomaticAlignment {
    candidate: Option<AlignmentCandidate>,
    confidence: Option<f64>,
    ambiguous: bool,
    reason: Option<String>,
}

fn align_automatically(
    project: &[f64],
    project_channels: usize,
    reference: &[f64],
    reference_channels: usize,
    max_lag_seconds: f64,
) -> AutomaticAlignment {
    let unavailable =
        |reason: &str| AutomaticAlignment { candidate: None, confidence: None, ambiguous: true, reason: Some(reason.to_string()) };
    let project_envelope = alignment_envelope(project, project_channels);
    let reference_envelope = alignment_envelope(reference, reference_channels);
    let minimum = (ALIGNMENT_RATE / 2.0) as usize;
    if project_envelope.len() < minimum || reference_envelope.len() < minimum {
        return unavailable("automatic alignment requires at least 0.5 seconds from each source");
    }

    // Scan a 100 Hz envelope first, then refine only ±10 ms at 1 kHz. This
    // freezes worst-case correlation work below roughly eight million products
    // instead of allowing O(programmeLength × fineLagRange) growth.
    let coarse_factor = 10usize;
    let coarse_rate = ALIGNMENT_RATE / coarse_factor as f64;
    let project_coarse = downsample_envelope(&project_envelope, coarse_factor);
    let reference_coarse = downsample_envelope(&reference_envelope, coarse_factor);
    let max_coarse_lag = (number::round(max_lag_seconds * coarse_rate) as i64)
        .min(project_coarse.len().max(reference_coarse.len()) as i64 - number::round(coarse_rate / 2.0) as i64);
    let mut coarse_best: Option<AlignmentCandidate> = None;
    let mut coarse_candidates: Vec<AlignmentCandidate> = Vec::new();
    for lag in -max_coarse_lag..=max_coarse_lag {
        let Some(correlation) = correlation_at(&project_coarse, &reference_coarse, lag, (coarse_rate / 2.0) as usize) else { continue };
        let candidate = AlignmentCandidate { lag, correlation };
        coarse_candidates.push(candidate);
        if coarse_best.is_none_or(|best| correlation > best.correlation) {
            coarse_best = Some(candidate);
        }
    }
    let Some(coarse_best) = coarse_best else {
        return unavailable("automatic alignment has insufficient non-silent envelope variation");
    };

    let mut best: Option<AlignmentCandidate> = None;
    let center = coarse_best.lag * coarse_factor as i64;
    let fine_maximum = number::round(max_lag_seconds * ALIGNMENT_RATE) as i64;
    for lag in (-fine_maximum).max(center - coarse_factor as i64)..=fine_maximum.min(center + coarse_factor as i64) {
        let correlation = correlation_at(&project_envelope, &reference_envelope, lag, minimum);
        if let Some(correlation) = correlation {
            if best.is_none_or(|best| correlation > best.correlation) {
                best = Some(AlignmentCandidate { lag, correlation });
            }
        }
    }
    let Some(best) = best else {
        return unavailable("automatic alignment refinement has insufficient overlap");
    };
    let coarse_exclusion = number::round(0.05 * coarse_rate) as i64;
    let competing = coarse_candidates
        .iter()
        .filter(|candidate| (candidate.lag - coarse_best.lag).abs() > coarse_exclusion)
        .map(|candidate| candidate.correlation)
        .reduce(f64::max);
    let separation = coarse_best.correlation - competing.unwrap_or(-1.0);
    let confidence = best.correlation.clamp(0.0, 1.0) * (separation / 0.1).clamp(0.0, 1.0);
    let ambiguous = best.correlation < 0.5 || separation < 0.02;
    AutomaticAlignment {
        candidate: Some(best),
        confidence: Some(confidence),
        ambiguous,
        reason: ambiguous.then(|| "alignment is weak or has a competing envelope match".to_string()),
    }
}

struct AlignedSlices {
    project: Vec<f64>,
    reference: Vec<f64>,
    frames: usize,
}

fn aligned_slices(
    project: &[f64],
    project_channels: usize,
    reference: &[f64],
    reference_channels: usize,
    lag_frames: i64,
) -> AlignedSlices {
    let project_frames = project.len() / project_channels;
    let reference_frames = reference.len() / reference_channels;
    let project_start = (-lag_frames).max(0) as usize;
    let reference_start = lag_frames.max(0) as usize;
    let frames = (project_frames as i64 - project_start as i64).min(reference_frames as i64 - reference_start as i64).max(0) as usize;
    let slice = |samples: &[f64], start: usize, channels: usize| -> Vec<f64> {
        if frames == 0 {
            Vec::new()
        } else {
            samples[start * channels..(start + frames) * channels].to_vec()
        }
    };
    AlignedSlices {
        project: slice(project, project_start, project_channels),
        reference: slice(reference, reference_start, reference_channels),
        frames,
    }
}

fn finite_difference(left: Option<f64>, right: Option<f64>) -> Option<f64> {
    Some(left? - right?)
}

pub fn compare_reference_audio(input: &ReferenceComparisonInput) -> Result<ReferenceComparison, RangeError> {
    compare_reference_audio_mode(input, true)
}

/// Non-string modes accepted by the source worker use automatic alignment, but fail its strict
/// `mode === "auto"` test when deciding whether an unavailable alignment makes a comparison untrusted.
pub(crate) fn compare_reference_audio_mode(input: &ReferenceComparisonInput, string_mode: bool) -> Result<ReferenceComparison, RangeError> {
    let project_length = input.project.samples.len();
    let reference_length = input.reference.samples.len();
    if project_length == 0 || reference_length == 0 || project_length + reference_length > MAX_COMPARISON_TOTAL_SAMPLES {
        return Err(RangeError(format!("comparison exceeds the {MAX_COMPARISON_TOTAL_SAMPLES}-sample pair limit")));
    }
    let project_source = validate_source(&input.project, "project")?;
    let reference_source = validate_source(&input.reference, "reference")?;

    let alignment = input.alignment.unwrap_or_default();
    let mode = alignment.mode.unwrap_or(AlignmentMode::Auto);
    let max_lag_seconds = alignment.max_lag_seconds.unwrap_or(5.0);
    if !max_lag_seconds.is_finite() || max_lag_seconds < 0.0 || max_lag_seconds > MAX_ALIGNMENT_LAG_SECONDS {
        return Err(RangeError(format!("alignment.maxLagSeconds must be from 0 to {}", number::to_string(MAX_ALIGNMENT_LAG_SECONDS))));
    }
    let project_resampled = resample_validated_pcm(&project_source, COMPARISON_ANALYSIS_RATE)?;
    let reference_resampled = resample_validated_pcm(&reference_source, COMPARISON_ANALYSIS_RATE)?;
    let project_source_clipping = source_clipping(&project_source.samples);
    let reference_source_clipping = source_clipping(&reference_source.samples);
    let offset_seconds: f64;
    let mut correlation: Option<f64> = None;
    let mut confidence: Option<f64> = None;
    let mut ambiguous = false;
    let mut alignment_available = true;
    let mut alignment_reason: Option<String> = None;

    match mode {
        AlignmentMode::Manual => {
            let manual = alignment.manual_offset_seconds;
            match manual {
                Some(manual) if manual.is_finite() && manual.abs() <= max_lag_seconds => offset_seconds = manual,
                _ => return Err(RangeError("alignment.manualOffsetSeconds must be finite and within maxLagSeconds".to_string())),
            }
        }
        AlignmentMode::Disabled => {
            offset_seconds = 0.0;
            alignment_reason = Some("alignment was disabled; sources are compared from their starts".to_string());
        }
        AlignmentMode::Auto => {
            let automatic = align_automatically(
                &project_resampled,
                project_source.channels,
                &reference_resampled,
                reference_source.channels,
                max_lag_seconds,
            );
            match automatic.candidate {
                Some(candidate) if !automatic.ambiguous => {
                    offset_seconds = candidate.lag as f64 / ALIGNMENT_RATE;
                    correlation = Some(candidate.correlation);
                    confidence = automatic.confidence;
                }
                candidate => {
                    alignment_available = false;
                    ambiguous = automatic.ambiguous;
                    alignment_reason = Some(automatic.reason.unwrap_or_else(|| "automatic alignment unavailable".to_string()));
                    offset_seconds = 0.0;
                    correlation = candidate.map(|candidate| candidate.correlation);
                    confidence = automatic.confidence;
                }
            }
        }
    }

    let comparison_trusted = alignment_available || !string_mode || mode != AlignmentMode::Auto;
    let lag_frames = number::round(offset_seconds * COMPARISON_ANALYSIS_RATE) as i64;
    let aligned = aligned_slices(&project_resampled, project_source.channels, &reference_resampled, reference_source.channels, lag_frames);
    if comparison_trusted && aligned.frames == 0 {
        return Err(RangeError("alignment leaves no overlapping audio".to_string()));
    }
    let project_samples: &[f64] = if comparison_trusted { &aligned.project } else { &project_resampled };
    let reference_samples: &[f64] = if comparison_trusted { &aligned.reference } else { &reference_resampled };
    let project_input = PcmAnalysisInput {
        samples: project_samples,
        sample_rate: COMPARISON_ANALYSIS_RATE,
        channels: Some(project_source.channels as f64),
        channel_layout: project_source.channel_layout.as_deref(),
        frame_size: None,
    };
    let reference_input = PcmAnalysisInput {
        samples: reference_samples,
        sample_rate: COMPARISON_ANALYSIS_RATE,
        channels: Some(reference_source.channels as f64),
        channel_layout: reference_source.channel_layout.as_deref(),
        frame_size: None,
    };
    let project = if project_source.sample_rate == COMPARISON_ANALYSIS_RATE {
        analyze_pcm(&project_input)?
    } else {
        analyze_reconstructed_pcm(&project_input)?
    };
    let reference = if reference_source.sample_rate == COMPARISON_ANALYSIS_RATE {
        analyze_pcm(&reference_input)?
    } else {
        analyze_reconstructed_pcm(&reference_input)?
    };
    let project_lufs = project.standards_audio.loudness.integrated_lufs;
    let reference_lufs = reference.standards_audio.loudness.integrated_lufs;
    let gain = if comparison_trusted { finite_difference(reference_lufs, project_lufs) } else { None };
    let level_available = comparison_trusted && gain.is_some();
    let project_true_peak = project.standards_audio.true_peak.aggregate_dbtp;
    let reference_true_peak = reference.standards_audio.true_peak.aggregate_dbtp;
    let trusted = |value: f64| if comparison_trusted { Some(value) } else { None };

    Ok(ReferenceComparison {
        version: REFERENCE_ANALYSIS_VERSION.to_string(),
        analysis_rate: COMPARISON_ANALYSIS_RATE,
        resampling: Resampling {
            method: "32-tap Blackman-windowed sinc".to_string(),
            project: ResampledSource {
                source_rate: project_source.sample_rate,
                target_rate: COMPARISON_ANALYSIS_RATE,
                resampled: project_source.sample_rate != COMPARISON_ANALYSIS_RATE,
                source_clipping: project_source_clipping,
            },
            reference: ResampledSource {
                source_rate: reference_source.sample_rate,
                target_rate: COMPARISON_ANALYSIS_RATE,
                resampled: reference_source.sample_rate != COMPARISON_ANALYSIS_RATE,
                source_clipping: reference_source_clipping,
            },
        },
        alignment: Alignment {
            available: alignment_available,
            reason: alignment_reason,
            mode,
            reference_offset_seconds: if alignment_available || !string_mode || mode != AlignmentMode::Auto {
                Some(offset_seconds)
            } else {
                None
            },
            correlation,
            confidence,
            ambiguous,
            overlap_seconds: if comparison_trusted { aligned.frames as f64 / COMPARISON_ANALYSIS_RATE } else { 0.0 },
            max_lag_seconds,
            resolution_seconds: 1.0 / ALIGNMENT_RATE,
        },
        level_match: LevelMatch {
            available: level_available,
            reason: if level_available {
                None
            } else if comparison_trusted {
                Some("both aligned sources need qualifying BS.1770 integrated loudness".to_string())
            } else {
                Some("automatic alignment is unavailable; choose manual or disabled alignment before comparing levels".to_string())
            },
            project_integrated_lufs: project_lufs,
            reference_integrated_lufs: reference_lufs,
            project_gain_to_reference_db: gain,
            bounded_suggested_gain_db: gain.map(|gain| gain.clamp(-24.0, 24.0)),
            suggestion_limit_db: 24.0,
            changes_audio: false,
        },
        deltas: Deltas {
            project_minus_reference: ProjectMinusReference {
                integrated_loudness_lu: if comparison_trusted { finite_difference(project_lufs, reference_lufs) } else { None },
                true_peak_db: if comparison_trusted { finite_difference(project_true_peak, reference_true_peak) } else { None },
                sample_peak_db: trusted(project.peak_dbfs - reference.peak_dbfs),
                rms_db: trusted(project.rms_dbfs - reference.rms_dbfs),
                crest_factor_db: trusted(project.dynamics.crest_factor_db - reference.dynamics.crest_factor_db),
                dynamic_range_db: trusted(project.dynamics.dynamic_range_db - reference.dynamics.dynamic_range_db),
                spectral_centroid_hz: trusted(project.spectral.centroid_hz - reference.spectral.centroid_hz),
                dominant_frequency_hz: trusted(project.spectral.dominant_frequency_hz - reference.spectral.dominant_frequency_hz),
                transient_density_per_second: trusted(project.transients.density_per_second - reference.transients.density_per_second),
            },
        },
        project,
        reference,
        privacy: Privacy { raw_audio_retained: false, raw_audio_returned: false, source_path_accepted: false },
        performance: ComparisonPerformance {
            bounded: true,
            max_total_input_samples: MAX_COMPARISON_TOTAL_SAMPLES,
            max_seconds_per_source: MAX_COMPARISON_SECONDS_PER_SOURCE,
            max_channels: MAX_COMPARISON_CHANNELS,
            alignment_rate: ALIGNMENT_RATE,
            max_alignment_lag_seconds: MAX_ALIGNMENT_LAG_SECONDS,
        },
    })
}
