//! Answering the wire's requests through the project reader: what the run loop does for a queued `read`, `snapshot`
//! or `focus_body`, and what the fixture recorder and the reads test do over a helper of their own, so the three run
//! the same code; and the same for the change methods, through the change side ([`answer_change`]).

use std::time::Instant;

use serde_json::Value;

use crate::ax::Ax;
use crate::bodies;
use crate::changes::{Changes, Ctx};
use crate::helper::Helper;
use crate::log;
use crate::mcu::Mcu;
use crate::project::{Needs, Project};
use crate::queue::Answer;
use crate::read::{self, Want};
use crate::refs::Ref;

/// The methods the change side answers.
pub const CHANGE_METHODS: [&str; 11] = [
    "change_preview",
    "change_apply",
    "change_restore",
    "transport_stop",
    "transport_play",
    "cursor_set",
    "undo_step_begin",
    "undo_step_end",
    "project_save",
    "bounce",
    "export_tracks",
];

/// Answers one change request over Logic, through the change side: the run loop's for a queued one, the recorder's
/// and the changes test's over a helper of their own.
pub fn answer_change(
    project: &mut Project,
    changes: &mut Changes,
    helper: &mut dyn Helper,
    pid: u32,
    method: &str,
    params: &Value,
) -> Answer {
    let mut ax = Ax::new(helper, pid);
    let started = Instant::now();
    let result = {
        let mut ctx = Ctx { project, ax: &mut ax };
        match method {
            "change_preview" => changes.preview(&mut ctx, params),
            "change_apply" => changes.apply(&mut ctx, params),
            "change_restore" => changes.restore(&mut ctx, params),
            "transport_stop" => changes.transport_stop(&mut ctx),
            "transport_play" => changes.transport_play(&mut ctx),
            "cursor_set" => changes.cursor_set(&mut ctx, params),
            "undo_step_begin" => changes.step_begin(params),
            "undo_step_end" => changes.step_end(params),
            "project_save" => changes.project_save(&mut ctx, params),
            "bounce" => changes.bounce(&mut ctx, params),
            "export_tracks" => changes.export_tracks(&mut ctx, params),
            other => Err((daw_bridge::wire::code::UNKNOWN_METHOD.into(), format!("the Logic Pro bridge has no method {other:?}"))),
        }
    };
    let said = match &result {
        Ok(_) => "ok".to_string(),
        Err((code, _)) => code.clone(),
    };
    log::line(&format!(
        "{method} {} -> {said} in {} ms ({} helper calls)",
        params.get("kind").and_then(Value::as_str).unwrap_or(""),
        started.elapsed().as_millis(),
        ax.calls
    ));
    result
}

/// The listening methods: `hear`, `audition`, `render`.
pub const LISTEN_METHODS: [&str; 3] = crate::listen::METHODS;

/// Answers one listening request over Logic, with the bridge's Mackie Control when there is one (its timecode is how
/// the playhead is followed while Logic plays, its solo buttons how a track is soloed for a pass): the run loop's for a
/// queued one, the recorder's and the listening test's over a helper of their own. `keepalive` is called about once a
/// second while Logic plays.
#[allow(clippy::too_many_arguments)]
pub fn answer_listen(
    project: &mut Project,
    mcu: Option<&mut Mcu>,
    helper: &mut dyn Helper,
    pid: u32,
    method: &str,
    params: &Value,
    keepalive: &mut dyn FnMut(),
) -> Answer {
    let mut ax = Ax::new(helper, pid);
    let started = Instant::now();
    let result = {
        let mut ctx = Ctx { project, ax: &mut ax };
        crate::listen::answer(&mut ctx, mcu, method, params, keepalive)
    };
    let said = match &result {
        Ok(_) => "ok".to_string(),
        Err((code, _)) => code.clone(),
    };
    log::line(&format!("{method} -> {said} in {} ms ({} helper calls)", started.elapsed().as_millis(), ax.calls));
    result
}

/// Answers one request over Logic: the catalog gathers what the request needs, then the dispatch answers from it. The
/// reads that need a key command of Logic's (the Track Automation Event List) post the factory set's chord: the
/// bridge's run loop answers through [`answer_keyed`] with the producer's own set.
pub fn answer(project: &mut Project, helper: &mut dyn Helper, pid: u32, method: &str, params: &Value) -> Answer {
    static FACTORY: std::sync::OnceLock<crate::keys::KeyCommands> = std::sync::OnceLock::new();
    answer_keyed(project, helper, pid, method, params, FACTORY.get_or_init(crate::keys::KeyCommands::embedded))
}

/// [`answer`] with the key command set the reads post their chords with.
pub fn answer_keyed(
    project: &mut Project,
    helper: &mut dyn Helper,
    pid: u32,
    method: &str,
    params: &Value,
    keys: &crate::keys::KeyCommands,
) -> Answer {
    let mut ax = Ax::new(helper, pid);
    let started = Instant::now();
    let result = match method {
        "read" => 'read: {
            let want = read::want(params, project.epoch)?;
            if matches!(&want, Want::Kind { kind, .. } if kind == "recent") {
                // The projects Logic opened lately: its File > Open Recent menu, read without opening it.
                let rows = crate::recent::rows(project, &mut ax).map_err(|e| Project::fault(&e))?;
                break 'read read::page(&rows, params, project.epoch);
            }
            if let Some(answer) = crate::changes::read_wide(project, &mut ax, keys, &want, params) {
                break 'read answer;
            }
            let needs = match &want {
                Want::One(reference) => {
                    let parsed = Ref::parse(reference);
                    let kind = parsed.as_ref().map(|r| r.kind.clone()).unwrap_or_default();
                    let notes_of = parsed.filter(|r| r.kind == "note").map(|r| r.first());
                    Needs::for_kind(&kind, notes_of)
                }
                Want::Kind { kind, parent } => {
                    let notes_of = if kind == "note" {
                        Some(project.region_place(parent, params.get("parent").and_then(Value::as_str).unwrap_or_default())?)
                    } else {
                        None
                    };
                    Needs::for_kind(kind, notes_of)
                }
            };
            let catalog = project.catalog(&mut ax, &needs).map_err(|e| Project::fault(&e))?;
            read::read(&catalog, params)
        }
        "snapshot" => match params.get("rows").filter(|rows| !rows.is_null()) {
            // The bulk read: every row of the kinds asked from one catalog, the panes it needs shown once.
            Some(rows) => {
                let kinds = read::bulk_kinds(rows)?;
                let catalog = project.catalog(&mut ax, &Needs::for_kinds(&kinds)).map_err(|e| Project::fault(&e))?;
                Ok(read::bulk(&catalog, &kinds, params))
            }
            None => {
                let catalog = project.catalog(&mut ax, &Needs::snapshot()).map_err(|e| Project::fault(&e))?;
                Ok(read::snapshot(&catalog))
            }
        },
        "focus_body" => {
            let kind = params.get("kind").and_then(Value::as_str).unwrap_or("");
            let mut catalog = project.catalog(&mut ax, &Needs::for_body(kind, None)).map_err(|e| Project::fault(&e))?;
            if kind == "notes" {
                let place = bodies::notes_region(&catalog, params)?;
                let beats_per_bar = catalog.transport.get("timeSignature").and_then(Value::as_str).map_or(4, crate::rows::beats_per_bar_of);
                let notes = project.notes_rows(&mut ax, place, beats_per_bar).map_err(|e| Project::fault(&e))?;
                catalog.notes.push((place, notes));
            }
            bodies::body(&catalog, params)
        }
        other => Err((daw_bridge::wire::code::UNKNOWN_METHOD.into(), format!("the Logic Pro bridge has no method {other:?}"))),
    };
    log::line(&format!("{method} {params} in {} ms ({} helper calls)", started.elapsed().as_millis(), ax.calls));
    result
}
