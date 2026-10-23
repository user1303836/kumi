//! Fixed one-liners Kumi runs with no model call ("tempo 124", "mute drums", "loop 9-16"), in words any DAW
//! understands. Only a whole message that matches is one; anything else, or a track that isn't there or that two
//! fit, goes to the model as it was written. Each integration turns an intent into its own change (Live's in
//! integrations/ableton/instant.rs), which lands in HISTORY with its undo.

use std::sync::LazyLock;

use regex::Regex;

/// What a phrase asks for.
#[derive(Debug, Clone, PartialEq)]
pub enum Intent {
    Tempo {
        bpm: f64,
    },
    Track {
        track: TrackPick,
        switch: Switch,
        on: bool,
    },
    /// Bars counted from 1, both included.
    Loop {
        from_bar: u32,
        to_bar: u32,
    },
    LoopOff,
    Metronome {
        on: bool,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Switch {
    Mute,
    Solo,
    Arm,
}

/// The track a phrase names: by its name, or by its place among the Set's tracks ("track 3").
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TrackPick {
    Name(String),
    Position(usize),
}

/// The tempo a DAW takes, in BPM.
const TEMPO: std::ops::RangeInclusive<f64> = 20.0..=999.0;
/// A track name a phrase may give: past this, it's a sentence.
const NAME_WORDS: usize = 5;

static TEMPO_FIRST: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^(?:tempo|bpm)\s+(\d{2,3}(?:\.\d{1,2})?)(?:\s*bpm)?$").expect("regex"));
static TEMPO_LAST: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^(\d{2,3}(?:\.\d{1,2})?)\s*bpm$").expect("regex"));
static SWITCH: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^(mute|unmute|solo|unsolo|arm|disarm)\s+(.+)$").expect("regex"));
static POSITION: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^track\s+(\d{1,3})$").expect("regex"));
static LOOP: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^loop\s+(?:bars?\s+)?(\d{1,4})\s*(?:-|–|—|to|through)\s*(\d{1,4})$").expect("regex"));
static METRONOME: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^metronome\s+(on|off)$").expect("regex"));
/// Words that make a request more than a switch: "and", "then", "a bit", "please"…
static MORE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\b(and|then|but|also|except|bit|little|slightly|more|less|too|please|while|until|after|before|when|if|all|every|other|others|again)\b")
        .expect("regex")
});

/// The intent of a whole message that is a fixed phrase, or None (the model takes it).
pub fn parse(text: &str) -> Option<Intent> {
    let line = text.trim();
    // One line, no question, nothing to say beside it.
    if line.is_empty() || line.len() > 80 || line.contains(['\n', '?', '!', ',', ';', ':', '"', '(', ')']) {
        return None;
    }
    let line = line.strip_suffix('.').unwrap_or(line).trim().to_lowercase();
    let line = line.split_whitespace().collect::<Vec<_>>().join(" ");
    if let Some(found) = TEMPO_FIRST.captures(&line).or_else(|| TEMPO_LAST.captures(&line)) {
        let bpm: f64 = found[1].parse().ok()?;
        return TEMPO.contains(&bpm).then_some(Intent::Tempo { bpm });
    }
    if let Some(found) = LOOP.captures(&line) {
        let (from_bar, to_bar): (u32, u32) = (found[1].parse().ok()?, found[2].parse().ok()?);
        return (from_bar >= 1 && to_bar >= from_bar).then_some(Intent::Loop { from_bar, to_bar });
    }
    if line == "loop off" {
        return Some(Intent::LoopOff);
    }
    if let Some(found) = METRONOME.captures(&line) {
        return Some(Intent::Metronome { on: &found[1] == "on" });
    }
    if let Some(found) = SWITCH.captures(&line) {
        let (switch, on) = match &found[1] {
            "mute" => (Switch::Mute, true),
            "unmute" => (Switch::Mute, false),
            "solo" => (Switch::Solo, true),
            "unsolo" => (Switch::Solo, false),
            "arm" => (Switch::Arm, true),
            _ => (Switch::Arm, false),
        };
        let rest = found[2].trim();
        let track = match POSITION.captures(rest) {
            Some(position) => TrackPick::Position(position[1].parse().ok().filter(|n| *n >= 1)?),
            None => {
                if rest.split_whitespace().count() > NAME_WORDS || MORE.is_match(rest) {
                    return None;
                }
                TrackPick::Name(rest.to_string())
            }
        };
        return Some(Intent::Track { track, switch, on });
    }
    None
}

/// Words a track name is compared by: lower case, single spaces.
fn normalize(name: &str) -> String {
    name.split_whitespace().collect::<Vec<_>>().join(" ").to_lowercase()
}

/// Which of the Set's tracks (their names, in order) a pick means: the one track whose name is the pick's (case
/// aside), else the one whose name starts with it, or the track at a position (from 1). None when none or several
/// fit: the model asks or decides.
pub fn resolve(pick: &TrackPick, names: &[&str]) -> Option<usize> {
    match pick {
        TrackPick::Position(position) => (*position >= 1 && *position <= names.len()).then(|| position - 1),
        TrackPick::Name(name) => {
            let wanted = normalize(name);
            let unique = |matches: Vec<usize>| if matches.len() == 1 { Some(matches[0]) } else { None };
            let tries = [wanted.clone(), wanted.strip_prefix("the ").unwrap_or(&wanted).to_string()];
            for wanted in tries.iter().filter(|wanted| !wanted.is_empty()) {
                let exact: Vec<usize> = names.iter().enumerate().filter(|(_, name)| normalize(name) == *wanted).map(|(at, _)| at).collect();
                if !exact.is_empty() {
                    return unique(exact);
                }
            }
            for wanted in tries.iter().filter(|wanted| wanted.len() >= 2) {
                let starts: Vec<usize> =
                    names.iter().enumerate().filter(|(_, name)| normalize(name).starts_with(wanted.as_str())).map(|(at, _)| at).collect();
                if !starts.is_empty() {
                    return unique(starts);
                }
            }
            None
        }
    }
}

/// A track as a fixed phrase finds it: its ref and name, whether it counts by place among the project's own tracks
/// (not a return or the master), and whether it's the master (never one a phrase names).
pub struct PhraseTrack<'a> {
    pub reference: &'a str,
    pub name: &'a str,
    pub own: bool,
    pub master: bool,
}

/// How a DAW makes each fixed phrase's one change: the make_changes step for it.
pub trait InstantVocab {
    /// Whether the DAW takes this tempo as the phrase gives it (Logic Pro's goes to 990 BPM): one it doesn't goes to the
    /// model, which says why.
    fn takes_tempo(&self, bpm: f64) -> bool {
        let _ = bpm;
        true
    }
    fn tempo(&self, bpm: f64) -> serde_json::Value;
    fn track(&self, reference: &str, switch: Switch, on: bool) -> serde_json::Value;
    /// A loop from `start` for `length` beats.
    fn loop_on(&self, start: f64, length: f64) -> serde_json::Value;
    fn loop_off(&self) -> serde_json::Value;
    fn metronome(&self, on: bool) -> serde_json::Value;
}

/// The make_changes plan for an intent: one step, so it lands in HISTORY with its undo and Kumi says what changed,
/// with no model call. None when it can't run as asked (a track the phrase names isn't there, or two fit), so the
/// request goes to the model.
pub fn instant_steps(vocab: &dyn InstantVocab, intent: &Intent, tracks: &[PhraseTrack], beats_per_bar: f64) -> Option<serde_json::Value> {
    let step = match intent {
        Intent::Tempo { bpm } if !vocab.takes_tempo(*bpm) => return None,
        Intent::Tempo { bpm } => vocab.tempo(*bpm),
        Intent::Track { track, switch, on } => vocab.track(&phrase_track(track, tracks)?, *switch, *on),
        Intent::Loop { from_bar, to_bar } => {
            let bar = if beats_per_bar.is_finite() && beats_per_bar > 0.0 { beats_per_bar } else { 4.0 };
            let start = f64::from(from_bar - 1) * bar;
            let length = f64::from(to_bar - from_bar + 1) * bar;
            vocab.loop_on(start, length)
        }
        Intent::LoopOff => vocab.loop_off(),
        Intent::Metronome { on } => vocab.metronome(*on),
    };
    Some(serde_json::json!({"steps": [step], "final": true}))
}

/// The track a phrase means: by name among every track but the master, or by place among the project's own tracks.
pub fn phrase_track(track: &TrackPick, tracks: &[PhraseTrack]) -> Option<String> {
    let tracks: Vec<_> = tracks
        .iter()
        .filter(|node| match track {
            TrackPick::Position(_) => node.own,
            TrackPick::Name(_) => !node.master,
        })
        .collect();
    let names: Vec<&str> = tracks.iter().map(|node| node.name).collect();
    resolve(track, &names).map(|at| tracks[at].reference.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn name(text: &str) -> TrackPick {
        TrackPick::Name(text.into())
    }

    /// Each input, and what it runs: Some(intent), or None when it must go to the model.
    #[test]
    fn only_a_whole_fixed_phrase_runs_without_the_model() {
        use Intent::*;
        let track = |pick: TrackPick, switch: Switch, on: bool| Some(Track { track: pick, switch, on });
        let table: Vec<(&str, Option<Intent>)> = vec![
            ("tempo 124", Some(Tempo { bpm: 124.0 })),
            ("Tempo 124", Some(Tempo { bpm: 124.0 })),
            ("bpm 124", Some(Tempo { bpm: 124.0 })),
            ("124 bpm", Some(Tempo { bpm: 124.0 })),
            ("124bpm", Some(Tempo { bpm: 124.0 })),
            ("tempo 124 bpm", Some(Tempo { bpm: 124.0 })),
            ("tempo 87.5", Some(Tempo { bpm: 87.5 })),
            ("  tempo   140.  ", Some(Tempo { bpm: 140.0 })),
            ("tempo 19", None),
            ("tempo 1000", None),
            ("tempo 124?", None),
            ("tempo up a bit", None),
            ("set the tempo to 124 and add a kick", None),
            ("what's the tempo", None),
            ("tempo", None),
            ("mute drums", track(name("drums"), Switch::Mute, true)),
            ("Mute Drums", track(name("drums"), Switch::Mute, true)),
            ("unmute drums", track(name("drums"), Switch::Mute, false)),
            ("solo bass", track(name("bass"), Switch::Solo, true)),
            ("unsolo bass", track(name("bass"), Switch::Solo, false)),
            ("arm vocals", track(name("vocals"), Switch::Arm, true)),
            ("disarm vocals", track(name("vocals"), Switch::Arm, false)),
            ("mute the drums", track(name("the drums"), Switch::Mute, true)),
            ("mute track 3", track(TrackPick::Position(3), Switch::Mute, true)),
            ("solo lead synth", track(name("lead synth"), Switch::Solo, true)),
            ("mute drums.", track(name("drums"), Switch::Mute, true)),
            ("mute track 0", None),
            ("mute drums and add reverb", None),
            ("mute the drums a bit", None),
            ("solo the bass?", None),
            ("mute drums, then solo bass", None),
            ("mute drums and bass", None),
            ("mute everything", track(name("everything"), Switch::Mute, true)),
            ("mute all tracks", None),
            ("mute the other tracks", None),
            ("solo the bass please", None),
            ("mute", None),
            ("mute the kick when the vocal comes in", None),
            ("mute drums!", None),
            ("arm the vocal track and record", None),
            ("loop 9-16", Some(Loop { from_bar: 9, to_bar: 16 })),
            ("loop 9 - 16", Some(Loop { from_bar: 9, to_bar: 16 })),
            ("loop 9–16", Some(Loop { from_bar: 9, to_bar: 16 })),
            ("loop bars 9 to 16", Some(Loop { from_bar: 9, to_bar: 16 })),
            ("loop bar 1 to 4", Some(Loop { from_bar: 1, to_bar: 4 })),
            ("loop 5 through 8", Some(Loop { from_bar: 5, to_bar: 8 })),
            ("loop 16-9", None),
            ("loop 0-4", None),
            ("loop off", Some(LoopOff)),
            ("Loop off.", Some(LoopOff)),
            ("loop on", None),
            ("loop the chorus", None),
            ("loop 9-16 and play", None),
            ("loop", None),
            ("metronome on", Some(Metronome { on: true })),
            ("metronome off", Some(Metronome { on: false })),
            ("Metronome Off", Some(Metronome { on: false })),
            ("metronome", None),
            ("turn the metronome on", None),
            ("make the bass darker", None),
            ("mute drums\nsolo bass", None),
            ("", None),
            ("solo bass (just for now)", None),
            ("mute \"drums\"", None),
        ];
        assert!(table.len() >= 50, "{} inputs", table.len());
        for (input, want) in table {
            assert_eq!(parse(input), want, "{input:?}");
        }
    }

    #[test]
    fn a_track_is_its_one_exact_name_else_its_one_prefix_else_its_place() {
        let names = ["Drums", "Bass", "Bass FX", "Lead Synth", "Kick 1", "Kick 2", "the end", "Vox"];
        assert_eq!(resolve(&name("drums"), &names), Some(0));
        assert_eq!(resolve(&name("DRUMS"), &names), Some(0));
        assert_eq!(resolve(&name("the drums"), &names), Some(0));
        assert_eq!(resolve(&name("bass"), &names), Some(1), "an exact name beats a longer one it starts");
        assert_eq!(resolve(&name("bass fx"), &names), Some(2));
        assert_eq!(resolve(&name("lead"), &names), Some(3), "the one name it starts");
        assert_eq!(resolve(&name("lead  synth"), &names), Some(3));
        assert_eq!(resolve(&name("kick"), &names), None, "two fit: the model decides");
        assert_eq!(resolve(&name("the end"), &names), Some(6), "a name that starts with the");
        assert_eq!(resolve(&name("guitar"), &names), None);
        assert_eq!(resolve(&name("v"), &names), None, "one letter isn't a name");
        assert_eq!(resolve(&TrackPick::Position(2), &names), Some(1));
        assert_eq!(resolve(&TrackPick::Position(9), &names), None);
        assert_eq!(resolve(&name("drums"), &["Drums", "drums"]), None, "two of the same name");
    }
}
