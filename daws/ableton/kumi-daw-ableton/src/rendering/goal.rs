use super::super::{bridge_version::GOAL_BRIDGE, concurrent::eager_all, connection::NO_CURRENT_LIVE};
use super::rig::{Held, Rig, Source, Window};
use super::*;
use async_trait::async_trait;
use kumi_agent::core::{
    evolve::Knob,
    goal_rig::{check_slots, device_label, knob_key as key, moved as moved_knobs, not_in_the_search, tell_generation, GoalDaw},
};
use kumi_common::js::number::parse;
use kumi_listening::{
    engine::Listening,
    goal::{hear, screen_pick, tell, tell_timing, trial_key, HeardCache, Told},
};
use kumi_media::audio::analyze::Analysis;
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
struct Goal {
    render: Rc<LiveRenderer>,
    rig: tokio::sync::Mutex<Rig>,
    slots: RefCell<Vec<GoalSlotInfo>>,
    current: RefCell<IndexMap<String, IndexMap<String, f64>>>,
    known: RefCell<IndexMap<String, (u64, Knobs)>>,
    /// The Set's tempo as the goal opened: what a render is heard at when a look since can't say what it is now.
    tempo: f64,
    reference: Analysis,
    focus: Option<Focus>,
    screen: Option<Screen>,
    heard: RefCell<HeardCache<Told>>,
}
fn device_name(device: &JsonObject) -> String {
    device
        .get("name")
        .filter(|v| !v.is_null())
        .or_else(|| device.get("className").filter(|v| !v.is_null()))
        .map(js_string)
        .unwrap_or_else(|| "Device".into())
}
/// Who each knob's device is, by the knobs' word for it (`<place>:<name>`), for what Kumi remembers of a search.
fn identities(read: &Knobs) -> std::collections::BTreeMap<String, String> {
    read.devices
        .iter()
        .enumerate()
        .filter_map(|(index, device)| {
            let text = |key: &str| device.get(key).and_then(Value::as_str).unwrap_or("");
            Some((
                device_label(index, &device_name(device)),
                super::probe::identity_of(text("className"), text("classDisplayName"), text("name"))?,
            ))
        })
        .collect()
}
fn gain(read: &Knobs) -> Option<&Knob> {
    let limiter = read.devices.last()?;
    let name = limiter.get("name").filter(|v| !v.is_null()).map(js_string).unwrap_or_else(|| "Limiter".into());
    read.knobs.iter().find(|knob| {
        knob.device == format!("{}:{name}", read.devices.len() - 1)
            && matches!(knob.name.to_ascii_lowercase().as_str(), "gain" | "input" | "input gain")
    })
}

/// The tempo a render is heard at: the Set's as it is now, which Live plays and records it at (REAPER's and Renoise's offline
/// renders keep the tempo a goal opened with); the goal's own when a look can't say what it is.
fn heard_tempo(now: Option<f64>, opened: f64) -> f64 {
    now.filter(|tempo| *tempo > 0.).unwrap_or(opened)
}

fn is_final_limiter(device: &JsonObject) -> bool {
    if device.get("enabled") == Some(&Value::Bool(false)) {
        return false;
    }
    match device.get("className").and_then(Value::as_str) {
        Some("Limiter") => true,
        Some("PluginDevice") if device.get("enabled") == Some(&Value::Bool(true)) => {
            device.get("classDisplayName").and_then(Value::as_str).is_some_and(kumi_library::plugins::inventory::is_limiter_name)
        }
        _ => false,
    }
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
impl LiveRenderer {
    async fn read_knobs(&self, name: &str, signal: Signal) -> Result<Knobs, RuntimeError> {
        let tracks = self.rows("track", json!({"fields":["name"]}), signal.clone()).await?;
        let reference = tracks
            .iter()
            .find(|track| track.get("name").and_then(Value::as_str) == Some(name))
            .and_then(|track| track.get("ref"))
            .and_then(Value::as_str)
            .ok_or_else(|| observation(format!("The track “{name}” is gone.")))?
            .to_owned();
        let devices = self
            .rows("device", json!({"parent":reference,"fields":["name","className","classDisplayName","enabled"]}), signal.clone())
            .await?;
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
                    device: device_label(index, &device_name(device)),
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
        listening: &Listening,
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
        let Some(tempo) = self.observer.tempo.get().filter(|v| *v != 0. && !v.is_nan()) else {
            return Ok(Err("Kumi doesn't know the Set's tempo yet; try again.".into()));
        };
        let signal = abort::any([original, self.connection().lifetime.clone()]);
        let reference = listening.heard_reference(named, request, signal.clone()).await?;
        let mut rig = self.open_rig(&request.candidates, request.from_beat, request.beats, signal.clone()).await?;
        rig.hold = Some(Held::default());
        // The candidates are tried where only Kumi hears them; the best goes on “Kumi · Goal best” once the search
        // pauses or ends (`keep_best`, after the rig is closed).
        for source in &mut rig.sources {
            source.quiet = true;
        }
        self.tell(
            if rig.ears.is_some() {
                "Kumi tries the goal's candidates where only it hears them, and puts its best on “Kumi · Goal best” when the search pauses or ends"
            } else {
                "The goal's candidates play as Kumi tries them, and its best goes on “Kumi · Goal best” when the search pauses or ends"
            },
            None,
        );
        let sources: Vec<_> = rig.sources.iter().map(|source| (source.name.clone(), source.label.clone())).collect();
        let mut goal = Goal {
            render: self.clone(),
            rig: tokio::sync::Mutex::new(rig),
            slots: RefCell::new(vec![]),
            current: RefCell::new(IndexMap::new()),
            known: RefCell::new(IndexMap::new()),
            tempo,
            reference,
            focus: request.focus,
            screen: None,
            heard: RefCell::new(HeardCache::default()),
        };
        for (name, label) in sources {
            if let Err(error) = goal.adopt(&name, &label, signal.clone()).await {
                self.close_rig(goal.rig.get_mut()).await;
                return Err(error);
            }
        }
        let rig = goal.rig.get_mut();
        // (Live's slices are as long as the analysis says, to the tenth of a second)
        let slice = parse_float(&goal.reference.over_time.every)
            .filter(|v| *v != 0. && !v.is_nan())
            .unwrap_or(goal.reference.seconds / goal.reference.over_time.lufs.len().max(1) as f64);
        if let Some(pick) = screen_pick(&goal.reference.over_time.lufs, tempo, rig.beats, slice) {
            // A stop or a failed read here leaves the rig as much as one before: it's taken down, as above.
            let reference = match listening.heard_reference(named, &pick.request(request, tempo), signal).await {
                Ok(reference) => reference,
                Err(error) => {
                    self.close_rig(rig).await;
                    return Err(error);
                }
            };
            goal.screen = Some(Screen { from: rig.from + pick.offset, beats: pick.beats, reference });
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
        let limited = read.devices.last().is_some_and(is_final_limiter);
        if !limited {
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
        let devices = identities(&read);
        let slot = GoalSlotInfo { name: name.into(), label: label.into(), chain: read.chain, knobs: read.knobs, devices };
        self.slots.borrow_mut().push(slot.clone());
        Ok(slot)
    }
    async fn set_slot(&self, slot: &str, knobs: &[Knob], values: &[f64], signal: Signal) -> Result<(), RuntimeError> {
        if !self.current.borrow().contains_key(slot) {
            return Err(RuntimeError::plain(not_in_the_search(slot)));
        }
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
        if let Some(last) = self.current.borrow_mut().get_mut(slot) {
            for (knob, value) in moved {
                last.insert(key(&knob), value);
            }
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
                    quiet: true,
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
        check_slots(trials, |slot| self.current.borrow().contains_key(slot))?;
        let signal = self.signal(given);
        let r = &self.render;
        let screened = options.and_then(|v| v.screen) == Some(true) && self.screen.is_some();
        let cache_key = |trial: &GenerationTrial| trial_key(&trial.slot, screened, &trial.values);
        if r.is_rendering() {
            return Err(observation("Another render is running."));
        }
        let rendering = r.rendering_now();
        let set_from = now_ms();
        let mut render_from = set_from;
        let mut generation = Generation { screened, ..Default::default() };
        let mut rig = self.rig.lock().await;
        let result: Result<IndexMap<String, Render>, RuntimeError> = async {
            let mut plans = vec![];
            for trial in trials {
                // (a value a trial doesn't give is sent as it is, as Live's goal has always sent it)
                let moved = moved_knobs(trial, self.current.borrow().get(&trial.slot), false);
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
            if !trials.is_empty() && trials.iter().all(|trial| trial.fresh != Some(true) && self.heard.borrow().contains(&cache_key(trial)))
            {
                Ok(IndexMap::new())
            } else {
                rig.window =
                    if screened { self.screen.as_ref().map(|screen| Window { from: screen.from, beats: screen.beats }) } else { None };
                r.render_pass(&mut rig, signal.clone()).await
            }
        }
        .await;
        drop(rendering);
        let files = result?;
        let heard_from = now_ms();
        let tempo = heard_tempo(r.observer.tempo.get(), self.tempo);
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
                let known = if trial.fresh != Some(true) { self.heard.borrow().get(&cache_key) } else { None };
                if let Some(known) = known {
                    tell_generation(&mut generation.borrow_mut(), &trial.slot, known, true);
                    return Ok::<_, RuntimeError>(());
                }
                let Some(rendered) = files.get(&trial.slot) else {
                    generation.borrow_mut().silent.push(trial.slot.clone());
                    return Ok(());
                };
                let heard = hear(rendered, beats, tempo, self.focus, signal).await.map_err(|e| RuntimeError::plain(e.to_string()))?;
                // (each slot is told as its hearing ends, as Live's goal has always told them)
                match tell(&heard, against, self.focus) {
                    None => generation.borrow_mut().silent.push(trial.slot.clone()),
                    Some(told) => {
                        tell_generation(&mut generation.borrow_mut(), &trial.slot, told.clone(), false);
                        self.heard.borrow_mut().insert(cache_key, told);
                    }
                }
                Ok(())
            }
        }))
        .await?;
        tell_timing(trials.len(), render_from - set_from, heard_from - render_from, now_ms() - heard_from);
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
                        r.take_back(id, signal.clone(), discard).await;
                    }
                })
                .await;
            let name = "Kumi · Goal best";
            let mut steps = vec![];
            let result: Result<String, RuntimeError> = r
                .history
                .quietly(Some(&mut steps), async {
                    let before = r.rows("track", json!({"fields":["name"]}), signal.clone()).await?;
                    let at = before
                        .iter()
                        .position(|row| row.get("name").and_then(Value::as_str) == Some(slot))
                        .ok_or_else(|| observation(format!("The track “{slot}” is gone.")))?;
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
                        .unwrap_or_else(|| name_new.clone());
                    let _ = self.limiter_at_unity(&name_now, signal.clone()).await;
                    // (the name it was given, which the goal says: an older best may hold the plain one)
                    Ok(name_new)
                })
                .await;
            r.best_steps.borrow_mut().extend(steps);
            result
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
        let tidy = LiveTidy { render: &self.render, signal: self.signal(given), tracks: RefCell::new(vec![]) };
        let slots = self.slots.borrow().iter().map(|slot| slot.name.clone()).collect();
        Ok(kumi_agent::core::goal_rig::tidy(&tidy, slots, top).await)
    }
    async fn close(&self) -> Result<Vec<String>, RuntimeError> {
        let mut rig = self.rig.lock().await;
        self.render.close_rig(&mut rig).await;
        Ok(rig.notes.clone())
    }
}

/// A finished goal's candidates tidied in Live: each found again by its name in the Set's tracks as they are now, a
/// candidate the model made (its Structure change) taken back.
struct LiveTidy<'a> {
    render: &'a LiveRenderer,
    signal: Signal,
    /// The Set's tracks as tidying began: where a slot is found when a later read fails.
    tracks: RefCell<Vec<JsonObject>>,
}

#[async_trait(?Send)]
impl GoalDaw for LiveTidy<'_> {
    async fn track_names(&self) -> Option<Vec<String>> {
        let tracks = self.render.rows("track", json!({"fields":["name"]}), self.signal.clone()).await.ok()?;
        let names = tracks.iter().filter_map(|row| row.get("name").and_then(Value::as_str).map(str::to_owned)).collect();
        *self.tracks.borrow_mut() = tracks;
        Some(names)
    }
    async fn track_of(&self, slot: &str) -> Option<String> {
        let fresh = match self.render.rows("track", json!({"fields":["name"]}), self.signal.clone()).await {
            Ok(fresh) => fresh,
            Err(_) => self.tracks.borrow().clone(),
        };
        fresh
            .iter()
            .find(|row| row.get("name").and_then(Value::as_str) == Some(slot))
            .and_then(|row| row.get("ref"))
            .and_then(Value::as_str)
            .map(str::to_owned)
    }
    async fn mute(&self, track: &str) {
        let _ = self.render.step("set_mixer", json!({"trackRef":track,"mute":true}), self.signal.clone()).await;
    }
    fn made_by_kumi(&self, slot: &str) -> Option<String> {
        self.render.history.entries.borrow().values().rev().find_map(|entry| {
            let record = &entry.borrow().record;
            (record.family == ChangeFamily::Structure
                && record.state == ChangeState::Applied
                && record.title.contains(&format!("“{slot}”")))
            .then(|| record.id.clone())
        })
    }
    async fn take_back(&self, id: &str) -> bool {
        self.render.history.undo(id, self.signal.clone(), true).await.is_ok_and(|undone| !undone.is_error)
    }
}

fn parse_float(text: &str) -> Option<f64> {
    let text = kumi_common::js::string::trim_start(text);
    let expression = regex::Regex::new(r"^[+-]?(?:Infinity|(?:[0-9]+(?:\.[0-9]*)?|\.[0-9]+)(?:[eE][+-]?[0-9]+)?)").unwrap();
    expression.find(text).and_then(|found| found.as_str().parse().ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_render_is_heard_at_the_sets_tempo_now_and_the_goals_when_a_look_cant_say() {
        assert_eq!(heard_tempo(Some(90.), 120.), 90., "a tempo that moved wins");
        for lost in [None, Some(0.), Some(f64::NAN)] {
            assert_eq!(heard_tempo(lost, 120.), 120., "{lost:?}");
        }
    }

    #[test]
    fn a_renamed_device_cannot_impersonate_an_enabled_plugin_limiter() {
        let mut device =
            object(json!({"className":"PluginDevice","classDisplayName":"Serum 2","name":"Ozone 12 Maximizer","enabled":true}));
        assert!(!is_final_limiter(&device));
        device.insert("classDisplayName".into(), json!("Ozone 12 Maximizer"));
        device.insert("name".into(), json!("My master ceiling"));
        assert!(is_final_limiter(&device));
        device.insert("enabled".into(), json!(false));
        assert!(!is_final_limiter(&device));
        device.remove("enabled");
        assert!(!is_final_limiter(&device));
        device.insert("enabled".into(), json!(true));
        device.insert("classDisplayName".into(), json!("Ozone 12"));
        assert!(!is_final_limiter(&device));
        device.insert("className".into(), json!("Eq8"));
        device.insert("classDisplayName".into(), json!("Ozone 12 Maximizer"));
        assert!(!is_final_limiter(&device));
    }
}
