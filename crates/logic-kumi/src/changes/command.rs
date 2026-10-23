//! `key-command`: any of Logic's key commands, by its English name or its id, posted with the producer's own binding
//! (the key command set resolved by [`crate::keys`]) while Logic is in front: the escape hatch, REAPER's `run_action`.
//! What it did is Logic's to know: when the Edit menu's validated Undo title moved, Logic made a step of it and the
//! change is a HISTORY row only Logic's own undo takes back; when the title stayed, it changed nothing Logic keeps a
//! step for, and there is no row (`applied.changed: false`, `display.unchanged: true`, `undo.entry: false`). A restore
//! is `daw-only`. Never a command that opens a dialog (its name ends in an ellipsis), leaves the project (Quit, Open,
//! Close, New, Import, Export, Bounce) or saves (`project_save` does), nor Logic's own Undo and Redo, which would move
//! its history under Kumi's.

use std::time::Duration;

use serde_json::{json, Value};

use super::{changed, front, Changes, Ctx, Done, Preview};
use crate::keys::Command;
use crate::queue::Answer;
use crate::refs::bad;

const NOTE: &str = "Only Logic Pro's own undo can take this back (Cmd-Z in Logic Pro).";
const NO_STEP: &str = "Logic Pro made no undo step of it: it changed nothing its history keeps (a view, selection or transport command), so it has no row in HISTORY.";
/// How long a command gets before the Edit menu's titles are read back.
/// Logic names the step a moment after the command lands, later for a marker made than for a selection: the Undo
/// title is read this often, this many times at most, until it moves.
const COMMAND_SETTLE: Duration = Duration::from_millis(250);
const COMMAND_READS: usize = 6;

/// Commands never run as a change, by id: Save and Save As (`project_save`), Undo and Redo.
const REFUSED_IDS: [(u32, &str); 4] = [
    (754, "saves the project: use project_save"),
    (32, "saves the project as: use project_save"),
    (761, "moves Logic Pro's undo history under Kumi's HISTORY: Kumi's own changes are taken back by Kumi's undo, anything else is the producer's to undo in Logic Pro"),
    (796, "moves Logic Pro's undo history under Kumi's HISTORY: anything to redo is the producer's to redo in Logic Pro (Shift-Cmd-Z)"),
];

/// The same by name, for what the ids don't cover.
fn refused_by_name(name: &str) -> Option<&'static str> {
    if name.ends_with('…') || name.ends_with("...") {
        return Some("opens a dialog, which Kumi never runs unattended");
    }
    let lower = name.to_lowercase();
    let starts = |prefix: &str| lower.starts_with(prefix);
    if starts("quit") {
        return Some("quits Logic Pro");
    }
    if starts("open ") && !starts("open track stack") && !lower.contains("list") && !lower.contains("editor") && !lower.contains("window")
        || starts("close project")
        || starts("new project")
        || starts("new from template")
    {
        return Some("opens, closes or switches projects");
    }
    if starts("save") || starts("revert to") {
        return Some("saves or reverts the project: use project_save");
    }
    if starts("import") || starts("export") || starts("bounce") || starts("share") {
        return Some("imports, exports or bounces through a dialog, which Kumi never runs unattended");
    }
    if lower == "undo" || lower == "redo" || starts("delete undo history") {
        return Some("moves Logic Pro's undo history under Kumi's HISTORY");
    }
    None
}

/// The command a request names, resolved and allowed.
fn resolve(changes: &Changes, input: &Value) -> Done<Command> {
    let asked = input.get("command").filter(|v| !v.is_null()).ok_or_else(|| {
        bad("command: a key command's English name as Logic Pro's Key Commands window lists it (\"Select All\"), or its id")
    })?;
    let command = changes.keys.resolve(asked)?;
    if let Some((_, why)) = REFUSED_IDS.iter().find(|(id, _)| *id == command.id) {
        return Err(bad(format!("{} {why}: Kumi won't run it as a change", command.label())));
    }
    if let Some(why) = command.name.as_deref().and_then(refused_by_name) {
        return Err(bad(format!("{} {why}: Kumi won't run it as a change", command.label())));
    }
    if command.chord.is_none() {
        return Err(bad(format!(
            "{} isn't bound to any key in Logic Pro's key command set, so Kumi can't post it: assign one in Logic Pro > Key Commands",
            command.label()
        )));
    }
    Ok(command)
}

fn title(command: &Command) -> String {
    let chord = command.chord.as_ref().map(|chord| format!(" ({})", chord.text())).unwrap_or_default();
    format!("Ran key command {}{chord}", command.label())
}

pub fn preview(changes: &Changes, _ctx: &mut Ctx, input: &Value) -> Done<Preview> {
    let command = resolve(changes, input)?;
    let mut target = json!({"kind": "action", "command": command.id});
    if let Some(name) = &command.name {
        target["name"] = name.clone().into();
    }
    Ok(Preview {
        target,
        prior: json!({}),
        fence: json!({"command": command.id, "name": command.name}),
        display: json!({"title": title(&command), "note": format!("{NOTE} It acts on what the producer has selected now.")}),
    })
}

/// The apply, whole: the keys posted in front, the Edit menu's titles compared before and after.
pub fn apply(changes: &mut Changes, ctx: &mut Ctx, params: &Value) -> Answer {
    let input = super::object(params, "input")?;
    let fence = super::object(params, "fence")?;
    let desc = super::text(params, "desc")?;
    let command = resolve(changes, input)?;
    if fence["command"] != command.id {
        return Err(changed("the command isn't the one previewed"));
    }
    let chord = command.chord.clone().ok_or_else(|| bad("the command has no key"))?;
    front::ensure(ctx.ax)?;
    let (before, verified) = changes.watch.fresh_titles(ctx)?;
    if !verified {
        return Err(super::busy("Logic Pro went behind before the command could be posted; nothing changed"));
    }
    front::post(ctx.ax, &chord)?;
    // A key command can change anything Logic shows, so what was read of the project before it is stale: the M4 gate's
    // marker read, answered from the two-second list cache, missed the marker Create Marker had made.
    ctx.project.forget();
    // The M4 gate read the title once, 500 ms after the chord, and took a marker's step for none: read until it moves
    // (once over a recorded scene, whose reads come in the order the real run made them).
    let mut after = None;
    for _ in 0..COMMAND_READS {
        super::pause(COMMAND_SETTLE);
        let (titles, verified) = changes.watch.fresh_titles(ctx).unwrap_or_default();
        let moved = verified && titles.undo != before.undo;
        after = Some((titles, verified));
        if moved || !super::settling() {
            break;
        }
    }
    let (titles, verified) = after.unwrap_or_default();
    let key = super::change_key("key-command", &Value::Null);
    let (undo, step) = changes.watch.applied_with_titles(ctx, desc, &key, &before, titles, verified);
    let mut applied = json!({"command": command.id, "name": command.name, "key": chord.text()});
    let mut display = json!({"title": title(&command), "note": NOTE});
    if let Some(step) = step {
        applied["step"] = step.into();
    } else {
        applied["changed"] = false.into();
        display["unchanged"] = true.into();
        display["note"] = NO_STEP.into();
    }
    if let Some((_, _, count)) = changes.open_step.as_mut() {
        *count += 1;
    }
    Ok(json!({"epoch": ctx.project.epoch.to_string(), "applied": applied, "display": display, "undo": undo}))
}

pub fn restore() -> Answer {
    Err(("daw-only".into(), "Kumi can't take back what a key command did: Logic Pro's own undo can (Cmd-Z in Logic Pro)".into()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commands_that_leave_the_project_open_a_dialog_or_move_the_history_are_refused_and_the_rest_resolve() {
        let changes = Changes::default();
        let ok = |name: &str| resolve(&changes, &json!({"command": name}));
        let split = ok("Split Regions/Events at Playhead Position").unwrap();
        assert_eq!((split.id, split.chord.unwrap().text()), (574, "⌘T".into()));
        assert_eq!(title(&ok("Select All").unwrap()), "Ran key command “Select All” (⌘A)");
        for (asked, why) in [
            (json!(754), "project_save"),
            (json!("Undo"), "undo history"),
            (json!("Redo"), "undo history"),
            (json!("Save As Control Bar and Display Defaults"), "saves"),
            (json!("Set Left Locator by Playhead"), "isn't bound to any key"),
            (json!("Bounce Tracks in Place"), "bounces"),
        ] {
            let refused = resolve(&changes, &json!({"command": asked})).unwrap_err();
            assert_eq!(refused.0, "bad-params", "{asked}");
            assert!(refused.1.contains(why), "{asked}: {}", refused.1);
        }
        assert!(resolve(&changes, &json!({})).unwrap_err().1.starts_with("command:"));
        assert_eq!(refused_by_name("Open Track Stack"), None);
        assert_eq!(refused_by_name("Open Marker List"), None);
        assert!(refused_by_name("Open Project…").is_some());
        assert_eq!(refused_by_name("Import MIDI File…"), Some("opens a dialog, which Kumi never runs unattended"));
        assert!(refused_by_name("Quit Logic Pro").is_some());
        assert_eq!(restore().unwrap_err().0, "daw-only");
    }
}
