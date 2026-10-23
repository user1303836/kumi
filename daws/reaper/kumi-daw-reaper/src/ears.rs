//! REAPER's ears (#401): audio to MIDI writes the notes in one MIDI item on a new track under the source, and stems put each
//! on a new track under it, an item over the stretch.
use crate::ReaperRefs;
use async_trait::async_trait;
use kumi_agent::core::stems::backend::Words as StemWords;
use kumi_agent::core::to_midi::backend::Words;
use kumi_agent::core::to_midi::MidiNote;
use kumi_common::abort::Signal;
use kumi_daw_own::ears::{EarsDaw, OwnEars};
use kumi_types::contracts::JsonObject;
use kumi_types::daw::DawProfile;
use serde_json::{json, Value};
use std::path::Path;

pub static REAPER_WORDS: Words = Words {
    app: "REAPER",
    written: "a new MIDI track under it, one item over the stretch (no instrument on it: add one to hear them)",
    options: "none in REAPER",
    options_schema: r#"{"type":"object","additionalProperties":false,"properties":{}}"#,
};

pub static REAPER_STEM_WORDS: StemWords =
    StemWords { app: "REAPER", placed: "a new track each under it, its audio an item over the stretch" };

/// REAPER's ears.
pub struct ReaperEars;

#[async_trait(?Send)]
impl EarsDaw<ReaperRefs> for ReaperEars {
    fn words(&self) -> &'static Words {
        &REAPER_WORDS
    }

    fn stem_words(&self) -> &'static StemWords {
        &REAPER_STEM_WORDS
    }

    /// As Kumi's own ears have always named the notes they heard, REAPER's too: the tracker's way ("C-4").
    fn note_name(&self, pitch: u8) -> String {
        DawProfile::RENOISE.note_name(pitch.min(119))
    }

    async fn write(
        &self,
        ears: &OwnEars<ReaperRefs>,
        name: &str,
        index: u64,
        notes: &[MidiNote],
        _drums: bool,
        from: f64,
        beats: f64,
        _options: &JsonObject,
        signal: &Signal,
    ) -> Result<Value, String> {
        ears.run(reaper_steps(name, index, notes, from, beats), signal).await
    }

    async fn place(
        &self,
        ears: &OwnEars<ReaperRefs>,
        _row: &Value,
        index: u64,
        stems: &[(String, &Path)],
        from: f64,
        beats: f64,
        signal: &Signal,
    ) -> Result<Value, String> {
        ears.run(reaper_stem_steps(stems, index, from, beats), signal).await
    }
}

/// REAPER's plan: a new track under the source, one MIDI item over the stretch, and the notes in it.
pub fn reaper_steps(name: &str, index: u64, notes: &[MidiNote], from: f64, beats: f64) -> Vec<Value> {
    let notes: Vec<Value> = notes
        .iter()
        .map(|note| json!({"pitch": note.pitch, "start": round(note.start), "length": round(note.length.max(1. / 64.)), "velocity": note.velocity.max(1)}))
        .collect();
    vec![
        json!({"tool": "track_add", "as": "midi", "input": {"name": name, "index": index}}),
        json!({"tool": "item_add", "as": "item", "input": {"ref": "@midi", "positionBeats": from, "lengthBeats": beats, "name": name}}),
        json!({"tool": "notes_add", "input": {"ref": "@item", "notes": notes}}),
    ]
}

/// REAPER's plan for stems: a new track each under the source, its file an item over the stretch.
pub fn reaper_stem_steps(stems: &[(String, &Path)], index: u64, from: f64, beats: f64) -> Vec<Value> {
    let mut steps = Vec::new();
    for (k, (name, path)) in stems.iter().enumerate() {
        steps.push(json!({"tool": "track_add", "as": format!("s{k}"), "input": {"name": name, "index": index + k as u64}}));
        steps.push(json!({"tool": "item_import", "input": {"ref": format!("@s{k}"), "path": path.to_string_lossy(), "positionBeats": from, "lengthBeats": beats, "name": name}}));
    }
    steps
}

fn round(beats: f64) -> f64 {
    (beats * 10_000.).round() / 10_000.
}
