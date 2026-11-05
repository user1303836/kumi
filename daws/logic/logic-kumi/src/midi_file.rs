//! A pure Standard MIDI File encoder: format 0, one track, 480 ticks per quarter note, explicit note-on/off pairs,
//! no tempo or time-signature events, and no running status. It returns bytes and performs no I/O or import.
//!
//! Derived from `6f8f6e9bbc3666159ce68be1ce7466aa30e82670:crates/logic-kumi/src/changes/smf.rs` (Logic M4a).
//! Valid inputs keep the original bytes. Invalid ranges and unrepresentable timing are rejected instead of being
//! clamped, wrapped or written as a nonstandard MIDI delta time.

use std::fmt;

/// Ticks per quarter note in the returned file.
pub const TICKS_PER_QUARTER: u32 = 480;
/// The original M4a import's note budget, also bounding this encoder's allocations and track-chunk size.
pub const MAX_NOTES: usize = 10_000;
/// Standard MIDI File delta times use at most four bytes, seven data bits each.
const MAX_DELTA: u32 = 0x0FFF_FFFF;

/// One note: beats from the file's start, length in beats, and a MIDI channel numbered from 1.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Note {
    pub pitch: u8,
    pub start: f64,
    pub length: f64,
    pub velocity: u8,
    pub channel: u8,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    TooManyNotes(usize),
    /// `index` is zero-based, so a caller can identify the rejected input note without changing its order.
    InvalidNote {
        index: usize,
        reason: &'static str,
    },
    DeltaTooLong(u64),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooManyNotes(count) => write!(f, "{count} notes exceed the MIDI file limit of {MAX_NOTES}"),
            Self::InvalidNote { index, reason } => write!(f, "note {}: {reason}", index + 1),
            Self::DeltaTooLong(ticks) => write!(f, "a MIDI delta of {ticks} ticks exceeds the four-byte limit of {MAX_DELTA}"),
        }
    }
}

impl std::error::Error for Error {}

/// Note-offs sort before note-ons at the same tick: repeating a pitch on the beat does not cut off the new note.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct Event {
    tick: u64,
    order: u8,
    status: u8,
    pitch: u8,
    velocity: u8,
}

fn ticks(beats: f64) -> Option<u64> {
    let ticks = (beats * f64::from(TICKS_PER_QUARTER)).round();
    // u64::MAX rounds up to 2^64 as f64: use a strict bound before the float-to-integer conversion.
    (beats.is_finite() && beats >= 0.0 && ticks.is_finite() && ticks < u64::MAX as f64).then(|| ticks as u64)
}

fn vlq(mut value: u32, out: &mut Vec<u8>) {
    debug_assert!(value <= MAX_DELTA);
    let mut bytes = [0u8; 4];
    let mut start = 3;
    bytes[start] = (value & 0x7F) as u8;
    value >>= 7;
    while value > 0 {
        start -= 1;
        bytes[start] = (value & 0x7F) as u8 | 0x80;
        value >>= 7;
    }
    out.extend_from_slice(&bytes[start..]);
}

/// Encode the notes in time order. An empty sequence is a valid file containing only End of Track.
///
/// Starts and lengths are rounded to the nearest 1/480 beat. A positive length that rounds to zero is refused;
/// pitch, velocity and channel must already be valid MIDI values. No invalid input is silently changed.
pub fn format0(notes: &[Note]) -> Result<Vec<u8>, Error> {
    if notes.len() > MAX_NOTES {
        return Err(Error::TooManyNotes(notes.len()));
    }
    let mut events = Vec::with_capacity(notes.len() * 2);
    for (index, note) in notes.iter().enumerate() {
        let invalid = |reason| Error::InvalidNote { index, reason };
        if note.pitch > 127 {
            return Err(invalid("pitch must be from 0 to 127"));
        }
        if !(1..=127).contains(&note.velocity) {
            return Err(invalid("velocity must be from 1 to 127"));
        }
        if !(1..=16).contains(&note.channel) {
            return Err(invalid("channel must be from 1 to 16"));
        }
        let start = ticks(note.start).ok_or_else(|| invalid("start must be a finite, nonnegative representable beat"))?;
        let length = ticks(note.length)
            .filter(|length| *length > 0)
            .ok_or_else(|| invalid("length must be a finite, positive beat that rounds to at least one tick"))?;
        let end = start.checked_add(length).ok_or_else(|| invalid("the note end exceeds the supported tick range"))?;
        let channel = note.channel - 1;
        events.push(Event { tick: start, order: 1, status: 0x90 | channel, pitch: note.pitch, velocity: note.velocity });
        events.push(Event { tick: end, order: 0, status: 0x80 | channel, pitch: note.pitch, velocity: 0 });
    }
    events.sort();
    // At most four delta bytes and three event bytes, twice per note, plus End of Track.
    let mut track = Vec::with_capacity(events.len() * 7 + 4);
    let mut at = 0;
    for event in &events {
        let delta = event.tick - at;
        if delta > u64::from(MAX_DELTA) {
            return Err(Error::DeltaTooLong(delta));
        }
        vlq(delta as u32, &mut track);
        at = event.tick;
        track.extend_from_slice(&[event.status, event.pitch, event.velocity]);
    }
    track.extend_from_slice(&[0x00, 0xFF, 0x2F, 0x00]);
    let track_length = u32::try_from(track.len()).expect("at most 10,000 pairs of seven-byte MIDI events");
    let mut out = Vec::with_capacity(track.len() + 22);
    out.extend_from_slice(b"MThd");
    out.extend_from_slice(&6u32.to_be_bytes());
    out.extend_from_slice(&0u16.to_be_bytes());
    out.extend_from_slice(&1u16.to_be_bytes());
    out.extend_from_slice(&(TICKS_PER_QUARTER as u16).to_be_bytes());
    out.extend_from_slice(b"MTrk");
    out.extend_from_slice(&track_length.to_be_bytes());
    out.extend_from_slice(&track);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOTE: Note = Note { pitch: 60, start: 0.0, length: 1.0, velocity: 100, channel: 1 };

    /// This independent hand-made expected file is unchanged from the original M4a test.
    #[test]
    fn one_note_is_the_hand_made_file_byte_for_byte() {
        let file = format0(&[NOTE]).unwrap();
        let expected: Vec<u8> = vec![
            0x4D, 0x54, 0x68, 0x64, 0x00, 0x00, 0x00, 0x06, 0x00, 0x00, 0x00, 0x01, 0x01,
            0xE0, // MThd, format 0, one track, 480 ticks
            0x4D, 0x54, 0x72, 0x6B, 0x00, 0x00, 0x00, 0x0D, // MTrk, 13 bytes
            0x00, 0x90, 0x3C, 0x64, // at 0: note on C3, velocity 100
            0x83, 0x60, 0x80, 0x3C, 0x00, // 480 ticks later: note off
            0x00, 0xFF, 0x2F, 0x00, // end of track
        ];
        assert_eq!(file, expected);
    }

    /// The original M4a multi-note case: an unsorted repeated pitch and a chord tone on channel 2.
    #[test]
    fn notes_are_ordered_by_tick_with_offs_before_ons_and_deltas_as_variable_length_quantities() {
        let file = format0(&[
            Note { pitch: 64, start: 1.0, length: 0.5, velocity: 90, channel: 2 },
            Note { pitch: 60, start: 0.0, length: 1.0, velocity: 100, channel: 2 },
            Note { pitch: 60, start: 1.0, length: 2.0, velocity: 80, channel: 2 },
        ])
        .unwrap();
        let track = &file[22..];
        assert_eq!(&track[..4], &[0x00, 0x91, 0x3C, 0x64], "C3 on first");
        assert_eq!(&track[4..9], &[0x83, 0x60, 0x81, 0x3C, 0x00], "old C3 off before its replacement starts");
        assert_eq!(&track[9..13], &[0x00, 0x91, 0x3C, 0x50]);
        assert_eq!(&track[13..17], &[0x00, 0x91, 0x40, 0x5A]);
        assert_eq!(&track[17..22], &[0x81, 0x70, 0x81, 0x40, 0x00]);
        assert_eq!(&track[22..27], &[0x85, 0x50, 0x81, 0x3C, 0x00]);
        assert_eq!(&track[27..], &[0x00, 0xFF, 0x2F, 0x00]);
        assert_eq!(u32::from_be_bytes(file[18..22].try_into().unwrap()) as usize, track.len());
        let mut out = Vec::new();
        for value in [0, 127, 128, 0x1F_FFFF, MAX_DELTA] {
            vlq(value, &mut out);
        }
        assert_eq!(out, [0x00, 0x7F, 0x81, 0x00, 0xFF, 0xFF, 0x7F, 0xFF, 0xFF, 0xFF, 0x7F]);
    }

    #[test]
    fn invalid_note_values_are_refused_without_changing_their_meaning() {
        for note in [
            Note { pitch: 128, ..NOTE },
            Note { velocity: 0, ..NOTE },
            Note { velocity: 128, ..NOTE },
            Note { channel: 0, ..NOTE },
            Note { channel: 17, ..NOTE },
            Note { start: -1.0, ..NOTE },
            Note { start: f64::NAN, ..NOTE },
            Note { start: f64::INFINITY, ..NOTE },
            Note { start: f64::MAX, ..NOTE },
            Note { length: -1.0, ..NOTE },
            Note { length: 0.0, ..NOTE },
            Note { length: 1e-12, ..NOTE },
            Note { length: f64::NAN, ..NOTE },
            Note { length: f64::INFINITY, ..NOTE },
            Note { length: f64::MAX, ..NOTE },
        ] {
            assert!(matches!(format0(&[NOTE, note]), Err(Error::InvalidNote { index: 1, .. })), "{note:?}");
        }
        let far = u64::MAX as f64 / f64::from(TICKS_PER_QUARTER) * 0.75;
        assert!(matches!(format0(&[Note { start: far, length: far, ..NOTE }]), Err(Error::InvalidNote { index: 0, .. })));
    }

    #[test]
    fn standard_midi_delta_limits_are_checked_before_any_file_is_returned() {
        let boundary =
            format0(&[Note { pitch: 127, velocity: 127, channel: 16, length: 1.0 / f64::from(TICKS_PER_QUARTER), ..NOTE }]).unwrap();
        assert_eq!(&boundary[22..], &[0x00, 0x9F, 0x7F, 0x7F, 0x01, 0x8F, 0x7F, 0x00, 0x00, 0xFF, 0x2F, 0x00]);
        let maximum = f64::from(MAX_DELTA) / f64::from(TICKS_PER_QUARTER);
        let file = format0(&[Note { start: maximum, length: 1.0 / f64::from(TICKS_PER_QUARTER), ..NOTE }]).unwrap();
        assert_eq!(&file[22..29], &[0xFF, 0xFF, 0xFF, 0x7F, 0x90, 0x3C, 0x64]);
        assert_eq!(&file[29..33], &[0x01, 0x80, 0x3C, 0x00]);
        let too_far = f64::from(MAX_DELTA + 1) / f64::from(TICKS_PER_QUARTER);
        for note in [Note { start: too_far, ..NOTE }, Note { length: too_far, ..NOTE }] {
            assert_eq!(format0(&[note]), Err(Error::DeltaTooLong(u64::from(MAX_DELTA) + 1)));
        }
    }

    #[test]
    fn the_note_budget_is_explicit_and_an_empty_track_is_valid() {
        let largest = format0(&vec![NOTE; MAX_NOTES]).unwrap();
        assert_eq!(u32::from_be_bytes(largest[18..22].try_into().unwrap()) as usize, largest.len() - 22);
        assert_eq!(format0(&vec![NOTE; MAX_NOTES + 1]), Err(Error::TooManyNotes(MAX_NOTES + 1)));
        assert_eq!(
            format0(&[]).unwrap(),
            [
                0x4D, 0x54, 0x68, 0x64, 0, 0, 0, 6, 0, 0, 0, 1, 0x01, 0xE0, // header
                0x4D, 0x54, 0x72, 0x6B, 0, 0, 0, 4, 0, 0xFF, 0x2F, 0 // track containing only End of Track
            ]
        );
    }
}
