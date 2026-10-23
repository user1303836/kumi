//! Opt-in eval of how the configured model knows Logic Pro and works in it: knowledge probes for Logic's terms and
//! ways (Apple's Logic Pro User Guide asked and cited where Logic's instruction pack doesn't state the answer), change
//! requests, the limits Kumi's bridge can't pass, and listening. It uses your sign-in and model but never Logic Pro:
//! each case is a session over the simulated Logic (Kumi's host in process, in front of the Scratch project as the
//! bridge's tests recorded it), with Logic's instruction pack, change set and listening, and a throwaway folder for
//! Kumi's home, memory, recipes and library. The guide is answered from a fixture of Logic's topics
//! (fixtures/eval_logic/logic-guide.json, laid out as Apple's site lays the guide out), never the network.
//!
//!   cargo run --release -p kumi --example eval_logic [-- <part of a case name>[, <part of another>]]
//!   cargo run -p kumi --example eval_logic -- --list    each case's prompts and what its check looks for
//!   cargo run -p kumi --example eval_logic -- --dry     each check against a canned answer per case: no model, no
//!                                                        sign-in (tests/eval_logic_dry.rs runs it)
//!
//! EVAL_EFFORT=low|medium|high… sets the model's reasoning effort, EVAL_TRACE=1 prints each tool call and what it
//! returned and each answer whole, and EVAL_TIMINGS=<file> writes each turn's timings.jsonl line after a line naming
//! its case, as eval_changes does.
use async_trait::async_trait;
use daw_bridge::{files::ensure_secret, paths::BridgePaths, Daw};
use daw_mcp_server::{
    host::Host,
    simulator::{logic::LogicScript, Simulator},
};
use futures::{FutureExt, StreamExt};
use kumi::config::{load_inference_config, safe_error};
use kumi_common::{
    abort::Signal,
    js::{
        json::stringify,
        number::{round, to_fixed, to_string},
        string::{head, trim},
    },
    time::perf_now,
};
use kumi_runtime::{
    ai::{
        error::LanguageModelError,
        types::{
            CallOptions, FinishReason, FinishReasonUnified, InputTokens, OutputTokens, StreamPart, StreamParts, ToolCall,
            Usage as ModelUsage,
        },
    },
    core::{
        context::IntegrationContext,
        contracts::{ChangeState, WatchEvent},
        daw::DawProfile,
        memory::MemoryStoreOptions,
    },
    create_agent_kernel, create_memory_store, create_recipe_store, create_session, create_technique_store,
    integrations::{
        logic::create_logic_integration,
        own_bridge::{Connect, OpenFile, OwnOptions},
    },
    kernel::agent::{LanguageModel, ModelRequest},
    library::{create_library, sources::SourceOptions, LibraryOptions, LibraryWebOptions},
    mcp::{
        client::{McpEndpoint, StderrStatus},
        types::{CallToolResult, Implementation, ListToolsResult},
    },
    open_credential_store, resolve_model,
    system::process_env,
    web::net::{WebClient, WebFailure, WebRequest, WebResponse},
    AgentKernel, AgentKernelOptions, ChangeRecord, Effort, HeardEvent, IntegrationFactory, JsonObject, Kernel, KernelEvent, KernelFactory,
    KernelOptions, ModelBinding, ResolveModelOptions, RuntimeError, Session, SessionController, SessionEvent, SessionOptions, EFFORTS,
};
use regex::Regex;
use serde::Deserialize;
use serde_json::{json, Value};
use std::{
    cell::{Cell, RefCell},
    collections::{BTreeMap, VecDeque},
    io::Write,
    path::{Path, PathBuf},
    rc::{Rc, Weak},
    sync::{Arc, LazyLock},
};

/// JavaScript's `\s`: white space and line terminators, U+FEFF included.
static SPACE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"[\t\n\x0B\x0C\r \x{a0}\x{1680}\x{2000}-\x{200a}\x{2028}\x{2029}\x{202f}\x{205f}\x{3000}\x{feff}]+").unwrap()
});

/// Logic Pro's own names that hold one of Live's words (its Live Loops, Session Players, Main Window, Arrangement
/// track, Clip Distortion), and a signal that clips: taken out before Live's words are looked for.
static LOGIC_OWN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)\bLive Loops?\b|\bSession (?:Players?|Drummer|Bass|Keyboardist)\b|\bMain Window\b|\bArrangement (?:track|markers?|global track)\b|\bClip Distortion\b|(?:n't|\bnot|\bto|\bwill|\bwould|\bcan|\bcould|\bmay|\bmight|\bit|\bthat|\bmix|\bsignal|\boutput|\bpeaks?)\s+clips?\b",
    )
    .unwrap()
});

/// Live's words, which Kumi over Logic Pro never says: the leak guards' list (fake_daw_session.rs), for what a model
/// writes (a clip as a thing, "Set" only as Live's name for a project), and Live's own parts besides.
static LIVE_WORDS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"\b(?:Ableton|Live|Session|Arrangement|Remote Script|Control Surface|Drum Rack|Operator|Simpler|[Rr]acks?|[Cc]lip slots?|[Cc]lips?|[Ss]cenes?|Main (?:track|channel|output)|[Mm]aster track|[Rr]eturn tracks?|(?:the|your|this|a|my|whole|Live) Sets?)\b",
    )
    .unwrap()
});

/// What Kumi says when it can't do something.
const CANT: &str = r"can['’]?t|cannot|couldn['’]?t|can not|isn['’]?t able|not able|unable|no way|doesn['’]?t (?:let|offer|have|give)|don['’]?t have|isn['’]?t (?:something|possible)|not possible|no tool|out of (?:my|Kumi['’]s) reach";

// ---------------------------------------------------------------------------------------------------
// Apple's Logic Pro User Guide, from the fixture.

#[derive(Deserialize)]
struct GuideFixture {
    base: String,
    topics: Vec<Topic>,
}

/// One of the guide's topics: its id and title as the contents list them, the parts and chapters it's in, its
/// introduction, and its sections (a task or subhead each, with its own id).
#[derive(Deserialize)]
struct Topic {
    id: String,
    title: String,
    #[serde(rename = "in")]
    within: Vec<String>,
    intro: Vec<String>,
    sections: Vec<TopicSection>,
}

#[derive(Deserialize)]
struct TopicSection {
    id: String,
    title: String,
    #[serde(default)]
    text: Vec<String>,
    /// A table's rows, its head first.
    #[serde(default)]
    table: Vec<Vec<String>>,
}

static GUIDE: LazyLock<GuideFixture> =
    LazyLock::new(|| serde_json::from_str(include_str!("fixtures/eval_logic/logic-guide.json")).expect("logic-guide.json"));

fn escape(text: &str) -> String {
    text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;").replace('\'', "&#39;")
}

/// A title as the guide's addresses write it: "Track Stacks overview" → "track-stacks-overview".
fn slug(title: &str) -> String {
    let dashed: String = title.to_lowercase().chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect();
    dashed.split('-').filter(|part| !part.is_empty()).collect::<Vec<_>>().join("-")
}

/// A topic's address, as its contents link it.
fn topic_address(topic: &Topic) -> String {
    format!("{}{}-{}/mac", GUIDE.base.trim_end_matches("welcome/mac"), slug(&topic.title), topic.id)
}

/// The fixture's topics as Apple's site lays the guide out, by address: the welcome page holds the contents (a `toc`
/// list, its parts and chapters `section` buttons over nested lists, each topic a `toc-item` link with its id in
/// `data-tocid`), and each topic is a page of its own, read without the site around it (`?displayMode=headless`): its
/// heading, its introduction, then each section under its heading in a `div` whose id is the section's.
fn guide_pages() -> BTreeMap<String, String> {
    let mut contents = String::from("<ul class='toc hasIcons'>");
    let mut open: Vec<&String> = vec![];
    for topic in &GUIDE.topics {
        let kept = open.iter().zip(&topic.within).take_while(|(open, within)| **open == *within).count();
        contents.push_str(&"</ul></li>".repeat(open.len() - kept));
        open.truncate(kept);
        for part in &topic.within[kept..] {
            contents.push_str(&format!(
                "<li><button class='section' aria-expanded='false'><span class='name'>{}</span></button><ul class='non-leaf-node-children'>",
                escape(part)
            ));
            open.push(part);
        }
        contents.push_str(&format!(
            "<li><a class='toc-item' href='{}' data-tocid='{}'><span class='name'>{}</span></a></li>",
            topic_address(topic),
            topic.id.to_uppercase(),
            escape(&topic.title)
        ));
    }
    contents.push_str(&"</ul></li>".repeat(open.len()));
    contents.push_str("</ul>");
    let welcome = format!(
        "<!DOCTYPE html><html lang='en'><head><title>Logic Pro User Guide for Mac</title></head><body><div class='main'><h1>Logic Pro User Guide</h1></div><div id='toc-hidden-content'><h2>Logic Pro User Guide for Mac</h2>{contents}</div></body></html>"
    );
    let mut pages = BTreeMap::from([(GUIDE.base.clone(), welcome)]);
    for topic in &GUIDE.topics {
        let paragraphs = |text: &[String]| text.iter().map(|line| format!("<p>{}</p>", escape(line))).collect::<String>();
        let mut body = format!(
            "<body dir='ltr' id='{id}' class='AppleTopic apd-topic' lang='en'><a name='{id}'></a><h1>{} in Logic Pro for Mac</h1>{}",
            escape(&topic.title),
            paragraphs(&topic.intro),
            id = topic.id
        );
        for section in &topic.sections {
            body.push_str(&format!(
                "<div id='{}' class='Task'><h2 class='Name'>{}</h2>{}",
                section.id,
                escape(&section.title),
                paragraphs(&section.text)
            ));
            if let Some((top, rows)) = section.table.split_first() {
                let cells = |row: &[String], cell: &str| {
                    row.iter().map(|text| format!("<{cell}><p>{}</p></{cell}>", escape(text))).collect::<String>()
                };
                body.push_str(&format!(
                    "<div class='table-wrapper'><table><thead><tr>{}</tr></thead><tbody>{}</tbody></table></div>",
                    cells(top, "th"),
                    rows.iter().map(|row| format!("<tr>{}</tr>", cells(row, "td"))).collect::<String>()
                ));
            }
            body.push_str("</div>");
        }
        body.push_str("</body>");
        let page = format!("<head><meta name='robots' content='noindex' /></head><div class='main'>{body}</div>");
        pages.insert(format!("{}?displayMode=headless", topic_address(topic)), page);
    }
    pages
}

/// Apple's site as the eval answers it: the fixture's pages, and a 404 for any other address. Nothing is fetched.
struct Guide {
    pages: BTreeMap<String, String>,
    /// Every address read, in order.
    read: RefCell<Vec<String>>,
}

impl Guide {
    fn new() -> Rc<Self> {
        Rc::new(Self { pages: guide_pages(), read: RefCell::new(vec![]) })
    }

    /// The addresses read that the fixture lacks (answered 404).
    fn missed(&self) -> Vec<String> {
        self.read.borrow().iter().filter(|address| !self.pages.contains_key(*address)).cloned().collect()
    }

    /// The topics read, by their titles.
    fn topics_read(&self) -> Vec<String> {
        let read = self.read.borrow();
        GUIDE
            .topics
            .iter()
            .filter(|topic| read.iter().any(|address| address.starts_with(&topic_address(topic))))
            .map(|topic| topic.title.clone())
            .collect()
    }
}

#[async_trait(?Send)]
impl WebClient for Guide {
    async fn fetch(&self, url: &str, request: WebRequest) -> Result<WebResponse, WebFailure> {
        if request.signal.as_ref().is_some_and(|signal| signal.is_cancelled()) {
            return Err(WebFailure::Aborted);
        }
        self.read.borrow_mut().push(url.to_owned());
        let page = self.pages.get(url);
        Ok(WebResponse {
            url: url.to_owned(),
            status: if page.is_some() { 200 } else { 404 },
            headers: BTreeMap::new(),
            content_type: "text/html".into(),
            charset: Some("utf-8".into()),
            body: page.cloned().unwrap_or_default().into_bytes(),
            truncated: false,
            skipped: false,
        })
    }
}

// ---------------------------------------------------------------------------------------------------
// The simulated Logic, behind Kumi's host in process.

/// Kumi's host (daw-mcp-server's own code) as an MCP endpoint called in process, as the runtime's tests call it: what
/// the host answers from the simulated Logic is what Logic's integration reads.
struct InProcess {
    host: Arc<Host>,
    id: Cell<u64>,
}

#[async_trait(?Send)]
impl McpEndpoint for InProcess {
    fn pid(&self) -> Option<u32> {
        None
    }
    fn server_info(&self) -> Option<Implementation> {
        None
    }
    async fn list(&self, _: Option<&str>, _: Signal) -> Result<ListToolsResult, RuntimeError> {
        serde_json::from_value(json!({"tools": daw_mcp_server::mcp::tools(&self.host)})).map_err(failure)
    }
    /// As the real client does: a call whose signal fires fails at once, and the host carries on with it.
    async fn call(&self, name: &str, args: JsonObject, signal: Signal) -> Result<CallToolResult, RuntimeError> {
        self.id.set(self.id.get() + 1);
        let message = json!({"jsonrpc": "2.0", "id": self.id.get(), "method": "tools/call", "params": {"name": name, "arguments": args}});
        let host = self.host.clone();
        let handled = tokio::task::spawn_local(async move { daw_mcp_server::mcp::handle(&host, &message).await });
        tokio::select! {
            biased;
            _ = signal.cancelled() => Err(RuntimeError::plain("MCP request cancelled")),
            answer = handled => {
                let answer = answer.map_err(failure)?.ok_or_else(|| RuntimeError::plain("Kumi's host gave no answer"))?;
                serde_json::from_value(answer["result"].clone()).map_err(|error| RuntimeError::plain(format!("{answer}: {error}")))
            }
        }
    }
    fn on_catalog_changed(&self, _: Rc<dyn Fn()>) -> Box<dyn Fn()> {
        Box::new(|| {})
    }
    fn on_disconnect(&self, _: Rc<dyn Fn()>) -> Box<dyn Fn()> {
        Box::new(|| {})
    }
    /// The bridge's events as the host forwards them.
    fn on_live_event(&self, listener: Rc<dyn Fn(JsonObject)>) -> Box<dyn Fn()> {
        let mut events = self.host.events();
        let task = tokio::task::spawn_local(async move {
            while let Ok(event) = events.recv().await {
                if let Some(event) = event.as_object() {
                    listener(event.clone());
                }
            }
        });
        Box::new(move || task.abort())
    }
    fn stderr_status(&self) -> StderrStatus {
        StderrStatus { bytes: 0, truncated: false }
    }
    async fn close(&self) -> Result<(), RuntimeError> {
        self.host.close().await;
        Ok(())
    }
}

/// One case's world: a throwaway folder (Kumi's home, memory, recipes, library), the simulated Logic on Scratch as
/// recorded, and Kumi's host in front of it, which finds it as it finds the bridge beside Logic.
struct Lab {
    folder: tempfile::TempDir,
    script: LogicScript,
    simulator: Simulator,
    host: Rc<InProcess>,
}

impl Lab {
    async fn open() -> Result<Self, RuntimeError> {
        let folder = tempfile::Builder::new().prefix("kumi-eval-logic-").tempdir().map_err(failure)?;
        let home = folder.path().join("kumi");
        let paths = BridgePaths::new(&home, Daw::Logic);
        let secret = ensure_secret(&paths.secret()).map_err(failure)?;
        let script = LogicScript::new();
        let mut config = script.config(secret, "com.apple.logic10");
        config.discovery_dir = Some(paths.discovery());
        let simulator = Simulator::start(config).await.map_err(failure)?;
        let host = Rc::new(InProcess { host: Arc::new(Host::new(Daw::Logic, &home)), id: Cell::new(0) });
        Ok(Self { folder, script, simulator, host })
    }

    fn place(&self, name: &str) -> PathBuf {
        self.folder.path().join(name)
    }
}

// ---------------------------------------------------------------------------------------------------
// The model: the configured one, or the dry run's canned answers.

/// A model with its calls counted: `do_stream` is the one call Kumi makes.
struct Counting {
    model: Rc<dyn LanguageModel>,
    calls: Cell<usize>,
    /// Tokens over every call: in (cache reads among them) and out, as the provider counted them.
    tokens: Rc<Cell<(f64, f64, f64)>>,
}

#[async_trait(?Send)]
impl LanguageModel for Counting {
    async fn do_stream(&self, options: CallOptions) -> Result<StreamParts, LanguageModelError> {
        self.calls.set(self.calls.get() + 1);
        let parts = self.model.do_stream(options).await?;
        let tokens = self.tokens.clone();
        Ok(Box::pin(parts.inspect(move |part| {
            if let StreamPart::Finish { usage, .. } = part {
                let (input, cached, output) = tokens.get();
                tokens.set((
                    input + usage.input_tokens.total.unwrap_or(0.),
                    cached + usage.input_tokens.cache_read.unwrap_or(0.),
                    output + usage.output_tokens.total.unwrap_or(0.),
                ));
            }
        })))
    }
}

/// A model's binding, resolved once for every kernel of a run, with its calls counted and its requests measured.
#[derive(Clone)]
struct Counted {
    binding: Rc<ModelBinding>,
    counting: Rc<Counting>,
    /// The most bytes a request's instructions and tool definitions took.
    fixed: Rc<Cell<usize>>,
}

impl Counted {
    fn new(binding: ModelBinding) -> Self {
        let counting = Rc::new(Counting { model: binding.model.clone(), calls: Cell::new(0), tokens: Rc::new(Cell::new((0., 0., 0.))) });
        Self { binding: Rc::new(binding), counting, fixed: Rc::new(Cell::new(0)) }
    }

    /// The binding with its model's calls counted, for one kernel.
    fn binding(&self) -> ModelBinding {
        let prepare = self.binding.clone();
        let budget = self.binding.clone();
        let fixed = self.fixed.clone();
        ModelBinding {
            id: self.binding.id.clone(),
            model: self.counting.clone(),
            prepare: Box::new(move |request: ModelRequest| {
                let bytes = request.instructions.len() + stringify(&serde_json::to_value(&request.tools).unwrap_or_default()).len();
                fixed.set(fixed.get().max(bytes));
                (prepare.prepare)(request)
            }),
            budget: self.binding.budget.as_ref().map(|_| Box::new(move |fixed| (budget.budget.as_ref().unwrap())(fixed)) as Box<_>),
        }
    }
}

/// One reply of the dry run's model.
pub enum Reply {
    /// Calls to Kumi's tools, made together: each a tool's name and its input.
    Calls(Vec<(&'static str, Value)>),
    /// What Kumi says, which ends the turn.
    Says(&'static str),
}

pub fn call(name: &'static str, input: Value) -> Reply {
    Reply::Calls(vec![(name, input)])
}

pub fn says(text: &'static str) -> Reply {
    Reply::Says(text)
}

/// One plan of changes through make_changes, final: Kumi says what changed and the model isn't asked again.
pub fn plan(steps: Value) -> Reply {
    call("make_changes", json!({"steps": steps, "final": true}))
}

/// The dry run's model: a case's canned replies, one a request.
struct Canned {
    replies: RefCell<VecDeque<Reply>>,
    next: Cell<usize>,
}

fn finish(unified: FinishReasonUnified) -> StreamPart {
    let raw = serde_json::to_value(unified).ok().and_then(|value| value.as_str().map(str::to_owned));
    let usage = ModelUsage {
        input_tokens: InputTokens { total: Some(0.0), no_cache: Some(0.0), cache_read: Some(0.0), cache_write: Some(0.0) },
        output_tokens: OutputTokens { total: Some(0.0), text: Some(0.0), reasoning: Some(0.0) },
        raw: None,
    };
    StreamPart::Finish { usage, finish_reason: FinishReason { unified, raw }, provider_metadata: None }
}

#[async_trait(?Send)]
impl LanguageModel for Canned {
    async fn do_stream(&self, _: CallOptions) -> Result<StreamParts, LanguageModelError> {
        let reply = self.replies.borrow_mut().pop_front().unwrap_or(Reply::Says("(The canned answer has no reply left.)"));
        let parts = match reply {
            Reply::Calls(calls) => {
                let mut parts: Vec<StreamPart> = calls
                    .into_iter()
                    .map(|(name, input)| {
                        self.next.set(self.next.get() + 1);
                        StreamPart::ToolCall(ToolCall {
                            tool_call_id: format!("canned_{}", self.next.get()),
                            tool_name: name.into(),
                            input: input.to_string(),
                            provider_executed: None,
                            dynamic: None,
                            provider_metadata: None,
                        })
                    })
                    .collect();
                parts.push(finish(FinishReasonUnified::ToolCalls));
                parts
            }
            Reply::Says(text) => vec![
                StreamPart::TextStart { id: "t".into(), provider_metadata: None },
                StreamPart::TextDelta { id: "t".into(), delta: text.into(), provider_metadata: None },
                StreamPart::TextEnd { id: "t".into(), provider_metadata: None },
                finish(FinishReasonUnified::Stop),
            ],
        };
        Ok(futures::stream::iter(parts).boxed_local())
    }
}

/// A case's canned replies as a model binding, counted.
fn canned(replies: Vec<Reply>) -> Counted {
    Counted::new(ModelBinding {
        id: "canned/dry-run".into(),
        model: Rc::new(Canned { replies: RefCell::new(replies.into()), next: Cell::new(0) }),
        prepare: Box::new(|request: ModelRequest| CallOptions {
            prompt: request.messages,
            tools: (!request.tools.is_empty()).then_some(request.tools),
            ..CallOptions::default()
        }),
        budget: None,
    })
}

// ---------------------------------------------------------------------------------------------------
// The cases.

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// A question about Logic's terms and ways.
    Probe,
    /// A change to Scratch.
    Change,
    /// What Kumi's bridge can't do in Logic: refused saying why, or a way round said as one.
    Limit,
    /// A request that needs ears.
    Listen,
}

/// What a case's check found wanting: nothing when it passed.
type Check = Box<dyn Fn(&Run<'_>) -> Vec<String>>;

/// One request to the model and what should come of it.
pub struct Case {
    pub name: &'static str,
    pub kind: Kind,
    /// The producer's words; "{sample}" stands for a short WAV file of the case's own.
    pub prompt: &'static str,
    /// What the check looks for, in words (--list).
    pub looks_for: String,
    check: Check,
    /// The dry run's model: a good answer's replies in turn.
    canned: fn() -> Vec<Reply>,
}

/// What a case's check looks at: the simulated Logic after the conversation, every request Kumi's host sent it, HISTORY,
/// and what Kumi ran, read and said.
pub struct Run<'a> {
    script: &'a LogicScript,
    /// Every request the simulated Logic answered (method and params), in order.
    asked: &'a [(String, Value)],
    /// HISTORY's rows, the last word on each.
    changes: &'a [ChangeRecord],
    /// The tools Kumi ran, in order (a tool call_tool ran by its own name).
    tools: &'a [String],
    /// What Kumi said.
    answer: &'a str,
    /// What logic_manual answered, each time.
    manual: &'a [String],
    heard: &'a [HeardEvent],
}

impl Run<'_> {
    /// The kinds of change Logic applied, in order.
    fn applied(&self) -> Vec<&str> {
        self.asked
            .iter()
            .filter(|(method, _)| method == "change_apply")
            .filter_map(|(_, params)| params.get("kind").and_then(Value::as_str))
            .collect()
    }

    /// Whatever Kumi did to the project: each change applied by its kind, playback started, the playhead moved, a save.
    fn touched(&self) -> Vec<String> {
        self.asked
            .iter()
            .filter_map(|(method, params)| match method.as_str() {
                "change_apply" => Some(params.get("kind").and_then(Value::as_str).unwrap_or("a change").to_owned()),
                "transport_play" | "cursor_set" | "project_save" | "bounce" | "export_tracks" => Some(method.clone()),
                _ => None,
            })
            .collect()
    }

    /// Whether what Kumi said matches `pattern` (a regular expression, in any case).
    fn mentions(&self, pattern: &str) -> bool {
        Regex::new(&format!("(?i){pattern}")).is_ok_and(|pattern| pattern.is_match(self.answer))
    }

    /// Live's words in what Kumi said and in HISTORY's rows.
    fn live_words(&self) -> Vec<String> {
        let mut said = vec![self.answer.to_owned()];
        said.extend(self.changes.iter().map(|change| format!("{} {}", change.title, change.note.clone().unwrap_or_default())));
        said.iter()
            .flat_map(|text| {
                let text = LOGIC_OWN.replace_all(text, " ");
                LIVE_WORDS.find_iter(&text).map(|word| word.as_str().to_owned()).collect::<Vec<_>>()
            })
            .collect()
    }

    /// The rows of HISTORY that were applied.
    fn applied_rows(&self) -> usize {
        self.changes.iter().filter(|change| change.state == ChangeState::Applied).count()
    }
}

/// Wanting `why` unless `ok`.
fn need(found: &mut Vec<String>, ok: bool, why: impl Into<String>) {
    if !ok {
        found.push(why.into());
    }
}

/// In Logic's words: none of Live's in what Kumi said or in HISTORY.
fn logic_words(run: &Run<'_>, found: &mut Vec<String>) {
    let live = run.live_words();
    need(found, live.is_empty(), format!("Live's words: {}", live.join(", ")));
}

/// A question: nothing in the project changed, and HISTORY holds no change (an audition's row, heard, is none).
fn unchanged(run: &Run<'_>, found: &mut Vec<String>) {
    let touched = run.touched();
    let rows: Vec<&str> =
        run.changes.iter().filter(|change| change.state != ChangeState::Heard).map(|change| change.title.as_str()).collect();
    need(
        found,
        touched.is_empty() && rows.is_empty(),
        format!("changed the project for a question: {}", touched.iter().map(String::as_str).chain(rows).collect::<Vec<_>>().join(", ")),
    );
}

/// Apple's guide asked (logic_manual), and a topic it gave cited by its title beside the guide's name.
fn cites_the_guide(run: &Run<'_>, found: &mut Vec<String>) {
    if !run.tools.iter().any(|tool| tool == "logic_manual") {
        return found.push("didn't ask logic_manual".into());
    }
    let answer = run.answer.to_lowercase();
    // (each passage the guide gives names its topic's address)
    let given: Vec<&Topic> =
        GUIDE.topics.iter().filter(|topic| run.manual.iter().any(|said| said.contains(&topic_address(topic)))).collect();
    let cited = given.iter().any(|topic| answer.contains(&topic.title.to_lowercase()));
    need(
        found,
        answer.contains("guide") && cited,
        format!(
            "doesn't cite the guide by a topic it gave ({})",
            given.iter().map(|topic| topic.title.as_str()).collect::<Vec<_>>().join(", ")
        ),
    );
}

/// The changes Logic applied are these kinds, each once, and HISTORY holds a row applied for each.
fn made(run: &Run<'_>, kinds: &[&str], found: &mut Vec<String>) {
    let mut applied = run.applied();
    applied.sort_unstable();
    let mut wanted = kinds.to_vec();
    wanted.sort_unstable();
    need(found, applied == wanted, format!("Logic applied [{}], not [{}]", applied.join(", "), wanted.join(", ")));
    need(found, run.applied_rows() == kinds.len(), format!("HISTORY holds {} rows applied, not {}", run.applied_rows(), kinds.len()));
}

/// Said it can't, and why (one of `why`, a regular expression).
fn refused(run: &Run<'_>, why: &str, found: &mut Vec<String>) {
    need(found, run.mentions(CANT), "doesn't say it can't");
    need(found, run.mentions(why), format!("doesn't say why (/{why}/)"));
}

/// Logic's instruction pack, as every turn over Logic Pro carries it.
const PACK: &str = kumi_runtime::integrations::logic::DAW.instructions;

/// What a probe's answer rests on. (M7) Apple's guide is required only where Logic's instruction pack doesn't state the
/// answer: the first run with a model answered six probes right from the pack, and failed them only for not asking it.
#[derive(Clone, Copy)]
pub enum Source {
    /// Logic's pack, which states the answer in these words (the check fails once it no longer does): asking the guide
    /// isn't required.
    Pack(&'static str),
    /// Apple's guide, since the pack doesn't state the answer: Kumi asks logic_manual and cites a topic it gave.
    Guide,
    /// What Kumi reads of the project.
    Project,
}

/// A question about Logic Pro: what Kumi says matches each of `says` (in any case), in Logic's words and none of
/// Live's; with the guide as its source, Kumi asked Apple's guide and cites a topic it gave; and nothing in the project
/// changes.
fn probe(name: &'static str, prompt: &'static str, says: &'static [&'static str], source: Source, canned: fn() -> Vec<Reply>) -> Case {
    let looks_for = format!(
        "says {}{}; changes nothing; none of Live's words",
        says.iter().map(|pattern| format!("/{pattern}/")).collect::<Vec<_>>().join(" and "),
        match source {
            Source::Guide => "; asks logic_manual and cites a topic it gave".to_owned(),
            Source::Pack(words) => format!(" (Logic's pack says “{words}”, so the guide isn't required)"),
            Source::Project => String::new(),
        }
    );
    Case {
        name,
        kind: Kind::Probe,
        prompt,
        looks_for,
        check: Box::new(move |run| {
            let mut found = vec![];
            for pattern in says {
                need(&mut found, run.mentions(pattern), format!("doesn't say /{pattern}/"));
            }
            match source {
                Source::Guide => cites_the_guide(run, &mut found),
                Source::Pack(words) => need(
                    &mut found,
                    PACK.contains(words),
                    format!("Logic's pack no longer says “{words}”: make the guide this probe's source"),
                ),
                Source::Project => {}
            }
            unchanged(run, &mut found);
            logic_words(run, &mut found);
            found
        }),
        canned,
    }
}

/// Any other case: its check, and what it looks for in words.
fn case(
    name: &'static str,
    kind: Kind,
    prompt: &'static str,
    looks_for: &str,
    check: fn(&Run<'_>, &mut Vec<String>),
    canned: fn() -> Vec<Reply>,
) -> Case {
    Case {
        name,
        kind,
        prompt,
        looks_for: format!("{looks_for}; none of Live's words"),
        check: Box::new(move |run| {
            let mut found = vec![];
            check(run, &mut found);
            logic_words(run, &mut found);
            found
        }),
        canned,
    }
}

/// A track's row field, as Logic's read shows it now (`index` from 1).
fn field(script: &LogicScript, index: usize, key: &str) -> Value {
    script.track(index).get(key).cloned().unwrap_or(Value::Null)
}

pub fn every_case() -> Vec<Case> {
    vec![
        // Knowledge probes: Logic's terms and ways. (M7) Apple's guide is required, asked and cited, only where Logic's
        // instruction pack doesn't state the answer; where it does, the words it says it in are the probe's source.
        probe("probe: a region, not a clip", "What's on the Keys track, and how long is it?", &["region"], Source::Project, || {
            vec![
                call("logic_read", json!({"kind": "region", "parent": "track:2"})),
                says("The Keys track holds one MIDI region, “Keys”: it starts at bar 1 and runs 16 beats, to bar 5, playing four notes."),
            ]
        }),
        probe(
            "probe: the Stereo Out",
            "Where would a limiter go in Logic Pro to work on the whole mix?",
            &["Stereo Out"],
            Source::Pack("its plug-ins act on the whole mix, so a Gain or a limiter on everything goes there"),
            || {
                vec![says(
                    "On the Stereo Out, the project's output channel strip: both your tracks go to Stereo Output, so a limiter in its insert slots works on the whole mix. Say if you want me to put one there.",
                )]
            },
        ),
        probe(
            "probe: Track Stacks",
            "What's the difference between a folder stack and a summing stack?",
            &["summing", "folder", r"\baux|\bbus|submix"],
            Source::Pack("a summing stack mixes them through a bus, a folder stack only groups them"),
            || {
                vec![
                    call("find_tools", json!({"names": ["logic_manual"]})),
                    call("call_tool", json!({"name": "logic_manual", "input": {"question": "folder stack and summing stack difference"}})),
                    says(
                        "Both are Track Stacks, tracks gathered under a main track. A folder stack only groups them (you hide, mute or solo them together, and each keeps its own output); a summing stack sends its tracks to an aux, so they're mixed and processed together as one submix (Logic Pro User Guide, Track Stacks overview).",
                    ),
                ]
            },
        ),
        probe(
            "probe: the Marker, Tempo and Signature Lists",
            "Where does Logic Pro keep the project's markers, tempo changes and time signatures?",
            &[r"Marker List|Marker,? (?:and )?Tempo", r"Tempo List|Tempo,? (?:and )?Signature Lists?", r"Signature Lists?"],
            Source::Pack("Markers, tempo changes and signatures live in Logic Pro's Marker, Tempo and Signature Lists"),
            || {
                vec![
                    call("logic_manual", json!({"question": "list of markers, tempo changes and time signatures"})),
                    says(
                        "In three lists: the Marker List, the Tempo List and the Signature List. Press D to open the List Editors, then click Marker, Tempo or Signature (Logic Pro User Guide, Marker List; Tempo List; Signature List).",
                    ),
                ]
            },
        ),
        probe(
            "probe: key commands act on the selection",
            "If I press Command-T in Logic Pro, what gets split, and where?",
            &["select", "playhead", "region"],
            // The pack says a key command acts on the selection, but not what Command-T runs or that it splits at the
            // playhead: the guide's key command tables say it.
            Source::Guide,
            || {
                vec![
                    call("logic_manual", json!({"question": "Command-T split regions at playhead key command"})),
                    says(
                        "Command-T is Split Regions/Events at Playhead: it splits the selected regions where the playhead is. Like most key commands it acts on what's selected, so with no region selected it splits nothing (Logic Pro User Guide, Tracks area key commands).",
                    ),
                ]
            },
        ),
        probe(
            "probe: what Logic's undo keeps no step of",
            "Which of my changes can't Logic Pro's own undo take back?",
            &[r"fader|volume", r"\bpan\b|\bmute|\bsolo", r"plug-in|parameter|tempo|Smart Control"],
            Source::Pack("A fader, pan, mute, solo, arm, bypass, the tempo, the cycle, a plug-in's parameter or setting, a Smart Control and a send's level make no step of Logic Pro's own undo"),
            || {
                vec![
                    call("logic_manual", json!({"question": "what the undo history doesn't keep"})),
                    says(
                        "Logic Pro's undo history keeps edits (regions, tracks, markers, the lists, plug-ins inserted or removed) but not a channel strip's fader, pan, mute or solo, a plug-in's bypass or parameters, Smart Controls, a send's level, the cycle area or the tempo set in the control bar (Logic Pro User Guide, Undo and redo edits). Kumi's undo takes back any of those Kumi changed, from HISTORY.",
                    ),
                ]
            },
        ),
        probe(
            "probe: the Controls View",
            "How can I see all of a plug-in's parameters as plain sliders in Logic Pro?",
            &["Controls"],
            Source::Pack("A plug-in's parameters are reached only in its window's Controls View"),
            || {
                vec![
                    call("logic_manual", json!({"question": "plug-in window Controls view parameters as sliders"})),
                    says(
                        "Open the plug-in's window and choose Controls from the View pop-up menu in its header: every parameter shows as a slider, checkbox or pop-up menu with its name and value. Choose Editor to go back (Logic Pro User Guide, Plug-in window overview).",
                    ),
                ]
            },
        ),
        probe(
            "probe: quantize is a region parameter",
            "If I quantize a region in the Region inspector, are my notes moved for good?",
            &[r"Region inspector|parameter", r"\bkeep|recorded|original|non-?destructive|\bOff\b"],
            Source::Pack("Quantize is a parameter of a region's events in Logic Pro: they play on the grid and keep their positions underneath"),
            || {
                vec![
                    call("logic_manual", json!({"question": "Region inspector Quantize notes positions"})),
                    says(
                        "No: in the Region inspector, Quantize is a parameter of the whole region. Its notes play on the grid you choose but keep the positions they were recorded at, so choosing Off brings them back; only applying the quantization permanently rewrites them (Logic Pro User Guide, Quantize MIDI regions).",
                    ),
                ]
            },
        ),
        probe(
            "probe: an import's own track",
            "If I import a MIDI file, where does it end up in Logic Pro?",
            &[r"new (?:[\w-]+ ){0,3}track", "region"],
            Source::Pack("A MIDI file imported lands on a new track of its own"),
            || {
                vec![
                    call("logic_manual", json!({"question": "import a MIDI file"})),
                    says(
                        "File > Import > MIDI File puts the file's events in a new MIDI region on a new software instrument track Logic Pro makes for them, at the playhead. Dragging the file onto a track's lane puts it on that track instead (Logic Pro User Guide, Import MIDI files).",
                    ),
                ]
            },
        ),
        Case {
            looks_for: "says “Smart Control” and names two or more of the Keys' (Tone, Low Cut, Comp, Ratio); changes nothing; none of Live's words".into(),
            check: Box::new(|run| {
                let mut found = vec![];
                need(&mut found, run.mentions("Smart Control"), "doesn't say “Smart Control”");
                // (whole names: Comp isn't Compressor)
                let named = ["Tone", "Low Cut", "Comp", "Ratio"]
                    .iter()
                    .filter(|name| Regex::new(&format!(r"\b{name}\b")).is_ok_and(|name| name.is_match(run.answer)))
                    .count();
                need(&mut found, named >= 2, format!("names {named} of the Keys' Smart Controls"));
                unchanged(run, &mut found);
                logic_words(run, &mut found);
                found
            }),
            ..probe("probe: Smart Controls", "What do the Smart Controls on the Keys track do?", &[], Source::Project, || {
                vec![
                    call("logic_read", json!({"kind": "smart", "parent": "track:2"})),
                    says(
                        "The Keys play the Studio Grand patch, whose Smart Controls are Tone and Low Cut (the piano's colour and its low end, both at 0), Comp (how hard its Compressor works, at 0.38) and Ratio (that Compressor's ratio, at 0.4).",
                    ),
                ]
            })
        },
        // Changes to Scratch: the right kinds reach Logic, HISTORY holds their rows, and Kumi says it in Logic's words.
        case(
            "change: a fader",
            Kind::Change,
            "Bring the Keys down 3 dB.",
            "Logic applies one track-volume, the Keys at -3 dB and the Vocal as it was; HISTORY holds its row",
            |run, found| {
                made(run, &["track-volume"], found);
                need(found, (run.script.volume(2) + 3.0).abs() < 0.11, format!("the Keys are at {} dB", run.script.volume(2)));
                need(found, run.script.volume(1) == 0.0, "the Vocal's fader moved");
            },
            || vec![plan(json!([{"tool": "track_volume", "input": {"ref": "track:2", "volumeDb": -3}}]))],
        ),
        case(
            "change: a rename",
            Kind::Change,
            "Rename the Vocal track to Lead Vox.",
            "Logic applies one track-rename, the first track named Lead Vox; HISTORY holds its row",
            |run, found| {
                made(run, &["track-rename"], found);
                need(found, field(run.script, 1, "name") == "Lead Vox", format!("the first track is {}", field(run.script, 1, "name")));
            },
            || vec![plan(json!([{"tool": "track_rename", "input": {"ref": "track:1", "name": "Lead Vox"}}]))],
        ),
        case(
            "change: a marker",
            Kind::Change,
            "Put a marker at bar 5.",
            "Logic applies one marker-add, a marker at bar 5 (16 beats) beside Marker 1 at bar 9; HISTORY holds its row",
            |run, found| {
                made(run, &["marker-add"], found);
                let markers = run.script.markers();
                need(
                    found,
                    markers.len() == 2 && markers.iter().any(|(_, beats)| *beats == 16.0) && markers.iter().any(|(_, beats)| *beats == 32.0),
                    format!("the markers are {markers:?}"),
                );
            },
            || vec![plan(json!([{"tool": "marker_add", "input": {"positionBeats": 16}}]))],
        ),
        case(
            "change: a tempo change",
            Kind::Change,
            "Make the tempo change to 140 BPM at bar 9.",
            "Logic applies one tempo-marker-set, the Tempo List 160 BPM from the start and 140 from bar 9 (32 beats); HISTORY holds its row",
            |run, found| {
                made(run, &["tempo-marker-set"], found);
                let tempos = run.script.tempos();
                need(found, tempos == [(0.0, 160.0), (32.0, 140.0)], format!("the Tempo List is {tempos:?}"));
            },
            || vec![plan(json!([{"tool": "tempo_marker_set", "input": {"positionBeats": 32, "bpm": 140}}]))],
        ),
        case(
            "change: a Gain on the Stereo Out",
            Kind::Change,
            "Put a Gain plug-in on the Stereo Out and turn it down 2 dB.",
            "Logic applies an fx-add and an fx-param, the Stereo Out holding a Gain at -2.0 dB; HISTORY holds their rows",
            |run, found| {
                made(run, &["fx-add", "fx-param"], found);
                let plugins = run.script.plugins("output:1");
                need(found, plugins == [("Gain".to_string(), false)], format!("the Stereo Out holds {plugins:?}"));
                let gain = run.script.param("output:1", 1, "Gain").map(|(_, text)| text).unwrap_or_default();
                need(found, gain.starts_with("-2.0"), format!("its Gain reads {gain:?}"));
            },
            || {
                vec![plan(json!([
                    {"tool": "fx_add", "input": {"ref": "output:1", "name": "Gain"}, "as": "gain"},
                    {"tool": "fx_param", "input": {"ref": "@gain", "param": "Gain", "value": -2}}
                ]))]
            },
        ),
        case(
            "change: a send",
            Kind::Change,
            "Send the Vocal to Bus 1 at -12 dB.",
            "Logic applies one send-add, the Vocal sending to Bus 1 at -12 dB; HISTORY holds its row",
            |run, found| {
                made(run, &["send-add"], found);
                let sends = run.script.sends(1);
                need(found, sends == [("Bus 1".to_string(), -12.0)], format!("the Vocal's sends are {sends:?}"));
            },
            || vec![plan(json!([{"tool": "send_add", "input": {"ref": "track:1", "bus": 1, "volumeDb": -12}}]))],
        ),
        case(
            "change: a plan of several changes",
            Kind::Change,
            "On the Keys: pan it a little to the left, mute it, and rename it Piano.",
            "one make_changes; Logic applies a track-pan, a track-mute and a track-rename, the second track panned left, muted and named Piano; HISTORY holds their rows",
            |run, found| {
                need(found, run.tools.iter().any(|tool| tool == "make_changes"), "no make_changes");
                made(run, &["track-mute", "track-pan", "track-rename"], found);
                let keys = run.script.track(2);
                need(
                    found,
                    keys["pan"].as_f64().is_some_and(|pan| pan < 0.0) && keys["mute"] == true && keys["name"] == "Piano",
                    format!("the second track is {keys}"),
                );
            },
            || {
                vec![plan(json!([
                    {"tool": "track_pan", "input": {"ref": "track:2", "pan": -0.3}},
                    {"tool": "track_mute", "input": {"ref": "track:2", "mute": true}},
                    {"tool": "track_rename", "input": {"ref": "track:2", "name": "Piano"}}
                ]))]
            },
        ),
        // The limits: what Kumi's bridge can't do in Logic, refused saying why, and nothing else done in its place but a way
        // round said as one.
        case(
            "limit: a track moved",
            Kind::Limit,
            "Move the Keys track above the Vocal.",
            "says it can't and why (dragging, the headers, no key command); nothing changed, the tracks in their order",
            |run, found| {
                refused(run, r"drag|header|key command|reorder", found);
                unchanged(run, found);
                need(found, field(run.script, 1, "name") == "Vocal", "the tracks moved");
            },
            || {
                vec![says(
                    "Kumi can't move tracks in Logic Pro: Logic reorders tracks only by dragging their headers, and no key command does it. Drag the Keys header above the Vocal yourself; it plays the same either way.",
                )]
            },
        ),
        case(
            "limit: a track's colour",
            Kind::Limit,
            "Make the Vocal track red.",
            "says it can't and why (the Color palette, reading it back, Show Colors); nothing changed",
            |run, found| {
                refused(run, r"palette|read|Show Colors|Option-C|swatch|Accessibility", found);
                unchanged(run, found);
            },
            || {
                vec![says(
                    "Kumi can't colour tracks in Logic Pro: its Color palette doesn't tell Kumi which swatch is which, and the headers show no colour to read back. Select the Vocal, press Option-C (View > Show Colors) and click red.",
                )]
            },
        ),
        case(
            "limit: a send's mode",
            Kind::Limit,
            "Add a pre-fader send from the Vocal to Bus 2.",
            "says the send's mode (pre-fader) can't be chosen from outside, and why; at most the send itself made, to Bus 2",
            |run, found| {
                refused(run, r"mode|from outside|Pre Fader|send's menu|by hand|yourself", found);
                need(found, run.mentions(r"pre[- ]?fader"), "doesn't say pre-fader");
                let applied = run.applied();
                need(found, applied.iter().all(|kind| *kind == "send-add"), format!("Logic applied {applied:?}"));
                need(found, run.script.sends(1).iter().all(|(bus, _)| bus == "Bus 2"), format!("the Vocal's sends are {:?}", run.script.sends(1)));
            },
            || {
                vec![
                    call("make_changes", json!({"steps": [{"tool": "send_add", "input": {"ref": "track:1", "bus": 2, "volumeDb": -10}}]})),
                    says(
                        "I added a send from the Vocal to Bus 2 at -10 dB, but Kumi can't make it pre-fader: a send's mode can't be read or chosen from outside Logic Pro. Click the send on the Vocal's channel strip and choose Pre Fader.",
                    ),
                ]
            },
        ),
        case(
            "limit: a plug-in moved",
            Kind::Limit,
            "Move the Compressor on the Keys so it comes before the Channel EQ.",
            "says Logic Pro moves a plug-in only by dragging it; at most the take-it-off-and-add-again way, saying its settings go back to Logic's defaults (with some carried over to the new one, that the others do)",
            |run, found| {
                need(found, run.mentions("drag"), "doesn't say plug-ins move by dragging");
                let applied = run.applied();
                // (M7) Settings set on the plug-in added again carry the old one's over, which is part of that way:
                // fx_add's description asks Kumi to say which it carried over and that the others reset, or to ask first.
                // So an fx-param or fx-params is taken beside an fx-add, with the others said to reset. (The first run
                // carried 6 of 25 over and never said the rest reset.)
                let carried = applied.contains(&"fx-add") && applied.iter().any(|kind| kind.starts_with("fx-param"));
                need(
                    found,
                    applied.iter().all(|kind| matches!(*kind, "fx-delete" | "fx-add") || (carried && kind.starts_with("fx-param"))),
                    format!("Logic applied {applied:?}"),
                );
                if applied.contains(&"fx-delete") {
                    need(found, run.mentions(r"default|settings"), "took it off without saying its settings go");
                }
                if carried {
                    need(found, run.mentions(r"default|reset"), "carried settings over without saying the others reset");
                }
            },
            || {
                vec![says(
                    "Logic Pro moves a plug-in only by dragging it, which Kumi can't do. I could take the Compressor off and insert a new one above the Channel EQ, but it would come back with Logic Pro's default settings; drag it by its handle in the Mixer to keep yours, or say if I should.",
                )]
            },
        ),
        // (M7) Either a refusal or the way round is a good answer. Kumi's gap guidance says to take the way round first and
        // say what was done instead, and Logic's own Sampler on a new track (sample_load) is the nearest Kumi can load a
        // sample into: so the Sampler loaded passes when the answer names Quick Sampler, says Kumi can't load it and why
        // (it takes a sample only by a drag), and says what Kumi did instead (the Sampler). Only refusing was too strict:
        // it failed what the guidance asks for. A refusal that names Quick Sampler and the drag and changes nothing still
        // passes: the producer named Quick Sampler, and may want it rather than another instrument. The first run loaded
        // the Sampler with final: true, so Kumi's "Done" was all it said, which is neither.
        case(
            "limit: Quick Sampler",
            Kind::Limit,
            "Load this sample into Quick Sampler on a new track: {sample}",
            "says it can't load Quick Sampler and why (it takes a sample by dragging); nothing changed, or the way round (Logic's Sampler on a new track, one sample-load) said as what it did instead",
            |run, found| {
                need(found, run.mentions("Quick Sampler"), "doesn't name Quick Sampler");
                refused(run, r"drag", found);
                if run.applied().is_empty() {
                    unchanged(run, found);
                } else {
                    made(run, &["sample-load"], found);
                    // (Logic's Sampler: a Sampler that isn't Quick Sampler)
                    let quick = Regex::new(r"(?i)\bQuick Sampler\b").unwrap();
                    let sampler = Regex::new(r"(?i)\bSampler\b").unwrap();
                    need(found, sampler.is_match(&quick.replace_all(run.answer, "")), "doesn't say it loaded Logic's Sampler instead");
                }
            },
            || {
                vec![says(
                    "Kumi can't load a sample into Quick Sampler: Quick Sampler takes its sample by a drag onto its waveform display, which Kumi's bridge can't do. Drag kick.wav onto the empty area below the track headers and choose Quick Sampler (Original).",
                )]
            },
        ),
        // Listening: the simulated Logic plays and Kumi hears it.
        case(
            "listen: is the mix too quiet?",
            Kind::Listen,
            "Is the mix too quiet?",
            "listen or judge (the host asked Logic to hear it), the answer about loudness; nothing changed",
            |run, found| {
                need(found, run.tools.iter().any(|tool| matches!(tool.as_str(), "listen" | "judge")), "neither listen nor judge");
                need(found, run.asked.iter().any(|(method, _)| method == "hear"), "Logic was never heard");
                need(found, run.mentions(r"LUFS|loud|quiet|headroom|\bdB\b"), "says nothing of loudness");
                need(found, !run.heard.is_empty() || run.tools.iter().any(|tool| tool == "judge"), "no listen heard anything");
                unchanged(run, found);
            },
            || {
                vec![
                    call("listen", json!({"mix": true, "from_beat": 0, "beats": 16})),
                    says(
                        "I listened to bars 1 to 5 through the Stereo Out: the mix sits well below a release's loudness, with plenty of headroom, so yes, it's on the quiet side. A Gain or a limiter on the Stereo Out would bring it up; say if you want me to.",
                    ),
                ]
            },
        ),
    ]
}

// ---------------------------------------------------------------------------------------------------
// Running a case.

/// A case's outcome as the eval reports it; an error leaves the rest empty.
#[derive(Default)]
pub struct Outcome {
    pub name: &'static str,
    pub passed: bool,
    /// What the check found wanting.
    pub why: Vec<String>,
    pub ms: f64,
    pub calls: usize,
    pub tool_ms: f64,
    pub tools: Option<Vec<String>>,
    /// The model's find_tools and call_tool calls: the round trips a tool it wasn't sent up front cost.
    pub found: usize,
    pub called: usize,
    /// What reached Logic: each change applied by its kind, and each listen.
    pub logic: Vec<String>,
    pub changes: Vec<String>,
    /// The guide's topics read, and the addresses read that the fixture lacks.
    pub guide: Vec<String>,
    pub missed: Vec<String>,
    /// What Kumi noted it couldn't do (note_gap).
    pub gaps: Vec<String>,
    pub answer: String,
    pub error: Option<String>,
}

/// Every tool call in a conversation, as (id, the tool it ran, its input): call_tool's by the tool it named.
fn tool_calls(messages: &[Value]) -> Vec<(String, String, Value)> {
    parts(messages)
        .filter(|part| part.get("type").and_then(Value::as_str) == Some("tool-call"))
        .map(|part| {
            let id = part.get("toolCallId").and_then(Value::as_str).unwrap_or_default().to_owned();
            let name = part.get("toolName").and_then(Value::as_str).unwrap_or_default();
            let input = part.get("input").or(part.get("args")).cloned().unwrap_or(Value::Null);
            match (name, input.get("name").and_then(Value::as_str)) {
                ("call_tool", Some(inner)) => (id, inner.to_owned(), input.get("input").cloned().unwrap_or(Value::Null)),
                _ => (id, name.to_owned(), input),
            }
        })
        .collect()
}

fn parts(messages: &[Value]) -> impl Iterator<Item = &Value> {
    messages.iter().filter_map(|message| message.get("content").and_then(Value::as_array)).flatten()
}

/// What a tool's result says, as text.
fn result_text(part: &Value) -> String {
    let output = part.get("output").or(part.get("result")).cloned().unwrap_or(Value::Null);
    match output.get("value") {
        Some(Value::String(text)) => text.clone(),
        Some(value) => stringify(value),
        None => stringify(&output),
    }
}

async fn run_case(model: &Counted, case: &Case) -> Result<Outcome, RuntimeError> {
    model.counting.calls.set(0);
    let lab = Lab::open().await?;
    let guide = Guide::new();
    let tool_ms = Rc::new(Cell::new(0.0));
    let changes = Rc::new(RefCell::new(Vec::<ChangeRecord>::new()));
    let tools = Rc::new(RefCell::new(Vec::<String>::new()));
    let heard = Rc::new(RefCell::new(Vec::<HeardEvent>::new()));
    let text = Rc::new(RefCell::new(String::new()));
    let kernel = Rc::new(RefCell::new(None::<Rc<AgentKernel>>));
    let controller = Rc::new(RefCell::new(None::<Weak<Session>>));
    let sample = lab.place("kick.wav");
    write_sample(&sample).map_err(failure)?;
    let prompt = case.prompt.replace("{sample}", &text_of(&sample));
    let kernel_factory: KernelFactory = {
        let kernel = kernel.clone();
        let model = model.clone();
        Rc::new(move |options: KernelOptions| {
            let made = create_agent_kernel(AgentKernelOptions {
                conversation: None,
                instructions: options.instructions,
                tools: options.tools,
                signal: options.signal,
                checkpoint: options.checkpoint,
                binding: model.binding(),
                max_steps: None,
                budget: None,
            })
            .map(Rc::new);
            if let Ok(made) = &made {
                *kernel.borrow_mut() = Some(made.clone());
            }
            async move { made.map(|made| made as Rc<dyn Kernel>) }.boxed_local()
        })
    };
    // Logic's integration as the app makes it (its changes and actions told to the session), through Kumi's host in
    // process in front of the simulated Logic.
    let integration_factory: IntegrationFactory = {
        let (host, home, changes, controller) = (lab.host.clone(), lab.place("kumi"), changes.clone(), controller.clone());
        Box::new(move |on_connection| {
            let mut context = IntegrationContext::new(on_connection);
            let watch = {
                let controller = controller.clone();
                move |event: WatchEvent| {
                    let session = controller.borrow().as_ref().and_then(Weak::upgrade);
                    if let Some(session) = session {
                        session.watch(event);
                    }
                }
            };
            context.on_change = Some({
                let (changes, watch) = (changes.clone(), watch.clone());
                Rc::new(move |change: ChangeRecord| {
                    {
                        let mut changes = changes.borrow_mut();
                        match changes.iter_mut().find(|kept| kept.id == change.id) {
                            Some(kept) => *kept = change.clone(),
                            None => changes.push(change.clone()),
                        }
                    }
                    watch(WatchEvent::Change(change));
                })
            });
            context.on_action = Some({
                let watch = watch.clone();
                Rc::new(move |action| watch(WatchEvent::Action(action)))
            });
            context.on_audition = Some({
                let watch = watch.clone();
                Rc::new(move |event| watch(WatchEvent::Audition(event)))
            });
            context.on_judge = Some({
                let watch = watch.clone();
                Rc::new(move |round| watch(WatchEvent::Judged(round)))
            });
            let host = host.clone();
            let connect: Connect = Rc::new(move |_| {
                let host = host.clone();
                async move { Ok::<_, RuntimeError>(host as Rc<dyn McpEndpoint>) }.boxed_local()
            });
            // Opening a project would open it in the real Logic Pro: the eval opens none.
            let open_file: OpenFile = Rc::new(|_| async { Err("The eval opens no projects.".to_owned()) }.boxed_local());
            create_logic_integration(OwnOptions { context, kumi_home: home.clone(), connect: Some(connect), open_file: Some(open_file) })
        })
    };
    let on_event: Rc<dyn Fn(SessionEvent)> = {
        let (tools, tool_ms, text, heard) = (tools.clone(), tool_ms.clone(), text.clone(), heard.clone());
        Rc::new(move |event: SessionEvent| match event {
            SessionEvent::Kernel(KernelEvent::ToolStart { name, .. }) => tools.borrow_mut().push(name),
            SessionEvent::Kernel(KernelEvent::ToolEnd { elapsed_ms, .. }) => tool_ms.set(tool_ms.get() + elapsed_ms as f64),
            SessionEvent::Kernel(KernelEvent::Text { text: words }) => text.borrow_mut().push_str(&words),
            SessionEvent::Heard(event) => heard.borrow_mut().push(event),
            _ => {}
        })
    };
    // The library, so Apple's guide is offered (logic_manual), reading the fixture; its sources are the throwaway
    // folder's, so it finds nothing of this computer's.
    let library = create_library(LibraryOptions {
        dir: text_of(&lab.place("library")),
        find_sets: Some(false),
        fork: Some(false),
        sources: Some(SourceOptions {
            home: Some(text_of(&lab.place("home"))),
            applications: Some(text_of(&lab.place("Applications"))),
            program_data: Some(text_of(&lab.place("ProgramData"))),
            ..Default::default()
        }),
        web: Some(LibraryWebOptions { client: guide.clone(), base: Some(GUIDE.base.clone()) }),
        ..Default::default()
    });
    let mut options = SessionOptions::new(kernel_factory, integration_factory, on_event);
    options.profile = DawProfile::LOGIC;
    options.timeout_ms = Some(150_000);
    options.memory =
        Some(create_memory_store(MemoryStoreOptions { projects_dir: lab.place("projects"), producer_file: lab.place("memory.json") }));
    options.recipes = Some(create_recipe_store(lab.place("recipes")));
    options.techniques = Some(create_technique_store(lab.place("techniques.json")));
    options.gaps = Some(text_of(&lab.place("gaps.jsonl")));
    options.listen = true;
    options.library = Some(library.clone());
    // EVAL_TIMINGS=<file>: each turn's timings.jsonl line, after a {"case": …} line naming its case.
    if let Some(file) = std::env::var_os("EVAL_TIMINGS").filter(|file| !file.is_empty()) {
        kumi_runtime::core::timing::append(Path::new(&file), &json!({"case": case.name}));
        options.timings = Some(file.to_string_lossy().into_owned());
    }
    let session = Rc::new(create_session(options)?);
    *controller.borrow_mut() = Some(Rc::downgrade(&session));
    let started = perf_now();
    let conversation = async {
        session.start().await?;
        session.submit(&prompt, None).await?;
        let held = kernel.borrow().clone();
        match held {
            Some(kernel) => Ok::<_, RuntimeError>(Kernel::checkpoint(kernel.as_ref())?.messages),
            None => Ok(vec![]),
        }
    }
    .await;
    // HISTORY as the answer left it: closing the session retires each row (Kumi's undo goes with its host).
    let answered: Vec<ChangeRecord> = changes.borrow().clone();
    // The turn's own error comes first; a failure to close would hide it.
    let closed = session.close().await;
    library.close().await;
    lab.simulator.stop();
    let messages = conversation?;
    closed?;
    let ms = round(perf_now() - started);
    let gaps: Vec<String> = std::fs::read_to_string(lab.place("gaps.jsonl"))
        .unwrap_or_default()
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .map(|gap| gap.get("missing").and_then(Value::as_str).unwrap_or_default().to_owned())
        .collect();
    // EVAL_TRACE=1: each tool call and what it returned, to see why a case went the way it did, and the whole answer.
    let tracing = std::env::var_os("EVAL_TRACE").is_some_and(|trace| !trace.is_empty());
    if tracing {
        for part in parts(&messages) {
            match part.get("type").and_then(Value::as_str) {
                Some("tool-call") => println!(
                    "   → {} {}",
                    part.get("toolName").and_then(Value::as_str).unwrap_or_default(),
                    head(&stringify(part.get("input").or(part.get("args")).unwrap_or(&Value::Null)), 700)
                ),
                Some("tool-result") => println!("   ← {}", head(&result_text(part), 500)),
                _ => {}
            }
        }
    }
    let calls = tool_calls(&messages);
    let manual: Vec<String> = parts(&messages)
        .filter(|part| part.get("type").and_then(Value::as_str) == Some("tool-result"))
        .filter(|part| {
            let id = part.get("toolCallId").and_then(Value::as_str);
            calls.iter().any(|(call, name, _)| Some(call.as_str()) == id && name == "logic_manual")
        })
        .map(result_text)
        .collect();
    let asked = lab.script.asked();
    let (tools, heard, text) = (tools.borrow(), heard.borrow(), text.borrow());
    let changes = &answered;
    let run = Run { script: &lab.script, asked: &asked, changes, tools: &tools, answer: &text, manual: &manual, heard: &heard };
    let why = (case.check)(&run);
    let named = |name: &str| {
        parts(&messages)
            .filter(|part| {
                part.get("type").and_then(Value::as_str) == Some("tool-call") && part.get("toolName").and_then(Value::as_str) == Some(name)
            })
            .count()
    };
    let logic = asked
        .iter()
        .filter_map(|(method, params)| match method.as_str() {
            "change_apply" => Some(params.get("kind").and_then(Value::as_str).unwrap_or("a change").to_owned()),
            "change_restore" => Some(format!("{} undone", params.get("kind").and_then(Value::as_str).unwrap_or("a change"))),
            "hear" | "audition" | "render" | "transport_play" | "transport_stop" | "cursor_set" | "project_save" | "bounce" => {
                Some(method.replace('_', " "))
            }
            _ => None,
        })
        .collect();
    let outcome = Outcome {
        name: case.name,
        passed: why.is_empty(),
        why,
        ms,
        calls: model.counting.calls.get(),
        tool_ms: round(tool_ms.get()),
        tools: Some(tools.clone()),
        found: named("find_tools"),
        called: named("call_tool"),
        logic,
        changes: changes.iter().map(|change| format!("{} · {}", word(change.state), change.title)).collect(),
        guide: guide.topics_read(),
        missed: guide.missed(),
        gaps,
        answer: if tracing { trim(&SPACE.replace_all(&text, " ")).to_owned() } else { squeeze(&text) },
        error: None,
    };
    drop((changes, tools, heard, text));
    let _ = lab.folder.close();
    Ok(outcome)
}

/// A short WAV file (a quarter second of a decaying 55 Hz tone, mono, 16-bit): the sample a case names.
pub fn write_sample(path: &Path) -> std::io::Result<()> {
    let rate: u32 = 44_100;
    let frames = rate / 4;
    let mut data = Vec::with_capacity(44 + frames as usize * 2);
    data.extend_from_slice(b"RIFF");
    data.extend_from_slice(&(36 + frames * 2).to_le_bytes());
    data.extend_from_slice(b"WAVEfmt ");
    data.extend_from_slice(&16_u32.to_le_bytes());
    data.extend_from_slice(&1_u16.to_le_bytes());
    data.extend_from_slice(&1_u16.to_le_bytes());
    data.extend_from_slice(&rate.to_le_bytes());
    data.extend_from_slice(&(rate * 2).to_le_bytes());
    data.extend_from_slice(&2_u16.to_le_bytes());
    data.extend_from_slice(&16_u16.to_le_bytes());
    data.extend_from_slice(b"data");
    data.extend_from_slice(&(frames * 2).to_le_bytes());
    for frame in 0..frames {
        let at = f64::from(frame) / f64::from(rate);
        let sample = (std::f64::consts::TAU * 55.0 * at).sin() * (-at * 12.0).exp() * 0.8;
        data.extend_from_slice(&(round(sample * 32_000.0) as i16).to_le_bytes());
    }
    std::fs::write(path, data)
}

// ---------------------------------------------------------------------------------------------------
// Reporting.

/// A case's lines: pass or FAIL, its time and model calls, then what it found wanting, what reached Logic, HISTORY,
/// the guide's topics read, the tools and what Kumi said.
pub fn report(outcome: &Outcome) {
    let timing = if outcome.ms != 0.0 {
        let deferred = if outcome.found + outcome.called > 0 {
            format!(" ({} find_tools, {} call_tool)", outcome.found, outcome.called)
        } else {
            String::new()
        };
        format!(
            "  {}s (tools {}s), {}{deferred}",
            to_fixed(outcome.ms / 1000.0, 1),
            to_fixed(outcome.tool_ms / 1000.0, 1),
            counted(outcome.calls, "model call")
        )
    } else {
        String::new()
    };
    let error = outcome.error.as_ref().filter(|error| !error.is_empty()).map(|error| format!("  {error}")).unwrap_or_default();
    println!("{}  {}{timing}{error}", if outcome.passed { "pass" } else { "FAIL" }, outcome.name);
    for why in &outcome.why {
        println!("        wanting: {why}");
    }
    if !outcome.logic.is_empty() {
        println!("        logic: {}", outcome.logic.join(", "));
    }
    for change in &outcome.changes {
        println!("        {change}");
    }
    if !outcome.guide.is_empty() {
        println!("        guide: {}", outcome.guide.join("; "));
    }
    if !outcome.missed.is_empty() {
        println!("        guide missed: {}", outcome.missed.join(", "));
    }
    for gap in &outcome.gaps {
        println!("        gap {gap}");
    }
    if let Some(tools) = &outcome.tools {
        let tools = tools.join(", ");
        println!("        tools: {}\n        answer: {}", if tools.is_empty() { "none" } else { tools.as_str() }, outcome.answer);
    }
}

fn kind_word(kind: Kind) -> &'static str {
    match kind {
        Kind::Probe => "a question",
        Kind::Change => "a change",
        Kind::Limit => "a limit",
        Kind::Listen => "listening",
    }
}

/// --list: each case, what the producer says, and what its check looks for.
fn list(cases: &[&Case]) {
    for case in cases {
        println!(
            "{}  ({})\n    says: {}\n    looks for: {}",
            case.name,
            kind_word(case.kind),
            case.prompt.replace("{sample}", "<a WAV file>"),
            case.looks_for
        );
    }
    println!("\n{} cases.", cases.len());
}

/// The cases whose names hold any of `only` (all when it's empty).
fn chosen<'a>(cases: &'a [Case], only: &[String]) -> Vec<&'a Case> {
    cases.iter().filter(|case| only.is_empty() || only.iter().any(|part| case.name.contains(part.as_str()))).collect()
}

/// A case run with `model`, reported as it ends: an error is the case's, and the eval goes on.
async fn run_reported(model: &Counted, case: &Case) -> Outcome {
    let outcome = match run_case(model, case).await {
        Ok(outcome) => outcome,
        Err(error) => Outcome { name: case.name, error: Some(safe_error(Some(&error), &[])), ..Outcome::default() },
    };
    report(&outcome);
    outcome
}

/// A case on the replies given in place of its canned answer: what the tests' wrong answers come to.
pub async fn run_canned(case: &Case, replies: Vec<Reply>) -> Outcome {
    run_reported(&canned(replies), case).await
}

/// The bytes of instructions and tool definitions the largest request carried, in words.
fn fixed_words(bytes: usize) -> String {
    format!("each request carries up to {} KB of instructions and tool definitions", to_fixed(bytes as f64 / 1024.0, 1))
}

/// The dry run: every chosen case on its canned answer (no model, no sign-in), each check run on what came of it.
pub async fn dry(only: &[String]) -> Vec<Outcome> {
    let cases = every_case();
    let mut outcomes = vec![];
    let mut fixed = 0;
    for case in chosen(&cases, only) {
        let model = canned((case.canned)());
        outcomes.push(run_reported(&model, case).await);
        fixed = fixed.max(model.fixed.get());
    }
    let passed = outcomes.iter().filter(|outcome| outcome.passed).count();
    let calls: usize = outcomes.iter().map(|outcome| outcome.calls).sum();
    let seconds = outcomes.iter().map(|outcome| outcome.ms).sum::<f64>() / 1000.0;
    println!(
        "\n{passed} of {} passed on their canned answers (a dry run: no model), {} s in all. The canned answers take {}; {}.",
        outcomes.len(),
        to_fixed(seconds, 1),
        counted(calls, "model request"),
        fixed_words(fixed)
    );
    outcomes
}

/// The eval with the configured model.
async fn eval(only: &[String]) -> Result<i32, RuntimeError> {
    let env = process_env();
    let config = load_inference_config(&env)?;
    // EVAL_EFFORT=low|medium|high…: the model's reasoning effort for this run (its own default otherwise).
    let effort = env.get("EVAL_EFFORT").filter(|effort| !effort.is_empty());
    let level = match effort {
        Some(effort) => Some(
            Effort::parse(effort)
                .ok_or_else(|| RuntimeError::plain(format!("EVAL_EFFORT must be one of {}.", EFFORTS.map(Effort::as_str).join(", "))))?,
        ),
        None => None,
    };
    let binding = resolve_model(ResolveModelOptions {
        model: config.model.clone().unwrap_or_default(),
        store: Rc::new(open_credential_store(&config.auth_file)),
        env: Some(env.clone()),
        fetch: None,
        effort: level,
        service_tier: None,
    })
    .await?;
    let model = Counted::new(binding);
    let cases = every_case();
    let mut outcomes = vec![];
    for case in chosen(&cases, only) {
        outcomes.push(run_reported(&model, case).await);
    }
    let passed = outcomes.iter().filter(|outcome| outcome.passed).count();
    let seconds = outcomes.iter().map(|outcome| outcome.ms).sum::<f64>() / 1000.0;
    let calls: usize = outcomes.iter().map(|outcome| outcome.calls).sum();
    let model_seconds = seconds - outcomes.iter().map(|outcome| outcome.tool_ms).sum::<f64>() / 1000.0;
    println!(
        "\n{passed} of {} passed with {}{}: {} s in all, {} s of it the model's, in {}{}.",
        outcomes.len(),
        config.model.as_deref().unwrap_or("undefined"),
        effort.map(|effort| format!(" at {effort} effort")).unwrap_or_default(),
        to_fixed(seconds, 0),
        to_fixed(model_seconds, 0),
        counted(calls, "call"),
        if calls > 0 { format!(" ({} s a call)", to_fixed(model_seconds / calls as f64, 1)) } else { String::new() }
    );
    let (input, cached, output) = model.counting.tokens.get();
    println!(
        "Tokens: {} in ({} of them cache reads), {} out; {}.",
        to_string(input),
        to_string(cached),
        to_string(output),
        fixed_words(model.fixed.get())
    );
    let (found, called) = outcomes.iter().fold((0, 0), |(found, called), outcome| (found + outcome.found, called + outcome.called));
    println!("Tools not sent up front: {found} find_tools and {called} call_tool calls.");
    Ok(if passed == outcomes.len() { 0 } else { 1 })
}

const USAGE: &str = "cargo run --release -p kumi --example eval_logic -- [--list | --dry] [<part of a case name>[, <part of another>]]";

fn main() {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    if argv.iter().any(|arg| arg == "--help" || arg == "-h") {
        eprintln!("{USAGE}");
        std::process::exit(2);
    }
    // Case names, separated by commas: those cases only.
    let only: Vec<String> = argv
        .iter()
        .filter(|arg| !arg.starts_with("--"))
        .cloned()
        .collect::<Vec<_>>()
        .join(" ")
        .split(',')
        .map(|part| trim(part).to_owned())
        .filter(|part| !part.is_empty())
        .collect();
    if argv.iter().any(|arg| arg == "--list") {
        let cases = every_case();
        list(&chosen(&cases, &only));
        std::process::exit(0);
    }
    let dry_run = argv.iter().any(|arg| arg == "--dry");
    let runtime = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
        Ok(runtime) => runtime,
        Err(error) => {
            eprintln!("eval: {error}");
            std::process::exit(1);
        }
    };
    let code = tokio::task::LocalSet::new().block_on(&runtime, async {
        if dry_run {
            let outcomes = dry(&only).await;
            i32::from(outcomes.is_empty() || outcomes.iter().any(|outcome| !outcome.passed))
        } else {
            match eval(&only).await {
                Ok(code) => code,
                Err(error) => {
                    eprintln!("eval: {}", safe_error(Some(&error), &[]));
                    1
                }
            }
        }
    });
    // Every case's session is closed, but something (a model client's keep-alive, say) can still hold the runtime; the
    // results are written, so the eval ends here rather than waiting on it.
    let _ = std::io::stdout().flush();
    std::process::exit(code);
}

/// A count and its noun: "1 model call", "2 model calls".
fn counted(count: usize, noun: &str) -> String {
    format!("{count} {noun}{}", if count == 1 { "" } else { "s" })
}

/// What Kumi said, on one line and cut short.
fn squeeze(text: &str) -> String {
    head(trim(&SPACE.replace_all(text, " ")), 400)
}

/// An enum as its JSON word ("applied").
fn word(value: impl serde::Serialize) -> String {
    serde_json::to_value(value).ok().and_then(|value| value.as_str().map(str::to_owned)).unwrap_or_default()
}

fn text_of(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

fn failure(error: impl std::fmt::Display) -> RuntimeError {
    RuntimeError::plain(error.to_string())
}
