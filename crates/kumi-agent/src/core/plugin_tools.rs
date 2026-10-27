//! Native VST3 discovery and preset work, shared by every DAW. Loading remains a DAW change.

use std::{
    io::{Read, Write},
    path::{Path, PathBuf},
    rc::Rc,
};

use async_trait::async_trait;
use kumi_common::{
    abort::{Signal, SignalExt},
    js::json::stringify,
};
use kumi_library::plugins::{
    formats::{structure, MAX_DECODED_BYTES},
    inventory, preset,
};
use kumi_types::{
    contracts::{JsonObject, KernelTool, Reach, ToolResult},
    errors::RuntimeError,
};
use serde_json::{json, Value};

pub const GUIDANCE: &str = "Use Kumi's vst3 tool for plug-ins before researching external resources. Call installed to discover this computer's VST3s and roles; catalog gives exact editable paths, stored units, tested versions and limitations. Create or edit with Kumi's built-in writers, then use the DAW's preset-loading change and verify the loaded result. Installed files alone do not prove the DAW can load or authorize a plug-in. For mastering, reference mastering, louder or wider requests: read vst3 guide, check installed processing, prefer Ozone 12 when available, then suitable installed VST3s, then native devices if suitable plug-ins cannot be used. Respect any specific processor the producer asks for. A bucket-brigade delay request should check ValhallaDelay. External research is a fallback when the built-in format or host operation cannot handle the request.";

const MASTERING: &str = "Mastering workflow (the same on every DAW):\n1. Read the current master/output chain and routing. Preserve intentional processing, and reuse suitable existing processors. Render or capture a representative passage and the whole mix as needed. Use listen to measure loudness, true peak, dynamics, tonal balance and stereo width. If a reference was supplied, use listen with compare_to on comparable sections, loudness matched; do not infer targets from the reference's filename or genre.\n2. Call installed with role mastering, limiting, eq, compression or stereo as needed. Prefer an available Ozone 12 suite/module for those jobs, otherwise appropriate installed VST3s (for example LOUD for loudness, WIDE for width, Weiss for dynamics), then the DAW's native processors when suitable plug-ins cannot be used. Confirm the DAW sees the exact VST3; a filesystem match is not a successful load. Choose the minimum useful processing, not every listed plug-in.\n3. Read catalog/inspect for exact paths, units and verified versions. Create a preset from the plug-in's initialized state, or edit a saved existing state to preserve its settings. Load through the DAW's change tools so HISTORY and undo cover the change. Use existing host parameter tools where a serialized writer is unavailable; do not invent unsupported fields.\n4. Work in small measured passes: correct tonal problems, control dynamics if needed, adjust width while checking mono/bass compatibility, and place final limiting after gain-raising processing. Do not use the track fader alone as mastering. Choose target loudness from the producer/reference and the material, with an explicit true-peak ceiling; avoid imposing a universal LUFS target. Ozone paths and module chain come from catalog/inspect, never guessed parameter numbers.\n5. Render/capture again; compare with the source and reference at matched loudness. Check integrated/short-term loudness, true peak, transients, pumping, clipping and mono compatibility. Reduce processing if it damages the material. Repeat only while the measured or heard result needs work. Report what changed and the measured result; file creation alone does not mean the track was mastered.";

pub struct PluginTool {
    roots: Vec<PathBuf>,
}

impl PluginTool {
    pub fn new() -> Rc<dyn KernelTool> {
        Rc::new(Self { roots: inventory::standard_roots() })
    }

    fn run(&self, input: &JsonObject) -> Result<Value, String> {
        let string = |key: &str| input.get(key).and_then(Value::as_str);
        let required = |key: &str| string(key).filter(|s| !s.is_empty()).ok_or_else(|| format!("{key} is required"));
        let action = required("action")?;
        let limit = input.get("limit").and_then(Value::as_u64).unwrap_or(60).clamp(1, 200) as usize;
        match action {
            "installed" => {
                let mut roots = self.roots.clone();
                if let Some(extra) = input.get("folders") {
                    let extra: Vec<PathBuf> =
                        serde_json::from_value(extra.clone()).map_err(|_| "folders must be a list of absolute directories")?;
                    if extra.len() > 16 || extra.iter().any(|p| !p.is_absolute()) {
                        return Err("folders takes at most 16 absolute VST3 directories".into());
                    }
                    roots.extend(extra);
                }
                let found = inventory::scan(&roots);
                let matching: Vec<_> = found
                    .plugins
                    .iter()
                    .filter(|p| string("role").is_none_or(|r| inventory::matches_role(p, r)))
                    .filter(|p| string("query").is_none_or(|q| p.name.to_lowercase().contains(&q.to_lowercase())))
                    .collect();
                Ok(
                    json!({"format":"VST3", "plugins":matching.iter().take(limit).collect::<Vec<_>>(), "more":matching.len().saturating_sub(limit),
                    "unclassified":if string("role").is_some() { found.plugins.iter().filter(|p|p.roles.is_empty()).take(limit).collect::<Vec<_>>() } else { Vec::new() },
                    "warnings":found.warnings, "note":"Presence only; confirm loadability and the exact class/version in the DAW. Unclassified products may still do the requested job: inspect the DAW's browser/parameters before falling back to native devices. No VST2/AU/CLAP scan."}),
                )
            }
            "guide" => Ok(
                json!({"workflow":MASTERING, "preset_workflow":"installed → catalog (query exact controls) → inspect an existing file or create from init → edit if needed → the DAW's preset-loading change → read back and listen"}),
            ),
            "catalog" => {
                if let Some(plugin) = string("plugin") {
                    let p = structure::builtin(plugin).ok_or_else(|| format!("No built-in format for {plugin}"))?;
                    let words: Vec<_> = string("query").unwrap_or_default().to_lowercase().split_whitespace().map(str::to_string).collect();
                    let matches = |path: &str| words.iter().all(|w| path.to_lowercase().contains(w));
                    let fields: Vec<_> = p
                        .structure
                        .fields
                        .iter()
                        .filter(|(path, field)| matches(&format!("{path} {}", field.about.as_deref().unwrap_or_default())))
                        .collect();
                    let parameters: Vec<_> =
                        p.structure.parameters.iter().filter(|(name, link)| matches(&format!("{name} {}", link.field))).collect();
                    Ok(
                        json!({"plugin":p.info, "verified":p.verified, "fields":fields.iter().take(limit).map(|(path,field)|json!({"path":path,"field":field})).collect::<Vec<_>>(),
                        "parameters":parameters.iter().take(limit).map(|(name,link)|json!({"name":name,"path":structure::Structure::location(link),"link":link})).collect::<Vec<_>>(),
                        "more_fields":fields.len().saturating_sub(limit), "more_parameters":parameters.len().saturating_sub(limit),
                        "operations":match plugin {
                            "ozone-12" => json!({"$chain":{"value":"ordered array of module names; suite only","modules":kumi_library::plugins::formats::ozone12::MODULES}}),
                            "serum-2" => json!({"$add_distortion":{"value":"rack number: 0 for main, 1 or 2 for buses"}}),
                            _ => json!({}),
                        },
                        "note":"Paths with {n} or [] describe repeated sections; inspect gives concrete paths. 'seen' is an observed range, not an allowed range. Write only verified versions. $chain replaces an Ozone module order; $add_distortion adds a Serum initialized Distortion slot (value is rack 0–2)."}),
                    )
                } else {
                    Ok(
                        json!({"plugins":structure::builtins().iter().map(|p|json!({"plugin":p.info.plugin,"name":p.info.name,"roles":p.info.roles,"writes":p.verified.writes})).collect::<Vec<_>>()}),
                    )
                }
            }
            "inspect" => {
                let bytes = read_file(Path::new(required("input")?))?;
                preset::inspect_fields(required("plugin")?, &bytes, string("class_id"), string("query"), limit).map_err(|e| e.to_string())
            }
            "edit-native" | "create-native-from-template" => {
                let bytes = read_file(Path::new(required("input")?))?;
                let edits: Vec<preset::Edit> =
                    serde_json::from_value(input.get("edits").cloned().unwrap_or_else(|| json!([]))).map_err(|e| format!("edits: {e}"))?;
                if edits.len() > 512 {
                    return Err("At most 512 edits per preset".into());
                }
                let made = if action == "edit-native" {
                    preset::edit_native(required("plugin")?, &bytes, string("name"), &edits)
                } else {
                    preset::create_native_from_template(
                        required("plugin")?,
                        &bytes,
                        input.get("preset_index").and_then(Value::as_u64).unwrap_or(0) as usize,
                        required("name")?,
                        &edits,
                    )
                }
                .map_err(|e| e.to_string())?;
                let output = Path::new(required("output")?);
                if !output.is_absolute()
                    || !output.extension().is_some_and(|ext| ext.eq_ignore_ascii_case(made.extension.trim_start_matches('.')))
                {
                    return Err(format!("output must be an absolute {} path", made.extension));
                }
                if made.bytes.len() > MAX_DECODED_BYTES {
                    return Err("Preset exceeds 64 MiB".into());
                }
                write_new(output, &made.bytes)?;
                Ok(json!({"file":output,"plugin":made.plugin,"format":"native preset","source_version":made.source_version,
                    "runtime_verified":made.runtime_verified,"identity":made.identity_note,
                    "next":"The native file is written. Plugin acceptance has not been verified for this format/product. It is not a VST3 component state and cannot be sent to the VST3 loading changes."}))
            }
            "create" | "edit" => {
                let plugin = required("plugin")?;
                let output = Path::new(required("output")?);
                if !output.is_absolute() || !output.extension().is_some_and(|ext| ext.eq_ignore_ascii_case("vstpreset")) {
                    return Err("output must be an absolute .vstpreset path".into());
                }
                let edits: Vec<preset::Edit> =
                    serde_json::from_value(input.get("edits").cloned().unwrap_or_else(|| json!([]))).map_err(|e| format!("edits: {e}"))?;
                if edits.len() > 512 {
                    return Err("At most 512 edits per preset".into());
                }
                let options = preset::WriteOptions {
                    version: required("version")?.into(),
                    class_id: string("class_id").map(str::to_string),
                    name: string("name").map(str::to_string),
                };
                let result = if action == "create" {
                    preset::create(plugin, &options, &edits)
                } else {
                    preset::edit(plugin, &read_file(Path::new(required("input")?))?, &options, &edits)
                }
                .map_err(|e| e.to_string())?;
                let bytes = result.write();
                if bytes.len() > MAX_DECODED_BYTES {
                    return Err("Preset exceeds 64 MiB".into());
                }
                write_new(output, &bytes)?;
                Ok(json!({"file":output,"plugin":plugin,"class_id":result.class_id,"format":"VST3","bytes":bytes.len(),"edits":edits.len(),
                    "next":"Load this file through the DAW's preset-loading change, read back the changed controls and listen. This tool only created the preset file."}))
            }
            _ => Err("action is installed, catalog, guide, inspect, create, edit, edit-native or create-native-from-template".into()),
        }
    }
}

fn read_file(path: &Path) -> Result<Vec<u8>, String> {
    if !path.is_absolute() {
        return Err("input must be an absolute file path".into());
    }
    let file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    if !file.metadata().map_err(|e| e.to_string())?.is_file() {
        return Err("input must be a regular file".into());
    }
    let mut bytes = Vec::new();
    file.take(MAX_DECODED_BYTES as u64 + 1).read_to_end(&mut bytes).map_err(|e| e.to_string())?;
    if bytes.len() > MAX_DECODED_BYTES {
        return Err("Preset exceeds 64 MiB".into());
    }
    Ok(bytes)
}

fn write_new(path: &Path, bytes: &[u8]) -> Result<(), String> {
    // create_new avoids overwriting a user's preset, including a symlink raced into place.
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|e| format!("{}: {e}; use a new output filename", path.display()))?;
    if let Err(e) = file.write_all(bytes).and_then(|_| file.sync_all()) {
        drop(file);
        let _ = std::fs::remove_file(path);
        return Err(e.to_string());
    }
    Ok(())
}

#[async_trait(?Send)]
impl KernelTool for PluginTool {
    fn name(&self) -> &str {
        "vst3"
    }
    fn description(&self) -> &str {
        "Built-in VST3 discovery, preset creation and editing. Use before external research. installed lists local plug-ins and roles; catalog gives tested versions and exact editable fields; inspect reads a preset; create starts an initialized patch; edit preserves existing state. Writes a new .vstpreset for the DAW's loading tools. guide gives the measured mastering/reference workflow."
    }
    fn reach(&self) -> Reach {
        Reach::AwayFromDaw
    }
    fn input_schema(&self) -> JsonObject {
        json!({"type":"object","additionalProperties":false,"required":["action"],"properties":{
        "action":{"type":"string","enum":["installed","catalog","guide","inspect","create","edit","edit-native","create-native-from-template"]},
        "plugin":{"type":"string","description":"Catalog ID, e.g. serum-2, ozone-12, valhalla-delay"},
        "role":{"type":"string","description":"installed: mastering, limiting, eq, compression, stereo, delay, bbd, reverb, instrument…"},
        "query":{"type":"string","description":"Name/control search; narrow catalog results before choosing paths"},
        "folders":{"type":"array","maxItems":16,"items":{"type":"string"},"description":"Additional absolute VST3 install directories"},
        "input":{"type":"string","description":"Absolute existing preset path"},"output":{"type":"string","description":"Absolute new preset path; .vstpreset for create/edit, the native extension for native actions; never overwrites"},
        "preset_index":{"type":"integer","minimum":0,"description":"create-native-from-template: source preset index within a Softube bundle (0 otherwise)"},
        "version":{"type":"string","description":"Exact verified write version from catalog; required for create/edit"},
        "class_id":{"type":"string","description":"Exact VST3 class, required to choose an Ozone individual module"},
        "name":{"type":"string"},"limit":{"type":"integer","minimum":1,"maximum":200},
        "edits":{"type":"array","maxItems":512,"items":{"type":"object","additionalProperties":false,"required":["path","value"],"properties":{"path":{"type":"string"},"value":{}}}}
    }}).as_object().cloned().unwrap_or_default()
    }
    async fn execute(&self, input: JsonObject, signal: Signal) -> Result<ToolResult, RuntimeError> {
        signal.check()?;
        Ok(match self.run(&input) {
            Ok(reply) => ToolResult { text: stringify(&reply), ..Default::default() },
            Err(why) => ToolResult { text: why, is_error: true, ..Default::default() },
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn input(value: Value) -> JsonObject {
        value.as_object().unwrap().clone()
    }

    #[test]
    fn discovers_local_roles_and_writes_an_inspectable_preset_without_overwriting() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(temp.path().join("ValhallaDelay.vst3"), b"not executed").unwrap();
        let tool = PluginTool { roots: vec![temp.path().into()] };
        let installed = tool.run(&input(json!({"action":"installed","role":"bbd"}))).unwrap();
        assert_eq!(installed["plugins"][0]["plugin"], "valhalla-delay");
        let file = temp.path().join("BBD.vstpreset");
        let create = input(json!({"action":"create","plugin":"valhalla-delay","version":"3.0.5","output":file,"edits":[
            {"path":"parameters/Mode","value":0.125}, {"path":"parameters/Mix","value":0.25}
        ]}));
        let reply = tool.run(&create).unwrap();
        assert_eq!(reply["format"], "VST3");
        let before = std::fs::read(&file).unwrap();
        assert!(tool.run(&create).is_err());
        assert_eq!(std::fs::read(&file).unwrap(), before);
        let inspected =
            tool.run(&input(json!({"action":"inspect","plugin":"valhalla-delay","input":file,"query":"Mix","limit":5}))).unwrap();
        assert!(inspected["fields"].as_array().unwrap().iter().any(|f| f["path"] == "parameters/Mix" && f["value"] == 0.25));
    }

    #[test]
    fn unsupported_versions_and_invalid_paths_create_no_file() {
        let temp = tempfile::tempdir().unwrap();
        let tool = PluginTool { roots: vec![temp.path().into()] };
        let file = temp.path().join("invalid.vstpreset");
        let mut request = input(json!({"action":"create","plugin":"serum-2","version":"unknown","output":file}));
        assert!(tool.run(&request).is_err());
        request.insert("version".into(), json!("2.1.5"));
        request.insert("edits".into(), json!([{"path":"Oscillator1/plainParams/not-real","value":1}]));
        assert!(tool.run(&request).is_err());
        assert!(!file.exists());
    }
}
