//! Plug-ins put on a strip and taken off it, through the Mixer's insert slots (the pane opened for the change and
//! closed after), whose menus Logic draws itself ([`super::menus`]): `fx-add` presses the empty slot under the
//! strip's last plug-in, or the insert bar along the top of the plug-in the new one goes before, types the plug-in's
//! name into the menu's search, and takes its first result (Down, Return); `fx-delete` presses the plug-in's list
//! button and takes No Plug-in, the menu's second item (Down, Down, Return). The strip's slots are read back, and the
//! plug-in Kumi asked for must be the one there. Logic opens a plug-in's window when it inserts one: the bridge
//! closes it again. Logic's history names the steps "Insert Plug-in in Channel Strip" (an empty slot), "Change
//! Plug-in in Channel Strip" (an insert bar) and "Remove Plug-in in Channel Strip". A strip is a track's or an
//! output's (the Stereo Out, `<epoch>:output:1`), see [`super::strips`]. `fx-move` isn't made: Logic moves a plug-in
//! only by dragging it.

use std::time::Duration;

use serde_json::{json, Value};

use super::menus::{self, Key};
use super::nudge::{self, Control};
use super::strips::{self, Inserts, Owner};
use super::undo::{Probe, StripPart};
use super::{changed, gone, quoted, Applied, Ctx, Done, Epoch, Preview, Restore, Restored, Watch};
use crate::ax::{paths, Node};
use crate::refs::{bad, failed};

/// The longest plug-in name Kumi types.
const NAME_LIMIT: usize = 120;
/// How long Logic gets to search its plug-ins for the typed name, to highlight the first result (which opens its
/// channel formats beside it), and to insert the one chosen.
const SEARCH_SETTLE: Duration = Duration::from_millis(500);
const HIGHLIGHT_SETTLE: Duration = Duration::from_millis(500);
const INSERT_SETTLE: Duration = Duration::from_millis(1000);
/// The keys that take No Plug-in, the second item of an occupied slot's menu.
const NO_PLUGIN: [Key; 3] = [Key::Named("down"), Key::Named("down"), Key::Named("return")];
/// How often and how long a plug-in window closed is waited for, and the Edit menu after it.
const CLOSE_POLLS: usize = 6;
const CLOSE_POLL: Duration = Duration::from_millis(150);
const CLOSE_SETTLE: Duration = Duration::from_millis(250);
/// Logic's names for the steps.
const INSERTED: &str = "Insert Plug-in in Channel Strip";
const CHANGED: &str = "Change Plug-in in Channel Strip";
const REMOVED: &str = "Remove Plug-in in Channel Strip";

/// The plug-in a request names: the name Logic's slot will show (the last part of a category path such as
/// "Dynamics/Compressor"), which is also what the menu's search is given.
pub fn plugin_name(input: &Value) -> Done<String> {
    let name =
        input.get("name").and_then(Value::as_str).map(str::trim).filter(|n| !n.is_empty()).ok_or_else(|| {
            bad("name must be the plug-in's name as Logic Pro's plug-in menu shows it, such as \"Channel EQ\" or \"Gain\"")
        })?;
    if name.chars().count() > NAME_LIMIT || name.chars().any(char::is_control) {
        return Err(bad(format!("name must be one line of at most {NAME_LIMIT} characters")));
    }
    let leaf = name.rsplit('/').map(str::trim).find(|part| !part.is_empty()).unwrap_or(name);
    Ok(leaf.to_string())
}

/// Where a new plug-in goes among the strip's effects (from 0; the end when not given), checked against how many there
/// are.
fn index_of(input: &Value, count: usize) -> Done<usize> {
    match input.get("index") {
        None | Some(Value::Null) => Ok(count),
        Some(value) => value
            .as_u64()
            .map(|i| i as usize)
            .filter(|i| *i <= count)
            .ok_or_else(|| bad(format!("index must be a place among the strip's {count} effect plug-in(s), from 0 to {count}"))),
    }
}

/// What an add fences: the strip's effects from the top, and its instrument.
fn fence_of(inserts: &Inserts) -> Value {
    let instrument = inserts.instrument.as_ref().filter(|slot| paths::occupied(slot)).map(|slot| slot.desc.clone());
    json!({"plugins": inserts.names(), "instrument": instrument})
}

/// The strip's label and inserts, read in the open Mixer.
fn strip_now(ctx: &mut Ctx, owner: Owner, strip: &Node) -> Done<(String, Inserts)> {
    let label = strips::label(ctx, owner, Some(strip))?;
    Ok((label, Inserts::of(strip, &ctx.project.strings)))
}

fn owner_target(ctx: &Ctx, owner: Owner) -> Value {
    let kind = match owner {
        Owner::Track(_) => "track",
        Owner::Output(_) => "output",
    };
    json!({"ref": owner.text(ctx.project.epoch), "kind": kind})
}

/// The slot number the rows give the effect at `index` (the instrument counts first).
fn row_slot(inserts: &Inserts, index: usize) -> usize {
    let instrument = usize::from(inserts.instrument.as_ref().is_some_and(|slot| paths::occupied(slot)));
    instrument + index + 1
}

/// The control that puts a plug-in at `index` among the effects, and the step Logic will name: the empty slot under
/// the last effect, or the insert bar along the top of the effect now at `index`.
fn insert_point(ctx: &Ctx, inserts: &Inserts, index: usize) -> Done<(Control, &'static str)> {
    let occupied = inserts.occupied();
    let (node, step) = if index == occupied.len() {
        let after = occupied.last().map_or(0, |(at, _)| at + 1);
        (inserts.effects.iter().skip(after).find(|slot| !paths::occupied(slot)), INSERTED)
    } else {
        let (place, _) = occupied.get(index).copied().ok_or_else(|| bad("no such place among the effects"))?;
        (inserts.bars.iter().find(|(above, _)| *above == place).map(|(_, bar)| bar), CHANGED)
    };
    let node = node.ok_or_else(|| failed("Logic Pro's strip shows no free slot there (its slots are all taken)"))?;
    Ok((Control::new(&ctx.project.root, &node.path), step))
}

/// The windows Logic shows now, by title.
fn window_titles(ctx: &mut Ctx) -> Done<Vec<String>> {
    let reply = nudge::read(ctx.ax.windows())?;
    let (windows, _) = crate::logic::parse_windows(&reply);
    Ok(windows.into_iter().map(|window| window.title).collect())
}

/// Closes the windows that weren't up before (the plug-in window Logic opens on an insert), by their close buttons,
/// and waits for them to go: while one is up, Logic's Edit menu speaks for its window ("Undo"), not the project.
pub(crate) fn close_new_windows(ctx: &mut Ctx, before: &[String]) {
    let Ok(now) = window_titles(ctx) else { return };
    let new: Vec<String> = now.into_iter().filter(|title| !title.is_empty() && !before.contains(title)).collect();
    for title in &new {
        let root = crate::ax::Root::Title(title.clone());
        let Ok(window) = ctx.ax.tree(&root, "", 1, 12, Some(&["role", "subrole", "title", "desc"])) else { continue };
        if window.title != *title {
            continue;
        }
        // A plug-in window's close button is a plain button named "close"; a standard window's has its subrole.
        let strings = &ctx.project.strings;
        if let Some(close) =
            window.children.iter().find(|c| c.role == "AXButton" && (c.subrole == "AXCloseButton" || strings.is(&c.desc, "close")))
        {
            let _ = ctx.ax.action(&root, &close.path, "AXPress", 150);
        }
    }
    if new.is_empty() {
        return;
    }
    for _ in 0..CLOSE_POLLS {
        super::pause(CLOSE_POLL);
        match window_titles(ctx) {
            Ok(now) if !now.iter().any(|title| new.contains(title)) => break,
            Ok(_) => {}
            Err(_) => break,
        }
    }
    super::pause(CLOSE_SETTLE);
}

/// Takes the plug-in named from the menu a slot or bar opens: its name typed into the menu's search, its first result
/// highlighted (Down) and taken (Return).
fn choose(ctx: &mut Ctx, point: &Control, name: &str) -> Done<()> {
    let keys = menus::typed(name);
    if keys.is_empty() {
        return Err(bad(format!("{} doesn't start with anything Kumi can type into Logic Pro's plug-in search", quoted(name))));
    }
    menus::open(ctx, point)?;
    let chosen = (|| {
        menus::keys(ctx, &keys)?;
        super::pause(SEARCH_SETTLE);
        menus::keys(ctx, &[Key::Named("down")])?;
        super::pause(HIGHLIGHT_SETTLE);
        menus::keys(ctx, &[Key::Named("return")])
    })();
    if let Err(fault) = chosen {
        menus::close(ctx);
        menus::close(ctx);
        return Err(fault);
    }
    super::pause(INSERT_SETTLE);
    Ok(())
}

/// The undo watch's probe of a track's effects, before and after (the Inspector shows the selected track's strip; an
/// output's has none).
fn strip_probe(owner: Owner, prior: Vec<String>, applied: Vec<String>) -> Option<Probe> {
    match owner {
        Owner::Track(place) => Some(Probe::Strip { place, part: StripPart::Plugins, prior, applied }),
        Owner::Output(_) => None,
    }
}

/// Whether two plug-in names are the same as Logic shows them (the search is given any case).
fn same_name(a: &str, b: &str) -> bool {
    a.trim().eq_ignore_ascii_case(b.trim())
}

fn same_names(a: &[String], b: &[String]) -> bool {
    a.len() == b.len() && a.iter().zip(b).all(|(a, b)| same_name(a, b))
}

pub fn add_preview(ctx: &mut Ctx, input: &Value) -> Done<Preview> {
    let owner = strips::owner(ctx, input, "ref", Epoch::Current)?;
    let name = plugin_name(input)?;
    let (label, inserts) = strips::with_strip(ctx, owner, |ctx, strip| strip_now(ctx, owner, strip))?;
    let index = index_of(input, inserts.names().len())?;
    let fence = fence_of(&inserts);
    Ok(Preview {
        target: owner_target(ctx, owner),
        prior: fence.clone(),
        fence,
        display: json!({"title": format!("Added {} to {}", quoted(&name), quoted(&label)), "index": index}),
    })
}

/// What an add did, as read back in the Mixer.
struct Added {
    label: String,
    index: usize,
    shown: String,
    after: Inserts,
    step: &'static str,
}

pub fn add_apply(ctx: &mut Ctx, input: &Value, fence: &Value) -> Done<Applied> {
    let owner = strips::owner(ctx, input, "ref", Epoch::Current).map_err(strips::went)?;
    let name = plugin_name(input)?;
    super::front::ensure(ctx.ax)?;
    let windows_before = window_titles(ctx)?;
    let added = strips::with_strip(ctx, owner, |ctx, strip| {
        let (label, inserts) = strip_now(ctx, owner, strip)?;
        if fence_of(&inserts) != *fence {
            return Err(changed(format!("the plug-ins on {} changed since the preview", quoted(&label))));
        }
        let index = index_of(input, inserts.names().len())?;
        let (point, step) = insert_point(ctx, &inserts, index)?;
        choose(ctx, &point, &name)?;
        let strip = strips::strip_of(ctx, owner).map_err(|(_, why)| super::uncertain(why))?;
        let after = Inserts::of(&strip, &ctx.project.strings);
        let mut expected = inserts.names();
        expected.insert(index, name.clone());
        let shown = after.names();
        if same_names(&shown, &expected) {
            return Ok(Added { label, index, shown: shown[index].clone(), after, step });
        }
        if shown.len() == expected.len() && !same_name(&shown[index], &name) {
            // Logic's search took another plug-in: off it comes again, and nothing is left changed.
            let other = shown[index].clone();
            if let Some((node, _)) = after.occupied().get(index).map(|(_, node)| (*node, ())) {
                let node = node.clone();
                let _ = take_off(ctx, owner, &node);
            }
            return Err(failed(format!(
                "Logic Pro's plug-in search for {} took {}, so Kumi took it off again: give the name as Logic Pro's plug-in menu shows it",
                quoted(&name),
                quoted(&other)
            )));
        }
        Err(super::uncertain(format!(
            "Logic Pro's strip {} shows {shown:?} after Kumi asked for {} at place {}",
            quoted(&label),
            quoted(&name),
            index + 1
        )))
    });
    close_new_windows(ctx, &windows_before);
    let added = added?;
    let slot = row_slot(&added.after, added.index);
    let prior: Vec<String> = fence["plugins"].as_array().into_iter().flatten().filter_map(Value::as_str).map(str::to_string).collect();
    Ok(Applied {
        applied: json!({"index": added.index, "name": added.shown, "slot": slot, "plugins": added.after.names()}),
        created: Some(json!({"ref": owner.plugin(ctx.project.epoch, slot), "kind": "plugin"})),
        display: json!({"title": format!("Added {} to {}", quoted(&added.shown), quoted(&added.label)), "index": added.index}),
        step: Some(added.step),
        probe: strip_probe(owner, prior, added.after.names()),
        ..Applied::default()
    })
}

/// Takes the plug-in in a slot off its strip: its list button pressed, No Plug-in taken. The strip's inserts after.
fn take_off(ctx: &mut Ctx, owner: Owner, slot: &Node) -> Done<Inserts> {
    let strings = &ctx.project.strings;
    let list = slot.children.iter().find(|c| c.role == "AXButton" && strings.is(&c.desc, "list"));
    let point = Control::new(&ctx.project.root, list.map_or(slot.path.as_str(), |button| button.path.as_str()));
    menus::open(ctx, &point)?;
    if let Err(fault) = menus::keys(ctx, &NO_PLUGIN) {
        menus::close(ctx);
        return Err(fault);
    }
    super::pause(INSERT_SETTLE);
    let strip = strips::strip_of(ctx, owner).map_err(|(_, why)| super::uncertain(why))?;
    Ok(Inserts::of(&strip, &ctx.project.strings))
}

/// The effect a restore means: at its slot when it still has its name there, else the one effect of that name.
fn effect_named<'a>(inserts: &'a Inserts, slot: usize, name: &str) -> Option<&'a Node> {
    if let Some((node, false)) = strips::plugin_at(inserts, slot) {
        if same_name(&node.desc, name) {
            return Some(node);
        }
    }
    let named: Vec<&Node> = inserts.occupied().into_iter().map(|(_, node)| node).filter(|node| same_name(&node.desc, name)).collect();
    (named.len() == 1).then(|| named[0])
}

pub fn add_restore(ctx: &mut Ctx, restore: &Restore) -> Done<Restored> {
    let created = restore.created.ok_or_else(|| bad("created: the plug-in that was added"))?;
    let (owner, slot) = strips::plugin(ctx, created, "ref", Epoch::Any).map_err(|f| if f.0 == "failed" { gone(f.1) } else { f })?;
    let name = restore.applied["name"].as_str().unwrap_or_default().to_string();
    let (label, outcome) = strips::with_strip(ctx, owner, |ctx, strip| {
        let (label, inserts) = strip_now(ctx, owner, strip)?;
        if !inserts.names().iter().any(|n| same_name(n, &name)) {
            return Ok((label, None));
        }
        let Some(node) = effect_named(&inserts, slot, &name).cloned() else {
            return Err(changed(format!(
                "the plug-ins on {} have been rearranged in Logic Pro since Kumi added {}",
                quoted(&label),
                quoted(&name)
            )));
        };
        let after = take_off(ctx, owner, &node)?;
        Ok((label, Some((inserts.names().len(), after))))
    })?;
    let Some((count, after)) = outcome else {
        return Ok(Restored::by_value(
            json!({"deleted": created["ref"]}),
            json!({"title": format!("{} is off {} already", quoted(&name), quoted(&label))}),
        ));
    };
    if after.names().len() + 1 != count {
        return Err(super::uncertain(format!("Logic Pro's strip {} still shows {} after Kumi took it off", quoted(&label), quoted(&name))));
    }
    Ok(Restored::by_value(
        json!({"deleted": created["ref"], "plugins": after.names()}),
        json!({"title": format!("{} taken off {}", quoted(&name), quoted(&label))}),
    ))
}

// ----- delete -----

pub fn delete_preview(ctx: &mut Ctx, input: &Value) -> Done<Preview> {
    let (owner, slot) = strips::plugin(ctx, input, "ref", Epoch::Current)?;
    let (label, name, fence) = strips::with_strip(ctx, owner, |ctx, strip| {
        let (label, inserts) = strip_now(ctx, owner, strip)?;
        let (node, instrument) =
            strips::plugin_at(&inserts, slot).ok_or_else(|| crate::refs::not_in_project(&owner.plugin(ctx.project.epoch, slot)))?;
        if instrument {
            return Err(bad(format!(
                "{} is the instrument of {}: Kumi takes effects off a strip, not its instrument (load another patch instead)",
                quoted(&node.desc),
                quoted(&label)
            )));
        }
        Ok((label, node.desc.clone(), fence_of(&inserts)))
    })?;
    Ok(Preview {
        target: strips::plugin_target(ctx.project.epoch, owner, slot),
        prior: json!({"name": name, "slot": slot}),
        fence,
        display: json!({"title": format!("Deleted {} from {}", quoted(&name), quoted(&label)), "note": "Logic Pro's own undo puts it back with its settings while the deletion is its latest step; after that Kumi puts it back by name, with Logic Pro's default settings."}),
    })
}

pub fn delete_apply(ctx: &mut Ctx, input: &Value, fence: &Value, desc: &str) -> Done<Applied> {
    let (owner, slot) = strips::plugin(ctx, input, "ref", Epoch::Current).map_err(strips::went)?;
    super::front::ensure(ctx.ax)?;
    let (label, name, index, before, after) = strips::with_strip(ctx, owner, |ctx, strip| {
        let (label, inserts) = strip_now(ctx, owner, strip)?;
        if fence_of(&inserts) != *fence {
            return Err(changed(format!("the plug-ins on {} changed since the preview", quoted(&label))));
        }
        let (node, _) = strips::plugin_at(&inserts, slot).ok_or_else(|| changed("the plug-in went since the preview"))?;
        let (node, name) = (node.clone(), node.desc.clone());
        let index = inserts.occupied().iter().position(|(_, slot)| slot.path == node.path).unwrap_or(0);
        let after = take_off(ctx, owner, &node)?;
        Ok((label, name, index, inserts, after))
    })?;
    let mut expected = before.names();
    expected.remove(index);
    if !same_names(&after.names(), &expected) {
        return Err(super::uncertain(format!(
            "Logic Pro's strip {} shows {:?} after Kumi took {} off",
            quoted(&label),
            after.names(),
            quoted(&name)
        )));
    }
    Ok(Applied {
        applied: json!({"deleted": true, "plugins": after.names()}),
        material: Some(json!({"name": name, "index": index, "strip": owner.text(ctx.project.epoch), "step": desc})),
        display: json!({"title": format!("Deleted {} from {}", quoted(&name), quoted(&label))}),
        step: Some(REMOVED),
        probe: strip_probe(owner, before.names(), after.names()),
        ..Applied::default()
    })
}

pub fn delete_restore(ctx: &mut Ctx, watch: &mut Watch, restore: &Restore) -> Done<Restored> {
    let material = restore.material.ok_or_else(|| bad("material: what the delete took off"))?;
    let name = material["name"].as_str().unwrap_or_default().to_string();
    let index = material["index"].as_u64().unwrap_or(0) as usize;
    let (owner, _) = strips::plugin(ctx, restore.target, "ref", Epoch::Any).map_err(|f| if f.0 == "failed" { gone(f.1) } else { f })?;
    let (label, now) = strips::with_strip(ctx, owner, |ctx, strip| strip_now(ctx, owner, strip))?;
    if now.names().get(index).is_some_and(|shown| same_name(shown, &name)) {
        return Ok(Restored::by_value(
            json!({"ref": owner.plugin(ctx.project.epoch, row_slot(&now, index))}),
            json!({"title": format!("{} is back on {} already", quoted(&name), quoted(&label))}),
        ));
    }
    // Logic's own undo first, while the deletion is its latest step: it brings the plug-in back with its settings.
    if watch.top_is(&restore.key) {
        let pressed = super::structure::undo_in_logic(ctx, watch, &restore.key, REMOVED, material["step"].as_str())?;
        let (_, after) = strips::with_strip(ctx, owner, |ctx, strip| strip_now(ctx, owner, strip))?;
        if !after.names().get(index).is_some_and(|shown| same_name(shown, &name)) {
            return Err(super::uncertain(format!(
                "Logic Pro's {pressed:?} was pressed, but {} isn't back on {}",
                quoted(&name),
                quoted(&label)
            )));
        }
        return Ok(Restored {
            restored: json!({"ref": owner.plugin(ctx.project.epoch, row_slot(&after, index)), "pressed": pressed}),
            display: json!({"title": format!("{} put back on {}", quoted(&name), quoted(&label)), "note": "Put back with Logic Pro's own undo, with its settings."}),
            undone_in_logic: true,
        });
    }
    super::front::ensure(ctx.ax)?;
    let windows_before = window_titles(ctx)?;
    let outcome = strips::with_strip(ctx, owner, |ctx, strip| {
        let (_, inserts) = strip_now(ctx, owner, strip)?;
        let at = index.min(inserts.names().len());
        let (point, _) = insert_point(ctx, &inserts, at)?;
        choose(ctx, &point, &name)?;
        let strip = strips::strip_of(ctx, owner).map_err(|(_, why)| super::uncertain(why))?;
        Ok((Inserts::of(&strip, &ctx.project.strings), at))
    });
    close_new_windows(ctx, &windows_before);
    let (after, at) = outcome?;
    if !after.names().get(at).is_some_and(|shown| same_name(shown, &name)) {
        return Err(super::uncertain(format!(
            "Logic Pro's strip {} shows {:?} after Kumi put {} back",
            quoted(&label),
            after.names(),
            quoted(&name)
        )));
    }
    Ok(Restored::by_value(
        json!({"ref": owner.plugin(ctx.project.epoch, row_slot(&after, at)), "plugins": after.names()}),
        json!({"title": format!("{} put back on {}", quoted(&name), quoted(&label)), "note": "Put back by name, with Logic Pro's default settings: Logic Pro's own undo had other steps on top."}),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_plugin_is_named_as_its_slot_shows_it_and_placed_under_another() {
        assert_eq!(plugin_name(&json!({"name": "Gain"})).unwrap(), "Gain");
        assert_eq!(plugin_name(&json!({"name": "Dynamics/Compressor"})).unwrap(), "Compressor");
        assert!(plugin_name(&json!({"name": ""})).is_err());
        assert!(plugin_name(&json!({})).is_err());
        assert_eq!(index_of(&json!({}), 3).unwrap(), 3);
        assert_eq!(index_of(&json!({"index": 1}), 3).unwrap(), 1);
        assert_eq!(index_of(&json!({"index": 0}), 3).unwrap(), 0);
        assert_eq!(index_of(&json!({"index": 0}), 0).unwrap(), 0);
        assert!(index_of(&json!({"index": 4}), 3).is_err());
    }
}
