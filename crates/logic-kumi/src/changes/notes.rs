//! Notes in a MIDI region, through the Event List, which shows the selected region's events: "Create new Event"
//! makes a note at the playhead (so the playhead goes there first and comes back after) with Logic's defaults, and
//! the row's cells (position, channel, pitch, velocity, length) are then stepped to what was asked; a row's Delete
//! action takes one away. A note has no identity of its own, so the preview fences the region's whole note list
//! (count and hash), a row is found again after each write by its values (the list keeps its rows in order of
//! position), and an undo finds the notes it wrote by their values. Logic's history names the steps "Create Event",
//! "Value Change" and "Delete".

use std::time::Duration;

use serde_json::{json, Value};

use super::nudge::{self, Control, Reading};
use super::{changed, gone, quoted, ref_text, timeline, Applied, Ctx, Done, Epoch, Preview, Restore, Restored};
use crate::ax::paths;
use crate::ax::{AxError, Node};
use crate::project::ListOpen;
use crate::refs::{bad, failed};
use crate::rows::{self, Note, Position, Region};

/// The most notes one change may write, change or delete: each costs a second or so of Accessibility.
const MOST_NOTES: usize = 64;
/// The notes a preview or an answer shows.
const SHOWN: usize = 8;
/// How long the list gets after a press before its rows are read.
const LIST_SETTLE: Duration = Duration::from_millis(250);
const LIST_SETTLE_MS: u64 = 250;
/// How long a row that read incomplete after a step gets before the list is read again.
const RETRY_MS: u64 = 250;
/// The velocity slider's raw value runs the whole of 32 bits, the velocity 2^25 a step above bits of its own: a
/// small step is exactly one velocity, so the low bits stay.
const VELOCITY_STEP: i64 = 1 << 25;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
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

    fn step(self) -> &'static str {
        match self {
            Mode::Add => "Create Event",
            Mode::Change => "Value Change",
            Mode::Delete => "Delete",
        }
    }
}

/// What a note is, exactly, as the Event List shows it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Spec {
    pub position: Position,
    pub pitch: u32,
    pub velocity: u32,
    pub length_ticks: u32,
    pub channel: u32,
}

impl Spec {
    fn of(note: &Note) -> Spec {
        Spec { position: note.position, pitch: note.pitch, velocity: note.velocity, length_ticks: note.length_ticks, channel: note.channel }
    }

    pub(super) fn json(&self) -> Value {
        json!({"position": self.position.to_json(), "pitch": self.pitch, "velocity": self.velocity, "lengthTicks": self.length_ticks, "channel": self.channel})
    }

    pub(super) fn from_json(value: &Value) -> Option<Spec> {
        let p = value.get("position")?;
        let at = |key: &str| p.get(key).and_then(Value::as_u64).map(|n| n as u32);
        Some(Spec {
            position: Position { bar: at("bar")?, beat: at("beat")?, division: at("division").unwrap_or(1), tick: at("tick").unwrap_or(1) },
            pitch: value.get("pitch")?.as_u64()? as u32,
            velocity: value.get("velocity")?.as_u64()? as u32,
            length_ticks: value.get("lengthTicks")?.as_u64()? as u32,
            channel: value.get("channel").and_then(Value::as_u64).unwrap_or(1) as u32,
        })
    }
}

/// A note row as read: the note and its row's node.
#[derive(Clone)]
pub(super) struct Row {
    pub(super) note: Note,
    pub(super) node: Node,
}

/// The region a change is about, with its place and the beats to a bar, and the Event List shown for it.
pub(super) struct Shown {
    pub(super) region: Region,
    pub(super) beats_per_bar: u32,
    open: ListOpen,
    columns: Vec<(String, usize)>,
}

/// The region at a place, read now.
fn region(ctx: &mut Ctx, place: usize) -> Done<(Region, u32)> {
    let (_, transport) = timeline::control_bar(ctx)?;
    let regions = nudge::read(ctx.project.read_regions(ctx.ax))?;
    let (_, region) = regions.get(place - 1).ok_or_else(|| crate::refs::not_in_project(&ref_text(ctx, "region", &[place])))?;
    if !region.midi {
        return Err(bad(format!("{} is an audio region: notes are written in MIDI regions", quoted(&region.name))));
    }
    Ok((region.clone(), transport.beats_per_bar()))
}

/// Shows the region's events: selects the region when it isn't, and shows the Event tab.
pub(super) fn show(ctx: &mut Ctx, place: usize) -> Done<Shown> {
    let (region, beats_per_bar) = region(ctx, place)?;
    if !region.selected {
        nudge::read(ctx.project.select_region(ctx.ax, place))?;
    }
    let open = nudge::read(ctx.project.show_list(ctx.ax, "Event"))?;
    let (columns, _) = nudge::read(ctx.project.list_table(ctx.ax, "Event"))?;
    Ok(Shown { region, beats_per_bar, open, columns })
}

pub(super) fn hide(ctx: &mut Ctx, shown: Shown) {
    let _ = ctx.project.hide_list(ctx.ax, shown.open);
    ctx.project.forget();
}

pub(super) fn rows(ctx: &mut Ctx, shown: &Shown) -> Done<Vec<Row>> {
    let (columns, table) = nudge::read(ctx.project.list_table(ctx.ax, "Event"))?;
    let strings = &ctx.project.strings;
    Ok(paths::rows(&table)
        .into_iter()
        .filter_map(|row| Note::of(row, &columns, shown.beats_per_bar, strings).map(|note| Row { note, node: row.clone() }))
        .collect())
}

pub(super) fn specs(list: &[Row]) -> Vec<Spec> {
    list.iter().map(|row| Spec::of(&row.note)).collect()
}

/// The region's notes as one value, to fence a change with: their number and a hash of what they are.
pub(super) fn fence_of(notes: &[Spec]) -> Value {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for note in notes {
        let p = note.position;
        for word in [p.bar, p.beat, p.division, p.tick, note.pitch, note.velocity, note.length_ticks, note.channel] {
            for byte in word.to_le_bytes() {
                hash = (hash ^ u64::from(byte)).wrapping_mul(0x0000_0100_0000_01b3);
            }
        }
    }
    json!({"count": notes.len(), "hash": format!("{hash:016x}")})
}

/// The notes of `a` that `b` has no note for.
pub(super) fn difference(a: &[Spec], b: &[Spec]) -> Vec<Spec> {
    let mut left: Vec<Spec> = b.to_vec();
    a.iter()
        .filter(|note| match left.iter().position(|other| other == *note) {
            Some(at) => {
                left.swap_remove(at);
                false
            }
            None => true,
        })
        .copied()
        .collect()
}

pub(super) fn find_row<'a>(list: &'a [Row], spec: &Spec) -> Option<&'a Row> {
    let matching: Vec<&Row> = list.iter().filter(|row| Spec::of(&row.note) == *spec).collect();
    matching.iter().find(|row| row.note.selected).or(matching.first()).copied()
}

/// The row of a note, selected alone first when it is selected with others: Logic applies a value change, and a
/// delete, to every selected event when the edited one is among them (a set of `AXSelected` leaves the row the one
/// selection). The row as read after.
fn alone(ctx: &mut Ctx, shown: &Shown, spec: &Spec, why: &'static str) -> Done<Row> {
    let list = rows(ctx, shown)?;
    let row = find_row(&list, spec).ok_or_else(|| super::uncertain(why))?;
    if !row.note.selected || list.iter().filter(|r| r.note.selected).count() < 2 {
        return Ok(row.clone());
    }
    let path = row.node.path.clone();
    nudge::wrote(ctx.ax.set(&ctx.project.root, &path, "AXSelected", json!(true), LIST_SETTLE_MS))?;
    super::pause(LIST_SETTLE);
    let list = rows(ctx, shown)?;
    find_row(&list, spec).cloned().ok_or_else(|| super::uncertain(why))
}

/// A cell's control: the slider of a Ch, Num or Val cell, or the segment sliders of a Position or Length cell.
fn cell_controls(ctx: &mut Ctx, row: &Row, columns: &[(String, usize)], column: &str) -> Done<Vec<Control>> {
    if matches!(column, "Position" | "Length") {
        return super::marks::segments(ctx, &row.node, columns, column, "the note");
    }
    let index = columns
        .iter()
        .find(|(name, _)| name == column)
        .map(|(_, i)| *i)
        .ok_or_else(|| failed(format!("the Event List has no {column} column")))?;
    let cell = row
        .node
        .children
        .iter()
        .filter(|c| c.role == "AXCell")
        .nth(index)
        .ok_or_else(|| failed(format!("the note's row has no {column} cell")))?;
    let controls: Vec<Control> =
        cell.children.iter().filter(|c| c.role == "AXSlider").map(|c| Control::new(&ctx.project.root, &c.path)).collect();
    if controls.is_empty() {
        return Err(failed(format!("the note's {column} cell has nothing to set")));
    }
    Ok(controls)
}

/// A length in ticks as the Length scrubber's segments: bars, beats, divisions, ticks.
fn length_segments(ticks: u32, beats_per_bar: u32) -> [u32; 4] {
    let bar = beats_per_bar.max(1) * rows::TICKS_PER_BEAT;
    [
        ticks / bar,
        (ticks % bar) / rows::TICKS_PER_BEAT,
        (ticks % rows::TICKS_PER_BEAT) / rows::TICKS_PER_DIVISION,
        ticks % rows::TICKS_PER_DIVISION,
    ]
}

/// Steps one row's cells from `from` to `to`, finding the row again after each cell by its values. The note as read
/// after.
fn set_cells(ctx: &mut Ctx, shown: &Shown, from: Spec, to: Spec) -> Done<Spec> {
    let mut now = from;
    // The pitch, channel and velocity first: they leave the row where it is. The position last: it moves the row.
    if now.pitch != to.pitch {
        let row = alone(ctx, shown, &now, "the note isn't in the Event List as it was")?;
        let control = cell_controls(ctx, &row, &shown.columns, "Num")?.remove(0);
        nudge::wrote(nudge::step_to(ctx.ax, &control, i64::from(now.pitch), i64::from(to.pitch)))?;
        now.pitch = to.pitch;
    }
    if now.channel != to.channel {
        let row = alone(ctx, shown, &now, "the note isn't in the Event List as it was")?;
        let control = cell_controls(ctx, &row, &shown.columns, "Ch")?.remove(0);
        nudge::wrote(nudge::step_to(ctx.ax, &control, i64::from(now.channel), i64::from(to.channel)))?;
        now.channel = to.channel;
    }
    if now.velocity != to.velocity {
        let row = alone(ctx, shown, &now, "the note isn't in the Event List as it was")?;
        let control = cell_controls(ctx, &row, &shown.columns, "Val")?.remove(0);
        let (root, path) = (ctx.project.root.clone(), control.path.clone());
        let read = move |ax: &mut crate::ax::Ax| -> Result<Reading, AxError> {
            let attrs = ax.get(&root, &path, &["AXValue", "AXValueDescription"])?;
            let raw = attrs.get("AXValue").and_then(Value::as_f64).unwrap_or(0.0).round() as i64;
            let value = attrs.get("AXValueDescription").and_then(Value::as_str).and_then(rows::first_number).unwrap_or(f64::NAN);
            Ok(Reading { value, raw })
        };
        let reached = nudge::wrote(nudge::step_reading(ctx.ax, &control, f64::from(to.velocity), 9.5, 0.5, VELOCITY_STEP, read))?;
        now.velocity = reached.value.round() as u32;
    }
    if now.length_ticks != to.length_ticks {
        let row = alone(ctx, shown, &now, "the note isn't in the Event List as it was")?;
        let controls = cell_controls(ctx, &row, &shown.columns, "Length")?;
        let (from_segments, to_segments) =
            (length_segments(now.length_ticks, shown.beats_per_bar), length_segments(to.length_ticks, shown.beats_per_bar));
        for (index, control) in controls.iter().enumerate().take(4) {
            if from_segments[index] != to_segments[index] {
                nudge::wrote(nudge::step_to(ctx.ax, control, i64::from(from_segments[index]), i64::from(to_segments[index])))?;
            }
        }
        now.length_ticks = to.length_ticks;
    }
    if now.position != to.position {
        let key = now;
        let columns = shown.columns.clone();
        let reached = super::marks::step_position(ctx, &columns, now.position, to.position, "the note", |ctx, at| {
            let wanted = Spec { position: *at, ..key };
            let mut list = rows(ctx, shown)?;
            if find_row(&list, &wanted).is_none() {
                // A row read while Logic rebuilds it after a step can miss its cells: read once more after a moment.
                super::pause(Duration::from_millis(RETRY_MS));
                list = rows(ctx, shown)?;
            }
            let row = find_row(&list, &wanted).ok_or_else(|| super::uncertain("the note isn't in the Event List as it was"))?;
            if row.note.selected && list.iter().filter(|r| r.note.selected).count() > 1 {
                // Selected with others, the step would move them all (see `alone`): the row alone first.
                let path = row.node.path.clone();
                nudge::wrote(ctx.ax.set(&ctx.project.root, &path, "AXSelected", json!(true), LIST_SETTLE_MS))?;
                super::pause(LIST_SETTLE);
                let list = rows(ctx, shown)?;
                let row = find_row(&list, &wanted).ok_or_else(|| super::uncertain("the note isn't in the Event List as it was"))?;
                return Ok(row.node.clone());
            }
            Ok(row.node.clone())
        })?;
        now.position = reached;
    }
    super::pause(LIST_SETTLE);
    let list = rows(ctx, shown)?;
    let row = find_row(&list, &now).ok_or_else(|| super::uncertain("the note isn't in the Event List as Kumi set it"))?;
    Ok(Spec::of(&row.note))
}

/// Makes a note: the playhead to its bar and beat, "Create new Event", the new row's cells set. The note as read.
fn create(ctx: &mut Ctx, shown: &Shown, spec: Spec) -> Done<Spec> {
    let (_, transport) = timeline::control_bar(ctx)?;
    let was = transport.position();
    let before = specs(&rows(ctx, shown)?);
    timeline::set_playhead(ctx, Position { bar: spec.position.bar, beat: spec.position.beat, ..Position::START })?;
    let group = nudge::read(ctx.project.list_group(ctx.ax))?;
    let strings = &ctx.project.strings;
    let button = group
        .find(&|n| n.role == "AXButton" && strings.is(&n.desc, "Create new Event"))
        .ok_or_else(|| failed("Logic Pro's Event List doesn't show its Create new Event button"))?;
    let button = Control::new(&ctx.project.root, &button.path);
    nudge::wrote(nudge::press(ctx.ax, &button, LIST_SETTLE_MS))?;
    super::pause(LIST_SETTLE);
    let after = specs(&rows(ctx, shown)?);
    let made = difference(&after, &before);
    let _ = timeline::set_playhead(ctx, was);
    let [made] = made.as_slice() else {
        return Err(super::uncertain(format!("Logic Pro's Event List shows {} new events after Kumi asked for one", made.len())));
    };
    set_cells(ctx, shown, *made, spec)
}

fn delete(ctx: &mut Ctx, shown: &Shown, spec: &Spec) -> Done<()> {
    let row = alone(ctx, shown, spec, "a note Kumi would take away isn't in the Event List as it was").map_err(|(code, why)| {
        if code == "uncertain" {
            changed(why)
        } else {
            (code, why)
        }
    })?;
    nudge::wrote(ctx.ax.action(&ctx.project.root, &row.node.path, super::marks::DELETE_ACTION, LIST_SETTLE_MS))?;
    super::pause(LIST_SETTLE);
    Ok(())
}

// ----- what a request asks -----

fn list_of<'a>(input: &'a Value, key: &str) -> Done<&'a Vec<Value>> {
    let list = input.get(key).and_then(Value::as_array).ok_or_else(|| bad(format!("{key} must be a list")))?;
    if list.is_empty() || list.len() > MOST_NOTES {
        return Err(bad(format!("{key} must hold from 1 to {MOST_NOTES} entries")));
    }
    Ok(list)
}

fn whole(spec: &Value, key: &str, low: u64, high: u64) -> Done<Option<u32>> {
    match spec.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(value) => value
            .as_u64()
            .filter(|n| (low..=high).contains(n))
            .map(|n| Some(n as u32))
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
            .ok_or_else(|| bad(format!("{key} must be a number (beats from the start of the region)"))),
    }
}

fn new_spec(region_start: f64, beats_per_bar: u32, spec: &Value) -> Done<Spec> {
    let pitch = whole(spec, "pitch", 0, 127)?.ok_or_else(|| bad("every note needs a pitch (0 to 127)"))?;
    let start = number(spec, "start")?
        .filter(|s| *s >= 0.0)
        .ok_or_else(|| bad("every note needs a start from 0 (beats from the start of the region)"))?;
    let length = number(spec, "length")?.filter(|l| *l > 0.0).ok_or_else(|| bad("every note needs a length above 0 (beats)"))?;
    let length_ticks = (length * f64::from(rows::TICKS_PER_BEAT)).round().max(1.0) as u32;
    Ok(Spec {
        position: Position::from_beats(region_start + start, beats_per_bar),
        pitch,
        velocity: whole(spec, "velocity", 1, 127)?.unwrap_or(100),
        length_ticks,
        channel: whole(spec, "channel", 1, 16)?.unwrap_or(1),
    })
}

fn changed_spec(region_start: f64, beats_per_bar: u32, was: &Spec, spec: &Value) -> Done<Spec> {
    let mut note = *was;
    if let Some(pitch) = whole(spec, "pitch", 0, 127)? {
        note.pitch = pitch;
    }
    if let Some(velocity) = whole(spec, "velocity", 1, 127)? {
        note.velocity = velocity;
    }
    if let Some(channel) = whole(spec, "channel", 1, 16)? {
        note.channel = channel;
    }
    if let Some(start) = number(spec, "start")? {
        if start < 0.0 {
            return Err(bad("start must be from 0"));
        }
        note.position = Position::from_beats(region_start + start, beats_per_bar);
    }
    if let Some(length) = number(spec, "length")? {
        if length <= 0.0 {
            return Err(bad("length must be above 0"));
        }
        note.length_ticks = (length * f64::from(rows::TICKS_PER_BEAT)).round().max(1.0) as u32;
    }
    Ok(note)
}

struct Plan {
    before: Vec<Spec>,
    remove: Vec<Spec>,
    add: Vec<Spec>,
}

fn plan(ctx: &mut Ctx, shown: &Shown, input: &Value, mode: Mode) -> Done<Plan> {
    let before = specs(&rows(ctx, shown)?);
    let region_start = shown.region.start.beats(shown.beats_per_bar);
    let (mut remove, mut add) = (Vec::new(), Vec::new());
    match mode {
        Mode::Add => {
            for spec in list_of(input, "notes")? {
                add.push(new_spec(region_start, shown.beats_per_bar, spec)?);
            }
        }
        Mode::Delete => {
            let mut seen = Vec::new();
            for index in list_of(input, "indexes")? {
                let at = index.as_u64().map(|i| i as usize).filter(|i| *i < before.len()).ok_or_else(|| {
                    bad(format!("indexes must be positions of notes in that region (it has {}): read it again", before.len()))
                })?;
                if seen.contains(&at) {
                    return Err(bad("a note is named twice"));
                }
                seen.push(at);
                remove.push(before[at]);
            }
        }
        Mode::Change => {
            let mut seen = Vec::new();
            for spec in list_of(input, "changes")? {
                let at = spec.get("index").and_then(Value::as_u64).map(|i| i as usize).filter(|i| *i < before.len()).ok_or_else(|| {
                    bad(format!("every change needs the index of a note in that region (it has {}): read it again", before.len()))
                })?;
                if seen.contains(&at) {
                    return Err(bad("a note is named twice"));
                }
                seen.push(at);
                add.push(changed_spec(region_start, shown.beats_per_bar, &before[at], spec)?);
                remove.push(before[at]);
            }
        }
    }
    Ok(Plan { before, remove, add })
}

pub(super) fn label(region: &Region) -> String {
    if region.name.is_empty() {
        "the region".into()
    } else {
        format!("{}", quoted(&rows::clip(&region.name)))
    }
}

fn title(verb: &str, count: usize, label: &str) -> String {
    format!("{verb} {count} note{} in {label}", if count == 1 { "" } else { "s" })
}

fn shown_notes(notes: &[Spec]) -> Vec<Value> {
    notes.iter().take(SHOWN).map(Spec::json).collect()
}

fn target(ctx: &Ctx, place: usize) -> Value {
    json!({"ref": ref_text(ctx, "region", &[place]), "kind": "region"})
}

// ----- preview, apply, restore -----

pub fn preview(ctx: &mut Ctx, input: &Value, mode: Mode) -> Done<Preview> {
    let place = super::region_place(ctx, input, "ref", Epoch::Current)?;
    let shown = show(ctx, place)?;
    let result = (|| {
        let plan = plan(ctx, &shown, input, mode)?;
        let count = plan.add.len().max(plan.remove.len());
        let label = label(&shown.region);
        let mut display = json!({"title": title(mode.verb(), count, &label), "count": count});
        if mode != Mode::Add {
            display["before"] = shown_notes(&plan.remove).into();
        }
        if mode != Mode::Delete {
            display["after"] = shown_notes(&plan.add).into();
        }
        if mode == Mode::Change && plan.add == plan.remove {
            display["unchanged"] = true.into();
        }
        let fence = fence_of(&plan.before);
        Ok(Preview {
            target: target(ctx, place),
            prior: json!({"notes": plan.before.len(), "fence": fence.clone(), "removed": shown_notes(&plan.remove)}),
            fence,
            display,
        })
    })();
    hide(ctx, shown);
    result
}

/// Takes the notes in `remove` out of the region and puts the notes in `add` in; a note in both lists at the same
/// place (a change) is set in its row instead. The region's notes after.
pub(super) fn write_notes(ctx: &mut Ctx, shown: &Shown, remove: &[Spec], add: &[Spec], in_place: bool) -> Done<Vec<Spec>> {
    if in_place && remove.len() == add.len() {
        for (from, to) in remove.iter().zip(add) {
            if from == to {
                continue;
            }
            let made = set_cells(ctx, shown, *from, *to)?;
            if made != *to {
                return Err(nudge::kept("a note", format!("{to:?}"), format!("{made:?}")));
            }
        }
    } else {
        for spec in remove {
            delete(ctx, shown, spec)?;
        }
        for spec in add {
            let made = create(ctx, shown, *spec)?;
            if made != *spec {
                return Err(nudge::kept("a note", format!("{spec:?}"), format!("{made:?}")));
            }
        }
    }
    Ok(specs(&rows(ctx, shown).map_err(|(_, why)| super::uncertain(why))?))
}

pub fn apply(ctx: &mut Ctx, input: &Value, fence: &Value, mode: Mode) -> Done<Applied> {
    let place = super::region_place(ctx, input, "ref", Epoch::Current)?;
    let shown = show(ctx, place)?;
    let result = (|| {
        let plan = plan(ctx, &shown, input, mode)?;
        if fence_of(&plan.before) != *fence {
            return Err(changed("the notes in that region changed since the preview"));
        }
        let after = write_notes(ctx, &shown, &plan.remove, &plan.add, mode == Mode::Change)?;
        if after.len() + plan.remove.len() != plan.before.len() + plan.add.len() {
            return Err(super::uncertain("the region doesn't hold the notes it should after the change"));
        }
        let (removed, added) = (difference(&plan.before, &after), difference(&after, &plan.before));
        let count = plan.add.len().max(plan.remove.len());
        let label = label(&shown.region);
        Ok(Applied {
            applied: json!({"added": added.iter().map(Spec::json).collect::<Vec<_>>(), "removed": removed.iter().map(Spec::json).collect::<Vec<_>>(), "notes": after.len()}),
            created: None,
            target: None,
            material: None,
            display: json!({"title": title(mode.verb(), count, &label), "notes": after.len(), "written": shown_notes(&added)}),
            step: Some(mode.step()),
            probe: None,
        })
    })();
    hide(ctx, shown);
    result
}

fn notes_of(value: &Value, key: &str) -> Done<Vec<Spec>> {
    value[key]
        .as_array()
        .ok_or_else(|| bad(format!("applied.{key}: the notes the change wrote")))?
        .iter()
        .map(|n| Spec::from_json(n).ok_or_else(|| bad("applied: a note is not {position, pitch, velocity, lengthTicks, channel}")))
        .collect()
}

pub fn restore(ctx: &mut Ctx, restore: &Restore) -> Done<Restored> {
    let place = super::region_place(ctx, restore.target, "ref", Epoch::Any).map_err(|f| if f.0 == "failed" { gone(f.1) } else { f })?;
    let (added, removed) = (notes_of(restore.applied, "added")?, notes_of(restore.applied, "removed")?);
    let shown = show(ctx, place).map_err(|f| if f.0 == "failed" { gone(f.1) } else { f })?;
    let result = (|| {
        let current = specs(&rows(ctx, &shown)?);
        // The notes Kumi wrote, found by their values among those there now.
        let present = difference(&added, &difference(&added, &current));
        let missing = added.len() - present.len();
        let to_bring_back = difference(&removed, &current);
        if missing > 0 && !restore.discard && !(present.is_empty() && to_bring_back.is_empty()) {
            return Err(changed("the notes Kumi wrote in that region have been changed in Logic Pro since"));
        }
        // A change put back: its notes set in place; else the notes written taken out and those taken out written.
        let in_place = added.len() == removed.len() && present.len() == to_bring_back.len();
        let after = write_notes(ctx, &shown, &present, &to_bring_back, in_place)?;
        if !difference(&removed, &after).is_empty() {
            return Err(super::uncertain("the region doesn't hold the notes it should after the undo"));
        }
        let label = label(&shown.region);
        Ok(Restored {
            restored: json!({"notes": after.len()}),
            display: json!({"title": format!("Notes in {label} put back ({} removed, {} written again)", present.len(), to_bring_back.len())}),
            undone_in_logic: false,
        })
    })();
    hide(ctx, shown);
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notes_are_placed_from_the_regions_start_and_told_apart_by_their_values() {
        let spec = new_spec(0.0, 4, &json!({"pitch": 62, "start": 1.5, "length": 0.25})).unwrap();
        assert_eq!(
            spec,
            Spec { position: Position { bar: 1, beat: 2, division: 3, tick: 1 }, pitch: 62, velocity: 100, length_ticks: 240, channel: 1 }
        );
        let spec = new_spec(16.0, 4, &json!({"pitch": 60, "start": 0, "length": 1, "velocity": 90, "channel": 2})).unwrap();
        assert_eq!((spec.position.bar, spec.velocity, spec.length_ticks, spec.channel), (5, 90, 960, 2));
        assert!(new_spec(0.0, 4, &json!({"pitch": 128, "start": 0, "length": 1})).is_err());
        assert!(new_spec(0.0, 4, &json!({"pitch": 60, "start": 0, "length": 0})).is_err());
        let changed = changed_spec(0.0, 4, &spec, &json!({"pitch": 64, "length": 0.5})).unwrap();
        assert_eq!((changed.pitch, changed.length_ticks, changed.velocity), (64, 480, 90));
        assert_eq!(Spec::from_json(&spec.json()), Some(spec));
        let other = Spec { pitch: 61, ..spec };
        assert_eq!(difference(&[spec, other, spec], &[spec]), vec![other, spec]);
        assert_eq!(fence_of(&[spec])["count"], 1);
        assert_ne!(fence_of(&[spec])["hash"], fence_of(&[other])["hash"]);
        assert_eq!(length_segments(1200, 4), [0, 1, 1, 0]);
        assert_eq!(length_segments(4 * 960 + 241, 4), [1, 0, 1, 1]);
        assert_eq!(title("Wrote", 2, "“Keys”"), "Wrote 2 notes in “Keys”");
        assert_eq!(Mode::Delete.step(), "Delete");
    }
}
