//! Note-level moves on a DAW-neutral note form (#400 review notes, shared moves): a MIDI take's notes, in beats from its
//! start, and the operators every DAW whose takes hold notes shares: notes moved by semitones (or some of them an octave),
//! velocities and timing loosened, notes thinned out; each leaves the anchors (the notes every take keeps) as they are.
//! REAPER's MIDI items and Live's Session clips take them from here; a backend's own operators (an audio take's rate, its
//! fades) stay its own.
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

/// The shared note operators (offered to the model; Kumi's own directions also move some notes an octave: "octave").
pub const NOTE_OPS: [&str; 4] = ["pitch", "velocity", "timing", "thin"];

/// The directions Kumi picks for takes of notes (a Live clip, a REAPER MIDI item) the model leaves to it.
pub const NOTE_DIRECTIONS: [&str; 5] = ["higher", "lower", "looser", "softer", "sparser"];

/// The operators of a note direction, each able to make a take of an ordinary clip that passes for its source and isn't a
/// copy of it: some notes an octave up or down, timing and velocities loosened either way, velocities down, fewer notes.
pub fn note_direction_ops(direction: &str) -> Vec<Value> {
    let op = |name: &str, up: bool| json!({"op": name, "amount": 0.5, "up": up});
    match direction {
        "lower" => vec![op("octave", false)],
        // (timing alone moves a note a few hundredths of a beat: a near-copy)
        "looser" => vec![json!({"op": "timing", "amount": 1}), json!({"op": "velocity", "amount": 0.5})],
        "softer" => vec![op("velocity", false)],
        "sparser" => vec![op("thin", true)],
        _ => vec![op("octave", true)],
    }
}

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
            // (up to 40 steps, about a dynamic: all of it makes a take of even a short figure, not a near-copy)
            for note in notes.iter_mut().filter(|note| free(note)) {
                let moved = (sign(op, rng) * some_of(40. * much, rng)).round() as i64;
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
        // (Kumi's own "higher" and "lower": some of the notes, at least one, an octave up or down, the same notes of the scale
        // in another register; moving every note would make another line, too far from the source to pass for it)
        "octave" => {
            let step = if sign(op, rng) > 0. { 12 } else { -12 };
            let loose: Vec<usize> =
                (0..notes.len()).filter(|k| free(&notes[*k]) && (0..=127).contains(&(notes[*k].pitch + step))).collect();
            let count = ((loose.len() as f64 * some_of(much, rng)).round() as usize).clamp(1.min(loose.len()), loose.len());
            for k in rng.choose(loose.len(), count) {
                notes[loose[k]].pitch += step;
            }
            format!("{count} notes an octave {}", if step > 0 { "up" } else { "down" })
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

/// For a backend's tests: each note direction makes a take of an ordinary riff and of a short figure (their first note the
/// anchor) that passes core's checks, drawn as the tool draws it (bolder after a near-copy, milder after one too far) and clear
/// of the takes before it: a make of five gets five takes. `material` makes the backend's material of notes and anchors,
/// `apply` is its operators', `notes` its notes.
#[cfg(test)]
pub fn every_note_direction_makes_a_take<M: super::Material + Clone>(
    material: impl Fn(Vec<Note>, Vec<Note>) -> M,
    apply: impl Fn(&[Value], f64, &mut M, &M, &mut Rng) -> Result<Vec<String>, String>,
    notes: fn(&M) -> &[Note],
) {
    use super::{bolder_or_milder, roll, Bounds, TRIES};
    let note = |pitch: i64, start: f64, velocity: i64| Note { pitch, start, length: 0.4, velocity, channel: 1 };
    let riff: Vec<Note> =
        [0, 3, 7, 10, 12, 10, 7, 3].iter().enumerate().map(|(k, step)| note(48 + step, k as f64 * 0.5, 100 - 5 * k as i64)).collect();
    let figure: Vec<Note> = [60, 63, 67, 70].iter().enumerate().map(|(k, pitch)| note(*pitch, k as f64, 96)).collect();
    for played in [riff, figure] {
        let anchors = vec![played[0].clone()];
        let source = material(played, anchors.clone());
        for seed in 0..32 {
            let mut made: Vec<(usize, M)> = Vec::new();
            for (k, direction) in NOTE_DIRECTIONS.iter().enumerate() {
                let ops = note_direction_ops(direction);
                let others: Vec<(usize, &M)> = made.iter().map(|(n, take)| (*n, take)).collect();
                let mut factor = 1.;
                let rolled = roll(seed, k + 1, TRIES, Some(&source), &others, &Bounds::default(), |seed, before| {
                    factor = bolder_or_milder(factor, before);
                    let mut take = source.clone();
                    apply(&ops, factor, &mut take, &source, &mut Rng::new(seed))?;
                    anchors_in(&anchors, notes(&take))?;
                    let issues = foreign_classes(notes(&source), notes(&take), &ops);
                    if issues.is_empty() {
                        Ok(take)
                    } else {
                        Err(issues.join("; "))
                    }
                });
                assert!(rolled.verdict.is_fine(), "{direction}, seed {seed}: {}", rolled.verdict.words());
                made.push((k + 1, rolled.take.unwrap()));
            }
        }
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

    #[test]
    fn some_notes_go_an_octave_up_never_the_anchors_nor_past_the_top() {
        let anchors = vec![note(48, 0.)];
        let source: Vec<Note> =
            [48, 51, 55, 58, 60, 58, 55, 120].iter().enumerate().map(|(k, pitch)| note(*pitch, k as f64 * 0.5)).collect();
        for seed in 0..32 {
            let mut notes = source.clone();
            let said =
                note_op("octave", &json!({"op": "octave", "up": true}), 0.5, &mut notes, &anchors, &mut Rng::new(seed)).unwrap().unwrap();
            let moved: Vec<(i64, i64)> =
                source.iter().zip(&notes).filter(|(was, now)| was.pitch != now.pitch).map(|(was, now)| (was.pitch, now.pitch)).collect();
            assert!((1..7).contains(&moved.len()), "some notes, not all: {moved:?}");
            assert!(moved.iter().all(|(was, now)| now - was == 12 && *was != 120 && *was != 48), "{moved:?}");
            assert_eq!(said, format!("{} notes an octave up", moved.len()));
        }
        // (nothing that can move: nothing moves)
        let mut notes = vec![note(48, 0.)];
        let said = note_op("octave", &json!({"op": "octave", "up": false}), 1., &mut notes, &anchors, &mut Rng::new(1)).unwrap().unwrap();
        assert_eq!((said.as_str(), notes[0].pitch), ("0 notes an octave down", 48));
    }
}
