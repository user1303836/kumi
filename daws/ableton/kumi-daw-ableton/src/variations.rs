//! Live's backend for the variations tool (#401): only Live's facts. Its material is what a Session MIDI clip plays (core's
//! shared note form and operators), its takes are new clips below the source on its track, each in a scene with no other
//! clip (a song's section plays what its scene holds), a scene added at the end when none is free, written by write_midi_clip, found where Kumi left them or by the names Kumi gave them, a
//! rewrite the clip deleted and written again in its slot, kept takes renamed. Live plays a MIDI clip through its instrument
//! only live, so its takes aren't rendered: the producer launches their slots to hear them. Core's tool does the rest.
use super::changes::CHANGES;
use super::connection::LiveConnection;
use super::history::History;
use super::mutations::Mutations;
use super::notes::{clip_and_track, read_notes};
use super::observation::Observer;
use async_trait::async_trait;
use kumi_common::abort::Signal;
use kumi_runtime::core::{
    contracts::JsonObject,
    variations::{
        backend::{Rendered, Rewritten, Source, Undroppable, VariationsBackend, Words},
        notes::{
            amount, anchors_in, foreign_classes, note_direction_ops, note_op, notes_apart, round, with_anchors, Note, NOTE_DIRECTIONS,
            NOTE_OPS,
        },
        tool::{names, names_to_find},
        Material, Rng, TakeSet,
    },
};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::rc::Rc;

pub static WORDS: Words = Words {
    app: "Live",
    sources: "a Session MIDI clip (clipRef): what it plays, its loop",
    anchors: "the clip's notes every take keeps as they are (their index, from 0, as read_notes' json lists them)",
    ops: "pitch (semitones, up or down: the notes move), velocity (velocities moved), timing (loosened, in beats), thin (fewer notes); amount 0 to 1",
    lines: "a MIDI clip's notes ({pitch, start, duration, velocity}: beats from the start of what the clip plays; its anchor notes stay as they are: no need to write them)",
    places: "clips (each take below the source on its track, in a scene with no other clip; a scene added at the end when none is free)",
    place_names: &["clips"],
    at: "none in Live: a kept take stays in its slot (drag it where it goes)",
    options: "none in Live",
    extract: None,
    hears: Some("doesn't render them (Live plays a MIDI clip only live: the producer launches the takes' slots to hear them)"),
    rewrites: Some("two changes each (Live adds no notes to a clip: its clip is deleted and written again in its slot)"),
    drops: Some("go a change each (one clip each)"),
    idioms: "Live's Session clips; notes in beats from the start of what the clip plays",
    directions: &NOTE_DIRECTIONS,
    source_schema: r#"{"type":"object","additionalProperties":false,"required":["clipRef"],"properties":{"clipRef":{"type":"string","description":"The Session MIDI clip"}}}"#,
    op_schema: r#"{"type":"object","additionalProperties":false,"required":["op"],"properties":{"op":{"type":"string","enum":["pitch","velocity","timing","thin"]},"amount":{"type":"number","minimum":0,"maximum":1},"up":{"type":"boolean"},"semitones":{"type":"number","minimum":-24,"maximum":24}}}"#,
    options_schema: r#"{"type":"object","additionalProperties":false,"properties":{}}"#,
    at_schema: r#"{"type":"object","additionalProperties":false,"properties":{}}"#,
};

/// Clips read for their names and notes at once (read_notes takes 16).
const CLIPS_AT_ONCE: usize = 16;

/// A take: the notes its clip plays (beats from the start of what it plays), how long that is, and the notes every take
/// keeps (the anchors).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ClipTake {
    pub notes: Vec<Note>,
    pub length: f64,
    pub anchors: Vec<Note>,
}

impl Material for ClipTake {
    /// Its notes, by what tells them apart (core's), and a little its length.
    fn distance(&self, other: &Self) -> f64 {
        let length = if self.length > 0. && other.length > 0. { ((self.length / other.length).ln().abs() / 2f64.ln()).min(1.) } else { 1. };
        0.9 * notes_apart(&self.notes, &other.notes) + 0.1 * length
    }

    fn silent(&self) -> bool {
        self.notes.is_empty() || self.length <= 0.
    }
}

/// Where a set's source is: its track (by Kumi's own id when Live keeps it, else its name, and the ref it had), the source
/// clip's scene, and its name.
#[derive(Debug, Clone, PartialEq)]
pub struct LiveData {
    track: String,
    track_id: Option<String>,
    track_name: String,
    scene: u64,
    clip_name: String,
}

pub type LiveSet = TakeSet<Source<ClipTake, LiveData>, Value, ClipTake>;

pub struct LiveVariations {
    connection: Rc<LiveConnection>,
    mutations: Rc<Mutations>,
    history: Rc<History>,
    observer: Rc<Observer>,
}

/// A clip's slot (its scene) from its long ref (`E:clip:T:S`).
fn scene_of(clip: &str) -> Option<u64> {
    clip.rsplit(':').next()?.parse().ok()
}

/// The long ref of the clip in a track's slot (`E:track:T` and scene S: `E:clip:T:S`).
fn clip_at(track: &str, scene: u64) -> Option<String> {
    let [epoch, "track", index] = track.split(':').collect::<Vec<_>>()[..] else { return None };
    Some(format!("{epoch}:clip:{index}:{scene}"))
}

/// The first slot below `scene` in a scene with no clip on any track (`empty`): a scene that's a section of the song
/// plays what it holds, an empty slot there included (it stops the track).
fn free_below(slots: &[(u64, Option<String>)], scene: u64, empty: &BTreeSet<u64>) -> Option<u64> {
    slots.iter().find(|(at, clip)| *at > scene && clip.is_none() && empty.contains(at)).map(|(at, _)| *at)
}

/// The part of a clip that plays (beats from its start): its loop when it loops, else its markers.
fn played(clip: &Value) -> (f64, f64) {
    let at = |key: &str| clip.get("loop").and_then(|looped| looped.get(key)).and_then(Value::as_f64);
    let looping = clip.get("loop").and_then(|looped| looped.get("looping")).and_then(Value::as_bool).unwrap_or(true);
    let (from, to) = if looping { (at("loopStart"), at("loopEnd")) } else { (at("startMarker"), at("endMarker")) };
    let from = from.unwrap_or(0.).max(0.);
    let to = to.or_else(|| at("length").map(|length| from + length)).unwrap_or(from + 4.);
    (from, to.max(from + 1. / 16.))
}

/// A note row as the clip plays it, from the start of what it plays; None when it doesn't play (muted, or outside it).
fn played_note(row: &Value, (from, to): (f64, f64)) -> Option<Note> {
    if row.get("mute") == Some(&Value::Bool(true)) {
        return None;
    }
    let mut note = Note::of(row)?;
    if note.start < from - 1e-6 || note.start >= to - 1e-6 {
        return None;
    }
    note.start = round((note.start - from).max(0.));
    Some(note)
}

/// A clip as read_notes' json gives it: the notes it plays and how long that is.
fn take_of(clip: &Value) -> Result<ClipTake, String> {
    if let Some(error) = clip.get("error").and_then(Value::as_str) {
        return Err(error.to_string());
    }
    if let Some(partial) = clip.get("truncated").and_then(Value::as_str) {
        return Err(format!("{partial}: Kumi makes takes only of a clip it reads whole"));
    }
    let part = played(clip);
    let notes = clip.get("notes").and_then(Value::as_array).into_iter().flatten().filter_map(|row| played_note(row, part)).collect();
    Ok(ClipTake { notes, length: part.1 - part.0, anchors: Vec::new() })
}

/// A take's notes as write_midi_clip takes them.
fn notes_json(take: &ClipTake) -> Vec<Value> {
    take.notes
        .iter()
        .map(|note| {
            let mut out = json!({"pitch": note.pitch, "start": round(note.start), "duration": round(note.length), "velocity": note.velocity.clamp(1, 127)});
            if note.channel != 1 {
                out["channel"] = json!(note.channel);
            }
            out
        })
        .collect()
}

/// What a take holds, to tell whether anyone changed it: its length and notes, in time and pitch order (Live gives a clip's
/// notes back in its own).
fn digest_of(material: &ClipTake) -> String {
    use std::hash::{Hash, Hasher};
    let mut notes = notes_json(material);
    notes.sort_by(|a, b| {
        a["start"].as_f64().unwrap_or(0.).total_cmp(&b["start"].as_f64().unwrap_or(0.)).then(a.to_string().cmp(&b.to_string()))
    });
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    json!({"length": round(material.length), "notes": notes}).to_string().hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

/// Operators applied to a take, the anchors left as they are and its notes inside its clip: what they did, in a few words.
fn applied(ops: &[Value], factor: f64, take: &mut ClipTake, source: &ClipTake, rng: &mut Rng) -> Result<Vec<String>, String> {
    let said = ops
        .iter()
        .map(|op| {
            let name = op.get("op").and_then(Value::as_str).unwrap_or("");
            note_op(name, op, amount(op, factor), &mut take.notes, &source.anchors, rng)
                .unwrap_or_else(|| Err(format!("{name} isn't a Live operator")))
        })
        .collect();
    kept_inside(take);
    said
}

/// A take's notes inside its clip: one loosened past the clip's end wouldn't play.
fn kept_inside(take: &mut ClipTake) {
    let last = round((take.length - 1. / 64.).max(0.));
    for note in &mut take.notes {
        note.start = note.start.min(last);
    }
}

/// Where a take is on its track: the slot Kumi left it in when the clip there has one of its names, else the first clip of
/// one of its names (in the order to try them).
fn pick(slots: &[(u64, Option<String>)], named: &BTreeMap<String, String>, at: Option<u64>, wanted: &[String]) -> Option<(u64, String)> {
    let name_of = |clip: &str| named.get(clip).map(String::as_str);
    let here = at.and_then(|scene| slots.iter().find(|(s, _)| *s == scene)).and_then(|(scene, clip)| {
        let clip = clip.as_ref()?;
        wanted.iter().any(|w| Some(w.as_str()) == name_of(clip)).then(|| (*scene, clip.clone()))
    });
    here.or_else(|| {
        wanted.iter().find_map(|name| {
            slots.iter().find_map(|(scene, clip)| {
                let clip = clip.as_ref()?;
                (name_of(clip) == Some(name.as_str())).then(|| (*scene, clip.clone()))
            })
        })
    })
}

impl LiveVariations {
    pub fn new(connection: Rc<LiveConnection>, mutations: Rc<Mutations>, history: Rc<History>, observer: Rc<Observer>) -> Self {
        LiveVariations { connection, mutations, history, observer }
    }

    fn lengthen(&self, short: &str) -> String {
        self.connection.references.borrow().lengthen(&json!({"ref": short}))["ref"].as_str().unwrap_or(short).to_owned()
    }

    /// Rows of a discovery read (their refs registered for this turn's changes, given short).
    async fn rows(&self, kind: &str, extra: Value, signal: &Signal) -> Result<Vec<JsonObject>, String> {
        self.connection.rows(kind, extra.as_object().cloned().unwrap_or_default(), signal.clone()).await.map_err(|error| {
            let error: kumi_types::errors::RuntimeError = error.into();
            error.to_string()
        })
    }

    /// The Set's tracks: their names, and Kumi's own ids when this Live keeps them.
    async fn tracks(&self, signal: &Signal) -> Result<Vec<JsonObject>, String> {
        let mut fields = vec!["name"];
        if self.history.remember.track_ids.reported(self.connection.epoch.get()) {
            fields.push("kumiTrack");
        }
        self.rows("track", json!({"fields": fields}), signal).await
    }

    /// One change by its kind's name: its id and what it answered (`ref`: what it made), or why it wasn't made and whether
    /// the request must stop there.
    async fn change(&self, tool: &str, input: Value, signal: &Signal) -> Result<(String, Value), (String, bool)> {
        let kind = CHANGES
            .iter()
            .find(|kind| kind.tool == tool && kind.internal != Some(true))
            .ok_or((format!("Live has no {tool} change"), false))?;
        let outcome = self.mutations.change(kind, input.as_object().cloned().unwrap_or_default(), signal.clone(), false).await;
        let answer: Value = serde_json::from_str(&outcome.text).unwrap_or_else(|_| json!({"changed": outcome.text}));
        if outcome.is_error || outcome.stops {
            let why = answer.get("changed").and_then(Value::as_str).map(str::to_owned).unwrap_or(outcome.text);
            return Err((why, outcome.stops));
        }
        let id = answer.get("change").and_then(Value::as_str).unwrap_or("").to_string();
        Ok((id, answer))
    }

    /// The source's track now: by Kumi's own id, else its name (one track of that name): its short ref.
    async fn track_now(&self, data: &LiveData, signal: &Signal) -> Result<String, String> {
        let tracks = self.tracks(signal).await?;
        let by_id =
            data.track_id.as_deref().and_then(|id| tracks.iter().find(|row| row.get("kumiTrack").and_then(Value::as_str) == Some(id)));
        let named: Vec<&JsonObject> =
            tracks.iter().filter(|row| row.get("name").and_then(Value::as_str) == Some(data.track_name.as_str())).collect();
        let row = by_id.or(if named.len() == 1 { Some(named[0]) } else { None }).ok_or_else(|| {
            format!("the source's track \u{201c}{}\u{201d} isn't in the Set where Kumi left it: read again", data.track_name)
        })?;
        row.get("ref").and_then(Value::as_str).map(str::to_owned).ok_or_else(|| "a track without a ref".to_string())
    }

    /// The scenes with no clip on any track (by their index), as Live says of each.
    async fn empty_scenes(&self, signal: &Signal) -> Result<BTreeSet<u64>, String> {
        let rows = self.rows("scene", json!({"fields": ["index", "isEmpty"]}), signal).await?;
        Ok(rows.iter().filter(|row| row.get("isEmpty") == Some(&Value::Bool(true))).filter_map(|row| row.get("index")?.as_u64()).collect())
    }

    /// A track's Session slots in order: each scene, and the clip there (short ref) when there is one.
    async fn slots(&self, track: &str, signal: &Signal) -> Result<Vec<(u64, Option<String>)>, String> {
        let rows = self.rows("clip-slot", json!({"parent": track, "fields": ["sceneIndex", "clipRef", "empty"]}), signal).await?;
        let mut slots: Vec<(u64, Option<String>)> = rows
            .iter()
            .filter_map(|row| {
                let scene = row.get("sceneIndex")?.as_u64()?;
                let clip =
                    row.get("clipRef").and_then(Value::as_str).filter(|_| row.get("empty") != Some(&Value::Bool(true))).map(str::to_owned);
                Some((scene, clip))
            })
            .collect();
        slots.sort_by_key(|(scene, _)| *scene);
        Ok(slots)
    }

    /// Clips' names, notes and loops (read_notes' json), by their short refs, 16 at a time.
    async fn clips(&self, refs: &[String], signal: &Signal) -> Result<BTreeMap<String, Value>, String> {
        let mut out = BTreeMap::new();
        for chunk in refs.chunks(CLIPS_AT_ONCE) {
            let input = json!({"clipRefs": chunk, "format": "json"});
            let read =
                read_notes(input.as_object().unwrap_or(&JsonObject::new()), &self.connection, self.observer.tempo.get(), signal.clone())
                    .await;
            if read.is_error {
                return Err(read.text);
            }
            let answer: Value = serde_json::from_str(&read.text).unwrap_or(Value::Null);
            for clip in answer.get("clips").and_then(Value::as_array).into_iter().flatten() {
                if let Some(reference) = clip.get("clip").and_then(Value::as_str) {
                    out.insert(reference.to_string(), clip.clone());
                }
            }
        }
        Ok(out)
    }
}

#[async_trait(?Send)]
impl VariationsBackend for LiveVariations {
    type Material = ClipTake;
    type Data = LiveData;

    fn words(&self) -> &'static Words {
        &WORDS
    }

    async fn project_key(&self, _signal: &Signal) -> Option<String> {
        let current = self.history.remember.current()?;
        Some(current.project.clone().unwrap_or_else(|| current.identity.clone())).filter(|key| !key.is_empty())
    }

    async fn source(&self, input: &JsonObject, signal: &Signal) -> Result<Source<ClipTake, LiveData>, String> {
        let given =
            input.get("source").and_then(|source| source.get("clipRef")).and_then(Value::as_str).ok_or("make needs a source: clipRef")?;
        let (clip, track) = clip_and_track(&self.connection, given).ok_or("clipRef is a Session clip of this turn's reads")?;
        if !clip.contains(":clip:") {
            return Err("variations in Live are of a Session clip: an Arrangement clip isn't one".into());
        }
        let scene = scene_of(&clip).ok_or("that clip's slot wasn't read")?;
        let read = self.clips(&[given.to_string()], signal).await?;
        let row = read.get(given).ok_or("Kumi couldn't read that clip's notes")?;
        let mut material = take_of(row)?;
        // (anchors by their index in read_notes' json, each a note the clip plays)
        let listed = row.get("notes").and_then(Value::as_array).cloned().unwrap_or_default();
        let part = played(row);
        for n in input.get("anchors").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_u64) {
            let note = listed.get(n as usize).ok_or_else(|| format!("anchor {n}: the clip has {} notes (from 0)", listed.len()))?;
            material.anchors.push(
                played_note(note, part)
                    .ok_or_else(|| format!("anchor {n} isn't a note the clip plays (it's muted, or outside its loop)"))?,
            );
        }
        let tracks = self.tracks(signal).await?;
        let track_row = tracks
            .iter()
            .find(|row| row.get("ref").and_then(Value::as_str).map(|short| self.lengthen(short)).as_deref() == Some(track.as_str()));
        let track_name = track_row.and_then(|row| row.get("name")).and_then(Value::as_str).unwrap_or("the track").to_string();
        let track_id = track_row.and_then(|row| row.get("kumiTrack")).and_then(Value::as_str).map(str::to_owned);
        let clip_name = row.get("name").and_then(Value::as_str).filter(|name| !name.is_empty()).unwrap_or(&track_name).to_string();
        Ok(Source {
            label: format!("\u{201c}{clip_name}\u{201d} on \u{201c}{track_name}\u{201d}"),
            short: clip_name.clone(),
            key: format!("clip:{}:{scene}", track_id.clone().unwrap_or_else(|| track_name.clone())),
            material: Some(material),
            data: LiveData { track, track_id, track_name, scene, clip_name },
        })
    }

    fn direction_ops(&self, _source: &Source<ClipTake, LiveData>, direction: &str) -> Vec<Value> {
        note_direction_ops(direction)
    }

    fn check_ops(&self, _source: &Source<ClipTake, LiveData>, ops: &[Value]) -> Result<(), String> {
        for op in ops {
            let name = op.get("op").and_then(Value::as_str).unwrap_or("");
            if !NOTE_OPS.contains(&name) {
                return Err(format!("{name} isn't an operator for a Live clip: {}", NOTE_OPS.join(", ")));
            }
        }
        Ok(())
    }

    fn apply_ops(&self, ops: &[Value], factor: f64, take: &mut ClipTake, source: &ClipTake, rng: &mut Rng) -> Result<Vec<String>, String> {
        applied(ops, factor, take, source, rng)
    }

    fn written(&self, _set: &LiveSet, lines: &[Value], base: &ClipTake) -> Result<ClipTake, String> {
        let notes: Vec<Note> = lines
            .iter()
            .map(|line| Note::of(line).ok_or_else(|| format!("a note needs pitch, start and duration: {line}")))
            .collect::<Result<_, _>>()?;
        if let Some(late) = notes.iter().find(|note| note.start >= base.length) {
            return Err(format!(
                "a note at {} starts past the clip's end ({} beats): it wouldn't play",
                round(late.start),
                round(base.length)
            ));
        }
        Ok(ClipTake { notes: with_anchors(notes, &base.anchors), ..base.clone() })
    }

    fn empty(&self, set: &LiveSet) -> ClipTake {
        set.source
            .material
            .clone()
            .map(|material| ClipTake { notes: Vec::new(), ..material })
            .unwrap_or(ClipTake { length: 4., ..ClipTake::default() })
    }

    fn anchors_kept(&self, source: &ClipTake, take: &ClipTake) -> Result<(), String> {
        anchors_in(&source.anchors, &take.notes)
    }

    fn judge(&self, source: Option<&ClipTake>, take: &ClipTake, ops: &[Value], _factor: f64) -> Vec<String> {
        source.map(|source| foreign_classes(&source.notes, &take.notes, ops)).unwrap_or_default()
    }

    fn digest(&self, material: &ClipTake) -> String {
        digest_of(material)
    }

    async fn write_new(
        &self,
        set: &LiveSet,
        number: usize,
        material: &ClipTake,
        signal: &Signal,
    ) -> Result<(String, Value, String), (String, bool)> {
        let name = names(number, &set.source.short)[0].clone();
        let mut track = self.track_now(&set.source.data, signal).await.map_err(|why| (why, true))?;
        // (the first slot below the source in a scene of its own; a scene at the end when none is free, named after the take)
        let empty = self.empty_scenes(signal).await.map_err(|why| (why, true))?;
        let mut free = free_below(&self.slots(&track, signal).await.map_err(|why| (why, true))?, set.source.data.scene, &empty);
        let mut made = Vec::new();
        if free.is_none() {
            let (id, _) = self.change("add_tracks_and_scenes", json!({"tracks": [], "scenes": [{"name": name}]}), signal).await?;
            made.push(id);
            track = self.track_now(&set.source.data, signal).await.map_err(|why| (why, true))?;
            let slots = self.slots(&track, signal).await.map_err(|why| (why, true))?;
            free = slots.last().filter(|(_, clip)| clip.is_none()).map(|(scene, _)| *scene);
        }
        let scene = free.ok_or(("Live made the scene, but its slot on the track isn't free".to_string(), true))?;
        let input =
            json!({"trackRef": track, "sceneIndex": scene, "name": name, "length": round(material.length), "notes": notes_json(material)});
        let (id, answer) = self.change("write_midi_clip", input, signal).await?;
        made.push(id);
        let made_ref = answer
            .get("ref")
            .and_then(Value::as_str)
            .map(|short| self.lengthen(short))
            .or_else(|| clip_at(&self.lengthen(&track), scene))
            .unwrap_or_default();
        Ok((made.join(", "), json!({"scene": scene, "name": name}), made_ref))
    }

    async fn find(&self, set: &mut LiveSet, numbers: &[usize], signal: &Signal) -> Result<Vec<Value>, String> {
        if numbers.is_empty() {
            return Ok(vec![]);
        }
        let Ok(track) = self.track_now(&set.source.data, signal).await else {
            return Ok(numbers.iter().map(|_| json!({"gone": true})).collect());
        };
        let slots = self.slots(&track, signal).await?;
        let wanted: Vec<(usize, Option<u64>, [String; 2])> = numbers
            .iter()
            .filter_map(|number| {
                let take = set.take(*number)?;
                Some((*number, take.place.get("scene").and_then(Value::as_u64), names_to_find(*number, &set.source.short, take.state)))
            })
            .collect();
        // (the clips where Kumi left the takes first; the track's others only when a take isn't there)
        let left: Vec<String> = wanted
            .iter()
            .filter_map(|(_, at, _)| slots.iter().find(|(scene, _)| Some(*scene) == *at)?.1.clone())
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect();
        let mut clips = self.clips(&left, signal).await?;
        let names_of = |clips: &BTreeMap<String, Value>| -> BTreeMap<String, String> {
            clips.iter().filter_map(|(clip, read)| Some((clip.clone(), read.get("name")?.as_str()?.to_string()))).collect()
        };
        let mut named = names_of(&clips);
        let all_there = wanted.iter().all(|(_, at, names)| pick(&slots, &named, *at, names).is_some_and(|(scene, _)| Some(scene) == *at));
        if !all_there {
            let others: Vec<String> = slots.iter().filter_map(|(_, clip)| clip.clone()).filter(|clip| !clips.contains_key(clip)).collect();
            clips.extend(self.clips(&others, signal).await?);
            named = names_of(&clips);
        }
        let mut out = Vec::new();
        for number in numbers {
            let Some((_, at, names)) = wanted.iter().find(|(n, _, _)| n == number) else {
                out.push(json!({"gone": true}));
                continue;
            };
            match pick(&slots, &named, *at, names) {
                Some((scene, clip)) => {
                    let read = clips.get(&clip).cloned().unwrap_or(Value::Null);
                    if let Some(take) = set.take_mut(*number) {
                        take.place = json!({"scene": scene, "name": read.get("name").cloned().unwrap_or(Value::Null)});
                    }
                    out.push(json!({"ref": self.lengthen(&clip), "scene": scene, "track": track, "name": read.get("name"), "read": read}));
                }
                None => out.push(json!({"gone": true})),
            }
        }
        Ok(out)
    }

    async fn current(&self, _set: &LiveSet, row: &Value, signal: &Signal) -> Result<ClipTake, String> {
        if let Some(read) = row.get("read").filter(|read| read.is_object()) {
            return take_of(read);
        }
        let reference = row.get("ref").and_then(Value::as_str).ok_or("the take wasn't found")?;
        let read = self.clips(&[self.short(reference)], signal).await?;
        take_of(read.values().next().ok_or("Kumi couldn't read the take")?)
    }

    async fn rewrite(
        &self,
        set: &LiveSet,
        row: &Value,
        _before: &ClipTake,
        after: &ClipTake,
        signal: &Signal,
    ) -> Result<Rewritten<ClipTake>, (String, bool)> {
        // (Live adds no notes to a clip: the take's clip goes and is written again in its slot, with its name)
        let clip = row.get("ref").and_then(Value::as_str).ok_or(("the take wasn't found".to_string(), false))?;
        let track = row.get("track").and_then(Value::as_str).ok_or(("the take's track wasn't found".to_string(), false))?;
        let scene = row.get("scene").and_then(Value::as_u64).ok_or(("the take's slot wasn't found".to_string(), false))?;
        let name = row.get("name").and_then(Value::as_str).filter(|name| !name.is_empty()).unwrap_or(&set.source.short).to_string();
        let (deleted, _) = self.change("delete_clip", json!({"clipRef": self.short(clip)}), signal).await?;
        let input =
            json!({"trackRef": track, "sceneIndex": scene, "name": name, "length": round(after.length), "notes": notes_json(after)});
        match self.change("write_midi_clip", input, signal).await {
            Ok((written, _)) => Ok((format!("{deleted}, {written}"), None)),
            // (the take is gone until its delete is undone: the request stops there)
            Err((why, _)) => Err((format!("{why} (its clip was deleted first: undo_change {deleted} puts it back)"), true)),
        }
    }

    async fn mark_kept(&self, set: &LiveSet, number: usize, row: &Value, signal: &Signal) -> Result<Option<String>, String> {
        let [made, kept] = names(number, &set.source.short);
        // (a take the producer named otherwise keeps its name)
        if row.get("name").and_then(Value::as_str) != Some(made.as_str()) {
            return Ok(None);
        }
        let clip = row.get("ref").and_then(Value::as_str).ok_or("the take wasn't found")?;
        let (id, _) =
            self.change("rename", json!({"kind": "clip", "ref": self.short(clip), "name": kept}), signal).await.map_err(|(why, _)| why)?;
        Ok(Some(id))
    }

    async fn place(&self, _set: &mut LiveSet, _number: usize, _row: &Value, _at: &JsonObject, _signal: &Signal) -> Result<Value, String> {
        Err("in Live a kept take stays in its slot: drag it where it goes".into())
    }

    async fn undroppable(&self, _set: &LiveSet, _number: usize, _row: &Value, _signal: &Signal) -> Option<Undroppable> {
        None
    }

    async fn drop(&self, set: &LiveSet, rows: &[(usize, Value)], signal: &Signal) -> Result<(String, Value), String> {
        // (one change a clip: Live deletes a clip at a time)
        let mut ids = Vec::new();
        let mut dropped = Vec::new();
        for (number, row) in rows {
            let clip = row.get("ref").and_then(Value::as_str).ok_or("a take without its clip")?;
            match self.change("delete_clip", json!({"clipRef": self.short(clip)}), signal).await {
                Ok((id, _)) => {
                    ids.push(id);
                    dropped.push(format!("V{number}"));
                }
                Err((why, _)) if ids.is_empty() => return Err(why),
                Err((why, _)) => {
                    return Err(format!(
                        "{why} (dropped already: {} in {}; undo_change brings each back)",
                        dropped.join(", "),
                        ids.join(", ")
                    ))
                }
            }
        }
        // (a scene Kumi added for a take stays, now empty: deleting it would leave its clip's undo nowhere to go)
        let scenes = self.rows("scene", json!({"fields": ["index", "name", "isEmpty"]}), signal).await.unwrap_or_default();
        let left: Vec<String> = rows
            .iter()
            .filter_map(|(number, row)| {
                let at = row.get("scene").and_then(Value::as_u64)?;
                let scene = scenes.iter().find(|scene| scene.get("index").and_then(Value::as_u64) == Some(at))?;
                let named = scene.get("name").and_then(Value::as_str)?;
                (scene.get("isEmpty") == Some(&Value::Bool(true)) && named == names(*number, &set.source.short)[0])
                    .then(|| format!("\u{201c}{named}\u{201d}"))
            })
            .collect();
        let mut display = json!({"title": format!("Deleted {} takes", rows.len())});
        if !left.is_empty() {
            display["note"] = json!(format!(
                "The scene{} Kumi added for {} stay{}, empty ({}): later takes go there, or the producer deletes {} in Live.",
                if left.len() == 1 { "" } else { "s" },
                if left.len() == 1 { "it" } else { "them" },
                if left.len() == 1 { "s" } else { "" },
                left.join(", "),
                if left.len() == 1 { "it" } else { "them" }
            ));
        }
        Ok((ids.join(", "), json!({"display": display})))
    }

    fn cant_render(&self) -> Option<String> {
        Some("Live plays a MIDI clip through its instrument only live".into())
    }

    async fn playing(&self, _signal: &Signal) -> Option<bool> {
        self.connection.playing()
    }

    async fn render(&self, _set: &mut LiveSet, _numbers: &[usize], _with_source: bool, _signal: &Signal) -> Result<Rendered, String> {
        Err("Live renders no MIDI clip offline".into())
    }

    fn short(&self, long: &str) -> String {
        self.connection.references.borrow_mut().short_ref(long)
    }

    fn end_note(&self, set: &LiveSet) -> Option<String> {
        Some(format!(
            "The takes are in the slots below \u{201c}{}\u{201d} on its track: launch them to hear them.",
            set.source.data.clip_name
        ))
    }

    async fn extract(&self, _input: &JsonObject, _signal: &Signal) -> Result<Value, String> {
        Err("Live's variations have no extract".into())
    }

    fn material_json(&self, material: &ClipTake) -> Value {
        json!({"length": material.length, "notes": material.notes.iter().map(Note::json).collect::<Vec<_>>(), "anchors": material.anchors.iter().map(Note::json).collect::<Vec<_>>()})
    }

    fn material_of(&self, value: &Value) -> Option<ClipTake> {
        let notes = |key: &str| value.get(key).and_then(Value::as_array).map(|list| list.iter().filter_map(Note::of).collect::<Vec<_>>());
        Some(ClipTake { length: value.get("length")?.as_f64()?, notes: notes("notes")?, anchors: notes("anchors").unwrap_or_default() })
    }

    fn data_json(&self, data: &LiveData) -> Value {
        json!({"track": data.track, "trackId": data.track_id, "trackName": data.track_name, "scene": data.scene, "clipName": data.clip_name})
    }

    fn data_of(&self, value: &Value) -> Option<LiveData> {
        Some(LiveData {
            track: value.get("track")?.as_str()?.to_string(),
            track_id: value.get("trackId").and_then(Value::as_str).map(str::to_owned),
            track_name: value.get("trackName")?.as_str()?.to_string(),
            scene: value.get("scene")?.as_u64()?,
            clip_name: value.get("clipName")?.as_str()?.to_string(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn clip(notes: Value, looped: Value) -> Value {
        json!({"clip": "clip:1", "name": "Arp", "notes": notes, "loop": looped})
    }

    #[test]
    fn a_clip_read_is_a_take_of_the_unmuted_notes_its_loop_plays() {
        let notes = json!([
            {"id": 1, "pitch": 60, "start": 0, "duration": 0.5, "velocity": 100, "mute": false},
            {"id": 2, "pitch": 64, "start": 0.5, "duration": 0.5, "velocity": 90.5, "mute": true},
            {"id": 3, "pitch": 67, "start": 1, "duration": 0.25, "velocity": 80, "channel": 2},
            {"id": 4, "pitch": 72, "start": 4, "duration": 0.25, "velocity": 80}
        ]);
        let take = take_of(&clip(notes.clone(), json!({"looping": true, "loopStart": 0, "loopEnd": 4, "length": 4}))).unwrap();
        assert_eq!(
            take.notes.iter().map(|note| note.pitch).collect::<Vec<_>>(),
            [60, 67],
            "the muted note and the one past the loop left out"
        );
        assert_eq!(take.length, 4.);
        assert_eq!(notes_json(&take)[1], json!({"pitch": 67, "start": 1.0, "duration": 0.25, "velocity": 80, "channel": 2}));
        // (a loop from beat 1: the notes from its start, as the clip plays them)
        let take = take_of(&clip(notes.clone(), json!({"looping": true, "loopStart": 1, "loopEnd": 5, "length": 4}))).unwrap();
        assert_eq!(take.notes.iter().map(|note| (note.pitch, note.start)).collect::<Vec<_>>(), [(67, 0.), (72, 3.)]);
        // (a clip that doesn't loop plays from its start marker to its end marker)
        let take =
            take_of(&clip(notes, json!({"looping": false, "loopStart": 0, "loopEnd": 8, "startMarker": 0, "endMarker": 2, "length": 2})))
                .unwrap();
        assert_eq!((take.notes.len(), take.length), (2, 2.));
        assert!(take_of(&json!({"error": "gone"})).is_err());
        assert!(take_of(&json!({"name": "Big", "notes": [], "truncated": "only its first 2000 notes were read"}))
            .unwrap_err()
            .contains("whole"));
        assert_eq!(scene_of("1:clip:3:5"), Some(5));
        assert_eq!(clip_at("7:track:2", 9).as_deref(), Some("7:clip:2:9"));
    }

    #[test]
    fn a_take_read_back_in_lives_order_is_the_take_kumi_wrote_and_a_loosened_note_stays_inside_the_clip() {
        let note = |pitch: i64, start: f64, length: f64| Note { pitch, start, length, velocity: 100, channel: 1 };
        let written = ClipTake { notes: vec![note(64, 1., 0.01), note(60, 1., 0.5), note(67, 0., 0.25)], length: 4., anchors: vec![] };
        // (Live's order: by start, then pitch; a short note as written)
        let read = take_of(&clip(
            json!([
                {"pitch": 67, "start": 0, "duration": 0.25, "velocity": 100},
                {"pitch": 60, "start": 1.00000001, "duration": 0.5, "velocity": 100},
                {"pitch": 64, "start": 1, "duration": 0.01, "velocity": 100}
            ]),
            json!({"looping": true, "loopStart": 0, "loopEnd": 4}),
        ))
        .unwrap();
        assert_eq!(digest_of(&read), digest_of(&written));
        let mut take = ClipTake { notes: vec![note(60, 3.99, 0.25)], length: 4., anchors: vec![] };
        let source = take.clone();
        let mut rng = Rng::new(1);
        for _ in 0..8 {
            note_op("timing", &json!({"op": "timing", "up": true}), 1., &mut take.notes, &source.anchors, &mut rng);
            kept_inside(&mut take);
            assert!(take.notes[0].start < 4., "{:?}", take.notes);
        }
    }

    #[test]
    fn a_take_goes_below_its_source_in_a_scene_with_no_other_clip_and_is_found_where_it_was_left_or_by_its_name() {
        let slots = vec![(0, Some("c0".to_string())), (1, None), (2, Some("c2".into())), (3, None), (4, Some("c4".into()))];
        let empty: BTreeSet<u64> = [1, 3].into();
        assert_eq!(free_below(&slots, 0, &empty), Some(1));
        assert_eq!(free_below(&slots, 1, &empty), Some(3));
        assert_eq!(free_below(&slots, 3, &empty), None, "a scene is added");
        // (a section of the song: its other tracks' clips play with an empty slot there, which stops this track)
        assert_eq!(free_below(&slots, 0, &[3].into()), Some(3));
        let named: BTreeMap<String, String> =
            [("c0", "Arp"), ("c2", "V1 \u{b7} Arp"), ("c4", "V2 \u{b7} Arp")].iter().map(|(c, n)| (c.to_string(), n.to_string())).collect();
        let wanted = |name: &str| [format!("{name} \u{b7} kept"), name.to_string()];
        assert_eq!(pick(&slots, &named, Some(2), &wanted("V1 \u{b7} Arp")), Some((2, "c2".into())));
        // (moved: found by its name)
        assert_eq!(pick(&slots, &named, Some(1), &wanted("V2 \u{b7} Arp")), Some((4, "c4".into())));
        assert_eq!(pick(&slots, &named, Some(4), &wanted("V3 \u{b7} Arp")), None);
    }

    /// Every direction Kumi picks makes a take of an ordinary clip: a make of five gets five takes (real Live: V3 "looser" moved
    /// notes a few hundredths of a beat, a near-copy, and "higher" and "lower" moved every note, too far).
    #[test]
    fn every_direction_kumi_picks_makes_a_take_of_an_ordinary_clip() {
        kumi_runtime::core::variations::notes::every_note_direction_makes_a_take(
            |notes, anchors| ClipTake { notes, length: 4., anchors },
            applied,
            |take| take.notes.as_slice(),
        );
    }

    /// The bridge's own schemas for each change a take makes, as every bridge in the fixtures has them.
    #[test]
    fn every_change_a_take_makes_fits_the_bridges_schema() {
        let oracle: Value = serde_json::from_str(&kumi_testkit::oracle("kumi-daw-ableton/public-tools-oracle.json")).unwrap();
        let take =
            ClipTake { notes: vec![Note { pitch: 60, start: 0., length: 0.5, velocity: 100, channel: 1 }], length: 4., anchors: vec![] };
        for (tool, input) in [
            (
                "write_midi_clip",
                json!({"trackRef": "1:track:2", "sceneIndex": 3, "name": "V1 \u{b7} Arp", "length": 4.0, "notes": notes_json(&take)}),
            ),
            ("delete_clip", json!({"clipRef": "1:clip:2:3"})),
            ("rename", json!({"kind": "clip", "ref": "1:clip:2:3", "name": "V1 \u{b7} Arp \u{b7} kept"})),
            ("add_tracks_and_scenes", json!({"tracks": [], "scenes": [{"name": "V1 \u{b7} Arp"}]})),
        ] {
            let kind = CHANGES.iter().find(|kind| kind.tool == tool).unwrap_or_else(|| panic!("{tool}"));
            let schemas: Vec<&Value> = oracle["pool"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|listed| listed["name"] == kind.preview.as_str())
                .map(|listed| &listed["inputSchema"])
                .collect();
            assert!(!schemas.is_empty(), "{tool}: {} isn't in the fixtures", kind.preview);
            for schema in schemas {
                assert_eq!(kumi_runtime::kernel::schema::problem(schema, &input), None, "{tool}: {input}");
            }
        }
    }
}
