//! Audio to MIDI (#401), built once for every DAW without a converter of its own: a track's audio over a stretch heard by
//! Kumi's own ears and written as notes. Drums by the hit detector the chop uses: each hit a kick, snare or hat on General
//! MIDI's drum keys, as loud as it hits against the loudest of its kind, a ghost softer. Pitched parts by Basic Pitch: every
//! note (harmony), or the highest one sounding at each moment (melody). The tool (`tool`) renders, hears and writes; the
//! DAW's backend (`backend`) renders the track and writes the notes on a new track.
pub mod backend;
pub mod tool;

#[cfg(test)]
mod fake_backend_tests;

use crate::core::chop::hits::{Hit, Role};
use crate::listening::transcribe::Heard;

/// A note as the DAW gets it: MIDI pitch, start and length in beats from the stretch's start, velocity 1 to 127.
#[derive(Debug, Clone, PartialEq)]
pub struct MidiNote {
    pub pitch: u8,
    pub start: f64,
    pub length: f64,
    pub velocity: u8,
}

/// General MIDI's drum keys for a kick, a snare and a closed hat.
pub const KICK: u8 = 36;
pub const SNARE: u8 = 38;
pub const HAT: u8 = 42;

/// How long a drum note is (beats: a 16th), at most.
const DRUM_LENGTH: f64 = 0.25;
/// The shortest leftover a melody keeps of a note that outlasts the one over it (beats: a 16th of a beat): a chord tone
/// ringing a few ms past the top note is no note of the melody.
const LEFTOVER: f64 = 1. / 16.;
/// A drum's velocity: its kind's loudest hit's, less this much a dB under it; a ghost at most `GHOST_MOST`.
const LOUDEST: f64 = 120.;
const PER_DB: f64 = 3.;
const GHOST_MOST: f64 = 60.;

pub fn key_of(role: Role) -> u8 {
    match role {
        Role::Kick => KICK,
        Role::Snare => SNARE,
        Role::Hat => HAT,
    }
}

/// The hits as drum notes, `per_second` beats a second, in the stretch's `beats`.
pub fn drums(hits: &[Hit], per_second: f64, beats: f64) -> Vec<MidiNote> {
    let loudest = |role: Role| hits.iter().filter(|hit| hit.role == role).map(|hit| hit.peak_db).fold(f64::NEG_INFINITY, f64::max);
    let mut notes: Vec<MidiNote> = hits
        .iter()
        .map(|hit| {
            let mut velocity = LOUDEST + (hit.peak_db - loudest(hit.role)) * PER_DB;
            if hit.ghost {
                velocity = velocity.min(GHOST_MOST);
            }
            MidiNote {
                pitch: key_of(hit.role),
                start: hit.seconds * per_second,
                length: DRUM_LENGTH,
                velocity: velocity.clamp(1., 127.).round() as u8,
            }
        })
        .filter(|note| note.start < beats)
        .collect();
    notes.sort_by(|a, b| a.start.total_cmp(&b.start).then(a.pitch.cmp(&b.pitch)));
    apart(notes)
}

/// Basic Pitch's notes as MIDI notes, `per_second` beats a second, in the stretch's `beats`: every one, or (`melody`) the
/// highest sounding at each moment: a lower one cut where a higher one starts over it, and one starting under a higher one
/// starts where that ends (Basic Pitch overlaps legato steps). Velocity from Basic Pitch's confidence (40 to 120).
pub fn pitched(heard: &[Heard], per_second: f64, beats: f64, melody: bool) -> Vec<MidiNote> {
    let mut notes: Vec<MidiNote> = heard
        .iter()
        .map(|note| MidiNote {
            pitch: note.pitch,
            start: note.start * per_second,
            length: ((note.end - note.start) * per_second).max(1. / 64.),
            velocity: (40. + 80. * note.strength.clamp(0., 1.) as f64).round() as u8,
        })
        .filter(|note| note.start < beats)
        .collect();
    notes.sort_by(|a, b| a.start.total_cmp(&b.start).then(b.pitch.cmp(&a.pitch)));
    if melody {
        notes = skyline(notes);
    }
    for note in &mut notes {
        note.length = note.length.min(beats - note.start);
    }
    apart(notes)
}

/// The highest note sounding at each moment: a note starting under a higher one starts where that one ends (or goes, when it
/// ends first or what's left is shorter than a 16th of a beat), and one a higher one starts over is cut there.
fn skyline(notes: Vec<MidiNote>) -> Vec<MidiNote> {
    let mut out: Vec<MidiNote> = Vec::new();
    for mut note in notes {
        let mut moved = false;
        loop {
            let over = out
                .iter()
                .filter(|kept| kept.pitch > note.pitch && kept.start <= note.start + 1e-9 && kept.start + kept.length > note.start + 1e-9)
                .map(|kept| kept.start + kept.length)
                .fold(f64::NAN, f64::max);
            if over.is_nan() {
                break;
            }
            let ends = note.start + note.length;
            note.length = ends - over;
            note.start = over;
            moved = true;
            if note.length <= 1e-9 {
                break;
            }
        }
        if note.length <= 1e-9 || (moved && note.length < LEFTOVER) {
            continue;
        }
        for kept in
            out.iter_mut().filter(|kept| kept.pitch < note.pitch && kept.start < note.start && kept.start + kept.length > note.start)
        {
            kept.length = (note.start - kept.start).max(1. / 64.);
        }
        out.push(note);
    }
    out.sort_by(|a, b| a.start.total_cmp(&b.start));
    out
}

/// Notes of one pitch apart: one is cut where the next of its pitch starts (a DAW refuses two of a pitch overlapping).
fn apart(mut notes: Vec<MidiNote>) -> Vec<MidiNote> {
    for k in 0..notes.len() {
        if let Some(next) = notes[k + 1..].iter().find(|later| later.pitch == notes[k].pitch).map(|later| later.start) {
            if notes[k].start + notes[k].length > next {
                notes[k].length = (next - notes[k].start).max(1. / 64.);
            }
        }
    }
    notes.retain(|note| note.length > 0.);
    notes
}

/// What the notes are, in a few words: how many of each drum, or the range of pitches.
pub fn said(notes: &[MidiNote], drums: bool, name: impl Fn(u8) -> String) -> String {
    if notes.is_empty() {
        return "no notes".into();
    }
    if drums {
        let count = |key: u8| notes.iter().filter(|note| note.pitch == key).count();
        let parts: Vec<String> = [("kick", KICK), ("snare", SNARE), ("hat", HAT)]
            .iter()
            .filter(|(_, key)| count(*key) > 0)
            .map(|(word, key)| format!("{} {word}{}", count(*key), if count(*key) == 1 { "" } else { "s" }))
            .collect();
        return format!("{} notes: {}", notes.len(), parts.join(", "));
    }
    let low = notes.iter().map(|note| note.pitch).min().unwrap_or(0);
    let high = notes.iter().map(|note| note.pitch).max().unwrap_or(0);
    format!("{} notes, {} to {}", notes.len(), name(low), name(high))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hit(seconds: f64, role: Role, peak_db: f64, ghost: bool) -> Hit {
        Hit { frame: (seconds * 48_000.) as usize, seconds, peak_db, role, ghost, lows: 0., highs: 0. }
    }

    #[test]
    fn hits_are_drum_notes_on_their_keys_as_loud_as_they_hit_and_a_ghost_softer() {
        let hits = vec![
            hit(0., Role::Kick, -3., false),
            hit(0.5, Role::Snare, -6., false),
            hit(0.75, Role::Snare, -20., true),
            hit(0.25, Role::Hat, -12., false),
        ];
        let notes = drums(&hits, 2., 4.);
        assert_eq!(
            notes.iter().map(|note| (note.pitch, note.start)).collect::<Vec<_>>(),
            [(KICK, 0.), (HAT, 0.5), (SNARE, 1.), (SNARE, 1.5)]
        );
        assert_eq!(notes[0].velocity, 120, "the loudest kick");
        assert_eq!(notes[3].velocity, 60, "a ghost, softer than 78");
        assert!(notes.iter().all(|note| note.length <= 0.25));
    }

    #[test]
    fn a_melody_is_the_highest_note_at_each_moment_and_notes_of_a_pitch_never_overlap() {
        let heard = |start: f64, end: f64, pitch: u8| Heard { start, end, pitch, strength: 0.5 };
        // (a chord C-E-G for a second, a high C over it from 0.5 s)
        let all = vec![heard(0., 1., 60), heard(0., 1., 64), heard(0., 1., 67), heard(0.5, 1.5, 72)];
        let harmony = pitched(&all, 2., 8., false);
        assert_eq!(harmony.len(), 4);
        assert_eq!(harmony[0].velocity, 80);
        let melody = pitched(&all, 2., 8., true);
        assert_eq!(melody.iter().map(|note| (note.pitch, note.start, note.length)).collect::<Vec<_>>(), [(67, 0., 1.), (72, 1., 2.)]);
        // (legato steps Basic Pitch overlaps: 72 over 0 to 1.05 s, then 70, then 69: each starts where the one above ends)
        let steps = pitched(&[heard(0., 1.05, 72), heard(1., 2., 70), heard(1.9, 3., 69)], 1., 8., true);
        assert_eq!(
            steps.iter().map(|note| (note.pitch, (note.start * 100.).round() / 100.)).collect::<Vec<_>>(),
            [(72, 0.), (70, 1.05), (69, 2.)]
        );
        // (a chord's tones ringing 4 to 8 ms past the top note leave nothing of themselves in the melody)
        let ringing = pitched(&[heard(0., 0.996, 72), heard(0., 1.0, 67), heard(0., 1.002, 64), heard(0., 1.004, 60)], 1., 8., true);
        assert_eq!(ringing.iter().map(|note| note.pitch).collect::<Vec<_>>(), [72]);
        // (one pitch twice, overlapping: the first is cut where the second starts)
        let again = pitched(&[heard(0., 1., 60), heard(0.5, 1., 60)], 2., 8., false);
        assert_eq!(again[0].length, 1.);
        assert_eq!(said(&again, false, |n| format!("n{n}")), "2 notes, n60 to n60");
    }
}
