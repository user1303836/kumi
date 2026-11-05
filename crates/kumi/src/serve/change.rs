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
use super::history::{CallIdentity, Row};
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

/// A plan's steps as checked: each one's HISTORY family, and its file field when it places a file (CC-11).
struct Checked {
    families: Vec<&'static str>,
    files: Vec<(usize, &'static str)>,
}

/// A plan's steps, each checked against the DAW's allowlist, what the DAW says it keeps no undo for, and what the step
/// takes. A file step's field holds a source, so its input is checked once the source is resolved to a file.
fn checked(ctx: &Ctx, plan: &Rc<dyn KernelTool>, steps: &[Value]) -> Result<Checked, Value> {
    let Some(provider) = crate::daw::provider(ctx.daw()) else {
        return Err(refused("unsupported", "This runtime's DAW isn't one this build integrates."));
    };
    let offered = offered_steps(&plan.input_schema());
    let mut checked = Checked { families: Vec::new(), files: Vec::new() };
    for (n, step) in steps.iter().enumerate() {
        let tool = step.get("tool").and_then(Value::as_str).unwrap_or_default();
        let input = step.get("input").cloned().unwrap_or_else(|| json!({}));
        let at = n + 1;
        if !offered.iter().any(|name| name == tool) {
            return Err(refused("unknown_step", format!("Step {at}: {tool} isn't a change this DAW makes.")));
        }
        let allowed = kumi_daw::plugin::check(provider.plugin_steps(), tool, &input)
            .map_err(|why| refused("not_allowed", format!("Step {at}: {why}")))?;
        if let Some(why) = provider.plugin_refuses(tool, &input) {
            return Err(refused("not_allowed", format!("Step {at} ({tool}): {why}")));
        }
        match allowed.file_field {
            Some(field) if input.get(field).is_some() => checked.files.push((n, field)),
            _ => step_takes(ctx, n, tool, &input)?,
        }
        checked.families.push(allowed.family);
    }
    Ok(checked)
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
        let described: Vec<Value> = steps
            .iter()
            .zip(&checked.families)
            .map(|(step, family)| {
                let tool = step.get("tool").and_then(Value::as_str).unwrap_or_default();
                describe(&names, tool, family, step.get("input").unwrap_or(&Value::Null))
            })
            .collect();
        return Ok(json!({
            "state": "preview",
            "title": title,
            "project": {"key": project.key, "epoch": project.epoch},
            "steps": described,
        }));
    }
    let asked = super::mcp::digest(CHANGE.name, args);
    let identity = CallIdentity::of(&call.host, &asked);
    let input: Map<String, Value> = [("steps".to_string(), Value::Array(steps))].into_iter().collect();
    let history = ctx.history();
    history.begin_call();
    let ran = plan.execute(input, call.signal.clone()).await;
    let rows = history.end_call(identity, ctx.snapshots());
    let mut outcome = outcome(ran, &rows);
    // What the DAW said of a step can name the file it was given: its name only, as everywhere else.
    connection::names_only(&mut outcome.steps);
    outcome.reason = outcome.reason.map(|reason| connection::names_only_text(&reason));
    // Each row keeps the files its own step placed (by the step the plan says made it); a row it doesn't place keeps
    // them all.
    let steps_of: HashMap<String, u64> = outcome.steps["done"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|done| Some((done.get("change")?.as_str()?.to_string(), done.get("step")?.as_u64()?)))
        .collect();
    for row in &rows {
        let own: Vec<Value> = match steps_of.get(&row.label) {
            Some(step) => sources.iter().filter(|source| source["step"].as_u64() == Some(*step)).cloned().collect(),
            None => sources.clone(),
        };
        history.placed(std::slice::from_ref(&row.id), &own, ctx.snapshots());
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
    history.call_came_to(&ids, outcome.state, ctx.snapshots());
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
        "summary": connection::names_only_text(&outcome.summary),
        "undo_available": rows.iter().any(|row| row.undo_available),
        "project_key": project_now.key,
        "epoch": project_now.epoch,
    });
    if let Some(reason) = outcome.reason {
        answer["reason"] = json!(reason);
    }
    Ok(answer)
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
            };
        }
    };
    let Ok(Value::Object(said)) = serde_json::from_str::<Value>(&result.text) else {
        // A plain answer: refused before it ran (its checks), an error after it changed something, or done.
        return match (result.is_error, rows.is_empty()) {
            (true, true) => Outcome { state: "refused", steps: Value::Array(vec![]), summary: String::new(), reason: Some(result.text) },
            (true, false) => Outcome { state: "unsure", steps: Value::Array(vec![]), summary: summary(rows), reason: Some(result.text) },
            (false, _) => Outcome { state: "applied", steps: Value::Array(vec![]), summary: summary(rows), reason: None },
        };
    };
    let list = |name: &str| said.get(name).and_then(Value::as_array).cloned().unwrap_or_default();
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
        };
    }
    if !changed && done.is_empty() {
        return Outcome { state: "refused", steps, summary: String::new(), reason: reason() };
    }
    if !refused.is_empty() || !held.is_empty() {
        return Outcome { state: "partial", steps, summary: summary(rows), reason: reason() };
    }
    Outcome { state: "applied", steps, summary: summary(rows), reason: None }
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
    let on = if targets.is_empty() {
        String::new()
    } else {
        format!(" — on {}", targets.iter().map(|name| quoted(name, NAME)).collect::<Vec<_>>().join(", "))
    };
    let text = one_line(&format!("{}{set}{on}", capitalized(&tool.replace('_', " "))), LINE);
    json!({"tool": tool, "family": family, "effect": "write", "text": text})
}

/// A value as the approval shows it: a string quoted (a ref by its name), a list or a group of values by their first
/// [`FIRST`], with how many more.
fn shown(names: &HashMap<String, String>, value: &Value) -> String {
    let more = |n: usize| if n > FIRST { format!(", +{} more", n - FIRST) } else { String::new() };
    match value {
        Value::String(text) => match target(names, text) {
            Some(name) => quoted(&name, NAME),
            None => quoted(text, QUOTED),
        },
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

/// What a ref is called: the name the last look gave it, or what a step before makes (`@name`).
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
                  `preview` says what it would undo without doing it.",
    schema: undo_schema,
    effect: undo_effect,
    run: run_undo,
};

fn undo_schema() -> Value {
    json!({"type": "object", "additionalProperties": false, "required": ["history_id"], "properties": {
        "history_id": {"type": "string", "minLength": 1, "maxLength": 96},
        "preview": {"type": "boolean"}
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
    // Why it can't be undone from here, before the DAW is asked: CC-3's reason, C1's state and the words.
    let why_not = if row.runtime_generation != ctx.history().generation() {
        Some(("restarted", "expired", "It was made before Kumi's runtime restarted, so it can't be undone from here."))
    } else if row.state == "expired" {
        Some(("expired", "expired", "It can't be undone any more."))
    } else if !row.undo_available {
        Some(("kept", "refused", "That change can't be undone now (see its state in HISTORY)."))
    } else {
        None
    };
    // The DAW must answer, with that change's project open and no return since the last look.
    let connection = match ctx.connection() {
        Ok(connection) => connection,
        Err(error) if previewing(&call.args) => return Err(error),
        Err(error) => return Ok(json!({"state": "refused", "code": error.code(), "reason": error.reason()})),
    };
    let here = ctx.link().expect_project(&row.project_key);
    let track = row.track.as_deref().map(|track| format!(" on {track}")).unwrap_or_default();
    if previewing(&call.args) {
        if let Err(error) = here {
            return Ok(json!({"state": "refused", "code": error.code(), "reason": error.reason()}));
        }
        let mut preview = json!({"state": "preview", "text": format!("Undo {}{track}", row.title), "undo_available": why_not.is_none()});
        if let Some((reason, _, _)) = why_not {
            preview["reason"] = json!(reason);
        }
        return Ok(preview);
    }
    if let Some((_, state, reason)) = why_not {
        return Ok(answer(state, reason));
    }
    if let Err(error) = here {
        return Ok(json!({"state": "refused", "code": error.code(), "reason": error.reason()}));
    }
    let undone = connection.integration.undo(Some(&row.label), call.signal.clone()).await;
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
    // Its own audit row for an unsure undo, in the answer's history_ids: the adapter then adds none of its own.
    let mut audited = None;
    if state == "unsure" {
        let asked = super::mcp::digest(UNDO.name, &call.args);
        let identity = CallIdentity::of(&call.host, &asked);
        let audit = ctx.history().audit(&format!("Undo {}", row.title), &row.project_key, &reason, identity, ctx.snapshots());
        ctx.history().call_came_to(std::slice::from_ref(&audit.id), "unsure", ctx.snapshots());
        ctx.link().note_unsure("undo", &reason, "unsure");
        audited = Some(audit.id);
    }
    if state != "refused" {
        moved_on(ctx);
    }
    super::core::publish_status(ctx, false).await;
    let mut answered = answer(state, &reason);
    if let Some(audit) = audited {
        answered["history_ids"] = json!([audit]);
    }
    Ok(answered)
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
        let items: Vec<Value> = ctx.history().rows(limit).iter().filter_map(|row| serde_json::to_value(row).ok()).map(shown_row).collect();
        Ok(json!({"items": items}))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use kumi_types::contracts::ToolResult;

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
            profile_id: None,
            session_id: None,
            tool_call_id: None,
            digest: None,
            call_outcome: None,
            sources: None,
        }
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
    }
}
