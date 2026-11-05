//! What every DAW's goal rig does alike with a goal's own types (`core::goal_rig`), over a DAW of the test's.
use async_trait::async_trait;
use indexmap::IndexMap;
use kumi_agent::core::{
    contracts::{Generation, GenerationOptions, GenerationTrial},
    evolve::Knob,
    goal_rig::{
        check_slots, device_label, given_values, knob_key, moved, moved_or_went, near, tell_generation, tidy, GoalBackend, GoalCore,
        GoalDaw, GoalPass, GoalScreen, HearOrder, SetOrder,
    },
};
use kumi_common::abort::Signal;
use kumi_listening::goal::Told;
use kumi_media::audio::analyze::Analysis;
use kumi_types::{
    contracts::{Focus, Render, StructuralMove},
    errors::RuntimeError,
};
use std::{
    cell::{Cell, RefCell},
    collections::HashSet,
};

/// A project holding `tracks` (None: it can't be read), noting the slots as they're tidied.
struct Project {
    tracks: Option<Vec<String>>,
    tidied: RefCell<Vec<String>>,
}

impl Project {
    fn new(tracks: Option<&[&str]>) -> Self {
        Self { tracks: tracks.map(names), tidied: RefCell::default() }
    }
}

fn names(names: &[&str]) -> Vec<String> {
    names.iter().map(|name| name.to_string()).collect()
}

#[async_trait(?Send)]
impl GoalDaw for Project {
    async fn track_names(&self) -> Option<Vec<String>> {
        self.tracks.clone()
    }
    async fn track_of(&self, slot: &str) -> Option<String> {
        Some(slot.into())
    }
    async fn mute(&self, track: &str) {
        self.tidied.borrow_mut().push(track.into());
    }
    fn made_by_kumi(&self, _: &str) -> Option<String> {
        None
    }
    async fn take_back(&self, _: &str) -> bool {
        false
    }
}

#[tokio::test(flavor = "current_thread")]
async fn a_goals_candidates_are_tidied_last_in_the_project_first_and_one_it_doesnt_name_after_them() {
    let project = Project::new(Some(&["Keys", "Bass", "Pad", "Lead"]));
    tidy(&project, names(&["Gone", "Bass", "Lead", "Keys"]), &[]).await;
    assert_eq!(*project.tidied.borrow(), ["Lead", "Bass", "Keys", "Gone"]);
}

#[tokio::test(flavor = "current_thread")]
async fn when_the_project_cant_be_read_the_last_candidate_to_join_is_tidied_first() {
    let project = Project::new(None);
    tidy(&project, names(&["Bass", "Lead", "Keys"]), &[]).await;
    assert_eq!(*project.tidied.borrow(), ["Keys", "Lead", "Bass"]);
}

#[test]
fn a_trial_for_a_slot_the_goal_doesnt_hold_is_refused_in_plain_words() {
    let trial = |slot: &str| GenerationTrial { slot: slot.into(), knobs: vec![], values: vec![], fresh: None };
    let holds = |slot: &str| slot == "Bass";
    assert!(check_slots(&[trial("Bass")], holds).is_ok());
    assert_eq!(check_slots(&[trial("Bass"), trial("Nobody")], holds).unwrap_err().to_string(), "“Nobody” isn't in the search");
}

fn knob(device: &str, name: &str) -> Knob {
    Knob {
        r#ref: format!("{device}/{name}"),
        device: device.into(),
        name: name.into(),
        value: 0.5,
        min: 0.,
        max: 1.,
        step: None,
        device_id: None,
    }
}

#[test]
fn a_knob_and_its_device_are_named_by_place_and_name() {
    assert_eq!(device_label(1, "EQ Eight"), "1:EQ Eight");
    assert_eq!(knob_key(&knob("1:EQ Eight", "Gain")), "1:EQ Eight|Gain");
}

#[test]
fn a_trials_knobs_move_when_they_differ_from_what_was_last_set() {
    let (tone, drive, level) = (knob("0:Synth", "Tone"), knob("0:Synth", "Drive"), knob("0:Synth", "Level"));
    let trial = GenerationTrial {
        slot: "Bass".into(),
        knobs: vec![tone.clone(), drive.clone(), level.clone()],
        values: vec![0.5, 0.9, 0.3],
        fresh: None,
    };
    let last: IndexMap<String, f64> = [(knob_key(&tone), 0.5 + 1e-7), (knob_key(&drive), 0.2)].into_iter().collect();
    let moved_names = |found: Vec<(Knob, f64)>| found.into_iter().map(|(knob, value)| (knob.name, value)).collect::<Vec<_>>();
    assert_eq!(
        moved_names(moved(&trial, Some(&last))),
        [("Drive".to_string(), 0.9), ("Level".to_string(), 0.3)],
        "as it was stays; no value set yet moves"
    );
    assert_eq!(moved(&trial, None).len(), 3, "nothing set yet: all of them");
}

#[test]
fn a_value_not_given_or_not_a_number_is_never_sent() {
    let (tone, drive, level) = (knob("0:Synth", "Tone"), knob("0:Synth", "Drive"), knob("0:Synth", "Level"));
    let knobs = vec![tone, drive, level];
    let given = given_values(&knobs, &[f64::NAN, 0.5]);
    assert_eq!(given.iter().map(|(knob, _)| knob.name.as_str()).collect::<Vec<_>>(), ["Drive"]);
    let trial = GenerationTrial { slot: "Bass".into(), knobs, values: vec![f64::NAN, 0.5], fresh: None };
    assert_eq!(moved(&trial, None).len(), 1);
}

#[test]
fn a_generation_is_told_what_was_heard_and_counts_what_it_had_heard_before() {
    let told = |structural| Told { score: 70., gaps: vec!["too dark".into()], structural };
    let mut generation = Generation::default();
    tell_generation(&mut generation, "Bass", told(None), false);
    assert_eq!(generation.cached, 0);
    assert_eq!(generation.scores["Bass"], 70.);
    assert_eq!(generation.gaps["Bass"], ["too dark"]);
    assert!(generation.structural.is_empty());
    let structural = StructuralMove { gap: "length".into(), r#move: "change the family".into() };
    tell_generation(&mut generation, "Lead", told(Some(structural.clone())), true);
    assert_eq!(generation.cached, 1, "it had been heard");
    assert_eq!(generation.structural["Lead"], structural);
}

/// A project that can't be read: `made` were added by the model, those in `kept` can't be taken back, those in `gone` aren't
/// there any more. Notes what's done.
struct Tidying {
    made: Vec<&'static str>,
    kept: Vec<&'static str>,
    gone: Vec<&'static str>,
    done: RefCell<Vec<String>>,
}

#[async_trait(?Send)]
impl GoalDaw for Tidying {
    async fn track_names(&self) -> Option<Vec<String>> {
        None
    }
    async fn track_of(&self, slot: &str) -> Option<String> {
        (!self.gone.iter().any(|gone| *gone == slot)).then(|| slot.to_string())
    }
    async fn mute(&self, track: &str) {
        self.done.borrow_mut().push(format!("muted {track}"));
    }
    fn made_by_kumi(&self, slot: &str) -> Option<String> {
        self.made.iter().any(|made| *made == slot).then(|| slot.to_string())
    }
    async fn take_back(&self, id: &str) -> bool {
        self.done.borrow_mut().push(format!("took back {id}"));
        !self.kept.iter().any(|kept| *kept == id)
    }
}

#[tokio::test(flavor = "current_thread")]
async fn the_top_candidates_are_muted_those_the_model_made_taken_back_and_the_producers_stay_muted_and_said() {
    let daw = Tidying { made: vec!["Made"], kept: vec![], gone: vec![], done: RefCell::default() };
    // (the last to join goes first, as the project can't be read here)
    let said = tidy(&daw, names(&["Top", "Theirs", "Made"]), &names(&["Top"])).await;
    assert_eq!(*daw.done.borrow(), ["took back Made", "muted Theirs", "muted Top"]);
    assert_eq!(said, ["“Theirs” stays, muted."]);
}

#[tokio::test(flavor = "current_thread")]
async fn a_top_candidate_the_model_made_stays_for_the_ab() {
    let daw = Tidying { made: vec!["Top"], kept: vec![], gone: vec![], done: RefCell::default() };
    let said = tidy(&daw, names(&["Top"]), &names(&["Top"])).await;
    assert_eq!(*daw.done.borrow(), ["muted Top"]);
    assert!(said.is_empty());
}

#[tokio::test(flavor = "current_thread")]
async fn a_candidate_that_cant_be_taken_back_stays_muted_and_said() {
    let daw = Tidying { made: vec!["Made"], kept: vec!["Made"], gone: vec![], done: RefCell::default() };
    let said = tidy(&daw, names(&["Made"]), &[]).await;
    assert_eq!(*daw.done.borrow(), ["took back Made", "muted Made"]);
    assert_eq!(said, ["“Made” stays, muted."]);
}

#[tokio::test(flavor = "current_thread")]
async fn a_candidate_that_went_is_left_out() {
    let daw = Tidying { made: vec!["Gone"], kept: vec![], gone: vec!["Gone"], done: RefCell::default() };
    let said = tidy(&daw, names(&["Gone"]), &[]).await;
    assert!(daw.done.borrow().is_empty());
    assert!(said.is_empty());
}

#[test]
fn what_moved_or_went_names_each_device_once_first_seen_first() {
    let on = |device: &str, name: &str| Knob { device: device.into(), name: name.into(), ..knob_for_words() };
    // (a slot's knobs come shaping knobs first, so they interleave by device)
    let knobs = [on("0:Synth", "Tone"), on("1:EQ", "Gain"), on("0:Synth", "Drive"), on("1:EQ", "Q")];
    let said = moved_or_went("Pad", &knobs);
    assert!(said.starts_with("“Synth”, “EQ” moved or went on “Pad” since the search began"), "{said}");
}

fn knob_for_words() -> Knob {
    Knob { r#ref: "1".into(), device: String::new(), name: String::new(), value: 0., min: 0., max: 1., step: None, device_id: None }
}

#[test]
fn one_value_of_a_knob_is_what_a_daw_reads_back_of_it() {
    let wide = Knob { min: 20., max: 20000., ..knob_for_words() };
    assert!(near(&wide, 1234.5678, 1234.568), "a float's rounding, on a wide knob");
    assert!(!near(&wide, 1234., 1300.));
    let unit = knob_for_words();
    assert!(near(&unit, 0.7, 0.699_999_988), "a float's rounding");
    assert!(!near(&unit, 0.7, 0.71));
    let stepped = Knob { max: 10., step: Some(1.), ..knob_for_words() };
    assert!(near(&stepped, 2.4, 2.), "a stepped knob lands on its step");
    assert!(!near(&stepped, 2.6, 2.));
}

#[derive(Clone)]
struct Kept {
    told: Told,
    render: Render,
}
struct Eager;
struct Batch;
macro_rules! backend {
    ($backend:ty, $setting:ident, $hearing:ident) => {
        impl GoalBackend for $backend {
            type Cached = Kept;
            const SET_ORDER: SetOrder = SetOrder::$setting;
            const HEAR_ORDER: HearOrder = HearOrder::$hearing;
            fn cached_told(kept: Kept) -> Told {
                kept.told
            }
            fn cache(told: Told, render: &Render) -> Kept {
                Kept { told, render: render.clone() }
            }
        }
    };
}
backend!(Eager, ReadAllThenSet, AsCompleted);
backend!(Batch, OneAtATime, AfterBatch);

/// Explicit measured-shaped data: the scripted pass overrides hearing/scoring, so tests need no audio file or decoder.
fn analysis(file: &str) -> Analysis {
    serde_json::from_value(serde_json::json!({
        "kumiAudio":1,"file":file,"format":"WAV","sampleRate":48000.0,"channels":1,"seconds":1.0,
        "analyzed":{"from":"0","to":"1","focus":"mix"},
        "loudness":{"integratedLufs":-14.0,"shortTermMaxLufs":-14.0,"rangeLu":0.0,"truePeakDbtp":-1.0,"samplePeakDbfs":-1.0,"clippedSamples":0},
        "balance":{"bands":[],"tiltDbPerOctave":0.0,"centroidHz":1000.0},"stereo":null,
        "dynamics":{"crestDb":10.0,"peakToLoudnessDb":10.0,"onsetsPerSecond":1.0},"tempo":null,"key":null,
        "overTime":{"every":"1 s","lufs":[-14.0]},"spectrogram":{"rows":[],"columns":"","scale":""}
    })).unwrap()
}

#[derive(Default)]
struct Script {
    events: RefCell<Vec<String>>,
    rendering: Cell<bool>,
    refused: IndexMap<String, HashSet<String>>,
    missing: HashSet<String>,
    gone: Vec<String>,
    gone_later: Vec<String>,
    absent: HashSet<String>,
    silent: HashSet<String>,
    fail: HashSet<String>,
    wait_a: RefCell<Option<tokio::sync::oneshot::Receiver<()>>>,
    release_a: RefCell<Option<tokio::sync::oneshot::Sender<()>>>,
}
impl Script {
    fn note(&self, event: impl Into<String>) {
        self.events.borrow_mut().push(event.into());
    }
    fn pass<'a, B: GoalBackend>(&'a self, core: &'a GoalCore<B>) -> Pass<'a, B> {
        Pass { script: self, core, held: false }
    }
    fn writes(&self) -> Vec<String> {
        self.events.borrow().iter().filter(|event| event.starts_with("plan:") || event.starts_with("set:")).cloned().collect()
    }
}
struct Pass<'a, B: GoalBackend> {
    script: &'a Script,
    core: &'a GoalCore<B>,
    held: bool,
}
impl<B: GoalBackend> Drop for Pass<'_, B> {
    fn drop(&mut self) {
        if self.held {
            self.script.rendering.set(false);
        }
        self.script.note("drop");
    }
}
#[async_trait(?Send)]
impl<B: GoalBackend> GoalPass<B> for Pass<'_, B> {
    type Plan = ();
    fn signal(&self, given: Signal) -> Signal {
        given
    }
    fn begin(&mut self) -> Result<(), RuntimeError> {
        assert!(!self.script.rendering.replace(true));
        self.held = true;
        self.script.note("begin");
        Ok(())
    }
    async fn before_set(&mut self, _: Signal) -> Result<Vec<String>, RuntimeError> {
        self.script.note("before_set");
        for slot in &self.script.gone {
            self.core.current.borrow_mut().shift_remove(slot);
        }
        Ok(self.script.gone.clone())
    }
    async fn before_render(&mut self, _: Signal) -> Result<Vec<String>, RuntimeError> {
        self.script.note("before_render");
        for slot in &self.script.gone_later {
            self.core.current.borrow_mut().shift_remove(slot);
        }
        Ok(self.script.gone_later.clone())
    }
    async fn plan(&mut self, slot: &str, _: &[(Knob, f64)], _: Signal) -> Result<Option<()>, RuntimeError> {
        self.script.note(format!("plan:{slot}"));
        Ok((!self.script.missing.contains(slot)).then_some(()))
    }
    async fn set(&mut self, slot: &str, _: &[(Knob, f64)], _: (), _: Signal) -> Result<HashSet<String>, RuntimeError> {
        self.script.note(format!("set:{slot}"));
        Ok(self.script.refused.get(slot).cloned().unwrap_or_default())
    }
    async fn render(
        &mut self,
        fresh: &[&GenerationTrial],
        screen: Option<&GoalScreen>,
        _: Signal,
    ) -> Result<IndexMap<String, Render>, RuntimeError> {
        let names = fresh.iter().map(|trial| trial.slot.as_str()).collect::<Vec<_>>().join(",");
        self.script.note(format!("render:{names}:{}", if screen.is_some() { "screen" } else { "full" }));
        Ok(fresh
            .iter()
            .filter(|trial| !self.script.absent.contains(&trial.slot))
            .map(|trial| (trial.slot.clone(), Render { file: trial.slot.clone(), start: 0., seconds: None }))
            .collect())
    }
    fn rendered(&mut self) {
        if B::HEAR_ORDER == HearOrder::AsCompleted {
            self.held = false;
            self.script.rendering.set(false);
            self.script.note("release");
        }
    }
    fn hearing_window(&mut self, screen: Option<&GoalScreen>) -> (f64, f64) {
        self.script.note("window");
        (screen.map_or(8., |screen| screen.beats), 120.)
    }
    async fn hear(&self, render: &Render, _: f64, _: f64, _: Option<Focus>, _: Signal) -> Result<Analysis, RuntimeError> {
        assert_eq!(self.script.rendering.get(), B::HEAR_ORDER == HearOrder::AfterBatch, "render guard during hearing");
        self.script.note(format!("hear:{}", render.file));
        if self.script.fail.contains(&render.file) {
            return Err(RuntimeError::plain("hearing failed"));
        }
        let wait = if render.file == "A" { self.script.wait_a.borrow_mut().take() } else { None };
        if let Some(wait) = wait {
            wait.await.map_err(|_| RuntimeError::plain("the other hearing went"))?;
        }
        if render.file == "B" {
            if let Some(release) = self.script.release_a.borrow_mut().take() {
                let _ = release.send(());
            }
        }
        Ok(analysis(&render.file))
    }
    fn score(&self, heard: &Analysis, against: &Analysis, _: Option<Focus>) -> Option<Told> {
        self.script.note(format!("score:{}:{}", heard.file, against.file));
        (!self.script.silent.contains(&heard.file)).then(|| Told {
            score: 70.,
            gaps: vec![],
            structural: (heard.file == "B").then(|| StructuralMove { gap: "shape".into(), r#move: "change family".into() }),
        })
    }
}

fn opened_core<B: GoalBackend>(slots: &[&str]) -> GoalCore<B> {
    let core = GoalCore::new(analysis("reference"), None);
    for slot in slots {
        core.current.borrow_mut().insert((*slot).into(), [(knob_key(&knob("0:Synth", "Tone")), 0.5)].into_iter().collect());
    }
    core
}
fn trial(slot: &str, value: f64) -> GenerationTrial {
    GenerationTrial { slot: slot.into(), knobs: vec![knob("0:Synth", "Tone")], values: vec![value], fresh: None }
}

async fn state_trace<B: GoalBackend<Cached = Kept>>() {
    let core = opened_core::<B>(&["A", "B", "Missing", "Gone", "Late", "Absent", "Silent"]);
    let mut script = Script::default();
    script.refused.insert("A".into(), [knob_key(&knob("0:Synth", "Tone"))].into_iter().collect());
    script.missing.insert("Missing".into());
    script.gone.push("Gone".into());
    script.gone_later.push("Late".into());
    script.absent.extend(names(&["Missing", "Absent"]));
    script.silent.insert("Silent".into());
    let trials: Vec<_> = ["A", "B", "Missing", "Gone", "Late", "Absent", "Silent"].map(|slot| trial(slot, 0.8)).into();
    let found = core.generation(script.pass(&core), &trials, Signal::new(), None).await.unwrap();
    assert_eq!(found.gone, ["Gone", "Late"]);
    assert!(found.frozen.contains_key("A") && found.frozen.contains_key("Missing"));
    assert_eq!(found.silent, ["Missing", "Absent", "Silent"]);
    assert_eq!(found.scores.len(), 2, "only A and B are scored");
    assert_eq!(found.structural["B"].gap, "shape");
    assert_eq!(core.current.borrow()["A"][&knob_key(&knob("0:Synth", "Tone"))], 0.5);
    assert_eq!(core.current.borrow()["B"][&knob_key(&knob("0:Synth", "Tone"))], 0.8);
    assert!(!core.current.borrow().contains_key("Gone") && !core.current.borrow().contains_key("Late"));
    let writes = script.writes();
    if B::SET_ORDER == SetOrder::ReadAllThenSet {
        assert_eq!(
            writes,
            names(&[
                "plan:A",
                "plan:B",
                "plan:Missing",
                "plan:Late",
                "plan:Absent",
                "plan:Silent",
                "set:A",
                "set:B",
                "set:Late",
                "set:Absent",
                "set:Silent"
            ])
        );
    } else {
        assert_eq!(
            writes,
            names(&[
                "plan:A",
                "set:A",
                "plan:B",
                "set:B",
                "plan:Missing",
                "plan:Late",
                "set:Late",
                "plan:Absent",
                "set:Absent",
                "plan:Silent",
                "set:Silent"
            ])
        );
    }
    assert!(!script.rendering.get());
    let unchanged = Script::default();
    assert_eq!(core.generation(unchanged.pass(&core), &[trial("B", 0.8)], Signal::new(), None).await.unwrap().cached, 1);
    assert!(unchanged.writes().is_empty(), "an accepted unchanged value needs no write");
    assert!(unchanged.events.borrow().iter().any(|event| event == "before_set"));
    assert!(
        unchanged.events.borrow().iter().any(|event| event == "before_render"),
        "cached unchanged trials still check availability twice"
    );
    // Duplicate-slot trials distinguish preplanned values from a sequential second read of accepted state.
    let core = opened_core::<B>(&["B"]);
    let script = Script::default();
    core.generation(script.pass(&core), &[trial("B", 0.8), trial("B", 0.8)], Signal::new(), None).await.unwrap();
    let sets = script.writes().iter().filter(|event| *event == "set:B").count();
    assert_eq!(sets, if B::SET_ORDER == SetOrder::ReadAllThenSet { 2 } else { 1 });
}

#[tokio::test(flavor = "current_thread")]
async fn the_shared_engine_preserves_read_write_order_and_accepts_only_unrefused_values() {
    state_trace::<Eager>().await;
    state_trace::<Batch>().await;
}

async fn cached_trace<B: GoalBackend<Cached = Kept>>() {
    let mut core = opened_core::<B>(&["A"]);
    core.screen = Some(GoalScreen { offset: 2., beats: 4., reference: analysis("short-reference") });
    let mut script = Script::default();
    script.refused.insert("A".into(), [knob_key(&knob("0:Synth", "Tone"))].into_iter().collect());
    core.generation(script.pass(&core), &[trial("A", 0.9)], Signal::new(), None).await.unwrap();
    script.events.borrow_mut().clear();
    let cached = core.generation(script.pass(&core), &[trial("A", 0.90000001)], Signal::new(), None).await.unwrap();
    assert_eq!(cached.cached, 1, "the original requested values key the cache, despite refused/current values");
    assert_eq!(script.writes(), ["plan:A", "set:A"]);
    assert!(script.events.borrow().iter().any(|event| event == "before_render"));
    assert!(!script.events.borrow().iter().any(|event| event.starts_with("render:") || event.starts_with("hear:")));
    let fresh = GenerationTrial { fresh: Some(true), ..trial("A", 0.9) };
    assert_eq!(core.generation(script.pass(&core), &[fresh], Signal::new(), None).await.unwrap().cached, 0);
    let screened = core
        .generation(script.pass(&core), &[trial("A", 0.9)], Signal::new(), Some(GenerationOptions { screen: Some(true) }))
        .await
        .unwrap();
    assert!(screened.screened && screened.cached == 0);
    assert!(script.events.borrow().iter().any(|event| event == "score:A:short-reference"));
    assert_eq!(core.generation(script.pass(&core), &[trial("A", 0.9)], Signal::new(), None).await.unwrap().cached, 1);
    script.events.borrow_mut().clear();
    core.generation(script.pass(&core), &[], Signal::new(), None).await.unwrap();
    assert_eq!(script.events.borrow().iter().filter(|event| *event == "before_set").count(), 1);
    assert_eq!(script.events.borrow().iter().filter(|event| *event == "before_render").count(), 1);
    assert!(!script.events.borrow().iter().any(|event| event.starts_with("render:")));
}

#[tokio::test(flavor = "current_thread")]
async fn cached_trials_are_still_set_and_screening_has_its_own_reference_and_cache() {
    cached_trace::<Eager>().await;
    cached_trace::<Batch>().await;
}

async fn error_trace<B: GoalBackend<Cached = Kept>>() {
    let core = opened_core::<B>(&["A", "B"]);
    let mut script = Script::default();
    script.fail.insert("A".into());
    assert!(core.generation(script.pass(&core), &[trial("A", 0.8), trial("B", 0.8)], Signal::new(), None).await.is_err());
    assert_eq!(core.current.borrow()["A"][&knob_key(&knob("0:Synth", "Tone"))], 0.8);
    assert_eq!(
        core.current.borrow()["B"][&knob_key(&knob("0:Synth", "Tone"))],
        0.8,
        "hearing failure does not forget native values already accepted"
    );
    let b = kumi_listening::goal::trial_key("B", false, &[0.8]);
    if B::HEAR_ORDER == HearOrder::AsCompleted {
        assert!(script.events.borrow().iter().any(|event| event == "hear:B"), "every ready hearing is dispatched");
        assert!(core.heard.borrow().get(&b).is_some(), "a completed result commits before another hearing's error");
    } else {
        assert!(!script.events.borrow().iter().any(|event| event.starts_with("score:")));
        assert!(core.heard.borrow().get(&b).is_none(), "no result commits until the whole batch succeeds");
    }
    assert!(!script.rendering.get(), "errors release the native listening guard");
}

#[tokio::test(flavor = "current_thread")]
async fn a_hearing_error_preserves_each_backends_dispatch_and_cache_commit_boundary() {
    error_trace::<Eager>().await;
    error_trace::<Batch>().await;
}

async fn completion_trace<B: GoalBackend<Cached = Kept>>() {
    let core = opened_core::<B>(&["A", "B"]);
    let script = Script::default();
    let (release, wait) = tokio::sync::oneshot::channel();
    *script.wait_a.borrow_mut() = Some(wait);
    *script.release_a.borrow_mut() = Some(release);
    core.generation(script.pass(&core), &[trial("A", 0.8), trial("B", 0.8)], Signal::new(), None).await.unwrap();
    let scores: Vec<_> = script.events.borrow().iter().filter(|event| event.starts_with("score:")).cloned().collect();
    let eager = B::HEAR_ORDER == HearOrder::AsCompleted;
    assert_eq!(
        scores,
        if eager { names(&["score:B:reference", "score:A:reference"]) } else { names(&["score:A:reference", "score:B:reference"]) }
    );
    let a = kumi_listening::goal::trial_key("A", false, &[0.8]);
    let b = kumi_listening::goal::trial_key("B", false, &[0.8]);
    // Looking up a result must not promote it: the bounded cache remains FIFO, not LRU.
    let first = if eager { &b } else { &a };
    assert!(core.heard.borrow().get(first).is_some());
    for at in 0..1999 {
        core.heard.borrow_mut().insert(
            format!("other:{at}"),
            Kept {
                told: Told { score: 0., gaps: vec![], structural: None },
                render: Render { file: "other".into(), start: 0., seconds: None },
            },
        );
    }
    assert_eq!(core.heard.borrow().get(&a).is_some(), eager);
    assert_eq!(core.heard.borrow().get(&b).is_some(), !eager);
    let kept = core.heard.borrow().get(if eager { &a } else { &b }).unwrap();
    assert_eq!(kept.render.file, if eager { "A" } else { "B" }, "backend cache payload keeps its matching render");
}

#[tokio::test(flavor = "current_thread")]
async fn out_of_order_hearing_preserves_completion_or_input_order_without_changing_fifo_eviction() {
    completion_trace::<Eager>().await;
    completion_trace::<Batch>().await;
}
