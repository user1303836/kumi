//! Global groove amount and exact Groove Pool entry edits.
use super::*;
use kumi_common::{
    abort::{Signal, SignalExt},
    js::json as js_json,
};
use tuning::field;
const FIELDS: [&str; 6] = ["name", "base", "quantizationAmount", "randomAmount", "timingAmount", "velocityAmount"];
fn row(read: &Value, reference: &Value, label: &str) -> Result<Option<Value>, LiveError> {
    let grooves = field(read, "grooves")?;
    let Some(grooves) = grooves.filter(|v| !v.is_null()) else { return Ok(None) };
    let rows = grooves.as_array().ok_or_else(|| LiveError::type_error(format!("({label}.grooves ?? []).find is not a function")))?;
    for candidate in rows {
        if field(candidate, "ref")? == Some(reference) {
            return Ok(Some(candidate.clone()));
        }
    }
    Ok(None)
}
fn outcome(id: &Value, t: &Value, read: &Value) -> Value {
    let mut out = json!({
    "transactionId":t["id"],
    "state":"applied"}
    );
    if let Some(v) = read.get("revision") {
        out["revision"] = v.clone();
    }
    out["idempotent"] = json!(false);
    success_text(id, &out)
}

impl McpHost {
    pub async fn dispatch_groove_tool(&self, call: &ToolCall, signal: Option<&Signal>) -> Option<Result<Option<Value>, LiveError>> {
        let p = call.arguments.as_ref().unwrap_or(&Value::Null);
        Some(Ok(match call.name.as_str() {
            "live_groove_preview" => Some(self.live_groove_preview_async(&call.id, p).await),
            "live_groove_apply" => self.live_groove_apply_async(&call.id, p, signal).await,
            _ => return None,
        }))
    }

    pub async fn live_groove_preview_async(&self, id: &Value, params: &Value) -> Value {
        if !has_only(
            params,
            &[
                "action",
                "grooveAmount",
                "grooveRef",
                "name",
                "base",
                "quantizationAmount",
                "randomAmount",
                "timingAmount",
                "velocityAmount",
            ],
        ) {
            return error(id, -32602, "action is required", None);
        }

        if params["action"] != "set-amount" && params["action"] != "edit" {
            return error(id, -32602, "action must be set-amount or edit", None);
        }
        let result = async {
            let status = self.fresh_status(Some(&LiveOperationContext::with_deadline(self.deadline(reads::AUDITION_DEADLINE_MS)))).await?;
            if !status.connected || !status.capabilities.iter().any(|c| c.as_str() == "session.read") {
                return Err(LiveError::error("session read capability is unavailable"));
            }

            let operation = if params["action"] == "set-amount" { "groove.set" } else { "groove.edit" };
            if !status.has_operation(operation) || !status.has_operation("groove.read") {
                return Err(LiveError::error(format!("{operation} is unavailable")));
            }

            let adapter = self.async_adapter();
            let set = self.set_identity_view(None).await?;
            if !is_non_empty_string(&set["objectIdentity"], 256) {
                return Err(LiveError::error("Set identity is not authoritative"));
            }

            let context = LiveOperationContext::with_deadline(self.deadline(reads::AUDITION_DEADLINE_MS));
            let read = adapter
                .invoke_async(
                    &LiveInvocation::new(
                        "groove.read",
                        json!({
                        "setRef":set["ref"]}
                        ),
                    ),
                    Some(&context),
                )
                .await?;

            if !field(&read, "revision")?.is_some_and(|v| is_non_empty_string(v, 64)) {
                return Err(LiveError::error("groove revision is unavailable"));
            }
            let (payload, prior) = if params["action"] == "set-amount" {
                if !params["grooveAmount"].as_f64().is_some_and(|n| n.is_finite() && (0.0..=1.3125).contains(&n)) {
                    return Ok(error(id, -32602, "grooveAmount must be 0-1.3125", None));
                }

                (
                    json!({
                    "action":params["action"],
                    "setRef":set["ref"],
                    "grooveAmount":params["grooveAmount"],
                    "expectedObjectIdentity":set["objectIdentity"],
                    "expectedRevision":read["revision"]}
                    ),
                    json!({
                    "grooveAmount":read["grooveAmount"]}
                    ),
                )
            } else {
                if !is_non_empty_string(&params["grooveRef"], 256) {
                    return Ok(error(id, -32602, "grooveRef is required for edit", None));
                }
                let Some(groove) = row(&read, &params["grooveRef"], "read")?.filter(|r| is_non_empty_string(&r["objectIdentity"], 256))
                else {
                    return Ok(transaction_error(id, "groove reference is unknown"));
                };

                if FIELDS.iter().all(|f| params.get(*f).is_none()) {
                    return Ok(error(id, -32602, "at least one groove field is required", None));
                }
                let mut payload = json!({"action":params["action"],"ref":params["grooveRef"]});
                for f in FIELDS {
                    let Some(v) = params.get(f) else { continue };
                    if f == "name" && !is_non_empty_string(v, 256) {
                        return Ok(error(id, -32602, "name is invalid", None));
                    }
                    if f == "base" && !is_integer_in_range(v, 0.0, 16.0) {
                        return Ok(error(id, -32602, "base is invalid", None));
                    }
                    if f != "name"
                        && f != "base"
                        && !v.as_f64().is_some_and(|n| n.is_finite() && n >= if f == "velocityAmount" { -100.0 } else { 0.0 } && n <= 100.0)
                    {
                        return Ok(error(
                            id,
                            -32602,
                            &format!("{f} must be {}", if f == "velocityAmount" { "-100-100" } else { "0-100" }),
                            None,
                        ));
                    }
                    payload[f] = v.clone();
                }

                payload["expectedObjectIdentity"] = groove["objectIdentity"].clone();
                payload["expectedRevision"] = read["revision"].clone();
                let mut prior = json!({});
                for f in FIELDS {
                    prior[f] = groove[f].clone();
                }
                (payload, prior)
            };
            let fence = js_json::stringify(&json!({"action":params["action"],"payload":payload,"revision":read["revision"]}));
            let t = json!({
            "id":tempo::transaction_id("groove"),
            "epoch":status.epoch,
            "kind":"groove",
            "fence":fence,
            "payload":payload,
            "prior":prior,
            "expiresAt":kumi_common::time::now_ms_f64()+TRANSACTION_TTL_MS,
            "state":"previewed"}
            );
            self.retain_bounded_transaction(&self.clip_lifecycle_transactions, t.clone(), "groove")?;

            Ok(success_text(
                id,
                &json!({
                "transactionId":t["id"],
                "epoch":t["epoch"],
                "action":params["action"],
                "prior":prior,
                "impact":if params["action"]=="set-amount"{
                "edits-global-groove-amount-audible"}
                else{
                "edits-groove-pool-entry"}
                ,
                "confirmation":"apply",
                "expiresAt":t["expiresAt"]}
                ),
            ))
        }
        .await;
        result.unwrap_or_else(|e| adapter_tool_error(id, &e, "Groove preview requires fresh authoritative state."))
    }
    pub async fn live_groove_apply_async(&self, id: &Value, params: &Value, signal: Option<&Signal>) -> Option<Value> {
        if !valid_transaction_params(params, "apply") {
            return Some(error(id, -32602, "transactionId, confirmation=apply, and idempotencyKey are required", None));
        }

        let Some(record) = self.clip_lifecycle_transactions.get(params["transactionId"].as_str().unwrap()) else {
            return Some(transaction_error(id, "Unknown or expired groove transaction"));
        };
        let t = record.borrow().clone();
        if t["kind"] != "groove"
            || (t["state"] == "previewed" && t["expiresAt"].as_f64().is_some_and(|n| n <= kumi_common::time::now_ms_f64()))
        {
            return Some(transaction_error(id, "Unknown or expired groove transaction"));
        }

        if t["state"] == "applied" && t["applyKey"] == params["idempotencyKey"] {
            return Some(success_text(
                id,
                &json!({
                "transactionId":t["id"],
                "state":"applied",
                "idempotent":true}
                ),
            ));
        }

        let reconciliation = t["state"] == "uncertain" && t["applyKey"] == params["idempotencyKey"];
        if t["state"] != "previewed" && !reconciliation {
            return Some(transaction_error(id, "Transaction is no longer applicable"));
        }
        if signal.is_some_and(Signal::aborted) {
            return None;
        }

        let result = async {
            if reconciliation {
                self.fresh_status(Some(&LiveOperationContext::with_deadline(self.deadline(reads::AUDITION_DEADLINE_MS)))).await?;
            }
            let status = self.require_connected(Some("session.read"))?;
            if json!(status.epoch) != t["epoch"] {
                return Ok(transaction_error(id, "Live connection epoch changed; preview again"));
            }

            let adapter = self.async_adapter();
            let context = self.transaction_context(params, signal, reads::AUDITION_DEADLINE_MS);
            let payload = &t["payload"];

            if !reconciliation {
                let set = self.set_identity_view(Some(&context)).await?;
                let before = adapter
                    .invoke_async(
                        &LiveInvocation::new(
                            "groove.read",
                            json!({
                            "setRef":set["ref"]}
                            ),
                        ),
                        Some(&context),
                    )
                    .await?;
                let mut fence = json!({
                "action":payload["action"],
                "payload":payload}
                );
                if let Some(v) = field(&before, "revision")? {
                    fence["revision"] = v.clone();
                }
                if js_json::stringify(&fence) != t["fence"] {
                    return Ok(transaction_error(id, "groove state changed since preview; preview again"));
                }
            }

            let action = &payload["action"];
            let operation = if action == "set-amount" { "groove.set" } else { "groove.edit" };
            {
                let mut row = record.borrow_mut();
                row["state"] = json!("applying");
                row["applyKey"] = params["idempotencyKey"].clone();
            }

            let mut args = payload.clone();
            args.as_object_mut().unwrap().remove("action");
            let result = adapter.invoke_async(&LiveInvocation::new(operation, args), Some(&context)).await?;
            if field(&result, "changed")? != Some(&json!(true)) {
                return Err(LiveError::error("groove change was not confirmed"));
            }

            let set = self.set_identity_view(Some(&context)).await?;
            let verified = adapter
                .invoke_async(
                    &LiveInvocation::new(
                        "groove.read",
                        json!({
                        "setRef":set["ref"]}
                        ),
                    ),
                    Some(&context),
                )
                .await?;

            if action == "set-amount" {
                if !same_live_value(field(&verified, "grooveAmount")?, payload.get("grooveAmount")) {
                    return Err(LiveError::error("groove amount postcondition was not confirmed"));
                }
            } else {
                let groove = row(&verified, &payload["ref"], "verified")?
                    .ok_or_else(|| LiveError::error("edited groove disappeared after apply"))?;
                for f in FIELDS {
                    if let Some(v) = payload.get(f) {
                        if !same_live_value(groove.get(f), Some(v)) {
                            return Err(LiveError::error("groove postcondition was not confirmed"));
                        }
                    }
                }
            }

            {
                let mut row = record.borrow_mut();
                row["applyKey"] = params["idempotencyKey"].clone();
                row["state"] = json!("applied");
            }
            Ok(outcome(id, &t, &verified))
        }
        .await;
        Some(
            result.unwrap_or_else(|e| {
                self.apply_failed(id, &record, &e, "Groove state is uncertain; perform fresh discovery before retrying.")
            }),
        )
    }
    pub async fn undo_groove_async(&self, id: &Value, params: &Value, signal: Option<&Signal>) -> Value {
        let Some(record) = params["transactionId"]
            .as_str()
            .and_then(|id| self.clip_lifecycle_transactions.get(id))
            .filter(|r| r.borrow()["kind"] == "groove")
        else {
            return transaction_error(id, "Unknown or expired groove transaction");
        };
        let t = record.borrow().clone();
        if t["state"] == "undone" && t["undoKey"] == params["idempotencyKey"] {
            return success_text(
                id,
                &json!({
                "transactionId":t["id"],
                "state":"undone",
                "idempotent":true}
                ),
            );
        }

        let reconciliation = t["state"] == "uncertain" && t["undoKey"] == params["idempotencyKey"];
        if (t["state"] != "applied" && !reconciliation) || !arrangement::truthy(&t["prior"]) {
            return transaction_error(id, "Only an applied or exact-key uncertain groove transaction can be undone");
        }

        let result = async {
            self.begin_undo_recovery(&record, params["idempotencyKey"].as_str().unwrap())?;
            let status = self.require_connected(Some("session.read"))?;
            if json!(status.epoch) != t["epoch"] {
                return Ok(transaction_error(id, "Live connection epoch changed; undo refused"));
            }

            let adapter = self.async_adapter();
            let context = self.transaction_context(params, signal, reads::AUDITION_DEADLINE_MS);
            record.borrow_mut()["undoKey"] = params["idempotencyKey"].clone();
            if reconciliation {
                self.replay_undo_recovery(&record, adapter.as_ref(), &context).await?;
            }

            let set = self.set_identity_view(Some(&context)).await?;
            if !is_non_empty_string(&set["objectIdentity"], 256) {
                return Err(LiveError::error("Set identity is not authoritative"));
            }
            let payload = &t["payload"];
            let action = &payload["action"];

            if !reconciliation {
                let current =
                    adapter.invoke_async(&LiveInvocation::new("groove.read", json!({"setRef":set["ref"]})), Some(&context)).await?;
                if action == "set-amount" {
                    if !same_live_value(field(&current, "grooveAmount")?, payload.get("grooveAmount")) {
                        return Ok(transaction_error(id, "groove amount changed after apply; undo refused"));
                    }
                } else {
                    let Some(groove) = row(&current, &payload["ref"], "current")? else {
                        return Ok(transaction_error(id, "edited groove disappeared after apply"));
                    };
                    for f in FIELDS {
                        if let Some(v) = payload.get(f) {
                            if !same_live_value(groove.get(f), Some(v)) {
                                return Ok(transaction_error(id, "groove changed after apply; undo refused"));
                            }
                        }
                    }
                }
            }
            let before = adapter
                .invoke_async(
                    &LiveInvocation::new(
                        "groove.read",
                        json!({
                        "setRef":set["ref"]}
                        ),
                    ),
                    Some(&context),
                )
                .await?;
            if !field(&before, "revision")?.is_some_and(|v| is_non_empty_string(v, 64)) {
                return Err(LiveError::error("groove undo authority is unavailable"));
            }

            if action != "set-amount" {
                let groove = row(&before, &payload["ref"], "before")?;
                if let Some(moved) = self.undo_target_moved(
                    id,
                    &record.borrow(),
                    "groove",
                    &payload["ref"],
                    groove.as_ref().and_then(|r| r.get("objectIdentity")),
                    payload.get("expectedObjectIdentity"),
                )? {
                    return Ok(moved);
                }
            }

            record.borrow_mut()["state"] = json!("undoing");
            if action == "set-amount" {
                let prior = &t["prior"]["grooveAmount"];
                if !prior.is_number() {
                    return Ok(transaction_error(id, "prior groove amount is unavailable"));
                }
                let result = self
                    .invoke_undo_recovery(
                        &record,
                        adapter.as_ref(),
                        "groove.set",
                        &json!({
                        "setRef":set["ref"],
                        "grooveAmount":prior,
                        "expectedObjectIdentity":set["objectIdentity"],
                        "expectedRevision":before["revision"]}
                        ),
                        &context,
                    )
                    .await?;
                if field(&result, "changed")? != Some(&json!(true)) {
                    return Err(LiveError::error("groove amount restoration was not confirmed"));
                }
            } else {
                let groove = row(&before, &payload["ref"], "before")?
                    .filter(|r| is_non_empty_string(&r["objectIdentity"], 256))
                    .ok_or_else(|| LiveError::error("edited groove identity is unavailable"))?;
                let mut args = json!({
                "ref":payload["ref"]}
                );
                // Only what the edit changed: a field the producer changed since stays as they left it.
                for (k, v) in t["prior"].as_object().unwrap() {
                    if payload.get(k).is_some() {
                        args[k] = v.clone();
                    }
                }
                args["expectedObjectIdentity"] = groove["objectIdentity"].clone();
                args["expectedRevision"] = before["revision"].clone();

                let result = self.invoke_undo_recovery(&record, adapter.as_ref(), "groove.edit", &args, &context).await?;
                if field(&result, "changed")? != Some(&json!(true)) {
                    return Err(LiveError::error("groove restoration was not confirmed"));
                }
            }
            let verified = adapter.invoke_async(&LiveInvocation::new("groove.read", json!({"setRef":set["ref"]})), Some(&context)).await?;
            if action == "set-amount" {
                if !same_live_value(field(&verified, "grooveAmount")?, t["prior"].get("grooveAmount")) {
                    return Err(LiveError::error("groove amount undo did not restore the exact prior value"));
                }
            } else {
                let groove =
                    row(&verified, &payload["ref"], "verified")?.ok_or_else(|| LiveError::error("edited groove disappeared after undo"))?;
                for f in FIELDS.iter().filter(|f| payload.get(**f).is_some()) {
                    if !same_live_value(groove.get(*f), t["prior"].get(*f)) {
                        return Err(LiveError::error("groove undo did not restore the exact prior fields"));
                    }
                }
            }

            record.borrow_mut()["state"] = json!("undone");
            Ok(success_text(id, &json!({"transactionId":t["id"],"state":"undone","idempotent":false})))
        }
        .await;
        result.unwrap_or_else(|e| {
            record.borrow_mut()["state"] = json!("uncertain");
            adapter_tool_error(id, &e, "Groove undo is uncertain; perform fresh discovery.")
        })
    }
}
