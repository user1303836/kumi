//! What the model is sent with Ableton, byte for byte, as Kumi sends it today: the system text, every tool (those sent
//! with each request and those behind find_tools, with how each is offered), the first message with its observation,
//! and the tools' fingerprint; a worker's first request too. The integration is Live's own, on the fullest catalog the
//! public-tools oracle holds, reading the observation oracle's `basic` Set. `KUMI_BLESS=1` writes the files afresh.

use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::rc::Rc;

use async_trait::async_trait;
use futures::stream::{self, StreamExt};
use futures::FutureExt;
use kumi_agent::core::contracts::*;
use kumi_agent::core::memory::{create_memory_store, MemoryStoreOptions};
use kumi_agent::core::session::JobsOptions;
use kumi_agent::core::session::VideoDirectories;
use kumi_agent::core::techniques::create_technique_store;
use kumi_agent::kernel::agent::{create_agent_kernel, AgentKernel, AgentKernelOptions};
use kumi_agent::mcp::client::{McpEndpoint, StderrStatus};
use kumi_agent::mcp::types::{CallToolResult, Implementation, ListToolsResult};
use kumi_agent::{
    create_conversation_store, create_goal_store, create_objective_store, create_playbook_store, create_recipe_store, create_session,
    with_fallback, SessionOptions,
};
use kumi_ai::binding::DEFAULT_BUDGET;
use kumi_ai::binding::{LanguageModel, ModelBinding, ModelRequest};
use kumi_ai::error::LanguageModelError;
use kumi_ai::types::{
    CallOptions, FinishReason, FinishReasonUnified, InputTokens, OutputTokens, StreamPart, StreamParts, ToolCall, Usage as ModelUsage,
};
use kumi_common::abort::{Signal, SignalExt};
use kumi_common::js::json::stringify;
use kumi_daw_ableton::options::{AbletonOptions, HandsSetup};
use kumi_daw_ableton::willington::WillingtonSwitch;
use kumi_daw_ableton::{create_ableton_integration, create_inference_only_integration};
use kumi_library::{create_library, sources::SourceOptions, LibraryOptions};
use kumi_types::contracts::*;
use kumi_types::errors::{FailureKind, KumiError, RuntimeError};
use serde_json::{json, Value};

const ASKED: &str = "Make the kick punchier.";

/// The bridge on the fixtures: one public-tools case's catalog and version, answering the `basic` observation's reads
/// by tool and kind.
struct Bridge {
    version: Value,
    catalog: Value,
    replies: std::collections::HashMap<String, Value>,
}
#[async_trait(?Send)]
impl McpEndpoint for Bridge {
    fn pid(&self) -> Option<u32> {
        None
    }
    fn server_info(&self) -> Option<Implementation> {
        Some(serde_json::from_value(json!({"name": "fixture", "version": self.version})).unwrap())
    }
    async fn list(&self, _: Option<&str>, signal: Signal) -> Result<ListToolsResult, RuntimeError> {
        signal.check()?;
        Ok(serde_json::from_value(json!({"tools": self.catalog})).unwrap())
    }
    async fn call(&self, name: &str, args: JsonObject, signal: Signal) -> Result<CallToolResult, RuntimeError> {
        signal.check()?;
        let key = format!("{name}:{}", args.get("kind").and_then(Value::as_str).unwrap_or(""));
        let reply = self.replies.get(&key).cloned().ok_or_else(|| RuntimeError::plain(format!("no reply for {key}")))?;
        Ok(serde_json::from_value(reply).unwrap())
    }
    fn on_catalog_changed(&self, _: Rc<dyn Fn()>) -> Box<dyn Fn()> {
        Box::new(|| {})
    }
    fn on_disconnect(&self, _: Rc<dyn Fn()>) -> Box<dyn Fn()> {
        Box::new(|| {})
    }
    fn stderr_status(&self) -> StderrStatus {
        StderrStatus { bytes: 0, truncated: false }
    }
    async fn close(&self) -> Result<(), RuntimeError> {
        Ok(())
    }
}

/// Which public-tools case the bridge plays.
#[derive(Clone, Copy)]
enum Catalog {
    Fullest,
    Oldest,
    Newest,
}

fn version_of(case: &Value) -> Vec<u64> {
    case["config"]["version"].as_str().unwrap_or("1.0.73").split('.').map(|part| part.parse().unwrap_or(0)).collect()
}

fn bridge(catalog: Catalog) -> (Rc<Bridge>, bool) {
    let source: Value = serde_json::from_str(&kumi_testkit::oracle("kumi-daw-ableton/public-tools-oracle.json")).unwrap();
    let cases = source["cases"].as_array().unwrap();
    let size = |case: &&Value| case["definitions"].as_array().unwrap().len();
    // The first of the most tools, so a tie picks the same case every time.
    let pick =
        |better: &dyn Fn(&Value, &Value) -> bool| cases.iter().fold(&cases[0], |best, case| if better(case, best) { case } else { best });
    let case = match catalog {
        Catalog::Fullest => pick(&|case, best| size(&case) > size(&best)),
        Catalog::Oldest => {
            pick(&|case, best| version_of(case) < version_of(best) || (version_of(case) == version_of(best) && size(&case) > size(&best)))
        }
        Catalog::Newest => {
            pick(&|case, best| version_of(case) > version_of(best) || (version_of(case) == version_of(best) && size(&case) > size(&best)))
        }
    };
    let tools =
        Value::Array(case["catalog"].as_array().unwrap().iter().map(|id| source["pool"][id.as_u64().unwrap() as usize].clone()).collect());
    let observed: Value = serde_json::from_str(&kumi_testkit::oracle("kumi-daw-ableton/observation-oracle.json")).unwrap();
    let basic = observed["cases"].as_array().unwrap().iter().find(|case| case["label"] == "basic").unwrap();
    let replies = basic["calls"]
        .as_array()
        .unwrap()
        .iter()
        .zip(basic["responses"].as_array().unwrap())
        .map(|(call, id)| {
            let key = format!("{}:{}", call["name"].as_str().unwrap(), call["args"]["kind"].as_str().unwrap_or(""));
            (key, observed["values"][id.as_u64().unwrap() as usize]["reply"].clone())
        })
        .collect();
    let version = case["config"].get("version").cloned().unwrap_or(json!("1.0.73"));
    (Rc::new(Bridge { version, catalog: tools, replies }), case["config"]["hands"] != false)
}

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
            tool_call_id: "call_1".into(),
            tool_name: name.into(),
            input: input.to_string(),
            provider_executed: None,
            dynamic: None,
            provider_metadata: None,
        }),
        finish(FinishReasonUnified::ToolCalls),
    ]
}

type Script = Rc<dyn Fn(usize) -> Vec<StreamPart>>;
struct Scripted {
    script: Script,
    calls: Cell<usize>,
}
#[async_trait(?Send)]
impl LanguageModel for Scripted {
    async fn do_stream(&self, _: CallOptions) -> Result<StreamParts, LanguageModelError> {
        self.calls.set(self.calls.get() + 1);
        Ok(stream::iter((self.script)(self.calls.get())).boxed_local())
    }
}

/// One request as the binding was given it.
struct Sent {
    instructions: String,
    messages: Vec<kumi_ai::types::Message>,
    tools: Vec<kumi_ai::types::FunctionTool>,
}

/// What one kernel was made with and sent: its tools as offered, each request, the bytes its instructions and tools
/// take, and the kernel itself (for its fingerprint once it has answered).
#[derive(Default)]
struct Recorded {
    tools: RefCell<Vec<Rc<dyn KernelTool>>>,
    requests: RefCell<Vec<Sent>>,
    fixed: Cell<Option<usize>>,
    kernel: RefCell<Option<Rc<AgentKernel>>>,
}

/// A kernel factory making today's agent kernel, on a scripted model, recording into `into`.
fn recording(into: Rc<Recorded>, script: Script, max_steps: Option<usize>) -> KernelFactory {
    Rc::new(move |options: KernelOptions| {
        let (into, script) = (into.clone(), script.clone());
        async move {
            *into.tools.borrow_mut() = options.tools.clone();
            let requests = into.clone();
            let sized = into.clone();
            let binding = ModelBinding {
                id: "test/fixture".into(),
                model: Rc::new(Scripted { script, calls: Cell::new(0) }),
                prepare: Box::new(move |request: ModelRequest| {
                    requests.requests.borrow_mut().push(Sent {
                        instructions: request.instructions.clone(),
                        messages: request.messages.clone(),
                        tools: request.tools.clone(),
                    });
                    CallOptions {
                        prompt: request.messages,
                        tools: (!request.tools.is_empty()).then_some(request.tools),
                        ..CallOptions::default()
                    }
                }),
                budget: Some(Box::new(move |fixed| {
                    sized.fixed.set(Some(fixed));
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
                max_steps,
                budget: None,
            })?);
            *into.kernel.borrow_mut() = Some(kernel.clone());
            Ok(kernel as Rc<dyn Kernel>)
        }
        .boxed_local()
    })
}

/// How the session is set up for one golden.
#[derive(Clone, Copy)]
struct Setup {
    catalog: Catalog,
    /// Every option a producer's Kumi turns on, or none.
    every_option: bool,
    willington: Option<WillingtonSwitch>,
    /// Live fails to start, so the session carries on without it.
    without_live: bool,
}

fn now() -> chrono::DateTime<chrono::Utc> {
    chrono::DateTime::parse_from_rfc3339("2026-10-03T12:00:00Z").unwrap().with_timezone(&chrono::Utc)
}

/// The integration as the app makes it: Live's, behind the fallback to running without it.
fn integration(setup: Setup) -> IntegrationFactory {
    Box::new(move |on_connection: ConnectionListener| {
        let (endpoint, hands) = bridge(setup.catalog);
        let mut options = AbletonOptions::new(on_connection.clone());
        let without_live = setup.without_live;
        options.connect = Some(Rc::new(move |_| {
            let endpoint = endpoint.clone();
            async move {
                if without_live {
                    return Err(KumiError::new(FailureKind::Live, "Kumi can't reach Live.").into());
                }
                Ok(endpoint as Rc<dyn McpEndpoint>)
            }
            .boxed_local()
        }));
        options.hands = Some(if hands { HandsSetup::Open(Rc::new(|| async { Ok(None) }.boxed_local())) } else { HandsSetup::Disabled });
        options.now = Some(Rc::new(now));
        options.generation = Some("connection".into());
        options.reconnect_interval_ms = Some(3_600_000);
        options.focus_interval_ms = Some(3_600_000);
        if let Some(switch) = setup.willington {
            options.willington = Some(Rc::new(move || Some(switch)));
        }
        let fallback = on_connection.clone();
        with_fallback(
            create_ableton_integration(options),
            Rc::new(move || {
                let on_connection = fallback.clone();
                create_inference_only_integration(Rc::new(move |state| on_connection(state, None)))
            }),
            Rc::new(|_| {}),
        )
    })
}

struct Run {
    main: Rc<Recorded>,
    worker: Rc<Recorded>,
    dir: tempfile::TempDir,
}

/// One session, set up as asked, answering `ASKED` once with `script`.
async fn run(setup: Setup, script: Script) -> Run {
    let dir = tempfile::tempdir().unwrap();
    let (main, worker) = (Rc::new(Recorded::default()), Rc::new(Recorded::default()));
    let mut options = SessionOptions::new(recording(main.clone(), script, None), integration(setup), Rc::new(|_: SessionEvent| {}));
    options.timeout_ms = Some(20_000);
    options.close_timeout_ms = Some(5_000);
    if setup.every_option {
        let text = |name: &str| dir.path().join(name).to_string_lossy().into_owned();
        options.memory = Some(create_memory_store(MemoryStoreOptions {
            projects_dir: dir.path().join("projects"),
            producer_file: dir.path().join("producer.json"),
        }));
        options.listen = true;
        options.web = true;
        options.recipes = Some(create_recipe_store(dir.path().join("recipes")));
        options.watch = Some(VideoDirectories { videos_dir: text("videos"), tools_dir: text("tools") });
        options.references = Some(Rc::new(kumi_listening::references::store::ReferenceStore::new(text("references"))));
        options.techniques = Some(create_technique_store(dir.path().join("techniques.json")));
        options.gaps = Some(text("gaps.jsonl"));
        options.conversations = Some(create_conversation_store(dir.path()));
        options.library = Some(create_library(LibraryOptions {
            daws: kumi::daw::registry::LIBRARIES,
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
        options.jobs = Some(JobsOptions {
            worker_factory: recording(worker.clone(), Rc::new(|_| answer("Found three.")), Some(40)),
            max: 3,
            store: None,
        });
    }
    let session = create_session(options).unwrap();
    session.start().await.unwrap();
    session.submit(ASKED, None).await.unwrap();
    session.close().await.unwrap();
    Run { main, worker, dir }
}

/// Machine bits out of the bytes: the run's own folder, and its separators.
fn plain(text: &str, dir: &Path) -> String {
    let folder = dir.to_string_lossy().into_owned();
    let mut out = text.replace(&folder, "<dir>");
    // The folder as JSON writes it (Windows' backslashes doubled), and with forward slashes.
    out = out.replace(&folder.replace('\\', "\\\\"), "<dir>").replace(&folder.replace('\\', "/"), "<dir>");
    out = out.replace("\r\n", "\n");
    // Running without Live stamps its observation with the clock's time, which nothing injects (as JSON in a message,
    // or as a string inside JSON).
    let stamp = regex::Regex::new(r#"(observedAt\\?":\\?")(\d{4}-\d\d-\d\dT[0-9:.]+Z)"#).unwrap();
    stamp
        .replace_all(
            &out,
            |found: &regex::Captures| {
                if &found[2] == "2026-10-03T12:00:00.000Z" {
                    found[0].to_string()
                } else {
                    format!("{}<now>", &found[1])
                }
            },
        )
        .into_owned()
}

/// Every tool as the kernel was given it, in order: its definition, and how it's offered.
fn tools(recorded: &Recorded) -> Value {
    Value::Array(
        recorded
            .tools
            .borrow()
            .iter()
            .map(|tool| {
                json!({
                    "name": tool.name(),
                    "description": tool.description(),
                    "inputSchema": tool.input_schema(),
                    "deferred": tool.deferred(),
                    "listedBy": tool.listed_by(),
                    "reach": format!("{:?}", tool.reach()),
                    "stream": tool.stream(Signal::new(), Rc::new(|| {})).is_some(),
                })
            })
            .collect(),
    )
}

fn pretty(value: &Value) -> String {
    let mut text = serde_json::to_string_pretty(value).unwrap();
    text.push('\n');
    text
}

/// The first request's files: its system text, its first message, the tools it sent, every tool the kernel has, and
/// the fingerprint and size of instructions and tools.
fn files(recorded: &Recorded) -> Vec<(&'static str, String)> {
    let requests = recorded.requests.borrow();
    let first = requests.first().expect("the model was asked");
    let message = first.messages.first().map(|message| serde_json::to_value(message).unwrap()).unwrap_or(Value::Null);
    let words: String = message["content"]
        .as_array()
        .map(|parts| parts.iter().filter_map(|part| part["text"].as_str()).collect::<Vec<_>>().join("\n"))
        .unwrap_or_else(|| message["content"].as_str().unwrap_or("").to_string());
    let kernel = recorded.kernel.borrow().clone().expect("a kernel");
    let key = kernel.checkpoint().ok().and_then(|checkpoint| checkpoint.tools);
    let sent: Vec<&str> = first.tools.iter().map(|tool| tool.name.as_str()).collect();
    vec![
        ("system.txt", first.instructions.clone()),
        ("message.txt", format!("{words}\n")),
        ("messages.json", pretty(&serde_json::to_value(&first.messages).unwrap())),
        ("sent.json", pretty(&serde_json::to_value(&first.tools).unwrap())),
        ("tools.json", pretty(&tools(recorded))),
        (
            "meta.json",
            pretty(&json!({
                "toolsKey": key,
                "fixed": recorded.fixed.get(),
                "fixedByHand": first.instructions.len() + stringify(&serde_json::to_value(&first.tools).unwrap()).len(),
                "sent": sent,
                "requests": requests.len(),
            })),
        ),
    ]
}

fn golden(folder: &str, name: &str, actual: &str) {
    let file: PathBuf = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/goldens/first-request").join(folder).join(name);
    if std::env::var_os("KUMI_BLESS").is_some() {
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, actual).unwrap();
        return;
    }
    let expected =
        std::fs::read_to_string(&file).unwrap_or_else(|_| panic!("{} is missing: run this test with KUMI_BLESS=1", file.display()));
    if actual != expected {
        panic!("{} changed: check the difference, then run this test with KUMI_BLESS=1\n{}", file.display(), difference(&expected, actual));
    }
}

/// The first lines that differ, with a little around them.
fn difference(expected: &str, actual: &str) -> String {
    let (want, got): (Vec<&str>, Vec<&str>) = (expected.lines().collect(), actual.lines().collect());
    let at = want.iter().zip(&got).position(|(a, b)| a != b).unwrap_or(want.len().min(got.len()));
    let from = at.saturating_sub(3);
    let mut out = format!("first difference at line {}:\n", at + 1);
    for (label, lines) in [("expected", &want), ("actual", &got)] {
        out.push_str(&format!("--- {label}\n"));
        for (n, line) in lines.iter().enumerate().skip(from).take(8) {
            out.push_str(&format!("{:>5} {}{line}\n", n + 1, if n == at { ">" } else { " " }));
        }
    }
    out
}

fn check(folder: &str, recorded: &Recorded, dir: &Path) {
    for (name, text) in files(recorded) {
        golden(folder, name, &plain(&text, dir));
    }
}

macro_rules! local_test {
    ($name:ident, $body:block) => {
        #[tokio::test(flavor = "current_thread")]
        async fn $name() {
            tokio::task::LocalSet::new().run_until(async $body).await;
        }
    };
}

const FULL: Setup = Setup { catalog: Catalog::Fullest, every_option: true, willington: Some(WillingtonSwitch::On), without_live: false };

local_test!(with_every_option_on_the_first_request_is_as_it_was, {
    let run = run(FULL, Rc::new(|_| answer("Done."))).await;
    check("full", &run.main, run.dir.path());
});

local_test!(with_willington_off_the_first_request_is_as_it_was, {
    let run = run(Setup { willington: Some(WillingtonSwitch::Off), ..FULL }, Rc::new(|_| answer("Done."))).await;
    check("willington-off", &run.main, run.dir.path());
});

local_test!(with_no_option_on_the_first_request_is_as_it_was, {
    let run = run(Setup { every_option: false, willington: None, ..FULL }, Rc::new(|_| answer("Done."))).await;
    check("minimal", &run.main, run.dir.path());
});

local_test!(without_live_the_first_request_is_as_it_was, {
    let run = run(Setup { without_live: true, ..FULL }, Rc::new(|_| answer("Done."))).await;
    check("without-live", &run.main, run.dir.path());
});

local_test!(on_the_oldest_bridge_the_first_request_is_as_it_was, {
    let run = run(Setup { catalog: Catalog::Oldest, ..FULL }, Rc::new(|_| answer("Done."))).await;
    check("oldest-bridge", &run.main, run.dir.path());
});

local_test!(on_the_newest_bridge_the_first_request_is_as_it_was, {
    let run = run(Setup { catalog: Catalog::Newest, ..FULL }, Rc::new(|_| answer("Done."))).await;
    check("newest-bridge", &run.main, run.dir.path());
});

local_test!(a_workers_first_request_is_as_it_was, {
    let task = json!({"title": "Kick references", "task": "Find three reference tracks with a punchy kick at 124 BPM."});
    let run = run(FULL, Rc::new(move |n| if n == 1 { call("start_job", task.clone()) } else { answer("Started a job.") })).await;
    // The worker runs beside the answer; it has asked its model by the time the session has closed, or soon after.
    for _ in 0..500 {
        if !run.worker.requests.borrow().is_empty() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    check("worker", &run.worker, run.dir.path());
});
