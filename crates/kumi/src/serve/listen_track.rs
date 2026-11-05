//! CC-2: the track branch of `listen`, never a second public tool or mutation path.
//! The MCP adapter holds the endpoint lease through capture, restoration, staging and publication.
use super::change::one_line;
use super::ctx::{epoch_of, epoch_schema, Call, Ctx, Effect, JsonObject, OpError, OpFuture};
use super::listen;
use kumi_types::contracts::{HearMethod, HearPreview, HearRequest};
use kumi_types::errors::{HearFailure, RuntimeError};
use serde_json::{json, Value};
use std::path::Path;

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
    if is_track(args) && args.get("preview") != Some(&Value::Bool(true)) { Effect::Playback } else { Effect::Read }
}

fn invalid() -> OpError {
    OpError::Invalid { reason: "Track listening requires one current track ref or the mix, from_beat >= 0 and 0 < beats <= 256, with a finite end, project key and epoch. No output path or implicit window is accepted.".into() }
}

fn request(args: &JsonObject) -> Result<HearRequest, OpError> {
    if args.keys().any(|key| !matches!(key.as_str(), "source" | "compare_to" | "expected_project_key" | "expected_epoch" | "preview"))
        || args.get("preview").is_some_and(|value| !value.is_boolean())
    { return Err(invalid()); }
    let source = args.get("source").and_then(Value::as_object).filter(|source| source.len() == 3).ok_or_else(invalid)?;
    let track = source.get("track").and_then(Value::as_object).filter(|track| track.len() == 1).ok_or_else(invalid)?;
    let from = source.get("from_beat").and_then(Value::as_f64).filter(|from| from.is_finite() && *from >= 0.).ok_or_else(invalid)?;
    let beats = source.get("beats").and_then(Value::as_f64).filter(|beats| beats.is_finite() && *beats > 0. && *beats <= 256.).ok_or_else(invalid)?;
    if !(from + beats).is_finite() || from + beats <= from { return Err(invalid()); }
    let (tracks, mix) = if track.get("mix") == Some(&Value::Bool(true)) {
        (vec![], Some(true))
    } else {
        let reference = track.get("ref").and_then(Value::as_str).filter(|value| !value.trim().is_empty() && value.len() <= 256).ok_or_else(invalid)?;
        (vec![reference.to_owned()], None)
    };
    Ok(HearRequest { tracks, mix, from_beat: Some(from), beats: Some(beats), seconds: None, whole: None, scratch_dir: None, expected_native_epoch: None })
}

fn outcome(state: &str, code: &str, reason: &str, restored: bool) -> Value {
    json!({"state":state,"code":code,"reason":one_line(reason, 500),"cleanup":if restored {"restored"} else {"unsure"}})
}

fn hearing_error(ctx: &Ctx, error: RuntimeError, dispatched: bool) -> Result<Value, OpError> {
    match error {
        RuntimeError::Hear(HearFailure::Refused { code, reason }) => {
            let mut result = outcome("refused", code, &reason, true);
            if code == "ears_missing" {
                let owned = ctx.paths().state_dir.parent().is_some_and(|data| super::connection::inside(&ctx.paths().kumi_home, data));
                result["fix"] = json!(if owned { "setup" } else { "owning_kumi" });
                let instruction = if owned {
                    "Run the DAW setup step again to add or update its capture device."
                } else {
                    "Run kumi bridge in the Kumi installation that owns this DAW bridge to add or update its capture device."
                };
                result["reason"] = json!(format!("{} {instruction}", one_line(&reason, 350)));
            }
            Ok(result)
        }
        RuntimeError::Hear(HearFailure::Unsure { reason }) => Ok(outcome("unsure", "restoration_unknown", &reason, false)),
        RuntimeError::Aborted => Ok(outcome("cancelled", "cancelled", "Listening was cancelled; temporary DAW state is restored.", true)),
        _ if dispatched => Ok(outcome("unsure", "capture_failed", "The capture did not return a verified restoration result. Observe again before another action.", false)),
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
    let key = call.args.get("expected_project_key").and_then(Value::as_str).filter(|key| !key.is_empty() && key.len() <= 64).ok_or_else(invalid)?;
    let epoch = epoch_of(&call.args, "expected_epoch").ok_or_else(invalid)?;
    let project = ctx.link().expect(key, epoch)?;
    request.expected_native_epoch = ctx.link().observed_native_epoch();
    let connection = ctx.connection()?;
    if call.signal.is_cancelled() { return hearing_error(ctx, RuntimeError::Aborted, false); }
    let reference = match call.args.get("compare_to") {
        Some(value) => Some((value, listen::resolve_source(ctx, value, &call.signal).await?)),
        None => None,
    };
    let preview = match connection.integration.preview_hear(&request, call.signal.clone()).await {
        Ok(preview) => preview,
        Err(error) => return hearing_error(ctx, error, false),
    };
    if !preview.seconds.is_finite() || preview.seconds <= 0. || preview.seconds > 60.
        || !(1..=360).contains(&preview.deadline_seconds)
    { return Err(OpError::Refused { code:"unsupported", reason:"The DAW did not provide a bounded capture plan.".into(), owner:None }); }
    if call.args.get("preview") == Some(&Value::Bool(true)) {
        return Ok(json!({"state":"preview","project":{"key":project.key,"epoch":project.epoch.to_string()},
            "text":preview_text(&request, &preview),"estimated_seconds":preview.seconds,
            "method":preview.method,"deadline_seconds":preview.deadline_seconds}));
    }
    // A bad reference cannot spend an approved playback. Measurement remains unpublished.
    let reference = match reference {
        Some((source, path)) => {
            let staged = listen::stage_source(ctx, source, &call.signal).await?;
            Some(listen::measure(ctx, staged, &path, &call.signal).await?)
        }
        None => None,
    };
    ctx.link().expect(key, epoch)?;
    let scratch = ctx.scratch("listen")?;
    request.scratch_dir = Some(scratch.path().to_path_buf());
    if preview.method == HearMethod::Offline {
        let mut directory = std::fs::DirBuilder::new();
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            directory.mode(0o700);
        }
        match directory.create(ctx.paths().state_dir.join("quarantine")) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(_) => return Err(OpError::Failed { reason: "The runtime could not prepare its private quarantine; no capture started.".into() }),
        }
    }
    let _settling = ctx.settling(preview.deadline_seconds as u16)?;
    call.progress.report(0., Some(3.), Some("Capturing the approved window; temporary DAW state will be restored."));
    let takes = match connection.integration.hear(&request, call.signal.clone()).await {
        Ok(Ok(takes)) => takes,
        Ok(Err(_)) => return Ok(outcome("unsure", "capture_failed", "The DAW returned no verified restoration outcome. Observe again before another action.", false)),
        Err(error) => return hearing_error(ctx, error, true),
    };
    if call.signal.is_cancelled() { return hearing_error(ctx, RuntimeError::Aborted, true); }
    let [take] = takes.as_slice() else {
        return Ok(outcome("unsure", "capture_failed", "The DAW returned an unexpected number of capture files; none was published.", false));
    };
    if take.start != 0. || !take.seconds.is_some_and(|seconds| seconds.is_finite() && seconds > 0. && seconds <= 65.) {
        return Ok(outcome("unsure", "capture_bounds", "The DAW returned audio outside the approved capture bound; none was published.", false));
    }
    call.progress.report(1., Some(3.), Some("DAW restoration verified; measuring the private capture."));
    // Everything after `hear` is local. A failure here must preserve its known restoration,
    // produce no successful metrics or MEDIA line, and drop both unpublished copies.
    let measured = async {
        let staged = ctx.artifacts().stage_scratch(ctx, &scratch, Path::new(&take.file), &call.signal).await?;
        listen::measure(ctx, staged, Path::new("track-capture.wav"), &call.signal).await
    }.await;
    let measured = match measured {
        Ok(measured) => measured,
        Err(error) => return Ok(outcome(if call.signal.is_cancelled() {"cancelled"} else {"refused"}, error.code(), error.reason(), true)),
    };
    let seconds = measured.metrics.seconds;
    call.progress.report(2., Some(3.), Some("Measurements complete; retaining the approved capture."));
    let mut result = match listen::retain(ctx, measured, reference, &call.signal).await {
        Ok(result) => result,
        Err(error) => return Ok(outcome(if call.signal.is_cancelled() {"cancelled"} else {"refused"}, error.code(), error.reason(), true)),
    };
    result["state"] = json!("done");
    result["capture"] = json!({"method":preview.method,"from_beat":request.from_beat,"beats":request.beats,
        "seconds":seconds,"cleanup":"restored"});
    if take.marked_modified { result["capture"]["marked_modified"] = json!(true); }
    if let Some(note) = &take.note {
        let note = one_line(note, 200);
        if !note.is_empty() { result["capture"]["note"] = json!(note); }
    }
    call.progress.report(3., Some(3.), Some("Capture retained; temporary DAW state is restored."));
    Ok(result)
}
