//! Answering the wire's requests through the project reader: what the run loop does for a queued `read`, `snapshot`
//! or `focus_body`, and what the fixture recorder and the reads test do over a helper of their own, so the three run
//! the same code.

use std::time::Instant;

use serde_json::Value;

use crate::ax::Ax;
use crate::bodies;
use crate::helper::Helper;
use crate::log;
use crate::project::{Needs, Project};
use crate::queue::Answer;
use crate::read::{self, Want};
use crate::refs::Ref;

/// Answers one request over Logic: the catalog gathers what the request needs, then the dispatch answers from it.
pub fn answer(project: &mut Project, helper: &mut dyn Helper, pid: u32, method: &str, params: &Value) -> Answer {
    let mut ax = Ax::new(helper, pid);
    let started = Instant::now();
    let result = match method {
        "read" => {
            let want = read::want(params, project.epoch)?;
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
        "snapshot" => {
            let catalog = project.catalog(&mut ax, &Needs::snapshot()).map_err(|e| Project::fault(&e))?;
            Ok(read::snapshot(&catalog))
        }
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
