//! The choices a mode takes beyond the shared options: how resonate pairs its sounds, how modulate shapes its
//! carrier, and how a loop is built (its meter and groove and, for a melody, its key, scale and tuning). Every choice
//! is optional: one left out is drawn from the seed, so a reroll varies it.

use super::{refuse, DesignError};
use serde::{Deserialize, Serialize};

/// Who rings through whom in a resonance.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Pairing {
    /// A half drawn from the seed rings through the other half.
    Halves,
    /// The first half, as picked, through the second.
    InOrder,
    /// The 1st, 3rd, 5th… through the 2nd, 4th, 6th….
    Interleave,
    /// The brighter half through the darker.
    BrightDark,
    /// The darker half through the brighter.
    DarkBright,
    /// The punchy, short ones through the tonal, long ones.
    HitsTones,
    /// The tonal, long ones through the punchy, short ones.
    TonesHits,
    /// The first through all the others.
    OneAll,
    /// All the others through the last.
    AllOne,
    /// Each through the next: the first rings the second, and that rings the third.
    Chain,
}

impl Pairing {
    pub const ALL: [Pairing; 10] = [
        Pairing::Halves,
        Pairing::InOrder,
        Pairing::Interleave,
        Pairing::BrightDark,
        Pairing::DarkBright,
        Pairing::HitsTones,
        Pairing::TonesHits,
        Pairing::OneAll,
        Pairing::AllOne,
        Pairing::Chain,
    ];
}

/// How a resonator rings: by its strongest partials, as a body (its own sound as an impulse response), or both.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Ring {
    Partials,
    Body,
    Both,
}

impl Ring {
    pub const ALL: [Ring; 3] = [Ring::Partials, Ring::Body, Ring::Both];
}

/// Resonate's choices (cross:resonate). `Options::split`, when given, still names the exciters itself.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Resonate {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pairing: Option<Pairing>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ring: Option<Ring>,
    /// 0, a short ring, to 1, a long one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decay: Option<f64>,
}

/// Which sound modulate plays and shapes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Carrier {
    Random,
    First,
    Last,
    Brightest,
    Darkest,
    Longest,
    Shortest,
    Tonal,
    Noisy,
}

impl Carrier {
    pub const ALL: [Carrier; 9] = [
        Carrier::Random,
        Carrier::First,
        Carrier::Last,
        Carrier::Brightest,
        Carrier::Darkest,
        Carrier::Longest,
        Carrier::Shortest,
        Carrier::Tonal,
        Carrier::Noisy,
    ];
}

/// A way one sound shapes another.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Modulation {
    /// The carrier read faster and slower as the modulator moves (phase modulation).
    Phase,
    /// Multiplied by the modulator (ring modulation).
    Ring,
    /// Its level swayed by the modulator (amplitude modulation).
    Am,
    /// A tone the modulator bends (frequency modulation), shaped like the carrier.
    Fm,
    /// A filter whose cutoff follows the modulator.
    Filter,
    /// Its level following or ducking under the modulator's.
    Gain,
    /// The modulator's spectrum on the carrier's sound.
    Vocode,
}

impl Modulation {
    pub const ALL: [Modulation; 7] =
        [Modulation::Phase, Modulation::Ring, Modulation::Am, Modulation::Fm, Modulation::Filter, Modulation::Gain, Modulation::Vocode];
}

/// Modulate's choices (cross:modulate).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Modulate {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub carrier: Option<Carrier>,
    /// The ways allowed, in any order; not given, every way at its usual odds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kinds: Option<Vec<Modulation>>,
    /// How many shapings run, one after another (1 to 8).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stages: Option<u32>,
    /// The chance a modulator much shorter than the carrier repeats along it (0 to 1).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retrigger: Option<f64>,
}

/// A loop of hits, or a melody of the sounds repitched.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Style {
    Beat,
    Melody,
}

impl Style {
    pub const ALL: [Style; 2] = [Style::Beat, Style::Melody];
}

/// A scale, as the cents of its degrees above the root in one octave.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Scale {
    Major,
    Minor,
    Dorian,
    Phrygian,
    Lydian,
    Mixolydian,
    Locrian,
    HarmonicMinor,
    MelodicMinor,
    Pentatonic,
    MinorPentatonic,
    Blues,
    WholeTone,
    Chromatic,
    Hirajoshi,
    InSen,
    PhrygianDominant,
    HungarianMinor,
    Pelog,
    Slendro,
}

impl Scale {
    pub const ALL: [Scale; 20] = [
        Scale::Major,
        Scale::Minor,
        Scale::Dorian,
        Scale::Phrygian,
        Scale::Lydian,
        Scale::Mixolydian,
        Scale::Locrian,
        Scale::HarmonicMinor,
        Scale::MelodicMinor,
        Scale::Pentatonic,
        Scale::MinorPentatonic,
        Scale::Blues,
        Scale::WholeTone,
        Scale::Chromatic,
        Scale::Hirajoshi,
        Scale::InSen,
        Scale::PhrygianDominant,
        Scale::HungarianMinor,
        Scale::Pelog,
        Scale::Slendro,
    ];
    /// The scale's degrees, cents above the root, within one octave. Pelog and slendro are as Javanese gamelans are
    /// often measured, not their 12-tone stand-ins.
    pub fn cents(self) -> &'static [f64] {
        match self {
            Scale::Major => &[0.0, 200.0, 400.0, 500.0, 700.0, 900.0, 1100.0],
            Scale::Minor => &[0.0, 200.0, 300.0, 500.0, 700.0, 800.0, 1000.0],
            Scale::Dorian => &[0.0, 200.0, 300.0, 500.0, 700.0, 900.0, 1000.0],
            Scale::Phrygian => &[0.0, 100.0, 300.0, 500.0, 700.0, 800.0, 1000.0],
            Scale::Lydian => &[0.0, 200.0, 400.0, 600.0, 700.0, 900.0, 1100.0],
            Scale::Mixolydian => &[0.0, 200.0, 400.0, 500.0, 700.0, 900.0, 1000.0],
            Scale::Locrian => &[0.0, 100.0, 300.0, 500.0, 600.0, 800.0, 1000.0],
            Scale::HarmonicMinor => &[0.0, 200.0, 300.0, 500.0, 700.0, 800.0, 1100.0],
            Scale::MelodicMinor => &[0.0, 200.0, 300.0, 500.0, 700.0, 900.0, 1100.0],
            Scale::Pentatonic => &[0.0, 200.0, 400.0, 700.0, 900.0],
            Scale::MinorPentatonic => &[0.0, 300.0, 500.0, 700.0, 1000.0],
            Scale::Blues => &[0.0, 300.0, 500.0, 600.0, 700.0, 1000.0],
            Scale::WholeTone => &[0.0, 200.0, 400.0, 600.0, 800.0, 1000.0],
            Scale::Chromatic => &[0.0, 100.0, 200.0, 300.0, 400.0, 500.0, 600.0, 700.0, 800.0, 900.0, 1000.0, 1100.0],
            Scale::Hirajoshi => &[0.0, 200.0, 300.0, 700.0, 800.0],
            Scale::InSen => &[0.0, 100.0, 500.0, 700.0, 1000.0],
            Scale::PhrygianDominant => &[0.0, 100.0, 400.0, 500.0, 700.0, 800.0, 1000.0],
            Scale::HungarianMinor => &[0.0, 200.0, 300.0, 600.0, 700.0, 800.0, 1100.0],
            Scale::Pelog => &[0.0, 120.0, 270.0, 540.0, 670.0, 785.0, 950.0],
            Scale::Slendro => &[0.0, 240.0, 480.0, 720.0, 960.0],
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            Scale::Major => "major",
            Scale::Minor => "minor",
            Scale::Dorian => "Dorian",
            Scale::Phrygian => "Phrygian",
            Scale::Lydian => "Lydian",
            Scale::Mixolydian => "Mixolydian",
            Scale::Locrian => "Locrian",
            Scale::HarmonicMinor => "harmonic minor",
            Scale::MelodicMinor => "melodic minor",
            Scale::Pentatonic => "pentatonic",
            Scale::MinorPentatonic => "minor pentatonic",
            Scale::Blues => "blues",
            Scale::WholeTone => "whole-tone",
            Scale::Chromatic => "chromatic",
            Scale::Hirajoshi => "hirajoshi",
            Scale::InSen => "in-sen",
            Scale::PhrygianDominant => "Phrygian dominant",
            Scale::HungarianMinor => "Hungarian minor",
            Scale::Pelog => "pelog",
            Scale::Slendro => "slendro",
        }
    }
}

/// How the notes are tuned: equal divisions of the octave, or ratios.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Tuning {
    #[serde(rename = "12-tet")]
    Tet12,
    #[serde(rename = "19-tet")]
    Tet19,
    #[serde(rename = "22-tet")]
    Tet22,
    #[serde(rename = "24-tet")]
    Tet24,
    #[serde(rename = "31-tet")]
    Tet31,
    /// Five-limit just intonation.
    #[serde(rename = "just")]
    Just,
    /// Stacked pure fifths.
    #[serde(rename = "pythagorean")]
    Pythagorean,
    /// Thirteen equal steps of the 3:1 twelfth instead of the octave.
    #[serde(rename = "bohlen-pierce")]
    BohlenPierce,
    /// The harmonic series' 8th to 16th partials.
    #[serde(rename = "harmonic")]
    Harmonic,
}

impl Tuning {
    pub const ALL: [Tuning; 9] = [
        Tuning::Tet12,
        Tuning::Tet19,
        Tuning::Tet22,
        Tuning::Tet24,
        Tuning::Tet31,
        Tuning::Just,
        Tuning::Pythagorean,
        Tuning::BohlenPierce,
        Tuning::Harmonic,
    ];
    pub fn name(self) -> &'static str {
        match self {
            Tuning::Tet12 => "12-TET",
            Tuning::Tet19 => "19-TET",
            Tuning::Tet22 => "22-TET",
            Tuning::Tet24 => "24-TET",
            Tuning::Tet31 => "31-TET",
            Tuning::Just => "just intonation",
            Tuning::Pythagorean => "Pythagorean",
            Tuning::BohlenPierce => "Bohlen–Pierce",
            Tuning::Harmonic => "harmonic series",
        }
    }
    /// The pitches the tuning has within its period, cents above the root, and the period (cents): the scale's
    /// degrees are moved to the nearest of these.
    pub fn grid(self) -> (Vec<f64>, f64) {
        let equal = |n: usize| ((0..n).map(|i| i as f64 * 1200.0 / n as f64).collect(), 1200.0);
        let ratios = |list: &[f64]| (list.iter().map(|r| 1200.0 * r.log2()).collect(), 1200.0);
        match self {
            Tuning::Tet12 => equal(12),
            Tuning::Tet19 => equal(19),
            Tuning::Tet22 => equal(22),
            Tuning::Tet24 => equal(24),
            Tuning::Tet31 => equal(31),
            Tuning::Just => ratios(&[1.0, 16.0 / 15.0, 9.0 / 8.0, 6.0 / 5.0, 5.0 / 4.0, 4.0 / 3.0, 45.0 / 32.0, 3.0 / 2.0, 8.0 / 5.0, 5.0 / 3.0, 9.0 / 5.0, 15.0 / 8.0]),
            Tuning::Pythagorean => ratios(&[
                1.0,
                256.0 / 243.0,
                9.0 / 8.0,
                32.0 / 27.0,
                81.0 / 64.0,
                4.0 / 3.0,
                729.0 / 512.0,
                3.0 / 2.0,
                128.0 / 81.0,
                27.0 / 16.0,
                16.0 / 9.0,
                243.0 / 128.0,
            ]),
            Tuning::BohlenPierce => {
                let tritave = 1200.0 * 3f64.log2();
                ((0..13).map(|i| i as f64 * tritave / 13.0).collect(), tritave)
            }
            Tuning::Harmonic => ratios(&(8..16).map(|h| h as f64 / 8.0).collect::<Vec<_>>()),
        }
    }
    /// The smallest step between neighbouring pitches of the tuning, cents: how far `micro` bends a note.
    pub fn step(self) -> f64 {
        let (grid, period) = self.grid();
        let mut least = period;
        for (i, cents) in grid.iter().enumerate() {
            let next = grid.get(i + 1).copied().unwrap_or(period);
            least = least.min(next - cents);
        }
        least
    }
}

/// How a melody moves through its notes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Contour {
    Up,
    Down,
    UpDown,
    Walk,
    Leaps,
    Random,
}

impl Contour {
    pub const ALL: [Contour; 6] = [Contour::Up, Contour::Down, Contour::UpDown, Contour::Walk, Contour::Leaps, Contour::Random];
}

/// A loop's choices (fuse:loop). The tempo and bars are `Options::bpm` and `Options::bars`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Loop {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub style: Option<Style>,
    /// Beats to the bar and the note a beat is: `[7, 8]` is 7/8.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub meter: Option<[u32; 2]>,
    /// Steps a beat: 2 (eighths in 4/4), 3 (triplets), 4 (sixteenths), 6, 8.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub grid: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub swing: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub density: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub humanize: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub accent: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fills: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ratchet: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub poly: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub variation: Option<f64>,
    /// Beat: hits clustered toward the loop's start (-1) or end (1).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lean: Option<f64>,
    /// Beat: hits pulled onto the off-beats (0 to 1).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub syncopation: Option<f64>,
    /// Beat: quiet ghost notes between the hits (0 to 1).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ghosts: Option<f64>,
    /// Beat: the chance a hat rings open (0 to 1).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub open: Option<f64>,
    /// Beat: how busy each voice is (0 to 1).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kick: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snare: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hats: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub perc: Option<f64>,
    /// A melody's root: `C` to `B`, sharps as `C#` and flats as `Db`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scale: Option<Scale>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tuning: Option<Tuning>,
    /// The chance a note bends off its scale by the tuning's smallest step.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub micro: Option<f64>,
    /// How many octaves the melody spans (1 to 4).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub octaves: Option<u32>,
    /// Octaves up or down from the middle (-2 to 2).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub register: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub contour: Option<Contour>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chords: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub glide: Option<f64>,
    /// How much of the time to its next note a note sounds (0.05 to 1).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gate: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub drone: Option<f64>,
}

/// The pitch classes by name, C at 0: sharps and flats, ASCII or not.
pub fn key_index(name: &str) -> Option<usize> {
    let mut chars = name.trim().chars();
    let letter = chars.next()?.to_ascii_uppercase();
    let base: i32 = match letter {
        'C' => 0,
        'D' => 2,
        'E' => 4,
        'F' => 5,
        'G' => 7,
        'A' => 9,
        'B' => 11,
        _ => return None,
    };
    let shift = match chars.as_str() {
        "" => 0,
        "#" | "♯" => 1,
        "b" | "♭" => -1,
        _ => return None,
    };
    Some((base + shift).rem_euclid(12) as usize)
}

pub const KEY_NAMES: [&str; 12] = ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"];

fn within(value: Option<f64>, low: f64, high: f64, what: &str) -> Result<(), DesignError> {
    match value {
        Some(v) if !(v.is_finite() && (low..=high).contains(&v)) => refuse(format!("{what} is {low} to {high}.")),
        _ => Ok(()),
    }
}

impl Resonate {
    pub(crate) fn check(&self) -> Result<(), DesignError> {
        within(self.decay, 0.0, 1.0, "resonate's decay")
    }
}

impl Modulate {
    pub(crate) fn check(&self) -> Result<(), DesignError> {
        within(self.retrigger, 0.0, 1.0, "modulate's retrigger")?;
        if self.stages.is_some_and(|stages| !(1..=8).contains(&stages)) {
            return refuse("modulate's stages are 1 to 8.");
        }
        if self.kinds.as_ref().is_some_and(Vec::is_empty) {
            return refuse("modulate's kinds name at least one way to modulate.");
        }
        Ok(())
    }
}

impl Loop {
    pub(crate) fn check(&self) -> Result<(), DesignError> {
        if let Some([beats, unit]) = self.meter {
            if !(1..=16).contains(&beats) || ![2, 4, 8, 16].contains(&unit) {
                return refuse("A loop's meter is [beats, unit]: 1 to 16 beats of a 2, 4, 8 or 16.");
            }
        }
        if self.grid.is_some_and(|grid| ![2, 3, 4, 6, 8].contains(&grid)) {
            return refuse("A loop's grid is 2, 3, 4, 6 or 8 steps a beat.");
        }
        within(self.swing, 0.0, 0.75, "swing")?;
        within(self.lean, -1.0, 1.0, "lean")?;
        for (value, what) in [
            (self.density, "density"),
            (self.humanize, "humanize"),
            (self.accent, "accent"),
            (self.fills, "fills"),
            (self.ratchet, "ratchet"),
            (self.poly, "poly"),
            (self.variation, "variation"),
            (self.syncopation, "syncopation"),
            (self.ghosts, "ghosts"),
            (self.open, "open"),
            (self.kick, "kick"),
            (self.snare, "snare"),
            (self.hats, "hats"),
            (self.perc, "perc"),
            (self.micro, "micro"),
            (self.chords, "chords"),
            (self.glide, "glide"),
            (self.drone, "drone"),
        ] {
            within(value, 0.0, 1.0, what)?;
        }
        within(self.gate, 0.05, 1.0, "gate")?;
        if let Some(key) = &self.key {
            if key_index(key).is_none() {
                return refuse("A loop's key is a note name: C to B, with # or b.");
            }
        }
        if self.octaves.is_some_and(|octaves| !(1..=4).contains(&octaves)) {
            return refuse("A melody spans 1 to 4 octaves.");
        }
        if self.register.is_some_and(|register| !(-2..=2).contains(&register)) {
            return refuse("A melody's register is -2 to 2 octaves.");
        }
        Ok(())
    }
}
