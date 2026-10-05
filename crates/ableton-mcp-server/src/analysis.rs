//! Deterministic, local-only analysis of normalized PCM samples.
//!
//! This module deliberately accepts samples rather than paths or URLs. It never
//! retains or returns audio data; callers receive aggregate measurements only.

use std::f64::consts::PI;

use base64::Engine;
use kumi_common::js::{number, string};
use serde::{Deserialize, Serialize};

pub use crate::audio_standards::RangeError;
use crate::audio_standards::{analyze_standards_audio, is_integer, ConventionalChannelLabel, StandardsAudioAnalysis, StandardsAudioInput};

pub const ANALYSIS_VERSION: &str = "pcm-analysis/v3";
pub const MAX_ANALYSIS_SAMPLES: usize = 10_000_000;
pub const MAX_ANALYSIS_SECONDS: f64 = 600.0;
pub const MAX_ANALYSIS_CHANNELS: usize = 32;
pub const MAX_SPECTRAL_FRAMES: usize = 32;
pub const MAX_FFT_SIZE: usize = 4096;
pub const MAX_WAVEFORM_BINS: usize = 256;
pub const MAX_TIME_FREQUENCY_FRAMES: usize = 32;
pub const MAX_TIME_FREQUENCY_BANDS: usize = 24;

/// The input as the TypeScript took it: `sampleRate`, `channels` and `frameSize` are numbers that
/// the analysis itself checks to be integers in range.
#[derive(Debug, Clone, Copy)]
pub struct PcmAnalysisInput<'a> {
    pub samples: &'a [f64],
    pub sample_rate: f64,
    pub channels: Option<f64>,
    pub channel_layout: Option<&'a [ConventionalChannelLabel]>,
    pub frame_size: Option<f64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RemediationSeverity {
    Info,
    Warning,
    Critical,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AudioRemediation {
    pub id: String,
    pub severity: RemediationSeverity,
    pub reason: String,
    pub action: String,
    pub reversible: bool,
    pub changes_audio: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BoundaryCount {
    pub count: usize,
    pub ratio: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReconstructedOvers {
    pub count: usize,
    pub ratio: f64,
    pub threshold: usize,
    pub applicable: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WaveformChannel {
    pub min: f64,
    pub max: f64,
    pub rms: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WaveformEnvelope {
    pub start_seconds: f64,
    pub end_seconds: f64,
    pub channels: Vec<WaveformChannel>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TimeFrequencyBand {
    pub low_hz: f64,
    pub high_hz: f64,
    pub center_hz: f64,
    pub energy: f64,
    pub energy_db: f64,
    pub channels: Vec<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TimeFrequencyFrame {
    pub start_seconds: f64,
    pub end_seconds: f64,
    pub bands: Vec<TimeFrequencyBand>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChannelDetail {
    pub channel: usize,
    pub peak: f64,
    pub peak_dbfs: f64,
    pub rms: f64,
    pub rms_dbfs: f64,
    pub dc_offset: f64,
    pub clipping: BoundaryCount,
    pub reconstructed_overs: ReconstructedOvers,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StereoAnalysis {
    pub phase_correlation: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// Compatibility-only RMS proxy. Use `standards_audio.loudness`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LoudnessProxy {
    pub rms_loudness_proxy_db: f64,
    pub integrated_lufs_estimate: f64,
    pub method: String,
    pub standards_compliant: bool,
    pub deprecated_integrated_lufs_estimate: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Dynamics {
    pub crest_factor_db: f64,
    pub dynamic_range_db: f64,
    pub silence_ratio: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Spectral {
    pub centroid_hz: f64,
    pub dominant_frequency_hz: f64,
    pub analyzed_frames: usize,
    pub fft_size: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Waveform {
    pub bins: Vec<WaveformEnvelope>,
    pub bin_count: usize,
    pub channel_aggregation: String,
    pub lossy: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FrequencyRange {
    pub min: f64,
    pub max: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TimeFrequency {
    pub frames: Vec<TimeFrequencyFrame>,
    pub frame_count: usize,
    pub band_count: usize,
    pub frequency_range_hz: FrequencyRange,
    pub method: String,
    pub window: String,
    pub hop_samples: usize,
    pub channel_aggregation: String,
    pub normalization: String,
    pub lossy: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StrongestTransient {
    pub sample_index: usize,
    pub time_seconds: f64,
    pub amplitude: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Transients {
    pub peak_count: usize,
    pub density_per_second: f64,
    pub threshold: f64,
    pub strongest: Option<StrongestTransient>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Privacy {
    pub raw_audio_retained: bool,
    pub raw_audio_returned: bool,
    pub source_path_accepted: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Safety {
    pub playback_started: bool,
    pub project_mutated: bool,
    pub destructive_action_required: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Performance {
    pub bounded: bool,
    pub max_samples: usize,
    pub max_seconds: f64,
    pub max_spectral_frames: usize,
    pub max_fft_size: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PcmAnalysis {
    pub version: String,
    pub sample_rate: f64,
    pub channels: usize,
    pub duration_seconds: f64,
    pub sample_count: usize,
    pub peak: f64,
    pub peak_dbfs: f64,
    pub rms: f64,
    pub rms_dbfs: f64,
    pub channels_detail: Vec<ChannelDetail>,
    pub stereo: StereoAnalysis,
    /// Compatibility-only RMS proxy. Use `standards_audio.loudness`.
    pub loudness: LoudnessProxy,
    pub standards_audio: StandardsAudioAnalysis,
    pub dynamics: Dynamics,
    pub clipping: BoundaryCount,
    pub reconstructed_overs: ReconstructedOvers,
    pub spectral: Spectral,
    pub waveform: Waveform,
    pub time_frequency: TimeFrequency,
    pub transients: Transients,
    pub privacy: Privacy,
    pub safety: Safety,
    pub performance: Performance,
    pub remediation: Vec<AudioRemediation>,
}

const EPSILON: f64 = 1e-12;

fn finite(value: f64, label: &str) -> Result<(), RangeError> {
    if !value.is_finite() {
        return Err(RangeError(format!("{label} must be finite")));
    }
    Ok(())
}

fn db(value: f64) -> f64 {
    20.0 * value.max(EPSILON).log10()
}

fn next_power_of_two(value: usize) -> usize {
    let mut result = 1;
    while result < value {
        result *= 2;
    }
    result
}

fn fft_magnitudes(samples: &[f64], start: usize, frame_size: usize) -> (Vec<f64>, usize) {
    let fft_size = next_power_of_two(frame_size.clamp(256, MAX_FFT_SIZE));
    let mut real = vec![0.0f64; fft_size];
    let mut imaginary = vec![0.0f64; fft_size];
    for (i, slot) in real.iter_mut().enumerate() {
        let sample = if start + i < samples.len() { samples[start + i] } else { 0.0 };
        *slot = sample * (0.5 - 0.5 * ((2.0 * PI * i as f64) / (fft_size - 1) as f64).cos());
    }
    let mut j = 0usize;
    for i in 1..fft_size {
        let mut bit = fft_size >> 1;
        while j & bit != 0 {
            j ^= bit;
            bit >>= 1;
        }
        j ^= bit;
        if i < j {
            real.swap(i, j);
            imaginary.swap(i, j);
        }
    }
    let mut width = 2;
    while width <= fft_size {
        let half = width / 2;
        let phase = (-2.0 * PI) / width as f64;
        let mut offset = 0;
        while offset < fft_size {
            for i in 0..half {
                let angle = phase * i as f64;
                let cosine = angle.cos();
                let sine = angle.sin();
                let even = offset + i;
                let odd = even + half;
                let even_real = real[even];
                let even_imaginary = imaginary[even];
                let odd_real = real[odd] * cosine - imaginary[odd] * sine;
                let odd_imaginary = real[odd] * sine + imaginary[odd] * cosine;
                real[odd] = even_real - odd_real;
                imaginary[odd] = even_imaginary - odd_imaginary;
                real[even] = even_real + odd_real;
                imaginary[even] = even_imaginary + odd_imaginary;
            }
            offset += width;
        }
        width *= 2;
    }
    let magnitudes = (1..=fft_size / 2).map(|bin| real[bin].hypot(imaginary[bin])).collect();
    (magnitudes, fft_size)
}

fn analyze_spectrum(samples: &[f64], sample_rate: f64, frame_size: usize) -> Spectral {
    let frame_count = ((samples.len() as f64 / frame_size as f64).ceil() as usize).max(1);
    let analyzed_frames = frame_count.min(MAX_SPECTRAL_FRAMES);
    let fft_size = next_power_of_two(frame_size.clamp(256, MAX_FFT_SIZE));
    let mut centroid_total = 0.0;
    let mut active_frames = 0usize;
    let mut dominant_frequency = 0.0;
    let mut dominant_magnitude = -1.0;
    for frame in 0..analyzed_frames {
        let start = frame * samples.len().saturating_sub(frame_size) / (analyzed_frames - 1).max(1);
        let (magnitudes, _) = fft_magnitudes(samples, start, frame_size);
        let mut total_magnitude = 0.0;
        let mut weighted_frequency = 0.0;
        for bin in 1..=fft_size / 2 {
            let magnitude = magnitudes[bin - 1];
            let frequency = (bin as f64 * sample_rate) / fft_size as f64;
            total_magnitude += magnitude;
            weighted_frequency += frequency * magnitude;
            if magnitude > dominant_magnitude {
                dominant_magnitude = magnitude;
                dominant_frequency = frequency;
            }
        }
        if total_magnitude > EPSILON {
            centroid_total += weighted_frequency / total_magnitude;
            active_frames += 1;
        }
    }
    Spectral {
        centroid_hz: if active_frames > 0 { centroid_total / active_frames as f64 } else { 0.0 },
        dominant_frequency_hz: if active_frames > 0 { dominant_frequency } else { 0.0 },
        analyzed_frames,
        fft_size,
    }
}

fn waveform(samples: &[f64], channels: usize, sample_rate: f64) -> Waveform {
    let frames = samples.len() / channels;
    // A lossy envelope must never degenerate into a sample-for-sample export.
    // There is no bounded, non-lossy envelope for a one-frame input, so return
    // an empty summary rather than leaking that frame as a single bin.
    let bin_count = if frames <= 1 { 0 } else { MAX_WAVEFORM_BINS.min(frames - 1) };
    let bins = (0..bin_count)
        .map(|bin| {
            let start = bin * frames / bin_count;
            let end = (start + 1).max((bin + 1) * frames / bin_count);
            let details = (0..channels)
                .map(|channel| {
                    let mut min = f64::INFINITY;
                    let mut max = f64::NEG_INFINITY;
                    let mut squares = 0.0;
                    let mut count = 0usize;
                    for frame in start..end.min(frames) {
                        let value = samples[frame * channels + channel];
                        min = min.min(value);
                        max = max.max(value);
                        squares += value * value;
                        count += 1;
                    }
                    WaveformChannel { min, max, rms: (squares / count.max(1) as f64).sqrt() }
                })
                .collect();
            WaveformEnvelope {
                start_seconds: start as f64 / sample_rate,
                end_seconds: end.min(frames) as f64 / sample_rate,
                channels: details,
            }
        })
        .collect();
    Waveform { bins, bin_count, channel_aggregation: "per-channel".to_string(), lossy: true }
}

fn time_frequency(samples: &[f64], channels: usize, sample_rate: f64, frame_size: usize) -> TimeFrequency {
    let frames = samples.len() / channels;
    let frame_count = MAX_TIME_FREQUENCY_FRAMES.min(((frames as f64 / frame_size as f64).ceil() as usize).max(1));
    let fft_size = next_power_of_two(frame_size.clamp(256, MAX_FFT_SIZE));
    let nyquist = sample_rate / 2.0;
    let min_frequency = 20f64.min(nyquist / 2.0);
    let max_frequency = nyquist;
    let bands: Vec<(f64, f64, f64)> = (0..MAX_TIME_FREQUENCY_BANDS)
        .map(|index| {
            let low_hz = min_frequency * (max_frequency / min_frequency).powf(index as f64 / MAX_TIME_FREQUENCY_BANDS as f64);
            let high_hz = min_frequency * (max_frequency / min_frequency).powf((index + 1) as f64 / MAX_TIME_FREQUENCY_BANDS as f64);
            (low_hz, high_hz, (low_hz * high_hz).sqrt())
        })
        .collect();
    // Deinterleave once. The previous implementation rebuilt one full mono
    // buffer for every channel on every spectral frame, turning the declared
    // maximum input into avoidable O(frames * channels * spectralFrames) copying.
    let deinterleaved: Vec<Vec<f64>> = if channels == 1 {
        Vec::new()
    } else {
        (0..channels).map(|channel| (0..frames).map(|frame| samples[frame * channels + channel]).collect()).collect()
    };
    let channel_samples: Vec<&[f64]> = if channels == 1 { vec![samples] } else { deinterleaved.iter().map(Vec::as_slice).collect() };
    let output = (0..frame_count)
        .map(|index| {
            let start = index * frames.saturating_sub(frame_size) / (frame_count - 1).max(1);
            let per_channel: Vec<Vec<f64>> = channel_samples.iter().map(|channel| fft_magnitudes(channel, start, frame_size).0).collect();
            let band_values = bands
                .iter()
                .map(|&(low_hz, high_hz, center_hz)| {
                    let channel_energy: Vec<f64> = per_channel
                        .iter()
                        .map(|magnitudes| {
                            let mut sum = 0.0;
                            let mut count = 0usize;
                            for bin in 1..=magnitudes.len() {
                                let frequency = (bin as f64 * sample_rate) / fft_size as f64;
                                if frequency >= low_hz && frequency < high_hz {
                                    let magnitude = magnitudes[bin - 1];
                                    sum += magnitude * magnitude;
                                    count += 1;
                                }
                            }
                            sum / (count * fft_size * fft_size).max(1) as f64
                        })
                        .collect();
                    let energy = channel_energy.iter().fold(0.0, |sum, value| sum + value) / channels as f64;
                    TimeFrequencyBand {
                        low_hz,
                        high_hz,
                        center_hz,
                        energy,
                        energy_db: db(energy.sqrt()),
                        channels: channel_energy.iter().map(|value| value.sqrt()).collect(),
                    }
                })
                .collect();
            TimeFrequencyFrame {
                start_seconds: start as f64 / sample_rate,
                end_seconds: (start + frame_size).min(frames) as f64 / sample_rate,
                bands: band_values,
            }
        })
        .collect();
    TimeFrequency {
        frames: output,
        frame_count,
        band_count: bands.len(),
        frequency_range_hz: FrequencyRange { min: min_frequency, max: max_frequency },
        method: "hann-windowed-fft".to_string(),
        window: "hann".to_string(),
        hop_samples: frame_size,
        channel_aggregation: "per-channel-and-aggregate".to_string(),
        normalization: "mean-square-per-frame".to_string(),
        lossy: true,
    }
}

fn transients(samples: &[f64], sample_rate: f64, maximum: f64) -> Transients {
    let mut peak_count = 0usize;
    let mut strongest_index: Option<usize> = None;
    let mut strongest_amplitude = 0.0;
    let threshold = 0.25f64.max(0.9f64.min(maximum * 0.5));
    let refractory = ((sample_rate * 0.01).floor() as i64).max(1);
    let mut last_peak = -refractory;
    for index in 0..samples.len() {
        let amplitude = samples[index].abs();
        let previous = if index > 0 { samples[index - 1].abs() } else { 0.0 };
        let next = samples.get(index + 1).map_or(0.0, |value| value.abs());
        if amplitude >= threshold && amplitude >= previous && amplitude > next && index as i64 - last_peak >= refractory {
            peak_count += 1;
            last_peak = index as i64;
            if amplitude > strongest_amplitude {
                strongest_amplitude = amplitude;
                strongest_index = Some(index);
            }
        }
    }
    Transients {
        peak_count,
        density_per_second: peak_count as f64 / (samples.len() as f64 / sample_rate),
        threshold,
        strongest: strongest_index.map(|sample_index| StrongestTransient {
            sample_index,
            time_seconds: sample_index as f64 / sample_rate,
            amplitude: strongest_amplitude,
        }),
    }
}

fn remediation(analysis: &PcmAnalysis) -> Vec<AudioRemediation> {
    let advise = |id: &str, severity: RemediationSeverity, reason: &str, action: &str| AudioRemediation {
        id: id.to_string(),
        severity,
        reason: reason.to_string(),
        action: action.to_string(),
        reversible: true,
        changes_audio: false,
    };
    let mut result = Vec::new();
    if analysis.clipping.count > 0 {
        result.push(advise(
            "reduce-clipping",
            RemediationSeverity::Warning,
            "Source samples reach the normalized full-scale boundary.",
            "Preview a reversible gain reduction or limiter adjustment before applying it.",
        ));
    }
    if analysis.reconstructed_overs.count > 0 {
        result.push(advise(
            "inspect-reconstructed-overs",
            RemediationSeverity::Info,
            "Band-limited reconstruction exceeded 0 dBFS; this does not prove source sample clipping.",
            "Inspect source-domain clipping, true peak, and headroom separately before changing audio.",
        ));
    }
    if analysis.peak_dbfs > -1.0 {
        let reason = if analysis.reconstructed_overs.applicable {
            "Reconstructed peak is within 1 dB of or above full scale; this does not prove source clipping."
        } else {
            "Source sample peak is within 1 dB of full scale."
        };
        result.push(advise(
            "leave-headroom",
            RemediationSeverity::Warning,
            reason,
            "Preview at least 1 dB of headroom; do not change the Live set automatically.",
        ));
    }
    if analysis.loudness.integrated_lufs_estimate > -9.0 {
        result.push(advise(
            "check-loudness",
            RemediationSeverity::Info,
            "The compatibility RMS proxy is high; inspect the standardsAudio loudness result before making a delivery judgement.",
            "Compare the standards-based result against the delivery target and audition at a safe monitoring level.",
        ));
    }
    if analysis.dynamics.silence_ratio > 0.95 {
        result.push(advise(
            "inspect-silence",
            RemediationSeverity::Info,
            "More than 95% of samples are near silence.",
            "Inspect the capture range before editing or deleting anything.",
        ));
    }
    result
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AnalysisSampleKind {
    NormalizedSource,
    BandLimitedReconstruction,
}

fn analyze_pcm_internal(
    input: &PcmAnalysisInput,
    sample_kind: AnalysisSampleKind,
    amplitude_limit: f64,
) -> Result<PcmAnalysis, RangeError> {
    let channels = input.channels.unwrap_or(1.0);
    let frame_size = input.frame_size.unwrap_or(2048.0);
    finite(input.sample_rate, "sampleRate")?;
    finite(channels, "channels")?;
    finite(frame_size, "frameSize")?;
    if !is_integer(input.sample_rate) || input.sample_rate < 8_000.0 || input.sample_rate > 384_000.0 {
        return Err(RangeError("sampleRate must be an integer from 8000 to 384000".to_string()));
    }
    if !is_integer(channels) || channels < 1.0 || channels > MAX_ANALYSIS_CHANNELS as f64 {
        return Err(RangeError(format!("channels must be an integer from 1 to {MAX_ANALYSIS_CHANNELS}")));
    }
    if !is_integer(frame_size) || frame_size < 256.0 || frame_size > 4096.0 {
        return Err(RangeError("frameSize must be an integer from 256 to 4096".to_string()));
    }
    let sample_count = input.samples.len();
    if sample_count == 0 || sample_count > MAX_ANALYSIS_SAMPLES {
        return Err(RangeError(format!("samples must contain 1-{MAX_ANALYSIS_SAMPLES} values")));
    }
    let sample_rate = input.sample_rate;
    let channels = channels as usize;
    let frame_size = frame_size as usize;
    if sample_count % channels != 0 {
        return Err(RangeError("samples must contain complete channel frames".to_string()));
    }
    if sample_count as f64 / channels as f64 / sample_rate > MAX_ANALYSIS_SECONDS {
        return Err(RangeError(format!("analysis duration exceeds {} seconds", number::to_string(MAX_ANALYSIS_SECONDS))));
    }

    let mut sum_squares = 0.0;
    let mut peak = 0.0f64;
    let mut mono_peak = 0.0f64;
    let mut clipping_count = 0usize;
    let mut reconstructed_over_count = 0usize;
    let mut silence_count = 0usize;
    let mut histogram = vec![0u32; 2048];
    // Copy caller-owned storage once. This both bounds the trust boundary and
    // ensures all aggregate, channel, stereo, and spectral fields describe the
    // same immutable snapshot when an ArrayLike has observable getters.
    let mut normalized_samples = vec![0.0f64; sample_count];
    // Mono input is already the immutable analysis snapshot. Reusing it avoids
    // a second ten-million-sample allocation and copy on the declared maximum
    // input while preserving the trust-boundary copy above.
    let mut mono_buffer = if channels == 1 { Vec::new() } else { vec![0.0f64; sample_count / channels] };
    let mut channel_squares = vec![0.0f64; channels];
    let mut channel_sums = vec![0.0f64; channels];
    let mut channel_peaks = vec![0.0f64; channels];
    let mut channel_clips = vec![0u32; channels];
    let mut channel_reconstructed_overs = vec![0u32; channels];
    let normalized_source = sample_kind == AnalysisSampleKind::NormalizedSource;
    for i in 0..sample_count {
        let sample = input.samples[i];
        finite(sample, &format!("samples[{i}]"))?;
        if sample < -amplitude_limit || sample > amplitude_limit {
            return Err(RangeError(if amplitude_limit == 1.0 {
                format!("samples[{i}] must be normalized between -1 and 1")
            } else {
                format!("samples[{i}] exceeds the bounded reconstructed-audio range")
            }));
        }
        normalized_samples[i] = sample;
        let magnitude = sample.abs();
        mono_peak = mono_peak.max(magnitude);
        let channel = i % channels;
        let bucket = (((magnitude / amplitude_limit) * histogram.len() as f64).floor() as usize).min(histogram.len() - 1);
        histogram[bucket] += 1;
        sum_squares += sample * sample;
        channel_squares[channel] += sample * sample;
        channel_sums[channel] += sample;
        channel_peaks[channel] = channel_peaks[channel].max(magnitude);
        if normalized_source && magnitude >= 0.999999 {
            channel_clips[channel] += 1;
        }
        if !normalized_source && magnitude > 1.0 {
            channel_reconstructed_overs[channel] += 1;
        }
        peak = peak.max(magnitude);
        if normalized_source && magnitude >= 0.999999 {
            clipping_count += 1;
        }
        if !normalized_source && magnitude > 1.0 {
            reconstructed_over_count += 1;
        }
        if magnitude < 0.0001 {
            silence_count += 1;
        }
        if channels > 1 {
            let frame = i / channels;
            if i % channels == 0 {
                mono_buffer[frame] = sample;
            } else if magnitude > mono_buffer[frame].abs() {
                mono_buffer[frame] = sample;
            }
        }
    }
    let mono_samples: &[f64] = if channels == 1 { &normalized_samples } else { &mono_buffer };
    let rms = (sum_squares / sample_count as f64).sqrt();
    let frames = sample_count / channels;
    let not_applicable_reason = "input contains normalized source samples, not band-limited reconstruction";
    let channels_detail = (0..channels)
        .map(|channel| {
            let channel_rms = (channel_squares[channel] / frames as f64).sqrt();
            let channel_peak = channel_peaks[channel];
            let clip_count = channel_clips[channel] as usize;
            let reconstructed_over_channel_count = channel_reconstructed_overs[channel] as usize;
            ChannelDetail {
                channel,
                peak: channel_peak,
                peak_dbfs: db(channel_peak),
                rms: channel_rms,
                rms_dbfs: db(channel_rms),
                dc_offset: channel_sums[channel] / frames as f64,
                clipping: BoundaryCount { count: clip_count, ratio: clip_count as f64 / frames as f64 },
                reconstructed_overs: ReconstructedOvers {
                    count: reconstructed_over_channel_count,
                    ratio: reconstructed_over_channel_count as f64 / frames as f64,
                    threshold: 1,
                    applicable: !normalized_source,
                    reason: normalized_source.then(|| not_applicable_reason.to_string()),
                },
            }
        })
        .collect();
    let mut phase_correlation: Option<f64> = None;
    let mut correlation_reason: Option<String> = None;
    if channels == 2 {
        let mut left_squares = 0.0;
        let mut right_squares = 0.0;
        let mut product = 0.0;
        for frame in 0..frames {
            let left = normalized_samples[frame * 2];
            let right = normalized_samples[frame * 2 + 1];
            left_squares += left * left;
            right_squares += right * right;
            product += left * right;
        }
        phase_correlation = Some(if left_squares == 0.0 || right_squares == 0.0 {
            0.0
        } else {
            (product / (left_squares * right_squares).sqrt()).clamp(-1.0, 1.0)
        });
    } else {
        correlation_reason = Some("phase correlation is applicable only to stereo input".to_string());
    }
    let quantile = |fraction: f64| -> f64 {
        let target = (sample_count as f64 * fraction).floor() as u64;
        let mut seen = 0u64;
        for (bucket, count) in histogram.iter().enumerate() {
            seen += *count as u64;
            if seen > target {
                return (bucket as f64 / histogram.len() as f64) * amplitude_limit;
            }
        }
        amplitude_limit
    };
    let p10 = quantile(0.1);
    let p95 = quantile(0.95);
    let dynamic_range_db = db(p95) - db(p10.max(EPSILON));
    let standards_audio = analyze_standards_audio(&StandardsAudioInput {
        samples: &normalized_samples,
        sample_rate,
        channels: channels as f64,
        channel_layout: input.channel_layout,
    })?;
    let mut analysis = PcmAnalysis {
        version: ANALYSIS_VERSION.to_string(),
        sample_rate,
        channels,
        duration_seconds: sample_count as f64 / channels as f64 / sample_rate,
        sample_count,
        peak,
        peak_dbfs: db(peak),
        rms,
        rms_dbfs: db(rms),
        channels_detail,
        stereo: StereoAnalysis { phase_correlation, reason: correlation_reason },
        loudness: LoudnessProxy {
            rms_loudness_proxy_db: db(rms),
            integrated_lufs_estimate: db(rms),
            method: "rms-derived-proxy".to_string(),
            standards_compliant: false,
            deprecated_integrated_lufs_estimate: true,
        },
        standards_audio,
        dynamics: Dynamics {
            crest_factor_db: db(peak / rms.max(EPSILON)),
            dynamic_range_db,
            silence_ratio: silence_count as f64 / sample_count as f64,
        },
        clipping: BoundaryCount { count: clipping_count, ratio: clipping_count as f64 / sample_count as f64 },
        reconstructed_overs: ReconstructedOvers {
            count: reconstructed_over_count,
            ratio: reconstructed_over_count as f64 / sample_count as f64,
            threshold: 1,
            applicable: !normalized_source,
            reason: normalized_source.then(|| not_applicable_reason.to_string()),
        },
        spectral: analyze_spectrum(mono_samples, sample_rate, frame_size),
        waveform: waveform(&normalized_samples, channels, sample_rate),
        time_frequency: time_frequency(&normalized_samples, channels, sample_rate, frame_size),
        transients: transients(mono_samples, sample_rate, mono_peak),
        privacy: Privacy { raw_audio_retained: false, raw_audio_returned: false, source_path_accepted: false },
        safety: Safety { playback_started: false, project_mutated: false, destructive_action_required: false },
        performance: Performance {
            bounded: true,
            max_samples: MAX_ANALYSIS_SAMPLES,
            max_seconds: MAX_ANALYSIS_SECONDS,
            max_spectral_frames: MAX_SPECTRAL_FRAMES,
            max_fft_size: MAX_FFT_SIZE,
        },
        remediation: Vec::new(),
    };
    analysis.remediation = remediation(&analysis);
    Ok(analysis)
}

pub fn analyze_pcm(input: &PcmAnalysisInput) -> Result<PcmAnalysis, RangeError> {
    analyze_pcm_internal(input, AnalysisSampleKind::NormalizedSource, 1.0)
}

/// Package-internal path for a band-limited resampler whose reconstructed
/// inter-sample values can truthfully exceed normalized sample full scale.
pub fn analyze_reconstructed_pcm(input: &PcmAnalysisInput) -> Result<PcmAnalysis, RangeError> {
    analyze_pcm_internal(input, AnalysisSampleKind::BandLimitedReconstruction, 4.0)
}

pub fn decode_float32_le(base64: &str) -> Result<Vec<f32>, RangeError> {
    let max_base64_length = ((MAX_ANALYSIS_SAMPLES * 4) as f64 / 3.0).ceil() as usize * 4;
    let length = if base64.is_ascii() { base64.len() } else { string::utf16_len(base64) };
    if length > max_base64_length {
        return Err(RangeError("pcmBase64 is invalid or exceeds the analysis limit".to_string()));
    }
    let invalid = || RangeError("pcmBase64 is invalid".to_string());
    if length == 0 || length % 4 != 0 {
        return Err(invalid());
    }
    let mut content_length = base64.len();
    if let Some(padding_start) = base64.find('=') {
        content_length = padding_start;
        let padding = &base64[padding_start..];
        if (padding != "=" && padding != "==") || padding_start < 2 {
            return Err(invalid());
        }
    }
    if !base64.as_bytes()[..content_length].iter().all(|code| code.is_ascii_alphanumeric() || *code == b'+' || *code == b'/') {
        return Err(invalid());
    }
    let engine = base64::engine::general_purpose::STANDARD;
    let bytes = engine.decode(base64).map_err(|_| invalid())?;
    if engine.encode(&bytes) != base64 {
        // non-canonical encoding
        return Err(invalid());
    }
    if bytes.is_empty() || bytes.len() % 4 != 0 || bytes.len() / 4 > MAX_ANALYSIS_SAMPLES {
        return Err(RangeError("pcmBase64 must contain bounded float32 PCM".to_string()));
    }
    let mut result = Vec::with_capacity(bytes.len() / 4);
    for chunk in bytes.chunks_exact(4) {
        let sample = f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
        if !sample.is_finite() || !(-1.0..=1.0).contains(&sample) {
            return Err(RangeError("pcmBase64 must contain finite normalized float32 PCM".to_string()));
        }
        result.push(sample);
    }
    Ok(result)
}
