use super::*;
fn only(value: &Value, allowed: &[&str]) -> bool {
    value.as_object().is_some_and(|row| row.keys().all(|key| allowed.contains(&key.as_str())))
}
fn finite(value: &Value, min: f64, max: f64) -> bool {
    value.as_f64().is_some_and(|number| number.is_finite() && number >= min && number <= max)
}
fn integer(value: &Value, min: f64, max: f64) -> bool {
    finite(value, min, max) && number(value).fract() == 0.0
}
fn proposed_fields(operation: &Value, fields: &[&str]) -> Value {
    Value::Object(fields.iter().filter_map(|key| operation.get(*key).map(|value| ((*key).into(), value.clone()))).collect())
}
pub(super) fn validate_operation(value: &Value, index: usize) -> Result<Value, LiveError> {
    let error = |text: &str| fail(format!("transaction batch operation {index} {text}"));
    let kind = string(&value["kind"]);
    if !value.is_object() || !BATCH_OPERATION_KINDS.contains(&kind) {
        return Err(error("kind is not in the composable allowlist"));
    }
    match kind {
        "mixer.set" => {
            if !only(value, &["kind", "trackRef", "volume", "pan", "mute", "solo", "cueVolume", "sends"])
                || !is_non_empty_string(&value["trackRef"], 256)
            {
                return Err(error("requires an exact trackRef"));
            }
            let mut proposed = json!({});
            for field in MIXER_STATE_FIELDS {
                let Some(v) = value.get(*field) else { continue };
                match *field {
                    "mute" | "solo" => {
                        if !v.is_boolean() {
                            return Err(error(&format!("{field} must be boolean")));
                        }
                    }
                    "sends" => {
                        if !v.as_array().is_some_and(|rows| rows.iter().all(|row| finite(row, 0.0, 1.0))) {
                            return Err(error("sends must be 0-1 values"));
                        }
                    }
                    _ => {
                        if !finite(v, if *field == "pan" { -1.0 } else { 0.0 }, 1.0) {
                            return Err(error(&format!("{field} is out of bounds")));
                        }
                    }
                }
                proposed[*field] = v.clone();
            }
            if proposed.as_object().unwrap().is_empty() {
                return Err(error("requires at least one mixer field"));
            }
            Ok(merge(json!({"kind":kind,"trackRef":value["trackRef"]}), proposed))
        }
        "device.parameter.set" => {
            if !only(value, &["kind", "deviceRef", "parameterRef", "value"])
                || !is_non_empty_string(&value["deviceRef"], 256)
                || !is_non_empty_string(&value["parameterRef"], 256)
                || !number(&value["value"]).is_finite()
            {
                return Err(error("requires deviceRef, parameterRef, and a finite value"));
            }
            Ok(json!({"kind":kind,"deviceRef":value["deviceRef"],"parameterRef":value["parameterRef"],"value":value["value"]}))
        }
        "clip.set" => {
            let fields = &["muted", "colorIndex", "looping", "loopStart", "loopEnd"];
            if !only(value, &["kind", "clipRef", "muted", "colorIndex", "looping", "loopStart", "loopEnd"])
                || !is_non_empty_string(&value["clipRef"], 256)
            {
                return Err(error("requires an exact clipRef"));
            }
            for field in ["muted", "looping"] {
                if value.get(field).is_some_and(|value| !value.is_boolean()) {
                    return Err(error(&format!("{field} must be boolean")));
                }
            }
            if value.get("colorIndex").is_some_and(|value| !integer(value, 0.0, 69.0)) {
                return Err(error("colorIndex is out of bounds"));
            }
            for field in ["loopStart", "loopEnd"] {
                if value.get(field).is_some_and(|value| !finite(value, 0.0, 1_000_000_000.0)) {
                    return Err(error(&format!("{field} is out of bounds")));
                }
            }
            let proposed = proposed_fields(value, fields);
            if proposed.as_object().unwrap().is_empty() {
                return Err(error("requires at least one clip field"));
            }
            Ok(merge(json!({"kind":kind,"clipRef":value["clipRef"]}), proposed))
        }
        "track.rename" | "scene.rename" => {
            let reference = if kind == "track.rename" { "trackRef" } else { "sceneRef" };
            if !only(value, &["kind", reference, "name"])
                || !is_non_empty_string(&value[reference], 256)
                || !is_non_empty_string(&value["name"], 256)
            {
                return Err(error(&format!("requires {reference} and a non-empty name")));
            }
            Ok(json!({"kind":kind,reference:value[reference],"name":value["name"]}))
        }
        "track.create" => {
            if !only(value, &["kind", "name", "trackKind", "index"])
                || !is_non_empty_string(&value["name"], 128)
                || !["audio", "midi"].contains(&string(&value["trackKind"]))
                || value.get("index").is_some_and(|value| !integer(value, 0.0, 100_000.0))
            {
                return Err(error("requires a name, trackKind audio|midi, and an optional bounded index"));
            }
            let mut result = json!({"kind":kind,"name":value["name"],"trackKind":value["trackKind"]});
            if let Some(index) = value.get("index") {
                result["index"] = index.clone();
            }
            Ok(result)
        }
        "routing.arm" => {
            if !only(value, &["kind", "trackRef", "armed"]) || !is_non_empty_string(&value["trackRef"], 256) || !value["armed"].is_boolean()
            {
                return Err(error("requires trackRef and a boolean armed"));
            }
            Ok(json!({"kind":kind,"trackRef":value["trackRef"],"armed":value["armed"]}))
        }
        _ => unreachable!(),
    }
}
pub(super) fn batch_target_key(operation: &Value) -> Option<String> {
    let (prefix, field) = match string(&operation["kind"]) {
        "mixer.set" => ("mixer", "trackRef"),
        "device.parameter.set" => ("parameter", "parameterRef"),
        "clip.set" => ("clip", "clipRef"),
        "track.rename" => ("rename", "trackRef"),
        "scene.rename" => ("rename", "sceneRef"),
        "routing.arm" => ("routing", "trackRef"),
        _ => return None,
    };
    Some(format!("{prefix}:{}", string(&operation[field])))
}
pub(super) fn plan_operation(snapshot: &Value, operation: &Value, index: usize) -> Result<Value, LiveError> {
    let error = |text: &str| fail(format!("transaction batch operation {index} {text}"));
    let kind = string(&operation["kind"]);
    let plan = |summary: String, target: Value, prior: Value, proposed: Value| json!({"index":index,"kind":kind,"summary":summary,"target":target,"prior":prior,"proposed":proposed});
    match kind {
        "mixer.set" => {
            let reference = string(&operation["trackRef"]);
            let target = mixer_target(snapshot, reference)?;
            let proposed = proposed_fields(operation, MIXER_STATE_FIELDS);
            if proposed["sends"].is_array() && array(&proposed["sends"]).len() > array(&target.mixer["sends"]).len() {
                return Err(error("proposes more sends than the track exposes"));
            }
            for (field, ref_field, message) in
                [("cueVolume", "cueRef", "cue volume"), ("volume", "volumeRef", "volume"), ("pan", "panRef", "pan")]
            {
                if proposed.get(field).is_some() && target.mixer.get(ref_field) == Some(&Value::Null) {
                    return Err(error(&format!("{message} is unavailable on this track")));
                }
            }
            let fields: Vec<_> = proposed.as_object().unwrap().keys().map(String::as_str).collect();
            let prior = state_fields(target.mixer, &fields);
            let prior = merge(
                prior,
                json!({"authorityDigest":mixer_identity_digest(&target)?,"stateRevision":fingerprint(&state_fields(target.mixer,MIXER_STATE_FIELDS))?}),
            );
            Ok(plan(
                format!("set mixer {} on {reference}", fields.join(", ")),
                json!({"trackRef":reference,"trackIdentity":target.track["objectIdentity"],"name":target.track["name"]}),
                prior,
                proposed,
            ))
        }
        "device.parameter.set" => {
            let device = string(&operation["deviceRef"]);
            let parameter = string(&operation["parameterRef"]);
            let target = parameter_target(snapshot, device, parameter)?;
            if target.device["enabled"] == false || target.parameter["enabled"] == false || target.parameter["automatable"] != true {
                return Err(error("parameter is disabled or not supported for guarded adjustment"));
            }
            let quantization = target.parameter["quantization"].as_f64().unwrap_or(0.0);
            let value = number(&operation["value"]);
            if !target.parameter["min"].is_number()
                || !target.parameter["max"].is_number()
                || value < number(&target.parameter["min"])
                || value > number(&target.parameter["max"])
            {
                return Err(error("value is outside authoritative bounds"));
            }
            let unit = (value - number(&target.parameter["min"])) / quantization;
            if quantization > 0.0 && (unit - (unit + 0.5).floor()).abs() > 1e-9 {
                return Err(error("value does not match authoritative quantization"));
            }
            if !number(&target.parameter["value"]).is_finite() {
                return Err(error("parameter value is unavailable"));
            }
            Ok(plan(
                format!("set {parameter} to {}", kumi_common::js::json::stringify(&operation["value"])),
                json!({"deviceRef":device,"parameterRef":parameter,"name":target.parameter["name"],"trackRef":target.track["ref"]}),
                json!({"value":target.parameter["value"],"revision":parameter_revision(target.parameter),"authorityDigest":fingerprint(&parameter_authority(snapshot,parameter)?)?}),
                json!({"value":operation["value"]}),
            ))
        }
        "clip.set" => {
            let reference = string(&operation["clipRef"]);
            let located = clip_row(snapshot, reference)?;
            let fields: Vec<_> = ["muted", "colorIndex", "looping", "loopStart", "loopEnd"]
                .into_iter()
                .filter(|field| operation.get(*field).is_some())
                .collect();
            if fields.iter().any(|field| located.clip[*field].is_null()) {
                return Err(error("field is unavailable on this exact clip"));
            }
            if located.clip["isAudio"] == true && fields.iter().any(|field| ["looping", "loopStart", "loopEnd"].contains(field)) {
                return Err(error("audio clip loop editing uses live_audio_clip_preview"));
            }
            if number(&operation["loopEnd"]) < number(&operation["loopStart"]) {
                return Err(error("loopEnd precedes loopStart"));
            }
            Ok(plan(
                format!("set clip {} on {reference}", fields.join(", ")),
                json!({"clipRef":reference,"clipIdentity":located.clip["objectIdentity"],"name":located.clip["name"],"arrangement":located.arrangement}),
                merge(
                    state_fields(located.clip, &fields),
                    json!({"authorityDigest":fingerprint(&clip_authority(snapshot,reference)?)?,"stateRevision":fingerprint(&state_fields(located.clip,CLIP_STATE_FIELDS))?}),
                ),
                proposed_fields(operation, &fields),
            ))
        }
        "track.rename" | "scene.rename" => {
            let (noun, collection, ref_field, identity_field) = if kind == "track.rename" {
                ("track", "tracks", "trackRef", "trackIdentity")
            } else {
                ("scene", "scenes", "sceneRef", "sceneIdentity")
            };
            let reference = string(&operation[ref_field]);
            let row = array(&snapshot[collection])
                .iter()
                .find(|row| row["ref"] == reference && is_non_empty_string(&row["objectIdentity"], 256) && row["name"].is_string())
                .ok_or_else(|| error(&format!("{noun} rename target lacks exact authoritative identity")))?;
            if row["name"] == operation["name"] {
                return Err(error("rename would not change the target"));
            }
            let mut target = json!({ref_field:reference,identity_field:row["objectIdentity"]});
            if noun == "scene" {
                target["index"] = row["index"].clone();
            }
            Ok(plan(
                format!("rename {noun} {reference} to {}", string(&operation["name"])),
                target,
                json!({"name":row["name"]}),
                json!({"name":operation["name"]}),
            ))
        }
        "track.create" => {
            if array(&snapshot["tracks"]).iter().chain(array(&snapshot["scenes"])).any(|row| row["name"] == operation["name"]) {
                return Err(error("track name already exists in the Set"));
            }
            let regular =
                array(&snapshot["tracks"]).iter().filter(|track| !["return", "main", "master"].contains(&string(&track["kind"]))).count();
            if number(&operation["index"]) > regular as f64 {
                return Err(error("track index exceeds the current regular-track collection"));
            }
            let mut target = json!({"name":operation["name"],"trackKind":operation["trackKind"]});
            if let Some(index) = operation.get("index") {
                target["index"] = index.clone();
            }
            Ok(plan(
                format!("create {} track {}", string(&operation["trackKind"]), string(&operation["name"])),
                target,
                json!({"existed":false,"structureRevision":structure_revision(snapshot)}),
                json!({"name":operation["name"],"trackKind":operation["trackKind"]}),
            ))
        }
        "routing.arm" => {
            let reference = string(&operation["trackRef"]);
            let track = routing_target(snapshot, reference)?;
            if !track["armed"].is_boolean() {
                return Err(error("arm is unavailable on this exact track"));
            }
            Ok(plan(
                format!("{} track {reference}", if operation["armed"] == true { "arm" } else { "disarm" }),
                json!({"trackRef":reference,"trackIdentity":track["objectIdentity"],"name":track["name"]}),
                json!({"armed":track["armed"],"stateRevision":routing_state_revision(track)?}),
                json!({"armed":operation["armed"]}),
            ))
        }
        _ => unreachable!(),
    }
}
