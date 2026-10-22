//! A DAW's side of sample mapping: only its own facts. Its note names and the keys its sampler maps, an instrument's samples
//! (their names and audio), the change that makes an instrument of planned files or lays out one the song has, and what its
//! songs' instruments are for the tidy. Core does the rest, the same in every DAW.
use super::Planned;
use async_trait::async_trait;
use kumi_common::abort::Signal;
use serde_json::Value;

/// The DAW's words and keys.
pub struct Words {
    pub app: &'static str,
    /// What an instrument is called, and a sample.
    pub instrument: &'static str,
    /// How notes are written, for the schema ("C-4, or 48: the DAW's own numbers").
    pub notes: &'static str,
    /// The lowest and highest MIDI notes the sampler maps, and where a drum kit starts.
    pub lowest: i32,
    pub highest: i32,
    pub first_key: i32,
}

/// A sample of an instrument the DAW has: its ref, name, and its audio as a file (or why there's none).
#[derive(Debug, Clone)]
pub struct Held {
    pub reference: String,
    pub name: String,
    pub file: Result<String, String>,
}

#[async_trait(?Send)]
pub trait MappingBackend {
    fn words(&self) -> &'static Words;

    /// A MIDI note in the DAW's own notation.
    fn note_name(&self, midi: i32) -> String;

    /// A note the model wrote in the DAW's notation, as MIDI.
    fn note_of(&self, value: &Value) -> Option<i32>;

    /// An instrument's samples, with their audio: its name, and the samples.
    async fn samples_of(&self, instrument: &str, signal: &Signal) -> Result<(String, Vec<Held>), String>;

    /// Makes one planned instrument of sound files (one change): what the DAW answered.
    async fn make(&self, planned: &Planned, name: &str, drums: bool, signal: &Signal) -> Result<Value, String>;

    /// Lays out an instrument's own samples as planned (one change): what the DAW answered.
    async fn remap(&self, instrument: &str, planned: &Planned, signal: &Signal) -> Result<Value, String>;

    /// The song's instruments for the tidy: each one's ref, name, how many notes play it, and its samples' prints.
    async fn instruments(&self, signal: &Signal) -> Result<Value, String>;
}
