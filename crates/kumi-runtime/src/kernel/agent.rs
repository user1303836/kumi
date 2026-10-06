use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::rc::Rc;
use std::sync::LazyLock;
use std::time::Duration;

use async_trait::async_trait;
use base64::Engine;
use futures::future::{join_all, FutureExt, LocalBoxFuture, Shared};
use futures::StreamExt;
use kumi_common::abort::{self, Aborted, Controller, Signal};
use kumi_common::js::json::{quote, stringify};
use kumi_common::js::number::{is_safe_integer, round};
use kumi_common::js::string::{head, trim, utf16_len};
use kumi_common::time::perf_now;
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use tokio::sync::watch;

use super::budget::{fit, opening, put_away_images, transcript_of, ContextBudget, PutAway, DEFAULT_BUDGET};
use super::failure::{describe_failure, retry_delay_ms, retry_reason, MAX_RETRIES};
use crate::ai::error::LanguageModelError;
use crate::ai::types::{
    AssistantPart, CallOptions, DataContent, FileData, FilePart, FinishReason, FinishReasonUnified, FunctionTool, Message, Prompt,
    ProviderMetadata, ReasoningPart, StreamPart, StreamParts, TextPart, ToolCall, ToolCallPart, ToolPart, ToolResultContentItem,
    ToolResultOutput, ToolResultPart, Usage as ModelUsage, UserPart,
};
use crate::core::contracts::{
    JsonObject, Kernel, KernelCheckpoint, KernelEmit, KernelEvent, KernelTool, OnText, Picture, StopReason, StreamingCall, ToolImage,
    TranscriptLine, TurnResult, Usage,
};
use crate::core::errors::{FailureKind, KumiError, RuntimeError};

/// A model that streams: `LanguageModelV4.doStream`, the one call Kumi makes.
#[async_trait(?Send)]
pub trait LanguageModel {
    async fn do_stream(&self, options: CallOptions) -> Result<StreamParts, LanguageModelError>;
}

pub struct ModelRequest {
    pub instructions: String,
    /// Conversation so far, without a system message; bindings place instructions where their API expects them.
    pub messages: Prompt,
    pub tools: Vec<FunctionTool>,
    /// Stable for one kernel: provider prompt-cache routing and request correlation.
    pub session_id: String,
}

/// A configured model plus its provider-specific request shaping. The loop itself is provider-neutral.
pub struct ModelBinding {
    /// "<provider>/<model>" as configured.
    pub id: String,
    pub model: Rc<dyn LanguageModel>,
    pub prepare: Box<dyn Fn(ModelRequest) -> CallOptions>,
    /// How much conversation fits beside the instructions and tools (`fixed` bytes), for a model that
    /// reads less at once than the default budget assumes (one on the producer's computer). Asked
    /// before every call: what the model reads may be known only once its server has answered.
    pub budget: Option<Box<dyn Fn(usize) -> ContextBudget>>,
}

/// Plain JSON, owned by Kumi: settled messages only, including provider replay metadata.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Checkpoint {
    pub version: u32,
    pub messages: Vec<Message>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tools: Option<String>,
}

impl From<Checkpoint> for KernelCheckpoint {
    fn from(checkpoint: Checkpoint) -> Self {
        Self {
            version: checkpoint.version,
            messages: checkpoint.messages.iter().map(|message| serde_json::to_value(message).expect("message serializes")).collect(),
            origin: checkpoint.origin,
            tools: checkpoint.tools,
        }
    }
}

pub struct AgentKernelOptions {
    pub instructions: String,
    pub tools: Vec<Rc<dyn KernelTool>>,
    pub signal: Signal,
    /// Continue this conversation instead of starting empty.
    pub checkpoint: Option<KernelCheckpoint>,
    pub binding: ModelBinding,
    /// Model calls per turn; each tool round trip is one more. A patch built knob by knob takes dozens.
    pub max_steps: Option<usize>,
    /// How much conversation to send; older Live reads are cleared first.
    pub budget: Option<ContextBudget>,
    /// The conversation's own id: the session id providers key their prompt cache on comes from it,
    /// so a resumed conversation finds its cache. Without it, each kernel has a new one.
    pub conversation: Option<String>,
}

struct StepResult {
    content: Vec<AssistantPart>,
    calls: Vec<(ToolCall, Option<JsonObject>)>,
    usage: ModelUsage,
}

/// What stops a step: the abort signal, or the model (its call, its stream, or Kumi's reading of it).
enum StepError {
    Aborted,
    Model(LanguageModelError),
}

impl From<Aborted> for StepError {
    fn from(_: Aborted) -> Self {
        Self::Aborted
    }
}

impl StepError {
    fn into_model_error(self) -> LanguageModelError {
        match self {
            Self::Model(error) => error,
            Self::Aborted => LanguageModelError::other("This operation was aborted"),
        }
    }
}

fn kumi(kind: FailureKind, message: &str) -> StepError {
    StepError::Model(LanguageModelError::Kumi(KumiError::new(kind, message)))
}

/// `AbortSignal.any(parts)`: the signals a turn stops on. An abort shows the moment it happens
/// (the parts are checked themselves); the combined token is what's awaited and handed to tools.
#[derive(Clone)]
struct Abort {
    parts: Vec<Signal>,
    combined: Signal,
}

impl Abort {
    fn new(parts: Vec<Signal>) -> Self {
        let combined = abort::any(parts.iter().cloned());
        Self { parts, combined }
    }

    /// `signal.aborted`.
    fn is_cancelled(&self) -> bool {
        self.parts.iter().any(Signal::is_cancelled)
    }

    /// `signal.throwIfAborted()`.
    fn check(&self) -> Result<(), Aborted> {
        if self.is_cancelled() {
            Err(Aborted)
        } else {
            Ok(())
        }
    }

    /// The one signal a tool or a model call takes.
    fn signal(&self) -> Signal {
        self.combined.clone()
    }

    async fn cancelled(&self) {
        self.combined.cancelled().await
    }
}

static TOOL_NAME: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[a-zA-Z0-9_-]{1,64}$").expect("regex"));
const MAX_TOOLS: usize = 128;
const MAX_INSTRUCTIONS: usize = 64 * 1024;
const MAX_STEER: usize = 16 * 1024;
const MAX_TOOL_ERROR: usize = 4 * 1024;
/// The most of a tool's words one result carries, as the bridge's results are capped: past it the opening
/// stays with a note saying how much was cut, so one huge result can't take a request past its budget.
const MAX_TOOL_RESULT: usize = 64 * 1024;
/// A tool's own answer to the producer, when it finished the request.
const MAX_REPLY: usize = 8 * 1024;
/// Images one tool result shows, and the media types models read.
const MAX_TOOL_IMAGES: usize = 16;
const IMAGE_TYPES: [&str; 4] = ["image/jpeg", "image/png", "image/webp", "image/gif"];
/// Says what a side question is, ahead of its words.
const ASIDE_NOTE: &str = "(A side question while you work. Answer it briefly, in plain words, from what's above; use no tools. It doesn't change the request you're working on, and your answer isn't kept in the conversation.)";
/// What the model is told when its answer broke off and Kumi carries on. `[Kumi]` marks it as Kumi's, not the producer's.
pub const CARRY_ON_NOTE: &str =
    "[Kumi] The connection dropped partway through your answer. Carry on from where it stopped, without repeating what you said.";
/// Ends a stopped turn's kept steps, for the model and in the transcript.
pub const STOPPED_NOTE: &str =
    "(Stopped before finishing. The steps above happened; the one in progress may have too, so check Live before carrying on.)";
/// The result of a call a stopped batch never ran, and of the one it was running.
pub const STOPPED_BEFORE_RUNNING: &str = "Stopped before running; nothing was done.";
pub const STOPPED_WHILE_RUNNING: &str = "Stopped while running; it may have changed Live, so check before running it again.";
/// The result of a read away from Live that was stopped while it ran.
pub const STOPPED_READING: &str = "Stopped while running; nothing came back.";

/// The turn under way: guidance waiting for its next step, what it has said and done so far, and when it settles.
struct Running {
    steering: RefCell<Vec<String>>,
    context: RefCell<Option<Rc<dyn Fn() -> Vec<Message>>>>,
    done: watch::Receiver<bool>,
}

struct Inner {
    binding: ModelBinding,
    instructions: String,
    max_steps: usize,
    budget: ContextBudget,
    budget_given: bool,
    tools: HashMap<String, Rc<dyn KernelTool>>,
    specs: Vec<FunctionTool>,
    session_id: String,
    lifetime: Controller,
    /// What the conversation was made with besides its messages: some models' reasoning is bound to it.
    tools_key: String,
    /// What the instructions and tools take, for a binding that sizes the conversation to its model.
    fixed: usize,
    history: RefCell<Vec<Message>>,
    running: RefCell<Option<Rc<Running>>>,
    closing: RefCell<Option<Shared<LocalBoxFuture<'static, ()>>>>,
}

impl Inner {
    fn budget_now(&self) -> ContextBudget {
        match &self.binding.budget {
            Some(budget) if !self.budget_given => budget(self.fixed),
            _ => self.budget,
        }
    }
}

/// The kernel `createAgentKernel` returns: a turn loop over one model binding, with steering, side
/// questions, checkpoints and the transcript. Clones share the kernel.
#[derive(Clone)]
pub struct AgentKernel {
    inner: Rc<Inner>,
}

pub fn create_agent_kernel(options: AgentKernelOptions) -> Result<AgentKernel, RuntimeError> {
    let AgentKernelOptions { instructions, tools, signal: _, checkpoint, binding, max_steps, budget, conversation } = options;
    let max_steps = max_steps.unwrap_or(200);
    let budget_given = budget.is_some();
    let budget = budget.unwrap_or(DEFAULT_BUDGET);
    if trim(&instructions).is_empty() || instructions.len() > MAX_INSTRUCTIONS {
        return Err(RuntimeError::plain("Kernel instructions must be nonempty and at most 64 KiB."));
    }
    if max_steps < 1 {
        return Err(RuntimeError::plain("maxSteps must be a positive integer."));
    }
    if !is_safe_integer(budget.clear_at) || !is_safe_integer(budget.limit) || budget.clear_at < 1024.0 || budget.limit < budget.clear_at {
        return Err(RuntimeError::plain("The context budget must clear at 1 KiB or more, with a limit at least that."));
    }
    let names: Vec<&str> = tools.iter().map(|tool| tool.name()).collect();
    if names.len() > MAX_TOOLS
        || names.iter().collect::<HashSet<_>>().len() != names.len()
        || names.iter().any(|name| !TOOL_NAME.is_match(name))
    {
        return Err(RuntimeError::plain("Tool names must be unique, at most 64 characters of [a-zA-Z0-9_-], and at most 128 tools."));
    }
    let specs: Vec<FunctionTool> =
        tools.iter().map(|tool| FunctionTool::new(tool.name(), tool.description(), Value::Object(tool.input_schema()))).collect();
    let tools: HashMap<String, Rc<dyn KernelTool>> = tools.iter().map(|tool| (tool.name().to_string(), tool.clone())).collect();
    let session_id = match conversation {
        // A UUID of the conversation's own: the same one every time it's carried on.
        Some(conversation) => {
            let digest = Sha256::digest(format!("kumi-conversation:{conversation}").as_bytes());
            uuid::Builder::from_random_bytes(digest[..16].try_into().expect("16 bytes")).into_uuid().to_string()
        }
        None => uuid::Uuid::new_v4().to_string(),
    };
    let lifetime = Controller::new();
    // What the conversation was made with besides its messages: some models' reasoning is bound to it.
    let fingerprint = stringify(&serde_json::json!({ "instructions": instructions, "specs": specs }));
    let tools_key = head(&base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(Sha256::digest(fingerprint.as_bytes())), 22);
    // A binding that sizes the conversation to its model is told what the instructions and tools take.
    let fixed = instructions.len() + stringify(&serde_json::to_value(&specs).expect("specs serialize")).len();
    let history = match &checkpoint {
        Some(checkpoint) => restore(checkpoint, &binding.id, &tools_key)?,
        None => Vec::new(),
    };
    Ok(AgentKernel {
        inner: Rc::new(Inner {
            binding,
            instructions,
            max_steps,
            budget,
            budget_given,
            tools,
            specs,
            session_id,
            lifetime,
            tools_key,
            fixed,
            history: RefCell::new(history),
            running: RefCell::new(None),
            closing: RefCell::new(None),
        }),
    })
}

/// Clears the kernel's running state when a turn settles (or its future is dropped), and says so to `close`.
struct TurnGuard {
    inner: Rc<Inner>,
    done: watch::Sender<bool>,
}

impl Drop for TurnGuard {
    fn drop(&mut self) {
        *self.inner.running.borrow_mut() = None;
        self.done.send_replace(true);
    }
}

impl AgentKernel {
    pub async fn run(&self, input: &str, signal: Signal, emit: KernelEmit) -> Result<TurnResult, RuntimeError> {
        self.run_with(input, vec![], signal, emit).await
    }

    /// A turn whose request carries pictures beside the words, for the model to see.
    pub async fn run_with(
        &self,
        input: &str,
        pictures: Vec<Picture>,
        signal: Signal,
        emit: KernelEmit,
    ) -> Result<TurnResult, RuntimeError> {
        if self.inner.closing.borrow().is_some() {
            return Err(RuntimeError::plain("Kernel is closed"));
        }
        if self.inner.running.borrow().is_some() {
            return Err(RuntimeError::plain("Kernel is busy; cancel first"));
        }
        if signal.is_cancelled() {
            return Ok(TurnResult { stop_reason: StopReason::Cancelled, usage: None });
        }
        let (done, settled) = watch::channel(false);
        let state = Rc::new(Running { steering: RefCell::new(Vec::new()), context: RefCell::new(None), done: settled });
        *self.inner.running.borrow_mut() = Some(state.clone());
        let _guard = TurnGuard { inner: self.inner.clone(), done };
        let mut content = vec![UserPart::Text(TextPart::new(input))];
        content.extend(pictures.into_iter().map(|picture| {
            UserPart::File(FilePart {
                filename: Some(picture.name),
                data: FileData::Data { data: DataContent::Bytes(picture.data) },
                media_type: picture.media_type,
                provider_options: None,
            })
        }));
        turn(self.inner.clone(), Message::User { content, provider_options: None }, signal, emit, state).await
    }

    /// Queue guidance for the running turn; it enters at the next model boundary. False when idle.
    pub fn steer(&self, text: &str) -> bool {
        let running = self.inner.running.borrow();
        let Some(running) = running.as_ref() else { return false };
        if self.inner.closing.borrow().is_some() || trim(text).is_empty() || text.len() > MAX_STEER {
            return false;
        }
        running.steering.borrow_mut().push(text.to_string());
        true
    }

    /// A side question about the conversation so far (the turn under way included, up to its last
    /// finished step), answered in one model call without tools. It never enters the conversation.
    pub async fn aside(&self, question: &str, signal: Signal, on_text: OnText) -> Result<String, RuntimeError> {
        let inner = &self.inner;
        if inner.closing.borrow().is_some() {
            return Err(RuntimeError::plain("Kernel is closed"));
        }
        let words = trim(question);
        if words.is_empty() || words.len() > MAX_STEER {
            return Err(KumiError::new(FailureKind::Request, "Ask a side question of at most 16 KiB.").into());
        }
        let context = inner.running.borrow().as_ref().and_then(|running| running.context.borrow().clone());
        let context: Vec<Message> = match context {
            Some(context) => context(),
            None => inner.history.borrow().clone(),
        };
        let asked = [Message::user_text(format!("{ASIDE_NOTE}\n\n{words}"))];
        let fitted = fit(&context, &asked, &inner.budget_now());
        // The conversation in plain words, its calls and their results written out: then no tools are
        // offered at all, which every provider takes (some can't be told "none" with calls in the conversation).
        let messages = plain_words(&[fitted.history.as_ref(), fitted.turn.as_ref()].concat());
        let request = (inner.binding.prepare)(ModelRequest {
            instructions: inner.instructions.clone(),
            messages,
            tools: Vec::new(),
            session_id: inner.session_id.clone(),
        });
        let abort = Abort::new(vec![signal, inner.lifetime.signal.clone()]);
        for attempt in 0.. {
            let delivered = Cell::new(false);
            let hear = |text: &str| {
                delivered.set(true);
                on_text(text);
            };
            let attempted: Result<StepResult, StepError> = async {
                let mut call = request.clone();
                call.abort_signal = Some(abort.signal());
                let timed = crate::core::timing::model_call(&inner.binding.id);
                let parts = inner.binding.model.do_stream(call).await.map_err(StepError::Model)?;
                consume(crate::core::timing::timed(parts, timed), &abort, &hear, None).await
            }
            .await;
            match attempted {
                Ok(result) => {
                    let words: String = result
                        .content
                        .iter()
                        .map(|part| if let AssistantPart::Text(text) = part { text.text.as_str() } else { "" })
                        .collect();
                    return Ok(trim(&words).to_string());
                }
                Err(error) => {
                    let wait = match &error {
                        StepError::Model(model) if attempt < MAX_RETRIES && !delivered.get() && !abort.is_cancelled() => {
                            retry_delay_ms(model, attempt)
                        }
                        _ => None,
                    };
                    let Some(wait) = wait else {
                        return Err(match error {
                            StepError::Aborted => RuntimeError::Aborted,
                            StepError::Model(LanguageModelError::Kumi(error)) => RuntimeError::Kumi(error),
                            StepError::Model(error) if abort.is_cancelled() => RuntimeError::Plain(error.to_string()),
                            StepError::Model(error) => RuntimeError::Kumi(describe_failure(&error, &inner.binding.id)),
                        });
                    };
                    delay(wait, &abort).await.map_err(|_| RuntimeError::Aborted)?;
                }
            }
        }
        unreachable!("a side question's attempts end by returning")
    }

    /// Settled conversation only; an in-flight turn is never included.
    pub fn checkpoint(&self) -> Result<Checkpoint, RuntimeError> {
        if self.inner.running.borrow().is_some() {
            return Err(RuntimeError::plain("Kernel is busy; checkpoint between turns"));
        }
        Ok(Checkpoint {
            version: 1,
            messages: self.inner.history.borrow().clone(),
            origin: Some(self.inner.binding.id.clone()),
            tools: Some(self.inner.tools_key.clone()),
        })
    }

    pub fn transcript(&self) -> Vec<TranscriptLine> {
        let values: Vec<Value> =
            self.inner.history.borrow().iter().map(|message| serde_json::to_value(message).expect("message serializes")).collect();
        transcript_of(&values)
    }

    pub async fn close(&self) {
        let existing = self.inner.closing.borrow().clone();
        let closing = match existing {
            Some(closing) => closing,
            None => {
                self.inner.lifetime.abort();
                let active = self.inner.running.borrow().as_ref().map(|running| running.done.clone());
                let future: Shared<LocalBoxFuture<'static, ()>> = async move {
                    if let Some(mut active) = active {
                        let _ = active.wait_for(|done| *done).await;
                    }
                }
                .boxed_local()
                .shared();
                *self.inner.closing.borrow_mut() = Some(future.clone());
                future
            }
        };
        closing.await;
    }
}

#[async_trait(?Send)]
impl Kernel for AgentKernel {
    async fn run(&self, input: &str, signal: Signal, emit: KernelEmit) -> Result<TurnResult, RuntimeError> {
        AgentKernel::run(self, input, signal, emit).await
    }
    async fn run_with(&self, input: &str, pictures: Vec<Picture>, signal: Signal, emit: KernelEmit) -> Result<TurnResult, RuntimeError> {
        AgentKernel::run_with(self, input, pictures, signal, emit).await
    }
    async fn close(&self) {
        AgentKernel::close(self).await
    }
    fn has_checkpoint(&self) -> bool {
        true
    }
    fn checkpoint(&self) -> Result<KernelCheckpoint, RuntimeError> {
        AgentKernel::checkpoint(self).map(Into::into)
    }
    fn has_transcript(&self) -> bool {
        true
    }
    fn transcript(&self) -> Vec<TranscriptLine> {
        AgentKernel::transcript(self)
    }
    fn has_steer(&self) -> bool {
        true
    }
    fn steer(&self, text: &str) -> bool {
        AgentKernel::steer(self, text)
    }
    fn has_aside(&self) -> bool {
        true
    }
    async fn aside(&self, question: &str, signal: Signal, on_text: OnText) -> Result<String, RuntimeError> {
        AgentKernel::aside(self, question, signal, on_text).await
    }
}

/// Where a turn's finished steps end: a reply whose calls have no results yet is left out.
fn settled_end(messages: &[Message]) -> usize {
    match messages.last() {
        Some(Message::Assistant { content, .. }) if content.iter().any(|part| matches!(part, AssistantPart::ToolCall(_))) => {
            messages.len() - 1
        }
        _ => messages.len(),
    }
}

/// A reply's first call, running while it's written; `begun` is when its work started.
struct Early {
    begun: Rc<Cell<Option<f64>>>,
    call: Box<dyn StreamingCall>,
}

/// One turn's shared state: its signal, its listener, what it has said, and the call under way early.
struct Turn {
    inner: Rc<Inner>,
    abort: Abort,
    deliver: Rc<dyn Fn(KernelEvent)>,
    messages: Rc<RefCell<Vec<Message>>>,
    /// What this turn sends as the earlier conversation, fitted to the budget. It becomes the
    /// history only when the turn settles; fitting is deterministic, so the next turn sends the same.
    earlier: Rc<RefCell<Vec<Message>>>,
    /// A call already running while the model writes it (a plan whose first steps are under way).
    early: RefCell<HashMap<String, Rc<Early>>>,
    spoke: Cell<bool>,
    /// The words the step in progress has shown, and whether its answer broke off after they began.
    shown: RefCell<String>,
    broke_off: Cell<bool>,
}

async fn turn(inner: Rc<Inner>, first: Message, signal: Signal, emit: KernelEmit, state: Rc<Running>) -> Result<TurnResult, RuntimeError> {
    let failed = Controller::new();
    let abort = Abort::new(vec![signal.clone(), inner.lifetime.signal.clone(), failed.signal.clone()]);
    // A throwing listener must not leave a half-delivered turn in history.
    let deliver: Rc<dyn Fn(KernelEvent)> = {
        let abort = abort.clone();
        let failed = failed.clone();
        Rc::new(move |event| {
            if abort.is_cancelled() {
                return;
            }
            if emit(event).is_err() {
                failed.abort();
            }
        })
    };
    let turn = Turn {
        inner: inner.clone(),
        abort: abort.clone(),
        deliver,
        messages: Rc::new(RefCell::new(vec![first])),
        earlier: Rc::new(RefCell::new(inner.history.borrow().clone())),
        early: RefCell::new(HashMap::new()),
        spoke: Cell::new(false),
        shown: RefCell::new(String::new()),
        broke_off: Cell::new(false),
    };
    let usage = RefCell::new(Usage::default());
    let reported = Cell::new(false);
    let settled = |stop_reason: StopReason| TurnResult { stop_reason, usage: reported.get().then(|| usage.borrow().clone()) };
    // A side question sees this turn up to its last finished step: a call still waiting for its result is left out.
    *state.context.borrow_mut() = Some({
        let messages = turn.messages.clone();
        let earlier = turn.earlier.clone();
        Rc::new(move || {
            let messages = messages.borrow();
            let end = settled_end(&messages);
            earlier.borrow().iter().chain(&messages[..end]).cloned().collect()
        })
    });
    let outcome: Result<TurnResult, StepError> = async {
        let mut carried_on = false;
        for step in 0.. {
            if step == inner.max_steps {
                abort.check()?;
                turn.settle_history();
                return Ok(settled(StopReason::MaxSteps));
            }
            turn.fit_to_budget();
            let request = (inner.binding.prepare)(ModelRequest {
                instructions: inner.instructions.clone(),
                messages: turn.conversation(),
                tools: inner.specs.clone(),
                session_id: inner.session_id.clone(),
            });
            turn.early.borrow_mut().clear();
            let result = match turn.stream(request).await {
                Ok(result) => result,
                // An answer that broke off after its words began showing, with nothing that changes Live begun:
                // once a turn, Kumi carries on from the steps it finished and the words shown, and says so.
                Err(StepError::Model(error)) if !carried_on && turn.can_carry_on(&error) => {
                    carried_on = true;
                    (turn.deliver)(KernelEvent::Retry { reason: retry_reason(&error, &inner.binding.id), wait_ms: 0 });
                    turn.carry_on();
                    continue;
                }
                Err(error) => return Err(error),
            };
            add(&mut usage.borrow_mut(), &result.usage);
            reported.set(true);
            if !result.content.is_empty() {
                turn.messages.borrow_mut().push(Message::Assistant { content: result.content.clone(), provider_options: None });
            }
            if !result.calls.is_empty() {
                let (results, reply) = turn.execute(&result.calls).await?;
                turn.messages.borrow_mut().push(Message::Tool { content: results, provider_options: None });
                // A batch stopped partway is kept with what it finished (see `execute`); the turn stops here.
                abort.check()?;
                // The tools finished the request and said so: their reply is the answer, with no model call to
                // write one. A quiet call (a note kept, with final) adds nothing: it ends the turn only when
                // this reply already holds the model's answer; a model that kept a note first still gets to
                // answer.
                let answered = result.content.iter().any(|part| matches!(part, AssistantPart::Text(text) if !trim(&text.text).is_empty()));
                if let Some(reply) = reply {
                    if state.steering.borrow().is_empty() && (!reply.is_empty() || answered) {
                        if !reply.is_empty() {
                            (turn.deliver)(KernelEvent::Text {
                                text: if turn.spoke.get() { format!("\n\n{reply}") } else { reply.clone() },
                            });
                            turn.messages.borrow_mut().push(Message::assistant_text(reply));
                        }
                        abort.check()?;
                        turn.settle_history();
                        return Ok(settled(StopReason::Completed));
                    }
                }
            } else if state.steering.borrow().is_empty() {
                abort.check()?;
                turn.settle_history();
                return Ok(settled(StopReason::Completed));
            }
            let pending: Vec<String> = state.steering.borrow_mut().drain(..).collect();
            for text in pending {
                turn.messages.borrow_mut().push(Message::user_text(&text));
                (turn.deliver)(KernelEvent::Steer { text });
            }
        }
        unreachable!("a turn's steps end by returning")
    }
    .await;
    match outcome {
        Ok(result) => Ok(result),
        Err(error) => {
            // A reply that broke off mid-plan: nothing more starts, and what's under way finishes first.
            if !abort.is_cancelled() {
                let entries: Vec<Rc<Early>> = turn.early.borrow().values().cloned().collect();
                join_all(entries.iter().map(|entry| entry.call.abandon())).await;
            }
            turn.keep_finished();
            if signal.is_cancelled() || inner.lifetime.signal.is_cancelled() {
                return Ok(settled(StopReason::Cancelled));
            }
            if failed.signal.is_cancelled() {
                return Err(KumiError::new(
                    FailureKind::Output,
                    "Inference output could not be delivered; the rest of the answer was dropped.",
                )
                .into());
            }
            Err(describe_failure(&error.into_model_error(), &inner.binding.id).into())
        }
    }
}

/// A call's input as the model writes it.
trait InputStream {
    fn start(&self, id: &str, name: &str);
    fn delta(&self, id: &str, delta: &str);
}

struct TurnInput<'a> {
    turn: &'a Turn,
    calls: &'a Cell<usize>,
}

impl InputStream for TurnInput<'_> {
    fn start(&self, id: &str, name: &str) {
        let tool = self.turn.inner.tools.get(name).cloned();
        let calls = self.calls.get();
        self.calls.set(calls + 1);
        if calls == 0 {
            if let Some(tool) = tool {
                let begun = Rc::new(Cell::new(None));
                let on_start: Rc<dyn Fn()> = {
                    let begun = begun.clone();
                    let deliver = self.turn.deliver.clone();
                    let (id, name) = (id.to_string(), name.to_string());
                    Rc::new(move || {
                        if begun.get().is_some() {
                            return;
                        }
                        begun.set(Some(perf_now()));
                        deliver(KernelEvent::ToolStart { id: id.clone(), name: name.clone() });
                    })
                };
                if let Some(call) = tool.stream(self.turn.abort.signal(), on_start) {
                    self.turn.early.borrow_mut().insert(id.to_string(), Rc::new(Early { begun, call }));
                }
            }
        }
        (self.turn.deliver)(KernelEvent::ToolInput { id: id.to_string(), name: name.to_string() });
    }

    fn delta(&self, id: &str, delta: &str) {
        let entry = self.turn.early.borrow().get(id).cloned();
        if let Some(entry) = entry {
            entry.call.push(delta);
        }
    }
}

/// A tool's outcome as the kernel reads it: its words, whether it failed, and the images it showed.
struct Outcome {
    text: String,
    is_error: bool,
    images: Vec<ToolImage>,
}

impl Turn {
    fn conversation(&self) -> Vec<Message> {
        [self.earlier.borrow().as_slice(), self.messages.borrow().as_slice()].concat()
    }

    fn settle_history(&self) {
        *self.inner.history.borrow_mut() = [self.earlier.borrow().clone(), without_images(&self.messages.borrow())].concat();
    }

    fn fit_to_budget(&self) {
        let budget = self.inner.budget_now();
        let refitted = {
            let earlier = self.earlier.borrow();
            let messages = self.messages.borrow();
            let fitted = fit(&earlier, &messages, &budget);
            // Kumi just changed what came before. Some providers bind a model's reasoning to the exact
            // conversation it saw (Anthropic's current models refuse the request otherwise), so the
            // reasoning goes, once: what was said, called and read stays.
            fitted.changed().then(|| (without_reasoning(&fitted.history), without_reasoning(&fitted.turn)))
        };
        if let Some((earlier, messages)) = refitted {
            *self.earlier.borrow_mut() = earlier;
            *self.messages.borrow_mut() = messages;
        }
    }

    /// Whether a step that failed can carry on: its answer broke off (the connection, or the provider
    /// mid-answer) after its words began showing, and no call that changes Live had begun.
    fn can_carry_on(&self, error: &LanguageModelError) -> bool {
        let broke_off = retry_delay_ms(error, 0).is_some() || matches!(error, LanguageModelError::Other(_));
        self.broke_off.get() && broke_off && !self.abort.is_cancelled() && !self.early.borrow().values().any(|entry| entry.call.started())
    }

    /// The next request carries on: the words shown so far stay as the model's, with a note to go on
    /// from where they stopped.
    fn carry_on(&self) {
        self.early.borrow_mut().clear();
        let shown = self.shown.borrow().clone();
        let mut messages = self.messages.borrow_mut();
        if !trim(&shown).is_empty() {
            messages.push(Message::assistant_text(shown));
        }
        messages.push(Message::user_text(CARRY_ON_NOTE));
    }

    /// A stopped turn (cancelled, timed out or failed) keeps the steps it finished, each model reply
    /// with all its tool results, so the conversation says what those steps changed in Live. The
    /// step in progress goes; a turn that finished no tool round leaves no trace.
    fn keep_finished(&self) {
        let messages = self.messages.borrow();
        let finished = &messages[..settled_end(&messages)];
        if finished.iter().any(|message| matches!(message, Message::Tool { .. })) {
            *self.inner.history.borrow_mut() =
                [self.earlier.borrow().clone(), without_images(finished), vec![Message::assistant_text(STOPPED_NOTE)]].concat();
        }
    }

    /// One model call. Retries up to MAX_RETRIES times, only before any output has escaped this step: text shown, or a
    /// streaming call's work begun. A reply's first call whose tool can stream starts as it's written.
    async fn stream(&self, request: CallOptions) -> Result<StepResult, StepError> {
        for attempt in 0.. {
            let delivered = Cell::new(false);
            let calls = Cell::new(0usize);
            let input = TurnInput { turn: self, calls: &calls };
            self.shown.borrow_mut().clear();
            let on_text = |text: &str| {
                delivered.set(true);
                self.spoke.set(true);
                self.shown.borrow_mut().push_str(text);
                (self.deliver)(KernelEvent::Text { text: text.to_string() });
            };
            let attempted: Result<StepResult, StepError> = async {
                let mut call = request.clone();
                call.abort_signal = Some(self.abort.signal());
                crate::core::timing::effort(&call);
                let timed = crate::core::timing::model_call(&self.inner.binding.id);
                let parts = self.inner.binding.model.do_stream(call).await.map_err(StepError::Model)?;
                consume(crate::core::timing::timed(parts, timed), &self.abort, &on_text, Some(&input as &dyn InputStream)).await
            }
            .await;
            match attempted {
                Ok(result) => {
                    // A call that began but never arrived whole is settled, not left running.
                    let entries: Vec<(String, Rc<Early>)> =
                        self.early.borrow().iter().map(|(id, entry)| (id.clone(), entry.clone())).collect();
                    for (id, entry) in entries {
                        if result.calls.iter().any(|(call, _)| call.tool_call_id == id) {
                            continue;
                        }
                        entry.call.abandon().await;
                        self.early.borrow_mut().remove(&id);
                    }
                    return Ok(result);
                }
                Err(error) => {
                    let escaped = delivered.get() || self.early.borrow().values().any(|entry| entry.call.started());
                    self.broke_off.set(delivered.get());
                    let wait = match &error {
                        StepError::Model(model) if attempt < MAX_RETRIES && !escaped && !self.abort.is_cancelled() => {
                            retry_delay_ms(model, attempt).map(|wait| (wait, retry_reason(model, &self.inner.binding.id)))
                        }
                        _ => None,
                    };
                    let Some((wait, reason)) = wait else { return Err(error) };
                    (self.deliver)(KernelEvent::Retry { reason, wait_ms: wait.round() as u64 });
                    // Nothing began, so the retry starts clean.
                    let entries: Vec<Rc<Early>> = self.early.borrow().values().cloned().collect();
                    join_all(entries.iter().map(|entry| entry.call.abandon())).await;
                    self.early.borrow_mut().clear();
                    delay(wait, &self.abort).await?;
                }
            }
        }
        unreachable!("a step's attempts end by returning")
    }

    /// Runs a step's calls in order. `reply` is set when all succeeded and some finished the request, or
    /// every call was quiet (an empty reply: done, nothing to add) and said `final`; then no model reply
    /// follows. A call
    /// that started while it was written finishes with its whole input. Consecutive calls that only read
    /// away from Live run together (`TOGETHER`), their results kept in the reply's order.
    ///
    /// Stopped partway, a batch keeps the calls that finished, and each of the rest gets a result saying
    /// it was stopped (every call needs one), so the next request doesn't redo what's done. A batch
    /// stopped before any call finished goes, as before.
    async fn execute(&self, calls: &[(ToolCall, Option<JsonObject>)]) -> Result<(Vec<ToolPart>, Option<String>), StepError> {
        let mut results = Vec::new();
        let mut finished = 0;
        let mut replies: Vec<String> = Vec::new();
        let mut failed = false;
        let mut quiet = 0;
        while results.len() < calls.len() {
            if self.abort.is_cancelled() {
                return self.stopped(results, finished, calls);
            }
            let rest = &calls[results.len()..];
            // At most four at once: a reply can ask for many pages, and search services limit bursts.
            let together = rest.iter().take_while(|(call, _)| TOGETHER.contains(&call.tool_name.as_str())).take(4).count();
            let group = &rest[..together.max(1)];
            let ran = if group.len() > 1 {
                let began = perf_now();
                let ran = join_all(group.iter().map(|(call, input)| self.call(call, input, false))).await;
                crate::core::timing::tools(group.len() as u32, round(perf_now() - began).max(0.0) as u64);
                ran
            } else {
                vec![self.call(&group[0].0, &group[0].1, true).await]
            };
            for ((call, input), ran) in group.iter().zip(ran) {
                let outcome = match ran {
                    Ran::Stopped => {
                        // A read away from Live can't have changed it.
                        let text = if TOGETHER.contains(&call.tool_name.as_str()) { STOPPED_READING } else { STOPPED_WHILE_RUNNING };
                        Outcome { text: text.into(), is_error: true, images: Vec::new() }
                    }
                    Ran::Done { outcome, reply } => {
                        finished += 1;
                        match reply.filter(|_| !outcome.is_error) {
                            // A note kept ends the answer only beside the finished one: words such as
                            // "removing the test clip now" with a note mid-task aren't it (#180).
                            Some(words) if words.is_empty() => {
                                if input.as_ref().and_then(|input| input.get("final")) == Some(&Value::Bool(true)) {
                                    quiet += 1;
                                }
                            }
                            Some(words) => replies.push(words),
                            None => {}
                        }
                        outcome
                    }
                };
                failed |= outcome.is_error;
                results.push(ToolPart::ToolResult(ToolResultPart {
                    tool_call_id: call.tool_call_id.clone(),
                    tool_name: call.tool_name.clone(),
                    output: tool_output(outcome),
                    provider_options: None,
                }));
            }
        }
        if self.abort.is_cancelled() {
            return self.stopped(results, finished, calls);
        }
        let reply = if !failed && !replies.is_empty() {
            Some(replies.join("\n\n"))
        } else if !failed && quiet == calls.len() {
            Some(String::new())
        } else {
            None
        };
        Ok((results, reply))
    }

    /// One call, from its start to its result. `alone`: not one of calls running together, whose time
    /// is the group's, not the sum of theirs.
    async fn call(&self, call: &ToolCall, input: &Option<JsonObject>, alone: bool) -> Ran {
        let streamed = self.early.borrow().get(&call.tool_call_id).cloned();
        let begun = streamed.as_ref().and_then(|entry| entry.begun.get());
        let started = begun.unwrap_or_else(perf_now);
        if begun.is_none() {
            (self.deliver)(KernelEvent::ToolStart { id: call.tool_call_id.clone(), name: call.tool_name.clone() });
        }
        // A streamed call that hadn't begun (its first step waiting for the next, to batch them) begins in
        // finish: it's started now, so beginning there doesn't say so a second time.
        if let Some(entry) = &streamed {
            if entry.begun.get().is_none() {
                entry.begun.set(Some(started));
            }
        }
        let tool = self.inner.tools.get(&call.tool_name).cloned();
        let mut reply = None;
        let outcome = match tool {
            None => Outcome {
                text: format!("Unknown tool {}; use only the supplied tools.", quote(&head(&call.tool_name, 64))),
                is_error: true,
                images: Vec::new(),
            },
            Some(_) if input.is_none() && streamed.is_none() => {
                Outcome { text: "Tool arguments must be a JSON object.".to_string(), is_error: true, images: Vec::new() }
            }
            Some(tool) => {
                let input = input.clone();
                let signal = self.abort.signal();
                let work = async move {
                    match streamed {
                        Some(entry) => entry.call.finish(input).await,
                        None => tool.execute(input.unwrap_or_default(), signal).await,
                    }
                };
                match until_aborted(work, &self.abort).await {
                    Ok(result) => {
                        reply = result.reply.map(|words| head(trim(&words), MAX_REPLY));
                        Outcome { text: capped(result.text), is_error: result.is_error, images: result.images }
                    }
                    Err(_) if self.abort.is_cancelled() => {
                        // A call stopped while it ran still took its time: often the slowest of the turn.
                        self.timed(call, started, alone);
                        return Ran::Stopped;
                    }
                    Err(error) => Outcome { text: head(&error.to_string(), MAX_TOOL_ERROR), is_error: true, images: Vec::new() },
                }
            }
        };
        let elapsed_ms = self.timed(call, started, alone);
        (self.deliver)(KernelEvent::ToolEnd {
            id: call.tool_call_id.clone(),
            name: call.tool_name.clone(),
            is_error: outcome.is_error,
            elapsed_ms,
        });
        Ran::Done { outcome, reply }
    }

    /// A call's time, from `started`, added to the turn's: by its tool, and to the turn's tool time when
    /// it ran alone (calls run together add their group's).
    fn timed(&self, call: &ToolCall, started: f64, alone: bool) -> u64 {
        let elapsed_ms = round(perf_now() - started).max(0.0) as u64;
        crate::core::timing::tool_call(&call.tool_name, elapsed_ms);
        if alone {
            crate::core::timing::tool(elapsed_ms);
        }
        elapsed_ms
    }

    /// A batch's results once it's stopped: the finished calls' and those cut off while running, then a
    /// stopped result for each of the rest. None finished: the batch goes.
    fn stopped(
        &self,
        mut results: Vec<ToolPart>,
        finished: usize,
        calls: &[(ToolCall, Option<JsonObject>)],
    ) -> Result<(Vec<ToolPart>, Option<String>), StepError> {
        if finished == 0 {
            return Err(StepError::Aborted);
        }
        for (call, _) in &calls[results.len()..] {
            results.push(ToolPart::ToolResult(ToolResultPart {
                tool_call_id: call.tool_call_id.clone(),
                tool_name: call.tool_name.clone(),
                output: tool_output(Outcome { text: STOPPED_BEFORE_RUNNING.into(), is_error: true, images: Vec::new() }),
                provider_options: None,
            }));
        }
        Ok((results, None))
    }
}

/// Tools that only read, away from Live: the web, the library and its samples, Live's manual, earlier
/// conversations. Consecutive calls to them in one reply run together.
const TOGETHER: &[&str] = &["search_web", "read_web", "find_sounds", "find_presets", "my_sets", "live_manual", "search_conversations"];

/// What one call came to.
enum Ran {
    /// It finished, with its reply to the producer when it has one ("" when it's quiet).
    Done { outcome: Outcome, reply: Option<String> },
    /// Stop came while it ran.
    Stopped,
}

/// `delay(ms, { signal })`: a wait that ends early, as an abort, when the signal fires.
async fn delay(ms: f64, signal: &Abort) -> Result<(), StepError> {
    tokio::select! {
        biased;
        _ = signal.cancelled() => Err(StepError::Aborted),
        _ = tokio::time::sleep(Duration::from_secs_f64(ms / 1000.0)) => Ok(()),
    }
}

/// Longest a call's input or result runs in a side question's plain-words copy of the conversation.
const PLAIN_PART: usize = 4 * 1024;

/// The conversation as words only, for a side question: what was said, each tool call as "[called
/// name {input}]", each result as "[name returned: …]" (from the producer's side), reasoning and
/// replay metadata left out.
pub fn plain_words(messages: &[Message]) -> Vec<Message> {
    let clip = |text: &str| if utf16_len(text) > PLAIN_PART { format!("{}…", head(text, PLAIN_PART)) } else { text.to_string() };
    let said = |assistant: bool, text: String| -> Vec<Message> {
        if trim(&text).is_empty() {
            Vec::new()
        } else if assistant {
            vec![Message::assistant_text(text)]
        } else {
            vec![Message::user_text(text)]
        }
    };
    messages
        .iter()
        .flat_map(|message| match message {
            Message::System { .. } => Vec::new(),
            Message::Tool { content, .. } => said(
                false,
                content
                    .iter()
                    .map(|part| match part {
                        ToolPart::ToolResult(result) => format!("[{} returned: {}]", result.tool_name, clip(&output_words(&result.output))),
                        ToolPart::ToolApprovalResponse(_) => String::new(),
                    })
                    .filter(|line| !line.is_empty())
                    .collect::<Vec<_>>()
                    .join("\n"),
            ),
            Message::Assistant { content, .. } => said(
                true,
                content
                    .iter()
                    .map(|part| match part {
                        AssistantPart::Text(text) => text.text.clone(),
                        AssistantPart::ToolCall(call) => format!("[called {} {}]", call.tool_name, clip(&stringify(&call.input))),
                        _ => String::new(),
                    })
                    .filter(|line| !line.is_empty())
                    .collect::<Vec<_>>()
                    .join("\n"),
            ),
            Message::User { content, .. } => {
                said(false, content.iter().map(|part| if let UserPart::Text(text) = part { text.text.as_str() } else { "" }).collect())
            }
        })
        .collect()
}

fn output_words(output: &ToolResultOutput) -> String {
    match output {
        ToolResultOutput::Text { value, .. } | ToolResultOutput::ErrorText { value, .. } => value.clone(),
        ToolResultOutput::Json { value, .. } | ToolResultOutput::ErrorJson { value, .. } => stringify(value),
        ToolResultOutput::Content { value, .. } => value
            .iter()
            .filter_map(|item| if let ToolResultContentItem::Text { text, .. } = item { Some(text.as_str()) } else { None })
            .filter(|text| !text.is_empty())
            .collect::<Vec<_>>()
            .join("\n"),
        ToolResultOutput::ExecutionDenied { .. } => String::new(),
    }
}

/// Assemble one streamed response into replayable assistant content, preserving provider metadata.
async fn consume(
    mut parts: StreamParts,
    abort: &Abort,
    on_text: &dyn Fn(&str),
    input: Option<&dyn InputStream>,
) -> Result<StepResult, StepError> {
    enum Block {
        Text { text: String, metadata: Option<ProviderMetadata> },
        Reasoning { text: String, metadata: Option<ProviderMetadata> },
        ToolCall(ToolCall),
    }
    let mut blocks: Vec<Block> = Vec::new();
    let mut open: HashMap<String, usize> = HashMap::new();
    fn block<'b>(
        blocks: &'b mut Vec<Block>,
        open: &mut HashMap<String, usize>,
        reasoning: bool,
        id: &str,
        metadata: Option<ProviderMetadata>,
    ) -> &'b mut String {
        let key = format!("{}:{id}", if reasoning { "reasoning" } else { "text" });
        let index = *open.entry(key).or_insert_with(|| {
            blocks.push(if reasoning {
                Block::Reasoning { text: String::new(), metadata: None }
            } else {
                Block::Text { text: String::new(), metadata: None }
            });
            blocks.len() - 1
        });
        match &mut blocks[index] {
            Block::Text { text, metadata: current } | Block::Reasoning { text, metadata: current } => {
                if let Some(next) = metadata {
                    *current = Some(merge(current.take(), &next));
                }
                text
            }
            Block::ToolCall(_) => unreachable!("text and reasoning blocks are keyed apart from calls"),
        }
    }
    let mut finish: Option<(ModelUsage, FinishReason)> = None;
    loop {
        let next = tokio::select! {
            biased;
            _ = abort.cancelled() => return Err(StepError::Aborted),
            part = parts.next() => part,
        };
        abort.check()?;
        let Some(part) = next else { break };
        match part {
            StreamPart::TextStart { id, provider_metadata } | StreamPart::TextEnd { id, provider_metadata } => {
                block(&mut blocks, &mut open, false, &id, provider_metadata);
            }
            StreamPart::TextDelta { id, delta, provider_metadata } => {
                block(&mut blocks, &mut open, false, &id, provider_metadata).push_str(&delta);
                if !delta.is_empty() {
                    on_text(&delta);
                }
            }
            StreamPart::ReasoningStart { id, provider_metadata } | StreamPart::ReasoningEnd { id, provider_metadata } => {
                block(&mut blocks, &mut open, true, &id, provider_metadata);
            }
            StreamPart::ReasoningDelta { id, delta, provider_metadata } => {
                block(&mut blocks, &mut open, true, &id, provider_metadata).push_str(&delta);
            }
            StreamPart::ToolInputStart { id, tool_name, provider_executed, .. } => {
                if provider_executed != Some(true) {
                    if let Some(input) = input {
                        input.start(&id, &tool_name);
                    }
                }
            }
            StreamPart::ToolInputDelta { id, delta, .. } => {
                if !delta.is_empty() {
                    if let Some(input) = input {
                        input.delta(&id, &delta);
                    }
                }
            }
            StreamPart::ToolCall(call) => {
                if call.provider_executed != Some(true) {
                    blocks.push(Block::ToolCall(call));
                }
            }
            StreamPart::Finish { usage, finish_reason, .. } => finish = Some((usage, finish_reason)),
            StreamPart::Error { error } => return Err(StepError::Model(error)),
            _ => {}
        }
    }
    drop(parts);
    let Some((usage, finish_reason)) = finish else {
        return Err(kumi(FailureKind::Protocol, "The model response ended before it finished; the turn was discarded."));
    };
    match finish_reason.unified {
        FinishReasonUnified::Error => return Err(kumi(FailureKind::Provider, "The provider ended the response with an error.")),
        FinishReasonUnified::ContentFilter => {
            return Err(kumi(FailureKind::Provider, "The provider's content filter stopped the response."))
        }
        _ => {}
    }
    let mut content: Vec<AssistantPart> = Vec::new();
    let mut calls: Vec<(ToolCall, Option<JsonObject>)> = Vec::new();
    for item in blocks {
        match item {
            Block::ToolCall(call) => {
                let input = parse_arguments(&call.input);
                content.push(AssistantPart::ToolCall(ToolCallPart {
                    tool_call_id: call.tool_call_id.clone(),
                    tool_name: call.tool_name.clone(),
                    input: Value::Object(input.clone().unwrap_or_default()),
                    provider_executed: None,
                    provider_options: call.provider_metadata.clone(),
                }));
                calls.push((call, input));
            }
            Block::Text { text, metadata } => {
                if !text.is_empty() {
                    content.push(AssistantPart::Text(TextPart { text, provider_options: metadata }));
                }
            }
            // Reasoning may carry only encrypted/signature metadata; providers need it replayed verbatim.
            Block::Reasoning { text, metadata } => {
                if !text.is_empty() || metadata.is_some() {
                    content.push(AssistantPart::Reasoning(ReasoningPart { text, provider_options: metadata }));
                }
            }
        }
    }
    Ok(StepResult { content, calls, usage })
}

fn parse_arguments(raw: &str) -> Option<JsonObject> {
    if trim(raw).is_empty() {
        return Some(JsonObject::new());
    }
    match serde_json::from_str::<Value>(raw) {
        Ok(Value::Object(object)) => Some(object),
        _ => None,
    }
}

fn merge(current: Option<ProviderMetadata>, next: &ProviderMetadata) -> ProviderMetadata {
    let mut merged = current.unwrap_or_default();
    for (provider, values) in next {
        let entry = merged.entry(provider.clone()).or_insert_with(|| Value::Object(JsonObject::new()));
        match (entry, values) {
            (Value::Object(target), Value::Object(source)) => {
                for (key, value) in source {
                    target.insert(key.clone(), value.clone());
                }
            }
            (entry, values) => *entry = values.clone(),
        }
    }
    merged
}

fn add(total: &mut Usage, usage: &ModelUsage) {
    total.input_tokens += usage.input_tokens.total.unwrap_or(0.0);
    total.output_tokens += usage.output_tokens.total.unwrap_or(0.0);
    total.cache_read_tokens += usage.input_tokens.cache_read.unwrap_or(0.0);
    total.cache_write_tokens += usage.input_tokens.cache_write.unwrap_or(0.0);
}

fn restore(checkpoint: &KernelCheckpoint, model: &str, tools: &str) -> Result<Vec<Message>, RuntimeError> {
    if checkpoint.version != 1 {
        return Err(RuntimeError::plain("Unsupported checkpoint version."));
    }
    // TS: the messages were taken as they came; here they must be messages a model can read.
    let messages: Vec<Message> = serde_json::from_value(Value::Array(checkpoint.messages.clone()))
        .map_err(|_| RuntimeError::plain("Unsupported checkpoint version."))?;
    let messages = without_images(&messages);
    let Some(origin) = &checkpoint.origin else { return Ok(messages) };
    // Reasoning belongs to the model that wrote it, and Claude's to the tools it saw as well: another
    // model, or other tools, continue from the words, tool calls and results. (Older saves name only
    // the provider.)
    let same_model = origin == model || Some(origin.as_str()) == model.split('/').next();
    let same_tools = checkpoint.tools.as_deref().is_none_or(|key| key == tools);
    Ok(if same_model && same_tools { messages } else { without_reasoning(&messages) })
}

/// The conversation without the models' reasoning or any provider's replay metadata: what was said,
/// the tool calls and their results. A copy; messages left with nothing in them go.
pub fn without_reasoning(messages: &[Message]) -> Vec<Message> {
    messages
        .iter()
        .filter_map(|message| match message {
            Message::System { .. } => Some(message.clone()),
            Message::User { content, .. } => {
                let content: Vec<UserPart> = content
                    .iter()
                    .cloned()
                    .map(|mut part| {
                        *part.provider_options_mut() = None;
                        part
                    })
                    .collect();
                (!content.is_empty()).then_some(Message::User { content, provider_options: None })
            }
            Message::Assistant { content, .. } => {
                let content: Vec<AssistantPart> = content
                    .iter()
                    .filter(|part| !matches!(part, AssistantPart::Reasoning(_)))
                    .cloned()
                    .map(|mut part| {
                        *part.provider_options_mut() = None;
                        part
                    })
                    .collect();
                (!content.is_empty()).then_some(Message::Assistant { content, provider_options: None })
            }
            Message::Tool { content, .. } => {
                let content: Vec<ToolPart> = content
                    .iter()
                    .cloned()
                    .map(|mut part| {
                        *part.provider_options_mut() = None;
                        part
                    })
                    .collect();
                (!content.is_empty()).then_some(Message::Tool { content, provider_options: None })
            }
        })
        .collect()
}

/// A tool's words, cut to their opening past MAX_TOOL_RESULT, with how much went and how to see more.
fn capped(text: String) -> String {
    if text.len() <= MAX_TOOL_RESULT {
        return text;
    }
    format!(
        "{}\n[Kumi cut the rest of this result: it was {} KB, and one result carries {} KB. Ask for less of it (a narrower read, a page, a filter) to see more.]",
        opening(&text, MAX_TOOL_RESULT - 256),
        text.len() / 1024,
        MAX_TOOL_RESULT / 1024
    )
}

/// A tool's result as the model reads it: its words, then each image after its caption.
fn tool_output(outcome: Outcome) -> ToolResultOutput {
    if outcome.is_error {
        return ToolResultOutput::error_text(outcome.text);
    }
    let images: Vec<&ToolImage> = outcome
        .images
        .iter()
        .filter(|image| IMAGE_TYPES.contains(&image.media_type.as_str()) && !image.data.is_empty())
        .take(MAX_TOOL_IMAGES)
        .collect();
    if images.is_empty() {
        return ToolResultOutput::text(outcome.text);
    }
    let mut value = vec![ToolResultContentItem::Text { text: outcome.text, provider_options: None }];
    for image in images {
        if let Some(caption) = image.caption.as_deref().filter(|caption| !caption.is_empty()) {
            value.push(ToolResultContentItem::Text { text: head(caption, 1000), provider_options: None });
        }
        // A copy, as a plain byte array: a Buffer would measure (and save) as a list of numbers.
        value.push(ToolResultContentItem::File {
            data: FileData::Data { data: DataContent::Bytes(image.data.clone()) },
            media_type: image.media_type.clone(),
            filename: None,
            provider_options: None,
        });
    }
    ToolResultOutput::Content { value, provider_options: None }
}

/// A turn without the images its tools showed, each result saying how many it had: they served
/// that turn, and kept, they'd cost every later request (and a saved conversation) their size. The
/// reasoning written after the first goes too, since it was written seeing them. A plain copy when there were none.
pub fn without_images(messages: &[Message]) -> Vec<Message> {
    let cleared: Vec<Message> = put_away_images(messages, 0, PutAway::TurnEnded).iter().map(pictures_named).collect();
    let Some(first) = cleared.iter().zip(messages).position(|(after, before)| after != before) else { return messages.to_vec() };
    [cleared[..first].to_vec(), without_reasoning(&cleared[first..])].concat()
}

/// A message without the pictures the producer added to it, each named in its place: they went with
/// that request, and kept, every later one would carry them again.
fn pictures_named(message: &Message) -> Message {
    let Message::User { content, provider_options } = message else { return message.clone() };
    if !content.iter().any(|part| matches!(part, UserPart::File(_))) {
        return message.clone();
    }
    let content = content
        .iter()
        .map(|part| match part {
            UserPart::File(file) => UserPart::Text(TextPart::new(format!(
                "[The producer showed {} with this message; it went with that request only.]",
                file.filename.as_deref().unwrap_or("a picture")
            ))),
            other => other.clone(),
        })
        .collect();
    Message::User { content, provider_options: provider_options.clone() }
}

/// Stop waiting on abort without dropping the admitted execution: sent mutations still record
/// their outcome, and tools still run cleanup. Like the source Promise, it owns its dependencies
/// independently of the turn; late results never become kernel events or conversation history.
async fn until_aborted<T: 'static>(
    work: impl Future<Output = Result<T, RuntimeError>> + 'static,
    signal: &Abort,
) -> Result<T, RuntimeError> {
    // JS calls execute/finish before untilAborted sees the signal, even if tool-start cancelled it.
    let mut work = Box::pin(work);
    if let std::task::Poll::Ready(result) = futures::poll!(work.as_mut()) {
        return if signal.is_cancelled() { Err(RuntimeError::Aborted) } else { result };
    }
    // Dropping a JoinHandle detaches the task; it does not abort its future.
    let work = tokio::task::spawn_local(work);
    tokio::select! {
        biased;
        _ = signal.cancelled() => Err(RuntimeError::Aborted),
        result = work => match result {
            Ok(result) => result,
            Err(error) => std::panic::resume_unwind(error.into_panic()),
        },
    }
}
