//! The stems tool against a fake DAW (no DAW): a track of a bass, a pad and clicks rendered to a file; split, its stems are
//! written as files that play back as the track did, and put back beside it.
use super::backend::{StemsBackend, Words};
use super::tool::Stems;
use crate::core::contracts::{KernelTool, Render};
use async_trait::async_trait;
use kumi_common::abort::Signal;
use serde_json::{json, Value};
use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;

static WORDS: Words = Words { app: "Fake", placed: "new tracks" };

const RATE: u32 = 44_100;

struct Fake {
    dir: tempfile::TempDir,
    placed: RefCell<Vec<(String, PathBuf)>>,
    /// The files Kumi wrote, which it deletes once they're placed.
    made: RefCell<Vec<PathBuf>>,
}

fn the_track() -> Vec<f32> {
    let n = RATE as usize * 2;
    let mut mono: Vec<f32> = (0..n)
        .map(|k| {
            let t = k as f64 / RATE as f64;
            (0.3 * (2. * std::f64::consts::PI * 55. * t).sin() + 0.2 * (2. * std::f64::consts::PI * 440. * t).sin()) as f32
        })
        .collect();
    for click in (2_000..n).step_by(RATE as usize / 4) {
        for k in 0..64 {
            if let Some(value) = mono.get_mut(click + k) {
                *value += 0.8 * (-(k as f32) / 8.).exp() * if k % 2 == 0 { 1. } else { -1. };
            }
        }
    }
    mono
}

#[async_trait(?Send)]
impl StemsBackend for Rc<Fake> {
    fn words(&self) -> &'static Words {
        &WORDS
    }

    async fn render(&self, _track: &str, _from: f64, _beats: f64, _signal: &Signal) -> Result<(Render, String), String> {
        let file = self.dir.path().join("track.wav");
        crate::audio::decode::write_test_wav(&file, &the_track(), RATE);
        Ok((Render { file: file.to_string_lossy().into_owned(), start: 0., seconds: Some(2.) }, "Loop".into()))
    }

    fn folder(&self) -> PathBuf {
        self.dir.path().to_path_buf()
    }

    async fn place(&self, _track: &str, stems: &[(String, &Path)], _from: f64, _beats: f64, _signal: &Signal) -> Result<Value, String> {
        // (a copy of each, as a DAW keeps one: Kumi's own goes once placed)
        let kept = self.dir.path().join("kept");
        std::fs::create_dir_all(&kept).unwrap();
        for (k, (name, path)) in stems.iter().enumerate() {
            let copy = kept.join(format!("{k}.wav"));
            std::fs::copy(path, &copy).unwrap();
            self.placed.borrow_mut().push((name.clone(), copy));
            self.made.borrow_mut().push(path.to_path_buf());
        }
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
async fn a_tracks_stems_are_files_that_play_back_as_it_did() {
    let fake = Rc::new(Fake { dir: tempfile::tempdir().unwrap(), placed: RefCell::default(), made: RefCell::default() });
    let tool = Stems::new(fake.clone()).tool();
    let done = run(&tool, json!({"track": "t1", "fromBeat": 0, "beats": 4})).await.unwrap();
    assert_eq!(done["stems"], json!(["Loop drums", "Loop bass", "Loop other"]), "{done}");
    let placed = fake.placed.borrow().clone();
    assert_eq!(placed.len(), 3);
    assert!(fake.made.borrow().iter().all(|path| !path.exists()), "Kumi's own files go once placed");
    // (the three files add back up to the track, away from its edges)
    let mut sum = vec![0f32; RATE as usize * 2];
    for (_, path) in placed.iter() {
        let ([left, _], rate) = crate::audio::decode::read_stereo(path, &Signal::new(), 0., 2.).await.unwrap();
        assert_eq!(rate, RATE as f64);
        for (total, value) in sum.iter_mut().zip(&left) {
            *total += value;
        }
    }
    let track = the_track();
    let inner = 4096..track.len() - 4096;
    let error: f64 = inner.clone().map(|k| ((sum[k] - track[k]) as f64).powi(2)).sum();
    let energy: f64 = inner.map(|k| (track[k] as f64).powi(2)).sum();
    assert!(error < energy * 1e-3, "{error} of {energy}");
}

#[tokio::test(flavor = "current_thread")]
async fn what_stems_needs_is_said() {
    let tool = Stems::new(Rc::new(Fake { dir: tempfile::tempdir().unwrap(), placed: RefCell::default(), made: RefCell::default() })).tool();
    assert!(run(&tool, json!({"fromBeat": 0, "beats": 4})).await.unwrap_err().starts_with("track:"));
    assert!(run(&tool, json!({"track": "t1", "fromBeat": 0, "beats": 400})).await.unwrap_err().starts_with("fromBeat and beats"));
}
