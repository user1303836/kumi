//! What the designs read from their sources: where a hit starts and ends, its onsets, loudness, pitch, partials and
//! colour, and the short-time spectra they're rebuilt from.

use super::RATE;
use kumi_media::audio::dsp::{fft, k_weighting};
use std::f64::consts::{PI, TAU};

pub type Stereo = [Vec<f32>; 2];

pub fn mono(s: &Stereo) -> Vec<f32> {
    s[0].iter().zip(&s[1]).map(|(l, r)| 0.5 * (l + r)).collect()
}

/// RMS over windows of `2 * hop` samples, one value a hop.
pub fn envelope(x: &[f32], hop: usize) -> Vec<f32> {
    let count = x.len().div_ceil(hop).max(1);
    (0..count)
        .map(|i| {
            let (from, to) = ((i * hop).saturating_sub(hop / 2), (i * hop + hop + hop / 2).min(x.len()));
            if from >= to {
                return 0.0;
            }
            let sum: f64 = x[from..to].iter().map(|v| (*v as f64).powi(2)).sum();
            (sum / (to - from) as f64).sqrt() as f32
        })
        .collect()
}

const ENV_HOP: usize = 64;

/// Where the sound's first hit begins: the first moment its level reaches a twentieth of its loudest, walked back over
/// the rise before it (at most ~10 ms).
pub fn start_of(x: &[f32]) -> usize {
    let env = envelope(x, ENV_HOP);
    let peak = env.iter().copied().fold(0.0f32, f32::max);
    if peak <= 0.0 {
        return 0;
    }
    let Some(mut at) = env.iter().position(|v| *v >= peak * 0.05) else { return 0 };
    let mut back = 0;
    while at > 0 && back < 8 && env[at - 1] < env[at] && env[at - 1] > peak * 0.003 {
        at -= 1;
        back += 1;
    }
    (at * ENV_HOP).saturating_sub(ENV_HOP / 2)
}

/// Where the sound has died away: past the last moment its level is within 60 dB of its loudest.
pub fn end_of(x: &[f32]) -> usize {
    let env = envelope(x, ENV_HOP);
    let peak = env.iter().copied().fold(0.0f32, f32::max);
    if peak <= 0.0 {
        return 0;
    }
    let last = env.iter().rposition(|v| *v >= peak * 0.001).unwrap_or(0);
    ((last + 2) * ENV_HOP).min(x.len())
}

/// The loudest moment within `within` samples of `from`.
pub fn peak_after(x: &[f32], from: usize, within: usize) -> usize {
    let to = (from + within).min(x.len());
    (from..to).max_by(|a, b| x[*a].abs().total_cmp(&x[*b].abs())).unwrap_or(from)
}

/// A periodic Hann window, unnormalized.
pub fn hann(n: usize) -> Vec<f64> {
    (0..n).map(|i| 0.5 - 0.5 * (TAU * i as f64 / n as f64).cos()).collect()
}

/// One frame's spectrum, bins 0 to n/2.
#[derive(Clone)]
pub struct Frame {
    pub re: Vec<f64>,
    pub im: Vec<f64>,
}
impl Frame {
    pub fn magnitude(&self, k: usize) -> f64 {
        self.re[k].hypot(self.im[k])
    }
    pub fn phase(&self, k: usize) -> f64 {
        self.im[k].atan2(self.re[k])
    }
}

/// The spectrum of `x` windowed around sample `center` (negative or past the end reads silence).
pub fn frame_at(x: &[f32], center: i64, window: &[f64]) -> Frame {
    let n = window.len();
    let mut re = vec![0.0; n];
    let mut im = vec![0.0; n];
    let from = center - n as i64 / 2;
    for (i, w) in window.iter().enumerate() {
        let at = from + i as i64;
        if at >= 0 && (at as usize) < x.len() {
            re[i] = x[at as usize] as f64 * w;
        }
    }
    fft(&mut re, &mut im);
    re.truncate(n / 2 + 1);
    im.truncate(n / 2 + 1);
    Frame { re, im }
}

/// A real frame back from its half spectrum, unwindowed.
pub fn inverse(frame: &Frame, n: usize) -> Vec<f64> {
    let mut re = vec![0.0; n];
    let mut im = vec![0.0; n];
    for k in 0..=n / 2 {
        re[k] = frame.re[k];
        im[k] = -frame.im[k];
        if k > 0 && k < n / 2 {
            re[n - k] = frame.re[k];
            im[n - k] = frame.im[k];
        }
    }
    fft(&mut re, &mut im);
    re.iter().map(|v| v / n as f64).collect()
}

/// Overlap-adds synthesized frames (windowed again) into a signal, normalized by the windows' overlap.
pub struct Synth {
    out: Vec<f64>,
    weight: Vec<f64>,
    window: Vec<f64>,
}
impl Synth {
    pub fn new(len: usize, n: usize) -> Self {
        Self { out: vec![0.0; len + n], weight: vec![0.0; len + n], window: hann(n) }
    }
    /// Adds a frame centred on `center` (samples).
    pub fn add(&mut self, frame: &Frame, center: usize) {
        let n = self.window.len();
        let samples = inverse(frame, n);
        for i in 0..n {
            let at = center + i;
            if at < n / 2 {
                continue;
            }
            let at = at - n / 2;
            if at < self.out.len() {
                self.out[at] += samples[i] * self.window[i];
                self.weight[at] += self.window[i] * self.window[i];
            }
        }
    }
    pub fn finish(self, len: usize) -> Vec<f32> {
        let floor = self.weight.iter().copied().fold(0.0, f64::max) * 0.1;
        (0..len).map(|i| (self.out[i] / self.weight[i].max(floor).max(1e-9)) as f32).collect()
    }
}

/// Wraps a phase into (-pi, pi].
pub fn wrap(phase: f64) -> f64 {
    let p = (phase + PI).rem_euclid(TAU) - PI;
    if p == -PI {
        PI
    } else {
        p
    }
}

/// Onsets (sample positions) by positive spectral flux, peak-picked against a moving median, at least 50 ms apart.
pub fn onsets(x: &[f32]) -> Vec<usize> {
    let (n, hop) = (1024usize, 256usize);
    let window = hann(n);
    let frames = x.len() / hop + 1;
    let mut previous = vec![0.0f64; n / 2 + 1];
    let mut flux = Vec::with_capacity(frames);
    for t in 0..frames {
        let frame = frame_at(x, (t * hop) as i64, &window);
        let mut sum = 0.0;
        for (k, prev) in previous.iter_mut().enumerate() {
            let m = (1.0 + 1000.0 * frame.magnitude(k)).ln();
            sum += (m - *prev).max(0.0);
            *prev = m;
        }
        flux.push(sum);
    }
    let max = flux.iter().copied().fold(0.0, f64::max);
    if max <= 0.0 {
        return Vec::new();
    }
    let env = envelope(x, ENV_HOP);
    let loudest = env.iter().copied().fold(0.0f32, f32::max);
    let mut found: Vec<usize> = Vec::new();
    for t in 0..frames {
        let (from, to) = (t.saturating_sub(8), (t + 9).min(frames));
        let mut around: Vec<f64> = flux[from..to].to_vec();
        around.sort_by(f64::total_cmp);
        let threshold = around[around.len() / 2] * 1.5 + max * 0.08;
        let local = (t.saturating_sub(3)..(t + 4).min(frames)).all(|u| flux[u] <= flux[t]);
        if flux[t] <= threshold || !local {
            continue;
        }
        // The rise within the frame's reach: the hop before the level climbs most.
        let (lo, hi) = ((t * hop).saturating_sub(n / 2) / ENV_HOP, ((t * hop + n / 2) / ENV_HOP).min(env.len().saturating_sub(1)));
        let mut at = lo;
        let mut best = f32::MIN;
        for i in lo.max(1)..=hi {
            let rise = env[i] - env[i - 1];
            if rise > best {
                best = rise;
                at = i - 1;
            }
        }
        // A quiet rise isn't a hit worth slicing.
        let level = env[at..=(at + 4).min(env.len() - 1)].iter().copied().fold(0.0f32, f32::max);
        if level < loudest * 0.05 {
            continue;
        }
        let sample = at * ENV_HOP;
        if found.last().is_none_or(|last| sample >= last + (0.05 * RATE) as usize) {
            found.push(sample);
        }
    }
    found
}

/// Integrated loudness (BS.1770, gated) in LUFS; a sound shorter than a block is measured whole. Silence is -100.
pub fn lufs(s: &Stereo) -> f64 {
    let len = s[0].len();
    let mut weighted = [Vec::with_capacity(len), Vec::with_capacity(len)];
    for c in 0..2 {
        let [mut a, mut b] = k_weighting(RATE);
        weighted[c] = s[c].iter().map(|v| b.process(a.process(*v as f64))).collect::<Vec<f64>>();
    }
    let block = (0.4 * RATE) as usize;
    let step = block / 4;
    let power = |from: usize, to: usize| -> f64 {
        let mut sum = 0.0;
        for w in &weighted {
            sum += w[from..to].iter().map(|v| v * v).sum::<f64>() / (to - from).max(1) as f64;
        }
        sum
    };
    let loudness = |p: f64| if p > 0.0 { -0.691 + 10.0 * p.log10() } else { -100.0 };
    if len < block {
        return loudness(power(0, len.max(1).min(len))).max(-100.0);
    }
    let blocks: Vec<f64> = (0..=(len - block) / step).map(|i| power(i * step, i * step + block)).collect();
    let absolute: Vec<f64> = blocks.iter().copied().filter(|p| loudness(*p) > -70.0).collect();
    if absolute.is_empty() {
        return -100.0;
    }
    let mean = absolute.iter().sum::<f64>() / absolute.len() as f64;
    let gate = loudness(mean) - 10.0;
    let relative: Vec<f64> = absolute.into_iter().filter(|p| loudness(*p) > gate).collect();
    loudness(relative.iter().sum::<f64>() / relative.len().max(1) as f64)
}

/// The fundamental (Hz) of the sustained part after `from`, by YIN, when it's clear; None for noise and clicks.
pub fn pitch(x: &[f32], from: usize) -> Option<f64> {
    let n = 2048usize;
    let at = (from + (0.03 * RATE) as usize).min(x.len());
    if x.len() < at + n + 1200 {
        return None;
    }
    let (min_lag, max_lag) = ((RATE / 1500.0) as usize, (RATE / 40.0) as usize);
    let frame = &x[at..at + n + max_lag];
    let mut diff = vec![0.0f64; max_lag + 1];
    for (lag, d) in diff.iter_mut().enumerate().skip(1) {
        *d = (0..n).map(|i| (frame[i] as f64 - frame[i + lag] as f64).powi(2)).sum();
    }
    let mut running = 0.0;
    let mut cmnd = vec![1.0f64; max_lag + 1];
    for lag in 1..=max_lag {
        running += diff[lag];
        cmnd[lag] = if running > 0.0 { diff[lag] * lag as f64 / running } else { 1.0 };
    }
    let mut lag = min_lag;
    while lag < max_lag {
        if cmnd[lag] < 0.15 {
            while lag + 1 < max_lag && cmnd[lag + 1] < cmnd[lag] {
                lag += 1;
            }
            let (a, b, c) = (cmnd[lag - 1], cmnd[lag], cmnd[lag + 1]);
            let shift = 0.5 * (a - c) / (a - 2.0 * b + c).max(1e-12);
            return Some(RATE / (lag as f64 + shift.clamp(-1.0, 1.0)));
        }
        lag += 1;
    }
    None
}

/// A partial of a ringing sound: its frequency, its level against the strongest, and its 60 dB decay in seconds.
#[derive(Clone, Copy, Debug)]
pub struct Partial {
    pub hz: f64,
    pub amp: f64,
    pub t60: f64,
}

/// The `count` strongest partials of the sound after `from`, with how long each rings.
pub fn partials(x: &[f32], from: usize, count: usize) -> Vec<Partial> {
    let n = 8192usize;
    let hop = 2048usize;
    let window = hann(n);
    let start = from + (0.01 * RATE) as usize;
    let first = frame_at(x, (start + n / 2) as i64, &window);
    let mags: Vec<f64> = (0..=n / 2).map(|k| first.magnitude(k)).collect();
    let max = mags.iter().copied().fold(0.0, f64::max);
    if max <= 0.0 {
        return Vec::new();
    }
    let bin_hz = RATE / n as f64;
    let mut peaks: Vec<(f64, f64, usize)> = (2..n / 2 - 1)
        .filter(|&k| mags[k] > mags[k - 1] && mags[k] >= mags[k + 1] && mags[k] > max * 1e-3 && (k as f64 * bin_hz) > 30.0)
        .map(|k| {
            let (a, b, c) = ((mags[k - 1] + 1e-12).ln(), (mags[k] + 1e-12).ln(), (mags[k + 1] + 1e-12).ln());
            let shift = (0.5 * (a - c) / (a - 2.0 * b + c).min(-1e-12)).clamp(-0.5, 0.5);
            ((k as f64 + shift) * bin_hz, mags[k] / max, k)
        })
        .collect();
    peaks.sort_by(|a, b| b.1.total_cmp(&a.1));
    let mut chosen: Vec<(f64, f64, usize)> = Vec::new();
    for peak in peaks {
        if chosen.len() >= count {
            break;
        }
        if chosen.iter().all(|c| (c.0 - peak.0).abs() > 15.0) {
            chosen.push(peak);
        }
    }
    // Each partial's level along the sound, frame by frame: the slope of its fall in dB is its decay.
    let frames: Vec<Frame> = (1..12).map(|t| frame_at(x, (start + n / 2 + t * hop) as i64, &window)).collect();
    chosen
        .into_iter()
        .map(|(hz, amp, k)| {
            let mut track = vec![20.0 * (mags[k] + 1e-12).log10()];
            for frame in &frames {
                let m = (k.saturating_sub(1)..=(k + 1).min(n / 2)).map(|b| frame.magnitude(b)).fold(0.0, f64::max);
                let db = 20.0 * (m + 1e-12).log10();
                if db < track[0] - 45.0 {
                    break;
                }
                track.push(db);
            }
            let t60 = if track.len() < 3 {
                0.08
            } else {
                let count = track.len() as f64;
                let seconds = hop as f64 / RATE;
                let mean_t = (count - 1.0) / 2.0;
                let mean_db = track.iter().sum::<f64>() / count;
                let (mut num, mut den) = (0.0, 0.0);
                for (i, db) in track.iter().enumerate() {
                    num += (i as f64 - mean_t) * (db - mean_db);
                    den += (i as f64 - mean_t).powi(2);
                }
                let slope = num / den / seconds;
                if slope >= -1.0 {
                    6.0
                } else {
                    (-60.0 / slope).clamp(0.05, 6.0)
                }
            };
            Partial { hz, amp, t60 }
        })
        .collect()
}

/// A sound's colour, read around its start: its share of energy under 150 Hz, its spectral centroid (Hz), how noisy it
/// is (spectral flatness, 0 tonal to 1 noise), and how long it lasts above -30 dB (seconds).
#[derive(Clone, Copy, Debug)]
pub struct Colour {
    pub low: f64,
    pub centroid: f64,
    pub flatness: f64,
    pub decay: f64,
}

pub fn colour(x: &[f32], from: usize) -> Colour {
    let n = 2048usize;
    let window = hann(n);
    let mut power = vec![0.0f64; n / 2 + 1];
    for t in 0..4 {
        let frame = frame_at(x, (from + n / 2 + t * n / 4) as i64, &window);
        for (k, p) in power.iter_mut().enumerate() {
            *p += frame.magnitude(k).powi(2);
        }
    }
    let bin_hz = RATE / n as f64;
    let total: f64 = power.iter().sum::<f64>().max(1e-18);
    let low = power.iter().enumerate().filter(|(k, _)| (*k as f64) * bin_hz < 150.0).map(|(_, p)| p).sum::<f64>() / total;
    let centroid = power.iter().enumerate().map(|(k, p)| k as f64 * bin_hz * p).sum::<f64>() / total;
    let band: Vec<f64> = power[2..n / 2].iter().map(|p| p + 1e-18).collect();
    let geometric = (band.iter().map(|p| p.ln()).sum::<f64>() / band.len() as f64).exp();
    let arithmetic = band.iter().sum::<f64>() / band.len() as f64;
    let env = envelope(x, ENV_HOP);
    let peak = env.iter().copied().fold(0.0f32, f32::max);
    let start = from / ENV_HOP;
    let last = env.iter().rposition(|v| *v >= peak * 0.0316).unwrap_or(start).max(start);
    Colour { low, centroid, flatness: (geometric / arithmetic).clamp(0.0, 1.0), decay: (last - start) as f64 * ENV_HOP as f64 / RATE }
}

/// Spectral centroid (Hz) of each `hop`-sample stretch: how a sound's brightness moves.
pub fn centroid_track(x: &[f32], hop: usize) -> Vec<f64> {
    let n = 2048usize;
    let window = hann(n);
    let bin_hz = RATE / n as f64;
    (0..x.len() / hop)
        .map(|t| {
            let frame = frame_at(x, (t * hop + hop / 2) as i64, &window);
            let (mut num, mut den) = (0.0, 0.0);
            for k in 0..=n / 2 {
                let p = frame.magnitude(k).powi(2);
                num += k as f64 * bin_hz * p;
                den += p;
            }
            if den > 0.0 {
                num / den
            } else {
                0.0
            }
        })
        .collect()
}

/// Reads `x` at a fractional position by cubic (Catmull–Rom) interpolation; outside reads silence.
pub fn read_at(x: &[f32], position: f64) -> f64 {
    let i = position.floor();
    let f = position - i;
    let i = i as i64;
    let at = |j: i64| if j >= 0 && (j as usize) < x.len() { x[j as usize] as f64 } else { 0.0 };
    let (p0, p1, p2, p3) = (at(i - 1), at(i), at(i + 1), at(i + 2));
    p1 + 0.5 * f * (p2 - p0 + f * (2.0 * p0 - 5.0 * p1 + 4.0 * p2 - p3 + f * (3.0 * (p1 - p2) + p3 - p0)))
}
