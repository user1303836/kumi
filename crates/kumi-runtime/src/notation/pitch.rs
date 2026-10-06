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
    let pitch = (octave + 2) * 12 + class;
    (0..=127).contains(&pitch).then_some(pitch as u8)
}
/// A name as lanes match it: lowercase letters and digits only ("Kick 808" is "kick808").
pub(super) fn normalized(name: &str) -> String {
    name.chars().filter(char::is_ascii_alphanumeric).map(|c| c.to_ascii_lowercase()).collect()
}
/// The pitch a lane name plays: the track's pad by that name (or the one pad it begins), else General MIDI's drum.
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
    if let Some((_, pitch)) = GENERAL_MIDI.iter().find(|(name, _)| *name == wanted) {
        return Ok(*pitch);
    }
    let named: Vec<&str> = pads.iter().map(|(name, _)| name.as_str()).take(16).collect();
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
}
