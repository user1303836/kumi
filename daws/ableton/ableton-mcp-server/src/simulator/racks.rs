use super::simulator_device_state::{nested_device_path, pad_path, top_device_path};
use super::simulator_views::{fence, fields, set_bool, set_number};
use super::*;
use serde_json::json;
fn rack_state(device: &Value) -> Value {
    let mut row = fields(device, &["visibleMacroCount", "selectedVariationIndex", "variationCount"]);
    for key in ["macros", "chains", "drumPads"] {
        row[key] = Value::Array(array(&device[key]).iter().map(|r| r["objectIdentity"].clone()).collect());
    }
    row
}
impl DeterministicLiveSimulator {
    pub(super) fn load_drum_pad_sample(&self, args: &Map<String, Value>, operation: &str) -> Result<Value, LiveError> {
        let reference = string_arg(args, "ref")?;
        let drum_sampler = args.get("instrument") == Some(&json!("Drum Sampler"));
        let key = if drum_sampler { "presetItemId" } else { "samplePath" };
        let sample = args
            .get(key)
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty() && kumi_common::js::string::utf16_len(s) <= 1024)
            .ok_or_else(|| LiveError::type_error(format!("{key} must be a non-empty string")))?;
        let mut state = self.state.borrow_mut();
        let path = pad_path(&state, reference).ok_or_else(|| LiveError::error("drum pad reference is stale or invalid"))?;
        let pad = state.pointer_mut(&path).unwrap();
        if pad.get("objectIdentity") != args.get("expectedObjectIdentity") {
            return Err(LiveError::error("drum pad identity changed since preview"));
        }
        if !array(&pad["chains"]).is_empty() {
            return Err(LiveError::error("drum pad already has a sound; choose an empty pad"));
        }
        let chain_ref = format!("chain:{reference}:0");
        let chain_id = format!("simulator:chain:{}", self.next_sequence());
        let device_id = format!("simulator:device:{}", self.next_sequence());
        let device = json!({"ref":format!("device:{chain_ref}:0"),"parentRef":chain_ref,"name":if drum_sampler{args.get("name").and_then(Value::as_str).unwrap_or("Drum Sampler")}else{"Simpler"},"kind":"instrument","className":if drum_sampler{"DrumCell"}else{"OriginalSimpler"},"parameters":[],"objectIdentity":device_id,"enabled":true,"samplePath":sample});
        let chain = json!({"ref":chain_ref,"parentRef":reference,"name":args.get("name").and_then(Value::as_str).unwrap_or("Simpler"),"objectIdentity":chain_id,"devices":[device]});
        pad["chains"].as_array_mut().unwrap().push(chain);
        let identity = pad["objectIdentity"].clone();
        drop(state);
        self.emit(LiveEventType::Object, Some(reference.into()), json!({"operation":operation}));
        Ok(
            json!({"ref":reference,"objectIdentity":identity,"chainIdentity":chain_id,"deviceIdentity":device_id,"samplePath":sample,"route":if drum_sampler{"preset"}else{"chain"},"tried":[]}),
        )
    }
    pub(super) fn invoke_racks(&self, operation: &str, args: &Map<String, Value>) -> Result<Value, LiveError> {
        if operation == "drum-pad.load-sample" {
            return self.load_drum_pad_sample(args, operation);
        }
        if operation == "drum-pad.load-samples" {
            let pads = args
                .get("pads")
                .and_then(Value::as_array)
                .filter(|a| !a.is_empty() && a.len() <= 128)
                .ok_or_else(|| LiveError::error("drum pad authority is invalid"))?;
            let mut loaded: Vec<Value> = Vec::new();
            for (index, pad) in pads.iter().enumerate() {
                let result = if pad.is_null() {
                    Err(LiveError::type_error("Cannot read properties of null (reading 'ref')"))
                } else {
                    self.load_drum_pad_sample(pad.as_object().unwrap_or(&Map::new()), operation)
                };
                match result {
                    Ok(result) => loaded.push(result),
                    Err(error) => {
                        let mut state = self.state.borrow_mut();
                        for prior in loaded.iter().rev() {
                            if let Some(path) = pad_path(&state, prior["ref"].as_str().unwrap()) {
                                state.pointer_mut(&path).unwrap()["chains"] = json!([]);
                            }
                        }
                        return Err(LiveError::error(format!("drum pad {} of {}: {error}", index + 1, pads.len())));
                    }
                }
            }
            return Ok(json!({"pads":loaded}));
        }
        let reference = string_arg(args, "ref")?;
        let mut state = self.state.borrow_mut();
        if operation == "drum-pad.set" || operation == "drum-pad.delete-all-chains" {
            let path = pad_path(&state, reference).ok_or_else(|| LiveError::error("drum pad reference is stale or invalid"))?;
            let pad = state.pointer_mut(&path).unwrap();
            if pad.get("objectIdentity") != args.get("expectedObjectIdentity") {
                return Err(LiveError::error("drum pad identity changed since preview"));
            }
            let result = if operation == "drum-pad.set" {
                fence(args, &fields(pad, &["note", "solo"]), "drum pad")?;
                if args.contains_key("note") {
                    return Err(LiveError::range_error("DrumPad.note is read-only in the public LOM and cannot be assigned"));
                }
                set_bool(pad, args, "solo", "solo")?;
                json!({"changed":true})
            } else {
                if args.get("expectedStateRevision")
                    != Some(&json!(simulator_revision(&Value::Array(
                        array(&pad["chains"]).iter().map(|c| c["objectIdentity"].clone()).collect()
                    ))))
                {
                    return Err(LiveError::error("drum pad chain collection changed since preview"));
                }
                let count = array(&pad["chains"]).len();
                pad["chains"] = json!([]);
                json!({"deleted":count})
            };
            drop(state);
            self.emit(LiveEventType::Object, Some(reference.into()), json!({"operation":operation}));
            let mut result = result;
            if operation == "drum-pad.set" {
                result["revision"] = self.next_sequence().into();
            }
            return Ok(result);
        }
        let path = if operation == "rack.action" { nested_device_path(&state, reference) } else { top_device_path(&state, reference) };
        let path = path.filter(|p| state.pointer(p).unwrap()["canHaveChains"] == true).ok_or_else(|| {
            LiveError::error(match operation {
                "rack.set" => "rack operations require a rack device",
                "rack.action" => "rack actions require a rack device",
                _ => "rack view requires a rack device",
            })
        })?;
        if state.pointer(&path).unwrap().get("objectIdentity") != args.get("expectedObjectIdentity") {
            return Err(LiveError::error("rack identity changed since preview"));
        }
        let valid_chain =
            args.get("selectedChainRef").is_none_or(|r| r.is_null() || super::simulator_devices::chain_path(&state, r).is_some());
        let device = state.pointer_mut(&path).unwrap();
        let mut result = json!({"changed":true});
        match operation {
            "rack.set" => {
                fence(args, &rack_state(device), "rack")?;
                if args.contains_key("visibleMacroCount") {
                    return Err(LiveError::range_error("RackDevice.visible_macro_count is read-only in the public LOM; use rack.action add-macro/remove-macro to change it"));
                }
                set_number(device, args, "selectedVariationIndex", -1., 256., true)?;
            }
            "rack.view.set" => {
                fence(args, &fields(&device["rackView"], &["padScrollPosition", "showChainDevices"]), "rack view")?;
                if device["rackView"].is_null() {
                    device["rackView"] = json!({});
                }
                if let Some(value) = args.get("selectedChainRef") {
                    if !valid_chain {
                        return Err(LiveError::error("selectedChainRef is stale or invalid"));
                    }
                    device["rackView"]["selectedChainRef"] = value.clone();
                }
                set_number(&mut device["rackView"], args, "selectedPadIndex", -1., 127., true)?;
                set_number(&mut device["rackView"], args, "padScrollPosition", 0., 127., true)?;
                set_bool(&mut device["rackView"], args, "showChainDevices", "showChainDevices")?;
            }
            "rack.action" => {
                fence(args, &rack_state(device), "rack")?;
                let action = args.get("action").and_then(Value::as_str);
                if action == Some("remove-macro") && args.contains_key("index") {
                    return Err(LiveError::range_error("this rack action takes no index in the public LOM"));
                }
                if action.is_some_and(|a| ["recall-variation", "delete-variation"].contains(&a)) {
                    if let Some(index) = args.get("index") {
                        let value = ranged_number(index, 0., f64::INFINITY, true, "variation index is out of range")?;
                        if value >= device["variationCount"].as_f64().unwrap_or(0.) {
                            return Err(LiveError::range_error("variation index is out of range"));
                        }
                        device["selectedVariationIndex"] = index.clone();
                    }
                }
                match action {
                    Some("add-macro") => {
                        device["visibleMacroCount"] = (device["visibleMacroCount"].as_i64().unwrap_or(0) + 1).min(16).into();
                        let count = array(&device["macros"]).len();
                        let macro_row = json!({"ref":format!("parameter:{reference}:macro:{count}"),"objectIdentity":format!("simulator:parameter:{}",self.sequence.get()+1),"name":format!("Macro {}",count+1),"value":0});
                        if device["macros"].is_null() {
                            device["macros"] = json!([]);
                        }
                        device["macros"].as_array_mut().unwrap().push(macro_row);
                    }
                    Some("remove-macro") => {
                        let count = device["visibleMacroCount"].as_i64().unwrap_or(1);
                        if count <= 1 {
                            return Err(LiveError::error("cannot remove the last macro"));
                        }
                        device["visibleMacroCount"] = (count - 1).into();
                        if let Some(macros) = device.get_mut("macros").and_then(Value::as_array_mut) {
                            macros.pop();
                        }
                    }
                    Some("randomize-macros") => {
                        if let Some(macros) = device.get_mut("macros").and_then(Value::as_array_mut) {
                            for m in macros {
                                m["value"] = (kumi_common::js::number::round(rand::random::<f64>() * 100.) / 100.).into();
                            }
                        }
                    }
                    Some("insert-chain") => {
                        let index = array(&device["chains"]).len();
                        let chain = json!({"ref":format!("chain:{reference}:{index}"),"parentRef":reference,"objectIdentity":format!("simulator:chain:{}",self.sequence.get()+1),"index":index,"name":"New Chain","mute":false,"solo":false,"devices":[]});
                        if device["chains"].is_null() {
                            device["chains"] = json!([]);
                        }
                        device["chains"].as_array_mut().unwrap().push(chain);
                    }
                    Some("copy-pad") => {
                        for key in ["sourceIndex", "targetIndex"] {
                            ranged_number(
                                args.get(key).unwrap_or(&Value::Null),
                                0.,
                                f64::INFINITY,
                                true,
                                "copy-pad requires sourceIndex and targetIndex",
                            )?;
                        }
                    }
                    Some("store-variation") => device["variationCount"] = (device["variationCount"].as_i64().unwrap_or(0) + 1).into(),
                    Some("recall-variation") => {}
                    Some("delete-variation") => {
                        device["variationCount"] = (device["variationCount"].as_i64().unwrap_or(0) - 1).max(0).into()
                    }
                    _ => return Err(LiveError::range_error("rack action is invalid")),
                }
                result = json!({"done":true});
                if action == Some("insert-chain") {
                    let chain = array(&device["chains"]).last().unwrap();
                    result["chainRef"] = chain["ref"].clone();
                    result["chainObjectIdentity"] = chain["objectIdentity"].clone();
                }
            }
            _ => unreachable!("rack dispatcher routes only implemented operations"),
        }
        drop(state);
        self.emit(LiveEventType::Object, Some(reference.into()), json!({"operation":operation}));
        result["revision"] = self.next_sequence().into();
        Ok(result)
    }
}
