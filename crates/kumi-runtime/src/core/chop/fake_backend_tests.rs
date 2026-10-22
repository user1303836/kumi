//! The chop tool against a fake DAW (no DAW): a break written as a sound file, planned, cut on the song's grid, laid out to
//! play it as recorded, heard back (the fake renders the recording itself, as a DAW would play the slices in order) and its
//! lighter densities handed to the variations tool as takes.
use super::backend::{ChopBackend, Key, Laid, Recording, Song, Step, Words};
use super::hits::tests::break_bar;
use super::tool::Chop;
use crate::core::contracts::{JsonObject, KernelTool, ToolResult};
use crate::core::errors::RuntimeError;
use async_trait::async_trait;
use kumi_common::abort::Signal;
use serde_json::{json, Value};
use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

static WORDS: Words = Words { app: "Fake", source: "a recording", place: "at: where", cut: "slice", at_schema: r#"{"type":"object"}"# };

const RATE: f64 = 44_100.;

struct Fake {
    file: PathBuf,
    frames: usize,
    playing: bool,
    cuts: RefCell<Vec<usize>>,
    replaced: RefCell<Vec<bool>>,
    laid: RefCell<Vec<(Vec<Step>, u32)>>,
}

#[async_trait(?Send)]
impl ChopBackend for Rc<Fake> {
    fn words(&self) -> &'static Words {
        &WORDS
    }

    async fn recording(&self, reference: &str, _signal: &Signal) -> Result<Recording, String> {
        Ok(Recording {
            reference: reference.into(),
            name: "Think".into(),
            file: self.file.to_string_lossy().into_owned(),
            rate: RATE,
            frames: self.frames,
            cuts: self.cuts.borrow().clone(),
        })
    }

    async fn song(&self, _signal: &Signal) -> Result<Song, String> {
        Ok(Song { bpm: 170., lpb: 4, beats_per_bar: 4., playing: self.playing })
    }

    async fn cut(&self, _recording: &Recording, frames: &[usize], replace: bool, _signal: &Signal) -> Result<Value, String> {
        *self.cuts.borrow_mut() = frames.to_vec();
        self.replaced.borrow_mut().push(replace);
        Ok(json!({"change": "c1"}))
    }

    async fn keys(&self, _recording: &Recording, _signal: &Signal) -> Result<Vec<Key>, String> {
        Ok(self.cuts.borrow().iter().enumerate().map(|(k, frame)| Key { frame: *frame, note: format!("K{k}") }).collect())
    }

    async fn lay(&self, _recording: &Recording, at: &Value, steps: &[Step], lines: u32, _signal: &Signal) -> Result<Laid, String> {
        self.laid.borrow_mut().push((steps.to_vec(), lines));
        let first = at.get("line").and_then(Value::as_u64).unwrap_or(0) as u32;
        Ok(Laid {
            answer: json!({"change": "c2", "cell": {"instrument": 3}}),
            range: json!({"patternRef": "p", "trackRef": "t", "first": first, "last": first + lines - 1}),
            first,
            last: first + lines - 1,
        })
    }

    async fn render(&self, _laid: &Laid, _seconds: f64, _signal: &Signal) -> Result<(String, f64), String> {
        // (the slices played in order on the grid are the recording again)
        Ok((self.file.to_string_lossy().into_owned(), 0.))
    }

    fn cant_render(&self) -> Option<String> {
        None
    }
}

#[derive(Default)]
struct FakeVariations {
    asked: RefCell<Vec<JsonObject>>,
}

#[async_trait(?Send)]
impl KernelTool for FakeVariations {
    fn name(&self) -> &str {
        "variations"
    }

    fn description(&self) -> &str {
        ""
    }

    fn input_schema(&self) -> JsonObject {
        JsonObject::new()
    }

    async fn execute(&self, input: JsonObject, _signal: Signal) -> Result<ToolResult, RuntimeError> {
        self.asked.borrow_mut().push(input);
        Ok(ToolResult { text: r#"{"takes": [{"take": "V1"}, {"take": "V2"}]}"#.into(), ..Default::default() })
    }
}

/// One bar of the break at 170 BPM (its frames cut to the bar), as a sound file.
fn the_break(folder: &std::path::Path) -> (PathBuf, usize) {
    let (mut mono, _) = break_bar(RATE);
    let bar = (60. / 170. * 4. * RATE).round() as usize;
    mono.truncate(bar);
    let file = folder.join("think.wav");
    crate::audio::decode::write_test_wav(&file, &mono, RATE as u32);
    (file, bar)
}

async fn run(tool: &Rc<dyn KernelTool>, input: Value) -> Result<Value, String> {
    let done = tool.execute(input.as_object().cloned().unwrap(), Signal::new()).await.unwrap();
    if done.is_error {
        return Err(done.text);
    }
    Ok(serde_json::from_str(&done.text).unwrap())
}

fn fake(folder: &std::path::Path, playing: bool) -> Rc<Fake> {
    let (file, frames) = the_break(folder);
    Rc::new(Fake { file, frames, playing, cuts: RefCell::default(), replaced: RefCell::default(), laid: RefCell::default() })
}

#[tokio::test(flavor = "current_thread")]
async fn a_break_is_planned_cut_on_the_grid_laid_out_heard_back_and_its_lighter_densities_offered_as_takes() {
    let folder = tempfile::tempdir().unwrap();
    let fake = fake(folder.path(), false);
    let variations = Rc::new(FakeVariations::default());
    let tool = Chop::new(fake.clone(), Some(variations.clone() as Rc<dyn KernelTool>)).tool();

    let planned = run(&tool, json!({"ref": "s:1"})).await.unwrap();
    assert!(planned["tempo"].as_str().unwrap().starts_with("1 bar at 170.0 BPM"), "{planned}");
    assert_eq!(planned["hits"].as_array().unwrap().len(), 12, "{planned}");
    assert!(planned["hits"][5].as_str().unwrap().ends_with("snare (ghost)"), "{planned}");
    assert_eq!(planned["densities"]["main"], "10: the main hits, no ghosts or rolls");
    assert_eq!(planned["densities"]["kick-snare"], "4: kick and snare only");
    assert!(fake.cuts.borrow().is_empty(), "a plan cuts nothing");

    let cut = run(&tool, json!({"ref": "s:1", "action": "cut"})).await.unwrap();
    assert_eq!(cut["cuts"], 12, "{cut}");
    assert!(cut["grid"].is_string(), "the break plays at the song's tempo: {cut}");
    let first = cut["keys"][0].as_str().unwrap();
    assert!(first.starts_with("K0: 0.0") && first.ends_with(" s kick"), "{cut}");
    // Each cut on the grid: a line and delay start it exactly.
    for frame in fake.cuts.borrow().iter() {
        let (line, delay) = super::place(*frame as f64 / RATE, 170., 4);
        let back = (line as f64 + delay as f64 / 256.) * (60. / 170. / 4.) * RATE;
        assert!((back - *frame as f64).abs() < 0.51, "{frame}");
    }
    // Cut again only when asked.
    let refused = run(&tool, json!({"ref": "s:1", "action": "cut", "density": "main"})).await.unwrap_err();
    assert!(refused.contains("replace: true"), "{refused}");
    let again = run(&tool, json!({"ref": "s:1", "action": "cut", "density": "main", "replace": true})).await.unwrap();
    assert_eq!(again["cuts"], 10);
    assert_eq!(*fake.replaced.borrow(), vec![false, true]);
    run(&tool, json!({"ref": "s:1", "action": "cut", "replace": true})).await.unwrap();

    let laid = run(&tool, json!({"ref": "s:1", "action": "lay", "at": {"patternRef": "p", "trackRef": "t", "line": 16}})).await.unwrap();
    assert_eq!(laid["lines"], "16 to 31", "{laid}");
    let (steps, lines) = fake.laid.borrow()[0].clone();
    assert_eq!(lines, 16, "a bar at 4 lines a beat");
    // (10 ms into the bar is line 00, delay 29 of 256; the snare on the second beat is line 04)
    assert!(steps[0].line == 0 && (28..=30).contains(&steps[0].delay), "{:?}", steps[0]);
    assert!(steps[3].line == 4 && (28..=30).contains(&steps[3].delay), "{:?}", steps[3]);
    let heard = laid["heard"].as_str().unwrap();
    assert!(heard.starts_with("every hit lands within"), "{heard}");
    assert!(heard.contains("nulls to"), "{heard}");
    // The lighter densities as takes of the range just laid out.
    let asked = variations.asked.borrow();
    assert_eq!(asked.len(), 1);
    assert_eq!(asked[0]["source"], json!({"patternRef": "p", "trackRef": "t", "first": 16, "last": 31}));
    let takes = asked[0]["takes"].as_array().unwrap();
    assert_eq!(takes.len(), 2);
    assert_eq!(takes[0]["direction"], "the main hits, no ghosts or rolls");
    assert_eq!(takes[0]["lines"].as_array().unwrap().len(), 10);
    assert_eq!(takes[1]["lines"].as_array().unwrap().len(), 4);
    let cell = &takes[1]["lines"][0]["notes"][0];
    assert_eq!(
        (takes[1]["lines"][0]["line"].as_u64(), cell["note"].as_str(), cell["instrument"].as_u64()),
        (Some(16), Some("K0"), Some(3))
    );
    assert!((28..=30).contains(&cell["delay"].as_u64().unwrap()), "{cell}");
    assert_eq!(laid["takes"]["takes"][1]["take"], "V2");
}

#[tokio::test(flavor = "current_thread")]
async fn nothing_renders_while_the_daw_plays_and_lay_needs_cuts() {
    let folder = tempfile::tempdir().unwrap();
    let fake = fake(folder.path(), true);
    let tool = Chop::new(fake.clone(), None).tool();
    let refused = run(&tool, json!({"ref": "s:1", "action": "lay", "at": {"patternRef": "p", "trackRef": "t"}})).await.unwrap_err();
    assert!(refused.contains("cut it first"), "{refused}");
    run(&tool, json!({"ref": "s:1", "action": "cut"})).await.unwrap();
    let laid = run(&tool, json!({"ref": "s:1", "action": "lay", "at": {"patternRef": "p", "trackRef": "t"}, "takes": true})).await.unwrap();
    assert_eq!(laid["heard"], "not heard: a render would stop Fake playing");
    assert!(laid.get("takes").is_none(), "no variations tool, no takes");
    assert!(run(&tool, json!({"ref": "s:1", "action": "cut", "density": "most", "replace": true}))
        .await
        .unwrap_err()
        .contains("density is"));
}

#[tokio::test(flavor = "current_thread")]
async fn a_break_slower_than_the_song_is_said_and_cut_where_it_hits() {
    let folder = tempfile::tempdir().unwrap();
    let fake = fake(folder.path(), false);
    let tool = Chop::new(fake.clone(), None).tool();
    // Played at 85 BPM: the song (170) plays it twice as fast as recorded.
    let cut = run(&tool, json!({"ref": "s:1", "action": "cut", "bpm": 85})).await.unwrap();
    assert!(cut.get("grid").is_none(), "{cut}");
    let laid = run(&tool, json!({"ref": "s:1", "action": "lay", "bpm": 85, "at": {"patternRef": "p", "trackRef": "t"}})).await.unwrap();
    assert!(laid["note"].as_str().unwrap().contains("cut short by the next"), "{laid}");
    assert_eq!(fake.laid.borrow()[0].1, 8, "two beats of the song's lines");
    // Told it's two bars: twice the tempo, gaps between the slices.
    let laid = run(&tool, json!({"ref": "s:1", "action": "lay", "bars": 2, "takes": false, "at": {"patternRef": "p", "trackRef": "t"}}))
        .await
        .unwrap();
    assert!(laid["note"].as_str().unwrap().contains("gaps open"), "{laid}");
    assert_eq!(fake.laid.borrow()[1].1, 32, "two bars of lines");
}
