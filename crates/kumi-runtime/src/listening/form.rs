//! A song's form, heard: the energy curve bar by bar (loudness, density, brightness, low end), the sections it falls
//! into (where they start and end, their lengths, roles and repeats), the transitions between them (drops, gaps,
//! risers), the intro and outro, and the time to the first hook. Then the problems a form can have: neighbouring
//! sections without contrast, energy that plateaus, odd phrase lengths, a drop that arrives unprepared, and a stretch
//! left unchanged too long. Which elements play where comes from the Arrangement, read by the caller.

use super::measure::{Heard, THIRDS};
use serde::{Deserialize, Serialize};

/// One bar of the energy curve.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Bar {
    /// Its level, dB of full scale's power.
    pub loudness: f64,
    /// Onsets a second.
    pub density: f64,
    /// Its spectrum's centre, in octaves above 20 Hz.
    pub brightness: f64,
    /// Its share below 120 Hz, dB.
    pub low: f64,
    /// Silent for most of it: a gap, a lead-in or a tail.
    #[serde(default)]
    pub silent: bool,
}

/// A section: its bars (from 0, end not included), its role, and the letter it shares with the sections it repeats.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Section {
    pub from: usize,
    pub to: usize,
    pub role: String,
    pub letter: char,
    /// Its mean level, dB.
    pub loudness: f64,
}

/// How one section turns into the next, at bar `at`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Transition {
    pub at: usize,
    /// A drop (energy jumps up), a fall (it falls away), or a change.
    pub kind: String,
    /// What leads into it: a gap (a dip just before), a riser (brightness or density climbing over the bars before).
    pub prepared: Vec<String>,
}

/// A song's form.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Form {
    pub bars: Vec<Bar>,
    pub sections: Vec<Section>,
    pub transitions: Vec<Transition>,
    /// Bars before the first section that isn't an intro, and after the last that isn't an outro.
    pub intro: usize,
    pub outro: usize,
    /// The bar the first hook (the first section at the song's peak energy) starts on.
    pub hook: Option<usize>,
    /// The longest stretch of bars that barely change, and where it starts.
    pub unchanged: (usize, usize),
    pub problems: Vec<String>,
}

/// The form of what was heard, `bar` seconds a bar.
pub fn form(heard: &Heard, bar: f64) -> Form {
    let bars = floored(curve(heard, bar));
    let sections = sections(&bars);
    let transitions = transitions(&bars, &sections);
    let peak = sections.iter().map(|section| section.loudness).fold(f64::MIN, f64::max);
    let intro = sections.first().filter(|section| section.role == "intro").map_or(0, |section| section.to);
    let outro = sections.last().filter(|section| section.role == "outro").map_or(0, |section| section.to - section.from);
    let hook = sections.iter().find(|section| section.loudness >= peak - 1.5).map(|section| section.from);
    let unchanged = unchanged(&bars);
    let mut form = Form { bars, sections, transitions, intro, outro, hook, unchanged, problems: vec![] };
    form.problems = problems(&form);
    form
}

/// The energy curve, bar by bar.
pub fn curve(heard: &Heard, bar: f64) -> Vec<Bar> {
    let frames = &heard.frames;
    if frames.hop <= 0. || bar <= 0. || frames.level.is_empty() {
        return vec![];
    }
    let per_bar = (bar / frames.hop).max(1.);
    // A last bar that's mostly there counts (the frames stop a window short of the end).
    let count = (frames.level.len() as f64 / per_bar + 0.25).floor() as usize;
    // Onsets: frames whose summed rise across the bands stands well over the song's usual rise.
    let flux: Vec<f64> = (0..frames.mid.len())
        .map(|frame| {
            if frame == 0 {
                return 0.;
            }
            (0..31)
                .map(|band| {
                    let (now, was) = (frames.mid[frame][band] as f64 + 1e-12, frames.mid[frame - 1][band] as f64 + 1e-12);
                    (10. * (now / was).log10()).max(0.)
                })
                .sum()
        })
        .collect();
    let mut sorted = flux.clone();
    sorted.sort_by(f64::total_cmp);
    let typical = sorted.get(sorted.len() / 2).copied().unwrap_or(0.);
    let spread = sorted.get(sorted.len() * 9 / 10).copied().unwrap_or(typical) - typical;
    let line = typical + spread.max(1.) * 1.5;
    let onset: Vec<bool> = (0..flux.len())
        .map(|at| flux[at] > line && (at == 0 || flux[at] >= flux[at - 1]) && flux.get(at + 1).is_none_or(|next| flux[at] >= *next))
        .collect();
    let low_bands = THIRDS.iter().filter(|hz| **hz < 120.).count();
    let loudest = frames.level.iter().copied().fold(f32::MIN, f32::max) as f64;
    (0..count)
        .map(|index| {
            let range = (index as f64 * per_bar) as usize..(((index + 1) as f64 * per_bar) as usize).min(frames.level.len());
            let power: f64 =
                range.clone().map(|frame| 10f64.powf(frames.level[frame] as f64 / 10.)).sum::<f64>() / range.len().max(1) as f64;
            let bands: Vec<f64> = (0..31)
                .map(|band| range.clone().map(|frame| (frames.mid[frame][band] + frames.side[frame][band]) as f64).sum::<f64>())
                .collect();
            let total: f64 = bands.iter().sum::<f64>() + 1e-20;
            let centre = bands.iter().zip(THIRDS.iter()).map(|(power, hz)| power * (hz / 20.).log2()).sum::<f64>() / total;
            let low: f64 = bands[..low_bands].iter().sum();
            Bar {
                loudness: 10. * (power + 1e-20).log10(),
                density: range.clone().filter(|frame| onset.get(*frame).copied().unwrap_or(false)).count() as f64 / bar,
                brightness: centre,
                low: 10. * ((low + 1e-20) / total).log10(),
                silent: range.clone().filter(|frame| (frames.level[*frame] as f64) < loudest - SILENT).count() * 2 >= range.len().max(1),
            }
        })
        .collect()
}

/// How far under the loudest a frame or a bar is silent (a gap, a lead-in, a tail): a silent bar's loudness is held
/// there.
const SILENT: f64 = 60.;

/// Which bars are silent: most of their frames, or the whole bar, `SILENT` dB and more under the loudest.
fn silent(bars: &[Bar]) -> Vec<bool> {
    let loudest = bars.iter().map(|bar| bar.loudness).fold(f64::MIN, f64::max);
    bars.iter().map(|bar| bar.silent || bar.loudness <= loudest - SILENT + 1e-9).collect()
}

/// The curve with its silent bars held at the floor (`SILENT` dB under the loudest) and given the density, brightness
/// and low end of the nearest bar that's heard: digital silence reads -197 dB with no spectrum, which would swamp the
/// song's scale and read as a riser out of nothing. A silent bar stays a gap (a turn's preparation).
fn floored(mut bars: Vec<Bar>) -> Vec<Bar> {
    let quiet = silent(&bars);
    let floor = bars.iter().map(|bar| bar.loudness).fold(f64::MIN, f64::max) - SILENT;
    for index in 0..bars.len() {
        if !quiet[index] {
            continue;
        }
        let near =
            (1..bars.len()).flat_map(|by| [index.checked_sub(by), Some(index + by)]).flatten().find(|at| *at < bars.len() && !quiet[*at]);
        bars[index] = Bar { loudness: floor, silent: true, ..near.map_or(bars[index], |at| bars[at]) };
    }
    bars
}

/// The least each measure's spread is taken as: about one noticeable step (2 dB of loudness, an onset a second, a
/// quarter of an octave of brightness, 2 dB of low end), so a song that barely moves isn't read as one that does.
const FLOORS: [f64; 4] = [2., 1., 0.25, 2.];

/// The bars as one vector each, every measure put on the song's own scale (how far from its middle, in its spread):
/// the scale of the bars that are heard, silent ones left out of it.
fn scaled(bars: &[Bar]) -> Vec<[f64; 4]> {
    let quiet = silent(bars);
    let columns: Vec<Vec<f64>> = vec![
        bars.iter().map(|bar| bar.loudness).collect(),
        bars.iter().map(|bar| bar.density).collect(),
        bars.iter().map(|bar| bar.brightness).collect(),
        bars.iter().map(|bar| bar.low).collect(),
    ];
    let scales: Vec<(f64, f64)> = columns
        .iter()
        .zip(FLOORS)
        .map(|(column, floor)| {
            let heard: Vec<f64> = column.iter().zip(&quiet).filter(|(_, quiet)| !**quiet).map(|(value, _)| *value).collect();
            let mean = heard.iter().sum::<f64>() / heard.len().max(1) as f64;
            let spread = (heard.iter().map(|value| (value - mean).powi(2)).sum::<f64>() / heard.len().max(1) as f64).sqrt();
            (mean, spread.max(floor))
        })
        .collect();
    // Loudness counts twice: a section is first of all how loud it is.
    let weights = [2., 1., 1., 1.];
    (0..bars.len())
        .map(|index| {
            let mut row = [0.; 4];
            for (column, ((values, (mean, spread)), weight)) in columns.iter().zip(&scales).zip(weights).enumerate() {
                row[column] = (values[index] - mean) / spread * weight;
            }
            row
        })
        .collect()
}

fn distance(a: &[f64; 4], b: &[f64; 4]) -> f64 {
    a.iter().zip(b).map(|(x, y)| (x - y).powi(2)).sum::<f64>().sqrt()
}

fn mean(rows: &[[f64; 4]]) -> [f64; 4] {
    let mut sum = [0.; 4];
    for row in rows {
        for (total, value) in sum.iter_mut().zip(row) {
            *total += value;
        }
    }
    sum.map(|total| total / rows.len().max(1) as f64)
}

/// The sections: cut where the bars on either side differ most (four bars each way), at least four bars apart; each
/// with its role (by how its energy sits in the song) and a letter shared with the sections it sounds like. Silent bars
/// aren't compared (the bars heard on either side of them are), and no section starts on one: a gap ends the section
/// before it.
pub fn sections(bars: &[Bar]) -> Vec<Section> {
    if bars.is_empty() {
        return vec![];
    }
    let rows = scaled(bars);
    let quiet = silent(bars);
    let heard: Vec<usize> = (0..bars.len()).filter(|at| !quiet[*at]).collect();
    let width = 4;
    let novelty: Vec<f64> = (0..=bars.len())
        .map(|at| {
            if at < 2 || at + 2 > bars.len() || quiet.get(at) == Some(&true) {
                return 0.;
            }
            let before: Vec<[f64; 4]> = heard.iter().rev().filter(|bar| **bar < at).take(width).map(|bar| rows[*bar]).collect();
            let after: Vec<[f64; 4]> = heard.iter().filter(|bar| **bar >= at).take(width).map(|bar| rows[*bar]).collect();
            if before.is_empty() || after.is_empty() {
                return 0.;
            }
            distance(&mean(&before), &mean(&after))
        })
        .collect();
    let mut ranked: Vec<usize> = (1..bars.len()).filter(|at| novelty[*at] > 0.).collect();
    ranked.sort_by(|a, b| novelty[*b].total_cmp(&novelty[*a]));
    let mean_novelty = novelty.iter().sum::<f64>() / novelty.len().max(1) as f64;
    let mut cuts: Vec<usize> = vec![];
    for at in ranked {
        // Only a clear change (a step and a half at least), never closer than four bars to another cut or an end.
        if novelty[at] < (mean_novelty * 1.2).max(1.5) || at < 4 || at + 4 > bars.len() {
            continue;
        }
        if cuts.iter().all(|cut| cut.abs_diff(at) >= 4) {
            cuts.push(at);
        }
    }
    cuts.sort_unstable();
    let mut edges = vec![0];
    edges.extend(cuts);
    edges.push(bars.len());
    let spans: Vec<(usize, usize)> = edges.windows(2).map(|pair| (pair[0], pair[1])).collect();
    let level = |(from, to): (usize, usize)| {
        let power: f64 = bars[from..to].iter().map(|bar| 10f64.powf(bar.loudness / 10.)).sum::<f64>() / (to - from).max(1) as f64;
        10. * (power + 1e-20).log10()
    };
    let levels: Vec<f64> = spans.iter().map(|span| level(*span)).collect();
    let peak = levels.iter().copied().fold(f64::MIN, f64::max);
    let last = spans.len() - 1;
    let mut letters: Vec<([f64; 4], char)> = vec![];
    spans
        .iter()
        .enumerate()
        .map(|(index, (from, to))| {
            let loudness = levels[index];
            // Rising: its second half louder than its first by 2 dB (its last bar left out: a window there hears the next
            // section's start).
            let rising = to - from >= 4 && {
                let body: Vec<f64> = (*from..to - 1).filter(|at| !quiet[*at]).map(|at| bars[at].loudness).collect();
                let half = body.len() / 2;
                let level = |levels: &[f64]| levels.iter().sum::<f64>() / levels.len().max(1) as f64;
                body.len() >= 2 && level(&body[half..]) - level(&body[..half]) > 2.
            };
            let role = if loudness >= peak - 1.5 {
                "peak"
            } else if index == 0 && spans.len() > 1 {
                "intro"
            } else if index == last && spans.len() > 1 {
                "outro"
            } else if rising && levels.get(index + 1).is_some_and(|next| *next >= peak - 1.5) {
                "build"
            } else if index > 0 && levels[index - 1] - loudness > 3. && levels.get(index + 1).is_some_and(|next| next - loudness > 3.) {
                "break"
            } else {
                "verse"
            };
            let heard: Vec<[f64; 4]> = (*from..*to).filter(|at| !quiet[*at]).map(|at| rows[at]).collect();
            let centre = mean(if heard.is_empty() { &rows[*from..*to] } else { &heard[..] });
            let letter = match letters.iter().find(|(known, _)| distance(known, &centre) < 0.75) {
                Some((_, letter)) => *letter,
                None => {
                    let letter = (b'A' + letters.len().min(25) as u8) as char;
                    letters.push((centre, letter));
                    letter
                }
            };
            Section { from: *from, to: *to, role: role.into(), letter, loudness: round1(loudness) }
        })
        .collect()
}

/// How each section turns into the next: a drop or a fall by 3 dB and more, and what leads into it.
fn transitions(bars: &[Bar], sections: &[Section]) -> Vec<Transition> {
    sections
        .windows(2)
        .map(|pair| {
            let (before, after) = (&pair[0], &pair[1]);
            let at = after.from;
            let kind = if after.loudness - before.loudness >= 3. {
                "drop"
            } else if before.loudness - after.loudness >= 3. {
                "fall"
            } else {
                "change"
            };
            let mut prepared = vec![];
            // A gap: the bar before the turn dips well under the section's level.
            if at >= 1 && before.loudness - bars[at - 1].loudness >= 3. {
                prepared.push("gap".to_string());
            }
            // A riser: brightness or density climbing over the bars before (up to the bar before last: the last one
            // hears the turn's start).
            let lead = at.saturating_sub(5).max(before.from);
            if at >= 2 && at - 2 > lead {
                let climbs = |value: fn(&Bar) -> f64, by: f64| value(&bars[at - 2]) - value(&bars[lead]) > by;
                if climbs(|bar| bar.brightness, 0.3) || climbs(|bar| bar.density, 1.) {
                    prepared.push("riser".to_string());
                }
            }
            if before.role == "build" {
                prepared.push("build".to_string());
            }
            Transition { at, kind: kind.into(), prepared }
        })
        .collect()
}

/// The longest run of bars that barely change from one to the next (and where it starts).
fn unchanged(bars: &[Bar]) -> (usize, usize) {
    let rows = scaled(bars);
    let (mut best, mut start, mut run_start) = (0, 0, 0);
    for at in 1..=rows.len() {
        let still = at < rows.len() && distance(&rows[at], &rows[run_start]) < 1.25;
        if !still {
            if at - run_start > best {
                best = at - run_start;
                start = run_start;
            }
            run_start = at;
        }
    }
    (best, start)
}

/// What's wrong with the form, in words: sections without contrast, a plateau, odd phrase lengths, an unprepared drop,
/// and a stretch left unchanged for 32 bars and more.
fn problems(form: &Form) -> Vec<String> {
    let mut found = vec![];
    for pair in form.sections.windows(2) {
        let (a, b) = (&pair[0], &pair[1]);
        if a.letter == b.letter && (a.loudness - b.loudness).abs() < 1. {
            found.push(format!("bars {}–{} and {}–{} sound alike: no contrast between them", a.from + 1, a.to, b.from + 1, b.to));
        }
    }
    // A plateau: 32 bars and more within 1.5 dB of each other.
    let (mut from, mut longest, mut at_longest) = (0, 0, 0);
    for at in 1..=form.bars.len() {
        let span = &form.bars[from..at];
        let (low, high) = span.iter().fold((f64::MAX, f64::MIN), |(low, high), bar| (low.min(bar.loudness), high.max(bar.loudness)));
        if high - low > 1.5 {
            from = at - 1;
        } else if at - from > longest {
            longest = at - from;
            at_longest = from;
        }
    }
    if longest >= 32 {
        found.push(format!("the energy plateaus for {longest} bars from bar {}", at_longest + 1));
    }
    for section in &form.sections {
        let length = section.to - section.from;
        let inner = section.from > 0 && section.to < form.bars.len();
        if inner && length % 4 != 0 {
            found.push(format!("a {length}-bar section at bar {} (phrases usually run in fours and eights)", section.from + 1));
        }
    }
    for turn in &form.transitions {
        if turn.kind == "drop" && turn.prepared.is_empty() {
            found.push(format!("the drop at bar {} arrives unprepared: no build, gap or riser before it", turn.at + 1));
        }
    }
    let (length, start) = form.unchanged;
    if length >= 32 {
        found.push(format!("bars {}–{} barely change ({length} bars)", start + 1, start + length));
    }
    found
}

/// How one form differs from a reference's: its intro, outro, time to the hook and length (in bars), how many
/// sections it has and their typical length, and the contrast between its loudest and quietest section.
pub fn compare(form: &Form, reference: &Form) -> Vec<String> {
    let mut said = vec![];
    let mut differ = |what: &str, ours: f64, theirs: f64, by: f64| {
        if (ours - theirs).abs() >= by {
            said.push(format!("{what}: {} here, {} in the reference", number(ours), number(theirs)));
        }
    };
    differ("intro (bars)", form.intro as f64, reference.intro as f64, 4.);
    differ("outro (bars)", form.outro as f64, reference.outro as f64, 4.);
    if let (Some(ours), Some(theirs)) = (form.hook, reference.hook) {
        differ("bars to the first hook", ours as f64, theirs as f64, 4.);
    }
    differ("length (bars)", form.bars.len() as f64, reference.bars.len() as f64, 8.);
    differ("sections", form.sections.len() as f64, reference.sections.len() as f64, 2.);
    let typical = |form: &Form| form.bars.len() as f64 / form.sections.len().max(1) as f64;
    differ("bars a section", typical(form), typical(reference), 4.);
    let contrast = |form: &Form| {
        let levels = form.sections.iter().map(|section| section.loudness);
        levels.clone().fold(f64::MIN, f64::max) - levels.fold(f64::MAX, f64::min)
    };
    differ("contrast between sections (dB)", contrast(form), contrast(reference), 2.);
    said
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
