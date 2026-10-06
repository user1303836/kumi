//! Kumi-owned text stored inside a Set or on an identity-bound track.
use super::*;
use kumi_common::{
    abort::Signal,
    js::{json as js_json, string::utf16_len},
    time::now_ms_f64,
};
impl McpHost {
    pub async fn dispatch_data_tool(&self, call: &ToolCall, signal: Option<&Signal>) -> Option<Result<Option<Value>, LiveError>> {
        let p = call.arguments.as_ref().unwrap_or(&Value::Null);
        Some(Ok(match call.name.as_str() {
            "live_data_preview" => Some(self.live_data_preview_async(&call.id, p).await),
            "live_data_apply" => self.live_data_apply_async(&call.id, p, signal).await,
            _ => return None,
        }))
    }
    pub async fn live_data_preview_async(&self, id: &Value, p: &Value) -> Value {
        if !has_only(p, &["key", "value", "trackRef", "expectedValue", "expectedIdentity", "entries"])
            || !is_non_empty_string(&p["key"], 256)
            || p.get("value").is_none()
            || !text_or_null(&p["value"])
            || p.get("trackRef").is_some_and(|v| !is_non_empty_string(v, 256))
        {
            return error(id, -32602, "key and value (text of at most 1 MiB, or null to clear it) are required", None);
        }
        if !p["key"].as_str().unwrap().starts_with("kumi.") {
            return error(id, -32602, "Kumi writes only its own keys: start the key with kumi. (other keys are read-only)", None);
        }
        if p.get("entries").is_some() || p.get("expectedValue").is_some() || p.get("expectedIdentity").is_some() {
            return self.live_data_batch_preview_async(id, p).await;
        }
        let result=async{
            let status=self.fresh_status(Some(&LiveOperationContext::with_deadline(self.deadline(reads::AUDITION_DEADLINE_MS)))).await?;
            if !status.has_operation("data.get")||!status.has_operation("data.set"){return Err(LiveError::error("data saved in the Set is unavailable on this Live shape"));}
            let context=LiveOperationContext::with_deadline(self.deadline(reads::AUDITION_DEADLINE_MS));
            let owner=if let Some(reference)=p.get("trackRef"){reference.clone()}else{
                let snapshot=self.views.view(Some(&context),LiveViewScope::Indices(vec![]),Some(&[LiveSnapshotPart::Set])).await?;
                json!(snapshot.set.ok_or_else(||LiveError::type_error("Cannot read properties of undefined (reading 'ref')"))?.ref_)
            };
            let identity=if let Some(reference)=p["trackRef"].as_str(){self.track_one_async(Some(&context),reference,&["ref","objectIdentity"]).await?.and_then(|r|r.get("objectIdentity").cloned())}else{None};
            if p.get("trackRef").is_some()&&!identity.as_ref().is_some_and(|v|is_non_empty_string(v,256)){return Err(LiveError::error("track reference is stale or invalid"));}
            let read=self.async_adapter().invoke_async(&LiveInvocation::new("data.get",json!({"ref":owner,"key":p["key"]})),Some(&context)).await?;
            if read.is_null(){return Err(LiveError::type_error("Cannot read properties of null (reading 'value')"));}
            let current=if read["value"].is_string(){read["value"].clone()}else{Value::Null};let payload=json!({"ref":owner,"key":p["key"],"value":p["value"],"expectedValue":current});
            let mut t=json!({"id":tempo::transaction_id("data"),"epoch":status.epoch,"kind":"data-set","fence":js_json::stringify(&json!({"ref":owner,"key":p["key"],"value":current})),"payload":payload,"prior":{"value":current},"expiresAt":now_ms_f64()+TRANSACTION_TTL_MS,"state":"previewed"});if let Some(identity)=identity.filter(|v|is_non_empty_string(v,256)){t["targetIdentity"]=identity;}
            self.retain_bounded_transaction(&self.clip_lifecycle_transactions,t.clone(),"saved text")?;
            Ok(success_text(id,&json!({"transactionId":t["id"],"epoch":t["epoch"],"ref":owner,"key":p["key"],"prior":current,"proposed":p["value"],"impact":"saves-text-in-the-set","confirmation":"apply","expiresAt":t["expiresAt"]})))
        }.await;
        result.unwrap_or_else(|e| adapter_tool_error(id, &e, "Nothing was saved; preview again from a fresh track reference."))
    }
    /// Text under one key on several tracks at once (Kumi's own ids for them). Each place names what was read
    /// there (its text and the track's identity); the apply checks them, so the preview reads nothing itself.
    async fn live_data_batch_preview_async(&self, id: &Value, p: &Value) -> Value {
        // Short text in a batch: it's for ids, and a thousand places of 1 MiB each would be too much.
        let short = |value: &Value| value.is_null() || value.as_str().is_some_and(|s| utf16_len(s) <= 256);
        let place = |entry: &Value| {
            is_non_empty_string(&entry["trackRef"], 256)
                && entry.get("value").is_some_and(short)
                && entry.get("expectedValue").is_some_and(short)
                && is_non_empty_string(&entry["expectedIdentity"], 256)
        };
        let entries = p.get("entries").map(|v| v.as_array().filter(|a| (1..=1023).contains(&a.len())));
        if !place(p)
            || entries.is_some_and(|e| {
                e.is_none_or(|e| !e.iter().all(|e| has_only(e, &["trackRef", "value", "expectedValue", "expectedIdentity"]) && place(e)))
            })
        {
            return error(id, -32602, "each place needs trackRef, value and expectedValue (what was read there, or null; text of at most 256 characters in a batch) and expectedIdentity, and entries 1 to 1023 of them", None);
        }
        let more: Vec<&Value> = entries.flatten().into_iter().flatten().collect();
        let mut places: Vec<&Value> = std::iter::once(&p["trackRef"]).chain(more.iter().map(|e| &e["trackRef"])).collect();
        places.sort_by_key(|r| r.as_str().unwrap_or(""));
        if places.windows(2).any(|w| w[0] == w[1]) {
            return error(id, -32602, "each track can be named only once", None);
        }
        let result = async {
            let status = self.fresh_status(Some(&LiveOperationContext::with_deadline(self.deadline(reads::AUDITION_DEADLINE_MS)))).await?;
            if !status.has_operation("data.set") {
                return Err(LiveError::error("data saved in the Set is unavailable on this Live shape"));
            }
            let mut payload = json!({"ref":p["trackRef"],"key":p["key"],"value":p["value"],"expectedValue":p["expectedValue"],"expectedIdentity":p["expectedIdentity"]});
            if !more.is_empty() {
                payload["entries"] = more
                    .iter()
                    .map(|e| json!({"ref":e["trackRef"],"value":e["value"],"expectedValue":e["expectedValue"],"expectedIdentity":e["expectedIdentity"]}))
                    .collect();
            }
            let t = json!({"id":tempo::transaction_id("data"),"epoch":status.epoch,"kind":"data-set","batch":true,"payload":payload,"expiresAt":now_ms_f64()+TRANSACTION_TTL_MS,"state":"previewed"});
            self.retain_bounded_transaction(&self.clip_lifecycle_transactions, t.clone(), "saved text")?;
            Ok(success_text(
                id,
                &json!({"transactionId":t["id"],"epoch":t["epoch"],"key":p["key"],"places":more.len()+1,"impact":"saves-text-on-tracks","confirmation":"apply","expiresAt":t["expiresAt"]}),
            ))
        }
        .await;
        result.unwrap_or_else(|e| adapter_tool_error(id, &e, "Nothing was saved; preview again from a fresh read of the tracks."))
    }
    /// A batch, applied: every track named is checked to be the one read (together, one read each) before
    /// any text is written, then all of it is written in one call, all or none: each track's write checks what
    /// it holds, and one that changed since leaves every track as it was.
    async fn apply_data_batch(
        &self,
        id: &Value,
        t: &Value,
        record: &Rc<RefCell<Value>>,
        p: &Value,
        context: &LiveOperationContext,
    ) -> Result<Value, LiveError> {
        let payload = &t["payload"];
        let places: Vec<&Value> = std::iter::once(payload).chain(payload["entries"].as_array().into_iter().flatten()).collect();
        let found = futures::future::join_all(
            places.iter().map(|place| self.track_one_async(Some(context), place["ref"].as_str().unwrap_or(""), &["ref", "objectIdentity"])),
        )
        .await;
        for (place, found) in places.iter().zip(found) {
            if found?.as_ref().and_then(|row| row.get("objectIdentity")) != Some(&place["expectedIdentity"]) {
                return Ok(transaction_error(
                    id,
                    "A track named isn't the one that was read any more, so nothing was saved; read the tracks again",
                ));
            }
        }
        record.borrow_mut()["state"] = json!("applying");
        record.borrow_mut()["applyKey"] = p["idempotencyKey"].clone();
        // One call: Live saves them all or none, so a track whose text changed since it was read leaves every
        // track as it was.
        let entries: Vec<Value> = places
            .iter()
            .map(|place| json!({"ref":place["ref"],"key":payload["key"],"value":place["value"],"expectedValue":place["expectedValue"]}))
            .collect();
        let written = match self
            .async_adapter()
            .invoke_async(&LiveInvocation::new("data.set", json!({"entries":entries})), Some(context))
            .await
        {
            Ok(written) => written,
            // Refused before anything was written: the transaction is as it was.
            Err(error) if error.message().contains("changed since it was read") => {
                record.borrow_mut()["state"] = json!("previewed");
                return Ok(transaction_error(id, "A track's text changed since it was read, so nothing was saved; read the tracks again"));
            }
            Err(error) => return Err(error),
        };
        let saved: Vec<Value> = places
            .iter()
            .zip(written["entries"].as_array().into_iter().flatten())
            .filter(|(place, write)| write["ref"] == place["ref"] && write["value"] == place["value"])
            .map(|(place, write)| json!({"ref":place["ref"],"value":write["value"],"prior":write["prior"]}))
            .collect();
        if saved.len() != places.len() {
            return Err(LiveError::error("Live didn't confirm the text on every track; read them again"));
        }
        record.borrow_mut()["state"] = json!("applied");
        Ok(success_text(id, &json!({"transactionId":t["id"],"state":"applied","key":payload["key"],"saved":saved,"idempotent":false})))
    }
    pub async fn live_data_apply_async(&self, id: &Value, p: &Value, signal: Option<&Signal>) -> Option<Value> {
        if !valid_transaction_params(p, "apply") {
            return Some(error(id, -32602, "transactionId, confirmation=apply, and idempotencyKey are required", None));
        }
        let Some(record) = self.clip_lifecycle_transactions.get(p["transactionId"].as_str().unwrap()).filter(|r| {
            let t = r.borrow();
            t["kind"] == "data-set" && !(t["state"] == "previewed" && t["expiresAt"].as_f64().unwrap_or(0.) <= now_ms_f64())
        }) else {
            return Some(transaction_error(id, "Unknown or expired saved-text transaction"));
        };
        let t = record.borrow().clone();
        if t["state"] == "applied" && t["applyKey"] == p["idempotencyKey"] {
            return Some(success_text(id, &json!({"transactionId":t["id"],"state":"applied","idempotent":true})));
        }
        let reconciliation = t["state"] == "uncertain" && t["applyKey"] == p["idempotencyKey"];
        if t["state"] != "previewed" && !reconciliation {
            return Some(transaction_error(id, "Transaction is no longer applicable"));
        }
        if signal.is_some_and(Signal::is_cancelled) {
            return None;
        }
        let result=async{
            let status=self.require_connected(None)?;if json!(status.epoch)!=t["epoch"]{return Ok(transaction_error(id,"Live connection epoch changed; preview again"));}
            let context=self.transaction_context(p,signal,reads::AUDITION_DEADLINE_MS);
            if t["batch"]==true{return self.apply_data_batch(id,&t,&record,p,&context).await;}
            record.borrow_mut()["state"]=json!("applying");record.borrow_mut()["applyKey"]=p["idempotencyKey"].clone();
            let result=self.async_adapter().invoke_async(&LiveInvocation::new("data.set",t["payload"].clone()),Some(&context)).await?;
            if result.is_null(){return Err(LiveError::type_error("Cannot read properties of null (reading 'value')"));}
            if result.get("value")!=Some(&t["payload"]["value"]){return Err(LiveError::error("the saved text wasn't confirmed"));}
            record.borrow_mut()["state"]=json!("applied");Ok(success_text(id,&json!({"transactionId":t["id"],"state":"applied","key":t["payload"]["key"],"value":result["value"],"prior":result["prior"],"idempotent":false})))
        }.await;
        Some(result.unwrap_or_else(|e| {
            if record.borrow()["state"] == "applying" {
                record.borrow_mut()["state"] = json!("uncertain");
            }
            adapter_tool_error(id, &e, "Whether the text was saved is uncertain: read it again before trying again.")
        }))
    }
    pub async fn undo_data_async(&self, id: &Value, p: &Value, signal: Option<&Signal>) -> Value {
        let Some(record) =
            self.clip_lifecycle_transactions.get(p["transactionId"].as_str().unwrap()).filter(|r| r.borrow()["kind"] == "data-set")
        else {
            return transaction_error(id, "Unknown or expired saved-text transaction");
        };
        let t = record.borrow().clone();
        if t["batch"] == true {
            return transaction_error(id, "Text saved on several tracks at once (Kumi's own ids for them) isn't taken back");
        }
        if t["state"] == "undone" && t["undoKey"] == p["idempotencyKey"] {
            return success_text(id, &json!({"transactionId":t["id"],"state":"undone","idempotent":true}));
        }
        let reconciliation = t["state"] == "uncertain" && t["undoKey"] == p["idempotencyKey"];
        if t["state"] != "applied" && !reconciliation {
            return transaction_error(id, "Only applied saved text can be undone");
        }
        let result = async {
            let status = self.require_connected(None)?;
            if json!(status.epoch) != t["epoch"] {
                return Ok(transaction_error(id, "Live connection epoch changed; undo refused"));
            }
            let context = self.transaction_context(p, signal, reads::AUDITION_DEADLINE_MS);
            let prior = &t["prior"]["value"];
            if t.get("targetIdentity").is_some() {
                let track = self.track_one_async(Some(&context), t["payload"]["ref"].as_str().unwrap(), &["ref", "objectIdentity"]).await?;
                if let Some(moved) = self.undo_target_moved(
                    id,
                    &t,
                    "track",
                    &t["payload"]["ref"],
                    track.as_ref().and_then(|r| r.get("objectIdentity")),
                    t.get("targetIdentity"),
                )? {
                    return Ok(moved);
                }
            }
            record.borrow_mut()["state"] = json!("undoing");
            record.borrow_mut()["undoKey"] = p["idempotencyKey"].clone();
            let result = self
                .async_adapter()
                .invoke_async(
                    &LiveInvocation::new(
                        "data.set",
                        json!({"ref":t["payload"]["ref"],"key":t["payload"]["key"],"value":prior,"expectedValue":t["payload"]["value"]}),
                    ),
                    Some(&context),
                )
                .await?;
            if result.is_null() {
                return Err(LiveError::type_error("Cannot read properties of null (reading 'value')"));
            }
            if result.get("value") != Some(prior) {
                return Err(LiveError::error("the text that was there wasn't confirmed"));
            }
            record.borrow_mut()["state"] = json!("undone");
            Ok(success_text(
                id,
                &json!({"transactionId":t["id"],"state":"undone","key":t["payload"]["key"],"value":prior,"idempotent":false}),
            ))
        }
        .await;
        result.unwrap_or_else(|e| {
            record.borrow_mut()["state"] = json!("uncertain");
            adapter_tool_error(id, &e, "Whether the text went back is uncertain: read it again.")
        })
    }
}
/// Text of at most 1 MiB, or null.
fn text_or_null(value: &Value) -> bool {
    value.is_null() || value.as_str().is_some_and(|s| utf16_len(s) <= 1_048_576)
}
