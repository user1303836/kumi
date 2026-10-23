use super::*;
use serde_json::json;
fn capture_revision(state: &Value) -> String {
    simulator_revision(
        &json!({"tracks":array(&state["tracks"]).iter().map(|t|json!({"ref":t["ref"],"objectIdentity":t["objectIdentity"],"clips":array(&t["clips"]).iter().map(|c|json!({"ref":c["ref"],"objectIdentity":c["objectIdentity"],"notesRevision":c["notesRevision"]})).collect::<Vec<_>>()})).collect::<Vec<_>>(),"scenes":array(&state["scenes"]).iter().map(|s|json!({"ref":s["ref"],"objectIdentity":s["objectIdentity"],"index":s["index"]})).collect::<Vec<_>>(),"playbackRevision":state["playback"]["revision"]}),
    )
}
fn eligible_keys(args: &Map<String, Value>, key: &str, min: usize) -> Result<Vec<String>, LiveError> {
    let values = args
        .get(key)
        .and_then(Value::as_array)
        .filter(|a| a.len() >= min && a.len() <= 256)
        .ok_or_else(|| LiveError::type_error(format!("{key} are invalid")))?;
    let mut keys = Vec::new();
    for value in values {
        let s = value
            .as_str()
            .filter(|s| !s.is_empty() && kumi_common::js::string::utf16_len(s) <= 1024)
            .ok_or_else(|| LiveError::type_error(format!("{key} are invalid")))?;
        if keys.iter().any(|v| v == s) {
            return Err(LiveError::type_error(format!("{key} are invalid")));
        }
        keys.push(s.into());
    }
    Ok(keys)
}
fn audition_revision(state: &Value, reference: &str, eligible: &[String]) -> Result<String, LiveError> {
    let scene =
        array(&state["scenes"]).iter().find(|s| s["ref"] == reference).ok_or_else(|| LiveError::error("audition scene is unavailable"))?;
    let mut eligible = eligible.to_vec();
    eligible.sort_by(|a, b| a.encode_utf16().cmp(b.encode_utf16()));
    let mut targets = Vec::new();
    for key in eligible {
        let parts = key.split('|').collect::<Vec<_>>();
        let t = array(&state["tracks"]).iter().find(|t| t["ref"].as_str() == parts.first().copied());
        let slot = t.and_then(|t| array(&t["clipSlots"]).iter().find(|s| s["ref"].as_str() == parts.get(1).copied()));
        let clip = t.zip(slot).and_then(|(t, s)| array(&t["clips"]).iter().find(|c| c["ref"] == s["clipRef"]));
        let Some(((t, slot), clip)) = t.zip(slot).zip(clip).filter(|_| parts.get(2).copied() == Some(reference)) else {
            return Err(LiveError::error("audition hierarchy is incomplete"));
        };
        targets.push(json!({"trackRef":t["ref"],"trackIdentity":t["objectIdentity"],"slotRef":slot["ref"],"slotIdentity":slot["objectIdentity"],"sceneRef":scene["ref"],"sceneIdentity":scene["objectIdentity"],"clipRef":clip["ref"],"clipIdentity":clip["objectIdentity"]}));
    }
    Ok(simulator_revision(
        &json!({"set":{"ref":state["set"]["ref"],"objectIdentity":state["set"]["objectIdentity"]},"scene":{"ref":scene["ref"],"objectIdentity":scene["objectIdentity"],"index":scene["index"]},"targets":targets}),
    ))
}
fn target_key(target: &Value) -> String {
    format!(
        "{}|{}|{}",
        target["trackRef"].as_str().unwrap_or("undefined"),
        target["clipSlotRef"].as_str().unwrap_or("undefined"),
        target["sceneRef"].as_str().unwrap_or("undefined")
    )
}
impl DeterministicLiveSimulator {
    fn stop_playback(&self, operation: &str) {
        let mut state = self.state.borrow_mut();
        state["set"]["playing"] = false.into();
        state["playback"]["transport"]["playing"] = false.into();
        state["playback"]["firedTargets"] = json!([]);
        state["playback"]["playingTargets"] = json!([]);
        state["playback"]["revision"] = format!("{}:stopped", self.epoch.get()).into();
        for t in state["tracks"].as_array_mut().unwrap() {
            t["firedSlotIndex"] = Value::Null;
            t["playingSlotIndex"] = Value::Null;
        }
        drop(state);
        self.emit(LiveEventType::Transport, None, json!({"operation":operation}));
    }
    pub(super) fn invoke_session(&self, operation: &str, args: &Map<String, Value>) -> Result<Value, LiveError> {
        match operation {
            "session.capture-midi" | "scene.capture" => {
                let mut state = self.state.borrow_mut();
                if args.get("expectedStateRevision") != Some(&json!(capture_revision(&state))) {
                    return Err(LiveError::error("Session state changed since capture preview"));
                }
                let index = array(&state["scenes"]).len();
                if operation == "session.capture-midi" {
                    if array(&state["tracks"]).is_empty() {
                        return Err(LiveError::error("MIDI capture is unavailable"));
                    }
                    let next = self.sequence.get() + 1;
                    let scene = json!({"ref":format!("scene:capture-target-{next}"),"objectIdentity":format!("sim-object:scene:capture-target-{next}"),"name":"Capture Target","index":index});
                    state["scenes"].as_array_mut().unwrap().push(scene);
                    let next = self.next_sequence();
                    let clip = json!({"ref":format!("clip:captured-{next}"),"objectIdentity":format!("sim-object:clip:{next}"),"name":"Captured","kind":"midi","start":index*4,"length":4,"notes":[],"notesRevision":simulator_revision(&json!([])),"warp":false,"takes":[],"automation":[]});
                    let track = &mut state["tracks"][0];
                    track["clips"].as_array_mut().unwrap().push(clip.clone());
                    let reference = track["ref"].as_str().unwrap().to_string();
                    let slot = json!({"ref":format!("clip-slot:{reference}:{index}"),"parentRef":reference,"objectIdentity":format!("sim-object:clip-slot:{next}"),"sceneIndex":index,"clipRef":clip["ref"],"empty":false});
                    if track["clipSlots"].is_null() {
                        track["clipSlots"] = json!([]);
                    }
                    track["clipSlots"].as_array_mut().unwrap().push(slot);
                    drop(state);
                    self.emit(LiveEventType::Object, Some(reference.into()), json!({"operation":operation,"clip":clip}));
                    Ok(
                        json!({"captured":true,"clips":[clip["ref"]],"clipIdentities":[{"ref":clip["ref"],"objectIdentity":clip["objectIdentity"],"createdFingerprint":simulator_revision(&without_playback_state(&clip))}]}),
                    )
                } else {
                    let next = self.next_sequence();
                    let scene = json!({"ref":format!("scene:captured-{next}"),"objectIdentity":format!("sim-object:scene:{next}"),"name":"Captured","index":index});
                    state["scenes"].as_array_mut().unwrap().push(scene.clone());
                    for t in state["tracks"].as_array_mut().unwrap() {
                        let tr = t["ref"].as_str().unwrap();
                        let sr = scene["ref"].as_str().unwrap();
                        let slot = json!({"ref":format!("clip-slot:{tr}:{sr}"),"parentRef":tr,"objectIdentity":format!("simulator:clip-slot:{tr}:{sr}"),"sceneIndex":index,"clipRef":null,"empty":true});
                        if t["clipSlots"].is_null() {
                            t["clipSlots"] = json!([]);
                        }
                        t["clipSlots"].as_array_mut().unwrap().push(slot);
                    }
                    drop(state);
                    self.emit(LiveEventType::Object, None, json!({"operation":operation,"scene":scene}));
                    Ok(
                        json!({"captured":true,"ref":scene["ref"],"objectIdentity":scene["objectIdentity"],"createdFingerprint":self.structure_created_fingerprint("scene",scene["ref"].as_str().unwrap())?}),
                    )
                }
            }
            "session.audition-launch" | "session.audition-stop" => {
                let reference = string_arg(args, "ref")?;
                let set_name = string_arg(args, "setName")?;
                let launch = operation == "session.audition-launch";
                let playback = if launch {
                    if !args.get("sceneName").and_then(Value::as_str).is_some_and(|s| kumi_common::js::string::utf16_len(s) <= 256) {
                        return Err(LiveError::type_error("sceneName must be a string of at most 256 characters"));
                    }
                    ranged_number(args.get("sceneIndex").unwrap_or(&Value::Null), 0., 10_000., true, "sceneIndex is invalid")?;
                    Some(string_arg(args, "playbackRevision")?)
                } else {
                    None
                };
                let eligible = eligible_keys(args, "eligibleTargets", usize::from(launch))?;
                let mut state = self.state.borrow_mut();
                if state["set"]["name"] != set_name
                    || args.get("expectedSetIdentity") != state["set"].get("objectIdentity")
                    || args.get("expectedAuthorityRevision") != Some(&json!(audition_revision(&state, reference, &eligible)?))
                {
                    return Err(LiveError::error("disposable Set identity or audition hierarchy does not match"));
                }
                if !launch {
                    if array(&state["playback"]["firedTargets"])
                        .iter()
                        .chain(array(&state["playback"]["playingTargets"]))
                        .any(|t| !eligible.contains(&target_key(t)) || t["sceneRef"] != reference)
                    {
                        return Err(LiveError::error("external or unknown playback is active; owned stop refused"));
                    }
                    drop(state);
                    self.stop_playback(operation);
                    return Ok(json!({"stopped":true}));
                }
                let scene = array(&state["scenes"])
                    .iter()
                    .find(|s| s["ref"] == reference)
                    .filter(|s| {
                        s.get("index").and_then(Value::as_f64) == args.get("sceneIndex").and_then(Value::as_f64)
                            && s.get("name") == args.get("sceneName")
                    })
                    .ok_or_else(|| LiveError::error("scene identity changed since preview"))?;
                let scene = scene.clone();
                let transport = &state["playback"]["transport"];
                if ["playing", "arrangementRecord", "sessionRecord"].iter().any(|key| transport[*key] != false) {
                    return Err(LiveError::error("audition requires a stopped, non-recording authoritative state"));
                }
                if !transport["launchQuantization"]["normalized"]
                    .as_str()
                    .is_some_and(|s| !s.is_empty() && !["none", "unknown", "free"].contains(&s))
                {
                    return Err(LiveError::error("launch quantization is unsafe or unknown"));
                }
                if !array(&state["playback"]["firedTargets"]).is_empty() || !array(&state["playback"]["playingTargets"]).is_empty() {
                    return Err(LiveError::error("existing Session playback prevents audition"));
                }
                if state["playback"]["revision"].as_str() != playback {
                    return Err(LiveError::error("playback state changed since preview"));
                }
                for t in array(&state["tracks"]) {
                    let monitorable = t["kind"].as_str().is_some_and(|s| ["regular", "audio", "midi"].contains(&s));
                    if if monitorable {
                        t["armed"] != false || !t["monitoringState"].as_str().is_some_and(|s| ["off", "auto"].contains(&s))
                    } else {
                        t["armed"] == true || t["monitoringState"] == "in"
                    } {
                        return Err(LiveError::error("armed, input-monitored, or unknown-state track prevents audition"));
                    }
                }
                for key in &eligible {
                    let parts = key.split('|').collect::<Vec<_>>();
                    if parts.get(2).copied() != Some(reference) {
                        return Err(LiveError::error("eligible target references a different scene"));
                    }
                    let slot = array(&state["tracks"])
                        .iter()
                        .find(|t| t["ref"].as_str() == parts.first().copied())
                        .and_then(|t| array(&t["clipSlots"]).iter().find(|s| s["ref"].as_str() == parts.get(1).copied()));
                    if !slot.is_some_and(|s| s["sceneIndex"] == scene["index"] && s["clipRef"].as_str().is_some_and(|s| !s.is_empty())) {
                        return Err(LiveError::error("eligible target is not an authoritative clip slot with a clip"));
                    }
                }
                let mut targets = Vec::new();
                for t in array(&state["tracks"]) {
                    for slot in array(&t["clipSlots"]) {
                        let target = json!({"trackRef":t["ref"],"clipSlotRef":slot["ref"],"sceneRef":scene["ref"],"sceneIndex":scene["index"],"clipRef":slot["clipRef"]});
                        if slot["sceneIndex"] == scene["index"]
                            && slot["clipRef"].as_str().is_some_and(|s| !s.is_empty())
                            && eligible.contains(&target_key(&target))
                        {
                            targets.push(target);
                        }
                    }
                }
                if targets.is_empty() {
                    return Err(LiveError::error("launch verification failed; stop was attempted"));
                }
                state["set"]["playing"] = true.into();
                state["playback"]["transport"]["playing"] = true.into();
                for target in &targets {
                    if let Some(t) = state["tracks"].as_array_mut().unwrap().iter_mut().find(|t| t["ref"] == target["trackRef"]) {
                        t["firedSlotIndex"] = target["sceneIndex"].clone();
                        t["playingSlotIndex"] = target["sceneIndex"].clone();
                    }
                }
                state["playback"]["firedTargets"] = json!(targets);
                state["playback"]["playingTargets"] = json!(targets);
                state["playback"]["revision"] = format!("{}:playing:{reference}", self.epoch.get()).into();
                drop(state);
                self.emit(LiveEventType::Transport, Some(reference.into()), json!({"operation":operation,"scene":reference}));
                Ok(json!({"launched":reference,"targets":targets}))
            }
            "session.emergency-stop" => {
                let expected = eligible_keys(args, "expectedTargets", 0)?;
                let state = self.state.borrow();
                let mut active = array(&state["playback"]["firedTargets"])
                    .iter()
                    .chain(array(&state["playback"]["playingTargets"]))
                    .map(target_key)
                    .collect::<Vec<_>>();
                active.sort_by(|a, b| a.encode_utf16().cmp(b.encode_utf16()));
                active.dedup();
                let transport = &state["playback"]["transport"];
                let recording = match (
                    transport["sessionRecord"].as_bool().unwrap_or(false),
                    transport["arrangementRecord"].as_bool().unwrap_or(false),
                ) {
                    (true, true) => "both",
                    (true, false) => "session",
                    (false, true) => "arrangement",
                    _ => "stopped",
                };
                if args.get("expectedRecording") != Some(&json!(recording))
                    || active.len() != expected.len()
                    || active.iter().any(|k| !expected.contains(k))
                {
                    return Err(LiveError::error(
                        "playback or recording exceeds the separately authorized observation; perform fresh discovery",
                    ));
                }
                drop(state);
                self.stop_playback(operation);
                let mut state = self.state.borrow_mut();
                state["playback"]["transport"]["sessionRecord"] = false.into();
                state["playback"]["transport"]["arrangementRecord"] = false.into();
                Ok(json!({"stopped":true,"stoppedTargets":active,"recordingStopped":true}))
            }
            _ => unreachable!("session dispatcher routes only implemented operations"),
        }
    }
}
