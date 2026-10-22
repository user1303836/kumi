//! Audio to MIDI through Kumi's own bridges (#401): REAPER's and Renoise's facts for core's tool. Both render the track
//! offline (REAPER its items before its FX, Renoise the track through its devices, as Renoise renders a track soloed) and
//! take the tempo Kumi follows; REAPER writes the notes in one MIDI item on a new track under the source, Renoise in a new
//! track's pattern lines, a note off the grid on its line with a delay and a pitched note ended by an OFF.
use super::own_changes::OwnPlan;
use super::own_listening::OwnRenderer;
use super::renoise_lines::note_text;
use crate::core::contracts::{JsonObject, Render};
use crate::core::plan::make_changes;
use crate::core::refs::RefScheme;
use crate::core::to_midi::backend::{ToMidiBackend, Words};
use crate::core::to_midi::MidiNote;
use crate::listening::renderer::{Renderer, Window};
use async_trait::async_trait;
use kumi_common::abort::Signal;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::rc::Rc;

pub static REAPER_WORDS: Words = Words {
    app: "REAPER",
    written: "a new MIDI track under it, one item over the stretch (no instrument on it: add one to hear them)",
    options: "none in REAPER",
    options_schema: r#"{"type":"object","additionalProperties":false,"properties":{}}"#,
};

pub static RENOISE_WORDS: Words = Words {
    app: "Renoise",
    written: "a new track after it, in the patterns its slots play (a note between lines on its line with a delay, a pitched note ended by an OFF)",
    options: "instrumentRef, the instrument the notes play (a drum kit for drums): needed in Renoise",
    options_schema: r#"{"type":"object","additionalProperties":false,"properties":{"instrumentRef":{"type":"string","description":"The instrument the notes play"}}}"#,
};

/// The most note columns a Renoise track has, and the most pages of rows read.
const COLUMNS: u32 = 12;
const PAGES: usize = 10;

/// Which bridge: REAPER writes an item, Renoise pattern lines.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Daw {
    Reaper,
    Renoise,
}

pub struct OwnToMidi<S: RefScheme + 'static> {
    plan: Rc<OwnPlan<S>>,
    renderer: Rc<OwnRenderer<S>>,
    daw: Daw,
}

impl<S: RefScheme + 'static> OwnToMidi<S> {
    pub fn new(plan: Rc<OwnPlan<S>>, renderer: Rc<OwnRenderer<S>>, daw: Daw) -> Self {
        OwnToMidi { plan, renderer, daw }
    }

    async fn row(&self, long: &str, signal: &Signal) -> Result<Value, String> {
        let answer = self.plan.read_through(json!({"ref": long}), None, signal).await?;
        answer.pointer("/items/0").cloned().ok_or_else(|| "that track isn't in the project any more: read again".to_string())
    }

    async fn run(&self, steps: Vec<Value>, signal: &Signal) -> Result<Value, String> {
        let done = make_changes(&self.plan, json!({"steps": steps}).as_object().cloned().unwrap_or_default(), signal.clone())
            .await
            .map_err(|error| error.to_string())?;
        if done.is_error {
            return Err(done.text);
        }
        Ok(serde_json::from_str(&done.text).unwrap_or_else(|_| json!({"said": done.text})))
    }

    /// Renoise's pattern lines for the notes: each pattern of the stretch's slots (patternRef → lines), and how many notes
    /// didn't fit (more than 12 at once on a line).
    async fn renoise_lines(
        &self,
        notes: &[MidiNote],
        drums: bool,
        from: f64,
        beats: f64,
        instrument: &str,
        signal: &Signal,
    ) -> Result<Cells, String> {
        let song = self.plan.read_through(json!({"kind": "song"}), None, signal).await?;
        let lpb = song.pointer("/items/0/lpb").and_then(Value::as_u64).unwrap_or(4).max(1) as f64;
        let slots = self.plan.rows_through(json!({"kind": "sequence", "limit": 200}), PAGES, signal).await?;
        let slots: Vec<Slot> = slots
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
        renoise_cells(notes, drums, from, lpb, &slots, instrument)
    }
}

/// Each pattern's lines to write (patternRef, notes_write lines), and how many notes didn't fit.
type Cells = (Vec<(String, Vec<Value>)>, usize);

/// A sequence slot over the stretch: its number, its pattern, where it starts and how long it plays (beats).
#[derive(Debug, Clone, PartialEq)]
struct Slot {
    number: u64,
    pattern: String,
    at: f64,
    beats: f64,
}

/// The notes as Renoise's cells, by pattern: each note on its line (with a delay between lines) in the first column free there,
/// a pitched one ended by an OFF where it ends within its pattern (drums play out). Refused when two slots of the stretch play
/// one pattern: a note written there would play in both.
fn renoise_cells(notes: &[MidiNote], drums: bool, from: f64, lpb: f64, slots: &[Slot], instrument: &str) -> Result<Cells, String> {
    for (k, slot) in slots.iter().enumerate() {
        if let Some(other) = slots[k + 1..].iter().find(|other| other.pattern == slot.pattern) {
            return Err(format!(
                "slots {:02} and {:02} play the same pattern: a note written there would play in both. Make them unique first (slot_unique), or hear a stretch of one",
                slot.number, other.number
            ));
        }
    }
    // (each pattern's cells: line → column → cell)
    let mut by_pattern: BTreeMap<String, BTreeMap<u32, BTreeMap<u32, Value>>> = BTreeMap::new();
    // (each pattern's columns: where the note in each ends, beats from the stretch's start)
    let mut busy: BTreeMap<String, [f64; COLUMNS as usize]> = BTreeMap::new();
    let mut left_out = 0;
    let place = |beat: f64| -> Option<(&Slot, u32, u32)> {
        let slot = slots.iter().find(|slot| slot.at <= beat + 1e-9 && beat < slot.at + slot.beats - 1e-9)?;
        let at = (beat - slot.at) * lpb;
        let mut line = at.floor() as u32;
        let mut delay = ((at - line as f64) * 256.).round() as u32;
        if delay >= 256 {
            line += 1;
            delay = 0;
        }
        ((line as f64) < slot.beats * lpb).then_some((slot, line, delay))
    };
    for note in notes {
        let Some((slot, line, delay)) = place(from + note.start) else {
            left_out += 1;
            continue;
        };
        let lines = by_pattern.entry(slot.pattern.clone()).or_default();
        // (a column free of cells on the line and of a note still sounding there: a new note in it would cut that one)
        let ends = busy.entry(slot.pattern.clone()).or_insert([f64::NEG_INFINITY; COLUMNS as usize]);
        let taken = lines.entry(line).or_default();
        let Some(col) = (1..=COLUMNS).find(|col| !taken.contains_key(col) && ends[*col as usize - 1] <= note.start + 1e-9) else {
            left_out += 1;
            continue;
        };
        ends[col as usize - 1] = if drums { note.start } else { note.start + note.length };
        // (the volume column's hex, 80 the loudest)
        let volume = format!("{:02X}", (note.velocity as f64 * 128. / 127.).round().min(128.) as u64);
        let mut cell = json!({"col": col, "note": note_text(note.pitch), "instrumentRef": instrument, "volume": volume});
        if delay > 0 {
            cell["delay"] = json!(delay);
        }
        taken.insert(col, cell);
        // (a pitched note ends where it does, when that's in its pattern and its column is free there)
        if !drums {
            if let Some((end_slot, end_line, end_delay)) = place(from + note.start + note.length) {
                if end_slot.pattern == slot.pattern && end_line > line {
                    if let std::collections::btree_map::Entry::Vacant(free) = lines.entry(end_line).or_default().entry(col) {
                        let mut off = json!({"col": col, "note": "OFF"});
                        if end_delay > 0 {
                            off["delay"] = json!(end_delay);
                        }
                        free.insert(off);
                    }
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

#[async_trait(?Send)]
impl<S: RefScheme + 'static> ToMidiBackend for OwnToMidi<S> {
    fn words(&self) -> &'static Words {
        match self.daw {
            Daw::Reaper => &REAPER_WORDS,
            Daw::Renoise => &RENOISE_WORDS,
        }
    }

    async fn render(&self, track: &str, from: f64, beats: f64, signal: &Signal) -> Result<(Render, String), String> {
        if self.renderer.listening_now() {
            return Err("Kumi is listening to something else just now".into());
        }
        if let Some(why) = self.renderer.why_not() {
            return Err(why);
        }
        let long = self.plan.lengthen(track)?;
        let row = self.row(&long, signal).await?;
        let name = row.get("name").and_then(Value::as_str).unwrap_or("Track").to_string();
        // (REAPER: the track's own items before its FX; Renoise: the track through its devices)
        let stage = match self.daw {
            Daw::Reaper => "items",
            Daw::Renoise => "pre-fader",
        };
        let sources = vec![json!({"id": "t", "what": "track", "ref": long, "stage": stage})];
        let _listening = self.renderer.begin_listen();
        let rendered = self.renderer.render_sources(sources, Window { from, beats }, signal).await;
        self.renderer.put_tempo_back().await;
        match rendered {
            Ok(Ok((files, _))) => {
                files.get("t").map(|(render, _)| (render.clone(), name)).ok_or_else(|| "the render made no file".to_string())
            }
            Ok(Err(why)) => Err(why),
            Err(error) => Err(error.to_string()),
        }
    }

    async fn tempo(&self, _from: f64, _signal: &Signal) -> Result<f64, String> {
        self.renderer
            .tempo()
            .filter(|bpm| *bpm > 0.)
            .ok_or_else(|| "Kumi doesn't know the tempo yet: read the project, then ask again".to_string())
    }

    fn note_name(&self, pitch: u8) -> String {
        note_text(pitch.min(119))
    }

    async fn write(
        &self,
        track: &str,
        name: &str,
        notes: &[MidiNote],
        drums: bool,
        from: f64,
        beats: f64,
        options: &JsonObject,
        signal: &Signal,
    ) -> Result<Value, String> {
        let long = self.plan.lengthen(track)?;
        let index = self.row(&long, signal).await?.get("index").and_then(Value::as_u64).unwrap_or(0) + 1;
        match self.daw {
            Daw::Reaper => self.run(reaper_steps(name, index, notes, from, beats), signal).await,
            Daw::Renoise => {
                let instrument = options
                    .get("instrumentRef")
                    .and_then(Value::as_str)
                    .ok_or("options.instrumentRef: the instrument the notes play in Renoise (a drum kit for drums)")?;
                let instrument = self.plan.lengthen(instrument)?;
                let (patterns, left_out) = self.renoise_lines(notes, drums, from, beats, &instrument, signal).await?;
                let mut steps =
                    vec![json!({"tool": "track_add", "as": "midi", "input": {"type": "sequencer", "name": name, "index": index}})];
                for (pattern, lines) in patterns {
                    steps.push(json!({"tool": "notes_write", "input": {"patternRef": pattern, "trackRef": "@midi", "lines": lines}}));
                }
                let mut made = self.run(steps, signal).await?;
                if left_out > 0 {
                    made["leftOut"] =
                        json!(format!("{left_out} notes didn't fit: past the stretch's slots, or more than 12 at once on a line"));
                }
                Ok(made)
            }
        }
    }
}

/// REAPER's plan: a new track under the source, one MIDI item over the stretch, and the notes in it.
fn reaper_steps(name: &str, index: u64, notes: &[MidiNote], from: f64, beats: f64) -> Vec<Value> {
    let notes: Vec<Value> = notes
        .iter()
        .map(|note| json!({"pitch": note.pitch, "start": round(note.start), "length": round(note.length.max(1. / 64.)), "velocity": note.velocity.max(1)}))
        .collect();
    vec![
        json!({"tool": "track_add", "as": "midi", "input": {"name": name, "index": index}}),
        json!({"tool": "item_add", "as": "item", "input": {"ref": "@midi", "positionBeats": from, "lengthBeats": beats, "name": name}}),
        json!({"tool": "notes_add", "input": {"ref": "@item", "notes": notes}}),
    ]
}

fn round(beats: f64) -> f64 {
    (beats * 10_000.).round() / 10_000.
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::integrations::renoise_changes::CHANGES as RENOISE;
    use crate::kernel::schema::problem;

    fn slot(number: u64, pattern: &str, at: f64, beats: f64) -> Slot {
        Slot { number, pattern: pattern.into(), at, beats }
    }

    fn note(pitch: u8, start: f64, length: f64) -> MidiNote {
        MidiNote { pitch, start, length, velocity: 127 }
    }

    #[test]
    fn notes_land_on_their_slots_lines_with_a_delay_and_pitched_ones_end_with_an_off() {
        // (two slots of 16 lines at LPB 4, from beat 4: the stretch starts at beat 6)
        let slots = vec![slot(2, "1:pattern:1", 4., 4.), slot(3, "1:pattern:2", 8., 4.)];
        let (patterns, left_out) =
            renoise_cells(&[note(60, 0., 1.), note(64, 0.1, 0.5), note(67, 2.5, 1.)], false, 6., 4., &slots, "1:instrument:1").unwrap();
        assert_eq!(left_out, 0);
        let first = &patterns[0];
        assert_eq!(first.0, "1:pattern:1");
        // (beat 6 is line 8 of the first slot; beat 6.1 is line 8 at 0.4 of a line: delay 102)
        assert_eq!(first.1[0]["line"], 8);
        assert_eq!(first.1[0]["notes"][0], json!({"col": 1, "note": "C-5", "instrumentRef": "1:instrument:1", "volume": "80"}));
        assert_eq!(first.1[0]["notes"][1]["col"], 2);
        assert_eq!(first.1[0]["notes"][1]["delay"], 102);
        assert!(first.1.iter().any(|line| line["line"] == 12 && line["notes"][0]["note"] == "OFF"), "C-5 ends a beat on: {first:?}");
        // (beat 8.5 is line 2 of the second slot)
        assert_eq!(patterns[1].1[0]["line"], 2);
        for (pattern, lines) in &patterns {
            let input = json!({"patternRef": pattern, "trackRef": "1:track:9", "lines": lines});
            let kind = RENOISE.kinds.iter().find(|kind| kind.tool == "notes_write").unwrap();
            assert_eq!(problem(&Value::Object(kind.input_schema()), &input), None, "{input}");
        }
    }

    #[test]
    fn reapers_plan_and_renoises_track_fit_their_kinds() {
        use crate::integrations::reaper_changes::CHANGES as REAPER;
        let fits = |kinds: &[crate::integrations::own_changes::OwnKind], step: &Value| {
            let tool = step["tool"].as_str().unwrap();
            let kind = kinds.iter().find(|kind| kind.tool == tool).unwrap_or_else(|| panic!("{tool}"));
            assert_eq!(problem(&Value::Object(kind.input_schema()), &step["input"]), None, "{step}");
        };
        for step in reaper_steps("Amen drums", 3, &[note(36, 0., 0.25), note(38, 1., 0.25)], 8., 4.) {
            fits(REAPER.kinds, &step);
        }
        fits(RENOISE.kinds, &json!({"tool": "track_add", "input": {"type": "sequencer", "name": "Amen drums", "index": 3}}));
    }

    #[test]
    fn a_note_starting_while_another_sounds_takes_another_column() {
        let slots = vec![slot(1, "1:pattern:1", 0., 4.)];
        // (C-5 for two beats; E-5 starts a beat in, under it: column 2, and C-5's OFF stays in column 1)
        let (patterns, _) = renoise_cells(&[note(60, 0., 2.), note(64, 1., 2.)], false, 0., 4., &slots, "1:instrument:1").unwrap();
        let lines = &patterns[0].1;
        let at = |line: u64| lines.iter().find(|row| row["line"] == line).cloned().unwrap_or(Value::Null);
        assert_eq!(at(4)["notes"][0]["col"], 2, "{lines:?}");
        assert_eq!(at(8)["notes"][0], json!({"col": 1, "note": "OFF"}), "{lines:?}");
        assert_eq!(at(12)["notes"][0], json!({"col": 2, "note": "OFF"}), "{lines:?}");
    }

    #[test]
    fn a_stretch_over_two_slots_of_one_pattern_is_refused_and_drums_play_out() {
        let slots = vec![slot(1, "1:pattern:1", 0., 4.), slot(2, "1:pattern:1", 4., 4.)];
        let refused = renoise_cells(&[note(36, 0., 0.25)], true, 0., 4., &slots, "1:instrument:1").unwrap_err();
        assert!(refused.starts_with("slots 01 and 02 play the same pattern"), "{refused}");
        let (patterns, _) = renoise_cells(&[note(36, 0., 0.25), note(38, 1., 0.25)], true, 0., 4., &slots[..1], "1:instrument:1").unwrap();
        assert!(!Value::Array(patterns[0].1.clone()).to_string().contains("OFF"), "drums play out: {:?}", patterns[0]);
    }
}
