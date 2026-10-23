//! What a sound is, the way a producer would file it: its instrument class (kick, snare, pad…),
//! one-shot or loop, a loop's tempo and a sound's key or note. Names say most of it ("Kick 808
//! Long", "Bass Loop 128 Fmin", a "Hats" folder); the sound itself says the rest, and checks the
//! name where a name is ambiguous ("Stab C" is a C only if it sounds like one).

use std::collections::HashMap;
use std::sync::LazyLock;

use kumi_common::js::number::round;
use regex::Regex;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SoundClass {
    Kick,
    Snare,
    Clap,
    Hat,
    Cymbal,
    Tom,
    Perc,
    Drums,
    Bass,
    Lead,
    Pad,
    Keys,
    Pluck,
    Stab,
    Synth,
    Guitar,
    Strings,
    Brass,
    Vocal,
    Fx,
    Noise,
    Texture,
}

pub const CLASSES: [SoundClass; 22] = [
    SoundClass::Kick,
    SoundClass::Snare,
    SoundClass::Clap,
    SoundClass::Hat,
    SoundClass::Cymbal,
    SoundClass::Tom,
    SoundClass::Perc,
    SoundClass::Drums,
    SoundClass::Bass,
    SoundClass::Lead,
    SoundClass::Pad,
    SoundClass::Keys,
    SoundClass::Pluck,
    SoundClass::Stab,
    SoundClass::Synth,
    SoundClass::Guitar,
    SoundClass::Strings,
    SoundClass::Brass,
    SoundClass::Vocal,
    SoundClass::Fx,
    SoundClass::Noise,
    SoundClass::Texture,
];

impl SoundClass {
    /// The class's name ("kick").
    pub fn as_str(self) -> &'static str {
        match self {
            SoundClass::Kick => "kick",
            SoundClass::Snare => "snare",
            SoundClass::Clap => "clap",
            SoundClass::Hat => "hat",
            SoundClass::Cymbal => "cymbal",
            SoundClass::Tom => "tom",
            SoundClass::Perc => "perc",
            SoundClass::Drums => "drums",
            SoundClass::Bass => "bass",
            SoundClass::Lead => "lead",
            SoundClass::Pad => "pad",
            SoundClass::Keys => "keys",
            SoundClass::Pluck => "pluck",
            SoundClass::Stab => "stab",
            SoundClass::Synth => "synth",
            SoundClass::Guitar => "guitar",
            SoundClass::Strings => "strings",
            SoundClass::Brass => "brass",
            SoundClass::Vocal => "vocal",
            SoundClass::Fx => "fx",
            SoundClass::Noise => "noise",
            SoundClass::Texture => "texture",
        }
    }

    /// A class from its name, when the text is one.
    pub fn parse(text: &str) -> Option<SoundClass> {
        CLASSES.into_iter().find(|class| class.as_str() == text)
    }
}

impl std::fmt::Display for SoundClass {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SoundKind {
    OneShot,
    Loop,
}

impl SoundKind {
    pub fn as_str(self) -> &'static str {
        match self {
            SoundKind::OneShot => "one-shot",
            SoundKind::Loop => "loop",
        }
    }
}

/// Words in names, and the class each says; earlier classes win when a name says several.
fn words_of(class: SoundClass) -> &'static [&'static str] {
    match class {
        SoundClass::Kick => &["kick", "kicks", "kik", "kck", "bd", "bassdrum", "kickdrum"],
        SoundClass::Snare => &["snare", "snares", "snr", "sd", "rimshot", "snareroll"],
        SoundClass::Clap => &["clap", "claps", "clp", "handclap", "handclaps", "snap", "snaps", "fingersnap"],
        SoundClass::Hat => &["hat", "hats", "hh", "hihat", "hihats", "ohh", "chh", "ch", "oh", "openhat", "closedhat"],
        SoundClass::Cymbal => &["cymbal", "cymbals", "crash", "crashes", "ride", "rides", "splash", "china", "cym"],
        SoundClass::Tom => &["tom", "toms", "floortom"],
        SoundClass::Perc => &[
            "perc",
            "percs",
            "percussion",
            "conga",
            "congas",
            "bongo",
            "bongos",
            "tabla",
            "cowbell",
            "clave",
            "claves",
            "shaker",
            "shakers",
            "tamb",
            "tambourine",
            "woodblock",
            "wood",
            "rim",
            "block",
            "triangle",
            "guiro",
            "cabasa",
            "djembe",
            "timbale",
            "timbales",
            "agogo",
            "click",
            "eperc",
        ],
        SoundClass::Drums => &[
            "drums",
            "drum",
            "break",
            "breaks",
            "breakbeat",
            "beat",
            "beats",
            "kit",
            "tops",
            "top",
            "toploop",
            "fullkit",
            "groove",
            "fill",
            "fills",
        ],
        SoundClass::Bass => &["bass", "basses", "bassline", "sub", "subs", "808", "808s", "reese", "lowend"],
        SoundClass::Lead => &["lead", "leads", "ld", "solo"],
        SoundClass::Pad => &["pad", "pads"],
        SoundClass::Keys => &[
            "keys",
            "key",
            "piano",
            "pianos",
            "rhodes",
            "wurli",
            "wurlitzer",
            "ep",
            "organ",
            "organs",
            "clav",
            "clavinet",
            "harpsichord",
            "celesta",
            "marimba",
            "vibes",
            "vibraphone",
            "xylophone",
            "kalimba",
            "mallet",
            "mallets",
            "bell",
            "bells",
            "chord",
            "chords",
        ],
        SoundClass::Pluck => &["pluck", "plucks", "plk"],
        SoundClass::Stab => &["stab", "stabs"],
        SoundClass::Synth => &["synth", "synths", "arp", "arps", "seq", "sequence", "bleep", "bleeps", "blip"],
        SoundClass::Guitar => &["guitar", "guitars", "gtr", "strum", "strums", "riff"],
        SoundClass::Strings => {
            &["strings", "string", "violin", "violins", "viola", "cello", "cellos", "orchestra", "orchestral", "pizz", "pizzicato"]
        }
        SoundClass::Brass => &["brass", "horn", "horns", "trumpet", "trumpets", "sax", "saxophone", "trombone", "flute", "woodwind"],
        SoundClass::Vocal => &[
            "vocal",
            "vocals",
            "vox",
            "voc",
            "voice",
            "voices",
            "acapella",
            "acappella",
            "adlib",
            "adlibs",
            "chant",
            "chants",
            "choir",
            "spoken",
            "phrase",
            "shout",
            "shouts",
            "scream",
            "speech",
            "hum",
            "bv",
            "bvs",
        ],
        SoundClass::Fx => &[
            "fx",
            "sfx",
            "riser",
            "risers",
            "rise",
            "uplifter",
            "downlifter",
            "sweep",
            "sweeps",
            "impact",
            "impacts",
            "whoosh",
            "swoosh",
            "swell",
            "transition",
            "reverse",
            "reversed",
            "glitch",
            "laser",
            "zap",
            "boom",
            "siren",
            "tapestop",
            "buildup",
            "drop",
            "foley",
            "effect",
            "effects",
        ],
        SoundClass::Noise => &["noise", "noises", "hiss", "crackle", "static", "vinylnoise"],
        SoundClass::Texture => &[
            "texture",
            "textures",
            "atmos",
            "atmosphere",
            "atmospheres",
            "ambience",
            "ambient",
            "drone",
            "drones",
            "soundscape",
            "field",
            "fieldrecording",
            "room",
            "rain",
        ],
    }
}

const PRIORITY: [SoundClass; 22] = [
    SoundClass::Kick,
    SoundClass::Snare,
    SoundClass::Clap,
    SoundClass::Hat,
    SoundClass::Cymbal,
    SoundClass::Tom,
    SoundClass::Vocal,
    SoundClass::Bass,
    SoundClass::Perc,
    SoundClass::Drums,
    SoundClass::Lead,
    SoundClass::Pluck,
    SoundClass::Stab,
    SoundClass::Pad,
    SoundClass::Keys,
    SoundClass::Guitar,
    SoundClass::Strings,
    SoundClass::Brass,
    SoundClass::Fx,
    SoundClass::Noise,
    SoundClass::Texture,
    SoundClass::Synth,
];

static CLASS_OF: LazyLock<HashMap<&'static str, SoundClass>> = LazyLock::new(|| {
    let mut map = HashMap::new();
    for class in PRIORITY {
        for word in words_of(class) {
            map.entry(*word).or_insert(class);
        }
    }
    map
});

/// Words that are a class only beside a drum word or in a drum folder ("open" and "ch" alone mean little).
const WEAK: &[&str] = &[
    "oh", "ch", "wood", "block", "rim", "click", "top", "tops", "fill", "fills", "key", "drop", "room", "field", "hum", "rise", "boom",
    "beat", "beats", "kit", "riff",
];
const DRUM_ELEMENTS: &[SoundClass] =
    &[SoundClass::Kick, SoundClass::Snare, SoundClass::Clap, SoundClass::Hat, SoundClass::Cymbal, SoundClass::Tom, SoundClass::Perc];
/// Classes with no key to speak of.
const UNTUNED: &[SoundClass] = &[
    SoundClass::Kick,
    SoundClass::Snare,
    SoundClass::Clap,
    SoundClass::Hat,
    SoundClass::Cymbal,
    SoundClass::Tom,
    SoundClass::Perc,
    SoundClass::Drums,
    SoundClass::Fx,
    SoundClass::Noise,
    SoundClass::Texture,
];

const LOOP_WORDS: &[&str] = &[
    "loop",
    "loops",
    "lp",
    "bpm",
    "groove",
    "grooves",
    "break",
    "breaks",
    "breakbeat",
    "beat",
    "beats",
    "phrase",
    "riff",
    "riffs",
    "arp",
    "arps",
    "fill",
    "fills",
    "tops",
    "toploop",
];
const SHOT_WORDS: &[&str] =
    &["oneshot", "oneshots", "shot", "shots", "hit", "hits", "single", "singles", "stab", "stabs", "multisample", "multisamples"];

static CAMEL: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"([a-z])([A-Z])").unwrap());
static LETTERS_DIGIT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"([A-Za-z]{2,})([0-9])").unwrap());
static DIGIT_LETTERS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"([0-9])([A-Za-z]{3,})").unwrap());
static NOT_A_TOKEN: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[^a-z0-9#]+").unwrap());

/// Words of a name or folder: "KickPunchy_01" is kick, punchy, 01; "F#m" and "808" stay whole.
pub fn tokens(text: &str) -> Vec<String> {
    let spaced = CAMEL.replace_all(text, "$1 $2");
    let spaced = LETTERS_DIGIT.replace_all(&spaced, "$1 $2");
    let spaced = DIGIT_LETTERS.replace_all(&spaced, "$1 $2");
    NOT_A_TOKEN.split(&spaced.to_lowercase()).filter(|word| !word.is_empty()).map(str::to_string).collect()
}

/// Pairs of words that name one thing: "hi hat", "bass drum", "one shot".
const PAIRS: &[(&str, &str)] = &[
    ("hi hat", "hihat"),
    ("hi hats", "hihats"),
    ("bass drum", "bassdrum"),
    ("kick drum", "kickdrum"),
    ("one shot", "oneshot"),
    ("one shots", "oneshots"),
    ("open hat", "openhat"),
    ("closed hat", "closedhat"),
    ("white noise", "noise"),
    ("pink noise", "noise"),
    ("field recording", "fieldrecording"),
    ("tape stop", "tapestop"),
    ("snare roll", "snareroll"),
    ("hand clap", "handclap"),
    ("finger snap", "fingersnap"),
    ("floor tom", "floortom"),
    ("e perc", "eperc"),
    ("vinyl noise", "vinylnoise"),
    ("low end", "lowend"),
    ("top loop", "toploop"),
    ("tops loop", "toploop"),
    ("full kit", "fullkit"),
];

fn joined(words: Vec<String>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut index = 0;
    while index < words.len() {
        let pair = if index + 1 < words.len() {
            let both = format!("{} {}", words[index], words[index + 1]);
            PAIRS.iter().find(|(pair, _)| *pair == both).map(|(_, one)| *one)
        } else {
            None
        };
        if let Some(pair) = pair {
            out.push(pair.to_string());
            index += 1;
        } else {
            out.push(words[index].clone());
        }
        index += 1;
    }
    out
}

const NOTE_NAMES: [&str; 12] = ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"];
const FLATS: &[(&str, &str)] =
    &[("Db", "C#"), ("Eb", "D#"), ("Gb", "F#"), ("Ab", "G#"), ("Bb", "A#"), ("Cb", "B"), ("Fb", "E"), ("E#", "F"), ("B#", "C")];

/// "F#", "Bb" → a pitch class name with sharps ("A#"), or None.
pub fn pitch_class(letter: &str, accidental: &str) -> Option<String> {
    let lower = accidental.to_lowercase();
    let sign = if lower == "sharp" || accidental == "♯" || accidental == "s" {
        "#"
    } else if lower == "flat" || accidental == "♭" {
        "b"
    } else {
        accidental
    };
    let name = format!("{}{sign}", letter.to_uppercase());
    let sharp = FLATS.iter().find(|(flat, _)| *flat == name).map(|(_, sharp)| sharp.to_string()).unwrap_or(name);
    NOTE_NAMES.contains(&sharp.as_str()).then_some(sharp)
}

/// "A minor", "F# major": what keys are called here.
pub fn key_name(root: &str, minor: bool) -> String {
    format!("{root} {}", if minor { "minor" } else { "major" })
}

static SPELLED_KEY: LazyLock<fancy_regex::Regex> = LazyLock::new(|| {
    fancy_regex::Regex::new(
        r"(?<![A-Za-z0-9#])([A-Ga-g])\s?(#|b|♯|♭|sharp|flat)?[\s_-]?(major|minor|maj|min|m)(?:7|6|9|11|13)?(?![A-Za-z0-9#])",
    )
    .unwrap()
});

/// A key said in words, the way producers write them: "A minor", "F#m", "Bbmaj", "C# min", "Dmin7".
pub fn parse_key(text: &str) -> Option<String> {
    let spelled = SPELLED_KEY.captures(text).ok().flatten()?;
    let letter = spelled.get(1).map(|found| found.as_str()).unwrap_or_default();
    let root = pitch_class(letter, spelled.get(2).map(|found| found.as_str()).unwrap_or_default());
    let quality = spelled.get(3).map(|found| found.as_str()).unwrap_or_default();
    // A capital letter then "m" is minor; a small letter needs the word spelled out ("am" isn't a key).
    match root {
        Some(root) if letter == letter.to_uppercase() || quality.len() > 1 => {
            Some(key_name(&root, quality == "m" || quality.to_lowercase().starts_with("min")))
        }
        _ => None,
    }
}

/// A note with its octave, as sample packs name pitched sounds: "C3", "F#2", "Bb0".
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Note {
    pub name: String,
    pub midi: i64,
}

static NOTE_WITH_OCTAVE: LazyLock<fancy_regex::Regex> =
    LazyLock::new(|| fancy_regex::Regex::new(r"(?<![A-Za-z0-9#])([A-G])(#|b)?(-?[0-9])(?![A-Za-z0-9#])").unwrap());

pub fn parse_note(text: &str) -> Option<Note> {
    let found = NOTE_WITH_OCTAVE.captures(text).ok().flatten()?;
    let root = pitch_class(&found[1], found.get(2).map(|found| found.as_str()).unwrap_or_default())?;
    let octave: i64 = found[3].parse().ok()?;
    let index = NOTE_NAMES.iter().position(|name| *name == root).unwrap_or(0) as i64;
    Some(Note { name: format!("{root}{octave}"), midi: (octave + 1) * 12 + index })
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Tempo {
    pub bpm: f64,
    pub explicit: bool,
}

static SPELLED_TEMPO: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)([0-9]{2,3}(?:\.[0-9]+)?)\s?bpm|bpm\s?([0-9]{2,3}(?:\.[0-9]+)?)").unwrap());
static BARE_TEMPO: LazyLock<fancy_regex::Regex> = LazyLock::new(|| fancy_regex::Regex::new(r"(?<![0-9.])([0-9]{2,3})(?![0-9.])").unwrap());

/// A tempo a name gives ("128 BPM", "bpm90", "_128_" in a loop's name). Bare numbers need the sound's length to agree.
pub fn parse_tempo(text: &str) -> Vec<Tempo> {
    let mut found: Vec<Tempo> = Vec::new();
    for spelled in SPELLED_TEMPO.captures_iter(text) {
        let number = spelled.get(1).or_else(|| spelled.get(2)).map(|found| found.as_str()).unwrap_or_default();
        if let Ok(bpm) = number.parse::<f64>() {
            found.push(Tempo { bpm, explicit: true });
        }
    }
    for bare in BARE_TEMPO.captures_iter(text).filter_map(Result::ok) {
        let Ok(bpm) = bare[1].parse::<f64>() else { continue };
        if (60.0..=200.0).contains(&bpm) && !found.iter().any(|item| item.bpm == bpm) {
            found.push(Tempo { bpm, explicit: false });
        }
    }
    found.into_iter().filter(|item| item.bpm >= 40.0 && item.bpm <= 300.0).collect()
}

/// Where a class came from: the file's own name, a folder it's in, or how it sounds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ClassFrom {
    Name,
    Folder,
    Sound,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NameHints {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub class: Option<SoundClass>,
    /// Where the class came from: the file's own name, or a folder it's in.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub class_from: Option<ClassFrom>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kind: Option<SoundKind>,
    pub tempos: Vec<Tempo>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<Note>,
    /// A lone note letter ("Stab C"): believed only if the sound agrees.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub letter: Option<String>,
    /// The words of its name and folders.
    pub words: Vec<String>,
}

static EXTENSION: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\.[A-Za-z0-9]{2,5}$").unwrap());
static LONE_LETTER: LazyLock<fancy_regex::Regex> =
    LazyLock::new(|| fancy_regex::Regex::new(r"(?<![A-Za-z0-9#])([A-G])(#|b)?(?![A-Za-z0-9#'’])").unwrap());

/// What a sound's path says about it, from its name first and then its folders, nearest first.
pub fn name_hints(relative_path: &str) -> NameHints {
    let mut parts: Vec<&str> = relative_path.split(['\\', '/']).filter(|part| !part.is_empty()).collect();
    let file = EXTENSION.replace(parts.pop().unwrap_or_default(), "").into_owned();
    let name_words = joined(tokens(&file));
    parts.reverse();
    let folder_words: Vec<Vec<String>> = parts.iter().map(|part| joined(tokens(part))).collect();
    let all: Vec<String> = std::iter::once(&name_words).chain(folder_words.iter()).flatten().cloned().collect();
    let drum_context = all.iter().any(|word| match CLASS_OF.get(word.as_str()) {
        Some(found) => !WEAK.contains(&word.as_str()) && (DRUM_ELEMENTS.contains(found) || *found == SoundClass::Drums),
        None => false,
    });
    let classes_in = |words: &[String]| -> Vec<SoundClass> {
        words
            .iter()
            .filter_map(|word| if WEAK.contains(&word.as_str()) && !drum_context { None } else { CLASS_OF.get(word.as_str()).copied() })
            .collect()
    };
    let mut kind: Option<SoundKind> = None;
    for words in std::iter::once(&name_words).chain(folder_words.iter()) {
        if words.iter().any(|word| SHOT_WORDS.contains(&word.as_str())) {
            kind = Some(SoundKind::OneShot);
            break;
        }
        if words.iter().any(|word| LOOP_WORDS.contains(&word.as_str())) {
            kind = Some(SoundKind::Loop);
            break;
        }
    }
    let mut found: Option<SoundClass> = None;
    let mut from: Option<ClassFrom> = None;
    let named = classes_in(&name_words);
    if !named.is_empty() {
        // A loop of several drums, or a "drum loop" of one ("drum loop kick heavy"), is drums; a "Hat Loop" is a hat.
        let mut elements: Vec<SoundClass> = Vec::new();
        for class in named.iter().filter(|class| DRUM_ELEMENTS.contains(class)) {
            if !elements.contains(class) {
                elements.push(*class);
            }
        }
        found = if kind == Some(SoundKind::Loop) && (elements.len() >= 2 || (!elements.is_empty() && named.contains(&SoundClass::Drums))) {
            Some(SoundClass::Drums)
        } else {
            PRIORITY.into_iter().find(|class| named.contains(class))
        };
        from = Some(ClassFrom::Name);
    } else {
        for words in &folder_words {
            let in_folder = classes_in(words);
            if !in_folder.is_empty() {
                found = PRIORITY.into_iter().find(|class| in_folder.contains(class));
                from = Some(ClassFrom::Folder);
                break;
            }
        }
    }
    // A key ("Fmin"), else a note with its octave ("C3"), else a lone letter the sound has to agree with.
    let key = parse_key(&file);
    let note = if key.is_some() { None } else { parse_note(&file) };
    let normalized_file = file.replace(['_', '-'], " ");
    let letter = if key.is_none() && note.is_none() { LONE_LETTER.captures(&normalized_file).ok().flatten() } else { None };
    let lone = letter.and_then(|letter| pitch_class(&letter[1], letter.get(2).map(|found| found.as_str()).unwrap_or_default()));
    NameHints {
        class: found,
        class_from: if found.is_some() { from } else { None },
        kind,
        tempos: parse_tempo(&file),
        key,
        note,
        letter: lone,
        words: all,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Pitch {
    pub hz: f64,
    pub confidence: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HeardKey {
    pub name: String,
    pub confidence: f64,
}

/// What listening measured, as classify() needs it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Heard {
    pub seconds: f64,
    pub centroid_hz: f64,
    pub flatness: f64,
    pub attack_ms: f64,
    pub decay_ms: f64,
    pub onsets_per_second: f64,
    pub low_share: f64,
    pub high_share: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pitch: Option<Pitch>,
    /// Tempo from the onsets' rhythm, when there is one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rhythm_bpm: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rhythm_confidence: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<HeardKey>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Classified {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub class: Option<SoundClass>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub class_from: Option<ClassFrom>,
    pub kind: SoundKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bpm: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// How well a length fits whole beats at a tempo: beats in it, and how far off (as a share of a beat).
fn beats_at(seconds: f64, bpm: f64) -> (i64, f64) {
    let beats = seconds * bpm / 60.0;
    (round(beats) as i64, (beats - round(beats)).abs())
}

fn fits_bars(seconds: f64, bpm: f64) -> bool {
    let (beats, off) = beats_at(seconds, bpm);
    beats >= 2 && off < 0.06 && (beats % 2 == 0 || beats == 3 || beats % 3 == 0)
}

/// The name's hints and what the sound measured, made one answer.
pub fn classify(hints: &NameHints, heard: &Heard) -> Classified {
    // Tempo: the name's when the length agrees (or it says BPM outright), else one the length and rhythm agree on.
    let named = hints
        .tempos
        .iter()
        .find(|item| item.explicit)
        .or_else(|| hints.tempos.iter().find(|item| fits_bars(heard.seconds, item.bpm)))
        .copied();
    let mut bpm = named.map(|item| item.bpm);
    if bpm.is_none() && heard.seconds >= 1.5 {
        bpm = tempo_from_length(heard.seconds, heard.rhythm_bpm, heard.rhythm_confidence.unwrap_or(0.0));
    }
    // A loop by name with a tail past its bars: its rhythm's tempo, when that's clear.
    if bpm.is_none()
        && hints.kind == Some(SoundKind::Loop)
        && heard.rhythm_bpm.is_some_and(|rhythm| rhythm != 0.0)
        && heard.rhythm_confidence.unwrap_or(0.0) >= 0.4
    {
        bpm = heard.rhythm_bpm;
    }
    let rhythmic = heard.onsets_per_second >= 1.2;
    // Under a second is a hit, whatever its folder says (a "Break" kit's hat is one hit).
    let kind = if heard.seconds < 1.0 {
        SoundKind::OneShot
    } else {
        hints.kind.unwrap_or(if heard.seconds < 1.2 {
            SoundKind::OneShot
        } else if bpm.is_some_and(|bpm| rhythmic && fits_bars(heard.seconds, bpm)) {
            SoundKind::Loop
        } else {
            SoundKind::OneShot
        })
    };
    if kind == SoundKind::OneShot && !named.is_some_and(|item| item.explicit) {
        bpm = None;
    }
    let pitched = heard.pitch.filter(|pitch| pitch.confidence >= 0.5);
    let midi = pitched.map(|pitch| round(69.0 + 12.0 * (pitch.hz / 440.0).log2()) as i64);
    let heard_note = midi.map(|midi| format!("{}{}", NOTE_NAMES[((midi % 12) + 12) as usize % 12], midi.div_euclid(12) - 1));
    let mut found = hints.class;
    let mut from = hints.class_from;
    // "808" alone is a bass in most packs; a short thump of one is the kick.
    if found == Some(SoundClass::Bass)
        && hints.words.iter().any(|word| word == "808")
        && !hints.words.iter().any(|word| words_of(SoundClass::Bass).contains(&word.as_str()) && word != "808" && word != "808s")
        && kind == SoundKind::OneShot
        && heard.decay_ms < 250.0
        && heard.seconds < 0.8
    {
        found = Some(SoundClass::Kick);
    }
    if found.is_none() {
        found = class_from_sound(kind, heard, pitched.as_ref());
        from = if found.is_some() { Some(ClassFrom::Sound) } else { None };
    }
    let note = hints.note.as_ref().map(|note| note.name.clone()).or(if kind == SoundKind::OneShot { heard_note } else { None });
    // A key is heard only in a tonal loop, and only when the sound is clear about it; a lone letter in the name must agree.
    let mut key = hints.key.clone();
    let tonal = found.is_none_or(|found| !UNTUNED.contains(&found));
    if key.is_none() && kind == SoundKind::Loop && tonal && heard.flatness < 0.15 {
        if let Some(heard_key) = &heard.key {
            if heard_key.confidence >= 0.5 {
                key = Some(heard_key.name.clone());
            } else if hints.letter.as_ref().is_some_and(|letter| heard_key.name.starts_with(&format!("{letter} "))) {
                key = Some(heard_key.name.clone());
            }
        }
    }
    Classified {
        class: found,
        class_from: if found.is_some() { from } else { None },
        kind,
        bpm: bpm.filter(|bpm| *bpm != 0.0).map(|bpm| round(bpm * 10.0) / 10.0),
        key,
        note,
    }
}

/// A loop's tempo from its length: whole bars (1, 2, 4, 8 or 16 of 4/4, or 3/4) at a tempo between
/// 70 and 180, the one nearest what the rhythm says, or the most usual when the rhythm says nothing.
pub fn tempo_from_length(seconds: f64, rhythm: Option<f64>, confidence: f64) -> Option<f64> {
    let mut candidates: Vec<(f64, i64)> = Vec::new();
    for beats in [4, 8, 16, 32, 64, 3, 6, 12, 24, 48] {
        let bpm = 60.0 * beats as f64 / seconds;
        if (70.0..=180.0).contains(&bpm) {
            candidates.push((bpm, beats));
        }
    }
    if candidates.is_empty() {
        return None;
    }
    if let Some(rhythm) = rhythm.filter(|rhythm| *rhythm != 0.0 && confidence >= 0.2) {
        // The rhythm's tempo, or its half or double, picks among the lengths that fit.
        let near = |bpm: f64| {
            [rhythm, rhythm * 2.0, rhythm / 2.0].into_iter().map(|value| (bpm / value).log2().abs()).fold(f64::INFINITY, f64::min)
        };
        let best = candidates.iter().copied().reduce(|a, b| if near(a.0) <= near(b.0) { a } else { b })?;
        if near(best.0) < 0.04 {
            return Some(best.0);
        }
        return None;
    }
    // No clear rhythm: a 4/4 length near 120 is the likeliest reading, and only a whole-number tempo is believed.
    let whole: Vec<(f64, i64)> = candidates.into_iter().filter(|(bpm, beats)| beats % 4 == 0 && (bpm - round(*bpm)).abs() < 0.05).collect();
    whole.into_iter().reduce(|a, b| if (a.0 - 120.0).abs() <= (b.0 - 120.0).abs() { a } else { b }).map(|(bpm, _)| bpm)
}

/// The class a nameless sound most likely is, from how it sounds.
fn class_from_sound(kind: SoundKind, heard: &Heard, pitched: Option<&Pitch>) -> Option<SoundClass> {
    let noisy = heard.flatness >= 0.25;
    if kind == SoundKind::Loop {
        if heard.onsets_per_second >= 2.0 && pitched.is_none() {
            return Some(if heard.high_share > 0.5 && heard.low_share < 0.1 { SoundClass::Hat } else { SoundClass::Drums });
        }
        if pitched.is_some_and(|pitch| pitch.hz < 160.0) {
            return Some(SoundClass::Bass);
        }
        return if pitched.is_some() {
            Some(SoundClass::Synth)
        } else if noisy {
            Some(SoundClass::Texture)
        } else {
            None
        };
    }
    let short = heard.seconds < 0.8 || heard.decay_ms < 400.0;
    if pitched.is_some_and(|pitch| pitch.hz < 130.0) && heard.low_share > 0.5 {
        return Some(if heard.decay_ms < 400.0 && heard.seconds < 1.5 { SoundClass::Kick } else { SoundClass::Bass });
    }
    if heard.low_share > 0.6 && heard.attack_ms < 15.0 && short {
        return Some(SoundClass::Kick);
    }
    if noisy && heard.centroid_hz > 5000.0 && heard.low_share < 0.1 {
        return Some(if heard.seconds < 0.6 { SoundClass::Hat } else { SoundClass::Cymbal });
    }
    if heard.attack_ms < 15.0 && short && heard.centroid_hz >= 900.0 && heard.centroid_hz < 6000.0 && noisy {
        return Some(SoundClass::Snare);
    }
    if heard.attack_ms < 15.0 && short {
        return Some(SoundClass::Perc);
    }
    if pitched.is_some() {
        return Some(if heard.attack_ms > 80.0 {
            SoundClass::Pad
        } else if heard.decay_ms < 600.0 {
            SoundClass::Pluck
        } else {
            SoundClass::Synth
        });
    }
    if noisy {
        return Some(if heard.seconds > 2.0 { SoundClass::Texture } else { SoundClass::Noise });
    }
    None
}
