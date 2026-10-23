//! A track's automation, through Logic's Track Automation Event List: its key command (Control-Command-E in the
//! factory set; the producer's own binding of command 1032 when they have one) opens it for the selected track as a
//! window of its own ("<project> - Track Automation Root Folder:<track> - Event List"), an Event List whose rows are
//! the track's automation as "Fader" events: a position, the parameter's number (Num), its value (Val, 0 to 127) and
//! the parameter's name, which Logic writes in the Length/Info cell ("Volume", "Balance"). The read `automation`
//! (`read {kind: automation, parent: <track>}`) selects the track, opens the list, reads it, closes it and puts the
//! selection back: a row per parameter, with its events.
//!
//! The kinds write there as the note kinds write the Event List: `automation-add` sets the list's event type to
//! Fader, moves the playhead to each event's position (on a beat: the LCD places it), presses Create new Event, steps
//! the new row's Num until its Length/Info cell names the parameter (or to the number given) and its Val to the
//! value; `automation-change` steps a row's Val and Position; `automation-delete` presses a row's Delete. An event is
//! named by its parameter and its place among that parameter's events (from 0, as the read lists them). Read back
//! from the list's rows; a restore is by value (the events Kumi made deleted, the ones it deleted made again).

use std::time::{Duration, Instant};

use serde_json::{json, Value};

use super::nudge::Control;
use super::{changed, front, gone, nudge, panes, quoted, timeline, Applied, Ctx, Done, Epoch, Preview, Restore, Restored};
use crate::ax::{paths, Node, Root};
use crate::keys::{Chord, KeyCommands};
use crate::refs::{bad, failed, Ref};
use crate::rows::{AutomationEvent, Position};

/// Logic's command that opens the Track Automation Event List, and its factory chord.
const OPEN_LIST: u32 = 1032;
const OPEN_CHORD: &str = "cmd-ctrl-e";
/// How long the list's window gets to come up.
const OPEN_WAIT: Duration = Duration::from_secs(3);
const POLL: Duration = Duration::from_millis(150);
/// The window's title says what it is.
const TITLE_PART: &str = "Track Automation";

/// The list's window, shown for a read.
pub struct Shown {
    pub root: Root,
    pub title: String,
}

fn list_windows(ctx: &mut Ctx) -> Done<Vec<String>> {
    let reply = nudge::read(ctx.ax.windows())?;
    let (windows, _) = crate::logic::parse_windows(&reply);
    Ok(windows.into_iter().map(|w| w.title).filter(|title| title.contains(TITLE_PART)).collect())
}

/// Opens the Track Automation Event List of the selected track (`track` its name, which the window's title carries).
fn show(ctx: &mut Ctx, keys: &KeyCommands, track: &str) -> Done<Shown> {
    let before = list_windows(ctx)?;
    let chord = keys
        .chord(OPEN_LIST)
        .cloned()
        .or_else(|| Chord::parse(OPEN_CHORD))
        .ok_or_else(|| failed("no key for the Track Automation Event List"))?;
    front::ensure(ctx.ax)?;
    front::post(ctx.ax, &chord)?;
    let until = Instant::now() + OPEN_WAIT;
    loop {
        let now = list_windows(ctx)?;
        let found = now
            .iter()
            .find(|title| !before.contains(title) && title.contains(&format!(":{track} ")))
            .or_else(|| now.iter().find(|title| title.contains(&format!(":{track} "))));
        if let Some(title) = found {
            return Ok(Shown { root: Root::Title(title.clone()), title: title.clone() });
        }
        if Instant::now() >= until {
            return Err(failed(format!("Logic Pro didn't show the Track Automation Event List of {}", super::quoted(track))));
        }
        std::thread::sleep(POLL);
    }
}

/// Closes the list's window by its close button.
fn hide(ctx: &mut Ctx, shown: &Shown) {
    if let Ok(window) = ctx.ax.tree(&shown.root, "", 1, 12, Some(&["role", "subrole", "title"])) {
        if window.title == shown.title {
            if let Some(close) = window.children.iter().find(|c| c.role == "AXButton" && c.subrole == "AXCloseButton") {
                let _ = ctx.ax.action(&shown.root, &close.path, "AXPress", 200);
            }
        }
    }
}

/// The list's Fader events, in the list's order, with their rows.
fn events(ctx: &mut Ctx, shown: &Shown) -> Done<Vec<(AutomationEvent, Node)>> {
    let window = nudge::read(ctx.ax.tree(&shown.root, "", 8, 256, Some(crate::ax::CHEAP)))?;
    let table = paths::list_table(&window).ok_or_else(|| failed("the Track Automation Event List shows no table"))?;
    let columns = paths::columns(table);
    let strings = &ctx.project.strings;
    Ok(paths::rows(table)
        .into_iter()
        .filter_map(|row| AutomationEvent::of(row, &columns, strings).map(|event| (event, row.clone())))
        .collect())
}

/// The events grouped by parameter, in the order the list first names each: the read's rows.
fn grouped(events: &[AutomationEvent], beats_per_bar: u32) -> Vec<(String, u32, Vec<Value>)> {
    let mut out: Vec<(String, u32, Vec<Value>)> = Vec::new();
    for event in events {
        let point = json!({"positionBeats": event.position.beats(beats_per_bar), "value": event.value});
        match out.iter_mut().find(|(name, number, _)| *name == event.parameter && *number == event.number) {
            Some((_, _, points)) => points.push(point),
            None => out.push((event.parameter.clone(), event.number, vec![point])),
        }
    }
    out
}

/// `read {kind: automation, parent: <track>}`: the track's automation, a row per parameter with its events.
pub fn rows(ctx: &mut Ctx, keys: &KeyCommands, place: usize) -> Done<Vec<Value>> {
    let (_, transport) = timeline::control_bar(ctx)?;
    let beats_per_bar = transport.beats_per_bar();
    let track = super::tracks::at(ctx, place)?;
    let name = track.header.name.clone();
    let events = panes::with_selected(ctx, place, |ctx| {
        let shown = show(ctx, keys, &name)?;
        let read = events(ctx, &shown);
        hide(ctx, &shown);
        ctx.project.forget();
        read
    })?;
    let epoch = ctx.project.epoch;
    let track_ref = Ref::new(epoch, "track", &[place]).text();
    let list: Vec<AutomationEvent> = events.into_iter().map(|(event, _)| event).collect();
    Ok(grouped(&list, beats_per_bar)
        .into_iter()
        .enumerate()
        .map(|(i, (parameter, number, points))| {
            json!({"ref": Ref::new(epoch, "automation", &[place, i + 1]).text(), "trackRef": track_ref, "index": i + 1, "parameter": parameter, "number": number, "events": points})
        })
        .collect())
}

// ----- the kinds -----

/// The most events one change writes.
const MOST_EVENTS: usize = 32;
/// The highest parameter number Kumi steps through looking for a parameter's name.
const MOST_NUMBER: u32 = 127;
const LIST_SETTLE: Duration = Duration::from_millis(250);
const LIST_SETTLE_MS: u64 = 250;
/// The event type Logic's automation rows are.
const FADER: &str = "Fader";

/// The parameter a request names: its name as the list's Length/Info cell writes it, or its number.
#[derive(Debug, Clone, PartialEq)]
enum Parameter {
    Named(String),
    Number(u32),
}

impl Parameter {
    fn of(input: &Value) -> Done<Parameter> {
        match input.get("parameter") {
            Some(Value::String(name)) if !name.trim().is_empty() => Ok(Parameter::Named(name.trim().to_string())),
            Some(Value::Number(n)) => n
                .as_u64()
                .filter(|n| *n <= 16383)
                .map(|n| Parameter::Number(n as u32))
                .ok_or_else(|| bad("parameter: a number from 0 to 16383")),
            _ => {
                Err(bad("parameter: the parameter's name as Logic Pro's Track Automation Event List names it (\"Volume\"), or its number"))
            }
        }
    }

    fn matches(&self, event: &AutomationEvent) -> bool {
        match self {
            Parameter::Named(name) => event.parameter.eq_ignore_ascii_case(name),
            Parameter::Number(n) => event.number == *n,
        }
    }

    fn label(&self) -> String {
        match self {
            Parameter::Named(name) => name.clone(),
            Parameter::Number(n) => format!("parameter {n}"),
        }
    }
}

fn point_of(spec: &Value, beats_per_bar: u32) -> Done<(Position, u32)> {
    let beats = spec
        .get("positionBeats")
        .and_then(Value::as_f64)
        .filter(|n| n.is_finite() && (0.0..=1.0e6).contains(n))
        .ok_or_else(|| bad("every event needs positionBeats (beats from the project's start)"))?;
    if beats.fract().abs() > 1e-9 {
        return Err(bad("Logic Pro's playhead display places an automation event on a beat: give whole beats"));
    }
    let value =
        spec.get("value").and_then(Value::as_u64).filter(|v| *v <= 127).ok_or_else(|| bad("every event needs a value from 0 to 127"))?;
    Ok((Position::from_beats(beats, beats_per_bar), value as u32))
}

fn events_json(events: &[AutomationEvent], beats_per_bar: u32) -> Vec<Value> {
    events
        .iter()
        .map(|e| json!({"positionBeats": e.position.beats(beats_per_bar), "value": e.value, "number": e.number, "parameter": e.parameter}))
        .collect()
}

/// The list opened for a write: its window, and the project's window by its title (where the playhead is moved: the
/// list's window is the main one while it is up).
struct Opened {
    list: Shown,
    project: Root,
}

/// The project window's title, as the windows say it now.
fn project_title(ctx: &mut Ctx) -> Done<String> {
    let reply = nudge::read(ctx.ax.windows())?;
    let (windows, _) = crate::logic::parse_windows(&reply);
    crate::logic::project_window(&windows).map(|w| w.title.clone()).ok_or_else(|| failed("Logic Pro shows no project window"))
}

/// Runs `with` with the playhead's control bar read from the project's window by its title, the list's being main.
fn on_project<T>(ctx: &mut Ctx, project: &Root, with: impl FnOnce(&mut Ctx) -> Done<T>) -> Done<T> {
    let was = ctx.project.root.clone();
    ctx.project.set_root(project.clone());
    let result = with(ctx);
    ctx.project.set_root(was);
    result
}

/// The list's window tree, its table, the table's columns and its Fader rows with their events.
fn table(ctx: &mut Ctx, list: &Shown) -> Done<(Node, Vec<(String, usize)>, Vec<(AutomationEvent, Node)>)> {
    let window = nudge::read(ctx.ax.tree(&list.root, "", 8, 256, Some(crate::ax::CHEAP)))?;
    let table = paths::list_table(&window).ok_or_else(|| failed("the Track Automation Event List shows no table"))?.clone();
    let columns = paths::columns(&table);
    let strings = &ctx.project.strings;
    let rows =
        paths::rows(&table).into_iter().filter_map(|row| AutomationEvent::of(row, &columns, strings).map(|e| (e, row.clone()))).collect();
    Ok((window, columns, rows))
}

/// A row's cell's first slider, under a column.
fn cell_slider(row: &Node, columns: &[(String, usize)], column: &str) -> Option<Node> {
    let index = columns.iter().find(|(name, _)| name == column)?.1;
    let cell = row.children.iter().filter(|c| c.role == "AXCell").nth(index)?;
    cell.find(&|n| n.role == "AXSlider").cloned()
}

/// The items of the list's event type pop-up, by which it is told from its Quantize pop-up.
const TYPES: [&str; 10] = [
    "Notes",
    "Progr. Change",
    "Pitch Bend",
    "Controller",
    "Aftertouch",
    "Poly Aftertouch",
    "Syst. Exclusive",
    "Meta Events",
    FADER,
    "Fader MIDI",
];

/// The list's event type pop-up, the type of event Create new Event makes.
fn type_popup(window: &Node) -> Done<&Node> {
    window
        .find(&|n| n.role == "AXPopUpButton" && TYPES.contains(&n.value_text().as_str()))
        .ok_or_else(|| failed("the Track Automation Event List shows no event type"))
}

/// Sets the list's event type pop-up (Notes, Fader…) through its menu.
fn choose_type(ctx: &mut Ctx, list: &Shown, window: &Node, item: &str) -> Done<()> {
    let popup = type_popup(window)?;
    if popup.value_text() == item {
        return Ok(());
    }
    let path = popup.path.clone();
    nudge::wrote(ctx.ax.action(&list.root, &path, "AXPress", 300))?;
    let menu = nudge::wrote(ctx.ax.tree_in_popup(&list.root, &path, 3, 40, Some(&["role", "title"])))?;
    let found = menu.find(&|n| n.role == "AXMenuItem" && n.title == item).map(|n| n.path.clone());
    let Some(found) = found else {
        if let Some(open) = menu.find(&|n| n.role == "AXMenu").map(|n| n.path.clone()) {
            let _ = ctx.ax.action_in_popup(&list.root, &open, "AXCancel", 100);
        }
        return Err(failed(format!("the event type menu has no {item}")));
    };
    nudge::wrote(ctx.ax.action_in_popup(&list.root, &found, "AXPress", 300))?;
    Ok(())
}

/// Makes one event: the playhead to its position, Create new Event, the new row's Num stepped to the parameter and its
/// Val to the value. The event as read after.
fn create(ctx: &mut Ctx, opened: &Opened, parameter: &Parameter, at: Position, value: u32) -> Done<AutomationEvent> {
    on_project(ctx, &opened.project, |ctx| timeline::set_playhead(ctx, Position { bar: at.bar, beat: at.beat, ..Position::START }))?;
    let (window, _, before) = table(ctx, &opened.list)?;
    let strings = &ctx.project.strings;
    let button = window
        .find(&|n| n.role == "AXButton" && strings.is(&n.desc, "Create new Event"))
        .map(|n| n.path.clone())
        .ok_or_else(|| failed("the Track Automation Event List shows no Create new Event"))?;
    nudge::wrote(ctx.ax.action(&opened.list.root, &button, "AXPress", LIST_SETTLE_MS))?;
    super::pause(LIST_SETTLE);
    let (_, columns, after) = table(ctx, &opened.list)?;
    let olds: Vec<&AutomationEvent> = before.iter().map(|(e, _)| e).collect();
    let mut made: Vec<(AutomationEvent, Node)> = after.into_iter().filter(|(e, _)| !olds.contains(&e)).collect();
    let (mut event, mut row) = match made.len() {
        1 => made.remove(0),
        n => {
            return Err(super::uncertain(format!("Logic Pro's Track Automation Event List shows {n} new events after Kumi asked for one")))
        }
    };
    // The parameter: stepped to its number, or one at a time until the Length/Info cell names it.
    let num = cell_slider(&row, &columns, "Num").ok_or_else(|| failed("the new event has no Num"))?;
    let control = Control::new(&opened.list.root, &num.path);
    match parameter {
        Parameter::Number(n) => {
            nudge::wrote(nudge::step_to(ctx.ax, &control, i64::from(event.number), i64::from(*n)))?;
        }
        Parameter::Named(name) => {
            let mut now = event.number;
            while !event.parameter.eq_ignore_ascii_case(name) {
                if now >= MOST_NUMBER {
                    let _ = ctx.ax.action(&opened.list.root, &row.path, super::marks::DELETE_ACTION, LIST_SETTLE_MS);
                    return Err(bad(format!(
                        "no automation parameter of this track is named {} (Kumi looked through numbers 0 to {MOST_NUMBER})",
                        quoted(name)
                    )));
                }
                nudge::wrote(nudge::step_to(ctx.ax, &control, i64::from(now), i64::from(now + 1)))?;
                now += 1;
                let (_, _, rows) = table(ctx, &opened.list)?;
                let found = rows
                    .into_iter()
                    .find(|(e, _)| e.position == event.position && e.number == now)
                    .ok_or_else(|| super::uncertain("the new event isn't in the list as Kumi set it"))?;
                event = found.0;
                row = found.1;
            }
        }
    }
    let (_, columns, rows) = table(ctx, &opened.list)?;
    let (found_event, found_row) = rows
        .into_iter()
        .find(|(e, _)| e.position == event.position && parameter.matches(e) && e.value == event.value)
        .ok_or_else(|| super::uncertain("the new event isn't in the list as Kumi set it"))?;
    event = found_event;
    if event.value != value {
        let val = cell_slider(&found_row, &columns, "Val").ok_or_else(|| failed("the new event has no Val"))?;
        nudge::wrote(nudge::step_to(ctx.ax, &Control::new(&opened.list.root, &val.path), i64::from(event.value), i64::from(value)))?;
    }
    super::pause(LIST_SETTLE);
    let (_, _, rows) = table(ctx, &opened.list)?;
    rows.into_iter().map(|(e, _)| e).find(|e| e.position == event.position && parameter.matches(e) && e.value == value).ok_or_else(|| {
        super::uncertain(format!("Logic Pro's Track Automation Event List doesn't show {} at {value} after Kumi set it", parameter.label()))
    })
}

/// Deletes the row of an event, found by its state.
fn delete_event(ctx: &mut Ctx, opened: &Opened, event: &AutomationEvent) -> Done<()> {
    let (_, _, rows) = table(ctx, &opened.list)?;
    let (_, row) = rows
        .into_iter()
        .find(|(e, _)| e == event)
        .ok_or_else(|| changed(format!("the {} event isn't in the list as it was", event.parameter)))?;
    nudge::wrote(ctx.ax.action(&opened.list.root, &row.path, super::marks::DELETE_ACTION, LIST_SETTLE_MS))?;
    super::pause(LIST_SETTLE);
    Ok(())
}

/// Opens the list for a write on the selected track and runs `with`; the list closed and the playhead put back after.
fn with_list<T>(ctx: &mut Ctx, keys: &KeyCommands, place: usize, with: impl FnOnce(&mut Ctx, &Opened) -> Done<T>) -> Done<T> {
    let track = super::tracks::at(ctx, place)?;
    let name = track.header.name.clone();
    panes::with_selected(ctx, place, |ctx| {
        let (_, transport) = timeline::control_bar(ctx)?;
        let was = transport.position();
        let project = Root::Title(project_title(ctx)?);
        let list = show(ctx, keys, &name)?;
        let opened = Opened { list, project };
        // The event type as the producer left it, put back after (a write sets it to Fader).
        let was_type = table(ctx, &opened.list).ok().and_then(|(window, _, _)| type_popup(&window).ok().map(Node::value_text));
        let result = with(ctx, &opened);
        if let (Some(was_type), Ok((window, _, _))) = (was_type, table(ctx, &opened.list)) {
            let _ = choose_type(ctx, &opened.list, &window, &was_type);
        }
        hide(ctx, &opened.list);
        ctx.project.forget();
        let _ = timeline::set_playhead(ctx, was);
        result
    })
}

/// The automation of a track as a change fences it: every Fader event.
fn fence_of(events: &[AutomationEvent], beats_per_bar: u32) -> Value {
    json!({"events": events_json(events, beats_per_bar)})
}

fn beats_per_bar(ctx: &mut Ctx) -> Done<u32> {
    Ok(timeline::control_bar(ctx)?.1.beats_per_bar())
}

fn track_target(ctx: &Ctx, place: usize) -> Value {
    json!({"ref": super::ref_text(ctx, "track", &[place]), "kind": "track"})
}

/// What a change asks: add (events), change (index, value?, positionBeats?), delete (indexes).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mode {
    Add,
    Change,
    Delete,
}

fn title(mode: Mode, count: usize, parameter: &Parameter, track: &str) -> String {
    let verb = match mode {
        Mode::Add => "Wrote",
        Mode::Change => "Changed",
        Mode::Delete => "Deleted",
    };
    format!("{verb} {count} {} automation event{} on {}", parameter.label(), if count == 1 { "" } else { "s" }, quoted(track))
}

pub fn preview(ctx: &mut Ctx, keys: &KeyCommands, input: &Value, mode: Mode) -> Done<Preview> {
    let found = super::tracks::find(ctx, input, "ref", Epoch::Current)?;
    let parameter = Parameter::of(input)?;
    let bpb = beats_per_bar(ctx)?;
    let count = asked_count(input, mode, bpb)?;
    let events = with_list(ctx, keys, found.place, |ctx, opened| Ok(table(ctx, &opened.list)?.2))?;
    let all: Vec<AutomationEvent> = events.into_iter().map(|(e, _)| e).collect();
    if mode != Mode::Add {
        check_indexes(input, mode, all.iter().filter(|e| parameter.matches(e)).count())?;
    }
    let fence = fence_of(&all, bpb);
    Ok(Preview {
        target: track_target(ctx, found.place),
        prior: fence.clone(),
        fence,
        display: json!({"title": title(mode, count, &parameter, &found.label())}),
    })
}

/// How many events a request names, checked.
fn asked_count(input: &Value, mode: Mode, bpb: u32) -> Done<usize> {
    let key = match mode {
        Mode::Add => "events",
        Mode::Change => "changes",
        Mode::Delete => "indexes",
    };
    let list = input.get(key).and_then(Value::as_array).filter(|l| !l.is_empty()).ok_or_else(|| bad(format!("{key}: a list")))?;
    if list.len() > MOST_EVENTS {
        return Err(bad(format!("at most {MOST_EVENTS} events at once")));
    }
    if mode == Mode::Add {
        for spec in list {
            point_of(spec, bpb)?;
        }
    }
    Ok(list.len())
}

/// The indexes a change or delete names, checked against the parameter's events.
fn check_indexes(input: &Value, mode: Mode, count: usize) -> Done<Vec<usize>> {
    let indexes: Vec<Option<u64>> = match mode {
        Mode::Delete => input["indexes"].as_array().into_iter().flatten().map(Value::as_u64).collect(),
        _ => input["changes"].as_array().into_iter().flatten().map(|c| c.get("index").and_then(Value::as_u64)).collect(),
    };
    let mut seen = Vec::new();
    for index in indexes {
        let index = index.map(|i| i as usize).filter(|i| *i < count).ok_or_else(|| {
            bad(format!("every index must be the place of an event of that parameter, from 0 (it has {count}): read the automation again"))
        })?;
        if seen.contains(&index) {
            return Err(bad("an event is named twice"));
        }
        seen.push(index);
    }
    Ok(seen)
}

pub fn apply(ctx: &mut Ctx, keys: &KeyCommands, input: &Value, fence: &Value, mode: Mode) -> Done<Applied> {
    let found = super::tracks::find(ctx, input, "ref", Epoch::Current).map_err(|f| if f.0 == "failed" { changed(f.1) } else { f })?;
    let parameter = Parameter::of(input)?;
    let bpb = beats_per_bar(ctx)?;
    asked_count(input, mode, bpb)?;
    let label = found.label();
    let (made, removed, after) = with_list(ctx, keys, found.place, |ctx, opened| {
        let (window, _, rows) = table(ctx, &opened.list)?;
        let all: Vec<AutomationEvent> = rows.iter().map(|(e, _)| e.clone()).collect();
        if fence_of(&all, bpb) != *fence {
            return Err(changed(format!("{}'s automation changed since the preview", quoted(&label))));
        }
        let mine: Vec<AutomationEvent> = all.iter().filter(|e| parameter.matches(e)).cloned().collect();
        let (mut made, mut removed) = (Vec::new(), Vec::new());
        match mode {
            Mode::Add => {
                choose_type(ctx, &opened.list, &window, FADER)?;
                for spec in input["events"].as_array().into_iter().flatten() {
                    let (at, value) = point_of(spec, bpb)?;
                    made.push(create(ctx, opened, &parameter, at, value)?);
                }
            }
            Mode::Delete => {
                for index in check_indexes(input, mode, mine.len())? {
                    delete_event(ctx, opened, &mine[index])?;
                    removed.push(mine[index].clone());
                }
            }
            Mode::Change => {
                choose_type(ctx, &opened.list, &window, FADER)?;
                for change in input["changes"].as_array().into_iter().flatten() {
                    let index = change["index"].as_u64().unwrap_or(0) as usize;
                    let was = mine.get(index).cloned().ok_or_else(|| bad("no such event"))?;
                    let at = match change.get("positionBeats").filter(|v| !v.is_null()) {
                        Some(_) => point_of(&json!({"positionBeats": change["positionBeats"], "value": 0}), bpb)?.0,
                        None => was.position,
                    };
                    let value = match change.get("value").filter(|v| !v.is_null()) {
                        Some(v) => v.as_u64().filter(|v| *v <= 127).ok_or_else(|| bad("value must be from 0 to 127"))? as u32,
                        None => was.value,
                    };
                    // A change is the event taken out and made again where and as asked: Logic's Event List moves a
                    // row's position a segment at a time, which this does in two steps instead.
                    delete_event(ctx, opened, &was)?;
                    removed.push(was.clone());
                    made.push(create(ctx, opened, &Parameter::Number(was.number), at, value)?);
                }
            }
        }
        let (_, _, rows) = table(ctx, &opened.list)?;
        Ok((made, removed, rows.into_iter().map(|(e, _)| e).collect::<Vec<_>>()))
    })?;
    let count = made.len().max(removed.len());
    Ok(Applied {
        applied: json!({"made": events_json(&made, bpb), "removed": events_json(&removed, bpb), "events": events_json(&after, bpb)}),
        display: json!({"title": title(mode, count, &parameter, &label)}),
        step: Some(match mode {
            Mode::Delete => "Delete",
            _ => "Value Change",
        }),
        ..Applied::default()
    })
}

/// An event as a change kept it.
fn event_of(value: &Value, bpb: u32) -> Option<AutomationEvent> {
    Some(AutomationEvent {
        position: Position::from_beats(value["positionBeats"].as_f64()?, bpb),
        number: value["number"].as_u64()? as u32,
        parameter: value["parameter"].as_str().unwrap_or_default().to_string(),
        value: value["value"].as_u64()? as u32,
    })
}

pub fn restore(ctx: &mut Ctx, keys: &KeyCommands, restore: &Restore) -> Done<Restored> {
    let found = super::tracks::find(ctx, restore.target, "ref", Epoch::Any).map_err(|f| if f.0 == "failed" { gone(f.1) } else { f })?;
    let bpb = beats_per_bar(ctx)?;
    let made: Vec<AutomationEvent> = restore.applied["made"].as_array().into_iter().flatten().filter_map(|v| event_of(v, bpb)).collect();
    let removed: Vec<AutomationEvent> =
        restore.applied["removed"].as_array().into_iter().flatten().filter_map(|v| event_of(v, bpb)).collect();
    let label = found.label();
    let after = with_list(ctx, keys, found.place, |ctx, opened| {
        let (window, _, rows) = table(ctx, &opened.list)?;
        let all: Vec<AutomationEvent> = rows.iter().map(|(e, _)| e.clone()).collect();
        if !restore.discard && events_json(&all, bpb) != restore.applied["events"].as_array().cloned().unwrap_or_default() {
            return Err(changed(format!("{}'s automation has been changed in Logic Pro since Kumi wrote it", quoted(&label))));
        }
        for event in &made {
            if all.contains(event) {
                delete_event(ctx, opened, event)?;
            }
        }
        if !removed.is_empty() {
            choose_type(ctx, &opened.list, &window, FADER)?;
        }
        for event in &removed {
            create(ctx, opened, &Parameter::Number(event.number), event.position, event.value)?;
        }
        let (_, _, rows) = table(ctx, &opened.list)?;
        Ok(rows.into_iter().map(|(e, _)| e).collect::<Vec<_>>())
    })?;
    let back = fence_of(&after, bpb);
    if back["events"] != restore.prior["events"] {
        return Err(super::uncertain(format!("{}'s automation isn't as it was after Kumi put it back", quoted(&label))));
    }
    Ok(Restored::by_value(json!({"events": back["events"]}), json!({"title": format!("{}'s automation put back", quoted(&label))})))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rows::Position;

    #[test]
    fn automation_events_are_grouped_by_their_parameter() {
        let at = |bar: u32| Position { bar, ..Position::START };
        let events = vec![
            AutomationEvent { position: at(1), number: 7, parameter: "Volume".into(), value: 90 },
            AutomationEvent { position: at(1), number: 8, parameter: "Balance".into(), value: 64 },
            AutomationEvent { position: at(3), number: 7, parameter: "Volume".into(), value: 100 },
        ];
        let rows = grouped(&events, 4);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].0, "Volume");
        assert_eq!(rows[0].2, vec![json!({"positionBeats": 0.0, "value": 90}), json!({"positionBeats": 8.0, "value": 100})]);
        assert_eq!((rows[1].0.as_str(), rows[1].1), ("Balance", 8));
    }

    #[test]
    fn an_automation_change_names_its_parameter_and_events_on_whole_beats() {
        assert_eq!(Parameter::of(&json!({"parameter": "Volume"})).unwrap(), Parameter::Named("Volume".into()));
        assert_eq!(Parameter::of(&json!({"parameter": 8})).unwrap(), Parameter::Number(8));
        assert!(Parameter::of(&json!({"parameter": ""})).is_err());
        assert!(Parameter::of(&json!({"parameter": 20000})).is_err());
        let (at, value) = point_of(&json!({"positionBeats": 5, "value": 90}), 4).unwrap();
        assert_eq!((at.bar, at.beat, value), (2, 2, 90));
        assert!(point_of(&json!({"positionBeats": 1.5, "value": 90}), 4).is_err());
        assert!(point_of(&json!({"positionBeats": 1, "value": 128}), 4).is_err());
        assert_eq!(asked_count(&json!({"events": [{"positionBeats": 0, "value": 1}]}), Mode::Add, 4).unwrap(), 1);
        assert!(asked_count(&json!({"events": []}), Mode::Add, 4).is_err());
        assert_eq!(check_indexes(&json!({"indexes": [1, 0]}), Mode::Delete, 2).unwrap(), vec![1, 0]);
        assert!(check_indexes(&json!({"indexes": [2]}), Mode::Delete, 2).is_err());
        assert!(check_indexes(&json!({"indexes": [0, 0]}), Mode::Delete, 2).is_err());
        assert!(check_indexes(&json!({"changes": [{"index": 0, "value": 3}]}), Mode::Change, 1).is_ok());
    }
}
