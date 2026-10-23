//! What the listening tools ask of a DAW: its facts (tempo, meter, tracks), hearing the project, Kumi's changes in its
//! history and the level a judged run keeps. The tools themselves (a judged run, a tune, a groove, a form, a sound) are
//! the same whichever DAW is there; a DAW answers these.
use crate::{
    core::{
        contracts::{AuditionCandidate, ChangeRecord, JsonObject, Render},
        daw::DawProfile,
        errors::RuntimeError,
    },
    listening::{
        checklist::MainState,
        fit::Band,
        judging::Placed,
        knobs::Scale,
        notes::{Kit, Note},
    },
    plugins::roles::Seen,
};
use async_trait::async_trait;
use indexmap::IndexMap;
use kumi_common::abort::Signal;
use std::{any::Any, cell::Cell, path::PathBuf};

/// A stretch of the timeline, in beats.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Window {
    pub from: f64,
    pub beats: f64,
}

/// The bars `window` covers, as said while the DAW plays them: "bars 17–24", or "bar 17".
pub fn bars_of(beats_per_bar: f64, window: Window) -> String {
    let meter = beats_per_bar.max(1.);
    let bar = |beat: f64| (beat / meter).floor() as i64 + 1;
    let (first, last) = (bar(window.from), bar(window.from + window.beats - 1e-6));
    if first >= last {
        format!("bar {first}")
    } else {
        format!("bars {first}–{last}")
    }
}

/// Whether Kumi is playing the project to listen. Main stays as the producer has it, so they hear what Kumi hears (and
/// steer from it).
#[derive(Default)]
pub struct ListenFlag {
    on: Cell<bool>,
}

impl ListenFlag {
    /// Kumi starts playing the project to listen (see `ListenNow`).
    pub fn begin(&self) -> ListenNow<'_> {
        let began = !self.on.replace(true);
        ListenNow { flag: self, began }
    }
    /// Whether Kumi is playing the project to listen now.
    pub fn is_on(&self) -> bool {
        self.on.get()
    }
}

/// Kumi playing the project to listen. However the listen ends, a panic included, dropping it ends it. One taken while
/// another listen runs changes nothing.
pub struct ListenNow<'a> {
    flag: &'a ListenFlag,
    began: bool,
}

impl Drop for ListenNow<'_> {
    fn drop(&mut self) {
        if self.began {
            self.flag.on.set(false);
        }
    }
}

/// What a listen captured: each source's capture by its name, the sources in order with whether each is the mix, and the
/// mix as it comes into the master chain when the DAW heard it there too.
pub struct Captured {
    pub files: IndexMap<String, Render>,
    pub sources: Vec<(String, bool)>,
    pub before_master: Option<Render>,
}

/// One listen through the DAW: what it captured, what the DAW noted, and why the mix wasn't heard as it comes into the
/// master chain when it was tried.
pub struct Pass {
    /// What the listen captured; the error when it failed (the notes are still the DAW's).
    pub rendered: Result<Captured, RuntimeError>,
    /// What went wrong around the listen, or was done to make it work (a source the DAW hears on its own).
    pub notes: Vec<String>,
    /// Why the mix wasn't heard as it comes into the master chain, when the DAW tried (and that it came through empty).
    pub tried: Option<String>,
    /// The most a capture runs before the part asked for, in seconds: a pass that records keeps a short lead for a hit
    /// to rise out of; one that taps the signal has none.
    pub lead: f64,
    /// Whether the producer heard the listen where it plays (the project played out loud) or it was captured offline
    /// or silently. The tools ask for listens the producer hears; a DAW that can't says so here.
    pub audible: bool,
}

/// A device's knob: where it is, its range, the values it lists (a switch or a menu), and its scale.
#[derive(Debug, Clone)]
pub struct DeviceKnob {
    pub reference: String,
    pub name: String,
    pub raw: f64,
    pub min: f64,
    pub max: f64,
    pub items: Vec<String>,
    pub scale: Option<Scale>,
}

impl DeviceKnob {
    /// The raw value of one of the values it lists.
    pub fn item(&self, wanted: &str) -> Option<f64> {
        let index = self.items.iter().position(|item| item.eq_ignore_ascii_case(wanted))?;
        let steps = (self.items.len().max(2) - 1) as f64;
        Some(self.min + index as f64 * (self.max - self.min) / steps)
    }
}

/// What the DAW sets on an equaliser to draw fitted bands: knobs (by reference) to raw values, in the order to set them,
/// and what else it set to make them land as calculated, in words.
pub struct EqFit {
    pub values: Vec<(String, f64)>,
    pub settings: Vec<&'static str>,
}

/// Which of an audition's candidates the producer hears where they play: Kumi hears the rest on its own (silently, when
/// the DAW can).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Audible {
    /// Every one.
    Every,
    /// The first (the one the model leans toward).
    First,
}

/// What an audition pass is asked for besides the candidates and where the part is.
#[derive(Debug, Clone, Copy)]
pub struct AuditionOptions {
    pub audible: Audible,
    /// This round of the answer's auditions, to say.
    pub round: usize,
    /// The project's tempo when the audition began.
    pub tempo: f64,
    /// Whether to say how long a round takes: the first audition of the answer says it once.
    pub first: bool,
}

/// One candidate of an audition pass: the track asked for, the name it goes by now, the Session clip scene played (when a
/// clip was asked for), and whether the producer heard it where it plays or Kumi heard it on its own.
#[derive(Debug, Clone, PartialEq)]
pub struct AuditionSource {
    pub track: String,
    pub name: String,
    pub scene: Option<usize>,
    pub audible: bool,
}

/// What an audition pass captured: its candidates in order, each one's capture by its name, and how long the part is.
pub struct AuditionRender {
    pub sources: Vec<AuditionSource>,
    pub files: IndexMap<String, Render>,
    pub beats: f64,
}

/// One audition pass through the DAW: what it captured or why it didn't, what the DAW noted, and whether it got as far
/// as saying how long a round takes.
pub struct AuditionPass {
    pub rendered: Result<AuditionRender, RuntimeError>,
    pub notes: Vec<String>,
    pub opened: bool,
}

/// A clip's notes as the DAW plays them: each with its id (-1 when the DAW gave none), and where its time starts.
pub struct ClipNotes {
    pub name: String,
    pub notes: Vec<(i64, Note)>,
    /// The song beat of the clip's own time zero, for a clip on the timeline.
    pub song_zero: Option<f64>,
    /// Where what the DAW plays of the clip ends, in its own time (its loop's end, say).
    pub to: f64,
    /// Where the clip's notes have to end, in its own time: the DAW refuses a note past it.
    pub end: f64,
}

/// A note's change: where it starts now, how hard, and a shorter length when it would end past the clip.
#[derive(Debug, Clone, PartialEq)]
pub struct NotePatch {
    pub id: i64,
    pub start: f64,
    pub velocity: f64,
    pub duration: Option<f64>,
}

/// Scratch copies of a device's track, made for one search or probe and taken away after it.
pub struct Copies {
    /// The copies' track names, in order.
    pub names: Vec<String>,
    /// What the DAW keeps to set and remove them.
    pub held: Box<dyn Any>,
}

/// The DAW as the listening tools meet it.
#[async_trait(?Send)]
pub trait Renderer {
    /// The DAW's words.
    fn profile(&self) -> &DawProfile;
    /// Why nothing can be heard now (the DAW isn't connected), when it can't.
    fn refusal(&self) -> Option<String>;
    /// The project's tempo, once known.
    fn tempo(&self) -> Option<f64>;
    /// Beats in a bar.
    fn beats_per_bar(&self) -> f64;
    /// Ends when the DAW goes away.
    fn lifetime(&self) -> Signal;
    /// Ends a clean-up after a stopped answer, in its own time.
    fn cleanup(&self) -> Signal;
    /// Says what Kumi is doing, as the app shows it; `playing` when the DAW plays or stops for it.
    fn tell(&self, title: &str, playing: Option<bool>);
    /// Whether Kumi is playing the project to listen.
    fn flag(&self) -> &ListenFlag;
    /// Where captures worth keeping go, out of the listening folder's pruning.
    fn kept_folder(&self) -> PathBuf;

    /// Where the song ends, in beats.
    async fn song_end(&self, _signal: Signal) -> Option<f64> {
        None
    }
    /// A track's name, from its reference or its name (None: the name given stands).
    async fn track_name(&self, _named: &str, _signal: Signal) -> Option<String> {
        None
    }
    /// A track's long reference, as the history keeps references (positions): from its reference or its name.
    async fn track_ref(&self, _named: &str, _signal: Signal) -> Option<String> {
        None
    }
    /// The audio file a clip or a file name stands for; None when it names a file itself.
    async fn clip_file(&self, _named: &str, _signal: Signal) -> Result<Option<String>, RuntimeError> {
        Ok(None)
    }

    /// Hears `window` of these sources in one pass (the mix is `MIX_CANDIDATE`): with `before_main`, the mix as it comes
    /// into the master chain too, when the DAW can.
    async fn hear_pass(&self, candidates: &[AuditionCandidate], window: Window, before_main: bool, signal: Signal) -> Pass;
    /// The master chain as masking needs it, read as it is now: what moves the mix but not an element heard before it,
    /// and the level its gain devices add. A chain the DAW can't read counts as one masking can't be read through.
    async fn master_chain(&self, signal: Signal) -> MainState;
    /// A track's fader, dB (−inf reads as −120; None when it can't be read).
    async fn fader_of(&self, _track: &str, _signal: Signal) -> Option<f64> {
        None
    }

    /// Kumi's changes in the history now (applied ones), oldest first.
    fn applied(&self) -> Vec<String>;
    /// Those not in `mark`: ids and titles.
    fn applied_since(&self, mark: &[String]) -> Vec<(String, String)>;
    /// Whether a change can change what's heard; None when the history has no such change.
    fn audible(&self, _id: &str) -> Option<bool> {
        None
    }
    /// How much has changed the project outside the history (a count, to compare later).
    fn outside(&self) -> usize {
        0
    }
    /// What changed it outside the history since `mark`, in words.
    fn outside_since(&self, _mark: usize) -> Vec<&'static str> {
        vec![]
    }
    /// Kumi's undo of one change; why not, when the DAW wouldn't.
    async fn undo(&self, id: &str) -> Result<(), String>;
    /// The DAW's identities for the devices these changes made, as the history recorded them.
    fn made(&self, _changes: &[(String, String)]) -> Vec<String> {
        vec![]
    }
    /// How many devices these changes made (loaded or duplicated).
    fn devices_made(&self, _changes: &[(String, String)]) -> usize {
        0
    }
    /// Kumi's applied track renames not among `before`, oldest first: the renamed track's reference, the name it had
    /// and its new name.
    fn renames(&self, _before: &[String]) -> Vec<(String, String, String)> {
        vec![]
    }

    /// Turns the level by `gain` dB where rebalancing goes on the run's track (the master without one); says what it
    /// turned, and whether a limiter holds the peaks after it.
    async fn turn_gain(&self, gain: f64, track: Option<&str>, signal: Signal) -> Result<(String, bool), String>;
    /// Whether a limiter after the gain rebalancing turns holds the peaks.
    async fn limited(&self, track: Option<&str>, signal: Signal) -> bool;
    /// The devices on the run's track (the master without one), in order.
    async fn chain(&self, track: Option<&str>) -> Result<Vec<Placed>, String>;
    /// Deletes a device; whether it went.
    async fn delete_device(&self, reference: &str) -> bool;

    /// A device's knobs with the text the DAW shows across each one's range; why not, when it can't be read.
    async fn knobs(&self, _device: &str, _signal: Signal) -> Result<Result<Vec<DeviceKnob>, String>, RuntimeError> {
        Ok(Err("Kumi can't read this DAW's device knobs.".into()))
    }
    /// Sets knobs (by reference) to raw values in one step.
    async fn set_knobs(&self, _device: &str, _values: &[(&str, f64)], _signal: Signal) -> Result<(), RuntimeError> {
        Err(RuntimeError::plain("Kumi can't set this DAW's device knobs."))
    }
    /// What kind of device it is, as its probes are saved: the DAW's class for its own devices, the name for a plug-in.
    async fn device_kind(&self, _device: &str, _signal: Signal) -> Option<String> {
        None
    }
    /// The knobs to set so an equaliser's bands are these (the ones it isn't using take them); why not, when it can't.
    async fn fit_eq(
        &self,
        _device: &str,
        _knobs: &[DeviceKnob],
        _bands: &[Band],
        _signal: Signal,
    ) -> Result<Result<EqFit, String>, RuntimeError> {
        Ok(Err("Kumi can't set this DAW's equaliser from a fit.".into()))
    }
    /// Whether the device is on the track (its reference, as the history keeps them).
    async fn on_track(&self, _device: &str, _track: &str, _signal: Signal) -> bool {
        false
    }
    /// Makes `count` copies of a device's track, for work heard in `passes` passes over `window`.
    async fn open_copies(
        &self,
        _device: &str,
        _count: usize,
        _label: &str,
        _passes: u32,
        _window: Window,
        _signal: Signal,
    ) -> Result<Result<Copies, String>, RuntimeError> {
        Ok(Err("Kumi can't make scratch copies of a track in this DAW.".into()))
    }
    /// Sets each copy's knobs (by copy name, knob name and raw value).
    async fn set_copies(
        &self,
        _copies: &Copies,
        _values: serde_json::Map<String, serde_json::Value>,
        _signal: Signal,
    ) -> Result<Result<(), String>, RuntimeError> {
        Ok(Err("Kumi can't make scratch copies of a track in this DAW.".into()))
    }
    /// Takes the copies away, whatever happened.
    async fn close_copies(&self, _copies: Copies) {}

    /// The devices on a device's track, as tune sees them, and where the device is among them.
    async fn track_devices(&self, _device: &str, _signal: Signal) -> Result<(Vec<Seen>, usize), RuntimeError> {
        Err(RuntimeError::plain("Kumi can't read a track's devices in this DAW."))
    }
    /// Every parameter name the DAW lists for a plug-in; None when it can't list them.
    async fn listed_names(&self, _device: &str, _signal: Signal) -> Result<Option<Vec<String>>, RuntimeError> {
        Ok(None)
    }
    /// A device's turnable parameters, the switches that are on, an equaliser's bands, and for a mapped plug-in what
    /// the DAW lists; left empty when it can't say.
    async fn read_seen(&self, _device: &mut Seen, _signal: Signal) {}
    /// An equaliser's bands from its knobs: each band's number, frequency (Hz), whether it's on, and whether its gain
    /// shapes. None for any other device.
    fn eq_bands(&self, _knobs: &[DeviceKnob]) -> Vec<(u8, f64, bool, bool)> {
        vec![]
    }
    /// The devices on the run's track (the master without one) as tune sees them, before their parameters are read.
    async fn devices_seen(&self, _track: Option<&str>, _signal: Signal) -> Option<Vec<Seen>> {
        None
    }

    /// A clip's notes the DAW plays, each with its id; why not, when it can't be read or holds none.
    async fn clip_notes(&self, _clip: &str, _signal: Signal) -> Result<ClipNotes, String> {
        Err("Kumi can't read a clip's notes in this DAW.".into())
    }
    /// How a clip's notes fall into lanes: by its track's drum rack, when it has one.
    async fn lanes_of(&self, _clip: &str, _signal: Signal) -> Kit {
        Kit::pitched()
    }
    /// The clip read again, so this answer may change it.
    async fn reread_clip(&self, _clip: &str, _signal: Signal) -> Result<(), RuntimeError> {
        Ok(())
    }
    /// Moves notes in one change, so the DAW takes all of it or none and the history takes it back as one.
    async fn change_notes(&self, _clip: &str, _patches: &[NotePatch], _signal: Signal) -> Result<(), RuntimeError> {
        Err(RuntimeError::plain("Kumi can't move a clip's notes in this DAW."))
    }
    /// The clip's long reference, as the history keeps it.
    fn clip_id(&self, _clip: &str) -> Option<String> {
        None
    }
    /// The changes applied since a checkpoint, split into the note edits on a clip (by its long reference) and the rest.
    fn note_changes(&self, checkpoint: &[String], _clip: Option<&str>) -> (Vec<(String, String)>, Vec<(String, String)>) {
        (vec![], self.applied_since(checkpoint))
    }
    /// The track a drum stem's clip is on, by name; why not, when it isn't one on the timeline.
    async fn stem_track(&self, _audio: &str, _signal: Signal) -> Result<String, String> {
        Err("Kumi can't find a drum stem's track in this DAW.".into())
    }

    /// Why nothing can be rendered now: the DAW isn't connected, or can't play and record for it.
    fn render_refusal(&self) -> Option<String> {
        self.refusal()
    }
    /// Plays the candidates over the part and captures each: the producer hears them as `options` says.
    async fn audition_pass(
        &self,
        _candidates: &[AuditionCandidate],
        _from_beat: Option<f64>,
        _beats: Option<f64>,
        _options: AuditionOptions,
        _signal: Signal,
    ) -> AuditionPass {
        AuditionPass { rendered: Err(RuntimeError::plain("Kumi can't audition in this DAW.")), notes: vec![], opened: false }
    }
    /// The track's own audio between two points, rendered to a file at once (offline and silent), before its devices:
    /// the reply's file, seconds, channels and sample rate; why not, when it can't be.
    async fn render(&self, _track: &str, _from: f64, _beats: f64, _signal: Signal) -> Result<Result<JsonObject, String>, RuntimeError> {
        Ok(Err("Kumi can't render a track's audio offline in this DAW.".into()))
    }
    /// A reference as the model sees it this turn (a short name for a long one).
    fn short_ref(&self, long: &str) -> String {
        long.into()
    }
    /// Puts a line in the history (an audition heard).
    fn emit(&self, _record: ChangeRecord) {}
    /// The DAW's clock, in milliseconds since the epoch.
    fn now_ms(&self) -> i64 {
        chrono::Utc::now().timestamp_millis()
    }

    /// Each track's clips on the timeline, by track name (the first 64 tracks): where each that plays starts and ends, in
    /// beats. A muted clip doesn't play; a looped clip ends where it stops playing, not where its loop does.
    async fn clips_by_track(&self, _signal: Signal) -> Vec<(String, Vec<(f64, f64)>)> {
        vec![]
    }
}
