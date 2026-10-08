//! Sequencing measured on the notes, exactly and cheaply: where in the bar a part plays (how its notes cluster), how
//! early or late each grid step sits (its timing profile, and the swing in it), how hard (its velocity profile), how
//! much it plays off the beat (syncopation) and how its phrase ends differ (fills). A drum part splits into lanes
//! (kick, snare, hats, the rest); a pitched part is one lane.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Note {
    /// In beats.
    pub start: f64,
    pub length: f64,
    pub pitch: i32,
    pub velocity: f64,
}

/// One lane's feel, step by step (sixteenths of the bar).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Lane {
    pub name: String,
    /// How often a bar has a note on each step (0–1).
    pub density: Vec<f64>,
    /// How late (+) or early (−) its notes sit on each step, ms from the straight grid at the part's tempo.
    pub offset: Vec<Option<f64>>,
    /// Its notes' mean velocity on each step.
    pub velocity: Vec<Option<f64>>,
    pub notes: usize,
}

/// A part's feel.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Feel {
    pub tempo: f64,
    pub steps: usize,
    pub bars: usize,
    pub lanes: Vec<Lane>,
    /// The grid it swings on: 8 (eighths) or 16, and how much: 50 % is straight, 66.7 % triplet.
    pub grid: u32,
    pub swing: f64,
    /// How far notes sit from the swung grid on average, ms (how loose it plays).
    pub looseness: f64,
    /// Whether it's ahead (−) or behind (+) the beat on average, ms.
    pub push: f64,
    /// Share of notes on weak steps with nothing on the next strong one.
    pub syncopation: f64,
    /// Notes in each phrase's last bar against the others' (1: no fills), when there are phrases.
    pub fills: Option<f64>,
    /// How much the velocities vary.
    pub velocity_spread: f64,
}

/// A drum note's lane, by the General MIDI drum map (Live's Drums to MIDI writes kick, snare and hats on it).
pub fn drum_lane(pitch: i32) -> &'static str {
    match pitch {
        35 | 36 => "kick",
        37..=40 => "snare",
        42 | 44 | 46 => "hats",
        49 | 51 | 52 | 53 | 55 | 57 | 59 => "cymbals",
        _ => "perc",
    }
}

/// Whether the notes look like drums: few pitches, all in the drum map's range.
pub fn drums(notes: &[Note]) -> bool {
    let mut pitches: Vec<i32> = notes.iter().map(|note| note.pitch).collect();
    pitches.sort();
    pitches.dedup();
    !pitches.is_empty() && pitches.len() <= 12 && pitches.iter().all(|pitch| (35..=81).contains(pitch))
}

/// The swung grid that explains the notes best: eighths or sixteenths, and the swing (50–75 %), by how close the
/// notes sit to it. Sixteenths have to explain them clearly better, being twice as dense.
pub fn fit_swing(starts: &[f64]) -> (u32, f64) {
    // Lateness all round isn't swing: the part's push (how it sits on the eighths) comes out first.
    let push = push_of(starts);
    let starts: Vec<f64> = starts.iter().map(|start| start - push).collect();
    let starts = &starts;
    let residual = |grid: u32, swing: f64| -> f64 {
        let step = 4. / grid as f64;
        starts
            .iter()
            .map(|start| {
                let pair = (start / (2. * step)).floor();
                let base = pair * 2. * step;
                let points = [base, base + 2. * step * swing / 100., base + 2. * step];
                points.iter().map(|point| (start - point).abs()).fold(f64::MAX, f64::min) / step
            })
            .sum::<f64>()
            / starts.len().max(1) as f64
    };
    let best = |grid: u32| {
        (50..=75).map(|swing| (swing as f64, residual(grid, swing as f64))).min_by(|a, b| a.1.total_cmp(&b.1)).unwrap_or((50., 1.))
    };
    let (eighths, sixteenths) = (best(8), best(16));
    // The residuals are in steps of each grid: compare them in beats.
    if sixteenths.1 * 0.5 < eighths.1 * 0.6 {
        (16, sixteenths.0)
    } else {
        (8, eighths.0)
    }
}

/// How far ahead (−) or behind (+) the part sits on the beat, in beats: the median offset of the notes near an eighth
/// (within a sixteenth's half), where swing doesn't move them.
pub fn push_of(starts: &[f64]) -> f64 {
    let mut near: Vec<f64> =
        starts.iter().map(|start| start - (start / 0.5).round() * 0.5).filter(|offset| offset.abs() <= 0.125).collect();
    if near.is_empty() {
        return 0.;
    }
    near.sort_by(f64::total_cmp);
    near[near.len() / 2]
}

/// The part's feel at `tempo`, bars of `beats_per_bar`.
pub fn feel(notes: &[Note], tempo: f64, beats_per_bar: f64) -> Feel {
    let steps = (beats_per_bar * 4.).round().max(1.) as usize;
    let sixteenth = 0.25;
    let ms = 60_000. / tempo.max(1.);
    let bars = notes.iter().map(|note| (note.start / beats_per_bar).floor() as i64 + 1).max().unwrap_or(0).max(1) as usize;
    let starts: Vec<f64> = notes.iter().map(|note| note.start).collect();
    let (grid, swing) = fit_swing(&starts);
    // Each note's step on the swung grid, and how far it sits from the straight one.
    let pushed = push_of(&starts);
    let swung = |step: usize| -> f64 {
        let at = step as f64 * sixteenth + pushed;
        let (unit, odd) = if grid == 16 { (sixteenth, step % 2 == 1) } else { (2. * sixteenth, step % 4 == 2) };
        if odd {
            at - unit + 2. * unit * swing / 100.
        } else {
            at
        }
    };
    let placed: Vec<(usize, usize, f64)> = notes
        .iter()
        .map(|note| {
            let bar = (note.start / beats_per_bar).floor().max(0.) as usize;
            let within = note.start - bar as f64 * beats_per_bar;
            let step = (0..steps).min_by(|a, b| (within - swung(*a)).abs().total_cmp(&(within - swung(*b)).abs())).unwrap_or(0);
            // The last step's neighbour is the next bar's first.
            let (bar, step) = if within - swung(step) > sixteenth * 0.75 && step == steps - 1 { (bar + 1, 0) } else { (bar, step) };
            let straight = bar as f64 * beats_per_bar + step as f64 * sixteenth;
            (bar, step, (note.start - straight) * ms)
        })
        .collect();
    let is_drums = drums(notes);
    let mut names: Vec<&str> = if is_drums { notes.iter().map(|note| drum_lane(note.pitch)).collect() } else { vec!["notes"] };
    names.sort();
    names.dedup();
    let mean = |values: &Vec<f64>| (!values.is_empty()).then(|| round1(values.iter().sum::<f64>() / values.len() as f64));
    let lanes: Vec<Lane> = names
        .iter()
        .map(|name| {
            let members: Vec<usize> = (0..notes.len()).filter(|index| !is_drums || drum_lane(notes[*index].pitch) == *name).collect();
            let mut hit = vec![std::collections::BTreeSet::new(); steps];
            let mut offsets = vec![vec![]; steps];
            let mut velocities = vec![vec![]; steps];
            for &index in &members {
                let (bar, step, offset) = placed[index];
                hit[step].insert(bar);
                offsets[step].push(offset);
                velocities[step].push(notes[index].velocity);
            }
            Lane {
                name: (*name).into(),
                density: hit.iter().map(|bars_hit| round2(bars_hit.len() as f64 / bars as f64)).collect(),
                offset: offsets.iter().map(mean).collect(),
                velocity: velocities.iter().map(mean).collect(),
                notes: members.len(),
            }
        })
        .collect();
    let distance: Vec<f64> = notes
        .iter()
        .zip(&placed)
        .map(|(note, (bar, step, _))| ((note.start - (*bar as f64 * beats_per_bar + swung(*step))) * ms).abs())
        .collect();
    let looseness = round1(distance.iter().sum::<f64>() / distance.len().max(1) as f64);
    let push = round1(push_of(&starts) * ms);
    // Syncopation: notes on weak sixteenths (off the eighths) with nothing on the next eighth.
    let occupied: std::collections::HashSet<(usize, usize)> = placed.iter().map(|(bar, step, _)| (*bar, *step)).collect();
    let weak: Vec<&(usize, usize, f64)> = placed.iter().filter(|(_, step, _)| step % 2 == 1).collect();
    let syncopated = weak
        .iter()
        .filter(|(bar, step, _)| {
            let next = (step + 1) % steps;
            let next_bar = if next == 0 { bar + 1 } else { *bar };
            !occupied.contains(&(next_bar, next))
        })
        .count();
    let syncopation = round2(syncopated as f64 / placed.len().max(1) as f64);
    // Fills: the last bar of each 4-bar phrase against the rest, when there are two phrases or more.
    let per_bar: Vec<usize> = (0..bars).map(|bar| placed.iter().filter(|(at, _, _)| *at == bar).count()).collect();
    let fills = (bars >= 8).then(|| {
        let (ends, rest): (Vec<(usize, usize)>, Vec<(usize, usize)>) =
            per_bar.iter().copied().enumerate().partition(|(bar, _)| bar % 4 == 3);
        let mean = |part: &[(usize, usize)]| part.iter().map(|(_, count)| *count as f64).sum::<f64>() / part.len().max(1) as f64;
        round2(mean(&ends) / mean(&rest).max(0.5))
    });
    let velocities: Vec<f64> = notes.iter().map(|note| note.velocity).collect();
    let mean = velocities.iter().sum::<f64>() / velocities.len().max(1) as f64;
    let velocity_spread = round1((velocities.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / velocities.len().max(1) as f64).sqrt());
    Feel { tempo, steps, bars, lanes, grid, swing, looseness, push, syncopation, fills, velocity_spread }
}

/// How far a part's feel is from a reference's, item by item: what the groove checklist reads.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Gaps {
    /// Swing points apart.
    pub swing: f64,
    /// Per lane both have: mean timing difference over the steps both play, ms.
    pub timing: Vec<(String, f64)>,
    /// Per lane both have: mean velocity difference over the steps both play.
    pub velocity: Vec<(String, f64)>,
    /// Per lane: mean density difference over all steps (where they play, how often).
    pub density: Vec<(String, f64)>,
    pub syncopation: f64,
    pub fills: Option<f64>,
}

/// The gaps between a part and a reference, lane by lane (lanes only one of them has count as density only).
pub fn gaps(part: &Feel, reference: &Feel) -> Gaps {
    let mut timing = vec![];
    let mut velocity = vec![];
    let mut density = vec![];
    for wanted in &reference.lanes {
        let Some(lane) = part.lanes.iter().find(|lane| lane.name == wanted.name) else {
            density.push((wanted.name.clone(), round2(wanted.density.iter().sum::<f64>() / wanted.density.len().max(1) as f64)));
            continue;
        };
        let both = |a: &[Option<f64>], b: &[Option<f64>]| -> Option<f64> {
            let pairs: Vec<f64> = a.iter().zip(b).filter_map(|(x, y)| Some((x.as_ref()? - y.as_ref()?).abs())).collect();
            (!pairs.is_empty()).then(|| round1(pairs.iter().sum::<f64>() / pairs.len() as f64))
        };
        if let Some(gap) = both(&lane.offset, &wanted.offset) {
            timing.push((wanted.name.clone(), gap));
        }
        if let Some(gap) = both(&lane.velocity, &wanted.velocity) {
            velocity.push((wanted.name.clone(), gap));
        }
        let apart: f64 =
            lane.density.iter().zip(&wanted.density).map(|(a, b)| (a - b).abs()).sum::<f64>() / wanted.density.len().max(1) as f64;
        density.push((wanted.name.clone(), round2(apart)));
    }
    Gaps {
        swing: round1((part.swing - reference.swing).abs()),
        timing,
        velocity,
        density,
        syncopation: round2((part.syncopation - reference.syncopation).abs()),
        fills: match (part.fills, reference.fills) {
            (Some(a), Some(b)) => Some(round2((a - b).abs())),
            _ => None,
        },
    }
}

/// The part's notes moved toward the reference's feel: each note to the reference's timing on its step (and lane),
/// its velocity toward the reference's there, by `amount` (0–1). Notes on steps the reference doesn't play stay.
pub fn toward(notes: &[Note], part: &Feel, reference: &Feel, beats_per_bar: f64, amount: f64) -> Vec<Note> {
    let ms = 60_000. / part.tempo.max(1.);
    let is_drums = drums(notes);
    notes
        .iter()
        .map(|note| {
            let lane_name = if is_drums { drum_lane(note.pitch) } else { "notes" };
            let Some(wanted) = reference.lanes.iter().find(|lane| lane.name == lane_name) else { return *note };
            let bar = (note.start / beats_per_bar).floor().max(0.);
            let within = note.start - bar * beats_per_bar;
            let step = ((within / 0.25).round() as usize).min(part.steps - 1);
            let straight = bar * beats_per_bar + step as f64 * 0.25;
            let mut moved = *note;
            if let Some(offset) = wanted.offset.get(step).copied().flatten() {
                let target = straight + offset / ms;
                moved.start = (note.start + (target - note.start) * amount).max(0.);
            }
            if let Some(velocity) = wanted.velocity.get(step).copied().flatten() {
                moved.velocity = (note.velocity + (velocity - note.velocity) * amount).clamp(1., 127.).round();
            }
            moved
        })
        .collect()
}

fn round1(value: f64) -> f64 {
    (value * 10.).round() / 10.
}

fn round2(value: f64) -> f64 {
    (value * 100.).round() / 100.
}

/// Where hits start in a recording, seconds: the rises of a short-window energy envelope (a quarter of a millisecond
/// apart, on the signal's first difference, so lows don't smear them), each at least 30 ms after the last.
pub fn onsets(samples: &[f32], rate: f64) -> Vec<f64> {
    let hop = ((rate / 1000.).round() as usize).max(1);
    let window = hop * 4;
    if samples.len() < window * 4 {
        return vec![];
    }
    let energy: Vec<f64> = (0..(samples.len() - window) / hop)
        .map(|frame| {
            let at = frame * hop;
            let sum: f64 = (at + 1..at + window).map(|n| ((samples[n] - samples[n - 1]) as f64).powi(2)).sum();
            10. * (sum / window as f64 + 1e-12).log10()
        })
        .collect();
    let rise: Vec<f64> = energy.windows(4).map(|run| (run[3] - run[0]).max(0.)).collect();
    let mut sorted = rise.clone();
    sorted.sort_by(f64::total_cmp);
    let threshold = sorted[sorted.len() * 9 / 10].max(6.);
    let gap = (0.03 * rate / hop as f64) as usize;
    let mut found: Vec<f64> = vec![];
    let mut last: Option<usize> = None;
    for (frame, value) in rise.iter().enumerate() {
        let peak = *value >= threshold && (frame == 0 || rise[frame - 1] < *value) && rise.get(frame + 1).is_none_or(|next| next <= value);
        if peak && last.is_none_or(|at| frame - at >= gap) {
            found.push(((frame + 2) * hop) as f64 / rate);
            last = Some(frame);
        }
    }
    found
}

/// Notes moved onto the hits they stand for: each to the nearest onset within `within` beats (onsets in beats too).
/// Drums to MIDI places hits on its own analysis grid; the recording says where they really are.
pub fn refine(notes: &mut [Note], onsets: &[f64], within: f64) -> usize {
    let mut moved = 0;
    for note in notes.iter_mut() {
        let nearest = onsets.iter().copied().min_by(|a, b| (a - note.start).abs().total_cmp(&(b - note.start).abs()));
        if let Some(at) = nearest.filter(|at| (at - note.start).abs() <= within) {
            if (at - note.start).abs() > 1e-6 {
                note.start = at.max(0.);
                moved += 1;
            }
        }
    }
    moved
}

/// One line of a groove checklist: a gap to the reference, and how small it has to get.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Line {
    pub id: String,
    pub label: String,
    pub unit: String,
    pub value: f64,
    pub within: f64,
    /// One just-noticeable step, in the unit.
    pub step: f64,
}

impl Line {
    /// How many steps it's still off.
    pub fn off(&self) -> f64 {
        ((self.value - self.within) / self.step).max(0.)
    }
}

/// The gaps as a checklist: swing, each shared lane's timing and velocity, where each lane plays, syncopation and
/// fills, each with its tolerance.
pub fn lines(gaps: &Gaps) -> Vec<Line> {
    let line = |id: String, label: String, unit: &str, value: f64, within: f64, step: f64| Line {
        id,
        label,
        unit: unit.into(),
        value,
        within,
        step,
    };
    let mut lines = vec![line("swing".into(), "Swing apart".into(), "points", gaps.swing, 2., 1.)];
    for (lane, value) in &gaps.timing {
        lines.push(line(format!("timing {lane}"), format!("Timing, {lane}"), "ms", *value, 5., 2.5));
    }
    for (lane, value) in &gaps.velocity {
        lines.push(line(format!("velocity {lane}"), format!("Velocity, {lane}"), "", *value, 6., 4.));
    }
    for (lane, value) in &gaps.density {
        lines.push(line(format!("density {lane}"), format!("Where the {lane} plays"), "", *value, 0.1, 0.05));
    }
    lines.push(line("syncopation".into(), "Syncopation apart".into(), "", gaps.syncopation, 0.05, 0.03));
    if let Some(fills) = gaps.fills {
        lines.push(line("fills".into(), "Fills apart".into(), "", fills, 0.25, 0.15));
    }
    lines
}
