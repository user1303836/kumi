//! Audition requests, render timing, and the crash journal that restores Main.
use crate::{
    audio::analyze::Analysis,
    core::contracts::{AuditionCandidate, AuditionRequest, JsonObject, MIX_CANDIDATE},
};
use kumi_common::js::{
    json::file_text,
    string::{head, trim},
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{fs, io::Write, path::PathBuf, sync::LazyLock};
pub const AUDITION_TOOL: &str = "audition";
pub const RENDER_TOOL: &str = "render";
static DATA: LazyLock<Value> = LazyLock::new(|| serde_json::from_str(include_str!("assets/audition.json")).unwrap());
pub static AUDITION_DESCRIPTION: LazyLock<String> = LazyLock::new(|| DATA["description"].as_str().unwrap().into());
pub static AUDITION_SCHEMA: LazyLock<JsonObject> = LazyLock::new(|| DATA["schema"].as_object().unwrap().clone());
pub static RENDER_DESCRIPTION: LazyLock<String> = LazyLock::new(|| DATA["renderDescription"].as_str().unwrap().into());
pub static RENDER_SCHEMA: LazyLock<JsonObject> = LazyLock::new(|| DATA["renderSchema"].as_object().unwrap().clone());
pub fn audition_request(input: &JsonObject) -> Result<AuditionRequest, String> {
    let mut candidates = Vec::new();
    for item in input.get("candidates").and_then(Value::as_array).map(Vec::as_slice).unwrap_or_default() {
        let label = item.get("label").and_then(Value::as_str).map(trim).filter(|s| !s.is_empty()).map(|s| head(s, 60));
        if item.get("mix") == Some(&Value::Bool(true)) {
            if item.get("track").is_some() || item.get("clip").is_some() {
                return Err("A mix candidate is the whole mix: give it no track or clip.".into());
            }
            candidates.push(AuditionCandidate { track: MIX_CANDIDATE.into(), mix: Some(true), label, clip: None });
            continue;
        }
        let track = item
            .get("track")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .ok_or("Each candidate needs its track (a trackRef from discovery), or is the whole mix ({\"mix\": true}).")?;
        let clip = item.get("clip").and_then(Value::as_str).filter(|s| !s.is_empty()).map(str::to_owned);
        candidates.push(AuditionCandidate { track: track.into(), mix: None, label, clip });
    }
    if candidates.is_empty() || candidates.len() > 8 {
        return Err("Give 1 to 8 candidates.".into());
    }
    let mix = candidates.iter().any(|c| c.mix == Some(true));
    if mix && candidates.len() > 1 {
        return Err("The whole mix renders on its own: every other candidate would play into it. Audition the mix alone (and tracks in another call).".into());
    }
    let mut tracks = std::collections::HashSet::new();
    if !candidates.iter().all(|c| tracks.insert(c.track.clone())) {
        return Err("Each candidate is a track of its own; put ideas on separate tracks to hear them side by side.".into());
    }
    let number = |key: &str| input.get(key).and_then(Value::as_f64).filter(|v| v.is_finite());
    let from_beat = number("from_beat");
    let beats = number("beats");
    if from_beat.is_none() && candidates.iter().any(|c| c.clip.is_none()) {
        return Err("Say where the part is: from_beat (and beats) in the Arrangement, or a Session clip for each candidate.".into());
    }
    if beats.is_some_and(|v| !(v > 0.0 && v <= 64.0)) {
        return Err("beats is from just above 0 to 64.".into());
    }
    let focus = input.get("focus").and_then(Value::as_str).filter(|s| ["sound", "section"].contains(s)).or(if mix {
        Some("section")
    } else {
        None
    });
    Ok(AuditionRequest {
        candidates,
        from_beat,
        beats,
        reference: input.get("reference").and_then(Value::as_str).map(trim).filter(|s| !s.is_empty()).map(str::to_owned),
        reference_from: number("reference_from_seconds"),
        reference_seconds: number("reference_seconds"),
        focus: focus.map(|s| serde_json::from_value(Value::String(s.into())).unwrap()),
    })
}
pub fn silent_render(analysis: &Analysis) -> bool {
    analysis.loudness.integrated_lufs.is_none_or(|value| value < -60.0) || analysis.loudness.sample_peak_dbfs < -55.0
}
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct RenderSpan {
    pub position: f64,
    pub preroll: f64,
    pub wait: f64,
}
pub fn render_span(from_beat: f64, beats: f64, beats_per_bar: f64, tempo: f64, longer: bool, lead_seconds: Option<f64>) -> RenderSpan {
    let bar = beats_per_bar * 60.0 / tempo;
    let times = if longer { 2.0 } else { 1.0 };
    let minimum = (lead_seconds.unwrap_or(3.0) * times / bar).ceil();
    let lead = if minimum.is_nan() { f64::NAN } else { times.max(minimum) } * beats_per_bar;
    let position = if from_beat > lead { from_beat - lead } else { 0.0 };
    let preroll = from_beat - position;
    RenderSpan { position, preroll, wait: preroll + beats + beats_per_bar / 2.0 }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MainRestore {
    pub set: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    pub volume: f64,
    pub at: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scratch: Option<Vec<String>>,
}
#[derive(Debug, Clone)]
pub struct RestoreStore {
    file: PathBuf,
}
pub fn restore_store(file: impl Into<PathBuf>) -> RestoreStore {
    RestoreStore { file: file.into() }
}
impl RestoreStore {
    /// Whether the journal was kept. It's written beside itself and renamed into place, so a failed write (a full
    /// disk) leaves the one before whole, never a truncated one.
    pub fn save(&self, value: &impl Serialize) -> bool {
        let temporary = PathBuf::from(format!("{}.{}.tmp", self.file.display(), std::process::id()));
        let write = || -> Result<(), Box<dyn std::error::Error>> {
            if let Some(parent) = self.file.parent() {
                if !parent.as_os_str().is_empty() {
                    fs::create_dir_all(parent)?;
                }
            }
            let mut options = fs::OpenOptions::new();
            options.write(true).create(true).truncate(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            let mut file = options.open(&temporary)?;
            file.write_all(file_text(&serde_json::to_value(value)?).as_bytes())?;
            file.sync_all()?;
            drop(file);
            fs::rename(&temporary, &self.file)?;
            Ok(())
        };
        let kept = write().is_ok();
        if !kept {
            let _ = fs::remove_file(&temporary);
        }
        kept
    }
    pub fn load(&self) -> Option<JsonObject> {
        let bytes = fs::read(&self.file).ok()?;
        let value: Value = serde_json::from_str(&String::from_utf8_lossy(&bytes)).ok()?;
        let value = value.as_object()?;
        if value.get("set").is_some_and(Value::is_string)
            && value.get("volume").and_then(Value::as_f64).is_some_and(|v| (0.0..=1.0).contains(&v))
            && value.get("at").is_some_and(Value::is_number)
        {
            Some(value.clone())
        } else {
            None
        }
    }
    pub fn clear(&self) {
        let _ = fs::remove_file(&self.file);
    }
}
