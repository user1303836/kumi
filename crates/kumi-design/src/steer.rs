//! Steering a design before it renders. Controls on the result (tone, shape, colour, space) work on what the design
//! made; controls on each source repitch, reverse, start later, delay, pan and level every sound before the design
//! hears it. Each source draws its own value inside the range given, from the seed, so a reroll draws again. Every
//! control is optional and neutral when left out: a steer that moves nothing renders the same bytes as none.

use super::{
    analysis::{self, frame_at, hann, wrap, Stereo, Synth},
    fx, refuse,
    rng::Rng,
    DesignError, Ending, Prepared, MAX_SOURCE_SECONDS, RATE,
};
use kumi_media::audio::dsp::resample;
use serde::{Deserialize, Serialize};
use std::f64::consts::{PI, TAU};

/// The controls (see the crate's docs for what each does). On the result: `bright`, `body`, `filter`, `resonance`,
/// `noise`, `punch`, `length`, `drive`, `crush`, `width`, `space`. On each source: `pitch`, `reverse`, `start`,
/// `delay`, `pan`, `level`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Steer {
    /// Dark (-1) to bright (1): a tilt about 1.5 kHz with air above.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bright: Option<f64>,
    /// Thin (-1) to full (1): a shelf below 200 Hz.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<f64>,
    /// A low-pass closing toward 150 Hz (-1) or a high-pass opening toward 5 kHz (1); 0 is open.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filter: Option<f64>,
    /// The filter's peak at its cutoff (0 to 1).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resonance: Option<f64>,
    /// Tonal (-1, spectral peaks sharpened) to noisy (1, the spectrum flattened and its phases scattered).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub noise: Option<f64>,
    /// Soft (-1) to percussive (1): transients down or up by as much as 12 dB.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub punch: Option<f64>,
    /// Shorter (-1, decaying up to 4× faster) to longer (1, ringing up to 2× longer).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub length: Option<f64>,
    /// Saturation, its level matched (0 to 1).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub drive: Option<f64>,
    /// Fewer bits and a lower rate (0 to 1).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub crush: Option<f64>,
    /// Mono (-1) to wide (1).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub width: Option<f64>,
    /// Dry (0) to a room growing into a hall (1).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub space: Option<f64>,
    /// Each source repitched, tape-style, by semitones somewhere in `[low, high]` (-24 to 24).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pitch: Option<[f64; 2]>,
    /// The chance each source plays backwards (0 to 1).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reverse: Option<f64>,
    /// Each source starts this far into itself, a fraction of its length in `[low, high]` (0 to 0.9).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start: Option<[f64; 2]>,
    /// Each source comes in this many milliseconds later, somewhere in `[low, high]` (0 to 500).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delay: Option<[f64; 2]>,
    /// Each source panned somewhere within ±`pan` (0 to 1).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pan: Option<f64>,
    /// Each source's level drops by up to `level` × 18 dB (0 to 1).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub level: Option<f64>,
}

/// A control set away from neutral.
fn moved(value: Option<f64>) -> Option<f64> {
    value.filter(|v| v.abs() > 1e-6)
}

/// A range set away from `[0, 0]`.
fn spans(range: Option<[f64; 2]>) -> Option<[f64; 2]> {
    range.filter(|[low, high]| low.abs() > 1e-9 || high.abs() > 1e-9)
}

fn hz_words(hz: f64) -> String {
    if hz >= 1000.0 {
        format!("{:.1} kHz", hz / 1000.0)
    } else {
        format!("{:.0} Hz", hz)
    }
}

fn range_words(low: f64, high: f64, unit: &str, signed: bool) -> String {
    let say = |v: f64| if signed { format!("{v:+.0}").replace('-', "−") } else { format!("{v:.0}") };
    if (high - low).abs() < 0.5 {
        format!("{}{unit}", say(low))
    } else {
        format!("{}…{}{unit}", say(low), say(high))
    }
}

impl Steer {
    /// Whether any control moves anything.
    pub fn moves(&self) -> bool {
        [
            self.bright,
            self.body,
            self.filter,
            self.noise,
            self.punch,
            self.length,
            self.drive,
            self.crush,
            self.width,
            self.space,
            self.reverse,
            self.pan,
            self.level,
        ]
        .into_iter()
        .any(|v| moved(v).is_some())
            || [self.pitch, self.start, self.delay].into_iter().any(|r| spans(r).is_some())
    }

    pub(crate) fn check(&self) -> Result<(), DesignError> {
        for (value, what, low, high) in [
            (self.bright, "bright", -1.0, 1.0),
            (self.body, "body", -1.0, 1.0),
            (self.filter, "filter", -1.0, 1.0),
            (self.resonance, "resonance", 0.0, 1.0),
            (self.noise, "noise", -1.0, 1.0),
            (self.punch, "punch", -1.0, 1.0),
            (self.length, "length", -1.0, 1.0),
            (self.drive, "drive", 0.0, 1.0),
            (self.crush, "crush", 0.0, 1.0),
            (self.width, "width", -1.0, 1.0),
            (self.space, "space", 0.0, 1.0),
            (self.reverse, "reverse", 0.0, 1.0),
            (self.pan, "pan", 0.0, 1.0),
            (self.level, "level", 0.0, 1.0),
        ] {
            if let Some(v) = value {
                if !(v.is_finite() && (low..=high).contains(&v)) {
                    return refuse(format!("steer's {what} is {low} to {high}."));
                }
            }
        }
        for (range, what, low, high) in
            [(self.pitch, "pitch", -24.0, 24.0), (self.start, "start", 0.0, 0.9), (self.delay, "delay", 0.0, 500.0)]
        {
            if let Some([a, b]) = range {
                if !(a.is_finite() && b.is_finite() && low <= a && a <= b && b <= high) {
                    return refuse(format!("steer's {what} is [low, high], low first, each {low} to {high}."));
                }
            }
        }
        Ok(())
    }

    /// The sources as the per-source controls draw them, each control from its own stream of the seed (so one
    /// control's draws never move another's), and the recipe's words for what they did.
    pub(crate) fn sources(&self, sources: Vec<Prepared>, rng: &Rng) -> (Vec<Prepared>, Vec<String>) {
        let n = sources.len();
        let draws = |salt: u64| {
            let mut r = rng.fork(salt);
            (0..n).map(|_| r.unit()).collect::<Vec<f64>>()
        };
        let within = |range: [f64; 2], salt: u64| draws(salt).into_iter().map(|u| range[0] + (range[1] - range[0]) * u).collect::<Vec<f64>>();
        let pitch = spans(self.pitch).map(|r| within(r, 1));
        let reverse = moved(self.reverse).map(|p| draws(2).into_iter().map(|u| u < p).collect::<Vec<bool>>());
        let start = spans(self.start).map(|r| within(r, 3));
        let delay = spans(self.delay).map(|r| within(r, 4));
        let pan = moved(self.pan).map(|p| draws(5).into_iter().map(|u| p * (2.0 * u - 1.0)).collect::<Vec<f64>>());
        let level = moved(self.level).map(|l| draws(6).into_iter().map(|u| -l * 18.0 * u).collect::<Vec<f64>>());
        let most = (MAX_SOURCE_SECONDS * RATE) as usize;
        let shortest = (0.02 * RATE) as usize;
        let mut out = Vec::with_capacity(n);
        for (i, source) in sources.into_iter().enumerate() {
            let mut audio = source.audio;
            if let Some(semitones) = pitch.as_ref().map(|p| p[i]).filter(|s| s.abs() > 0.01) {
                let ratio = 2f64.powf(semitones / 12.0);
                for channel in audio.iter_mut() {
                    let mut moved = resample(channel, RATE * ratio, RATE);
                    moved.truncate(most);
                    *channel = moved;
                }
            }
            if reverse.as_ref().is_some_and(|r| r[i]) {
                for channel in audio.iter_mut() {
                    channel.reverse();
                }
                fx::fade_in(&mut audio, 64);
            }
            if let Some(fraction) = start.as_ref().map(|s| s[i]).filter(|f| *f > 0.0) {
                let len = audio[0].len();
                let skip = ((fraction * len as f64) as usize).min(len.saturating_sub(shortest));
                for channel in audio.iter_mut() {
                    channel.drain(..skip);
                }
                fx::fade_in(&mut audio, (0.003 * RATE) as usize);
            }
            if let Some(db) = level.as_ref().map(|l| l[i]) {
                fx::scale(&mut audio, 10f64.powf(db / 20.0));
            }
            if let Some(place) = pan.as_ref().map(|p| p[i]) {
                for (channel, gain) in audio.iter_mut().zip(fx::pan(place)) {
                    for v in channel.iter_mut() {
                        *v = (*v as f64 * gain) as f32;
                    }
                }
            }
            if let Some(ms) = delay.as_ref().map(|d| d[i]).filter(|ms| *ms >= 0.5) {
                let pad = (ms / 1000.0 * RATE) as usize;
                for channel in audio.iter_mut() {
                    channel.splice(0..0, std::iter::repeat_n(0.0, pad));
                }
            }
            let mono = analysis::mono(&audio);
            let lead = mono.iter().position(|v| v.abs() > 0.0).unwrap_or(0);
            let peak = analysis::peak_after(&mono, lead, (0.05 * RATE) as usize);
            out.push(Prepared { name: source.name, audio, mono, peak });
        }
        let mut words = Vec::new();
        if let Some([low, high]) = spans(self.pitch) {
            words.push(format!("pitch {}", range_words(low, high, " st", true)));
        }
        if let Some(reversed) = &reverse {
            let count = reversed.iter().filter(|r| **r).count();
            words.push(format!("{count} of {n} reversed"));
        }
        if let Some([low, high]) = spans(self.start) {
            words.push(format!("started {} in", range_words(low * 100.0, high * 100.0, "%", false)));
        }
        if let Some([low, high]) = spans(self.delay) {
            words.push(format!("delayed {}", range_words(low, high, " ms", false)));
        }
        if let Some(p) = moved(self.pan) {
            words.push(format!("panned ±{:.0}%", p * 100.0));
        }
        if let Some(l) = moved(self.level) {
            words.push(format!("levels within −{:.0} dB", l * 18.0));
        }
        (out, words)
    }

    /// The result as the controls on it shape it, and the recipe's words for what they did. A loop is shaped over
    /// two rounds and its second kept, so every effect comes round into its start.
    pub(crate) fn result(&self, audio: Stereo, ending: &Ending, rng: &Rng) -> (Stereo, Vec<String>) {
        let looped = matches!(ending, Ending::Loop);
        let len = audio[0].len();
        let mut x: Stereo = if looped { std::array::from_fn(|c| audio[c].iter().chain(&audio[c]).copied().collect()) } else { audio };
        let mut words = Vec::new();
        if !looped {
            // Room for a tail to ring into: finishing trims what stays silent.
            let room = moved(self.space).map_or(0.0, |s| (0.3 + 3.2 * s) * RATE) as usize;
            for channel in x.iter_mut() {
                channel.resize(channel.len() + room, 0.0);
            }
        }
        if let Some(amount) = moved(self.noise) {
            spectral(&mut x, amount, rng.fork(11));
            words.push(if amount < 0.0 { "more tonal" } else { "noisier" }.into());
        }
        if let Some(sweep) = moved(self.filter) {
            let resonance = self.resonance.unwrap_or(0.0);
            let (hz, low) =
                if sweep < 0.0 { (20_000.0 * (150.0 / 20_000.0f64).powf(-sweep), true) } else { (20.0 * (5_000.0 / 20.0f64).powf(sweep), false) };
            let q = 0.707 + 9.0 * resonance;
            for channel in x.iter_mut() {
                let mut svf = fx::Svf::default();
                for v in channel.iter_mut() {
                    let out = svf.process(*v as f64, hz, q);
                    *v = (if low { out.low } else { out.high }) as f32;
                }
            }
            let peak = if resonance >= 0.3 { ", resonant" } else { "" };
            words.push(format!("{}-passed at {}{peak}", if low { "low" } else { "high" }, hz_words(hz)));
        }
        if let Some(body) = moved(self.body) {
            fx::filter(&mut x, || fx::shelf(200.0, 9.0 * body, false));
            words.push(if body < 0.0 { "thinner" } else { "fuller" }.into());
        }
        if let Some(bright) = moved(self.bright) {
            fx::filter(&mut x, || fx::shelf(1500.0, 8.0 * bright, true));
            fx::filter(&mut x, || fx::shelf(8000.0, 4.0 * bright, true));
            fx::filter(&mut x, || fx::shelf(250.0, -3.0 * bright, false));
            words.push(if bright < 0.0 { "darker" } else { "brighter" }.into());
        }
        if let Some(punch) = moved(self.punch) {
            transients(&mut x, punch);
            words.push(if punch < 0.0 { "softer" } else { "punchier" }.into());
        }
        if let Some(length) = moved(self.length) {
            x = if looped { lengthen_hits(x, length) } else { lengthen(x, length) };
            words.push(if length < 0.0 { "shorter" } else { "longer" }.into());
        }
        if let Some(drive) = moved(self.drive) {
            saturate(&mut x, drive);
            words.push("driven".into());
        }
        if let Some(crush) = moved(self.crush) {
            crushed(&mut x, crush);
            words.push("crushed".into());
        }
        if let Some(width) = moved(self.width) {
            widen(&mut x, width);
            words.push(if width <= -0.99 { "mono" } else if width < 0.0 { "narrower" } else { "wider" }.into());
        }
        if let Some(space) = moved(self.space) {
            let mut verb_rng = rng.fork(12);
            let t60 = 0.3 + 3.2 * space;
            let wet = fx::reverb(&x, t60, 0.5 + 1.3 * space, 0.3, &mut verb_rng, 0);
            for (channel, wet) in x.iter_mut().zip(&wet) {
                for (v, w) in channel.iter_mut().zip(wet) {
                    *v = (*v as f64 * (1.0 - 0.35 * space) + *w as f64 * 0.9 * space) as f32;
                }
            }
            words.push(if space < 0.5 { "in a room" } else { "in a hall" }.into());
        }
        if looped {
            for channel in x.iter_mut() {
                *channel = channel[len..2 * len].to_vec();
            }
        }
        (x, words)
    }
}

/// Tonal (`amount` below 0) to noisy (above): each frame's magnitudes raised to a power (above 1 the peaks stand out,
/// below 1 the spectrum flattens), its energy kept; going noisy also scatters the phases.
fn spectral(x: &mut Stereo, amount: f64, mut rng: Rng) {
    let (n, hop) = (2048usize, 512usize);
    let window = hann(n);
    let bins = n / 2 + 1;
    let power = 2f64.powf(-1.25 * amount);
    let scatter = amount.max(0.0).powi(2) * PI;
    let len = x[0].len();
    for channel in x.iter_mut() {
        let mut synth = Synth::new(len, n);
        for t in 0..len / hop + 2 {
            let center = t * hop;
            let mut frame = frame_at(channel, center as i64, &window);
            let magnitudes: Vec<f64> = (0..bins).map(|k| frame.magnitude(k)).collect();
            let energy: f64 = magnitudes.iter().map(|m| m * m).sum();
            let top = magnitudes.iter().copied().fold(0.0f64, f64::max);
            if energy > 1e-20 && top > 0.0 {
                let shaped: Vec<f64> = magnitudes.iter().map(|m| top * (m / top).powf(power)).collect();
                let gain = (energy / shaped.iter().map(|m| m * m).sum::<f64>().max(1e-30)).sqrt();
                for k in 0..bins {
                    let turn = if scatter > 0.0 && k > 0 && k < n / 2 { scatter * (2.0 * rng.unit() - 1.0) } else { 0.0 };
                    let phase = frame.phase(k) + turn;
                    let m = shaped[k] * gain;
                    frame.re[k] = m * phase.cos();
                    frame.im[k] = if k > 0 && k < n / 2 { m * phase.sin() } else { 0.0 };
                }
            }
            synth.add(&frame, center);
        }
        *channel = synth.finish(len);
    }
}

/// Transients up (`amount` above 0) or down by as much as 12 dB: where a fast envelope runs ahead of a slow one.
fn transients(x: &mut Stereo, amount: f64) {
    let mono = analysis::mono(x);
    let fast = fx::follow(&mono, 0.0005, 0.05);
    let slow = fx::follow(&mono, 0.02, 0.05);
    for i in 0..mono.len() {
        let lead = ((fast[i] as f64 / (slow[i] as f64 + 1e-9)) - 1.0).clamp(0.0, 1.0);
        let gain = 10f64.powf(amount * 12.0 * lead / 20.0) as f32;
        x[0][i] *= gain;
        x[1][i] *= gain;
    }
}

/// A one-shot shorter (`amount` below 0: it decays up to 4× faster past its hit) or longer (above: what follows its
/// hit stretched up to 2×, its pitch kept).
fn lengthen(x: Stereo, amount: f64) -> Stereo {
    let mono = analysis::mono(&x);
    let len = mono.len();
    let hit = analysis::peak_after(&mono, 0, (0.1 * RATE) as usize);
    if amount < 0.0 {
        let end = analysis::end_of(&mono).max(hit + 1);
        let mut x = x;
        decay(&mut x, hit, end, -amount);
        return x;
    }
    let keep = (hit + (0.03 * RATE) as usize).min(len);
    let blend = ((0.005 * RATE) as usize).min(len - keep);
    std::array::from_fn(|c| {
        let stretched = stretch(&x[c][keep..], 1.0 + amount);
        let mut out = x[c][..keep].to_vec();
        for (i, v) in stretched.iter().enumerate() {
            if i < blend {
                let w = i as f64 / blend as f64;
                out.push((x[c][keep + i] as f64 * (1.0 - w) + *v as f64 * w) as f32);
            } else {
                out.push(*v);
            }
        }
        out
    })
}

/// A loop's hits shorter or longer, each from its own onset: shorter decays faster past each hit, longer lifts each
/// hit's tail toward its peak (by as much as 9 dB).
fn lengthen_hits(mut x: Stereo, amount: f64) -> Stereo {
    let mono = analysis::mono(&x);
    let len = mono.len();
    let mut starts = analysis::onsets(&mono);
    if starts.first() != Some(&0) {
        starts.insert(0, 0);
    }
    let level = fx::follow(&mono, 0.001, 0.05);
    for (j, &start) in starts.iter().enumerate() {
        let end = starts.get(j + 1).copied().unwrap_or(len);
        let hit = analysis::peak_after(&mono, start, ((0.03 * RATE) as usize).min(end - start));
        if amount < 0.0 {
            decay(&mut x, hit, end, -amount);
        } else {
            let top = level[hit].max(1e-9) as f64;
            for i in hit..end {
                let below = top / (level[i] as f64).max(top * 1e-3);
                let lift = (20.0 * below.log10() * 0.35 * amount).min(9.0 * amount);
                let gain = 10f64.powf(lift / 20.0) as f32;
                x[0][i] *= gain;
                x[1][i] *= gain;
            }
        }
    }
    x
}

/// Past `hit`, an extra fall of `faster` × 3 × 60 dB over the time to `end`: the sound dies `1 + 3 × faster` times
/// sooner.
fn decay(x: &mut Stereo, hit: usize, end: usize, faster: f64) {
    let span = (end - hit).max(1) as f64;
    for i in hit..x[0].len() {
        let db = (-180.0 * faster * (i - hit) as f64 / span).max(-120.0);
        let gain = if db <= -120.0 { 0.0 } else { 10f64.powf(db / 20.0) as f32 };
        x[0][i] *= gain;
        x[1][i] *= gain;
    }
}

/// `x` made `factor` times longer at the same pitch, by a phase vocoder.
fn stretch(x: &[f32], factor: f64) -> Vec<f32> {
    let (n, synthesis) = (2048usize, 512usize);
    let window = hann(n);
    let bins = n / 2 + 1;
    let out_len = (x.len() as f64 * factor) as usize;
    let mut synth = Synth::new(out_len, n);
    let mut last = vec![0.0f64; bins];
    let mut phase = vec![0.0f64; bins];
    let mut read_at = 0i64;
    for j in 0..out_len / synthesis + 2 {
        let at = (j as f64 * synthesis as f64 / factor).round() as i64;
        let frame = frame_at(x, at, &window);
        let mut out = frame.clone();
        let step = (at - read_at) as f64;
        for k in 0..bins {
            let measured = frame.phase(k);
            if j == 0 || step <= 0.0 {
                phase[k] = measured;
            } else {
                let omega = TAU * k as f64 / n as f64;
                let true_freq = omega + wrap(measured - last[k] - omega * step) / step;
                phase[k] += true_freq * synthesis as f64;
            }
            last[k] = measured;
            let m = frame.magnitude(k);
            out.re[k] = m * phase[k].cos();
            out.im[k] = if k > 0 && k < n / 2 { m * phase[k].sin() } else { 0.0 };
        }
        read_at = at;
        synth.add(&out, j * synthesis);
    }
    synth.finish(out_len)
}

/// Soft saturation up to 24 dB into a tanh, brought back to the level it had.
fn saturate(x: &mut Stereo, amount: f64) {
    let drive = 10f64.powf(amount * 24.0 / 20.0);
    let before = fx::rms(x, 0, x[0].len());
    for channel in x.iter_mut() {
        for v in channel.iter_mut() {
            *v = (drive * *v as f64).tanh() as f32;
        }
    }
    let after = fx::rms(x, 0, x[0].len());
    if after > 1e-12 {
        fx::scale(x, before / after);
    }
}

/// From 16 bits at full rate down to 4 bits at a twelfth of it.
fn crushed(x: &mut Stereo, amount: f64) {
    let levels = 2f64.powf(15.0 - 12.0 * amount);
    let hold = 1 + (11.0 * amount * amount).round() as usize;
    for channel in x.iter_mut() {
        let mut held = 0.0f64;
        for (i, v) in channel.iter_mut().enumerate() {
            if i % hold == 0 {
                held = *v as f64;
            }
            *v = ((held * levels).round() / levels) as f32;
        }
    }
}

/// The sides scaled from nothing (`amount` -1) to twice (1); widening also adds a decorrelated copy of the middle, so
/// a mono sound widens too.
fn widen(x: &mut Stereo, amount: f64) {
    let len = x[0].len();
    let middle: Vec<f32> = (0..len).map(|i| 0.5 * (x[0][i] + x[1][i])).collect();
    let spread = if amount > 0.0 { allpasses(&middle, &[113, 337, 509]) } else { Vec::new() };
    for i in 0..len {
        let (m, s) = (middle[i] as f64, 0.5 * (x[0][i] - x[1][i]) as f64);
        let mut side = s * (1.0 + amount);
        if amount > 0.0 {
            side += amount * 0.35 * spread[i] as f64;
        }
        x[0][i] = (m + side) as f32;
        x[1][i] = (m - side) as f32;
    }
}

/// `x` through Schroeder allpasses of the given delays (samples), gain 0.5.
fn allpasses(x: &[f32], delays: &[usize]) -> Vec<f32> {
    let mut signal: Vec<f64> = x.iter().map(|v| *v as f64).collect();
    for &delay in delays {
        let mut out = vec![0.0f64; signal.len()];
        for i in 0..signal.len() {
            let back = if i >= delay { signal[i - delay] } else { 0.0 };
            let fed = if i >= delay { out[i - delay] } else { 0.0 };
            out[i] = -0.5 * signal[i] + back + 0.5 * fed;
        }
        signal = out;
    }
    signal.into_iter().map(|v| v as f32).collect()
}
