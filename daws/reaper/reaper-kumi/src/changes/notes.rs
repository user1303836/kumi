//! Notes in a MIDI take: written, changed and deleted, many at a time. A note has no identity of its own in REAPER, so the
//! preview fences the take's whole note list (count and a hash), and an undo finds the notes it wrote by their values: it
//! refuses `changed` if any of them is not there as written, and never touches the other notes.

use std::collections::HashMap;

use serde_json::{json, Value};

use super::{Applied, Ctx, Done, Fault, Preview, Restore, Restored};
use crate::midi::Event;
use crate::read::{bad, failed, take_start_qn};
use crate::reaper::{MidiNote, Take};
use crate::refs;
use crate::rows;

/// The most notes one change may write, change or delete.
const MOST_NOTES: usize = 10_000;
/// The notes a preview or an answer shows.
const SHOWN: usize = 8;

fn changed(message: impl Into<String>) -> Fault {
    ("changed".into(), message.into())
}

#[derive(Clone, Copy, Debug)]
pub(super) struct Note {
    pub channel: i32,
    pub pitch: i32,
    pub start: f64,
    pub end: f64,
    pub velocity: i32,
    pub muted: bool,
    pub selected: bool,
    /// The note-off's velocity (how hard a recorded note was let go): kept through every rewrite.
    pub off_velocity: i32,
}

/// What tells a note apart: everything but whether it is selected, to a thousandth of a tick.
type Key = (i32, i32, i64, i64, i32, bool, i32);

impl Note {
    fn key(&self) -> Key {
        let (start, end) = ((self.start * 1000.0).round() as i64, (self.end * 1000.0).round() as i64);
        (self.channel, self.pitch, start, end, self.velocity, self.muted, self.off_velocity)
    }

    pub(super) fn json(&self) -> Value {
        json!([self.channel, self.pitch, self.start, self.end, self.velocity, self.muted as u8, self.selected as u8, self.off_velocity])
    }

    fn from_json(value: &Value) -> Option<Note> {
        let at = |i: usize| value.get(i);
        Some(Note {
            channel: at(0)?.as_i64()? as i32,
            pitch: at(1)?.as_i64()? as i32,
            start: at(2)?.as_f64()?,
            end: at(3)?.as_f64()?,
            velocity: at(4)?.as_i64()? as i32,
            muted: at(5)?.as_i64()? != 0,
            selected: at(6)?.as_i64()? != 0,
            // (a record from before release velocities were kept has seven)
            off_velocity: at(7).and_then(Value::as_i64).unwrap_or(0) as i32,
        })
    }

    fn midi(&self) -> MidiNote {
        MidiNote {
            selected: self.selected,
            muted: self.muted,
            start_ppq: self.start,
            end_ppq: self.end,
            channel: self.channel,
            pitch: self.pitch,
            velocity: self.velocity,
        }
    }
}

fn counts(notes: &[Note]) -> HashMap<Key, usize> {
    let mut counts = HashMap::new();
    for note in notes {
        *counts.entry(note.key()).or_insert(0) += 1;
    }
    counts
}

/// The notes of `a` that `b` has no note for: a note that is in both, as many times as `b` has it, doesn't count.
pub(super) fn difference(a: &[Note], b: &[Note]) -> Vec<Note> {
    let mut have = counts(b);
    a.iter()
        .filter(|note| match have.get_mut(&note.key()) {
            Some(n) if *n > 0 => {
                *n -= 1;
                false
            }
            _ => true,
        })
        .copied()
        .collect()
}

fn spec(note: &Note) -> crate::midi::NoteSpec {
    crate::midi::NoteSpec {
        channel: note.channel as u8,
        pitch: note.pitch as u8,
        start: note.start.round() as i64,
        end: note.end.round() as i64,
        velocity: note.velocity as u8,
        muted: note.muted,
        selected: note.selected,
        off_velocity: note.off_velocity.clamp(0, 127) as u8,
    }
}

/// The notes in `remove` taken out of the take (as `events` has them) and the notes in `add` put in, all in one go:
/// nothing else in the take changes.
pub(super) fn write_notes(ctx: &Ctx, take: Take, events: &[Event], remove: &[Note], add: &[Note]) -> Done<()> {
    let (remove, add): (Vec<_>, Vec<_>) = (remove.iter().map(spec).collect(), add.iter().map(spec).collect());
    let edited = crate::midi::edit_notes(events, &remove, &add).map_err(|message| changed(format!("the take changed: {message}")))?;
    if !ctx.reaper.set_midi_events(take, &edited) {
        return Err(failed("REAPER wouldn't take the new MIDI"));
    }
    Ok(())
}

/// The take's notes in the order of the one buffer REAPER gives all its MIDI in (a call, where a note at a time is a call
/// each), and the buffer.
pub(super) fn read_all(ctx: &Ctx, take: Take) -> Done<(Vec<Note>, Vec<Event>)> {
    let events = ctx.reaper.midi_events(take).ok_or_else(|| failed("REAPER wouldn't give the take's MIDI"))?;
    let notes = crate::midi::notes(&events).iter().map(from_spec).collect();
    Ok((notes, events))
}

fn from_spec(note: &crate::midi::NoteSpec) -> Note {
    Note {
        channel: note.channel as i32,
        pitch: note.pitch as i32,
        start: note.start as f64,
        end: note.end as f64,
        velocity: note.velocity as i32,
        muted: note.muted,
        selected: note.selected,
        off_velocity: note.off_velocity as i32,
    }
}

/// A write that did not leave the take as it should be: what was written is taken back, as far as it can be checked. If
/// it can't be, nobody knows what the take holds.
pub(super) fn take_back(ctx: &Ctx, take: Take, events: &[Event], was: &Value, message: &str) -> Fault {
    let put_back = ctx.reaper.set_midi_events(take, events) && read_all(ctx, take).is_ok_and(|(notes, _)| fence_of(&notes) == *was);
    if put_back {
        failed(format!("{message}; what was written is taken back"))
    } else {
        ("uncertain".into(), format!("{message}, and Kumi could not take back what it wrote: read the take"))
    }
}

/// The take's notes as one value, to fence an index with: their number and a hash of what they are.
pub(super) fn fence_of(notes: &[Note]) -> Value {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for note in notes {
        let (channel, pitch, start, end, velocity, muted, off_velocity) = note.key();
        for word in [channel as i64, pitch as i64, start, end, velocity as i64, muted as i64, off_velocity as i64] {
            for byte in word.to_le_bytes() {
                hash = (hash ^ byte as u64).wrapping_mul(0x0000_0100_0000_01b3);
            }
        }
    }
    json!({"count": notes.len(), "hash": format!("{hash:016x}")})
}

pub(super) fn take_of(ctx: &Ctx, reference: &Value) -> Done<Take> {
    let text = reference.as_str().ok_or_else(|| bad("ref must be a take or item ref"))?;
    ctx.reads.midi_take(ctx.reaper, ctx.proj, Some(text))
}

pub(super) fn target(ctx: &Ctx, take: Take) -> Value {
    json!({"ref": refs::take_ref(&ctx.reaper.take_guid(take)), "kind": "take"})
}

pub(super) fn take_label(ctx: &Ctx, take: Take) -> String {
    let name = ctx.reaper.take_name(take);
    if name.is_empty() {
        "a take".into()
    } else {
        format!("take {}", super::quoted(&rows::clip(&name)))
    }
}

fn title(verb: &str, count: usize, label: &str) -> String {
    format!("{verb} {count} note{} in {label}", if count == 1 { "" } else { "s" })
}

pub(super) fn rows_of(ctx: &Ctx, take: Take, item_qn: f64, notes: impl Iterator<Item = (usize, Note)>) -> Vec<Value> {
    notes.take(SHOWN).map(|(i, note)| rows::note_row(ctx.reaper, take, item_qn, i as i32, &note.midi())).collect()
}

// ----- what a request asks -----

#[derive(Clone, Copy, PartialEq)]
pub enum Mode {
    Add,
    Change,
    Delete,
}

impl Mode {
    fn verb(self) -> &'static str {
        match self {
            Mode::Add => "Wrote",
            Mode::Change => "Changed",
            Mode::Delete => "Deleted",
        }
    }
}

struct Plan {
    take: Take,
    item_qn: f64,
    before: Vec<Note>,
    /// The take's MIDI as it was read.
    events: Vec<Event>,
    /// The places in `before` that go.
    remove: Vec<usize>,
    /// What is written.
    add: Vec<Note>,
}

fn list<'a>(input: &'a Value, key: &str) -> Done<&'a Vec<Value>> {
    let list = input.get(key).and_then(Value::as_array).ok_or_else(|| bad(format!("{key} must be a list")))?;
    if list.is_empty() || list.len() > MOST_NOTES {
        return Err(bad(format!("{key} must hold from 1 to {MOST_NOTES} entries")));
    }
    Ok(list)
}

fn whole(spec: &Value, key: &str, low: i64, high: i64) -> Done<Option<i32>> {
    match spec.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(value) => value
            .as_i64()
            .filter(|n| (low..=high).contains(n))
            .map(|n| Some(n as i32))
            .ok_or_else(|| bad(format!("{key} must be a whole number from {low} to {high}"))),
    }
}

fn number(spec: &Value, key: &str) -> Done<Option<f64>> {
    match spec.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(value) => value
            .as_f64()
            .filter(|n| n.is_finite() && n.abs() <= 1.0e6)
            .map(Some)
            .ok_or_else(|| bad(format!("{key} must be a number (quarter notes from the start of the item)"))),
    }
}

fn flag(spec: &Value, key: &str) -> Done<Option<bool>> {
    match spec.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Bool(flag)) => Ok(Some(*flag)),
        Some(_) => Err(bad(format!("{key} must be true or false"))),
    }
}

/// A position in the take's ticks, from quarter notes counted from the start of the item.
fn ticks(ctx: &Ctx, take: Take, item_qn: f64, qn: f64) -> Done<f64> {
    ctx.reaper.quarter_notes_to_ppq(take, item_qn + qn).map(f64::round).ok_or_else(|| failed("this REAPER can't turn beats into ticks"))
}

fn quarter_notes(ctx: &Ctx, take: Take, ppq: f64) -> Done<f64> {
    ctx.reaper.ppq_to_quarter_notes(take, ppq).ok_or_else(|| failed("this REAPER can't turn ticks into beats"))
}

pub(super) fn new_note(ctx: &Ctx, take: Take, item_qn: f64, spec: &Value) -> Done<Note> {
    let pitch = whole(spec, "pitch", 0, 127)?.ok_or_else(|| bad("every note needs a pitch (0 to 127)"))?;
    let start = number(spec, "start")?
        .filter(|s| *s >= 0.0)
        .ok_or_else(|| bad("every note needs a start from 0 (quarter notes from the start of the item)"))?;
    let length = number(spec, "length")?.filter(|l| *l > 0.0).ok_or_else(|| bad("every note needs a length above 0 (quarter notes)"))?;
    let (start_ppq, end_ppq) = (ticks(ctx, take, item_qn, start)?, ticks(ctx, take, item_qn, start + length)?);
    if end_ppq <= start_ppq {
        return Err(bad("a note must be at least one tick long"));
    }
    Ok(Note {
        channel: whole(spec, "channel", 1, 16)?.map_or(0, |c| c - 1),
        pitch,
        start: start_ppq,
        end: end_ppq,
        velocity: whole(spec, "velocity", 1, 127)?.unwrap_or(100),
        muted: flag(spec, "muted")?.unwrap_or(false),
        selected: false,
        off_velocity: 0,
    })
}

/// Inside the take's source: no note starts at or past the marker that ends it, or ends past it. REAPER would sort it
/// after the marker: the source grows, and a looped item's period with it (the old end then plays mid-loop).
pub(super) fn check_inside(ctx: &Ctx, take: Take, item_qn: f64, events: &[Event], add: &[Note]) -> Done<()> {
    let Some(end) = super::midi_events::source_end(events) else { return Ok(()) };
    if add.iter().any(|note| note.start.round() as i64 >= end || note.end.round() as i64 > end) {
        let end_qn = ctx.reaper.ppq_to_quarter_notes(take, end as f64).map_or(0.0, |qn| qn - item_qn);
        return Err(bad(format!(
            "a note must start before the end of the take's source and end by it, {end_qn:.3} quarter notes from the item's start"
        )));
    }
    Ok(())
}

fn changed_note(ctx: &Ctx, take: Take, item_qn: f64, was: &Note, spec: &Value) -> Done<Note> {
    let mut note = *was;
    if let Some(pitch) = whole(spec, "pitch", 0, 127)? {
        note.pitch = pitch;
    }
    if let Some(velocity) = whole(spec, "velocity", 1, 127)? {
        note.velocity = velocity;
    }
    if let Some(channel) = whole(spec, "channel", 1, 16)? {
        note.channel = channel - 1;
    }
    if let Some(muted) = flag(spec, "muted")? {
        note.muted = muted;
    }
    let start = number(spec, "start")?;
    if start.is_some_and(|s| s < 0.0) {
        return Err(bad("start must be from 0"));
    }
    let length = number(spec, "length")?;
    if length.is_some_and(|l| l <= 0.0) {
        return Err(bad("length must be above 0"));
    }
    if start.is_some() || length.is_some() {
        let start_qn = start.unwrap_or(quarter_notes(ctx, take, was.start)? - item_qn);
        let length_qn = length.unwrap_or(quarter_notes(ctx, take, was.end)? - quarter_notes(ctx, take, was.start)?);
        note.start = ticks(ctx, take, item_qn, start_qn)?;
        note.end = ticks(ctx, take, item_qn, start_qn + length_qn)?;
        if note.end <= note.start {
            return Err(bad("a note must be at least one tick long"));
        }
    }
    Ok(note)
}

/// REAPER shortens one of two notes of the same pitch on the same channel that overlap, which changes a note Kumi wasn't
/// asked to touch (and can't be put back from the values it wrote): a note that would overlap one is refused.
fn check_overlaps(before: &[Note], remove: &[usize], add: &[Note]) -> Result<(), String> {
    let gone: std::collections::HashSet<usize> = remove.iter().copied().collect();
    // Per channel and pitch: where each note starts and ends, and whether it is one of the new ones.
    type Spans = Vec<(f64, f64, bool)>;
    let mut by_key: HashMap<(i32, i32), Spans> = HashMap::new();
    for note in before.iter().enumerate().filter(|(i, _)| !gone.contains(i)).map(|(_, n)| n) {
        by_key.entry((note.channel, note.pitch)).or_default().push((note.start, note.end, false));
    }
    for note in add {
        by_key.entry((note.channel, note.pitch)).or_default().push((note.start, note.end, true));
    }
    for ((channel, pitch), mut list) in by_key {
        if !list.iter().any(|n| n.2) {
            continue;
        }
        list.sort_by(|a, b| a.0.total_cmp(&b.0));
        let (mut latest_end, mut latest_new_end) = (f64::MIN, f64::MIN);
        for (start, end, new) in list {
            if (new && start < latest_end) || (!new && start < latest_new_end) {
                return Err(format!(
                    "a note of pitch {pitch} on channel {} would overlap another note of the same pitch (at tick {start}): REAPER would shorten one of them, so leave a gap or use another channel",
                    channel + 1
                ));
            }
            latest_end = latest_end.max(end);
            if new {
                latest_new_end = latest_new_end.max(end);
            }
        }
    }
    Ok(())
}

fn plan(ctx: &Ctx, input: &Value, mode: Mode) -> Done<Plan> {
    let take = take_of(ctx, &input["ref"])?;
    let item_qn = take_start_qn(ctx.reaper, ctx.proj, take);
    let (before, events) = read_all(ctx, take)?;
    let (mut remove, mut add) = (Vec::new(), Vec::new());
    match mode {
        Mode::Add => {
            for spec in list(input, "notes")? {
                add.push(new_note(ctx, take, item_qn, spec)?);
            }
        }
        Mode::Delete => {
            for index in list(input, "indexes")? {
                let at = index.as_u64().map(|i| i as usize).filter(|i| *i < before.len()).ok_or_else(|| {
                    bad(format!("indexes must be positions of notes in that take (it has {}): read it again", before.len()))
                })?;
                remove.push(at);
            }
        }
        Mode::Change => {
            for spec in list(input, "changes")? {
                let at = spec.get("index").and_then(Value::as_u64).map(|i| i as usize).filter(|i| *i < before.len()).ok_or_else(|| {
                    bad(format!("every change needs the index of a note in that take (it has {}): read it again", before.len()))
                })?;
                add.push(changed_note(ctx, take, item_qn, &before[at], spec)?);
                remove.push(at);
            }
        }
    }
    remove.sort_unstable();
    if remove.windows(2).any(|pair| pair[0] == pair[1]) {
        return Err(bad("a note is named twice"));
    }
    check_overlaps(&before, &remove, &add).map_err(bad)?;
    check_inside(ctx, take, item_qn, &events, &add)?;
    Ok(Plan { take, item_qn, before, events, remove, add })
}

// ----- preview, apply, restore -----

pub fn preview(ctx: &Ctx, input: &Value, mode: Mode) -> Done<Preview> {
    let plan = plan(ctx, input, mode)?;
    let label = take_label(ctx, plan.take);
    let count = plan.add.len().max(plan.remove.len());
    let fence = fence_of(&plan.before);
    let gone = plan.remove.iter().map(|&i| (i, plan.before[i]));
    let mut display = json!({"title": title(mode.verb(), count, &label), "count": count});
    if mode != Mode::Add {
        display["before"] = rows_of(ctx, plan.take, plan.item_qn, gone.clone()).into();
    }
    if mode != Mode::Delete {
        display["after"] = rows_of(ctx, plan.take, plan.item_qn, plan.add.iter().copied().enumerate()).into();
    }
    if mode == Mode::Change {
        let was: Vec<Note> = plan.remove.iter().map(|&i| plan.before[i]).collect();
        display["unchanged"] = (difference(&plan.add, &was).is_empty() && difference(&was, &plan.add).is_empty()).into();
    }
    let prior = json!({"notes": plan.before.len(), "fence": fence.clone(), "removed": rows_of(ctx, plan.take, plan.item_qn, gone)});
    Ok(Preview { target: target(ctx, plan.take), prior, fence, display })
}

pub fn apply(ctx: &Ctx, input: &Value, fence: &Value, mode: Mode) -> Done<Applied> {
    let plan = plan(ctx, input, mode)?;
    if fence_of(&plan.before) != *fence {
        return Err(changed("the notes in that take changed since the preview"));
    }
    let take = plan.take;
    let remove: Vec<Note> = plan.remove.iter().map(|&i| plan.before[i]).collect();
    write_notes(ctx, take, &plan.events, &remove, &plan.add)?;
    let (after, _) = read_all(ctx, take)?;
    if after.len() + plan.remove.len() != plan.before.len() + plan.add.len() {
        return Err(take_back(ctx, take, &plan.events, fence, "the take doesn't hold the notes it should after the change"));
    }
    let (removed, added) = (difference(&plan.before, &after), difference(&after, &plan.before));
    let label = take_label(ctx, take);
    let count = plan.add.len().max(plan.remove.len());
    Ok(Applied {
        applied: json!({"added": added.iter().map(Note::json).collect::<Vec<_>>(), "removed": removed.iter().map(Note::json).collect::<Vec<_>>(), "notes": after.len()}),
        created: None,
        material: None,
        display: json!({"title": title(mode.verb(), count, &label), "notes": after.len(), "written": rows_of(ctx, take, plan.item_qn, added.iter().copied().enumerate())}),
        unwritten: removed.is_empty() && added.is_empty(),
    })
}

fn notes_of(value: &Value, key: &str) -> Done<Vec<Note>> {
    value[key]
        .as_array()
        .ok_or_else(|| bad(format!("applied.{key}: the notes the change wrote")))?
        .iter()
        .map(|n| Note::from_json(n).ok_or_else(|| bad("applied: a note is not [channel, pitch, start, end, velocity, muted, selected]")))
        .collect()
}

pub fn restore(ctx: &Ctx, restore: &Restore) -> Done<Restored> {
    let reference = &restore.target["ref"];
    let take = take_of(ctx, reference).map_err(|fault| if fault.0 == "failed" { ("gone".to_string(), fault.1) } else { fault })?;
    let (added, removed) = (notes_of(restore.applied, "added")?, notes_of(restore.applied, "removed")?);
    let (current, events) = read_all(ctx, take)?;
    let was = fence_of(&current);
    // The notes Kumi wrote, found by their values among those there now.
    let mut wanted = counts(&added);
    let mut found = Vec::new();
    for (index, note) in current.iter().enumerate() {
        if let Some(n) = wanted.get_mut(&note.key()).filter(|n| **n > 0) {
            *n -= 1;
            found.push(index);
        }
    }
    let missing = added.len() - found.len();
    let to_bring_back = difference(&removed, &current);
    if missing > 0 && !restore.discard {
        // None of them there and all that went there again is back already only when the take's notes are as they were
        // before the change (REAPER's own undo took it back): notes the producer edited since are no longer Kumi's to find.
        let back = found.is_empty() && to_bring_back.is_empty() && restore.prior.get("fence") == Some(&was);
        if !back {
            return Err(changed("the notes Kumi wrote in that take have been changed in REAPER since"));
        }
    }
    let taken_out: std::collections::HashSet<usize> = found.iter().copied().collect();
    let kept: Vec<Note> = current.iter().enumerate().filter(|(i, _)| !taken_out.contains(i)).map(|(_, n)| *n).collect();
    let to_insert = difference(&removed, &kept);
    // A note the producer wrote since, over the place of one that goes back, would have REAPER shorten one of the two.
    if !restore.discard {
        check_overlaps(&kept, &[], &to_insert).map_err(|why| changed(format!("the notes can't all be put back: {why}")))?;
    }
    if !found.is_empty() || !to_insert.is_empty() {
        let taken: Vec<Note> = found.iter().map(|&i| current[i]).collect();
        write_notes(ctx, take, &events, &taken, &to_insert)?;
    }
    let (after, _) = read_all(ctx, take)?;
    if after.len() != kept.len() + to_insert.len() || !difference(&removed, &after).is_empty() {
        return Err(take_back(ctx, take, &events, &was, "the take doesn't hold the notes it should after the undo"));
    }
    let label = take_label(ctx, take);
    Ok(Restored {
        restored: json!({"notes": after.len()}),
        display: json!({"title": format!("Notes in {label} put back ({} removed, {} written again)", found.len(), to_insert.len())}),
        unwritten: found.is_empty() && to_insert.is_empty(),
    })
}
