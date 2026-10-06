use super::*;
use serde_json::json;
fn envelope_revision(clip: &Value, parameter: &str) -> String {
    simulator_revision(
        &json!({"exists":clip["envelopes"].get(parameter).is_some(),"points":clip["envelopes"].get(parameter).filter(|v|!v.is_null()).cloned().unwrap_or_else(||json!([]))}),
    )
}
fn note_ids(value: Option<&Value>, max: usize, unique: bool) -> Result<Vec<f64>, LiveError> {
    let rows = value
        .and_then(Value::as_array)
        .filter(|a| !a.is_empty() && a.len() <= max)
        .ok_or_else(|| LiveError::range_error("note ids are invalid"))?;
    let mut ids = Vec::new();
    for row in rows {
        let n = ranged_number(row, 0., f64::INFINITY, true, "note ids are invalid")?;
        if unique && ids.contains(&n) {
            return Err(LiveError::range_error("note ids are invalid"));
        }
        ids.push(n);
    }
    Ok(ids)
}
impl DeterministicLiveSimulator {
    pub(super) fn invoke_automation(&self, operation: &str, args: &Map<String, Value>) -> Result<Value, LiveError> {
        match operation {
            "recording.session" | "recording.arrangement" => {
                let start = args.get("action") == Some(&json!("start"));
                if !start && args.get("action") != Some(&json!("stop")) {
                    return Err(LiveError::range_error("action is invalid"));
                }
                let mut state = self.state.borrow_mut();
                let transport = &state["playback"]["transport"];
                if !["expectedSessionRecord", "expectedArrangementRecord"].iter().all(|key| args.get(*key).is_some_and(Value::is_boolean))
                    || args.get("expectedSessionRecord") != transport.get("sessionRecord")
                    || args.get("expectedArrangementRecord") != transport.get("arrangementRecord")
                {
                    return Err(LiveError::error("recording state changed since preview"));
                }
                let safety = args.get("outputSafety");
                if !safety.is_some_and(|v| {
                    v["safe"] == true && v["provenance"].as_str().is_some_and(|s| !["", "unknown", "simulator"].contains(&s))
                }) {
                    return Err(LiveError::error("authoritative output safety is required"));
                }
                if start {
                    let reference = string_arg(args, "destinationTrackRef")?;
                    let destination = array(&state["tracks"]).iter().find(|t| t["ref"] == reference);
                    let also = args.get("alsoTrackRefs").map(array).unwrap_or(&[]);
                    let identities = args.get("alsoTrackIdentities").map(array).unwrap_or(&[]);
                    for (i, r) in also.iter().enumerate() {
                        if !array(&state["tracks"])
                            .iter()
                            .any(|t| t.get("ref") == Some(r) && t.get("objectIdentity") == identities.get(i) && t["armed"] == true)
                        {
                            return Err(LiveError::error("a track recorded alongside changed identity or is not armed"));
                        }
                    }
                    if !destination.is_some_and(|t| t.get("objectIdentity") == args.get("destinationTrackIdentity") && t["armed"] == true) {
                        return Err(LiveError::error("recording destination isn't armed; arm it first"));
                    }
                } else if args.get("destinationTrackRef") != Some(&Value::Null)
                    || args.get("destinationTrackIdentity") != Some(&Value::Null)
                {
                    return Err(LiveError::error("recording stop destination authority must be null"));
                }
                let transport = &mut state["playback"]["transport"];
                transport[if operation == "recording.session" { "sessionRecord" } else { "arrangementRecord" }] = start.into();
                if operation == "recording.arrangement" && start {
                    transport["playing"] = true.into();
                }
                drop(state);
                self.emit(LiveEventType::Transport, None, json!({"operation":operation}));
                Ok(json!({"recording":start}))
            }
            "arrangement.automation.read" => {
                let reference = string_arg(args, "clipRef")?;
                let state = self.state.borrow();
                let row = array(&state["arrangementClips"])
                    .iter()
                    .find(|r| r["clip"]["ref"] == reference)
                    .ok_or_else(|| LiveError::error("arrangement clip reference is stale or invalid"))?;
                let parameter = string_arg(args, "parameterRef")?;
                let points = row["clip"]["envelopes"].get(parameter);
                if points.is_some_and(|p| array(p).len() > 512) {
                    return Err(LiveError::error("complete arrangement envelope exceeds its authoritative point bound"));
                }
                Ok(
                    json!({"available":true,"exists":points.is_some(),"points":points.filter(|v|!v.is_null()).cloned().unwrap_or_else(||json!([]))}),
                )
            }
            "audio.comp.read" => {
                let reference = string_arg(args, "clipRef")?;
                let state = self.state.borrow();
                let found = array(&state["tracks"])
                    .iter()
                    .find_map(|t| array(&t["clips"]).iter().find(|c| c["ref"] == reference).map(|c| (t, c)))
                    .ok_or_else(|| LiveError::error("comp read requires an exact clip reference"))?;
                let (track, clip) = found;
                let from = clip["start"].as_f64().unwrap();
                let to = from + clip["length"].as_f64().unwrap();
                let mut segments = Vec::new();
                for lane in array(&track["takeLanes"]) {
                    for c in array(&lane["clips"]) {
                        let start = c["start"].as_f64().unwrap().max(from);
                        let end = (c["start"].as_f64().unwrap() + c["length"].as_f64().unwrap()).min(to);
                        if end > start {
                            segments.push(json!({"laneRef":lane["ref"],"from":start,"to":end}));
                        }
                        if segments.len() > 512 {
                            return Err(LiveError::error("comp segment collection exceeds its authoritative bound"));
                        }
                    }
                }
                Ok(json!({"segments":segments}))
            }
            "note.read-by-id" | "note.read-selected" | "note.duplicate" | "note.quantize" => {
                let reference = string_arg(args, "ref")?;
                let mut state = self.state.borrow_mut();
                let path = note_clip_path(&state, reference)?;
                if operation == "note.duplicate" || operation == "note.quantize" {
                    let authority = Self::note_clip_authority(&state, reference)?;
                    let expected =
                        args.get("expectedClipAuthority").ok_or_else(|| LiveError::error("unsupported simulator authority value"))?;
                    if simulator_revision(&authority) != simulator_revision(expected) {
                        return Err(LiveError::error("note clip hierarchy identity changed since preview"));
                    }
                }
                let clip = state.pointer_mut(&path).unwrap();
                let revision = clip
                    .get("notesRevision")
                    .filter(|v| !v.is_null())
                    .cloned()
                    .unwrap_or_else(|| json!(simulator_revision(&clip["notes"])));
                if operation == "note.read-selected" {
                    return Ok(json!({"available":true,"notes":[],"notesRevision":revision}));
                }
                if operation == "note.read-by-id" {
                    let wanted = note_ids(args.get("noteIds"), 1024, false)?;
                    return Ok(
                        json!({"notes":array(&clip["notes"]).iter().filter(|n|n["id"].as_f64().is_some_and(|id|wanted.contains(&id))).cloned().collect::<Vec<_>>(),"notesRevision":revision}),
                    );
                }
                if args.get("expectedNotesRevision") != Some(&revision) {
                    return Err(LiveError::error("clip notes changed since preview"));
                }
                let duplicated = if operation == "note.duplicate" {
                    let wanted = note_ids(args.get("noteIds"), 512, true)?;
                    let sources = array(&clip["notes"])
                        .iter()
                        .filter(|n| n["id"].as_f64().is_some_and(|id| wanted.contains(&id)))
                        .cloned()
                        .collect::<Vec<_>>();
                    if sources.len() != wanted.len() {
                        return Err(LiveError::error("complete stable note identity is required for duplication"));
                    }
                    let mut id = array(&clip["notes"]).iter().map(|n| n["id"].as_f64().unwrap_or(0.)).fold(0., f64::max) + 1.;
                    let count = sources.len();
                    for mut source in sources {
                        source["id"] = (id as i64).into();
                        id += 1.;
                        clip["notes"].as_array_mut().unwrap().push(source);
                    }
                    Some(count)
                } else {
                    let grid = ranged_number(
                        args.get("grid").unwrap_or(&Value::Null),
                        f64::MIN_POSITIVE,
                        f64::INFINITY,
                        false,
                        "quantize arguments are invalid",
                    )?;
                    let amount =
                        ranged_number(args.get("amount").unwrap_or(&Value::Null), 0., 1., false, "quantize arguments are invalid")?;
                    let pitch = args.get("pitch").map(|v| ranged_number(v, 0., 127., true, "pitch is invalid")).transpose()?;
                    for note in clip["notes"].as_array_mut().unwrap() {
                        let start = note["start"].as_f64().unwrap();
                        note["start"] = (kumi_common::js::number::round(start / grid) * grid * amount + start * (1. - amount)).into();
                        if let Some(pitch) = pitch {
                            note["pitch"] = pitch.into();
                        }
                    }
                    None
                };
                let revision = simulator_revision(&clip["notes"]);
                clip["notesRevision"] = revision.clone().into();
                drop(state);
                self.emit(LiveEventType::Object, Some(reference.into()), json!({"operation":operation}));
                Ok(if let Some(count) = duplicated {
                    json!({"duplicated":count,"notesRevision":revision})
                } else {
                    json!({"changed":true,"notesRevision":revision})
                })
            }
            "automation.envelope.read"
            | "automation.envelope.create"
            | "automation.envelope.delete"
            | "automation.point.insert"
            | "automation.point.delete" => {
                let reference = string_arg(args, "clipRef")?;
                let mut state = self.state.borrow_mut();
                let path = clip_path(&state, reference)?;
                let parameter = string_arg(args, "parameterRef")?;
                if operation == "automation.envelope.read" {
                    let clip = state.pointer(&path).unwrap();
                    let points = clip["envelopes"].get(parameter);
                    return Ok(
                        json!({"available":true,"exists":points.is_some(),"points":points.filter(|v|!v.is_null()).cloned().unwrap_or_else(||json!([])),"revision":envelope_revision(clip,parameter)}),
                    );
                }
                let authority = simulator_revision(
                    &json!({"clip":Self::session_clip_authority(&state,reference)?,"parameter":Self::parameter_authority(&state,parameter)?}),
                );
                if args.get("expectedAuthorityDigest") != Some(&json!(authority))
                    || args.get("expectedEnvelopeRevision") != Some(&json!(envelope_revision(state.pointer(&path).unwrap(), parameter)))
                {
                    return Err(LiveError::error("automation target identity or envelope changed since preview"));
                }
                let clip = state.pointer_mut(&path).unwrap();
                let result = match operation {
                    "automation.envelope.create" => {
                        if clip["envelopes"].is_null() {
                            clip["envelopes"] = json!({});
                        }
                        if clip["envelopes"][parameter].is_null() {
                            clip["envelopes"][parameter] = json!([]);
                        }
                        json!({"created":true})
                    }
                    "automation.envelope.delete" => {
                        if !clip.get_mut("envelopes").and_then(Value::as_object_mut).is_some_and(|o| o.remove(parameter).is_some()) {
                            return Err(LiveError::error("envelope does not exist"));
                        }
                        json!({"deleted":true})
                    }
                    "automation.point.insert" => {
                        let points = args
                            .get("points")
                            .and_then(Value::as_array)
                            .filter(|points| {
                                !points.is_empty()
                                    && points.len() <= 512
                                    && points.iter().all(|p| {
                                        p["time"].as_f64().is_some_and(|v| v.is_finite() && v >= 0.)
                                            && p["value"].as_f64().is_some_and(|v| v.is_finite())
                                    })
                            })
                            .ok_or_else(|| LiveError::range_error("points are invalid"))?;
                        if clip["envelopes"].is_null() {
                            clip["envelopes"] = json!({});
                        }
                        if clip["envelopes"][parameter].is_null() {
                            clip["envelopes"][parameter] = json!([]);
                        }
                        let envelope = clip["envelopes"][parameter].as_array_mut().unwrap();
                        envelope.extend(points.iter().cloned());
                        envelope.sort_by(|a, b| a["time"].as_f64().unwrap().total_cmp(&b["time"].as_f64().unwrap()));
                        json!({"inserted":points.len()})
                    }
                    "automation.point.delete" => {
                        let envelope = clip
                            .get_mut("envelopes")
                            .and_then(|v| v.get_mut(parameter))
                            .and_then(Value::as_array_mut)
                            .ok_or_else(|| LiveError::error("envelope does not exist"))?;
                        let from =
                            ranged_number(args.get("from").unwrap_or(&Value::Null), 0., f64::INFINITY, false, "from/to are invalid")?;
                        let to = ranged_number(
                            args.get("to").unwrap_or(&Value::Null),
                            f64::NEG_INFINITY,
                            f64::INFINITY,
                            false,
                            "from/to are invalid",
                        )?;
                        if to <= from {
                            return Err(LiveError::range_error("from/to are invalid"));
                        }
                        let before = envelope.len();
                        envelope.retain(|p| p["time"].as_f64().is_some_and(|v| v < from || v > to));
                        json!({"deleted":before-envelope.len()})
                    }
                    _ => unreachable!(),
                };
                drop(state);
                self.emit(LiveEventType::Object, Some(reference.into()), json!({"operation":operation}));
                Ok(result)
            }
            _ => unreachable!("automation dispatcher routes only implemented operations"),
        }
    }
}

fn device_parameter_refs(devices: &Value, refs: &mut Vec<Value>) {
    for device in array(devices) {
        refs.extend(array(&device["parameters"]).iter().map(|p| p["ref"].clone()));
        for chain in array(&device["chains"]) {
            device_parameter_refs(&chain["devices"], refs);
        }
        for pad in array(&device["drumPads"]) {
            for chain in array(&pad["chains"]) {
                device_parameter_refs(&chain["devices"], refs);
            }
        }
    }
}
impl DeterministicLiveSimulator {
    pub(super) fn clear_envelopes(&self, args: &Map<String, Value>) -> Result<Value, LiveError> {
        let reference = string_arg(args, "clipRef")?;
        let mut state = self.state.borrow_mut();
        let path = clip_path(&state, reference)?;
        let authority = if reference.starts_with("arrangement-clip:") {
            Self::arrangement_authority_revision(&state, reference)?
        } else {
            simulator_revision(&Self::session_clip_authority(&state, reference)?)
        };
        if args.get("expectedAuthorityDigest") != Some(&json!(authority)) {
            return Err(LiveError::error("clip hierarchy changed since preview"));
        }
        let track = array(&state["tracks"])
            .iter()
            .find(|t| array(&t["clips"]).iter().any(|c| c["ref"] == reference))
            .ok_or_else(|| LiveError::error("envelope clear requires a Session clip"))?;
        let mut refs = Vec::new();
        device_parameter_refs(&track["devices"], &mut refs);
        if !track["mixer"].is_null() {
            for key in ["volumeRef", "panRef", "cueRef"] {
                if track["mixer"][key].as_str().is_some_and(|s| !s.is_empty()) {
                    refs.push(track["mixer"][key].clone());
                }
            }
            refs.extend(array(&track["mixer"]["sendRefs"]).iter().filter(|r| r.as_str().is_some_and(|s| !s.is_empty())).cloned());
        }
        let clip = state.pointer_mut(&path).unwrap();
        let presence = refs.iter().map(|r| r.as_str().is_some_and(|r| clip["envelopes"].get(r).is_some())).collect::<Vec<_>>();
        if args.get("expectedEnvelopesRevision") != Some(&json!(simulator_revision(&json!(presence)))) {
            return Err(LiveError::error("clip envelope collection changed since preview"));
        }
        let cleared = presence.iter().filter(|p| **p).count();
        clip["envelopes"] = json!({});
        drop(state);
        self.emit(LiveEventType::Object, Some(reference.into()), json!({"operation":"automation.envelope.clear"}));
        Ok(json!({"cleared":cleared,"envelopesRevision":simulator_revision(&json!(vec![false;refs.len()]))}))
    }
    pub(super) fn invoke_gap_automation(&self, operation: &str, args: &Map<String, Value>) -> Result<Value, LiveError> {
        let reference = string_arg(args, "clipRef").map_err(|_| LiveError::type_error("clipRef is invalid"))?;
        let mut state = self.state.borrow_mut();
        let path = clip_path(&state, reference)?;
        let parameter = string_arg(args, "parameterRef").map_err(|_| LiveError::type_error("parameterRef is invalid"))?;
        if operation == "automation.value-at" {
            let time = ranged_number(args.get("time").unwrap_or(&Value::Null), 0., f64::INFINITY, false, "time is invalid")?;
            let clip = state.pointer(&path).unwrap();
            let points = clip["envelopes"].get(parameter);
            let Some(points) = points.filter(|v| !v.is_null()) else {
                return Ok(json!({"value":null}));
            };
            let points = array(points);
            if points.is_empty() {
                return Ok(json!({"value":0}));
            }
            let after = points.iter().position(|p| p["time"].as_f64().is_some_and(|t| t >= time));
            let Some(after) = after.filter(|a| *a > 0) else {
                return Ok(json!({"value":if after==Some(0){&points[0]["value"]}else{&points.last().unwrap()["value"]}}));
            };
            let left = &points[after - 1];
            let right = &points[after];
            let lt = left["time"].as_f64().unwrap();
            let rt = right["time"].as_f64().unwrap();
            let lv = left["value"].as_f64().unwrap();
            let rv = right["value"].as_f64().unwrap();
            return Ok(json!({"value":if rt==lt{rv}else{lv+(rv-lv)*(time-lt)/(rt-lt)}}));
        }
        let authority = simulator_revision(
            &json!({"clip":Self::session_clip_authority(&state,reference)?,"parameter":Self::parameter_authority(&state,parameter)?}),
        );
        if args.get("expectedAuthorityDigest") != Some(&json!(authority))
            || args.get("expectedEnvelopeRevision") != Some(&json!(envelope_revision(state.pointer(&path).unwrap(), parameter)))
        {
            return Err(LiveError::error("automation target identity or envelope changed since preview"));
        }
        let start = ranged_number(args.get("start").unwrap_or(&Value::Null), 0., f64::INFINITY, false, "start is invalid")?;
        let length = ranged_number(args.get("length").unwrap_or(&Value::Null), 0.001, f64::INFINITY, false, "length is invalid")?;
        let value = ranged_number(args.get("value").unwrap_or(&Value::Null), f64::NEG_INFINITY, f64::INFINITY, false, "value is invalid")?;
        let clip = state.pointer_mut(&path).unwrap();
        if start + length > clip["length"].as_f64().unwrap() + 1e-9 {
            return Err(LiveError::error("the step is outside the clip"));
        }
        if clip["envelopes"].is_null() {
            clip["envelopes"] = json!({});
        }
        let mut kept = array(&clip["envelopes"][parameter])
            .iter()
            .filter(|p| p["time"].as_f64().is_some_and(|t| t < start || t > start + length))
            .cloned()
            .collect::<Vec<_>>();
        kept.extend([json!({"time":start,"value":value}), json!({"time":start+length,"value":value})]);
        kept.sort_by(|a, b| a["time"].as_f64().unwrap().total_cmp(&b["time"].as_f64().unwrap()));
        clip["envelopes"][parameter] = json!(kept);
        drop(state);
        self.emit(LiveEventType::Object, Some(reference.into()), json!({"operation":operation}));
        self.next_sequence();
        Ok(json!({"inserted":1}))
    }
}
