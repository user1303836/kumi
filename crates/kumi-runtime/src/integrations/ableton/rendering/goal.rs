use super::super::{bridge_version::GOAL_BRIDGE, concurrent::eager_all, connection::NO_CURRENT_LIVE};
use super::*;
use super::{
    listen::heard_options,
    rig::{Held, Rig, Source, Window},
};
use crate::{audio::matching::closeness, core::evolve::Knob, integrations::ableton::audition::silent_render};
use async_trait::async_trait;
use kumi_common::js::number::{parse, round};
use std::collections::{HashMap, HashSet};
#[derive(Clone)]
struct Knobs {
    reference: String,
    chain: String,
    devices: Vec<JsonObject>,
    knobs: Vec<Knob>,
}
struct Screen {
    from: f64,
    beats: f64,
    reference: Analysis,
}
#[derive(Clone)]
struct Cached {
    score: f64,
    gaps: Vec<String>,
    structural: Option<StructuralMove>,
}
struct Goal {
    render: Rc<Rendering>,
    rig: tokio::sync::Mutex<Rig>,
    slots: RefCell<Vec<GoalSlotInfo>>,
    current: RefCell<IndexMap<String, IndexMap<String, f64>>>,
    known: RefCell<IndexMap<String, (u64, Knobs)>>,
    reference: Analysis,
    focus: Option<Focus>,
    screen: Option<Screen>,
    heard: RefCell<IndexMap<String, Cached>>,
}
fn key(knob: &Knob) -> String {
    format!("{}|{}", knob.device, knob.name)
}
fn device_name(device: &JsonObject) -> String {
    device
        .get("name")
        .filter(|v| !v.is_null())
        .or_else(|| device.get("className").filter(|v| !v.is_null()))
        .map(js_string)
        .unwrap_or_else(|| "Device".into())
}
fn gain(read: &Knobs) -> Option<&Knob> {
    let limiter = read.devices.last()?;
    let name = limiter.get("name").filter(|v| !v.is_null()).map(js_string).unwrap_or_else(|| "Limiter".into());
    read.knobs.iter().find(|knob| {
        knob.device == format!("{}:{name}", read.devices.len() - 1)
            && matches!(knob.name.to_ascii_lowercase().as_str(), "gain" | "input" | "input gain")
    })
}
fn by_device(read: &Knobs, moved: &[(Knob, f64)]) -> IndexMap<String, Vec<Value>> {
    let by_key: HashMap<_, _> = read.knobs.iter().map(|knob| (key(knob), knob)).collect();
    let mut devices: IndexMap<String, Vec<Value>> = IndexMap::new();
    for (knob, value) in moved {
        let Some(now) = by_key.get(&key(knob)) else {
            continue;
        };
        let Some(index) = parse(knob.device.split(':').next().unwrap_or("")).filter(|n| *n >= 0. && n.fract() == 0. && n.is_finite())
        else {
            continue;
        };
        let Some(reference) = read.devices.get(index as usize).and_then(|device| device.get("ref")).and_then(Value::as_str) else {
            continue;
        };
        devices.entry(reference.into()).or_default().push(json!({"parameterRef":now.r#ref,"value":value}));
    }
    devices
}
impl Rendering {
    async fn read_knobs(&self, name: &str, signal: Signal) -> Result<Knobs, RuntimeError> {
        let tracks = self.rows("track", json!({"fields":["name"]}), signal.clone()).await?;
        let reference = tracks
            .iter()
            .find(|track| track.get("name").and_then(Value::as_str) == Some(name))
            .and_then(|track| track.get("ref"))
            .and_then(Value::as_str)
            .ok_or_else(|| observation(format!("The track “{name}” is gone.")))?
            .to_owned();
        let devices = self.rows("device", json!({"parent":reference,"fields":["name","className"]}), signal.clone()).await?;
        let mut knobs = vec![];
        for (index, device) in devices.iter().enumerate() {
            let mut input = object(json!({"fields":["name","value","min","max","quantization"]}));
            if let Some(reference) = device.get("ref") {
                input.insert("parent".into(), reference.clone());
            }
            for parameter in self.connection().rows("parameter", input, signal.clone()).await? {
                let (Some(reference), Some(name), Some(value), Some(min), Some(max)) = (
                    parameter.get("ref").and_then(Value::as_str),
                    parameter.get("name").and_then(Value::as_str),
                    parameter.get("value").and_then(Value::as_f64),
                    parameter.get("min").and_then(Value::as_f64),
                    parameter.get("max").and_then(Value::as_f64),
                ) else {
                    continue;
                };
                knobs.push(Knob {
                    r#ref: reference.into(),
                    device: format!("{index}:{}", device_name(device)),
                    name: name.into(),
                    value,
                    min,
                    max,
                    step: parameter.get("quantization").and_then(Value::as_f64).filter(|v| *v > 0.),
                });
            }
        }
        let chain = devices.iter().map(device_name).collect::<Vec<_>>().join(" → ");
        Ok(Knobs { reference, chain: if chain.is_empty() { "empty".into() } else { chain }, devices, knobs })
    }
    pub async fn open_goal(
        self: &Rc<Self>,
        request: &AuditionRequest,
        original: Signal,
    ) -> Result<Result<Rc<dyn GoalRig>, String>, RuntimeError> {
        if request.candidates.iter().any(|candidate| candidate.mix == Some(true) || candidate.track == MIX_CANDIDATE) {
            return Ok(Err("A goal searches a track's own devices. For the whole mix, audition it against the reference (candidates [{\"mix\": true}]) and change EQ, compression and levels between rounds.".into()));
        }
        if !self.available() {
            return Ok(Err(NO_CURRENT_LIVE.into()));
        }
        if !self.supported(GOAL_BRIDGE) {
            return Ok(Err(self.too_old(GOAL_BRIDGE)));
        }
        let Some(named) = request.reference.as_deref().filter(|s| !s.is_empty()) else {
            return Ok(Err("A goal needs a reference to reach.".into()));
        };
        if self.observer.tempo.get().is_none_or(|v| v == 0. || v.is_nan()) {
            return Ok(Err("Kumi doesn't know the Set's tempo yet; try again.".into()));
        }
        let signal = abort::any([original, self.connection().lifetime.clone()]);
        let reference = self.heard_reference(named, request, signal.clone()).await?;
        let mut rig = self.open_rig(&request.candidates, request.from_beat, request.beats, signal.clone()).await?;
        rig.hold = Some(Held::default());
        self.tell("Live stays quiet while the goal searches; it comes back when the goal stops or pauses", None);
        let sources: Vec<_> = rig.sources.iter().map(|source| (source.name.clone(), source.label.clone())).collect();
        let mut goal = Goal {
            render: self.clone(),
            rig: tokio::sync::Mutex::new(rig),
            slots: RefCell::new(vec![]),
            current: RefCell::new(IndexMap::new()),
            known: RefCell::new(IndexMap::new()),
            reference,
            focus: request.focus,
            screen: None,
            heard: RefCell::new(IndexMap::new()),
        };
        for (name, label) in sources {
            if let Err(error) = goal.adopt(&name, &label, signal.clone()).await {
                self.close_rig(goal.rig.get_mut()).await;
                return Err(error);
            }
        }
        let rig = goal.rig.get_mut();
        let tempo = self.observer.tempo.get().unwrap();
        if rig.beats * 60. / tempo >= 6. {
            let beats = round(3. * tempo / 60.).max(2.);
            let lufs = &goal.reference.over_time.lufs;
            let slice = parse_float(&goal.reference.over_time.every)
                .filter(|v| *v != 0. && !v.is_nan())
                .unwrap_or(goal.reference.seconds / lufs.len().max(1) as f64);
            let across = round(beats * 60. / tempo / slice).max(1.);
            let mut best_at = 0;
            let mut best_score = f64::NEG_INFINITY;
            if across.is_finite() && across <= lufs.len() as f64 {
                let across = across as usize;
                for at in 0..=lufs.len() - across {
                    let part: Vec<_> = lufs[at..at + across].iter().map(|v| v.unwrap_or(-70.)).collect();
                    let mean = part.iter().sum::<f64>() / part.len() as f64;
                    let change = part.windows(2).map(|v| (v[1] - v[0]).abs()).sum::<f64>();
                    if mean + change > best_score {
                        best_score = mean + change;
                        best_at = at;
                    }
                }
            }
            let offset = round(best_at as f64 * slice * tempo / 60.).min(rig.beats - beats).max(0.);
            let mut snippet = request.clone();
            snippet.reference_from = Some(request.reference_from.unwrap_or(0.) + offset * 60. / tempo);
            snippet.reference_seconds = Some(beats * 60. / tempo);
            let reference = self.heard_reference(named, &snippet, signal).await?;
            goal.screen = Some(Screen { from: rig.from + offset, beats, reference });
        }
        Ok(Ok(Rc::new(goal)))
    }
}
impl Goal {
    fn signal(&self, given: Signal) -> Signal {
        abort::any([given, self.render.connection().lifetime.clone()])
    }
    async fn knobs_now(&self, name: &str, signal: Signal) -> Result<Knobs, RuntimeError> {
        let lease = self.render.connection().lease.get();
        if let Some((known, read)) = self.known.borrow().get(name) {
            if *known == lease {
                return Ok(read.clone());
            }
        }
        let read = self.render.read_knobs(name, signal).await?;
        self.known.borrow_mut().insert(name.into(), (self.render.connection().lease.get(), read.clone()));
        Ok(read)
    }
    async fn adopt(&self, name: &str, label: &str, signal: Signal) -> Result<GoalSlotInfo, RuntimeError> {
        let r = &self.render;
        let mut read = r.read_knobs(name, signal.clone()).await?;
        if read.devices.last().and_then(|d| d.get("className")).and_then(Value::as_str) != Some("Limiter") {
            r.history
                .quietly(None, r.step("load_device", json!({"itemId":"audio_effects/Limiter","trackRef":read.reference}), signal.clone()))
                .await?;
            read = r.read_knobs(name, signal.clone()).await?;
            if let (Some(input), Some(reference)) = (gain(&read), read.devices.last().and_then(|d| d.get("ref")).and_then(Value::as_str)) {
                let down = if input.min < 0. {
                    Some(input.min.max(-12.))
                } else if input.min == 0. && input.max == 1. {
                    Some(0.25)
                } else {
                    None
                };
                if let Some(down) = down {
                    let _ = r
                        .history
                        .quietly(
                            None,
                            r.step(
                                "set_device_parameters",
                                json!({"deviceRef":reference,"values":[{"parameterRef":input.r#ref,"value":down}]}),
                                signal.clone(),
                            ),
                        )
                        .await;
                    read = r.read_knobs(name, signal).await?;
                }
            }
        }
        self.current.borrow_mut().insert(name.into(), read.knobs.iter().map(|knob| (key(knob), knob.value)).collect());
        let slot = GoalSlotInfo { name: name.into(), label: label.into(), chain: read.chain, knobs: read.knobs };
        self.slots.borrow_mut().push(slot.clone());
        Ok(slot)
    }
    async fn set_slot(&self, slot: &str, knobs: &[Knob], values: &[f64], signal: Signal) -> Result<(), RuntimeError> {
        let fresh = self.render.read_knobs(slot, signal.clone()).await?;
        let moved: Vec<_> =
            knobs.iter().enumerate().map(|(index, knob)| (knob.clone(), values.get(index).copied().unwrap_or(f64::NAN))).collect();
        let devices = by_device(&fresh, &moved);
        self.render
            .history
            .quietly(None, async {
                for (reference, values) in devices {
                    for chunk in values.chunks(64) {
                        self.render.step("set_device_parameters", json!({"deviceRef":reference,"values":chunk}), signal.clone()).await?;
                    }
                }
                Ok::<_, RuntimeError>(())
            })
            .await?;
        let mut current = self.current.borrow_mut();
        let last = current.get_mut(slot).ok_or_else(|| RuntimeError::plain("Cannot read properties of undefined (reading 'set')"))?;
        for (knob, value) in moved {
            last.insert(key(&knob), value);
        }
        Ok(())
    }
    async fn limiter_at_unity(&self, track: &str, signal: Signal) -> Result<(), RuntimeError> {
        let r = &self.render;
        let read = r.read_knobs(track, signal.clone()).await?;
        if read.devices.last().and_then(|d| d.get("className")).and_then(Value::as_str) != Some("Limiter") {
            return Ok(());
        }
        if let (Some(input), Some(reference)) = (gain(&read), read.devices.last().and_then(|d| d.get("ref")).and_then(Value::as_str)) {
            let unity = if input.min < 0. {
                Some(0.)
            } else if input.min == 0. && input.max == 1. {
                Some(0.5)
            } else {
                None
            };
            if let Some(unity) = unity {
                r.history
                    .quietly(
                        None,
                        r.step(
                            "set_device_parameters",
                            json!({"deviceRef":reference,"values":[{"parameterRef":input.r#ref,"value":unity}]}),
                            signal,
                        ),
                    )
                    .await?;
            }
        }
        Ok(())
    }
}
#[async_trait(?Send)]
impl GoalRig for Goal {
    fn slots(&self) -> Vec<GoalSlotInfo> {
        self.slots.borrow().clone()
    }
    fn screens(&self) -> bool {
        self.screen.is_some()
    }
    async fn add(&self, candidate: &AuditionCandidate, given: Signal) -> Result<Result<GoalSlotInfo, String>, RuntimeError> {
        let signal = self.signal(given);
        let r = &self.render;
        let result: Result<Result<GoalSlotInfo, String>, RuntimeError> = async {
            let tracks = r.rows("track", json!({"fields":["name"]}), signal.clone()).await?;
            let track = tracks.iter().find(|track| track.get("ref").and_then(Value::as_str) == Some(&candidate.track));
            let Some(name) = track.and_then(|track| track.get("name")).and_then(Value::as_str) else {
                return Ok(Err(format!("{} isn't a track in this turn's discovery.", candidate.track)));
            };
            if self.slots.borrow().iter().any(|slot| slot.name == name) {
                return Ok(Err(format!("“{name}” is already in the search.")));
            }
            let label = candidate.label.as_deref().unwrap_or(name);
            r.add_to_rig(
                &mut *self.rig.lock().await,
                Source {
                    track: candidate.track.clone(),
                    name: name.into(),
                    label: label.into(),
                    scratch: String::new(),
                    clip: candidate.clip.clone().filter(|s| !s.is_empty()),
                    scene: None,
                    mix: false,
                },
                signal.clone(),
            )
            .await?;
            Ok(Ok(self.adopt(name, label, signal.clone()).await?))
        }
        .await;
        match result {
            Ok(value) => Ok(value),
            Err(error) => {
                signal.check()?;
                Ok(Err(error.to_string()))
            }
        }
    }
    async fn generation(
        &self,
        trials: &[GenerationTrial],
        given: Signal,
        options: Option<GenerationOptions>,
    ) -> Result<Generation, RuntimeError> {
        let signal = self.signal(given);
        let r = &self.render;
        let screened = options.and_then(|v| v.screen) == Some(true) && self.screen.is_some();
        let cache_key = |trial: &GenerationTrial| {
            format!(
                "{}|{}|{}",
                trial.slot,
                if screened { "screen" } else { "full" },
                trial.values.iter().map(|value| to_fixed(*value, 4)).collect::<Vec<_>>().join(",")
            )
        };
        if r.rendering.replace(true) {
            return Err(observation("Another render is running."));
        }
        let set_from = now_ms();
        let mut render_from = set_from;
        let mut generation = Generation { screened, ..Default::default() };
        let mut rig = self.rig.lock().await;
        let result: Result<IndexMap<String, Render>, RuntimeError> = async {
            let mut plans = vec![];
            for trial in trials {
                let moved = {
                    let current = self.current.borrow();
                    let last = current
                        .get(&trial.slot)
                        .ok_or_else(|| RuntimeError::plain("Cannot read properties of undefined (reading 'get')"))?;
                    trial
                        .knobs
                        .iter()
                        .enumerate()
                        .map(|(i, k)| (k.clone(), trial.values.get(i).copied().unwrap_or(f64::NAN)))
                        .filter(|(knob, value)| last.get(&key(knob)).is_none_or(|last| (last - value).abs() > 1e-6))
                        .collect::<Vec<_>>()
                };
                if moved.is_empty() {
                    continue;
                }
                let fresh = self.knobs_now(&trial.slot, signal.clone()).await?;
                let devices = by_device(&fresh, &moved);
                plans.push((trial, moved, fresh, devices));
            }
            for (trial, moved, fresh, devices) in plans {
                let mut refused = HashSet::new();
                r.history
                    .quietly(None, async {
                        for (reference, values) in devices {
                            if r.step("set_device_parameters", json!({"deviceRef":reference,"values":values}), signal.clone()).await.is_ok()
                            {
                                continue;
                            }
                            signal.check()?;
                            for value in values {
                                if r.step("set_device_parameters", json!({"deviceRef":reference,"values":[value]}), signal.clone())
                                    .await
                                    .is_err()
                                {
                                    signal.check()?;
                                    if let Some((knob, _)) = moved.iter().find(|(knob, _)| {
                                        fresh
                                            .knobs
                                            .iter()
                                            .find(|fresh| key(fresh) == key(knob))
                                            .is_some_and(|fresh| Some(fresh.r#ref.as_str()) == value["parameterRef"].as_str())
                                    }) {
                                        refused.insert(key(knob));
                                    }
                                }
                            }
                        }
                        Ok::<_, RuntimeError>(())
                    })
                    .await?;
                let mut current = self.current.borrow_mut();
                let last = current.get_mut(&trial.slot).unwrap();
                for (knob, value) in moved {
                    if !refused.contains(&key(&knob)) {
                        last.insert(key(&knob), value);
                    }
                }
                if !refused.is_empty() {
                    generation.frozen.insert(trial.slot.clone(), refused);
                }
            }
            render_from = now_ms();
            if !trials.is_empty()
                && trials.iter().all(|trial| trial.fresh != Some(true) && self.heard.borrow().contains_key(&cache_key(trial)))
            {
                Ok(IndexMap::new())
            } else {
                rig.window =
                    if screened { self.screen.as_ref().map(|screen| Window { from: screen.from, beats: screen.beats }) } else { None };
                r.render_pass(&mut rig, signal.clone()).await
            }
        }
        .await;
        r.set_rendering(false);
        let files = result?;
        let heard_from = now_ms();
        let tempo = r.observer.tempo.get().unwrap_or(f64::NAN);
        let against = if screened { &self.screen.as_ref().unwrap().reference } else { &self.reference };
        let beats = if screened { self.screen.as_ref().unwrap().beats } else { rig.beats };
        drop(rig);
        let generation = RefCell::new(generation);
        eager_all(trials.iter().map(|trial| {
            let signal = signal.clone();
            let generation = &generation;
            let files = &files;
            let cache_key = &cache_key;
            async move {
                let cache_key = cache_key(trial);
                let known = if trial.fresh != Some(true) { self.heard.borrow().get(&cache_key).cloned() } else { None };
                if let Some(known) = known {
                    let mut out = generation.borrow_mut();
                    out.cached += 1;
                    out.scores.insert(trial.slot.clone(), known.score);
                    out.gaps.insert(trial.slot.clone(), known.gaps);
                    if let Some(structural) = known.structural {
                        out.structural.insert(trial.slot.clone(), structural);
                    }
                    return Ok::<_, RuntimeError>(());
                }
                let Some(rendered) = files.get(&trial.slot) else {
                    generation.borrow_mut().silent.push(trial.slot.clone());
                    return Ok(());
                };
                let heard = audio::hear(&rendered.file, heard_options(rendered.start, beats * 60. / tempo, self.focus, signal))
                    .await
                    .map_err(|e| RuntimeError::plain(e.to_string()))?;
                if silent_render(&heard) {
                    generation.borrow_mut().silent.push(trial.slot.clone());
                    return Ok(());
                }
                let close = closeness(&heard, against, self.focus);
                let structural = close.structural.map(|s| StructuralMove { gap: s.gap, r#move: s.r#move });
                let mut out = generation.borrow_mut();
                out.scores.insert(trial.slot.clone(), close.score);
                out.gaps.insert(trial.slot.clone(), close.gaps.clone());
                if let Some(structural) = &structural {
                    out.structural.insert(trial.slot.clone(), structural.clone());
                }
                let mut cache = self.heard.borrow_mut();
                cache.insert(cache_key, Cached { score: close.score, gaps: close.gaps, structural });
                if cache.len() > 2000 {
                    cache.shift_remove_index(0);
                }
                Ok(())
            }
        }))
        .await?;
        if std::env::var("KUMI_TIMING").is_ok_and(|s| !s.is_empty()) {
            eprintln!(
                "[generation · {} trials] set {} ms · render {} ms · hear {} ms",
                trials.len(),
                render_from - set_from,
                heard_from - render_from,
                now_ms() - heard_from
            );
        }
        Ok(generation.into_inner())
    }
    async fn settle(&self, slot: &str, knobs: &[Knob], values: &[f64], given: Signal) -> Result<Option<String>, RuntimeError> {
        let signal = self.signal(given);
        let result = async {
            self.set_slot(slot, knobs, values, signal.clone()).await?;
            self.limiter_at_unity(slot, signal.clone()).await
        }
        .await;
        match result {
            Ok(()) => Ok(None),
            Err(error) => {
                signal.check()?;
                Ok(Some(error.to_string()))
            }
        }
    }
    async fn keep_best(&self, slot: &str, knobs: &[Knob], values: &[f64], given: Signal) -> Result<String, RuntimeError> {
        let signal = self.signal(given);
        let r = &self.render;
        let result: Result<String, RuntimeError> = async {
            self.set_slot(slot, knobs, values, signal.clone()).await?;
            self.known.borrow_mut().clear();
            let previous = std::mem::take(&mut *r.best_steps.borrow_mut());
            r.history
                .quietly(None, async {
                    for id in previous.iter().rev() {
                        let discard =
                            r.history.entries.borrow().get(id).is_some_and(|entry| entry.borrow().record.family == ChangeFamily::Structure);
                        let _ = r.history.undo(id, signal.clone(), discard).await;
                    }
                })
                .await;
            for id in previous {
                r.history.entries.borrow_mut().shift_remove(&id);
            }
            let name = "Kumi · Goal best";
            let mut steps = vec![];
            let result: Result<(), RuntimeError> = r
                .history
                .quietly(Some(&mut steps), async {
                    let before = r.rows("track", json!({"fields":["name"]}), signal.clone()).await?;
                    let at = before
                        .iter()
                        .position(|row| row.get("name").and_then(Value::as_str) == Some(slot))
                        .ok_or_else(|| RuntimeError::plain("Cannot read properties of undefined (reading 'ref')"))?;
                    r.step("change_structure", json!({"action":"duplicate-track","ref":before[at].get("ref")}), signal.clone()).await?;
                    let after = r.rows("track", json!({"fields":["name"]}), signal.clone()).await?;
                    let copy = after
                        .get(at + 1)
                        .and_then(|copy| copy.get("ref"))
                        .and_then(Value::as_str)
                        .ok_or_else(|| observation("The copy didn't appear."))?;
                    let name_new = if after.iter().any(|row| row.get("name").and_then(Value::as_str) == Some(name)) {
                        format!("{name} {}", &uuid::Uuid::new_v4().to_string()[..3])
                    } else {
                        name.into()
                    };
                    r.step("rename", json!({"kind":"track","ref":copy,"name":name_new}), signal.clone()).await?;
                    let _ = r.step("set_mixer", json!({"trackRef":copy,"mute":false}), signal.clone()).await;
                    let tracks = r.rows("track", json!({"fields":["name"]}), signal.clone()).await?;
                    let name_now = tracks
                        .iter()
                        .find(|row| row.get("ref").and_then(Value::as_str) == Some(copy))
                        .and_then(|row| row.get("name"))
                        .filter(|v| !v.is_null())
                        .map(js_string)
                        .unwrap_or_else(|| name.into());
                    let _ = self.limiter_at_unity(&name_now, signal.clone()).await;
                    Ok(())
                })
                .await;
            r.best_steps.borrow_mut().extend(steps);
            result?;
            Ok(name.into())
        }
        .await;
        match result {
            Ok(name) => Ok(name),
            Err(error) => {
                signal.check()?;
                Ok(error.to_string())
            }
        }
    }
    async fn tidy(&self, top: &[String], given: Signal) -> Result<Vec<String>, RuntimeError> {
        let signal = self.signal(given);
        let r = &self.render;
        let mut said = vec![];
        let tracks = r.rows("track", json!({"fields":["name"]}), signal.clone()).await.unwrap_or_default();
        let mut order = self.slots.borrow().clone();
        let index = |name: &str| {
            tracks.iter().position(|row| row.get("name").and_then(Value::as_str) == Some(name)).map(|i| i as i64).unwrap_or(-1)
        };
        order.sort_by_key(|slot| std::cmp::Reverse(index(&slot.name)));
        for slot in order {
            let fresh = r.rows("track", json!({"fields":["name"]}), signal.clone()).await.unwrap_or_else(|_| tracks.clone());
            let Some(reference) = fresh
                .iter()
                .find(|row| row.get("name").and_then(Value::as_str) == Some(&slot.name))
                .and_then(|row| row.get("ref"))
                .and_then(Value::as_str)
            else {
                continue;
            };
            if top.contains(&slot.name) {
                let _ = r.step("set_mixer", json!({"trackRef":reference,"mute":true}), signal.clone()).await;
                continue;
            }
            let made = r.history.entries.borrow().values().rev().find_map(|entry| {
                let record = &entry.borrow().record;
                (record.family == ChangeFamily::Structure
                    && record.state == ChangeState::Applied
                    && record.title.contains(&format!("“{}”", slot.name)))
                .then(|| record.id.clone())
            });
            let undone = match made {
                Some(id) => r.history.undo(&id, signal.clone(), true).await.ok(),
                None => None,
            };
            if undone.is_none_or(|undone| undone.is_error) {
                let _ = r.step("set_mixer", json!({"trackRef":reference,"mute":true}), signal.clone()).await;
                said.push(format!("“{}” stays, muted.", slot.name));
            }
        }
        Ok(said)
    }
    async fn close(&self) -> Result<Vec<String>, RuntimeError> {
        let mut rig = self.rig.lock().await;
        self.render.close_rig(&mut rig).await;
        Ok(rig.notes.clone())
    }
}

fn parse_float(text: &str) -> Option<f64> {
    let text = kumi_common::js::string::trim_start(text);
    let expression = regex::Regex::new(r"^[+-]?(?:Infinity|(?:[0-9]+(?:\.[0-9]*)?|\.[0-9]+)(?:[eE][+-]?[0-9]+)?)").unwrap();
    expression.find(text).and_then(|found| found.as_str().parse().ok())
}
