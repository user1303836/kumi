//! Note-level moves on a DAW-neutral note form (#400 review notes, shared moves): a MIDI take's notes, in beats from its
//! start, and the operators every DAW whose takes hold notes shares: notes moved by semitones, velocities and timing loosened,
//! notes thinned out; each leaves the anchors (the notes every take keeps) as they are. REAPER's MIDI items take them
//! from here; a backend's own operators (an audio take's rate, its fades) stay its own.
use super::Rng;
use serde_json::{json, Value};
use std::collections::BTreeSet;

/// A MIDI note: pitch, start and length (beats from the take's start), velocity (1 to 127) and channel (1 to 16).
#[derive(Debug, Clone, PartialEq)]
pub struct Note {
    pub pitch: i64,
    pub start: f64,
    pub length: f64,
    pub velocity: i64,
    pub channel: i64,
}

impl Note {
    pub fn json(&self) -> Value {
        json!({"pitch": self.pitch, "start": round(self.start), "length": round(self.length), "velocity": self.velocity, "channel": self.channel})
    }

    /// A note as a DAW's notes write it ({pitch, start, length, velocity?, channel?}), or None when it isn't one.
    pub fn of(value: &Value) -> Option<Note> {
        Some(Note {
            pitch: value.get("pitch")?.as_i64()?.clamp(0, 127),
            start: value.get("start")?.as_f64()?.max(0.),
            length: value.get("length").or_else(|| value.get("duration"))?.as_f64()?.max(1e-3),
            velocity: value.get("velocity").and_then(Value::as_f64).map_or(100, |v| v.round() as i64).clamp(1, 127),
            channel: value.get("channel").and_then(Value::as_i64).unwrap_or(1).clamp(1, 16),
        })
    }

    /// The same note: its pitch and channel, starting at the same time (a rounded start is the same).
    pub fn same(&self, other: &Note) -> bool {
        self.pitch == other.pitch && self.channel == other.channel && (self.start - other.start).abs() < 1e-3
    }
}

/// A time in beats, to the 10 000th.
pub fn round(beats: f64) -> f64 {
    (beats * 10_000.).round() / 10_000.
}

/// How much of an operator, `factor` times as much: 0 to 1.
pub fn amount(op: &Value, factor: f64) -> f64 {
    (op.get("amount").and_then(Value::as_f64).unwrap_or(0.5) * factor).clamp(0., 1.)
}

/// Up or down: as the operator says, else by chance.
pub fn sign(op: &Value, rng: &mut Rng) -> f64 {
    match op.get("up").and_then(Value::as_bool) {
        Some(true) => 1.,
        Some(false) => -1.,
        None if rng.chance(0.5) => 1.,
        None => -1.,
    }
}

/// Some of `most`: a third to all of it, by chance.
pub fn some_of(most: f64, rng: &mut Rng) -> f64 {
    most * (1. + 2. * rng.below(1000) as f64 / 1000.) / 3.
}

/// The shared note operators.
pub const NOTE_OPS: [&str; 4] = ["pitch", "velocity", "timing", "thin"];

/// A shared note operator applied to `notes`, `much` of it (0 to 1), the anchors left as they are: what it did, in a few
/// words; None when `name` isn't one of them (the backend's own).
pub fn note_op(
    name: &str,
    op: &Value,
    much: f64,
    notes: &mut Vec<Note>,
    anchors: &[Note],
    rng: &mut Rng,
) -> Option<Result<String, String>> {
    let free = |note: &Note| !anchors.iter().any(|anchor| anchor.same(note));
    Some(Ok(match name {
        "pitch" => {
            let semitones =
                op.get("semitones").and_then(Value::as_f64).map(f64::round).unwrap_or_else(|| sign(op, rng) * (1. + (much * 4.).round()));
            for note in notes.iter_mut().filter(|note| free(note)) {
                note.pitch = (note.pitch + semitones as i64).clamp(0, 127);
            }
            format!("notes {semitones:+} st")
        }
        "velocity" => {
            for note in notes.iter_mut().filter(|note| free(note)) {
                let moved = (sign(op, rng) * some_of(30. * much, rng)).round() as i64;
                note.velocity = (note.velocity + moved).clamp(1, 127);
            }
            "velocities moved".into()
        }
        "timing" => {
            for note in notes.iter_mut().filter(|note| free(note)) {
                note.start = (note.start + sign(op, rng) * some_of(0.06 * much, rng)).max(0.);
            }
            "timing loosened".into()
        }
        "thin" => {
            let loose: Vec<usize> = (0..notes.len()).filter(|k| free(&notes[*k])).collect();
            // (at least one note goes, and one stays)
            let most = if notes.len() > loose.len() { loose.len() } else { loose.len().saturating_sub(1) };
            let count = ((loose.len() as f64 * 0.4 * much).round() as usize).max(1).min(most);
            let dropped: BTreeSet<usize> = rng.choose(loose.len(), count).into_iter().map(|k| loose[k]).collect();
            let mut k = 0;
            notes.retain(|_| {
                let keep = !dropped.contains(&k);
                k += 1;
                keep
            });
            format!("{} notes fewer", dropped.len())
        }
        _ => return None,
    }))
}

/// Notes the model wrote with the take's anchor notes in, as they are (#400 review notes: written notes never have to repeat
/// them), by start and pitch: a written note that is an anchor's (its pitch, channel and start, another velocity or length)
/// gives way to it.
pub fn with_anchors(mut notes: Vec<Note>, anchors: &[Note]) -> Vec<Note> {
    notes.retain(|note| !anchors.iter().any(|anchor| anchor.same(note)));
    notes.extend(anchors.iter().cloned());
    notes.sort_by(|a, b| a.start.total_cmp(&b.start).then(a.pitch.cmp(&b.pitch)));
    notes
}

/// How far apart two note lists are: the share of notes without a partner (same pitch and channel, its start near), and
/// how far partners' velocities and starts are.
pub fn notes_apart(a: &[Note], b: &[Note]) -> f64 {
    let most = a.len().max(b.len());
    if most == 0 {
        return 0.;
    }
    let mut used = vec![false; b.len()];
    let mut apart = 0.;
    let mut unmatched = 0usize;
    for note in a {
        let partner = b
            .iter()
            .enumerate()
            .filter(|(k, other)| {
                !used[*k] && other.pitch == note.pitch && other.channel == note.channel && (other.start - note.start).abs() <= 0.25
            })
            .min_by(|(_, x), (_, y)| (x.start - note.start).abs().total_cmp(&(y.start - note.start).abs()));
        match partner {
            Some((k, other)) => {
                used[k] = true;
                apart += 0.5 * ((other.velocity - note.velocity).abs() as f64 / 64.).min(1.)
                    + 0.5 * ((other.start - note.start).abs() / 0.25).min(1.);
            }
            None => unmatched += 1,
        }
    }
    unmatched += used.iter().filter(|u| !**u).count();
    ((unmatched as f64 + apart) / most as f64).min(1.)
}

/// What a take holds of notes its source doesn't: pitch classes it has no note of, unless an operator moved pitch on purpose.
pub fn foreign_classes(source: &[Note], take: &[Note], ops: &[Value]) -> Vec<String> {
    if ops.iter().any(|op| op.get("op").and_then(Value::as_str) == Some("pitch")) {
        return Vec::new();
    }
    let classes: BTreeSet<i64> = source.iter().map(|note| note.pitch % 12).collect();
    let outside: BTreeSet<i64> = take.iter().map(|note| note.pitch % 12).filter(|class| !classes.contains(class)).collect();
    if outside.is_empty() {
        Vec::new()
    } else {
        vec![format!("{} note class{} the source doesn't have", outside.len(), if outside.len() == 1 { "" } else { "es" })]
    }
}

/// Whether a take keeps every anchor note as it is: which one it doesn't, in words.
pub fn anchors_in(anchors: &[Note], notes: &[Note]) -> Result<(), String> {
    match anchors.iter().find(|anchor| !notes.iter().any(|note| note == *anchor)) {
        Some(anchor) => Err(format!("the anchor note {} at {:.2} isn't as it was", anchor.pitch, anchor.start)),
        None => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn note(pitch: i64, start: f64) -> Note {
        Note { pitch, start, length: 0.25, velocity: 100, channel: 1 }
    }

    #[test]
    fn written_notes_get_the_anchor_notes_they_left_out() {
        let anchors = vec![note(36, 0.), note(38, 1.)];
        let written = with_anchors(vec![note(42, 0.5), note(36, 0.)], &anchors);
        assert_eq!(written, vec![note(36, 0.), note(42, 0.5), note(38, 1.)]);
        // (the kick written softer, longer and a hair late is the anchor's own note: one kick, the anchor's)
        let loose = Note { velocity: 60, length: 0.5, start: 0.0004, ..note(36, 0.) };
        assert_eq!(with_anchors(vec![loose], &anchors), anchors);
    }

    #[test]
    fn the_shared_operators_leave_the_anchors_and_thin_keeps_one() {
        let anchors = vec![note(36, 0.)];
        let mut notes = vec![note(36, 0.), note(42, 0.5), note(42, 1.5)];
        let mut rng = Rng::new(3);
        assert_eq!(
            note_op("pitch", &json!({"op": "pitch", "semitones": 12}), 1., &mut notes, &anchors, &mut rng).unwrap().unwrap(),
            "notes +12 st"
        );
        assert_eq!(notes.iter().map(|note| note.pitch).collect::<Vec<_>>(), [36, 54, 54], "the anchor stays");
        let said = note_op("thin", &json!({"op": "thin"}), 1., &mut notes, &anchors, &mut rng).unwrap().unwrap();
        assert_eq!(said, "1 notes fewer");
        assert!(notes.contains(&note(36, 0.)) && notes.len() == 2);
        assert!(note_op("rate", &json!({"op": "rate"}), 1., &mut notes, &anchors, &mut rng).is_none(), "a backend's own");
        assert_eq!(Note::of(&json!({"pitch": 60, "start": 1, "duration": 0.5, "velocity": 90.4})).unwrap().velocity, 90);
    }
}
