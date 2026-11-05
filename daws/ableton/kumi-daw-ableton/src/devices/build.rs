//! A Max for Live audio effect built from a typed spec for Hermes (CC-22): `{name, about, controls, code}`, refused with
//! the findings rather than made the nearest way (standalone's make_device adjusts; this never does), checked against
//! GenExpr's limits on Live's audio thread ([`kumi_genexpr::limits`]), and ending in the guarded output stage
//! ([`kumi_genexpr::guard`]), proven before anything is written.
//!
//! It's written only as a new file, under the folder's free name, never over one (W1-A: no device this conversation made
//! is replaced, so two builds asking one name give "<name>" and "<name> 2"); a build whose free name isn't the one its
//! preview named refuses (W1-B). Once written, it waits for Live's Browser as make_device does.

use super::{
    amxd::{encode_amxd, DeviceType},
    gen::{guarded_effect_patcher, GenSpec},
    spec::{check_spec, AudioEffectSpec, DeviceSpec},
    tool::{beside, free_name, kind_folder, wait_listed, write_device, DeviceToolOptions},
};
use crate::ears::device::EARS_NAME;
use kumi_agent::core::contracts::{BuiltDevice, DevicePreview};
use kumi_common::abort::{Signal, SignalExt};
use kumi_types::errors::RuntimeError;
use serde_json::{json, Map, Value};
use std::path::{Path, PathBuf};

/// The most controls a built device has (22b).
pub const MOST_CONTROLS: usize = 16;
const SPEC_KEYS: [&str; 4] = ["name", "about", "controls", "code"];
const CONTROL_KEYS: [&str; 7] = ["name", "type", "min", "max", "default", "unit", "options"];
const CONTROL_TYPES: [&str; 4] = ["number", "integer", "choice", "switch"];

/// The spec as an audio effect, or every finding that refuses it.
pub fn checked(spec: &Value) -> Result<AudioEffectSpec, Vec<String>> {
    let Some(given) = spec.as_object() else {
        return Err(vec!["spec: give it as an object, {name, about, controls, code}.".into()]);
    };
    let mut found: Vec<String> = Vec::new();
    for key in given.keys().filter(|key| !SPEC_KEYS.contains(&key.as_str())) {
        found.push(format!("{key}: not part of a device's spec, which is name, about, controls and code."));
    }
    for key in ["name", "about", "code"] {
        if !given.get(key).is_some_and(Value::is_string) {
            found.push(format!("{key}: give it as text."));
        }
    }
    let controls = match given.get("controls") {
        None => Vec::new(),
        Some(Value::Array(controls)) => controls.clone(),
        Some(_) => {
            found.push("controls: give them as a list.".into());
            Vec::new()
        }
    };
    if controls.len() > MOST_CONTROLS {
        found.push(format!("controls: {} of them, past the {MOST_CONTROLS} a device has.", controls.len()));
    }
    for (index, control) in controls.iter().enumerate() {
        let Some(control) = control.as_object() else {
            found.push(format!("controls[{index}]: give it as an object."));
            continue;
        };
        for key in control.keys().filter(|key| !CONTROL_KEYS.contains(&key.as_str())) {
            found
                .push(format!("controls[{index}].{key}: not part of a control, which is name, type, min, max, default, unit and options."));
        }
        if !control.get("type").and_then(Value::as_str).is_some_and(|kind| CONTROL_TYPES.contains(&kind)) {
            found.push(format!("controls[{index}].type: number, integer, choice or switch."));
        }
    }
    let code = given.get("code").and_then(Value::as_str).unwrap_or_default();
    found.extend(kumi_genexpr::limits::check(code));
    if !found.is_empty() {
        return Err(found);
    }
    let mut input = Map::new();
    input.insert("type".into(), json!(DeviceType::AudioEffect.as_str()));
    for key in SPEC_KEYS {
        if let Some(value) = given.get(key) {
            input.insert(key.into(), value.clone());
        }
    }
    let (spec, checked) = check_spec(&input);
    let refused: Vec<String> = checked.wrong.into_iter().chain(checked.changed).collect();
    match spec {
        _ if !refused.is_empty() => Err(refused),
        DeviceSpec::AudioEffect(spec) => Ok(spec),
        _ => Err(vec!["spec: it's not an audio effect.".into()]),
    }
}

/// The folder a built device goes in.
fn folder(user_library: &Path) -> PathBuf {
    user_library.join("Kumi").join(kind_folder(DeviceType::AudioEffect))
}

/// The device's file as it's said, from the User Library's root: never an absolute path.
pub fn label(name: &str) -> String {
    format!("User Library/Kumi/{}/{name}.amxd", kind_folder(DeviceType::AudioEffect))
}

/// The name a device asked as `asked` is written under now: its own, numbered past a file the folder has.
fn free(folder: &Path, asked: &str) -> String {
    free_name(folder, asked, kumi_types::system::platform() == "win32", &[EARS_NAME])
}

/// What building `spec` would write, reading only the folder for its free name; nothing is written.
pub fn preview(user_library: &Path, spec: &Value) -> Result<DevicePreview, Vec<String>> {
    let checked = checked(spec)?;
    let name = free(&folder(user_library), &checked.name);
    Ok(DevicePreview {
        file: label(&name),
        name,
        controls: checked.controls.iter().map(|control| serde_json::to_value(control).unwrap_or_default()).collect(),
        lines: checked.code.lines().count(),
        code: checked.code,
    })
}

/// Builds `spec` into the User Library as a new file named as its preview's `file`, and waits for Live's Browser to list
/// it. Err inside: the findings that refuse the spec, or (the spec passing) that its free name isn't `file` any more:
/// nothing is written then. A build that can't be written, or whose output stage fails its proof, is an error.
pub async fn build(
    options: &DeviceToolOptions,
    spec: &Value,
    file: &str,
    signal: &Signal,
) -> Result<Result<BuiltDevice, Vec<String>>, RuntimeError> {
    let checked = match checked(spec) {
        Ok(checked) => checked,
        Err(found) => return Ok(Err(found)),
    };
    let folder = folder(Path::new(&options.user_library));
    let name = free(&folder, &checked.name);
    let stale = |name: &str| -> Result<Result<BuiltDevice, Vec<String>>, RuntimeError> {
        Ok(Err(vec![format!(
            "file: it would be written as {} now, not {file} as approved (a file appeared since): preview it again.",
            label(name)
        )]))
    };
    if label(&name) != file {
        return stale(&name);
    }
    // The fixed stage is proven for every device built, before anything is written; it takes a moment, beside the app's
    // thread.
    match beside(signal, || kumi_genexpr::guard::prove(kumi_genexpr::guard::STAGE)).await? {
        Some(Ok(())) => {}
        Some(Err(why)) => return Err(RuntimeError::plain(format!("Kumi's output stage failed its proof, so nothing was written: {why}"))),
        None => return Err(RuntimeError::plain("Kumi couldn't prove its output stage, so nothing was written.")),
    }
    signal.check()?;
    let patcher = guarded_effect_patcher(&GenSpec::from(&AudioEffectSpec { name: name.clone(), ..checked }));
    let bytes = encode_amxd(DeviceType::AudioEffect, &patcher);
    let mut builder = tokio::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    builder.mode(0o755);
    let path = folder.join(format!("{name}.amxd"));
    let written = match builder.create(&folder).await {
        Ok(()) => write_device(&folder, &path, &bytes, false).await,
        Err(error) => Err(error),
    };
    match written {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => return stale(&name),
        Err(error) => return Err(RuntimeError::plain(format!("Kumi couldn't write the device to Live's User Library: {error}"))),
    }
    let item = format!("user_library/Kumi/{}/{name}", kind_folder(DeviceType::AudioEffect));
    let listed = wait_listed(options, &item, signal).await?;
    Ok(Ok(BuiltDevice { file: label(&name), name, item, path, listed }))
}
