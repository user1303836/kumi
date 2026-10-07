//! Compact measurements for sound classification and similarity search.
use super::classify::{Heard, HeardKey, Pitch};
use crate::audio::{
    analyze::{estimate_key, estimate_tempo, track_pitch, Key, Tempo},
    decode::{open_audio_to, AudioError},
    dsp::{db_amplitude, fft, k_weighting, percentile, window, Window},
};
use kumi_common::{
    abort::{Signal, SignalExt},
    js::number::round,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    f64::consts::PI,
    sync::{Arc, LazyLock, Mutex},
};
pub const FEATURES_VERSION: u32 = 1;
#[derive(Debug, Clone, Copy)]
pub struct VectorGroup {
    pub group: &'static str,
    pub count: usize,
    pub weight: f64,
}
pub const VECTOR_GROUPS: [VectorGroup; 9] = [
    VectorGroup { group: "tone", count: 12, weight: 2. },
    VectorGroup { group: "movement", count: 13, weight: 0.4 },
    VectorGroup { group: "brightness", count: 3, weight: 1. },
    VectorGroup { group: "noise", count: 3, weight: 1. },
    VectorGroup { group: "weight", count: 3, weight: 1. },
    VectorGroup { group: "envelope", count: 3, weight: 1.2 },
    VectorGroup { group: "length", count: 1, weight: 0.8 },
    VectorGroup { group: "rhythm", count: 1, weight: 0.8 },
    VectorGroup { group: "width", count: 1, weight: 0.3 },
];
pub const VECTOR_LENGTH: usize = 40;
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SoundFeatures {
    pub seconds: f64,
    pub audible_seconds: f64,
    pub loudness_db: f64,
    pub peak_db: f64,
    pub crest_db: f64,
    pub centroid_hz: f64,
    pub rolloff_hz: f64,
    pub flatness: f64,
    pub attack_ms: f64,
    pub decay_ms: f64,
    pub onsets_per_second: f64,
    pub width: f64,
    pub low_share: f64,
    pub mid_share: f64,
    pub high_share: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pitch: Option<Pitch>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<Key>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rhythm: Option<Tempo>,
    pub vector: Vec<f64>,
}
impl SoundFeatures {
    pub fn heard(&self) -> Heard {
        Heard {
            seconds: self.seconds,
            centroid_hz: self.centroid_hz,
            flatness: self.flatness,
            attack_ms: self.attack_ms,
            decay_ms: self.decay_ms,
            onsets_per_second: self.onsets_per_second,
            low_share: self.low_share,
            high_share: self.high_share,
            pitch: self.pitch,
            rhythm_bpm: self.rhythm.as_ref().map(|r| r.bpm),
            rhythm_confidence: self.rhythm.as_ref().map(|r| r.confidence),
            key: self.key.as_ref().map(|k| HeardKey { name: k.name.clone(), confidence: k.confidence }),
        }
    }
}
const MAX_SECONDS: f64 = 30.;
const MEL_BANDS: usize = 40;
const COEFFICIENTS: usize = 13;
struct MelBank {
    from: Vec<usize>,
    to: Vec<usize>,
    weights: Vec<Vec<f64>>,
}
static FILTERBANKS: LazyLock<Mutex<HashMap<(usize, u64), Arc<MelBank>>>> = LazyLock::new(|| Mutex::new(HashMap::new()));
fn mel_filters(size: usize, sample_rate: f64) -> Arc<MelBank> {
    let mut banks = FILTERBANKS.lock().unwrap();
    banks
        .entry((size, sample_rate.to_bits()))
        .or_insert_with(|| {
            let mel = |hz: f64| 2595. * (1. + hz / 700.).log10();
            let hz = |m: f64| 700. * (10_f64.powf(m / 2595.) - 1.);
            let low = mel(30.);
            let high = mel(16000_f64.min(sample_rate / 2. - 1.));
            let edges: Vec<_> = (0..MEL_BANDS + 2)
                .map(|i| hz(low + (high - low) * i as f64 / (MEL_BANDS + 1) as f64) * size as f64 / sample_rate)
                .collect();
            let mut from = vec![];
            let mut to = vec![];
            let mut weights = vec![];
            for band in 0..MEL_BANDS {
                let left = edges[band];
                let center = edges[band + 1];
                let right = edges[band + 2];
                let first = 1_f64.max(left.floor()) as usize;
                let last = (size as f64 / 2.).min(right.ceil()) as usize;
                from.push(first);
                to.push(last);
                weights.push(
                    (first..=last)
                        .map(|bin| {
                            if (bin as f64) < center {
                                0_f64.max((bin as f64 - left) / 1e-9_f64.max(center - left))
                            } else {
                                0_f64.max((right - bin as f64) / 1e-9_f64.max(right - center))
                            }
                        })
                        .collect(),
                );
            }
            Arc::new(MelBank { from, to, weights })
        })
        .clone()
}
static COSINES: LazyLock<Vec<Vec<f64>>> = LazyLock::new(|| {
    (0..COEFFICIENTS).map(|k| (0..MEL_BANDS).map(|n| (PI * k as f64 * (n as f64 + 0.5) / MEL_BANDS as f64).cos()).collect()).collect()
});
struct Frame {
    energy: f64,
    centroid: f64,
    rolloff: f64,
    flatness: f64,
    flux: f64,
    mfcc: [f64; COEFFICIENTS],
}
fn mean(values: &[f64]) -> f64 {
    values.iter().sum::<f64>() / values.len().max(1) as f64
}
fn spread(values: &[f64]) -> f64 {
    let average = mean(values);
    mean(&values.iter().map(|v| (v - average).powi(2)).collect::<Vec<_>>()).sqrt()
}
fn rounded(value: f64, places: i32) -> f64 {
    let multiple = 10_f64.powi(places);
    round(value * multiple) / multiple
}
/// One Float32 sample slice per channel. `seconds` is the full file length, including unread audio.
pub fn measure_samples(channels: &[Vec<f32>], sample_rate: f64, seconds: f64) -> SoundFeatures {
    let left = &channels[0];
    let right = channels.get(1).unwrap_or(left);
    let length = left.len();
    let mut mono = vec![0_f32; length];
    let (mut peak, mut squares, mut sides, mut crossings) = (0_f64, 0_f64, 0_f64, 0_f64);
    for index in 0..length {
        let l = left[index] as f64;
        let r = right[index] as f64;
        let mid = (l + r) / 2.;
        let side = (l - r) / 2.;
        mono[index] = mid as f32;
        squares += mid * mid;
        sides += side * side;
        let magnitude = l.abs().max(r.abs());
        if magnitude > peak {
            peak = magnitude;
        }
        if index > 0 && (mid >= 0.) != (mono[index - 1] >= 0.) {
            crossings += 1.;
        }
    }
    let step = round(sample_rate * 0.005).max(1.) as usize;
    let mut levels = vec![];
    for from in (0..length).step_by(step) {
        let to = length.min(from + step);
        let sum = mono[from..to].iter().map(|v| (*v as f64) * (*v as f64)).sum::<f64>();
        levels.push((sum / (to - from).max(1) as f64).sqrt());
    }
    let top = levels.iter().copied().fold(1e-9_f64, f64::max);
    let peak_at = levels.iter().position(|v| *v == top).map(|i| i as isize).unwrap_or(-1);
    let start = levels.iter().position(|v| *v > top * 0.01).unwrap_or(0);
    let attack_end = levels.iter().enumerate().position(|(i, v)| i >= start && *v >= top * 0.9).map(|i| i as isize).unwrap_or(-1);
    let fallen = levels.iter().enumerate().position(|(i, v)| i as isize > peak_at && *v < top * 0.1).map(|i| i as isize).unwrap_or(-1);
    let mut end = levels.len() as isize - 1;
    while end > start as isize && levels[end as usize] < top * 0.01 {
        end -= 1;
    }
    let ms = |steps: isize| steps as f64 * step as f64 / sample_rate * 1000.;
    let attack_ms = ms((attack_end - start as isize).max(0));
    let decay_ms = ms(((if fallen < 0 { end } else { fallen }) - peak_at).max(1));
    let audible_seconds = (step as f64 / sample_rate).max((end - start as isize + 1) as f64 * step as f64 / sample_rate);
    let [mut shelf, mut high_pass] = k_weighting(sample_rate);
    let block = round(sample_rate / 10.).max(1.) as usize;
    let mut blocks = vec![];
    let mut block_sum = 0.;
    let mut filled = 0;
    for (index, value) in mono.iter().enumerate() {
        let weighted = high_pass.process(shelf.process(*value as f64));
        block_sum += weighted * weighted;
        filled += 1;
        if filled == block || index == length - 1 {
            blocks.push(block_sum / filled as f64);
            block_sum = 0.;
            filled = 0;
        }
    }
    let loudest = blocks.iter().copied().fold(1e-20_f64, f64::max);
    let counted: Vec<_> = blocks.into_iter().filter(|v| *v > loudest / 100.).collect();
    let loudness_db = -0.691 + 10. * mean(&counted).max(1e-20).log10();
    let size = if sample_rate >= 32000. { 2048 } else { 1024 };
    let hop = size / 4;
    let hann = window(size, Window::Hann);
    let bank = mel_filters(size, sample_rate);
    let mut re = vec![0.; size];
    let mut im = vec![0.; size];
    let mut previous = vec![0.; size / 2 + 1];
    let mut power = vec![0.; size / 2 + 1];
    let bin_hz = sample_rate / size as f64;
    let flat_from = round(50. / bin_hz).max(1.) as usize;
    let flat_to = round(16000. / bin_hz).min(size as f64 / 2.) as usize;
    let mut chroma = vec![0.; 12];
    let mut frames = vec![];
    let (mut low, mut mid, mut high) = (0., 0., 0.);
    let mut flux = vec![];
    for from in (0..=length.max(size) - size).step_by(hop) {
        let mut energy = 0.;
        for index in 0..size {
            let value = mono.get(from + index).copied().unwrap_or(0.) as f64;
            re[index] = value * hann[index];
            im[index] = 0.;
            energy += value * value;
        }
        fft(&mut re, &mut im);
        let (mut total, mut weighted, mut log_sum, mut flat_sum, mut rise, mut magnitudes) = (0., 0., 0., 0., 0., 0.);
        for bin in 1..=size / 2 {
            let value = re[bin] * re[bin] + im[bin] * im[bin];
            power[bin] = value;
            total += value;
            weighted += value * bin as f64 * bin_hz;
            let magnitude = value.sqrt();
            rise += (magnitude - previous[bin]).max(0.);
            magnitudes += magnitude;
            previous[bin] = magnitude;
            if bin >= flat_from && bin <= flat_to {
                log_sum += (value + 1e-20).ln();
                flat_sum += value;
            }
            let hz = bin as f64 * bin_hz;
            if hz < 250. {
                low += value;
            } else if hz < 4000. {
                mid += value;
            } else {
                high += value;
            }
            if (55.0..=4500.).contains(&hz) {
                let pitch = round(69. + 12. * (hz / 440.).log2()) as i64;
                chroma[pitch.rem_euclid(12) as usize] += magnitude;
            }
        }
        flux.push(rise / magnitudes.max(1e-12));
        if total < 1e-14 {
            frames.push(Frame { energy: 0., centroid: 0., rolloff: 0., flatness: 0., flux: 0., mfcc: [0.; COEFFICIENTS] });
            continue;
        }
        let mut cumulative = 0.;
        let mut rolloff_bin = size / 2;
        for (bin, value) in power.iter().enumerate().take(size / 2 + 1).skip(1) {
            cumulative += value;
            if cumulative >= total * 0.85 {
                rolloff_bin = bin;
                break;
            }
        }
        let bins = (flat_to - flat_from + 1) as f64;
        let flatness = (log_sum / bins).exp() / (flat_sum / bins).max(1e-30);
        let mut bands = [0.; MEL_BANDS];
        for band in 0..MEL_BANDS {
            let mut sum = 0.;
            let row = &bank.weights[band];
            let first = bank.from[band];
            for bin in first..=bank.to[band] {
                sum += power[bin] * row[bin - first];
            }
            bands[band] = (sum + 1e-12).log10();
        }
        let mut mfcc = [0.; COEFFICIENTS];
        for k in 0..COEFFICIENTS {
            let mut sum = 0.;
            for (n, value) in bands.iter().enumerate() {
                sum += value * COSINES[k][n];
            }
            mfcc[k] = sum / MEL_BANDS as f64;
        }
        frames.push(Frame {
            energy,
            centroid: weighted / total,
            rolloff: rolloff_bin as f64 * bin_hz,
            flatness,
            flux: *flux.last().unwrap(),
            mfcc,
        });
    }
    let loudest_frame = frames.iter().map(|f| f.energy).fold(1e-20_f64, f64::max);
    let heard: Vec<_> = frames.iter().filter(|f| f.energy > loudest_frame * 1e-5).collect();
    let coefficient = |k: usize| heard.iter().map(|f| f.mfcc[k]).collect::<Vec<_>>();
    let centroids: Vec<_> = heard.iter().map(|f| f.centroid.max(20.).log2()).collect();
    let centroid_hz = 2_f64.powf(mean(&centroids));
    let rolloff_hz = 2_f64.powf(mean(&heard.iter().map(|f| f.rolloff.max(20.).log2()).collect::<Vec<_>>()));
    let flatness = mean(&heard.iter().map(|f| f.flatness).collect::<Vec<_>>());
    let flux_mean = mean(&heard.iter().map(|f| f.flux).collect::<Vec<_>>());
    let shares = low + mid + high;
    let low_share = low / shares.max(1e-30);
    let mid_share = mid / shares.max(1e-30);
    let high_share = high / shares.max(1e-30);
    let rate = sample_rate / hop as f64;
    let threshold = percentile(&flux, 0.5) + 1.5 * spread(&flux);
    let mut onsets = 0.;
    let mut last = f64::NEG_INFINITY;
    for index in 1..flux.len().saturating_sub(1) {
        let value = flux[index];
        if value > threshold && value >= flux[index - 1] && value >= flux[index + 1] && (index as f64 - last) / rate >= 0.05 {
            onsets += 1.;
            last = index as f64;
        }
    }
    let analyzed_seconds = length as f64 / sample_rate;
    let onsets_per_second = onsets / analyzed_seconds.max(0.25);
    let rhythm = if analyzed_seconds >= 3. { estimate_tempo(&flux, rate) } else { None };
    let pitch_from = start * step;
    let tracked = track_pitch(&mono[pitch_from.min(length)..length.min((pitch_from as f64 + sample_rate + 4096.) as usize)], sample_rate);
    let voiced: Vec<_> = tracked.iter().filter(|f| f.confidence > 0.6 && f.hz > 25.).collect();
    let pitch = if voiced.len() as f64 >= (tracked.len() as f64 * 0.3).max(2.) {
        Some(Pitch {
            hz: round(percentile(&voiced.iter().map(|f| f.hz).collect::<Vec<_>>(), 0.5) * 10.) / 10.,
            confidence: round(voiced.len() as f64 / tracked.len() as f64 * 100.) / 100.,
        })
    } else {
        None
    };
    let key = estimate_key(&chroma);
    let rms = (squares / length.max(1) as f64).sqrt();
    let crest_db = db_amplitude(peak) - db_amplitude(rms);
    let width = sides / (squares + sides).max(1e-30);
    let mut vector: Vec<_> = (1..=12).map(|k| mean(&coefficient(k))).chain((0..13).map(|k| spread(&coefficient(k)))).collect();
    vector.extend([
        centroid_hz.log2(),
        spread(&centroids),
        rolloff_hz.log2(),
        flatness.max(1e-4).log10(),
        flux_mean,
        (crossings / length.max(1) as f64).max(1e-4).log10(),
        low_share,
        mid_share,
        high_share,
        (attack_ms + 1.).log10(),
        (decay_ms + 1.).log10(),
        crest_db / 10.,
        (audible_seconds + 0.01).log10(),
        onsets_per_second.ln_1p(),
        width,
    ]);
    for value in &mut vector {
        *value = if value.is_finite() { round(*value * 10000.) / 10000. } else { 0. };
    }
    SoundFeatures {
        seconds: rounded(seconds, 3),
        audible_seconds: rounded(audible_seconds, 3),
        loudness_db: rounded(loudness_db, 1),
        peak_db: rounded(db_amplitude(peak), 1),
        crest_db: rounded(crest_db, 1),
        centroid_hz: round(centroid_hz),
        rolloff_hz: round(rolloff_hz),
        flatness: rounded(flatness, 3),
        attack_ms: round(attack_ms),
        decay_ms: round(decay_ms),
        onsets_per_second: rounded(onsets_per_second, 2),
        width: rounded(width, 2),
        low_share: rounded(low_share, 3),
        mid_share: rounded(mid_share, 3),
        high_share: rounded(high_share, 3),
        pitch,
        key,
        rhythm,
        vector,
    }
}
#[derive(Default, Clone)]
pub struct MeasureOptions {
    pub signal: Option<Signal>,
    pub start: Option<f64>,
    pub seconds: Option<f64>,
}
/// Measures at most 30 seconds, decoding non-PCM formats through the same audio source as listening.
pub async fn measure_sound(path: &str, options: MeasureOptions) -> Result<SoundFeatures, AudioError> {
    let reach = options.start.unwrap_or(0.).max(0.) + options.seconds.unwrap_or(MAX_SECONDS).clamp(0., MAX_SECONDS) + 1.;
    let mut source = open_audio_to(path, options.signal.clone(), Some(reach)).await?;
    let result = async {
        let total = source.frames as f64 / source.sample_rate;
        let start_frame = (options.start.unwrap_or(0.) * source.sample_rate).floor().min(source.frames as f64).max(0.) as usize;
        let frames = (source.frames - start_frame)
            .min((options.seconds.unwrap_or(MAX_SECONDS).min(MAX_SECONDS) * source.sample_rate).floor().max(0.) as usize);
        if (frames as f64) < source.sample_rate * 0.005 {
            return Err(AudioError("There's no audio in that part of the file.".into()));
        }
        source.seek(start_frame as f64);
        let mut channels = vec![vec![0_f32; frames]; source.channels.min(2)];
        let mut done = 0;
        while done < frames {
            if let Some(signal) = &options.signal {
                signal.check()?;
            }
            let Some(block) = source.read((frames - done).min(65536)).await? else { break };
            for (channel, target) in channels.iter_mut().enumerate() {
                let count = block[channel].len().min(frames - done);
                target[done..done + count].copy_from_slice(&block[channel][..count]);
            }
            done += block[0].len();
        }
        for channel in &mut channels {
            channel.truncate(done);
        }
        Ok(measure_samples(&channels, source.sample_rate, total))
    }
    .await;
    source.close().await?;
    result
}
