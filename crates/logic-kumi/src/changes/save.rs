//! `project_save`: the project saved where it is (Cmd-S, through the producer's own binding of Save, with Logic in
//! front) or as another file (File > Save As…, the save panel driven through the panel service). Saving is no change
//! and no step of Logic's undo, so it is a method of the bridge, not a kind: the host's `project_save` passes it
//! through. Read back: the window's `AXDocument` for the path, and the window's close button's `AXEdited` for whether
//! unsaved changes remain (the dot in the close button; a file under `~/Music` can't be looked at from a process
//! without the Files and Folders permission, and the window says it anyway). Never over a file that exists: a file
//! Kumi can see there is refused, and Logic's own replace sheet, should one come, is cancelled. `version` is
//! REAPER's: Logic keeps no versions.

use std::path::{Component, Path, PathBuf};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use super::{front, nudge, panel, Ctx, Done};
use crate::keys::{Chord, KeyCommands};
use crate::queue::Answer;
use crate::refs::{bad, failed};

/// Logic's Save command's id, and the chord it has from the factory.
const SAVE: u32 = 754;
const SAVE_CHORD: &str = "cmd-s";
/// How long a save gets to show on disk, and Logic to settle after a save as.
const SAVE_WAIT: Duration = Duration::from_secs(6);
const AFTER_SAVE_AS: Duration = Duration::from_millis(800);

/// The project's path as the window's document says it, if the project was ever saved.
pub(crate) fn document(ctx: &mut Ctx) -> Done<Option<String>> {
    let reply = nudge::read(ctx.ax.windows())?;
    let (windows, _) = crate::logic::parse_windows(&reply);
    let window = crate::logic::project_window(&windows);
    Ok(window.and_then(|window| window.document.as_deref().map(crate::logic::document_path)))
}

/// Whether the project has unsaved changes: the window's close button's `AXEdited` (the dot in it).
pub(crate) fn edited(ctx: &mut Ctx) -> Done<bool> {
    let window = nudge::read(ctx.ax.tree(&ctx.project.root, "", 1, 8, Some(&["role", "subrole"])))?;
    let close = window
        .children
        .iter()
        .find(|c| c.role == "AXButton" && c.subrole == "AXCloseButton")
        .ok_or_else(|| failed("Logic Pro's window shows no close button"))?;
    let attrs = nudge::read(ctx.ax.get(&ctx.project.root, &close.path, &["AXEdited"]))?;
    Ok(attrs.get("AXEdited") == Some(&Value::Bool(true)))
}

/// Whether a file is there, as far as Kumi can see (a folder macOS keeps from it reads as nothing there).
fn visible(path: &Path) -> bool {
    std::fs::metadata(path).is_ok()
}

fn plain_path(text: &str) -> Done<PathBuf> {
    let path = PathBuf::from(text);
    if !path.is_absolute() || path.components().any(|c| matches!(c, Component::ParentDir)) {
        return Err(bad("path must be a whole path, with no \"..\" in it"));
    }
    if path.extension().is_none_or(|ext| !ext.eq_ignore_ascii_case("logicx")) {
        return Err(bad("path must name a .logicx package"));
    }
    let name = path.file_stem().map(|stem| stem.to_string_lossy().into_owned()).unwrap_or_default();
    if name.is_empty() || name.contains(':') || name.contains('/') || name.chars().any(char::is_control) || name.chars().count() > 200 {
        return Err(bad("path must end in a file name Logic Pro can save: no colon or slash in it"));
    }
    Ok(path)
}

/// `project_save {epoch, path?, version?}`.
pub fn project_save(ctx: &mut Ctx, keys: &KeyCommands, params: &Value) -> Answer {
    if params.get("version").is_some_and(|v| !v.is_null() && v != &Value::Bool(false)) {
        return Err(bad(
            "Logic Pro keeps no versions: give path (a .logicx that isn't there yet) to save a copy, or nothing to save in place",
        ));
    }
    let epoch = ctx.project.epoch.to_string();
    let now = document(ctx)?;
    match params.get("path").and_then(Value::as_str).filter(|p| !p.is_empty()) {
        Some(path) => {
            let path = plain_path(path)?;
            if now.as_deref() == Some(path.to_string_lossy().as_ref()) {
                return Err(bad("that is the project's own file: save in place (no path), or give another path"));
            }
            if visible(&path) {
                return Err(bad(format!("{} is there already: Kumi never saves over a file", path.display())));
            }
            let saved = save_as(ctx, &path)?;
            Ok(json!({"epoch": epoch, "saved": saved, "dirty": false}))
        }
        None => {
            let Some(path) = now else {
                return Err(failed("this project was never saved: give path, a .logicx that isn't there yet, to save it there (Kumi never saves over a file)"));
            };
            let dirty = save_in_place(ctx, keys, Path::new(&path))?;
            Ok(json!({"epoch": epoch, "saved": path, "dirty": dirty}))
        }
    }
}

/// Cmd-S (the producer's binding of Save) with Logic in front; the window's edited flag read back until it clears.
fn save_in_place(ctx: &mut Ctx, keys: &KeyCommands, path: &Path) -> Done<bool> {
    let chord = keys.chord(SAVE).cloned().or_else(|| Chord::parse(SAVE_CHORD)).ok_or_else(|| failed("no key for Save"))?;
    if !edited(ctx)? {
        // Nothing to save: Logic's Cmd-S would write the same file; the project is as saved.
        return Ok(false);
    }
    front::ensure(ctx.ax)?;
    front::post(ctx.ax, &chord)?;
    let until = Instant::now() + SAVE_WAIT;
    loop {
        super::pause(Duration::from_millis(200));
        if !edited(ctx)? {
            return Ok(false);
        }
        if let Some(modal) = modal_title(ctx)? {
            return Err(super::uncertain(format!(
                "Logic Pro shows a dialog ({modal:?}) after Cmd-S; whether it saved {} isn't known",
                path.display()
            )));
        }
        if Instant::now() >= until {
            return Err(super::uncertain(format!(
                "Cmd-S was posted to Logic Pro, but its window still shows {} as edited",
                path.display()
            )));
        }
    }
}

/// The title of a modal dialog Logic shows, if one is up.
fn modal_title(ctx: &mut Ctx) -> Done<Option<String>> {
    let reply = nudge::read(ctx.ax.windows())?;
    let (windows, _) = crate::logic::parse_windows(&reply);
    Ok(windows.iter().find(|window| window.dialog() || window.sheet()).map(|window| window.title.clone()))
}

/// File > Save As…, the panel taken to the folder (which must be there: the panel's "where" pop-up says where it
/// stands after the go-to), named, and its Save pressed; the window's document and edited flag read back.
fn save_as(ctx: &mut Ctx, path: &Path) -> Done<String> {
    let folder = path.parent().ok_or_else(|| bad("path must be inside a folder"))?;
    let name = path.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
    front::ensure(ctx.ax)?;
    super::structure::file_menu(ctx, &["Save As…"])?;
    let panel = panel::find_or_close(ctx.ax)?;
    let result = (|| {
        panel::go_to(ctx.ax, &panel, &folder.to_string_lossy())?;
        // A folder that isn't there leaves the panel where it was (the sheet closes anyway): the panel's own
        // "where" pop-up must name the folder asked before anything is saved into it.
        let wanted = folder.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let landed = panel::location(ctx.ax, &panel)?;
        if landed != wanted {
            return Err(bad(format!(
                "the folder {} isn't there (Logic Pro's panel stands in {landed:?}); Kumi makes no folders",
                folder.display()
            )));
        }
        panel::set_name(ctx.ax, &panel, &name)?;
        panel::press(ctx.ax, &panel, "OKButton")?;
        Ok(())
    })();
    if let Err(fault) = result {
        panel::cancel(ctx.ax, &panel);
        return Err(fault);
    }
    if !panel::gone(ctx.ax, panel::PANEL_WAIT)? {
        // A sheet asks something (a replace, a folder that can't be written): nothing is saved over.
        let asked = modal_title(ctx)?.unwrap_or_default();
        panel::cancel(ctx.ax, &panel);
        return Err(failed(format!("Logic Pro's save panel stayed up ({asked:?}): nothing was saved; is a file of that name there?")));
    }
    super::pause(AFTER_SAVE_AS);
    let until = Instant::now() + SAVE_WAIT;
    loop {
        let now = document(ctx)?;
        if now.as_deref() == Some(path.to_string_lossy().as_ref()) && !edited(ctx)? {
            return Ok(path.to_string_lossy().into_owned());
        }
        if Instant::now() >= until {
            return Err(super::uncertain(format!(
                "Logic Pro's window shows {} after Save As, not {}",
                now.unwrap_or_else(|| "no document".into()),
                path.display()
            )));
        }
        super::pause(Duration::from_millis(200));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_path_must_be_a_whole_logicx_not_there_yet() {
        assert!(plain_path("/Users/me/Music/Logic/Copy.logicx").is_ok());
        assert_eq!(plain_path("Copy.logicx").unwrap_err().1, "path must be a whole path, with no \"..\" in it");
        assert!(plain_path("/a/../b.logicx").is_err());
        assert!(plain_path("/a/b.rpp").is_err());
        assert!(plain_path("/a/b:c.logicx").is_err());
        assert!(plain_path("/a/.logicx").is_err());
        let dir = tempfile::tempdir().unwrap();
        assert!(!visible(&dir.path().join("Nothing.logicx")));
        std::fs::create_dir_all(dir.path().join("Song.logicx")).unwrap();
        assert!(visible(&dir.path().join("Song.logicx")));
    }
}
