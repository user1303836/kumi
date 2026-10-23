use std::f64::consts::PI;

use ableton_mcp_server::analysis::{analyze_pcm, PcmAnalysisInput};
use ableton_mcp_server::audio_standards::{
    analyze_standards_audio, ChannelLayout, ConventionalChannelLabel, StandardsAudioInput, STANDARDS_AUDIO_VERSION,
};
use kumi_common::js::number;

const SAMPLE_RATE: f64 = 48_000.0;

/// `Float32Array.from(...)`: each value is rounded to `f32` before the analysis widens it again.
fn programme(seconds: f64, channels: usize, sample: impl Fn(usize, usize, usize) -> f64) -> Vec<f64> {
    let frames = number::round(seconds * SAMPLE_RATE) as usize;
    (0..frames * channels).map(|index| sample(index / channels, index % channels, frames) as f32 as f64).collect()
}

fn faded_tone(seconds: f64, amplitude: f64, frequency: f64) -> Vec<f64> {
    programme(seconds, 2, |frame, _channel, frames| {
        let fade_frames = SAMPLE_RATE * 0.05;
        let fade = 1f64.min(frame as f64 / fade_frames).min((frames - 1 - frame) as f64 / fade_frames);
        amplitude * fade * (2.0 * PI * frequency * frame as f64 / SAMPLE_RATE).sin()
    })
}

fn stepped_tone(levels: &[f64], segment_seconds: f64) -> Vec<f64> {
    let total_seconds = levels.len() as f64 * segment_seconds;
    programme(total_seconds, 2, |frame, _, _| {
        let segment_frames = SAMPLE_RATE * segment_seconds;
        let segment = (levels.len() - 1).min((frame as f64 / segment_frames).floor() as usize);
        let local = frame as f64 - segment as f64 * segment_frames;
        let fade_frames = SAMPLE_RATE * 0.05;
        let fade = 1f64.min(local / fade_frames).min((segment_frames - 1.0 - local) / fade_frames);
        levels[segment] * fade * (2.0 * PI * 1_000.0 * frame as f64 / SAMPLE_RATE).sin()
    })
}

fn close(actual: Option<f64>, expected: f64, tolerance: f64, label: &str) {
    let actual = actual.unwrap_or_else(|| panic!("{label} should be available"));
    assert!((actual - expected).abs() <= tolerance, "{label}: expected {expected}±{tolerance}, got {actual}");
}

fn standards(samples: &[f64], sample_rate: f64, channels: f64) -> StandardsAudioInput<'_> {
    StandardsAudioInput { samples, sample_rate, channels, channel_layout: None }
}

// Expected values are independent FFmpeg 8.1 ebur128/libavfilter results for
// these generated programmes. See docs/evidence/phase-8-audio-oracle.json.
#[test]
fn matches_an_independent_bs1770_ebu_oracle_for_a_steady_stereo_programme() {
    let samples = faded_tone(10.0, 0.1, 1_000.0);
    let result = analyze_standards_audio(&standards(&samples, SAMPLE_RATE, 2.0)).unwrap();
    assert_eq!(result.version, STANDARDS_AUDIO_VERSION);
    assert!(result.loudness.standards_compliant);
    close(result.loudness.integrated_lufs, -20.0, 0.1, "integrated loudness");
    close(result.loudness.relative_gate_lufs, -30.0, 0.1, "relative gate");
    close(result.loudness.loudness_range.lra_lu, 0.0, 0.1, "loudness range");
    close(result.true_peak.aggregate_dbtp, -20.0, 0.1, "true peak");
    assert_eq!(
        result.channel_layout,
        ChannelLayout {
            labels: vec![ConventionalChannelLabel::L, ConventionalChannelLabel::R],
            weights: vec![1.0, 1.0],
            explicit: false,
            lfe_excluded: false
        }
    );
    assert!(result.loudness.momentary.series.len() <= 128);
    assert!(result.loudness.short_term.series.len() <= 128);
}

#[test]
fn matches_independent_absolute_relative_gating_and_lra_plateaus() {
    let samples = stepped_tone(&[0.01, 0.1, 0.0, 0.03162277660168379], 4.0);
    let gated = analyze_standards_audio(&standards(&samples, SAMPLE_RATE, 2.0)).unwrap();
    close(gated.loudness.integrated_lufs, -22.8, 0.1, "gated integrated loudness");
    close(gated.loudness.relative_gate_lufs, -34.4, 0.1, "gated relative threshold");
    close(gated.loudness.loudness_range.lra_lu, 20.0, 0.1, "gated loudness range");
    close(gated.loudness.loudness_range.low_lufs, -40.0, 0.1, "LRA low");
    close(gated.loudness.loudness_range.high_lufs, -20.0, 0.1, "LRA high");
    assert!(gated.loudness.blocks.above_relative_gate < gated.loudness.blocks.above_absolute_gate);

    let samples = stepped_tone(&[0.01, 0.1, 0.03162277660168379, 0.0031622776601683794, 0.05623413251903491], 5.0);
    let lra = analyze_standards_audio(&standards(&samples, SAMPLE_RATE, 2.0)).unwrap();
    close(lra.loudness.integrated_lufs, -23.4, 0.1, "LRA programme integrated loudness");
    close(lra.loudness.loudness_range.lra_lu, 20.0, 0.1, "LRA programme range");
}

#[test]
fn uses_the_published_annex_2_fir_and_detects_an_inter_sample_peak() {
    let samples = programme(5.0, 1, |frame, _channel, frames| {
        let fade = 1f64.min(frame as f64 / (SAMPLE_RATE * 0.1)).min((frames - 1 - frame) as f64 / (SAMPLE_RATE * 0.1));
        0.9 * fade * (2.0 * PI * 12_000.0 * frame as f64 / SAMPLE_RATE + PI / 4.0).sin()
    });
    let sample_peak = samples.iter().fold(0.0f64, |maximum, value| maximum.max(value.abs()));
    let result = analyze_standards_audio(&standards(&samples, SAMPLE_RATE, 1.0)).unwrap();
    close(result.true_peak.aggregate_dbtp, -0.9, 0.1, "Annex 2 true peak");
    assert!(20.0 * sample_peak.log10() < -3.8);
    assert!(result.true_peak.aggregate_dbtp.unwrap_or(-100.0) - 20.0 * sample_peak.log10() > 2.9);
    close(result.loudness.integrated_lufs, -0.6, 0.1, "high-frequency integrated loudness");
}

#[test]
fn does_not_manufacture_true_peak_overshoot_at_programme_boundaries() {
    let constant = vec![1.0f64; SAMPLE_RATE as usize];
    let result = analyze_standards_audio(&standards(&constant, SAMPLE_RATE, 1.0)).unwrap();
    close(result.true_peak.aggregate_dbtp, 0.0, 0.02, "constant full-scale true peak");
    let short = analyze_standards_audio(&standards(&[1.0, 1.0, 1.0], SAMPLE_RATE, 1.0)).unwrap();
    close(short.true_peak.aggregate_dbtp, 0.0, 0.01, "short full-scale true peak");
}

#[test]
fn rejects_non_finite_unbounded_empty_or_oversized_direct_standards_input() {
    let message = |samples: &[f64]| analyze_standards_audio(&standards(samples, SAMPLE_RATE, 1.0)).unwrap_err().0;
    assert!(message(&[f64::INFINITY]).contains("finite"));
    assert!(message(&[5.0]).contains("bounded"));
    assert!(message(&[]).contains("1-10000000"));
    assert!(message(&vec![0.0f64; 10_000_001]).contains("1-10000000"));
    assert_eq!(
        analyze_standards_audio(&standards(&[0.0], 1_000.0, 1.0)).unwrap_err().0,
        "sampleRate must be an integer from 8000 to 384000"
    );
    assert_eq!(
        analyze_standards_audio(&standards(&[0.0, 0.0], SAMPLE_RATE, 1.5)).unwrap_err().0,
        "channels and samples must contain 1-10000000 complete frames across at most 32 channels"
    );
}

#[test]
fn requires_semantic_multichannel_layout_excludes_lfe_and_weights_surround() {
    use ConventionalChannelLabel::{Ls, Rs, C, L, LFE, R};
    let frames = SAMPLE_RATE as usize;
    let layout = [L, R, C, LFE, Ls, Rs];
    let mut lfe_only = vec![0.0f64; frames * layout.len()];
    for frame in 0..frames {
        lfe_only[frame * layout.len() + 3] = (0.5 * (2.0 * PI * 100.0 * frame as f64 / SAMPLE_RATE).sin()) as f32 as f64;
    }
    let with_layout = |samples: &[f64]| {
        analyze_standards_audio(&StandardsAudioInput {
            samples,
            sample_rate: SAMPLE_RATE,
            channels: layout.len() as f64,
            channel_layout: Some(&layout),
        })
        .unwrap()
    };
    let excluded = with_layout(&lfe_only);
    assert_eq!(excluded.loudness.integrated_lufs, None);
    assert!(excluded.channel_layout.lfe_excluded);

    let no_layout = analyze_standards_audio(&standards(&lfe_only, SAMPLE_RATE, layout.len() as f64)).unwrap();
    assert!(!no_layout.loudness.available);
    assert!(no_layout.loudness.reason.as_deref().unwrap_or("").contains("channelLayout is required"));

    let mut left = vec![0.0f64; frames * layout.len()];
    let mut surround = vec![0.0f64; frames * layout.len()];
    for frame in 0..frames {
        let value = (0.1 * (2.0 * PI * 1_000.0 * frame as f64 / SAMPLE_RATE).sin()) as f32 as f64;
        left[frame * layout.len()] = value;
        surround[frame * layout.len() + 4] = value;
    }
    let left_result = with_layout(&left);
    let surround_result = with_layout(&surround);
    close(
        Some(surround_result.loudness.integrated_lufs.unwrap_or(0.0) - left_result.loudness.integrated_lufs.unwrap_or(0.0)),
        10.0 * 1.41f64.log10(),
        0.01,
        "surround weighting",
    );
    // The layout rules each have their own reason.
    let reason = |layout: &[ConventionalChannelLabel], channels: usize| {
        let samples = vec![0.0f64; frames * channels];
        analyze_standards_audio(&StandardsAudioInput {
            samples: &samples,
            sample_rate: SAMPLE_RATE,
            channels: channels as f64,
            channel_layout: Some(layout),
        })
        .unwrap()
        .loudness
        .reason
        .unwrap_or_default()
    };
    assert_eq!(reason(&[L, R], 3), "channelLayout must contain exactly one semantic label per channel");
    assert_eq!(reason(&[L, L], 2), "channelLayout labels must be unique");
    assert_eq!(reason(&[ConventionalChannelLabel::M, L], 2), "the mono channel label M is valid only for one-channel input");
    assert_eq!(reason(&[LFE], 1), "channelLayout must contain at least one programme channel; LFE is excluded from loudness");
}

#[test]
fn returns_explicit_unavailable_states_for_short_material_and_unvalidated_true_peak_rates() {
    let short = analyze_standards_audio(&standards(&vec![0.0f64; 1_000], SAMPLE_RATE, 1.0)).unwrap();
    assert!(!short.loudness.available);
    assert_eq!(short.loudness.integrated_lufs, None);
    let silence = analyze_standards_audio(&standards(&vec![0.0f64; SAMPLE_RATE as usize], SAMPLE_RATE, 1.0)).unwrap();
    assert!(!silence.loudness.available);
    assert_eq!(silence.loudness.integrated_lufs, None);
    assert!(silence.loudness.reason.as_deref().unwrap_or("").contains("absolute loudness gate"));
    assert!(!short.loudness.loudness_range.available);
    let tone_44100: Vec<f64> =
        (0..44_100).map(|frame| (0.1 * (2.0 * PI * 1_000.0 * frame as f64 / 44_100.0).sin()) as f32 as f64).collect();
    let rate44100 = analyze_standards_audio(&standards(&tone_44100, 44_100.0, 1.0)).unwrap();
    assert!(rate44100.true_peak.available);
    close(rate44100.true_peak.aggregate_dbtp, -20.0, 0.15, "44.1 kHz true peak");
    assert!(rate44100.loudness.standards_compliant);
    assert!(rate44100.loudness.integrated_lufs.is_some_and(f64::is_finite));
    let other_rate = analyze_standards_audio(&standards(&vec![0.0f64; 96_000], 96_000.0, 1.0)).unwrap();
    assert!(!other_rate.true_peak.available);
    assert!(other_rate.true_peak.reason.as_deref().unwrap_or("").contains("44.1 and 48 kHz"));
    assert_eq!(other_rate.true_peak.oversampling_factor, None);
    assert_eq!(rate44100.true_peak.method, "64-tap Blackman-sinc 44.1-to-48 kHz then Annex 2 FIR");
    assert_eq!(silence.true_peak.method, "ITU-R BS.1770-5 Annex 2 order-48 four-phase FIR");
    let oversized = analyze_standards_audio(&standards(&vec![0.0f64; 2_880_001], SAMPLE_RATE, 1.0)).unwrap();
    assert_eq!(
        oversized.true_peak.reason.as_deref(),
        Some("true-peak input exceeds the bounded 1440000-frame/2880000-sample analysis limit")
    );
}

#[test]
fn snapshots_mutable_direct_array_like_input_once() {
    // A slice is read once per sample by construction; the TypeScript's observable Proxy has no Rust
    // counterpart. The analysis of that one read is what it asserted.
    let source = vec![0.1f64; SAMPLE_RATE as usize];
    let result = analyze_standards_audio(&standards(&source, SAMPLE_RATE, 1.0)).unwrap();
    assert!(result.loudness.integrated_lufs.is_some_and(f64::is_finite));
}

#[test]
fn pcm_analysis_v3_retains_the_named_compatibility_proxy_but_exposes_standards_separately() {
    let samples = faded_tone(1.0, 0.1, 1_000.0);
    let result = analyze_pcm(&PcmAnalysisInput {
        samples: &samples,
        sample_rate: SAMPLE_RATE,
        channels: Some(2.0),
        channel_layout: None,
        frame_size: None,
    })
    .unwrap();
    assert_eq!(result.version, "pcm-analysis/v3");
    assert_eq!(result.loudness.method, "rms-derived-proxy");
    assert!(!result.loudness.standards_compliant);
    assert!(result.standards_audio.loudness.standards_compliant);
    assert_ne!(result.standards_audio.loudness.integrated_lufs, None);
}

#[test]
fn serializes_the_true_peak_keys_in_the_order_the_typescript_wrote_them() {
    let result = analyze_standards_audio(&standards(&[1.0, 1.0, 1.0], SAMPLE_RATE, 1.0)).unwrap();
    let json = kumi_common::js::json::stringify(&serde_json::to_value(&result.true_peak).unwrap());
    assert!(json.starts_with("{\"sampleRate\":48000,\"method\":\"ITU-R BS.1770-5 Annex 2 order-48 four-phase FIR\",\"maxFrames\":1440000,\"maxSamples\":2880000,\"available\":true,\"standardsCompliant\":true,\"aggregateDbtp\":"), "{json}");
    assert!(json.ends_with(",\"oversamplingFactor\":4}"), "{json}");
    let loudness = kumi_common::js::json::stringify(&serde_json::to_value(&result.loudness).unwrap());
    assert!(loudness.starts_with("{\"available\":false,\"reason\":\"standards integrated loudness requires at least one complete 400 ms block\",\"standardsCompliant\":true,\"integratedLufs\":null,\"absoluteGateLufs\":-70,\"relativeGateLufs\":null,\"blocks\":{\"total\":0,\"aboveAbsoluteGate\":0,\"aboveRelativeGate\":0},\"momentary\":{\"windowSeconds\":0.4,\"cadenceSeconds\":0.1,\"currentLufs\":null,\"maximumLufs\":null,\"series\":[],\"seriesLossy\":true}"), "{loudness}");
}
