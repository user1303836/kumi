//! What a change did, as HISTORY shows it: neutral, so every DAW's integration summarises its changes the same way.
use crate::core::contracts::*;
use crate::core::history::next_change_id;
use crate::core::refs::KnownTrack;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ChangeSummary {
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub track: Option<KnownTrack>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub range: Option<[f64; 2]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clip: Option<ClipPicture>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub colors: Option<ColorChange>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lines: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub devices: Option<DevicePlacement>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub picture: Option<ChangePicture>,
}
impl ChangeSummary {
    pub fn title(title: impl Into<String>) -> Self {
        Self { title: title.into(), ..Default::default() }
    }
}
pub fn new_record(family: ChangeFamily, summary: ChangeSummary, state: ChangeState, at: i64) -> ChangeRecord {
    ChangeRecord {
        id: next_change_id(),
        family,
        title: summary.title,
        track: summary.track,
        from: summary.from,
        to: summary.to,
        range: summary.range,
        colors: summary.colors,
        clip: summary.clip,
        devices: summary.devices,
        state,
        score: None,
        note: None,
        at,
        picture: summary.picture.and_then(ChangePicture::bounded),
    }
}
