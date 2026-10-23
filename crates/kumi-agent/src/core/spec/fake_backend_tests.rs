//! The delivery_spec tool against a fake DAW (no DAW): its mix a tone whose level follows its master fader, rendered to a file
//! each time. A check measures it against a spec; a fix moves the fader by the gain the spec needs and measures again, and
//! says what a fader can't do (peaks past the ceiling, a fader at its most, a noise floor).
use super::backend::{SpecBackend, Words};
use super::tool::DeliverySpec;
use async_trait::async_trait;
use kumi_common::abort::Signal;
use kumi_types::contracts::{KernelTool, Render};
use serde_json::{json, Value};
use std::cell::{Cell, RefCell};
use std::rc::Rc;

static WORDS: Words = Words { app: "Fake", fader_most: 6., limiter: "the Fake's limiter", gainer: "a gainer on the master" };

const RATE: u32 = 48_000;

struct Fake {
    dir: tempfile::TempDir,
    /// The master fader (dB).
    master: Cell<f64>,
    /// The mix before the fader: 1 kHz at this amplitude for this long, with spikes (peaks) and a quiet tail as asked.
    amplitude: f32,
    seconds: u32,
    spikes: f32,
    tail: Option<f32>,
    renders: Cell<usize>,
    moves: RefCell<Vec<f64>>,
}

impl Fake {
    fn new(amplitude: f32) -> Rc<Fake> {
        Rc::new(Fake {
            dir: tempfile::tempdir().unwrap(),
            master: Cell::new(0.),
            amplitude,
            seconds: 4,
            spikes: 0.,
            tail: None,
            renders: Cell::new(0),
            moves: RefCell::default(),
        })
    }
}

#[async_trait(?Send)]
impl SpecBackend for Rc<Fake> {
    fn words(&self) -> &'static Words {
        &WORDS
    }

    async fn render_mix(&self, _stretch: Option<(f64, f64)>, _signal: &Signal) -> Result<Render, String> {
        let gain = 10f32.powf(self.master.get() as f32 / 20.);
        let mut mono: Vec<f32> =
            (0..RATE * self.seconds).map(|k| self.amplitude * (2. * std::f32::consts::PI * 1000. * k as f32 / RATE as f32).sin()).collect();
        if self.spikes > 0. {
            for at in (0..mono.len()).step_by(RATE as usize / 2) {
                mono[at] = self.spikes;
            }
        }
        if let Some(tail) = self.tail {
            mono.extend((0..RATE).map(|k| if k % 2 == 0 { tail } else { -tail }));
        }
        let mono: Vec<f32> = mono.into_iter().map(|s| s * gain).collect();
        self.renders.set(self.renders.get() + 1);
        let path = self.dir.path().join(format!("mix{}.wav", self.renders.get()));
        kumi_media::audio::decode::write_test_wav(&path, &mono, RATE);
        Ok(Render { file: path.to_string_lossy().into_owned(), start: 0., seconds: None })
    }

    async fn master_db(&self, _signal: &Signal) -> Result<f64, String> {
        Ok(self.master.get())
    }

    async fn set_master_db(&self, db: f64, _signal: &Signal) -> Result<Value, String> {
        self.master.set(db);
        self.moves.borrow_mut().push(db);
        Ok(json!({"change": format!("c{}", self.moves.borrow().len())}))
    }
}

async fn run(tool: &Rc<dyn KernelTool>, input: Value) -> Result<Value, String> {
    let done = tool.execute(input.as_object().cloned().unwrap(), Signal::new()).await.unwrap();
    if done.is_error {
        return Err(done.text);
    }
    Ok(serde_json::from_str(&done.text).unwrap())
}

fn line<'a>(reply: &'a Value, what: &str) -> &'a Value {
    reply["lines"].as_array().unwrap().iter().find(|line| line["what"] == what).unwrap_or_else(|| panic!("{what}: {reply}"))
}

#[tokio::test(flavor = "current_thread")]
async fn a_quiet_mix_is_checked_against_spotify_fixed_by_its_master_fader_and_measured_again() {
    // (a 1 kHz tone at 0.12 peak: about -18 LUFS, a few dB under Spotify's; a second of it measures as four do)
    let fake = Rc::new(Fake { seconds: 1, ..Rc::try_unwrap(Fake::new(0.12)).ok().unwrap() });
    let tool = DeliverySpec::new(fake.clone()).tool();
    let checked = run(&tool, json!({"spec": "spotify"})).await.unwrap();
    assert_eq!(checked["ok"], false, "{checked}");
    assert_eq!(line(&checked, "integrated loudness")["ok"], false);
    assert_eq!(line(&checked, "true peak")["ok"], true);
    assert!(checked["gain"].as_str().unwrap().starts_with('+'), "{checked}");
    assert!(fake.moves.borrow().is_empty(), "a check moves nothing");
    let fixed = run(&tool, json!({"action": "fix", "spec": "Spotify"})).await.unwrap();
    assert_eq!(fixed["ok"], true, "{fixed}");
    assert_eq!(fixed["before"]["ok"], false);
    assert_eq!(fixed["changes"], json!(["c1"]), "one move of the fader");
    let integrated: f64 =
        line(&fixed["after"], "integrated loudness")["measured"].as_str().unwrap().trim_end_matches(" LUFS").parse().unwrap();
    assert!((integrated + 14.).abs() < 0.5, "{integrated}");
    // Measured again: it passes, and a fix moves nothing.
    let again = run(&tool, json!({"action": "fix", "spec": "spotify"})).await.unwrap();
    assert!(again["changes"].as_array().unwrap().is_empty(), "{again}");
    assert!(run(&tool, json!({"spec": "vinyl"})).await.unwrap_err().contains("isn't a spec"));
    assert_eq!(run(&tool, json!({"action": "list"})).await.unwrap()["specs"].as_array().unwrap().len(), super::table::SPECS.len());
}

#[tokio::test(flavor = "current_thread")]
async fn a_fader_never_pushes_peaks_past_the_ceiling_nor_past_its_most() {
    // Spikes at full scale over a quiet tone: louder means past the ceiling, so a limiter first. (A second of each mix.)
    let short = |amplitude| Rc::try_unwrap(Fake::new(amplitude)).ok().unwrap();
    let fake = Rc::new(Fake { spikes: 0.95, seconds: 1, ..short(0.03) });
    let tool = DeliverySpec::new(fake.clone()).tool();
    let fixed = run(&tool, json!({"action": "fix", "spec": "spotify"})).await.unwrap();
    assert!(fake.moves.borrow().is_empty(), "{fixed}");
    assert!(fixed["said"][0].as_str().unwrap().contains("limit the master's peaks first (the Fake's limiter)"), "{fixed}");
    // Too loud is brought down whatever its peaks.
    let loud = Rc::new(Fake { seconds: 1, ..short(0.9) });
    let tool = DeliverySpec::new(loud.clone()).tool();
    let fixed = run(&tool, json!({"action": "fix", "spec": "ebu-r128"})).await.unwrap();
    assert!(loud.moves.borrow()[0] < 0., "{fixed}");
    assert_eq!(fixed["ok"], true, "{fixed}");
    // A fader at its most: a gainer.
    let quiet = Rc::new(Fake { seconds: 1, ..short(0.002) });
    let tool = DeliverySpec::new(quiet.clone()).tool();
    let fixed = run(&tool, json!({"action": "fix", "spec": "spotify"})).await.unwrap();
    assert!(fixed["said"][0].as_str().unwrap().contains("a gainer on the master"), "{fixed}");
}

#[tokio::test(flavor = "current_thread")]
async fn an_acx_raise_stops_where_the_noise_floor_would_fail_and_aims_lower_in_the_range() {
    // (RMS about -24 over the whole, room tone at -63: +4 to ACX's -20 would put the floor at -59)
    let fake = Rc::new(Fake { tail: Some(0.000_708), ..Rc::try_unwrap(Fake::new(0.0998)).ok().unwrap() });
    let tool = DeliverySpec::new(fake.clone()).tool();
    let fixed = run(&tool, json!({"action": "fix", "spec": "acx"})).await.unwrap();
    assert_eq!(fixed["ok"], true, "{fixed}");
    assert!(fixed["said"][0].as_str().unwrap().starts_with("aimed lower in the range"), "{fixed}");
    let floor: f64 = line(&fixed["after"], "noise floor")["measured"].as_str().unwrap().trim_end_matches(" dB").parse().unwrap();
    assert!(floor <= -60., "{floor}");
}

#[tokio::test(flavor = "current_thread")]
async fn acx_reads_rms_and_the_noise_floor_and_says_noise_is_the_producers_call() {
    // A tone with a second of loud hiss after it: the noise floor fails ACX's -60.
    let fake = Rc::new(Fake { tail: Some(0.01), ..Rc::try_unwrap(Fake::new(0.1)).ok().unwrap() });
    let tool = DeliverySpec::new(fake.clone()).tool();
    let checked = run(&tool, json!({"spec": "acx"})).await.unwrap();
    assert_eq!(line(&checked, "noise floor")["ok"], false, "{checked}");
    assert!(line(&checked, "RMS")["measured"].as_str().unwrap().ends_with(" dB"));
    assert!(checked["also"].as_str().unwrap().contains("room tone"));
    let fixed = run(&tool, json!({"action": "fix", "spec": "acx"})).await.unwrap();
    assert!(fixed["said"].to_string().contains("taking noise away is the producer's call"), "{fixed}");
}
