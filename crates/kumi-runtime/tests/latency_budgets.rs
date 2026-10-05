//! The kernel's latency budgets (the Live ones go with the Ableton integration).
//!
//! Latency budgets, counted rather than timed (CI machines are too noisy for milliseconds): the
//! things Kumi's speed is made of. On real Live every Live round trip waits for a display tick,
//! about 100 ms, and every model reply costs seconds, so one more of either is a regression a
//! producer feels. Raising a budget should be a decision, not a side effect.

use std::cell::Cell;
use std::rc::Rc;

use async_trait::async_trait;
use futures::stream::{self, StreamExt};
use kumi_common::abort::{Controller, Signal};
use kumi_runtime::ai::error::LanguageModelError;
use kumi_runtime::ai::types::{
    CallOptions, FinishReason, FinishReasonUnified, InputTokens, OutputTokens, StreamPart, StreamParts, ToolCall, Usage,
};
use kumi_runtime::core::contracts::{JsonObject, KernelTool, StopReason, ToolResult};
use kumi_runtime::core::errors::RuntimeError;
use kumi_runtime::kernel::agent::{create_agent_kernel, AgentKernelOptions, LanguageModel, ModelBinding};
use serde_json::json;

fn signal() -> Signal {
    Controller::new().signal
}

/// A model that answers every call with the same parts, counting its replies.
struct Replaying {
    parts: Vec<StreamPart>,
    replies: Rc<Cell<usize>>,
}

#[async_trait(?Send)]
impl LanguageModel for Replaying {
    async fn do_stream(&self, _options: CallOptions) -> Result<StreamParts, LanguageModelError> {
        self.replies.set(self.replies.get() + 1);
        Ok(stream::iter(self.parts.clone()).boxed_local())
    }
}

struct Plan;

#[async_trait(?Send)]
impl KernelTool for Plan {
    fn name(&self) -> &str {
        "make_changes"
    }
    fn description(&self) -> &str {
        "plan"
    }
    fn input_schema(&self) -> JsonObject {
        match json!({"type": "object"}) {
            serde_json::Value::Object(map) => map,
            _ => unreachable!(),
        }
    }
    async fn execute(&self, _input: JsonObject, _signal: Signal) -> Result<ToolResult, RuntimeError> {
        Ok(ToolResult { text: "{\"done\":[]}".into(), reply: Some("Done: Tempo 120 → 124 BPM.".into()), ..ToolResult::default() })
    }
}

#[tokio::test]
async fn budget_a_plan_that_finishes_the_request_is_one_model_reply_with_no_reply_written_after_it() {
    let replies = Rc::new(Cell::new(0));
    let parts = vec![
        StreamPart::ToolCall(ToolCall {
            tool_call_id: "c1".into(),
            tool_name: "make_changes".into(),
            input: kumi_common::js::json::stringify(&json!({"steps": [{"tool": "set_tempo", "input": {"tempo": 124}}], "final": true})),
            provider_executed: None,
            dynamic: None,
            provider_metadata: None,
        }),
        StreamPart::Finish {
            usage: Usage {
                input_tokens: InputTokens { total: Some(1.0), no_cache: Some(1.0), cache_read: Some(0.0), cache_write: Some(0.0) },
                output_tokens: OutputTokens { total: Some(1.0), text: Some(1.0), reasoning: Some(0.0) },
                raw: None,
            },
            finish_reason: FinishReason { unified: FinishReasonUnified::ToolCalls, raw: Some("tool_use".into()) },
            provider_metadata: None,
        },
    ];
    let binding = ModelBinding {
        id: "test/budget".into(),
        model: Rc::new(Replaying { parts, replies: replies.clone() }),
        prepare: Box::new(|request| CallOptions { prompt: request.messages, tools: Some(request.tools), ..CallOptions::default() }),
        budget: None,
    };
    let kernel = create_agent_kernel(AgentKernelOptions {
        binding,
        instructions: "budget".into(),
        tools: vec![Rc::new(Plan)],
        signal: signal(),
        checkpoint: None,
        max_steps: None,
        budget: None,
    })
    .unwrap();
    let result = kernel.run("set the tempo to 124", signal(), Rc::new(|_| Ok(()))).await.unwrap();
    assert_eq!(result.stop_reason, StopReason::Completed);
    assert_eq!(replies.get(), 1, "model replies for a finished plan: {}, each seconds long", replies.get());
    kernel.close().await;
}
