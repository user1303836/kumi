use std::cell::{Cell, RefCell};
use std::future::Future;
use std::rc::Rc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use futures::channel::oneshot;
use futures::future::LocalBoxFuture;
use futures::stream::{self, StreamExt};
use kumi_common::abort::{Controller, Signal};
use kumi_common::js::json::stringify;
use kumi_runtime::ai::error::{ApiCallError, LanguageModelError};
use kumi_runtime::ai::types::{
    AssistantPart, CallOptions, DataContent, FileData, FinishReason, FinishReasonUnified, InputTokens, Message, OutputTokens,
    ProviderMetadata, StreamPart, StreamParts, TextPart, ToolCall, ToolPart, ToolResultContentItem, ToolResultOutput, Usage as ModelUsage,
    UserPart,
};
use kumi_runtime::core::contracts::{
    ChangeState, Integration, JsonObject, KernelCheckpoint, KernelEmit, KernelEvent, KernelTool, Picture, StopReason, StreamingCall,
    ToolImage, ToolResult, TranscriptLine, TranscriptRole, Usage,
};
use kumi_runtime::core::errors::{FailureKind, RuntimeError};
use kumi_runtime::integrations::ableton::{integration::Ableton, observation::ObservationHost, options::AbletonOptions};
use kumi_runtime::kernel::agent::{
    create_agent_kernel, plain_words, AgentKernel, AgentKernelOptions, LanguageModel, ModelBinding, CARRY_ON_NOTE, STOPPED_BEFORE_RUNNING,
    STOPPED_NOTE, STOPPED_READING, STOPPED_WHILE_RUNNING,
};
use kumi_runtime::kernel::budget::{transcript_of, ContextBudget, SHORTENED};
use kumi_runtime::mcp::{
    client::{McpEndpoint, StderrStatus},
    types::{CallToolResult, Implementation, ListToolsResult},
};
use serde::Serialize;
use serde_json::{json, Value};
use tokio::task::{spawn_local, LocalSet};
use tokio::time::sleep;

/// What a scripted model answers a call with: parts, a stream of its own, or a rejection.
enum Scripted {
    Parts(Vec<StreamPart>),
    Stream(StreamParts),
    Reject(LanguageModelError),
}

type Script = Rc<dyn Fn(&CallOptions, usize) -> Scripted>;

fn usage(input: f64, output: f64) -> ModelUsage {
    ModelUsage {
        input_tokens: InputTokens { total: Some(input), no_cache: Some(input - 1.0), cache_read: Some(1.0), cache_write: Some(0.0) },
        output_tokens: OutputTokens { total: Some(output), text: Some(output), reasoning: Some(0.0) },
        raw: None,
    }
}
fn finish(unified: FinishReasonUnified) -> StreamPart {
    let raw = serde_json::to_value(unified).unwrap().as_str().unwrap().to_string();
    StreamPart::Finish { usage: usage(3.0, 2.0), finish_reason: FinishReason { unified, raw: Some(raw) }, provider_metadata: None }
}
fn stop() -> StreamPart {
    finish(FinishReasonUnified::Stop)
}
fn tool_calls() -> StreamPart {
    finish(FinishReasonUnified::ToolCalls)
}
fn text_id(value: &str, id: &str) -> Vec<StreamPart> {
    vec![
        StreamPart::TextStart { id: id.into(), provider_metadata: None },
        StreamPart::TextDelta { id: id.into(), delta: value.into(), provider_metadata: None },
        StreamPart::TextEnd { id: id.into(), provider_metadata: None },
    ]
}
fn text(value: &str) -> Vec<StreamPart> {
    text_id(value, "t1")
}
/// `[...text(value), finish()]`.
fn answer(value: &str) -> Scripted {
    Scripted::Parts([text(value), vec![stop()]].concat())
}
fn call_id(name: &str, input: &str, id: &str) -> StreamPart {
    StreamPart::ToolCall(ToolCall {
        tool_call_id: id.into(),
        tool_name: name.into(),
        input: input.into(),
        provider_executed: None,
        dynamic: None,
        provider_metadata: None,
    })
}
fn call(name: &str, input: &str) -> StreamPart {
    call_id(name, input, "c1")
}
fn meta(value: Value) -> Option<ProviderMetadata> {
    match value {
        Value::Object(map) => Some(map),
        _ => unreachable!(),
    }
}
fn object(value: Value) -> JsonObject {
    match value {
        Value::Object(map) => map,
        _ => unreachable!(),
    }
}
fn js<T: Serialize + ?Sized>(value: &T) -> String {
    stringify(&serde_json::to_value(value).unwrap())
}
fn signal() -> Signal {
    Controller::new().signal
}
fn ignore() -> KernelEmit {
    Rc::new(|_| Ok(()))
}
fn user(text: &str) -> Message {
    Message::user_text(text)
}
fn assistant(text: &str) -> Message {
    Message::assistant_text(text)
}
async fn local<F: Future>(future: F) -> F::Output {
    LocalSet::new().run_until(future).await
}

struct ScriptedModel {
    script: Script,
    requests: Rc<RefCell<Vec<CallOptions>>>,
}

#[async_trait(?Send)]
impl LanguageModel for ScriptedModel {
    async fn do_stream(&self, options: CallOptions) -> Result<StreamParts, LanguageModelError> {
        let mut recorded = options.clone();
        recorded.abort_signal = None;
        self.requests.borrow_mut().push(recorded);
        let n = self.requests.borrow().len();
        match (self.script)(&options, n) {
            Scripted::Parts(parts) => Ok(stream::iter(parts).boxed_local()),
            Scripted::Stream(parts) => Ok(parts),
            Scripted::Reject(error) => Err(error),
        }
    }
}

#[derive(Default)]
struct Options {
    instructions: Option<String>,
    tools: Vec<Rc<dyn KernelTool>>,
    max_steps: Option<usize>,
    budget: Option<ContextBudget>,
    checkpoint: Option<KernelCheckpoint>,
}

struct Harness {
    kernel: AgentKernel,
    requests: Rc<RefCell<Vec<CallOptions>>>,
}

impl Harness {
    fn request(&self, index: usize) -> CallOptions {
        self.requests.borrow()[index].clone()
    }
    fn count(&self) -> usize {
        self.requests.borrow().len()
    }
    fn messages(&self) -> Vec<Message> {
        self.kernel.checkpoint().unwrap().messages
    }
    fn transcript_texts(&self) -> Vec<String> {
        self.kernel.transcript().into_iter().filter(|line| !line.text.is_empty()).map(|line| line.text).collect()
    }
}

fn try_harness(script: impl Fn(&CallOptions, usize) -> Scripted + 'static, options: Options) -> Result<Harness, RuntimeError> {
    let requests = Rc::new(RefCell::new(Vec::new()));
    let model = ScriptedModel { script: Rc::new(script), requests: requests.clone() };
    let binding = ModelBinding {
        id: "test/fixture".into(),
        model: Rc::new(model),
        prepare: Box::new(|request| CallOptions {
            prompt: request.messages,
            tools: (!request.tools.is_empty()).then_some(request.tools),
            ..CallOptions::default()
        }),
        budget: None,
    };
    let kernel = create_agent_kernel(AgentKernelOptions {
        conversation: None,
        binding,
        instructions: options.instructions.unwrap_or_else(|| "fixture instructions".into()),
        tools: options.tools,
        signal: signal(),
        checkpoint: options.checkpoint,
        max_steps: options.max_steps,
        budget: options.budget,
    })?;
    Ok(Harness { kernel, requests })
}
fn harness(script: impl Fn(&CallOptions, usize) -> Scripted + 'static, options: Options) -> Harness {
    try_harness(script, options).expect("a kernel")
}
fn collect() -> (Rc<RefCell<Vec<KernelEvent>>>, KernelEmit) {
    let events = Rc::new(RefCell::new(Vec::new()));
    let sink = events.clone();
    (
        events,
        Rc::new(move |event| {
            sink.borrow_mut().push(event);
            Ok(())
        }),
    )
}
fn types(events: &[KernelEvent]) -> Vec<&'static str> {
    events
        .iter()
        .map(|event| match event {
            KernelEvent::Text { .. } => "text",
            KernelEvent::ToolInput { .. } => "tool-input",
            KernelEvent::ToolStart { .. } => "tool-start",
            KernelEvent::ToolEnd { .. } => "tool-end",
            KernelEvent::Steer { .. } => "steer",
            KernelEvent::Retry { .. } => "retry",
        })
        .collect()
}
fn texts(events: &[KernelEvent]) -> Vec<String> {
    events.iter().filter_map(|event| if let KernelEvent::Text { text } = event { Some(text.clone()) } else { None }).collect()
}
/// A stream that emits some parts, then waits until the call is abandoned.
fn hanging(parts: Vec<StreamPart>) -> Scripted {
    Scripted::Stream(stream::iter(parts).chain(stream::pending()).boxed_local())
}

type Execute = Rc<dyn Fn(JsonObject, Signal) -> LocalBoxFuture<'static, Result<ToolResult, RuntimeError>>>;
type StreamFactory = Rc<dyn Fn(Signal, Rc<dyn Fn()>) -> Box<dyn StreamingCall>>;

struct FnTool {
    name: String,
    description: String,
    input_schema: JsonObject,
    execute: Execute,
    stream: Option<StreamFactory>,
}

#[async_trait(?Send)]
impl KernelTool for FnTool {
    fn name(&self) -> &str {
        &self.name
    }
    fn description(&self) -> &str {
        &self.description
    }
    fn input_schema(&self) -> JsonObject {
        self.input_schema.clone()
    }
    async fn execute(&self, input: JsonObject, signal: Signal) -> Result<ToolResult, RuntimeError> {
        (self.execute)(input, signal).await
    }
    fn stream(&self, signal: Signal, on_start: Rc<dyn Fn()>) -> Option<Box<dyn StreamingCall>> {
        self.stream.as_ref().map(|factory| factory(signal, on_start))
    }
}

fn tool<F, Fut>(name: &str, execute: F) -> Rc<dyn KernelTool>
where
    F: Fn(JsonObject) -> Fut + 'static,
    Fut: Future<Output = Result<ToolResult, RuntimeError>> + 'static,
{
    Rc::new(FnTool {
        name: name.into(),
        description: format!("{name} fixture"),
        input_schema: object(json!({"type": "object", "properties": {}})),
        execute: Rc::new(move |input, _signal| Box::pin(execute(input))),
        stream: None,
    })
}
/// A tool that always answers with these words.
fn saying(name: &str, text: &str) -> Rc<dyn KernelTool> {
    let text = text.to_string();
    tool(name, move |_| {
        let text = text.clone();
        async move { Ok(ToolResult::text(text)) }
    })
}
fn tool_message(message: &Message) -> &[ToolPart] {
    match message {
        Message::Tool { content, .. } => content,
        other => panic!("not a tool message: {other:?}"),
    }
}
fn output_type(part: &ToolPart) -> &'static str {
    match part {
        ToolPart::ToolResult(result) => match result.output {
            ToolResultOutput::Text { .. } => "text",
            ToolResultOutput::ErrorText { .. } => "error-text",
            ToolResultOutput::Content { .. } => "content",
            _ => "other",
        },
        ToolPart::ToolApprovalResponse(_) => "approval",
    }
}
fn kumi_error(error: RuntimeError) -> kumi_runtime::core::errors::KumiError {
    match error {
        RuntimeError::Kumi(error) => error,
        other => panic!("not a KumiError: {other:?}"),
    }
}

#[tokio::test]
async fn streams_text_reports_summed_usage_and_settles_the_turn_into_history() {
    local(async {
        let h = harness(
            |_, _| {
                Scripted::Parts(
                    [text("hel"), vec![StreamPart::TextDelta { id: "t1".into(), delta: "lo".into(), provider_metadata: None }, stop()]]
                        .concat(),
                )
            },
            Options::default(),
        );
        let (events, emit) = collect();
        let result = h.kernel.run("hi", signal(), emit).await.unwrap();
        assert_eq!(*events.borrow(), vec![KernelEvent::Text { text: "hel".into() }, KernelEvent::Text { text: "lo".into() }]);
        assert_eq!(result.stop_reason, StopReason::Completed);
        assert_eq!(result.usage, Some(Usage { input_tokens: 3.0, output_tokens: 2.0, cache_read_tokens: 1.0, cache_write_tokens: 0.0 }));
        assert_eq!(h.messages(), vec![user("hi"), assistant("hello")]);
        h.kernel.close().await;
    })
    .await
}

#[tokio::test]
async fn pictures_the_producer_added_go_beside_their_words_in_the_request() {
    local(async {
        let h = harness(|_, _| answer("a warm pad"), Options::default());
        let picture = Picture { name: "synth.png".into(), media_type: "image/png".into(), data: vec![137, 80, 78, 71] };
        h.kernel.run_with("make this", vec![picture], signal(), ignore()).await.unwrap();
        let Message::User { content, .. } = &h.request(0).prompt[0] else { panic!("the producer's message comes first") };
        assert_eq!(content[0], UserPart::Text(TextPart::new("make this")));
        let UserPart::File(file) = &content[1] else { panic!("the picture follows the words") };
        assert_eq!((file.filename.as_deref(), file.media_type.as_str()), (Some("synth.png"), "image/png"));
        assert_eq!(file.data, FileData::Data { data: DataContent::Bytes(vec![137, 80, 78, 71]) });
        h.kernel.close().await;
    })
    .await
}

#[tokio::test]
async fn a_picture_goes_with_its_own_request_only_and_is_named_after() {
    local(async {
        let h = harness(|_, _| answer("A warm pad."), Options::default());
        let picture = Picture { name: "synth.png".into(), media_type: "image/png".into(), data: vec![137, 80, 78, 71] };
        h.kernel.run_with("make this", vec![picture], signal(), ignore()).await.unwrap();
        h.kernel.run("now brighter", signal(), ignore()).await.unwrap();
        let later = js(&h.request(1).prompt);
        assert!(!later.contains("\"type\":\"file\""), "{later}");
        assert!(later.contains("The producer showed synth.png with this message"), "{later}");
        h.kernel.close().await;
    })
    .await
}

#[tokio::test]
async fn a_conversation_keeps_its_prompt_cache_key_across_kernels_and_restarts() {
    local(async {
        let keys = Rc::new(RefCell::new(Vec::<String>::new()));
        for conversation in [Some("k3j2h1abc"), Some("k3j2h1abc"), Some("other123"), None, None] {
            let keys = keys.clone();
            let model = ScriptedModel { script: Rc::new(|_, _| answer("ok")), requests: Rc::new(RefCell::new(Vec::new())) };
            let kernel = create_agent_kernel(AgentKernelOptions {
                conversation: conversation.map(str::to_owned),
                binding: ModelBinding {
                    id: "test/fixture".into(),
                    model: Rc::new(model),
                    prepare: Box::new(move |request| {
                        keys.borrow_mut().push(request.session_id.clone());
                        CallOptions { prompt: request.messages, ..CallOptions::default() }
                    }),
                    budget: None,
                },
                instructions: "fixture instructions".into(),
                tools: vec![],
                signal: signal(),
                checkpoint: None,
                max_steps: None,
                budget: None,
            })
            .unwrap();
            kernel.run("hi", signal(), ignore()).await.unwrap();
            kernel.close().await;
        }
        let keys = keys.borrow();
        assert_eq!(keys[0], keys[1], "the same conversation, carried on");
        assert_ne!(keys[0], keys[2], "another conversation");
        assert_ne!(keys[3], keys[4], "kernels without a conversation");
        assert!(uuid::Uuid::parse_str(&keys[0]).is_ok());
    })
    .await
}

#[tokio::test]
async fn runs_tool_calls_between_model_steps_and_feeds_results_back() {
    local(async {
        let seen = Rc::new(RefCell::new(Vec::new()));
        let seen_by_tool = seen.clone();
        let h = harness(
            |_, n| if n == 1 { Scripted::Parts(vec![call("lookup", "{\"key\":\"tempo\"}"), tool_calls()]) } else { answer("120 BPM") },
            Options {
                tools: vec![tool("lookup", move |input| {
                    let seen = seen_by_tool.clone();
                    async move {
                        seen.borrow_mut().push(input);
                        Ok(ToolResult::text("{\"tempo\":120}"))
                    }
                })],
                ..Options::default()
            },
        );
        let (events, emit) = collect();
        let result = h.kernel.run("tempo?", signal(), emit).await.unwrap();
        assert_eq!(*seen.borrow(), vec![object(json!({"key": "tempo"}))]);
        assert_eq!(types(&events.borrow()), ["tool-start", "tool-end", "text"]);
        assert_eq!(result.usage.unwrap().input_tokens, 6.0);
        assert_eq!(h.request(0).tools.unwrap()[0].name, "lookup");
        let prompt = h.request(1).prompt;
        assert_eq!(
            js(&prompt[1..]),
            js(&json!([
                { "role": "assistant", "content": [{ "type": "tool-call", "toolCallId": "c1", "toolName": "lookup", "input": { "key": "tempo" } }] },
                { "role": "tool", "content": [{ "type": "tool-result", "toolCallId": "c1", "toolName": "lookup", "output": { "type": "text", "value": "{\"tempo\":120}" } }] },
            ]))
        );
        h.kernel.close().await;
    })
    .await
}

#[tokio::test]
async fn replays_provider_metadata_encrypted_reasoning_signatures_item_ids_verbatim_on_the_next_request() {
    local(async {
        let h = harness(
            |_, n| {
                if n > 1 {
                    return answer("second");
                }
                Scripted::Parts(vec![
                    StreamPart::ReasoningStart { id: "r1".into(), provider_metadata: meta(json!({"openai": {"itemId": "rs_1"}})) },
                    StreamPart::ReasoningDelta { id: "r1".into(), delta: String::new(), provider_metadata: meta(json!({"anthropic": {"signature": "sig"}})) },
                    StreamPart::ReasoningEnd { id: "r1".into(), provider_metadata: meta(json!({"openai": {"itemId": "rs_1", "reasoningEncryptedContent": "enc"}})) },
                    StreamPart::TextStart { id: "m1".into(), provider_metadata: meta(json!({"openai": {"itemId": "msg_1"}})) },
                    StreamPart::TextDelta { id: "m1".into(), delta: "first".into(), provider_metadata: None },
                    StreamPart::TextEnd { id: "m1".into(), provider_metadata: None },
                    StreamPart::ReasoningStart { id: "empty".into(), provider_metadata: None },
                    StreamPart::ReasoningEnd { id: "empty".into(), provider_metadata: None },
                    stop(),
                ])
            },
            Options::default(),
        );
        h.kernel.run("one", signal(), ignore()).await.unwrap();
        h.kernel.run("two", signal(), ignore()).await.unwrap();
        assert_eq!(
            js(&h.request(1).prompt[1]),
            js(&json!({ "role": "assistant", "content": [
                { "type": "reasoning", "text": "", "providerOptions": { "openai": { "itemId": "rs_1", "reasoningEncryptedContent": "enc" }, "anthropic": { "signature": "sig" } } },
                { "type": "text", "text": "first", "providerOptions": { "openai": { "itemId": "msg_1" } } },
            ] }))
        );
        h.kernel.close().await;
    })
    .await
}

#[tokio::test]
async fn cancellation_mid_stream_returns_promptly_discards_the_turn_and_the_next_turn_recovers() {
    local(async {
        let h = harness(|_, n| if n == 1 { hanging(text("partial")[..2].to_vec()) } else { answer("recovered") }, Options::default());
        let controller = Controller::new();
        let aborter = controller.clone();
        let started = Instant::now();
        let running = h.kernel.run(
            "long",
            controller.signal.clone(),
            Rc::new(move |event| {
                if matches!(event, KernelEvent::Text { .. }) {
                    aborter.abort();
                }
                Ok(())
            }),
        );
        assert_eq!(running.await.unwrap().stop_reason, StopReason::Cancelled);
        assert!(started.elapsed() < Duration::from_secs(1));
        assert!(h.messages().is_empty());
        let (events, emit) = collect();
        assert_eq!(h.kernel.run("again", signal(), emit).await.unwrap().stop_reason, StopReason::Completed);
        assert_eq!(*events.borrow(), vec![KernelEvent::Text { text: "recovered".into() }]);
        assert_eq!(h.request(1).prompt.len(), 1, "cancelled turn must not be replayed");
        h.kernel.close().await;
    })
    .await
}

#[tokio::test]
async fn cancellation_during_an_uncooperative_tool_settles_without_waiting_for_it_late_results_are_ignored() {
    local(async {
        let (release, held) = tokio::sync::watch::channel(false);
        let began = Rc::new(Cell::new(false));
        let started = began.clone();
        let settled = Rc::new(tokio::sync::Notify::new());
        let finished = settled.clone();
        let h = harness(
            |_, _| Scripted::Parts(vec![call("slow", "{}"), tool_calls()]),
            Options {
                tools: vec![tool("slow", move |_| {
                    let mut held = held.clone();
                    let started = started.clone();
                    let finished = finished.clone();
                    async move {
                        started.set(true);
                        let _ = held.wait_for(|released| *released).await;
                        finished.notify_one();
                        Ok(ToolResult::text("late"))
                    }
                })],
                ..Options::default()
            },
        );
        let controller = Controller::new();
        let aborter = controller.clone();
        let events = Rc::new(RefCell::new(Vec::new()));
        let sink = events.clone();
        let result = h
            .kernel
            .run(
                "go",
                controller.signal.clone(),
                Rc::new(move |event| {
                    let starting = matches!(event, KernelEvent::ToolStart { .. });
                    sink.borrow_mut().push(event);
                    if starting {
                        aborter.abort();
                    }
                    Ok(())
                }),
            )
            .await
            .unwrap();
        assert!(began.get(), "source calls execute before checking the cancellation raised by tool-start");
        release.send_replace(true);
        tokio::time::timeout(Duration::from_secs(1), settled.notified()).await.unwrap();
        assert_eq!(result.stop_reason, StopReason::Cancelled);
        assert_eq!(
            result.usage,
            Some(Usage { input_tokens: 3.0, output_tokens: 2.0, cache_read_tokens: 1.0, cache_write_tokens: 0.0 }),
            "usage reported before cancellation is kept"
        );
        assert_eq!(types(&events.borrow()), ["tool-start"]);
        assert!(h.messages().is_empty());
        h.kernel.close().await;
    })
    .await
}

#[tokio::test]
async fn a_turns_timing_has_the_effort_its_model_asked_for_and_each_tools_time_a_stopped_ones_too() {
    use kumi_runtime::core::timing;
    local(async {
        let model = ScriptedModel {
            script: Rc::new(|_, n| match n {
                1 => Scripted::Parts(vec![call_id("quick", "{}", "a"), call_id("slow", "{}", "b"), tool_calls()]),
                _ => Scripted::Parts(vec![call_id("stuck", "{}", "c"), tool_calls()]),
            }),
            requests: Rc::new(RefCell::new(Vec::new())),
        };
        let pause = |name: &str, ms: u64| {
            tool(name, move |_| async move {
                sleep(Duration::from_millis(ms)).await;
                Ok(ToolResult::text("done"))
            })
        };
        let kernel = create_agent_kernel(AgentKernelOptions {
            conversation: None,
            binding: ModelBinding {
                id: "openai-codex/fixture".into(),
                model: Rc::new(model),
                // As the provider asks: its options carry the effort and the service tier.
                prepare: Box::new(|request| CallOptions {
                    prompt: request.messages,
                    tools: (!request.tools.is_empty()).then_some(request.tools),
                    provider_options: Some(serde_json::Map::from_iter([(
                        "openai".to_string(),
                        json!({"reasoningEffort":"xhigh","serviceTier":"priority"}),
                    )])),
                    ..CallOptions::default()
                }),
                budget: None,
            },
            instructions: "fixture instructions".into(),
            tools: vec![pause("quick", 10), pause("slow", 120), pause("stuck", 60_000)],
            signal: signal(),
            checkpoint: None,
            max_steps: None,
            budget: None,
        })
        .unwrap();
        let controller = Controller::new();
        let aborter = controller.clone();
        let recorder = timing::begin();
        let result = kernel
            .run(
                "go",
                controller.signal.clone(),
                Rc::new(move |event| {
                    if matches!(&event, KernelEvent::ToolStart { name, .. } if name == "stuck") {
                        let aborter = aborter.clone();
                        spawn_local(async move {
                            sleep(Duration::from_millis(60)).await;
                            aborter.abort();
                        });
                    }
                    Ok(())
                }),
            )
            .await
            .unwrap();
        assert_eq!(result.stop_reason, StopReason::Cancelled);
        let line = timing::line(&recorder.finish(), 0, json!("cancelled"), None);
        assert_eq!((&line["effort"], &line["tier"]), (&json!("xhigh"), &json!("priority")), "{line}");
        let took = |name: &str| {
            line["slowTools"]
                .as_array()
                .unwrap()
                .iter()
                .find(|tool| tool["tool"] == name)
                .map(|tool| (tool["calls"].clone(), tool["ms"].as_u64().unwrap()))
        };
        assert!(took("slow").is_some_and(|(calls, ms)| calls == 1 && ms >= 120), "{line}");
        assert!(took("stuck").is_some_and(|(calls, ms)| calls == 1 && ms >= 60), "a call stopped while it ran keeps its time: {line}");
        assert!(took("quick").is_some_and(|(calls, _)| calls == 1), "{line}");
        assert!(line["toolMs"].as_u64().unwrap() >= 190, "{line}");
        kernel.close().await;
    })
    .await
}

#[tokio::test]
async fn an_already_aborted_turn_makes_no_request_concurrent_turns_are_rejected_close_is_idempotent_and_aborts_work() {
    local(async {
        let h = Rc::new(harness(|_, _| hanging(Vec::new()), Options::default()));
        let aborted = Controller::new();
        aborted.abort();
        let result = h.kernel.run("never", aborted.signal.clone(), ignore()).await.unwrap();
        assert_eq!(result.stop_reason, StopReason::Cancelled);
        assert_eq!(result.usage, None);
        assert_eq!(h.count(), 0);
        let held = h.clone();
        let running = spawn_local(async move { held.kernel.run("hold", signal(), ignore()).await });
        sleep(Duration::from_millis(5)).await;
        assert!(h.kernel.run("second", signal(), ignore()).await.unwrap_err().to_string().contains("busy"));
        assert!(h.kernel.checkpoint().unwrap_err().to_string().contains("busy"));
        futures::join!(h.kernel.close(), h.kernel.close());
        assert_eq!(running.await.unwrap().unwrap().stop_reason, StopReason::Cancelled);
        assert!(h.kernel.run("after", signal(), ignore()).await.unwrap_err().to_string().contains("closed"));
    })
    .await
}

#[tokio::test]
async fn incomplete_errored_and_filtered_responses_fail_instead_of_completing_empty() {
    local(async {
        let scripts: Vec<Vec<StreamPart>> = vec![
            text("no finish"),
            [text("x"), vec![StreamPart::Error { error: LanguageModelError::other("Bearer secret-token") }]].concat(),
            vec![finish(FinishReasonUnified::Error)],
            vec![StreamPart::Finish {
                usage: usage(3.0, 2.0),
                finish_reason: FinishReason { unified: FinishReasonUnified::ContentFilter, raw: Some("x".into()) },
                provider_metadata: None,
            }],
        ];
        for parts in scripts {
            let h = harness(move |_, _| Scripted::Parts(parts.clone()), Options::default());
            let error = kumi_error(h.kernel.run("q", signal(), ignore()).await.unwrap_err());
            assert!(!error.message.contains("secret-token"));
            assert!(h.messages().is_empty());
            h.kernel.close().await;
        }
    })
    .await
}

fn failure(status_code: u16) -> ApiCallError {
    let mut error = ApiCallError::new(
        if status_code == 400 { "Unsupported parameter: max_output_tokens" } else { "Bearer leaked-token" },
        "https://example.invalid",
        Some(json!({ "authorization": "Bearer leaked-token" })),
        Some(status_code),
    );
    error.response_body = Some(String::new());
    error.is_retryable = false;
    error
}

#[tokio::test]
async fn provider_http_failures_become_kumi_written_messages_without_credentials_or_bodies() {
    local(async {
        // Each says what went wrong, and its kind and provider let the app offer the fix.
        for (status, kind, pattern) in [
            (401, FailureKind::Auth, "test didn't accept Kumi's sign-in (HTTP 401): sign in again"),
            (403, FailureKind::Auth, "can't use fixture (HTTP 403)"),
            (402, FailureKind::Billing, "billing"),
            (404, FailureKind::Model, "doesn't offer fixture to this sign-in (HTTP 404); choose another model"),
            (429, FailureKind::RateLimit, "limit"),
            (529, FailureKind::Provider, "overloaded"),
            (400, FailureKind::Request, "turned the request down (HTTP 400): Unsupported parameter: max_output_tokens"),
        ] {
            let h = harness(move |_, _| Scripted::Reject(LanguageModelError::ApiCall(failure(status))), Options::default());
            let error = kumi_error(h.kernel.run("q", signal(), ignore()).await.unwrap_err());
            assert!(error.message.contains(pattern), "{status}: {}", error.message);
            assert!(!error.message.contains("leaked-token"));
            assert_eq!(error.kind, kind);
            assert_eq!(error.provider.as_deref(), Some("test"));
            h.kernel.close().await;
        }
    })
    .await
}

fn unavailable() -> LanguageModelError {
    let mut error = ApiCallError::new("down", "u", Some(json!({})), Some(503));
    error.is_retryable = true;
    error.response_headers = Some([("retry-after-ms".to_string(), "1".to_string())].into_iter().collect());
    LanguageModelError::ApiCall(error)
}

#[test]
fn a_retrys_reason_says_what_happened() {
    use kumi_runtime::kernel::failure::retry_reason;
    let status = |code: Option<u16>| LanguageModelError::ApiCall(ApiCallError::new("x", "u", Some(json!({})), code));
    let said: Vec<String> = [Some(200), Some(429), Some(503), Some(502), Some(408), None]
        .into_iter()
        .map(|code| retry_reason(&status(code), "openai-codex/gpt-6-astra"))
        .collect();
    assert_eq!(
        said,
        [
            "ChatGPT's answer broke off",
            "ChatGPT is busy (HTTP 429)",
            "ChatGPT is overloaded (HTTP 503)",
            "ChatGPT is having trouble (HTTP 502)",
            "ChatGPT asked Kumi to wait (HTTP 408)",
            "ChatGPT couldn't be reached",
        ]
    );
}

#[tokio::test]
async fn retries_up_to_three_times_before_any_output_escapes_but_never_after_text_was_delivered() {
    local(async {
        let retried = harness(|_, n| if n <= 3 { Scripted::Reject(unavailable()) } else { answer("ok") }, Options::default());
        let (events, emit) = collect();
        assert_eq!(retried.kernel.run("q", signal(), emit).await.unwrap().stop_reason, StopReason::Completed);
        assert_eq!(retried.count(), 4);
        // Each wait is shown, with why: the status line counts it down, from code rather than the model.
        let waits: Vec<_> = events
            .borrow()
            .iter()
            .filter_map(|event| match event {
                KernelEvent::Retry { reason, wait_ms } => Some((reason.clone(), *wait_ms)),
                _ => None,
            })
            .collect();
        assert_eq!(waits, vec![("test is overloaded (HTTP 503)".to_string(), 250); 3]);
        assert_eq!(types(&events.borrow()), ["retry", "retry", "retry", "text"]);
        let gave_up = harness(|_, _| Scripted::Reject(unavailable()), Options::default());
        assert!(gave_up.kernel.run("q", signal(), ignore()).await.unwrap_err().to_string().contains("overloaded right now"));
        assert_eq!(gave_up.count(), 4, "the first try and three more");
        gave_up.kernel.close().await;
        let streamed = harness(
            |_, _| Scripted::Parts([text("partial"), vec![StreamPart::Error { error: unavailable() }]].concat()),
            Options::default(),
        );
        assert!(streamed.kernel.run("q", signal(), ignore()).await.unwrap_err().to_string().contains("overloaded right now (HTTP 503)"));
        assert_eq!(streamed.count(), 2, "after words were shown it carries on once, not three times, then stops");
        retried.kernel.close().await;
        streamed.kernel.close().await;
    })
    .await
}

#[tokio::test]
async fn a_throwing_listener_discards_the_turn_without_poisoning_the_next_one() {
    local(async {
        let h = harness(|_, _| answer("x"), Options::default());
        let error = h.kernel.run("q", signal(), Rc::new(|_| Err(RuntimeError::plain("listener state")))).await.unwrap_err();
        assert!(error.to_string().contains("could not be delivered"), "{error}");
        assert!(h.messages().is_empty());
        assert_eq!(h.kernel.run("q", signal(), ignore()).await.unwrap().stop_reason, StopReason::Completed);
        h.kernel.close().await;
    })
    .await
}

#[tokio::test]
async fn unknown_tools_and_malformed_arguments_return_errors_to_the_model_without_executing_anything() {
    local(async {
        let executed = Rc::new(Cell::new(0));
        let counter = executed.clone();
        let h = harness(
            |_, n| {
                if n == 1 {
                    Scripted::Parts(vec![
                        call_id("missing", "{}", "a"),
                        call_id("lookup", "[1]", "b"),
                        call_id("lookup", "{bad", "c"),
                        tool_calls(),
                    ])
                } else {
                    answer("done")
                }
            },
            Options {
                tools: vec![tool("lookup", move |_| {
                    counter.set(counter.get() + 1);
                    async { Ok(ToolResult::text("never")) }
                })],
                ..Options::default()
            },
        );
        let (events, emit) = collect();
        h.kernel.run("q", signal(), emit).await.unwrap();
        assert_eq!(executed.get(), 0);
        let errors: Vec<bool> = events
            .borrow()
            .iter()
            .filter_map(|event| if let KernelEvent::ToolEnd { is_error, .. } = event { Some(*is_error) } else { None })
            .collect();
        assert_eq!(errors, [true, true, true]);
        let prompt = h.request(1).prompt;
        let results = tool_message(&prompt[2]);
        assert_eq!(results.iter().map(output_type).collect::<Vec<_>>(), ["error-text", "error-text", "error-text"]);
        h.kernel.close().await;
    })
    .await
}

#[tokio::test]
async fn tool_failures_are_reported_to_the_model_as_errors_and_the_turn_continues() {
    local(async {
        let h = harness(
            |_, n| if n == 1 { Scripted::Parts(vec![call("flaky", "{}"), tool_calls()]) } else { answer("recovered") },
            Options { tools: vec![tool("flaky", |_| async { Err(RuntimeError::plain("Live read failed")) })], ..Options::default() },
        );
        assert_eq!(h.kernel.run("q", signal(), ignore()).await.unwrap().stop_reason, StopReason::Completed);
        let prompt = h.request(1).prompt;
        assert_eq!(
            js(&tool_message(&prompt[2])[0]),
            js(&json!({ "type": "tool-result", "toolCallId": "c1", "toolName": "flaky", "output": { "type": "error-text", "value": "Live read failed" } }))
        );
        h.kernel.close().await;
    })
    .await
}

fn replying(name: &str, text: &str, reply: &str) -> Rc<dyn KernelTool> {
    let (text, reply) = (text.to_string(), reply.to_string());
    tool(name, move |_| {
        let (text, reply) = (text.clone(), reply.clone());
        async move { Ok(ToolResult { text, reply: Some(reply), ..ToolResult::default() }) }
    })
}

#[tokio::test]
async fn a_tool_that_finished_the_request_answers_for_the_model_the_turn_ends_with_its_reply_and_no_further_model_call() {
    local(async {
        let h = harness(
            |_, _| Scripted::Parts([text("On it."), vec![call("plan", "{}"), tool_calls()]].concat()),
            Options { tools: vec![replying("plan", "{\"done\":[1]}", "Done: Tempo 120 → 124 BPM.")], ..Options::default() },
        );
        let (events, emit) = collect();
        assert_eq!(h.kernel.run("faster", signal(), emit).await.unwrap().stop_reason, StopReason::Completed);
        assert_eq!(h.count(), 1, "no model call to write the answer");
        assert_eq!(texts(&events.borrow()), ["On it.", "\n\nDone: Tempo 120 → 124 BPM."]);
        assert_eq!(h.messages().last(), Some(&assistant("Done: Tempo 120 → 124 BPM.")), "the reply is the answer the next turn sees");
        h.kernel.close().await;
    })
    .await
}

#[tokio::test]
async fn a_reply_doesnt_end_the_turn_when_another_call_in_the_step_failed_or_guidance_is_waiting() {
    local(async {
        let failing = harness(
            |_, n| {
                if n == 1 {
                    Scripted::Parts(vec![call_id("plan", "{}", "c1"), call_id("read", "{}", "c2"), tool_calls()])
                } else {
                    answer("fixed")
                }
            },
            Options {
                tools: vec![replying("plan", "ok", "Done."), tool("read", |_| async { Ok(ToolResult::error("gone")) })],
                ..Options::default()
            },
        );
        failing.kernel.run("q", signal(), ignore()).await.unwrap();
        assert_eq!(failing.count(), 2, "the model sees the failure");
        failing.kernel.close().await;

        let kernel_ref: Rc<RefCell<Option<AgentKernel>>> = Rc::new(RefCell::new(None));
        let steering = kernel_ref.clone();
        let steered = harness(
            |_, n| if n == 1 { Scripted::Parts(vec![call("plan", "{}"), tool_calls()]) } else { answer("darker too") },
            Options {
                tools: vec![tool("plan", move |_| {
                    let kernel = steering.borrow().clone().unwrap();
                    async move {
                        kernel.steer("make it darker");
                        Ok(ToolResult { text: "ok".into(), reply: Some("Done.".into()), ..ToolResult::default() })
                    }
                })],
                ..Options::default()
            },
        );
        *kernel_ref.borrow_mut() = Some(steered.kernel.clone());
        let (events, emit) = collect();
        steered.kernel.run("q", signal(), emit).await.unwrap();
        assert_eq!(steered.count(), 2, "the model hears the guidance");
        assert!(!texts(&events.borrow()).iter().any(|text| text.contains("Done.")));
        steered.kernel.close().await;
    })
    .await
}

#[tokio::test]
async fn stops_at_the_step_bound_when_the_model_keeps_calling_tools() {
    local(async {
        let h = harness(
            |_, _| Scripted::Parts(vec![call("again", "{}"), tool_calls()]),
            Options { max_steps: Some(3), tools: vec![saying("again", "ok")], ..Options::default() },
        );
        let result = h.kernel.run("loop", signal(), ignore()).await.unwrap();
        assert_eq!(result.stop_reason, StopReason::MaxSteps);
        assert_eq!(h.count(), 3);
        h.kernel.close().await;
    })
    .await
}

#[tokio::test]
async fn a_side_question_sees_the_conversation_and_the_turn_so_far_calls_no_tools_and_leaves_no_trace() {
    local(async {
        let kernel_ref: Rc<RefCell<Option<AgentKernel>>> = Rc::new(RefCell::new(None));
        let heard = Rc::new(RefCell::new(Vec::new()));
        let answered = Rc::new(RefCell::new(None));
        let (asking, hearing, answers) = (kernel_ref.clone(), heard.clone(), answered.clone());
        let h = harness(
            |_, n| match n {
                1 => answer("fine"),
                2 => Scripted::Parts(vec![call("look", "{}"), tool_calls()]),
                3 => answer("About 2.4 s."),
                _ => answer("done"),
            },
            Options {
                tools: vec![tool("look", move |_| {
                    let kernel = asking.borrow().clone().unwrap();
                    let (hearing, answers) = (hearing.clone(), answers.clone());
                    async move {
                        let words = kernel
                            .aside("how long is the tail?", signal(), Rc::new(move |text| hearing.borrow_mut().push(text.to_string())))
                            .await
                            .unwrap();
                        *answers.borrow_mut() = Some(words);
                        Ok(ToolResult::text("looked"))
                    }
                })],
                ..Options::default()
            },
        );
        *kernel_ref.borrow_mut() = Some(h.kernel.clone());
        h.kernel.run("one", signal(), ignore()).await.unwrap();
        h.kernel.run("two", signal(), ignore()).await.unwrap();
        assert_eq!(answered.borrow().as_deref(), Some("About 2.4 s."));
        assert_eq!(*heard.borrow(), vec!["About 2.4 s.".to_string()]);
        let aside = h.request(2);
        assert!(aside.tools.is_none(), "no tools offered");
        let said: Vec<String> =
            aside.prompt.iter().map(|message| format!("{}: {}", js(&message.role()).trim_matches('"'), js(&content_of(message)))).collect();
        assert_eq!(said.len(), 4, "the conversation, the turn under way without its call still running, and the question");
        assert!(said[2].starts_with("user: ") && said[2].contains("two"), "{}", said[2]);
        assert!(
            said[3].starts_with("user: ") && said[3].contains("side question") && said[3].contains("how long is the tail?"),
            "{}",
            said[3]
        );
        assert!(aside.prompt.iter().all(|message| content_of(message).iter().all(|part| part["type"] == "text")), "words only");
        assert!(!js(&h.kernel.checkpoint().unwrap()).contains("tail"), "never kept");
        h.kernel.close().await;
    })
    .await
}

fn content_of(message: &Message) -> Vec<Value> {
    serde_json::to_value(message).unwrap()["content"].as_array().cloned().unwrap_or_default()
}

#[test]
fn a_side_questions_copy_of_the_conversation_writes_calls_and_results_out_as_words() {
    let words = plain_words(&[
        user("tempo?"),
        serde_json::from_value(json!({ "role": "assistant", "content": [{ "type": "reasoning", "text": "hmm" }, { "type": "text", "text": "Looking." }, { "type": "tool-call", "toolCallId": "c1", "toolName": "look", "input": { "what": "tempo" } }] })).unwrap(),
        serde_json::from_value(json!({ "role": "tool", "content": [{ "type": "tool-result", "toolCallId": "c1", "toolName": "look", "output": { "type": "text", "value": "{\"tempo\":124}" } }] })).unwrap(),
        assistant("124 BPM."),
    ]);
    let lines: Vec<(String, String)> = words
        .iter()
        .map(|message| (js(&message.role()).trim_matches('"').to_string(), content_of(message)[0]["text"].as_str().unwrap().to_string()))
        .collect();
    assert_eq!(
        lines,
        [
            ("user".to_string(), "tempo?".to_string()),
            ("assistant".into(), "Looking.\n[called look {\"what\":\"tempo\"}]".into()),
            ("user".into(), "[look returned: {\"tempo\":124}]".into()),
            ("assistant".into(), "124 BPM.".into()),
        ]
    );
}

#[tokio::test]
async fn steering_enters_at_the_next_model_boundary_even_after_a_final_answer() {
    local(async {
        let kernel_ref: Rc<RefCell<Option<AgentKernel>>> = Rc::new(RefCell::new(None));
        let steering = kernel_ref.clone();
        let h = harness(
            |_, n| match n {
                1 => Scripted::Parts(vec![call("look", "{}"), tool_calls()]),
                2 => answer("first"),
                _ => answer("steered"),
            },
            Options {
                tools: vec![tool("look", move |_| {
                    let kernel = steering.borrow().clone().unwrap();
                    async move {
                        assert!(kernel.steer("make it darker"));
                        Ok(ToolResult::text("ok"))
                    }
                })],
                ..Options::default()
            },
        );
        *kernel_ref.borrow_mut() = Some(h.kernel.clone());
        assert!(!h.kernel.steer("idle"));
        let events = Rc::new(RefCell::new(Vec::new()));
        let sink = events.clone();
        let kernel = h.kernel.clone();
        h.kernel
            .run(
                "build a pad",
                signal(),
                Rc::new(move |event| {
                    let first = matches!(&event, KernelEvent::Text { text } if text == "first");
                    sink.borrow_mut().push(event);
                    if first {
                        assert!(kernel.steer("and shorter"));
                    }
                    Ok(())
                }),
            )
            .await
            .unwrap();
        let steers: Vec<String> = events
            .borrow()
            .iter()
            .filter_map(|event| if let KernelEvent::Steer { text } = event { Some(text.clone()) } else { None })
            .collect();
        assert_eq!(steers, ["make it darker", "and shorter"]);
        assert_eq!(h.request(1).prompt.last(), Some(&user("make it darker")), "applied before step 2");
        assert_eq!(h.request(2).prompt.last(), Some(&user("and shorter")), "a final answer continues");
        assert_eq!(h.count(), 3);
        h.kernel.close().await;
    })
    .await
}

fn thinking(id: &str, metadata: Option<ProviderMetadata>, words: &str) -> Vec<StreamPart> {
    vec![
        StreamPart::ReasoningStart { id: id.into(), provider_metadata: metadata },
        StreamPart::ReasoningDelta { id: id.into(), delta: words.into(), provider_metadata: None },
        StreamPart::ReasoningEnd { id: id.into(), provider_metadata: None },
    ]
}

#[tokio::test]
async fn a_checkpoint_says_which_model_wrote_it_with_which_tools_another_model_or_other_tools_continue_from_a_portable_copy() {
    local(async {
        let first = harness(
            |_, _| {
                Scripted::Parts(
                    [
                        thinking("r1", meta(json!({"test": {"replay": "provider-only"}})), "thinking"),
                        text("Try a shorter release."),
                        vec![stop()],
                    ]
                    .concat(),
                )
            },
            Options::default(),
        );
        first
            .kernel
            .run(
                "How do I tame the snare?\n\n<current_observation_untrusted>\n{\"tempo\":120}\n</current_observation_untrusted>",
                signal(),
                ignore(),
            )
            .await
            .unwrap();
        let checkpoint: KernelCheckpoint = first.kernel.checkpoint().unwrap().into();
        assert_eq!(checkpoint.origin.as_deref(), Some("test/fixture"));
        let key = checkpoint.tools.clone().unwrap_or_default();
        assert!(key.len() == 22 && key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-'), "{key}");
        assert_eq!(
            first.kernel.transcript(),
            vec![
                TranscriptLine { role: TranscriptRole::User, text: "How do I tame the snare?".into(), tools: None },
                TranscriptLine { role: TranscriptRole::Assistant, text: "Try a shorter release.".into(), tools: None },
            ],
            "the producer's words, without the host's observation"
        );
        let replay = |origin: Option<&str>, tools: Vec<Rc<dyn KernelTool>>| {
            let mut checkpoint: KernelCheckpoint = serde_json::from_str(&serde_json::to_string(&checkpoint).unwrap()).unwrap();
            if let Some(origin) = origin {
                checkpoint.origin = Some(origin.into());
            }
            async move {
                let next = harness(|_, _| answer("ok"), Options { checkpoint: Some(checkpoint), tools, ..Options::default() });
                next.kernel.run("next", signal(), ignore()).await.unwrap();
                next.kernel.close().await;
                js(&next.request(0).prompt[1])
            }
        };
        assert!(replay(None, Vec::new()).await.contains("provider-only"), "the same model with the same tools replays everything");
        assert!(replay(Some("test"), Vec::new()).await.contains("provider-only"), "an older save names only the provider");
        for (origin, tools, why) in [
            (Some("elsewhere/model"), Vec::new(), "another provider"),
            (Some("test/other-model"), Vec::new(), "another model"),
            (None, vec![saying("lookup", "x")], "other tools"),
        ] {
            let replayed = replay(origin, tools).await;
            assert!(!replayed.contains("provider-only") && !replayed.contains("reasoning"), "{why}: {replayed}");
            assert!(replayed.contains("Try a shorter release"), "{why}");
        }
        first.kernel.close().await;
    })
    .await
}

fn observed(words: &str) -> String {
    format!("{words}\n\n<current_observation_untrusted>\n{{\"tempo\":120}}\n</current_observation_untrusted>")
}

#[tokio::test]
async fn when_the_budget_clears_earlier_turns_their_reasoning_goes_too_so_a_model_that_checks_its_history_never_sees_it_edited() {
    local(async {
        let big = stringify(&json!({ "items": "x".repeat(3000) }));
        let h = harness(
            |_, n| {
                if n % 2 == 1 {
                    Scripted::Parts(
                        [
                            thinking("r1", meta(json!({"anthropic": {"signature": "sig-1"}})), "hmm"),
                            vec![call_id("read", "{}", &format!("c{n}")), tool_calls()],
                        ]
                        .concat(),
                    )
                } else {
                    answer(&format!("answer {}", n / 2))
                }
            },
            Options {
                tools: vec![saying("read", &big)],
                budget: Some(ContextBudget { clear_at: 4096.0, limit: 64.0 * 1024.0 }),
                ..Options::default()
            },
        );
        h.kernel.run(&observed("one"), signal(), ignore()).await.unwrap();
        h.kernel.run(&observed("two"), signal(), ignore()).await.unwrap();
        assert!(js(&h.request(2).prompt).contains("sig-1"), "under the budget, reasoning is replayed as it was");
        h.kernel.run(&observed("three"), signal(), ignore()).await.unwrap();
        let cleared = js(&h.request(4).prompt);
        assert!(cleared.contains("Kumi cleared the rest"));
        assert!(!cleared.contains("sig-1") && !cleared.contains("\"reasoning\""));
        assert!(!js(&h.messages()[..4].to_vec()).contains("sig-1"), "and stays gone from the saved conversation");
        h.kernel.close().await;
    })
    .await
}

#[tokio::test]
async fn a_checkpoint_restores_settled_history_into_a_fresh_kernel() {
    local(async {
        let first = harness(|_, _| answer("remembered"), Options::default());
        first.kernel.run("marker-123", signal(), ignore()).await.unwrap();
        let checkpoint: KernelCheckpoint =
            serde_json::from_str(&serde_json::to_string(&first.kernel.checkpoint().unwrap()).unwrap()).unwrap();
        let second = harness(|_, _| answer("ok"), Options { checkpoint: Some(checkpoint.clone()), ..Options::default() });
        second.kernel.run("what marker?", signal(), ignore()).await.unwrap();
        assert_eq!(js(&second.request(0).prompt[..2].to_vec()), js(&checkpoint.messages));
        let unsupported = try_harness(
            |_, _| Scripted::Parts(Vec::new()),
            Options {
                checkpoint: Some(KernelCheckpoint { version: 2, messages: Vec::new(), origin: None, tools: None }),
                ..Options::default()
            },
        );
        assert!(unsupported.err().unwrap().to_string().contains("checkpoint"));
        first.kernel.close().await;
        second.kernel.close().await;
    })
    .await
}

#[tokio::test]
async fn long_conversations_stay_in_budget_earlier_reads_are_cleared_in_requests_and_the_checkpoint_and_the_transcript_keeps_every_word() {
    local(async {
        let big = stringify(&json!({ "items": "x".repeat(3000) }));
        // Each turn reads once, then answers.
        let h = harness(
            |_, n| if n % 2 == 1 { Scripted::Parts(vec![call_id("read", "{}", &format!("c{n}")), tool_calls()]) } else { answer(&format!("answer {}", n / 2)) },
            Options { tools: vec![saying("read", &big)], budget: Some(ContextBudget { clear_at: 4096.0, limit: 64.0 * 1024.0 }), ..Options::default() },
        );
        for words in ["one", "two", "three"] {
            h.kernel.run(&observed(words), signal(), ignore()).await.unwrap();
        }
        // The third turn's first request: turn one cleared, turn two whole.
        let prompt = h.request(4).prompt;
        assert_eq!(prompt[0], user("one"));
        assert!(js(&prompt[2]).contains("Kumi cleared the rest of this earlier result"));
        assert_eq!(
            js(&prompt[6]),
            js(&json!({ "role": "tool", "content": [{ "type": "tool-result", "toolCallId": "c3", "toolName": "read", "output": { "type": "text", "value": big } }] }))
        );
        assert!(js(&prompt[4]).contains("current_observation_untrusted"));
        assert!(js(&h.messages()[2]).contains("Kumi cleared the rest"));
        assert_eq!(h.transcript_texts(), ["one", "answer 1", "two", "answer 2", "three", "answer 3"]);
        let tools: Vec<String> = h.kernel.transcript().into_iter().flat_map(|line| line.tools.unwrap_or_default()).collect();
        assert_eq!(tools, ["read", "read", "read"], "each answer's steps come back with it");
        h.kernel.close().await;
    })
    .await
}

#[tokio::test]
async fn a_huge_tool_result_is_cut_to_its_opening_so_the_request_stays_in_budget() {
    local(async {
        let huge = "x".repeat(1024 * 1024);
        let h = harness(
            |_, n| if n == 1 { Scripted::Parts(vec![call_id("read", "{}", "c1"), tool_calls()]) } else { answer("done") },
            Options { tools: vec![saying("read", &huge)], ..Options::default() },
        );
        h.kernel.run(&observed("read it"), signal(), ignore()).await.unwrap();
        let sent = js(&h.request(1).prompt);
        assert!(sent.len() < 70 * 1024, "{} bytes went to the model", sent.len());
        assert!(sent.contains("Kumi cut the rest of this result: it was 1024 KB, and one result carries 64 KB."));
        h.kernel.close().await;
    })
    .await
}

#[derive(Clone, Copy, PartialEq)]
enum Mode {
    Answer,
    Fail,
    Hang,
    HangFirst,
}

fn roles(messages: &[Message]) -> Vec<String> {
    messages.iter().map(|message| js(&message.role()).trim_matches('"').to_string()).collect()
}

#[tokio::test]
async fn a_stopped_turn_keeps_the_steps_it_finished_with_a_note_the_step_in_progress_and_a_turn_that_finished_none_leave_no_trace() {
    local(async {
        let mode = Rc::new(Cell::new(Mode::Answer));
        let step = Rc::new(Cell::new(0));
        let (scripted_mode, scripted_step) = (mode.clone(), step.clone());
        let h = Rc::new(harness(
            move |_, _| {
                match scripted_mode.get() {
                    Mode::Answer => return answer("fine"),
                    Mode::HangFirst => return hanging(Vec::new()),
                    _ => {}
                }
                scripted_step.set(scripted_step.get() + 1);
                if scripted_step.get() % 2 == 1 {
                    return Scripted::Parts(vec![call_id("read", "{}", &format!("r{}", scripted_step.get())), tool_calls()]);
                }
                if scripted_mode.get() == Mode::Fail {
                    Scripted::Reject(LanguageModelError::other("provider down"))
                } else {
                    hanging(Vec::new())
                }
            },
            Options { tools: vec![saying("read", "{\"tempo\":120}")], ..Options::default() },
        ));
        h.kernel.run("one", signal(), ignore()).await.unwrap();
        mode.set(Mode::Fail);
        assert!(h.kernel.run("two", signal(), ignore()).await.is_err());
        assert_eq!(h.transcript_texts(), ["one", "fine", "two", STOPPED_NOTE]);
        assert_eq!(roles(&h.messages()[3..]), ["assistant", "tool", "assistant"], "the read and its result stay; the failed step goes");
        mode.set(Mode::Hang);
        let controller = Controller::new();
        let held = h.clone();
        let stop_signal = controller.signal.clone();
        let stopped = spawn_local(async move { held.kernel.run("three", stop_signal, ignore()).await });
        sleep(Duration::from_millis(20)).await;
        controller.abort();
        assert_eq!(stopped.await.unwrap().unwrap().stop_reason, StopReason::Cancelled);
        let texts = h.transcript_texts();
        assert_eq!(&texts[texts.len() - 2..], ["three", STOPPED_NOTE]);
        let settled = h.messages();
        mode.set(Mode::HangFirst);
        let early = Controller::new();
        let held = h.clone();
        let early_signal = early.signal.clone();
        let nothing = spawn_local(async move { held.kernel.run("four", early_signal, ignore()).await });
        sleep(Duration::from_millis(20)).await;
        early.abort();
        assert_eq!(nothing.await.unwrap().unwrap().stop_reason, StopReason::Cancelled);
        assert_eq!(h.messages(), settled, "no finished step, no trace");
        h.kernel.close().await;
    })
    .await
}

#[tokio::test]
async fn a_turn_stopped_during_a_tool_keeps_what_finished_before_it_never_a_call_without_its_result() {
    local(async {
        let calls = Rc::new(Cell::new(0));
        let counter = calls.clone();
        let h = Rc::new(harness(
            move |_, _| {
                counter.set(counter.get() + 1);
                if counter.get() == 1 {
                    Scripted::Parts(vec![call_id("read", "{}", "r1"), tool_calls()])
                } else {
                    Scripted::Parts(vec![call_id("slow", "{}", "s1"), tool_calls()])
                }
            },
            Options { tools: vec![saying("read", "ok"), tool("slow", |_| std::future::pending())], ..Options::default() },
        ));
        let controller = Controller::new();
        let held = h.clone();
        let stop_signal = controller.signal.clone();
        let stopped = spawn_local(async move { held.kernel.run("go", stop_signal, ignore()).await });
        sleep(Duration::from_millis(20)).await;
        controller.abort();
        assert_eq!(stopped.await.unwrap().unwrap().stop_reason, StopReason::Cancelled);
        let messages = h.messages();
        assert_eq!(roles(&messages), ["user", "assistant", "tool", "assistant"]);
        assert!(!js(&messages).contains("\"s1\""), "the slow call, which has no result, is gone");
        h.kernel.close().await;
    })
    .await
}

#[tokio::test]
async fn a_batch_stopped_on_its_second_call_keeps_the_first_calls_result_and_the_next_turn_sees_it() {
    local(async {
        let h = Rc::new(harness(
            |_, n| match n {
                1 => {
                    Scripted::Parts(vec![call_id("add", "{}", "a1"), call_id("slow", "{}", "s1"), call_id("add", "{}", "a2"), tool_calls()])
                }
                _ => answer("done"),
            },
            Options { tools: vec![saying("add", "Added Reverb"), tool("slow", |_| std::future::pending())], ..Options::default() },
        ));
        let controller = Controller::new();
        let held = h.clone();
        let stop_signal = controller.signal.clone();
        let stopped = spawn_local(async move { held.kernel.run("go", stop_signal, ignore()).await });
        sleep(Duration::from_millis(20)).await;
        controller.abort();
        assert_eq!(stopped.await.unwrap().unwrap().stop_reason, StopReason::Cancelled);
        let messages = h.messages();
        assert_eq!(roles(&messages), ["user", "assistant", "tool", "assistant"]);
        let results: Vec<_> = tool_message(&messages[2])
            .iter()
            .map(|part| match part {
                ToolPart::ToolResult(result) => (result.tool_call_id.as_str(), output_type(part), js(&result.output)),
                other => panic!("not a result: {other:?}"),
            })
            .collect();
        assert_eq!(
            results.iter().map(|(id, kind, _)| (*id, *kind)).collect::<Vec<_>>(),
            [("a1", "text"), ("s1", "error-text"), ("a2", "error-text")],
            "every call keeps a result"
        );
        assert!(results[0].2.contains("Added Reverb"), "the finished call's result stays");
        assert!(results[1].2.contains(STOPPED_WHILE_RUNNING) && results[2].2.contains(STOPPED_BEFORE_RUNNING));
        h.kernel.run("carry on", signal(), ignore()).await.unwrap();
        assert!(js(&h.request(1).prompt).contains("Added Reverb"), "the next request says the first change happened");
        h.kernel.close().await;
    })
    .await
}

#[tokio::test]
async fn independent_reads_away_from_live_run_together_in_order_and_one_failure_keeps_the_rest() {
    local(async {
        let slow = |name: &str, words: &'static str, fail: bool| {
            tool(name, move |_| async move {
                sleep(Duration::from_millis(300)).await;
                if fail {
                    Err(RuntimeError::plain("the site didn't answer"))
                } else {
                    Ok(ToolResult::text(words))
                }
            })
        };
        let h = harness(
            |_, n| match n {
                1 => Scripted::Parts(vec![
                    call_id("search_web", "{}", "w1"),
                    call_id("read_web", "{}", "w2"),
                    call_id("find_sounds", "{}", "w3"),
                    call_id("make_changes", "{}", "c1"),
                    call_id("search_web", "{}", "w4"),
                    tool_calls(),
                ]),
                _ => answer("done"),
            },
            Options {
                tools: vec![
                    slow("search_web", "three reverb chains", false),
                    slow("read_web", "", true),
                    slow("find_sounds", "a warm pad", false),
                    slow("make_changes", "Added Reverb", false),
                ],
                ..Options::default()
            },
        );
        let began = Instant::now();
        h.kernel.run("go", signal(), ignore()).await.unwrap();
        // Three reads together, then the change, then the last read: three waits of 300 ms, not five.
        let took = began.elapsed();
        assert!(took < Duration::from_millis(1400), "{took:?}");
        let messages = h.messages();
        let results: Vec<_> = tool_message(&messages[2])
            .iter()
            .map(|part| match part {
                ToolPart::ToolResult(result) => (result.tool_call_id.clone(), output_type(part), js(&result.output)),
                other => panic!("not a result: {other:?}"),
            })
            .collect();
        assert_eq!(
            results.iter().map(|(id, kind, _)| (id.as_str(), *kind)).collect::<Vec<_>>(),
            [("w1", "text"), ("w2", "error-text"), ("w3", "text"), ("c1", "text"), ("w4", "text")],
            "the reply's order, the failed read's error beside the others' results"
        );
        assert!(
            results[0].2.contains("three reverb chains") && results[1].2.contains("didn't answer") && results[2].2.contains("a warm pad")
        );
        h.kernel.close().await;
    })
    .await
}

#[tokio::test]
async fn huge_results_of_reads_run_together_are_each_cut_to_their_opening() {
    local(async {
        let huge = "x".repeat(1024 * 1024);
        let h = harness(
            |_, n| match n {
                1 => Scripted::Parts(vec![call_id("search_web", "{}", "w1"), call_id("read_web", "{}", "w2"), tool_calls()]),
                _ => answer("done"),
            },
            Options { tools: vec![saying("search_web", &huge), saying("read_web", &huge)], ..Options::default() },
        );
        h.kernel.run(&observed("read both"), signal(), ignore()).await.unwrap();
        let sent = js(&h.request(1).prompt);
        assert!(sent.len() < 140 * 1024, "{} bytes went to the model", sent.len());
        assert_eq!(sent.matches("Kumi cut the rest of this result: it was 1024 KB").count(), 2, "both results are cut");
        h.kernel.close().await;
    })
    .await
}

#[tokio::test]
async fn reads_stopped_together_keep_what_finished_and_mark_what_was_running() {
    local(async {
        let h = Rc::new(harness(
            |_, n| match n {
                1 => Scripted::Parts(vec![
                    call_id("search_web", "{}", "w1"),
                    call_id("read_web", "{}", "w2"),
                    call_id("make_changes", "{}", "c1"),
                    tool_calls(),
                ]),
                _ => answer("done"),
            },
            Options {
                tools: vec![
                    saying("search_web", "three reverb chains"),
                    tool("read_web", |_| std::future::pending()),
                    saying("make_changes", "Added Reverb"),
                ],
                ..Options::default()
            },
        ));
        let controller = Controller::new();
        let held = h.clone();
        let stop_signal = controller.signal.clone();
        let stopped = spawn_local(async move { held.kernel.run("go", stop_signal, ignore()).await });
        sleep(Duration::from_millis(20)).await;
        controller.abort();
        assert_eq!(stopped.await.unwrap().unwrap().stop_reason, StopReason::Cancelled);
        let messages = h.messages();
        let results: Vec<_> = tool_message(&messages[2]).iter().map(|part| (output_type(part), js(part))).collect();
        assert_eq!(results.iter().map(|(kind, _)| *kind).collect::<Vec<_>>(), ["text", "error-text", "error-text"]);
        assert!(results[0].1.contains("three reverb chains"));
        assert!(results[1].1.contains(STOPPED_READING) && results[2].1.contains(STOPPED_BEFORE_RUNNING));
        h.kernel.close().await;
    })
    .await
}

#[tokio::test]
async fn when_a_stopped_turns_steps_are_kept_so_is_the_budgets_trimming_for_them_with_its_note() {
    local(async {
        let mode = Rc::new(Cell::new(Mode::Answer));
        let step = Rc::new(Cell::new(0));
        let (scripted_mode, scripted_step) = (mode.clone(), step.clone());
        let big = "x".repeat(9_000);
        let h = harness(
            move |_, _| {
                if scripted_mode.get() == Mode::Answer {
                    return answer("fine");
                }
                scripted_step.set(scripted_step.get() + 1);
                if scripted_step.get() == 1 {
                    Scripted::Parts(vec![call_id("read", "{}", "big"), tool_calls()])
                } else {
                    Scripted::Reject(LanguageModelError::other("provider down"))
                }
            },
            Options {
                tools: vec![saying("read", &big)],
                budget: Some(ContextBudget { clear_at: 1024.0, limit: 8192.0 }),
                ..Options::default()
            },
        );
        for words in ["one", "two", "three"] {
            h.kernel.run(&format!("{words} {}", "w".repeat(600)), signal(), ignore()).await.unwrap();
        }
        mode.set(Mode::Fail);
        assert!(h.kernel.run("four", signal(), ignore()).await.is_err());
        assert!(js(&h.messages()[0]).contains("Kumi removed the earlier part"), "the model is told the start is gone");
        assert_eq!(h.transcript_texts(), ["four", STOPPED_NOTE]);
        h.kernel.close().await;
    })
    .await
}

#[tokio::test]
async fn when_the_earliest_exchanges_go_the_model_is_told_and_the_transcript_isnt() {
    local(async {
        let h = harness(
            |_, _| answer(&"w".repeat(1500)),
            Options { budget: Some(ContextBudget { clear_at: 1024.0, limit: 4096.0 }), ..Options::default() },
        );
        for words in ["one", "two", "three", "four"] {
            h.kernel.run(&format!("{words} {}", "w".repeat(1500)), signal(), ignore()).await.unwrap();
        }
        assert!(js(&h.request(h.count() - 1).prompt[0]).contains("Kumi removed the earlier part of this conversation"));
        let lines = h.kernel.transcript();
        assert!(lines.len() < 8);
        assert!(lines.iter().all(|line| !line.text.contains("Kumi removed")));
        assert!(["two ", "three ", "four "].iter().any(|start| lines[0].text.starts_with(start)), "{}", lines[0].text);
        h.kernel.close().await;
    })
    .await
}

#[test]
fn rejects_empty_instructions_invalid_or_duplicate_tool_names_and_a_budget_that_cant_hold_anything() {
    let noop = |name: &str| saying(name, "");
    let error = |options: Options| try_harness(|_, _| Scripted::Parts(Vec::new()), options).err().expect("an error").to_string();
    assert!(error(Options { instructions: Some(" ".into()), ..Options::default() }).contains("instructions"));
    for tools in [vec![noop("bad name")], vec![noop("dup"), noop("dup")], vec![noop(&"x".repeat(65))]] {
        assert!(error(Options { tools, ..Options::default() }).contains("Tool names"));
    }
    for budget in [
        ContextBudget { clear_at: 100.0, limit: 4096.0 },
        ContextBudget { clear_at: 4096.0, limit: 2048.0 },
        ContextBudget { clear_at: f64::NAN, limit: 4096.0 },
    ] {
        assert!(error(Options { budget: Some(budget), ..Options::default() }).contains("context budget"));
    }
}

/// What a streaming tool's call saw: its pushes, the input it finished with, and whether it was abandoned.
#[derive(Default)]
struct CallRecord {
    pushes: RefCell<Vec<String>>,
    finished: RefCell<Option<Option<JsonObject>>>,
    abandoned: Cell<bool>,
}

struct RecordedCall {
    record: Rc<CallRecord>,
    started: Cell<bool>,
    on_start: Rc<dyn Fn()>,
}

#[async_trait(?Send)]
impl StreamingCall for RecordedCall {
    fn push(&self, delta: &str) {
        self.record.pushes.borrow_mut().push(delta.to_string());
        if !self.started.get() && self.record.pushes.borrow().concat().contains('}') {
            self.started.set(true);
            (self.on_start)();
        }
    }
    async fn finish(&self, input: Option<JsonObject>) -> Result<ToolResult, RuntimeError> {
        *self.record.finished.borrow_mut() = Some(input);
        Ok(ToolResult { text: "planned".into(), reply: Some("Done: Tempo 120 → 126 BPM.".into()), ..ToolResult::default() })
    }
    async fn abandon(&self) {
        self.record.abandoned.set(true);
    }
    fn started(&self) -> bool {
        self.started.get()
    }
}

/// A tool whose calls can start while they're written: work begins once a whole step ("}") has arrived.
fn streaming_tool() -> (Rc<dyn KernelTool>, Rc<RefCell<Vec<Rc<CallRecord>>>>, Rc<Cell<usize>>) {
    let calls: Rc<RefCell<Vec<Rc<CallRecord>>>> = Rc::new(RefCell::new(Vec::new()));
    let executed = Rc::new(Cell::new(0));
    let (recording, counting) = (calls.clone(), executed.clone());
    let plan = Rc::new(FnTool {
        name: "plan".into(),
        description: "plan fixture".into(),
        input_schema: object(json!({"type": "object", "properties": {}})),
        execute: Rc::new(move |_, _| {
            counting.set(counting.get() + 1);
            Box::pin(async { Ok(ToolResult::text("never")) })
        }),
        stream: Some(Rc::new(move |_signal, on_start| {
            let record = Rc::new(CallRecord::default());
            recording.borrow_mut().push(record.clone());
            Box::new(RecordedCall { record, started: Cell::new(false), on_start })
        })),
    });
    (plan, calls, executed)
}
fn plan_parts(whole: bool) -> Vec<StreamPart> {
    vec![
        StreamPart::ToolInputStart {
            id: "c1".into(),
            tool_name: "plan".into(),
            provider_metadata: None,
            provider_executed: None,
            dynamic: None,
            title: None,
        },
        StreamPart::ToolInputDelta {
            id: "c1".into(),
            delta: (if whole { "{\"steps\":[{\"tool\":\"set_tempo\"}" } else { "{\"steps\":[{\"tool\":" }).into(),
            provider_metadata: None,
        },
    ]
}
fn overloaded() -> LanguageModelError {
    unavailable()
}
const PLAN_INPUT: &str = "{\"steps\":[{\"tool\":\"set_tempo\"}]}";

/// A streaming call that does nothing until it's finished; finishing says it started.
struct LateStart {
    on_start: Rc<dyn Fn()>,
}

#[async_trait(?Send)]
impl StreamingCall for LateStart {
    fn push(&self, _delta: &str) {}
    async fn finish(&self, _input: Option<JsonObject>) -> Result<ToolResult, RuntimeError> {
        (self.on_start)();
        Ok(ToolResult { text: "planned".into(), reply: Some("Done.".into()), ..ToolResult::default() })
    }
    async fn abandon(&self) {}
    fn started(&self) -> bool {
        false
    }
}

#[tokio::test]
async fn a_plan_whose_first_step_waits_for_the_next_begins_when_its_finished_and_says_it_started_once() {
    local(async {
        // Like parameter steps waiting to batch: nothing starts while it's written; finishing starts it.
        let begin: Rc<RefCell<Option<Rc<dyn Fn()>>>> = Rc::new(RefCell::new(None));
        let beginning = begin.clone();
        let plan = Rc::new(FnTool {
            name: "plan".into(),
            description: "plan".into(),
            input_schema: object(json!({"type": "object"})),
            execute: Rc::new(|_, _| Box::pin(async { Ok(ToolResult::text("whole")) })),
            stream: Some(Rc::new(move |_signal, on_start| {
                *beginning.borrow_mut() = Some(on_start.clone());
                Box::new(LateStart { on_start })
            })),
        });
        let h = harness(
            |_, _| {
                Scripted::Parts(
                    [
                        plan_parts(true),
                        vec![StreamPart::ToolInputEnd { id: "c1".into(), provider_metadata: None }, call("plan", PLAN_INPUT), tool_calls()],
                    ]
                    .concat(),
                )
            },
            Options { tools: vec![plan], ..Options::default() },
        );
        let (events, emit) = collect();
        h.kernel.run("go", signal(), emit).await.unwrap();
        assert!(begin.borrow().is_some());
        let starts = events.borrow().iter().filter(|event| matches!(event, KernelEvent::ToolStart { .. })).count();
        assert_eq!(starts, 1, "one start, so its end closes it: {:?}", events.borrow());
        h.kernel.close().await;
    })
    .await
}

#[tokio::test]
async fn a_plan_starts_while_the_model_is_still_writing_it_and_finishes_with_the_whole_of_it() {
    local(async {
        let (plan, calls, executed) = streaming_tool();
        let (release, held) = oneshot::channel::<()>();
        let held = Rc::new(RefCell::new(Some(held)));
        let h = Rc::new(harness(
            move |_, _| {
                let held = held.borrow_mut().take().expect("one call");
                let rest = vec![
                    StreamPart::ToolInputDelta { id: "c1".into(), delta: "]}".into(), provider_metadata: None },
                    StreamPart::ToolInputEnd { id: "c1".into(), provider_metadata: None },
                    call("plan", PLAN_INPUT),
                    tool_calls(),
                ];
                Scripted::Stream(
                    stream::iter(plan_parts(true))
                        .chain(
                            stream::once(async move {
                                let _ = held.await;
                                stream::iter(rest)
                            })
                            .flatten(),
                        )
                        .boxed_local(),
                )
            },
            Options { tools: vec![plan], ..Options::default() },
        ));
        let (events, emit) = collect();
        let held_kernel = h.clone();
        let running = spawn_local(async move { held_kernel.kernel.run("go", signal(), emit).await });
        sleep(Duration::from_millis(10)).await;
        assert_eq!(types(&events.borrow()), ["tool-input", "tool-start"], "running before the model finished writing it");
        release.send(()).unwrap();
        assert_eq!(running.await.unwrap().unwrap().stop_reason, StopReason::Completed);
        let record = calls.borrow()[0].clone();
        assert_eq!(*record.finished.borrow(), Some(Some(object(json!({ "steps": [{ "tool": "set_tempo" }] })))));
        assert_eq!(record.pushes.borrow().concat(), PLAN_INPUT);
        assert_eq!(executed.get(), 0, "not run a second time");
        assert_eq!(types(&events.borrow()), ["tool-input", "tool-start", "tool-end", "text"], "its reply ends the turn");
        assert_eq!(h.count(), 1);
        h.kernel.close().await;
    })
    .await
}

#[tokio::test]
async fn a_reply_that_breaks_off_after_its_plan_began_isnt_asked_for_again_one_that_broke_off_before_is_afresh() {
    local(async {
        let (plan, begun, _) = streaming_tool();
        let attempts = Rc::new(Cell::new(0));
        let counter = attempts.clone();
        let failing = harness(
            move |_, _| {
                counter.set(counter.get() + 1);
                Scripted::Parts([plan_parts(true), vec![StreamPart::Error { error: overloaded() }]].concat())
            },
            Options { tools: vec![plan], ..Options::default() },
        );
        assert!(failing.kernel.run("go", signal(), ignore()).await.unwrap_err().to_string().contains("overloaded"));
        assert_eq!(attempts.get(), 1, "its first step ran, so the reply isn't asked for again");
        assert!(begun.borrow()[0].abandoned.get(), "and nothing more of the plan starts");
        failing.kernel.close().await;

        let (plan, early, _) = streaming_tool();
        let retried = harness(
            |_, n| {
                if n == 1 {
                    Scripted::Parts([plan_parts(false), vec![StreamPart::Error { error: overloaded() }]].concat())
                } else {
                    answer("ok")
                }
            },
            Options { tools: vec![plan], ..Options::default() },
        );
        assert_eq!(retried.kernel.run("go", signal(), ignore()).await.unwrap().stop_reason, StopReason::Completed);
        assert_eq!(retried.count(), 2, "nothing had begun, so the reply was asked for again");
        assert!(early.borrow()[0].abandoned.get());
        retried.kernel.close().await;
    })
    .await
}

/// An answer's stream that broke off after the response began (it keeps its 200).
fn broke_off() -> LanguageModelError {
    let mut error = ApiCallError::new("the stream ended early", "u", Some(json!({})), Some(200));
    error.is_retryable = true;
    LanguageModelError::ApiCall(error)
}

#[tokio::test]
async fn an_answer_that_breaks_off_after_its_words_began_carries_on_once_from_what_was_shown() {
    local(async {
        let h = harness(
            |_, n| match n {
                1 => Scripted::Parts([text("The Reese needs"), vec![StreamPart::Error { error: broke_off() }]].concat()),
                _ => answer(" a darker filter."),
            },
            Options::default(),
        );
        let (events, emit) = collect();
        assert_eq!(h.kernel.run("why is my bass harsh?", signal(), emit).await.unwrap().stop_reason, StopReason::Completed);
        assert_eq!(h.count(), 2, "one more request carries on");
        assert_eq!(texts(&events.borrow()).concat(), "The Reese needs a darker filter.");
        assert!(
            events.borrow().iter().any(|e| matches!(e, KernelEvent::Retry { reason, wait_ms: 0 } if reason.contains("broke off"))),
            "the status line says so"
        );
        let carried = js(&h.request(1).prompt);
        assert!(carried.contains("The Reese needs") && carried.contains("connection dropped partway"), "{carried}");
        assert_eq!(h.transcript_texts(), ["why is my bass harsh?", "The Reese needs a darker filter."], "the note isn't the producer's");
        let kept_from_the_note = [
            json!({"role": "user", "content": format!("{SHORTENED}{CARRY_ON_NOTE}")}),
            json!({"role": "assistant", "content": " a darker filter."}),
        ];
        assert_eq!(transcript_of(&kept_from_the_note).iter().map(|line| line.text.as_str()).collect::<Vec<_>>(), ["a darker filter."]);
        h.kernel.close().await;

        // Once a turn: breaking off again ends it, as before.
        let twice = harness(
            |_, _| Scripted::Parts([text("The Reese"), vec![StreamPart::Error { error: broke_off() }]].concat()),
            Options::default(),
        );
        assert!(twice.kernel.run("why is my bass harsh?", signal(), ignore()).await.is_err());
        assert_eq!(twice.count(), 2);
        twice.kernel.close().await;
    })
    .await
}

#[tokio::test]
async fn a_quiet_call_a_note_kept_beside_the_models_answer_ends_the_turn_before_the_answer_or_beside_a_read_the_model_carries_on() {
    local(async {
        let note = || replying("remember", "{\"kept\":\"s1\"}", "");
        let read = || saying("read", "{\"tempo\":120}");
        let answered = harness(
            |_, _| Scripted::Parts([text("Got it: the Reese carries the low end."), vec![call("remember", "{\"note\":\"The Reese is the main bass\",\"about\":\"set\"}"), tool_calls()]].concat()),
            Options { tools: vec![note(), read()], ..Options::default() },
        );
        let (events, emit) = collect();
        assert_eq!(answered.kernel.run("the reese is my main bass", signal(), emit).await.unwrap().stop_reason, StopReason::Completed);
        assert_eq!(answered.count(), 1, "no model reply after keeping a note");
        assert_eq!(texts(&events.borrow()).concat(), "Got it: the Reese carries the low end.", "nothing added to the answer");
        assert!(matches!(answered.messages().last(), Some(Message::Tool { .. })), "the call and its result stay in the conversation");
        answered.kernel.close().await;

        let reading = harness(
            |_, n| if n == 1 { Scripted::Parts(vec![call_id("read", "{}", "a"), call_id("remember", "{\"note\":\"x\",\"about\":\"set\"}", "b"), tool_calls()]) } else { answer("It's 120.") },
            Options { tools: vec![note(), read()], ..Options::default() },
        );
        reading.kernel.run("what's the tempo? and remember I like it", signal(), ignore()).await.unwrap();
        assert_eq!(reading.count(), 2, "the read's result goes back to the model");
        reading.kernel.close().await;

        // A note kept before the answer is written: the model still answers, after it.
        let first = harness(
            |_, n| if n == 1 { Scripted::Parts(vec![call("remember", "{\"note\":\"Prefers short reverbs\",\"about\":\"producer\"}"), tool_calls()]) } else { answer("Short, dark reverbs: try Hybrid Reverb.") },
            Options { tools: vec![note()], ..Options::default() },
        );
        let (answers, emit) = collect();
        assert_eq!(first.kernel.run("I like short reverbs. Which reverb suits that?", signal(), emit).await.unwrap().stop_reason, StopReason::Completed);
        assert_eq!(first.count(), 2, "a note kept first doesn't cut the answer off");
        assert!(texts(&answers.borrow()).concat().contains("Hybrid Reverb"));
        let empty_answer = first.messages().iter().any(|message| matches!(message, Message::Assistant { content, .. } if content.iter().any(|part| matches!(part, AssistantPart::Text(text) if text.text.is_empty()))));
        assert!(!empty_answer, "no empty answer is kept");
        first.kernel.close().await;
    })
    .await
}

#[tokio::test]
async fn words_beside_a_reaction_alone_get_another_model_call() {
    use kumi_runtime::core::{
        store_client::StoreClient,
        taste_log::{TasteLog, Whereabouts},
    };
    local(async {
        // The producer's reaction noted beside progress words: the change comes in the model's next call.
        let folder = tempfile::tempdir().unwrap();
        let store = StoreClient::new(kumi_store::Store::open(folder.path().join("kumi.db")).unwrap());
        let taste = TasteLog::new(store, Rc::new(Whereabouts::default), Rc::new(|_| None));
        taste.turn_started("too bright, make it darker", true);
        let h = harness(
            |_, n| {
                if n == 1 {
                    Scripted::Parts(
                        [text("Making it darker now."), vec![call("reaction", "{\"quote\":\"too bright\",\"lean\":\"less\"}"), tool_calls()]].concat(),
                    )
                } else {
                    answer("Darker: the filter is down 2 kHz.")
                }
            },
            Options { tools: vec![taste.tool()], ..Options::default() },
        );
        assert_eq!(h.kernel.run("too bright, make it darker", signal(), ignore()).await.unwrap().stop_reason, StopReason::Completed);
        assert_eq!(h.count(), 2, "the reaction doesn't end the turn");
        h.kernel.close().await;
    })
    .await
}

#[tokio::test]
async fn a_conversation_carried_into_other_instructions_other_notes_say_continues_without_its_reasoning() {
    local(async {
        let first = harness(
            |_, _| {
                Scripted::Parts(
                    [thinking("r1", meta(json!({"anthropic": {"signature": "sig-9"}})), "hmm"), text("ok"), vec![stop()]].concat(),
                )
            },
            Options::default(),
        );
        first.kernel.run("one", signal(), ignore()).await.unwrap();
        let checkpoint: KernelCheckpoint =
            serde_json::from_str(&serde_json::to_string(&first.kernel.checkpoint().unwrap()).unwrap()).unwrap();
        let same = harness(|_, _| answer("ok"), Options { checkpoint: Some(checkpoint.clone()), ..Options::default() });
        same.kernel.run("two", signal(), ignore()).await.unwrap();
        assert!(js(&same.request(0).prompt).contains("sig-9"));
        let other = harness(
            |_, _| answer("ok"),
            Options {
                checkpoint: Some(checkpoint),
                instructions: Some(
                    "fixture instructions\n\n<remembered_notes_untrusted>\n- [s1] new note\n</remembered_notes_untrusted>".into(),
                ),
                ..Options::default()
            },
        );
        other.kernel.run("two", signal(), ignore()).await.unwrap();
        assert!(!js(&other.request(0).prompt).contains("sig-9"));
        first.kernel.close().await;
        same.kernel.close().await;
        other.kernel.close().await;
    })
    .await
}

fn shown(request: &CallOptions) -> Option<&Message> {
    request.prompt.iter().find(|message| matches!(message, Message::Tool { .. }))
}

fn first_output(message: &Message) -> &ToolResultOutput {
    match &tool_message(message)[0] {
        ToolPart::ToolResult(result) => &result.output,
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
async fn a_tools_images_reach_the_model_for_the_rest_of_the_turn_when_it_ends_theyre_put_away_with_the_reasoning_written_after_them() {
    local(async {
        let thought = |id: &str| thinking(id, meta(json!({"openai": {"itemId": format!("rs_{id}"), "reasoningEncryptedContent": "enc"}})), "thinking");
        let h = harness(
            move |_, n| match n {
                1 => Scripted::Parts([thought("a"), vec![call("watch", "{}"), tool_calls()]].concat()),
                2 => Scripted::Parts([thought("b"), vec![call_id("read", "{}", "c2"), tool_calls()]].concat()),
                3 => Scripted::Parts([thought("c"), text("It loads Operator."), vec![stop()]].concat()),
                _ => answer("again"),
            },
            Options {
                tools: vec![
                    tool("watch", |_| async {
                        Ok(ToolResult {
                            text: "Video: a tutorial".into(),
                            images: vec![
                                ToolImage { data: vec![0xff, 0xd8, 0xff, 0xe0, 1, 2], media_type: "image/jpeg".into(), caption: Some("Frame at 0:05".into()) },
                                ToolImage { data: vec![1], media_type: "text/plain".into(), caption: None },
                                ToolImage { data: vec![0x89, 0x50], media_type: "image/png".into(), caption: None },
                            ],
                            ..ToolResult::default()
                        })
                    }),
                    saying("read", "tempo 120"),
                ],
                ..Options::default()
            },
        );
        h.kernel.run("watch this", signal(), ignore()).await.unwrap();
        // During the turn: words, each image after its caption (what models can't read is left out), as plain bytes.
        let request = h.request(1);
        let output = first_output(shown(&request).unwrap());
        let ToolResultOutput::Content { value, .. } = output else { panic!("{output:?}") };
        let kinds: Vec<String> = value
            .iter()
            .map(|part| match part {
                ToolResultContentItem::Text { text, .. } => text.clone(),
                ToolResultContentItem::File { media_type, .. } => media_type.clone(),
                ToolResultContentItem::Custom { .. } => "custom".into(),
            })
            .collect();
        assert_eq!(kinds, ["Video: a tutorial", "Frame at 0:05", "image/jpeg", "image/png"]);
        assert!(matches!(&value[2], ToolResultContentItem::File { data: FileData::Data { data: DataContent::Bytes(_) }, .. }));
        let later = h.request(2);
        assert!(matches!(first_output(shown(&later).unwrap()), ToolResultOutput::Content { .. }));
        // Settled: the words stay with a line saying images were there; reasoning before them stays, after them goes.
        let settled = h.messages();
        assert_eq!(
            js(&settled[2]),
            js(&json!({ "role": "tool", "content": [{ "type": "tool-result", "toolCallId": "c1", "toolName": "watch",
                "output": { "type": "text", "value": "Video: a tutorial\nFrame at 0:05\n[2 images were shown here; they're no longer attached (the tool shows them again when asked).]" } }] }))
        );
        assert!(matches!(&settled[1], Message::Assistant { content, .. } if matches!(content[0], AssistantPart::Reasoning(_))));
        assert!(settled[3..].iter().all(|message| !matches!(message, Message::Assistant { content, .. } if content.iter().any(|part| matches!(part, AssistantPart::Reasoning(_))))));
        assert!(!js(&settled).contains("\"0\":255"));
        h.kernel.run("and now?", signal(), ignore()).await.unwrap();
        assert!(!js(&h.request(3).prompt).contains("image/jpeg"));
        h.kernel.close().await;
    })
    .await
}

#[tokio::test]
async fn a_stopped_turn_keeps_a_tools_words_not_its_images() {
    local(async {
        let h = Rc::new(harness(
            |_, n| if n == 1 { Scripted::Parts(vec![call("watch", "{}"), tool_calls()]) } else { hanging(Vec::new()) },
            Options {
                tools: vec![tool("watch", |_| async {
                    Ok(ToolResult {
                        text: "Video".into(),
                        images: vec![ToolImage { data: vec![0xff, 0xd8], media_type: "image/jpeg".into(), caption: None }],
                        ..ToolResult::default()
                    })
                })],
                ..Options::default()
            },
        ));
        let controller = Controller::new();
        let held = h.clone();
        let stop_signal = controller.signal.clone();
        let running = spawn_local(async move { held.kernel.run("watch", stop_signal, ignore()).await });
        while h.count() < 2 {
            sleep(Duration::from_millis(1)).await;
        }
        controller.abort();
        assert_eq!(running.await.unwrap().unwrap().stop_reason, StopReason::Cancelled);
        let settled = js(&h.messages());
        assert!(settled.contains("An image was shown here"));
        assert!(!settled.contains("image/jpeg"));
        h.kernel.close().await;
    })
    .await
}

/// Hold the reply after a real public tool has sent its apply to Live.
struct HeldTempoApply {
    dispatched: tokio::sync::Notify,
    acknowledgement: RefCell<Option<oneshot::Receiver<()>>>,
    calls: RefCell<Vec<String>>,
}
#[async_trait(?Send)]
impl McpEndpoint for HeldTempoApply {
    fn pid(&self) -> Option<u32> {
        None
    }
    fn server_info(&self) -> Option<Implementation> {
        Some(serde_json::from_value(json!({"name":"fixture","version":"1.0.73"})).unwrap())
    }
    async fn list(&self, _: Option<&str>, _: Signal) -> Result<ListToolsResult, RuntimeError> {
        let tools = ["live_status", "live_tempo_preview", "live_tempo_apply", "live_undo"]
            .map(|name| json!({"name":name,"inputSchema":{"type":"object"}}));
        Ok(serde_json::from_value(json!({"tools":tools})).unwrap())
    }
    async fn call(&self, name: &str, args: JsonObject, signal: Signal) -> Result<CallToolResult, RuntimeError> {
        assert!(!signal.is_cancelled(), "the dispatched mutation has its own settlement signal");
        self.calls.borrow_mut().push(name.into());
        let result = match name {
            "live_status" => json!({"connected":true,"adapter":"remote-script","epoch":7}),
            "live_tempo_preview" => {
                assert_eq!(args["tempo"], 126);
                json!({"epoch":7,"transactionId":"tempo_held","confirmation":"apply","priorTempo":120,"proposedTempo":126})
            }
            "live_tempo_apply" => {
                assert_eq!(args["transactionId"], "tempo_held");
                let reply = self.acknowledgement.borrow_mut().take().expect("one apply");
                self.dispatched.notify_one();
                reply.await.expect("release the held Live acknowledgement");
                assert!(!signal.is_cancelled(), "turn cancellation must not cancel a sent mutation");
                json!({"state":"applied"})
            }
            "live_undo" => {
                assert_eq!(args["transactionId"], "tempo_held");
                json!({"state":"undone"})
            }
            _ => panic!("unexpected bridge call {name}"),
        };
        Ok(serde_json::from_value(json!({"content":[{"type":"text","text":stringify(&result)}]})).unwrap())
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

#[tokio::test]
async fn cancelled_dispatched_tempo_settles_into_history_and_remains_undoable_after_kernel_close() {
    use futures::FutureExt;
    local(async {
        for close_turn in [false, true] {
            let (acknowledge, reply) = oneshot::channel();
            let endpoint = Rc::new(HeldTempoApply {
                dispatched: tokio::sync::Notify::new(),
                acknowledgement: RefCell::new(Some(reply)),
                calls: RefCell::new(Vec::new()),
            });
            let connect = endpoint.clone();
            let remembered = Rc::new(tokio::sync::Notify::new());
            let changed = remembered.clone();
            let mut options = AbletonOptions::new(Rc::new(|_, _| {}));
            options.connect = Some(Rc::new(move |_| {
                let endpoint = connect.clone();
                async move { Ok(endpoint as Rc<dyn McpEndpoint>) }.boxed_local()
            }));
            options.on_change = Some(Rc::new(move |_| changed.notify_one()));
            let integration = Ableton::new(options);
            integration.start(signal()).await.unwrap();
            integration.connection.tools().unwrap().refresh(signal()).await.unwrap();
            integration.connection.epoch.set(Some(7.0));
            let tempo = integration.definitions().into_iter().find(|tool| tool.name() == "set_tempo").unwrap();
            let h = Rc::new(harness(
                |_, _| Scripted::Parts(vec![call("set_tempo", "{\"tempo\":126}"), tool_calls()]),
                Options { tools: vec![tempo], ..Options::default() },
            ));
            let (events, emit) = collect();
            let controller = Controller::new();
            let run_signal = controller.signal.clone();
            let running = h.clone();
            let run = spawn_local(async move { running.kernel.run("set the tempo", run_signal, emit).await });
            tokio::time::timeout(Duration::from_secs(1), endpoint.dispatched.notified()).await.unwrap();
            if close_turn {
                tokio::time::timeout(Duration::from_secs(1), h.kernel.close()).await.unwrap();
            } else {
                controller.abort();
            }
            let result = tokio::time::timeout(Duration::from_secs(1), run).await.unwrap().unwrap().unwrap();
            assert_eq!(result.stop_reason, StopReason::Cancelled);
            assert!(integration.history.entries.borrow().is_empty(), "Live has not acknowledged the sent mutation yet");
            assert!(h.messages().is_empty(), "the unfinished tool round is not replayed to the model");
            assert_eq!(types(&events.borrow()), ["tool-start"]);
            tokio::time::timeout(Duration::from_secs(1), h.kernel.close()).await.unwrap();
            drop(h);
            assert!(acknowledge.send(()).is_ok(), "cancelling the wait dropped the already-dispatched mutation");
            tokio::time::timeout(Duration::from_secs(1), remembered.notified()).await.unwrap();
            let entry = integration.history.entries.borrow().values().next().unwrap().borrow().clone();
            assert_eq!(entry.record.state, ChangeState::Applied);
            assert_eq!(entry.record.title, "Tempo 120 → 126 BPM");
            assert_eq!(entry.transaction_id, "tempo_held");
            assert_eq!(types(&events.borrow()), ["tool-start"], "late settlement emits no kernel tool result");
            let undone = integration.history.undo("last", signal(), false).await.unwrap();
            assert!(!undone.is_error, "{}", undone.text);
            assert_eq!(undone.record.unwrap().state, ChangeState::Undone);
            assert_eq!(*endpoint.calls.borrow(), ["live_status", "live_tempo_preview", "live_tempo_apply", "live_undo"]);
            integration.close().await.unwrap();
        }
    })
    .await;
}

struct HeldStreamingCall {
    abandoning: bool,
    entered: Rc<tokio::sync::Notify>,
    reply: Rc<RefCell<Option<oneshot::Receiver<()>>>>,
    cleaned: Rc<tokio::sync::Notify>,
    on_start: Rc<dyn Fn()>,
}
impl HeldStreamingCall {
    async fn settle(&self) {
        let reply = self.reply.borrow_mut().take().expect("one settlement");
        self.entered.notify_one();
        reply.await.expect("release streaming cleanup");
        self.cleaned.notify_one();
    }
}
#[async_trait(?Send)]
impl StreamingCall for HeldStreamingCall {
    fn push(&self, _: &str) {
        (self.on_start)();
    }
    fn started(&self) -> bool {
        true
    }
    async fn finish(&self, _: Option<JsonObject>) -> Result<ToolResult, RuntimeError> {
        assert!(!self.abandoning);
        self.settle().await;
        Err(RuntimeError::plain("late streaming failure after cleanup"))
    }
    async fn abandon(&self) {
        assert!(self.abandoning);
        self.settle().await;
    }
}
async fn cancelled_stream_cleanup(abandoning: bool) {
    let (release, reply) = oneshot::channel();
    let reply = Rc::new(RefCell::new(Some(reply)));
    let entered = Rc::new(tokio::sync::Notify::new());
    let cleaned = Rc::new(tokio::sync::Notify::new());
    let started = entered.clone();
    let settled = cleaned.clone();
    let plan = Rc::new(FnTool {
        name: "plan".into(),
        description: "streamed cleanup fixture".into(),
        input_schema: object(json!({"type":"object"})),
        execute: Rc::new(|_, _| Box::pin(async { panic!("a streaming call must not execute twice") })),
        stream: Some(Rc::new(move |_, on_start| {
            Box::new(HeldStreamingCall { abandoning, entered: started.clone(), reply: reply.clone(), cleaned: settled.clone(), on_start })
        })),
    });
    let h = Rc::new(harness(
        move |_, _| {
            let mut parts = plan_parts(true);
            parts.extend(if abandoning {
                vec![StreamPart::Error { error: overloaded() }]
            } else {
                vec![call("plan", PLAN_INPUT), tool_calls()]
            });
            Scripted::Parts(parts)
        },
        Options { tools: vec![plan], ..Default::default() },
    ));
    let (events, emit) = collect();
    let controller = Controller::new();
    let active = h.clone();
    let run_signal = controller.signal.clone();
    let running = spawn_local(async move { active.kernel.run("plan", run_signal, emit).await });
    tokio::time::timeout(Duration::from_secs(1), entered.notified()).await.unwrap();
    controller.abort();
    let expected_events = types(&events.borrow());
    if abandoning {
        assert!(!running.is_finished(), "source waits for abandon cleanup already in progress");
        release.send(()).expect("abandon retains its cleanup");
        let result = tokio::time::timeout(Duration::from_secs(1), running).await.unwrap().unwrap().unwrap();
        assert_eq!(result.stop_reason, StopReason::Cancelled);
        h.kernel.close().await;
    } else {
        let result = tokio::time::timeout(Duration::from_secs(1), running).await.unwrap().unwrap().unwrap();
        assert_eq!(result.stop_reason, StopReason::Cancelled);
        tokio::time::timeout(Duration::from_secs(1), h.kernel.close()).await.unwrap();
        assert!(h.messages().is_empty());
        drop(h);
        assert!(release.send(()).is_ok(), "cancelled streamed finish lost its cleanup and owner");
    }
    tokio::time::timeout(Duration::from_secs(1), cleaned.notified()).await.unwrap();
    assert_eq!(types(&events.borrow()), expected_events, "late streaming settlement must not reach the UI");
}
#[tokio::test]
async fn cancelled_streamed_finish_keeps_cleanup_alive_and_ignores_late_failure() {
    local(cancelled_stream_cleanup(false)).await;
}
#[tokio::test]
async fn cancellation_during_streamed_abandon_preserves_source_cleanup_wait() {
    local(cancelled_stream_cleanup(true)).await;
}
