//! The disposable analysis process: one job as JSON on stdin, one `{ ok, result | error }`
//! envelope on stdout, exit code 1 on failure.

use std::io::{Read, Write};

use kumi_common::js::{json, string};
use serde_json::{json as json_value, Map, Value};

use crate::analysis::{analyze_pcm, decode_float32_le, PcmAnalysisInput};
use crate::analysis_runner::MAX_ANALYSIS_JOB_REQUEST_BYTES;
use crate::audio_standards::{is_integer, ConventionalChannelLabel};
use crate::reference_analysis::{
    compare_reference_audio_mode, AlignmentMode, AlignmentOptions, ReferenceComparisonInput, ReferencePcmSource,
};

struct EncodedSource {
    pcm_base64: String,
    sample_rate: f64,
    channels: Option<f64>,
    channel_layout: Option<Vec<ConventionalChannelLabel>>,
    frame_size: Option<f64>,
}

enum WorkerRequest {
    Analyze { source: EncodedSource },
    Compare { project: EncodedSource, reference: EncodedSource, alignment: Option<WorkerAlignment> },
}

struct WorkerAlignment {
    options: AlignmentOptions,
    raw_mode: Option<Value>,
}

fn json_string(value: &Value) -> Result<String, String> {
    crate::host::helpers::js_string(value).map_err(|error| error.message().to_owned())
}

fn has_only(value: &Map<String, Value>, keys: &[&str]) -> bool {
    value.keys().all(|key| keys.contains(&key.as_str()))
}

fn is_integer_value(value: &Value) -> bool {
    value.as_f64().is_some_and(is_integer)
}

fn parse_source(value: Option<&Value>, label: &str) -> Result<EncodedSource, String> {
    let invalid = || format!("{label} is invalid");
    let Some(value) = value.and_then(Value::as_object) else { return Err(invalid()) };
    if !has_only(value, &["pcmBase64", "sampleRate", "channels", "channelLayout", "frameSize"])
        || !value.get("pcmBase64").is_some_and(Value::is_string)
        || !value.get("sampleRate").is_some_and(is_integer_value)
        || value.get("channels").is_some_and(|channels| !is_integer_value(channels))
        || value.get("frameSize").is_some_and(|frame_size| !is_integer_value(frame_size))
    {
        return Err(invalid());
    }
    let channel_layout = match value.get("channelLayout") {
        None => None,
        Some(Value::Array(items)) => {
            let mut labels = Vec::with_capacity(items.len());
            for item in items {
                if ConventionalChannelLabel::parse(&json_string(item)?).is_none() {
                    return Err(format!("{label}.channelLayout is invalid"));
                }
                labels.push(item.as_str().and_then(ConventionalChannelLabel::parse).unwrap_or(ConventionalChannelLabel::Invalid));
            }
            Some(labels)
        }
        Some(_) => return Err(format!("{label}.channelLayout is invalid")),
    };
    Ok(EncodedSource {
        pcm_base64: value["pcmBase64"].as_str().unwrap_or_default().to_string(),
        sample_rate: value["sampleRate"].as_f64().unwrap_or_default(),
        channels: value.get("channels").and_then(Value::as_f64),
        channel_layout,
        frame_size: value.get("frameSize").and_then(Value::as_f64),
    })
}

fn parse_alignment(value: Option<&Value>) -> Result<Option<WorkerAlignment>, String> {
    let Some(value) = value else { return Ok(None) };
    let invalid = || "alignment is invalid".to_string();
    let Some(value) = value.as_object() else { return Err(invalid()) };
    if !has_only(value, &["mode", "maxLagSeconds", "manualOffsetSeconds"]) {
        return Err(invalid());
    }
    if let Some(mode) = value.get("mode") {
        if !["auto", "manual", "disabled"].contains(&json_string(mode)?.as_str()) {
            return Err(invalid());
        }
    }
    if value.get("maxLagSeconds").is_some_and(|seconds| !seconds.is_number())
        || value.get("manualOffsetSeconds").is_some_and(|seconds| !seconds.is_number())
    {
        return Err(invalid());
    }
    Ok(Some(WorkerAlignment {
        options: AlignmentOptions {
            mode: value.get("mode").map(|mode| match mode.as_str() {
                Some("manual") => AlignmentMode::Manual,
                Some("disabled") => AlignmentMode::Disabled,
                _ => AlignmentMode::Auto,
            }),
            max_lag_seconds: value.get("maxLagSeconds").and_then(Value::as_f64),
            manual_offset_seconds: value.get("manualOffsetSeconds").and_then(Value::as_f64),
        },
        raw_mode: value.get("mode").cloned(),
    }))
}

fn read_request(input: &mut dyn Read) -> Result<WorkerRequest, String> {
    let mut bytes = Vec::new();
    let mut buffer = vec![0u8; 64 * 1024];
    loop {
        let count = input.read(&mut buffer).map_err(|cause| cause.to_string())?;
        if count == 0 {
            break;
        }
        if bytes.len() + count > MAX_ANALYSIS_JOB_REQUEST_BYTES {
            return Err("analysis job request exceeds the worker input limit".to_string());
        }
        bytes.extend_from_slice(&buffer[..count]);
    }
    let text = String::from_utf8_lossy(&bytes);
    let value: Value = serde_json::from_str(&text).map_err(|cause| cause.to_string())?;
    let Some(value) = value.as_object() else { return Err("analysis job mode is invalid".to_string()) };
    match value.get("mode").and_then(Value::as_str) {
        Some("analyze") => {
            if !has_only(value, &["mode", "source"]) {
                return Err("analysis job has unknown fields".to_string());
            }
            Ok(WorkerRequest::Analyze { source: parse_source(value.get("source"), "source")? })
        }
        Some("compare") => {
            if !has_only(value, &["mode", "project", "reference", "alignment"]) {
                return Err("comparison job has unknown fields".to_string());
            }
            let alignment = parse_alignment(value.get("alignment"))?;
            Ok(WorkerRequest::Compare {
                project: parse_source(value.get("project"), "project")?,
                reference: parse_source(value.get("reference"), "reference")?,
                alignment,
            })
        }
        _ => Err("analysis job mode is invalid".to_string()),
    }
}

struct DecodedSource {
    samples: Vec<f64>,
    sample_rate: f64,
    channels: f64,
    channel_layout: Option<Vec<ConventionalChannelLabel>>,
    frame_size: Option<f64>,
}

fn decode(source: EncodedSource) -> Result<DecodedSource, String> {
    let samples = decode_float32_le(&source.pcm_base64).map_err(|cause| cause.0)?;
    Ok(DecodedSource {
        samples: samples.into_iter().map(|sample| sample as f64).collect(),
        sample_rate: source.sample_rate,
        channels: source.channels.unwrap_or(1.0),
        channel_layout: source.channel_layout,
        // `frameSize ? { frameSize } : {}`: a zero frame size is left to the default.
        frame_size: source.frame_size.filter(|frame_size| *frame_size != 0.0),
    })
}

fn run(input: &mut dyn Read) -> Result<Value, String> {
    let request = read_request(input)?;
    let result = match request {
        WorkerRequest::Analyze { source } => {
            let decoded = decode(source)?;
            let analysis = analyze_pcm(&PcmAnalysisInput {
                samples: &decoded.samples,
                sample_rate: decoded.sample_rate,
                channels: Some(decoded.channels),
                channel_layout: decoded.channel_layout.as_deref(),
                frame_size: decoded.frame_size,
            })
            .map_err(|cause| cause.0)?;
            serde_json::to_value(analysis).map_err(|cause| cause.to_string())?
        }
        WorkerRequest::Compare { project, reference, alignment } => {
            let project = decode(project)?;
            let reference = decode(reference)?;
            let string_mode = alignment.as_ref().and_then(|alignment| alignment.raw_mode.as_ref()).is_none_or(Value::is_string);
            let comparison = compare_reference_audio_mode(
                &ReferenceComparisonInput {
                    project: ReferencePcmSource {
                        samples: &project.samples,
                        sample_rate: project.sample_rate,
                        channels: project.channels,
                        channel_layout: project.channel_layout.as_deref(),
                    },
                    reference: ReferencePcmSource {
                        samples: &reference.samples,
                        sample_rate: reference.sample_rate,
                        channels: reference.channels,
                        channel_layout: reference.channel_layout.as_deref(),
                    },
                    alignment: alignment.as_ref().map(|alignment| alignment.options),
                },
                string_mode,
            )
            .map_err(|cause| cause.0)?;
            let mut result = serde_json::to_value(comparison).map_err(|cause| cause.to_string())?;
            if let Some(mode) = alignment.and_then(|alignment| alignment.raw_mode) {
                result["alignment"]["mode"] = mode;
            }
            result
        }
    };
    Ok(result)
}

/// The worker's entry point: reads the job from stdin, writes the envelope to stdout, and returns
/// the exit code.
pub fn main() -> i32 {
    let outcome = run(&mut std::io::stdin().lock());
    let mut stdout = std::io::stdout().lock();
    let (text, code) = match outcome {
        Ok(result) => (json::stringify(&json_value!({ "ok": true, "result": result })), 0),
        Err(message) => (json::stringify(&json_value!({ "ok": false, "error": string::head(&message, 1_024) })), 1),
    };
    let _ = stdout.write_all(text.as_bytes());
    let _ = stdout.flush();
    code
}
