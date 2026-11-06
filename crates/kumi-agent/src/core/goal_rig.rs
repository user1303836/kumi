//! A goal's shared generation engine and state (#399, K29). Backends retain native reads, writes, rendering and cleanup;
//! the core owns accepted values, trial freshness, hearing, scoring and its bounded cache. The native IO and hearing
//! orders are explicit policies, so sharing the engine does not change either backend's failure or commit ordering.
use crate::core::{
    contracts::{Generation, GenerationOptions, GenerationTrial},
    evolve::Knob,
};
use async_trait::async_trait;
use indexmap::IndexMap;
use kumi_common::{abort::Signal, time::now_ms};
use kumi_listening::goal::{hear, tell, tell_timing, trial_key, HeardCache, Told};
use kumi_media::audio::analyze::Analysis;
use kumi_types::{
    contracts::{Focus, Render},
    errors::RuntimeError,
};
use std::{cell::RefCell, collections::HashSet};

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

/// Knobs at their values: what may be sent to a DAW, so none where a value isn't given or isn't a number.
pub fn given_values(knobs: &[Knob], values: &[f64]) -> Vec<(Knob, f64)> {
    knobs.iter().zip(values).filter(|(_, value)| value.is_finite()).map(|(knob, value)| (knob.clone(), *value)).collect()
}

/// A knob's device by its name, without its place in the chain.
pub fn device_of(knob: &Knob) -> &str {
    knob.device.split_once(':').map_or(knob.device.as_str(), |(_, name)| name)
}

/// Whether these are one value of a knob: what a DAW reads back of a value it was given is close, not equal, and a stepped
/// knob lands on its step.
pub fn near(knob: &Knob, a: f64, b: f64) -> bool {
    (a - b).abs() <= (1e-5 * (knob.max - knob.min).abs().max(1e-9)).max(knob.step.unwrap_or(0.) / 2.)
}

/// What's said of these knobs (each as “device knob”), which the producer turned on a track since the search began.
pub fn you_turned(slot: &str, turned: &[String]) -> String {
    let them = if turned.len() == 1 { "it" } else { "them" };
    format!("you turned {} on “{slot}” since the search began, so Kumi left {them} as you set {them}", turned.join(", "))
}

/// A paused goal's best, muted beside its candidate, which plays as it was.
pub fn best_waits_muted(best: &str, slot: &str) -> String {
    format!("“{best}” waits muted beside “{slot}”, which plays as it was: unmute it to hear the best")
}

/// A paused goal's best that couldn't be muted.
pub fn best_plays_beside(best: &str, slot: &str) -> String {
    format!("“{best}” plays beside “{slot}”: mute one of them to hear the other alone")
}

/// A last best the producer worked on: it stays theirs, and why; `after` is anything more to say of it.
pub fn last_best_stays(best: &str, why: &str, after: &str) -> String {
    format!("The last “{best}” stays where it is ({why}){after}; the new best goes on a track of its own")
}

/// A last best left where it is, with why, and where the new one is: why there are two.
pub fn last_best_beside(last: &str, why: &str, new: &str) -> String {
    format!("The last “{last}” stays where it is ({why}), so the new best is beside it on “{new}”.")
}

/// A safety limiter that couldn't come off a candidate.
pub fn limiter_stays(limiter: &str, track: &str) -> String {
    format!("Kumi's safety {limiter} stays last on “{track}”: delete it if you don't want it there.")
}

/// Why nothing was set: these knobs of a slot, which the search moved, moved or went meanwhile. Each device is named once,
/// first seen first.
pub fn moved_or_went(slot: &str, knobs: &[Knob]) -> String {
    let mut named: Vec<String> = vec![];
    for knob in knobs {
        let name = format!("“{}”", device_of(knob));
        if !named.contains(&name) {
            named.push(name);
        }
    }
    format!(
        "{} moved or went on “{slot}” since the search began, so Kumi set nothing there: start the goal again to search the chain as it is now",
        named.join(", ")
    )
}

/// A trial's knobs at its values, those that moved from `last` (the values last set on its slot, by key): one with no value
/// set yet always moves.
pub fn moved(trial: &GenerationTrial, last: Option<&IndexMap<String, f64>>) -> Vec<(Knob, f64)> {
    given_values(&trial.knobs, &trial.values)
        .into_iter()
        .filter(|(knob, value)| last.and_then(|last| last.get(&knob_key(knob))).is_none_or(|last| (last - value).abs() > 1e-6))
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

/// Whether all native reads must finish before any trial writes, or each trial is read and set in turn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SetOrder {
    ReadAllThenSet,
    OneAtATime,
}

/// Hearing is either committed as each result arrives, or only after the entire batch succeeds, in trial order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HearOrder {
    AsCompleted,
    AfterBatch,
}

/// A backend's cache payload and ordering, independent of a borrowed native pass.
pub trait GoalBackend {
    type Cached: Clone;
    const SET_ORDER: SetOrder;
    const HEAR_ORDER: HearOrder;
    fn cached_told(cached: Self::Cached) -> Told;
    fn cache(told: Told, render: &Render) -> Self::Cached;
}

/// A short reference stretch used while screening, relative to each candidate's window.
pub struct GoalScreen {
    pub offset: f64,
    pub beats: f64,
    pub reference: Analysis,
}

/// Native IO for one generation. A pass borrows its rig and owns any render/rig guards until the required boundary.
#[async_trait(?Send)]
pub trait GoalPass<B: GoalBackend> {
    type Plan;
    fn signal(&self, given: Signal) -> Signal;
    fn begin(&mut self) -> Result<(), RuntimeError>;
    /// Availability checks may also retire native sources and their shared current-value entries.
    async fn before_set(&mut self, _signal: Signal) -> Result<Vec<String>, RuntimeError> {
        Ok(vec![])
    }
    async fn before_render(&mut self, _signal: Signal) -> Result<Vec<String>, RuntimeError> {
        Ok(vec![])
    }
    /// None means the slot cannot be set; every moved knob is frozen. No current value is accepted in that case.
    async fn plan(&mut self, slot: &str, moved: &[(Knob, f64)], signal: Signal) -> Result<Option<Self::Plan>, RuntimeError>;
    /// Returns exactly the refused knobs. Backend-owned restore history is recorded here; the core accepts the rest.
    async fn set(&mut self, slot: &str, moved: &[(Knob, f64)], plan: Self::Plan, signal: Signal) -> Result<HashSet<String>, RuntimeError>;
    async fn render(
        &mut self,
        fresh: &[&GenerationTrial],
        screen: Option<&GoalScreen>,
        signal: Signal,
    ) -> Result<IndexMap<String, Render>, RuntimeError>;
    /// Called even when rendering fails: a backend may release its rendering flag before hearing starts.
    fn rendered(&mut self);
    /// Beats and tempo to hear at. A backend releases a held rig here when hearing must not hold its mutex.
    fn hearing_window(&mut self, screen: Option<&GoalScreen>) -> (f64, f64);
    async fn hear(&self, render: &Render, beats: f64, tempo: f64, focus: Option<Focus>, signal: Signal) -> Result<Analysis, RuntimeError> {
        hear(render, beats, tempo, focus, signal).await.map_err(|error| RuntimeError::plain(error.to_string()))
    }
    fn score(&self, heard: &Analysis, against: &Analysis, focus: Option<Focus>) -> Option<Told> {
        tell(heard, against, focus)
    }
}

/// The generation state shared by every goal backend. Native ownership and kept-best records stay on the renderer.
pub struct GoalCore<B: GoalBackend> {
    pub current: RefCell<IndexMap<String, IndexMap<String, f64>>>,
    pub reference: Analysis,
    pub focus: Option<Focus>,
    pub screen: Option<GoalScreen>,
    pub heard: RefCell<HeardCache<B::Cached>>,
}

struct Planned<P> {
    moved: Vec<(Knob, f64)>,
    native: Option<P>,
}

impl<B: GoalBackend> GoalCore<B> {
    pub fn new(reference: Analysis, focus: Option<Focus>) -> Self {
        Self { current: RefCell::default(), reference, focus, screen: None, heard: RefCell::new(HeardCache::default()) }
    }

    async fn plan<P: GoalPass<B>>(
        &self,
        pass: &mut P,
        trial: &GenerationTrial,
        generation: &mut Generation,
        signal: Signal,
    ) -> Result<Planned<P::Plan>, RuntimeError> {
        let moved = moved(trial, self.current.borrow().get(&trial.slot));
        let native = if moved.is_empty() { None } else { pass.plan(&trial.slot, &moved, signal).await? };
        if !moved.is_empty() && native.is_none() {
            generation.frozen.insert(trial.slot.clone(), moved.iter().map(|(knob, _)| knob_key(knob)).collect());
        }
        Ok(Planned { moved, native })
    }

    async fn set<P: GoalPass<B>>(
        &self,
        pass: &mut P,
        trial: &GenerationTrial,
        plan: Planned<P::Plan>,
        generation: &mut Generation,
        signal: Signal,
    ) -> Result<(), RuntimeError> {
        let Some(native) = plan.native else { return Ok(()) };
        let refused = pass.set(&trial.slot, &plan.moved, native, signal).await?;
        let mut current = self.current.borrow_mut();
        let last = current.entry(trial.slot.clone()).or_default();
        for (knob, value) in plan.moved.iter().filter(|(knob, _)| !refused.contains(&knob_key(knob))) {
            last.insert(knob_key(knob), *value);
        }
        if !refused.is_empty() {
            generation.frozen.insert(trial.slot.clone(), refused);
        }
        Ok(())
    }

    /// One engine chooses moved values, freshness, hearing and result/cache commits. Backends supply only native IO.
    pub async fn generation<P: GoalPass<B>>(
        &self,
        mut pass: P,
        trials: &[GenerationTrial],
        given: Signal,
        options: Option<GenerationOptions>,
    ) -> Result<Generation, RuntimeError> {
        check_slots(trials, |slot| self.current.borrow().contains_key(slot))?;
        let signal = pass.signal(given);
        let screened = options.and_then(|options| options.screen) == Some(true) && self.screen.is_some();
        let screen = self.screen.as_ref().filter(|_| screened);
        let cache_key = |trial: &GenerationTrial| trial_key(&trial.slot, screened, &trial.values);
        pass.begin()?;
        let set_from = now_ms();
        let mut generation = Generation { screened, ..Default::default() };
        generation.gone = pass.before_set(signal.clone()).await?;
        let mut active: Vec<_> = trials.iter().filter(|trial| !generation.gone.contains(&trial.slot)).collect();
        match B::SET_ORDER {
            SetOrder::ReadAllThenSet => {
                let mut plans = Vec::with_capacity(active.len());
                for trial in &active {
                    plans.push(self.plan(&mut pass, trial, &mut generation, signal.clone()).await?);
                }
                for (trial, plan) in active.iter().zip(plans) {
                    self.set(&mut pass, trial, plan, &mut generation, signal.clone()).await?;
                }
            }
            SetOrder::OneAtATime => {
                for trial in &active {
                    let plan = self.plan(&mut pass, trial, &mut generation, signal.clone()).await?;
                    self.set(&mut pass, trial, plan, &mut generation, signal.clone()).await?;
                }
            }
        }
        generation.gone.extend(pass.before_render(signal.clone()).await?);
        active.retain(|trial| !generation.gone.contains(&trial.slot));
        let render_from = now_ms();
        let fresh: Vec<_> =
            active.iter().copied().filter(|trial| trial.fresh == Some(true) || !self.heard.borrow().contains(&cache_key(trial))).collect();
        let rendered = if fresh.is_empty() { Ok(IndexMap::new()) } else { pass.render(&fresh, screen, signal.clone()).await };
        pass.rendered();
        let files = rendered?;
        let heard_from = now_ms();
        let (beats, tempo) = pass.hearing_window(screen);
        let against = screen.map_or(&self.reference, |screen| &screen.reference);
        match B::HEAR_ORDER {
            HearOrder::AsCompleted => {
                let generation = RefCell::new(&mut generation);
                eager_hearing(active.iter().map(|trial| {
                    let pass = &pass;
                    let signal = signal.clone();
                    let files = &files;
                    let generation = &generation;
                    let cache_key = &cache_key;
                    async move {
                        let key = cache_key(trial);
                        let known = if trial.fresh != Some(true) { self.heard.borrow().get(&key) } else { None };
                        if let Some(known) = known {
                            tell_generation(&mut generation.borrow_mut(), &trial.slot, B::cached_told(known), true);
                        } else if let Some(render) = files.get(&trial.slot) {
                            let heard = pass.hear(render, beats, tempo, self.focus, signal).await?;
                            if let Some(told) = pass.score(&heard, against, self.focus) {
                                tell_generation(&mut generation.borrow_mut(), &trial.slot, told.clone(), false);
                                self.heard.borrow_mut().insert(key, B::cache(told, render));
                            } else {
                                generation.borrow_mut().silent.push(trial.slot.clone());
                            }
                        } else {
                            generation.borrow_mut().silent.push(trial.slot.clone());
                        }
                        Ok::<_, RuntimeError>(())
                    }
                }))
                .await?;
            }
            HearOrder::AfterBatch => {
                let known: Vec<_> = active
                    .iter()
                    .map(|trial| {
                        let key = cache_key(trial);
                        let cached = if trial.fresh != Some(true) { self.heard.borrow().get(&key) } else { None };
                        (*trial, key, cached)
                    })
                    .collect();
                let hearing = known
                    .iter()
                    .filter(|(_, _, cached)| cached.is_none())
                    .filter_map(|(trial, _, _)| files.get(&trial.slot))
                    .map(|render| pass.hear(render, beats, tempo, self.focus, signal.clone()));
                let mut heard = futures::future::try_join_all(hearing).await?.into_iter();
                for (trial, key, cached) in known {
                    if let Some(cached) = cached {
                        tell_generation(&mut generation, &trial.slot, B::cached_told(cached), true);
                    } else if let Some(render) = files.get(&trial.slot) {
                        let Some(heard) = heard.next() else { break };
                        if let Some(told) = pass.score(&heard, against, self.focus) {
                            tell_generation(&mut generation, &trial.slot, told.clone(), false);
                            self.heard.borrow_mut().insert(key, B::cache(told, render));
                        } else {
                            generation.silent.push(trial.slot.clone());
                        }
                    } else {
                        generation.silent.push(trial.slot.clone());
                    }
                }
            }
        }
        tell_timing(active.len(), render_from - set_from, heard_from - render_from, now_ms() - heard_from);
        Ok(generation)
    }
}

/// Every ready hearing is polled before an error returns; an immediate error must not prevent later work dispatch.
async fn eager_hearing<I, F, T, E>(work: I) -> Result<Vec<T>, E>
where
    I: IntoIterator<Item = F>,
    F: std::future::Future<Output = Result<T, E>>,
{
    use std::task::Poll;
    let mut work: Vec<_> = work.into_iter().map(|future| Some(Box::pin(future))).collect();
    let mut values: Vec<Option<T>> = std::iter::repeat_with(|| None).take(work.len()).collect();
    std::future::poll_fn(move |cx| {
        let mut error = None;
        let mut pending = false;
        for (index, future) in work.iter_mut().enumerate() {
            let Some(active) = future else { continue };
            match active.as_mut().poll(cx) {
                Poll::Pending => pending = true,
                Poll::Ready(value) => {
                    *future = None;
                    match value {
                        Ok(value) => values[index] = Some(value),
                        Err(why) if error.is_none() => error = Some(why),
                        Err(_) => {}
                    }
                }
            }
        }
        if let Some(error) = error {
            Poll::Ready(Err(error))
        } else if pending {
            Poll::Pending
        } else {
            Poll::Ready(Ok(values.iter_mut().map(|value| value.take().unwrap()).collect()))
        }
    })
    .await
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
