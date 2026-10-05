//! A song's form from bar-by-bar chroma, timbre, level and low-end similarity.

use super::{
    analyze::{onset_peaks, onset_strength},
    decode::{open_audio, AudioError},
    dsp::{clock, db, fft, percentile, round, window, Biquad, Window},
};
use kumi_common::{
    abort::{Signal, SignalExt},
    js::number::{round as js_round, to_string},
};
use serde::{Deserialize, Serialize};
use std::f64::consts::{PI, SQRT_2};
pub const FORM_VERSION: u32 = 1;
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FormSection {
    pub from: String,
    pub to: String,
    pub bar: usize,
    pub bars: usize,
    pub energy: f64,
    pub level: String,
    pub density: String,
    pub low: String,
    pub like: String,
    pub role: String,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FormTempo {
    pub bpm: f64,
    pub from: String,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Form {
    pub kumi_form: u32,
    pub file: String,
    pub seconds: f64,
    pub tempo: FormTempo,
    pub beats_per_bar: f64,
    pub bars: usize,
    pub sections: Vec<FormSection>,
    pub summary: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub at_set_tempo: Option<String>,
}
#[derive(Debug, Clone, Default)]
pub struct FormOptions {
    pub tempo: Option<f64>,
    pub beats_per_bar: Option<f64>,
    pub signal: Option<Signal>,
}
pub async fn hear_form(path: &str, options: FormOptions) -> Result<Form, AudioError> {
    let mut source = open_audio(path, options.signal.clone()).await?;
    let result = async {
        let rate = source.sample_rate;
        if !(8000.0..=384000.0).contains(&rate) {
            return Err(AudioError(format!("A sample rate of {} Hz isn't one Kumi can analyze.", to_string(rate))));
        }
        let frames = source.frames.min((900.0 * rate).floor() as usize);
        let mut meter = FormMeter::new(rate);
        let mut done = 0;
        while done < frames {
            if let Some(signal) = &options.signal {
                signal.check()?;
            }
            let Some(block) = source.read(65536.min(frames - done)).await? else {
                break;
            };
            meter.push(&block);
            done += block[0].len();
            tokio::task::yield_now().await;
        }
        Ok(meter.form(path.rsplit(['\\', '/']).next().unwrap_or(path), source.frames as f64 / rate, &options))
    }
    .await;
    source.close().await?;
    result
}
const MEL_BANDS: usize = 24;
const TIMBRE: usize = 12;
fn mel_bank(rate: f64, size: usize) -> Vec<(usize, Vec<f64>)> {
    let mel = |hz: f64| 2595.0 * (1.0 + hz / 700.0).log10();
    let hz = |v: f64| 700.0 * (10f64.powf(v / 2595.0) - 1.0);
    let low = mel(40.0);
    let high = mel(16000f64.min(rate / 2.0 - 1.0));
    let edges: Vec<_> =
        (0..MEL_BANDS + 2).map(|i| hz(low + (high - low) * i as f64 / (MEL_BANDS + 1) as f64) * size as f64 / rate).collect();
    (0..MEL_BANDS)
        .map(|band| {
            let (left, center, right) = (edges[band], edges[band + 1], edges[band + 2]);
            let from = 1.max(left.floor() as usize);
            let to = (size / 2).min(right.ceil() as usize);
            let weights = (from..=to)
                .map(|bin| {
                    if (bin as f64) < center {
                        ((bin as f64 - left) / (center - left).max(1e-9)).max(0.0)
                    } else {
                        ((right - bin as f64) / (right - center).max(1e-9)).max(0.0)
                    }
                })
                .collect();
            (from, weights)
        })
        .collect()
}
struct FormMeter {
    rate: f64,
    size: usize,
    hop: usize,
    ring: Vec<f64>,
    filled: usize,
    re: Vec<f64>,
    im: Vec<f64>,
    mels: Vec<(usize, Vec<f64>)>,
    chroma_of: Vec<Option<usize>>,
    low_bins: usize,
    chroma: Vec<[f64; 12]>,
    timbre: Vec<[f64; 12]>,
    level: Vec<f64>,
    low_share: Vec<f64>,
    onset_low: Biquad,
    onset_high: Biquad,
    onset_energy: [f64; 3],
    onset_count: usize,
    onset_frames: Vec<[f64; 3]>,
}
impl FormMeter {
    fn new(rate: f64) -> Self {
        let size = if rate > 50000.0 { 8192 } else { 4096 };
        let hop = size / 2;
        let mut chroma_of = vec![None; size / 2 + 1];
        for (bin, class) in chroma_of.iter_mut().enumerate().skip(1) {
            let hz = bin as f64 * rate / size as f64;
            if (55.0..=4500.0).contains(&hz) {
                *class = Some((js_round(69.0 + 12.0 * (hz / 440.0).log2()) as i64).rem_euclid(12) as usize);
            }
        }
        let low = (PI * 150.0 / rate).tan();
        let ln = 1.0 / (1.0 + low * SQRT_2 + low * low);
        let high = (PI * 2500.0 / rate).tan();
        let hn = 1.0 / (1.0 + high * SQRT_2 + high * high);
        Self {
            rate,
            size,
            hop,
            ring: vec![0.0; size],
            filled: 0,
            re: vec![0.0; size],
            im: vec![0.0; size],
            mels: mel_bank(rate, size),
            chroma_of,
            low_bins: js_round(150.0 * size as f64 / rate) as usize,
            chroma: Vec::new(),
            timbre: Vec::new(),
            level: Vec::new(),
            low_share: Vec::new(),
            onset_low: Biquad::new(
                low * low * ln,
                2.0 * low * low * ln,
                low * low * ln,
                2.0 * (low * low - 1.0) * ln,
                (1.0 - low * SQRT_2 + low * low) * ln,
            ),
            onset_high: Biquad::new(hn, -2.0 * hn, hn, 2.0 * (high * high - 1.0) * hn, (1.0 - high * SQRT_2 + high * high) * hn),
            onset_energy: [0.0; 3],
            onset_count: 0,
            onset_frames: Vec::new(),
        }
    }
    fn push(&mut self, block: &[Vec<f32>]) {
        let left = &block[0];
        let right = block.get(1).unwrap_or(left);
        for i in 0..left.len() {
            let mono = (left[i] as f64 + right[i] as f64) / 2.0;
            self.ring[self.filled % self.size] = mono;
            self.filled += 1;
            if self.filled >= self.size && (self.filled - self.size) % self.hop == 0 {
                self.spectrum();
            }
            let low = self.onset_low.process(mono);
            let high = self.onset_high.process(mono);
            self.onset_energy[0] += low * low;
            self.onset_energy[1] += mono * mono;
            self.onset_energy[2] += high * high;
            self.onset_count += 1;
            if self.onset_count == 512 {
                self.onset_frames.push(self.onset_energy);
                self.onset_energy = [0.0; 3];
                self.onset_count = 0;
            }
        }
    }
    fn spectrum(&mut self) {
        let hann = window(self.size, Window::Hann);
        let start = self.filled % self.size;
        for i in 0..self.size {
            self.re[i] = self.ring[(start + i) % self.size] * hann[i];
            self.im[i] = 0.0;
        }
        fft(&mut self.re, &mut self.im);
        let mut chroma = [0.0; 12];
        let (mut total, mut low) = (0.0, 0.0);
        let mut power = vec![0.0; self.size / 2 + 1];
        for (bin, p) in power.iter_mut().enumerate().skip(1) {
            let value = self.re[bin].powi(2) + self.im[bin].powi(2);
            *p = value;
            total += value;
            if bin <= self.low_bins {
                low += value;
            }
            if let Some(pitch) = self.chroma_of[bin] {
                chroma[pitch] += value.sqrt();
            }
        }
        let bands: Vec<_> = self
            .mels
            .iter()
            .map(|(from, weights)| {
                (1e-12 + weights.iter().enumerate().map(|(at, w)| w * power.get(from + at).copied().unwrap_or(0.0)).sum::<f64>()).log10()
            })
            .collect();
        let mut timbre = [0.0; TIMBRE];
        for coefficient in 1..=TIMBRE {
            timbre[coefficient - 1] =
                (0..MEL_BANDS).map(|band| bands[band] * (PI * coefficient as f64 * (band as f64 + 0.5) / MEL_BANDS as f64).cos()).sum();
        }
        self.chroma.push(chroma);
        self.timbre.push(timbre);
        self.level.push(db(total));
        self.low_share.push(if total > 1e-20 { low / total } else { 0.0 });
    }
    fn form(&self, file: &str, seconds: f64, options: &FormOptions) -> Form {
        let beats_per_bar = options.beats_per_bar.unwrap_or(4.0);
        let onsets = onset_strength(&self.onset_frames);
        let onset_rate = self.rate / 512.0;
        let found = estimate_tempo(&onsets, onset_rate, options.tempo);
        let bpm = found.or(options.tempo).unwrap_or(120.0);
        let tempo = FormTempo { bpm: round(bpm, 1), from: if found.is_some() { "file" } else { "set" }.into() };
        let frame_seconds = self.hop as f64 / self.rate;
        let loudest = self.level.iter().copied().fold(-200.0, f64::max);
        let first = self.level.iter().position(|v| *v > loudest - 40.0).unwrap_or(0);
        let bar_seconds = beats_per_bar * 60.0 / bpm;
        let origin = first as f64 * frame_seconds;
        let count = ((seconds - origin) / bar_seconds + 0.25).floor().max(0.0) as usize;
        let mut form = Form {
            kumi_form: FORM_VERSION,
            file: file.into(),
            seconds: round(seconds, 2),
            tempo,
            beats_per_bar,
            bars: count,
            sections: Vec::new(),
            summary: "too short to have a form".into(),
            at_set_tempo: None,
        };
        if count < 4 || self.level.is_empty() {
            return form;
        }
        struct Bar {
            chroma: Vec<f64>,
            timbre: Vec<f64>,
            level: f64,
            low: f64,
            hits: f64,
        }
        let peaks = onset_peaks(&onsets);
        let bars: Vec<_> = (0..count)
            .map(|bar| {
                let from = ((origin + bar as f64 * bar_seconds) / frame_seconds).floor() as usize;
                let to = (from + 1).max(((origin + (bar + 1) as f64 * bar_seconds) / frame_seconds).floor() as usize);
                let frames: Vec<_> = (from..to).filter(|f| *f < self.level.len()).collect();
                let mean = |pick: &dyn Fn(usize) -> f64| {
                    if frames.is_empty() {
                        0.0
                    } else {
                        frames.iter().map(|f| pick(*f)).sum::<f64>() / frames.len() as f64
                    }
                };
                let mut chroma: Vec<_> = (0..12).map(|pitch| mean(&|f| self.chroma[f][pitch])).collect();
                let norm = chroma.iter().map(|v| v * v).sum::<f64>().sqrt();
                let norm = if norm == 0.0 { 1.0 } else { norm };
                for v in &mut chroma {
                    *v /= norm;
                }
                let hits = peaks
                    .iter()
                    .filter(|at| {
                        **at as f64 / onset_rate >= origin + bar as f64 * bar_seconds
                            && (**at as f64 / onset_rate) < origin + (bar + 1) as f64 * bar_seconds
                    })
                    .count() as f64;
                Bar {
                    chroma,
                    timbre: (0..TIMBRE).map(|i| mean(&|f| self.timbre[f][i])).collect(),
                    level: 10.0 * (1e-20 + mean(&|f| 10f64.powf(self.level[f] / 10.0))).log10(),
                    low: mean(&|f| self.low_share[f]),
                    hits,
                }
            })
            .collect();
        let z = |values: Vec<f64>| {
            let mean = values.iter().sum::<f64>() / values.len() as f64;
            let spread = (values.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / values.len() as f64).sqrt();
            let spread = if spread == 0.0 { 1.0 } else { spread };
            values.iter().map(|v| (v - mean) / spread).collect::<Vec<_>>()
        };
        let timbres: Vec<_> = (0..TIMBRE).map(|i| z(bars.iter().map(|b| b.timbre[i]).collect())).collect();
        let levels = z(bars.iter().map(|b| b.level).collect());
        let lows = z(bars.iter().map(|b| b.low).collect());
        let vectors: Vec<Vec<_>> = bars
            .iter()
            .enumerate()
            .map(|(at, bar)| {
                bar.chroma
                    .iter()
                    .map(|v| v * 2.0)
                    .chain(timbres.iter().map(|c| c[at] / (TIMBRE as f64).sqrt() * 2.0))
                    .chain([levels[at] * 1.5, lows[at]])
                    .collect()
            })
            .collect();
        let matrix: Vec<Vec<_>> = vectors.iter().map(|row| vectors.iter().map(|other| similar(row, other)).collect()).collect();
        let edges = boundaries(&matrix, count);
        let starts: Vec<_> = std::iter::once(0).chain(edges).collect();
        let sections: Vec<_> = starts.iter().enumerate().map(|(i, from)| (*from, starts.get(i + 1).copied().unwrap_or(count))).collect();
        let section_level: Vec<_> = sections
            .iter()
            .map(|(from, to)| {
                10.0 * (1e-20 + bars[*from..*to].iter().map(|b| 10f64.powf(b.level / 10.0)).sum::<f64>() / (to - from) as f64).log10()
            })
            .collect();
        let loudest_section = section_level.iter().copied().fold(f64::NEG_INFINITY, f64::max);
        let hits_per_bar: Vec<_> =
            sections.iter().map(|(from, to)| bars[*from..*to].iter().map(|b| b.hits).sum::<f64>() / (to - from) as f64).collect();
        let usual = percentile(&bars.iter().map(|b| b.hits).collect::<Vec<_>>(), 0.5);
        let usual = if usual == 0.0 { 1.0 } else { usual };
        let low_end: Vec<_> =
            sections.iter().map(|(from, to)| bars[*from..*to].iter().map(|b| b.low).sum::<f64>() / (to - from) as f64).collect();
        let fullest = low_end.iter().copied().fold(1e-9, f64::max);
        let mut letters: Vec<(String, Vec<f64>)> = Vec::new();
        let like: Vec<_> = sections
            .iter()
            .map(|(from, to)| {
                let centre: Vec<_> =
                    (0..vectors[0].len()).map(|i| vectors[*from..*to].iter().map(|v| v[i]).sum::<f64>() / (to - from) as f64).collect();
                let mut matched = None;
                let mut best = f64::NEG_INFINITY;
                for (letter, prior) in &letters {
                    let score = similar(prior, &centre);
                    if score > best {
                        best = score;
                        matched = Some(letter.clone());
                    }
                }
                if best >= 0.8 {
                    return matched.unwrap();
                }
                let letter = char::from_u32(65 + letters.len().min(25) as u32).unwrap().to_string();
                letters.push((letter.clone(), centre));
                letter
            })
            .collect();
        let energy: Vec<_> = section_level.iter().map(|v| round(1.0 + (v - loudest_section) / 18.0, 2).clamp(0.0, 1.0)).collect();
        let level: Vec<_> = section_level
            .iter()
            .map(|v| {
                if *v > loudest_section - 3.0 {
                    "high"
                } else if *v > loudest_section - 8.0 {
                    "mid"
                } else {
                    "low"
                }
            })
            .collect();
        let at = |bar: usize| clock(seconds.min(origin + bar as f64 * bar_seconds));
        form.sections = sections
            .iter()
            .enumerate()
            .map(|(i, (from, to))| {
                let role = if level[i] == "high" {
                    "peak"
                } else if i == 0 {
                    "intro"
                } else if i == sections.len() - 1 {
                    "outro"
                } else if level[i + 1] == "high" && energy[i] >= energy[i - 1] - 0.05 {
                    "build"
                } else if level[i] == "low" {
                    "break"
                } else {
                    "main"
                };
                FormSection {
                    from: at(*from),
                    to: at(*to),
                    bar: from + 1,
                    bars: to - from,
                    energy: energy[i],
                    level: level[i].into(),
                    density: if hits_per_bar[i] < usual * 0.6 {
                        "sparse"
                    } else if hits_per_bar[i] > usual * 1.4 {
                        "busy"
                    } else {
                        "steady"
                    }
                    .into(),
                    low: if low_end[i] >= fullest * 0.6 { "full" } else { "thin" }.into(),
                    like: like[i].clone(),
                    role: role.into(),
                }
            })
            .collect();
        form.summary = format!(
            "{} ({count} bars at {} BPM)",
            form.sections.iter().map(|s| format!("{} {}", s.role, s.bars)).collect::<Vec<_>>().join(" · "),
            to_string(form.tempo.bpm)
        );
        form.at_set_tempo = options
            .tempo
            .filter(|v| *v != 0.0)
            .map(|tempo| format!("{} at the Set's {} BPM", clock(count as f64 * beats_per_bar * 60.0 / tempo), to_string(round(tempo, 2))));
        form
    }
}
fn similar(a: &[f64], b: &[f64]) -> f64 {
    let (mut dot, mut left, mut right) = (0.0, 0.0, 0.0);
    for (x, y) in a.iter().zip(b) {
        dot += x * y;
        left += x * x;
        right += y * y;
    }
    dot / (left * right).max(1e-12).sqrt()
}
/// Section edges from diagonal novelty, snapped to nearby four-bar phrases and at least four bars apart.
pub fn boundaries(matrix: &[Vec<f64>], count: usize) -> Vec<usize> {
    let half = if count >= 32 { 4 } else { 2 };
    let sigma = half as f64 / 2.0;
    let novelty: Vec<_> = (0..count)
        .map(|bar| {
            if bar < 2 || bar > count.saturating_sub(2) {
                return 0.0;
            }
            let (mut sum, mut weight) = (0.0, 0.0);
            for a in -half..half {
                for b in -half..half {
                    let i = bar as i64 + a;
                    let j = bar as i64 + b;
                    if i < 0 || j < 0 || i >= count as i64 || j >= count as i64 {
                        continue;
                    }
                    let taper = (-((a as f64 + 0.5).powi(2) + (b as f64 + 0.5).powi(2)) / (2.0 * sigma * sigma)).exp();
                    sum += (if a < 0 { -1.0 } else { 1.0 }) * (if b < 0 { -1.0 } else { 1.0 }) * taper * matrix[i as usize][j as usize];
                    weight += taper;
                }
            }
            if weight != 0.0 {
                sum / weight
            } else {
                0.0
            }
        })
        .collect();
    let positive: Vec<_> = novelty.iter().copied().filter(|v| *v > 0.0).collect();
    if positive.is_empty() {
        return Vec::new();
    }
    let mean = positive.iter().sum::<f64>() / positive.len() as f64;
    let spread = (positive.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / positive.len() as f64).sqrt();
    let mut peaks: Vec<_> = novelty
        .iter()
        .enumerate()
        .filter(|(bar, value)| {
            **value > 0.05f64.max(mean + 0.25 * spread)
                && **value >= bar.checked_sub(1).map_or(0.0, |i| novelty[i])
                && **value >= novelty.get(bar + 1).copied().unwrap_or(0.0)
        })
        .collect();
    peaks.sort_by(|a, b| b.1.total_cmp(a.1));
    let mut chosen = Vec::new();
    for (bar, _) in peaks {
        let phrase = js_round(bar as f64 / 4.0) as usize * 4;
        let snapped = if phrase.abs_diff(bar) <= 1 && phrase >= 2 && phrase <= count.saturating_sub(2) { phrase } else { bar };
        if chosen.iter().all(|other: &usize| other.abs_diff(snapped) >= 4) {
            chosen.push(snapped);
        }
    }
    chosen.sort_unstable();
    chosen
}
fn estimate_tempo(strength: &[f64], rate: f64, near: Option<f64>) -> Option<f64> {
    let min = (rate * 60.0 / 200.0).floor() as usize;
    let max = rate.ceil() as usize;
    if strength.len() < max * 3 {
        return None;
    }
    let mean = strength.iter().sum::<f64>() / strength.len() as f64;
    let centered: Vec<_> = strength.iter().map(|v| v - mean).collect();
    let zero = centered.iter().map(|v| v * v).sum::<f64>();
    if zero <= 0.0 {
        return None;
    }
    let (mut best, mut best_score, mut raw) = (None, f64::NEG_INFINITY, 0.0);
    for lag in min..=max {
        let score = (lag..centered.len()).map(|i| centered[i] * centered[i - lag]).sum::<f64>() / zero;
        let bpm = 60.0 * rate / lag as f64;
        let weight = (-0.5 * ((bpm / 125.0).log2() / 0.9).powi(2)).exp();
        if score * weight > best_score {
            best_score = score * weight;
            best = Some(lag);
            raw = score;
        }
    }
    let best = best?;
    if raw <= 0.02 {
        return None;
    }
    let bpm = 60.0 * rate / best as f64;
    let Some(near) = near.filter(|v| *v != 0.0) else {
        return Some(bpm);
    };
    let mut octaves: Vec<_> = [bpm / 2.0, bpm, bpm * 2.0].into_iter().filter(|v| (40.0..=300.0).contains(v)).collect();
    octaves.sort_by(|a, b| (a / near).log2().abs().total_cmp(&(b / near).log2().abs()));
    octaves.first().copied()
}
