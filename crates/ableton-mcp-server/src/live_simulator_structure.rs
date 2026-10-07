use super::*;
use serde_json::json;
struct Duplicator {
    sequence: u64,
    count: u64,
}
impl Duplicator {
    fn fresh(&mut self, row: &mut Value, kind: &str) {
        let reference = format!("{kind}:dup-{}-{}", self.sequence, self.count);
        self.count += 1;
        row["ref"] = reference.clone().into();
        row["objectIdentity"] = format!("simulator:{reference}").into();
    }
    fn devices(&mut self, devices: &mut Value, parent: &Value) {
        if devices.is_null() {
            *devices = json!([]);
        }
        for d in devices.as_array_mut().unwrap() {
            self.fresh(d, "device");
            d["parentRef"] = parent.clone();
            for key in ["parameters", "chains", "drumPads", "macros"] {
                if d[key].is_null() {
                    d[key] = json!([]);
                }
            }
            for p in d["parameters"].as_array_mut().unwrap() {
                self.fresh(p, "parameter");
            }
            let reference = d["ref"].clone();
            for c in d["chains"].as_array_mut().unwrap() {
                self.fresh(c, "chain");
                c["parentRef"] = reference.clone();
                let cr = c["ref"].clone();
                self.devices(&mut c["devices"], &cr);
            }
            for pad in d["drumPads"].as_array_mut().unwrap() {
                self.fresh(pad, "drum-pad");
                pad["parentRef"] = reference.clone();
                let pr = pad["ref"].clone();
                if pad["chains"].is_null() {
                    pad["chains"] = json!([]);
                }
                for c in pad["chains"].as_array_mut().unwrap() {
                    self.fresh(c, "chain");
                    c["parentRef"] = pr.clone();
                    let cr = c["ref"].clone();
                    self.devices(&mut c["devices"], &cr);
                }
            }
            for m in d["macros"].as_array_mut().unwrap() {
                self.fresh(m, "parameter");
            }
        }
    }
}
fn collect_refs(value: &Value, seen: &mut std::collections::HashSet<String>) -> Result<(), LiveError> {
    match value {
        Value::Array(values) => {
            for value in values {
                collect_refs(value, seen)?;
            }
        }
        Value::Object(values) => {
            if let Some(reference) = value["ref"].as_str() {
                if !seen.insert(reference.into()) {
                    return Err(LiveError::error("duplicated track graph contains a duplicate ref"));
                }
            }
            for value in values.values() {
                collect_refs(value, seen)?;
            }
        }
        _ => {}
    }
    Ok(())
}
fn lane_siblings(track: &Value) -> Value {
    Value::Array(
        array(&track["takeLanes"]).iter().map(|l| json!({"ref":l["ref"],"objectIdentity":l["objectIdentity"],"name":l["name"]})).collect(),
    )
}
fn lane_path(state: &Value, reference: &str) -> Option<String> {
    for (ti, t) in array(&state["tracks"]).iter().enumerate() {
        for (li, l) in array(&t["takeLanes"]).iter().enumerate() {
            if l["ref"] == reference {
                return Some(format!("/tracks/{ti}/takeLanes/{li}"));
            }
        }
    }
    None
}
pub(super) fn absolute_audio_path(value: Option<&Value>) -> Result<String, LiveError> {
    let s = value
        .and_then(Value::as_str)
        .filter(|s| {
            !s.is_empty()
                && kumi_common::js::string::utf16_len(s) <= 1024
                && (s.starts_with('/') || (s.as_bytes().first().is_some_and(u8::is_ascii_alphabetic) && s.as_bytes().get(1) == Some(&b':')))
        })
        .ok_or_else(|| LiveError::range_error("filePath must be an absolute path"))?;
    Ok(s.into())
}
impl DeterministicLiveSimulator {
    pub(super) fn invoke_structure(&self, operation: &str, args: &Map<String, Value>) -> Result<Value, LiveError> {
        match operation {
            "track.create-return" => {
                self.require_structure_revision(args).map_err(|_| LiveError::error("structure changed since preview"))?;
                let name = args.get("name").map(|v| bounded_text(v, 256, "name is invalid")).transpose()?;
                let mut state = self.state.borrow_mut();
                let index = array(&state["tracks"]).iter().filter(|t| t["kind"] == "return").count();
                let next = self.sequence.get() + 1;
                let name = name.unwrap_or_else(|| format!("Return {}", char::from_u32((65 + index as u32) % 65536).unwrap_or('\u{fffd}')));
                let track = json!({"ref":format!("track:return-{next}"),"objectIdentity":format!("simulator:track:{next}"),"name":name,"kind":"return","volume":0.85,"pan":0,"mute":false,"solo":false,"armed":null,"clips":[],"devices":[],"sends":[]});
                state["tracks"].as_array_mut().unwrap().push(track.clone());
                drop(state);
                self.emit(LiveEventType::State, None, json!({"operation":operation}));
                let mut result = json!({"ref":track["ref"],"objectIdentity":track["objectIdentity"],"name":name,"index":index});
                result["createdFingerprint"] = simulator_revision(&result).into();
                Ok(result)
            }
            "track.delete-return" | "track.duplicate" | "scene.duplicate" => {
                let reference = string_arg(args, "ref")?;
                let collection = if operation == "scene.duplicate" { "scenes" } else { "tracks" };
                let what = if operation == "track.delete-return" {
                    "return-track"
                } else if operation == "track.duplicate" {
                    "track"
                } else {
                    "scene"
                };
                let state = self.state.borrow();
                let index = array(&state[collection])
                    .iter()
                    .position(|row| row["ref"] == reference && (operation != "track.delete-return" || row["kind"] == "return"))
                    .ok_or_else(|| LiveError::error(format!("{what} reference is stale or invalid")))?;
                drop(state);
                self.require_structure_revision(args).map_err(|_| LiveError::error("structure changed since preview"))?;
                let mut state = self.state.borrow_mut();
                let target = &state[collection][index];
                if target.get("objectIdentity") != args.get("expectedObjectIdentity") {
                    return Err(LiveError::error(format!("{what} identity changed since preview")));
                }
                if operation == "track.delete-return" {
                    state[collection].as_array_mut().unwrap().remove(index);
                    drop(state);
                    self.emit(LiveEventType::State, None, json!({"operation":operation}));
                    return Ok(json!({"deleted":reference}));
                }
                let next = self.sequence.get() + 1;
                let mut copy = target.clone();
                let name = format!("{} copy", target["name"].as_str().unwrap());
                copy["name"] = name.clone().into();
                if operation == "track.duplicate" {
                    copy["ref"] = format!("track:track-dup-{next}").into();
                    copy["objectIdentity"] = format!("simulator:track:track-dup-{next}").into();
                    let mut dupe = Duplicator { sequence: next, count: 0 };
                    let mut clips = HashMap::new();
                    for key in ["clips", "clipSlots", "takeLanes"] {
                        if copy[key].is_null() {
                            copy[key] = json!([]);
                        }
                    }
                    for c in copy["clips"].as_array_mut().unwrap() {
                        let old = c["ref"].as_str().unwrap().to_string();
                        dupe.fresh(c, "clip");
                        clips.insert(old, c["ref"].clone());
                    }
                    let reference = copy["ref"].clone();
                    for slot in copy["clipSlots"].as_array_mut().unwrap() {
                        let old = slot["clipRef"].clone();
                        dupe.fresh(slot, "clip-slot");
                        slot["parentRef"] = reference.clone();
                        if let Some(old) = old.as_str().filter(|s| !s.is_empty()) {
                            slot["clipRef"] = clips.get(old).cloned().unwrap_or(Value::Null);
                        }
                    }
                    dupe.devices(&mut copy["devices"], &reference);
                    for lane in copy["takeLanes"].as_array_mut().unwrap() {
                        dupe.fresh(lane, "take-lane");
                        if lane["clips"].is_null() {
                            lane["clips"] = json!([]);
                        }
                        for c in lane["clips"].as_array_mut().unwrap() {
                            dupe.fresh(c, "clip");
                        }
                    }
                    state["tracks"].as_array_mut().unwrap().insert(index + 1, copy.clone());
                    collect_refs(&copy, &mut std::collections::HashSet::new())?;
                } else {
                    copy["ref"] = format!("scene:scene-{next}").into();
                    copy["objectIdentity"] = format!("simulator:scene:{next}").into();
                    copy["index"] = (index + 1).into();
                    state["scenes"].as_array_mut().unwrap().insert(index + 1, copy.clone());
                    for (i, scene) in state["scenes"].as_array_mut().unwrap().iter_mut().enumerate() {
                        scene["index"] = i.into();
                    }
                    for track in state["tracks"].as_array_mut().unwrap() {
                        // As Live: the scenes after the copy move down one, and their slots with them.
                        for later in track["clipSlots"].as_array_mut().into_iter().flatten() {
                            if let Some(scene) = later["sceneIndex"].as_u64().filter(|scene| *scene > index as u64) {
                                later["sceneIndex"] = (scene + 1).into();
                            }
                        }
                        let slot = array(&track["clipSlots"]).iter().find(|s| s["sceneIndex"].as_u64() == Some(index as u64));
                        let clip = slot
                            .and_then(|s| s["clipRef"].as_str())
                            .and_then(|r| array(&track["clips"]).iter().find(|c| c["ref"] == r))
                            .cloned();
                        let tr = track["ref"].as_str().unwrap();
                        let mut new_slot = json!({"ref":format!("clip-slot:dup-{next}-{tr}"),"parentRef":tr,"objectIdentity":format!("simulator:clip-slot:dup-{next}-{tr}"),"sceneIndex":index+1,"clipRef":null,"empty":true});
                        if let Some(mut clip) = clip {
                            clip["ref"] = format!("clip:dup-{next}-{tr}").into();
                            clip["objectIdentity"] = format!("simulator:clip:dup-{next}-{tr}").into();
                            new_slot["clipRef"] = clip["ref"].clone();
                            new_slot["empty"] = false.into();
                            if track["clips"].is_null() {
                                track["clips"] = json!([]);
                            }
                            track["clips"].as_array_mut().unwrap().push(clip);
                        }
                        if track["clipSlots"].is_null() {
                            track["clipSlots"] = json!([]);
                        }
                        track["clipSlots"].as_array_mut().unwrap().push(new_slot);
                    }
                }
                drop(state);
                self.emit(LiveEventType::State, None, json!({"operation":operation}));
                let mut result = json!({"ref":copy["ref"],"objectIdentity":copy["objectIdentity"],"name":name,"index":index+1});
                result["createdFingerprint"] = simulator_revision(&result).into();
                Ok(result)
            }
            "audio.take-lane.read" => {
                let reference = string_arg(args, "trackRef")?;
                let state = self.state.borrow();
                let track = array(&state["tracks"]).iter().find(|t| t["ref"] == reference);
                Ok(
                    json!({"lanes":track.map(|t|array(&t["takeLanes"]).iter().map(|l|json!({"ref":l["ref"],"name":l["name"]})).collect::<Vec<_>>()).unwrap_or_default()}),
                )
            }
            "take-lane.create" => {
                let reference = string_arg(args, "trackRef")?;
                let mut state = self.state.borrow_mut();
                let track = state["tracks"]
                    .as_array_mut()
                    .unwrap()
                    .iter_mut()
                    .find(|t| t["ref"] == reference)
                    .filter(|t| t.get("objectIdentity") == args.get("expectedTrackIdentity"))
                    .ok_or_else(|| LiveError::error("take-lane target track identity changed since preview"))?;
                if args.get("expectedTakeLaneCollectionRevision") != Some(&json!(simulator_revision(&lane_siblings(track)))) {
                    return Err(LiveError::error("take-lane collection changed since preview"));
                }
                let name = args.get("name").map(|v| bounded_text(v, 256, "name is invalid")).transpose()?;
                let index = array(&track["takeLanes"]).len();
                let name = name.unwrap_or_else(|| format!("Take {}", index + 1));
                let lane = json!({"ref":format!("take-lane:{reference}:{index}"),"objectIdentity":format!("simulator:take-lane:{}",self.sequence.get()+1),"parentRef":reference,"trackRef":reference,"name":name,"index":index,"clips":[]});
                if track["takeLanes"].is_null() {
                    track["takeLanes"] = json!([]);
                }
                track["takeLanes"].as_array_mut().unwrap().push(lane.clone());
                drop(state);
                self.emit(LiveEventType::Object, Some(reference.into()), json!({"operation":operation,"lane":lane}));
                let mut result = json!({"ref":lane["ref"],"objectIdentity":lane["objectIdentity"],"name":name,"index":index});
                result["createdFingerprint"] = simulator_revision(&result).into();
                Ok(result)
            }
            "take-lane.rename" => {
                let reference = string_arg(args, "ref")?;
                let mut state = self.state.borrow_mut();
                let path = lane_path(&state, reference).ok_or_else(|| LiveError::error("take-lane reference is stale or invalid"))?;
                let name = bounded_text(args.get("name").unwrap_or(&Value::Null), 256, "name is invalid")?;
                let lane = state.pointer(&path).unwrap();
                if lane.get("objectIdentity") != args.get("expectedObjectIdentity") || lane.get("name") != args.get("expectedName") {
                    return Err(LiveError::error("take-lane rename target changed since preview"));
                }
                let track_path = path.split("/takeLanes/").next().unwrap();
                if args.get("expectedAuthorityRevision")
                    != Some(&json!(simulator_revision(&lane_siblings(state.pointer(track_path).unwrap()))))
                {
                    return Err(LiveError::error("take-lane hierarchy changed since preview"));
                }
                state.pointer_mut(&path).unwrap()["name"] = name.clone().into();
                drop(state);
                self.emit(LiveEventType::Object, Some(reference.into()), json!({"operation":operation}));
                Ok(json!({"renamed":reference,"name":name}))
            }
            "take-lane.clip.create" | "take-lane.audio-clip.create" => {
                let reference = string_arg(args, "takeLaneRef")?;
                let mut state = self.state.borrow_mut();
                let path = lane_path(&state, reference).ok_or_else(|| LiveError::error("take-lane reference is stale or invalid"))?;
                let lane = state.pointer_mut(&path).unwrap();
                if lane.get("objectIdentity") != args.get("expectedTakeLaneIdentity") {
                    return Err(LiveError::error("take-lane identity changed since preview"));
                }
                let siblings = Value::Array(
                    array(&lane["clips"]).iter().map(|c| json!({"ref":c["ref"],"objectIdentity":c["objectIdentity"]})).collect(),
                );
                if args.get("expectedCollectionRevision") != Some(&json!(simulator_revision(&siblings))) {
                    return Err(LiveError::error("take-lane clip collection changed since preview"));
                }
                let audio = operation == "take-lane.audio-clip.create";
                let position =
                    ranged_number(args.get("position").unwrap_or(&Value::Null), 0., f64::INFINITY, false, "position is invalid")?;
                let (file, length, name) = if audio {
                    let file = absolute_audio_path(args.get("filePath"))?;
                    let name = args
                        .get("name")
                        .map(|v| bounded_text(v, 256, "name is invalid"))
                        .transpose()?
                        .unwrap_or_else(|| file.rsplit('/').next().unwrap_or("Audio Clip").into());
                    (Some(file), 4., name)
                } else {
                    let length = ranged_number(
                        args.get("length").unwrap_or(&Value::Null),
                        f64::MIN_POSITIVE,
                        f64::INFINITY,
                        false,
                        "length is invalid",
                    )?;
                    let name = bounded_text(args.get("name").unwrap_or(&Value::Null), 256, "name is invalid")?;
                    (None, length, name)
                };
                let mut clip = json!({"ref":format!("take-lane-clip:{reference}:{}",kumi_common::js::number::to_string(position)),"objectIdentity":format!("simulator:take-lane-clip:{}",self.sequence.get()+1),"name":name,"kind":if audio{"audio"}else{"midi"},"start":position,"length":length,"notes":[],"notesRevision":simulator_revision(&json!([])),"warp":audio,"takes":[],"automation":[],"isAudio":audio,"muted":false,"isTakeLaneClip":true});
                if let Some(file) = &file {
                    clip["filePath"] = file.clone().into();
                }
                lane["clips"].as_array_mut().unwrap().push(clip.clone());
                drop(state);
                self.emit(LiveEventType::Object, Some(reference.into()), json!({"operation":operation,"clip":clip}));
                let mut result = json!({"ref":clip["ref"],"objectIdentity":clip["objectIdentity"],"name":name,"start":position,"length":length,"createdFingerprint":simulator_revision(&without_playback_state(&clip))});
                if let Some(file) = file {
                    result["filePath"] = file.into();
                }
                Ok(result)
            }
            _ => unreachable!("structure dispatcher routes only implemented operations"),
        }
    }
}
