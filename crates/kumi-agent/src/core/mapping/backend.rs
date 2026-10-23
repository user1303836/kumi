//! A DAW's side of sample mapping: only its own facts. The keys its sampler maps, an instrument's samples (their names, zones
//! and audio), the change that makes an instrument of planned files or lays out one the song has, and what its songs'
//! instruments are for the tidy. Note names come from the DAW's profile. Core does the rest, the same in every DAW.
use super::Planned;
use crate::core::daw::DawProfile;
use async_trait::async_trait;
use kumi_common::abort::Signal;
use serde_json::Value;

/// The DAW's words and keys.
pub struct Words {
    pub app: &'static str,
    /// The DAW's profile: how it names notes (and so how the model writes them).
    pub profile: &'static DawProfile,
    /// What an instrument is called.
    pub instrument: &'static str,
    /// The lowest and highest MIDI notes the sampler maps, and where a drum kit starts.
    pub lowest: i32,
    pub highest: i32,
    pub first_key: i32,
}

/// A sample of an instrument the DAW has: its ref, name, its audio as a file (or why there's none), where it plays now (keys
/// and velocities, MIDI), how far it's transposed, and whether it plays as a key lets go (the note-off layer).
#[derive(Debug, Clone)]
pub struct Held {
    pub reference: String,
    pub name: String,
    pub file: Result<String, String>,
    pub zone: (i32, i32),
    pub velocity: (u8, u8),
    pub transpose: i32,
    pub note_off: bool,
}

#[async_trait(?Send)]
pub trait MappingBackend {
    fn words(&self) -> &'static Words;

    /// An instrument's samples, with their audio (refused before any is saved when there are more than `most`): its name, and
    /// the samples.
    async fn samples_of(&self, instrument: &str, most: usize, signal: &Signal) -> Result<(String, Vec<Held>), String>;

    /// Makes one planned instrument of sound files (one change): what the DAW answered.
    async fn make(&self, planned: &Planned, name: &str, drums: bool, signal: &Signal) -> Result<Value, String>;

    /// Lays out an instrument's own samples as planned (one change): what the DAW answered.
    async fn remap(&self, instrument: &str, planned: &Planned, signal: &Signal) -> Result<Value, String>;

    /// The song's instruments for the tidy: each one's ref, name, how many notes play it, a print of its settings and of each
    /// sample's audio, and how far the count got (`counted`, or `countedTo` and `patterns` when the song was too big).
    async fn instruments(&self, signal: &Signal) -> Result<Value, String>;
}
