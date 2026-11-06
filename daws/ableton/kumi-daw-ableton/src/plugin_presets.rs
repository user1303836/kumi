//! Put a complete VST3 preset in Live's User Library. `load_device` then performs the normal
//! preview/apply/history/undo transaction; preparing a file itself changes no Live device.
//!
//! Two callers share the decoding, the catalog check and the new-file-only writer: standalone's
//! `plugin {action: "prepare-preset"}`, which reads a `.vstpreset` path ([`prepare`]), and serve's
//! `preset` (CC-26), whose bytes come from a verified artifact and never from a path ([`preview`], [`place_approved`]).

use crate::devices::tool::DeviceToolOptions;
use futures::future::LocalBoxFuture;
use kumi_agent::core::contracts::{PlacedPreset, PresetPreview, PresetRefusal};
use kumi_common::{
    abort::{Signal, SignalExt},
    path::network_or_device,
};
use kumi_library::plugins::formats::{structure, vstpreset::VstPreset, MAX_DECODED_BYTES};
use kumi_types::errors::RuntimeError;
use sha2::{Digest, Sha256};
use std::{
    io::{Read, Write},
    path::{Path, PathBuf},
    rc::Rc,
    time::Duration,
};

const DIFFERENT_BYTES: &str = "Preset destination already exists with different bytes";

#[derive(Debug)]
pub struct PreparedPreset {
    pub file: PathBuf,
    pub item: String,
    pub name: String,
    pub plugin: String,
    pub kind: String,
    pub class_id: String,
}

/// What a preset's bytes would be placed as, read from the bytes alone: nothing is written.
#[derive(Debug, Clone, PartialEq)]
pub struct PresetPlan {
    /// "Instruments" or "Audio Effects", from the plug-in's kind in the built-in catalog.
    pub folder: &'static str,
    /// The file's name without its extension: the producer's name, then the first 12 hex of the bytes' SHA-256.
    pub label: String,
    pub plugin: String,
    pub kind: String,
    pub class_id: String,
}

impl PresetPlan {
    /// The file from the User Library's root, as an approval names it: no absolute path.
    pub fn file(&self) -> String {
        format!("User Library/Kumi/{}/{}.vstpreset", self.folder, self.label)
    }
    /// Live's Browser path for it; Live keeps the extension in a VST3 preset's Browser name (unlike its own device presets).
    pub fn item(&self) -> String {
        format!("user_library/Kumi/{}/{}.vstpreset", self.folder, self.label)
    }
}

fn check_name(name: &str) -> Result<(), String> {
    if name.trim().is_empty()
        || name.chars().count() > 64
        || name.chars().any(|c| c.is_control() || r#"<>:"/\|?*"#.contains(c))
        || name.ends_with(['.', ' '])
    {
        return Err("Preset name must be 1–64 characters without filename separators or control characters".into());
    }
    Ok(())
}

/// Decodes `bytes` and names the file they'd be: a VST3 preset container of version 1 with processor state, whose
/// class is in Kumi's built-in catalog.
pub fn plan(bytes: &[u8], name: &str) -> Result<PresetPlan, String> {
    check_name(name)?;
    if bytes.len() > MAX_DECODED_BYTES {
        return Err("VST3 preset exceeds the supported size limit".into());
    }
    let preset = VstPreset::read(bytes).map_err(|e| e.to_string())?;
    if preset.version != 1 {
        return Err("Unsupported VST3 preset container version".into());
    }
    if preset.component().is_none_or(|c| c.is_empty()) {
        return Err("VST3 preset has no processor state".into());
    }
    let format = structure::builtins()
        .iter()
        .find(|p| p.info.ids.vst3.iter().any(|id| id.id.eq_ignore_ascii_case(&preset.class_id)))
        .ok_or("VST3 preset class is not in Kumi's built-in catalog")?;
    let product = format.info.ids.vst3.iter().find(|id| id.id.eq_ignore_ascii_case(&preset.class_id)).unwrap();
    let digest = hex::encode(Sha256::digest(bytes));
    Ok(PresetPlan {
        // The folder also tells Live's existing loader whether an instrument could replace another.
        folder: if format.info.kind == "instrument" { "Instruments" } else { "Audio Effects" },
        label: format!("{name} {}", &digest[..12]),
        plugin: product.name.clone(),
        kind: format.info.kind.clone(),
        class_id: preset.class_id,
    })
}

/// Live's configured User Library, checked as a place to write: absolute, local, there.
fn check_library(library: &Path) -> Result<(), String> {
    let shown = library.to_string_lossy();
    if shown.chars().any(char::is_control) || kumi_common::path::reserved_device(&shown) {
        return Err("Use a local User Library path without control characters or reserved device names.".into());
    }
    if !library.is_absolute() {
        return Err("The User Library path must be absolute".into());
    }
    if network_or_device(&shown) {
        return Err("Use a local User Library path; network and device paths are not supported".into());
    }
    if !library.is_dir() {
        return Err("Live's configured User Library is missing. Open Live, check its Library location, then restart Kumi.".into());
    }
    Ok(())
}

/// Whether `library` already holds a file of `plan`'s name with other bytes (or one Kumi can't read): what [`write`]
/// would refuse, said by a preview before anything is written.
pub fn taken(plan: &PresetPlan, bytes: &[u8], library: &Path) -> bool {
    let file = library.join("Kumi").join(plan.folder).join(format!("{}.vstpreset", plan.label));
    std::fs::symlink_metadata(&file).is_ok() && !same_bytes(&file, bytes).unwrap_or(false)
}

/// The names of the plug-ins Live's Browser lists for a query, as `live_browser_search` answers them in the plug-ins
/// category. The search is fuzzy (a query for "Pro-Q 4" also finds "Massive Passive EQ"), so only an exact name counts.
/// Live groups plug-ins by maker, one row each, with no format in the path (26-s4's probe, LAB-SCHEDULE row 24a).
pub type PluginsNamed = Rc<dyn Fn(String, Signal) -> LocalBoxFuture<'static, Result<Vec<String>, RuntimeError>>>;

/// Live's Browser as serve's `preset` (CC-26) asks it: which plug-ins it lists, and whether it lists a file yet
/// (`options.browser_sees`, its wait and the conversation's [`crate::devices::tool::Made`]).
pub struct Browser {
    pub plugins: PluginsNamed,
    pub options: DeviceToolOptions,
}

/// What placing `bytes` as the preset `name` would write in `library`: nothing is written. The findings refuse it.
/// The library's path is checked as text first (local, not a network or device path), then its Kumi folders as plain
/// folders inside it, and only then is the file's place read (agent-m's review of #1180).
pub fn preview(library: &Path, bytes: &[u8], name: &str) -> Result<PresetPreview, Vec<String>> {
    let plan = plan(bytes, name).map_err(|why| vec![why])?;
    check_library(library).map_err(|why| vec![why])?;
    plain_folders(library, plan.folder).map_err(|why| vec![why])?;
    if taken(&plan, bytes, library) {
        return Err(vec![format!("{} is already in your User Library with other contents: name the preset differently.", plan.file())]);
    }
    Ok(PresetPreview {
        name: plan.label.clone(),
        file: plan.file(),
        plugin: plan.plugin.clone(),
        class_id: plan.class_id.clone(),
        kind: plan.kind,
    })
}

/// Places `bytes` as the preview named them (`file`), once Live's Browser lists the plug-in by its exact name, then
/// waits for the Browser to list the file. Nothing is written when it's refused. Live's Browser doesn't say a plug-in's
/// format; a `.vstpreset` loads only the VST3, so the device read back after the load is the last check (26d). Once the
/// file is written the answer says so: a cancelled or failed wait for the Browser is `indexed: false`, never an error.
pub async fn place_approved(
    browser: &Browser,
    bytes: Vec<u8>,
    name: &str,
    file: &str,
    signal: &Signal,
) -> Result<Result<PlacedPreset, PresetRefusal>, RuntimeError> {
    let refusal = |code, reason: String| Ok(Err(PresetRefusal { code, reason }));
    let library = PathBuf::from(&browser.options.user_library);
    check_library(&library).map_err(RuntimeError::plain)?;
    let plan = match plan(&bytes, name) {
        Ok(plan) => plan,
        Err(why) => return refusal("refused", why),
    };
    let changed = || format!("{file} isn't the file this preset would be written as now: preview it again.");
    if plan.file() != file {
        return refusal("changed", changed());
    }
    // The folders before the file: nothing is read through a link (or a junction to a network share) it would refuse.
    if let Err(why) = plain_folders(&library, plan.folder) {
        return refusal("refused", why);
    }
    if taken(&plan, &bytes, &library) {
        return refusal("changed", changed());
    }
    let names = (browser.plugins)(plan.plugin.clone(), signal.clone()).await?;
    if !names.iter().any(|listed| listed.trim().eq_ignore_ascii_case(&plan.plugin)) {
        return refusal("not_installed", format!("{} isn't in Live's plug-ins. Install or enable its VST3, then ask again.", plan.plugin));
    }
    signal.check()?;
    let planned = plan.clone();
    let written = tokio::task::spawn_blocking(move || match contained_folder(&library, planned.folder) {
        Ok(directory) => write_in(&planned, &bytes, &directory).map_err(|why| (false, why)),
        Err(why) => Err((true, why)),
    })
    .await
    .map_err(|e| RuntimeError::plain(format!("Placing the preset failed: {e}")))?;
    let made = match written {
        Ok(made) => made,
        Err((true, why)) => return refusal("refused", why),
        // Another writer took the name between the check above and the write: nothing of ours was written.
        Err((_, why)) if why == DIFFERENT_BYTES => return refusal("changed", changed()),
        Err((_, why)) => return Err(RuntimeError::plain(why)),
    };
    // Written: its first load waits for the Browser (`load_device`) until Live lists it, so it never loads by path
    // (26-r5). Live releases differ on whether a VST3 preset's Browser name keeps its extension: either is the file.
    let candidates = [made.item.clone(), made.item.trim_end_matches(".vstpreset").to_string()];
    candidates.iter().for_each(|candidate| browser.options.made.borrow_mut().written_unlisted(candidate));
    let deadline = std::time::Instant::now() + Duration::from_millis(browser.options.wait_ms.unwrap_or(20_000));
    let mut item = None;
    while !signal.is_cancelled() {
        for candidate in &candidates {
            if (browser.options.browser_sees)(candidate.clone(), signal.clone()).await.unwrap_or(false) {
                item = Some(candidate.clone());
                break;
            }
        }
        if item.is_some() || std::time::Instant::now() >= deadline {
            break;
        }
        tokio::select! { _ = signal.cancelled() => {}, _ = tokio::time::sleep(Duration::from_millis(400)) => {} }
    }
    if item.is_some() {
        candidates.iter().for_each(|candidate| browser.options.made.borrow_mut().listed(candidate));
    }
    Ok(Ok(PlacedPreset {
        name: made.name,
        item: item.clone().unwrap_or(made.item),
        file: plan.file(),
        indexed: item.is_some(),
        plugin: made.plugin,
        class_id: made.class_id,
    }))
}

/// Whether a folder entry is a link Kumi won't write through: a symlink, or on Windows any reparse point (a junction).
fn linked(metadata: &std::fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
        if metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return true;
        }
    }
    metadata.file_type().is_symlink()
}

fn not_plain(part: &Path) -> String {
    format!(
        "User Library/{} isn't a plain folder in your User Library (a link or junction leads elsewhere): Kumi writes presets only into the Kumi folder itself. Make it a plain folder, then ask again.",
        part.display()
    )
}

/// Whether the parts of `Kumi/<folder>` that exist under `library` are plain folders inside it, reading only.
fn plain_folders(library: &Path, folder: &str) -> Result<(), String> {
    let root = library.canonicalize().map_err(|e| e.to_string())?;
    let (mut path, mut relative) = (library.to_path_buf(), PathBuf::new());
    for part in ["Kumi", folder] {
        path.push(part);
        relative.push(part);
        match std::fs::symlink_metadata(&path) {
            Ok(metadata) if linked(&metadata) || !metadata.is_dir() => return Err(not_plain(&relative)),
            Ok(_) if path.canonicalize().map_err(|e| e.to_string())? != root.join(&relative) => return Err(not_plain(&relative)),
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(e) => return Err(e.to_string()),
        }
    }
    Ok(())
}

/// `library/Kumi/<folder>`, made one level at a time where missing, each part checked as a plain folder inside the
/// library once it's there (made just now or by someone else meanwhile): where an approved preset is written.
fn contained_folder(library: &Path, folder: &str) -> Result<PathBuf, String> {
    let root = library.canonicalize().map_err(|e| e.to_string())?;
    let (mut path, mut relative) = (library.to_path_buf(), PathBuf::new());
    for part in ["Kumi", folder] {
        path.push(part);
        relative.push(part);
        match std::fs::create_dir(&path) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(format!("Kumi couldn't write Live's User Library; check its location and folder permissions: {e}")),
        }
        let metadata = std::fs::symlink_metadata(&path).map_err(|e| e.to_string())?;
        if linked(&metadata) || !metadata.is_dir() || path.canonicalize().map_err(|e| e.to_string())? != root.join(&relative) {
            return Err(not_plain(&relative));
        }
    }
    Ok(path)
}

pub fn prepare(input: &Path, library: &Path, name: &str) -> Result<PreparedPreset, String> {
    if [input, library]
        .iter()
        .any(|path| path.to_string_lossy().chars().any(char::is_control) || kumi_common::path::reserved_device(&path.to_string_lossy()))
    {
        return Err("Use local preset and User Library paths without control characters or reserved device names.".into());
    }
    if !input.is_absolute() || !library.is_absolute() {
        return Err("Preset and User Library paths must be absolute".into());
    }
    if network_or_device(&input.to_string_lossy()) || network_or_device(&library.to_string_lossy()) {
        return Err("Use local preset and User Library paths; network and device paths are not supported".into());
    }
    if !library.is_dir() {
        return Err("Live's configured User Library is missing. Open Live, check its Library location, then restart Kumi.".into());
    }
    if !input.extension().is_some_and(|ext| ext.eq_ignore_ascii_case("vstpreset")) {
        return Err("Give a .vstpreset produced by vst3 create/edit".into());
    }
    check_name(name)?;
    let metadata = std::fs::symlink_metadata(input).map_err(|e| e.to_string())?;
    if !metadata.is_file() || metadata.len() > MAX_DECODED_BYTES as u64 {
        return Err("Preset input must be a regular file within the supported size limit".into());
    }
    let options = reading();
    let source = options.open(input).map_err(|e| e.to_string())?;
    let metadata = source.metadata().map_err(|e| e.to_string())?;
    if !metadata.is_file() || metadata.len() > MAX_DECODED_BYTES as u64 {
        return Err("Preset input must be a regular file within the supported size limit".into());
    }
    let mut bytes = Vec::new();
    source.take(MAX_DECODED_BYTES as u64 + 1).read_to_end(&mut bytes).map_err(|e| e.to_string())?;
    let plan = plan(&bytes, name)?;
    write(&plan, &bytes, library)
}

/// Opens a file to read without following a symlink or waiting on a FIFO.
fn reading() -> std::fs::OpenOptions {
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NONBLOCK | libc::O_NOFOLLOW);
    }
    options
}

/// Standalone's `prepare-preset`: makes the folder as it always has, then writes the file.
fn write(plan: &PresetPlan, bytes: &[u8], library: &Path) -> Result<PreparedPreset, String> {
    let directory = library.join("Kumi").join(plan.folder);
    std::fs::create_dir_all(&directory)
        .map_err(|e| format!("Kumi couldn't write Live's User Library; check its location and folder permissions: {e}"))?;
    write_in(plan, bytes, &directory)
}

/// Writes `bytes` as `plan`'s file in `directory`: a new file, or one already there with exactly these bytes.
fn write_in(plan: &PresetPlan, bytes: &[u8], directory: &Path) -> Result<PreparedPreset, String> {
    let file = directory.join(format!("{}.vstpreset", plan.label));
    match std::fs::OpenOptions::new().write(true).create_new(true).open(&file) {
        Ok(mut out) => {
            if let Err(e) = out.write_all(bytes).and_then(|_| out.sync_all()) {
                drop(out);
                let _ = std::fs::remove_file(&file);
                return Err(format!(
                    "Kumi couldn't finish saving the preset in Live's User Library; check free disk space and folder permissions: {e}"
                ));
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            // A concurrent copy or a repeated request is safe only if its bytes are still ours.
            if !same_bytes(&file, bytes)? {
                return Err(DIFFERENT_BYTES.into());
            }
        }
        Err(e) => return Err(format!("Kumi couldn't write Live's User Library; check its folder permissions: {e}")),
    }
    Ok(PreparedPreset {
        file,
        item: plan.item(),
        name: plan.label.clone(),
        plugin: plan.plugin.clone(),
        kind: plan.kind.clone(),
        class_id: plan.class_id.clone(),
    })
}

/// Whether `file` is a regular file holding exactly `bytes`; an error only when it can't be read.
fn same_bytes(file: &Path, bytes: &[u8]) -> Result<bool, String> {
    let metadata = std::fs::symlink_metadata(file).map_err(|e| e.to_string())?;
    if !metadata.is_file() || metadata.len() != bytes.len() as u64 {
        return Ok(false);
    }
    let mut existing = Vec::new();
    reading().open(file).map_err(|e| e.to_string())?.take(bytes.len() as u64 + 1).read_to_end(&mut existing).map_err(|e| e.to_string())?;
    Ok(existing == bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_fabfilter_vst3_template_stages_with_its_kind_and_exact_state() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("input.vstpreset");
        let mut effects = 0;
        let mut instruments = 0;
        for format in structure::builtins().iter().filter(|format| format.info.family == "fabfilter") {
            let bytes = kumi_library::plugins::templates::initial(&format.info.plugin).unwrap();
            std::fs::write(&source, bytes).unwrap();
            let made = prepare(&source, temp.path(), &format.info.plugin).unwrap();
            let folder = if format.info.kind == "instrument" {
                instruments += 1;
                "Instruments"
            } else {
                effects += 1;
                "Audio Effects"
            };
            assert_eq!(made.kind, format.info.kind);
            assert!(format.has_id(&made.class_id));
            assert_eq!(made.item, format!("user_library/Kumi/{folder}/{}.vstpreset", made.name));
            assert_eq!(std::fs::read(&made.file).unwrap(), bytes, "{}: processor/controller state changed", format.info.plugin);
            assert_eq!(prepare(&source, temp.path(), &format.info.plugin).unwrap().file, made.file);
        }
        assert!(effects >= 22 && instruments >= 3, "all 25 current and legacy products must have the correct Live route");
    }

    #[test]
    fn stages_exact_vst3_state_in_the_right_live_folder_and_reuses_only_identical_bytes() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("input.vstpreset");
        let bytes = include_bytes!(concat!(env!("KUMI_WORKSPACE"), "/plugin-formats/serum-2/format-1/fixtures/init.vstpreset"));
        std::fs::write(&source, bytes).unwrap();
        let made = prepare(&source, temp.path(), "Kumi synth").unwrap();
        assert!(made.item.starts_with("user_library/Kumi/Instruments/"));
        assert_eq!(made.item, format!("user_library/Kumi/Instruments/{}.vstpreset", made.name));
        assert_eq!(std::fs::read(&made.file).unwrap(), bytes);
        assert_eq!(prepare(&source, temp.path(), "Kumi synth").unwrap().file, made.file);
        std::fs::write(&made.file, b"someone else's preset").unwrap();
        assert!(prepare(&source, temp.path(), "Kumi synth").is_err());
        assert!(prepare(&source, temp.path(), "../escape").is_err());
        assert!(prepare(&source, &temp.path().join("missing"), "valid").unwrap_err().contains("Library is missing"));
        assert!(!temp.path().join("missing").exists());
        let blocked = tempfile::tempdir().unwrap();
        std::fs::write(blocked.path().join("Kumi"), b"not a folder").unwrap();
        let error = prepare(&source, blocked.path(), "valid").unwrap_err();
        assert!(error.contains("User Library") && error.contains("permissions"), "{error}");
        assert!(prepare(Path::new("//host/share/test.vstpreset"), temp.path(), "valid").is_err());
        for path in ["\0//host/share/x.vstpreset", "/tmp/x\n.vstpreset"] {
            assert!(prepare(Path::new(path), temp.path(), "valid").unwrap_err().contains("local"));
            assert!(prepare(&source, Path::new(path), "valid").unwrap_err().contains("local"));
        }
        #[cfg(windows)]
        assert!(prepare(Path::new("C:/P/CON.vstpreset"), temp.path(), "valid").unwrap_err().contains("local"));
    }

    #[cfg(unix)]
    #[test]
    fn unix_preset_paths_allow_colons_and_dos_device_directory_names() {
        let temp = tempfile::tempdir().unwrap();
        let directory = temp.path().join("Aux");
        std::fs::create_dir(&directory).unwrap();
        let source = directory.join("X:Y.vstpreset");
        let bytes = include_bytes!(concat!(env!("KUMI_WORKSPACE"), "/plugin-formats/serum-2/format-1/fixtures/init.vstpreset"));
        std::fs::write(&source, bytes).unwrap();
        let made = prepare(&source, &directory, "Kumi synth").unwrap();
        assert_eq!(std::fs::read(made.file).unwrap(), bytes);
    }

    const SERUM: &[u8] = include_bytes!(concat!(env!("KUMI_WORKSPACE"), "/plugin-formats/serum-2/format-1/fixtures/init.vstpreset"));

    /// Live's Browser listing the plug-ins `names` for any query, and listing a file once `lists_files`; every plug-in
    /// search is kept.
    fn browser(library: &Path, names: &[&str], lists_files: bool) -> (Browser, Rc<std::cell::RefCell<Vec<String>>>) {
        let rows: Vec<String> = names.iter().map(|name| name.to_string()).collect();
        let searched = Rc::new(std::cell::RefCell::new(Vec::new()));
        let asked = searched.clone();
        let browser = Browser {
            plugins: Rc::new(move |name, _| {
                asked.borrow_mut().push(name);
                let rows = rows.clone();
                futures::FutureExt::boxed_local(async move { Ok(rows) })
            }),
            options: DeviceToolOptions {
                user_library: library.to_string_lossy().into_owned(),
                browser_sees: Rc::new(move |_, _| futures::FutureExt::boxed_local(async move { Ok(lists_files) })),
                wait_ms: Some(0),
                made: Default::default(),
            },
        };
        (browser, searched)
    }

    #[tokio::test]
    async fn an_approved_preset_is_placed_as_previewed_only_where_live_lists_its_plug_in_by_name() {
        let temp = tempfile::tempdir().unwrap();
        let kumi = temp.path().join("Kumi");
        let preview = preview(temp.path(), SERUM, "Night Bass").unwrap();
        assert_eq!(preview.file, format!("User Library/Kumi/Instruments/{}.vstpreset", preview.name));
        assert!(!kumi.exists(), "a preview writes nothing");

        // Live's search is fuzzy: "Serum 2 FX" for "Serum 2" isn't Serum 2, so nothing is written.
        let (near, _) = browser(temp.path(), &["Serum 2 FX"], true);
        let refused = place_approved(&near, SERUM.to_vec(), "Night Bass", &preview.file, &Signal::new()).await.unwrap().unwrap_err();
        assert_eq!(
            (refused.code, refused.reason.as_str()),
            ("not_installed", "Serum 2 isn't in Live's plug-ins. Install or enable its VST3, then ask again.")
        );
        assert!(!kumi.exists());

        // Another name than the one approved is another file: refused before Live is asked.
        let (vst3, searched) = browser(temp.path(), &["Serum 2"], true);
        assert_eq!(
            place_approved(&vst3, SERUM.to_vec(), "Night Bass 2", &preview.file, &Signal::new()).await.unwrap().unwrap_err().code,
            "changed"
        );
        assert!(searched.borrow().is_empty() && !kumi.exists());

        let placed = place_approved(&vst3, SERUM.to_vec(), "Night Bass", &preview.file, &Signal::new()).await.unwrap().unwrap();
        assert_eq!((placed.file.as_str(), placed.name.as_str(), placed.indexed), (preview.file.as_str(), preview.name.as_str(), true));
        assert_eq!(placed.item, format!("user_library/Kumi/Instruments/{}.vstpreset", preview.name));
        assert_eq!(std::fs::read(kumi.join("Instruments").join(format!("{}.vstpreset", preview.name))).unwrap(), SERUM);
        assert!(!vst3.options.made.borrow().unlisted(&placed.item));
        // The same bytes again are the same file, never a second one.
        assert!(place_approved(&vst3, SERUM.to_vec(), "Night Bass", &preview.file, &Signal::new()).await.unwrap().is_ok());
        assert_eq!(std::fs::read_dir(kumi.join("Instruments")).unwrap().count(), 1);
    }

    #[tokio::test]
    async fn a_previewed_file_taken_by_other_bytes_or_not_yet_listed_is_said() {
        let temp = tempfile::tempdir().unwrap();
        let preview = preview(temp.path(), SERUM, "Night Bass").unwrap();
        let rows = ["Serum 2"];

        // Live hasn't listed the file by the deadline: it's placed, and its first load waits for the Browser.
        let (slow, _) = browser(temp.path(), &rows, false);
        let placed = place_approved(&slow, SERUM.to_vec(), "Night Bass", &preview.file, &Signal::new()).await.unwrap().unwrap();
        assert!(!placed.indexed);
        assert!(slow.options.made.borrow().unlisted(&placed.item));

        // Someone else's file under that name since: the preview says so, and the approved place writes nothing.
        let file = temp.path().join("Kumi/Instruments").join(format!("{}.vstpreset", preview.name));
        std::fs::write(&file, b"someone else's preset").unwrap();
        assert!(super::preview(temp.path(), SERUM, "Night Bass").unwrap_err()[0].contains("already in your User Library"));
        let (vst3, _) = browser(temp.path(), &rows, true);
        assert_eq!(
            place_approved(&vst3, SERUM.to_vec(), "Night Bass", &preview.file, &Signal::new()).await.unwrap().unwrap_err().code,
            "changed"
        );
        assert_eq!(std::fs::read(&file).unwrap(), b"someone else's preset");

        // Bytes that aren't a catalogued plug-in's preset are refused by the preview and by the place.
        assert!(super::preview(temp.path(), b"not a preset", "Night Bass").is_err());
        assert_eq!(
            place_approved(&vst3, b"not a preset".to_vec(), "Night Bass", &preview.file, &Signal::new()).await.unwrap().unwrap_err().code,
            "refused"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_kumi_folder_that_links_elsewhere_is_refused_and_nothing_is_written_outside() {
        // agent-m's review of #1180: the approval names only User Library/Kumi, so a link there never takes a write.
        // A User Library on a network share is refused from its path's text, before anything is read there.
        let shared = preview(Path::new("//host/share/User Library"), SERUM, "Night Bass").unwrap_err();
        assert!(shared[0].contains("network"), "{shared:?}");
        for linked in ["Kumi", "Kumi/Instruments"] {
            let (temp, outside) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
            let library = temp.path().join("User Library");
            std::fs::create_dir_all(library.join("Kumi")).unwrap();
            if linked == "Kumi" {
                std::fs::remove_dir(library.join("Kumi")).unwrap();
            }
            std::os::unix::fs::symlink(outside.path(), library.join(linked)).unwrap();
            let refused = preview(&library, SERUM, "Night Bass").unwrap_err();
            assert!(refused[0].contains(&format!("User Library/{linked} isn't a plain folder")), "{linked}: {refused:?}");
            let file = format!("User Library/Kumi/Instruments/Night Bass {}.vstpreset", &hex::encode(Sha256::digest(SERUM))[..12]);
            let (vst3, _) = browser(&library, &["Serum 2"], true);
            let placed = place_approved(&vst3, SERUM.to_vec(), "Night Bass", &file, &Signal::new()).await.unwrap().unwrap_err();
            assert_eq!(placed.code, "refused", "{linked}");
            let mut written = Vec::new();
            for entry in walkdir(outside.path()) {
                written.push(entry);
            }
            assert!(written.is_empty(), "{linked}: wrote outside the User Library: {written:?}");
        }
    }

    /// Every file under `folder`.
    fn walkdir(folder: &Path) -> Vec<PathBuf> {
        let mut found = Vec::new();
        for entry in std::fs::read_dir(folder).into_iter().flatten().flatten() {
            if entry.path().is_dir() {
                found.extend(walkdir(&entry.path()));
            } else {
                found.push(entry.path());
            }
        }
        found
    }

    #[tokio::test]
    async fn a_wait_cancelled_after_the_write_says_the_file_is_there_and_not_yet_listed() {
        let temp = tempfile::tempdir().unwrap();
        let preview = preview(temp.path(), SERUM, "Night Bass").unwrap();
        let (mut cancelled, _) = browser(temp.path(), &["Serum 2"], true);
        // The conversation is cancelled while Kumi waits for Live's Browser, after the file is written.
        cancelled.options.browser_sees = Rc::new(|_, signal: Signal| {
            signal.cancel();
            futures::FutureExt::boxed_local(async { Ok(false) })
        });
        let placed = place_approved(&cancelled, SERUM.to_vec(), "Night Bass", &preview.file, &Signal::new()).await.unwrap().unwrap();
        assert!(!placed.indexed);
        assert!(cancelled.options.made.borrow().unlisted(&placed.item), "its first load waits for Live's Browser");
        assert_eq!(std::fs::read(temp.path().join("Kumi/Instruments").join(format!("{}.vstpreset", preview.name))).unwrap(), SERUM);
    }
}
