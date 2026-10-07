//! Compound mutation batches and the exact hierarchy authority shared with device-state recall.
use crate::{
    live::*,
    registry::{canonical_json, UNBOUNDED_CANONICAL_LIMITS},
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

pub const BATCH_TRANSACTION_TTL_MS: f64 = 30_000.0;
pub const MAX_BATCH_OPERATIONS: usize = 32;
const MAX_SET_ROWS: usize = 10_000_000;
pub const BATCH_OPERATION_KINDS: &[&str] =
    &["mixer.set", "device.parameter.set", "clip.set", "track.rename", "scene.rename", "track.create", "routing.arm"];
pub const BATCH_OPERATION_POLICY_TOOLS: &[(&str, &str)] = &[
    ("mixer.set", "live_mixer_apply"),
    ("device.parameter.set", "live_device_parameter_apply"),
    ("clip.set", "live_clip_properties_apply"),
    ("track.rename", "live_object_rename_apply"),
    ("scene.rename", "live_object_rename_apply"),
    ("track.create", "live_session_structure_apply"),
    ("routing.arm", "live_routing_apply"),
];
pub fn operation_requirements(kind: &str) -> Option<(&'static [&'static str], &'static [&'static str])> {
    match kind {
        "mixer.set" => Some((&["mixing"], &["snapshot", "mixer.set"])),
        "device.parameter.set" => Some((&["devices", "parameters", "device.parameter.write"], &["snapshot", "device.parameter.set"])),
        "clip.set" => Some((&["clips"], &["snapshot", "clip.set"])),
        "track.rename" => Some((&["tracks"], &["snapshot", "track.rename"])),
        "scene.rename" => Some((&["scenes"], &["snapshot", "scene.rename"])),
        "track.create" => Some((&["session.structure"], &["snapshot", "track.create", "track.delete"])),
        "routing.arm" => Some((&["routing"], &["snapshot", "routing.set"])),
        _ => None,
    }
}
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MutationCheckpoint {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub invocation: Option<LiveInvocation>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub acknowledged: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wire_result: Option<Value>,
}
/// The adapter's execution ledger establishes a prior dispatch; a matching value never does.
pub async fn invoke_checkpoint(
    adapter: &dyn AsyncLiveAdapter,
    checkpoint: &mut MutationCheckpoint,
    context: Option<&LiveOperationContext>,
) -> Result<Value, LiveError> {
    if checkpoint.acknowledged == Some(true) {
        return Ok(checkpoint.wire_result.clone().unwrap_or(Value::Null));
    }
    let invocation = checkpoint.invocation.clone().ok_or_else(|| fail("transaction batch dispatch checkpoint is missing"))?;
    let result = adapter.invoke_async(&invocation, context).await?;
    checkpoint.wire_result = Some(result.clone());
    checkpoint.acknowledged = Some(true);
    Ok(result)
}
fn fail(message: impl Into<String>) -> LiveError {
    LiveError::error(message)
}
fn array(value: &Value) -> &[Value] {
    value.as_array().map(Vec::as_slice).unwrap_or(&[])
}
fn string(value: &Value) -> &str {
    value.as_str().unwrap_or("")
}
fn number(value: &Value) -> f64 {
    value.as_f64().unwrap_or(f64::NAN)
}
pub fn is_object(value: &Value) -> bool {
    value.is_object()
}
pub fn is_non_empty_string(value: &Value, max_length: usize) -> bool {
    value.as_str().is_some_and(|text| (1..=max_length).contains(&kumi_common::js::string::utf16_len(text)))
}
pub fn same_parameter_value(observed: &Value, expected: &Value) -> bool {
    let (Some(observed), Some(expected)) = (observed.as_f64(), expected.as_f64()) else { return false };
    (observed - expected).abs() <= 1e-6 * 1.0_f64.max(observed.abs()).max(expected.abs())
}
/// Whether a parameter holds the value a step asked for: the same (within float32 precision), or the whole number
/// Live keeps for it on a whole-number range (63.5 asked, 64 kept), as the single-parameter path accepts.
pub fn parameter_holds(parameter: &Value, wanted: &Value) -> bool {
    same_parameter_value(&parameter["value"], wanted)
        || crate::host::helpers::whole_number_live_kept(&parameter["value"], number(wanted), parameter).is_some()
}
/// A refusal that says nothing changed (the Remote Script's "; nothing changed", or a request that never left):
/// the step it stopped has nothing in flight.
pub(crate) fn not_dispatched(cause: &LiveError) -> bool {
    matches!(cause, LiveError::MutationNotDispatched(_)) || crate::host::helpers::nothing_changed(cause)
}
/// Forget the invocation of every step a not-dispatched refusal stopped before Live acknowledged it, so the failure
/// is classed by what reached Live: a clean refusal compensates the steps before it.
pub(crate) fn forget_undispatched(steps: &mut Value) {
    for step in steps.as_array_mut().into_iter().flatten() {
        if step["completed"] != true && step["acknowledged"] != true {
            if let Some(step) = step.as_object_mut() {
                step.shift_remove("invocation");
            }
        }
    }
}
/// What a rollback after a failed apply runs under: the apply's signal and authority, with as long again from now as
/// the apply had (it may have spent its deadline, and the rollback's first read would fail at once).
pub(crate) fn compensation_context(context: &LiveOperationContext, span: f64) -> LiveOperationContext {
    LiveOperationContext {
        deadline_ms: Some((kumi_common::time::now_ms() as f64 + span.clamp(5000.0, 60000.0)).round()),
        ..context.clone()
    }
}
pub fn canonical(value: &Value) -> Result<String, LiveError> {
    canonical_json(value, &UNBOUNDED_CANONICAL_LIMITS).map_err(|e| fail(format!("{e:?}")))
}
pub fn fingerprint(value: &Value) -> Result<String, LiveError> {
    Ok(hex::encode(Sha256::digest(canonical(value)?.as_bytes())))
}
pub fn flatten_device_rows(values: &Value) -> Vec<&Value> {
    let mut rows = vec![];
    let mut stack: Vec<_> = array(values).iter().rev().collect();
    while let Some(value) = stack.pop() {
        if !value.is_object() || rows.len() >= MAX_SET_ROWS {
            continue;
        }
        rows.push(value);
        let children = array(&value["chains"]).iter().filter(|chain| chain.is_object()).flat_map(|chain| array(&chain["devices"])).chain(
            array(&value["drumPads"])
                .iter()
                .filter(|pad| pad.is_object())
                .flat_map(|pad| array(&pad["chains"]))
                .filter(|chain| chain.is_object())
                .flat_map(|chain| array(&chain["devices"])),
        );
        let children: Vec<_> = children.collect();
        stack.extend(children.into_iter().rev());
    }
    rows
}
#[derive(Debug, Clone, Copy)]
pub struct ParameterTarget<'a> {
    pub device: &'a Value,
    pub parameter: &'a Value,
    pub track: &'a Value,
}
pub fn parameter_target<'a>(snapshot: &'a Value, device_ref: &str, parameter_ref: &str) -> Result<ParameterTarget<'a>, LiveError> {
    for track in array(&snapshot["tracks"]) {
        if let Some(device) = flatten_device_rows(&track["devices"]).into_iter().find(|row| row["ref"] == device_ref) {
            if let Some(parameter) = array(&device["parameters"]).iter().find(|row| row.is_object() && row["ref"] == parameter_ref) {
                return Ok(ParameterTarget { device, parameter, track });
            }
        }
    }
    Err(fail("transaction batch device and parameter references are not authoritative children"))
}
pub fn parameter_revision(parameter: &Value) -> f64 {
    parameter["revision"].as_f64().unwrap_or(1.0)
}
#[derive(PartialEq, Eq, Hash)]
enum RowKey<'a> {
    Absent,
    Null,
    Bool(bool),
    Number(u64),
    String(&'a str),
    Object(usize),
}
fn row_key(value: Option<&Value>) -> RowKey<'_> {
    match value {
        None => RowKey::Absent,
        Some(Value::Null) => RowKey::Null,
        Some(Value::Bool(v)) => RowKey::Bool(*v),
        Some(Value::Number(v)) => {
            let n = v.as_f64().unwrap_or(f64::NAN);
            RowKey::Number(if n == 0.0 { 0 } else { n.to_bits() })
        }
        Some(Value::String(v)) => RowKey::String(v),
        Some(v) => RowKey::Object(v as *const Value as usize),
    }
}
/// Preserve the first occurrence of each ref/identity pair, including macro aliases.
pub fn unique_parameter_rows(rows: &[Value]) -> Vec<Value> {
    let mut seen = std::collections::HashSet::new();
    rows.iter().filter(|row| seen.insert((row_key(row.get("ref")), row_key(row.get("objectIdentity"))))).cloned().collect()
}
pub fn parameter_authority(snapshot: &Value, parameter_ref: &str) -> Result<Value, LiveError> {
    fn visit(candidate: &Value, track: &Value, parameter_ref: &str) -> Option<Value> {
        let candidates = candidate.as_array().filter(|rows| rows.len() <= MAX_SET_ROWS && rows.iter().all(Value::is_object))?;
        for device in candidates {
            let rows: Vec<_> =
                array(&device["parameters"]).iter().chain(array(&device["macros"])).filter(|row| row.is_object()).cloned().collect();
            let rows = unique_parameter_rows(&rows);
            if [track.get("ref"), track.get("objectIdentity"), device.get("ref"), device.get("objectIdentity")]
                .iter()
                .all(|v| v.and_then(Value::as_str).is_some_and(|s| !s.is_empty()))
                && rows.iter().all(|row| row["ref"].is_string() && row["objectIdentity"].is_string())
            {
                if let Some(found) = rows.iter().find(|row| row["ref"] == parameter_ref && row["objectIdentity"].is_string()) {
                    let siblings: Vec<_> =
                        rows.iter().map(|row| json!({"ref":row["ref"],"objectIdentity":row["objectIdentity"]})).collect();
                    return Some(
                        json!({"ref":parameter_ref,"parameterIdentity":found["objectIdentity"],"ownerRef":device["ref"],"ownerIdentity":device["objectIdentity"],"trackRef":track["ref"],"trackIdentity":track["objectIdentity"],"siblings":siblings}),
                    );
                }
            }
            for chain in array(&device["chains"]).iter().filter(|chain| chain.is_object()) {
                if let Some(found) = visit(&chain["devices"], track, parameter_ref) {
                    return Some(found);
                }
            }
            for pad in array(&device["drumPads"]).iter().filter(|pad| pad.is_object()) {
                for chain in array(&pad["chains"]).iter().filter(|chain| chain.is_object()) {
                    if let Some(found) = visit(&chain["devices"], track, parameter_ref) {
                        return Some(found);
                    }
                }
            }
        }
        None
    }
    for track in array(&snapshot["tracks"]) {
        if let Some(found) = visit(&track["devices"], track, parameter_ref) {
            return Ok(found);
        }
    }
    Err(fail("transaction batch parameter lacks exact hierarchy authority"))
}

const MIXER_STATE_FIELDS: &[&str] = &["volume", "pan", "mute", "solo", "cueVolume", "sends"];
const CLIP_STATE_FIELDS: &[&str] = &[
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
    "velocityAmount",
];
fn state_fields(row: &Value, fields: &[&str]) -> Value {
    Value::Object(fields.iter().map(|field| ((*field).into(), row[*field].clone())).collect())
}
fn rename_revision(row: &Value) -> Result<String, LiveError> {
    fingerprint(&json!({"ref":row["ref"],"objectIdentity":row["objectIdentity"],"name":row["name"]}))
}
/// The Set's structure as a batch's track creation checks it since its preview: each track's and scene's identity,
/// name, kind and place, but not its ref. Live gives refs by place, so a track the batch made moves the refs of the
/// ones after it, and a second creation would otherwise always see a changed Set.
fn structure_identity(snapshot: &Value) -> String {
    let tracks: Vec<_> = array(&snapshot["tracks"])
        .iter()
        .enumerate()
        .map(|(index, row)| json!([row["objectIdentity"], row["name"], row["kind"], index]))
        .collect();
    let scenes: Vec<_> =
        array(&snapshot["scenes"]).iter().enumerate().map(|(index, row)| json!([row["objectIdentity"], row["name"], index])).collect();
    hex::encode(Sha256::digest(kumi_common::js::json::stringify(&json!({"tracks":tracks,"scenes":scenes}))))
}
fn structure_revision(snapshot: &Value) -> String {
    let tracks: Vec<_> = array(&snapshot["tracks"])
        .iter()
        .enumerate()
        .map(|(index, row)| json!([row["ref"], row["objectIdentity"], row["name"], row["kind"], index]))
        .collect();
    let scenes: Vec<_> = array(&snapshot["scenes"])
        .iter()
        .enumerate()
        .map(|(index, row)| json!([row["ref"], row["objectIdentity"], row["name"], index]))
        .collect();
    hex::encode(Sha256::digest(kumi_common::js::json::stringify(&json!({"tracks":tracks,"scenes":scenes}))))
}
struct MixerTarget<'a> {
    track: &'a Value,
    mixer: &'a Value,
}
fn mixer_target<'a>(snapshot: &'a Value, reference: &str) -> Result<MixerTarget<'a>, LiveError> {
    let track = array(&snapshot["tracks"])
        .iter()
        .find(|track| track["ref"] == reference && track["mixer"].is_object() && is_non_empty_string(&track["objectIdentity"], 256))
        .ok_or_else(|| fail("transaction batch requires a track with an exact authoritative mixer identity"))?;
    let mixer = &track["mixer"];
    let nullable_identity = |value: Option<&Value>| value.is_some_and(|v| v.is_null() || is_non_empty_string(v, 256));
    if !["volumeIdentity", "panIdentity", "cueIdentity"].iter().all(|key| nullable_identity(mixer.get(*key)))
        || !mixer["sendIdentities"].as_array().is_some_and(|rows| rows.iter().all(|row| is_non_empty_string(row, 256)))
        || !mixer["sendRefs"].as_array().is_some_and(|rows| rows.len() == array(&mixer["sendIdentities"]).len())
    {
        return Err(fail("transaction batch mixer parameter identities are incomplete"));
    }
    Ok(MixerTarget { track, mixer })
}
fn mixer_authority(target: &MixerTarget<'_>) -> Result<Value, LiveError> {
    Ok(
        json!({"expectedObjectIdentity":target.track["objectIdentity"],"expectedVolumeIdentity":target.mixer["volumeIdentity"],"expectedPanIdentity":target.mixer["panIdentity"],"expectedCueIdentity":target.mixer["cueIdentity"],"expectedSendIdentities":target.mixer["sendIdentities"],"expectedStateRevision":fingerprint(&state_fields(target.mixer,MIXER_STATE_FIELDS))?}),
    )
}
fn mixer_identity_digest(target: &MixerTarget<'_>) -> Result<String, LiveError> {
    let mut identity = mixer_authority(target)?;
    identity.as_object_mut().unwrap().shift_remove("expectedStateRevision");
    fingerprint(&identity)
}
struct ClipRow<'a> {
    track: Option<&'a Value>,
    clip: &'a Value,
    arrangement: bool,
}
fn clip_row<'a>(snapshot: &'a Value, reference: &str) -> Result<ClipRow<'a>, LiveError> {
    for track in array(&snapshot["tracks"]) {
        if let Some(clip) = array(&track["clips"]).iter().find(|clip| clip.is_object() && clip["ref"] == reference) {
            return Ok(ClipRow { track: Some(track), clip, arrangement: false });
        }
        for lane in array(&track["takeLanes"]).iter().filter(|lane| lane.is_object()) {
            if let Some(clip) = array(&lane["clips"]).iter().find(|clip| clip.is_object() && clip["ref"] == reference) {
                return Ok(ClipRow { track: Some(track), clip, arrangement: true });
            }
        }
    }
    if let Some(clip) = array(&snapshot["arrangement"]["clips"]).iter().find(|clip| clip.is_object() && clip["ref"] == reference) {
        return Ok(ClipRow {
            track: array(&snapshot["tracks"]).iter().find(|track| track["ref"] == clip["trackRef"]),
            clip,
            arrangement: true,
        });
    }
    Err(fail("transaction batch clip reference is not authoritative"))
}
fn clip_authority(snapshot: &Value, reference: &str) -> Result<Value, LiveError> {
    let located = clip_row(snapshot, reference)?;
    if !is_non_empty_string(&located.clip["objectIdentity"], 256) {
        return Err(fail("transaction batch clip lacks exact object identity"));
    }
    if located.arrangement {
        let track = located
            .track
            .filter(|track| is_non_empty_string(&track["ref"], 256) && is_non_empty_string(&track["objectIdentity"], 256))
            .ok_or_else(|| fail("transaction batch Arrangement clip hierarchy authority is incomplete"))?;
        let siblings: Vec<_> = array(&snapshot["arrangement"]["clips"])
            .iter()
            .filter(|clip| clip.is_object() && clip["trackRef"] == track["ref"])
            .map(|clip| json!({"ref":clip["ref"],"objectIdentity":clip["objectIdentity"]}))
            .collect();
        return Ok(
            json!({"expectedObjectIdentity":located.clip["objectIdentity"],"expectedAuthorityRevision":fingerprint(&json!({"clip":{"ref":reference,"objectIdentity":located.clip["objectIdentity"]},"owner":{"ref":track["ref"],"objectIdentity":track["objectIdentity"]},"siblings":siblings}))?}),
        );
    }
    let track = located.track.unwrap();
    if !is_non_empty_string(&track["ref"], 256) || !is_non_empty_string(&track["objectIdentity"], 256) || !track["clipSlots"].is_array() {
        return Err(fail("transaction batch clip track authority is incomplete"));
    }
    let slot = array(&track["clipSlots"]).iter().find(|slot| slot.is_object() && slot["clipRef"] == reference);
    let scene =
        slot.and_then(|slot| array(&snapshot["scenes"]).iter().find(|scene| number(&scene["index"]) == number(&slot["sceneIndex"])));
    let (Some(slot), Some(scene)) = (slot, scene) else {
        return Err(fail("transaction batch clip slot or scene authority is incomplete"));
    };
    if ![&slot["ref"], &slot["objectIdentity"], &scene["ref"], &scene["objectIdentity"]].iter().all(|value| is_non_empty_string(value, 256))
    {
        return Err(fail("transaction batch clip slot or scene authority is incomplete"));
    }
    Ok(
        json!({"expectedObjectIdentity":located.clip["objectIdentity"],"expectedTrackRef":track["ref"],"expectedTrackIdentity":track["objectIdentity"],"expectedSlotRef":slot["ref"],"expectedSlotIdentity":slot["objectIdentity"],"expectedSceneRef":scene["ref"],"expectedSceneIdentity":scene["objectIdentity"]}),
    )
}
fn clip_properties_authority(snapshot: &Value, reference: &str) -> Result<Value, LiveError> {
    let located = clip_row(snapshot, reference)?;
    let authority = clip_authority(snapshot, reference)?;
    let revision = if located.arrangement { authority["expectedAuthorityRevision"].clone() } else { fingerprint(&authority)?.into() };
    Ok(
        json!({"expectedObjectIdentity":authority["expectedObjectIdentity"],"expectedAuthorityRevision":revision,"expectedStateRevision":fingerprint(&state_fields(located.clip,CLIP_STATE_FIELDS))?}),
    )
}
fn track_created_fingerprint(snapshot: &Value, reference: &str) -> Result<String, LiveError> {
    let track = array(&snapshot["tracks"])
        .iter()
        .find(|track| track["ref"] == reference)
        .ok_or_else(|| fail("transaction batch created-track fingerprint is unavailable"))?;
    let track: Track = serde_json::from_value(track.clone())?;
    let owned = owned_track_fingerprint_row(&track);
    let clips: Vec<_> = array(&snapshot["arrangement"]["clips"])
        .iter()
        .filter(|clip| clip["trackRef"] == reference || clip["parentRef"] == reference)
        .collect();
    fingerprint(&without_playback_state(&json!({"track":owned,"arrangementClips":clips})))
}
fn routing_state_revision(track: &Value) -> Result<String, LiveError> {
    let routing = &track["routing"];
    if !routing.is_object() {
        return Err(fail("transaction batch routing state is unavailable"));
    }
    fingerprint(
        &json!({"inputType":routing["inputType"],"inputSubRouting":routing["inputSubRouting"],"outputType":routing["outputType"],"outputSubRouting":routing["outputSubRouting"],"arm":track["armed"],"monitoring":track["monitoringState"]}),
    )
}
fn routing_target<'a>(snapshot: &'a Value, reference: &str) -> Result<&'a Value, LiveError> {
    array(&snapshot["tracks"])
        .iter()
        .find(|track| track["ref"] == reference && track["routing"].is_object() && is_non_empty_string(&track["objectIdentity"], 256))
        .ok_or_else(|| fail("transaction batch requires a track with exact authoritative routing identity"))
}

#[path = "batch_plan.rs"]
mod plan;
use plan::{batch_target_key, plan_operation, validate_operation};
fn merge(mut target: Value, source: Value) -> Value {
    target.as_object_mut().unwrap().extend(source.as_object().unwrap().clone());
    target
}

#[path = "batch_manager.rs"]
mod manager;
pub use manager::{AssertBatchPolicy, BatchTransactionManager};

#[path = "batch_steps.rs"]
mod steps;
