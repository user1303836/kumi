//! Transport and view actions, which report activity without creating change history.
use super::more_changes::bars;
use crate::core::contracts::{JsonObject, TrackChip};
use kumi_common::js::number::to_string;
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::sync::LazyLock;
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ActionKind {
    pub tool: String,
    pub preview: String,
    pub apply: String,
    pub description: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_schema: Option<JsonObject>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub since: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub newer: Option<JsonObject>,
    /// Sent with every request as a tool of its own (see catalog.rs).
    #[serde(default, skip_serializing)]
    pub core: bool,
}
pub use crate::core::plan::ActionSummary;
impl ActionSummary {
    fn title(title: impl Into<String>) -> Self {
        Self { title: title.into(), playing: None, recording: None }
    }
}
pub static ACTIONS: LazyLock<Vec<ActionKind>> = LazyLock::new(|| serde_json::from_str(include_str!("assets/actions.json")).unwrap());
fn safety() -> Value {
    json!({"safe":true,"provenance":"The producer asked Kumi to play this in their own Set.","scope":"open Set"})
}
fn truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(v) => *v,
        Value::Number(v) => v.as_f64().is_some_and(|n| n != 0.0 && !n.is_nan()),
        Value::String(v) => !v.is_empty(),
        _ => true,
    }
}
fn js_string(value: Option<&Value>) -> String {
    match value {
        None => "undefined".into(),
        Some(Value::Null) => "null".into(),
        Some(Value::String(v)) => v.clone(),
        Some(Value::Number(v)) => to_string(v.as_f64().unwrap()),
        Some(Value::Bool(v)) => v.to_string(),
        Some(Value::Object(_)) => "[object Object]".into(),
        Some(Value::Array(v)) => {
            v.iter().map(|v| if v.is_null() { String::new() } else { js_string(Some(v)) }).collect::<Vec<_>>().join(",")
        }
    }
}
impl ActionKind {
    pub fn has_prepare(&self) -> bool {
        matches!(self.tool.as_str(), "launch_clip" | "record")
    }
    pub fn prepare(&self, input: &JsonObject) -> Result<JsonObject, String> {
        let mut result = match self.tool.as_str() {
            "launch_clip" => {
                json!({"slotRef":input.get("slotRef").cloned().unwrap_or(Value::Null),"outputSafety":safety()}).as_object().unwrap().clone()
            }
            "record" => {
                let mut result = JsonObject::new();
                for key in ["action", "lane"] {
                    if let Some(value) = input.get(key) {
                        result.insert(key.into(), value.clone());
                    }
                }
                let start = input.get("action").and_then(Value::as_str) == Some("start");
                result.insert(
                    "intent".into(),
                    json!(if start { "The producer asked Kumi to record." } else { "The producer asked Kumi to stop recording." }),
                );
                if let Some(value) = input.get("destinationTrackRef").filter(|v| truthy(v)) {
                    result.insert("destinationTrackRef".into(), value.clone());
                }
                if let Some(values) = input.get("alsoTrackRefs").and_then(Value::as_array).filter(|v| start && !v.is_empty()) {
                    result.insert("alsoTrackRefs".into(), json!(values));
                }
                result.insert("outputSafety".into(), safety());
                result
            }
            _ => input.clone(),
        };
        Ok(std::mem::take(&mut result))
    }
    pub fn summarize(&self, preview: &JsonObject, input: &JsonObject, track: &dyn Fn(&Value) -> Option<TrackChip>) -> ActionSummary {
        let action = input.get("action").and_then(Value::as_str);
        let name = |key: &str| track(input.get(key).unwrap_or(&Value::Null)).map(|v| v.name);
        let mut result = match self.tool.as_str() {
            "play" => ActionSummary::title(match action {
                Some("start") => "Playing from the start marker",
                Some("continue") => "Playing on from where it stopped",
                Some("stop") => "Stopped",
                Some("play-selection") => "Playing the selection",
                Some("stop-all-clips") => "Stopped all clips",
                Some("tap-tempo") => "Tapped the tempo",
                Some("nudge-up") => "Nudged ahead",
                Some("nudge-down") => "Nudged back",
                Some("re-enable-automation") => "Automation back on",
                Some("trigger-session-record") => "Session recording",
                Some("back-to-arrangement") => "Back to the Arrangement",
                _ => "Transport",
            }),
            "fire_scene" => ActionSummary::title("Launched a scene"),
            "launch_clip" => {
                static SLOT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^([0-9]+):(?:clip_slot|slot):([0-9]+):([0-9]+)").unwrap());
                let found = input.get("slotRef").and_then(Value::as_str).and_then(|s| SLOT.captures(s));
                let known = found.as_ref().and_then(|m| track(&json!(format!("{}:track:{}", &m[1], &m[2]))));
                ActionSummary::title(format!(
                    "Playing the clip in {}{}",
                    known.map(|v| v.name).unwrap_or_else(|| "its slot".into()),
                    found.map(|m| format!(", scene {}", to_string(m[3].parse::<f64>().unwrap_or(f64::INFINITY) + 1.0))).unwrap_or_default()
                ))
            }
            "record" => ActionSummary::title(if action == Some("start") {
                format!(
                    "Recording{}{}",
                    if input.get("lane").and_then(Value::as_str) == Some("arrangement") { " in the Arrangement" } else { "" },
                    name("destinationTrackRef").map(|s| format!(" on {s}")).unwrap_or_default()
                )
            } else {
                "Recording stopped".into()
            }),
            "jump_to_locator" => ActionSummary::title(
                preview
                    .get("target")
                    .and_then(Value::as_f64)
                    .map(|n| format!("Playhead to {}", bars(n)))
                    .unwrap_or_else(|| "Playhead to the next locator".into()),
            ),
            "select" => ActionSummary::title(name("trackRef").map(|s| format!("Selected {s}")).unwrap_or_else(|| {
                if input.get("detailClipRef").is_some_and(truthy) {
                    "Showing the clip"
                } else if input.get("chainRef").is_some_and(truthy) {
                    "Showing the chain"
                } else {
                    "Selected it in Live"
                }
                .into()
            })),
            "show" => {
                let view =
                    input.get("view").and_then(Value::as_str).map(|s| s.replacen("Arranger", "Arrangement", 1).replacen("Detail/", "", 1));
                ActionSummary::title(if action == Some("focus-view") && view.as_ref().is_some_and(|s| !s.is_empty()) {
                    format!("Showing the {}", view.unwrap())
                } else {
                    format!(
                        "View: {}",
                        js_string(input.get("action").filter(|v| !v.is_null()).or(Some(&json!("changed")))).replace('-', " ")
                    )
                })
            }
            _ => unreachable!("known action kind"),
        };
        match self.tool.as_str() {
            "play" => match action {
                Some("start" | "continue" | "play-selection") => result.playing = Some(true),
                Some("stop") => result.playing = Some(false),
                Some("trigger-session-record") => result.recording = Some(true),
                _ => {}
            },
            "launch_clip" | "fire_scene" => result.playing = Some(true),
            "record" => result.recording = Some(action == Some("start")),
            _ => {}
        }
        result
    }
}
