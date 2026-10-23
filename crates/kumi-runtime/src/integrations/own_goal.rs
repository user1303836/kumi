//! A goal's render rig in a DAW whose bridge is Kumi's own (`/goal`, a match run's search, `/loop` with a reference):
//! each candidate track is searched on a scratch copy of it that never reaches the master, every generation's copies
//! rendered offline in one job and scored against the reference, so the producer's tracks stay as they are until the
//! search settles. The best is kept on a track of its own ("Kumi · Goal best") and played out loud: the winner is heard.
use super::own_listening::{DeviceRow, OwnRenderer};
use crate::{
    audio::{
        self,
        analyze::Analysis,
        matching::{closeness, Focus},
    },
    core::{contracts::*, errors::RuntimeError, evolve::Knob, refs::RefScheme},
    listening::{
        engine::{heard_options, silent_render, Listening},
        renderer::{Renderer, Window},
    },
};
use async_trait::async_trait;
use indexmap::IndexMap;
use kumi_common::{
    abort::{self, Signal, SignalExt},
    js::number::to_fixed,
};
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
    score: f64,
    gaps: Vec<String>,
    structural: Option<StructuralMove>,
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
    reference: Analysis,
    focus: Option<Focus>,
    screen: Option<Screen>,
    heard: RefCell<IndexMap<String, Cached>>,
    /// The finished goal's candidates were tidied (the top ones muted for an A/B): its best plays on its own.
    tidied: Cell<bool>,
    /// The safety limiters this rig put on candidates: taken back if the goal never opens (or a candidate never joins), and
    /// only these are put back at unity when a slot settles.
    limiters: RefCell<Vec<Limiter>>,
}

fn key(knob: &Knob) -> String {
    format!("{}|{}", knob.device, knob.name)
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
            reference,
            focus: request.focus,
            screen: None,
            heard: RefCell::new(IndexMap::new()),
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
        if window.beats * 60. / tempo >= 6. {
            let beats = (3. * tempo / 60.).round().max(2.);
            let lufs = &rig.reference.over_time.lufs;
            let slice = rig.reference.seconds / lufs.len().max(1) as f64;
            let across = (beats * 60. / tempo / slice).round().max(1.) as usize;
            let mut best = (0, f64::NEG_INFINITY);
            if across <= lufs.len() {
                for at in 0..=lufs.len() - across {
                    let part: Vec<f64> = lufs[at..at + across].iter().map(|v| v.unwrap_or(-70.)).collect();
                    let mean = part.iter().sum::<f64>() / part.len() as f64;
                    let change: f64 = part.windows(2).map(|v| (v[1] - v[0]).abs()).sum();
                    if mean + change > best.1 {
                        best = (at, mean + change);
                    }
                }
            }
            let offset = (best.0 as f64 * slice * tempo / 60.).round().min(window.beats - beats).max(0.);
            let mut snippet = request.clone();
            snippet.reference_from = Some(request.reference_from.unwrap_or(0.) + offset * 60. / tempo);
            snippet.reference_seconds = Some(beats * 60. / tempo);
            match listening.heard_reference(named, &snippet, signal).await {
                Ok(reference) => rig.screen = Some(Screen { offset, beats, reference }),
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
        if !devices.last().is_some_and(|device| device.own_name.contains(limiter)) {
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
        for note in super::own_listening::notes_of(&made) {
            r.tell_app(&note);
        }
        let chain = devices.iter().map(|device| device.name.clone()).collect::<Vec<_>>().join(" → ");
        let info =
            GoalSlotInfo { name: name.clone(), label: label.into(), chain: if chain.is_empty() { "empty".into() } else { chain }, knobs };
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
                    device: format!("{at}:{}", device.name),
                    name,
                    min,
                    max,
                    step: super::own_listening::steps_of(row).map(|steps| (max - min) / (steps - 1) as f64),
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

    /// Sets a slot's trial values on its copy; the knobs the DAW refused, by key.
    async fn set_copy(&self, slot: &str, moved: &[(Knob, f64)], signal: &Signal) -> Result<Result<(), String>, RuntimeError> {
        let r = &self.renderer;
        let (scratch, copy, places) = {
            let slots = self.slots.borrow();
            let Some(found) = slots.iter().find(|found| found.info.name == slot) else {
                return Ok(Err(format!("“{slot}” isn't in the search")));
            };
            let Some(copy) = r.copy_ref(&found.copy_name) else { return Ok(Err(format!("“{slot}”'s copy is gone"))) };
            (found.scratch.clone(), copy, found.places.clone())
        };
        let values: Vec<Value> = moved
            .iter()
            .filter_map(|(knob, value)| {
                let (device, param) = places.get(&key(knob))?;
                let mut set = json!({"ref": copy, "device": device, "param": param});
                set[r.table.scratch_field] = json!(value);
                Some(set)
            })
            .collect();
        if values.is_empty() {
            return Ok(Ok(()));
        }
        for chunk in values.chunks(256) {
            match r.ask("scratch", json!({"action": "set", "scratchId": scratch, "values": chunk}), signal.clone()).await? {
                Ok(done) => {
                    let steps = done.pointer("/undo/steps").and_then(Value::as_u64).unwrap_or(0);
                    if let Some(found) = self.slots.borrow_mut().iter_mut().find(|found| found.info.name == slot) {
                        found.steps += steps;
                    }
                }
                Err(refused) => return Ok(Err(refused.why)),
            }
        }
        Ok(Ok(()))
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
        let Some((stored, short)) = found else { return Err(format!("“{slot}” isn't in the search")) };
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
            if let Some(device) = super::own_listening::device_of_param(&knob.r#ref) {
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
        let r = &self.renderer;
        let signal = abort::any([given, r.lifetime()]);
        let screened = options.and_then(|options| options.screen) == Some(true) && self.screen.is_some();
        let cache_key = |trial: &GenerationTrial| {
            format!(
                "{}|{}|{}",
                trial.slot,
                if screened { "screen" } else { "full" },
                trial.values.iter().map(|value| to_fixed(*value, 4)).collect::<Vec<_>>().join(",")
            )
        };
        if r.flag().is_on() {
            return Err(RuntimeError::Observation("Another render is running.".into()));
        }
        let _rendering = r.flag().begin();
        let mut generation = Generation { screened, ..Default::default() };
        // Each trial's values on its slot's copy (only what moved).
        for trial in trials {
            let moved: Vec<(Knob, f64)> = {
                let current = self.current.borrow();
                let last = current.get(&trial.slot);
                trial
                    .knobs
                    .iter()
                    .enumerate()
                    .map(|(at, knob)| (knob.clone(), trial.values.get(at).copied().unwrap_or(f64::NAN)))
                    .filter(|(knob, value)| {
                        value.is_finite() && last.and_then(|last| last.get(&key(knob))).is_none_or(|last| (last - value).abs() > 1e-6)
                    })
                    .collect()
            };
            if moved.is_empty() {
                continue;
            }
            match self.set_copy(&trial.slot, &moved, &signal).await? {
                Ok(()) => {
                    let mut current = self.current.borrow_mut();
                    let last = current.entry(trial.slot.clone()).or_default();
                    for (knob, value) in &moved {
                        last.insert(key(knob), *value);
                    }
                }
                // (the DAW refused them: the search leaves them be)
                Err(_) => {
                    generation.frozen.insert(trial.slot.clone(), moved.iter().map(|(knob, _)| key(knob)).collect::<HashSet<_>>());
                }
            }
        }
        // Rendered unless every trial was heard before at these values.
        let fresh: Vec<&GenerationTrial> =
            trials.iter().filter(|trial| trial.fresh == Some(true) || !self.heard.borrow().contains_key(&cache_key(trial))).collect();
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
        let tempo = r.tempo().unwrap_or(120.);
        let against = if screened { &self.screen.as_ref().unwrap().reference } else { &self.reference };
        for trial in trials {
            let cache_key = cache_key(trial);
            let known = if trial.fresh != Some(true) { self.heard.borrow().get(&cache_key).cloned() } else { None };
            if let Some(known) = known {
                generation.cached += 1;
                generation.scores.insert(trial.slot.clone(), known.score);
                generation.gaps.insert(trial.slot.clone(), known.gaps);
                if let Some(structural) = known.structural {
                    generation.structural.insert(trial.slot.clone(), structural);
                }
                continue;
            }
            let Some(rendered) = files.get(&trial.slot) else {
                generation.silent.push(trial.slot.clone());
                continue;
            };
            let seconds = rendered.seconds.unwrap_or(beats * 60. / tempo);
            let heard = audio::hear(&rendered.file, heard_options(rendered.start, seconds, self.focus, signal.clone()))
                .await
                .map_err(|error| RuntimeError::plain(error.to_string()))?;
            if silent_render(&heard) {
                generation.silent.push(trial.slot.clone());
                continue;
            }
            let close = closeness(&heard, against, self.focus);
            let structural = close.structural.map(|found| StructuralMove { gap: found.gap, r#move: found.r#move });
            generation.scores.insert(trial.slot.clone(), close.score);
            generation.gaps.insert(trial.slot.clone(), close.gaps.clone());
            if let Some(structural) = &structural {
                generation.structural.insert(trial.slot.clone(), structural.clone());
            }
            let mut cache = self.heard.borrow_mut();
            cache.insert(cache_key, Cached { score: close.score, gaps: close.gaps, structural, render: rendered.clone() });
            if cache.len() > 2000 {
                cache.shift_remove_index(0);
            }
        }
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
            let values_key = values.iter().map(|value| to_fixed(*value, 4)).collect::<Vec<_>>().join(",");
            let heard = {
                let cache = self.heard.borrow();
                ["full", "screen"].iter().find_map(|how| cache.get(&format!("{slot}|{how}|{values_key}"))).map(|found| found.render.clone())
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
        let r = &self.renderer;
        let signal = abort::any([given, r.lifetime()]);
        let mut said = vec![];
        // Last in the song first, each found again by its name: taking one away moves the refs of those after it.
        let names: Vec<String> = self.slots.borrow().iter().map(|slot| slot.info.name.clone()).collect();
        let order = match r.track_rows(&signal).await {
            Ok(Ok(rows)) => {
                let mut named: Vec<(usize, String)> =
                    names.iter().map(|name| (rows.iter().position(|row| row.name == *name).unwrap_or(0), name.clone())).collect();
                named.sort_by(|a, b| b.0.cmp(&a.0));
                named.into_iter().map(|(_, name)| name).collect()
            }
            _ => names.into_iter().rev().collect::<Vec<_>>(),
        };
        for name in order {
            r.forget_tracks();
            let Ok(track) = self.track_now(&name, &signal).await else { continue };
            if top.contains(&name) {
                let _ = r.change("track_mute", json!({"ref": track, "mute": true}), signal.clone()).await;
                continue;
            }
            // A candidate the model made goes again (its add undone); one of the producer's stays, muted.
            let made = r.plan.history.entries.borrow().values().rev().find_map(|entry| {
                let entry = entry.borrow();
                (entry.tool.as_deref() == Some("track_add")
                    && entry.record.state == ChangeState::Applied
                    && entry.record.title.contains(&format!("“{name}”")))
                .then(|| entry.record.id.clone())
            });
            let undone = match made {
                Some(id) => r.plan.history.undo(&id, signal.clone(), true).await.ok(),
                None => None,
            };
            if undone.is_none_or(|undone| undone.is_error) {
                let _ = r.change("track_mute", json!({"ref": track, "mute": true}), signal.clone()).await;
                said.push(format!("“{name}” stays, muted."));
            }
        }
        Ok(said)
    }
    async fn close(&self) -> Result<Vec<String>, RuntimeError> {
        self.drop_copies().await;
        Ok(self.renderer.put_tempo_back().await.into_iter().collect())
    }
}
