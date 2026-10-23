//! Groove in REAPER and Renoise: a part's notes as the DAW plays them, each with an id Kumi moves it by, and a round's
//! move made as one change. REAPER's part is a MIDI item (its active take): notes by their index in the take, in quarter
//! notes from the item's start, moved with notes_change. Renoise's is a pattern track: each note cell a note that lasts
//! to the next note or OFF in its column, its velocity the volume column (00–80; empty is full); a move changes the
//! note's delay within its line, or takes it to the line before or after when that cell of its column is free (else as
//! far as its own line goes), with notes_write.

use crate::listening::OwnRenderer;
use kumi_agent::core::refs::RefScheme;
use kumi_common::abort::Signal;
use kumi_listening::{
    notes::{Kit, Note},
    renderer::{ClipNotes, NotePatch, Renderer, Window},
};
use kumi_types::contracts::ChangeState;
use kumi_types::errors::RuntimeError;
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap, HashSet};

/// The pages of notes groove reads of a part (200 a page).
const NOTE_PAGES: usize = 50;
/// The most note cells one Renoise change moves (a step writes about 3,000).
const MOST_CELLS: usize = 2_500;
/// The tools that edit a part's notes: a groove round takes back only these, on its own part.
const NOTE_TOOLS: [&str; 5] = ["notes_add", "notes_change", "notes_delete", "notes_write", "notes_clear"];
/// The most FX or samples read for a part's drum kit.
const MOST_PADS: usize = 64;

/// A part groove read: what its note edits are kept under, and what moving its notes needs.
#[derive(Debug, Clone)]
pub enum Part {
    /// A MIDI item: its active take's ref (what a note edit's change names) and its track's.
    Item { take: String, track: Option<String> },
    /// A pattern track: the pattern's ref and the track's (the pattern an alias plays from, where it is one), the pattern
    /// track's ref (what a note edit's change names), the song's lines per beat and the pattern's lines, its note cells
    /// by id and every cell of its note columns that holds something.
    Lines { pattern: String, track: String, place: String, lpb: f64, lines: u64, cells: HashMap<i64, Cell>, taken: HashSet<(u64, u64)> },
}

/// A note cell of a pattern track as the line read gives it (a field Renoise shows as ".." left out).
#[derive(Debug, Clone, PartialEq)]
pub struct Cell {
    line: u64,
    col: u64,
    note: String,
    instrument: Option<String>,
    volume: Option<String>,
    panning: Option<String>,
    delay: u32,
    fx: Option<String>,
}

/// A note's number from Renoise's text ("C-4" is 48, "C#4" 49); None for "OFF", "---" and anything else.
fn note_number(text: &str) -> Option<i32> {
    let chars: Vec<char> = text.chars().collect();
    let [name, sharp, octave] = chars[..] else { return None };
    let base = match name.to_ascii_uppercase() {
        'C' => 0,
        'D' => 2,
        'E' => 4,
        'F' => 5,
        'G' => 7,
        'A' => 9,
        'B' => 11,
        _ => return None,
    };
    let sharp = match sharp {
        '-' => 0,
        '#' => 1,
        _ => return None,
    };
    Some(octave.to_digit(10)? as i32 * 12 + base + sharp)
}

/// The velocity a volume column plays a note at: empty is full (127), 00–80 scales to it. None for an effect there,
/// which Kumi leaves as it is.
fn velocity_of(volume: Option<&str>) -> Option<f64> {
    match volume {
        None => Some(127.),
        Some(text) => u32::from_str_radix(text, 16).ok().filter(|value| *value <= 0x80).map(|value| (value as f64 * 127. / 128.).round()),
    }
}

/// The volume column for a velocity: two hex digits, 80 for full.
fn volume_of(velocity: f64) -> String {
    format!("{:02X}", ((velocity.clamp(0., 127.) * 128. / 127.).round() as u32).min(0x80))
}

fn field(cell: &Value, key: &str) -> Option<String> {
    cell.get(key).and_then(Value::as_str).filter(|text| *text != "..").map(str::to_owned)
}

/// A pattern track's notes from its line rows (lines by Renoise's numbers, from 0): each note cell by its id (its line × 16
/// + its column), with its start
/// and length in beats (to the next note or OFF in its column, or the pattern's end), and every cell that holds
/// something.
pub fn cells_of(rows: &[Value], lines: u64, lpb: f64) -> (Vec<(i64, Cell, Note)>, HashSet<(u64, u64)>) {
    let mut columns: BTreeMap<u64, Vec<(f64, Option<Cell>)>> = BTreeMap::new();
    let mut taken = HashSet::new();
    for row in rows {
        let Some(line) = row.get("line").and_then(Value::as_u64) else { continue };
        for cell in row.get("notes").and_then(Value::as_array).into_iter().flatten() {
            let Some(col) = cell.get("col").and_then(Value::as_u64) else { continue };
            taken.insert((line, col));
            let note = cell.get("note").and_then(Value::as_str).unwrap_or("---");
            let delay = field(cell, "delay").and_then(|text| u32::from_str_radix(&text, 16).ok()).unwrap_or(0).min(255);
            let at = line as f64 + delay as f64 / 256.;
            if note == "OFF" {
                columns.entry(col).or_default().push((at, None));
            } else if note_number(note).is_some() {
                let found = Cell {
                    line,
                    col,
                    note: note.to_owned(),
                    instrument: field(cell, "instrument"),
                    volume: field(cell, "volume"),
                    panning: field(cell, "panning"),
                    delay,
                    fx: field(cell, "fx"),
                };
                columns.entry(col).or_default().push((at, Some(found)));
            }
        }
    }
    let mut notes = vec![];
    for events in columns.values_mut() {
        events.sort_by(|a, b| a.0.total_cmp(&b.0));
        for (index, (at, cell)) in events.iter().enumerate() {
            let Some(cell) = cell else { continue };
            let end = events.get(index + 1).map_or(lines as f64, |(next, _)| *next);
            let note = Note {
                start: at / lpb,
                length: ((end - at) / lpb).max(1. / 256. / lpb),
                pitch: note_number(&cell.note).unwrap_or(48),
                velocity: velocity_of(cell.volume.as_deref()).unwrap_or(127.),
            };
            notes.push((cell.line as i64 * 16 + cell.col as i64, cell.clone(), note));
        }
    }
    notes.sort_by(|a, b| a.2.start.total_cmp(&b.2.start).then(a.0.cmp(&b.0)));
    (notes, taken)
}

/// The lines notes_write writes to move a pattern track's notes as the patches say: each note to its new line and delay,
/// a line away at most and only into a cell of its column that holds nothing (else as far as its own line goes), and
/// its volume for its velocity (one holding an effect stays). Each cell is written whole, as it was read but for those.
pub fn writes_for(cells: &HashMap<i64, Cell>, taken: &HashSet<(u64, u64)>, patches: &[NotePatch], lpb: f64, lines: u64) -> Vec<Value> {
    let mut entries: BTreeMap<u64, Vec<Value>> = BTreeMap::new();
    let mut claimed: HashSet<(u64, u64)> = HashSet::new();
    for patch in patches {
        let Some(cell) = cells.get(&patch.id) else { continue };
        let wanted = (patch.start.max(0.) * lpb).min(lines as f64 - 1. / 256.);
        let (mut line, mut delay) = (wanted.floor() as u64, ((wanted - wanted.floor()) * 256.).round() as u32);
        if delay >= 256 {
            (line, delay) = (line + 1, 0);
        }
        if line >= lines {
            (line, delay) = (lines - 1, 255);
        }
        if line != cell.line {
            let free = line.abs_diff(cell.line) == 1 && !taken.contains(&(line, cell.col)) && !claimed.contains(&(line, cell.col));
            if !free {
                (line, delay) = if line < cell.line { (cell.line, 0) } else { (cell.line, 255) };
            }
        }
        let volume = match velocity_of(cell.volume.as_deref()) {
            Some(was) if (was - patch.velocity.round()).abs() >= 1. => Some(volume_of(patch.velocity)),
            _ => cell.volume.clone(),
        };
        if line == cell.line && delay == cell.delay && volume == cell.volume {
            continue;
        }
        let mut written = json!({"col": cell.col, "note": cell.note});
        for (key, value) in [("instrument", &cell.instrument), ("volume", &volume), ("panning", &cell.panning), ("fx", &cell.fx)] {
            if let Some(value) = value {
                written[key] = value.clone().into();
            }
        }
        if delay > 0 {
            written["delay"] = format!("{delay:02X}").into();
        }
        if line != cell.line {
            claimed.insert((line, cell.col));
            entries.entry(cell.line).or_default().push(json!({"col": cell.col, "clear": true}));
        }
        entries.entry(line).or_default().push(written);
    }
    entries.into_iter().map(|(line, notes)| json!({"line": line, "notes": notes})).collect()
}

/// A positional ref's numbers ("1:ptrack:3.2" → [3, 2]).
fn numbers(reference: &str) -> Vec<u64> {
    reference.rsplit(':').next().unwrap_or_default().split('.').filter_map(|n| n.parse().ok()).collect()
}

/// The epoch a positional ref was made in ("1:ptrack:3.2" → "1").
fn epoch_of(reference: &str) -> &str {
    reference.split(':').next().unwrap_or_default()
}

impl<S: RefScheme + 'static> OwnRenderer<S> {
    fn long(&self, named: &str) -> String {
        self.link.book.borrow().lengthen(&json!({"ref": named}))["ref"].as_str().unwrap_or(named).to_owned()
    }

    fn kind_of(&self, long: &str) -> Option<String> {
        self.link.book.borrow().scheme.kind_of(long).map(str::to_owned)
    }

    /// A part's notes, each with the id Kumi moves it by: the ones the DAW plays (not muted, inside the item or pattern).
    pub async fn part_notes(&self, clip: &str, signal: Signal) -> Result<ClipNotes, String> {
        if self.link.book.borrow().scheme.positional() {
            self.pattern_notes(clip, &signal).await
        } else {
            self.item_notes(clip, &signal).await
        }
    }

    /// The item a MIDI item's or take's ref names, as the item read gives it (a take is found as its item's active one).
    pub async fn item_row(&self, named: &str, signal: &Signal) -> Result<Value, String> {
        let long = self.long(named);
        let vocab = &self.profile().vocab;
        let cant = |why: String| why;
        match self.kind_of(&long).as_deref() {
            Some("item") => match self.rows(json!({"ref": long}), 1, signal).await.map_err(|error| error.to_string())? {
                Ok(rows) => rows.into_iter().next().ok_or_else(|| format!("{named} isn't in the {} now", self.profile().words.place)),
                Err(why) => Err(cant(why)),
            },
            Some("take") => {
                let tracks = self.track_rows(signal).await.map_err(|error| error.to_string())??;
                for track in tracks.iter().take(1_000) {
                    let rows = self.rows(json!({"kind": "item", "parent": track.reference, "limit": 200}), 5, signal).await;
                    let Ok(Ok(rows)) = rows else { continue };
                    if let Some(row) = rows.into_iter().find(|row| row.get("takeRef").and_then(Value::as_str) == Some(long.as_str())) {
                        return Ok(row);
                    }
                }
                Err(format!("{named} isn't any item's active take, so it doesn't play: give its item's {}", vocab.clip_ref))
            }
            _ => Err(format!("{named} isn't {}: give {}", vocab.midi_clip, vocab.part_ref)),
        }
    }

    async fn item_notes(&self, clip: &str, signal: &Signal) -> Result<ClipNotes, String> {
        let row = self.item_row(clip, signal).await?;
        let name = row
            .get("name")
            .and_then(Value::as_str)
            .filter(|name| !name.is_empty())
            .map_or("the MIDI item".to_owned(), |name| format!("“{name}”"));
        let take = row.get("takeRef").and_then(Value::as_str).ok_or_else(|| format!("{name} has no take"))?.to_owned();
        if row.get("source").and_then(Value::as_str) != Some("mid") {
            return Err(format!("{name} isn't MIDI: groove reads {}'s notes", self.profile().vocab.midi_clip));
        }
        let length = row.get("lengthBeats").and_then(Value::as_f64).unwrap_or(f64::INFINITY);
        let rows = self
            .rows(json!({"kind": "note", "parent": take, "limit": 200}), NOTE_PAGES, signal)
            .await
            .map_err(|error| error.to_string())??;
        let notes: Vec<(i64, Note)> = rows
            .iter()
            .filter(|row| row.get("muted") != Some(&Value::Bool(true)))
            .filter_map(|row| {
                let note = Note {
                    start: row.get("start")?.as_f64()?,
                    length: row.get("length").and_then(Value::as_f64).unwrap_or(0.25),
                    pitch: row.get("pitch")?.as_i64()? as i32,
                    velocity: row.get("velocity").and_then(Value::as_f64).unwrap_or(100.),
                };
                Some((row.get("index")?.as_i64()?, note))
            })
            .filter(|(_, note)| note.start >= -1e-9 && note.start < length - 1e-9)
            .collect();
        if notes.is_empty() {
            return Err(format!("{name} has no notes to measure."));
        }
        let track = row.get("trackRef").and_then(Value::as_str).map(str::to_owned);
        self.parts.borrow_mut().insert(clip.to_owned(), Part::Item { take, track });
        // REAPER refuses no note past the item's end: it just doesn't play it.
        Ok(ClipNotes { name, notes, song_zero: row.get("positionBeats").and_then(Value::as_f64), to: length, end: f64::INFINITY })
    }

    /// The pattern and track a Renoise part names: a pattern track's ref, or a pattern's (or sequence slot's) ref and a
    /// track's.
    async fn pattern_track(&self, clip: &str, signal: &Signal) -> Result<(u64, u64, String), String> {
        let vocab = &self.profile().vocab;
        let wrong = || format!("{clip} isn't {}", vocab.part_ref);
        let named: Vec<String> = clip.split([' ', ',', '/']).filter(|part| !part.is_empty()).map(|part| self.long(part)).collect();
        let (mut pattern, mut track, mut epoch) = (None, None, String::new());
        for long in &named {
            let at = numbers(long);
            match (self.kind_of(long).as_deref(), at.as_slice()) {
                (Some("ptrack"), [p, t]) => (pattern, track) = (Some(*p), Some(*t)),
                (Some("pattern"), [p]) => pattern = Some(*p),
                (Some("track"), [t]) => track = Some(*t),
                (Some("seq"), [_]) => {
                    let rows = self.rows(json!({"ref": long}), 1, signal).await.map_err(|error| error.to_string())??;
                    let of = rows.first().and_then(|row| row.get("patternRef")).and_then(Value::as_str).ok_or_else(wrong)?;
                    pattern = numbers(of).first().copied();
                }
                _ => return Err(wrong()),
            }
            epoch = epoch_of(long).to_owned();
        }
        match (pattern, track) {
            (Some(pattern), Some(track)) => Ok((pattern, track, epoch)),
            _ => Err(wrong()),
        }
    }

    async fn pattern_notes(&self, clip: &str, signal: &Signal) -> Result<ClipNotes, String> {
        let error = |error: RuntimeError| error.to_string();
        let (p, t, epoch) = self.pattern_track(clip, signal).await?;
        let track = format!("{epoch}:track:{t}");
        let song = self.rows(json!({"kind": "song"}), 1, signal).await.map_err(error)??;
        let lpb = song.first().and_then(|row| row.get("lpb")).and_then(Value::as_f64).filter(|lpb| *lpb >= 1.).unwrap_or(4.);
        let pattern_rows = self.rows(json!({"ref": format!("{epoch}:pattern:{p}")}), 1, signal).await.map_err(error)??;
        // (the pattern as Renoise numbers it, from 00)
        let shown = format!("pattern {:02}", p.saturating_sub(1));
        let row = pattern_rows.first().ok_or_else(|| format!("there's no {shown}"))?;
        // An alias plays another pattern's lines: those are read and written (the row numbers it as Renoise does).
        let from = row.pointer(&format!("/aliases/{t}")).and_then(Value::as_u64).map_or(p, |alias| alias + 1);
        let pattern = format!("{epoch}:pattern:{from}");
        let label = row.get("name").and_then(Value::as_str).filter(|name| !name.is_empty()).map_or(shown, |name| format!("“{name}”"));
        let lines_row = if from == p {
            row.clone()
        } else {
            self.rows(json!({"ref": pattern}), 1, signal).await.map_err(error)??.into_iter().next().unwrap_or_default()
        };
        let lines = lines_row.get("lines").and_then(Value::as_u64).unwrap_or(64);
        let rows =
            self.rows(json!({"kind": "line", "parent": pattern, "track": track, "limit": 200}), 8, signal).await.map_err(error)??;
        let (mut found, taken) = cells_of(&rows, lines, lpb);
        // A muted note column plays nothing: its notes aren't the part's (its cells still take their places).
        let first = self
            .link
            .call("read", self.read_placed(json!({"kind": "line", "parent": pattern, "track": track, "limit": 1})), signal.clone())
            .await;
        let muted: Vec<u64> = match first {
            Ok(Ok(page)) => page.get("mutedColumns").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_u64).collect(),
            _ => vec![],
        };
        found.retain(|(_, cell, _)| !muted.contains(&cell.col));
        let tracks = self.track_rows(signal).await.map_err(error)??;
        let track_name =
            tracks.iter().find(|row| self.same_place(&row.reference, &track)).map_or(format!("track {t}"), |row| format!("“{}”", row.name));
        let name = format!("{track_name} in {label}");
        if found.is_empty() {
            return Err(format!("{name} has no notes to measure."));
        }
        let song_zero = self.pattern_start(p, lpb, signal).await;
        let notes = found.iter().map(|(id, _, note)| (*id, *note)).collect();
        let cells = found.into_iter().map(|(id, cell, _)| (id, cell)).collect();
        let place = format!("{epoch}:ptrack:{from}.{t}");
        self.parts.borrow_mut().insert(clip.to_owned(), Part::Lines { pattern, track, place, lpb, lines, cells, taken });
        let end = lines as f64 / lpb;
        Ok(ClipNotes { name, notes, song_zero, to: end, end })
    }

    /// Where a Renoise pattern (or a pattern track) plays in the song, and for how long: over its first slot in the
    /// pattern sequence. None for anything else, or a pattern no slot plays.
    pub async fn pattern_window(&self, long: &str, signal: &Signal) -> Option<Window> {
        if !matches!(self.kind_of(long).as_deref(), Some("pattern" | "ptrack")) {
            return None;
        }
        let (pattern, epoch) = (*numbers(long).first()?, epoch_of(long).to_owned());
        let song = self.rows(json!({"kind": "song"}), 1, signal).await.ok()?.ok()?;
        let lpb = song.first()?.get("lpb")?.as_f64().filter(|lpb| *lpb >= 1.)?;
        let row = self.rows(json!({"ref": format!("{epoch}:pattern:{pattern}")}), 1, signal).await.ok()?.ok()?;
        let lines = row.first()?.get("lines")?.as_f64()?;
        let from = self.pattern_start(pattern, lpb, signal).await?;
        Some(Window { from, beats: lines / lpb })
    }

    /// Where a pattern first plays in the song, in beats: its first slot in the pattern sequence (None when no slot
    /// plays it).
    async fn pattern_start(&self, pattern: u64, lpb: f64, signal: &Signal) -> Option<f64> {
        let slots = self.rows(json!({"kind": "sequence"}), 5, signal).await.ok()?.ok()?;
        let at =
            slots.iter().position(|slot| slot.get("patternRef").and_then(Value::as_str).map(numbers).as_deref() == Some(&[pattern]))?;
        if let Some(beats) = slots[at].get("positionBeats").and_then(Value::as_f64) {
            return Some(beats);
        }
        let patterns = self.rows(json!({"kind": "pattern"}), 5, signal).await.ok()?.ok()?;
        let lines: HashMap<u64, u64> =
            patterns.iter().filter_map(|row| Some((*numbers(row.get("ref")?.as_str()?).first()?, row.get("lines")?.as_u64()?))).collect();
        let before: u64 =
            slots[..at].iter().filter_map(|slot| lines.get(numbers(slot.get("patternRef")?.as_str()?).first()?).copied()).sum();
        Some(before as f64 / lpb)
    }

    /// Moves a part's notes in one change: notes_change of the take's notes by index (REAPER), notes_write of the pattern
    /// track's lines (Renoise).
    pub async fn move_notes(&self, clip: &str, patches: &[NotePatch], signal: Signal) -> Result<(), RuntimeError> {
        if !self.parts.borrow().contains_key(clip) {
            self.part_notes(clip, signal.clone()).await.map_err(RuntimeError::plain)?;
        }
        let part = self.parts.borrow().get(clip).cloned().ok_or_else(|| RuntimeError::plain("Kumi couldn't read the part's notes."))?;
        let (tool, input) = match part {
            Part::Item { take, .. } => {
                let changes: Vec<Value> = patches
                    .iter()
                    .map(|patch| {
                        let mut change = json!({
                            "index": patch.id,
                            "start": (patch.start.max(0.) * 10_000.).round() / 10_000.,
                            "velocity": patch.velocity.round().clamp(1., 127.) as i64,
                        });
                        if let Some(length) = patch.duration.filter(|length| *length > 0.) {
                            change["length"] = ((length * 10_000.).round() / 10_000.).max(0.0001).into();
                        }
                        change
                    })
                    .collect();
                ("notes_change", json!({"ref": take, "changes": changes}))
            }
            Part::Lines { pattern, track, lpb, lines, cells, taken, .. } => {
                if patches.len() > MOST_CELLS {
                    return Err(RuntimeError::plain(format!(
                        "That's {} notes, more than Kumi moves in one change of Renoise's ({MOST_CELLS}): use a shorter pattern.",
                        patches.len()
                    )));
                }
                let lines = writes_for(&cells, &taken, patches, lpb, lines);
                if lines.is_empty() {
                    return Err(RuntimeError::plain(
                        "The pattern's lines leave these notes no room to move: their columns are full around them.",
                    ));
                }
                ("notes_write", json!({"patternRef": pattern, "trackRef": track, "lines": lines}))
            }
        };
        self.change(tool, input, signal).await.map(|_| ()).map_err(RuntimeError::plain)
    }

    /// The ref the history keeps a part's note edits under: its take's (REAPER), its pattern track's (Renoise).
    pub fn part_id(&self, clip: &str) -> Option<String> {
        self.parts.borrow().get(clip).map(|part| match part {
            Part::Item { take, .. } => take.clone(),
            Part::Lines { place, .. } => place.clone(),
        })
    }

    /// The changes applied since a checkpoint, split into the note edits on the part (by the ref its changes name) and the
    /// rest.
    pub fn part_changes(&self, checkpoint: &[String], clip: Option<&str>) -> (Vec<(String, String)>, Vec<(String, String)>) {
        let entries = self.plan.history.entries.borrow();
        self.applied_since(checkpoint).into_iter().partition(|(id, _)| {
            entries.get(id).is_some_and(|entry| {
                let entry = entry.borrow();
                let edit = entry.tool.as_deref().is_some_and(|tool| NOTE_TOOLS.contains(&tool));
                edit && clip.zip(entry.clip.as_deref()).is_some_and(|(part, on)| self.same_place(part, on))
            })
        })
    }

    /// How a part's notes fall into lanes: by its track's drum kit, when it plays one (REAPER: ReaSamplOmatic5000s, one
    /// note each; Renoise: the part's instrument with a sample on each of its keys).
    pub async fn part_lanes(&self, clip: &str, signal: Signal) -> Kit {
        let part = self.parts.borrow().get(clip).cloned();
        let pads = match part {
            Some(Part::Item { track: Some(track), .. }) => self.sampler_pads(&track, &signal).await,
            Some(Part::Lines { pattern, cells, .. }) => self.keyzone_pads(epoch_of(&pattern), &cells, &signal).await,
            _ => vec![],
        };
        if pads.len() >= 2 {
            Kit::rack(&pads)
        } else {
            Kit::pitched()
        }
    }

    /// A REAPER track's sampler pads: each FX playing one note (its note range starts and ends there), by its name.
    async fn sampler_pads(&self, track: &str, signal: &Signal) -> Vec<(String, u8)> {
        let Ok(Ok(devices)) = self.devices(track, signal).await else { return vec![] };
        let mut pads = vec![];
        for device in devices.iter().take(MOST_PADS) {
            let Ok(Ok(params)) = self.parameters(&device.reference, false, signal).await else { continue };
            let note = |name: &str| {
                params
                    .iter()
                    .find(|row| row.get("name").and_then(Value::as_str) == Some(name))
                    .and_then(|row| row.get("value")?.as_f64())
                    .map(|value| (value * 127.).round() as u8)
            };
            if let (Some(start), Some(end)) = (note("Note range start"), note("Note range end")) {
                if start == end {
                    pads.push((crate::listening::plugin_name(&device.name).to_owned(), start));
                }
            }
        }
        pads
    }

    /// A Renoise part's drum pads: the samples of the instrument most of its notes play, each on one key.
    async fn keyzone_pads(&self, epoch: &str, cells: &HashMap<i64, Cell>, signal: &Signal) -> Vec<(String, u8)> {
        let mut counts: HashMap<&str, usize> = HashMap::new();
        for cell in cells.values() {
            *counts.entry(cell.instrument.as_deref().unwrap_or("..")).or_default() += 1;
        }
        let Some((most, _)) = counts.into_iter().filter(|(text, _)| *text != "..").max_by_key(|(_, count)| *count) else { return vec![] };
        let Ok(index) = u64::from_str_radix(most, 16) else { return vec![] };
        let parent = format!("{epoch}:instrument:{}", index + 1);
        let Ok(Ok(samples)) = self.rows(json!({"kind": "sample", "parent": parent}), 1, signal).await else { return vec![] };
        samples
            .iter()
            .take(MOST_PADS)
            .filter_map(|row| {
                let range = row.get("noteRange")?.as_array()?;
                let (low, high) = (range.first()?.as_u64()?, range.get(1)?.as_u64()?);
                (low == high && low <= 127).then(|| (row.get("name").and_then(Value::as_str).unwrap_or_default().to_owned(), low as u8))
            })
            .collect()
    }

    /// The track a drum stem plays on, by name: an audio item's (REAPER), or the track itself (Renoise plays a stem from a
    /// pattern track).
    pub async fn stem_track_of(&self, audio: &str, signal: Signal) -> Result<String, String> {
        let long = self.long(audio);
        let tracks = self.track_rows(&signal).await.map_err(|error| error.to_string())??;
        let track = match self.kind_of(&long).as_deref() {
            Some("track") => Some(long),
            Some("item" | "take") => self.item_row(audio, &signal).await?.get("trackRef").and_then(Value::as_str).map(str::to_owned),
            _ => tracks.iter().find(|row| row.name == audio).map(|row| row.reference.clone()),
        };
        let vocab = &self.profile().vocab;
        track
            .and_then(|track| tracks.iter().find(|row| self.same_place(&row.reference, &track)).map(|row| row.name.clone()))
            .ok_or_else(|| format!("{audio} isn't what plays the drum stem: give {}", vocab.stem_audio))
    }

    /// How many devices these changes added.
    pub fn devices_added(&self, changes: &[(String, String)]) -> usize {
        let entries = self.plan.history.entries.borrow();
        changes
            .iter()
            .filter(|(id, _)| entries.get(id).is_some_and(|entry| entry.borrow().tool.as_deref() == Some(self.table.add_tool)))
            .count()
    }

    /// Kumi's track renames since `before`, still applied, oldest first: the track's ref, the name it had and its new one.
    pub fn renames_since(&self, before: &[String]) -> Vec<(String, String, String)> {
        let entries = self.plan.history.entries.borrow();
        self.plan
            .renamed
            .borrow()
            .iter()
            .filter(|(id, ..)| !before.contains(id))
            .filter(|(id, ..)| {
                entries.get(id).is_some_and(|entry| matches!(entry.borrow().record.state, ChangeState::Applied | ChangeState::Kept))
            })
            .map(|(_, reference, from, to)| (self.place_key(reference), from.clone(), to.clone()))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rows() -> Vec<Value> {
        vec![
            json!({"line": 0, "notes": [{"col": 1, "note": "C-4", "instrument": "00"}, {"col": 2, "note": "E-4", "instrument": "00", "volume": "40"}]}),
            json!({"line": 2, "notes": [{"col": 1, "note": "OFF"}]}),
            json!({"line": 3, "notes": [{"col": 1, "note": "OFF"}]}),
            json!({"line": 4, "notes": [{"col": 1, "note": "D#4", "instrument": "00", "delay": "80", "fx": "0D10"}]}),
            json!({"line": 5, "notes": [{"col": 2, "note": "---", "volume": "G3"}]}),
        ]
    }

    #[test]
    fn a_pattern_tracks_notes_last_to_the_next_note_or_off_in_their_column_and_play_at_their_volume() {
        let (notes, taken) = cells_of(&rows(), 8, 4.);
        let shown: Vec<(i64, f64, f64, i32, f64)> = notes.iter().map(|(id, _, n)| (*id, n.start, n.length, n.pitch, n.velocity)).collect();
        // C-4 to the OFF on line 02 (two lines: half a beat); E-4 to the pattern's end; D#4 half a line late, to the end.
        assert_eq!(shown, [(1, 0., 0.5, 48, 127.), (2, 0., 2., 52, 64.), (65, 1.125, 0.875, 51, 127.)]);
        assert!(taken.contains(&(2, 1)) && taken.contains(&(5, 2)), "an OFF and a cell with only a volume take their cells");
        assert_eq!(note_number("C#9"), Some(109));
        assert_eq!((note_number("OFF"), note_number("---"), note_number("C4")), (None, None, None));
        assert_eq!((velocity_of(Some("80")), velocity_of(Some("G3")), volume_of(64.)), (Some(127.), None, "41".into()));
    }

    #[test]
    fn a_move_changes_a_notes_delay_or_takes_it_a_line_over_into_a_free_cell_and_never_onto_another() {
        let (notes, taken) = cells_of(&rows(), 8, 4.);
        let cells: HashMap<i64, Cell> = notes.into_iter().map(|(id, cell, _)| (id, cell)).collect();
        let patch = |id: i64, start: f64, velocity: f64| NotePatch { id, start, velocity, duration: None };
        // D#4 (line 04, delay 80) a little earlier: its own line, delay 40.
        let lines = writes_for(&cells, &taken, &[patch(65, 1.0625, 127.)], 4., 8);
        assert_eq!(lines, [json!({"line": 4, "notes": [{"col": 1, "note": "D#4", "instrument": "00", "fx": "0D10", "delay": "40"}]})]);
        // Earlier than its line: line 03 holds an OFF in its column, so it stays on its own line, as early as that goes.
        let lines = writes_for(&cells, &taken, &[patch(65, 0.98, 127.)], 4., 8);
        assert_eq!(lines, [json!({"line": 4, "notes": [{"col": 1, "note": "D#4", "instrument": "00", "fx": "0D10"}]})]);
        // E-4 0.6 of a line past line 01's start: line 01 is free in its column, so it goes there (delay 9A) and its old
        // cell is cleared.
        let lines = writes_for(&cells, &taken, &[patch(2, 0.4, 64.)], 4., 8);
        assert_eq!(
            lines,
            [
                json!({"line": 0, "notes": [{"col": 2, "clear": true}]}),
                json!({"line": 1, "notes": [{"col": 2, "note": "E-4", "instrument": "00", "volume": "40", "delay": "9A"}]}),
            ]
        );
        // Two lines later is further than a move goes: as late as its own line goes.
        let lines = writes_for(&cells, &taken, &[patch(2, 0.6, 64.)], 4., 8);
        assert_eq!(lines, [json!({"line": 0, "notes": [{"col": 2, "note": "E-4", "instrument": "00", "volume": "40", "delay": "FF"}]})]);
        // Not before the pattern's start; only its volume changes.
        let lines = writes_for(&cells, &taken, &[patch(2, -0.1, 100.)], 4., 8);
        assert_eq!(lines, [json!({"line": 0, "notes": [{"col": 2, "note": "E-4", "instrument": "00", "volume": "65"}]})]);
        // Nothing to change: nothing written.
        assert!(writes_for(&cells, &taken, &[patch(1, 0., 127.)], 4., 8).is_empty());
    }
}
