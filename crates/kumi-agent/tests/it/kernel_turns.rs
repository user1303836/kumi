//! What the kernel asks a model and keeps between turns: a step asked again sends what it first sent, and each turn
//! carries every turn before it whole.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use async_trait::async_trait;
use futures::stream::{self, StreamExt};
use kumi_agent::core::contracts::StopReason;
use kumi_agent::kernel::agent::{create_agent_kernel, AgentKernelOptions};
use kumi_ai::binding::{LanguageModel, ModelBinding};
use kumi_ai::error::LanguageModelError;
use kumi_ai::types::{CallOptions, FinishReason, FinishReasonUnified, Message, StreamPart, StreamParts, Usage};
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
