//! Non-parameter device edits retain the source family-specific readback and undo fences.
use super::{clip_properties::scalar_same, reads::AUDITION_DEADLINE_MS, *};
use kumi_common::{abort::Signal, js::json as js_json, time::now_ms_f64};
use sha2::{Digest, Sha256};
const EDITS: &[(&str, &[&str])] = &[
    ("set", &["setting", "value"]),
    ("modulate", &["source", "value", "targetIndex", "parameterRef"]),
    ("slice-insert", &["time"]),
    ("slice-move", &["time", "toTime"]),
    ("slice-remove", &["time"]),
    ("slice-clear", &[]),
    ("slice-reset", &[]),
    ("warp-as", &["beats"]),
    ("warp-double", &[]),
    ("warp-half", &[]),
    ("resend", &[]),
];
fn kept(edit: &str) -> Option<&'static str> {
    match edit {
        "slice-clear" | "slice-reset" => Some("Kumi can't put the slices back; Live's undo can."),
        "warp-as" | "warp-double" | "warp-half" => Some("Kumi can't take a warp back; Live's undo can."),
        "resend" => Some("Resending changes nothing in the Set: there's nothing to undo."),
        "modulate" => {
            Some("Kumi can't tell whether the amount was turned since (Live doesn't read it back); Live's undo can take it back.")
        }
        _ => None,
    }
}
fn family<'a>(device: &'a Value, key: &str) -> Option<&'a Value> {
    device.get(key).filter(|v| v.is_object())
}
fn state(edit: &str, device: &Value, setting: Option<&str>) -> Value {
    match edit {
        "set" => {
            json!({"property":setting,"value":setting.and_then(|s| DEVICE_PROPERTIES.get(s)).and_then(|s| family(device,&s.row_key).and_then(|row| row.get(&s.field)))})
        }
        "modulate" => {
            json!({"targets":family(device,"wavetable").and_then(|r| r["visibleModulationTargetNames"].as_array()).cloned().unwrap_or_default()})
        }
        _ if edit.starts_with("slice-") => {
            json!({"slices":family(device,"sample").and_then(|r| r["slices"].as_array()).cloned().unwrap_or_default()})
        }
        _ => json!({"sample":if edit=="resend" { None } else { family(device,"sample") }}),
    }
}
fn digest(value: &Value) -> Result<String, LiveError> {
    Ok(hex::encode(Sha256::digest(canonical_mutation_identity(value)?.as_bytes())))
}
fn field<'a>(value: &'a Value, name: &str) -> Result<Option<&'a Value>, LiveError> {
    if value.is_null() {
        Err(LiveError::type_error(format!("Cannot read properties of null (reading '{name}')")))
    } else {
        Ok(value.get(name))
    }
}
fn sorted(value: &Value) -> Result<String, LiveError> {
    // Array.sort's numeric comparator coerces values; stable ordering retains ties and NaN comparisons.
    let mut a = value.as_array().cloned().unwrap_or_default();
    fn num(v: &Value) -> Result<f64, LiveError> {
        Ok(match v {
            Value::Null => 0.0,
            Value::Bool(b) => {
                if *b {
                    1.0
                } else {
                    0.0
                }
            }
            _ => kumi_common::js::number::parse(&js_string(v)?).unwrap_or(f64::NAN),
        })
    }
    let mut error = None;
    a.sort_by(|a, b| match (num(a), num(b)) {
        (Ok(a), Ok(b)) => (a - b).partial_cmp(&0.0).unwrap_or(std::cmp::Ordering::Equal),
        (Err(e), _) | (_, Err(e)) => {
            error = Some(e);
            std::cmp::Ordering::Equal
        }
    });
    if let Some(e) = error {
        return Err(e);
    }
    Ok(js_json::stringify(&json!(a)))
}
impl McpHost {
    pub async fn dispatch_device_edit_tool(&self, call: &ToolCall, signal: Option<&Signal>) -> Option<Result<Option<Value>, LiveError>> {
        let p = call.arguments.as_ref().unwrap_or(&Value::Null);
        Some(Ok(match call.name.as_str() {
            "live_device_edit_preview" => Some(self.live_device_edit_preview_async(&call.id, p).await),
            "live_device_edit_apply" => self.live_device_edit_apply_async(&call.id, p, signal).await,
            _ => return None,
        }))
    }
    pub async fn live_device_edit_preview_async(&self, id: &Value, p: &Value) -> Value {
        let edit = p["action"].as_str().unwrap_or("");
        let takes = EDITS.iter().find(|(name, _)| *name == edit).map(|(_, fields)| *fields);
        if !p.is_object() || !is_non_empty_string(&p["deviceRef"], 256) || takes.is_none() {
            return error(
                id,
                -32602,
                &format!("deviceRef and an action ({}) are required", EDITS.iter().map(|(e, _)| *e).collect::<Vec<_>>().join(", ")),
                None,
            );
        }
        let takes = takes.unwrap();
        let extra: Vec<_> = p
            .as_object()
            .unwrap()
            .keys()
            .filter(|k| k.as_str() != "deviceRef" && k.as_str() != "action" && !takes.contains(&k.as_str()))
            .map(String::as_str)
            .collect();
        if !extra.is_empty() {
            return error(id, -32602, &format!("{edit} doesn't take {}", extra.join(" or ")), None);
        }
        if edit == "set" && !p["setting"].as_str().is_some_and(|s| DEVICE_PROPERTIES.contains_key(s)) {
            return error(id, -32602, &format!("setting is one of {}", PROPERTY_NAMES.join(", ")), None);
        }
        if edit == "set" && !p["value"].is_boolean() && !p["value"].as_f64().is_some_and(f64::is_finite) {
            return error(id, -32602, "value is a number, or true or false", None);
        }
        if edit == "modulate" {
            if p.get("targetIndex").is_none() == p.get("parameterRef").is_none() {
                return error(id, -32602, "name exactly one of targetIndex or parameterRef", None);
            }
            if !is_integer_in_range(&p["source"], 0.0, 1000.0)
                || !p["value"].as_f64().is_some_and(|n| n.is_finite() && (-1.0..=1.0).contains(&n))
            {
                return error(id, -32602, "source (0-1000) and value (-1 to 1) are required", None);
            }
            if p.get("targetIndex").is_some_and(|v| !is_integer_in_range(v, 0.0, 100000.0))
                || p.get("parameterRef").is_some_and(|v| !is_non_empty_string(v, 256))
            {
                return error(id, -32602, "targetIndex or parameterRef is invalid", None);
            }
        }
        let frames: Vec<_> = takes.iter().copied().filter(|k| *k == "time" || *k == "toTime").collect();
        if frames.iter().any(|f| !is_integer_in_range(&p[*f], 0.0, 9007199254740991.0)) {
            return error(id, -32602, &format!("{edit} takes {} in whole sample frames", frames.join(" and ")), None);
        }
        if edit == "warp-as" && !p["beats"].as_f64().is_some_and(|n| n.is_finite() && (0.001..=1000000.0).contains(&n)) {
            return error(id, -32602, "warp-as takes beats: how many beats the sample spans", None);
        }
        let result = async {
            let operation = match edit {
                "set" => "device.property.set",
                "modulate" => "wavetable.modulation.set",
                _ if edit.starts_with("slice-") => "sample.slice",
                _ => "device.action",
            };
            let status = self.fresh_status(Some(&LiveOperationContext::with_deadline(self.deadline(AUDITION_DEADLINE_MS)))).await?;
            if !status.has_operation(operation) {
                return Err(LiveError::error(format!("{operation} is unavailable on this Live shape")));
            }
            let snapshot = self
                .views
                .view_for(
                    Some(&LiveOperationContext::with_deadline(self.deadline(AUDITION_DEADLINE_MS))),
                    &[p["deviceRef"].clone()],
                    None,
                    &[],
                )
                .await?;
            let row = self.device_row(&snapshot, p["deviceRef"].as_str().unwrap())?;
            let device = &row.device;
            if !is_non_empty_string(&device["objectIdentity"], 256) {
                return Err(LiveError::error("device identity is not authoritative"));
            }
            let state = state(edit, device, p["setting"].as_str());
            let mut args = json!({"ref":p["deviceRef"]});
            let prior;
            let impact;
            if edit == "set" {
                let setting = p["setting"].as_str().unwrap();
                let spec = &DEVICE_PROPERTIES[setting];
                let current = family(device, &spec.row_key).and_then(|v| v.get(&spec.field));
                let Some(current) = current.filter(|v| !v.is_null()) else {
                    return Ok(transaction_error(id, &format!("that device has no {setting}")));
                };
                if current.is_boolean() != p["value"].is_boolean() {
                    return Ok(error(
                        id,
                        -32602,
                        &format!("{setting} takes {}", if current.is_boolean() { "true or false" } else { "a number" }),
                        None,
                    ));
                }
                let choices =
                    spec.choices.as_ref().and_then(|key| family(device, &spec.row_key).and_then(|r| r.get(key))).and_then(Value::as_array);
                if let Some(choices) = choices {
                    if !is_integer_in_range(&p["value"], 0.0, choices.len() as f64 - 1.0) {
                        let choices = choices
                            .iter()
                            .enumerate()
                            .map(|(i, v)| Ok(format!("{i} {}", js_string(v)?)))
                            .collect::<Result<Vec<_>, LiveError>>()?
                            .join(", ");
                        return Ok(error(id, -32602, &format!("{setting} is the index of one of its choices: {choices}"), None));
                    }
                }
                args["property"] = p["setting"].clone();
                args["value"] = p["value"].clone();
                let mut value = json!({"setting":p["setting"],"value":current});
                if let Some(choices) = choices {
                    value["choices"] = json!(choices);
                }
                prior = value;
                impact = "edits-device-setting";
            } else if edit == "modulate" {
                if family(device, "wavetable").is_none() {
                    return Ok(transaction_error(id, "modulating needs a Wavetable; that device isn't one"));
                }
                let targets = state["targets"].as_array().unwrap();
                if p.get("targetIndex").is_some_and(|v| v.as_f64().unwrap() >= targets.len() as f64) {
                    return Ok(error(
                        id,
                        -32602,
                        &format!("targetIndex is one of the matrix's {} targets (0-{})", targets.len(), targets.len() as i64 - 1),
                        None,
                    ));
                }
                if p.get("parameterRef").is_some()
                    && !device["parameters"].as_array().into_iter().flatten().any(|v| v.is_object() && v["ref"] == p["parameterRef"])
                {
                    return Ok(error(id, -32602, "parameterRef must name one of this Wavetable's parameters", None));
                }
                let key = if p.get("targetIndex").is_some() { "targetIndex" } else { "parameterRef" };
                args[key] = p[key].clone();
                args["source"] = p["source"].clone();
                args["value"] = p["value"].clone();
                prior = json!({"targets":targets});
                impact = "edits-wavetable-modulation";
            } else if edit.starts_with("slice-") {
                if family(device, "sample").is_none() {
                    return Ok(transaction_error(id, "slicing needs a Simpler with a sample; that device has none"));
                }
                let slices = state["slices"].as_array().unwrap();
                let includes = |key| slices.iter().any(|v| scalar_same(Some(v), p.get(key)));
                if edit == "slice-insert" && includes("time") {
                    return Ok(transaction_error(id, "a slice is already at that time"));
                }
                if matches!(edit, "slice-move" | "slice-remove") && !includes("time") {
                    return Ok(transaction_error(id, "no slice is at that time"));
                }
                if edit == "slice-move" && !scalar_same(p.get("toTime"), p.get("time")) && includes("toTime") {
                    return Ok(transaction_error(id, "a slice is already at the new time"));
                }
                args["action"] = json!(&edit[6..]);
                for key in ["time", "toTime"] {
                    if let Some(v) = p.get(key) {
                        args[key] = v.clone();
                    }
                }
                prior = json!({"slices":slices});
                impact = "edits-simpler-slices";
            } else {
                if family(device, if edit == "resend" { "ccControl" } else { "simpler" }).is_none() {
                    return Ok(transaction_error(
                        id,
                        &if edit == "resend" {
                            "resend needs a CC Control; that device isn't one".into()
                        } else {
                            format!("{edit} needs a Simpler; that device isn't one")
                        },
                    ));
                }
                if edit != "resend" && family(device, "sample").is_none() {
                    return Ok(transaction_error(id, "that Simpler has no sample"));
                }
                args["action"] = json!(if edit=="resend"{"cc-control-resend".into()}else{format!("simpler-{edit}")});
                if edit == "warp-as" {
                    args["beats"] = p["beats"].clone();
                }
                prior = json!({});
                impact = if edit == "resend" { "resends-cc-control-values" } else { "warps-simpler-sample" };
            }
            args["expectedObjectIdentity"] = device["objectIdentity"].clone();
            args["expectedStateRevision"] = json!(digest(&state)?);
            let t = json!({"id":tempo::transaction_id("devedit"),"epoch":status.epoch,"kind":"device-edit","fence":js_json::stringify(&json!({"ref":p["deviceRef"],"objectIdentity":device["objectIdentity"],"state":state})),"payload":{"edit":edit,"operation":operation,"args":args},"prior":prior,"expiresAt":now_ms_f64()+TRANSACTION_TTL_MS,"state":"previewed"});
            self.retain_bounded_transaction(&self.clip_lifecycle_transactions, t.clone(), "device edit")?;
            let mut body = json!({"transactionId":t["id"],"epoch":t["epoch"],"deviceRef":p["deviceRef"],"action":edit,"prior":prior,"impact":impact});
            if let Some(kept) = kept(edit) {
                body["undo"] = json!(kept);
            }
            body["confirmation"] = json!("apply");
            body["expiresAt"] = t["expiresAt"].clone();
            Ok(success_text(id, &body))
        }
        .await;
        result.unwrap_or_else(|e| {
            adapter_tool_error(id, &e, "Nothing was changed; discover the device again and preview from fresh references.")
        })
    }
    pub async fn live_device_edit_apply_async(&self, id: &Value, p: &Value, signal: Option<&Signal>) -> Option<Value> {
        if !valid_transaction_params(p, "apply") {
            return Some(error(id, -32602, "transactionId, confirmation=apply, and idempotencyKey are required", None));
        }
        let Some(record) = self.clip_lifecycle_transactions.get(p["transactionId"].as_str().unwrap()) else {
            return Some(transaction_error(id, "Unknown or expired device-edit transaction"));
        };
        let t = record.borrow().clone();
        if t["kind"] != "device-edit" || (t["state"] == "previewed" && t["expiresAt"].as_f64().unwrap_or(f64::NAN) <= now_ms_f64()) {
            return Some(transaction_error(id, "Unknown or expired device-edit transaction"));
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
        let edit = t["payload"]["edit"].as_str().unwrap_or("");
        let operation = t["payload"]["operation"].as_str().unwrap_or("");
        let args = &t["payload"]["args"];
        let result = async {
            if reconcile {
                self.fresh_status(Some(&LiveOperationContext::with_deadline(self.deadline(AUDITION_DEADLINE_MS)))).await?;
            }
            let status = self.require_connected(None)?;
            if json!(status.epoch) != t["epoch"] {
                return Ok(transaction_error(id, "Live connection epoch changed; preview again"));
            }
            let context = self.transaction_context(p, signal, AUDITION_DEADLINE_MS);
            if !reconcile {
                let snapshot = self.views.view_for(Some(&context), &[args["ref"].clone()], None, &[]).await?;
                let row = self.device_row(&snapshot, args["ref"].as_str().unwrap_or(""))?;
                let fence = json!({"ref":args["ref"],"objectIdentity":row.device["objectIdentity"],"state":state(edit,&row.device,args["property"].as_str())});
                if json!(js_json::stringify(&fence)) != t["fence"] {
                    return Ok(transaction_error(id, "the device changed since the preview; preview again"));
                }
            }
            record.borrow_mut()["state"] = json!("applying");
            record.borrow_mut()["applyKey"] = p["idempotencyKey"].clone();
            let result = self.async_adapter().invoke_async(&LiveInvocation::new(operation, args.clone()), Some(&context)).await?;
            let confirmed = match operation {
                "device.action" => field(&result, "done")? == Some(&Value::Bool(true)),
                "sample.slice" => field(&result, "slices")?.is_some_and(Value::is_array),
                _ => field(&result, "changed")? == Some(&Value::Bool(true)),
            };
            if !confirmed {
                return Err(LiveError::error("the device edit wasn't confirmed"));
            }
            if edit == "set" {
                record.borrow_mut()["created"] = json!({"value":result.get("value").filter(|v|!v.is_null()).unwrap_or(&args["value"])});
            } else if edit.starts_with("slice-") {
                record.borrow_mut()["created"] = json!({"slices":result["slices"]});
            }
            record.borrow_mut()["state"] = json!("applied");
            let mut body = json!({"transactionId":t["id"],"state":"applied","result":result});
            if let Some(kept) = kept(edit) {
                body["undo"] = json!(kept);
            }
            body["idempotent"] = json!(false);
            Ok(success_text(id, &body))
        }
        .await;
        Some(result.unwrap_or_else(|e| {
            self.apply_failed(id, &record, &e, "Whether the device changed is uncertain: look at it before trying again.")
        }))
    }
    pub async fn undo_device_edit_async(&self, id: &Value, p: &Value, signal: Option<&Signal>) -> Value {
        let Some(record) = self.clip_lifecycle_transactions.get(p["transactionId"].as_str().unwrap_or("")) else {
            return transaction_error(id, "Unknown or expired device-edit transaction");
        };
        let t = record.borrow().clone();
        if t["kind"] != "device-edit" {
            return transaction_error(id, "Unknown or expired device-edit transaction");
        }
        let edit = t["payload"]["edit"].as_str().unwrap_or("");
        let args = &t["payload"]["args"];
        if let Some(kept) = kept(edit) {
            return reason_error(id, kept, "If the producer wants it back, Live's own undo can take it back (Cmd-Z in Live).");
        }
        if t["state"] == "undone" && t["undoKey"] == p["idempotencyKey"] {
            return success_text(id, &json!({"transactionId":t["id"],"state":"undone","idempotent":true}));
        }
        let reconcile = t["state"] == "uncertain" && t["undoKey"] == p["idempotencyKey"];
        if (t["state"] != "applied" && !reconcile) || !t.get("created").is_some_and(arrangement::truthy) {
            return transaction_error(id, "Only an applied device edit can be undone");
        }
        let result = async {
            let status = self.require_connected(None)?;
            if json!(status.epoch) != t["epoch"] {
                return Ok(transaction_error(id, "Live connection epoch changed; undo refused"));
            }
            let context = self.transaction_context(p, signal, AUDITION_DEADLINE_MS);
            let snapshot = self.views.view_for(Some(&context), &[args["ref"].clone()], None, &[]).await?;
            let row = self.device_row(&snapshot, args["ref"].as_str().unwrap_or(""))?;
            if let Some(moved) = self.undo_target_moved(
                id,
                &t,
                "device",
                &args["ref"],
                row.device.get("objectIdentity"),
                args.get("expectedObjectIdentity"),
            )? {
                return Ok(moved);
            }
            let state = state(edit, &row.device, args["property"].as_str());
            let mut inverse = json!({"ref":args["ref"]});
            let operation;
            if edit == "set" {
                if !reconcile && !same_live_value(state.get("value"), t["created"].get("value")) {
                    return Ok(transaction_error(id, "the setting changed after the edit; undo refused"));
                }
                operation = "device.property.set";
                inverse["property"] = args["property"].clone();
                inverse["value"] = t["prior"]["value"].clone();
            } else {
                if !reconcile && sorted(&state["slices"])? != sorted(&t["created"]["slices"])? {
                    return Ok(transaction_error(id, "the slices changed after the edit; undo refused"));
                }
                operation = "sample.slice";
                inverse["action"] = json!(match edit {
                    "slice-insert" => "remove",
                    "slice-remove" => "insert",
                    _ => "move",
                });
                inverse["time"] = args[if matches!(edit, "slice-insert" | "slice-remove") { "time" } else { "toTime" }].clone();
                if !matches!(edit, "slice-insert" | "slice-remove") {
                    inverse["toTime"] = args["time"].clone();
                }
            }
            inverse["expectedObjectIdentity"] = row.device["objectIdentity"].clone();
            inverse["expectedStateRevision"] = json!(digest(&state)?);
            record.borrow_mut()["state"] = json!("undoing");
            record.borrow_mut()["undoKey"] = p["idempotencyKey"].clone();
            let result = self.async_adapter().invoke_async(&LiveInvocation::new(operation, inverse), Some(&context)).await?;
            if if operation == "sample.slice" {
                !field(&result, "slices")?.is_some_and(Value::is_array)
            } else {
                field(&result, "changed")? != Some(&Value::Bool(true))
            } {
                return Err(LiveError::error("the device edit's undo wasn't confirmed"));
            }
            record.borrow_mut()["state"] = json!("undone");
            Ok(success_text(id, &json!({"transactionId":t["id"],"state":"undone","idempotent":false})))
        }
        .await;
        result.unwrap_or_else(|e| {
            record.borrow_mut()["state"] = json!("uncertain");
            adapter_tool_error(id, &e, "Whether the device went back is uncertain: look at it.")
        })
    }
}

const PROPERTY_NAMES: &[&str] = &[
    "roar.routing_mode_index",
    "roar.env_listen",
    "shifter.pitch_mode_index",
    "spectral_resonator.frequency_dial_mode",
    "spectral_resonator.midi_gate",
    "spectral_resonator.mod_mode",
    "spectral_resonator.mono_poly",
    "spectral_resonator.pitch_mode",
    "hybrid_reverb.ir_time_shaping_on",
    "cc_control.custom_bool_target",
    "cc_control.custom_float_target_0",
    "cc_control.custom_float_target_1",
    "cc_control.custom_float_target_2",
    "cc_control.custom_float_target_3",
    "cc_control.custom_float_target_4",
    "cc_control.custom_float_target_5",
    "cc_control.custom_float_target_6",
    "cc_control.custom_float_target_7",
    "cc_control.custom_float_target_8",
    "cc_control.custom_float_target_9",
    "cc_control.custom_float_target_10",
    "cc_control.custom_float_target_11",
    "simpler.playback_mode",
    "simpler.retrigger",
    "simpler.slicing_playback_mode",
    "simpler.voices",
    "simpler.pad_slicing",
    "simpler.note_pitch_bend_range",
];
