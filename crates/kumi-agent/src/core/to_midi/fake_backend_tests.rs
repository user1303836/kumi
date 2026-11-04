//! The audio_to_midi tool against a fake DAW (no DAW): a bar of a break at 170 BPM (kicks, snares, ghosts and hats, the
//! chop's own test break) rendered to a file; as drums, each hit is a note on its drum key where it falls, written on a new
//! track named after the source.
use super::backend::{ToMidiBackend, Words};
use super::tool::ToMidi;
use super::{MidiNote, HAT, KICK, SNARE};
use crate::core::chop::hits::{tests::break_bar, Role};
use async_trait::async_trait;
use kumi_common::abort::Signal;
use kumi_types::contracts::{JsonObject, KernelTool, Render};
use serde_json::{json, Value};
use std::cell::RefCell;
use std::rc::Rc;

static WORDS: Words = Words { app: "Fake", written: "a new track", options: "none", options_schema: r#"{"type":"object"}"# };

const RATE: f64 = 48_000.;

struct Fake {
    dir: tempfile::TempDir,
    written: RefCell<Vec<(String, Vec<MidiNote>)>>,
}

#[async_trait(?Send)]
impl ToMidiBackend for Rc<Fake> {
    fn words(&self) -> &'static Words {
        &WORDS
    }

    async fn render(&self, _track: &str, _from: f64, _beats: f64, _signal: &Signal) -> Result<(Render, String), String> {
        let (mono, _) = break_bar(RATE);
        let file = self.dir.path().join("break.wav");
        kumi_media::audio::decode::write_test_wav(&file, &mono, RATE as u32);
        // (a bar at 170 BPM: the window the render was asked for)
        Ok((Render { file: file.to_string_lossy().into_owned(), start: 0., seconds: Some(4. * 60. / 170.) }, "Amen".into()))
    }

    async fn tempo(&self, _from: f64, _signal: &Signal) -> Result<f64, String> {
        Ok(170.)
    }

    fn note_name(&self, pitch: u8) -> String {
        format!("n{pitch}")
    }

    async fn write(
        &self,
        _track: &str,
        name: &str,
        notes: &[MidiNote],
        _drums: bool,
        _from: f64,
        _beats: f64,
        _options: &JsonObject,
        _signal: &Signal,
    ) -> Result<Value, String> {
        self.written.borrow_mut().push((name.into(), notes.to_vec()));
        Ok(json!({"change": "c1"}))
    }
}

async fn run(tool: &Rc<dyn KernelTool>, input: Value) -> Result<Value, String> {
    let done = tool.execute(input.as_object().cloned().unwrap(), Signal::new()).await.unwrap();
    if done.is_error {
        return Err(done.text);
    }
    Ok(serde_json::from_str(&done.text).unwrap())
}

#[tokio::test(flavor = "current_thread")]
async fn a_breaks_hits_are_drum_notes_on_their_keys_where_they_fall() {
    let fake = Rc::new(Fake { dir: tempfile::tempdir().unwrap(), written: RefCell::default() });
    let tool = ToMidi::new(fake.clone()).tool();
    let done = run(&tool, json!({"track": "t1", "as": "drums", "fromBeat": 0, "beats": 4})).await.unwrap();
    assert_eq!(done["track"], "Amen drums", "{done}");
    let written = fake.written.borrow();
    let (name, notes) = &written[0];
    assert_eq!(name, "Amen drums");
    // (the break's truth: 16ths at 170 BPM, 441 frames in)
    let (_, truth) = break_bar(RATE);
    let beat_of = |frame: usize| frame as f64 / RATE * 170. / 60.;
    for (frame, role, ghost) in truth {
        let key = match role {
            Role::Kick => KICK,
            Role::Snare => SNARE,
            Role::Hat => HAT,
        };
        let found = notes.iter().find(|note| note.pitch == key && (note.start - beat_of(frame)).abs() < 0.05);
        let note = found.unwrap_or_else(|| panic!("a {role:?} at beat {:.2}: {notes:?}", beat_of(frame)));
        if ghost {
            assert!(note.velocity <= 60, "a ghost is soft: {note:?}");
        }
    }
    assert!(done["heard"].as_str().unwrap().contains("kicks"), "{done}");
}

#[tokio::test(flavor = "current_thread")]
async fn what_audio_to_midi_needs_is_said() {
    let fake = Rc::new(Fake { dir: tempfile::tempdir().unwrap(), written: RefCell::default() });
    let tool = ToMidi::new(fake).tool();
    assert!(run(&tool, json!({"as": "drums", "fromBeat": 0, "beats": 4})).await.unwrap_err().starts_with("track:"));
    assert!(run(&tool, json!({"track": "t1", "as": "chords", "fromBeat": 0, "beats": 4})).await.unwrap_err().starts_with("as is drums"));
    assert!(run(&tool, json!({"track": "t1", "fromBeat": 0, "beats": 500})).await.unwrap_err().starts_with("fromBeat and beats"));
}

struct Provider {
    result: Result<Option<Value>, String>,
    calls: RefCell<Vec<JsonObject>>,
}

#[async_trait(?Send)]
impl kumi_types::contracts::AudioTranscriber for Provider {
    async fn preferred(&self) -> Result<bool, kumi_types::errors::RuntimeError> {
        Ok(self.result.as_ref().is_ok_and(Option::is_some))
    }
    async fn transcribe(&self, input: JsonObject, _: Signal) -> Result<Option<Value>, kumi_types::errors::RuntimeError> {
        self.calls.borrow_mut().push(input);
        self.result.clone().map_err(kumi_types::errors::RuntimeError::plain)
    }
}

#[tokio::test(flavor = "current_thread")]
async fn configured_provider_notes_keep_velocity_and_map_render_seconds_to_beats() {
    let fake = Rc::new(Fake { dir: tempfile::tempdir().unwrap(), written: RefCell::default() });
    let provider = Rc::new(Provider {
        result: Ok(Some(json!({
            "provider":"mirelo", "attribution":"Powered by Mirelo", "midi_path":"/output/transcription.mid",
            "notes":[{"pitch":60,"start":0.3,"end":0.6,"velocity":17},{"pitch":72,"start":1.2,"end":2.,"velocity":109}]
        }))),
        calls: RefCell::default(),
    });
    let tool = ToMidi::new(fake.clone()).tool().with_audio_transcriber(provider.clone()).unwrap();
    let done = run(&tool, json!({"track":"t1","as":"harmony","fromBeat":16,"beats":4,"provider":"local"})).await.unwrap();
    let written = fake.written.borrow();
    let notes = &written[0].1;
    assert_eq!(notes.iter().map(|n| n.velocity).collect::<Vec<_>>(), [17, 109]);
    assert!((notes[0].start - 0.85).abs() < 1e-9);
    assert!((notes[0].length - 0.85).abs() < 1e-9);
    assert!((notes[1].start + notes[1].length - 4.).abs() < 1e-9);
    assert_eq!(done["audio_model"]["attribution"], "Powered by Mirelo");
    let calls = provider.calls.borrow();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0]["provider"], "local");
    assert_eq!(calls[0]["audio_start_seconds"], 0.);
    assert!((calls[0]["audio_duration_seconds"].as_f64().unwrap() - 4. * 60. / 170.).abs() < 1e-9);
}

#[tokio::test(flavor = "current_thread")]
async fn quote_and_provider_failure_never_insert_or_silently_fall_back() {
    for (result, quote, fails) in [
        (Ok(Some(json!({"quote":true,"credits":2}))), true, false),
        (Err("provider unavailable".to_string()), false, true),
        (Ok(Some(json!({"midi_path":"/retained.mid"}))), false, true),
        (Ok(Some(json!({"notes":[{"pitch":140,"start":0.,"end":1.,"velocity":100}]}))), false, true),
    ] {
        let fake = Rc::new(Fake { dir: tempfile::tempdir().unwrap(), written: RefCell::default() });
        let provider = Rc::new(Provider { result, calls: RefCell::default() });
        let tool = ToMidi::new(fake.clone()).tool().with_audio_transcriber(provider.clone()).unwrap();
        let result = run(&tool, json!({"track":"t1","fromBeat":0,"beats":4,"quote":quote})).await;
        assert_eq!(result.is_err(), fails);
        assert!(fake.written.borrow().is_empty());
        assert_eq!(provider.calls.borrow().len(), 1);
    }
}

#[tokio::test(flavor = "current_thread")]
async fn disabled_provider_and_explicit_builtin_preserve_original_detector() {
    for builtin in [false, true] {
        let fake = Rc::new(Fake { dir: tempfile::tempdir().unwrap(), written: RefCell::default() });
        let provider = Rc::new(Provider { result: Ok(None), calls: RefCell::default() });
        let tool = ToMidi::new(fake.clone()).tool().with_audio_transcriber(provider.clone()).unwrap();
        let result =
            run(&tool, json!({"track":"t1","fromBeat":0,"beats":4,"provider":if builtin {"builtin"} else {"auto"}})).await.unwrap();
        assert!(result["heard"].as_str().unwrap().contains("kicks"));
        assert_eq!(provider.calls.borrow().len(), usize::from(!builtin));
        assert!(!fake.written.borrow()[0].1.is_empty());
    }
}
