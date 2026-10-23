//! What Kumi's searches hear, kept in its database (`heard_settings`): every setting tune's knob search, a match run's
//! and a goal's heard on a device, with how it sounded, so a later search on the same device starts from what worked.
//! Each generation's rows are written as it's heard; a search's best is marked once it ends: its cheapest setting on
//! each device, one heard at full length before any heard on a short screening window.
//!
//! A search reads back each past search's best on its device (`SettingsMemory::bests`): tune hears the nearest few in
//! its first generation, and a goal's or a match run's slot tries the best for its devices once its own start is heard.
use crate::core::{
    contracts::{Generation, GenerationTrial},
    evolve::Knob,
    store_client::StoreClient,
};
use async_trait::async_trait;
use kumi_common::time::now_ms;
use kumi_listening::heard_settings::{DeviceIdentity, Remembered, SearchKind, Setting, SettingsHeard, SettingsMemory};
use kumi_store::{
    heard_settings::{self, HeardSetting, Source},
    ids::new_id,
    StoreError,
};
use kumi_types::daw::DawId;
use serde_json::{json, Value};
use std::{
    cell::RefCell,
    collections::{BTreeMap, HashMap},
    rc::Rc,
};

/// The past searches a new one compares itself with, at most, on a device.
const PAST: usize = 200;

/// A search's best so far on one device: the row, its cost, and whether it was heard on a short window.
struct Best {
    id: String,
    cost: f64,
    screened: bool,
}

/// What Kumi's searches remember, in a line of /status.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SearchMemoryStatus {
    /// Settings kept, and the devices they're on.
    pub settings: usize,
    pub devices: usize,
    /// Searches whose first generation heard a remembered setting, and those where one was that generation's best.
    pub started: usize,
    pub won: usize,
}

pub struct SearchMemory {
    store: StoreClient,
    /// The DAW the session works in: a search that doesn't say is heard there.
    daw: DawId,
    /// Each search under way, by its run: its best on each device so far.
    runs: RefCell<HashMap<String, HashMap<String, Best>>>,
    now: Rc<dyn Fn() -> i64>,
}

impl SearchMemory {
    pub fn new(store: StoreClient, daw: DawId) -> Rc<SearchMemory> {
        Self::with_clock(store, daw, Rc::new(now_ms))
    }
    pub fn with_clock(store: StoreClient, daw: DawId, now: Rc<dyn Fn() -> i64>) -> Rc<SearchMemory> {
        Rc::new(SearchMemory { store, daw, runs: RefCell::default(), now })
    }

    /// How many settings Kumi keeps and on how many devices, and how often one remembered won a search's first round.
    pub async fn status(&self) -> Result<SearchMemoryStatus, StoreError> {
        let ((settings, devices), (started, won)) =
            self.store.read(|c| Ok((heard_settings::counts(c)?, heard_settings::first_rounds(c)?))).await?;
        Ok(SearchMemoryStatus { settings, devices, started, won })
    }

    /// A setting to try on a goal's or a match run's slot: for each of its devices, the best a past goal or match run
    /// heard there (the closest it came to its reference), on the knobs it shares; one value for each of `knobs` (the
    /// rest as `elite` has them), and which remembered setting each device's came from. None when nothing is remembered
    /// for them, or it's where the slot already is.
    pub async fn suggestion(&self, search: &SlotSearch, knobs: &[Knob], elite: &[f64]) -> Option<(Vec<f64>, BTreeMap<String, String>)> {
        let mut values: Vec<f64> = knobs.iter().enumerate().map(|(i, knob)| elite.get(i).copied().unwrap_or(knob.value)).collect();
        let mut from = BTreeMap::new();
        // Each device once, in the order its knobs first come.
        let mut devices: Vec<String> = vec![];
        for device in knobs.iter().map(|knob| search.device(knob)) {
            if !devices.contains(&device) {
                devices.push(device);
            }
        }
        for device in devices {
            let past = self.bests(None, &device, &[SearchKind::Goal, SearchKind::Match]).await;
            let Some(best) = past
                .into_iter()
                .filter(|past| past.target.get("kind").and_then(Value::as_str) == Some("reference"))
                .min_by(|a, b| a.cost.total_cmp(&b.cost).then(b.at.cmp(&a.at)))
            else {
                continue;
            };
            for (i, knob) in knobs.iter().enumerate().filter(|(_, knob)| search.device(knob) == device) {
                if let Some(at) = best.knobs.get(&knob.name) {
                    values[i] = snapped(knob, knob.min + at.clamp(0., 1.) * (knob.max - knob.min));
                    from.insert(device.clone(), best.id.clone());
                }
            }
        }
        let moved = values.iter().zip(elite).any(|(value, was)| (value - was).abs() > 1e-9);
        (moved && !from.is_empty()).then_some((values, from))
    }
}

#[async_trait(?Send)]
impl SettingsMemory for SearchMemory {
    /// What a search heard, kept: a generation's settings as rows, or, once it ended, its best marked.
    fn heard(&self, event: SettingsHeard) {
        match event {
            SettingsHeard::Heard { run, kind, daw, device, track, target, before, settings } => {
                let at = (self.now)();
                let daw = daw.unwrap_or(self.daw).as_str().to_string();
                let mut rows = Vec::with_capacity(settings.len());
                let mut runs = self.runs.borrow_mut();
                let best = runs.entry(run.clone()).or_default();
                for Setting { knobs, measured, cost, generation, remembered } in
                    settings.into_iter().filter(|setting| setting.cost.is_finite())
                {
                    let id = new_id();
                    let screened = measured.get("screened") == Some(&Value::Bool(true));
                    // One heard at full length is better evidence than any heard on a short window.
                    let better = best.get(&device.name).is_none_or(|kept| (screened, cost) < (kept.screened, kept.cost));
                    if better {
                        best.insert(device.name.clone(), Best { id: id.clone(), cost, screened });
                    }
                    rows.push(HeardSetting {
                        id,
                        at,
                        daw: daw.clone(),
                        device: device.name.clone(),
                        device_version: device.version.clone(),
                        track: track.clone(),
                        source: source(kind),
                        run_id: run.clone(),
                        knobs: Value::Object(knobs.into_iter().map(|(name, at)| (name, json!(at))).collect()),
                        target: target.clone(),
                        before: before.clone(),
                        measured,
                        cost,
                        best: false,
                        generation,
                        remembered,
                    });
                }
                if !rows.is_empty() {
                    self.store.store().write(move |connection| heard_settings::add(connection, &rows), |_| {});
                }
            }
            SettingsHeard::Ended { run } => {
                let Some(best) = self.runs.borrow_mut().remove(&run) else { return };
                let ids: Vec<String> = best.into_values().map(|best| best.id).collect();
                if !ids.is_empty() {
                    self.store.store().write(move |connection| heard_settings::mark_best(connection, &run, &ids), |_| {});
                }
            }
        }
    }

    async fn bests(&self, daw: Option<DawId>, device: &str, kinds: &[SearchKind]) -> Vec<Remembered> {
        let (daw, device) = (daw.unwrap_or(self.daw).as_str().to_string(), device.to_string());
        let sources: Vec<Source> = kinds.iter().map(|kind| source(*kind)).collect();
        let rows = self.store.read(move |c| heard_settings::bests_on_device(c, &daw, &device, &sources, PAST)).await.unwrap_or_default();
        rows.into_iter()
            .map(|row| Remembered {
                kind: kind(row.source),
                knobs: row.knobs.as_object().into_iter().flatten().filter_map(|(name, at)| Some((name.clone(), at.as_f64()?))).collect(),
                id: row.id,
                at: row.at,
                track: row.track,
                target: row.target,
                before: row.before,
                cost: row.cost,
            })
            .collect()
    }
}

fn source(kind: SearchKind) -> Source {
    match kind {
        SearchKind::Tune => Source::Tune,
        SearchKind::Match => Source::Match,
        SearchKind::Goal => Source::Goal,
    }
}

fn kind(source: Source) -> SearchKind {
    match source {
        Source::Tune => SearchKind::Tune,
        Source::Match => SearchKind::Match,
        Source::Goal => SearchKind::Goal,
    }
}

/// A raw value on a knob's steps, inside its range.
fn snapped(knob: &Knob, value: f64) -> f64 {
    let clamped = value.max(knob.min).min(knob.max);
    match knob.step.filter(|step| *step > 0.) {
        Some(step) => (knob.min + ((clamped - knob.min) / step).round() * step).clamp(knob.min, knob.max),
        None => clamped,
    }
}

/// A goal's (or a match run's) search over one slot: the slot's run, what the search aims at, and who each of its
/// knobs' devices is.
pub struct SlotSearch {
    pub run: String,
    pub track: String,
    /// `Knob::device` to who the device is (the rig's word for it); a device it doesn't name is known by its name.
    pub devices: BTreeMap<String, String>,
    /// Each knob's value before the search moved it, by `Knob::device` and name.
    pub before: HashMap<(String, String), f64>,
    /// The remembered setting the slot tries (`SearchMemory::suggestion`): which remembered row each device's knobs came
    /// from.
    pub suggested: BTreeMap<String, String>,
}

impl SlotSearch {
    /// A search over a slot's knobs as they are now (before any trial moved them).
    pub fn new(track: &str, knobs: &[Knob], devices: BTreeMap<String, String>) -> Self {
        SlotSearch {
            run: new_id(),
            track: track.into(),
            devices,
            before: knobs.iter().map(|knob| ((knob.device.clone(), knob.name.clone()), knob.value)).collect(),
            suggested: BTreeMap::new(),
        }
    }
    /// Who a knob's device is: what the rig said, else its name in the chain (never its place).
    pub fn device(&self, knob: &Knob) -> String {
        self.devices.get(&knob.device).cloned().unwrap_or_else(|| {
            knob.device
                .split_once(':')
                .filter(|(place, _)| !place.is_empty() && place.bytes().all(|b| b.is_ascii_digit()))
                .map_or(knob.device.clone(), |(_, name)| name.to_string())
        })
    }
}

/// A knob's value as a fraction of its range (a goal's knobs carry no perceptual scale).
fn fraction(knob: &Knob, value: f64) -> f64 {
    if knob.max > knob.min {
        ((value - knob.min) / (knob.max - knob.min)).clamp(0., 1.)
    } else {
        0.
    }
}

/// A generation's trials as settings heard, a device at a time: each scored trial's knobs on that device (fractions of
/// their ranges), how it scored (its cost is how far its score is from a full match), whether it was heard on the
/// short screening window, which generation heard it, and the remembered setting it was (a slot's in `remembered`
/// tried what its search suggested). `searches` are the slots' searches by slot.
pub fn trials_heard(
    kind: SearchKind,
    searches: &HashMap<String, SlotSearch>,
    trials: &[GenerationTrial],
    generation: &Generation,
    target: &Value,
    heard_in: Option<u32>,
    remembered: &[String],
) -> Vec<SettingsHeard> {
    let mut heard = vec![];
    for trial in trials {
        let (Some(search), Some(score)) = (searches.get(&trial.slot), generation.scores.get(&trial.slot)) else { continue };
        if !score.is_finite() {
            continue;
        }
        // The trial's knobs, grouped by the device they're on, in the chain's order.
        let mut devices: Vec<(String, Vec<(&Knob, f64)>)> = vec![];
        for (knob, value) in trial.knobs.iter().zip(&trial.values) {
            let device = search.device(knob);
            match devices.iter_mut().find(|(name, _)| *name == device) {
                Some((_, knobs)) => knobs.push((knob, *value)),
                None => devices.push((device, vec![(knob, *value)])),
            }
        }
        let measured = json!({
            "score": score,
            "screened": generation.screened,
            "gaps": generation.gaps.get(&trial.slot).cloned().unwrap_or_default(),
        });
        for (device, knobs) in devices {
            let was: serde_json::Map<String, Value> = knobs
                .iter()
                .filter_map(|(knob, _)| {
                    let value = search.before.get(&(knob.device.clone(), knob.name.clone()))?;
                    Some((knob.name.clone(), json!(fraction(knob, *value))))
                })
                .collect();
            let from = remembered.contains(&trial.slot).then(|| search.suggested.get(&device).cloned()).flatten();
            heard.push(SettingsHeard::Heard {
                run: search.run.clone(),
                kind,
                daw: None,
                device: DeviceIdentity::named(device),
                track: Some(search.track.clone()),
                target: target.clone(),
                before: Some(json!({ "knobs": was })),
                settings: vec![Setting {
                    knobs: knobs.iter().map(|(knob, value)| (knob.name.clone(), fraction(knob, *value))).collect(),
                    measured: measured.clone(),
                    cost: 100. - score,
                    generation: heard_in,
                    remembered: from,
                }],
            });
        }
    }
    heard
}
