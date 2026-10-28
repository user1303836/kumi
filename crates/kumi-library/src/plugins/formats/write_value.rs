//! Evidence gates for scalar edits. Parsing a field does not authorize changing
//! a filename, opaque container, or an inferred text value.

use serde_json::Value;

use super::{
    structure::{self, Status},
    FormatError,
};

/// `path` is a structure key, with the codec's repeated indices canonicalized.
pub fn validate_scalar(plugin: &str, path: &str, value: &Value) -> Result<(), FormatError> {
    let fail = |why: &str| FormatError::new(format!("{path}: {why}"));
    let format = structure::builtin(plugin).ok_or_else(|| fail("unknown format"))?;
    let field = format.structure.field(path).ok_or_else(|| fail("no verified writable field"))?;
    match value {
        Value::Number(number) => {
            let number = number.as_f64().ok_or_else(|| fail("numeric value is not finite"))?;
            structure::validate_number(plugin, path, number)?;
            if field.status == Status::Verified {
                if let Some(options) = &field.options {
                    if !options.keys().any(|option| option.parse::<f64>().ok() == Some(number)) {
                        return Err(fail("value is not a documented option"));
                    }
                }
            }
            Ok(())
        }
        Value::Bool(value) if field.kind != "bool" => validate_scalar(plugin, path, &Value::from(u8::from(*value))),
        Value::Bool(_) if field.status == Status::Verified => Ok(()),
        Value::String(text) => {
            if text.len() > 4096 || text.chars().any(char::is_control) {
                return Err(fail("text must be at most 4096 bytes without control characters"));
            }
            let path_lower = path.to_ascii_lowercase();
            if path_lower.contains("path")
                || path_lower.contains("filename")
                || path_lower.contains("directory")
                || path_lower.contains("folder")
                || path_lower.contains("url")
                || path_lower.contains("uri")
                || kumi_common::path::network_or_device(text)
                || kumi_common::path::reserved_device(text)
            {
                return Err(fail("file and resource path edits are not supported"));
            }
            if field.kind.split('|').any(|kind| kind == "map") {
                return if field.kind.split('|').any(|kind| kind == "text") && text == "default" {
                    Ok(())
                } else {
                    Err(fail("a map may only be reset to the documented default sentinel"))
                };
            }
            let family = if format.info.family.is_empty() { format.info.plugin.as_str() } else { format.info.family.as_str() };
            // These codecs expose a dedicated display-name operation. They do
            // not interpret the value as a resource or preset source filename.
            let display_name = matches!(
                (family, path),
                ("serum-2", "presetName") | ("valhalla", "name") | ("softube", "presets[]/name") | ("zynaptiq", "@progName")
            );
            if (field.status != Status::Verified && !display_name) || !field.kind.split('|').any(|kind| kind == "text") {
                return Err(fail("text writes have no verified semantics"));
            }
            if field.options.as_ref().is_some_and(|options| !options.contains_key(text)) {
                return Err(fail("text is not a documented option"));
            }
            Ok(())
        }
        _ => Err(fail("only verified scalar fields are writable")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn guessed_paths_and_text_cannot_be_written_but_safe_names_and_defaults_can() {
        for path in [
            "Oscillator{n}/SampleOsc{n}/samplePath",
            "Oscillator{n}/GranularOsc{n}/samplePath",
            "FXRack{n}/FX[]/FXConv/pathToIR",
            "Oscillator{n}/WTOsc{n}/relativePathToWT",
        ] {
            assert!(structure::builtin("serum-2").unwrap().structure.field(path).is_some(), "missing fixture field {path}");
            for value in [r"\\host\share\sample.wav", r"C:\samples\sample.wav", "relative.wav"] {
                assert!(validate_scalar("serum-2", path, &json!(value)).is_err(), "{path}");
            }
        }
        assert!(validate_scalar("serum-2", "VoicePanel{n}/displayName", &json!("inferred")).is_err());
        assert!(validate_scalar("serum-2", "Oscillator{n}/plainParams", &json!("arbitrary container replacement")).is_err());
        assert!(validate_scalar("serum-2", "Oscillator{n}/plainParams", &json!("default")).is_ok());
        assert!(validate_scalar("serum-2", "presetName", &json!("Kumi Pad")).is_ok());
        assert!(validate_scalar("serum-2", "presetName", &json!("\0\\\\host\\share")).is_err());
        assert!(validate_scalar("serum-2", "presetName", &json!("\\\\host\\share")).is_err());
        assert!(validate_scalar("serum-2", "Oscillator{n}/plainParams/kParamEnable", &json!(1)).is_ok());
        assert!(validate_scalar("serum-2", "Oscillator{n}/plainParams/kParamEnable", &json!(0.5)).is_err());
    }
}
