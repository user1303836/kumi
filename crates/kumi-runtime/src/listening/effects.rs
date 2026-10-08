//! Effects, read off what's heard: a reverb's decay time and how its tails darken and widen, a delay's echoes (their
//! time, how many stand out, how far each falls and darkens), how deep a swing goes, how far the brightness sweeps (a
//! filter moving), pumping against the beat, and how much of the sound is tail. And what's wrong: tails cut off,
//! echoes out of time, repeats that don't die away, tails burying the dry sound.

use super::{
    measure::{percentile, Heard, ENVELOPE_HOP, THIRDS},
    sound,
};
use serde::{Deserialize, Serialize};

const HOP: f64 = ENVELOPE_HOP;

/// A delay's echoes.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Echo {
    /// The time from one repeat to the next, ms.
    pub ms: f64,
    /// How many repeats follow a hit (the median over the hits that start a run of them).
    pub repeats: usize,
    /// How far each repeat falls under the one before, dB.
    pub falls: f64,
    /// How much darker each repeat is than the one before, octaves (a dub delay's darken).
    pub darkens: Option<f64>,
}

/// Pumping against the beat.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Pump {
    /// How far the level dips once a beat, dB.
    pub depth: f64,
    /// Where the dip is deepest, a fraction of a beat after the beat (when where the beat falls is known).
    pub lowest: Option<f64>,
    /// How long it takes from the lowest point back within 1 dB of the top, a fraction of a beat.
    pub back: f64,
}

/// What effects do to a sound, as heard.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Effects {
    /// Decay time (RT60) from the slope of its tails, seconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decay_time: Option<f64>,
    /// Whether its tails fall in two slopes, the second under half as steep: a reverb's tail after the sound's own fall
    /// (one slope is the sound's own decay).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub reverb: bool,
    /// How much darker its tails are than its hits, octaves (damping).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub darkening: Option<f64>,
    /// How much wider its tails are than its hits, dB of side against mid.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub widening: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub echo: Option<Echo>,
    /// How far its level swings at its modulation rate, dB (a tremolo's or an LFO's depth).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub swing: Option<f64>,
    /// How far its brightness moves, octaves (the 10th to the 90th percentile of its loud moments).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sweep: Option<f64>,
    /// How long the brightness takes to come round again, seconds, when it repeats.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sweep_cycle: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pump: Option<Pump>,
    /// The share of its energy after each hit's first 60 ms, % (its tail and room: a wet sound's is high).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tail_share: Option<f64>,
}

/// Everything at once. With the tempo, pumping against the beat too; `first_beat` is where a beat falls (seconds
/// into what was heard), when it's known.
pub fn effects(heard: &Heard, tempo: Option<f64>, first_beat: Option<f64>) -> Effects {
    let (sweep, sweep_cycle) = match sweep(heard) {
        Some((octaves, cycle)) => (Some(octaves), cycle),
        None => (None, None),
    };
    let (darkening, widening) = colour(heard);
    Effects {
        decay_time: decay_time(heard),
        reverb: slopes(heard).is_some_and(|(_, tail)| tail.is_some()),
        darkening,
        widening,
        echo: echo(heard),
        swing: swing(heard),
        sweep,
        sweep_cycle,
        pump: tempo.and_then(|tempo| pump(heard, tempo, first_beat)),
        tail_share: tail_share(heard),
    }
}

/// Its level every 5 ms (dB), the floor under it (the 5th percentile, but no lower than 70 dB under the loud parts)
/// and the loud parts' level (the 95th).
fn levels(heard: &Heard) -> Option<(Vec<f64>, f64, f64)> {
    let db: Vec<f64> = heard.frames.envelope.iter().map(|value| 20. * (*value as f64 + 1e-9).log10()).collect();
    if db.len() < 40 {
        return None;
    }
    let mut sorted = db.clone();
    sorted.sort_by(f64::total_cmp);
    let loud = percentile(&sorted, 0.95);
    if loud < -100. {
        return None;
    }
    Some((db, percentile(&sorted, 0.05).max(loud - 70.), loud))
}

/// Where it dies away undisturbed: from a peak 25 dB over the floor (the loudest within 50 ms), the level falling
/// until something new starts (a rise of 6 dB) or it nears the floor; kept when it falls 20 dB and more over 100 ms
/// and more. Each as its peak's step and its lowest's.
fn decays(db: &[f64], floor: f64) -> Vec<(usize, usize)> {
    let near = (0.05 / HOP).round() as usize;
    let mut found = vec![];
    let mut at = 0;
    while at < db.len() {
        let (from, to) = (at.saturating_sub(near), (at + near + 1).min(db.len()));
        if db[at] < floor + 25. || db[from..to].iter().any(|level| *level > db[at]) {
            at += 1;
            continue;
        }
        let (mut lowest, mut end, mut next) = (db[at], at, at + 1);
        while next < db.len() && db[next] <= lowest + 6. && lowest > floor + 6. {
            if db[next] < lowest {
                lowest = db[next];
                end = next;
            }
            next += 1;
        }
        if db[at] - lowest >= 20. && (end - at) as f64 * HOP >= 0.1 {
            found.push((at, end));
        }
        at = next.max(at + 1);
    }
    found
}

/// A line's slope through levels a step apart, dB a second.
fn slope(levels: &[f64]) -> f64 {
    let count = levels.len() as f64;
    let (mean_x, mean_y) = ((count - 1.) / 2., levels.iter().sum::<f64>() / count);
    let (mut sxy, mut sxx) = (0., 0.);
    for (k, level) in levels.iter().enumerate() {
        let x = k as f64 - mean_x;
        sxy += x * (level - mean_y);
        sxx += x * x;
    }
    sxy / sxx.max(1e-12) / HOP
}

/// One tail's decay time (RT60): the slope of its level from 15 dB down when that leaves 10 dB and 60 ms to read
/// (a reverb's tail after the dry sound's own fall), else from 5 dB down.
fn decay_time_of(db: &[f64], (peak, end): (usize, usize)) -> Option<f64> {
    let top = db[peak];
    let least = (0.06 / HOP).round() as usize;
    let start = [15., 5.].iter().find_map(|down| {
        let start = (peak..=end).find(|k| db[*k] <= top - down)?;
        (end >= start + least && db[start] - db[end] >= 10.).then_some(start)
    })?;
    let falls = slope(&db[start..=end]);
    (falls < -1.).then(|| (-60. / falls).clamp(0.05, 30.))
}

/// How its undisturbed tails fall, dB a second: the sound's own fall (the median), and a slower tail after it (a
/// reverb's) when most of them show one. A tail's first 25 ms are the hit's own fall, however close under the hit the
/// tail starts; what follows, a tail when it falls under half as steeply after a drop of 3 dB and more, and evenly.
/// Otherwise it's one slope, the sound's own, and nothing is said of a reverb.
pub fn slopes(heard: &Heard) -> Option<(f64, Option<f64>)> {
    let (db, floor, _) = levels(heard)?;
    let (head, least) = ((0.025 / HOP).round() as usize, (0.06 / HOP).round() as usize);
    let mut own = vec![];
    let mut tails = vec![];
    for (peak, end) in decays(&db, floor) {
        let knee = peak + head;
        let two = end >= knee + least && db[peak] - db[knee] >= 3. && db[knee] - db[end] >= 10.;
        // A reverb's tail falls evenly, its two halves at about the same rate; a note held at a sustain and then
        // released doesn't (its first half is flat).
        let even = |from: usize, to: usize| {
            let middle = (from + to) / 2;
            let (first, second) = (slope(&db[from..=middle]), slope(&db[middle..=to]));
            first < 0. && second < 0. && first.max(second) / first.min(second) >= 0.5
        };
        let (fall, tail) = (slope(&db[peak..=knee.min(end)]), (two && even(knee, end)).then(|| slope(&db[knee..=end])));
        match tail.filter(|tail| *tail > fall / 2.) {
            Some(tail) => {
                own.push(fall);
                tails.push(tail);
            }
            None => own.push(slope(&db[peak..=end])),
        }
    }
    if own.is_empty() {
        return None;
    }
    let median = |mut values: Vec<f64>| {
        values.sort_by(f64::total_cmp);
        percentile(&values, 0.5)
    };
    let reverb = (tails.len() * 2 >= own.len()).then(|| median(tails));
    Some((median(own), reverb))
}

/// Decay time (RT60, seconds): the median over its undisturbed tails.
pub fn decay_time(heard: &Heard) -> Option<f64> {
    let (db, floor, _) = levels(heard)?;
    let mut times: Vec<f64> = decays(&db, floor).into_iter().filter_map(|decay| decay_time_of(&db, decay)).collect();
    if times.is_empty() {
        return None;
    }
    times.sort_by(f64::total_cmp);
    Some(round2(percentile(&times, 0.5)))
}

/// How much darker its tails get than its hits, octaves (the median over its undisturbed tails).
pub fn darkening(heard: &Heard) -> Option<f64> {
    colour(heard).0
}

/// The analysis frame whose window is centred nearest a step (a frame's window spans about four of its hops).
fn frame_of(heard: &Heard, step: usize) -> usize {
    ((step as f64 * HOP / heard.frames.hop.max(1e-9)) - 2.).round().max(0.) as usize
}

/// A frame's centroid, Hz.
fn centroid_at(heard: &Heard, frame: usize) -> Option<f64> {
    let (mid, side) = (heard.frames.mid.get(frame)?, heard.frames.side.get(frame)?);
    let (weighted, total) = (0..31).fold((0., 0.), |(weighted, total), band| {
        let power = (mid[band] + side[band]) as f64;
        (weighted + power * THIRDS[band], total + power)
    });
    (total > 1e-14).then(|| weighted / total)
}

/// A frame's side against its mid, dB.
fn width_at(heard: &Heard, frame: usize) -> Option<f64> {
    let mid: f64 = heard.frames.mid.get(frame)?.iter().map(|value| *value as f64).sum();
    let side: f64 = heard.frames.side.get(frame)?.iter().map(|value| *value as f64).sum();
    (mid > 1e-14).then(|| (10. * ((side + 1e-20) / mid).log10()).max(-60.))
}

/// How its tails change colour: how much darker (octaves of centroid) and wider (dB of side against mid) they are
/// 20 dB down than at the hit, the medians over its undisturbed tails.
fn colour(heard: &Heard) -> (Option<f64>, Option<f64>) {
    let Some((db, floor, _)) = levels(heard) else { return (None, None) };
    let least = (0.1 / HOP).round() as usize;
    let (mut darker, mut wider) = (vec![], vec![]);
    for (peak, end) in decays(&db, floor) {
        let Some(late) = (peak..=end).find(|k| db[*k] <= db[peak] - 20.) else { continue };
        if late < peak + least || db[late] < floor + 10. {
            continue;
        }
        let (hit, tail) = (frame_of(heard, peak), frame_of(heard, late));
        if let (Some(bright), Some(dark)) = (centroid_at(heard, hit), centroid_at(heard, tail)) {
            darker.push((bright / dark).log2());
        }
        if let (Some(narrow), Some(wide)) = (width_at(heard, hit), width_at(heard, tail)) {
            wider.push(wide - narrow);
        }
    }
    let median = |mut values: Vec<f64>| -> Option<f64> {
        values.sort_by(f64::total_cmp);
        (!values.is_empty()).then(|| round2(percentile(&values, 0.5)))
    };
    (median(darker), median(wider).map(round1))
}

/// Its hits: a rise of 9 dB within 20 ms to 15 dB over the floor, at least 40 ms apart; each as its peak's step and
/// level (dB).
fn onsets(db: &[f64], floor: f64) -> Vec<(usize, f64)> {
    let (rise, gap) = ((0.02 / HOP).round() as usize, (0.04 / HOP).round() as usize);
    let mut found = vec![];
    let mut at = rise;
    while at < db.len() {
        let before = db[at - rise..at].iter().copied().fold(f64::MAX, f64::min);
        if db[at] - before >= 9. && db[at] > floor + 15. {
            let end = (at + gap).min(db.len());
            let (peak_at, peak) = (at..end).map(|k| (k, db[k])).max_by(|a, b| a.1.total_cmp(&b.1)).unwrap_or((at, db[at]));
            found.push((peak_at, peak));
            at = end;
        } else {
            at += 1;
        }
    }
    found
}

/// A delay's echoes: a lag (60 ms to 1.6 s) at which most hits are followed by a quieter copy (by a steady 0.5 to
/// 30 dB), with a second copy quieter again by about as much at twice the lag. Played notes repeat at their own
/// levels, so a rhythm doesn't read as echoes.
pub fn echo(heard: &Heard) -> Option<Echo> {
    let (db, floor, _) = levels(heard)?;
    let hits = onsets(&db, floor);
    if hits.len() < 3 {
        return None;
    }
    let tolerance = (0.015 / HOP).round() as usize;
    let (shortest, longest) = ((0.06 / HOP).round() as usize, (1.6 / HOP).round() as usize);
    // The hit nearest `lag` after `from`, within 15 ms (the hits run in order).
    let follow = |from: usize, lag: usize| -> Option<(usize, f64)> {
        let target = from + lag;
        let start = hits.partition_point(|(at, _)| at + tolerance < target);
        hits[start..].iter().take_while(|(at, _)| *at <= target + tolerance).min_by_key(|(at, _)| at.abs_diff(target)).copied()
    };
    let mut lags: Vec<usize> = hits
        .iter()
        .enumerate()
        .flat_map(|(index, (a, _))| hits[index + 1..].iter().map(move |(b, _)| b - a))
        .filter(|lag| (shortest..=longest).contains(lag))
        .collect();
    lags.sort_unstable();
    lags.dedup();
    let loudest = hits.iter().map(|(_, peak)| *peak).fold(f64::MIN, f64::max);
    // The best lag: the most hits followed, then the steadiest falls.
    let mut best: Option<(usize, Vec<f64>, Vec<usize>)> = None;
    for lag in lags {
        let (mut falls, mut seen, mut seconds, mut strong) = (vec![], vec![], 0, 0);
        for (at, peak) in &hits {
            if at + lag + tolerance >= db.len() || *peak < loudest - 10. {
                continue;
            }
            strong += 1;
            let Some((first_at, first)) = follow(*at, lag) else { continue };
            let fall = peak - first;
            if !(0.5..=30.).contains(&fall) {
                continue;
            }
            falls.push(fall);
            seen.push(first_at - at);
            if follow(first_at, lag).is_some_and(|(_, second)| (first - second - fall).abs() <= 3.) {
                seconds += 1;
            }
        }
        if falls.len() < 3 || falls.len() * 2 < strong || seconds * 10 < falls.len() * 3 {
            continue;
        }
        let mut sorted = falls.clone();
        sorted.sort_by(f64::total_cmp);
        if percentile(&sorted, 0.75) - percentile(&sorted, 0.25) > 4. {
            continue;
        }
        if best.as_ref().is_none_or(|(_, kept, _)| falls.len() > kept.len()) {
            best = Some((lag, falls, seen));
        }
    }
    let (lag, mut falls, seen) = best?;
    falls.sort_by(f64::total_cmp);
    let ms = seen.iter().sum::<usize>() as f64 / seen.len() as f64 * HOP * 1000.;
    // Runs of repeats: from each hit that isn't itself a repeat (a quieter copy of the hit a lag before), how many
    // follow.
    let mut repeat = vec![false; hits.len()];
    for (at, peak) in &hits {
        if let Some((next, level)) = follow(*at, lag).filter(|(_, level)| level < peak) {
            if let Ok(index) = hits.binary_search_by_key(&next, |(at, _)| *at) {
                repeat[index] = level < *peak;
            }
        }
    }
    let mut runs: Vec<f64> = vec![];
    let mut darker: Vec<f64> = vec![];
    for (at, _) in hits.iter().zip(&repeat).filter(|(_, repeat)| !**repeat).map(|(hit, _)| hit) {
        let (mut count, mut from) = (0, *at);
        while let Some((next, _)) = follow(from, lag) {
            if count == 0 {
                if let (Some(bright), Some(dark)) = (centroid_at(heard, frame_of(heard, from)), centroid_at(heard, frame_of(heard, next))) {
                    darker.push((bright / dark).log2());
                }
            }
            count += 1;
            from = next;
        }
        if count > 0 {
            runs.push(count as f64);
        }
    }
    runs.sort_by(f64::total_cmp);
    darker.sort_by(f64::total_cmp);
    Some(Echo {
        ms: ms.round(),
        repeats: if runs.is_empty() { 1 } else { percentile(&runs, 0.5).round() as usize },
        falls: round1(percentile(&falls, 0.5)),
        darkens: (!darker.is_empty()).then(|| round2(percentile(&darker, 0.5))),
    })
}

/// How far each echo falls when there are none: as far as a reverb's decay is measured.
pub const NO_ECHO: f64 = 60.;

/// How far each echo falls, dB: `NO_ECHO` when there are none and there could have been, so a dry sound and a delayed
/// one can be compared: most of its hits die away 40 dB before the next comes, so a copy (30 dB under at most) would
/// have stood out. None when that can't be told: hits too close to hear between (a delayed 8th-note riff's repeats
/// land on its notes), or none at all.
pub fn echo_falls(heard: &Heard) -> Option<f64> {
    if let Some(echo) = echo(heard) {
        return Some(echo.falls);
    }
    let (db, floor, _) = levels(heard)?;
    let hits = onsets(&db, floor);
    let clear = hits
        .iter()
        .enumerate()
        .filter(|(index, (at, peak))| {
            let next = hits.get(index + 1).map_or(db.len(), |(next, _)| *next);
            db[*at..next].iter().any(|level| *level <= peak - 40.)
        })
        .count();
    (!hits.is_empty() && clear * 3 >= hits.len() * 2).then_some(NO_ECHO)
}

/// A time as the nearest note value at a tempo ("dotted 1/8") and how far off it is (a fraction: 0.02 is 2 % long).
pub fn note_value(seconds: f64, tempo: f64) -> (String, f64) {
    const VALUES: [(&str, f64); 14] = [
        ("1/32", 0.125),
        ("1/16 triplet", 1. / 6.),
        ("1/16", 0.25),
        ("1/8 triplet", 1. / 3.),
        ("dotted 1/16", 0.375),
        ("1/8", 0.5),
        ("1/4 triplet", 2. / 3.),
        ("dotted 1/8", 0.75),
        ("1/4", 1.),
        ("1/2 triplet", 4. / 3.),
        ("dotted 1/4", 1.5),
        ("1/2", 2.),
        ("dotted 1/2", 3.),
        ("whole note", 4.),
    ];
    let beats = seconds * tempo / 60.;
    let (name, value) =
        VALUES.iter().min_by(|a, b| (beats / a.1).ln().abs().total_cmp(&(beats / b.1).ln().abs())).copied().unwrap_or(("1/4", 1.));
    (name.to_string(), beats / value - 1.)
}

/// A moving average over `width` steps, centred.
fn moving(values: &[f64], width: usize) -> Vec<f64> {
    let half = width / 2;
    let mut sums = vec![0.; values.len() + 1];
    for (k, value) in values.iter().enumerate() {
        sums[k + 1] = sums[k] + value;
    }
    (0..values.len())
        .map(|k| {
            let (from, to) = (k.saturating_sub(half), (k + half + 1).min(values.len()));
            (sums[to] - sums[from]) / (to - from) as f64
        })
        .collect()
}

/// How far its level swings at its modulation rate, dB: within each cycle, the level's highest against its lowest
/// around its own slow trend (the median over the loud cycles). 0 for a held sound (no hits) heard long enough to find
/// a swing (two seconds) that has none, so a sound without modulation can be compared with one with it. None when that
/// can't be told: hits whose rhythm could hide a swing (a gated pad), or too short a listen.
pub fn swing(heard: &Heard) -> Option<f64> {
    // A held sound has no silence to set a floor by: what sounds is within 30 dB of its loud parts.
    let (db, floor, loud) = levels(heard)?;
    let Some(rate) = heard.measures.modulation.filter(|rate| *rate > 0.) else {
        return (db.len() as f64 * HOP >= 2. && onsets(&db, floor).is_empty()).then_some(0.);
    };
    let period = ((1. / rate) / HOP).round().max(2.) as usize;
    let trend = moving(&db, period * 2 + 1);
    let mut depths: Vec<f64> = db
        .chunks(period)
        .zip(trend.chunks(period))
        .filter(|(cycle, trend)| cycle.len() == period && trend.iter().all(|level| *level > loud - 30.))
        .map(|(cycle, trend)| {
            let swings: Vec<f64> = cycle.iter().zip(trend).map(|(level, trend)| level - trend).collect();
            swings.iter().copied().fold(f64::MIN, f64::max) - swings.iter().copied().fold(f64::MAX, f64::min)
        })
        .collect();
    if depths.len() < 3 {
        return None;
    }
    depths.sort_by(f64::total_cmp);
    Some(round1(percentile(&depths, 0.5)))
}

/// How far its brightness moves (octaves of centroid, the 10th to the 90th percentile over its loud frames) and, when
/// it comes round again (a repeat as strong as half its movement, from a quarter second to 8 s), how long a cycle takes.
pub fn sweep(heard: &Heard) -> Option<(f64, Option<f64>)> {
    let frames = &heard.frames;
    let mut levels: Vec<f64> = frames.level.iter().map(|level| *level as f64).collect();
    if levels.len() < 20 {
        return None;
    }
    levels.sort_by(f64::total_cmp);
    let loud = percentile(&levels, 0.95) - 20.;
    // A pitched part's brightness follows its notes: read against them (a low part's bass line, another's lowest clear
    // peak frame by frame), a melody isn't a filter moving.
    let notes: Option<Vec<Option<f64>>> = match sound::bass_line(heard) {
        Some(line) => {
            let mut pitches = vec![None; frames.bass.len()];
            for (low, hz) in line {
                pitches[low] = Some(hz);
            }
            let ratio = frames.bass_hop / frames.hop.max(1e-9);
            Some(
                (0..frames.level.len())
                    .map(|frame| pitches.get(((frame as f64 + 2.) / ratio - 2.).round().max(0.) as usize).copied().flatten())
                    .collect(),
            )
        }
        None => sound::melody(heard),
    };
    let series: Vec<Option<f64>> = (0..frames.level.len())
        .map(|frame| {
            let centroid = centroid_at(heard, frame).filter(|_| frames.level[frame] as f64 >= loud)?.log2();
            match &notes {
                Some(notes) => notes[frame].map(|hz| centroid - hz.log2()),
                None => Some(centroid),
            }
        })
        .collect();
    let mut values: Vec<f64> = series.iter().flatten().copied().collect();
    if values.len() < 20 {
        return None;
    }
    values.sort_by(f64::total_cmp);
    let range = percentile(&values, 0.9) - percentile(&values, 0.1);
    // Held through quiet frames, its mean taken out.
    let mean = values.iter().sum::<f64>() / values.len() as f64;
    let mut last = mean;
    let held: Vec<f64> = series
        .iter()
        .map(|value| {
            last = value.unwrap_or(last);
            last - mean
        })
        .collect();
    let energy: f64 = held.iter().map(|value| value * value).sum();
    let (from, to) = ((0.25 / frames.hop).ceil() as usize, ((8. / frames.hop) as usize).min(held.len() / 2));
    let correlation = |lag: usize| held.iter().zip(&held[lag..]).map(|(a, b)| a * b).sum::<f64>() / energy.max(1e-12);
    let cycle = (from.max(1)..to)
        .map(|lag| (lag, correlation(lag)))
        .filter(|(lag, value)| *value >= 0.5 && *value >= correlation(lag - 1) && *value >= correlation(lag + 1))
        .max_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(lag, _)| round2(lag as f64 * frames.hop));
    Some((round2(range), cycle.filter(|_| range >= 0.25)))
}

/// Pumping against the beat, for a sound that holds (sounding most of the time, mostly within 15 dB of its top): its level around
/// its slow trend, folded over the beat (from `first_beat` when it's known), and the cycle's depth, recovery and (from
/// the beat) its lowest point. None when it dips under 1.5 dB.
pub fn pump(heard: &Heard, tempo: f64, first_beat: Option<f64>) -> Option<Pump> {
    let (db, _, loud) = levels(heard)?;
    let mut active: Vec<f64> = db.iter().copied().filter(|level| *level > loud - 30.).collect();
    if active.len() * 2 < db.len() {
        return None;
    }
    active.sort_by(f64::total_cmp);
    if percentile(&active, 0.2) < percentile(&active, 0.9) - 15. {
        return None;
    }
    let beat = 60. / tempo.max(1.);
    let trend = moving(&db, ((4. * beat) / HOP) as usize | 1);
    const BINS: usize = 20;
    let mut bins: Vec<Vec<f64>> = vec![vec![]; BINS];
    for (k, (level, trend)) in db.iter().zip(&trend).enumerate() {
        if *trend < loud - 30. {
            continue;
        }
        let phase = ((k as f64 * HOP - first_beat.unwrap_or(0.)) / beat).rem_euclid(1.);
        bins[((phase * BINS as f64) as usize).min(BINS - 1)].push(level - trend);
    }
    if bins.iter().any(|bin| bin.len() < 3) {
        return None;
    }
    let cycle: Vec<f64> = bins
        .iter_mut()
        .map(|bin| {
            bin.sort_by(f64::total_cmp);
            percentile(bin, 0.5)
        })
        .collect();
    let top = cycle.iter().copied().fold(f64::MIN, f64::max);
    let (low_at, low) = cycle.iter().copied().enumerate().min_by(|a, b| a.1.total_cmp(&b.1))?;
    if top - low < 1.5 {
        return None;
    }
    let back = (1..=BINS).find(|ahead| top - cycle[(low_at + ahead) % BINS] <= 1.).unwrap_or(BINS);
    Some(Pump {
        depth: round1(top - low),
        lowest: first_beat.map(|_| round2((low_at as f64 + 0.5) / BINS as f64)),
        back: round2(back as f64 / BINS as f64),
    })
}

/// The share of its energy after each hit's first 60 ms (until the next hit, a second at most), % — over the floor.
pub fn tail_share(heard: &Heard) -> Option<f64> {
    let (db, floor, _) = levels(heard)?;
    tail_share_of(&db, floor, 0, db.len())
}

fn tail_share_of(db: &[f64], floor: f64, from: usize, to: usize) -> Option<f64> {
    let hits: Vec<(usize, f64)> = onsets(db, floor).into_iter().filter(|(at, _)| (from..to).contains(at)).collect();
    if hits.len() < 2 {
        return None;
    }
    let (head, longest, lead) = ((0.06 / HOP).round() as usize, (1. / HOP).round() as usize, (0.01 / HOP).round() as usize);
    let over = |level: f64| (10f64.powf(level / 10.) - 10f64.powf(floor / 10.)).max(0.);
    let (mut heads, mut tails) = (0., 0.);
    for (index, (at, _)) in hits.iter().enumerate() {
        let next = hits.get(index + 1).map_or(db.len(), |(next, _)| next.saturating_sub(lead)).min(at + longest).min(db.len());
        let split = (at + head).min(next);
        heads += db[at.saturating_sub(lead)..split].iter().map(|level| over(*level)).sum::<f64>();
        tails += db[split..next].iter().map(|level| over(*level)).sum::<f64>();
    }
    (heads + tails > 0.).then(|| (tails / (heads + tails) * 100.).round())
}

/// The tail's share bar by bar (`bar` seconds long), where it can be read.
pub fn tail_shares(heard: &Heard, bar: f64) -> Vec<Option<f64>> {
    let Some((db, floor, _)) = levels(heard) else { return vec![] };
    let steps = (bar / HOP).round().max(1.) as usize;
    (0..db.len().div_ceil(steps)).map(|index| tail_share_of(&db, floor, index * steps, ((index + 1) * steps).min(db.len()))).collect()
}

/// Where its tails are cut off: a fall of 25 dB within 20 ms, to near the floor, out of a tail that was dying away
/// (3 to 10 dB in the 100 ms before, and not already falling fast: a held note's release isn't a cut tail). Seconds
/// into what was heard.
pub fn cut_tails(heard: &Heard) -> Vec<f64> {
    let Some((db, floor, _)) = levels(heard) else { return vec![] };
    let (fall, before) = ((0.02 / HOP).round() as usize, (0.1 / HOP).round() as usize);
    let mut found: Vec<f64> = vec![];
    let mut at = fall + before;
    while at < db.len() {
        let (was, then) = (db[at - fall], db[at - fall - before]);
        let steady = db[at - fall - 1] - was <= 1.5;
        if was - db[at] >= 25. && was > floor + 25. && db[at] <= floor + 8. && then - was < 10. && then - was >= 3. && steady {
            found.push(round2(at as f64 * HOP));
            at += before;
        } else {
            at += 1;
        }
    }
    found
}

/// What's wrong with its effects: tails cut off (`at` turns seconds into where they are), echoes out of time with the
/// tempo, repeats that don't die away, tails burying the dry hits.
pub fn problems(heard: &Heard, effects: &Effects, tempo: Option<f64>, at: &dyn Fn(f64) -> String) -> Vec<String> {
    let mut found = vec![];
    let cut = cut_tails(heard);
    if !cut.is_empty() {
        let said: Vec<String> = cut.iter().take(3).map(|seconds| at(*seconds)).collect();
        found.push(format!(
            "its tail is cut off {} time{} ({}): a gate, a choke or a clip ending under it",
            cut.len(),
            if cut.len() == 1 { "" } else { "s" },
            said.join(", ")
        ));
    }
    if let Some(echo) = effects.echo {
        if let Some(tempo) = tempo {
            let (name, off) = note_value(echo.ms / 1000., tempo);
            if off.abs() > 0.03 {
                found.push(format!(
                    "its echoes come every {} ms, {} % {} a {name} at {} BPM: out of time",
                    echo.ms,
                    (off.abs() * 100.).round(),
                    if off > 0. { "longer than" } else { "shorter than" },
                    round1(tempo)
                ));
            }
        }
        if echo.falls < 1.5 && echo.repeats >= 5 {
            found.push(format!("its echoes barely die away ({} dB a repeat): the feedback is near running away", echo.falls));
        }
    }
    // Against the dry hit's own fall: a long sound is mostly tail by itself, an effect burying it adds far more.
    if let (Some(share), Some((own, _))) = (effects.tail_share.filter(|share| *share >= 75.), slopes(heard)) {
        if share - own_share(own) >= 25. {
            found.push(format!("{share} % of its energy is tail: the effect buries the dry hits"));
        }
    }
    found
}

/// The tail's share (%) a hit decaying at `slope` dB a second would have by itself, over the second after it.
fn own_share(slope: f64) -> f64 {
    let rate = (-slope).max(1e-6) * std::f64::consts::LN_10 / 10.;
    let energy = |from: f64, to: f64| ((-rate * from).exp() - (-rate * to).exp()) / rate;
    energy(0.06, 1.) / energy(0., 1.) * 100.
}

fn round1(value: f64) -> f64 {
    (value * 10.).round() / 10.
}

fn round2(value: f64) -> f64 {
    (value * 100.).round() / 100.
}
