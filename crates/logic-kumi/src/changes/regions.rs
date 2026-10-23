//! Regions in the Tracks area (what REAPER calls items), each through one channel and read back from the Tracks
//! area's layout items (M2's region rows, which say where a region starts and ends and what it is called):
//!
//! - `region-move`: the Event List at the region level (its Leave Folder button takes the Event tab from a region's
//!   events up to the regions of the Tracks area, as rows with Position and Length scrubbers, a Name text field and
//!   the track's number), the row's Position segments stepped as a marker's are. The list comes back to the region's
//!   events when the pane is closed and opened again, which the reader's note rows need.
//! - `region-resize`: the region selected alone, the playhead at the new edge, Logic's own Set Region/Event/Marquee
//!   Start (or End) to Playhead Position (Cmd-[ and Cmd-], through the key command set): the start edge moves and the
//!   notes stay where they are, as REAPER's trim does.
//! - `region-split`: the region selected alone, the playhead there, Split Regions/Events at Playhead Position
//!   (Cmd-T); the right part comes back as `created`.
//! - `region-loop`: the Region inspector's Loop checkbox, pressed (Logic toggles it and answers
//!   `attributeUnsupported` all the same; a click at its frame does nothing), read back from the checkbox: the
//!   Tracks area's region item says nothing of its loop that Accessibility reads (M2's `looped` never reads true).
//! - `region-rename`: the Region inspector's name field, confirmed (the region selected through its item, which
//!   the Inspector follows; the key commands below need a click, for the keyboard focus).
//! - `region-delete`: the region selected alone, the Delete key.
//!
//! A region added, removed, moved, resized or renamed moves the epoch (M2's rule), so an apply answers the region's
//! ref in the new epoch as its `target` and a restore names it by that. A split or a deleted region comes back only
//! through Logic's own undo while that step is still Logic's latest.

use std::time::Duration;

use serde_json::{json, Value};

use super::nudge::{self, Control};
use super::{changed, front, gone, quoted, ref_text, timeline, tracks, Applied, Ctx, Done, Epoch, Preview, Restore, Restored, Watch};
use crate::ax::paths;
use crate::ax::Node;
use crate::keys::{Chord, KeyCommands};
use crate::project::ListOpen;
use crate::refs::{bad, failed};
use crate::rows::{self, Position, Region};

/// How long the list gets after a press before its rows are read, and a key before the regions are.
const LIST_SETTLE: Duration = Duration::from_millis(250);
const LIST_SETTLE_MS: u64 = 250;
const KEY_SETTLE: Duration = Duration::from_millis(400);
const SELECT_SETTLE_MS: u64 = 300;

/// Logic's key commands the region kinds post, with the factory chords for when the set has none.
const SPLIT: (u32, &str) = (574, "cmd-t");
const START_TO_PLAYHEAD: (u32, &str) = (1500, "cmd-[");
const END_TO_PLAYHEAD: (u32, &str) = (1501, "cmd-]");

/// A region as read now: its place through the project (from 1), its track's place and its row.
#[derive(Debug, Clone)]
pub struct Found {
    pub place: usize,
    pub track: usize,
    pub region: Region,
}

/// Every region as read now, in the order they are numbered.
fn all(ctx: &mut Ctx) -> Done<Vec<Found>> {
    nudge::read(tracks::ensure_layout(ctx))?;
    let regions = nudge::read(ctx.project.read_regions(ctx.ax))?;
    Ok(regions.into_iter().enumerate().map(|(i, (track, region))| Found { place: i + 1, track, region }).collect())
}

fn find(ctx: &mut Ctx, input: &Value, key: &str, epoch: Epoch) -> Done<Found> {
    let place = super::region_place(ctx, input, key, epoch)?;
    let mut regions = all(ctx)?;
    if place == 0 || place > regions.len() {
        return Err(crate::refs::not_in_project(input[key].as_str().unwrap_or_default()));
    }
    Ok(regions.swap_remove(place - 1))
}

fn find_for_apply(ctx: &mut Ctx, input: &Value) -> Done<Found> {
    find(ctx, input, "ref", Epoch::Current).map_err(|fault| {
        if fault.0 == "failed" {
            changed(format!("{}: it went since the preview", fault.1))
        } else {
            fault
        }
    })
}

fn find_for_restore(ctx: &mut Ctx, target: &Value) -> Done<Found> {
    find(ctx, target, "ref", Epoch::Any).map_err(|fault| if fault.0 == "failed" { gone(fault.1) } else { fault })
}

fn beats_per_bar(ctx: &mut Ctx) -> Done<u32> {
    Ok(timeline::control_bar(ctx)?.1.beats_per_bar())
}

/// A region's bounds and name as the state the journal keeps.
fn state(found: &Found, beats_per_bar: u32) -> Value {
    json!({
        "startBeats": found.region.start.beats(beats_per_bar),
        "endBeats": found.region.end.beats(beats_per_bar),
        "name": found.region.name,
        "track": found.track,
        "looped": found.region.looped,
    })
}

fn label(found: &Found) -> String {
    if found.region.name.is_empty() {
        format!("region {}", found.place)
    } else {
        format!("region {}", quoted(&rows::clip(&found.region.name)))
    }
}

fn target(ctx: &Ctx, place: usize) -> Value {
    json!({"ref": ref_text(ctx, "region", &[place]), "kind": "region"})
}

fn beats_input(input: &Value, key: &str) -> Done<Option<f64>> {
    match input.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(value) => value
            .as_f64()
            .filter(|n| n.is_finite() && (0.0..=1.0e6).contains(n))
            .map(Some)
            .ok_or_else(|| bad(format!("{key} must be a number from 0: beats from the project's start"))),
    }
}

/// `positionBeats`, refusing REAPER's seconds.
fn position_input(input: &Value) -> Done<f64> {
    if input.get("position").is_some_and(|v| !v.is_null()) && input.get("positionBeats").is_none_or(Value::is_null) {
        return Err(bad("Logic Pro's bridge places regions by positionBeats (beats from the project's start), not seconds"));
    }
    beats_input(input, "positionBeats")?.ok_or_else(|| bad("positionBeats must be a number from 0: beats from the project's start"))
}

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() < 1.0 / f64::from(rows::TICKS_PER_BEAT)
}

/// The region whose track, name and bounds are `state`'s, read now: where it is after a move or a resize.
fn by_state(regions: &[Found], state: &Value, beats_per_bar: u32) -> Option<Found> {
    regions
        .iter()
        .find(|found| {
            let now = self::state(found, beats_per_bar);
            now["track"] == state["track"]
                && now["name"] == state["name"]
                && close(now["startBeats"].as_f64().unwrap_or(-1.0), state["startBeats"].as_f64().unwrap_or(-2.0))
                && close(now["endBeats"].as_f64().unwrap_or(-1.0), state["endBeats"].as_f64().unwrap_or(-2.0))
        })
        .cloned()
}

// ----- selection -----

/// Selects a region alone, as a producer does: a click at its middle (the edges resize or loop), with Logic in
/// front. The key commands act on every selected region in the area that has the keyboard focus, and a click gives
/// the Tracks area the focus where a set of `AXSelected` doesn't.
fn select_only(ctx: &mut Ctx, place: usize) -> Done<()> {
    nudge::read(tracks::ensure_layout(ctx))?;
    let contents = nudge::read(ctx.project.contents(ctx.ax))?;
    let items: Vec<&Node> = contents
        .children
        .iter()
        .filter(|c| c.role == "AXLayoutArea")
        .flat_map(|lane| lane.children.iter().filter(|c| c.role == "AXLayoutItem"))
        .collect();
    let paths: Vec<String> = items.iter().map(|item| item.path.clone()).collect();
    if place == 0 || place > paths.len() {
        return Err(gone("that region isn't in the Tracks area any more"));
    }
    let root = ctx.project.root.clone();
    front::ensure(ctx.ax)?;
    let (x, y, w, h) = nudge::read(ctx.ax.frame(&root, &paths[place - 1]))?;
    nudge::wrote(ctx.ax.click(x + w / 2.0, y + h / 2.0))?;
    super::pause(Duration::from_millis(SELECT_SETTLE_MS));
    let after = all(ctx)?;
    let selected: Vec<usize> = after.iter().filter(|f| f.region.selected).map(|f| f.place).collect();
    if selected != [place] {
        return Err(failed(format!("Logic Pro didn't select region {place} alone (selected: {selected:?}); nothing changed")));
    }
    Ok(())
}

/// A key command's chord from the set, else the factory's.
fn chord_of(keys: &KeyCommands, command: (u32, &str)) -> Done<Chord> {
    keys.chord(command.0)
        .cloned()
        .or_else(|| Chord::parse(command.1))
        .ok_or_else(|| failed(format!("no key for Logic Pro's command {}", command.0)))
}

/// Logic in front, a chord posted, a moment for the Tracks area to follow.
fn post(ctx: &mut Ctx, chord: &Chord) -> Done<()> {
    front::ensure(ctx.ax)?;
    front::post(ctx.ax, chord)?;
    super::pause(KEY_SETTLE);
    Ok(())
}

// ----- the Event List at the region level -----

/// The Event List shown at the region level: the pane and tab as they were, and the columns.
struct RegionList {
    open: ListOpen,
    was_open: bool,
    columns: Vec<(String, usize)>,
}

/// A row of the region-level list: the region it stands for, by track and bounds, and its node.
struct Row {
    track: usize,
    position: Position,
    name: String,
    node: Node,
}

/// The list's header texts and buttons.
fn header(ctx: &mut Ctx) -> Done<Node> {
    nudge::read(ctx.project.list_group(ctx.ax))
}

/// The region path the Event List shows: empty at the region level.
fn region_path(ctx: &mut Ctx) -> Done<String> {
    let group = header(ctx)?;
    let strings = &ctx.project.strings;
    Ok(group.find(&|n| n.role == "AXStaticText" && strings.is(&n.desc, "Region Path")).map(Node::value_text).unwrap_or_default())
}

/// Shows the Event List and takes it to the region level with its Leave Folder button.
fn show(ctx: &mut Ctx) -> Done<RegionList> {
    let was_open = {
        let (bar, _) = timeline::control_bar(ctx)?;
        paths::view_button(&bar, &ctx.project.strings, "List Editors").is_some_and(Node::is_on)
    };
    let open = nudge::read(ctx.project.show_list(ctx.ax, "Event"))?;
    for _ in 0..4 {
        if region_path(ctx)?.is_empty() {
            break;
        }
        let group = header(ctx)?;
        let strings = &ctx.project.strings;
        let button = group
            .find(&|n| n.role == "AXButton" && strings.is(&n.desc, "Leave Folder"))
            .ok_or_else(|| failed("Logic Pro's Event List doesn't show its Leave Folder button"))?;
        let control = Control::new(&ctx.project.root, &button.path);
        nudge::wrote(nudge::press(ctx.ax, &control, LIST_SETTLE_MS))?;
        super::pause(LIST_SETTLE);
    }
    if !region_path(ctx)?.is_empty() {
        let _ = ctx.project.hide_list(ctx.ax, open);
        return Err(failed("Logic Pro's Event List wouldn't leave the region for the Tracks area's regions"));
    }
    let (columns, _) = nudge::read(ctx.project.list_table(ctx.ax, "Event"))?;
    Ok(RegionList { open, was_open, columns })
}

/// Puts the List Editors back: the pane closed, and opened again when it was open before, so the Event List shows
/// the selected region's events again (the level it comes back at).
fn hide(ctx: &mut Ctx, list: RegionList) {
    let _ = ctx.project.close_pane(ctx.ax, "List Editors");
    if list.was_open {
        let _ = ctx.project.open_pane(ctx.ax, "List Editors");
    }
    let _ = list.open;
    ctx.project.forget();
}

fn cell_text(row: &Node, columns: &[(String, usize)], id: &str) -> Option<String> {
    let index = columns.iter().find(|(name, _)| name == id)?.1;
    let cell = row.children.iter().filter(|c| c.role == "AXCell").nth(index)?;
    let inner = cell.find(&|n| n.role == "AXTextField" || n.role == "AXStaticText").or_else(|| cell.children.first())?;
    let text = inner.value_text();
    Some(if text.is_empty() { inner.desc.clone() } else { text })
}

fn cell_group<'a>(row: &'a Node, columns: &[(String, usize)], id: &str) -> Option<&'a Node> {
    let index = columns.iter().find(|(name, _)| name == id)?.1;
    row.children.iter().filter(|c| c.role == "AXCell").nth(index)?.children.first()
}

fn rows(ctx: &mut Ctx) -> Done<Vec<Row>> {
    let (columns, table) = nudge::read(ctx.project.list_table(ctx.ax, "Event"))?;
    Ok(paths::rows(&table)
        .into_iter()
        .filter_map(|row| {
            let position = cell_group(row, &columns, "Position").and_then(rows::position_of)?;
            let track = cell_text(row, &columns, "Track").and_then(|t| t.trim().parse::<usize>().ok()).unwrap_or(0);
            let name = cell_text(row, &columns, "Name").unwrap_or_default();
            Some(Row { track, position, name, node: row.clone() })
        })
        .collect())
}

/// The row of a region: by its track and position, its name telling two apart.
fn row_of<'a>(list: &'a [Row], track: usize, position: &Position, name: &str) -> Option<&'a Row> {
    let same: Vec<&Row> = list.iter().filter(|row| row.track == track && row.position == *position).collect();
    same.iter().find(|row| row.name == name).or(same.first()).copied()
}

// ----- moving -----

pub fn move_preview(ctx: &mut Ctx, input: &Value) -> Done<Preview> {
    if input.get("toRef").is_some_and(|v| !v.is_null()) {
        return Err(bad("Logic Pro's Event List moves a region in time, not to another track: leave toRef out"));
    }
    let found = find(ctx, input, "ref", Epoch::Current)?;
    let beats_per_bar = beats_per_bar(ctx)?;
    let to = position_input(input)?;
    let now = state(&found, beats_per_bar);
    let length = now["endBeats"].as_f64().unwrap_or(0.0) - now["startBeats"].as_f64().unwrap_or(0.0);
    let after = json!({"startBeats": to, "endBeats": to + length, "name": now["name"], "track": now["track"], "looped": now["looped"]});
    let mut display = json!({"title": format!("Moved {} to {}", label(&found), timeline::bar_text(&Position::from_beats(to, beats_per_bar))), "before": now, "after": after});
    if close(now["startBeats"].as_f64().unwrap_or(-1.0), to) {
        display["unchanged"] = true.into();
    }
    Ok(Preview { target: target(ctx, found.place), prior: now.clone(), fence: now, display })
}

/// Moves a region's row to `to` through the list's Position segments, finding it again after each step.
fn move_row(ctx: &mut Ctx, list: &RegionList, found: &Found, to: Position) -> Done<Position> {
    let columns = list.columns.clone();
    let (track, name) = (found.track, found.region.name.clone());
    super::marks::step_position(ctx, &columns, found.region.start, to, "the region", |ctx, at| {
        let rows = rows(ctx)?;
        let row = row_of(&rows, track, at, &name)
            .ok_or_else(|| super::uncertain(format!("the region isn't in the Event List at {} any more", timeline::bar_text(at))))?;
        Ok(row.node.clone())
    })
}

pub fn move_apply(ctx: &mut Ctx, input: &Value, fence: &Value) -> Done<Applied> {
    let found = find_for_apply(ctx, input)?;
    let beats_per_bar = beats_per_bar(ctx)?;
    let to = position_input(input)?;
    let now = state(&found, beats_per_bar);
    if now != *fence {
        return Err(changed(format!("{} changed since the preview", label(&found))));
    }
    let length = now["endBeats"].as_f64().unwrap_or(0.0) - now["startBeats"].as_f64().unwrap_or(0.0);
    let wanted = json!({"startBeats": to, "endBeats": to + length, "name": now["name"], "track": now["track"], "looped": now["looped"]});
    let title = format!("Moved {} to {}", label(&found), timeline::bar_text(&Position::from_beats(to, beats_per_bar)));
    if close(now["startBeats"].as_f64().unwrap_or(-1.0), to) {
        return Ok(Applied {
            applied: now.clone(),
            target: Some(target(ctx, found.place)),
            display: json!({"title": title}),
            ..Applied::default()
        });
    }
    let list = show(ctx)?;
    let moved = move_row(ctx, &list, &found, Position::from_beats(to, beats_per_bar));
    hide(ctx, list);
    let reached = moved?;
    // The Tracks area shows it where it is now (the epoch moves with it).
    timeline::look(ctx).map_err(|(_, why)| super::uncertain(why))?;
    let regions = all(ctx).map_err(|(_, why)| super::uncertain(why))?;
    let after = by_state(&regions, &wanted, beats_per_bar).ok_or_else(|| {
        nudge::kept(
            &format!("{}'s position", label(&found)),
            timeline::bar_text(&Position::from_beats(to, beats_per_bar)),
            timeline::bar_text(&reached),
        )
    })?;
    Ok(Applied {
        applied: state(&after, beats_per_bar),
        target: Some(target(ctx, after.place)),
        display: json!({"title": title, "before": now, "after": state(&after, beats_per_bar)}),
        step: Some("Value Change"),
        ..Applied::default()
    })
}

pub fn move_restore(ctx: &mut Ctx, restore: &Restore) -> Done<Restored> {
    let beats_per_bar = beats_per_bar(ctx)?;
    let regions = all(ctx)?;
    if let Some(back) = by_state(&regions, restore.prior, beats_per_bar) {
        return Ok(Restored::by_value(state(&back, beats_per_bar), json!({"title": format!("{} is where it was", label(&back))})));
    }
    let found = match by_state(&regions, restore.applied, beats_per_bar) {
        Some(found) => found,
        None if restore.discard => find_for_restore(ctx, restore.target)?,
        None => return Err(changed("the region has been moved in Logic Pro since Kumi moved it")),
    };
    let to = restore.prior["startBeats"].as_f64().ok_or_else(|| bad("prior.startBeats"))?;
    let list = show(ctx)?;
    let moved = move_row(ctx, &list, &found, Position::from_beats(to, beats_per_bar));
    hide(ctx, list);
    let reached = moved?;
    timeline::look(ctx).map_err(|(_, why)| super::uncertain(why))?;
    let regions = all(ctx).map_err(|(_, why)| super::uncertain(why))?;
    let back = by_state(&regions, restore.prior, beats_per_bar).ok_or_else(|| {
        nudge::kept(
            &format!("{}'s position", label(&found)),
            timeline::bar_text(&Position::from_beats(to, beats_per_bar)),
            timeline::bar_text(&reached),
        )
    })?;
    Ok(Restored::by_value(
        state(&back, beats_per_bar),
        json!({"title": format!("{} moved back to {}", label(&back), timeline::bar_text(&back.region.start))}),
    ))
}

// ----- resizing -----

fn edges_input(input: &Value, now: &Value) -> Done<(Option<f64>, Option<f64>)> {
    if (input.get("start").is_some_and(|v| !v.is_null()) || input.get("end").is_some_and(|v| !v.is_null()))
        && input.get("startBeats").is_none_or(Value::is_null)
        && input.get("endBeats").is_none_or(Value::is_null)
    {
        return Err(bad(
            "Logic Pro's bridge moves a region's edges by startBeats and endBeats (beats from the project's start), not seconds",
        ));
    }
    let start = beats_input(input, "startBeats")?;
    let end = beats_input(input, "endBeats")?;
    if start.is_none() && end.is_none() {
        return Err(bad("pass the new start (startBeats), the new end (endBeats), or both"));
    }
    let (s, e) = (start.unwrap_or(now["startBeats"].as_f64().unwrap_or(0.0)), end.unwrap_or(now["endBeats"].as_f64().unwrap_or(0.0)));
    if e <= s + 1.0 / f64::from(rows::TICKS_PER_BEAT) {
        return Err(bad("the region must end after it starts"));
    }
    Ok((start, end))
}

fn edges_title(found: &Found, start: f64, end: f64, beats_per_bar: u32) -> String {
    format!(
        "Trimmed {} to {}–{}",
        label(found),
        timeline::bar_text(&Position::from_beats(start, beats_per_bar)),
        timeline::bar_text(&Position::from_beats(end, beats_per_bar))
    )
}

pub fn resize_preview(ctx: &mut Ctx, input: &Value) -> Done<Preview> {
    let found = find(ctx, input, "ref", Epoch::Current)?;
    let beats_per_bar = beats_per_bar(ctx)?;
    let now = state(&found, beats_per_bar);
    let (start, end) = edges_input(input, &now)?;
    let (s, e) = (start.unwrap_or(now["startBeats"].as_f64().unwrap_or(0.0)), end.unwrap_or(now["endBeats"].as_f64().unwrap_or(0.0)));
    let after = json!({"startBeats": s, "endBeats": e, "name": now["name"], "track": now["track"], "looped": now["looped"]});
    let mut display = json!({"title": edges_title(&found, s, e, beats_per_bar), "before": now, "after": after});
    if close(s, now["startBeats"].as_f64().unwrap_or(-1.0)) && close(e, now["endBeats"].as_f64().unwrap_or(-1.0)) {
        display["unchanged"] = true.into();
    }
    Ok(Preview { target: target(ctx, found.place), prior: now.clone(), fence: now, display })
}

/// Sets a region's edges: selected alone, the playhead at each new edge and Logic's own command for it posted.
/// The edges to set, each on a beat with its command, both checked before either is written: an edge refused once the
/// other had moved would leave half a resize.
fn edges_on_the_beat(start: Option<f64>, end: Option<f64>, beats_per_bar: u32) -> Done<Vec<(Position, (u32, &'static str))>> {
    let edges: Vec<(Position, (u32, &'static str))> = [(start, START_TO_PLAYHEAD), (end, END_TO_PLAYHEAD)]
        .into_iter()
        .filter_map(|(beats, command)| beats.map(|beats| (Position::from_beats(beats, beats_per_bar), command)))
        .collect();
    if edges.iter().any(|(position, _)| position.division > 1 || position.tick > 1) {
        return Err(bad("Logic Pro's playhead display places an edge on a beat: give whole beats"));
    }
    Ok(edges)
}

fn put_edges(
    ctx: &mut Ctx,
    keys: &KeyCommands,
    found: &Found,
    start: Option<f64>,
    end: Option<f64>,
    beats_per_bar: u32,
) -> Done<Vec<Found>> {
    let edges = edges_on_the_beat(start, end, beats_per_bar)?;
    let (_, before) = timeline::control_bar(ctx)?;
    let was = before.position();
    select_only(ctx, found.place)?;
    for (position, command) in edges {
        timeline::set_playhead(ctx, position)?;
        let chord = chord_of(keys, command)?;
        post(ctx, &chord)?;
    }
    let _ = timeline::set_playhead(ctx, was);
    timeline::look(ctx).map_err(|(_, why)| super::uncertain(why))?;
    all(ctx).map_err(|(_, why)| super::uncertain(why))
}

pub fn resize_apply(ctx: &mut Ctx, keys: &KeyCommands, input: &Value, fence: &Value) -> Done<Applied> {
    let found = find_for_apply(ctx, input)?;
    let beats_per_bar = beats_per_bar(ctx)?;
    let now = state(&found, beats_per_bar);
    if now != *fence {
        return Err(changed(format!("{} changed since the preview", label(&found))));
    }
    let (start, end) = edges_input(input, &now)?;
    let (s, e) = (start.unwrap_or(now["startBeats"].as_f64().unwrap_or(0.0)), end.unwrap_or(now["endBeats"].as_f64().unwrap_or(0.0)));
    let wanted = json!({"startBeats": s, "endBeats": e, "name": now["name"], "track": now["track"], "looped": now["looped"]});
    let title = edges_title(&found, s, e, beats_per_bar);
    if wanted == now {
        return Ok(Applied {
            applied: now.clone(),
            target: Some(target(ctx, found.place)),
            display: json!({"title": title}),
            ..Applied::default()
        });
    }
    let regions = put_edges(
        ctx,
        keys,
        &found,
        start.filter(|s| !close(*s, now["startBeats"].as_f64().unwrap_or(-1.0))),
        end.filter(|e| !close(*e, now["endBeats"].as_f64().unwrap_or(-1.0))),
        beats_per_bar,
    )?;
    let after = by_state(&regions, &wanted, beats_per_bar).ok_or_else(|| {
        let shown = regions
            .iter()
            .find(|r| r.track == found.track && r.region.name == found.region.name)
            .map(|r| state(r, beats_per_bar))
            .unwrap_or(Value::Null);
        nudge::kept(&format!("{}'s edges", label(&found)), wanted.clone(), shown)
    })?;
    Ok(Applied {
        applied: state(&after, beats_per_bar),
        target: Some(target(ctx, after.place)),
        display: json!({"title": title, "before": now, "after": state(&after, beats_per_bar)}),
        step: Some("Set Region/Event/Marquee Start/End"),
        ..Applied::default()
    })
}

pub fn resize_restore(ctx: &mut Ctx, keys: &KeyCommands, restore: &Restore) -> Done<Restored> {
    let beats_per_bar = beats_per_bar(ctx)?;
    let regions = all(ctx)?;
    if let Some(back) = by_state(&regions, restore.prior, beats_per_bar) {
        return Ok(Restored::by_value(state(&back, beats_per_bar), json!({"title": format!("{} is as it was", label(&back))})));
    }
    let found = match by_state(&regions, restore.applied, beats_per_bar) {
        Some(found) => found,
        None if restore.discard => find_for_restore(ctx, restore.target)?,
        None => return Err(changed("the region has been changed in Logic Pro since Kumi trimmed it")),
    };
    let (s, e) = (
        restore.prior["startBeats"].as_f64().ok_or_else(|| bad("prior.startBeats"))?,
        restore.prior["endBeats"].as_f64().ok_or_else(|| bad("prior.endBeats"))?,
    );
    let now = state(&found, beats_per_bar);
    let start = (!close(s, now["startBeats"].as_f64().unwrap_or(-1.0))).then_some(s);
    let end = (!close(e, now["endBeats"].as_f64().unwrap_or(-1.0))).then_some(e);
    let regions = put_edges(ctx, keys, &found, start, end, beats_per_bar)?;
    let back = by_state(&regions, restore.prior, beats_per_bar)
        .ok_or_else(|| nudge::kept(&format!("{}'s edges", label(&found)), restore.prior.clone(), Value::Null))?;
    Ok(Restored::by_value(state(&back, beats_per_bar), json!({"title": format!("{} back to its length", label(&back))})))
}

// ----- splitting -----

pub fn split_preview(ctx: &mut Ctx, input: &Value) -> Done<Preview> {
    let found = find(ctx, input, "ref", Epoch::Current)?;
    let beats_per_bar = beats_per_bar(ctx)?;
    let at = position_input(input)?;
    let now = state(&found, beats_per_bar);
    let (s, e) = (now["startBeats"].as_f64().unwrap_or(0.0), now["endBeats"].as_f64().unwrap_or(0.0));
    if at <= s || at >= e {
        return Err(bad(format!(
            "that's not inside the region: it runs from {} to {}",
            timeline::bar_text(&found.region.start),
            timeline::bar_text(&found.region.end)
        )));
    }
    let position = Position::from_beats(at, beats_per_bar);
    if position.division > 1 || position.tick > 1 {
        return Err(bad("Logic Pro's playhead display splits on a beat: give whole beats"));
    }
    Ok(Preview {
        target: target(ctx, found.place),
        prior: now.clone(),
        fence: json!({"region": now, "regions": all(ctx)?.len()}),
        display: json!({"title": format!("Split {} at {}", label(&found), timeline::bar_text(&position)), "note": "Logic Pro's own undo puts the region back whole while the split is its latest step."}),
    })
}

pub fn split_apply(ctx: &mut Ctx, keys: &KeyCommands, input: &Value, fence: &Value) -> Done<Applied> {
    let found = find_for_apply(ctx, input)?;
    let beats_per_bar = beats_per_bar(ctx)?;
    let now = state(&found, beats_per_bar);
    let before = all(ctx)?;
    if json!({"region": now, "regions": before.len()}) != *fence {
        return Err(changed(format!("{} or the regions changed since the preview", label(&found))));
    }
    let at = position_input(input)?;
    let position = Position::from_beats(at, beats_per_bar);
    let title = format!("Split {} at {}", label(&found), timeline::bar_text(&position));
    let (_, transport) = timeline::control_bar(ctx)?;
    let was = transport.position();
    select_only(ctx, found.place)?;
    timeline::set_playhead(ctx, position)?;
    let chord = chord_of(keys, SPLIT)?;
    post(ctx, &chord)?;
    let _ = timeline::set_playhead(ctx, was);
    timeline::look(ctx).map_err(|(_, why)| super::uncertain(why))?;
    let after = all(ctx).map_err(|(_, why)| super::uncertain(why))?;
    let (s, e) = (now["startBeats"].as_f64().unwrap_or(0.0), now["endBeats"].as_f64().unwrap_or(0.0));
    let left = json!({"startBeats": s, "endBeats": at, "name": now["name"], "track": now["track"], "looped": now["looped"]});
    let right = json!({"startBeats": at, "endBeats": e, "name": now["name"], "track": now["track"], "looped": now["looped"]});
    let (left_found, right_found) = (by_state(&after, &left, beats_per_bar), by_state(&after, &right, beats_per_bar));
    let (Some(left_found), Some(right_found)) = (left_found, right_found) else {
        return Err(super::uncertain(format!(
            "Logic Pro shows {} regions after Kumi asked to split {} at {}",
            after.len(),
            label(&found),
            timeline::bar_text(&position)
        )));
    };
    Ok(Applied {
        applied: json!({"left": state(&left_found, beats_per_bar), "right": state(&right_found, beats_per_bar)}),
        target: Some(target(ctx, left_found.place)),
        created: Some(json!({"ref": ref_text(ctx, "region", &[right_found.place]), "kind": "region"})),
        display: json!({"title": title}),
        step: Some("Split Regions"),
        ..Applied::default()
    })
}

pub fn split_restore(ctx: &mut Ctx, watch: &mut Watch, restore: &Restore) -> Done<Restored> {
    let beats_per_bar = beats_per_bar(ctx)?;
    let regions = all(ctx)?;
    if by_state(&regions, restore.prior, beats_per_bar).is_some() {
        return Ok(Restored::by_value(json!({"ref": restore.target["ref"]}), json!({"title": "The region is whole again"})));
    }
    let (left, right) = (&restore.applied["left"], &restore.applied["right"]);
    if !restore.discard && (by_state(&regions, left, beats_per_bar).is_none() || by_state(&regions, right, beats_per_bar).is_none()) {
        return Err(changed("one of the split's parts has been changed in Logic Pro since Kumi split it"));
    }
    let pressed = super::structure::undo_in_logic(ctx, watch, &restore.key, "Split Regions", None)?;
    timeline::look(ctx).map_err(|(_, why)| super::uncertain(why))?;
    let after = all(ctx).map_err(|(_, why)| super::uncertain(why))?;
    let Some(whole) = by_state(&after, restore.prior, beats_per_bar) else {
        return Err(super::uncertain(format!("Logic Pro's {pressed:?} was pressed, but the region isn't whole again")));
    };
    Ok(Restored {
        restored: json!({"ref": target(ctx, whole.place)["ref"], "was": restore.target["ref"], "pressed": pressed}),
        display: json!({"title": format!("{} whole again", label(&whole)), "note": "Put back with Logic Pro's own undo."}),
        undone_in_logic: true,
    })
}

// ----- the loop -----

fn loop_input(input: &Value) -> Done<bool> {
    input.get("loop").and_then(Value::as_bool).ok_or_else(|| bad("loop must be true or false"))
}

/// The Region inspector's row whose label is `label` (the "Loop:" row): its control.
fn inspector_control(ctx: &mut Ctx, label_text: &str, role: &str) -> Done<Control> {
    let inspector = nudge::read(ctx.project.inspector(ctx.ax))?;
    let strings = &ctx.project.strings;
    let row = inspector
        .find(&|n| {
            n.role == "AXRow" && n.children.iter().any(|c| c.role == "AXTextField" && strings.starts_with(&c.value_text(), label_text))
        })
        .ok_or_else(|| failed(format!("Logic Pro's Region inspector doesn't show its {label_text} row")))?;
    let control =
        row.child_where(|c| c.role == role).ok_or_else(|| failed(format!("the Region inspector's {label_text} row has no control")))?;
    Ok(Control::new(&ctx.project.root, &control.path))
}

/// Whether a region's Loop checkbox is on: the region selected (through its item, as the note reads select it), the
/// Region inspector shows it.
fn loop_shown(ctx: &mut Ctx, place: usize) -> Done<bool> {
    nudge::read(ctx.project.select_region(ctx.ax, place))?;
    let control = inspector_control(ctx, "Loop", "AXCheckBox")?;
    let attrs = nudge::read(ctx.ax.get(&control.root, &control.path, &["AXValue"]))?;
    Ok(nudge::is_on(attrs.get("AXValue").unwrap_or(&Value::Null)))
}

pub fn loop_preview(ctx: &mut Ctx, input: &Value) -> Done<Preview> {
    let found = find(ctx, input, "ref", Epoch::Current)?;
    let wanted = loop_input(input)?;
    let looped = loop_shown(ctx, found.place)?;
    let now = json!({"loop": looped});
    let mut display = json!({"title": format!("{} {}", label(&found), if wanted { "looped" } else { "not looped" }), "before": now, "after": {"loop": wanted}});
    if looped == wanted {
        display["unchanged"] = true.into();
    }
    Ok(Preview { target: target(ctx, found.place), prior: now.clone(), fence: now, display })
}

/// Presses the region's Loop checkbox when it doesn't read `wanted`; the checkbox as read after. Logic performs the
/// press and answers `attributeUnsupported`, which the readback settles.
fn put_loop(ctx: &mut Ctx, found: &Found, wanted: bool) -> Done<bool> {
    if loop_shown(ctx, found.place)? != wanted {
        let control = inspector_control(ctx, "Loop", "AXCheckBox")?;
        match ctx.ax.action(&control.root, &control.path, "AXPress", LIST_SETTLE_MS) {
            Ok(_) => {}
            Err(crate::ax::AxError::Helper(crate::helper::HelperError::Refused { code, .. })) if code == "attributeUnsupported" => {}
            Err(error) => return Err(super::ax_fault(&error, true)),
        }
        super::pause(LIST_SETTLE);
    }
    loop_shown(ctx, found.place).map_err(|(_, why)| super::uncertain(why))
}

pub fn loop_apply(ctx: &mut Ctx, input: &Value, fence: &Value) -> Done<Applied> {
    let found = find_for_apply(ctx, input)?;
    let wanted = loop_input(input)?;
    let looped = loop_shown(ctx, found.place)?;
    let now = json!({"loop": looped});
    if now != *fence {
        return Err(changed(format!("{}'s loop changed since the preview", label(&found))));
    }
    let after = if looped == wanted { wanted } else { put_loop(ctx, &found, wanted)? };
    if after != wanted {
        return Err(nudge::kept(&format!("{}'s loop", label(&found)), wanted, after));
    }
    Ok(Applied {
        applied: json!({"loop": after}),
        display: json!({"title": format!("{} {}", label(&found), if after { "looped" } else { "not looped" }), "before": now, "after": {"loop": after}}),
        step: Some("Set Loop"),
        ..Applied::default()
    })
}

pub fn loop_restore(ctx: &mut Ctx, restore: &Restore) -> Done<Restored> {
    let found = find_for_restore(ctx, restore.target)?;
    let (prior, applied) = (restore.prior["loop"] == true, restore.applied["loop"] == true);
    let looped = loop_shown(ctx, found.place)?;
    if looped != prior {
        if !restore.discard && looped != applied {
            return Err(changed(format!("{}'s loop has been changed in Logic Pro since Kumi set it", label(&found))));
        }
        let after = put_loop(ctx, &found, prior)?;
        if after != prior {
            return Err(nudge::kept(&format!("{}'s loop", label(&found)), prior, after));
        }
    }
    Ok(Restored::by_value(
        json!({"loop": prior}),
        json!({"title": format!("{} {} (put back)", label(&found), if prior { "looped" } else { "not looped" })}),
    ))
}

// ----- renaming -----

fn name_input(input: &Value) -> Done<String> {
    let name = input.get("name").and_then(Value::as_str).ok_or_else(|| bad("name must be text"))?;
    if name.trim().is_empty() {
        return Err(bad("name must not be empty"));
    }
    if name.chars().count() > 256 || name.chars().any(char::is_control) {
        return Err(bad("name must be one line of at most 256 characters"));
    }
    Ok(name.to_string())
}

pub fn rename_preview(ctx: &mut Ctx, input: &Value) -> Done<Preview> {
    let found = find(ctx, input, "ref", Epoch::Current)?;
    let name = name_input(input)?;
    let now = json!({"name": found.region.name});
    let mut display = json!({"title": format!("Renamed region {} → {}", quoted(&found.region.name), quoted(&name)), "before": now, "after": {"name": name}});
    if found.region.name == name {
        display["unchanged"] = true.into();
    }
    Ok(Preview { target: target(ctx, found.place), prior: now.clone(), fence: now, display })
}

/// Renames the region through the Region inspector's name field (the region selected alone first). The name the
/// Tracks area shows after, and the region's place then.
fn rename(ctx: &mut Ctx, found: &Found, name: &str) -> Done<(String, usize)> {
    // Selected through its item: the Inspector follows the selection, no keyboard focus needed.
    nudge::read(ctx.project.select_region(ctx.ax, found.place))?;
    let inspector = nudge::read(ctx.project.inspector(ctx.ax))?;
    let strings = &ctx.project.strings;
    let group = inspector
        .find(&|n| {
            n.role == "AXGroup" && n.children.iter().any(|c| c.role == "AXStaticText" && strings.starts_with(&c.value_text(), "Region"))
        })
        .ok_or_else(|| failed("Logic Pro's Inspector doesn't show the Region section"))?;
    let field = group.child_where(|c| c.role == "AXTextField").ok_or_else(|| failed("Logic Pro's Region inspector shows no name field"))?;
    let control = Control::new(&ctx.project.root, &field.path);
    nudge::wrote(nudge::confirm_text(ctx.ax, &control, name, LIST_SETTLE_MS))?;
    super::pause(LIST_SETTLE);
    timeline::look(ctx).map_err(|(_, why)| super::uncertain(why))?;
    let regions = all(ctx).map_err(|(_, why)| super::uncertain(why))?;
    let after = regions
        .iter()
        .find(|r| r.track == found.track && r.region.start == found.region.start && r.region.end == found.region.end)
        .ok_or_else(|| super::uncertain("the region isn't in the Tracks area where it was after the rename"))?;
    Ok((after.region.name.clone(), after.place))
}

pub fn rename_apply(ctx: &mut Ctx, input: &Value, fence: &Value) -> Done<Applied> {
    let found = find_for_apply(ctx, input)?;
    let now = json!({"name": found.region.name});
    if now != *fence {
        return Err(changed(format!("{}'s name changed since the preview", label(&found))));
    }
    let name = name_input(input)?;
    let (shown, place) = if found.region.name == name { (name.clone(), found.place) } else { rename(ctx, &found, &name)? };
    if shown != name {
        return Err(nudge::kept("the region's name", quoted(&name), quoted(&shown)));
    }
    Ok(Applied {
        applied: json!({"name": shown}),
        target: Some(target(ctx, place)),
        display: json!({"title": format!("Renamed region {} → {}", quoted(&found.region.name), quoted(&shown)), "before": now, "after": {"name": shown}}),
        step: Some("Renaming"),
        ..Applied::default()
    })
}

pub fn rename_restore(ctx: &mut Ctx, restore: &Restore) -> Done<Restored> {
    let found = find_for_restore(ctx, restore.target)?;
    let prior = restore.prior["name"].as_str().unwrap_or_default().to_string();
    if found.region.name != prior {
        if !restore.discard && json!({"name": found.region.name}) != *restore.applied {
            return Err(changed(format!("{}'s name has been changed in Logic Pro since Kumi set it", label(&found))));
        }
        let (shown, _) = rename(ctx, &found, &prior)?;
        if shown != prior {
            return Err(nudge::kept("the region's name", quoted(&prior), quoted(&shown)));
        }
    }
    Ok(Restored::by_value(json!({"name": prior}), json!({"title": format!("Region named {} again", quoted(&prior))})))
}

// ----- deleting -----

pub fn delete_preview(ctx: &mut Ctx, input: &Value) -> Done<Preview> {
    let found = find(ctx, input, "ref", Epoch::Current)?;
    let beats_per_bar = beats_per_bar(ctx)?;
    let now = state(&found, beats_per_bar);
    Ok(Preview {
        target: target(ctx, found.place),
        prior: now.clone(),
        fence: json!({"region": now, "regions": all(ctx)?.len()}),
        display: json!({"title": format!("Deleted {}", label(&found)), "note": "Only Logic Pro's own undo brings a deleted region back, while the deletion is its latest step."}),
    })
}

pub fn delete_apply(ctx: &mut Ctx, input: &Value, fence: &Value, desc: &str) -> Done<Applied> {
    let found = find_for_apply(ctx, input)?;
    let beats_per_bar = beats_per_bar(ctx)?;
    let now = state(&found, beats_per_bar);
    let before = all(ctx)?;
    if json!({"region": now, "regions": before.len()}) != *fence {
        return Err(changed(format!("{} or the regions changed since the preview", label(&found))));
    }
    let title = format!("Deleted {}", label(&found));
    select_only(ctx, found.place)?;
    let chord = Chord::parse("delete").ok_or_else(|| failed("no delete key"))?;
    post(ctx, &chord)?;
    timeline::look(ctx).map_err(|(_, why)| super::uncertain(why))?;
    let after = all(ctx).map_err(|(_, why)| super::uncertain(why))?;
    if after.len() != before.len() - 1 || by_state(&after, &now, beats_per_bar).is_some() {
        return Err(super::uncertain(format!("Logic Pro shows {} regions after Kumi asked to delete {}", after.len(), label(&found))));
    }
    let mut material = now.clone();
    material["kind"] = found.region.kind().into();
    material["step"] = desc.into();
    Ok(Applied {
        applied: json!({"deleted": true}),
        material: Some(material),
        display: json!({"title": title}),
        step: Some("Delete"),
        ..Applied::default()
    })
}

pub fn delete_restore(ctx: &mut Ctx, watch: &mut Watch, restore: &Restore) -> Done<Restored> {
    let material = restore.material.ok_or_else(|| bad("material: what the delete removed"))?;
    let beats_per_bar = beats_per_bar(ctx)?;
    let regions = all(ctx)?;
    if let Some(back) = by_state(&regions, material, beats_per_bar) {
        return Ok(Restored::by_value(
            json!({"ref": target(ctx, back.place)["ref"]}),
            json!({"title": format!("{} is back already", label(&back))}),
        ));
    }
    let pressed = super::structure::undo_in_logic(ctx, watch, &restore.key, "Delete", material["step"].as_str())?;
    timeline::look(ctx).map_err(|(_, why)| super::uncertain(why))?;
    let after = all(ctx).map_err(|(_, why)| super::uncertain(why))?;
    let Some(back) = by_state(&after, material, beats_per_bar) else {
        return Err(super::uncertain(format!("Logic Pro's {pressed:?} was pressed, but the region isn't back")));
    };
    Ok(Restored {
        restored: json!({"ref": target(ctx, back.place)["ref"], "was": restore.target["ref"], "pressed": pressed}),
        display: json!({"title": format!("{} put back", label(&back)), "note": "Put back with Logic Pro's own undo."}),
        undone_in_logic: true,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn found(place: usize, track: usize, name: &str, start: u32, end: u32) -> Found {
        Found {
            place,
            track,
            region: Region {
                track_index: track,
                name: name.into(),
                start: Position { bar: start, ..Position::START },
                end: Position { bar: end, ..Position::START },
                midi: true,
                selected: false,
                looped: false,
            },
        }
    }

    #[test]
    fn regions_are_placed_by_beats_and_found_again_by_their_state() {
        let keys = found(1, 2, "Keys", 1, 5);
        let state = state(&keys, 4);
        assert_eq!(state, json!({"startBeats": 0.0, "endBeats": 16.0, "name": "Keys", "track": 2, "looped": false}));
        assert_eq!(label(&keys), "region “Keys”");
        let moved = found(1, 2, "Keys", 3, 7);
        assert!(by_state(&[moved.clone()], &json!({"startBeats": 8.0, "endBeats": 24.0, "name": "Keys", "track": 2}), 4).is_some());
        assert!(by_state(&[moved], &state, 4).is_none());
        assert_eq!(position_input(&json!({"positionBeats": 8})).unwrap(), 8.0);
        assert!(position_input(&json!({"position": 2.0})).is_err());
        assert!(position_input(&json!({})).is_err());
        let now = json!({"startBeats": 0.0, "endBeats": 16.0});
        assert_eq!(edges_input(&json!({"endBeats": 8}), &now).unwrap(), (None, Some(8.0)));
        assert_eq!(edges_input(&json!({"startBeats": 4, "endBeats": 8}), &now).unwrap(), (Some(4.0), Some(8.0)));
        assert!(edges_input(&json!({"startBeats": 16}), &now).is_err(), "it must end after it starts");
        assert!(edges_input(&json!({"start": 1.0}), &now).is_err(), "seconds are REAPER's");
        assert!(edges_input(&json!({}), &now).is_err());
        assert_eq!(edges_title(&keys, 0.0, 8.0, 4), "Trimmed region “Keys” to bar 1–bar 3");
        let edges = edges_on_the_beat(Some(4.0), Some(8.0), 4).unwrap();
        assert_eq!(
            edges.iter().map(|(position, command)| (position.bar, position.beat, command.0)).collect::<Vec<_>>(),
            [(2, 1, 1500), (3, 1, 1501)]
        );
        assert!(edges_on_the_beat(Some(4.0), Some(8.5), 4).is_err(), "an end off the beat refuses before the start moves");
        assert!(edges_on_the_beat(None, Some(7.25), 4).is_err());
        assert!(loop_input(&json!({"loop": "yes"})).is_err());
        assert_eq!(name_input(&json!({"name": "Verse"})).unwrap(), "Verse");
        assert!(name_input(&json!({"name": ""})).is_err());
        assert!(close(0.0, 0.0005) && !close(0.0, 0.01));
    }
}
