//! Sound design, measured. One sound: its harmonics (warmth: the 2nd and 3rd against the fundamental; how even
//! harmonics weigh against odd ones), its width, its bandwidth, its noise floor and its tail. A kit (several sounds that
//! should belong together): whether they share a noise floor, bandwidth and grit, whether they cover the spectrum
//! without gaps or pile-ups, whether their decays fit the tempo, and whether each is distinct. Two parts together: how
//! deep one ducks under the other and how fast it comes back, whether their hits interlock or collide, and whether a
//! bass note sits off the key. And the problems: clicks at note edges, a note jumping out.

use super::measure::{band_edges, fine_hz, percentile, Heard, FINE_BINS};
use serde::{Deserialize, Serialize};

/// A pitched sound's harmonics: its fundamental (Hz), the 2nd and 3rd against it (dB: warmth), and even harmonics
/// against odd ones (dB: above 0 the even, tube-like ones lead).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Harmonics {
    pub fundamental: f64,
    pub warmth: f64,
    pub even_odd: f64,
}

/// The loud frames' mean power in each third-octave and each fine bin.
fn spectrum(heard: &Heard) -> Option<(Vec<f64>, Vec<f64>)> {
    let frames = &heard.frames;
    let mut levels: Vec<f64> = frames.level.iter().map(|level| *level as f64).collect();
    if levels.is_empty() {
        return None;
    }
    levels.sort_by(f64::total_cmp);
    let loud = percentile(&levels, 0.95) - 20.;
    let picked: Vec<usize> = (0..frames.level.len()).filter(|frame| frames.level[*frame] as f64 >= loud).collect();
    if picked.is_empty() {
        return None;
    }
    let bands: Vec<f64> = (0..31)
        .map(|band| {
            picked.iter().map(|frame| (frames.mid[*frame][band] + frames.side[*frame][band]) as f64).sum::<f64>() / picked.len() as f64
        })
        .collect();
    let fine: Vec<f64> = (0..FINE_BINS)
        .map(|bin| {
            picked.iter().filter_map(|frame| frames.fine.get(*frame)).map(|row| 10f64.powf(row[bin] as f64 / 10.)).sum::<f64>()
                / picked.len() as f64
        })
        .collect();
    Some((bands, fine))
}

/// The power at a frequency: from the fine spectrum where it reaches (a semitone's resolution), else the
/// third-octave holding it.
fn power_at(hz: f64, bands: &[f64], fine: &[f64]) -> Option<f64> {
    if hz >= fine_hz(0) && hz <= fine_hz(FINE_BINS - 1) {
        let bin = (12. * (hz / fine_hz(0)).log2()).round() as usize;
        return fine.get(bin).copied();
    }
    let band = (0..31).find(|band| {
        let (low, high) = band_edges(*band);
        hz >= low && hz < high
    })?;
    bands.get(band).copied()
}

/// The harmonics of a pitched sound, when it has a steady pitch: the bass line's for a low sound, else the lowest fine
/// peak standing 10 dB over the spectrum's middle.
pub fn harmonics(heard: &Heard) -> Option<Harmonics> {
    let (bands, fine) = spectrum(heard)?;
    let mut pitches: Vec<f64> = heard.frames.bass.iter().flatten().map(|(hz, _)| *hz as f64).collect();
    let fundamental = if pitches.len() * 2 >= heard.frames.bass.len().max(1) {
        pitches.sort_by(f64::total_cmp);
        percentile(&pitches, 0.5)
    } else {
        let mut sorted = fine.clone();
        sorted.sort_by(f64::total_cmp);
        let middle = sorted[sorted.len() / 2];
        let bin = (1..FINE_BINS - 1).find(|bin| fine[*bin] > middle * 10. && fine[*bin] >= fine[bin - 1] && fine[*bin] >= fine[bin + 1])?;
        fine_hz(bin)
    };
    let level = |k: f64| power_at(fundamental * k, &bands, &fine);
    let first = level(1.)?;
    let (second, third) = (level(2.)?, level(3.)?);
    let db = |power: f64| 10. * (power + 1e-20).log10();
    let even: f64 = [2., 4., 6.].iter().filter_map(|k| level(*k)).sum();
    let odd: f64 = [3., 5., 7.].iter().filter_map(|k| level(*k)).sum();
    Some(Harmonics {
        fundamental: fundamental.round(),
        warmth: round1(db(second + third) - db(first)),
        even_odd: round1(db(even) - db(odd)),
    })
}

/// How wide it is: its side against its mid over the loud frames, dB (−30 and under is mono).
pub fn width(heard: &Heard) -> Option<f64> {
    let frames = &heard.frames;
    let (mid, side) = frames.mid.iter().zip(&frames.side).fold((0., 0.), |(mid, side), (m, s)| {
        (mid + m.iter().map(|v| *v as f64).sum::<f64>(), side + s.iter().map(|v| *v as f64).sum::<f64>())
    });
    (mid > 1e-14).then(|| round1((10. * ((side + 1e-20) / mid).log10()).max(-60.)))
}

/// Where it stops: the top of the highest third-octave within 30 dB of its loudest, Hz (a lo-fi sound's is low).
pub fn bandwidth(heard: &Heard) -> Option<f64> {
    let (bands, _) = spectrum(heard)?;
    let loudest = bands.iter().copied().fold(0., f64::max);
    if loudest <= 0. {
        return None;
    }
    let top = (0..31).rev().find(|band| bands[*band] >= loudest * 1e-3)?;
    Some(band_edges(top).1.round())
}

/// The noise floor: its quietest frames (the 5th percentile) under its loud ones (the 95th), dB.
pub fn noise_floor(heard: &Heard) -> Option<f64> {
    let mut levels: Vec<f64> = heard.frames.level.iter().map(|level| *level as f64).filter(|level| level.is_finite()).collect();
    if levels.len() < 20 {
        return None;
    }
    levels.sort_by(f64::total_cmp);
    Some(round1(percentile(&levels, 0.05) - percentile(&levels, 0.95)))
}

/// Space: how far the level stands under each hit's peak 300 ms on (the median over hits, dB): a dry hit falls far,
/// a reverberant one hangs on.
pub fn tail(heard: &Heard) -> Option<f64> {
    let frames = &heard.frames;
    let steps = (0.3 / frames.hop.max(1e-9)).round() as usize;
    let level: Vec<f64> = frames.level.iter().map(|level| *level as f64).collect();
    let mut tails = vec![];
    let mut at = 2;
    while at + steps < level.len() {
        if level[at] - level[at - 2] >= 10. {
            let peak = level[at..(at + 5).min(level.len())].iter().copied().fold(f64::MIN, f64::max);
            tails.push(level[at + steps] - peak);
            at += steps;
        } else {
            at += 1;
        }
    }
    tails.sort_by(f64::total_cmp);
    (!tails.is_empty()).then(|| round1(percentile(&tails, 0.5)))
}

/// The hits' peak levels (dB), each a rise of 10 dB within two frames.
fn hits(heard: &Heard) -> Vec<(usize, f64)> {
    let level: Vec<f64> = heard.frames.level.iter().map(|level| *level as f64).collect();
    let mut found = vec![];
    let mut at = 2;
    while at < level.len() {
        if level[at] - level[at - 2] >= 10. {
            let end = (at + 5).min(level.len());
            let peak = level[at..end].iter().copied().fold(f64::MIN, f64::max);
            found.push((at, peak));
            at = end;
        } else {
            at += 1;
        }
    }
    found
}

/// What's wrong with one sound: clicks at its note edges, DC, a note jumping out of the rest.
pub fn problems(heard: &Heard) -> Vec<String> {
    let mut found = vec![];
    let m = &heard.measures;
    if m.clicks > 0 {
        found.push(format!(
            "{} notes start with a click (a jump out of silence: a fade-in of a few milliseconds would smooth it)",
            m.clicks
        ));
    }
    let dc = m.dc[0].abs().max(m.dc[1].abs());
    if dc >= 0.01 {
        found.push(format!("a DC offset of {:.1} % of full scale", dc * 100.));
    }
    let peaks: Vec<f64> = hits(heard).iter().map(|(_, peak)| *peak).collect();
    if peaks.len() >= 4 {
        let mut sorted = peaks.clone();
        sorted.sort_by(f64::total_cmp);
        let middle = percentile(&sorted, 0.5);
        let jumping = peaks.iter().filter(|peak| **peak - middle >= 6.).count();
        if jumping > 0 && jumping * 4 <= peaks.len() {
            found.push(format!("{jumping} of {} notes jump out of the rest by 6 dB and more", peaks.len()));
        }
    }
    found
}

/// One piece of a kit, as measured.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Piece {
    pub name: String,
    pub noise_floor: Option<f64>,
    pub bandwidth: Option<f64>,
    pub distortion: Option<f64>,
    /// Its decay (ms) against a beat: under 1 it rings out before the next beat.
    pub decay_beats: Option<f64>,
    pub centroid: Option<f64>,
}

/// A kit's measures and what's wrong with it, given each piece's sound (heard alone) and the tempo.
pub fn kit(pieces: &[(String, Heard)], tempo: f64) -> (Vec<Piece>, Vec<String>) {
    let beat_ms = 60_000. / tempo.max(1.);
    let measured: Vec<Piece> = pieces
        .iter()
        .map(|(name, heard)| Piece {
            name: name.clone(),
            noise_floor: noise_floor(heard),
            bandwidth: bandwidth(heard),
            distortion: heard.measures.distortion,
            decay_beats: heard.measures.decay.map(|ms| round1(ms / beat_ms)),
            centroid: heard.measures.centroid,
        })
        .collect();
    let mut found = vec![];
    // Cohesion: one piece far off the others in noise floor, bandwidth or grit.
    let odd_one = |values: Vec<(String, f64)>, by: f64, what: &str, found: &mut Vec<String>| {
        if values.len() < 3 {
            return;
        }
        let mut sorted: Vec<f64> = values.iter().map(|(_, value)| *value).collect();
        sorted.sort_by(f64::total_cmp);
        let middle = percentile(&sorted, 0.5);
        for (name, value) in &values {
            if (value - middle).abs() >= by {
                found.push(format!("{name}'s {what} ({}) sits apart from the kit's ({})", number(*value), number(middle)));
            }
        }
    };
    odd_one(
        measured.iter().filter_map(|piece| Some((piece.name.clone(), piece.noise_floor?))).collect(),
        12.,
        "noise floor (dB)",
        &mut found,
    );
    odd_one(
        measured.iter().filter_map(|piece| Some((piece.name.clone(), 12. * (piece.bandwidth? / 1000.).log2()))).collect(),
        12.,
        "bandwidth (semitones above 1 kHz)",
        &mut found,
    );
    odd_one(measured.iter().filter_map(|piece| Some((piece.name.clone(), piece.distortion?))).collect(), 6., "grit (dB)", &mut found);
    // Coverage: a pile-up (three pieces centred within a third of an octave) or a gap (an octave and a half between
    // neighbours' centres).
    let mut centres: Vec<(String, f64)> = measured.iter().filter_map(|piece| Some((piece.name.clone(), piece.centroid?))).collect();
    centres.sort_by(|a, b| a.1.total_cmp(&b.1));
    for window in centres.windows(3) {
        if (window[2].1 / window[0].1).log2() < 1. / 3. {
            found.push(format!("{}, {} and {} pile up around {} Hz", window[0].0, window[1].0, window[2].0, window[1].1.round()));
        }
    }
    for pair in centres.windows(2) {
        if (pair[1].1 / pair[0].1).log2() > 1.5 && pair[0].1 > 60. {
            found.push(format!("nothing between {} ({} Hz) and {} ({} Hz)", pair[0].0, pair[0].1.round(), pair[1].0, pair[1].1.round()));
        }
    }
    // Decays against the tempo: a piece ringing over two beats.
    for piece in &measured {
        if let Some(beats) = piece.decay_beats.filter(|beats| *beats > 2.) {
            found.push(format!("{} rings for {} beats: its decay runs into the next hits", piece.name, number(beats)));
        }
    }
    // Distinct: two pieces whose balance is all but the same.
    for (index, (a, heard_a)) in pieces.iter().enumerate() {
        for (b, heard_b) in pieces.iter().skip(index + 1) {
            let differ = heard_a.measures.balance.iter().zip(&heard_b.measures.balance).map(|(x, y)| (x - y).abs()).sum::<f64>()
                / heard_a.measures.balance.len().max(1) as f64;
            if differ < 1.5 {
                found.push(format!("{a} and {b} sound almost the same"));
            }
        }
    }
    (measured, found)
}

/// How one part ducks under another heard with it (`trigger`): how far its level falls at the trigger's hits (dB) and
/// how long it takes to come back within 1 dB (ms). None without hits to duck under.
pub fn ducking(target: &Heard, trigger: &Heard) -> Option<(f64, f64)> {
    let hop = target.frames.hop;
    let level: Vec<f64> = target.frames.level.iter().map(|level| *level as f64).collect();
    let mut depths = vec![];
    let mut recoveries = vec![];
    for (at, _) in hits(trigger) {
        let before = level[at.saturating_sub(4)..at].iter().copied().fold(f64::MIN, f64::max);
        let window = &level[at..(at + (0.5 / hop) as usize).min(level.len())];
        let Some((low_at, low)) = window.iter().copied().enumerate().min_by(|a, b| a.1.total_cmp(&b.1)) else { continue };
        if before - low < 1. {
            continue;
        }
        depths.push(before - low);
        if let Some(back) = window[low_at..].iter().position(|level| before - level <= 1.) {
            recoveries.push((low_at + back) as f64 * hop * 1000.);
        }
    }
    if depths.len() < 2 {
        return None;
    }
    depths.sort_by(f64::total_cmp);
    recoveries.sort_by(f64::total_cmp);
    let recovery = if recoveries.is_empty() { 500. } else { percentile(&recoveries, 0.5) };
    Some((round1(percentile(&depths, 0.5)), recovery.round()))
}

/// How two parts' hits meet: the share of one's hits landing within 30 ms of the other's (colliding) and the share
/// falling between them (interlocking).
pub fn interlock(a: &Heard, b: &Heard) -> Option<(f64, f64)> {
    let hop = a.frames.hop;
    let (first, second): (Vec<usize>, Vec<usize>) =
        (hits(a).iter().map(|(at, _)| *at).collect(), hits(b).iter().map(|(at, _)| *at).collect());
    if first.len() < 4 || second.len() < 4 {
        return None;
    }
    let near = (0.03 / hop).ceil() as usize;
    let colliding = first.iter().filter(|at| second.iter().any(|other| other.abs_diff(**at) <= near)).count();
    let share = |count: usize| ((count as f64 / first.len() as f64) * 100.).round();
    Some((share(colliding), share(first.len() - colliding)))
}

/// A low part's pitch against a key (its notes as pitch classes, 0 is C): how far its notes sit from the nearest note
/// of the key, cents (median), and the share of its time off the key by over a quarter tone.
pub fn against_key(heard: &Heard, key: &[u8]) -> Option<(f64, f64)> {
    let pitches: Vec<f64> = heard.frames.bass.iter().flatten().map(|(hz, _)| *hz as f64).collect();
    if pitches.len() < 8 || key.is_empty() {
        return None;
    }
    let mut off: Vec<f64> = pitches
        .iter()
        .map(|hz| {
            let midi = 69. + 12. * (hz / 440.).log2();
            key.iter()
                .map(|class| {
                    let class = *class as f64;
                    let nearest = ((midi - class) / 12.).round() * 12. + class;
                    ((midi - nearest) * 100.).abs()
                })
                .fold(f64::MAX, f64::min)
        })
        .collect();
    let clashing = off.iter().filter(|cents| **cents > 50.).count() as f64 / off.len() as f64;
    off.sort_by(f64::total_cmp);
    Some((percentile(&off, 0.5).round(), (clashing * 100.).round()))
}

fn number(value: f64) -> String {
    let rounded = round1(value);
    if rounded == rounded.trunc() {
        format!("{}", rounded as i64)
    } else {
        format!("{rounded:.1}")
    }
}

fn round1(value: f64) -> f64 {
    (value * 10.).round() / 10.
}

/// A key in words ("F# minor", "Bbm", "C", "D dorian") as its notes' pitch classes (0 is C): the major scale, the
/// natural minor or a mode. None when it doesn't read as a key.
pub fn key_classes(key: &str) -> Option<Vec<u8>> {
    const MAJOR: [i32; 7] = [0, 2, 4, 5, 7, 9, 11];
    let words: Vec<String> = key.split_whitespace().map(|word| word.to_lowercase()).collect();
    let mut letters = words.first()?.chars().peekable();
    let root: i32 = match letters.next()? {
        'c' => 0,
        'd' => 2,
        'e' => 4,
        'f' => 5,
        'g' => 7,
        'a' => 9,
        'b' => 11,
        _ => return None,
    };
    let mut shift = 0;
    while let Some(accidental) = letters.next_if(|c| matches!(c, '#' | '♯' | 'b' | '♭')) {
        shift += if matches!(accidental, '#' | '♯') { 1 } else { -1 };
    }
    // What follows the note, joined or as the next word: nothing or "maj" is major, "m" or "min" minor, or a mode.
    let rest: String = letters.collect();
    let quality = if rest.is_empty() { words.get(1).cloned().unwrap_or_default() } else { rest };
    let mode = match quality.as_str() {
        "" => 0,
        q if q.starts_with("maj") || q.starts_with("ion") => 0,
        q if q.starts_with("dor") => 1,
        q if q.starts_with("phr") => 2,
        q if q.starts_with("lyd") => 3,
        q if q.starts_with("mix") => 4,
        q if q.starts_with('m') || q.starts_with("aeo") => 5,
        q if q.starts_with("loc") => 6,
        _ => return None,
    };
    Some((0..7).map(|step| (root + shift + MAJOR[(mode + step) % 7] - MAJOR[mode]).rem_euclid(12) as u8).collect())
}
