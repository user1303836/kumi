//! A goal as a checklist, not one score: targets (a loudness, a ceiling), a reference per dimension (tonal balance,
//! dynamics, the low end), problems to clear, and guards that mustn't get worse (punch, pumping, distortion,
//! clipping). Every gap is measured on one scale, in just-noticeable steps, so the biggest is fixed first; a change
//! stays only if its target improved and nothing else got audibly worse.

use super::{
    detect::{self, hertz, Problem, ProblemKind},
    fit::{response, Band, Shape},
    measure::{fine_hz, percentile, Heard, Measures, FINE_BINS, THIRDS},
    sound,
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
    /// How far its quiet and loud stretches lie apart (loudness range, LU): the song's contrast between sections.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub range: Option<Spread>,
    /// A sound's own: its attack and decay (ms), where it settles (dB under its peak), brightness (Hz) and noisiness
    /// (dB).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attack: Option<Spread>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decay: Option<Spread>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sustain: Option<Spread>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub centroid: Option<Spread>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub noise: Option<Spread>,
    /// The rest of a sound's measures: its pitch drop, modulation, width, top, noise floor, warmth, tail and crackle.
    #[serde(default, skip_serializing_if = "SoundProfile::is_empty")]
    pub sound: SoundProfile,
}

/// A sound's measures beyond its envelope and tone, each with its range.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SoundProfile {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pitch_drop: Option<Spread>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub modulation: Option<Spread>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub width: Option<Spread>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bandwidth: Option<Spread>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub noise_floor: Option<Spread>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub warmth: Option<Spread>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tail: Option<Spread>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub crackle: Option<Spread>,
}

impl SoundProfile {
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }
    /// One sound's, with a step either side.
    pub fn of(heard: &Heard) -> Self {
        let m = &heard.measures;
        Self {
            pitch_drop: m.pitch_drop.map(|value| Spread::point(value, 0.5)),
            modulation: m.modulation.map(|value| Spread::point(value, (value * 0.1).max(0.25))),
            width: sound::width(heard).map(|value| Spread::point(value, 2.)),
            bandwidth: sound::bandwidth(heard).map(|hz| Spread::point(round1(hz / 1000.), 1.)),
            noise_floor: sound::noise_floor(heard).map(|value| Spread::point(value, 3.)),
            warmth: sound::harmonics(heard).map(|harmonics| Spread::point(harmonics.warmth, 2.)),
            tail: sound::tail(heard).map(|value| Spread::point(value, 3.)),
            crackle: Some(Spread::point(m.crackle, 2.)),
        }
    }
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
            range: m.range.map(|value| Spread::point(value, 1.5)),
            // A step either side: a fifth of an attack or decay, two dB, a sixth of an octave of brightness.
            attack: m.attack.map(|value| Spread::point(value, (value * 0.2).max(2.))),
            decay: m.decay.map(|value| Spread::point(value, (value * 0.15).max(10.))),
            sustain: m.sustain.map(|value| Spread::point(value, 2.)),
            centroid: m.centroid.map(|value| Spread::point(value, value * 0.12)),
            noise: m.noise.map(|value| Spread::point(value, 2.)),
            sound: SoundProfile::of(heard),
        }
    }

    /// Several tracks' profile: each measure's middle across them and the range they keep to, from the 10th to the
    /// 90th percentile of the tracks (with few tracks, from the lowest to the highest, never narrower than one
    /// track's own range). "In the style" is inside it.
    pub fn combine(name: &str, tracks: &[Profile]) -> Option<Profile> {
        let first = tracks.first()?;
        if tracks.len() == 1 {
            return Some(Profile { name: name.into(), ..first.clone() });
        }
        let spread = |spreads: Vec<Spread>| -> Option<Spread> {
            if spreads.is_empty() {
                return None;
            }
            let mut mids: Vec<f64> = spreads.iter().map(|spread| spread.mid).collect();
            mids.sort_by(f64::total_cmp);
            let (low, high) =
                if mids.len() >= 5 { (percentile(&mids, 0.1), percentile(&mids, 0.9)) } else { (mids[0], mids[mids.len() - 1]) };
            // At least as wide as a typical track's own range.
            let mut halves: Vec<f64> = spreads.iter().map(|spread| (spread.high - spread.low) / 2.).collect();
            halves.sort_by(f64::total_cmp);
            let half = percentile(&halves, 0.5);
            let mid = percentile(&mids, 0.5);
            Some(Spread { mid: round1(mid), low: round1(low.min(mid - half)), high: round1(high.max(mid + half)) })
        };
        let pick = |get: &dyn Fn(&Profile) -> Option<Spread>| spread(tracks.iter().filter_map(get).collect());
        Some(Profile {
            name: name.into(),
            tracks: tracks.iter().map(|track| track.tracks).sum(),
            regions: (0..first.regions.len()).filter_map(|region| pick(&|track| track.regions.get(region).copied())).collect(),
            integrated: pick(&|track| track.integrated),
            plr: pick(&|track| track.plr),
            crest: pick(&|track| track.crest),
            low_width: pick(&|track| track.low_width),
            tilt: pick(&|track| Some(track.tilt)).unwrap_or(first.tilt),
            range: pick(&|track| track.range),
            attack: pick(&|track| track.attack),
            decay: pick(&|track| track.decay),
            sustain: pick(&|track| track.sustain),
            centroid: pick(&|track| track.centroid),
            noise: pick(&|track| track.noise),
            sound: SoundProfile {
                pitch_drop: pick(&|track| track.sound.pitch_drop),
                modulation: pick(&|track| track.sound.modulation),
                width: pick(&|track| track.sound.width),
                bandwidth: pick(&|track| track.sound.bandwidth),
                noise_floor: pick(&|track| track.sound.noise_floor),
                warmth: pick(&|track| track.sound.warmth),
                tail: pick(&|track| track.sound.tail),
                crackle: pick(&|track| track.sound.crackle),
            },
        })
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
    /// A single sound against a reference sound: its envelope, brightness and noisiness are on the checklist too.
    #[serde(default)]
    pub sound: bool,
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
    Attack,
    Decay,
    Sustain,
    Centroid,
    Noise,
    PitchDrop,
    Modulation,
    Width,
    Bandwidth,
    NoiseFloor,
    Warmth,
    Tail,
    Crackle,
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

impl Quantity {
    /// A value the whole stretch is predicted at, from what the excerpt moved: counts and found problems (how far a
    /// peak stands out, how much of the time a part is buried) don't go below nothing.
    pub fn moved(&self, whole: f64, before: f64, after: f64) -> f64 {
        let moved = whole + (after - before);
        match self {
            Quantity::Clipped | Quantity::Problem { .. } => moved.max(0.),
            _ => moved,
        }
    }
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
            // A sound against a sound: its envelope and tone.
            if goal.sound {
                let sound = [
                    ("attack", "Attack, as the reference", "ms", Quantity::Attack, reference.attack, 0.2, 2.),
                    ("decay", "Decay, as the reference", "ms", Quantity::Decay, reference.decay, 0.15, 10.),
                    ("sustain", "Where it settles, as the reference", "dB", Quantity::Sustain, reference.sustain, 0., 2.),
                    ("brightness", "Brightness (centroid), as the reference", "Hz", Quantity::Centroid, reference.centroid, 0.12, 50.),
                    ("noisiness", "Noisiness, as the reference", "dB", Quantity::Noise, reference.noise, 0., 2.),
                    ("pitch drop", "Pitch drop, as the reference", "st", Quantity::PitchDrop, reference.sound.pitch_drop, 0., 0.5),
                    ("modulation", "Modulation rate, as the reference", "Hz", Quantity::Modulation, reference.sound.modulation, 0.1, 0.25),
                    ("width", "Width, as the reference", "dB", Quantity::Width, reference.sound.width, 0., 2.),
                    ("top", "Top (bandwidth), as the reference", "kHz", Quantity::Bandwidth, reference.sound.bandwidth, 0., 1.),
                    ("noise floor", "Noise floor, as the reference", "dB", Quantity::NoiseFloor, reference.sound.noise_floor, 0., 3.),
                    ("warmth", "Warmth (2nd and 3rd harmonics), as the reference", "dB", Quantity::Warmth, reference.sound.warmth, 0., 2.),
                    ("tail", "Tail (300 ms after a hit), as the reference", "dB", Quantity::Tail, reference.sound.tail, 0., 3.),
                    ("crackle", "Crackle, as the reference", "/s", Quantity::Crackle, reference.sound.crackle, 0., 2.),
                ];
                for (id, label, unit, quantity, spread, share, least) in sound {
                    let Some(spread) = spread else { continue };
                    items.push(item(
                        id,
                        label,
                        Role::Reference,
                        unit,
                        quantity,
                        Target::Between { low: round1(spread.low), high: round1(spread.high) },
                        (spread.mid.abs() * share).max(least),
                    ));
                }
            }
            // Its form's contrast: how far the quiet and the loud sections lie apart.
            if let Some(spread) = reference.range.filter(|_| heard.measures.range.is_some()) {
                items.push(item(
                    "range",
                    "Contrast between sections (loudness range), as the reference",
                    Role::Reference,
                    "LU",
                    Quantity::Range,
                    Target::Between { low: round1(spread.low), high: round1(spread.high) },
                    1.,
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
                        // where it stops sticking out, not flattened. A found peak asks for 3 dB less (a fix you hear)
                        // or the limit, whichever is higher: music has peaks, and a checklist nobody can finish
                        // helps nobody.
                        // Measured as the checklist reads it (over all the loud frames, not only the ones it flared
                        // in): one already under its limit that way isn't worth a round.
                        let limit: f64 = if steady { 6. } else { 3. };
                        let now = region_excess(heard, low, high, steady);
                        (now > limit).then(|| {
                            (
                                Quantity::Problem { problem: problem.kind, low, high, steady, focus: None },
                                Target::AtMost { value: round1(limit.max(now - 3.)) },
                                1.,
                                "dB",
                            )
                        })
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
                    let steady = matches!(
                        quantity,
                        Quantity::Problem { problem: ProblemKind::Resonance | ProblemKind::Harshness, steady: true, .. }
                    );
                    let mut added = item(&problem.id, &capital(&problem.id), Role::Problem, unit, quantity, target, jnd);
                    added.fix =
                        Some(if steady { format!("{} (tune with how: fit calculates the cut)", problem.fix) } else { problem.fix.clone() });
                    items.push(added);
                }
            }
        }
        for added in items.iter_mut() {
            added.fix = added.fix.take().or_else(|| match added.quantity {
                Quantity::Integrated => Some("the last limiter's gain, homed in (tune with how: home, knobs [\"Gain\"])".into()),
                Quantity::TruePeak => {
                    Some("a true-peak limiter last, its ceiling under the target (tune with how: home, knobs [\"Ceiling\"])".into())
                }
                Quantity::Region { .. } => Some("an EQ Eight toward the reference's shape (tune with how: fit)".into()),
                _ => None,
            });
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
            // A step is a twentieth of what clips now: a few samples either way between listens isn't a change.
            let step = (heard.measures.clipped as f64 * 0.05).max(1.).round();
            items.push(item("clipping", "Clipped samples", Role::Guard, "", Quantity::Clipped, Target::AtMost { value: 0. }, step));
        }
        Self { items }
    }

    /// What each item reads from a listen (and, for masking, the focus element's own listen).
    pub fn read(&self, heard: &Heard, focus: Option<&Heard>) -> Vec<Option<f64>> {
        self.items.iter().map(|item| read(&item.quantity, heard, focus)).collect()
    }

    /// The items a listen can't read, taken off the checklist (and said): what can't be measured can't be judged, and an
    /// item left on that reads nothing would pass as within tolerance. Their labels.
    pub fn drop_unreadable(&mut self, values: &mut Vec<Option<f64>>) -> Vec<String> {
        let mut dropped = vec![];
        let mut kept = vec![];
        for (item, value) in self.items.drain(..).zip(values.drain(..)) {
            if value.is_none() && item.role != Role::Guard {
                dropped.push(item.label.clone());
            } else {
                kept.push((item, value));
            }
        }
        (self.items, *values) = kept.into_iter().unzip();
        dropped
    }

    /// Whether anything on it is to be worked toward (not only guards).
    pub fn has_targets(&self) -> bool {
        self.items.iter().any(|item| item.role != Role::Guard)
    }

    /// `after` as it will read once loudness is brought back by `gain` dB: peaks move with it (unless a limiter after
    /// the gain holds them, then `gain` is 0 here), and how many samples clip can't be told until it's heard, so it
    /// reads as before.
    pub fn at_level(&self, before: &[Option<f64>], after: &[Option<f64>], gain: f64) -> Vec<Option<f64>> {
        self.items
            .iter()
            .zip(before.iter().zip(after))
            .map(|(item, (before, after))| match item.quantity {
                Quantity::TruePeak => after.map(|peak| peak + gain),
                Quantity::Clipped if gain != 0. => before.or(*after),
                _ => *after,
            })
            .collect()
    }

    /// The points an EQ is fitted to toward a reference's shape: each region's centre (Hz), how far it's off (dB: an
    /// open gap pulls its way, one in tolerance is held at 0 so a wide band can't push it out) and its weight.
    pub fn region_points(&self, whole: &[Option<f64>]) -> Vec<(f64, f64, f64)> {
        self.items
            .iter()
            .zip(whole)
            .filter_map(|(item, value)| match (&item.quantity, item.target, value) {
                (Quantity::Region { region }, Target::Between { low, high }, Some(value)) => {
                    let (_, from, to) = REGIONS[*region];
                    let gap = if item.gap(Some(*value)) > 0. { (low + high) / 2. - value } else { 0. };
                    Some(((THIRDS[from] * THIRDS[to]).sqrt(), gap, 1. / item.jnd.max(0.1)))
                }
                _ => None,
            })
            .collect()
    }

    /// The biggest gap, the item to work on next; None when every item is within tolerance.
    pub fn next(&self, values: &[Option<f64>]) -> Option<usize> {
        self.next_skipping(values, &[])
    }

    /// The biggest gap, passing over `skip` (targets that changes keep failing on) while another is still off.
    pub fn next_skipping(&self, values: &[Option<f64>], skip: &[usize]) -> Option<usize> {
        let open: Vec<(usize, f64)> = self
            .items
            .iter()
            .zip(values)
            .enumerate()
            .filter(|(_, (item, _))| item.role != Role::Guard)
            .map(|(index, (item, value))| (index, if value.is_some() { item.gap(*value) } else { f64::INFINITY }))
            .filter(|(_, gap)| *gap > 0.)
            .collect();
        let fresh = open.iter().filter(|(index, _)| !skip.contains(index)).max_by(|a, b| a.1.total_cmp(&b.1));
        fresh.or_else(|| open.iter().max_by(|a, b| a.1.total_cmp(&b.1))).map(|(index, _)| *index)
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
        let mut lost = vec![];
        for (index, item) in self.items.iter().enumerate() {
            let (b, a) = (before[index], after[index]);
            let (gap_before, gap_after) = (item.gap(b), item.gap(a));
            // A reading that's gone (silence, or the part stopped playing) is never within tolerance.
            if b.is_some() && a.is_none() {
                lost.push(index);
            }
            // A limiter bringing peaks down flattens them a little too.
            let peaks = target.is_some_and(|index| self.items[index].quantity == Quantity::TruePeak);
            let slack = match item.quantity {
                Quantity::Crest => allowance,
                Quantity::Pumping => allowance / 2.,
                Quantity::Distortion if peaks => allowance / 2.,
                _ => 0.,
            };
            let change = match (item.target, b, a) {
                (_, Some(_), None) => Change::Worse,
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
            if !is_target && change == Change::Worse && (item.quantity != Quantity::Integrated || lost.contains(&index)) {
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
        let kept = improved && hurt.is_empty() && lost.is_empty();
        let why = if !lost.is_empty() {
            format!(
                "{} couldn't be read after it (silence, or it stopped playing there)",
                lost.iter().map(|index| self.items[*index].label.to_lowercase()).collect::<Vec<_>>().join(", ")
            )
        } else if !improved {
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
        let index = self.items.iter().position(|item| item.quantity == Quantity::Integrated)?;
        let now = after[index]?;
        match self.items[index].target {
            Target::Exactly { value, within } => ((now - value).abs() > within).then_some(value - now),
            // No loudness asked for: keep it where it was.
            _ => before[index].filter(|was| (now - was).abs() > 0.5).map(|was| was - now),
        }
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
        Quantity::Attack => ("attack".into(), "Attack".into(), "ms", 2.),
        Quantity::Decay => ("decay".into(), "Decay".into(), "ms", 10.),
        Quantity::Sustain => ("sustain".into(), "Where it settles".into(), "dB", 2.),
        Quantity::Centroid => ("brightness".into(), "Brightness (centroid)".into(), "Hz", 50.),
        Quantity::Noise => ("noisiness".into(), "Noisiness".into(), "dB", 2.),
        Quantity::PitchDrop => ("pitch drop".into(), "Pitch drop".into(), "st", 0.5),
        Quantity::Modulation => ("modulation".into(), "Modulation rate".into(), "Hz", 0.25),
        Quantity::Width => ("width".into(), "Width".into(), "dB", 2.),
        Quantity::Bandwidth => ("top".into(), "Top (bandwidth)".into(), "kHz", 1.),
        Quantity::NoiseFloor => ("noise floor".into(), "Noise floor".into(), "dB", 3.),
        Quantity::Warmth => ("warmth".into(), "Warmth (2nd and 3rd harmonics)".into(), "dB", 2.),
        Quantity::Tail => ("tail".into(), "Tail (300 ms after a hit)".into(), "dB", 3.),
        Quantity::Crackle => ("crackle".into(), "Crackle".into(), "/s", 2.),
        Quantity::Region { region } => (format!("balance {}", REGIONS[*region].0), capital(REGIONS[*region].0), "dB", 1.),
        Quantity::Problem { problem, low, high, .. } => {
            (format!("{problem:?} {}", hertz((low * high).sqrt())).to_lowercase(), format!("{problem:?}"), "dB", 1.)
        }
    }
}

/// A quantity read from a listen: None when it can't be read there (silence reads as nothing, never as a number).
pub fn read(quantity: &Quantity, heard: &Heard, focus: Option<&Heard>) -> Option<f64> {
    reading(quantity, heard, focus).filter(|value| value.is_finite())
}

fn reading(quantity: &Quantity, heard: &Heard, focus: Option<&Heard>) -> Option<f64> {
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
        Quantity::Attack => m.attack,
        Quantity::Decay => m.decay,
        Quantity::Sustain => m.sustain,
        Quantity::Centroid => m.centroid,
        Quantity::Noise => m.noise,
        Quantity::PitchDrop => m.pitch_drop,
        Quantity::Modulation => m.modulation,
        Quantity::Width => sound::width(heard),
        Quantity::Bandwidth => sound::bandwidth(heard).map(|hz| round1(hz / 1000.)),
        Quantity::NoiseFloor => sound::noise_floor(heard),
        Quantity::Warmth => sound::harmonics(heard).map(|harmonics| harmonics.warmth),
        Quantity::Tail => sound::tail(heard),
        Quantity::Crackle => Some(m.crackle),
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
                let _ = name;
                detect::masking_share(focus?, heard)
            }
            _ => None,
        },
    }
}

fn overlaps(problem: &Problem, low: f64, high: f64) -> bool {
    problem.hz.is_some_and(|[from, to]| from < high && to > low)
}

/// How far the short-term spectrum stands over its neighbours (a quarter to a full octave away) between `low` and `high`: the median over
/// the loud frames for a steady problem, the 90th percentile for one that comes and goes. Read whether or not it's
/// still bad enough to be found, so a fix shows as a number going down.
pub fn region_excess(heard: &Heard, low: f64, high: f64, steady: bool) -> f64 {
    region_excess_with(heard, low, high, steady, &|_| 0.)
}

/// `region_excess` as it would read with each frequency moved by `shift` dB (an EQ's curve): what a cut would do,
/// before it's heard.
pub fn region_excess_with(heard: &Heard, low: f64, high: f64, steady: bool, shift: &dyn Fn(f64) -> f64) -> f64 {
    region_excess_in(heard, low, high, steady, 0..heard.frames.fine.len(), shift)
}

/// Where in what was heard a problem between `low` and `high` stands out most: the start of the `seconds`-long
/// stretch (moved a bar at a time, `step` seconds) where it reads highest.
pub fn worst_stretch(heard: &Heard, low: f64, high: f64, steady: bool, seconds: f64, step: f64) -> Option<f64> {
    let hop = heard.frames.hop;
    let count = heard.frames.fine.len();
    let length = (seconds / hop).round() as usize;
    if hop <= 0. || length == 0 || count <= length {
        return None;
    }
    let stride = ((step / hop).round() as usize).max(1);
    (0..=count - length)
        .step_by(stride)
        .map(|at| (at, region_excess_in(heard, low, high, steady, at..at + length, &|_| 0.)))
        .max_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(at, _)| at as f64 * hop)
}

/// Where a bass note between `low` and `high` plays most: the start of the `seconds`-long stretch (moved `step`
/// seconds at a time) with the most of its frames, so a change to it is heard where it is.
pub fn note_stretch(heard: &Heard, low: f64, high: f64, seconds: f64, step: f64) -> Option<f64> {
    let frames = &heard.frames;
    let hop = frames.bass_hop;
    let length = (seconds / hop.max(1e-9)).round() as usize;
    if hop <= 0. || length == 0 || frames.bass.len() <= length {
        return None;
    }
    let playing: Vec<u32> =
        frames.bass.iter().map(|pitch| u32::from(pitch.is_some_and(|(hz, _)| (low..=high).contains(&(hz as f64))))).collect();
    let stride = ((step / hop).round() as usize).max(1);
    (0..=playing.len() - length)
        .step_by(stride)
        .map(|at| (at, playing[at..at + length].iter().sum::<u32>()))
        .filter(|(_, count)| *count > 0)
        .max_by_key(|(_, count)| *count)
        .map(|(at, _)| at as f64 * hop)
}

fn region_excess_in(heard: &Heard, low: f64, high: f64, steady: bool, range: std::ops::Range<usize>, shift: &dyn Fn(f64) -> f64) -> f64 {
    let moved: Vec<f32> = (0..FINE_BINS).map(|bin| shift(fine_hz(bin)) as f32).collect();
    let frames = &heard.frames;
    let range = range.start.min(frames.fine.len())..range.end.min(frames.fine.len());
    let mut levels: Vec<f64> = frames.level[range.clone()].iter().map(|level| *level as f64).collect();
    levels.sort_by(f64::total_cmp);
    if levels.is_empty() {
        return 0.;
    }
    let loud = percentile(&levels, 0.95) - 20.;
    let bins: Vec<usize> = (1..FINE_BINS - 1).filter(|bin| (low..=high).contains(&fine_hz(*bin))).collect();
    if bins.is_empty() {
        return 0.;
    }
    let mut excesses: Vec<f64> = range
        .filter(|frame| frames.level[*frame] as f64 >= loud)
        .map(|frame| {
            let fine: Vec<f32> = frames.fine[frame].iter().zip(&moved).map(|(db, by)| db + by).collect();
            let power: Vec<f64> = fine.iter().map(|db| 10f64.powf(*db as f64 / 10.)).collect();
            bins.iter().map(|bin| fine[*bin] as f64 - detect::local_level(&power, *bin)).fold(f64::MIN, f64::max)
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

/// The smallest bell cut that brings a peak between `low` and `high` to `wanted` dB over its neighbours, predicted on
/// what was heard: every width from `q` out, every depth to 12 dB, the one that changes least of the rest of the
/// spectrum. With none enough, the deepest the limits allow. The band and the excess it predicts.
pub fn plan_cut(heard: &Heard, low: f64, high: f64, steady: bool, wanted: f64, q: f64, rate: f64) -> (Band, f64) {
    let center = (low * high).sqrt();
    let widths = [q, 0.7, 1., 1.4, 2., 2.8, 4., 5.6, 8.];
    let mut best: Option<(f64, Band, f64)> = None;
    let mut deepest: Option<(Band, f64)> = None;
    for &q in widths.iter().filter(|q| **q > 0.) {
        for step in 1..=24 {
            let band = Band { shape: Shape::Bell, hz: center, db: -0.5 * step as f64, q };
            let predicted = region_excess_with(heard, low, high, steady, &|hz| response(&band, hz, rate));
            if deepest.as_ref().is_none_or(|(_, excess)| predicted < *excess) {
                deepest = Some((band, predicted));
            }
            if predicted <= wanted {
                // What else it moves: the mean change over the fine bins, outside the peak's own band.
                let collateral =
                    (0..FINE_BINS).map(fine_hz).filter(|hz| *hz < low || *hz > high).map(|hz| response(&band, hz, rate).abs()).sum::<f64>()
                        / FINE_BINS as f64;
                let cost = collateral + band.db.abs() * 0.05;
                if best.as_ref().is_none_or(|(known, _, _)| cost < *known) {
                    best = Some((cost, band, predicted));
                }
                break;
            }
        }
    }
    match best {
        Some((_, band, predicted)) => (band, predicted),
        None => deepest.unwrap_or((Band { shape: Shape::Bell, hz: center, db: 0., q }, region_excess(heard, low, high, steady))),
    }
}
