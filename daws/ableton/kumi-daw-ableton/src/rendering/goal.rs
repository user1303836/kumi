use super::super::{bridge_version::GOAL_BRIDGE, connection::NO_CURRENT_LIVE};
use super::ears::object_identity;
use super::rig::{Held, Rig, Source, Window};
use super::*;
use async_trait::async_trait;
use kumi_agent::core::{
    evolve::Knob,
    goal_rig::{
        best_plays_beside, best_waits_muted, device_label, device_of, given_values, knob_key as key, last_best_beside, limiter_stays,
        moved_or_went, near, not_in_the_search, you_turned, GoalBackend, GoalCore, GoalDaw, GoalPass, GoalScreen as Screen, HearOrder,
        SetOrder,
    },
};
use kumi_listening::{
    engine::Listening,
    goal::{screen_pick, Told},
};
use std::collections::{HashMap, HashSet};
/// What a goal has done to a candidate's own track. It's kept on the renderer, because a leap closes the goal's rig and opens
/// another, which reads the search's values and Kumi's limiter as if they'd always been there.
#[derive(Default)]
pub(super) struct Candidate {
    /// Each safety limiter Kumi put on its chain, turned down for the search: Live's identity for it, empty where Live gives
    /// none (it's then the last device).
    limiters: Vec<String>,
    /// A match's winner, left at its values.
    settled: bool,
    /// Each knob the search moved, by the words the search has for it.
    moved: IndexMap<String, Moved>,
}
struct Moved {
    knob: Knob,
    /// Where it was before the search first moved it.
    from: f64,
    /// Where the search last left it.
    to: f64,
}
/// The last best as it was kept. Live's undo of a duplicated track deletes it whatever happened to it since, so Kumi reads the
/// whole track before taking it back, and only one it can show is unchanged goes.
#[derive(Clone)]
pub(super) struct KeptBest {
    name: String,
    /// What `BEST_PRINT` read of it as it was kept, the last thing Kumi did to it; None when that couldn't be read.
    print: Option<Value>,
}
/// Reads a track whole, in one go on Live's thread: its devices (inside racks too), clips, notes, a Session clip's automation and
/// its mixer, each as a digest. None when no track has the name.
const BEST_PRINT: &str = include_str!("../assets/best-print.py");
/// What's to be done with the last best.
enum LastBest {
    /// It's as Kumi kept it, or there isn't one.
    Free,
    /// No track has its name now: deleted, or renamed and so the producer's. It's left without a word.
    Gone,
    /// Kumi can't show it's unchanged: it stays, and why.
    Stays(String, String),
}
/// What the print's parts are, as said of a best that changed.
fn what_changed(kept: &Value, now: &Value) -> String {
    let parts: Vec<&str> = [("devices", "its devices"), ("clips", "its clips"), ("mixer", "its mixer")]
        .into_iter()
        .filter(|(part, _)| kept[part] != now[part])
        .map(|(_, said)| said)
        .collect();
    if parts.is_empty() {
        "it changed since Kumi kept it".into()
    } else {
        format!("{} changed since Kumi kept it", parts.join(" and "))
    }
}
/// What came of putting one candidate back.
#[derive(Default)]
struct Back {
    /// The knobs the producer turned since the search left them, which stay.
    turned: Vec<String>,
    /// The knobs Live wouldn't take back.
    refused: Vec<String>,
    /// A renamed or missing track was found only at its old position, and Live gave no device identity for these knobs.
    unidentified: Vec<String>,
    /// The same uncertainty for a safety limiter recorded without a device identity.
    unidentified_limiter: bool,
}
#[derive(Clone)]
struct Knobs {
    reference: String,
    chain: String,
    devices: Vec<JsonObject>,
    knobs: Vec<Knob>,
}
struct Goal {
    render: Rc<LiveRenderer>,
    rig: tokio::sync::Mutex<Rig>,
    slots: RefCell<Vec<GoalSlotInfo>>,
    core: GoalCore<LiveGoal>,
    known: RefCell<IndexMap<String, (u64, Knobs)>>,
    /// Each slot's track as last read: where it stays while another track shares its name.
    places: RefCell<HashMap<String, String>>,
    /// The Set's tempo as the goal opened: what a render is heard at when a look since can't say what it is now.
    tempo: f64,
    /// The safety limiters this rig added, each with its candidate: they come off if the goal never opens.
    limiters_added: RefCell<Vec<(String, String)>>,
    /// The finished goal's candidates were tidied (the top ones muted for an A/B): its best plays on its own.
    tidied: Cell<bool>,
    /// Why the best just kept is beside the last one, when it is.
    beside: RefCell<Option<String>>,
}

struct LiveGoal;
impl GoalBackend for LiveGoal {
    type Cached = Told;
    const SET_ORDER: SetOrder = SetOrder::ReadAllThenSet;
    const HEAR_ORDER: HearOrder = HearOrder::AsCompleted;
    fn cached_told(told: Told) -> Told {
        told
    }
    fn cache(told: Told, _: &Render) -> Told {
        told
    }
}

struct LivePlan {
    fresh: Knobs,
    devices: IndexMap<String, Vec<Value>>,
    lost: Vec<Knob>,
}

/// The held native rig belongs to this pass; the core never owns taps, clips, device refs or restore records.
struct LivePass<'a> {
    goal: &'a Goal,
    rendering: Option<RenderingNow<'a>>,
    rig: Option<tokio::sync::MutexGuard<'a, Rig>>,
}

#[async_trait(?Send)]
impl GoalPass<LiveGoal> for LivePass<'_> {
    type Plan = LivePlan;
    fn signal(&self, given: Signal) -> Signal {
        self.goal.signal(given)
    }
    fn begin(&mut self) -> Result<(), RuntimeError> {
        if self.goal.render.is_rendering() {
            return Err(observation("Another render is running."));
        }
        self.rendering = Some(self.goal.render.rendering_now());
        Ok(())
    }
    async fn before_set(&mut self, signal: Signal) -> Result<Vec<String>, RuntimeError> {
        let mut rig = tokio::select! {
            biased;
            _ = signal.cancelled() => return Err(RuntimeError::Aborted),
            rig = self.goal.rig.lock() => rig,
        };
        signal.check()?;
        let gone = self.goal.retire_gone(&mut rig, signal).await?;
        self.rig = Some(rig);
        Ok(gone)
    }
    async fn before_render(&mut self, signal: Signal) -> Result<Vec<String>, RuntimeError> {
        self.goal.retire_gone(self.rig.as_mut().expect("a generation holds its rig"), signal).await
    }
    async fn plan(&mut self, slot: &str, moved: &[(Knob, f64)], signal: Signal) -> Result<Option<LivePlan>, RuntimeError> {
        // A track can go between availability and this read; the check before rendering retires its source.
        let Some(fresh) = self.goal.knobs_now(slot, signal).await? else { return Ok(None) };
        let (devices, lost) = by_device(&fresh, moved);
        Ok(Some(LivePlan { fresh, devices, lost }))
    }
    async fn set(&mut self, slot: &str, moved: &[(Knob, f64)], plan: LivePlan, signal: Signal) -> Result<HashSet<String>, RuntimeError> {
        let LivePlan { fresh, devices, lost } = plan;
        let r = &self.goal.render;
        let mut refused: HashSet<String> = lost.iter().map(key).collect();
        r.history
            .quietly(None, async {
                for (reference, values) in devices {
                    if r.step("set_device_parameters", json!({"deviceRef":reference,"values":values}), signal.clone()).await.is_ok() {
                        continue;
                    }
                    signal.check()?;
                    for value in values {
                        if r.step("set_device_parameters", json!({"deviceRef":reference,"values":[value]}), signal.clone()).await.is_err() {
                            signal.check()?;
                            if let Some((knob, _)) = moved.iter().find(|(knob, _)| {
                                knob_now(&fresh, knob).is_some_and(|(_, now)| Some(now.r#ref.as_str()) == value["parameterRef"].as_str())
                            }) {
                                refused.insert(key(knob));
                            }
                        }
                    }
                }
                Ok::<_, RuntimeError>(())
            })
            .await?;
        for (knob, value) in moved.iter().filter(|(knob, _)| !refused.contains(&key(knob))) {
            let was = knob_now(&fresh, knob).map_or(knob.value, |(_, now)| now.value);
            self.goal.remember_candidate(slot, knob, *value, was);
        }
        Ok(refused)
    }
    async fn render(
        &mut self,
        _fresh: &[&GenerationTrial],
        screen: Option<&Screen>,
        signal: Signal,
    ) -> Result<IndexMap<String, Render>, RuntimeError> {
        // A fresh trial renders the whole held rig, including other candidates, exactly as the native pass did.
        let rig = self.rig.as_mut().expect("a generation holds its rig");
        rig.window = screen.map(|screen| Window { from: rig.from + screen.offset, beats: screen.beats });
        self.goal.render.render_pass(rig, signal).await
    }
    fn rendered(&mut self) {
        self.rendering.take();
    }
    fn hearing_window(&mut self, screen: Option<&Screen>) -> (f64, f64) {
        let rig = self.rig.take().expect("a generation holds its rig");
        let tempo = heard_tempo(self.goal.render.observer.tempo.get(), self.goal.tempo);
        let beats = screen.map_or(rig.beats, |screen| screen.beats);
        drop(rig);
        (beats, tempo)
    }
}
pub(super) fn left_search(name: &str) -> String {
    format!("“{name}” is no longer available under that name, so it left the search.")
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
/// A limiter's input, on the device at this place.
fn gain_at(read: &Knobs, at: usize) -> Option<&Knob> {
    let device = read.devices.get(at)?;
    let label = device_label(at, &device_name(device));
    read.knobs
        .iter()
        .find(|knob| knob.device == label && matches!(knob.name.to_ascii_lowercase().as_str(), "gain" | "input" | "input gain"))
}
fn is_stock_limiter(device: &JsonObject) -> bool {
    device.get("className").and_then(Value::as_str) == Some("Limiter")
}
/// Where Kumi's limiters are on a chain as read: by Live's identity for each, wherever it is now, or the last device where Live
/// gave none. In chain order.
fn limiter_places(devices: &[JsonObject], limiters: &[String]) -> Vec<usize> {
    let mut places: Vec<usize> = limiters
        .iter()
        .filter_map(|identity| {
            if identity.is_empty() {
                devices.len().checked_sub(1).filter(|last| is_stock_limiter(&devices[*last]))
            } else {
                devices.iter().position(|device| object_identity(device) == *identity)
            }
        })
        .collect();
    places.sort_unstable();
    places.dedup();
    places
}

/// Where a slot's track is now, by #700's rule for a ref held across changes: at its ref if that still has its name, else the
/// only track of its name; several are told, never the first. None when no track has its name.
fn track_now(tracks: &[JsonObject], name: &str, held: Option<&str>) -> Result<Option<String>, RuntimeError> {
    let named: Vec<&str> = tracks
        .iter()
        .filter(|track| track.get("name").and_then(Value::as_str) == Some(name))
        .filter_map(|track| track.get("ref").and_then(Value::as_str))
        .collect();
    match named.as_slice() {
        [] => Ok(None),
        [only] => Ok(Some((*only).to_owned())),
        several => match held.filter(|held| several.contains(held)) {
            Some(held) => Ok(Some(held.to_owned())),
            None => Err(observation(format!("Two tracks are named “{name}”; rename one so Kumi can render it."))),
        },
    }
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
/// A knob as the chain has it now, with its device's ref. Live's refs are places, and a device put in above shifts them, so a
/// knob goes by who its device is, wherever that is now, and is None if it went: a device of the same name is another device.
/// Where Live didn't say who it is, it's on its device at its place under the same name, else on the one device of that name;
/// None when it went, or when several could be it.
fn knob_now<'a>(read: &'a Knobs, knob: &Knob) -> Option<(&'a str, &'a Knob)> {
    let place = match &knob.device_id {
        Some(identity) => read.devices.iter().position(|device| object_identity(device) == *identity)?,
        None => {
            let (at, name) = knob.device.split_once(':')?;
            let named = |at: &usize| read.devices.get(*at).is_some_and(|device| device_name(device) == name);
            at.parse().ok().filter(|at| named(at)).or_else(|| {
                let mut same = (0..read.devices.len()).filter(|at| named(at));
                same.next().filter(|_| same.next().is_none())
            })?
        }
    };
    let label = device_label(place, &device_name(&read.devices[place]));
    let now = read.knobs.iter().find(|now| now.device == label && now.name == knob.name)?;
    Some((read.devices[place].get("ref")?.as_str()?, now))
}
/// Knobs at their values, by the device each is on now; and the ones that moved or went, or whose parameter another knob
/// already took (of two devices of one name with one gone, either could be the one that's left).
fn by_device(read: &Knobs, moved: &[(Knob, f64)]) -> (IndexMap<String, Vec<Value>>, Vec<Knob>) {
    let mut devices: IndexMap<String, Vec<Value>> = IndexMap::new();
    let mut lost = vec![];
    let mut taken = HashSet::new();
    for (knob, value) in moved {
        match knob_now(read, knob).filter(|(_, now)| taken.insert(now.r#ref.clone())) {
            Some((reference, now)) => devices.entry(reference.into()).or_default().push(json!({"parameterRef":now.r#ref,"value":value})),
            None => lost.push(knob.clone()),
        }
    }
    (devices, lost)
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
        let devices = self.device_rows(&reference, signal.clone()).await?;
        self.knobs_on(reference, devices, signal).await
    }
    /// The devices on a track, and who each is.
    async fn device_rows(&self, reference: &str, signal: Signal) -> Result<Vec<JsonObject>, RuntimeError> {
        self.rows("device", json!({"parent":reference,"fields":["name","className","classDisplayName","enabled","objectIdentity"]}), signal)
            .await
    }
    /// The knobs of these devices, as a track at `reference` has them now.
    async fn knobs_on(&self, reference: String, devices: Vec<JsonObject>, signal: Signal) -> Result<Knobs, RuntimeError> {
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
                    device_id: Some(object_identity(device)).filter(|identity| !identity.is_empty()),
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
        // Saved goals name their candidates. A deleted or renamed candidate leaves the search; the others can resume.
        // `open_rig` itself stays strict for an ordinary audition's freshly discovered tracks.
        let mut rig = self.open_rig_with(&request.candidates, request.from_beat, request.beats, false, true, signal.clone()).await?;
        // (hearing a long reference takes a while: the goal and its rig go by the one tempo the rig opened with)
        let tempo = *rig.tempo.get_or_insert(tempo);
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
            core: GoalCore::new(reference, request.focus),
            known: RefCell::new(IndexMap::new()),
            places: RefCell::default(),
            tempo,
            limiters_added: RefCell::new(vec![]),
            tidied: Cell::new(false),
            beside: RefCell::new(None),
        };
        for (name, label) in sources {
            if let Err(error) = goal.adopt(&name, &label, signal.clone()).await {
                self.close_rig(goal.rig.get_mut()).await;
                goal.abandon().await;
                return Err(error);
            }
        }
        let rig = goal.rig.get_mut();
        if let Some(pick) = screen_pick(&goal.core.reference.over_time.lufs, goal.core.reference.seconds, tempo, rig.beats) {
            // A stop or a failed read here leaves the rig as much as one before: it's taken down, as above.
            let reference = match listening.heard_reference(named, &pick.request(request, tempo), signal).await {
                Ok(reference) => reference,
                Err(error) => {
                    self.close_rig(rig).await;
                    goal.abandon().await;
                    return Err(error);
                }
            };
            goal.core.screen = Some(Screen { offset: pick.offset, beats: pick.beats, reference });
        }
        Ok(Ok(Rc::new(goal)))
    }
}
impl Goal {
    fn signal(&self, given: Signal) -> Signal {
        abort::any([given, self.render.connection().lifetime.clone()])
    }
    /// Retires unavailable candidates from both the search and its actual listening rig. Closing before reopening uses
    /// the rig's own undo records for its taps, copied clips and recording tracks; it never deletes a producer's track.
    /// Even an unchanged or cached trial must look: otherwise a vanished tap is armed, or a silent recording is scored.
    async fn retire_gone(&self, rig: &mut Rig, signal: Signal) -> Result<Vec<String>, RuntimeError> {
        let tracks = self.render.rows("track", json!({"fields":["name"]}), signal.clone()).await?;
        let mut gone = vec![];
        let mut available = HashMap::new();
        for slot in self.slots.borrow().iter() {
            match track_now(&tracks, &slot.name, self.places.borrow().get(&slot.name).map(String::as_str))? {
                Some(reference) => {
                    available.insert(slot.name.clone(), reference);
                }
                None => gone.push(slot.name.clone()),
            }
        }
        if gone.is_empty() {
            return Ok(gone);
        }
        let candidates: Vec<_> = rig
            .sources
            .iter()
            .filter(|source| !gone.contains(&source.name))
            .map(|source| AuditionCandidate {
                track: available[&source.name].clone(),
                label: Some(source.label.clone()),
                clip: source.clip.clone().or_else(|| rig.clips.then(|| "first".into())),
                mix: None,
            })
            .collect();
        let (from, beats) = (rig.from, rig.beats);
        self.render.close_rig(rig).await;
        let notes = std::mem::take(&mut rig.notes);
        *rig = self.render.open_rig(&candidates, Some(from), Some(beats), signal).await?;
        rig.hold = Some(Held::default());
        rig.notes.extend(notes);
        rig.notes.extend(gone.iter().map(|name| left_search(name)));
        for source in &mut rig.sources {
            source.quiet = true;
        }
        self.slots.borrow_mut().retain(|slot| !gone.contains(&slot.name));
        for name in &gone {
            self.core.current.borrow_mut().shift_remove(name);
            self.known.borrow_mut().shift_remove(name);
        }
        // Keep the old positions for `put_back`: a renamed track can still have the devices Kumi moved, which are
        // restored only by their identities. The slot leaving the search doesn't grant ownership of another track.
        Ok(gone)
    }
    /// A slot's knobs as its track has them now, for a trial to set; None when no track has its name. Where the track is and its
    /// devices are read each time, because a device or a track put in above moves Live's refs, which are places, and changes
    /// neither the lease nor the epoch; the knobs themselves are read again only when those differ from what's known.
    async fn knobs_now(&self, name: &str, signal: Signal) -> Result<Option<Knobs>, RuntimeError> {
        let tracks = self.render.rows("track", json!({"fields":["name"]}), signal.clone()).await?;
        let held = self.places.borrow().get(name).cloned();
        let Some(reference) = track_now(&tracks, name, held.as_deref())? else { return Ok(None) };
        self.places.borrow_mut().insert(name.into(), reference.clone());
        let devices = self.render.device_rows(&reference, signal.clone()).await?;
        let lease = self.render.connection().lease.get();
        if let Some((known, read)) = self.known.borrow().get(name) {
            if *known == lease && read.reference == reference && read.devices == devices {
                return Ok(Some(read.clone()));
            }
        }
        let read = self.render.knobs_on(reference, devices, signal).await?;
        self.known.borrow_mut().insert(name.into(), (self.render.connection().lease.get(), read.clone()));
        Ok(Some(read))
    }
    async fn adopt(&self, name: &str, label: &str, signal: Signal) -> Result<GoalSlotInfo, RuntimeError> {
        let r = &self.render;
        let mut read = r.read_knobs(name, signal.clone()).await?;
        if !read.devices.last().is_some_and(is_final_limiter) {
            r.history
                .quietly(None, r.step("load_device", json!({"itemId":"audio_effects/Limiter","trackRef":read.reference}), signal.clone()))
                .await?;
            read = match self.limiter_down(name, signal).await {
                Ok(read) => read,
                // (a candidate that never joined keeps no safety limiter of the rig's)
                Err(error) => {
                    self.take_off(&[(name.to_owned(), String::new())]).await;
                    return Err(error);
                }
            };
            let identity = read.devices.last().map(object_identity).unwrap_or_default();
            r.goal_candidates.borrow_mut().entry(name.into()).or_default().limiters.push(identity.clone());
            self.limiters_added.borrow_mut().push((name.into(), identity));
        }
        self.core.current.borrow_mut().insert(name.into(), read.knobs.iter().map(|knob| (key(knob), knob.value)).collect());
        self.places.borrow_mut().insert(name.into(), read.reference.clone());
        self.carry_over(name, &read);
        let devices = identities(&read);
        let slot = GoalSlotInfo { name: name.into(), label: label.into(), chain: read.chain, knobs: read.knobs, devices };
        self.slots.borrow_mut().push(slot.clone());
        Ok(slot)
    }
    /// The safety limiter just put on a track turned down for the search: "a hot synth into a limiter at 0 dB is limited all the
    /// time, and that's heard". The chain as it is then.
    async fn limiter_down(&self, name: &str, signal: Signal) -> Result<Knobs, RuntimeError> {
        let r = &self.render;
        let read = r.read_knobs(name, signal.clone()).await?;
        let last = read.devices.len().saturating_sub(1);
        let (Some(input), Some(reference)) = (gain_at(&read, last), read.devices.last().and_then(|d| d.get("ref")).and_then(Value::as_str))
        else {
            return Ok(read);
        };
        let down = if input.min < 0. {
            input.min.max(-12.)
        } else if input.min == 0. && input.max == 1. {
            0.25
        } else {
            return Ok(read);
        };
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
        r.read_knobs(name, signal).await
    }
    /// A goal that never opened: the safety limiters this rig put on its candidates come off again.
    async fn abandon(&self) {
        let added = self.limiters_added.take();
        self.take_off(&added).await;
    }
    /// One safety limiter off the track at this ref, found on its chain as it is now (a device's going retires the refs on its
    /// track). Whether it went.
    async fn limiter_off(&self, reference: &str, identity: &str, signal: Signal) -> Result<bool, RuntimeError> {
        let r = &self.render;
        let devices = r.device_rows(reference, signal.clone()).await?;
        let Some(at) = limiter_places(&devices, &[identity.to_owned()]).pop() else { return Ok(false) };
        let Some(device) = devices[at].get("ref").and_then(Value::as_str) else { return Ok(false) };
        r.history.quietly(None, r.step("delete_device", json!({"ref":device}), signal)).await?;
        Ok(true)
    }
    /// Takes these safety limiters off again; what stays is said.
    async fn take_off(&self, limiters: &[(String, String)]) {
        let r = &self.render;
        let signal = r.cleanup();
        for (name, identity) in limiters.iter().rev() {
            let off: Result<bool, RuntimeError> = async {
                let tracks = r.rows("track", json!({"fields":["name"]}), signal.clone()).await?;
                let held = self.places.borrow().get(name).cloned();
                let Some(reference) = track_now(&tracks, name, held.as_deref())? else { return Ok(false) };
                self.limiter_off(&reference, identity, signal.clone()).await
            }
            .await;
            if !matches!(off, Ok(true)) {
                r.tell(limiter_stays("Limiter", name), None);
            }
            let mut candidates = r.goal_candidates.borrow_mut();
            if let Some(candidate) = candidates.get_mut(name) {
                candidate.limiters.retain(|kept| kept != identity);
                if candidate.moved.is_empty() && candidate.limiters.is_empty() {
                    candidates.shift_remove(name);
                }
            }
        }
    }
    /// A candidate an earlier rig of this goal searched, as its chain is now. A knob the search moved that's gone, or that reads
    /// other than the search left it, was the model's idea or the producer's hand: it's theirs now, and doesn't go back. The rest
    /// keep their words, which a search's knobs keep too however the chain moved, so a trial moves them from where they are.
    fn carry_over(&self, name: &str, read: &Knobs) {
        let mut candidates = self.render.goal_candidates.borrow_mut();
        let Some(candidate) = candidates.get_mut(name) else { return };
        candidate.moved.retain(|_, moved| knob_now(read, &moved.knob).is_some_and(|(_, now)| near(now, now.value, moved.to)));
        candidate.limiters.retain(|identity| identity.is_empty() || read.devices.iter().any(|device| object_identity(device) == *identity));
        if let Some(current) = self.core.current.borrow_mut().get_mut(name) {
            current.extend(candidate.moved.iter().map(|(word, moved)| (word.clone(), moved.to)));
        }
    }
    /// A knob the search just set on a slot: where the next trial moves it from, and what goes back when the goal ends. `was` is
    /// where it was as read just before.
    fn remember(&self, slot: &str, knob: &Knob, value: f64, was: f64) {
        if let Some(last) = self.core.current.borrow_mut().get_mut(slot) {
            last.insert(key(knob), value);
        }
        self.remember_candidate(slot, knob, value, was);
    }
    /// Native restore ownership stays on the renderer; the shared engine records accepted generation values.
    fn remember_candidate(&self, slot: &str, knob: &Knob, value: f64, was: f64) {
        let mut candidates = self.render.goal_candidates.borrow_mut();
        candidates.entry(slot.into()).or_default().moved.entry(key(knob)).and_modify(|moved| moved.to = value).or_insert_with(|| Moved {
            knob: knob.clone(),
            from: was,
            to: value,
        });
    }
    /// Kumi's limiters on a slot, as they are on this read of it.
    fn limiters_on(&self, slot: &str, read: &Knobs) -> Vec<usize> {
        self.render.goal_candidates.borrow().get(slot).map_or(vec![], |candidate| limiter_places(&read.devices, &candidate.limiters))
    }
    /// Knobs set, device by device, quietly.
    async fn set_on_devices(&self, devices: IndexMap<String, Vec<Value>>, signal: Signal) -> Result<(), RuntimeError> {
        self.render
            .history
            .quietly(None, async {
                for (reference, values) in devices {
                    for chunk in values.chunks(64) {
                        self.render.step("set_device_parameters", json!({"deviceRef":reference,"values":chunk}), signal.clone()).await?;
                    }
                }
                Ok(())
            })
            .await
    }
    /// Knobs set back: a batch Live refuses (one greyed-out knob refuses its whole device's) is tried a value at a time. The
    /// parameters Live wouldn't take.
    async fn set_each(&self, devices: IndexMap<String, Vec<Value>>, signal: Signal) -> Result<Vec<String>, RuntimeError> {
        let r = &self.render;
        let mut refused = vec![];
        r.history
            .quietly(None, async {
                for (reference, values) in devices {
                    for chunk in values.chunks(64) {
                        if r.step("set_device_parameters", json!({"deviceRef":reference,"values":chunk}), signal.clone()).await.is_ok() {
                            continue;
                        }
                        signal.check()?;
                        for value in chunk {
                            if r.step("set_device_parameters", json!({"deviceRef":reference,"values":[value]}), signal.clone())
                                .await
                                .is_err()
                            {
                                signal.check()?;
                                refused.push(value["parameterRef"].as_str().unwrap_or_default().to_owned());
                            }
                        }
                    }
                }
                Ok::<_, RuntimeError>(())
            })
            .await?;
        Ok(refused)
    }
    async fn set_slot(&self, slot: &str, knobs: &[Knob], values: &[f64], signal: Signal) -> Result<Knobs, RuntimeError> {
        if !self.core.current.borrow().contains_key(slot) {
            return Err(RuntimeError::plain(not_in_the_search(slot)));
        }
        let fresh = self.render.read_knobs(slot, signal.clone()).await?;
        let moved = given_values(knobs, values);
        let (devices, lost) = by_device(&fresh, &moved);
        if !lost.is_empty() {
            return Err(RuntimeError::plain(moved_or_went(slot, &lost)));
        }
        self.set_on_devices(devices, signal).await?;
        for (knob, value) in moved {
            let was = knob_now(&fresh, &knob).map_or(knob.value, |(_, now)| now.value);
            self.remember(slot, &knob, value, was);
        }
        Ok(fresh)
    }
    /// Kumi's safety limiters at these places on the track as read back at unity: their inputs were turned down for the search.
    async fn limiter_at_unity(&self, read: &Knobs, places: &[usize], signal: Signal) -> Result<(), RuntimeError> {
        let r = &self.render;
        for at in places {
            let (Some(input), Some(reference)) = (gain_at(read, *at), read.devices[*at].get("ref").and_then(Value::as_str)) else {
                continue;
            };
            let unity = if input.min < 0. {
                0.
            } else if input.min == 0. && input.max == 1. {
                0.5
            } else {
                continue;
            };
            r.history
                .quietly(
                    None,
                    r.step(
                        "set_device_parameters",
                        json!({"deviceRef":reference,"values":[{"parameterRef":input.r#ref,"value":unity}]}),
                        signal.clone(),
                    ),
                )
                .await?;
        }
        Ok(())
    }
    /// A track read whole (`BEST_PRINT`); None when no track has its name.
    async fn best_print(&self, name: &str, signal: Signal) -> Result<Option<Value>, RuntimeError> {
        let connection = self.render.connection();
        if !connection.has("live_run_python") {
            return Err(RuntimeError::plain("this bridge can't read a whole track"));
        }
        let code = crate::fast::with_args("best-print", &json!({"track":name}), BEST_PRINT);
        let read = connection.call("live_run_python", object(json!({"code":code,"mode":"exec","timeoutMs":20000})), signal).await?;
        let done = super::super::context::payload(&read)?;
        if done.get("ok") != Some(&Value::Bool(true)) {
            let why = done.get("error").and_then(|error| error.get("message")).and_then(Value::as_str).unwrap_or("Live didn't say why");
            return Err(RuntimeError::plain(why));
        }
        Ok(done.get("result").filter(|result| !result.is_null()).cloned())
    }
    /// What's to be done with the last best: it goes only if Kumi can show it's as Kumi kept it. A failed proof can only add a
    /// track, never lose one.
    async fn last_best(&self, signal: Signal) -> Result<LastBest, RuntimeError> {
        let Some(kept) = self.render.best_kept.borrow().clone() else { return Ok(LastBest::Free) };
        let why = match (self.best_print(&kept.name, signal.clone()).await, &kept.print) {
            (Ok(None), _) => return Ok(LastBest::Gone),
            (Ok(Some(now)), Some(print)) if now == *print => return Ok(LastBest::Free),
            (Ok(Some(now)), Some(print)) => what_changed(print, &now),
            (Ok(Some(_)), None) => "Kumi couldn't read it when it kept it".to_owned(),
            (Err(error), _) => {
                signal.check()?;
                format!("Kumi couldn't read it: {}", error.to_string().trim_end_matches('.'))
            }
        };
        Ok(LastBest::Stays(kept.name, why))
    }
    /// One candidate back as the goal first found it, without Kumi's limiters. What's done leaves `candidate`, so what's left of
    /// it is what a later goal can still put back. With `only_its_own`, the track was found by its old place, not its name, and
    /// only what Live says is this candidate's is touched.
    async fn put_candidate_back(
        &self,
        reference: String,
        candidate: &mut Candidate,
        only_its_own: bool,
        signal: Signal,
    ) -> Result<Back, RuntimeError> {
        let r = &self.render;
        let devices = r.device_rows(&reference, signal.clone()).await?;
        let read = r.knobs_on(reference, devices, signal.clone()).await?;
        let mut said = Back::default();
        let mut back = vec![];
        let mut names = HashMap::new();
        let mut leaving = vec![];
        for (word, moved) in &candidate.moved {
            let name = format!("{} {}", device_of(&moved.knob), moved.knob.name);
            if only_its_own && moved.knob.device_id.is_none() {
                if !near(&moved.knob, moved.from, moved.to) {
                    said.unidentified.push(name);
                }
                leaving.push(word.clone());
                continue;
            }
            // (a knob with no place now can't be put back, and its device going was someone's choice)
            let found = knob_now(&read, &moved.knob);
            let Some((_, now)) = found else {
                leaving.push(word.clone());
                continue;
            };
            if !near(now, now.value, moved.to) {
                said.turned.push(name);
            } else if !near(now, moved.from, moved.to) {
                names.insert(now.r#ref.clone(), (word.clone(), name));
                back.push((moved.knob.clone(), moved.from));
                continue;
            }
            leaving.push(word.clone());
        }
        let (devices, _) = by_device(&read, &back);
        let refused = self.set_each(devices, signal.clone()).await?;
        for (reference, (word, name)) in names {
            if refused.contains(&reference) {
                said.refused.push(name);
            } else {
                leaving.push(word);
            }
        }
        said.refused.sort();
        candidate.moved.retain(|word, _| !leaving.contains(word));
        // Last, from the end of the chain. One that won't come off doesn't stay turned down, and it's said.
        said.unidentified_limiter = only_its_own && candidate.limiters.iter().any(String::is_empty);
        let mine: Vec<String> = candidate.limiters.iter().filter(|identity| !only_its_own || !identity.is_empty()).cloned().collect();
        let mut stuck = vec![];
        for identity in mine.iter().rev() {
            match self.limiter_off(&read.reference, identity, signal.clone()).await {
                Ok(true) => {}
                Ok(false) => continue,
                Err(_) => {
                    signal.check()?;
                    stuck.push(identity.clone());
                }
            }
        }
        candidate.limiters = stuck;
        if !candidate.limiters.is_empty() {
            let devices = r.device_rows(&read.reference, signal.clone()).await?;
            let now = r.knobs_on(read.reference.clone(), devices, signal.clone()).await?;
            self.limiter_at_unity(&now, &limiter_places(&now.devices, &candidate.limiters), signal).await?;
        }
        Ok(said)
    }
}
#[async_trait(?Send)]
impl GoalRig for Goal {
    fn slots(&self) -> Vec<GoalSlotInfo> {
        self.slots.borrow().clone()
    }
    fn screens(&self) -> bool {
        self.core.screen.is_some()
    }
    fn kept_only(&self) -> Option<String> {
        self.beside.borrow().clone()
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
        self.core.generation(LivePass { goal: self, rendering: None, rig: None }, trials, given, options).await
    }
    async fn settle(&self, slot: &str, knobs: &[Knob], values: &[f64], given: Signal) -> Result<Option<String>, RuntimeError> {
        let signal = self.signal(given);
        let result = async {
            let fresh = self.set_slot(slot, knobs, values, signal.clone()).await?;
            if !self.limiters_on(slot, &fresh).is_empty() {
                let read = self.render.read_knobs(slot, signal.clone()).await?;
                self.limiter_at_unity(&read, &self.limiters_on(slot, &read), signal.clone()).await?;
            }
            Ok::<_, RuntimeError>(())
        }
        .await;
        match result {
            Ok(()) => {
                self.render.goal_candidates.borrow_mut().entry(slot.into()).or_default().settled = true;
                Ok(None)
            }
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
            let fresh = self.set_slot(slot, knobs, values, signal.clone()).await?;
            self.known.borrow_mut().clear();
            *self.beside.borrow_mut() = None;
            let last = self.last_best(signal.clone()).await?;
            *r.best_kept.borrow_mut() = None;
            let previous = std::mem::take(&mut *r.best_steps.borrow_mut());
            if matches!(last, LastBest::Free) {
                r.history
                    .quietly(None, async {
                        for id in previous.iter().rev() {
                            let discard = r
                                .history
                                .entries
                                .borrow()
                                .get(id)
                                .is_some_and(|entry| entry.borrow().record.family == ChangeFamily::Structure);
                            r.take_back(id, signal.clone(), discard).await;
                        }
                    })
                    .await;
            } else {
                r.history.forget(&previous);
            }
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
                    // A paused goal's candidates weren't tidied: they still play, so the best waits muted beside them rather
                    // than playing the part twice. (A finished goal muted its top ones for an A/B.)
                    let waits = !self.tidied.get();
                    let muted = r.step("set_mixer", json!({"trackRef":copy,"mute":waits}), signal.clone()).await.is_ok();
                    let tracks = r.rows("track", json!({"fields":["name"]}), signal.clone()).await?;
                    let name_now = tracks
                        .iter()
                        .find(|row| row.get("ref").and_then(Value::as_str) == Some(copy))
                        .and_then(|row| row.get("name"))
                        .filter(|v| !v.is_null())
                        .map(js_string)
                        .unwrap_or_else(|| name_new.clone());
                    // (a copy's devices have identities of their own: its limiters are where the candidate's are)
                    let places = self.limiters_on(slot, &fresh);
                    if let Ok(read) = r.read_knobs(&name_now, signal.clone()).await {
                        // Live copies a track a moment behind what was just set on it: a real Live's copy held the last trial's
                        // value, though the track read as the winner's. So the copy is held to what it's the best at.
                        let behind: Vec<(Knob, f64)> = given_values(knobs, values)
                            .into_iter()
                            .filter_map(|(knob, value)| {
                                let (_, now) = knob_now(&fresh, &knob)?;
                                let copied = read.knobs.iter().find(|copied| copied.device == now.device && copied.name == now.name)?;
                                (!near(copied, copied.value, value)).then(|| (copied.clone(), value))
                            })
                            .collect();
                        if !behind.is_empty() {
                            self.set_on_devices(by_device(&read, &behind).0, signal.clone()).await?;
                        }
                        if !places.is_empty() {
                            let _ = self.limiter_at_unity(&read, &places, signal.clone()).await;
                        }
                    }
                    // Read last, with all Kumi does to it done.
                    let print = self.best_print(&name_now, signal.clone()).await.ok().flatten();
                    *r.best_kept.borrow_mut() = Some(KeptBest { name: name_now.clone(), print });
                    if let LastBest::Stays(last, why) = &last {
                        *self.beside.borrow_mut() = Some(last_best_beside(last, why, &name_new));
                    }
                    if waits {
                        r.tell(if muted { best_waits_muted(&name_new, slot) } else { best_plays_beside(&name_new, slot) }, None);
                    }
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
        self.tidied.set(true);
        let tidy = LiveTidy { render: &self.render, signal: self.signal(given), tracks: RefCell::new(vec![]) };
        let slots = self.slots.borrow().iter().map(|slot| slot.name.clone()).collect();
        Ok(kumi_agent::core::goal_rig::tidy(&tidy, slots, top).await)
    }
    async fn close(&self) -> Result<Vec<String>, RuntimeError> {
        let mut rig = self.rig.lock().await;
        self.render.close_rig(&mut rig).await;
        Ok(rig.notes.clone())
    }
    /// Live's search moves the candidates' own tracks, so each goes back as the goal first found it, without Kumi's limiter: a
    /// resumed goal, or a later one, puts on a fresh one turned down, and hears as it did. (REAPER's and Renoise's limiters are
    /// never turned down, and stay.) A settled winner keeps its limiter, at unity. What can't be put back now is kept for a later
    /// goal on the track to finish.
    async fn put_back(&self, given: Signal) -> Vec<String> {
        let signal = self.signal(given);
        let r = &self.render;
        let candidates: IndexMap<String, Candidate> =
            std::mem::take(&mut *r.goal_candidates.borrow_mut()).into_iter().filter(|(_, candidate)| !candidate.settled).collect();
        if candidates.is_empty() {
            return vec![];
        }
        let tracks = match r.rows("track", json!({"fields":["name"]}), signal.clone()).await {
            Ok(tracks) => tracks,
            Err(error) => {
                let names: Vec<String> = candidates.keys().map(|name| format!("“{name}”")).collect();
                *r.goal_candidates.borrow_mut() = candidates;
                return vec![couldnt_put_back(&names.join(", "), &error)];
            }
        };
        let mut said = vec![];
        let mut left = IndexMap::new();
        for (name, mut candidate) in candidates {
            let held = self.places.borrow().get(&name).cloned();
            let (reference, only_its_own) = match track_now(&tracks, &name, held.as_deref()) {
                Ok(Some(reference)) => (reference, false),
                // No track has its name: renamed, at its old place if nothing moved it, or tidied away, which leaves nothing of it
                // there for Live's identities to find.
                Ok(None) => match held.filter(|held| tracks.iter().any(|track| track.get("ref").and_then(Value::as_str) == Some(held))) {
                    Some(held) => (held, true),
                    None => continue,
                },
                Err(error) => {
                    said.push(couldnt_put_back(&format!("“{name}”"), &error));
                    left.insert(name, candidate);
                    continue;
                }
            };
            match self.put_candidate_back(reference, &mut candidate, only_its_own, signal.clone()).await {
                Ok(back) => {
                    if !back.turned.is_empty() {
                        said.push(if !back.unidentified.is_empty() || back.unidentified_limiter {
                            format!("“{name}” could only be put back in part: {}.", you_turned(&name, &back.turned))
                        } else {
                            format!("“{name}” is back as it was, except that {}.", you_turned(&name, &back.turned))
                        });
                    }
                    if !back.refused.is_empty() {
                        said.push(format!(
                            "Live wouldn't take {} on “{name}” back to where the goal found {}: check {}.",
                            back.refused.join(", "),
                            if back.refused.len() == 1 { "it" } else { "them" },
                            if back.refused.len() == 1 { "it" } else { "them" }
                        ));
                    }
                    if !back.unidentified.is_empty() || back.unidentified_limiter {
                        let mut left = vec![];
                        if !back.unidentified.is_empty() {
                            left.push(format!("{} may still have the search's values", back.unidentified.join(", ")));
                        }
                        if back.unidentified_limiter {
                            left.push("Kumi's safety Limiter may still be turned down".to_string());
                        }
                        said.push(format!(
                            "Live didn't identify the devices from “{name}” at its old track position, so {}. Check the former candidate.",
                            left.join("; ")
                        ));
                    }
                    if !candidate.limiters.is_empty() {
                        said.push(limiter_stays("Limiter", &name));
                    }
                }
                Err(error) => said.push(couldnt_put_back(&format!("“{name}”"), &error)),
            }
            if !candidate.moved.is_empty() || !candidate.limiters.is_empty() {
                left.insert(name, candidate);
            }
        }
        r.goal_candidates.borrow_mut().extend(left);
        said
    }
}
fn couldnt_put_back(names: &str, error: &RuntimeError) -> String {
    format!("Kumi couldn't put {names} back as the goal found it ({}): check its knobs.", error.to_string().trim_end_matches('.'))
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
            let entry = entry.borrow();
            (entry.tool.as_deref() == Some("add_tracks_and_scenes")
                && entry.record.state == ChangeState::Applied
                && entry.record.title.contains(&format!("“{slot}”")))
            .then(|| entry.record.id.clone())
        })
    }
    async fn take_back(&self, id: &str) -> bool {
        self.render.history.undo(id, self.signal.clone(), true).await.is_ok_and(|undone| !undone.is_error)
    }
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

    fn chain(names: &[&str]) -> Knobs {
        let device = |at: usize, name: &&str| object(json!({"ref": format!("7:device:0:d{at}"), "name": name}));
        let knob = |at: usize, name: &&str| Knob {
            r#ref: format!("7:parameter:0:{at}:0"),
            device: device_label(at, name),
            name: "Tone".into(),
            value: 0.5,
            min: 0.,
            max: 1.,
            step: None,
            device_id: None,
        };
        Knobs {
            reference: "7:track:0".into(),
            chain: String::new(),
            devices: names.iter().enumerate().map(|(at, name)| device(at, name)).collect(),
            knobs: names.iter().enumerate().map(|(at, name)| knob(at, name)).collect(),
        }
    }

    #[test]
    fn a_knob_is_found_at_its_place_or_on_the_one_device_of_its_name_once_the_chain_moved() {
        let tone = chain(&["Synth", "EQ"]).knobs[1].clone();
        let found = |names: &[&str]| knob_now(&chain(names), &tone).map(|(device, now)| (device.to_owned(), now.r#ref.clone()));
        let at = |n: usize| Some((format!("7:device:0:d{n}"), format!("7:parameter:0:{n}:0")));
        assert_eq!(found(&["Synth", "EQ"]), at(1));
        assert_eq!(found(&["New", "Synth", "EQ"]), at(2), "a device put in above moved it");
        assert_eq!(found(&["EQ", "EQ", "Synth"]), at(1), "still at its place, under its name");
        assert_eq!(found(&["EQ", "Synth", "EQ"]), None, "moved, and either could be it");
        assert_eq!(found(&["Synth"]), None, "it went");
    }

    #[test]
    fn a_knob_whose_device_went_isnt_sent() {
        let (read, tone) = (chain(&["Synth"]), chain(&["Synth", "EQ"]).knobs.clone());
        let (devices, lost) = by_device(&read, &[(tone[0].clone(), 0.7), (tone[1].clone(), 0.9)]);
        assert_eq!(devices.len(), 1);
        assert_eq!(devices["7:device:0:d0"], [json!({"parameterRef": "7:parameter:0:0:0", "value": 0.7})]);
        assert_eq!(lost, [tone[1].clone()]);
    }

    #[test]
    fn of_two_knobs_that_come_to_one_parameter_the_second_isnt_sent() {
        // Two devices of one name, one of them gone: both knobs find the one that's left.
        let (read, tone) = (chain(&["EQ"]), chain(&["EQ", "EQ"]).knobs.clone());
        let (devices, lost) = by_device(&read, &[(tone[0].clone(), 0.7), (tone[1].clone(), 0.9)]);
        assert_eq!(devices["7:device:0:d0"], [json!({"parameterRef": "7:parameter:0:0:0", "value": 0.7})]);
        assert_eq!(lost, [tone[1].clone()]);
    }

    #[test]
    fn a_slots_track_is_at_its_ref_or_the_only_one_of_its_name_and_several_are_told() {
        let tracks: Vec<JsonObject> = ["Pad", "Bass", "Pad"]
            .iter()
            .enumerate()
            .map(|(at, name)| object(json!({"ref": format!("7:track:{at}"), "name": name})))
            .collect();
        let now = |name: &str, held: Option<&str>| track_now(&tracks, name, held).map_err(|error| error.to_string());
        assert_eq!(now("Bass", Some("7:track:9")), Ok(Some("7:track:1".to_owned())), "the only one of its name");
        assert_eq!(now("Pad", Some("7:track:2")), Ok(Some("7:track:2".to_owned())), "at its ref, with its name");
        assert_eq!(now("Lead", None), Ok(None), "no track has it");
        for held in [Some("7:track:1"), None] {
            assert_eq!(now("Pad", held), Err("Two tracks are named “Pad”; rename one so Kumi can render it.".to_owned()), "{held:?}");
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
