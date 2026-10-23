//! Track Stacks. `track-stack` makes a summing or folder stack of the tracks named: the first selected through its
//! header, the others added with Shift-clicks on theirs, then Logic's own key command (Create Summing Stack,
//! Cmd-Shift-G, or Create Folder Stack, Cmd-Shift-H: what Track > Create Track Stack… and its dialog come to).
//! `track-stack-flatten` takes one apart: its header selected, Flatten Stack (Cmd-Shift-U). Logic names a stack
//! itself (Sum 1, Folder 1); `name` renames it through the Inspector's name field as `track-rename` does. Both are
//! structural: the epoch moves. Read back from the headers: a stack's header carries a disclosure triangle (the
//! track row's `stack: true`) and its subtracks follow it; Accessibility shows no membership beyond that order.
//!
//! A stack Kumi made is taken back by Logic's own undo while that step is on top (it gives the tracks their places
//! back exactly), else flattened by value. A flattened stack comes back only through Logic's own undo: which tracks
//! were under it isn't readable once it is gone.

use serde_json::{json, Value};

use super::nudge;
use super::tracks::{self, Found};
use super::undo::Probe;
use super::{changed, front, quoted, ref_text, structure, timeline, Applied, Ctx, Done, Epoch, Preview, Restore, Restored, Watch};
use crate::keys::{Chord, KeyCommands};
use crate::refs::{bad, failed};

/// Logic's key commands, by id and their factory chords.
const CREATE_SUMMING: (u32, &str) = (1793, "cmd-shift-g");
const CREATE_FOLDER: (u32, &str) = (1792, "cmd-shift-h");
const FLATTEN: (u32, &str) = (1794, "cmd-shift-u");
const OPEN_STACK: (u32, &str) = (2274, "cmd-ctrl-right");
const SELECT_SETTLE: std::time::Duration = std::time::Duration::from_millis(300);
const STACK_SETTLE: std::time::Duration = std::time::Duration::from_millis(900);

fn chord_of(keys: &KeyCommands, command: (u32, &str)) -> Done<Chord> {
    keys.chord(command.0)
        .cloned()
        .or_else(|| Chord::parse(command.1))
        .ok_or_else(|| failed(format!("no key for Logic Pro's command {}", command.0)))
}

/// What a `track-stack` asks: the tracks' places (in order, without repeats), the kind, a name.
struct Plan {
    places: Vec<usize>,
    kind: &'static str,
    name: Option<String>,
}

fn plan(ctx: &Ctx, input: &Value) -> Done<Plan> {
    let refs = input.get("refs").and_then(Value::as_array).ok_or_else(|| bad("refs must be a list of track refs"))?;
    if refs.is_empty() || refs.len() > 64 {
        return Err(bad("refs must name from 1 to 64 tracks"));
    }
    let mut places = Vec::with_capacity(refs.len());
    for reference in refs {
        let place = super::track_place(ctx, &json!({"ref": reference}), "ref", Epoch::Current)?;
        if !places.contains(&place) {
            places.push(place);
        }
    }
    places.sort_unstable();
    let kind = match input.get("kind").and_then(Value::as_str) {
        Some("summing") => "summing",
        Some("folder") => "folder",
        _ => return Err(bad("kind must be \"summing\" or \"folder\"")),
    };
    let name = match input.get("name") {
        None | Some(Value::Null) => None,
        Some(value) => Some(value.as_str().filter(|n| !n.trim().is_empty()).ok_or_else(|| bad("name must be text"))?.to_string()),
    };
    Ok(Plan { places, kind, name })
}

fn fence_of(all: &[Found]) -> Value {
    json!({"tracks": all.len(), "names": tracks::names(all)})
}

fn label(all: &[Found], places: &[usize]) -> String {
    let names: Vec<String> = places.iter().filter_map(|p| all.get(p - 1)).map(|f| quoted(&f.header.name)).collect();
    match names.len() {
        0 => String::new(),
        1 => names[0].clone(),
        n => format!("{} and {}", names[..n - 1].join(", "), names[n - 1]),
    }
}

/// Selects the tracks at `places` together: the first alone, the others with Shift-clicks. The headers as read.
fn select_all(ctx: &mut Ctx, places: &[usize]) -> Done<Vec<Found>> {
    let Some((first, rest)) = places.split_first() else { return Err(bad("refs must name a track")) };
    structure::select(ctx, *first)?;
    for place in rest {
        nudge::read(ctx.project.extend_track_selection(ctx.ax, *place))?;
        super::pause(SELECT_SETTLE);
    }
    let all = nudge::read(tracks::headers(ctx))?;
    let selected: Vec<usize> = all.iter().filter(|f| f.header.selected).map(|f| f.place).collect();
    if selected != places {
        return Err(failed(format!("Logic Pro selected tracks {selected:?} when Kumi asked for {places:?}; nothing changed")));
    }
    Ok(all)
}

/// Whether a stack header's disclosure triangle shows the stack open (its subtracks shown as headers).
fn is_open(found: &Found) -> Option<bool> {
    found.item.children.iter().find(|c| c.role == "AXDisclosureTriangle").map(|c| c.number().unwrap_or(0.0) >= 1.0)
}

/// The headers, with the stack at `place` opened first when Logic shows it closed (a new folder stack is), so its
/// subtracks are headers of their own and counted: the header selected and Logic's Open Track Stack command (the
/// triangle takes no press from outside).
fn opened(ctx: &mut Ctx, keys: &KeyCommands, place: usize) -> Done<Vec<Found>> {
    let all = nudge::read(tracks::headers(ctx))?;
    let Some(found) = all.get(place.wrapping_sub(1)) else { return Ok(all) };
    if is_open(found) != Some(false) {
        return Ok(all);
    }
    structure::select(ctx, place)?;
    front::post(ctx.ax, &chord_of(keys, OPEN_STACK)?)?;
    super::pause(STACK_SETTLE);
    nudge::wrote(tracks::headers(ctx))
}

/// Flattens the stack whose header is at `place`: opened, selected, Flatten Stack. How many headers showed with it
/// open, and the headers as read after.
fn flatten(ctx: &mut Ctx, keys: &KeyCommands, place: usize) -> Done<(usize, Vec<Found>)> {
    let shown = opened(ctx, keys, place)?.len();
    structure::select(ctx, place)?;
    front::post(ctx.ax, &chord_of(keys, FLATTEN)?)?;
    super::pause(STACK_SETTLE);
    timeline::look(ctx).map_err(|(_, why)| super::uncertain(why))?;
    Ok((shown, nudge::wrote(tracks::headers(ctx))?))
}

// ----- track-stack -----

pub fn stack_preview(ctx: &mut Ctx, input: &Value) -> Done<Preview> {
    let plan = plan(ctx, input)?;
    let all = nudge::read(tracks::headers(ctx))?;
    if let Some(stacked) = plan.places.iter().find(|p| all.get(*p - 1).is_some_and(|f| f.header.stack)) {
        return Err(bad(format!("track {stacked} is a Track Stack already; Logic Pro doesn't stack stacks from outside")));
    }
    let mut display = json!({"title": format!("{} stack of {}", if plan.kind == "summing" { "Summing" } else { "Folder" }, label(&all, &plan.places)), "kind": plan.kind});
    if let Some(name) = &plan.name {
        display["name"] = name.clone().into();
    }
    Ok(Preview { target: json!({"ref": null, "kind": "track"}), prior: json!({"tracks": all.len()}), fence: fence_of(&all), display })
}

pub fn stack_apply(ctx: &mut Ctx, keys: &KeyCommands, input: &Value, fence: &Value) -> Done<Applied> {
    let plan = plan(ctx, input)?;
    let all = nudge::read(tracks::headers(ctx))?;
    if fence_of(&all) != *fence {
        return Err(changed("the tracks changed since the preview"));
    }
    let before = tracks::names(&all);
    let members = label(&all, &plan.places);
    select_all(ctx, &plan.places)?;
    front::post(ctx.ax, &chord_of(keys, if plan.kind == "summing" { CREATE_SUMMING } else { CREATE_FOLDER })?)?;
    super::pause(STACK_SETTLE);
    let place = plan.places[0];
    let after = opened(ctx, keys, place)?;
    let header = after.get(place - 1).filter(|f| f.header.stack && after.len() == all.len() + 1);
    let Some(header) = header else {
        return Err(super::uncertain(format!(
            "Logic Pro shows {} tracks and no stack header at track {place} after Kumi asked for a {} stack",
            after.len(),
            plan.kind
        )));
    };
    let mut name = header.header.name.clone();
    if let Some(wanted) = &plan.name {
        let shown = tracks::rename(ctx, place, wanted)?;
        if shown != *wanted {
            return Err(nudge::kept("the stack's name", quoted(wanted), quoted(&shown)));
        }
        name = shown;
    }
    // The reader sees the new header: the epoch moves once, and the refs answered are in it.
    timeline::look(ctx).map_err(|(_, why)| super::uncertain(why))?;
    let names: Vec<String> = nudge::wrote(tracks::headers(ctx))?.iter().map(|f| f.header.name.clone()).collect();
    let subtracks: Vec<String> = (place + 1..=place + plan.places.len()).map(|p| ref_text(ctx, "track", &[p])).collect();
    Ok(Applied {
        applied: json!({"index": place, "name": name, "kind": plan.kind, "tracks": subtracks}),
        created: Some(json!({"ref": ref_text(ctx, "track", &[place]), "kind": "track", "stack": plan.kind})),
        display: json!({"title": format!("{} stack {} of {members}", if plan.kind == "summing" { "Summing" } else { "Folder" }, quoted(&name))}),
        step: Some("Create Track Stack"),
        probe: Some(Probe::Headers { prior: before, applied: names }),
        ..Applied::default()
    })
}

pub fn stack_restore(ctx: &mut Ctx, keys: &KeyCommands, watch: &mut Watch, restore: &Restore) -> Done<Restored> {
    let place = super::track_place(ctx, restore.created.unwrap_or(&Value::Null), "ref", Epoch::Any).map_err(|f| {
        if f.0 == "failed" {
            super::gone(f.1)
        } else {
            f
        }
    })?;
    let all = opened(ctx, keys, place)?;
    let name = restore.applied["name"].as_str().unwrap_or_default();
    let Some(found) = all.get(place - 1) else { return Err(super::gone("that stack isn't among the tracks any more")) };
    if !found.header.stack || found.header.name != name {
        if restore.discard {
            return Ok(Restored::by_value(
                json!({"tracks": all.len()}),
                json!({"title": format!("Stack {} is gone already", quoted(name))}),
            ));
        }
        return Err(changed(format!("track {place} isn't the stack {} Kumi made any more", quoted(name))));
    }
    let wanted = all.len() - 1;
    // Logic's own undo first: it gives the tracks their places back exactly.
    if let Ok(pressed) = structure::undo_in_logic(ctx, watch, &restore.key, "Create Track Stack", None) {
        super::pause(STACK_SETTLE);
        timeline::look(ctx).map_err(|(_, why)| super::uncertain(why))?;
        let after = nudge::wrote(tracks::headers(ctx))?;
        if after.len() != wanted {
            return Err(super::uncertain(format!(
                "Logic Pro's {pressed:?} was pressed, but it shows {} tracks, not {wanted}",
                after.len()
            )));
        }
        return Ok(Restored {
            restored: json!({"tracks": after.len(), "pressed": pressed}),
            display: json!({"title": format!("Stack {} taken apart again", quoted(name)), "note": "Put back with Logic Pro's own undo."}),
            undone_in_logic: true,
        });
    }
    let (shown, after) = flatten(ctx, keys, place)?;
    if after.len() != shown - 1 || after.get(place - 1).is_some_and(|f| f.header.stack && f.header.name == name) {
        return Err(super::uncertain(format!("Logic Pro shows {} tracks after Kumi flattened the stack, not {}", after.len(), shown - 1)));
    }
    Ok(Restored::by_value(json!({"tracks": after.len()}), json!({"title": format!("Stack {} flattened again", quoted(name))})))
}

// ----- track-stack-flatten -----

fn stack_at(ctx: &mut Ctx, input: &Value, epoch: Epoch) -> Done<(usize, Vec<Found>)> {
    let place = super::track_place(ctx, input, "ref", epoch)?;
    let all = nudge::read(tracks::headers(ctx))?;
    let found = all.get(place - 1).ok_or_else(|| crate::refs::not_in_project(input["ref"].as_str().unwrap_or_default()))?;
    if !found.header.stack {
        return Err(bad(format!("track {place} {} isn't a Track Stack", quoted(&found.header.name))));
    }
    Ok((place, all))
}

pub fn flatten_preview(ctx: &mut Ctx, input: &Value) -> Done<Preview> {
    let (place, all) = stack_at(ctx, input, Epoch::Current)?;
    let name = all[place - 1].header.name.clone();
    Ok(Preview {
        target: json!({"ref": ref_text(ctx, "track", &[place]), "kind": "track"}),
        prior: json!({"name": name, "index": place, "tracks": all.len()}),
        fence: fence_of(&all),
        display: json!({"title": format!("Stack {} flattened", quoted(&name))}),
    })
}

pub fn flatten_apply(ctx: &mut Ctx, keys: &KeyCommands, input: &Value, fence: &Value) -> Done<Applied> {
    let (place, all) = stack_at(ctx, input, Epoch::Current)?;
    if fence_of(&all) != *fence {
        return Err(changed("the tracks changed since the preview"));
    }
    let name = all[place - 1].header.name.clone();
    let (shown, after) = flatten(ctx, keys, place)?;
    if after.len() != shown - 1 || after.get(place - 1).is_some_and(|f| f.header.stack && f.header.name == name) {
        return Err(super::uncertain(format!(
            "Logic Pro shows {} tracks after Kumi flattened {}, not {}",
            after.len(),
            quoted(&name),
            shown - 1
        )));
    }
    let names = tracks::names(&after);
    Ok(Applied {
        applied: json!({"name": name, "tracks": after.len()}),
        material: Some(json!({"name": name, "index": place, "tracks": shown})),
        display: json!({"title": format!("Stack {} flattened", quoted(&name))}),
        step: Some("Flatten Stack"),
        probe: Some(Probe::Headers { prior: tracks::names(&all), applied: names }),
        ..Applied::default()
    })
}

pub fn flatten_restore(ctx: &mut Ctx, keys: &KeyCommands, watch: &mut Watch, restore: &Restore) -> Done<Restored> {
    let material = restore.material.ok_or_else(|| bad("material: the stack as it was"))?;
    let name = material["name"].as_str().unwrap_or_default().to_string();
    let place = material["index"].as_u64().unwrap_or(0) as usize;
    let all = nudge::read(tracks::headers(ctx))?;
    if all.get(place.wrapping_sub(1)).is_some_and(|f| f.header.stack && f.header.name == name) {
        return Ok(Restored::by_value(json!({"tracks": all.len()}), json!({"title": format!("Stack {} is back already", quoted(&name))})));
    }
    let shown = material["tracks"].as_u64().unwrap_or(0) as usize;
    // Which tracks were under the stack isn't readable once it is flat: Logic's own undo, while on top.
    let pressed = structure::undo_in_logic(ctx, watch, &restore.key, "Flatten Stack", None)?;
    super::pause(STACK_SETTLE);
    timeline::look(ctx).map_err(|(_, why)| super::uncertain(why))?;
    let after = opened(ctx, keys, place)?;
    if after.len() != shown || !after.get(place - 1).is_some_and(|f| f.header.stack) {
        return Err(super::uncertain(format!(
            "Logic Pro's {pressed:?} was pressed, but no stack {} is back at track {place}",
            quoted(&name)
        )));
    }
    Ok(Restored {
        restored: json!({"tracks": after.len(), "pressed": pressed, "ref": ref_text(ctx, "track", &[place])}),
        display: json!({"title": format!("Stack {} back", quoted(&name)), "note": "Put back with Logic Pro's own undo."}),
        undone_in_logic: true,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_stacks_kind_and_name_are_checked_and_the_factory_chords_stand_in_for_unbound_keys() {
        let keys = KeyCommands::embedded();
        assert_eq!(chord_of(&keys, CREATE_SUMMING).unwrap().spelled(), "cmd-shift-g");
        assert_eq!(chord_of(&keys, CREATE_FOLDER).unwrap().spelled(), "cmd-shift-h");
        assert_eq!(chord_of(&keys, FLATTEN).unwrap().spelled(), "cmd-shift-u");
        assert_eq!(chord_of(&keys, OPEN_STACK).unwrap().spelled(), "cmd-ctrl-right");
        assert_eq!(label(&[], &[1]), "");
    }
}
