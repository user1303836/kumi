//! A goal's render rig in a DAW whose bridge is Kumi's own (`/goal`, a match run's search, `/loop` with a reference):
//! each candidate track is searched on a scratch copy of it that never reaches the master, every generation's copies
//! rendered offline in one job and scored against the reference, so the producer's tracks stay as they are until the
//! search settles. The best is kept on a track of its own ("Kumi · Goal best") and played out loud: the winner is heard.
use crate::listening::{DeviceRow, OwnRenderer};
use async_trait::async_trait;
use indexmap::IndexMap;
use kumi_agent::core::{
    contracts::*,
    evolve::Knob,
    goal_rig::{check_slots, device_label, knob_key as key, moved, not_in_the_search, tell_generation, GoalDaw},
    refs::RefScheme,
};
use kumi_common::{
    abort::{self, Signal, SignalExt},
    time::now_ms,
};
use kumi_listening::{
    engine::Listening,
    goal::{hear, screen_pick, tell, tell_timing, trial_key, HeardCache, Told},
    renderer::{Renderer, Window},
};
use kumi_media::audio::{analyze::Analysis, matching::Focus};
use kumi_types::contracts::*;
use kumi_types::errors::RuntimeError;
use serde_json::{json, Value};
use std::{
    cell::{Cell, RefCell},
    collections::HashSet,
    rc::Rc,
};

/// The most knobs a slot offers the search: a device's first ones, and in all.
const KNOBS_PER_DEVICE: usize = 128;
const KNOBS_PER_SLOT: usize = 512;
/// The name the best goes by.
const BEST: &str = "Kumi · Goal best";
/// Knobs a search never moves in an own DAW, besides the shared ones: gain a plug-in adds after its sound (a louder trial
/// would score closer), by name (exact) or by a word in it.
const LEAVE_NAMES: [&str; 3] = ["global gain", "adjustment (db)", "output gain"];
const LEAVE_WORDS: [&str; 1] = ["makeup"];

/// One searched candidate: its track, and the scratch copy its trials are set on and heard from.
struct Slot {
    info: GoalSlotInfo,
    /// The candidate track's long ref, and its short one, which the book follows where refs are places (Renoise's: a track
    /// made or deleted above moves it).
    track: String,
    short: String,
    scratch: String,
    copy_name: String,
    /// The stretch its copy is heard over: its own part's, else the first candidate's.
    window: Window,
    /// Each knob's place (its device's in the chain, its own in the device), by its key.
    places: IndexMap<String, (u64, u64)>,
    /// The steps setting its copy left in the DAW's undo (Renoise's: one a set), told when the copy goes.
    steps: u64,
}

/// What a trial scored, kept so the same values aren't heard twice.
#[derive(Clone)]
struct Cached {
    told: Told,
    /// The render it was heard from, to play the winner out loud.
    render: Render,
}

/// A safety limiter this rig put last on a candidate: its track (its ref as the book follows it), the candidate's name,
/// and the limiter's place in the chain and its ref once read (REAPER's FX GUID holds; Renoise's place is checked again).
struct Limiter {
    track: String,
    name: String,
    place: u64,
    device: String,
}

/// A short stretch characteristic of the reference, heard while a search screens: as far into each slot's stretch.
struct Screen {
    offset: f64,
    beats: f64,
    reference: Analysis,
}

pub struct OwnGoalRig<S: RefScheme + 'static> {
    renderer: Rc<OwnRenderer<S>>,
    slots: RefCell<Vec<Slot>>,
    /// Each slot's values as last set on its copy, by knob key.
    current: RefCell<IndexMap<String, IndexMap<String, f64>>>,
    /// The first candidate's stretch (every slot's is this long).
    window: Window,
    /// The request's from_beat: where a candidate added later is heard when it names no part.
    from_given: Option<f64>,
    /// The tempo the windows were measured in, read as the goal opened: a render whose file doesn't say how long it is
    /// is heard over its window at this tempo. (A look meanwhile may read another: Renoise's render leaves the transport
    /// where the song's tempo commands took it, until the goal puts it back.)
    tempo: f64,
    reference: Analysis,
    focus: Option<Focus>,
    screen: Option<Screen>,
    heard: RefCell<HeardCache<Cached>>,
    /// The finished goal's candidates were tidied (the top ones muted for an A/B): its best plays on its own.
    tidied: Cell<bool>,
    /// The safety limiters this rig put on candidates: taken back if the goal never opens (or a candidate never joins), and
    /// only these are put back at unity when a slot settles.
    limiters: RefCell<Vec<Limiter>>,
}

fn text(value: &Value, key: &str) -> Option<String> {
    value.get(key).and_then(Value::as_str).map(str::to_owned)
}

impl<S: RefScheme + 'static> OwnGoalRig<S> {
    /// A goal's rig over these candidates, toward the reference the request names; why not, when it can't be.
    pub async fn open(
        renderer: Rc<OwnRenderer<S>>,
        listening: &Listening,
        request: &AuditionRequest,
        original: Signal,
    ) -> Result<Result<Rc<dyn GoalRig>, String>, RuntimeError> {
        if request.candidates.iter().any(|candidate| candidate.mix == Some(true) || candidate.track == MIX_CANDIDATE) {
            return Ok(Err("A goal searches a track's own devices. For the whole mix, audition it against the reference (candidates [{\"mix\": true}]) and change EQ, compression and levels between rounds.".into()));
        }
        if let Some(why) = renderer.why_not() {
            return Ok(Err(why));
        }
        if !renderer.flags().scratch {
            return Ok(Err(format!("{} can't make the scratch copies a goal searches on just now.", renderer.app())));
        }
        let Some(named) = request.reference.as_deref().filter(|named| !named.is_empty()) else {
            return Ok(Err("A goal needs a reference to reach.".into()));
        };
        let Some(tempo) = renderer.tempo().filter(|tempo| *tempo > 0.) else {
            return Ok(Err(format!("Kumi doesn't know the {}'s tempo yet; try again.", renderer.profile().words.place)));
        };
        let signal = abort::any([original, renderer.lifetime()]);
        let reference = listening.heard_reference(named, request, signal.clone()).await?;
        // Each candidate is heard over its own part (an item, a pattern), as an audition hears it.
        let windows = match renderer.part_windows(&request.candidates, request.from_beat, request.beats, &signal).await? {
            Ok(windows) => windows,
            Err(why) => return Ok(Err(why)),
        };
        let window =
            windows.first().copied().unwrap_or(Window { from: request.from_beat.unwrap_or(0.), beats: request.beats.unwrap_or(8.) });
        let mut rig = OwnGoalRig {
            renderer: renderer.clone(),
            slots: RefCell::new(vec![]),
            current: RefCell::new(IndexMap::new()),
            window,
            from_given: request.from_beat,
            tempo,
            reference,
            focus: request.focus,
            screen: None,
            heard: RefCell::new(HeardCache::default()),
            tidied: Cell::new(false),
            limiters: RefCell::new(vec![]),
        };
        renderer.tell_app(&format!(
            "Kumi tries the goal's candidates on copies only it hears, and puts its best on “{BEST}” when the search pauses or ends"
        ));
        for (candidate, window) in request.candidates.iter().zip(windows) {
            let label = candidate.label.clone().unwrap_or_else(|| candidate.track.clone());
            match rig.adopt(&candidate.track, &label, window, &signal).await {
                Ok(Ok(_)) => {}
                Ok(Err(why)) => {
                    let stays = rig.abandon().await;
                    return Ok(Err(std::iter::once(why).chain(stays).collect::<Vec<_>>().join(" ")));
                }
                Err(error) => {
                    rig.abandon().await;
                    return Err(error);
                }
            }
        }
        // A long part is screened on its most characteristic three seconds while the search is young.
        let slice = rig.reference.seconds / rig.reference.over_time.lufs.len().max(1) as f64;
        if let Some(pick) = screen_pick(&rig.reference, tempo, window.beats, slice) {
            match listening.heard_reference(named, &pick.request(request, tempo), signal).await {
                Ok(reference) => rig.screen = Some(Screen { offset: pick.offset, beats: pick.beats, reference }),
                Err(error) => {
                    rig.abandon().await;
                    return Err(error);
                }
            }
        }
        Ok(Ok(Rc::new(rig)))
    }

    /// A candidate joins: a safety limiter last in its chain (kept quietly, as it's part of the candidate), its knobs
    /// read, and a scratch copy of it made to search on.
    async fn adopt(&self, named: &str, label: &str, window: Window, signal: &Signal) -> Result<Result<GoalSlotInfo, String>, RuntimeError> {
        let r = &self.renderer;
        let (track, name) = match r.resolve(named, signal).await? {
            Ok(found) => found,
            Err(why) => return Ok(Err(why)),
        };
        if self.slots.borrow().iter().any(|slot| slot.info.name == name) {
            return Ok(Err(format!("“{name}” is already in the search.")));
        }
        let mut devices = match r.devices(&track, signal).await? {
            Ok(devices) => devices,
            Err(why) => return Ok(Err(why)),
        };
        let (add, limiter) = r.table.safety_limiter;
        if !devices
            .last()
            .is_some_and(|device| device.on && (device.own_name.contains(limiter) || crate::listening::verified_plugin_limiter(device)))
        {
            let mut input = json!({"ref": track});
            input[r.table.add_field] = add.into();
            // Part of the candidate, as a goal's is in Live: kept quietly, at its input's unity once the search settles.
            let added = r.plan.history.quietly(None, r.change(r.table.add_tool, input, signal.clone())).await;
            if let Err(why) = added {
                return Ok(Err(format!("Kumi couldn't put a safety {limiter} on “{name}”: {why}")));
            }
            // Kept before the chain is read again: a stop meanwhile (Esc) still takes it off, found last on the chain.
            let short = r.link.book.borrow_mut().short_ref(&track);
            let place = devices.last().map_or(0, |device| device.index + 1);
            self.limiters.borrow_mut().push(Limiter { track: short, name: name.clone(), place, device: String::new() });
            devices = match r.devices(&track, signal).await? {
                Ok(devices) => devices,
                Err(why) => return Ok(Err(why)),
            };
            match devices.last().filter(|device| device.own_name.contains(limiter)) {
                Some(added) => {
                    if let Some(kept) = self.limiters.borrow_mut().last_mut() {
                        (kept.place, kept.device) = (added.index, added.reference.clone());
                    }
                }
                // (not there after all: nothing to take off)
                None => drop(self.limiters.borrow_mut().pop()),
            }
            r.tell_app(&format!("Kumi put a safety {limiter} last on “{name}” for the goal's search"));
        }
        let (knobs, places) = match self.knobs_of(&devices, signal).await? {
            Ok(read) => read,
            Err(why) => return Ok(Err(why)),
        };
        let args = json!({"action": "make", "ref": track, "count": 1, "label": "Kumi: goal search", "range": {"fromBeat": window.from, "beats": window.beats}});
        // (asked in the clean-up's own time: a copy made while the producer stopped is dropped, never left behind)
        let made = match r.ask("scratch", args, r.cleanup_signal()).await? {
            Ok(made) => made,
            Err(refused) => return Ok(Err(refused.why)),
        };
        let (Some(scratch), Some(copy)) = (text(&made, "scratchId"), made.pointer("/copies/0").cloned()) else {
            return Ok(Err(format!("{} made no copy of “{name}”", r.app())));
        };
        if signal.is_cancelled() {
            r.drop_make(&scratch, made.pointer("/undo/steps").and_then(Value::as_u64).unwrap_or(0)).await;
            return Err(RuntimeError::Aborted);
        }
        let (Some(copy_ref), Some(copy_name)) = (text(&copy, "ref"), text(&copy, "name")) else {
            return Ok(Err(format!("{} made no copy of “{name}”", r.app())));
        };
        r.keep_copy(copy_name.clone(), &copy_ref);
        r.tell_scratch_steps(made.pointer("/undo/steps").and_then(Value::as_u64).unwrap_or(0));
        for note in crate::listening::notes_of(&made) {
            r.tell_app(&note);
        }
        let chain = devices.iter().map(|device| device.name.clone()).collect::<Vec<_>>().join(" → ");
        // Who each knob's device is, by the knobs' word for it (as `knobs_of` names them), for what Kumi remembers.
        let identities = devices.iter().enumerate().map(|(at, device)| (device_label(at, &device.name), device.identity.clone())).collect();
        let info = GoalSlotInfo {
            name: name.clone(),
            label: label.into(),
            chain: if chain.is_empty() { "empty".into() } else { chain },
            knobs,
            devices: identities,
        };
        self.current.borrow_mut().insert(name.clone(), info.knobs.iter().map(|knob| (key(knob), knob.value)).collect());
        let short = r.link.book.borrow_mut().short_ref(&track);
        self.slots.borrow_mut().push(Slot { info: info.clone(), track, short, scratch, copy_name, window, places, steps: 0 });
        Ok(Ok(info))
    }

    /// A chain's knobs as the search moves them, each with its place (device, parameter) to set it by on a copy.
    async fn knobs_of(
        &self,
        devices: &[DeviceRow],
        signal: &Signal,
    ) -> Result<Result<(Vec<Knob>, IndexMap<String, (u64, u64)>), String>, RuntimeError> {
        let r = &self.renderer;
        let mut knobs = vec![];
        let mut places = IndexMap::new();
        for (at, device) in devices.iter().enumerate() {
            // (Renoise's mixer is the track's fader and pan: the search leaves the producer's mix where it is; a limiter,
            // the safety one Kumi put last included, would only make trials louder)
            if device.mixer || r.table.limiters.iter().any(|(word, _, _)| device.own_name.contains(word)) {
                continue;
            }
            // (a grid of two points: a stepped parameter answers its steps' texts)
            let rows = match r.rows(json!({"kind": "param", "parent": device.reference, "grid": 2}), 16, signal).await? {
                Ok(rows) => rows,
                Err(why) => return Ok(Err(why)),
            };
            // (REAPER lists Bypass, Wet and Delta last on every FX: not the sound's)
            let pseudo = rows
                .iter()
                .rev()
                .take(r.table.pseudo_params.len())
                .filter(|row| text(row, "name").is_some_and(|name| r.table.pseudo_params.contains(&name.as_str())))
                .count();
            for row in rows[..rows.len() - pseudo].iter().take(KNOBS_PER_DEVICE) {
                let (Some(reference), Some(name)) = (text(row, "ref"), text(row, "name")) else { continue };
                let lower = name.to_lowercase();
                if LEAVE_NAMES.contains(&lower.as_str()) || LEAVE_WORDS.iter().any(|word| lower.contains(word)) {
                    continue;
                }
                // (where a device aims, not how it sounds: kept on a copy, it would aim the copy at the producer's tracks)
                if (r.table.aims)(&device.own_name, &name) {
                    continue;
                }
                let number = |key: &str| row.get(key).and_then(Value::as_f64);
                let (min, max) = if r.table.normalized { (0., 1.) } else { (number("min").unwrap_or(0.), number("max").unwrap_or(1.)) };
                let index =
                    row.get("index").and_then(Value::as_u64).or_else(|| reference.rsplit([':', '.']).next()?.parse().ok()).unwrap_or(0);
                let knob = Knob {
                    r#ref: reference,
                    device: device_label(at, &device.name),
                    name,
                    min,
                    max,
                    step: crate::listening::steps_of(row).map(|steps| (max - min) / (steps - 1) as f64),
                    value: number("value").unwrap_or(min),
                };
                places.insert(key(&knob), (device.index, index));
                knobs.push(knob);
                if knobs.len() >= KNOBS_PER_SLOT {
                    return Ok(Ok((knobs, places)));
                }
            }
        }
        Ok(Ok((knobs, places)))
    }

    /// Sets a slot's trial values on its copy: the knobs the DAW refused, by key. A batch it refuses is set again a value
    /// at a time, so a knob it won't take doesn't keep the others from moving (as Live's goal sets them). Err: the slot or
    /// its copy is gone.
    async fn set_copy(&self, slot: &str, moved: &[(Knob, f64)], signal: &Signal) -> Result<Result<HashSet<String>, String>, RuntimeError> {
        let r = &self.renderer;
        let (scratch, copy, places) = {
            let slots = self.slots.borrow();
            let Some(found) = slots.iter().find(|found| found.info.name == slot) else {
                return Ok(Err(not_in_the_search(slot)));
            };
            let Some(copy) = r.copy_ref(&found.copy_name) else { return Ok(Err(format!("“{slot}”'s copy is gone"))) };
            (found.scratch.clone(), copy, found.places.clone())
        };
        let values: Vec<(String, Value)> = moved
            .iter()
            .filter_map(|(knob, value)| {
                let (device, param) = places.get(&key(knob))?;
                let mut set = json!({"ref": copy, "device": device, "param": param});
                set[r.table.scratch_field] = json!(value);
                Some((key(knob), set))
            })
            .collect();
        let mut refused = HashSet::new();
        for chunk in values.chunks(256) {
            if self.set_on_copy(slot, &scratch, chunk.iter().map(|(_, set)| set.clone()).collect(), signal).await? {
                continue;
            }
            // (refused because the search was stopped: nothing is asked again)
            signal.check()?;
            for (key, set) in chunk {
                if !self.set_on_copy(slot, &scratch, vec![set.clone()], signal).await? {
                    refused.insert(key.clone());
                }
            }
        }
        Ok(Ok(refused))
    }

    /// One scratch `set` of these values on a slot's copy, the steps it left in the DAW's undo counted: whether it took them.
    async fn set_on_copy(&self, slot: &str, scratch: &str, values: Vec<Value>, signal: &Signal) -> Result<bool, RuntimeError> {
        match self.renderer.ask("scratch", json!({"action": "set", "scratchId": scratch, "values": values}), signal.clone()).await? {
            Ok(done) => {
                let steps = done.pointer("/undo/steps").and_then(Value::as_u64).unwrap_or(0);
                if let Some(found) = self.slots.borrow_mut().iter_mut().find(|found| found.info.name == slot) {
                    found.steps += steps;
                }
                Ok(true)
            }
            Err(_) => Ok(false),
        }
    }

    /// A goal that never opened: its copies go, and the safety limiters it put on candidates are taken off again, each
    /// found on its track's chain as it is now (Renoise's device refs go stale as soon as a device list changes). What
    /// stays is said.
    async fn abandon(&self) -> Vec<String> {
        self.drop_copies().await;
        let limiters = self.limiters.take();
        self.take_off(limiters).await
    }

    /// Takes these safety limiters off again, each found on its track's chain as it is now (just after it was added: last
    /// there, if not at its place); what stays is said.
    async fn take_off(&self, limiters: Vec<Limiter>) -> Vec<String> {
        let r = &self.renderer;
        let (_, limiter) = r.table.safety_limiter;
        let signal = r.cleanup_signal();
        let mut stays = vec![];
        for kept in limiters {
            let name = kept.name.clone();
            let track = r.link.book.borrow().lengthen(&json!({"ref": kept.track}))["ref"].as_str().unwrap_or(&kept.track).to_owned();
            let found = match r.devices(&track, &signal).await {
                Ok(Ok(chain)) => self
                    .own_limiter(&kept, &chain)
                    .or_else(|| chain.last().filter(|device| device.own_name.contains(limiter)))
                    .map(|device| device.reference.clone()),
                _ => None,
            };
            let gone = match found {
                Some(reference) => {
                    r.plan.history.quietly(None, r.change(r.table.delete_tool, json!({"ref": reference}), signal.clone())).await.is_ok()
                }
                None => false,
            };
            if !gone {
                let note = format!("Kumi's safety {limiter} stays last on “{name}”: delete it if you don't want it there.");
                r.tell_app(&note);
                stays.push(note);
            }
        }
        stays
    }

    /// The safety limiter this rig added, on its track's chain as it is now: by its ref where refs are identities (REAPER's
    /// FX GUIDs), else at its place. None when it isn't there (the producer's own limiter is never taken for it).
    fn own_limiter<'a>(&self, kept: &Limiter, chain: &'a [DeviceRow]) -> Option<&'a DeviceRow> {
        let (_, limiter) = self.renderer.table.safety_limiter;
        let lasting = !self.renderer.link.book.borrow().scheme.positional() && !kept.device.is_empty();
        chain.iter().find(|device| {
            device.own_name.contains(limiter) && if lasting { device.reference == kept.device } else { device.index == kept.place }
        })
    }

    /// A slot's track now: by the ref it had where refs are identities (REAPER's GUIDs: a track inserted above an unnamed
    /// "Track 2" renames it, never its GUID), else as the book has followed it since it joined (a track made or deleted above
    /// moves Renoise's refs); by its name only when the book lost it (another epoch) and one track alone has it. Why not.
    async fn track_now(&self, slot: &str, signal: &Signal) -> Result<String, String> {
        let found =
            self.slots.borrow().iter().find(|found| found.info.name == slot).map(|found| (found.track.clone(), found.short.clone()));
        let Some((stored, short)) = found else { return Err(not_in_the_search(slot)) };
        let followed = {
            let book = self.renderer.link.book.borrow();
            if !book.scheme.positional() {
                return Ok(stored);
            }
            let long = book.lengthen(&json!({"ref": short}))["ref"].as_str().map(str::to_owned);
            long.filter(|long| book.scheme.kind_of(long) == Some("track"))
        };
        if let Some(long) = followed {
            return Ok(long);
        }
        let lost = format!("Kumi lost track of “{slot}” as the tracks moved, and can't tell it by its name: start the goal again");
        let tracks = match self.renderer.track_rows(signal).await {
            Ok(Ok(tracks)) => tracks,
            _ => return Err(lost),
        };
        let mut named = tracks.iter().filter(|track| track.name == slot);
        match (named.next(), named.next()) {
            (Some(only), None) => Ok(only.reference.clone()),
            _ => Err(lost),
        }
    }

    /// The safety limiter this rig put on a settled slot back at unity (its knob at 0 dB), quietly, as it was added. One
    /// the producer had last on the track (the rig added none) is theirs, and stays as they set it.
    async fn limiter_at_unity(&self, slot: &str, track: &str, signal: &Signal) {
        let r = &self.renderer;
        let Ok(Ok(devices)) = r.devices(track, signal).await else { return };
        let device = {
            let limiters = self.limiters.borrow();
            let Some(kept) = limiters.iter().find(|kept| kept.name == slot) else { return };
            match self.own_limiter(kept, &devices) {
                Some(device) => device.clone(),
                None => return,
            }
        };
        let device = &device;
        let Some((_, knob, _)) = r.table.limiters.iter().find(|(word, _, _)| device.own_name.contains(word)) else { return };
        let Ok(Ok(rows)) = r.parameters(&device.reference, false, signal).await else { return };
        let Some(reference) = rows.iter().find(|row| text(row, "name").as_deref() == Some(*knob)).and_then(|row| text(row, "ref")) else {
            return;
        };
        let mut input = json!({"ref": reference});
        input[r.table.text_field] = "0.0 dB".into();
        let _ = r.plan.history.quietly(None, r.change(r.table.param_tool, input, signal.clone())).await;
    }

    /// The scratch copies go, whatever happened.
    async fn drop_copies(&self) {
        let r = &self.renderer;
        let slots: Vec<(String, String, u64)> =
            self.slots.borrow().iter().map(|slot| (slot.scratch.clone(), slot.copy_name.clone(), slot.steps)).collect();
        for (scratch, name, steps) in slots {
            r.drop_make(&scratch, steps).await;
            r.copies.borrow_mut().shift_remove(&name);
        }
    }

    /// The slot's knobs the search moved set on the candidate track itself, as one change in HISTORY (one row, however many
    /// devices): only those whose winner differs from where the search began (none when nothing won: the track stays as it
    /// is), and never one the producer turned on the track since (said, and left as they set it).
    async fn set_track(&self, slot: &str, knobs: &[Knob], values: &[f64], signal: Signal) -> Result<(), String> {
        let near = |knob: &Knob, a: f64, b: f64| (a - b).abs() <= 1e-5 * (knob.max - knob.min).abs().max(1e-9);
        let won: Vec<(&Knob, f64)> =
            knobs.iter().zip(values).filter(|(knob, value)| !near(knob, **value, knob.value)).map(|(knob, value)| (knob, *value)).collect();
        if won.is_empty() {
            return Ok(());
        }
        let track = self.track_now(slot, &signal).await?;
        let chain = match self.renderer.devices(&track, &signal).await {
            Ok(Ok(chain)) => chain,
            Ok(Err(why)) => return Err(why),
            Err(error) => return Err(error.to_string()),
        };
        let places = self.slots.borrow().iter().find(|found| found.info.name == slot).map(|found| found.places.clone()).unwrap_or_default();
        let named = |knob: &Knob| knob.device.split_once(':').map_or(knob.device.as_str(), |(_, name)| name).to_owned();
        // (by device as it is now: one change for each, as a device's knobs are set; nothing when one moved or went)
        let mut by_device: IndexMap<String, Vec<(&str, f64)>> = IndexMap::new();
        let mut moved: Vec<String> = vec![];
        let mut turned: Vec<String> = vec![];
        let mut read: IndexMap<String, Vec<Value>> = IndexMap::new();
        for (knob, value) in won {
            let Some((_, row)) = self.device_now(knob, &chain) else {
                if !moved.contains(&named(knob)) {
                    moved.push(named(knob));
                }
                continue;
            };
            if !read.contains_key(&row.reference) {
                let rows = match self.renderer.parameters(&row.reference, false, &signal).await {
                    Ok(Ok(rows)) => rows,
                    Ok(Err(why)) => return Err(why),
                    Err(error) => return Err(error.to_string()),
                };
                read.insert(row.reference.clone(), rows);
            }
            let param = places.get(&key(knob)).map(|(_, param)| *param);
            let now = read[&row.reference]
                .iter()
                .find(|p| match param {
                    Some(param) => p.get("index").and_then(Value::as_u64) == Some(param),
                    None => text(p, "name").as_deref() == Some(knob.name.as_str()),
                })
                .and_then(|p| p.get("value").and_then(Value::as_f64));
            if now.is_some_and(|now| !near(knob, now, knob.value)) {
                turned.push(format!("{} {}", named(knob), knob.name));
                continue;
            }
            by_device.entry(row.reference.clone()).or_default().push((knob.r#ref.as_str(), value));
        }
        if !moved.is_empty() {
            return Err(self.moved_words(slot, &moved));
        }
        let mark = self.renderer.applied();
        let set: usize = by_device.values().map(Vec::len).sum();
        for (reference, values) in by_device {
            self.renderer.set_knobs(&reference, &values, signal.clone()).await.map_err(|error| error.to_string())?;
        }
        let made: Vec<String> = self.renderer.applied_since(&mark).into_iter().map(|(id, _)| id).collect();
        if made.len() > 1 {
            self.renderer.plan.history.grouped(&format!("“{slot}”: {set} knobs the goal set"), &made, &[]);
        }
        if !turned.is_empty() {
            let them = if turned.len() == 1 { "it" } else { "them" };
            return Err(format!(
                "you turned {} on “{slot}” since the search began, so Kumi left {them} as you set {them}",
                turned.join(", ")
            ));
        }
        Ok(())
    }

    /// A knob's device in the chain as it is now, and its place there: by its id where the DAW's refs are ids (REAPER's FX
    /// GUID), else at its place under the same name (Renoise's refs are places). None when it moved or went.
    fn device_now<'a>(&self, knob: &Knob, chain: &'a [DeviceRow]) -> Option<(usize, &'a DeviceRow)> {
        let lasting = !self.renderer.link.book.borrow().scheme.positional();
        if lasting {
            if let Some(device) = crate::listening::device_of_param(&knob.r#ref) {
                return chain.iter().enumerate().find(|(_, row)| row.reference == device);
            }
        }
        let (at, name) = knob.device.split_once(':')?;
        let at: usize = at.parse().ok()?;
        chain.get(at).filter(|row| row.name == name).map(|row| (at, row))
    }

    /// Why nothing was set: devices the search moved knobs of moved or went meanwhile.
    fn moved_words(&self, slot: &str, moved: &[String]) -> String {
        let named: Vec<String> = moved.iter().map(|name| format!("“{name}”")).collect();
        format!(
            "{} moved or went on “{slot}” since the search began, so Kumi set nothing there: start the goal again to search the chain as it is now",
            named.join(", ")
        )
    }
}

#[async_trait(?Send)]
impl<S: RefScheme + 'static> GoalRig for OwnGoalRig<S> {
    fn slots(&self) -> Vec<GoalSlotInfo> {
        self.slots.borrow().iter().map(|slot| slot.info.clone()).collect()
    }
    fn screens(&self) -> bool {
        self.screen.is_some()
    }
    async fn add(&self, candidate: &AuditionCandidate, given: Signal) -> Result<Result<GoalSlotInfo, String>, RuntimeError> {
        let signal = abort::any([given, self.renderer.lifetime()]);
        let label = candidate.label.clone().unwrap_or_else(|| candidate.track.clone());
        // (over its own part, as long as the others; else where the first candidate is heard)
        let window = match self.renderer.part_of(candidate, self.from_given, &signal).await {
            Ok(Ok(part)) => part.map_or(self.window, |part| Window { from: part.from, beats: self.window.beats }),
            Ok(Err(why)) => return Ok(Err(why)),
            Err(error) if !error.is_aborted() => return Ok(Err(error.to_string())),
            Err(error) => return Err(error),
        };
        let before = self.limiters.borrow().len();
        let adopted = self.adopt(&candidate.track, &label, window, &signal).await;
        // (a candidate that never joined keeps no safety limiter of the rig's: it comes off again, said if it can't)
        if !matches!(adopted, Ok(Ok(_))) {
            let added: Vec<Limiter> = self.limiters.borrow_mut().drain(before..).collect();
            self.take_off(added).await;
        }
        match adopted {
            Err(error) if !error.is_aborted() => Ok(Err(error.to_string())),
            other => other,
        }
    }
    async fn generation(
        &self,
        trials: &[GenerationTrial],
        given: Signal,
        options: Option<GenerationOptions>,
    ) -> Result<Generation, RuntimeError> {
        check_slots(trials, |slot| self.current.borrow().contains_key(slot))?;
        let r = &self.renderer;
        let signal = abort::any([given, r.lifetime()]);
        let screened = options.and_then(|options| options.screen) == Some(true) && self.screen.is_some();
        let cache_key = |trial: &GenerationTrial| trial_key(&trial.slot, screened, &trial.values);
        if r.flag().is_on() {
            return Err(RuntimeError::Observation("Another render is running.".into()));
        }
        let _rendering = r.flag().begin();
        let set_from = now_ms();
        let mut generation = Generation { screened, ..Default::default() };
        // Each trial's values on its slot's copy (only what moved).
        for trial in trials {
            let moved = moved(trial, self.current.borrow().get(&trial.slot), true);
            if moved.is_empty() {
                continue;
            }
            match self.set_copy(&trial.slot, &moved, &signal).await? {
                // (what the DAW took is the copy's now; the knobs it refused, the search leaves be)
                Ok(refused) => {
                    let mut current = self.current.borrow_mut();
                    let last = current.entry(trial.slot.clone()).or_default();
                    for (knob, value) in moved.iter().filter(|(knob, _)| !refused.contains(&key(knob))) {
                        last.insert(key(knob), *value);
                    }
                    if !refused.is_empty() {
                        generation.frozen.insert(trial.slot.clone(), refused);
                    }
                }
                // (the slot or its copy is gone: none of its knobs can move)
                Err(_) => {
                    generation.frozen.insert(trial.slot.clone(), moved.iter().map(|(knob, _)| key(knob)).collect::<HashSet<_>>());
                }
            }
        }
        let render_from = now_ms();
        // Rendered unless every trial was heard before at these values.
        let fresh: Vec<&GenerationTrial> =
            trials.iter().filter(|trial| trial.fresh == Some(true) || !self.heard.borrow().contains(&cache_key(trial))).collect();
        // Each slot's copy heard over its own stretch (or the screen's slice of it): one render for each stretch.
        let window_of = |slot: &Slot| match self.screen.as_ref().filter(|_| screened) {
            Some(screen) => Window { from: slot.window.from + screen.offset, beats: screen.beats },
            None => slot.window,
        };
        let beats = self.screen.as_ref().filter(|_| screened).map_or(self.window.beats, |screen| screen.beats);
        let mut files: IndexMap<String, Render> = IndexMap::new();
        if !fresh.is_empty() {
            // Each source's id with its slot, made together so an id always names its own slot's copy; by stretch.
            let mut groups: Vec<(Window, Vec<Value>, Vec<String>)> = vec![];
            {
                let slots = self.slots.borrow();
                let heard = fresh.iter().filter_map(|trial| slots.iter().find(|slot| slot.info.name == trial.slot));
                for (at, slot) in heard.enumerate() {
                    let window = window_of(slot);
                    let source = json!({"id": format!("g{at}"), "what": "track", "ref": r.copy_ref(&slot.copy_name), "stage": "pre-fader"});
                    match groups.iter_mut().find(|(over, _, _)| (over.from - window.from).abs() < 1e-9) {
                        Some((_, sources, ids)) => {
                            sources.push(source);
                            ids.push(slot.info.name.clone());
                        }
                        None => groups.push((window, vec![source], vec![slot.info.name.clone()])),
                    }
                }
            }
            for (window, sources, ids) in groups {
                let names: Vec<String> = sources.iter().filter_map(|source| source["id"].as_str().map(str::to_owned)).collect();
                match r.render_sources(sources, window, &signal).await? {
                    Ok((rendered, _)) => {
                        for (id, slot) in names.iter().zip(&ids) {
                            if let Some((render, _)) = rendered.get(id) {
                                files.insert(slot.clone(), render.clone());
                            }
                        }
                    }
                    Err(why) => return Err(RuntimeError::Observation(why)),
                }
            }
        }
        let heard_from = now_ms();
        let against = if screened { &self.screen.as_ref().unwrap().reference } else { &self.reference };
        // Each trial heard before is told from what was heard; the rest are heard together, as Live's goal hears them.
        let told: Vec<(&GenerationTrial, String, Option<Cached>)> = trials
            .iter()
            .map(|trial| {
                let cache_key = cache_key(trial);
                let known = if trial.fresh != Some(true) { self.heard.borrow().get(&cache_key) } else { None };
                (trial, cache_key, known)
            })
            .collect();
        let hearing = told
            .iter()
            .filter(|(_, _, known)| known.is_none())
            .filter_map(|(trial, _, _)| files.get(&trial.slot))
            .map(|rendered| hear(rendered, beats, self.tempo, self.focus, signal.clone()));
        let mut heard = futures::future::try_join_all(hearing).await.map_err(|error| RuntimeError::plain(error.to_string()))?.into_iter();
        for (trial, cache_key, known) in told {
            if let Some(known) = known {
                tell_generation(&mut generation, &trial.slot, known.told, true);
                continue;
            }
            let Some(rendered) = files.get(&trial.slot) else {
                generation.silent.push(trial.slot.clone());
                continue;
            };
            let Some(heard) = heard.next() else { break };
            let Some(told) = tell(&heard, against, self.focus) else {
                generation.silent.push(trial.slot.clone());
                continue;
            };
            tell_generation(&mut generation, &trial.slot, told.clone(), false);
            self.heard.borrow_mut().insert(cache_key, Cached { told, render: rendered.clone() });
        }
        tell_timing(trials.len(), render_from - set_from, heard_from - render_from, now_ms() - heard_from);
        Ok(generation)
    }
    async fn keep_best(&self, slot: &str, knobs: &[Knob], values: &[f64], given: Signal) -> Result<String, RuntimeError> {
        let r = &self.renderer;
        let signal = abort::any([given, r.lifetime()]);
        let result: Result<Result<String, String>, RuntimeError> = async {
            if !r.link.daw.changes.kinds.iter().any(|kind| kind.tool == "scratch_keep") {
                return Ok(Err(format!("this {} bridge can't keep a copy as a track yet: update Kumi's bridge", r.app())));
            }
            let track = match self.track_now(slot, &signal).await {
                Ok(track) => track,
                Err(why) => return Ok(Err(why)),
            };
            // Each knob's place in the chain as it is now (the copy is made of the track now): none when its device moved.
            let chain = match r.devices(&track, &signal).await? {
                Ok(chain) => chain,
                Err(why) => return Ok(Err(why)),
            };
            let places =
                self.slots.borrow().iter().find(|found| found.info.name == slot).map(|found| found.places.clone()).unwrap_or_default();
            let mut set_at: Vec<(u64, u64, f64)> = vec![];
            let mut moved: Vec<String> = vec![];
            for (knob, value) in knobs.iter().zip(values) {
                match (self.device_now(knob, &chain), places.get(&key(knob))) {
                    (Some((_, row)), Some((_, param))) => set_at.push((row.index, *param, *value)),
                    _ => moved.push(knob.device.split_once(':').map_or(knob.device.as_str(), |(_, name)| name).to_owned()),
                }
            }
            moved.dedup();
            if !moved.is_empty() {
                return Ok(Err(self.moved_words(slot, &moved)));
            }
            // The last best goes (Kumi's undo of keeping it, whichever goal kept it), then a fresh copy of the slot at these
            // values is kept. One the producer worked on since stays theirs (the bridge refuses the undo): muted again when
            // the mute a paused goal put on it was taken back first, so it doesn't play beside the original.
            let previous = r.goal_best.take();
            let best_track = r.goal_best_track.take();
            let mut unmuted = false;
            for (at, id) in previous.iter().enumerate().rev() {
                if let Err(why) = r.undo(id).await {
                    let why = why.trim_end_matches('.');
                    let muted = match best_track.as_deref().filter(|_| unmuted) {
                        Some(short) => {
                            let long = r.link.book.borrow().lengthen(&json!({"ref": short}))["ref"].as_str().unwrap_or(short).to_owned();
                            r.change("track_mute", json!({"ref": long, "mute": true}), signal.clone()).await.is_ok()
                        }
                        None => !unmuted,
                    };
                    let after = if muted { "" } else { " and plays now: mute it to hear the new best alone" };
                    r.tell_app(&format!("The last “{BEST}” stays where it is ({why}){after}; the new best goes on a track of its own"));
                    break;
                }
                unmuted |= at > 0;
            }
            let args = json!({"action": "make", "ref": track, "count": 1, "label": "Kumi: goal best"});
            let made = match r.ask("scratch", args, r.cleanup_signal()).await? {
                Ok(made) => made,
                Err(refused) => return Ok(Err(refused.why)),
            };
            let (Some(scratch), Some(copy)) =
                (text(&made, "scratchId"), made.pointer("/copies/0/ref").and_then(Value::as_str).map(str::to_owned))
            else {
                return Ok(Err(format!("{} made no copy of “{slot}”", r.app())));
            };
            let set: Vec<Value> = set_at
                .iter()
                .map(|(device, param, value)| {
                    let mut entry = json!({"ref": copy, "device": device, "param": param});
                    entry[r.table.scratch_field] = json!(value);
                    entry
                })
                .collect();
            let mut steps = made.pointer("/undo/steps").and_then(Value::as_u64).unwrap_or(0);
            if !set.is_empty() {
                match r.ask("scratch", json!({"action": "set", "scratchId": scratch, "values": set}), signal.clone()).await? {
                    Ok(done) => steps += done.pointer("/undo/steps").and_then(Value::as_u64).unwrap_or(0),
                    Err(refused) => {
                        r.drop_make(&scratch, steps).await;
                        return Ok(Err(refused.why));
                    }
                }
            }
            let kept = r.change("scratch_keep", json!({"ref": copy, "name": BEST}), signal.clone()).await;
            // The make goes either way: a copy kept as a track stays (the bridge leaves it), and the make no longer counts
            // as standing.
            r.drop_make(&scratch, steps).await;
            let mut kept = match kept {
                Ok((id, _)) => vec![id],
                Err(why) => return Ok(Err(why)),
            };
            // A paused goal's candidates weren't tidied: the producer's track still plays as it was, so the best waits
            // muted beside it rather than playing the part twice. (A finished goal muted its candidates for an A/B.)
            if !self.tidied.get() {
                match r.change("track_mute", json!({"ref": copy, "mute": true}), signal.clone()).await {
                    Ok((id, _)) => {
                        kept.push(id);
                        r.tell_app(&format!("“{BEST}” waits muted beside “{slot}”, which plays as it was: unmute it to hear the best"));
                    }
                    Err(_) => r.tell_app(&format!("“{BEST}” plays beside “{slot}”: mute one of them to hear the other alone")),
                }
            }
            *r.goal_best.borrow_mut() = kept;
            *r.goal_best_track.borrow_mut() = Some(r.link.book.borrow_mut().short_ref(&copy));
            // The winner is heard: the render it scored from at these very values, out loud.
            let heard = {
                let cache = self.heard.borrow();
                [false, true].into_iter().find_map(|screened| cache.get(&trial_key(slot, screened, values))).map(|found| found.render)
            };
            let (played, said) = match heard {
                Some(render) => r.play(&render, Some(&track)).await,
                None => (false, Some("it wasn't heard at these values in this search".to_owned())),
            };
            if !played {
                let why = said.map(|why| format!(" ({})", why.trim_end_matches('.'))).unwrap_or_default();
                r.tell_app(&format!("Kumi couldn't play “{BEST}” out loud{why}: play it to hear it"));
            }
            Ok(Ok(BEST.to_owned()))
        }
        .await;
        match result {
            Ok(Ok(name)) => Ok(name),
            Ok(Err(why)) => Ok(why),
            Err(error) => {
                signal.check()?;
                Ok(error.to_string())
            }
        }
    }
    async fn settle(&self, slot: &str, knobs: &[Knob], values: &[f64], given: Signal) -> Result<Option<String>, RuntimeError> {
        let signal = abort::any([given, self.renderer.lifetime()]);
        match self.set_track(slot, knobs, values, signal.clone()).await {
            Ok(()) => {
                if let Ok(track) = self.track_now(slot, &signal).await {
                    self.limiter_at_unity(slot, &track, &signal).await;
                }
                Ok(None)
            }
            Err(why) => {
                signal.check()?;
                Ok(Some(why))
            }
        }
    }
    async fn tidy(&self, top: &[String], given: Signal) -> Result<Vec<String>, RuntimeError> {
        self.tidied.set(true);
        let tidy = OwnTidy { rig: self, signal: abort::any([given, self.renderer.lifetime()]) };
        let slots = self.slots.borrow().iter().map(|slot| slot.info.name.clone()).collect();
        Ok(kumi_agent::core::goal_rig::tidy(&tidy, slots, top).await)
    }
    async fn close(&self) -> Result<Vec<String>, RuntimeError> {
        self.drop_copies().await;
        Ok(self.renderer.put_tempo_back().await.into_iter().collect())
    }
}

/// A finished goal's candidates tidied in REAPER or Renoise: each found as the book follows it (by its name when the book
/// lost it), a candidate the model made (its `track_add`) taken back.
struct OwnTidy<'a, S: RefScheme + 'static> {
    rig: &'a OwnGoalRig<S>,
    signal: Signal,
}

#[async_trait(?Send)]
impl<S: RefScheme + 'static> GoalDaw for OwnTidy<'_, S> {
    async fn track_names(&self) -> Option<Vec<String>> {
        match self.rig.renderer.track_rows(&self.signal).await {
            Ok(Ok(rows)) => Some(rows.iter().map(|row| row.name.clone()).collect()),
            _ => None,
        }
    }
    async fn track_of(&self, slot: &str) -> Option<String> {
        self.rig.renderer.forget_tracks();
        self.rig.track_now(slot, &self.signal).await.ok()
    }
    async fn mute(&self, track: &str) {
        let _ = self.rig.renderer.change("track_mute", json!({"ref": track, "mute": true}), self.signal.clone()).await;
    }
    fn made_by_kumi(&self, slot: &str) -> Option<String> {
        self.rig.renderer.plan.history.entries.borrow().values().rev().find_map(|entry| {
            let entry = entry.borrow();
            (entry.tool.as_deref() == Some("track_add")
                && entry.record.state == ChangeState::Applied
                && entry.record.title.contains(&format!("“{slot}”")))
            .then(|| entry.record.id.clone())
        })
    }
    async fn take_back(&self, id: &str) -> bool {
        self.rig.renderer.plan.history.undo(id, self.signal.clone(), true).await.is_ok_and(|undone| !undone.is_error)
    }
}
