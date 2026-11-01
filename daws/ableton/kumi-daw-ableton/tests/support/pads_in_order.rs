use serde_json::{json, Value};

/// Keep replaying the frozen source for Kumi's words from before a kit's samples filled a Drum Rack's pads in order. A
/// pad's note left out is now the rack's next empty pad, and a drum General MIDI names finds the pad that holds one. The
/// first-request goldens pin the words as they are; this projection puts back only these, each pair (now, then).
const SINCE: &[(&str, &str)] = &[
    (
        "Leave note out and the sample takes the rack's next empty pad, in order from its first, row by row: C1, C#1, D1 and \
         D#1, then E1 up (36 to 51 on a new rack), so a kit's sounds fill the first pads in the order you load them. Give note \
         (36 is C1) only for a pad the producer names, or General MIDI's keys when they ask for them. sample is",
        "note is the pad's note: 36 (C1) is the first pad, then 37, 38 and so on up to 51 on a new rack. sample is",
    ),
    (
        "The pad, only for one the producer names (36 is C1, the first pad); left out, the next empty pad",
        "The pad: 36 is C1, the first pad",
    ),
    (
        "mapping {kick, snare, closedHat, …: each one's pad note; left out, Kumi finds them by the rack's chain names}}",
        "mapping {kick: 36, snare: 38, closedHat: 42, …}}",
    ),
    (
        r#"A step with each repeats: {"sample": ["…/Kick.wav", "…/Snare.wav", "…/Hat.wav"]},"#,
        r#"A step with each repeats: {"note": [36, 37, 38]},"#,
    ),
    (
        r#"then one load_sample_to_pad step on "@rack" with each: {"sample": [...]} in the order the producer asked for the sounds, and no note: they fill the rack's first empty pads in order, row by row (C1, C#1, D1, D#1, then E1 up), so four sounds sit on C1 to D#1. Give notes only when the producer names pads or notes, or asks for General MIDI's keys."#,
        r#"then one load_sample_to_pad step on "@rack" with each: {"note": [36, 37, …]} for the pads from 36 (C1) up."#,
    ),
    (
        "Name drums by the track's pads (or kick, snare, clap, hat, openhat…, which find the pad that holds one by its name) or \
         by pitch",
        "Name drums by the track's pads (or kick, snare, clap, hat, openhat…) or by pitch",
    ),
    (r#"e.g. {"sample": ["…/Kick.wav", "…/Snare.wav", "…/Hat.wav"]},"#, r#"e.g. {"note": [36, 37, 38, 39]},"#),
    // (make_changes' line for it, the fields it takes: a note may be left out)
    ("in a new Simpler on… (deviceRef, sample, note?, instrument?)", "in a new Simpler on… (deviceRef, note, sample, instrument?)"),
];

/// The paragraph after the one on arranging, which arrange's own description carries since #324: it's sent only when
/// arrange is looked up.
const AFTER_ARRANGING: &str = "
You can watch a video with watch_video:";

/// The instructions' paragraph on arranging, as the source had it: arrange's guide, worded for the instructions.
fn arranging() -> String {
    let guide = kumi_daw_ableton::arrange::ARRANGE_GUIDE.as_str();
    let rest = guide.strip_prefix("Use it in two calls: first without sections to see the material").expect("arrange's guide");
    format!(
        "
To turn a loop, scenes or a few bars into a track (\"arrange this\", \"make a 3-minute track from these scenes\", \"arrange          it like this reference\"), use arrange: first without sections to see the material{}",
        rest.replacen("; then once with the whole form", "; then call arrange once with the whole form", 1)
    )
}

/// A text as the source had it.
pub fn legacy(text: &str) -> String {
    let text = SINCE.iter().fold(text.to_owned(), |text, (now, then)| text.replace(now, then));
    text.replacen(AFTER_ARRANGING, &format!("{}{AFTER_ARRANGING}", arranging()), 1)
}

/// A catalog (the change kinds, the tools' definitions) as the source had it: its words, and load_sample_to_pad's note
/// required again.
pub fn legacy_json(value: &Value) -> Value {
    match value {
        Value::String(text) => Value::String(legacy(text)),
        Value::Array(items) => Value::Array(items.iter().map(legacy_json).collect()),
        Value::Object(map) => {
            let mut out: serde_json::Map<String, Value> = map.iter().map(|(key, value)| (key.clone(), legacy_json(value))).collect();
            let note = out.get("properties").and_then(|properties| properties.get("note")).and_then(|note| note.get("description"));
            if note == Some(&json!(SINCE[1].1)) && out.get("required") == Some(&json!(["deviceRef", "sample"])) {
                out.insert("required".into(), json!(["deviceRef", "note", "sample"]));
            }
            Value::Object(out)
        }
        other => other.clone(),
    }
}
