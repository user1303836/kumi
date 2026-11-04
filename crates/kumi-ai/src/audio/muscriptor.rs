use super::wire::{self, Client, Failure};
use crate::sse::SseParser;
use base64::{engine::general_purpose::STANDARD, Engine};
use kumi_common::abort::Signal;
use reqwest::{multipart, Method};
use serde_json::{json, Value};
use std::{collections::BTreeMap, path::Path};

pub(super) async fn execute(client: &Client, input: &Value, output: &Path, signal: &Signal) -> Result<Value, Failure> {
    let media = wire::media_file(wire::required_string(input, "audio")?, false).await?;
    if let Some(duration) = input.get("duration_ms") {
        if !duration.as_u64().is_some_and(|duration| duration > 0 && duration <= 900_000) {
            return Err(Failure::new("MuScriptor duration_ms must be positive and at most 15 minutes"));
        }
    }
    if input["quote"] == true {
        return Ok(
            json!({"provider":"local","protocol":"muscriptor","submitted":false,"quote":{"credits":0},"cost_basis":"User-run server; no Mirelo charge"}),
        );
    }
    let mut form = multipart::Form::new().text("detect_tempo", "best-effort");
    if let Some(instruments) = input.get("instruments") {
        let instruments = instruments
            .as_array()
            .filter(|values| values.len() <= 35)
            .ok_or_else(|| Failure::new("instruments must be an array of up to 35 names"))?;
        for instrument in instruments {
            let name = instrument.as_str().filter(|name| !name.is_empty()).ok_or_else(|| Failure::new("Invalid instrument name"))?;
            form = form.text("instruments", name.to_owned());
        }
    }
    let file = tokio::fs::File::open(&media.path).await.map_err(|_| Failure::new("Could not open media file"))?;
    let part = multipart::Part::stream_with_length(file, media.bytes)
        .file_name(media.name)
        .mime_str(media.mime)
        .map_err(|_| Failure::new("Invalid media MIME type"))?;
    let request = client.api(Method::POST, "/transcribe")?.header("Accept", "text/event-stream").multipart(form.part("file", part));
    let mut response = client.send(request, signal).await?;
    if !response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.starts_with("text/event-stream"))
    {
        return Err(Failure::new("MuScriptor /transcribe did not return its native SSE protocol"));
    }
    let mut parser = SseParser::default();
    let mut open = BTreeMap::<u64, Value>::new();
    let mut notes = Vec::new();
    let mut bytes = 0usize;
    loop {
        let chunk = tokio::select! { biased;
            _ = signal.cancelled() => return Err(Failure::new("MuScriptor transcription cancelled; the server may finish its current chunk")),
            chunk = response.chunk() => chunk.map_err(|_| Failure::new("MuScriptor stream interrupted before a complete transcription"))?
        };
        let Some(chunk) = chunk else {
            return Err(Failure::new("MuScriptor stream ended without transcription_complete; no complete result was saved"));
        };
        bytes += chunk.len();
        if bytes > 64 * 1024 * 1024 {
            return Err(Failure::new("MuScriptor event stream exceeds 64 MiB"));
        }
        for event in parser.push(&chunk) {
            let event: Value = serde_json::from_str(&event.data).map_err(|_| Failure::new("Invalid JSON in MuScriptor event stream"))?;
            match event["type"].as_str() {
                Some("start") => {
                    let index = event["index"].as_u64().ok_or_else(|| Failure::new("MuScriptor start event omitted its index"))?;
                    let pitch = event["pitch"]
                        .as_u64()
                        .filter(|pitch| *pitch <= 127)
                        .ok_or_else(|| Failure::new("Invalid MuScriptor note pitch"))?;
                    let start = seconds(&event, "start_time")?;
                    let instrument = wire::required_string(&event, "instrument")?;
                    if open.insert(index, json!({"pitch":pitch,"start":start,"instrument":instrument,"velocity":100})).is_some() {
                        return Err(Failure::new("Duplicate open MuScriptor note index"));
                    }
                }
                Some("end") => {
                    let index =
                        event["start_event_index"].as_u64().ok_or_else(|| Failure::new("MuScriptor end event omitted its note index"))?;
                    let mut note = open.remove(&index).ok_or_else(|| Failure::new("MuScriptor end event has no matching start"))?;
                    let end = seconds(&event, "end_time")?;
                    let start = note["start"].as_f64().unwrap_or_default();
                    if end < start {
                        return Err(Failure::new("MuScriptor note ends before it starts"));
                    }
                    note["end"] = json!(end.max(start + 0.01));
                    notes.push(note);
                }
                Some("transcription_complete") => {
                    if !open.is_empty() {
                        return Err(Failure::new("MuScriptor completed with unclosed notes"));
                    }
                    let midi = STANDARD
                        .decode(wire::required_string(&event, "data")?)
                        .map_err(|_| Failure::new("Invalid base64 MIDI in MuScriptor completion"))?;
                    if !midi.starts_with(b"MThd") {
                        return Err(Failure::new("MuScriptor completion did not contain MIDI"));
                    }
                    let delay = event
                        .pointer("/beat_grid/onset_delay")
                        .filter(|value| !value.is_null())
                        .map(|value| {
                            value.as_f64().filter(|value| value.is_finite()).ok_or_else(|| Failure::new("Invalid MuScriptor onset delay"))
                        })
                        .transpose()?
                        .unwrap_or(0.0);
                    for note in &mut notes {
                        let start = (note["start"].as_f64().unwrap_or_default() - delay).max(0.0);
                        let end = (note["end"].as_f64().unwrap_or_default() - delay).max(0.0);
                        note["start"] = json!(start);
                        note["end"] = json!(end);
                    }
                    notes.retain(|note| note["end"].as_f64() > note["start"].as_f64());
                    notes.sort_by(|a, b| a["start"].as_f64().unwrap_or_default().total_cmp(&b["start"].as_f64().unwrap_or_default()));
                    // The native exporter trims overlapping notes of the same instrument/pitch.
                    let mut previous = BTreeMap::<(String, u64), usize>::new();
                    for index in 0..notes.len() {
                        let key = (
                            notes[index]["instrument"].as_str().unwrap_or_default().to_owned(),
                            notes[index]["pitch"].as_u64().unwrap_or_default(),
                        );
                        if let Some(prior) = previous.insert(key, index) {
                            if notes[prior]["end"].as_f64() > notes[index]["start"].as_f64() {
                                notes[prior]["end"] = notes[index]["start"].clone();
                            }
                        }
                    }
                    notes.retain(|note| note["end"].as_f64() > note["start"].as_f64());
                    let path = wire::save(output, "mid", &midi, signal).await?;
                    let notes_path = wire::save_notes(output, &json!(notes), signal).await?;
                    return Ok(
                        json!({"provider":"local","protocol":"muscriptor","model":"muscriptor (user-run server)","status":"succeeded",
                        "midi_path":path,"files":[{"path":path,"type":"mid"}],"notes":notes,"notes_source":"native SSE; onset_delay removed; velocity=100 as in MuScriptor MIDI export",
                        "notes_path":notes_path,"beat_grid":event["beat_grid"],"credits":0}),
                    );
                }
                Some("progress") => {}
                Some("error") => return Err(Failure::new("MuScriptor reported a transcription error")),
                _ => return Err(Failure::new("Unknown MuScriptor event type; no complete result saved")),
            }
        }
    }
}
fn seconds(event: &Value, field: &str) -> Result<f64, Failure> {
    event[field]
        .as_f64()
        .filter(|value| value.is_finite() && *value >= 0.0)
        .ok_or_else(|| Failure::new(format!("Invalid MuScriptor {field}")))
}
