use super::{manager::Record, *};
use crate::host::helpers::{named_mixer_part, same_mixer_value};
/// Whether `row` holds a step's value for `field`: within float32 precision (Live keeps many values as 32-bit
/// floats, so a written 0.6 reads back 0.6000000238418579), and a `sends` list over just the sends it names.
fn holds(row: &Value, field: &str, expected: &Value) -> bool {
    same_mixer_value(field, row.get(field), Some(expected))
}
/// A mixer field's prior value, as much of it as the step changed (a shorter `sends` list set only its first sends).
fn prior_part(plan: &Value, field: &str) -> Value {
    named_mixer_part(field, &plan["prior"][field], &plan["proposed"][field])
}
fn rename_parts<'a>(snapshot: &'a Value, operation: &Value, plan: &Value) -> (&'a [Value], Value, Value) {
    if operation["kind"] == "track.rename" {
        (array(&snapshot["tracks"]), operation["trackRef"].clone(), plan["target"]["trackIdentity"].clone())
    } else {
        (array(&snapshot["scenes"]), operation["sceneRef"].clone(), plan["target"]["sceneIdentity"].clone())
    }
}
fn parameter_args(reference: &Value, value: &Value, parameter: &Value, authority: &Value) -> Value {
    json!({"ref":reference,"value":value,"expectedRevision":parameter_revision(parameter),"expectedObjectIdentity":authority["parameterIdentity"],"expectedOwnerRef":authority["ownerRef"],"expectedOwnerIdentity":authority["ownerIdentity"],"expectedTrackRef":authority["trackRef"],"expectedTrackIdentity":authority["trackIdentity"],"expectedSiblings":authority["siblings"]})
}
impl BatchTransactionManager {
    /// The Set's structure as the preview saw it, from `snapshot`: the tracks this batch made taken out and its renames
    /// put back, matched by identity (a track made above another moves that one's ref).
    fn preview_structure_identity(&self, snapshot: &Value, record: &Record) -> Result<String, LiveError> {
        let mut baseline = snapshot.clone();
        let created = record.get("created");
        baseline["tracks"]
            .as_array_mut()
            .unwrap()
            .retain(|track| !array(&created).iter().any(|owned| owned["objectIdentity"] == track["objectIdentity"]));
        for (index, operation) in array(&record.get("operations")).iter().enumerate() {
            if record.step(false, index)["completed"] != true || !["track.rename", "scene.rename"].contains(&string(&operation["kind"])) {
                continue;
            }
            let plan = record.get("plans")[index].clone();
            let (_, _, identity) = rename_parts(&baseline, operation, &plan);
            let collection = if operation["kind"] == "track.rename" { "tracks" } else { "scenes" };
            let row = baseline[collection]
                .as_array_mut()
                .unwrap()
                .iter_mut()
                .find(|row| row["objectIdentity"] == identity)
                .filter(|row| row["name"] == operation["name"])
                .ok_or_else(|| fail("transaction batch structure changed after an owned rename"))?;
            row["name"] = plan["prior"]["name"].clone();
        }
        Ok(structure_identity(&baseline))
    }
    pub(super) fn step_args(&self, snapshot: &Value, record: &Record, index: usize) -> Result<Value, LiveError> {
        let operation = record.get("operations")[index].clone();
        let plan = record.get("plans")[index].clone();
        let kind = string(&operation["kind"]);
        let error = |message: &str| fail(format!("transaction batch step {index} {message}"));
        let invoke = |op: &str, args: Value| json!({"operation":op,"args":args});
        match kind {
            "mixer.set" => {
                let target = mixer_target(snapshot, string(&operation["trackRef"]))?;
                if target.track["objectIdentity"] != plan["target"]["trackIdentity"]
                    || mixer_identity_digest(&target)? != string(&plan["prior"]["authorityDigest"])
                    || fingerprint(&state_fields(target.mixer, MIXER_STATE_FIELDS))? != string(&plan["prior"]["stateRevision"])
                {
                    return Err(error("mixer target or state changed since preview"));
                }
                Ok(invoke(kind, merge(merge(json!({"ref":operation["trackRef"]}), plan["proposed"].clone()), mixer_authority(&target)?)))
            }
            "device.parameter.set" => {
                let target = parameter_target(snapshot, string(&operation["deviceRef"]), string(&operation["parameterRef"]))?;
                let authority = parameter_authority(snapshot, string(&operation["parameterRef"]))?;
                if !same_parameter_value(&target.parameter["value"], &plan["prior"]["value"])
                    || parameter_revision(target.parameter) != number(&plan["prior"]["revision"])
                    || fingerprint(&authority)? != string(&plan["prior"]["authorityDigest"])
                {
                    return Err(error("parameter identity, value, or revision changed since preview"));
                }
                Ok(invoke(kind, parameter_args(&operation["parameterRef"], &operation["value"], target.parameter, &authority)))
            }
            "clip.set" => {
                let reference = string(&operation["clipRef"]);
                let located = clip_row(snapshot, reference)?;
                if located.clip["objectIdentity"] != plan["target"]["clipIdentity"]
                    || fingerprint(&clip_authority(snapshot, reference)?)? != string(&plan["prior"]["authorityDigest"])
                    || fingerprint(&state_fields(located.clip, CLIP_STATE_FIELDS))? != string(&plan["prior"]["stateRevision"])
                {
                    return Err(error("clip identity or state changed since preview"));
                }
                Ok(invoke(
                    kind,
                    merge(merge(json!({"ref":reference}), plan["proposed"].clone()), clip_properties_authority(snapshot, reference)?),
                ))
            }
            "track.rename" | "scene.rename" => {
                let (rows, reference, identity) = rename_parts(snapshot, &operation, &plan);
                let row = rows
                    .iter()
                    .find(|row| row["ref"] == reference && row["objectIdentity"] == identity && row["name"] == plan["prior"]["name"])
                    .ok_or_else(|| {
                        error(if kind == "track.rename" {
                            "track identity or name changed since preview"
                        } else {
                            "scene identity or name changed since preview"
                        })
                    })?;
                Ok(invoke(
                    kind,
                    json!({"ref":reference,"name":operation["name"],"expectedName":plan["prior"]["name"],"expectedObjectIdentity":identity,"expectedAuthorityRevision":rename_revision(row)?}),
                ))
            }
            "track.create" => {
                if array(&snapshot["tracks"]).iter().chain(array(&snapshot["scenes"])).any(|row| row["name"] == operation["name"]) {
                    return Err(error("track name already exists"));
                }
                if self.preview_structure_identity(snapshot, record)? != string(&record.get("structureIdentity")) {
                    return Err(error("structure changed since preview"));
                }
                let mut args = json!({"name":operation["name"],"kind":operation["trackKind"]});
                if let Some(index) = operation.get("index") {
                    args["index"] = index.clone();
                }
                args["expectedStructureRevision"] = structure_revision(snapshot).into();
                Ok(invoke(kind, args))
            }
            "routing.arm" => {
                let track = routing_target(snapshot, string(&operation["trackRef"]))?;
                if track["objectIdentity"] != plan["target"]["trackIdentity"]
                    || routing_state_revision(track)? != string(&plan["prior"]["stateRevision"])
                    || track["armed"] != plan["prior"]["armed"]
                {
                    return Err(error("routing target or state changed since preview"));
                }
                Ok(invoke(
                    "routing.set",
                    json!({"ref":operation["trackRef"],"arm":operation["armed"],"expectedObjectIdentity":track["objectIdentity"],"expectedStateRevision":routing_state_revision(track)?}),
                ))
            }
            _ => unreachable!(),
        }
    }
    fn step_postcondition(&self, snapshot: &Value, record: &Record, index: usize) -> bool {
        let operation = record.get("operations")[index].clone();
        let plan = record.get("plans")[index].clone();
        (|| -> Result<bool, LiveError> {
            Ok(match string(&operation["kind"]) {
                "mixer.set" => {
                    let target = mixer_target(snapshot, string(&operation["trackRef"]))?;
                    target.track["objectIdentity"] == plan["target"]["trackIdentity"]
                        && mixer_identity_digest(&target)? == string(&plan["prior"]["authorityDigest"])
                        && plan["proposed"].as_object().unwrap().iter().all(|(key, value)| holds(target.mixer, key, value))
                }
                "device.parameter.set" => {
                    let target = parameter_target(snapshot, string(&operation["deviceRef"]), string(&operation["parameterRef"]))?;
                    parameter_holds(target.parameter, &plan["proposed"]["value"])
                        && parameter_revision(target.parameter) > number(&plan["prior"]["revision"])
                        && fingerprint(&parameter_authority(snapshot, string(&operation["parameterRef"]))?)?
                            == string(&plan["prior"]["authorityDigest"])
                }
                "clip.set" => {
                    let reference = string(&operation["clipRef"]);
                    let located = clip_row(snapshot, reference)?;
                    located.clip["objectIdentity"] == plan["target"]["clipIdentity"]
                        && fingerprint(&clip_authority(snapshot, reference)?)? == string(&plan["prior"]["authorityDigest"])
                        && plan["proposed"].as_object().unwrap().iter().all(|(key, value)| holds(located.clip, key, value))
                }
                "track.rename" | "scene.rename" => {
                    let (rows, reference, identity) = rename_parts(snapshot, &operation, &plan);
                    rows.iter().any(|row| row["ref"] == reference && row["objectIdentity"] == identity && row["name"] == operation["name"])
                }
                "routing.arm" => {
                    let track = routing_target(snapshot, string(&operation["trackRef"]))?;
                    track["objectIdentity"] == plan["target"]["trackIdentity"] && track["armed"] == operation["armed"]
                }
                _ => false,
            })
        })()
        .unwrap_or(false)
    }
    pub(super) async fn verify_step(
        &self,
        context: Option<&LiveOperationContext>,
        record: &Record,
        index: usize,
        result: &Value,
    ) -> Result<Value, LiveError> {
        let operation = record.get("operations")[index].clone();
        let plan = record.get("plans")[index].clone();
        let kind = string(&operation["kind"]);
        let snapshot = self.record_view(context, record, &[result["ref"].clone()]).await?;
        let error = |message: &str| fail(format!("transaction batch step {index} {message}"));
        if kind != "track.create" && !self.step_postcondition(&snapshot, record, index) {
            return Err(error("identity or postcondition was not confirmed"));
        }
        match kind {
            "mixer.set" => {
                if !result.is_object() || result["changed"] != true {
                    return Err(error("mixer change was not confirmed"));
                }
                let target = mixer_target(&snapshot, string(&operation["trackRef"]))?;
                if plan["proposed"].as_object().unwrap().iter().any(|(key, value)| !holds(target.mixer, key, value)) {
                    return Err(error("mixer postcondition was not confirmed"));
                }
                Ok(json!({"index":index,"kind":kind,"trackRef":operation["trackRef"],"applied":plan["proposed"]}))
            }
            "device.parameter.set" => {
                let target = parameter_target(&snapshot, string(&operation["deviceRef"]), string(&operation["parameterRef"]))?;
                if !parameter_holds(target.parameter, &operation["value"])
                    || parameter_revision(target.parameter) <= number(&plan["prior"]["revision"])
                {
                    return Err(error("parameter postcondition was not confirmed"));
                }
                // The whole number Live kept is the change made: undo checks for it.
                if !same_parameter_value(&target.parameter["value"], &operation["value"]) {
                    let mut plans = record.get("plans");
                    plans[index]["proposed"]["value"] = target.parameter["value"].clone();
                    record.put("plans", plans);
                }
                Ok(
                    json!({"index":index,"kind":kind,"parameterRef":operation["parameterRef"],"value":target.parameter["value"],"revision":parameter_revision(target.parameter)}),
                )
            }
            "clip.set" => {
                if !result.is_object() || result["changed"] != true {
                    return Err(error("clip change was not confirmed"));
                }
                let located = clip_row(&snapshot, string(&operation["clipRef"]))?;
                if plan["proposed"].as_object().unwrap().iter().any(|(key, value)| !holds(located.clip, key, value)) {
                    return Err(error("clip postcondition was not confirmed"));
                }
                Ok(json!({"index":index,"kind":kind,"clipRef":operation["clipRef"],"applied":plan["proposed"]}))
            }
            "track.rename" | "scene.rename" => {
                let (rows, reference, _) = rename_parts(&snapshot, &operation, &plan);
                if !rows.iter().any(|row| row["ref"] == reference && row["name"] == operation["name"]) {
                    return Err(error("rename postcondition was not confirmed"));
                }
                Ok(json!({"index":index,"kind":kind,"ref":reference,"name":operation["name"]}))
            }
            "track.create" => {
                if !result.is_object()
                    || !is_non_empty_string(&result["ref"], 256)
                    || !is_non_empty_string(&result["objectIdentity"], 256)
                    || !is_non_empty_string(&result["createdFingerprint"], 64)
                {
                    return Err(error("track creation did not return exact identity and fingerprint"));
                }
                if !array(&snapshot["tracks"]).iter().any(|track| {
                    track["ref"] == result["ref"]
                        && track["objectIdentity"] == result["objectIdentity"]
                        && track["name"] == operation["name"]
                }) {
                    return Err(error("track creation postcondition was not confirmed"));
                }
                let fingerprint = track_created_fingerprint(&snapshot, string(&result["ref"]))?;
                if fingerprint != string(&result["createdFingerprint"]) {
                    return Err(error("created track changed after atomic creation"));
                }
                let mut created = array(&record.get("created")).to_vec();
                created.push(
                    json!({"stepIndex":index,"ref":result["ref"],"objectIdentity":result["objectIdentity"],"fingerprint":fingerprint}),
                );
                record.put("created", json!(created));
                let mut output = json!({"index":index,"kind":kind,"ref":result["ref"],"name":operation["name"]});
                if let Some(track_index) = result.get("index") {
                    output["trackIndex"] = track_index.clone();
                }
                Ok(output)
            }
            "routing.arm" => {
                if !result.is_object() || result["changed"] != true {
                    return Err(error("routing change was not confirmed"));
                }
                if routing_target(&snapshot, string(&operation["trackRef"]))?["armed"] != operation["armed"] {
                    return Err(error("arm postcondition was not confirmed"));
                }
                Ok(json!({"index":index,"kind":kind,"trackRef":operation["trackRef"],"armed":operation["armed"]}))
            }
            _ => unreachable!(),
        }
    }
}

impl BatchTransactionManager {
    pub(super) async fn revert(&self, context: Option<&LiveOperationContext>, record: &Record, mode: &str) -> Result<usize, LiveError> {
        let operations = record.get("operations");
        if record.get("undoSteps").is_null() {
            record.put("undoSteps", json!(array(&operations).iter().map(|_| json!({"completed":false})).collect::<Vec<_>>()));
        }
        let mut reverted = 0;
        for index in (0..array(&operations).len()).rev() {
            if mode == "rollback" && record.step(false, index)["completed"] != true {
                continue;
            }
            if record.step(true, index)["completed"] == true {
                continue;
            }
            self.policy(record)?;
            if record.step(true, index).get("invocation").is_some() {
                self.checkpoint(record, true, index, context).await?;
                self.verify_restoration(&self.record_view(context, record, &[]).await?, record, index)?;
                record.set_step(true, index, "completed", true);
                reverted += 1;
                continue;
            }
            let snapshot = self.record_view(context, record, &[]).await?;
            self.policy(record)?;
            let operation = &operations[index];
            let plan = record.get("plans")[index].clone();
            let kind = string(&operation["kind"]);
            let error = |message: &str| fail(format!("transaction batch {mode} step {index} {message}"));
            let invoke = |op: &str, args: Value| json!({"operation":op,"args":args});
            let invocation = match kind {
                "mixer.set" => {
                    let target = mixer_target(&snapshot, string(&operation["trackRef"]))?;
                    if target.track["objectIdentity"] != plan["target"]["trackIdentity"]
                        || mixer_identity_digest(&target)? != string(&plan["prior"]["authorityDigest"])
                    {
                        return Err(error("mixer target identity changed"));
                    }
                    if plan["proposed"].as_object().unwrap().iter().any(|(key, value)| !holds(target.mixer, key, value)) {
                        return Err(error("mixer state changed after apply"));
                    }
                    let prior = Value::Object(
                        plan["proposed"].as_object().unwrap().keys().map(|field| (field.clone(), prior_part(&plan, field))).collect(),
                    );
                    invoke("mixer.set", merge(merge(json!({"ref":operation["trackRef"]}), prior), mixer_authority(&target)?))
                }
                "device.parameter.set" => {
                    let target = parameter_target(&snapshot, string(&operation["deviceRef"]), string(&operation["parameterRef"]))?;
                    let authority = parameter_authority(&snapshot, string(&operation["parameterRef"]))?;
                    if !parameter_holds(target.parameter, &plan["proposed"]["value"])
                        || fingerprint(&authority)? != string(&plan["prior"]["authorityDigest"])
                    {
                        return Err(error("parameter value or identity changed after apply"));
                    }
                    invoke(kind, parameter_args(&operation["parameterRef"], &plan["prior"]["value"], target.parameter, &authority))
                }
                "clip.set" => {
                    let reference = string(&operation["clipRef"]);
                    let located = clip_row(&snapshot, reference)?;
                    if located.clip["objectIdentity"] != plan["target"]["clipIdentity"]
                        || fingerprint(&clip_authority(&snapshot, reference)?)? != string(&plan["prior"]["authorityDigest"])
                    {
                        return Err(error("clip identity changed after apply"));
                    }
                    if plan["proposed"].as_object().unwrap().iter().any(|(key, value)| !holds(located.clip, key, value)) {
                        return Err(error("clip state changed after apply"));
                    }
                    let fields: Vec<_> = plan["proposed"].as_object().unwrap().keys().map(String::as_str).collect();
                    invoke(
                        kind,
                        merge(
                            merge(json!({"ref":reference}), state_fields(&plan["prior"], &fields)),
                            clip_properties_authority(&snapshot, reference)?,
                        ),
                    )
                }
                "track.rename" | "scene.rename" => {
                    let (rows, reference, identity) = rename_parts(&snapshot, operation, &plan);
                    let row = rows
                        .iter()
                        .find(|row| row["ref"] == reference && row["objectIdentity"] == identity && row["name"] == operation["name"])
                        .ok_or_else(|| error("rename target identity or name changed after apply"))?;
                    invoke(
                        kind,
                        json!({"ref":reference,"name":plan["prior"]["name"],"expectedName":operation["name"],"expectedObjectIdentity":identity,"expectedAuthorityRevision":rename_revision(row)?}),
                    )
                }
                "track.create" => {
                    let created = record.get("created");
                    let owned = array(&created)
                        .iter()
                        .find(|owned| number(&owned["stepIndex"]) == index as f64)
                        .ok_or_else(|| error("created-track record is unavailable"))?;
                    if !array(&snapshot["tracks"])
                        .iter()
                        .any(|track| track["ref"] == owned["ref"] && track["objectIdentity"] == owned["objectIdentity"])
                        || track_created_fingerprint(&snapshot, string(&owned["ref"]))? != string(&owned["fingerprint"])
                    {
                        return Err(error("created track changed after creation; deletion refused"));
                    }
                    invoke(
                        "track.delete",
                        json!({"ref":owned["ref"],"expectedStructureRevision":structure_revision(&snapshot),"expectedObjectIdentity":owned["objectIdentity"]}),
                    )
                }
                "routing.arm" => {
                    let track = routing_target(&snapshot, string(&operation["trackRef"]))?;
                    if track["objectIdentity"] != plan["target"]["trackIdentity"] {
                        return Err(error("routing target identity changed"));
                    }
                    if track["armed"] != operation["armed"] {
                        return Err(error("arm state changed after apply"));
                    }
                    invoke(
                        "routing.set",
                        json!({"ref":operation["trackRef"],"arm":plan["prior"]["armed"],"expectedObjectIdentity":track["objectIdentity"],"expectedStateRevision":routing_state_revision(track)?}),
                    )
                }
                _ => unreachable!(),
            };
            record.set_step(true, index, "invocation", invocation);
            self.checkpoint(record, true, index, context).await?;
            let snapshot = self.record_view(context, record, &[]).await?;
            match kind {
                "mixer.set" => {
                    let target = mixer_target(&snapshot, string(&operation["trackRef"]))?;
                    if plan["proposed"].as_object().unwrap().keys().any(|field| !holds(target.mixer, field, &prior_part(&plan, field))) {
                        return Err(error("mixer prior-state restoration was not confirmed"));
                    }
                }
                "device.parameter.set" => {
                    let target = parameter_target(&snapshot, string(&operation["deviceRef"]), string(&operation["parameterRef"]))?;
                    if !same_parameter_value(&target.parameter["value"], &plan["prior"]["value"]) {
                        return Err(error("parameter prior-value restoration was not confirmed"));
                    }
                }
                "clip.set" => {
                    let located = clip_row(&snapshot, string(&operation["clipRef"]))?;
                    if plan["proposed"].as_object().unwrap().keys().any(|field| !holds(located.clip, field, &plan["prior"][field])) {
                        return Err(error("clip prior-state restoration was not confirmed"));
                    }
                }
                "track.rename" | "scene.rename" => {
                    let (rows, reference, _) = rename_parts(&snapshot, operation, &plan);
                    if !rows.iter().any(|row| row["ref"] == reference && row["name"] == plan["prior"]["name"]) {
                        return Err(error("rename prior-name restoration was not confirmed"));
                    }
                }
                "track.create" => {
                    let created = record.get("created");
                    let owned = array(&created).iter().find(|owned| number(&owned["stepIndex"]) == index as f64).unwrap();
                    // By identity: Live gives refs by place, so the next track takes the deleted one's ref.
                    if array(&snapshot["tracks"]).iter().any(|track| track["objectIdentity"] == owned["objectIdentity"]) {
                        return Err(error("created-track deletion was not confirmed"));
                    }
                }
                "routing.arm" => {
                    if routing_target(&snapshot, string(&operation["trackRef"]))?["armed"] != plan["prior"]["armed"] {
                        return Err(error("arm prior-state restoration was not confirmed"));
                    }
                }
                _ => unreachable!(),
            }
            self.verify_restoration(&self.record_view(context, record, &[]).await?, record, index)?;
            record.set_step(true, index, "completed", true);
            reverted += 1;
        }
        Ok(reverted)
    }
    fn verify_restoration(&self, snapshot: &Value, record: &Record, index: usize) -> Result<(), LiveError> {
        let operation = record.get("operations")[index].clone();
        let plan = record.get("plans")[index].clone();
        let (row, identity, expected) = match string(&operation["kind"]) {
            "mixer.set" => {
                let target = mixer_target(snapshot, string(&operation["trackRef"]))?;
                (Some(target.mixer), json!(mixer_identity_digest(&target)?), plan["prior"]["authorityDigest"].clone())
            }
            "clip.set" => (
                Some(clip_row(snapshot, string(&operation["clipRef"]))?.clip),
                json!(fingerprint(&clip_authority(snapshot, string(&operation["clipRef"]))?)?),
                plan["prior"]["authorityDigest"].clone(),
            ),
            "device.parameter.set" => {
                let target = parameter_target(snapshot, string(&operation["deviceRef"]), string(&operation["parameterRef"]))?;
                if !same_parameter_value(&target.parameter["value"], &plan["prior"]["value"])
                    || fingerprint(&parameter_authority(snapshot, string(&operation["parameterRef"]))?)?
                        != string(&plan["prior"]["authorityDigest"])
                {
                    return Err(fail("transaction batch parameter restoration identity or value changed"));
                }
                return Ok(());
            }
            "track.rename" | "scene.rename" => {
                let (rows, reference, identity) = rename_parts(snapshot, &operation, &plan);
                let row = rows.iter().find(|row| row["ref"] == reference);
                (row, row.map(|row| row["objectIdentity"].clone()).unwrap_or(Value::Null), identity)
            }
            "routing.arm" => {
                let track = routing_target(snapshot, string(&operation["trackRef"]))?;
                (Some(track), track["objectIdentity"].clone(), plan["target"]["trackIdentity"].clone())
            }
            "track.create" => {
                let created = record.get("created");
                let owned = array(&created).iter().find(|owned| number(&owned["stepIndex"]) == index as f64);
                if owned
                    .is_none_or(|owned| array(&snapshot["tracks"]).iter().any(|track| track["objectIdentity"] == owned["objectIdentity"]))
                {
                    return Err(fail("transaction batch created-track deletion was not confirmed"));
                }
                return Ok(());
            }
            _ => unreachable!(),
        };
        if row.is_none() || identity != expected {
            return Err(fail("transaction batch prior-state restoration identity or value changed"));
        }
        for field in plan["proposed"].as_object().unwrap().keys() {
            if !holds(row.unwrap(), field, &prior_part(&plan, field)) {
                return Err(fail("transaction batch prior-state restoration identity or value changed"));
            }
        }
        Ok(())
    }
}
