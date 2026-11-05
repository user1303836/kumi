//! Normalize the requested window before estimating or uploading; never infer it from a later call.

use super::{generate_source, refused, Call, Ctx, OpError, Pin, StagedArtifact, Task};
use serde_json::{json, Value};
use std::rc::Rc;

pub(super) struct Input {
    pub request: Value,
    pub details: Value,
}

pub(super) async fn prepare(ctx: &Ctx, call: &Call, task: Task, source: Option<&(Rc<Pin>, StagedArtifact)>) -> Result<Input, OpError> {
    let kind = call.args.get("kind").and_then(Value::as_str).unwrap_or("");
    let prompt = call
        .args
        .get("prompt")
        .map(|value| {
            let text = value.as_str().ok_or_else(|| refused("invalid_request", "Use a nonempty sound description."))?;
            let text = super::super::change::one_line(text, 500);
            if text.trim().is_empty() {
                return Err(refused("invalid_request", "Use a nonempty sound description."));
            }
            Ok(text)
        })
        .transpose()?;
    if task == Task::TextToSfx {
        let prompt = prompt.ok_or_else(|| refused("invalid_request", "Describe the sound effect to generate."))?;
        let millis = match call.args.get("seconds") {
            Some(value) => argument(value, "seconds", false)?,
            None => 5000,
        };
        let seconds = seconds(millis);
        return Ok(Input {
            request: json!({"model":"sfx-2.0", "prompt":prompt, "duration_ms":millis}),
            details: json!({"kind":kind, "prompt":prompt, "file":null, "seconds":seconds,
                "model":"sfx-2.0", "summary":format!("One {seconds}-second sound effect")}),
        });
    }
    if task == Task::AudioToMidi && prompt.is_some() {
        return Err(refused("invalid_request", "Audio-to-MIDI does not take a sound description."));
    }
    let (pin, stage) = source.ok_or_else(|| refused("invalid_request", "This generation requires one approved source."))?;
    let measured = if task == Task::VideoToSfx {
        generate_source::video_seconds(ctx, stage, &call.signal).await?
    } else {
        generate_source::seconds(ctx, stage, &call.signal).await?
    };
    let source_ms = measured_millis(measured)?;
    let mut request = json!({});
    let mut details = json!({"kind":kind, "prompt":prompt, "file":pin.name});
    if let Some(prompt) = prompt {
        request["prompt"] = json!(prompt);
    }
    if task == Task::VideoToSfx {
        request["video"] = json!(stage.path());
    } else {
        request["audio"] = json!(stage.path());
    }
    if task == Task::AudioToMidi {
        request["duration_ms"] = json!(source_ms);
        // Preserve the original transcription DTO; only the three new tasks add normalized windows.
        details["seconds"] = json!(measured);
        details["summary"] = json!(format!("One audio-to-MIDI transcription ({measured:.3} seconds)"));
        return Ok(Input { request, details });
    }
    let model = if task == Task::VideoToSfx { "sfx-2.0" } else { "sfx-1.6" };
    request["model"] = json!(model);
    details["model"] = json!(model);
    details["source_seconds"] = json!(seconds(source_ms));
    match task {
        Task::VideoToSfx => {
            let span = match call.args.get("seconds") {
                Some(value) => argument(value, "seconds", false)?,
                None => source_ms,
            };
            if span > source_ms {
                return Err(refused("invalid_request", "The requested video window is longer than the measured source."));
            }
            request["duration_ms"] = json!(span);
            details["seconds"] = json!(seconds(span));
            details["summary"] = json!(format!("Sound for the first {} seconds of {}", seconds(span), pin.name));
        }
        Task::Extend => {
            if measured < 3.0 {
                return Err(refused("invalid_source", "Mirelo extension needs at least three seconds of source audio."));
            }
            let span = required(call, "append_seconds", false)?;
            request["append_duration_ms"] = json!(span);
            details["seconds"] = json!(seconds(span));
            details["append_seconds"] = json!(seconds(span));
            details["summary"] = json!(format!("Append {} seconds to {}", seconds(span), pin.name));
        }
        Task::Inpaint => {
            let start = required(call, "region_start_seconds", true)?;
            let end = required(call, "region_end_seconds", false)?;
            if start >= end || end > source_ms {
                return Err(refused("invalid_request", "The inpaint region must end after its start and stay within the measured source."));
            }
            request["region_start_ms"] = json!(start);
            request["region_end_ms"] = json!(end);
            details["seconds"] = json!(seconds(end - start));
            details["region_start_seconds"] = json!(seconds(start));
            details["region_end_seconds"] = json!(seconds(end));
            details["summary"] = json!(format!("Inpaint {} from {} to {} seconds", pin.name, seconds(start), seconds(end)));
        }
        Task::TextToSfx | Task::AudioToMidi => unreachable!(),
    }
    // The SDK checks this explicit release's live operation limits before uploading. No combined
    // source + append limit is invented here, and the approved source is never silently trimmed.
    Ok(Input { request, details })
}

fn required(call: &Call, name: &str, zero: bool) -> Result<u64, OpError> {
    argument(call.args.get(name).unwrap_or(&Value::Null), name, zero)
}

fn argument(value: &Value, name: &str, zero: bool) -> Result<u64, OpError> {
    let invalid =
        || refused("invalid_request", &format!("Use finite {} seconds for {name}.", if zero { "non-negative" } else { "positive" }));
    let seconds = value.as_f64().filter(|seconds| seconds.is_finite() && *seconds >= 0.0).ok_or_else(invalid)?;
    let millis = (seconds * 1000.0).round();
    if !millis.is_finite() || millis > 9_007_199_254_740_991.0 || millis < if zero { 0.0 } else { 1.0 } {
        return Err(invalid());
    }
    Ok(millis as u64)
}

fn measured_millis(seconds: f64) -> Result<u64, OpError> {
    let millis = (seconds * 1000.0).ceil();
    if !millis.is_finite() || !(1.0..=9_007_199_254_740_991.0).contains(&millis) {
        return Err(refused("invalid_source", "The complete source duration could not be represented safely."));
    }
    Ok(millis as u64)
}

fn seconds(millis: u64) -> f64 {
    millis as f64 / 1000.0
}
