//! `transport` (CC-17): starts or stops the DAW's playback with the DAW's own transport, through its integration. A play
//! is an action, not a change: it changes nothing in the project but where play starts, and keeps no HISTORY row.
//!
//! A play is refused while a listen or capture job runs or Kumi holds the DAW for another call (`busy`), while the DAW
//! records or would record (`recording`: read fresh, and a DAW that can't say is taken as recording), when the project
//! isn't the one named (`stale`), and from a place this DAW doesn't play from (`invalid`, in its words). A stop is always
//! allowed, takes no lease and waits on none.

use super::ctx::{Call, Ctx, Effect, OpError, OpFuture, Operation};
use kumi_agent::core::contracts::{PlayFrom, Played};
use kumi_types::contracts::JsonObject;
use serde_json::{json, Value};

pub static TRANSPORT: Operation = Operation {
    name: "transport",
    description: "Starts or stops the DAW's playback with its own transport. `play` starts from where it is, from a bar \
                  or beat (REAPER), a sequence slot and line (Renoise), or launches a Session clip or fires a scene \
                  (Live); `stop` stops it. It changes nothing in the project but where play starts, and keeps no \
                  HISTORY row. `play` is refused while a listen runs, while the DAW records or would record, and when \
                  the project isn't the one named; `stop` always runs. `preview` says what it would do.",
    schema: transport_schema,
    effect: transport_effect,
    run: run_transport,
};

/// The jobs whose playback is their own: a play waits for them.
const PLAYING_JOBS: [&str; 2] = ["listen", "capture"];

fn transport_schema() -> Value {
    json!({"type": "object", "additionalProperties": false, "required": ["action"], "properties": {
        "action": {"type": "string", "enum": ["play", "stop"]},
        "from_bar": {"type": "integer", "minimum": 1, "maximum": 100000, "description": "play: the bar as the DAW shows it, from 1"},
        "from_beat": {"type": "number", "minimum": 0, "maximum": 1000000, "description": "play: quarter notes from the song's start"},
        "from_slot": {"type": "integer", "minimum": 1, "maximum": 1000, "description": "play: a pattern sequence slot, from 1"},
        "from_line": {"type": "integer", "minimum": 0, "maximum": 512, "description": "play: a line, from 0"},
        "clip_ref": {"type": "string", "minLength": 1, "maxLength": 256, "description": "play: a Session clip to launch"},
        "scene_index": {"type": "integer", "minimum": 0, "maximum": 1000, "description": "play: a scene to fire, 0 the first"},
        "expected_project_key": {"type": "string", "minLength": 1, "maxLength": 64},
        "preview": {"type": "boolean"}
    }})
}

fn previewing(args: &JsonObject) -> bool {
    args.get("preview").and_then(Value::as_bool) == Some(true)
}

/// A preview reads; a play or a stop moves the transport without the DAW's write lease.
fn transport_effect(args: &JsonObject) -> Effect {
    if previewing(args) {
        Effect::Read
    } else {
        Effect::Transport
    }
}

fn run_transport(ctx: &Ctx, call: Call) -> OpFuture<'_> {
    Box::pin(transport(ctx, call))
}

/// Where the play starts, from the call's fields: one place at most.
fn play_from(args: &JsonObject) -> Result<PlayFrom, OpError> {
    let whole = |field: &str| args.get(field).and_then(Value::as_u64).map(|n| n as u32);
    let given = ["from_bar", "from_beat", "clip_ref", "scene_index"].iter().filter(|field| args.contains_key(**field)).count()
        + usize::from(args.contains_key("from_slot") || args.contains_key("from_line"));
    if given > 1 {
        return Err(OpError::Invalid {
            reason: "Give one place to play from: from_bar, from_beat, from_slot with from_line, clip_ref or scene_index.".into(),
        });
    }
    Ok(if let Some(bar) = whole("from_bar") {
        PlayFrom::Bar(bar)
    } else if let Some(beat) = args.get("from_beat").and_then(Value::as_f64) {
        PlayFrom::Beat(beat)
    } else if args.contains_key("from_slot") || args.contains_key("from_line") {
        PlayFrom::Slot { slot: whole("from_slot"), line: whole("from_line") }
    } else if let Some(clip) = args.get("clip_ref").and_then(Value::as_str) {
        PlayFrom::Clip(clip.to_string())
    } else if let Some(scene) = whole("scene_index") {
        PlayFrom::Scene(scene)
    } else {
        PlayFrom::Here
    })
}

/// What a play would do, in words, naming the project and what it launches.
fn play_text(from: &PlayFrom, project: &str, names: &std::collections::HashMap<String, String>) -> String {
    match from {
        PlayFrom::Here => format!("Play {project} from where it is"),
        PlayFrom::Bar(bar) => format!("Play {project} from bar {bar}"),
        PlayFrom::Beat(beat) => {
            let beat = kumi_common::js::number::round(*beat * 1000.0) / 1000.0;
            format!("Play {project} from beat {}", kumi_common::js::number::to_string(beat))
        }
        PlayFrom::Slot { slot: Some(slot), line: Some(line) } => format!("Play {project} from sequence slot {slot}, line {line}"),
        PlayFrom::Slot { slot: Some(slot), line: None } => format!("Play {project} from sequence slot {slot}"),
        PlayFrom::Slot { slot: None, line } => format!("Play {project} from line {} of the pattern being edited", line.unwrap_or(0)),
        PlayFrom::Clip(clip) => match names.get(clip) {
            Some(name) => format!("Launch “{name}” in {project}"),
            None => format!("Launch a Session clip in {project}"),
        },
        PlayFrom::Scene(index) => format!("Fire scene {} in {project}", index + 1),
    }
}

fn refused(code: &'static str, reason: String) -> OpError {
    OpError::Refused { code, reason, owner: None }
}

fn answer(played: Played) -> Value {
    let mut answer = json!({"state": if played.playing { "playing" } else { "stopped" }});
    if let Some(position) = played.position_beats {
        answer["position_beats"] = json!(position);
    }
    answer
}

async fn transport(ctx: &Ctx, call: Call) -> Result<Value, OpError> {
    let args = &call.args;
    let connection = ctx.connection()?;
    let project = ctx.link().project().map(|project| project.name).unwrap_or_else(|| "the project".into());
    let app = ctx.daw().profile().words.app;
    if args.get("action").and_then(Value::as_str) == Some("stop") {
        if previewing(args) {
            return Ok(json!({"state": "preview", "text": format!("Stop {project}")}));
        }
        let stopped = connection.integration.stop_playback(call.signal.clone()).await;
        return stopped.map(answer).map_err(|error| OpError::Failed { reason: format!("{app} didn't stop as asked: {error}") });
    }
    let from = play_from(args)?;
    connection.integration.plays_from(&from).map_err(|reason| OpError::Invalid { reason })?;
    let Some(key) = args.get("expected_project_key").and_then(Value::as_str) else {
        return Err(OpError::Invalid { reason: "A play names the project it plays (expected_project_key, from the last look).".into() });
    };
    ctx.link().expect_project(key)?;
    if ctx.jobs().running_of(&PLAYING_JOBS) || ctx.lease().held() {
        return Err(refused("busy", format!("Kumi is listening to {project}, or changing it, right now: play it once that's done.")));
    }
    match connection.integration.record_armed(call.signal.clone()).await {
        Ok(Some(false)) => {}
        Ok(Some(true)) => {
            return Err(refused(
                "recording",
                format!(
                "{app} is recording, or its record is on: Kumi never starts a play that would record. Turn recording off in {app} first."
            ),
            ))
        }
        Ok(None) | Err(_) => {
            return Err(refused("recording", format!("Kumi can't tell whether {app} would record, so it doesn't start it.")))
        }
    }
    let text = play_text(&from, &project, &ctx.link().ref_names());
    if previewing(args) {
        return Ok(json!({"state": "preview", "text": text}));
    }
    let played = connection.integration.play(&from, call.signal.clone()).await;
    played.map(answer).map_err(|error| OpError::Failed { reason: format!("{app} didn't start as asked: {error}") })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(value: Value) -> JsonObject {
        value.as_object().unwrap().clone()
    }

    #[test]
    fn one_place_to_play_from_at_most() {
        assert_eq!(play_from(&args(json!({"action": "play"}))).unwrap(), PlayFrom::Here);
        assert_eq!(play_from(&args(json!({"action": "play", "from_line": 16}))).unwrap(), PlayFrom::Slot { slot: None, line: Some(16) });
        assert_eq!(
            play_from(&args(json!({"action": "play", "from_slot": 3, "from_line": 16}))).unwrap(),
            PlayFrom::Slot { slot: Some(3), line: Some(16) }
        );
        assert!(play_from(&args(json!({"action": "play", "from_bar": 9, "clip_ref": "7:clip:0:0"}))).is_err());
        assert!(play_from(&args(json!({"action": "play", "from_beat": 4, "from_line": 0}))).is_err());
    }
}
