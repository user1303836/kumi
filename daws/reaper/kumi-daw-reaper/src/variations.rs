//! REAPER's backend for the variations tool (#401, SFX variations): only REAPER's facts. A source is an item, audio or MIDI;
//! its takes are copies of it after it on its track, each with its own settings (pitch, rate, start offset, length, fades,
//! volume; a MIDI take its own notes), made, rewritten and dropped by REAPER's own kinds (`item_variation`,
//! `item_variation_set`, `items_delete`), one change each. Takes are found by their items' ids, else by the names Kumi gave
//! them, heard as their track plays them, and turned down by their take volume when that clips (#407). Core's tool does the
//! rest.
use async_trait::async_trait;
use kumi_agent::core::{
    refs::RefScheme,
    variations::{
        backend::{Rendered, Rewritten, Source, Undroppable, VariationsBackend, Words},
        heard::heard_of,
        notes::{
            amount, anchors_in, foreign_classes, note_direction_ops, note_op, notes_apart, round, sign, some_of, with_anchors, Note,
            NOTE_DIRECTIONS,
        },
        tool::{names, names_to_find},
        Material, Rng, TakeSet,
    },
};
use kumi_common::abort::Signal;
use kumi_daw_own::changes::OwnPlan;
use kumi_daw_own::listening::OwnRenderer;
use kumi_types::contracts::JsonObject;
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::rc::Rc;

pub static WORDS: Words = Words {
    app: "REAPER",
    sources: "an item (itemRef): an audio item (a footstep, an impact, a whoosh) or a MIDI item",
    anchors: "a MIDI take's notes every take keeps as they are (their index, from 0, as read); an audio take keeps its attack whatever the anchors",
    ops: "pitch (semitones, up or down; a MIDI take's notes move), rate (an audio take's playrate: faster or slower, the whole sound still heard), offset (a later start in its source, a few milliseconds), length (shorter), fades, volume; a MIDI take's velocity, timing and thin (fewer notes); amount 0 to 1",
    lines: "a MIDI take's notes as notes_add takes them ({pitch, start, length, velocity, channel}: quarter notes from the item's start; its anchor notes stay as they are: no need to write them)",
    places: "items (each take a copy of the source item after it on its track, gap seconds apart)",
    place_names: &["items"],
    at: "position (seconds) or positionBeats, and toRef (a track): where a kept take goes",
    options: "gap: the seconds between takes (0.5 by default)",
    extract: None,
    hears: None,
    rewrites: None,
    drops: None,
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
/// The most notes of a MIDI take Kumi varies: as many as one change writes.
const MOST_NOTES: u64 = 10_000;
/// What an item's row says of a MIDI take's source (reaper-kumi's `rows::source_word`).
const MIDI_SOURCE: &str = "mid";
/// How far an audio take's start moves at most (seconds): past that its attack isn't the sound's any more.
const ATTACK: f64 = 0.03;

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

    /// What an item holds now: its row, its take's, and a MIDI take's notes that play, with each one's index as REAPER reads it.
    async fn material_now(&self, item: &Value, anchors: &[Note], signal: &Signal) -> Result<(ItemTake, Vec<usize>), String> {
        let take_ref = item.get("takeRef").and_then(Value::as_str).ok_or("the item has no take")?;
        let take = self.one(take_ref, signal).await?;
        let midi = item.get("source").and_then(Value::as_str) == Some(MIDI_SOURCE);
        let (indexes, notes) = if midi {
            let count = take.get("noteCount").and_then(Value::as_u64).unwrap_or(0);
            if count > MOST_NOTES {
                return Err(format!("that take has {count} notes, and Kumi varies a MIDI take of at most {MOST_NOTES}"));
            }
            // (read to the end: a page holds what REAPER read in its time, a busy REAPER's fewer; every page holds one at least)
            let rows =
                self.plan.rows_through(json!({"kind": "note", "parent": take_ref, "limit": 200}), count as usize + 1, signal).await?;
            if (rows.len() as u64) < count {
                return Err(format!("Kumi read {} of that take's {count} notes: try again", rows.len()));
            }
            played(&rows).into_iter().unzip()
        } else {
            Default::default()
        };
        let number = |row: &Value, key: &str, default: f64| row.get(key).and_then(Value::as_f64).unwrap_or(default);
        let fade = |side: &str| item.pointer(&format!("/{side}/length")).and_then(Value::as_f64).unwrap_or(0.);
        let material = ItemTake {
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
        };
        Ok((material, indexes))
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
    async fn hear(&self, row: &Value, track: &str, signal: &Signal) -> Result<kumi_agent::core::variations::heard::Heard, String> {
        let from = row.get("positionBeats").and_then(Value::as_f64).ok_or("the take's place in beats isn't known")?;
        let beats = row.get("lengthBeats").and_then(Value::as_f64).filter(|beats| *beats > 0.).ok_or("the take has no length")?;
        let sources = vec![json!({"id": "t", "what": "track", "ref": track, "stage": "pre-fader"})];
        let window = kumi_listening::renderer::Window { from, beats };
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

/// A MIDI take's notes that play, of its note rows, each with its index as read: not its muted ones, nor those a trimmed start
/// hides (written, one would sound at the item's start); one held across the start plays from it.
fn played(rows: &[Value]) -> Vec<(usize, Note)> {
    rows.iter()
        .enumerate()
        .filter(|(_, row)| row.get("muted") != Some(&Value::Bool(true)))
        .filter_map(|(at, row)| {
            let mut note = Note::of(row)?;
            let start = row.get("start").and_then(Value::as_f64).unwrap_or(0.);
            if start < 0. {
                note.length = round(start + note.length);
                if note.length <= 0. {
                    return None;
                }
            }
            Some((row.get("index").and_then(Value::as_u64).map_or(at, |index| index as usize), note))
        })
        .collect()
}

/// The notes `wanted` by their index as read, of a take's notes that play and their indexes: which index isn't one of them.
fn anchors_of(indexes: &[usize], notes: &[Note], wanted: &[usize]) -> Result<Vec<Note>, String> {
    wanted
        .iter()
        .map(|n| {
            let at = indexes.iter().position(|index| index == n);
            at.map(|at| notes[at].clone()).ok_or_else(|| {
                format!("anchor {n}: no note that plays has that index (the take plays {} notes; read their indexes)", indexes.len())
            })
        })
        .collect()
}

/// Why an item can't be a source: its words. A pooled MIDI item's notes can't be read yet (reaper-kumi takes only "MIDI" for a
/// MIDI source, and REAPER says "MIDIPOOL"), and read as audio its takes would keep every note.
fn refused_source(item: &Value) -> Option<String> {
    (item.get("source").and_then(Value::as_str) == Some("midipool")).then(|| {
        "that MIDI item is pooled with others, and Kumi can't read a pooled item's notes yet: take it out of the pool in REAPER (the action \"Item: Remove active take from MIDI source data pool (unpool)\"), or vary another".to_string()
    })
}

/// A take as REAPER holds it once written: its settings as `settings` writes them and REAPER's reads give them back (times,
/// pitch and rate to the 10 000th, the volume to the 100th of a dB), so what Kumi reads back is what it kept and the take
/// stays Kumi's (#407: a take drawn to more places read back as edited since Kumi wrote it).
fn as_held(take: &ItemTake) -> ItemTake {
    ItemTake {
        length: round(take.length.max(0.001)),
        offset: round(take.offset.max(0.)),
        pitch: round(take.pitch.clamp(-96., 96.)),
        playrate: round(take.playrate.clamp(0.01, 100.)),
        volume_db: (take.volume_db.clamp(-150., 24.) * 100.).round() / 100.,
        fade_in: round(take.fade_in.clamp(0., take.length)),
        fade_out: round(take.fade_out.clamp(0., take.length)),
        ..take.clone()
    }
}

/// The operators applied to a take so far, each `factor` times as much (what each did, in a few words), the take then as REAPER
/// will hold it.
fn applied(ops: &[Value], factor: f64, take: &mut ItemTake, source: &ItemTake, rng: &mut Rng) -> Result<Vec<String>, String> {
    let said = ops.iter().map(|op| apply_one(op, factor, take, source, rng)).collect();
    *take = as_held(take);
    said
}

/// A take whose render clipped, turned down by its take volume (#407): `db` quieter, the rest of it as it is.
fn turned_down(take: &ItemTake, db: f64) -> ItemTake {
    as_held(&ItemTake { volume_db: take.volume_db - db, ..take.clone() })
}

/// What a take holds, as kept between runs: what its digest is of.
fn material_value(material: &ItemTake) -> Value {
    json!({
        "midi": material.midi, "length": material.length, "offset": material.offset, "pitch": material.pitch,
        "playrate": material.playrate, "volumeDb": material.volume_db, "fadeIn": material.fade_in, "fadeOut": material.fade_out,
        "notes": material.notes.iter().map(Note::json).collect::<Vec<_>>(),
        "anchors": material.anchors.iter().map(Note::json).collect::<Vec<_>>(),
    })
}

fn digest_of(material: &ItemTake) -> String {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    material_value(material).to_string().hash(&mut hasher);
    format!("{:016x}", hasher.finish())
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

/// One operator applied to the take so far, `factor` times as much: what it did, in a few words.
fn apply_one(op: &Value, factor: f64, take: &mut ItemTake, source: &ItemTake, rng: &mut Rng) -> Result<String, String> {
    let name = op.get("op").and_then(Value::as_str).unwrap_or("");
    let much = amount(op, factor);
    // (a MIDI take's notes move by core's shared note operators)
    if take.midi {
        if let Some(done) = note_op(name, op, much, &mut take.notes, &source.anchors, rng) {
            return done;
        }
    }
    match name {
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
        other => Err(format!("{other} isn't one of REAPER's variation operators")),
    }
}

const AUDIO_OPS: [&str; 6] = ["pitch", "rate", "offset", "length", "fades", "volume"];
// (no rate: REAPER writes notes at the project's beats whatever the playrate, so a MIDI take's rate moves nothing and shortens it)
const MIDI_OPS: [&str; 6] = ["pitch", "length", "volume", "velocity", "timing", "thin"];

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
        if let Some(why) = refused_source(&item) {
            return Err(why);
        }
        let (mut material, indexes) = self.material_now(&item, &[], signal).await?;
        // (the anchors: a MIDI take's notes by their index as read, which counts the ones that don't play)
        let anchors: Vec<usize> =
            input.get("anchors").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_u64).map(|n| n as usize).collect();
        if !anchors.is_empty() && !material.midi {
            return Err("anchors are a MIDI take's notes: an audio take keeps its attack anyway".into());
        }
        material.anchors = anchors_of(&indexes, &material.notes, &anchors)?;
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

    fn directions(&self, source: &Source<ItemTake, ReaperData>) -> &'static [&'static str] {
        if source.material.as_ref().is_some_and(|material| material.midi) {
            &NOTE_DIRECTIONS
        } else {
            WORDS.directions
        }
    }

    fn direction_ops(&self, source: &Source<ItemTake, ReaperData>, direction: &str) -> Vec<Value> {
        // (a MIDI item's takes go by its notes, as a Live clip's do: its settings alone move it too little to be a take)
        if source.material.as_ref().is_some_and(|material| material.midi) {
            return note_direction_ops(direction);
        }
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
        applied(ops, factor, take, source, rng)
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
        // (within half the 10 000th of a second a take is held to: its start as REAPER holds it)
        if !source.midi && take.offset - source.offset > ATTACK + 5e-5 {
            return Err(format!("its start moved {:.0} ms into the sound: past its attack", (take.offset - source.offset) * 1000.));
        }
        anchors_in(&source.anchors, &take.notes)
    }

    fn judge(&self, source: Option<&ItemTake>, take: &ItemTake, ops: &[Value], _factor: f64) -> Vec<String> {
        // (a MIDI take keeps its source's notes' pitch classes, unless it was moved on purpose)
        let Some(source) = source.filter(|source| source.midi) else { return Vec::new() };
        foreign_classes(&source.notes, &take.notes, ops)
    }

    fn digest(&self, material: &ItemTake) -> String {
        digest_of(material)
    }

    fn quieter(&self, take: &ItemTake, db: f64) -> Result<ItemTake, String> {
        Ok(turned_down(take, db))
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
        Ok(self.material_now(row, &anchors, signal).await?.0)
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

    async fn undroppable(&self, _set: &ReaperSet, _number: usize, _row: &Value, _signal: &Signal) -> Option<Undroppable> {
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
        material_value(material)
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

#[cfg(test)]
mod tests {

    use super::*;
    use kumi_agent::core::variations::Bounds;

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
        use crate::changes::CHANGES;
        use kumi_agent::kernel::schema::problem;
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

    /// (#407) What REAPER's reads give back of an audio take Kumi wrote (reaper-kumi's item and take rows: times, pitch and rate
    /// to the millionth, the volume to the 100th of a dB) is the take Kumi kept, drawn by any operator and then turned down:
    /// it stays Kumi's for a change or a drop. Before, a take kept the places its draw gave it, and read back as edited.
    #[test]
    fn an_audio_take_reads_back_from_reaper_as_kumi_kept_it_drawn_or_turned_down() {
        let read_back = |take: &ItemTake| {
            let written = settings(take, None);
            let number = |key: &str| written[key].as_f64().unwrap();
            let millionth = |value: f64| (value * 1e6).round() / 1e6;
            let length = number("length");
            let gain = 10f64.powf(number("volumeDb") / 20.);
            ItemTake {
                length: millionth(length),
                offset: millionth(number("startOffset")),
                pitch: millionth(number("pitch")),
                playrate: millionth(number("playrate")),
                volume_db: millionth((20. * gain.log10() * 100.).round() / 100.),
                fade_in: millionth(number("fadeIn").min(length)),
                fade_out: millionth(number("fadeOut").min(length)),
                ..take.clone()
            }
        };
        // (a source as REAPER reads it: a hot one, its offset to the millionth)
        let source = ItemTake { volume_db: -0.37, offset: 0.123456, ..audio() };
        for name in AUDIO_OPS {
            for (seed, up) in [(1, true), (2, false), (3, true), (4, false)] {
                let mut take = source.clone();
                let mut rng = Rng::new(seed);
                applied(&[json!({"op": name, "amount": 0.5, "up": up})], 1.35, &mut take, &source, &mut rng).unwrap();
                assert_eq!(digest_of(&read_back(&take)), digest_of(&take), "{name}: {take:?}");
                let quieter = turned_down(&take, 1.4);
                assert!((quieter.volume_db - (take.volume_db - 1.4)).abs() < 1e-9, "{name}: {quieter:?}");
                assert_eq!(ItemTake { volume_db: take.volume_db, ..quieter.clone() }, take, "{name}: only its volume moves");
                assert_eq!(digest_of(&read_back(&quieter)), digest_of(&quieter), "{name}: {quieter:?}");
            }
        }
    }

    /// The word this backend takes a MIDI item by is the one REAPER's extension writes: a row saying "mid" where it looked for
    /// "MIDI" made every MIDI item an audio one in real REAPER. (A shared word comes with the extension's next change.)
    #[test]
    fn a_midi_items_word_is_the_one_reapers_rows_write() {
        let rows = include_str!("../../reaper-kumi/src/rows.rs");
        assert!(rows.contains(&format!("\"midi\" => \"{MIDI_SOURCE}\".into()")), "rows::source_word writes another word for MIDI");
    }

    /// A take's notes that play, by REAPER's own indexes: not a muted one, nor one a trimmed start hides, which written would
    /// sound at the item's start (lab, 7.82); one held across the start plays from it.
    #[test]
    fn the_notes_of_a_take_are_those_that_play_with_their_indexes_as_read() {
        let row = |index: u64, start: f64, length: f64, muted: bool| json!({"index": index, "pitch": 60, "start": start, "length": length, "velocity": 90, "channel": 1, "selected": false, "muted": muted});
        let rows = [row(0, -0.5, 0.25, false), row(1, -0.25, 0.75, false), row(2, 0.5, 0.25, true), row(3, 1., 0.25, false)];
        let notes: Vec<(usize, f64, f64)> = played(&rows).into_iter().map(|(at, note)| (at, note.start, note.length)).collect();
        assert_eq!(notes, [(1, 0., 0.5), (3, 1., 0.25)]);
    }

    /// Anchors go by REAPER's own index, which counts the notes that don't play: past a muted note, the old count named the next.
    #[test]
    fn anchors_are_by_the_index_reaper_reads_past_a_muted_note() {
        let row = |index: u64, pitch: i64, muted: bool| json!({"index": index, "pitch": pitch, "start": index as f64, "length": 0.25, "velocity": 90, "channel": 1, "selected": false, "muted": muted});
        let (indexes, notes): (Vec<usize>, Vec<Note>) =
            played(&[row(0, 60, false), row(1, 61, true), row(2, 62, false), row(3, 63, false)]).into_iter().unzip();
        let pitches =
            |wanted: &[usize]| anchors_of(&indexes, &notes, wanted).map(|kept| kept.iter().map(|note| note.pitch).collect::<Vec<_>>());
        assert_eq!(pitches(&[0, 3]).unwrap(), [60, 63], "index 3 is the note at 3, not the fourth that plays");
        assert!(pitches(&[1]).unwrap_err().contains("anchor 1: no note that plays"), "a muted note can't be one");
        assert!(pitches(&[9]).unwrap_err().contains("anchor 9"));
    }

    /// A pooled MIDI item says why it can't be varied yet (lab, 7.82: REAPER calls its source MIDIPOOL), where read as audio its
    /// takes would keep every note; any other item is no matter of this.
    #[test]
    fn a_pooled_midi_item_is_refused_in_words() {
        assert!(refused_source(&json!({"source": "midipool"})).unwrap().contains("take it out of the pool"));
        for source in ["mid", "wav", "empty"] {
            assert_eq!(refused_source(&json!({"source": source})), None, "{source}");
        }
    }

    /// A MIDI take's operators are those that move something: REAPER writes notes at the project's beats whatever its playrate
    /// (lab, 7.82), so a rate moves no note and only cuts the end off.
    #[test]
    fn a_midi_take_has_no_rate_and_an_audio_take_has() {
        assert!(!MIDI_OPS.contains(&"rate") && AUDIO_OPS.contains(&"rate"));
        for direction in NOTE_DIRECTIONS {
            assert!(note_direction_ops(direction).iter().all(|op| op["op"] != "rate"), "{direction}");
        }
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

    /// Every direction Kumi picks for a MIDI item makes a take of it: a make of five gets five takes (its settings alone
    /// moved a take at most 0.06 from its source, a near-copy: "make 4" of a MIDI item gave V1 and V2).
    #[test]
    fn every_direction_kumi_picks_for_a_midi_item_makes_a_take_of_it() {
        kumi_agent::core::variations::notes::every_note_direction_makes_a_take(
            |notes, anchors| ItemTake { midi: true, notes, anchors, ..audio() },
            |ops, factor, take, source, rng| ops.iter().map(|op| apply_one(op, factor, take, source, rng)).collect(),
            |take| take.notes.as_slice(),
        );
    }
}
