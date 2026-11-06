//! Logic's content libraries and manual, available without a running DAW. Discovery reads only existing content
//! folders; native recent-project menus and the inside of Logic projects remain outside this adapter.
use std::collections::HashSet;
use std::path::{Path, PathBuf};

use kumi_library::daws::{Layout, LibrarySources, Places};
use kumi_library::presets::{PresetCategory, PresetFacts};
use kumi_library::sources::{home_of, platform_of, Source, SourceKind, SourceOptions};
use kumi_types::daw::{DawId, DawProfile};

pub struct LogicLibrary;
pub static LIBRARY: LogicLibrary = LogicLibrary;

const PRESETS: &[&str] = &[".patch", ".pst", ".cst", ".exs", ".aupreset"];
static LAYOUT: Layout = Layout {
    folders: &[
        ("patches", &[".patch"]),
        ("sampler instruments", &[".exs"]),
        ("channel strip settings", &[".cst"]),
        ("plug-in settings", &[".pst", ".aupreset"]),
        ("presets", &[".aupreset"]),
    ],
    sounds: &["samples", "untagged loops", "exs factory samples", "alchemy samples", "ultrabeat samples", "apple loops"],
    formats: &[],
};

/// Preserve source order while collapsing filesystem aliases. Canonical outward names also let no-follow package
/// walks start at the real content root rather than traverse a symlink used to relocate a library.
fn existing(paths: impl IntoIterator<Item = (PathBuf, &'static str)>) -> Vec<Source> {
    let mut seen = HashSet::new();
    paths
        .into_iter()
        .filter_map(|(path, label)| {
            let path = path.canonicalize().ok().filter(|path| path.is_dir())?;
            let has_content = std::fs::read_dir(&path).ok()?.take(4096).filter_map(Result::ok).any(|entry| {
                let name = entry.file_name();
                LAYOUT
                    .folders
                    .iter()
                    .map(|(folder, _)| *folder)
                    .chain(LAYOUT.sounds.iter().copied())
                    .any(|folder| name.to_string_lossy().eq_ignore_ascii_case(folder))
                    && entry.path().is_dir()
            });
            if !has_content || !seen.insert(path.clone()) {
                return None;
            }
            Some(Source { path: path.to_string_lossy().into_owned(), label: label.into(), kind: SourceKind::Daw(DawId::Logic) })
        })
        .collect()
}

fn roots(options: &SourceOptions) -> (Vec<Source>, Vec<Source>) {
    if platform_of(options) != "darwin" || options.approved_folders.is_some() {
        return (vec![], vec![]);
    }
    // A deliberately injected platform/home never searches another machine's real home or shared Library.
    if options.home.is_none() && options.platform.is_some() {
        return (vec![], vec![]);
    }
    let home = PathBuf::from(home_of(options));
    let user = existing([
        (home.join("Music/Audio Music Apps"), "Logic User Library"),
        (home.join("Library/Audio"), "Audio presets and loops (yours)"),
    ]);
    let injected = options.home.is_some() || options.platform.is_some() || options.env.is_some();
    let system = options.system_library.as_deref().or((!injected).then_some("/Library"));
    let factory = system
        .map(|library| {
            existing([
                (Path::new(library).join("Application Support/Logic"), "Logic Library"),
                (Path::new(library).join("Audio"), "Audio presets and loops"),
            ])
        })
        .unwrap_or_default();
    (user, factory)
}

impl LibrarySources for LogicLibrary {
    fn manual_tool(&self, options: kumi_library::daws::ManualOptions) -> Option<std::rc::Rc<dyn kumi_types::contracts::KernelTool>> {
        Some(crate::manual::manual_tool(options))
    }

    fn profile(&self) -> &'static DawProfile {
        &DawProfile::LOGIC
    }

    fn places(&self, options: &SourceOptions) -> Places {
        let (before, mut after) = roots(options);
        if platform_of(options) == "darwin" && options.approved_folders.is_none() && (options.home.is_some() || options.platform.is_none())
        {
            let splice = PathBuf::from(home_of(options)).join("Splice");
            if let Some(path) = splice.canonicalize().ok().filter(|path| path.is_dir()) {
                after.insert(0, Source { path: path.to_string_lossy().into_owned(), label: "Splice".into(), kind: SourceKind::Splice });
            }
        }
        Places { before, after }
    }

    fn own_library(&self, options: &SourceOptions) -> Vec<Source> {
        let (user, factory) = roots(options);
        let mut seen = HashSet::new();
        user.into_iter().chain(factory).filter(|source| seen.insert(source.path.clone())).collect()
    }

    fn layout(&self) -> &'static Layout {
        &LAYOUT
    }

    fn sample_extensions(&self) -> &'static [&'static str] {
        &[".caf"]
    }

    fn preset_extensions(&self) -> &'static [&'static str] {
        PRESETS
    }

    fn preset_packages(&self) -> &'static [&'static str] {
        &[".patch"]
    }

    fn preset_facts(&self, format: &str, relative: &str) -> Option<PresetFacts> {
        let parts: Vec<_> = relative.split(['\\', '/']).filter(|part| !part.is_empty()).collect();
        Some(match format {
            "pst" | "aupreset" => {
                let parent = parts.len().checked_sub(2).map(|at| parts[at]);
                let device = parent
                    .filter(|parent| !["plug-in settings", "presets"].iter().any(|root| parent.eq_ignore_ascii_case(root)))
                    .map(str::to_string);
                PresetFacts { category: Some(PresetCategory::Plugin), device, ..Default::default() }
            }
            "exs" => PresetFacts { category: Some(PresetCategory::Instrument), device: Some("Sampler".into()), ..Default::default() },
            "patch" | "cst" => PresetFacts::default(),
            _ => return None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kumi_common::abort::Signal;
    use kumi_library::daws::Daws;
    use kumi_library::learn::{learn, library_logs, LearnOptions, LearnPhase, LearnPlan};
    use kumi_library::plan::{plan_learning, PlanOptions};
    use kumi_library::sources::library_sources;
    use std::cell::Cell;
    use std::rc::Rc;

    static DAWS: [&'static dyn LibrarySources; 1] = [&LIBRARY];

    fn put(path: &Path, body: &[u8]) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, body).unwrap();
    }

    fn options(root: &Path) -> SourceOptions {
        SourceOptions {
            home: Some(root.join("home").to_string_lossy().into_owned()),
            system_library: Some(root.join("Library").to_string_lossy().into_owned()),
            platform: Some("darwin".into()),
            daw: Some(DawId::Logic),
            ..Default::default()
        }
    }

    #[test]
    fn discovery_uses_only_existing_injected_content_roots_and_respects_exclusive_approval() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        put(&root.join("home/Music/Audio Music Apps/Samples/Kick.caf"), &[0; 128]);
        put(&root.join("home/Library/Audio/Presets/EQ/Voice.aupreset"), &[0; 128]);
        put(&root.join("Library/Application Support/Logic/Patches/Voice.patch/Voice.cst"), &[0; 128]);
        put(&root.join("Library/Audio/Apple Loops/Apple/Beat.caf"), &[0; 128]);
        let options = options(&root);
        let sources = library_sources(Daws(&DAWS), &options);
        assert_eq!(sources.len(), 4, "{sources:?}");
        assert!(sources.iter().all(|source| source.kind == SourceKind::Daw(DawId::Logic)));
        assert!(sources.iter().all(|source| Path::new(&source.path).starts_with(&root)));
        assert_eq!(sources[0].label, "Logic User Library");
        assert!(!sources.iter().any(|source| source.path.contains("Ableton")));
        let mut non_mac = options.clone();
        non_mac.platform = Some("linux".into());
        assert!(LIBRARY.own_library(&non_mac).is_empty());
        let mut restricted = options.clone();
        restricted.approved_folders = Some(vec![]);
        assert!(library_sources(Daws(&DAWS), &restricted).is_empty());
        assert!(LIBRARY.own_library(&restricted).is_empty());
        let mut user_only = options;
        user_only.system_library = None;
        assert_eq!(LIBRARY.own_library(&user_only).len(), 2, "an injected home never falls back to real /Library");
        assert!(LIBRARY.own_library(&SourceOptions { platform: Some("darwin".into()), ..Default::default() }).is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn discovered_aliases_are_one_canonical_library() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let shared = root.join("Shared");
        put(&shared.join("Samples/Kick.wav"), &[0; 128]);
        std::fs::create_dir_all(root.join("home/Music")).unwrap();
        std::os::unix::fs::symlink(&shared, root.join("home/Music/Audio Music Apps")).unwrap();
        let mut options = options(&root);
        options.system_library = Some(root.join("System").to_string_lossy().into_owned());
        std::fs::create_dir_all(root.join("System/Application Support")).unwrap();
        std::os::unix::fs::symlink(&shared, root.join("System/Application Support/Logic")).unwrap();
        let sources = LIBRARY.own_library(&options);
        assert_eq!(sources.len(), 1);
        assert_eq!(sources[0].path, shared.to_string_lossy());
    }

    #[tokio::test]
    async fn logic_content_is_indexed_as_presets_and_packages_without_entering_package_children() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let content = root.join("home/Music/Audio Music Apps");
        put(&content.join("Patches/Keys/Soft.patch/Inside.cst"), &[1; 128]);
        put(&content.join("Plug-In Settings/Channel EQ/Bright.pst"), &[2; 128]);
        put(&content.join("Channel Strip Settings/Bus/Warm.cst"), &[3; 128]);
        put(&content.join("Sampler Instruments/Kit.exs"), &[4; 128]);
        put(&root.join("home/Library/Audio/Presets/EQ/Voice.aupreset"), &[5; 128]);
        put(&content.join("Samples/Kick.caf"), &[0; 128]);
        put(&content.join("Samples/Snare.wav"), &[0; 128]);
        put(&content.join("Samples/Not Logic.adg"), &[0; 128]);
        let plan = plan_learning(PlanOptions {
            dir: root.join("index").to_string_lossy().into_owned(),
            sources: Some(options(&root)),
            daws: Daws(&DAWS),
            find_sets: Some(false),
            workers: Some(0),
            ..Default::default()
        })
        .await;
        let (phase, sounds) = (Rc::new(Cell::new(LearnPhase::Looking)), Rc::new(Cell::new(0)));
        let signal = Signal::new();
        let mut learning = LearnOptions::new(plan.clone(), signal.clone());
        learning.daws = Daws(&DAWS);
        let (seen, counted) = (phase.clone(), sounds.clone());
        learning.on_progress = Some(Rc::new(move |progress| {
            seen.set(progress.phase);
            counted.set(progress.sounds.todo);
        }));
        learning.gate = Some(Rc::new(move || {
            if phase.get() == LearnPhase::Sounds {
                signal.cancel();
            }
            Box::pin(async {})
        }));
        assert!(learn(learning).await.unwrap_err().is_aborted(), "stop before invoking any audio measurement");
        assert_eq!(sounds.get(), 2, "CAF and WAV in Logic's own content, with no Live preset beside them");
        let presets = library_logs(&plan.dir).presets.load().await;
        assert_eq!(presets.len(), 5);
        let find = |format| presets.values().find(|preset| preset.format == format).unwrap();
        assert_eq!(find("patch").name, "Soft");
        assert_eq!(find("patch").size, 128);
        assert!(find("patch").package_fingerprint.is_some());
        assert!(presets.keys().all(|path| !path.ends_with("Inside.cst")));
        assert_eq!(find("pst").device.as_deref(), Some("Channel EQ"));
        assert_eq!(find("aupreset").device.as_deref(), Some("EQ"));
        assert_eq!(find("exs").device.as_deref(), Some("Sampler"));
        assert!(presets.values().all(|preset| preset.error.is_none() && preset.browser.is_none()));
    }

    #[tokio::test]
    async fn approved_package_child_edits_relearn_once_and_deletions_remove_the_atomic_entry() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let approved = root.join("Approved");
        let child = approved.join("Voice.patch/Voice.cst");
        put(&child, &[1; 128]);
        put(&root.join("Unapproved/Hidden.patch/Hidden.cst"), &[2; 128]);
        let mut source_options = options(&root);
        source_options.approved_folders = Some(vec![approved.to_string_lossy().into_owned()]);
        let plan = plan_learning(PlanOptions {
            dir: root.join("index").to_string_lossy().into_owned(),
            sources: Some(source_options),
            daws: Daws(&DAWS),
            workers: Some(0),
            ..Default::default()
        })
        .await;
        assert_eq!(plan.daw, Some(DawId::Logic));
        assert_eq!(plan.sources.len(), 1);
        let run = || LearnOptions { daws: Daws(&DAWS), ..LearnOptions::new(plan.clone(), Signal::new()) };
        assert_eq!(learn(run()).await.unwrap().presets.done, 1);
        let first = library_logs(&plan.dir).presets.load().await;
        let first = first.values().next().unwrap().package_fingerprint.clone();
        assert_eq!(learn(run()).await.unwrap().presets.done, 0);
        let metadata = std::fs::metadata(&child).unwrap();
        std::fs::write(&child, [2; 128]).unwrap();
        std::fs::File::options()
            .write(true)
            .open(&child)
            .unwrap()
            .set_times(std::fs::FileTimes::new().set_modified(metadata.modified().unwrap()))
            .unwrap();
        assert_eq!(learn(run()).await.unwrap().presets.done, 1);
        let changed = library_logs(&plan.dir).presets.load().await;
        assert_eq!(changed.len(), 1);
        assert_ne!(changed.values().next().unwrap().package_fingerprint, first);
        #[cfg(unix)]
        {
            let link = approved.join("Voice.patch/untrusted.cst");
            std::os::unix::fs::symlink(root.join("Unapproved/Hidden.patch/Hidden.cst"), &link).unwrap();
            assert_eq!(learn(run()).await.unwrap().presets.done, 0);
            let retained = library_logs(&plan.dir).presets.load().await;
            assert_eq!(retained.len(), 1, "an incomplete package scan must retain the prior atomic preset");
            assert_eq!(retained.values().next().unwrap().package_fingerprint, changed.values().next().unwrap().package_fingerprint);
            std::fs::remove_file(link).unwrap();
        }
        std::fs::remove_dir_all(approved.join("Voice.patch")).unwrap();
        learn(run()).await.unwrap();
        assert!(library_logs(&plan.dir).presets.load().await.is_empty());
    }

    #[test]
    fn extra_sample_formats_are_profile_specific_and_old_serialized_plans_default_to_live() {
        let daws = Daws(&DAWS);
        assert!(daws.accepts_sample(Some(DawId::Logic), ".caf"));
        assert!(!daws.accepts_sample(None, ".caf"));
        assert!(!daws.accepts_sample(Some(DawId::Ableton), ".caf"));
        assert!(daws.accepts_sample(None, ".wav"));
        let old = serde_json::json!({"dir": "index", "sources": []});
        assert_eq!(serde_json::from_value::<LearnPlan>(old).unwrap().daw, None);
    }

    #[tokio::test]
    async fn an_approved_source_that_is_itself_a_package_is_counted_once_without_entering_its_children() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let package = root.join("Approved.patch");
        put(&package.join("Internal.cst"), &[1; 128]);
        put(&package.join("Audio/Kick.wav"), &[2; 128]);
        let mut sources = options(&root);
        sources.approved_folders = Some(vec![package.to_string_lossy().into_owned()]);
        let plan = plan_learning(PlanOptions {
            dir: root.join("index").to_string_lossy().into_owned(),
            sources: Some(sources),
            daws: Daws(&DAWS),
            workers: Some(0),
            ..Default::default()
        })
        .await;
        assert_eq!(plan.sources.len(), 1);
        assert_eq!(plan.sources[0].path, package.to_string_lossy());
        let run = || LearnOptions { daws: Daws(&DAWS), ..LearnOptions::new(plan.clone(), Signal::new()) };
        let learned = learn(run()).await.unwrap();
        assert_eq!((learned.presets.done, learned.sounds.known, learned.sets.known), (1, 0, 0));
        let presets = library_logs(&plan.dir).presets.load().await;
        assert_eq!(presets.len(), 1);
        let preset = presets.values().next().unwrap();
        assert_eq!(preset.file.path, package.to_string_lossy());
        assert_eq!((preset.name.as_str(), preset.format.as_str(), preset.size), ("Approved", "patch", 256));
        assert!(preset.package_fingerprint.is_some());
        assert_eq!(learn(run()).await.unwrap().presets.done, 0);

        // Audio-only rebuilds skip an atomic package whether it is the approved root or a child of one. Adding a
        // profile-specific audio child must neither leak that sound nor refresh the package's saved preset facts.
        put(&package.join("Audio/Snare.caf"), &[3; 128]);
        let preserved: Vec<_> = ["presets.jsonl", "taste.json"]
            .into_iter()
            .map(|name| {
                let path = Path::new(&plan.dir).join(name);
                let bytes = std::fs::read(&path).unwrap();
                (path, bytes)
            })
            .collect();
        for approved in [&package, &root] {
            let mut sources = options(&root);
            sources.approved_folders = Some(vec![approved.to_string_lossy().into_owned()]);
            let audio_plan = plan_learning(PlanOptions {
                dir: plan.dir.clone(),
                sources: Some(sources),
                daws: Daws(&DAWS),
                audio_only: true,
                workers: Some(0),
                ..Default::default()
            })
            .await;
            assert_eq!(audio_plan.daw, Some(DawId::Logic));
            assert_eq!(audio_plan.sources.len(), 1);
            assert_eq!(audio_plan.sources[0].path, approved.to_string_lossy());
            let audio =
                learn(LearnOptions { daws: Daws(&DAWS), rebuild: true, ..LearnOptions::new(audio_plan, Signal::new()) }).await.unwrap();
            assert_eq!(audio.sounds, kumi_library::learn::Counts::default(), "approved root: {}", approved.display());
            assert_eq!(audio.presets, kumi_library::learn::Counts::default());
            assert_eq!(audio.sets, kumi_library::learn::Counts::default());
            assert_eq!(audio.failed, 0);
            for (path, bytes) in &preserved {
                assert_eq!(&std::fs::read(path).unwrap(), bytes, "{} changed for {}", path.display(), approved.display());
            }
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn registered_library_tools_find_logic_paths_and_keep_extra_audio_out_of_lives_index() {
        tokio::task::LocalSet::new()
            .run_until(async {
                use kumi_library::learn::SoundEntry;
                use kumi_library::store::Entry;
                use kumi_library::{create_library, LibraryOptions, LibraryToolsOptions};
                use serde_json::json;
                let temp = tempfile::tempdir().unwrap();
                let root = temp.path().canonicalize().unwrap();
                let approved = root.join("Approved");
                put(&approved.join("Soft.patch/Voice.cst"), &[1; 128]);
                put(&approved.join("Channel EQ/Bright.pst"), &[2; 128]);
                let mut sources = options(&root);
                sources.approved_folders = Some(vec![approved.to_string_lossy().into_owned()]);
                let dir = root.join("index").to_string_lossy().into_owned();
                let plan = plan_learning(PlanOptions {
                    dir: dir.clone(),
                    sources: Some(sources.clone()),
                    daws: Daws(&DAWS),
                    workers: Some(0),
                    ..Default::default()
                })
                .await;
                learn(LearnOptions { daws: Daws(&DAWS), ..LearnOptions::new(plan, Signal::new()) }).await.unwrap();
                // Seed already-measured facts: this read-side tool test must not invoke an audio decoder or worker.
                let mut measured = vec![];
                for extension in ["caf", "wav"] {
                    let path = approved.join(format!("Kick.{extension}"));
                    put(&path, &[0; 128]);
                    measured.push(SoundEntry {
                        file: Entry { path: path.to_string_lossy().into_owned(), size: 128, mtime: 1, gone: None },
                        ..Default::default()
                    });
                }
                library_logs(&dir).sounds.append(&measured).await.unwrap();
                let library = create_library(LibraryOptions { dir, sources: Some(sources), daws: Daws(&DAWS), ..Default::default() });
                let tools = library.tools_for(&DawProfile::LOGIC, LibraryToolsOptions::default());
                let preset_tool = tools.iter().find(|tool| tool.name() == "find_presets").unwrap();
                let answer = preset_tool.execute(json!({"words": ["Soft"]}).as_object().unwrap().clone(), Signal::new()).await.unwrap();
                assert!(!answer.is_error, "{}", answer.text);
                let answer: serde_json::Value = serde_json::from_str(&answer.text).unwrap();
                assert_eq!(answer["presets"].as_array().unwrap().len(), 1, "{answer}");
                assert_eq!(answer["presets"][0]["path"], approved.join("Soft.patch").to_string_lossy().as_ref());
                assert!(answer["presets"][0].get("browser").is_none(), "a Logic preset is a path, never a Live Browser item");
                let sound_tool = tools.iter().find(|tool| tool.name() == "find_sounds").unwrap();
                let answer = sound_tool.execute(json!({"words": ["Kick"]}).as_object().unwrap().clone(), Signal::new()).await.unwrap();
                assert!(!answer.is_error, "{}", answer.text);
                let answer: serde_json::Value = serde_json::from_str(&answer.text).unwrap();
                assert_eq!(answer["sounds"].as_array().unwrap().len(), 2, "{answer}");
                let live = library.tools_for(&DawProfile::ABLETON, LibraryToolsOptions::default());
                let sound_tool = live.iter().find(|tool| tool.name() == "find_sounds").unwrap();
                let answer = sound_tool.execute(json!({"words": ["Kick"]}).as_object().unwrap().clone(), Signal::new()).await.unwrap();
                let answer: serde_json::Value = serde_json::from_str(&answer.text).unwrap();
                assert_eq!(
                    answer["sounds"].as_array().unwrap().len(),
                    1,
                    "CAF learned for Logic never expands Live's format set: {answer}"
                );
                assert!(answer["sounds"][0]["path"].as_str().unwrap().ends_with(".wav"));
                library.close().await;
            })
            .await;
    }
}
