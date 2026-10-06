//! Published device-family properties, fenced over each family's complete state.
use super::*;
use super::{arrangement::truthy, reads::AUDITION_DEADLINE_MS};
use kumi_common::{abort::Signal, js::json as js_json};
use sha2::{Digest, Sha256};
fn family_fields(family: &str) -> Option<&'static [&'static str]> {
    Some(match family {
        "drift" => &[
            "pitchBendRange",
            "voiceCount",
            "voiceMode",
            "modFilterSource1",
            "modFilterSource2",
            "modLfoSource",
            "modPitchSource1",
            "modPitchSource2",
            "modShapeSource",
            "modSource1",
            "modSource2",
            "modSource3",
            "modTarget1",
            "modTarget2",
            "modTarget3",
        ],
        "drum-cell" => &["gain"],
        "eq8" => &["editMode", "globalMode", "oversample", "selectedBand"],
        "hybrid-reverb" => &["irCategory", "irFile", "attack", "decay", "size"],
        "meld" => &["engine", "unison", "monoPoly", "polyphony"],
        "plugin" => &["presetIndex", "isEditorOpen"],
        "sample" => SAMPLE_FIELDS,
        "wavetable" => WAVETABLE_FIELDS,
        _ => return None,
    })
}
fn row_key(family: &str) -> &str {
    match family {
        "drum-cell" => "drumCell",
        "hybrid-reverb" => "hybridReverb",
        _ => family,
    }
}
/// What each place means for the settings Live takes as a place in a fixed list without giving the list
/// (Live's own encodings for Meld and Wavetable).
fn fixed_choices(field: &str) -> Option<&'static [&'static str]> {
    Some(match field {
        "unison" => &["off", "two", "three", "four"],
        "polyphony" => &["two", "three", "four", "five", "six", "eight", "twelve"],
        "filterRouting" => &["Serial", "Parallel", "Split"],
        "oscillator1EffectMode" | "oscillator2EffectMode" => &["None", "FM", "Classic", "Modern"],
        "unisonMode" => &["None", "Classic", "Shimmer", "Noise", "Phase Sync", "Position Spread", "Random Note"],
        _ => return None,
    })
}
/// Meld's settings whose first choice Live has but the bridge's protocol doesn't take yet (it starts
/// them at 1): run_python sets those, through the device property each names.
fn first_not_yet(field: &str) -> Option<&'static str> {
    match field {
        "unison" => Some("unison_voices"),
        "polyphony" => Some("poly_voices"),
        _ => None,
    }
}
/// Places and what they mean, "1 two, 2 three or 3 four".
fn said_places(choices: &[&str], from: usize) -> String {
    let places: Vec<_> = choices.iter().enumerate().skip(from).map(|(place, name)| format!("{place} {name}")).collect();
    match places.split_last() {
        Some((last, rest)) if !rest.is_empty() => format!("{} or {last}", rest.join(", ")),
        _ => places.join(""),
    }
}
fn out_of_bounds(field: &str, value: &Value) -> String {
    match (fixed_choices(field), first_not_yet(field)) {
        (Some(choices), Some(property)) if value.as_f64() == Some(0.) => format!(
            "Kumi's Meld tool can't set {field} to 0 ({}) yet ({} work); run_python can set the device's {property} to 0",
            choices[0],
            said_places(choices, 1)
        ),
        (Some(choices), Some(_)) => {
            format!(
                "{field} is out of bounds; its choices are {} (0 {} only through run_python for now)",
                said_places(choices, 1),
                choices[0]
            )
        }
        (Some(choices), None) => format!("{field} is out of bounds; its choices are {}", said_places(choices, 0)),
        _ if field == "unisonVoiceCount" => "unisonVoiceCount is out of bounds; Wavetable has 2 to 8 unison voices".into(),
        _ => format!("{field} is out of bounds"),
    }
}
fn bounds(field: &str) -> Option<(f64, f64, bool)> {
    if let Some(choices) = fixed_choices(field) {
        return Some((if first_not_yet(field).is_some() { 1. } else { 0. }, (choices.len() - 1) as f64, true));
    }
    Some(match field {
        "pitchBendRange" => (1., 96., true),
        "voiceCount" => (1., 64., true),
        "unisonVoiceCount" => (2., 8., true),
        "voiceMode" => (0., 8., true),
        "modFilterSource1" | "modFilterSource2" | "modLfoSource" | "modPitchSource1" | "modPitchSource2" | "modShapeSource"
        | "modSource1" | "modSource2" | "modSource3" | "modTarget1" | "modTarget2" | "modTarget3" => (0., 1000., true),
        "gain" => (-70., 24., false),
        "editMode" | "globalMode" | "engine" => (0., 4., true),
        "selectedBand" => (0., 8., true),
        "attack" | "size" => (0., 10000., false),
        "decay" | "time" => (0., 100000., false),
        "presetIndex" => (0., 1024., true),
        value if SAMPLE_FIELDS.contains(&value) => (0., 1000000., false),
        value if WAVETABLE_FIELDS.contains(&value) => (0., 100000., true),
        _ => return None,
    })
}
/// Drift's settings that pick from a list the device row gives: each field, then its list.
const DRIFT_CHOICES: [(&str, &str); 13] = [
    ("voiceMode", "voiceModeList"),
    ("modFilterSource1", "modFilterSourceList"),
    ("modFilterSource2", "modFilterSourceList"),
    ("modLfoSource", "modLfoSourceList"),
    ("modPitchSource1", "modPitchSourceList"),
    ("modPitchSource2", "modPitchSourceList"),
    ("modShapeSource", "modShapeSourceList"),
    ("modSource1", "modSources"),
    ("modSource2", "modSources"),
    ("modSource3", "modSources"),
    ("modTarget1", "modTargets"),
    ("modTarget2", "modTargets"),
    ("modTarget3", "modTargets"),
];
fn names(list: &Value) -> Option<Vec<&str>> {
    list.as_array()?.iter().map(Value::as_str).collect()
}
/// A place a setting picks in a list the device row gives: one past its end is refused, with the list's
/// first places named.
fn check_place(device: &str, field: &str, index: u64, choices: &str, list: &[&str]) -> Result<(), String> {
    if (index as usize) < list.len() {
        return Ok(());
    }
    let mut places: Vec<_> = list.iter().take(24).enumerate().map(|(place, name)| format!("{place} {name}")).collect();
    if list.len() > places.len() {
        places.push(format!("… {} in all", list.len()));
    }
    Err(format!("{field} {index} isn't a place in this {device}'s {choices} ({})", places.join(", ")))
}
/// The number of voices a name in Drift's voiceCountList stands for ("8").
fn voices(name: &str) -> Option<i64> {
    name.trim().parse().ok()
}
/// Drift's voiceCount is a number of voices, one its voiceCountList names; Live takes that name's place
/// in the list, which is what the device row's voiceCount holds. Its other choices are places in their
/// lists, and one past the end is refused here rather than failing in Live. A row without the lists
/// (an older Remote Script's) passes as it is.
fn drift_choices(proposed: &mut Value, row: &Value) -> Result<(), String> {
    if let (Some(count), Some(list)) = (proposed.get("voiceCount").and_then(Value::as_i64), names(&row["voiceCountList"])) {
        let Some(index) = list.iter().position(|name| voices(name) == Some(count)) else {
            return Err(format!("voiceCount {count} isn't one of this Drift's voice counts ({})", list.join(", ")));
        };
        // The bridge's protocol takes Drift's voice count from place 1, so its first count waits for it.
        if index == 0 {
            let (first, rest) = list.split_first().unwrap();
            let rest = match rest.split_last() {
                Some((last, others)) if !others.is_empty() => format!("{} or {last}", others.join(", ")),
                _ => rest.join(""),
            };
            return Err(format!(
                "Kumi's Drift tool can't set {first} voices yet ({rest} work); run_python can set the device's voice_count_index to 0"
            ));
        }
        proposed["voiceCount"] = json!(index);
    }
    for (field, choices) in DRIFT_CHOICES {
        if let (Some(index), Some(list)) = (proposed.get(field).and_then(Value::as_u64), names(&row[choices])) {
            check_place("Drift", field, index, choices, &list)?;
        }
    }
    Ok(())
}
/// Wavetable's oscillators pick a category from the row's categories, and a wavetable from the row's
/// list for the oscillator's category; when the category changes too, its list isn't known yet.
fn wavetable_choices(proposed: &Value, row: &Value) -> Result<(), String> {
    for oscillator in ["oscillator1", "oscillator2"] {
        let category = format!("{oscillator}WavetableCategory");
        if let (Some(index), Some(list)) = (proposed.get(&category).and_then(Value::as_u64), names(&row["categories"])) {
            check_place("Wavetable", &category, index, "categories", &list)?;
        }
        let changes = proposed.get(&category).is_some_and(|asked| Some(asked) != row.get(&category));
        let wavetables = format!("{oscillator}Wavetables");
        let field = format!("{oscillator}WavetableIndex");
        if let (false, Some(index), Some(list)) = (changes, proposed.get(&field).and_then(Value::as_u64), names(&row[&wavetables])) {
            check_place("Wavetable", &field, index, &wavetables, &list)?;
        }
    }
    Ok(())
}
/// What a preview shows of Drift's state: its voiceCount as the number of voices, as it's asked for.
fn shown_voice_count(mut state: Value, row: &Value) -> Value {
    let count = state.get("voiceCount").and_then(Value::as_u64).zip(names(&row["voiceCountList"]));
    if let Some(count) = count.and_then(|(index, list)| list.get(index as usize).and_then(|name| voices(name))) {
        state["voiceCount"] = json!(count);
    }
    state
}
fn digest(v: &Value) -> Result<String, LiveError> {
    Ok(hex::encode(Sha256::digest(canonical_mutation_identity(v)?)))
}
fn state(fields: &[&str], row: &Value) -> Value {
    Value::Object(fields.iter().map(|k| ((*k).into(), row[*k].clone())).collect())
}
fn fence(family: &str, reference: &Value, device: &Value, fields: &[&str]) -> String {
    let mut row = json!({"family":family,"ref":reference});
    if let Some(v) = device.get("objectIdentity") {
        row["objectIdentity"] = v.clone()
    }
    row["state"] = state(fields, &device[row_key(family)]);
    js_json::stringify(&row)
}
impl McpHost {
    pub async fn dispatch_specialized_device_tool(
        &self,
        call: &ToolCall,
        signal: Option<&Signal>,
    ) -> Option<Result<Option<Value>, LiveError>> {
        let p = call.arguments.as_ref().unwrap_or(&Value::Null);
        Some(Ok(match call.name.as_str() {
            "live_device_specialized_preview" => Some(self.live_device_specialized_preview_async(&call.id, p).await),
            "live_device_specialized_apply" => self.live_device_specialized_apply_async(&call.id, p, signal).await,
            _ => return None,
        }))
    }
    pub async fn live_device_specialized_preview_async(&self, id: &Value, p: &Value) -> Value {
        let family = p["family"].as_str().unwrap_or("");
        let Some(fields) = family_fields(family).filter(|_| p.is_object() && is_non_empty_string(&p["deviceRef"], 256)) else {
            return error(id, -32602, "family and deviceRef are required", None);
        };
        let mut allowed = vec!["family", "deviceRef"];
        allowed.extend_from_slice(fields);
        if !has_only(p, &allowed) {
            return error(id, -32602, &format!("only {family} fields are accepted"), None);
        }
        if fields.iter().all(|f| p.get(*f).is_none()) {
            return error(id, -32602, "at least one field is required", None);
        }
        let result=async{
   let status=self.fresh_status(Some(&LiveOperationContext::with_deadline(self.deadline(AUDITION_DEADLINE_MS)))).await?;if !status.connected||!status.capabilities.iter().any(|c|c.as_str()=="session.read"){return Err(LiveError::error("session read capability is unavailable"))}let operation=format!("{family}.set");if !status.has_operation(&operation){return Err(LiveError::error(format!("{operation} is unavailable on this Live shape")))}let snapshot=self.views.view_for(None,&[p["deviceRef"].clone()],None,&[]).await?;let row=self.device_row(&snapshot,p["deviceRef"].as_str().unwrap())?;
   let mut proposed=json!({});for field in fields{let Some(value)=p.get(*field)else{continue};if ["oversample","monoPoly","isEditorOpen"].contains(field){if !value.is_boolean(){return Ok(error(id,-32602,&format!("{field} must be boolean"),None))}}else if ["irCategory","irFile"].contains(field){if !is_non_empty_string(value,256){return Ok(error(id,-32602,&format!("{field} is invalid"),None))}}else if !value.is_number(){return Ok(error(id,-32602,&format!("{field} must be a number"),None))}
    if let Some((low,high,integer))=bounds(field){if value.as_f64().is_some_and(|n|n<low||n>high||integer&&n.fract()!=0.){return Ok(error(id,-32602,&out_of_bounds(field,value),None))}}proposed[*field]=value.clone();}
   let family_row=&row.device[row_key(family)];if ["sample","wavetable"].contains(&family)&&!row.device[family].is_object(){return Ok(transaction_error(id,if family=="sample"{"that device isn't a Simpler with a sample"}else{"that device isn't a Wavetable"}))}
   let checked=match family{"drift"=>drift_choices(&mut proposed,family_row),"wavetable"=>wavetable_choices(&proposed,family_row),_=>Ok(())};if let Err(problem)=checked{return Ok(error(id,-32602,&problem,None))}
   let state=state(fields,family_row);let prior=Value::Object(proposed.as_object().unwrap().keys().map(|k|(k.clone(),family_row[k].clone())).collect());let mut payload=json!({"family":family,"ref":p["deviceRef"]});payload.as_object_mut().unwrap().extend(proposed.as_object().unwrap().clone());if let Some(v)=row.device.get("objectIdentity"){payload["expectedObjectIdentity"]=v.clone()}payload["expectedStateRevision"]=json!(digest(&state)?);
   let t=json!({"id":tempo::transaction_id("devspec"),"epoch":status.epoch,"kind":"device-specialized","fence":fence(family,&p["deviceRef"],&row.device,fields),"payload":payload,"prior":prior,"expiresAt":kumi_common::time::now_ms_f64()+TRANSACTION_TTL_MS,"state":"previewed"});self.retain_bounded_transaction(&self.clip_lifecycle_transactions,t.clone(),"specialized device")?;let (prior,proposed)=if family=="drift"{(shown_voice_count(prior,family_row),shown_voice_count(proposed,family_row))}else{(prior,proposed)};Ok(success_text(id,&json!({"transactionId":t["id"],"epoch":t["epoch"],"family":family,"deviceRef":p["deviceRef"],"prior":prior,"proposed":proposed,"impact":format!("edits-{family}"),"confirmation":"apply","expiresAt":t["expiresAt"]})))
  }.await;
        result.unwrap_or_else(|e| adapter_tool_error(id, &e, "Specialized-device preview requires fresh authoritative state."))
    }
    pub async fn live_device_specialized_apply_async(&self, id: &Value, p: &Value, signal: Option<&Signal>) -> Option<Value> {
        if !valid_transaction_params(p, "apply") {
            return Some(error(id, -32602, "transactionId, confirmation=apply, and idempotencyKey are required", None));
        }
        let record = self.clip_lifecycle_transactions.get(p["transactionId"].as_str().unwrap());
        let t = record.as_ref().map(|r| r.borrow().clone()).unwrap_or(Value::Null);
        if t["kind"] != "device-specialized"
            || (t["state"] == "previewed" && t["expiresAt"].as_f64().is_some_and(|n| n <= kumi_common::time::now_ms_f64()))
        {
            return Some(transaction_error(id, "Unknown or expired specialized-device transaction"));
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
            let family = t["payload"]["family"].as_str().unwrap_or("");
            let fields = family_fields(family).unwrap_or(&[]);
            let reference = t["payload"]["ref"].as_str().unwrap_or("");
            if !reconcile {
                let snapshot = self.views.view_for(Some(&context), &[t["payload"]["ref"].clone()], None, &[]).await?;
                let row = self.device_row(&snapshot, reference)?;
                if t["fence"] != fence(family, &t["payload"]["ref"], &row.device, fields) {
                    return Ok(transaction_error(id, "device identity or state changed since preview; preview again"));
                }
            }
            {
                let mut row = record.borrow_mut();
                row["state"] = json!("applying");
                row["applyKey"] = p["idempotencyKey"].clone()
            }
            let mut args = t["payload"].clone();
            args.as_object_mut().unwrap().remove("family");
            let result = adapter.invoke_async(&LiveInvocation::new(&format!("{family}.set"), args), Some(&context)).await?;
            if result.is_null() {
                return Err(LiveError::type_error("Cannot read properties of null (reading 'changed')"));
            }
            if result["changed"] != true {
                return Err(LiveError::error("specialized device change was not confirmed"));
            }
            let snapshot = self.views.view_for(Some(&context), &[t["payload"]["ref"].clone()], None, &[]).await?;
            let device = self.device_row(&snapshot, reference)?.device;
            let verified = &device[row_key(family)];
            for field in fields {
                if let Some(value) = t["payload"].get(*field) {
                    if !same_live_value(verified.get(*field), Some(value)) {
                        return Err(LiveError::error("specialized device postcondition was not confirmed"));
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
            adapter_tool_error(id, &e, "Specialized-device state is uncertain; perform fresh discovery before retrying.")
        }))
    }
    pub async fn undo_specialized_device_async(&self, id: &Value, p: &Value, signal: Option<&Signal>) -> Value {
        let record = p["transactionId"].as_str().and_then(|id| self.clip_lifecycle_transactions.get(id));
        let t = record.as_ref().map(|r| r.borrow().clone()).unwrap_or(Value::Null);
        if t["kind"] != "device-specialized" {
            return transaction_error(id, "Unknown or expired specialized-device transaction");
        }
        let record = record.unwrap();
        if t["state"] == "undone" && t["undoKey"] == p["idempotencyKey"] {
            return success_text(id, &json!({"transactionId":t["id"],"state":"undone","idempotent":true}));
        }
        let reconcile = t["state"] == "uncertain" && t["undoKey"] == p["idempotencyKey"];
        if (t["state"] != "applied" && !reconcile) || !truthy(&t["prior"]) {
            return transaction_error(id, "Only an applied or exact-key uncertain specialized-device transaction can be undone");
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
            let family = t["payload"]["family"].as_str().unwrap_or("");
            let fields = family_fields(family).unwrap_or(&[]);
            let snapshot = self.views.view_for(Some(&context), &[t["payload"]["ref"].clone()], None, &[]).await?;
            let row = self.device_row(&snapshot, t["payload"]["ref"].as_str().unwrap_or(""))?;
            if let Some(moved) = self.undo_target_moved(
                id,
                &record.borrow(),
                "device",
                &t["payload"]["ref"],
                row.device.get("objectIdentity"),
                t["payload"].get("expectedObjectIdentity"),
            )? {
                return Ok(moved);
            }
            let current = &row.device[row_key(family)];
            if !reconcile {
                for field in fields {
                    if let Some(v) = t["payload"].get(*field) {
                        if !same_live_value(current.get(*field), Some(v)) {
                            return Ok(transaction_error(id, "device state changed after apply; undo refused"));
                        }
                    }
                }
            }
            record.borrow_mut()["state"] = json!("undoing");
            let mut args = json!({"ref":t["payload"]["ref"]});
            args.as_object_mut().unwrap().extend(t["prior"].as_object().unwrap().clone());
            if let Some(v) = row.device.get("objectIdentity") {
                args["expectedObjectIdentity"] = v.clone()
            }
            args["expectedStateRevision"] = json!(digest(&state(fields, current))?);
            let result = self.invoke_undo_recovery(&record, adapter.as_ref(), &format!("{family}.set"), &args, &context).await?;
            if result.is_null() {
                return Err(LiveError::type_error("Cannot read properties of null (reading 'changed')"));
            }
            if result["changed"] != true {
                return Err(LiveError::error("specialized device restoration was not confirmed"));
            }
            record.borrow_mut()["state"] = json!("undone");
            Ok(success_text(id, &json!({"transactionId":t["id"],"state":"undone","idempotent":false})))
        }
        .await;
        result.unwrap_or_else(|e| {
            record.borrow_mut()["state"] = json!("uncertain");
            adapter_tool_error(id, &e, "Specialized-device undo is uncertain; perform fresh discovery.")
        })
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn drift_row() -> Value {
        json!({
            "voiceCount": 1,
            "voiceCountList": ["4", "8", "16", "24", "32"],
            "voiceModeList": ["Poly", "Mono", "Stereo", "Unison"],
            "modSources": ["Env 1", "Env 2", "LFO", "Key", "Vel", "Mod", "Press", "Slide"],
        })
    }
    #[test]
    fn drift_takes_a_number_of_voices_at_its_place_in_lives_list() {
        let row = drift_row();
        let mut proposed = json!({"voiceCount": 32, "voiceMode": 3, "modSource1": 7});
        drift_choices(&mut proposed, &row).unwrap();
        assert_eq!(proposed, json!({"voiceCount": 4, "voiceMode": 3, "modSource1": 7}), "Live is sent 32's place in the list");
        assert_eq!(shown_voice_count(proposed, &row)["voiceCount"], 32, "the preview shows the voices asked for");
        assert_eq!(shown_voice_count(json!({"voiceCount": row["voiceCount"]}), &row), json!({"voiceCount": 8}), "and what Drift has now");
    }
    #[test]
    fn drift_refuses_a_count_or_a_place_its_lists_dont_have() {
        let row = drift_row();
        let refused = |proposed: Value| drift_choices(&mut proposed.clone(), &row).unwrap_err();
        assert_eq!(refused(json!({"voiceCount": 5})), "voiceCount 5 isn't one of this Drift's voice counts (4, 8, 16, 24, 32)");
        assert_eq!(
            refused(json!({"voiceMode": 4})),
            "voiceMode 4 isn't a place in this Drift's voiceModeList (0 Poly, 1 Mono, 2 Stereo, 3 Unison)"
        );
        assert!(refused(json!({"modSource3": 8})).starts_with("modSource3 8 isn't a place in this Drift's modSources (0 Env 1,"));
        assert_eq!(
            refused(json!({"voiceCount": 4})),
            "Kumi's Drift tool can't set 4 voices yet (8, 16, 24 or 32 work); run_python can set the device's voice_count_index to 0",
            "the protocol takes Drift's voice count from place 1"
        );
        let mut older = json!({"voiceCount": 3, "voiceMode": 6});
        drift_choices(&mut older, &json!({"voiceCount": 2})).unwrap();
        assert_eq!(older, json!({"voiceCount": 3, "voiceMode": 6}), "a row without the lists passes as it is");
    }
    #[test]
    fn wavetable_checks_its_category_and_the_categorys_wavetables() {
        let row = json!({"oscillator1WavetableCategory": 1, "categories": ["Basics", "Collection", "Complex"], "oscillator1Wavetables": ["A", "B"]});
        assert!(wavetable_choices(&json!({"oscillator1WavetableIndex": 1}), &row).is_ok());
        assert_eq!(
            wavetable_choices(&json!({"oscillator1WavetableIndex": 2}), &row).unwrap_err(),
            "oscillator1WavetableIndex 2 isn't a place in this Wavetable's oscillator1Wavetables (0 A, 1 B)"
        );
        assert!(wavetable_choices(&json!({"oscillator1WavetableCategory": 1, "oscillator1WavetableIndex": 2}), &row).is_err());
        assert!(
            wavetable_choices(&json!({"oscillator1WavetableCategory": 2, "oscillator1WavetableIndex": 5}), &row).is_ok(),
            "another category's wavetables aren't known yet"
        );
        assert!(wavetable_choices(&json!({"oscillator1WavetableCategory": 3}), &row).is_err());
        let long = json!({"categories": (0..30).map(|n| format!("C{n}")).collect::<Vec<_>>()});
        assert!(wavetable_choices(&json!({"oscillator2WavetableCategory": 30}), &long).unwrap_err().ends_with("23 C23, … 30 in all)"));
    }
    #[test]
    fn settings_without_a_list_from_live_are_bounded_by_their_choices() {
        assert_eq!(bounds("unison"), Some((1., 3., true)), "off waits for the protocol");
        assert_eq!(bounds("polyphony"), Some((1., 6., true)), "two voices wait for the protocol");
        assert_eq!(bounds("unisonMode"), Some((0., 6., true)));
        assert_eq!(bounds("unisonVoiceCount"), Some((2., 8., true)));
        assert_eq!(bounds("voiceCount"), Some((1., 64., true)));
        assert_eq!(
            out_of_bounds("polyphony", &json!(9)),
            "polyphony is out of bounds; its choices are 1 three, 2 four, 3 five, 4 six, 5 eight or 6 twelve (0 two only through run_python for now)"
        );
        assert_eq!(
            out_of_bounds("unison", &json!(0)),
            "Kumi's Meld tool can't set unison to 0 (off) yet (1 two, 2 three or 3 four work); run_python can set the device's unison_voices to 0"
        );
        assert_eq!(
            out_of_bounds("filterRouting", &json!(3)),
            "filterRouting is out of bounds; its choices are 0 Serial, 1 Parallel or 2 Split"
        );
        assert_eq!(out_of_bounds("pitchBendRange", &json!(200)), "pitchBendRange is out of bounds");
    }
    #[test]
    fn the_catalog_describes_each_choice_as_the_bridge_bounds_it() {
        let entry = crate::tool_catalog::TOOL_CATALOG.iter().find(|e| e.name == "live_device_specialized_preview").unwrap();
        let properties = &entry.input_schema["properties"];
        for field in
            ["unison", "polyphony", "filterRouting", "oscillator1EffectMode", "oscillator2EffectMode", "unisonMode", "unisonVoiceCount"]
        {
            let (low, high, _) = bounds(field).unwrap();
            assert_eq!((properties[field]["minimum"].as_f64(), properties[field]["maximum"].as_f64()), (Some(low), Some(high)), "{field}");
            let described = properties[field]["description"].as_str().unwrap();
            for (place, name) in fixed_choices(field).unwrap_or_default().iter().enumerate() {
                assert!(described.contains(&format!("{place} {name}")), "{field}: {described}");
            }
        }
    }
}
