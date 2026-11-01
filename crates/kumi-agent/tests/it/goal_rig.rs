//! What every DAW's goal rig does alike with a goal's own types (`core::goal_rig`), over a DAW of the test's.
use async_trait::async_trait;
use indexmap::IndexMap;
use kumi_agent::core::{
    contracts::{Generation, GenerationTrial},
    evolve::Knob,
    goal_rig::{check_slots, device_label, knob_key, moved, tell_generation, tidy, GoalDaw},
};
use kumi_listening::goal::Told;
use kumi_types::contracts::StructuralMove;
use std::cell::RefCell;

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
    Knob { r#ref: format!("{device}/{name}"), device: device.into(), name: name.into(), value: 0.5, min: 0., max: 1., step: None }
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
        moved_names(moved(&trial, Some(&last), true)),
        [("Drive".to_string(), 0.9), ("Level".to_string(), 0.3)],
        "as it was stays; no value set yet moves"
    );
    assert_eq!(moved(&trial, None, true).len(), 3, "nothing set yet: all of them");
}

#[test]
fn a_value_a_trial_doesnt_give_isnt_a_number() {
    let (tone, drive) = (knob("0:Synth", "Tone"), knob("0:Synth", "Drive"));
    let trial = GenerationTrial { slot: "Bass".into(), knobs: vec![tone, drive], values: vec![0.5], fresh: None };
    assert_eq!(moved(&trial, None, true).len(), 1, "with finite_only, it never moves");
    let sent = moved(&trial, None, false);
    assert_eq!(sent.len(), 2);
    assert!(sent[1].1.is_nan());
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
