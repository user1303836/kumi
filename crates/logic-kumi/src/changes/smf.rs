//! A Standard MIDI File for `midi-import`: format 0, one track, 480 ticks a quarter note, the notes as note-on and
//! note-off pairs at their beats from the file's start (Logic places the file at the playhead), no tempo or time
//! signature events (Logic asks whether to import a tempo, and the bridge answers no), no running status. What the
//! bridge writes under a folder it can read, for Logic's File > Import > MIDI File… to take.

/// Ticks a quarter note in the files the bridge writes.
pub const TICKS_PER_QUARTER: u32 = 480;

/// A note as `midi-import` takes it: beats from the file's start, a length in beats, a channel from 1.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Note {
    pub pitch: u8,
    pub start: f64,
    pub length: f64,
    pub velocity: u8,
    pub channel: u8,
}

/// One MIDI event at an absolute tick: note-offs sort before note-ons at the same tick, so a note repeated on the
/// beat isn't cut short by its own successor.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct Event {
    tick: u64,
    /// 0 for a note-off, 1 for a note-on: the order at one tick.
    order: u8,
    status: u8,
    pitch: u8,
    velocity: u8,
}

fn ticks(beats: f64) -> u64 {
    (beats.max(0.0) * f64::from(TICKS_PER_QUARTER)).round() as u64
}

/// A variable-length quantity, as MIDI files write delta times.
fn vlq(mut value: u64, out: &mut Vec<u8>) {
    let mut bytes = vec![(value & 0x7F) as u8];
    value >>= 7;
    while value > 0 {
        bytes.push((value & 0x7F) as u8 | 0x80);
        value >>= 7;
    }
    bytes.reverse();
    out.extend_from_slice(&bytes);
}

/// The file's bytes.
pub fn format0(notes: &[Note]) -> Vec<u8> {
    let mut events = Vec::with_capacity(notes.len() * 2);
    for note in notes {
        let channel = note.channel.clamp(1, 16) - 1;
        let start = ticks(note.start);
        let end = start + ticks(note.length).max(1);
        events.push(Event {
            tick: start,
            order: 1,
            status: 0x90 | channel,
            pitch: note.pitch & 0x7F,
            velocity: note.velocity.clamp(1, 127),
        });
        events.push(Event { tick: end, order: 0, status: 0x80 | channel, pitch: note.pitch & 0x7F, velocity: 0 });
    }
    events.sort();
    let mut track = Vec::with_capacity(events.len() * 5 + 4);
    let mut at = 0;
    for event in &events {
        vlq(event.tick - at, &mut track);
        at = event.tick;
        track.extend_from_slice(&[event.status, event.pitch, event.velocity]);
    }
    track.extend_from_slice(&[0x00, 0xFF, 0x2F, 0x00]);
    let mut out = Vec::with_capacity(track.len() + 22);
    out.extend_from_slice(b"MThd");
    out.extend_from_slice(&6u32.to_be_bytes());
    out.extend_from_slice(&0u16.to_be_bytes());
    out.extend_from_slice(&1u16.to_be_bytes());
    out.extend_from_slice(&(TICKS_PER_QUARTER as u16).to_be_bytes());
    out.extend_from_slice(b"MTrk");
    out.extend_from_slice(&(track.len() as u32).to_be_bytes());
    out.extend_from_slice(&track);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_note_is_the_hand_made_file_byte_for_byte() {
        let file = format0(&[Note { pitch: 60, start: 0.0, length: 1.0, velocity: 100, channel: 1 }]);
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

    #[test]
    fn notes_are_ordered_by_tick_with_offs_before_ons_and_deltas_as_variable_length_quantities() {
        // Two notes of the same pitch back to back on channel 2, the second starting as the first ends, and a chord
        // tone starting later.
        let file = format0(&[
            Note { pitch: 64, start: 1.0, length: 0.5, velocity: 90, channel: 2 },
            Note { pitch: 60, start: 0.0, length: 1.0, velocity: 100, channel: 2 },
            Note { pitch: 60, start: 1.0, length: 2.0, velocity: 80, channel: 2 },
        ]);
        let track = &file[22..];
        assert_eq!(&track[..4], &[0x00, 0x91, 0x3C, 0x64], "C3 on first");
        // At tick 480: the first C3 off (0x81), then the second C3 on, then E3 on.
        assert_eq!(&track[4..9], &[0x83, 0x60, 0x81, 0x3C, 0x00]);
        assert_eq!(&track[9..13], &[0x00, 0x91, 0x3C, 0x50]);
        assert_eq!(&track[13..17], &[0x00, 0x91, 0x40, 0x5A]);
        // E3 off at 720 (delta 240), C3 off at 1440 (delta 720 = 0x85 0x50).
        assert_eq!(&track[17..22], &[0x81, 0x70, 0x81, 0x40, 0x00]);
        assert_eq!(&track[22..27], &[0x85, 0x50, 0x81, 0x3C, 0x00]);
        assert_eq!(&track[27..], &[0x00, 0xFF, 0x2F, 0x00]);
        let mut out = Vec::new();
        vlq(0, &mut out);
        vlq(127, &mut out);
        vlq(128, &mut out);
        vlq(0x1F_FFFF, &mut out);
        assert_eq!(out, [0x00, 0x7F, 0x81, 0x00, 0xFF, 0xFF, 0x7F]);
        assert_eq!(ticks(0.25), 120);
        assert_eq!(ticks(-1.0), 0);
        assert_eq!(format0(&[]).len(), 22 + 4);
    }
}
