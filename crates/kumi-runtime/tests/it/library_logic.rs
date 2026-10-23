//! (M6) Logic Pro's library and places: its content folders (the producer's before Logic's own, Apple Loops, Splice's
//! and the folders named), never Live's; its projects where they usually are and the ones its bridge read from File >
//! Open Recent; its `.caf` sounds, its patches, settings and Sampler instruments as presets, and its `.logicx` packages
//! as projects, each counted whole; and the library's tools in its words, Live's layout as it was.
use kumi_common::abort::Signal;
use kumi_runtime::core::daw::{DawId, DawProfile};
use kumi_runtime::library::{
    create_library,
    learn::{learn, library_logs, LearnOptions, LearnPhase},
    plan::{plan_learning, recent_file, PlanOptions},
    sources::{SourceKind, SourceOptions},
    LibraryOptions, LibraryToolsOptions,
};
use serde_json::{json, Value};
use std::{cell::Cell, path::Path, rc::Rc};

fn put(path: &Path, body: impl AsRef<[u8]>) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, body).unwrap();
}

/// A Mac with Logic's content, Live's User Library beside it, Splice's folder and a folder the producer named; the
/// system's Library under `system`.
struct Mac {
    root: tempfile::TempDir,
}

impl Mac {
    fn new() -> Mac {
        let root = tempfile::tempdir().unwrap();
        let mac = Mac { root };
        let (home, system) = (mac.home(), mac.system());
        let user = home.join("Music/Audio Music Apps");
        put(&user.join("Samples/Drums/Kick 01.caf"), [0u8; 128]);
        put(&user.join("Samples/Drums/Snare 01.wav"), [0u8; 128]);
        put(&user.join("Patches/Vocals/Classic Vocal.patch/Classic Vocal.cst"), [0u8; 128]);
        put(&user.join("Plug-In Settings/Channel EQ/Bright Vocal.pst"), [0u8; 128]);
        put(&user.join("Sampler Instruments/Tight Kit.exs"), [0u8; 128]);
        put(&user.join("Channel Strip Settings/Audio Track/Warm Bus.cst"), [0u8; 128]);
        // (a Live preset in Logic's folder isn't Logic's to learn)
        put(&user.join("Samples/Old Rack.adg"), [0u8; 128]);
        put(&home.join("Music/Ableton/User Library/Samples/Live Kick.wav"), [0u8; 128]);
        put(&home.join("Splice/sounds/Hat.wav"), [0u8; 128]);
        put(&home.join("Crate/Clap.aif"), [0u8; 128]);
        put(&home.join("Music/Logic/Night Drive.logicx/Alternatives/000/ProjectData"), [0u8; 128]);
        put(&home.join("Music/Logic/Old Live Song.als"), [0u8; 128]);
        put(&home.join("Projects/Sunrise.logicx/Alternatives/000/ProjectData"), [0u8; 128]);
        put(&system.join("Audio/Apple Loops/Apple/01 Hip Hop/Beat.caf"), [0u8; 128]);
        put(&system.join("Application Support/Logic/EXS Factory Samples/Piano/C3.wav"), [0u8; 128]);
        put(&system.join("Application Support/Logic/Channel Strip Settings/Instrument/Lead.cst"), [0u8; 128]);
        mac
    }
    fn home(&self) -> std::path::PathBuf {
        self.root.path().join("home")
    }
    fn system(&self) -> std::path::PathBuf {
        self.root.path().join("Library")
    }
    fn dir(&self) -> std::path::PathBuf {
        self.root.path().join("home/.kumi/library")
    }
    fn options(&self, daw: Option<DawId>) -> SourceOptions {
        SourceOptions {
            home: Some(self.home().to_string_lossy().into()),
            platform: Some("darwin".into()),
            applications: Some(self.root.path().join("Applications").to_string_lossy().into()),
            system_library: Some(self.system().to_string_lossy().into()),
            folders: Some(vec![self.home().join("Crate").to_string_lossy().into()]),
            daw,
            ..Default::default()
        }
    }
}

#[test]
fn logics_places_are_its_content_folders_the_producers_before_logics_own_and_never_lives() {
    let mac = Mac::new();
    let sources = kumi_runtime::integrations::library_sources(&mac.options(Some(DawId::Logic)));
    let seen: Vec<(String, SourceKind)> = sources.iter().map(|source| (source.label.clone(), source.kind)).collect();
    assert_eq!(
        seen,
        [
            ("Samples".to_string(), SourceKind::UserLibrary),
            ("Sampler Instruments".to_string(), SourceKind::UserLibrary),
            ("Patches".to_string(), SourceKind::UserLibrary),
            ("Channel Strip Settings".to_string(), SourceKind::UserLibrary),
            ("Plug-In Settings".to_string(), SourceKind::UserLibrary),
            ("Crate".to_string(), SourceKind::Folder),
            ("Splice".to_string(), SourceKind::Splice),
            ("Apple Loops".to_string(), SourceKind::Pack),
            ("Logic Pro EXS Factory Samples".to_string(), SourceKind::Core),
            ("Logic Pro Channel Strip Settings".to_string(), SourceKind::Core),
        ],
        "only the folders that are there, the producer's first"
    );
    assert!(!sources.iter().any(|source| source.path.contains("Ableton")), "never Live's: {sources:?}");
    // Live's layout is as it was, for Live and for a DAW whose own places haven't come yet.
    for daw in [None, Some(DawId::Ableton), Some(DawId::Reaper), Some(DawId::Renoise)] {
        let live = kumi_runtime::integrations::library_sources(&mac.options(daw));
        assert_eq!(live, kumi_runtime::library::sources::library_sources(&mac.options(None)), "{daw:?}");
        assert!(live.iter().any(|source| source.kind == SourceKind::UserLibrary && source.path.ends_with("User Library")));
    }
}

#[tokio::test]
async fn the_plan_takes_logics_projects_where_they_usually_are_and_the_recent_ones_its_bridge_read() {
    let mac = Mac::new();
    let dir = mac.dir();
    let (night, sunrise) = (mac.home().join("Music/Logic/Night Drive.logicx"), mac.home().join("Projects/Sunrise.logicx"));
    put(
        Path::new(&recent_file(&dir.to_string_lossy(), DawId::Logic)),
        json!([format!("{}/", sunrise.display()), mac.home().join("Gone.logicx"), mac.home().join("Music/Logic/Old Live Song.als")])
            .to_string(),
    );
    let options = PlanOptions {
        dir: dir.to_string_lossy().into(),
        sources: Some(mac.options(Some(DawId::Logic))),
        workers: Some(0),
        ..Default::default()
    };
    let plan = plan_learning(options).await;
    assert_eq!(plan.daw, Some(DawId::Logic));
    assert_eq!(plan.set_files, [sunrise.to_string_lossy()], "the recent projects still there, Logic's alone");
    assert_eq!(
        plan.set_folders,
        [mac.home().join("Music/Logic").to_string_lossy(), mac.home().join("Projects").to_string_lossy()],
        "Logic's own folder, and the one holding a recent project"
    );
    assert!(night.is_dir());
    let live =
        plan_learning(PlanOptions { dir: dir.to_string_lossy().into(), sources: Some(mac.options(None)), ..Default::default() }).await;
    assert_eq!((live.daw, live.set_files.is_empty()), (None, true), "Live's plan reads no Logic recents");
}

#[tokio::test]
async fn learning_logics_library_counts_its_sounds_presets_and_projects_as_logic_keeps_them() {
    let mac = Mac::new();
    let dir = mac.dir();
    let plan = plan_learning(PlanOptions {
        dir: dir.to_string_lossy().into(),
        // (the folders the producer named are the plan's, as the session gives them)
        folders: Some(vec![mac.home().join("Crate").to_string_lossy().into()]),
        sources: Some(mac.options(Some(DawId::Logic))),
        workers: Some(0),
        ..Default::default()
    })
    .await;
    let signal = Signal::new();
    let stop = signal.clone();
    let phase = Rc::new(Cell::new(LearnPhase::Looking));
    let todo = Rc::new(Cell::new(0));
    let mut options = LearnOptions::new(plan, signal);
    let (seen, counted) = (phase.clone(), todo.clone());
    options.on_progress = Some(Rc::new(move |progress| {
        seen.set(progress.phase);
        counted.set(progress.sounds.todo);
    }));
    // (stopped as the sounds are about to be measured: what was found is what counts here)
    options.gate = Some(Rc::new(move || {
        if phase.get() == LearnPhase::Sounds {
            stop.cancel();
        }
        Box::pin(async {})
    }));
    assert!(learn(options).await.unwrap_err().is_aborted());
    assert_eq!(todo.get(), 6, "Kick 01.caf, Snare 01.wav, the Crate's Clap, Splice's Hat, Apple Loops' Beat and EXS's C3; none of Live's");
    let logs = library_logs(&dir.to_string_lossy());
    let presets = logs.presets.load().await;
    let mut shown: Vec<(String, String, Option<String>)> =
        presets.values().map(|preset| (preset.name.clone(), preset.format.clone(), preset.device.clone())).collect();
    shown.sort();
    assert_eq!(
        shown,
        [
            ("Bright Vocal".to_string(), "pst".to_string(), Some("Channel EQ".to_string())),
            ("Classic Vocal".to_string(), "patch".to_string(), None),
            ("Lead".to_string(), "cst".to_string(), None),
            ("Tight Kit".to_string(), "exs".to_string(), Some("Sampler".to_string())),
            ("Warm Bus".to_string(), "cst".to_string(), None),
        ],
        "a patch counted whole (its .cst inside isn't another), no Live preset"
    );
    assert!(presets.values().all(|preset| preset.browser.is_none()), "Live's Browser paths are Live's");
    let sets = logs.sets.load().await;
    let mut projects: Vec<(String, Value)> =
        sets.values().map(|entry| (entry.set.as_ref().unwrap().name.clone(), json!(entry.set.as_ref().unwrap().daw))).collect();
    projects.sort_by(|a, b| a.0.cmp(&b.0));
    assert_eq!(projects, [("Night Drive".to_string(), json!("logic"))], "a .logicx package, not the .als beside it");
    assert!(kumi_runtime::library::learn::songs_of(sets.values()).is_empty(), "the producer's habits are learned from Live's Sets");

    // The library's tools in Logic's words: its presets are found, its projects listed by their file.
    tokio::task::LocalSet::new()
        .run_until(async {
            let library = create_library(LibraryOptions { dir: dir.to_string_lossy().into(), ..Default::default() });
            let names = |profile: &DawProfile| -> Vec<String> {
                library.tools_for(profile, LibraryToolsOptions::default()).iter().map(|tool| tool.name().to_string()).collect()
            };
            assert!(names(&DawProfile::LOGIC).contains(&"find_presets".to_string()), "Logic's presets are found");
            assert!(names(&DawProfile::REAPER).contains(&"find_presets".to_string()), "REAPER's own presets, as before");
            let tools = library.tools_for(&DawProfile::LOGIC, LibraryToolsOptions::default());
            let tool = |name: &str| tools.iter().find(|tool| tool.name() == name).unwrap().clone();
            // In Logic's words: its regions, its sample load, its presets' kinds, its projects counted by their files.
            for tool in &tools {
                let said = format!("{} {}", tool.description(), Value::Object(tool.input_schema()));
                let words: Vec<&str> = said.split(|c: char| !(c.is_alphanumeric() || c == '_')).collect();
                for live in ["clip", "clipRef", "load_sample", "Live", "Set", "Sets"] {
                    assert!(!words.contains(&live), "{live} in {said}");
                }
                assert!(!said.to_lowercase().contains("drum rack"), "{said}");
            }
            assert_eq!(tool("find_presets").input_schema()["properties"]["kind"]["enum"], json!(["instrument", "plug-in"]));
            assert_eq!(
                tool("find_sounds").input_schema()["properties"]["like"]["description"],
                "Sounds like this: an audio file's path (~/… or absolute) or an audio region's ref"
            );
            let projects = tool("my_sets").input_schema();
            assert!(projects["properties"].get("min_tempo").is_none() && projects["properties"].get("key").is_none(), "{projects:?}");
            assert!(tool("my_sets").description().starts_with("The producer's own Logic Pro projects, as Kumi found them"));
            let my_sets = tool("my_sets");
            let listed = my_sets.execute(serde_json::Map::new(), Signal::new()).await.unwrap();
            let listed: Value = serde_json::from_str(&listed.text).unwrap();
            assert_eq!(listed["sets"][0]["name"], "Night Drive", "{listed}");
            assert!(listed["sets"][0].get("tracks").is_none(), "what's inside isn't read: {listed}");
            let one = my_sets.execute(json!({"set": "Night Drive"}).as_object().unwrap().clone(), Signal::new()).await.unwrap();
            let one: Value = serde_json::from_str(&one.text).unwrap();
            assert_eq!(
                one["note"],
                "Kumi counts a Logic Pro project by its file and doesn't read inside it: open it in Logic Pro to look at it."
            );
            library.close().await;
        })
        .await;
}
