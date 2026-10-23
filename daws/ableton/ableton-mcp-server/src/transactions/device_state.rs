//! Private device-subtree snapshots and exact-authority recall/morph transactions.
use super::batch::{canonical, fingerprint, flatten_device_rows, is_non_empty_string};
use crate::live::LiveError;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::HashSet;
pub const DEVICE_STATE_SCHEMA: &str = "ableton-mcp-device-state/v1";
pub const DEVICE_STATE_TRANSACTION_TTL_MS: f64 = 30_000.;
pub const MAX_DEVICE_STATE_PARAMETERS: usize = 1024;
pub type DeviceStateFile = Value;
pub type DeviceStatePlan = Value;
#[derive(Debug, Clone, thiserror::Error)]
#[error("{error}")]
pub struct DeviceStateError {
    pub error: LiveError,
    pub device_state_report: Option<Value>,
}
impl DeviceStateError {
    pub fn message(&self) -> &str {
        self.error.message()
    }
}
impl From<LiveError> for DeviceStateError {
    fn from(error: LiveError) -> Self {
        Self { error, device_state_report: None }
    }
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
fn js_string(value: &Value) -> String {
    match value {
        Value::Null => "null".into(),
        Value::String(v) => v.clone(),
        Value::Array(v) => v.iter().map(|v| if v.is_null() { String::new() } else { js_string(v) }).collect::<Vec<_>>().join(","),
        Value::Object(_) => "[object Object]".into(),
        _ => kumi_common::js::json::stringify(value),
    }
}
fn device_identity(row: &Value) -> Result<Value, LiveError> {
    if !is_non_empty_string(&row["name"], 256) {
        return Err(fail("device state requires a named device row"));
    }
    Ok(
        json!({"name":row["name"],"className":if is_non_empty_string(&row["className"],256){row["className"].clone()}else{Value::Null},"kind":row["kind"].as_str().unwrap_or("device")}),
    )
}
fn class_key(identity: &Value) -> String {
    format!("{}:{}", string(&identity["kind"]), identity["className"].as_str().unwrap_or(string(&identity["name"])))
}
fn subtree_parameters(device: &Value, prefix: &str, sibling_index: Option<usize>) -> Result<Vec<Value>, LiveError> {
    let name = if is_non_empty_string(&device["name"], 256) { string(&device["name"]) } else { "unnamed" };
    let base = format!("{prefix}{name}{}", sibling_index.map(|i| format!("[{i}]")).unwrap_or_default());
    let mut rows = Vec::new();
    // A device can have two parameters of one name: the later ones' paths take #2, #3... in their order, so each
    // path names one parameter, the same way each time the device is read.
    let mut paths = HashSet::new();
    for parameter in array(&device["parameters"]).iter().filter(|v| v.is_object()) {
        if !is_non_empty_string(&parameter["ref"], 256) || !is_non_empty_string(&parameter["name"], 256) {
            return Err(fail("device state parameter identity is unavailable"));
        }
        if !number(&parameter["value"]).is_finite() || !parameter["min"].is_number() || !parameter["max"].is_number() {
            continue;
        }
        let mut path = format!("{base}/{}", string(&parameter["name"]));
        let mut count = 1;
        while !paths.insert(path.clone()) {
            count += 1;
            path = format!("{base}/{}#{count}", string(&parameter["name"]));
        }
        rows.push(json!({"path":path,"name":parameter["name"],"value":parameter["value"],"min":parameter["min"],"max":parameter["max"],"quantization":if number(&parameter["quantization"]).is_finite(){parameter["quantization"].clone()}else{json!(0)},"parameterRef":parameter["ref"],"deviceRef":device["ref"].as_str().unwrap_or(""),"automatable":parameter["automatable"]==true,"enabled":parameter["enabled"]!=false,"deviceEnabled":device["enabled"]!=false}));
    }
    let indexed_name = |row: &Value, index: usize, prefix: &str| {
        if is_non_empty_string(&row["name"], 256) {
            string(&row["name"]).to_owned()
        } else {
            format!("{prefix}-{}", row.get("index").filter(|v| !v.is_null()).map(js_string).unwrap_or_else(|| index.to_string()))
        }
    };
    for (chain_index, chain) in array(&device["chains"]).iter().enumerate() {
        if !chain.is_object() {
            continue;
        }
        let segment = format!("{}[{chain_index}]", indexed_name(chain, chain_index, "chain"));
        for (index, child) in array(&chain["devices"]).iter().enumerate() {
            if child.is_object() {
                rows.extend(subtree_parameters(child, &format!("{base}/{segment}/"), Some(index))?);
            }
        }
    }
    for (pad_index, pad) in array(&device["drumPads"]).iter().enumerate() {
        if !pad.is_object() {
            continue;
        }
        let pad_name = indexed_name(pad, pad_index, "pad");
        for (chain_index, chain) in array(&pad["chains"]).iter().enumerate() {
            if !chain.is_object() {
                continue;
            }
            let segment = format!("{pad_name}[{pad_index}]/{}[{chain_index}]", indexed_name(chain, chain_index, "chain"));
            for (index, child) in array(&chain["devices"]).iter().enumerate() {
                if child.is_object() {
                    rows.extend(subtree_parameters(child, &format!("{base}/{segment}/"), Some(index))?);
                }
            }
        }
    }
    Ok(rows)
}
fn layout_fingerprint(rows: &[Value]) -> Result<String, LiveError> {
    fingerprint(&json!(rows
        .iter()
        .map(|row| json!({"path":row["path"],"min":row["min"],"max":row["max"],"quantization":row["quantization"]}))
        .collect::<Vec<_>>()))
}
fn device_state_digest(core: &Value) -> Result<String, LiveError> {
    Ok(hex::encode(Sha256::digest(
        canonical(&json!({"schema":core["schema"],"device":core["device"],"parameters":core["parameters"]}))?.as_bytes(),
    )))
}
fn find_device_row<'a>(snapshot: &'a Value, device_ref: &str) -> Result<&'a Value, LiveError> {
    for track in array(&snapshot["tracks"]) {
        if let Some(found) = flatten_device_rows(&track["devices"]).into_iter().find(|item| item["ref"] == device_ref) {
            return Ok(found);
        }
    }
    Err(fail("device state target reference is not an authoritative device"))
}
pub fn build_device_state_file(snapshot: &Value, device_ref: &str, name: &str) -> Result<DeviceStateFile, LiveError> {
    let row = find_device_row(snapshot, device_ref)?;
    let identity = device_identity(row)?;
    let subtree = subtree_parameters(row, "", None)?;
    if !(1..=MAX_DEVICE_STATE_PARAMETERS).contains(&subtree.len()) {
        return Err(fail("device state requires 1-1024 numeric parameters in the target subtree"));
    }
    let mut core = json!({"schema":DEVICE_STATE_SCHEMA,"name":name,"device":{"identity":identity,"parameterCount":subtree.len(),"layoutFingerprint":layout_fingerprint(&subtree)?},"privacy":{"profile":"device-state/v1","note":"parameter and device names only; no project paths, session refs, object identities, or filesystem paths are persisted"},"parameters":subtree.iter().map(|row|json!({"path":row["path"],"name":row["name"],"value":row["value"],"min":row["min"],"max":row["max"],"quantization":row["quantization"]})).collect::<Vec<_>>()});
    let digest = device_state_digest(&core)?;
    core["savedAt"] = json!(kumi_common::time::iso_string(kumi_common::time::now_ms()));
    core["digest"] = digest.into();
    Ok(core)
}
pub fn validate_device_state_file(data: &Value) -> Result<DeviceStateFile, LiveError> {
    if !data.is_object() {
        return Err(fail("device state file is not a JSON object"));
    }
    if data["schema"] != DEVICE_STATE_SCHEMA {
        return Err(fail(format!("device state file schema is unsupported (expected {DEVICE_STATE_SCHEMA})")));
    }
    if !["name", "savedAt", "digest"].iter().all(|key| is_non_empty_string(&data[*key], 64))
        || !string(&data["digest"]).bytes().all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
        || string(&data["digest"]).len() != 64
    {
        return Err(fail("device state file name, timestamp, or digest is invalid"));
    }
    let device = &data["device"];
    let identity = &device["identity"];
    if !device.is_object()
        || !identity.is_object()
        || !is_non_empty_string(&identity["name"], 256)
        || !identity.get("className").is_some_and(|v| v.is_null() || is_non_empty_string(v, 256))
        || !is_non_empty_string(&identity["kind"], 64)
        || !number(&device["parameterCount"]).is_finite()
        || number(&device["parameterCount"]).fract() != 0.
        || !is_non_empty_string(&device["layoutFingerprint"], 64)
    {
        return Err(fail("device state file device identity is invalid"));
    }
    if !data["privacy"].is_object() || !data["privacy"]["profile"].is_string() {
        return Err(fail("device state file privacy profile is missing"));
    }
    let parameters = array(&data["parameters"]);
    if !data["parameters"].is_array()
        || !(1..=MAX_DEVICE_STATE_PARAMETERS).contains(&parameters.len())
        || parameters.len() as f64 != number(&device["parameterCount"])
    {
        return Err(fail("device state file parameter list is invalid"));
    }
    let mut seen = HashSet::new();
    for row in parameters {
        let value = number(&row["value"]);
        let min = number(&row["min"]);
        let max = number(&row["max"]);
        let quantization = number(&row["quantization"]);
        if !row.is_object()
            || !is_non_empty_string(&row["path"], 512)
            || !is_non_empty_string(&row["name"], 256)
            || !value.is_finite()
            || !(min <= max)
            || !quantization.is_finite()
            || quantization < 0.
            || value < min
            || value > max
        {
            return Err(fail("device state file parameter row is invalid"));
        }
        if !seen.insert(string(&row["path"])) {
            return Err(fail("device state file contains a duplicate parameter path"));
        }
    }
    if device_state_digest(data)? != string(&data["digest"]) {
        return Err(fail("device state file content digest does not match; the file was modified or corrupted"));
    }
    Ok(data.clone())
}
fn min(a: f64, b: f64) -> f64 {
    if a.is_nan() || b.is_nan() {
        f64::NAN
    } else if a == 0. && b == 0. {
        if a.is_sign_negative() || b.is_sign_negative() {
            -0.
        } else {
            0.
        }
    } else {
        a.min(b)
    }
}
fn max(a: f64, b: f64) -> f64 {
    if a.is_nan() || b.is_nan() {
        f64::NAN
    } else if a == 0. && b == 0. {
        if a.is_sign_negative() && b.is_sign_negative() {
            -0.
        } else {
            0.
        }
    } else {
        a.max(b)
    }
}
pub fn morph_value(from: f64, to: f64, amount: f64, minimum: f64, maximum: f64, quantization: f64) -> f64 {
    let raw = from + (to - from) * amount;
    if quantization > 0. {
        let steps = kumi_common::js::number::round((raw - minimum) / quantization);
        let max_steps = ((maximum - minimum) / quantization + 1e-9).floor();
        min(maximum, minimum + min(max(steps, 0.), max_steps) * quantization)
    } else {
        min(max(raw, minimum), maximum)
    }
}
pub fn plan_device_state_recall(
    snapshot: &Value,
    file: &Value,
    target_device_ref: &str,
    options: &Value,
) -> Result<DeviceStatePlan, DeviceStateError> {
    let row = find_device_row(snapshot, target_device_ref)?;
    let identity = device_identity(row)?;
    let subtree = subtree_parameters(row, "", None)?;
    let target_fingerprint = layout_fingerprint(&subtree)?;
    let target = |parameter: &Value| subtree.iter().find(|candidate| candidate["path"] == parameter["path"]);
    let bounds_match = |target: &Value, parameter: &Value| {
        ["min", "max", "quantization"].iter().all(|key| number(&target[*key]) == number(&parameter[*key]))
    };
    let report = |reason: String| -> DeviceStateError {
        let dispositions=array(&file["parameters"]).iter().map(|parameter|{let target=target(parameter);let mut row=json!({"path":parameter["path"],"disposition":match target{None=>"skipped-missing",Some(t)if !bounds_match(t,parameter)=>"skipped-rebound",_=>"skipped-read-only"},"reason":reason,"fileValue":parameter["value"]});if let Some(target)=target{row["targetValue"]=target["value"].clone();}row}).collect::<Vec<_>>();
        DeviceStateError {
            error: fail(reason.clone()),
            device_state_report: Some(
                json!({"refused":true,"reason":reason,"target":{"deviceRef":target_device_ref,"identity":identity,"layoutFingerprint":target_fingerprint},"file":{"name":file["name"],"identity":file["device"]["identity"],"layoutFingerprint":file["device"]["layoutFingerprint"]},"dispositions":dispositions}),
            ),
        }
    };
    if class_key(&identity) != class_key(&file["device"]["identity"]) {
        return Err(report(format!(
            "device state device class does not match the target ({} vs {})",
            class_key(&file["device"]["identity"]),
            class_key(&identity)
        )));
    }
    if json!(target_fingerprint) != file["device"]["layoutFingerprint"] && options["allowPartialLayout"] != true {
        return Err(report("device state parameter-layout fingerprint does not match the target; re-save a fresh snapshot or pass allowPartialLayout for a partial recall with per-parameter skips".into()));
    }
    let amount = number(&options["amount"]);
    let morph_from = options.get("morphFrom");
    if morph_from.is_some() && (!amount.is_finite() || !(0.0..=1.0).contains(&amount)) {
        return Err(fail("device state morph requires an explicit amount from 0 to 1").into());
    }
    let morph_file = morph_from.filter(|m| m["kind"] == "file").map(|m| &m["file"]);
    if morph_file.is_some_and(|file| class_key(&identity) != class_key(&file["device"]["identity"])) {
        return Err(report("device state morph source device class does not match the target".into()));
    }
    let mut dispositions = Vec::new();
    for parameter in array(&file["parameters"]) {
        let Some(target) = target(parameter) else {
            dispositions.push(json!({"path":parameter["path"],"disposition":"skipped-missing","reason":"no parameter with this stable path exists on the target","fileValue":parameter["value"]}));
            continue;
        };
        let skipped = |disposition: &str, reason: &str| json!({"path":parameter["path"],"disposition":disposition,"reason":reason,"fileValue":parameter["value"],"targetValue":target["value"]});
        if !bounds_match(target, parameter) {
            dispositions.push(skipped("skipped-rebound", "target bounds or quantization differ from the snapshot"));
            continue;
        }
        if target["automatable"] != true || target["enabled"] != true || target["deviceEnabled"] != true {
            dispositions.push(skipped("skipped-read-only", "parameter is disabled or not automatable on the target"));
            continue;
        }
        let mut proposed = number(&parameter["value"]);
        if let Some(morph_from) = morph_from {
            let from = if morph_from["kind"] == "live" {
                number(&target["value"])
            } else {
                let Some(source) =
                    morph_file.and_then(|file| array(&file["parameters"]).iter().find(|candidate| candidate["path"] == parameter["path"]))
                else {
                    dispositions.push(skipped("skipped-missing", "the morph source snapshot has no parameter with this stable path"));
                    continue;
                };
                number(&source["value"])
            };
            proposed = morph_value(
                from,
                proposed,
                amount,
                number(&parameter["min"]),
                number(&parameter["max"]),
                number(&parameter["quantization"]),
            );
        }
        dispositions.push(json!({"path":parameter["path"],"disposition":"applicable","fileValue":parameter["value"],"proposedValue":proposed,"targetValue":target["value"],"parameterRef":target["parameterRef"],"deviceRef":target["deviceRef"]}));
    }
    let applicable = dispositions.iter().filter(|row| row["disposition"] == "applicable").count();
    if applicable == 0 {
        return Err(report("device state recall has no applicable parameters on the target".into()));
    }
    Ok(
        json!({"deviceRef":target_device_ref,"identity":identity,"layoutFingerprint":target_fingerprint,"dispositions":dispositions,"applicable":applicable,"skipped":dispositions.len()-applicable}),
    )
}
#[path = "device_state_manager.rs"]
mod manager;
pub use manager::DeviceStateTransactionManager;
