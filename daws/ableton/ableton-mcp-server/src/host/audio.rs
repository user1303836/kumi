//! Bounded PCM request validation and isolated analysis/diagnosis execution.
use super::*;
use crate::audio_diagnosis::{diagnose_audio_with_live_context_value, AudioSourceKind, AudioSourceProvenance};
use kumi_common::abort::Signal;

#[derive(Debug, Clone, PartialEq)]
pub struct EncodedSource {
    pub source: Value,
    pub sample_count: usize,
}
#[derive(Debug, Clone, PartialEq)]
pub enum PreparedAudioJob {
    Job(Value),
    Rejected(Value),
}
pub fn base64_float_count(value: &Value) -> Option<usize> {
    let text = value.as_str()?;
    if text.is_empty() || text.len() % 4 != 0 {
        return None;
    }
    let (content, padding) = match text.find('=') {
        Some(index) if index >= 2 && matches!(&text[index..], "=" | "==") => (&text[..index], text.len() - index),
        Some(_) => return None,
        None => (text, 0),
    };
    if !content.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'+' || c == b'/') {
        return None;
    }
    let bytes = text.len().checked_div(4)?.checked_mul(3)?.checked_sub(padding)?;
    (bytes > 0 && bytes % 4 == 0 && bytes <= 9_007_199_254_740_991).then_some(bytes / 4)
}
pub fn encoded_analysis_source(value: &Value, max_channels: usize, allow_frame_size: bool) -> Option<EncodedSource> {
    let fields = if allow_frame_size {
        &["pcmBase64", "sampleRate", "channels", "channelLayout", "frameSize"][..]
    } else {
        &["pcmBase64", "sampleRate", "channels", "channelLayout"][..]
    };
    if !has_only(value, fields) {
        return None;
    }
    let sample_count = base64_float_count(&value["pcmBase64"])?;
    let channels = value.get("channels").cloned().unwrap_or(json!(1));
    if !is_integer_in_range(&value["sampleRate"], 8000.0, 384000.0)
        || !is_integer_in_range(&channels, 1.0, max_channels as f64)
        || sample_count % channels.as_f64()? as usize != 0
    {
        return None;
    }
    if allow_frame_size && value.get("frameSize").is_some_and(|v| !is_integer_in_range(v, 256.0, 4096.0)) {
        return None;
    }
    if let Some(layout) = value.get("channelLayout") {
        let layout = layout.as_array()?;
        let mut seen = HashSet::new();
        if layout.len() != channels.as_f64()? as usize
            || layout.iter().any(|v| !v.as_str().is_some_and(|s| ["M", "L", "R", "C", "Ls", "Rs", "LFE"].contains(&s) && seen.insert(s)))
        {
            return None;
        }
    }
    let mut source = json!({"pcmBase64":value["pcmBase64"],"sampleRate":value["sampleRate"]});
    for key in ["channels", "channelLayout"] {
        if let Some(v) = value.get(key) {
            source[key] = v.clone();
        }
    }
    if allow_frame_size {
        if let Some(v) = value.get("frameSize") {
            source["frameSize"] = v.clone();
        }
    }
    Some(EncodedSource { source, sample_count })
}
pub fn prepare_audio_analyze(id: &Value, params: Option<&Value>) -> PreparedAudioJob {
    let parsed = encoded_analysis_source(params.unwrap_or(&Value::Null), 32, true);
    let Some(parsed) = parsed.filter(|v| v.sample_count <= 10_000_000) else {
        return PreparedAudioJob::Rejected(error(
            id,
            -32602,
            "audio_analyze requires bounded normalized float32 pcmBase64, sampleRate, and matching channel metadata",
            None,
        ));
    };
    PreparedAudioJob::Job(json!({"mode":"analyze","source":parsed.source}))
}
pub fn prepare_audio_compare(id: &Value, params: Option<&Value>) -> Result<PreparedAudioJob, LiveError> {
    let params = params.unwrap_or(&Value::Null);
    if !has_only(params, &["project", "reference", "alignment"]) {
        return Ok(PreparedAudioJob::Rejected(error(
            id,
            -32602,
            "audio_compare_reference requires project and reference PCM sources",
            None,
        )));
    }
    let project = encoded_analysis_source(&params["project"], 2, false);
    let reference = encoded_analysis_source(&params["reference"], 2, false);
    let valid = project.as_ref().zip(reference.as_ref()).is_some_and(|(p, r)| {
        (32000.0..=96000.0).contains(&p.source["sampleRate"].as_f64().unwrap())
            && (32000.0..=96000.0).contains(&r.source["sampleRate"].as_f64().unwrap())
            && p.sample_count + r.sample_count <= 4_000_000
    });
    if !valid {
        return Ok(PreparedAudioJob::Rejected(error(
            id,
            -32602,
            "audio comparison sources must use the validated 32000-96000 Hz range and fit the 4000000-sample pair limit",
            None,
        )));
    }
    if let Some(alignment) = params.get("alignment") {
        let valid = has_only(alignment, &["mode", "maxLagSeconds", "manualOffsetSeconds"])
            && match alignment.get("mode") {
                None => true,
                Some(mode) => ["auto", "manual", "disabled"].contains(&js_string(mode)?.as_str()),
            }
            && alignment.get("maxLagSeconds").is_none_or(|v| v.as_f64().is_some_and(|v| v.is_finite() && (0.0..=10.0).contains(&v)))
            && alignment.get("manualOffsetSeconds").is_none_or(|v| v.as_f64().is_some_and(|v| v.is_finite() && v.abs() <= 10.0));
        if !valid {
            return Ok(PreparedAudioJob::Rejected(error(id, -32602, "audio comparison alignment is invalid", None)));
        }
    }
    let mut job = json!({"mode":"compare","project":project.unwrap().source,"reference":reference.unwrap().source});
    if let Some(alignment) = params.get("alignment") {
        job["alignment"] = alignment.clone();
    }
    Ok(PreparedAudioJob::Job(job))
}
impl McpHost {
    pub async fn dispatch_audio_tool(&self, call: &ToolCall, signal: Option<&Signal>) -> Option<Result<Option<Value>, LiveError>> {
        if !call.asynchronous {
            return None;
        }
        Some(match call.name.as_str() {
            "audio_analyze" => Ok(self.audio_analyze_async(&call.id, call.arguments.as_ref(), signal).await),
            "audio_compare_reference" => self.audio_compare_reference_async(&call.id, call.arguments.as_ref(), signal).await,
            "audio_diagnose_live_context" => {
                Ok(self.audio_diagnose_live_context_async(&call.id, call.arguments.as_ref().unwrap_or(&Value::Null), signal).await)
            }
            _ => return None,
        })
    }
    pub async fn audio_analyze_async(&self, id: &Value, params: Option<&Value>, signal: Option<&Signal>) -> Option<Value> {
        let job = match prepare_audio_analyze(id, params) {
            PreparedAudioJob::Rejected(frame) => return Some(frame),
            PreparedAudioJob::Job(job) => job,
        };
        let result = self.analysis_runner.run_value(&job, signal.cloned(), None).await;
        if signal.is_some_and(Signal::is_cancelled) {
            return None;
        }
        Some(match result {
            Ok(value) => success_text(id, &value),
            Err(cause) => reason_error(
                id,
                &cause.to_string(),
                "Provide bounded little-endian float32 PCM normalized to [-1, 1], or retry after the isolated worker queue clears.",
            ),
        })
    }
    pub async fn audio_compare_reference_async(
        &self,
        id: &Value,
        params: Option<&Value>,
        signal: Option<&Signal>,
    ) -> Result<Option<Value>, LiveError> {
        let job = match prepare_audio_compare(id, params)? {
            PreparedAudioJob::Rejected(frame) => return Ok(Some(frame)),
            PreparedAudioJob::Job(job) => job,
        };
        let result = self.analysis_runner.run_value(&job, signal.cloned(), None).await;
        if signal.is_some_and(Signal::is_cancelled) {
            return Ok(None);
        }
        Ok(Some(match result {
            Ok(value)=>success_text(id,&value),
            Err(cause)=>reason_error(id,&cause.to_string(),"Use bounded PCM sources, explicit manual alignment for ambiguous material, or retry after the isolated worker queue clears."),
        }))
    }
    pub async fn audio_diagnose_live_context_async(&self, id: &Value, params: &Value, signal: Option<&Signal>) -> Option<Value> {
        if !has_only(params, &["pcmBase64", "sampleRate", "channels", "channelLayout", "trackRef", "provenance"])
            || !is_non_empty_string(&params["trackRef"], 256)
            || !has_only(&params["provenance"], &["observedAt", "description"])
            || !is_non_empty_string(&params["provenance"]["observedAt"], 128)
            || !is_non_empty_string(&params["provenance"]["description"], 512)
        {
            return Some(error(id, -32602, "bounded PCM, trackRef, and explicit source provenance are required", None));
        }
        let mut input = json!({"pcmBase64":params["pcmBase64"],"sampleRate":params["sampleRate"]});
        for key in ["channels", "channelLayout"] {
            if let Some(value) = params.get(key) {
                input[key] = value.clone();
            }
        }
        let Some(parsed) = encoded_analysis_source(&input, 2, false).filter(|v| v.sample_count <= 4_000_000) else {
            return Some(error(id, -32602, "diagnosis PCM metadata is invalid or exceeds the bounded source limit", None));
        };
        let result = async {
            let status = self.fresh_status(Some(&LiveOperationContext::with_deadline(self.deadline(reads::AUDITION_DEADLINE_MS)))).await?;
            if !status.connected || status.epoch.is_none() || !status.capabilities.iter().any(|c| c.as_str() == "session.read") {
                return Err(LiveError::error("fresh Live context is unavailable"));
            }
            let context = LiveOperationContext {
                signal: signal.cloned(),
                deadline_ms: Some(self.deadline(reads::AUDITION_DEADLINE_MS)),
                ..Default::default()
            };
            let snapshot = self.views.view_for(Some(&context), &[params["trackRef"].clone()], None, &[]).await?;
            let analysis = self
                .analysis_runner
                .run_value(&json!({"mode":"analyze","source":parsed.source}), signal.cloned(), None)
                .await
                .map_err(|e| LiveError::error(e.to_string()))?;
            if signal.is_some_and(Signal::is_cancelled) {
                return Ok(None);
            }
            let typed = serde_json::from_value(analysis.clone()).map_err(|e| LiveError::error(e.to_string()))?;
            let source = AudioSourceProvenance {
                kind: AudioSourceKind::CallerSuppliedPcm,
                observed_at: params["provenance"]["observedAt"].as_str().unwrap().into(),
                description: params["provenance"]["description"].as_str().unwrap().into(),
                capture_id: None,
            };
            let diagnosis = diagnose_audio_with_live_context_value(
                &typed,
                &serde_json::to_value(snapshot).unwrap(),
                status.epoch.unwrap(),
                params["trackRef"].as_str().unwrap(),
                &source,
                None,
            )
            .map_err(|e| LiveError::error(e.to_string()))?;
            Ok(Some(success_text(id, &json!({"analysis":analysis,"diagnosis":diagnosis}))))
        }
        .await;
        if signal.is_some_and(Signal::is_cancelled) {
            return None;
        }
        match result {
            Ok(result) => result,
            Err(cause) => Some(adapter_tool_error(
                id,
                &cause,
                "Audio was not attributed to Live; refresh the exact track context and source provenance before retrying.",
            )),
        }
    }
}
