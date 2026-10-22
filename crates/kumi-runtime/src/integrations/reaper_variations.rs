//! REAPER's backend for the variations tool (#401, SFX variations): only REAPER's facts. A source is an item, audio or MIDI;
//! its takes are copies of it after it on its track, each with its own settings (pitch, rate, start offset, length, fades,
//! volume; a MIDI take its own notes), made, rewritten and dropped by REAPER's own kinds (`item_variation`,
//! `item_variation_set`, `items_delete`), one change each. Takes are found by their items' ids, else by the names Kumi gave
//! them, and heard as their track plays them. Core's tool does the rest.
use super::own_changes::OwnPlan;
use super::own_listening::OwnRenderer;
use crate::core::{
    contracts::JsonObject,
    refs::RefScheme,
    variations::{
        backend::{Rendered, Rewritten, Source, VariationsBackend, Words},
        heard::heard_of,
        tool::{names, names_to_find},
        Material, Rng, TakeSet,
    },
};
use async_trait::async_trait;
use kumi_common::abort::Signal;
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::rc::Rc;

pub static WORDS: Words = Words {
    app: "REAPER",
    sources: "an item (itemRef): an audio item (a footstep, an impact, a whoosh) or a MIDI item",
    anchors: "a MIDI take's notes every take keeps as they are (their index, from 0, as read); an audio take keeps its attack whatever the anchors",
    ops: "pitch (semitones, up or down; a MIDI take's notes move), rate (its playrate: faster or slower, the whole sound still heard), offset (a later start in its source, a few milliseconds), length (shorter), fades, volume; a MIDI take's velocity, timing and thin (fewer notes); amount 0 to 1",
    lines: "a MIDI take's notes as notes_add takes them ({pitch, start, length, velocity, channel}: quarter notes from the item's start; its anchor notes stay as they are: no need to write them)",
    places: "items (each take a copy of the source item after it on its track, gap seconds apart)",
    place_names: &["items"],
    at: "position (seconds) or positionBeats, and toRef (a track): where a kept take goes",
    options: "gap: the seconds between takes (0.5 by default)",
    extract: None,
    idioms: "REAPER's items and takes, in seconds; a MIDI take's notes in quarter notes from the item's start",
    directions: &["higher", "lower", "faster", "slower", "shorter", "softer", "a later start", "longer fades"],
    source_schema: r#"{"type":"object","additionalProperties":false,"required":["itemRef"],"properties":{"itemRef":{"type":"string","description":"The item"}}}"#,
    op_schema: r#"{"type":"object","additionalProperties":false,"required":["op"],"properties":{"op":{"type":"string","enum":["pitch","rate","offset","length","fades","volume","velocity","timing","thin"]},"amount":{"type":"number","minimum":0,"maximum":1},"up":{"type":"boolean"},"semitones":{"type":"number","minimum":-24,"maximum":24}}}"#,
    options_schema: r#"{"type":"object","additionalProperties":false,"properties":{"gap":{"type":"number","minimum":0,"maximum":60}}}"#,
    at_schema: r#"{"type":"object","additionalProperties":false,"properties":{"position":{"type":"number","minimum":0},"positionBeats":{"type":"number","minimum":0},"toRef":{"type":"string"}}}"#,
};

/// The seconds between takes by default.
const GAP: f64 = 0.5;
/// Pages of rows read (200 a page).
const PAGES: usize = 20;
/// How far an audio take's start moves at most (seconds): past that its attack isn't the sound's any more.
const ATTACK: f64 = 0.03;

/// A MIDI note as a take holds it: pitch, start and length (quarter notes from the item's start), velocity and channel.
#[derive(Debug, Clone, PartialEq)]
pub struct Note {
    pub pitch: i64,
    pub start: f64,
    pub length: f64,
    pub velocity: i64,
    pub channel: i64,
}

impl Note {
    fn json(&self) -> Value {
        json!({"pitch": self.pitch, "start": round(self.start), "length": round(self.length), "velocity": self.velocity, "channel": self.channel})
    }

    fn of(value: &Value) -> Option<Note> {
        Some(Note {
            pitch: value.get("pitch")?.as_i64()?.clamp(0, 127),
            start: value.get("start")?.as_f64()?.max(0.),
            length: value.get("length")?.as_f64()?.max(1e-3),
            velocity: value.get("velocity").and_then(Value::as_i64).unwrap_or(100).clamp(1, 127),
            channel: value.get("channel").and_then(Value::as_i64).unwrap_or(1).clamp(1, 16),
        })
    }

    fn same(&self, other: &Note) -> bool {
        self.pitch == other.pitch && self.channel == other.channel && (self.start - other.start).abs() < 1e-3
    }
}

/// A take: its item's length and fades, its take's start offset, pitch, rate and volume, a MIDI take's notes, and the notes
/// every take keeps (the anchors).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ItemTake {
    pub midi: bool,
    pub length: f64,
    pub offset: f64,
    pub pitch: f64,
    pub playrate: f64,
    pub volume_db: f64,
    pub fade_in: f64,
    pub fade_out: f64,
    pub notes: Vec<Note>,
    pub anchors: Vec<Note>,
}

fn round(value: f64) -> f64 {
    (value * 10_000.).round() / 10_000.
}

/// How far apart two note lists are: the share of notes without a partner (same pitch and channel, its start near), and
/// how far partners' velocities and starts are.
fn notes_apart(a: &[Note], b: &[Note]) -> f64 {
    let most = a.len().max(b.len());
    if most == 0 {
        return 0.;
    }
    let mut used = vec![false; b.len()];
    let mut apart = 0.;
    let mut unmatched = 0usize;
    for note in a {
        let partner = b
            .iter()
            .enumerate()
            .filter(|(k, other)| {
                !used[*k] && other.pitch == note.pitch && other.channel == note.channel && (other.start - note.start).abs() <= 0.25
            })
            .min_by(|(_, x), (_, y)| (x.start - note.start).abs().total_cmp(&(y.start - note.start).abs()));
        match partner {
            Some((k, other)) => {
                used[k] = true;
                apart += 0.5 * ((other.velocity - note.velocity).abs() as f64 / 64.).min(1.)
                    + 0.5 * ((other.start - note.start).abs() / 0.25).min(1.);
            }
            None => unmatched += 1,
        }
    }
    unmatched += used.iter().filter(|u| !**u).count();
    ((unmatched as f64 + apart) / most as f64).min(1.)
}

impl Material for ItemTake {
    /// Each setting by what makes another sound of it (two semitones, a fifth faster, 50 ms, half again as long, 6 dB, 0.2 s
    /// of fade), weighted; a MIDI take's notes weigh most.
    fn distance(&self, other: &Self) -> f64 {
        let ratio = |a: f64, b: f64, scale: f64| if a > 0. && b > 0. { ((a / b).ln().abs() / scale).min(1.) } else { 1. };
        let pitch = ((self.pitch - other.pitch).abs() / 2.).min(1.);
        let rate = ratio(self.playrate, other.playrate, 1.2f64.ln());
        let offset = ((self.offset - other.offset).abs() / 0.05).min(1.);
        let length = ratio(self.length, other.length, 1.5f64.ln());
        let volume = ((self.volume_db - other.volume_db).abs() / 6.).min(1.);
        let fades = (((self.fade_in - other.fade_in).abs() + (self.fade_out - other.fade_out).abs()) / 0.2).min(1.);
        let settings = (3. * pitch + 2. * rate + 2. * offset + length + volume + fades) / 10.;
        if self.midi {
            0.3 * settings + 0.7 * notes_apart(&self.notes, &other.notes)
        } else {
            settings
        }
    }

    fn silent(&self) -> bool {
        self.length <= 0. || self.volume_db <= -60. || (self.midi && self.notes.is_empty())
    }
}

/// Where a set's source is: its item and take, its track, where it ends, and the gap between takes.
#[derive(Debug, Clone, PartialEq)]
pub struct ReaperData {
    item: String,
    take: String,
    track: String,
    track_name: String,
    end: f64,
    gap: f64,
}

pub type ReaperSet = TakeSet<Source<ItemTake, ReaperData>, Value, ItemTake>;

pub struct ReaperVariations<S: RefScheme + 'static> {
    plan: Rc<OwnPlan<S>>,
    renderer: Rc<OwnRenderer<S>>,
}

impl<S: RefScheme + 'static> ReaperVariations<S> {
    pub fn new(plan: Rc<OwnPlan<S>>, renderer: Rc<OwnRenderer<S>>) -> Self {
        ReaperVariations { plan, renderer }
    }

    async fn one(&self, reference: &str, signal: &Signal) -> Result<Value, String> {
        let answer = self.plan.read_through(json!({"ref": reference}), None, signal).await?;
        answer.pointer("/items/0").cloned().ok_or_else(|| format!("{} isn't in the project any more", self.short(reference)))
    }

    /// What an item holds now: its row, its take's, and a MIDI take's notes.
    async fn material_now(&self, item: &Value, anchors: &[Note], signal: &Signal) -> Result<ItemTake, String> {
        let take_ref = item.get("takeRef").and_then(Value::as_str).ok_or("the item has no take")?;
        let take = self.one(take_ref, signal).await?;
        let midi = item.get("source").and_then(Value::as_str) == Some("MIDI");
        let notes = if midi {
            let rows = self.plan.rows_through(json!({"kind": "note", "parent": take_ref, "limit": 200}), PAGES, signal).await?;
            rows.iter().filter(|row| row.get("muted") != Some(&Value::Bool(true))).filter_map(Note::of).collect()
        } else {
            Vec::new()
        };
        let number = |row: &Value, key: &str, default: f64| row.get(key).and_then(Value::as_f64).unwrap_or(default);
        let fade = |side: &str| item.pointer(&format!("/{side}/length")).and_then(Value::as_f64).unwrap_or(0.);
        Ok(ItemTake {
            midi,
            length: number(item, "length", 0.),
            offset: number(&take, "startOffset", 0.),
            pitch: number(&take, "pitch", 0.),
            playrate: number(&take, "playrate", 1.).max(0.01),
            volume_db: number(&take, "volumeDb", 0.),
            fade_in: fade("fadeIn"),
            fade_out: fade("fadeOut"),
            notes,
            anchors: anchors.to_vec(),
        })
    }

    async fn change(&self, tool: &str, input: Value, signal: &Signal) -> Result<(String, Value), (String, bool)> {
        let outcome = self.plan.change_by(tool, input.as_object().cloned().unwrap_or_default(), signal.clone()).await;
        let answer: Value = serde_json::from_str(&outcome.text).unwrap_or(Value::Null);
        if outcome.is_error || outcome.stops {
            let why = answer.get("changed").and_then(Value::as_str).map(str::to_owned).unwrap_or(outcome.text);
            return Err((why, outcome.stops));
        }
        let id = answer.get("change").and_then(Value::as_str).unwrap_or("").to_string();
        Ok((id, outcome.applied.unwrap_or(Value::Null)))
    }

    /// A track's items now, by their refs.
    async fn items_on(&self, track: &str, signal: &Signal) -> Result<Vec<Value>, String> {
        self.plan.rows_through(json!({"kind": "item", "parent": track, "limit": 200}), PAGES, signal).await
    }

    /// A take heard as its track plays it: one render over its item.
    async fn hear(&self, row: &Value, track: &str, signal: &Signal) -> Result<crate::core::variations::heard::Heard, String> {
        let from = row.get("positionBeats").and_then(Value::as_f64).ok_or("the take's place in beats isn't known")?;
        let beats = row.get("lengthBeats").and_then(Value::as_f64).filter(|beats| *beats > 0.).ok_or("the take has no length")?;
        let sources = vec![json!({"id": "t", "what": "track", "ref": track, "stage": "pre-fader"})];
        let window = crate::listening::renderer::Window { from, beats };
        match self.renderer.render_sources(sources, window, signal).await {
            Ok(Ok((files, _))) => match files.get("t") {
                Some((render, _)) => heard_of(render, signal).await,
                None => Err("the render made no file".to_string()),
            },
            Ok(Err(why)) => Err(why),
            Err(error) => Err(error.to_string()),
        }
    }
}

/// The settings a change writes for a take.
fn settings(material: &ItemTake, name: Option<&str>) -> Value {
    let mut input = json!({
        "length": round(material.length.max(0.001)),
        "startOffset": round(material.offset.max(0.)),
        "pitch": round(material.pitch.clamp(-96., 96.)),
        "playrate": round(material.playrate.clamp(0.01, 100.)),
        "volumeDb": round(material.volume_db.clamp(-150., 24.)),
        "fadeIn": round(material.fade_in.clamp(0., material.length)),
        "fadeOut": round(material.fade_out.clamp(0., material.length)),
    });
    if let Some(name) = name {
        input["name"] = json!(name);
    }
    if material.midi {
        input["notes"] = json!(material.notes.iter().map(Note::json).collect::<Vec<_>>());
    }
    input
}

/// Where a take of `length` seconds first fits after `from` with `gap` seconds clear of every span (an item's start and end).
fn free_after(spans: &[(f64, f64)], from: f64, length: f64, gap: f64) -> f64 {
    let mut spans: Vec<(f64, f64)> = spans.iter().copied().filter(|(_, end)| *end > from).collect();
    spans.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut at = from + gap;
    for (start, end) in spans {
        if start < at + length + gap && end + gap > at {
            at = at.max(end + gap);
        }
    }
    at
}

/// An operator's amount, `factor` times as much (0 to 1).
fn amount(op: &Value, factor: f64) -> f64 {
    (op.get("amount").and_then(Value::as_f64).unwrap_or(0.5) * factor).clamp(0., 1.)
}

/// Up or down: as the operator says, else by chance.
fn sign(op: &Value, rng: &mut Rng) -> f64 {
    match op.get("up").and_then(Value::as_bool) {
        Some(true) => 1.,
        Some(false) => -1.,
        None if rng.chance(0.5) => 1.,
        None => -1.,
    }
}

/// A share of `most`, from a third of it to all of it: never next to nothing.
fn some_of(most: f64, rng: &mut Rng) -> f64 {
    most * (1. + 2. * rng.below(1000) as f64 / 1000.) / 3.
}

/// One operator applied to the take so far, `factor` times as much: what it did, in a few words.
fn apply_one(op: &Value, factor: f64, take: &mut ItemTake, source: &ItemTake, rng: &mut Rng) -> Result<String, String> {
    let name = op.get("op").and_then(Value::as_str).unwrap_or("");
    let much = amount(op, factor);
    match name {
        "pitch" if take.midi => {
            let semitones =
                op.get("semitones").and_then(Value::as_f64).map(f64::round).unwrap_or_else(|| sign(op, rng) * (1. + (much * 4.).round()));
            for note in take.notes.iter_mut().filter(|note| !source.anchors.iter().any(|anchor| anchor.same(note))) {
                note.pitch = (note.pitch + semitones as i64).clamp(0, 127);
            }
            Ok(format!("notes {semitones:+} st"))
        }
        "pitch" => {
            let semitones = op.get("semitones").and_then(Value::as_f64).unwrap_or_else(|| sign(op, rng) * some_of(2. * much, rng));
            take.pitch += semitones;
            Ok(format!("pitch {:+.2} st", semitones))
        }
        "rate" => {
            let ratio = 1. + sign(op, rng) * some_of(0.15 * much, rng);
            let was = take.playrate;
            take.playrate = (was * ratio).clamp(0.01, 100.);
            // (the whole sound still heard, and no more of it: the item as long as the take so far plays at its new rate)
            take.length = (take.length * was / take.playrate).max(0.01);
            Ok(format!("rate \u{d7}{ratio:.3}"))
        }
        "offset" => {
            // (within the attack, however many offsets: never more than ATTACK from the source's start)
            let later = some_of(ATTACK * much, rng).min((source.offset + ATTACK - take.offset).max(0.));
            take.offset += later;
            take.length = (take.length - later / take.playrate.max(0.01)).max(0.01);
            Ok(format!("starts {:.0} ms later", later * 1000.))
        }
        "length" => {
            let share = 1. - some_of(0.35 * much, rng);
            take.length = (take.length * share).max(0.01);
            take.fade_out = take.fade_out.min(take.length);
            Ok(format!("{:.0}% as long", share * 100.))
        }
        "fades" => {
            take.fade_out = (take.fade_out + some_of(0.4 * much, rng) * take.length).min(take.length);
            take.fade_in = (take.fade_in + some_of(0.004 * much, rng)).min(take.length / 4.);
            Ok(format!("fades {:.0} ms in, {:.0} ms out", take.fade_in * 1000., take.fade_out * 1000.))
        }
        "volume" => {
            let db = sign(op, rng) * some_of(4. * much, rng);
            take.volume_db = (take.volume_db + db).clamp(-150., 24.);
            Ok(format!("volume {db:+.1} dB"))
        }
        "velocity" => {
            for note in take.notes.iter_mut().filter(|note| !source.anchors.iter().any(|anchor| anchor.same(note))) {
                let moved = (sign(op, rng) * some_of(30. * much, rng)).round() as i64;
                note.velocity = (note.velocity + moved).clamp(1, 127);
            }
            Ok("velocities moved".into())
        }
        "timing" => {
            for note in take.notes.iter_mut().filter(|note| !source.anchors.iter().any(|anchor| anchor.same(note))) {
                note.start = (note.start + sign(op, rng) * some_of(0.06 * much, rng)).max(0.);
            }
            Ok("timing loosened".into())
        }
        "thin" => {
            let free: Vec<usize> =
                (0..take.notes.len()).filter(|k| !source.anchors.iter().any(|anchor| anchor.same(&take.notes[*k]))).collect();
            // (at least one note goes, and one stays)
            let most = if take.notes.len() > free.len() { free.len() } else { free.len().saturating_sub(1) };
            let count = ((free.len() as f64 * 0.4 * much).round() as usize).max(1).min(most);
            let dropped: BTreeSet<usize> = rng.choose(free.len(), count).into_iter().map(|k| free[k]).collect();
            let mut k = 0;
            take.notes.retain(|_| {
                let keep = !dropped.contains(&k);
                k += 1;
                keep
            });
            Ok(format!("{} notes fewer", dropped.len()))
        }
        other => Err(format!("{other} isn't one of REAPER's variation operators")),
    }
}

const AUDIO_OPS: [&str; 6] = ["pitch", "rate", "offset", "length", "fades", "volume"];
const MIDI_OPS: [&str; 7] = ["pitch", "rate", "length", "volume", "velocity", "timing", "thin"];

#[async_trait(?Send)]
impl<S: RefScheme + 'static> VariationsBackend for ReaperVariations<S> {
    type Material = ItemTake;
    type Data = ReaperData;

    fn words(&self) -> &'static Words {
        &WORDS
    }

    async fn project_key(&self, signal: &Signal) -> Option<String> {
        let answer = self.plan.read_through(json!({"kind": "project"}), None, signal).await.ok()?;
        let project = answer.pointer("/items/0")?.clone();
        ["id", "identity"]
            .iter()
            .find_map(|key| project.get(*key).and_then(Value::as_str).filter(|text| !text.is_empty()).map(str::to_owned))
    }

    async fn source(&self, input: &JsonObject, signal: &Signal) -> Result<Source<ItemTake, ReaperData>, String> {
        let given =
            input.get("source").and_then(|source| source.get("itemRef")).and_then(Value::as_str).ok_or("make needs a source: itemRef")?;
        let item_ref = self.plan.lengthen(given)?;
        let item = self.one(&item_ref, signal).await?;
        let take = item.get("takeRef").and_then(Value::as_str).ok_or("that item has no take to vary")?.to_string();
        let track = item.get("trackRef").and_then(Value::as_str).ok_or("that item isn't on a track")?.to_string();
        let track_name = self
            .one(&track, signal)
            .await
            .ok()
            .and_then(|row| row.get("name").and_then(Value::as_str).map(str::to_owned))
            .unwrap_or_default();
        let mut material = self.material_now(&item, &[], signal).await?;
        // (the anchors: a MIDI take's notes by their index as read)
        let anchors: Vec<usize> =
            input.get("anchors").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_u64).map(|n| n as usize).collect();
        if !anchors.is_empty() && !material.midi {
            return Err("anchors are a MIDI take's notes: an audio take keeps its attack anyway".into());
        }
        if let Some(missing) = anchors.iter().find(|n| **n >= material.notes.len()) {
            return Err(format!("anchor {missing}: the take has {} notes (from 0)", material.notes.len()));
        }
        material.anchors = anchors.iter().map(|n| material.notes[*n].clone()).collect();
        let name = item.get("name").and_then(Value::as_str).unwrap_or("").to_string();
        let short = name.rsplit_once('.').map_or(name.as_str(), |(stem, _)| stem).trim().to_string();
        let short = if short.is_empty() { track_name.clone() } else { short };
        let position = item.get("position").and_then(Value::as_f64).unwrap_or(0.);
        let gap = input.get("options").and_then(|options| options.get("gap")).and_then(Value::as_f64).unwrap_or(GAP).clamp(0., 60.);
        Ok(Source {
            label: format!("\u{201c}{short}\u{201d} on \u{201c}{track_name}\u{201d}"),
            short,
            key: item_ref.clone(),
            material: Some(material.clone()),
            data: ReaperData { item: item_ref, take, track, track_name, end: position + material.length, gap },
        })
    }

    fn direction_ops(&self, direction: &str) -> Vec<Value> {
        let op = |name: &str, up: bool| json!({"op": name, "amount": 0.5, "up": up});
        match direction {
            "higher" => vec![op("pitch", true)],
            "lower" => vec![op("pitch", false)],
            "faster" => vec![op("rate", true)],
            "slower" => vec![op("rate", false)],
            "shorter" => vec![op("length", false)],
            "softer" => vec![op("volume", false)],
            "a later start" => vec![op("offset", true)],
            "longer fades" => vec![op("fades", true)],
            _ => vec![op("pitch", true)],
        }
    }

    fn check_ops(&self, source: &Source<ItemTake, ReaperData>, ops: &[Value]) -> Result<(), String> {
        let midi = source.material.as_ref().is_some_and(|material| material.midi);
        for op in ops {
            let name = op.get("op").and_then(Value::as_str).unwrap_or("");
            let fits: &[&str] = if midi { &MIDI_OPS } else { &AUDIO_OPS };
            if !fits.contains(&name) {
                return Err(format!("{name} isn't an operator for {} item: {}", if midi { "a MIDI" } else { "an audio" }, fits.join(", ")));
            }
        }
        Ok(())
    }

    fn apply_ops(&self, ops: &[Value], factor: f64, take: &mut ItemTake, source: &ItemTake, rng: &mut Rng) -> Result<Vec<String>, String> {
        ops.iter().map(|op| apply_one(op, factor, take, source, rng)).collect()
    }

    fn written(&self, _set: &ReaperSet, lines: &[Value], base: &ItemTake) -> Result<ItemTake, String> {
        if !base.midi {
            return Err("an audio item's takes come from operators: lines are a MIDI take's notes".into());
        }
        let notes: Vec<Note> = lines
            .iter()
            .map(|line| Note::of(line).ok_or_else(|| format!("a note needs pitch, start and length: {line}")))
            .collect::<Result<_, _>>()?;
        Ok(ItemTake { notes: with_anchors(notes, &base.anchors), ..base.clone() })
    }

    fn empty(&self, set: &ReaperSet) -> ItemTake {
        set.source.material.clone().map(|material| ItemTake { notes: Vec::new(), ..material }).unwrap_or(ItemTake {
            midi: true,
            playrate: 1.,
            length: 1.,
            ..ItemTake::default()
        })
    }

    fn anchors_kept(&self, source: &ItemTake, take: &ItemTake) -> Result<(), String> {
        if !source.midi && take.offset - source.offset > ATTACK + 1e-6 {
            return Err(format!("its start moved {:.0} ms into the sound: past its attack", (take.offset - source.offset) * 1000.));
        }
        match source.anchors.iter().find(|anchor| !take.notes.iter().any(|note| note == *anchor)) {
            Some(anchor) => Err(format!("the anchor note {} at {:.2} isn't as it was", anchor.pitch, anchor.start)),
            None => Ok(()),
        }
    }

    fn judge(&self, source: Option<&ItemTake>, take: &ItemTake, ops: &[Value], _factor: f64) -> Vec<String> {
        // (a MIDI take keeps its source's notes' pitch classes, unless it was moved on purpose)
        let Some(source) = source.filter(|source| source.midi) else { return Vec::new() };
        if ops.iter().any(|op| op.get("op").and_then(Value::as_str) == Some("pitch")) {
            return Vec::new();
        }
        let classes: BTreeSet<i64> = source.notes.iter().map(|note| note.pitch % 12).collect();
        let outside: BTreeSet<i64> = take.notes.iter().map(|note| note.pitch % 12).filter(|class| !classes.contains(class)).collect();
        if outside.is_empty() {
            Vec::new()
        } else {
            vec![format!("{} note class{} the source doesn't have", outside.len(), if outside.len() == 1 { "" } else { "es" })]
        }
    }

    fn digest(&self, material: &ItemTake) -> String {
        use std::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        self.material_json(material).to_string().hash(&mut hasher);
        format!("{:016x}", hasher.finish())
    }

    async fn write_new(
        &self,
        set: &ReaperSet,
        number: usize,
        material: &ItemTake,
        signal: &Signal,
    ) -> Result<(String, Value, String), (String, bool)> {
        // (the first free stretch after the source on its track, gap seconds clear of every item there: REAPER plays
        // overlapping items together, and render_files would take an overlap for one sound)
        let items = self.items_on(&set.source.data.track, signal).await.map_err(|why| (why, true))?;
        let spans: Vec<(f64, f64)> = items
            .iter()
            .filter_map(|row| {
                let start = row.get("position")?.as_f64()?;
                Some((start, start + row.get("length")?.as_f64()?))
            })
            .collect();
        let position = round(free_after(&spans, set.source.data.end, material.length, set.source.data.gap));
        let name = names(number, &set.source.short)[0].clone();
        let mut input = settings(material, Some(&name));
        input["ref"] = json!(set.source.data.item);
        input["position"] = json!(position);
        let (change, applied) = self.change("item_variation", input, signal).await?;
        let made = applied.pointer("/created/ref").and_then(Value::as_str).unwrap_or("").to_string();
        Ok((change, json!({"ref": made, "position": position, "length": round(material.length), "track": set.source.data.track}), made))
    }

    async fn find(&self, set: &mut ReaperSet, numbers: &[usize], signal: &Signal) -> Result<Vec<Value>, String> {
        let mut tracks: BTreeSet<String> = BTreeSet::from([set.source.data.track.clone()]);
        for take in &set.takes {
            if let Some(track) = take.place.get("track").and_then(Value::as_str) {
                tracks.insert(track.to_string());
            }
        }
        let mut rows: Vec<Value> = Vec::new();
        for track in &tracks {
            rows.extend(self.items_on(track, signal).await.unwrap_or_default());
        }
        let by_ref: BTreeMap<String, Value> =
            rows.iter().filter_map(|row| Some((row.get("ref")?.as_str()?.to_string(), row.clone()))).collect();
        let mut out = Vec::new();
        for number in numbers {
            let Some(take) = set.take(*number) else {
                out.push(json!({"gone": true}));
                continue;
            };
            let known = take.place.get("ref").and_then(Value::as_str).and_then(|reference| by_ref.get(reference)).cloned();
            // (found again by the name Kumi gave it, as made or as kept: a kept take's kept name first)
            let named = names_to_find(*number, &set.source.short, take.state);
            let found = known.or_else(|| {
                named.iter().find_map(|n| rows.iter().find(|row| row.get("name").and_then(Value::as_str) == Some(n.as_str()))).cloned()
            });
            match found {
                Some(row) => {
                    if let Some(take) = set.take_mut(*number) {
                        take.place =
                            json!({"ref": row["ref"], "position": row["position"], "length": row["length"], "track": row["trackRef"]});
                    }
                    out.push(row);
                }
                None => out.push(json!({"gone": true})),
            }
        }
        Ok(out)
    }

    async fn current(&self, set: &ReaperSet, row: &Value, signal: &Signal) -> Result<ItemTake, String> {
        let anchors = set.source.material.as_ref().map(|material| material.anchors.clone()).unwrap_or_default();
        self.material_now(row, &anchors, signal).await
    }

    async fn rewrite(
        &self,
        _set: &ReaperSet,
        row: &Value,
        _before: &ItemTake,
        after: &ItemTake,
        signal: &Signal,
    ) -> Result<Rewritten<ItemTake>, (String, bool)> {
        let mut input = settings(after, None);
        input["ref"] = row["ref"].clone();
        let (change, _) = self.change("item_variation_set", input, signal).await?;
        Ok((change, None))
    }

    async fn mark_kept(&self, set: &ReaperSet, number: usize, row: &Value, signal: &Signal) -> Result<Option<String>, String> {
        let [made, kept] = names(number, &set.source.short);
        let name = row.get("name").and_then(Value::as_str).unwrap_or("");
        if name == kept {
            return Ok(None);
        }
        if name != made {
            // (the producer named it: their name stays)
            return Ok(None);
        }
        let (change, _) = self.change("take_props", json!({"ref": row["ref"], "name": kept}), signal).await.map_err(|(why, _)| why)?;
        Ok(Some(change))
    }

    async fn place(&self, set: &mut ReaperSet, number: usize, row: &Value, at: &JsonObject, signal: &Signal) -> Result<Value, String> {
        let mut input = json!({"ref": row["ref"]});
        for key in ["position", "positionBeats", "toRef"] {
            if let Some(value) = at.get(key) {
                input[key] = value.clone();
            }
        }
        if input.as_object().is_some_and(|input| input.len() == 1) {
            return Err("at: position (seconds) or positionBeats, and toRef, a track".into());
        }
        let (change, applied) = self.change("item_move", input, signal).await.map_err(|(why, _)| why)?;
        if let Some(take) = set.take_mut(number) {
            take.place["position"] = applied.pointer("/applied/position").cloned().unwrap_or(Value::Null);
        }
        Ok(json!({"change": change}))
    }

    async fn undroppable(&self, _set: &ReaperSet, _number: usize, _row: &Value, _signal: &Signal) -> Option<String> {
        None
    }

    async fn drop(&self, _set: &ReaperSet, rows: &[(usize, Value)], signal: &Signal) -> Result<(String, Value), String> {
        let refs: Vec<Value> = rows.iter().map(|(_, row)| row["ref"].clone()).collect();
        self.change("items_delete", json!({"refs": refs}), signal).await.map_err(|(why, _)| why)
    }

    fn cant_render(&self) -> Option<String> {
        self.renderer.why_not()
    }

    async fn playing(&self, signal: &Signal) -> Option<bool> {
        self.plan.playing(signal).await
    }

    async fn render(&self, set: &mut ReaperSet, numbers: &[usize], with_source: bool, signal: &Signal) -> Result<Rendered, String> {
        if self.renderer.listening_now() {
            return Err("Kumi is listening to something else just now".into());
        }
        let _listening = self.renderer.begin_listen();
        let track = set.source.data.track.clone();
        let source = if with_source {
            let row = self.one(&set.source.data.item, signal).await?;
            Some(self.hear(&row, &track, signal).await?)
        } else {
            None
        };
        let found = self.find(set, numbers, signal).await?;
        let mut heard = BTreeMap::new();
        for (number, row) in numbers.iter().zip(&found) {
            if row.get("ref").is_some() {
                let on = row.get("trackRef").and_then(Value::as_str).unwrap_or(&track).to_string();
                heard.insert(*number, self.hear(row, &on, signal).await);
            }
        }
        self.renderer.put_tempo_back().await;
        Ok((source, heard))
    }

    fn short(&self, long: &str) -> String {
        self.plan.link().book.borrow_mut().short_ref(long)
    }

    fn end_note(&self, set: &ReaperSet) -> Option<String> {
        Some(format!(
            "The takes are copies of the source on \u{201c}{}\u{201d}, each in the first free stretch after it (clear of the items there): render_files plans their regions and renders a file each.",
            set.source.data.track_name
        ))
    }

    async fn extract(&self, _input: &JsonObject, _signal: &Signal) -> Result<Value, String> {
        Err("REAPER's variations have no extract".into())
    }

    fn material_json(&self, material: &ItemTake) -> Value {
        json!({
            "midi": material.midi, "length": material.length, "offset": material.offset, "pitch": material.pitch,
            "playrate": material.playrate, "volumeDb": material.volume_db, "fadeIn": material.fade_in, "fadeOut": material.fade_out,
            "notes": material.notes.iter().map(Note::json).collect::<Vec<_>>(),
            "anchors": material.anchors.iter().map(Note::json).collect::<Vec<_>>(),
        })
    }

    fn material_of(&self, value: &Value) -> Option<ItemTake> {
        let number = |key: &str| value.get(key).and_then(Value::as_f64);
        let notes = |key: &str| value.get(key).and_then(Value::as_array).into_iter().flatten().filter_map(Note::of).collect();
        Some(ItemTake {
            midi: value.get("midi")?.as_bool()?,
            length: number("length")?,
            offset: number("offset")?,
            pitch: number("pitch")?,
            playrate: number("playrate")?,
            volume_db: number("volumeDb")?,
            fade_in: number("fadeIn")?,
            fade_out: number("fadeOut")?,
            notes: notes("notes"),
            anchors: notes("anchors"),
        })
    }

    fn data_json(&self, data: &ReaperData) -> Value {
        json!({"item": data.item, "take": data.take, "track": data.track, "trackName": data.track_name, "end": data.end, "gap": data.gap})
    }

    fn data_of(&self, value: &Value) -> Option<ReaperData> {
        let text = |key: &str| value.get(key).and_then(Value::as_str).map(str::to_owned);
        Some(ReaperData {
            item: text("item")?,
            take: text("take")?,
            track: text("track")?,
            track_name: text("trackName").unwrap_or_default(),
            end: value.get("end")?.as_f64()?,
            gap: value.get("gap").and_then(Value::as_f64).unwrap_or(GAP),
        })
    }
}

/// Notes the model wrote with the take's anchor notes in, as they are (#400 review notes: written notes never have to repeat
/// them), by start and pitch.
fn with_anchors(mut notes: Vec<Note>, anchors: &[Note]) -> Vec<Note> {
    for anchor in anchors {
        if !notes.contains(anchor) {
            notes.push(anchor.clone());
        }
    }
    notes.sort_by(|a, b| a.start.total_cmp(&b.start).then(a.pitch.cmp(&b.pitch)));
    notes
}

#[cfg(test)]
mod tests {
    #[test]
    fn written_notes_get_the_anchor_notes_they_left_out() {
        let note = |pitch: i64, start: f64| Note { pitch, start, length: 0.25, velocity: 100, channel: 0 };
        let anchors = vec![note(36, 0.), note(38, 1.)];
        let written = with_anchors(vec![note(42, 0.5), note(36, 0.)], &anchors);
        assert_eq!(written, vec![note(36, 0.), note(42, 0.5), note(38, 1.)]);
    }

    use super::*;
    use crate::core::variations::Bounds;

    fn audio() -> ItemTake {
        ItemTake { midi: false, length: 1.0, playrate: 1.0, fade_out: 0.1, ..ItemTake::default() }
    }

    #[test]
    fn an_sfx_take_a_semitone_up_and_a_little_faster_is_a_variation_not_a_copy_nor_another_sound() {
        let source = audio();
        let take = ItemTake { pitch: 1.0, playrate: 1.05, length: 1.0 / 1.05, ..audio() };
        let d = take.distance(&source);
        let bounds = Bounds::default();
        assert!(d > bounds.min_source && d < bounds.max_source, "{d}");
        assert!(ItemTake { pitch: 0.05, ..audio() }.distance(&source) < bounds.min_source, "a twentieth of a semitone is a copy");
        assert!(ItemTake { volume_db: -80., ..audio() }.silent());
    }

    #[test]
    fn every_change_a_take_makes_fits_its_reaper_kind() {
        use crate::integrations::reaper_changes::CHANGES;
        use crate::kernel::schema::problem;
        let note = Note { pitch: 38, start: 0.5, length: 0.25, velocity: 90, channel: 10 };
        let midi = ItemTake { midi: true, notes: vec![note], ..audio() };
        let mut made = settings(&midi, Some("V1 \u{b7} snare"));
        made["ref"] = json!("item:1");
        made["position"] = json!(12.5);
        let mut rewritten = settings(&audio(), None);
        rewritten["ref"] = json!("item:2");
        for (tool, input) in [
            ("item_variation", made),
            ("item_variation_set", rewritten),
            ("items_delete", json!({"refs": ["item:2", "item:3"]})),
            ("take_props", json!({"ref": "item:2", "name": "V1 \u{b7} snare \u{b7} kept"})),
            ("item_move", json!({"ref": "item:2", "position": 40.0, "toRef": "track:2"})),
        ] {
            let kind = CHANGES.kinds.iter().find(|kind| kind.tool == tool).unwrap_or_else(|| panic!("{tool}"));
            assert_eq!(problem(&Value::Object(kind.input_schema()), &input), None, "{tool}: {input}");
        }
    }

    #[test]
    fn a_take_goes_in_the_first_free_stretch_after_its_source_and_ops_build_on_each_other() {
        // (the source ends at 2; the producer's items at 2.6–4 and 9–10; a 1-second take with half a second clear)
        let spans = [(0.0, 2.0), (2.6, 4.0), (9.0, 10.0)];
        assert_eq!(free_after(&spans, 2.0, 1.0, 0.5), 4.5, "not over the next item: after it");
        assert_eq!(free_after(&spans, 2.0, 5.0, 0.5), 10.5, "too long for the gap before 9: after that one");
        assert_eq!(free_after(&[], 2.0, 1.0, 0.5), 2.5);
        // Rate then length: the length follows the take so far, not the source.
        let backend_ops = [json!({"op": "rate", "amount": 1.0, "up": true}), json!({"op": "length", "amount": 1.0})];
        let source = audio();
        let mut take = source.clone();
        let mut rng = Rng::new(7);
        for op in &backend_ops {
            apply_one(op, 1.0, &mut take, &source, &mut rng).unwrap();
        }
        assert!(take.length < source.length / take.playrate + 1e-9, "never longer than its sound at the new rate: {take:?}");
    }

    #[test]
    fn midi_takes_are_apart_by_their_notes_and_keep_their_anchors() {
        let note = |pitch: i64, start: f64, velocity: i64| Note { pitch, start, length: 0.25, velocity, channel: 1 };
        let source = ItemTake {
            midi: true,
            notes: vec![note(36, 0., 100), note(38, 1., 100), note(42, 0.5, 80)],
            anchors: vec![note(36, 0., 100)],
            ..audio()
        };
        let mut take = source.clone();
        take.notes[2].velocity = 40;
        take.notes.remove(1);
        let d = take.distance(&source);
        assert!(d > 0.2 && d < 0.6, "{d}");
        assert_eq!(notes_apart(&source.notes, &source.notes), 0.);
    }
}
