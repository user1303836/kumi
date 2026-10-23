//! What the model is first sent over Logic Pro, as the app sends it with every option on: the system text and the
//! tools, through Kumi's host (in process) in front of the simulated Logic. Logic's instruction pack carries the
//! persona and the general rules Live's does, in Logic's words, and the request stays smaller than Live's (the full
//! first-request golden's): nothing in it is in Live's words.
use async_trait::async_trait;
use daw_bridge::{files::ensure_secret, paths::BridgePaths, Daw};
use daw_mcp_server::{host::Host, simulator::logic::LogicScript, simulator::Simulator};
use futures::stream::{self, StreamExt};
use futures::FutureExt;
use kumi_common::js::json::stringify;
use kumi_runtime::ai::error::LanguageModelError;
use kumi_runtime::ai::types::{
    CallOptions, FinishReason, FinishReasonUnified, FunctionTool, InputTokens, OutputTokens, StreamPart, StreamParts, Usage as ModelUsage,
};
use kumi_runtime::core::context::IntegrationContext;
use kumi_runtime::core::contracts::*;
use kumi_runtime::core::memory::{create_memory_store, MemoryStoreOptions};
use kumi_runtime::core::session::{JobsOptions, VideoDirectories};
use kumi_runtime::core::techniques::create_technique_store;
use kumi_runtime::integrations::{
    logic::create_logic_integration,
    own_bridge::{Connect, OwnOptions},
};
use kumi_runtime::kernel::agent::{create_agent_kernel, AgentKernelOptions, LanguageModel, ModelBinding, ModelRequest};
use kumi_runtime::library::{create_library, sources::SourceOptions, LibraryOptions};
use kumi_runtime::{
    create_conversation_store, create_goal_store, create_objective_store, create_playbook_store, create_recipe_store, create_session,
    SessionOptions, DEFAULT_BUDGET,
};
use regex::Regex;
use serde_json::{json, Value};
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
    sync::Arc,
};
kumi_testkit::in_process!();

fn finish(unified: FinishReasonUnified) -> StreamPart {
    let raw = serde_json::to_value(unified).unwrap().as_str().unwrap().to_string();
    let usage = ModelUsage {
        input_tokens: InputTokens { total: Some(3.0), no_cache: Some(3.0), cache_read: Some(0.0), cache_write: Some(0.0) },
        output_tokens: OutputTokens { total: Some(2.0), text: Some(2.0), reasoning: Some(0.0) },
        raw: None,
    };
    StreamPart::Finish { usage, finish_reason: FinishReason { unified, raw: Some(raw) }, provider_metadata: None }
}

struct Done;
#[async_trait(?Send)]
impl LanguageModel for Done {
    async fn do_stream(&self, _: CallOptions) -> Result<StreamParts, LanguageModelError> {
        let parts = vec![
            StreamPart::TextStart { id: "t".into(), provider_metadata: None },
            StreamPart::TextDelta { id: "t".into(), delta: "Done.".into(), provider_metadata: None },
            StreamPart::TextEnd { id: "t".into(), provider_metadata: None },
            finish(FinishReasonUnified::Stop),
        ];
        Ok(stream::iter(parts).boxed_local())
    }
}

/// The first request: its system text, the tools it sent, every tool the kernel has, and the bytes its instructions
/// and tools take as the kernel counts them.
#[derive(Default)]
struct First {
    system: RefCell<Option<String>>,
    sent: RefCell<Vec<FunctionTool>>,
    tools: RefCell<Vec<Rc<dyn KernelTool>>>,
    fixed: Cell<Option<usize>>,
}

fn recording(into: Rc<First>) -> KernelFactory {
    Rc::new(move |options: KernelOptions| {
        let into = into.clone();
        async move {
            *into.tools.borrow_mut() = options.tools.clone();
            let (asked, sized) = (into.clone(), into.clone());
            let binding = ModelBinding {
                id: "test/fixture".into(),
                model: Rc::new(Done),
                prepare: Box::new(move |request: ModelRequest| {
                    if asked.system.borrow().is_none() {
                        *asked.system.borrow_mut() = Some(request.instructions.clone());
                        *asked.sent.borrow_mut() = request.tools.clone();
                    }
                    CallOptions {
                        prompt: request.messages,
                        tools: (!request.tools.is_empty()).then_some(request.tools),
                        ..CallOptions::default()
                    }
                }),
                budget: Some(Box::new(move |fixed| {
                    sized.fixed.set(sized.fixed.get().or(Some(fixed)));
                    DEFAULT_BUDGET
                })),
            };
            let kernel = Rc::new(create_agent_kernel(AgentKernelOptions {
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

/// A session over the simulated Logic with every option a producer's Kumi turns on, asked once.
async fn first_request() -> Rc<First> {
    let dir = tempfile::tempdir().unwrap();
    let paths = BridgePaths::new(dir.path(), Daw::Logic);
    let secret = ensure_secret(&paths.secret()).unwrap();
    let script = LogicScript::new();
    let mut config = script.config(secret, "com.apple.logic10");
    config.discovery_dir = Some(paths.discovery());
    let simulator = Simulator::start(config).await.unwrap();
    let host = Rc::new(in_process::InProcess { host: Arc::new(Host::new(Daw::Logic, dir.path())), id: Cell::new(0) });
    let home = dir.path().to_path_buf();
    let integration: IntegrationFactory = Box::new(move |listener: ConnectionListener| {
        let host = host.clone();
        let connect: Connect = Rc::new(move |_| {
            let host = host.clone();
            async move { Ok(host as Rc<dyn kumi_runtime::mcp::client::McpEndpoint>) }.boxed_local()
        });
        create_logic_integration(OwnOptions {
            context: IntegrationContext::new(listener),
            kumi_home: home.clone(),
            connect: Some(connect),
            open_file: None,
        })
    });
    let first = Rc::new(First::default());
    let worker = Rc::new(First::default());
    let mut options = SessionOptions::new(recording(first.clone()), integration, Rc::new(|_: SessionEvent| {}));
    options.profile = DawProfile::LOGIC;
    options.timeout_ms = Some(20_000);
    options.close_timeout_ms = Some(5_000);
    let text = |name: &str| dir.path().join(name).to_string_lossy().into_owned();
    options.memory = Some(create_memory_store(MemoryStoreOptions {
        projects_dir: dir.path().join("projects"),
        producer_file: dir.path().join("producer.json"),
    }));
    options.listen = true;
    options.web = true;
    options.recipes = Some(create_recipe_store(dir.path().join("recipes")));
    options.watch = Some(VideoDirectories { videos_dir: text("videos"), tools_dir: text("tools") });
    options.references = Some(Rc::new(kumi_runtime::references::store::ReferenceStore::new(text("references"))));
    options.techniques = Some(create_technique_store(dir.path().join("techniques.json")));
    options.gaps = Some(text("gaps.jsonl"));
    options.conversations = Some(create_conversation_store(dir.path()));
    options.library = Some(create_library(LibraryOptions {
        dir: text("library"),
        find_sets: Some(false),
        fork: Some(false),
        sources: Some(SourceOptions { home: Some(text("home")), ..Default::default() }),
        ..Default::default()
    }));
    options.playbook = Some(create_playbook_store(text("playbook.json")));
    options.goals = Some(create_goal_store(text("goals")));
    options.objectives = Some(create_objective_store(text("objectives")));
    options.timings = Some(text("timings.jsonl"));
    options.jobs = Some(JobsOptions { worker_factory: recording(worker), max: 3, store: None });
    let session = create_session(options).unwrap();
    session.start().await.unwrap();
    session.submit("Make the kick punchier.", None).await.unwrap();
    session.close().await.unwrap();
    simulator.stop();
    first
}

/// Live's first request with every option on, as its golden keeps it: the bytes of its instructions and tools, and
/// of its system text.
fn lives() -> (usize, usize) {
    let folder = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/goldens/first-request/full");
    let meta: Value = serde_json::from_str(&std::fs::read_to_string(folder.join("meta.json")).unwrap()).unwrap();
    let system = std::fs::read_to_string(folder.join("system.txt")).unwrap();
    (meta["fixed"].as_u64().unwrap() as usize, system.len())
}

#[tokio::test(flavor = "current_thread")]
async fn logics_first_request_carries_its_pack_in_its_words_and_stays_smaller_than_lives() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let first = first_request().await;
            let system = first.system.borrow().clone().expect("the model was asked");
            let sent = first.sent.borrow().clone();
            let fixed = first.fixed.get().expect("the kernel counted its fixed part");
            let by_hand = system.len() + stringify(&serde_json::to_value(&sent).unwrap()).len();
            assert_eq!(fixed, by_hand, "instructions and tools, as the kernel counts them");
            let (live_fixed, live_system) = lives();
            let names: Vec<&str> = sent.iter().map(|tool| tool.name.as_str()).collect();
            let deferred: Vec<String> =
                first.tools.borrow().iter().filter(|tool| tool.deferred()).map(|tool| tool.name().to_owned()).collect();
            println!(
                "Logic's first request: {fixed} bytes ({} of system text, {} tools sent: {names:?}); deferred: {deferred:?}. Live's: {live_fixed} ({live_system} of system text).",
                system.len(),
                sent.len()
            );
            if std::env::var_os("KUMI_PRINT_SYSTEM").is_some() {
                println!("{system}");
            }
            // The pack first (Kumi's persona, the general rules, then Logic's words and ways), the rules every DAW of
            // Kumi's own bridge shares, in Logic's words, the change set's instructions after them, then what the session
            // adds (memory, techniques, gaps, jobs).
            let pack = kumi_runtime::integrations::logic::DAW.instructions;
            let shared = kumi_runtime::integrations::own_bridge::own_rules(&kumi_runtime::integrations::logic::DAW);
            assert!(system.starts_with("You are Kumi, a concise, practical producer collaborator."), "{system}");
            assert!(system.starts_with(&format!("{pack} {shared} Change Logic Pro with your change tools")), "{system}");
            assert_eq!(system.matches("If the observation has sinceLastTime").count(), 1, "{system}");
            assert!(system.contains("ask logic_manual: it answers from Apple's Logic Pro User Guide"), "{system}");
            assert!(fixed < live_fixed, "{fixed} bytes, Live's {live_fixed}");
            assert!(system.len() < live_system, "{} bytes of system text, Live's {live_system}", system.len());
            assert!(deferred.contains(&"logic_manual".to_string()), "the guide waits behind find_tools: {deferred:?}");

            // Nothing the model is first sent is in Live's words: the system text, a clip not even (Logic's are regions),
            // and every tool's definition as the whole-session guard has them (core's listening tools still say "clip"
            // for any DAW, fake_daw_session.rs says).
            let words = r"Live|Set|Sets|Ableton|Session|Arrangement|Remote Script|Control Surface|Main|Drum Rack|Operator|[Rr]acks?|[Cc]lip slots?|[Ss]cenes?";
            let strict = Regex::new(&format!(r"\b({words}|[Cc]lips?)\b")).unwrap();
            let live = Regex::new(&format!(r"\b({words})\b")).unwrap();
            let mut said = vec![("system text".to_string(), system.clone(), &strict)];
            said.extend(first.tools.borrow().iter().map(|tool| {
                let definition = json!({"name": tool.name(), "description": tool.description(), "inputSchema": tool.input_schema()});
                // (a file the producer may hand Kumi in any DAW, which Kumi reads without Live: as the whole-session guard
                // allows it)
                (tool.name().to_owned(), definition.to_string().replace("a Max patch or Max for Live device", ""), &live)
            }));
            let found: Vec<String> = said
                .iter()
                .flat_map(|(at, text, words)| {
                    words.find_iter(text).map(move |word| format!("{at}: {:?} in …{}…", word.as_str(), around(text, word.start(), word.end())))
                })
                .collect();
            assert!(found.is_empty(), "in Live's words:\n{}", found.join("\n"));
        })
        .await;
}

fn around(text: &str, start: usize, end: usize) -> &str {
    let mut from = start.saturating_sub(60);
    while !text.is_char_boundary(from) {
        from -= 1;
    }
    let mut to = (end + 60).min(text.len());
    while !text.is_char_boundary(to) {
        to += 1;
    }
    &text[from..to]
}
