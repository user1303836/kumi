//! Fuse: a cluster of sounds as one continuous sound that moves between them. **Texture** is a loopable granular (or
//! spectral) bed whose mix follows a seeded path through the sources; **loop** slices their hits and sequences them on a
//! tempo grid; **smear** stretches them 4 to 16 times into a diffuse wash.

use super::{
    analysis::{self, envelope, frame_at, hann, read_at, Frame, Stereo, Synth},
    fx,
    rng::Rng,
    short, Ending, Options, Prepared, RATE,
};
use kumi_media::audio::dsp::{high_pass, low_pass};
use std::f64::consts::TAU;

/// One grain: where it lands, where it reads its source, how long, at what speed (pitch), how loud and where it's panned.
struct Grain {
    source: usize,
    at: usize,
    from: f64,
    len: usize,
    rate: f64,
    gain: f64,
    pan: f64,
}

/// Grains summed into a buffer `len` long (those past the end run on past it).
fn render_grains(grains: &[Grain], sources: &[Prepared], len: usize) -> Stereo {
    let longest = grains.iter().map(|g| g.at + g.len).max().unwrap_or(0).max(len);
    let mut out = fx::silence(longest);
    for g in grains {
        let window = hann(g.len);
        let pan = fx::pan(g.pan);
        let source = &sources[g.source].audio;
        for c in 0..2 {
            let gain = g.gain * pan[c];
            for (i, w) in window.iter().enumerate() {
                out[c][g.at + i] += (read_at(&source[c], g.from + i as f64 * g.rate) * w * gain) as f32;
            }
        }
    }
    out
}

/// Where each source is loud enough to take a grain from (64-sample steps), and its level at each step.
struct Map {
    loud: Vec<usize>,
    env: Vec<f32>,
    rms: f64,
}

fn map(source: &Prepared) -> Map {
    let env = envelope(&source.mono, 64);
    let peak = env.iter().copied().fold(0.0f32, f32::max);
    let loud: Vec<usize> = env.iter().enumerate().filter(|(_, v)| **v >= peak * 0.1).map(|(i, _)| i * 64).collect();
    let rms = (source.mono.iter().map(|v| (*v as f64).powi(2)).sum::<f64>() / source.mono.len().max(1) as f64).sqrt();
    Map { loud, env, rms }
}

/// A grain's speed: mostly a few cents off, sometimes an interval away (more with `scatter`).
fn grain_rate(rng: &mut Rng, scatter: f64) -> f64 {
    let semitones =
        if rng.chance(scatter * 0.6) { [-12.0, -7.0, -5.0, 5.0, 7.0, 12.0][rng.below(6)] } else { rng.range(-1.0, 1.0) * scatter * 0.3 };
    2f64.powf(semitones / 12.0)
}

/// A smooth seeded path through the sources that comes back to where it began after `period` samples: each source's
/// share at each moment.
struct Path {
    waves: Vec<Vec<(f64, f64, f64)>>,
    period: f64,
    temperature: f64,
}

impl Path {
    fn new(rng: &mut Rng, n: usize, period: usize) -> Self {
        let waves =
            (0..n).map(|_| (0..3).map(|_| (rng.range(0.3, 1.0), (1 + rng.below(3)) as f64, rng.range(0.0, TAU))).collect()).collect();
        Path { waves, period: period as f64, temperature: rng.range(0.25, 0.7) }
    }
    fn shares(&self, at: f64) -> Vec<f64> {
        let t = at / self.period;
        let raw: Vec<f64> =
            self.waves.iter().map(|w| w.iter().map(|(a, k, p)| a * (TAU * k * t + p).sin()).sum::<f64>() / self.temperature).collect();
        let top = raw.iter().copied().fold(f64::MIN, f64::max);
        let e: Vec<f64> = raw.iter().map(|v| (v - top).exp()).collect();
        let sum: f64 = e.iter().sum();
        e.into_iter().map(|v| v / sum).collect()
    }
    /// The order the sources lead in along the path, for the recipe.
    fn leaders(&self, steps: usize) -> Vec<usize> {
        let mut order = Vec::new();
        for s in 0..steps {
            let shares = self.shares(s as f64 * self.period / steps as f64);
            let lead = (0..shares.len()).max_by(|a, b| shares[*a].total_cmp(&shares[*b])).unwrap_or(0);
            if order.last() != Some(&lead) {
                order.push(lead);
            }
        }
        if order.len() > 1 && order.first() == order.last() {
            order.pop();
        }
        order
    }
}

/// Runs a reverb over a loop as if it had been playing round and round: over two periods, keeping the second.
fn loop_reverb(audio: &Stereo, rng: &mut Rng, t60: f64, wet: f64) -> Stereo {
    let len = audio[0].len();
    let twice: Stereo = std::array::from_fn(|c| audio[c].iter().chain(&audio[c]).copied().collect());
    let tail = fx::reverb(&twice, t60, rng.range(0.8, 1.6), rng.range(0.2, 0.5), rng, 0);
    std::array::from_fn(|c| (0..len).map(|i| audio[c][i] * (1.0 - wet as f32) + tail[c][len + i] * wet as f32).collect())
}

pub(super) fn texture(sources: &[Prepared], options: &Options, mut rng: Rng, progress: &mut dyn FnMut(f64)) -> (Stereo, String, Ending) {
    let len = (options.seconds.unwrap_or(8.0) * RATE) as usize;
    let n = sources.len();
    let path = Path::new(&mut rng, n, len);
    let maps: Vec<Map> = sources.iter().map(map).collect();
    let scatter = options.depth.unwrap_or_else(|| rng.range(0.05, 0.5));
    let spectral = rng.chance(0.3);
    let (mut audio, how) = if spectral {
        // Spectral: each frame's magnitudes the path's mix of the sources' spectra where each one's scan has reached,
        // with phases that drift: a smooth bed of their colours.
        let size = 4096usize;
        let hop = 1024usize;
        let window = hann(size);
        let bins = size / 2 + 1;
        let fade = (len / 8).min((0.5 * RATE) as usize);
        let total = len + fade;
        let speeds: Vec<f64> = (0..n).map(|_| rng.range(0.05, 0.4)).collect();
        let starts: Vec<f64> = (0..n).map(|_| rng.unit()).collect();
        let mut synth = [Synth::new(total, size), Synth::new(total, size)];
        let mut phase = [vec![0.0f64; bins], vec![0.0f64; bins]];
        for c in 0..2 {
            for p in phase[c].iter_mut() {
                *p = rng.range(0.0, TAU);
            }
        }
        let frames = total / hop + 2;
        for t in 0..frames {
            let at = t * hop;
            let shares = path.shares(at as f64);
            let mut magnitude = [vec![0.0f64; bins], vec![0.0f64; bins]];
            for i in 0..n {
                if shares[i] < 0.02 || maps[i].loud.is_empty() {
                    continue;
                }
                let loud = &maps[i].loud;
                let index = ((starts[i] + at as f64 / RATE * speeds[i]) * loud.len() as f64) as usize % loud.len();
                for c in 0..2 {
                    let frame = frame_at(&sources[i].audio[c], loud[index] as i64, &window);
                    let level = 1.0 / maps[i].rms.max(1e-6);
                    for k in 0..bins {
                        magnitude[c][k] += shares[i] * frame.magnitude(k) * level;
                    }
                }
            }
            for c in 0..2 {
                for k in 0..bins {
                    phase[c][k] += TAU * k as f64 * hop as f64 / size as f64 + rng.range(-0.6, 0.6);
                }
                let frame = Frame {
                    re: (0..bins).map(|k| magnitude[c][k] * phase[c][k].cos()).collect(),
                    im: (0..bins).map(|k| magnitude[c][k] * phase[c][k].sin()).collect(),
                };
                synth[c].add(&frame, at);
            }
            if t % 16 == 0 {
                progress(0.1 + 0.7 * t as f64 / frames as f64);
            }
        }
        let [l, r] = synth;
        let mut audio = [l.finish(total), r.finish(total)];
        fx::loop_crossfade(&mut audio, len, fade);
        (audio, "spectral".to_string())
    } else {
        // Granular: grains drawn along the path, each from its source's scan, kept out of its silences; grains that
        // run past the end come round to the start, so the bed loops.
        let density = rng.range(18.0, 60.0);
        let grain_seconds = rng.range(0.05, 0.22);
        let spread = rng.range(0.3, 0.9);
        let speeds: Vec<f64> = (0..n).map(|_| rng.range(0.2, 1.0)).collect();
        let starts: Vec<f64> = (0..n).map(|_| rng.unit()).collect();
        let count = (len as f64 / RATE * density) as usize;
        let mut grains = Vec::with_capacity(count);
        for g in 0..count {
            let at = ((g as f64 + rng.range(-0.5, 0.5)) / density * RATE).max(0.0) as usize % len;
            let source = rng.weighted(&path.shares(at as f64));
            let m = &maps[source];
            let source_len = sources[source].len();
            let glen = ((grain_seconds * rng.range(0.7, 1.3)) * RATE) as usize;
            let mut from = ((starts[source] + at as f64 / RATE * speeds[source] / (source_len as f64 / RATE).max(0.05)).fract()
                * source_len as f64
                + rng.range(-0.05, 0.05) * RATE)
                .clamp(0.0, source_len.saturating_sub(1) as f64);
            let level = m.env.get(from as usize / 64).copied().unwrap_or(0.0) as f64;
            if level < m.rms * 0.3 && !m.loud.is_empty() {
                from = m.loud[rng.below(m.loud.len())] as f64;
            }
            let local = m.env.get(from as usize / 64).copied().unwrap_or(0.0) as f64;
            let gain = (m.rms / local.max(m.rms * 0.05)).sqrt() / (density * grain_seconds).sqrt();
            grains.push(Grain {
                source,
                at,
                from,
                len: glen.max(64),
                rate: grain_rate(&mut rng, scatter),
                gain,
                pan: rng.range(-spread, spread),
            });
        }
        progress(0.5);
        let mut audio = render_grains(&grains, sources, len);
        fx::wrap_tail(&mut audio, len);
        (audio, format!("granular, {:.0} grains/s of {:.0} ms", density, grain_seconds * 1000.0))
    };
    let verb = rng.chance(0.5);
    if verb {
        let (t60, wet) = (rng.range(1.5, 4.0), rng.range(0.15, 0.35));
        audio = loop_reverb(&audio, &mut rng, t60, wet);
    }
    let leaders: Vec<String> = path.leaders(64).into_iter().map(|i| short(&sources[i].name)).collect();
    let recipe = format!(
        "Texture of {} ({}{}): moves {}; loops at {:.1} s",
        n,
        how,
        if verb { ", with a room" } else { "" },
        leaders.join(" → "),
        len as f64 / RATE
    );
    (audio, recipe, Ending::Loop)
}

/// A hit sliced from a source.
struct Hit {
    audio: Stereo,
    colour: analysis::Colour,
    source: usize,
}

fn slice(sources: &[Prepared], most: usize) -> Vec<Hit> {
    let mut hits = Vec::new();
    for (s, source) in sources.iter().enumerate() {
        let mut starts = analysis::onsets(&source.mono);
        if starts.first().is_none_or(|first| *first > (0.03 * RATE) as usize) {
            starts.insert(0, 0);
        } else {
            starts[0] = 0;
        }
        let mut found: Vec<(f64, Hit)> = Vec::new();
        for (j, &start) in starts.iter().enumerate() {
            let mut end = starts.get(j + 1).copied().unwrap_or(source.len()).min(start + most).min(source.len());
            end = start + first_hit(&source.mono[start..end]);
            if end <= start + (0.02 * RATE) as usize {
                continue;
            }
            let mut audio: Stereo = [source.audio[0][start..end].to_vec(), source.audio[1][start..end].to_vec()];
            fx::fade_in(&mut audio, 48);
            fx::fade_out(&mut audio, (0.008 * RATE) as usize);
            let level = fx::rms(&audio, 0, ((0.1 * RATE) as usize).min(end - start));
            let colour = analysis::colour(&analysis::mono(&audio), 0);
            found.push((level, Hit { audio, colour, source: s }));
        }
        found.sort_by(|a, b| b.0.total_cmp(&a.0));
        hits.extend(found.into_iter().take(8).map(|(_, h)| h));
    }
    hits.truncate(48);
    hits
}

/// How much of a slice is its one hit: up to where, past its peak, its level climbs again (a hit the onsets missed).
fn first_hit(x: &[f32]) -> usize {
    let env = envelope(x, 64);
    let Some(peak_at) = (0..env.len()).max_by(|a, b| env[*a].total_cmp(&env[*b])) else { return x.len() };
    let peak = env[peak_at];
    let mut low = peak;
    for (i, v) in env.iter().enumerate().skip(peak_at + 1) {
        if *v > low * 2.0 && *v > peak * 0.08 {
            // Back to the quiet before the climb.
            return ((i.saturating_sub(2)) * 64).max(64).min(x.len());
        }
        low = low.min(*v);
    }
    x.len()
}

/// `k` hits spread as evenly as they go over `n` steps, turned `rotation` steps (Bjorklund's patterns).
fn euclid(k: usize, n: usize, rotation: usize) -> Vec<bool> {
    (0..n).map(|i| ((i + n - rotation % n) * k) % n < k).collect()
}

#[derive(Clone, Copy, PartialEq)]
enum Role {
    Kick,
    Snare,
    Hat,
    Perc,
}

/// A hit placed in the loop: when (samples), which hit, how loud, how fast it plays (pitch).
struct Event {
    at: usize,
    hit: usize,
    velocity: f64,
    rate: f64,
}

fn shifted(hit: &Stereo, rate: f64) -> Stereo {
    if (rate - 1.0).abs() < 1e-9 {
        return hit.clone();
    }
    let len = (hit[0].len() as f64 / rate) as usize;
    std::array::from_fn(|c| (0..len).map(|i| read_at(&hit[c], i as f64 * rate) as f32).collect())
}

pub(super) fn looped(sources: &[Prepared], options: &Options, mut rng: Rng) -> (Stereo, String, Ending) {
    let bpm = options.bpm.unwrap_or(120.0);
    let bars = options.bars.unwrap_or(2) as usize;
    let step = 60.0 / bpm / 4.0 * RATE;
    let steps = 16 * bars;
    let len = (step * steps as f64).round() as usize;
    let mut hits = slice(sources, ((2.0 * 4.0 * step) as usize).min((1.5 * RATE) as usize));
    // Roles by colour, drawn from the seed among the likeliest: the lowest hit kicks, the brightest ticks, the noisiest
    // mid hit snaps; a hit used twice is filtered into its second role.
    let score = |role: Role, c: &analysis::Colour| -> f64 {
        let bright = c.centroid / 10_000.0;
        match role {
            Role::Kick => 3.0 * c.low - bright - 0.5 * c.flatness,
            Role::Hat => 2.0 * bright + c.flatness - 3.0 * c.low - c.decay.min(1.0),
            Role::Snare => c.flatness + 0.5 - (bright - 0.25).abs() * 2.0 - c.low,
            Role::Perc => 0.0,
        }
    };
    let mut used = vec![false; hits.len()];
    let pick = |role: Role, rng: &mut Rng, hits: &mut Vec<Hit>, used: &mut Vec<bool>| -> usize {
        let scores: Vec<f64> = hits.iter().enumerate().map(|(i, h)| score(role, &h.colour) - if used[i] { 3.0 } else { 0.0 }).collect();
        let top = scores.iter().copied().fold(f64::MIN, f64::max);
        let odds: Vec<f64> = scores.iter().map(|s| ((s - top) / 0.15).exp()).collect();
        let chosen = rng.weighted(&odds);
        if used[chosen] {
            // A second role for a hit: filtered (and pitched) into it.
            let mut audio = hits[chosen].audio.clone();
            match role {
                Role::Hat => fx::filter(&mut audio, || high_pass(RATE, 6000.0)),
                Role::Snare => {
                    audio = shifted(&audio, 2f64.powf(3.0 / 12.0));
                    fx::filter(&mut audio, || high_pass(RATE, 200.0));
                }
                _ => {
                    audio = shifted(&audio, 2f64.powf(-5.0 / 12.0));
                    fx::filter(&mut audio, || low_pass(RATE, 250.0));
                }
            }
            let colour = analysis::colour(&analysis::mono(&audio), 0);
            hits.push(Hit { audio, colour, source: hits[chosen].source });
            used.push(true);
            return hits.len() - 1;
        }
        used[chosen] = true;
        chosen
    };
    let kick = pick(Role::Kick, &mut rng, &mut hits, &mut used);
    let hat = pick(Role::Hat, &mut rng, &mut hits, &mut used);
    let snare = pick(Role::Snare, &mut rng, &mut hits, &mut used);
    let mut percs: Vec<usize> = (0..hits.len()).filter(|i| !used[*i]).collect();
    if percs.is_empty() {
        percs = (0..hits.len()).filter(|i| ![kick, hat, snare].contains(i)).collect();
    }
    if percs.is_empty() {
        percs = vec![snare];
    }
    let order = rng.shuffled(percs.len());
    let percs: Vec<usize> = order.into_iter().take(3).map(|i| percs[i]).collect();
    let swing = rng.range(0.0, 0.25);
    let place = |s: usize, rng: &mut Rng| -> usize {
        let late = if s % 2 == 1 { swing * step } else { 0.0 };
        ((s as f64 * step + late + rng.range(-0.001, 0.001) * RATE).max(0.0) as usize).min(len - 1)
    };
    let mut events: Vec<(Role, Event)> = Vec::new();
    let kicks = [3usize, 4, 4, 5][rng.below(4)];
    let kick_pattern = euclid(kicks, 16, 0);
    let backbeat = rng.chance(0.75);
    let snare_pattern = if backbeat { (0..16).map(|s| s == 4 || s == 12).collect() } else { euclid(2 + rng.below(2), 16, 4) };
    let hat_style = rng.weighted(&[0.35, 0.25, 0.4]);
    let hat_hits = 5 + rng.below(7);
    let hat_pattern: Vec<bool> = match hat_style {
        0 => (0..16).map(|s| s % 2 == 0).collect(),
        1 => vec![true; 16],
        _ => euclid(hat_hits, 16, rng.below(4)),
    };
    let perc_hits = 3 + rng.below(5);
    let perc_pattern = euclid(perc_hits, 16, rng.below(16));
    let intervals = [0.0, 0.0, 3.0, -5.0, 7.0, -12.0, 12.0];
    for bar in 0..bars {
        let pickup = bar > 0 && rng.chance(0.4);
        for s in 0..16 {
            let at_step = bar * 16 + s;
            let mut kick_here = kick_pattern[s] || (pickup && s == 14);
            if bar == bars - 1 && s > 8 && rng.chance(0.06) {
                kick_here = !kick_here && s % 2 == 0;
            }
            if kick_here {
                events.push((Role::Kick, Event { at: place(at_step, &mut rng), hit: kick, velocity: rng.range(0.9, 1.0), rate: 1.0 }));
            }
            if snare_pattern[s] {
                events.push((Role::Snare, Event { at: place(at_step, &mut rng), hit: snare, velocity: rng.range(0.85, 1.0), rate: 1.0 }));
            } else if !kick_here && rng.chance(0.06) {
                events.push((Role::Snare, Event { at: place(at_step, &mut rng), hit: snare, velocity: rng.range(0.25, 0.4), rate: 1.0 }));
            }
            if hat_pattern[s] {
                let accent = if s % 4 == 0 {
                    0.95
                } else if s % 2 == 0 {
                    0.75
                } else {
                    0.55
                };
                events.push((
                    Role::Hat,
                    Event { at: place(at_step, &mut rng), hit: hat, velocity: accent * rng.range(0.85, 1.05), rate: 1.0 },
                ));
            }
            if perc_pattern[s] {
                let hit = percs[rng.below(percs.len())];
                let rate = 2f64.powf(intervals[rng.below(intervals.len())] / 12.0);
                events.push((Role::Perc, Event { at: place(at_step, &mut rng), hit, velocity: rng.range(0.6, 0.95), rate }));
            }
        }
    }
    // Each role's hits cut the one before (a closing hat), percussion rings on.
    let gains = |role: Role| match role {
        Role::Kick => 1.0,
        Role::Snare => 0.8,
        Role::Hat => 0.35,
        Role::Perc => 0.45,
    };
    let mut out = fx::silence(len + (2.0 * RATE) as usize);
    for (index, (role, event)) in events.iter().enumerate() {
        let mut audio = shifted(&hits[event.hit].audio, event.rate);
        if *role != Role::Perc {
            let next = events[index + 1..]
                .iter()
                .find(|(r, _)| r == role)
                .map_or(event.at + len, |(_, e)| e.at + if e.at < event.at { len } else { 0 });
            let room = next.saturating_sub(event.at).max(64);
            if audio[0].len() > room {
                for c in audio.iter_mut() {
                    c.truncate(room);
                }
                fx::fade_out(&mut audio, (0.004 * RATE) as usize);
            }
        }
        let g = gains(*role) * event.velocity;
        fx::mix_into(&mut out, &audio, event.at, [g, g]);
    }
    fx::wrap_tail(&mut out, len);
    let name = |hit: usize| short(&sources[hits[hit].source].name);
    let mut perc_names: Vec<String> = percs.iter().map(|h| name(*h)).collect();
    perc_names.dedup();
    let recipe = format!(
        "Loop, {} bar{} at {} BPM, {:.0}% swing: kick from {} (E({},16)), snare from {}, hats from {} ({}), perc from {} (E({},16))",
        bars,
        if bars == 1 { "" } else { "s" },
        (bpm * 100.0).round() / 100.0,
        swing * 100.0,
        name(kick),
        kicks,
        name(snare),
        name(hat),
        match hat_style {
            0 => "8ths".to_string(),
            1 => "16ths".to_string(),
            _ => format!("E({hat_hits},16)"),
        },
        perc_names.join(" + "),
        perc_hits
    );
    (out, recipe, Ending::Loop)
}

pub(super) fn smear(sources: &[Prepared], options: &Options, mut rng: Rng, progress: &mut dyn FnMut(f64)) -> (Stereo, String, Ending) {
    let n = sources.len();
    let stretch = options.stretch.unwrap_or_else(|| (rng.range(4f64.ln(), 16f64.ln())).exp());
    let mean = sources.iter().map(Prepared::len).sum::<usize>() as f64 / n as f64;
    let len = options.seconds.map_or(mean * stretch, |s| s * RATE).clamp(2.0 * RATE, super::MAX_SECONDS * RATE) as usize;
    let diffusion = options.depth.unwrap_or_else(|| rng.range(0.35, 0.8));
    // The grains run through 80% of the length; the reverb's tail carries the rest.
    let body = (len as f64 * 0.8) as usize;
    let in_turn = rng.chance(0.5);
    let density = rng.range(30.0, 80.0);
    let grain_seconds = rng.range(0.08, 0.3);
    let cents = rng.range(5.0, 50.0);
    let octaves = rng.range(0.0, 0.25);
    let spread = rng.range(0.5, 1.0);
    let jitter = rng.range(0.02, 0.12);
    let maps: Vec<Map> = sources.iter().map(map).collect();
    let count = (body as f64 / RATE * density) as usize;
    let mut grains = Vec::with_capacity(count);
    for g in 0..count {
        let at = ((g as f64 + rng.range(-0.5, 0.5)) / density * RATE).max(0.0) as usize;
        let t = at as f64 / body as f64;
        // Each source scans its whole length over the smear; in turn, each leads in its own stretch of it.
        let shares: Vec<f64> = (0..n)
            .map(|i| {
                if in_turn {
                    let centre = (i as f64 + 0.5) / n as f64;
                    (-((t - centre) * n as f64 * 1.2).powi(2)).exp() + 0.05
                } else {
                    1.0
                }
            })
            .collect();
        let source = rng.weighted(&shares);
        let source_len = sources[source].len() as f64;
        let from = (t * source_len + rng.range(-jitter, jitter) * RATE).clamp(0.0, (source_len - 1.0).max(0.0));
        let m = &maps[source];
        let local = m.env.get(from as usize / 64).copied().unwrap_or(0.0) as f64;
        let gain = (m.rms / local.max(m.rms * 0.1)).powf(0.3) / (density * grain_seconds).sqrt();
        let semitones = if rng.chance(octaves) { [-12.0, 12.0][rng.below(2)] } else { rng.range(-cents, cents) / 100.0 };
        grains.push(Grain {
            source,
            at,
            from,
            len: ((grain_seconds * rng.range(0.7, 1.3)) * RATE).max(64.0) as usize,
            rate: 2f64.powf(semitones / 12.0),
            gain,
            pan: rng.range(-spread, spread),
        });
    }
    progress(0.4);
    let mut dry = render_grains(&grains, sources, len);
    for c in dry.iter_mut() {
        c.truncate(len);
    }
    fx::fade_out(&mut dry, (len - body).max(1));
    progress(0.6);
    let t60 = rng.range(2.5, 8.0) * (0.6 + diffusion * 0.6);
    let wash = fx::reverb(&dry, t60, rng.range(1.0, 2.0), rng.range(0.2, 0.6), &mut rng, 0);
    let wet = 0.25 + 0.5 * diffusion;
    let mut out = fx::silence(len);
    fx::mix_into(&mut out, &dry, 0, [1.0 - wet * 0.5, 1.0 - wet * 0.5]);
    fx::mix_into(&mut out, &wash, 0, [wet, wet]);
    fx::fade_in(&mut out, (0.05 * RATE) as usize);
    let names: Vec<String> = sources.iter().map(|s| short(&s.name)).collect();
    let recipe = format!(
        "Smear of {} at {:.1}× ({:.0} ms grains, ±{:.0} cents{}): {} {}; {:.1} s tail",
        n,
        stretch,
        grain_seconds * 1000.0,
        cents,
        if octaves > 0.1 { ", octaves" } else { "" },
        if in_turn { "in turn" } else { "together" },
        names.join(", "),
        t60
    );
    (out, recipe, Ending::OneShot { fade: (len / 4).min((3.0 * RATE) as usize) })
}
