//! Chopping at real hits (#401), built once for every DAW: a drum recording's hits found to the frame (`hits`), the break's
//! own tempo, the rolls and the densities (every hit, the main hits, kick and snare), and where each hit falls on the song's
//! grid of lines. The tool (`tool`) cuts the recording at the hits, lays the cuts out to play it as recorded, checks that by
//! ear, and offers lighter densities as takes of the variations loop; the DAW's backend (`backend`) holds only its own facts.
pub mod backend;
#[cfg(test)]
mod fake_backend_tests;
pub mod hits;
pub mod tool;

use hits::{Hit, Role};

/// The break's own tempo, and how it was told.
#[derive(Debug, Clone, PartialEq)]
pub struct Tempo {
    pub bars: Option<u32>,
    pub bpm: f64,
    pub how: String,
}

/// The tempo a recording `seconds` long plays at: as given, by its bars, else the whole bar count (1, 2, 4 or 8) whose tempo
/// is nearest the song's (a break is cut to whole bars).
pub fn break_tempo(seconds: f64, beats_per_bar: f64, song_bpm: f64, bars: Option<u32>, bpm: Option<f64>) -> Tempo {
    if let Some(bpm) = bpm {
        return Tempo { bars: None, bpm, how: "as asked".into() };
    }
    let at = |bars: u32| bars as f64 * beats_per_bar * 60. / seconds.max(1e-6);
    if let Some(bars) = bars {
        return Tempo { bars: Some(bars), bpm: at(bars), how: "as asked".into() };
    }
    let bars =
        [1u32, 2, 4, 8].into_iter().min_by(|a, b| (at(*a) / song_bpm).ln().abs().total_cmp(&(at(*b) / song_bpm).ln().abs())).unwrap_or(1);
    Tempo { bars: Some(bars), bpm: at(bars), how: "from its length, as whole bars nearest the song's tempo: say bars or bpm if not".into() }
}

/// Which hits are a roll's: those less than 0.7 of a sixteenth after the one before, in runs of three or more (every one but
/// the run's first).
pub fn rolls(hits: &[Hit], bpm: f64) -> Vec<bool> {
    let close = 0.7 * 60. / bpm / 4.;
    let mut out = vec![false; hits.len()];
    let mut start = 0;
    for k in 1..=hits.len() {
        let joined = k < hits.len() && hits[k].seconds - hits[k - 1].seconds < close;
        if !joined {
            if k - start >= 3 {
                for flag in &mut out[start + 1..k] {
                    *flag = true;
                }
            }
            start = k;
        }
    }
    out
}

/// How many of the hits a chop cuts or plays.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Density {
    /// Every hit, ghosts and rolls too.
    Every,
    /// The hits without the ghosts and the rolls' repeats.
    Main,
    /// Only the main kicks and snares.
    KickSnare,
}

impl Density {
    pub const ALL: [Density; 3] = [Density::Every, Density::Main, Density::KickSnare];

    pub fn name(self) -> &'static str {
        match self {
            Density::Every => "every",
            Density::Main => "main",
            Density::KickSnare => "kick-snare",
        }
    }

    pub fn words(self) -> &'static str {
        match self {
            Density::Every => "every hit, ghosts and rolls too",
            Density::Main => "the main hits, no ghosts or rolls",
            Density::KickSnare => "kick and snare only",
        }
    }

    pub fn of(text: &str) -> Option<Density> {
        Density::ALL.into_iter().find(|density| density.name() == text)
    }

    /// Whether a hit (a roll's repeat or not) is in it.
    pub fn keeps(self, hit: &Hit, roll: bool) -> bool {
        match self {
            Density::Every => true,
            Density::Main => !hit.ghost && !roll,
            Density::KickSnare => !hit.ghost && !roll && matches!(hit.role, Role::Kick | Role::Snare),
        }
    }
}

/// Where a moment `seconds` into the break falls at `bpm` (the break's own) on a grid of `lpb` lines a beat: whole lines and
/// 256ths of a line (the delay column), from the range's first line.
pub fn place(seconds: f64, bpm: f64, lpb: u32) -> (u32, u8) {
    let units = (seconds * bpm / 60. * lpb as f64 * 256.).round().max(0.) as u64;
    ((units / 256) as u32, (units % 256) as u8)
}

/// A cut moved onto the grid, when the break plays at the song's own tempo: back (under a 256th of a line) to the frame a line
/// and delay start exactly, so the cuts played in order are the recording again.
pub fn on_grid(frame: usize, rate: f64, bpm: f64, lpb: u32) -> usize {
    let unit = 60. / bpm / lpb as f64 / 256. * rate;
    let units = (frame as f64 / unit + 1e-9).floor();
    ((units * unit).round() as usize).min(frame)
}

/// Whether the break plays at the song's tempo (within a tenth of a percent).
pub fn same_tempo(break_bpm: f64, song_bpm: f64) -> bool {
    (break_bpm / song_bpm - 1.).abs() < 0.001
}

/// A hit in words, for the plan: "0.0 s kick", "0.35 s snare (ghost)".
pub fn hit_words(hit: &Hit, roll: bool) -> String {
    let mut words = format!("{:.3} s {}", hit.seconds, hit.role.word());
    if hit.ghost {
        words.push_str(" (ghost)");
    }
    if roll {
        words.push_str(" (roll)");
    }
    words
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hit(seconds: f64, role: Role, ghost: bool) -> Hit {
        Hit { frame: (seconds * 44_100.) as usize, seconds, peak_db: -6., role, ghost, lows: 0., highs: 0. }
    }

    #[test]
    fn a_breaks_tempo_is_the_whole_bars_nearest_the_songs() {
        // Two bars at 136.7 BPM are 3.511 seconds; at a song of 170 BPM two bars are nearest.
        let tempo = break_tempo(3.511, 4., 170., None, None);
        assert_eq!(tempo.bars, Some(2));
        assert!((tempo.bpm - 136.7).abs() < 0.1, "{tempo:?}");
        assert_eq!(break_tempo(3.511, 4., 170., Some(1), None).bpm.round(), 68.);
        assert_eq!(break_tempo(3.511, 4., 170., None, Some(140.)).bpm, 140.);
    }

    #[test]
    fn rolls_are_runs_of_three_hits_closer_than_a_sixteenth() {
        let sixteenth = 60. / 170. / 4.;
        let hits: Vec<Hit> = [0., 4., 4.5, 5., 5.5, 8., 9.].iter().map(|step| hit(step * sixteenth, Role::Snare, false)).collect();
        assert_eq!(rolls(&hits, 170.), vec![false, false, true, true, true, false, false]);
    }

    #[test]
    fn densities_keep_what_they_say() {
        let ghost = hit(0.1, Role::Snare, true);
        let hat = hit(0.2, Role::Hat, false);
        let kick = hit(0., Role::Kick, false);
        assert!(Density::Every.keeps(&ghost, true));
        assert!(!Density::Main.keeps(&ghost, false));
        assert!(Density::Main.keeps(&hat, false) && !Density::Main.keeps(&hat, true));
        assert!(!Density::KickSnare.keeps(&hat, false) && Density::KickSnare.keeps(&kick, false));
        assert_eq!(Density::of("kick-snare"), Some(Density::KickSnare));
    }

    #[test]
    fn a_hit_lands_on_its_line_and_delay_and_a_cut_on_the_grid_starts_exactly_there() {
        // At 170 BPM and 4 lines a beat, a sixteenth is a line: the fifth sixteenth is line 4, delay 0.
        let sixteenth = 60. / 170. / 4.;
        assert_eq!(place(4. * sixteenth, 170., 4), (4, 0));
        assert_eq!(place(4.5 * sixteenth, 170., 4), (4, 128));
        let rate = 44_100.;
        for frame in [441usize, 15_000, 77_777] {
            let snapped = on_grid(frame, rate, 170., 4);
            assert!(snapped <= frame && frame - snapped <= 16, "{frame} → {snapped}");
            let (line, delay) = place(snapped as f64 / rate, 170., 4);
            let back = (line as f64 + delay as f64 / 256.) * sixteenth * rate;
            assert!((back - snapped as f64).abs() < 0.51, "{frame}: {back} vs {snapped}");
        }
    }
}
