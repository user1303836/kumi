use super::simulator_views::{fence, fields, set_bool, set_number};
use super::*;
use serde_json::json;
pub(super) fn top_device_path(state: &Value, reference: &str) -> Option<String> {
    for (ti, t) in array(&state["tracks"]).iter().enumerate() {
        if let Some(di) = array(&t["devices"]).iter().position(|d| d["ref"] == reference) {
            return Some(format!("/tracks/{ti}/devices/{di}"));
        }
    }
    None
}
pub(super) fn nested_device_path(state: &Value, reference: &str) -> Option<String> {
    let mut pending = Vec::new();
    for (ti, t) in array(&state["tracks"]).iter().enumerate() {
        for (di, _) in array(&t["devices"]).iter().enumerate() {
            pending.push(format!("/tracks/{ti}/devices/{di}"));
        }
    }
    let mut at = 0;
    while at < pending.len() && at < 16384 {
        let path = &pending[at];
        let device = state.pointer(path).unwrap();
        if device["ref"] == reference {
            return Some(path.clone());
        }
        let additions = array(&device["chains"])
            .iter()
            .enumerate()
            .flat_map(|(ci, c)| array(&c["devices"]).iter().enumerate().map(move |(di, _)| format!("{path}/chains/{ci}/devices/{di}")))
            .collect::<Vec<_>>();
        pending.extend(additions);
        at += 1;
    }
    None
}
pub(super) fn pad_path(state: &Value, reference: &str) -> Option<String> {
    for (ti, t) in array(&state["tracks"]).iter().enumerate() {
        for (di, d) in array(&t["devices"]).iter().enumerate() {
            if let Some(pi) = array(&d["drumPads"]).iter().position(|p| p["ref"] == reference) {
                return Some(format!("/tracks/{ti}/devices/{di}/drumPads/{pi}"));
            }
        }
    }
    None
}
impl DeterministicLiveSimulator {
    pub(super) fn invoke_device_state(&self, operation: &str, args: &Map<String, Value>) -> Result<Value, LiveError> {
        let reference = string_arg(args, "ref")?;
        let mut state = self.state.borrow_mut();
        if operation == "parameter.re-enable-automation" {
            let p = array(&state["tracks"])
                .iter()
                .flat_map(|t| array(&t["devices"]))
                .flat_map(|d| array(&d["parameters"]).iter().chain(array(&d["macros"])))
                .find(|p| p["ref"] == reference)
                .ok_or_else(|| LiveError::error("parameter reference is stale or invalid"))?;
            if p.get("objectIdentity") != args.get("expectedObjectIdentity") {
                return Err(LiveError::error("parameter identity changed since preview"));
            }
            // As Live: the parameter's own automation state ("none" when it has none).
            fence(
                args,
                &json!({"automationState":p.get("automationState").filter(|v| v.is_string()).cloned().unwrap_or(json!("none"))}),
                "parameter automation",
            )?;
            drop(state);
            self.emit(LiveEventType::Object, Some(reference.into()), json!({"operation":operation}));
            return Ok(json!({"done":true}));
        }
        if operation == "chain.set" || operation == "chain-mixer.set" {
            let path = super::simulator_devices::chain_path(&state, &json!(reference))
                .ok_or_else(|| LiveError::error("chain reference is stale or invalid"))?;
            let chain = state.pointer_mut(&path).unwrap();
            if operation == "chain-mixer.set" && chain["mixer"].is_null() {
                return Err(LiveError::error("chain mixer is unavailable"));
            }
            if chain.get("objectIdentity") != args.get("expectedObjectIdentity") {
                return Err(LiveError::error("chain identity changed since preview"));
            }
            if operation == "chain.set" {
                fence(args, &fields(chain, &["colorIndex", "autoColor", "mute", "solo"]), "chain")?;
                set_number(chain, args, "colorIndex", 0., 69., true)?;
                for key in ["autoColor", "mute", "solo"] {
                    set_bool(chain, args, key, key)?;
                }
            } else {
                let mixer = &mut chain["mixer"];
                if args.get("expectedMixerIdentity") != mixer.get("mixerIdentity") {
                    return Err(LiveError::error("chain mixer identity changed since preview"));
                }
                fence(args, &json!({"sends":mixer["sends"]}), "chain mixer")?;
                set_number(mixer, args, "volume", 0., 1., false)?;
                set_number(mixer, args, "pan", -1., 1., false)?;
                if let Some(sends) = args.get("sends") {
                    let sends = sends
                        .as_array()
                        .filter(|a| {
                            a.len() <= array(&mixer["sends"]).len()
                                && a.iter().all(|v| v.as_f64().is_some_and(|n| n.is_finite() && (0.0..=1.0).contains(&n)))
                        })
                        .ok_or_else(|| LiveError::range_error("sends are invalid"))?;
                    for (i, value) in sends.iter().enumerate() {
                        mixer["sends"][i] = value.clone();
                    }
                }
                set_bool(mixer, args, "chainActivator", "chainActivator")?;
            }
            drop(state);
            self.emit(LiveEventType::Object, Some(reference.into()), json!({"operation":operation}));
            return Ok(json!({"changed":true,"revision":self.next_sequence()}));
        }
        let path = top_device_path(&state, reference).ok_or_else(|| LiveError::error("device reference is stale or invalid"))?;
        let device = state.pointer_mut(&path).unwrap();
        if device.get("objectIdentity") != args.get("expectedObjectIdentity") {
            return Err(LiveError::error("device identity changed since preview"));
        }
        let mut extras = Map::new();
        let mut done = false;
        let mut action_payload = None;
        match operation {
            "device-io.set" => {
                let io = device
                    .get_mut("deviceIo")
                    .filter(|v| !v.is_null())
                    .ok_or_else(|| LiveError::error("device IO is unavailable on this shape"))?;
                fence(args, &json!({"routingType":io["routingType"],"routingChannel":io["routingChannel"]}), "device IO")?;
                for key in ["routingType", "routingChannel"] {
                    if let Some(v) = args.get(key) {
                        if !v.as_str().is_some_and(|s| !s.is_empty()) {
                            return Err(LiveError::range_error(format!("{key} is invalid")));
                        }
                        io[key] = v.clone();
                    }
                }
            }
            "compressor.sidechain.set" => {
                let value = device
                    .get("sidechainRoutingType")
                    .ok_or_else(|| LiveError::error("sidechain routing is unavailable on this device shape"))?;
                fence(args, &json!({"routingType":value}), "sidechain")?;
                let value = args
                    .get("routingType")
                    .filter(|v| v.as_str().is_some_and(|s| !s.is_empty()))
                    .ok_or_else(|| LiveError::range_error("routingType is invalid"))?;
                device["sidechainRoutingType"] = value.clone();
            }
            "device.bank.set" => {
                let count = device
                    .get("parameterBank")
                    .filter(|v| !v.is_null())
                    .ok_or_else(|| LiveError::error("parameter banks are unavailable on this device"))?;
                fence(args, &json!({"bankCount":count}), "device bank")?;
                let bank = ranged_number(args.get("bank").unwrap_or(&Value::Null), 0., 32., true, "bank is invalid")?;
                if let Some(script) = args.get("scriptIndex") {
                    ranged_number(script, 0., 16., true, "scriptIndex is invalid")?;
                }
                if count.as_f64().is_some_and(|c| bank >= c) {
                    return Err(LiveError::range_error("bank exceeds the device's parameter bank count"));
                }
                device["chosenBank"] = (bank as i64).into();
            }
            "device.comparison.save-to-slot" => {
                fence(
                    args,
                    &json!({"canCompareAb":device["comparison"]["capability"],"isUsingComparePresetB":device["comparison"]["activeSide"].as_f64()==Some(1.)}),
                    "comparison",
                )?;
                if device["comparison"]["capability"] != true {
                    return Err(LiveError::error("A/B comparison is unavailable on this device"));
                }
                done = true;
            }
            "simpler.replace-sample" => {
                let file = super::simulator_structure::absolute_audio_path(args.get("filePath"))?;
                fence(
                    args,
                    &json!({"filePath":device.get("samplePath").filter(|v|!v.is_null()).cloned().unwrap_or_else(||json!(""))}),
                    "simpler sample",
                )?;
                device["samplePath"] = file.clone().into();
                extras.insert("filePath".into(), file.into());
            }
            "drift.set" | "drum-cell.set" | "eq8.set" | "hybrid-reverb.set" | "looper.set" | "meld.set" | "plugin.set" => {
                let (key, names): (&str, &[&str]) = match operation {
                    "drift.set" => (
                        "drift",
                        &[
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
                    ),
                    "drum-cell.set" => ("drumCell", &["gain"]),
                    "eq8.set" => ("eq8", &["editMode", "globalMode", "oversample", "selectedBand"]),
                    "hybrid-reverb.set" => ("hybridReverb", &["irCategory", "irFile", "attack", "decay", "size"]),
                    "looper.set" => ("looper", &["overdubAfterRecord", "recordLengthIndex"]),
                    "meld.set" => ("meld", &["engine", "unison", "monoPoly", "polyphony"]),
                    "plugin.set" => ("plugin", &["presetIndex", "isEditorOpen"]),
                    _ => unreachable!(),
                };
                if device[key].is_null() {
                    device[key] = json!({});
                }
                let row = &mut device[key];
                fence(args, &fields(row, names), "device")?;
                for key in names {
                    let Some(value) = args.get(*key) else {
                        continue;
                    };
                    if ["oversample", "monoPoly", "isEditorOpen", "overdubAfterRecord"].contains(key) {
                        set_bool(row, args, key, key)?;
                    } else if ["irCategory", "irFile"].contains(key) {
                        if !value.as_str().is_some_and(|s| !s.is_empty()) {
                            return Err(LiveError::range_error(format!("{key} is invalid")));
                        }
                        let list = row.get(if *key == "irCategory" { "irCategoryList" } else { "irFileList" }).and_then(Value::as_array);
                        if list.is_none_or(|a| !a.contains(value)) {
                            return Err(LiveError::range_error(format!("{key} is not an available choice")));
                        }
                        row[*key] = value.clone();
                    } else {
                        set_number(
                            row,
                            args,
                            key,
                            f64::NEG_INFINITY,
                            f64::INFINITY,
                            [
                                "pitchBendRange",
                                "voiceCount",
                                "voiceMode",
                                "editMode",
                                "globalMode",
                                "selectedBand",
                                "engine",
                                "unison",
                                "polyphony",
                                "presetIndex",
                                "recordLengthIndex",
                            ]
                            .contains(key),
                        )?;
                    }
                }
            }
            "looper.action" => {
                if device["looper"].is_null() {
                    device["looper"] = json!({});
                }
                let row = &device["looper"];
                fence(args, &fields(row, &["overdubAfterRecord", "recordLengthIndex", "loopLength", "tempo", "state"]), "looper")?;
                let action = args
                    .get("action")
                    .and_then(Value::as_str)
                    .filter(|a| ["record", "overdub", "play", "stop", "clear", "undo", "double-speed", "half-speed", "export"].contains(a))
                    .ok_or_else(|| LiveError::range_error("looper action is invalid"))?;
                let length = row.get("loopLength").filter(|v| !v.is_null()).cloned().unwrap_or_else(|| json!(4));
                if action == "export" {
                    let slot_ref = args
                        .get("slotRef")
                        .and_then(Value::as_str)
                        .ok_or_else(|| LiveError::range_error("export requires an exact target clip slot"))?;
                    let (ti, si) = array(&state["tracks"])
                        .iter()
                        .enumerate()
                        .find_map(|(ti, t)| array(&t["clipSlots"]).iter().position(|s| s["ref"] == slot_ref).map(|si| (ti, si)))
                        .ok_or_else(|| LiveError::error("export target clip slot is stale or invalid"))?;
                    if state["tracks"][ti]["clipSlots"][si].get("clipRef").is_some_and(|v| !v.is_null()) {
                        return Err(LiveError::error("export target clip slot is not empty"));
                    }
                    let next = self.sequence.get() + 1;
                    let clip = json!({"ref":format!("clip:looper-export-{next}"),"objectIdentity":format!("simulator:clip:looper-export:{next}"),"name":"Looper Export","kind":"audio","start":0,"length":length,"notes":[],"warp":true,"takes":[],"automation":[],"muted":false});
                    state["tracks"][ti]["clipSlots"][si]["clipRef"] = clip["ref"].clone();
                    state["tracks"][ti]["clipSlots"][si]["empty"] = false.into();
                    state["tracks"][ti]["clips"].as_array_mut().unwrap().push(clip);
                } else if args.contains_key("slotRef") {
                    return Err(LiveError::range_error("slotRef is only valid for the export action"));
                }
                action_payload = Some(action.to_string());
                done = true;
            }
            _ => unreachable!("device-state dispatcher routes only implemented operations"),
        }
        drop(state);
        let mut payload = json!({"operation":operation});
        if let Some(action) = action_payload {
            payload["action"] = action.into();
        }
        self.emit(LiveEventType::Object, Some(reference.into()), payload);
        let mut result = if done { json!({"done":true}) } else { json!({"changed":true}) };
        if operation != "device.comparison.save-to-slot" {
            result["revision"] = self.next_sequence().into();
        }
        result.as_object_mut().unwrap().extend(extras);
        Ok(result)
    }
}
