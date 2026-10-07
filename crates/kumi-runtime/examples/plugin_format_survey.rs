//! Survey a plug-in's preset and state format across the files a machine has, and fold what it finds into
//! the plug-in's structure.json. It decodes every preset under the folders given (factory ones included,
//! read where they lie) and every state of that plug-in in Live Sets (.als), device presets (.adv) and VST3
//! presets (.vstpreset), and checks each round trip. Only counts, types, ranges and option-like values leave the machine.
//!
//! cargo run -p kumi-runtime --example plugin_format_survey -- <serum-2|vital|ozone-12> <report.json>
//!     [--structure plugin-formats/<plug-in>/format-1/structure.json] <file or folder>...

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use kumi_runtime::plugins::formats::live_set::{self, PluginFormat as Host, PluginState};
use kumi_runtime::plugins::formats::ozone12::{OzonePreset, OzoneState};
use kumi_runtime::plugins::formats::serum2::{SerumPreset, XferFile};
use kumi_runtime::plugins::formats::structure::{builtin, merge_survey, to_text};
use kumi_runtime::plugins::formats::survey::Survey;
use kumi_runtime::plugins::formats::vital::VitalPreset;
use kumi_runtime::plugins::formats::vstpreset::{vst2_chunk, VstPreset};
use kumi_runtime::plugins::formats::{cbor, FormatError};
use serde_json::{json, Value};

#[derive(Default)]
struct Tally {
    files: BTreeMap<String, usize>,
    versions: BTreeMap<String, usize>,
    checks: BTreeMap<String, usize>,
}

impl Tally {
    fn count(map: &mut BTreeMap<String, usize>, key: impl Into<String>) {
        *map.entry(key.into()).or_default() += 1;
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let usage = "usage: plugin_format_survey <serum-2|vital|ozone-12> <report.json> [--structure <structure.json>] <file or folder>...";
    if args.len() < 3 {
        eprintln!("{usage}");
        std::process::exit(2);
    }
    let plugin = args[0].as_str();
    let format = builtin(plugin).unwrap_or_else(|| panic!("{plugin} isn't a plug-in Kumi knows: {usage}"));
    let report_path = PathBuf::from(&args[1]);
    let mut structure_path = None;
    let mut inputs = Vec::new();
    let mut rest = args[2..].iter();
    while let Some(arg) = rest.next() {
        if arg == "--structure" {
            structure_path = rest.next().map(PathBuf::from);
        } else {
            inputs.push(PathBuf::from(arg));
        }
    }

    let mut files = Vec::new();
    for input in &inputs {
        walk(input, &mut files);
    }
    files.sort();
    let mut survey = Survey::new(format.structure.paths);
    let mut tally = Tally::default();
    let wanted: Vec<String> = format
        .info
        .presets
        .extensions
        .iter()
        .map(|e| e.trim_start_matches('.').to_ascii_lowercase())
        .chain(["als".into(), "adv".into(), "vstpreset".into()])
        .collect();
    for file in &files {
        let extension = file.extension().and_then(|e| e.to_str()).unwrap_or_default().to_ascii_lowercase();
        if !wanted.contains(&extension) {
            continue;
        }
        let bytes = match std::fs::read(file) {
            Ok(bytes) => bytes,
            Err(error) => {
                survey.fail(&file.display().to_string(), &error.to_string());
                continue;
            }
        };
        let result = match extension.as_str() {
            "als" | "adv" | "vstpreset" => states(plugin, &extension, &bytes, &mut survey, &mut tally),
            _ => preset(plugin, &extension, &bytes, &mut survey, &mut tally),
        };
        match result {
            Ok(()) => {}
            Err(error) if error.0 == "skip" => {}
            Err(error) => survey.fail(&file.display().to_string(), &error.0),
        }
    }

    let report = survey.report();
    let summary = json!({
        "date": chrono::Local::now().format("%Y-%m-%d").to_string(),
        "files": tally.files,
        "versions": tally.versions,
    });
    let out = json!({"plugin": plugin, "summary": summary, "checks": tally.checks, "report": report});
    std::fs::write(&report_path, serde_json::to_string_pretty(&out).unwrap()).expect("write the report");
    eprintln!(
        "{plugin}: {} files read, {} failed, {} field paths; files {:?}; checks {:?}",
        report.files,
        report.failures.len(),
        report.fields.len(),
        tally.files,
        tally.checks
    );
    for (file, error) in report.failures.iter().take(10) {
        eprintln!("  failed: {file}: {error}");
    }
    if let Some(path) = structure_path {
        let mut structure: Value =
            serde_json::from_str(&std::fs::read_to_string(&path).expect("read structure.json")).expect("structure.json is JSON");
        merge_survey(&mut structure, &report, summary);
        std::fs::write(&path, to_text(&structure)).expect("write structure.json");
        eprintln!("merged into {}", path.display());
    }
}

/// Files under a path. Live's Backup folders (older copies of Sets) and code folders are skipped.
fn walk(path: &Path, files: &mut Vec<PathBuf>) {
    if path.is_dir() {
        if path.file_name().is_some_and(|name| name == "Backup" || name == "node_modules" || name == ".git") {
            return;
        }
        if let Ok(entries) = std::fs::read_dir(path) {
            for entry in entries.flatten() {
                walk(&entry.path(), files);
            }
        }
    } else {
        files.push(path.to_path_buf());
    }
}

fn skip() -> FormatError {
    FormatError::new("skip")
}

fn preset(plugin: &str, extension: &str, bytes: &[u8], survey: &mut Survey, tally: &mut Tally) -> Result<(), FormatError> {
    match (plugin, extension) {
        ("serum-2", "serumpreset") => {
            let (file, decoded) = XferFile::read_with_body(bytes)?;
            let preset = SerumPreset::read(bytes)?;
            Tally::count(
                &mut tally.versions,
                format!("preset {} (format {})", preset.product_version().unwrap_or_default(), preset.format_version().unwrap_or_default()),
            );
            serum_checks(&file, &decoded, "preset", tally)?;
            survey.add(&file.body.to_node(), "preset");
            Tally::count(&mut tally.files, "preset");
        }
        ("vital", "vital") => {
            let preset = VitalPreset::read(bytes)?;
            Tally::count(&mut tally.versions, format!("preset {}", preset.synth_version().unwrap_or("?")));
            let again = VitalPreset::read(&preset.write())?;
            Tally::count(&mut tally.checks, if again == preset { "preset round trip" } else { "preset round trip FAILED" });
            survey.add(&preset.to_node(), "preset");
            Tally::count(&mut tally.files, "preset");
        }
        ("ozone-12", "xml") => {
            let preset = OzonePreset::read(bytes)?;
            Tally::count(
                &mut tally.versions,
                format!(
                    "preset {} PresetVer {} PluginVer {}",
                    preset.root(),
                    preset.attribute("PresetVer").unwrap_or("?"),
                    preset.attribute("PluginVer").unwrap_or("?")
                ),
            );
            let again = OzonePreset::read(&preset.write())?;
            Tally::count(&mut tally.checks, if again == preset { "preset round trip" } else { "preset round trip FAILED" });
            if let Some(chain) = preset.chain()? {
                for entry in chain {
                    Tally::count(&mut tally.checks, format!("chain flag {}", entry.flag));
                }
            }
            survey.add(&preset.to_node(), "preset");
            Tally::count(&mut tally.files, "preset");
        }
        _ => return Err(skip()),
    }
    Ok(())
}

fn serum_checks(file: &XferFile, decoded: &[u8], tag: &str, tally: &mut Tally) -> Result<(), FormatError> {
    Tally::count(
        &mut tally.checks,
        format!(
            "{tag} hash {}",
            match file.hash_ok {
                Some(true) => "matches",
                Some(false) => "DIFFERS",
                None => "absent",
            }
        ),
    );
    let exact = cbor::encode(&file.body) == decoded;
    Tally::count(&mut tally.checks, format!("{tag} CBOR re-encodes {}", if exact { "byte for byte" } else { "DIFFERENTLY" }));
    let again = XferFile::read(&file.write()?)?;
    // write() sets the hash for the new encoding; everything else in the header stays.
    let without_hash = |file: &XferFile| {
        file.header_json().map(|mut header| {
            header.as_object_mut().map(|object| object.remove("hash"));
            header
        })
    };
    let same = again.body == file.body && without_hash(&again)? == without_hash(file)? && again.hash_ok != Some(false);
    Tally::count(&mut tally.checks, if same { format!("{tag} round trip") } else { format!("{tag} round trip FAILED") });
    Ok(())
}

/// The plug-in's states in a Live Set or device preset (.als, .adv) or a VST3 preset (.vstpreset).
fn states(plugin: &str, extension: &str, bytes: &[u8], survey: &mut Survey, tally: &mut Tally) -> Result<(), FormatError> {
    let format = builtin(plugin).expect("checked in main");
    let found = if extension == "vstpreset" {
        let preset = VstPreset::read(bytes)?;
        vec![PluginState {
            format: Host::Vst3,
            name: String::new(),
            id: preset.class_id.clone(),
            processor: preset.component().unwrap_or_default().to_vec(),
            controller: preset.controller().unwrap_or_default().to_vec(),
            chunk_type: None,
        }]
    } else {
        live_set::read(bytes)?
    };
    for state in found.iter().filter(|state| format.has_id(&state.id)) {
        match plugin {
            "serum-2" => {
                for (tag, half) in [("processor", &state.processor), ("controller", &state.controller)] {
                    if half.is_empty() {
                        continue;
                    }
                    let (file, decoded) = XferFile::read_with_body(half)?;
                    let header = file.header_json()?;
                    Tally::count(
                        &mut tally.versions,
                        format!("{tag} {} (format {})", header["productVersion"].as_str().unwrap_or("?"), header["version"]),
                    );
                    serum_checks(&file, &decoded, tag, tally)?;
                    survey.add(&file.body.to_node(), tag);
                    Tally::count(&mut tally.files, tag);
                }
            }
            "vital" => {
                // The VST3 build wraps the VST2 chunk; the VST2 build hands it over as is.
                let wrapped = vst2_chunk(&state.processor)?;
                let chunk = wrapped.as_ref().map_or(state.processor.as_slice(), |found| found.data);
                let preset = VitalPreset::read_state(chunk)?;
                let host = if wrapped.is_some() { "VST3" } else { "VST2" };
                Tally::count(&mut tally.versions, format!("state {} {host}", preset.synth_version().unwrap_or("?")));
                survey.add(&preset.to_node(), "state");
                Tally::count(&mut tally.files, "state");
            }
            "ozone-12" => {
                let read = OzoneState::read(&state.processor)?;
                Tally::count(&mut tally.versions, format!("state header version {}", read.version));
                Tally::count(
                    &mut tally.checks,
                    if state.controller.is_empty() { "controller state empty" } else { "controller state present" },
                );
                survey.add(&read.to_node(), "state");
                Tally::count(&mut tally.files, "state");
            }
            _ => {}
        }
    }
    Ok(())
}
