//! A DAW's side of audio to MIDI: only its own facts. A track rendered over a stretch, the tempo there, and the notes written
//! on a new track beside it (one plan).
use super::MidiNote;
use crate::core::contracts::{JsonObject, Render};
use async_trait::async_trait;
use kumi_common::abort::Signal;
use serde_json::Value;

pub struct Words {
    pub app: &'static str,
    /// Where the notes go, in words ("a new MIDI track under it, one item over the stretch").
    pub written: &'static str,
    /// What `options` take (the DAW's own: Renoise's instrument), in words, and their schema.
    pub options: &'static str,
    pub options_schema: &'static str,
}

#[async_trait(?Send)]
pub trait ToMidiBackend {
    fn words(&self) -> &'static Words;

    /// The track rendered over the stretch (`from`, `beats` in quarter notes), and its name.
    async fn render(&self, track: &str, from: f64, beats: f64, signal: &Signal) -> Result<(Render, String), String>;

    /// The tempo at the stretch's start (BPM).
    async fn tempo(&self, from: f64, signal: &Signal) -> Result<f64, String>;

    /// A note's name as the DAW shows it.
    fn note_name(&self, pitch: u8) -> String;

    /// The notes written on a new track named `name` beside `track`, over the stretch (one plan): what was made. `drums`: they
    /// play out (no note ends).
    #[allow(clippy::too_many_arguments)]
    async fn write(
        &self,
        track: &str,
        name: &str,
        notes: &[MidiNote],
        drums: bool,
        from: f64,
        beats: f64,
        options: &JsonObject,
        signal: &Signal,
    ) -> Result<Value, String>;
}
