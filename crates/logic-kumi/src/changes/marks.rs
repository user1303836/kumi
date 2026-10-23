//! Markers, through the Marker List: "Create new Marker" makes one at the playhead (so the playhead is moved there
//! first and put back after), a row's Delete action takes one away, and a row's position scrubber moves one, a
//! segment at a time. Logic names markers itself ("Marker 1", "Marker 2", in order), and a name cell isn't a text
//! field, so a marker can't be named from outside. Logic's history names the steps "Create Marker", "Delete" and
//! "Value Change".

use std::time::Duration;

use serde_json::{json, Value};

use super::nudge::{self, Control};
use super::{changed, gone, quoted, ref_text, timeline, Applied, Ctx, Done, Epoch, Preview, Restore, Restored, Watch};
use crate::ax::paths;
use crate::ax::Node;
use crate::project::ListOpen;
use crate::refs::{bad, failed};
use crate::rows::{self, Marker, Position};

/// How long the list gets after a press before its rows are read.
const LIST_SETTLE: Duration = Duration::from_millis(250);
const LIST_SETTLE_MS: u64 = 250;
/// The row action that deletes a list's row, as Logic names it.
pub const DELETE_ACTION: &str = "Name:Delete\nTarget:0x0\nSelector:(null)";

/// A marker row as read: the marker, and its row's node (its cells' controls).
pub struct Row {
    pub marker: Marker,
    pub node: Node,
}

/// The Marker List's rows as they stand, with the tab shown.
fn rows(ctx: &mut Ctx, beats_per_bar: u32) -> Done<Vec<Row>> {
    let (columns, table) = nudge::read(ctx.project.list_table(ctx.ax, "Marker"))?;
    Ok(paths::rows(&table)
        .into_iter()
        .filter_map(|row| Marker::of(row, &columns, beats_per_bar).map(|marker| (marker, row.clone())))
        .map(|(marker, node)| Row { marker, node })
        .collect())
}

/// The list shown for a run of reads and writes: the tab open, and the beats to a bar it is read with.
struct Shown {
    open: ListOpen,
    beats_per_bar: u32,
}

fn show(ctx: &mut Ctx) -> Done<Shown> {
    let (_, transport) = timeline::control_bar(ctx)?;
    let open = nudge::read(ctx.project.show_list(ctx.ax, "Marker"))?;
    Ok(Shown { open, beats_per_bar: transport.beats_per_bar() })
}

fn hide(ctx: &mut Ctx, shown: Shown) {
    let _ = ctx.project.hide_list(ctx.ax, shown.open);
    ctx.project.forget();
}

fn marker_place(ctx: &Ctx, input: &Value, key: &str, epoch: Epoch) -> Done<usize> {
    Ok(super::parse_ref(ctx, input, key, "marker", epoch, 1)?[0])
}

fn state(marker: &Marker) -> Value {
    json!({"position": marker.position.to_json(), "name": marker.name, "length": marker.length_beats, "text": marker.text})
}

fn label(marker: &Marker) -> String {
    if marker.name.is_empty() {
        "marker".into()
    } else {
        format!("marker {}", quoted(&rows::clip(&marker.name)))
    }
}

fn target(ctx: &Ctx, place: usize) -> Value {
    json!({"ref": ref_text(ctx, "marker", &[place]), "kind": "marker"})
}

fn position_input(input: &Value, beats_per_bar: u32) -> Done<Position> {
    if input.get("position").is_some_and(|v| !v.is_null()) && input.get("positionBeats").is_none_or(Value::is_null) {
        return Err(bad("Logic Pro's bridge places markers by positionBeats (beats from the project's start), not seconds"));
    }
    let beats = input
        .get("positionBeats")
        .and_then(Value::as_f64)
        .filter(|n| n.is_finite() && (0.0..=1.0e6).contains(n))
        .ok_or_else(|| bad("positionBeats must be a number from 0: beats from the project's start"))?;
    Ok(Position::from_beats(beats, beats_per_bar))
}

/// The row at a position, if one is.
fn row_at(list: &[Row], position: &Position) -> Option<usize> {
    list.iter().position(|row| row.marker.position == *position)
}

/// The list's "Create new Marker" button.
fn create_button(ctx: &mut Ctx) -> Done<Control> {
    let group = nudge::read(ctx.project.list_group(ctx.ax))?;
    let strings = &ctx.project.strings;
    let button = group
        .find(&|n| n.role == "AXButton" && strings.is(&n.desc, "Create new Marker"))
        .ok_or_else(|| failed("Logic Pro's Marker List doesn't show its Create new Marker button"))?;
    Ok(Control::new(&ctx.project.root, &button.path))
}

/// Makes a marker at a position: the playhead there, the list's button, the playhead back. The new marker's row.
fn create_at(ctx: &mut Ctx, shown: &Shown, position: Position) -> Done<(Marker, usize)> {
    let (_, before) = timeline::control_bar(ctx)?;
    let was = before.position();
    let rows_before = rows(ctx, shown.beats_per_bar)?;
    timeline::set_playhead(ctx, position)?;
    let button = create_button(ctx)?;
    nudge::wrote(nudge::press(ctx.ax, &button, LIST_SETTLE_MS))?;
    super::pause(LIST_SETTLE);
    let after = rows(ctx, shown.beats_per_bar).map_err(|(_, why)| super::uncertain(why))?;
    let _ = timeline::set_playhead(ctx, was);
    // The new row: the one the list didn't have. Made at the playhead, it carries the playhead's ticks within the
    // beat (the LCD shows none), so it is moved onto the beat asked for when it isn't there.
    let mut left: Vec<Position> = rows_before.iter().map(|row| row.marker.position).collect();
    let made = after.iter().position(|row| match left.iter().position(|p| *p == row.marker.position) {
        Some(at) => {
            left.swap_remove(at);
            false
        }
        None => true,
    });
    let Some(index) = made.filter(|_| after.len() == rows_before.len() + 1) else {
        return Err(super::uncertain(format!("Logic Pro's Marker List shows {} markers after Kumi asked for one more", after.len())));
    };
    let at = Position { bar: position.bar, beat: position.beat, ..Position::START };
    let made = after[index].marker.clone();
    let marker = if made.position == at { made } else { move_marker(ctx, shown, &made.position, at)? };
    let list = rows(ctx, shown.beats_per_bar).map_err(|(_, why)| super::uncertain(why))?;
    let place =
        row_at(&list, &at).map(|i| i + 1).ok_or_else(|| super::uncertain("the marker made isn't in the Marker List where it was put"))?;
    Ok((marker, place))
}

/// Deletes a row through its Delete action and reads the list back: the rows after.
fn delete_row(ctx: &mut Ctx, shown: &Shown, row: &Row) -> Done<Vec<Row>> {
    nudge::wrote(ctx.ax.action(&ctx.project.root, &row.node.path, DELETE_ACTION, LIST_SETTLE_MS))?;
    super::pause(LIST_SETTLE);
    rows(ctx, shown.beats_per_bar).map_err(|(_, why)| super::uncertain(why))
}

/// The segment sliders of a row's scrubber cell (Position or Length): bar, beat, division, tick. The table is read
/// to the rows' cells; the scrubber's own children are one level deeper, so they are read on their own.
pub(crate) fn segments(ctx: &mut Ctx, row: &Node, columns: &[(String, usize)], column: &str, what: &str) -> Done<Vec<Control>> {
    let index = columns
        .iter()
        .find(|(name, _)| name == column)
        .map(|(_, i)| *i)
        .ok_or_else(|| failed(format!("the list has no {column} column")))?;
    let cell = row
        .children
        .iter()
        .filter(|c| c.role == "AXCell")
        .nth(index)
        .ok_or_else(|| failed(format!("{what}'s row has no {column} cell")))?;
    let group = cell.children.first().ok_or_else(|| failed(format!("{what}'s {column} cell is empty")))?;
    let group = if group.children.iter().any(|c| c.role == "AXSlider") {
        group.clone()
    } else {
        nudge::read(ctx.ax.tree(&ctx.project.root, &group.path, 1, 8, Some(crate::ax::BRIEF)))?
    };
    let sliders: Vec<Control> =
        group.children.iter().filter(|c| c.role == "AXSlider").map(|c| Control::new(&ctx.project.root, &c.path)).collect();
    if sliders.len() < 2 {
        return Err(failed(format!("{what}'s {column} has no segments to set")));
    }
    Ok(sliders)
}

/// Moves a row's position scrubber from `from` to `to`, one step at a time (the list keeps its rows in order of
/// position, so the row is found again after every step, through `locate`, which gives the row at a position now):
/// the position reached.
pub(crate) fn step_position(
    ctx: &mut Ctx,
    columns: &[(String, usize)],
    from: Position,
    to: Position,
    what: &str,
    mut locate: impl FnMut(&mut Ctx, &Position) -> Done<Node>,
) -> Done<Position> {
    let mut at = from;
    let fields: [(usize, fn(&Position) -> u32, fn(&mut Position, u32)); 4] = [
        (0, |p| p.bar, |p, v| p.bar = v),
        (1, |p| p.beat, |p, v| p.beat = v),
        (2, |p| p.division, |p, v| p.division = v),
        (3, |p| p.tick, |p, v| p.tick = v),
    ];
    for (segment, get, set) in fields {
        let target = get(&to);
        for _ in 0..nudge::MOST_STEPS {
            let now = get(&at);
            if now == target {
                break;
            }
            let row = locate(ctx, &at)?;
            let controls = segments(ctx, &row, columns, "Position", what)?;
            let Some(control) = controls.get(segment) else { break };
            let away = i64::from(target) - i64::from(now);
            let big = away.abs() >= nudge::BIG_STEP;
            // The reply's `after` is gone once the row re-sorted (its element went stale): the step asked for is
            // taken as made, and the row found at that position next, or not at all.
            let moved = nudge::wrote(nudge::step(ctx.ax, control, i64::from(now), i64::from(target), big))?;
            let expected = i64::from(now) + away.signum() * if big { nudge::BIG_STEP } else { 1 };
            let reached = moved.filter(|moved| *moved != i64::from(now)).unwrap_or(expected);
            set(&mut at, reached.max(0) as u32);
        }
    }
    Ok(at)
}

/// Moves a marker, one step at a time, finding its row again after each. The marker as read after.
fn move_marker(ctx: &mut Ctx, shown: &Shown, from: &Position, to: Position) -> Done<Marker> {
    let (columns, _) = nudge::read(ctx.project.list_table(ctx.ax, "Marker"))?;
    let beats_per_bar = shown.beats_per_bar;
    let reached = step_position(ctx, &columns, *from, to, "the marker", |ctx, at| {
        let list = rows(ctx, beats_per_bar).map_err(|(_, why)| super::uncertain(why))?;
        let index = row_at(&list, at)
            .ok_or_else(|| super::uncertain(format!("the marker isn't in the Marker List at {} any more", timeline::bar_text(at))))?;
        Ok(list[index].node.clone())
    })?;
    super::pause(LIST_SETTLE);
    let list = rows(ctx, shown.beats_per_bar).map_err(|(_, why)| super::uncertain(why))?;
    let index = row_at(&list, &reached).ok_or_else(|| {
        super::uncertain(format!("Logic Pro's Marker List shows no marker at {} after the move", timeline::bar_text(&reached)))
    })?;
    Ok(list[index].marker.clone())
}

// ----- adding -----

pub fn add_preview(ctx: &mut Ctx, input: &Value) -> Done<Preview> {
    if input.get("name").is_some_and(|v| !v.is_null()) {
        return Err(bad(
            "Logic Pro names markers itself (Marker 1, Marker 2, in order): a marker can't be named from outside, so leave name out",
        ));
    }
    let (_, transport) = timeline::control_bar(ctx)?;
    let position = position_input(input, transport.beats_per_bar())?;
    let markers = nudge::read(ctx.project.read_markers(ctx.ax, transport.beats_per_bar()))?;
    let mut display = json!({"title": format!("Marker at {}", timeline::bar_text(&position))});
    if position.division > 1 || position.tick > 1 {
        display["note"] = "Logic Pro's playhead display places a marker on a beat: the division and tick are dropped.".into();
    }
    Ok(Preview {
        target: json!({"ref": null, "kind": "marker"}),
        prior: json!({"markers": markers.len()}),
        fence: json!({"markers": markers.len()}),
        display,
    })
}

pub fn add_apply(ctx: &mut Ctx, input: &Value, fence: &Value) -> Done<Applied> {
    let shown = show(ctx)?;
    let result = (|| {
        let position = position_input(input, shown.beats_per_bar)?;
        let before = rows(ctx, shown.beats_per_bar)?;
        if json!({"markers": before.len()}) != *fence {
            return Err(changed("the markers changed since the preview"));
        }
        let (marker, place) = create_at(ctx, &shown, position)?;
        Ok(Applied {
            applied: state(&marker),
            created: Some(target(ctx, place)),
            target: None,
            material: None,
            display: json!({"title": format!("Marker {} at {}", quoted(&marker.name), timeline::bar_text(&marker.position))}),
            step: Some("Create Marker"),
            probe: None,
        })
    })();
    hide(ctx, shown);
    result
}

pub fn add_restore(ctx: &mut Ctx, restore: &Restore) -> Done<Restored> {
    let created = restore.created.ok_or_else(|| bad("created: the marker that was added"))?;
    let place = marker_place(ctx, created, "ref", Epoch::Any)?;
    let shown = show(ctx)?;
    let result = (|| {
        let list = rows(ctx, shown.beats_per_bar)?;
        let Some(row) = list.get(place - 1) else { return Err(gone("the marker Kumi added isn't in the Marker List any more")) };
        let now = state(&row.marker);
        if !restore.discard && now["position"] != restore.applied["position"] {
            return Err(changed(format!("{} has been moved in Logic Pro since Kumi added it", label(&row.marker))));
        }
        let name = label(&row.marker);
        let after = delete_row(ctx, &shown, row)?;
        if after.len() != list.len() - 1 {
            return Err(super::uncertain(format!("Logic Pro's Marker List shows {} markers after Kumi asked to delete one", after.len())));
        }
        Ok(Restored::by_value(json!({"deleted": created["ref"]}), json!({"title": format!("{name} taken away again")})))
    })();
    hide(ctx, shown);
    result
}

// ----- moving -----

pub fn move_preview(ctx: &mut Ctx, input: &Value) -> Done<Preview> {
    let place = marker_place(ctx, input, "ref", Epoch::Current)?;
    let (_, transport) = timeline::control_bar(ctx)?;
    let to = position_input(input, transport.beats_per_bar())?;
    let markers = nudge::read(ctx.project.read_markers(ctx.ax, transport.beats_per_bar()))?;
    let marker = markers.get(place - 1).ok_or_else(|| crate::refs::not_in_project(input["ref"].as_str().unwrap_or_default()))?;
    let now = json!({"position": marker.position.to_json()});
    let mut display = json!({"title": format!("Moved {} to {}", label(marker), timeline::bar_text(&to)), "before": now, "after": {"position": to.to_json()}});
    if marker.position == to {
        display["unchanged"] = true.into();
    }
    Ok(Preview { target: target(ctx, place), prior: now.clone(), fence: now, display })
}

pub fn move_apply(ctx: &mut Ctx, input: &Value, fence: &Value) -> Done<Applied> {
    let place = marker_place(ctx, input, "ref", Epoch::Current)?;
    let shown = show(ctx)?;
    let result = (|| {
        let to = position_input(input, shown.beats_per_bar)?;
        let list = rows(ctx, shown.beats_per_bar)?;
        let row = list.get(place - 1).ok_or_else(|| changed("the marker went since the preview"))?;
        let now = json!({"position": row.marker.position.to_json()});
        if now != *fence {
            return Err(changed(format!("{} moved since the preview", label(&row.marker))));
        }
        let from = row.marker.position;
        let name = label(&row.marker);
        let after = if from == to { row.marker.clone() } else { move_marker(ctx, &shown, &from, to)? };
        if after.position != to {
            return Err(nudge::kept(&format!("{name}'s position"), timeline::bar_text(&to), timeline::bar_text(&after.position)));
        }
        Ok(Applied {
            applied: json!({"position": after.position.to_json()}),
            created: None,
            target: None,
            material: None,
            display: json!({"title": format!("Moved {name} to {}", timeline::bar_text(&to)), "before": now, "after": {"position": to.to_json()}}),
            step: Some("Value Change"),
            probe: None,
        })
    })();
    hide(ctx, shown);
    result
}

fn position_of(value: &Value) -> Option<Position> {
    let p = value.get("position")?;
    let at = |key: &str| p.get(key).and_then(Value::as_u64).map(|n| n as u32);
    Some(Position { bar: at("bar")?, beat: at("beat")?, division: at("division").unwrap_or(1), tick: at("tick").unwrap_or(1) })
}

pub fn move_restore(ctx: &mut Ctx, restore: &Restore) -> Done<Restored> {
    let prior = position_of(restore.prior).ok_or_else(|| bad("prior.position"))?;
    let applied = position_of(restore.applied).ok_or_else(|| bad("applied.position"))?;
    let shown = show(ctx)?;
    let result = (|| {
        let list = rows(ctx, shown.beats_per_bar)?;
        if row_at(&list, &prior).is_some() && row_at(&list, &applied).is_none() {
            return Ok(Restored {
                restored: json!({"position": prior.to_json()}),
                display: json!({"title": "The marker is where it was"}),
                undone_in_logic: false,
            });
        }
        let Some(index) = row_at(&list, &applied) else {
            if restore.discard {
                return Err(gone("the marker Kumi moved isn't where it left it, nor where it was"));
            }
            return Err(changed("the marker has been moved in Logic Pro since Kumi moved it"));
        };
        let name = label(&list[index].marker);
        let after = move_marker(ctx, &shown, &applied, prior)?;
        if after.position != prior {
            return Err(nudge::kept(&format!("{name}'s position"), timeline::bar_text(&prior), timeline::bar_text(&after.position)));
        }
        Ok(Restored {
            restored: json!({"position": prior.to_json()}),
            display: json!({"title": format!("{name} back at {}", timeline::bar_text(&prior))}),
            undone_in_logic: false,
        })
    })();
    hide(ctx, shown);
    result
}

// ----- deleting -----

pub fn delete_preview(ctx: &mut Ctx, input: &Value) -> Done<Preview> {
    let place = marker_place(ctx, input, "ref", Epoch::Current)?;
    let (_, transport) = timeline::control_bar(ctx)?;
    let markers = nudge::read(ctx.project.read_markers(ctx.ax, transport.beats_per_bar()))?;
    let marker = markers.get(place - 1).ok_or_else(|| crate::refs::not_in_project(input["ref"].as_str().unwrap_or_default()))?;
    let now = state(marker);
    Ok(Preview {
        target: target(ctx, place),
        prior: now.clone(),
        fence: now,
        display: json!({"title": format!("Deleted {}", label(marker))}),
    })
}

pub fn delete_apply(ctx: &mut Ctx, input: &Value, fence: &Value) -> Done<Applied> {
    let place = marker_place(ctx, input, "ref", Epoch::Current)?;
    let shown = show(ctx)?;
    let result = (|| {
        let list = rows(ctx, shown.beats_per_bar)?;
        let row = list.get(place - 1).ok_or_else(|| changed("the marker went since the preview"))?;
        if state(&row.marker) != *fence {
            return Err(changed(format!("{} changed since the preview", label(&row.marker))));
        }
        let name = label(&row.marker);
        let material = state(&row.marker);
        let after = delete_row(ctx, &shown, row)?;
        if after.len() != list.len() - 1 {
            return Err(super::uncertain(format!(
                "Logic Pro's Marker List shows {} markers after Kumi asked to delete {name}",
                after.len()
            )));
        }
        Ok(Applied {
            applied: json!({"deleted": true}),
            created: None,
            target: None,
            material: Some(material),
            display: json!({"title": format!("Deleted {name}")}),
            step: Some("Delete"),
            probe: None,
        })
    })();
    hide(ctx, shown);
    result
}

/// A length in beats as the Length scrubber's segments: bars, beats, divisions, ticks.
fn length_segments(beats: f64, beats_per_bar: u32) -> [u32; 4] {
    let ticks = (beats.max(0.0) * rows::TICKS_PER_BEAT as f64).round() as u64;
    let bar = u64::from(beats_per_bar.max(1)) * u64::from(rows::TICKS_PER_BEAT);
    [
        (ticks / bar) as u32,
        ((ticks % bar) / u64::from(rows::TICKS_PER_BEAT)) as u32,
        ((ticks % u64::from(rows::TICKS_PER_BEAT)) / u64::from(rows::TICKS_PER_DIVISION)) as u32,
        (ticks % u64::from(rows::TICKS_PER_DIVISION)) as u32,
    ]
}

pub fn delete_restore(ctx: &mut Ctx, watch: &mut Watch, restore: &Restore) -> Done<Restored> {
    let material = restore.material.ok_or_else(|| bad("material: what the delete removed"))?;
    let position = position_of(material).ok_or_else(|| bad("material.position"))?;
    let (name, length) = (material["name"].as_str().unwrap_or_default().to_string(), material["length"].as_f64().unwrap_or(0.0));
    // Logic's own undo brings it back whole, name and all, while the Edit menu still names Kumi's deletion first.
    if let Ok(pressed) = super::structure::undo_in_logic(ctx, watch, &restore.key, "Delete", None) {
        let shown = show(ctx)?;
        let list = rows(ctx, shown.beats_per_bar);
        hide(ctx, shown);
        let list = list.map_err(|(_, why)| super::uncertain(why))?;
        return match row_at(&list, &position) {
            Some(index) => Ok(Restored {
                restored: json!({"ref": target(ctx, index + 1)["ref"], "was": restore.target["ref"], "pressed": pressed}),
                display: json!({"title": format!("Marker {} put back", quoted(&name)), "note": "Put back with Logic Pro's own undo."}),
                undone_in_logic: true,
            }),
            None => Err(super::uncertain(format!(
                "Logic Pro's {pressed:?} was pressed, but no marker is back at {}",
                timeline::bar_text(&position)
            ))),
        };
    }
    // Else by value: a marker made again at its place, with its length; Logic numbers it itself.
    let shown = show(ctx)?;
    let result = (|| {
        let list = rows(ctx, shown.beats_per_bar)?;
        if let Some(index) = row_at(&list, &position) {
            if restore.discard || list[index].marker.name == name {
                return Ok(Restored {
                    restored: json!({"ref": target(ctx, index + 1)["ref"]}),
                    display: json!({"title": format!("Marker {} is back already", quoted(&name))}),
                    undone_in_logic: false,
                });
            }
            return Err(changed(format!(
                "another marker ({}) is at {} now",
                quoted(&list[index].marker.name),
                timeline::bar_text(&position)
            )));
        }
        let (made, place) = create_at(ctx, &shown, position)?;
        let mut note = String::from("Logic Pro numbers a marker made again itself.");
        if length > 0.0 && length < 1.0e6 {
            let (columns, _) = nudge::read(ctx.project.list_table(ctx.ax, "Marker"))?;
            let list = rows(ctx, shown.beats_per_bar)?;
            if let Some(row) = list.get(place - 1) {
                match segments(ctx, &row.node, &columns, "Length", "the marker") {
                    Ok(sliders) => {
                        for (slider, target) in sliders.iter().zip(length_segments(length, shown.beats_per_bar)) {
                            let _ = nudge::step_to(ctx.ax, slider, 0, i64::from(target));
                        }
                    }
                    Err(_) => note.push_str(" Its length wasn't put back."),
                }
            }
        }
        Ok(Restored {
            restored: json!({"ref": target(ctx, place)["ref"], "name": made.name}),
            display: json!({"title": format!("Marker {} put back as {}", quoted(&name), quoted(&made.name)), "note": note}),
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
    fn markers_are_placed_by_beats_and_lengths_split_into_segments() {
        assert_eq!(position_input(&json!({"positionBeats": 32}), 4).unwrap(), Position { bar: 9, ..Position::START });
        assert!(position_input(&json!({"position": 12.0}), 4).is_err());
        assert_eq!(length_segments(32.0, 4), [8, 0, 0, 0]);
        assert_eq!(length_segments(1.25, 4), [0, 1, 1, 0]);
        assert_eq!(length_segments(0.5 + 1.0 / 960.0, 4), [0, 0, 2, 1]);
        let marker =
            Marker { position: Position { bar: 9, ..Position::START }, name: "Marker 1".into(), length_beats: 32.0, text: String::new() };
        assert_eq!(label(&marker), "marker “Marker 1”");
        assert_eq!(position_of(&state(&marker)), Some(marker.position));
        assert_eq!(state(&marker)["length"], 32.0);
    }
}
