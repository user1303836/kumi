//! Native VST3 discovery and preset work, shared by every DAW. Loading remains a DAW change.

use std::{
    io::{Read, Write},
    path::{Path, PathBuf},
    rc::Rc,
    sync::{Arc, Mutex},
};

use async_trait::async_trait;
use kumi_common::{
    abort::{Signal, SignalExt},
    js::json::stringify,
    path::{network_or_device, reserved_device},
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

pub const GUIDANCE: &str = "vst3 (find_tools) discovers installed VST3 processors and creates/edits their presets locally; use it when choosing new processing, before native fallback or external research. Reuse suitable processors already in the project.";

const MASTERING: &str = "Mastering and reference mastering: preserve routing and intentional processing; reuse suitable existing processors and adjust their exposed parameters. When adding processing, prefer an available Ozone 12 suite/module, then suitable installed VST3s, then native devices if no suitable plug-in can be used. A specific processor requested by the producer takes precedence. installed gives roles and verified write versions; catalog/inspect is for controls whose paths or units you need. Creating or reloading a preset is optional, not required for each measured pass. Use the DAW's changes for HISTORY and undo; a file on disk is not a loaded processor.\nUse listen on comparable sections before and after; compare any reference at matched loudness. Choose loudness from the material and the producer's goal, with an explicit true-peak ceiling. Check dynamics, tonal balance, transients, pumping, clipping, width and mono compatibility; put final limiting after gain-raising processing. Report the measured result, and reduce processing that damages the music. A simple track-level adjustment is not a request to master it.";

#[derive(Clone)]
pub struct PluginTool {
    roots: Vec<PathBuf>,
    output: PathBuf,
    cache: Arc<Mutex<Option<(Vec<PathBuf>, inventory::Inventory)>>>,
    stop: Signal,
}

impl PluginTool {
    pub fn new(output: Option<PathBuf>) -> Rc<dyn KernelTool> {
        Rc::new(Self {
            roots: inventory::standard_roots(),
            output: output.unwrap_or_else(|| std::env::temp_dir().join(format!("kumi-presets-{}", uuid::Uuid::new_v4()))),
            cache: Arc::default(),
            stop: Signal::default(),
        })
    }

    fn installed(&self, input: &JsonObject) -> Result<inventory::Inventory, String> {
        let mut roots = self.roots.clone();
        if let Some(extra) = input.get("folders") {
            let extra: Vec<PathBuf> =
                serde_json::from_value(extra.clone()).map_err(|_| "folders must be a list of absolute directories")?;
            if extra.len() > 16 {
                return Err("folders takes at most 16 absolute VST3 directories".into());
            }
            roots.extend(extra);
        }
        let mut cache = self.cache.lock().map_err(|_| "VST3 discovery cache is unavailable")?;
        if !input.contains_key("folders") {
            if let Some((previous, _)) = cache.as_ref() {
                roots = previous.clone();
            }
        }
        for root in &roots {
            local_path(root)?;
        }
        if input.get("refresh") != Some(&Value::Bool(true)) {
            if let Some((_, found)) = cache.as_ref().filter(|(cached_roots, _)| cached_roots == &roots) {
                return Ok(found.clone());
            }
        }
        let found = inventory::scan(&roots);
        *cache = Some((roots, found.clone()));
        Ok(found)
    }

    fn output(&self, name: &str, extension: &str) -> Result<PathBuf, String> {
        self.stop.check().map_err(|e| e.to_string())?;
        let extension = extension.trim_start_matches('.');
        let stem = name.split('.').next().unwrap_or_default().trim_end().to_ascii_uppercase();
        if name.trim().is_empty()
            || name.chars().count() > 96
            || name.ends_with(['.', ' '])
            || name.chars().any(|c| c.is_control() || r#"<>:"/\|?*"#.contains(c))
            || matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$")
            || (stem.chars().count() == 4
                && (stem.starts_with("COM") || stem.starts_with("LPT"))
                && stem.ends_with(['1', '2', '3', '4', '5', '6', '7', '8', '9', '¹', '²', '³']))
            || !Path::new(name).extension().is_some_and(|ext| ext.eq_ignore_ascii_case(extension))
        {
            return Err(format!("output must be a plain filename ending in .{extension}, without separators or device names"));
        }
        local_path(&self.output)?;
        std::fs::create_dir_all(&self.output).map_err(|e| format!("Kumi couldn't create its preset folder: {e}"))?;
        Ok(self.output.join(name))
    }

    fn run(&self, input: &JsonObject) -> Result<Value, String> {
        self.stop.check().map_err(|e| e.to_string())?;
        let string = |key: &str| input.get(key).and_then(Value::as_str);
        let required = |key: &str| string(key).filter(|s| !s.is_empty()).ok_or_else(|| format!("{key} is required"));
        let action = required("action")?;
        let limit = input.get("limit").and_then(Value::as_u64).unwrap_or(20).clamp(1, 40) as usize;
        match action {
            "installed" => {
                let found = self.installed(input)?;
                let limit = limit.min(20);
                let matching: Vec<_> = found
                    .plugins
                    .iter()
                    .filter(|p| string("role").is_none_or(|r| inventory::matches_role(p, r)))
                    .filter(|p| string("query").is_none_or(|q| p.name.to_lowercase().contains(&q.to_lowercase())))
                    .collect();
                Ok(
                    json!({"format":"VST3", "plugins":matching.iter().take(limit).collect::<Vec<_>>(), "more":matching.len().saturating_sub(limit),
                    "unclassified":if string("role").is_some() { found.plugins.iter().filter(|p|p.roles.is_empty()).filter(|p|string("query").is_none_or(|q|p.name.to_lowercase().contains(&q.to_lowercase()))).take(limit.min(20)).map(|p|p.name.as_str()).collect::<Vec<_>>() } else { Vec::new() },
                    "warnings":found.warnings, "note":"Presence only; confirm loadability and the exact class/version in the DAW. Unclassified products may still do the requested job: inspect the DAW's browser/parameters before falling back to native devices. No VST2/AU/CLAP scan."}),
                )
            }
            "guide" => Ok(json!({"workflow":MASTERING})),
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
                            "serum-2" => json!({"$add_distortion":{"value":"effects chain number: 0 for main, 1 or 2 for buses"}}),
                            _ => json!({}),
                        },
                        "note":"Paths with {n} or [] describe repeated sections; inspect gives concrete paths. 'seen' is an observed range, not an allowed range. Write only verified versions. $chain replaces an Ozone module order; $add_distortion adds a Serum initialized Distortion slot (value selects effects chain 0–2)."}),
                    )
                } else {
                    let matched: Vec<_> = structure::builtins()
                        .iter()
                        .filter(|p| {
                            string("query")
                                .is_none_or(|q| format!("{} {}", p.info.plugin, p.info.name).to_lowercase().contains(&q.to_lowercase()))
                        })
                        .collect();
                    Ok(
                        json!({"plugins":matched.iter().take(limit.min(20)).map(|p|json!({"plugin":p.info.plugin,"name":p.info.name})).collect::<Vec<_>>(),"more":matched.len().saturating_sub(limit.min(20)),"next":"Narrow by query, or give plugin for exact controls."}),
                    )
                }
            }
            "inspect" => {
                let bytes = read_file(Path::new(required("input")?), required("plugin")?)?;
                preset::inspect_fields(required("plugin")?, &bytes, string("class_id"), string("query"), limit).map_err(|e| e.to_string())
            }
            "edit-native" | "create-native-from-template" => {
                let bytes = read_file(Path::new(required("input")?), required("plugin")?)?;
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
                let output = self.output(required("output")?, &made.extension)?;
                if made.bytes.len() > MAX_DECODED_BYTES {
                    return Err("Preset exceeds the supported size limit".into());
                }
                write_new(&output, &made.bytes)?;
                Ok(json!({"file":output,"plugin":made.plugin,"format":"native preset","source_version":made.source_version,
                    "runtime_verified":made.runtime_verified,"identity":made.identity_note,
                    "next":"The native file is written. Plug-in acceptance has not been verified for this format/product. It is not a VST3 component state and cannot be sent to the VST3 loading changes."}))
            }
            "create" | "edit" => {
                let plugin = required("plugin")?;
                let found = self.installed(input)?;
                let matches: Vec<_> = found
                    .plugins
                    .iter()
                    .filter(|p| {
                        p.plugin.as_deref() == Some(plugin)
                            && string("class_id").is_none_or(|id| p.class_id.as_deref().is_some_and(|known| known.eq_ignore_ascii_case(id)))
                    })
                    .collect();
                let classes: std::collections::BTreeSet<_> =
                    matches.iter().filter_map(|p| p.class_id.as_ref().map(|id| id.to_ascii_uppercase())).collect();
                if matches.is_empty() || classes.len() != 1 || matches.iter().any(|p| p.class_id.is_none()) {
                    return Err(
                        "Choose an installed VST3 class with installed/class_id; a write requires one exact installed product and version"
                            .into(),
                    );
                }
                // Discovery is cached, but a plug-in may have been updated since the last
                // query. Read this installation again before deciding which state to write.
                let mut versions = std::collections::BTreeSet::new();
                for cached in &matches {
                    let current = inventory::scan(std::slice::from_ref(&cached.path));
                    let installed = current
                        .plugins
                        .iter()
                        .find(|p| p.plugin == cached.plugin && p.class_id == cached.class_id)
                        .ok_or("The selected VST3 installation changed; refresh installed and choose it again")?;
                    if installed.installed_version_verified != Some(true) {
                        return Err("The installed VST3 version is unknown or has no verified writer. Inspect presets or use the host's exposed parameters.".into());
                    }
                    versions.insert(installed.version.clone().ok_or("Installed VST3 version is unknown")?);
                }
                if versions.len() != 1 {
                    return Err("The same VST3 class has conflicting installed versions; use exposed parameters until the installation is unambiguous".into());
                }
                let version = versions.iter().next().expect("at least one checked installation").as_str();
                if string("version").is_some_and(|asked| asked != version) {
                    return Err(format!("The installed version is {version}; a requested version cannot override it"));
                }
                let edits: Vec<preset::Edit> =
                    serde_json::from_value(input.get("edits").cloned().unwrap_or_else(|| json!([]))).map_err(|e| format!("edits: {e}"))?;
                if edits.len() > 512 {
                    return Err("At most 512 edits per preset".into());
                }
                let options = preset::WriteOptions {
                    version: version.into(),
                    class_id: matches[0].class_id.clone(),
                    name: string("name").map(str::to_string),
                };
                let result = if action == "create" {
                    preset::create(plugin, &options, &edits)
                } else {
                    preset::edit(plugin, &read_file(Path::new(required("input")?), plugin)?, &options, &edits)
                }
                .map_err(|e| e.to_string())?;
                let bytes = result.write();
                if bytes.len() > MAX_DECODED_BYTES {
                    return Err("Preset exceeds the supported size limit".into());
                }
                let output = self.output(required("output")?, "vstpreset")?;
                write_new(&output, &bytes)?;
                Ok(json!({"file":output,"plugin":plugin,"class_id":result.class_id,"format":"VST3","bytes":bytes.len(),"edits":edits.len(),
                    "next":"Load this file through the DAW's preset-loading change, read back the changed controls and listen. This tool only created the preset file."}))
            }
            _ => Err("action is installed, catalog, guide, inspect, create, edit, edit-native or create-native-from-template".into()),
        }
    }
}

/// Whether a path a model gives is one Kumi opens: absolute, on this computer, and no device's or another machine's.
pub fn local_path(path: &Path) -> Result<(), String> {
    let text = path.to_string_lossy();
    if text.chars().any(char::is_control) || network_or_device(&text) || reserved_device(&text) || !path.is_absolute() {
        return Err("Use an absolute local path; network and device paths are not supported".into());
    }
    Ok(())
}

fn read_file(path: &Path, plugin: &str) -> Result<Vec<u8>, String> {
    local_path(path)?;
    if !std::fs::metadata(path).map_err(|e| e.to_string())?.is_file() {
        return Err("input must be a regular file".into());
    }
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NONBLOCK | libc::O_NOFOLLOW);
    }
    let file = options.open(path).map_err(|e| e.to_string())?;
    if !file.metadata().map_err(|e| e.to_string())?.is_file() {
        return Err("input must be a regular file".into());
    }
    let mut bytes = Vec::new();
    let limit = preset::input_limit(plugin);
    file.take(limit as u64 + 1).read_to_end(&mut bytes).map_err(|e| e.to_string())?;
    if bytes.len() > limit {
        return Err("Preset exceeds the supported size limit".into());
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
    fn deferred(&self) -> bool {
        true
    }
    fn input_schema(&self) -> JsonObject {
        json!({"type":"object","additionalProperties":false,"required":["action"],"properties":{
        "action":{"type":"string","enum":["installed","catalog","guide","inspect","create","edit","edit-native","create-native-from-template"]},
        "plugin":{"type":"string","description":"Catalog ID, e.g. serum-2, ozone-12, valhalla-delay"},
        "role":{"type":"string","description":"installed: mastering, limiting, eq, compression, stereo, delay, bbd, reverb, instrument…"},
        "query":{"type":"string","description":"Name/control search; narrow catalog results before choosing paths"},
        "folders":{"type":"array","maxItems":16,"items":{"type":"string"},"description":"Additional absolute VST3 install directories"},
        "refresh":{"type":"boolean","description":"Rescan the session's installed inventory after installing or updating a plug-in"},
        "input":{"type":"string","description":"Absolute local existing preset path"},"output":{"type":"string","description":"New filename only, saved in Kumi's preset folder; .vstpreset for create/edit, native extension for native actions; never overwrites"},
        "preset_index":{"type":"integer","minimum":0,"description":"create-native-from-template: source preset index within a Softube bundle (0 otherwise)"},
        "version":{"type":"string","description":"Optional assertion of installed version; cannot override inventory's version gate"},
        "class_id":{"type":"string","description":"Exact VST3 class, required to choose an Ozone individual module"},
        "name":{"type":"string"},"limit":{"type":"integer","minimum":1,"maximum":40},
        "edits":{"type":"array","maxItems":512,"items":{"type":"object","additionalProperties":false,"required":["path","value"],"properties":{"path":{"type":"string"},"value":{}}}}
    }}).as_object().cloned().unwrap_or_default()
    }
    async fn execute(&self, input: JsonObject, signal: Signal) -> Result<ToolResult, RuntimeError> {
        signal.check()?;
        let mut this = self.clone();
        this.stop = signal.clone();
        let working = tokio::task::spawn_blocking(move || this.run(&input));
        let result = tokio::select! {
            result = working => result.map_err(|e|RuntimeError::plain(format!("VST3 work failed: {e}")))?,
            _ = signal.cancelled() => { signal.check()?; unreachable!() }
        };
        Ok(match result {
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
    fn tool(temp: &Path) -> PluginTool {
        PluginTool { roots: vec![temp.into()], output: temp.join("owned"), cache: Arc::default(), stop: Signal::default() }
    }
    fn installation(temp: &Path, version: Option<&str>) -> PathBuf {
        let resources = temp.join("ValhallaDelay.vst3/Contents/Resources");
        std::fs::create_dir_all(&resources).unwrap();
        let metadata = resources.join("moduleinfo.json");
        std::fs::write(
            &metadata,
            serde_json::to_vec(&json!({"Version":version,"Classes":[
                {"Name":"ValhallaDelay","CID":"565354644C617976616C68616C6C6164","Category":"Audio Module Class"}
            ]}))
            .unwrap(),
        )
        .unwrap();
        metadata
    }

    #[test]
    fn discovers_local_roles_and_writes_an_inspectable_preset_without_overwriting() {
        let temp = tempfile::tempdir().unwrap();
        installation(temp.path(), Some("3.0.5"));
        let tool = tool(temp.path());
        let installed = tool.run(&input(json!({"action":"installed","role":"bbd"}))).unwrap();
        assert_eq!(installed["plugins"][0]["plugin"], "valhalla-delay");
        let file = tool.output.join("BBD.vstpreset");
        let create = input(json!({"action":"create","plugin":"valhalla-delay","output":"BBD.vstpreset","edits":[
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
        let tool = tool(temp.path());
        let file = temp.path().join("invalid.vstpreset");
        let mut request = input(json!({"action":"create","plugin":"serum-2","version":"unknown","output":file}));
        assert!(tool.run(&request).is_err());
        request.insert("version".into(), json!("2.1.5"));
        request.insert("edits".into(), json!([{"path":"Oscillator1/plainParams/not-real","value":1}]));
        assert!(tool.run(&request).is_err());
        assert!(!file.exists());
    }

    #[test]
    fn inventory_cache_never_overrides_the_installed_write_version() {
        let temp = tempfile::tempdir().unwrap();
        installation(temp.path(), Some("3.0.5"));
        let tool = tool(temp.path());
        let query = input(json!({"action":"installed"}));
        assert_eq!(tool.run(&query).unwrap()["plugins"][0]["version"], "3.0.5");
        installation(temp.path(), Some("99.0.0"));
        assert_eq!(tool.run(&query).unwrap()["plugins"][0]["version"], "3.0.5");
        let create = input(json!({"action":"create","plugin":"valhalla-delay","version":"3.0.5","output":"new.vstpreset"}));
        assert!(tool.run(&create).unwrap_err().contains("no verified writer"));
        let refreshed = tool.run(&input(json!({"action":"installed","refresh":true}))).unwrap();
        assert_eq!(refreshed["plugins"][0]["version"], "99.0.0");
        installation(temp.path(), None);
        assert!(tool.run(&create).is_err());
        assert!(!tool.output.exists());
    }

    #[test]
    fn paths_stay_local_and_outputs_stay_in_the_owned_folder() {
        let temp = tempfile::tempdir().unwrap();
        let tool = tool(temp.path());
        for path in [r"\\server\share\x.vstpreset", "//server/share/x", r"\??\C:\x", r"\\.\pipe\x"] {
            assert!(read_file(Path::new(path), "serum-2").unwrap_err().contains("local path"));
            assert!(tool.run(&input(json!({"action":"installed","folders":[path]}))).is_err());
        }
        for name in ["../escape.vstpreset", "CON.vstpreset", "NUL.vstpreset", "LPT1.vstpreset", "x/escape.vstpreset", "x.vstpreset."] {
            assert!(tool.output(name, "vstpreset").is_err(), "{name}");
        }
        assert!(read_file(temp.path(), "serum-2").unwrap_err().contains("regular file"));
        assert!(!tool.output.exists());
        let all = tool.run(&input(json!({"action":"catalog"}))).unwrap();
        assert_eq!(all["plugins"].as_array().unwrap().len(), 20);
        assert!(tool.deferred());
    }

    #[test]
    fn input_paths_refuse_controls_and_device_components_before_opening() {
        for path in [r"C:\x\COM3.vstpreset", r"C:\x\NUL .vstpreset", "\0\\\\host\\share\\x.vstpreset\0", "/tmp/a\n.vital"] {
            assert!(read_file(Path::new(path), "vital").unwrap_err().contains("local path"), "{path:?}");
        }
        #[cfg(windows)]
        assert!(read_file(Path::new("/tmp/LPT¹.xml"), "vital").unwrap_err().contains("local path"));
    }

    #[cfg(unix)]
    #[test]
    fn unix_preset_paths_can_use_aux_folders_and_colon_filenames() {
        let temp = tempfile::tempdir().unwrap();
        let folder = temp.path().join("Aux");
        std::fs::create_dir(&folder).unwrap();
        let path = folder.join("A: Bright Pad.vital");
        let bytes = br#"{"synth_version":"1.0.7","settings":{"osc_1_level":1}}"#;
        std::fs::write(&path, bytes).unwrap();
        assert_eq!(read_file(&path, "vital").unwrap().as_slice(), bytes);
        let result = tool(temp.path()).run(&input(json!({"action":"inspect","plugin":"vital","input":path}))).unwrap();
        assert!(!result["fields"].as_array().unwrap().is_empty());
    }

    #[test]
    fn duplicate_installs_of_the_same_verified_release_allow_writes_but_conflicts_do_not() {
        let temp = tempfile::tempdir().unwrap();
        installation(&temp.path().join("one"), Some("3.0.5"));
        let second = temp.path().join("two");
        installation(&second, Some("3.0.5"));
        let tool = tool(temp.path());
        let make = input(json!({"action":"create","plugin":"valhalla-delay","output":"same.vstpreset"}));
        assert!(tool.run(&make).is_ok());
        installation(&second, Some("99.0.0"));
        let make = input(json!({"action":"create","plugin":"valhalla-delay","output":"different.vstpreset"}));
        assert!(tool.run(&make).unwrap_err().contains("no verified writer"));
        assert!(!tool.output.join("different.vstpreset").exists());
    }

    #[test]
    fn vital_embedded_audio_can_exceed_two_mebibytes_while_tool_output_stays_bounded() {
        let temp = tempfile::tempdir().unwrap();
        let file = temp.path().join("sample.vital");
        let bytes =
            serde_json::to_vec(&json!({"synth_version":"1.0.7", "settings":{"sample":{"samples":"A".repeat(MAX_DECODED_BYTES + 16)}}}))
                .unwrap();
        std::fs::write(&file, &bytes).unwrap();
        assert_eq!(read_file(&file, "vital").unwrap().len(), bytes.len());
        assert!(read_file(&file, "serum-2").unwrap_err().contains("size limit"));
        let reply =
            tool(temp.path()).run(&input(json!({"action":"inspect","plugin":"vital","input":file,"query":"samples","limit":1}))).unwrap();
        assert_eq!(reply["fields"][0]["value"].as_str().unwrap().len(), 4096);
        assert_eq!(reply["fields"][0]["truncated"], true);
    }
}
