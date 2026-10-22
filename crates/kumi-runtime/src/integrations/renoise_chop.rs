//! Renoise's backend for chopping (#401): only Renoise's facts. A recording is an instrument's first sample (its audio saved to
//! Kumi's folder to be heard), its cuts are slice markers (frames from 1) made by sample_slices in one change, each slice plays
//! on its own key (the read gives it), and the slices are laid out in a pattern track's first note column by notes_write, one
//! change, on lines and delays at the song's lines per beat; the render is of that track where a slot plays the pattern.
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
    place: "at: patternRef (or a sequence slot for its pattern), trackRef, and line (as Renoise numbers lines, from 00; 00 by default), in the track's first note column",
    cut: "slice",
    at_schema: r#"{"type":"object","additionalProperties":false,"required":["patternRef","trackRef"],"properties":{"patternRef":{"type":"string","description":"The pattern, or a sequence slot for its pattern"},"trackRef":{"type":"string"},"line":{"type":"integer","minimum":0}}}"#,
};

/// Pages of sample rows read for an instrument's slices (200 a page).
const SAMPLE_PAGES: usize = 2;

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
}

/// The instrument a sample's long ref ("<epoch>:sample:<i>.<s>") is of: its long ref and its number from 0.
fn instrument_of(sample: &str) -> Option<(String, u64)> {
    let (epoch, rest) = sample.split_once(":sample:")?;
    let (i, _) = rest.split_once('.')?;
    let i: u64 = i.parse().ok()?;
    Some((format!("{epoch}:instrument:{i}"), i.checked_sub(1)?))
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
        Ok(Song {
            bpm: row.get("bpm").and_then(Value::as_f64).unwrap_or(120.),
            lpb: row.get("lpb").and_then(Value::as_u64).unwrap_or(4).max(1) as u32,
            beats_per_bar: row.pointer("/metronome/beatsPerBar").and_then(Value::as_f64).unwrap_or(4.).max(1.),
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

    async fn lay(&self, recording: &Recording, at: &Value, steps: &[Step], length: u32, signal: &Signal) -> Result<Laid, String> {
        let (_, instrument) = instrument_of(&recording.reference).ok_or("that isn't a sample")?;
        let pattern = at.get("patternRef").and_then(Value::as_str).ok_or("at.patternRef: the pattern (or a slot for its pattern)")?;
        let track = at.get("trackRef").and_then(Value::as_str).ok_or("at.trackRef: the track")?;
        let first = at.get("line").and_then(Value::as_u64).unwrap_or(0) as u32;
        // (the range is the whole recording's length on the grid, its first note column cleared where no slice starts)
        let last = first + steps.last().map_or(0, |step| step.line).max(length.saturating_sub(1));
        let input = lay_input(pattern, track, first, last, steps, instrument);
        let mut answer = self.change("notes_write", input, signal).await?;
        answer["cell"] = json!({"instrument": instrument});
        Ok(Laid { answer, range: json!({"patternRef": pattern, "trackRef": track, "first": first, "last": last}), first, last })
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
        let _listening = self.renderer.begin_listen();
        let sources = vec![json!({"id": "t", "what": "track", "ref": track, "stage": "pre-fader"})];
        let window = Window { from: start + laid.first as f64 / song.lpb as f64, beats: seconds * song.bpm / 60. };
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

/// sample_slices' input: a slice marker at each frame (Renoise counts frames from 1), the old ones deleted when `replace`.
fn cut_input(recording: &Recording, frames: &[usize], replace: bool) -> Value {
    let mut input = json!({"ref": recording.reference, "insert": frames.iter().map(|frame| frame + 1).collect::<Vec<_>>()});
    if replace && !recording.cuts.is_empty() {
        input["delete"] = json!(recording.cuts.iter().map(|frame| frame + 1).collect::<Vec<_>>());
    }
    input
}

/// notes_write's input: each slice's note on its line and delay in the first note column, that column cleared on the range's
/// other lines.
fn lay_input(pattern: &str, track: &str, first: u32, last: u32, steps: &[Step], instrument: u64) -> Value {
    let mut lines: Vec<Value> = Vec::new();
    for line in first..=last {
        // (two slices on one line can't both start in one column: the later wins, as Renoise would play it)
        match steps.iter().rfind(|step| first + step.line == line) {
            Some(step) => {
                let mut cell = json!({"col": 1, "note": step.note, "instrument": instrument});
                if step.delay > 0 {
                    cell["delay"] = json!(step.delay);
                }
                lines.push(json!({"line": line, "notes": [cell]}));
            }
            None => lines.push(json!({"line": line, "notes": [{"col": 1, "clear": true}]})),
        }
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
    fn the_cuts_and_the_lines_fit_each_kinds_input() {
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
        assert_eq!(cut["insert"], json!([1, 251, 701]), "Renoise counts frames from 1");
        assert_eq!(cut["delete"], json!([1, 501]));
        assert!(cut_input(&recording, &[3], false).get("delete").is_none());
        let steps = vec![Step { note: "C#4".into(), line: 0, delay: 29 }, Step { note: "D-4".into(), line: 2, delay: 0 }];
        let lay = lay_input("1:pattern:1", "1:track:2", 16, 19, &steps, 1);
        assert_eq!(problem(&schema_of("notes_write"), &lay), None, "{lay}");
        assert_eq!(lay["lines"][0], json!({"line": 16, "notes": [{"col": 1, "note": "C#4", "instrument": 1, "delay": 29}]}));
        assert_eq!(lay["lines"][1], json!({"line": 17, "notes": [{"col": 1, "clear": true}]}), "a line with no slice is cleared");
        assert_eq!(lay["lines"][2]["notes"][0].get("delay"), None);
        assert_eq!(lay["lines"].as_array().unwrap().len(), 4);
    }

    #[test]
    fn a_samples_instrument_is_read_from_its_ref() {
        assert_eq!(instrument_of("7:sample:3.1"), Some(("7:instrument:3".to_string(), 2)));
        assert_eq!(instrument_of("7:track:3"), None);
    }
}
