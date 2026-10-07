//! Exact mixer identity, display readback, and restoration of the changed fields.
use super::reads::AUDITION_DEADLINE_MS;
use super::*;
use kumi_common::{abort::Signal, js::json as js_json, time::now_ms_f64};
use sha2::{Digest, Sha256};
pub(super) const MIXER_FIELDS: &[&str] = &["volume", "pan", "mute", "solo", "cueVolume", "sends"];
pub(super) struct MixerTarget {
    pub track: Value,
    pub mixer: Value,
}
fn mixer_fence(target: &MixerTarget, reference: &Value) -> String {
    js_json::stringify(&json!({"ref":reference,"objectIdentity":target.track["objectIdentity"],"mixer":target.mixer}))
}
pub(super) fn mixer_authority(target: &MixerTarget) -> Result<Value, LiveError> {
    let state = Value::Object(MIXER_FIELDS.iter().map(|k| ((*k).into(), target.mixer[*k].clone())).collect());
    Ok(
        json!({"expectedObjectIdentity":target.track["objectIdentity"],"expectedVolumeIdentity":target.mixer["volumeIdentity"],"expectedPanIdentity":target.mixer["panIdentity"],"expectedCueIdentity":target.mixer["cueIdentity"],"expectedSendIdentities":target.mixer["sendIdentities"],"expectedStateRevision":hex::encode(Sha256::digest(canonical_mutation_identity(&state)?))}),
    )
}
fn mixer_identities(authority: &Value) -> Value {
    json!({"track":authority["expectedObjectIdentity"],"volume":authority["expectedVolumeIdentity"],"pan":authority["expectedPanIdentity"],"cue":authority["expectedCueIdentity"],"sends":authority["expectedSendIdentities"]})
}
fn mixer_displays(mixer: &Value, fields: impl Iterator<Item = impl AsRef<str>>) -> Option<Value> {
    let mut out = json!({});
    for field in fields {
        let key = field.as_ref();
        let value = match key {
            "volume" => &mixer["volumeDisplay"],
            "pan" => &mixer["panDisplay"],
            "cueVolume" => &mixer["cueVolumeDisplay"],
            "sends" => &mixer["sendDisplays"],
            _ => continue,
        };
        let short = |v: &Value| v.as_str().is_some_and(|s| s.encode_utf16().count() <= 32);
        if short(value) || value.as_array().is_some_and(|a| a.iter().all(|v| v.is_null() || short(v))) {
            out[key] = value.clone();
        }
    }
    (!out.as_object().unwrap().is_empty()).then_some(out)
}
impl McpHost {
    pub(super) fn mixer_target(&self, snapshot: &Value, reference: &str) -> Result<MixerTarget, LiveError> {
        let track = snapshot["tracks"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|t| t["ref"] == reference)
            .filter(|t| t["mixer"].is_object() && is_non_empty_string(&t["objectIdentity"], 256))
            .ok_or_else(|| LiveError::error("track with an exact authoritative mixer identity is required"))?;
        let mixer = &track["mixer"];
        let nullable = |key: &str| mixer.get(key).is_some_and(|v| v.is_null() || is_non_empty_string(v, 256));
        if !["volumeIdentity", "panIdentity", "cueIdentity"].iter().all(|k| nullable(k))
            || !mixer["sendIdentities"].as_array().is_some_and(|a| {
                a.iter().all(|v| is_non_empty_string(v, 256)) && mixer["sendRefs"].as_array().is_some_and(|b| a.len() == b.len())
            })
        {
            return Err(LiveError::error("mixer parameter identities are incomplete"));
        }
        Ok(MixerTarget { track: track.clone(), mixer: mixer.clone() })
    }
    pub(super) async fn mixer_read_async(&self, context: Option<&LiveOperationContext>, reference: &str) -> Result<MixerTarget, LiveError> {
        let track = self.track_one_async(context, reference, &["ref", "objectIdentity", "name", "kind", "mixer"]).await?;
        self.mixer_target(&json!({"tracks":track.into_iter().collect::<Vec<_>>()}), reference)
    }
    pub async fn dispatch_mixer_tool(&self, call: &ToolCall, signal: Option<&Signal>) -> Option<Result<Option<Value>, LiveError>> {
        let p = call.arguments.as_ref().unwrap_or(&Value::Null);
        Some(Ok(match call.name.as_str() {
            "live_mixer_preview" => Some(self.live_mixer_preview_async(&call.id, p).await),
            "live_mixer_apply" => self.live_mixer_apply_async(&call.id, p, signal).await,
            _ => return None,
        }))
    }
    pub async fn live_mixer_preview_async(&self, id: &Value, p: &Value) -> Value {
        let allowed = [&["trackRef"][..], MIXER_FIELDS].concat();
        if !has_only(p, &allowed) || !is_non_empty_string(&p["trackRef"], 256) {
            return error(id, -32602, "trackRef is required", None);
        }
        let mut proposed = json!({});
        for field in MIXER_FIELDS {
            let Some(v) = p.get(*field) else {
                continue;
            };
            if matches!(*field, "mute" | "solo") {
                if !v.is_boolean() {
                    return error(id, -32602, &format!("{field} must be boolean"), None);
                }
            } else if *field == "sends" {
                if !v.as_array().is_some_and(|a| a.iter().all(|v| v.as_f64().is_some_and(|n| n.is_finite() && (0.0..=1.0).contains(&n)))) {
                    return error(id, -32602, "sends must be 0-1 values", None);
                }
            } else if !v.as_f64().is_some_and(|n| n.is_finite() && if *field == "pan" { n.abs() <= 1.0 } else { (0.0..=1.0).contains(&n) })
            {
                return error(id, -32602, &format!("{field} is out of bounds"), None);
            }
            proposed[*field] = v.clone();
        }
        if proposed.as_object().unwrap().is_empty() {
            return error(id, -32602, "at least one mixer field is required", None);
        }
        let result = async {
            let status = self.require_connected(Some("session.read"))?;
            if !status.has_operation("mixer.set") {
                return Err(LiveError::error("mixer editing is unavailable"));
            }
            let target = self.mixer_read_async(None, p["trackRef"].as_str().unwrap()).await?;
            let mixer = &target.mixer;
            if let Some(sends) = proposed["sends"].as_array() {
                let observed = match mixer.get("sends") {
                    None => return Err(LiveError::type_error("Cannot read properties of undefined (reading 'length')")),
                    Some(Value::Null) => return Err(LiveError::type_error("Cannot read properties of null (reading 'length')")),
                    Some(Value::Array(a)) => a.len() as f64,
                    Some(Value::String(s)) => s.encode_utf16().count() as f64,
                    Some(v) => match v.get("length") {
                        None => f64::NAN,
                        Some(Value::Null) => 0.0,
                        Some(Value::Bool(b)) => {
                            if *b {
                                1.0
                            } else {
                                0.0
                            }
                        }
                        Some(v) => kumi_common::js::number::parse(&js_string(v)?).unwrap_or(f64::NAN),
                    },
                };
                if sends.len() as f64 > observed {
                    return Err(LiveError::error("track has fewer sends than proposed"));
                }
            }
            for (field, reference, label) in
                [("cueVolume", "cueRef", "cue volume"), ("volume", "volumeRef", "volume"), ("pan", "panRef", "pan")]
            {
                if proposed.get(field).is_some() && mixer.get(reference) == Some(&Value::Null) {
                    return Err(LiveError::error(format!("{label} is unavailable on this track")));
                }
            }
            let prior = Value::Object(proposed.as_object().unwrap().keys().map(|k| (k.clone(), mixer[k].clone())).collect());
            let display = mixer_displays(mixer, proposed.as_object().unwrap().keys());
            let mut payload = json!({"ref":p["trackRef"]});
            payload.as_object_mut().unwrap().extend(proposed.as_object().unwrap().clone());
            payload.as_object_mut().unwrap().extend(mixer_authority(&target)?.as_object().unwrap().clone());
            let t = json!({"id":tempo::transaction_id("mixer"),"epoch":status.epoch,"kind":"mixer-set","fence":mixer_fence(&target,&p["trackRef"]),"clipRef":p["trackRef"],"payload":payload,"prior":prior,"expiresAt":now_ms_f64()+TRANSACTION_TTL_MS,"state":"previewed"});
            self.retain_bounded_transaction(&self.clip_lifecycle_transactions, t.clone(), "mixer")?;
            let mut body = json!({"transactionId":t["id"],"epoch":t["epoch"],"trackRef":p["trackRef"],"prior":prior});
            if let Some(display) = display {
                body["priorDisplay"] = display;
            }
            body["proposed"] = proposed;
            body["impact"] = json!("edits-mixer");
            body["confirmation"] = json!("apply");
            body["expiresAt"] = t["expiresAt"].clone();
            Ok(success_text(id, &body))
        }
        .await;
        result.unwrap_or_else(|e| adapter_tool_error(id, &e, "Mixer preview requires fresh authoritative state."))
    }
    pub async fn live_mixer_apply_async(&self, id: &Value, p: &Value, signal: Option<&Signal>) -> Option<Value> {
        if !valid_transaction_params(p, "apply") {
            return Some(error(id, -32602, "transactionId, confirmation=apply, and idempotencyKey are required", None));
        }
        let Some(record) = self.clip_lifecycle_transactions.get(p["transactionId"].as_str().unwrap()) else {
            return Some(transaction_error(id, "Unknown or expired mixer transaction"));
        };
        let t = record.borrow().clone();
        if t["kind"] != "mixer-set" || (t["state"] == "previewed" && t["expiresAt"].as_f64().unwrap_or(f64::NAN) <= now_ms_f64()) {
            return Some(transaction_error(id, "Unknown or expired mixer transaction"));
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
            let reference = t["clipRef"].as_str().unwrap();
            if !reconcile {
                let target = self.mixer_read_async(Some(&context), reference).await?;
                if mixer_fence(&target, &t["clipRef"]) != t["fence"] {
                    return Ok(transaction_error(id, "mixer target or state changed since preview; preview again"));
                }
            }
            record.borrow_mut()["state"] = json!("applying");
            record.borrow_mut()["applyKey"] = p["idempotencyKey"].clone();
            let result = adapter.invoke_async(&LiveInvocation::new("mixer.set", t["payload"].clone()), Some(&context)).await?;
            if result.is_null() {
                return Err(LiveError::type_error("Cannot read properties of null (reading 'changed')"));
            }
            if result["changed"] != true {
                return Err(LiveError::error("mixer change was not confirmed"));
            }
            let verified = self.mixer_read_async(Some(&context), reference).await?;
            for field in MIXER_FIELDS {
                if let Some(expected) = t["payload"].get(*field) {
                    if !same_mixer_value(field, verified.mixer.get(*field), Some(expected)) {
                        return Err(LiveError::error("mixer postcondition was not confirmed"));
                    }
                }
            }
            record.borrow_mut()["applyKey"] = p["idempotencyKey"].clone();
            record.borrow_mut()["state"] = json!("applied");
            let mut body = json!({"transactionId":t["id"],"state":"applied"});
            if let Some(revision) = result.get("revision") {
                body["revision"] = revision.clone();
            }
            if let Some(display) = mixer_displays(&verified.mixer, t["prior"].as_object().unwrap().keys()) {
                body["display"] = display;
            }
            body["idempotent"] = json!(false);
            Ok(success_text(id, &body))
        }
        .await;
        Some(result.unwrap_or_else(|e| apply_failed(id, &record, &e, "Mixer state is uncertain; perform fresh discovery before retrying.")))
    }
    pub async fn undo_mixer_async(&self, id: &Value, p: &Value, signal: Option<&Signal>) -> Value {
        let Some(record) = p["transactionId"].as_str().and_then(|key| self.clip_lifecycle_transactions.get(key)) else {
            return transaction_error(id, "Unknown or expired mixer transaction");
        };
        let t = record.borrow().clone();
        if t["kind"] != "mixer-set" {
            return transaction_error(id, "Unknown or expired mixer transaction");
        }
        if t["state"] == "undone" && t["undoKey"] == p["idempotencyKey"] {
            return success_text(id, &json!({"transactionId":t["id"],"state":"undone","idempotent":true}));
        }
        let reconcile = t["state"] == "uncertain" && t["undoKey"] == p["idempotencyKey"];
        if t["state"] != "applied" && !reconcile {
            return transaction_error(id, "Only an applied or exact-key uncertain mixer transaction can be undone");
        }
        let result = async {
            self.begin_undo_recovery(&record, p["idempotencyKey"].as_str().unwrap())?;
            let status = self.require_connected(Some("session.read"))?;
            if json!(status.epoch) != t["epoch"] {
                return Ok(transaction_error(id, "Live connection epoch changed; undo refused"));
            }
            let adapter = self.async_adapter();
            let context = self.transaction_context(p, signal, AUDITION_DEADLINE_MS);
            record.borrow_mut()["undoKey"] = p["idempotencyKey"].clone();
            if reconcile {
                self.replay_undo_recovery(&record, &*adapter, &context).await?;
            }
            let reference = t["clipRef"].as_str().unwrap();
            let mut current = self.mixer_read_async(Some(&context), reference).await?;
            // Shared moved-target helper is supplied by the audio-clip family.
            if let Some(moved) = self.undo_target_moved(
                id,
                &t,
                "track",
                &t["clipRef"],
                Some(&mixer_identities(&mixer_authority(&current)?)),
                Some(&mixer_identities(&t["payload"])),
            )? {
                return Ok(moved);
            }
            // What the change named: a shorter `sends` list set only the first sends, so only they go back.
            let named = |field: &str, value: &Value| named_mixer_part(field, value, &t["payload"][field]);
            if reconcile {
                for field in MIXER_FIELDS {
                    if t["payload"].get(*field).is_some()
                        && !same_live_value(Some(&named(field, &current.mixer[*field])), Some(&named(field, &t["prior"][*field])))
                    {
                        return Ok(transaction_error(id, "mixer undo replay did not restore prior state"));
                    }
                }
            }
            let already = MIXER_FIELDS.iter().all(|field| {
                t["payload"].get(*field).is_none()
                    || js_json::stringify(&named(field, &current.mixer[*field])) == js_json::stringify(&named(field, &t["prior"][*field]))
            });
            if !reconcile && !already {
                let mut restore = json!({"ref":t["clipRef"]});
                restore.as_object_mut().unwrap().extend(mixer_authority(&current)?.as_object().unwrap().clone());
                for field in MIXER_FIELDS {
                    if t["payload"].get(*field).is_some() {
                        restore[*field] = named(field, &t["prior"][*field]);
                    }
                }
                record.borrow_mut()["state"] = json!("undoing");
                let result = self.invoke_undo_recovery(&record, &*adapter, "mixer.set", &restore, &context).await?;
                if result.is_null() {
                    return Err(LiveError::type_error("Cannot read properties of null (reading 'changed')"));
                }
                if result["changed"] != true {
                    return Err(LiveError::error("mixer undo was not confirmed"));
                }
            }
            current = self.mixer_read_async(Some(&context), reference).await?;
            for field in MIXER_FIELDS {
                if t["payload"].get(*field).is_some()
                    && js_json::stringify(&named(field, &current.mixer[*field])) != js_json::stringify(&named(field, &t["prior"][*field]))
                {
                    return Err(LiveError::error("mixer exact prior state was not restored"));
                }
            }
            record.borrow_mut()["state"] = json!("undone");
            Ok(success_text(id, &json!({"transactionId":t["id"],"state":"undone","restored":t["prior"],"idempotent":false})))
        }
        .await;
        result.unwrap_or_else(|e| {
            record.borrow_mut()["state"] = json!("uncertain");
            adapter_tool_error(id, &e, "Mixer undo is uncertain; perform fresh discovery.")
        })
    }
}
