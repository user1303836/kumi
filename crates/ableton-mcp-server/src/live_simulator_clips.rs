use super::*;
use serde_json::json;
fn collection(state: &Value, reference: &str) -> Value {
    Value::Array(
        array(&state["arrangementClips"])
            .iter()
            .filter(|r| r["trackRef"] == reference)
            .map(|r| json!({"ref":r["clip"]["ref"],"objectIdentity":r["clip"]["objectIdentity"]}))
            .collect(),
    )
}
fn push_arrangement(state: &mut Value, clip: &Value, track: &str) {
    if state["arrangementClips"].is_null() {
        state["arrangementClips"] = json!([]);
    }
    state["arrangementClips"].as_array_mut().unwrap().push(json!({"clip":clip,"trackRef":track}));
}
/// The first of `names` no Arrangement clip has as its `field` (its ref or identity): Live never holds two
/// clips with the same one.
pub(super) fn untaken(rows: &[&Value], field: &str, names: impl IntoIterator<Item = String>) -> String {
    names.into_iter().find(|name| !rows.iter().any(|row| row["clip"][field] == name.as_str())).unwrap()
}
/// A ref and identity for the clip Live makes when it cuts one in two, starting at `start`: a ref by its
/// position, as a new clip's, and an identity numbered past every Arrangement clip's, so no clip has either
/// (and no event sequence is taken for them).
pub(super) fn split_names<'a>(rows: impl Iterator<Item = &'a Value>, track: &str, start: f64) -> (String, String) {
    let rows: Vec<&Value> = rows.collect();
    let position = kumi_common::js::number::to_string(start);
    let reference = untaken(
        &rows,
        "ref",
        std::iter::once(format!("arrangement-clip:{track}:{position}"))
            .chain((2..).map(|n| format!("arrangement-clip:{track}:{position}-{n}"))),
    );
    let past = rows.iter().filter_map(|row| row["clip"]["objectIdentity"].as_str()?.rsplit(':').next()?.parse::<u64>().ok()).max();
    (reference, format!("simulator:arrangement-clip:{}", past.unwrap_or(0) + 1))
}
/// Cuts a clip's start to `to` as Live does: the same notes, its start marker moved on by the cut (round its
/// loop when it loops), so what's left plays as it did there.
pub(super) fn cut_start(clip: &mut Value, to: f64) {
    let start = clip["start"].as_f64().unwrap_or(0.);
    let end = clip["endTime"].as_f64().unwrap_or(start + clip["length"].as_f64().unwrap_or(0.));
    let mut marker = clip["startMarker"].as_f64().unwrap_or(0.) + (to - start);
    if let (Some(true), Some(from), Some(until)) = (clip["looping"].as_bool(), clip["loopStart"].as_f64(), clip["loopEnd"].as_f64()) {
        if until > from && marker >= until {
            marker = from + (marker - from).rem_euclid(until - from);
        }
    }
    clip["startMarker"] = marker.into();
    clip["start"] = to.into();
    clip["length"] = (end - to).into();
    clip["endTime"] = end.into();
}
impl DeterministicLiveSimulator {
    fn arrangement_clip_fingerprint(&self, reference: &str) -> String {
        let snapshot = self.snapshot_value();
        let row = array(&snapshot["arrangement"]["clips"]).iter().find(|r| r["ref"] == reference).unwrap();
        simulator_revision(&without_playback_state(row))
    }
    pub(super) fn invoke_clips(&self, operation: &str, args: &Map<String, Value>) -> Result<Value, LiveError> {
        match operation {
            "arrangement.clip.create" | "arrangement.audio-clip.create" => {
                let reference = string_arg(args, "trackRef")?;
                let mut state = self.state.borrow_mut();
                let audio = operation == "arrangement.audio-clip.create";
                let track = array(&state["tracks"]).iter().find(|t| t["ref"] == reference);
                if track.is_none_or(|t| t.get("objectIdentity") != args.get("expectedTrackIdentity"))
                    || args.get("expectedCollectionRevision") != Some(&json!(simulator_revision(&collection(&state, reference))))
                {
                    return Err(LiveError::error(if audio {
                        "arrangement audio clip target track or collection identity changed"
                    } else {
                        "arrangement clip target track or collection identity changed"
                    }));
                }
                let position = ranged_number(
                    args.get("position").unwrap_or(&Value::Null),
                    0.,
                    f64::INFINITY,
                    false,
                    if audio { "position is invalid" } else { "arrangement clip bounds are invalid" },
                )?;
                let (length, name, file) = if audio {
                    let file = bounded_text(args.get("filePath").unwrap_or(&Value::Null), 1024, "filePath is invalid")?;
                    let name = args
                        .get("name")
                        .map(|v| bounded_text(v, 256, "name is invalid"))
                        .transpose()?
                        .unwrap_or_else(|| file.rsplit('/').next().unwrap_or("Audio Clip").into());
                    (4., name, Some(file))
                } else {
                    let length = ranged_number(
                        args.get("length").unwrap_or(&Value::Null),
                        f64::MIN_POSITIVE,
                        f64::INFINITY,
                        false,
                        "arrangement clip bounds are invalid",
                    )?;
                    let name = bounded_text(args.get("name").unwrap_or(&Value::Null), 256, "arrangement clip bounds are invalid")?;
                    (length, name, None)
                };
                let mut clip = json!({"ref":format!("arrangement-clip:{reference}:{}",kumi_common::js::number::to_string(position)),"objectIdentity":format!("simulator:arrangement-clip:{}",self.sequence.get()+1),"name":name,"kind":if audio{"audio"}else{"midi"},"start":position,"length":length,"notes":[],"notesRevision":simulator_revision(&json!([])),"warp":audio,"takes":[],"automation":[]});
                if let Some(file) = &file {
                    clip["filePath"] = file.clone().into();
                    clip["isAudio"] = true.into();
                    clip["muted"] = false.into();
                }
                push_arrangement(&mut state, &clip, reference);
                drop(state);
                self.emit(LiveEventType::Object, Some(reference.into()), json!({"operation":operation,"clip":clip}));
                let mut result = json!({"ref":clip["ref"],"objectIdentity":clip["objectIdentity"],"name":name,"start":position,"length":length,"createdFingerprint":self.arrangement_clip_fingerprint(clip["ref"].as_str().unwrap())});
                if let Some(file) = file {
                    result["filePath"] = file.into();
                }
                Ok(result)
            }
            "arrangement.clip.delete" | "arrangement.clip.move" => {
                let reference = string_arg(args, "ref")?;
                let mut state = self.state.borrow_mut();
                let index = array(&state["arrangementClips"]).iter().position(|r| r["clip"]["ref"] == reference);
                let error = if operation == "arrangement.clip.delete" {
                    "arrangement clip identity or hierarchy changed; deletion refused"
                } else {
                    "arrangement clip identity or hierarchy changed; move refused"
                };
                if index.is_none()
                    || state["arrangementClips"][index.unwrap()]["clip"].get("objectIdentity") != args.get("expectedObjectIdentity")
                    || args.get("expectedAuthorityRevision") != Some(&json!(Self::arrangement_authority_revision(&state, reference)?))
                {
                    return Err(LiveError::error(error));
                }
                if operation == "arrangement.clip.delete" {
                    state["arrangementClips"].as_array_mut().unwrap().retain(|r| r["clip"]["ref"] != reference);
                    drop(state);
                    self.emit(LiveEventType::Object, None, json!({"operation":operation,"ref":reference}));
                    Ok(json!({"deleted":reference}))
                } else {
                    let position =
                        ranged_number(args.get("position").unwrap_or(&Value::Null), 0., f64::INFINITY, false, "position is invalid")?;
                    // Like dropping a clip in Live, and as the Remote Script does: a clip in the new place goes,
                    // one crossing its edges is cut there, and one it lands in the middle of keeps both ends. A copy
                    // (keepSource) or a move to another track lands a new clip there; a copy on its own track cuts
                    // its own clip where it lands.
                    let moving = state["arrangementClips"][index.unwrap()].clone();
                    let keep = args.get("keepSource") == Some(&json!(true));
                    let target = match args.get("targetTrackRef").and_then(Value::as_str) {
                        Some(target) => {
                            let identity = array(&state["tracks"]).iter().find(|t| t["ref"] == target).map(|t| t["objectIdentity"].clone());
                            if identity.as_ref() != args.get("expectedTargetTrackIdentity") {
                                return Err(LiveError::error("the target track changed since preview; move refused"));
                            }
                            json!(target)
                        }
                        None => moving["trackRef"].clone(),
                    };
                    let elsewhere = keep || target != moving["trackRef"];
                    let (from, to) = (position, position + moving["clip"]["length"].as_f64().unwrap_or(0.));
                    let mut rows = Vec::new();
                    // What was in the new place before anything was cut, as the Remote Script says it.
                    let mut cleared = Vec::new();
                    for row in array(&state["arrangementClips"]).to_vec() {
                        let start = row["clip"]["start"].as_f64().unwrap_or(0.);
                        let end = row["clip"]["endTime"].as_f64().unwrap_or(start + row["clip"]["length"].as_f64().unwrap_or(0.));
                        if row["trackRef"] != target
                            || (!elsewhere && row["clip"]["ref"] == reference)
                            || start >= to - 1e-6
                            || end <= from + 1e-6
                        {
                            rows.push(row);
                            continue;
                        }
                        let name: String = row["clip"]["name"].as_str().unwrap_or("").chars().take(60).collect();
                        cleared.push(json!({"name":name,"start":start,"end":end}));
                        if start < from - 1e-6 {
                            let mut head = row.clone();
                            head["clip"]["length"] = (from - start).into();
                            head["clip"]["endTime"] = from.into();
                            rows.push(head);
                        }
                        if end > to + 1e-6 {
                            let mut tail = row.clone();
                            if start < from - 1e-6 {
                                let (name, identity) = split_names(
                                    array(&state["arrangementClips"]).iter().chain(&rows),
                                    row["trackRef"].as_str().unwrap_or(""),
                                    to,
                                );
                                tail["clip"]["ref"] = name.into();
                                tail["clip"]["objectIdentity"] = identity.into();
                            }
                            cut_start(&mut tail["clip"], to);
                            rows.push(tail);
                        }
                    }
                    if elsewhere {
                        let (name, identity) =
                            split_names(array(&state["arrangementClips"]).iter().chain(&rows), target.as_str().unwrap_or(""), position);
                        let mut copy = moving.clone();
                        copy["trackRef"] = target.clone();
                        for (key, value) in [("ref", json!(name)), ("objectIdentity", json!(identity)), ("start", json!(position))] {
                            copy["clip"][key] = value;
                        }
                        for key in ["trackRef", "parentRef"] {
                            if copy["clip"].get(key).is_some() {
                                copy["clip"][key] = target.clone();
                            }
                        }
                        if copy["clip"].get("endTime").is_some() {
                            copy["clip"]["endTime"] = to.into();
                        }
                        if !keep {
                            rows.retain(|r| r["clip"]["ref"] != reference);
                        }
                        rows.push(copy);
                        state["arrangementClips"] = Value::Array(rows);
                        drop(state);
                        self.emit(LiveEventType::Object, Some(name.as_str().into()), json!({"operation":operation}));
                        return Ok(
                            json!({"ref":name,"objectIdentity":identity,"start":position,"createdFingerprint":self.arrangement_clip_fingerprint(&name),"cleared":cleared}),
                        );
                    }
                    let index = rows.iter().position(|r| r["clip"]["ref"] == reference);
                    state["arrangementClips"] = Value::Array(rows);
                    state["arrangementClips"][index.unwrap()]["clip"]["start"] = position.into();
                    let identity = state["arrangementClips"][index.unwrap()]["clip"]["objectIdentity"].clone();
                    drop(state);
                    self.emit(LiveEventType::Object, Some(reference.into()), json!({"operation":operation}));
                    Ok(
                        json!({"ref":reference,"objectIdentity":identity,"start":position,"createdFingerprint":self.arrangement_clip_fingerprint(reference),"cleared":cleared}),
                    )
                }
            }
            "clip.duplicate" | "clip.move" => {
                let reference = string_arg(args, "ref")?;
                let mut state = self.state.borrow_mut();
                let found = array(&state["tracks"])
                    .iter()
                    .enumerate()
                    .find_map(|(ti, t)| array(&t["clips"]).iter().position(|c| c["ref"] == reference).map(|ci| (ti, ci)));
                let Some((ti, ci)) = found else {
                    return Err(LiveError::error("clip duplication source identity changed since preview"));
                };
                let authority = Self::session_clip_authority(&state, reference)?;
                let mut expected = Map::new();
                for key in [
                    "expectedObjectIdentity",
                    "expectedTrackRef",
                    "expectedTrackIdentity",
                    "expectedSlotRef",
                    "expectedSlotIdentity",
                    "expectedSceneRef",
                    "expectedSceneIdentity",
                ] {
                    expected.insert(
                        key.into(),
                        args.get(key).ok_or_else(|| LiveError::error("unsupported simulator authority value"))?.clone(),
                    );
                }
                if simulator_revision(&authority) != simulator_revision(&Value::Object(expected)) {
                    return Err(LiveError::error("clip duplication source identity changed since preview"));
                }
                if operation == "clip.move" && args.get("arrangementPosition") != Some(&Value::Null) {
                    return Err(LiveError::error("Session clip move cannot target the Arrangement"));
                }
                let track_ref = state["tracks"][ti]["ref"].as_str().unwrap().to_string();
                let mut clip = state["tracks"][ti]["clips"][ci].clone();
                if args.get("arrangementPosition") != Some(&Value::Null) {
                    if args.get("expectedTargetCollectionRevision") != Some(&json!(simulator_revision(&collection(&state, &track_ref)))) {
                        return Err(LiveError::error("Arrangement target collection changed since preview"));
                    }
                    let position = ranged_number(
                        args.get("arrangementPosition").unwrap_or(&Value::Null),
                        0.,
                        f64::INFINITY,
                        false,
                        "arrangement position is invalid",
                    )?;
                    clip["ref"] = format!("arrangement-clip:{track_ref}:{}", kumi_common::js::number::to_string(position)).into();
                    clip["objectIdentity"] = format!("simulator:arrangement-clip:{}", self.sequence.get() + 1).into();
                    clip["start"] = position.into();
                    push_arrangement(&mut state, &clip, &track_ref);
                    drop(state);
                    self.emit(LiveEventType::Object, Some(track_ref.into()), json!({"operation":operation,"clip":clip}));
                    return Ok(
                        json!({"ref":clip["ref"],"objectIdentity":clip["objectIdentity"],"name":clip["name"],"createdFingerprint":self.arrangement_clip_fingerprint(clip["ref"].as_str().unwrap())}),
                    );
                }
                if args.get("expectedTargetCollectionRevision") != Some(&Value::Null) {
                    return Err(LiveError::error("Session duplication cannot carry Arrangement collection authority"));
                }
                let target_ref = string_arg(args, "targetTrackRef")?;
                let tti = array(&state["tracks"]).iter().position(|t| t["ref"] == target_ref);
                let scene_index =
                    args.get("targetSceneIndex").and_then(Value::as_f64).filter(|v| v.is_finite() && *v >= 0. && v.fract() == 0.);
                if tti.is_none() || scene_index.is_none() {
                    return Err(LiveError::error("target track or scene index is invalid"));
                }
                let tti = tti.unwrap();
                let target = &state["tracks"][tti];
                let slot_index = array(&target["clipSlots"]).iter().position(|s| s["sceneIndex"].as_f64() == scene_index);
                let scene = array(&state["scenes"]).iter().find(|s| s["index"].as_f64() == scene_index);
                let valid = slot_index.zip(scene).is_some_and(|(si, scene)| {
                    let slot = &target["clipSlots"][si];
                    args.get("expectedTargetTrackIdentity") == target.get("objectIdentity")
                        && args.get("expectedTargetSlotRef") == slot.get("ref")
                        && args.get("expectedTargetSlotIdentity") == slot.get("objectIdentity")
                        && args.get("expectedTargetSceneRef") == scene.get("ref")
                        && args.get("expectedTargetSceneIdentity") == scene.get("objectIdentity")
                });
                if !valid {
                    return Err(LiveError::error("target clip hierarchy identity changed"));
                }
                let si = slot_index.unwrap();
                if !target["clipSlots"][si]["clipRef"].is_null() {
                    return Err(LiveError::error("target Session slot is occupied"));
                }
                clip["ref"] = format!("clip:{target_ref}:{}", kumi_common::js::number::to_string(scene_index.unwrap())).into();
                clip["objectIdentity"] = format!("simulator:clip:{}", self.sequence.get() + 1).into();
                state["tracks"][tti]["clips"].as_array_mut().unwrap().push(clip.clone());
                state["tracks"][tti]["clipSlots"][si]["clipRef"] = clip["ref"].clone();
                state["tracks"][tti]["clipSlots"][si]["empty"] = false.into();
                if operation == "clip.move" {
                    let source_slot = array(&state["tracks"][ti]["clipSlots"]).iter().position(|s| s["clipRef"] == reference);
                    if let Some(source_slot) = source_slot {
                        state["tracks"][ti]["clips"].as_array_mut().unwrap().remove(ci);
                        state["tracks"][ti]["clipSlots"][source_slot]["clipRef"] = Value::Null;
                        state["tracks"][ti]["clipSlots"][source_slot]["empty"] = true.into();
                    } else {
                        state["tracks"][tti]["clips"].as_array_mut().unwrap().pop();
                        state["tracks"][tti]["clipSlots"][si]["clipRef"] = Value::Null;
                        state["tracks"][tti]["clipSlots"][si]["empty"] = true.into();
                        return Err(LiveError::error("source Session slot changed during move"));
                    }
                }
                drop(state);
                self.emit(LiveEventType::Object, Some(target_ref.into()), json!({"operation":operation,"clip":clip}));
                Ok(
                    json!({"ref":clip["ref"],"objectIdentity":clip["objectIdentity"],"name":clip["name"],"createdFingerprint":simulator_revision(&without_playback_state(&clip))}),
                )
            }
            "session.audio-clip.create" => {
                let reference = string_arg(args, "trackRef")?;
                let mut state = self.state.borrow_mut();
                let ti = array(&state["tracks"]).iter().position(|t| t["ref"] == reference);
                let scene_index = args.get("sceneIndex").and_then(Value::as_f64).filter(|v| v.is_finite() && *v >= 0. && v.fract() == 0.);
                if ti.is_none() || scene_index.is_none() {
                    return Err(LiveError::error("audio import target is invalid"));
                }
                let ti = ti.unwrap();
                let track = &state["tracks"][ti];
                let si = array(&track["clipSlots"]).iter().position(|s| s["sceneIndex"].as_f64() == scene_index);
                let scene = array(&state["scenes"]).iter().find(|s| s["index"].as_f64() == scene_index);
                let valid = si.zip(scene).is_some_and(|(si, scene)| {
                    let slot = &track["clipSlots"][si];
                    args.get("expectedTrackIdentity") == track.get("objectIdentity")
                        && args.get("expectedSlotRef") == slot.get("ref")
                        && args.get("expectedSlotIdentity") == slot.get("objectIdentity")
                        && args.get("expectedSceneRef") == scene.get("ref")
                        && args.get("expectedSceneIdentity") == scene.get("objectIdentity")
                });
                if !valid {
                    return Err(LiveError::error("audio import target identity changed since preview"));
                }
                let si = si.unwrap();
                if !track["clipSlots"][si]["clipRef"].is_null() {
                    return Err(LiveError::error("session slot is occupied"));
                }
                let file = super::simulator_structure::absolute_audio_path(args.get("filePath"))?;
                let name = args
                    .get("name")
                    .map(|v| bounded_text(v, 256, "name is invalid"))
                    .transpose()?
                    .unwrap_or_else(|| file.rsplit('/').next().unwrap_or("Audio Clip").into());
                let clip = json!({"ref":format!("clip:{reference}:{}",kumi_common::js::number::to_string(scene_index.unwrap())),"objectIdentity":format!("simulator:clip:{}",self.sequence.get()+1),"name":name,"kind":"audio","start":scene_index.unwrap()*4.,"length":4,"notes":[],"notesRevision":simulator_revision(&json!([])),"warp":true,"takes":[],"automation":[],"isAudio":true,"filePath":file,"muted":false,"warpMarkers":[{"beatTime":1,"sampleTime":44100}]});
                state["tracks"][ti]["clips"].as_array_mut().unwrap().push(clip.clone());
                state["tracks"][ti]["clipSlots"][si]["clipRef"] = clip["ref"].clone();
                state["tracks"][ti]["clipSlots"][si]["empty"] = false.into();
                drop(state);
                self.emit(LiveEventType::Object, Some(reference.into()), json!({"operation":operation,"clip":clip}));
                Ok(
                    json!({"ref":clip["ref"],"objectIdentity":clip["objectIdentity"],"name":name,"length":4,"filePath":file,"createdFingerprint":simulator_revision(&without_playback_state(&clip))}),
                )
            }
            _ => unreachable!("clip dispatcher routes only implemented operations"),
        }
    }
}

impl DeterministicLiveSimulator {
    fn clip_authority_revision(state: &Value, reference: &str) -> Result<String, LiveError> {
        if reference.starts_with("arrangement-clip:") {
            Self::arrangement_authority_revision(state, reference)
        } else {
            Ok(simulator_revision(&Self::session_clip_authority(state, reference)?))
        }
    }
    pub(super) fn invoke_clip_properties(&self, operation: &str, args: &Map<String, Value>) -> Result<Value, LiveError> {
        use super::simulator_views::{fields, set_bool, set_number};
        let reference = string_arg(args, "ref")?;
        let mut state = self.state.borrow_mut();
        let path = clip_path(&state, reference)?;
        if operation == "audio.warp-marker.read" {
            let mut markers = array(&state.pointer(&path).unwrap()["warpMarkers"]).to_vec();
            markers.sort_by(|a, b| a["beatTime"].as_f64().unwrap().total_cmp(&b["beatTime"].as_f64().unwrap()));
            return Ok(json!({"revision":simulator_revision(&json!(markers)),"markers":markers}));
        }
        let clip = state.pointer(&path).unwrap();
        if operation.starts_with("audio.warp-marker.") {
            if clip["kind"] != "audio" {
                return Err(LiveError::error("warp markers require an audio clip"));
            }
            let authority = Self::clip_authority_revision(&state, reference)?;
            if args.get("expectedObjectIdentity").is_some_and(|v| clip.get("objectIdentity") != Some(v)) {
                return Err(LiveError::error("clip identity changed since preview"));
            }
            if args.get("expectedClipAuthorityDigest") != Some(&json!(authority)) {
                return Err(LiveError::error("clip hierarchy changed since preview"));
            }
            let mut markers = array(&clip["warpMarkers"]).to_vec();
            markers.sort_by(|a, b| a["beatTime"].as_f64().unwrap().total_cmp(&b["beatTime"].as_f64().unwrap()));
            if args.get("expectedMarkerCollectionRevision") != Some(&json!(simulator_revision(&json!(markers)))) {
                return Err(LiveError::error("warp-marker collection changed since preview"));
            }
            let beat = ranged_number(
                args.get("beatTime").unwrap_or(&Value::Null),
                f64::NEG_INFINITY,
                f64::INFINITY,
                false,
                "beatTime is invalid",
            )?;
            let clip = state.pointer_mut(&path).unwrap();
            clip["warpMarkers"] = json!(markers);
            match operation {
                "audio.warp-marker.add" => {
                    if beat < 0. || markers.iter().any(|m| m["beatTime"].as_f64() == Some(beat)) {
                        return Err(LiveError::range_error("a warp marker already exists at that beat time"));
                    }
                    markers.push(json!({"beatTime":beat,"sampleTime":beat*44100.}));
                }
                "audio.warp-marker.move" => {
                    let distance = ranged_number(
                        args.get("distance").unwrap_or(&Value::Null),
                        f64::NEG_INFINITY,
                        f64::INFINITY,
                        false,
                        "distance is invalid",
                    )?;
                    let index = markers
                        .iter()
                        .position(|m| m["beatTime"].as_f64() == Some(beat))
                        .ok_or_else(|| LiveError::error("no warp marker exists at that beat time"))?;
                    let target = beat + distance;
                    if target < 0. || markers.iter().enumerate().any(|(i, m)| i != index && m["beatTime"].as_f64() == Some(target)) {
                        return Err(LiveError::range_error("warp-marker move target collides with an existing marker"));
                    }
                    markers[index]["beatTime"] = target.into();
                    markers[index]["sampleTime"] = (target * 44100.).into();
                }
                "audio.warp-marker.delete" => {
                    if !markers.iter().any(|m| m["beatTime"].as_f64() == Some(beat)) {
                        return Err(LiveError::error("no warp marker exists at that beat time"));
                    }
                    markers.retain(|m| m["beatTime"].as_f64() != Some(beat));
                }
                _ => unreachable!(),
            };
            markers.sort_by(|a, b| a["beatTime"].as_f64().unwrap().total_cmp(&b["beatTime"].as_f64().unwrap()));
            clip["warpMarkers"] = json!(markers);
        } else if operation == "clip.follow-actions.set" {
            if !array(&state["tracks"]).iter().any(|t| array(&t["clips"]).iter().any(|c| c["ref"] == reference)) {
                return Err(LiveError::error("Follow Actions require a Session clip"));
            }
            if clip.get("objectIdentity") != args.get("expectedObjectIdentity") {
                return Err(LiveError::error("clip identity changed since preview"));
            }
            let prior = crate::follow_actions::FOLLOW_ACTION_FIELDS.iter().map(|f| (f.clone(), clip[f].clone())).collect::<Map<_, _>>();
            crate::follow_actions::validate_follow_actions(&prior).map_err(|e| LiveError::error(e.to_string()))?;
            if args.get("expectedAuthorityRevision") != Some(&json!(simulator_revision(&Self::session_clip_authority(&state, reference)?)))
                || args.get("expectedStateRevision") != Some(&json!(simulator_revision(&json!(prior))))
            {
                return Err(LiveError::error("clip hierarchy or Follow Action state changed since preview"));
            }
            if state["playback"]["transport"]["playing"] != false || clip["isRecording"] != false {
                return Err(LiveError::error("Follow Action edits require stopped transport and a non-recording clip"));
            }
            crate::follow_actions::validate_follow_actions(args).map_err(|e| LiveError::error(e.to_string()))?;
            let clip = state.pointer_mut(&path).unwrap();
            for f in crate::follow_actions::FOLLOW_ACTION_FIELDS.iter() {
                clip[f] = args[f].clone();
            }
        } else {
            if clip.get("objectIdentity") != args.get("expectedObjectIdentity") {
                return Err(LiveError::error(if operation == "audio.clip.set" {
                    "audio clip identity changed since preview"
                } else {
                    "clip identity changed since preview"
                }));
            }
            let authority = if operation == "clip.set" && reference.starts_with("take-lane-clip:") {
                let lane = array(&state["tracks"])
                    .iter()
                    .flat_map(|t| array(&t["takeLanes"]))
                    .find(|l| array(&l["clips"]).iter().any(|c| c["ref"] == reference))
                    .ok_or_else(|| LiveError::error("take-lane clip hierarchy is unavailable"))?;
                let siblings =
                    array(&lane["clips"]).iter().map(|c| json!({"ref":c["ref"],"objectIdentity":c["objectIdentity"]})).collect::<Vec<_>>();
                simulator_revision(&json!({"takeLaneRevision":simulator_revision(&json!(siblings)),"laneIdentity":lane["objectIdentity"]}))
            } else {
                Self::clip_authority_revision(&state, reference)?
            };
            match operation {
                "audio.clip.set" => {
                    let fields = fields(
                        clip,
                        &[
                            "gain",
                            "pitchCoarse",
                            "pitchFine",
                            "loopStart",
                            "loopEnd",
                            "warpMode",
                            "warping",
                            "fadeInLength",
                            "fadeOutLength",
                        ],
                    );
                    if args.get("expectedAuthorityRevision") != Some(&json!(authority))
                        || args.get("expectedStateRevision") != Some(&json!(simulator_revision(&fields)))
                    {
                        return Err(LiveError::error("audio clip hierarchy or state changed since preview"));
                    }
                    if clip["kind"] != "audio" {
                        return Err(LiveError::error("audio properties require an audio clip"));
                    }
                    let clip = state.pointer_mut(&path).unwrap();
                    for key in ["gain", "pitchCoarse", "pitchFine", "loopStart", "loopEnd", "warpMode", "fadeInLength", "fadeOutLength"] {
                        if let Some(value) = args.get(key) {
                            if !value.as_f64().is_some_and(f64::is_finite) {
                                return Err(LiveError::type_error(format!("{key} is invalid")));
                            }
                            clip[key] = value.clone();
                        }
                    }
                    set_bool(clip, args, "warping", "warping")?;
                }
                "clip.action" => {
                    if args.get("expectedAuthorityRevision") != Some(&json!(authority)) {
                        return Err(LiveError::error("clip hierarchy changed since preview"));
                    }
                    if args.get("expectedStateRevision")
                        != Some(&json!(simulator_revision(&fields(clip, &["isPlaying", "length", "loopStart", "loopEnd"]))))
                    {
                        return Err(LiveError::error("clip state changed since preview"));
                    }
                    let action = args.get("action").and_then(Value::as_str);
                    if action.is_some_and(|a| ["crop", "duplicate-loop", "duplicate-region"].contains(&a))
                        && args.get("expectedContentFingerprint") != Some(&json!(simulator_revision(&without_playback_state(clip))))
                    {
                        return Err(LiveError::error("clip content changed since preview"));
                    }
                    let clip = state.pointer_mut(&path).unwrap();
                    let length = clip["length"].as_f64().unwrap();
                    match action {
                        Some("crop") | Some("duplicate-loop") => {
                            let loop_length = clip["loopEnd"].as_f64().unwrap_or(length) - clip["loopStart"].as_f64().unwrap_or(0.);
                            clip["length"] = (loop_length + if action == Some("duplicate-loop") { length } else { 0. }).into();
                        }
                        Some("duplicate-region") => {
                            let start = ranged_number(
                                args.get("regionStart").unwrap_or(&Value::Null),
                                0.,
                                f64::INFINITY,
                                false,
                                "duplicate-region bounds are invalid",
                            )?;
                            let end = ranged_number(
                                args.get("regionEnd").unwrap_or(&Value::Null),
                                f64::NEG_INFINITY,
                                f64::INFINITY,
                                false,
                                "duplicate-region bounds are invalid",
                            )?;
                            if end <= start {
                                return Err(LiveError::range_error("duplicate-region bounds are invalid"));
                            }
                            let destination = ranged_number(
                                args.get("destination").unwrap_or(&Value::Null),
                                0.,
                                f64::INFINITY,
                                false,
                                "duplicate-region destination is invalid",
                            )?;
                            clip["length"] = length.max(destination + end - start).into();
                        }
                        Some("scrub-start") | Some("move-playing-position") => {
                            let offset = ranged_number(
                                args.get("offset").unwrap_or(&Value::Null),
                                f64::NEG_INFINITY,
                                f64::INFINITY,
                                false,
                                if action == Some("scrub-start") {
                                    "scrub position is invalid"
                                } else {
                                    "playing-position offset is invalid"
                                },
                            )?;
                            clip["playingPosition"] = (offset
                                + if action == Some("move-playing-position") {
                                    clip["playingPosition"].as_f64().unwrap_or(0.)
                                } else {
                                    0.
                                })
                            .into();
                        }
                        Some("scrub-stop") => clip["playingPosition"] = 0.into(),
                        _ => return Err(LiveError::range_error("clip action is invalid")),
                    }
                }
                "clip.set" => {
                    if args.get("expectedAuthorityRevision") != Some(&json!(authority))
                        || args.get("expectedStateRevision")
                            != Some(&json!(simulator_revision(&fields(
                                clip,
                                &[
                                    "muted",
                                    "colorIndex",
                                    "looping",
                                    "loopStart",
                                    "loopEnd",
                                    "groove",
                                    "launchMode",
                                    "launchQuantization",
                                    "legato",
                                    "ramMode",
                                    "velocityAmount"
                                ]
                            ))))
                    {
                        return Err(LiveError::error("clip hierarchy or state changed since preview"));
                    }
                    let groove = args
                        .get("grooveRef")
                        .filter(|v| !v.is_null())
                        .and_then(|r| array(&state["groovePool"]["grooves"]).iter().find(|g| g.get("ref") == Some(r)))
                        .cloned();
                    let clip = state.pointer_mut(&path).unwrap();
                    set_bool(clip, args, "muted", "muted")?;
                    set_number(clip, args, "colorIndex", 0., 69., true)?;
                    if (args.contains_key("launchMode") || args.contains_key("launchQuantization"))
                        && (clip["isPlaying"] == true || clip["isTriggered"] == true)
                    {
                        return Err(LiveError::error("launch behavior changes on a playing or triggered clip are refused"));
                    }
                    set_number(clip, args, "launchMode", 0., 3., true)?;
                    set_number(clip, args, "launchQuantization", 0., 14., true)?;
                    set_bool(clip, args, "legato", "legato")?;
                    if let Some(value) = args.get("ramMode") {
                        if !value.is_boolean() {
                            return Err(LiveError::type_error("ramMode is invalid"));
                        }
                        if clip["kind"] != "audio" {
                            return Err(LiveError::error("ramMode is only available on audio clips"));
                        }
                        clip["ramMode"] = value.clone();
                    }
                    if let Some(value) = args.get("velocityAmount") {
                        ranged_number(value, 0., 1., false, "velocityAmount is invalid")?;
                        if clip["kind"] == "audio" {
                            return Err(LiveError::error("velocityAmount is only available on MIDI clips"));
                        }
                        clip["velocityAmount"] = value.clone();
                    }
                    if let Some(value) = args.get("looping") {
                        if !value.is_boolean() {
                            return Err(LiveError::type_error("looping is invalid"));
                        }
                        if clip["kind"] == "audio" {
                            return Err(LiveError::error("audio clip loop editing uses audio.clip.set"));
                        }
                        clip["looping"] = value.clone();
                    }
                    if args.contains_key("loopStart") || args.contains_key("loopEnd") {
                        if clip["kind"] == "audio" {
                            return Err(LiveError::error("audio clip loop editing uses audio.clip.set"));
                        }
                        let start = args
                            .get("loopStart")
                            .filter(|v| !v.is_null())
                            .or_else(|| clip.get("loopStart").filter(|v| !v.is_null()))
                            .cloned()
                            .unwrap_or_else(|| json!(0));
                        let end = args
                            .get("loopEnd")
                            .filter(|v| !v.is_null())
                            .or_else(|| clip.get("loopEnd").filter(|v| !v.is_null()))
                            .cloned()
                            .unwrap_or_else(|| clip["length"].clone());
                        let start_number = ranged_number(&start, 0., f64::INFINITY, false, "clip loop bounds are invalid")?;
                        ranged_number(&end, start_number, f64::INFINITY, false, "clip loop bounds are invalid")?;
                        clip["loopStart"] = start;
                        clip["loopEnd"] = end;
                    }
                    if let Some(value) = args.get("grooveRef") {
                        if value.is_null() {
                            clip["groove"] = Value::Null;
                            clip["hasGroove"] = false.into();
                        } else {
                            let groove = groove.ok_or_else(|| LiveError::error("groove reference is stale or invalid"))?;
                            clip["groove"] = json!({"ref":groove["ref"],"name":groove["name"]});
                            clip["hasGroove"] = true.into();
                        }
                    }
                }
                _ => unreachable!("clip property dispatcher routes only implemented operations"),
            }
        }
        drop(state);
        self.emit(LiveEventType::Object, Some(reference.into()), json!({"operation":operation}));
        Ok(json!({"changed":true,"revision":self.next_sequence()}))
    }
}
