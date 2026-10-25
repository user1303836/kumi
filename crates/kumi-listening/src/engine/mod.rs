//! The listening tools' engines, apart from any DAW: a judged run (the judge, and the tune that picks its numbers) over
//! what the DAW can do (`Renderer`). The DAW hears, changes and undoes; the engines decide.
mod audition;
mod call;
mod form;
mod groove;
mod judge;
mod probe;
mod roles;
mod sound;
mod tune;
pub use audition::{analysis_focus, heard_options, silent_render};
pub(crate) use judge::masking_items;
pub use judge::{JudgeRun, Named};

use super::{
    listener::Listener,
    renderer::{bars_of, Renderer, Window},
    round::Round,
};
use crate::{
    heard_settings::{SettingsHeard, SettingsMemory},
    listener::ListenerSource,
    references::store::ReferenceStore,
};
use indexmap::IndexMap;
use kumi_common::{abort::Signal, js::number::round};
use kumi_media::audio::{self, analyze::Analysis};
use kumi_types::{contracts::AuditionEvent, errors::RuntimeError};
use std::{
    cell::{Cell, RefCell},
    path::PathBuf,
    rc::Rc,
};

/// The tools' state across calls (a judged run), and the DAW they listen through.
pub struct Listening {
    pub(crate) host: Rc<dyn Renderer>,
    on_judge: Option<Rc<dyn Fn(Round)>>,
    /// What Kumi remembers of searches: each setting a search hears is handed to it as it's heard, and a search asks it
    /// what worked on its device before.
    pub(crate) memory: Option<Rc<dyn SettingsMemory>>,
    /// The judged run under way (or the last one).
    pub(crate) judge: RefCell<Option<JudgeRun>>,
    /// The groove run under way (or the last one).
    groove: RefCell<Option<groove::GrooveRun>>,
    on_audition: Option<Rc<dyn Fn(AuditionEvent)>>,
    /// How many auditions the answer has made, and the best closeness any has had.
    rounds: Cell<usize>,
    best: Cell<Option<f64>>,
    /// Whether an audition said how long its rounds take.
    told_length: Cell<bool>,
    /// The references auditions compared with lately, by what was asked for.
    reference_cache: RefCell<IndexMap<String, Analysis>>,
    listener_source: Option<ListenerSource>,
    /// Measured references, read by name for the judge.
    references: Option<Rc<ReferenceStore>>,
    /// Whether the run's listens are heard by the learned models too: the style model, the effects model.
    pub(crate) embedding_wanted: Cell<(bool, bool)>,
    /// The style model a judge run hears with, fixed when it starts: the embeddings slot's file then (None: Kumi's
    /// own) and its identity. A slot swapped during a run doesn't change it.
    style_model: RefCell<Option<(Option<PathBuf>, String)>>,
    /// The listening model as last looked up (found, a definite none, or why the lookup failed), and when.
    listener: RefCell<Option<(Result<Option<Rc<dyn Listener>>, String>, i64)>>,
}

impl Listening {
    pub fn new(
        host: Rc<dyn Renderer>,
        on_judge: Option<Rc<dyn Fn(Round)>>,
        on_audition: Option<Rc<dyn Fn(AuditionEvent)>>,
        listener_source: Option<ListenerSource>,
        references: Option<Rc<ReferenceStore>>,
        memory: Option<Rc<dyn SettingsMemory>>,
    ) -> Rc<Self> {
        Rc::new(Self {
            host,
            on_judge,
            memory,
            judge: RefCell::new(None),
            groove: RefCell::new(None),
            on_audition,
            rounds: Cell::new(0),
            best: Cell::new(None),
            told_length: Cell::new(false),
            reference_cache: RefCell::new(IndexMap::new()),
            listener_source,
            references,
            embedding_wanted: Cell::new((false, false)),
            style_model: RefCell::new(None),
            listener: RefCell::new(None),
        })
    }

    /// A new answer, not the match run's continuing one: what the producer asked for since isn't a judged run's change to
    /// take back, so its changes count from `ids` (the changes applied now). If that changed the sound, the run's
    /// numbers are out of date: its next judge hears its bars again first.
    pub fn new_answer(&self, ids: Vec<String>) {
        self.rounds.set(0);
        self.best.set(None);
        // A groove run's changes are the ones since its last round: only its checkpoint moves (its rounds re-read the
        // notes).
        if let Some(run) = self.groove.borrow_mut().as_mut() {
            run.carry_on(ids.clone());
        }
        let mut guard = self.judge.borrow_mut();
        if let Some(run) = guard.as_mut() {
            let audible = |id: &String| self.host.audible(id) == Some(true);
            // An undo in between counts too: a change the run's numbers took in that isn't in the project any more.
            let made = self.applied_since(&run.checkpoint).iter().any(|(id, _)| audible(id));
            let undone = run.checkpoint.iter().any(|id| !ids.contains(id) && audible(id));
            // Python run in the DAW, a command or its own undo may have changed it too, outside the history.
            let outside = !self.host.outside_since(run.outside).is_empty();
            if made || undone || outside {
                run.stale = true;
            }
            run.checkpoint = ids;
            run.outside = self.host.outside();
            // The steer was this answer's: the new answer's changes are its own again.
            run.steered = None;
        }
    }

    /// The producer's words came into the answer under way: the judged run's changes from here may be theirs, so they
    /// aren't a round's to take back. The first steer marks where (`applied`, the changes applied now); the run's next
    /// round takes back only what came before it, then starts again from what's in the project.
    pub fn steered(&self, applied: Vec<String>) {
        if let Some(run) = self.judge.borrow_mut().as_mut() {
            run.steered.get_or_insert_with(|| applied.clone());
        }
        if let Some(run) = self.groove.borrow_mut().as_mut() {
            run.steered(applied);
        }
    }

    /// What Kumi says when the project's tempo isn't known yet.
    pub(crate) fn no_tempo(&self) -> String {
        format!("Kumi doesn't know the {}'s tempo yet; try again.", self.host.profile().words.place)
    }

    /// How many auditions the answer has made.
    pub fn round_count(&self) -> usize {
        self.rounds.get()
    }

    /// Says what Kumi is doing.
    pub(crate) fn tell(&self, title: impl AsRef<str>, playing: Option<bool>) {
        self.host.tell(title.as_ref(), playing);
    }

    /// Whether anyone keeps the settings searches hear.
    pub(crate) fn remembers(&self) -> bool {
        self.memory.is_some()
    }

    /// Settings a search heard, handed out.
    pub(crate) fn heard(&self, event: SettingsHeard) {
        if let Some(memory) = &self.memory {
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| memory.heard(event)));
        }
    }

    /// The bars `window` covers, as said while the DAW plays them: "bars 17–24", or "bar 17".
    pub(crate) fn bars_of(&self, window: Window) -> String {
        bars_of(self.host.beats_per_bar(), window)
    }

    /// What Kumi does to hear a stretch, as said while it does: Live plays it; a DAW that renders to listen renders it.
    pub(crate) fn hearing(&self) -> &'static str {
        if self.host.profile().caps.renders_to_listen {
            "Rendering"
        } else {
            "Playing"
        }
    }

    /// Kumi's changes in the history now (applied ones), to tell a round's own from what came before.
    pub(crate) fn applied_ids(&self) -> Vec<String> {
        self.host.applied()
    }

    /// The applied changes since a checkpoint, oldest first: their ids and titles.
    pub(crate) fn applied_since(&self, checkpoint: &[String]) -> Vec<(String, String)> {
        self.host.applied_since(checkpoint)
    }

    /// The audio file a reference names: a clip's, else the name itself as a file. A clipRef that's stale, gone or a
    /// MIDI clip's says so, rather than reading as a file that isn't there.
    pub(crate) async fn reference_file(&self, named: &str, signal: Signal) -> Result<Result<String, String>, RuntimeError> {
        match self.host.clip_file(named, signal).await {
            Ok(file) => Ok(Ok(file.unwrap_or_else(|| audio::audio_path(named)))),
            Err(RuntimeError::Aborted) => Err(RuntimeError::Aborted),
            Err(error) => Ok(Err(error.to_string())),
        }
    }
}

/// The longest stretch one listen hears, in seconds (a judged run's span, a form's, a sound's and an audition's too).
pub(crate) const LONGEST_LISTEN: f64 = 3600.;

/// Why a stretch of `beats` is too long to hear at once, when it is, and what to do instead.
pub fn too_long(beats: f64, tempo: f64, instead: &str) -> Option<String> {
    let seconds = beats * 60. / tempo;
    (seconds > LONGEST_LISTEN || !seconds.is_finite())
        .then(|| format!("That's {} of music; Kumi listens to at most an hour at once, so {instead}.", clock(seconds)))
}

fn clock(seconds: f64) -> String {
    let whole = round(seconds) as i64;
    format!("{}:{:02}", whole / 60, whole % 60)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        checklist::MainState,
        judging::Placed,
        renderer::{Captured, ListenFlag, Pass},
    };
    use async_trait::async_trait;
    use kumi_types::{
        contracts::{AuditionCandidate, JsonObject, Render},
        daw::DawProfile,
    };
    use serde_json::{json, Value};

    /// A DAW with nothing in it, in an unknown DAW's words: only what every DAW must answer.
    struct Bare {
        tempo: Option<f64>,
        refuses: Option<&'static str>,
        flag: ListenFlag,
    }

    #[async_trait(?Send)]
    impl Renderer for Bare {
        fn profile(&self) -> &DawProfile {
            &DawProfile::UNKNOWN
        }
        fn refusal(&self) -> Option<String> {
            self.refuses.map(str::to_owned)
        }
        fn tempo(&self) -> Option<f64> {
            self.tempo
        }
        fn beats_per_bar(&self) -> f64 {
            4.
        }
        fn lifetime(&self) -> Signal {
            Signal::new()
        }
        fn cleanup(&self) -> Signal {
            Signal::new()
        }
        fn tell(&self, _title: &str, _playing: Option<bool>) {}
        fn flag(&self) -> &ListenFlag {
            &self.flag
        }
        fn kept_folder(&self) -> PathBuf {
            std::env::temp_dir().join("kumi-bare-host")
        }
        async fn hear_pass(&self, _candidates: &[AuditionCandidate], _window: Window, _before_main: bool, _signal: Signal) -> Pass {
            Pass { rendered: Err(RuntimeError::plain("Nothing to hear.")), notes: vec![], tried: None, lead: 0., audible: false }
        }
        async fn master_chain(&self, _signal: Signal) -> MainState {
            MainState::default()
        }
        fn applied(&self) -> Vec<String> {
            vec![]
        }
        fn applied_since(&self, _mark: &[String]) -> Vec<(String, String)> {
            vec![]
        }
        async fn undo(&self, _id: &str) -> Result<(), String> {
            Ok(())
        }
        async fn turn_gain(&self, _gain: f64, _track: Option<&str>, _signal: Signal) -> Result<(String, bool), String> {
            Err("No gain stage.".into())
        }
        async fn limited(&self, _track: Option<&str>, _signal: Signal) -> bool {
            false
        }
        async fn chain(&self, _track: Option<&str>) -> Result<Vec<Placed>, String> {
            Ok(vec![])
        }
        async fn delete_device(&self, _reference: &str) -> bool {
            false
        }
    }

    fn listening(tempo: Option<f64>, refuses: Option<&'static str>) -> Rc<Listening> {
        Listening::new(Rc::new(Bare { tempo, refuses, flag: ListenFlag::default() }), None, None, None, None, None)
    }

    /// Each tool, with an input that reaches the engine.
    fn calls() -> Vec<(&'static str, JsonObject)> {
        let object = |value: Value| value.as_object().cloned().unwrap();
        vec![
            ("judge", object(json!({"goal": {"loudness": -14}}))),
            ("tune", object(json!({"device": "d", "how": "fit"}))),
            ("sound", object(json!({"tracks": ["a"]}))),
            ("form", object(json!({}))),
            ("groove", object(json!({"clip": "c", "reference": "r"}))),
            ("audition", object(json!({"candidates": [{"track": "t"}], "from_beat": 0}))),
        ]
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_dawss_refusal_is_what_every_listening_tool_says_when_it_cant_listen() {
        let listening = listening(Some(120.), Some("Connect the DAW first."));
        for (name, input) in calls() {
            let said = listening.call(name, input, Signal::new()).await.expect("a listening tool").unwrap();
            assert!(said.is_error, "{name}");
            assert_eq!(said.text, "Connect the DAW first.", "{name}");
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_project_without_a_tempo_is_said_in_the_dawss_word_for_it() {
        let listening = listening(None, None);
        for (name, input) in calls().into_iter().filter(|(name, _)| *name != "groove" && *name != "tune") {
            let said = listening.call(name, input, Signal::new()).await.expect("a listening tool").unwrap();
            assert_eq!(said.text, "Kumi doesn't know the project's tempo yet; try again.", "{name}");
        }
    }

    /// A DAW whose listens record (Live's record pass, without Ears): each take starts `lead` seconds before the part,
    /// or at the song's start when the part starts sooner; a lead of 0 is a tap or an offline render, which starts at the
    /// part.
    struct Recording {
        lead: f64,
        dir: tempfile::TempDir,
        flag: ListenFlag,
    }

    #[async_trait(?Send)]
    impl Renderer for Recording {
        fn profile(&self) -> &DawProfile {
            &DawProfile::UNKNOWN
        }
        fn refusal(&self) -> Option<String> {
            None
        }
        fn tempo(&self) -> Option<f64> {
            Some(120.)
        }
        fn beats_per_bar(&self) -> f64 {
            4.
        }
        fn lifetime(&self) -> Signal {
            Signal::new()
        }
        fn cleanup(&self) -> Signal {
            Signal::new()
        }
        fn tell(&self, _title: &str, _playing: Option<bool>) {}
        fn flag(&self) -> &ListenFlag {
            &self.flag
        }
        fn kept_folder(&self) -> PathBuf {
            self.dir.path().join("kept")
        }
        async fn hear_pass(&self, candidates: &[AuditionCandidate], window: Window, _before_main: bool, _signal: Signal) -> Pass {
            let part = window.beats * 0.5;
            let start = if window.from * 0.5 >= self.lead { self.lead } else { 0. };
            let mut files = IndexMap::new();
            for candidate in candidates {
                let file = self.dir.path().join(format!("{}.wav", candidate.track));
                noise(&file, start + part);
                files.insert(candidate.track.clone(), Render { file: file.to_string_lossy().into_owned(), start, seconds: Some(part) });
            }
            let sources = candidates.iter().map(|candidate| (candidate.track.clone(), false)).collect();
            Pass {
                rendered: Ok(Captured { files, sources, before_master: None }),
                notes: vec![],
                tried: None,
                lead: self.lead,
                audible: true,
            }
        }
        async fn master_chain(&self, _signal: Signal) -> MainState {
            MainState::default()
        }
        fn applied(&self) -> Vec<String> {
            vec![]
        }
        fn applied_since(&self, _mark: &[String]) -> Vec<(String, String)> {
            vec![]
        }
        async fn undo(&self, _id: &str) -> Result<(), String> {
            Ok(())
        }
        async fn turn_gain(&self, _gain: f64, _track: Option<&str>, _signal: Signal) -> Result<(String, bool), String> {
            Err("No gain stage.".into())
        }
        async fn limited(&self, _track: Option<&str>, _signal: Signal) -> bool {
            false
        }
        async fn chain(&self, _track: Option<&str>) -> Result<Vec<Placed>, String> {
            Ok(vec![])
        }
        async fn delete_device(&self, _reference: &str) -> bool {
            false
        }
    }

    /// Noise for `seconds`, as a 16-bit stereo WAV at 48 kHz.
    fn noise(path: &std::path::Path, seconds: f64) {
        let frames = (seconds * 48_000.) as u32;
        let mut data = Vec::with_capacity(frames as usize * 4);
        let mut seed: u32 = 0x2468_ace1;
        for _ in 0..frames {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            let sample = (((seed >> 16) as f64 / 32_768. - 1.) * 0.3 * 32_767.) as i16;
            data.extend_from_slice(&sample.to_le_bytes());
            data.extend_from_slice(&sample.to_le_bytes());
        }
        let mut file = b"RIFF".to_vec();
        file.extend_from_slice(&(36 + data.len() as u32).to_le_bytes());
        file.extend_from_slice(b"WAVEfmt ");
        for (value, bytes) in [(16u32, 4), (1, 2), (2, 2), (48_000, 4), (48_000 * 4, 4), (4, 2), (16, 2)] {
            file.extend_from_slice(&value.to_le_bytes()[..bytes]);
        }
        file.extend_from_slice(b"data");
        file.extend_from_slice(&(data.len() as u32).to_le_bytes());
        file.extend_from_slice(&data);
        std::fs::write(path, file).unwrap();
    }

    /// Carried from M1: a record pass's take (no Ears) keeps its whole lead inside the song, a lead cut short to what the
    /// song holds before the part at its start, and a tapped or rendered take (lead 0) none.
    #[tokio::test(flavor = "current_thread")]
    async fn a_recorded_takes_lead_is_whole_inside_the_song_cut_short_at_its_start_and_none_for_a_tap() {
        let leads = |lead: f64, from: f64| async move {
            let host = Rc::new(Recording { lead, dir: tempfile::tempdir().unwrap(), flag: ListenFlag::default() });
            let listening = Listening::new(host, None, None, None, None, None);
            // (a beat of noise after its lead: the lead is what's checked, and a short take hears quickly)
            let heard = listening.hear_takes(&["Bass".to_string()], Window { from, beats: 2. }, "measuring Bass", Signal::new()).await;
            heard.unwrap().unwrap()["Bass"].1
        };
        assert_eq!(leads(0.5, 16.).await, 0.5, "inside the song: the whole lead");
        assert_eq!(leads(0.5, 0.).await, 0., "the part starts the song: nothing before it");
        assert_eq!(leads(0.5, 0.5).await, 0.25, "a quarter second of song before the part");
        assert_eq!(leads(0., 16.).await, 0., "a tap or an offline render starts at the part");
    }

    /// A judged listen measures its takes together, each as it measures alone, and keeps each by a link where the volume
    /// allows one: the kept file is the capture itself under the judge's name, not a copy written again (a whole song's
    /// is 138 MB). A name kept already is kept again whole, the capture with it.
    #[tokio::test(flavor = "current_thread")]
    async fn a_judged_listen_measures_its_takes_together_and_keeps_them_linked() {
        let host = Rc::new(Recording { lead: 0., dir: tempfile::tempdir().unwrap(), flag: ListenFlag::default() });
        let listening = Listening::new(host.clone(), None, None, None, None, None);
        let window = Window { from: 16., beats: 4. };
        let heard = listening.judge_hear(Some("Bass"), Some("Kick"), window, "as it is", Signal::new()).await.unwrap().unwrap();
        let capture = |name: &str| host.dir.path().join(format!("{name}.wav"));
        let alone = |name: &str| {
            let file = capture(name).to_string_lossy().into_owned();
            async move {
                let options = crate::measure::MeasureOptions { start: Some(0.), seconds: Some(2.), signal: None };
                crate::measure::measure_file(&file, options).await.unwrap()
            }
        };
        let (bass, kick) = (alone("Bass").await, alone("Kick").await);
        let focus = heard.focus.as_ref().expect("the focus heard");
        for (together, alone) in [(&heard.main, &bass), (focus, &kick)] {
            assert_eq!(together.measures.integrated, alone.measures.integrated);
            assert_eq!(together.measures.true_peak, alone.measures.true_peak);
            assert_eq!(together.frames.level, alone.frames.level);
        }
        let kept = host.kept_folder();
        assert_eq!(PathBuf::from(&heard.file), kept.join("Bass.wav"));
        assert_eq!(heard.focus_file.as_ref().map(|(file, _)| file.clone()), Some(kept.join("Kick.wav")));
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            let inode = |file: &std::path::Path| std::fs::metadata(file).unwrap().ino();
            assert_eq!(inode(&kept.join("Bass.wav")), inode(&capture("Bass")), "the capture itself, linked");
            assert_eq!(inode(&kept.join("Kick.wav")), inode(&capture("Kick")));
        }
        let bytes = std::fs::read(capture("Bass")).unwrap();
        assert_eq!(listening.keep_file(&capture("Bass")).await, kept.join("Bass.wav"));
        assert_eq!(std::fs::read(kept.join("Bass.wav")).unwrap(), bytes, "kept again, whole");
        assert_eq!(std::fs::read(capture("Bass")).unwrap(), bytes, "and the capture with it");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn what_a_daw_doesnt_do_is_said_plainly_never_in_lives_words() {
        let listening = listening(Some(120.), None);
        let input = json!({"track": "t", "from_beat": 0, "beats": 4}).as_object().cloned().unwrap();
        let said = listening.call("render", input, Signal::new()).await.expect("a listening tool").unwrap();
        assert_eq!((said.is_error, said.text.as_str()), (true, "Kumi can't render a track's audio offline in this DAW."));
        assert!(listening.call("make_changes", JsonObject::new(), Signal::new()).await.is_none());
    }
}
