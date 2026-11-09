//! A melodic loop: the picked sounds repitched into a key, a scale and a tuning, and played on the loop's grid. Each
//! note is drawn from the tonal sounds (with none, every sound, each tuned by its brightness), moving through the
//! scale as its contour says, with chords, glides, notes bent off the scale and a held root as asked. A motif of one
//! cycle repeats over the bars, varied as `variation` says. Every choice left out is drawn from the seed, so a reroll is
//! another melody.

use super::{
    analysis::{self, read_at, Stereo},
    choices::{key_index, Contour, Scale, Tuning, KEY_NAMES},
    fuse::{drawn, euclid, Grid},
    fx,
    rng::Rng,
    short, steer, Ending, Options, Prepared, RATE,
};

/// A sound the melody plays: its audio and the pitch it sounds at.
struct Voice {
    audio: Stereo,
    hz: f64,
    tonal: bool,
}

fn voices(sources: &[Prepared]) -> Vec<Voice> {
    sources
        .iter()
        .map(|source| {
            let found = analysis::pitch(&source.mono, source.peak);
            let hz = found.unwrap_or_else(|| {
                // No clear pitch: a note near half its brightness, as modulate's FM tunes one.
                let colour = analysis::colour(&source.mono, 0);
                440.0 * 2f64.powf((12.0 * (colour.centroid.max(60.0) / 2.0 / 440.0).log2()).round() / 12.0)
            });
            Voice { audio: source.audio.clone(), hz, tonal: found.is_some() }
        })
        .collect()
}

/// The scale's degrees in cents, each moved to the nearest pitch the tuning has, and the tuning's period (an octave,
/// or Bohlen–Pierce's tritave, which the scale is spread across).
fn degrees(scale: Scale, tuning: Tuning) -> (Vec<f64>, f64) {
    let (grid, period) = tuning.grid();
    let mut out: Vec<f64> = scale
        .cents()
        .iter()
        .map(|cents| {
            let wanted = cents * period / 1200.0;
            grid.iter().copied().min_by(|a, b| (a - wanted).abs().total_cmp(&(b - wanted).abs())).unwrap_or(wanted)
        })
        .collect();
    out.dedup_by(|a, b| (*a - *b).abs() < 1e-6);
    (out, period)
}

/// A note: when it starts (a step of the whole loop), the scale degrees it sounds (a chord has several), how loud,
/// how far it's bent off the scale (cents), whether it slides in from the note before, and which sound plays it.
struct Note {
    step: usize,
    degrees: Vec<i64>,
    velocity: f64,
    bend: f64,
    glide: bool,
    voice: usize,
}

/// `audio` played at `rate` (sliding in from `from` over its first `glide` samples) for `len` samples, faded in and
/// out so it never clicks.
fn play(audio: &Stereo, rate: f64, from: Option<f64>, glide: usize, len: usize) -> Stereo {
    let mut out: Stereo = [vec![0.0; len], vec![0.0; len]];
    let mut at = 0.0f64;
    let end = audio[0].len() as f64;
    for i in 0..len {
        if at >= end {
            break;
        }
        for c in 0..2 {
            out[c][i] = read_at(&audio[c], at) as f32;
        }
        at += match from {
            Some(start) if i < glide => {
                let t = i as f64 / glide as f64;
                start + (rate - start) * t * t * (3.0 - 2.0 * t)
            }
            _ => rate,
        };
    }
    fx::fade_in(&mut out, 64.min(len / 4));
    fx::fade_out(&mut out, ((0.015 * RATE) as usize).min(len / 3));
    out
}

pub(super) fn melody(sources: &[Prepared], options: &Options, mut rng: Rng) -> (Stereo, String, Ending) {
    let choices = options.r#loop.clone().unwrap_or_default();
    let g = Grid::of(options);
    let voices = voices(sources);
    let tonal: Vec<usize> = (0..voices.len()).filter(|&i| voices[i].tonal).collect();
    let pool: Vec<usize> = if tonal.is_empty() { (0..voices.len()).collect() } else { tonal.clone() };
    // The key: as given; else, half the time, the note of the clearest-pitched sound; else any.
    let key = match choices.key.as_deref().and_then(key_index) {
        Some(key) => key,
        None => match tonal.first() {
            Some(&i) if rng.chance(0.5) => ((12.0 * (voices[i].hz / 440.0).log2()).round() as i64 + 9).rem_euclid(12) as usize,
            _ => rng.below(12),
        },
    };
    let common = [Scale::Major, Scale::Minor, Scale::Dorian, Scale::Pentatonic, Scale::MinorPentatonic, Scale::Mixolydian];
    let scale = choices.scale.unwrap_or_else(|| {
        if rng.chance(0.8) {
            common[rng.below(common.len())]
        } else {
            Scale::ALL[rng.below(Scale::ALL.len())]
        }
    });
    let tuning = choices.tuning.unwrap_or(Tuning::Tet12);
    let (cents, period) = degrees(scale, tuning);
    let octaves = choices.octaves.unwrap_or_else(|| 1 + rng.below(2) as u32) as usize;
    let register = choices.register.unwrap_or(0);
    let contour = choices.contour.unwrap_or_else(|| Contour::ALL[rng.below(Contour::ALL.len())]);
    let density = drawn(choices.density, 0.3, 0.6, &mut rng);
    let swing = drawn(choices.swing, 0.0, 0.2, &mut rng);
    let jitter = 0.001 + 0.014 * choices.humanize.unwrap_or(0.1);
    let accent = drawn(choices.accent, 0.2, 0.5, &mut rng);
    let chords = choices.chords.unwrap_or_else(|| if rng.chance(0.3) { rng.range(0.1, 0.4) } else { 0.0 });
    let glide = choices.glide.unwrap_or_else(|| if rng.chance(0.3) { rng.range(0.1, 0.5) } else { 0.0 });
    let gate = drawn(choices.gate, 0.45, 0.9, &mut rng);
    let drone = choices.drone.unwrap_or_else(|| if rng.chance(0.25) { rng.range(0.2, 0.5) } else { 0.0 });
    let micro = choices.micro.unwrap_or(0.0);
    let variation = drawn(choices.variation, 0.1, 0.35, &mut rng);
    let fills = choices.fills.unwrap_or(0.0);
    let ratchet = choices.ratchet.unwrap_or(0.0);
    let poly = choices.poly.unwrap_or(0.0);
    let root = 261.63 * 2f64.powf(key as f64 / 12.0) * 2f64.powi(register);
    let per = cents.len() as i64;
    let top = per * octaves as i64;
    let hz_of = |degree: i64, bend: f64| -> f64 {
        let (octave, step) = (degree.div_euclid(per), degree.rem_euclid(per));
        root * 2f64.powf((cents[step as usize] + octave as f64 * period + bend) / 1200.0)
    };
    // The motif's cycle: a bar, or (polymeter) a step short or long of it, or a beat and a half.
    let steps = g.steps;
    let cycle = if rng.chance(poly) { [steps - 1, steps + 1, (3 * g.grid).div_ceil(2)][rng.below(3)].max(2) } else { steps };
    let rhythm = euclid(((cycle as f64 * density).round() as usize).clamp(1, cycle), cycle, rng.below(cycle));
    // The contour walked once over the cycle's notes.
    let mut degree = rng.below(per as usize) as i64;
    let mut rising = true;
    let mut next = |degree: i64, rng: &mut Rng| -> i64 {
        let reflect = |d: i64| {
            if d < 0 {
                -d
            } else if d > top {
                2 * top - d
            } else {
                d
            }
        };
        match contour {
            Contour::Up => (degree + 1).rem_euclid(top + 1),
            Contour::Down => (degree - 1).rem_euclid(top + 1),
            Contour::UpDown => {
                if (rising && degree >= top) || (!rising && degree <= 0) {
                    rising = !rising;
                }
                degree + if rising { 1 } else { -1 }
            }
            Contour::Walk => reflect(degree + [-2, -1, 1, 2][rng.below(4)]),
            Contour::Leaps => reflect(degree + [-5, -4, -3, 3, 4, 5][rng.below(6)]),
            Contour::Random => rng.below(top as usize + 1) as i64,
        }
    };
    let mut motif = vec![0i64; cycle];
    for (s, slot) in motif.iter_mut().enumerate() {
        if rhythm[s] {
            *slot = degree;
            degree = next(degree, &mut rng);
        }
    }
    let total = steps * g.bars;
    let fill_from = total.saturating_sub(g.grid * if fills > 0.6 { 2 } else { 1 });
    let mut notes: Vec<Note> = Vec::new();
    let mut last = motif[0];
    for s in 0..total {
        let filling = s >= fill_from && rng.chance(fills);
        if !rhythm[s % cycle] && !filling {
            continue;
        }
        let mut sounded = if filling && !rhythm[s % cycle] { next(last, &mut rng) } else { motif[s % cycle] };
        if s >= cycle && rng.chance(variation) {
            sounded = next(sounded, &mut rng);
        }
        let mut chord = vec![sounded];
        if rng.chance(chords) {
            chord.push(sounded + 2);
            if rng.chance(0.5) {
                chord.push(sounded + 4);
            }
        }
        let bend = if rng.chance(micro) { tuning.step() * if rng.chance(0.5) { 1.0 } else { -1.0 } } else { 0.0 };
        let velocity = g.accent(s % steps, accent) * rng.range(0.85, 1.0);
        notes.push(Note {
            step: s,
            degrees: chord,
            velocity,
            bend,
            glide: !notes.is_empty() && rng.chance(glide),
            voice: pool[rng.below(pool.len())],
        });
        last = sounded;
        if rng.chance(ratchet * 0.5) {
            // A ratchet: the note again, quickly, within its step (placed as a half step on).
            let repeat =
                Note { step: s, degrees: vec![sounded], velocity: velocity * 0.8, bend, glide: false, voice: notes[notes.len() - 1].voice };
            notes.push(repeat);
        }
    }
    let len = g.len;
    let mut out = fx::silence(len + (2.0 * RATE) as usize);
    let starts: Vec<usize> = notes.iter().map(|note| g.place(note.step, swing, jitter, &mut rng)).collect();
    let mut previous_hz: Option<f64> = None;
    for (i, note) in notes.iter().enumerate() {
        // A ratchet's repeat comes half a step after its note.
        let at = if i > 0 && notes[i - 1].step == note.step { starts[i] + (g.step / 2.0) as usize } else { starts[i] };
        let following = notes[i + 1..].iter().position(|n| n.step != note.step).map(|j| starts[i + 1 + j]).unwrap_or(starts[0] + len);
        let room = following.saturating_sub(at).max((0.04 * RATE) as usize);
        let sounding = ((room as f64 * gate) as usize).max((0.04 * RATE) as usize);
        let voice = &voices[note.voice];
        let gain = note.velocity * 0.6 / (note.degrees.len() as f64).sqrt();
        for (k, &degree) in note.degrees.iter().enumerate() {
            let hz = hz_of(degree, note.bend);
            let from = if note.glide && k == 0 { previous_hz.map(|p| p / voice.hz) } else { None };
            let audio = play(&voice.audio, hz / voice.hz, from, (0.06 * RATE) as usize, sounding);
            fx::mix_into(&mut out, &audio, at, [gain, gain]);
        }
        previous_hz = Some(hz_of(note.degrees[0], note.bend));
    }
    if drone > 0.0 {
        // The root an octave down, held: the longest tonal sound stretched over the whole loop, its end flowing into
        // its start.
        let longest = *pool.iter().max_by_key(|&&i| voices[i].audio[0].len()).unwrap_or(&0);
        let voice = &voices[longest];
        let fade = ((0.1 * RATE) as usize).min(len / 4);
        let natural = play(&voice.audio, hz_of(-per, 0.0) / voice.hz, None, 0, len + fade);
        let heard = natural[0].iter().rposition(|v| v.abs() > 1e-4).unwrap_or(0).max(1);
        let factor = ((len + fade) as f64 / heard as f64).clamp(1.0, 16.0);
        let mut held: Stereo = std::array::from_fn(|c| {
            let mut channel = steer::stretch(&natural[c][..heard], factor);
            channel.resize(len + fade, 0.0);
            channel
        });
        fx::loop_crossfade(&mut held, len, fade);
        fx::mix_into(&mut out, &held, 0, [drone * 0.5, drone * 0.5]);
    }
    fx::wrap_tail(&mut out, len);
    let mut names: Vec<String> = pool.iter().map(|&i| short(&sources[i].name)).collect();
    names.truncate(4);
    let contour_words = match contour {
        Contour::Up => "rising",
        Contour::Down => "falling",
        Contour::UpDown => "up and down",
        Contour::Walk => "walking",
        Contour::Leaps => "leaping",
        Contour::Random => "scattered",
    };
    let mut extras = Vec::new();
    for (on, word) in [
        (chords >= 0.2, "chords"),
        (glide >= 0.2, "glides"),
        (micro > 0.0, "notes bent off the scale"),
        (drone > 0.0, "a held root"),
        (fills >= 0.3, "fills"),
        (ratchet > 0.0, "ratchets"),
        (cycle != steps, "a motif across the bar line"),
    ] {
        if on {
            extras.push(word);
        }
    }
    let tuned = if tuning == Tuning::Tet12 { String::new() } else { format!(", {}", tuning.name()) };
    let recipe = format!(
        "Melody, {}, {:.0}% swing, in {} {}{tuned}: {contour_words} over {} octave{}, notes from {}{}",
        g.words(),
        swing * 100.0,
        KEY_NAMES[key],
        scale.name(),
        octaves,
        if octaves == 1 { "" } else { "s" },
        names.join(" + "),
        if extras.is_empty() { String::new() } else { format!("; {}", extras.join(", ")) }
    );
    (out, recipe, Ending::Loop)
}
