//! Exact device/parameter hierarchy reads and transactions.
use super::reads::AUDITION_DEADLINE_MS;
use super::*;
use crate::transactions::batch;
use kumi_common::js::json as js_json;

pub(super) struct DeviceRow {
    pub track: Value,
    pub device: Value,
    pub owner_ref: String,
    pub owner_identity: String,
    pub siblings: Vec<Value>,
}
pub(super) struct ParameterTarget {
    pub device: Value,
    pub parameter: Value,
    pub track_ref: Value,
    pub authority: Value,
}
pub(super) struct ParameterState {
    pub parameter: Value,
    pub same: bool,
}
fn rows(value: &Value) -> impl Iterator<Item = &Value> {
    value.as_array().into_iter().flatten()
}
pub(super) fn fields(value: &Value, keys: &[&str]) -> Value {
    Value::Object(keys.iter().filter_map(|key| value.get(*key).map(|v| ((*key).into(), v.clone()))).collect())
}
pub(super) fn authority_fields(value: &Value, keys: &[&str]) -> Result<Value, LiveError> {
    if keys.iter().any(|key| value.get(*key).is_none()) {
        return Err(LiveError::error("mutation authority contains an unsupported value"));
    }
    Ok(fields(value, keys))
}
impl McpHost {
    pub(super) fn device_row(&self, snapshot: &LiveSnapshot, reference: &str) -> Result<DeviceRow, LiveError> {
        fn siblings(values: &Value) -> Result<Option<Vec<Value>>, LiveError> {
            let Some(values) = values.as_array() else { return Ok(None) };
            values
                .iter()
                .map(|value| {
                    if !value.is_object() || !value["ref"].is_string() || !value["objectIdentity"].is_string() {
                        return Err(LiveError::error("device sibling identity is unavailable"));
                    }
                    Ok(fields(value, &["ref", "objectIdentity"]))
                })
                .collect::<Result<Vec<_>, _>>()
                .map(Some)
        }
        // The reference is passed separately to keep recursion scoped to the selected tree. Only the row found is
        // copied: a big rack's devices are megabytes (#174).
        fn walk(
            values: &Value,
            owner_ref: &str,
            owner_identity: &str,
            reference: &str,
        ) -> Result<Option<(Value, String, String, Vec<Value>)>, LiveError> {
            let Some(siblings) = siblings(values)? else { return Ok(None) };
            for value in rows(values) {
                if value["ref"] == reference {
                    return Ok(Some((value.clone(), owner_ref.into(), owner_identity.into(), siblings)));
                }
                for chain in rows(&value["chains"]).chain(rows(&value["drumPads"]).flat_map(|pad| rows(&pad["chains"]))) {
                    if let (Some(reference_), Some(identity)) = (chain["ref"].as_str(), chain["objectIdentity"].as_str()) {
                        if let Some(found) = walk(&chain["devices"], reference_, identity, reference)? {
                            return Ok(Some(found));
                        }
                    }
                }
            }
            Ok(None)
        }
        let snapshot = serde_json::to_value(snapshot).unwrap();
        for track in rows(&snapshot["tracks"]) {
            if let (Some(owner), Some(identity)) = (track["ref"].as_str(), track["objectIdentity"].as_str()) {
                if let Some((device, owner_ref, owner_identity, siblings)) = walk(&track["devices"], owner, identity, reference)? {
                    return Ok(DeviceRow { track: track.clone(), device, owner_ref, owner_identity, siblings });
                }
            }
        }
        Err(LiveError::error("device reference is not authoritative"))
    }
    pub(super) async fn discover_one_async(
        &self,
        context: Option<&LiveOperationContext>,
        kind: LiveDiscoveryKind,
        reference: &str,
        fields: Option<&[&str]>,
        parent: Option<&str>,
    ) -> Result<Option<Value>, LiveError> {
        let mut request = LiveDiscoveryRequest::of(kind);
        request.filter = Some(json!({"ref":reference}).as_object().unwrap().clone());
        request.limit = Some(1);
        request.fields = fields.map(|v| v.iter().map(|s| (*s).into()).collect());
        request.parent = parent.map(str::to_owned);
        let fallback = LiveOperationContext::with_deadline(self.deadline(AUDITION_DEADLINE_MS));
        let page = self.async_adapter().discover_async(&request, Some(context.unwrap_or(&fallback))).await?;
        Ok(page.items.into_iter().find(|item| item.get("ref").and_then(Value::as_str) == Some(reference)).map(Value::Object))
    }
    pub(super) async fn track_one_async(
        &self,
        context: Option<&LiveOperationContext>,
        reference: &str,
        fields: &[&str],
    ) -> Result<Option<Value>, LiveError> {
        for kind in [LiveDiscoveryKind::Track, LiveDiscoveryKind::ReturnTrack, LiveDiscoveryKind::MainTrack] {
            if let Some(found) = self.discover_one_async(context, kind, reference, Some(fields), None).await? {
                return Ok(Some(found));
            }
        }
        Ok(None)
    }
    pub(super) fn parameter_target(
        &self,
        snapshot: &LiveSnapshot,
        device: &str,
        parameter: &str,
    ) -> Result<(Value, Value, Value), LiveError> {
        let snapshot = serde_json::to_value(snapshot).unwrap();
        let target = batch::parameter_target(&snapshot, device, parameter)
            .map_err(|_| LiveError::error("device and parameter references are not authoritative children"))?;
        Ok((target.device.clone(), target.parameter.clone(), target.track["ref"].clone()))
    }
    pub(super) fn parameter_authority(&self, snapshot: &LiveSnapshot, reference: &str) -> Result<Value, LiveError> {
        let error = || LiveError::error("parameter lacks complete exact hierarchy authority");
        let targets = self.realtime_parameter_targets(&serde_json::to_value(snapshot).unwrap(), &[reference.to_string()])?;
        let authority = targets.first().and_then(|t| t.get("authority")).cloned().ok_or_else(error)?;
        if authority["ref"] != reference
            || ["parameterIdentity", "ownerRef", "ownerIdentity", "trackRef", "trackIdentity"]
                .iter()
                .any(|key| !is_non_empty_string(&authority[*key], 256))
            || !authority["siblings"].is_array()
        {
            return Err(error());
        }
        Ok(authority)
    }
    pub(super) async fn parameter_rows_async(
        &self,
        context: Option<&LiveOperationContext>,
        device: &str,
        parameters: &[String],
    ) -> Result<Vec<Value>, LiveError> {
        let listed = if parameters.len() > 4 {
            let mut request = LiveDiscoveryRequest::of(LiveDiscoveryKind::Parameter);
            request.parent = Some(device.into());
            request.limit = Some(1024);
            let fallback = LiveOperationContext::with_deadline(self.deadline(AUDITION_DEADLINE_MS));
            Some(self.views.discover_all(&request, Some(context.unwrap_or(&fallback)), None).await?)
        } else {
            None
        };
        let mut found = Vec::new();
        for reference in parameters {
            let row = if let Some(listed) = &listed {
                listed.iter().rev().find(|r| r.get("ref").and_then(Value::as_str) == Some(reference.as_str())).cloned().map(Value::Object)
            } else {
                self.discover_one_async(context, LiveDiscoveryKind::Parameter, reference, None, Some(device)).await?
            };
            let row = row.ok_or_else(|| LiveError::error("device and parameter references are not authoritative children"))?;
            if row.get("parentRef").is_some_and(|p| p != device) || !is_non_empty_string(&row["objectIdentity"], 256) {
                return Err(LiveError::error("device and parameter references are not authoritative children"));
            }
            found.push(row);
        }
        Ok(found)
    }
    pub(super) async fn parameter_targets_async(
        &self,
        context: Option<&LiveOperationContext>,
        device_ref: &str,
        parameters: &[String],
    ) -> Result<Vec<ParameterTarget>, LiveError> {
        let parameters = self.parameter_rows_async(context, device_ref, parameters).await?;
        let device = self
            .discover_one_async(
                context,
                LiveDiscoveryKind::Device,
                device_ref,
                Some(&["ref", "parentRef", "objectIdentity", "name", "kind", "enabled"]),
                None,
            )
            .await?;
        let parent = device.as_ref().and_then(|d| d["parentRef"].as_str());
        let track_ref = if let Some(parent) = parent.filter(|p| ref_kind(p) == Some("track")) {
            Some(parent.into())
        } else {
            track_index_of_ref(device_ref).map(|i| format!("{}:track:{i}", device_ref.split(':').next().unwrap_or("")))
        };
        let track = if let Some(reference) = track_ref {
            self.track_one_async(context, &reference, &["ref", "objectIdentity", "name", "kind"]).await?
        } else {
            None
        };
        let (Some(device), Some(track)) = (device, track) else {
            return Err(LiveError::error("device and parameter references are not authoritative children"));
        };
        parameters
            .into_iter()
            .map(|parameter| {
                let authority = json!({"ref":parameter["ref"],"parameterIdentity":parameter["objectIdentity"],"ownerRef":device["ref"],"ownerIdentity":device["objectIdentity"],"trackRef":track["ref"],"trackIdentity":track["objectIdentity"]});
                if authority.as_object().unwrap().values().any(|v| !is_non_empty_string(v, 256)) {
                    return Err(LiveError::error("parameter lacks complete exact hierarchy authority"));
                }
                Ok(ParameterTarget { device: device.clone(), parameter, track_ref: track["ref"].clone(), authority })
            })
            .collect()
    }
    pub(super) async fn parameter_state_async(
        &self,
        context: &LiveOperationContext,
        device: &str,
        items: &[Value],
        owner: bool,
    ) -> Result<Vec<ParameterState>, LiveError> {
        if items.iter().all(|i| i["authority"]["siblings"].as_array().is_none_or(Vec::is_empty)) {
            let references = items.iter().map(|i| i["ref"].as_str().unwrap_or("").into()).collect::<Vec<_>>();
            let parameters = self.parameter_rows_async(Some(context), device, &references).await?;
            let device_row = if owner {
                self.discover_one_async(Some(context), LiveDiscoveryKind::Device, device, Some(&["ref", "objectIdentity"]), None).await?
            } else {
                None
            };
            return Ok(parameters
                .into_iter()
                .enumerate()
                .map(|(i, parameter)| {
                    let authority = &items[i]["authority"];
                    let same = parameter["objectIdentity"] == authority["parameterIdentity"]
                        && authority["ownerRef"] == device
                        && (!owner || device_row.as_ref().is_some_and(|d| d["objectIdentity"] == authority["ownerIdentity"]));
                    ParameterState { parameter, same }
                })
                .collect());
        }
        let references = std::iter::once(json!(device)).chain(items.iter().map(|i| i["ref"].clone())).collect::<Vec<_>>();
        let snapshot = self.views.view_for(Some(context), &references, None, &[]).await?;
        items
            .iter()
            .map(|item| {
                let reference = item["ref"].as_str().unwrap_or("");
                let (_, parameter, _) = self.parameter_target(&snapshot, device, reference)?;
                let same = js_json::stringify(&self.parameter_authority(&snapshot, reference)?) == js_json::stringify(&item["authority"]);
                Ok(ParameterState { parameter, same })
            })
            .collect()
    }
}

use super::mutations::{parameter_mutation_args, parameters_mutation_args};
use base64::Engine;
use kumi_common::abort::Signal;
use rand::RngCore;
const PREVIEW_FAILURE: &str = "Parameter preview failed without mutation; discover an enabled published numeric parameter and retry.";
const MULTI_PREVIEW_FAILURE: &str =
    "Parameter preview failed without mutation; discover enabled published numeric parameters of one device and retry.";
const APPLY_FAILURE: &str = "Device-parameter apply may be uncertain; perform fresh authoritative discovery and do not retry blindly.";
const UNDO_FAILURE: &str = "Device-parameter undo is uncertain; inspect authoritative parameter state.";
fn number(value: &Value) -> f64 {
    value.as_f64().unwrap_or(f64::NAN)
}
fn revision(parameter: &Value) -> f64 {
    batch::parameter_revision(parameter)
}
fn numeric_same(left: &Value, right: &Value) -> bool {
    if left.is_number() && right.is_number() {
        left.as_f64() == right.as_f64()
    } else {
        left == right
    }
}
fn confirmation() -> String {
    let mut bytes = [0u8; 24];
    rand::rng().fill_bytes(&mut bytes);
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}
fn valid_preview(params: &Value) -> bool {
    has_only(params, &["deviceRef", "parameterRef", "value"])
        && is_non_empty_string(&params["deviceRef"], 256)
        && is_non_empty_string(&params["parameterRef"], 256)
        && params["value"].as_f64().is_some_and(f64::is_finite)
}
fn valid_apply(params: &Value) -> bool {
    has_only(params, &["transactionId", "confirmation", "idempotencyKey"])
        && is_non_empty_string(&params["transactionId"], 128)
        && is_non_empty_string(&params["confirmation"], 128)
        && is_idempotency_key(&params["idempotencyKey"])
}
fn shown_device(target: &ParameterTarget) -> Value {
    let mut value = fields(&target.device, &["ref", "name", "kind"]);
    value["trackRef"] = target.track_ref.clone();
    value["enabled"] = json!(target.device["enabled"] != false);
    value
}
fn value_text(value: Option<&Value>) -> String {
    match value {
        None => "undefined".into(),
        Some(Value::String(s)) => s.clone(),
        Some(Value::Object(_)) => "[object Object]".into(),
        Some(Value::Array(values)) => {
            values.iter().map(|v| if v.is_null() { String::new() } else { value_text(Some(v)) }).collect::<Vec<_>>().join(",")
        }
        Some(value) => js_json::stringify(value),
    }
}
fn shown_parameters(transaction: &Value) -> Vec<Value> {
    transaction["parameters"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|p| {
            let mut out = fields(p, &["ref"]);
            out["value"] = p["proposedValue"].clone();
            if let Some(value) = p.get("appliedDisplay") {
                out["displayValue"] = value.clone();
            }
            if let Some(value) = p.get("appliedRevision") {
                out["revision"] = value.clone();
            }
            out
        })
        .collect()
}
fn single_result(transaction: &Value, value: Value, epoch: bool, idempotent: bool) -> Value {
    let mut out = json!({"transactionId":transaction["id"],"state":"applied","value":value});
    if let Some(display) = transaction.get("appliedDisplay") {
        out["displayValue"] = display.clone();
    }
    if let Some(revision) = transaction.get("appliedRevision") {
        out["revision"] = revision.clone();
    }
    if epoch {
        out["epoch"] = transaction["epoch"].clone();
    }
    out["idempotent"] = json!(idempotent);
    out
}
fn singleton(transaction: &Value) -> Vec<Value> {
    vec![json!({"ref":transaction["parameterRef"],"authority":transaction["authority"]})]
}
impl McpHost {
    pub async fn dispatch_device_parameter_tool(&self, call: &ToolCall, signal: Option<&Signal>) -> Option<Result<Value, LiveError>> {
        let params = call.arguments.as_ref().unwrap_or(&Value::Null);
        Some(Ok(match (call.name.as_str(), call.asynchronous) {
            ("live_device_parameter_preview", true) => self.live_device_parameter_preview_async(&call.id, params).await,
            ("live_device_parameter_preview", false) => self.live_device_parameter_preview(&call.id, params),
            ("live_device_parameter_apply", true) => self.live_device_parameter_apply_async(&call.id, params, signal).await,
            ("live_device_parameter_apply", false) => self.live_device_parameter_apply(&call.id, params),
            _ => return None,
        }))
    }
    fn make_single_parameter_preview(
        &self,
        id: &Value,
        params: &Value,
        status: &LiveStatus,
        target: ParameterTarget,
    ) -> Result<Value, LiveError> {
        if target.parameter["enabled"] == false {
            return Err(LiveError::error("parameter is greyed out in Live right now"));
        }
        let proposed = fit_parameter_value(number(&params["value"]), &target.parameter);
        let revision = revision(&target.parameter);
        let mut transaction = json!({"id":tempo::transaction_id("parameter"),"confirmation":confirmation(),"epoch":status.epoch,"deviceRef":target.device["ref"],"parameterRef":target.parameter["ref"],"authority":target.authority,"proposedValue":proposed,"priorRevision":revision,"expiresAt":kumi_common::time::now_ms_f64()+TRANSACTION_TTL_MS,"state":"previewed"});
        if let Some(value) = target.parameter.get("value") {
            transaction["priorValue"] = value.clone();
        }
        self.device_parameter_transactions.insert(transaction["id"].as_str().unwrap(), transaction.clone())?;
        let mut parameter = fields(&target.parameter, &["ref", "name", "min", "max", "automatable"]);
        if let Some(value) = target.parameter.get("value") {
            parameter["currentValue"] = value.clone();
        }
        parameter["proposedValue"] = json!(proposed);
        parameter["quantization"] = target.parameter.get("quantization").filter(|v| !v.is_null()).cloned().unwrap_or(json!(0));
        parameter["enabled"] = json!(target.parameter["enabled"] != false);
        parameter["displayValue"] = target
            .parameter
            .get("displayValue")
            .filter(|v| !v.is_null())
            .cloned()
            .unwrap_or_else(|| json!(value_text(target.parameter.get("value"))));
        parameter["revision"] = json!(revision);
        Ok(success_text(
            id,
            &json!({"transactionId":transaction["id"],"epoch":transaction["epoch"],"device":shown_device(&target),"parameter":parameter,"impact":"changes-one-published-device-parameter","confirmation":transaction["confirmation"],"expiresAt":transaction["expiresAt"]}),
        ))
    }
    pub fn live_device_parameter_preview(&self, id: &Value, params: &Value) -> Value {
        if !valid_preview(params) {
            return error(id, -32602, "deviceRef, parameterRef, and finite value are required", None);
        }
        let result = (|| {
            let status = self.require_connected(Some("device.parameter.write"))?;
            let snapshot = self.adapter.snapshot()?;
            let (device, parameter, track_ref) =
                self.parameter_target(&snapshot, params["deviceRef"].as_str().unwrap(), params["parameterRef"].as_str().unwrap())?;
            let authority = self.parameter_authority(&snapshot, parameter["ref"].as_str().unwrap_or(""))?;
            self.make_single_parameter_preview(id, params, &status, ParameterTarget { device, parameter, track_ref, authority })
        })();
        result.unwrap_or_else(|cause| adapter_tool_error(id, &cause, PREVIEW_FAILURE))
    }
    pub async fn live_device_parameter_preview_async(&self, id: &Value, params: &Value) -> Value {
        if params.is_object() && params.get("values").is_some() {
            return self.live_device_parameters_preview_async(id, params).await;
        }
        if !valid_preview(params) {
            return error(id, -32602, "deviceRef, parameterRef, and finite value are required", None);
        }
        let result = async {
            let status = self.require_connected(Some("device.parameter.write"))?;
            let mut targets = self
                .parameter_targets_async(None, params["deviceRef"].as_str().unwrap(), &[params["parameterRef"].as_str().unwrap().into()])
                .await?;
            let target = targets.pop().ok_or_else(|| LiveError::error("device and parameter references are not authoritative children"))?;
            self.make_single_parameter_preview(id, params, &status, target)
        }
        .await;
        result.unwrap_or_else(|cause| adapter_tool_error(id, &cause, PREVIEW_FAILURE))
    }
    pub async fn live_device_parameters_preview_async(&self, id: &Value, params: &Value) -> Value {
        if !has_only(params, &["deviceRef", "values"])
            || !is_non_empty_string(&params["deviceRef"], 256)
            || !params["values"].as_array().is_some_and(|values| {
                (1..=10000).contains(&values.len())
                    && values.iter().all(|item| {
                        has_only(item, &["parameterRef", "value"])
                            && is_non_empty_string(&item["parameterRef"], 256)
                            && item["value"].as_f64().is_some_and(f64::is_finite)
                    })
            })
        {
            return error(id, -32602, "deviceRef and 1 to 10000 values, each a parameterRef and a finite value, are required", None);
        }
        let requested = params["values"].as_array().unwrap();
        let refs = requested.iter().map(|i| i["parameterRef"].as_str().unwrap().to_owned()).collect::<Vec<_>>();
        if refs.iter().collect::<std::collections::HashSet<_>>().len() != refs.len() {
            return error(id, -32602, "each parameter takes one value", None);
        }
        let result = async {
            let status = self.require_connected(Some("device.parameter.write"))?;
            if !status.has_operation("device.parameters.set") {
                return Err(LiveError::error("parameter changes on several parameters at once are unavailable"));
            }
            let targets = self.parameter_targets_async(None, params["deviceRef"].as_str().unwrap(), &refs).await?;
            let mut parameters: Vec<Value> = Vec::new();
            let mut shown = Vec::new();
            for (index, item) in requested.iter().enumerate() {
                let target = &targets[index];
                if target.parameter["enabled"] == false {
                    return Err(LiveError::error(format!(
                        "parameter “{}” is greyed out in Live right now",
                        kumi_common::js::string::head(&value_text(target.parameter.get("name")), 64)
                    )));
                }
                let value = fit_parameter_value(number(&item["value"]), &target.parameter);
                let mut authority = target.authority.clone();
                authority["ref"] = Value::Null;
                authority["parameterIdentity"] = Value::Null;
                if let Some(first) = parameters.first() {
                    let mut prior = first["authority"].clone();
                    prior["ref"] = Value::Null;
                    prior["parameterIdentity"] = Value::Null;
                    if js_json::stringify(&authority) != js_json::stringify(&prior) {
                        return Err(LiveError::error("parameter changes must all be on one device"));
                    }
                }
                let mut parameter = json!({"ref":target.parameter["ref"],"authority":target.authority,"proposedValue":value,"priorRevision":revision(&target.parameter)});
                if let Some(value) = target.parameter.get("value") {
                    parameter["priorValue"] = value.clone();
                }
                parameters.push(parameter);
                let mut row = fields(&target.parameter, &["ref", "name", "min", "max"]);
                if let Some(current) = target.parameter.get("value") {
                    row["currentValue"] = current.clone();
                }
                row["proposedValue"] = json!(value);
                if let Some(display) = target.parameter.get("displayValue").filter(|v| v.is_string()) {
                    row["displayValue"] = display.clone();
                }
                shown.push(row);
            }
            let transaction = json!({"id":tempo::transaction_id("parameters"),"confirmation":confirmation(),"epoch":status.epoch,"deviceRef":targets[0].device["ref"],"parameters":parameters,"expiresAt":kumi_common::time::now_ms_f64()+TRANSACTION_TTL_MS,"state":"previewed"});
            self.device_parameters_transactions.insert(transaction["id"].as_str().unwrap(), transaction.clone())?;
            Ok(success_text(id, &json!({"transactionId":transaction["id"],"epoch":transaction["epoch"],"device":shown_device(&targets[0]),"parameters":shown,"confirmation":transaction["confirmation"],"expiresAt":transaction["expiresAt"]})))
        }
        .await;
        result.unwrap_or_else(|cause| adapter_tool_error(id, &cause, MULTI_PREVIEW_FAILURE))
    }
    fn parameter_apply_record(
        &self,
        id: &Value,
        params: &Value,
        multi: bool,
        asynchronous: bool,
    ) -> Result<(retention::TransactionRecord, bool), Value> {
        let map = if multi { &self.device_parameters_transactions } else { &self.device_parameter_transactions };
        let Some(record) = map.get(params["transactionId"].as_str().unwrap_or("")) else {
            return Err(transaction_error(id, "Unknown or expired device-parameter transaction"));
        };
        let transaction = record.borrow().clone();
        if params["confirmation"] != transaction["confirmation"] {
            return Err(transaction_error(id, "Device-parameter confirmation token is invalid"));
        }
        if transaction["state"] == "applied" && transaction["applyKey"] == params["idempotencyKey"] {
            return Err(success_text(
                id,
                &if multi {
                    json!({"transactionId":transaction["id"],"state":"applied","parameters":shown_parameters(&transaction),"epoch":transaction["epoch"],"idempotent":true})
                } else {
                    single_result(&transaction, transaction["proposedValue"].clone(), false, true)
                },
            ));
        }
        let reconciliation = asynchronous && transaction["state"] == "uncertain" && transaction["applyKey"] == params["idempotencyKey"];
        if transaction["state"] == "uncertain" && !reconciliation {
            return Err(transaction_error(
                id,
                if asynchronous {
                    "Device-parameter state is uncertain; reconcile with the exact original idempotency key"
                } else {
                    "Device-parameter state is uncertain; perform fresh discovery before retrying"
                },
            ));
        }
        if (transaction["state"] != "previewed" && !reconciliation)
            || (transaction["state"] == "previewed"
                && transaction["expiresAt"].as_f64().is_some_and(|t| t <= kumi_common::time::now_ms_f64()))
        {
            return Err(transaction_error(id, "Device-parameter preview expired or is no longer applicable"));
        }
        Ok((record, reconciliation))
    }
    pub fn live_device_parameter_apply(&self, id: &Value, params: &Value) -> Value {
        if !valid_apply(params) {
            return error(id, -32602, "transactionId, confirmation token, and idempotencyKey are required", None);
        }
        let (record, _) = match self.parameter_apply_record(id, params, false, false) {
            Ok(found) => found,
            Err(reply) => return reply,
        };
        let transaction = record.borrow().clone();
        let device = transaction["deviceRef"].as_str().unwrap();
        let reference = transaction["parameterRef"].as_str().unwrap();
        let result = (|| {
            let status = self.require_connected(Some("device.parameter.write"))?;
            if json!(status.epoch) != transaction["epoch"] {
                return Ok(transaction_error(id, "Live connection epoch changed; preview again"));
            }
            let snapshot = self.adapter.snapshot()?;
            let (_, target, _) = self.parameter_target(&snapshot, device, reference)?;
            if revision(&target) != number(&transaction["priorRevision"])
                || !numeric_same(&target["value"], &transaction["priorValue"])
                || js_json::stringify(&self.parameter_authority(&snapshot, reference)?) != js_json::stringify(&transaction["authority"])
            {
                return Ok(transaction_error(id, "Device parameter identity or value changed since preview"));
            }
            self.adapter.invoke(&LiveInvocation::new(
                "device.parameter.set",
                parameter_mutation_args(&transaction, number(&transaction["proposedValue"]), number(&transaction["priorRevision"])),
            ))?;
            let snapshot = self.adapter.snapshot()?;
            let (_, verified, _) = self.parameter_target(&snapshot, device, reference)?;
            if !same_live_value(verified.get("value"), transaction.get("proposedValue"))
                || revision(&verified) <= number(&transaction["priorRevision"])
                || js_json::stringify(&self.parameter_authority(&snapshot, reference)?) != js_json::stringify(&transaction["authority"])
            {
                record.borrow_mut()["state"] = json!("uncertain");
                return Err(LiveError::error("Live did not confirm the requested exact device parameter"));
            }
            {
                let mut record = record.borrow_mut();
                record["appliedRevision"] = json!(revision(&verified));
                record["applyKey"] = params["idempotencyKey"].clone();
                record["state"] = json!("applied");
                if let Some(display) = verified.get("displayValue").filter(|v| v.is_string()) {
                    record["appliedDisplay"] = display.clone();
                }
            }
            Ok(success_text(id, &single_result(&record.borrow(), verified["value"].clone(), true, false)))
        })();
        result.unwrap_or_else(|cause| adapter_tool_error(id, &cause, APPLY_FAILURE))
    }
    pub async fn live_device_parameter_apply_async(&self, id: &Value, params: &Value, signal: Option<&Signal>) -> Value {
        if !valid_apply(params) {
            return error(id, -32602, "transactionId, confirmation token, and idempotencyKey are required", None);
        }
        if params["transactionId"].as_str().unwrap().starts_with("parameters_") {
            return self.live_device_parameters_apply_async(id, params, signal).await;
        }
        let (record, reconciliation) = match self.parameter_apply_record(id, params, false, true) {
            Ok(found) => found,
            Err(reply) => return reply,
        };
        let transaction = record.borrow().clone();
        let device = transaction["deviceRef"].as_str().unwrap();
        let items = singleton(&transaction);
        let result = async {
            if reconciliation {
                self.fresh_status(Some(&LiveOperationContext::with_deadline(self.deadline(AUDITION_DEADLINE_MS)))).await?;
            }
            let status = self.require_connected(Some("device.parameter.write"))?;
            if json!(status.epoch) != transaction["epoch"] {
                return Ok(transaction_error(id, "Live connection epoch changed; preview again"));
            }
            let adapter = self.async_adapter();
            let context = self.transaction_context(params, signal, AUDITION_DEADLINE_MS);
            let current = self.parameter_state_async(&context, device, &items, true).await?;
            let revision = if reconciliation { number(&transaction["priorRevision"]) } else { revision(&current[0].parameter) };
            if !reconciliation
                && (revision != number(&transaction["priorRevision"])
                    || !numeric_same(&current[0].parameter["value"], &transaction["priorValue"])
                    || !current[0].same)
            {
                return Ok(transaction_error(id, "Device parameter identity or value changed since preview"));
            }
            {
                let mut record = record.borrow_mut();
                record["state"] = json!("applying");
                record["applyKey"] = params["idempotencyKey"].clone();
            }
            adapter
                .invoke_async(
                    &LiveInvocation::new(
                        "device.parameter.set",
                        parameter_mutation_args(&transaction, number(&transaction["proposedValue"]), revision),
                    ),
                    Some(&context),
                )
                .await?;
            let reads = self.parameter_state_async(&context, device, &items, false).await?;
            let verified = &reads[0].parameter;
            let mut proposed = transaction["proposedValue"].clone();
            if !same_live_value(verified.get("value"), Some(&proposed)) {
                if let Some(kept) = whole_number_live_kept(&verified["value"], number(&proposed), verified) {
                    proposed = json!(kept);
                    record.borrow_mut()["proposedValue"] = proposed.clone();
                }
            }
            if !same_live_value(verified.get("value"), Some(&proposed)) || batch::parameter_revision(verified) <= revision || !reads[0].same
            {
                record.borrow_mut()["state"] = json!("uncertain");
                return Err(LiveError::error("Live did not confirm the requested exact device parameter"));
            }
            {
                let mut record = record.borrow_mut();
                record["appliedRevision"] = json!(batch::parameter_revision(verified));
                record["applyKey"] = params["idempotencyKey"].clone();
                record["state"] = json!("applied");
                if let Some(display) = verified.get("displayValue").filter(|v| v.is_string()) {
                    record["appliedDisplay"] = display.clone();
                }
            }
            Ok(success_text(id, &single_result(&record.borrow(), verified["value"].clone(), true, false)))
        }
        .await;
        result.unwrap_or_else(|cause: LiveError| {
            self.parameter_apply_failed(&record, &cause);
            adapter_tool_error(id, &cause, APPLY_FAILURE)
        })
    }
    fn parameter_apply_failed(&self, record: &retention::TransactionRecord, cause: &LiveError) {
        let mut record = record.borrow_mut();
        if record["state"] == "applying" {
            let cancelled = cause.message().contains("cancelled before dispatch");
            record["state"] = json!(if cancelled { "previewed" } else { "uncertain" });
            if cancelled {
                record.as_object_mut().unwrap().remove("applyKey");
            }
        }
    }
    pub async fn live_device_parameters_apply_async(&self, id: &Value, params: &Value, signal: Option<&Signal>) -> Value {
        let (record, reconciliation) = match self.parameter_apply_record(id, params, true, true) {
            Ok(found) => found,
            Err(reply) => return reply,
        };
        let transaction = record.borrow().clone();
        let device = transaction["deviceRef"].as_str().unwrap();
        let items = transaction["parameters"].as_array().unwrap();
        let result = async {
            if reconciliation {
                self.fresh_status(Some(&LiveOperationContext::with_deadline(self.deadline(AUDITION_DEADLINE_MS)))).await?;
            }
            let status = self.require_connected(Some("device.parameter.write"))?;
            if json!(status.epoch) != transaction["epoch"] {
                return Ok(transaction_error(id, "Live connection epoch changed; preview again"));
            }
            let adapter = self.async_adapter();
            let context = self.transaction_context(params, signal, AUDITION_DEADLINE_MS);
            if !reconciliation {
                let current = self.parameter_state_async(&context, device, items, true).await?;
                for (index, item) in items.iter().enumerate() {
                    if revision(&current[index].parameter) != number(&item["priorRevision"])
                        || !numeric_same(&current[index].parameter["value"], &item["priorValue"])
                        || !current[index].same
                    {
                        return Ok(transaction_error(id, "Device parameter identity or value changed after preview; preview again"));
                    }
                }
            }
            {
                let mut record = record.borrow_mut();
                record["state"] = json!("applying");
                record["applyKey"] = params["idempotencyKey"].clone();
            }
            adapter
                .invoke_async(
                    &LiveInvocation::new(
                        "device.parameters.set",
                        parameters_mutation_args(&transaction, |p, _| (number(&p["proposedValue"]), number(&p["priorRevision"])))?,
                    ),
                    Some(&context),
                )
                .await?;
            let reads = self.parameter_state_async(&context, device, items, false).await?;
            for (index, item) in items.iter().enumerate() {
                let verified = &reads[index].parameter;
                let mut proposed = item["proposedValue"].clone();
                if !same_live_value(verified.get("value"), Some(&proposed)) {
                    if let Some(kept) = whole_number_live_kept(&verified["value"], number(&proposed), verified) {
                        proposed = json!(kept);
                        record.borrow_mut()["parameters"][index]["proposedValue"] = proposed.clone();
                    }
                }
                if !same_live_value(verified.get("value"), Some(&proposed))
                    || revision(verified) <= number(&item["priorRevision"])
                    || !reads[index].same
                {
                    record.borrow_mut()["state"] = json!("uncertain");
                    return Err(LiveError::error("Live did not confirm the parameter changes"));
                }
                let mut record = record.borrow_mut();
                record["parameters"][index]["appliedRevision"] = json!(revision(verified));
                if let Some(display) = verified.get("displayValue").filter(|v| v.is_string()) {
                    record["parameters"][index]["appliedDisplay"] = display.clone();
                }
            }
            record.borrow_mut()["state"] = json!("applied");
            let record = record.borrow();
            Ok(success_text(id, &json!({"transactionId":record["id"],"state":"applied","parameters":shown_parameters(&record),"epoch":record["epoch"],"idempotent":false})))
        }
        .await;
        result.unwrap_or_else(|cause: LiveError| {
            self.parameter_apply_failed(&record, &cause);
            adapter_tool_error(id, &cause, APPLY_FAILURE)
        })
    }
    pub fn undo_device_parameter(&self, id: &Value, params: &Value) -> Value {
        let Some(record) = params["transactionId"].as_str().and_then(|id| self.device_parameter_transactions.get(id)) else {
            return transaction_error(id, "Device-parameter state is uncertain; read authoritative parameter state before undo");
        };
        let transaction = record.borrow().clone();
        if transaction["state"] == "uncertain" {
            return transaction_error(id, "Device-parameter state is uncertain; read authoritative parameter state before undo");
        }
        if transaction["state"] != "applied" || transaction.get("appliedRevision").is_none() {
            return transaction_error(id, "Only an applied device-parameter transaction can be undone");
        }
        if transaction["undoKey"] == params["idempotencyKey"] {
            return success_text(id, &json!({"transactionId":transaction["id"],"state":"undone","idempotent":true}));
        }
        let device = transaction["deviceRef"].as_str().unwrap();
        let reference = transaction["parameterRef"].as_str().unwrap();
        let result = (|| {
            let status = self.require_connected(Some("device.parameter.write"))?;
            if json!(status.epoch) != transaction["epoch"] {
                return Ok(transaction_error(id, "Live connection epoch changed; undo refused"));
            }
            let snapshot = self.adapter.snapshot()?;
            let (_, current, _) = self.parameter_target(&snapshot, device, reference)?;
            if !same_live_value(current.get("value"), transaction.get("proposedValue"))
                || revision(&current) != number(&transaction["appliedRevision"])
                || js_json::stringify(&self.parameter_authority(&snapshot, reference)?) != js_json::stringify(&transaction["authority"])
            {
                return Ok(transaction_error(id, "Device parameter identity or value changed after apply; undo refused"));
            }
            self.adapter.invoke(&LiveInvocation::new(
                "device.parameter.set",
                parameter_mutation_args(&transaction, number(&transaction["priorValue"]), number(&transaction["appliedRevision"])),
            ))?;
            let snapshot = self.adapter.snapshot()?;
            let (_, restored, _) = self.parameter_target(&snapshot, device, reference)?;
            if !numeric_same(&restored["value"], &transaction["priorValue"])
                || revision(&restored) <= number(&transaction["appliedRevision"])
                || js_json::stringify(&self.parameter_authority(&snapshot, reference)?) != js_json::stringify(&transaction["authority"])
            {
                record.borrow_mut()["state"] = json!("uncertain");
                return Err(LiveError::error("Live did not confirm exact device-parameter restoration"));
            }
            {
                let mut record = record.borrow_mut();
                record["undoKey"] = params["idempotencyKey"].clone();
                record["state"] = json!("undone");
            }
            Ok(success_text(
                id,
                &json!({"transactionId":transaction["id"],"state":"undone","value":restored["value"],"revision":revision(&restored),"idempotent":false}),
            ))
        })();
        result.unwrap_or_else(|cause| adapter_tool_error(id, &cause, UNDO_FAILURE))
    }
    pub async fn undo_device_parameter_async(&self, id: &Value, params: &Value, signal: Option<&Signal>) -> Value {
        self.undo_parameters_async(id, params, signal, false).await
    }
    pub async fn undo_device_parameters_async(&self, id: &Value, params: &Value, signal: Option<&Signal>) -> Value {
        self.undo_parameters_async(id, params, signal, true).await
    }
    async fn undo_parameters_async(&self, id: &Value, params: &Value, signal: Option<&Signal>, multi: bool) -> Value {
        let map = if multi { &self.device_parameters_transactions } else { &self.device_parameter_transactions };
        let Some(record) = params["transactionId"].as_str().and_then(|id| map.get(id)) else {
            return transaction_error(id, "Only an applied or exact-key uncertain device-parameter transaction can be undone");
        };
        let transaction = record.borrow().clone();
        let reconciliation = transaction["state"] == "uncertain" && transaction["undoKey"] == params["idempotencyKey"];
        if transaction["state"] == "undone" && transaction["undoKey"] == params["idempotencyKey"] {
            return success_text(id, &json!({"transactionId":transaction["id"],"state":"undone","idempotent":true}));
        }
        let applied = if multi {
            transaction["parameters"].as_array().unwrap().iter().all(|p| p.get("appliedRevision").is_some())
        } else {
            transaction.get("appliedRevision").is_some()
        };
        if (transaction["state"] != "applied" && !reconciliation) || !applied {
            return transaction_error(id, "Only an applied or exact-key uncertain device-parameter transaction can be undone");
        }
        let items = if multi { transaction["parameters"].as_array().unwrap().clone() } else { singleton(&transaction) };
        let device = transaction["deviceRef"].as_str().unwrap();
        let prior = |i: usize| if multi { &transaction["parameters"][i]["priorValue"] } else { &transaction["priorValue"] };
        let result = async {
            self.begin_undo_recovery(&record, params["idempotencyKey"].as_str().unwrap())?;
            let status = self.require_connected(Some("device.parameter.write"))?;
            if json!(status.epoch) != transaction["epoch"] {
                return Ok(transaction_error(id, "Live connection epoch changed; undo refused"));
            }
            let adapter = self.async_adapter();
            let context = self.transaction_context(params, signal, AUDITION_DEADLINE_MS);
            record.borrow_mut()["undoKey"] = params["idempotencyKey"].clone();
            if reconciliation {
                self.replay_undo_recovery(&record, &*adapter, &context).await?;
            }
            let current = self.parameter_state_async(&context, device, &items, true).await?;
            if reconciliation {
                if current.iter().enumerate().any(|(i, c)| !numeric_same(&c.parameter["value"], prior(i)) || !c.same) {
                    return Err(LiveError::error("device-parameter undo replay did not restore exact prior state"));
                }
            } else {
                if current.iter().any(|c| !c.same) {
                    return Ok(transaction_error(
                        id,
                        if multi {
                            "A device parameter isn't the one this change was made on any more; undo refused"
                        } else {
                            "Device parameter isn't the one this change was made on any more; undo refused"
                        },
                    ));
                }
                record.borrow_mut()["state"] = json!("undoing");
                let args = if multi {
                    parameters_mutation_args(&transaction, |p, i| (number(&p["priorValue"]), revision(&current[i].parameter)))?
                } else {
                    parameter_mutation_args(&transaction, number(prior(0)), revision(&current[0].parameter))
                };
                self.invoke_undo_recovery(
                    &record,
                    &*adapter,
                    if multi { "device.parameters.set" } else { "device.parameter.set" },
                    &args,
                    &context,
                )
                .await?;
            }
            let restored = self.parameter_state_async(&context, device, &items, false).await?;
            if restored.iter().enumerate().any(|(i, c)| !numeric_same(&c.parameter["value"], prior(i)) || !c.same) {
                record.borrow_mut()["state"] = json!("uncertain");
                return Err(LiveError::error("Live did not confirm exact device-parameter restoration"));
            }
            record.borrow_mut()["state"] = json!("undone");
            let mut out = json!({"transactionId":transaction["id"],"state":"undone"});
            if !multi {
                out["value"] = restored[0].parameter["value"].clone();
                out["revision"] = json!(revision(&restored[0].parameter));
            }
            out["idempotent"] = json!(false);
            Ok(success_text(id, &out))
        }
        .await;
        result.unwrap_or_else(|cause| {
            if matches!(record.borrow()["state"].as_str(), Some("undoing" | "uncertain")) {
                record.borrow_mut()["state"] = json!("uncertain");
            }
            adapter_tool_error(id, &cause, UNDO_FAILURE)
        })
    }
}
