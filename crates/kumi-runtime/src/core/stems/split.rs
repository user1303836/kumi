//! Kumi's own splitter (#401: REAPER and Renoise have none), no model needed: harmonic–percussive separation by median
//! filtering (a spectrogram's peaks along time are what sustains, along frequency what hits), masks from the mid applied
//! to each channel, and the harmonic part cut at `BASS_HZ` into bass and the rest. Drums, bass and other add up to the
//! input; voices stay in other (that takes a model).
use crate::audio::dsp::{fft, window, Window};

/// The analysis frame and hop (samples): 2048 at 44.1–48 kHz is ~43 ms, a hop a quarter of it.
const SIZE: usize = 2048;
const HOP: usize = 512;
/// The median filters' lengths: frames along time (what sustains), bins along frequency (what hits).
const ALONG_TIME: usize = 17;
const ALONG_FREQUENCY: usize = 17;
/// Where bass ends (Hz).
pub const BASS_HZ: f64 = 150.;

/// A stereo stretch split: each stem's left and right.
#[derive(Debug, Clone, PartialEq)]
pub struct Split {
    pub drums: [Vec<f32>; 2],
    pub bass: [Vec<f32>; 2],
    pub other: [Vec<f32>; 2],
}

/// One channel's frames: each frame's spectrum (bins 0 to SIZE/2), real and imaginary.
struct Spectra {
    re: Vec<Vec<f32>>,
    im: Vec<Vec<f32>>,
}

fn analyse(samples: &[f32], frames: usize) -> Spectra {
    let hann = window(SIZE, Window::Hann);
    let bins = SIZE / 2 + 1;
    let (mut re_out, mut im_out) = (Vec::with_capacity(frames), Vec::with_capacity(frames));
    let (mut re, mut im) = (vec![0f64; SIZE], vec![0f64; SIZE]);
    for frame in 0..frames {
        let start = frame * HOP;
        for k in 0..SIZE {
            // (the window normalized to a sum of 1: scaled back so resynthesis adds up)
            re[k] = samples.get(start + k).copied().unwrap_or(0.) as f64 * hann[k] * SIZE as f64 / 2.;
            im[k] = 0.;
        }
        fft(&mut re, &mut im);
        re_out.push(re[..bins].iter().map(|v| *v as f32).collect());
        im_out.push(im[..bins].iter().map(|v| *v as f32).collect());
    }
    Spectra { re: re_out, im: im_out }
}

/// A channel back from its spectra masked by `mask(frame, bin)`, overlap-added with the same window, `length` samples.
fn synthesise(spectra: &Spectra, mask: impl Fn(usize, usize) -> f32, length: usize) -> Vec<f32> {
    let hann = window(SIZE, Window::Hann);
    let bins = SIZE / 2 + 1;
    let mut out = vec![0f64; length + SIZE];
    let mut norm = vec![0f64; length + SIZE];
    let (mut re, mut im) = (vec![0f64; SIZE], vec![0f64; SIZE]);
    for (frame, (frame_re, frame_im)) in spectra.re.iter().zip(&spectra.im).enumerate() {
        for k in 0..bins {
            let m = mask(frame, k) as f64;
            re[k] = frame_re[k] as f64 * m;
            // (the inverse by the forward transform of the conjugate)
            im[k] = -(frame_im[k] as f64 * m);
        }
        for k in bins..SIZE {
            re[k] = re[SIZE - k];
            im[k] = -im[SIZE - k];
        }
        fft(&mut re, &mut im);
        let start = frame * HOP;
        for k in 0..SIZE {
            let w = hann[k] * SIZE as f64 / 2.;
            out[start + k] += re[k] / SIZE as f64 * w;
            norm[start + k] += w * w;
        }
    }
    out.iter().zip(&norm).take(length).map(|(v, n)| if *n > 1e-9 { (*v / n) as f32 } else { 0. }).collect()
}

fn median(values: &mut [f32]) -> f32 {
    values.sort_by(f32::total_cmp);
    values[values.len() / 2]
}

/// Splits a stereo stretch (left, right, at `rate`). Its frames start `SIZE − HOP` before it and end as far past it (silence
/// there), so its first and last samples are covered by every frame a sample is, and nothing at its edges is divided by a
/// window's near-nothing.
pub fn split(left: &[f32], right: &[f32], rate: f64) -> Split {
    let length = left.len().min(right.len());
    let lead = SIZE - HOP;
    let padded = |samples: &[f32]| {
        let mut out = vec![0f32; lead];
        out.extend_from_slice(&samples[..length]);
        out.extend(std::iter::repeat_n(0f32, lead));
        out
    };
    let (left, right) = (padded(left), padded(right));
    let total = left.len();
    let frames = (total - SIZE).div_ceil(HOP) + 1;
    let (l, r) = (analyse(&left, frames), analyse(&right, frames));
    let bins = SIZE / 2 + 1;
    // (the mid's magnitudes, what the masks are read from)
    let magnitude: Vec<Vec<f32>> =
        (0..frames).map(|f| (0..bins).map(|k| ((l.re[f][k] + r.re[f][k]).hypot(l.im[f][k] + r.im[f][k])) / 2.).collect()).collect();
    let (half_t, half_f) = (ALONG_TIME / 2, ALONG_FREQUENCY / 2);
    let mut window_values = Vec::with_capacity(ALONG_TIME.max(ALONG_FREQUENCY));
    // (the share of each bin that sustains: what hits is the rest)
    let mut harmonic = vec![vec![0f32; bins]; frames];
    for f in 0..frames {
        for k in 0..bins {
            window_values.clear();
            window_values.extend((f.saturating_sub(half_t)..(f + half_t + 1).min(frames)).map(|g| magnitude[g][k]));
            let sustains = median(&mut window_values);
            window_values.clear();
            window_values.extend((k.saturating_sub(half_f)..(k + half_f + 1).min(bins)).map(|j| magnitude[f][j]));
            let hits = median(&mut window_values);
            let (h, p) = (sustains * sustains, hits * hits);
            let total = h + p;
            harmonic[f][k] = if total > 1e-12 { h / total } else { 0.5 };
        }
    }
    drop(magnitude);
    let cut = ((BASS_HZ / rate * SIZE as f64).round() as usize).min(bins);
    let harmonic = &harmonic;
    let stem = |mask: &dyn Fn(usize, usize) -> f32| {
        [&l, &r].map(|spectra| synthesise(spectra, mask, total).into_iter().skip(lead).take(length).collect::<Vec<f32>>())
    };
    Split {
        drums: stem(&|f, k| 1. - harmonic[f][k]),
        bass: stem(&|f, k| if k < cut { harmonic[f][k] } else { 0. }),
        other: stem(&|f, k| if k >= cut { harmonic[f][k] } else { 0. }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_bass_note_a_pad_and_clicks_split_into_bass_other_and_drums_that_add_up() {
        let rate = 44_100.;
        let n = (rate * 2.) as usize;
        // (a 55 Hz bass and a 440 Hz pad held, a click each quarter second)
        let mut mono = vec![0f32; n];
        for (k, value) in mono.iter_mut().enumerate() {
            let t = k as f64 / rate;
            *value = (0.3 * (2. * std::f64::consts::PI * 55. * t).sin() + 0.2 * (2. * std::f64::consts::PI * 440. * t).sin()) as f32;
        }
        for click in (2_000..n).step_by((rate / 4.) as usize) {
            for k in 0..64 {
                if let Some(value) = mono.get_mut(click + k) {
                    *value += 0.8 * (-(k as f32) / 8.).exp() * if k % 2 == 0 { 1. } else { -1. };
                }
            }
        }
        let split = split(&mono, &mono, rate);
        let energy = |samples: &[f32]| samples.iter().map(|v| (*v as f64).powi(2)).sum::<f64>();
        // (the stems add back up to the input, its edges too)
        let whole = 0..n;
        let sum: Vec<f32> = whole.clone().map(|k| split.drums[0][k] + split.bass[0][k] + split.other[0][k]).collect();
        let error: f64 = sum.iter().zip(&mono).map(|(a, b)| ((a - b) as f64).powi(2)).sum();
        assert!(error < energy(&mono) * 1e-3, "{error}");
        // (no stem clicks at the edges: none peaks past the input's peak there)
        let peak = |samples: &[f32]| samples.iter().fold(0f32, |most, v| most.max(v.abs()));
        let edge = peak(&mono[..256]).max(peak(&mono[n - 256..]));
        for stem in [&split.drums[0], &split.bass[0], &split.other[0]] {
            assert!(peak(&stem[..256]) <= edge * 1.5 && peak(&stem[n - 256..]) <= edge * 1.5, "{} {}", peak(&stem[..256]), edge);
        }
        let inner = 4096..n - 4096;
        // (the bass in bass, the pad in other, the clicks in drums: each stem holds most of its part's energy)
        let tone = |samples: &[f32], hz: f64| {
            let (mut re, mut im) = (0f64, 0f64);
            for (k, v) in samples.iter().enumerate() {
                let phase = 2. * std::f64::consts::PI * hz * k as f64 / rate;
                re += *v as f64 * phase.cos();
                im += *v as f64 * phase.sin();
            }
            re.hypot(im)
        };
        assert!(tone(&split.bass[0][inner.clone()], 55.) > 5. * tone(&split.other[0][inner.clone()], 55.));
        assert!(tone(&split.other[0][inner.clone()], 440.) > 5. * tone(&split.bass[0][inner.clone()], 440.));
        assert!(tone(&split.other[0][inner.clone()], 440.) > 3. * tone(&split.drums[0][inner.clone()], 440.));
        let click = 2_000 + (rate / 4.) as usize * 2;
        let around = click..click + 64;
        assert!(
            energy(&split.drums[0][around.clone()]) > energy(&split.other[0][around.clone()]) + energy(&split.bass[0][around]),
            "the clicks in drums"
        );
    }
}
