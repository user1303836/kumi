//! A bounded, readable receipt for a new verified preset. Values come back from the codec, not the request.

use super::{create, edit, inspect, Edit, WriteOptions, MAX_EDITS};
use crate::plugins::formats::{
    serum2_modulation::{self, Route},
    structure::{self, Field, PluginFormat, Status, Structure},
    tree::PathRule,
    write_value, FormatError,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

pub const PLUGINS: &[&str] = &["serum-2", "fabfilter-pro-q-4", "ozone-12"];
pub const WRITER: &str = "kumi-vstpreset-1";
/// Room for every setting's line at the shared edit cap; a receipt past it is refused, never cut.
pub const MAX_REVIEW_CHARS: usize = MAX_EDITS * 160;
pub const VALUE_NOTE: &str = "Values as the preset file stores them; the plug-in may show them differently.";

#[derive(Debug, thiserror::Error)]
pub enum ReviewError {
    #[error("{0}")]
    Invalid(#[from] FormatError),
    #[error("The complete preset review is too long; use at most {max} changes and shorter values.", max = MAX_EDITS)]
    TooLong,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Setting {
    pub path: String,
    pub name: String,
    pub before: Value,
    pub after: Value,
    pub unit: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub range: Option<[f64; 2]>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PresetMetadata {
    pub writer: String,
    pub plugin: String,
    pub class: String,
    pub version: String,
    pub format: String,
    pub digest: String,
    pub name: String,
    pub settings: Vec<Setting>,
}

#[derive(Debug)]
pub struct ReviewedPreset {
    pub metadata: PresetMetadata,
    pub bytes: Vec<u8>,
}

/// The caller supplies an installed, independently read version in `options`; a requested version is not evidence.
pub fn build(plugin: &str, options: &WriteOptions, edits: &[Edit]) -> Result<ReviewedPreset, ReviewError> {
    let format = supported(plugin)?;
    let name = options.name.as_deref().ok_or_else(|| fail("Give the preset a name."))?;
    display_name(name)?;
    if edits.len() > MAX_EDITS {
        return Err(ReviewError::TooLong);
    }
    let mut paths = Vec::with_capacity(edits.len());
    for change in edits {
        if change.path.len() > 256 || change.path.chars().any(hidden) || change.path == "presetName" {
            return Err(fail("Use a catalog control path, and the separate name field for the preset's name."));
        }
        let path = review_path(plugin, change)?;
        if !paths.contains(&path) {
            paths.push(path);
        }
    }
    let initial = create(plugin, options, &[])?;
    let class = initial.class_id.to_uppercase();
    let initial = initial.write();
    let before = inspect(plugin, &initial, Some(&class))?;
    let bytes = if edits.is_empty() { initial } else { edit(plugin, &initial, options, edits)?.write() };
    let after = inspect(plugin, &bytes, Some(&class))?;
    let mut settings = Vec::with_capacity(paths.len());
    for path in paths {
        let (name, unit, range, field) = descriptor(format, &path)?;
        let before = read_value(plugin, &before["data"], &path, field, true)?;
        let after = read_value(plugin, &after["data"], &path, field, false)?;
        if before != after {
            settings.push(Setting { path, name, before, after, unit, range });
        }
    }
    let metadata = PresetMetadata {
        writer: WRITER.into(),
        plugin: plugin.into(),
        class,
        version: options.version.clone(),
        format: "vstpreset".into(),
        digest: hex::encode(Sha256::digest(&bytes)),
        name: name.into(),
        settings,
    };
    metadata.validate()?;
    Ok(ReviewedPreset { metadata, bytes })
}

impl PresetMetadata {
    /// Validate a retained receipt without granting a new writer or interpreting stored values as physical units.
    pub fn validate(&self) -> Result<(), ReviewError> {
        let format = supported(&self.plugin)?;
        if self.writer != WRITER
            || self.format != "vstpreset"
            || self.class.len() != 32
            || !self.class.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_lowercase())
            || !format.info.ids.vst3.iter().any(|class| class.id.eq_ignore_ascii_case(&self.class))
            || self.digest.len() != 64
            || !self.digest.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
        {
            return Err(fail("This isn't a receipt from Kumi's verified preset writer."));
        }
        super::writable(format, &self.version)?;
        display_name(&self.name)?;
        if self.settings.len() > MAX_EDITS {
            return Err(ReviewError::TooLong);
        }
        let mut seen = std::collections::BTreeSet::new();
        let mut characters =
            self.name.chars().count() + format.info.name.chars().count() + self.version.chars().count() + VALUE_NOTE.chars().count();
        for setting in &self.settings {
            if setting.path.len() > 256 || !seen.insert(setting.path.as_str()) {
                return Err(fail("The preset receipt has an invalid or repeated control path."));
            }
            let (name, unit, range, _) = descriptor(format, &setting.path)?;
            if setting.name != name
                || setting.unit != unit
                || setting.range != range
                || !bounded_value(&setting.before)
                || !bounded_value(&setting.after)
            {
                return Err(fail("The preset receipt's settings don't match the built-in control catalog."));
            }
            characters += setting.line().chars().count() + 1;
        }
        if characters > MAX_REVIEW_CHARS {
            return Err(ReviewError::TooLong);
        }
        Ok(())
    }

    /// Check the retained bytes against the receipt and re-read every reported final value from that same byte buffer.
    pub fn verify(&self, bytes: &[u8]) -> Result<(), ReviewError> {
        self.validate()?;
        if hex::encode(Sha256::digest(bytes)) != self.digest {
            return Err(fail("The preset bytes changed since Kumi built them."));
        }
        let actual = inspect(&self.plugin, bytes, Some(&self.class))?;
        let format = supported(&self.plugin)?;
        for setting in &self.settings {
            let (_, _, _, field) = descriptor(format, &setting.path)?;
            if read_value(&self.plugin, &actual["data"], &setting.path, field, false)? != setting.after {
                return Err(fail("A reported preset setting differs from the file."));
            }
        }
        Ok(())
    }
}

impl Setting {
    pub fn line(&self) -> String {
        let range = self.range.map(|[low, high]| format!(", {low}–{high}")).unwrap_or_default();
        format!("{}: {} → {}, {}{}", self.name, said(&self.before), said(&self.after), self.unit, range)
    }
}

fn said(value: &Value) -> String {
    value.as_str().map(str::to_owned).unwrap_or_else(|| value.to_string())
}
fn fail(message: &str) -> ReviewError {
    ReviewError::Invalid(FormatError::new(message))
}
fn supported(plugin: &str) -> Result<&'static PluginFormat, ReviewError> {
    if !PLUGINS.contains(&plugin) {
        return Err(fail("Preset building supports the verified Serum 2, Pro-Q 4 and Ozone 12 writers, not this plug-in."));
    }
    structure::builtin(plugin).ok_or_else(|| fail("The plug-in's built-in format is missing."))
}
fn display_name(name: &str) -> Result<(), ReviewError> {
    write_value::validate_display_name(name)?;
    if name.chars().count() > 80 || name.chars().any(hidden) {
        return Err(fail("Use a preset name of at most 80 visible characters, without hidden or direction-changing characters."));
    }
    Ok(())
}
fn hidden(character: char) -> bool {
    character.is_control()
        || matches!(character, '\u{00ad}' | '\u{061c}' | '\u{180e}' | '\u{200b}'..='\u{200f}'
        | '\u{2028}'..='\u{202e}' | '\u{2060}'..='\u{206f}' | '\u{feff}')
}
fn bounded_value(value: &Value) -> bool {
    match value {
        Value::Bool(_) => true,
        Value::Number(value) => value.as_f64().is_some_and(f64::is_finite),
        Value::String(value) => value.chars().count() <= 80 && !value.chars().any(hidden),
        Value::Array(values) => {
            values.len() <= 16
                && values.iter().all(|value| value.as_str().is_some_and(|value| value.chars().count() <= 40 && !value.chars().any(hidden)))
        }
        _ => false,
    }
}
fn review_path(plugin: &str, change: &Edit) -> Result<String, ReviewError> {
    if plugin == "serum-2" && change.path == "$add_distortion" {
        let rack = change.value.as_u64().filter(|rack| *rack <= 2).ok_or_else(|| fail("$add_distortion takes rack 0, 1 or 2."))?;
        return Ok(format!("FXRack{rack}/FX"));
    }
    if plugin == "serum-2" && change.path == serum2_modulation::EDIT {
        return Ok(Route::parse(&change.value)?.section());
    }
    if plugin == "ozone-12" {
        if change.path == "$chain" {
            return Ok(change.path.clone());
        }
        let path =
            change.path.strip_prefix("DSP State/DSP Elements/").or_else(|| change.path.strip_prefix("parameters/")).unwrap_or(&change.path);
        return Ok(format!("parameters/{path}"));
    }
    Ok(change.path.trim_start_matches('/').to_owned())
}
fn rack(path: &str) -> Option<usize> {
    path.strip_prefix("FXRack")?.strip_suffix("/FX")?.parse::<usize>().ok().filter(|rack| *rack <= 2)
}
/// "ModSlot0" → slot 1: the section a whole-slot route is reviewed as.
fn mod_slot(path: &str) -> Option<usize> {
    let slot = path.strip_prefix("ModSlot").filter(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))?.parse::<usize>().ok()? + 1;
    serum2_modulation::matrix().ok().filter(|matrix| slot <= matrix.slots as usize).map(|_| slot)
}
type Description<'a> = (String, String, Option<[f64; 2]>, Option<&'a Field>);
fn descriptor<'a>(format: &'a PluginFormat, path: &str) -> Result<Description<'a>, ReviewError> {
    if format.info.plugin == "serum-2" {
        if let Some(rack) = rack(path) {
            return Ok((format!("FX rack {} effects", rack + 1), "modules".into(), None, None));
        }
        if let Some(slot) = mod_slot(path) {
            return Ok((format!("Mod {slot}"), "routing".into(), None, None));
        }
    }
    if format.info.plugin == "ozone-12" && path == "$chain" {
        return Ok(("Module order".into(), "modules".into(), None, None));
    }
    let canonical = if format.info.plugin == "serum-2" {
        let mut canonical = String::new();
        let mut parts = path.split('/').peekable();
        while let Some(part) = parts.next() {
            let segment = PathRule::NumberedSections.segment(&canonical, part, parts.peek().is_some());
            if !canonical.is_empty() {
                canonical.push('/');
            }
            canonical.push_str(&segment);
        }
        canonical
    } else if format.info.plugin == "ozone-12" {
        format!("DSP State/DSP Elements/{}", path.strip_prefix("parameters/").ok_or_else(|| fail("Use an Ozone control path."))?)
    } else {
        path.to_owned()
    };
    let field = format.structure.field(&canonical).ok_or_else(|| fail("This control has no built-in preset description."))?;
    let name = format
        .structure
        .parameters
        .iter()
        .find(|(_, link)| link.status == Status::Verified && link.field == canonical && Structure::location(link) == path)
        .map(|(name, _)| name.clone())
        .unwrap_or_else(|| path.strip_prefix("parameters/").unwrap_or(path).to_owned());
    let range = field.range.filter(|_| field.range_status == Some(Status::Verified));
    let unit = field.unit.as_deref().filter(|_| field.status == Status::Verified).unwrap_or("stored value");
    Ok((name, unit.to_owned(), range, Some(field)))
}
fn read_value(plugin: &str, data: &Value, path: &str, field: Option<&Field>, from_initial: bool) -> Result<Value, ReviewError> {
    if plugin == "ozone-12" && path == "$chain" {
        // The initialized suite may omit ElementChain. That isn't a measured empty chain.
        if from_initial && data.get("chain").is_some_and(Value::is_null) {
            return Ok(json!("not stored in template"));
        }
        return data
            .get("chain")
            .filter(|value| bounded_value(value))
            .cloned()
            .ok_or_else(|| fail("The Ozone module order couldn't be read."));
    }
    if plugin == "serum-2" && mod_slot(path).is_some() {
        return Ok(json!(serum2_modulation::describe(data.get(path))?));
    }
    let mut value = data;
    for part in path.split('/') {
        match value.get(part).or_else(|| part.parse::<usize>().ok().and_then(|index| value.get(index))) {
            Some(next) => value = next,
            None if from_initial && plugin == "serum-2" && field.is_some() => {
                // Serum omits defaults from its initialized state; don't invent their displayed or stored number.
                return Ok(json!("not stored in template"));
            }
            None => return Err(fail("A reported control is missing from the decoded preset.")),
        }
    }
    if plugin == "serum-2" && rack(path).is_some() {
        let slots = value.as_array().ok_or_else(|| fail("The Serum FX rack couldn't be read."))?;
        if slots.len() > 16 || slots.iter().any(|slot| slot.get("FXDistortion").is_none()) {
            return Err(fail("This Serum FX rack contains an effect outside the initialized Distortion writer."));
        }
        return Ok(Value::Array(slots.iter().map(|_| json!("Distortion")).collect()));
    }
    if bounded_value(value) {
        Ok(value.clone())
    } else {
        Err(fail("This preset setting isn't a bounded scalar or module list."))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn options(plugin: &str) -> WriteOptions {
        WriteOptions {
            version: structure::builtin(plugin).unwrap().verified.writes[0].version.clone(),
            name: Some("Reviewed patch".into()),
            class_id: None,
        }
    }
    fn change(path: &str, value: Value) -> Edit {
        Edit { path: path.into(), value }
    }

    #[test]
    fn serum_volume_is_a_stored_value_not_an_invented_percent() {
        let reviewed = build("serum-2", &options("serum-2"), &[change("Oscillator1/plainParams/kParamVolume", json!(0.45))]).unwrap();
        let setting = &reviewed.metadata.settings[0];
        assert_eq!(setting.after, json!(0.45));
        assert_eq!(setting.unit, "stored value");
        assert_eq!(setting.range, Some([0.0, 1.0]));
        assert!(setting.line().ends_with(" → 0.45, stored value, 0–1"), "{}", setting.line());
        assert_eq!(VALUE_NOTE, "Values as the preset file stores them; the plug-in may show them differently.");
        reviewed.metadata.verify(&reviewed.bytes).unwrap();
        let mut lying = reviewed.metadata.clone();
        lying.settings[0].after = json!(45.0);
        assert!(lying.verify(&reviewed.bytes).is_err());
    }

    #[test]
    fn fabfilter_review_reports_the_written_float_not_the_requested_precision() {
        let requested = 0.123_456_789_f64;
        let reviewed =
            build("fabfilter-pro-q-4", &options("fabfilter-pro-q-4"), &[change("parameters/Band 1 Gain", json!(requested))]).unwrap();
        assert_eq!(reviewed.metadata.settings[0].after, json!(f64::from(requested as f32)));
        assert_ne!(reviewed.metadata.settings[0].after, json!(requested));
        reviewed.metadata.verify(&reviewed.bytes).unwrap();
    }

    #[test]
    fn a_review_takes_as_many_settings_as_the_shared_writer() {
        let edits: Vec<_> = (1..=24)
            .flat_map(|band| {
                [change(&format!("parameters/Band {band} Gain"), json!(2.0)), change(&format!("parameters/Band {band} Q"), json!(0.25))]
            })
            .collect();
        assert_eq!(edits.len(), 48);
        let reviewed = build("fabfilter-pro-q-4", &options("fabfilter-pro-q-4"), &edits).unwrap();
        assert_eq!(reviewed.metadata.settings.len(), 48);
        reviewed.metadata.verify(&reviewed.bytes).unwrap();
        let native = create("fabfilter-pro-q-4", &options("fabfilter-pro-q-4"), &edits).unwrap();
        assert_eq!(native.write(), reviewed.bytes, "the same edits make the same file as standalone's vst3 create");
    }

    #[test]
    fn ozone_aliases_report_one_final_control_and_the_written_module_order() {
        let reviewed = build(
            "ozone-12",
            &options("ozone-12"),
            &[
                change("$chain", json!(["Equalizer", "Stereo Imager", "Maximizer"])),
                change("parameters/Maximizer/Gain", json!(3.0)),
                change("DSP State/DSP Elements/Maximizer/Gain", json!(4.5)),
            ],
        )
        .unwrap();
        assert_eq!(reviewed.metadata.settings.iter().map(|s| s.path.as_str()).collect::<Vec<_>>(), ["$chain", "parameters/Maximizer/Gain"]);
        assert_eq!(reviewed.metadata.settings[0].before, json!("not stored in template"));
        assert_eq!(reviewed.metadata.settings[0].after, json!(["Equalizer", "Stereo Imager", "Maximizer"]));
        assert_eq!(reviewed.metadata.settings[1].after, json!(4.5));
        reviewed.metadata.verify(&reviewed.bytes).unwrap();
    }

    #[test]
    fn the_initialized_distortion_writer_reports_the_modules_it_actually_added() {
        let reviewed =
            build("serum-2", &options("serum-2"), &[change("$add_distortion", json!(0)), change("$add_distortion", json!(0))]).unwrap();
        assert_eq!(reviewed.metadata.settings.len(), 1);
        assert_eq!(reviewed.metadata.settings[0].before, json!([]));
        assert_eq!(reviewed.metadata.settings[0].after, json!(["Distortion", "Distortion"]));
        reviewed.metadata.verify(&reviewed.bytes).unwrap();
    }

    #[test]
    fn reviews_refuse_hidden_names_unverified_controls_versions_and_oversized_changes() {
        let mut opts = options("serum-2");
        for name in ["../patch", "C:\\patch", "safe\u{202e}name", "safe\u{200b}name"] {
            opts.name = Some(name.into());
            assert!(build("serum-2", &opts, &[]).is_err(), "{name:?}");
        }
        opts = options("serum-2");
        for path in ["Oscillator0/SampleOsc0/samplePath", "presetAuthor", "presetName"] {
            assert!(build("serum-2", &opts, &[change(path, json!("not allowed"))]).is_err(), "{path}");
        }
        let many = vec![change("Oscillator1/plainParams/kParamVolume", json!(0.5)); MAX_EDITS + 1];
        assert!(matches!(build("serum-2", &opts, &many), Err(ReviewError::TooLong)));
        opts.version = "999.0".into();
        assert!(build("serum-2", &opts, &[]).is_err());
        assert!(build("vital", &opts, &[]).is_err());
    }
}
