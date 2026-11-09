//! The processors the designs are built from: filters, an envelope follower, convolution, a diffuse reverb, fades.

use super::{analysis::Stereo, rng::Rng, RATE};
use kumi_media::audio::dsp::{fft, Biquad};
use std::f64::consts::PI;

/// A state-variable filter (topology-preserving), whose cutoff may move every sample.
#[derive(Clone, Default)]
pub struct Svf {
    ic1: f64,
    ic2: f64,
}
pub struct SvfOut {
    pub low: f64,
    pub band: f64,
    pub high: f64,
}
impl Svf {
    pub fn process(&mut self, x: f64, hz: f64, q: f64) -> SvfOut {
        let g = (PI * hz.clamp(10.0, RATE * 0.45) / RATE).tan();
        let k = 1.0 / q.max(0.3);
        let a1 = 1.0 / (1.0 + g * (g + k));
        let a2 = g * a1;
        let a3 = g * a2;
        let v3 = x - self.ic2;
        let v1 = a1 * self.ic1 + a2 * v3;
        let v2 = self.ic2 + a2 * self.ic1 + a3 * v3;
        self.ic1 = 2.0 * v1 - self.ic1;
        self.ic2 = 2.0 * v2 - self.ic2;
        SvfOut { low: v2, band: v1, high: x - k * v1 - v2 }
    }
}

/// A peaking equalizer (RBJ) at `hz`, `db` up or down.
pub fn peaking(hz: f64, db: f64, q: f64) -> Biquad {
    let a = 10f64.powf(db / 40.0);
    let w = 2.0 * PI * hz / RATE;
    let alpha = w.sin() / (2.0 * q);
    let a0 = 1.0 + alpha / a;
    Biquad::new((1.0 + alpha * a) / a0, -2.0 * w.cos() / a0, (1.0 - alpha * a) / a0, -2.0 * w.cos() / a0, (1.0 - alpha / a) / a0)
}

/// Runs both channels through a fresh copy of the filter.
pub fn filter(s: &mut Stereo, make: impl Fn() -> Biquad) {
    for channel in s.iter_mut() {
        let mut f = make();
        for v in channel.iter_mut() {
            *v = f.process(*v as f64) as f32;
        }
    }
}

/// The level of `x` as an envelope follower hears it: rising in `attack` seconds, falling in `release`.
pub fn follow(x: &[f32], attack: f64, release: f64) -> Vec<f32> {
    let (up, down) = ((-1.0 / (attack * RATE)).exp(), (-1.0 / (release * RATE)).exp());
    let mut level = 0.0f64;
    x.iter()
        .map(|v| {
            let v = v.abs() as f64;
            let c = if v > level { up } else { down };
            level = v + c * (level - v);
            level as f32
        })
        .collect()
}

/// `x` convolved with `h`, by one FFT.
pub fn convolve(x: &[f32], h: &[f32]) -> Vec<f32> {
    if x.is_empty() || h.is_empty() {
        return Vec::new();
    }
    let len = x.len() + h.len() - 1;
    let n = len.next_power_of_two();
    let (mut xr, mut xi) = (vec![0.0; n], vec![0.0; n]);
    let (mut hr, mut hi) = (vec![0.0; n], vec![0.0; n]);
    for (i, v) in x.iter().enumerate() {
        xr[i] = *v as f64;
    }
    for (i, v) in h.iter().enumerate() {
        hr[i] = *v as f64;
    }
    fft(&mut xr, &mut xi);
    fft(&mut hr, &mut hi);
    // The product, conjugated so the forward transform runs it backwards.
    for k in 0..n {
        let (re, im) = (xr[k] * hr[k] - xi[k] * hi[k], xr[k] * hi[k] + xi[k] * hr[k]);
        xr[k] = re;
        xi[k] = -im;
    }
    fft(&mut xr, &mut xi);
    xr.truncate(len);
    xr.into_iter().map(|v| (v / n as f64) as f32).collect()
}

/// A diffuse stereo reverb: four allpass diffusers into an eight-line feedback delay network with damping, its size and
/// decay (`t60`, seconds) as asked, its lines' lengths drawn from the seed.
pub fn reverb(input: &Stereo, t60: f64, size: f64, damping: f64, rng: &mut Rng, tail: usize) -> Stereo {
    let len = input[0].len() + tail;
    let base = [1109.0, 1277.0, 1493.0, 1621.0, 1867.0, 2053.0, 2311.0, 2539.0];
    let lengths: Vec<usize> = base.iter().map(|b| (b * size * rng.range(0.9, 1.1)) as usize).collect();
    let gains: Vec<f64> = lengths.iter().map(|l| 10f64.powf(-3.0 * *l as f64 / (t60 * RATE))).collect();
    let mut lines: Vec<Vec<f64>> = lengths.iter().map(|l| vec![0.0; *l]).collect();
    let mut at = vec![0usize; 8];
    let mut damp = vec![0.0f64; 8];
    let diffusers = [142usize, 107, 379, 277].map(|d| (d as f64 * size.sqrt()) as usize);
    let mut allpass: Vec<(Vec<f64>, usize)> = diffusers.iter().map(|d| (vec![0.0; (*d).max(1)], 0)).collect();
    let mut out = [vec![0.0f32; len], vec![0.0f32; len]];
    for n in 0..len {
        let dry = 0.5 * (input[0].get(n).copied().unwrap_or(0.0) as f64 + input[1].get(n).copied().unwrap_or(0.0) as f64);
        let mut x = dry;
        for (buffer, i) in allpass.iter_mut() {
            let delayed = buffer[*i];
            let y = -0.6 * x + delayed;
            buffer[*i] = x + 0.6 * y;
            *i = (*i + 1) % buffer.len();
            x = y;
        }
        let taps: Vec<f64> = (0..8).map(|j| lines[j][at[j]]).collect();
        // Householder mix: each line hears all the others.
        let sum: f64 = taps.iter().sum::<f64>() * 0.25;
        for j in 0..8 {
            let fed = (taps[j] - sum) * gains[j];
            damp[j] = fed + damping * (damp[j] - fed);
            lines[j][at[j]] = damp[j] + x * if j % 2 == 0 { 0.5 } else { -0.5 };
            at[j] = (at[j] + 1) % lines[j].len();
        }
        out[0][n] = (taps[0] + taps[2] + taps[4] + taps[6]) as f32 * 0.35;
        out[1][n] = (taps[1] + taps[3] + taps[5] + taps[7]) as f32 * 0.35;
    }
    out
}

/// A raised-cosine fade over the first `n` samples.
pub fn fade_in(s: &mut Stereo, n: usize) {
    let n = n.min(s[0].len());
    for channel in s.iter_mut() {
        for (i, v) in channel.iter_mut().take(n).enumerate() {
            *v *= (0.5 - 0.5 * (PI * i as f64 / n as f64).cos()) as f32;
        }
    }
}

/// A raised-cosine fade over the last `n` samples.
pub fn fade_out(s: &mut Stereo, n: usize) {
    let len = s[0].len();
    let n = n.min(len);
    for channel in s.iter_mut() {
        for i in 0..n {
            channel[len - 1 - i] *= (0.5 - 0.5 * (PI * i as f64 / n as f64).cos()) as f32;
        }
    }
}

/// A silent stereo buffer.
pub fn silence(len: usize) -> Stereo {
    [vec![0.0; len], vec![0.0; len]]
}

/// Adds `from` into `into` at `offset`, scaled by `gain` (left and right gains apart).
pub fn mix_into(into: &mut Stereo, from: &Stereo, offset: usize, gain: [f64; 2]) {
    for c in 0..2 {
        for (i, v) in from[c].iter().enumerate() {
            if let Some(slot) = into[c].get_mut(offset + i) {
                *slot += (*v as f64 * gain[c]) as f32;
            }
        }
    }
}

/// Folds what runs past `len` back onto the start, so the buffer loops without a seam.
pub fn wrap_tail(s: &mut Stereo, len: usize) {
    for channel in s.iter_mut() {
        if channel.len() > len {
            let tail: Vec<f32> = channel.split_off(len);
            for (i, v) in tail.into_iter().enumerate() {
                channel[i % len] += v;
            }
        } else {
            channel.resize(len, 0.0);
        }
    }
}

/// Makes a buffer of `len + fade` samples loop cleanly at `len`: its last `fade` samples crossfade (equal power) into
/// its first, so the end flows into the start as it would have flowed on.
pub fn loop_crossfade(s: &mut Stereo, len: usize, fade: usize) {
    for channel in s.iter_mut() {
        let fade = fade.min(channel.len().saturating_sub(len)).min(len);
        for i in 0..fade {
            let t = i as f64 / fade as f64;
            let (gain_in, gain_out) = ((t * PI / 2.0).sin(), (t * PI / 2.0).cos());
            channel[i] = (channel[i] as f64 * gain_in + channel[len + i] as f64 * gain_out) as f32;
        }
        channel.truncate(len);
        channel.resize(len, 0.0);
    }
}

/// Equal-power pan gains for `pan` in -1 (left) to 1 (right).
pub fn pan(pan: f64) -> [f64; 2] {
    let angle = (pan.clamp(-1.0, 1.0) + 1.0) * PI / 4.0;
    [angle.cos() * std::f64::consts::SQRT_2, angle.sin() * std::f64::consts::SQRT_2]
}

/// The RMS of a stretch of both channels.
pub fn rms(s: &Stereo, from: usize, to: usize) -> f64 {
    let to = to.min(s[0].len());
    if to <= from {
        return 0.0;
    }
    let sum: f64 = s.iter().map(|c| c[from..to].iter().map(|v| (*v as f64).powi(2)).sum::<f64>()).sum();
    (sum / (2 * (to - from)) as f64).sqrt()
}

/// Both channels scaled by `gain`.
pub fn scale(s: &mut Stereo, gain: f64) {
    for channel in s.iter_mut() {
        for v in channel.iter_mut() {
            *v = (*v as f64 * gain) as f32;
        }
    }
}
