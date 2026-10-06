//! Global tuning and scale transactions with exact prior-state restoration.
use super::*;
use kumi_common::{
    abort::{Signal, SignalExt},
    js::json as js_json,
};
const SYSTEM: [&str; 5] = ["name", "lowestNote", "highestNote", "referencePitch", "noteTunings"];
const SCALE: [&str; 3] = ["rootNote", "scaleName", "scaleMode"];
/// A tuning's lowest or highest note as Live takes it (a PitchClassAndOctave): a step of the pseudo-octave and
/// an octave, in that order, or None.
fn pitch_place(value: &Value) -> Option<Value> {
    (value.as_object()?.len() == 2
        && is_integer_in_range(&value["indexInOctave"], 0.0, 1024.0)
        && is_integer_in_range(&value["octave"], -64.0, 64.0))
    .then(|| json!({"indexInOctave":value["indexInOctave"],"octave":value["octave"]}))
}
/// A reference pitch as Live takes it (a ReferencePitch): a frequency in Hz on a step of an octave, or None.
fn reference_pitch(value: &Value) -> Option<Value> {
    (value.as_object()?.len() == 3
        && value["frequency"].as_f64().is_some_and(|hz| hz.is_finite() && hz > 0.0 && hz <= 100_000.0)
        && is_integer_in_range(&value["indexInOctave"], 0.0, 1024.0)
        && is_integer_in_range(&value["octave"], -64.0, 64.0))
    .then(|| json!({"frequency":value["frequency"],"indexInOctave":value["indexInOctave"],"octave":value["octave"]}))
}
fn same_json(a: Option<&Value>, b: Option<&Value>) -> bool {
    a.map(js_json::stringify) == b.map(js_json::stringify)
}
pub(super) fn field<'a>(row: &'a Value, name: &str) -> Result<Option<&'a Value>, LiveError> {
    if row.is_null() {
        return Err(LiveError::type_error(format!("Cannot read properties of null (reading '{name}')")));
    }
    Ok(row.get(name))
}

fn observed<'a>(row: &'a Value, name: &str) -> Result<Option<&'a Value>, LiveError> {
    Ok(field(row, if SYSTEM.contains(&name) { "tuningSystem" } else { "scale" })?.and_then(|v| v.get(name)))
}

fn set(snapshot: &LiveSnapshot) -> Result<Value, LiveError> {
    snapshot
        .set
        .as_ref()
        .map(|s| serde_json::to_value(s).unwrap())
        .ok_or_else(|| LiveError::type_error("Cannot read properties of undefined (reading 'objectIdentity')"))
}

fn result_value(id: &Value, t: &Value, verified: &Value) -> Value {
    let mut v = json!({
    "transactionId":t["id"],
    "state":"applied"}
    );
    if let Some(revision) = verified.get("revision") {
        v["revision"] = revision.clone();
    }
    v["idempotent"] = json!(false);
    success_text(id, &v)
}

impl McpHost {
    pub(super) async fn set_identity_view(&self, context: Option<&LiveOperationContext>) -> Result<Value, LiveError> {
        set(&self.views.view(context, LiveViewScope::Indices(vec![]), Some(&[LiveSnapshotPart::Set])).await?)
    }

    pub async fn dispatch_tuning_tool(&self, call: &ToolCall, signal: Option<&Signal>) -> Option<Result<Option<Value>, LiveError>> {
        let p = call.arguments.as_ref().unwrap_or(&Value::Null);
        Some(Ok(match call.name.as_str() {
            "live_tuning_preview" => Some(self.live_tuning_preview_async(&call.id, p).await),
            "live_tuning_apply" => self.live_tuning_apply_async(&call.id, p, signal).await,
            _ => return None,
        }))
    }

    pub async fn live_tuning_preview_async(&self, id: &Value, params: &Value) -> Value {
        let fields: Vec<_> = SYSTEM.into_iter().chain(SCALE).collect();
        if !has_only(params, &fields) {
            return error(id, -32602, "only bounded tuning and scale fields are accepted", None);
        }
        if fields.iter().all(|f| params.get(*f).is_none()) {
            return error(id, -32602, "at least one tuning field is required", None);
        }
        if params.get("noteTunings").is_some() {
            return error(id, -32602, crate::live::NOTE_TUNINGS_FIXED, None);
        }
        // Live's own shapes, in their order, so what's read back after compares exactly (#206).
        let mut params = params.clone();
        for key in ["lowestNote", "highestNote"] {
            if let Some(value) = params.get(key) {
                let Some(place) = pitch_place(value) else {
                    return error(
                        id,
                        -32602,
                        &format!("{key} is {{indexInOctave, octave}}: a step of the tuning's pseudo-octave (from 0) and an octave"),
                        None,
                    );
                };
                params[key] = place;
            }
        }
        if let Some(value) = params.get("referencePitch") {
            let Some(pitch) = reference_pitch(value) else {
                return error(
                    id,
                    -32602,
                    "referencePitch is {frequency, indexInOctave, octave}: a frequency in Hz on a step of an octave",
                    None,
                );
            };
            params["referencePitch"] = pitch;
        }
        let params = &params;

        let result = async {
            let status = self.fresh_status(Some(&LiveOperationContext::with_deadline(self.deadline(reads::AUDITION_DEADLINE_MS)))).await?;
            if !status.connected || !status.capabilities.iter().any(|c| c.as_str() == "session.read") {
                return Err(LiveError::error("session read capability is unavailable"));
            }

            if !status.has_operation("tuning.read") || !status.has_operation("tuning.set") {
                return Err(LiveError::error("tuning editing is unavailable"));
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
                        "tuning.read",
                        json!({
                        "setRef":set["ref"]}
                        ),
                    ),
                    Some(&context),
                )
                .await?;

            if !field(&read, "revision")?.is_some_and(|v| is_non_empty_string(v, 64)) {
                return Err(LiveError::error("tuning revision is unavailable"));
            }
            let mut proposed = json!({});
            let mut payload = json!({
            "setRef":set["ref"]}
            );
            for f in fields {
                if let Some(v) = params.get(f) {
                    proposed[f] = v.clone();
                    payload[f] = v.clone();
                }
            }
            payload["expectedObjectIdentity"] = set["objectIdentity"].clone();
            payload["expectedRevision"] = read["revision"].clone();

            let mut prior = json!({});
            for f in ["tuningSystem", "scale", "revision"] {
                if let Some(v) = read.get(f) {
                    prior[f] = v.clone();
                }
            }
            let t = json!({
            "id":tempo::transaction_id("tuning"),
            "epoch":status.epoch,
            "kind":"tuning",
            "fence":js_json::stringify(&json!({
            "setRef":set["ref"],
            "identity":set["objectIdentity"],
            "revision":read["revision"]}
            )),
            "payload":payload,
            "prior":prior,
            "expiresAt":kumi_common::time::now_ms_f64()+TRANSACTION_TTL_MS,
            "state":"previewed"}
            );

            self.retain_bounded_transaction(&self.clip_lifecycle_transactions, t.clone(), "tuning")?;
            Ok(success_text(
                id,
                &json!({
                "transactionId":t["id"],
                "epoch":t["epoch"],
                "prior":prior,
                "proposed":proposed,
                "impact":"edits-global-tuning-audible",
                "confirmation":"apply",
                "expiresAt":t["expiresAt"]}
                ),
            ))
        }
        .await;
        result.unwrap_or_else(|e| adapter_tool_error(id, &e, "Tuning preview requires fresh authoritative state."))
    }
    pub async fn live_tuning_apply_async(&self, id: &Value, params: &Value, signal: Option<&Signal>) -> Option<Value> {
        if !valid_transaction_params(params, "apply") {
            return Some(error(id, -32602, "transactionId, confirmation=apply, and idempotencyKey are required", None));
        }

        let Some(record) = self.clip_lifecycle_transactions.get(params["transactionId"].as_str().unwrap()) else {
            return Some(transaction_error(id, "Unknown or expired tuning transaction"));
        };
        let t = record.borrow().clone();

        if t["kind"] != "tuning"
            || (t["state"] == "previewed" && t["expiresAt"].as_f64().is_some_and(|n| n <= kumi_common::time::now_ms_f64()))
        {
            return Some(transaction_error(id, "Unknown or expired tuning transaction"));
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
                            "tuning.read",
                            json!({
                            "setRef":payload["setRef"]}
                            ),
                        ),
                        Some(&context),
                    )
                    .await?;

                let mut fence = json!({
                "setRef":payload["setRef"]}
                );
                if let Some(identity) = set.get("objectIdentity") {
                    fence["identity"] = identity.clone();
                }
                if let Some(revision) = field(&before, "revision")? {
                    fence["revision"] = revision.clone();
                }
                if js_json::stringify(&fence) != t["fence"] {
                    return Ok(transaction_error(id, "tuning or scale state changed since preview; preview again"));
                }
            }
            {
                let mut row = record.borrow_mut();
                row["state"] = json!("applying");
                row["applyKey"] = params["idempotencyKey"].clone();
            }
            let result = adapter.invoke_async(&LiveInvocation::new("tuning.set", payload.clone()), Some(&context)).await?;
            if field(&result, "changed")? != Some(&json!(true)) {
                return Err(LiveError::error("tuning change was not confirmed"));
            }

            let verified =
                adapter.invoke_async(&LiveInvocation::new("tuning.read", json!({"setRef":payload["setRef"]})), Some(&context)).await?;
            for (f, v) in payload.as_object().unwrap() {
                if ["setRef", "expectedObjectIdentity", "expectedRevision"].contains(&f.as_str()) {
                    continue;
                }
                if !same_json(observed(&verified, f)?, Some(v)) {
                    return Err(LiveError::error("tuning postcondition was not confirmed"));
                }
            }

            {
                let mut row = record.borrow_mut();
                row["applyKey"] = params["idempotencyKey"].clone();
                row["state"] = json!("applied");
            }
            Ok(result_value(id, &t, &verified))
        }
        .await;
        Some(result.unwrap_or_else(|e| {
            record.borrow_mut()["state"] = json!("uncertain");
            adapter_tool_error(id, &e, "Tuning state is uncertain; perform fresh discovery before retrying.")
        }))
    }
    pub async fn undo_tuning_async(&self, id: &Value, params: &Value, signal: Option<&Signal>) -> Value {
        let Some(record) = params["transactionId"]
            .as_str()
            .and_then(|id| self.clip_lifecycle_transactions.get(id))
            .filter(|r| r.borrow()["kind"] == "tuning")
        else {
            return transaction_error(id, "Unknown or expired tuning transaction");
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
            return transaction_error(id, "Only an applied or exact-key uncertain tuning transaction can be undone");
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

            let prior = &t["prior"];
            let payload = &t["payload"];
            let set = self.set_identity_view(Some(&context)).await?;
            if !reconciliation {
                let current =
                    adapter.invoke_async(&LiveInvocation::new("tuning.read", json!({"setRef":payload["setRef"]})), Some(&context)).await?;
                for (f, v) in payload.as_object().unwrap() {
                    if ["setRef", "expectedObjectIdentity", "expectedRevision"].contains(&f.as_str()) {
                        continue;
                    }
                    if !same_json(observed(&current, f)?, Some(v)) {
                        return Ok(transaction_error(id, "tuning state changed after apply; undo refused"));
                    }
                }
            }
            let before =
                adapter.invoke_async(&LiveInvocation::new("tuning.read", json!({"setRef":payload["setRef"]})), Some(&context)).await?;
            if !field(&before, "revision")?.is_some_and(|v| is_non_empty_string(v, 64)) || !is_non_empty_string(&set["objectIdentity"], 256)
            {
                return Err(LiveError::error("tuning undo authority is unavailable"));
            }

            let mut restore =
                json!({"setRef":payload["setRef"],"expectedObjectIdentity":set["objectIdentity"],"expectedRevision":before["revision"]});
            // What the apply changed goes back, and nothing else: the rest is as it was, and a tuning's note
            // tunings can't be set at all (#206).
            for (category, fields) in [("tuningSystem", SYSTEM.as_slice()), ("scale", SCALE.as_slice())] {
                for f in fields.iter().filter(|f| payload.get(**f).is_some()) {
                    let row = prior
                        .get(category)
                        .ok_or_else(|| LiveError::type_error(format!("Cannot read properties of undefined (reading '{f}')")))?;
                    if let Some(v) = field(row, f)?.filter(|v| !v.is_null()) {
                        restore[*f] = v.clone();
                    }
                }
            }

            record.borrow_mut()["state"] = json!("undoing");
            let result = self.invoke_undo_recovery(&record, adapter.as_ref(), "tuning.set", &restore, &context).await?;

            if field(&result, "changed")? != Some(&json!(true)) {
                return Err(LiveError::error("tuning restoration was not confirmed"));
            }
            let verified =
                adapter.invoke_async(&LiveInvocation::new("tuning.read", json!({"setRef":payload["setRef"]})), Some(&context)).await?;
            if !same_json(field(&verified, "tuningSystem")?, prior.get("tuningSystem"))
                || !same_json(field(&verified, "scale")?, prior.get("scale"))
            {
                return Err(LiveError::error("tuning undo did not restore the exact prior state"));
            }

            record.borrow_mut()["state"] = json!("undone");
            Ok(success_text(id, &json!({"transactionId":t["id"],"state":"undone","idempotent":false})))
        }
        .await;
        result.unwrap_or_else(|e| {
            record.borrow_mut()["state"] = json!("uncertain");
            adapter_tool_error(id, &e, "Tuning undo is uncertain; perform fresh discovery.")
        })
    }
}
