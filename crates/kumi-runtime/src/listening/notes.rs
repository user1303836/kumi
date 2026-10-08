//! Sequencing measured on the notes, exactly and cheaply: where in the bar a part plays (how its notes cluster), how
//! early or late each grid step sits (its timing profile, and the swing in it), how hard (its velocity profile), how
//! much it plays off the beat (syncopation) and how its phrase ends differ (fills). A part on a Drum Rack splits into
//! lanes (kick, snare, hats, the rest), named from its pads; any other part is one lane.

use serde::{Deserialize, Serialize};
use std::f64::consts::TAU;

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

/// How a part's notes fall into lanes: a Drum Rack's by its pads, each lane what its pad holds (kick, snare, hats,
/// cymbals, the rest as perc: by the pad's name, else by its note on the General MIDI drum map); any other part's, one
/// lane.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Kit {
    /// Whether the part plays a Drum Rack.
    pub drums: bool,
    /// The rack's pads that hold something: each one's note and lane.
    pub pads: Vec<(i32, String)>,
}

impl Kit {
    /// A pitched part's: one lane.
    pub fn pitched() -> Self {
        Self::default()
    }

    /// A Drum Rack's, from its pads that hold something (name and note).
    pub fn rack(pads: &[(String, u8)]) -> Self {
        Self { drums: true, pads: pads.iter().map(|(name, note)| (*note as i32, pad_lane(name, *note as i32).to_string())).collect() }
    }

    /// A note's lane.
    pub fn lane(&self, pitch: i32) -> &str {
        if !self.drums {
            return "notes";
        }
        self.pads.iter().find(|(note, _)| *note == pitch).map_or_else(|| drum_lane(pitch), |(_, lane)| lane.as_str())
    }
}

/// A part's feel.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Feel {
    pub tempo: f64,
    pub steps: usize,
    pub bars: usize,
    /// How its notes fall into lanes.
    pub kit: Kit,
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

/// A Drum Rack pad's lane, by what its name says it holds, else by its note on the General MIDI drum map.
pub fn pad_lane(name: &str, note: i32) -> &'static str {
    let name = name.to_lowercase();
    let words: Vec<&str> = name.split(|c: char| !c.is_alphanumeric()).filter(|word| !word.is_empty()).collect();
    let says =
        |parts: &[&str], short: &[&str]| parts.iter().any(|part| name.contains(part)) || words.iter().any(|word| short.contains(word));
    if says(&["kick", "kik", "bassdrum", "bass drum"], &["bd"]) {
        "kick"
    } else if says(&["hat", "hihat"], &["hh", "oh", "ch"]) {
        "hats"
    } else if says(&["snare", "clap", "rim", "snap"], &["sd", "snr", "clp", "cp"]) {
        "snare"
    } else if says(&["crash", "ride", "cymbal", "china", "splash"], &["cym"]) {
        "cymbals"
    } else if says(&["tom", "conga", "bongo", "shaker", "perc", "cowbell", "tamb", "clave"], &[]) {
        "perc"
    } else {
        drum_lane(note)
    }
}

/// The swung grid that explains the notes best: eighths or sixteenths, and the swing (50–75 %), by how close the
/// notes sit to it. Sixteenths have to explain them clearly better, being twice as dense.
pub fn fit_swing(starts: &[f64]) -> (u32, f64) {
    // Lateness all round isn't swing: the part's push (how it sits on the beats) comes out first.
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

/// How far ahead (−) or behind (+) the part sits on the beat, in beats: the median offset of the notes near a beat
/// (within a sixteenth's half), which neither eighth nor sixteenth swing moves.
pub fn push_of(starts: &[f64]) -> f64 {
    let mut near: Vec<f64> = starts.iter().map(|start| start - start.round()).filter(|offset| offset.abs() <= 0.125).collect();
    if near.is_empty() {
        return 0.;
    }
    near.sort_by(f64::total_cmp);
    near[near.len() / 2]
}

/// Where a step of the bar sits on the swung grid, in beats from the bar's start: `grid` 8 or 16, `swing` its swing
/// (50 straight), `pushed` the part's push in beats.
fn swung(step: usize, grid: u32, swing: f64, pushed: f64) -> f64 {
    let sixteenth = 0.25;
    let at = step as f64 * sixteenth + pushed;
    let (unit, odd) = if grid == 16 { (sixteenth, step % 2 == 1) } else { (2. * sixteenth, step % 4 == 2) };
    if odd {
        at - unit + 2. * unit * swing / 100.
    } else {
        at
    }
}

/// Each note's place: its bar, its step on the swung grid (a late last step is the next bar's first), and how far it
/// sits from that step's straight place, ms. Measuring and moving notes place them alike.
fn place(notes: &[Note], beats_per_bar: f64, steps: usize, (grid, swing, pushed): (u32, f64, f64), ms: f64) -> Vec<(usize, usize, f64)> {
    let sixteenth = 0.25;
    let at = |step: usize| swung(step, grid, swing, pushed);
    notes
        .iter()
        .map(|note| {
            let bar = (note.start / beats_per_bar).floor().max(0.) as usize;
            let within = note.start - bar as f64 * beats_per_bar;
            let step = (0..steps).min_by(|a, b| (within - at(*a)).abs().total_cmp(&(within - at(*b)).abs())).unwrap_or(0);
            // The last step's neighbour is the next bar's first.
            let (bar, step) = if within - at(step) > sixteenth * 0.75 && step == steps - 1 { (bar + 1, 0) } else { (bar, step) };
            let straight = bar as f64 * beats_per_bar + step as f64 * sixteenth;
            (bar, step, (note.start - straight) * ms)
        })
        .collect()
}

/// The part's feel at `tempo`, bars of `beats_per_bar`, its notes in lanes by `kit`.
pub fn feel(notes: &[Note], tempo: f64, beats_per_bar: f64, kit: &Kit) -> Feel {
    let steps = (beats_per_bar * 4.).round().max(1.) as usize;
    let ms = 60_000. / tempo.max(1.);
    let bars = notes.iter().map(|note| (note.start / beats_per_bar).floor() as i64 + 1).max().unwrap_or(0).max(1) as usize;
    let starts: Vec<f64> = notes.iter().map(|note| note.start).collect();
    let (grid, swing) = fit_swing(&starts);
    // Each note's step on the swung grid, and how far it sits from the straight one.
    let pushed = push_of(&starts);
    let placed = place(notes, beats_per_bar, steps, (grid, swing, pushed), ms);
    // How often a step is played is counted over the bars the part plays in: a reference's stretches without it (a
    // song's drum stem through its breaks) aren't read as it playing sparsely.
    let playing = placed.iter().map(|(bar, _, _)| *bar).collect::<std::collections::BTreeSet<_>>().len().max(1);
    let mut names: Vec<&str> = notes.iter().map(|note| kit.lane(note.pitch)).collect();
    names.sort();
    names.dedup();
    let mean = |values: &Vec<f64>| (!values.is_empty()).then(|| round1(values.iter().sum::<f64>() / values.len() as f64));
    let lanes: Vec<Lane> = names
        .iter()
        .map(|name| {
            let members: Vec<usize> = (0..notes.len()).filter(|index| kit.lane(notes[*index].pitch) == *name).collect();
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
                density: hit.iter().map(|bars_hit| round2(bars_hit.len() as f64 / playing as f64)).collect(),
                offset: offsets.iter().map(mean).collect(),
                velocity: velocities.iter().map(mean).collect(),
                notes: members.len(),
            }
        })
        .collect();
    let distance: Vec<f64> = notes
        .iter()
        .zip(&placed)
        .map(|(note, (bar, step, _))| ((note.start - (*bar as f64 * beats_per_bar + swung(*step, grid, swing, pushed))) * ms).abs())
        .collect();
    let looseness = round1(distance.iter().sum::<f64>() / distance.len().max(1) as f64);
    let push = round1(pushed * ms);
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
    Feel { tempo, steps, bars, kit: kit.clone(), lanes, grid, swing, looseness, push, syncopation, fills, velocity_spread }
}

impl Feel {
    /// Its velocities left out (a transcription's are how sure it was of a note, not how hard it was played): no
    /// velocity lines against it, and moving notes toward it leaves their velocities alone.
    pub fn without_velocities(mut self) -> Self {
        for lane in &mut self.lanes {
            lane.velocity.fill(None);
        }
        self
    }

    /// One of its lanes, by name.
    pub fn lane(&self, name: &str) -> Option<&Lane> {
        self.lanes.iter().find(|lane| lane.name == name)
    }
}

/// The index of the largest value, the first of equals.
fn largest(values: impl Iterator<Item = (usize, f64)>) -> Option<usize> {
    values
        .fold(None, |best: Option<(usize, f64)>, (index, value)| match best {
            Some((_, top)) if top >= value => best,
            _ => Some((index, value)),
        })
        .map(|(index, _)| index)
}

/// A recording's beat grid, fitted to its notes' onsets.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Grid {
    pub tempo: f64,
    /// Seconds: the first downbeat at or before the first onset (beat one of the bar the recording starts in).
    pub downbeat: f64,
}

/// A recording's beat grid fitted to its notes' onsets (seconds, each with its weight): the tempo searched within 2 %
/// of `tempo` (and of half and double it, with `octaves`) for the one whose sixteenths the onsets sit on most closely,
/// its phase, and the sixteenth that starts the bar: the one that puts the most weight on the bar's strongest places
/// (its downbeat, its middle, the beats, the eighths). Of tempos that explain the onsets about as well (half and double
/// often do), the one nearest `near`.
pub fn fit_grid(onsets: &[(f64, f64)], tempo: f64, beats_per_bar: f64, octaves: bool, near: Option<f64>) -> Grid {
    let weight: f64 = onsets.iter().map(|(_, weight)| weight).sum();
    let first = onsets.iter().map(|(at, _)| *at).fold(f64::MAX, f64::min);
    if onsets.len() < 8 || weight <= 0. || tempo <= 0. {
        return Grid { tempo, downbeat: if first.is_finite() { first.max(0.) } else { 0. } };
    }
    // How closely the onsets sit on a tempo's sixteenths (0–1), and where those fall (0–1 of a sixteenth).
    let resultant = |tempo: f64| -> (f64, f64) {
        let period = 15. / tempo;
        let (x, y) = onsets.iter().fold((0., 0.), |(x, y), (at, weight)| {
            let angle = TAU * at / period;
            (x + weight * angle.cos(), y + weight * angle.sin())
        });
        ((x * x + y * y).sqrt() / weight, y.atan2(x).rem_euclid(TAU) / TAU)
    };
    // Within 2 % in steps of 0.01 %, then finer around the best: over two minutes, a tempo off by 0.01 % drifts 12 ms.
    let search = |around: f64| -> (f64, f64) {
        let mut best = (around, resultant(around).0);
        for step in -200..=200 {
            let candidate = around * (1. + step as f64 * 1e-4);
            let strength = resultant(candidate).0;
            if strength > best.1 {
                best = (candidate, strength);
            }
        }
        let coarse = best.0;
        for step in -60..=60 {
            let candidate = coarse * (1. + step as f64 * 2.5e-6);
            let strength = resultant(candidate).0;
            if strength > best.1 {
                best = (candidate, strength);
            }
        }
        best
    };
    let mut candidates = vec![search(tempo)];
    if octaves {
        candidates.extend([search(tempo / 2.), search(tempo * 2.)]);
    }
    let strongest = candidates.iter().map(|(_, strength)| *strength).fold(0., f64::max);
    let near = near.unwrap_or(tempo);
    let tempo = candidates
        .iter()
        .filter(|(_, strength)| *strength >= strongest * 0.9)
        .min_by(|a, b| (a.0 / near).ln().abs().total_cmp(&(b.0 / near).ln().abs()))
        .map_or(tempo, |(tempo, _)| *tempo);
    let period = 15. / tempo;
    let origin = resultant(tempo).1 * period;
    // The weight on each sixteenth of the bar.
    let slots = (beats_per_bar * 4.).round().max(4.) as usize;
    let mut per_slot = vec![0.; slots];
    for (at, weight) in onsets {
        per_slot[(((at - origin) / period).round() as i64).rem_euclid(slots as i64) as usize] += weight;
    }
    let salience = |slot: usize| match slot {
        0 => 4.,
        _ if slots.is_multiple_of(8) && slot == slots / 2 => 3.,
        _ if slot.is_multiple_of(4) => 2.,
        _ if slot.is_multiple_of(2) => 1.5,
        _ => 1.,
    };
    let downbeat = largest((0..slots).map(|first| (first, (0..slots).map(|slot| per_slot[(first + slot) % slots] * salience(slot)).sum())))
        .unwrap_or(0);
    let bar = slots as f64 * period;
    let mut at = origin + downbeat as f64 * period;
    at += ((first + period / 2. - at) / bar).floor() * bar;
    Grid { tempo, downbeat: at }
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
    // Swing on eighths and swing on sixteenths aren't the same feel, however alike their numbers: on different grids,
    // each one's swing counts as apart.
    let swing =
        if part.grid == reference.grid { (part.swing - reference.swing).abs() } else { (part.swing - 50.) + (reference.swing - 50.) };
    Gaps {
        swing: round1(swing),
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

/// The part's notes moved toward the reference's feel, by `amount` (0–1): each step's notes (in each lane) by how far
/// the reference's timing and velocity there are from the part's, so each note keeps its own lean (a roll, a flam).
/// Notes are placed on steps as `feel` places them (`part` is the feel of these notes); notes on steps the reference
/// doesn't play stay.
pub fn toward(notes: &[Note], part: &Feel, reference: &Feel, beats_per_bar: f64, amount: f64) -> Vec<Note> {
    let ms = 60_000. / part.tempo.max(1.);
    let starts: Vec<f64> = notes.iter().map(|note| note.start).collect();
    let placed = place(notes, beats_per_bar, part.steps, (part.grid, part.swing, push_of(&starts)), ms);
    notes
        .iter()
        .zip(&placed)
        .map(|(note, (_, step, _))| {
            let name = part.kit.lane(note.pitch);
            let (Some(ours), Some(wanted)) = (part.lane(name), reference.lane(name)) else { return *note };
            let at = |values: &[Option<f64>]| values.get(*step).copied().flatten();
            let mut moved = *note;
            if let (Some(have), Some(want)) = (at(&ours.offset), at(&wanted.offset)) {
                moved.start = (note.start + (want - have) / ms * amount).max(0.);
            }
            if let (Some(have), Some(want)) = (at(&ours.velocity), at(&wanted.velocity)) {
                moved.velocity = (note.velocity + (want - have) * amount).clamp(1., 127.).round();
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
