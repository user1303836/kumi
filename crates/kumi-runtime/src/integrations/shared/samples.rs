//! Finding samples on disk without following links or reading whole audio files.
use crate::{
    core::errors::RuntimeError,
    library::sources::{basename, current_platform, expand_folder, homedir, join, resolve},
};
use futures::future::join_all;
use kumi_common::{
    abort::{Signal, SignalExt},
    js::string::locale_compare_numeric_base,
    js::{number::round, string::trim},
};
use rand::{Rng, TryRngCore};
use serde::{Deserialize, Serialize};
use std::{collections::VecDeque, path::Path};
use tokio::{fs, io::AsyncReadExt};

pub const SAMPLE_EXTENSIONS: &[&str] = &[".wav", ".wave", ".aif", ".aiff", ".flac", ".mp3", ".ogg", ".m4a"];
const MAX_FILES: usize = 60_000;
const MAX_FOLDERS: usize = 12_000;
const MAX_DEPTH: usize = 12;
const MAX_MATCHES: usize = 5_000;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Sample {
    pub name: String,
    pub path: String,
    pub folder: String,
    pub bytes: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub seconds: Option<f64>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SampleSearch {
    pub samples: Vec<Sample>,
    pub scanned: usize,
    pub matched: usize,
    pub partial: bool,
    pub missing: Vec<String>,
}
#[derive(Debug, Clone, Default)]
pub struct FindSamplesOptions {
    pub folders: Vec<String>,
    pub words: Vec<String>,
    pub limit: usize,
    pub random: bool,
    pub signal: Option<Signal>,
}
pub fn user_library(platform: Option<&str>, home: Option<&str>) -> String {
    let home = home.map(str::to_owned).unwrap_or_else(homedir);
    join(
        &join(&join(&home, if platform.unwrap_or_else(|| current_platform()) == "win32" { "Documents" } else { "Music" }), "Ableton"),
        "User Library",
    )
}
pub fn default_sample_folders(platform: Option<&str>, home: Option<&str>, program_data: Option<&str>) -> Vec<String> {
    let platform = platform.unwrap_or_else(|| current_platform());
    let home = home.map(str::to_owned).unwrap_or_else(homedir);
    let program_data =
        program_data.map(str::to_owned).unwrap_or_else(|| std::env::var("ProgramData").unwrap_or_else(|_| "C:\\ProgramData".into()));
    let root = if platform == "win32" { join(&program_data, "Ableton") } else { "/Applications".into() };
    let mut names: Vec<String> =
        std::fs::read_dir(&root).into_iter().flatten().flatten().map(|entry| entry.file_name().to_string_lossy().into_owned()).collect();
    names.sort_by(|a, b| a.encode_utf16().cmp(b.encode_utf16()));
    let core = names
        .into_iter()
        .filter_map(|name| {
            let lower = name.to_lowercase();
            if platform == "win32" {
                lower.starts_with("live ").then(|| join(&join(&root, &name), "Resources/Core Library/Samples"))
            } else {
                (lower.starts_with("ableton live") && lower.ends_with(".app") && !name.contains(['\n', '\r', '\u{2028}', '\u{2029}']))
                    .then(|| join(&join(&root, &name), "Contents/App-Resources/Core Library/Samples"))
            }
        })
        .find(|folder| Path::new(folder).exists());
    let packs = join(&join(&join(&home, if platform == "win32" { "Documents" } else { "Music" }), "Ableton"), "Factory Packs");
    [Some(user_library(Some(platform), Some(&home))), core, Some(packs)]
        .into_iter()
        .flatten()
        .filter(|path| Path::new(path).exists())
        .collect()
}
/// The audio file a change brings in, found as the sample bank finds Live's: a whole path, or one under `~`, to a file
/// that is there on this computer (never on a network share or a device), of one of the `extensions` the DAW takes
/// (".wav"). Its whole path, or why not, in Kumi's words.
pub fn sample_file(path: &str, extensions: &[&str]) -> Result<String, String> {
    let path = trim(path);
    let full = match path.strip_prefix("~/").or_else(|| path.strip_prefix("~\\")) {
        Some(rest) => join(&homedir(), rest),
        None => path.to_string(),
    };
    let asked = "Give the whole path of an audio file on this computer (one find_sounds returned, a recording, the producer's own)";
    if kumi_common::path::network_or_device(&full) || !Path::new(&full).is_absolute() {
        return Err(format!("{asked}."));
    }
    let extension = Path::new(&full).extension().map(|e| format!(".{}", e.to_string_lossy().to_lowercase())).unwrap_or_default();
    if !extensions.contains(&extension.as_str()) {
        let kinds: Vec<&str> = extensions.iter().map(|e| e.trim_start_matches('.')).collect();
        return Err(format!("{asked}: one of {}.", kinds.join(", ")));
    }
    match std::fs::metadata(&full) {
        Ok(stat) if stat.is_file() => Ok(full),
        _ => Err(format!("There's no audio file at {full}. {asked}.")),
    }
}

pub fn folder_path(value: &str, home: Option<&str>) -> Option<String> {
    expand_folder(value, &home.map(str::to_owned).unwrap_or_else(homedir))
}

pub async fn find_samples(options: FindSamplesOptions) -> Result<SampleSearch, RuntimeError> {
    find_samples_with(SAMPLE_EXTENSIONS, options).await
}

/// `find_samples` over the audio files of `extensions` (another DAW's own: Logic Pro's `.caf` among them).
pub async fn find_samples_with(extensions: &[&str], options: FindSamplesOptions) -> Result<SampleSearch, RuntimeError> {
    let words: Vec<String> = options.words.iter().map(|word| trim(word).to_lowercase()).filter(|word| !word.is_empty()).collect();
    let mut found: Vec<(Sample, usize)> = Vec::new();
    let mut missing = Vec::new();
    let (mut scanned, mut folders, mut partial) = (0, 0, false);
    for root in options.folders {
        let mut queue = VecDeque::from([(root.clone(), 0)]);
        let mut readable = false;
        while let Some((path, depth)) = queue.pop_front() {
            if let Some(signal) = &options.signal {
                signal.check()?;
            }
            folders += 1;
            if folders > MAX_FOLDERS {
                partial = true;
                break;
            }
            let Ok(mut entries) = fs::read_dir(&path).await else { continue };
            readable = true;
            while let Some(entry) = entries.next_entry().await.map_err(|e| RuntimeError::plain(e.to_string()))? {
                let name = entry.file_name().to_string_lossy().into_owned();
                if name.starts_with('.') || name.to_lowercase() == "ableton folder info" {
                    continue;
                }
                let full = join(&path, &name);
                let kind = entry.file_type().await.map_err(|e| RuntimeError::plain(e.to_string()))?;
                if kind.is_dir() {
                    if depth < MAX_DEPTH {
                        queue.push_back((full, depth + 1));
                    } else {
                        partial = true;
                    }
                    continue;
                }
                let extension = name.rfind('.').map(|at| &name[at..]).unwrap_or("");
                if !kind.is_file() || !extensions.contains(&extension.to_lowercase().as_str()) {
                    continue;
                }
                scanned += 1;
                if scanned > MAX_FILES {
                    partial = true;
                    break;
                }
                let absolute = resolve(&full);
                let absolute_root = resolve(&root);
                let where_ =
                    Path::new(&absolute).strip_prefix(&absolute_root).unwrap_or(Path::new(&absolute)).to_string_lossy().to_lowercase();
                if !words.iter().all(|word| where_.contains(word)) {
                    continue;
                }
                let base = basename(&name);
                let name = base[..base.len() - extension.len()].to_string();
                let lower = name.to_lowercase();
                let parts: Vec<_> = where_.split(['\\', '/']).collect();
                let directory_parts = &parts[..parts.len().saturating_sub(1)];
                let score = words
                    .iter()
                    .map(|word| {
                        (if lower.starts_with(word) {
                            10
                        } else if lower.contains(word) {
                            5
                        } else {
                            0
                        }) + if directory_parts.iter().any(|part| *part == word || *part == format!("{word}s")) { 3 } else { 0 }
                    })
                    .sum();
                if found.len() < MAX_MATCHES {
                    found.push((Sample { name, path: full, folder: root.clone(), bytes: 0, seconds: None }, score));
                } else {
                    partial = true;
                }
            }
            if scanned > MAX_FILES {
                break;
            }
        }
        if !readable {
            missing.push(root);
        }
        if scanned > MAX_FILES || folders > MAX_FOLDERS {
            break;
        }
    }
    let matched = found.len();
    if options.random {
        let mut random = rand::rngs::OsRng.unwrap_err();
        for index in (1..found.len()).rev() {
            let other = random.random_range(0..=index);
            found.swap(index, other);
        }
    } else {
        found.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| locale_compare_numeric_base(&a.0.name, &b.0.name)));
    }
    let samples = join_all(found.into_iter().take(options.limit).map(|(mut sample, _)| async move {
        if let Ok((bytes, seconds)) = describe(&sample.path).await {
            sample.bytes = bytes;
            sample.seconds = seconds.map(|seconds| round(seconds * 1000.0) / 1000.0);
        }
        sample
    }))
    .await;
    Ok(SampleSearch { samples, scanned: scanned.min(MAX_FILES), matched, partial, missing })
}
async fn describe(path: &str) -> Result<(u64, Option<f64>), RuntimeError> {
    let io = |e: std::io::Error| RuntimeError::plain(e.to_string());
    let mut file = fs::File::open(path).await.map_err(io)?;
    let bytes = file.metadata().await.map_err(io)?.len();
    let mut header = vec![0; bytes.min(64 * 1024) as usize];
    let read = file.read(&mut header).await.map_err(io)?;
    Ok((bytes, audio_seconds(&header[..read], bytes as f64)?))
}
/// Seconds from WAV/AIFF headers. A truncated WAV rate field throws as Node's Buffer read does.
pub fn audio_seconds(header: &[u8], file_bytes: f64) -> Result<Option<f64>, RuntimeError> {
    let tag = |at: usize| header.get(at..at.saturating_add(4)).unwrap_or_default();
    if header.len() >= 12 && tag(0) == b"RIFF" && tag(8) == b"WAVE" {
        let (mut at, mut byte_rate) = (12usize, 0u32);
        while at.saturating_add(8) <= header.len() {
            let size = u32::from_le_bytes(header[at + 4..at + 8].try_into().unwrap());
            if tag(at) == b"fmt " && at + 16 <= header.len() {
                byte_rate = u32::from_le_bytes(
                    header
                        .get(at + 16..at + 20)
                        .ok_or_else(|| RuntimeError::plain("Attempt to access memory outside buffer bounds"))?
                        .try_into()
                        .unwrap(),
                );
            }
            if tag(at) == b"data" {
                let bytes = if size == 0 || size == u32::MAX { file_bytes - at as f64 - 8.0 } else { size as f64 };
                return Ok((byte_rate != 0).then(|| bytes / byte_rate as f64));
            }
            at = at.saturating_add(8 + size as usize + size as usize % 2);
        }
    }
    if header.len() >= 12 && tag(0) == b"FORM" && [b"AIFF".as_slice(), b"AIFC".as_slice()].contains(&tag(8)) {
        let mut at = 12usize;
        while at.saturating_add(8) <= header.len() {
            let size = u32::from_be_bytes(header[at + 4..at + 8].try_into().unwrap());
            if tag(at) == b"COMM" && at + 26 <= header.len() {
                let frames = u32::from_be_bytes(header[at + 10..at + 14].try_into().unwrap());
                let rate = extended(&header[at + 16..at + 26]);
                return Ok((rate > 0.0).then(|| frames as f64 / rate));
            }
            at = at.saturating_add(8 + size as usize + size as usize % 2);
        }
    }
    Ok(None)
}
fn extended(bytes: &[u8]) -> f64 {
    let exponent = ((bytes[0] as i32 & 0x7f) << 8) | bytes[1] as i32;
    let mantissa = u32::from_be_bytes(bytes[2..6].try_into().unwrap()) as f64 * 4294967296.0
        + u32::from_be_bytes(bytes[6..10].try_into().unwrap()) as f64;
    if exponent == 0 && mantissa == 0.0 {
        0.0
    } else {
        mantissa * 2.0f64.powi(exponent - 16383 - 63) * if bytes[0] & 0x80 != 0 { -1.0 } else { 1.0 }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// (M6) A change's audio file is found on this computer before the DAW is asked: a whole path to a file of a kind
    /// the DAW takes, else why not in Kumi's words (a path under ~ is made whole first).
    #[test]
    fn a_changes_audio_file_is_a_whole_path_to_a_file_the_daw_takes() {
        let folder = tempfile::tempdir().unwrap();
        let wav = folder.path().join("Kick 01.WAV");
        std::fs::write(&wav, b"RIFF").unwrap();
        let takes = [".wav", ".caf"];
        let wav = wav.to_string_lossy().into_owned();
        assert_eq!(sample_file(&format!("  {wav} "), &takes), Ok(wav.clone()), "the extension in any case, the path trimmed");
        let flac = folder.path().join("Pad.flac");
        std::fs::write(&flac, b"fLaC").unwrap();
        let refused = sample_file(&flac.to_string_lossy(), &takes).unwrap_err();
        assert!(refused.ends_with("one of wav, caf."), "{refused}");
        let refused = sample_file(&folder.path().join("gone.wav").to_string_lossy(), &takes).unwrap_err();
        assert!(refused.starts_with("There's no audio file at"), "{refused}");
        assert!(sample_file(&folder.path().to_string_lossy(), &[""]).is_err(), "a folder isn't a file");
        for asked in ["Kick 01.wav", "//host/share/Kick.wav", "\\\\host\\share\\Kick.wav"] {
            let refused = sample_file(asked, &takes).unwrap_err();
            assert!(refused.starts_with("Give the whole path of an audio file on this computer"), "{asked}: {refused}");
        }
        let home = sample_file("~/kumi-no-such-sample.wav", &takes).unwrap_err();
        assert!(home.contains(&join(&homedir(), "kumi-no-such-sample.wav")), "a path under ~ is made whole: {home}");
    }
}
