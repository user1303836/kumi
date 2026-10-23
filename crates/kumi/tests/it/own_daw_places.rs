//! REAPER and Renoise sessions end to end (Kumi's real session and agent kernel on a scripted model, Kumi's host in
//! process in front of a simulated DAW), for what M6 makes work in them as it works in Live: the fixed phrases ("tempo
//! 124", "mute drums") as the DAW's changes with HISTORY and Kumi's undo and no model call; notes kept per producer and
//! per place; conversations per place, every unsaved project sharing its DAW's unsaved place until its first save; and
//! nothing said in Live's words.
use async_trait::async_trait;
use daw_bridge::{files::ensure_secret, paths::BridgePaths, Daw};
use daw_mcp_server::{
    host::Host,
    reaper::simulator::ReaperScript,
    renoise::simulator::RenoiseScript,
    simulator::{Simulator, SimulatorConfig},
};
use futures::stream::{self, StreamExt};
use futures::FutureExt;
use kumi::daw::registry::PLACES;
use kumi_daw_reaper::create_reaper_integration;
use kumi_daw_renoise::create_renoise_integration;
use kumi_runtime::ai::error::LanguageModelError;
use kumi_runtime::ai::types::{
    CallOptions, FinishReason, FinishReasonUnified, InputTokens, OutputTokens, StreamPart, StreamParts, ToolCall, Usage as ModelUsage,
};
use kumi_runtime::core::context::IntegrationContext;
use kumi_runtime::core::contracts::*;
use kumi_runtime::core::conversations::create_conversation_store_in;
use kumi_runtime::core::jobs::{create_job_store, JobState};
use kumi_runtime::core::memory::{create_memory_store_in, MemoryStoreOptions};
use kumi_runtime::core::session::JobsOptions;
use kumi_runtime::integrations::own_bridge::{Connect, OwnOptions};
use kumi_runtime::kernel::agent::{create_agent_kernel, AgentKernel, AgentKernelOptions, LanguageModel, ModelBinding, ModelRequest};
use kumi_runtime::{create_session, Session, SessionOptions};
use regex::Regex;
use serde_json::{json, Value};
use std::{
    cell::{Cell, RefCell},
    collections::VecDeque,
    path::Path,
    rc::Rc,
    sync::{Arc, LazyLock},
};

kumi_testkit::in_process!();

/// Live's words, whole: none may reach the producer or the model over another DAW.
static LIVE_WORDS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\b(Live|Set|Sets|Ableton|Session|Arrangement|Remote Script|Control Surface|Main|Drum Rack|Operator|[Rr]acks?|[Cc]lip slots?|[Ss]cenes?)\b")
        .unwrap()
});

const SONG_A: &str = "0123456789abcdef0123456789abcdef";
const SONG_B: &str = "fedcba9876543210fedcba9876543210";

fn finish(unified: FinishReasonUnified) -> StreamPart {
    let raw = serde_json::to_value(unified).unwrap().as_str().unwrap().to_string();
    let usage = ModelUsage {
        input_tokens: InputTokens { total: Some(3.0), no_cache: Some(3.0), cache_read: Some(0.0), cache_write: Some(0.0) },
        output_tokens: OutputTokens { total: Some(2.0), text: Some(2.0), reasoning: Some(0.0) },
        raw: None,
    };
    StreamPart::Finish { usage, finish_reason: FinishReason { unified, raw: Some(raw) }, provider_metadata: None }
}
fn answer(words: &str) -> Vec<StreamPart> {
    vec![
        StreamPart::TextStart { id: "t".into(), provider_metadata: None },
        StreamPart::TextDelta { id: "t".into(), delta: words.into(), provider_metadata: None },
        StreamPart::TextEnd { id: "t".into(), provider_metadata: None },
        finish(FinishReasonUnified::Stop),
    ]
}
fn call(name: &str, input: Value) -> Vec<StreamPart> {
    vec![
        StreamPart::ToolCall(ToolCall {
            tool_call_id: format!("call_{name}"),
            tool_name: name.into(),
            input: input.to_string(),
            provider_executed: None,
            dynamic: None,
            provider_metadata: None,
        }),
        finish(FinishReasonUnified::ToolCalls),
    ]
}

/// What the model says next: each step in turn, then "Done."
type Step = Box<dyn Fn() -> Vec<StreamPart>>;
#[derive(Default)]
struct Recorded {
    /// Each request: its system text, messages and tools, as JSON text.
    requests: RefCell<Vec<String>>,
    script: RefCell<VecDeque<Step>>,
}
struct Scripted {
    recorded: Rc<Recorded>,
}
#[async_trait(?Send)]
impl LanguageModel for Scripted {
    async fn do_stream(&self, _: CallOptions) -> Result<StreamParts, LanguageModelError> {
        let step = self.recorded.script.borrow_mut().pop_front();
        let parts = step.map(|step| step()).unwrap_or_else(|| answer("Done."));
        Ok(stream::iter(parts).boxed_local())
    }
}
fn step(make: impl Fn() -> Vec<StreamPart> + 'static) -> Step {
    Box::new(make)
}

/// Kumi's agent kernel on the scripted model, recording each request.
fn recording(into: Rc<Recorded>) -> KernelFactory {
    Rc::new(move |options: KernelOptions| {
        let into = into.clone();
        async move {
            let requests = into.clone();
            let binding = ModelBinding {
                id: "test/fixture".into(),
                model: Rc::new(Scripted { recorded: into.clone() }),
                prepare: Box::new(move |request: ModelRequest| {
                    requests
                        .requests
                        .borrow_mut()
                        .push(json!({"system":request.instructions,"messages":request.messages,"tools":request.tools}).to_string());
                    CallOptions {
                        prompt: request.messages,
                        tools: (!request.tools.is_empty()).then_some(request.tools),
                        ..CallOptions::default()
                    }
                }),
                budget: None,
            };
            let kernel: Rc<AgentKernel> = Rc::new(create_agent_kernel(AgentKernelOptions {
                conversation: options.conversation,
                binding,
                instructions: options.instructions,
                tools: options.tools,
                signal: options.signal,
                checkpoint: options.checkpoint,
                max_steps: None,
                budget: None,
            })?);
            Ok(kernel as Rc<dyn Kernel>)
        }
        .boxed_local()
    })
}

/// A session in a DAW of Kumi's own, as the app makes one: every store keeps its places for every DAW Kumi integrates.
struct Own {
    session: Session,
    main: Rc<Recorded>,
    /// The background workers' model.
    worker: Rc<Recorded>,
    events: Rc<RefCell<Vec<SessionEvent>>>,
    /// HISTORY's records, as the integration told them.
    changes: Rc<RefCell<Vec<ChangeRecord>>>,
    simulator: Simulator,
    controller: Rc<RefCell<Option<Session>>>,
}

impl Own {
    async fn open(
        dir: &Path,
        daw: Daw,
        profile: DawProfile,
        config: impl FnOnce(daw_bridge::auth::Secret) -> SimulatorConfig,
        create: fn(OwnOptions) -> Rc<dyn Integration>,
    ) -> Own {
        Own::open_with(dir, daw, profile, config, create, |_| {}).await
    }

    /// `open`, with more of the session's options set.
    async fn open_with(
        dir: &Path,
        daw: Daw,
        profile: DawProfile,
        config: impl FnOnce(daw_bridge::auth::Secret) -> SimulatorConfig,
        create: fn(OwnOptions) -> Rc<dyn Integration>,
        configure: impl FnOnce(&mut SessionOptions),
    ) -> Own {
        let paths = BridgePaths::new(dir, daw);
        let mut config = config(ensure_secret(&paths.secret()).unwrap());
        config.discovery_dir = Some(paths.discovery());
        let simulator = Simulator::start(config).await.unwrap();
        let host = Rc::new(in_process::InProcess { host: Arc::new(Host::new(daw, dir)), id: Cell::new(0) });
        let home = dir.to_path_buf();
        let changes: Rc<RefCell<Vec<ChangeRecord>>> = Rc::default();
        let into = changes.clone();
        // As the app does: each change the integration tells goes to the session too, which keeps it with the conversation.
        let controller: Rc<RefCell<Option<Session>>> = Rc::default();
        let told = controller.clone();
        let integration: IntegrationFactory = Box::new(move |listener: ConnectionListener| {
            let host = host.clone();
            let connect: Connect = Rc::new(move |_| {
                let host = host.clone();
                async move { Ok(host as Rc<dyn kumi_runtime::mcp::client::McpEndpoint>) }.boxed_local()
            });
            let mut context = IntegrationContext::new(listener);
            let (into, told) = (into.clone(), told.clone());
            context.on_change = Some(Rc::new(move |record: ChangeRecord| {
                into.borrow_mut().push(record.clone());
                if let Some(session) = told.borrow().clone() {
                    session.watch(WatchEvent::Change(record));
                }
            }));
            create(OwnOptions { context, kumi_home: home.clone(), connect: Some(connect) })
        });
        let (main, worker) = (Rc::new(Recorded::default()), Rc::new(Recorded::default()));
        let events = Rc::new(RefCell::new(vec![]));
        let out = events.clone();
        let mut options = SessionOptions::new(recording(main.clone()), integration, Rc::new(move |event| out.borrow_mut().push(event)));
        options.profile = profile;
        options.timeout_ms = Some(20_000);
        options.close_timeout_ms = Some(5_000);
        let projects = dir.join("projects");
        options.conversations = Some(create_conversation_store_in(&projects, None, PLACES));
        options.memory = Some(create_memory_store_in(
            MemoryStoreOptions { projects_dir: projects.clone(), producer_file: dir.join("memory.json") },
            PLACES,
        ));
        // Jobs kept per place between sessions, beside the goals, as the app keeps them.
        options.jobs =
            Some(JobsOptions { worker_factory: recording(worker.clone()), max: 3, store: Some(create_job_store(dir.join("goals"))) });
        configure(&mut options);
        let session = create_session(options).unwrap();
        *controller.borrow_mut() = Some(session.clone());
        session.start().await.unwrap();
        Own { session, main, worker, events, changes, simulator, controller }
    }

    /// A fixed phrase, run at once: what Kumi said, and whether it's an error. None: the model takes it.
    async fn say(&self, phrase: &str) -> Option<(String, bool)> {
        self.session.instant(phrase).await.unwrap().map(|outcome| (outcome.text, outcome.is_error))
    }

    /// The titles of HISTORY's records, in order (a record told again when its state changed, once).
    fn titles(&self) -> Vec<String> {
        let mut titles: Vec<String> = Vec::new();
        for record in self.changes.borrow().iter() {
            if !titles.contains(&record.title) {
                titles.push(record.title.clone());
            }
        }
        titles
    }

    /// Everything the model was sent and the producer told.
    fn said(&self) -> Vec<String> {
        let mut said = self.main.requests.borrow().clone();
        said.extend(self.worker.requests.borrow().iter().cloned());
        said.extend(self.events.borrow().iter().map(|event| serde_json::to_string(event).unwrap()));
        said.extend(self.changes.borrow().iter().map(|record| serde_json::to_string(record).unwrap()));
        said
    }

    async fn close(self) -> Vec<String> {
        self.controller.borrow_mut().take();
        self.session.close().await.unwrap();
        self.simulator.stop();
        self.said()
    }
}

fn assert_no_live_words(said: &[String]) {
    let found: Vec<String> = said
        .iter()
        .flat_map(|text| {
            LIVE_WORDS
                .find_iter(text)
                .map(|word| {
                    let (start, end) = (word.start().saturating_sub(60), (word.end() + 60).min(text.len()));
                    let (start, end) = (floor(text, start), ceil(text, end));
                    format!("{:?}: …{}…", word.as_str(), &text[start..end])
                })
                .collect::<Vec<_>>()
        })
        .collect();
    assert!(found.is_empty(), "{} in Live's words:\n{}", found.len(), found.join("\n"));
}
fn floor(text: &str, mut at: usize) -> usize {
    while !text.is_char_boundary(at) {
        at -= 1;
    }
    at
}
fn ceil(text: &str, mut at: usize) -> usize {
    while !text.is_char_boundary(at) {
        at += 1;
    }
    at
}

fn reaper(script: &ReaperScript) -> impl FnOnce(daw_bridge::auth::Secret) -> SimulatorConfig {
    let script = script.clone();
    move |secret| script.config(secret, "/lab/REAPER")
}

fn renoise(script: &RenoiseScript) -> impl FnOnce(daw_bridge::auth::Secret) -> SimulatorConfig {
    let script = script.clone();
    move |secret| script.config_with_changes(secret, "/lab/Renoise")
}

/// REAPER's fixed phrases are its own changes: a track's mute, solo and arm, the tempo, the loop (its range and repeat) and
/// the metronome, each in HISTORY with Kumi's undo, and none of them a model call. What REAPER can't take as asked goes to
/// the model; what it can't do at all is said in its words.
#[tokio::test(flavor = "current_thread")]
async fn reapers_fixed_phrases_are_its_changes_in_history_with_kumis_undo_and_no_model_call() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let dir = tempfile::tempdir().unwrap();
            let script = ReaperScript::new(3, 0);
            script.set_project(Some(SONG_A), "/lab/Night.rpp", "sim:night", "Night.rpp");
            let own = Own::open(dir.path(), Daw::Reaper, DawProfile::REAPER, reaper(&script), create_reaper_integration).await;

            let (said, error) = own.say("mute track 2").await.expect("a fixed phrase");
            assert!(!error, "{said}");
            assert_eq!(script.track(2)["mute"], true, "{said}");
            let (said, error) = own.say("solo Track 1").await.unwrap();
            assert!(!error && script.track(1)["solo"] == true, "{said}");
            let (said, error) = own.say("arm track 3").await.unwrap();
            assert!(!error && script.track(3)["arm"] == true, "{said}");
            let (said, error) = own.say("tempo 124").await.unwrap();
            assert!(!error && script.settings()["bpm"] == 124.0, "{said}");
            // Bars 9 to 16 in 4/4: quarter notes 32 to 64, and repeat on.
            let (said, error) = own.say("loop 9-16").await.unwrap();
            assert!(!error, "{said}");
            assert_eq!(script.settings()["loop"], json!({"start": 32.0, "end": 64.0}));
            assert_eq!(script.settings()["repeat"], true);
            let (said, error) = own.say("metronome on").await.unwrap();
            assert!(!error && script.settings()["metronome"] == true, "{said}");
            assert_eq!(
                own.titles(),
                [
                    "Track 2 muted",
                    "Track 1 soloed",
                    "Track 3 armed",
                    "Tempo 124 BPM",
                    "Loop 32.0 to 64.0 beats",
                    "Repeat on",
                    "Metronome on"
                ]
            );
            // The producer's /undo takes them back, newest first, as REAPER's own undo steps or Kumi's.
            for _ in 0..3 {
                own.session.undo(None).await.unwrap().expect("undone");
            }
            assert_eq!(script.settings()["metronome"], false);
            assert_eq!(script.settings()["repeat"], false);
            assert_eq!(script.settings()["loop"], json!({"start": 0.0, "end": 0.0}));
            let (said, error) = own.say("loop off").await.unwrap();
            assert!(!error, "already off: {said}");
            own.session.undo(None).await.unwrap();
            assert_eq!(script.settings()["bpm"], 120.0);
            // REAPER's tempo goes up to 960: said at once, nothing changed.
            let (said, error) = own.say("tempo 980").await.unwrap();
            assert!(error && said.contains("960"), "{said}");
            // A track it doesn't have, or two that fit: the model's.
            assert_eq!(own.say("mute guitar").await, None);
            assert_eq!(own.say("mute track").await, None);
            assert!(own.main.requests.borrow().is_empty(), "no model call");
            let said = own.close().await;
            assert_no_live_words(&said);
        })
        .await;
}

/// Renoise's fixed phrases: the tempo, a track's mute and solo by its name, the loop over whole slots of the sequence
/// (bars 3 and 4 are its second pattern of 32 lines at 4 lines a beat) and the metronome; Renoise arms no tracks, and says
/// so; bars that start inside a pattern go to the model.
#[tokio::test(flavor = "current_thread")]
async fn renoises_fixed_phrases_are_its_changes_and_a_loop_is_whole_slots_of_the_sequence() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let dir = tempfile::tempdir().unwrap();
            let script = RenoiseScript::new();
            let own = Own::open(dir.path(), Daw::Renoise, DawProfile::RENOISE, renoise(&script), create_renoise_integration).await;

            let (said, error) = own.say("mute kick").await.expect("a fixed phrase");
            assert!(!error && script.track_state(1).1, "{said}");
            let (said, error) = own.say("solo bass").await.unwrap();
            assert!(!error && script.solo(4), "{said}");
            let (said, error) = own.say("tempo 140").await.unwrap();
            assert!(!error && script.song()["bpm"].as_f64() == Some(140.0), "{said}");
            let (said, error) = own.say("loop 3-4").await.unwrap();
            assert!(!error, "{said}");
            assert_eq!(script.song()["loop"], json!({"first": 2, "last": 2}));
            let (said, error) = own.say("loop 1-8").await.unwrap();
            assert!(!error && script.song()["loop"] == json!({"first": 1, "last": 4}), "{said}");
            let (said, error) = own.say("metronome on").await.unwrap();
            assert!(!error && script.song()["metronome"] == true, "{said}");
            let (said, error) = own.say("loop off").await.unwrap();
            assert!(!error, "{said}");
            assert_eq!(script.song()["loop"], json!({"first": 0, "last": 0}));
            let undone = own.session.undo(None).await.unwrap().expect("undone");
            assert_eq!(undone.title, "Loop cleared");
            // Renoise has no track arm.
            let (said, error) = own.say("arm kick").await.unwrap();
            assert!(error && said.starts_with("Renoise doesn't arm tracks"), "{said}");
            assert_eq!(own.say("tempo 25").await.map(|(_, error)| error), Some(true), "under Renoise's 32 BPM");
            // Bars 2 to 3 start and end inside patterns: the model works it out.
            assert_eq!(own.say("loop 2-3").await, None);
            // The one name it starts: the Drums group.
            assert_eq!(own.say("mute drum").await.map(|(_, error)| error), Some(false));
            assert!(script.track_state(3).1);
            assert!(own.main.requests.borrow().is_empty(), "no model call");
            let said = own.close().await;
            assert_no_live_words(&said);
        })
        .await;
}

/// The conversations kept in a place: their ids.
async fn kept_in(dir: &Path, place: &str) -> Vec<String> {
    let store = create_conversation_store_in(dir.join("projects"), None, PLACES);
    store.list(place).await.unwrap().into_iter().map(|row| row.id).collect()
}

/// The conversations kept in a place once the session has saved `count` there (it saves after the answer).
async fn kept_once(dir: &Path, place: &str, count: usize) -> Vec<String> {
    for _ in 0..300 {
        let kept = kept_in(dir, place).await;
        if kept.len() == count {
            return kept;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    kept_in(dir, place).await
}

/// The system text of the model's latest request.
fn latest_system(own: &Own) -> String {
    let requests = own.main.requests.borrow();
    let latest: Value = serde_json::from_str(requests.last().expect("a request")).unwrap();
    latest["system"].as_str().unwrap_or_default().to_string()
}

/// Every unsaved REAPER project keeps its conversations in `reaper-unsaved`, as Live's unsaved Sets do theirs (each one
/// starts its own there); once the open one is saved (the same project in the same tab, now with a file and an id), its
/// conversation goes on, moved into the project's own place.
#[tokio::test(flavor = "current_thread")]
async fn unsaved_reaper_projects_share_one_place_and_a_first_save_carries_the_conversation_into_its_own() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let dir = tempfile::tempdir().unwrap();
            let script = ReaperScript::new(2, 0);
            script.set_project(None, "", "sim:u1", "untitled");
            let own = Own::open(dir.path(), Daw::Reaper, DawProfile::REAPER, reaper(&script), create_reaper_integration).await;
            own.session.submit("Make a beat.", None).await.unwrap();
            let first = kept_once(dir.path(), "reaper-unsaved", 1).await;
            assert_eq!(first.len(), 1, "the unsaved project's conversation");
            // Another unsaved project: its own conversation, in the same place.
            script.set_project(None, "", "sim:u2", "untitled");
            own.session.submit("And this one?", None).await.unwrap();
            let both = kept_once(dir.path(), "reaper-unsaved", 2).await;
            assert_eq!(both.len(), 2, "one place for every unsaved project");
            let second: Vec<String> = both.into_iter().filter(|id| !first.contains(id)).collect();
            // Saved: the same project in the same tab. Its conversation goes on, in the project's place.
            script.set_project(Some(SONG_A), "/lab/Beat.rpp", "sim:u2", "Beat.rpp");
            let notices = || {
                own.events
                    .borrow()
                    .iter()
                    .filter_map(|event| match event {
                        SessionEvent::Notice { message } => Some(message.clone()),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
            };
            let before = notices().len();
            own.session.submit("Saved it.", None).await.unwrap();
            assert!(!notices()[before..].iter().any(|notice| notice.contains("changed")), "no fresh start: {:?}", &notices()[before..]);
            assert!(own.main.requests.borrow().last().unwrap().contains("And this one?"), "the same conversation goes on");
            let place = format!("reaper-{SONG_A}");
            assert_eq!(kept_once(dir.path(), &place, 1).await, second, "the conversation went with the project");
            assert_eq!(kept_once(dir.path(), "reaper-unsaved", 1).await, first, "the other unsaved project's stays");
            let said = own.close().await;
            assert_no_live_words(&said);
        })
        .await;
}

/// Notes about the producer go with them into every REAPER project; a note about one project stays with that project,
/// and comes back with it.
#[tokio::test(flavor = "current_thread")]
async fn reaper_notes_are_kept_per_producer_and_per_project_and_come_back_with_their_project() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let dir = tempfile::tempdir().unwrap();
            let script = ReaperScript::new(2, 0);
            script.set_project(Some(SONG_A), "/lab/A.rpp", "sim:a", "A.rpp");
            let own = Own::open(dir.path(), Daw::Reaper, DawProfile::REAPER, reaper(&script), create_reaper_integration).await;
            own.main.script.borrow_mut().extend([
                step(|| call("remember", json!({"note": "The kick stays dry in this mix", "about": "set"}))),
                step(|| call("remember", json!({"note": "Likes tape delay on vocals", "about": "producer"}))),
                step(|| answer("Noted.")),
            ]);
            own.session.submit("Remember these.", None).await.unwrap();
            for _ in 0..300 {
                if dir.path().join(format!("projects/reaper-{SONG_A}/memory.json")).is_file() {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
            assert!(dir.path().join(format!("projects/reaper-{SONG_A}/memory.json")).is_file(), "the project's notes, in its place");
            // Another project: the producer's note, not the other project's.
            script.set_project(Some(SONG_B), "/lab/B.rpp", "sim:b", "B.rpp");
            own.session.submit("Hello.", None).await.unwrap();
            let system = latest_system(&own);
            assert!(system.contains("Likes tape delay on vocals"), "{system}");
            assert!(!system.contains("The kick stays dry"), "{system}");
            // Back to the first: its note comes back.
            script.set_project(Some(SONG_A), "/lab/A.rpp", "sim:a2", "A.rpp");
            own.session.submit("Back again.", None).await.unwrap();
            let system = latest_system(&own);
            assert!(system.contains("The kick stays dry in this mix") && system.contains("Likes tape delay"), "{system}");
            // Each project kept its own conversation.
            assert_eq!(kept_once(dir.path(), &format!("reaper-{SONG_A}"), 1).await.len(), 1);
            assert_eq!(kept_once(dir.path(), &format!("reaper-{SONG_B}"), 1).await.len(), 1);
            let said = own.close().await;
            assert_no_live_words(&said);
        })
        .await;
}

/// A Renoise conversation exported as the producer would share it speaks Renoise's words: what Kumi changed in Renoise,
/// in the song.
#[tokio::test(flavor = "current_thread")]
async fn a_renoise_conversation_exports_in_renoises_words() {
    use kumi_runtime::core::export::{html, markdown, Options, Source};
    tokio::task::LocalSet::new()
        .run_until(async {
            let dir = tempfile::tempdir().unwrap();
            let script = RenoiseScript::new();
            let own = Own::open(dir.path(), Daw::Renoise, DawProfile::RENOISE, renoise(&script), create_renoise_integration).await;
            own.main.script.borrow_mut().extend([
                step(|| call("track_volume", json!({"ref": "track:1", "volumeDb": -6}))),
                step(|| answer("The kick is 6 dB down.")),
            ]);
            own.session.submit("Turn the kick down.", None).await.unwrap();
            own.say("metronome on").await.unwrap();
            let found = own.session.conversation_to_export().await.expect("a conversation to export");
            assert_eq!(found.words, ("Renoise", "song"));
            let same = |text: &str| text.to_string();
            let options = Options {
                full: true,
                redact: &same,
                redacted: false,
                label: &|tool: &str| tool.replace('_', " "),
                picture: &|_| None,
                zone: chrono::FixedOffset::east_opt(0).unwrap(),
                now: 0,
                version: "test",
            };
            let source = Source {
                id: &found.id,
                conversation: &found.conversation,
                title: &found.title,
                place: found.set.as_deref(),
                words: found.words,
                created: found.created,
            };
            let (page, text) = (html(&source, &options), markdown(&source, &options));
            for made in [&page, &text] {
                assert!(made.contains("What Kumi changed in Renoise"), "{made}");
                assert!(made.contains("Track 1 volume -6 dB"), "{made}");
            }
            let mut said = own.close().await;
            // (the page's style sheet names its own classes, which nobody reads)
            let style = Regex::new(r"(?s)<style>.*?</style>").unwrap();
            said.extend([style.replace_all(&page, "").into_owned(), text]);
            assert_no_live_words(&said);
        })
        .await;
}

fn kept_conversation(saved_at: i64, words: &str) -> SavedConversation {
    SavedConversation {
        saved_at,
        checkpoint: KernelCheckpoint {
            version: 1,
            messages: serde_json::from_value(json!([{"role": "user", "content": words}])).unwrap(),
            origin: Some("test/fixture".into()),
            tools: None,
        },
        changes: None,
        first: None,
        turns: None,
    }
}

/// A Kumi rolled back to before REAPER and Renoise (1.19.x) keeps only Live's places, so its index drops their
/// conversations and takes them for let go. Back on this Kumi, the index is made again from the folders: every one is
/// found again, by its id and by its name, pinned as it was, and none is said to be let go.
#[tokio::test(flavor = "current_thread")]
async fn after_a_rollback_and_back_the_index_finds_reaper_and_renoise_conversations_again() {
    use kumi_runtime::core::place::Places;
    let dir = tempfile::tempdir().unwrap();
    let projects = dir.path().join("projects");
    let ids = ["mf1a2b3c4d5e6", "mf1a2b3c4d5e7", "mf1a2b3c4d5e8", "mf1a2b3c4d5e9"];
    let places = [format!("reaper-{SONG_A}"), format!("renoise-{SONG_B}"), "renoise-unsaved".into(), SONG_A.to_string()];
    {
        let store = create_conversation_store_in(&projects, None, PLACES);
        for (at, (place, id)) in places.iter().zip(ids).enumerate() {
            store.save(place, id, &kept_conversation(1_760_000_000_000 + at as i64, &format!("Request {at}"))).await.unwrap();
        }
        assert!(store.rename(&places[0], ids[0], Some("Night drive mix")).await.unwrap());
        assert!(store.pin(&places[1], ids[1], true).await.unwrap());
        assert_eq!(store.all().await.unwrap().len(), 4);
    }
    // Rolled back: Live's places only. Its index keeps Live's conversation and drops the others.
    {
        let earlier = create_conversation_store_in(&projects, None, Places::LIVE);
        let listed: Vec<String> = earlier.all().await.unwrap().into_iter().map(|row| row.id).collect();
        assert_eq!(listed, [ids[3]]);
        let index: Value = serde_json::from_slice(&std::fs::read(projects.join("conversation-index.json")).unwrap()).unwrap();
        assert!(index["gone"].get(ids[0]).is_some(), "the earlier Kumi took it for let go: {index}");
    }
    // Back on this Kumi.
    let store = create_conversation_store_in(&projects, None, PLACES);
    let mut listed: Vec<String> = store.all().await.unwrap().into_iter().map(|row| row.id).collect();
    listed.sort();
    assert_eq!(listed, ids, "every conversation found again");
    match store.find("Night drive mix").await.unwrap() {
        ConversationMatch::One(row) => assert_eq!((row.id.as_str(), row.place.as_str()), (ids[0], places[0].as_str())),
        other => panic!("{other:?}"),
    }
    match store.find(ids[1]).await.unwrap() {
        ConversationMatch::One(row) => assert!(row.pinned, "pinned as it was"),
        other => panic!("{other:?}"),
    }
    assert!(matches!(store.find(ids[2]).await.unwrap(), ConversationMatch::One(row) if row.place == "renoise-unsaved"));
    let index: Value = serde_json::from_slice(&std::fs::read(projects.join("conversation-index.json")).unwrap()).unwrap();
    for id in ids {
        assert!(index["gone"].get(id).is_none(), "{id} isn't let go: {index}");
    }
}

/// Walking away from Kumi in Renoise: its line goes in Renoise's status bar (REAPER has no status line, so nothing is
/// sent there), and each DAW says whether it records, so no notification lands over a take.
#[tokio::test(flavor = "current_thread")]
async fn walk_away_tells_renoises_status_bar_and_each_daw_says_whether_it_records() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let dir = tempfile::tempdir().unwrap();
            let script = RenoiseScript::new();
            let own = Own::open(dir.path(), Daw::Renoise, DawProfile::RENOISE, renoise(&script), create_renoise_integration).await;
            assert!(own.session.has_daw_message());
            assert!(own.session.daw_message("Kumi · done · the kick is down").await.unwrap(), "shown");
            assert_eq!(script.statuses(), ["Kumi · done · the kick is down"]);
            assert_eq!(own.session.daw_recording().await.unwrap(), Some(false), "stopped, not recording");
            own.close().await;

            let dir = tempfile::tempdir().unwrap();
            let script = ReaperScript::new(2, 0);
            let own = Own::open(dir.path(), Daw::Reaper, DawProfile::REAPER, reaper(&script), create_reaper_integration).await;
            assert!(!own.session.daw_message("Kumi · done").await.unwrap(), "REAPER has no status line");
            assert!(!script.asked().iter().any(|(method, _)| method == "show_status"), "nothing sent");
            assert_eq!(own.session.daw_recording().await.unwrap(), Some(false));
            own.close().await;
        })
        .await;
}

/// A background job started in a REAPER project: the worker gets the task in REAPER's words, and the report the
/// conversation hadn't heard when Kumi closed comes back with that project the next time.
#[tokio::test(flavor = "current_thread")]
async fn a_reaper_projects_background_job_report_comes_back_with_the_project() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let dir = tempfile::tempdir().unwrap();
            let script = ReaperScript::new(2, 0);
            script.set_project(Some(SONG_A), "/lab/A.rpp", "sim:a", "A.rpp");
            let own = Own::open(dir.path(), Daw::Reaper, DawProfile::REAPER, reaper(&script), create_reaper_integration).await;
            own.worker.script.borrow_mut().push_back(step(|| answer("Three references with a punchy kick: Alpha, Beta and Gamma.")));
            own.main.script.borrow_mut().extend([
                step(|| call("start_job", json!({"title": "Kick references", "task": "Find three reference tracks with a punchy kick."}))),
                step(|| answer("A job is looking for references.")),
            ]);
            own.session.submit("Find me kick references.", None).await.unwrap();
            for _ in 0..500 {
                if own.session.jobs().iter().any(|job| job.state == JobState::Done) {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
            assert!(own.session.jobs().iter().any(|job| job.state == JobState::Done), "{:?}", own.session.jobs());
            assert!(!own.worker.requests.borrow().is_empty(), "the worker ran");
            let said = own.close().await;
            assert_no_live_words(&said);

            // Next time, the same project: the report reaches the conversation at its next turn.
            let script = ReaperScript::new(2, 0);
            script.set_project(Some(SONG_A), "/lab/A.rpp", "sim:a-again", "A.rpp");
            let own = Own::open(dir.path(), Daw::Reaper, DawProfile::REAPER, reaper(&script), create_reaper_integration).await;
            own.session.submit("Anything new?", None).await.unwrap();
            assert!(own.main.requests.borrow().last().unwrap().contains("Alpha, Beta and Gamma"), "the report came back");
            let said = own.close().await;
            assert_no_live_words(&said);
        })
        .await;
}

/// Techniques and taste in REAPER: a plan that builds a reusable idea is offered as a technique after the answer, kept on
/// the producer's yes, and in the next conversation's system text in REAPER's words; the producer's reaction is kept
/// with the project's place and REAPER's tag.
#[tokio::test(flavor = "current_thread")]
async fn reaper_techniques_are_offered_and_kept_and_reactions_are_kept_with_the_projects_place() {
    use kumi_runtime::core::{store_client::StoreClient, store_import::JsonFiles};
    use kumi_runtime::{create_technique_store, TechniqueStore};
    use kumi_store::observations::{self, Kind};
    tokio::task::LocalSet::new()
        .run_until(async {
            let dir = tempfile::tempdir().unwrap();
            let files = JsonFiles {
                memory: dir.path().join("memory.json"),
                projects: dir.path().join("projects"),
                techniques: dir.path().join("techniques.json"),
                playbook: dir.path().join("playbook.json"),
                gaps: dir.path().join("gaps.jsonl"),
            };
            let (client, _) = StoreClient::open(dir.path().join("kumi.db"), files, 1).await.unwrap();
            let techniques = create_technique_store(dir.path().join("techniques.json"));
            let script = ReaperScript::new(2, 0);
            script.set_project(Some(SONG_A), "/lab/A.rpp", "sim:a", "A.rpp");
            let (store, kept) = (client.clone(), techniques.clone());
            let own = Own::open_with(dir.path(), Daw::Reaper, DawProfile::REAPER, reaper(&script), create_reaper_integration, |o| {
                o.store = Some(store);
                o.techniques = Some(kept);
            })
            .await;
            own.main.script.borrow_mut().extend([
                step(|| {
                    call(
                        "make_changes",
                        json!({
                            "steps": [{"tool": "track_volume", "input": {"ref": "track:2", "volumeDb": -6}}],
                            "technique": {"name": "Room for the kick", "fits": "a bass that masks the kick", "idea": "ReaEQ dips the bass at the kick's fundamental, then ReaComp ducks it from the kick"},
                        }),
                    )
                }),
                step(|| answer("The bass sits under the kick now.")),
            ]);
            own.session.submit("Make room for the kick.", None).await.unwrap();
            let offered = own.events.borrow().iter().any(|event| matches!(event, SessionEvent::Technique(t) if t.action == TechniqueAction::Offered));
            assert!(offered, "offered after the answer");
            assert!(own.session.answer_technique(true).await.unwrap());
            assert_eq!(techniques.list().await.unwrap().iter().map(|t| t.body.name.clone()).collect::<Vec<_>>(), ["Room for the kick"]);
            // The producer's reaction, kept with where it happened.
            own.main.script.borrow_mut().extend([step(|| call("reaction", json!({"quote": "too dark", "lean": "less", "about": "the bass"}))), step(|| answer("Brightening it."))]);
            own.session.submit("The bass is too dark.", None).await.unwrap();
            client.store().write_wait(|_| Ok(())).unwrap();
            let rows = client.store().read(|c| observations::recent(c, 10)).unwrap();
            let words = rows.iter().find(|row| row.kind == Kind::Words).expect("the reaction kept");
            assert_eq!(words.project.as_deref(), Some(format!("reaper-{SONG_A}").as_str()));
            assert_eq!(words.context["daw"], "reaper");
            // A new conversation: the technique is in its system text, to adapt to this project.
            own.session.new_conversation().await.unwrap();
            own.session.submit("Hello again.", None).await.unwrap();
            let system = latest_system(&own);
            assert!(system.contains("Room for the kick") && system.contains("this sound and project"), "{system}");
            let said = own.close().await;
            assert_no_live_words(&said);
        })
        .await;
}

/// Renoise's places: an unsaved song (which Renoise's tool names by an id of its own until its first save) keeps its
/// conversation in `renoise-unsaved`, not in a place of its own; a saved song keeps its conversation, notes, a background
/// job's unheard report and a technique built there, which all come back with it next time.
#[tokio::test(flavor = "current_thread")]
async fn renoises_unsaved_song_is_renoise_unsaved_and_a_saved_songs_notes_jobs_and_techniques_come_back_with_it() {
    use kumi_runtime::{create_technique_store, TechniqueStore};
    const DEMO: &str = "b9e741f2bc26a406294ae3e2318dbafc";
    tokio::task::LocalSet::new()
        .run_until(async {
            let dir = tempfile::tempdir().unwrap();
            let techniques = create_technique_store(dir.path().join("techniques.json"));
            let script = RenoiseScript::new();
            script.start_again_on_the_empty_song();
            let kept = techniques.clone();
            let own = Own::open_with(dir.path(), Daw::Renoise, DawProfile::RENOISE, renoise(&script), create_renoise_integration, |o| {
                o.techniques = Some(kept)
            })
            .await;
            own.session.submit("Start a song.", None).await.unwrap();
            assert_eq!(kept_once(dir.path(), "renoise-unsaved", 1).await.len(), 1, "the unsaved song's conversation");
            let places: Vec<String> =
                std::fs::read_dir(dir.path().join("projects")).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
            assert!(!places.iter().any(|place| place.starts_with("renoise-") && place != "renoise-unsaved"), "no place of its own: {places:?}");
            // The saved song opens: its own place.
            script.load_the_file();
            own.worker.script.borrow_mut().push_back(step(|| answer("Three swung hat patterns: Delta, Echo and Foxtrot.")));
            own.main.script.borrow_mut().extend([
                step(|| call("remember", json!({"note": "The hats swing at 58 percent", "about": "set"}))),
                step(|| call("start_job", json!({"title": "Hat grooves", "task": "Find three swung hi-hat patterns."}))),
                step(|| {
                    call(
                        "make_changes",
                        json!({
                            "steps": [{"tool": "track_volume", "input": {"ref": "track:2", "volumeDb": -6}}],
                            "technique": {"name": "Swung hats", "fits": "hats that sound stiff", "idea": "Delay column nudges on the off-beats, then a Compressor to glue them"},
                        }),
                    )
                }),
                step(|| answer("The hats swing now.")),
            ]);
            own.session.submit("Make the hats swing.", None).await.unwrap();
            assert!(own.session.answer_technique(true).await.unwrap());
            for _ in 0..500 {
                if own.session.jobs().iter().any(|job| job.state == JobState::Done) {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
            assert_eq!(kept_once(dir.path(), &format!("renoise-{DEMO}"), 1).await.len(), 1, "the song's own conversation");
            assert!(dir.path().join(format!("projects/renoise-{DEMO}/memory.json")).is_file());
            assert_eq!(techniques.list().await.unwrap().iter().map(|t| t.body.name.clone()).collect::<Vec<_>>(), ["Swung hats"]);
            let said = own.close().await;
            assert_no_live_words(&said);

            // Next time, the same song: its note, the job's report and the technique come back.
            let script = RenoiseScript::new();
            let kept = techniques.clone();
            let own = Own::open_with(dir.path(), Daw::Renoise, DawProfile::RENOISE, renoise(&script), create_renoise_integration, |o| {
                o.techniques = Some(kept)
            })
            .await;
            own.session.submit("Anything new?", None).await.unwrap();
            let last = own.main.requests.borrow().last().unwrap().clone();
            assert!(last.contains("Delta, Echo and Foxtrot"), "the report came back");
            let system = latest_system(&own);
            assert!(system.contains("The hats swing at 58 percent") && system.contains("Swung hats"), "{system}");
            let said = own.close().await;
            assert_no_live_words(&said);
        })
        .await;
}

/// A REAPER session lists only REAPER's conversations (and imported ones); another DAW's is still found by its id, and
/// taking it up says whose it is and how to open it there (review L4).
#[tokio::test(flavor = "current_thread")]
async fn a_reaper_session_leaves_lives_conversations_to_live() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let dir = tempfile::tempdir().unwrap();
            {
                let store = create_conversation_store_in(dir.path().join("projects"), None, PLACES);
                store.save(SONG_B, "mf1a2b3c4d5e1", &kept_conversation(1_760_000_000_000, "A Live Set's mix")).await.unwrap();
                store.save("imported", "mf1a2b3c4d5e2", &kept_conversation(1_760_000_000_001, "From a file")).await.unwrap();
            }
            let script = ReaperScript::new(2, 0);
            script.set_project(Some(SONG_A), "/lab/A.rpp", "sim:a", "A.rpp");
            let own = Own::open(dir.path(), Daw::Reaper, DawProfile::REAPER, reaper(&script), create_reaper_integration).await;
            own.session.submit("Hello.", None).await.unwrap();
            kept_once(dir.path(), &format!("reaper-{SONG_A}"), 1).await;
            let listed: Vec<String> = own.session.all_conversations().await.unwrap().into_iter().map(|row| row.place).collect();
            assert!(!listed.contains(&SONG_B.to_string()), "Live's isn't listed: {listed:?}");
            assert!(listed.contains(&"imported".to_string()) && listed.contains(&format!("reaper-{SONG_A}")), "{listed:?}");
            assert!(matches!(own.session.find_conversation("mf1a2b3c4d5e1").await.unwrap(), ConversationMatch::One(_)));
            let refused = own.session.resume_conversation("mf1a2b3c4d5e1").await.unwrap_err();
            assert_eq!(refused.message(), "That conversation is about a Live Set: open Kumi with Live (/daw live) to carry it on there.");
            own.close().await;
        })
        .await;
}
