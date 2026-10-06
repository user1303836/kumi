use super::simulator_device_state::nested_device_path;
use super::simulator_views::{fence, fields};
use super::*;
use serde_json::json;
fn text<'a>(args: &'a Map<String, Value>, key: &str) -> Result<&'a str, LiveError> {
    string_arg(args, key).map_err(|_| LiveError::type_error(format!("{key} is invalid")))
}
fn number(args: &Map<String, Value>, key: &str, min: f64, max: f64) -> Result<f64, LiveError> {
    ranged_number(args.get(key).unwrap_or(&Value::Null), min, max, false, &format!("{key} is invalid"))
}
fn note_authority(state: &Value, reference: &str, args: &Map<String, Value>) -> Result<(), LiveError> {
    let current = DeterministicLiveSimulator::note_clip_authority(state, reference)?;
    let expected = args.get("expectedClipAuthority").ok_or_else(|| LiveError::error("unsupported simulator authority value"))?;
    if simulator_revision(&current) != simulator_revision(expected) {
        Err(LiveError::error("note clip hierarchy identity changed since preview"))
    } else {
        Ok(())
    }
}
fn numeric(value: &Value) -> f64 {
    match value {
        Value::Null => 0.,
        Value::Bool(v) => {
            if *v {
                1.
            } else {
                0.
            }
        }
        Value::Number(n) => n.as_f64().unwrap_or(f64::NAN),
        Value::String(s) => {
            if s.trim().is_empty() {
                0.
            } else {
                kumi_common::js::number::parse(s).unwrap_or(f64::NAN)
            }
        }
        _ => f64::NAN,
    }
}
impl DeterministicLiveSimulator {
    pub(super) fn invoke_lom(&self, operation: &str, args: &Map<String, Value>) -> Result<Value, LiveError> {
        match operation {
            "data.set" if args.get("entries").is_some() => {
                // Entries, all or none, as the Remote Script saves them: every one checked before any is written.
                let entries: Vec<Map<String, Value>> =
                    args["entries"].as_array().into_iter().flatten().filter_map(|entry| entry.as_object().cloned()).collect();
                let mut prior = Vec::new();
                {
                    let state = self.state.borrow();
                    for entry in &entries {
                        let owner = text(entry, "ref")?;
                        if state["set"]["ref"] != owner && !array(&state["tracks"]).iter().any(|t| t["ref"] == owner) {
                            return Err(LiveError::error("track reference is stale or invalid"));
                        }
                        let key = text(entry, "key")?;
                        if !key.starts_with("kumi.") {
                            return Err(LiveError::error("Kumi writes only its own keys (kumi.…); other keys are read-only"));
                        }
                        let held =
                            self.stored_data.borrow().get(&format!("{owner}\0{key}")).cloned().map(Value::String).unwrap_or(Value::Null);
                        if entry.get("expectedValue").is_some_and(|v| v != &held) {
                            return Err(LiveError::error("the data under that key changed since it was read"));
                        }
                        prior.push(held);
                    }
                }
                let mut saved = Vec::new();
                for (entry, prior) in entries.iter().zip(prior) {
                    let (owner, key) = (text(entry, "ref")?, text(entry, "key")?);
                    let slot = format!("{owner}\0{key}");
                    match entry.get("value").and_then(Value::as_str) {
                        Some(v) => self.stored_data.borrow_mut().insert(slot, v.into()),
                        None => self.stored_data.borrow_mut().remove(&slot),
                    };
                    saved.push(json!({"ref":owner,"key":key,"value":entry.get("value"),"prior":prior}));
                }
                Ok(json!({"entries":saved}))
            }
            "data.get" | "data.set" => {
                let owner = text(args, "ref")?;
                let state = self.state.borrow();
                if state["set"]["ref"] != owner && !array(&state["tracks"]).iter().any(|t| t["ref"] == owner) {
                    return Err(LiveError::error("track reference is stale or invalid"));
                }
                let key = text(args, "key")?;
                let slot = format!("{owner}\0{key}");
                let prior = self.stored_data.borrow().get(&slot).cloned().map(Value::String).unwrap_or(Value::Null);
                if operation == "data.get" {
                    return Ok(json!({"ref":owner,"key":key,"value":prior}));
                }
                if !key.starts_with("kumi.") {
                    return Err(LiveError::error("Kumi writes only its own keys (kumi.…); other keys are read-only"));
                }
                let value = args
                    .get("value")
                    .filter(|v| v.is_null() || v.is_string())
                    .ok_or_else(|| LiveError::type_error("value must be text of at most 1 MiB, or null"))?;
                if args.get("expectedValue").is_some_and(|v| v != &prior) {
                    return Err(LiveError::error("the data under that key changed since it was read"));
                }
                if let Some(v) = value.as_str() {
                    self.stored_data.borrow_mut().insert(slot, v.into());
                } else {
                    self.stored_data.borrow_mut().remove(&slot);
                }
                Ok(json!({"ref":owner,"key":key,"value":value,"prior":prior}))
            }
            "note.select" | "note.delete-range" => {
                let reference = text(args, "ref")?;
                let mut state = self.state.borrow_mut();
                let path = note_clip_path(&state, reference)?;
                note_authority(&state, reference, args)?;
                let clip = state.pointer_mut(&path).unwrap();
                if operation == "note.select" {
                    let modes = ["noteIds", "all", "none"].into_iter().filter(|key| args.contains_key(*key)).collect::<Vec<_>>();
                    if modes.len() != 1 || (modes[0] != "noteIds" && args.get(modes[0]) != Some(&json!(true))) {
                        return Err(LiveError::error("name exactly one of noteIds, all: true or none: true"));
                    }
                    let selected = if args.get("all") == Some(&json!(true)) {
                        array(&clip["notes"]).iter().map(|n| n["id"].clone()).collect::<Vec<_>>()
                    } else if args.get("none") == Some(&json!(true)) {
                        vec![]
                    } else {
                        let ids = args
                            .get("noteIds")
                            .and_then(Value::as_array)
                            .filter(|a| !a.is_empty())
                            .ok_or_else(|| LiveError::range_error("note ids are invalid"))?;
                        let mut seen = Vec::new();
                        for id in ids {
                            let n = ranged_number(id, 0., f64::INFINITY, true, "note ids are invalid")?;
                            if seen.contains(&n) {
                                return Err(LiveError::range_error("note ids are invalid"));
                            }
                            seen.push(n);
                        }
                        if ids.iter().any(|id| !array(&clip["notes"]).iter().any(|n| n["id"].as_f64() == id.as_f64())) {
                            return Err(LiveError::error("note id is not present in the clip"));
                        }
                        ids.clone()
                    };
                    let count = selected.len();
                    self.selected_notes.borrow_mut().insert(reference.into(), selected);
                    return Ok(json!({"selected":count}));
                }
                let revision = clip
                    .get("notesRevision")
                    .filter(|v| !v.is_null())
                    .cloned()
                    .unwrap_or_else(|| json!(simulator_revision(&clip["notes"])));
                if args.get("expectedNotesRevision") != Some(&revision) {
                    return Err(LiveError::error("clip notes changed since preview"));
                }
                let pitch = number(args, "fromPitch", 0., 127.)?;
                let span = number(args, "pitchSpan", 1., 128.)?;
                let from = number(args, "fromTime", 0., f64::INFINITY)?;
                let time = number(args, "timeSpan", 0.001, f64::INFINITY)?;
                if pitch.fract() != 0. || span.fract() != 0. {
                    return Err(LiveError::range_error("the pitch range is invalid"));
                }
                let notes = clip["notes"].as_array_mut().unwrap();
                let before = notes.len();
                notes.retain(|note| {
                    let p = note["pitch"].as_f64().unwrap();
                    let s = note["start"].as_f64().unwrap();
                    !(p >= pitch && p < pitch + span && s >= from && s < from + time)
                });
                let deleted = before - notes.len();
                let revision = simulator_revision(&clip["notes"]);
                clip["notesRevision"] = revision.clone().into();
                drop(state);
                self.emit(LiveEventType::Object, Some(reference.into()), json!({"operation":operation}));
                self.next_sequence();
                Ok(json!({"deleted":deleted,"notesRevision":revision}))
            }
            "fire-button.set" => {
                let reference = text(args, "ref")?;
                let pressed =
                    args.get("pressed").and_then(Value::as_bool).ok_or_else(|| LiveError::type_error("pressed must be true or false"))?;
                let safety = args.get("outputSafety");
                if !safety.is_some_and(|v| {
                    v["safe"] == true && v["provenance"].as_str().is_some_and(|p| !["", "unknown", "simulator"].contains(&p))
                }) {
                    return Err(LiveError::error("explicit output-safety evidence is required"));
                }
                let mut state = self.state.borrow_mut();
                let clip = array(&state["tracks"]).iter().enumerate().find_map(|(ti, t)| {
                    array(&t["clips"]).iter().position(|c| c["ref"] == reference).map(|ci| format!("/tracks/{ti}/clips/{ci}"))
                });
                let slot = array(&state["tracks"]).iter().enumerate().find_map(|(ti, t)| {
                    array(&t["clipSlots"])
                        .iter()
                        .position(|s| s["ref"] == reference || (clip.is_some() && s["clipRef"] == reference))
                        .map(|si| format!("/tracks/{ti}/clipSlots/{si}"))
                });
                let scene = array(&state["scenes"]).iter().position(|s| s["ref"] == reference).map(|si| format!("/scenes/{si}"));
                let target = clip.as_ref().or(slot.as_ref()).or(scene.as_ref());
                let button = scene.as_ref().or(slot.as_ref());
                let (target, button) = target.zip(button).ok_or_else(|| LiveError::error("that has no launch button"))?;
                if state.pointer(target).unwrap().get("objectIdentity") != args.get("expectedObjectIdentity") {
                    return Err(LiveError::error("fire button target changed since preview"));
                }
                state.pointer_mut(button).unwrap()["fireButtonState"] = pressed.into();
                if pressed {
                    self.held_fire_buttons.borrow_mut().insert(reference.into());
                } else {
                    self.held_fire_buttons.borrow_mut().remove(reference);
                }
                drop(state);
                self.emit(LiveEventType::Transport, None, json!({"operation":operation,"pressed":pressed}));
                Ok(json!({"ref":reference,"pressed":pressed}))
            }
            "track.action" => {
                let reference = text(args, "ref")?;
                let state = self.state.borrow();
                let track = array(&state["tracks"])
                    .iter()
                    .find(|t| t["ref"] == reference)
                    .ok_or_else(|| LiveError::error("track reference is stale or invalid"))?;
                if args.get("action") != Some(&json!("jump-in-running-clip")) {
                    return Err(LiveError::range_error("track action is invalid"));
                }
                if track.get("objectIdentity") != args.get("expectedObjectIdentity") {
                    return Err(LiveError::error("track identity changed since preview"));
                }
                let beats = number(args, "beats", -1_000_000., 1_000_000.)?;
                if track["playingSlotIndex"].is_null() {
                    return Err(LiveError::error("no Session clip is playing on this track"));
                }
                drop(state);
                self.emit(LiveEventType::Transport, None, json!({"operation":operation,"beats":beats}));
                Ok(json!({"done":true}))
            }
            "clip.time-convert" => {
                let reference = text(args, "ref")?;
                let state = self.state.borrow();
                let path = clip_path(&state, reference)?;
                let clip = state.pointer(&path).unwrap();
                if clip["kind"] != "audio" {
                    return Err(LiveError::error("only an audio clip converts between beats and its sample's time"));
                }
                let rate = clip["sampleRate"].as_f64().unwrap_or(44100.);
                let seconds_per_beat = 60. / state["set"]["tempo"].as_f64().unwrap_or(120.);
                let warped = clip["warp"] != false;
                let value = number(args, "value", f64::NEG_INFINITY, f64::INFINITY)?;
                Ok(match args.get("from").and_then(Value::as_str) {
                    Some("beats") => {
                        let samples = warped.then_some(value * seconds_per_beat * rate);
                        json!({"beats":value,"samples":samples,"seconds":samples.map(|v|v/rate)})
                    }
                    Some("samples") => json!({"beats":warped.then_some(value/rate/seconds_per_beat),"samples":value,"seconds":value/rate}),
                    Some("seconds") => json!({"beats":warped.then_some(value/seconds_per_beat),"samples":value*rate,"seconds":value}),
                    _ => return Err(LiveError::range_error("time conversion arguments are invalid")),
                })
            }
            "application.message" => {
                let message = args
                    .get("text")
                    .and_then(Value::as_str)
                    .filter(|s| !s.is_empty() && kumi_common::js::string::utf16_len(s) <= 1024)
                    .ok_or_else(|| LiveError::range_error("message arguments are invalid"))?;
                if args.get("modal").is_some_and(|v| !v.is_boolean()) {
                    return Err(LiveError::range_error("message arguments are invalid"));
                }
                self.shown_messages.borrow_mut().push(json!({"text":message,"modal":args.get("modal")==Some(&json!(true))}));
                Ok(json!({"shown":true}))
            }
            "browser.preview.start" => {
                let item_id = text(args, "itemId")?;
                let catalog = Self::browser_catalog();
                let item = catalog.iter().find(|i| i["id"] == item_id).ok_or_else(|| LiveError::error("browser item is not present"))?;
                if args.get("expectedName") != item.get("name") || args.get("expectedItemIdentity") != item.get("objectIdentity") {
                    return Err(LiveError::error("browser item identity changed since it was found"));
                }
                let preview = format!("preview_simulator_{:024}", self.next_sequence());
                *self.browser_preview.borrow_mut() = Some(preview.clone());
                Ok(json!({"previewId":preview,"started":true}))
            }
            "browser.preview.stop" => {
                let preview = self.browser_preview.borrow().clone();
                if preview.as_ref().is_none_or(|p| args.get("previewId") != Some(&json!(p))) {
                    return Err(LiveError::error("that preview isn't playing any more"));
                }
                self.browser_preview.borrow_mut().take();
                Ok(json!({"stopped":true}))
            }
            _ => self.invoke_lom_device(operation, args),
        }
    }
    fn invoke_lom_device(&self, operation: &str, args: &Map<String, Value>) -> Result<Value, LiveError> {
        let reference = text(args, "ref")?;
        let mut state = self.state.borrow_mut();
        let path = nested_device_path(&state, reference).ok_or_else(|| LiveError::error("device reference is stale or invalid"))?;
        let device = state.pointer_mut(&path).unwrap();
        if !["plugin.parameter-names", "device.banks.read"].contains(&operation)
            && device.get("objectIdentity") != args.get("expectedObjectIdentity")
        {
            return Err(LiveError::error("device identity changed since preview"));
        }
        let mut result = match operation {
            "device.property.set" => {
                let property =
                    args.get("property").and_then(Value::as_str).ok_or_else(|| LiveError::range_error("device property is unknown"))?;
                let spec = DEVICE_PROPERTIES.get(property).ok_or_else(|| LiveError::range_error("device property is unknown"))?;
                let row = device
                    .get_mut(&spec.row_key)
                    .filter(|v| v.is_object() || v.is_array())
                    .ok_or_else(|| LiveError::error(format!("{property} is unavailable on this device")))?;
                let current = row
                    .get(&spec.field)
                    .filter(|v| !v.is_null())
                    .ok_or_else(|| LiveError::error(format!("{property} is unavailable on this device")))?;
                fence(args, &json!({"property":property,"value":current}), "device property")?;
                let value = args.get("value");
                let choices = spec.choices.as_ref().and_then(|key| row.get(key)).and_then(Value::as_array);
                let valid = if current.is_boolean() {
                    value.is_some_and(Value::is_boolean)
                } else {
                    value
                        .and_then(Value::as_f64)
                        .is_some_and(|v| v.is_finite() && choices.is_none_or(|a| v.fract() == 0. && v >= 0. && v < (a.len() as f64)))
                };
                if !valid {
                    return Err(LiveError::range_error(format!("{property} is not one of its choices")));
                }
                row[&spec.field] = value.unwrap().clone();
                json!({"changed":true,"value":value.unwrap()})
            }
            "device.action" => {
                let action = args.get("action").and_then(Value::as_str);
                let family = match action {
                    Some("cc-control-resend") => "ccControl",
                    Some("simpler-warp-as" | "simpler-warp-double" | "simpler-warp-half") => "simpler",
                    _ => return Err(LiveError::range_error("device action is unknown")),
                };
                if !device.get(family).is_some_and(|v| v.is_object() || v.is_array()) {
                    return Err(LiveError::error(format!(
                        "this needs a {}; that device isn't one",
                        if family == "simpler" { "Simpler" } else { "CC Control" }
                    )));
                }
                let sample = if family == "simpler" {
                    device.get("sample").filter(|v| v.is_object() || v.is_array()).cloned().unwrap_or(Value::Null)
                } else {
                    Value::Null
                };
                fence(args, &json!({"sample":sample}), "device")?;
                if (action == Some("simpler-warp-as")) != args.contains_key("beats") {
                    return Err(LiveError::range_error("beats goes with simpler-warp-as, and only with it"));
                }
                if action == Some("simpler-warp-as") {
                    number(args, "beats", 0.001, f64::INFINITY)?;
                }
                if family == "simpler" {
                    if sample.is_null() {
                        return Err(LiveError::error("this Simpler has no sample"));
                    }
                    device["sample"]["warping"] = true.into();
                }
                json!({"done":true})
            }
            "sample.set" | "wavetable.set" => {
                let sample = operation == "sample.set";
                let key = if sample { "sample" } else { "wavetable" };
                let names = if sample { SAMPLE_FIELDS } else { WAVETABLE_FIELDS };
                let row = device.get_mut(key).filter(|v| v.is_object() || v.is_array()).ok_or_else(|| {
                    LiveError::error(if sample { "this Simpler has no sample" } else { "this needs a Wavetable; that device isn't one" })
                })?;
                fence(args, &fields(row, names), key)?;
                let given = names.iter().filter(|n| args.contains_key(**n)).collect::<Vec<_>>();
                if given.is_empty() {
                    return Err(LiveError::error(format!("{key} mutation has no fields")));
                }
                for key in given {
                    let value = number(args, key, 0., if sample { 1_000_000. } else { 100_000. })?;
                    if !sample && value.fract() != 0. {
                        return Err(LiveError::range_error(format!("{key} is invalid")));
                    }
                    row[*key] = args[*key].clone();
                }
                json!({"changed":true})
            }
            "sample.slice" => {
                let sample = device
                    .get_mut("sample")
                    .filter(|v| v.is_object() || v.is_array())
                    .ok_or_else(|| LiveError::error("this Simpler has no sample"))?;
                let before = array(&sample["slices"]).to_vec();
                fence(args, &json!({"slices":before}), "slice")?;
                let time = args.get("time");
                let to = args.get("toTime");
                let mut slices = match args.get("action").and_then(Value::as_str) {
                    Some("insert") => {
                        if time.is_some_and(|v| before.contains(v)) {
                            return Err(LiveError::error("a slice is already at that time"));
                        }
                        let mut a = before;
                        a.push(time.cloned().unwrap_or(Value::Null));
                        a
                    }
                    Some("move") => {
                        if !time.is_some_and(|v| before.contains(v)) {
                            return Err(LiveError::error("no slice is at that time"));
                        }
                        if to.is_some_and(|v| before.contains(v)) && to != time {
                            return Err(LiveError::error("a slice is already at the new time"));
                        }
                        before.into_iter().map(|v| if Some(&v) == time { to.cloned().unwrap_or(Value::Null) } else { v }).collect()
                    }
                    Some("remove") => {
                        if !time.is_some_and(|v| before.contains(v)) {
                            return Err(LiveError::error("no slice is at that time"));
                        }
                        before.into_iter().filter(|v| Some(v) != time).collect()
                    }
                    Some("clear") => vec![],
                    Some("reset") => array(&sample["detectedSlices"]).to_vec(),
                    _ => return Err(LiveError::range_error("slice action is unknown")),
                };
                if args.get("action").is_some_and(|a| a == "insert" || a == "move") {
                    slices.sort_by(|a, b| (numeric(a) - numeric(b)).partial_cmp(&0.).unwrap_or(std::cmp::Ordering::Equal));
                }
                sample["slices"] = json!(slices);
                json!({"slices":slices})
            }
            "wavetable.modulation.set" => {
                let parameters = device["parameters"].clone();
                let table = device
                    .get_mut("wavetable")
                    .filter(|v| v.is_object() || v.is_array())
                    .ok_or_else(|| LiveError::error("this needs a Wavetable; that device isn't one"))?;
                let mut targets = array(&table["visibleModulationTargetNames"]).to_vec();
                fence(args, &json!({"targets":targets}), "Wavetable modulation")?;
                if args.contains_key("targetIndex") == args.contains_key("parameterRef") {
                    return Err(LiveError::range_error("name exactly one of targetIndex or parameterRef"));
                }
                let source = number(args, "source", 0., 1000.)?;
                let value = number(args, "value", -1., 1.)?;
                let target = if args.contains_key("targetIndex") {
                    let index = number(args, "targetIndex", 0., targets.len() as f64 - 1.)?;
                    if index.fract() != 0. {
                        return Err(LiveError::range_error("targetIndex is not one of the matrix's targets"));
                    }
                    index as usize
                } else {
                    let parameter = array(&parameters)
                        .iter()
                        .find(|p| p.get("ref") == args.get("parameterRef"))
                        .ok_or_else(|| LiveError::error("parameterRef must name one of this Wavetable's parameters"))?;
                    if let Some(index) = targets.iter().position(|t| t == &parameter["name"]) {
                        index
                    } else {
                        targets.push(parameter["name"].clone());
                        table["visibleModulationTargetNames"] = json!(targets);
                        targets.len() - 1
                    }
                };
                let key = format!("{target}:{}", kumi_common::js::number::to_string(source));
                let prior = self.modulation_amounts.borrow_mut().entry(reference.into()).or_default().insert(key, value).unwrap_or(0.);
                json!({"changed":true,"targetIndex":target,"value":value,"prior":prior})
            }
            "plugin.parameter-names" => {
                let names = device
                    .get("parameterNames")
                    .and_then(Value::as_array)
                    .ok_or_else(|| LiveError::error("only a plug-in lists all its parameter names"))?;
                let begin = if args.contains_key("begin") { number(args, "begin", 0., f64::INFINITY)? } else { 0. };
                let end = if args.contains_key("end") { number(args, "end", -1., f64::INFINITY)? } else { -1. };
                let from = (begin.trunc() as usize).min(names.len());
                let to = if end == -1. { names.len() } else { (end.trunc() as usize).min(names.len()) };
                let listed = &names[from..to.max(from)];
                return Ok(json!({"names":listed,"total":if begin==0.&&end== -1.{Some(listed.len())}else{None}}));
            }
            "device.banks.read" => {
                let banks = device
                    .get("banks")
                    .filter(|v| v.is_array())
                    .ok_or_else(|| LiveError::error("only a Max for Live device lists its banks"))?;
                return Ok(json!({"banks":banks}));
            }
            _ => unreachable!("LOM dispatcher routes only implemented operations"),
        };
        drop(state);
        self.emit(LiveEventType::Object, Some(reference.into()), json!({"operation":operation}));
        result["revision"] = self.next_sequence().into();
        Ok(result)
    }
}
