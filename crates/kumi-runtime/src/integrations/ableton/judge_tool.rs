//! The judge tool: the model's side of a judged run (a goal in, a round's log out).
use super::rendering::{GoalRequest, JudgeRequest, TuneHow, TuneRequest};
use crate::{
    core::contracts::JsonObject,
    listening::{
        checklist::{Explicit, Quantity, Target},
        round::{Round, RoundKind},
    },
};
use kumi_common::js::string::trim;
use serde_json::{json, Value};
use std::sync::LazyLock;

pub const JUDGE_TOOL: &str = "judge";
static DATA: LazyLock<Value> = LazyLock::new(|| serde_json::from_str(include_str!("assets/judge.json")).unwrap());
pub static JUDGE_DESCRIPTION: LazyLock<String> = LazyLock::new(|| DATA["description"].as_str().unwrap().into());
pub static JUDGE_SCHEMA: LazyLock<JsonObject> = LazyLock::new(|| DATA["schema"].as_object().unwrap().clone());

pub const TUNE_TOOL: &str = "tune";
static TUNE: LazyLock<Value> = LazyLock::new(|| serde_json::from_str(include_str!("assets/tune.json")).unwrap());
pub static TUNE_DESCRIPTION: LazyLock<String> = LazyLock::new(|| TUNE["description"].as_str().unwrap().into());
pub static TUNE_SCHEMA: LazyLock<JsonObject> = LazyLock::new(|| TUNE["schema"].as_object().unwrap().clone());

/// The tune tool's input as a request, or what's wrong with it.
pub fn tune_request(input: &JsonObject) -> Result<TuneRequest, String> {
    let text = |key: &str| input.get(key).and_then(Value::as_str).map(trim).filter(|s| !s.is_empty()).map(str::to_owned);
    let how =
        match input.get("how").and_then(Value::as_str) {
            Some("fit") => TuneHow::Fit,
            Some("home") => TuneHow::Home,
            Some("search") => TuneHow::Search,
            _ => return Err(
                "how is fit (an EQ calculated from what was measured), home (one knob homed in on) or search (2 to 5 knobs that interact)."
                    .into(),
            ),
        };
    let device = text("device").ok_or("Name the device to tune (its ref).")?;
    let knobs: Vec<String> = input
        .get("knobs")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .collect();
    if how == TuneHow::Home && knobs.len() != 1 {
        return Err("home moves one knob: give its name in knobs.".into());
    }
    if how == TuneHow::Search && !(2..=5).contains(&knobs.len()) {
        return Err("search tunes 2 to 5 knobs that interact: give their names in knobs.".into());
    }
    Ok(TuneRequest { device, how, target: text("target"), knobs, change: text("change") })
}

/// The tool's input as a request, or what's wrong with it.
pub fn judge_request(input: &JsonObject) -> Result<JudgeRequest, String> {
    let number = |value: &Value, key: &str| value.get(key).and_then(Value::as_f64).filter(|v| v.is_finite());
    let text = |value: &Value, key: &str| value.get(key).and_then(Value::as_str).map(trim).filter(|s| !s.is_empty()).map(str::to_owned);
    let input = Value::Object(input.clone());
    let goal = match input.get("goal") {
        Some(goal) if goal.is_object() => {
            let mut targets = vec![];
            for wanted in goal.get("targets").and_then(Value::as_array).into_iter().flatten() {
                let measure = match wanted.get("measure").and_then(Value::as_str).unwrap_or("") {
                    "dynamics" => Quantity::Plr,
                    "density" => Quantity::Psr,
                    "range" => Quantity::Range,
                    "punch" => Quantity::Crest,
                    "pumping" => Quantity::Pumping,
                    "distortion" => Quantity::Distortion,
                    "low_width" => Quantity::LowWidth,
                    "rumble" => Quantity::Rumble,
                    "brightness" => Quantity::Tilt,
                    other => return Err(format!("{other} isn't a measure Kumi can target.")),
                };
                let target = match (number(wanted, "value"), number(wanted, "at_least"), number(wanted, "at_most")) {
                    (Some(value), _, _) => Target::Exactly { value, within: number(wanted, "within").unwrap_or(0.5) },
                    (None, Some(low), Some(high)) if low <= high => Target::Between { low, high },
                    (None, Some(value), None) => Target::AtLeast { value },
                    (None, None, Some(value)) => Target::AtMost { value },
                    _ => return Err("Give each target a value (and within), at_least or at_most.".into()),
                };
                targets.push(Explicit { measure, target });
            }
            Some(GoalRequest {
                loudness: number(goal, "loudness"),
                true_peak: number(goal, "true_peak"),
                reference: text(goal, "reference"),
                problems: goal.get("problems").and_then(Value::as_bool).unwrap_or(true),
                focus: text(goal, "focus"),
                targets,
            })
        }
        Some(_) => return Err("goal is an object: what to reach.".into()),
        None => None,
    };
    let request = JudgeRequest {
        goal,
        track: text(&input, "track"),
        from_beat: number(&input, "from_beat"),
        beats: number(&input, "beats").filter(|beats| *beats > 0.),
        change: text(&input, "change"),
        done: input.get("done") == Some(&Value::Bool(true)),
    };
    if request.goal.is_some() && (request.change.is_some() || request.done) {
        return Err("A goal starts a run; judge a change (or end the run) in a later call.".into());
    }
    if request.goal.is_none() && request.change.is_none() && !request.done {
        return Err("Give a goal to start, the change you made, or done: true to end.".into());
    }
    Ok(request)
}

/// A round as the model reads it: the log's lines, and the checklist itself when the run starts or ends.
pub fn judge_reply(round: &Round) -> Value {
    let mut reply = json!({"round":round.round,"log":round.lines(),"met":round.met,"listens":round.listens});
    if let Some(next) = &round.next {
        reply["next"] = json!(next);
    }
    if round.kind != RoundKind::Judged {
        reply["checklist"] = json!(round.rows);
    }
    if !round.problems.is_empty() {
        reply["problems"] = json!(round.problems);
    }
    if let Some(kept) = round.kept {
        reply["kept"] = json!(kept);
    }
    reply["note"] = json!(match (round.kind, round.met) {
        (_, true) => "Every item is within tolerance: say so and stop, or end the run with done: true for a last whole listen.",
        (RoundKind::Done, false) => "The run is over. Say what changed, before → after, and what's still off.",
        (RoundKind::Start, false) => "Make one change toward next, then call judge with change.",
        (RoundKind::Judged, false) if round.kept == Some(false) =>
            "Kumi took that change back. Try another way to close next (another device, a different setting), then judge it.",
        (RoundKind::Judged, false) => "Kept. Make one change toward next, then judge it.",
    });
    reply
}
