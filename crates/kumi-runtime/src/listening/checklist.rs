//! A goal as a checklist, not one score: targets (a loudness, a ceiling), a reference per dimension (tonal balance,
//! dynamics, the low end), problems to clear, and guards that mustn't get worse (punch, pumping, distortion,
//! clipping). Every gap is measured on one scale, in just-noticeable steps, so the biggest is fixed first; a change
//! stays only if its target improved and nothing else got audibly worse.

use super::{
    detect::{self, hertz, Problem, ProblemKind},
    measure::{fine_hz, percentile, Heard, Measures, FINE_BINS},
};
use serde::{Deserialize, Serialize};

/// The regions tonal balance is judged in: third-octave bands, first to last.
pub const REGIONS: [(&str, usize, usize); 8] = [
    ("sub", 0, 4),
    ("bass", 5, 8),
    ("low mids", 9, 12),
    ("mids", 13, 16),
    ("upper mids", 17, 20),
    ("presence", 21, 24),
    ("brilliance", 25, 27),
    ("air", 28, 30),
];

/// A region's share of the whole, dB, from a balance (each band's share in dB).
pub fn region_level(balance: &[f64], region: usize) -> f64 {
    let (_, from, to) = REGIONS[region];
    10. * balance[from..=to].iter().map(|db| 10f64.powf(db / 10.)).sum::<f64>().max(1e-12).log10()
}

/// What a reference sounds like, measured: each region's level and the range it moves in, and its dynamics, width
/// and low end. One track's range is how its own loud moments vary; several tracks' (later) is how they differ.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Profile {
    pub name: String,
    pub tracks: usize,
    pub regions: Vec<Spread>,
    pub integrated: Option<Spread>,
    pub plr: Option<Spread>,
    pub crest: Option<Spread>,
    pub low_width: Option<Spread>,
    pub tilt: Spread,
}

/// A measure's typical value and the range it keeps to (the 20th to 80th percentile).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Spread {
    pub mid: f64,
    pub low: f64,
    pub high: f64,
}
impl Spread {
    pub fn point(value: f64, half: f64) -> Self {
        Self { mid: value, low: value - half, high: value + half }
    }
}

impl Profile {
    /// One reference track's profile: its regions' levels over its loud frames, at least ±1 dB wide.
    pub fn of(name: &str, heard: &Heard) -> Self {
        let frames = &heard.frames;
        let mut levels: Vec<f64> = frames.level.iter().map(|level| *level as f64).collect();
        levels.sort_by(f64::total_cmp);
        let loud = if levels.is_empty() { 0. } else { percentile(&levels, 0.95) - 15. };
        let regions = (0..REGIONS.len())
            .map(|region| {
                let (_, from, to) = REGIONS[region];
                let mut shares: Vec<f64> = (0..frames.mid.len())
                    .filter(|frame| frames.level[*frame] as f64 >= loud)
                    .map(|frame| {
                        let total: f64 = (0..31).map(|band| (frames.mid[frame][band] + frames.side[frame][band]) as f64).sum();
                        let part: f64 = (from..=to).map(|band| (frames.mid[frame][band] + frames.side[frame][band]) as f64).sum();
                        10. * ((part + 1e-20) / (total + 1e-20)).log10()
                    })
                    .collect();
                shares.sort_by(f64::total_cmp);
                let mid = region_level(&heard.measures.balance, region);
                if shares.len() < 5 {
                    return Spread::point(mid, 1.5);
                }
                let (low, high) = (percentile(&shares, 0.2), percentile(&shares, 0.8));
                // Centered on the whole's level, as wide as its loud moments vary.
                let half = ((high - low) / 2.).clamp(1., 4.);
                Spread::point(round1(mid), half)
            })
            .collect();
        let m = &heard.measures;
        Self {
            name: name.into(),
            tracks: 1,
            regions,
            integrated: m.integrated.map(|value| Spread::point(value, 1.)),
            plr: m.plr.map(|value| Spread::point(value, 1.5)),
            crest: m.crest.map(|value| Spread::point(value, 1.5)),
            low_width: m.low_width.map(|value| Spread::point(value, 4.)),
            tilt: Spread::point(m.tilt, 0.5),
        }
    }
}

/// What a goal asks: explicit targets, a reference, the problems to clear and the element that must cut through.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Goal {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub loudness: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub true_peak: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reference: Option<Profile>,
    /// Clear what the detectors find (on unless the goal says otherwise).
    #[serde(default = "yes")]
    pub problems: bool,
    /// The element that must cut through the rest (masking is measured against it).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub focus: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub targets: Vec<Explicit>,
}
fn yes() -> bool {
    true
}

/// An explicit number the producer gave: "crest at least 9 dB", "58% swing" (later).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Explicit {
    pub measure: Quantity,
    pub target: Target,
}

/// What an item reads from a listen.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", tag = "kind")]
pub enum Quantity {
    Integrated,
    TruePeak,
    Plr,
    Psr,
    Range,
    Crest,
    Pumping,
    Distortion,
    LowWidth,
    Rumble,
    Clipped,
    Tilt,
    Region {
        region: usize,
    },
    /// A problem's excess where it was found (dB over its local level, or under the others for masking).
    Problem {
        problem: ProblemKind,
        low: f64,
        high: f64,
        steady: bool,
        focus: Option<String>,
    },
}

/// Where an item should be.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum Target {
    /// Within `within` of a value.
    Exactly {
        value: f64,
        within: f64,
    },
    AtMost {
        value: f64,
    },
    AtLeast {
        value: f64,
    },
    Between {
        low: f64,
        high: f64,
    },
    /// A guard: wherever it is, it mustn't get worse (higher, for these).
    NoHigher,
    /// A guard that mustn't get lower (punch).
    NoLower,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    /// Something asked for (a loudness, a ceiling, an explicit number).
    Target,
    /// A reference's dimension.
    Reference,
    /// A problem to clear.
    Problem,
    /// Mustn't get audibly worse.
    Guard,
}

/// One line of the checklist.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Item {
    pub id: String,
    pub label: String,
    pub role: Role,
    pub unit: String,
    pub quantity: Quantity,
    pub target: Target,
    /// One just-noticeable step, in the unit.
    pub jnd: f64,
    /// What it calls for when it's off (from a detector), for the model.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fix: Option<String>,
}

impl Item {
    /// How far off target a value is, in just-noticeable steps (0 within tolerance).
    pub fn gap(&self, value: Option<f64>) -> f64 {
        let Some(value) = value else { return 0. };
        let off = match self.target {
            Target::Exactly { value: target, within } => ((value - target).abs() - within).max(0.),
            Target::AtMost { value: limit } => (value - limit).max(0.),
            Target::AtLeast { value: limit } => (limit - value).max(0.),
            Target::Between { low, high } => (low - value).max(value - high).max(0.),
            Target::NoHigher | Target::NoLower => 0.,
        };
        off / self.jnd
    }
    /// The target in words.
    pub fn wanted(&self) -> String {
        let unit = if self.unit.is_empty() { String::new() } else { format!(" {}", self.unit) };
        match self.target {
            Target::Exactly { value, within } => format!("{}{unit} ±{}", number(value), number(within)),
            Target::AtMost { value } => format!("≤ {}{unit}", number(value)),
            Target::AtLeast { value } => format!("≥ {}{unit}", number(value)),
            Target::Between { low, high } => format!("{} to {}{unit}", number(low), number(high)),
            Target::NoHigher => "no higher".into(),
            Target::NoLower => "no lower".into(),
        }
    }
}

/// The checklist a goal makes against what was heard first.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Checklist {
    pub items: Vec<Item>,
}

impl Checklist {
    /// The goal's targets and reference, the problems found in the first listen (and in the focus element against
    /// the mix), and the guards.
    pub fn new(goal: &Goal, heard: &Heard, problems: &[Problem]) -> Self {
        let mut items = vec![];
        let reference = goal.reference.as_ref();
        if let Some(value) = goal.loudness {
            items.push(item(
                "loudness",
                "Loudness",
                Role::Target,
                "LUFS",
                Quantity::Integrated,
                Target::Exactly { value, within: 0.5 },
                1.,
            ));
        } else if let Some(spread) = reference.and_then(|r| r.integrated) {
            items.push(item(
                "loudness",
                "Loudness, as the reference",
                Role::Reference,
                "LUFS",
                Quantity::Integrated,
                Target::Exactly { value: spread.mid, within: 0.5 },
                1.,
            ));
        }
        if let Some(value) = goal.true_peak {
            items.push(item("true peak", "True peak", Role::Target, "dBTP", Quantity::TruePeak, Target::AtMost { value }, 0.2));
        }
        if let Some(reference) = reference {
            for (region, spread) in reference.regions.iter().enumerate() {
                let name = REGIONS[region].0;
                items.push(item(
                    &format!("balance {name}"),
                    &format!("{} against the reference", capital(name)),
                    Role::Reference,
                    "dB",
                    Quantity::Region { region },
                    Target::Between { low: round1(spread.low), high: round1(spread.high) },
                    1.,
                ));
            }
            if let Some(spread) = reference.plr {
                items.push(item(
                    "dynamics",
                    "Dynamics (peak to loudness), as the reference",
                    Role::Reference,
                    "dB",
                    Quantity::Plr,
                    Target::Between { low: round1(spread.low), high: round1(spread.high) },
                    1.,
                ));
            }
            if let Some(spread) = reference.low_width {
                items.push(item(
                    "low width",
                    "Stereo below 120 Hz, as the reference",
                    Role::Reference,
                    "dB",
                    Quantity::LowWidth,
                    Target::AtMost { value: round1(spread.high.max(-20.)) },
                    2.,
                ));
            }
        }
        for explicit in &goal.targets {
            let (id, label, unit, jnd) = describe(&explicit.measure);
            if !items.iter().any(|item| item.id == id) {
                items.push(item(&id, &label, Role::Target, unit, explicit.measure.clone(), explicit.target, jnd));
            }
        }
        if goal.problems {
            for problem in problems {
                if items.iter().any(|item| item.id == problem.id) {
                    continue;
                }
                let added = match problem.kind {
                    ProblemKind::Harshness | ProblemKind::Resonance => {
                        let [low, high] = problem.hz.unwrap_or([0., 0.]);
                        let steady = problem.steady.unwrap_or(true);
                        // A steady peak in a whole mix may be the music's own (a held note, a drone): taken down to
                        // where it stops sticking out, not flattened.
                        let limit = if steady { 6. } else { 3. };
                        Some((
                            Quantity::Problem { problem: problem.kind, low, high, steady, focus: None },
                            Target::AtMost { value: limit },
                            1.,
                            "dB",
                        ))
                    }
                    ProblemKind::StereoLows if !items.iter().any(|item| item.id == "low width") => {
                        Some((Quantity::LowWidth, Target::AtMost { value: -20. }, 2., "dB"))
                    }
                    ProblemKind::Rumble => Some((Quantity::Rumble, Target::AtMost { value: -15. }, 2., "dB")),
                    ProblemKind::LoudNote => {
                        let [low, high] = problem.hz.unwrap_or([0., 0.]);
                        Some((
                            Quantity::Problem { problem: problem.kind, low, high, steady: false, focus: None },
                            Target::AtMost { value: 2. },
                            1.,
                            "dB",
                        ))
                    }
                    ProblemKind::Masking => {
                        let [low, high] = problem.hz.unwrap_or([0., 0.]);
                        // The share of the time it plays that it's buried, in percent.
                        Some((
                            Quantity::Problem { problem: problem.kind, low, high, steady: true, focus: goal.focus.clone() },
                            Target::AtMost { value: 10. },
                            5.,
                            "%",
                        ))
                    }
                    _ => None,
                };
                if let Some((quantity, target, jnd, unit)) = added {
                    let mut added = item(&problem.id, &capital(&problem.id), Role::Problem, unit, quantity, target, jnd);
                    added.fix = Some(problem.fix.clone());
                    items.push(added);
                }
            }
        }
        // The guards: punch, pumping, distortion and clipping mustn't get audibly worse.
        if heard.measures.crest.is_some() {
            items.push(item("punch", "Punch (crest)", Role::Guard, "dB", Quantity::Crest, Target::NoLower, 1.));
        }
        if heard.measures.pumping.is_some() {
            items.push(item("pumping", "Pumping", Role::Guard, "dB", Quantity::Pumping, Target::NoHigher, 1.));
        }
        if heard.measures.distortion.is_some() {
            items.push(item("distortion", "Distortion at peaks", Role::Guard, "dB", Quantity::Distortion, Target::NoHigher, 1.));
        }
        if !items.iter().any(|item| item.quantity == Quantity::Clipped) {
            items.push(item("clipping", "Clipped samples", Role::Guard, "", Quantity::Clipped, Target::AtMost { value: 0. }, 1.));
        }
        Self { items }
    }

    /// What each item reads from a listen (and, for masking, the focus element's own listen).
    pub fn read(&self, heard: &Heard, focus: Option<&Heard>) -> Vec<Option<f64>> {
        self.items.iter().map(|item| read(&item.quantity, heard, focus)).collect()
    }

    /// The biggest gap, the item to work on next; None when every item is within tolerance.
    pub fn next(&self, values: &[Option<f64>]) -> Option<usize> {
        self.items
            .iter()
            .zip(values)
            .enumerate()
            .filter(|(_, (item, _))| item.role != Role::Guard)
            .map(|(index, (item, value))| (index, item.gap(*value)))
            .filter(|(_, gap)| *gap > 0.)
            .max_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(index, _)| index)
    }

    /// Before against after, item by item: whether the target improved and what got audibly worse. Loudness isn't
    /// counted as worse unless it was the target: rebalancing brings it back.
    pub fn verdict(&self, target: Option<usize>, before: &[Option<f64>], after: &[Option<f64>]) -> Verdict {
        let mut rows = vec![];
        let mut hurt = vec![];
        let mut improved = false;
        let mut total = (0., 0.);
        // Bringing peaks or loudness to a target squeezes dynamics by about as much as it moves them: punch may drop
        // that far (and pumping rise half as far) before it counts against the change, no further.
        let allowance = target
            .filter(|index| {
                matches!(
                    self.items[*index].quantity,
                    Quantity::TruePeak | Quantity::Integrated | Quantity::Plr | Quantity::Psr | Quantity::Range
                )
            })
            .and_then(|index| Some((after[index]? - before[index]?).abs()))
            .unwrap_or(0.);
        for (index, item) in self.items.iter().enumerate() {
            let (b, a) = (before[index], after[index]);
            let (gap_before, gap_after) = (item.gap(b), item.gap(a));
            let slack = match item.quantity {
                Quantity::Crest => allowance,
                Quantity::Pumping => allowance / 2.,
                _ => 0.,
            };
            let change = match (item.target, b, a) {
                (Target::NoHigher, Some(b), Some(a)) if a - b > item.jnd + slack => Change::Worse,
                (Target::NoLower, Some(b), Some(a)) if b - a > item.jnd + slack => Change::Worse,
                (Target::NoHigher | Target::NoLower, Some(b), Some(a)) if (a - b).abs() <= item.jnd + slack => Change::Same,
                (Target::NoHigher, Some(b), Some(a)) => {
                    if a < b {
                        Change::Better
                    } else {
                        Change::Same
                    }
                }
                (Target::NoLower, Some(b), Some(a)) => {
                    if a > b {
                        Change::Better
                    } else {
                        Change::Same
                    }
                }
                _ if gap_after < gap_before - 0.5 || (gap_before > 0. && gap_after == 0.) => Change::Better,
                // Worse by a step, or out of tolerance by half of one.
                _ if gap_after > gap_before + 1. || (gap_before == 0. && gap_after > 0.5) => Change::Worse,
                _ => Change::Same,
            };
            let is_target = target == Some(index);
            if is_target && change == Change::Better {
                improved = true;
            }
            if !is_target && change == Change::Worse && item.quantity != Quantity::Integrated {
                hurt.push(index);
            }
            if item.role != Role::Guard {
                total.0 += gap_before;
                total.1 += gap_after;
            }
            rows.push(Row {
                id: item.id.clone(),
                label: item.label.clone(),
                unit: item.unit.clone(),
                wanted: item.wanted(),
                before: b.map(round1),
                after: a.map(round1),
                gap_before: round1(gap_before),
                gap_after: round1(gap_after),
                change,
            });
        }
        // Without a named target, the change has to close the gaps as a whole.
        if target.is_none() && total.1 < total.0 - 0.5 {
            improved = true;
        }
        let kept = improved && hurt.is_empty();
        let why = if !improved {
            match target {
                Some(index) => format!("{} didn't improve", self.items[index].label),
                None => "the gaps didn't close".into(),
            }
        } else if !hurt.is_empty() {
            format!("it hurt {}", hurt.iter().map(|index| self.items[*index].label.to_lowercase()).collect::<Vec<_>>().join(", "))
        } else {
            match target {
                Some(index) => format!("{} improved and nothing else got audibly worse", self.items[index].label),
                None => "the gaps closed and nothing got audibly worse".into(),
            }
        };
        Verdict {
            kept,
            why,
            target: target.map(|index| self.items[index].id.clone()),
            rows,
            hurt: hurt.into_iter().map(|index| self.items[index].id.clone()).collect(),
        }
    }

    /// The gain that brings loudness back to its target (or to where it was, without one), when it's audibly off.
    pub fn rebalance(&self, before: &[Option<f64>], after: &[Option<f64>]) -> Option<f64> {
        let index = self.items.iter().position(|item| item.quantity == Quantity::Integrated);
        let gain = match index.map(|index| (&self.items[index], after[index])) {
            Some((item, Some(now))) => match item.target {
                Target::Exactly { value, within } if (now - value).abs() > within => Some(value - now),
                _ => None,
            },
            _ => None,
        };
        gain.or_else(|| {
            // No loudness asked for: keep it where it was.
            let (Some(Some(b)), Some(Some(a))) = (index.map(|index| before[index]), index.map(|index| after[index])) else { return None };
            ((a - b).abs() > 0.5 && index.is_none()).then_some(b - a)
        })
        .map(round1)
    }
}

fn item(id: &str, label: &str, role: Role, unit: &str, quantity: Quantity, target: Target, jnd: f64) -> Item {
    Item { id: id.into(), label: label.into(), role, unit: unit.into(), quantity, target, jnd, fix: None }
}

/// An explicit measure's id, label, unit and step.
fn describe(quantity: &Quantity) -> (String, String, &'static str, f64) {
    match quantity {
        Quantity::Integrated => ("loudness".into(), "Loudness".into(), "LUFS", 1.),
        Quantity::TruePeak => ("true peak".into(), "True peak".into(), "dBTP", 0.2),
        Quantity::Plr => ("dynamics".into(), "Dynamics (peak to loudness)".into(), "dB", 1.),
        Quantity::Psr => ("density".into(), "Density (peak to short-term loudness)".into(), "dB", 1.),
        Quantity::Range => ("range".into(), "Loudness range".into(), "LU", 1.),
        Quantity::Crest => ("crest".into(), "Punch (crest)".into(), "dB", 1.),
        Quantity::Pumping => ("pumping".into(), "Pumping".into(), "dB", 1.),
        Quantity::Distortion => ("distortion".into(), "Distortion at peaks".into(), "dB", 1.),
        Quantity::LowWidth => ("low width".into(), "Stereo below 120 Hz".into(), "dB", 2.),
        Quantity::Rumble => ("rumble".into(), "Rumble under 30 Hz".into(), "dB", 2.),
        Quantity::Clipped => ("clipping".into(), "Clipped samples".into(), "", 1.),
        Quantity::Tilt => ("tilt".into(), "Brightness (tilt)".into(), "dB/oct", 0.3),
        Quantity::Region { region } => (format!("balance {}", REGIONS[*region].0), capital(REGIONS[*region].0), "dB", 1.),
        Quantity::Problem { problem, low, high, .. } => {
            (format!("{problem:?} {}", hertz((low * high).sqrt())).to_lowercase(), format!("{problem:?}"), "dB", 1.)
        }
    }
}

/// A quantity read from a listen.
pub fn read(quantity: &Quantity, heard: &Heard, focus: Option<&Heard>) -> Option<f64> {
    let m: &Measures = &heard.measures;
    match quantity {
        Quantity::Integrated => m.integrated,
        Quantity::TruePeak => Some(m.true_peak),
        Quantity::Plr => m.plr,
        Quantity::Psr => m.psr,
        Quantity::Range => m.range,
        Quantity::Crest => m.crest,
        Quantity::Pumping => m.pumping,
        Quantity::Distortion => m.distortion,
        Quantity::LowWidth => m.low_width,
        Quantity::Rumble => m.rumble,
        Quantity::Clipped => Some(m.clipped as f64),
        Quantity::Tilt => Some(m.tilt),
        Quantity::Region { region } => Some(round1(region_level(&m.balance, *region))),
        Quantity::Problem { problem, low, high, steady, focus: name } => match problem {
            ProblemKind::Harshness | ProblemKind::Resonance => Some(region_excess(heard, *low, *high, *steady)),
            ProblemKind::LoudNote => Some(
                detect::low_end(heard)
                    .into_iter()
                    .find(|p| p.kind == ProblemKind::LoudNote && overlaps(p, *low, *high))
                    .map_or(0., |p| p.excess),
            ),
            ProblemKind::Masking => {
                let focus = focus?;
                Some(detect::masking(focus, heard, name.as_deref().unwrap_or("it")).map_or(0., |p| p.excess))
            }
            _ => None,
        },
    }
}

fn overlaps(problem: &Problem, low: f64, high: f64) -> bool {
    problem.hz.is_some_and(|[from, to]| from < high && to > low)
}

/// How far the short-term spectrum stands over its own ±half-octave average between `low` and `high`: the median over
/// the loud frames for a steady problem, the 90th percentile for one that comes and goes. Read whether or not it's
/// still bad enough to be found, so a fix shows as a number going down.
pub fn region_excess(heard: &Heard, low: f64, high: f64, steady: bool) -> f64 {
    let frames = &heard.frames;
    let mut levels: Vec<f64> = frames.level.iter().map(|level| *level as f64).collect();
    levels.sort_by(f64::total_cmp);
    if levels.is_empty() {
        return 0.;
    }
    let loud = percentile(&levels, 0.95) - 20.;
    let bins: Vec<usize> = (1..FINE_BINS - 1).filter(|bin| (low..=high).contains(&fine_hz(*bin))).collect();
    if bins.is_empty() {
        return 0.;
    }
    let mut excesses: Vec<f64> = (0..frames.fine.len())
        .filter(|frame| frames.level[*frame] as f64 >= loud)
        .map(|frame| {
            let fine = &frames.fine[frame];
            bins.iter()
                .map(|bin| {
                    let (from, to) = (bin.saturating_sub(6), (bin + 7).min(FINE_BINS));
                    let local =
                        10. * (fine[from..to].iter().map(|db| 10f64.powf(*db as f64 / 10.)).sum::<f64>() / (to - from) as f64).log10();
                    fine[*bin] as f64 - local
                })
                .fold(f64::MIN, f64::max)
        })
        .collect();
    excesses.sort_by(f64::total_cmp);
    round1(percentile(&excesses, if steady { 0.5 } else { 0.9 }).max(0.))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Change {
    Better,
    Same,
    Worse,
}

/// One checklist line, before and after a change.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Row {
    pub id: String,
    pub label: String,
    pub unit: String,
    pub wanted: String,
    pub before: Option<f64>,
    pub after: Option<f64>,
    pub gap_before: f64,
    pub gap_after: f64,
    pub change: Change,
}

/// Keep or revert, and why.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Verdict {
    pub kept: bool,
    pub why: String,
    pub target: Option<String>,
    pub rows: Vec<Row>,
    pub hurt: Vec<String>,
}

fn capital(text: &str) -> String {
    let mut chars = text.chars();
    chars.next().map(|first| first.to_uppercase().collect::<String>() + chars.as_str()).unwrap_or_default()
}
pub fn number(value: f64) -> String {
    let rounded = (value * 10.).round() / 10.;
    if rounded == rounded.trunc() {
        format!("{}", rounded as i64)
    } else {
        format!("{rounded:.1}")
    }
}
fn round1(value: f64) -> f64 {
    (value * 10.).round() / 10.
}
