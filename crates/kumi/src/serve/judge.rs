//! `judge` and `tune`: Live's own judged run and tune tools (Kumi's listening), through the runtime. A judged run starts
//! with a goal over a stretch of the Arrangement, judges each change made since as a round (kept, or taken back by Kumi
//! itself), and ends with done; tune picks a device's numbers against the run's checklist and judges them as a round.
//! The tools are the integration's; this passes them through with what the runtime adds:
//! - Live only, on the project key and epoch an observe gave. A goal's stretch is from_beat and beats, never the whole
//!   song: the plugin host's listening hears at most a minute a pass, through Kumi Ears alone, with Live stopped and not
//!   recording (the integration holds those limits on every pass).
//! - A reference is a clip ref or a source the runtime resolves (a map sound, an artifact, an approved file), copied
//!   aside for the call: never a path a call gives.
//! - A call runs in `change`'s call scope: what it records (a device it adds to bring the level back, a tune's settings)
//!   is HISTORY rows of one change, and a change a round takes back is undone in HISTORY, which the answer names.

use super::connection;
use super::ctx::{epoch_of, epoch_schema, Call, Ctx, Effect, JsonObject, OpError, OpFuture, Operation};
use super::history::{CallIdentity, Row, KEPT};
use super::listen::{source_schema, Source};
use super::live_pass::{readable_clip, readable_reference, window_refusal, Capturing};
use kumi_listening::tools::{judge_tool, tune_tool};
use kumi_types::contracts::ToolResult;
use kumi_types::daw::{DawId, DawProfile};
use kumi_types::errors::RuntimeError;
use serde_json::{json, Map, Value};
use std::collections::HashMap;

/// How the integration's words map here, said with every round.
const IN_HERMES: &str = "In Kumi for Hermes: make_changes is kumi_change, judge is kumi_judge, tune is kumi_tune, \
                         discovery is kumi_observe and kumi_read. A change a round took back is undone already (taken_back): \
                         don't undo it again. Each call goes on the project_key and epoch this answer gives.";

pub static JUDGE: Operation = Operation {
    name: "judge",
    description: "Kumi's judged run in Live: start one with goal (loudness, true_peak, a reference, focus, targets) over \
                  from_beat and beats of a track or the mix (Live plays at most 60 seconds: the stretch, a bar before it and half a \
                  bar after); Kumi hears it through Kumi Ears and answers \
                  the checklist and the next target. Make ONE change toward it with kumi_change, then judge with change: Kumi \
                  hears the same bars again, keeps the change only if the evidence says so and takes it back itself \
                  otherwise. done ends the run with before → after. Made on the project key and epoch an observe gave.",
    schema: judge_schema,
    effect: judge_effect,
    run: run_judge,
};

pub static TUNE: Operation = Operation {
    name: "tune",
    description: "Kumi picks a device's numbers against the judged run's checklist and judges them as a round (kept or \
                  taken back): fit (an EQ Eight's bands, no trial listens), home (one knob toward its target), search (2–5 \
                  knobs that interact, on scratch copies of the run's track) or probe (one knob across its range on scratch \
                  copies, then set where it meets the target). Start a judged run first (kumi_judge goal). Made on the \
                  project key and epoch an observe gave.",
    schema: tune_schema,
    effect: |_| Effect::Write,
    run: run_tune,
};

/// The runtime's own fields beside the tool's.
fn bound(schema: &mut Map<String, Value>) {
    let properties = schema["properties"].as_object_mut().expect("a tool schema has properties");
    properties.insert("expected_project_key".into(), json!({"type": "string", "minLength": 1, "maxLength": 64}));
    properties.insert("expected_epoch".into(), epoch_schema());
    let required = schema.entry("required").or_insert_with(|| json!([])).as_array_mut().expect("required is a list");
    required.extend([json!("expected_project_key"), json!("expected_epoch")]);
}

fn judge_schema() -> Value {
    let mut schema = judge_tool(&DawProfile::ABLETON).schema;
    let goal = schema["properties"]["goal"]["properties"].as_object_mut().expect("judge's goal has properties");
    goal.insert(
        "reference".into(),
        json!({"description": "A reference whose balance, dynamics and low end become targets: an audio clip in the Set by its clip ref, or a source (a map sound, a file Kumi made, an approved file)",
            "anyOf": [
                {"type": "object", "additionalProperties": false, "required": ["clip_ref"],
                    "properties": {"clip_ref": {"type": "string", "minLength": 1, "maxLength": 256}}},
                source_schema()
            ]}),
    );
    bound(&mut schema);
    let mut schema = Value::Object(schema);
    schema["oneOf"] = json!([
        {"required": ["goal", "from_beat", "beats"], "not": {"anyOf": [{"required": ["change"]}, {"required": ["done"]}]}},
        {"required": ["change"], "not": {"anyOf": [{"required": ["goal"]}, {"required": ["done"]}, {"required": ["track"]}, {"required": ["from_beat"]}, {"required": ["beats"]}]}},
        {"required": ["done"], "properties": {"done": {"const": true}}, "not": {"anyOf": [{"required": ["goal"]}, {"required": ["change"]}, {"required": ["track"]}, {"required": ["from_beat"]}, {"required": ["beats"]}]}}
    ]);
    schema
}

fn tune_schema() -> Value {
    let mut schema = tune_tool(&DawProfile::ABLETON).schema;
    bound(&mut schema);
    Value::Object(schema)
}

/// A goal or done listens (a capture, with Live's state put back); a change's round may take a change back.
fn judge_effect(args: &JsonObject) -> Effect {
    if args.contains_key("change") {
        Effect::Write
    } else {
        Effect::Playback
    }
}

fn run_judge(ctx: &Ctx, call: Call) -> OpFuture<'_> {
    Box::pin(listened(ctx, call, "judge"))
}

fn run_tune(ctx: &Ctx, call: Call) -> OpFuture<'_> {
    Box::pin(listened(ctx, call, "tune"))
}

fn refused(code: &str, reason: impl Into<String>) -> Value {
    json!({"state": "refused", "code": code, "reason": reason.into(), "history_ids": [], "taken_back": []})
}

fn refusal(error: OpError) -> Value {
    refused(error.code(), error.reason())
}

/// What a judge call asks the integration's tool, its reference resolved: refused (with its words) when it can't be.
/// The staged copy a source was resolved to lives as long as the call.
async fn judge_input(
    ctx: &Ctx,
    args: &JsonObject,
    call: &Call,
    capturing: &Capturing,
) -> Result<(Map<String, Value>, Option<super::artifacts::StagedArtifact>), Value> {
    let given = ["goal", "change", "done"].iter().filter(|key| args.contains_key(**key)).count();
    if given != 1 || args.get("done").is_some_and(|done| done != &json!(true)) {
        return Err(refused("invalid", "Give one of goal (with from_beat and beats), change, or done: true."));
    }
    let mut input: Map<String, Value> =
        args.iter().filter(|(key, _)| !key.starts_with("expected_")).map(|(key, value)| (key.clone(), value.clone())).collect();
    let Some(goal) = input.get_mut("goal") else {
        if ["track", "from_beat", "beats"].iter().any(|key| args.contains_key(*key)) {
            return Err(refused("invalid", "track, from_beat and beats go with a goal."));
        }
        return Ok((input, None));
    };
    let (Some(from), Some(beats)) = (args.get("from_beat").and_then(Value::as_f64), args.get("beats").and_then(Value::as_f64)) else {
        return Err(refused("invalid", "A goal hears from_beat and beats of the Arrangement (at most 60 seconds with a bar before and half a bar after), never the whole song."));
    };
    if !(from.is_finite() && from >= 0. && beats.is_finite() && beats > 0.) {
        return Err(refused("invalid", "from_beat is 0 or more and beats more than 0."));
    }
    // The integration refuses a longer pass; at the tempo the DAW last said, it's said before Kumi hears anything.
    if let Some(error) = window_refusal(ctx, from, beats) {
        return Err(refusal(error));
    }
    let mut staged = None;
    if let Some(reference) = goal.get_mut("reference") {
        let resolved = if let Some(clip) = reference.get("clip_ref").and_then(Value::as_str) {
            readable_clip(ctx, clip, capturing.folder(), &call.signal).await.map_err(refusal)?
        } else {
            let source = Source::parse(reference).map_err(refusal)?;
            let (path, copy) = readable_reference(ctx, &source, capturing.folder(), &call.signal).await.map_err(refusal)?;
            staged = Some(copy);
            path.to_string_lossy().into_owned()
        };
        *reference = json!(resolved);
    }
    Ok((input, staged))
}

/// One judge or tune call, through the integration's tool of that name.
async fn listened(ctx: &Ctx, call: Call, name: &'static str) -> Result<Value, OpError> {
    let args = &call.args;
    if ctx.daw() != DawId::Ableton {
        let app = ctx.daw().profile().words.app;
        return Ok(refused("unsupported", format!("Kumi's judged runs and tune work in Live here, not {app}.")));
    }
    if let Err(error) = ctx.connection() {
        return Ok(refusal(error));
    }
    let Some(key) = args.get("expected_project_key").and_then(Value::as_str) else {
        return Ok(refused("invalid", "expected_project_key is the project key an observe gave."));
    };
    let Some(epoch) = epoch_of(args, "expected_epoch") else {
        return Ok(refused("invalid", "expected_epoch is the epoch an observe gave: a whole number, or its digits."));
    };
    let project = match ctx.link().expect(key, epoch) {
        Ok(project) => project,
        Err(error) => return Ok(refusal(error)),
    };
    let Some(tool) = ctx.link().tools().into_iter().find(|tool| tool.name() == name) else {
        return Ok(refused(
            "unsupported",
            "This Live offers Kumi no judged listening: update Kumi's part in Live with setup, then restart Live.",
        ));
    };
    // What the call's passes capture, and a reference decoded for them, go in its own folder, removed with it.
    let capturing = match Capturing::new(ctx) {
        Ok(capturing) => capturing,
        Err(error) => return Ok(refusal(error)),
    };
    let (input, _staged) = if name == "judge" {
        match judge_input(ctx, args, &call, &capturing).await {
            Ok(input) => input,
            Err(refusal) => return Ok(refusal),
        }
    } else {
        (args.iter().filter(|(key, _)| !key.starts_with("expected_")).map(|(key, value)| (key.clone(), value.clone())).collect(), None)
    };
    if let Some(problem) = kumi_agent::kernel::schema::problem(&Value::Object(tool.input_schema()), &Value::Object(input.clone())) {
        return Ok(refused("invalid", problem));
    }
    let history = ctx.history();
    let before: HashMap<String, String> = history.rows(KEPT).into_iter().map(|row| (row.id, row.state)).collect();
    let identity_digest = super::mcp::digest(name, args);
    let identity = CallIdentity::of(&call.host, &identity_digest);
    history.begin_call();
    let ran = tool.execute(input, call.signal.clone()).await;
    let rows = history.end_call(identity, ctx.snapshots());
    // A change of an earlier call that this round took back: its row is undone now.
    let taken_back: Vec<String> = history
        .rows(KEPT)
        .into_iter()
        .filter(|row| row.state == "undone" && before.get(&row.id).is_some_and(|was| was != "undone"))
        .map(|row| row.id)
        .collect();
    let mut answer = outcome(ran, &rows, !taken_back.is_empty());
    let state = answer["state"].as_str().unwrap_or("unsure").to_string();
    let mut ids: Vec<String> = rows.iter().map(|row| row.id.clone()).collect();
    if state == "unsure" {
        let reason = answer["reason"]
            .as_str()
            .map(str::to_string)
            .unwrap_or_else(|| "Kumi didn't confirm how the round ended: look at the project before another change.".into());
        answer["reason"] = json!(reason);
        let title = if name == "judge" { "Judged round" } else { "Tune" };
        ids.push(history.audit(title, &project.key, &reason, identity, ctx.snapshots()).id);
        ctx.link().note_unsure(name, &reason, "unsure");
    }
    history.one_change(&ids, ctx.snapshots());
    history.call_came_to(&ids, &state, ctx.snapshots());
    if state != "refused" {
        ctx.link().advance();
        connection::publish_focus(&ctx.handle());
    }
    let now = ctx.link().project().unwrap_or(project);
    super::core::publish_status(ctx, false).await;
    answer["history_ids"] = json!(ids);
    answer["taken_back"] = json!(taken_back);
    answer["undo_available"] = json!(rows.iter().any(|row| row.undo_available));
    answer["project_key"] = json!(now.key);
    answer["epoch"] = json!(now.epoch);
    if state != "refused" {
        answer["in_hermes"] = json!(IN_HERMES);
    }
    Ok(answer)
}

/// What a call came to: `done` with the round (the tool's own answer, names only), `refused` when the tool said why
/// and nothing changed, `unsure` when it stopped after something changed or a row is unsure.
fn outcome(ran: Result<ToolResult, RuntimeError>, rows: &[Row], took_back: bool) -> Value {
    let changed = !rows.is_empty() || took_back;
    let unsure = |reason: String| json!({"state": "unsure", "reason": connection::names_only_text(&reason)});
    let round = match ran {
        Ok(result) if !result.is_error => serde_json::from_str::<Value>(&result.text).ok().filter(Value::is_object),
        Ok(result) if !changed => return refused("refused", connection::names_only_text(&result.text)),
        Ok(result) => return unsure(result.text),
        Err(error) if !changed => return refused("failed", connection::names_only_text(&error.to_string())),
        Err(error) => return unsure(format!("The listen stopped: {error}")),
    };
    let Some(mut round) = round else {
        return if changed {
            unsure("Kumi's listening answered something the runtime can't read: look at the project before another change.".into())
        } else {
            refused("failed", "Kumi's listening answered something the runtime can't read.")
        };
    };
    connection::names_only_said(&mut round);
    if rows.iter().any(|row| row.state == "unsure") {
        return json!({"state": "unsure", "round": round});
    }
    json!({"state": "done", "round": round})
}

#[cfg(test)]
#[path = "judge_tests.rs"]
mod tests;
