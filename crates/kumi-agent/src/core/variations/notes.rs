//! Note-level moves on a DAW-neutral note form (#400 review notes, shared moves): a MIDI take's notes, in beats from its
//! start, and the operators every DAW whose takes hold notes shares: notes moved by semitones (or some of them an octave),
//! velocities and timing loosened, notes thinned out; each leaves the anchors (the notes every take keeps) and notes
//! before a trimmed start as they are. Those earlier notes can play when a REAPER item loops its source again.
//! REAPER's MIDI items and Live's Session clips take them from here; a backend's own operators (an audio take's rate, its
//! fades) stay its own.
use super::Rng;
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};

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
            start: value.get("start")?.as_f64()?,
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
    let free = |note: &Note| note.start >= 0. && !anchors.iter().any(|anchor| anchor.same(note));
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
            // (no later than the take's last note ends: past its source's end a DAW may refuse the take)
            let last = notes.iter().map(|note| note.start + note.length).fold(0., f64::max);
            // (which are anchors is told before any moves: one may land right on an anchor)
            let stays: Vec<bool> = notes.iter().map(|note| !free(note)).collect();
            for note in notes.iter_mut().filter(|note| free(note)) {
                let moved = note.start + sign(op, rng) * some_of(0.06 * much, rng);
                note.start = moved.min((last - note.length).max(note.start)).max(0.);
            }
            parted(notes, &stays, last);
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

/// How close two notes of one pitch and channel may start: a sixty-fourth of a triplet, a whole tick on any common grid.
const CLOSEST: f64 = 1. / 96.;

/// Notes of one pitch and channel no longer overlapping: a note ends just before the next of them starts, as a note-off of one pitch
/// stops the note sounding on it whichever it was meant for, and it'd cut the later one short. An anchor stays as it is: a note that
/// would land on it, or too close before it, goes to its nearer side with room. None is dropped, and none ends past `last` while there's room after the last
/// anchor for them to start `CLOSEST` apart: a roll denser than that can.
fn parted(notes: &mut [Note], stays: &[bool], last: f64) {
    let mut runs: BTreeMap<(i64, i64), Vec<usize>> = BTreeMap::new();
    for (at, note) in notes.iter().enumerate() {
        runs.entry((note.pitch, note.channel)).or_default().push(at);
    }
    for run in runs.values_mut() {
        run.sort_by(|a, b| notes[*a].start.total_cmp(&notes[*b].start));
        let mut fixed: Vec<(f64, f64)> = run
            .iter()
            .filter(|at| stays[**at])
            .map(|at| (round(notes[*at].start), round(notes[*at].start) + round(notes[*at].length)))
            .collect();
        fixed.sort_by(|a, b| a.0.total_cmp(&b.0));
        // (each after the one before, and clear of the anchors, its end where it was)
        let mut floor = 0_f64;
        for at in run.iter().copied().filter(|at| !stays[*at]) {
            let (start, end) = (notes[at].start, notes[at].start + notes[at].length);
            let (mut from, mut before) = (start.max(floor), 0_f64);
            for (anchor, anchor_end) in &fixed {
                if from + CLOSEST > *anchor && from < *anchor_end {
                    // (to the nearer side that has room: past a long anchor is a long way from where it was)
                    let back = *anchor - CLOSEST;
                    let fits = back >= floor && back >= before && back >= 0.;
                    from = if fits && from - back <= *anchor_end - from { back } else { *anchor_end };
                }
                before = *anchor_end;
            }
            if from != start {
                notes[at].start = from;
                notes[at].length = (end - from).max(CLOSEST);
            }
            floor = from + CLOSEST;
        }
        // (the tail back inside the take, if that pushed it past the end: up to the last anchor, or if that ends the take, up to it and
        // before it)
        let (mut ceiling, after) = match fixed.as_slice() {
            [.., before, (anchor, end)] if end + CLOSEST > last => (*anchor, before.1),
            [(anchor, end)] if end + CLOSEST > last => (*anchor, 0.),
            [.., (_, end)] => (last, *end),
            [] => (last, 0.),
        };
        for at in run.iter().rev().copied().filter(|at| !stays[*at]) {
            let from = ceiling - CLOSEST;
            if notes[at].start <= from || from < after {
                break;
            }
            notes[at].start = from;
            notes[at].length = notes[at].length.min(ceiling - from);
            ceiling = from;
        }
        // (then each ends in time, as written, each to the 10 000th, and a 10 000th short of the next: Live refuses a note that ends
        // past it by any amount)
        run.sort_by(|a, b| notes[*a].start.total_cmp(&notes[*b].start));
        for pair in run.windows(2) {
            let (first, second) = (pair[0], pair[1]);
            let room = round(round(notes[second].start) - round(notes[first].start)) - 1e-4;
            if !stays[first] && round(notes[first].length) > room {
                notes[first].length = room;
            }
        }
    }
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
mod timing_tests {
    use super::*;

    fn note(pitch: i64, start: f64, length: f64) -> Note {
        Note { pitch, start, length, velocity: 90, channel: 1 }
    }

    fn loosened(mut notes: Vec<Note>, anchors: &[Note], seed: u64) -> Vec<Note> {
        note_op("timing", &json!({"op": "timing", "amount": 1}), 1., &mut notes, anchors, &mut Rng::new(seed)).unwrap().unwrap();
        notes
    }

    /// Loosened, no note ends past where the take's last did: a legato line to its item's end fills its source, and a DAW may refuse a
    /// note past it (REAPER does, and the whole take with it).
    #[test]
    fn loosened_notes_stay_inside_what_the_take_held() {
        let legato: Vec<Note> = (0..8).map(|k| note(48 + k, k as f64 * 0.5, 0.5)).collect();
        for seed in 0..64 {
            for moved in loosened(legato.clone(), &[], seed) {
                assert!(moved.start + moved.length <= 4. + 1e-9 && moved.start >= 0., "seed {seed}: {moved:?}");
            }
        }
    }

    /// Loosened, notes of one pitch and channel don't overlap: a note-off of that pitch would stop the later one too. None goes, and an
    /// anchor stays as it is.
    #[test]
    fn loosened_notes_of_one_pitch_never_overlap_and_anchors_stay() {
        let hats: Vec<Note> = (0..16).map(|k| note(42, k as f64 * 0.25, 0.25)).collect();
        let anchors = [hats[4].clone(), hats[9].clone()];
        for seed in 0..64 {
            let moved = loosened(hats.clone(), &anchors, seed);
            assert_eq!(moved.len(), 16);
            assert!(anchors.iter().all(|anchor| moved.contains(anchor)), "seed {seed}: {moved:?}");
            let mut sorted = moved.clone();
            sorted.sort_by(|a, b| a.start.total_cmp(&b.start));
            // (as a DAW is sent them, each to the 10 000th: real Live refused a take whose note ended past the next by that rounding)
            for pair in sorted.windows(2) {
                assert!(round(pair[0].start) + round(pair[0].length) <= round(pair[1].start), "seed {seed}: {pair:?}");
                assert!(pair[0].length > 0., "seed {seed}: {pair:?}");
            }
        }
        // Rolls with anchors in them, down to 128ths: a run pushed up against an anchor comes back from it (real Live refuses an overlap).
        for (step, name) in
            [(0.0625, "32nds"), (1. / 6., "triplets"), (0.0625 / 2., "64ths"), (0.0625 / 4., "128ths"), (0.0625 / 8., "256ths")]
        {
            let roll: Vec<Note> = (0..48).map(|k| note(38, k as f64 * step, step)).collect();
            let anchors = [roll[6].clone(), roll[7].clone(), roll[20].clone(), roll[33].clone()];
            for seed in 0..512 {
                let moved = loosened(roll.clone(), &anchors, seed);
                assert_eq!(moved.len(), roll.len());
                assert!(anchors.iter().all(|anchor| moved.contains(anchor)), "{name}, seed {seed}: {moved:?}");
                let mut sorted = moved.clone();
                sorted.sort_by(|a, b| a.start.total_cmp(&b.start));
                for pair in sorted.windows(2) {
                    assert!(round(pair[0].start) + round(pair[0].length) <= round(pair[1].start) + 1e-9, "{name}, seed {seed}: {pair:?}");
                    assert!(pair[0].length > 0., "{name}, seed {seed}: {pair:?}");
                }
                let end = sorted.iter().map(|note| note.start + note.length).fold(0., f64::max);
                // (a roll denser than notes may start apart has no room for that)
                assert!(step < CLOSEST || end <= 48. * step + 1e-9, "{name}, seed {seed}: ends at {end}");
            }
        }
        // Around anchors, a note stays near where it was and inside the take: a run to an anchored last note, a flam, and a pickup before a
        // long note, where going past the anchor would move it a long way or out of the take.
        let run: Vec<Note> = (0..32).map(|k| note(38, k as f64 * 0.03125, 0.03125)).collect();
        let flam: Vec<Note> = (1..5).flat_map(|k| [note(38, k as f64 - 0.03, 0.03), note(38, k as f64, 0.25)]).collect();
        let pickup = vec![note(38, 0., 0.25), note(38, 0.9375, 0.0625), note(38, 1., 2.)];
        for (name, played, anchors, furthest) in [
            ("to an anchored last note", run.clone(), vec![run[31].clone()], 0.125),
            ("flams", flam.clone(), flam.iter().skip(1).step_by(2).cloned().collect(), 0.0601),
            ("a pickup", pickup.clone(), vec![pickup[2].clone()], 0.0601),
        ] {
            let end = played.iter().map(|note| note.start + note.length).fold(0., f64::max);
            for seed in 0..512 {
                let moved = loosened(played.clone(), &anchors, seed);
                assert!(anchors.iter().all(|anchor| moved.contains(anchor)), "{name}, seed {seed}: {moved:?}");
                for (now, was) in moved.iter().zip(&played) {
                    assert!((now.start - was.start).abs() <= furthest, "{name}, seed {seed}: {was:?} moved to {now:?}");
                    assert!(now.start + now.length <= end + 1e-9, "{name}, seed {seed}: {now:?} ends past {end}");
                }
                let mut sorted = moved.clone();
                sorted.sort_by(|a, b| a.start.total_cmp(&b.start));
                for pair in sorted.windows(2) {
                    assert!(round(pair[0].start) + round(pair[0].length) <= round(pair[1].start) + 1e-9, "{name}, seed {seed}: {pair:?}");
                    assert!(pair[0].length > 0., "{name}, seed {seed}: {pair:?}");
                }
            }
        }
        // Other pitches and channels may overlap as they like.
        let chord = vec![note(60, 0., 1.), note(64, 0., 1.), Note { channel: 2, ..note(60, 0.5, 1.) }];
        let moved = loosened(chord.clone(), &[], 3);
        assert!(moved.iter().zip(&chord).all(|(a, b)| a.length == b.length), "{moved:?}");
    }
}

/// For a backend's tests: each note direction makes a take of an ordinary riff and of a short figure (their first note the
/// anchor) that passes core's checks, drawn as the tool draws it (bolder after a near-copy, milder after one too far) and clear
/// of the takes before it: a make of five gets five takes. `material` makes the backend's material of notes and anchors,
/// `apply` is its operators', `notes` its notes.
#[cfg(any(test, feature = "test-support"))]
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
        let hidden = [note(42, -1.), Note { length: 0.75, ..note(42, -0.25) }];
        for hidden_note in &hidden {
            assert_eq!(Note::of(&hidden_note.json()), Some(hidden_note.clone()), "a kept negative start comes back whole");
        }
        for op in ["pitch", "velocity", "timing", "thin", "octave"] {
            let mut notes = vec![hidden[0].clone(), hidden[1].clone(), note(42, 0.5), note(42, 1.5)];
            note_op(op, &json!({"op": op, "up": true}), 1., &mut notes, &[], &mut Rng::new(3)).unwrap().unwrap();
            assert!(hidden.iter().all(|note| notes.contains(note)), "{op} leaves the later passes' hidden notes unchanged: {notes:?}");
        }
        assert_eq!(notes_apart(&hidden, &hidden), 0., "negative starts need no special offset to compare them");
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
