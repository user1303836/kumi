//! The layers tool against a fake DAW (no DAW): a main break of clicks, one layer 4 ms late with its polarity flipped and one
//! in time, and their group through a compressor the fake models (an instant peak follower at its threshold and ratio).
//! Look proposes aligning and flipping the late layer; done, it's in time and with the main. Glue hears the group without
//! its compressor first (before adding one, or bypassed) and moves the threshold until the glue measures as asked.
use super::backend::{Compressor, LayersBackend, Words};
use super::tool::Layers;
use crate::core::contracts::{KernelTool, Render};
use async_trait::async_trait;
use kumi_common::abort::Signal;
use serde_json::{json, Value};
use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, BTreeSet};
use std::rc::Rc;

static WORDS: Words = Words { app: "Fake", nudge: "the track's delay", invert: "the layer's samples", compressor: "the Fake's compressor" };

const RATE: u32 = 48_000;
const LAYERS: [&str; 3] = ["main", "l1", "l2"];

struct Fake {
    dir: tempfile::TempDir,
    renders: Cell<usize>,
    delays: RefCell<BTreeMap<String, f64>>,
    flipped: RefCell<BTreeSet<String>>,
    compressor: RefCell<Option<Compressor>>,
    ratio: f64,
    done: RefCell<Vec<String>>,
    /// Whether the compressor was on at each render of the group.
    group_heard: RefCell<Vec<bool>>,
}

impl Fake {
    fn new(compressor: Option<(f64, bool)>, ratio: f64) -> Rc<Fake> {
        Rc::new(Fake {
            dir: tempfile::tempdir().unwrap(),
            renders: Cell::new(0),
            delays: RefCell::default(),
            flipped: RefCell::default(),
            compressor: RefCell::new(compressor.map(|(threshold, active)| comp(threshold, active))),
            ratio,
            done: RefCell::default(),
            group_heard: RefCell::default(),
        })
    }

    /// A layer as it plays: where its hits start, their polarity and level, moved by its delay and flipped when asked.
    fn layer(&self, track: &str) -> Vec<f32> {
        let (at, sign, level) = match track {
            "main" => (0.1, 1., 0.5),
            "l1" => (0.104, -1., 0.25),
            _ => (0.1, 1., 0.2),
        };
        let delay = self.delays.borrow().get(track).copied().unwrap_or(0.) / 1000.;
        let sign = if self.flipped.borrow().contains(track) { -sign } else { sign };
        clicks(at + delay, sign, level)
    }

    /// The group: its layers summed, through the compressor when it's on.
    fn group(&self) -> Vec<f32> {
        let mut sum = vec![0f32; RATE as usize * 4];
        for track in LAYERS {
            for (out, x) in sum.iter_mut().zip(self.layer(track)) {
                *out += x;
            }
        }
        let on = self.compressor.borrow().clone().filter(|found| found.active);
        self.group_heard.borrow_mut().push(on.is_some());
        match on {
            Some(found) => squash(&sum, found.threshold, self.ratio),
            None => sum,
        }
    }
}

fn comp(threshold: f64, active: bool) -> Compressor {
    Compressor { reference: "comp".into(), name: "Fake Comp".into(), threshold, lowest: -60., highest: 0., active }
}

/// Clicks with a decaying tone each half second from `from`, `sign` their polarity.
fn clicks(from: f64, sign: f32, level: f32) -> Vec<f32> {
    let rate = RATE as f64;
    let mut out = vec![0f32; RATE as usize * 4];
    let mut at = from;
    while at < 3.8 {
        let start = (at * rate).round() as usize;
        for k in 0..(0.15 * rate) as usize {
            let t = k as f32 / rate as f32;
            out[start + k] += sign * level * ((-t * 30.).exp() * (2. * std::f32::consts::PI * 80. * t).sin() + (-t * 400.).exp());
        }
        at += 0.5;
    }
    out
}

/// A compressor: an instant peak follower (50 ms release) taking `1 - 1/ratio` of each dB over the threshold.
fn squash(input: &[f32], threshold: f64, ratio: f64) -> Vec<f32> {
    let fall = (-1. / (0.05 * RATE as f64)).exp();
    let mut env = 0f64;
    input
        .iter()
        .map(|&x| {
            env = (x.abs() as f64).max(env * fall);
            let over = (20. * env.max(1e-9).log10() - threshold).max(0.);
            (x as f64 * 10f64.powf(-over * (1. - 1. / ratio) / 20.)) as f32
        })
        .collect()
}

#[async_trait(?Send)]
impl LayersBackend for Rc<Fake> {
    fn words(&self) -> &'static Words {
        &WORDS
    }

    async fn render(&self, tracks: &[String], _from: f64, _beats: f64, _signal: &Signal) -> Result<Vec<Render>, String> {
        let mut renders = Vec::new();
        for track in tracks {
            let mono = if track == "g" { self.group() } else { self.layer(track) };
            self.renders.set(self.renders.get() + 1);
            let path = self.dir.path().join(format!("r{}.wav", self.renders.get()));
            crate::audio::decode::write_test_wav(&path, &mono, RATE);
            renders.push(Render { file: path.to_string_lossy().into_owned(), start: 0., seconds: None });
        }
        Ok(renders)
    }

    async fn nudge(&self, track: &str, ms: f64, _from: f64, _beats: f64, _signal: &Signal) -> Result<Value, String> {
        *self.delays.borrow_mut().entry(track.to_owned()).or_insert(0.) += ms;
        self.done.borrow_mut().push(format!("nudge {track}"));
        Ok(json!({"change": "c"}))
    }

    async fn invert(&self, track: &str, _signal: &Signal) -> Result<Value, String> {
        let mut flipped = self.flipped.borrow_mut();
        if !flipped.remove(track) {
            flipped.insert(track.to_owned());
        }
        self.done.borrow_mut().push(format!("invert {track}"));
        Ok(json!({"change": "c"}))
    }

    async fn compressor(&self, group: &str, add: bool, _signal: &Signal) -> Result<Option<Compressor>, String> {
        if group != "g" {
            return Err("no such group".into());
        }
        if add && self.compressor.borrow().is_none() {
            *self.compressor.borrow_mut() = Some(comp(-12., true));
            self.done.borrow_mut().push("add".into());
        }
        Ok(self.compressor.borrow().clone())
    }

    async fn set_threshold(&self, _compressor: &Compressor, db: f64, _signal: &Signal) -> Result<Value, String> {
        if let Some(found) = self.compressor.borrow_mut().as_mut() {
            found.threshold = db;
        }
        self.done.borrow_mut().push(format!("threshold {db:.1}"));
        Ok(json!({"change": "c"}))
    }

    async fn bypass(&self, _compressor: &Compressor, bypass: bool, _signal: &Signal) -> Result<Value, String> {
        if let Some(found) = self.compressor.borrow_mut().as_mut() {
            found.active = !bypass;
        }
        self.done.borrow_mut().push(if bypass { "bypass" } else { "on" }.into());
        Ok(json!({"change": "c"}))
    }
}

async fn run(tool: &Rc<dyn KernelTool>, input: Value) -> Result<Value, String> {
    let done = tool.execute(input.as_object().cloned().unwrap(), Signal::new()).await.unwrap();
    if done.is_error {
        return Err(done.text);
    }
    Ok(serde_json::from_str(&done.text).unwrap())
}

/// A layer's offset in ms from look's words (late is positive), 0 when in time.
fn offset(row: &Value) -> f64 {
    let text = row["offset"].as_str().unwrap();
    if text.starts_with("in time") {
        return 0.;
    }
    let ms: f64 = text.split(" ms").next().unwrap().parse().unwrap();
    if text.contains("late") {
        ms
    } else {
        -ms
    }
}

#[tokio::test(flavor = "current_thread")]
async fn a_late_flipped_layer_is_proposed_aligned_and_inverted_and_sits_with_the_main_once_done() {
    let fake = Fake::new(None, 2.);
    let tool = Layers::new(fake.clone()).tool();
    let look = json!({"tracks": ["main", "l1", "l2"], "fromBeat": 0, "beats": 8, "group": "g"});
    let looked = run(&tool, look.clone()).await.unwrap();
    let (l1, l2) = (&looked["layers"][0], &looked["layers"][1]);
    let late = offset(l1);
    assert!((late - 4.).abs() < 1.5, "{looked}");
    assert!(l1["polarity"].as_str().unwrap().starts_with("inverted against the main"), "{looked}");
    assert!(l1["level"].as_str().unwrap().starts_with("-6."), "{looked}");
    assert_eq!((offset(l2), l2["polarity"].as_str()), (0., Some("with the main")), "{looked}");
    let next = looked["next"].as_str().unwrap();
    assert!(
        next.contains(&format!("align l1 by {:+.1} ms", -late)) && next.contains("invert l1") && next.contains("glue the group"),
        "{next}"
    );
    assert!(!next.contains("l2"), "{next}");
    assert_eq!(looked["group"], "no compressor: glue adds the Fake's compressor and sets it");
    // Done as proposed: in time, and with the main.
    run(&tool, json!({"action": "align", "track": "l1", "ms": -late, "fromBeat": 0, "beats": 8})).await.unwrap();
    run(&tool, json!({"action": "invert", "track": "l1"})).await.unwrap();
    assert_eq!(*fake.done.borrow(), ["nudge l1", "invert l1"]);
    let again = run(&tool, look).await.unwrap();
    assert!(offset(&again["layers"][0]).abs() < 1., "{again}");
    assert_eq!(again["layers"][0]["polarity"], "with the main", "{again}");
}

#[tokio::test(flavor = "current_thread")]
async fn glue_hears_the_group_without_its_compressor_first_and_moves_the_threshold_until_the_glue_is_as_asked() {
    // No compressor: the group heard before one is added.
    let fake = Fake::new(None, 2.);
    let tool = Layers::new(fake.clone()).tool();
    let glue = json!({"action": "glue", "group": "g", "fromBeat": 0, "beats": 8});
    let glued = run(&tool, glue.clone()).await.unwrap();
    assert_eq!(glued["ok"], true, "{glued}");
    assert_eq!(fake.done.borrow()[0], "add");
    assert!(!fake.group_heard.borrow()[0], "the reference without a compressor");
    assert!(fake.group_heard.borrow()[1..].iter().all(|on| *on));
    let last = glued["passes"].as_array().unwrap().last().unwrap()["glue"].as_f64().unwrap();
    assert!((last - 1.5).abs() <= 0.5, "{glued}");
    assert!(glued["glue"].as_str().unwrap().starts_with("Fake Comp takes"), "{glued}");
    // A compressor on already: bypassed for the reference, on again before the passes.
    let fake = Fake::new(Some((-3., true)), 2.);
    let tool = Layers::new(fake.clone()).tool();
    let glued = run(&tool, json!({"action": "glue", "group": "g", "fromBeat": 0, "beats": 8, "target": 1.2})).await.unwrap();
    assert_eq!(&fake.done.borrow()[..2], ["bypass", "on"], "{glued}");
    assert!(!fake.group_heard.borrow()[0]);
    assert_eq!(glued["ok"], true, "{glued}");
    assert!(fake.compressor.borrow().as_ref().unwrap().active);
    // A compressor that takes nothing: said, not chased down to its lowest.
    let fake = Fake::new(Some((-12., true)), 1.);
    let tool = Layers::new(fake.clone()).tool();
    let glued = run(&tool, glue).await.unwrap();
    assert_eq!(glued["ok"], false, "{glued}");
    assert!(glued["said"].as_str().unwrap().starts_with("the glue doesn't grow"), "{glued}");
    assert!(fake.done.borrow().iter().filter(|done| done.starts_with("threshold")).count() <= 1, "{:?}", fake.done.borrow());
}

#[tokio::test(flavor = "current_thread")]
async fn what_the_layers_tool_needs_is_said() {
    let tool = Layers::new(Fake::new(None, 2.)).tool();
    assert!(run(&tool, json!({"tracks": ["main"], "fromBeat": 0, "beats": 8}))
        .await
        .unwrap_err()
        .starts_with("tracks: the main break first"));
    assert!(run(&tool, json!({"tracks": ["main", "l1"]})).await.unwrap_err().starts_with("fromBeat and beats"));
    assert!(run(&tool, json!({"action": "align", "track": "l1", "ms": 0, "fromBeat": 0, "beats": 8}))
        .await
        .unwrap_err()
        .starts_with("ms:"));
    assert!(run(&tool, json!({"action": "glue", "fromBeat": 0, "beats": 8})).await.unwrap_err().starts_with("group:"));
    assert!(run(&tool, json!({"action": "glue", "group": "x", "fromBeat": 0, "beats": 8})).await.unwrap_err().contains("no such group"));
}
