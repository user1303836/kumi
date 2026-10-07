use super::*;
use serde_json::json;

fn siblings(devices: &Value) -> Value {
    Value::Array(
        array(devices)
            .iter()
            .map(|d| {
                let mut row = json!({"ref":d["ref"]});
                if let Some(id) = d.get("objectIdentity") {
                    row["objectIdentity"] = id.clone();
                }
                row
            })
            .collect(),
    )
}
fn canonical_expected(args: &Map<String, Value>, key: &str, current: &Value) -> Result<bool, LiveError> {
    let expected = args.get(key).ok_or_else(|| LiveError::error("unsupported simulator authority value"))?;
    Ok(simulator_revision(expected) == simulator_revision(current))
}
fn require_siblings(args: &Map<String, Value>, devices: &Value) -> Result<(), LiveError> {
    let expected = args
        .get("expectedSiblings")
        .and_then(Value::as_array)
        .filter(|a| a.len() <= 256 && a.iter().all(|v| v.is_object() && v["ref"].is_string() && v["objectIdentity"].is_string()))
        .ok_or_else(|| LiveError::type_error("expected device siblings are invalid"))?;
    if kumi_common::js::json::stringify(&siblings(devices)) != kumi_common::js::json::stringify(&Value::Array(expected.clone())) {
        return Err(LiveError::error("device siblings changed since preview"));
    }
    Ok(())
}
fn track_path(state: &Value, reference: &Value) -> Option<String> {
    array(&state["tracks"]).iter().position(|t| t.get("ref") == Some(reference)).map(|i| format!("/tracks/{i}"))
}
pub(super) fn chain_path(state: &Value, reference: &Value) -> Option<String> {
    for (ti, t) in array(&state["tracks"]).iter().enumerate() {
        for (di, d) in array(&t["devices"]).iter().enumerate() {
            for (ci, c) in array(&d["chains"]).iter().enumerate() {
                if c.get("ref") == Some(reference) {
                    return Some(format!("/tracks/{ti}/devices/{di}/chains/{ci}"));
                }
            }
        }
    }
    None
}
/// `wanted`, or `wanted` with a suffix when a row in `rows` has it already: a ref built from a position can repeat
/// one a device or clip kept after the others moved.
pub(super) fn unique_ref(rows: &Value, wanted: String) -> String {
    let taken = |candidate: &str| array(rows).iter().any(|row| row["ref"] == candidate || row["clip"]["ref"] == candidate);
    if !taken(&wanted) {
        return wanted;
    }
    (2..).map(|n| format!("{wanted}-{n}")).find(|candidate| !taken(candidate)).unwrap()
}
fn device_fingerprint(device: &Value) -> Result<String, LiveError> {
    Ok(simulator_revision(&owned_device_fingerprint_row(device)))
}
impl DeterministicLiveSimulator {
    pub(super) fn invoke_devices(&self, operation: &str, args: &Map<String, Value>) -> Result<Value, LiveError> {
        match operation {
            "device.insert" => {
                let chained = args.get("chainRef").is_some_and(|v| !v.is_null());
                let track_ref = string_arg(args, "trackRef")?;
                let mut state = self.state.borrow_mut();
                let tp = track_path(&state, &json!(track_ref));
                if chained {
                    let tp = tp
                        .filter(|p| state.pointer(p).unwrap().get("objectIdentity") == args.get("expectedTrackIdentity"))
                        .ok_or_else(|| LiveError::error("track identity changed since preview"))?;
                    let _ = tp;
                    let cp =
                        chain_path(&state, &args["chainRef"]).ok_or_else(|| LiveError::error("chain reference is stale or invalid"))?;
                    let name = string_arg(args, "deviceName")?;
                    let chain = state.pointer_mut(&cp).unwrap();
                    if !canonical_expected(args, "expectedSiblings", &siblings(&chain["devices"]))? {
                        return Err(LiveError::error("chain device collection changed since preview"));
                    }
                    let index = array(&chain["devices"]).len();
                    let reference = chain["ref"].as_str().unwrap().to_string();
                    let device = json!({"ref":unique_ref(&chain["devices"], format!("device:{reference}:{index}")),"parentRef":reference,"name":name,"kind":if name.to_lowercase().contains("rack"){"rack"}else{"device"},"className":name,"parameters":[],"objectIdentity":format!("simulator:device:{}:{reference}:{index}",self.sequence.get()+1),"enabled":true});
                    chain["devices"].as_array_mut().unwrap().push(device.clone());
                    drop(state);
                    self.emit(LiveEventType::Object, Some(reference.into()), json!({"operation":operation,"device":device}));
                    return Ok(
                        json!({"ref":device["ref"],"objectIdentity":device["objectIdentity"],"name":name,"index":index,"createdFingerprint":device_fingerprint(&device)?}),
                    );
                }
                let tp = tp
                    .filter(|p| state.pointer(p).unwrap().get("objectIdentity") == args.get("expectedTrackIdentity"))
                    .ok_or_else(|| LiveError::error("device insertion target changed since preview"))?;
                let track = state.pointer_mut(&tp).unwrap();
                if !canonical_expected(args, "expectedSiblings", &siblings(&track["devices"]))? {
                    return Err(LiveError::error("device insertion target changed since preview"));
                }
                let name = string_arg(args, "deviceName")?;
                let index = ranged_number(
                    args.get("index").filter(|v| !v.is_null()).unwrap_or(&json!(-1)),
                    -1.,
                    256.,
                    true,
                    "device index is invalid",
                )? as i64;
                let count = array(&track["devices"]).len();
                let reference = track["ref"].as_str().unwrap();
                let identity = format!("simulator:device:{}:{reference}:{count}", self.sequence.get() + 1);
                let mut device = json!({"ref":format!("device:{reference}:{count}"),"parentRef":reference,"name":name,"kind":if name.to_lowercase().contains("rack"){"rack"}else{"device"},"className":name,"parameters":[],"objectIdentity":identity,"enabled":true,"canHaveChains":name.to_lowercase().contains("rack"),"canHaveDrumPads":name.to_lowercase().contains("drum rack")});
                if device["canHaveDrumPads"] == true {
                    device["drumPads"]=Value::Array((0..16).map(|i|json!({"ref":format!("drum_pad:{}:{i}",device["ref"].as_str().unwrap()),"parentRef":device["ref"],"index":i,"name":format!("Pad {}",i+1),"mute":false,"chains":[],"note":36+i,"solo":false,"objectIdentity":format!("{identity}:pad:{i}")})).collect());
                }
                let position = if index < 0 || index as usize > count { count } else { index as usize };
                device["ref"] = unique_ref(&track["devices"], format!("device:{reference}:{position}")).into();
                if let Some(sample) = args.get("samplePath").filter(|v| v.is_string()) {
                    device["samplePath"] = sample.clone();
                }
                track["devices"].as_array_mut().unwrap().insert(position, device.clone());
                drop(state);
                self.emit(LiveEventType::Object, Some(track_ref.into()), json!({"operation":operation,"device":device}));
                let mut result = json!({"ref":device["ref"],"objectIdentity":device["objectIdentity"],"name":name,"index":position,"createdFingerprint":device_fingerprint(&device)?});
                if let Some(sample) = args.get("samplePath").filter(|v| v.is_string()) {
                    result["samplePath"] = sample.clone();
                }
                Ok(result)
            }
            "device.delete" => {
                let reference = string_arg(args, "ref")?;
                let identity = string_arg(args, "expectedObjectIdentity")?;
                let owner_ref = string_arg(args, "expectedOwnerRef")?;
                let owner_identity = string_arg(args, "expectedOwnerIdentity")?;
                let mut state = self.state.borrow_mut();
                for ti in 0..array(&state["tracks"]).len() {
                    let tp = format!("/tracks/{ti}");
                    let track = state.pointer(&tp).unwrap();
                    if args.get("expectedTrackRef") != track.get("ref") || args.get("expectedTrackIdentity") != track.get("objectIdentity")
                    {
                        continue;
                    }
                    let mut owners = vec![tp];
                    let mut at = 0;
                    while at < owners.len() && at < 4096 {
                        let owner = state.pointer(&owners[at]).unwrap();
                        for (di, d) in array(&owner["devices"]).iter().enumerate() {
                            for (ci, _) in array(&d["chains"]).iter().enumerate() {
                                owners.push(format!("{}/devices/{di}/chains/{ci}", owners[at]));
                            }
                        }
                        at += 1;
                    }
                    for path in owners {
                        let owner = state.pointer_mut(&path).unwrap();
                        let index = array(&owner["devices"]).iter().position(|d| {
                            d["ref"] == reference
                                && d["objectIdentity"] == identity
                                && d["parentRef"] == owner_ref
                                && owner["ref"] == owner_ref
                                && owner["objectIdentity"] == owner_identity
                        });
                        if let Some(index) = index {
                            require_siblings(args, &owner["devices"])?;
                            owner["devices"].as_array_mut().unwrap().remove(index);
                            let owner_ref = owner["ref"].as_str().unwrap().to_string();
                            drop(state);
                            self.emit(LiveEventType::Object, Some(owner_ref.into()), json!({"operation":operation,"ref":reference}));
                            return Ok(json!({"deleted":reference}));
                        }
                    }
                }
                Err(LiveError::error("unknown device reference"))
            }
            "device.enable" => {
                let reference = string_arg(args, "ref")?;
                let mut state = self.state.borrow_mut();
                let path = find_live_path(&state, reference);
                let tp = array(&state["tracks"])
                    .iter()
                    .position(|t| array(&t["devices"]).iter().any(|d| d["ref"] == reference))
                    .map(|i| format!("/tracks/{i}"));
                let valid = if let (Some(path), Some(tp)) = (&path, &tp) {
                    let d = state.pointer(path).unwrap();
                    let t = state.pointer(tp).unwrap();
                    d.get("parameters").is_some()
                        && d["objectIdentity"] == string_arg(args, "expectedObjectIdentity")?
                        && d["parentRef"] == string_arg(args, "expectedOwnerRef")?
                        && t["objectIdentity"] == string_arg(args, "expectedOwnerIdentity")?
                        && args.get("expectedTrackRef") == t.get("ref")
                        && args.get("expectedTrackIdentity") == t.get("objectIdentity")
                } else {
                    false
                };
                if !valid {
                    return Err(LiveError::error("unknown, replaced, or reparented device reference"));
                }
                require_siblings(args, &state.pointer(&tp.unwrap()).unwrap()["devices"])?;
                let d = state.pointer_mut(&path.unwrap()).unwrap();
                if args.get("expectedStateRevision") != Some(&json!(simulator_revision(&json!({"enabled":d["enabled"]})))) {
                    return Err(LiveError::error("device enable state changed since preview"));
                }
                let enabled =
                    args.get("enabled").filter(|v| v.is_boolean()).ok_or_else(|| LiveError::type_error("enabled must be boolean"))?.clone();
                d["enabled"] = enabled.clone();
                drop(state);
                self.emit(LiveEventType::Object, Some(reference.into()), json!({"operation":operation}));
                Ok(json!({"changed":true,"enabled":enabled,"revision":self.next_sequence()}))
            }
            "device.move" => {
                let reference = string_arg(args, "ref")?;
                let identity = string_arg(args, "expectedObjectIdentity")?;
                let owner_ref = string_arg(args, "expectedOwnerRef")?;
                let owner_identity = string_arg(args, "expectedOwnerIdentity")?;
                let index = ranged_number(args.get("index").unwrap_or(&Value::Null), 0., 256., true, "device index is invalid")? as usize;
                let mut state = self.state.borrow_mut();
                let target = if let Some(reference) = args.get("targetTrackRef").filter(|v| !v.is_null()) {
                    Some(track_path(&state, reference).ok_or_else(|| LiveError::error("move target reference is stale or invalid"))?)
                } else if let Some(reference) = args.get("targetChainRef").filter(|v| !v.is_null()) {
                    Some(chain_path(&state, reference).ok_or_else(|| LiveError::error("move target chain reference is stale or invalid"))?)
                } else {
                    None
                };
                if let Some(target) = target {
                    let target_row = state.pointer(&target).unwrap();
                    if target_row.get("objectIdentity") != args.get("expectedTargetIdentity") {
                        return Err(LiveError::error("move target identity changed since preview"));
                    }
                    let owner = target_row["ref"].as_str().unwrap().to_string();
                    let count = array(&target_row["devices"]).len();
                    for ti in 0..array(&state["tracks"]).len() {
                        if let Some(current) = array(&state["tracks"][ti]["devices"])
                            .iter()
                            .position(|d| d["ref"] == reference && d["objectIdentity"] == identity)
                        {
                            if index > count {
                                return Err(LiveError::range_error("device index is invalid"));
                            }
                            let device = state["tracks"][ti]["devices"].as_array_mut().unwrap().remove(current);
                            let devices = state.pointer_mut(&target).unwrap()["devices"].as_array_mut().unwrap();
                            devices.insert(index.min(devices.len()), device);
                            drop(state);
                            self.emit(LiveEventType::Object, Some(owner.into()), json!({"operation":operation}));
                            return Ok(json!({"ref":reference,"objectIdentity":identity,"index":index}));
                        }
                    }
                    return Err(LiveError::error("device move source identity changed since preview"));
                }
                for t in state["tracks"].as_array_mut().unwrap() {
                    let current = array(&t["devices"]).iter().position(|d| {
                        d["ref"] == reference
                            && d["objectIdentity"] == identity
                            && d["parentRef"] == owner_ref
                            && t["objectIdentity"] == owner_identity
                            && args.get("expectedTrackRef") == t.get("ref")
                            && args.get("expectedTrackIdentity") == t.get("objectIdentity")
                    });
                    if let Some(current) = current {
                        require_siblings(args, &t["devices"])?;
                        let devices = t["devices"].as_array_mut().unwrap();
                        if index >= devices.len() {
                            return Err(LiveError::range_error("device index is invalid"));
                        }
                        let d = devices.remove(current);
                        devices.insert(index, d);
                        let owner = t["ref"].as_str().unwrap().to_string();
                        drop(state);
                        self.emit(LiveEventType::Object, Some(owner.into()), json!({"operation":operation}));
                        return Ok(json!({"ref":reference,"objectIdentity":identity,"index":index}));
                    }
                }
                Err(LiveError::error("unknown device reference"))
            }
            "browser.load" => {
                let item_id = string_arg(args, "itemId")?;
                let catalog = Self::browser_catalog();
                let item = catalog.iter().find(|item| item["id"] == item_id);
                let item = item
                    .filter(|item| {
                        item["isDevice"] == true
                            && item.get("name") == args.get("expectedName")
                            && item.get("objectIdentity") == args.get("expectedItemIdentity")
                    })
                    .ok_or_else(|| LiveError::error("browser item identity is not an exact loadable device"))?;
                let track_ref = string_arg(args, "trackRef")?;
                let state = self.state.borrow();
                let tp = track_path(&state, &json!(track_ref));
                let track = tp.as_ref().and_then(|p| state.pointer(p));
                let insert = if let Some(chain) = args.get("chainRef").filter(|v| !v.is_null()) {
                    let cp = chain_path(&state, chain);
                    let found = cp.as_ref().and_then(|p| state.pointer(p));
                    let (track, chain) = track
                        .zip(found)
                        .filter(|(track, chain)| {
                            track.get("objectIdentity") == args.get("expectedTrackIdentity")
                                && chain.get("objectIdentity") == args.get("expectedChainIdentity")
                        })
                        .ok_or_else(|| LiveError::error("browser target chain changed since preview"))?;
                    if item["category"] == "instruments" && array(&chain["devices"]).iter().any(|d| d["kind"] == "instrument") {
                        return Err(LiveError::error(
                            "this chain already has an instrument, which Live would replace; add another chain for it",
                        ));
                    }
                    let mut insert = json!({"trackRef":track["ref"],"chainRef":chain["ref"],"deviceName":item["name"],"expectedTrackIdentity":track["objectIdentity"]});
                    if let Some(v) = args.get("expectedSiblings") {
                        insert["expectedSiblings"] = v.clone();
                    }
                    insert
                } else {
                    let track = track
                        .filter(|t| t.get("objectIdentity") == args.get("expectedTrackIdentity"))
                        .ok_or_else(|| LiveError::error("browser target track or devices changed since preview"))?;
                    if !canonical_expected(args, "expectedSiblings", &siblings(&track["devices"]))? {
                        return Err(LiveError::error("browser target track or devices changed since preview"));
                    }
                    json!({"trackRef":track["ref"],"deviceName":item["name"],"expectedTrackIdentity":track["objectIdentity"],"expectedSiblings":args["expectedSiblings"]})
                };
                drop(state);
                let inserted = self.invoke_operation("device.insert", insert.as_object().unwrap())?;
                Ok(
                    json!({"loaded":true,"deviceRef":inserted["ref"],"deviceObjectIdentity":inserted["objectIdentity"],"createdFingerprint":inserted["createdFingerprint"]}),
                )
            }
            _ => unreachable!("device dispatcher routes only implemented operations"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn a_new_ref_never_repeats_one_a_row_kept() {
        // Device 0 was deleted: the one left kept ref 1, and the next insert's position is 1 again.
        let rows = json!([{"ref":"device:chain:1"},{"clip":{"ref":"arrangement-clip:track:8"}}]);
        assert_eq!(unique_ref(&rows, "device:chain:0".into()), "device:chain:0");
        assert_eq!(unique_ref(&rows, "device:chain:1".into()), "device:chain:1-2");
        assert_eq!(unique_ref(&rows, "arrangement-clip:track:8".into()), "arrangement-clip:track:8-2");
    }
}
