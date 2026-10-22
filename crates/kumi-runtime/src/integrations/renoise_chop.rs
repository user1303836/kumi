//! Renoise's backend for chopping (#401): only Renoise's facts. A recording is an instrument's first sample (its audio saved to
//! Kumi's folder to be heard), its cuts are slice markers (frames from 1) made by sample_slices in one change, each slice plays
//! on its own key (the read gives it), and the slices are laid out in a pattern track by notes_write, one change, on lines and
//! delays (256ths of a line) at the song's lines per beat: slices that start on one line take a note column each, the track
//! shows the columns and delays it needs (a change of its own when it doesn't yet), and notes already there are never written
//! over unless asked. The render is of that track where a slot plays the pattern.
use super::own_changes::OwnPlan;
use super::own_listening::OwnRenderer;
use super::renoise_lines::note_text;
use crate::core::chop::backend::{ChopBackend, Key, Laid, Recording, Song, Step, Words};
use crate::core::refs::RefScheme;
use crate::listening::renderer::{Renderer, Window};
use async_trait::async_trait;
use kumi_common::abort::Signal;
use serde_json::{json, Value};
use std::rc::Rc;

pub static WORDS: Words = Words {
    app: "Renoise",
    source: "a sample: an instrument's first sample, the one its slices cut",
    place: "at: patternRef (or a sequence slot for its pattern), trackRef, and line (as Renoise numbers lines, from 00; 00 by default): on lines and delays at the song's lines per beat, a note column for each slice that starts on a line another does",
    cut: "slice",
    at_schema: r#"{"type":"object","additionalProperties":false,"required":["patternRef","trackRef"],"properties":{"patternRef":{"type":"string","description":"The pattern, or a sequence slot for its pattern"},"trackRef":{"type":"string"},"line":{"type":"integer","minimum":0}}}"#,
};

/// Pages of sample rows read for an instrument's slices, and of a pattern track's lines (200 a page).
const SAMPLE_PAGES: usize = 2;
const LINE_PAGES: usize = 8;
/// A line's delay counts 256ths of it.
const DELAYS: u64 = 256;

pub struct RenoiseChop<S: RefScheme + 'static> {
    plan: Rc<OwnPlan<S>>,
    renderer: Rc<OwnRenderer<S>>,
}

impl<S: RefScheme + 'static> RenoiseChop<S> {
    pub fn new(plan: Rc<OwnPlan<S>>, renderer: Rc<OwnRenderer<S>>) -> Self {
        RenoiseChop { plan, renderer }
    }

    async fn change(&self, tool: &str, input: Value, signal: &Signal) -> Result<Value, String> {
        let outcome = self.plan.change_by(tool, input.as_object().cloned().unwrap_or_default(), signal.clone()).await;
        let answer: Value = serde_json::from_str(&outcome.text).unwrap_or_else(|_| json!({"said": outcome.text}));
        if outcome.is_error || outcome.stops {
            return Err(answer.get("changed").and_then(Value::as_str).map(str::to_owned).unwrap_or(outcome.text));
        }
        Ok(answer)
    }

    async fn row(&self, long: &str, signal: &Signal) -> Result<Value, String> {
        let answer = self.plan.read_through(json!({"ref": long}), None, signal).await?;
        answer.pointer("/items/0").cloned().ok_or_else(|| "that isn't in the song any more: read again".to_string())
    }

    async fn lpb(&self, signal: &Signal) -> Result<u64, String> {
        let answer = self.plan.read_through(json!({"kind": "song"}), None, signal).await?;
        Ok(answer.pointer("/items/0/lpb").and_then(Value::as_u64).unwrap_or(4).max(1))
    }

    /// The pattern a pattern or slot ref names: its long ref and how many lines it has.
    async fn pattern_of(&self, long: &str, signal: &Signal) -> Result<(String, u64), String> {
        let pattern = if long.contains(":seq:") {
            let slot = self.row(long, signal).await?;
            slot.get("patternRef").and_then(Value::as_str).map(str::to_owned).ok_or("that slot plays no pattern")?
        } else {
            long.to_string()
        };
        let row = self.row(&pattern, signal).await?;
        Ok((pattern, row.get("lines").and_then(Value::as_u64).unwrap_or(64)))
    }
}

/// The instrument a sample's long ref ("<epoch>:sample:<i>.<s>") is of: its long ref and its number from 0.
fn instrument_of(sample: &str) -> Option<(String, u64)> {
    let (epoch, rest) = sample.split_once(":sample:")?;
    let (i, _) = rest.split_once('.')?;
    let i: u64 = i.parse().ok()?;
    Some((format!("{epoch}:instrument:{i}"), i.checked_sub(1)?))
}

/// One slice's note as written: its line, note column and delay.
#[derive(Debug, Clone, PartialEq)]
struct Cell {
    line: u64,
    col: u64,
    note: String,
    delay: u64,
}

/// Each step on Renoise's grid from line `first` at `lpb` lines a beat: its line and delay, and a note column of its own when
/// another starts on the same line (in order).
fn cells(steps: &[Step], first: u64, lpb: u64) -> Vec<Cell> {
    let mut out: Vec<Cell> = Vec::new();
    for step in steps {
        let units = (step.beat * (lpb * DELAYS) as f64).round().max(0.) as u64;
        let line = first + units / DELAYS;
        let col = out.iter().filter(|cell| cell.line == line).count() as u64 + 1;
        out.push(Cell { line, col, note: step.note.clone(), delay: units % DELAYS });
    }
    out
}

#[async_trait(?Send)]
impl<S: RefScheme + 'static> ChopBackend for RenoiseChop<S> {
    fn words(&self) -> &'static Words {
        &WORDS
    }

    async fn recording(&self, reference: &str, signal: &Signal) -> Result<Recording, String> {
        let long = self.plan.lengthen(reference)?;
        if instrument_of(&long).is_none() {
            return Err("ref is a sample: an instrument's first sample".into());
        }
        let row = self.row(&long, signal).await?;
        if row.get("slice") == Some(&Value::Bool(true)) {
            return Err("that's a slice: give the sliced sample, its instrument's first".into());
        }
        if row.get("index").and_then(Value::as_u64) != Some(0) {
            return Err("only an instrument's first sample can be sliced: give that one".into());
        }
        let frames = row.get("frames").and_then(Value::as_u64).unwrap_or(0) as usize;
        if frames == 0 {
            return Err("that sample has no audio".into());
        }
        let rate = row.get("rate").and_then(Value::as_f64).unwrap_or(44_100.);
        let file = match self.renderer.clip_file(&long, signal.clone()).await {
            Ok(Some(file)) => file,
            Ok(None) => return Err("Renoise gave no audio for that sample".into()),
            Err(error) => return Err(error.to_string()),
        };
        let cuts = row
            .get("sliceMarkers")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_u64)
            .map(|frame| frame.saturating_sub(1) as usize)
            .collect();
        Ok(Recording { reference: long, name: row.get("name").and_then(Value::as_str).unwrap_or("").to_string(), file, rate, frames, cuts })
    }

    async fn song(&self, signal: &Signal) -> Result<Song, String> {
        let answer = self.plan.read_through(json!({"kind": "song"}), None, signal).await?;
        let row = answer.pointer("/items/0").cloned().unwrap_or(Value::Null);
        let lpb = row.get("lpb").and_then(Value::as_u64).unwrap_or(4).max(1);
        Ok(Song {
            bpm: row.get("bpm").and_then(Value::as_f64).unwrap_or(120.),
            beats_per_bar: row.pointer("/metronome/beatsPerBar").and_then(Value::as_f64).unwrap_or(4.).max(1.),
            // (a note starts on a line's 256ths)
            grid: Some(1. / (lpb * DELAYS) as f64),
            playing: row.get("playing") == Some(&Value::Bool(true)),
        })
    }

    async fn cut(&self, recording: &Recording, frames: &[usize], replace: bool, signal: &Signal) -> Result<Value, String> {
        self.change("sample_slices", cut_input(recording, frames, replace), signal).await
    }

    async fn keys(&self, recording: &Recording, signal: &Signal) -> Result<Vec<Key>, String> {
        let (instrument, _) = instrument_of(&recording.reference).ok_or("that isn't a sample")?;
        let rows = self.plan.rows_through(json!({"kind": "sample", "parent": instrument, "limit": 200}), SAMPLE_PAGES, signal).await?;
        let markers: Vec<usize> = rows
            .first()
            .and_then(|row| row.get("sliceMarkers"))
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_u64)
            .map(|frame| frame.saturating_sub(1) as usize)
            .collect();
        let slices: Vec<&Value> = rows.iter().filter(|row| row.get("slice") == Some(&Value::Bool(true))).collect();
        Ok(markers
            .iter()
            .zip(slices)
            .filter_map(|(frame, row)| {
                let note = row.pointer("/noteRange/0").and_then(Value::as_u64)?;
                Some(Key { frame: *frame, note: note_text(note.min(119) as u8) })
            })
            .collect())
    }

    async fn lay(
        &self,
        recording: &Recording,
        at: &Value,
        steps: &[Step],
        beats: f64,
        replace: bool,
        signal: &Signal,
    ) -> Result<Laid, String> {
        let (_, instrument) = instrument_of(&recording.reference).ok_or("that isn't a sample")?;
        let pattern = at.get("patternRef").and_then(Value::as_str).ok_or("at.patternRef: the pattern (or a slot for its pattern)")?;
        let track = at.get("trackRef").and_then(Value::as_str).ok_or("at.trackRef: the track")?;
        let first = at.get("line").and_then(Value::as_u64).unwrap_or(0);
        let lpb = self.lpb(signal).await?;
        let cells = cells(steps, first, lpb);
        // (the range is the whole recording's length on the grid: a hair under a whole line is that line)
        let length = (beats * lpb as f64 - 1e-3).ceil().max(1.) as u64;
        let last = (first + length - 1).max(cells.iter().map(|cell| cell.line).max().unwrap_or(first));
        let (pattern_long, lines) = self.pattern_of(&self.plan.lengthen(pattern)?, signal).await?;
        if let Some(why) = too_long(length, lpb, first, last, lines) {
            return Err(why);
        }
        let columns = cells.iter().map(|cell| cell.col).max().unwrap_or(1);
        // Notes already in those columns of the range are the producer's: never written over unless asked.
        let track_long = self.plan.lengthen(track)?;
        let rows = self
            .plan
            .rows_through(
                json!({"kind": "line", "parent": pattern_long, "track": track_long, "hidden": true, "limit": 200}),
                LINE_PAGES,
                signal,
            )
            .await?;
        let held = held_lines(&rows, first, last, columns);
        if !held.is_empty() && !replace {
            let shown: Vec<String> = held.iter().take(8).map(|line| format!("{line:02}")).collect();
            return Err(format!(
                "line{} {}{} hold{} notes in the note columns the slices need: lay them elsewhere, or replace: true writes over them (undo_change puts them back)",
                if held.len() == 1 { "" } else { "s" },
                shown.join(", "),
                if held.len() > 8 { "…" } else { "" },
                if held.len() == 1 { "s" } else { "" }
            ));
        }
        // The track shows the columns and the delays the slices need (a change of its own, said, when it doesn't yet).
        let track_row = self.row(&track_long, signal).await?;
        let shown = track_row.get("noteColumns").and_then(Value::as_u64).unwrap_or(1);
        let delays = track_row.pointer("/subColumns/delay").and_then(Value::as_bool).unwrap_or(false);
        let want_delays = cells.iter().any(|cell| cell.delay > 0) && !delays;
        let mut shown_now = None;
        if shown < columns || want_delays {
            let mut input = json!({"ref": track});
            if shown < columns {
                input["noteColumns"] = json!(columns);
            }
            if want_delays {
                input["delay"] = json!(true);
            }
            shown_now = Some(self.change("track_columns", input, signal).await?);
        }
        let mut answer = self.change("notes_write", lay_input(pattern, track, first, last, columns, &cells, instrument), signal).await?;
        if let Some(shown_now) = shown_now {
            answer["shown"] =
                json!({"change": shown_now.get("change"), "said": "the track shows the note columns and delays the slices need now"});
        }
        Ok(Laid {
            answer,
            range: json!({"patternRef": pattern, "trackRef": track, "first": first, "last": last}),
            words: format!("lines {first:02} to {last:02}"),
            data: json!({"first": first, "lpb": lpb, "instrument": instrument, "columns": columns}),
        })
    }

    fn take_lines(&self, laid: &Laid, steps: &[Step]) -> Value {
        let first = laid.data.get("first").and_then(Value::as_u64).unwrap_or(0);
        let lpb = laid.data.get("lpb").and_then(Value::as_u64).unwrap_or(4);
        let instrument = laid.data.get("instrument").and_then(Value::as_u64).unwrap_or(0);
        let cells = cells(steps, first, lpb);
        let mut lines: Vec<Value> = Vec::new();
        for cell in &cells {
            let mut note = json!({"col": cell.col, "note": cell.note, "instrument": instrument});
            if cell.delay > 0 {
                note["delay"] = json!(cell.delay);
            }
            match lines.iter_mut().find(|line| line["line"] == json!(cell.line)) {
                Some(line) => {
                    if let Some(notes) = line["notes"].as_array_mut() {
                        notes.push(note);
                    }
                }
                None => lines.push(json!({"line": cell.line, "notes": [note]})),
            }
        }
        Value::Array(lines)
    }

    async fn render(&self, laid: &Laid, seconds: f64, signal: &Signal) -> Result<(String, f64), String> {
        if self.renderer.listening_now() {
            return Err("Kumi is listening to something else just now".into());
        }
        let pattern = laid.range.get("patternRef").and_then(Value::as_str).unwrap_or("");
        let track = self.plan.lengthen(laid.range.get("trackRef").and_then(Value::as_str).unwrap_or(""))?;
        let long = self.plan.lengthen(pattern)?;
        // Where the lines play: the slot named, or the first slot playing the pattern.
        let slot = if long.contains(":seq:") {
            long.clone()
        } else {
            let row = self.row(&long, signal).await?;
            row.get("seqRef")
                .and_then(Value::as_str)
                .map(str::to_owned)
                .ok_or("no slot of the sequence plays that pattern: Renoise can't render it")?
        };
        let rows = self.plan.rows_through(json!({"kind": "sequence", "limit": 200}), 8, signal).await?;
        let start = rows
            .iter()
            .find(|row| row.get("ref").and_then(Value::as_str) == Some(slot.as_str()))
            .and_then(|row| row.get("positionBeats"))
            .and_then(Value::as_f64)
            .ok_or("that slot isn't in the sequence now: read again")?;
        let song = self.song(signal).await?;
        let first = laid.data.get("first").and_then(Value::as_u64).unwrap_or(0) as f64;
        let lpb = laid.data.get("lpb").and_then(Value::as_u64).unwrap_or(4) as f64;
        let _listening = self.renderer.begin_listen();
        let sources = vec![json!({"id": "t", "what": "track", "ref": track, "stage": "pre-fader"})];
        let window = Window { from: start + first / lpb, beats: seconds * song.bpm / 60. };
        let rendered = self.renderer.render_sources(sources, window, signal).await;
        self.renderer.put_tempo_back().await;
        match rendered {
            Ok(Ok((files, _))) => {
                files.get("t").map(|(render, _)| (render.file.clone(), render.start)).ok_or_else(|| "the render made no file".to_string())
            }
            Ok(Err(why)) => Err(why),
            Err(error) => Err(error.to_string()),
        }
    }

    fn cant_render(&self) -> Option<String> {
        self.renderer.why_not()
    }
}

/// Why a range from `first` to `last` doesn't fit a pattern of `lines`, in words with the way out, or None.
fn too_long(length: u64, lpb: u64, first: u64, last: u64, lines: u64) -> Option<String> {
    (last >= lines).then(|| {
        format!(
            "the recording is {length} lines at {lpb} lines a beat, from line {first:02} to {last:02}, and the pattern has {lines}: lay it from an earlier line, in a longer pattern, or lengthen this one (pattern_length)"
        )
    })
}

/// The lines from `first` to `last` whose first `columns` note columns hold a note (shown or hidden), as the line read gives them.
fn held_lines(rows: &[Value], first: u64, last: u64, columns: u64) -> Vec<u64> {
    rows.iter()
        .filter_map(|row| {
            let line = row.get("line").and_then(Value::as_u64)?;
            let shown = row.get("notes").and_then(Value::as_array).into_iter().flatten();
            let mut notes = shown.chain(row.pointer("/hidden/notes").and_then(Value::as_array).into_iter().flatten());
            let taken = notes.any(|note| note.get("col").and_then(Value::as_u64).is_some_and(|col| col <= columns));
            (line >= first && line <= last && taken).then_some(line)
        })
        .collect()
}

/// sample_slices' input: a slice marker at each frame not cut yet (Renoise counts frames from 1), and, with `replace`, the old
/// ones that aren't among them deleted. A marker already where it should be stays.
fn cut_input(recording: &Recording, frames: &[usize], replace: bool) -> Value {
    let insert: Vec<usize> = frames.iter().filter(|frame| !recording.cuts.contains(frame)).map(|frame| frame + 1).collect();
    let mut input = json!({"ref": recording.reference});
    if !insert.is_empty() {
        input["insert"] = json!(insert);
    }
    if replace {
        let delete: Vec<usize> = recording.cuts.iter().filter(|frame| !frames.contains(frame)).map(|frame| frame + 1).collect();
        if !delete.is_empty() {
            input["delete"] = json!(delete);
        }
    }
    input
}

/// notes_write's input: each slice's note on its line, column and delay; the first `columns` note columns cleared on the range's
/// other lines (and past a line's own slices), so what plays there is the chop alone.
fn lay_input(pattern: &str, track: &str, first: u64, last: u64, columns: u64, cells: &[Cell], instrument: u64) -> Value {
    let mut lines: Vec<Value> = Vec::new();
    for line in first..=last {
        let mut notes: Vec<Value> = Vec::new();
        for col in 1..=columns {
            match cells.iter().find(|cell| cell.line == line && cell.col == col) {
                Some(cell) => {
                    let mut note = json!({"col": col, "note": cell.note, "instrument": instrument});
                    if cell.delay > 0 {
                        note["delay"] = json!(cell.delay);
                    }
                    notes.push(note);
                }
                None => notes.push(json!({"col": col, "clear": true})),
            }
        }
        lines.push(json!({"line": line, "notes": notes}));
    }
    json!({"patternRef": pattern, "trackRef": track, "lines": lines})
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::integrations::renoise_changes::CHANGES;
    use crate::kernel::schema::problem;

    fn schema_of(tool: &str) -> Value {
        Value::Object(CHANGES.kinds.iter().find(|kind| kind.tool == tool).unwrap().input_schema())
    }

    #[test]
    fn a_cut_again_changes_only_the_markers_that_move_and_fits_sample_slices() {
        let recording = Recording {
            reference: "1:sample:2.1".into(),
            name: "Amen".into(),
            file: "/x.wav".into(),
            rate: 44_100.,
            frames: 1000,
            cuts: vec![0, 500],
        };
        let cut = cut_input(&recording, &[0, 250, 700], true);
        assert_eq!(problem(&schema_of("sample_slices"), &cut), None, "{cut}");
        assert_eq!(cut["insert"], json!([251, 701]), "Renoise counts frames from 1; the marker at 1 stays");
        assert_eq!(cut["delete"], json!([501]));
        assert!(cut_input(&recording, &[3], false).get("delete").is_none());
    }

    #[test]
    fn slices_land_on_lines_and_delays_and_two_on_one_line_take_a_column_each() {
        // At 4 lines a beat: beat 0.0283 is line 00 delay 29; two 32nds on line 03 share it.
        let steps = vec![
            Step { note: "C#4".into(), beat: 29. / 1024. },
            Step { note: "D-4".into(), beat: 0.75 },
            Step { note: "D#4".into(), beat: 0.75 + 0.125 },
        ];
        let cells = cells(&steps, 16, 4);
        assert_eq!(cells[0], Cell { line: 16, col: 1, note: "C#4".into(), delay: 29 });
        assert_eq!((cells[1].line, cells[1].col, cells[1].delay), (19, 1, 0));
        assert_eq!((cells[2].line, cells[2].col, cells[2].delay), (19, 2, 128), "the second on line 19 is in column 2");
        let lay = lay_input("1:pattern:1", "1:track:2", 16, 19, 2, &cells, 1);
        assert_eq!(problem(&schema_of("notes_write"), &lay), None, "{lay}");
        assert_eq!(lay["lines"][0]["notes"][0], json!({"col": 1, "note": "C#4", "instrument": 1, "delay": 29}));
        assert_eq!(lay["lines"][0]["notes"][1], json!({"col": 2, "clear": true}), "the other column is cleared");
        assert_eq!(lay["lines"][1]["notes"], json!([{"col": 1, "clear": true}, {"col": 2, "clear": true}]));
        assert_eq!(lay["lines"][3]["notes"][1], json!({"col": 2, "note": "D#4", "instrument": 1, "delay": 128}));
        assert_eq!(lay["lines"].as_array().unwrap().len(), 4);
    }

    #[test]
    fn notes_in_the_columns_a_chop_needs_are_found_and_a_range_past_the_pattern_is_said() {
        let rows = vec![
            json!({"line": 3, "notes": [{"col": 1, "note": "C-4"}]}),
            json!({"line": 5, "notes": [{"col": 3, "note": "E-4"}]}),
            json!({"line": 6, "hidden": {"notes": [{"col": 2, "note": "G-4"}]}}),
            json!({"line": 40, "notes": [{"col": 1, "note": "C-4"}]}),
        ];
        assert_eq!(held_lines(&rows, 0, 31, 2), vec![3, 6], "column 3 isn't needed; line 40 is past the range; hidden counts");
        assert!(held_lines(&rows, 8, 31, 2).is_empty());
        let why = too_long(128, 8, 0, 127, 64).unwrap();
        assert!(why.starts_with("the recording is 128 lines at 8 lines a beat, from line 00 to 127, and the pattern has 64"), "{why}");
        assert!(too_long(64, 4, 0, 63, 64).is_none());
    }

    #[test]
    fn a_samples_instrument_is_read_from_its_ref() {
        assert_eq!(instrument_of("7:sample:3.1"), Some(("7:instrument:3".to_string(), 2)));
        assert_eq!(instrument_of("7:track:3"), None);
    }
}
