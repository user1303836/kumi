//! A take's stretch markers and take markers (M4): added, changed and deleted, several at a time. Neither has an identity
//! of its own in REAPER (an index, which shifts as markers come and go), so a preview fences the take's whole list, and an
//! undo finds the markers the change wrote by their values: it refuses `changed` if any of them is not there as written,
//! and touches no other marker.
//!
//! A take marker can also carry a length the producer drew, which REAPER's API neither reads nor writes: Kumi never
//! rewrites a marker it wasn't asked to change, and a marker it deleted comes back without its length (said so).

use serde_json::{json, Value};

use super::items::{changed, close, gone};
use super::takes::{self, TakeFound};
use super::{quoted, Applied, Ctx, Done, Preview, Restore, Restored};
use crate::read::{bad, failed};
use crate::reaper::{Reaper, StretchMarker, Take, TakeMarker};
use crate::rows;

/// The most markers one change adds, changes or deletes.
const MOST: usize = 1000;

fn list<'a>(input: &'a Value, key: &str) -> Done<&'a Vec<Value>> {
    let list = input.get(key).and_then(Value::as_array).ok_or_else(|| bad(format!("{key} must be a list")))?;
    if list.is_empty() || list.len() > MOST {
        return Err(bad(format!("{key} must hold from 1 to {MOST} entries")));
    }
    Ok(list)
}

fn number(spec: &Value, key: &str) -> Done<Option<f64>> {
    match spec.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(value) => {
            value.as_f64().filter(|n| n.is_finite() && n.abs() <= 1.0e7).map(Some).ok_or_else(|| bad(format!("{key} must be a number")))
        }
    }
}

/// The indexes a request names, each once, within the list as read.
fn indexes(input: &Value, key: &str, count: usize, what: &str) -> Done<Vec<usize>> {
    let mut out = Vec::new();
    for value in list(input, key)? {
        let at = value.as_u64().map(|n| n as usize).filter(|n| *n < count).ok_or_else(|| {
            bad(format!("{key} must be positions of {what} in that take (it has {count}, counted from 0): read it again"))
        })?;
        if out.contains(&at) {
            return Err(bad(format!("a {what} is named twice")));
        }
        out.push(at);
    }
    Ok(out)
}

/// A position given as `position` (seconds from the item's start, as REAPER counts a take's markers) or `positionBeats`
/// (quarter notes from the project start), as seconds from the item's start.
fn item_position(ctx: &Ctx, spec: &Value, found: &TakeFound) -> Done<Option<f64>> {
    match (number(spec, "position")?, number(spec, "positionBeats")?) {
        (Some(_), Some(_)) => Err(bad("pass position or positionBeats, not both")),
        (Some(seconds), None) => Ok(Some(seconds)),
        (None, Some(qn)) => {
            let time = ctx.reaper.time_at_quarter_notes(ctx.proj, qn).ok_or_else(|| failed("this REAPER can't turn beats into time"))?;
            Ok(Some(time - ctx.reaper.item_value(found.item, "D_POSITION")))
        }
        (None, None) => Ok(None),
    }
}

fn changed_take(ctx: &Ctx, found: &TakeFound, what: &str) -> super::Fault {
    changed(format!("the {what} of {} changed since the preview", takes::label(ctx, found.take)))
}

fn refuse_locked(ctx: &Ctx, found: &TakeFound) -> Done<()> {
    if super::items::is_locked(ctx.reaper, found.item) {
        return Err(bad(format!("{} is locked in REAPER: unlock it first (item_props lock: false)", super::items::label(ctx, found.item))));
    }
    Ok(())
}

/// A list's count and a hash of what it holds: what a preview's indexes are fenced by.
fn fence_of(words: impl Iterator<Item = String>) -> Value {
    let (mut count, mut hash) = (0usize, 0xcbf2_9ce4_8422_2325u64);
    for word in words {
        count += 1;
        for byte in word.bytes().chain(std::iter::once(0)) {
            hash = (hash ^ byte as u64).wrapping_mul(0x0000_0100_0000_01b3);
        }
    }
    json!({"count": count, "hash": format!("{hash:016x}")})
}

// ----- stretch markers -----

/// A stretch marker to 9 significant digits, as REAPER's saved state keeps it (lab, 7.82): one back from REAPER's undo or
/// redo has the same word.
fn stretch_word(m: &StretchMarker) -> String {
    format!("{:.8e} {:.8e} {:.8e}", m.position, m.source, m.slope)
}

fn stretch_json(m: &StretchMarker) -> Value {
    json!([m.position, m.source, m.slope])
}

fn stretch_from(value: &Value) -> Option<StretchMarker> {
    Some(StretchMarker { position: value.get(0)?.as_f64()?, source: value.get(1)?.as_f64()?, slope: value.get(2)?.as_f64()? })
}

fn same_stretch(a: &StretchMarker, b: &StretchMarker) -> bool {
    close(a.position, b.position) && close(a.source, b.source) && close(a.slope, b.slope)
}

fn stretch_list(value: &Value, key: &str) -> Done<Vec<StretchMarker>> {
    value[key]
        .as_array()
        .ok_or_else(|| bad(format!("applied.{key}: the markers the change wrote")))?
        .iter()
        .map(|m| stretch_from(m).ok_or_else(|| bad("applied: a stretch marker is not [position, source, slope]")))
        .collect()
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Which {
    Add,
    Change,
    Delete,
}

struct StretchPlan {
    found: TakeFound,
    before: Vec<StretchMarker>,
    /// The places in `before` that go.
    remove: Vec<usize>,
    /// What is written.
    add: Vec<StretchMarker>,
}

fn stretch_plan(ctx: &Ctx, input: &Value, which: Which) -> Done<StretchPlan> {
    let found = takes::find(ctx, &input["ref"], "stretch markers")?;
    let reaper = ctx.reaper;
    let before = reaper.stretch_markers(found.take);
    let length = reaper.item_value(found.item, "D_LENGTH");
    let (mut remove, mut add) = (Vec::new(), Vec::new());
    match which {
        Which::Add => {
            for spec in list(input, "markers")? {
                let position = item_position(ctx, spec, &found)?
                    .ok_or_else(|| bad("every marker needs a position (seconds from the item's start) or positionBeats"))?;
                // Not given, its source is left to REAPER, which pins the point that plays there now (the take's offset
                // plus the position at its rate holds only while no marker is moved): what it wrote is read back.
                let source = number(spec, "sourcePosition")?.unwrap_or(f64::NAN);
                add.push(StretchMarker { position, source, slope: 0.0 });
            }
        }
        Which::Change => {
            for spec in list(input, "changes")? {
                let at = spec.get("index").and_then(Value::as_u64).map(|n| n as usize).filter(|n| *n < before.len()).ok_or_else(|| {
                    bad(format!(
                        "every change needs the index of a stretch marker in that take (it has {}, from 0): read it again",
                        before.len()
                    ))
                })?;
                let mut marker = before[at];
                if let Some(position) = item_position(ctx, spec, &found)? {
                    marker.position = position;
                }
                if let Some(source) = number(spec, "sourcePosition")? {
                    marker.source = source;
                }
                if let Some(slope) = number(spec, "slope")? {
                    if !(-1.0..=1.0).contains(&slope) {
                        return Err(bad("slope must be from -1 to 1"));
                    }
                    marker.slope = slope;
                }
                if remove.contains(&at) {
                    return Err(bad("a stretch marker is named twice"));
                }
                remove.push(at);
                add.push(marker);
            }
        }
        Which::Delete => remove = indexes(input, "indexes", before.len(), "stretch markers")?,
    }
    // REAPER keeps one marker at a position, in order: what is left and what is written may not share one.
    let mut positions: Vec<f64> = before
        .iter()
        .enumerate()
        .filter(|(i, _)| !remove.contains(i))
        .map(|(_, m)| m.position)
        .chain(add.iter().map(|m| m.position))
        .collect();
    positions.sort_by(f64::total_cmp);
    if positions.windows(2).any(|pair| (pair[1] - pair[0]).abs() < 1e-6) {
        return Err(bad("two stretch markers would be at one position: REAPER keeps one marker at a time there"));
    }
    if add.iter().any(|m| m.position < -1e-9 || m.position > length + 1e-9 || m.source < -1e-9) {
        return Err(bad("a stretch marker must be inside the item (0 to its length in seconds) at a source position from 0"));
    }
    // A marker changed in place stays between its neighbours (REAPER keeps it there): the order may not change.
    if which == Which::Change {
        let mut after = before.clone();
        for (at, marker) in remove.iter().zip(&add) {
            after[*at] = *marker;
        }
        if after.windows(2).any(|pair| pair[1].position <= pair[0].position) {
            return Err(bad("a stretch marker moves only between its neighbours: delete it and add one where it should be"));
        }
    }
    remove.sort_unstable();
    Ok(StretchPlan { found, before, remove, add })
}

fn stretch_fence(markers: &[StretchMarker]) -> Value {
    fence_of(markers.iter().map(stretch_word))
}

fn stretch_title(ctx: &Ctx, take: Take, which: Which, count: usize) -> String {
    let verb = match which {
        Which::Add => "Added",
        Which::Change => "Moved",
        Which::Delete => "Deleted",
    };
    format!("{verb} {count} stretch marker{} in {}", if count == 1 { "" } else { "s" }, takes::label(ctx, take))
}

/// Writes a take's stretch markers to exactly `after`, from `before`: the ones that go are deleted, the new ones added,
/// and each one's slope set. Markers in both are left alone.
fn write_stretch(reaper: &Reaper, take: Take, gone: &[StretchMarker], new: &[StretchMarker]) -> bool {
    let mut ok = true;
    for marker in gone {
        let now = reaper.stretch_markers(take);
        match now.iter().position(|m| same_stretch(m, marker)) {
            Some(at) => ok &= reaper.delete_stretch_markers(take, at as i32, 1) == 1,
            None => ok = false,
        }
    }
    for marker in new {
        // (a source of NaN is REAPER's to find)
        let at = reaper.set_stretch_marker(take, -1, marker.position, Some(marker.source).filter(|s| !s.is_nan()));
        ok &= at >= 0;
        if at >= 0 && marker.slope != 0.0 {
            ok &= reaper.set_stretch_slope(take, at, marker.slope);
        }
    }
    ok
}

/// The markers of `a` that `b` has no marker for (each counted once).
fn stretch_difference(a: &[StretchMarker], b: &[StretchMarker]) -> Vec<StretchMarker> {
    let mut left: Vec<StretchMarker> = b.to_vec();
    a.iter()
        .filter(|m| match left.iter().position(|other| same_stretch(other, m)) {
            Some(at) => {
                left.remove(at);
                false
            }
            None => true,
        })
        .copied()
        .collect()
}

pub fn stretch_preview(ctx: &Ctx, input: &Value, which: Which) -> Done<Preview> {
    let plan = stretch_plan(ctx, input, which)?;
    let count = plan.add.len().max(plan.remove.len());
    let fence = stretch_fence(&plan.before);
    let gone: Vec<Value> = plan.remove.iter().map(|&i| stretch_json(&plan.before[i])).collect();
    Ok(Preview {
        target: takes::target(&plan.found.guid),
        prior: json!({"markers": plan.before.len(), "fence": fence.clone(), "removed": gone}),
        fence,
        display: json!({"title": stretch_title(ctx, plan.found.take, which, count), "count": count}),
    })
}

pub fn stretch_apply(ctx: &Ctx, input: &Value, fence: &Value, which: Which) -> Done<Applied> {
    let plan = stretch_plan(ctx, input, which).map_err(|(code, message)| {
        if code == "failed" {
            changed(format!("{message}: it went since the preview"))
        } else {
            (code, message)
        }
    })?;
    if stretch_fence(&plan.before) != *fence {
        return Err(changed_take(ctx, &plan.found, "stretch markers"));
    }
    refuse_locked(ctx, &plan.found)?;
    let reaper = ctx.reaper;
    let take = plan.found.take;
    let removed: Vec<StretchMarker> = plan.remove.iter().map(|&i| plan.before[i]).collect();
    let wrote = write_stretch(reaper, take, &removed, &plan.add);
    let after = reaper.stretch_markers(take);
    let (gone_now, added_now) = (stretch_difference(&plan.before, &after), stretch_difference(&after, &plan.before));
    // Each marker asked for is one REAPER made: at its place, with its source where one was given.
    let mut made = added_now.clone();
    let all_made = plan.add.iter().all(|wanted| {
        let found = made.iter().position(|m| {
            close(m.position, wanted.position) && close(m.slope, wanted.slope) && (wanted.source.is_nan() || close(m.source, wanted.source))
        });
        found.map(|at| made.remove(at)).is_some()
    });
    let whole = wrote && stretch_difference(&removed, &gone_now).is_empty() && all_made && made.is_empty();
    if !whole {
        // Half a change is none: the take's markers go back as they were.
        let back = write_stretch(reaper, take, &added_now, &gone_now) && stretch_fence(&reaper.stretch_markers(take)) == *fence;
        return Err(if back {
            failed("REAPER didn't take those stretch markers as asked; the take's markers are as they were")
        } else {
            (
                "uncertain".into(),
                "REAPER didn't take those stretch markers as asked, and Kumi could not put them back: read the take".into(),
            )
        });
    }
    reaper.update_item(plan.found.item);
    let count = plan.add.len().max(plan.remove.len());
    Ok(Applied {
        applied: json!({"added": added_now.iter().map(stretch_json).collect::<Vec<_>>(), "removed": gone_now.iter().map(stretch_json).collect::<Vec<_>>(), "markers": after.len()}),
        display: json!({"title": stretch_title(ctx, take, which, count)}),
        unwritten: gone_now.is_empty() && added_now.is_empty(),
        ..Default::default()
    })
}

pub fn stretch_restore(ctx: &Ctx, restore: &Restore) -> Done<Restored> {
    let found = takes::find(ctx, &restore.target["ref"], "stretch markers").map_err(|(code, message)| {
        if code == "failed" {
            gone(message)
        } else {
            (code, message)
        }
    })?;
    let reaper = ctx.reaper;
    let (added, removed) = (stretch_list(restore.applied, "added")?, stretch_list(restore.applied, "removed")?);
    let current = reaper.stretch_markers(found.take);
    // The list as it was before the change: back already (a retry, or the producer took it back by hand).
    if stretch_fence(&current) == restore.prior["fence"] {
        return Ok(Restored {
            restored: json!({"markers": current.len()}),
            display: json!({"title": format!("Stretch markers in {} are as they were", takes::label(ctx, found.take))}),
            unwritten: true,
        });
    }
    let present: Vec<StretchMarker> = added.iter().filter(|m| current.iter().any(|c| same_stretch(c, m))).copied().collect();
    let to_bring_back = stretch_difference(&removed, &current);
    if present.len() < added.len() && !restore.discard {
        return Err(changed(format!(
            "the stretch markers Kumi wrote in {} have been changed in REAPER since",
            takes::label(ctx, found.take)
        )));
    }
    // A marker the producer has put where one goes back would make REAPER keep one of the two.
    let kept: Vec<StretchMarker> = stretch_difference(&current, &present);
    if !restore.discard && to_bring_back.iter().any(|m| kept.iter().any(|k| (k.position - m.position).abs() < 1e-6)) {
        return Err(changed("a stretch marker is at the place of one that goes back: the markers can't all be put back"));
    }
    let unwritten = present.is_empty() && to_bring_back.is_empty();
    if !unwritten && !write_stretch(reaper, found.take, &present, &to_bring_back) {
        return Err(("uncertain".into(), "REAPER didn't take all of the stretch markers back: read the take".into()));
    }
    reaper.update_item(found.item);
    Ok(Restored {
        restored: json!({"markers": reaper.stretch_markers(found.take).len()}),
        display: json!({"title": format!("Stretch markers in {} put back ({} removed, {} added again)", takes::label(ctx, found.take), present.len(), to_bring_back.len())}),
        unwritten,
    })
}

// ----- take markers -----

fn marker_json(m: &TakeMarker) -> Value {
    json!([m.source, m.name, m.color])
}

fn marker_from(value: &Value) -> Option<TakeMarker> {
    Some(TakeMarker { source: value.get(0)?.as_f64()?, name: value.get(1)?.as_str()?.to_string(), color: value.get(2)?.as_i64()? as i32 })
}

fn same_marker(a: &TakeMarker, b: &TakeMarker) -> bool {
    close(a.source, b.source) && a.name == b.name && a.color == b.color
}

fn marker_word(m: &TakeMarker) -> String {
    format!("{:.9} {} {}", m.source, m.color, m.name)
}

fn marker_list(value: &Value, key: &str) -> Done<Vec<TakeMarker>> {
    value[key]
        .as_array()
        .ok_or_else(|| bad(format!("applied.{key}: the markers the change wrote")))?
        .iter()
        .map(|m| marker_from(m).ok_or_else(|| bad("applied: a take marker is not [source position, name, colour]")))
        .collect()
}

fn marker_difference(a: &[TakeMarker], b: &[TakeMarker]) -> Vec<TakeMarker> {
    let mut left: Vec<TakeMarker> = b.to_vec();
    a.iter()
        .filter(|m| match left.iter().position(|other| same_marker(other, m)) {
            Some(at) => {
                left.remove(at);
                false
            }
            None => true,
        })
        .cloned()
        .collect()
}

/// A take marker's source position: given as `sourcePosition` (seconds in the source), or as a place in the item
/// (`position`, `positionBeats`) through the take's offset and rate (not under stretch markers, which bend that).
fn source_position(ctx: &Ctx, spec: &Value, found: &TakeFound) -> Done<Option<f64>> {
    let given = number(spec, "sourcePosition")?;
    let place = item_position(ctx, spec, found)?;
    match (given, place) {
        (Some(_), Some(_)) => Err(bad("pass sourcePosition, position or positionBeats: one of them")),
        (Some(source), None) => Ok(Some(source)),
        (None, Some(position)) => {
            let reaper = ctx.reaper;
            if !reaper.stretch_markers(found.take).is_empty() {
                return Err(bad("this take has stretch markers, which bend where its source plays: give the marker's sourcePosition"));
            }
            Ok(Some(reaper.take_value(found.take, "D_STARTOFFS") + position * reaper.take_value(found.take, "D_PLAYRATE")))
        }
        (None, None) => Ok(None),
    }
}

fn name_of(spec: &Value) -> Done<Option<String>> {
    match spec.get("name") {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(name)) if name.chars().count() <= 256 && !name.chars().any(char::is_control) => Ok(Some(name.clone())),
        Some(_) => Err(bad("name must be one line of at most 256 characters")),
    }
}

fn color_of(ctx: &Ctx, spec: &Value) -> Done<Option<i32>> {
    match spec.get("color") {
        None => Ok(None),
        Some(value) => Ok(Some(super::items::color_of(ctx, value, "color")?.as_i64().unwrap_or(0) as i32)),
    }
}

struct MarkerPlan {
    found: TakeFound,
    before: Vec<TakeMarker>,
    /// (index in `before`, what it becomes) for a change; (None, what is added) for an add.
    writes: Vec<(Option<usize>, TakeMarker)>,
    remove: Vec<usize>,
}

fn marker_plan(ctx: &Ctx, input: &Value, which: Which) -> Done<MarkerPlan> {
    let found = takes::find(ctx, &input["ref"], "take markers")?;
    let before = ctx.reaper.take_markers(found.take);
    let (mut writes, mut remove) = (Vec::new(), Vec::new());
    match which {
        Which::Add => {
            for spec in list(input, "markers")? {
                let source = source_position(ctx, spec, &found)?
                    .ok_or_else(|| bad("every marker needs a sourcePosition (seconds in the source), position or positionBeats"))?;
                writes.push((
                    None,
                    TakeMarker { source, name: name_of(spec)?.unwrap_or_default(), color: color_of(ctx, spec)?.unwrap_or(0) },
                ));
            }
        }
        Which::Change => {
            for spec in list(input, "changes")? {
                let at = spec.get("index").and_then(Value::as_u64).map(|n| n as usize).filter(|n| *n < before.len()).ok_or_else(|| {
                    bad(format!(
                        "every change needs the index of a take marker in that take (it has {}, from 0): read it again",
                        before.len()
                    ))
                })?;
                if writes.iter().any(|(index, _)| *index == Some(at)) {
                    return Err(bad("a take marker is named twice"));
                }
                let mut marker = before[at].clone();
                if let Some(source) = source_position(ctx, spec, &found)? {
                    marker.source = source;
                }
                if let Some(name) = name_of(spec)? {
                    marker.name = name;
                }
                if let Some(color) = color_of(ctx, spec)? {
                    marker.color = color;
                }
                writes.push((Some(at), marker));
            }
        }
        Which::Delete => remove = indexes(input, "indexes", before.len(), "take markers")?,
    }
    if writes.iter().any(|(_, m)| m.source < -1e-9) {
        return Err(bad("a take marker's source position is from 0 (seconds in the source)"));
    }
    Ok(MarkerPlan { found, before, writes, remove })
}

fn marker_fence(markers: &[TakeMarker]) -> Value {
    fence_of(markers.iter().map(marker_word))
}

fn marker_title(ctx: &Ctx, plan: &MarkerPlan, which: Which) -> String {
    let take = takes::label(ctx, plan.found.take);
    match (which, plan.writes.as_slice()) {
        (Which::Add, [(_, marker)]) if !marker.name.is_empty() => {
            format!("Added the take marker {} to {take}", quoted(&rows::clip(&marker.name)))
        }
        (Which::Add, writes) => format!("Added {} take marker{} to {take}", writes.len(), if writes.len() == 1 { "" } else { "s" }),
        (Which::Change, writes) => format!("Changed {} take marker{} in {take}", writes.len(), if writes.len() == 1 { "" } else { "s" }),
        (Which::Delete, _) => {
            format!("Deleted {} take marker{} from {take}", plan.remove.len(), if plan.remove.len() == 1 { "" } else { "s" })
        }
    }
}

pub fn marker_preview(ctx: &Ctx, input: &Value, which: Which) -> Done<Preview> {
    let plan = marker_plan(ctx, input, which)?;
    let fence = marker_fence(&plan.before);
    let mut display = json!({"title": marker_title(ctx, &plan, which)});
    if which == Which::Delete {
        display["note"] =
            "A take marker's length (drawn in REAPER) isn't in REAPER's API: undo_change brings a deleted marker back without it.".into();
    }
    Ok(Preview {
        target: takes::target(&plan.found.guid),
        prior: json!({"markers": plan.before.len(), "fence": fence.clone()}),
        fence,
        display,
    })
}

pub fn marker_apply(ctx: &Ctx, input: &Value, fence: &Value, which: Which) -> Done<Applied> {
    let plan = marker_plan(ctx, input, which).map_err(|(code, message)| {
        if code == "failed" {
            changed(format!("{message}: it went since the preview"))
        } else {
            (code, message)
        }
    })?;
    if marker_fence(&plan.before) != *fence {
        return Err(changed_take(ctx, &plan.found, "take markers"));
    }
    refuse_locked(ctx, &plan.found)?;
    let reaper = ctx.reaper;
    let take = plan.found.take;
    let mut ok = true;
    // Changes by index first, each found again by what it was (a change of position re-sorts the list); then adds; deletes
    // from the last, so earlier indexes hold.
    for (at, marker) in &plan.writes {
        match at {
            Some(at) => {
                let was = &plan.before[*at];
                let index = reaper.take_markers(take).iter().position(|m| same_marker(m, was));
                ok &= index.is_some_and(|index| {
                    reaper.set_take_marker(take, index as i32, &marker.name, Some(marker.source), Some(marker.color)) >= 0
                });
            }
            None => ok &= reaper.set_take_marker(take, -1, &marker.name, Some(marker.source), Some(marker.color)) >= 0,
        }
    }
    let mut remove = plan.remove.clone();
    remove.sort_unstable_by(|a, b| b.cmp(a));
    for at in remove {
        ok &= reaper.delete_take_marker(take, at as i32);
    }
    let after = reaper.take_markers(take);
    let (gone_now, added_now) = (marker_difference(&plan.before, &after), marker_difference(&after, &plan.before));
    let wanted_gone: Vec<TakeMarker> = plan
        .remove
        .iter()
        .map(|&i| plan.before[i].clone())
        .chain(plan.writes.iter().filter_map(|(at, _)| at.map(|i| plan.before[i].clone())))
        .collect();
    let wanted_new: Vec<TakeMarker> = plan.writes.iter().map(|(_, m)| m.clone()).collect();
    let as_asked = ok && marker_difference(&wanted_gone, &gone_now).is_empty() && marker_difference(&wanted_new, &added_now).is_empty();
    if !as_asked {
        return Err(("uncertain".into(), "REAPER didn't take those take markers as asked: read the take".into()));
    }
    reaper.update_item(plan.found.item);
    Ok(Applied {
        applied: json!({"added": added_now.iter().map(marker_json).collect::<Vec<_>>(), "removed": gone_now.iter().map(marker_json).collect::<Vec<_>>(), "markers": after.len()}),
        display: json!({"title": marker_title(ctx, &plan, which)}),
        unwritten: gone_now.is_empty() && added_now.is_empty(),
        ..Default::default()
    })
}

pub fn marker_restore(ctx: &Ctx, restore: &Restore) -> Done<Restored> {
    let found =
        takes::find(ctx, &restore.target["ref"], "take markers")
            .map_err(|(code, message)| if code == "failed" { gone(message) } else { (code, message) })?;
    let reaper = ctx.reaper;
    let (added, removed) = (marker_list(restore.applied, "added")?, marker_list(restore.applied, "removed")?);
    let current = reaper.take_markers(found.take);
    let present: Vec<TakeMarker> = {
        let mut left = current.clone();
        added
            .iter()
            .filter(|m| match left.iter().position(|c| same_marker(c, m)) {
                Some(at) => {
                    left.remove(at);
                    true
                }
                None => false,
            })
            .cloned()
            .collect()
    };
    if marker_fence(&current) == restore.prior["fence"] {
        return Ok(Restored {
            restored: json!({"markers": current.len()}),
            display: json!({"title": format!("Take markers in {} are as they were", takes::label(ctx, found.take))}),
            unwritten: true,
        });
    }
    let to_bring_back = marker_difference(&removed, &marker_difference(&current, &present));
    if present.len() < added.len() && !restore.discard {
        return Err(changed(format!("the take markers Kumi wrote in {} have been changed in REAPER since", takes::label(ctx, found.take))));
    }
    let unwritten = present.is_empty() && to_bring_back.is_empty();
    let mut ok = true;
    for marker in &present {
        let index = reaper.take_markers(found.take).iter().position(|m| same_marker(m, marker));
        ok &= index.is_some_and(|index| reaper.delete_take_marker(found.take, index as i32));
    }
    for marker in &to_bring_back {
        ok &= reaper.set_take_marker(found.take, -1, &marker.name, Some(marker.source), Some(marker.color)) >= 0;
    }
    if !ok {
        return Err(("uncertain".into(), "REAPER didn't take all of the take markers back: read the take".into()));
    }
    reaper.update_item(found.item);
    let mut display = json!({"title": format!("Take markers in {} put back ({} removed, {} added again)", takes::label(ctx, found.take), present.len(), to_bring_back.len())});
    if !to_bring_back.is_empty() {
        display["note"] = "A marker's length, if it had one, isn't in REAPER's API and didn't come back.".into();
    }
    Ok(Restored { restored: json!({"markers": reaper.take_markers(found.take).len()}), display, unwritten })
}
