//! Automation envelope edits, clear authority and recoverable compound undo.
use super::*;
use kumi_common::{
    abort::{Signal, SignalExt},
    js::json as js_json,
};
use sha2::{Digest, Sha256};
const MAX_SET_COLLECTION: usize = 10_000_000;
const ACTIONS: [&str; 6] = ["create-envelope", "delete-envelope", "insert", "insert-step", "delete-range", "clear-envelopes"];
fn hash(value: &Value) -> Result<String, LiveError> {
    Ok(hex::encode(Sha256::digest(canonical_mutation_identity(value)?)))
}
fn operation(action: &Value) -> &'static str {
    match action.as_str() {
        Some("insert-step") => "automation.step.insert",
        Some("insert") => "automation.point.insert",
        Some("delete-range") => "automation.point.delete",
        Some("create-envelope") => "automation.envelope.create",
        _ => "automation.envelope.delete",
    }
}

fn copy_fields(value: &Value, fields: &[&str]) -> Value {
    let mut result = json!({});
    for f in fields {
        if let Some(v) = value.get(*f) {
            result[*f] = v.clone();
        }
    }
    result
}

fn point_array(value: &Value) -> Vec<Value> {
    value.as_array().cloned().unwrap_or_default()
}
fn nullish_array(value: Option<&Value>) -> Value {
    value.filter(|v| !v.is_null()).cloned().unwrap_or(json!([]))
}
fn inserted_range(payload: &Value) -> (f64, f64) {
    let times: Vec<f64> = if payload["action"] == "insert-step" {
        let start = payload["start"].as_f64().unwrap();
        vec![start, start + payload["length"].as_f64().unwrap()]
    } else {
        payload["points"].as_array().unwrap().iter().map(|p| p["time"].as_f64().unwrap()).collect()
    };
    (
        0.0_f64.max(times.iter().copied().fold(f64::INFINITY, f64::min) - 0.001),
        times.iter().copied().fold(f64::NEG_INFINITY, f64::max) + 0.001,
    )
}

fn exact_content(a: &Value, b: &Value) -> Result<bool, LiveError> {
    Ok(clip_properties::scalar_same(tuning::field(a, "exists")?, tuning::field(b, "exists")?)
        && canonical_mutation_identity(&nullish_array(tuning::field(a, "points")?))?
            == canonical_mutation_identity(&nullish_array(tuning::field(b, "points")?))?)
}

impl McpHost {
    /// Whether the clip has an envelope for each parameter its track's clear would clear, as Live reports it: the
    /// digest a clear fences on, and how many there are. Live walks every drum pad's chains, which a snapshot's rows
    /// don't all list, and a snapshot doesn't say which parameters have envelopes.
    pub(super) async fn envelope_presence(
        &self,
        context: Option<&LiveOperationContext>,
        snapshot: &LiveSnapshot,
        reference: &str,
    ) -> Result<Value, LiveError> {
        let located = self.clip_row(snapshot, reference)?;
        let track =
            located.track.filter(|_| !located.arrangement).ok_or_else(|| LiveError::error("envelope clear requires a Session clip"))?;
        let slot = track["clipSlots"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|slot| slot["clipRef"] == reference)
            .and_then(|slot| slot["ref"].as_str());
        let row = self
            .discover_one_async(context, LiveDiscoveryKind::SessionClip, reference, Some(&["envelopesRevision", "envelopesPresent"]), slot)
            .await?
            .ok_or_else(|| LiveError::error("envelope clear requires a Session clip"))?;
        if !is_non_empty_string(&row["envelopesRevision"], 64) || !row["envelopesPresent"].is_u64() {
            return Err(LiveError::error("envelope presence is unavailable"));
        }
        Ok(json!({"revision":row["envelopesRevision"],"cleared":row["envelopesPresent"]}))
    }

    pub(super) fn automation_authority_digest(
        &self,
        snapshot: &LiveSnapshot,
        clip_ref: &str,
        parameter_ref: &str,
    ) -> Result<String, LiveError> {
        hash(&json!({
        "clip":self.clip_authority(snapshot,
        clip_ref)?,
        "parameter":self.parameter_authority(snapshot,
        parameter_ref)?}
        ))
    }

    async fn automation_view(&self, context: Option<&LiveOperationContext>, payload: &Value) -> Result<LiveSnapshot, LiveError> {
        self.views.view_for(context, &[payload["clipRef"].clone(), payload["parameterRef"].clone()], None, &[]).await
    }

    async fn automation_read(
        &self,
        adapter: &dyn AsyncLiveAdapter,
        context: &LiveOperationContext,
        payload: &Value,
    ) -> Result<Value, LiveError> {
        adapter
            .invoke_async(
                &LiveInvocation::new("automation.envelope.read", copy_fields(payload, &["clipRef", "parameterRef"])),
                Some(context),
            )
            .await
    }

    fn automation_fence(&self, payload: &Value, read: &Value, points: Value, authority: &Value) -> Result<String, LiveError> {
        let mut fence = copy_fields(payload, &["clipRef", "parameterRef"]);
        if let Some(exists) = tuning::field(read, "exists")? {
            fence["exists"] = exists.clone();
        }
        fence["points"] = points;
        if let Some(revision) = tuning::field(read, "revision")? {
            fence["revision"] = revision.clone();
        }
        fence["authorityDigest"] = authority.clone();
        Ok(js_json::stringify(&fence))
    }

    pub async fn dispatch_automation_tool(&self, call: &ToolCall, signal: Option<&Signal>) -> Option<Result<Option<Value>, LiveError>> {
        let p = call.arguments.as_ref().unwrap_or(&Value::Null);
        Some(Ok(match call.name.as_str() {
            "live_automation_preview" => Some(self.live_automation_preview_async(&call.id, p).await),
            "live_automation_apply" => self.live_automation_apply_async(&call.id, p, signal).await,
            _ => return None,
        }))
    }

    pub async fn live_automation_preview_async(&self, id: &Value, params: &Value) -> Value {
        if !has_only(params, &["action", "clipRef", "parameterRef", "points", "from", "to", "start", "length", "value"])
            || !ACTIONS.iter().any(|a| params["action"] == *a)
            || !is_non_empty_string(&params["clipRef"], 256)
        {
            return error(id, -32602, "action and clipRef are required", None);
        }
        let clear = params["action"] == "clear-envelopes";
        if if clear { params.get("parameterRef").is_some() } else { !is_non_empty_string(&params["parameterRef"], 256) } {
            return error(id, -32602, if clear { "clear-envelopes takes no parameterRef" } else { "parameterRef is required" }, None);
        }
        let result = async {
            let status = self.fresh_status(Some(&LiveOperationContext::with_deadline(self.deadline(reads::AUDITION_DEADLINE_MS)))).await?;
            if !status.connected || !status.capabilities.iter().any(|c| c.as_str() == "session.read") {
                return Err(LiveError::error("session read capability is unavailable"));
            }
            let adapter = self.async_adapter();
            let clip_ref = params["clipRef"].as_str().unwrap();
            if clear {
                if !status.has_operation("automation.envelope.clear") {
                    return Err(LiveError::error("automation.envelope.clear is unavailable"));
                }
                let snapshot = self.views.view_for(None, &[params["clipRef"].clone()], None, &[]).await?;
                let authority = self.clip_authority_digest(&snapshot, clip_ref)?;
                let presence = self.envelope_presence(None, &snapshot, clip_ref).await?;
                let t = json!({
                "id":tempo::transaction_id("automation"),
                "epoch":status.epoch,
                "kind":"automation",
                "fence":js_json::stringify(&json!({
                "clipRef":params["clipRef"],
                "presence":presence["revision"],
                "authorityDigest":authority}
                )),
                "clipRef":params["clipRef"],
                "payload":{
                "action":params["action"],
                "clipRef":params["clipRef"],
                "expectedAuthorityDigest":authority,
                "expectedEnvelopesRevision":presence["revision"]}
                ,
                "prior":{
                "cleared":presence["cleared"],
                "reversible":false}
                ,
                "expiresAt":kumi_common::time::now_ms_f64()+TRANSACTION_TTL_MS,
                "state":"previewed"}
                );
                self.retain_bounded_transaction(&self.clip_lifecycle_transactions, t.clone(), "automation")?;
                return Ok(success_text(
                    id,
                    &json!({
                    "transactionId":t["id"],
                    "epoch":t["epoch"],
                    "action":params["action"],
                    "clipRef":params["clipRef"],
                    "envelopes":presence["cleared"],
                    "impact":"clears-all-clip-envelopes-not-undoable",
                    "confirmation":"apply",
                    "expiresAt":t["expiresAt"]}
                    ),
                ));
            }
            let op = operation(&params["action"]);
            if !status.has_operation(op) {
                return Err(LiveError::error(format!("{op} is unavailable")));
            }
            let parameter_ref = params["parameterRef"].as_str().unwrap();
            let context = LiveOperationContext::with_deadline(self.deadline(reads::AUDITION_DEADLINE_MS));
            let before = self.automation_view(Some(&context), params).await?;
            let authority = self.automation_authority_digest(&before, clip_ref, parameter_ref)?;
            let read = self.automation_read(adapter.as_ref(), &context, params).await?;
            if tuning::field(&read, "available")? != Some(&json!(true)) || !is_non_empty_string(&read["revision"], 64) {
                return Err(LiveError::error("clip envelope revision is unavailable"));
            }
            if self.automation_authority_digest(&self.automation_view(Some(&context), params).await?, clip_ref, parameter_ref)? != authority
            {
                return Err(LiveError::error("automation target identity changed during preview"));
            }
            let points = point_array(&read["points"]);
            let fence = self.automation_fence(params, &read, json!(points), &json!(authority))?;
            let mut payload = json!({
            "action":params["action"],
            "clipRef":params["clipRef"],
            "parameterRef":params["parameterRef"],
            "expectedAuthorityDigest":authority,
            "expectedEnvelopeRevision":read["revision"]}
            );
            let action = &params["action"];
            if action == "insert" {
                let Some(points) = params["points"].as_array().filter(|a| !a.is_empty() && a.len() <= MAX_SET_COLLECTION) else {
                    return Ok(error(id, -32602, "points must be one or more point objects", None));
                };
                if points.iter().any(|point| {
                    !has_only(point, &["time", "value"])
                        || !point["time"].as_f64().is_some_and(|n| n.is_finite() && n >= 0.0)
                        || !point["value"].as_f64().is_some_and(f64::is_finite)
                }) {
                    return Ok(error(id, -32602, "points are invalid", None));
                }
                payload["points"] = params["points"].clone();
            }
            if action == "delete-range" {
                if !params["from"].as_f64().is_some_and(|n| n.is_finite() && n >= 0.0)
                    || !params["to"].as_f64().is_some_and(|n| n.is_finite() && n > params["from"].as_f64().unwrap_or(f64::NAN))
                {
                    return Ok(error(id, -32602, "from/to are invalid", None));
                }
                payload["from"] = params["from"].clone();
                payload["to"] = params["to"].clone();
            }
            if action == "insert-step" {
                if !params["start"].as_f64().is_some_and(|n| n.is_finite() && n >= 0.0)
                    || !params["length"].as_f64().is_some_and(|n| n.is_finite() && n >= 0.001)
                    || !params["value"].as_f64().is_some_and(f64::is_finite)
                {
                    return Ok(error(id, -32602, "insert-step takes start (0 or more), length (more than 0) and value", None));
                }
                for f in ["start", "length", "value"] {
                    payload[f] = params[f].clone();
                }
            }
            if action == "delete-envelope" && read["exists"] != true {
                return Ok(transaction_error(id, "envelope does not exist"));
            }
            let mut prior = copy_fields(&read, &["exists"]);
            prior["points"] = json!(points);
            let t = json!({
            "id":tempo::transaction_id("automation"),
            "epoch":status.epoch,
            "kind":"automation",
            "fence":fence,
            "clipRef":params["clipRef"],
            "payload":payload,
            "prior":prior,
            "expiresAt":kumi_common::time::now_ms_f64()+TRANSACTION_TTL_MS,
            "state":"previewed"}
            );
            self.retain_bounded_transaction(&self.clip_lifecycle_transactions, t.clone(), "automation")?;
            Ok(success_text(
                id,
                &json!({
                "transactionId":t["id"],
                "epoch":t["epoch"],
                "action":params["action"],
                "clipRef":params["clipRef"],
                "parameterRef":params["parameterRef"],
                "current":prior,
                "impact":"edits-clip-automation",
                "confirmation":"apply",
                "expiresAt":t["expiresAt"]}
                ),
            ))
        }
        .await;
        result.unwrap_or_else(|e| adapter_tool_error(id, &e, "Automation preview requires fresh authoritative state."))
    }

    pub async fn live_automation_apply_async(&self, id: &Value, params: &Value, signal: Option<&Signal>) -> Option<Value> {
        if !valid_transaction_params(params, "apply") {
            return Some(error(id, -32602, "transactionId, confirmation=apply, and idempotencyKey are required", None));
        }
        let Some(record) = self.clip_lifecycle_transactions.get(params["transactionId"].as_str().unwrap()) else {
            return Some(transaction_error(id, "Unknown or expired automation transaction"));
        };
        let t = record.borrow().clone();
        if t["kind"] != "automation"
            || (t["state"] == "previewed" && t["expiresAt"].as_f64().is_some_and(|n| n <= kumi_common::time::now_ms_f64()))
        {
            return Some(transaction_error(id, "Unknown or expired automation transaction"));
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
            let action = &payload["action"];
            let clip_ref = payload["clipRef"].as_str().unwrap();
            if action == "clear-envelopes" {
                if !reconciliation {
                    let snapshot = self.views.view_for(Some(&context), &[payload["clipRef"].clone()], None, &[]).await?;
                    let authority = self.clip_authority_digest(&snapshot, clip_ref)?;
                    let presence = self.envelope_presence(Some(&context), &snapshot, clip_ref).await?;
                    if js_json::stringify(&json!({
                    "clipRef":payload["clipRef"],
                    "presence":presence["revision"],
                    "authorityDigest":authority}
                    )) != t["fence"]
                    {
                        return Ok(transaction_error(id, "envelope collection or clip hierarchy changed since preview; preview again"));
                    }
                }
                {
                    let mut r = record.borrow_mut();
                    r["state"] = json!("applying");
                    r["applyKey"] = params["idempotencyKey"].clone();
                }
                let cleared = adapter
                    .invoke_async(
                        &LiveInvocation::new(
                            "automation.envelope.clear",
                            copy_fields(payload, &["clipRef", "expectedAuthorityDigest", "expectedEnvelopesRevision"]),
                        ),
                        Some(&context),
                    )
                    .await?;
                let snapshot = self.views.view_for(Some(&context), &[payload["clipRef"].clone()], None, &[]).await?;
                let after = self.envelope_presence(Some(&context), &snapshot, clip_ref).await?;
                if !clip_properties::scalar_same(tuning::field(&cleared, "cleared")?, t["prior"].get("cleared"))
                    || after["cleared"].as_f64() != Some(0.0)
                    || cleared["envelopesRevision"] != after["revision"]
                {
                    return Err(LiveError::error("envelope clear postcondition was not confirmed"));
                }
                {
                    let mut r = record.borrow_mut();
                    r["applyKey"] = params["idempotencyKey"].clone();
                    r["state"] = json!("applied");
                }
                return Ok(success_text(
                    id,
                    &json!({
                    "transactionId":t["id"],
                    "state":"applied",
                    "cleared":cleared["cleared"],
                    "idempotent":false}
                    ),
                ));
            }
            let parameter_ref = payload["parameterRef"].as_str().unwrap();
            let mut read = json!({
            "revision":payload["expectedEnvelopeRevision"]}
            );
            let mut authority = payload["expectedAuthorityDigest"].clone();
            if !reconciliation {
                read = self.automation_read(adapter.as_ref(), &context, payload).await?;
                authority = json!(self.automation_authority_digest(
                    &self.automation_view(Some(&context), payload).await?,
                    clip_ref,
                    parameter_ref
                )?);
                if self.automation_fence(payload, &read, nullish_array(tuning::field(&read, "points")?), &authority)? != t["fence"] {
                    return Ok(transaction_error(id, "envelope or target identity changed since preview; preview again"));
                }
            }
            let mut args = copy_fields(payload, &["clipRef", "parameterRef", "expectedAuthorityDigest", "expectedEnvelopeRevision"]);
            for f in if action == "insert" {
                &["points"][..]
            } else if action == "delete-range" {
                &["from", "to"][..]
            } else if action == "insert-step" {
                &["start", "length", "value"][..]
            } else {
                &[][..]
            } {
                args[*f] = payload[*f].clone();
            }
            {
                let mut r = record.borrow_mut();
                r["state"] = json!("applying");
                r["applyKey"] = params["idempotencyKey"].clone();
            }
            let result = adapter.invoke_async(&LiveInvocation::new(operation(action), args), Some(&context)).await?;
            let after = self.automation_read(adapter.as_ref(), &context, payload).await?;
            if !tuning::field(&after, "revision")?.is_some_and(|v| is_non_empty_string(v, 64))
                || (after["revision"] == read["revision"]
                    && !(action == "insert-step" && tuning::field(&result, "inserted")?.and_then(Value::as_f64) == Some(0.0)))
            {
                return Err(LiveError::error("automation mutation did not change the exact envelope revision"));
            }
            let mut created = copy_fields(&after, &["exists", "points", "revision"]);
            created["authorityDigest"] = authority;
            record.borrow_mut()["created"] = created;
            record.borrow_mut()["state"] = json!("applied");
            Ok(success_text(
                id,
                &json!({
                "transactionId":t["id"],
                "state":"applied",
                "result":result,
                "idempotent":false}
                ),
            ))
        }
        .await;
        Some(result.unwrap_or_else(|e| {
            // Nothing was sent before applying: it stays as it was, to try again.
            if record.borrow()["state"] == "applying" {
                record.borrow_mut()["state"] = json!("uncertain");
            }
            adapter_tool_error(id, &e, "Automation state is uncertain; perform fresh discovery before retrying.")
        }))
    }

    async fn automation_guarded(
        &self,
        adapter: &dyn AsyncLiveAdapter,
        context: &LiveOperationContext,
        payload: &Value,
        extra: Value,
    ) -> Result<Value, LiveError> {
        let read = self.automation_read(adapter, context, payload).await?;
        let authority = self.automation_authority_digest(
            &self.automation_view(Some(context), payload).await?,
            payload["clipRef"].as_str().unwrap(),
            payload["parameterRef"].as_str().unwrap(),
        )?;
        if !tuning::field(&read, "revision")?.is_some_and(|v| is_non_empty_string(v, 64)) {
            return Err(LiveError::error("automation undo revision is unavailable"));
        }
        let mut args = copy_fields(payload, &["clipRef", "parameterRef"]);
        args["expectedAuthorityDigest"] = json!(authority);
        args["expectedEnvelopeRevision"] = read["revision"].clone();
        args.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
        Ok(args)
    }

    pub async fn undo_automation_async(&self, id: &Value, params: &Value, signal: Option<&Signal>) -> Value {
        let Some(record) = params["transactionId"]
            .as_str()
            .and_then(|id| self.clip_lifecycle_transactions.get(id))
            .filter(|r| r.borrow()["kind"] == "automation")
        else {
            return transaction_error(id, "Unknown or expired automation transaction");
        };
        let t = record.borrow().clone();
        if t["payload"]["action"] == "clear-envelopes" {
            return transaction_error(id, "Cleared envelopes cannot be reconstructed; undo is unavailable for this transaction");
        }
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
        if t["state"] != "applied" && !reconciliation {
            return transaction_error(id, "Only an applied or exact-key uncertain automation transaction can be undone");
        }
        let result = async {
            let (_, steps) = self.begin_undo_recovery(&record, params["idempotencyKey"].as_str().unwrap())?;
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
            let payload = &t["payload"];
            let action = &payload["action"];
            let prior = &t["prior"];
            let current = self.automation_read(adapter.as_ref(), &context, payload).await?;
            if reconciliation && exact_content(&current, prior)? {
                record.borrow_mut()["state"] = json!("undone");
                return Ok(success_text(
                    id,
                    &json!({
                    "transactionId":t["id"],
                    "state":"undone",
                    "idempotent":false}
                    ),
                ));
            }
            let authority = self.automation_authority_digest(
                &self.automation_view(Some(&context), payload).await?,
                payload["clipRef"].as_str().unwrap(),
                payload["parameterRef"].as_str().unwrap(),
            )?;
            if !reconciliation
                && (!arrangement::truthy(&t["created"])
                    || !clip_properties::scalar_same(tuning::field(&current, "revision")?, t["created"].get("revision"))
                    || t["created"]["authorityDigest"] != authority)
            {
                return Ok(transaction_error(id, "automation target changed after apply; undo refused"));
            }
            if reconciliation && !steps.is_empty() {
                let prior_points = point_array(&prior["points"]);
                if action == "insert" || (action == "insert-step" && prior["exists"] == true) {
                    let (from, to) = inserted_range(payload);
                    let intermediate: Vec<_> = prior_points
                        .into_iter()
                        .filter(|p| !p.is_object() || p["time"].as_f64().is_none_or(|n| n < from || n >= to))
                        .collect();
                    if !clip_properties::scalar_same(tuning::field(&current, "exists")?, prior.get("exists"))
                        || canonical_mutation_identity(&nullish_array(tuning::field(&current, "points")?))?
                            != canonical_mutation_identity(&json!(intermediate))?
                    {
                        return Err(LiveError::error("automation undo partial state conflicts with exact prior content"));
                    }
                } else if action == "insert-step" {
                    if !clip_properties::scalar_same(tuning::field(&current, "exists")?, prior.get("exists")) {
                        return Err(LiveError::error("automation envelope removal has conflicting content"));
                    }
                } else if action == "delete-envelope" {
                    if tuning::field(&current, "exists")? != Some(&json!(true))
                        || canonical_mutation_identity(&nullish_array(tuning::field(&current, "points")?))? != "[]"
                    {
                        return Err(LiveError::error("automation envelope recreation has conflicting content"));
                    }
                } else {
                    return Err(LiveError::error("automation undo replay did not restore exact prior content"));
                }
            }
            let has = |op: &str| steps.iter().any(|step| step.borrow()["operation"] == op);

            if action == "insert-step" && prior["exists"] != true {
                if !has("automation.envelope.delete") {
                    let args = self.automation_guarded(adapter.as_ref(), &context, payload, json!({})).await?;
                    self.invoke_undo_recovery(&record, adapter.as_ref(), "automation.envelope.delete", &args, &context).await?;
                }
            } else if action == "insert" || action == "insert-step" {
                let (from, to) = inserted_range(payload);
                if !has("automation.point.delete") {
                    let args = self
                        .automation_guarded(
                            adapter.as_ref(),
                            &context,
                            payload,
                            json!({
                            "from":from,
                            "to":to}
                            ),
                        )
                        .await?;
                    self.invoke_undo_recovery(&record, adapter.as_ref(), "automation.point.delete", &args, &context).await?;
                }
                let restore: Vec<_> = point_array(&prior["points"])
                    .into_iter()
                    .filter(|p| p.is_object() && p["time"].as_f64().is_some_and(|n| n >= from && n < to))
                    .collect();
                if !restore.is_empty() && !has("automation.point.insert") {
                    let args = self
                        .automation_guarded(
                            adapter.as_ref(),
                            &context,
                            payload,
                            json!({
                            "points":restore}
                            ),
                        )
                        .await?;
                    self.invoke_undo_recovery(&record, adapter.as_ref(), "automation.point.insert", &args, &context).await?;
                }
            } else if action == "delete-range" || action == "delete-envelope" {
                let points = point_array(&prior["points"]);
                if action == "delete-envelope" && prior["exists"] == true && !has("automation.envelope.create") {
                    let args = self.automation_guarded(adapter.as_ref(), &context, payload, json!({})).await?;
                    self.invoke_undo_recovery(&record, adapter.as_ref(), "automation.envelope.create", &args, &context).await?;
                }
                let restore = if action == "delete-range" {
                    let mut restore = vec![];
                    for point in points {
                        let time = tuning::field(&point, "time")?.and_then(Value::as_f64).unwrap_or(f64::NAN);
                        // What Live's delete_events_in_range took: from up to, not including, to.
                        if time >= payload["from"].as_f64().unwrap() && time < payload["to"].as_f64().unwrap() {
                            restore.push(point);
                        }
                    }
                    restore
                } else {
                    points
                };
                if !restore.is_empty() && !has("automation.point.insert") {
                    let args = self
                        .automation_guarded(
                            adapter.as_ref(),
                            &context,
                            payload,
                            json!({
                            "points":restore}
                            ),
                        )
                        .await?;
                    self.invoke_undo_recovery(&record, adapter.as_ref(), "automation.point.insert", &args, &context).await?;
                }
            } else if action == "create-envelope" && !has("automation.envelope.delete") {
                let args = self.automation_guarded(adapter.as_ref(), &context, payload, json!({})).await?;
                self.invoke_undo_recovery(&record, adapter.as_ref(), "automation.envelope.delete", &args, &context).await?;
            }

            let restored = self.automation_read(adapter.as_ref(), &context, payload).await?;
            if !exact_content(&restored, prior)? {
                return Err(LiveError::error("automation undo did not restore the exact prior envelope"));
            }
            {
                let mut r = record.borrow_mut();
                r["state"] = json!("undone");
                r["undoKey"] = params["idempotencyKey"].clone();
            }
            Ok(success_text(
                id,
                &json!({
                "transactionId":t["id"],
                "state":"undone",
                "idempotent":false}
                ),
            ))
        }
        .await;
        result.unwrap_or_else(|e| {
            record.borrow_mut()["state"] = json!("uncertain");
            adapter_tool_error(id, &e, "Automation undo is uncertain; perform fresh discovery.")
        })
    }
}
