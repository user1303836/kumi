//! Exact host validation and framing around standalone transaction managers.
use super::*;
use kumi_common::abort::Signal;

fn midi_preview_params(params: &Value) -> bool {
    has_only(params, &["trackRef", "sceneIndex", "name", "length", "notes"])
        && params["trackRef"].is_string()
        && is_integer_in_range(&params["sceneIndex"], 0.0, 100_000.0)
        && params["name"].is_string()
        && params["length"].as_f64().is_some_and(|length| length.is_finite() && length > 0.0 && length <= 1024.0)
        && params["notes"].is_array()
}
impl McpHost {
    pub async fn dispatch_managed_tool(&self, call: &ToolCall, signal: Option<&Signal>) -> Option<Result<Option<Value>, LiveError>> {
        let args = call.arguments.as_ref().unwrap_or(&Value::Null);
        Some(if call.asynchronous {
            match call.name.as_str() {
                "live_batch_preview" => Ok(Some(self.live_batch_preview_async(&call.id, args).await)),
                "live_batch_apply" => Ok(self.live_batch_apply_async(&call.id, args, signal).await),
                "live_midi_clip_preview" => self.live_midi_preview_async(&call.id, args).await.map(Some),
                "live_midi_clip_apply" => self.live_midi_apply_async(&call.id, args, signal).await.map(Some),
                _ => return None,
            }
        } else {
            match call.name.as_str() {
                "live_midi_clip_preview" => Ok(Some(self.live_midi_preview(&call.id, args))),
                "live_midi_clip_apply" => Ok(Some(self.live_midi_apply(&call.id, args))),
                _ => return None,
            }
        })
    }
    pub(super) fn transaction_context(&self, params: &Value, signal: Option<&Signal>, base: f64) -> LiveOperationContext {
        LiveOperationContext {
            signal: signal.cloned(),
            deadline_ms: Some(self.deadline(base)),
            idempotency_key: params["idempotencyKey"].as_str().map(str::to_owned),
            transaction_id: params["transactionId"].as_str().map(str::to_owned),
            ..Default::default()
        }
    }
    pub async fn live_batch_preview_async(&self, id: &Value, params: &Value) -> Value {
        if !has_only(params, &["operations"]) || !params["operations"].is_array() {
            return error(id, -32602, "a bounded operations array is required", None);
        }
        let result = async {
            for operation in params["operations"].as_array().unwrap() {
                let owner = operation["kind"].as_str().and_then(|kind| {
                    BATCH_OPERATION_POLICY_TOOLS.iter().find(|(candidate, _)| *candidate == kind).map(|(_, owner)| *owner)
                });
                let Some(owner) = owner else {
                    // The source's object lookup also sees Object.prototype keys. Their values can
                    // never equal a catalog tool name, but their string forms appear in the refusal.
                    let inherited = match operation["kind"].as_str() {
                        Some("__proto__") => Some("[object Object]".to_owned()),
                        Some("constructor") => Some("function Object() { [native code] }".to_owned()),
                        Some(
                            name @ ("__defineGetter__"
                            | "__defineSetter__"
                            | "hasOwnProperty"
                            | "__lookupGetter__"
                            | "__lookupSetter__"
                            | "isPrototypeOf"
                            | "propertyIsEnumerable"
                            | "toString"
                            | "valueOf"
                            | "toLocaleString"),
                        ) => Some(format!("function {name}() {{ [native code] }}")),
                        _ => None,
                    };
                    return Ok(transaction_error(
                        id,
                        &inherited
                            .map(|owner| format!("transaction batch contains an operation denied by the deployment policy ({owner})"))
                            .unwrap_or_else(|| "transaction batch operation kind is not in the composable allowlist".into()),
                    ));
                };
                if !self.policy_allows_tool(Some(owner))? {
                    return Ok(transaction_error(
                        id,
                        &format!("transaction batch contains an operation denied by the deployment policy ({owner})"),
                    ));
                }
            }
            Ok(success_text(id, &self.batch_transactions.preview_async(params).await?))
        }
        .await;
        result.unwrap_or_else(|e| {
            adapter_tool_error(
                id,
                &e,
                "Batch preview failed without mutation; every operation must resolve against fresh authoritative state.",
            )
        })
    }
    pub async fn live_batch_apply_async(&self, id: &Value, params: &Value, signal: Option<&Signal>) -> Option<Value> {
        if !valid_transaction_params(params, "apply") {
            return Some(error(id, -32602, "transactionId, confirmation=apply, and idempotencyKey are required", None));
        }
        if signal.is_some_and(Signal::is_cancelled) {
            return None;
        }
        let result = self
            .batch_transactions
            .apply_async(
                params["transactionId"].as_str().unwrap(),
                &params["confirmation"],
                params["idempotencyKey"].as_str().unwrap(),
                Some(&self.transaction_context(params, signal, reads::AUDITION_DEADLINE_MS)),
            )
            .await;
        Some(match result {
            Ok(value) => success_text(id, &value),
            Err(e) => adapter_tool_error(
                id,
                &e,
                "Batch apply may be uncertain; reconcile with the exact original idempotency key and do not retry blindly.",
            ),
        })
    }
    pub async fn live_midi_preview_async(&self, id: &Value, params: &Value) -> Result<Value, LiveError> {
        if !midi_preview_params(params) {
            return Ok(error(id, -32602, "Invalid MIDI clip preview", None));
        }
        Ok(success_text(id, &self.midi_transactions.preview_async(&mut params.clone()).await?))
    }
    pub fn live_midi_preview(&self, id: &Value, params: &Value) -> Value {
        if !midi_preview_params(params) {
            return error(id, -32602, "Invalid MIDI clip preview", None);
        }
        match self.midi_transactions.preview(&mut params.clone()) {
            Ok(value) => success_text(id, &value),
            Err(e) => adapter_tool_error(id, &e, "MIDI preview failed without mutation; verify the track, empty slot, and bounded notes."),
        }
    }
    pub async fn live_midi_apply_async(&self, id: &Value, params: &Value, signal: Option<&Signal>) -> Result<Value, LiveError> {
        if !valid_transaction_params(params, "apply") {
            return Ok(error(id, -32602, "transactionId, confirmation=apply, and idempotencyKey are required", None));
        }
        Ok(success_text(
            id,
            &self
                .midi_transactions
                .apply_async(
                    params["transactionId"].as_str().unwrap(),
                    &params["confirmation"],
                    params["idempotencyKey"].as_str().unwrap(),
                    Some(&self.transaction_context(params, signal, 30_000.0)),
                )
                .await?,
        ))
    }
    pub fn live_midi_apply(&self, id: &Value, params: &Value) -> Value {
        if !valid_transaction_params(params, "apply") {
            return error(id, -32602, "transactionId, confirmation=apply, and idempotencyKey are required", None);
        }
        match self.midi_transactions.apply(
            params["transactionId"].as_str().unwrap(),
            &params["confirmation"],
            params["idempotencyKey"].as_str().unwrap(),
        ) {
            Ok(value) => success_text(id, &value),
            Err(e) => adapter_tool_error(id, &e, "MIDI apply did not complete; read the target slot before retrying."),
        }
    }
}
