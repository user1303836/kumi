//! The sample map's tool against a fake DAW (no DAW): real sound files named as libraries name them, heard and planned, made
//! as one change per instrument; an instrument's own samples laid out; the tidy.
use super::backend::{Held, MappingBackend, Words};
use super::tool::Mapping;
use super::Planned;
use crate::core::contracts::KernelTool;
use async_trait::async_trait;
use kumi_common::abort::Signal;
use serde_json::{json, Value};
use std::cell::RefCell;
use std::path::Path;
use std::rc::Rc;

static WORDS: Words = Words { app: "Fake", instrument: "instrument", notes: "a MIDI number", lowest: 12, highest: 131, first_key: 60 };

#[derive(Default)]
struct Fake {
    made: RefCell<Vec<(String, Planned, bool)>>,
    remapped: RefCell<Vec<(String, Planned)>>,
    held: RefCell<Vec<Held>>,
    listed: RefCell<Value>,
}

#[async_trait(?Send)]
impl MappingBackend for Rc<Fake> {
    fn words(&self) -> &'static Words {
        &WORDS
    }

    fn note_name(&self, midi: i32) -> String {
        const CLASSES: [&str; 12] = ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"];
        format!("{}{}", CLASSES[midi.rem_euclid(12) as usize], midi.div_euclid(12) - 1)
    }

    fn note_of(&self, value: &Value) -> Option<i32> {
        value.as_i64().map(|n| n as i32)
    }

    async fn samples_of(&self, _instrument: &str, _signal: &Signal) -> Result<(String, Vec<Held>), String> {
        Ok(("Keys".into(), self.held.borrow().clone()))
    }

    async fn make(&self, planned: &Planned, name: &str, drums: bool, _signal: &Signal) -> Result<Value, String> {
        self.made.borrow_mut().push((name.to_string(), planned.clone(), drums));
        Ok(json!({"change": format!("c{}", self.made.borrow().len())}))
    }

    async fn remap(&self, instrument: &str, planned: &Planned, _signal: &Signal) -> Result<Value, String> {
        self.remapped.borrow_mut().push((instrument.to_string(), planned.clone()));
        Ok(json!({"change": "c9"}))
    }

    async fn instruments(&self, _signal: &Signal) -> Result<Value, String> {
        Ok(self.listed.borrow().clone())
    }
}

/// A 16-bit mono WAV of a sine at `midi`, `seconds` long, at `amplitude`.
fn tone(path: &Path, midi: f64, amplitude: f64, seconds: f64) {
    let rate = 44_100u32;
    let hz = 440. * 2f64.powf((midi - 69.) / 12.);
    let frames = (rate as f64 * seconds) as u32;
    let mut bytes = Vec::new();
    bytes.extend(b"RIFF");
    bytes.extend((36 + frames * 2).to_le_bytes());
    bytes.extend(b"WAVEfmt ");
    bytes.extend(16u32.to_le_bytes());
    bytes.extend(1u16.to_le_bytes());
    bytes.extend(1u16.to_le_bytes());
    bytes.extend(rate.to_le_bytes());
    bytes.extend((rate * 2).to_le_bytes());
    bytes.extend(2u16.to_le_bytes());
    bytes.extend(16u16.to_le_bytes());
    bytes.extend(b"data");
    bytes.extend((frames * 2).to_le_bytes());
    for k in 0..frames {
        let value = amplitude * (2. * std::f64::consts::PI * hz * k as f64 / rate as f64).sin();
        bytes.extend(((value * 32767.) as i16).to_le_bytes());
    }
    std::fs::write(path, bytes).unwrap();
}

async fn run(tool: &Rc<dyn KernelTool>, input: Value) -> Result<Value, String> {
    let done = tool.execute(input.as_object().cloned().unwrap(), Signal::new()).await.unwrap();
    if done.is_error {
        return Err(done.text);
    }
    Ok(serde_json::from_str(&done.text).unwrap())
}

#[tokio::test(flavor = "current_thread")]
async fn a_folder_named_an_octave_low_is_heard_planned_and_made_in_one_change() {
    let folder = tempfile::tempdir().unwrap();
    // "C3" is middle C here (sounds as MIDI 60): Kumi hears it and says so.
    for (name, midi, amplitude) in
        [("Piano_C3_v1", 60., 0.2), ("Piano_C3_v2", 60., 0.8), ("Piano_G3_v1", 67., 0.2), ("Piano_G3_v2", 67., 0.8)]
    {
        tone(&folder.path().join(format!("{name}.wav")), midi, amplitude, 1.0);
    }
    std::fs::write(folder.path().join("notes.txt"), "not a sound").unwrap();
    let fake = Rc::new(Fake::default());
    let tool = Mapping::new(fake.clone()).tool();
    let folder_text = folder.path().to_string_lossy().into_owned();
    let planned = run(&tool, json!({"folder": folder_text})).await.unwrap();
    assert_eq!(planned["layout"], "keys", "{planned}");
    assert!(planned["middleC"].as_str().unwrap().starts_with("C3 (the pitch heard agrees in 4 of 4"), "{planned}");
    let samples = planned["instruments"][0]["samples"].as_array().unwrap();
    assert_eq!(samples.len(), 4, "the text file isn't a sound: {planned}");
    let first = samples.iter().find(|s| s["sample"] == "Piano_C3_v1").unwrap();
    assert_eq!(first["root"], "C4");
    assert_eq!(first["velocity"], json!([0, 63]));
    assert!(planned["next"].as_str().unwrap().contains("action apply"), "{planned}");
    assert!(fake.made.borrow().is_empty(), "a plan makes nothing");
    // Apply: one instrument, named for what the names share.
    let made = run(&tool, json!({"folder": folder_text, "action": "apply"})).await.unwrap();
    assert_eq!(made["made"], json!([{"change": "c1"}]), "{made}");
    let made = fake.made.borrow();
    let (name, planned, drums) = &made[0];
    assert_eq!(name, "Piano");
    assert!(!drums);
    assert_eq!(planned.samples.len(), 4);
    let g = planned.samples.iter().find(|p| p.name == "Piano_G3_v2").unwrap();
    assert_eq!((g.root, g.zone, g.velocity), (67, (64, 131), (64, 127)));
}

#[tokio::test(flavor = "current_thread")]
async fn an_instruments_own_samples_are_laid_out_and_articulations_need_a_part() {
    let folder = tempfile::tempdir().unwrap();
    let file = |name: &str, midi: f64| {
        let path = folder.path().join(format!("{name}.wav"));
        tone(&path, midi, 0.5, 0.8);
        path.to_string_lossy().into_owned()
    };
    let fake = Rc::new(Fake::default());
    *fake.held.borrow_mut() = vec![
        Held { reference: "s:1".into(), name: "Low".into(), file: Ok(file("low", 48.)) },
        Held { reference: "s:2".into(), name: "High".into(), file: Ok(file("high", 72.)) },
        Held { reference: "s:3".into(), name: "Broken".into(), file: Err("no audio".into()) },
    ];
    let tool = Mapping::new(fake.clone()).tool();
    let laid = run(&tool, json!({"instrumentRef": "in:1", "action": "apply", "layout": "keys"})).await.unwrap();
    assert_eq!(laid["leftOut"][0]["sample"], "Broken", "{laid}");
    let (instrument, roots): (String, Vec<(String, i32)>) = {
        let remapped = fake.remapped.borrow();
        (remapped[0].0.clone(), remapped[0].1.samples.iter().map(|p| (p.id.clone(), p.root)).collect())
    };
    assert_eq!(instrument, "in:1");
    assert_eq!(roots, vec![("s:1".to_string(), 48), ("s:2".to_string(), 72)], "found by their pitch");
    // Two articulations in one instrument: say which.
    *fake.held.borrow_mut() = vec![
        Held { reference: "s:1".into(), name: "Vln sus C4".into(), file: Ok(file("a", 60.)) },
        Held { reference: "s:2".into(), name: "Vln pizz C4".into(), file: Ok(file("b", 60.)) },
    ];
    let refused = run(&tool, json!({"instrumentRef": "in:1", "action": "apply"})).await.unwrap_err();
    assert!(refused.contains("2 articulations (pizzicato, sustain)"), "{refused}");
    run(&tool, json!({"instrumentRef": "in:1", "action": "apply", "part": "pizzicato"})).await.unwrap();
    assert_eq!(fake.remapped.borrow()[1].1.samples[0].id, "s:2");
}

#[tokio::test(flavor = "current_thread")]
async fn roots_set_by_hand_and_bad_input_are_answered_in_words() {
    let folder = tempfile::tempdir().unwrap();
    let path = folder.path().join("Texture.wav");
    // (noise-like: a chord of two far tones has no single pitch the tracker is sure of)
    tone(&path, 30., 0.5, 0.5);
    let fake = Rc::new(Fake::default());
    let tool = Mapping::new(fake.clone()).tool();
    let path_text = path.to_string_lossy().into_owned();
    let planned =
        run(&tool, json!({"files": [path_text], "layout": "keys", "roots": [{"sample": "Texture.wav", "note": 64}]})).await.unwrap();
    let sample = &planned["instruments"][0]["samples"][0];
    assert_eq!((sample["root"].as_str(), sample["by"].as_str()), (Some("E4"), Some("as asked")), "{planned}");
    assert!(run(&tool, json!({"files": [path_text], "roots": [{"sample": "Other", "note": 64}]}))
        .await
        .unwrap_err()
        .contains("no sample is Other"));
    assert!(run(&tool, json!({})).await.unwrap_err().contains("give files"));
    assert!(run(&tool, json!({"files": [path_text], "middleC": "C5"})).await.unwrap_err().contains("middleC is C3 or C4"));
    assert!(run(&tool, json!({"files": [path_text], "instrumentRef": "in:1"})).await.unwrap_err().contains("one of them"));
}

#[tokio::test(flavor = "current_thread")]
async fn the_tidy_reads_the_songs_instruments_through_the_backend() {
    let fake = Rc::new(Fake::default());
    *fake.listed.borrow_mut() = json!({"counted": true, "instruments": [
        {"ref": "in:0", "name": "Kick", "notes": 3, "samples": [{"name": "k", "print": "a"}]},
        {"ref": "in:1", "name": "Spare", "notes": 0, "samples": [{"name": "s", "print": "b"}]}
    ]});
    let tool = Mapping::new(fake.clone()).tool();
    let tidy = run(&tool, json!({"action": "tidy"})).await.unwrap();
    assert_eq!(tidy["noNotesPlay"], json!(["in:1 “Spare”"]));
    assert!(tidy["next"].as_str().unwrap().contains("only what the producer accepts"));
}
