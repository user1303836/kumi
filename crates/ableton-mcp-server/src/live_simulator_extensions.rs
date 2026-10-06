use super::*;
use serde_json::json;
fn named(args: &Map<String, Value>, actual: &str, what: &str) -> Result<(), LiveError> {
    if let Some(expected) = args.get("expectedName").and_then(Value::as_str) {
        if expected != actual {
            return Err(LiveError::error(format!("the {what} there is \"{actual}\" now, not \"{expected}\"")));
        }
    }
    Ok(())
}
fn number(args: &Map<String, Value>, key: &str) -> Result<f64, LiveError> {
    args.get(key).and_then(Value::as_f64).filter(|v| v.is_finite()).ok_or_else(|| LiveError::type_error(format!("{key} must be a number")))
}
fn js_string(value: Option<&Value>) -> String {
    match value {
        None => "undefined".into(),
        Some(Value::Null) => "null".into(),
        Some(Value::String(s)) => s.clone(),
        Some(Value::Bool(v)) => v.to_string(),
        Some(Value::Number(n)) => kumi_common::js::json::number(n),
        Some(Value::Array(a)) => {
            a.iter().map(|v| if v.is_null() { String::new() } else { js_string(Some(v)) }).collect::<Vec<_>>().join(",")
        }
        Some(Value::Object(_)) => "[object Object]".into(),
    }
}
fn media(track: &Value) -> Option<&str> {
    if track["kind"] == "group" {
        None
    } else {
        track["mediaKind"]
            .as_str()
            .filter(|s| ["audio", "midi"].contains(s))
            .or_else(|| track["kind"].as_str().filter(|s| ["audio", "midi"].contains(s)))
    }
}
fn write_silent_wav(path: &std::path::Path, seconds: f64) -> Result<u64, LiveError> {
    use std::io::Write;
    let data = kumi_common::js::number::round(44100. * seconds) * 6.;
    if !data.is_finite() || data < 0. || data > u32::MAX as f64 - 36. {
        let value = 36. + data;
        let mut received = kumi_common::js::number::to_string(value);
        if value.is_finite() && value.abs() >= 1_000_000. && !received.contains('e') {
            let negative = received.starts_with('-');
            let digits = received.trim_start_matches('-');
            let mut grouped = String::new();
            for (i, c) in digits.chars().enumerate() {
                if i > 0 && (digits.len() - i) % 3 == 0 {
                    grouped.push('_');
                }
                grouped.push(c);
            }
            received = format!("{}{}", if negative { "-" } else { "" }, grouped);
        }
        return Err(LiveError::range_error(format!(
            "The value of \"value\" is out of range. It must be >= 0 and <= 4294967295. Received {received}"
        )));
    }
    let data = data as u32;
    let mut header = [0u8; 44];
    header[0..4].copy_from_slice(b"RIFF");
    header[4..8].copy_from_slice(&(36 + data).to_le_bytes());
    header[8..16].copy_from_slice(b"WAVEfmt ");
    header[16..20].copy_from_slice(&16u32.to_le_bytes());
    header[20..22].copy_from_slice(&1u16.to_le_bytes());
    header[22..24].copy_from_slice(&2u16.to_le_bytes());
    header[24..28].copy_from_slice(&44100u32.to_le_bytes());
    header[28..32].copy_from_slice(&(44100u32 * 6).to_le_bytes());
    header[32..34].copy_from_slice(&6u16.to_le_bytes());
    header[34..36].copy_from_slice(&24u16.to_le_bytes());
    header[36..40].copy_from_slice(b"data");
    header[40..44].copy_from_slice(&data.to_le_bytes());
    let mut file = std::fs::File::create(path).map_err(|e| LiveError::error(e.to_string()))?;
    file.write_all(&header).and_then(|_| file.set_len(44 + data as u64)).map_err(|e| LiveError::error(e.to_string()))?;
    Ok(44 + data as u64)
}
impl DeterministicLiveSimulator {
    pub(super) fn invoke_extension(&self, operation: &str, args: &Map<String, Value>) -> Result<Value, LiveError> {
        match operation {
            "render.offline" | "arrangement.midi-clip.create" | "clip.clear-range" => {
                let mut state = self.state.borrow_mut();
                let ti = array(&state["tracks"])
                    .iter()
                    .position(|t| t.get("ref") == args.get("trackRef"))
                    .ok_or_else(|| LiveError::error("track reference is stale or invalid"))?;
                let track = &state["tracks"][ti];
                let reference = track["ref"].as_str().unwrap().to_string();
                let name = track["name"].as_str().unwrap();
                named(args, name, "track")?;
                if operation == "render.offline" {
                    if media(track) != Some("audio") {
                        return Err(LiveError::error(format!(
                            "track \"{name}\" isn't an audio track: offline renders are of an audio track's own clips, before its devices"
                        )));
                    }
                    let from = number(args, "fromBeat")?;
                    let to = number(args, "toBeat")?;
                    if to <= from {
                        return Err(LiveError::error("the range to render is empty"));
                    }
                    let seconds = (to - from) * 60. / state["set"]["tempo"].as_f64().unwrap_or(120.);
                    let path =
                        std::env::temp_dir().join(format!("kumi-simulated-render-{}-{}.wav", std::process::id(), self.next_sequence()));
                    let bytes = write_silent_wav(&path, seconds)?;
                    return Ok(
                        json!({"path":path.to_string_lossy(),"format":"wav","channels":2,"sampleRate":44100,"bitDepth":24,"seconds":seconds,"bytes":bytes,"renderMs":0}),
                    );
                }
                if operation == "arrangement.midi-clip.create" {
                    if media(track) != Some("midi") {
                        return Err(LiveError::error(format!("track \"{name}\" isn't a MIDI track")));
                    }
                    if args.contains_key("takeLaneRef") {
                        return Err(LiveError::error("the simulator makes Arrangement clips on a track's own lane"));
                    }
                    let start = number(args, "start")?;
                    let length = number(args, "length")?;
                    let rows = args.get("notes").and_then(Value::as_array).ok_or_else(|| LiveError::type_error("notes must be a list"))?;
                    let mut notes = Vec::new();
                    let mut unsupported = false;
                    for note in rows {
                        if note.is_null() {
                            return Err(LiveError::type_error("Cannot read properties of null (reading 'pitch')"));
                        }
                        let mut row = Map::new();
                        for key in ["pitch", "start", "duration"] {
                            if let Some(v) = note.get(key) {
                                row.insert(key.into(), v.clone());
                            } else {
                                unsupported = true;
                            }
                        }
                        for (key, default) in [
                            ("velocity", json!(100)),
                            ("mute", json!(false)),
                            ("probability", json!(1)),
                            ("velocityDeviation", json!(0)),
                            ("releaseVelocity", json!(64)),
                        ] {
                            row.insert(key.into(), note.get(key).filter(|v| !v.is_null()).cloned().unwrap_or(default));
                        }
                        row.insert("channel".into(), json!(1));
                        row.insert("id".into(), self.next_note_id.get().into());
                        self.next_note_id.set(self.next_note_id.get() + 1);
                        notes.push(Value::Object(row));
                    }
                    let sequence = self.next_sequence();
                    if unsupported {
                        return Err(LiveError::error("unsupported simulator authority value"));
                    }
                    let mut clip = json!({"ref":format!("arrangement-clip:{reference}:{sequence}"),"objectIdentity":format!("simulator:arrangement-clip:{sequence}"),"name":args.get("name").and_then(Value::as_str).unwrap_or(""),"kind":"midi","start":start,"length":length,"notes":notes,"notesRevision":simulator_revision(&json!(notes)),"warp":false,"takes":[],"automation":[]});
                    if let Some(looping) = args.get("looping").filter(|v| v.is_boolean()) {
                        clip["looping"] = looping.clone();
                    }
                    if state["arrangementClips"].is_null() {
                        state["arrangementClips"] = json!([]);
                    }
                    state["arrangementClips"].as_array_mut().unwrap().push(json!({"clip":clip,"trackRef":reference}));
                    drop(state);
                    self.emit(LiveEventType::Object, Some(reference.clone().into()), json!({"operation":operation,"clip":clip["ref"]}));
                    return Ok(
                        json!({"ref":clip["ref"],"trackRef":reference,"name":clip["name"],"start":start,"end":start+length,"notes":notes.len()}),
                    );
                }
                if args.contains_key("takeLaneRef") {
                    return Err(LiveError::error("a range is cleared on the track's own lane"));
                }
                let from = number(args, "fromBeat")?;
                let to = number(args, "toBeat")?;
                if to <= from {
                    return Err(LiveError::error("the range to clear is empty"));
                }
                let mut all = array(&state["arrangementClips"]).to_vec();
                let mut removed = Vec::new();
                let mut indexes = Vec::new();
                let mut rests = Vec::new();
                let mut before = 0;
                for (index, row) in all.iter_mut().enumerate() {
                    if row["trackRef"] != reference {
                        continue;
                    }
                    before += 1;
                    let clip = &mut row["clip"];
                    let start = clip["start"].as_f64().unwrap();
                    let length = clip["length"].as_f64().unwrap();
                    let end = clip["endTime"].as_f64().unwrap_or(start + length);
                    if start >= from && end <= to {
                        removed.push(json!({"name":clip["name"],"start":start,"end":start+length,"isAudio":clip["kind"]=="audio"}));
                        indexes.push(index);
                        continue;
                    }
                    if start < to && end > from {
                        if start < from && end > to {
                            // A clip crossing both edges is split: its far end stays, a clip of its own.
                            let sequence = self.next_sequence();
                            let mut rest = row.clone();
                            rest["clip"]["ref"] = json!(format!("arrangement-clip:{reference}:{sequence}"));
                            rest["clip"]["objectIdentity"] = json!(format!("simulator:arrangement-clip:{sequence}"));
                            rest["clip"]["start"] = to.into();
                            rest["clip"]["length"] = (end - to).into();
                            rest["clip"]["endTime"] = end.into();
                            rests.push(rest);
                        }
                        let clip = &mut row["clip"];
                        if start < from {
                            clip["length"] = length.min(from - start).into();
                            clip["endTime"] = from.into();
                        } else {
                            clip["length"] = length.min(end - to).into();
                            clip["start"] = to.into();
                            clip["endTime"] = end.into();
                        }
                    }
                }
                let split = rests.len();
                state["arrangementClips"] = Value::Array(
                    all.into_iter().enumerate().filter(|(i, _)| !indexes.contains(i)).map(|(_, row)| row).chain(rests).collect(),
                );
                drop(state);
                self.emit(LiveEventType::Object, Some(reference.clone().into()), json!({"operation":operation}));
                Ok(json!({"trackRef":reference,"clipsBefore":before,"clipsAfter":before-removed.len()+split,"removed":removed}))
            }
            "device.duplicate" => {
                let mut state = self.state.borrow_mut();
                let mut owners = (0..array(&state["tracks"]).len()).map(|i| format!("/tracks/{i}")).collect::<Vec<_>>();
                let mut at = 0;
                while at < owners.len() {
                    let owner = state.pointer(&owners[at]).unwrap();
                    let extra = array(&owner["devices"])
                        .iter()
                        .enumerate()
                        .flat_map(|(di, d)| {
                            let prefix = &owners[at];
                            array(&d["chains"]).iter().enumerate().map(move |(ci, _)| format!("{prefix}/devices/{di}/chains/{ci}"))
                        })
                        .collect::<Vec<_>>();
                    owners.extend(extra);
                    at += 1;
                }
                for path in owners {
                    let owner = state.pointer_mut(&path).unwrap();
                    let index = array(&owner["devices"]).iter().position(|d| d.get("ref") == args.get("ref"));
                    let Some(index) = index else {
                        continue;
                    };
                    let device = &owner["devices"][index];
                    named(args, device["name"].as_str().unwrap(), "device")?;
                    if args.get("expectedObjectIdentity").is_some_and(|v| device.get("objectIdentity") != Some(v)) {
                        return Err(LiveError::error("device identity changed since preview"));
                    }
                    let siblings = array(&owner["devices"])
                        .iter()
                        .map(|d| json!({"ref":d["ref"],"objectIdentity":d["objectIdentity"]}))
                        .collect::<Vec<_>>();
                    if args.get("expectedSiblings").is_some_and(|v| simulator_revision(v) != simulator_revision(&json!(siblings))) {
                        return Err(LiveError::error("device siblings changed since preview"));
                    }
                    let sequence = self.next_sequence();
                    let reference = owner["ref"].as_str().unwrap().to_string();
                    let mut copy = device.clone();
                    copy["ref"] = format!("device:{reference}:copy-{sequence}").into();
                    copy["objectIdentity"] = format!("simulator:device:copy-{sequence}").into();
                    owner["devices"].as_array_mut().unwrap().insert(index + 1, copy.clone());
                    drop(state);
                    self.emit(LiveEventType::Object, Some(reference.into()), json!({"operation":operation,"device":copy["ref"]}));
                    return Ok(
                        json!({"ref":copy["ref"],"name":copy["name"],"index":index+1,"objectIdentity":copy["objectIdentity"],"createdFingerprint":simulator_revision(&owned_device_fingerprint_row(&copy))}),
                    );
                }
                Err(LiveError::error("device reference is stale or invalid"))
            }
            "drum-pad.sample-chain" => {
                let state = self.state.borrow();
                let devices = all_device_rows(&state);
                let rack = devices
                    .iter()
                    .find(|d| d.get("ref") == args.get("rackRef"))
                    .filter(|d| !array(&d["drumPads"]).is_empty())
                    .ok_or_else(|| LiveError::error("that isn't a Drum Rack"))?;
                let name = rack["name"].as_str().unwrap();
                named(args, name, "Drum Rack")?;
                let note = number(args, "note")?;
                let pad = array(&rack["drumPads"]).iter().find(|p| p["note"].as_f64() == Some(note)).ok_or_else(|| {
                    LiveError::error(format!("the Drum Rack has no pad on note {}", kumi_common::js::number::to_string(note)))
                })?;
                if !array(&pad["chains"]).is_empty() {
                    return Err(LiveError::error(format!(
                        "drum pad {} of \"{name}\" already plays something; clear it first",
                        kumi_common::js::number::to_string(note)
                    )));
                }
                let reference = pad["ref"].as_str().unwrap().to_string();
                let mut input = json!({"ref":reference,"expectedObjectIdentity":pad["objectIdentity"]});
                for key in ["samplePath", "name"] {
                    if let Some(v) = args.get(key) {
                        input[key] = v.clone();
                    }
                }
                drop(state);
                let loaded = self.load_drum_pad_sample(input.as_object().unwrap(), operation)?;
                let state = self.state.borrow();
                let path = super::simulator_device_state::pad_path(&state, &reference).unwrap();
                let chain = &state.pointer(&path).unwrap()["chains"][0];
                Ok(json!({"chainRef":chain["ref"],"deviceRef":chain["devices"][0]["ref"],"note":note,"samplePath":loaded["samplePath"]}))
            }
            "project.import" => {
                let path = js_string(args.get("filePath"));
                let name = path.rsplit(['\\', '/']).next().unwrap_or("sample");
                let mut path = std::env::temp_dir().join("Kumi Simulated Project").join("Samples").join("Imported");
                match name {
                    "" | "." => {}
                    ".." => {
                        path.pop();
                    }
                    name => path.push(name),
                }
                Ok(json!({"path":path.to_string_lossy()}))
            }
            "transaction.group" => {
                let steps = args.get("ops").map(array).unwrap_or(&[]);
                let mut results = Vec::new();
                for (index, step) in steps.iter().enumerate() {
                    if step.is_null() {
                        return Err(LiveError::type_error("Cannot read properties of null (reading 'operation')"));
                    }
                    let name = step.get("operation").and_then(Value::as_str);
                    if name == Some("transaction.group") || name.is_none_or(|name| !EXTENSION_OPERATIONS.contains(&name)) {
                        return Err(LiveError::error(format!(
                            "step {}: a group can't hold {}",
                            index + 1,
                            js_string(step.get("operation"))
                        )));
                    }
                    let name = name.unwrap();
                    let result = if step.get("args").is_none_or(Value::is_null) {
                        let field = match name {
                            "device.duplicate" => "ref",
                            "drum-pad.sample-chain" => "rackRef",
                            "project.import" => "filePath",
                            _ => "trackRef",
                        };
                        Err(LiveError::type_error(format!(
                            "Cannot read properties of {} (reading '{field}')",
                            if step.get("args").is_none() { "undefined" } else { "null" }
                        )))
                    } else {
                        self.invoke_extension(name, step["args"].as_object().unwrap_or(&Map::new()))
                    };
                    match result {
                        Ok(value) => results.push(value),
                        Err(error) => {
                            return Err(LiveError::error(format!(
                                "step {} failed ({error}); {} of {} steps were made",
                                index + 1,
                                results.len(),
                                steps.len()
                            )))
                        }
                    }
                }
                Ok(json!({"results":results}))
            }
            _ => unreachable!("extension dispatcher routes only implemented operations"),
        }
    }
}
