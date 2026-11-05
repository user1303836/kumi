//! What an agent host's plugin may change in a DAW (Kumi for Hermes, C2): an allowlist of the DAW's change steps, each
//! with the values of its input it refuses, and the checks every step's input gets wherever in it they are: no file or
//! folder by path, no raw preset or state data, no code, no action that deletes. A DAW's crate gives its list
//! ([`crate::provider::DawProvider::plugin_steps`]); a step that isn't on it is refused, so a step a DAW adds stays
//! refused until its list says otherwise.

use serde_json::{Map, Value};
use std::borrow::Cow;

/// A change step a plugin's plan may use on a DAW.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PluginStep {
    /// The step's name in the DAW's plan tool ("track_volume").
    pub tool: &'static str,
    /// Its HISTORY family ("mixer", "device"…), the family a preview gives (CC-3).
    pub family: &'static str,
    /// The field that takes an audio file, when it takes one: the import step puts there the file a source names, so a
    /// plan gives a source (an object) in it, never a path.
    pub file_field: Option<&'static str>,
    /// What a file step does as the plugin offers it: the DAW's own description says its field takes a path, which a
    /// plugin's plan never gives.
    pub described: Option<&'static str>,
    /// The field that names something in the DAW's own browser by a path of names ("Audio/Effects/Native/Delay"), when
    /// it has one: allowed though it's called `path`, its value still never a file's path.
    pub browser_field: Option<&'static str>,
    /// Values of its input it refuses, by field name, wherever in the input the field is: `("action", &["delete-return"])`.
    /// A true or false, or a number, is matched by how JSON writes it (`("clear", &["true"])`); `"*"` refuses the field
    /// whatever it holds (`("remove", &["*"])`).
    pub refused: &'static [(&'static str, &'static [&'static str])],
    /// What the DAW's instructions teach about its input (Live's notation), which a plugin's host reads with the step
    /// since it doesn't have Kumi's instructions.
    pub noting: Option<&'static str>,
    /// Whether it's a change only the DAW's own Undo takes back (CC-20).
    pub daw_undo: DawUndo,
    /// Why Kumi doesn't undo such a change, when the step has its own reason: said after CC-20's words
    /// ([`DAW_UNDO_NOTE`]), as "Kumi can't yet tell whether the pad was changed since."
    pub daw_undo_because: Option<&'static str>,
}

/// When a step is a change only the DAW's own Undo takes back (CC-20): it runs though the DAW keeps no undo Kumi can
/// use for it, and Kumi undoes nothing of the change it's in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DawUndo {
    /// Never: Kumi undoes it, or it's refused where the DAW keeps no undo for it.
    Never,
    /// Whatever its input holds.
    Always,
    /// When a field at the top of its input (where the step's own action is) holds one of these values, whatever its
    /// case and the spaces around it: `("action", "add-chain")`.
    When(&'static [(&'static str, &'static str)]),
}

/// What Kumi says of a change only the DAW's own Undo takes back, the same in its approval, its rows and the refusal of
/// its undo (CC-20's words): `{app}` is the DAW's name, `{key}` its Undo key on the machine Kumi runs on.
pub const DAW_UNDO_NOTE: &str =
    "Kumi can't undo this change. {app}'s own Undo ({key}) takes all of it back while it's still {app}'s latest step.";

/// CC-20's words, or a step's reason, for `app` on the system `os` names (`std::env::consts::OS`), whose Undo key is
/// Cmd+Z on macOS and Ctrl+Z elsewhere.
pub fn daw_undo_said(note: &str, app: &str, os: &str) -> String {
    let key = if os == "macos" { "Cmd+Z" } else { "Ctrl+Z" };
    note.replace("{app}", app).replace("{key}", key)
}

impl PluginStep {
    pub const fn new(tool: &'static str, family: &'static str) -> PluginStep {
        PluginStep {
            tool,
            family,
            file_field: None,
            described: None,
            browser_field: None,
            refused: &[],
            noting: None,
            daw_undo: DawUndo::Never,
            daw_undo_because: None,
        }
    }

    /// It takes an audio file in `field`, which the import step fills from a source; `described` is what it does as the
    /// plugin offers it, with `field` taking a source.
    pub const fn file(self, field: &'static str, described: &'static str) -> PluginStep {
        PluginStep { file_field: Some(field), described: Some(described), ..self }
    }

    /// Its description as the plugin gives it: its own for a file step, the DAW's (`native`) otherwise; then what the
    /// DAW's instructions teach about its input.
    pub fn description<'a>(&self, native: &'a str) -> Cow<'a, str> {
        let said = self.described.unwrap_or(native);
        match self.noting {
            Some(more) => Cow::Owned(format!("{said}\n\nFrom Kumi's instructions: {more}")),
            None => Cow::Borrowed(said),
        }
    }

    /// It names something in the DAW's browser in `field`, by a path of names.
    pub const fn browser(self, field: &'static str) -> PluginStep {
        PluginStep { browser_field: Some(field), ..self }
    }

    /// It refuses these values of these fields.
    pub const fn refusing(self, refused: &'static [(&'static str, &'static [&'static str])]) -> PluginStep {
        PluginStep { refused, ..self }
    }

    /// Its input as the DAW's instructions teach it, after its description.
    pub const fn noting(self, more: &'static str) -> PluginStep {
        PluginStep { noting: Some(more), ..self }
    }

    /// These values of these fields make it a change only the DAW's own Undo takes back.
    pub const fn daw_undo(self, when: &'static [(&'static str, &'static str)]) -> PluginStep {
        PluginStep { daw_undo: DawUndo::When(when), ..self }
    }

    /// It's a change only the DAW's own Undo takes back, whatever its input.
    pub const fn daw_undo_always(self) -> PluginStep {
        PluginStep { daw_undo: DawUndo::Always, ..self }
    }

    /// Kumi says `why` after CC-20's words of such a change it's in.
    pub const fn daw_undo_because(self, why: &'static str) -> PluginStep {
        PluginStep { daw_undo_because: Some(why), ..self }
    }

    /// Whether `input` makes it a change only the DAW's own Undo takes back.
    pub fn daw_undo_only(&self, input: &Value) -> bool {
        let marked = |(field, value): &(&str, &str)| {
            input.get(*field).and_then(Value::as_str).is_some_and(|text| value.eq_ignore_ascii_case(text.trim()))
        };
        match self.daw_undo {
            DawUndo::Never => false,
            DawUndo::Always => true,
            DawUndo::When(marks) => marks.iter().any(marked),
        }
    }
}

/// Fields no step's input holds, wherever they are (compared lower case, without `_` or `-`): a file or folder by
/// path, raw preset or state data, code.
const NEVER: [&str; 15] = [
    "path",
    "paths",
    "file",
    "files",
    "filename",
    "folder",
    "directory",
    "url",
    "uri",
    "xml",
    "presetdata",
    "chunk",
    "code",
    "script",
    "lua",
];

/// Fields that say what a step does, whose values are checked for an action that deletes, saves, loads or records.
const ACTIONS: [&str; 4] = ["action", "op", "operation", "mode"];

/// Words in such a value that refuse it.
const DESTROYS: [&str; 18] = [
    "delete",
    "remove",
    "clear",
    "erase",
    "purge",
    "drop",
    "glue",
    "render",
    "freeze",
    "flatten",
    "consolidate",
    "crop",
    "save",
    "export",
    "load",
    "import",
    "record",
    "permanent",
];

/// The step `tool` is on the allowlist and its `input` holds nothing refused; or why not. The import step's file field
/// must hold a source of CC-11's shape (a sound from the map, a file Kumi made, or an approved file with its
/// provenance), never a path and never anything else (an integration may read another object as a search of folders).
pub fn check<'a>(steps: &'a [PluginStep], tool: &str, input: &Value) -> Result<&'a PluginStep, String> {
    let Some(step) = steps.iter().find(|step| step.tool == tool) else {
        return Err(format!("{tool} isn't a change Kumi for Hermes makes in this DAW."));
    };
    let Some(fields) = input.as_object() else { return Ok(step) };
    for (field, value) in fields {
        if step.file_field == Some(field.as_str()) {
            if !source(value) {
                return Err(format!(
                    "{tool}'s {field} takes a source ({{sound_id, map_revision}}, {{artifact_id}} or {{file, provenance}}), never a path or anything else."
                ));
            }
            continue;
        }
        walk(step, field, value)?;
    }
    Ok(step)
}

/// A source of CC-11's shape, with nothing more: `{sound_id, map_revision}`, `{artifact_id}`, or `{file, provenance:
/// {host_tool, tool_call_id?}}`, each a string.
fn source(value: &Value) -> bool {
    let Some(fields) = value.as_object() else { return false };
    let strings = |names: &[&str]| fields.len() == names.len() && names.iter().all(|name| fields.get(*name).is_some_and(Value::is_string));
    let provenance = || {
        fields.get("provenance").and_then(Value::as_object).is_some_and(|said| {
            said.get("host_tool").is_some_and(Value::is_string)
                && said.iter().all(|(name, value)| matches!(name.as_str(), "host_tool" | "tool_call_id") && value.is_string())
        })
    };
    strings(&["sound_id", "map_revision"])
        || strings(&["artifact_id"])
        || (fields.len() == 2 && fields.get("file").is_some_and(Value::is_string) && provenance())
}

fn walk(step: &PluginStep, field: &str, value: &Value) -> Result<(), String> {
    let plain = field.to_ascii_lowercase().replace(['_', '-'], "");
    if NEVER.contains(&plain.as_str()) && step.browser_field != Some(field) {
        return Err(format!("{}'s {field} names a file or holds raw data or code, which Kumi for Hermes doesn't change.", step.tool));
    }
    let refused = |text: &str| step.refused.iter().any(|(name, values)| *name == field && values.contains(&text));
    if refused("*") {
        return Err(format!("{}'s {field} deletes, or can't be undone, which Kumi for Hermes doesn't do.", step.tool));
    }
    match value {
        Value::String(text) => {
            let lower = text.to_ascii_lowercase();
            if refused(text) || (ACTIONS.contains(&plain.as_str()) && DESTROYS.iter().any(|word| lower.contains(word))) {
                return Err(format!("{}'s {field} {text} deletes, or can't be undone, which Kumi for Hermes doesn't do.", step.tool));
            }
            if path_like(text) || lower.starts_with("<?xml") {
                return Err(format!("{}'s {field} names a file or holds raw data, which Kumi for Hermes doesn't change.", step.tool));
            }
        }
        Value::Bool(_) | Value::Number(_) if refused(&value.to_string()) => {
            return Err(format!("{}'s {field} {value} deletes, or can't be undone, which Kumi for Hermes doesn't do.", step.tool));
        }
        Value::Array(items) => {
            for item in items {
                walk(step, field, item)?;
            }
        }
        Value::Object(fields) => {
            for (inner, value) in fields {
                walk(step, inner, value)?;
            }
        }
        _ => {}
    }
    Ok(())
}

/// A step's input schema as a plugin's plan fills it: its file field takes `source` (an object naming where the file
/// comes from) in place of a path, and the fields no step may hold, and those this step refuses whatever they hold, are
/// left out wherever they are (and out of `required`).
pub fn plugin_schema(step: &PluginStep, schema: &Map<String, Value>, source: &Value) -> Map<String, Value> {
    let mut schema = schema.clone();
    prune(step, &mut schema, true);
    if let (Some(field), Some(Value::Object(properties))) = (step.file_field, schema.get_mut("properties")) {
        if properties.contains_key(field) {
            properties.insert(field.to_string(), source.clone());
        }
    }
    schema
}

/// `schema` without the fields no step may hold, nor those `step` refuses whatever they hold, at every depth (`top`: the
/// step's own fields, where its file and browser fields are its own to keep).
fn prune(step: &PluginStep, schema: &mut Map<String, Value>, top: bool) {
    let mut gone = Vec::new();
    if let Some(Value::Object(properties)) = schema.get_mut("properties") {
        properties.retain(|field, _| {
            let plain = field.to_ascii_lowercase().replace(['_', '-'], "");
            let own = top && (step.file_field == Some(field.as_str()) || step.browser_field == Some(field.as_str()));
            let refused = step.refused.iter().any(|(name, values)| *name == field.as_str() && values.contains(&"*"));
            let kept = own || !(NEVER.contains(&plain.as_str()) || refused);
            if !kept {
                gone.push(field.clone());
            }
            kept
        });
        for inner in properties.values_mut().filter_map(Value::as_object_mut) {
            prune(step, inner, false);
        }
    }
    if let Some(Value::Object(items)) = schema.get_mut("items") {
        prune(step, items, false);
    }
    if let Some(Value::Array(required)) = schema.get_mut("required") {
        required.retain(|field| !field.as_str().is_some_and(|field| gone.iter().any(|left| left == field)));
    }
}

/// A file or folder by its path or URL: absolute, home-relative, a drive's, a share's, or a `file:` or `library:` one.
fn path_like(text: &str) -> bool {
    let bytes = text.as_bytes();
    let drive = bytes.len() > 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' && (bytes[2] == b'\\' || bytes[2] == b'/');
    drive
        || text.starts_with('/')
        || text.starts_with("~/")
        || text.starts_with("~\\")
        || text.starts_with("\\\\")
        || ["file:", "library:", "smb:", "http:", "https:"]
            .iter()
            .any(|scheme| text.get(..scheme.len()).is_some_and(|start| start.eq_ignore_ascii_case(scheme)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const STEPS: [PluginStep; 3] = [
        PluginStep::new("track_volume", "mixer"),
        PluginStep::new("change_structure", "structure").refusing(&[("action", &["delete-return"])]),
        PluginStep::new("replace_sample", "device").file("sample", "Swap the sample in a Simpler: sample, a source."),
    ];

    #[test]
    fn only_a_listed_step_runs() {
        assert_eq!(check(&STEPS, "track_volume", &json!({"ref": "7:track:1", "volumeDb": -6})).unwrap().family, "mixer");
        for unlisted in ["wait", "run_lua", "take_add"] {
            assert!(check(&STEPS, unlisted, &json!({})).is_err(), "{unlisted}");
        }
    }

    #[test]
    fn a_refused_value_or_an_action_that_destroys_is_refused_wherever_it_is() {
        assert!(check(&STEPS, "change_structure", &json!({"action": "add-return"})).is_ok());
        assert!(check(&STEPS, "change_structure", &json!({"action": "delete-return"})).is_err());
        assert!(check(&STEPS, "change_structure", &json!({"edits": [{"action": "delete-return"}]})).is_err(), "inside a list");
        assert!(check(&STEPS, "change_structure", &json!({"op": "clear-envelope"})).is_err(), "an action that destroys");
        assert!(check(&STEPS, "change_structure", &json!({"name": "Clear vocals"})).is_ok(), "only what a step does is read so");
    }

    #[test]
    fn a_marked_action_is_a_change_only_the_daws_undo_takes_back() {
        let rack = PluginStep::new("edit_rack", "device").refusing(&[("action", &["insert-chain"])]).daw_undo(&[("action", "add-chain")]);
        for marked in [json!({"rackRef": "d1", "action": "add-chain"}), json!({"action": " Add-Chain "})] {
            assert!(rack.daw_undo_only(&marked), "{marked}");
            assert!(check(std::slice::from_ref(&rack), "edit_rack", &marked).is_ok(), "{marked}");
        }
        // Only the step's own action marks it, and a refused one stays refused.
        assert!(!rack.daw_undo_only(&json!({"action": "rename-chain"})));
        assert!(!rack.daw_undo_only(&json!({"edits": [{"action": "add-chain"}]})), "only at the top");
        assert!(!rack.daw_undo_only(&json!({"action": "insert-chain"})));
        assert!(check(std::slice::from_ref(&rack), "edit_rack", &json!({"action": "insert-chain"})).is_err());
        assert!(!PluginStep::new("set_mixer", "mixer").daw_undo_only(&json!({"action": "add-chain"})), "a step marks only its own");
        // A step marked whatever its input is marked so for any input, and may give its reason.
        let pad = PluginStep::new("load_sample_to_pad", "device").daw_undo_always().daw_undo_because("Kumi can't tell.");
        assert!(pad.daw_undo_only(&json!({})) && pad.daw_undo_only(&json!({"padRef": "p1"})));
        assert_eq!(
            daw_undo_said(DAW_UNDO_NOTE, "Live", "macos"),
            "Kumi can't undo this change. Live's own Undo (Cmd+Z) takes all of it back while it's still Live's latest step."
        );
        assert!(daw_undo_said(DAW_UNDO_NOTE, "Live", "windows").contains("Live's own Undo (Ctrl+Z)"));
    }

    #[test]
    fn no_step_names_a_file_or_holds_raw_data_or_code_anywhere() {
        for input in [
            json!({"ref": "a", "path": "x.wav"}),
            json!({"ref": "a", "options": {"presetData": "<x/>"}}),
            json!({"ref": "a", "preset_data": "abc"}),
            json!({"ref": "a", "name": "/Users/me/Samples/kick.wav"}),
            json!({"ref": "a", "name": "C:\\Samples\\kick.wav"}),
            json!({"ref": "a", "name": "~/Samples/kick.wav"}),
            json!({"ref": "a", "name": "library:Drums/kick.wav"}),
            json!({"ref": "a", "names": ["kick", "\\\\server\\share\\kick.wav"]}),
            json!({"ref": "a", "data": "<?xml version=\"1.0\"?><preset/>"}),
            json!({"ref": "a", "Lua": "renoise.song()"}),
        ] {
            assert!(check(&STEPS, "track_volume", &input).is_err(), "{input}");
        }
        assert!(check(&STEPS, "track_volume", &json!({"ref": "{4F2A}", "name": "Kick 2: tight", "color": "#ff0000"})).is_ok());
    }

    #[test]
    fn a_browser_name_may_be_called_path_but_a_true_or_a_number_can_be_refused() {
        const OTHERS: [PluginStep; 2] = [
            PluginStep::new("device_add", "device").browser("path"),
            PluginStep::new("notes_write", "notes").refusing(&[("clear", &["true"])]),
        ];
        assert!(check(&OTHERS, "device_add", &json!({"ref": "t1", "path": "Audio/Effects/Native/Delay"})).is_ok());
        assert!(check(&OTHERS, "device_add", &json!({"ref": "t1", "path": "/Users/me/Delay.xrdp"})).is_err(), "never a file");
        assert!(check(&OTHERS, "device_add", &json!({"ref": "t1", "options": {"file": "x"}})).is_err(), "only that field");
        assert!(check(&OTHERS, "notes_write", &json!({"ref": "p1", "clear": false})).is_ok());
        assert!(check(&OTHERS, "notes_write", &json!({"ref": "p1", "clear": true})).is_err());
    }

    #[test]
    fn a_plugins_schema_takes_a_source_in_the_file_field_and_holds_no_path_nor_a_refused_field() {
        let schema = json!({"type": "object", "required": ["deviceRef", "sample"], "properties": {
            "deviceRef": {"type": "string"}, "sample": {"type": "string"}, "path": {"type": "string"}
        }});
        let source = json!({"type": "object", "description": "a source"});
        let shown = plugin_schema(&STEPS[2], schema.as_object().unwrap(), &source);
        assert_eq!(shown["properties"], json!({"deviceRef": {"type": "string"}, "sample": {"type": "object", "description": "a source"}}));
        assert_eq!(shown["required"], json!(["deviceRef", "sample"]));
        let required = json!({"type": "object", "required": ["ref", "path"], "properties": {"ref": {}, "path": {}}});
        assert_eq!(plugin_schema(&STEPS[0], required.as_object().unwrap(), &source)["required"], json!(["ref"]));
        let nested = json!({"type": "object", "properties": {"options": {"type": "object", "required": ["presetData"], "properties": {
            "presetData": {"type": "string"}, "amount": {"type": "number"}
        }}, "rows": {"type": "array", "items": {"type": "object", "properties": {"path": {}, "name": {}}}}}});
        let shown = plugin_schema(&STEPS[0], nested.as_object().unwrap(), &source);
        assert_eq!(shown["properties"]["options"], json!({"type": "object", "required": [], "properties": {"amount": {"type": "number"}}}));
        assert_eq!(shown["properties"]["rows"]["items"]["properties"], json!({"name": {}}));
        // A field the step refuses whatever it holds isn't shown, at any depth; one it refuses only some values of is.
        let lines = json!({"type": "object", "properties": {"clear": {"type": "boolean"}, "lines": {"type": "array", "items": {
            "type": "object", "required": ["line", "clear"], "properties": {"line": {}, "clear": {}}
        }}}});
        let whole = PluginStep::new("notes_write", "clip").refusing(&[("clear", &["*"])]);
        assert_eq!(
            plugin_schema(&whole, lines.as_object().unwrap(), &source)["properties"],
            json!({"lines": {"type": "array", "items": {"type": "object", "required": ["line"], "properties": {"line": {}}}}})
        );
        let some = PluginStep::new("notes_write", "clip").refusing(&[("clear", &["true"])]);
        assert!(plugin_schema(&some, lines.as_object().unwrap(), &source)["properties"].get("clear").is_some());
    }

    #[test]
    fn the_file_field_takes_a_source_never_a_path() {
        let source = json!({"deviceRef": "7:device:0:0", "sample": {"sound_id": "s-r7-1", "map_revision": "r7"}});
        assert!(check(&STEPS, "replace_sample", &source).is_ok());
        assert!(check(&STEPS, "replace_sample", &json!({"deviceRef": "7:device:0:0", "sample": "/Users/me/kick.wav"})).is_err());
        for other in [
            json!({"folders": ["/Users/x/Private"], "random": true}),
            json!({"sound_id": "s-r7-1", "map_revision": "r7", "random": true}),
            json!({"file": "/x.wav", "provenance": {"host_tool": "t", "folders": ["/"]}}),
            json!({"file": "/x.wav"}),
        ] {
            let input = json!({"deviceRef": "7:device:0:0", "sample": other});
            assert!(check(&STEPS, "replace_sample", &input).is_err(), "{input}");
        }
        let file = json!({"file": "/media/a.mp3", "provenance": {"host_tool": "elevenlabs_sound_effect", "tool_call_id": "c1"}});
        assert!(check(&STEPS, "replace_sample", &json!({"deviceRef": "7:device:0:0", "sample": file})).is_ok());
        assert!(check(&STEPS, "track_volume", &json!({"ref": "a", "sample": {"file": "/x.wav"}})).is_err(), "only its own file field");
    }
}
