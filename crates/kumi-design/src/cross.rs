//! Cross: two to sixteen sounds into one. **Blend** morphs their spectra into one body; **attack** puts one's transient
//! on the blend of the rest; **resonate** rings half of them through the other half; **modulate** has them modulate each
//! other (phase, ring, FM, envelope-driven filter and gain, spectral cross-synthesis).

use super::{
    analysis::{self, envelope, frame_at, hann, read_at, wrap, Frame, Stereo, Synth},
    fx,
    rng::Rng,
    shares, short, Ending, Options, Prepared, RATE,
};
use kumi_media::audio::dsp::high_pass;
use std::f64::consts::TAU;

const N: usize = 2048;
const HOP: usize = 512;
/// The lag the instantaneous frequency is read over: short enough to read any bin's unambiguously.
const LAG: usize = 128;
/// The stretch at the start every source keeps at its own speed, so attacks aren't stretched.
const ATTACK: f64 = 0.06;

/// A sound to blend: its audio and its mono mix.
pub(super) struct Layer<'a> {
    pub audio: &'a Stereo,
    pub mono: &'a [f32],
}

/// Log-magnitudes smoothed across frequency, about a fifth of an octave wide: a spectrum's envelope without its partials.
fn smooth(log: &[f64]) -> Vec<f64> {
    let mut sums = vec![0.0; log.len() + 1];
    for (k, v) in log.iter().enumerate() {
        sums[k + 1] = sums[k] + v;
    }
    (0..log.len())
        .map(|k| {
            let w = ((k as f64 * 0.06).round() as usize).max(2);
            let (from, to) = (k.saturating_sub(w), (k + w + 1).min(log.len()));
            (sums[to] - sums[from]) / (to - from) as f64
        })
        .collect()
}

/// The spectral morph at the heart of blend: the sources, aligned on their attacks and time-normalized to `out_len`
/// (their first 60 ms at their own speed), morphed frame by frame. Each frame's spectral envelope is the weighted
/// geometric mean of the sources' (so brightness and formants move between them, not just add), its fine structure the
/// weighted power mean of theirs (so every source's partials and noise are there), its level their weighted sum. Phases
/// come from the source loudest in each bin: copied through the attack, then advanced by a phase vocoder locked to the
/// spectral peaks.
pub(super) fn blend_core(layers: &[Layer], weights: &[f64], out_len: usize, fit: f64, progress: &mut dyn FnMut(f64)) -> Stereo {
    let window = hann(N);
    let bins = N / 2 + 1;
    let attack = (ATTACK * RATE) as f64;
    let envs: Vec<(Vec<f32>, f32)> = layers
        .iter()
        .map(|l| {
            let env = envelope(l.mono, 64);
            let peak = env.iter().copied().fold(0.0f32, f32::max).max(1e-9);
            (env, peak)
        })
        .collect();
    let rates: Vec<f64> = layers
        .iter()
        .map(|l| ((l.mono.len() as f64 - attack).max(1.0) / (out_len as f64 - attack).max(1.0)).powf(fit).clamp(0.2, 5.0))
        .collect();
    let mut synth = [Synth::new(out_len, N), Synth::new(out_len, N)];
    let mut previous = [vec![0.0f64; bins], vec![0.0f64; bins]];
    let frames = out_len / HOP + 2;
    for t in 0..frames {
        let center = (t * HOP) as f64;
        let positions: Vec<f64> = rates.iter().map(|r| if center < attack { center } else { attack + (center - attack) * r }).collect();
        let presence: Vec<f64> =
            positions.iter().zip(&envs).map(|(p, (env, peak))| env.get(*p as usize / 64).map_or(0.0, |v| (*v / *peak) as f64)).collect();
        let mut shape: Vec<f64> = weights.iter().zip(&presence).map(|(w, a)| w * a.sqrt()).collect();
        let total: f64 = shape.iter().sum();
        if total <= 1e-9 {
            continue;
        }
        shape.iter_mut().for_each(|v| *v /= total);
        let spectra: Vec<[Frame; 2]> = layers
            .iter()
            .zip(&positions)
            .map(|(l, p)| [frame_at(&l.audio[0], *p as i64, &window), frame_at(&l.audio[1], *p as i64, &window)])
            .collect();
        // Each bin's loudest source (by its share), which gives the bin its phase.
        let dominant: Vec<usize> = (0..bins)
            .map(|k| {
                (0..layers.len())
                    .max_by(|a, b| {
                        let m = |i: usize| shape[i] * (spectra[i][0].magnitude(k) + spectra[i][1].magnitude(k));
                        m(*a).total_cmp(&m(*b))
                    })
                    .unwrap_or(0)
            })
            .collect();
        let mut magnitudes = [vec![0.0f64; bins], vec![0.0f64; bins]];
        for c in 0..2 {
            let mut envelope_sum = vec![0.0f64; bins];
            let mut fine = vec![0.0f64; bins];
            let mut level = 0.0;
            for (i, spectrum) in spectra.iter().enumerate() {
                let log: Vec<f64> = (0..bins).map(|k| (spectrum[c].magnitude(k) + 1e-9).ln()).collect();
                let smoothed = smooth(&log);
                let mean = smoothed.iter().sum::<f64>() / bins as f64;
                level += weights[i] * mean.exp();
                for k in 0..bins {
                    envelope_sum[k] += shape[i] * (smoothed[k] - mean);
                    fine[k] += shape[i] * (2.0 * (log[k] - smoothed[k])).exp();
                }
            }
            let level = level.max(1e-12).ln();
            for k in 0..bins {
                magnitudes[c][k] = (envelope_sum[k] + level).exp() * fine[k].sqrt();
            }
        }
        // Peaks of both channels together, so left and right share their phase regions.
        let both: Vec<f64> = (0..bins).map(|k| magnitudes[0][k] + magnitudes[1][k]).collect();
        let peaks: Vec<usize> =
            (0..bins).filter(|&k| (k.saturating_sub(2)..(k + 3).min(bins)).all(|j| both[j] <= both[k]) && both[k] > 0.0).collect();
        let in_attack = center < attack;
        // The instantaneous frequency of a peak's dominant source, from its mono spectrum now and LAG samples before.
        let mut frequency = vec![0.0f64; bins];
        if !in_attack {
            let mut lagged: Vec<Option<Frame>> = vec![None; layers.len()];
            for &k in &peaks {
                let i = dominant[k];
                let lag = lagged[i].get_or_insert_with(|| frame_at(layers[i].mono, positions[i] as i64 - LAG as i64, &window));
                let now = (spectra[i][0].im[k] + spectra[i][1].im[k]).atan2(spectra[i][0].re[k] + spectra[i][1].re[k]);
                let expected = TAU * k as f64 * LAG as f64 / N as f64;
                frequency[k] = TAU * k as f64 / N as f64 + wrap(now - lag.phase(k) - expected) / LAG as f64;
            }
        }
        for c in 0..2 {
            let mut phase = vec![0.0f64; bins];
            if in_attack || t == 0 {
                for k in 0..bins {
                    phase[k] = spectra[dominant[k]][c].phase(k);
                }
            } else {
                // Identity phase locking: a peak's phase advances by its frequency; the bins around it keep their
                // source's phase relation to it.
                let mut region_start = 0usize;
                for (j, &p) in peaks.iter().enumerate() {
                    let region_end = peaks.get(j + 1).map_or(bins, |next| (p + next) / 2 + 1);
                    let advanced = previous[c][p] + frequency[p] * HOP as f64;
                    let source = &spectra[dominant[p]][c];
                    let reference = source.phase(p);
                    for k in region_start..region_end {
                        phase[k] = advanced + (source.phase(k) - reference);
                    }
                    region_start = region_end;
                }
            }
            let frame = Frame {
                re: (0..bins).map(|k| magnitudes[c][k] * phase[k].cos()).collect(),
                im: (0..bins).map(|k| magnitudes[c][k] * phase[k].sin()).collect(),
            };
            synth[c].add(&frame, t * HOP);
            previous[c] = phase.into_iter().map(wrap).collect();
        }
        if t % 16 == 0 {
            progress(0.1 + 0.8 * t as f64 / frames as f64);
        }
    }
    let [left, right] = synth;
    [left.finish(out_len), right.finish(out_len)]
}

fn names(sources: &[&Prepared]) -> Vec<String> {
    sources.iter().map(|s| s.name.clone()).collect()
}

/// Seeded weights around even, none too small to hear.
fn weights(rng: &mut Rng, n: usize) -> Vec<f64> {
    rng.dirichlet(n, 1.5).into_iter().map(|w| 0.8 * w + 0.2 / n as f64).collect()
}

pub(super) fn blend(sources: &[Prepared], options: &Options, mut rng: Rng, progress: &mut dyn FnMut(f64)) -> (Stereo, String, Ending) {
    let w = weights(&mut rng, sources.len());
    let fit = rng.range(0.6, 1.0);
    let natural: f64 = sources.iter().zip(&w).map(|(s, w)| s.len() as f64 * w).sum();
    let out_len = options.seconds.map_or(natural, |s| s * RATE).clamp(0.15 * RATE, super::MAX_SECONDS * RATE) as usize;
    let layers: Vec<Layer> = sources.iter().map(|s| Layer { audio: &s.audio, mono: &s.mono }).collect();
    let audio = blend_core(&layers, &w, out_len, fit, progress);
    let refs: Vec<&Prepared> = sources.iter().collect();
    let names = names(&refs);
    let names: Vec<&str> = names.iter().map(String::as_str).collect();
    let recipe = format!("Blend of {}: {}; aligned on the attack", sources.len(), shares(&names, &w));
    (audio, recipe, Ending::OneShot { fade: (0.03 * RATE) as usize })
}

/// How sharp a sound's attack is: its loudest early moment against its level just after.
fn sharpness(source: &Prepared) -> f64 {
    let env = envelope(&source.mono, 64);
    let early = (0.02 * RATE / 64.0) as usize;
    let after = (0.2 * RATE / 64.0) as usize;
    let peak = env.iter().take(early.max(1)).copied().fold(0.0f32, f32::max) as f64;
    let rest: Vec<f32> = env.iter().skip(early).take(after).copied().collect();
    let mean = if rest.is_empty() { 1e-6 } else { rest.iter().sum::<f32>() as f64 / rest.len() as f64 };
    peak / mean.max(1e-6)
}

pub(super) fn attack(sources: &[Prepared], options: &Options, mut rng: Rng, progress: &mut dyn FnMut(f64)) -> (Stereo, String, Ending) {
    let from = options.attack_from.unwrap_or_else(|| {
        let sharp: Vec<f64> = sources.iter().map(|s| sharpness(s).powi(2)).collect();
        rng.weighted(&sharp)
    });
    let giver = &sources[from];
    let others: Vec<&Prepared> = sources.iter().enumerate().filter(|(i, _)| *i != from).map(|(_, s)| s).collect();
    // The transient: from the start to 10-40 ms past its peak, cut where its level dips.
    let peak = giver.peak;
    let mut end = (peak + (rng.range(0.01, 0.04) * RATE) as usize).min(giver.len());
    let env = envelope(&giver.mono, 32);
    let (lo, hi) = (end.saturating_sub(240) / 32, ((end + 240) / 32).min(env.len().saturating_sub(1)));
    if let Some(quiet) = (lo..=hi).min_by(|a, b| env[*a].total_cmp(&env[*b])) {
        end = (quiet * 32).max(peak + (0.008 * RATE) as usize);
    }
    let end = end.min((0.08 * RATE) as usize).min(giver.len());
    let fade = (rng.range(0.004, 0.015) * RATE) as usize;
    let join = end.saturating_sub(fade / 2);
    let mut transient: Stereo =
        [giver.audio[0][..(join + fade).min(giver.len())].to_vec(), giver.audio[1][..(join + fade).min(giver.len())].to_vec()];
    let highpass = rng.chance(0.5).then(|| rng.range(80.0, 220.0));
    if let Some(hz) = highpass {
        fx::filter(&mut transient, || high_pass(RATE, hz));
    }
    // The body: the others from where their own transients end (mostly), blended, entering under the attack's tail.
    let skip_theirs = rng.chance(0.7);
    let cut: Vec<(Stereo, Vec<f32>)> = others
        .iter()
        .map(|s| {
            let skip = if skip_theirs { (s.peak + (0.01 * RATE) as usize).min(s.len().saturating_sub(1)) } else { 0 };
            ([s.audio[0][skip..].to_vec(), s.audio[1][skip..].to_vec()], s.mono[skip..].to_vec())
        })
        .collect();
    let w = weights(&mut rng, cut.len());
    let natural: f64 = cut.iter().zip(&w).map(|(c, w)| c.1.len() as f64 * w).sum::<f64>().max(0.1 * RATE);
    let total = options.seconds.map_or(join as f64 + natural, |s| s * RATE).clamp(0.1 * RATE, super::MAX_SECONDS * RATE) as usize;
    let body_len = total.saturating_sub(join).max(fade * 2);
    let mut body = if cut.len() == 1 {
        let mut b = cut[0].0.clone();
        for c in b.iter_mut() {
            c.resize(body_len, 0.0);
        }
        b
    } else {
        let layers: Vec<Layer> = cut.iter().map(|(a, m)| Layer { audio: a, mono: m }).collect();
        blend_core(&layers, &w, body_len, rng.range(0.6, 1.0), progress)
    };
    fx::fade_in(&mut body, fade);
    // Level the body to the attack where they meet.
    let attack_level = fx::rms(&transient, join.saturating_sub((0.005 * RATE) as usize), join);
    let body_level = fx::rms(&body, fade, fade + (0.02 * RATE) as usize).max(1e-9);
    let ratio = rng.range(0.6, 1.1);
    let gain = (attack_level * ratio / body_level).clamp(10f64.powf(-18.0 / 20.0), 10f64.powf(18.0 / 20.0));
    fx::scale(&mut body, gain);
    // Equal-power crossfade from the attack into the body.
    let len = transient[0].len();
    for c in 0..2 {
        for i in 0..fade.min(len.saturating_sub(join)) {
            let t = i as f64 / fade as f64;
            transient[c][join + i] = (transient[c][join + i] as f64 * (t * std::f64::consts::FRAC_PI_2).cos()) as f32;
        }
    }
    let mut out = fx::silence(join + body_len);
    fx::mix_into(&mut out, &transient, 0, [1.0, 1.0]);
    fx::mix_into(&mut out, &body, join, [1.0, 1.0]);
    let other_names = names(&others);
    let other_names: Vec<&str> = other_names.iter().map(String::as_str).collect();
    let body_words =
        if others.len() == 1 { short(other_names[0]) } else { format!("a blend of {}: {}", others.len(), shares(&other_names, &w)) };
    let recipe = format!(
        "Attack of {} ({} ms{}) onto {}; {} ms crossfade",
        short(&giver.name),
        (end as f64 / RATE * 1000.0).round(),
        highpass.map_or(String::new(), |hz| format!(", high-passed at {} Hz", hz.round())),
        body_words,
        (fade as f64 / RATE * 1000.0).round()
    );
    (out, recipe, Ending::OneShot { fade: (0.03 * RATE) as usize })
}

/// A bank of two-pole resonators tuned to a sound's partials: what's played through it rings as that sound would.
fn modal(exciter: &Stereo, partials: &[analysis::Partial], decay: f64, rng: &mut Rng, len: usize) -> Stereo {
    let mut out = fx::silence(len);
    for (c, channel) in out.iter_mut().enumerate() {
        for partial in partials {
            let detune = if c == 1 { 1.0 + rng.range(-0.002, 0.002) } else { 1.0 };
            let w = TAU * (partial.hz * detune).min(RATE * 0.45) / RATE;
            let t60 = (partial.t60 * decay).clamp(0.03, 8.0);
            let r = 10f64.powf(-3.0 / (t60 * RATE));
            let (a1, a2) = (2.0 * r * w.cos(), -r * r);
            let gain = (1.0 - r * r) * partial.amp;
            let (mut y1, mut y2) = (0.0f64, 0.0f64);
            for (n, slot) in channel.iter_mut().enumerate() {
                let x = exciter[c].get(n).copied().unwrap_or(0.0) as f64;
                let y = gain * x + a1 * y1 + a2 * y2;
                y2 = y1;
                y1 = y;
                *slot += y as f32;
            }
        }
    }
    out
}

pub(super) fn resonate(sources: &[Prepared], options: &Options, mut rng: Rng) -> (Stereo, String, Ending) {
    let n = sources.len();
    let exciters: Vec<usize> = match &options.split {
        Some(split) => split.clone(),
        None => {
            let order = rng.shuffled(n);
            let half = if n % 2 == 1 && rng.chance(0.5) { n / 2 + 1 } else { n / 2 };
            let mut e = order[..half.max(1)].to_vec();
            e.sort_unstable();
            e
        }
    };
    let resonators: Vec<usize> = (0..n).filter(|i| !exciters.contains(i)).collect();
    let most = options.seconds.map_or(15.0, |s| s.min(15.0));
    let exciter_len = exciters.iter().map(|&i| sources[i].len()).max().unwrap_or(0).min((most * RATE) as usize);
    let mut exciter = fx::silence(exciter_len);
    for &i in &exciters {
        let offset = if exciters.len() > 1 { (rng.range(0.0, 0.02) * RATE) as usize } else { 0 };
        let source = [sources[i].audio[0].clone(), sources[i].audio[1].clone()];
        fx::mix_into(&mut exciter, &source, offset, [1.0, 1.0]);
    }
    let decay = rng.range(0.6, 2.4) * (0.5 + options.depth.unwrap_or(0.5));
    let wet = options.depth.map_or_else(|| rng.range(0.6, 0.92), |d| 0.3 + 0.7 * d);
    let tail =
        resonators.iter().map(|&i| sources[i].len().min((2.0 * RATE) as usize)).max().unwrap_or(0).max((decay.min(3.0) * RATE) as usize);
    let len = options.seconds.map_or(exciter_len + tail, |s| (s * RATE) as usize).min((super::MAX_SECONDS * RATE) as usize);
    let exciter_level = fx::rms(&exciter, 0, exciter_len).max(1e-9);
    let mut wet_sum = fx::silence(len);
    let mut words = Vec::new();
    for &i in &resonators {
        let resonator = &sources[i];
        let kind = rng.below(3);
        let mut ringing = fx::silence(len);
        let mut said = Vec::new();
        if kind != 1 {
            let count = 8 + rng.below(25);
            let partials = analysis::partials(&resonator.mono, resonator.peak, count);
            if !partials.is_empty() {
                let mut m = modal(&exciter, &partials, decay, &mut rng, len);
                let level = fx::rms(&m, 0, len).max(1e-12);
                fx::scale(&mut m, exciter_level / level);
                fx::mix_into(&mut ringing, &m, 0, [1.0, 1.0]);
                said.push(format!("{} partials, decay ×{:.1}", partials.len(), decay));
            }
        }
        if kind != 0 || said.is_empty() {
            // Its own sound as a body: trimmed, faded, its energy one.
            let ir_len = resonator.len().min((rng.range(0.3, 2.0) * RATE) as usize).max(64);
            let mut ir: Stereo = [resonator.audio[0][..ir_len].to_vec(), resonator.audio[1][..ir_len].to_vec()];
            fx::fade_out(&mut ir, ir_len * 3 / 10);
            let energy = fx::rms(&ir, 0, ir_len) * (ir_len as f64).sqrt();
            fx::scale(&mut ir, 1.0 / energy.max(1e-9));
            let mut driven = exciter.clone();
            fx::filter(&mut driven, || high_pass(RATE, 60.0));
            let mono = analysis::mono(&driven);
            let mut b: Stereo = [fx_conv(&mono, &ir[0], len), fx_conv(&mono, &ir[1], len)];
            let level = fx::rms(&b, 0, len).max(1e-12);
            fx::scale(&mut b, exciter_level / level);
            fx::mix_into(&mut ringing, &b, 0, [1.0, 1.0]);
            said.push("body".into());
        }
        let level = fx::rms(&ringing, 0, len).max(1e-12);
        fx::scale(&mut ringing, exciter_level / level / (resonators.len() as f64).sqrt());
        fx::mix_into(&mut wet_sum, &ringing, 0, [1.0, 1.0]);
        words.push(format!("{} ({})", short(&resonator.name), said.join(" and ")));
    }
    let mut out = fx::silence(len);
    fx::mix_into(&mut out, &exciter, 0, [1.0 - wet, 1.0 - wet]);
    fx::mix_into(&mut out, &wet_sum, 0, [wet, wet]);
    let exciter_names: Vec<String> = exciters.iter().map(|&i| short(&sources[i].name)).collect();
    let recipe = format!("{} through {}; {}% wet", exciter_names.join(" + "), words.join(" and "), (wet * 100.0).round());
    (out, recipe, Ending::OneShot { fade: (0.08 * RATE) as usize })
}

fn fx_conv(x: &[f32], h: &[f32], len: usize) -> Vec<f32> {
    let mut y = fx::convolve(x, h);
    y.resize(len, 0.0);
    y
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Stage {
    Phase,
    Ring,
    Fm,
    Filter,
    Gain,
    Vocode,
}

/// A modulator made ready: its mono signal at peak 1, and how present it is at each moment (0 to 1). One much shorter
/// than the carrier may retrigger along it (`every`, samples), so it modulates the whole sound, not just its start.
struct Modulator {
    signal: Vec<f32>,
    presence: Vec<f32>,
    name: String,
}

fn modulator(source: &Prepared, len: usize, every: Option<usize>) -> Modulator {
    let peak = source.mono.iter().fold(0.0f32, |m, v| m.max(v.abs())).max(1e-9);
    let one: Vec<f32> = source.mono.iter().map(|v| v / peak).collect();
    let mut signal = vec![0.0f32; len];
    let every = every.unwrap_or(len.max(1)).max(1);
    let mut at = 0;
    while at < len {
        for (slot, v) in signal[at..].iter_mut().zip(&one) {
            *slot += v;
        }
        at += every;
    }
    let level = fx::follow(&signal, 0.005, 0.12);
    let top = level.iter().copied().fold(0.0f32, f32::max).max(1e-9);
    let presence = level.iter().map(|v| (v / top * 3.0).min(1.0)).collect();
    Modulator { signal, presence, name: short(&source.name) }
}

/// Matches `y`'s level to `x`'s where the modulator is there (`presence`), within 24 dB: a stage that silences the
/// carrier wherever its modulator is quiet isn't lifted to make up for the silence.
fn level_to(y: &mut Stereo, x: &Stereo, presence: &[f32]) {
    let power =
        |s: &Stereo| -> f64 { s.iter().map(|c| c.iter().zip(presence).map(|(v, p)| (*v as f64).powi(2) * *p as f64).sum::<f64>()).sum() };
    let gain = (power(x) / power(y).max(1e-18)).sqrt().clamp(1.0 / 16.0, 16.0);
    fx::scale(y, gain);
}

/// Spectral cross-synthesis: `envelope_of`'s spectral envelope on `x`'s fine structure, with `x`'s phases.
fn vocode(x: &Stereo, envelope_of: &[f32]) -> Stereo {
    let window = hann(N);
    let len = x[0].len();
    let bins = N / 2 + 1;
    let mut out = [Vec::new(), Vec::new()];
    for c in 0..2 {
        let mut synth = Synth::new(len, N);
        for t in 0..len / HOP + 2 {
            let center = (t * HOP) as i64;
            let carrier = frame_at(&x[c], center, &window);
            let shaper = frame_at(envelope_of, center, &window);
            let carrier_log: Vec<f64> = (0..bins).map(|k| (carrier.magnitude(k) + 1e-9).ln()).collect();
            let shaper_log: Vec<f64> = (0..bins).map(|k| (shaper.magnitude(k) + 1e-9).ln()).collect();
            let (carrier_env, shaper_env) = (smooth(&carrier_log), smooth(&shaper_log));
            let mut frame = carrier.clone();
            for k in 0..bins {
                let gain = (shaper_env[k] - carrier_env[k]).exp().min(1e4);
                frame.re[k] *= gain;
                frame.im[k] *= gain;
            }
            synth.add(&frame, t * HOP);
        }
        out[c] = synth.finish(len);
    }
    out
}

pub(super) fn modulate(sources: &[Prepared], options: &Options, mut rng: Rng) -> (Stereo, String, Ending) {
    let n = sources.len();
    let carrier = rng.below(n);
    let longest = sources.iter().map(Prepared::len).max().unwrap_or(0);
    let natural = sources[carrier].len().max(longest / 2) + (0.15 * RATE) as usize;
    let len = options.seconds.map_or(natural, |s| (s * RATE) as usize).clamp((0.2 * RATE) as usize, (super::MAX_SECONDS * RATE) as usize);
    let mut x: Stereo = [sources[carrier].audio[0].clone(), sources[carrier].audio[1].clone()];
    for c in x.iter_mut() {
        c.resize(len, 0.0);
    }
    let mut order: Vec<usize> = rng.shuffled(n).into_iter().filter(|&i| i != carrier).collect();
    let stages = order.len().max(2).min(6);
    while order.len() < stages {
        let extra = order[rng.below(order.len())];
        order.push(extra);
    }
    // A modulator under 60% of the carrier's length retriggers along it more often than not, every whole sixteenth
    // (at 120 BPM) it takes to play.
    let sixteenth = (0.125 * RATE) as usize;
    let modulators: Vec<Modulator> = (0..n)
        .map(|i| {
            let own = sources[i].len();
            let every = (own * 5 < len * 3 && rng.chance(0.6)).then(|| own.div_ceil(sixteenth).max(1) * sixteenth);
            let mut m = modulator(&sources[i], len, every);
            if every.is_some() {
                m.name.push_str(" (retriggered)");
            }
            m
        })
        .collect();
    let depth = options.depth.unwrap_or_else(|| rng.range(0.4, 0.85));
    let kinds = [Stage::Phase, Stage::Ring, Stage::Fm, Stage::Filter, Stage::Gain, Stage::Vocode];
    let odds = [0.26, 0.2, 0.14, 0.2, 0.08, 0.12];
    let mut last: Option<Stage> = None;
    let mut used_fm = false;
    let mut words = Vec::new();
    for &m in order.iter().take(stages) {
        let mut stage = kinds[rng.weighted(&odds)];
        while Some(stage) == last || (stage == Stage::Fm && used_fm) {
            stage = kinds[rng.weighted(&odds)];
        }
        last = Some(stage);
        used_fm |= stage == Stage::Fm;
        let modulator = &modulators[m];
        let amount = (depth * rng.range(0.7, 1.15)).clamp(0.05, 1.0);
        let (mut y, said) = match stage {
            Stage::Phase => {
                let reach = rng.range(4.0, 60.0) * amount;
                let y: Stereo = std::array::from_fn(|c| {
                    (0..len).map(|i| read_at(&x[c], i as f64 + reach * modulator.signal[i] as f64) as f32).collect()
                });
                (y, format!("read by {} (PM {:.0} smp)", modulator.name, reach))
            }
            Stage::Ring => {
                let ring = rng.chance(0.5);
                let y: Stereo = std::array::from_fn(|c| {
                    (0..len)
                        .map(|i| {
                            let m = modulator.signal[i] as f64;
                            let g = if ring { m } else { 0.5 + 0.5 * m };
                            (x[c][i] as f64 * g) as f32
                        })
                        .collect()
                });
                (y, format!("{} by {}", if ring { "ring" } else { "AM" }, modulator.name))
            }
            Stage::Fm => {
                let pitched = (0..n).filter_map(|i| analysis::pitch(&sources[i].mono, sources[i].peak)).collect::<Vec<_>>();
                let base = if pitched.is_empty() {
                    let colour = analysis::colour(&sources[carrier].mono, 0);
                    // An unpitched carrier: a note near half its brightness.
                    let note = (12.0 * (colour.centroid.max(60.0) / 2.0 / 440.0).log2()).round();
                    440.0 * 2f64.powf(note / 12.0)
                } else {
                    pitched[rng.below(pitched.len())]
                };
                let ratio = [0.5, 1.0, 1.0, 2.0, 1.5][rng.below(5)];
                let hz = (base * ratio).clamp(30.0, 4000.0);
                let index = rng.range(0.5, 6.0) * amount;
                let shape = fx::follow(&analysis::mono(&x), 0.002, 0.08);
                let y: Stereo = std::array::from_fn(|c| {
                    let spread = if c == 1 { 1.0015 } else { 1.0 };
                    let mut phase = 0.0f64;
                    (0..len)
                        .map(|i| {
                            phase += TAU * hz * spread * (1.0 + index * modulator.signal[i] as f64) / RATE;
                            (phase.sin() * shape[i] as f64) as f32
                        })
                        .collect()
                });
                (y, format!("FM at {:.0} Hz by {} (index {:.1})", hz, modulator.name, index))
            }
            Stage::Filter => {
                let release = rng.range(0.04, 0.2);
                let level = fx::follow(&modulator.signal, 0.002, release);
                let top = level.iter().copied().fold(0.0f32, f32::max).max(1e-9);
                let (lo, hi) = (rng.range(80.0, 400.0), rng.range(3000.0, 12000.0));
                let q = rng.range(0.7, 4.0);
                let inverse = rng.chance(0.25);
                // Low-pass mostly; band-pass sometimes; high-pass, which thins it as the modulator swells, now and then.
                let shape = rng.weighted(&[0.6, 0.28, 0.12]);
                let y: Stereo = std::array::from_fn(|c| {
                    let mut svf = fx::Svf::default();
                    (0..len)
                        .map(|i| {
                            let e = (level[i] / top) as f64;
                            let e = if inverse { 1.0 - e } else { e };
                            let out = svf.process(x[c][i] as f64, lo * (hi / lo).powf(e), q);
                            (match shape {
                                0 => out.low,
                                1 => out.band,
                                _ => out.high,
                            }) as f32
                        })
                        .collect()
                });
                let kind = ["low-pass", "band-pass", "high-pass"][shape];
                (y, format!("{kind} filter {} {}", if inverse { "ducks under" } else { "follows" }, modulator.name))
            }
            Stage::Gain => {
                let level = fx::follow(&modulator.signal, 0.003, rng.range(0.03, 0.15));
                let top = level.iter().copied().fold(0.0f32, f32::max).max(1e-9);
                let duck = rng.chance(0.5);
                let y: Stereo = std::array::from_fn(|c| {
                    (0..len)
                        .map(|i| {
                            let e = (level[i] / top) as f64;
                            (x[c][i] as f64 * if duck { 1.0 - 0.9 * e } else { e }) as f32
                        })
                        .collect()
                });
                (y, format!("gain {} {}", if duck { "ducks under" } else { "follows" }, modulator.name))
            }
            Stage::Vocode => (vocode(&x, &modulator.signal), format!("spectrum of {}", modulator.name)),
        };
        level_to(&mut y, &x, &modulator.presence);
        for c in 0..2 {
            for i in 0..len {
                let wet = amount * modulator.presence[i] as f64;
                x[c][i] = (x[c][i] as f64 * (1.0 - wet) + y[c][i] as f64 * wet) as f32;
            }
        }
        words.push(said);
    }
    let recipe = format!("Modulate {}: {}; depth {:.0}%", short(&sources[carrier].name), words.join(" → "), depth * 100.0);
    (x, recipe, Ending::OneShot { fade: (0.04 * RATE) as usize })
}
