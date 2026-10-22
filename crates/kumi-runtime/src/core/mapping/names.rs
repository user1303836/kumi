//! What a sample's file name says about where it plays: its note (as the name writes it, the octave convention still open),
//! its velocity (a layer's number, a velocity value or a dynamic), its round robin, its articulation and, for drums, its role.
//! Sample libraries disagree on all of these ("Piano_A#2_v3", "Vel 64", "C-4", "060", "Strings_sus_C3_rr2"), so each is read
//! in the common spellings and none is guessed from a bare number that could be anything else.
use regex::Regex;
use std::sync::LazyLock;

/// A note as a name writes it: its pitch class (C is 0) and the octave written, or a MIDI number written as one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Written {
    /// "C3", "A#2", "Db4", "C-4" (a tracker's natural): the octave as written, whose convention a set settles.
    Named { class: u8, octave: i32 },
    /// "060", "midi60", "note_60": a MIDI number, C4 = 60.
    Midi(u8),
}

/// A velocity as a name writes it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Velocity {
    /// "v3", "vl3", "layer3": a layer's number (1 the softest).
    Layer(u32),
    /// "vel64", "v064", "Vel 64", "velocity100": a MIDI velocity.
    Value(u8),
    /// "pp" to "fff", "soft", "hard": a dynamic, softest first (ppp 0 … fff 7).
    Dynamic(u8),
}

/// Everything a name says.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Hints {
    pub note: Option<Written>,
    pub velocity: Option<Velocity>,
    pub round_robin: Option<u32>,
    pub articulation: Option<&'static str>,
    /// A release sample: what a key plays when it lets go.
    pub release: bool,
    /// A drum's role ("kick", "snare", "closed hat"…) and its GM note.
    pub drum: Option<(&'static str, u8)>,
}

const DYNAMICS: [(&str, u8); 12] = [
    ("ppp", 0),
    ("pp", 1),
    ("p", 2),
    ("mp", 3),
    ("mf", 4),
    ("f", 5),
    ("ff", 6),
    ("fff", 7),
    ("soft", 1),
    ("medium", 4),
    ("med", 4),
    ("hard", 6),
];

const ARTICULATIONS: [(&str, &str); 22] = [
    ("sus", "sustain"),
    ("sustain", "sustain"),
    ("long", "sustain"),
    ("stac", "staccato"),
    ("staccato", "staccato"),
    ("stc", "staccato"),
    ("short", "staccato"),
    ("spic", "spiccato"),
    ("spiccato", "spiccato"),
    ("leg", "legato"),
    ("legato", "legato"),
    ("pizz", "pizzicato"),
    ("pizzicato", "pizzicato"),
    ("trem", "tremolo"),
    ("tremolo", "tremolo"),
    ("harm", "harmonics"),
    ("harmonics", "harmonics"),
    ("mute", "muted"),
    ("muted", "muted"),
    ("palm", "muted"),
    ("marc", "marcato"),
    ("marcato", "marcato"),
];

/// Drum roles by the words names use for them, with the GM note each plays on.
const DRUMS: [(&str, &str, u8); 27] = [
    ("kick", "kick", 36),
    ("bd", "kick", 36),
    ("bassdrum", "kick", 36),
    ("snare", "snare", 38),
    ("sd", "snare", 38),
    ("snr", "snare", 38),
    ("rim", "rim", 37),
    ("rimshot", "rim", 37),
    ("clap", "clap", 39),
    ("cp", "clap", 39),
    ("chh", "closed hat", 42),
    ("ch", "closed hat", 42),
    ("ohh", "open hat", 46),
    ("oh", "open hat", 46),
    ("phh", "pedal hat", 44),
    ("hat", "closed hat", 42),
    ("hh", "closed hat", 42),
    ("hihat", "closed hat", 42),
    ("tom", "tom", 45),
    ("crash", "crash", 49),
    ("ride", "ride", 51),
    ("cymbal", "crash", 49),
    ("perc", "percussion", 39),
    ("shaker", "shaker", 70),
    ("cowbell", "cowbell", 56),
    ("conga", "conga", 63),
    ("tamb", "tambourine", 54),
];

/// The words of a name: split at separators, and where a lower-case letter meets a capital or a letter meets a digit run
/// that isn't a note ("PianoC3" is "Piano", "C3"; "Kick02" is "Kick", "02").
pub fn words(stem: &str) -> Vec<String> {
    static SPLIT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[\s_.,()\[\]{}+=&]+").unwrap());
    let mut out = Vec::new();
    for part in SPLIT.split(stem) {
        // A tracker note ("C-4", "F#3") keeps its dash; any other dash separates.
        for piece in split_dashes(part) {
            out.extend(split_case(&piece));
        }
    }
    out.into_iter().filter(|word| !word.is_empty()).collect()
}

fn split_dashes(part: &str) -> Vec<String> {
    static TRACKER: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)^[a-g]-[0-9]$").unwrap());
    if TRACKER.is_match(part) {
        return vec![part.to_string()];
    }
    let mut out = Vec::new();
    let chunks: Vec<&str> = part.split('-').collect();
    let mut k = 0;
    while k < chunks.len() {
        // ("Piano-C-4-v2": the dash inside the tracker note stays)
        if k + 1 < chunks.len() && TRACKER.is_match(&format!("{}-{}", chunks[k], chunks[k + 1])) {
            out.push(format!("{}-{}", chunks[k], chunks[k + 1]));
            k += 2;
        } else {
            out.push(chunks[k].to_string());
            k += 1;
        }
    }
    out
}

/// "PianoC3" → "Piano", "C3"; "Kick02" → "Kick", "02"; "A#2" and "v3" stay whole.
fn split_case(word: &str) -> Vec<String> {
    static ATTACHED_NOTE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^(.*[a-z])([A-G](?:#|b)?-?[0-9])$").unwrap());
    static ATTACHED_NUMBER: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^([A-Za-z]{3,})([0-9]+)$").unwrap());
    if let Some(caps) = ATTACHED_NOTE.captures(word) {
        return vec![caps[1].to_string(), caps[2].to_string()];
    }
    if let Some(caps) = ATTACHED_NUMBER.captures(word) {
        let head = caps[1].to_ascii_lowercase();
        // ("vel64", "velocity100", "midi60", "note60", "layer3" are read whole below)
        if !["vel", "velo", "velocity", "midi", "note", "layer", "rr", "alt", "roundrobin"].contains(&head.as_str()) {
            return vec![caps[1].to_string(), caps[2].to_string()];
        }
    }
    vec![word.to_string()]
}

fn class_of(letter: char, accidental: &str) -> Option<u8> {
    let natural: i32 = match letter.to_ascii_uppercase() {
        'C' => 0,
        'D' => 2,
        'E' => 4,
        'F' => 5,
        'G' => 7,
        'A' => 9,
        'B' => 11,
        _ => return None,
    };
    let moved = match accidental {
        "#" | "s" | "\u{266f}" => 1,
        "b" | "\u{266d}" => -1,
        _ => 0,
    };
    Some((natural + moved).rem_euclid(12) as u8)
}

/// A word that is a note: "C3", "a#2", "Db4", "Cs3", "C-4", "C-1" read as a tracker's natural C1 (sample sets reach MIDI 0
/// rarely, trackers write every natural with a dash).
fn note_word(word: &str) -> Option<Written> {
    static NOTE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^([A-Ga-g])(#|b|s|\u{266f}|\u{266d}|-)?(-?[0-9])$").unwrap());
    let caps = NOTE.captures(word)?;
    let letter = caps[1].chars().next()?;
    // ("b3" is the note B3, "bb3" B flat 3, "cs3" C sharp 3)
    let accidental = caps.get(2).map_or("", |m| m.as_str());
    let octave: i32 = caps[3].parse().ok()?;
    Some(Written::Named { class: class_of(letter, accidental)?, octave })
}

fn number_word(word: &str) -> Option<u32> {
    (!word.is_empty() && word.len() <= 3 && word.bytes().all(|b| b.is_ascii_digit())).then(|| word.parse().ok()).flatten()
}

/// A word that tells a velocity or a round robin ("v3", "vel64", "rr2", "ff", or "vel"/"rr" before a number), not the sound.
pub fn is_tag(word: &str) -> bool {
    static TAG: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?i)^(v|vl|vel|velo|velocity|layer|rr|alt|roundrobin)[0-9]{0,3}$|^(ppp|pp|mp|mf|ff|fff|soft|medium|med|hard)$")
            .unwrap()
    });
    TAG.is_match(word)
}

/// What a file name (its stem, no folder or extension) says.
pub fn hints(stem: &str) -> Hints {
    static PREFIXED: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?i)^(vel|velo|velocity|v|vl|layer|rr|alt|roundrobin|midi|note|nn)([0-9]{1,3})$").unwrap());
    let words = words(stem);
    let mut hints = Hints::default();
    let mut k = 0;
    while k < words.len() {
        let word = &words[k];
        let lower = word.to_ascii_lowercase();
        let next_number = words.get(k + 1).and_then(|next| number_word(next));
        if let Some(caps) = PREFIXED.captures(word) {
            let n: u32 = caps[2].parse().unwrap_or(0);
            let digits = caps[2].len();
            match caps[1].to_ascii_lowercase().as_str() {
                "vel" | "velo" | "velocity" => hints.velocity = (n <= 127).then_some(Velocity::Value(n as u8)).or(hints.velocity),
                // ("v064" is a velocity; "v3" a layer)
                "v" | "vl" | "layer" if digits == 3 || n > 16 => {
                    hints.velocity = (n <= 127).then_some(Velocity::Value(n as u8)).or(hints.velocity)
                }
                "v" | "vl" | "layer" => hints.velocity = Some(Velocity::Layer(n)),
                "rr" | "alt" | "roundrobin" => hints.round_robin = Some(n),
                _ if n <= 127 => hints.note = Some(Written::Midi(n as u8)),
                _ => {}
            }
        } else if let Some(n) = next_number.filter(|_| matches!(lower.as_str(), "vel" | "velocity" | "rr" | "midi" | "note" | "layer")) {
            // ("Vel 64", "rr 2", "note 60": the number is the next word)
            match lower.as_str() {
                "vel" | "velocity" if n <= 127 => hints.velocity = Some(Velocity::Value(n as u8)),
                "layer" => hints.velocity = Some(Velocity::Layer(n)),
                "rr" => hints.round_robin = Some(n),
                "midi" | "note" if n <= 127 => hints.note = Some(Written::Midi(n as u8)),
                _ => {}
            }
            k += 1;
        } else if let Some(note) = note_word(word) {
            hints.note = Some(note);
        } else if word.len() == 3 && word.starts_with('0') && number_word(word).is_some_and(|n| n <= 127) {
            // ("060": a zero-padded MIDI number; a bare "60" or "2" could be a take, a layer or a round robin)
            hints.note = hints.note.or(Some(Written::Midi(number_word(word).unwrap_or(0) as u8)));
        } else if let Some((_, level)) = DYNAMICS.iter().find(|(name, _)| *name == lower) {
            // ("p" and "f" alone count only where the set's other names use dynamics too: see `dynamics_alone`)
            hints.velocity = hints.velocity.or(Some(Velocity::Dynamic(*level)));
        } else if lower == "rel" || lower == "release" {
            hints.release = true;
        } else if let Some((_, articulation)) = ARTICULATIONS.iter().find(|(name, _)| *name == lower) {
            hints.articulation = Some(articulation);
        } else if let Some((_, role, note)) = DRUMS.iter().find(|(name, _, _)| *name == lower) {
            hints.drum = Some((role, *note));
        } else if lower.contains("hat") {
            hints.drum = Some(if lower.contains("open") { ("open hat", 46) } else { ("closed hat", 42) });
        }
        k += 1;
    }
    // "Open" before "hat" ("Open_Hat_01") makes it an open hat.
    if hints.drum.is_some_and(|(role, _)| role == "closed hat") && words.iter().any(|word| word.eq_ignore_ascii_case("open")) {
        hints.drum = Some(("open hat", 46));
    }
    hints
}

/// A single-letter dynamic ("p", "f") counts only when other names in the set use dynamics: alone it's more often an initial.
pub fn dynamics_alone(stems: &[&str], hints: &mut [Hints]) {
    let longer = hints.iter().any(|hint| matches!(hint.velocity, Some(Velocity::Dynamic(level)) if ![2, 5].contains(&level)));
    if longer {
        return;
    }
    for (stem, hint) in stems.iter().zip(hints.iter_mut()) {
        if matches!(hint.velocity, Some(Velocity::Dynamic(_))) {
            let single = words(stem).iter().any(|word| word.eq_ignore_ascii_case("p") || word.eq_ignore_ascii_case("f"));
            if single {
                hint.velocity = None;
            }
        }
    }
}

/// The MIDI note a written note is, under a convention: where middle C (MIDI 60) is "C4" (`c4_is_60`) or "C3".
pub fn midi_of(written: Written, c4_is_60: bool) -> Option<i32> {
    match written {
        Written::Midi(n) => Some(n as i32),
        Written::Named { class, octave } => {
            let midi = 12 * (octave + if c4_is_60 { 1 } else { 2 }) + class as i32;
            (0..=127).contains(&midi).then_some(midi)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn named(class: u8, octave: i32) -> Option<Written> {
        Some(Written::Named { class, octave })
    }

    #[test]
    fn notes_are_read_in_the_spellings_libraries_use() {
        assert_eq!(hints("Piano_A#2_v3").note, named(10, 2));
        assert_eq!(hints("Piano_A#2_v3").velocity, Some(Velocity::Layer(3)));
        assert_eq!(hints("Strings Db4 ff").note, named(1, 4));
        assert_eq!(hints("Strings Db4 ff").velocity, Some(Velocity::Dynamic(6)));
        assert_eq!(hints("Bass-C-4").note, named(0, 4), "a tracker's natural keeps its dash");
        assert_eq!(hints("C-4").note, named(0, 4));
        assert_eq!(hints("Rhodes_F#3_rr2").note, named(6, 3));
        assert_eq!(hints("Rhodes_F#3_rr2").round_robin, Some(2));
        assert_eq!(hints("VibeC3").note, named(0, 3), "a capital note after a word");
        assert_eq!(hints("Piano_060_vel100").note, Some(Written::Midi(60)));
        assert_eq!(hints("Piano_060_vel100").velocity, Some(Velocity::Value(100)));
        assert_eq!(hints("Piano Vel 64 C2").velocity, Some(Velocity::Value(64)));
        assert_eq!(hints("Piano_v064_C2").velocity, Some(Velocity::Value(64)));
        assert_eq!(hints("note_72").note, Some(Written::Midi(72)));
        assert_eq!(hints("Cello_sus_G2").articulation, Some("sustain"));
        assert!(hints("Cello_G2_rel").release);
    }

    #[test]
    fn bare_numbers_and_drum_words_are_never_notes() {
        assert_eq!(hints("Kick02").note, None);
        assert_eq!(hints("Snare1").note, None, "e1 inside a word is no E1");
        assert_eq!(hints("Take 60").note, None, "a bare 60 could be anything");
        assert_eq!(hints("Kick02").drum, Some(("kick", 36)));
        assert_eq!(hints("Open_Hat_01").drum, Some(("open hat", 46)));
        assert_eq!(hints("ClosedHat").drum, Some(("closed hat", 42)));
        assert_eq!(hints("808 SD 3").drum, Some(("snare", 38)));
    }

    #[test]
    fn a_single_letter_dynamic_counts_only_beside_others() {
        let stems = ["Piano C3 p", "Piano C3 f"];
        let mut alone: Vec<Hints> = stems.iter().map(|stem| hints(stem)).collect();
        dynamics_alone(&stems, &mut alone);
        assert!(alone.iter().all(|hint| hint.velocity.is_none()), "p and f alone are initials");
        let stems = ["Piano C3 p", "Piano C3 mf", "Piano C3 f"];
        let mut together: Vec<Hints> = stems.iter().map(|stem| hints(stem)).collect();
        dynamics_alone(&stems, &mut together);
        assert_eq!(together[0].velocity, Some(Velocity::Dynamic(2)));
    }

    #[test]
    fn a_convention_places_middle_c() {
        assert_eq!(midi_of(Written::Named { class: 0, octave: 4 }, true), Some(60));
        assert_eq!(midi_of(Written::Named { class: 0, octave: 3 }, false), Some(60));
        assert_eq!(midi_of(Written::Midi(60), false), Some(60));
        assert_eq!(midi_of(Written::Named { class: 0, octave: 10 }, true), None);
    }
}
