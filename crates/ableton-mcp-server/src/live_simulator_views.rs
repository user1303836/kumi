use super::*;
use serde_json::json;
pub(super) fn fields(row: &Value, names: &[&str]) -> Value {
    Value::Object(names.iter().map(|name| ((*name).into(), row[*name].clone())).collect())
}
fn track_view(track: &Value) -> Value {
    json!({"collapsed":track["view"]["isCollapsed"],"deviceInsertMode":track["view"]["deviceInsertMode"],"showChains":track["view"]["isShowingChains"]})
}
pub(super) fn fence(args: &Map<String, Value>, state: &Value, what: &str) -> Result<(), LiveError> {
    if args.get("expectedStateRevision") != Some(&json!(simulator_revision(state))) {
        Err(LiveError::error(format!("{what} state changed since preview")))
    } else {
        Ok(())
    }
}
pub(super) fn set_bool(row: &mut Value, args: &Map<String, Value>, key: &str, target: &str) -> Result<(), LiveError> {
    if let Some(value) = args.get(key) {
        if !value.is_boolean() {
            return Err(LiveError::type_error(format!("{key} is invalid")));
        }
        row[target] = value.clone();
    }
    Ok(())
}
pub(super) fn set_number(
    row: &mut Value,
    args: &Map<String, Value>,
    key: &str,
    min: f64,
    max: f64,
    integer: bool,
) -> Result<(), LiveError> {
    if let Some(value) = args.get(key) {
        ranged_number(value, min, max, integer, &format!("{key} is invalid"))?;
        row[key] = value.clone();
    }
    Ok(())
}
impl DeterministicLiveSimulator {
    pub(super) fn invoke_views(&self, operation: &str, args: &Map<String, Value>) -> Result<Value, LiveError> {
        match operation {
            "scene.set" | "scene.fire-selected" => {
                let reference = string_arg(args, "ref")?;
                let mut state = self.state.borrow_mut();
                let index = array(&state["scenes"])
                    .iter()
                    .position(|r| r["ref"] == reference)
                    .ok_or_else(|| LiveError::error("scene reference is stale or invalid"))?;
                if state["scenes"][index].get("objectIdentity") != args.get("expectedObjectIdentity") {
                    return Err(LiveError::error("scene identity changed since preview"));
                }
                let scene_fields =
                    ["colorIndex", "tempo", "tempoEnabled", "signatureNumerator", "signatureDenominator", "timeSignatureEnabled"];
                let siblings = Value::Array(
                    array(&state["scenes"])
                        .iter()
                        .map(|scene| {
                            let mut row = fields(scene, &scene_fields);
                            for key in ["ref", "objectIdentity", "name"] {
                                if let Some(v) = scene.get(key) {
                                    row[key] = v.clone();
                                }
                            }
                            row
                        })
                        .collect(),
                );
                if args.get("expectedAuthorityRevision") != Some(&json!(simulator_revision(&siblings))) {
                    return Err(LiveError::error("scene collection changed since preview"));
                }
                if operation == "scene.fire-selected" {
                    fence(
                        args,
                        &json!({"isTriggered":state["scenes"][index]["isTriggered"],"playing":state["playback"]["transport"]["playing"]}),
                        "scene fire",
                    )?;
                    state["scenes"][index]["isTriggered"] = true.into();
                    state["playback"]["transport"]["playing"] = true.into();
                    state["set"]["playing"] = true.into();
                    drop(state);
                    self.emit(LiveEventType::Transport, None, json!({"operation":operation}));
                    return Ok(json!({"fired":true}));
                }
                let scene = &mut state["scenes"][index];
                fence(args, &fields(scene, &scene_fields), "scene")?;
                set_number(scene, args, "colorIndex", 0., 69., true)?;
                set_number(scene, args, "tempo", 20., 999., false)?;
                set_bool(scene, args, "tempoEnabled", "tempoEnabled")?;
                for key in ["signatureNumerator", "signatureDenominator"] {
                    set_number(scene, args, key, 1., 99., true)?;
                }
                set_bool(scene, args, "timeSignatureEnabled", "timeSignatureEnabled")?;
                drop(state);
                self.emit(LiveEventType::Object, Some(reference.into()), json!({"operation":operation}));
                Ok(json!({"changed":true,"revision":self.next_sequence()}))
            }
            "track.view.set" | "track.set" | "track.select-instrument" | "mixer.extended.set" => {
                let reference = string_arg(args, "ref")?;
                let mut state = self.state.borrow_mut();
                let track = state["tracks"].as_array_mut().unwrap().iter_mut().find(|t| t["ref"] == reference);
                let track = track.ok_or_else(|| {
                    LiveError::error(if operation == "mixer.extended.set" {
                        "mixer is unavailable"
                    } else {
                        "track reference is stale or invalid"
                    })
                })?;
                if operation == "mixer.extended.set" && track["mixer"].is_null() {
                    return Err(LiveError::error("mixer is unavailable"));
                }
                if track.get("objectIdentity") != args.get("expectedObjectIdentity") {
                    return Err(LiveError::error("track identity changed since preview"));
                }
                if operation == "mixer.extended.set" {
                    let mixer = &mut track["mixer"];
                    if args.get("expectedMixerIdentity") != mixer.get("mixerIdentity")
                        && args.get("expectedMixerIdentity") != Some(&json!("simulator"))
                    {
                        return Err(LiveError::error("mixer identity changed since preview"));
                    }
                    fence(args, &fields(mixer, &["crossfadeAssign", "panningMode"]), "extended mixer")?;
                    set_bool(mixer, args, "trackActivator", "trackActivator")?;
                    set_number(mixer, args, "crossfader", -1., 1., false)?;
                    set_number(mixer, args, "crossfadeAssign", 0., 2., true)?;
                    set_number(mixer, args, "panningMode", 0., 8., true)?;
                    for key in ["panningLeft", "panningRight"] {
                        set_number(mixer, args, key, -1., 1., false)?;
                    }
                } else if operation == "track.set" {
                    fence(args, &fields(track, &["colorIndex"]), "track properties")?;
                    set_number(track, args, "colorIndex", 0., 69., true)?;
                } else {
                    fence(args, &track_view(track), "track view")?;
                    if track["view"].is_null() {
                        track["view"] = json!({});
                    }
                    if operation == "track.select-instrument" {
                        track["view"]["selectedDeviceRef"] =
                            array(&track["devices"]).first().map(|d| d["ref"].clone()).unwrap_or(Value::Null);
                    } else {
                        set_bool(&mut track["view"], args, "showChains", "isShowingChains")?;
                        set_bool(&mut track["view"], args, "collapsed", "isCollapsed")?;
                        set_number(&mut track["view"], args, "deviceInsertMode", 0., 8., true)?;
                    }
                }
                drop(state);
                self.emit(LiveEventType::Object, Some(reference.into()), json!({"operation":operation}));
                Ok(if operation == "track.select-instrument" {
                    json!({"done":true})
                } else {
                    json!({"changed":true,"revision":self.next_sequence()})
                })
            }
            "selection.set" => {
                let mut state = self.state.borrow_mut();
                fence(args, &state["selection"], "selection")?;
                // As the bridge on Live 12.4.15: select_device leaves the selected track as it is, so a device on
                // another track is selected with its track; a trackRef naming another track can't be both.
                let device_track = args.get("deviceRef").filter(|v| !v.is_null()).and_then(|device| {
                    array(&state["tracks"])
                        .iter()
                        .find(|t| array(&t["devices"]).iter().any(|d| d.get("ref") == Some(device)))
                        .map(|t| t["ref"].clone())
                });
                if let Some(owner) = &device_track {
                    if args.get("trackRef").is_some_and(|named| !named.is_null() && named != owner) {
                        return Err(LiveError::error("deviceRef is on another track than trackRef"));
                    }
                }
                for (key, kind) in [
                    ("trackRef", "track"),
                    ("sceneRef", "scene"),
                    ("slotRef", "clip-slot"),
                    ("detailClipRef", "clip"),
                    ("deviceRef", "device"),
                    ("parameterRef", "parameter"),
                    ("chainRef", "chain"),
                ] {
                    if let Some(value) = args.get(key) {
                        let exists = value.is_null()
                            || match key {
                                "trackRef" => array(&state["tracks"]).iter().any(|t| t.get("ref") == Some(value)),
                                "sceneRef" => array(&state["scenes"]).iter().any(|s| s.get("ref") == Some(value)),
                                "slotRef" => array(&state["tracks"])
                                    .iter()
                                    .any(|t| array(&t["clipSlots"]).iter().any(|s| s.get("ref") == Some(value))),
                                "detailClipRef" => {
                                    array(&state["tracks"]).iter().any(|t| array(&t["clips"]).iter().any(|s| s.get("ref") == Some(value)))
                                }
                                "deviceRef" => {
                                    array(&state["tracks"]).iter().any(|t| array(&t["devices"]).iter().any(|d| d.get("ref") == Some(value)))
                                }
                                "parameterRef" => array(&state["tracks"]).iter().any(|t| {
                                    array(&t["devices"]).iter().any(|d| array(&d["parameters"]).iter().any(|p| p.get("ref") == Some(value)))
                                }),
                                _ => false,
                            };
                        if !exists {
                            return Err(LiveError::error(format!("{kind} reference is stale or invalid")));
                        }
                        state["selection"][key] = value.clone();
                    }
                }
                if let (Some(owner), None) = (device_track, args.get("trackRef").filter(|v| !v.is_null())) {
                    state["selection"]["trackRef"] = owner;
                }
                let revision = simulator_revision(&state["selection"]);
                drop(state);
                self.emit(LiveEventType::State, None, json!({"operation":operation}));
                Ok(json!({"changed":true,"revision":revision}))
            }
            "song.view.set" => {
                let mut state = self.state.borrow_mut();
                fence(args, &json!({"drawMode":state["view"]["drawMode"]}), "song view")?;
                let value = args.get("drawMode").filter(|v| v.is_boolean()).ok_or_else(|| LiveError::type_error("drawMode is invalid"))?;
                if state["view"].is_null() {
                    state["view"] = json!({"visibleView":"Session","follow":false});
                }
                state["view"]["drawMode"] = value.clone();
                let revision = simulator_revision(&json!({"drawMode":value}));
                drop(state);
                self.emit(LiveEventType::State, None, json!({"operation":operation}));
                Ok(json!({"changed":true,"revision":revision}))
            }
            "clip.view.set" => {
                let reference = string_arg(args, "ref")?;
                let mut state = self.state.borrow_mut();
                let path = clip_path(&state, reference)?;
                let clip = state.pointer_mut(&path).unwrap();
                if clip.get("objectIdentity") != args.get("expectedObjectIdentity") {
                    return Err(LiveError::error("clip identity changed since preview"));
                }
                fence(args, &fields(&clip["clipView"], &["gridQuantization", "gridIsTriplet"]), "clip view")?;
                if clip["clipView"].is_null() {
                    clip["clipView"] = json!({});
                }
                set_number(&mut clip["clipView"], args, "gridQuantization", 0., 16., true)?;
                set_bool(&mut clip["clipView"], args, "gridIsTriplet", "gridIsTriplet")?;
                if args.get("showEnvelope").is_some_and(|v| !v.is_boolean()) {
                    return Err(LiveError::type_error("showEnvelope is invalid"));
                }
                drop(state);
                self.emit(
                    LiveEventType::Object,
                    Some(reference.into()),
                    json!({"operation":operation,"showLoop":args.get("showLoop")==Some(&json!(true))}),
                );
                Ok(json!({"changed":true,"revision":self.next_sequence()}))
            }
            "device.view.set" => {
                let reference = string_arg(args, "ref")?;
                let mut state = self.state.borrow_mut();
                let device = state["tracks"]
                    .as_array_mut()
                    .unwrap()
                    .iter_mut()
                    .flat_map(|t| t["devices"].as_array_mut().unwrap())
                    .find(|d| d["ref"] == reference)
                    .ok_or_else(|| LiveError::error("device reference is stale or invalid"))?;
                if device.get("objectIdentity") != args.get("expectedObjectIdentity") {
                    return Err(LiveError::error("device identity changed since preview"));
                }
                fence(args, &json!({"collapsed":device["view"]["isCollapsed"]}), "device view")?;
                let value =
                    args.get("collapsed").filter(|v| v.is_boolean()).ok_or_else(|| LiveError::type_error("collapsed is invalid"))?;
                if device["view"].is_null() {
                    device["view"] = json!({});
                }
                device["view"]["isCollapsed"] = value.clone();
                drop(state);
                self.emit(LiveEventType::Object, Some(reference.into()), json!({"operation":operation}));
                Ok(json!({"changed":true,"revision":self.next_sequence()}))
            }
            "locator.jump-to" => {
                let reference = string_arg(args, "ref")?;
                let mut state = self.state.borrow_mut();
                let locator = array(&state["arrangement"]["locators"])
                    .iter()
                    .find(|r| r["ref"] == reference)
                    .ok_or_else(|| LiveError::error("locator reference is stale or invalid"))?;
                if locator.get("objectIdentity") != args.get("expectedObjectIdentity") {
                    return Err(LiveError::error("locator identity changed since preview"));
                }
                if args.get("expectedCollectionRevision") != Some(&json!(simulator_revision(&state["arrangement"]["locators"]))) {
                    return Err(LiveError::error("locator collection changed since preview"));
                }
                let position = locator["position"].clone();
                state["playback"]["transport"]["position"] = position.clone();
                state["set"]["position"] = position.clone();
                drop(state);
                self.emit(LiveEventType::Transport, None, json!({"operation":operation}));
                Ok(json!({"position":position}))
            }
            "application.dialog" => {
                let dialog = json!({"buttonCount":2,"message":"Save changes before closing?","openDialogCount":1,"done":true});
                if args.get("action") == Some(&json!("read")) {
                    return Ok(dialog);
                }
                if args.get("action") != Some(&json!("press")) {
                    return Err(LiveError::range_error("dialog action is invalid"));
                }
                let button = ranged_number(args.get("button").unwrap_or(&Value::Null), 0., 16., true, "dialog button is invalid")?;
                if args.get("expectedMessage") != dialog.get("message")
                    || args.get("expectedButtonCount") != dialog.get("buttonCount")
                    || args.get("expectedOpenDialogCount") != dialog.get("openDialogCount")
                {
                    return Err(LiveError::error("dialog state changed since preview"));
                }
                if button >= 2. {
                    return Err(LiveError::range_error("dialog button is not present in the current dialog"));
                }
                Ok(dialog)
            }
            "performance.read" => {
                let state = self.state.borrow();
                let tracks = array(&state["tracks"])
                    .iter()
                    .map(|track| {
                        let mut row = fields(
                            track,
                            &[
                                "ref",
                                "performanceImpact",
                                "inputMeterLeft",
                                "inputMeterRight",
                                "inputMeterLevel",
                                "outputMeterLeft",
                                "outputMeterRight",
                                "outputMeterLevel",
                            ],
                        );
                        row["devices"] = Value::Array(
                            array(&track["devices"]).iter().map(|d| fields(d, &["ref", "latencySamples", "latencyMs"])).collect(),
                        );
                        row
                    })
                    .collect::<Vec<_>>();
                let mut result = json!({"averageProcessUsage":0.42,"peakProcessUsage":0.87,"tracks":tracks});
                let revision = simulator_revision(&result);
                result["sampledAt"] = kumi_common::time::now_ms().into();
                result["revision"] = revision.into();
                Ok(result)
            }
            _ => unreachable!("view dispatcher routes only implemented operations"),
        }
    }
}

impl DeterministicLiveSimulator {
    pub(super) fn invoke_view_control(&self, operation: &str, args: &Map<String, Value>) -> Result<Value, LiveError> {
        match operation {
            "locator.jump" => {
                let direction = args
                    .get("direction")
                    .and_then(Value::as_str)
                    .filter(|d| ["next", "previous"].contains(d))
                    .ok_or_else(|| LiveError::range_error("locator jump direction is invalid"))?;
                let mut state = self.state.borrow_mut();
                let before = state["playback"]["transport"]["position"].as_f64().unwrap_or(0.);
                let mut times =
                    array(&state["arrangement"]["locators"]).iter().map(|l| l["position"].as_f64().unwrap()).collect::<Vec<_>>();
                times.sort_by(f64::total_cmp);
                if direction == "previous" {
                    times.reverse();
                }
                let position = times
                    .into_iter()
                    .find(|time| if direction == "next" { *time > before + 1e-9 } else { *time < before - 1e-9 })
                    .unwrap_or(before);
                state["playback"]["transport"]["position"] = position.into();
                state["set"]["position"] = position.into();
                drop(state);
                self.emit(LiveEventType::Transport, None, json!({"operation":operation}));
                Ok(json!({"direction":direction,"before":before,"position":position}))
            }
            "view.set" => {
                let view = bounded_text(args.get("view").unwrap_or(&Value::Null), 64, "view is invalid")?;
                let mut state = self.state.borrow_mut();
                state["view"] = json!({"visibleView":view,"follow":state["view"].get("follow").filter(|v|!v.is_null()).cloned().unwrap_or_else(||json!(false))});
                drop(state);
                self.emit(LiveEventType::State, None, json!({"operation":operation}));
                Ok(json!({"view":view,"visible":true}))
            }
            "view.control" => {
                let action = args
                    .get("action")
                    .and_then(Value::as_str)
                    .filter(|a| {
                        [
                            "zoom-in",
                            "zoom-out",
                            "scroll-left",
                            "scroll-right",
                            "follow-on",
                            "follow-off",
                            "collapse-track",
                            "expand-track",
                            "hide-view",
                            "focus-view",
                            "browser-toggle",
                        ]
                        .contains(a)
                    })
                    .ok_or_else(|| LiveError::range_error("view control action is invalid"))?;
                if ["hide-view", "focus-view"].contains(&action) {
                    bounded_text(args.get("view").unwrap_or(&Value::Null), 64, "view name is required")?;
                }
                let mut state = self.state.borrow_mut();
                if ["follow-on", "follow-off"].contains(&action) {
                    state["view"] = json!({"visibleView":state["view"].get("visibleView").filter(|v|!v.is_null()).cloned().unwrap_or_else(||json!("Session")),"follow":action=="follow-on"});
                }
                if ["collapse-track", "expand-track"].contains(&action) {
                    let reference = string_arg(args, "trackRef")?;
                    if !array(&state["tracks"]).iter().any(|t| t["ref"] == reference) {
                        return Err(LiveError::error("track reference is stale or invalid"));
                    }
                }
                drop(state);
                self.emit(LiveEventType::State, None, json!({"operation":operation}));
                Ok(json!({"action":action,"done":true}))
            }
            _ => unreachable!("view control dispatcher routes only implemented operations"),
        }
    }
}
