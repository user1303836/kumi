//! Kumi's own ears through its own bridges (#401): what audio to MIDI and stems share in the DAWs whose bridges are Kumi's own.
//! Both render the track offline and take the tempo Kumi follows; the stage audio to MIDI hears is the DAW's listening
//! table's (a track's own items before its FX where they render, else the track through its devices before its fader, which
//! a stem always is). What's one DAW's alone (its words, a note's name, how the notes and the stems go into it) is its
//! `EarsDaw`, which its crate gives with its own tools.
use crate::changes::OwnPlan;
use crate::listening::OwnRenderer;
use async_trait::async_trait;
use kumi_agent::core::plan::make_changes;
use kumi_agent::core::refs::RefScheme;
use kumi_agent::core::stems::backend::{StemsBackend, Words as StemWords};
use kumi_agent::core::to_midi::backend::{ToMidiBackend, Words};
use kumi_agent::core::to_midi::MidiNote;
use kumi_common::abort::Signal;
use kumi_listening::renderer::{Renderer, Window};
use kumi_types::contracts::{JsonObject, Render};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::rc::Rc;

/// What one DAW's ears do its own way, which its crate gives with its own tools: its words, a note's name, and how the notes
/// and the stems go into it.
#[async_trait(?Send)]
pub trait EarsDaw<S: RefScheme + 'static> {
    fn words(&self) -> &'static Words;
    fn stem_words(&self) -> &'static StemWords;
    /// A note's name as the DAW shows it.
    fn note_name(&self, pitch: u8) -> String;
    /// The notes written on a new track named `name` at `index`, after the track they were heard from (one plan): what
    /// was made. `drums`: they play out (no note ends).
    #[allow(clippy::too_many_arguments)]
    async fn write(
        &self,
        ears: &OwnEars<S>,
        name: &str,
        index: u64,
        notes: &[MidiNote],
        drums: bool,
        from: f64,
        beats: f64,
        options: &JsonObject,
        signal: &Signal,
    ) -> Result<Value, String>;
    /// The stems placed from `index`, after the track whose row is `row` (one plan): what was made.
    #[allow(clippy::too_many_arguments)]
    async fn place(
        &self,
        ears: &OwnEars<S>,
        row: &Value,
        index: u64,
        stems: &[(String, &Path)],
        from: f64,
        beats: f64,
        signal: &Signal,
    ) -> Result<Value, String>;
}

pub struct OwnEars<S: RefScheme + 'static> {
    plan: Rc<OwnPlan<S>>,
    renderer: Rc<OwnRenderer<S>>,
    daw: Rc<dyn EarsDaw<S>>,
}

impl<S: RefScheme + 'static> OwnEars<S> {
    pub fn new(plan: Rc<OwnPlan<S>>, renderer: Rc<OwnRenderer<S>>, daw: Rc<dyn EarsDaw<S>>) -> Self {
        OwnEars { plan, renderer, daw }
    }

    /// The shared change engine, which reads the project and takes the plan the notes and stems go in by.
    pub fn plan(&self) -> &Rc<OwnPlan<S>> {
        &self.plan
    }

    /// The track rendered alone over the stretch, and its name: `raw`, before the track's FX where its own items render (the
    /// DAW's listening table says; what audio to MIDI hears); else the track through its devices before its fader (what a
    /// stem is).
    async fn render_named(&self, track: &str, from: f64, beats: f64, raw: bool, signal: &Signal) -> Result<(Render, String), String> {
        let long = self.plan.lengthen(track)?;
        let name = self.row(&long, signal).await?.get("name").and_then(Value::as_str).unwrap_or("Track").to_string();
        let stage = if raw && self.renderer.table.no_items.is_none() { "items" } else { "pre-fader" };
        Ok((self.renderer.render_track(&long, stage, Window { from, beats }, signal).await?, name))
    }

    /// A row of the project, by its long ref.
    pub async fn row(&self, long: &str, signal: &Signal) -> Result<Value, String> {
        let answer = self.plan.read_through(json!({"ref": long}), None, signal).await?;
        answer.pointer("/items/0").cloned().ok_or_else(|| "that track isn't in the project any more: read again".to_string())
    }

    /// Steps run as one plan: what they made.
    pub async fn run(&self, steps: Vec<Value>, signal: &Signal) -> Result<Value, String> {
        let done = make_changes(&self.plan, json!({"steps": steps}).as_object().cloned().unwrap_or_default(), signal.clone())
            .await
            .map_err(|error| error.to_string())?;
        if done.is_error {
            return Err(done.text);
        }
        Ok(serde_json::from_str(&done.text).unwrap_or_else(|_| json!({"said": done.text})))
    }
}

#[async_trait(?Send)]
impl<S: RefScheme + 'static> ToMidiBackend for OwnEars<S> {
    fn words(&self) -> &'static Words {
        self.daw.words()
    }

    async fn render(&self, track: &str, from: f64, beats: f64, signal: &Signal) -> Result<(Render, String), String> {
        self.render_named(track, from, beats, true, signal).await
    }

    async fn tempo(&self, _from: f64, _signal: &Signal) -> Result<f64, String> {
        self.renderer
            .tempo()
            .filter(|bpm| *bpm > 0.)
            .ok_or_else(|| "Kumi doesn't know the tempo yet: read the project, then ask again".to_string())
    }

    fn note_name(&self, pitch: u8) -> String {
        self.daw.note_name(pitch)
    }

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
    ) -> Result<Value, String> {
        let long = self.plan.lengthen(track)?;
        let index = self.row(&long, signal).await?.get("index").and_then(Value::as_u64).unwrap_or(0) + 1;
        self.daw.write(self, name, index, notes, drums, from, beats, options, signal).await
    }
}

#[async_trait(?Send)]
impl<S: RefScheme + 'static> StemsBackend for OwnEars<S> {
    fn words(&self) -> &'static StemWords {
        self.daw.stem_words()
    }

    async fn render(&self, track: &str, from: f64, beats: f64, signal: &Signal) -> Result<(Render, String), String> {
        self.render_named(track, from, beats, false, signal).await
    }

    fn folder(&self) -> PathBuf {
        self.renderer.folder().to_path_buf()
    }

    async fn place(&self, track: &str, stems: &[(String, &Path)], from: f64, beats: f64, signal: &Signal) -> Result<Value, String> {
        let long = self.plan.lengthen(track)?;
        let row = self.row(&long, signal).await?;
        let index = row.get("index").and_then(Value::as_u64).unwrap_or(0) + 1;
        self.daw.place(self, &row, index, stems, from, beats, signal).await
    }
}
