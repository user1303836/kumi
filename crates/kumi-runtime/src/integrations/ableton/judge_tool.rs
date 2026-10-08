//! The judge tool: the model's side of a judged run (a goal in, a round's log out).
use super::rendering::{FormRequest, GoalRequest, GrooveRequest, JudgeRequest, SoundRequest, TuneHow, TuneRequest};
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

pub const SOUND_TOOL: &str = "sound";
static SOUND: LazyLock<Value> = LazyLock::new(|| serde_json::from_str(include_str!("assets/sound.json")).unwrap());
pub static SOUND_DESCRIPTION: LazyLock<String> = LazyLock::new(|| SOUND["description"].as_str().unwrap().into());
pub static SOUND_SCHEMA: LazyLock<JsonObject> = LazyLock::new(|| SOUND["schema"].as_object().unwrap().clone());

/// The sound tool's input as a request, or what's wrong with it.
pub fn sound_request(input: &JsonObject) -> Result<SoundRequest, String> {
    let text = |key: &str| input.get(key).and_then(Value::as_str).map(trim).filter(|s| !s.is_empty()).map(str::to_owned);
    let tracks: Vec<String> = input
        .get("tracks")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .collect();
    if tracks.is_empty() {
        return Err("Name the tracks to measure (tracks).".into());
    }
    Ok(SoundRequest {
        tracks,
        against: text("against"),
        key: text("key"),
        from_beat: input.get("from_beat").and_then(Value::as_f64).filter(|v| v.is_finite() && *v >= 0.),
        beats: input.get("beats").and_then(Value::as_f64).filter(|v| v.is_finite() && *v > 0.),
    })
}

pub const FORM_TOOL: &str = "form";
static FORM: LazyLock<Value> = LazyLock::new(|| serde_json::from_str(include_str!("assets/form.json")).unwrap());
pub static FORM_DESCRIPTION: LazyLock<String> = LazyLock::new(|| FORM["description"].as_str().unwrap().into());
pub static FORM_SCHEMA: LazyLock<JsonObject> = LazyLock::new(|| FORM["schema"].as_object().unwrap().clone());

/// The form tool's input as a request.
pub fn form_request(input: &JsonObject) -> FormRequest {
    FormRequest {
        from_beat: input.get("from_beat").and_then(Value::as_f64).filter(|v| v.is_finite() && *v >= 0.),
        beats: input.get("beats").and_then(Value::as_f64).filter(|v| v.is_finite() && *v > 0.),
        reference: input.get("reference").and_then(Value::as_str).map(trim).filter(|s| !s.is_empty()).map(str::to_owned),
    }
}

pub const GROOVE_TOOL: &str = "groove";
static GROOVE: LazyLock<Value> = LazyLock::new(|| serde_json::from_str(include_str!("assets/groove.json")).unwrap());
pub static GROOVE_DESCRIPTION: LazyLock<String> = LazyLock::new(|| GROOVE["description"].as_str().unwrap().into());
pub static GROOVE_SCHEMA: LazyLock<JsonObject> = LazyLock::new(|| GROOVE["schema"].as_object().unwrap().clone());

/// The groove tool's input as a request, or what's wrong with it.
pub fn groove_request(input: &JsonObject) -> Result<GrooveRequest, String> {
    let text = |key: &str| input.get(key).and_then(Value::as_str).map(trim).filter(|s| !s.is_empty()).map(str::to_owned);
    let request = GrooveRequest {
        clip: text("clip"),
        reference: text("reference"),
        reference_tempo: input
            .get("reference_tempo")
            .and_then(Value::as_f64)
            .filter(|tempo| tempo.is_finite() && (20. ..=400.).contains(tempo)),
        audio: text("audio"),
        change: text("change"),
        apply: input.get("apply") == Some(&Value::Bool(true)),
        amount: input.get("amount").and_then(Value::as_f64).filter(|v| v.is_finite()),
        done: input.get("done") == Some(&Value::Bool(true)),
    };
    let starting = request.clip.is_some() || request.reference.is_some();
    if starting && (request.clip.is_none() || request.reference.is_none()) {
        return Err(
            "A run starts with both clip (the part) and reference (the reference's MIDI clip, or an audio file of a pitched part).".into(),
        );
    }
    if starting && (request.change.is_some() || request.apply || request.done) {
        return Err("Start a run first; judge a change (or apply, or end it) in a later call.".into());
    }
    if !starting && request.change.is_none() && !request.apply && !request.done {
        return Err("Give clip and reference to start, the change you made, apply: true, or done: true.".into());
    }
    Ok(request)
}

/// The tune tool's input as a request, or what's wrong with it.
pub fn tune_request(input: &JsonObject) -> Result<TuneRequest, String> {
    let text = |key: &str| input.get(key).and_then(Value::as_str).map(trim).filter(|s| !s.is_empty()).map(str::to_owned);
    let how =
        match input.get("how").and_then(Value::as_str) {
            Some("fit") => TuneHow::Fit,
            Some("home") => TuneHow::Home,
            Some("search") => TuneHow::Search,
            Some("probe") => TuneHow::Probe,
            _ => return Err(
                "how is fit (an EQ calculated from what was measured), home (one knob homed in on), search (2 to 5 knobs that interact) or probe (one knob heard across its range)."
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
    if matches!(how, TuneHow::Home | TuneHow::Probe) && knobs.len() != 1 {
        return Err("home and probe move one knob: give its name in knobs.".into());
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
                    "decay_time" => Quantity::DecayTime,
                    "tail_darkening" => Quantity::Darkening,
                    "echo_time" => Quantity::EchoTime,
                    "echo_fall" => Quantity::EchoFalls,
                    "swing" => Quantity::Swing,
                    "sweep" => Quantity::Sweep,
                    "tail_share" => Quantity::TailShare,
                    other => return Err(format!("{other} isn't a measure Kumi can target.")),
                };
                // Within a noticeable step unless asked.
                let step = match measure {
                    Quantity::DecayTime => 0.1,
                    Quantity::EchoTime => 10.,
                    Quantity::EchoFalls => 1.5,
                    Quantity::Swing => 1.,
                    Quantity::Darkening | Quantity::Sweep => 0.25,
                    Quantity::TailShare => 5.,
                    _ => 0.5,
                };
                // A tolerance of nothing, or a range of one point, can't be met: a point is that value within a step.
                let target = match (number(wanted, "value"), number(wanted, "at_least"), number(wanted, "at_most")) {
                    (Some(_), _, _) if number(wanted, "within").is_some_and(|within| within <= 0.) => {
                        return Err("within is how far from value still counts: give it above 0 (a noticeable step, say).".into())
                    }
                    (Some(value), _, _) => Target::Exactly { value, within: number(wanted, "within").unwrap_or(step) },
                    (None, Some(low), Some(high)) if low == high => Target::Exactly { value: low, within: step },
                    (None, Some(low), Some(high)) if low < high => Target::Between { low, high },
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
                sound: goal.get("sound").and_then(Value::as_bool).unwrap_or(false),
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
    let unread = round.unread();
    let note: String = match (round.kind, round.met) {
        (_, true) if !unread.is_empty() => format!(
            "Every item Kumi could read is within tolerance, but it couldn't read {}: say so, then stop, or end the run with done: true for a last whole listen.",
            unread.join(", ")
        ),
        (_, true) => "Every item is within tolerance: say so and stop, or end the run with done: true for a last whole listen.".into(),
        (RoundKind::Done, false) => "The run is over. Say what changed, before → after, and what's still off.".into(),
        (RoundKind::Start, false) => "Make one change toward next, then call judge with change.".into(),
        (RoundKind::Judged, false) if round.kept == Some(false) => {
            "Kumi took that change back. Try another way to close next (another device, a different setting), then judge it.".into()
        }
        (RoundKind::Judged, false) => "Kept. Make one change toward next, then judge it.".into(),
    };
    reply["note"] = json!(note);
    reply
}
