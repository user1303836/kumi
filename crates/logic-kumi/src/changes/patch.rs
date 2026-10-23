//! `patch-load`: a patch from Logic's Library loaded onto a track. The Library (Y, the control bar's Library button)
//! shows the patches for the selected track's kind; its search field takes a name by Accessibility, and while it
//! searches the Library lists the patches of that name as rows of a table (name, date), and a row selected by
//! Accessibility loads its patch onto the selected track, as a click would (measured on Logic Pro 12.3.1). So the
//! bridge selects the track, opens the Library, searches the patch's name, selects its row, clears the search and
//! closes the Library. Read back from the track's strip in the Inspector: its setting button names the patch, its
//! slots the plug-ins it brought. Logic's history names the step "Load Patch"; a restore is Logic's own undo while
//! that step is on top, and nothing by value (a patch replaces the whole strip).

use std::time::Duration;

use serde_json::{json, Value};

use super::{changed, nudge, panes, quoted, tracks, Applied, Ctx, Done, Epoch, Preview, Restore, Restored, Watch};
use crate::ax::paths::{Inspector, Strip};
use crate::ax::Node;
use crate::refs::{bad, failed};

/// Logic's name for the step.
const STEP: &str = "Load Patch";
/// How long the Library gets to search, and a patch to load.
const SEARCH_SETTLE: Duration = Duration::from_millis(700);
const LOAD_SETTLE: Duration = Duration::from_millis(1800);

/// The patch a request names: its path in the Library's columns ("Vocal/Classic Vocal") or its name alone.
fn patch_of(input: &Value) -> Done<(Vec<String>, String)> {
    let text = input
        .get("patch")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .ok_or_else(|| bad("patch: the patch's name as Logic Pro's Library shows it, or its path there (\"Vocal/Classic Vocal\")"))?;
    if text.chars().any(char::is_control) || text.chars().count() > 300 {
        return Err(bad("patch must be one line of at most 300 characters"));
    }
    let parts: Vec<String> = text.split('/').map(str::trim).filter(|p| !p.is_empty()).map(str::to_string).collect();
    let name = parts.last().cloned().ok_or_else(|| bad("patch must name a patch"))?;
    Ok((parts, name))
}

/// The selected track's strip as the Inspector shows it: its patch (the setting button's name) and its plug-ins.
pub(super) fn strip_now(ctx: &mut Ctx) -> Done<(String, Vec<String>)> {
    let inspector = nudge::read(ctx.project.inspector(ctx.ax))?;
    let parts = Inspector::of(&inspector, &ctx.project.strings);
    let strip = parts.strips.first().ok_or_else(|| failed("Logic Pro's Inspector shows no channel strip"))?;
    let strings = &ctx.project.strings;
    let setting = strip
        .children
        .iter()
        .find(|c| c.role == "AXButton" && strings.starts_with(&c.help, "Setting button"))
        .map(|c| if strings.is(&c.desc, "setting") { String::new() } else { c.desc.clone() })
        .unwrap_or_default();
    let found = Strip::of(strip, strings);
    let mut plugins: Vec<String> = found.instrument.filter(|s| crate::ax::paths::occupied(s)).map(|s| s.desc.clone()).into_iter().collect();
    plugins.extend(found.inserts.iter().rev().filter(|s| crate::ax::paths::occupied(s)).map(|s| s.desc.clone()));
    Ok((setting, plugins))
}

/// The Library's search field in its pane.
fn search_field(ctx: &mut Ctx, pane: &Node) -> Done<String> {
    let tree = nudge::read(ctx.ax.tree(&ctx.project.root.clone(), &pane.path, 8, 60, Some(&["role", "subrole", "desc"])))?;
    tree.find(&|n| n.role == "AXTextField" && n.subrole == "AXSearchField")
        .map(|n| n.path.clone())
        .ok_or_else(|| failed("Logic Pro's Library shows no search field"))
}

/// Types into the Library's search field (set and confirmed) and waits for its results.
fn search(ctx: &mut Ctx, field: &str, text: &str) -> Done<()> {
    let root = ctx.project.root.clone();
    nudge::wrote(ctx.ax.set(&root, field, "AXValue", json!(text), 150))?;
    let _ = ctx.ax.action(&root, field, "AXConfirm", 150);
    super::pause(SEARCH_SETTLE);
    Ok(())
}

/// The search's results: the rows of the Library's table, with the name in their first cell. The table sits six levels
/// down the pane, its names three more, deeper than one read goes: the table is found first, then read.
fn results(ctx: &mut Ctx, pane: &Node) -> Done<Vec<(String, String)>> {
    let root = ctx.project.root.clone();
    let tree = nudge::read(ctx.ax.tree(&root, &pane.path, 8, 256, Some(&["role"])))?;
    let path =
        tree.find(&|n| n.role == "AXTable").map(|t| t.path.clone()).ok_or_else(|| failed("Logic Pro's Library shows no search results"))?;
    let table = nudge::read(ctx.ax.tree(&root, &path, 3, 256, Some(&["role", "subrole", "value"])))?;
    Ok(table
        .children
        .iter()
        .filter(|c| c.role == "AXRow")
        .map(|row| {
            let name = row.find(&|n| n.role == "AXStaticText").map(Node::value_text).unwrap_or_default();
            (name, row.path.clone())
        })
        .collect())
}

/// Loads the patch through the Library onto the selected track: its name searched, its row selected, the search
/// cleared.
fn load(ctx: &mut Ctx, name: &str) -> Done<()> {
    panes::with_pane(ctx, "Library", |ctx, pane| {
        let field = search_field(ctx, pane)?;
        search(ctx, &field, name)?;
        let chosen = (|| {
            let rows = results(ctx, pane)?;
            let matching: Vec<&(String, String)> = rows.iter().filter(|(shown, _)| shown.eq_ignore_ascii_case(name)).collect();
            match matching.as_slice() {
                [] => Err(failed(format!("Logic Pro's Library finds no patch named {} for this track", quoted(name)))),
                [(_, path)] => {
                    let path = path.clone();
                    nudge::wrote(ctx.ax.set(&ctx.project.root.clone(), &path, "AXSelected", json!(true), 300))?;
                    Ok(())
                }
                many => {
                    Err(bad(format!("Logic Pro's Library has {} patches named {}: Kumi can't tell them apart", many.len(), quoted(name))))
                }
            }
        })();
        let _ = search(ctx, &field, "");
        chosen
    })
}

fn track_target(ctx: &Ctx, place: usize) -> Value {
    json!({"ref": super::ref_text(ctx, "track", &[place]), "kind": "track"})
}

pub fn preview(ctx: &mut Ctx, input: &Value) -> Done<Preview> {
    let found = tracks::find(ctx, input, "ref", Epoch::Current)?;
    let (_, name) = patch_of(input)?;
    let (setting, plugins) = panes::with_selected(ctx, found.place, strip_now)?;
    let now = json!({"patch": setting, "plugins": plugins});
    let mut display = json!({"title": format!("Loaded the patch {} on {}", quoted(&name), quoted(&found.label())), "before": now});
    display["note"] =
        "A patch replaces the track's channel strip; only Logic Pro's own undo brings the strip back, while the load is its latest step."
            .into();
    Ok(Preview { target: track_target(ctx, found.place), prior: now.clone(), fence: now, display })
}

pub fn apply(ctx: &mut Ctx, input: &Value, fence: &Value, desc: &str) -> Done<Applied> {
    let found = tracks::find(ctx, input, "ref", Epoch::Current).map_err(|f| if f.0 == "failed" { changed(f.1) } else { f })?;
    let (_, name) = patch_of(input)?;
    let label = found.label();
    let (before, after) = panes::with_selected(ctx, found.place, |ctx| {
        let (setting, plugins) = strip_now(ctx)?;
        let before = json!({"patch": setting, "plugins": plugins});
        if before != *fence {
            return Err(changed(format!("{}'s channel strip changed since the preview", quoted(&label))));
        }
        load(ctx, &name)?;
        super::pause(LOAD_SETTLE);
        let (setting, plugins) = strip_now(ctx).map_err(|(_, why)| super::uncertain(why))?;
        Ok((before, json!({"patch": setting, "plugins": plugins})))
    })?;
    if !after["patch"].as_str().unwrap_or_default().eq_ignore_ascii_case(&name) {
        return Err(super::uncertain(format!(
            "Logic Pro's strip {} shows the patch {} after Kumi loaded {}",
            quoted(&label),
            quoted(after["patch"].as_str().unwrap_or_default()),
            quoted(&name)
        )));
    }
    let mut material = before.clone();
    material["step"] = desc.into();
    Ok(Applied {
        applied: after.clone(),
        material: Some(material),
        display: json!({"title": format!("Loaded the patch {} on {}", quoted(&name), quoted(&label)), "before": before, "after": after}),
        step: Some(STEP),
        ..Applied::default()
    })
}

pub fn restore(ctx: &mut Ctx, watch: &mut Watch, restore: &Restore) -> Done<Restored> {
    let found = tracks::find(ctx, restore.target, "ref", Epoch::Any).map_err(|f| if f.0 == "failed" { super::gone(f.1) } else { f })?;
    let label = found.label();
    let (setting, plugins) = panes::with_selected(ctx, found.place, strip_now)?;
    let now = json!({"patch": setting, "plugins": plugins});
    if now["patch"] == restore.prior["patch"] && now["plugins"] == restore.prior["plugins"] {
        return Ok(Restored::by_value(now, json!({"title": format!("{}'s channel strip is as it was", quoted(&label))})));
    }
    if !restore.discard && now != *restore.applied {
        return Err(changed(format!("{}'s channel strip has been changed in Logic Pro since Kumi loaded the patch", quoted(&label))));
    }
    if !watch.top_is(&restore.key) {
        return Err(failed(
            "Logic Pro's undo has other steps on top of the patch load, so Kumi can't put the strip back (a patch replaces the whole strip): Cmd-Z in Logic Pro can, step by step",
        ));
    }
    let step_desc = restore.material.and_then(|m| m["step"].as_str());
    let pressed = super::structure::undo_in_logic(ctx, watch, &restore.key, STEP, step_desc)?;
    // Logic's undo put the whole strip back: what the bridge kept of the Mixer is stale.
    ctx.project.forget();
    let (setting, plugins) = panes::with_selected(ctx, found.place, strip_now)?;
    let after = json!({"patch": setting, "plugins": plugins});
    if after["plugins"] != restore.prior["plugins"] {
        return Err(super::uncertain(format!("Logic Pro's {pressed:?} was pressed, but {}'s strip isn't as it was", quoted(&label))));
    }
    Ok(Restored {
        restored: json!({"patch": after["patch"], "plugins": after["plugins"], "pressed": pressed}),
        display: json!({"title": format!("{}'s channel strip put back", quoted(&label)), "note": "Put back with Logic Pro's own undo."}),
        undone_in_logic: true,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_patch_is_named_alone_or_by_its_path_in_the_library() {
        assert_eq!(
            patch_of(&json!({"patch": "Vocal/Classic Vocal"})).unwrap(),
            (vec!["Vocal".to_string(), "Classic Vocal".to_string()], "Classic Vocal".to_string())
        );
        assert_eq!(patch_of(&json!({"patch": "Classic Vocal"})).unwrap().1, "Classic Vocal");
        assert!(patch_of(&json!({"patch": ""})).is_err());
        assert!(patch_of(&json!({})).is_err());
    }
}
