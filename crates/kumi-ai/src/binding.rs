//! What the agent's loop asks of a model: a model that streams, a request shaped for its provider, and how much
//! conversation it takes beside the instructions and tools. The loop itself (kumi-runtime's kernel) is provider-neutral.
use std::rc::Rc;

use async_trait::async_trait;

use crate::error::LanguageModelError;
use crate::types::{CallOptions, FunctionTool, Prompt, StreamParts};

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

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ContextBudget {
    /// Bytes of conversation (as JSON, roughly 3 per token) past which earlier turns are cleared.
    pub clear_at: f64,
    /// Bytes never sent: past this, all earlier reads and then this turn's older ones are cleared, then the earliest exchanges dropped.
    pub limit: f64,
}

/// Roughly 50k and 130k tokens, leaving room for instructions and tools in every supported model's window.
pub const DEFAULT_BUDGET: ContextBudget = ContextBudget { clear_at: 160.0 * 1024.0, limit: 400.0 * 1024.0 };

/// Bytes a token stands for, as the budget counts them: JSON runs about 3, words more, so this errs toward room.
pub const BYTES_PER_TOKEN: f64 = 3.0;

/// The budget for a model that reads `window` tokens at once: what's left once the instructions and
/// tools (`fixed` bytes) and the answer (`answer` tokens) have their room, cleared from 40% of it as
/// the default budget is. Never less than a few exchanges, nor more than the default.
pub fn budget_for(window: f64, fixed: f64, answer: f64) -> ContextBudget {
    let limit = DEFAULT_BUDGET.limit.min((16.0_f64 * 1024.0).max((window - answer) * BYTES_PER_TOKEN - fixed));
    ContextBudget { clear_at: (limit * 0.4).floor(), limit }
}
