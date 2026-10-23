//! Drum Sampler presets that hold a sample. Live 12's Drum Sampler has no scripting call that takes a
//! sample, and Live makes a Simpler of any sample the Browser loads onto a pad. A preset does it: Live's
//! own default Drum Sampler preset, with the sample in its UserSample, written where the Browser sees it
//! (the User Library) and loaded onto the pad as a Drum Sampler. The preset is only a carrier; once
//! loaded, the device lives in the Set and the preset file can go.

use std::collections::HashMap;
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use flate2::read::MultiGzDecoder;
use flate2::{Compression, GzBuilder};
use regex::{NoExpand, Regex};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DrumSamplerTemplate {
    /// Live's default Drum Sampler preset, as XML.
    pub xml: String,
    /// Where Live keeps its built-in Drum Sampler, which the preset names as its origin.
    pub builtin_device_path: String,
}

/// The sample a preset carries: its path, byte size and modification time in seconds.
#[derive(Debug, Clone, PartialEq)]
pub struct DrumSamplerSample {
    pub path: String,
    pub size: f64,
    pub modified_seconds: f64,
}

/// A preset the bridge cannot build; the text is the TypeScript's.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct DrumSamplerPresetError(pub String);

/// `process.platform`: "win32", "darwin" or "linux".
pub fn process_platform() -> &'static str {
    if cfg!(windows) {
        "win32"
    } else if cfg!(target_os = "macos") {
        "darwin"
    } else {
        "linux"
    }
}

/// `process.env` as a map.
pub fn process_env() -> HashMap<String, String> {
    kumi_common::env::vars()
}

/// `path.join(...)`.
fn join(parts: &[&str]) -> String {
    let mut path = PathBuf::from(parts[0]);
    for part in &parts[1..] {
        path.push(part);
    }
    path.to_string_lossy().into_owned()
}

/// JavaScript's default string order: UTF-16 code units.
fn js_str_cmp(a: &str, b: &str) -> std::cmp::Ordering {
    a.encode_utf16().cmp(b.encode_utf16())
}

/// Live's resources folders, newest-looking first: the app bundles on macOS, ProgramData on Windows.
/// `platform` and `env` default to the process's own, as the TypeScript's parameters did.
pub fn live_resource_folders(platform: Option<&str>, env: Option<&HashMap<String, String>>) -> Vec<String> {
    static LIVE: LazyLock<Regex> = LazyLock::new(|| Regex::new("(?i)^Live ").unwrap());
    static APP: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)^Ableton Live.*\.app$").unwrap());
    let platform = platform.unwrap_or_else(|| process_platform());
    let process_env_map;
    let env = match env {
        Some(env) => env,
        None => {
            process_env_map = process_env();
            &process_env_map
        }
    };
    if let Some(folder) = env.get("ABLETON_MCP_LIVE_RESOURCES").filter(|folder| !folder.is_empty()) {
        return vec![folder.clone()];
    }
    let list = |folder: &str| -> Vec<String> {
        let Ok(entries) = fs::read_dir(folder) else { return Vec::new() };
        let mut names: Vec<String> = entries.filter_map(Result::ok).map(|entry| entry.file_name().to_string_lossy().into_owned()).collect();
        names.sort_by(|a, b| js_str_cmp(a, b));
        names.reverse();
        names
    };
    if platform == "win32" {
        // Windows environment variables are case-insensitive, as process.env reads them there.
        let program_data = env
            .get("ProgramData")
            .or_else(|| env.iter().find(|(key, _)| key.eq_ignore_ascii_case("ProgramData")).map(|(_, value)| value))
            .cloned()
            .unwrap_or_else(|| "C:\\ProgramData".to_string());
        return list(&join(&[&program_data, "Ableton"]))
            .into_iter()
            .filter(|name| LIVE.is_match(name))
            .map(|name| join(&[&program_data, "Ableton", &name, "Resources"]))
            .collect();
    }
    list("/Applications")
        .into_iter()
        .filter(|name| APP.is_match(name))
        .map(|name| join(&["/Applications", &name, "Contents", "App-Resources"]))
        .collect()
}

/// The first installed Live with a default Drum Sampler preset; None without one (Live 11 and earlier).
pub fn find_drum_sampler_template(folders: &[String]) -> Option<DrumSamplerTemplate> {
    for folder in folders {
        let preset = join(&[folder, "Core Library", "Defaults", "Instruments", "Drum Sampler.adv"]);
        if !Path::new(&preset).exists() {
            continue;
        }
        let Ok(bytes) = fs::read(&preset) else { continue };
        let mut xml: Vec<u8> = Vec::new();
        if MultiGzDecoder::new(bytes.as_slice()).read_to_end(&mut xml).is_err() {
            continue;
        }
        return Some(DrumSamplerTemplate {
            xml: String::from_utf8_lossy(&xml).into_owned(),
            builtin_device_path: join(&[folder, "Builtin", "Devices", "Instruments", "Drum Sampler"]),
        });
    }
    None
}

/// Live's User Library, where it is by default. `platform` and `home` default to the process's own.
pub fn default_user_library(platform: Option<&str>, home: Option<&str>) -> String {
    let platform = platform.unwrap_or_else(|| process_platform());
    let home = home.map(str::to_string).or_else(|| home::home_dir().map(|home| home.to_string_lossy().into_owned())).unwrap_or_default();
    if platform == "win32" {
        join(&[&home, "Documents", "Ableton", "User Library"])
    } else {
        join(&[&home, "Music", "Ableton", "User Library"])
    }
}

fn attribute(value: &str) -> String {
    let mut quoted = String::with_capacity(value.len() + 2);
    quoted.push('"');
    for character in value.chars() {
        match character {
            '&' => quoted.push_str("&amp;"),
            '<' => quoted.push_str("&lt;"),
            '>' => quoted.push_str("&gt;"),
            '"' => quoted.push_str("&quot;"),
            control if (control as u32) < 0x20 => quoted.push_str(&format!("&#{};", control as u32)),
            other => quoted.push(other),
        }
    }
    quoted.push('"');
    quoted
}

/// The byte zlib writes for the operating system in a gzip header, which Node's gzip carries.
fn gzip_operating_system() -> u8 {
    if cfg!(target_os = "macos") {
        19
    } else if cfg!(windows) {
        11
    } else {
        3
    }
}

/// The template with `sample.path` as its sample, gzipped as Live writes presets. The file reference
/// is absolute, as for a sample outside Live's libraries; Live reads the sample's length itself.
pub fn drum_sampler_preset(template: &DrumSamplerTemplate, sample: &DrumSamplerSample) -> Result<Vec<u8>, DrumSamplerPresetError> {
    static DRUM_CELL: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"<DrumCell[\s>]").unwrap());
    static EMPTY: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"<UserSample>\s*<Value\s*/>\s*</UserSample>").unwrap());
    static LAST: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?s)<LastPresetRef>.*?</LastPresetRef>").unwrap());
    let whole = |value: f64| kumi_common::js::number::to_string(value.floor().max(0.0));
    let user_sample = format!(
        "<UserSample>\n\t\t\t<Value>\n\t\t\t\t<SampleRef Id=\"0\">\n\t\t\t\t\t<FileRef>\n\t\t\t\t\t\t<RelativePathType Value=\"0\" />\n\t\t\t\t\t\t<RelativePath Value=\"\" />\n\t\t\t\t\t\t<Path Value={} />\n\t\t\t\t\t\t<Type Value=\"1\" />\n\t\t\t\t\t\t<LivePackName Value=\"\" />\n\t\t\t\t\t\t<LivePackId Value=\"\" />\n\t\t\t\t\t\t<OriginalFileSize Value=\"{}\" />\n\t\t\t\t\t\t<OriginalCrc Value=\"0\" />\n\t\t\t\t\t\t<SourceHint Value=\"\" />\n\t\t\t\t\t</FileRef>\n\t\t\t\t\t<LastModDate Value=\"{}\" />\n\t\t\t\t\t<SourceContext />\n\t\t\t\t\t<SampleUsageHint Value=\"0\" />\n\t\t\t\t\t<DefaultDuration Value=\"0\" />\n\t\t\t\t\t<DefaultSampleRate Value=\"0\" />\n\t\t\t\t\t<SamplesToAutoWarp Value=\"1\" />\n\t\t\t\t</SampleRef>\n\t\t\t</Value>\n\t\t</UserSample>",
        attribute(&sample.path),
        whole(sample.size),
        whole(sample.modified_seconds)
    );
    let origin = format!(
        "<LastPresetRef>\n\t\t\t<Value>\n\t\t\t\t<AbletonDefaultPresetRef Id=\"0\">\n\t\t\t\t\t<FileRef>\n\t\t\t\t\t\t<RelativePathType Value=\"7\" />\n\t\t\t\t\t\t<RelativePath Value=\"Devices/Instruments/Drum Sampler\" />\n\t\t\t\t\t\t<Path Value={} />\n\t\t\t\t\t\t<Type Value=\"2\" />\n\t\t\t\t\t\t<LivePackName Value=\"\" />\n\t\t\t\t\t\t<LivePackId Value=\"\" />\n\t\t\t\t\t\t<OriginalFileSize Value=\"0\" />\n\t\t\t\t\t\t<OriginalCrc Value=\"0\" />\n\t\t\t\t\t\t<SourceHint Value=\"\" />\n\t\t\t\t\t</FileRef>\n\t\t\t\t\t<DeviceId Name=\"DrumCell\" />\n\t\t\t\t</AbletonDefaultPresetRef>\n\t\t\t</Value>\n\t\t</LastPresetRef>",
        attribute(&template.builtin_device_path)
    );
    if !DRUM_CELL.is_match(&template.xml) || !EMPTY.is_match(&template.xml) || !LAST.is_match(&template.xml) {
        return Err(DrumSamplerPresetError("Live's default Drum Sampler preset has a shape the bridge doesn't know".to_string()));
    }
    // Literal replacements, so a "$" in a path is never read as a replacement pattern.
    let xml = EMPTY.replacen(&template.xml, 1, NoExpand(&user_sample));
    let xml = LAST.replacen(&xml, 1, NoExpand(&origin));
    let mut encoder = GzBuilder::new().operating_system(gzip_operating_system()).write(Vec::new(), Compression::default());
    encoder.write_all(xml.as_bytes()).map_err(|error| DrumSamplerPresetError(error.to_string()))?;
    encoder.finish().map_err(|error| DrumSamplerPresetError(error.to_string()))
}
