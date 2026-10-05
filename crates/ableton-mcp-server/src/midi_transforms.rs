//! Deterministic, seeded MIDI transformation primitives. Every transform is a
//! pure function over the canonical note schema: identical input notes,
//! parameters, and seed always produce byte-for-byte identical output. No
//! transform performs taste judgment or artist imitation, and none authors
//! per-note Pitch/Slide/Pressure — those fields are not in the canonical note
//! schema and are never fabricated here.

use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};
use std::sync::LazyLock;

use kumi_common::js::json::{quote, stringify};
use kumi_common::js::number::{round, to_string as number_text};
use kumi_common::js::string::{trim, utf16_len};
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};

use crate::live::Maybe;
pub use crate::live::Note;

/// What a transform throws: `RangeError` for invalid parameters or notes, a plain `Error` from the digests.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum MidiTransformError {
    #[error("{0}")]
    Range(String),
    #[error("{0}")]
    Other(String),
}

fn range(message: impl Into<String>) -> MidiTransformError {
    MidiTransformError::Range(message.into())
}

type Params = Map<String, Value>;
type Outcome = Result<MidiTransformOutcome, MidiTransformError>;

pub const MIDI_TRANSFORM_TYPES: &[&str] = &[
    "transpose",
    "scale-constrain",
    "quantize",
    "swing",
    "velocity-curve",
    "humanize-velocity",
    "humanize-timing",
    "legato",
    "staccato",
    "rotate",
    "repeat",
    "ratchet",
    "chord-voicing",
    "arpeggiate",
    "seeded-variation",
    "euclidean",
    "chord-progression",
    "drum-pattern",
    "bassline",
    "motif-invert",
    "motif-retrograde",
    "motif-augment",
    "motif-diminish",
];

/// Transforms that only patch fields of existing notes via note.update (which
/// preserves unexposed per-note data server-side).
pub const UPDATE_ONLY_TRANSFORMS: &[&str] = &[
    "transpose",
    "scale-constrain",
    "quantize",
    "swing",
    "velocity-curve",
    "humanize-velocity",
    "humanize-timing",
    "legato",
    "staccato",
    "rotate",
    "chord-voicing",
    "seeded-variation",
    "motif-invert",
    "motif-retrograde",
    "motif-augment",
    "motif-diminish",
];
/// Transforms that create or delete notes (delete/recreate would drop any
/// per-note expression the canonical schema cannot represent).
pub const GENERATIVE_TRANSFORMS: &[&str] =
    &["repeat", "ratchet", "arpeggiate", "euclidean", "chord-progression", "drum-pattern", "bassline"];

/// Notes a transform takes and returns: no cap below what one wire array carries (a clip's notes are the Set's own).
pub const MIDI_TRANSFORM_MAX_NOTES: usize = 10_000_000;
/// Notes a generator (arpeggiate, euclidean, bassline) may create in one go, checked before any is made: past a
/// million a generated clip is a mistake, and the notes alone would take hundreds of MiB of the host's memory.
pub const MIDI_TRANSFORM_MAX_GENERATED_NOTES: usize = 1_000_000;
pub const MIDI_TRANSFORM_LARGE_UPDATE_THRESHOLD: usize = 128;

pub const SCALE_INTERVALS: &[(&str, &[i64])] = &[
    ("major", &[0, 2, 4, 5, 7, 9, 11]),
    ("minor", &[0, 2, 3, 5, 7, 8, 10]),
    ("harmonic-minor", &[0, 2, 3, 5, 7, 8, 11]),
    ("melodic-minor", &[0, 2, 3, 5, 7, 9, 11]),
    ("dorian", &[0, 2, 3, 5, 7, 9, 10]),
    ("phrygian", &[0, 1, 3, 5, 7, 8, 10]),
    ("lydian", &[0, 2, 4, 6, 7, 9, 11]),
    ("mixolydian", &[0, 2, 4, 5, 7, 9, 10]),
    ("locrian", &[0, 1, 3, 5, 6, 8, 10]),
    ("major-pentatonic", &[0, 2, 4, 7, 9]),
    ("minor-pentatonic", &[0, 3, 5, 7, 10]),
    ("blues", &[0, 3, 5, 6, 7, 10]),
    ("chromatic", &[0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11]),
];

/// The intervals of a named scale.
pub fn scale_intervals(scale: &str) -> Option<&'static [i64]> {
    SCALE_INTERVALS.iter().find(|(name, _)| *name == scale).map(|(_, intervals)| *intervals)
}

fn scale_names() -> Vec<&'static str> {
    SCALE_INTERVALS.iter().map(|(name, _)| *name).collect()
}

#[derive(Debug, Clone, PartialEq)]
pub struct MidiTransformSpec {
    pub r#type: String,
    pub params: Params,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct MidiTransformOutcome {
    pub notes: Vec<Note>,
    pub assumptions: Vec<String>,
    pub generative: bool,
    /// Seeded transforms must echo the exact seed; None for fully deterministic specs.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub seed: Option<String>,
}

/// Deterministic PRNG (mulberry32) seeded from a string or integer.
#[derive(Debug, Clone)]
pub struct SeededRandom {
    state: u32,
}

impl SeededRandom {
    /// `seededRandom(seed: string)`.
    pub fn from_text(seed: &str) -> SeededRandom {
        let digest = Sha256::digest(seed.as_bytes());
        SeededRandom { state: u32::from_le_bytes([digest[0], digest[1], digest[2], digest[3]]) }
    }

    /// `seededRandom(seed: number)`: the seed as `seed >>> 0`.
    pub fn from_number(seed: f64) -> SeededRandom {
        SeededRandom { state: to_uint32(seed) }
    }

    /// The next value in [0, 1).
    pub fn next(&mut self) -> f64 {
        self.state = self.state.wrapping_add(0x6d2b79f5);
        let mut value = self.state;
        value = (value ^ (value >> 15)).wrapping_mul(value | 1);
        value ^= value.wrapping_add((value ^ (value >> 7)).wrapping_mul(value | 61));
        (value ^ (value >> 14)) as f64 / 4294967296.0
    }
}

pub fn seeded_random(seed: &str) -> SeededRandom {
    SeededRandom::from_text(seed)
}

/// JavaScript's `ToUint32`.
fn to_uint32(value: f64) -> u32 {
    if !value.is_finite() {
        return 0;
    }
    value.trunc().rem_euclid(4294967296.0) as u32
}

/// JavaScript's default string order: UTF-16 code units.
fn js_str_cmp(a: &str, b: &str) -> Ordering {
    a.encode_utf16().cmp(b.encode_utf16())
}

fn cmp_f64(a: f64, b: f64) -> Ordering {
    a.partial_cmp(&b).unwrap_or(Ordering::Equal)
}

fn stable_cmp(a: &Note, b: &Note) -> Ordering {
    let id = |note: &Note| note.id.cloned().map(|id| id as f64).unwrap_or(-1.0);
    cmp_f64(a.start, b.start)
        .then_with(|| cmp_f64(a.pitch, b.pitch))
        .then_with(|| cmp_f64(id(a), id(b)))
        .then_with(|| cmp_f64(a.channel, b.channel))
}

/// Stable processing order: identical for any equal-content input regardless of input ordering.
pub fn stable_note_order(notes: &[Note]) -> Vec<Note> {
    let mut ordered = notes.to_vec();
    ordered.sort_by(stable_cmp);
    ordered
}

/// The indices of `notes` in stable order, for transforms that patch notes through that view.
fn stable_order_indices(notes: &[Note]) -> Vec<usize> {
    let mut indices: Vec<usize> = (0..notes.len()).collect();
    indices.sort_by(|a, b| stable_cmp(&notes[*a], &notes[*b]));
    indices
}

fn validate_note_set(notes: &[Note]) -> Result<(), MidiTransformError> {
    if notes.len() > MIDI_TRANSFORM_MAX_NOTES {
        return Err(range(format!("note collection exceeds the bounded {MIDI_TRANSFORM_MAX_NOTES}-note limit")));
    }
    for note in notes {
        if !(note.pitch.is_finite() && note.pitch.fract() == 0.0) || note.pitch < 0.0 || note.pitch > 127.0 {
            return Err(range("note pitch is invalid"));
        }
        if !note.start.is_finite() || note.start < 0.0 {
            return Err(range("note start is invalid"));
        }
        if !note.duration.is_finite() || note.duration <= 0.0 {
            return Err(range("note duration is invalid"));
        }
        if !note.velocity.is_finite() || note.velocity < 1.0 || note.velocity > 127.0 {
            return Err(range("note velocity is invalid"));
        }
    }
    Ok(())
}

/// `params[name] ?? fallback`: a null is as good as absent.
fn param<'a>(params: &'a Params, name: &str) -> Option<&'a Value> {
    params.get(name).filter(|value| !value.is_null())
}

fn finite_param(params: &Params, name: &str, min: f64, max: f64, fallback: Option<f64>) -> Result<f64, MidiTransformError> {
    let value = match param(params, name) {
        Some(value) => value.as_f64(),
        None => fallback,
    };
    match value {
        Some(value) if value.is_finite() && value >= min && value <= max => Ok(value),
        _ => Err(range(format!("{name} must be a finite number in [{}, {}]", number_text(min), number_text(max)))),
    }
}

fn integer_param(params: &Params, name: &str, min: i64, max: i64, fallback: Option<i64>) -> Result<i64, MidiTransformError> {
    let value = match param(params, name) {
        Some(value) => value.as_f64(),
        None => fallback.map(|fallback| fallback as f64),
    };
    match value {
        Some(value) if value.is_finite() && value.fract() == 0.0 && value >= min as f64 && value <= max as f64 => Ok(value as i64),
        _ => Err(range(format!("{name} must be an integer in [{min}, {max}]"))),
    }
}

fn string_param(params: &Params, name: &str, allowed: &[&str], fallback: Option<&str>) -> Result<String, MidiTransformError> {
    let value = match param(params, name) {
        Some(Value::String(value)) => Some(value.as_str()),
        Some(_) => None,
        None => fallback,
    };
    match value {
        Some(value) if allowed.contains(&value) => Ok(value.to_string()),
        _ => Err(range(format!("{name} must be one of {}", allowed.join(", ")))),
    }
}

/// `String.prototype.localeCompare` under the root collation, for the ASCII identifiers parameter
/// names are: punctuation, then digits, then letters compared without case, and lower case before
/// upper case when nothing else differs.
// TS: localeCompare; characters outside ASCII sort after it by code point.
pub(crate) fn locale_compare(a: &str, b: &str) -> Ordering {
    const PUNCTUATION: &str = "_-,;:!?.'\"()[]{}@*/\\&#%`^+<=>|~$";
    let primary = |character: char| -> (u8, u32) {
        if character.is_ascii_whitespace() || character.is_ascii_control() {
            (0, character as u32)
        } else if let Some(index) = PUNCTUATION.find(character) {
            (1, index as u32)
        } else if character.is_ascii_digit() {
            (2, character as u32)
        } else if character.is_ascii_alphabetic() {
            (3, character.to_ascii_lowercase() as u32)
        } else {
            (4, character as u32)
        }
    };
    let by_primary = a.chars().map(primary).cmp(b.chars().map(primary));
    if by_primary != Ordering::Equal {
        return by_primary;
    }
    a.chars().map(|character| character.is_ascii_uppercase()).cmp(b.chars().map(|character| character.is_ascii_uppercase()))
}

/// The seed a stochastic transform uses: the one given, or one drawn from the request itself when none
/// is, so a preview and its apply agree. Another seed gives another variation.
fn seed_param(params: &Params) -> Result<String, MidiTransformError> {
    match params.get("seed") {
        None => {
            let mut entries: Vec<(&String, &Value)> = params.iter().filter(|(key, _)| *key != "seed").collect();
            entries.sort_by(|a, b| locale_compare(a.0, b.0));
            let request: Params = entries.into_iter().map(|(key, value)| (key.clone(), value.clone())).collect();
            let digest = hex::encode(Sha256::digest(stringify(&Value::Object(request)).as_bytes()));
            Ok(format!("auto-{}", &digest[..16]))
        }
        Some(Value::String(seed)) if (1..=128).contains(&utf16_len(seed)) => Ok(seed.clone()),
        Some(_) => Err(range("seed is a string of 1-128 characters")),
    }
}

fn clamp_pitch(value: f64) -> f64 {
    value.max(0.0).min(127.0)
}

fn clamp_velocity(value: f64) -> f64 {
    value.max(1.0).min(127.0)
}

fn transpose(notes: &[Note], params: &Params) -> Outcome {
    let semitones = integer_param(params, "semitones", -48, 48, None)? as f64;
    let mut clamped = 0;
    let mut result = notes.to_vec();
    for note in &mut result {
        let target = note.pitch + semitones;
        let next = clamp_pitch(target);
        if next != target {
            clamped += 1;
        }
        note.pitch = next;
    }
    Ok(MidiTransformOutcome {
        notes: result,
        generative: false,
        assumptions: if clamped > 0 { vec![format!("{clamped} note(s) clamped to the MIDI pitch range")] } else { vec![] },
        seed: None,
    })
}

fn nearest_scale_pitch(pitch: f64, root: i64, intervals: &[i64]) -> f64 {
    let pitch = pitch as i64;
    let mut best = pitch;
    let mut best_distance = i64::MAX;
    for candidate in (pitch - 12).max(0)..=(pitch + 12).min(127) {
        let degree = (((candidate - root) % 12) + 12) % 12;
        if !intervals.contains(&degree) {
            continue;
        }
        let distance = (candidate - pitch).abs();
        if distance < best_distance {
            best = candidate;
            best_distance = distance;
        }
    }
    best as f64
}

fn scale_constrain(notes: &[Note], params: &Params) -> Outcome {
    let root = integer_param(params, "root", 0, 11, None)?;
    let scale = string_param(params, "scale", &scale_names(), None)?;
    let intervals = scale_intervals(&scale).unwrap_or(&[]);
    let mut snapped = 0;
    let mut result = notes.to_vec();
    for note in &mut result {
        let target = nearest_scale_pitch(note.pitch, root, intervals);
        if target != note.pitch {
            snapped += 1;
        }
        note.pitch = target;
    }
    Ok(MidiTransformOutcome {
        notes: result,
        generative: false,
        assumptions: vec![format!(
            "scale {scale} at root {root}; {snapped} note(s) snapped to the nearest scale tone (ties resolve downward)"
        )],
        seed: None,
    })
}

fn quantize(notes: &[Note], params: &Params, clip_length: Option<f64>) -> Outcome {
    let grid = finite_param(params, "grid", 1.0 / 1024.0, 64.0, None)?;
    let amount = finite_param(params, "amount", 0.0, 1.0, Some(1.0))?;
    let target = string_param(params, "target", &["start", "end", "both"], Some("start"))?;
    let mut result = notes.to_vec();
    for note in &mut result {
        // Start and end are quantized from the ORIGINAL boundaries independently;
        // "both" never derives the end from an already-quantized start.
        let original_start = note.start;
        let original_end = note.start + note.duration;
        let mut next_start = original_start;
        let mut next_end = original_end;
        if target != "end" {
            next_start = (original_start + (round(original_start / grid) * grid - original_start) * amount).max(0.0);
        }
        if target != "start" {
            next_end = original_end + (round(original_end / grid) * grid - original_end) * amount;
        }
        note.start = next_start;
        note.duration = (next_end - next_start).max(1.0 / 1024.0);
        if let Some(clip_length) = clip_length {
            if note.start > clip_length - 1.0 / 1024.0 {
                note.start = (clip_length - note.duration.max(1.0 / 1024.0)).max(0.0);
            }
            if note.start + note.duration > clip_length {
                note.duration = (clip_length - note.start).max(1.0 / 1024.0);
            }
        }
    }
    Ok(MidiTransformOutcome {
        notes: result,
        generative: false,
        assumptions: vec![format!("grid {} beats at {}% strength toward {target}", number_text(grid), number_text(round(amount * 100.0)))],
        seed: None,
    })
}

fn swing(notes: &[Note], params: &Params, clip_length: Option<f64>) -> Outcome {
    let grid = finite_param(params, "grid", 1.0 / 1024.0, 8.0, None)?;
    let amount = finite_param(params, "amount", 0.0, 1.0, None)?;
    let epsilon = 1e-6;
    let mut shifted = 0;
    let mut skipped_off_grid = 0;
    let mut clamped = 0;
    let mut result = notes.to_vec();
    for note in &mut result {
        let index = round(note.start / grid);
        if (note.start - index * grid).abs() > epsilon {
            skipped_off_grid += 1;
            continue;
        }
        if index % 2.0 == 1.0 {
            note.start += amount * (grid / 2.0);
            shifted += 1;
        }
        // Swing never pushes a note past the clip end: the real mapper rejects
        // patches beyond the exact clip length.
        if let Some(clip_length) = clip_length {
            if note.start + note.duration > clip_length {
                note.start = (clip_length - note.duration).max(0.0);
                clamped += 1;
            }
        }
    }
    let _ = shifted;
    let mut assumptions = vec![format!(
        "swing shifts notes exactly on odd {}-beat divisions by {}% of a half division",
        number_text(grid),
        number_text(round(amount * 100.0))
    )];
    if skipped_off_grid > 0 {
        assumptions.push(format!("{skipped_off_grid} off-grid note(s) left untouched"));
    }
    if clamped > 0 {
        assumptions.push(format!("{clamped} note(s) clamped to the clip end"));
    }
    Ok(MidiTransformOutcome { notes: result, generative: false, assumptions, seed: None })
}

fn velocity_curve(notes: &[Note], params: &Params) -> Outcome {
    let curve = string_param(params, "curve", &["linear-up", "linear-down", "arch", "exp-up", "exp-down"], None)?;
    let amount = finite_param(params, "amount", 0.0, 1.0, None)?;
    let ordered = stable_note_order(notes);
    let min_start = ordered.first().map(|note| note.start).unwrap_or(0.0);
    let max_start = ordered.last().map(|note| note.start).unwrap_or(min_start);
    let span = max_start - min_start;
    let mut result = notes.to_vec();
    for note in &mut result {
        let t = if span > 0.0 { (note.start - min_start) / span } else { 0.0 };
        let shaped = match curve.as_str() {
            "linear-up" => t,
            "linear-down" => 1.0 - t,
            "arch" => (std::f64::consts::PI * t).sin(),
            "exp-up" => t * t,
            _ => 1.0 - (1.0 - t) * (1.0 - t),
        };
        let multiplier = 1.0 + (shaped - 0.5) * 2.0 * amount * 0.5;
        note.velocity = clamp_velocity(round(note.velocity * multiplier));
    }
    Ok(MidiTransformOutcome {
        notes: result,
        generative: false,
        assumptions: vec![format!("velocity curve {curve} at {}% depth across the clip time span", number_text(round(amount * 100.0)))],
        seed: None,
    })
}

fn humanize_velocity(notes: &[Note], params: &Params) -> Outcome {
    let seed = seed_param(params)?;
    let max_delta = finite_param(params, "maxDelta", 0.0, 64.0, None)?;
    let mut random = seeded_random(&seed);
    let mut result = notes.to_vec();
    for index in stable_order_indices(&result) {
        let delta = round((random.next() * 2.0 - 1.0) * max_delta);
        let note = &mut result[index];
        note.velocity = clamp_velocity(round(note.velocity + delta));
    }
    Ok(MidiTransformOutcome {
        notes: result,
        generative: false,
        seed: Some(seed),
        assumptions: vec![format!("seeded velocity jitter within ±{}", number_text(max_delta))],
    })
}

fn humanize_timing(notes: &[Note], params: &Params, clip_length: Option<f64>) -> Outcome {
    let seed = seed_param(params)?;
    let max_offset = finite_param(params, "maxOffset", 0.0, 0.5, None)?;
    let mut random = seeded_random(&seed);
    let mut result = notes.to_vec();
    for index in stable_order_indices(&result) {
        let offset = (random.next() * 2.0 - 1.0) * max_offset;
        let note = &mut result[index];
        note.start = (note.start + offset).max(0.0);
        if let Some(clip_length) = clip_length {
            if note.start + note.duration > clip_length {
                note.start = (clip_length - note.duration).max(0.0);
            }
        }
    }
    Ok(MidiTransformOutcome {
        notes: result,
        generative: false,
        seed: Some(seed),
        assumptions: vec![format!("seeded timing jitter within ±{} beats, clamped to the clip", number_text(max_offset))],
    })
}

fn legato(notes: &[Note], params: &Params, clip_length: Option<f64>) -> Outcome {
    let gap = finite_param(params, "gap", 0.0, 1.0, Some(0.0))?;
    let ordered = stable_note_order(notes);
    let mut onsets: Vec<f64> = Vec::new();
    for note in &ordered {
        if !onsets.contains(&note.start) {
            onsets.push(note.start);
        }
    }
    onsets.sort_by(|a, b| cmp_f64(*a, *b));
    let mut result = notes.to_vec();
    for note in &mut result {
        let next_onset = onsets.iter().copied().find(|onset| *onset > note.start + 1e-9);
        let reach = next_onset.or(clip_length).unwrap_or(note.start + note.duration) - gap;
        note.duration = (reach - note.start).max(1.0 / 1024.0);
    }
    Ok(MidiTransformOutcome {
        notes: result,
        generative: false,
        assumptions: vec![
            "each note extends to the next onset minus the gap; the final onset reaches the clip end or keeps its extent".to_string()
        ],
        seed: None,
    })
}

fn staccato(notes: &[Note], params: &Params) -> Outcome {
    let factor = finite_param(params, "factor", 0.05, 1.0, None)?;
    let mut result = notes.to_vec();
    for note in &mut result {
        note.duration = (note.duration * factor).max(1.0 / 1024.0);
    }
    Ok(MidiTransformOutcome {
        notes: result,
        generative: false,
        assumptions: vec![format!("durations scaled by {}", number_text(factor))],
        seed: None,
    })
}

fn rotate(notes: &[Note], params: &Params) -> Outcome {
    let steps = integer_param(params, "steps", -512, 512, None)?;
    let mut result = notes.to_vec();
    let ordered = stable_order_indices(&result);
    if ordered.is_empty() {
        return Ok(MidiTransformOutcome { notes: result, generative: false, assumptions: vec!["empty clip".to_string()], seed: None });
    }
    let pitches: Vec<f64> = ordered.iter().map(|index| result[*index].pitch).collect();
    let length = ordered.len() as i64;
    let shift = ((steps % length) + length) % length;
    // Assign distinct cloned occurrences positionally, not through a content key:
    // id-less duplicates (even repeated input object references) are distinct notes.
    for (position, index) in ordered.iter().enumerate() {
        result[*index].pitch = pitches[((position as i64 - shift + length) % length) as usize];
    }
    Ok(MidiTransformOutcome {
        notes: result,
        generative: false,
        assumptions: vec![format!("pitches rotated by {shift} positions in stable note order; rhythm unchanged")],
        seed: None,
    })
}

fn repeat(notes: &[Note], params: &Params) -> Outcome {
    let times = integer_param(params, "times", 2, 8, None)?;
    let decay = finite_param(params, "decay", 0.0, 1.0, Some(0.0))?;
    let mut result: Vec<Note> = Vec::new();
    for note in stable_note_order(notes) {
        let slice = note.duration / times as f64;
        for index in 0..times {
            let velocity = clamp_velocity(round(note.velocity * (1.0 - decay).powf(index as f64)));
            result.push(Note { start: note.start + index as f64 * slice, duration: slice, velocity, id: Maybe::Absent, ..note.clone() });
        }
    }
    let decay_note =
        if decay > 0.0 { format!(" with {}% velocity decay per step", number_text(round(decay * 100.0))) } else { String::new() };
    Ok(MidiTransformOutcome {
        notes: result,
        generative: true,
        assumptions: vec![format!("each note is subdivided into {times} equal parts{decay_note}; original notes are replaced")],
        seed: None,
    })
}

fn ratchet(notes: &[Note], params: &Params) -> Outcome {
    let seed = seed_param(params)?;
    let subdivisions = integer_param(params, "subdivisions", 2, 16, None)?;
    let probability = finite_param(params, "probability", 0.0, 1.0, Some(1.0))?;
    let mut random = seeded_random(&seed);
    let mut result: Vec<Note> = Vec::new();
    for note in stable_note_order(notes) {
        let slice = note.duration / subdivisions as f64;
        for index in 0..subdivisions {
            if random.next() > probability {
                continue;
            }
            result.push(Note { start: note.start + index as f64 * slice, duration: slice, id: Maybe::Absent, ..note.clone() });
        }
    }
    Ok(MidiTransformOutcome {
        notes: result,
        generative: true,
        seed: Some(seed),
        assumptions: vec![format!(
            "each note is subdivided into {subdivisions} ratchets kept at probability {} under its seed; original notes are replaced",
            number_text(probability)
        )],
    })
}

/// Notes grouped by onset (`Math.round(start / epsilon)`), in first-seen order as a Map keeps them.
fn onset_groups(notes: &[Note], epsilon: f64) -> Vec<(f64, Vec<usize>)> {
    let mut groups: Vec<(f64, Vec<usize>)> = Vec::new();
    let mut positions: HashMap<u64, usize> = HashMap::new();
    for (index, note) in notes.iter().enumerate() {
        let key = round(note.start / epsilon);
        let key = if key == 0.0 { 0.0 } else { key };
        match positions.get(&key.to_bits()) {
            Some(position) => groups[*position].1.push(index),
            None => {
                positions.insert(key.to_bits(), groups.len());
                groups.push((key, vec![index]));
            }
        }
    }
    groups
}

fn chord_voicing(notes: &[Note], params: &Params) -> Outcome {
    let strategy = string_param(params, "strategy", &["close", "open", "drop2"], None)?;
    let epsilon = 1e-6;
    let mut result = notes.to_vec();
    let groups = onset_groups(&result, epsilon);
    let mut voiced = 0;
    for (_, group) in &groups {
        if group.len() < 3 {
            continue;
        }
        let mut by_pitch = group.clone();
        by_pitch.sort_by(|a, b| cmp_f64(result[*a].pitch, result[*b].pitch));
        if strategy == "close" {
            let lowest = result[by_pitch[0]].pitch;
            for index in &by_pitch[1..] {
                while result[*index].pitch - lowest > 12.0 && result[*index].pitch - 12.0 >= 0.0 {
                    result[*index].pitch -= 12.0;
                    voiced += 1;
                }
            }
        } else if strategy == "open" {
            let mut position = by_pitch.len() as i64 - 2;
            while position >= 0 {
                let note = &mut result[by_pitch[position as usize]];
                if note.pitch + 12.0 <= 127.0 {
                    note.pitch += 12.0;
                    voiced += 1;
                }
                position -= 2;
            }
        } else {
            let second_highest = &mut result[by_pitch[by_pitch.len() - 2]];
            if second_highest.pitch - 12.0 >= 0.0 {
                second_highest.pitch -= 12.0;
                voiced += 1;
            }
        }
    }
    Ok(MidiTransformOutcome {
        notes: result,
        generative: false,
        assumptions: vec![format!(
            "{strategy} voicing applied to onset groups of 3+ notes within the MIDI pitch range; {voiced} voice(s) moved by an octave"
        )],
        seed: None,
    })
}

/// `array.slice(1, -1)`.
fn inner_slice<T: Clone>(items: &[T]) -> Vec<T> {
    if items.len() > 2 {
        items[1..items.len() - 1].to_vec()
    } else {
        Vec::new()
    }
}

fn arpeggiate(notes: &[Note], params: &Params) -> Outcome {
    let pattern = string_param(params, "pattern", &["up", "down", "updown", "downup", "random"], None)?;
    let seed = if pattern == "random" {
        Some(seed_param(params)?)
    } else {
        match params.get("seed") {
            Some(Value::String(seed)) if (1..=128).contains(&utf16_len(seed)) => Some(seed.clone()),
            _ => None,
        }
    };
    let rate = finite_param(params, "rate", 1.0 / 1024.0, 4.0, Some(0.25))?;
    let mut random = seed.as_deref().map(seeded_random);
    let epsilon = 1e-6;
    let groups = onset_groups(notes, epsilon);
    let group_span = |key: f64, group: &[usize]| -> (f64, f64) {
        let start = key * epsilon;
        let end = group.iter().map(|index| notes[*index].start + notes[*index].duration).fold(f64::NEG_INFINITY, f64::max);
        (start, end - start)
    };
    let mut result: Vec<Note> = Vec::new();
    // Bound the prospective output before generating anything: rate may be as
    // small as 1/1024 beats while note durations are unbounded, so
    // floor(span / rate) per onset group can otherwise attempt effectively
    // unbounded note generation and hang or OOM the host inside one tool call.
    let mut prospective = 0.0;
    for (key, group) in &groups {
        if group.len() < 2 {
            prospective += group.len() as f64;
            continue;
        }
        let (_, span) = group_span(*key, group);
        prospective += (span / rate).floor().max(1.0);
        if prospective > MIDI_TRANSFORM_MAX_GENERATED_NOTES as f64 {
            return Err(range(format!(
                "arpeggiate would generate more than the bounded {MIDI_TRANSFORM_MAX_GENERATED_NOTES}-note limit at rate {}; increase the rate or shorten the onset-group span",
                number_text(rate)
            )));
        }
    }
    let mut sorted_groups = groups.clone();
    sorted_groups.sort_by(|a, b| cmp_f64(a.0, b.0));
    for (key, group) in &sorted_groups {
        if group.len() < 2 {
            result.extend(group.iter().map(|index| notes[*index].clone()));
            continue;
        }
        let (start, duration) = group_span(*key, group);
        let mut pitches: Vec<f64> = group.iter().map(|index| notes[*index].pitch).collect();
        pitches.sort_by(|a, b| cmp_f64(*a, *b));
        pitches.dedup();
        let reversed: Vec<f64> = pitches.iter().rev().copied().collect();
        let order: Vec<f64> = match pattern.as_str() {
            "up" => pitches.clone(),
            "down" => reversed,
            "updown" => [pitches.clone(), inner_slice(&reversed)].concat(),
            "downup" => [reversed, inner_slice(&pitches)].concat(),
            _ => {
                let mut order = pitches.clone();
                let random = random.as_mut().expect("random patterns are seeded");
                for index in (1..order.len()).rev() {
                    let swap = (random.next() * (index as f64 + 1.0)).floor() as usize;
                    order.swap(index, swap);
                }
                order
            }
        };
        let count = (duration / rate).floor().max(1.0) as usize;
        let template = &notes[group[0]];
        for index in 0..count {
            let step_start = start + index as f64 * rate;
            result.push(Note {
                pitch: order[index % order.len()],
                start: step_start,
                duration: rate.min(start + duration - step_start),
                id: Maybe::Absent,
                ..template.clone()
            });
        }
    }
    Ok(MidiTransformOutcome {
        notes: result,
        generative: true,
        seed,
        assumptions: vec![format!("onset groups of 2+ notes become {pattern} arpeggios at {}-beat steps within the original group span; original chord notes are replaced", number_text(rate))],
    })
}

fn seeded_variation(notes: &[Note], params: &Params, clip_length: Option<f64>) -> Outcome {
    let seed = seed_param(params)?;
    let velocity_max = finite_param(params, "velocityMax", 0.0, 32.0, Some(8.0))?;
    let timing_max = finite_param(params, "timingMax", 0.0, 0.25, Some(0.0))?;
    let probability_depth = finite_param(params, "probabilityDepth", 0.0, 1.0, Some(0.0))?;
    let mut random = seeded_random(&seed);
    let mut result = notes.to_vec();
    for index in stable_order_indices(&result) {
        let velocity_delta = round((random.next() * 2.0 - 1.0) * velocity_max);
        let note = &mut result[index];
        note.velocity = clamp_velocity(round(note.velocity + velocity_delta));
        if timing_max > 0.0 {
            note.start = (note.start + (random.next() * 2.0 - 1.0) * timing_max).max(0.0);
            if let Some(clip_length) = clip_length {
                if note.start + note.duration > clip_length {
                    note.start = (clip_length - note.duration).max(0.0);
                }
            }
        }
        if probability_depth > 0.0 {
            note.probability = Maybe::Value(round((1.0 - random.next() * probability_depth) * 1000.0) / 1000.0);
        }
    }
    let timing_note = if timing_max > 0.0 { format!(", timing ±{} beats", number_text(timing_max)) } else { String::new() };
    let probability_note = if probability_depth > 0.0 {
        format!(", probability reduced up to {}%", number_text(round(probability_depth * 100.0)))
    } else {
        String::new()
    };
    Ok(MidiTransformOutcome {
        notes: result,
        generative: false,
        seed: Some(seed),
        assumptions: vec![format!("seeded variation: velocity ±{}{timing_note}{probability_note}", number_text(velocity_max))],
    })
}

/* -------------------------------------------------------------------------
 * Deterministic generative primitives (issue #47). Every generator is a pure
 * function of its parameters (plus an explicit seed for any stochastic gate)
 * and ignores the input note set: generation replaces the scoped content, and
 * the preview diff discloses exactly what is added and deleted. No kit
 * mapping, scale, or chord quality is ever invented: musical context is
 * explicit in the parameters or resolved host-side from the Set and disclosed
 * in the preview assumptions.
 * ------------------------------------------------------------------------- */

const NOTE_NAMES_PC: &[&str] = &["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"];

fn note_name(pitch_class: i64) -> &'static str {
    NOTE_NAMES_PC.get(pitch_class as usize).copied().unwrap_or("undefined")
}

/// Generated notes carry Live's documented new-note defaults (channel 1, mute
/// off, probability 1, no velocity deviation, release velocity 64) so the
/// previewed diff matches the post-apply canonical read byte-for-byte.
fn new_note(pitch: f64, start: f64, duration: f64, velocity: f64) -> Note {
    Note {
        pitch,
        start,
        duration,
        velocity,
        channel: 1.0,
        id: Maybe::Absent,
        mute: Maybe::Value(false),
        probability: Maybe::Value(1.0),
        velocity_deviation: Maybe::Value(0.0),
        release_velocity: Maybe::Value(64.0),
        extra: Map::new(),
    }
}

/// Bjorklund's algorithm: `pulses` distributed over `steps` as evenly as
/// possible. The raw bucket construction is normalized so the first pulse
/// lands on step 0 (the canonical representation of each rhythm).
pub fn bjorklund(pulses: i64, steps: i64) -> Result<Vec<bool>, MidiTransformError> {
    if pulses < 0 || steps < 1 || steps > 64 || pulses > steps {
        return Err(range("euclidean pulses/steps are invalid"));
    }
    let steps_count = steps as usize;
    if pulses == 0 {
        return Ok(vec![false; steps_count]);
    }
    if pulses == steps {
        return Ok(vec![true; steps_count]);
    }
    let mut pattern: Vec<bool> = Vec::new();
    let mut counts: Vec<i64> = Vec::new();
    let mut remainders: Vec<i64> = vec![pulses];
    let mut divisor = steps - pulses;
    let mut level: usize = 0;
    loop {
        counts.push(divisor / remainders[level]);
        remainders.push(divisor % remainders[level]);
        divisor = remainders[level];
        level += 1;
        if remainders[level] <= 1 {
            break;
        }
    }
    counts.push(divisor);
    fn build(level: i64, counts: &[i64], remainders: &[i64], pattern: &mut Vec<bool>) {
        if level == -1 {
            pattern.push(false);
            return;
        }
        if level == -2 {
            pattern.push(true);
            return;
        }
        for _ in 0..counts[level as usize] {
            build(level - 1, counts, remainders, pattern);
        }
        if remainders[level as usize] != 0 {
            build(level - 2, counts, remainders, pattern);
        }
    }
    build(level as i64, &counts, &remainders, &mut pattern);
    let raw: Vec<bool> = pattern.into_iter().take(steps_count).collect();
    match raw.iter().position(|hit| *hit) {
        Some(first_hit) if first_hit > 0 => Ok([&raw[first_hit..], &raw[..first_hit]].concat()),
        _ => Ok(raw),
    }
}

fn euclidean_rhythm(_notes: &[Note], params: &Params) -> Outcome {
    let pulses = integer_param(params, "pulses", 1, 64, None)?;
    let steps = integer_param(params, "steps", 1, 64, None)?;
    if pulses > steps {
        return Err(range("pulses must not exceed steps"));
    }
    let rotation = integer_param(params, "rotation", -64, 64, Some(0))?;
    let pitch = integer_param(params, "pitch", 0, 127, None)?;
    let velocity = integer_param(params, "velocity", 1, 127, Some(100))?;
    let step_length = finite_param(params, "stepLength", 1.0 / 1024.0, 16.0, Some(0.25))?;
    let note_length = finite_param(params, "noteLength", 1.0 / 1024.0, 64.0, Some((0.9 * step_length).min(step_length)))?;
    let bars = integer_param(params, "bars", 1, 64, Some(1))?;
    if pulses * bars > MIDI_TRANSFORM_MAX_GENERATED_NOTES as i64 {
        return Err(range(format!("euclidean would exceed the bounded {MIDI_TRANSFORM_MAX_GENERATED_NOTES}-note limit")));
    }
    let pattern = bjorklund(pulses, steps)?;
    let shift = ((rotation % steps) + steps) % steps;
    let mut result: Vec<Note> = Vec::new();
    for bar in 0..bars {
        for step in 0..steps {
            if !pattern[((step + shift) % steps) as usize] {
                continue;
            }
            result.push(new_note(pitch as f64, (bar * steps + step) as f64 * step_length, note_length, velocity as f64));
        }
    }
    let rotated = if shift != 0 { format!(" rotated {shift} step(s)") } else { String::new() };
    Ok(MidiTransformOutcome {
        notes: result,
        generative: true,
        assumptions: vec![format!(
            "Euclidean {pulses}-in-{steps}{rotated} on pitch {pitch} ({}{}), {bars} bar(s) at {}-beat steps; input notes are replaced",
            note_name(pitch % 12),
            pitch / 12 - 1,
            number_text(step_length)
        )],
        seed: None,
    })
}

/* ------------------------------- chords ---------------------------------- */

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChordSpec {
    /// Root pitch class 0-11.
    pub root_pc: i64,
    /// Pitch-class intervals above the root, ascending, starting at 0.
    pub intervals: Vec<i64>,
    /// Realized human-readable chord name (never invented: derived from intervals).
    pub name: String,
}

fn symbol_quality(quality: &str) -> Option<&'static [i64]> {
    Some(match quality {
        "" | "major" => &[0, 4, 7],
        "m" | "min" | "minor" => &[0, 3, 7],
        "maj7" | "major7" => &[0, 4, 7, 11],
        "7" => &[0, 4, 7, 10],
        "m7" | "min7" | "minor7" => &[0, 3, 7, 10],
        "m7b5" => &[0, 3, 6, 10],
        "dim" => &[0, 3, 6],
        "dim7" => &[0, 3, 6, 9],
        "aug" => &[0, 4, 8],
        "sus4" => &[0, 5, 7],
        _ => return None,
    })
}

fn join_numbers(values: &[i64], separator: &str) -> String {
    values.iter().map(|value| value.to_string()).collect::<Vec<_>>().join(separator)
}

fn quality_name(intervals: &[i64]) -> String {
    let key = join_numbers(intervals, ",");
    match key.as_str() {
        "0,4,7" => "",
        "0,3,7" => "m",
        "0,3,6" => "dim",
        "0,4,8" => "aug",
        "0,5,7" => "sus4",
        "0,4,7,11" => "maj7",
        "0,4,7,10" => "7",
        "0,3,7,10" => "m7",
        "0,3,6,10" => "m7b5",
        "0,3,6,9" => "dim7",
        "0,3,7,11" => "m(maj7)",
        _ => return format!("({{{}}})", join_numbers(intervals, "+")),
    }
    .to_string()
}

/// Parse an explicit chord symbol like "Dm7", "F#", "Bb7", "Gaug".
pub fn parse_chord_symbol(symbol: &str) -> Result<ChordSpec, MidiTransformError> {
    static SYMBOL: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new("(?i)^(A|B|C|D|E|F|G)(#|b)?(maj7|major7|major|m7b5|min7|minor7|m7|min|minor|m|dim7|dim|aug|sus4|7)?$").unwrap()
    });
    let Some(found) = SYMBOL.captures(trim(symbol)) else {
        return Err(range(format!(
            "unsupported chord symbol \"{symbol}\"; expected a root A-G with optional #/b and a documented quality ({})",
            ["", "m", "maj7", "7", "m7", "m7b5", "dim", "dim7", "aug", "sus4"].join("/")
        )));
    };
    let letter = found[1].to_uppercase();
    let accidental = found.get(2).map(|accidental| accidental.as_str()).unwrap_or("");
    let base = match letter.as_str() {
        "C" => 0,
        "D" => 2,
        "E" => 4,
        "F" => 5,
        "G" => 7,
        "A" => 9,
        _ => 11,
    };
    let root_pc = (base
        + if accidental == "#" {
            1
        } else if accidental == "b" {
            -1
        } else {
            0
        }
        + 12)
        % 12;
    let quality_key = found.get(3).map(|quality| quality.as_str()).unwrap_or("").to_lowercase();
    let intervals = symbol_quality(&quality_key).or_else(|| symbol_quality("")).unwrap_or(&[]).to_vec();
    let name = format!("{}{}", note_name(root_pc), quality_name(&intervals));
    Ok(ChordSpec { root_pc, intervals, name })
}

fn roman_degree(numeral: &str) -> Option<i64> {
    Some(match numeral {
        "i" => 0,
        "ii" => 1,
        "iii" => 2,
        "iv" => 3,
        "v" => 4,
        "vi" => 5,
        "vii" => 6,
        _ => return None,
    })
}

static ROMAN: LazyLock<Regex> = LazyLock::new(|| Regex::new("(?i)^(vii|vi|iv|v|iii|ii|i)(°|dim)?(7)?$").unwrap());

/// Parse a roman numeral (i-vii, optional "7", optional "°"/"dim") against a scale.
pub fn parse_roman_numeral(numeral: &str, root: i64, scale: &str) -> Result<ChordSpec, MidiTransformError> {
    let Some(found) = ROMAN.captures(trim(numeral)) else {
        return Err(range(format!("unsupported roman numeral \"{numeral}\"; expected i-vii with optional \"°\" and \"7\"")));
    };
    let degree_index = roman_degree(&found[1].to_lowercase()).unwrap_or(0);
    let intervals_of_scale = match scale_intervals(scale) {
        Some(intervals) if intervals.len() == 7 => intervals,
        _ => return Err(range(format!("roman numerals require a 7-tone scale, got \"{scale}\""))),
    };
    let degree_pc = |degree: i64| intervals_of_scale[(degree % 7) as usize] + 12 * (degree / 7);
    let diminished = found.get(2).is_some();
    let seventh = found.get(3).is_some();
    let root_pc = (root + degree_pc(degree_index)) % 12;
    let third = degree_pc(degree_index + 2) - degree_pc(degree_index);
    let fifth = degree_pc(degree_index + 4) - degree_pc(degree_index);
    let mut intervals: Vec<i64> = vec![0, if diminished { 3 } else { third }, if diminished { 6 } else { fifth }];
    if seventh {
        let diatonic_seventh = degree_pc(degree_index + 6) - degree_pc(degree_index);
        intervals.push(if diminished { 10 } else { diatonic_seventh });
    }
    let name = format!("{}{}", note_name(root_pc), quality_name(&intervals));
    Ok(ChordSpec { root_pc, intervals, name })
}

/// A chord list parsed: the chords, and where they came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChordList {
    pub chords: Vec<ChordSpec>,
    pub source: String,
}

/// Parse a chord list: explicit symbols, or roman numerals against root/scale.
pub fn parse_chord_list(chords: &[String], root: Option<i64>, scale: Option<&str>) -> Result<ChordList, MidiTransformError> {
    let looks_roman = |token: &str| ROMAN.is_match(trim(token));
    if chords.is_empty() || chords.len() > 32 {
        return Err(range("chords must name 1-32 entries"));
    }
    if chords.iter().all(|token| looks_roman(token)) {
        let (Some(root), Some(scale)) = (root, scale) else {
            return Err(range("roman-numeral chords require an explicit or Set-discovered key (root) and mode (scale)"));
        };
        let parsed = chords.iter().map(|token| parse_roman_numeral(token, root, scale)).collect::<Result<Vec<_>, _>>()?;
        return Ok(ChordList { chords: parsed, source: format!("roman numerals in {} {scale}", note_name(root)) });
    }
    if chords.iter().any(|token| looks_roman(token)) {
        return Err(range("chords must not mix roman numerals with explicit chord symbols"));
    }
    let parsed = chords.iter().map(|token| parse_chord_symbol(token)).collect::<Result<Vec<_>, _>>()?;
    Ok(ChordList { chords: parsed, source: "explicit chord symbols".to_string() })
}

fn string_array_param(params: &Params, name: &str) -> Result<Vec<String>, MidiTransformError> {
    let valid = match params.get(name) {
        Some(Value::Array(items)) if !items.is_empty() && items.len() <= 32 => items
            .iter()
            .map(|entry| entry.as_str().filter(|text| (1..=32).contains(&utf16_len(text))).map(str::to_string))
            .collect::<Option<Vec<String>>>(),
        _ => None,
    };
    valid.ok_or_else(|| range(format!("{name} must be an array of 1-32 non-empty strings")))
}

fn mapping_param(params: &Params, name: &str, roles: &[&str]) -> Result<HashMap<String, i64>, MidiTransformError> {
    let Some(value) = params.get(name) else { return Ok(HashMap::new()) };
    let Value::Object(entries) = value else {
        return Err(range(format!("{name} must be a flat role-to-pitch object")));
    };
    if entries.len() > 32 {
        return Err(range(format!("{name} must not exceed 32 roles")));
    }
    let mut mapping: HashMap<String, i64> = HashMap::new();
    for (role, pitch) in entries {
        if !roles.contains(&role.as_str()) {
            return Err(range(format!("{name} role \"{role}\" is not one of {}", roles.join(", "))));
        }
        let pitch = pitch.as_f64().filter(|pitch| pitch.fract() == 0.0 && (0.0..=127.0).contains(pitch));
        let Some(pitch) = pitch else {
            return Err(range(format!("{name}.{role} must be an integer MIDI pitch in 0..127")));
        };
        mapping.insert(role.clone(), pitch as i64);
    }
    Ok(mapping)
}

/// Voiced chord: one pitch per chord tone, ascending.
type VoicedChord = Vec<i64>;

fn style_voicing(chord: &ChordSpec, bass_pitch: i64, style: &str) -> VoicedChord {
    let close: VoicedChord = chord.intervals.iter().map(|interval| bass_pitch + interval).collect();
    if style == "close" {
        return close;
    }
    if style == "drop2" {
        if close.len() < 3 {
            return close;
        }
        let mut voiced = close.clone();
        let second_highest = voiced[voiced.len() - 2];
        if second_highest - 12 >= 0 {
            let position = voiced.len() - 2;
            voiced[position] = second_highest - 12;
        }
        voiced.sort();
        return voiced;
    }
    // spread: alternate chord tones (2nd, 4th, ...) rise an octave
    let mut spread: VoicedChord =
        close.iter().enumerate().map(|(index, pitch)| if index % 2 == 1 && pitch + 12 <= 127 { pitch + 12 } else { *pitch }).collect();
    spread.sort();
    spread
}

/// Every inversion/octave placement of the chord near a register, for voice leading.
fn voicing_candidates(chord: &ChordSpec, style: &str) -> Vec<VoicedChord> {
    let mut seen: HashSet<VoicedChord> = HashSet::new();
    let mut candidates: Vec<VoicedChord> = Vec::new();
    let tones = chord.intervals.len();
    for inversion in 0..tones {
        for octave in 0..=10 {
            let bass_pc = chord.intervals[inversion];
            let bass_pitch = 12 * octave + ((chord.root_pc + bass_pc) % 12);
            let rotated: Vec<i64> = chord.intervals[inversion..]
                .iter()
                .map(|interval| interval - bass_pc)
                .chain(chord.intervals[..inversion].iter().map(|interval| interval + 12 - bass_pc))
                .collect();
            let voiced =
                style_voicing(&ChordSpec { root_pc: chord.root_pc, intervals: rotated, name: chord.name.clone() }, bass_pitch, style);
            if voiced.iter().any(|pitch| *pitch < 0 || *pitch > 127) {
                continue;
            }
            if !seen.insert(voiced.clone()) {
                continue;
            }
            candidates.push(voiced);
        }
    }
    candidates
}

fn movement_cost(from: &[i64], to: &[i64]) -> i64 {
    from.iter().zip(to).map(|(a, b)| (a - b).abs()).sum()
}

fn chord_names(chords: &[ChordSpec]) -> String {
    chords.iter().map(|chord| chord.name.as_str()).collect::<Vec<_>>().join(" - ")
}

fn optional_root(params: &Params) -> Result<Option<i64>, MidiTransformError> {
    if params.contains_key("root") {
        Ok(Some(integer_param(params, "root", 0, 11, None)?))
    } else {
        Ok(None)
    }
}

fn optional_scale(params: &Params) -> Result<Option<String>, MidiTransformError> {
    if params.contains_key("scale") {
        Ok(Some(string_param(params, "scale", &scale_names(), None)?))
    } else {
        Ok(None)
    }
}

fn chord_progression(_notes: &[Note], params: &Params) -> Outcome {
    // `chords` takes chord symbols or roman numerals, as bassline's does; `numerals` and `symbols` still work.
    let given: Vec<&str> = ["chords", "numerals", "symbols"].into_iter().filter(|name| params.contains_key(*name)).collect();
    let [name] = given[..] else {
        return Err(range("exactly one of chords, numerals or symbols is required"));
    };
    let tokens = string_array_param(params, name)?;
    let root = optional_root(params)?;
    let scale = optional_scale(params)?;
    let ChordList { chords, source } = if name == "symbols" {
        ChordList {
            chords: tokens.iter().map(|token| parse_chord_symbol(token)).collect::<Result<Vec<_>, _>>()?,
            source: "explicit chord symbols".to_string(),
        }
    } else {
        parse_chord_list(&tokens, root, scale.as_deref())?
    };
    let voicing_style = string_param(params, "voicing", &["close", "drop2", "spread"], Some("close"))?;
    let voice_leading = integer_param(params, "voiceLeading", 0, 1, Some(1))? == 1;
    let chord_duration = finite_param(params, "chordDuration", 1.0 / 1024.0, 1024.0, Some(4.0))?;
    let start_beat = finite_param(params, "startBeat", 0.0, 1000000.0, Some(0.0))?;
    let velocity = integer_param(params, "velocity", 1, 127, Some(80))?;
    let octave = integer_param(params, "octave", 0, 8, Some(4))?;
    let mut voiced: Vec<VoicedChord> = Vec::new();
    let mut previous: Option<VoicedChord> = None;
    for chord in &chords {
        let placed = match &previous {
            Some(previous) if voice_leading => {
                let candidates = voicing_candidates(chord, &voicing_style);
                if candidates.is_empty() {
                    return Err(range(format!("no voicing of {} fits the MIDI pitch range", chord.name)));
                }
                let mean = |voiced_chord: &[i64]| voiced_chord.iter().sum::<i64>() as f64 / voiced_chord.len() as f64;
                let mut best = candidates[0].clone();
                for candidate in &candidates[1..] {
                    let cost = movement_cost(previous, candidate);
                    let best_cost = movement_cost(previous, &best);
                    if cost != best_cost {
                        if cost < best_cost {
                            best = candidate.clone();
                        }
                        continue;
                    }
                    if (mean(candidate) - mean(previous)).abs() < (mean(&best) - mean(previous)).abs() {
                        best = candidate.clone();
                    }
                }
                best
            }
            _ => {
                let placed = style_voicing(chord, 12 * (octave + 1) + chord.root_pc, &voicing_style);
                if placed.iter().any(|pitch| *pitch < 0 || *pitch > 127) {
                    return Err(range("requested chord octave and voicing exceed the MIDI pitch range"));
                }
                placed
            }
        };
        voiced.push(placed.clone());
        previous = Some(placed);
    }
    let mut result: Vec<Note> = Vec::new();
    for (index, chord) in voiced.iter().enumerate() {
        for pitch in chord {
            result.push(new_note(*pitch as f64, start_beat + index as f64 * chord_duration, chord_duration, velocity as f64));
        }
    }
    let movement: i64 = voiced.windows(2).map(|pair| movement_cost(&pair[0], &pair[1])).sum();
    let leading = if voice_leading {
        format!(" with minimal voice movement ({movement} total semitone steps)")
    } else {
        " (voice leading off)".to_string()
    };
    Ok(MidiTransformOutcome {
        notes: result,
        generative: true,
        assumptions: vec![
            format!("{} chord(s) from {source}: {}", chords.len(), chord_names(&chords)),
            format!(
                "{voicing_style} voicing{leading} at octave {octave}, {} beat(s) per chord; input notes are replaced",
                number_text(chord_duration)
            ),
        ],
        seed: None,
    })
}

/* ------------------------------- drums ------------------------------------ */

pub const DRUM_ROLES: &[&str] = &["kick", "snare", "closedHat", "openHat", "clap", "ride", "crash", "lowTom", "midTom", "highTom"];

struct DrumHit {
    role: &'static str,
    step16: i64,
    velocity: i64,
    optional: bool,
}

const fn hit(role: &'static str, step16: i64, velocity: i64) -> DrumHit {
    DrumHit { role, step16, velocity, optional: false }
}

const fn optional(role: &'static str, step16: i64, velocity: i64) -> DrumHit {
    DrumHit { role, step16, velocity, optional: true }
}

const DRUM_STYLES: &[(&str, &[DrumHit])] = &[
    (
        "four-on-the-floor",
        &[
            hit("kick", 0, 105),
            hit("kick", 4, 105),
            hit("kick", 8, 105),
            hit("kick", 12, 105),
            hit("snare", 4, 100),
            hit("snare", 12, 100),
            hit("closedHat", 0, 80),
            hit("closedHat", 2, 80),
            hit("closedHat", 4, 80),
            hit("closedHat", 6, 80),
            hit("closedHat", 8, 80),
            hit("closedHat", 10, 80),
            hit("closedHat", 12, 80),
            hit("closedHat", 14, 80),
            optional("closedHat", 1, 65),
            optional("closedHat", 3, 65),
            optional("closedHat", 5, 65),
            optional("closedHat", 7, 65),
            optional("closedHat", 9, 65),
            optional("closedHat", 11, 65),
            optional("closedHat", 13, 65),
            optional("closedHat", 15, 65),
            optional("openHat", 14, 85),
            optional("crash", 0, 90),
        ],
    ),
    (
        "backbeat",
        &[
            hit("kick", 0, 105),
            hit("kick", 8, 105),
            optional("kick", 10, 95),
            hit("snare", 4, 105),
            hit("snare", 12, 105),
            hit("closedHat", 0, 78),
            hit("closedHat", 2, 78),
            hit("closedHat", 4, 78),
            hit("closedHat", 6, 78),
            hit("closedHat", 8, 78),
            hit("closedHat", 10, 78),
            hit("closedHat", 12, 78),
            hit("closedHat", 14, 78),
            optional("crash", 0, 88),
        ],
    ),
    (
        "breakbeat",
        &[
            hit("kick", 0, 105),
            hit("kick", 7, 100),
            hit("kick", 10, 100),
            hit("snare", 4, 105),
            hit("snare", 12, 105),
            optional("snare", 15, 90),
            hit("closedHat", 0, 82),
            hit("closedHat", 2, 82),
            hit("closedHat", 4, 82),
            hit("closedHat", 6, 82),
            hit("closedHat", 8, 82),
            hit("closedHat", 10, 82),
            hit("closedHat", 12, 82),
            hit("closedHat", 14, 82),
            optional("closedHat", 3, 70),
            optional("closedHat", 11, 70),
            optional("openHat", 6, 80),
        ],
    ),
    (
        "trap-hats",
        &[
            hit("kick", 0, 108),
            hit("kick", 10, 100),
            hit("snare", 8, 105),
            hit("closedHat", 0, 85),
            hit("closedHat", 1, 60),
            hit("closedHat", 2, 70),
            hit("closedHat", 3, 60),
            hit("closedHat", 4, 80),
            hit("closedHat", 5, 60),
            hit("closedHat", 6, 70),
            hit("closedHat", 7, 65),
            hit("closedHat", 8, 85),
            hit("closedHat", 9, 60),
            hit("closedHat", 10, 70),
            hit("closedHat", 11, 60),
            hit("closedHat", 12, 80),
            hit("closedHat", 13, 65),
            hit("closedHat", 14, 70),
            hit("closedHat", 15, 75),
            optional("closedHat", 3, 95),
            optional("closedHat", 7, 95),
            optional("closedHat", 11, 95),
            optional("closedHat", 15, 95),
            optional("openHat", 15, 80),
        ],
    ),
];

fn drum_pattern(_notes: &[Note], params: &Params) -> Outcome {
    let style_names: Vec<&str> = DRUM_STYLES.iter().map(|(name, _)| *name).collect();
    let style = string_param(params, "style", &style_names, None)?;
    let bars = integer_param(params, "bars", 1, 8, Some(1))?;
    let grid_resolution = integer_param(params, "gridResolution", 8, 32, Some(16))?;
    if ![8, 16, 32].contains(&grid_resolution) {
        return Err(range("gridResolution must be 8, 16, or 32 steps per bar"));
    }
    let density = finite_param(params, "density", 0.0, 1.0, Some(1.0))?;
    let seed = if !params.contains_key("seed") && density >= 1.0 { None } else { Some(seed_param(params)?) };
    let bar_length = finite_param(params, "barLength", 1.0, 64.0, Some(4.0))?;
    let mapping = mapping_param(params, "mapping", DRUM_ROLES)?;
    let mut random = seed.as_deref().map(seeded_random);
    let template = DRUM_STYLES.iter().find(|(name, _)| *name == style).map(|(_, hits)| *hits).unwrap_or(&[]);
    let step_beats = bar_length / grid_resolution as f64;
    let mut missing: HashSet<&'static str> = HashSet::new();
    let mut dropped_coarse = 0;
    let mut gated_out = 0;
    let mut result: Vec<Note> = Vec::new();
    for bar in 0..bars {
        for hit in template {
            if grid_resolution == 8 && hit.step16 % 2 != 0 {
                dropped_coarse += 1;
                continue;
            }
            let step = hit.step16 as f64 * (grid_resolution as f64 / 16.0);
            if hit.optional && density < 1.0 && random.as_mut().map(|random| random.next() >= density).unwrap_or(false) {
                gated_out += 1;
                continue;
            }
            let Some(pitch) = mapping.get(hit.role) else {
                missing.insert(hit.role);
                continue;
            };
            result.push(new_note(
                *pitch as f64,
                bar as f64 * bar_length + step * step_beats,
                step_beats.min(bar_length - step * step_beats),
                hit.velocity as f64,
            ));
        }
    }
    let mut assumptions = vec![format!(
        "{style} template over {bars} bar(s) at {grid_resolution} steps/bar ({} beats/step); input notes are replaced",
        number_text(step_beats)
    )];
    if dropped_coarse > 0 {
        assumptions.push(format!("{dropped_coarse} hit(s) dropped because they fall between {grid_resolution}-step grid positions"));
    }
    if gated_out > 0 {
        assumptions.push(format!("{gated_out} optional hit(s) gated out at density {} under its seed", number_text(density)));
    }
    if !missing.is_empty() {
        let mut roles: Vec<&str> = missing.into_iter().collect();
        roles.sort_by(|a, b| js_str_cmp(a, b));
        assumptions
            .push(format!("no pitch mapping for role(s) {}; those hits were omitted (kit mapping is never invented)", roles.join(", ")));
    }
    Ok(MidiTransformOutcome { notes: result, generative: true, assumptions, seed })
}

/* ------------------------------ bassline ---------------------------------- */

fn bassline(_notes: &[Note], params: &Params) -> Outcome {
    let pattern = string_param(params, "pattern", &["octave", "walking", "arpeggiated"], None)?;
    let tokens = string_array_param(params, "chords")?;
    let ChordList { chords, source } = parse_chord_list(&tokens, optional_root(params)?, optional_scale(params)?.as_deref())?;
    let chord_duration = finite_param(params, "chordDuration", 1.0 / 1024.0, 1024.0, Some(4.0))?;
    let step_beats = finite_param(params, "stepBeats", 1.0 / 1024.0, 16.0, Some(1.0))?;
    let velocity = integer_param(params, "velocity", 1, 127, Some(85))?;
    let octave = integer_param(params, "octave", 0, 6, Some(2))?;
    let start_beat = finite_param(params, "startBeat", 0.0, 1000000.0, Some(0.0))?;
    let steps = (chord_duration / step_beats).ceil().max(1.0);
    if steps * chords.len() as f64 > MIDI_TRANSFORM_MAX_GENERATED_NOTES as f64 {
        return Err(range(format!("bassline would exceed the bounded {MIDI_TRANSFORM_MAX_GENERATED_NOTES}-note limit; increase stepBeats or shorten the progression")));
    }
    let steps = steps as i64;
    let mut result: Vec<Note> = Vec::new();
    for (chord_index, chord) in chords.iter().enumerate() {
        let root_pitch = 12 * (octave + 1) + chord.root_pc;
        let next_root = if chord_index + 1 < chords.len() { 12 * (octave + 1) + chords[chord_index + 1].root_pc } else { root_pitch + 12 };
        for step in 0..steps {
            let tone = |step: i64| chord.intervals[(step % chord.intervals.len() as i64) as usize];
            let pitch = match pattern.as_str() {
                "octave" => {
                    if step % 2 == 0 {
                        root_pitch
                    } else {
                        (root_pitch + 12).min(127)
                    }
                }
                "arpeggiated" => root_pitch + tone(step),
                _ => {
                    // walking: root, chord tone, fifth (or third), chromatic approach to the next root
                    let fifth = chord
                        .intervals
                        .iter()
                        .copied()
                        .find(|interval| *interval == 7)
                        .unwrap_or(chord.intervals[chord.intervals.len() - 1]);
                    if step == 0 {
                        root_pitch
                    } else if step == steps - 1 {
                        (next_root - 1).clamp(0, 127)
                    } else if step == steps - 2 {
                        root_pitch + fifth
                    } else {
                        root_pitch + tone(step)
                    }
                }
            };
            if !(0..=127).contains(&pitch) {
                continue;
            }
            result.push(new_note(
                pitch as f64,
                start_beat + chord_index as f64 * chord_duration + step as f64 * step_beats,
                step_beats.min(chord_duration - step as f64 * step_beats),
                velocity as f64,
            ));
        }
    }
    Ok(MidiTransformOutcome {
        notes: result,
        generative: true,
        assumptions: vec![
            format!("{pattern} bassline over {} chord(s) from {source}: {}", chords.len(), chord_names(&chords)),
            format!("{}-beat steps at octave {octave}; walking lines approach each next root chromatically from below; input notes are replaced", number_text(step_beats)),
        ],
        seed: None,
    })
}

/* --------------------------- motif transforms ------------------------------ */

fn motif_invert(notes: &[Note], params: &Params) -> Outcome {
    // Without an axis, the motif turns around its first note.
    let mut by_onset = notes.to_vec();
    by_onset.sort_by(|a, b| cmp_f64(a.start, b.start).then_with(|| cmp_f64(a.pitch, b.pitch)));
    let axis = if params.contains_key("axis") {
        integer_param(params, "axis", 0, 127, None)?
    } else {
        by_onset.first().map(|note| note.pitch as i64).unwrap_or(60)
    };
    let mut clamped = 0;
    let mut result = notes.to_vec();
    for note in &mut result {
        let inverted = 2.0 * axis as f64 - note.pitch;
        let next = clamp_pitch(inverted);
        if next != inverted {
            clamped += 1;
        }
        note.pitch = next;
    }
    let clamped_note = if clamped > 0 { format!("; {clamped} note(s) clamped to the MIDI pitch range") } else { String::new() };
    Ok(MidiTransformOutcome {
        notes: result,
        generative: false,
        assumptions: vec![format!(
            "melodic inversion around axis pitch {axis} ({}{}); rhythm unchanged{clamped_note}",
            note_name(axis % 12),
            axis / 12 - 1
        )],
        seed: None,
    })
}

fn motif_retrograde(notes: &[Note], _params: &Params, clip_length: Option<f64>) -> Outcome {
    let Some(clip_length) = clip_length.filter(|clip_length| clip_length.is_finite() && *clip_length > 0.0) else {
        return Err(range("motif-retrograde requires the exact clip length as its reversal span"));
    };
    let mut result = notes.to_vec();
    for note in &mut result {
        note.start = clip_length - (note.start + note.duration);
    }
    Ok(MidiTransformOutcome {
        notes: result,
        generative: false,
        assumptions: vec![format!(
            "retrograde within the exact {}-beat clip span; note order reversed in time, pitches and durations unchanged",
            number_text(clip_length)
        )],
        seed: None,
    })
}

fn motif_ratios(notes: &[Note], params: &Params, augment: bool) -> Outcome {
    let numerator = integer_param(params, "numerator", 1, 16, None)?;
    let denominator = integer_param(params, "denominator", 1, 16, None)?;
    if augment && numerator <= denominator {
        return Err(range("motif-augment requires numerator > denominator (a ratio above 1)"));
    }
    if !augment && numerator >= denominator {
        return Err(range("motif-diminish requires numerator < denominator (a ratio below 1)"));
    }
    let mut result = notes.to_vec();
    for note in &mut result {
        note.start = (note.start * numerator as f64) / denominator as f64;
        note.duration = ((note.duration * numerator as f64) / denominator as f64).max(1.0 / 1024.0);
    }
    let kind = if augment { "augmentation" } else { "diminution" };
    Ok(MidiTransformOutcome {
        notes: result,
        generative: false,
        assumptions: vec![format!(
            "rhythmic {kind} by the exact ratio {numerator}:{denominator}; starts and durations scaled, pitches unchanged"
        )],
        seed: None,
    })
}

/// Apply a pure deterministic transform. Fails with `Range` on invalid parameters.
pub fn apply_midi_transform(notes: &[Note], spec: &MidiTransformSpec, clip_length: Option<f64>) -> Outcome {
    validate_note_set(notes)?;
    let params = &spec.params;
    match spec.r#type.as_str() {
        "transpose" => transpose(notes, params),
        "scale-constrain" => scale_constrain(notes, params),
        "quantize" => quantize(notes, params, clip_length),
        "swing" => swing(notes, params, clip_length),
        "velocity-curve" => velocity_curve(notes, params),
        "humanize-velocity" => humanize_velocity(notes, params),
        "humanize-timing" => humanize_timing(notes, params, clip_length),
        "legato" => legato(notes, params, clip_length),
        "staccato" => staccato(notes, params),
        "rotate" => rotate(notes, params),
        "repeat" => repeat(notes, params),
        "ratchet" => ratchet(notes, params),
        "chord-voicing" => chord_voicing(notes, params),
        "arpeggiate" => arpeggiate(notes, params),
        "seeded-variation" => seeded_variation(notes, params, clip_length),
        "euclidean" => euclidean_rhythm(notes, params),
        "chord-progression" => chord_progression(notes, params),
        "drum-pattern" => drum_pattern(notes, params),
        "bassline" => bassline(notes, params),
        "motif-invert" => motif_invert(notes, params),
        "motif-retrograde" => motif_retrograde(notes, params, clip_length),
        "motif-augment" => motif_ratios(notes, params, true),
        "motif-diminish" => motif_ratios(notes, params, false),
        other => Err(range(format!("unknown MIDI transform: {other}"))),
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct NoteDiff {
    /// New notes without ids, in stable order.
    pub add: Vec<Note>,
    /// Existing notes with their full resulting field set (id required).
    pub update: Vec<Note>,
    /// Ids of source notes absent from the result.
    pub r#delete: Vec<i64>,
}

/// Exact add/update/delete diff between a source note set and a transform result.
pub fn diff_notes(before: &[Note], after: &[Note]) -> NoteDiff {
    let mut before_ids: Vec<i64> = Vec::new();
    let mut before_by_id: HashMap<i64, &Note> = HashMap::new();
    for note in before {
        if let Some(id) = note.id.cloned() {
            if before_by_id.insert(id, note).is_none() {
                before_ids.push(id);
            }
        }
    }
    let note_json = |note: &Note| stringify(&serde_json::to_value(note).unwrap_or(Value::Null));
    let mut add: Vec<Note> = Vec::new();
    let mut update: Vec<Note> = Vec::new();
    let mut after_ids: HashSet<i64> = HashSet::new();
    for note in after {
        let Some(id) = note.id.cloned() else {
            add.push(note.clone());
            continue;
        };
        after_ids.insert(id);
        let Some(prior) = before_by_id.get(&id) else {
            add.push(note.clone());
            continue;
        };
        if note_json(prior) != note_json(note) {
            update.push(note.clone());
        }
    }
    let remove: Vec<i64> = before_ids.into_iter().filter(|id| !after_ids.contains(id)).collect();
    NoteDiff { add, update, r#delete: remove }
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct MidiExpressionProbe {
    pub note_schema_fields: Vec<String>,
    pub exposes_per_note_expression: bool,
    pub delete_recreate_preserves_expression: bool,
}

/// Per-note expression probe: the canonical note schema fields the adapter
/// round-trips. Per-note Pitch/Slide/Pressure are not in the schema today, so
/// delete/recreate transforms must never claim to preserve them.
pub fn midi_expression_probe() -> MidiExpressionProbe {
    MidiExpressionProbe {
        note_schema_fields: [
            "pitch",
            "start",
            "duration",
            "velocity",
            "channel",
            "mute",
            "probability",
            "velocityDeviation",
            "releaseVelocity",
        ]
        .iter()
        .map(|field| field.to_string())
        .collect(),
        exposes_per_note_expression: false,
        delete_recreate_preserves_expression: false,
    }
}

/// Content digest for note sets, with the transform bounds (MIDI_TRANSFORM_MAX_NOTES) rather
/// than the mutation-authority canonicalizer's tighter wire bounds. Ignores
/// server-assigned note ids so content comparisons survive re-creation.
pub fn note_content_digest<T: Serialize>(notes: &[T]) -> Result<String, MidiTransformError> {
    note_set_digest(notes, false)
}

/// Exact identity-bound digest for note sets with stable server ids: two
/// same-onset notes swapping canonical content changes this digest even though
/// the ID-agnostic content digest stays unchanged. Used for in-place
/// apply/undo fences where identity is authoritative.
pub fn note_identity_digest<T: Serialize>(notes: &[T]) -> Result<String, MidiTransformError> {
    note_set_digest(notes, true)
}

fn canonical(value: &Value, depth: usize) -> Result<String, MidiTransformError> {
    let other = |message: &str| MidiTransformError::Other(message.to_string());
    if depth > 8 {
        return Err(other("note content is too deeply nested"));
    }
    match value {
        Value::Null | Value::Bool(_) => Ok(stringify(value)),
        Value::Number(number) => {
            if !number.as_f64().is_some_and(f64::is_finite) {
                return Err(other("note content contains a non-finite number"));
            }
            Ok(stringify(value))
        }
        Value::String(text) => {
            if utf16_len(text) > 16384 {
                return Err(other("note content string is too large"));
            }
            Ok(stringify(value))
        }
        Value::Array(items) => {
            if items.len() > MIDI_TRANSFORM_MAX_NOTES {
                return Err(other(&format!("note content array exceeds the {MIDI_TRANSFORM_MAX_NOTES}-note transform bound")));
            }
            let parts = items.iter().map(|item| canonical(item, depth + 1)).collect::<Result<Vec<_>, _>>()?;
            Ok(format!("[{}]", parts.join(",")))
        }
        Value::Object(record) => {
            if record.len() > 64 {
                return Err(other("note content object is too large"));
            }
            let mut keys: Vec<&String> = record.keys().collect();
            keys.sort_by(|a, b| js_str_cmp(a, b));
            let parts = keys
                .into_iter()
                .map(|key| Ok(format!("{}:{}", quote(key), canonical(&record[key], depth + 1)?)))
                .collect::<Result<Vec<_>, MidiTransformError>>()?;
            Ok(format!("{{{}}}", parts.join(",")))
        }
    }
}

/// `const { id: _id, ...content } = note`: the note's own properties but `id`.
fn without_id(note: Value) -> Result<Value, MidiTransformError> {
    match note {
        Value::Object(mut record) => {
            record.shift_remove("id");
            Ok(Value::Object(record))
        }
        Value::Null => Err(MidiTransformError::Other("Cannot destructure property 'id' of 'note' as it is null.".to_string())),
        Value::Array(items) => Ok(Value::Object(items.into_iter().enumerate().map(|(index, item)| (index.to_string(), item)).collect())),
        Value::String(text) => Ok(Value::Object(
            text.encode_utf16()
                .enumerate()
                .map(|(index, unit)| (index.to_string(), Value::String(String::from_utf16_lossy(&[unit]))))
                .collect(),
        )),
        _ => Ok(Value::Object(Map::new())),
    }
}

fn note_set_digest<T: Serialize>(notes: &[T], include_ids: bool) -> Result<String, MidiTransformError> {
    // TS: the row array's bound is checked when it is canonicalized, after the rows are; here first.
    if notes.len() > MIDI_TRANSFORM_MAX_NOTES {
        return Err(MidiTransformError::Other(format!("note content array exceeds the {MIDI_TRANSFORM_MAX_NOTES}-note transform bound")));
    }
    let mut rows: Vec<String> = Vec::with_capacity(notes.len());
    for note in notes {
        let value =
            serde_json::to_value(note).map_err(|_| MidiTransformError::Other("note content contains an unsupported value".to_string()))?;
        let row = if include_ids { value } else { without_id(value)? };
        rows.push(canonical(&row, 1)?);
    }
    rows.sort_by(|a, b| js_str_cmp(a, b));
    Ok(hex::encode(Sha256::digest(format!("[{}]", rows.join(",")).as_bytes())))
}
