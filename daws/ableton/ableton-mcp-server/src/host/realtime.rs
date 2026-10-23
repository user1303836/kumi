//! Bounded realtime authorization binds the endpoint to exact parameter owner and sibling identities.
use super::*;
use super::{reads::AUDITION_DEADLINE_MS, retention::TransactionRecord};
use crate::transactions::batch::unique_parameter_rows;
use kumi_common::{abort::Signal, js::json as js_json, time::now_ms_f64};
use std::collections::HashMap;
const MAX_SET_COLLECTION: usize = 10_000_000;
const OPERATIONS: [&str; 3] = ["realtime.arm", "realtime.disarm", "realtime.stats"];
const RECOVERY: &str = "live_realtime_disarm or live_session_emergency_stop remains independent of a realtime packet";
const ARM_FAILURE: &str = "Realtime arm state is uncertain; disarm through the authenticated control channel before retrying.";
fn array(v: &Value) -> &[Value] {
    v.as_array().map(Vec::as_slice).unwrap_or(&[])
}
fn add_target(available: &mut HashMap<String, Value>, reference: &str, value: &Value, details: Value, authority: Value) {
    let mut target = json!({"ref":reference,"value":if value.as_f64().is_some_and(f64::is_finite){value.clone()}else{Value::Null}});
    target.as_object_mut().unwrap().extend(details.as_object().unwrap().clone());
    target["authority"] = authority;
    available.insert(reference.into(), target);
}
fn consume(budget: &mut usize, n: usize) -> bool {
    if n > MAX_SET_COLLECTION - *budget {
        false
    } else {
        *budget += n;
        true
    }
}
fn realtime_fence(status: &LiveStatus, targets: &[Value]) -> String {
    let status = serde_json::to_value(status).unwrap();
    let mut v = device_parameter::fields(&status, &["epoch", "registryHash"]);
    v["operations"] = json!(OPERATIONS);
    v["targets"] = json!(targets);
    js_json::stringify(&v)
}
impl McpHost {
    pub fn realtime_parameter_targets(&self, snapshot: &Value, references: &[String]) -> Result<Vec<Value>, LiveError> {
        let wanted: HashSet<&str> = references.iter().map(String::as_str).collect();
        let mut available = HashMap::new();
        let mut budget = 0;
        'tracks: for track in array(&snapshot["tracks"]) {
            if wanted.iter().all(|r| available.contains_key(*r)) {
                break;
            }
            let track_ref = track["ref"].as_str().filter(|v| !v.is_empty());
            let track_identity = track["objectIdentity"].as_str().filter(|v| !v.is_empty());
            if track["mixer"].is_object() && track_ref.is_some() && track_identity.is_some() {
                let mixer = &track["mixer"];
                let mut rows = vec![];
                let mut complete = true;
                for (ref_key, identity_key, value_key, kind) in [
                    ("volumeRef", "volumeIdentity", "volume", "mixer-volume"),
                    ("panRef", "panIdentity", "pan", "mixer-pan"),
                    ("cueRef", "cueIdentity", "cueVolume", "mixer-cue"),
                ] {
                    if mixer[ref_key].is_null() {
                        if !mixer[identity_key].is_null() {
                            complete = false;
                        }
                        continue;
                    }
                    let (Some(reference), Some(identity)) = (mixer[ref_key].as_str(), mixer[identity_key].as_str()) else {
                        complete = false;
                        continue;
                    };
                    rows.push(json!({"ref":reference,"objectIdentity":identity,"value":mixer[value_key],"kind":kind}));
                }
                let refs = array(&mixer["sendRefs"]);
                let identities = array(&mixer["sendIdentities"]);
                let sends = array(&mixer["sends"]);
                if refs.len() != identities.len() {
                    complete = false;
                }
                for (index, reference) in refs.iter().enumerate() {
                    let (Some(reference), Some(identity)) = (reference.as_str(), identities.get(index).and_then(Value::as_str)) else {
                        complete = false;
                        continue;
                    };
                    rows.push(json!({"ref":reference,"objectIdentity":identity,"value":sends.get(index).unwrap_or(&Value::Null),"kind":"mixer-send","sendIndex":index}));
                }
                let holds = rows.iter().any(|r| wanted.contains(r["ref"].as_str().unwrap()));
                if holds && (!complete || rows.len() > MAX_SET_COLLECTION || !consume(&mut budget, rows.len())) {
                    break;
                }
                let siblings: Vec<_> = rows.iter().map(|r| json!({"ref":r["ref"],"objectIdentity":r["objectIdentity"]})).collect();
                if holds {
                    for row in rows {
                        let mut details = json!({"kind":row["kind"],"parameterIdentity":row["objectIdentity"],"trackRef":track_ref,"trackIdentity":track_identity});
                        if let Some(i) = row.get("sendIndex") {
                            details["sendIndex"] = i.clone();
                        }
                        add_target(
                            &mut available,
                            row["ref"].as_str().unwrap(),
                            &row["value"],
                            details,
                            json!({"ref":row["ref"],"parameterIdentity":row["objectIdentity"],"ownerRef":track_ref,"ownerIdentity":track_identity,"trackRef":track_ref,"trackIdentity":track_identity,"siblings":siblings}),
                        );
                    }
                }
            }
            // Explicit traversal stack keeps the source order without depending on the Rust call stack.
            enum Visit<'a> {
                Devices(&'a Value),
                Device(&'a Value),
                Chains(&'a Value),
                Chain(&'a Value),
                Pads(&'a Value),
                Pad(&'a Value),
            }
            let empty = json!([]);
            let devices = track.get("devices").filter(|v| !v.is_null()).unwrap_or(&empty);
            let mut pending = vec![Visit::Devices(devices)];
            while let Some(visit) = pending.pop() {
                match visit {
                    Visit::Devices(v) => {
                        let Some(rows) = v.as_array().filter(|r| r.len() <= MAX_SET_COLLECTION && r.iter().all(Value::is_object)) else {
                            break 'tracks;
                        };
                        pending.extend(rows.iter().rev().map(Visit::Device));
                    }
                    Visit::Device(device) => {
                        if !consume(&mut budget, 1) {
                            break 'tracks;
                        }
                        let parameters = array(&device["parameters"]);
                        let macros = array(&device["macros"]);
                        let holds =
                            parameters.iter().chain(macros).any(|r| r.is_object() && r["ref"].as_str().is_some_and(|r| wanted.contains(r)));
                        if holds
                            && (parameters.len() + macros.len() > MAX_SET_COLLECTION
                                || !parameters.iter().chain(macros).all(Value::is_object))
                        {
                            break 'tracks;
                        }
                        let rows = if holds {
                            unique_parameter_rows(&parameters.iter().chain(macros).cloned().collect::<Vec<_>>())
                        } else {
                            vec![]
                        };
                        if !consume(&mut budget, rows.len()) {
                            break 'tracks;
                        }
                        let siblings = rows
                            .iter()
                            .map(|r| Some(json!({"ref":r["ref"].as_str()?,"objectIdentity":r["objectIdentity"].as_str()?})))
                            .collect::<Option<Vec<_>>>();
                        if let (Some(track_ref), Some(track_identity), Some(device_ref), Some(device_identity), Some(siblings)) = (
                            track_ref,
                            track_identity,
                            device["ref"].as_str().filter(|s| !s.is_empty()),
                            device["objectIdentity"].as_str().filter(|s| !s.is_empty()),
                            siblings,
                        ) {
                            for (index, parameter) in rows.iter().enumerate() {
                                let kind = if index < parameters.len() { "device-parameter" } else { "rack-macro" };
                                let reference = parameter["ref"].as_str().unwrap();
                                let identity = &parameter["objectIdentity"];
                                let mut details = json!({"kind":kind,"parameterIdentity":identity,"deviceRef":device_ref,"deviceIdentity":device_identity,"trackRef":track_ref,"trackIdentity":track_identity});
                                if kind == "device-parameter" {
                                    for key in ["min", "max", "enabled", "automatable", "revision"] {
                                        details[key] = parameter[key].clone();
                                    }
                                }
                                add_target(
                                    &mut available,
                                    reference,
                                    &parameter["value"],
                                    details,
                                    json!({"ref":reference,"parameterIdentity":identity,"ownerRef":device_ref,"ownerIdentity":device_identity,"trackRef":track_ref,"trackIdentity":track_identity,"siblings":siblings}),
                                );
                            }
                        }
                        pending.push(Visit::Pads(&device["drumPads"]));
                        pending.push(Visit::Chains(&device["chains"]));
                    }
                    Visit::Chains(v) => {
                        let rows = array(v);
                        if rows.len() > MAX_SET_COLLECTION || !rows.iter().all(Value::is_object) {
                            break 'tracks;
                        }
                        pending.extend(rows.iter().rev().map(Visit::Chain));
                    }
                    Visit::Chain(chain) => {
                        if !consume(&mut budget, 1) {
                            break 'tracks;
                        }
                        pending.push(Visit::Devices(&chain["devices"]));
                    }
                    Visit::Pads(v) => {
                        let rows = array(v);
                        if rows.len() > MAX_SET_COLLECTION || !rows.iter().all(Value::is_object) {
                            break 'tracks;
                        }
                        pending.extend(rows.iter().rev().map(Visit::Pad));
                    }
                    Visit::Pad(pad) => {
                        if !consume(&mut budget, 1) {
                            break 'tracks;
                        }
                        pending.push(Visit::Chains(&pad["chains"]));
                    }
                }
            }
        }
        references
            .iter()
            .map(|r| {
                available
                    .get(r)
                    .cloned()
                    .ok_or_else(|| LiveError::error(format!("realtime parameter ref lacks exact authoritative identity: {r}")))
            })
            .collect()
    }
    pub async fn dispatch_realtime_tool(
        self: &Rc<Self>,
        call: &ToolCall,
        signal: Option<&Signal>,
    ) -> Option<Result<Option<Value>, LiveError>> {
        let p = call.arguments.as_ref().unwrap_or(&Value::Null);
        Some(Ok(match call.name.as_str() {
            "live_realtime_arm_preview" => Some(self.live_realtime_arm_preview_async(&call.id, p).await),
            "live_realtime_arm_apply" => self.live_realtime_arm_apply_async(&call.id, p, signal).await,
            "live_realtime_disarm" => Some(self.live_realtime_disarm_async(&call.id, p).await),
            "live_realtime_stats" => Some(self.live_realtime_stats_async(&call.id, p).await),
            _ => return None,
        }))
    }
    pub async fn live_realtime_arm_preview_async(&self, id: &Value, p: &Value) -> Value {
        let unique = |rows: &[Value]| rows.iter().enumerate().all(|(i, r)| !rows[..i].contains(r));
        let channels = p["channels"].as_array().is_some_and(|rows| {
            !rows.is_empty()
                && rows.len() <= 4
                && unique(rows)
                && rows.iter().all(|r| matches!(r.as_str(), Some("udp-json" | "osc" | "xy" | "max")))
        });
        let refs = p["parameterRefs"]
            .as_array()
            .is_some_and(|rows| rows.len() <= 32 && unique(rows) && rows.iter().all(|r| is_non_empty_string(r, 256)));
        let ports = p.get("sourcePorts").is_none_or(|v| {
            v.as_array().is_some_and(|r| r.len() <= 16 && unique(r) && r.iter().all(|r| is_integer_in_range(r, 1.0, 65535.0)))
        });
        if !has_only(p, &["ttlMs", "channels", "parameterRefs", "sourcePorts", "outputSafety"])
            || !channels
            || !refs
            || !ports
            || p.get("ttlMs").is_some_and(|v| !is_integer_in_range(v, 1000.0, 30000.0))
        {
            return error(id, -32602, "channels, parameterRefs, optional ttlMs/sourcePorts, and outputSafety are invalid", None);
        }
        let result = async {
            let status = self.fresh_status(Some(&LiveOperationContext::with_deadline(self.deadline(AUDITION_DEADLINE_MS)))).await?;
            if !status.connected || status.provenance.as_ref().map(|p| p.as_str()) != Some("real-live") {
                return Err(LiveError::error("realtime control requires authoritative real-Live provenance"));
            }
            for operation in OPERATIONS {
                if !status.operations.iter().flatten().any(|o| o == operation) {
                    return Err(LiveError::error(format!("{operation} is unavailable")));
                }
            }
            let refs: Vec<String> = array(&p["parameterRefs"]).iter().map(|v| v.as_str().unwrap().into()).collect();
            let targets = if refs.is_empty() {
                vec![]
            } else {
                self.realtime_parameter_targets(
                    &serde_json::to_value(
                        self.views
                            .view_for(
                                Some(&LiveOperationContext::with_deadline(self.deadline(AUDITION_DEADLINE_MS))),
                                array(&p["parameterRefs"]),
                                None,
                                &[],
                            )
                            .await?,
                    )
                    .unwrap(),
                    &refs,
                )?
            };
            let mut payload = json!({"ttlMs":p.get("ttlMs").cloned().unwrap_or(json!(10000)),"channels":p["channels"],"parameterRefs":refs,"targetAuthorities":targets.iter().map(|t|&t["authority"]).collect::<Vec<_>>(),"outputSafety":output_safety_of(&p["outputSafety"])});
            if let Some(ports) = p.get("sourcePorts") {
                payload["sourcePorts"] = ports.clone();
            }
            let t = json!({"id":tempo::transaction_id("realtime"),"epoch":status.epoch,"kind":"realtime-arm","fence":realtime_fence(&status,&targets),"payload":payload,"expiresAt":now_ms_f64()+TRANSACTION_TTL_MS,"state":"previewed"});
            self.retain_bounded_transaction(&self.clip_lifecycle_transactions, t.clone(), "realtime arm")?;
            Ok(success_text(id, &json!({"transactionId":t["id"],"epoch":t["epoch"],"ttlMs":payload["ttlMs"],"channels":payload["channels"],"parameterTargets":targets,"sourcePorts":payload.get("sourcePorts").cloned().unwrap_or(json!([])),"outputSafety":payload["outputSafety"],"impact":"temporarily-authorizes-bounded-realtime-control","packetLimitBytes":512,"sustainedRatePerSecond":64,"burst":16,"confirmation":"apply","expiresAt":t["expiresAt"]})))
        }
        .await;
        result.unwrap_or_else(|e| {
            adapter_tool_error(
                id,
                &e,
                "Realtime arming requires configured loopback UDP, real-Live provenance, and explicit output-safety evidence.",
            )
        })
    }
    pub async fn live_realtime_arm_apply_async(self: &Rc<Self>, id: &Value, p: &Value, signal: Option<&Signal>) -> Option<Value> {
        if !valid_transaction_params(p, "apply") {
            return Some(error(id, -32602, "transactionId, confirmation=apply, and idempotencyKey are required", None));
        }
        let Some(record) = self.clip_lifecycle_transactions.get(p["transactionId"].as_str().unwrap()) else {
            return Some(transaction_error(id, "Unknown or expired realtime-arm transaction"));
        };
        let t = record.borrow().clone();
        if t["kind"] != "realtime-arm" || (t["state"] == "previewed" && t["expiresAt"].as_f64().unwrap_or(f64::NAN) <= now_ms_f64()) {
            return Some(transaction_error(id, "Unknown or expired realtime-arm transaction"));
        }
        if t["state"] == "applied" && t["applyKey"] == p["idempotencyKey"] {
            return Some(success_text(id, &json!({"transactionId":t["id"],"state":"applied","endpoint":t["created"],"idempotent":true})));
        }
        if t["state"] == "applying" {
            let promise = self.record_operation(&record);
            if t["applyKey"] != p["idempotencyKey"] || promise.is_none() {
                return Some(transaction_error(id, "Realtime arm apply is already in progress with a different request"));
            }
            return Some(match promise.unwrap().await {
                Ok(endpoint) => success_text(
                    id,
                    &json!({"transactionId":t["id"],"state":"applied","endpoint":endpoint,"idempotent":true,"recovery":RECOVERY}),
                ),
                Err(e) => adapter_tool_error(id, &e, ARM_FAILURE),
            });
        }
        if t["state"] != "previewed" {
            return Some(transaction_error(id, "Transaction is no longer applicable"));
        }
        if signal.is_some_and(Signal::is_cancelled) {
            return None;
        }
        record.borrow_mut()["state"] = json!("applying");
        record.borrow_mut()["applyKey"] = p["idempotencyKey"].clone();
        let host = self.clone();
        let held = record.clone();
        let signal = signal.cloned();
        let promise = self.start_record_operation(&record, async move { host.dispatch_realtime_arm_apply(&held, signal.as_ref()).await });
        Some(match promise.await {
            Ok(endpoint) => success_text(
                id,
                &json!({"transactionId":t["id"],"state":"applied","endpoint":endpoint,"idempotent":false,"recovery":RECOVERY}),
            ),
            Err(e) => adapter_tool_error(id, &e, ARM_FAILURE),
        })
    }
    async fn dispatch_realtime_arm_apply(&self, record: &TransactionRecord, signal: Option<&Signal>) -> Result<Value, LiveError> {
        let result = async {
            let fresh = self.fresh_status(Some(&LiveOperationContext::with_deadline(self.deadline(AUDITION_DEADLINE_MS)))).await;
            // A source async call suspends here even when the adapter returns immediately.
            tokio::task::yield_now().await;
            let status = fresh?;
            let t = record.borrow().clone();
            let payload = &t["payload"];
            if !status.connected || status.provenance.as_ref().map(|p| p.as_str()) != Some("real-live") || json!(status.epoch) != t["epoch"] {
                return Err(LiveError::error("Live connection or provenance changed; preview again"));
            }
            let refs: Vec<String> = array(&payload["parameterRefs"]).iter().map(|v| v.as_str().unwrap().into()).collect();
            let context = |deadline| LiveOperationContext {
                signal: signal.cloned(),
                deadline_ms: Some(deadline),
                idempotency_key: t["applyKey"].as_str().map(str::to_owned),
                transaction_id: t["id"].as_str().map(str::to_owned),
            };
            let targets = if refs.is_empty() {
                vec![]
            } else {
                self.realtime_parameter_targets(
                    &serde_json::to_value(
                        self.views
                            .view_for(Some(&context(self.deadline(AUDITION_DEADLINE_MS))), array(&payload["parameterRefs"]), None, &[])
                            .await?,
                    )
                    .unwrap(),
                    &refs,
                )?
            };
            if realtime_fence(&status, &targets) != t["fence"] {
                return Err(LiveError::error("realtime control contract or parameter targets changed; preview again"));
            }
            if signal.is_some_and(Signal::is_cancelled) {
                return Err(LiveError::error("realtime arm cancelled before dispatch"));
            }
            let mut args = json!({"ttlMs":payload["ttlMs"],"channels":payload["channels"],"parameterRefs":refs,"targetAuthorities":targets.iter().map(|t|&t["authority"]).collect::<Vec<_>>(),"outputSafety":payload["outputSafety"]});
            if let Some(ports) = payload.get("sourcePorts") {
                args["sourcePorts"] = ports.clone();
            }
            let result = self
                .async_adapter()
                .invoke_async(&LiveInvocation::new("realtime.arm", args), Some(&context(self.deadline(AUDITION_DEADLINE_MS))))
                .await?;
            if result.is_null() {
                return Err(LiveError::type_error("Cannot read properties of null (reading 'port')"));
            }
            if !is_integer_in_range(&result["port"], 1.0, 65535.0)
                || !is_non_empty_string(&result["host"], 64)
                || !is_non_empty_string(&result["token"], 128)
                || !result["expiresAt"].as_f64().is_some_and(|v| v.is_finite() && v.fract() == 0.0 && v > now_ms_f64())
                || !result["channels"].is_array()
                || js_json::stringify(&result["channels"]) != js_json::stringify(&payload["channels"])
                || !result["parameterRefs"].is_array()
                || js_json::stringify(&result["parameterRefs"]) != js_json::stringify(&payload["parameterRefs"])
            {
                return Err(LiveError::error("realtime arming was not confirmed with the requested bounded endpoint and exact targets"));
            }
            record.borrow_mut()["created"] = result.clone();
            record.borrow_mut()["state"] = json!("applied");
            Ok(result)
        }
        .await;
        if let Err(e) = &result {
            record.borrow_mut()["state"] = json!(if e.message().contains("cancelled before dispatch") { "previewed" } else { "uncertain" });
        }
        result
    }
    pub async fn live_realtime_disarm_async(&self, id: &Value, p: &Value) -> Value {
        if !has_only(p, &["confirmation"]) || p["confirmation"] != "disarm" {
            return error(id, -32602, "confirmation=disarm is required", None);
        }
        let result = async {
            let status = self.fresh_status(Some(&LiveOperationContext::with_deadline(self.deadline(AUDITION_DEADLINE_MS)))).await?;
            if !status.connected || !status.operations.iter().flatten().any(|o| o == "realtime.disarm") {
                return Err(LiveError::error("realtime disarm is unavailable"));
            }
            let result = self
                .async_adapter()
                .invoke_async(
                    &LiveInvocation::new("realtime.disarm", json!({})),
                    Some(&LiveOperationContext::with_deadline(self.deadline(AUDITION_DEADLINE_MS))),
                )
                .await?;
            if result.is_null() {
                return Err(LiveError::type_error("Cannot read properties of null (reading 'armed')"));
            }
            if result["armed"] != false {
                return Err(LiveError::error("realtime disarm was not confirmed"));
            }
            Ok(success_text(id, &json!({"armed":false,"disarmed":true})))
        }
        .await;
        result.unwrap_or_else(|e| {
            adapter_tool_error(
                id,
                &e,
                "Realtime disarm failed; use the separately authorized emergency-stop path if playback may be active.",
            )
        })
    }
    pub async fn live_realtime_stats_async(&self, id: &Value, p: &Value) -> Value {
        if !has_only(p, &[]) {
            return error(id, -32602, "no arguments accepted", None);
        }
        let result = async {
            let status = self.fresh_status(Some(&LiveOperationContext::with_deadline(self.deadline(AUDITION_DEADLINE_MS)))).await?;
            if !status.connected || !status.operations.iter().flatten().any(|o| o == "realtime.stats") {
                return Err(LiveError::error("realtime stats are unavailable"));
            }
            let result = self
                .async_adapter()
                .invoke_async(
                    &LiveInvocation::new("realtime.stats", json!({})),
                    Some(&LiveOperationContext::with_deadline(self.deadline(AUDITION_DEADLINE_MS))),
                )
                .await?;
            Ok(success_text(id, &result))
        }
        .await;
        result.unwrap_or_else(|e| adapter_tool_error(id, &e, "Realtime stats require the configured loopback control plane."))
    }
}
