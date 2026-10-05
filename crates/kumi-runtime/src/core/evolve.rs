//! The goal mode's cheap monkeys: an evolutionary search over the knobs of a few candidate chains,
//! with no model call. Each slot is a track with its own chain; a candidate is a set of values for
//! that chain's knobs. Each generation proposes one trial per slot: a few knobs nudged around the
//! slot's best (the step shrinking as it fails, growing as it succeeds), now and then a crossover
//! with another slot of the same chain, now and then a fresh random draw to keep looking wide.
//! Selection keeps each slot's best (an elite per chain, so no one family takes over), and the
//! weakest slot, stuck for long, is reseeded from the leader. Structural leaps (new instruments,
//! topologies) are the model's, between generations.

use serde::{Deserialize, Serialize};

/// A knob a trial may move: its place, its range, and whether it moves in steps.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Knob {
    pub r#ref: String,
    pub device: String,
    pub name: String,
    pub min: f64,
    pub max: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub step: Option<f64>,
    pub value: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Slot {
    /// The track's name: slots are found again by it after a reconnect.
    pub name: String,
    pub label: String,
    /// What chain it is (its devices in order): crossover only between slots of one chain.
    pub chain: String,
    pub knobs: Vec<Knob>,
    /// The best values found for this slot, and their score.
    pub elite: Vec<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub score: Option<f64>,
    /// How far a nudge goes, in the knob's own range (0–1), and generations without gain.
    pub sigma: f64,
    pub stale: u32,
    /// How many renders its best score is the mean of: a render varies, so a lucky one is heard again.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub heard: Option<u32>,
}

use kumi_common::js::number::round;
use regex::Regex;
use std::{
    collections::{HashMap, HashSet},
    sync::LazyLock,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TrialHow {
    Start,
    Nudge,
    Cross,
    Random,
    Recheck,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Trial {
    pub slot: String,
    pub values: Vec<f64>,
    pub how: TrialHow,
}
static LEAVE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^(device on|on|power|output|out|volume|gain|input|master|global volume|limiter.*|ceiling|macro [0-9]+|chain selector|pan|panorama)$").unwrap()
});
static SHAPING: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)(filter|freq|cutoff|res(onance)?|q\b|attack|decay|sustain|release|\benv|shape|wave|tone|timbre|bright|color|colour|drive|dist|sat|detune|fine|coarse|level|mix|amount|depth|rate|spread|width|noise|body|decay|damp|stiff|mallet|feedback|morph|position|pw\b|pulse|glide)").unwrap()
});
pub const MOST_KNOBS: usize = 24;
pub fn searchable(knobs: &[Knob], most: usize) -> Vec<Knob> {
    let open: Vec<_> =
        knobs.iter().filter(|k| k.max > k.min && !LEAVE.is_match(k.name.trim()) && !k.device.to_lowercase().contains("limiter")).collect();
    open.iter()
        .copied()
        .filter(|k| SHAPING.is_match(&k.name))
        .chain(open.iter().copied().filter(|k| !SHAPING.is_match(&k.name)))
        .take(most)
        .cloned()
        .collect()
}
/// Mulberry32, with the same wrapping integer arithmetic as JavaScript's Math.imul.
pub fn seeded(seed: u32) -> impl FnMut() -> f64 {
    let mut state = seed;
    move || {
        state = state.wrapping_add(0x6d2b79f5);
        let mut value = (state ^ (state >> 15)).wrapping_mul(1 | state);
        value = value.wrapping_add((value ^ (value >> 7)).wrapping_mul(61 | value)) ^ value;
        (value ^ (value >> 14)) as f64 / 4294967296.0
    }
}
#[derive(Debug, Clone, Copy)]
pub struct EvolveOptions {
    pub recheck: u32,
    pub moves: usize,
    pub crossover: f64,
    pub random: f64,
    pub patience: u32,
}
pub const EVOLVE: EvolveOptions = EvolveOptions { moves: 3, crossover: 0.2, random: 0.1, patience: 6, recheck: 3 };
#[derive(Debug, Clone)]
pub struct NewSlot {
    pub name: String,
    pub label: String,
    pub chain: String,
    pub knobs: Vec<Knob>,
    pub score: Option<f64>,
    pub heard: Option<u32>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Scored {
    pub improved: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub best: Option<f64>,
}
pub struct Evolution {
    pub slots: Vec<Slot>,
    pub generation: u32,
    pub trend: Vec<f64>,
    pub rendered: u32,
    last_improved: u32,
    random: Box<dyn FnMut() -> f64>,
    options: EvolveOptions,
}
impl Default for Evolution {
    fn default() -> Self {
        Self::new(rand::random::<f64>, EVOLVE)
    }
}
impl Evolution {
    pub fn new(random: impl FnMut() -> f64 + 'static, options: EvolveOptions) -> Self {
        Self { slots: vec![], generation: 0, trend: vec![], rendered: 0, last_improved: 0, random: Box::new(random), options }
    }
    pub fn add(&mut self, slot: NewSlot) -> &Slot {
        let knobs = searchable(&slot.knobs, MOST_KNOBS);
        let elite = knobs.iter().map(|k| k.value).collect();
        let added = Slot {
            name: slot.name,
            label: slot.label,
            chain: slot.chain,
            knobs,
            elite,
            score: slot.score,
            sigma: 0.2,
            stale: 0,
            heard: slot.heard,
        };
        let at = if let Some(at) = self.slots.iter().position(|s| s.name == added.name) {
            self.slots[at] = added;
            at
        } else {
            self.slots.push(added);
            self.slots.len() - 1
        };
        &self.slots[at]
    }
    pub fn freeze(&mut self, name: &str, keys: &HashSet<String>) {
        if let Some(slot) = self.slots.iter_mut().find(|s| s.name == name) {
            let keep: Vec<_> = slot.knobs.iter().map(|k| !keys.contains(&format!("{}|{}", k.device, k.name))).collect();
            slot.elite = slot.elite.iter().enumerate().filter(|(i, _)| keep.get(*i).copied().unwrap_or(false)).map(|(_, v)| *v).collect();
            slot.knobs = slot.knobs.drain(..).enumerate().filter(|(i, _)| keep[*i]).map(|(_, k)| k).collect();
        }
    }
    pub fn remove(&mut self, name: &str) {
        if let Some(at) = self.slots.iter().position(|s| s.name == name) {
            self.slots.remove(at);
        }
    }
    pub fn leader(&self) -> Option<&Slot> {
        self.slots.iter().filter(|s| s.score.is_some()).reduce(|best, s| if s.score.unwrap() > best.score.unwrap() { s } else { best })
    }
    pub fn best(&self) -> Option<f64> {
        self.leader().and_then(|s| s.score)
    }
    pub fn stalled_for(&self) -> u32 {
        self.generation - self.last_improved
    }
    pub fn propose(&mut self) -> Vec<Trial> {
        let mut trials = Vec::with_capacity(self.slots.len());
        for slot in &self.slots {
            let start = || Trial { slot: slot.name.clone(), values: slot.elite.clone(), how: TrialHow::Start };
            if slot.score.is_none() || slot.knobs.is_empty() {
                trials.push(start());
                continue;
            }
            if slot.stale > 0 && self.options.recheck > 0 && slot.stale % self.options.recheck == 0 && slot.heard.unwrap_or(1) < 4 {
                trials.push(Trial { how: TrialHow::Recheck, ..start() });
                continue;
            }
            let roll = (self.random)();
            let partners: Vec<_> = self
                .slots
                .iter()
                .filter(|other| !std::ptr::eq(*other, slot) && other.chain == slot.chain && other.score.is_some())
                .collect();
            if !partners.is_empty() && roll < self.options.crossover {
                let other = partners[((self.random)() * partners.len() as f64).floor() as usize];
                let values = slot
                    .elite
                    .iter()
                    .enumerate()
                    .map(|(i, v)| if (self.random)() < 0.5 { *v } else { other.elite.get(i).copied().unwrap_or(*v) })
                    .collect();
                trials.push(Trial { slot: slot.name.clone(), values, how: TrialHow::Cross });
                continue;
            }
            if roll >= self.options.crossover && roll < self.options.crossover + self.options.random {
                let values = slot.knobs.iter().map(|k| snap(k, k.min + (self.random)() * (k.max - k.min))).collect();
                trials.push(Trial { slot: slot.name.clone(), values, how: TrialHow::Random });
                continue;
            }
            // One value per knob, whatever happened to the elite (a reseed or a refused knob).
            let mut values: Vec<f64> = slot.knobs.iter().enumerate().map(|(i, k)| slot.elite.get(i).copied().unwrap_or(k.value)).collect();
            let count = 1 + ((self.random)() * self.options.moves.min(slot.knobs.len()) as f64).floor() as usize;
            for _ in 0..count {
                let i = ((self.random)() * slot.knobs.len() as f64).floor() as usize;
                let k = &slot.knobs[i];
                let u = (self.random)().max(1e-9);
                let v = (self.random)();
                let normal = (-2.0 * u.ln()).sqrt() * (2.0 * std::f64::consts::PI * v).cos();
                values[i] = snap(k, values[i] + normal * slot.sigma * (k.max - k.min));
            }
            trials.push(Trial { slot: slot.name.clone(), values, how: TrialHow::Nudge });
        }
        trials
    }
    pub fn scored(&mut self, trials: &[Trial], scores: &HashMap<String, f64>) -> Scored {
        self.generation += 1;
        let before = self.best().unwrap_or(-1.0);
        for trial in trials {
            let Some(slot) = self.slots.iter_mut().find(|s| s.name == trial.slot) else {
                continue;
            };
            let Some(&score) = scores.get(&trial.slot) else {
                continue;
            };
            self.rendered += 1;
            if let (TrialHow::Recheck, Some(previous)) = (trial.how, slot.score) {
                let heard = slot.heard.unwrap_or(1) + 1;
                slot.score = Some((previous * (heard - 1) as f64 + score) / heard as f64);
                slot.heard = Some(heard);
                slot.stale += 1;
                continue;
            }
            if slot.score.is_none() || score > slot.score.unwrap() {
                slot.elite = trial.values.clone();
                slot.score = Some(score);
                slot.stale = 0;
                slot.heard = Some(1);
                slot.sigma = (slot.sigma * 1.25).min(0.5);
            } else {
                slot.stale += 1;
                slot.sigma = (slot.sigma * 0.85).max(0.01);
            }
        }
        let best = self.best();
        let improved = best.is_some_and(|s| s > before);
        if improved {
            self.last_improved = self.generation;
        }
        if let Some(best) = best {
            self.trend.push(best);
        }
        self.reseed();
        Scored { improved, best }
    }
    fn reseed(&mut self) {
        let Some(leader) = self.leader().cloned() else {
            return;
        };
        let stuck = self
            .slots
            .iter()
            .enumerate()
            .filter(|(_, s)| s.name != leader.name && s.score.is_some() && s.stale >= self.options.patience)
            .reduce(|best, s| if s.1.score.unwrap() < best.1.score.unwrap() { s } else { best })
            .map(|(i, _)| i);
        let Some(index) = stuck else {
            return;
        };
        let slot = &mut self.slots[index];
        slot.elite = if slot.chain == leader.chain {
            // The leader's best for each of this slot's knobs, matched by device and knob (the n-th of
            // a repeated one to the leader's n-th). A knob Live refused on the leader was frozen out of
            // its elite: this slot keeps its own value there, so its elite stays one value per knob.
            let key = |k: &Knob| (k.device.clone(), k.name.clone());
            let mut seen: HashMap<(String, String), usize> = HashMap::new();
            slot.knobs
                .iter()
                .enumerate()
                .map(|(i, knob)| {
                    let n = {
                        let count = seen.entry(key(knob)).or_default();
                        *count += 1;
                        *count - 1
                    };
                    leader
                        .knobs
                        .iter()
                        .enumerate()
                        .filter(|(_, k)| key(k) == key(knob))
                        .nth(n)
                        .and_then(|(at, _)| leader.elite.get(at).copied())
                        .unwrap_or_else(|| slot.elite.get(i).copied().unwrap_or(knob.value))
                })
                .collect()
        } else {
            slot.knobs.iter().map(|k| snap(k, k.min + (self.random)() * (k.max - k.min))).collect()
        };
        slot.sigma = 0.2;
        slot.stale = 0;
        slot.score = None;
    }
}
fn snap(knob: &Knob, value: f64) -> f64 {
    let clamped = value.max(knob.min).min(knob.max);
    match knob.step.filter(|step| *step > 0.0) {
        Some(step) => (knob.min + round((clamped - knob.min) / step) * step).max(knob.min).min(knob.max),
        None => clamped,
    }
}
