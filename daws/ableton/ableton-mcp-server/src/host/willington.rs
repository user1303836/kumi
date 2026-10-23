//! Willington device extension transactions bind complete mapper readback revisions.
use super::*;
use super::{clip_properties::scalar_same, reads::AUDITION_DEADLINE_MS};
use kumi_common::{abort::Signal, time::now_ms_f64};
const ZONES: &[&str] = &["selector-zone", "key-zone", "velocity-zone"];
const ENDPOINTS: &[&str] = &["minimum", "maximum", "fadeMinimum", "fadeMaximum"];
const SELECTOR: &[&str] = &["ref", "kind", "macroIndex", "targetRef"];
fn property<'a>(value: Option<&'a Value>, key: &str) -> Result<Option<&'a Value>, LiveError> {
    match value {
        None => Err(LiveError::type_error(format!("Cannot read properties of undefined (reading '{key}')"))),
        Some(Value::Null) => Err(LiveError::type_error(format!("Cannot read properties of null (reading '{key}')"))),
        Some(v) => Ok(v.get(key)),
    }
}
fn number(value: Option<&Value>) -> Result<f64, LiveError> {
    Ok(match value {
        None => f64::NAN,
        Some(Value::Null) => 0.0,
        Some(Value::Bool(v)) => {
            if *v {
                1.0
            } else {
                0.0
            }
        }
        Some(v) => kumi_common::js::number::parse(&js_string(v)?).unwrap_or(f64::NAN),
    })
}
fn put(out: &mut Value, key: &str, value: Option<&Value>) {
    if let Some(v) = value {
        out[key] = v.clone();
    }
}
impl McpHost {
    pub async fn dispatch_willington_tool(&self, call: &ToolCall, signal: Option<&Signal>) -> Option<Result<Option<Value>, LiveError>> {
        let p = call.arguments.as_ref().unwrap_or(&Value::Null);
        Some(match call.name.as_str() {
            "live_willington_device_preview" => self.live_willington_preview_async(&call.id, p).await.map(Some),
            "live_willington_device_apply" => Ok(self.live_willington_apply_async(&call.id, p, signal).await),
            _ => return None,
        })
    }
    pub async fn live_willington_preview_async(&self, id: &Value, p: &Value) -> Result<Value, LiveError> {
        if !has_only(
            p,
            &[
                "ref",
                "kind",
                "macroIndex",
                "targetRef",
                "name",
                "mappingIndex",
                "minimum",
                "maximum",
                "mappingKind",
                "fadeMinimum",
                "fadeMaximum",
            ],
        ) || !is_non_empty_string(&p["ref"], 256)
            || !["macro-name", "macro-mapping", "variation-name", "selector-zone", "key-zone", "velocity-zone"]
                .contains(&js_string(&p["kind"])?.as_str())
        {
            return Ok(error(id, -32602, "an exact Willington target and kind are required", None));
        }
        let result = async {
            let status = self.fresh_status(Some(&LiveOperationContext::with_deadline(now_ms_f64() + AUDITION_DEADLINE_MS))).await?;
            if !status.connected || !status.has_operation("willington.device.read") || !status.has_operation("willington.device.set") {
                return Err(LiveError::error("Willington device editing is unavailable"));
            }
            if let Some(kinds) = &status.willington_kinds {
                if !kinds.contains(&js_string(&p["kind"])?) {
                    return Err(LiveError::error("Willington edit kind is unavailable"));
                }
            }
            let zone = ZONES.contains(&js_string(&p["kind"])?.as_str());
            let allowed = if zone {
                &["ref", "kind", "targetRef", "minimum", "maximum", "fadeMinimum", "fadeMaximum"][..]
            } else if p["kind"] == "macro-name" {
                &["ref", "kind", "macroIndex", "name"][..]
            } else if p["kind"] == "variation-name" {
                &["ref", "kind", "name"][..]
            } else {
                &["ref", "kind", "targetRef", "mappingIndex", "minimum", "maximum", "mappingKind"][..]
            };
            if !has_only(p, allowed) {
                return Err(LiveError::error("Fields do not match the requested edit kind"));
            }
            let playback = serde_json::to_value(self.views.playback(None).await?).unwrap();
            if playback["transport"]["playing"] != false {
                return Err(LiveError::error("Willington edits require stopped playback"));
            }
            let selector = device_parameter::fields(p, SELECTOR);
            let read = self.async_adapter().invoke_async(&LiveInvocation::new("willington.device.read", selector.clone()), None).await?;
            let next = if zone {
                if !is_non_empty_string(&p["targetRef"], 256) {
                    return Err(LiveError::error("a chain targetRef is required"));
                }
                let mut next = json!({});
                for field in ENDPOINTS {
                    put(
                        &mut next,
                        field,
                        match p.get(*field) {
                            Some(v) => Some(v),
                            None => property(property(Some(&read), "state")?, field)?,
                        },
                    );
                }
                if !ENDPOINTS.iter().any(|f| p.get(*f).is_some()) {
                    return Err(LiveError::error("at least one zone endpoint is required"));
                }
                if !ENDPOINTS.iter().all(|f| next[*f].as_f64().is_some_and(|n| n.is_finite() && n.fract() == 0.0)) {
                    return Err(LiveError::error("zone endpoints must be integers"));
                }
                let state = property(Some(&read), "state")?;
                let low = number(property(state, "lowerBound")?)?;
                let high = number(property(state, "upperBound")?)?;
                let values = [
                    low,
                    next["minimum"].as_f64().unwrap(),
                    next["fadeMinimum"].as_f64().unwrap(),
                    next["fadeMaximum"].as_f64().unwrap(),
                    next["maximum"].as_f64().unwrap(),
                    high,
                ];
                if !values.windows(2).all(|v| v[0] <= v[1]) {
                    return Err(LiveError::error(
                        "zone endpoints must be ordered and in bounds; specify coupled fade endpoints when moving a range",
                    ));
                }
                let mut same = true;
                for field in ENDPOINTS {
                    same &= scalar_same(next.get(*field), property(state, field)?);
                }
                if same {
                    return Err(LiveError::error("zone preview would not change any endpoint"));
                }
                next
            } else if p["kind"] != "macro-mapping" {
                if !is_non_empty_string(&p["name"], 256) || p["name"].as_str().unwrap_or("").contains('\0') {
                    return Err(LiveError::error("name is required"));
                }
                json!({"name":p["name"]})
            } else {
                let mut next = if p.get("mappingIndex") == Some(&Value::Null) {
                    json!({"mapping":null})
                } else {
                    if !is_integer_in_range(&p["mappingIndex"], 0.0, 15.0)
                        || !["continuous", "enum", "boolean"].contains(&js_string(&p["mappingKind"])?.as_str())
                        || !p["minimum"].as_f64().is_some_and(f64::is_finite)
                        || !p["maximum"].as_f64().is_some_and(f64::is_finite)
                    {
                        return Err(LiveError::error("mapping index, kind and finite endpoints are required; use null index to unmap"));
                    }
                    let low = p["minimum"].as_f64().unwrap();
                    let high = p["maximum"].as_f64().unwrap();
                    if p["mappingKind"] == "boolean" {
                        if low.fract() != 0.0 || high.fract() != 0.0 || low < 0.0 || high > 127.0 || low > high {
                            return Err(LiveError::error("Boolean thresholds must be ordered integers in 0–127"));
                        }
                    } else {
                        let state = property(Some(&read), "state")?;
                        let min = number(property(state, "parameterMin")?)?;
                        let max = number(property(state, "parameterMax")?)?;
                        if ![low, high].iter().all(|v| *v >= min && *v <= max) {
                            return Err(LiveError::error("Mapping endpoints are outside parameter bounds"));
                        }
                        if p["mappingKind"] == "enum" && (low.fract() != 0.0 || high.fract() != 0.0) {
                            return Err(LiveError::error("Enum endpoints must be whole numbers"));
                        }
                    }
                    json!({"mapping":{"index":p["mappingIndex"],"minimum":p["minimum"],"maximum":p["maximum"],"kind":p["mappingKind"]}})
                };
                put(&mut next, "parameterValue", property(property(Some(&read), "state")?, "parameterValue")?);
                next
            };
            let revision = property(Some(&read), "stateRevision")?;
            let mut payload = selector;
            payload["next"] = next.clone();
            put(&mut payload, "expectedStateRevision", revision);
            let mut t = json!({"id":tempo::transaction_id("willington"),"epoch":status.epoch,"kind":"willington-device"});
            put(&mut t, "fence", revision);
            t["payload"] = payload;
            put(&mut t, "prior", property(Some(&read), "state")?);
            t["state"] = json!("previewed");
            t["expiresAt"] = json!(now_ms_f64() + TRANSACTION_TTL_MS);
            self.retain_bounded_transaction(&self.clip_lifecycle_transactions, t.clone(), "Willington device")?;
            let mut body = json!({"transactionId":t["id"],"epoch":t["epoch"]});
            put(&mut body, "prior", property(Some(&read), "state")?);
            body["proposed"] = next;
            body["confirmation"] = json!("apply");
            body["expiresAt"] = t["expiresAt"].clone();
            Ok(success_text(id, &body))
        }
        .await;
        Ok(result.unwrap_or_else(|e| adapter_tool_error(id, &e, "Willington preview requires complete current readback.")))
    }
    pub async fn live_willington_apply_async(&self, id: &Value, p: &Value, signal: Option<&Signal>) -> Option<Value> {
        if !valid_transaction_params(p, "apply") {
            return Some(error(id, -32602, "transactionId, confirmation=apply and idempotencyKey are required", None));
        }
        let Some(record) = self.clip_lifecycle_transactions.get(p["transactionId"].as_str().unwrap()) else {
            return Some(transaction_error(id, "Unknown or expired Willington preview"));
        };
        let t = record.borrow().clone();
        if t["kind"] != "willington-device" || (t["state"] == "previewed" && t["expiresAt"].as_f64().unwrap_or(f64::NAN) <= now_ms_f64()) {
            return Some(transaction_error(id, "Unknown or expired Willington preview"));
        }
        if t["state"] == "applied" && t["applyKey"] == p["idempotencyKey"] {
            return Some(success_text(id, &json!({"transactionId":t["id"],"state":"applied","idempotent":true})));
        }
        let reconcile = t["state"] == "uncertain" && t["applyKey"] == p["idempotencyKey"];
        if t["state"] != "previewed" && !reconcile {
            return Some(transaction_error(id, "Willington transaction is no longer applicable"));
        }
        if signal.is_some_and(Signal::is_cancelled) {
            return None;
        }
        let result = async {
            let status = self.fresh_status(Some(&LiveOperationContext::with_deadline(now_ms_f64() + AUDITION_DEADLINE_MS))).await?;
            if json!(status.epoch) != t["epoch"] || !status.has_operation("willington.device.set") {
                return Err(LiveError::error("Willington epoch or capability changed"));
            }
            let context = LiveOperationContext {
                signal: signal.cloned(),
                deadline_ms: Some(now_ms_f64() + AUDITION_DEADLINE_MS),
                transaction_id: t["id"].as_str().map(str::to_owned),
                idempotency_key: p["idempotencyKey"].as_str().map(str::to_owned),
            };
            record.borrow_mut()["state"] = json!("applying");
            record.borrow_mut()["applyKey"] = p["idempotencyKey"].clone();
            let result = self
                .async_adapter()
                .invoke_async(&LiveInvocation::new("willington.device.set", t["payload"].clone()), Some(&context))
                .await?;
            if property(Some(&result), "changed")? != Some(&Value::Bool(true)) {
                return Err(LiveError::error("Willington write was not confirmed"));
            }
            record.borrow_mut()["created"] = result.clone();
            record.borrow_mut()["state"] = json!("applied");
            let mut body = json!({"transactionId":t["id"],"state":"applied"});
            put(&mut body, "after", property(Some(&result), "state")?);
            body["idempotent"] = json!(false);
            Ok(success_text(id, &body))
        }
        .await;
        Some(result.unwrap_or_else(|e| {
            if record.borrow()["state"] == "applying" {
                record.borrow_mut()["state"] = json!("uncertain");
            }
            adapter_tool_error(
                id,
                &e,
                if record.borrow()["state"] == "uncertain" {
                    "Willington state is uncertain; reconcile this exact transaction and key."
                } else {
                    "Willington apply failed before dispatch; retry the preview before expiry."
                },
            )
        }))
    }
    pub async fn undo_willington_async(&self, id: &Value, p: &Value, signal: Option<&Signal>) -> Value {
        let Some(record) = p["transactionId"].as_str().and_then(|key| self.clip_lifecycle_transactions.get(key)) else {
            return transaction_error(id, "Unknown Willington transaction");
        };
        let t = record.borrow().clone();
        if t["kind"] != "willington-device" || !arrangement::truthy(&t["prior"]) {
            return transaction_error(id, "Unknown Willington transaction");
        }
        if t["state"] == "undone" && t["undoKey"] == p["idempotencyKey"] {
            return success_text(id, &json!({"transactionId":t["id"],"state":"undone","idempotent":true}));
        }
        let reconcile = t["state"] == "uncertain" && t["undoKey"] == p["idempotencyKey"];
        if t["state"] != "applied" && !reconcile {
            return transaction_error(id, "Only applied or exact-key uncertain Willington undo is allowed");
        }
        let result = async {
            let (_, steps) = self.begin_undo_recovery(&record, p["idempotencyKey"].as_str().unwrap())?;
            record.borrow_mut()["undoKey"] = p["idempotencyKey"].clone();
            let status = self.fresh_status(Some(&LiveOperationContext::with_deadline(now_ms_f64() + AUDITION_DEADLINE_MS))).await?;
            if json!(status.epoch) != t["epoch"] || !status.has_operation("willington.device.set") {
                return Err(LiveError::error("Willington epoch or capability changed"));
            }
            let adapter = self.async_adapter();
            let context = LiveOperationContext {
                signal: signal.cloned(),
                deadline_ms: Some(now_ms_f64() + AUDITION_DEADLINE_MS),
                transaction_id: t["id"].as_str().map(str::to_owned),
                idempotency_key: p["idempotencyKey"].as_str().map(str::to_owned),
            };
            record.borrow_mut()["undoKey"] = p["idempotencyKey"].clone();
            let selector = device_parameter::fields(&t["payload"], SELECTOR);
            if reconcile && !steps.is_empty() {
                self.replay_undo_recovery(&record, &*adapter, &context).await?;
                let restored = adapter.invoke_async(&LiveInvocation::new("willington.device.read", selector), Some(&context)).await?;
                if !scalar_same(property(Some(&restored), "stateRevision")?, t.get("fence")) {
                    return Err(LiveError::error("Willington prior state was not restored exactly"));
                }
            } else {
                let prior = &t["prior"];
                let next = if ZONES.contains(&js_string(&t["payload"]["kind"])?.as_str()) {
                    device_parameter::fields(prior, ENDPOINTS)
                } else if t["payload"]["kind"] == "macro-mapping" {
                    let text = match prior.get("mapping") {
                        Some(v) => js_string(v)?,
                        None => "undefined".into(),
                    };
                    let mut next = json!({"mapping":json_diagnostics::parse_json(&text)?});
                    put(&mut next, "parameterValue", prior.get("parameterValue"));
                    next
                } else {
                    device_parameter::fields(prior, &["name"])
                };
                record.borrow_mut()["state"] = json!("undoing");
                let mut args = selector;
                args["next"] = next;
                put(&mut args, "expectedStateRevision", t["created"].get("stateRevision"));
                let restored = self.invoke_undo_recovery(&record, &*adapter, "willington.device.set", &args, &context).await?;
                if property(Some(&restored), "changed")? != Some(&Value::Bool(true))
                    || !scalar_same(property(Some(&restored), "stateRevision")?, t.get("fence"))
                {
                    return Err(LiveError::error("Willington prior state was not restored exactly"));
                }
            }
            record.borrow_mut()["state"] = json!("undone");
            Ok(success_text(id, &json!({"transactionId":t["id"],"state":"undone","idempotent":false})))
        }
        .await;
        result.unwrap_or_else(|e| {
            record.borrow_mut()["state"] = json!("uncertain");
            adapter_tool_error(id, &e, "Willington undo is uncertain; reconcile this exact transaction and key.")
        })
    }
}
