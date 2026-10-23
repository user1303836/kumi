//! Renoise's ears (#401): audio to MIDI writes the notes in a new track's pattern lines (a note off the grid on its line with
//! a delay, a pitched note ended by an OFF), and stems load each as an instrument, played from the stretch's start on a new
//! track.
use crate::RenoiseRefs;
use async_trait::async_trait;
use kumi_agent::core::stems::backend::Words as StemWords;
use kumi_agent::core::to_midi::backend::Words;
use kumi_agent::core::to_midi::{MidiNote, HAT, KICK, SNARE};
use kumi_common::abort::Signal;
use kumi_daw_own::ears::{EarsDaw, OwnEars};
use kumi_types::contracts::JsonObject;
use kumi_types::daw::DawProfile;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::path::Path;

pub static RENOISE_WORDS: Words = Words {
    app: "Renoise",
    written: "a new track after it, in the patterns its slots play (a note between lines on its line with a delay, a pitched note ended by an OFF)",
    options: "instrumentRef, the instrument the notes play (a drum kit for drums): needed in Renoise",
    options_schema: r#"{"type":"object","additionalProperties":false,"properties":{"instrumentRef":{"type":"string","description":"The instrument the notes play"}}}"#,
};

pub static RENOISE_STEM_WORDS: StemWords =
    StemWords { app: "Renoise", placed: "a new instrument each, played from the stretch's start on a new track after it (a column each)" };

/// The most note columns a Renoise track has, and the most pages of rows read.
const COLUMNS: u32 = 12;
const PAGES: usize = 10;

/// Renoise's ears.
pub struct RenoiseEars;

/// Renoise's slots over the stretch and its lines a beat.
async fn slots(ears: &OwnEars<RenoiseRefs>, from: f64, beats: f64, signal: &Signal) -> Result<(Vec<Slot>, f64), String> {
    let song = ears.plan().read_through(json!({"kind": "song"}), None, signal).await?;
    let lpb = song.pointer("/items/0/lpb").and_then(Value::as_u64).unwrap_or(4).max(1) as f64;
    let rows = ears.plan().rows_through(json!({"kind": "sequence", "limit": 200}), PAGES, signal).await?;
    let slots = rows
        .iter()
        .filter_map(|row| {
            Some(Slot {
                number: row.get("ref")?.as_str()?.rsplit(':').next()?.parse().ok()?,
                pattern: row.get("patternRef")?.as_str()?.to_string(),
                at: row.get("positionBeats")?.as_f64()?,
                beats: row.get("lengthBeats")?.as_f64()?,
            })
        })
        .filter(|slot| slot.at < from + beats && slot.at + slot.beats > from)
        .collect();
    Ok((slots, lpb))
}

#[async_trait(?Send)]
impl EarsDaw<RenoiseRefs> for RenoiseEars {
    fn words(&self) -> &'static Words {
        &RENOISE_WORDS
    }

    fn stem_words(&self) -> &'static StemWords {
        &RENOISE_STEM_WORDS
    }

    fn note_name(&self, pitch: u8) -> String {
        DawProfile::RENOISE.note_name(pitch.min(119))
    }

    async fn write(
        &self,
        ears: &OwnEars<RenoiseRefs>,
        name: &str,
        index: u64,
        notes: &[MidiNote],
        drums: bool,
        from: f64,
        beats: f64,
        options: &JsonObject,
        signal: &Signal,
    ) -> Result<Value, String> {
        let instrument = options
            .get("instrumentRef")
            .and_then(Value::as_str)
            .ok_or("options.instrumentRef: the instrument the notes play in Renoise (a drum kit for drums)")?;
        let instrument = ears.plan().lengthen(instrument)?;
        // Renoise's pattern lines for the notes: each pattern of the stretch's slots (patternRef → lines), and how many notes
        // didn't fit (more than 12 at once on a line).
        let (slots, lpb) = slots(ears, from, beats, signal).await?;
        let (patterns, left_out) = renoise_cells(notes, drums, from, beats, lpb, &slots, &instrument)?;
        let mut steps = vec![json!({"tool": "track_add", "as": "midi", "input": {"type": "sequencer", "name": name, "index": index}})];
        for (pattern, lines) in patterns {
            steps.push(json!({"tool": "notes_write", "input": {"patternRef": pattern, "trackRef": "@midi", "lines": lines}}));
        }
        let mut made = ears.run(steps, signal).await?;
        if left_out > 0 {
            made["leftOut"] = json!(format!("{left_out} notes didn't fit: past the stretch's slots, or more than 12 at once on a line"));
        }
        Ok(made)
    }

    async fn place(
        &self,
        ears: &OwnEars<RenoiseRefs>,
        row: &Value,
        index: u64,
        stems: &[(String, &Path)],
        from: f64,
        beats: f64,
        signal: &Signal,
    ) -> Result<Value, String> {
        let (slots, lpb) = slots(ears, from, beats, signal).await?;
        let steps = renoise_stem_steps(stems, row.get("name").and_then(Value::as_str).unwrap_or("Track"), index, from, lpb, &slots)?;
        ears.run(steps, signal).await
    }
}

/// Each pattern's lines to write (patternRef, notes_write lines), and how many notes didn't fit.
type Cells = (Vec<(String, Vec<Value>)>, usize);

/// A sequence slot over the stretch: its number, its pattern, where it starts and how long it plays (beats).
#[derive(Debug, Clone, PartialEq)]
pub struct Slot {
    pub number: u64,
    pub pattern: String,
    pub at: f64,
    pub beats: f64,
}

/// Where a beat of the song falls in Renoise: the slot playing it, the line and the delay (256ths of a line), or None past the
/// slots given.
fn place_in(beat: f64, slots: &[Slot], lpb: f64) -> Option<(&Slot, u32, u32)> {
    let slot = slots.iter().find(|slot| slot.at <= beat + 1e-9 && beat < slot.at + slot.beats - 1e-9)?;
    let at = (beat - slot.at) * lpb;
    let mut line = at.floor() as u32;
    let mut delay = ((at - line as f64) * 256.).round() as u32;
    if delay >= 256 {
        line += 1;
        delay = 0;
    }
    ((line as f64) < slot.beats * lpb).then_some((slot, line, delay))
}

/// Renoise's plan for stems: each loaded as a new instrument, and a new track after the source playing them all from the
/// stretch's start (a column each, C-4: the sample as recorded).
pub fn renoise_stem_steps(
    stems: &[(String, &Path)],
    name: &str,
    index: u64,
    from: f64,
    lpb: f64,
    slots: &[Slot],
) -> Result<Vec<Value>, String> {
    let (slot, line, delay) = place_in(from, slots, lpb).ok_or("the stretch's start isn't in a slot of the song")?;
    // (each named after its stem, not its file)
    let mut steps: Vec<Value> = stems
        .iter()
        .enumerate()
        .flat_map(|(k, (stem, path))| {
            [
                json!({"tool": "instrument_load", "as": format!("i{k}"), "input": {"path": path.to_string_lossy()}}),
                json!({"tool": "instrument_rename", "input": {"ref": format!("@i{k}"), "name": stem}}),
            ]
        })
        .collect();
    steps
        .push(json!({"tool": "track_add", "as": "stems", "input": {"type": "sequencer", "name": format!("{name} stems"), "index": index}}));
    let notes: Vec<Value> = (0..stems.len())
        .map(|k| {
            let mut cell = json!({"col": k + 1, "note": "C-4", "instrumentRef": format!("@i{k}")});
            if delay > 0 {
                cell["delay"] = json!(delay);
            }
            cell
        })
        .collect();
    steps.push(json!({"tool": "notes_write", "input": {"patternRef": slot.pattern, "trackRef": "@stems", "lines": [{"line": line, "notes": notes}]}}));
    Ok(steps)
}

/// The notes as Renoise's cells, by pattern: each note on its line (with a delay between lines). Drums take a column a key (kick 1,
/// snare 2, hat 3, others after) and play out; a pitched note takes the first column free of a note still sounding there,
/// across the stretch, and ends with an OFF where it ends (in whichever pattern that is; on the next line for one shorter than a
/// line; on the stretch's last line, delayed to its end, for one sounding to it). Refused when two slots of the stretch play one
/// pattern: a note written there would play in both.
pub fn renoise_cells(
    notes: &[MidiNote],
    drums: bool,
    from: f64,
    beats: f64,
    lpb: f64,
    slots: &[Slot],
    instrument: &str,
) -> Result<Cells, String> {
    for (k, slot) in slots.iter().enumerate() {
        if let Some(other) = slots[k + 1..].iter().find(|other| other.pattern == slot.pattern) {
            return Err(format!(
                "slots {:02} and {:02} play the same pattern: a note written there would play in both. Make them unique first (slot_unique), or hear a stretch of one",
                slot.number, other.number
            ));
        }
    }
    let end_of_stretch = from + beats;
    // (each pattern's cells: line → column → cell)
    let mut by_pattern: BTreeMap<String, BTreeMap<u32, BTreeMap<u32, Value>>> = BTreeMap::new();
    // (where the note in each column ends, beats from the stretch's start: one track across the stretch's patterns)
    let mut busy = [f64::NEG_INFINITY; COLUMNS as usize];
    // (drums: each key's column)
    let mut key_columns: BTreeMap<u8, u32> = [(KICK, 1), (SNARE, 2), (HAT, 3)].into_iter().collect();
    let mut left_out = 0;
    let place = |beat: f64| place_in(beat, slots, lpb);
    for note in notes {
        let Some((slot, line, delay)) = place(from + note.start) else {
            left_out += 1;
            continue;
        };
        let taken = by_pattern.entry(slot.pattern.clone()).or_default().entry(line).or_default();
        let col = if drums {
            let next = key_columns.values().max().copied().unwrap_or(0) + 1;
            let col = *key_columns.entry(note.pitch).or_insert(next);
            (col <= COLUMNS && !taken.contains_key(&col)).then_some(col)
        } else {
            (1..=COLUMNS).find(|col| !taken.contains_key(col) && busy[*col as usize - 1] <= note.start + 1e-9)
        };
        let Some(col) = col else {
            left_out += 1;
            continue;
        };
        // (the volume column's hex, 80 the loudest)
        let volume = format!("{:02X}", (note.velocity as f64 * 128. / 127.).round().min(128.) as u64);
        let mut cell =
            json!({"col": col, "note": DawProfile::RENOISE.note_name(note.pitch), "instrumentRef": instrument, "volume": volume});
        if delay > 0 {
            cell["delay"] = json!(delay);
        }
        taken.insert(col, cell);
        if drums {
            continue;
        }
        busy[col as usize - 1] = note.start + note.length;
        // (where it ends: on its own line's next for one shorter than a line, on the stretch's last line, delayed to its end, for
        // one sounding to it, else where it does)
        let ends = from + note.start + note.length;
        let off = match place(ends) {
            Some((end_slot, end_line, end_delay)) if ends < end_of_stretch - 1e-9 => {
                if end_slot.pattern == slot.pattern && end_line == line {
                    place(slot.at + (line + 1) as f64 / lpb).map(|(next_slot, next_line, _)| (next_slot, next_line, 0))
                } else {
                    Some((end_slot, end_line, end_delay))
                }
            }
            _ => place(end_of_stretch - 1. / lpb / 256.).map(|(last_slot, last_line, _)| (last_slot, last_line, 255)),
        };
        if let Some((off_slot, off_line, off_delay)) = off {
            // (never on the note's own cell: one starting on the stretch's last line plays out)
            let own = off_slot.pattern == slot.pattern && off_line == line;
            if !own {
                if let std::collections::btree_map::Entry::Vacant(free) =
                    by_pattern.entry(off_slot.pattern.clone()).or_default().entry(off_line).or_default().entry(col)
                {
                    let mut cell = json!({"col": col, "note": "OFF"});
                    if off_delay > 0 {
                        cell["delay"] = json!(off_delay);
                    }
                    free.insert(cell);
                }
            }
        }
    }
    let out = by_pattern
        .into_iter()
        .map(|(pattern, lines)| {
            let lines =
                lines.into_iter().map(|(line, cells)| json!({"line": line, "notes": cells.into_values().collect::<Vec<_>>()})).collect();
            (pattern, lines)
        })
        .collect();
    Ok((out, left_out))
}
