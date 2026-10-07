//! Extended track/chain mixers and exact device routing transactions.
use super::*;
use super::{arrangement::truthy, device_parameter::fields, reads::AUDITION_DEADLINE_MS};
use kumi_common::{abort::Signal, js::json as js_json};
use sha2::{Digest, Sha256};

#[derive(Clone, Copy, PartialEq)]
enum Family {
    Extended,
    Chain,
    Io,
}
impl Family {
    fn kind(self) -> &'static str {
        match self {
            Self::Extended => "mixer-extended",
            Self::Chain => "chain-mixer",
            Self::Io => "device-io",
        }
    }
    fn label(self) -> &'static str {
        match self {
            Self::Extended => "extended-mixer",
            Self::Chain => "chain-mixer",
            Self::Io => "device-IO",
        }
    }
    fn title(self) -> &'static str {
        match self {
            Self::Extended => "Extended-mixer",
            Self::Chain => "Chain-mixer",
            Self::Io => "Device-IO",
        }
    }
    fn noun(self) -> &'static str {
        match self {
            Self::Extended => "extended mixer",
            Self::Chain => "chain mixer",
            Self::Io => "device routing",
        }
    }
    fn prefix(self) -> &'static str {
        match self {
            Self::Extended => "mixerext",
            Self::Chain => "chainmix",
            Self::Io => "devio",
        }
    }
    fn reference(self) -> &'static str {
        match self {
            Self::Extended => "trackRef",
            Self::Chain => "chainRef",
            Self::Io => "deviceRef",
        }
    }
    fn operation(self, p: &Value) -> &'static str {
        match self {
            Self::Extended => "mixer.extended.set",
            Self::Chain => "chain-mixer.set",
            Self::Io => {
                if p["action"] == "routing" {
                    "device-io.set"
                } else {
                    "compressor.sidechain.set"
                }
            }
        }
    }
    fn fields(self) -> &'static [&'static str] {
        match self {
            Self::Extended => &["trackActivator", "crossfader", "crossfadeAssign", "panningMode", "panningLeft", "panningRight"],
            Self::Chain => &["volume", "pan", "sends", "chainActivator"],
            Self::Io => &["routingType", "routingChannel"],
        }
    }
}
fn is_finite_in_range(v: &Value, low: f64, high: f64) -> bool {
    v.as_f64().is_some_and(|n| n.is_finite() && n >= low && n <= high)
}
fn digest(v: &Value) -> Result<String, LiveError> {
    Ok(hex::encode(Sha256::digest(canonical_mutation_identity(v)?)))
}
fn extend(a: &mut Value, b: &Value) {
    a.as_object_mut().unwrap().extend(b.as_object().unwrap().clone());
}
fn state(f: Family, object: &Value, p: &Value) -> Value {
    match f {
        Family::Extended => json!({"crossfadeAssign":object["mixer"]["crossfadeAssign"],"panningMode":object["mixer"]["panningMode"]}),
        Family::Chain => json!({"sends":object["mixer"].get("sends").filter(|v| !v.is_null()).cloned().unwrap_or(json!([]))}),
        Family::Io => {
            if p["action"] == "routing" {
                json!({"routingType":object["deviceIo"]["routingType"],"routingChannel":object["deviceIo"]["routingChannel"]})
            } else {
                let mut s = json!({});
                if let Some(v) = object.get("sidechainRoutingType") {
                    s["routingType"] = v.clone()
                }
                s
            }
        }
    }
}
fn fence(f: Family, reference: &Value, object: &Value, p: &Value) -> String {
    let mut result = json!({"ref":reference});
    if let Some(v) = object.get("objectIdentity") {
        result["objectIdentity"] = v.clone()
    }
    let state = state(f, object, p);
    if f == Family::Io {
        result["state"] = state;
    } else {
        if let Some(v) = object["mixer"].get("mixerIdentity") {
            result["mixerIdentity"] = v.clone()
        }
        if f == Family::Chain {
            result["sends"] = state["sends"].clone()
        } else {
            result["state"] = state
        }
    }
    js_json::stringify(&result)
}
fn family(value: &str) -> Option<Family> {
    match value {
        "mixer-extended" | "mixerext" => Some(Family::Extended),
        "chain-mixer" | "chainmix" => Some(Family::Chain),
        "device-io" | "devio" => Some(Family::Io),
        _ => None,
    }
}
impl McpHost {
    fn extended_object(&self, f: Family, snapshot: &LiveSnapshot, reference: &Value) -> Result<(Value, Value), LiveError> {
        match f {
            Family::Extended => Ok((
                snapshot
                    .tracks
                    .iter()
                    .flatten()
                    .find(|t| json!(t.ref_) == *reference)
                    .map(|v| serde_json::to_value(v).unwrap())
                    .unwrap_or(Value::Null),
                Value::Null,
            )),
            Family::Chain => {
                let found = self.chain_row(snapshot, reference.as_str().unwrap_or(""))?;
                Ok((found.chain, found.device))
            }
            Family::Io => Ok((self.device_row(snapshot, reference.as_str().unwrap_or(""))?.device, Value::Null)),
        }
    }
    pub async fn dispatch_extended_mixer_tool(&self, call: &ToolCall, signal: Option<&Signal>) -> Option<Result<Option<Value>, LiveError>> {
        let (f, preview) = match call.name.as_str() {
            "live_mixer_extended_preview" => (Family::Extended, true),
            "live_mixer_extended_apply" => (Family::Extended, false),
            "live_chain_mixer_preview" => (Family::Chain, true),
            "live_chain_mixer_apply" => (Family::Chain, false),
            "live_device_io_preview" => (Family::Io, true),
            "live_device_io_apply" => (Family::Io, false),
            _ => return None,
        };
        let p = call.arguments.as_ref().unwrap_or(&Value::Null);
        Some(Ok(if preview {
            Some(self.extended_preview(&call.id, p, f).await)
        } else {
            self.extended_apply(&call.id, p, f, signal).await
        }))
    }
    async fn extended_preview(&self, id: &Value, p: &Value, f: Family) -> Value {
        let mut proposed = json!({});
        if f == Family::Io {
            if !has_only(p, &["action", "deviceRef", "routingType", "routingChannel"])
                || !is_non_empty_string(&p["deviceRef"], 256)
                || !is_non_empty_string(&p["routingType"], 128)
            {
                return error(id, -32602, "action, deviceRef, and routingType are required", None);
            }
            if p["action"] != "routing" && p["action"] != "sidechain" {
                return error(id, -32602, "action must be routing or sidechain", None);
            }
            if p["action"] == "routing" && p.get("routingChannel").is_some_and(|v| !is_non_empty_string(v, 128)) {
                return error(id, -32602, "routingChannel is invalid", None);
            }
            // Live's sidechain operation sets the source (its routing type) only: a channel sent with it would be
            // refused at apply, after the preview said yes.
            if p["action"] == "sidechain" && p.get("routingChannel").is_some() {
                return error(
                    id,
                    -32602,
                    "routingChannel goes with action routing; a sidechain takes routingType only (Live keeps its channel)",
                    None,
                );
            }
        } else {
            let mut allowed = vec![f.reference()];
            allowed.extend_from_slice(f.fields());
            if !has_only(p, &allowed) || !is_non_empty_string(&p[f.reference()], 256) {
                return error(id, -32602, &format!("{} is required", f.reference()), None);
            }
            for field in f.fields() {
                let Some(value) = p.get(*field) else { continue };
                let invalid: Option<String> = match *field {
                    "trackActivator" | "chainActivator" => (!value.is_boolean()).then(|| format!("{field} must be boolean")),
                    "crossfader" | "panningLeft" | "panningRight" | "pan" => {
                        (!is_finite_in_range(value, -1., 1.)).then(|| format!("{field} must be -1 to 1"))
                    }
                    "crossfadeAssign" => (!is_integer_in_range(value, 0., 2.)).then(|| "crossfadeAssign must be 0-2".into()),
                    "panningMode" => (!is_integer_in_range(value, 0., 8.)).then(|| "panningMode must be 0-8".into()),
                    "volume" => (!is_finite_in_range(value, 0., 1.)).then(|| "volume must be 0-1".into()),
                    "sends" => value
                        .as_array()
                        .is_none_or(|a| !a.iter().all(|v| is_finite_in_range(v, 0., 1.)))
                        .then(|| "sends must be 0-1 values".into()),
                    _ => None,
                };
                if let Some(message) = invalid {
                    return error(id, -32602, &message, None);
                }
                proposed[*field] = value.clone();
            }
            if proposed.as_object().unwrap().is_empty() {
                return error(id, -32602, &format!("at least one {} field is required", f.noun()), None);
            }
        }
        let result=async{
            let status=self.fresh_status(Some(&LiveOperationContext::with_deadline(self.deadline(AUDITION_DEADLINE_MS)))).await?;
            if !status.connected||!status.capabilities.iter().any(|c|c.as_str()=="session.read"){return Err(LiveError::error("session read capability is unavailable"))}
            let op=f.operation(p);if !status.has_operation(op){return Err(LiveError::error(if f==Family::Io{format!("{op} is unavailable on this Live shape")}else{format!("{} editing is unavailable",f.noun())}))}
            let reference=&p[f.reference()];let snapshot=self.views.view_for(None,&[reference.clone()],None,&[]).await?;
            let (object,rack)=self.extended_object(f,&snapshot,reference)?;
            if f==Family::Extended && (object.is_null()||!is_non_empty_string(&object["objectIdentity"],256)){return Err(LiveError::error("track identity is not authoritative"))}
            let mixer=&object["mixer"];
            if f!=Family::Io && (!truthy(mixer)||!is_non_empty_string(&mixer["mixerIdentity"],256)){return Err(LiveError::error(if f==Family::Chain{"chain mixer identity is not authoritative"}else{"mixer identity is not authoritative"}))}
            if f==Family::Chain && proposed["sends"].as_array().is_some_and(|a|a.len()>mixer["sends"].as_array().map_or(0,Vec::len)){return Ok(error(id,-32602,"chain has fewer sends than proposed",None))}
            if f==Family::Io {
                // Live's rows always carry both: deviceIo with no routing types and a null type for a device without
                // inputs, and a null sidechainRoutingType for one without a sidechain.
                let io=&object["deviceIo"];
                let inputs=io.is_object()&&(!io["routingType"].is_null()||io["availableRoutingTypes"].as_array().is_some_and(|types|!types.is_empty()));
                if p["action"]=="routing"&&!inputs{return Ok(transaction_error(id,"device IO is unavailable on this exact device"))}
                if p["action"]=="sidechain"&&object.get("sidechainRoutingType").is_none_or(Value::is_null){return Ok(transaction_error(id,"sidechain routing is unavailable on this exact device"))}
            }
            let state=state(f,&object,p);
            let prior=if f==Family::Io{state.clone()}else{Value::Object(proposed.as_object().unwrap().keys().map(|k|(k.clone(),mixer[k].clone())).collect())};
            let mut payload=if f==Family::Io{let mut value=fields(p,&["action"]);value["ref"]=reference.clone();extend(&mut value,&fields(p,&["routingType","routingChannel"]));value}else{let mut value=json!({"ref":reference});extend(&mut value,&proposed);value};
            if let Some(v)=object.get("objectIdentity"){payload["expectedObjectIdentity"]=v.clone()}
            if f!=Family::Io {payload["expectedMixerIdentity"]=mixer["mixerIdentity"].clone()}
            payload["expectedStateRevision"]=json!(digest(&state)?);
            let mut t=json!({"id":tempo::transaction_id(f.prefix()),"epoch":status.epoch,"kind":f.kind(),"fence":fence(f,reference,&object,p),"payload":payload,"prior":prior,"expiresAt":kumi_common::time::now_ms_f64()+TRANSACTION_TTL_MS,"state":"previewed"});
            if f!=Family::Io{t["clipRef"]=reference.clone()}
            self.retain_bounded_transaction(&self.clip_lifecycle_transactions,t.clone(),if f==Family::Io{"device IO"}else{f.noun()})?;
            let mut response=json!({"transactionId":t["id"],"epoch":t["epoch"]});if f==Family::Io{response["action"]=p["action"].clone()}response[f.reference()]=reference.clone();
            if f==Family::Chain{response["chainName"]=object["name"].as_str().map(|v|json!(v)).unwrap_or(Value::Null);response["rackName"]=rack["name"].as_str().map(|v|json!(v)).unwrap_or(Value::Null)}
            response["prior"]=prior;if f!=Family::Io{response["proposed"]=proposed}response["impact"]=json!(match f{Family::Extended=>"edits-extended-mixer",Family::Chain=>"edits-chain-mixer",Family::Io=>"edits-device-routing"});response["confirmation"]=json!("apply");response["expiresAt"]=t["expiresAt"].clone();
            Ok(success_text(id,&response))
        }.await;
        result.unwrap_or_else(|e| adapter_tool_error(id, &e, &format!("{} preview requires fresh authoritative state.", f.title())))
    }
    async fn extended_apply(&self, id: &Value, p: &Value, f: Family, signal: Option<&Signal>) -> Option<Value> {
        if !valid_transaction_params(p, "apply") {
            return Some(error(id, -32602, "transactionId, confirmation=apply, and idempotencyKey are required", None));
        }
        let record = self.clip_lifecycle_transactions.get(p["transactionId"].as_str().unwrap());
        let t = record.as_ref().map(|r| r.borrow().clone()).unwrap_or(Value::Null);
        if t.is_null()
            || t["kind"] != f.kind()
            || (t["state"] == "previewed" && t["expiresAt"].as_f64().is_some_and(|n| n <= kumi_common::time::now_ms_f64()))
        {
            return Some(transaction_error(id, &format!("Unknown or expired {} transaction", f.label())));
        }
        let record = record.unwrap();
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
            let reference = if f == Family::Io { &t["payload"]["ref"] } else { &t["clipRef"] };
            if !reconcile {
                let snapshot = self.views.view_for(Some(&context), &[reference.clone()], None, &[]).await?;
                let (object, _) = self.extended_object(f, &snapshot, reference)?;
                if (f == Family::Extended && (object.is_null() || !truthy(&object["mixer"])))
                    || t["fence"] != fence(f, reference, &object, &t["payload"])
                {
                    return Ok(transaction_error(
                        id,
                        match f {
                            Family::Extended => "track, mixer, or extended state changed since preview; preview again",
                            Family::Chain => "chain or mixer state changed since preview; preview again",
                            Family::Io => "device or routing state changed since preview; preview again",
                        },
                    ));
                }
            }
            {
                let mut row = record.borrow_mut();
                row["state"] = json!("applying");
                row["applyKey"] = p["idempotencyKey"].clone()
            }
            let mut args = t["payload"].clone();
            if f == Family::Io {
                args.as_object_mut().unwrap().remove("action");
            }
            let result = adapter.invoke_async(&LiveInvocation::new(f.operation(&t["payload"]), args), Some(&context)).await?;
            if result.is_null() {
                return Err(LiveError::type_error("Cannot read properties of null (reading 'changed')"));
            }
            if result["changed"] != true {
                return Err(LiveError::error(format!("{} change was not confirmed", f.noun())));
            }
            let snapshot = self.views.view_for(Some(&context), &[reference.clone()], None, &[]).await?;
            let (object, _) = self.extended_object(f, &snapshot, reference)?;
            if f == Family::Io {
                let current = state(f, &object, &t["payload"]);
                for field in f.fields() {
                    if *field == "routingChannel" && t["payload"]["action"] != "routing" {
                        continue;
                    }
                    if let Some(expected) = t["payload"].get(*field) {
                        if current.get(*field) != Some(expected) {
                            return Err(LiveError::error(if t["payload"]["action"] != "routing" {
                                "sidechain postcondition was not confirmed"
                            } else if *field == "routingChannel" {
                                "device channel postcondition was not confirmed"
                            } else {
                                "device routing postcondition was not confirmed"
                            }));
                        }
                    }
                }
            } else {
                for field in f.fields() {
                    if let Some(value) = t["payload"].get(*field) {
                        if !same_mixer_value(field, object["mixer"].get(*field), Some(value)) {
                            return Err(LiveError::error(format!("{} postcondition was not confirmed", f.noun())));
                        }
                    }
                }
            }
            {
                let mut row = record.borrow_mut();
                row["applyKey"] = p["idempotencyKey"].clone();
                row["state"] = json!("applied")
            }
            let mut response = json!({"transactionId":t["id"],"state":"applied"});
            if let Some(v) = result.get("revision") {
                response["revision"] = v.clone()
            }
            response["idempotent"] = json!(false);
            Ok(success_text(id, &response))
        }
        .await;
        Some(result.unwrap_or_else(|e| {
            record.borrow_mut()["state"] = json!("uncertain");
            adapter_tool_error(id, &e, &format!("{} state is uncertain; perform fresh discovery before retrying.", f.title()))
        }))
    }
    pub async fn undo_extended_mixer_async(&self, id: &Value, p: &Value, signal: Option<&Signal>) -> Value {
        let prefix = p["transactionId"].as_str().unwrap_or("").split('_').next().unwrap_or("");
        let Some(f) = family(prefix) else { return transaction_error(id, "Unknown extended mixer transaction") };
        let record = p["transactionId"].as_str().and_then(|id| self.clip_lifecycle_transactions.get(id));
        let t = record.as_ref().map(|r| r.borrow().clone()).unwrap_or(Value::Null);
        if t["kind"] != f.kind() {
            return transaction_error(id, &format!("Unknown or expired {} transaction", f.label()));
        }
        let record = record.unwrap();
        if t["state"] == "undone" && t["undoKey"] == p["idempotencyKey"] {
            return success_text(id, &json!({"transactionId":t["id"],"state":"undone","idempotent":true}));
        }
        let reconcile = t["state"] == "uncertain" && t["undoKey"] == p["idempotencyKey"];
        if (t["state"] != "applied" && !reconcile) || !truthy(&t["prior"]) || (f == Family::Chain && !truthy(&t["clipRef"])) {
            return transaction_error(id, &format!("Only an applied or exact-key uncertain {} transaction can be undone", f.label()));
        }
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
            let reference = if f == Family::Io { &t["payload"]["ref"] } else { &t["clipRef"] };
            let snapshot = self.views.view_for(Some(&context), &[reference.clone()], None, &[]).await?;
            let (object, _) = self.extended_object(f, &snapshot, reference)?;
            let mixer = &object["mixer"];
            if f != Family::Io && (object.is_null() || !truthy(mixer) || !is_non_empty_string(&mixer["mixerIdentity"], 256)) {
                return Err(LiveError::error(format!("{} authority is unavailable", f.noun())));
            }
            let (what, found, made) = if f == Family::Io {
                ("device", object.get("objectIdentity").cloned(), t["payload"].get("expectedObjectIdentity").cloned())
            } else {
                let what = if f == Family::Chain { "chain" } else { "track" };
                let mut found = json!({});
                found[what] = object["objectIdentity"].clone();
                found["mixer"] = mixer["mixerIdentity"].clone();
                let mut made = json!({});
                made[what] = t["payload"]["expectedObjectIdentity"].clone();
                made["mixer"] = t["payload"]["expectedMixerIdentity"].clone();
                (what, Some(found), Some(made))
            };
            if let Some(moved) = self.undo_target_moved(id, &record.borrow(), what, reference, found.as_ref(), made.as_ref())? {
                return Ok(moved);
            }
            let current = state(f, &object, &t["payload"]);
            if !reconcile {
                if f == Family::Io {
                    // The fields the change set, as apply checks them: a type set alone leaves the channel to Live.
                    for field in f.fields() {
                        if *field == "routingChannel" && t["payload"]["action"] != "routing" {
                            continue;
                        }
                        if let Some(expected) = t["payload"].get(*field) {
                            if current.get(*field) != Some(expected) {
                                return Ok(transaction_error(id, "device routing changed after apply; undo refused"));
                            }
                        }
                    }
                } else {
                    for (field, value) in t["payload"].as_object().unwrap() {
                        if ["ref", "expectedObjectIdentity", "expectedMixerIdentity", "expectedStateRevision"].contains(&field.as_str()) {
                            continue;
                        }
                        if !same_mixer_value(field, mixer.get(field), Some(value)) {
                            return Ok(transaction_error(id, &format!("{} changed after apply; undo refused", f.noun())));
                        }
                    }
                }
            }
            record.borrow_mut()["state"] = json!("undoing");
            let mut args = json!({"ref":reference});
            if f != Family::Io {
                extend(
                    &mut args,
                    &Value::Object(
                        t["prior"]
                            .as_object()
                            .unwrap()
                            .iter()
                            .filter(|(_, v)| !v.is_null())
                            .map(|(k, v)| (k.clone(), named_mixer_part(k, v, &t["payload"][k])))
                            .collect(),
                    ),
                )
            }
            if let Some(v) = object.get("objectIdentity") {
                args["expectedObjectIdentity"] = v.clone()
            }
            if f != Family::Io {
                args["expectedMixerIdentity"] = mixer["mixerIdentity"].clone()
            }
            args["expectedStateRevision"] = json!(digest(&current)?);
            if f == Family::Io {
                for field in f.fields() {
                    if *field == "routingChannel" && t["payload"]["action"] != "routing" {
                        continue;
                    }
                    if let Some(v) = t["prior"].get(*field).filter(|v| !v.is_null()) {
                        args[*field] = v.clone()
                    }
                }
            }
            let result = self.invoke_undo_recovery(&record, adapter.as_ref(), f.operation(&t["payload"]), &args, &context).await?;
            if result.is_null() {
                return Err(LiveError::type_error("Cannot read properties of null (reading 'changed')"));
            }
            if result["changed"] != true {
                return Err(LiveError::error(format!("{} restoration was not confirmed", f.noun())));
            }
            record.borrow_mut()["state"] = json!("undone");
            Ok(success_text(id, &json!({"transactionId":t["id"],"state":"undone","idempotent":false})))
        }
        .await;
        result.unwrap_or_else(|e| {
            record.borrow_mut()["state"] = json!("uncertain");
            adapter_tool_error(id, &e, &format!("{} undo is uncertain; perform fresh discovery.", f.title()))
        })
    }
}
