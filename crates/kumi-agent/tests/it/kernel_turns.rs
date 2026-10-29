//! What the kernel asks a model and keeps between turns: a step asked again sends what it first sent, each turn
//! carries every turn before it whole, and each gives the messages it settled, once. What the model thinks, and
//! each call's tokens, are told as they come.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use async_trait::async_trait;
use futures::stream::{self, StreamExt};
use kumi_agent::core::contracts::{KernelEvent, StopReason};
use kumi_agent::kernel::agent::{create_agent_kernel, no_room, AgentKernelOptions};
use kumi_ai::binding::{budget_for, LanguageModel, ModelBinding};
use kumi_ai::error::LanguageModelError;
use kumi_ai::types::{CallOptions, FinishReason, FinishReasonUnified, InputTokens, Message, OutputTokens, StreamPart, StreamParts, Usage};
use kumi_common::abort::{Controller, Signal};
use kumi_net::error::ApiCallError;
use serde_json::{json, Value};

fn signal() -> Signal {
    Controller::new().signal
}

/// A model that's busy at its first call and answers every other with a line of words; it keeps every request it's sent.
struct BusyOnce {
    asked: Rc<RefCell<Vec<CallOptions>>>,
    calls: Cell<usize>,
}

#[async_trait(?Send)]
impl LanguageModel for BusyOnce {
    async fn do_stream(&self, options: CallOptions) -> Result<StreamParts, LanguageModelError> {
        self.asked.borrow_mut().push(options);
        let call = self.calls.get();
        self.calls.set(call + 1);
        if call == 0 {
            return Err(LanguageModelError::ApiCall(ApiCallError::new("busy", "https://example.invalid", None, Some(503))));
        }
        let id = format!("t{call}");
        Ok(stream::iter(vec![
            StreamPart::TextStart { id: id.clone(), provider_metadata: None },
            StreamPart::TextDelta { id: id.clone(), delta: format!("answer {call}"), provider_metadata: None },
            StreamPart::TextEnd { id, provider_metadata: None },
            StreamPart::Finish {
                usage: Usage::default(),
                finish_reason: FinishReason { unified: FinishReasonUnified::Stop, raw: Some("stop".into()) },
                provider_metadata: None,
            },
        ])
        .boxed_local())
    }
}

fn wire(request: &CallOptions) -> Value {
    serde_json::to_value(request).unwrap()
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn a_step_asked_again_sends_what_it_first_sent_and_each_turn_carries_the_ones_before() {
    let asked = Rc::new(RefCell::new(vec![]));
    let binding = ModelBinding {
        id: "test/busy".into(),
        model: Rc::new(BusyOnce { asked: asked.clone(), calls: Cell::new(0) }),
        prepare: Box::new(|request| CallOptions {
            prompt: [vec![Message::System { content: request.instructions, provider_options: None }], request.messages].concat(),
            tools: Some(request.tools),
            ..CallOptions::default()
        }),
        budget: None,
    };
    let kernel = create_agent_kernel(AgentKernelOptions {
        conversation: None,
        binding,
        instructions: "be brief".into(),
        tools: vec![],
        signal: signal(),
        checkpoint: None,
        max_steps: None,
        budget: None,
    })
    .unwrap();
    let mut settled: Vec<Value> = vec![];
    for (turn, said) in ["make the bass wider", "now the drums", "and the vocal"].into_iter().enumerate() {
        let result = kernel.run(said, signal(), Rc::new(|_| Ok(()))).await.unwrap();
        assert_eq!(result.stop_reason, StopReason::Completed);
        let asked = asked.borrow();
        // The first turn's step was asked twice (busy, then answered), each turn after it once.
        assert_eq!(asked.len(), turn + 2);
        if turn == 0 {
            assert_eq!(wire(&asked[0]), wire(&asked[1]), "asked again, the same request");
        }
        // What came before, then what's said now, after the instructions.
        let prompt = wire(asked.last().unwrap())["prompt"].as_array().unwrap().clone();
        assert_eq!(prompt[0], json!({"role": "system", "content": "be brief"}));
        assert_eq!(prompt[1..prompt.len() - 1], settled[..], "turn {turn}");
        assert_eq!(prompt.last().unwrap()["content"][0]["text"], said);
        settled = kernel.checkpoint().unwrap().messages.iter().map(|message| serde_json::to_value(message).unwrap()).collect();
        assert_eq!(settled.len(), 2 * (turn + 1));
        assert_eq!(settled[settled.len() - 2], *prompt.last().unwrap());
        assert_eq!(settled[settled.len() - 1]["content"][0]["text"], format!("answer {}", turn + 1));
    }
    kernel.close().await;
}

/// A model that answers each call with a line of words, and refuses (as a bad request, never asked again) the request
/// "fail".
struct Answers {
    calls: Cell<usize>,
}

#[async_trait(?Send)]
impl LanguageModel for Answers {
    async fn do_stream(&self, options: CallOptions) -> Result<StreamParts, LanguageModelError> {
        let said = wire(&options)["prompt"].as_array().and_then(|prompt| prompt.last().cloned()).unwrap_or_default();
        if said["content"][0]["text"] == "fail" {
            return Err(LanguageModelError::ApiCall(ApiCallError::new("bad request", "https://example.invalid", None, Some(400))));
        }
        let call = self.calls.get() + 1;
        self.calls.set(call);
        let id = format!("t{call}");
        Ok(stream::iter(vec![
            StreamPart::TextStart { id: id.clone(), provider_metadata: None },
            StreamPart::TextDelta { id: id.clone(), delta: format!("answer {call}"), provider_metadata: None },
            StreamPart::TextEnd { id, provider_metadata: None },
            StreamPart::Finish {
                usage: Usage::default(),
                finish_reason: FinishReason { unified: FinishReasonUnified::Stop, raw: Some("stop".into()) },
                provider_metadata: None,
            },
        ])
        .boxed_local())
    }
}

/// A model whose window is small, as a local one's can be: a request past the room it has is refused before it's sent,
/// saying how much room there is, and isn't sent when it comes again either. The conversation goes on without it (#663).
#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn a_request_past_a_small_models_room_is_refused_unsent_and_the_conversation_goes_on() {
    let model = Rc::new(Answers { calls: Cell::new(0) });
    let binding = ModelBinding {
        id: "test/small".into(),
        model: model.clone(),
        prepare: Box::new(|request| CallOptions { prompt: request.messages, tools: Some(request.tools), ..CallOptions::default() }),
        // A window of 8,192 tokens, 2,048 of them kept for the answer: about 18 KB of conversation.
        budget: Some(Box::new(|fixed| budget_for(8192.0, fixed as f64, 2048.0))),
    };
    let kernel = create_agent_kernel(AgentKernelOptions {
        conversation: None,
        binding,
        instructions: "be brief".into(),
        tools: vec![],
        signal: signal(),
        checkpoint: None,
        max_steps: None,
        budget: None,
    })
    .unwrap();
    let paste = "x".repeat(20_000);
    for _ in 0..2 {
        let error = kernel.run(&paste, signal(), Rc::new(|_| Ok(()))).await.unwrap_err();
        assert!(error.kumi().is_some_and(no_room), "the session knows it wasn't sent: {error:?}");
        let refused = error.to_string();
        assert!(
            refused.contains("test/small has room for about 17 KB at once, and this message comes to 20 KB with what Kumi reads of your Set: send part of it, or choose a model with more room with /model."),
            "{refused}"
        );
        assert_eq!(model.calls.get(), 0, "the model is never sent it");
    }
    kernel.run("make the bass wider", signal(), Rc::new(|_| Ok(()))).await.unwrap();
    assert_eq!(model.calls.get(), 1);
    let kept: Vec<Value> = kernel.checkpoint().unwrap().messages.iter().map(|message| serde_json::to_value(message).unwrap()).collect();
    assert_eq!(kept.len(), 2, "the refused request isn't kept: {kept:?}");
    assert_eq!(kept[0]["content"][0]["text"], "make the bass wider");
    // A side question has its own limit, said the same way.
    let refused = kernel.aside(&"x".repeat(16 * 1024 + 1), signal(), Rc::new(|_| {})).await.unwrap_err().to_string();
    assert!(refused.contains("This side question is 17 KB, and one takes up to 16 KB: ask a shorter one."), "{refused}");
    kernel.close().await;
}

/// What the session log keeps of each turn: the messages it settled, once. A run that settles none (stopped before it
/// began, or failed before a step finished) gives none, not the last turn's again.
#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn each_turn_gives_the_messages_it_settled_and_a_run_that_settled_none_gives_none() {
    let binding = ModelBinding {
        id: "test/answers".into(),
        model: Rc::new(Answers { calls: Cell::new(0) }),
        prepare: Box::new(|request| CallOptions { prompt: request.messages, tools: Some(request.tools), ..CallOptions::default() }),
        budget: None,
    };
    let kernel = create_agent_kernel(AgentKernelOptions {
        conversation: None,
        binding,
        instructions: "be brief".into(),
        tools: vec![],
        signal: signal(),
        checkpoint: None,
        max_steps: None,
        budget: None,
    })
    .unwrap();
    let words = |messages: Vec<Value>| -> Vec<String> {
        messages.iter().map(|message| message["content"][0]["text"].as_str().unwrap_or("").to_owned()).collect()
    };
    assert!(kernel.last_turn().is_empty());
    kernel.run("make the bass wider", signal(), Rc::new(|_| Ok(()))).await.unwrap();
    assert_eq!(words(kernel.last_turn()), ["make the bass wider", "answer 1"]);
    // Stopped before it began.
    let stopped = Controller::new();
    stopped.abort();
    let result = kernel.run("now the drums", stopped.signal, Rc::new(|_| Ok(()))).await.unwrap();
    assert_eq!(result.stop_reason, StopReason::Cancelled);
    assert!(kernel.last_turn().is_empty());
    // Refused before its step finished.
    assert!(kernel.run("fail", signal(), Rc::new(|_| Ok(()))).await.is_err());
    assert!(kernel.last_turn().is_empty());
    // The next turn gives its own, the tail of what's settled.
    kernel.run("and the vocal", signal(), Rc::new(|_| Ok(()))).await.unwrap();
    assert_eq!(words(kernel.last_turn()), ["and the vocal", "answer 2"]);
    let settled: Vec<Value> = kernel.checkpoint().unwrap().messages.iter().map(|message| serde_json::to_value(message).unwrap()).collect();
    assert_eq!(settled[settled.len() - 2..], kernel.last_turn()[..]);
    assert_eq!(settled.len(), 4);
    kernel.close().await;
}

/// A model that thinks before it answers: its first call says two parts of its reasoning, then its provider breaks off
/// busy; its second thinks again and answers, with its own tokens.
struct ThinksFirst {
    calls: Cell<usize>,
}

#[async_trait(?Send)]
impl LanguageModel for ThinksFirst {
    async fn do_stream(&self, _: CallOptions) -> Result<StreamParts, LanguageModelError> {
        let call = self.calls.get();
        self.calls.set(call + 1);
        let part = |id: &str, text: &str| {
            [
                StreamPart::ReasoningStart { id: id.into(), provider_metadata: None },
                StreamPart::ReasoningDelta { id: id.into(), delta: text.into(), provider_metadata: None },
                StreamPart::ReasoningEnd { id: id.into(), provider_metadata: None },
            ]
        };
        let mut parts: Vec<StreamPart> = [part("r0", "**Weighing the kick**"), part("r1", "**Checking the sidechain**")].concat();
        if call == 0 {
            let busy = ApiCallError::new("busy", "https://example.invalid", None, Some(503));
            parts.push(StreamPart::Error { error: LanguageModelError::ApiCall(busy) });
        } else {
            parts.extend([
                StreamPart::TextStart { id: "t".into(), provider_metadata: None },
                StreamPart::TextDelta { id: "t".into(), delta: "Sidechain it.".into(), provider_metadata: None },
                StreamPart::TextEnd { id: "t".into(), provider_metadata: None },
                StreamPart::Finish {
                    usage: Usage {
                        input_tokens: InputTokens { total: Some(1200.), cache_read: Some(900.), ..InputTokens::default() },
                        output_tokens: OutputTokens { total: Some(80.), ..OutputTokens::default() },
                        raw: None,
                    },
                    finish_reason: FinishReason { unified: FinishReasonUnified::Stop, raw: Some("stop".into()) },
                    provider_metadata: None,
                },
            ]);
        }
        Ok(stream::iter(parts).boxed_local())
    }
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn what_the_model_thinks_and_each_calls_tokens_are_told_as_they_come_and_thinking_is_never_output() {
    let binding = ModelBinding {
        id: "test/thinks".into(),
        model: Rc::new(ThinksFirst { calls: Cell::new(0) }),
        prepare: Box::new(|request| CallOptions { prompt: request.messages, ..CallOptions::default() }),
        budget: None,
    };
    let kernel = create_agent_kernel(AgentKernelOptions {
        conversation: None,
        binding,
        instructions: "be brief".into(),
        tools: vec![],
        signal: signal(),
        checkpoint: None,
        max_steps: None,
        budget: None,
    })
    .unwrap();
    let events = Rc::new(RefCell::new(vec![]));
    let sink = events.clone();
    let result = kernel
        .run(
            "punch up the kick",
            signal(),
            Rc::new(move |event| {
                sink.borrow_mut().push(event);
                Ok(())
            }),
        )
        .await
        .unwrap();
    assert_eq!(result.stop_reason, StopReason::Completed);
    let events = events.borrow();
    let thought = |text: &str| KernelEvent::Reasoning { text: text.into() };
    let thinking = [thought("**Weighing the kick**"), thought("\n\n"), thought("**Checking the sidechain**")];
    // Each part of the summary in a paragraph of its own. Thinking isn't output: the call that broke off after it is
    // tried again.
    assert_eq!(events[..3], thinking, "{events:?}");
    assert!(matches!(events[3], KernelEvent::Retry { .. }), "{events:?}");
    assert_eq!(events[4..7], thinking, "{events:?}");
    // The answer, then that call's own tokens as it finished.
    assert_eq!(events[7], KernelEvent::Text { text: "Sidechain it.".into() });
    let used = kumi_types::contracts::Usage { input_tokens: 1200., output_tokens: 80., cache_read_tokens: 900., cache_write_tokens: 0. };
    assert_eq!(events[8..], [KernelEvent::Usage(used.clone())]);
    assert_eq!(result.usage, Some(used));
    // What the model thought isn't its answer's words.
    let settled = serde_json::to_value(&kernel.checkpoint().unwrap().messages).unwrap();
    let words: Vec<&Value> = settled[1]["content"].as_array().unwrap().iter().filter(|part| part["type"] == "text").collect();
    assert_eq!(words.len(), 1);
    assert_eq!(words[0]["text"], "Sidechain it.");
    kernel.close().await;
}

/// A tool's result in the plain words a fallback model reads is cut between characters, never through one (#675).
#[test]
fn a_part_of_the_plain_words_is_cut_between_characters() {
    use kumi_ai::types::{ToolPart, ToolResultOutput, ToolResultPart};
    let long = format!("{}🎹 and more", "a".repeat(4 * 1024 - 1));
    let read = Message::Tool {
        content: vec![ToolPart::ToolResult(ToolResultPart {
            tool_call_id: "x".into(),
            tool_name: "read".into(),
            output: ToolResultOutput::Text { value: long, provider_options: None },
            provider_options: None,
        })],
        provider_options: None,
    };
    let plain = serde_json::to_string(&kumi_agent::kernel::agent::plain_words(&[read])).unwrap();
    assert!(plain.contains(&format!("[read returned: {}…]", "a".repeat(4 * 1024 - 1))), "{}", &plain[..120]);
    assert!(!plain.contains('\u{FFFD}'));
}
