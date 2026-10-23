//! Standards-grounded programme loudness and true-peak analysis.
//!
//! Loudness follows ITU-R BS.1770-5 Annex 1 and EBU R128/Tech 3341/3342.
//! True peak uses the 48 kHz, order-48, four-phase interpolator published in
//! BS.1770-5 Annex 2. Raw PCM is neither retained nor returned by this module.

use std::f64::consts::PI;

use kumi_common::js::number;
use serde::{Deserialize, Serialize};

pub const STANDARDS_AUDIO_VERSION: &str = "bs1770-5-ebu-r128-2023/v1";
pub const MAX_TRUE_PEAK_FRAMES: usize = 1_440_000; // 30 s at 48 kHz for mono.
pub const MAX_TRUE_PEAK_SAMPLES: usize = 2_880_000; // Shared bound across channels.
pub const MAX_LOUDNESS_SERIES_POINTS: usize = 128;

/// A `RangeError`: what the analysis modules throw for input outside their bounds. The message is
/// the user-facing text.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct RangeError(pub String);

/// `Number.isInteger(value)`.
pub(crate) fn is_integer(value: f64) -> bool {
    value.is_finite() && value.fract() == 0.0
}

#[allow(clippy::upper_case_acronyms)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ConventionalChannelLabel {
    M,
    L,
    R,
    C,
    Ls,
    Rs,
    LFE,
    /// A JSON value whose string coercion names a label, but which is not itself a semantic label.
    #[doc(hidden)]
    #[serde(skip)]
    Invalid,
}

impl ConventionalChannelLabel {
    pub const ALL: [ConventionalChannelLabel; 7] = [Self::M, Self::L, Self::R, Self::C, Self::Ls, Self::Rs, Self::LFE];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::M => "M",
            Self::L => "L",
            Self::R => "R",
            Self::C => "C",
            Self::Ls => "Ls",
            Self::Rs => "Rs",
            Self::LFE => "LFE",
            Self::Invalid => "",
        }
    }

    /// The label a string names, if it is one of the seven.
    pub fn parse(label: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|candidate| candidate.as_str() == label)
    }

    fn weight(self) -> f64 {
        match self {
            Self::M | Self::L | Self::R | Self::C => 1.0,
            Self::Ls | Self::Rs => 1.41,
            Self::LFE | Self::Invalid => 0.0,
        }
    }
}

/// The input as the TypeScript took it: `sampleRate` and `channels` are numbers that the analysis
/// itself checks to be integers in range.
#[derive(Debug, Clone, Copy)]
pub struct StandardsAudioInput<'a> {
    pub samples: &'a [f64],
    pub sample_rate: f64,
    pub channels: f64,
    pub channel_layout: Option<&'a [ConventionalChannelLabel]>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LoudnessPoint {
    pub time_seconds: f64,
    pub lufs: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Standards {
    pub programme_loudness: String,
    pub operating_recommendation: String,
    pub meter: String,
    pub loudness_range: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChannelLayout {
    pub labels: Vec<ConventionalChannelLabel>,
    pub weights: Vec<f64>,
    pub explicit: bool,
    pub lfe_excluded: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LoudnessBlocks {
    pub total: usize,
    pub above_absolute_gate: usize,
    pub above_relative_gate: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LoudnessWindow {
    pub window_seconds: f64,
    pub cadence_seconds: f64,
    pub current_lufs: Option<f64>,
    pub maximum_lufs: Option<f64>,
    pub series: Vec<LoudnessPoint>,
    pub series_lossy: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LoudnessRange {
    pub available: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    pub lra_lu: Option<f64>,
    pub low_lufs: Option<f64>,
    pub high_lufs: Option<f64>,
    pub absolute_gate_lufs: f64,
    pub relative_gate_offset_lu: f64,
    pub percentile_method: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Loudness {
    pub available: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    pub standards_compliant: bool,
    pub integrated_lufs: Option<f64>,
    pub absolute_gate_lufs: f64,
    pub relative_gate_lufs: Option<f64>,
    pub blocks: LoudnessBlocks,
    pub momentary: LoudnessWindow,
    pub short_term: LoudnessWindow,
    pub loudness_range: LoudnessRange,
}

/// Keys in the order the TypeScript wrote them: the `base` fields first, then availability.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TruePeak {
    pub sample_rate: f64,
    pub method: String,
    pub max_frames: usize,
    pub max_samples: usize,
    pub available: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    pub standards_compliant: bool,
    pub aggregate_dbtp: Option<f64>,
    pub per_channel_dbtp: Vec<Option<f64>>,
    pub oversampling_factor: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StandardsAudioAnalysis {
    pub version: String,
    pub standards: Standards,
    pub channel_layout: ChannelLayout,
    pub loudness: Loudness,
    pub true_peak: TruePeak,
}

const LOUDNESS_OFFSET: f64 = -0.691;
const ABSOLUTE_GATE_LUFS: f64 = -70.0;
const EPSILON_ENERGY: f64 = 1e-30;

#[derive(Debug, Clone, Copy)]
struct BiquadCoefficients {
    b0: f64,
    b1: f64,
    b2: f64,
    a1: f64,
    a2: f64,
}

#[derive(Debug, Clone, Copy, Default)]
struct BiquadState {
    x1: f64,
    x2: f64,
    y1: f64,
    y2: f64,
}

fn biquad(value: f64, coefficients: &BiquadCoefficients, state: &mut BiquadState) -> f64 {
    let output = coefficients.b0 * value + coefficients.b1 * state.x1 + coefficients.b2 * state.x2
        - coefficients.a1 * state.y1
        - coefficients.a2 * state.y2;
    state.x2 = state.x1;
    state.x1 = value;
    state.y2 = state.y1;
    state.y1 = output;
    output
}

/// BS.1770 gives exact 48 kHz coefficients and requires equivalent responses at
/// other rates. These are the pre-warped De Man parameterizations of those two
/// published biquads; the 48 kHz branch preserves the normative table exactly.
fn k_weighting_coefficients(sample_rate: f64) -> (BiquadCoefficients, BiquadCoefficients) {
    if sample_rate == 48_000.0 {
        return (
            BiquadCoefficients {
                b0: 1.53512485958697,
                b1: -2.69169618940638,
                b2: 1.19839281085285,
                a1: -1.69065929318241,
                a2: 0.73248077421585,
            },
            BiquadCoefficients { b0: 1.0, b1: -2.0, b2: 1.0, a1: -1.99004745483398, a2: 0.99007225036621 },
        );
    }

    let shelf_frequency = 1_681.974450955533;
    let shelf_gain_db = 3.999843853973347;
    let shelf_q = 0.7071752369554196;
    let shelf_k = (PI * shelf_frequency / sample_rate).tan();
    let vh = 10f64.powf(shelf_gain_db / 20.0);
    let vb = vh.powf(0.4996667741545416);
    let shelf_a0 = 1.0 + shelf_k / shelf_q + shelf_k * shelf_k;
    let shelf = BiquadCoefficients {
        b0: (vh + vb * shelf_k / shelf_q + shelf_k * shelf_k) / shelf_a0,
        b1: 2.0 * (shelf_k * shelf_k - vh) / shelf_a0,
        b2: (vh - vb * shelf_k / shelf_q + shelf_k * shelf_k) / shelf_a0,
        a1: 2.0 * (shelf_k * shelf_k - 1.0) / shelf_a0,
        a2: (1.0 - shelf_k / shelf_q + shelf_k * shelf_k) / shelf_a0,
    };

    let high_pass_frequency = 38.13547087602444;
    let high_pass_q = 0.5003270373238773;
    let high_pass_k = (PI * high_pass_frequency / sample_rate).tan();
    let high_pass_a0 = 1.0 + high_pass_k / high_pass_q + high_pass_k * high_pass_k;
    let high_pass = BiquadCoefficients {
        // The BS.1770 RLB numerator is intentionally not divided by a0.
        b0: 1.0,
        b1: -2.0,
        b2: 1.0,
        a1: 2.0 * (high_pass_k * high_pass_k - 1.0) / high_pass_a0,
        a2: (1.0 - high_pass_k / high_pass_q + high_pass_k * high_pass_k) / high_pass_a0,
    };
    (shelf, high_pass)
}

fn default_layout(channels: usize) -> Option<Vec<ConventionalChannelLabel>> {
    match channels {
        1 => Some(vec![ConventionalChannelLabel::M]),
        2 => Some(vec![ConventionalChannelLabel::L, ConventionalChannelLabel::R]),
        _ => None,
    }
}

struct ResolvedLayout {
    labels: Vec<ConventionalChannelLabel>,
    weights: Vec<f64>,
    explicit: bool,
}

fn resolve_layout(input: &StandardsAudioInput, channels: usize) -> Result<ResolvedLayout, String> {
    let labels = match input.channel_layout {
        Some(layout) => layout.to_vec(),
        None => default_layout(channels)
            .ok_or_else(|| "channelLayout is required for standards loudness when channels is greater than two".to_string())?,
    };
    if labels.len() != channels {
        return Err("channelLayout must contain exactly one semantic label per channel".to_string());
    }
    if labels.contains(&ConventionalChannelLabel::Invalid) {
        return Err("channelLayout contains a label outside the supported conventional BS.1770 layout".to_string());
    }
    let mut unique = labels.clone();
    unique.sort_by_key(|label| label.as_str());
    unique.dedup();
    if unique.len() != labels.len() {
        return Err("channelLayout labels must be unique".to_string());
    }
    if labels.contains(&ConventionalChannelLabel::M) && labels.len() != 1 {
        return Err("the mono channel label M is valid only for one-channel input".to_string());
    }
    if labels.iter().all(|label| *label == ConventionalChannelLabel::LFE) {
        return Err("channelLayout must contain at least one programme channel; LFE is excluded from loudness".to_string());
    }
    let weights = labels.iter().map(|label| label.weight()).collect();
    Ok(ResolvedLayout { labels, weights, explicit: input.channel_layout.is_some() })
}

fn loudness_from_energy(energy: f64) -> Option<f64> {
    if !(energy > EPSILON_ENERGY) {
        return None;
    }
    Some(LOUDNESS_OFFSET + 10.0 * energy.log10())
}

fn mean(values: &[f64]) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    let mut sum = 0.0;
    let mut compensation = 0.0;
    for &value in values {
        let adjusted = value - compensation;
        let next = sum + adjusted;
        compensation = (next - sum) - adjusted;
        sum = next;
    }
    sum / values.len() as f64
}

fn percentile_r7(sorted: &[f64], fraction: f64) -> Option<f64> {
    if sorted.is_empty() {
        return None;
    }
    if sorted.len() == 1 {
        return Some(sorted[0]);
    }
    let index = (sorted.len() - 1) as f64 * fraction;
    let lower = index.floor() as usize;
    let upper = index.ceil() as usize;
    let lower_value = sorted[lower];
    let upper_value = sorted.get(upper).copied().unwrap_or(lower_value);
    Some(lower_value + (upper_value - lower_value) * (index - lower as f64))
}

fn bounded_series(points: &[LoudnessPoint]) -> Vec<LoudnessPoint> {
    if points.len() <= MAX_LOUDNESS_SERIES_POINTS {
        return points.to_vec();
    }
    (0..MAX_LOUDNESS_SERIES_POINTS)
        .map(|index| {
            let source = number::round((index * (points.len() - 1)) as f64 / (MAX_LOUDNESS_SERIES_POINTS - 1) as f64) as usize;
            points[source].clone()
        })
        .collect()
}

struct LoudnessMeasurements {
    block_energies: Vec<f64>,
    momentary_points: Vec<LoudnessPoint>,
    momentary_current: Option<f64>,
    short_term_points: Vec<LoudnessPoint>,
    short_term_current: Option<f64>,
}

fn measure_loudness(samples: &[f64], sample_rate: f64, channels: usize, weights: &[f64]) -> LoudnessMeasurements {
    let (shelf, high_pass) = k_weighting_coefficients(sample_rate);
    let mut shelf_states = vec![BiquadState::default(); channels];
    let mut high_pass_states = vec![BiquadState::default(); channels];
    let frame_count = samples.len() / channels;
    let momentary_frames = number::round(sample_rate * 0.4) as usize;
    let momentary_hop = number::round(sample_rate * 0.1) as usize;
    let short_term_frames = number::round(sample_rate * 3.0) as usize;
    let short_term_hop = number::round(sample_rate * 0.1) as usize;
    let mut ring = vec![0.0f64; short_term_frames];
    let mut momentary_sum = 0.0;
    let mut short_term_sum = 0.0;
    let mut block_energies = Vec::new();
    let mut momentary_points = Vec::new();
    let mut short_term_points = Vec::new();

    for frame in 0..frame_count {
        let mut frame_energy = 0.0;
        for channel in 0..channels {
            let sample = samples[frame * channels + channel];
            let shelf_output = biquad(sample, &shelf, &mut shelf_states[channel]);
            let weighted = biquad(shelf_output, &high_pass, &mut high_pass_states[channel]);
            frame_energy += weights[channel] * weighted * weighted;
        }

        let momentary_old = if frame >= momentary_frames { ring[(frame - momentary_frames) % short_term_frames] } else { 0.0 };
        let short_term_old = if frame >= short_term_frames { ring[frame % short_term_frames] } else { 0.0 };
        momentary_sum += frame_energy - momentary_old;
        short_term_sum += frame_energy - short_term_old;
        ring[frame % short_term_frames] = frame_energy;
        let end_frame = frame + 1;

        if end_frame >= momentary_frames && (end_frame - momentary_frames) % momentary_hop == 0 {
            let energy = (momentary_sum / momentary_frames as f64).max(0.0);
            block_energies.push(energy);
            if let Some(lufs) = loudness_from_energy(energy) {
                momentary_points.push(LoudnessPoint { time_seconds: end_frame as f64 / sample_rate, lufs });
            }
        }
        if end_frame >= short_term_frames && (end_frame - short_term_frames) % short_term_hop == 0 {
            if let Some(lufs) = loudness_from_energy((short_term_sum / short_term_frames as f64).max(0.0)) {
                short_term_points.push(LoudnessPoint { time_seconds: end_frame as f64 / sample_rate, lufs });
            }
        }
    }

    LoudnessMeasurements {
        block_energies,
        momentary_points,
        momentary_current: if frame_count >= momentary_frames {
            loudness_from_energy((momentary_sum / momentary_frames as f64).max(0.0))
        } else {
            None
        },
        short_term_points,
        short_term_current: if frame_count >= short_term_frames {
            loudness_from_energy((short_term_sum / short_term_frames as f64).max(0.0))
        } else {
            None
        },
    }
}

fn unavailable_loudness(reason: &str) -> Loudness {
    Loudness {
        available: false,
        reason: Some(reason.to_string()),
        standards_compliant: false,
        integrated_lufs: None,
        absolute_gate_lufs: -70.0,
        relative_gate_lufs: None,
        blocks: LoudnessBlocks { total: 0, above_absolute_gate: 0, above_relative_gate: 0 },
        momentary: LoudnessWindow {
            window_seconds: 0.4,
            cadence_seconds: 0.1,
            current_lufs: None,
            maximum_lufs: None,
            series: Vec::new(),
            series_lossy: true,
        },
        short_term: LoudnessWindow {
            window_seconds: 3.0,
            cadence_seconds: 0.1,
            current_lufs: None,
            maximum_lufs: None,
            series: Vec::new(),
            series_lossy: true,
        },
        loudness_range: LoudnessRange {
            available: false,
            reason: Some(reason.to_string()),
            lra_lu: None,
            low_lufs: None,
            high_lufs: None,
            absolute_gate_lufs: -70.0,
            relative_gate_offset_lu: -20.0,
            percentile_method: "linear-r7".to_string(),
        },
    }
}

// Rows are the twelve taps; columns are the four phases from BS.1770-5 Annex 2.
const TRUE_PEAK_PHASES: [[f64; 4]; 12] = [
    [0.0017089843750, -0.0291748046875, -0.0189208984375, -0.0083007812500],
    [0.0109863281250, 0.0292968750000, 0.0330810546875, 0.0148925781250],
    [-0.0196533203125, -0.0517578125000, -0.0582275390625, -0.0266113281250],
    [0.0332031250000, 0.0891113281250, 0.1015625000000, 0.0476074218750],
    [-0.0594482421875, -0.1665039062500, -0.2003173828125, -0.1022949218750],
    [0.1373291015625, 0.4650878906250, 0.7797851562500, 0.9721679687500],
    [0.9721679687500, 0.7797851562500, 0.4650878906250, 0.1373291015625],
    [-0.1022949218750, -0.2003173828125, -0.1665039062500, -0.0594482421875],
    [0.0476074218750, 0.1015625000000, 0.0891113281250, 0.0332031250000],
    [-0.0266113281250, -0.0582275390625, -0.0517578125000, -0.0196533203125],
    [0.0148925781250, 0.0330810546875, 0.0292968750000, 0.0109863281250],
    [-0.0083007812500, -0.0189208984375, -0.0291748046875, 0.0017089843750],
];

fn true_peak_channel_48k(samples: &[f64]) -> Option<f64> {
    let mut maximum = 0.0f64;
    for &sample in samples {
        maximum = maximum.max(sample.abs());
    }
    // Measure only positions with the complete published FIR support. Treating
    // programme boundaries as zero-valued discontinuities creates a false edge
    // overshoot (for example +0.95 dBTP for a constant full-scale programme),
    // unlike BS.1770 meters and the independent FFmpeg oracle.
    let length = samples.len() as i64;
    let mut output_frame = 5i64;
    while output_frame < length - 6 {
        for phase in 0..4 {
            let mut value = 0.0;
            for (tap, coefficients) in TRUE_PEAK_PHASES.iter().enumerate() {
                let input_frame = output_frame - tap as i64 + 6;
                value += coefficients[phase] * samples[input_frame as usize];
            }
            maximum = maximum.max(value.abs());
        }
        output_frame += 1;
    }
    if maximum > 0.0 {
        Some(20.0 * maximum.log10())
    } else {
        None
    }
}

fn true_peak_48k(samples: &[f64], channels: usize) -> Vec<Option<f64>> {
    let frame_count = samples.len() / channels;
    (0..channels)
        .map(|channel| {
            let channel_samples: Vec<f64> = (0..frame_count).map(|frame| samples[frame * channels + channel]).collect();
            true_peak_channel_48k(&channel_samples)
        })
        .collect()
}

fn sinc(value: f64) -> f64 {
    if value.abs() < 1e-12 {
        return 1.0;
    }
    let angle = PI * value;
    angle.sin() / angle
}

fn true_peak_44100(samples: &[f64], channels: usize) -> Vec<Option<f64>> {
    let input_frames = samples.len() / channels;
    let output_frames = number::round(input_frames as f64 * 48_000.0 / 44_100.0) as usize;
    let radius: i64 = 32;
    let mut resampled = vec![vec![0.0f64; output_frames]; channels];
    // An output frame's kernel is the same for every channel: worked out once per frame, it gives the same sums.
    let mut kernel: Vec<(f64, usize)> = Vec::with_capacity(2 * radius as usize);
    for output_frame in 0..output_frames {
        let position = output_frame as f64 * 44_100.0 / 48_000.0;
        let center = position.floor() as i64;
        kernel.clear();
        let mut normalization = 0.0;
        for tap in (center - radius + 1)..=(center + radius) {
            let distance = position - tap as f64;
            let window_position = (distance + radius as f64) / (2.0 * radius as f64);
            let window = 0.42 - 0.5 * (2.0 * PI * window_position).cos() + 0.08 * (4.0 * PI * window_position).cos();
            let coefficient = sinc(distance) * window;
            // Constant edge extension keeps the bounded programme boundary from
            // becoming an artificial impulse while retaining a complete kernel.
            kernel.push((coefficient, tap.clamp(0, input_frames as i64 - 1) as usize));
            normalization += coefficient;
        }
        for (channel, output) in resampled.iter_mut().enumerate() {
            let mut value = 0.0;
            for (coefficient, source_frame) in &kernel {
                value += coefficient * samples[source_frame * channels + channel];
            }
            output[output_frame] = if normalization.abs() > 1e-12 { value / normalization } else { 0.0 };
        }
    }
    resampled.iter().map(|channel| true_peak_channel_48k(channel)).collect()
}

fn analyze_true_peak(samples: &[f64], sample_rate: f64, channels: usize) -> TruePeak {
    let method = if sample_rate == 44_100.0 {
        "64-tap Blackman-sinc 44.1-to-48 kHz then Annex 2 FIR"
    } else {
        "ITU-R BS.1770-5 Annex 2 order-48 four-phase FIR"
    };
    let max_frames = MAX_TRUE_PEAK_FRAMES.min(MAX_TRUE_PEAK_SAMPLES / channels);
    let unavailable = |reason: String| TruePeak {
        sample_rate,
        method: method.to_string(),
        max_frames,
        max_samples: MAX_TRUE_PEAK_SAMPLES,
        available: false,
        reason: Some(reason),
        standards_compliant: false,
        aggregate_dbtp: None,
        per_channel_dbtp: vec![None; channels],
        oversampling_factor: None,
    };
    let frame_count = samples.len() / channels;
    if sample_rate != 48_000.0 && sample_rate != 44_100.0 {
        return unavailable("standards true peak is currently validated only for 44.1 and 48 kHz input".to_string());
    }
    if frame_count > max_frames || samples.len() > MAX_TRUE_PEAK_SAMPLES {
        return unavailable(format!(
            "true-peak input exceeds the bounded {max_frames}-frame/{MAX_TRUE_PEAK_SAMPLES}-sample analysis limit"
        ));
    }
    let per_channel_dbtp = if sample_rate == 48_000.0 { true_peak_48k(samples, channels) } else { true_peak_44100(samples, channels) };
    let finite: Vec<f64> = per_channel_dbtp.iter().flatten().copied().collect();
    TruePeak {
        sample_rate,
        method: method.to_string(),
        max_frames,
        max_samples: MAX_TRUE_PEAK_SAMPLES,
        available: true,
        reason: None,
        standards_compliant: true,
        aggregate_dbtp: finite.iter().copied().reduce(f64::max),
        per_channel_dbtp,
        oversampling_factor: Some(192_000.0 / sample_rate),
    }
}

pub fn analyze_standards_audio(input: &StandardsAudioInput) -> Result<StandardsAudioAnalysis, RangeError> {
    if !is_integer(input.sample_rate) || input.sample_rate < 8_000.0 || input.sample_rate > 384_000.0 {
        return Err(RangeError("sampleRate must be an integer from 8000 to 384000".to_string()));
    }
    let sample_length = input.samples.len();
    if !is_integer(input.channels)
        || input.channels < 1.0
        || input.channels > 32.0
        || sample_length == 0
        || sample_length > 10_000_000
        || sample_length % (input.channels as usize) != 0
    {
        return Err(RangeError("channels and samples must contain 1-10000000 complete frames across at most 32 channels".to_string()));
    }
    let channels = input.channels as usize;
    let sample_rate = input.sample_rate;
    // The samples are read once; validation, loudness, and true peak describe the same finite programme.
    for (index, &sample) in input.samples.iter().enumerate() {
        // The package-internal reference resampler can reconstruct bounded
        // inter-sample values above normalized sample full scale.
        if !sample.is_finite() || !(-4.0..=4.0).contains(&sample) {
            return Err(RangeError(format!("samples[{index}] must be finite and within the bounded analysis range")));
        }
    }
    let samples = input.samples;
    let (labels, weights, explicit, loudness) = match resolve_layout(input, channels) {
        Err(reason) => (Vec::new(), Vec::new(), input.channel_layout.is_some(), unavailable_loudness(&reason)),
        Ok(ResolvedLayout { labels, weights, explicit }) => {
            let measured = measure_loudness(samples, sample_rate, channels, &weights);
            let absolute_energy = 10f64.powf((ABSOLUTE_GATE_LUFS - LOUDNESS_OFFSET) / 10.0);
            let above_absolute: Vec<f64> = measured.block_energies.iter().copied().filter(|energy| *energy > absolute_energy).collect();
            let absolute_gated_loudness = loudness_from_energy(mean(&above_absolute));
            let relative_gate_lufs = absolute_gated_loudness.map(|loudness| loudness - 10.0);
            let relative_energy = match relative_gate_lufs {
                None => f64::INFINITY,
                Some(gate) => 10f64.powf((gate - LOUDNESS_OFFSET) / 10.0),
            };
            let above_relative: Vec<f64> = above_absolute.iter().copied().filter(|energy| *energy > relative_energy).collect();
            let integrated_lufs = loudness_from_energy(mean(&above_relative));

            let lra_absolute: Vec<&LoudnessPoint> =
                measured.short_term_points.iter().filter(|point| point.lufs > ABSOLUTE_GATE_LUFS).collect();
            let lra_absolute_energies: Vec<f64> =
                lra_absolute.iter().map(|point| 10f64.powf((point.lufs - LOUDNESS_OFFSET) / 10.0)).collect();
            let lra_center = loudness_from_energy(mean(&lra_absolute_energies));
            let lra_gate = lra_center.map(|center| center - 20.0);
            let mut lra_values: Vec<f64> =
                lra_absolute.iter().filter(|point| lra_gate.is_some_and(|gate| point.lufs > gate)).map(|point| point.lufs).collect();
            lra_values.sort_by(|left, right| left.partial_cmp(right).unwrap_or(std::cmp::Ordering::Equal));
            let lra_low = percentile_r7(&lra_values, 0.1);
            let lra_high = percentile_r7(&lra_values, 0.95);
            let lra_available = lra_low.is_some() && lra_high.is_some() && lra_values.len() >= 2;
            let momentary_maximum = measured.momentary_points.iter().map(|point| point.lufs).reduce(f64::max);
            let short_term_maximum = measured.short_term_points.iter().map(|point| point.lufs).reduce(f64::max);
            let has_complete_block = !measured.block_energies.is_empty();
            let enough_for_integrated = has_complete_block && integrated_lufs.is_some();

            let loudness = Loudness {
                available: enough_for_integrated,
                reason: if enough_for_integrated {
                    None
                } else if has_complete_block {
                    Some("programme is below the standards absolute loudness gate".to_string())
                } else {
                    Some("standards integrated loudness requires at least one complete 400 ms block".to_string())
                },
                standards_compliant: true,
                integrated_lufs,
                absolute_gate_lufs: -70.0,
                relative_gate_lufs,
                blocks: LoudnessBlocks {
                    total: measured.block_energies.len(),
                    above_absolute_gate: above_absolute.len(),
                    above_relative_gate: above_relative.len(),
                },
                momentary: LoudnessWindow {
                    window_seconds: 0.4,
                    cadence_seconds: 0.1,
                    current_lufs: measured.momentary_current,
                    maximum_lufs: momentary_maximum,
                    series: bounded_series(&measured.momentary_points),
                    series_lossy: true,
                },
                short_term: LoudnessWindow {
                    window_seconds: 3.0,
                    cadence_seconds: 0.1,
                    current_lufs: measured.short_term_current,
                    maximum_lufs: short_term_maximum,
                    series: bounded_series(&measured.short_term_points),
                    series_lossy: true,
                },
                loudness_range: LoudnessRange {
                    available: lra_available,
                    reason: if lra_available {
                        None
                    } else {
                        Some("loudness range requires at least two qualifying 3 s short-term measurements".to_string())
                    },
                    lra_lu: if lra_available { Some(lra_high.unwrap() - lra_low.unwrap()) } else { None },
                    low_lufs: if lra_available { lra_low } else { None },
                    high_lufs: if lra_available { lra_high } else { None },
                    absolute_gate_lufs: -70.0,
                    relative_gate_offset_lu: -20.0,
                    percentile_method: "linear-r7".to_string(),
                },
            };
            (labels, weights, explicit, loudness)
        }
    };

    Ok(StandardsAudioAnalysis {
        version: STANDARDS_AUDIO_VERSION.to_string(),
        standards: Standards {
            programme_loudness: "ITU-R BS.1770-5".to_string(),
            operating_recommendation: "EBU R128".to_string(),
            meter: "EBU Tech 3341".to_string(),
            loudness_range: "EBU Tech 3342".to_string(),
        },
        channel_layout: ChannelLayout { lfe_excluded: labels.contains(&ConventionalChannelLabel::LFE), labels, weights, explicit },
        loudness,
        true_peak: analyze_true_peak(samples, sample_rate, channels),
    })
}
