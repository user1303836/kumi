//! A DAW's side of chopping: only its own facts. What a recording to cut is (its audio, its cuts so far), the song's tempo, how
//! finely a note can start (its grid), the change that cuts it, the keys its cuts play on, the change that lays them out (and
//! the lines a take of some of them is), and a render of what was laid out. Core works in beats; the backend in its own grid.
use async_trait::async_trait;
use kumi_common::abort::Signal;
use serde_json::Value;

pub struct Words {
    pub app: &'static str,
    /// What the tool cuts ("a sample: an instrument's first sample…").
    pub source: &'static str,
    /// Where the cuts are laid out ("a pattern track: …").
    pub place: &'static str,
    /// What a cut is called ("slice").
    pub cut: &'static str,
    /// Its schema for `at`.
    pub at_schema: &'static str,
}

/// A recording to cut: its ref, name, its audio as a file (read at `rate`, `frames` long), and its cuts now (frames from 0).
#[derive(Debug, Clone)]
pub struct Recording {
    pub reference: String,
    pub name: String,
    pub file: String,
    pub rate: f64,
    pub frames: usize,
    pub cuts: Vec<usize>,
}

/// The song's tempo, its bar, how finely a note can start on its grid (beats; None when on any frame), and whether it plays.
#[derive(Debug, Clone, Copy)]
pub struct Song {
    pub bpm: f64,
    pub beats_per_bar: f64,
    pub grid: Option<f64>,
    pub playing: bool,
}

/// A cut's key: where it starts (frame from 0) and the note that plays it (the DAW's notation).
#[derive(Debug, Clone)]
pub struct Key {
    pub frame: usize,
    pub note: String,
}

/// One cut laid out: its key, and where it starts in beats from the range's start.
#[derive(Debug, Clone, PartialEq)]
pub struct Step {
    pub note: String,
    pub beat: f64,
}

/// What was laid out: the change, the range as the variations tool takes it as a source, where it is in the DAW's words, and the
/// backend's own facts about it (its grid and place).
#[derive(Debug, Clone)]
pub struct Laid {
    pub answer: Value,
    pub range: Value,
    pub words: String,
    pub data: Value,
}

#[async_trait(?Send)]
pub trait ChopBackend {
    fn words(&self) -> &'static Words;

    async fn recording(&self, reference: &str, signal: &Signal) -> Result<Recording, String>;

    async fn song(&self, signal: &Signal) -> Result<Song, String>;

    /// Cuts the recording at `frames` (from 0) in one change, its other cuts gone when `replace`: what the DAW answered.
    async fn cut(&self, recording: &Recording, frames: &[usize], replace: bool, signal: &Signal) -> Result<Value, String>;

    /// The cuts' keys, in order.
    async fn keys(&self, recording: &Recording, signal: &Signal) -> Result<Vec<Key>, String>;

    /// Lays the steps out at `at`, over `beats` (the recording's length at its tempo): refused when what's there holds the
    /// producer's notes, unless `replace`.
    async fn lay(
        &self,
        recording: &Recording,
        at: &Value,
        steps: &[Step],
        beats: f64,
        replace: bool,
        signal: &Signal,
    ) -> Result<Laid, String>;

    /// A take of some of the steps, as the variations tool takes its lines over `laid`'s range.
    fn take_lines(&self, laid: &Laid, steps: &[Step]) -> Value;

    /// Renders what was laid out, `seconds` of it from its start: the file, and where in it the start plays (seconds).
    async fn render(&self, laid: &Laid, seconds: f64, signal: &Signal) -> Result<(String, f64), String>;

    /// Why the DAW can't render for Kumi now, or None.
    fn cant_render(&self) -> Option<String>;
}
