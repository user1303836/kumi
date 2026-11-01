//! What every DAW's goal rig does alike (#399, K29): which short stretch of the reference a long part is screened on, how
//! a trial is keyed and what was heard of it kept, and how a trial's render is told against the reference. Each DAW's rig
//! (Live's in `kumi-daw-ableton`, REAPER's and Renoise's in `kumi-daw-own`) sets, renders and tidies in its own way.
use crate::engine::{heard_options, silent_render};
use indexmap::IndexMap;
use kumi_common::{
    abort::Signal,
    js::number::{round, to_fixed},
};
use kumi_media::audio::{
    self,
    analyze::Analysis,
    matching::{closeness, Focus},
};
use kumi_types::contracts::{AuditionRequest, Render, StructuralMove};

/// The most trials a rig keeps what it heard of.
const HEARD_MOST: usize = 2000;

/// The short stretch a long part is screened on while its search is young: `offset` beats into the part, `beats` long.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScreenPick {
    pub offset: f64,
    pub beats: f64,
}

/// A part six seconds or longer is screened on its most characteristic three seconds of the reference (the slices of
/// `slice` seconds whose loudness is highest and moves most); None for a shorter part. `part_beats` is the part's length
/// in beats at `tempo`.
pub fn screen_pick(reference: &Analysis, tempo: f64, part_beats: f64, slice: f64) -> Option<ScreenPick> {
    if part_beats * 60. / tempo < 6. {
        return None;
    }
    let beats = round(3. * tempo / 60.).max(2.);
    let lufs = &reference.over_time.lufs;
    let across = round(beats * 60. / tempo / slice).max(1.);
    let mut best = (0, f64::NEG_INFINITY);
    if across.is_finite() && across <= lufs.len() as f64 {
        let across = across as usize;
        for at in 0..=lufs.len() - across {
            let part: Vec<f64> = lufs[at..at + across].iter().map(|v| v.unwrap_or(-70.)).collect();
            let mean = part.iter().sum::<f64>() / part.len() as f64;
            let change: f64 = part.windows(2).map(|v| (v[1] - v[0]).abs()).sum();
            if mean + change > best.1 {
                best = (at, mean + change);
            }
        }
    }
    let offset = round(best.0 as f64 * slice * tempo / 60.).min(part_beats - beats).max(0.);
    Some(ScreenPick { offset, beats })
}

impl ScreenPick {
    /// The request that reads the reference over this stretch only.
    pub fn request(&self, request: &AuditionRequest, tempo: f64) -> AuditionRequest {
        let mut snippet = request.clone();
        snippet.reference_from = Some(request.reference_from.unwrap_or(0.) + self.offset * 60. / tempo);
        snippet.reference_seconds = Some(self.beats * 60. / tempo);
        snippet
    }
}

/// What a trial is known by: its slot, whether it was screened, and its values to four places.
pub fn trial_key(slot: &str, screened: bool, values: &[f64]) -> String {
    format!(
        "{slot}|{}|{}",
        if screened { "screen" } else { "full" },
        values.iter().map(|value| to_fixed(*value, 4)).collect::<Vec<_>>().join(",")
    )
}

/// How a trial's render compared with the reference.
#[derive(Debug, Clone, PartialEq)]
pub struct Told {
    pub score: f64,
    pub gaps: Vec<String>,
    pub structural: Option<StructuralMove>,
}

/// A render heard against the reference: how close it came, or None when it's silent.
pub fn tell(heard: &Analysis, against: &Analysis, focus: Option<Focus>) -> Option<Told> {
    if silent_render(heard) {
        return None;
    }
    let close = closeness(heard, against, focus);
    let structural = close.structural.map(|found| StructuralMove { gap: found.gap, r#move: found.r#move });
    Some(Told { score: close.score, gaps: close.gaps, structural })
}

/// Hears a render from where its part starts, for `seconds`.
pub async fn hear(render: &Render, seconds: f64, focus: Option<Focus>, signal: Signal) -> Result<Analysis, audio::AudioError> {
    audio::hear(&render.file, heard_options(render.start, seconds, focus, signal)).await
}

/// What a rig heard of its trials, by [`trial_key`], so the same values aren't heard twice: the oldest go past
/// [`HEARD_MOST`].
pub struct HeardCache<T> {
    heard: IndexMap<String, T>,
}

impl<T: Clone> Default for HeardCache<T> {
    fn default() -> Self {
        Self { heard: IndexMap::new() }
    }
}

impl<T: Clone> HeardCache<T> {
    pub fn get(&self, key: &str) -> Option<T> {
        self.heard.get(key).cloned()
    }
    pub fn contains(&self, key: &str) -> bool {
        self.heard.contains_key(key)
    }
    pub fn insert(&mut self, key: String, heard: T) {
        self.heard.insert(key, heard);
        if self.heard.len() > HEARD_MOST {
            self.heard.shift_remove_index(0);
        }
    }
}
