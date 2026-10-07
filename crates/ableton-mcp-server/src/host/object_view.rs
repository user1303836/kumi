//! Clip and device views preserve their different state and undo authority.
use super::{
    clip_properties::scalar_same,
    reads::AUDITION_DEADLINE_MS,
    track_view::{confirmed, digest},
    *,
};
use kumi_common::{abort::Signal, js::json as js_json, time::now_ms_f64};
#[derive(Clone, Copy)]
enum Kind {
    Clip,
    Device,
}
impl Kind {
    fn noun(self) -> &'static str {
        match self {
            Self::Clip => "clip",
            Self::Device => "device",
        }
    }
    fn title(self) -> &'static str {
        match self {
            Self::Clip => "Clip",
            Self::Device => "Device",
        }
    }
    fn kind(self) -> &'static str {
        match self {
            Self::Clip => "clip-view",
            Self::Device => "device-view",
        }
    }
    fn operation(self) -> &'static str {
        match self {
            Self::Clip => "clip.view.set",
            Self::Device => "device.view.set",
        }
    }
    fn fields(self) -> &'static [&'static str] {
        match self {
            Self::Clip => &["gridQuantization", "gridIsTriplet"],
            Self::Device => &["collapsed"],
        }
    }
    fn state(self, row: &Value) -> Value {
        match self {
            Self::Clip => json!({"gridQuantization":row["clipView"]["gridQuantization"],"gridIsTriplet":row["clipView"]["gridIsTriplet"]}),
            Self::Device => json!({"collapsed":row["view"]["isCollapsed"]}),
        }
    }
    fn observed<'a>(self, row: &'a Value, field: &str) -> Option<&'a Value> {
        match self {
            Self::Clip => row["clipView"].get(field),
            Self::Device => row["view"].get("isCollapsed"),
        }
    }
    fn fence(self, reference: &Value, row: &Value) -> String {
        let mut out = json!({"ref":reference});
        put(&mut out, "objectIdentity", row.get("objectIdentity"));
        match self {
            Self::Clip => out["viewState"] = self.state(row),
            Self::Device => out["collapsed"] = self.state(row)["collapsed"].clone(),
        };
        js_json::stringify(&out)
    }
}
fn put(out: &mut Value, key: &str, value: Option<&Value>) {
    if let Some(value) = value {
        out[key] = value.clone();
    }
}
impl McpHost {
    fn object_view_row(&self, kind: Kind, snapshot: &LiveSnapshot, reference: &Value) -> Result<Value, LiveError> {
        match kind {
            Kind::Clip => Ok(self.clip_row(snapshot, reference.as_str().unwrap_or(""))?.clip),
            Kind::Device => Ok(self.device_row(snapshot, reference.as_str().unwrap_or(""))?.device),
        }
    }
    pub async fn dispatch_object_view_tool(&self, call: &ToolCall, signal: Option<&Signal>) -> Option<Result<Option<Value>, LiveError>> {
        let p = call.arguments.as_ref().unwrap_or(&Value::Null);
        Some(Ok(match call.name.as_str() {
            "live_clip_view_preview" => Some(self.live_clip_view_preview_async(&call.id, p).await),
            "live_clip_view_apply" => self.live_clip_view_apply_async(&call.id, p, signal).await,
            "live_device_view_preview" => Some(self.live_device_view_preview_async(&call.id, p).await),
            "live_device_view_apply" => self.live_device_view_apply_async(&call.id, p, signal).await,
            _ => return None,
        }))
    }
    pub async fn live_clip_view_preview_async(&self, id: &Value, p: &Value) -> Value {
        let fields = ["gridQuantization", "gridIsTriplet", "showEnvelope"];
        if !has_only(p, &["clipRef", "showLoop", "gridQuantization", "gridIsTriplet", "showEnvelope"])
            || !is_non_empty_string(&p["clipRef"], 256)
        {
            return error(id, -32602, "clipRef is required", None);
        }
        if fields.iter().all(|f| p.get(*f).is_none()) && p["showLoop"] != true {
            return error(id, -32602, "at least one clip view field or showLoop is required", None);
        }
        let result = async {
            let status = self.fresh_status(Some(&LiveOperationContext::with_deadline(self.deadline(AUDITION_DEADLINE_MS)))).await?;
            if !status.connected || !status.capabilities.iter().any(|c| c.as_str() == "session.read") {
                return Err(LiveError::error("session read capability is unavailable"));
            }
            if !status.has_operation("clip.view.set") {
                return Err(LiveError::error("clip view editing is unavailable"));
            }
            let snapshot = self.views.view_for(None, &[p["clipRef"].clone()], None, &[]).await?;
            let row = self.clip_row(&snapshot, p["clipRef"].as_str().unwrap())?.clip;
            let state = Kind::Clip.state(&row);
            let mut proposed = json!({});
            for field in fields {
                if let Some(v) = p.get(field) {
                    if field == "gridQuantization" {
                        if !is_integer_in_range(v, 0.0, 16.0) {
                            return Ok(error(id, -32602, "gridQuantization is invalid", None));
                        }
                    } else if !v.is_boolean() {
                        return Ok(error(id, -32602, &format!("{field} must be boolean"), None));
                    }
                    proposed[field] = v.clone();
                }
            }
            let mut payload = json!({"ref":p["clipRef"]});
            payload.as_object_mut().unwrap().extend(proposed.as_object().unwrap().clone());
            payload["showLoop"] = json!(p["showLoop"]==true);
            put(&mut payload, "expectedObjectIdentity", row.get("objectIdentity"));
            payload["expectedStateRevision"] = json!(digest(&state)?);
            let t = json!({"id":tempo::transaction_id("clipview"),"epoch":status.epoch,"kind":"clip-view","fence":Kind::Clip.fence(&p["clipRef"],&row),"clipRef":p["clipRef"],"payload":payload,"prior":state,"expiresAt":now_ms_f64()+TRANSACTION_TTL_MS,"state":"previewed"});
            self.retain_bounded_transaction(&self.clip_lifecycle_transactions, t.clone(), "clip view")?;
            Ok(success_text(id, &json!({"transactionId":t["id"],"epoch":t["epoch"],"clipRef":p["clipRef"],"prior":state,"proposed":proposed,"showLoop":p["showLoop"]==true,"impact":"edits-clip-view","confirmation":"apply","expiresAt":t["expiresAt"]})))
        }
        .await;
        result.unwrap_or_else(|e| adapter_tool_error(id, &e, "Clip-view preview requires fresh authoritative state."))
    }
    pub async fn live_device_view_preview_async(&self, id: &Value, p: &Value) -> Value {
        if !has_only(p, &["ref", "collapsed"]) || !is_non_empty_string(&p["ref"], 256) || !p["collapsed"].is_boolean() {
            return error(id, -32602, "ref and collapsed are required", None);
        }
        let result = async {
            let status = self.fresh_status(Some(&LiveOperationContext::with_deadline(self.deadline(AUDITION_DEADLINE_MS)))).await?;
            if !status.connected || !status.capabilities.iter().any(|c| c.as_str() == "session.read") {
                return Err(LiveError::error("session read capability is unavailable"));
            }
            if !status.has_operation("device.view.set") {
                return Err(LiveError::error("device view editing is unavailable on this Live shape"));
            }
            let snapshot = self.views.view_for(None, &[p["ref"].clone()], None, &[]).await?;
            let row = self.device_row(&snapshot, p["ref"].as_str().unwrap())?.device;
            let state = Kind::Device.state(&row);
            if state["collapsed"].is_null() {
                return Ok(transaction_error(id, "device collapsed state is unavailable on this exact device"));
            }
            let mut payload = json!({"ref":p["ref"],"collapsed":p["collapsed"]});
            put(&mut payload, "expectedObjectIdentity", row.get("objectIdentity"));
            payload["expectedStateRevision"] = json!(digest(&state)?);
            let t = json!({"id":tempo::transaction_id("devview"),"epoch":status.epoch,"kind":"device-view","fence":Kind::Device.fence(&p["ref"],&row),"payload":payload,"prior":state,"expiresAt":now_ms_f64()+TRANSACTION_TTL_MS,"state":"previewed"});
            self.retain_bounded_transaction(&self.clip_lifecycle_transactions, t.clone(), "device view")?;
            Ok(success_text(id, &json!({"transactionId":t["id"],"epoch":t["epoch"],"ref":p["ref"],"prior":state,"proposed":{"collapsed":p["collapsed"]},"impact":"edits-device-view","confirmation":"apply","expiresAt":t["expiresAt"]})))
        }
        .await;
        result.unwrap_or_else(|e| adapter_tool_error(id, &e, "Device-view preview requires fresh authoritative state."))
    }
    pub async fn live_clip_view_apply_async(&self, id: &Value, p: &Value, signal: Option<&Signal>) -> Option<Value> {
        self.object_view_apply(Kind::Clip, id, p, signal).await
    }
    pub async fn live_device_view_apply_async(&self, id: &Value, p: &Value, signal: Option<&Signal>) -> Option<Value> {
        self.object_view_apply(Kind::Device, id, p, signal).await
    }
    async fn object_view_apply(&self, kind: Kind, id: &Value, p: &Value, signal: Option<&Signal>) -> Option<Value> {
        if !valid_transaction_params(p, "apply") {
            return Some(error(id, -32602, "transactionId, confirmation=apply, and idempotencyKey are required", None));
        }
        let unknown = format!("Unknown or expired {} transaction", kind.kind());
        let Some(record) = self.clip_lifecycle_transactions.get(p["transactionId"].as_str().unwrap()) else {
            return Some(transaction_error(id, &unknown));
        };
        let t = record.borrow().clone();
        if t["kind"] != kind.kind() || (t["state"] == "previewed" && t["expiresAt"].as_f64().unwrap_or(f64::NAN) <= now_ms_f64()) {
            return Some(transaction_error(id, &unknown));
        }
        if t["state"] == "applied" && t["applyKey"] == p["idempotencyKey"] {
            return Some(success_text(id, &json!({"transactionId":t["id"],"state":"applied","idempotent":true})));
        }
        let reconcile = t["state"] == "uncertain" && t["applyKey"] == p["idempotencyKey"];
        if t["state"] != "previewed" && !reconcile {
            return Some(transaction_error(id, "Transaction is no longer applicable"));
        }
        if signal.is_some_and(Signal::is_cancelled) {
            return None;
        }
        let reference = match kind {
            Kind::Clip => &t["clipRef"],
            Kind::Device => &t["payload"]["ref"],
        };
        let result = async {
            if reconcile {
                self.fresh_status(Some(&LiveOperationContext::with_deadline(self.deadline(AUDITION_DEADLINE_MS)))).await?;
            }
            let status = self.require_connected(Some("session.read"))?;
            if json!(status.epoch) != t["epoch"] {
                return Ok(transaction_error(id, "Live connection epoch changed; preview again"));
            }
            let adapter = self.async_adapter();
            let context = self.transaction_context(p, signal, AUDITION_DEADLINE_MS);
            if !reconcile {
                let snapshot = self.views.view_for(Some(&context), &[reference.clone()], None, &[]).await?;
                let row = self.object_view_row(kind, &snapshot, reference)?;
                if json!(kind.fence(reference, &row)) != t["fence"] {
                    return Ok(transaction_error(
                        id,
                        &format!("{} identity or view state changed since preview; preview again", kind.noun()),
                    ));
                }
            }
            record.borrow_mut()["state"] = json!("applying");
            record.borrow_mut()["applyKey"] = p["idempotencyKey"].clone();
            let result = adapter.invoke_async(&LiveInvocation::new(kind.operation(), t["payload"].clone()), Some(&context)).await?;
            confirmed(&result, "changed", &format!("{} view change was not confirmed", kind.noun()))?;
            let snapshot = self.views.view_for(Some(&context), &[reference.clone()], None, &[]).await?;
            let verified = self.object_view_row(kind, &snapshot, reference)?;
            for field in kind.fields() {
                if t["payload"].get(*field).is_some() && !scalar_same(kind.observed(&verified, field), t["payload"].get(*field)) {
                    return Err(LiveError::error(format!("{} view postcondition was not confirmed", kind.noun())));
                }
            }
            record.borrow_mut()["applyKey"] = p["idempotencyKey"].clone();
            record.borrow_mut()["state"] = json!("applied");
            Ok(success_text(id, &json!({"transactionId":t["id"],"state":"applied","idempotent":false})))
        }
        .await;
        Some(result.unwrap_or_else(|e| {
            apply_failed(id, &record, &e, &format!("{}-view state is uncertain; perform fresh discovery before retrying.", kind.title()))
        }))
    }
    pub async fn undo_clip_view_async(&self, id: &Value, p: &Value, signal: Option<&Signal>) -> Value {
        self.undo_object_view(Kind::Clip, id, p, signal).await
    }
    pub async fn undo_device_view_async(&self, id: &Value, p: &Value, signal: Option<&Signal>) -> Value {
        self.undo_object_view(Kind::Device, id, p, signal).await
    }
    async fn undo_object_view(&self, kind: Kind, id: &Value, p: &Value, signal: Option<&Signal>) -> Value {
        let unknown = format!("Unknown or expired {} transaction", kind.kind());
        let Some(record) = self.clip_lifecycle_transactions.get(p["transactionId"].as_str().unwrap_or("")) else {
            return transaction_error(id, &unknown);
        };
        let t = record.borrow().clone();
        if t["kind"] != kind.kind() {
            return transaction_error(id, &unknown);
        }
        if t["state"] == "undone" && t["undoKey"] == p["idempotencyKey"] {
            return success_text(id, &json!({"transactionId":t["id"],"state":"undone","idempotent":true}));
        }
        let reconcile = t["state"] == "uncertain" && t["undoKey"] == p["idempotencyKey"];
        if (t["state"] != "applied" && !reconcile)
            || !arrangement::truthy(&t["prior"])
            || (matches!(kind, Kind::Clip) && !arrangement::truthy(&t["clipRef"]))
        {
            return transaction_error(id, &format!("Only an applied or exact-key uncertain {} transaction can be undone", kind.kind()));
        }
        if matches!(kind, Kind::Clip) && kind.fields().iter().all(|f| t["payload"].get(*f).is_none()) {
            return transaction_error(id, "show-loop and envelope visibility are momentary and not undoable");
        }
        let reference = match kind {
            Kind::Clip => &t["clipRef"],
            Kind::Device => &t["payload"]["ref"],
        };
        let result = async {
            self.begin_undo_recovery(&record, p["idempotencyKey"].as_str().unwrap_or(""))?;
            let status = self.require_connected(Some("session.read"))?;
            if json!(status.epoch) != t["epoch"] {
                return Ok(transaction_error(id, "Live connection epoch changed; undo refused"));
            }
            let adapter = self.async_adapter();
            let context = self.transaction_context(p, signal, AUDITION_DEADLINE_MS);
            record.borrow_mut()["undoKey"] = p["idempotencyKey"].clone();
            if reconcile {
                self.replay_undo_recovery(&record, adapter.as_ref(), &context).await?;
            }
            let snapshot = self.views.view_for(Some(&context), &[reference.clone()], None, &[]).await?;
            let row = self.object_view_row(kind, &snapshot, reference)?;
            if let Some(moved) = self.undo_target_moved(
                id,
                &t,
                kind.noun(),
                reference,
                row.get("objectIdentity"),
                t["payload"].get("expectedObjectIdentity"),
            )? {
                return Ok(moved);
            }
            let state = kind.state(&row);
            if !reconcile {
                for field in kind.fields() {
                    let observed = match kind {
                        Kind::Clip => kind.observed(&row, field),
                        Kind::Device => state.get(*field),
                    };
                    if t["payload"].get(*field).is_some() && !scalar_same(observed, t["payload"].get(*field)) {
                        return Ok(transaction_error(id, &format!("{} view changed after apply; undo refused", kind.noun())));
                    }
                }
            }
            if matches!(kind, Kind::Device) && !t["prior"]["collapsed"].is_boolean() {
                return Ok(transaction_error(id, "prior device view state is unavailable"));
            }
            record.borrow_mut()["state"] = json!("undoing");
            let mut args = json!({"ref":reference});
            if matches!(kind, Kind::Device) {
                args["collapsed"] = t["prior"]["collapsed"].clone();
            }
            put(&mut args, "expectedObjectIdentity", row.get("objectIdentity"));
            args["expectedStateRevision"] = json!(digest(&state)?);
            if matches!(kind, Kind::Clip) {
                for field in kind.fields() {
                    if t["payload"].get(*field).is_some() {
                        put(&mut args, field, t["prior"].get(*field).filter(|v| !v.is_null()));
                    }
                }
            }
            let result = self.invoke_undo_recovery(&record, adapter.as_ref(), kind.operation(), &args, &context).await?;
            confirmed(&result, "changed", &format!("{} view restoration was not confirmed", kind.noun()))?;
            record.borrow_mut()["state"] = json!("undone");
            Ok(success_text(id, &json!({"transactionId":t["id"],"state":"undone","idempotent":false})))
        }
        .await;
        result.unwrap_or_else(|e| {
            record.borrow_mut()["state"] = json!("uncertain");
            adapter_tool_error(id, &e, &format!("{}-view undo is uncertain; perform fresh discovery.", kind.title()))
        })
    }
}
