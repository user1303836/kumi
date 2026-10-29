//! Pitches by Live's names (C3 is 60, so MIDI 0 is C-2), and drums by name: the track's Drum Rack pads first, then
//! General MIDI's drums.

const NAMES: [&str; 12] = ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"];
/// General MIDI's drums, by the names a producer writes (pads start at C1, 36, as Live's Drum Racks do).
const GENERAL_MIDI: &[(&str, u8)] = &[
    ("kick", 36),
    ("bd", 36),
    ("bassdrum", 36),
    ("kick2", 35),
    ("rim", 37),
    ("sidestick", 37),
    ("snare", 38),
    ("sd", 38),
    ("clap", 39),
    ("cp", 39),
    ("snare2", 40),
    ("floortom", 41),
    ("hat", 42),
    ("hihat", 42),
    ("closedhat", 42),
    ("chh", 42),
    ("ch", 42),
    ("hifloortom", 43),
    ("pedalhat", 44),
    ("lowtom", 45),
    ("tom", 45),
    ("openhat", 46),
    ("ohat", 46),
    ("ohh", 46),
    ("oh", 46),
    ("midtom", 47),
    ("himidtom", 48),
    ("crash", 49),
    ("hitom", 50),
    ("hightom", 50),
    ("ride", 51),
    ("china", 52),
    ("ridebell", 53),
    ("bell", 53),
    ("tambourine", 54),
    ("tamb", 54),
    ("splash", 55),
    ("cowbell", 56),
    ("crash2", 57),
    ("vibraslap", 58),
    ("ride2", 59),
    ("hibongo", 60),
    ("lowbongo", 61),
    ("muteconga", 62),
    ("conga", 63),
    ("lowconga", 64),
    ("hitimbale", 65),
    ("lowtimbale", 66),
    ("agogo", 67),
    ("cabasa", 69),
    ("maracas", 70),
    ("claves", 75),
    ("woodblock", 76),
    ("lowwoodblock", 77),
    ("cuica", 79),
    ("triangle", 81),
    ("shaker", 82),
];

/// Live's name for a pitch: C3 is 60.
pub fn name(pitch: u8) -> String {
    format!("{}{}", NAMES[pitch as usize % 12], pitch as i32 / 12 - 2)
}
/// A pitch class from its letter and accidentals (`C`, `F#`, `Bb`, `Ebb`), not wrapped (`Cb` is -1, a B below), and
/// the text after them.
pub(super) fn class(text: &str) -> Option<(i32, &str)> {
    let letter = text.chars().next().filter(char::is_ascii_uppercase)?;
    let mut class = match letter {
        'C' => 0,
        'D' => 2,
        'E' => 4,
        'F' => 5,
        'G' => 7,
        'A' => 9,
        'B' => 11,
        _ => return None,
    };
    let mut rest = &text[1..];
    loop {
        if let Some(after) = rest.strip_prefix('#').or_else(|| rest.strip_prefix('♯')) {
            class += 1;
            rest = after;
        } else if let Some(after) = rest.strip_prefix('b').or_else(|| rest.strip_prefix('♭')) {
            class -= 1;
            rest = after;
        } else {
            return Some((class, rest));
        }
    }
}
/// A pitch from Live's name (`C3`, `F#2`, `Bb-1`) or a MIDI number (0–127).
pub fn parse(text: &str) -> Option<u8> {
    if text.bytes().all(|b| b.is_ascii_digit()) {
        return text.parse::<u8>().ok().filter(|pitch| *pitch <= 127);
    }
    let (class, octave) = class(text)?;
    let octave: i32 = octave.parse().ok()?;
    // Checked: an octave in the millions would wrap round into the MIDI range.
    let pitch = octave.checked_add(2)?.checked_mul(12)?.checked_add(class)?;
    (0..=127).contains(&pitch).then_some(pitch as u8)
}
/// A name as lanes match it: lowercase letters and digits only ("Kick 808" is "kick808").
pub(super) fn normalized(name: &str) -> String {
    name.chars().filter(char::is_ascii_alphanumeric).map(|c| c.to_ascii_lowercase()).collect()
}
/// The drums a kit's lanes name by General MIDI's names, which a pad's name can say it holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Drum {
    Kick,
    Rim,
    Snare,
    Clap,
    ClosedHat,
    PedalHat,
    OpenHat,
    Tom,
    Crash,
    Ride,
}

impl Drum {
    /// The drum on General MIDI's key.
    fn at(pitch: u8) -> Option<Drum> {
        Some(match pitch {
            35 | 36 => Drum::Kick,
            37 => Drum::Rim,
            38 | 40 => Drum::Snare,
            39 => Drum::Clap,
            42 => Drum::ClosedHat,
            44 => Drum::PedalHat,
            46 => Drum::OpenHat,
            41 | 43 | 45 | 47 | 48 | 50 => Drum::Tom,
            49 | 52 | 55 | 57 => Drum::Crash,
            51 | 53 | 59 => Drum::Ride,
            _ => return None,
        })
    }
    fn words(self) -> &'static str {
        match self {
            Drum::Kick => "kick",
            Drum::Rim => "rim",
            Drum::Snare => "snare",
            Drum::Clap => "clap",
            Drum::ClosedHat => "closed hat",
            Drum::PedalHat => "pedal hat",
            Drum::OpenHat => "open hat",
            Drum::Tom => "tom",
            Drum::Crash => "crash",
            Drum::Ride => "ride",
        }
    }
    /// What a pad holds, by its name's words ("Kick 909", "909hh", "HH Open", "SD Tight", "Claps"): none when it doesn't
    /// say. Words part at spaces and signs, and where letters meet digits ("909hh" is 909 and hh).
    fn held_by(name: &str) -> Option<Drum> {
        let lower = name.to_lowercase();
        let mut words: Vec<String> = vec![];
        for part in lower.split(|c: char| !c.is_alphanumeric()).filter(|part| !part.is_empty()) {
            let mut word = String::new();
            for c in part.chars() {
                if word.chars().last().is_some_and(|last| last.is_ascii_digit() != c.is_ascii_digit()) {
                    words.push(std::mem::take(&mut word));
                }
                word.push(c);
            }
            words.push(word);
        }
        // Kick, snare, clap, crash, bassdrum and sidestick say so wherever they're written ("Kickdrum", "Claps"); the
        // shorter names only as words of their own ("Grime Snare" holds no rim, "Phantom" no tom, "Chatter" no hat).
        let says = |parts: &[&str], whole: &[&str]| {
            parts.iter().any(|part| lower.contains(part)) || words.iter().any(|word| whole.contains(&word.as_str()))
        };
        const CLOSED: &[&str] = &["hat", "hats", "hihat", "hihats", "hh", "chh", "ch", "closed", "closedhat"];
        const OPEN: &[&str] = &["open", "openhat", "openhats", "ohh", "oh", "ohat"];
        const PEDAL: &[&str] = &["pedal", "pedalhat", "ph"];
        Some(if says(&["kick", "bassdrum", "bass drum"], &["bd", "kik"]) {
            Drum::Kick
        } else if says(&["clap"], &["cp", "clp"]) {
            Drum::Clap
        } else if says(&["sidestick", "side stick"], &["rim", "rims", "rimshot", "rimshots"]) {
            Drum::Rim
        } else if says(&["snare"], &["sd", "snr"]) {
            Drum::Snare
        } else if says(&[], PEDAL) {
            Drum::PedalHat
        } else if says(&[], OPEN) {
            Drum::OpenHat
        } else if says(&[], CLOSED) {
            Drum::ClosedHat
        } else if says(&["crash"], &["china", "splash"]) {
            Drum::Crash
        } else if says(&[], &["ride", "rides"]) {
            Drum::Ride
        } else if says(&[], &["tom", "toms", "lowtom", "midtom", "hitom", "hightom", "floortom"]) {
            Drum::Tom
        } else {
            return None;
        })
    }
}

/// The pitch a lane name plays: the track's pad by that name (or the one pad it begins); for General MIDI's name of a
/// drum, the one pad whose name says it holds that drum, wherever the kit has it; else General MIDI's key, unless a pad
/// there holds another drum.
pub fn drum_pitch(lane: &str, pads: &[(String, u8)]) -> Result<u8, String> {
    let wanted = normalized(lane);
    if let Some((_, pitch)) = pads.iter().find(|(name, _)| normalized(name) == wanted) {
        return Ok(*pitch);
    }
    let begun: Vec<_> = pads.iter().filter(|(name, _)| !wanted.is_empty() && normalized(name).starts_with(&wanted)).collect();
    match begun[..] {
        [(_, pitch)] => return Ok(*pitch),
        [_, _, ..] => {
            let named: Vec<&str> = begun.iter().map(|(name, _)| name.as_str()).collect();
            return Err(format!("more than one pad begins with “{lane}”: name one of them ({}) or a pitch (C1 or 36)", named.join(", ")));
        }
        [] => {}
    }
    let named: Vec<&str> = pads.iter().map(|(name, _)| name.as_str()).take(16).collect();
    if let Some((_, key)) = GENERAL_MIDI.iter().find(|(name, _)| *name == wanted) {
        if let Some(drum) = Drum::at(*key) {
            let holding: Vec<&(String, u8)> = pads.iter().filter(|(name, _)| Drum::held_by(name) == Some(drum)).collect();
            match holding[..] {
                [(_, pitch)] => return Ok(*pitch),
                // (several: the one on General MIDI's key, as a kit laid out on them has it)
                [_, _, ..] => {
                    return holding.iter().find(|(_, pitch)| pitch == key).map(|(_, pitch)| *pitch).ok_or_else(|| {
                        let them: Vec<&str> = holding.iter().map(|(name, _)| name.as_str()).collect();
                        format!("more than one pad holds a {}: name one of them ({}) or a pitch (C1 or 36)", drum.words(), them.join(", "))
                    })
                }
                [] => {}
            }
            if let Some((other, _)) = pads.iter().find(|(name, at)| at == key && Drum::held_by(name).is_some_and(|held| held != drum)) {
                return Err(format!(
                    "no pad holds a {}, and “{other}” is on General MIDI's {}: name one of the pads ({}) or a pitch (C1 or 36)",
                    drum.words(),
                    self::name(*key),
                    named.join(", ")
                ));
            }
        }
        return Ok(*key);
    }
    Err(if named.is_empty() {
        format!("there's no drum called “{lane}”: name a pitch (C1 or 36) or a General MIDI drum (kick, snare, clap, hat, openhat…)")
    } else {
        format!("there's no pad or drum called “{lane}”: name a pitch (C1 or 36) or one of the pads ({})", named.join(", "))
    })
}
/// The name a print gives a drum lane: its pad's (when that name reads back to it), else General MIDI's, else the pitch.
pub(super) fn drum_name(pitch: u8, pads: &[(String, u8)]) -> String {
    for (name, at) in pads.iter().filter(|(_, at)| *at == pitch) {
        let short = normalized(name);
        if !short.is_empty() && !short.bytes().all(|b| b.is_ascii_digit()) && parse(&short).is_none() && drum_pitch(&short, pads) == Ok(*at)
        {
            return short;
        }
    }
    match GENERAL_MIDI.iter().find(|(_, at)| *at == pitch) {
        Some((name, _)) if pads.is_empty() || drum_pitch(name, pads) == Ok(pitch) => name.to_string(),
        _ => self::name(pitch),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pitches_go_by_lives_names() {
        for (text, pitch) in [("C3", 60), ("C-2", 0), ("G8", 127), ("F#2", 54), ("Bb1", 46), ("Cb3", 59), ("B#2", 60), ("36", 36)] {
            assert_eq!(parse(text), Some(pitch), "{text}");
        }
        assert_eq!((name(60), name(0), name(54)), ("C3".into(), "C-2".into(), "F#2".into()));
        for wrong in ["H3", "c3", "C9", "128", "C", "x"] {
            assert_eq!(parse(wrong), None, "{wrong}");
        }
        // An octave so high its arithmetic overflows isn't a pitch (wrapped round, "C357913942" was 32).
        for wrong in ["C357913942", "C2147483646", "C-2147483648", "Bb178956970"] {
            assert_eq!(parse(wrong), None, "{wrong}");
        }
    }

    #[test]
    fn drums_go_by_the_tracks_pads_then_general_midi() {
        let pads = vec![("Kick 808".to_owned(), 36), ("Snare Tight".to_owned(), 38), ("Snare Room".to_owned(), 40)];
        assert_eq!(drum_pitch("kick", &pads), Ok(36), "the one pad it begins");
        assert_eq!(drum_pitch("snaretight", &pads), Ok(38));
        assert!(drum_pitch("snare", &pads).unwrap_err().contains("Snare Tight, Snare Room"), "two pads begin with it");
        assert_eq!((drum_pitch("clap", &pads), drum_pitch("Open Hat", &[])), (Ok(39), Ok(46)), "General MIDI's");
        assert!(drum_pitch("zap", &[]).is_err());
        assert_eq!((drum_name(36, &pads), drum_name(42, &[]), drum_name(90, &[])), ("kick808".into(), "hat".into(), "F#5".into()));
    }

    #[test]
    fn a_pads_name_says_what_it_holds_in_words_of_its_own() {
        use Drum::*;
        for (name, drum) in [
            ("Kick 909", Some(Kick)),
            ("Kickdrum", Some(Kick)),
            ("BD Deep", Some(Kick)),
            ("Claps", Some(Clap)),
            ("Rimshot", Some(Rim)),
            ("Side Stick", Some(Rim)),
            ("SD Tight", Some(Snare)),
            ("909hh", Some(ClosedHat)),
            ("Hi-Hat", Some(ClosedHat)),
            ("Hihats", Some(ClosedHat)),
            ("VEH3 Closed", Some(ClosedHat)),
            ("HH Open", Some(OpenHat)),
            ("OpenHat", Some(OpenHat)),
            ("Pedal Hat", Some(PedalHat)),
            ("Toms", Some(Tom)),
            ("LowTom", Some(Tom)),
            ("Crash 2", Some(Crash)),
            ("China", Some(Crash)),
            ("Ride Bell", Some(Ride)),
            // A name inside another word says nothing of it.
            ("Grime Snare", Some(Snare)),
            ("Opening Crash", Some(Crash)),
            ("Custom Perc", None),
            ("Phantom", None),
            ("Bottom", None),
            ("Override", None),
            ("Chatter", None),
        ] {
            assert_eq!(Drum::held_by(name), drum, "{name}");
        }
        // So a snare lane plays "Grime Snare", and a rim lane says there's no rim rather than play it.
        let pads = vec![("Grime Snare".to_owned(), 37)];
        assert_eq!(drum_pitch("snare", &pads), Ok(37));
        assert!(drum_pitch("rim", &pads).unwrap_err().contains("no pad holds a rim"));
    }

    #[test]
    fn a_general_midi_drum_finds_the_pad_that_holds_one_wherever_the_kit_has_it() {
        let pads = |kit: &[(&str, u8)]| kit.iter().map(|(name, pitch)| (name.to_string(), *pitch)).collect::<Vec<_>>();
        // A kit laid out in order, as Kumi makes one: four sounds on C1 to D#1.
        let kit = pads(&[("Kick 909", 36), ("Claps", 37), ("909hh", 38), ("VEH3 Open", 39)]);
        for (lane, pitch) in [("kick", 36), ("clap", 37), ("hat", 38), ("hihat", 38), ("openhat", 39), ("oh", 39)] {
            assert_eq!(drum_pitch(lane, &kit), Ok(pitch), "{lane}");
        }
        // No pad holds a snare, and General MIDI's key (D1) is the hat's: said, not played on the hat.
        let snare = drum_pitch("snare", &kit).unwrap_err();
        assert!(snare.contains("no pad holds a snare") && snare.contains("909hh"), "{snare}");
        // General MIDI's key where no pad on it holds another drum: a crash past the kit.
        assert_eq!(drum_pitch("crash", &kit), Ok(49));
        // Two closed hats: the one on General MIDI's key; with neither there, Kumi asks which.
        assert_eq!(drum_pitch("hat", &pads(&[("HH Tight", 37), ("VEH3 Closed", 42)])), Ok(42));
        let two = drum_pitch("hat", &pads(&[("909hh", 38), ("VEH3 Closed", 39)])).unwrap_err();
        assert!(two.contains("more than one pad holds a closed hat") && two.contains("909hh, VEH3 Closed"), "{two}");
        // A kit on General MIDI's keys plays as before.
        let gm = pads(&[("Kick", 36), ("Snare", 38), ("Clap", 39), ("Hihat Closed", 42), ("Hihat Open", 46)]);
        for (lane, pitch) in [("kick", 36), ("snare", 38), ("clap", 39), ("hat", 42), ("openhat", 46), ("ride", 51)] {
            assert_eq!(drum_pitch(lane, &gm), Ok(pitch), "{lane}");
        }
    }
}
