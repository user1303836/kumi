//! Audition requests, render timing, and the crash journal of what a cut-off listen leaves to put right.
pub use crate::listening::tools::{AUDITION_TOOL, RENDER_TOOL};
use crate::{
    core::{
        contracts::{AuditionRequest, JsonObject},
        daw::DawProfile,
    },
    listening::tools::{audition_tool, render_tool, ToolDef},
};
use kumi_common::js::json::file_text;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{fs, io::Write, path::PathBuf, sync::LazyLock};
static AUDITION: LazyLock<ToolDef> = LazyLock::new(|| audition_tool(&DawProfile::ABLETON));
static RENDER: LazyLock<ToolDef> = LazyLock::new(|| render_tool(&DawProfile::ABLETON));
pub static AUDITION_DESCRIPTION: LazyLock<String> = LazyLock::new(|| AUDITION.description.clone());
pub static AUDITION_SCHEMA: LazyLock<JsonObject> = LazyLock::new(|| AUDITION.schema.clone());
pub static RENDER_DESCRIPTION: LazyLock<String> = LazyLock::new(|| RENDER.description.clone());
pub static RENDER_SCHEMA: LazyLock<JsonObject> = LazyLock::new(|| RENDER.schema.clone());
/// The audition tool's input as a request, or what's wrong with it.
pub fn audition_request(input: &JsonObject) -> Result<AuditionRequest, String> {
    crate::listening::tools::audition_request(&DawProfile::ABLETON, input)
}
pub use crate::listening::engine::silent_render;
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
/// What a cut-off listen leaves to put right: the record pass's render tracks, and Main's level in a journal an older
/// Kumi left (it turned Main down to listen; listens leave Main alone now).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RenderJournal {
    pub set: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub volume: Option<f64>,
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
            && value.get("volume").is_none_or(|v| v.as_f64().is_some_and(|v| (0.0..=1.0).contains(&v)))
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
