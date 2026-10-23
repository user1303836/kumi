//! Time in the notation: positions as `bar|beat` in a frame's meter, and lengths as note values or beats. Both are
//! exact on the grid of 960 ticks to a quarter note, and decimals off it.

use super::harmony::Key;

/// Ticks to a quarter note: fine enough for 1/128 notes, triplets and quintuplets alike.
pub const TICKS: f64 = 960.;
/// How close two times are to count as one (float noise, far below anything audible).
pub(super) const EPSILON: f64 = 1e-9;

/// The time a text's positions are in.
#[derive(Debug, Clone, PartialEq)]
pub struct Frame {
    /// The clip's start in the positions' time, in beats. It's 0 when they're the clip's own (a Session clip), and
    /// the clip's place in the Set when they're song time (the Arrangement).
    pub origin: f64,
    /// The meter bars are counted in: the clip's own for a Session clip, the Set's for the Arrangement.
    pub numerator: u32,
    pub denominator: u32,
    /// The tempo, for shifts in milliseconds.
    pub tempo: f64,
    /// The clip's length in beats, where a lane's `*` stops.
    pub length: Option<f64>,
    /// The track's Drum Rack pads (name and pitch). Lane names are looked up in them before General MIDI's drums.
    pub pads: Vec<(String, u8)>,
    /// Whether the notes are drums (a Drum Rack track): a print writes them as lanes where they fit.
    pub drums: bool,
    /// The key roman numerals are read in until a `key` line names another: the Set's scale, when it has one.
    pub key: Option<Key>,
}
impl Default for Frame {
    fn default() -> Self {
        Self { origin: 0., numerator: 4, denominator: 4, tempo: 120., length: None, pads: vec![], drums: false, key: None }
    }
}
impl Frame {
    /// Beats (quarter notes) in a bar.
    pub fn bar(&self) -> f64 {
        self.numerator as f64 * 4. / self.denominator as f64
    }
    /// Beats in the meter's beat (6/8: an eighth, half a beat).
    pub fn unit(&self) -> f64 {
        4. / self.denominator as f64
    }
    /// The time a bar starts at (bar 1 at 0).
    pub fn bar_start(&self, bar: u32) -> f64 {
        (bar as f64 - 1.) * self.bar()
    }
    /// The bar a time is in (from 1), float noise at its end counted in the next.
    pub fn bar_of(&self, time: f64) -> u32 {
        let bar = (time / self.bar()).floor();
        let bar = if (bar + 1.) * self.bar() - time < EPSILON { bar + 1. } else { bar };
        (bar.max(0.) as u32).saturating_add(1)
    }
    /// A time as `bar|beat`. The beat is a whole number, a decimal for halves and quarters and anything off the grid
    /// (`3|2.5`, `3|2.0234`), or a whole number and a fraction (`3|1+1/3`).
    pub fn position(&self, time: f64) -> String {
        let bar = self.bar_of(time);
        let within = (time - self.bar_start(bar)).max(0.);
        let beats = within / self.unit();
        let whole = if beats.ceil() - beats < EPSILON { beats.ceil() } else { beats.floor() };
        let rest = (beats - whole).max(0.);
        let beat = (whole as u64).saturating_add(1);
        if rest < EPSILON {
            return format!("{bar}|{beat}");
        }
        let ticks = rest * self.unit() * TICKS;
        if (ticks - ticks.round()).abs() < 1e-6 {
            let (unit, ticks) = ((self.unit() * TICKS).round() as u64, ticks.round() as u64);
            let common = gcd(ticks, unit);
            let (numerator, denominator) = (ticks / common, unit / common);
            if !denominator.is_power_of_two() {
                return format!("{bar}|{beat}+{numerator}/{denominator}");
            }
        }
        format!("{bar}|{}", decimal(1. + beats))
    }
    /// The time of a `bar|beat` (see `position`), if it is one.
    pub fn parse_position(&self, text: &str) -> Option<f64> {
        let (bar, beat) = text.split_once('|')?;
        let bar: u32 = bar.parse().ok().filter(|bar| *bar >= 1)?;
        let (beat, fraction) = match beat.split_once('+') {
            Some((beat, fraction)) => (beat, Some(fraction)),
            None => (beat, None),
        };
        let beat: f64 = beat.parse().ok().filter(|beat: &f64| *beat >= 1. && beat.is_finite())?;
        let mut within = (beat - 1.) * self.unit();
        if let Some(fraction) = fraction {
            let (numerator, denominator) = fraction.split_once('/')?;
            let (numerator, denominator): (u32, u32) = (numerator.parse().ok()?, denominator.parse().ok().filter(|d| *d > 0)?);
            within += numerator as f64 / denominator as f64 * self.unit();
        }
        Some(self.bar_start(bar) + within)
    }
}

/// A length in beats: a note value (`/8`, `/8.` dotted, `/8t` triplet, `3/8`) or beats (`0.37b`).
pub fn parse_length(text: &str) -> Option<f64> {
    if let Some(beats) = text.strip_suffix('b') {
        return beats.parse::<f64>().ok().filter(|beats| *beats > 0. && beats.is_finite());
    }
    let (count, value) = text.split_once('/')?;
    let count: f64 = if count.is_empty() { 1. } else { count.parse::<u32>().ok().filter(|count| *count > 0)? as f64 };
    let (value, factor) = match (value.strip_suffix('t'), value.strip_suffix('.')) {
        (Some(value), _) => (value, 2. / 3.),
        (_, Some(value)) => (value, 1.5),
        _ => (value, 1.),
    };
    let value: u32 = value.parse().ok().filter(|value| [1, 2, 4, 8, 16, 32, 64, 128].contains(value))?;
    Some(count * 4. / value as f64 * factor)
}
/// The shortest literal for a length in beats: a note value on the grid, beats off it.
pub fn length(beats: f64) -> String {
    let ticks = beats * TICKS;
    if (ticks - ticks.round()).abs() < 1e-6 && ticks.round() >= 1. {
        let ticks = ticks.round() as u64;
        for value in [1, 2, 4, 8, 16, 32, 64, 128] {
            let base = 3840 / value;
            if ticks == base {
                return format!("/{value}");
            }
            if ticks * 2 == base * 3 {
                return format!("/{value}.");
            }
            if ticks * 3 == base * 2 {
                return format!("/{value}t");
            }
        }
        for value in [1, 2, 4, 8, 16, 32, 64, 128] {
            let base = 3840 / value;
            if ticks.is_multiple_of(base) {
                return format!("{}/{value}", ticks / base);
            }
        }
    }
    format!("{}b", decimal(beats))
}
/// The shortest decimal that reads back as the number, to well below float noise in a time (trailing digits a
/// subtraction leaves, like `2.0234000000000005`, go).
pub fn decimal(value: f64) -> String {
    for places in 0..=12 {
        let text = format!("{value:.places$}");
        if text.parse::<f64>().is_ok_and(|read| (read - value).abs() < 1e-11) {
            return text;
        }
    }
    format!("{value}")
}
fn gcd(a: u64, b: u64) -> u64 {
    if b == 0 {
        a
    } else {
        gcd(b, a % b)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn positions_read_and_write_alike_in_four_four_and_six_eight() {
        let four = Frame::default();
        for (text, time) in [("1|1", 0.), ("3|2", 9.), ("3|2.5", 9.5), ("2|1+1/3", 4. + 1. / 3.), ("1|4.75", 3.75)] {
            assert_eq!((four.parse_position(text), four.position(time)), (Some(time), text.to_owned()), "{text}");
        }
        let six = Frame { numerator: 6, denominator: 8, ..Frame::default() };
        assert_eq!((six.bar(), six.parse_position("2|4")), (3., Some(4.5)));
        assert_eq!(six.position(4.5), "2|4");
        // Off the grid: a decimal that reads back to the time.
        let recorded = 9.0234;
        assert_eq!(four.position(recorded), "3|2.0234");
        assert!((four.parse_position(&four.position(recorded)).unwrap() - recorded).abs() < EPSILON);
        assert_eq!(four.parse_position("0|1").or(four.parse_position("1|0")).or(four.parse_position("1")), None);
        // The last bar a number can name, and times past it, stay there instead of overflowing.
        let last = four.parse_position(&format!("{}|4", u32::MAX)).unwrap();
        assert_eq!((four.bar_of(last), four.bar_of(last * 2.)), (u32::MAX, u32::MAX));
        assert!(four.position(last * 2.).starts_with(&format!("{}|", u32::MAX)));
    }

    #[test]
    fn lengths_are_note_values_on_the_grid_and_beats_off_it() {
        for (text, beats) in
            [("/4", 1.), ("/8.", 0.75), ("/4.", 1.5), ("/8t", 1. / 3.), ("/1", 4.), ("2/1", 8.), ("5/16", 1.25), ("/64", 0.0625)]
        {
            assert_eq!((parse_length(text), length(beats)), (Some(beats), text.to_owned()), "{text}");
        }
        assert_eq!(parse_length("3/8"), Some(1.5));
        assert_eq!((parse_length("0.37b"), length(0.37)), (Some(0.37), "0.37b".to_owned()));
        assert_eq!(parse_length("/7").or(parse_length("0/4")).or(parse_length("-1b")), None);
    }
}
