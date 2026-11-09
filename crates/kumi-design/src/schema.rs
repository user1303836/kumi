//! A design request's JSON Schema, for every surface that takes one (serve's `design`, the agent's `design_sound`):
//! one description, its lists of choices built from the same types `Request::check` reads, so no surface drifts from
//! what the engine takes. `check` still says what's wrong in words; the schema tells a caller what to send.

use super::{
    choices::{Carrier, Contour, Modulation, Pairing, Ring, Scale, Style, Tuning},
    Mode, Op, MAX_BARS, MAX_SECONDS,
};
use serde::Serialize;
use serde_json::{json, Value};

/// Each choice's name as a request writes it.
fn names<T: Serialize>(all: &[T]) -> Value {
    Value::Array(all.iter().map(|choice| serde_json::to_value(choice).unwrap_or(Value::Null)).collect())
}

pub fn ops() -> Value {
    names(&[Op::Cross, Op::Fuse, Op::Stack, Op::Grow])
}

pub fn modes() -> Value {
    names(&[Mode::Blend, Mode::Attack, Mode::Resonate, Mode::Modulate, Mode::Texture, Mode::Loop, Mode::Smear])
}

fn number(low: f64, high: f64, what: &str) -> Value {
    json!({"type": "number", "minimum": low, "maximum": high, "description": what})
}

fn range(low: f64, high: f64, what: &str) -> Value {
    json!({"type": "array", "minItems": 2, "maxItems": 2, "items": {"type": "number", "minimum": low, "maximum": high}, "description": what})
}

/// The steer: controls on the result, and on each source (each draws its own value within a range from the seed).
pub fn steer() -> Value {
    json!({"type": "object", "additionalProperties": false, "description": "Push the design one way before it renders; every control is optional and neutral left out.", "properties": {
        "bright": number(-1.0, 1.0, "dark (-1) to bright (1)"),
        "body": number(-1.0, 1.0, "thin (-1) to full (1)"),
        "filter": number(-1.0, 1.0, "a low-pass closing toward 150 Hz (-1) or a high-pass opening toward 5 kHz (1); 0 is open"),
        "resonance": number(0.0, 1.0, "the filter's peak"),
        "noise": number(-1.0, 1.0, "tonal (-1) to noisy (1)"),
        "punch": number(-1.0, 1.0, "soft (-1) to percussive (1)"),
        "length": number(-1.0, 1.0, "shorter (-1) to longer (1)"),
        "drive": number(0.0, 1.0, "saturation"),
        "crush": number(0.0, 1.0, "lo-fi: fewer bits, a lower rate"),
        "width": number(-1.0, 1.0, "mono (-1) to wide (1)"),
        "space": number(0.0, 1.0, "dry (0) to a room growing into a hall (1)"),
        "pitch": range(-24.0, 24.0, "[low, high] semitones: each sound repitched somewhere in it"),
        "reverse": number(0.0, 1.0, "the chance each sound plays backwards"),
        "start": range(0.0, 0.9, "[low, high]: each sound starts this far into itself (a fraction of its length)"),
        "delay": range(0.0, 500.0, "[low, high] ms: each sound comes in this much later"),
        "pan": number(0.0, 1.0, "each sound panned within ± this"),
        "level": number(0.0, 1.0, "each sound's level drops by up to this × 18 dB")
    }})
}

fn unit(what: &str) -> Value {
    number(0.0, 1.0, what)
}

fn resonate() -> Value {
    json!({"type": "object", "additionalProperties": false, "description": "cross resonate's choices", "properties": {
        "pairing": {"enum": names(&Pairing::ALL), "description": "who rings through whom (halves: a seeded half through the other)"},
        "ring": {"enum": names(&Ring::ALL), "description": "a resonator rings by its partials, as a body, or both"},
        "decay": unit("a short ring (0) to a long one (1)")
    }})
}

fn modulate() -> Value {
    json!({"type": "object", "additionalProperties": false, "description": "cross modulate's choices", "properties": {
        "carrier": {"enum": names(&Carrier::ALL), "description": "which sound is played and shaped"},
        "kinds": {"type": "array", "minItems": 1, "items": {"enum": names(&Modulation::ALL)}, "description": "the ways allowed"},
        "stages": {"type": "integer", "minimum": 1, "maximum": 8, "description": "how many shapings run, one after another"},
        "retrigger": unit("the chance a short modulator repeats along the carrier")
    }})
}

fn looping() -> Value {
    let properties: serde_json::Map<String, Value> = [
        ("style", json!({"enum": names(&Style::ALL), "description": "a beat of the hits, or a melody of the sounds repitched"})),
        (
            "meter",
            json!({"type": "array", "minItems": 2, "maxItems": 2, "items": {"type": "integer", "minimum": 1, "maximum": 16},
                "description": "[beats, unit]: [7, 8] is 7/8 (unit 2, 4, 8 or 16)"}),
        ),
        ("grid", json!({"enum": [2, 3, 4, 6, 8], "description": "steps a beat"})),
        ("swing", number(0.0, 0.75, "how late the off-steps fall")),
        ("density", unit("how many steps sound")),
        ("humanize", unit("timing and velocity drift")),
        ("accent", unit("how much the strong steps stand out")),
        ("fills", unit("more happening at a phrase's end")),
        ("ratchet", unit("rolls")),
        ("poly", unit("voices cycling in other lengths")),
        ("variation", unit("how much bars differ")),
        ("lean", number(-1.0, 1.0, "beat: hits clustered toward the loop's start (-1) or end (1)")),
        ("syncopation", unit("beat: hits pulled onto the off-beats")),
        ("ghosts", unit("beat: quiet ghost notes")),
        ("open", unit("beat: the chance a hat rings open")),
        ("kick", unit("beat: how busy the kick is")),
        ("snare", unit("beat: how busy the snare is")),
        ("hats", unit("beat: how busy the hats are")),
        ("perc", unit("beat: how busy the percussion is")),
        ("key", json!({"type": "string", "pattern": "^[A-Ga-g](#|b|♯|♭)?$", "description": "melody: the root"})),
        ("scale", json!({"enum": names(&Scale::ALL), "description": "melody: the scale"})),
        ("tuning", json!({"enum": names(&Tuning::ALL), "description": "melody: the tuning"})),
        ("micro", unit("melody: the chance a note bends off the scale by the tuning's smallest step")),
        ("octaves", json!({"type": "integer", "minimum": 1, "maximum": 4, "description": "melody: the octaves it spans"})),
        ("register", json!({"type": "integer", "minimum": -2, "maximum": 2, "description": "melody: octaves up or down"})),
        ("contour", json!({"enum": names(&Contour::ALL), "description": "melody: how it moves"})),
        ("chords", unit("melody: the chance of stacked notes")),
        ("glide", unit("melody: slides between notes")),
        ("gate", number(0.05, 1.0, "melody: how much of the time to its next note a note sounds")),
        ("drone", unit("melody: a held root under it")),
    ]
    .into_iter()
    .map(|(key, value)| (key.to_string(), value))
    .collect();
    json!({"type": "object", "additionalProperties": false, "description": "fuse loop's choices", "properties": properties})
}

/// The options: what a design may be told; everything is optional and drawn from the seed when it isn't given.
pub fn options() -> Value {
    json!({"type": "object", "additionalProperties": false, "properties": {
        "seconds": number(0.1, MAX_SECONDS, "the render's length"),
        "bpm": number(40.0, 240.0, "a loop's tempo (120 when not given; give the DAW's)"),
        "bars": {"type": "integer", "minimum": 1, "maximum": MAX_BARS, "description": "a loop's bars"},
        "attack_from": {"type": "integer", "minimum": 0, "description": "attack: the sound (by place) whose attack leads"},
        "split": {"type": "array", "minItems": 1, "items": {"type": "integer", "minimum": 0}, "description": "resonate: the sounds (by place) that excite"},
        "depth": unit("how far the design goes"),
        "stretch": number(4.0, 16.0, "smear: how many times longer"),
        "at": unit("grow: how far from the first sound toward the second (0.5 when not given)"),
        "steer": steer(),
        "resonate": resonate(),
        "modulate": modulate(),
        "loop": looping()
    }})
}
