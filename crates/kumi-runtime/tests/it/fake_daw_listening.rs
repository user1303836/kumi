//! The listening tools over a DAW that isn't Live, which plays what it captures out loud or captures it offline: core
//! asks for listens the producer hears, the DAW says which ones they did, and a round the producer didn't hear is
//! said so. Nothing the tools say over it is in Live's words. (M5) The same over Logic Pro, through Kumi's host in
//! front of the simulated Logic: the mix judged, a round's gain turned on the Stereo Out and taken back, two tracks
//! auditioned and one rendered, in Logic's words.
use async_trait::async_trait;
use indexmap::IndexMap;
use kumi_common::abort::Signal;
use kumi_runtime::core::{
    contracts::{AuditionCandidate, JsonObject, Render},
    daw::{DawProfile, DawWords, PromptVocab},
};
use kumi_runtime::listening::{
    checklist::MainState,
    engine::Listening,
    judging::Placed,
    renderer::{Audible, AuditionOptions, AuditionPass, AuditionRender, AuditionSource, Captured, ListenFlag, Pass, Renderer, Window},
};
use regex::Regex;
use serde_json::{json, Value};
use std::{
    cell::{Cell, RefCell},
    path::PathBuf,
    rc::Rc,
    sync::LazyLock,
};

static LIVE_WORDS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\b(Live|Set|Sets|Ableton|Session|Arrangement|Remote Script|Control Surface|Main|Drum Rack|Operator|[Rr]acks?|[Cc]lip slots?|[Ss]cenes?)\b")
        .unwrap()
});

const FAKE: DawProfile = DawProfile {
    id: None,
    words: DawWords { app: "Fakeloop", place: "song", places: "songs", master: "the master track", timeline: "timeline" },
    caps: DawProfile::UNKNOWN.caps,
    vocab: PromptVocab { own_undo: "Fakeloop's own undo (Ctrl-Z)", structure: "tracks", ..DawProfile::UNKNOWN.vocab },
};

/// Noise at `level` for `seconds`, as a 16-bit stereo WAV at 48 kHz.
fn wav(path: &std::path::Path, seconds: f64, level: f64) {
    let frames = (seconds * 48_000.) as u32;
    let mut data = Vec::with_capacity(frames as usize * 4);
    let mut seed: u32 = 0x1234_5678;
    for _ in 0..frames {
        seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        let sample = (((seed >> 16) as f64 / 32_768. - 1.) * level * 32_767.) as i16;
        data.extend_from_slice(&sample.to_le_bytes());
        data.extend_from_slice(&sample.to_le_bytes());
    }
    let mut file = Vec::with_capacity(44 + data.len());
    file.extend_from_slice(b"RIFF");
    file.extend_from_slice(&(36 + data.len() as u32).to_le_bytes());
    file.extend_from_slice(b"WAVEfmt ");
    file.extend_from_slice(&16u32.to_le_bytes());
    file.extend_from_slice(&1u16.to_le_bytes());
    file.extend_from_slice(&2u16.to_le_bytes());
    file.extend_from_slice(&48_000u32.to_le_bytes());
    file.extend_from_slice(&(48_000u32 * 4).to_le_bytes());
    file.extend_from_slice(&4u16.to_le_bytes());
    file.extend_from_slice(&16u16.to_le_bytes());
    file.extend_from_slice(b"data");
    file.extend_from_slice(&(data.len() as u32).to_le_bytes());
    file.extend_from_slice(&data);
    std::fs::write(path, file).unwrap();
}

/// A DAW that captures what it's asked to: out loud (`plays`), or offline.
struct Fake {
    plays: bool,
    dir: tempfile::TempDir,
    flag: ListenFlag,
    asked: RefCell<Vec<Audible>>,
    told: RefCell<Vec<String>>,
}
#[async_trait(?Send)]
impl Renderer for Fake {
    fn profile(&self) -> &DawProfile {
        &FAKE
    }
    fn refusal(&self) -> Option<String> {
        None
    }
    fn tempo(&self) -> Option<f64> {
        Some(120.)
    }
    fn beats_per_bar(&self) -> f64 {
        4.
    }
    fn lifetime(&self) -> Signal {
        Signal::new()
    }
    fn cleanup(&self) -> Signal {
        Signal::new()
    }
    fn tell(&self, title: &str, _playing: Option<bool>) {
        self.told.borrow_mut().push(title.to_owned());
    }
    fn flag(&self) -> &ListenFlag {
        &self.flag
    }
    fn kept_folder(&self) -> PathBuf {
        self.dir.path().join("kept")
    }
    async fn hear_pass(&self, _candidates: &[AuditionCandidate], window: Window, _before_main: bool, _signal: Signal) -> Pass {
        let file = self.dir.path().join("mix.wav");
        wav(&file, window.beats * 0.5, 0.3);
        let render = Render { file: file.to_string_lossy().into_owned(), start: 0., seconds: Some(window.beats * 0.5) };
        let rendered =
            Captured { files: IndexMap::from([("mix".to_owned(), render)]), sources: vec![("mix".into(), true)], before_master: None };
        Pass { rendered: Ok(rendered), notes: vec![], tried: None, lead: 0., audible: self.plays }
    }
    async fn master_chain(&self, _signal: Signal) -> MainState {
        MainState::default()
    }
    fn applied(&self) -> Vec<String> {
        vec![]
    }
    fn applied_since(&self, _mark: &[String]) -> Vec<(String, String)> {
        vec![]
    }
    async fn undo(&self, _id: &str) -> Result<(), String> {
        Ok(())
    }
    async fn turn_gain(&self, _gain: f64, _track: Option<&str>, _signal: Signal) -> Result<(String, bool), String> {
        Err("Fakeloop has no gain device on the master track.".into())
    }
    async fn limited(&self, _track: Option<&str>, _signal: Signal) -> bool {
        false
    }
    async fn chain(&self, _track: Option<&str>) -> Result<Vec<Placed>, String> {
        Ok(vec![])
    }
    async fn delete_device(&self, _reference: &str) -> bool {
        false
    }
    async fn audition_pass(
        &self,
        candidates: &[AuditionCandidate],
        _from_beat: Option<f64>,
        beats: Option<f64>,
        options: AuditionOptions,
        _signal: Signal,
    ) -> AuditionPass {
        self.asked.borrow_mut().push(options.audible);
        let beats = beats.unwrap_or(8.);
        let mut files = IndexMap::new();
        let mut sources = vec![];
        for (index, candidate) in candidates.iter().enumerate() {
            let file = self.dir.path().join(format!("candidate-{index}.wav"));
            wav(&file, beats * 0.5, 0.1 + 0.2 * index as f64);
            files.insert(
                candidate.track.clone(),
                Render { file: file.to_string_lossy().into_owned(), start: 0., seconds: Some(beats * 0.5) },
            );
            let heard = self.plays && (options.audible == Audible::Every || index == 0);
            sources.push(AuditionSource { track: candidate.track.clone(), name: candidate.track.clone(), scene: None, audible: heard });
        }
        AuditionPass { rendered: Ok(AuditionRender { sources, files, beats }), notes: vec![], opened: true }
    }
}

async fn session(plays: bool) -> (Vec<String>, Rc<Fake>) {
    let fake = Rc::new(Fake {
        plays,
        dir: tempfile::tempdir().unwrap(),
        flag: ListenFlag::default(),
        asked: RefCell::default(),
        told: RefCell::default(),
    });
    let listening = Listening::new(fake.clone(), None, None, None, None);
    let object = |value: Value| value.as_object().cloned().unwrap();
    let mut said = vec![];
    let calls: Vec<(&str, JsonObject)> = vec![
        (
            "audition",
            object(json!({"candidates":[{"track":"Pad A","label":"warm"},{"track":"Pad B","label":"bright"}],"from_beat":0,"beats":4})),
        ),
        ("judge", object(json!({"goal":{"loudness":-14},"from_beat":0,"beats":4}))),
        ("render", object(json!({"track":"Pad A","from_beat":0,"beats":4}))),
    ];
    for (name, input) in calls {
        let result = listening.call(name, input, Signal::new()).await.expect("a listening tool").unwrap();
        said.push(result.text.clone());
        said.extend(result.reply.clone());
    }
    said.extend(fake.told.borrow().iter().cloned());
    (said, fake)
}

#[tokio::test(flavor = "current_thread")]
async fn a_daw_that_plays_out_loud_or_offline_is_asked_the_same_and_said_in_its_own_words() {
    tokio::task::LocalSet::new()
        .run_until(async {
            for plays in [true, false] {
                let (said, fake) = session(plays).await;
                assert_eq!(*fake.asked.borrow(), [Audible::First], "core asks for the first candidate out loud");
                let audition = &said[0];
                assert!(audition.contains("warm") && audition.contains("bright"), "{audition}");
                let unheard = "The producer didn't hear this round: Fakeloop captured it without playing it out loud.";
                assert_eq!(audition.contains(unheard), !plays, "{audition}");
                let found: Vec<&String> = said.iter().filter(|text| LIVE_WORDS.is_match(text)).collect();
                assert!(found.is_empty(), "plays {plays}: {found:#?}");
            }
        })
        .await;
}

kumi_testkit::in_process!();

/// Each change Kumi made in Logic as HISTORY tells it (the last word on each id), and what the app was told Kumi did.
#[derive(Default)]
struct Told {
    changes: RefCell<Vec<kumi_runtime::core::contracts::ChangeRecord>>,
    actions: RefCell<Vec<String>>,
}

impl Told {
    /// The latest record of each change whose title starts with `title`, oldest first.
    fn rows(&self, title: &str) -> Vec<kumi_runtime::core::contracts::ChangeRecord> {
        let mut latest: IndexMap<String, kumi_runtime::core::contracts::ChangeRecord> = IndexMap::new();
        for record in self.changes.borrow().iter() {
            latest.insert(record.id.clone(), record.clone());
        }
        latest.into_values().filter(|record| record.title.starts_with(title)).collect()
    }
}

/// (M5) Logic Pro through Kumi's host, in front of the simulated Logic (each track a tone of its own, the Stereo Out's
/// plug-ins heard): the mix judged toward a loudness; a change judged in a round that brings the level back with a Gain
/// added at the end of the Stereo Out and set, each a change in HISTORY, then taken back through Kumi's undo; two
/// tracks auditioned, the first heard by the producer and the second silently; a track rendered offline, soloed. Each
/// listen's end tells the producer the project now shows as edited (the metronome that timed it is a project setting).
/// What the tools and HISTORY say is in Logic's words, never Live's.
#[tokio::test(flavor = "current_thread")]
async fn logic_pro_is_judged_auditioned_and_rendered_over_the_simulated_logic_in_its_own_words() {
    use daw_bridge::{files::ensure_secret, paths::BridgePaths, Daw};
    use daw_mcp_server::{host::Host, simulator::logic::LogicScript, simulator::Simulator};
    use futures::FutureExt;
    use kumi_runtime::core::{context::IntegrationContext, contracts::ChangeState};
    use kumi_runtime::integrations::{
        logic::create_logic_integration,
        own_bridge::{Connect, OwnOptions},
    };
    use kumi_runtime::mcp::client::McpEndpoint;
    use std::sync::Arc;
    tokio::task::LocalSet::new()
        .run_until(async {
            let home = tempfile::tempdir().unwrap();
            let paths = BridgePaths::new(home.path(), Daw::Logic);
            let secret = ensure_secret(&paths.secret()).unwrap();
            let script = LogicScript::new();
            let mut config = script.config(secret, "com.apple.logic10");
            config.discovery_dir = Some(paths.discovery());
            let simulator = Simulator::start(config).await.unwrap();
            let endpoint = Rc::new(in_process::InProcess { host: Arc::new(Host::new(Daw::Logic, home.path())), id: Cell::new(0) });
            let connect: Connect = Rc::new(move |_| {
                let endpoint = endpoint.clone();
                async move { Ok(endpoint as Rc<dyn McpEndpoint>) }.boxed_local()
            });
            let told = Rc::new(Told::default());
            let mut context = IntegrationContext::new(Rc::new(|_, _| {}));
            let into = told.clone();
            context.on_change = Some(Rc::new(move |record| into.changes.borrow_mut().push(record)));
            let into = told.clone();
            context.on_action = Some(Rc::new(move |event| into.actions.borrow_mut().push(event.title)));
            let logic =
                create_logic_integration(OwnOptions { context, kumi_home: home.path().into(), connect: Some(connect), open_file: None });
            logic.start(Signal::new()).await.unwrap();
            let observed = logic.observe(Signal::new(), None).await.unwrap();
            let tool = |name: &str| observed.tools.iter().find(|tool| tool.name() == name).cloned().unwrap_or_else(|| panic!("no {name}"));
            let object = |value: Value| value.as_object().cloned().unwrap();
            let mut said: Vec<String> = ["audition", "judge", "tune", "groove", "form", "sound", "render"]
                .iter()
                .map(|name| {
                    let offered = tool(name);
                    format!("{} {}", offered.description(), Value::Object(offered.input_schema()))
                })
                .collect();

            // The mix judged toward -14 LUFS: two tones at -20 dBFS read about 3.7 LU short.
            let judged = tool("judge")
                .execute(object(json!({"goal": {"loudness": -14, "problems": false}, "from_beat": 0, "beats": 16})), Signal::new())
                .await
                .unwrap();
            assert!(!judged.is_error, "{}", judged.text);
            said.push(judged.text.clone());

            // Keys up 3 dB, judged: the round brings the level back to the goal with a Gain at the end of the Stereo
            // Out, set through its Controls View; each a change in HISTORY.
            let raised = tool("track_volume").execute(object(json!({"ref": "track:2", "volumeDb": 3})), Signal::new()).await.unwrap();
            assert!(!raised.is_error, "{}", raised.text);
            let round = tool("judge").execute(object(json!({"change": "Keys up 3 dB"})), Signal::new()).await.unwrap();
            assert!(!round.is_error, "{}", round.text);
            said.push(round.text.clone());
            assert!(round.text.contains("the Gain on the Stereo Out"), "the round says what it turned: {}", round.text);
            assert_eq!(script.plugins("output:1"), [("Gain".to_string(), false)], "a Gain at the end of the Stereo Out");
            let gain = script.param("output:1", 1, "Gain").map(|(_, text)| text).unwrap();
            assert!(gain.starts_with('+'), "turned up toward -14 LUFS: {gain}");
            let added = told.rows("Added “Gain” to “Stereo Out”");
            let set = told.rows("“Gain”'s Gain → ");
            assert!(added.len() == 1 && set.len() == 1 && set[0].title.ends_with("on “Stereo Out”"), "{:?}", told.changes.borrow());

            // Taken back through Kumi's undo, newest first: the gain, the Gain, the fader.
            let mut made: Vec<_> = told.rows("").into_iter().filter(|record| record.state == ChangeState::Applied).collect();
            made.reverse();
            for record in &made {
                let undone = tool("undo_change").execute(object(json!({"change": record.id})), Signal::new()).await.unwrap();
                assert!(!undone.is_error, "{}: {}", record.title, undone.text);
            }
            assert!(script.plugins("output:1").is_empty() && script.volume(2) == 0.0, "the project as it was");

            // Vocal and Keys auditioned: the producer hears the first, Logic plays the second silently.
            let audition = tool("audition")
                .execute(
                    object(json!({"candidates": [{"track": "track:1", "label": "the vocal"}, {"track": "Keys", "label": "the keys"}], "from_beat": 0, "beats": 16})),
                    Signal::new(),
                )
                .await
                .unwrap();
            assert!(!audition.is_error, "{}", audition.text);
            said.push(audition.text.clone());
            let passes: Vec<(bool, Value)> = script
                .asked()
                .into_iter()
                .filter(|(method, _)| method == "hear")
                .map(|(_, params)| (params["mute"] == true, params["solo"].clone()))
                .collect();
            assert_eq!(passes[passes.len() - 2..], [(false, json!(["1:track:1"])), (true, json!(["1:track:2"]))], "{passes:?}");
            assert!(told.rows("Auditioned").first().is_some_and(|row| row.state == ChangeState::Heard), "the audition heard");

            // Keys rendered offline, soloed through its plug-ins and the Stereo Out's.
            let render = tool("render").execute(object(json!({"track": "track:2", "from_beat": 0, "beats": 16})), Signal::new()).await.unwrap();
            assert!(!render.is_error, "{}", render.text);
            let reply: Value = serde_json::from_str(&render.text).unwrap();
            assert_eq!((reply["seconds"].as_f64(), reply["sampleRate"].as_u64()), (Some(6.0), Some(44_100)), "{reply}");
            assert!(reply["note"].as_str().is_some_and(|note| note.contains("Stereo Out")), "{reply}");
            assert!(std::path::Path::new(reply["file"].as_str().unwrap()).is_file());
            said.push(render.text);

            // Each listen's end says the project now shows as edited in Logic (its metronome, a project setting, timed it).
            let edited = "(the project now shows as edited in Logic Pro: Kumi turned its metronome, a project setting, on and off to time the listen)";
            for end in ["Listened", "Auditioned"] {
                assert!(
                    told.actions.borrow().iter().any(|title| title.starts_with(end) && title.ends_with(edited)),
                    "{end}: {:?}",
                    told.actions.borrow()
                );
            }
            said.extend(told.actions.borrow().iter().cloned());
            said.extend(told.changes.borrow().iter().map(|record| format!("{} {}", record.title, record.note.clone().unwrap_or_default())));
            let found: Vec<&String> = said.iter().filter(|text| LIVE_WORDS.is_match(text)).collect();
            assert!(found.is_empty(), "{found:#?}");
            logic.close().await.unwrap();
            assert!(!std::path::Path::new(reply["file"].as_str().unwrap()).exists(), "the listening folder goes with the integration");
            simulator.stop();
        })
        .await;
}
