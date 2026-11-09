//! Grow: a sound part of the way toward another. Each moment of the two is heard as a spectrum, and the first's
//! energy moves along frequency toward where the second's lies (displacement interpolation, so partials glide rather
//! than crossfade) while its loudness moves from the first's to the second's. Phases carry on from each bin's
//! frequency, so the sound stays whole; at a sudden rise they start again from the two sounds' own. `at` 0 is the
//! first sound and 1 the second. The seed decides which parts of the spectrum arrive first, so one seed is one path
//! and another seed another.

use super::{
    analysis::{frame_at, hann, wrap, Frame, Stereo, Synth},
    rng::Rng,
    short, Ending, Options, Prepared, RATE,
};
use std::f64::consts::{PI, TAU};

const N: usize = 2048;
const HOP: usize = 512;

/// The path's character: how fast each part of the spectrum (by its share of the energy, low to high) gets there.
/// Part `q` has reached `at.powf(exponent(q))` of the way when the whole has reached `at`, so every part starts with
/// the first sound and ends with the second.
struct Path {
    waves: [(f64, f64); 3],
}

impl Path {
    fn new(mut rng: Rng) -> Self {
        Path { waves: std::array::from_fn(|m| (0.7 * rng.normal() / (m + 1) as f64, rng.range(0.0, TAU))) }
    }
    fn exponent(&self, q: f64) -> f64 {
        let wave: f64 = self.waves.iter().enumerate().map(|(m, (a, phase))| a * (PI * (m + 1) as f64 * q + phase).cos()).sum();
        2f64.powf(0.7 * wave.tanh())
    }
    /// Which part of the spectrum leads, for the recipe.
    fn leads(&self) -> &'static str {
        if self.exponent(0.2) < self.exponent(0.8) {
            "lows first"
        } else {
            "highs first"
        }
    }
}

/// One channel's running state: each source's last phases and the phases the output carries on.
struct Running {
    from: Vec<f64>,
    to: Vec<f64>,
    phase: Vec<f64>,
    started: bool,
    last_energy: f64,
}

impl Running {
    fn new(bins: usize) -> Self {
        Running { from: vec![0.0; bins], to: vec![0.0; bins], phase: vec![0.0; bins], started: false, last_energy: 0.0 }
    }
}

pub(super) fn grow(sources: &[Prepared], options: &Options, rng: Rng, progress: &mut dyn FnMut(f64)) -> (Stereo, String, Ending) {
    let at = options.at.unwrap_or(0.5).clamp(0.0, 1.0);
    let (from, to) = (&sources[0], &sources[1]);
    let path = Path::new(rng);
    let window = hann(N);
    let len = from.len().max(to.len());
    let frames = len / HOP + 2;
    let mut out: Stereo = [Vec::new(), Vec::new()];
    for (c, channel) in out.iter_mut().enumerate() {
        let mut synth = Synth::new(len, N);
        let mut running = Running::new(N / 2 + 1);
        for t in 0..frames {
            let center = t * HOP;
            let a = frame_at(&from.audio[c], center as i64, &window);
            let b = frame_at(&to.audio[c], center as i64, &window);
            synth.add(&morph(&a, &b, at, &path, &mut running), center);
            if t % 32 == 0 {
                progress(0.1 + 0.8 * (c * frames + t) as f64 / (2 * frames) as f64);
            }
        }
        *channel = synth.finish(len);
    }
    let recipe = format!("Grown {:.0}% of the way from {} toward {} ({})", at * 100.0, short(&from.name), short(&to.name), path.leads());
    (out, recipe, Ending::OneShot { fade: (0.03 * RATE) as usize })
}

/// Each bin's frequency (radians a sample) from how far its phase moved since the last frame; the frame's phases are
/// kept for the next.
fn frequencies(frame: &Frame, last: &mut [f64], started: bool) -> Vec<f64> {
    (0..last.len())
        .map(|k| {
            let omega = TAU * k as f64 / N as f64;
            let phase = frame.phase(k);
            let heard = if started { omega + wrap(phase - last[k] - omega * HOP as f64) / HOP as f64 } else { omega };
            last[k] = phase;
            heard
        })
        .collect()
}

/// One frame of the grown sound: the energy (`at`'s mix of the two frames') laid out along frequency by displacement
/// interpolation, and phases carried on from each bin's frequency.
fn morph(a: &Frame, b: &Frame, at: f64, path: &Path, running: &mut Running) -> Frame {
    let bins = N / 2 + 1;
    let power = |f: &Frame| -> Vec<f64> { (0..bins).map(|k| f.re[k] * f.re[k] + f.im[k] * f.im[k]).collect() };
    let (pa, pb) = (power(a), power(b));
    let (ea, eb) = (pa.iter().sum::<f64>(), pb.iter().sum::<f64>());
    let wa = frequencies(a, &mut running.from, running.started);
    let wb = frequencies(b, &mut running.to, running.started);
    let energy = (1.0 - at) * ea + at * eb;
    let mut out = Frame { re: vec![0.0; bins], im: vec![0.0; bins] };
    if energy <= 1e-24 {
        running.started = true;
        running.last_energy = 0.0;
        return out;
    }
    // Where the energy lies (shares summing to 1) and, weighted by it, the frequency each bin carries.
    let mut share = vec![0.0f64; bins];
    let mut carried = vec![0.0f64; bins];
    let only = |p: &[f64], e: f64, w: &[f64], share: &mut [f64], carried: &mut [f64]| {
        for k in 0..bins {
            share[k] = p[k] / e;
            carried[k] = w[k] * share[k];
        }
    };
    if at <= 0.0 || eb <= 1e-9 * ea {
        only(&pa, ea, &wa, &mut share, &mut carried);
    } else if at >= 1.0 || ea <= 1e-9 * eb {
        only(&pb, eb, &wb, &mut share, &mut carried);
    } else {
        // Walk both spectra's energy in step, low to high: each slice of it sits at bin i in the first and bin j in the
        // second, and lands between them, as far along as its part of the path has come.
        let (mut i, mut j) = (0usize, 0usize);
        let (mut left_a, mut left_b) = (pa[0] / ea, pb[0] / eb);
        let mut q = 0.0f64;
        while i < bins && j < bins {
            let slice = left_a.min(left_b);
            if slice > 0.0 {
                let reached = at.powf(path.exponent(q + slice / 2.0));
                let place = (1.0 - reached) * i as f64 + reached * j as f64;
                let heard = (1.0 - reached) * wa[i] + reached * wb[j];
                let low = (place.floor() as usize).min(bins - 1);
                let high = (low + 1).min(bins - 1);
                let part = place - low as f64;
                share[low] += slice * (1.0 - part);
                carried[low] += slice * (1.0 - part) * heard;
                share[high] += slice * part;
                carried[high] += slice * part * heard;
            }
            left_a -= slice;
            left_b -= slice;
            q += slice;
            if left_a <= 1e-15 {
                i += 1;
                if i < bins {
                    left_a += pa[i] / ea;
                }
            }
            if left_b <= 1e-15 {
                j += 1;
                if j < bins {
                    left_b += pb[j] / eb;
                }
            }
        }
    }
    // A sudden rise (6 dB) starts the phases again from the two frames' own, so a hit stays a hit.
    let restart = !running.started || energy > 4.0 * running.last_energy;
    for k in 0..bins {
        if restart {
            let (re, im) = ((1.0 - at) * a.re[k] + at * b.re[k], (1.0 - at) * a.im[k] + at * b.im[k]);
            running.phase[k] = if re == 0.0 && im == 0.0 { 0.0 } else { im.atan2(re) };
        } else {
            let heard = if share[k] > 0.0 { carried[k] / share[k] } else { TAU * k as f64 / N as f64 };
            running.phase[k] += heard * HOP as f64;
        }
        let magnitude = (energy * share[k]).sqrt();
        out.re[k] = magnitude * running.phase[k].cos();
        out.im[k] = if k > 0 && k < N / 2 { magnitude * running.phase[k].sin() } else { 0.0 };
    }
    running.started = true;
    running.last_energy = energy;
    out
}
