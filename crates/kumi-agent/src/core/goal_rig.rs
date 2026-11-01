//! What every DAW's goal rig does alike with a goal's own types (#399, K29): how a knob and its device are named, which of
//! a trial's knobs moved, what a generation is told of a heard trial, and how a finished goal's candidates are tidied. The
//! listening half (screening, keying and hearing a trial) is `kumi_listening::goal`; each DAW's rig sets, renders and
//! keeps the best in its own way.
use crate::core::{
    contracts::{Generation, GenerationTrial},
    evolve::Knob,
};
use async_trait::async_trait;
use indexmap::IndexMap;
use kumi_listening::goal::Told;
use kumi_types::errors::RuntimeError;

/// A knob as a search knows it: its device (as [`device_label`] names it) and its own name.
pub fn knob_key(knob: &Knob) -> String {
    format!("{}|{}", knob.device, knob.name)
}

/// A device as a knob names it: its place in the chain and its name.
pub fn device_label(at: usize, name: &str) -> String {
    format!("{at}:{name}")
}

/// What's said of a slot the goal doesn't hold: a caller's bug, never the producer's.
pub fn not_in_the_search(slot: &str) -> String {
    format!("“{slot}” isn't in the search")
}

/// A generation's trials are all for slots the goal holds (`holds`): a trial for another is refused in plain words, before
/// anything is set.
pub fn check_slots(trials: &[GenerationTrial], holds: impl Fn(&str) -> bool) -> Result<(), RuntimeError> {
    match trials.iter().find(|trial| !holds(&trial.slot)) {
        Some(trial) => Err(RuntimeError::plain(not_in_the_search(&trial.slot))),
        None => Ok(()),
    }
}

/// A trial's knobs at its values (NaN where it gives none), those that moved from `last` (the values last set on its
/// slot, by key): one with no value set yet always moves. With `finite_only`, a value that isn't a number never does.
pub fn moved(trial: &GenerationTrial, last: Option<&IndexMap<String, f64>>, finite_only: bool) -> Vec<(Knob, f64)> {
    trial
        .knobs
        .iter()
        .enumerate()
        .map(|(at, knob)| (knob.clone(), trial.values.get(at).copied().unwrap_or(f64::NAN)))
        .filter(|(knob, value)| {
            (!finite_only || value.is_finite())
                && last.and_then(|last| last.get(&knob_key(knob))).is_none_or(|last| (last - value).abs() > 1e-6)
        })
        .collect()
}

/// What the generation is told of a slot's trial, heard now or from what was heard before (`cached`).
pub fn tell_generation(generation: &mut Generation, slot: &str, told: Told, cached: bool) {
    if cached {
        generation.cached += 1;
    }
    generation.scores.insert(slot.into(), told.score);
    generation.gaps.insert(slot.into(), told.gaps);
    if let Some(structural) = told.structural {
        generation.structural.insert(slot.into(), structural);
    }
}

/// What tidying a finished goal asks of a DAW's rig, slot by slot, by the slot's name.
#[async_trait(?Send)]
pub trait GoalDaw {
    /// The project's tracks by name, first to last, as tidying begins; None when they can't be read.
    async fn track_names(&self) -> Option<Vec<String>>;
    /// The slot's track as it is now (read again for each slot); None when it can't be found.
    async fn track_of(&self, slot: &str) -> Option<String>;
    /// Mutes the track, whatever comes of it.
    async fn mute(&self, track: &str);
    /// The applied change that made the slot's track (the model added it for the search), by its id.
    fn made_by_kumi(&self, slot: &str) -> Option<String>;
    /// Takes that change back (dropping what it made); whether it went.
    async fn take_back(&self, id: &str) -> bool;
}

/// The slots in the order they're tidied: last in the project first, as taking one away moves those after it, and one the
/// project doesn't name now after them. When its tracks can't be read, the last to join goes first.
fn tidy_order(mut slots: Vec<String>, tracks: Option<Vec<String>>) -> Vec<String> {
    let Some(tracks) = tracks else {
        slots.reverse();
        return slots;
    };
    slots.sort_by_key(|slot| std::cmp::Reverse(tracks.iter().position(|name| name == slot).map_or(-1, |at| at as i64)));
    slots
}

/// A finished goal's candidates tidied: the `top` ones muted for the producer to A/B, those the model made taken back,
/// and the producer's own left muted (said). What it says.
pub async fn tidy(daw: &dyn GoalDaw, slots: Vec<String>, top: &[String]) -> Vec<String> {
    let mut said = vec![];
    for slot in tidy_order(slots, daw.track_names().await) {
        let Some(track) = daw.track_of(&slot).await else { continue };
        if top.contains(&slot) {
            daw.mute(&track).await;
            continue;
        }
        let taken_back = match daw.made_by_kumi(&slot) {
            Some(id) => daw.take_back(&id).await,
            None => false,
        };
        if !taken_back {
            daw.mute(&track).await;
            said.push(format!("“{slot}” stays, muted."));
        }
    }
    said
}
