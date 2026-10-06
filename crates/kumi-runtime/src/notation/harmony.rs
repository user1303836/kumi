//! Keys, chord symbols and roman numerals, for the notation's `{…}`: voiced close, the root from F2 to E3, and a
//! slash chord's bass below it.
use super::pitch;

/// A tonic and its mode, which roman numerals are read in (IV in D Dorian is G major).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Key {
    pub tonic: i32,
    pub steps: [i32; 7],
}
const MODES: &[(&str, [i32; 7])] = &[
    ("major", [0, 2, 4, 5, 7, 9, 11]),
    ("ionian", [0, 2, 4, 5, 7, 9, 11]),
    ("dorian", [0, 2, 3, 5, 7, 9, 10]),
    ("phrygian", [0, 1, 3, 5, 7, 8, 10]),
    ("lydian", [0, 2, 4, 6, 7, 9, 11]),
    ("mixolydian", [0, 2, 4, 5, 7, 9, 10]),
    ("minor", [0, 2, 3, 5, 7, 8, 10]),
    ("aeolian", [0, 2, 3, 5, 7, 8, 10]),
    ("locrian", [0, 1, 3, 5, 6, 8, 10]),
    ("harmonicminor", [0, 2, 3, 5, 7, 8, 11]),
    ("melodicminor", [0, 2, 3, 5, 7, 9, 11]),
];
impl Key {
    /// `D dorian`, `F# minor`, `A harmonic minor`, or `Bb` alone (major).
    pub fn parse(text: &str) -> Option<Key> {
        let (tonic, rest) = pitch::class(text.trim())?;
        let mode: String = rest.chars().filter(|c| !c.is_whitespace() && *c != '-').collect::<String>().to_lowercase();
        let mode = if mode.is_empty() { "major" } else { mode.as_str() };
        MODES.iter().find(|(name, _)| *name == mode).map(|(_, steps)| Key { tonic: tonic.rem_euclid(12), steps: *steps })
    }
}

/// Chord qualities: what follows the root, and the intervals above it.
const QUALITIES: &[(&str, &[i32])] = &[
    ("", &[0, 4, 7]),
    ("maj", &[0, 4, 7]),
    ("m", &[0, 3, 7]),
    ("min", &[0, 3, 7]),
    ("-", &[0, 3, 7]),
    ("dim", &[0, 3, 6]),
    ("°", &[0, 3, 6]),
    ("aug", &[0, 4, 8]),
    ("+", &[0, 4, 8]),
    ("sus2", &[0, 2, 7]),
    ("sus4", &[0, 5, 7]),
    ("sus", &[0, 5, 7]),
    ("5", &[0, 7]),
    ("6", &[0, 4, 7, 9]),
    ("m6", &[0, 3, 7, 9]),
    ("69", &[0, 4, 7, 9, 14]),
    ("m69", &[0, 3, 7, 9, 14]),
    ("7", &[0, 4, 7, 10]),
    ("maj7", &[0, 4, 7, 11]),
    ("M7", &[0, 4, 7, 11]),
    ("Δ", &[0, 4, 7, 11]),
    ("Δ7", &[0, 4, 7, 11]),
    ("m7", &[0, 3, 7, 10]),
    ("min7", &[0, 3, 7, 10]),
    ("-7", &[0, 3, 7, 10]),
    ("mmaj7", &[0, 3, 7, 11]),
    ("mM7", &[0, 3, 7, 11]),
    ("m7b5", &[0, 3, 6, 10]),
    ("ø", &[0, 3, 6, 10]),
    ("ø7", &[0, 3, 6, 10]),
    ("dim7", &[0, 3, 6, 9]),
    ("°7", &[0, 3, 6, 9]),
    ("aug7", &[0, 4, 8, 10]),
    ("+7", &[0, 4, 8, 10]),
    ("7sus4", &[0, 5, 7, 10]),
    ("7sus2", &[0, 2, 7, 10]),
    ("7b5", &[0, 4, 6, 10]),
    ("7b9", &[0, 4, 7, 10, 13]),
    ("7#9", &[0, 4, 7, 10, 15]),
    ("7#11", &[0, 4, 7, 10, 18]),
    ("add9", &[0, 4, 7, 14]),
    ("madd9", &[0, 3, 7, 14]),
    ("9", &[0, 4, 7, 10, 14]),
    ("maj9", &[0, 4, 7, 11, 14]),
    ("m9", &[0, 3, 7, 10, 14]),
    ("11", &[0, 4, 7, 10, 14, 17]),
    ("m11", &[0, 3, 7, 10, 14, 17]),
    ("13", &[0, 4, 7, 10, 14, 21]),
    ("maj13", &[0, 4, 7, 11, 14, 21]),
    ("m13", &[0, 3, 7, 10, 14, 21]),
];
const NUMERALS: [&str; 7] = ["vii", "iii", "vi", "iv", "ii", "v", "i"];

/// The pitches of a chord symbol (`Cm7`, `F#dim`, `Cm7/G`), or of a roman numeral read in the key (`IV`, `ii7`,
/// `bVII`, `V7/B`; uppercase is major, lowercase minor), voiced close.
pub fn chord(text: &str, key: Option<&Key>) -> Result<Vec<u8>, String> {
    let (body, bass) = match text.rsplit_once('/') {
        Some((body, bass)) => {
            let (class, _) = pitch::class(bass).filter(|(_, rest)| rest.is_empty()).ok_or_else(|| {
                format!("the bass after “/” in “{text}” isn't a note name: write one like {{Cm7/G}} (numerals don't go after a slash)")
            })?;
            (body, Some(class.rem_euclid(12)))
        }
        None => (text, None),
    };
    let (root, quality) = match pitch::class(body) {
        Some((root, quality)) if !numeral(body) => (root, quality.to_owned()),
        _ => roman(body, key)?,
    };
    let intervals = QUALITIES.iter().find(|(name, _)| *name == quality).map(|(_, intervals)| *intervals).ok_or_else(|| {
        format!("“{text}” isn't a chord Kumi knows: try a root and a quality like C, Cm, C7, Cmaj7, Cm7, Cdim, Csus4, Cadd9, Cm7b5 or C13")
    })?;
    Ok(voice(root.rem_euclid(12), intervals, bass))
}
/// Whether a chord begins with a roman numeral (an accidental first is allowed), not a note name.
fn numeral(text: &str) -> bool {
    let text = text.trim_start_matches(['b', '#']);
    text.starts_with(['I', 'V', 'i', 'v']) && !text.starts_with(|c: char| c.is_ascii_uppercase() && "ABCDEFG".contains(c))
}
/// A roman numeral's root (in the key) and its quality, as a chord symbol would name it.
fn roman(text: &str, key: Option<&Key>) -> Result<(i32, String), String> {
    let key = key.ok_or_else(|| format!("{{{text}}} needs a key first: put a line like `key C major` or `key D dorian` above it"))?;
    let (shift, rest) = match text.as_bytes().first() {
        Some(b'b') => (-1, &text[1..]),
        Some(b'#') => (1, &text[1..]),
        _ => (0, text),
    };
    let lower = rest.to_lowercase();
    let (degree, numeral) = NUMERALS
        .iter()
        .find(|numeral| lower.starts_with(**numeral))
        .map(|numeral| (["i", "ii", "iii", "iv", "v", "vi", "vii"].iter().position(|n| n == numeral).unwrap(), numeral.len()))
        .ok_or_else(|| format!("“{text}” isn't a chord symbol or a roman numeral"))?;
    let (written, suffix) = rest.split_at(numeral);
    let minor = if written.chars().all(|c| c.is_ascii_lowercase()) {
        true
    } else if written.chars().all(|c| c.is_ascii_uppercase()) {
        false
    } else {
        return Err(format!("“{written}” mixes cases: uppercase is a major chord, lowercase a minor one"));
    };
    let quality = match (minor, suffix) {
        (_, "°" | "dim" | "o") => "dim".to_owned(),
        (_, "°7" | "dim7" | "o7") => "dim7".to_owned(),
        (_, "ø" | "ø7" | "m7b5") => "m7b5".to_owned(),
        (_, "+" | "aug") => "aug".to_owned(),
        (_, "+7" | "aug7") => "aug7".to_owned(),
        (false, suffix) => suffix.to_owned(),
        (true, "maj7") => "mmaj7".to_owned(),
        (true, suffix) => format!("m{suffix}"),
    };
    Ok((key.tonic + key.steps[degree] + shift, quality))
}
/// A chord voiced close: the root from F2 (53) to E3 (64), the rest stacked above it, and a bass below them all.
fn voice(root: i32, intervals: &[i32], bass: Option<i32>) -> Vec<u8> {
    let low = 53 + (root - 5).rem_euclid(12);
    let mut pitches: Vec<i32> = intervals.iter().map(|interval| low + interval).collect();
    if let Some(bass) = bass {
        let lowest = pitches[0];
        let below = lowest - (lowest - bass).rem_euclid(12);
        pitches.insert(0, if below == lowest { below - 12 } else { below });
    }
    pitches.into_iter().filter(|pitch| (0..=127).contains(pitch)).map(|pitch| pitch as u8).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chord_symbols_are_voiced_close_with_a_slash_bass_below() {
        assert_eq!(chord("C", None), Ok(vec![60, 64, 67]));
        assert_eq!(chord("Cm7", None), Ok(vec![60, 63, 67, 70]));
        assert_eq!(chord("F#dim", None), Ok(vec![54, 57, 60]));
        assert_eq!(chord("Cm7/G", None), Ok(vec![55, 60, 63, 67, 70]), "G below the C");
        assert_eq!(chord("Am/A", None), Ok(vec![45, 57, 60, 64]), "a bass of the root goes an octave down");
        assert!(chord("Cxyz", None).unwrap_err().contains("isn't a chord"));
        assert!(chord("C/V", None).unwrap_err().contains("isn't a note name"));
    }

    #[test]
    fn numerals_are_read_in_the_keys_mode() {
        let dorian = Key::parse("D dorian").unwrap();
        assert_eq!(chord("IV", Some(&dorian)), Ok(vec![55, 59, 62]), "G major");
        assert_eq!(chord("ii7", Some(&dorian)), Ok(vec![64, 67, 71, 74]), "E minor seventh");
        let minor = Key::parse("A minor").unwrap();
        assert_eq!(chord("bVII", Some(&minor)), Ok(vec![54, 58, 61]), "F#: a flat on the minor's G");
        assert_eq!(chord("V7/B", Some(&Key::parse("C").unwrap())), Ok(vec![47, 55, 59, 62, 65]));
        assert!(chord("IV", None).unwrap_err().contains("needs a key"));
        assert!(chord("Iv", Some(&dorian)).unwrap_err().contains("mixes cases"));
        assert_eq!(Key::parse("A harmonic minor").map(|key| key.steps[6]), Some(11));
        assert_eq!(Key::parse("C bebop"), None);
    }
}
