#[path = "support/fixtures.rs"]
mod fixtures;

use std::f64::consts::PI;

use ableton_mcp_server::analysis::{
    analyze_pcm, analyze_reconstructed_pcm, decode_float32_le, PcmAnalysisInput, Performance, Privacy, ReconstructedOvers, Safety,
    MAX_ANALYSIS_CHANNELS, MAX_ANALYSIS_SAMPLES, MAX_ANALYSIS_SECONDS, MAX_FFT_SIZE, MAX_SPECTRAL_FRAMES, MAX_TIME_FREQUENCY_BANDS,
    MAX_TIME_FREQUENCY_FRAMES, MAX_WAVEFORM_BINS,
};
use base64::Engine;
use fixtures::{as_f64, dc_fixture, impulse_fixture, silence_fixture, sine_fixture, stereo_fixture, sweep_fixture};

fn input(samples: &[f64], sample_rate: f64) -> PcmAnalysisInput<'_> {
    PcmAnalysisInput { samples, sample_rate, channels: None, channel_layout: None, frame_size: None }
}

fn with_channels(samples: &[f64], sample_rate: f64, channels: f64) -> PcmAnalysisInput<'_> {
    PcmAnalysisInput { samples, sample_rate, channels: Some(channels), channel_layout: None, frame_size: None }
}

fn with_frame(samples: &[f64], sample_rate: f64, frame_size: f64) -> PcmAnalysisInput<'_> {
    PcmAnalysisInput { samples, sample_rate, channels: None, channel_layout: None, frame_size: Some(frame_size) }
}

fn base64(bytes: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

fn has(analysis: &ableton_mcp_server::analysis::PcmAnalysis, id: &str) -> bool {
    analysis.remediation.iter().any(|item| item.id == id)
}

#[test]
fn deterministically_analyzes_a_fixture_without_exposing_audio() {
    let fixture = as_f64(&sine_fixture(4096, 440.0, 48000.0, 0.5));
    let first = analyze_pcm(&input(&fixture, 48000.0)).unwrap();
    let second = analyze_pcm(&input(&fixture, 48000.0)).unwrap();
    assert_eq!(first, second);
    assert!((first.peak - 0.5).abs() < 0.001);
    assert!((first.spectral.dominant_frequency_hz - 440.625).abs() < 50.0);
    assert!(!first.privacy.raw_audio_retained);
    assert!(!first.privacy.raw_audio_returned);
    assert!(!first.safety.playback_started);
    assert!(!first.safety.project_mutated);
    assert_eq!(
        first.performance,
        Performance {
            bounded: true,
            max_samples: MAX_ANALYSIS_SAMPLES,
            max_seconds: 600.0,
            max_spectral_frames: MAX_SPECTRAL_FRAMES,
            max_fft_size: MAX_FFT_SIZE
        }
    );
    assert_eq!(MAX_ANALYSIS_SECONDS, 600.0);
}

#[test]
fn reports_clipping_and_bounded_reversible_remediation() {
    let samples = as_f64(&[0.0f32, 1.0, -1.0, 0.5]);
    let result = analyze_pcm(&input(&samples, 44100.0)).unwrap();
    assert_eq!((result.clipping.count, result.clipping.ratio), (2, 0.5));
    assert!(!result.reconstructed_overs.applicable);
    assert_eq!(result.reconstructed_overs.count, 0);
    assert!(!result.channels_detail[0].reconstructed_overs.applicable);
    assert!(has(&result, "reduce-clipping"));
    assert!(!has(&result, "inspect-reconstructed-overs"));
    assert!(result.remediation.iter().all(|item| item.reversible && !item.changes_audio));
    assert_eq!(result.performance.max_samples, MAX_ANALYSIS_SAMPLES);
}

#[test]
fn separates_reconstructed_overs_and_preserves_their_dynamics_histogram() {
    let samples: Vec<f64> = (0..100)
        .map(|index| {
            if index < 90 {
                0.5
            } else if index % 2 == 0 {
                1.25
            } else {
                -1.25
            }
        })
        .collect();
    let result = analyze_reconstructed_pcm(&input(&samples, 48_000.0)).unwrap();
    assert_eq!((result.clipping.count, result.clipping.ratio), (0, 0.0));
    assert_eq!(result.reconstructed_overs, ReconstructedOvers { count: 10, ratio: 0.1, threshold: 1, applicable: true, reason: None });
    assert_eq!(result.channels_detail[0].clipping.count, 0);
    assert_eq!(result.channels_detail[0].reconstructed_overs.count, 10);
    assert!(!has(&result, "reduce-clipping"));
    assert!(has(&result, "inspect-reconstructed-overs"));
    let expected_dynamic_range_db = 20.0 * (1.25f64 / 0.5).log10();
    assert!(
        (result.dynamics.dynamic_range_db - expected_dynamic_range_db).abs() < 0.01,
        "unexpected reconstructed dynamic range {}",
        result.dynamics.dynamic_range_db
    );
}

#[test]
fn rejects_unsafe_and_malformed_input() {
    let message = |result: Result<_, ableton_mcp_server::analysis::RangeError>| result.unwrap_err().0;
    assert!(message(analyze_pcm(&input(&[1.1], 44100.0))).contains("normalized"));
    assert!(message(analyze_pcm(&input(&[0.0], 1000.0))).contains("sampleRate"));
    assert!(message(analyze_pcm(&input(&[], 44100.0))).contains("samples"));
    let decode_message = |text: &str| decode_float32_le(text).unwrap_err().0;
    let not_base64 = decode_message("not base64");
    assert!(not_base64.contains("float32") || not_base64.contains("invalid"));
    assert!(decode_message("AA=A").contains("invalid"));
    assert!(decode_message("Zh==").contains("invalid"));
    let non_finite = 0x7fc00000u32.to_le_bytes();
    assert!(decode_message(&base64(&non_finite)).contains("finite normalized"));
    let out_of_range = 1.01f32.to_le_bytes();
    assert!(decode_message(&base64(&out_of_range)).contains("finite normalized"));
    assert!(message(analyze_pcm(&with_channels(&[0.0, 0.0, 0.0], 44100.0, 2.0))).contains("complete channel frames"));
    assert!(message(analyze_pcm(&input(&[f64::NAN], 44100.0))).contains("finite"));
    assert!(message(analyze_pcm(&input(&[f64::INFINITY], 44100.0))).contains("finite"));
    // The number-typed arguments are checked the way the TypeScript checked them.
    assert_eq!(message(analyze_pcm(&with_channels(&[0.0], 44100.0, 1.5))), "channels must be an integer from 1 to 32");
    assert_eq!(message(analyze_pcm(&with_frame(&[0.0], 44100.0, 100.0))), "frameSize must be an integer from 256 to 4096");
    assert_eq!(message(analyze_pcm(&input(&[0.0], f64::NAN))), "sampleRate must be finite");
}

#[test]
fn enforces_sample_and_duration_limits_before_reading_untrusted_sample_storage() {
    // A negative `length` has no Rust spelling; the two bounds that do are checked before any sample is read.
    let too_many_samples = vec![0.0f64; MAX_ANALYSIS_SAMPLES + 1];
    let too_long = vec![0.0f64; 8_000 * 600 + 1];
    assert!(analyze_pcm(&input(&too_many_samples, 8_000.0)).unwrap_err().0.contains("samples must contain"));
    assert!(analyze_pcm(&input(&too_long, 8_000.0)).unwrap_err().0.contains("duration exceeds"));
}

#[test]
fn keeps_spectral_work_bounded_and_remediation_advisory_at_each_threshold() {
    let samples = as_f64(&sine_fixture(4096 * 33, 440.0, 48_000.0, 0.5));
    let result = analyze_pcm(&with_frame(&samples, 48_000.0, 4096.0)).unwrap();
    assert_eq!(result.spectral.analyzed_frames, MAX_SPECTRAL_FRAMES);
    assert_eq!(result.spectral.fft_size, MAX_FFT_SIZE);
    assert_eq!(result.remediation.len(), 0);
    let loud = analyze_pcm(&input(&as_f64(&[0.99f32, -0.99, 0.5]), 44_100.0)).unwrap();
    assert!(has(&loud, "leave-headroom"));
    assert!(has(&loud, "check-loudness"));
}

#[test]
fn does_not_dilute_spectral_measurements_with_silent_sampled_frames() {
    let mut fixture = vec![0f32; 4096 * 2];
    fixture[..2048].copy_from_slice(&sine_fixture(2048, 440.0, 48000.0, 0.5));
    let result = analyze_pcm(&with_frame(&as_f64(&fixture), 48000.0, 2048.0)).unwrap();
    assert!(result.spectral.centroid_hz > 300.0);
    assert!(result.spectral.centroid_hz < 2_000.0);
}

#[test]
fn keeps_impulse_remediation_advisory_and_bounded() {
    let result = analyze_pcm(&input(&as_f64(&impulse_fixture(4096, 1.0)), 44100.0)).unwrap();
    assert!(has(&result, "reduce-clipping"));
    assert!(result.remediation.iter().all(|item| item.reversible && !item.changes_audio));
    assert_eq!(result.privacy, Privacy { raw_audio_retained: false, raw_audio_returned: false, source_path_accepted: false });
    assert_eq!(result.safety, Safety { playback_started: false, project_mutated: false, destructive_action_required: false });
}

#[test]
fn accepts_the_exact_maximum_pcm_payload_size() {
    let bytes = vec![0u8; MAX_ANALYSIS_SAMPLES * 4];
    let decoded = decode_float32_le(&base64(&bytes)).unwrap();
    assert_eq!(decoded.len(), MAX_ANALYSIS_SAMPLES);
}

#[test]
fn deinterleaves_channels_for_spectral_analysis_and_handles_silence() {
    let mut stereo = vec![0f32; 4096 * 2];
    for frame in 0..4096 {
        stereo[frame * 2] = (0.5 * ((2.0 * PI * 440.0 * frame as f64) / 48000.0).sin()) as f32;
    }
    let result = analyze_pcm(&with_channels(&as_f64(&stereo), 48000.0, 2.0)).unwrap();
    assert!((result.spectral.dominant_frequency_hz - 440.625).abs() < 50.0);
    let silence = analyze_pcm(&input(&vec![0.0; 4096], 48000.0)).unwrap();
    assert_eq!(silence.spectral.dominant_frequency_hz, 0.0);
    assert_eq!(silence.spectral.centroid_hz, 0.0);
}

#[test]
fn preserves_spectral_evidence_for_antiphase_stereo() {
    let mut stereo = vec![0f32; 4096 * 2];
    for frame in 0..4096 {
        let sample = (0.5 * ((2.0 * PI * 440.0 * frame as f64) / 48000.0).sin()) as f32;
        stereo[frame * 2] = sample;
        stereo[frame * 2 + 1] = -sample;
    }
    let result = analyze_pcm(&with_channels(&as_f64(&stereo), 48000.0, 2.0)).unwrap();
    assert!(result.spectral.dominant_frequency_hz > 300.0);
}

#[test]
fn reports_bounded_channel_metrics_and_stereo_phase_correlation() {
    let samples = as_f64(&[1.0f32, 1.0, -1.0, -1.0, 0.5, -0.5, 0.0, 0.0]);
    let result = analyze_pcm(&with_channels(&samples, 48_000.0, 2.0)).unwrap();
    assert_eq!(result.channels_detail.len(), 2);
    assert_eq!(result.channels_detail[0].clipping.count, 2);
    assert_eq!(result.channels_detail[1].dc_offset, -0.125);
    assert!((result.stereo.phase_correlation.unwrap_or(0.0) - 7.0 / 9.0).abs() < 1e-12);
    assert!(!result.loudness.standards_compliant);
    assert!(result.loudness.deprecated_integrated_lufs_estimate);
}

#[test]
fn covers_deterministic_mono_dc_unequal_level_independent_and_antiphase_fixtures() {
    let mono = analyze_pcm(&input(&as_f64(&sine_fixture(2048, 440.0, 48_000.0, 0.5)), 48_000.0)).unwrap();
    assert!((mono.channels_detail[0].rms - 0.3535).abs() < 0.01);
    let dc = analyze_pcm(&input(&as_f64(&dc_fixture(2048, 0.25)), 48_000.0)).unwrap();
    assert_eq!(dc.channels_detail[0].dc_offset, 0.25);
    let unequal = analyze_pcm(&with_channels(&as_f64(&stereo_fixture(2048, |_| 0.5, |_| 0.125)), 48_000.0, 2.0)).unwrap();
    assert_eq!(unequal.channels_detail[0].peak, 0.5);
    assert_eq!(unequal.channels_detail[1].peak, 0.125);
    assert_eq!(unequal.stereo.phase_correlation, Some(1.0));
    let independent = analyze_pcm(&with_channels(
        &as_f64(&stereo_fixture(
            2048,
            |frame| ((2.0 * PI * 220.0 * frame as f64) / 48_000.0).sin(),
            |frame| ((2.0 * PI * 880.0 * frame as f64) / 48_000.0).sin(),
        )),
        48_000.0,
        2.0,
    ))
    .unwrap();
    assert!(independent.stereo.phase_correlation.unwrap_or(0.0) < 0.9);
    let antiphase = analyze_pcm(&with_channels(
        &as_f64(&stereo_fixture(
            2048,
            |frame| ((2.0 * PI * 220.0 * frame as f64) / 48_000.0).sin(),
            |frame| -((2.0 * PI * 220.0 * frame as f64) / 48_000.0).sin(),
        )),
        48_000.0,
        2.0,
    ))
    .unwrap();
    assert!((antiphase.stereo.phase_correlation.unwrap_or(0.0) + 1.0).abs() < 1e-12);
}

#[test]
fn reports_explicit_not_applicable_correlation_and_supports_the_maximum_channel_count() {
    let samples = vec![0.0f64; MAX_ANALYSIS_CHANNELS * 256];
    let result = analyze_pcm(&with_channels(&samples, 48_000.0, MAX_ANALYSIS_CHANNELS as f64)).unwrap();
    assert_eq!(result.channels_detail.len(), MAX_ANALYSIS_CHANNELS);
    assert_eq!(result.stereo.phase_correlation, None);
    assert!(result.stereo.reason.as_deref().unwrap_or("").contains("only to stereo"));
    assert!(result
        .channels_detail
        .iter()
        .all(|detail| detail.rms.is_finite() && detail.clipping.ratio >= 0.0 && detail.clipping.ratio <= 1.0));
}

#[test]
fn validates_mutable_array_like_storage_once_and_keeps_the_result_deterministic() {
    // A slice has no observable getters: the one read per sample is the only read there can be. The
    // analysis of the snapshot is what the TypeScript asserted after its single read.
    let mut samples = vec![0.0f64; 2048];
    samples[0] = 0.5;
    let result = analyze_pcm(&with_frame(&samples, 48_000.0, 1024.0)).unwrap();
    assert_eq!(result.peak, 0.5);
    assert!(!result.safety.project_mutated);
}

#[test]
fn returns_bounded_lossy_waveform_logarithmic_time_frequency_and_transient_summaries() {
    let result = analyze_pcm(&with_frame(&as_f64(&sine_fixture(48_000, 440.0, 48_000.0, 0.5)), 48_000.0, 1024.0)).unwrap();
    assert!(result.waveform.bin_count <= MAX_WAVEFORM_BINS);
    assert!(result.waveform.bins.len() <= MAX_WAVEFORM_BINS);
    assert_eq!(result.waveform.channel_aggregation, "per-channel");
    assert!(result.waveform.lossy);
    assert!(result
        .waveform
        .bins
        .iter()
        .all(|bin| bin.channels.iter().all(|channel| channel.min <= channel.max && channel.rms.is_finite())));
    assert!(result.time_frequency.frame_count <= MAX_TIME_FREQUENCY_FRAMES);
    assert_eq!(result.time_frequency.band_count, MAX_TIME_FREQUENCY_BANDS);
    assert_eq!(result.time_frequency.channel_aggregation, "per-channel-and-aggregate");
    assert_eq!(result.time_frequency.normalization, "mean-square-per-frame");
    assert_eq!(result.time_frequency.window, "hann");
    assert_eq!(result.time_frequency.hop_samples, 1024);
    assert!(result.time_frequency.lossy);
    assert!(result.time_frequency.frames.iter().all(|frame| frame.bands.len() == MAX_TIME_FREQUENCY_BANDS
        && frame.bands.iter().all(|band| band.low_hz < band.high_hz
            && band.high_hz <= result.sample_rate / 2.0
            && band.energy.is_finite()
            && band.channels.len() == 1)));
    assert!(result.time_frequency.frames.iter().any(|frame| frame.bands.iter().any(|band| band.energy > 0.0)));
    let tone_frame = &result.time_frequency.frames[result.time_frequency.frames.len() / 2];
    let strongest_band =
        tone_frame.bands.iter().fold(&tone_frame.bands[0], |strongest, band| if band.energy > strongest.energy { band } else { strongest });
    assert!(strongest_band.low_hz <= 440.0 && strongest_band.high_hz >= 440.0);
    let sweep = analyze_pcm(&with_frame(&as_f64(&sweep_fixture(48_000, 220.0, 1_760.0, 48_000.0, 0.5)), 48_000.0, 1024.0)).unwrap();
    assert!(sweep.time_frequency.frames.iter().any(|frame| frame.bands.iter().any(|band| band.energy > 0.0)));
}

#[test]
fn never_emits_one_waveform_bin_per_source_frame_including_shortest_inputs() {
    for frame_count in [1usize, 2, 3, 257, 1024] {
        let result = analyze_pcm(&input(&vec![0.0f64; frame_count], 48_000.0)).unwrap();
        assert!(result.waveform.bins.len() < frame_count);
        assert_eq!(result.waveform.bin_count, result.waveform.bins.len());
    }
}

#[test]
fn independent_golden_checks_keep_logarithmic_energy_finite_and_tone_localized() {
    let sample_rate = 48_000.0;
    let frequency = 1_000.0;
    let amplitude = 0.5;
    let samples = as_f64(&sine_fixture(16_384, frequency, sample_rate, amplitude));
    let result = analyze_pcm(&with_frame(&samples, sample_rate, 1024.0)).unwrap();
    let frame = &result.time_frequency.frames[result.time_frequency.frames.len() / 2];
    let containing = frame.bands.iter().find(|band| band.low_hz <= frequency && frequency < band.high_hz).expect("a band holds the tone");
    // Independently calculated RMS power for a full-scale sine is A²/2. The
    // Hann window and logarithmic band aggregation retain a bounded fraction
    // of that energy, while silence remains exactly zero.
    assert!(containing.energy > (amplitude * amplitude) / 100.0);
    assert!(frame.bands.iter().all(|band| band.energy.is_finite() && band.energy_db.is_finite()));
    let silence = analyze_pcm(&with_frame(&vec![0.0f64; 16_384], sample_rate, 1024.0)).unwrap();
    assert!(silence.time_frequency.frames.iter().all(|item| item.bands.iter().all(|band| band.energy == 0.0)));
}

#[test]
fn keeps_waveform_and_time_frequency_channel_separation_deterministic() {
    let stereo = as_f64(&stereo_fixture(
        8192,
        |frame| 0.5 * ((2.0 * PI * 220.0 * frame as f64) / 48_000.0).sin(),
        |frame| 0.5 * ((2.0 * PI * 1760.0 * frame as f64) / 48_000.0).sin(),
    ));
    let analyze = || {
        analyze_pcm(&PcmAnalysisInput {
            samples: &stereo,
            sample_rate: 48_000.0,
            channels: Some(2.0),
            channel_layout: None,
            frame_size: Some(1024.0),
        })
        .unwrap()
    };
    let result = analyze();
    let first_bin = result.waveform.bins.first().expect("a waveform bin");
    assert_ne!(first_bin.channels[0], first_bin.channels[1]);
    let band_with_separation =
        result.time_frequency.frames[0].bands.iter().find(|band| (band.channels[0] - band.channels[1]).abs() > 0.001);
    assert!(band_with_separation.is_some());
    assert_eq!(result, analyze());
}

#[test]
fn summarizes_transients_without_making_event_or_mastering_claims() {
    let result = analyze_pcm(&input(&as_f64(&impulse_fixture(48_000, 0.8)), 48_000.0)).unwrap();
    assert_eq!(result.transients.peak_count, 1);
    assert_eq!(result.transients.strongest.as_ref().map(|strongest| strongest.sample_index), Some(0));
    assert!((result.transients.strongest.as_ref().map_or(0.0, |strongest| strongest.amplitude) - 0.8).abs() < 1e-6);
    assert_eq!(result.time_frequency.method, "hann-windowed-fft");
    assert!(!result.loudness.standards_compliant);
    let silence = analyze_pcm(&input(&as_f64(&silence_fixture(2048)), 48_000.0)).unwrap();
    assert!(silence
        .time_frequency
        .frames
        .iter()
        .all(|frame| frame.bands.iter().all(|band| band.energy == 0.0 && band.energy_db <= -200.0)));
}

#[test]
fn serializes_as_the_typescript_did() {
    // JavaScript's JSON: camelCase keys in declaration order, integers without a fraction, optional
    // keys left out, nullable ones written as null.
    let samples = as_f64(&[0.0f32, 1.0, -1.0, 0.5]);
    let result = analyze_pcm(&input(&samples, 44100.0)).unwrap();
    let json = kumi_common::js::json::stringify(&serde_json::to_value(&result).unwrap());
    assert!(json.starts_with("{\"version\":\"pcm-analysis/v3\",\"sampleRate\":44100,\"channels\":1,\"durationSeconds\":"));
    assert!(json.contains("\"clipping\":{\"count\":2,\"ratio\":0.5}"));
    assert!(json.contains("\"stereo\":{\"phaseCorrelation\":null,\"reason\":\"phase correlation is applicable only to stereo input\"}"));
    assert!(json.contains("\"reconstructedOvers\":{\"count\":0,\"ratio\":0,\"threshold\":1,\"applicable\":false,\"reason\":\"input contains normalized source samples, not band-limited reconstruction\"}"));
    assert!(json.contains(
        "\"performance\":{\"bounded\":true,\"maxSamples\":10000000,\"maxSeconds\":600,\"maxSpectralFrames\":32,\"maxFftSize\":4096}"
    ));
    assert!(json.contains("\"reversible\":true,\"changesAudio\":false"));
    let round_trip: ableton_mcp_server::analysis::PcmAnalysis = serde_json::from_str(&json).unwrap();
    assert_eq!(round_trip, result);
}
