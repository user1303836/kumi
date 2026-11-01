//! What every DAW's goal rig does alike with a goal's own types (`core::goal_rig`), over a DAW of the test's.
use async_trait::async_trait;
use kumi_agent::core::{
    contracts::GenerationTrial,
    goal_rig::{check_slots, tidy, GoalDaw},
};
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
