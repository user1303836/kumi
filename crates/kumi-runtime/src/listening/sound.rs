//! Sound design, measured. One sound: its harmonics (warmth: the 2nd and 3rd against the fundamental; how even
//! harmonics weigh against odd ones), its width, its bandwidth, its noise floor and its tail. A kit (several sounds that
//! should belong together): whether they share a noise floor, bandwidth and grit, whether they cover the spectrum
//! without gaps or pile-ups, whether their decays fit the tempo, and whether each is distinct. Two parts together: how
//! deep one ducks under the other and how fast it comes back, whether their hits interlock or collide, and whether a
//! bass note sits off the key. And the problems: clicks at note edges, a note jumping out.

use super::measure::{band_edges, fine_hz, percentile, Heard, FINE_BINS, FINE_FROM};
use serde::{Deserialize, Serialize};

/// A pitched sound's harmonics: its fundamental (Hz), the 2nd and 3rd against it (dB: warmth), and even harmonics
/// against odd ones (dB: above 0 the even, tube-like ones lead).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Harmonics {
    pub fundamental: f64,
    pub warmth: f64,
    pub even_odd: f64,
}

/// The loud frames' mean power: each third-octave's (mid and side), each fine bin's density (mid, per FFT bin, as
/// measured) and its mid in all (the density times the FFT bins it holds), and the low stream's own bins (mid, up to
/// `LOW_FINE_TOP`).
struct Spectrum {
    bands: Vec<f64>,
    fine: Vec<f64>,
    totals: Vec<f64>,
    lows: Vec<f64>,
}

fn spectrum(heard: &Heard) -> Option<Spectrum> {
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
    let mean = |value: &dyn Fn(usize) -> f64| picked.iter().map(|frame| value(*frame)).sum::<f64>() / picked.len() as f64;
    let bands: Vec<f64> = (0..31).map(|band| mean(&|frame| (frames.mid[frame][band] + frames.side[frame][band]) as f64)).collect();
    let fine: Vec<f64> =
        (0..FINE_BINS).map(|bin| mean(&|frame| frames.fine.get(frame).map_or(0., |row| 10f64.powf(row[bin] as f64 / 10.)))).collect();
    let counts = fine_counts(frames.hop, heard.measures.sample_rate);
    let totals = fine.iter().zip(counts).map(|(density, count)| density * count as f64).collect();
    // The low stream's frames that fall in loud main frames.
    let low: Vec<&Vec<f32>> = (0..frames.low_fine.len())
        .filter(|low| main_frame(heard, *low).is_some_and(|frame| frames.level[frame] as f64 >= loud))
        .map(|low| &frames.low_fine[low])
        .collect();
    let width = low.iter().map(|row| row.len()).min().unwrap_or(0);
    let lows = (0..width).map(|bin| low.iter().map(|row| row[bin] as f64).sum::<f64>() / low.len() as f64).collect();
    Some(Spectrum { bands, fine, totals, lows })
}

/// How many of the main FFT's bins each fine bin holds, as `measure` sorts them (frames `hop` apart: the FFT is four
/// hops long). A fine bin holding none repeats its neighbour's density, so it counts for nothing here.
fn fine_counts(hop: f64, rate: f64) -> [usize; FINE_BINS] {
    let bin_hz = 1. / (4. * hop.max(1e-9));
    let mut counts = [0; FINE_BINS];
    let mut k = 1;
    while k as f64 * bin_hz <= rate / 2. {
        let bin = (12. * (k as f64 * bin_hz / FINE_FROM).log2() + 0.5).floor();
        if bin >= FINE_BINS as f64 {
            break;
        }
        if bin >= 0. {
            counts[bin as usize] += 1;
        }
        k += 1;
    }
    counts
}

/// A partial's mid power: all of it, wherever it falls. Whole fine bins (semitones) from 250 Hz; under it, the low
/// stream's own bins (about 3 Hz apart, so a low note's neighbouring partials stay apart). The window is a semitone
/// either side, or two FFT bins when that's wider, so a partial on either side of 250 Hz reads in full.
fn partial(hz: f64, heard: &Heard, spectrum: &Spectrum) -> f64 {
    let frames = &heard.frames;
    let fine_edges = |bin: usize| (FINE_FROM * 2f64.powf((bin as f64 - 0.5) / 12.), FINE_FROM * 2f64.powf((bin as f64 + 0.5) / 12.));
    let low_stream = hz - hz * (2f64.powf(1. / 12.) - 1.) < fine_edges(0).0;
    // The low stream's FFT bins are `low_bin_hz` apart; the main one's a quarter of a hop's inverse.
    let bin_hz = if low_stream { frames.low_bin_hz } else { 1. / (4. * frames.hop.max(1e-9)) };
    let reach = (hz * (2f64.powf(1. / 12.) - 1.)).max(2. * bin_hz);
    let (low, high) = (hz - reach, hz + reach);
    if low_stream {
        spectrum.lows.iter().enumerate().filter(|(bin, _)| (low..=high).contains(&(*bin as f64 * bin_hz))).map(|(_, power)| power).sum()
    } else {
        (0..FINE_BINS).filter(|bin| fine_edges(*bin).0 < high && fine_edges(*bin).1 > low).map(|bin| spectrum.totals[bin]).sum()
    }
}

/// A low part's notes, from the low stream's strongest peak in each of its frames (30–250 Hz): kept when it's within
/// 18 dB of the loudest and holds a real share of all that sounds then (within 15 dB of it), each as its low frame and
/// pitch (Hz). None when the part isn't low: under half its loud frames have one (a lead's lows are leakage, not
/// notes).
pub fn bass_line(heard: &Heard) -> Option<Vec<(usize, f64)>> {
    let frames = &heard.frames;
    if frames.level.is_empty() {
        return None;
    }
    let mut levels: Vec<f64> = frames.level.iter().map(|level| *level as f64).collect();
    levels.sort_by(f64::total_cmp);
    let loud = percentile(&levels, 0.95) - 20.;
    let loudest = frames.bass.iter().flatten().map(|(_, db)| *db as f64).fold(f64::MIN, f64::max);
    let (mut sounding, mut notes) = (0, vec![]);
    for (low, note) in frames.bass.iter().enumerate() {
        let Some(level) = main_frame(heard, low).map(|frame| frames.level[frame] as f64).filter(|level| *level >= loud) else {
            continue;
        };
        sounding += 1;
        if let Some((hz, db)) = note.map(|(hz, db)| (hz as f64, db as f64)) {
            if db >= loudest - 18. && db >= level - 15. {
                notes.push((low, hz));
            }
        }
    }
    (notes.len() * 2 > sounding).then_some(notes)
}

/// The main frame centred nearest a low frame's centre.
pub fn main_frame(heard: &Heard, low: usize) -> Option<usize> {
    let frames = &heard.frames;
    let ratio = frames.bass_hop / frames.hop.max(1e-9);
    let frame = ((low + 2) as f64 * ratio - 2.).round().max(0.) as usize;
    (frame < frames.level.len()).then_some(frame)
}

/// The harmonics of a pitched sound, when it has a steady pitch: the bass line's for a low part, else the lowest fine
/// peak standing 10 dB over the spectrum's middle.
pub fn harmonics(heard: &Heard) -> Option<Harmonics> {
    let spectrum = spectrum(heard)?;
    let fundamental = match bass_line(heard) {
        Some(notes) => {
            let mut pitches: Vec<f64> = notes.into_iter().map(|(_, hz)| hz).collect();
            pitches.sort_by(f64::total_cmp);
            percentile(&pitches, 0.5)
        }
        None => {
            let fine = &spectrum.fine;
            let mut sorted = fine.clone();
            sorted.sort_by(f64::total_cmp);
            let middle = sorted[sorted.len() / 2];
            let bin =
                (1..FINE_BINS - 1).find(|bin| fine[*bin] > middle * 10. && fine[*bin] >= fine[bin - 1] && fine[*bin] >= fine[bin + 1])?;
            fine_hz(bin)
        }
    };
    // Up to the fine spectrum's top (about 15 kHz).
    let top = fine_hz(FINE_BINS - 1);
    let level = |k: f64| (fundamental * k <= top).then(|| partial(fundamental * k, heard, &spectrum));
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

/// How wide it is: its side against its mid over the loud frames, dB (−30 is mono: narrower isn't heard).
pub fn width(heard: &Heard) -> Option<f64> {
    let frames = &heard.frames;
    let (mid, side) = frames.mid.iter().zip(&frames.side).fold((0., 0.), |(mid, side), (m, s)| {
        (mid + m.iter().map(|v| *v as f64).sum::<f64>(), side + s.iter().map(|v| *v as f64).sum::<f64>())
    });
    (mid > 1e-14).then(|| round1((10. * ((side + 1e-20) / mid).log10()).max(-30.)))
}

/// Where it stops: the top of the highest third-octave within 30 dB of its loudest, Hz (a lo-fi sound's is low).
pub fn bandwidth(heard: &Heard) -> Option<f64> {
    let Spectrum { bands, .. } = spectrum(heard)?;
    let loudest = bands.iter().copied().fold(0., f64::max);
    if loudest <= 0. {
        return None;
    }
    let top = (0..31).rev().find(|band| bands[*band] >= loudest * 1e-3)?;
    Some(band_edges(top).1.round())
}

/// Its frames' levels (dB), none under 70 dB below its loud parts (digital silence reads as that, as `effects` sets
/// its floor), and the loud parts' level (the 95th percentile).
fn floored(heard: &Heard) -> Option<(Vec<f64>, f64)> {
    let level: Vec<f64> = heard.frames.level.iter().map(|level| *level as f64).filter(|level| level.is_finite()).collect();
    if level.is_empty() {
        return None;
    }
    let mut sorted = level.clone();
    sorted.sort_by(f64::total_cmp);
    let loud = percentile(&sorted, 0.95);
    Some((level.into_iter().map(|level| level.max(loud - 70.)).collect(), loud))
}

/// The noise floor: where its quiet stretches (30 dB and more under its loud parts) bottom out and hold still (their
/// frames within 3 dB of the stretch's lowest, falling under 6 dB a second), under its loud parts, dB, at most 70 dB
/// down. Silence reads as −70, and a tail dying away isn't a floor: None when nothing quiet holds still (a pad, gaps
/// a reverb fills).
pub fn noise_floor(heard: &Heard) -> Option<f64> {
    let (level, loud) = floored(heard)?;
    if level.len() < 20 {
        return None;
    }
    let hop = heard.frames.hop;
    let mut floors = vec![];
    let mut at = 0;
    while at < level.len() {
        if level[at] > loud - 30. {
            at += 1;
            continue;
        }
        let start = at;
        while at < level.len() && level[at] <= loud - 30. {
            at += 1;
        }
        let quiet = &level[start..at];
        let lowest = quiet.iter().copied().fold(f64::MAX, f64::min);
        let bottom: Vec<(f64, f64)> =
            quiet.iter().enumerate().filter(|(_, level)| **level <= lowest + 3.).map(|(k, level)| (k as f64 * hop, *level)).collect();
        if bottom.len() >= 4 && slope(&bottom).abs() <= 6. {
            floors.extend(bottom.iter().map(|(_, level)| *level));
        }
    }
    if floors.len() < 5 {
        return None;
    }
    floors.sort_by(f64::total_cmp);
    Some(round1(percentile(&floors, 0.5) - loud))
}

/// A line's slope through points, y a unit of x.
fn slope(points: &[(f64, f64)]) -> f64 {
    let count = points.len() as f64;
    let (mean_x, mean_y) = (points.iter().map(|p| p.0).sum::<f64>() / count, points.iter().map(|p| p.1).sum::<f64>() / count);
    let (sxy, sxx) = points.iter().fold((0., 0.), |(sxy, sxx), (x, y)| (sxy + (x - mean_x) * (y - mean_y), sxx + (x - mean_x).powi(2)));
    if sxx > 0. {
        sxy / sxx
    } else {
        0.
    }
}

/// Space: how far the level stands under each hit's peak 300 ms on (the median over hits, dB, at most 70 down): a dry
/// hit falls far, a reverberant one hangs on. Only hits with no other within those 300 ms: the next hit isn't tail.
pub fn tail(heard: &Heard) -> Option<f64> {
    let (level, _) = floored(heard)?;
    let steps = (0.3 / heard.frames.hop.max(1e-9)).round() as usize;
    let found = hits(heard);
    let mut tails: Vec<f64> = found
        .iter()
        .enumerate()
        .filter(|(index, (at, _))| at + steps < level.len() && found.get(index + 1).is_none_or(|(next, _)| *next > at + steps))
        .map(|(_, (at, peak))| level[at + steps] - peak)
        .collect();
    if tails.len() < 2 {
        return None;
    }
    tails.sort_by(f64::total_cmp);
    Some(round1(percentile(&tails, 0.5)))
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
    /// Each hit's crest (dB): saturation and clipping flatten it.
    pub hit_crest: Option<f64>,
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
            hit_crest: heard.measures.hit_crest,
            decay_beats: heard.measures.decay.map(|ms| round1(ms / beat_ms)),
            centroid: heard.measures.centroid,
        })
        .collect();
    let mut found = vec![];
    // Cohesion: one piece far off the others in noise floor, top or grit. Only against pieces of a similar range
    // (centred within two octaves of it, three at least): a kick's top and grit differ from a hat's by instrument, not
    // by belonging.
    let odd_one = |value: &dyn Fn(&Piece) -> Option<f64>, by: f64, what: &str, found: &mut Vec<String>| {
        for piece in &measured {
            let (Some(own), Some(centre)) = (value(piece), piece.centroid) else { continue };
            let mut peers: Vec<f64> = measured
                .iter()
                .filter(|other| other.centroid.is_some_and(|other| (other / centre).log2().abs() <= 2.))
                .filter_map(|other| value(other))
                .collect();
            if peers.len() < 3 {
                continue;
            }
            peers.sort_by(f64::total_cmp);
            let middle = percentile(&peers, 0.5);
            if (own - middle).abs() >= by {
                found.push(format!("{}'s {what} ({}) sits apart from its neighbours' ({})", piece.name, number(own), number(middle)));
            }
        }
    };
    odd_one(&|piece| piece.noise_floor, 12., "noise floor (dB)", &mut found);
    odd_one(&|piece| Some(12. * (piece.bandwidth? / 1000.).log2()), 12., "top (semitones above 1 kHz)", &mut found);
    odd_one(&|piece| piece.hit_crest, 4., "crest at each hit (dB: lower is grittier)", &mut found);
    // Coverage: a pile-up (three pieces centred within a third of an octave) or a gap (five octaves between
    // neighbours' centres: a kick and hats with nothing between them; a kick, a snare and hats lie closer).
    let mut centres: Vec<(String, f64)> = measured.iter().filter_map(|piece| Some((piece.name.clone(), piece.centroid?))).collect();
    centres.sort_by(|a, b| a.1.total_cmp(&b.1));
    for window in centres.windows(3) {
        if (window[2].1 / window[0].1).log2() < 1. / 3. {
            found.push(format!("{}, {} and {} pile up around {} Hz", window[0].0, window[1].0, window[2].0, window[1].1.round()));
        }
    }
    // Coverage is a kit's: two tracks (a bass and a lead) aren't asked to fill what lies between them.
    for pair in centres.windows(2).filter(|_| centres.len() >= 3) {
        if (pair[1].1 / pair[0].1).log2() > 5. && pair[0].1 > 60. {
            found.push(format!("nothing between {} ({} Hz) and {} ({} Hz)", pair[0].0, pair[0].1.round(), pair[1].0, pair[1].1.round()));
        }
    }
    // Decays against the piece's own hits: one still within 20 dB of its peak when its next hit comes, most times (a
    // crash or an 808 that hits rarely may ring as long as it likes).
    for (name, heard) in pieces {
        let level = &heard.frames.level;
        let found_hits = hits(heard);
        let pairs: Vec<(f64, f64)> = found_hits
            .windows(2)
            .map(|pair| (pair[0].1 - level[pair[1].0 - 2] as f64, (pair[1].0 - pair[0].0) as f64 * heard.frames.hop * 1000.))
            .collect();
        let ringing = pairs.iter().filter(|(fallen, _)| *fallen < 20.).count();
        if pairs.len() >= 2 && ringing * 2 > pairs.len() {
            let mut apart: Vec<f64> = pairs.iter().map(|(_, ms)| *ms).collect();
            apart.sort_by(f64::total_cmp);
            found.push(format!(
                "{name} rings into its next hit: still within 20 dB of its peak when the next comes ({} ms on)",
                percentile(&apart, 0.5).round()
            ));
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
    // The two parts were measured apart: a hit past the end of the target's take has nothing to duck.
    for (at, _) in hits(trigger).into_iter().filter(|(at, _)| *at > 0 && *at < level.len()) {
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
/// falling between them while the other plays (interlocking). The rest come while the other is silent.
pub fn interlock(a: &Heard, b: &Heard) -> Option<(f64, f64)> {
    let hop = a.frames.hop;
    let (first, second): (Vec<usize>, Vec<usize>) =
        (hits(a).iter().map(|(at, _)| *at).collect(), hits(b).iter().map(|(at, _)| *at).collect());
    if first.len() < 4 || second.len() < 4 {
        return None;
    }
    // Within 30 ms, to a frame (about 21 ms at 48 kHz).
    let near = (0.03 / hop).floor().max(1.) as usize;
    let colliding = first.iter().filter(|at| second.iter().any(|other| other.abs_diff(**at) <= near)).count();
    // Between: while the other plays (in a gap of its no wider than twice its usual spacing, or within one spacing
    // of its first or last hit), not where it's silent.
    let mut gaps: Vec<f64> = second.windows(2).map(|pair| (pair[1] - pair[0]) as f64).collect();
    gaps.sort_by(f64::total_cmp);
    let usual = percentile(&gaps, 0.5);
    let between = first
        .iter()
        .filter(|at| !second.iter().any(|other| other.abs_diff(**at) <= near))
        .filter(|at| {
            let next = second.partition_point(|other| other < at);
            let span = match (next.checked_sub(1).map(|prev| second[prev]), second.get(next)) {
                (Some(prev), Some(next)) => (next - prev) as f64 / 2.,
                (Some(prev), None) => (**at - prev) as f64,
                (None, Some(next)) => (next - **at) as f64,
                (None, None) => f64::INFINITY,
            };
            span <= usual
        })
        .count();
    let share = |count: usize| ((count as f64 / first.len() as f64) * 100.).round();
    Some((share(colliding), share(between)))
}

/// A low part's pitch against a key (its notes as pitch classes, 0 is C): how far its notes sit from the nearest note
/// of the key, cents (median), and the share of its time off the key by over a quarter tone. None for a part that
/// isn't low: its bass line is where its notes are read.
pub fn against_key(heard: &Heard, key: &[u8]) -> Option<(f64, f64)> {
    let pitches: Vec<f64> = bass_line(heard)?.into_iter().map(|(_, hz)| hz).collect();
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

/// A key in words ("F# minor", "Bbm", "C", "D dorian", "A harmonic minor") as its notes' pitch classes (0 is C): the
/// major scale, the natural minor, the harmonic or melodic minor, or a mode. None when it doesn't read as a key.
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
    let scale: Vec<i32> = match quality.as_str() {
        // The minor scale with its 7th raised, and the melodic minor with its 6th raised too.
        q if q.starts_with("har") => vec![0, 2, 3, 5, 7, 8, 11],
        q if q.starts_with("mel") => vec![0, 2, 3, 5, 7, 9, 11],
        q => {
            let mode = match q {
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
            (0..7).map(|step| MAJOR[(mode + step) % 7] - MAJOR[mode]).collect()
        }
    };
    Some(scale.into_iter().map(|interval| (root + shift + interval).rem_euclid(12) as u8).collect())
}
