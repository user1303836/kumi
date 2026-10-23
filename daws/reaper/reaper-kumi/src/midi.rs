//! A MIDI take's events as REAPER's `MIDI_GetAllEvts` and `MIDI_SetAllEvts` hand them over: one buffer of `{ i32 offset, u8
//! flag, i32 length, bytes }` entries, the offset in ticks from the event before. Editing notes through that buffer is one
//! call each way, where inserting and deleting them one at a time takes a millisecond or two each in a take of thousands.
//! Every event that is not a note Kumi touches goes back byte for byte, in its place.

use std::collections::HashMap;

/// An event with its position in ticks from the start of the take.
#[derive(Debug, Clone, PartialEq)]
pub struct Event {
    pub pos: i64,
    /// `&1` selected, `&2` muted (on a note, both its events carry them), the high bits a CC's shape.
    pub flag: u8,
    pub msg: Vec<u8>,
}

/// A note by what tells it apart: channel (0 to 15), pitch, where it starts and ends, how hard, and whether it is muted;
/// and how hard it was let go (its note-off's velocity, which a recorded note carries).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NoteSpec {
    pub channel: u8,
    pub pitch: u8,
    pub start: i64,
    pub end: i64,
    pub velocity: u8,
    pub muted: bool,
    pub selected: bool,
    pub off_velocity: u8,
}

impl NoteSpec {
    /// The note as `MIDI_GetNote` gives one: channel 0 to 15, positions in ticks.
    pub fn midi(&self) -> crate::reaper::MidiNote {
        crate::reaper::MidiNote {
            selected: self.selected,
            muted: self.muted,
            start_ppq: self.start as f64,
            end_ppq: self.end as f64,
            channel: self.channel as i32,
            pitch: self.pitch as i32,
            velocity: self.velocity as i32,
        }
    }
}

pub fn decode(buf: &[u8]) -> Option<Vec<Event>> {
    let (mut at, mut pos) = (0usize, 0i64);
    let mut events = Vec::new();
    while at < buf.len() {
        let header = buf.get(at..at + 9)?;
        let offset = i32::from_le_bytes(header[0..4].try_into().ok()?);
        let len = i32::from_le_bytes(header[5..9].try_into().ok()?);
        if offset < 0 || len < 0 {
            return None;
        }
        let msg = buf.get(at + 9..at + 9 + len as usize)?.to_vec();
        pos += offset as i64;
        events.push(Event { pos, flag: header[4], msg });
        at += 9 + len as usize;
    }
    Some(events)
}

pub fn encode(events: &[Event]) -> Vec<u8> {
    let mut buf = Vec::with_capacity(events.iter().map(|e| 9 + e.msg.len()).sum());
    let mut pos = 0i64;
    for event in events {
        buf.extend_from_slice(&((event.pos - pos) as i32).to_le_bytes());
        buf.push(event.flag);
        buf.extend_from_slice(&(event.msg.len() as i32).to_le_bytes());
        buf.extend_from_slice(&event.msg);
        pos = event.pos;
    }
    buf
}

fn is_on(event: &Event) -> bool {
    matches!(event.msg.as_slice(), [status, _, velocity] if status & 0xF0 == 0x90 && *velocity > 0)
}

fn is_off(event: &Event) -> bool {
    matches!(event.msg.as_slice(), [status, _, velocity] if status & 0xF0 == 0x80 || (status & 0xF0 == 0x90 && *velocity == 0))
}

fn flag_of(note: &NoteSpec) -> u8 {
    note.selected as u8 | (note.muted as u8) << 1
}

/// The take's notes, with their events paired the way `remove` pairs them: a note-on with the first note-off after it of the
/// same channel and pitch.
pub fn notes(events: &[Event]) -> Vec<NoteSpec> {
    let mut open: HashMap<(u8, u8), Vec<usize>> = HashMap::new();
    let mut out: Vec<(usize, NoteSpec)> = Vec::new();
    for (at, event) in events.iter().enumerate() {
        let (channel, pitch) = match event.msg.as_slice() {
            [status, pitch, _] => (status & 0x0F, *pitch),
            _ => continue,
        };
        if is_on(event) {
            open.entry((channel, pitch)).or_default().push(out.len());
            out.push((
                at,
                NoteSpec {
                    channel,
                    pitch,
                    start: event.pos,
                    end: event.pos,
                    velocity: event.msg[2],
                    muted: event.flag & 2 != 0,
                    selected: event.flag & 1 != 0,
                    off_velocity: 0,
                },
            ));
        } else if is_off(event) {
            if let Some(list) = open.get_mut(&(channel, pitch)).filter(|l| !l.is_empty()) {
                let note = list.remove(0);
                out[note].1.end = event.pos;
                // (a note-on of velocity 0 ends a note too, let go of with none)
                out[note].1.off_velocity = if event.msg[0] & 0xF0 == 0x80 { event.msg[2] } else { 0 };
            }
        }
    }
    out.into_iter().map(|(_, note)| note).collect()
}

/// Takes the notes in `remove` out and puts the notes in `add` in, leaving every other event as it was. Nothing is changed
/// when a note to remove isn't there as given.
pub fn edit_notes(events: &[Event], remove: &[NoteSpec], add: &[NoteSpec]) -> Result<Vec<Event>, String> {
    // Where each note-on and note-off is, by what it says.
    let mut ons: HashMap<(u8, u8, i64, u8, bool), Vec<usize>> = HashMap::new();
    let mut offs: HashMap<(u8, u8, i64), Vec<usize>> = HashMap::new();
    for (at, event) in events.iter().enumerate() {
        if let [status, pitch, velocity] = event.msg.as_slice() {
            let channel = status & 0x0F;
            if is_on(event) {
                ons.entry((channel, *pitch, event.pos, *velocity, event.flag & 2 != 0)).or_default().push(at);
            } else if is_off(event) {
                offs.entry((channel, *pitch, event.pos)).or_default().push(at);
            }
        }
    }
    let mut gone = vec![false; events.len()];
    for note in remove {
        let on = ons
            .get_mut(&(note.channel, note.pitch, note.start, note.velocity, note.muted))
            .and_then(|list| (!list.is_empty()).then(|| list.remove(0)))
            .ok_or_else(|| format!("a note ({} at tick {}) isn't in the take as given", note.pitch, note.start))?;
        let off = offs
            .get_mut(&(note.channel, note.pitch, note.end))
            .and_then(|list| list.iter().position(|&at| at > on).map(|n| list.remove(n)))
            .ok_or_else(|| format!("a note ({} at tick {}) has no end where it should", note.pitch, note.start))?;
        gone[on] = true;
        gone[off] = true;
    }
    // New note-offs go before what is already at their tick and new note-ons after it, so that a note never ends after
    // another begins on the same tick; the rest keeps the order it had.
    let mut keyed: Vec<((i64, u8, usize), Event)> =
        events.iter().enumerate().filter(|(at, _)| !gone[*at]).map(|(at, event)| ((event.pos, 1, at), event.clone())).collect();
    for (n, note) in add.iter().enumerate() {
        let (flag, order) = (flag_of(note), events.len() + n);
        keyed.push(((note.start, 2, order), Event { pos: note.start, flag, msg: vec![0x90 | note.channel, note.pitch, note.velocity] }));
        keyed.push(((note.end, 0, order), Event { pos: note.end, flag, msg: vec![0x80 | note.channel, note.pitch, note.off_velocity] }));
    }
    keyed.sort_by_key(|(key, _)| *key);
    Ok(keyed.into_iter().map(|(_, event)| event).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn note(channel: u8, pitch: u8, start: i64, end: i64) -> NoteSpec {
        NoteSpec { channel, pitch, start, end, velocity: 90, muted: false, selected: false, off_velocity: 0 }
    }

    fn take() -> Vec<Event> {
        let cc = |pos, value| Event { pos, flag: 0, msg: vec![0xB0, 7, value] };
        let on = |pos, pitch, flag| Event { pos, flag, msg: vec![0x90, pitch, 90] };
        let off = |pos, pitch, flag| Event { pos, flag, msg: vec![0x80, pitch, 0] };
        vec![cc(0, 1), on(0, 60, 0), off(480, 60, 0), on(480, 62, 1), cc(480, 2), off(960, 62, 1), on(960, 64, 2), off(1440, 64, 2)]
    }

    #[test]
    fn a_buffer_round_trips_byte_for_byte() {
        let events = take();
        let buf = encode(&events);
        assert_eq!(decode(&buf), Some(events));
        assert_eq!(decode(&buf[..buf.len() - 1]), None, "a cut buffer isn't read");
        assert_eq!(decode(&[]), Some(vec![]));
    }

    #[test]
    fn the_notes_are_read_from_their_pairs_with_selection_and_mute() {
        let notes = notes(&take());
        assert_eq!(notes.len(), 3);
        assert_eq!((notes[1].pitch, notes[1].start, notes[1].end, notes[1].selected, notes[1].muted), (62, 480, 960, true, false));
        assert_eq!((notes[2].selected, notes[2].muted), (false, true));
    }

    #[test]
    fn a_note_goes_out_and_another_comes_in_and_everything_else_stays_where_it_was() {
        let events = take();
        let out = edit_notes(&events, &[note(0, 60, 0, 480)], &[note(1, 70, 480, 720)]).unwrap();
        assert_eq!(out.len(), events.len());
        // The controller events are as they were; the note-off at 480 of the new note sits before what is on that tick.
        assert_eq!(out.iter().filter(|e| e.msg[0] == 0xB0).count(), 2);
        let notes = notes(&out);
        assert_eq!(notes.iter().map(|n| (n.channel, n.pitch)).collect::<Vec<_>>(), [(0, 62), (1, 70), (0, 64)]);
        assert!(out.windows(2).all(|pair| pair[0].pos <= pair[1].pos));
        // Not there as given: refused, and nothing changes.
        assert!(edit_notes(&events, &[note(0, 61, 0, 480)], &[]).is_err());
        assert!(edit_notes(&events, &[note(0, 60, 0, 481)], &[]).is_err());
    }

    #[test]
    fn a_notes_release_velocity_is_read_and_written_back() {
        // Every note Kumi wrote again ended with velocity 0: a recorded release velocity was lost to a change, a quantize
        // or a humanize, and its undo.
        let events = vec![
            Event { pos: 0, flag: 0, msg: vec![0x90, 60, 90] },
            Event { pos: 480, flag: 0, msg: vec![0x80, 60, 64] },
            Event { pos: 480, flag: 0, msg: vec![0x91, 62, 90] },
            Event { pos: 960, flag: 0, msg: vec![0x91, 62, 0] },
        ];
        let read = notes(&events);
        assert_eq!((read[0].off_velocity, read[1].off_velocity), (64, 0));
        let mut moved = read[0];
        moved.start += 10;
        let out = edit_notes(&events, &read[..1], &[moved]).unwrap();
        assert!(out.contains(&Event { pos: 480, flag: 0, msg: vec![0x80, 60, 64] }), "{out:?}");
        assert_eq!(notes(&out)[0].off_velocity, 64);
    }

    #[test]
    fn equal_notes_are_taken_one_at_a_time_and_a_new_note_ends_before_another_begins_on_the_same_tick() {
        let two = [note(0, 60, 0, 480), note(0, 60, 0, 480)];
        let events = edit_notes(&[], &[], &two).unwrap();
        assert_eq!(events.len(), 4);
        let one_left = edit_notes(&events, &two[..1], &[]).unwrap();
        assert_eq!(one_left.len(), 2);
        // A note ending on the tick another begins: the end first.
        let events = edit_notes(&[], &[], &[note(0, 60, 0, 480), note(0, 60, 480, 960)]).unwrap();
        let order: Vec<(i64, u8)> = events.iter().map(|e| (e.pos, e.msg[0] & 0xF0)).collect();
        assert_eq!(order, [(0, 0x90), (480, 0x80), (480, 0x90), (960, 0x80)]);
    }
}
