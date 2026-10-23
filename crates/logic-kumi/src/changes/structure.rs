//! Adding and deleting a track, through the Track menu with Logic in front: "New Audio Track" (or the software
//! instrument's, or the external MIDI's) makes one below the selected track, which is selected first with a click on
//! its header; "Delete Track" takes the selected track away, after the click that selects it and a check that it is
//! the one. Logic's history names them "Create Track" and "Delete Tracks"; an added track is taken back by value (its
//! own deletion), a deleted one only by Logic's own undo while the Edit menu still names Kumi's deletion first.

use std::time::Duration;

use serde_json::{json, Value};

use super::nudge;
use super::undo::Probe;
use super::{changed, front, gone, quoted, ref_text, timeline, tracks, Applied, Ctx, Done, Epoch, Preview, Restore, Restored, Watch};
use crate::ax::{Node, Root};
use crate::refs::{bad, failed};

/// How long a header click gets before the selection is read back, and a menu item before the headers are.
const SELECT_SETTLE: Duration = Duration::from_millis(300);
const MENU_SETTLE: Duration = Duration::from_millis(500);

/// The kinds of track Logic's Track menu makes, and the menu item for each.
const KINDS: [(&str, &str); 3] =
    [("audio", "New Audio Track"), ("instrument", "New Software Instrument Track"), ("external", "New External MIDI Track")];

/// Presses a menu item under the Track menu by its English title, or by Logic's translation of it.
pub(crate) fn track_menu(ctx: &mut Ctx, item: &str) -> Done<String> {
    menu_item(ctx, "Track", &[item])
}

/// Presses an item under the File menu (a submenu's item as `["Import", "MIDI File…"]`).
pub(crate) fn file_menu(ctx: &mut Ctx, items: &[&str]) -> Done<String> {
    menu_item(ctx, "File", items)
}

/// Presses an item under a menu bar menu by its English titles, or by Logic's translations of them (each title's
/// candidates tried in turn, the menu's first).
pub(crate) fn menu_item(ctx: &mut Ctx, menu: &str, items: &[&str]) -> Done<String> {
    let mut menus = vec![menu.to_string()];
    menus.extend(ctx.project.strings.candidates(menu));
    let candidates: Vec<Vec<String>> = items
        .iter()
        .map(|item| {
            let mut titles = vec![item.to_string()];
            titles.extend(ctx.project.strings.candidates(item));
            titles
        })
        .collect();
    let mut paths: Vec<Vec<String>> = menus.into_iter().map(|menu| vec![menu]).collect();
    for titles in &candidates {
        paths = paths
            .iter()
            .flat_map(|path| titles.iter().map(move |title| path.iter().cloned().chain(std::iter::once(title.clone())).collect::<Vec<_>>()))
            .collect();
    }
    let mut last = None;
    for path in &paths {
        let titles: Vec<&str> = path.iter().map(String::as_str).collect();
        match ctx.ax.menu(&titles) {
            Ok(shown) => return Ok(shown),
            Err(error) => last = Some(error),
        }
    }
    Err(last
        .map_or_else(|| failed(format!("Logic Pro's {menu} menu has no {}", items.join(" > "))), |error| super::ax_fault(&error, false)))
}

/// Selects a track by a click on its header (Logic in front first) and checks that it is the selected one.
pub(super) fn select(ctx: &mut Ctx, place: usize) -> Done<Vec<tracks::Found>> {
    front::ensure(ctx.ax)?;
    let all = nudge::read(tracks::headers(ctx))?;
    if all.get(place - 1).is_some_and(|f| f.header.selected) && all.iter().filter(|f| f.header.selected).count() == 1 {
        return Ok(all);
    }
    nudge::read(ctx.project.select_track(ctx.ax, place))?;
    super::pause(SELECT_SETTLE);
    let all = nudge::read(tracks::headers(ctx))?;
    let selected: Vec<usize> = all.iter().filter(|f| f.header.selected).map(|f| f.place).collect();
    if selected != [place] {
        return Err(failed(format!("Logic Pro didn't select track {place} alone (selected: {selected:?}); nothing changed")));
    }
    Ok(all)
}

/// A sheet or dialog in front of the project window, if one is up.
fn dialog(ctx: &mut Ctx) -> Option<Node> {
    let node = ctx.ax.tree(&Root::Focused, "", 2, 40, Some(&["role", "subrole", "title", "desc", "value"])).ok()?;
    (node.role == "AXSheet" || node.subrole == "AXDialog" || node.role == "AXDialog").then_some(node)
}

/// Presses the button of a dialog whose title is `label` (in English or Logic's language): whether one was.
fn press_button(ctx: &mut Ctx, dialog: &Node, label: &str) -> Done<bool> {
    let strings = &ctx.project.strings;
    let found = dialog.find(&|n| n.role == "AXButton" && (strings.is(&n.title, label) || strings.starts_with(&n.title, label)));
    let Some(button) = found else { return Ok(false) };
    nudge::wrote(ctx.ax.action(&Root::Focused, &button.path, "AXPress", 300))?;
    Ok(true)
}

// ----- adding -----

struct Add {
    /// The track to select, after which the new one goes (its place from 1).
    after: usize,
    kind: &'static str,
    item: &'static str,
    name: Option<String>,
}

fn add_plan(ctx: &mut Ctx, input: &Value, count: usize) -> Done<Add> {
    let kind = input.get("kind").and_then(Value::as_str).unwrap_or("audio");
    let (kind, item) =
        KINDS.iter().find(|(name, _)| *name == kind).copied().ok_or_else(|| bad("kind must be audio, instrument or external"))?;
    let after = match input.get("index").filter(|v| !v.is_null()) {
        None => count,
        Some(index) => {
            let index = index
                .as_u64()
                .map(|i| i as usize)
                .filter(|i| *i <= count)
                .ok_or_else(|| bad(format!("index must be a whole number from 1 to {count}")))?;
            if index == 0 {
                return Err(bad(
                    "Logic Pro adds a track below the selected one, so index 0 (above every track) isn't reachable: use 1 or more",
                ));
            }
            index
        }
    };
    if after == 0 {
        return Err(failed("Logic Pro has no track to add one after: the project has none"));
    }
    let name = match input.get("name") {
        None | Some(Value::Null) => None,
        Some(Value::String(name)) if !name.trim().is_empty() && name.chars().count() <= 256 && !name.chars().any(char::is_control) => {
            Some(name.clone())
        }
        Some(_) => return Err(bad("name must be one line of at most 256 characters")),
    };
    let _ = ctx;
    Ok(Add { after, kind, item, name })
}

fn add_fence(names: &[String]) -> Value {
    json!({"tracks": names.len(), "names": names})
}

pub fn add_preview(ctx: &mut Ctx, input: &Value) -> Done<Preview> {
    let all = nudge::read(tracks::headers(ctx))?;
    let plan = add_plan(ctx, input, all.len())?;
    let names = tracks::names(&all);
    let title = match &plan.name {
        Some(name) => format!("Added {} track {}", plan.kind, quoted(name)),
        None => format!(
            "Added {} track",
            if plan.kind == "external" {
                "an external MIDI"
            } else if plan.kind == "audio" {
                "an audio"
            } else {
                "an instrument"
            }
        ),
    };
    Ok(Preview {
        target: json!({"ref": null, "kind": "track", "index": plan.after}),
        prior: json!({"tracks": all.len()}),
        fence: add_fence(&names),
        display: json!({"title": title, "index": plan.after}),
    })
}

pub fn add_apply(ctx: &mut Ctx, input: &Value, fence: &Value) -> Done<Applied> {
    let all = nudge::read(tracks::headers(ctx))?;
    let plan = add_plan(ctx, input, all.len())?;
    let before = tracks::names(&all);
    if add_fence(&before) != *fence {
        return Err(changed("the tracks changed since the preview"));
    }
    select(ctx, plan.after)?;
    track_menu(ctx, plan.item)?;
    super::pause(MENU_SETTLE);
    let after = nudge::wrote(tracks::headers(ctx))?;
    let place = plan.after + 1;
    if after.len() != before.len() + 1 || after.get(place - 1).is_none() {
        return Err(super::uncertain(format!(
            "Logic Pro shows {} tracks after Kumi asked for a new one below track {}",
            after.len(),
            plan.after
        )));
    }
    let mut name = after[place - 1].header.name.clone();
    if let Some(wanted) = &plan.name {
        let shown = tracks::rename(ctx, place, wanted)?;
        if shown != *wanted {
            return Err(nudge::kept("the new track's name", quoted(wanted), quoted(&shown)));
        }
        name = shown;
    }
    // The reader sees the new header (and its name): the epoch moves as the observers move it, once.
    timeline::look(ctx).map_err(|(_, why)| super::uncertain(why))?;
    let names: Vec<String> = nudge::wrote(tracks::headers(ctx))?.iter().map(|f| f.header.name.clone()).collect();
    Ok(Applied {
        applied: json!({"index": place, "name": name, "kind": plan.kind}),
        created: Some(json!({"ref": ref_text(ctx, "track", &[place]), "kind": "track"})),
        target: None,
        material: None,
        display: json!({"title": format!("Added {} track {}", plan.kind, quoted(&name))}),
        step: Some("Create Track"),
        probe: Some(Probe::Headers { prior: before, applied: names }),
    })
}

/// Deletes the track at a place: selects it, presses Delete Track, answers a confirmation that asks about its
/// regions, and reads the headers back.
fn delete_track(ctx: &mut Ctx, place: usize, name: &str) -> Done<Vec<tracks::Found>> {
    let all = select(ctx, place)?;
    if all[place - 1].header.name != name {
        return Err(changed(format!("track {place} is {} now, not {}", quoted(&all[place - 1].header.name), quoted(name))));
    }
    track_menu(ctx, "Delete Track")?;
    super::pause(MENU_SETTLE);
    if let Some(sheet) = dialog(ctx) {
        if !press_button(ctx, &sheet, "Delete")? {
            let _ = press_button(ctx, &sheet, "Cancel");
            return Err(failed(format!("Logic Pro asked something Kumi didn't expect ({}); nothing changed", sheet.title)));
        }
        super::pause(MENU_SETTLE);
    }
    timeline::look(ctx).map_err(|(_, why)| super::uncertain(why))?;
    nudge::wrote(tracks::headers(ctx))
}

pub fn add_restore(ctx: &mut Ctx, restore: &Restore) -> Done<Restored> {
    let created = restore.created.ok_or_else(|| bad("created: the track that was added"))?;
    let place = super::track_place(ctx, created, "ref", Epoch::Any)?;
    let all = nudge::read(tracks::headers(ctx))?;
    let name = restore.applied["name"].as_str().unwrap_or_default().to_string();
    let Some(found) = all.get(place - 1).filter(|f| f.header.name == name) else {
        return Err(gone(format!("the track Kumi added ({}) isn't at place {place} any more", quoted(&name))));
    };
    let label = found.label();
    if !restore.discard {
        let regions = nudge::read(ctx.project.read_regions(ctx.ax))?;
        let held = regions.iter().filter(|(track, _)| *track == place).count();
        if held > 0 {
            return Err(changed(format!("the track Kumi added ({}) holds {held} region(s) now", quoted(&label))));
        }
    }
    let before = all.len();
    let after = delete_track(ctx, place, &name)?;
    if after.len() != before - 1 {
        return Err(super::uncertain(format!("Logic Pro shows {} tracks after Kumi asked to delete {}", after.len(), quoted(&label))));
    }
    Ok(Restored::by_value(json!({"deleted": created["ref"]}), json!({"title": format!("{} taken away again", quoted(&label))})))
}

// ----- deleting -----

fn delete_fence(found: &tracks::Found, regions: usize) -> Value {
    json!({"name": found.header.name, "index": found.place, "regions": regions})
}

fn regions_of(ctx: &mut Ctx, place: usize) -> Done<usize> {
    let regions = nudge::read(ctx.project.read_regions(ctx.ax))?;
    Ok(regions.iter().filter(|(track, _)| *track == place).count())
}

pub fn delete_preview(ctx: &mut Ctx, input: &Value) -> Done<Preview> {
    let found = tracks::find(ctx, input, "ref", Epoch::Current)?;
    let regions = regions_of(ctx, found.place)?;
    let now = delete_fence(&found, regions);
    let mut display = json!({"title": format!("Deleted track {}", quoted(&found.label()))});
    if regions > 0 {
        display["note"] = format!("It holds {regions} region(s); only Logic Pro's own undo brings a deleted track back.").into();
    }
    Ok(Preview { target: json!({"ref": ref_text(ctx, "track", &[found.place]), "kind": "track"}), prior: now.clone(), fence: now, display })
}

pub fn delete_apply(ctx: &mut Ctx, input: &Value, fence: &Value, desc: &str) -> Done<Applied> {
    let found = tracks::find(ctx, input, "ref", Epoch::Current).map_err(|f| if f.0 == "failed" { changed(f.1) } else { f })?;
    let regions = regions_of(ctx, found.place)?;
    if delete_fence(&found, regions) != *fence {
        return Err(changed(format!("{} changed since the preview", found.label())));
    }
    let all = nudge::read(tracks::headers(ctx))?;
    let before: Vec<String> = all.iter().map(|f| f.header.name.clone()).collect();
    // What the track was, for HISTORY: its kind from the Mixer's strips (read when not cached), the rest from its header.
    let headers: Vec<crate::rows::Header> = all.iter().map(|f| f.header.clone()).collect();
    let kind = ctx.project.strips(ctx.ax, &headers).ok().and_then(|strips| strips.get(found.place - 1).map(|s| s.kind.to_string()));
    let label = found.label();
    let after = delete_track(ctx, found.place, &found.header.name)?;
    if after.len() != before.len() - 1 {
        return Err(super::uncertain(format!("Logic Pro shows {} tracks after Kumi asked to delete {}", after.len(), quoted(&label))));
    }
    let names: Vec<String> = after.iter().map(|f| f.header.name.clone()).collect();
    let header = &found.header;
    Ok(Applied {
        applied: json!({"deleted": true}),
        created: None,
        target: None,
        material: Some(json!({
            "name": header.name, "kind": kind, "index": found.place, "regions": regions,
            "volumeDb": header.volume_db.unwrap_or(0.0), "pan": (header.pan as f64 / crate::rows::PAN_CENTRE as f64 * 1000.0).round() / 1000.0,
            "mute": header.mute, "solo": header.solo, "arm": header.armed, "step": desc,
        })),
        display: json!({"title": format!("Deleted track {}", quoted(&label))}),
        step: Some("Delete Tracks"),
        probe: Some(Probe::Headers { prior: before, applied: names }),
    })
}

/// Logic's own undo of Kumi's step, through the Edit menu: only while the validated Undo title still names it and the
/// watch holds the change (by its key, and by the step's own `desc` when the journal kept it) as Kumi's latest step.
/// The title pressed.
pub(crate) fn undo_in_logic(ctx: &mut Ctx, watch: &mut Watch, key: &str, step: &str, desc: Option<&str>) -> Done<String> {
    // In front first: the titles are validated (by a key) only there, and a stale title could name another step.
    front::ensure(ctx.ax)?;
    let (titles, verified) = watch.fresh_titles(ctx)?;
    if !verified {
        return Err(failed(
            "Logic Pro went behind before its Undo title could be checked, so Kumi can't put this back now; ask again with Logic Pro in front",
        ));
    }
    let names_it = titles.undo_action(&ctx.project.strings).as_deref() == Some(step);
    let ours = watch.top_is(key) && desc.is_none_or(|desc| watch.top_desc().as_deref() == Some(desc));
    if !names_it || !ours {
        return Err(failed(format!(
            "Logic Pro's undo has other steps on top of this one (its Edit menu says {:?}), so Kumi can't put it back; Cmd-Z in Logic Pro can, step by step",
            titles.undo
        )));
    }
    let mut menus = vec!["Edit".to_string()];
    menus.extend(ctx.project.strings.candidates("Edit"));
    let mut pressed = None;
    for menu in &menus {
        if let Ok(shown) = ctx.ax.menu(&[menu.as_str(), titles.undo.as_str()]) {
            pressed = Some(shown);
            break;
        }
    }
    let Some(pressed) = pressed else { return Err(failed(format!("Logic Pro's Edit menu wouldn't press {:?}", titles.undo))) };
    super::pause(MENU_SETTLE);
    watch.taken_back(key);
    Ok(pressed)
}

pub fn delete_restore(ctx: &mut Ctx, watch: &mut Watch, restore: &Restore) -> Done<Restored> {
    let material = restore.material.ok_or_else(|| bad("material: what the delete removed"))?;
    let name = material["name"].as_str().unwrap_or_default().to_string();
    let place = material["index"].as_u64().unwrap_or(0) as usize;
    let all = nudge::read(tracks::headers(ctx))?;
    if all.get(place.wrapping_sub(1)).is_some_and(|f| f.header.name == name) {
        return Ok(Restored {
            restored: json!({"ref": ref_text(ctx, "track", &[place])}),
            display: json!({"title": format!("{} is back already", quoted(&name))}),
            undone_in_logic: false,
        });
    }
    let pressed = undo_in_logic(ctx, watch, &restore.key, "Delete Tracks", material["step"].as_str())?;
    timeline::look(ctx).map_err(|(_, why)| super::uncertain(why))?;
    let after = nudge::wrote(tracks::headers(ctx))?;
    if !after.get(place.wrapping_sub(1)).is_some_and(|f| f.header.name == name) {
        return Err(super::uncertain(format!("Logic Pro's {pressed:?} was pressed, but {} isn't back at place {place}", quoted(&name))));
    }
    Ok(Restored {
        restored: json!({"ref": ref_text(ctx, "track", &[place]), "was": restore.target["ref"], "pressed": pressed}),
        display: json!({"title": format!("{} put back", quoted(&name)), "note": "Put back with Logic Pro's own undo."}),
        undone_in_logic: true,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_add_goes_below_a_track_and_names_its_kind() {
        let strings = crate::ax::strings::Strings::english();
        let mut project = crate::project::Project::new(strings);
        let hands = crate::helper::MockHands::new();
        let mut launcher = hands.clone();
        let mut helper = crate::helper::Launch::launch(&mut launcher).unwrap();
        let mut ax = crate::ax::Ax::new(helper.as_mut(), 7);
        let mut ctx = Ctx { project: &mut project, ax: &mut ax };
        let plan = add_plan(&mut ctx, &json!({"name": "Bass", "kind": "instrument", "index": 1}), 2).unwrap();
        assert_eq!(
            (plan.after, plan.kind, plan.item, plan.name.as_deref()),
            (1, "instrument", "New Software Instrument Track", Some("Bass"))
        );
        assert_eq!(add_plan(&mut ctx, &json!({}), 2).unwrap().after, 2, "the end by default");
        assert!(add_plan(&mut ctx, &json!({"index": 0}), 2).is_err());
        assert!(add_plan(&mut ctx, &json!({"index": 3}), 2).is_err());
        assert!(add_plan(&mut ctx, &json!({"kind": "drummer"}), 2).is_err());
        assert!(add_plan(&mut ctx, &json!({}), 0).is_err());
        assert_eq!(add_fence(&["Vocal".into(), "Keys".into()]), json!({"tracks": 2, "names": ["Vocal", "Keys"]}));
    }
}
