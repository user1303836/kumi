//! `change`, `undo` and `history` (C1, C2, CC-3): a plan of Kumi's own change steps run through the DAW's plan tool
//! (`make_changes`, the executor Kumi's app uses), on the project and epoch the plan was made on, under the endpoint's
//! write lease; the HISTORY rows it made; undo of one of them; and the rows.
//!
//! What a plan may do here is narrower than in Kumi's app (C2): each step must be on the DAW's allowlist for an agent
//! host's plugin (`DawProvider::plugin_steps`), with nothing in its input that names a file, holds raw data or code, or
//! asks for an action that deletes; and its input must be what the step takes. A refused step refuses the whole plan
//! before anything runs.

use super::connection;
use super::ctx::{epoch_of, epoch_schema, Call, Ctx, Effect, JsonObject, OpError, OpFuture, Operation};
use super::history::{CallIdentity, History, Row, DAW_UNDO};
use kumi_daw::{plugin::PluginStep, provider::DawProvider};
use kumi_types::contracts::{ChangeState, KernelTool};
use serde_json::{json, Map, Value};
use std::collections::HashMap;
use std::rc::Rc;

/// The most steps a plan takes (C2).
pub const MOST_STEPS: usize = 20;

/// The most of a step's preview line, of one field's value in it, of a quoted string, and of a name; and how many of a
/// list's or a group's values it shows.
const LINE: usize = 200;
const FIELD: usize = 80;
const QUOTED: usize = 120;
const NAME: usize = 40;
const FIRST: usize = 3;

pub static CHANGE: Operation = Operation {
    name: "change",
    description: "Changes the project: a plan of Kumi's change steps ({tool, input, as?}, at most 20), made on the project \
                  key and epoch an observe gave. It runs once, under the DAW's write lease, and answers what it did \
                  (applied, partial, unsure or refused, with HISTORY ids to undo). A plan made on another project or an \
                  earlier epoch is refused: observe again. `preview` describes it without changing anything.",
    schema: change_schema,
    effect: change_effect,
    run: run_change,
};

fn change_schema() -> Value {
    json!({"type": "object", "additionalProperties": false, "required": ["title", "steps", "expected_project_key", "expected_epoch"], "properties": {
        "title": {"type": "string", "minLength": 1, "maxLength": 120, "description": "What the plan does, in a few words"},
        "steps": {"type": "array", "minItems": 1, "maxItems": MOST_STEPS, "items": {
            "type": "object", "additionalProperties": false, "required": ["tool", "input"], "properties": {
                "tool": {"type": "string", "minLength": 1, "maxLength": 64},
                "input": {"type": "object"},
                "as": {"type": "string", "pattern": "^[A-Za-z][A-Za-z0-9_]{0,31}$", "description": "A name later steps use as @name for what this one makes"}
            }
        }},
        "expected_project_key": {"type": "string", "minLength": 1, "maxLength": 64},
        "expected_epoch": epoch_schema(),
        "note": {"type": "string", "maxLength": 500},
        "preview": {"type": "boolean", "description": "Describe the plan without changing anything (the host's approval)"}
    }})
}

/// What a file step's file field takes in a plugin's plan (CC-11): where the file comes from, never its path. A sound
/// from the map (of its current revision), a file Kumi made and keeps, or a file in an approved folder, with the host
/// tool that made it.
pub(super) fn source_schema() -> Value {
    json!({
        "description": "Where the file comes from, never its path: a sound from the map, a file Kumi made, or an approved file",
        "anyOf": [
            {"type": "object", "additionalProperties": false, "required": ["sound_id", "map_revision"], "properties": {
                "sound_id": {"type": "string", "minLength": 1, "maxLength": 128},
                "map_revision": {"type": "string", "minLength": 1, "maxLength": 64}
            }},
            {"type": "object", "additionalProperties": false, "required": ["artifact_id"], "properties": {
                "artifact_id": {"type": "string", "minLength": 1, "maxLength": 128}
            }},
            {"type": "object", "additionalProperties": false, "required": ["file", "provenance"], "properties": {
                "file": {"type": "string", "minLength": 1, "maxLength": 4096},
                "provenance": {"type": "object", "additionalProperties": false, "required": ["host_tool"], "properties": {
                    "host_tool": {"type": "string", "minLength": 1, "maxLength": 128},
                    "tool_call_id": {"type": "string", "maxLength": 256}
                }}
            }}
        ]
    })
}

/// A preview reads; the plan writes.
fn change_effect(args: &JsonObject) -> Effect {
    if previewing(args) {
        Effect::Read
    } else {
        Effect::Write
    }
}

fn previewing(args: &JsonObject) -> bool {
    args.get("preview").and_then(Value::as_bool).unwrap_or(false)
}

fn run_change(ctx: &Ctx, call: Call) -> OpFuture<'_> {
    Box::pin(change(ctx, call))
}

/// A plan refused before anything ran: C1's result with nothing changed.
fn refused(code: &str, reason: impl Into<String>) -> Value {
    json!({"state": "refused", "code": code, "reason": reason.into(), "history_ids": [], "steps": [], "undo_available": false})
}

fn refusal(error: OpError) -> Value {
    refused(error.code(), error.reason())
}

/// A plan's steps as checked: each one's HISTORY family, its file field when it places a file (CC-11), whether it's a
/// change only the DAW's own Undo takes back (CC-20), how the DAW's Undo holds those (by their step number), and the
/// reasons such steps give, with the DAW's name to say them.
struct Checked {
    families: Vec<&'static str>,
    files: Vec<(usize, &'static str)>,
    daw_undo: Vec<bool>,
    undo_steps: Vec<(u64, kumi_daw::plugin::DawUndoCount)>,
    because: Vec<&'static str>,
    uncounted: Option<&'static str>,
    app: &'static str,
}

impl Checked {
    /// What Kumi says of the change when a step of it is one only the DAW's own Undo takes back, on the system `os`
    /// names: CC-20's words with how many presses of the DAW's Undo take it back (or, while that isn't known, a step's
    /// own words for it when it has them), then each reason such steps give, once (twelve pad loads give theirs once).
    /// `ran` is what the plan answered once it ran (None for a preview); see [`Checked::presses`].
    fn undo_note(&self, os: &str, ran: Option<Option<&Value>>) -> Option<String> {
        use kumi_daw::plugin::{daw_undo_note, daw_undo_said};
        if !self.daw_undo.contains(&true) {
            return None;
        }
        let presses = self.presses(ran);
        let first = match (presses, self.uncounted) {
            (None, Some(words)) => daw_undo_said(words, self.app, os),
            _ => daw_undo_note(presses, self.app, os),
        };
        let mut said = vec![first];
        for why in &self.because {
            let why = daw_undo_said(why, self.app, os);
            if !said.contains(&why) {
                said.push(why);
            }
        }
        Some(said.join(" "))
    }

    /// A change holding a step only the DAW's own Undo takes back holds no other kind of step (CC-20's 20-r1): Live
    /// keeps a device load as an undo step of its own, outside the plan's (20g's trace), so the DAW's Undo would take a
    /// mixed change back one part at a time, not all of it as Kumi says. The refusal names the first such step.
    fn alone(&self, steps: &[Value]) -> Result<(), Value> {
        let (Some(first), true) = (self.daw_undo.iter().position(|marked| *marked), self.daw_undo.contains(&false)) else {
            return Ok(());
        };
        let tool = steps.get(first).and_then(|step| step.get("tool")).and_then(Value::as_str).unwrap_or_default();
        let app = self.app;
        Err(refused(
            "not_allowed",
            format!(
                "Step {} ({tool}) is a change only {app}'s own Undo takes back, so it goes in a change of its own: ask for the other steps separately.",
                first + 1
            ),
        ))
    }

    /// What decided step `step`'s count, from its done entry, when a field of it does: `{"route": "hotswap"}`.
    fn undo_by(&self, step: u64, done: &Value) -> Option<Value> {
        let field = self.undo_steps.iter().find(|(n, _)| *n == step)?.1.field()?;
        let said = done.get(field)?.clone();
        Some(json!({ field: said }))
    }

    /// How many presses of the DAW's Undo take the change back, when that's known. Before it runs (`ran` None), as its
    /// steps say, if they all can (a pad's route isn't known yet). Once it ran, from what the plan reports it did, and
    /// only when it did every step: after a run that stopped part way (`ran` holding no answer to read, or one that
    /// stopped, refused or held a step back), a number would have the producer undo their own work too.
    fn presses(&self, ran: Option<Option<&Value>>) -> Option<u32> {
        let step = |entry: &Value| entry.get("step").and_then(Value::as_u64);
        let steps: Vec<_> = match ran {
            None => self.undo_steps.iter().map(|(_, count)| count.steps(None)).collect::<Option<_>>()?,
            Some(answer) => {
                let answer = answer?;
                let list = |name: &str| answer.get(name).and_then(Value::as_array);
                let done = list("done")?;
                let whole = answer.get("stopped").is_none_or(Value::is_null)
                    && list("refused").is_none_or(Vec::is_empty)
                    && list("heldBack").is_none_or(Vec::is_empty)
                    && (1..=self.daw_undo.len() as u64).all(|n| done.iter().any(|entry| step(entry) == Some(n)));
                if !whole {
                    return None;
                }
                self.undo_steps
                    .iter()
                    .map(|(n, count)| count.steps(done.iter().find(|entry| step(entry) == Some(*n))))
                    .collect::<Option<_>>()?
            }
        };
        Some(kumi_daw::plugin::daw_undo_presses(steps))
    }
}

/// A plan's steps, each checked against the DAW's allowlist, what the DAW says it keeps no undo for (unless the step is
/// one only the DAW's own Undo takes back), and what the step takes. A file step's field holds a source, so its input is
/// checked once the source is resolved to a file.
fn checked(ctx: &Ctx, plan: &Rc<dyn KernelTool>, steps: &[Value]) -> Result<Checked, Value> {
    let Some(provider) = crate::daw::provider(ctx.daw()) else {
        return Err(refused("unsupported", "This runtime's DAW isn't one this build integrates."));
    };
    let offered = offered_steps(&plan.input_schema());
    let mut checked = Checked {
        families: Vec::new(),
        files: Vec::new(),
        daw_undo: Vec::new(),
        undo_steps: Vec::new(),
        because: Vec::new(),
        uncounted: None,
        app: provider.profile().words.app,
    };
    for (n, step) in steps.iter().enumerate() {
        let tool = step.get("tool").and_then(Value::as_str).unwrap_or_default();
        let input = step.get("input").cloned().unwrap_or_else(|| json!({}));
        let at = n + 1;
        let (allowed, daw_undo) = allowed_step(provider, &offered, at, tool, &input)?;
        if allowed.alone && steps.len() != 1 {
            return Err(refused(
                "not_allowed",
                format!("Step {at} ({tool}) must be the only step in this change. Ask for it in a separate change."),
            ));
        }
        match allowed.file_field {
            Some(field) if input.get(field).is_some() => checked.files.push((n, field)),
            _ => step_takes(ctx, n, tool, &input)?,
        }
        checked.families.push(allowed.family);
        checked.daw_undo.push(daw_undo);
        if daw_undo {
            checked.undo_steps.push(((n + 1) as u64, allowed.daw_undo_steps));
            checked.because.extend(allowed.daw_undo_because);
            checked.uncounted = checked.uncounted.or(allowed.daw_undo_uncounted);
        }
    }
    checked.alone(steps)?;
    Ok(checked)
}

/// Step `at` (`tool` with `input`) as a plugin's plan may hold it in this DAW, before what the step takes is checked: the
/// DAW's list of the steps it keeps out says why one is out, the plan tool must offer it, the allowlist must take its
/// input, and the DAW must keep an undo Kumi can use for it, unless it's marked as a change only the DAW's own Undo
/// takes back (CC-20). The step, and whether it's one of those.
fn allowed_step(
    provider: &dyn DawProvider,
    offered: &[String],
    at: usize,
    tool: &str,
    input: &Value,
) -> Result<(&'static PluginStep, bool), Value> {
    // A step the DAW keeps out of a plugin's plans says why, whether or not this DAW offers it now.
    if let Some((_, why)) = provider.plugin_refused().iter().find(|(name, _)| *name == tool) {
        return Err(refused("not_allowed", format!("Step {at}: Kumi for Hermes doesn't make {tool} here. {why}")));
    }
    if !offered.iter().any(|name| name == tool) {
        return Err(refused("unknown_step", format!("Step {at}: {tool} isn't a change this DAW makes.")));
    }
    let allowed =
        kumi_daw::plugin::check(provider.plugin_steps(), tool, input).map_err(|why| refused("not_allowed", format!("Step {at}: {why}")))?;
    // A step marked as a change only the DAW's own Undo takes back is one the DAW keeps no undo for (CC-20).
    let daw_undo = allowed.daw_undo_only(input);
    if let Some(why) = provider.plugin_refuses(tool, input).filter(|_| !daw_undo) {
        return Err(refused("not_allowed", format!("Step {at} ({tool}): {why}")));
    }
    Ok((allowed, daw_undo))
}

/// Step `n` (`tool`) takes `input`, as its own tool says: a step with none to check it against isn't run.
fn step_takes(ctx: &Ctx, n: usize, tool: &str, input: &Value) -> Result<(), Value> {
    let at = n + 1;
    let tools = ctx.link().tools();
    let Some(own) = tools.iter().find(|candidate| candidate.name() == tool) else {
        return Err(refused("invalid", format!("Step {at}: {tool}'s input can't be checked here, so it isn't run.")));
    };
    match kumi_agent::kernel::schema::problem(&Value::Object(own.input_schema()), input) {
        Some(problem) => Err(refused("invalid", format!("Step {at} ({tool}): {problem}"))),
        None => Ok(()),
    }
}

/// Each file step's source resolved (CC-11) and put in its field: the file's name for a preview, its path for the
/// dispatch, each step's input then checked as it'll be run. A dispatch whose files aren't the ones its preview pinned
/// (11b) places nothing; otherwise a host's file is copied into the artifacts first. With the sources as HISTORY keeps
/// them, and the identity of each file they named.
async fn placed(
    ctx: &Ctx,
    steps: &mut [Value],
    files: &[(usize, &'static str)],
    dispatch: Option<Vec<Value>>,
    signal: &kumi_common::abort::Signal,
) -> Result<(Vec<Value>, Vec<Value>), Value> {
    let at = |n: usize, error: OpError| refused(error.code(), format!("Step {}: {}", n + 1, error.reason()));
    let mut found = Vec::new();
    for (n, field) in files {
        let source = steps[*n]["input"][*field].clone();
        found.push((*n, *field, super::import::find(ctx, &source, signal).await.map_err(|error| at(*n, error))?));
    }
    let named: Vec<Value> = found.iter().map(|(_, _, found)| found.named.clone()).collect();
    let Some(pinned) = dispatch else {
        for (n, field, found) in &found {
            steps[*n]["input"][*field] = json!(found.name);
            let tool = steps[*n].get("tool").and_then(Value::as_str).unwrap_or_default().to_string();
            step_takes(ctx, *n, &tool, &steps[*n]["input"])?;
        }
        return Ok((Vec::new(), named));
    };
    if pinned != named {
        return Err(refused("stale", "A file this plan places changed since it was approved: preview it again."));
    }
    // Every host's file staged before any is kept: one refused leaves none of the others kept for nothing.
    let mut staged = Vec::new();
    for (n, _, found) in &found {
        staged.push(super::import::stage(ctx, found, signal).await.map_err(|error| at(*n, error))?);
    }
    let mut sources = Vec::new();
    for ((n, field, found), staged) in found.into_iter().zip(staged) {
        let (path, mut record) = super::import::keep(ctx, found, staged, signal).await.map_err(|error| at(n, error))?;
        steps[n]["input"][field] = json!(path.display().to_string());
        let tool = steps[n].get("tool").and_then(Value::as_str).unwrap_or_default().to_string();
        step_takes(ctx, n, &tool, &steps[n]["input"])?;
        record["field"] = json!(field);
        record["step"] = json!(n + 1);
        sources.push(record);
    }
    Ok((sources, named))
}

/// A native row may cover several batched steps. Keep every source those steps placed; an unmatched row keeps them all.
fn record_sources(history: &History, rows: &[Row], done: &Value, sources: &[Value], snapshots: &super::snapshots::Snapshots) {
    let mut steps_of: HashMap<&str, [bool; MOST_STEPS + 1]> = HashMap::new();
    for done in done.as_array().into_iter().flatten() {
        let (Some(change), Some(step)) = (done.get("change").and_then(Value::as_str), done.get("step").and_then(Value::as_u64)) else {
            continue;
        };
        let placed = steps_of.entry(change).or_insert([false; MOST_STEPS + 1]);
        if (1..=MOST_STEPS as u64).contains(&step) {
            placed[step as usize] = true;
        }
    }
    for row in rows {
        let own: Vec<Value> = match steps_of.get(row.label.as_str()) {
            Some(steps) => sources
                .iter()
                .filter(|source| source["step"].as_u64().is_some_and(|step| step <= MOST_STEPS as u64 && steps[step as usize]))
                .cloned()
                .collect(),
            None => sources.to_vec(),
        };
        history.placed(std::slice::from_ref(&row.id), &own, snapshots);
    }
}

async fn change(ctx: &Ctx, call: Call) -> Result<Value, OpError> {
    let args = &call.args;
    let title = args.get("title").and_then(Value::as_str).unwrap_or_default().to_string();
    let mut steps: Vec<Value> = args.get("steps").and_then(Value::as_array).cloned().unwrap_or_default();
    let key = args.get("expected_project_key").and_then(Value::as_str).unwrap_or_default();
    let Some(epoch) = epoch_of(args, "expected_epoch") else {
        return Ok(refused("invalid", "expected_epoch is the epoch an observe gave: a whole number, or its digits."));
    };
    if let Err(error) = ctx.connection() {
        return Ok(refusal(error));
    }
    let project = match ctx.link().expect(key, epoch) {
        Ok(project) => project,
        Err(error) => return Ok(refusal(error)),
    };
    let Some(plan) = ctx.link().plan_tool() else {
        return Ok(refused("unsupported", "This DAW's integration offers no plan of changes here."));
    };
    let checked = match checked(ctx, &plan, &steps) {
        Ok(checked) => checked,
        Err(refusal) => return Ok(refusal),
    };
    // The plan as the host's approval saw it, whatever `preview` says, read as the host's own digest reads it: its
    // hook previews the model's arguments as written, its handler dispatches them coerced ("4" then 4). A dispatch
    // with files must be that plan, by the same host call, placing the same files (11b).
    let approved = {
        let mut asked = args.clone();
        asked.remove("preview");
        super::mcp::digest(CHANGE.name, &loose(&asked))
    };
    let pin = pin_key(&call.host, &approved);
    let previewed = previewing(args);
    let dispatch = if previewed || checked.files.is_empty() {
        None
    } else {
        // Its own call's preview, or one that came with no call id (bound by its plan alone).
        let session = super::ctx::Host { session_id: call.host.session_id.clone(), ..Default::default() };
        let by_plan = pin_key(&session, &approved);
        let found = ctx.link().take_pin(&pin).or_else(|| if pin == by_plan { None } else { ctx.link().take_pin(&by_plan) });
        match found {
            Some((plan, files)) if plan == approved => Some(files),
            Some(_) => return Ok(refused("stale", "This plan isn't the one approved: preview it again.")),
            None => {
                return Ok(refused(
                    "stale",
                    "Kumi has no approval of this plan's files to hold it to (the runtime restarted, say): preview it again.",
                ))
            }
        }
    };
    let (sources, named) = match placed(ctx, &mut steps, &checked.files, dispatch, &call.signal).await {
        Ok(placed) => placed,
        Err(refusal) => return Ok(refusal),
    };
    if previewed {
        if !named.is_empty() {
            ctx.link().pin_files(&pin, &approved, named);
        }
        let names = ctx.link().ref_names();
        let mut preview = json!({
            "state": "preview",
            "title": title,
            "project": {"key": project.key, "epoch": project.epoch},
            "steps": preview_steps(&names, &steps, &checked),
        });
        // A change holding a step only the DAW's own Undo takes back is one Kumi undoes nothing of (CC-20).
        if let Some(note) = checked.undo_note(std::env::consts::OS, None) {
            preview["undo"] = json!("daw");
            preview["undo_note"] = json!(note);
        }
        return Ok(preview);
    }
    let asked = super::mcp::digest(CHANGE.name, args);
    let identity = CallIdentity::of(&call.host, &asked);
    let input: Map<String, Value> = [("steps".to_string(), Value::Array(steps))].into_iter().collect();
    let history = ctx.history();
    history.begin_call();
    // Every row of a change holding a step only the DAW's own Undo takes back says so, however the call ends (CC-20):
    // with no count until the plan says what it did, then with the count of that, when it did it all.
    let os = std::env::consts::OS;
    if let Some(note) = checked.undo_note(os, Some(None)) {
        history.daw_undo_call(note);
    }
    let ran = plan.execute(input, call.signal.clone()).await;
    let answer = ran.as_ref().ok().and_then(|result| serde_json::from_str::<Value>(&result.text).ok());
    if let Some(note) = checked.undo_note(os, Some(answer.as_ref())) {
        history.daw_undo_call(note);
    }
    let rows = history.end_call(identity, ctx.snapshots());
    let mut outcome = outcome(ran, &rows);
    // What the DAW said of a step can name the file it was given: its name only, as everywhere else.
    connection::names_only_said(&mut outcome.steps);
    outcome.reason = outcome.reason.map(|reason| connection::names_only_text(&reason));
    record_sources(history, &rows, &outcome.steps["done"], &sources, ctx.snapshots());
    // A row of a change only the DAW's own Undo takes back keeps what decided its count (a pad's route).
    for done in outcome.steps["done"].as_array().into_iter().flatten() {
        let (Some(change), Some(step)) = (done.get("change").and_then(Value::as_str), done.get("step").and_then(Value::as_u64)) else {
            continue;
        };
        if let Some(by) = checked.undo_by(step, done) {
            if let Some(row) = rows.iter().find(|row| row.label == change && row.reason.as_deref() == Some(DAW_UNDO)) {
                history.daw_undo_by(&row.id, by, ctx.snapshots());
            }
        }
    }
    let mut ids: Vec<String> = rows.iter().map(|row| row.id.clone()).collect();
    if outcome.state == "unsure" {
        let reason = outcome
            .reason
            .clone()
            .unwrap_or_else(|| "The DAW didn't confirm every step: look at the project before another change.".into());
        let audit = history.audit(&title, &project.key, &reason, identity, ctx.snapshots()).id;
        // Which steps ran is what's unsure: the audit row keeps every file the plan was given.
        history.placed(std::slice::from_ref(&audit), &sources, ctx.snapshots());
        ids.push(audit);
        ctx.link().note_unsure("change", &reason, "unsure");
    }
    // Every row this call recorded, its audit row included, is one change (CC-3 A2).
    history.one_change(&ids, ctx.snapshots());
    history.call_came_to(&ids, outcome.state, ctx.snapshots());
    // The copy of the project the plan kept before changing it: the answer and HISTORY say so, as standalone does.
    if let Some(copy) = &outcome.copy {
        history.copy(&ids, copy, ctx.snapshots());
    }
    // Whatever ran, the refs an earlier look gave may no longer hold: the next plan is made on a fresh look.
    if outcome.state != "refused" {
        moved_on(ctx);
    }
    let project_now = ctx.link().project().unwrap_or(project);
    super::core::publish_status(ctx, false).await;
    let mut answer = json!({
        "state": outcome.state,
        "history_ids": ids,
        "steps": outcome.steps,
        "summary": outcome.summary,
        "undo_available": rows.iter().any(|row| row.undo_available),
        "project_key": project_now.key,
        "epoch": project_now.epoch,
    });
    if let Some(reason) = outcome.reason {
        answer["reason"] = json!(reason);
    }
    if let Some(copy) = outcome.copy {
        answer["copy"] = copy;
    }
    Ok(answer)
}

/// A plan's steps as its preview says them, one line each; a step only the DAW's own Undo takes back is marked so.
fn preview_steps(names: &HashMap<String, String>, steps: &[Value], checked: &Checked) -> Vec<Value> {
    let names = shown_names(names, steps);
    steps
        .iter()
        .zip(&checked.families)
        .zip(&checked.daw_undo)
        .map(|((step, family), daw_undo)| {
            let tool = step.get("tool").and_then(Value::as_str).unwrap_or_default();
            let mut line = describe(&names, tool, family, step.get("input").unwrap_or(&Value::Null));
            if *daw_undo {
                line["undo"] = json!("daw");
            }
            line
        })
        .collect()
}

/// How a preview names the refs its steps point at: by the name the last look gave each, quoted; and what an earlier
/// step of the plan makes (`@alias`), by the name that step gives it ("the new "Reverb" (step 1)"), or by its number.
fn shown_names(names: &HashMap<String, String>, steps: &[Value]) -> HashMap<String, String> {
    let mut shown: HashMap<String, String> = names.iter().map(|(reference, name)| (reference.clone(), quoted(name, NAME))).collect();
    for (at, step) in steps.iter().enumerate() {
        let Some(alias) = step.get("as").and_then(Value::as_str) else { continue };
        let input = step.get("input").unwrap_or(&Value::Null);
        let name = input
            .get("name")
            .and_then(Value::as_str)
            .or_else(|| input.get("itemId").and_then(Value::as_str).and_then(|item| item.rsplit('/').next()))
            .filter(|name| !name.trim().is_empty());
        let said = match name {
            Some(name) => format!("the new {} (step {})", quoted(name, NAME), at + 1),
            None => format!("what step {} makes", at + 1),
        };
        shown.insert(format!("@{alias}"), said);
    }
    shown
}

/// The project may have changed: the epoch moves on, and `focus.json` gives the new one.
fn moved_on(ctx: &Ctx) {
    ctx.link().advance();
    connection::publish_focus(&ctx.handle());
}

/// What a plan did, as C1 says it.
struct Outcome {
    state: &'static str,
    steps: Value,
    summary: String,
    reason: Option<String>,
    /// The copy of the project the plan kept beside it, `{file, why}`: its file's name only, never the folder.
    copy: Option<Value>,
}

/// The plan tool's answer, with the HISTORY rows it made: `applied` when every step went through, `partial` when some
/// were refused or held back after others changed the project, `refused` when nothing changed, and `unsure` when the
/// plan stopped (the DAW may have changed and Kumi can't tell) or a row is unsure. An error from the tool itself, or
/// an error it answers after rows were made, is unsure too: never taken for "nothing changed".
fn outcome(ran: Result<kumi_types::contracts::ToolResult, kumi_types::errors::RuntimeError>, rows: &[Row]) -> Outcome {
    let summary = |rows: &[Row]| rows.iter().map(|row| row.title.clone()).collect::<Vec<_>>().join("; ");
    let result = match ran {
        Ok(result) => result,
        Err(error) => {
            return Outcome {
                state: "unsure",
                steps: Value::Array(vec![]),
                summary: summary(rows),
                reason: Some(format!("The plan stopped: {error}")),
                copy: None,
            };
        }
    };
    let Ok(Value::Object(said)) = serde_json::from_str::<Value>(&result.text) else {
        // A plain answer: refused before it ran (its checks), an error after it changed something, or done.
        return match (result.is_error, rows.is_empty()) {
            (true, true) => {
                Outcome { state: "refused", steps: Value::Array(vec![]), summary: String::new(), reason: Some(result.text), copy: None }
            }
            (true, false) => {
                Outcome { state: "unsure", steps: Value::Array(vec![]), summary: summary(rows), reason: Some(result.text), copy: None }
            }
            (false, _) => Outcome { state: "applied", steps: Value::Array(vec![]), summary: summary(rows), reason: None, copy: None },
        };
    };
    let list = |name: &str| said.get(name).and_then(Value::as_array).cloned().unwrap_or_default();
    // A plan stopped or refused after its copy was kept still says the copy: it's beside the project either way.
    let copy = said.get("copy").and_then(Value::as_str).filter(|path| !path.is_empty()).map(|path| {
        let file = std::path::Path::new(path).file_name().map(|name| name.to_string_lossy().into_owned());
        json!({"file": file, "why": said.get("copyWhy").cloned().unwrap_or(Value::Null)})
    });
    let (done, refused, held) = (list("done"), list("refused"), list("heldBack"));
    let stopped = said.get("stopped").filter(|stopped| !stopped.is_null()).cloned();
    let steps = json!({"done": done, "refused": refused, "held_back": held, "stopped": stopped, "skipped": said.get("skipped").cloned().unwrap_or(json!(0))});
    let unsure_row = rows.iter().any(|row| row.state == "unsure");
    let changed = !rows.is_empty() || done.iter().any(|row| row.get("changed").and_then(Value::as_bool).unwrap_or(true));
    let reason = || {
        let first = stopped.as_ref().or(refused.first()).and_then(|step| step.get("error")).and_then(Value::as_str);
        first.map(str::to_string)
    };
    if stopped.is_some() || unsure_row {
        return Outcome {
            state: "unsure",
            steps,
            summary: summary(rows),
            reason: reason().or(Some("The DAW didn't confirm a step.".into())),
            copy,
        };
    }
    if !changed && done.is_empty() {
        return Outcome { state: "refused", steps, summary: String::new(), reason: reason(), copy };
    }
    if !refused.is_empty() || !held.is_empty() {
        return Outcome { state: "partial", steps, summary: summary(rows), reason: reason(), copy };
    }
    Outcome { state: "applied", steps, summary: summary(rows), reason: None, copy }
}

/// The step names the plan tool takes (its schema's `steps.items.properties.tool.enum`).
pub(super) fn offered_steps(schema: &JsonObject) -> Vec<String> {
    schema
        .get("properties")
        .and_then(|properties| properties.get("steps"))
        .and_then(|steps| steps.get("items"))
        .and_then(|items| items.get("properties"))
        .and_then(|properties| properties.get("tool"))
        .and_then(|tool| tool.get("enum"))
        .and_then(Value::as_array)
        .map(|names| names.iter().filter_map(Value::as_str).map(str::to_string).collect())
        .unwrap_or_default()
}

/// One step as the host's approval says it (CC-3), on one line of at most [`LINE`] characters: its change, the values
/// it sets as given (strings quoted, a list or a group of values by their first few), then what it's on, by the names
/// the last look gave its refs, quoted and each at most [`NAME`] characters. Values come first, so a long name never
/// pushes them out of a cut line. Never a value it would have to read from the DAW.
fn describe(names: &HashMap<String, String>, tool: &str, family: &str, input: &Value) -> Value {
    let mut targets = Vec::new();
    let mut settings = Vec::new();
    if let Value::Object(fields) = input {
        for (field, value) in fields {
            match value {
                Value::String(text) if target(names, text).is_some() => targets.extend(target(names, text)),
                Value::Array(items)
                    if !items.is_empty() && items.iter().all(|item| item.as_str().is_some_and(|text| target(names, text).is_some())) =>
                {
                    targets.extend(items.iter().filter_map(|item| item.as_str().and_then(|text| target(names, text))));
                }
                _ => {
                    targets.extend(refs_in(names, value));
                    settings.push(format!("{field} {}", one_line(&shown(names, value), FIELD)));
                }
            }
        }
    }
    targets.dedup();
    let set = if settings.is_empty() { String::new() } else { format!(": {}", settings.join(", ")) };
    let on = if targets.is_empty() { String::new() } else { format!(" — on {}", targets.join(", ")) };
    let text = one_line(&format!("{}{set}{on}", capitalized(&tool.replace('_', " "))), LINE);
    json!({"tool": tool, "family": family, "effect": "write", "text": text})
}

/// A value as the approval shows it: a string quoted (a ref by its name), a list or a group of values by their first
/// [`FIRST`], with how many more.
fn shown(names: &HashMap<String, String>, value: &Value) -> String {
    let more = |n: usize| if n > FIRST { format!(", +{} more", n - FIRST) } else { String::new() };
    match value {
        Value::String(text) => target(names, text).unwrap_or_else(|| quoted(text, QUOTED)),
        Value::Number(number) => number.to_string(),
        Value::Bool(flag) => (if *flag { "on" } else { "off" }).into(),
        Value::Null => "none".into(),
        Value::Array(items) => {
            let first: Vec<String> = items.iter().take(FIRST).map(|item| shown(names, item)).collect();
            format!("[{}{}]", first.join(", "), more(items.len()))
        }
        Value::Object(fields) => {
            let first: Vec<String> = fields.iter().take(FIRST).map(|(field, item)| format!("{field} {}", shown(names, item))).collect();
            format!("{{{}{}}}", first.join(", "), more(fields.len()))
        }
    }
}

/// What a ref is called, as a preview shows it ([`shown_names`]): its name, or what a step before makes (`@name`).
fn target(names: &HashMap<String, String>, text: &str) -> Option<String> {
    names.get(text).cloned().or_else(|| text.strip_prefix('@').map(|made| format!("what {made} makes")))
}

/// The named refs inside a value, wherever they are.
fn refs_in(names: &HashMap<String, String>, value: &Value) -> Vec<String> {
    match value {
        Value::String(text) => target(names, text).into_iter().collect(),
        Value::Array(items) => items.iter().flat_map(|item| refs_in(names, item)).collect(),
        Value::Object(fields) => fields.values().flat_map(|item| refs_in(names, item)).collect(),
        _ => vec![],
    }
}

/// `text` quoted, on one line and at most `most` characters, its own quotes made single.
fn quoted(text: &str, most: usize) -> String {
    format!("\"{}\"", one_line(text, most).replace('"', "'"))
}

/// `text` on one line (every run of spaces, line breaks and control characters one space), with nothing that changes
/// how it shows unseen, at most `most` characters: a name from a sample pack is untrusted text.
pub(super) fn one_line(text: &str, most: usize) -> String {
    let shown: String = text.chars().filter(|c| !unseen(*c)).map(|c| if c.is_control() { ' ' } else { c }).collect();
    let line = shown.split_whitespace().collect::<Vec<_>>().join(" ");
    match line.char_indices().nth(most.saturating_sub(1)) {
        Some((at, _)) if line.chars().count() > most => format!("{}…", &line[..at]),
        _ => line,
    }
}

/// A character that changes how text shows without being seen: format controls (bidi overrides and isolates,
/// zero-width ones, the soft hyphen) and tag characters.
fn unseen(c: char) -> bool {
    matches!(
        c,
        '\u{00AD}'
            | '\u{061C}'
            | '\u{180E}'
            | '\u{200B}'..='\u{200F}'
            | '\u{202A}'..='\u{202E}'
            | '\u{2060}'..='\u{2064}'
            | '\u{2066}'..='\u{206F}'
            | '\u{FEFF}'
            | '\u{FFF9}'..='\u{FFFB}'
            | '\u{E0000}'..='\u{E007F}'
    )
}

fn capitalized(text: &str) -> String {
    let mut chars = text.chars();
    chars.next().map(|first| first.to_uppercase().chain(chars).collect()).unwrap_or_default()
}

pub static UNDO: Operation = Operation {
    name: "undo",
    description: "Undoes one change from HISTORY by its id, under the DAW's write lease: undone, kept (Kumi can't take it \
                  back, and says why), unsure, expired (made by an earlier runtime, or no longer undoable) or refused. \
                  With `whole_change`, every step of the change that row is part of, newest first, stopping at the first \
                  it can't undo. `preview` says what it would undo without doing it.",
    schema: undo_schema,
    effect: undo_effect,
    run: run_undo,
};

fn undo_schema() -> Value {
    json!({"type": "object", "additionalProperties": false, "required": ["history_id"], "properties": {
        "history_id": {"type": "string", "minLength": 1, "maxLength": 96},
        "preview": {"type": "boolean"},
        "whole_change": {"type": "boolean"},
        "expect_rows": {"type": "string", "pattern": "^[0-9a-f]{64}$"}
    }})
}

fn undo_effect(args: &JsonObject) -> Effect {
    if previewing(args) {
        Effect::Read
    } else {
        Effect::Write
    }
}

fn run_undo(ctx: &Ctx, call: Call) -> OpFuture<'_> {
    Box::pin(undo(ctx, call))
}

async fn undo(ctx: &Ctx, call: Call) -> Result<Value, OpError> {
    let id = call.args.get("history_id").and_then(Value::as_str).unwrap_or_default();
    let answer = |state: &str, reason: &str| json!({"state": state, "reason": reason});
    let Some(row) = ctx.history().row(id) else {
        return Ok(answer("refused", "There's no change by that id in HISTORY."));
    };
    // A script's row (CC-19): Kumi can't undo what code did, preview or not, whole or a row of it, and the DAW isn't
    // asked. It comes before CC-20's refusal and #1129's whole change, so neither reaches a script's row.
    if row.script.is_some() {
        return Ok(super::script::not_undoable(ctx, previewing(&call.args)));
    }
    // A change only the DAW's own Undo takes back is refused with its words, before anything is asked (CC-20), whole or
    // a row of it.
    if row.reason.as_deref() == Some(DAW_UNDO) {
        return Ok(daw_undo_refusal(ctx.history(), &row, previewing(&call.args)));
    }
    if call.args.get("whole_change").and_then(Value::as_bool) == Some(true) {
        return whole_change(ctx, &call, &row).await;
    }
    let why_not = cant_undo(ctx, &row);
    // The DAW must answer, with that change's project open and no return since the last look.
    let connection = match ctx.connection() {
        Ok(connection) => connection,
        Err(error) if previewing(&call.args) => return Err(error),
        Err(error) => return Ok(json!({"state": "refused", "code": error.code(), "reason": error.reason()})),
    };
    let here = ctx.link().expect_project(&row.project_key);
    if previewing(&call.args) {
        if let Err(error) = here {
            return Ok(json!({"state": "refused", "code": error.code(), "reason": error.reason()}));
        }
        return Ok(row_preview(&connection, &row, why_not.as_ref().map(|(reason, _, _)| *reason), &call).await);
    }
    if let Some((_, state, reason)) = why_not {
        return Ok(answer(state, &reason));
    }
    if let Err(error) = here {
        return Ok(json!({"state": "refused", "code": error.code(), "reason": error.reason()}));
    }
    let undone = undo_row(ctx, &connection, &row, &call).await;
    if let Some(code) = undone.code {
        return Ok(json!({"state": "refused", "code": code, "reason": undone.reason}));
    }
    if undone.state != "refused" {
        // Kumi's undo is a step of the DAW's own undo too: a change only that undo takes back is under it now.
        ctx.history().wrote(&row.project_key);
        moved_on(ctx);
    }
    super::core::publish_status(ctx, false).await;
    let mut answered = answer(undone.state, &undone.reason);
    if let Some(audit) = undone.audit {
        answered["history_ids"] = json!([audit]);
    }
    Ok(answered)
}

/// Why `row` can't be undone from here, before the DAW is asked: CC-3's reason, C1's state and the words.
fn cant_undo(ctx: &Ctx, row: &Row) -> Option<(&'static str, &'static str, String)> {
    if row.runtime_generation != ctx.history().generation() {
        let restarted = format!(
            "Kumi restarted since this change, so it can't undo it safely: undo it in {} if it's still the latest step there, or ask Hermes to set it back.",
            ctx.daw().profile().words.app
        );
        Some(("restarted", "expired", restarted))
    } else if row.state == "expired" {
        Some(("expired", "expired", "It can't be undone any more.".into()))
    } else if !row.undo_available {
        Some(("kept", "refused", "That change can't be undone now (see its state in HISTORY).".into()))
    } else {
        None
    }
}

/// What undoing `row` would do, `why_not` being why it can't: a value Kumi set is read fresh, what it is now and what the
/// undo would put back, in the DAW's own words, and why the undo would be refused.
async fn row_preview(connection: &connection::Connection, row: &Row, why_not: Option<&'static str>, call: &Call) -> Value {
    if why_not.is_none() {
        if let Ok(Some(fresh)) = connection.integration.undo_preview(&row.label, call.signal.clone()).await {
            let track = on_track(&fresh.text, row.track.as_deref());
            let said = match &fresh.refusal {
                Some(refusal) => format!("{}{track}. {}", fresh.text, refusal.reason),
                None => format!("{}{track}", fresh.text),
            };
            let mut preview = json!({"state": "preview", "text": one_line(&said, 600), "undo_available": fresh.refusal.is_none()});
            if let Some(refusal) = fresh.refusal {
                preview["reason"] = json!(refusal.code);
            }
            return preview;
        }
    }
    let text = format!("Undo {}{}", row.title, on_track(&row.title, row.track.as_deref()));
    let mut preview = json!({"state": "preview", "text": text, "undo_available": why_not.is_none()});
    if let Some(reason) = why_not {
        preview["reason"] = json!(reason);
    }
    preview
}

/// One row's undo as it came to: its state and why, the code of a refusal the DAW's guard made before anything changed,
/// and the audit row of an unsure undo.
struct RowUndone {
    state: &'static str,
    reason: String,
    code: Option<&'static str>,
    audit: Option<String>,
}

/// `row` undone: the DAW reads again what the change set right before it's asked, and one changed since is refused with
/// nothing asked, recorded, audited or moved on. An unsure undo gets its own audit row, which the answer names.
async fn undo_row(ctx: &Ctx, connection: &connection::Connection, row: &Row, call: &Call) -> RowUndone {
    let undone = match connection.integration.guarded_undo(&row.label, call.signal.clone()).await {
        Ok(Err(refusal)) => {
            return RowUndone { state: "refused", reason: one_line(&refusal.reason, 600), code: Some(refusal.code), audit: None }
        }
        Ok(Ok(record)) => Ok(record),
        Err(error) => Err(error),
    };
    let (state, reason) = match undone {
        Ok(record) => {
            let after = ctx.history().record(&record, &row.project_key, ctx.snapshots());
            match record.state {
                ChangeState::Undone => ("undone", String::new()),
                ChangeState::Kept => ("kept", after.note.unwrap_or_else(|| "Kumi can't take it back.".into())),
                ChangeState::Unsure => ("unsure", after.note.unwrap_or_else(|| "The DAW didn't confirm the undo.".into())),
                ChangeState::Expired => ("expired", after.note.unwrap_or_else(|| "It can't be undone any more.".into())),
                // Unchanged: nothing was undone (the DAW was busy, or a check refused it).
                ChangeState::Applied | ChangeState::Heard => {
                    ("refused", after.note.unwrap_or_else(|| "Nothing was undone: try again in a moment.".into()))
                }
            }
        }
        Err(error) => ("unsure", format!("The undo stopped: {error}")),
    };
    let mut audit = None;
    if state == "unsure" {
        let asked = super::mcp::digest(UNDO.name, &call.args);
        let identity = CallIdentity::of(&call.host, &asked);
        let audited = ctx.history().audit(&format!("Undo {}", row.title), &row.project_key, &reason, identity, ctx.snapshots());
        ctx.history().call_came_to(std::slice::from_ref(&audited.id), "unsure", ctx.snapshots());
        ctx.link().note_unsure("undo", &reason, "unsure");
        audit = Some(audited.id);
    }
    RowUndone { state, reason, code: None, audit }
}

/// The rows of the change `row` is part of, newest first (CC-3 A2): those with its `change_id`, or the row alone.
fn change_rows(ctx: &Ctx, row: &Row) -> Vec<Row> {
    match &row.change_id {
        Some(change) => ctx.history().rows(usize::MAX).into_iter().filter(|other| other.change_id.as_ref() == Some(change)).collect(),
        None => vec![row.clone()],
    }
}

/// The set of rows an approval binds (CC-3 A2-c): the lower-case hex SHA-256 of `<history_id>:<state>`, a line a row,
/// newest first, joined by `\n`.
fn rows_digest(rows: &[Row]) -> String {
    use sha2::{Digest, Sha256};
    let lines: Vec<String> = rows.iter().map(|row| format!("{}:{}", row.id, row.state)).collect();
    format!("{:x}", Sha256::digest(lines.join("\n").as_bytes()))
}

/// Every row of the change `row` is part of, newest first (CC-3 A2): what undoing them would do, or each undone in turn
/// under the one write lease, as one undo of it would be, until the first that isn't; the rows after it aren't tried, as
/// an older one can depend on a newer (a track's creation, and its volume since). A row undone already is skipped.
async fn whole_change(ctx: &Ctx, call: &Call, row: &Row) -> Result<Value, OpError> {
    let rows = change_rows(ctx, row);
    // A change only the DAW's own Undo takes back is undone of nothing, whichever of its rows says so (CC-20's 20e).
    if let Some(daw_undo) = rows.iter().find(|row| row.reason.as_deref() == Some(DAW_UNDO)) {
        return Ok(daw_undo_refusal(ctx.history(), daw_undo, previewing(&call.args)));
    }
    let digest = rows_digest(&rows);
    let previewed = previewing(&call.args);
    // The real call names the set its approval listed (the preview's `rows_digest`), and a set that's changed since (a
    // row undone by id, or expired) is refused before the DAW is asked anything.
    if !previewed {
        match call.args.get("expect_rows").and_then(Value::as_str) {
            None => {
                return Err(OpError::Invalid {
                    reason: "Undoing a whole change takes expect_rows, the rows_digest its preview gave: preview it first.".into(),
                })
            }
            Some(expected) if expected != digest => {
                return Ok(
                    json!({"state": "refused", "code": "stale", "reason": "This change's steps aren't as they were when its undo was approved: preview it again."}),
                );
            }
            Some(_) => {}
        }
    }
    let connection = match ctx.connection() {
        Ok(connection) => connection,
        Err(error) if previewed => return Err(error),
        Err(error) => return Ok(json!({"state": "refused", "code": error.code(), "reason": error.reason()})),
    };
    if let Err(error) = ctx.link().expect_project(&row.project_key) {
        return Ok(json!({"state": "refused", "code": error.code(), "reason": error.reason()}));
    }
    if previewed {
        let mut listed = Vec::new();
        let mut texts = Vec::new();
        let mut available = true;
        for row in &rows {
            let mut line = json!({"history_id": row.id, "title": row.title, "family": row.family, "state": row.state});
            if row.state == "undone" {
                line["text"] = json!(format!("{} is undone already", row.title));
                line["undo_available"] = json!(false);
            } else {
                let why_not = cant_undo(ctx, row);
                let preview = row_preview(&connection, row, why_not.as_ref().map(|(reason, _, _)| *reason), call).await;
                available &= preview["undo_available"] == true;
                texts.push(preview["text"].as_str().unwrap_or_default().to_string());
                line["text"] = preview["text"].clone();
                line["undo_available"] = preview["undo_available"].clone();
                if let Some(reason) = preview.get("reason") {
                    line["reason"] = reason.clone();
                }
            }
            listed.push(line);
        }
        let text = match texts.as_slice() {
            [] => "Every step of this change is undone already.".to_string(),
            [one] => one.clone(),
            many => one_line(&format!("Undo {} steps: {}", many.len(), many.join("; ")), 600),
        };
        return Ok(json!({
            "state": "preview",
            "text": text,
            "undo_available": available && !texts.is_empty(),
            "rows_digest": digest,
            "rows": listed,
        }));
    }
    let mut answered = Vec::new();
    let mut audits = Vec::new();
    // The first row that isn't undone: the undo stops there.
    let mut stopped: Option<(&'static str, String)> = None;
    let (mut moved, mut some_undone) = (false, false);
    for row in &rows {
        if stopped.is_some() {
            answered.push(json!({"history_id": row.id, "state": "not_tried"}));
            continue;
        }
        if row.state == "undone" {
            some_undone = true;
            answered.push(json!({"history_id": row.id, "state": "undone"}));
            continue;
        }
        let mut line = json!({"history_id": row.id});
        let (state, reason) = match cant_undo(ctx, row) {
            Some((_, state, reason)) => (state, reason),
            None => {
                let undone = undo_row(ctx, &connection, row, call).await;
                // The DAW was asked, and may have changed, unless its guard refused before anything did.
                moved |= undone.code.is_none() && undone.state != "refused";
                audits.extend(undone.audit);
                if let Some(code) = undone.code {
                    line["code"] = json!(code);
                }
                (undone.state, undone.reason)
            }
        };
        line["state"] = json!(state);
        if state == "undone" {
            some_undone = true;
        } else {
            line["reason"] = json!(reason);
            stopped = Some((state, reason));
        }
        answered.push(line);
    }
    if moved {
        // Kumi's undo is a step of the DAW's own undo too: a change only that undo takes back is under it now.
        ctx.history().wrote(&row.project_key);
        moved_on(ctx);
    }
    super::core::publish_status(ctx, false).await;
    let mut whole = match stopped {
        None => json!({"state": "undone"}),
        Some((state, reason)) => json!({"state": if some_undone { "partly_undone" } else { state }, "reason": reason}),
    };
    whole["rows"] = json!(answered);
    if !audits.is_empty() {
        whole["history_ids"] = json!(audits);
    }
    Ok(whole)
}

/// The answer to an undo of a change only the DAW's own Undo takes back (CC-20's 20e): refused with the change's words,
/// or a preview saying so (CC-3's `reason`), with where the change stands in that undo when that's known.
fn daw_undo_refusal(history: &History, row: &Row, previewed: bool) -> Value {
    let note = row.note.clone().unwrap_or_default();
    let mut answer = if previewed {
        let text = format!("Undo {}{}", row.title, on_track(&row.title, row.track.as_deref()));
        json!({"state": "preview", "text": text, "undo_available": false, "reason": DAW_UNDO, "note": note})
    } else {
        json!({"state": "refused", "code": DAW_UNDO, "reason": note})
    };
    if let Some(place) = history.daw_undo_place(row) {
        answer["daw_undo"] = json!(place);
    }
    answer
}

pub static HISTORY: Operation = Operation {
    name: "history",
    description: "Kumi's HISTORY of the changes made through this runtime, newest first: each with its id (for undo), \
                  title, family, state and whether it can be undone. Changes nothing.",
    schema: history_schema,
    effect: super::ctx::reads,
    run: run_history,
};

/// A HISTORY row as `history` gives it: without the host call it came from or the identity of each file it placed,
/// which `history.json` keeps for the runtime and the plugin.
fn shown_row(mut row: Value) -> Value {
    if let Value::Object(fields) = &mut row {
        for kept in ["profile_id", "session_id", "tool_call_id", "digest"] {
            fields.remove(kept);
        }
        if let Some(Value::Array(sources)) = fields.get_mut("sources") {
            for source in sources.iter_mut().filter_map(Value::as_object_mut) {
                source.remove("identity");
            }
        }
    }
    row
}

/// `args` as the host's digest reads them (CC-5's W2 note): every number in its plain form and every truth value as
/// lower-case text, so `"4"`, `4` and `4.0` are the same, and `"true"` and `true`.
fn loose(args: &Map<String, Value>) -> Map<String, Value> {
    fn plain(value: &Value) -> Value {
        match value {
            Value::Number(number) => Value::String(plain_number(number.as_f64().unwrap_or_default(), &number.to_string())),
            Value::Bool(flag) => Value::String(flag.to_string()),
            Value::String(text) => match text.parse::<f64>() {
                Ok(number) if number.is_finite() && !text.trim().is_empty() => Value::String(plain_number(number, text)),
                _ if text.eq_ignore_ascii_case("true") || text.eq_ignore_ascii_case("false") => Value::String(text.to_ascii_lowercase()),
                _ => value.clone(),
            },
            Value::Array(items) => Value::Array(items.iter().map(plain).collect()),
            Value::Object(fields) => Value::Object(fields.iter().map(|(key, field)| (key.clone(), plain(field))).collect()),
            Value::Null => Value::Null,
        }
    }
    fn plain_number(number: f64, written: &str) -> String {
        if number.fract() == 0.0 && number.abs() < 9_007_199_254_740_992.0 {
            format!("{}", number as i64)
        } else if written.parse::<f64>().is_ok() {
            format!("{number}")
        } else {
            written.to_string()
        }
    }
    args.iter().map(|(key, value)| (key.clone(), plain(value))).collect()
}

/// The call a preview's approval is for: the host call (its profile, session and id), as the hook's preview and the
/// handler's dispatch both carry it; a call without an id by its plan, in its session.
fn pin_key(host: &super::ctx::Host, approved: &str) -> String {
    match &host.tool_call_id {
        Some(id) => format!(
            "call\u{1f}{}\u{1f}{}\u{1f}{id}",
            host.profile_id.as_deref().unwrap_or_default(),
            host.session_id.as_deref().unwrap_or_default()
        ),
        None => format!("plan\u{1f}{}\u{1f}{approved}", host.session_id.as_deref().unwrap_or_default()),
    }
}

fn history_schema() -> Value {
    json!({"type": "object", "additionalProperties": false, "properties": {
        "limit": {"type": "integer", "minimum": 1, "maximum": super::history::KEPT}
    }})
}

fn run_history(ctx: &Ctx, call: Call) -> OpFuture<'_> {
    Box::pin(async move {
        let limit = call.args.get("limit").and_then(Value::as_u64).unwrap_or(20) as usize;
        let history = ctx.history();
        let items: Vec<Value> = history.rows(limit).iter().filter_map(|row| history.row_json(row)).map(shown_row).collect();
        Ok(json!({"items": items}))
    })
}

/// " on <track>" after an undo's words, unless they name the track already, as a change's title often does ("1 Drums
/// volume …", "Simpler on 1 Drums now plays …"): the track is said once.
fn on_track(words: &str, track: Option<&str>) -> String {
    let Some(track) = track.filter(|track| !track.is_empty()) else { return String::new() };
    let named = words.match_indices(track).any(|(at, _)| {
        let before = words[..at].chars().next_back();
        let after = words[at + track.len()..].chars().next();
        !before.is_some_and(char::is_alphanumeric) && !after.is_some_and(char::is_alphanumeric)
    });
    if named {
        String::new()
    } else {
        format!(" on {track}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kumi_daw::plugin::{daw_undo_note, DawUndoCount, DawUndoSteps};
    use kumi_types::contracts::ToolResult;

    /// An undo's words say the track once: not again when the change's title names it, and not for a name that's only
    /// part of another word in it.
    #[test]
    fn an_undo_says_its_track_once() {
        let sample = "Simpler on 1 Drums now plays “Snare 909 Tune7”";
        assert_eq!(format!("Undo {sample}{}", on_track(sample, Some("1 Drums"))), format!("Undo {sample}"));
        assert_eq!(on_track("volume -4.0 dB → -6.0 dB", Some("1 Drums")), " on 1 Drums");
        assert_eq!(on_track("Drumset volume up", Some("Drums")), " on Drums", "Drums inside Drumset isn't the track");
        assert_eq!(on_track("Tempo 120 → 124 BPM", None), "");
    }

    fn row(state: &str) -> Row {
        Row {
            id: "h-g-c1".into(),
            label: "c1".into(),
            project_key: "p-1".into(),
            runtime_generation: "g".into(),
            title: "Drums −6 → −4 dB".into(),
            family: "mixer".into(),
            at: 0,
            state: state.into(),
            undo_available: state == "applied",
            track: None,
            note: None,
            reason: None,
            profile_id: None,
            session_id: None,
            tool_call_id: None,
            digest: None,
            call_outcome: None,
            change_id: None,
            daw_undo_by: None,
            copy: None,
            sources: None,
            script: None,
        }
    }

    #[test]
    fn every_source_of_a_twelve_pad_batch_stays_with_its_own_history_row() {
        let dir = tempfile::tempdir().unwrap();
        let state = dir.path().join("state");
        super::super::ctx::make_private_dir(&state).unwrap();
        let rows = [
            row("kept"),
            Row { id: "h-g-c2".into(), label: "c2".into(), ..row("applied") },
            Row { id: "h-g-audit".into(), label: "audit".into(), ..row("unsure") },
        ];
        std::fs::write(state.join("history.json"), serde_json::to_vec(&json!({"items": &rows})).unwrap()).unwrap();
        let history = History::new(&state, "g".into());
        let snapshots = super::super::snapshots::Snapshots::new(state.clone());
        let sources: Vec<Value> =
            (1..=13).map(|step| json!({"step":step, "file":format!("Pad {}.wav", step + 35), "artifact_id":format!("a-{step}")})).collect();
        let done: Vec<Value> = (1..=13).map(|step| json!({"step":step, "change":if step <= 12 { "c1" } else { "c2" }})).collect();
        record_sources(&history, &rows, &json!(done), &sources, &snapshots);
        snapshots.flush();
        let saved = History::new(&state, "next".into());
        assert_eq!(saved.row("h-g-c1").unwrap().sources.as_deref(), Some(&sources[..12]), "no pad's source is overwritten by a later pad");
        assert_eq!(saved.row("h-g-c2").unwrap().sources.as_deref(), Some(&sources[12..]), "another change keeps only its own source");
        assert_eq!(
            saved.row("h-g-audit").unwrap().sources.as_deref(),
            Some(sources.as_slice()),
            "an unsure row keeps every possible source"
        );
    }

    fn said(text: Value, is_error: bool) -> Result<ToolResult, kumi_types::errors::RuntimeError> {
        Ok(ToolResult { text: text.to_string(), is_error, ..Default::default() })
    }

    #[test]
    fn a_plans_answer_says_applied_partial_refused_or_unsure() {
        let done = json!({"step": 1, "changed": true, "change": "c1"});
        assert_eq!(outcome(said(json!({"done": [done]}), false), &[row("applied")]).state, "applied");
        let partial = json!({"done": [done], "refused": [{"step": 2, "tool": "set_mixer", "error": "no such track"}]});
        let partial = outcome(said(partial, true), &[row("applied")]);
        assert_eq!((partial.state, partial.reason.as_deref()), ("partial", Some("no such track")));
        let none = json!({"done": [], "refused": [{"step": 1, "tool": "set_mixer", "error": "no such track"}]});
        assert_eq!(outcome(said(none, true), &[]).state, "refused");
        let stopped = json!({"done": [done], "stopped": {"step": 2, "tool": "set_mixer", "error": "Live didn't answer"}});
        assert_eq!(outcome(said(stopped, true), &[row("applied")]).state, "unsure", "a stop may have changed the project");
        assert_eq!(outcome(said(json!({"done": [done]}), false), &[row("unsure")]).state, "unsure");
        assert_eq!(
            outcome(Err(kumi_types::errors::RuntimeError::plain("lost")), &[]).state,
            "unsure",
            "an error after it began isn't 'nothing changed'"
        );
        let checked = outcome(Ok(ToolResult { text: "steps: give 1 to 20".into(), is_error: true, ..Default::default() }), &[]);
        assert_eq!(checked.state, "refused");
        let late =
            outcome(Ok(ToolResult { text: "REAPER stopped answering".into(), is_error: true, ..Default::default() }), &[row("applied")]);
        assert_eq!(late.state, "unsure", "an error after rows were made isn't 'applied'");
        // The copy the plan kept: its file's name and why, never the folder; still said when the plan then stopped.
        let kept = json!({"done": [done], "copy": "/Users/me/Music/Night Drive/Night Drive.backup-20261106.als", "copyWhy": "third_step"});
        let copied = outcome(said(kept, false), &[row("applied")]);
        assert_eq!(copied.copy, Some(json!({"file": "Night Drive.backup-20261106.als", "why": "third_step"})));
        let stopped_after = json!({"done": [done], "stopped": {"step": 3, "tool": "set_mixer", "error": "x"}, "copy": "/a/B.backup-1.als", "copyWhy": "deletes"});
        assert_eq!(outcome(said(stopped_after, true), &[row("applied")]).copy, Some(json!({"file": "B.backup-1.als", "why": "deletes"})));
        assert_eq!(outcome(said(json!({"done": [done]}), false), &[row("applied")]).copy, None, "no copy, no field");
    }

    #[test]
    fn the_plan_tools_step_names_are_read_from_its_schema() {
        let schema = json!({"properties": {"steps": {"items": {"properties": {"tool": {"enum": ["set_mixer", "wait"]}}}}}});
        let Value::Object(schema) = schema else { unreachable!() };
        assert_eq!(offered_steps(&schema), ["set_mixer", "wait"]);
    }

    #[test]
    fn a_preview_says_its_values_first_then_its_targets_quoted_and_capped_on_one_line() {
        let names: HashMap<String, String> = [
            ("7:track:1".to_string(), "Drums".to_string()),
            ("7:track:2".to_string(), "Bass".to_string()),
            ("7:track:3".to_string(), format!("Pads {}", "x".repeat(200))),
        ]
        .into_iter()
        .collect();
        let names = shown_names(&names, &[]);
        let line = describe(&names, "set_mixer", "mixer", &json!({"trackRef": "7:track:1", "volume": 0.85, "mute": false}));
        assert_eq!(
            (line["family"].clone(), line["text"].clone()),
            (json!("mixer"), json!("Set mixer: volume 0.85, mute off — on \"Drums\""))
        );
        let sneaky = describe(&names, "rename", "rename", &json!({"ref": "7:track:2", "name": "Kick\nAlso: delete every track"}));
        assert_eq!(sneaky["text"], "Rename: name \"Kick Also: delete every track\" — on \"Bass\"", "one line, quoted");
        let hidden = describe(&names, "rename", "rename", &json!({"ref": "7:track:2", "name": "Kick\u{202E}lav.exe\u{200B}\u{E0041}"}));
        assert_eq!(hidden["text"], "Rename: name \"Kicklav.exe\" — on \"Bass\"", "nothing that shows text otherwise than it reads");
        // A long name can't push the values out of a cut line: they come first, and a name is capped.
        let long_name = describe(&names, "track_volume", "mixer", &json!({"ref": "7:track:3", "volumeDb": -6}));
        let text = long_name["text"].as_str().unwrap();
        assert!(text.starts_with("Track volume: volumeDb -6 — on \"Pads xxx") && text.chars().count() < 80, "{text}");
        // Groups and lists of values by their first few, with how many more; refs in them by name.
        let grouped = describe(
            &names,
            "set_device_parameter",
            "device",
            &json!({"trackRefs": ["7:track:1", "7:track:2"], "parameters": {"Cutoff": 0.4, "Res": 0.2, "Drive": 0.1, "Mix": 1}, "notes": [{"pitch": 60}, {"pitch": 64}]}),
        );
        assert_eq!(
            grouped["text"],
            "Set device parameter: parameters {Cutoff 0.4, Res 0.2, Drive 0.1, +1 more}, notes [{pitch 60}, {pitch 64}] — on \"Drums\", \"Bass\""
        );
        let long = describe(&names, "rename", "rename", &json!({"ref": "x", "name": "a".repeat(500), "other": "b".repeat(500)}));
        assert!(long["text"].as_str().unwrap().chars().count() <= LINE, "{long}");
        // What an earlier step makes is named by that step's own name for it, and the step: never by its alias.
        let plan = [
            json!({"tool": "track_add", "input": {"name": "Reverb"}, "as": "rev"}),
            json!({"tool": "fx_add", "input": {"ref": "@rev", "name": "ReaVerbate (Cockos)"}}),
            json!({"tool": "load_device", "input": {"itemId": "audio_effects/Reverb"}, "as": "verb"}),
            json!({"tool": "track_add", "input": {}, "as": "blank"}),
        ];
        let made = shown_names(&HashMap::new(), &plan);
        let fx = describe(&made, "fx_add", "device", &plan[1]["input"]);
        assert_eq!(fx["text"], "Fx add: name \"ReaVerbate (Cockos)\" — on the new \"Reverb\" (step 1)");
        assert_eq!(made["@verb"], "the new \"Reverb\" (step 3)", "a loaded device by its Browser name");
        assert_eq!(made["@blank"], "what step 4 makes");
    }

    /// Live gives its `refs` in its long form, while its steps take the short refs its look shows: what a step is on is
    /// named by what the look showed, Main by its name and a device with the track it's on.
    #[test]
    fn a_live_approval_names_what_its_steps_are_on_by_the_refs_the_look_showed() {
        let context = json!({
            "selectedInLive": {"device": {"ref": "device:61", "name": "EQ Eight"}},
            "tracks": [{"ref": "track:7", "name": "Main", "type": "main", "devices": [{"ref": "device:61", "name": "EQ Eight", "chains": []}]}],
        });
        let Value::Object(context) = context else { unreachable!() };
        let names = shown_names(&crate::serve::connection::look_names(&context), &[]);
        let load = describe(&names, "load_device", "device", &json!({"trackRef": "track:7", "itemId": "audio_effects/Glue Compressor"}));
        assert_eq!(load["text"], "Load device: itemId \"audio_effects/Glue Compressor\" — on \"Main\"");
        let set = describe(
            &names,
            "set_device_parameter",
            "parameter",
            &json!({"deviceRef": "device:61", "parameter": "Gain", "value": "-3 dB"}),
        );
        assert_eq!(
            set["text"], "Set device parameter: parameter \"Gain\", value \"-3 dB\" — on \"EQ Eight (on Main)\"",
            "with its track, though the selection showed it alone first"
        );
    }

    #[test]
    fn a_preview_marks_only_the_steps_only_the_daws_undo_takes_back() {
        let steps = [
            json!({"tool": "set_mixer", "input": {"trackRef": "7:track:1", "volume": 0.5}}),
            json!({"tool": "edit_rack", "input": {"rackRef": "7:device:1", "action": "add-chain"}}),
        ];
        let checked = Checked {
            families: vec!["mixer", "device"],
            files: Vec::new(),
            daw_undo: vec![false, true],
            undo_steps: vec![(2, DawUndoCount::Fixed(DawUndoSteps::PLAN))],
            because: Vec::new(),
            uncounted: None,
            app: "Live",
        };
        let lines = preview_steps(&HashMap::new(), &steps, &checked);
        assert_eq!(lines.iter().map(|line| line.get("undo").cloned()).collect::<Vec<_>>(), [None, Some(json!("daw"))]);
        // The key the note names is the one on the machine Kumi runs on.
        let note = daw_undo_note(Some(1), "Live", "macos");
        assert_eq!(checked.undo_note("macos", None), Some(note.clone()));
        assert!(checked.undo_note("windows", None).unwrap().contains("Live's own Undo (Ctrl+Z)"));
        // CC-20's words come first, then each reason the marked steps give, once.
        let reasons = Checked { because: vec!["Kumi can't tell.", "Kumi can't tell."], ..checked };
        assert_eq!(reasons.undo_note("macos", None), Some(format!("{note} Kumi can't tell.")));
        assert_eq!(Checked { daw_undo: vec![false, false], ..reasons }.undo_note("macos", None), None, "no marked step, no note");
    }

    #[test]
    fn the_count_of_presses_is_what_the_plan_did_and_none_after_a_run_that_stopped() {
        let pad = DawUndoSteps { in_plan: true, own: 1 };
        const BY_ROUTE: DawUndoCount =
            DawUndoCount::By("route", &[("hotswap", DawUndoSteps { in_plan: true, own: 1 }), ("chain", DawUndoSteps::PLAN)]);
        let pads = |undo_steps: Vec<(u64, DawUndoCount)>| Checked {
            families: vec!["device"; undo_steps.len()],
            files: Vec::new(),
            daw_undo: vec![true; undo_steps.len()],
            undo_steps,
            because: Vec::new(),
            uncounted: None,
            app: "Live",
        };
        let fixed = pads(vec![(1, DawUndoCount::Fixed(pad)), (2, DawUndoCount::Fixed(pad))]);
        let done = |routes: &[&str]| json!({"done": routes.iter().enumerate().map(|(n, route)| json!({"step": n + 1, "route": route})).collect::<Vec<_>>()});
        // Before it runs, as the steps say; once it ran all of it, the same; two pads are three presses.
        assert_eq!((fixed.presses(None), fixed.presses(Some(Some(&done(&["hotswap", "hotswap"]))))), (Some(3), Some(3)));
        // A run that stopped, refused or held a step back, or gave no answer to read, says no number.
        let stopped = json!({"done": [{"step": 1}], "stopped": {"step": 2, "error": "Live didn't answer"}});
        let refused = json!({"done": [{"step": 1}], "refused": [{"step": 2, "error": "no such pad"}]});
        for ran in [Some(&stopped), Some(&refused), Some(&json!({"done": [{"step": 1}]})), None] {
            assert_eq!(fixed.presses(Some(ran)), None, "{ran:?}");
        }
        assert!(fixed.undo_note("macos", Some(Some(&stopped))).is_some_and(|note| !note.chars().any(|c| c.is_ascii_digit())));
        // A route-decided step is counted by the route the plan reports, unknown before it runs or for an untraced one.
        let routed = pads(vec![(1, BY_ROUTE), (2, BY_ROUTE)]);
        assert_eq!(routed.presses(None), None);
        assert_eq!(routed.presses(Some(Some(&done(&["hotswap", "chain"])))), Some(2));
        assert_eq!(routed.presses(Some(Some(&done(&["hotswap", "elsewhere"])))), None);
        // While the count isn't known, a step's own words say how to take it back; once it is, the count does.
        let usually = Checked { uncounted: Some("Usually two presses of {key}."), ..routed };
        assert_eq!(usually.undo_note("macos", None), Some("Usually two presses of Cmd+Z.".to_string()));
        let counted = usually.undo_note("macos", Some(Some(&done(&["hotswap", "chain"])))).unwrap();
        assert_eq!(counted, daw_undo_note(Some(2), "Live", "macos"));
        // Each row keeps what decided its step's count, from that step's done entry; a fixed count keeps nothing.
        assert_eq!(usually.undo_by(2, &json!({"step": 2, "route": "chain"})), Some(json!({"route": "chain"})));
        assert_eq!(fixed.undo_by(1, &json!({"step": 1, "route": "hotswap"})), None);
    }

    #[test]
    fn a_change_only_the_daws_undo_takes_back_holds_no_other_kind_of_step() {
        let chain = json!({"tool": "edit_rack", "input": {"rackRef": "7:device:1", "action": "add-chain"}});
        let load = json!({"tool": "load_device", "input": {"trackRef": "7:track:1", "itemId": "audio_effects/Utility"}});
        let checked = |daw_undo: Vec<bool>| Checked {
            families: vec!["device"; daw_undo.len()],
            files: Vec::new(),
            daw_undo,
            undo_steps: Vec::new(),
            because: Vec::new(),
            uncounted: None,
            app: "Live",
        };
        // Live keeps a load as an undo step of its own, so its Undo would take a mixed change back one part at a time.
        let refused = checked(vec![false, true]).alone(&[load.clone(), chain.clone()]).unwrap_err();
        assert_eq!((refused["state"].clone(), refused["code"].clone()), (json!("refused"), json!("not_allowed")));
        assert!(
            refused["reason"].as_str().is_some_and(|reason| reason.starts_with("Step 2 (edit_rack) is a change only Live's own Undo")),
            "{refused}"
        );
        assert!(checked(vec![true, true]).alone(&[chain.clone(), chain]).is_ok(), "chains alone");
        assert!(checked(vec![false, false]).alone(&[load.clone(), load]).is_ok(), "nothing marked");
    }

    #[test]
    fn an_undo_of_a_change_only_the_daws_undo_takes_back_is_refused_with_its_words() {
        let dir = tempfile::tempdir().unwrap();
        let history = History::new(dir.path(), "g".into());
        let note = "Kumi can't undo this change. Live's own Undo (Cmd+Z) takes all of it back while it's still Live's latest step.";
        let row = Row { reason: Some(DAW_UNDO.into()), note: Some(note.into()), undo_available: false, ..row("applied") };
        let preview = daw_undo_refusal(&history, &row, true);
        assert_eq!(
            (preview["state"].clone(), preview["undo_available"].clone(), preview["reason"].clone(), preview["note"].clone()),
            (json!("preview"), json!(false), json!(DAW_UNDO), json!(note))
        );
        let refused = daw_undo_refusal(&history, &row, false);
        assert_eq!(
            (refused["state"].clone(), refused["code"].clone(), refused["reason"].clone()),
            (json!("refused"), json!(DAW_UNDO), json!(note))
        );
        assert!(refused.get("daw_undo").is_none(), "where it stands isn't known to a runtime that didn't make it");
    }

    #[test]
    fn the_live_steps_left_to_lives_own_undo_are_a_racks_new_chain_and_a_device_move() {
        let live = crate::daw::provider(kumi_types::daw::DawId::Ableton).unwrap();
        let offered: Vec<String> = ["edit_rack", "import_audio", "load_device", "move_device", "move_device_to"].map(String::from).to_vec();
        let step = |tool, input| allowed_step(live, &offered, 1, tool, &input);
        let chain = step("edit_rack", json!({"rackRef": "1:device:0:0", "action": "add-chain"}));
        assert!(chain.is_ok_and(|(allowed, daw_undo)| allowed.tool == "edit_rack" && daw_undo), "through, as one Live's Undo takes back");
        for action in ["delete-variation", "remove-macro", "insert-chain"] {
            let refused = step("edit_rack", json!({"rackRef": "1:device:0:0", "action": action})).unwrap_err();
            assert_eq!(refused["code"], "not_allowed", "{action}: {refused}");
        }
        // A step Live keeps no undo for and that isn't marked stays refused: a sound into a take lane.
        let lane = json!({"sample": {"sound_id": "s-r1-1", "map_revision": "r1"}, "takeLaneRef": "1:take_lane:0:0", "position": 4});
        assert_eq!(step("import_audio", lane).unwrap_err()["code"], "not_allowed");
        // A device's move is through, as one only Live's Undo takes back (#1139); so a load and the move that places it
        // are two changes: the load Kumi undoes, the move Live's Undo takes back.
        let load = json!({"tool": "load_device", "input": {"trackRef": "1:track:0", "itemId": "audio_effects/Utility"}, "as": "util"});
        let place = json!({"tool": "move_device", "input": {"deviceRef": "@util", "index": 0}});
        let flags: Vec<bool> =
            [&load, &place].iter().map(|plan| step(plan["tool"].as_str().unwrap(), plan["input"].clone()).unwrap().1).collect();
        assert_eq!(flags, [false, true]);
        assert!(step("move_device_to", json!({"deviceRef": "1:device:0:1", "targetTrackRef": "1:track:2", "index": 0})).unwrap().1);
        let checked = Checked {
            families: vec!["device"; 2],
            files: Vec::new(),
            daw_undo: flags,
            undo_steps: Vec::new(),
            because: Vec::new(),
            uncounted: None,
            app: "Live",
        };
        let refused = checked.alone(&[load, place]).unwrap_err();
        assert!(refused["reason"].as_str().is_some_and(|reason| reason.starts_with("Step 2 (move_device)")), "{refused}");
    }
}
