//! Routing previews fence exact state; restoration follows track identity across positional moves.
use super::*;
use super::{device_parameter::fields, reads::AUDITION_DEADLINE_MS};
use kumi_common::{abort::Signal, js::json as js_json, time::now_ms_f64};
use sha2::{Digest, Sha256};
const FIELDS: &[&str] = &["inputType", "inputSubRouting", "outputType", "outputSubRouting", "arm", "monitoring"];
/// Live 12.4 crashes, losing unsaved work, when a track's input is set to Main, the main output (#195);
/// Live 11 called it Master. "Resampling" records what Main plays.
const INPUT_FROM_MAIN: &str = "Live crashes when a track's input is set to Main (Live 12.4: unsaved work is lost), so nothing changed. To record the mix, set inputType to \"Resampling\", which is what Main plays. A track the producer named Main or Master can't be told from it by name: rename that track to take it as an input.";
fn tracks(snapshot: &Value) -> impl Iterator<Item = &Value> {
    snapshot["tracks"].as_array().into_iter().flatten()
}
fn routing_fence(track: &Value, reference: &Value) -> String {
    let mut fence = json!({"ref":reference});
    for key in ["objectIdentity", "routing", "armed", "monitoringState"] {
        if let Some(value) = track.get(key) {
            fence[key] = value.clone();
        }
    }
    js_json::stringify(&fence)
}
fn observed_routing(track: &Value) -> Value {
    let mut out = fields(&track["routing"], &FIELDS[..4]);
    if let Some(value) = track.get("armed") {
        out["arm"] = value.clone();
    }
    if let Some(value) = track.get("monitoringState") {
        out["monitoring"] = value.clone();
    }
    out
}
fn routing_revision(track: &Value) -> Result<String, LiveError> {
    if !track["routing"].is_object() {
        return Err(LiveError::error("routing state is unavailable"));
    }
    let observed = observed_routing(track);
    let mut state = json!({});
    for key in FIELDS {
        state[key] = observed[key].clone();
    }
    Ok(hex::encode(Sha256::digest(canonical_mutation_identity(&state)?)))
}
fn routing_cycle(snapshot: &Value, target: &str, proposed: &Value) -> bool {
    use std::collections::HashMap;
    let rows: Vec<_> = tracks(snapshot).filter(|t| t["ref"].is_string() && t["name"].is_string() && t["routing"].is_object()).collect();
    let mut names: HashMap<&str, Vec<&str>> = HashMap::new();
    let mut edges: HashMap<&str, HashSet<&str>> = HashMap::new();
    for track in &rows {
        let reference = track["ref"].as_str().unwrap();
        names.entry(track["name"].as_str().unwrap()).or_default().push(reference);
        edges.insert(reference, HashSet::new());
    }
    for track in &rows {
        let reference = track["ref"].as_str().unwrap();
        let effective =
            |key: &str| if reference == target { proposed.get(key).unwrap_or(&track["routing"][key]) } else { &track["routing"][key] };
        if let Some(destinations) = effective("outputType").as_str().and_then(|name| names.get(name)) {
            edges.get_mut(reference).unwrap().extend(destinations.iter().copied());
        }
        if let Some(sources) = effective("inputType").as_str().and_then(|name| names.get(name)) {
            for source in sources {
                edges.get_mut(source).unwrap().insert(reference);
            }
        }
    }
    let mut incoming: HashMap<&str, usize> = edges.keys().map(|k| (*k, 0)).collect();
    for adjacent in edges.values() {
        for to in adjacent {
            *incoming.get_mut(to).unwrap() += 1;
        }
    }
    let mut ready: Vec<_> = incoming.iter().filter_map(|(r, n)| (*n == 0).then_some(*r)).collect();
    let mut removed = 0;
    while let Some(reference) = ready.pop() {
        removed += 1;
        for to in &edges[reference] {
            let n = incoming.get_mut(to).unwrap();
            *n -= 1;
            if *n == 0 {
                ready.push(*to);
            }
        }
    }
    removed != edges.len()
}
fn prior_keys(transaction: &Value) -> impl Iterator<Item = &str> {
    FIELDS.iter().copied().filter(|key| transaction["payload"].get(*key).is_some())
}
impl McpHost {
    pub async fn dispatch_routing_tool(&self, call: &ToolCall, signal: Option<&Signal>) -> Option<Result<Option<Value>, LiveError>> {
        let params = call.arguments.as_ref().unwrap_or(&Value::Null);
        Some(match call.name.as_str() {
            "live_routing_preview" => self.live_routing_preview_async(&call.id, params).await.map(Some),
            "live_routing_apply" => Ok(self.live_routing_apply_async(&call.id, params, signal).await),
            _ => return None,
        })
    }
    async fn routing_view_async(
        &self,
        context: Option<&LiveOperationContext>,
        reference: &Value,
        proposed: &Value,
    ) -> Result<LiveSnapshot, LiveError> {
        if proposed.get("inputType").is_some() || proposed.get("outputType").is_some() {
            self.views.whole_set(context, Some(&[LiveSnapshotPart::Tracks])).await
        } else {
            self.views.view_for(context, &[reference.clone()], None, &[]).await
        }
    }
    async fn routing_identity_view_async(
        &self,
        context: Option<&LiveOperationContext>,
        identity: &Value,
        restored: &Value,
    ) -> Result<LiveSnapshot, LiveError> {
        if restored.get("inputType").is_some() || restored.get("outputType").is_some() {
            return self.views.whole_set(context, Some(&[LiveSnapshotPart::Tracks])).await;
        }
        let listed = self.views.view(context, LiveViewScope::Indices(vec![]), Some(&[LiveSnapshotPart::Tracks])).await?;
        let value = serde_json::to_value(&listed).unwrap();
        let indices: Vec<_> = tracks(&value).enumerate().filter_map(|(i, t)| (&t["objectIdentity"] == identity).then_some(i)).collect();
        if indices.len() == 1 && value["tracks"][indices[0]]["light"] == true {
            self.views.view(context, LiveViewScope::Indices(indices), None).await
        } else {
            Ok(listed)
        }
    }
    async fn confirm_routing_fields(
        &self,
        context: &LiveOperationContext,
        reference: &Value,
        identity: &Value,
        expected: &Value,
        expected_keys: &[&str],
        failure: &str,
    ) -> Result<Value, LiveError> {
        loop {
            let snapshot = self.views.view_for(Some(context), &[reference.clone()], None, &[]).await?;
            let snapshot = serde_json::to_value(snapshot).unwrap();
            let track = tracks(&snapshot)
                .find(|t| &t["ref"] == reference)
                .filter(|t| &t["objectIdentity"] == identity)
                .ok_or_else(|| LiveError::error("routing target identity changed after apply"))?;
            let observed = observed_routing(track);
            if expected_keys.iter().all(|key| observed.get(*key) == expected.get(*key)) {
                return Ok(track.clone());
            }
            if now_ms_f64() >= context.deadline_ms.unwrap() - 250.0 {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
        Err(LiveError::error(failure))
    }
    pub async fn live_routing_preview_async(&self, id: &Value, params: &Value) -> Result<Value, LiveError> {
        if !has_only(params, &["trackRef", "inputType", "inputSubRouting", "outputType", "outputSubRouting", "arm", "monitoring"])
            || !is_non_empty_string(&params["trackRef"], 256)
        {
            return Ok(error(id, -32602, "trackRef is required", None));
        }
        let mut proposed = json!({});
        for key in FIELDS {
            let Some(value) = params.get(*key) else { continue };
            if *key == "arm" {
                if !value.is_boolean() {
                    return Ok(error(id, -32602, "arm must be boolean", None));
                }
            } else if *key == "monitoring" {
                if !["in", "auto", "off"].contains(&js_string(value)?.as_str()) {
                    return Ok(error(id, -32602, "monitoring must be in, auto, or off", None));
                }
            } else if !is_non_empty_string(value, 256) && value != "" {
                return Ok(error(id, -32602, &format!("{key} is invalid"), None));
            }
            proposed[key] = value.clone();
        }
        if proposed.as_object().unwrap().is_empty() {
            return Ok(error(id, -32602, "at least one routing field is required", None));
        }
        if proposed.get("inputType").and_then(Value::as_str).is_some_and(|input| ["Main", "Master"].contains(&input.trim())) {
            return Ok(adapter_tool_error(id, &LiveError::error(INPUT_FROM_MAIN), "Record the mix with inputType \"Resampling\" instead."));
        }
        let result = async {
            let status = self.fresh_status(Some(&LiveOperationContext::with_deadline(self.deadline(AUDITION_DEADLINE_MS)))).await?;
            if !status.connected || !status.capabilities.iter().any(|c| c.as_str() == "session.read") {
                return Err(LiveError::error("session read capability is unavailable"));
            }
            if !status.has_operation("routing.set") {
                return Err(LiveError::error("routing editing is unavailable"));
            }
            let snapshot = self.routing_view_async(None, &params["trackRef"], &proposed).await?;
            let snapshot = serde_json::to_value(snapshot).unwrap();
            let track = tracks(&snapshot)
                .find(|t| t["ref"] == params["trackRef"])
                .filter(|t| t["routing"].is_object() && is_non_empty_string(&t["objectIdentity"], 256))
                .ok_or_else(|| LiveError::error("track with exact authoritative routing identity is required"))?;
            if routing_cycle(&snapshot, params["trackRef"].as_str().unwrap(), &proposed) {
                return Err(LiveError::error("routing would create a direct or transitive feedback loop"));
            }
            let mut prior = json!({});
            for key in proposed.as_object().unwrap().keys() {
                let value = track["routing"].get(key).filter(|v| !v.is_null()).or_else(|| match key.as_str() {
                    "arm" => track.get("armed"),
                    "monitoring" => track.get("monitoringState"),
                    _ => Some(&Value::Null),
                });
                if let Some(value) = value {
                    prior[key] = value.clone();
                }
            }
            let mut payload = json!({"ref":params["trackRef"]});
            for (k, v) in proposed.as_object().unwrap() {
                payload[k] = v.clone();
            }
            payload["expectedObjectIdentity"] = track["objectIdentity"].clone();
            payload["expectedStateRevision"] = json!(routing_revision(track)?);
            let transaction = json!({"id":tempo::transaction_id("routing"),"epoch":status.epoch,"kind":"routing-set","fence":routing_fence(track,&params["trackRef"]),"clipRef":params["trackRef"],"payload":payload,"prior":prior,"expiresAt":now_ms_f64()+TRANSACTION_TTL_MS,"state":"previewed"});
            self.retain_bounded_transaction(&self.clip_lifecycle_transactions, transaction.clone(), "routing")?;
            Ok(success_text(id, &json!({"transactionId":transaction["id"],"epoch":transaction["epoch"],"trackRef":params["trackRef"],"prior":prior,"proposed":proposed,"impact":"edits-routing","confirmation":"apply","expiresAt":transaction["expiresAt"]})))
        }
        .await;
        Ok(result.unwrap_or_else(|e| adapter_tool_error(id, &e, "Routing preview requires fresh authoritative state.")))
    }
    pub async fn live_routing_apply_async(&self, id: &Value, params: &Value, signal: Option<&Signal>) -> Option<Value> {
        if !valid_transaction_params(params, "apply") {
            return Some(error(id, -32602, "transactionId, confirmation=apply, and idempotencyKey are required", None));
        }
        let Some(record) = self.clip_lifecycle_transactions.get(params["transactionId"].as_str().unwrap()) else {
            return Some(transaction_error(id, "Unknown or expired routing transaction"));
        };
        let t = record.borrow().clone();
        if t["kind"] != "routing-set" || (t["state"] == "previewed" && t["expiresAt"].as_f64().unwrap() <= now_ms_f64()) {
            return Some(transaction_error(id, "Unknown or expired routing transaction"));
        }
        if t["state"] == "applied" && t["applyKey"] == params["idempotencyKey"] {
            return Some(success_text(id, &json!({"transactionId":t["id"],"state":"applied","idempotent":true})));
        }
        let reconciliation = t["state"] == "uncertain" && t["applyKey"] == params["idempotencyKey"];
        if t["state"] != "previewed" && !reconciliation {
            return Some(transaction_error(id, "Transaction is no longer applicable"));
        }
        if signal.is_some_and(Signal::is_cancelled) {
            return None;
        }
        let result = async {
            if reconciliation {
                self.fresh_status(Some(&LiveOperationContext::with_deadline(self.deadline(AUDITION_DEADLINE_MS)))).await?;
            }
            let status = self.require_connected(Some("session.read"))?;
            if json!(status.epoch) != t["epoch"] {
                return Ok(transaction_error(id, "Live connection epoch changed; preview again"));
            }
            let adapter = self.async_adapter();
            let context = self.transaction_context(params, signal, AUDITION_DEADLINE_MS);
            if !reconciliation {
                let snapshot = self.routing_view_async(Some(&context), &t["clipRef"], &t["payload"]).await?;
                let snapshot = serde_json::to_value(snapshot).unwrap();
                let track = tracks(&snapshot).find(|v| v["ref"] == t["clipRef"]);
                if track.is_none_or(|track| routing_fence(track, &t["clipRef"]) != t["fence"]) {
                    return Ok(transaction_error(id, "routing target or state changed since preview; preview again"));
                }
                if routing_cycle(&snapshot, t["clipRef"].as_str().unwrap(), &t["payload"]) {
                    return Ok(transaction_error(id, "routing would create a direct or transitive feedback loop"));
                }
            }
            {
                let mut record = record.borrow_mut();
                record["state"] = json!("applying");
                record["applyKey"] = params["idempotencyKey"].clone();
            }
            let result = adapter.invoke_async(&LiveInvocation::new("routing.set", t["payload"].clone()), Some(&context)).await?;
            if result.is_null() {
                return Err(LiveError::type_error("Cannot read properties of null (reading 'changed')"));
            }
            if result["changed"] != true {
                return Err(LiveError::error("routing change was not confirmed"));
            }
            let expected = Value::Object(prior_keys(&t).map(|key| (key.into(), t["payload"][key].clone())).collect());
            let applied = self
                .confirm_routing_fields(
                    &context,
                    &t["clipRef"],
                    &t["payload"]["expectedObjectIdentity"],
                    &expected,
                    &prior_keys(&t).collect::<Vec<_>>(),
                    "routing postcondition was not confirmed",
                )
                .await?;
            {
                let mut record = record.borrow_mut();
                record["created"] = observed_routing(&applied);
                record["applyKey"] = params["idempotencyKey"].clone();
                record["state"] = json!("applied");
            }
            let mut response = json!({"transactionId":t["id"],"state":"applied"});
            if let Some(revision) = result.get("revision") {
                response["revision"] = revision.clone();
            }
            response["idempotent"] = json!(false);
            Ok(success_text(id, &response))
        }
        .await;
        Some(
            result.unwrap_or_else(|e| {
                self.apply_failed(id, &record, &e, "Routing state is uncertain; perform fresh discovery before retrying.")
            }),
        )
    }
    pub async fn undo_routing_async(&self, id: &Value, params: &Value, signal: Option<&Signal>) -> Value {
        let Some(record) = params["transactionId"].as_str().and_then(|id| self.clip_lifecycle_transactions.get(id)) else {
            return transaction_error(id, "Unknown routing transaction");
        };
        let t = record.borrow().clone();
        if t["kind"] != "routing-set" {
            return transaction_error(id, "Unknown routing transaction");
        }
        if t["state"] == "undone" && t["undoKey"] == params["idempotencyKey"] {
            return success_text(id, &json!({"transactionId":t["id"],"state":"undone","idempotent":true}));
        }
        let reconciliation = t["state"] == "uncertain" && t["undoKey"] == params["idempotencyKey"];
        if (t["state"] != "applied" && !reconciliation) || !arrangement::truthy(&t["clipRef"]) || !arrangement::truthy(&t["prior"]) {
            return transaction_error(id, "Only an applied or exact-key uncertain routing transaction can be undone");
        }
        let result = async {
            self.begin_undo_recovery(&record, params["idempotencyKey"].as_str().unwrap())?;
            let status = self.require_connected(Some("session.read"))?;
            if json!(status.epoch) != t["epoch"] {
                return Ok(transaction_error(id, "Live connection epoch changed; undo refused"));
            }
            let adapter = self.async_adapter();
            let context = self.transaction_context(params, signal, AUDITION_DEADLINE_MS);
            record.borrow_mut()["undoKey"] = params["idempotencyKey"].clone();
            if reconciliation {
                self.replay_undo_recovery(&record, &*adapter, &context).await?;
            }
            let snapshot = self.routing_identity_view_async(Some(&context), &t["payload"]["expectedObjectIdentity"], &t["prior"]).await?;
            let snapshot = serde_json::to_value(snapshot).unwrap();
            let matches: Vec<_> =
                tracks(&snapshot).filter(|track| track["objectIdentity"] == t["payload"]["expectedObjectIdentity"]).collect();
            let track = if matches.len() == 1 { Some(matches[0]) } else { None }.filter(|t| is_non_empty_string(&t["ref"], 256));
            let Some(track) = track else {
                return Ok(transaction_error(id, "routing track identity changed after apply; undo refused"));
            };
            let reference = &track["ref"];
            if track["routing"].is_null() {
                return Err(LiveError::type_error("Cannot read properties of null (reading 'inputType')"));
            }
            let current = observed_routing(track);
            let expected = if reconciliation { &t["prior"] } else { &t["created"] };
            if prior_keys(&t).any(|key| current.get(key) != expected.get(key)) {
                return Ok(transaction_error(
                    id,
                    if reconciliation {
                        "routing undo replay did not restore prior state"
                    } else {
                        "routing changed after apply; undo refused"
                    },
                ));
            }
            if !reconciliation {
                let mut restore = json!({"ref":reference});
                for (key, value) in t["prior"].as_object().unwrap() {
                    if !(value.is_null() && key.ends_with("SubRouting")) {
                        restore[key] = value.clone();
                    }
                }
                restore["expectedObjectIdentity"] = t["payload"]["expectedObjectIdentity"].clone();
                restore["expectedStateRevision"] = json!(routing_revision(track)?);
                if routing_cycle(&snapshot, reference.as_str().unwrap(), &restore) {
                    return Ok(transaction_error(id, "routing restoration would create a feedback loop"));
                }
                record.borrow_mut()["state"] = json!("undoing");
                let result = self.invoke_undo_recovery(&record, &*adapter, "routing.set", &restore, &context).await?;
                if result.is_null() {
                    return Err(LiveError::type_error("Cannot read properties of null (reading 'changed')"));
                }
                if result["changed"] != true {
                    return Err(LiveError::error("routing restoration was not confirmed"));
                }
            }
            self.confirm_routing_fields(
                &context,
                reference,
                &t["payload"]["expectedObjectIdentity"],
                &t["prior"],
                &prior_keys(&t).collect::<Vec<_>>(),
                "routing exact prior state was not restored",
            )
            .await?;
            record.borrow_mut()["state"] = json!("undone");
            Ok(success_text(id, &json!({"transactionId":t["id"],"state":"undone","restored":t["prior"],"idempotent":false})))
        }
        .await;
        result.unwrap_or_else(|e| {
            record.borrow_mut()["state"] = json!("uncertain");
            adapter_tool_error(id, &e, "Routing undo is uncertain; inspect routing and feedback state.")
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn routing_cycles_match_source_for_transitive_routes_and_duplicate_names() {
        let fixture: Value =
            serde_json::from_str(include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/host-routing-oracle.json"))).unwrap();
        for (index, row) in fixture["graphs"].as_array().unwrap().iter().enumerate() {
            assert_eq!(
                routing_cycle(&row["snapshot"], row["target"].as_str().unwrap(), &row["proposed"]),
                row["result"].as_bool().unwrap(),
                "graph {index}"
            );
        }
    }
}
