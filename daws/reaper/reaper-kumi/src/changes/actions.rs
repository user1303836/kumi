//! Any of REAPER's actions, by its command id or name (r-tracks, M4), and saving: the project, a new version of it, and
//! tracks as a track template.
//!
//! An action runs as a change: one named undo block around it, ended so that REAPER makes a point only when the action
//! marked a change of its own (7.82 lab: insert track and mute do, selection, transport and repeat don't), with the tracks
//! and items Kumi selects for it and the producer's selection put back after. Never an action that opens a dialog (its
//! name ends "…" or "..."), quits, opens, closes or switches projects, saves (save_project does) or renders. What the
//! action did is REAPER's to know: with a point it is a HISTORY row only REAPER's undo takes back (as run-lua's); without
//! one it changed nothing the project keeps, and there is no row.
//!
//! Saving writes files and changes nothing in the project, so it is no change and no row: a project with a file is saved
//! in place, or as its next version (REAPER's own action); one never saved only under a path inside the folder the
//! producer chose for projects (REAPER's default project path), and never over a file that is there. A track template is
//! written to REAPER's TrackTemplates folder (or a path), never over a file unless `replace` names it so.

use std::path::{Component, Path, PathBuf};

use serde_json::{json, Value};

use super::{Applied, Changes, Ctx, Done, Preview, Restore, Restored};
use crate::queue::Answer;
use crate::read::{bad, failed, Reads};
use crate::reaper::{Item, Reaper, Track};
use crate::refs::{self, Ref, TrackId};

const NOTE: &str = "Only REAPER's own undo can take this back (Edit > Undo).";
const NO_POINT: &str = "REAPER made no undo point of it: it changed nothing the project keeps (a view, selection or transport action), so it has no row in HISTORY.";
const HISTORY_MOVED: &str = "It moved REAPER's undo history itself (an undo or redo inside it): what it took back or made again shows in HISTORY once Kumi looks again. Only REAPER's own undo can take it back (Edit > Undo).";

/// Actions never run as a change, by id: quit; new, open, close and switch projects and tabs; save (save_project does);
/// render (a progress window holds REAPER's main thread); REAPER's undo and redo (they move REAPER's history under Kumi's
/// HISTORY: change_undo takes back Kumi's changes, and REAPER's own Edit > Undo is the producer's).
const REFUSED: [(i32, &str); 17] = [
    (40004, "quits REAPER"),
    (40029, "moves REAPER's undo history under Kumi's HISTORY: Kumi's own changes are taken back by Kumi's undo, anything else is the producer's to undo in REAPER"),
    (40030, "moves REAPER's undo history under Kumi's HISTORY: anything to redo is the producer's to redo in REAPER (Edit > Redo)"),
    (40023, "makes a new project in the tab"),
    (40025, "opens a project"),
    (40859, "makes a new project tab"),
    (40860, "closes the project tab"),
    (40861, "switches project tabs"),
    (40862, "switches project tabs"),
    (40886, "closes every project"),
    (41929, "makes a new project tab"),
    (40026, "saves the project: use save_project"),
    (40022, "saves the project as: use save_project"),
    (41895, "saves a new version of the project: use save_project"),
    (40394, "saves a project template"),
    (41824, "renders with a progress window that holds REAPER's main thread"),
    (42230, "renders with a progress window that holds REAPER's main thread"),
];

/// The same by name, for the actions of extensions and versions the ids don't cover.
fn refused_by_name(name: &str) -> Option<&'static str> {
    let lower = name.to_lowercase();
    if name.ends_with("...") || name.ends_with('…') {
        return Some("opens a dialog, which Kumi never runs unattended");
    }
    let starts = |prefix: &str| lower.starts_with(prefix);
    if starts("file: quit") || starts("file: exit") {
        return Some("quits REAPER");
    }
    if starts("file: open")
        || starts("file: new project")
        || starts("file: close")
        || starts("close current project")
        || lower.contains("project tab")
    {
        return Some("opens, closes or switches projects");
    }
    if starts("file: save") {
        return Some("saves: use save_project");
    }
    if starts("edit: undo") || starts("edit: redo") {
        return Some("moves REAPER's undo history under Kumi's HISTORY: Kumi's own changes are taken back by Kumi's undo, anything else is the producer's to undo or redo in REAPER");
    }
    if lower.contains("render project") || starts("file: render") {
        return Some("renders with a progress window that holds REAPER's main thread");
    }
    None
}

/// The action a request names: its id and its name in the action list.
fn resolve(ctx: &Ctx, input: &Value) -> Done<(i32, String)> {
    let asked = input
        .get("action")
        .ok_or_else(|| bad("action: a command id, a named command (\"_SWS_ABOUT\"), or the action's name as the action list shows it"))?;
    let reaper = ctx.reaper;
    let id = match asked {
        Value::Number(n) => n
            .as_i64()
            .filter(|n| *n > 0 && *n < i32::MAX as i64)
            .map(|n| n as i32)
            .ok_or_else(|| bad("action: a command id is a whole number from 1"))?,
        Value::String(text) if text.trim().starts_with('_') => reaper
            .command_id(text.trim())
            .ok_or_else(|| bad(format!("REAPER has no named command {:?} (an extension it needs may be missing)", text.trim())))?,
        Value::String(text) if text.trim().parse::<i32>().is_ok() => {
            reaper.command_id(text.trim()).ok_or_else(|| bad(format!("REAPER has no action {}", text.trim())))?
        }
        Value::String(text) => by_name(reaper, text.trim())?,
        _ => return Err(bad("action: a command id, a named command, or the action's name")),
    };
    let name = reaper.action_name(id).ok_or_else(|| bad(format!("REAPER has no action {id}")))?;
    if let Some((_, why)) = REFUSED.iter().find(|(refused, _)| *refused == id) {
        return Err(bad(format!("{} {why}: Kumi won't run it as a change", super::quoted(&name))));
    }
    if let Some(why) = refused_by_name(&name) {
        return Err(bad(format!("{} {why}: Kumi won't run it as a change", super::quoted(&name))));
    }
    Ok((id, name))
}

/// An action by its name in the action list (case aside); the close ones when none has it.
fn by_name(reaper: &Reaper, name: &str) -> Done<i32> {
    if name.is_empty() {
        return Err(bad("action: the action's name, as the action list shows it"));
    }
    let actions = reaper.actions();
    if let Some((id, _)) = actions.iter().find(|(_, known)| known.eq_ignore_ascii_case(name)) {
        return Ok(*id);
    }
    let lower = name.to_lowercase();
    let words: Vec<&str> = lower.split_whitespace().filter(|w| w.len() > 2).collect();
    let mut close: Vec<(usize, &String)> = actions
        .iter()
        .map(|(_, known)| (words.iter().filter(|w| known.to_lowercase().contains(**w)).count(), known))
        .filter(|(hits, _)| *hits > 0)
        .collect();
    close.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.len().cmp(&b.1.len())));
    let close: Vec<String> = close.into_iter().take(5).map(|(_, known)| super::quoted(known)).collect();
    if close.is_empty() {
        Err(bad(format!("REAPER has no action named {}", super::quoted(name))))
    } else {
        Err(bad(format!("REAPER has no action named {}; close: {}", super::quoted(name), close.join(", "))))
    }
}

/// The tracks and items a request selects for the action.
struct Selection {
    tracks: Option<Vec<Track>>,
    items: Option<Vec<Item>>,
}

fn selection_of(ctx: &Ctx, input: &Value) -> Done<Selection> {
    let refs = |key: &str| -> Done<Option<Vec<Ref>>> {
        match input.get(key) {
            None | Some(Value::Null) => Ok(None),
            Some(Value::Array(list)) if list.len() <= 10_000 => {
                list.iter().map(|r| Ref::parse(r.as_str().unwrap_or_default()).map_err(bad)).collect::<Done<Vec<_>>>().map(Some)
            }
            Some(_) => Err(bad(format!("{key} must list refs"))),
        }
    };
    let tracks = match refs("trackRefs")? {
        None => None,
        Some(list) => Some(
            list.into_iter()
                .map(|r| match r {
                    Ref::Track(id) => ctx.reads.track_by_id(ctx.reaper, ctx.proj, &id),
                    other => Err(bad(format!("trackRefs: a track ref, not r:{}", other.kind()))),
                })
                .collect::<Done<Vec<_>>>()?,
        ),
    };
    let items = match refs("itemRefs")? {
        None => None,
        Some(list) => Some(
            list.into_iter()
                .map(|r| match r {
                    Ref::Item(guid) => ctx.reads.item_by_ref(ctx.reaper, ctx.proj, &guid),
                    other => Err(bad(format!("itemRefs: an item ref, not r:{}", other.kind()))),
                })
                .collect::<Done<Vec<_>>>()?,
        ),
    };
    Ok(Selection { tracks, items })
}

/// The producer's selection, by id, to put back after the action: the tracks (the master among them) and the items.
struct Kept {
    tracks: Vec<String>,
    master: bool,
    items: Option<Vec<String>>,
}

fn keep(ctx: &Ctx, with_items: bool) -> Kept {
    let reaper = ctx.reaper;
    let tracks = (0..reaper.track_count(ctx.proj))
        .filter_map(|at| reaper.track_at(ctx.proj, at))
        .filter(|t| reaper.track_value(*t, "I_SELECTED") != 0.0)
        .map(|t| reaper.track_guid(t))
        .collect();
    let master = reaper.master_track(ctx.proj).is_some_and(|m| reaper.track_value(m, "I_SELECTED") != 0.0);
    let items = with_items.then(|| {
        reaper
            .all_items(ctx.proj)
            .into_iter()
            .filter(|item| reaper.item_value(*item, "B_UISEL") != 0.0)
            .map(|item| reaper.item_guid(item))
            .collect()
    });
    Kept { tracks, master, items }
}

fn put_back(ctx: &Ctx, kept: &Kept) {
    let reaper = ctx.reaper;
    for at in 0..reaper.track_count(ctx.proj) {
        if let Some(t) = reaper.track_at(ctx.proj, at) {
            let selected = kept.tracks.contains(&reaper.track_guid(t));
            if (reaper.track_value(t, "I_SELECTED") != 0.0) != selected {
                reaper.set_track_selected(t, selected);
            }
        }
    }
    if let Some(m) = reaper.master_track(ctx.proj) {
        reaper.set_track_selected(m, kept.master);
    }
    if let Some(items) = &kept.items {
        for item in reaper.all_items(ctx.proj) {
            let selected = items.contains(&reaper.item_guid(item));
            if (reaper.item_value(item, "B_UISEL") != 0.0) != selected {
                reaper.set_item_selected(item, selected);
            }
        }
    }
}

fn title(name: &str) -> String {
    format!("Ran action {}", super::quoted(name))
}

pub fn preview(ctx: &Ctx, input: &Value) -> Done<Preview> {
    let (id, name) = resolve(ctx, input)?;
    let selection = selection_of(ctx, input)?;
    let mut display = json!({"title": title(&name), "note": NOTE});
    if selection.tracks.is_none() && selection.items.is_none() {
        display["note"] = format!("{NOTE} It acts on what the producer has selected now.").into();
    }
    Ok(Preview { target: json!({"kind": "action", "command": id}), prior: json!({}), fence: json!({"command": id, "name": name}), display })
}

pub fn apply(ctx: &Ctx, input: &Value, fence: &Value) -> Done<Applied> {
    let (id, name) = resolve(ctx, input)?;
    if fence["command"] != id {
        return Err(bad("the action isn't the one previewed"));
    }
    let selection = selection_of(ctx, input).map_err(|fault| {
        if fault.0 == "failed" {
            ("changed".to_string(), format!("{}: it went since the preview", fault.1))
        } else {
            fault
        }
    })?;
    let reaper = ctx.reaper;
    let kept = (selection.tracks.is_some() || selection.items.is_some()).then(|| keep(ctx, selection.items.is_some()));
    if let Some(tracks) = &selection.tracks {
        for at in 0..reaper.track_count(ctx.proj) {
            if let Some(t) = reaper.track_at(ctx.proj, at) {
                reaper.set_track_selected(t, tracks.contains(&t));
            }
        }
        if let Some(m) = reaper.master_track(ctx.proj) {
            reaper.set_track_selected(m, tracks.contains(&m));
        }
    }
    if let Some(items) = &selection.items {
        reaper.select_all_items(ctx.proj, false);
        for item in items {
            reaper.set_item_selected(*item, true);
        }
    }
    let ran = reaper.run_action(id);
    if let Some(kept) = &kept {
        put_back(ctx, kept);
    }
    if !ran {
        return Err(failed("this REAPER can't run actions"));
    }
    Ok(Applied {
        applied: json!({"command": id, "name": name}),
        display: json!({"title": title(&name), "note": NOTE}),
        ..Default::default()
    })
}

pub fn restore(_ctx: &Ctx, _restore: &Restore) -> Done<Restored> {
    Err(("daw-only".into(), "Kumi can't take back what an action did: REAPER's own undo can (Edit > Undo)".into()))
}

/// What the answer of an action says once its block is closed: with no point of REAPER's, it changed nothing the project
/// keeps, and Kumi makes no row of it. One that moved REAPER's history itself (a script that undoes) keeps its row and says
/// so: the watch tells HISTORY what it took back on its next look.
pub fn after(applied: &mut Applied, undo: &Value) {
    if undo["historyMoved"] == true {
        applied.display["note"] = HISTORY_MOVED.into();
        applied.applied["historyMoved"] = true.into();
        return;
    }
    if undo["entry"] == false {
        applied.display["unchanged"] = true.into();
        applied.display["note"] = NO_POINT.into();
        applied.applied["changed"] = false.into();
    }
}

// ----- saving (actions: no change, no row) -----

/// File: Save new version of project (automatically increment project name).
const SAVE_NEW_VERSION: i32 = 41895;

fn plain_path(text: &str) -> Done<PathBuf> {
    let path = PathBuf::from(text);
    if !path.is_absolute() || path.components().any(|c| matches!(c, Component::ParentDir)) {
        return Err(bad("path must be a whole path, with no \"..\" in it"));
    }
    Ok(path)
}

impl Changes {
    /// `project_save {epoch, version?, path?}`: the project saved where it is, as its next version, or (one never saved)
    /// under a path in the producer's project folder.
    pub fn project_save(&self, reaper: &Reaper, reads: &Reads, params: &Value) -> Answer {
        let ctx = self.context(reaper, reads, params)?;
        let info = reaper.current_project().ok_or_else(|| failed("REAPER has no project open"))?;
        let version = params.get("version").and_then(Value::as_bool).unwrap_or(false);
        let path = params.get("path").and_then(Value::as_str).filter(|p| !p.is_empty());
        if info.path.is_empty() {
            let Some(path) = path else {
                return Err(failed("this project was never saved: give path, a .rpp file in the folder REAPER saves projects in, to save it there (Kumi never opens Save As)"));
            };
            let path = plain_path(path)?;
            if path.extension().is_none_or(|e| !e.eq_ignore_ascii_case("rpp")) {
                return Err(bad("path must name a .rpp file"));
            }
            let folder = reaper
                .config_text("defsavepath")
                .filter(|f| !f.is_empty())
                .ok_or_else(|| failed("REAPER has no project folder set (Preferences > Paths): save this project in REAPER first"))?;
            if !path.starts_with(Path::new(&folder)) {
                return Err(bad(format!("path must be inside the folder REAPER saves projects in, {folder}")));
            }
            if path.exists() {
                return Err(bad(format!("{} is there already: Kumi never saves over a file", path.display())));
            }
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).map_err(|e| failed(format!("Kumi couldn't make the folder {}: {e}", parent.display())))?;
            }
            let text = path.to_string_lossy().into_owned();
            reaper.save_project_as(ctx.proj, &text, 8);
            let now = reaper.current_project().map(|p| p.path).unwrap_or_default();
            if !path.exists() {
                return Err(failed("REAPER didn't write the project"));
            }
            return Ok(json!({"epoch": reads.epoch(), "saved": now, "dirty": reaper.is_dirty(ctx.proj)}));
        }
        if path.is_some() {
            return Err(bad(
                "this project has its file: it is saved there (or as its next version with version: true), not under another path",
            ));
        }
        if version {
            if !reaper.run_action(SAVE_NEW_VERSION) {
                return Err(failed("REAPER wouldn't save a new version"));
            }
        } else if !reaper.save(ctx.proj) {
            return Err(failed("REAPER wouldn't save the project"));
        }
        let now = reaper.current_project().map(|p| p.path).unwrap_or_default();
        let dirty = reaper.is_dirty(ctx.proj);
        if dirty {
            return Err(failed("REAPER still has unsaved changes after saving (a read-only folder, or a plug-in refusing)"));
        }
        Ok(json!({"epoch": reads.epoch(), "saved": now, "version": version, "dirty": dirty}))
    }

    /// `track_template_save {epoch, trackRefs, name | path, items?, envelopes?, replace?}`: the tracks written as a track
    /// template, the producer's selection put back.
    pub fn track_template_save(&self, reaper: &Reaper, reads: &Reads, params: &Value) -> Answer {
        let ctx = self.context(reaper, reads, params)?;
        let list = params
            .get("trackRefs")
            .and_then(Value::as_array)
            .filter(|l| !l.is_empty() && l.len() <= 1000)
            .ok_or_else(|| bad("trackRefs: the tracks to save, in their order"))?;
        let mut tracks = Vec::new();
        for reference in list {
            match Ref::parse(reference.as_str().unwrap_or_default()).map_err(bad)? {
                Ref::Track(TrackId::Master) => return Err(bad("the master track isn't saved in a track template")),
                Ref::Track(id) => tracks.push(reads.track_by_id(reaper, ctx.proj, &id)?),
                other => return Err(bad(format!("trackRefs: a track ref, not r:{}", other.kind()))),
            }
        }
        let path = match (params.get("name").and_then(Value::as_str), params.get("path").and_then(Value::as_str)) {
            (Some(name), None) => {
                let name = name.trim().trim_end_matches(".RTrackTemplate");
                let relative = Path::new(name);
                if name.is_empty() || relative.is_absolute() || relative.components().any(|c| !matches!(c, Component::Normal(_))) {
                    return Err(bad("name must be a template's name (folders inside TrackTemplates allowed, \"Drums/Kit\")"));
                }
                // (a folder at a time, so the path has the system's separators throughout)
                Path::new(&reaper.resource_path())
                    .join("TrackTemplates")
                    .join(Path::new(&format!("{name}.RTrackTemplate")).components().collect::<PathBuf>())
            }
            (None, Some(path)) => {
                let path = plain_path(path)?;
                if path.extension().is_none_or(|e| !e.eq_ignore_ascii_case("RTrackTemplate")) {
                    return Err(bad("path must name a .RTrackTemplate file"));
                }
                path
            }
            _ => return Err(bad("give name (saved in REAPER's TrackTemplates folder) or path, not both")),
        };
        let replace = params.get("replace").and_then(Value::as_bool).unwrap_or(false);
        if path.exists() && !replace {
            return Err(bad(format!(
                "{} is there already: pass replace: true only when the producer asked to write over it",
                path.display()
            )));
        }
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| failed(format!("Kumi couldn't make the folder {}: {e}", parent.display())))?;
        }
        let options = 1
            | if params.get("items").and_then(Value::as_bool).unwrap_or(false) { 2 } else { 0 }
            | if params.get("envelopes").and_then(Value::as_bool).unwrap_or(true) { 4 } else { 0 };
        let kept = keep(&ctx, false);
        for at in 0..reaper.track_count(ctx.proj) {
            if let Some(t) = reaper.track_at(ctx.proj, at) {
                reaper.set_track_selected(t, tracks.contains(&t));
            }
        }
        if let Some(m) = reaper.master_track(ctx.proj) {
            reaper.set_track_selected(m, false);
        }
        let before = std::fs::metadata(&path).and_then(|m| m.modified()).ok();
        let before_text = before.and_then(|_| std::fs::read(&path).ok());
        let text = path.to_string_lossy().into_owned();
        let saved = reaper.save_project_as(ctx.proj, &text, options);
        put_back(&ctx, &kept);
        let after = std::fs::metadata(&path).and_then(|m| m.modified()).ok();
        // A save over a file can keep its time where file times are coarse (Windows): then the contents say whether it was written.
        let unchanged = before.is_some() && before == after && std::fs::read(&path).ok() == before_text;
        if !saved || after.is_none() || unchanged {
            return Err(failed("REAPER didn't write the track template"));
        }
        let names: Vec<String> = tracks.iter().map(|t| super::track_name(reaper, *t)).collect();
        Ok(
            json!({"epoch": reads.epoch(), "saved": text, "tracks": names, "trackRefs": tracks.iter().map(|t| refs::track_ref(&super::tracks::track_id_of(&ctx, *t))).collect::<Vec<_>>()}),
        )
    }
}
