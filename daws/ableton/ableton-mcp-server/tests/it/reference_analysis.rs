use std::f64::consts::PI;

use ableton_mcp_server::reference_analysis::{
    compare_reference_audio, resample_pcm, AlignmentMode, AlignmentOptions, ReferenceComparisonInput, ReferencePcmSource,
    MAX_COMPARISON_TOTAL_SAMPLES,
};

/// `Float32Array.from(...)` of a seeded, smoothed noise programme under a raised-sine envelope.
fn deterministic_programme(sample_rate: f64, seconds: f64) -> Vec<f64> {
    let frames = kumi_common::js::number::round(sample_rate * seconds) as usize;
    let mut seed: u32 = 0x5eed1234;
    let mut smooth = 0.0f64;
    (0..frames)
        .map(|frame| {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            let noise = seed as f64 / 4_294_967_296.0 * 2.0 - 1.0;
            smooth = 0.98 * smooth + 0.02 * noise;
            let envelope = 0.15 + 0.75 * (PI * frame as f64 / frames as f64).sin().powi(2);
            (0.5 * smooth * envelope) as f32 as f64
        })
        .collect()
}

fn source(samples: &[f64], sample_rate: f64, channels: f64) -> ReferencePcmSource<'_> {
    ReferencePcmSource { samples, sample_rate, channels, channel_layout: None }
}

fn tone(sample_rate: f64, frames: usize, frequency: f64, amplitude: f64) -> Vec<f64> {
    (0..frames).map(|frame| (amplitude * (2.0 * PI * frequency * frame as f64 / sample_rate).sin()) as f32 as f64).collect()
}

fn alignment(mode: AlignmentMode) -> Option<AlignmentOptions> {
    Some(AlignmentOptions { mode: Some(mode), max_lag_seconds: None, manual_offset_seconds: None })
}

#[test]
fn band_limited_resampling_preserves_bounded_tone_frequency_level_and_duration() {
    let source_rate = 44_100.0;
    let samples = tone(source_rate, 44_100 * 2, 1_000.0, 0.5);
    let result = resample_pcm(&source(&samples, source_rate, 1.0), None).unwrap();
    assert_eq!(result.len(), 96_000);
    let mut sum_squares = 0.0;
    let mut peak = 0.0f64;
    for value in &result[100..result.len() - 100] {
        sum_squares += value * value;
        peak = peak.max(value.abs());
    }
    assert!(((sum_squares / (result.len() - 200) as f64).sqrt() - std::f64::consts::FRAC_1_SQRT_2 * 0.5).abs() < 0.001);
    assert!((peak - 0.5).abs() < 0.002);
    assert_eq!(result, resample_pcm(&source(&samples, source_rate, 1.0), None).unwrap());
}

#[test]
fn reference_comparison_reports_reconstruction_overs_without_false_clipping() {
    let source_rate = 44_100.0;
    let samples: Vec<f64> = (0..44_100).map(|frame| if (frame as f64) < 44_100.0 / 2.0 { 0.99f32 } else { -0.99f32 } as f64).collect();
    let resampled = resample_pcm(&source(&samples, source_rate, 1.0), None).unwrap();
    assert!(
        resampled.iter().fold(0.0f64, |maximum, value| maximum.max(value.abs())) > 1.0,
        "fixture must produce bounded sinc reconstruction overs"
    );
    let comparison = compare_reference_audio(&ReferenceComparisonInput {
        project: source(&samples, source_rate, 1.0),
        reference: source(&samples, source_rate, 1.0),
        alignment: alignment(AlignmentMode::Disabled),
    })
    .unwrap();
    assert_eq!(comparison.version, "reference-analysis/v2");
    for (side, analysis) in
        [(&comparison.resampling.project, &comparison.project), (&comparison.resampling.reference, &comparison.reference)]
    {
        assert_eq!((side.source_clipping.count, side.source_clipping.ratio), (0, 0.0));
        assert_eq!(analysis.version, "pcm-analysis/v3");
        assert_eq!((analysis.clipping.count, analysis.clipping.ratio), (0, 0.0));
        assert!(analysis.reconstructed_overs.applicable);
        assert!(analysis.reconstructed_overs.count > 0);
        assert!(!analysis.remediation.iter().any(|item| item.id == "reduce-clipping"));
        assert!(analysis.remediation.iter().any(|item| item.id == "inspect-reconstructed-overs"));
        assert!(analysis.dynamics.dynamic_range_db.is_finite());
    }
}

#[test]
fn resampling_short_alternating_material_remains_bounded_at_both_edges() {
    let samples: Vec<f64> = (0..188).map(|index| if index % 2 == 0 { 1.0 } else { -1.0 }).collect();
    let resampled = resample_pcm(&source(&samples, 32_000.0, 1.0), None).unwrap();
    assert!(resampled.iter().all(|value| value.is_finite()));
    assert!(
        resampled.iter().all(|value| value.abs() <= 2.0),
        "unexpected reconstructed peak {}",
        resampled.iter().fold(0.0f64, |maximum, value| maximum.max(value.abs()))
    );
    let comparison = compare_reference_audio(&ReferenceComparisonInput {
        project: source(&samples, 32_000.0, 1.0),
        reference: source(&samples, 32_000.0, 1.0),
        alignment: alignment(AlignmentMode::Disabled),
    })
    .unwrap();
    assert!(comparison.alignment.available);
    assert_eq!(
        (comparison.resampling.project.source_clipping.count, comparison.resampling.project.source_clipping.ratio),
        (samples.len(), 1.0)
    );
    assert_eq!(comparison.project.clipping.count, 0);
    assert!(!comparison.project.remediation.iter().any(|item| item.id == "reduce-clipping"));
}

#[test]
fn snapshots_observable_source_length_and_values_once() {
    // A slice's length and values cannot change under the resampler; the output of the one read is
    // what the TypeScript asserted of its Proxy.
    let samples = vec![0.1f64; 320];
    let output = resample_pcm(&source(&samples, 32_000.0, 1.0), None).unwrap();
    assert_eq!(output.len(), 480);
}

#[test]
fn aligns_a_known_offset_and_compares_only_equal_overlap() {
    let project = deterministic_programme(48_000.0, 4.0);
    let delay_frames = kumi_common::js::number::round(0.237 * 48_000.0) as usize;
    let mut reference = vec![0.0f64; project.len() + delay_frames];
    reference[delay_frames..].copy_from_slice(&project);
    let result = compare_reference_audio(&ReferenceComparisonInput {
        project: source(&project, 48_000.0, 1.0),
        reference: source(&reference, 48_000.0, 1.0),
        alignment: Some(AlignmentOptions { mode: Some(AlignmentMode::Auto), max_lag_seconds: Some(1.0), manual_offset_seconds: None }),
    })
    .unwrap();
    assert!(result.alignment.available);
    assert!(!result.alignment.ambiguous);
    assert!((result.alignment.reference_offset_seconds.unwrap_or(0.0) - 0.237).abs() <= 0.001);
    assert!(result.alignment.correlation.unwrap_or(0.0) > 0.999);
    assert_eq!(result.alignment.overlap_seconds, 4.0);
    assert!(result.deltas.project_minus_reference.integrated_loudness_lu.unwrap_or(1.0).abs() < 0.001);
    assert!(result.level_match.project_gain_to_reference_db.unwrap_or(1.0).abs() < 0.001);
    assert!(!result.privacy.raw_audio_returned);
}

#[test]
fn normalizes_mismatched_rates_and_reports_a_standards_loudness_level_match_suggestion() {
    let project_rate = 44_100.0;
    let reference_rate = 48_000.0;
    let seconds = 3;
    let project = tone(project_rate, 44_100 * seconds, 997.0, 0.1);
    let reference = tone(reference_rate, 48_000 * seconds, 997.0, 0.2);
    let result = compare_reference_audio(&ReferenceComparisonInput {
        project: source(&project, project_rate, 1.0),
        reference: source(&reference, reference_rate, 1.0),
        alignment: alignment(AlignmentMode::Disabled),
    })
    .unwrap();
    assert!(result.resampling.project.resampled);
    assert!(!result.resampling.reference.resampled);
    assert!(result.level_match.available);
    assert!((result.level_match.project_gain_to_reference_db.unwrap_or(0.0) - 6.0206).abs() < 0.1);
    assert!((result.deltas.project_minus_reference.integrated_loudness_lu.unwrap_or(0.0) + 6.0206).abs() < 0.1);
    assert!(!result.level_match.changes_audio);
}

#[test]
fn fails_auto_alignment_closed_for_an_ambiguous_steady_envelope_and_supports_explicit_manual_alignment() {
    let samples = tone(48_000.0, 48_000 * 2, 1_000.0, 0.1);
    let ambiguous = compare_reference_audio(&ReferenceComparisonInput {
        project: source(&samples, 48_000.0, 1.0),
        reference: source(&samples, 48_000.0, 1.0),
        alignment: Some(AlignmentOptions { mode: Some(AlignmentMode::Auto), max_lag_seconds: Some(0.5), manual_offset_seconds: None }),
    })
    .unwrap();
    assert!(!ambiguous.alignment.available);
    assert!(ambiguous.alignment.ambiguous);
    let reason = ambiguous.alignment.reason.clone().unwrap_or_default();
    assert!(reason.contains("variation") || reason.contains("weak") || reason.contains("competing"), "{reason}");
    assert_eq!(ambiguous.alignment.overlap_seconds, 0.0);
    assert!(!ambiguous.level_match.available);
    assert_eq!(ambiguous.level_match.project_gain_to_reference_db, None);
    assert_eq!(ambiguous.level_match.bounded_suggested_gain_db, None);
    assert!(ambiguous.deltas.project_minus_reference.values().iter().all(|value| value.is_none()));
    let manual = compare_reference_audio(&ReferenceComparisonInput {
        project: source(&samples, 48_000.0, 1.0),
        reference: source(&samples, 48_000.0, 1.0),
        alignment: Some(AlignmentOptions {
            mode: Some(AlignmentMode::Manual),
            max_lag_seconds: Some(0.5),
            manual_offset_seconds: Some(0.0),
        }),
    })
    .unwrap();
    assert!(manual.alignment.available);
    assert_eq!(manual.alignment.reference_offset_seconds, Some(0.0));
}

#[test]
fn enforces_pair_channel_duration_and_lag_bounds_before_unbounded_work() {
    let over_pair = vec![0.0f64; MAX_COMPARISON_TOTAL_SAMPLES];
    let one = [0.0f64];
    let message = |input: ReferenceComparisonInput| compare_reference_audio(&input).unwrap_err().0;
    assert!(message(ReferenceComparisonInput {
        project: source(&over_pair, 48_000.0, 1.0),
        reference: source(&one, 48_000.0, 1.0),
        alignment: None
    })
    .contains("pair limit"));
    assert!(message(ReferenceComparisonInput {
        project: source(&one, 48_000.0, 3.0),
        reference: source(&one, 48_000.0, 1.0),
        alignment: None
    })
    .contains("channels"));
    let too_long = vec![0.0f64; 32_000 * 31];
    assert!(message(ReferenceComparisonInput {
        project: source(&too_long, 32_000.0, 1.0),
        reference: source(&one, 48_000.0, 1.0),
        alignment: None
    })
    .contains("duration"));
    assert!(resample_pcm(&source(&[f64::NAN], 48_000.0, 1.0), None).unwrap_err().0.contains("finite normalized"));
    assert!(resample_pcm(&source(&one, 384_000.0, 1.0), None).unwrap_err().0.contains("32000 to 96000"));
    assert!(resample_pcm(&source(&one, 48_000.0, 1.0), Some(384_000.0)).unwrap_err().0.contains("fixed 48000"));
    assert!(message(ReferenceComparisonInput {
        project: source(&one, 48_000.0, 1.0),
        reference: source(&one, 48_000.0, 1.0),
        alignment: Some(AlignmentOptions { mode: None, max_lag_seconds: Some(11.0), manual_offset_seconds: None })
    })
    .contains("maxLagSeconds"));
    assert_eq!(
        message(ReferenceComparisonInput {
            project: source(&one, 48_000.0, 1.0),
            reference: source(&one, 48_000.0, 1.0),
            alignment: alignment(AlignmentMode::Manual)
        }),
        "alignment.manualOffsetSeconds must be finite and within maxLagSeconds"
    );
}
