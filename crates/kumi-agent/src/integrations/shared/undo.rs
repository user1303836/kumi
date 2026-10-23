//! undo_change: every DAW's way back for one of Kumi's changes, through its HISTORY.
use crate::core::history::{ChangeHost, History};
use kumi_common::abort::Signal;
use kumi_types::contracts::{JsonObject, ToolResult};
use kumi_types::errors::RuntimeError;
use serde_json::{json, Value};

/// undo_change's answer: HISTORY's undo of the change asked for (the last when none is named), with the producer's
/// line when it ends the answer. Whatever else the input holds, nothing is discarded.
pub async fn undo_change<H: ChangeHost>(history: &History<H>, input: &JsonObject, signal: Signal) -> Result<ToolResult, RuntimeError> {
    undo_change_with(history, input, signal, false).await
}

/// undo_change's answer for a DAW whose undo can be told to discard: with `discard`, the change's values go back even
/// though the producer changed them since.
pub async fn undo_change_with<H: ChangeHost>(
    history: &History<H>,
    input: &JsonObject,
    signal: Signal,
    discard: bool,
) -> Result<ToolResult, RuntimeError> {
    let outcome = history.undo(input.get("change").and_then(Value::as_str).unwrap_or("last"), signal, discard).await?;
    let reply = if input.get("final") == Some(&json!(true)) && !outcome.is_error {
        outcome.record.as_ref().filter(|r| !r.title.is_empty()).map(|r| format!("Undone: {}.", r.title))
    } else {
        None
    };
    Ok(ToolResult { text: outcome.for_model(), is_error: outcome.is_error, reply, ..Default::default() })
}
