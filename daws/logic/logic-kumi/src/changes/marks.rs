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
pub const UNKNOWN_MARKER_RESTORE: &str = "Kumi can't verify the marker's complete original contents because Logic Pro didn't expose every required field. Use Logic Pro's own undo manually; showing a field now can't recover its earlier value.";

/// A marker row as read: the marker, and its row's node (its cells' controls).
#[derive(Clone)]
pub struct Row {
    pub marker: Marker,
    pub node: Node,
    /// Full, unclipped observable material. Missing Text is null, not an invented empty string.
    pub identity: Value,
}

fn cell<'a>(row: &'a Node, columns: &[(String, usize)], id: &str) -> Done<&'a Node> {
    let mut found = columns.iter().filter(|(name, _)| name == id);
    let (_, index) = found.next().ok_or_else(|| failed(format!("the Marker List has no readable {id} column")))?;
    if found.next().is_some() {
        return Err(failed(format!("the Marker List has no unique {id} column")));
    }
    row.children
        .iter()
        .filter(|child| child.role == "AXCell")
        .nth(*index)
        .ok_or_else(|| failed(format!("the marker has no readable {id} cell")))
}

fn full_text(cell: &Node) -> Done<String> {
    if cell.more != 0 {
        return Err(failed("the marker text cell is incomplete"));
    }
    let field =
        cell.find(&|node| node.role == "AXTextField" || node.role == "AXStaticText").or_else(|| cell.children.first()).unwrap_or(cell);
    if field.more != 0 {
        return Err(failed("the marker text is incomplete"));
    }
    if let Some(value) = field.value.as_str() {
        return Ok(value.to_string());
    }
    let value = field.value_text();
    if !value.is_empty() {
        return Ok(value);
    }
    if !field.desc.is_empty() {
        return Ok(field.desc.clone());
    }
    Err(failed("the marker text is not readable"))
}

fn other_cell(node: &Node) -> Done<Value> {
    if node.more != 0 || node.role.is_empty() {
        return Err(failed("the marker's additional column is incomplete"));
    }
    let children = node.children.iter().map(other_cell).collect::<Done<Vec<_>>>()?;
    let mut value = json!({"role": node.role, "children": children});
    for (key, text) in [
        ("subrole", &node.subrole),
        ("id", &node.id),
        ("desc", &node.desc),
        ("title", &node.title),
        ("help", &node.help),
        ("valueDesc", &node.value_desc),
    ] {
        if !text.is_empty() {
            value[key] = text.clone().into();
        }
    }
    if !node.value.is_null() {
        value["value"] = node.value.clone();
    }
    Ok(value)
}

fn rows_of(table: &Node, columns: &[(String, usize)], beats_per_bar: u32) -> Done<Vec<Row>> {
    if table.more != 0 {
        return Err(failed("the complete Marker List is not readable"));
    }
    if columns.iter().any(|(name, _)| name.is_empty() || columns.iter().filter(|(other, _)| other == name).count() != 1) {
        return Err(failed("the Marker List columns are incomplete or ambiguous"));
    }
    paths::rows(table)
        .into_iter()
        .map(|row| {
            if row.more != 0 {
                return Err(failed("the marker row is incomplete"));
            }
            let marker = Marker::of(row, columns, beats_per_bar).ok_or_else(|| failed("a Marker List row has no readable position"))?;
            let position = cell(row, columns, "Position")?
                .children
                .first()
                .and_then(rows::position_of)
                .ok_or_else(|| failed("the marker has no complete readable position"))?;
            let name = full_text(cell(row, columns, "Name")?)?;
            if name.trim().is_empty() {
                return Err(failed("the marker has no readable name"));
            }
            let length_group = cell(row, columns, "Length")?.children.first().ok_or_else(|| failed("the marker has no readable length"))?;
            let length = match rows::length_beats_of(length_group, beats_per_bar).filter(|length| length.is_finite() && *length >= 0.0) {
                Some(length) => json!(length),
                None if length_group.desc.trim() == "∞" => json!("infinite"),
                None => return Err(failed("the marker has no readable length")),
            };
            let text = if columns.iter().any(|(name, _)| name == "Text") { Some(full_text(cell(row, columns, "Text")?)?) } else { None };
            let mut ax = json!({"role": row.role});
            for (key, value) in [("subrole", &row.subrole), ("id", &row.id)] {
                if !value.is_empty() {
                    ax[key] = value.clone().into();
                }
            }
            let mut other = json!({});
            for (name, _) in columns.iter().filter(|(name, _)| !matches!(name.as_str(), "Position" | "Name" | "Length" | "Text")) {
                other[name.as_str()] = other_cell(cell(row, columns, name)?)?;
            }
            let identity = json!({"position": position.to_json(), "name": name, "length": length,
                "text": text, "columns": columns.iter().map(|(name, _)| name).collect::<Vec<_>>(), "ax": ax, "other": other});
            Ok(Row { marker, node: row.clone(), identity })
        })
        .collect()
}

/// No unreadable row is silently omitted: doing so could give a later marker the old marker's positional ref.
fn rows(ctx: &mut Ctx, beats_per_bar: u32) -> Done<Vec<Row>> {
    let (columns, table) = nudge::read(ctx.project.list_table(ctx.ax, "Marker"))?;
    rows_of(&table, &columns, beats_per_bar)
}

fn list_identity(list: &[Row]) -> Value {
    Value::Array(list.iter().map(|row| row.identity.clone()).collect())
}

fn unique_identity(list: &[Row], identity: &Value) -> Option<usize> {
    let mut found = list.iter().enumerate().filter(|(_, row)| row.identity == *identity);
    let (index, _) = found.next()?;
    found.next().is_none().then_some(index)
}

fn complete_identity(identity: &Value) -> bool {
    let position = &identity["position"];
    ["bar", "beat", "division", "tick"].iter().all(|key| position[*key].as_u64().is_some_and(|number| number > 0))
        && identity["name"].as_str().is_some_and(|name| !name.trim().is_empty())
        && (identity["length"].as_f64().is_some_and(|length| length.is_finite() && length >= 0.0) || identity["length"] == "infinite")
        && identity["text"].is_string()
        && identity["columns"].as_array().is_some_and(|columns| {
            ["Position", "Name", "Length", "Text"]
                .iter()
                .all(|name| columns.iter().filter(|column| column.as_str() == Some(*name)).count() == 1)
        })
        && identity["ax"]["role"] == "AXRow"
        && identity["other"].is_object()
}

fn complete_material(material: &Value) -> bool {
    complete_identity(&material["marker"])
        && ["prior", "applied"].iter().all(|key| material[*key].as_array().is_some_and(|rows| rows.iter().all(complete_identity)))
        && material.get("priorMarker").is_none_or(complete_identity)
}

fn undo_hint(material: &Value) -> Value {
    if complete_material(material) {
        json!({"mode": "value"})
    } else {
        json!({"mode": "manual", "reason": UNKNOWN_MARKER_RESTORE, "guidance": "Use Logic Pro's own undo manually."})
    }
}

fn restore_identity<'a>(restore: &'a Restore<'_>) -> Done<&'a Value> {
    let material = restore
        .material
        .filter(|material| material["marker"].is_object() && material["prior"].is_array() && material["applied"].is_array())
        .ok_or_else(|| failed(UNKNOWN_MARKER_RESTORE))?;
    if !complete_material(material) {
        return Err(failed(UNKNOWN_MARKER_RESTORE));
    }
    Ok(material)
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
    let mut found = list.iter().enumerate().filter(|(_, row)| row.marker.position == *position);
    let (index, _) = found.next()?;
    found.next().is_none().then_some(index)
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
fn create_at(ctx: &mut Ctx, shown: &Shown, position: Position) -> Done<(Marker, usize, Vec<Row>)> {
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
    if made.position != at {
        move_marker(ctx, shown, &made.position, at, &after[index].identity, &list_identity(&after))
            .map_err(|(_, why)| super::uncertain(why))?;
    }
    let list = rows(ctx, shown.beats_per_bar).map_err(|(_, why)| super::uncertain(why))?;
    let place =
        row_at(&list, &at).map(|i| i + 1).ok_or_else(|| super::uncertain("the marker made isn't in the Marker List where it was put"))?;
    Ok((list[place - 1].marker.clone(), place, list))
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
fn move_marker(
    ctx: &mut Ctx,
    shown: &Shown,
    from: &Position,
    to: Position,
    expected: &Value,
    expected_list: &Value,
) -> Done<(Marker, Vec<Row>)> {
    let (columns, table) = nudge::read(ctx.project.list_table(ctx.ax, "Marker"))?;
    let beats_per_bar = shown.beats_per_bar;
    let before = rows_of(&table, &columns, beats_per_bar)?;
    if list_identity(&before) != *expected_list || unique_identity(&before, expected).is_none() {
        return Err(changed("the marker or its list changed before it could be moved; nothing changed"));
    }
    let others: Vec<_> = before.iter().filter(|row| row.identity != *expected).map(|row| row.identity.clone()).collect();
    let locate = |list: &[Row], at: &Position| -> Done<usize> {
        let mut target = expected.clone();
        target["position"] = at.to_json();
        let index = unique_identity(list, &target).ok_or_else(|| changed("the marker's full identity changed while it was being moved"))?;
        let now_others: Vec<_> = list.iter().enumerate().filter(|(at, _)| *at != index).map(|(_, row)| row.identity.clone()).collect();
        if now_others != others {
            return Err(changed("the Marker List changed while its marker was being moved"));
        }
        Ok(index)
    };
    let reached = step_position(ctx, &columns, *from, to, "the marker", |ctx, at| {
        let list = rows(ctx, beats_per_bar).map_err(|(_, why)| super::uncertain(why))?;
        let index = locate(&list, at)?;
        Ok(list[index].node.clone())
    })
    .map_err(|(_, why)| super::uncertain(why))?;
    super::pause(LIST_SETTLE);
    let list = rows(ctx, shown.beats_per_bar).map_err(|(_, why)| super::uncertain(why))?;
    let index = locate(&list, &reached).map_err(|(_, why)| super::uncertain(why))?;
    Ok((list[index].marker.clone(), list))
}

// ----- adding -----

pub fn add_preview(ctx: &mut Ctx, input: &Value) -> Done<Preview> {
    if input.get("name").is_some_and(|v| !v.is_null()) {
        return Err(bad(
            "Logic Pro names markers itself (Marker 1, Marker 2, in order): a marker can't be named from outside, so leave name out",
        ));
    }
    let shown = show(ctx)?;
    let result = (|| {
        let position = position_input(input, shown.beats_per_bar)?;
        let markers = rows(ctx, shown.beats_per_bar)?;
        let mut display = json!({"title": format!("Marker at {}", timeline::bar_text(&position))});
        if position.division > 1 || position.tick > 1 {
            display["note"] = "Logic Pro's playhead display places a marker on a beat: the division and tick are dropped.".into();
        }
        Ok(Preview {
            target: json!({"ref": null, "kind": "marker"}),
            prior: json!({"markers": markers.len()}),
            fence: json!({"markers": markers.len(), "identity": list_identity(&markers)}),
            display,
        })
    })();
    hide(ctx, shown);
    result
}

pub fn add_apply(ctx: &mut Ctx, input: &Value, fence: &Value) -> Done<Applied> {
    let shown = show(ctx)?;
    let result = (|| {
        let position = position_input(input, shown.beats_per_bar)?;
        let before = rows(ctx, shown.beats_per_bar)?;
        if json!({"markers": before.len(), "identity": list_identity(&before)}) != *fence {
            return Err(changed("the markers changed since the preview"));
        }
        let (marker, place, after) = create_at(ctx, &shown, position)?;
        let mut material = json!({"marker": after[place - 1].identity, "prior": list_identity(&before), "applied": list_identity(&after)});
        material["undo"] = undo_hint(&material);
        let mut display = json!({"title": format!("Marker {} at {}", quoted(&marker.name), timeline::bar_text(&marker.position))});
        if material["undo"]["mode"] == "manual" {
            display["note"] = UNKNOWN_MARKER_RESTORE.into();
        }
        Ok(Applied {
            applied: state(&marker),
            target: None,
            created: Some(target(ctx, place)),
            material: Some(material),
            display,
            step: Some("Create Marker"),
            probe: None,
        })
    })();
    hide(ctx, shown);
    result
}

pub fn add_restore(ctx: &mut Ctx, restore: &Restore) -> Done<Restored> {
    let identity = restore_identity(restore)?;
    if identity["marker"]["position"] != restore.applied["position"] {
        return Err(bad("the marker's undo position doesn't match its full recorded identity"));
    }
    let created = restore.created.ok_or_else(|| bad("created: the marker that was added"))?;
    let place = marker_place(ctx, created, "ref", Epoch::Any)?;
    let shown = show(ctx)?;
    let result = (|| {
        let list = rows(ctx, shown.beats_per_bar)?;
        if list_identity(&list) != identity["applied"] {
            return Err(changed("the marker or its full list changed since Kumi added it; nothing changed"));
        }
        let index =
            unique_identity(&list, &identity["marker"]).ok_or_else(|| gone("the marker Kumi added has no unique matching identity"))?;
        if index + 1 != place {
            return Err(changed("the marker's place changed since Kumi added it; nothing changed"));
        }
        let row = &list[index];
        let name = label(&row.marker);
        let after = delete_row(ctx, &shown, row)?;
        if list_identity(&after) != identity["prior"] {
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
    let shown = show(ctx)?;
    let result = (|| {
        let to = position_input(input, shown.beats_per_bar)?;
        let list = rows(ctx, shown.beats_per_bar)?;
        let marker = &list.get(place - 1).ok_or_else(|| crate::refs::not_in_project(input["ref"].as_str().unwrap_or_default()))?.marker;
        let now = json!({"position": marker.position.to_json()});
        let mut display = json!({"title": format!("Moved {} to {}", label(marker), timeline::bar_text(&to)), "before": now, "after": {"position": to.to_json()}});
        if marker.position == to {
            display["unchanged"] = true.into();
        }
        let mut fence = now;
        fence["identity"] = list_identity(&list);
        Ok(Preview { target: target(ctx, place), prior: fence.clone(), fence, display })
    })();
    hide(ctx, shown);
    result
}

pub fn move_apply(ctx: &mut Ctx, input: &Value, fence: &Value) -> Done<Applied> {
    let place = marker_place(ctx, input, "ref", Epoch::Current)?;
    let shown = show(ctx)?;
    let result = (|| {
        let to = position_input(input, shown.beats_per_bar)?;
        let list = rows(ctx, shown.beats_per_bar)?;
        let row = list.get(place - 1).ok_or_else(|| changed("the marker went since the preview"))?;
        let now = json!({"position": row.marker.position.to_json()});
        let mut current_fence = now.clone();
        current_fence["identity"] = list_identity(&list);
        if current_fence != *fence {
            return Err(changed(format!("{} moved since the preview", label(&row.marker))));
        }
        let from = row.marker.position;
        let name = label(&row.marker);
        let (after, confirmed) = if from == to {
            (row.marker.clone(), list.clone())
        } else {
            move_marker(ctx, &shown, &from, to, &row.identity, &list_identity(&list))?
        };
        if after.position != to {
            return Err(nudge::kept(&format!("{name}'s position"), timeline::bar_text(&to), timeline::bar_text(&after.position)));
        }
        let mut expected_after = row.identity.clone();
        expected_after["position"] = after.position.to_json();
        let index = unique_identity(&confirmed, &expected_after)
            .ok_or_else(|| super::uncertain("the moved marker has no unique final identity"))?;
        let mut material = json!({"marker": confirmed[index].identity, "priorMarker": row.identity,
            "prior": list_identity(&list), "applied": list_identity(&confirmed)});
        material["undo"] = undo_hint(&material);
        let mut display =
            json!({"title": format!("Moved {name} to {}", timeline::bar_text(&to)), "before": now, "after": {"position": to.to_json()}});
        if material["undo"]["mode"] == "manual" {
            display["note"] = UNKNOWN_MARKER_RESTORE.into();
        }
        Ok(Applied {
            applied: json!({"position": after.position.to_json()}),
            target: None,
            created: None,
            material: Some(material),
            display,
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
    let identity = restore_identity(restore)?;
    if !identity["priorMarker"].is_object() {
        return Err(failed(UNKNOWN_MARKER_RESTORE));
    }
    if identity["marker"]["position"] != restore.applied["position"] || identity["priorMarker"]["position"] != restore.prior["position"] {
        return Err(bad("the marker's undo positions don't match its full recorded identity"));
    }
    let prior = position_of(restore.prior).ok_or_else(|| bad("prior.position"))?;
    let applied = position_of(restore.applied).ok_or_else(|| bad("applied.position"))?;
    let shown = show(ctx)?;
    let result = (|| {
        let list = rows(ctx, shown.beats_per_bar)?;
        let current = list_identity(&list);
        if current == identity["prior"] && unique_identity(&list, &identity["priorMarker"]).is_some() {
            return Ok(Restored {
                restored: json!({"position": prior.to_json()}),
                display: json!({"title": "The marker is where it was"}),
                undone_in_logic: false,
            });
        }
        if current != identity["applied"] {
            return Err(changed("the marker or its full list changed since Kumi moved it; nothing changed"));
        }
        let index =
            unique_identity(&list, &identity["marker"]).ok_or_else(|| changed("the moved marker has no unique matching identity"))?;
        let name = label(&list[index].marker);
        let (after, confirmed) = move_marker(ctx, &shown, &applied, prior, &identity["marker"], &identity["applied"])?;
        if after.position != prior {
            return Err(nudge::kept(&format!("{name}'s position"), timeline::bar_text(&prior), timeline::bar_text(&after.position)));
        }
        if list_identity(&confirmed) != identity["prior"] {
            return Err(super::uncertain("the restored marker list doesn't match the complete original material"));
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
    let shown = show(ctx)?;
    let result = (|| {
        let list = rows(ctx, shown.beats_per_bar)?;
        let marker = &list.get(place - 1).ok_or_else(|| crate::refs::not_in_project(input["ref"].as_str().unwrap_or_default()))?.marker;
        let mut now = state(marker);
        now["identity"] = list_identity(&list);
        Ok(Preview {
            target: target(ctx, place),
            prior: now.clone(),
            fence: now,
            display: json!({"title": format!("Deleted {}", label(marker))}),
        })
    })();
    hide(ctx, shown);
    result
}

pub fn delete_apply(ctx: &mut Ctx, input: &Value, fence: &Value) -> Done<Applied> {
    let place = marker_place(ctx, input, "ref", Epoch::Current)?;
    let shown = show(ctx)?;
    let result = (|| {
        let list = rows(ctx, shown.beats_per_bar)?;
        let row = list.get(place - 1).ok_or_else(|| changed("the marker went since the preview"))?;
        let mut now = state(&row.marker);
        now["identity"] = list_identity(&list);
        if now != *fence {
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
            target: None,
            created: None,
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
/// The marker's name cannot be set through M3's channels, and a same-named native Undo step has no ownership
/// identity. Recreating a differently named marker would not restore the value the producer lost.
pub fn delete_restore(_ctx: &mut Ctx, _watch: &mut Watch, _restore: &Restore) -> Done<Restored> {
    Err(failed(super::NATIVE_UNDO_REFUSAL))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ax::strings::Strings;
    use crate::ax::{Ax, Root};
    use crate::fixtures::Scene;
    use crate::helper::{Launch, MockHands};
    use crate::project::Project;

    #[test]
    fn markers_are_placed_by_beats_and_keep_their_recorded_material() {
        assert_eq!(position_input(&json!({"positionBeats": 32}), 4).unwrap(), Position { bar: 9, ..Position::START });
        assert!(position_input(&json!({"position": 12.0}), 4).is_err());
        let marker =
            Marker { position: Position { bar: 9, ..Position::START }, name: "Marker 1".into(), length_beats: 32.0, text: String::new() };
        assert_eq!(label(&marker), "marker “Marker 1”");
        assert_eq!(position_of(&state(&marker)), Some(marker.position));
        assert_eq!(state(&marker)["length"], 32.0);
    }

    fn scene(name: &str) -> Scene {
        Scene::load(&std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures").join(name)).unwrap()
    }

    fn table_node(node: &mut Value) -> &mut Value {
        if node["role"] == "AXTable" {
            return node;
        }
        node["c"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .find_map(|child| {
                if child["role"] == "AXTable" {
                    Some(child)
                } else if child.get("c").is_some() {
                    let parsed = Node::from_json(child);
                    paths::list_table(&parsed).is_some().then(|| table_node(child))
                } else {
                    None
                }
            })
            .unwrap()
    }

    fn recorded_list(scene: &Scene, count: usize, first_bar: u32) -> Value {
        scene
            .calls
            .iter()
            .filter(|call| call.op == "tree")
            .find_map(|call| {
                let node = Node::from_json(&call.reply["node"]);
                let table = paths::list_table(&node)?;
                let rows = paths::rows(table);
                (rows.len() == count && Marker::of(rows[0], &paths::columns(table), 4)?.position.bar == first_bar)
                    .then(|| call.reply["node"].clone())
            })
            .unwrap()
    }

    fn marker_cell<'a>(node: &'a mut Value, row: usize, column: &str) -> &'a mut Value {
        let table = table_node(node);
        let index =
            table["c"].as_array().unwrap().iter().filter(|node| node["role"] == "AXColumn").position(|node| node["id"] == column).unwrap();
        table["c"].as_array_mut().unwrap().iter_mut().filter(|node| node["role"] == "AXRow").nth(row).unwrap()["c"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .filter(|node| node["role"] == "AXCell")
            .nth(index)
            .unwrap()
    }

    fn visible_text(node: &mut Value, text: &str) {
        let table = table_node(node);
        let children = table["c"].as_array_mut().unwrap();
        for row in children.iter_mut().filter(|node| node["role"] == "AXRow") {
            row["c"].as_array_mut().unwrap().push(json!({"role": "AXCell", "c": [{"role": "AXStaticText", "value": text}]}));
        }
        children.push(json!({"role": "AXColumn", "id": "Text"}));
    }

    fn identities(node: &Value) -> Vec<Row> {
        let node = Node::from_json(node);
        let table = paths::list_table(&node).unwrap();
        rows_of(table, &paths::columns(table), 4).unwrap()
    }

    fn navigation(scene: &Scene) -> (Value, Value) {
        let mut bar =
            scene.calls.iter().find(|call| call.op == "tree" && call.reply["node"]["desc"] == "Control Bar").unwrap().reply["node"].clone();
        bar["c"].as_array_mut().unwrap().iter_mut().find(|node| node["desc"] == "List Editors").unwrap()["value"] = 1.into();
        let tabs = scene
            .calls
            .iter()
            .find(|call| {
                call.op == "tree"
                    && call.reply["node"]["role"] == "AXRadioGroup"
                    && call.reply["node"]["c"].as_array().unwrap().iter().any(|node| node["desc"] == "Marker" && node["value"] == 1)
            })
            .unwrap()
            .reply["node"]
            .clone();
        (bar, tabs)
    }

    fn ready(hands: &MockHands, bar: &Value, tabs: &Value, list: &Value) {
        for node in [bar, bar, tabs, list] {
            hands.reply("tree", Ok(json!({"node": node})));
        }
    }

    fn project(bar: &Value, tabs: &Value, list: &Value) -> Project {
        let mut project = Project::new(Strings::english());
        project.root = Root::Main;
        project.layout.headers = Some("the recorded headers are not needed by marker restoration".into());
        project.layout.control_bar = Some(bar["i"].as_str().unwrap().into());
        project.layout.list_tabs = Some(tabs["i"].as_str().unwrap().into());
        project.layout.list = Some(list["i"].as_str().unwrap().into());
        project
    }

    #[test]
    fn replaced_markers_never_receive_an_add_or_move_restore_even_with_discard_or_a_prior_position() {
        super::super::set_settling(false);
        for adding in [true, false] {
            let scene = scene(if adding { "change-marker-add" } else { "change-marker-move" });
            let (bar, tabs) = navigation(&scene);
            let mut prior_list = recorded_list(&scene, 1, 9);
            let mut applied_list = recorded_list(&scene, if adding { 2 } else { 1 }, if adding { 9 } else { 10 });
            visible_text(&mut prior_list, "original visible text");
            visible_text(&mut applied_list, "original visible text");
            let index = usize::from(adding);
            // Beyond the legacy display clipping limit: the full material must still distinguish this suffix.
            let long_name = format!("{} original", "M".repeat(rows::TEXT_LIMIT + 1));
            marker_cell(&mut applied_list, index, "Name")["c"][0]["desc"] = long_name.clone().into();
            if !adding {
                marker_cell(&mut prior_list, 0, "Name")["c"][0]["desc"] = long_name.clone().into();
            }
            let before = identities(&prior_list);
            let applied = identities(&applied_list);
            assert_eq!(applied[index].identity["name"], long_name);
            assert_eq!(applied[index].identity["text"], "original visible text");
            let mut material =
                json!({"marker": applied[index].identity, "prior": list_identity(&before), "applied": list_identity(&applied)});
            if !adding {
                material["priorMarker"] = before[0].identity.clone();
            }
            material["undo"] = undo_hint(&material);
            assert_eq!(
                material["undo"]["mode"], "value",
                "all required historical fields were captured in this derived visible-Text scene"
            );
            let prior = if adding { json!({"markers": 1}) } else { json!({"position": before[0].marker.position.to_json()}) };
            let written = state(&applied[index].marker);
            let target = json!({"ref": if adding { "1:marker:2" } else { "1:marker:1" }, "kind": "marker"});
            let hands = MockHands::new();
            let mut helper = Launch::launch(&mut hands.clone()).unwrap();
            let mut ax = Ax::new(helper.as_mut(), 7);
            let mut project = project(&bar, &tabs, &applied_list);
            let mut ctx = Ctx { project: &mut project, ax: &mut ax };
            for mutation in ["name", "length", "text", "lock", "other-row", "unreadable-length", "empty-name"] {
                for prior_position in [false, true] {
                    for discard in [false, true] {
                        let mut current = applied_list.clone();
                        match mutation {
                            "name" => {
                                marker_cell(&mut current, index, "Name")["c"][0]["desc"] =
                                    format!("{} replacement", "M".repeat(rows::TEXT_LIMIT + 1)).into()
                            }
                            "length" => marker_cell(&mut current, index, "Length")["c"][0]["desc"] = "1 0 0 0".into(),
                            "text" => marker_cell(&mut current, index, "Text")["c"][0]["value"] = "producer's replacement text".into(),
                            "lock" => marker_cell(&mut current, index, "Lock")["value"] = true.into(),
                            "unreadable-length" => marker_cell(&mut current, index, "Length")["c"][0]["desc"] = "unreadable length".into(),
                            "empty-name" => marker_cell(&mut current, index, "Name")["c"][0]["desc"] = "".into(),
                            _ if adding => marker_cell(&mut current, 0, "Name")["c"][0]["desc"] = "producer changed another marker".into(),
                            _ => {
                                visible_text(&mut current, "another column is not the same list");
                            }
                        }
                        if prior_position {
                            marker_cell(&mut current, index, "Position")["c"][0]["desc"] = "9 1 1 1 ".into();
                        }
                        let restore = Restore {
                            target: &target,
                            prior: &prior,
                            applied: &written,
                            created: Some(&target),
                            material: Some(&material),
                            discard,
                            desc: "Kumi: restore marker",
                            key: "marker".into(),
                        };
                        let calls = hands.calls().len();
                        ready(&hands, &bar, &tabs, &current);
                        let result = if adding { add_restore(&mut ctx, &restore) } else { move_restore(&mut ctx, &restore) };
                        assert!(
                            result.is_err(),
                            "{mutation}, prior position {prior_position}, discard {discard}: a replacement is never already restored"
                        );
                        assert!(hands.calls()[calls..].iter().all(|(op, _)| op == "tree"), "a changed marker receives no helper write");
                    }
                }
            }
            let original = Restore {
                target: &target,
                prior: &prior,
                applied: &written,
                created: Some(&target),
                material: Some(&material),
                discard: false,
                desc: "Kumi: restore marker",
                key: "marker".into(),
            };
            if adding {
                ready(&hands, &bar, &tabs, &applied_list);
                hands.reply("action", Ok(json!({})));
                hands.reply("tree", Ok(json!({"node": prior_list})));
                assert!(add_restore(&mut ctx, &original).is_ok(), "the untouched full marker is still removed");
            } else {
                let position = scene
                    .calls
                    .iter()
                    .filter(|call| call.op == "tree")
                    .find(|call| {
                        let node = Node::from_json(&call.reply["node"]);
                        node.role == "AXGroup"
                            && node.children.iter().filter(|child| child.role == "AXSlider").count() >= 2
                            && rows::position_of(&node).is_some_and(|position| position.bar == 10)
                    })
                    .unwrap()
                    .reply["node"]
                    .clone();
                ready(&hands, &bar, &tabs, &applied_list);
                for node in [&applied_list, &applied_list, &position] {
                    hands.reply("tree", Ok(json!({"node": node})));
                }
                hands.reply("set", Ok(json!({"after": 9})));
                hands.reply("tree", Ok(json!({"node": prior_list})));
                assert_eq!(
                    move_restore(&mut ctx, &original).unwrap().restored["position"]["bar"],
                    9,
                    "a fully known unchanged marker still moves back through its scrubber"
                );
                let calls = hands.calls().len();
                ready(&hands, &bar, &tabs, &prior_list);
                assert!(move_restore(&mut ctx, &original).is_ok(), "a complete exact prior state proves a manual value reversion");
                assert!(hands.calls()[calls..].iter().all(|(op, _)| op == "tree"));
            }
            let missing = Restore { material: None, key: original.key.clone(), ..original };
            let calls = hands.calls().len();
            let refused = if adding { add_restore(&mut ctx, &missing) } else { move_restore(&mut ctx, &missing) };
            assert!(refused.is_err());
            assert_eq!(hands.calls().len(), calls, "a legacy restore without full identity reaches no helper request");
        }
    }

    #[test]
    fn unobserved_historical_marker_text_requires_manual_undo_before_any_current_state_is_read() {
        for adding in [true, false] {
            let scene = scene(if adding { "change-marker-add" } else { "change-marker-move" });
            let (bar, tabs) = navigation(&scene);
            let prior_list = recorded_list(&scene, 1, 9);
            let applied_list = recorded_list(&scene, if adding { 2 } else { 1 }, if adding { 9 } else { 10 });
            let before = identities(&prior_list);
            let after = identities(&applied_list);
            let index = usize::from(adding);
            let mut material = json!({"marker": after[index].identity, "prior": list_identity(&before), "applied": list_identity(&after)});
            if !adding {
                material["priorMarker"] = before[0].identity.clone();
            }
            material["undo"] = undo_hint(&material);
            assert_eq!(material["marker"]["text"], Value::Null, "the recording never exposed Text");
            assert_eq!(material["undo"]["mode"], "manual");
            assert_eq!(material["undo"]["reason"], UNKNOWN_MARKER_RESTORE);
            let prior = if adding { json!({"markers": 1}) } else { json!({"position": before[0].marker.position.to_json()}) };
            let applied = state(&after[index].marker);
            let target = json!({"ref": if adding { "1:marker:2" } else { "1:marker:1" }, "kind": "marker"});
            let hands = MockHands::new();
            let mut helper = Launch::launch(&mut hands.clone()).unwrap();
            let mut ax = Ax::new(helper.as_mut(), 7);
            let mut project = project(&bar, &tabs, &applied_list);
            let mut ctx = Ctx { project: &mut project, ax: &mut ax };
            for already_reverted in [false, true] {
                let mut now = if already_reverted { prior_list.clone() } else { applied_list.clone() };
                visible_text(&mut now, "showing Text now does not establish its historical value");
                hands.set_tree(now);
                for discard in [false, true] {
                    let restore = Restore {
                        target: &target,
                        prior: &prior,
                        applied: &applied,
                        created: Some(&target),
                        material: Some(&material),
                        discard,
                        desc: "Kumi: restore marker",
                        key: "marker".into(),
                    };
                    let result = if adding { add_restore(&mut ctx, &restore) } else { move_restore(&mut ctx, &restore) };
                    assert_eq!(result.err().unwrap(), failed(UNKNOWN_MARKER_RESTORE));
                    assert!(hands.calls().is_empty(), "unknown historical fields are refused before discard or already-restored checks");
                }
            }
        }
    }
}
