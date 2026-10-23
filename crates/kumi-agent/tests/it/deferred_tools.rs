//! Deferred tools: kept out of every request, listed by find_tools, run by call_tool or by their own name.

use std::cell::RefCell;
use std::future::Future;
use std::rc::Rc;

use async_trait::async_trait;
use futures::stream::{self, StreamExt};
use kumi_common::abort::{Controller, Signal};
use kumi_runtime::ai::error::LanguageModelError;
use kumi_runtime::ai::types::{
    CallOptions, FinishReason, FinishReasonUnified, InputTokens, Message, OutputTokens, StreamPart, StreamParts, ToolCall, ToolPart,
    ToolResultOutput, Usage as ModelUsage,
};
use kumi_runtime::core::contracts::{JsonObject, KernelEmit, KernelEvent, KernelTool, ToolResult};
use kumi_runtime::core::errors::RuntimeError;
use kumi_runtime::kernel::agent::{create_agent_kernel, plain_words, AgentKernel, AgentKernelOptions, LanguageModel, ModelBinding};
use kumi_runtime::kernel::budget::transcript_of;
use serde_json::{json, Value};
use tokio::sync::Notify;
use tokio::task::LocalSet;

type Script = Rc<dyn Fn(&CallOptions, usize) -> Vec<StreamPart>>;

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
        Ok(stream::iter((self.script)(&options, n)).boxed_local())
    }
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
fn calls(list: &[(&str, &str, Value)]) -> Vec<StreamPart> {
    let mut parts: Vec<StreamPart> = list
        .iter()
        .map(|(id, name, input)| {
            StreamPart::ToolCall(ToolCall {
                tool_call_id: (*id).into(),
                tool_name: (*name).into(),
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

fn object(value: Value) -> JsonObject {
    value.as_object().unwrap().clone()
}

type Run = Rc<dyn Fn(JsonObject) -> futures::future::LocalBoxFuture<'static, Result<ToolResult, RuntimeError>>>;

struct Fixture {
    name: String,
    description: String,
    schema: JsonObject,
    deferred: bool,
    listed_by: Option<String>,
    run: Run,
}

#[async_trait(?Send)]
impl KernelTool for Fixture {
    fn name(&self) -> &str {
        &self.name
    }
    fn description(&self) -> &str {
        &self.description
    }
    fn input_schema(&self) -> JsonObject {
        self.schema.clone()
    }
    async fn execute(&self, input: JsonObject, _signal: Signal) -> Result<ToolResult, RuntimeError> {
        (self.run)(input).await
    }
    fn deferred(&self) -> bool {
        self.deferred
    }
    fn listed_by(&self) -> Option<&str> {
        self.listed_by.as_deref()
    }
}

/// A tool that records each input it ran with and answers with it.
fn fixture(name: &str, deferred: bool, ran: &Rc<RefCell<Vec<(String, Value)>>>) -> Rc<Fixture> {
    let (named, ran) = (name.to_string(), ran.clone());
    Rc::new(Fixture {
        name: name.into(),
        description: format!("Does {name} things to a track. More words about {name} that the list leaves out."),
        schema: object(json!({"type":"object","additionalProperties":false,"required":["track"],"properties":{
            "track":{"type":"string","minLength":1},"amount":{"type":"number","minimum":0,"maximum":1},"final":{"type":"boolean"}
        }})),
        deferred,
        listed_by: None,
        run: Rc::new(move |input: JsonObject| {
            let (named, ran) = (named.clone(), ran.clone());
            Box::pin(async move {
                ran.borrow_mut().push((named.clone(), Value::Object(input.clone())));
                Ok(ToolResult::text(format!("{named} ran on {}", input["track"])))
            })
        }),
    })
}

struct Harness {
    kernel: AgentKernel,
    requests: Rc<RefCell<Vec<CallOptions>>>,
}

fn harness(tools: Vec<Rc<dyn KernelTool>>, script: impl Fn(&CallOptions, usize) -> Vec<StreamPart> + 'static) -> Harness {
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
        conversation: Some("deferred".into()),
        binding,
        instructions: "fixture instructions".into(),
        tools,
        signal: Controller::new().signal,
        checkpoint: None,
        max_steps: None,
        budget: None,
    })
    .expect("a kernel");
    Harness { kernel, requests }
}

fn sent(request: &CallOptions) -> Vec<String> {
    request.tools.as_ref().map(|tools| tools.iter().map(|tool| tool.name.clone()).collect()).unwrap_or_default()
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

async fn local<F: Future>(future: F) -> F::Output {
    LocalSet::new().run_until(future).await
}

/// The text of the tool results in the conversation's last tool message.
fn results(kernel: &AgentKernel) -> Vec<(String, String, bool)> {
    let messages = kernel.checkpoint().unwrap().messages;
    let Some(Message::Tool { content, .. }) = messages.iter().rev().find(|message| matches!(message, Message::Tool { .. })) else {
        return vec![];
    };
    content
        .iter()
        .filter_map(|part| match part {
            ToolPart::ToolResult(result) => match &result.output {
                ToolResultOutput::Text { value, .. } => Some((result.tool_name.clone(), value.clone(), false)),
                ToolResultOutput::ErrorText { value, .. } => Some((result.tool_name.clone(), value.clone(), true)),
                _ => None,
            },
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn deferred_tools_stay_out_of_every_request_and_find_tools_lists_them() {
    local(async {
        let ran = Rc::new(RefCell::new(vec![]));
        let tools: Vec<Rc<dyn KernelTool>> =
            vec![fixture("tempo", false, &ran), fixture("judge", true, &ran), fixture("mute", false, &ran), fixture("arrange", true, &ran)];
        let h = harness(tools, |_, _| answer("ok"));
        h.kernel.run("hi", Controller::new().signal, Rc::new(|_| Ok(()))).await.unwrap();
        let request = &h.requests.borrow()[0];
        // The core tools in their order, then the two that reach the rest.
        assert_eq!(sent(request), ["tempo", "mute", "find_tools", "call_tool"]);
        let find = request.tools.as_ref().unwrap().iter().find(|tool| tool.name == "find_tools").unwrap();
        let description = find.description.clone().unwrap();
        // Each with its fields, so a plain call needs no lookup, and what it's for.
        assert!(description.contains("judge(track, amount?, final?): Does judge things to a track"), "{description}");
        assert!(description.contains("arrange(track, amount?, final?): Does arrange things to a track"), "{description}");
        assert!(!description.contains("the list leaves out"), "only each tool's first clause: {description}");
    })
    .await;
}

#[tokio::test]
async fn without_deferred_tools_the_request_is_as_before() {
    local(async {
        let ran = Rc::new(RefCell::new(vec![]));
        let h = harness(vec![fixture("tempo", false, &ran), fixture("mute", false, &ran)], |_, _| answer("ok"));
        h.kernel.run("hi", Controller::new().signal, Rc::new(|_| Ok(()))).await.unwrap();
        assert_eq!(sent(&h.requests.borrow()[0]), ["tempo", "mute"]);
    })
    .await;
}

#[tokio::test]
async fn find_tools_gives_whole_definitions_by_name_and_by_words() {
    local(async {
        let ran = Rc::new(RefCell::new(vec![]));
        let tools: Vec<Rc<dyn KernelTool>> =
            vec![fixture("tempo", false, &ran), fixture("judge", true, &ran), fixture("arrange", true, &ran)];
        let h = harness(tools, |_, n| match n {
            1 => calls(&[
                ("f1", "find_tools", json!({"names": ["judge", "nothing"]})),
                ("f2", "find_tools", json!({"words": "arrange things"})),
            ]),
            _ => answer("found"),
        });
        h.kernel.run("which tools?", Controller::new().signal, Rc::new(|_| Ok(()))).await.unwrap();
        let found = results(&h.kernel);
        let by_name: Value = serde_json::from_str(&found[0].1).unwrap();
        assert_eq!(by_name["tools"][0]["name"], "judge");
        assert!(by_name["tools"][0]["description"].as_str().unwrap().contains("the list leaves out"), "the whole description");
        assert_eq!(by_name["tools"][0]["input"]["required"], json!(["track"]));
        assert_eq!(by_name["unknown"], json!(["nothing"]));
        let by_words: Value = serde_json::from_str(&found[1].1).unwrap();
        assert_eq!(by_words["tools"][0]["name"], "arrange", "{by_words}");
        // Nothing found is said, not left empty.
        assert!(!found[0].2 && !found[1].2);
    })
    .await;
}

#[tokio::test]
async fn call_tool_runs_the_deferred_tool_and_every_event_names_it() {
    local(async {
        let ran = Rc::new(RefCell::new(vec![]));
        let tools: Vec<Rc<dyn KernelTool>> = vec![fixture("tempo", false, &ran), fixture("judge", true, &ran)];
        let h = harness(tools, |_, n| match n {
            1 => calls(&[("c1", "call_tool", json!({"name": "judge", "input": {"track": "bass", "amount": 0.5}}))]),
            _ => answer("judged"),
        });
        let (events, emit) = collect();
        h.kernel.run("judge the bass", Controller::new().signal, emit).await.unwrap();
        assert_eq!(*ran.borrow(), [("judge".to_string(), json!({"track": "bass", "amount": 0.5}))]);
        let named: Vec<String> = events
            .borrow()
            .iter()
            .filter_map(|event| match event {
                KernelEvent::ToolInput { name, .. } | KernelEvent::ToolStart { name, .. } | KernelEvent::ToolEnd { name, .. } => {
                    Some(name.clone())
                }
                _ => None,
            })
            .collect();
        assert_eq!(named, ["judge", "judge"], "no tool-input for call_tool, and start and end name the tool it ran");
        // The result answers the model's own call.
        assert_eq!(results(&h.kernel), [("call_tool".to_string(), "judge ran on \"bass\"".to_string(), false)]);
        // The transcript and a side question's words name the tool that ran.
        let messages: Vec<Value> = h.kernel.checkpoint().unwrap().messages.iter().map(|m| serde_json::to_value(m).unwrap()).collect();
        let transcript = transcript_of(&messages);
        assert!(transcript.iter().any(|line| line.tools.as_deref() == Some(&["judge".to_string()][..])), "{transcript:?}");
        let words = serde_json::to_string(&plain_words(&h.kernel.checkpoint().unwrap().messages)).unwrap();
        assert!(words.contains("[called judge {"), "{words}");
    })
    .await;
}

#[tokio::test]
async fn a_deferred_tool_called_by_its_own_name_runs_and_its_input_is_checked() {
    local(async {
        let ran = Rc::new(RefCell::new(vec![]));
        let tools: Vec<Rc<dyn KernelTool>> = vec![fixture("tempo", false, &ran), fixture("judge", true, &ran)];
        let h = harness(tools, |_, n| match n {
            1 => calls(&[
                ("c1", "judge", json!({"track": "keys"})),
                ("c2", "judge", json!({"track": "keys", "amount": 2})),
                ("c3", "call_tool", json!({"name": "judge", "input": {"amount": 0.2}})),
                ("c4", "call_tool", json!({"name": "nope", "input": {}})),
                ("c5", "call_tool", json!({"input": {}})),
            ]),
            _ => answer("done"),
        });
        h.kernel.run("go", Controller::new().signal, Rc::new(|_| Ok(()))).await.unwrap();
        assert_eq!(*ran.borrow(), [("judge".to_string(), json!({"track": "keys"}))], "only the input that fits runs");
        let found = results(&h.kernel);
        assert_eq!(found[0], ("judge".into(), "judge ran on \"keys\"".into(), false));
        assert_eq!(found[1].1, "judge: `amount` must be at most 1, not 2. find_tools gives its whole input.");
        assert_eq!(found[2].1, "judge: The input needs `track`. find_tools gives its whole input.");
        assert_eq!(found[3].1, "No tool is called nope; find_tools' description lists them.");
        assert_eq!(found[4].1, "call_tool needs `name`: a tool from find_tools' list.");
        assert!(found[1..].iter().all(|(_, _, error)| *error));
    })
    .await;
}

#[tokio::test]
async fn an_unknown_tool_points_at_find_tools() {
    local(async {
        let ran = Rc::new(RefCell::new(vec![]));
        let tools: Vec<Rc<dyn KernelTool>> = vec![fixture("tempo", false, &ran), fixture("judge", true, &ran)];
        let h = harness(tools, |_, n| match n {
            1 => calls(&[("c1", "freeze_track", json!({}))]),
            _ => answer("ok"),
        });
        h.kernel.run("freeze", Controller::new().signal, Rc::new(|_| Ok(()))).await.unwrap();
        assert_eq!(results(&h.kernel)[0].1, "Unknown tool \"freeze_track\"; use only the supplied tools (find_tools lists the others).");
    })
    .await;
}

#[tokio::test]
async fn a_tool_that_a_plan_lists_is_left_to_it_in_the_list() {
    local(async {
        let ran = Rc::new(RefCell::new(vec![]));
        let step =
            Rc::new(Fixture { listed_by: Some("make_changes".into()), ..Rc::try_unwrap(fixture("set_scale", true, &ran)).ok().unwrap() });
        let tools: Vec<Rc<dyn KernelTool>> = vec![fixture("make_changes", false, &ran), step, fixture("judge", true, &ran)];
        let h = harness(tools, |_, n| match n {
            1 => calls(&[("f1", "find_tools", json!({"names": ["set_scale"]}))]),
            _ => answer("ok"),
        });
        h.kernel.run("scale", Controller::new().signal, Rc::new(|_| Ok(()))).await.unwrap();
        let find = h.requests.borrow()[0].tools.as_ref().unwrap().iter().find(|tool| tool.name == "find_tools").cloned().unwrap();
        let description = find.description.unwrap();
        assert!(!description.contains("set_scale"), "{description}");
        assert!(description.ends_with("Besides these, every step make_changes lists is one too."), "{description}");
        let found: Value = serde_json::from_str(&results(&h.kernel)[0].1).unwrap();
        assert_eq!(found["tools"][0]["alsoAStepOf"], "make_changes");
    })
    .await;
}

#[tokio::test]
async fn deferred_reads_away_from_live_still_run_together() {
    local(async {
        let gate = Rc::new(Notify::new());
        let started = Rc::new(RefCell::new(Vec::<String>::new()));
        let make = |name: &'static str| -> Rc<dyn KernelTool> {
            let (gate, started) = (gate.clone(), started.clone());
            Rc::new(Fixture {
                name: name.into(),
                description: format!("{name} reads away from Live"),
                schema: object(json!({"type":"object","properties":{"words":{"type":"string"}}})),
                deferred: true,
                listed_by: None,
                run: Rc::new(move |_| {
                    let (gate, started) = (gate.clone(), started.clone());
                    Box::pin(async move {
                        started.borrow_mut().push(name.into());
                        // Both have started before either finishes: they run together.
                        if started.borrow().len() == 2 {
                            gate.notify_waiters();
                        } else {
                            gate.notified().await;
                        }
                        Ok(ToolResult::text("read"))
                    })
                }),
            })
        };
        let ran = Rc::new(RefCell::new(vec![]));
        let tools: Vec<Rc<dyn KernelTool>> = vec![fixture("tempo", false, &ran), make("my_sets"), make("live_manual")];
        let h = harness(tools, |_, n| match n {
            1 => calls(&[
                ("c1", "call_tool", json!({"name": "my_sets", "input": {"words": "ep"}})),
                ("c2", "call_tool", json!({"name": "live_manual", "input": {"words": "warp"}})),
            ]),
            _ => answer("read both"),
        });
        // Run one after the other, the first would wait for the second for ever.
        let ran_both =
            tokio::time::timeout(std::time::Duration::from_secs(5), h.kernel.run("read", Controller::new().signal, Rc::new(|_| Ok(()))));
        ran_both.await.expect("they ran together").unwrap();
        assert_eq!(*started.borrow(), ["my_sets", "live_manual"]);
    })
    .await;
}

#[tokio::test]
async fn what_is_sent_is_the_same_every_time() {
    local(async {
        let build = || {
            let ran = Rc::new(RefCell::new(vec![]));
            let tools: Vec<Rc<dyn KernelTool>> = vec![
                fixture("tempo", false, &ran),
                fixture("judge", true, &ran),
                fixture("arrange", true, &ran),
                fixture("mute", false, &ran),
            ];
            harness(tools, |_, _| answer("ok"))
        };
        let (first, second) = (build(), build());
        first.kernel.run("a", Controller::new().signal, Rc::new(|_| Ok(()))).await.unwrap();
        first.kernel.run("b", Controller::new().signal, Rc::new(|_| Ok(()))).await.unwrap();
        second.kernel.run("a", Controller::new().signal, Rc::new(|_| Ok(()))).await.unwrap();
        let tools = |h: &Harness, index: usize| serde_json::to_string(&h.requests.borrow()[index].tools).unwrap();
        assert_eq!(tools(&first, 0), tools(&first, 1));
        assert_eq!(tools(&first, 0), tools(&second, 0));
    })
    .await;
}
