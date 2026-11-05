//! CC-2: the track branch of `listen`, never a second public tool or mutation path.
//! The MCP adapter holds the endpoint lease through capture, restoration, staging and publication.
use super::change::one_line;
use super::ctx::{epoch_of, epoch_schema, Call, Ctx, Effect, JsonObject, OpError, OpFuture};
use super::listen;
use kumi_common::abort::{self, Signal};
use kumi_types::contracts::{HearApproval, HearMethod, HearPreview, HearRequest};
use kumi_types::daw::DawId;
use kumi_types::errors::{HearFailure, RuntimeError};
use serde_json::{json, Value};
use std::{path::Path, time::Duration};

pub fn schema() -> Value {
    json!({"type":"object","oneOf": [
        listen::file_schema(),
        {"type":"object","additionalProperties":false,
            "required":["source","expected_project_key","expected_epoch"],
            "properties":{
                "source":{"type":"object","additionalProperties":false,"required":["track","from_beat","beats"],
                    "properties":{
                        "track":{"oneOf":[
                            {"type":"object","additionalProperties":false,"required":["ref"],"properties":{"ref":{"type":"string","minLength":1,"maxLength":256}}},
                            {"type":"object","additionalProperties":false,"required":["mix"],"properties":{"mix":{"const":true}}}
                        ]},
                        "from_beat":{"type":"number","minimum":0},
                        "beats":{"type":"number","exclusiveMinimum":0,"maximum":256}
                    }},
                "compare_to":listen::source_schema(),
                "expected_project_key":{"type":"string","minLength":1,"maxLength":64},
                "expected_epoch":epoch_schema(),
                "preview":{"type":"boolean","description":"Host-only: describe the approved capture without starting it."}
            }}
    ]})
}

pub(super) fn is_track(args: &JsonObject) -> bool {
    args.get("source").and_then(|source| source.get("track")).is_some()
}

pub fn effect(args: &JsonObject) -> Effect {
    if is_track(args) && args.get("preview") != Some(&Value::Bool(true)) {
        Effect::Playback
    } else {
        Effect::Read
    }
}

fn invalid() -> OpError {
    OpError::Invalid { reason: "Track listening requires one current track ref or the mix, from_beat >= 0 and 0 < beats <= 256, with a finite end, project key and epoch. No output path or implicit window is accepted.".into() }
}

fn request(args: &JsonObject) -> Result<HearRequest, OpError> {
    if args.keys().any(|key| !matches!(key.as_str(), "source" | "compare_to" | "expected_project_key" | "expected_epoch" | "preview"))
        || args.get("preview").is_some_and(|value| !value.is_boolean())
    {
        return Err(invalid());
    }
    let source = args.get("source").and_then(Value::as_object).filter(|source| source.len() == 3).ok_or_else(invalid)?;
    let track = source.get("track").and_then(Value::as_object).filter(|track| track.len() == 1).ok_or_else(invalid)?;
    let from = source.get("from_beat").and_then(Value::as_f64).filter(|from| from.is_finite() && *from >= 0.).ok_or_else(invalid)?;
    let beats = source
        .get("beats")
        .and_then(Value::as_f64)
        .filter(|beats| beats.is_finite() && *beats > 0. && *beats <= 256.)
        .ok_or_else(invalid)?;
    if !(from + beats).is_finite() || from + beats <= from {
        return Err(invalid());
    }
    let (tracks, mix) = if track.get("mix") == Some(&Value::Bool(true)) {
        (vec![], Some(true))
    } else {
        let reference =
            track.get("ref").and_then(Value::as_str).filter(|value| !value.trim().is_empty() && value.len() <= 256).ok_or_else(invalid)?;
        (vec![reference.to_owned()], None)
    };
    Ok(HearRequest { tracks, mix, from_beat: Some(from), beats: Some(beats), ..Default::default() })
}

fn approval(call: &Call) -> Result<(HearApproval, u64), OpError> {
    let missing = || OpError::Refused {
        code: "not_approved",
        reason: "This capture has no host-approved window and deadlines; preview it again.".into(),
        owner: None,
    };
    let invalid = || OpError::Invalid {
        reason: "_listen requires estimated_seconds in (0, 60], integer prepare_seconds in 0–120 and integer deadline_seconds in 1–360."
            .into(),
    };
    let given = call.host.private.get("_listen").ok_or_else(missing)?.as_object().filter(|given| given.len() == 3).ok_or_else(invalid)?;
    let seconds = given
        .get("estimated_seconds")
        .and_then(Value::as_f64)
        .filter(|seconds| seconds.is_finite() && *seconds > 0. && *seconds <= 60.)
        .ok_or_else(invalid)?;
    let deadline =
        given.get("deadline_seconds").and_then(Value::as_u64).filter(|deadline| (1..=360).contains(deadline)).ok_or_else(invalid)?;
    let prepare = given
        .get("prepare_seconds")
        .and_then(Value::as_u64)
        .filter(|prepare| *prepare <= 120 && (call.args.contains_key("compare_to") || *prepare == 0))
        .ok_or_else(invalid)?;
    Ok((HearApproval { seconds, deadline_seconds: deadline as u16 }, prepare))
}

/// Preparation cannot spend any of the approved native phase. Its cancellation reaches both
/// the converter and the DSP worker before the unpublished stage is dropped.
async fn prepare_reference(ctx: &Ctx, source: &Value, original: &Signal, seconds: u64) -> Result<listen::MeasuredAudio, OpError> {
    if seconds == 0 {
        return Err(OpError::Refused {
            code: "prepare_timeout",
            reason: "The reference has no preparation allowance; nothing was captured or rendered.".into(),
            owner: None,
        });
    }
    let stop = Signal::new();
    let signal = abort::any([original.clone(), stop.clone()]);
    let work = async {
        let path = listen::resolve_source(ctx, source, &signal).await?;
        let staged = listen::stage_source(ctx, source, &signal).await?;
        listen::measure(ctx, staged, &path, &signal).await
    };
    tokio::select! {
        biased;
        _ = tokio::time::sleep(Duration::from_secs(seconds)) => {
            stop.cancel();
            Err(OpError::Refused { code: "prepare_timeout", reason: "The reference exceeded its preparation deadline; nothing was captured or rendered.".into(), owner: None })
        }
        _ = original.cancelled() => {
            stop.cancel();
            Err(OpError::Refused { code: "cancelled", reason: "Reference preparation was cancelled; nothing was captured or rendered.".into(), owner: None })
        }
        measured = work => measured,
    }
}

fn outcome(state: &str, code: &str, reason: &str, restored: bool, marked_modified: bool) -> Value {
    const MODIFIED: &str = " The DAW reports that the project is marked modified.";
    let mut reason = one_line(reason, 500 - if marked_modified { MODIFIED.len() } else { 0 });
    if marked_modified {
        reason.push_str(MODIFIED);
    }
    json!({"state":state,"code":code,"reason":reason,"cleanup":if restored {"restored"} else {"unsure"}})
}

fn hearing_error(error: RuntimeError, dispatched: bool) -> Result<Value, OpError> {
    match error {
        RuntimeError::Hear(HearFailure::Refused { code, reason, marked_modified }) => {
            Ok(outcome("refused", code, &reason, true, marked_modified))
        }
        RuntimeError::Hear(HearFailure::Unsure { reason, restored, marked_modified }) => {
            Ok(outcome("unsure", if restored { "capture_failed" } else { "restoration_unknown" }, &reason, restored, marked_modified))
        }
        RuntimeError::Hear(HearFailure::Cancelled { marked_modified }) => {
            Ok(outcome("cancelled", "cancelled", "Listening was cancelled; temporary DAW state is restored.", true, marked_modified))
        }
        RuntimeError::Aborted => {
            Ok(outcome("cancelled", "cancelled", "Listening was cancelled; temporary DAW state is restored.", true, false))
        }
        _ if dispatched => Ok(outcome(
            "unsure",
            "capture_failed",
            "The capture did not return a verified restoration result. Observe again before another action.",
            false,
            false,
        )),
        _ => Err(OpError::Failed { reason: "The DAW could not provide a complete capture preflight; no capture started.".into() }),
    }
}

fn preview_text(request: &HearRequest, preview: &HearPreview) -> String {
    let from = request.from_beat.expect("validated window");
    let to = from + request.beats.expect("validated window");
    let action = if preview.method == HearMethod::Ears { "Capture" } else { "Render" };
    let label = one_line(&preview.label, 80).replace('"', "'");
    let target = request.tracks.first().map(|reference| format!("{} ", one_line(reference, 80))).unwrap_or_default();
    format!("{action} beats {from}–{to} ({:.3} s); restore the temporary transport, routing and device state before returning. {} Target: {target}\"{label}\".", preview.seconds, preview.disclosure)
}

pub fn run(ctx: &Ctx, call: Call) -> OpFuture<'_> {
    Box::pin(capture(ctx, call))
}

async fn capture(ctx: &Ctx, call: Call) -> Result<Value, OpError> {
    let mut request = request(&call.args)?;
    let key = call
        .args
        .get("expected_project_key")
        .and_then(Value::as_str)
        .filter(|key| !key.is_empty() && key.len() <= 64)
        .ok_or_else(invalid)?;
    let epoch = epoch_of(&call.args, "expected_epoch").ok_or_else(invalid)?;
    let project = ctx.link().expect(key, epoch)?;
    request.expected_native_epoch = project.native_epoch;
    let connection = ctx.connection()?;
    if call.signal.is_cancelled() {
        return hearing_error(RuntimeError::Aborted, false);
    }
    let preview_only = call.args.get("preview") == Some(&Value::Bool(true));
    let approved = if !preview_only || call.host.private.get("_listen").is_some() { Some(approval(&call)?) } else { None };
    if !preview_only {
        request.approval = approved.map(|(native, _)| native);
    }
    let reference = match call.args.get("compare_to") {
        Some(source) if preview_only => {
            listen::resolve_source(ctx, source, &call.signal).await?;
            None
        }
        Some(source) => Some(prepare_reference(ctx, source, &call.signal, approved.expect("actual capture requires approval").1).await?),
        None => None,
    };
    ctx.link().expect(key, epoch)?;
    if preview_only {
        let preview = match connection.integration.preview_hear(&request, call.signal.clone()).await {
            Ok(preview) => preview,
            Err(error) => return hearing_error(error, false),
        };
        if !preview.seconds.is_finite() || preview.seconds <= 0. || preview.seconds > 60. || !(1..=360).contains(&preview.deadline_seconds)
        {
            return Err(OpError::Refused {
                code: "unsupported",
                reason: "The DAW did not provide a bounded capture plan.".into(),
                owner: None,
            });
        }
        return Ok(json!({"state":"preview","project":{"key":project.key,"epoch":project.epoch.to_string()},
            "text":preview_text(&request, &preview),"estimated_seconds":preview.seconds,
            "method":preview.method,"deadline_seconds":preview.deadline_seconds,
            "prepare_seconds":if call.args.contains_key("compare_to") {120} else {0}}));
    }
    let native = approved.expect("actual capture requires approval").0;
    // Each adapter runs the same pure preflight again inside `hear`, checking this approval
    // before changing anything. Do not spend the native allowance on a duplicate preflight.
    let _settling = ctx.settling(native.deadline_seconds)?;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(u64::from(native.deadline_seconds));
    let scratch = ctx.scratch("listen")?;
    request.scratch_dir = Some(scratch.path().to_path_buf());
    if ctx.daw() != DawId::Ableton {
        let mut directory = std::fs::DirBuilder::new();
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            directory.mode(0o700);
        }
        match directory.create(ctx.paths().state_dir.join("quarantine")) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(_) => {
                return Err(OpError::Failed { reason: "The runtime could not prepare its private quarantine; no capture started.".into() })
            }
        }
    }
    call.progress.report(0., Some(3.), Some("Capturing the approved window; temporary DAW state will be restored."));
    let takes = match tokio::time::timeout_at(deadline, connection.integration.hear(&request, call.signal.clone())).await {
        Ok(Ok(Ok(takes))) => takes,
        Ok(Ok(Err(_))) => {
            return Ok(outcome(
                "unsure",
                "capture_failed",
                "The DAW returned no verified restoration outcome. Observe again before another action.",
                false,
                false,
            ))
        }
        Ok(Err(error)) => return hearing_error(error, true),
        Err(_) => {
            return Ok(outcome(
                "unsure",
                "capture_timeout",
                "The approved native deadline expired before restoration was verified. Observe again before another action.",
                false,
                false,
            ))
        }
    };
    let [take] = takes.as_slice() else {
        return Ok(outcome(
            "unsure",
            "capture_failed",
            "The DAW returned an unexpected number of capture files; none was published.",
            false,
            false,
        ));
    };
    if call.signal.is_cancelled() {
        return hearing_error(HearFailure::Cancelled { marked_modified: take.marked_modified }.into(), true);
    }
    if take.start != 0. || !take.seconds.is_some_and(|seconds| seconds.is_finite() && seconds > 0. && seconds <= 65.) {
        return Ok(outcome(
            "unsure",
            "capture_bounds",
            "The DAW returned audio outside the approved capture bound; none was published.",
            true,
            take.marked_modified,
        ));
    }
    call.progress.report(1., Some(3.), Some("DAW restoration verified; measuring the private capture."));
    // Everything after `hear` is local. A failure here must preserve its known restoration,
    // produce no successful metrics or MEDIA line, and drop both unpublished copies.
    let stop = Signal::new();
    let signal = abort::any([call.signal.clone(), stop.clone()]);
    let measured = async {
        let staged = ctx.artifacts().stage_scratch(ctx, &scratch, Path::new(&take.file), &signal).await?;
        listen::measure(ctx, staged, Path::new("track-capture.wav"), &signal).await
    };
    let measured = match tokio::time::timeout_at(deadline, measured).await {
        Ok(Ok(measured)) => measured,
        Ok(Err(error)) => {
            return Ok(outcome(
                if call.signal.is_cancelled() { "cancelled" } else { "refused" },
                error.code(),
                "The captured audio could not be validated after DAW restoration; no audio was published.",
                true,
                take.marked_modified,
            ))
        }
        Err(_) => {
            stop.cancel();
            return Ok(outcome(
                "refused",
                "measure_timeout",
                "The native deadline expired during local measurement; DAW state is restored and no audio was published.",
                true,
                take.marked_modified,
            ));
        }
    };
    let seconds = measured.metrics.seconds;
    call.progress.report(2., Some(3.), Some("Measurements complete; retaining the approved capture."));
    let mut result = match listen::retain(ctx, measured, reference, &call.signal, Some(deadline)).await {
        Ok(result) => result,
        Err(error) => {
            return Ok(outcome(
                if error.code() == "cancelled" { "cancelled" } else { "refused" },
                error.code(),
                "The validated audio could not be retained after DAW restoration; no audio was published.",
                true,
                take.marked_modified,
            ))
        }
    };
    result["state"] = json!("done");
    result["capture"] = json!({"method":if take.live {HearMethod::Ears} else {HearMethod::Offline},"from_beat":request.from_beat,"beats":request.beats,
        "seconds":seconds,"cleanup":"restored"});
    if take.marked_modified {
        result["capture"]["marked_modified"] = json!(true);
    }
    if let Some(note) = &take.note {
        let note = one_line(note, 200);
        if !note.is_empty() {
            result["capture"]["note"] = json!(note);
        }
    }
    call.progress.report(3., Some(3.), Some("Capture retained; temporary DAW state is restored."));
    Ok(result)
}

