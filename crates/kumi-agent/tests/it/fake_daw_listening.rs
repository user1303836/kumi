//! The listening tools over a DAW that isn't Live, which plays what it captures out loud or captures it offline: core
//! asks for listens the producer hears, the DAW says which ones they did, and a round the producer didn't hear is
//! said so. Nothing the tools say over it is in Live's words.
use async_trait::async_trait;
use indexmap::IndexMap;
use kumi_common::abort::Signal;
use kumi_listening::{
    checklist::MainState,
    engine::Listening,
    judging::Placed,
    renderer::{Audible, AuditionOptions, AuditionPass, AuditionRender, AuditionSource, Captured, ListenFlag, Pass, Renderer, Window},
};
use kumi_types::{
    contracts::{AuditionCandidate, JsonObject, Render},
    daw::{DawProfile, DawWords, PromptVocab},
};
use regex::Regex;
use serde_json::{json, Value};
use std::{cell::RefCell, path::PathBuf, rc::Rc, sync::LazyLock};

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
    let listening = Listening::new(fake.clone(), None, None, None, None, None);
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

#[tokio::test(flavor = "current_thread")]
async fn a_judged_run_works_toward_the_producers_own_music_kept_by_name() {
    use kumi_listening::{
        checklist::Profile,
        measure::measure_samples,
        references::store::{KeptReference, KeptTrack, ReferenceStore, KEPT_VERSION},
    };
    tokio::task::LocalSet::new()
        .run_until(async {
            // The learned models off, in a home of the test's own: the run measures, and fetches nothing.
            let home = tempfile::tempdir().unwrap();
            std::fs::write(home.path().join("slots.json"), r#"{"embeddings":{"now":{"use":"off"}}}"#).unwrap();
            std::env::set_var("KUMI_HOME", home.path());
            // Their songs as `taste` keeps them: a reference of kind music, its tracks their files.
            let song = home.path().join("Archangel.wav");
            wav(&song, 4., 0.2);
            let noise: Vec<f32> = (0..48_000 * 4).map(|n| ((n * 7919 % 1000) as f32 / 1000. - 0.5) * 0.4).collect();
            let profile = Profile::of("your music near Archangel", &measure_samples(&noise, &noise, 48_000.));
            let references = Rc::new(ReferenceStore::new(home.path().join("references")));
            let name = "your music near Archangel";
            references
                .save(&KeptReference {
                    version: KEPT_VERSION,
                    key: ReferenceStore::key(name),
                    name: name.into(),
                    kind: "music".into(),
                    tracks: vec![KeptTrack {
                        artist: "Burial".into(),
                        title: "Archangel".into(),
                        source: song.to_string_lossy().into_owned(),
                        mbid: None,
                    }],
                    profile,
                    at: 1,
                    mbid: None,
                    stamp: None,
                })
                .await
                .unwrap();
            let fake = Rc::new(Fake {
                plays: false,
                dir: tempfile::tempdir().unwrap(),
                flag: ListenFlag::default(),
                asked: RefCell::default(),
                told: RefCell::default(),
            });
            let listening = Listening::new(fake.clone(), None, None, None, Some(references));
            let input = json!({"goal": {"reference": name}, "from_beat": 0, "beats": 4});
            let result = listening.call("judge", input.as_object().cloned().unwrap(), Signal::new()).await.expect("judge").unwrap();
            assert!(!result.is_error, "{}", result.text);
            assert!(result.text.contains("as the reference"), "judged against their music's profile: {}", result.text);
        })
        .await;
}
