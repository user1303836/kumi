//! A DAW's own project files (#399 K9): the library learns them through its `LibrarySources::projects` as that DAW's
//! projects, beside Live's Sets, and `my_sets` finds them only in that DAW. The format here is a stand-in: a text file
//! whose first line is its tempo, made by "Renoise" (any DAW id serves).

use async_trait::async_trait;
use kumi_common::abort::{Signal, SignalExt};
use kumi_library::{
    create_library,
    daws::{Daws, Layout, LibrarySources, Places, ProjectFormat},
    features::MeasureOptions,
    learn::{learn, library_logs, LearnOptions, LearnPlan, PresetEntry, SetEntry, SoundEntry},
    plan::{kumi_sets, plan_learning, PlanOptions},
    search::SoundIndex,
    sets::{Clips, SetSummary, SetTrack},
    sources::{library_sources, recent_projects, Source, SourceKind, SourceOptions},
    store::Entry,
    tools::{library_tools_for, LearningState, LibraryAccess, LibraryToolsOptions},
    LibraryOptions,
};
use kumi_types::{
    daw::{DawId, DawProfile},
    errors::RuntimeError,
};
use serde_json::{json, Value};
use std::{path::Path, rc::Rc};

static SONG: ProjectFormat = ProjectFormat { extension: ".song", skip: song_copy, read: read_song };

/// A copy of a song is named "… copy".
fn song_copy(path: &Path) -> bool {
    path.file_stem().is_some_and(|stem| stem.to_string_lossy().ends_with(" copy"))
}

fn read_song(path: &Path, signal: &Signal) -> Result<SetSummary, RuntimeError> {
    signal.check()?;
    let text = std::fs::read_to_string(path).map_err(|error| RuntimeError::plain(error.to_string()))?;
    let tempo = text.lines().next().and_then(|line| line.trim().parse().ok()).ok_or_else(|| RuntimeError::plain("no tempo"))?;
    Ok(SetSummary {
        name: path.file_stem().unwrap().to_string_lossy().into(),
        tempo: Some(tempo),
        tracks: vec![SetTrack { name: "Lead".into(), ..Default::default() }],
        made_in: Some("Songs 1.0".into()),
        ..Default::default()
    })
}

struct Songs;

impl LibrarySources for Songs {
    fn profile(&self) -> &'static DawProfile {
        &DawProfile::RENOISE
    }
    fn projects(&self) -> Option<&'static ProjectFormat> {
        Some(&SONG)
    }
}

static SONGS: Songs = Songs;
static WITH_SONGS: [&'static dyn LibrarySources; 1] = [&SONGS];

/// A song file: its tempo, then enough for the walk to take it (it leaves out files under 64 bytes).
fn song(tempo: &str) -> String {
    format!("{tempo}\n{}\n", "a pattern of notes ".repeat(4))
}

const LIVE_SET: &str = r#"<Ableton Creator="Ableton Live 12.2"><LiveSet><Tracks><MidiTrack Id="1"><Name><EffectiveName Value="Bass"/></Name></MidiTrack></Tracks></LiveSet></Ableton>"#;

fn put(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

#[tokio::test]
async fn a_daws_project_files_are_learned_as_its_projects_beside_lives_sets_leaving_its_copies_out() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let home = tempfile::tempdir().unwrap();
            let (music, dir) = (home.path().join("Music"), home.path().join("library"));
            put(&music.join("Night Drive.als"), LIVE_SET);
            put(&music.join("Tracker Song.song"), &song("128"));
            put(&music.join("Tracker Song copy.song"), &song("90"));
            put(&music.join("Broken.song"), &format!("not a tempo\n{}", song("1")));
            // A song Kumi saw open, outside the folders the library walks.
            let elsewhere = home.path().join("Elsewhere");
            let recent = elsewhere.join("Recent Song.song");
            put(&recent, &song("100"));
            let recent_copy = elsewhere.join("Recent copy.song");
            put(&recent_copy, &song("100"));
            // What an earlier learn kept from outside the folders: a copy of a song and a Live backup go, by their names;
            // a Set in a folder that merely looks like a backup's stays.
            let (kept_copy, backup, in_folder) =
                (elsewhere.join("Old copy.song"), elsewhere.join("Song.backup-1.als"), elsewhere.join("Old.backup-1").join("Song.als"));
            let logs = library_logs(&dir.to_string_lossy());
            for file in [&kept_copy, &backup, &in_folder] {
                put(file, LIVE_SET);
                let file = Entry { path: file.to_string_lossy().into(), size: LIVE_SET.len() as u64, mtime: 1, gone: None };
                logs.sets.append(&[SetEntry { file, set: Some(SetSummary::default()), error: None }]).await.unwrap();
            }
            let plan = LearnPlan {
                daw: None,
                dir: dir.to_string_lossy().into(),
                sources: vec![Source { path: music.to_string_lossy().into(), label: "Music".into(), kind: SourceKind::Folder }],
                approved_folders: None,
                audio_only: false,
                decode: Default::default(),
                set_folders: vec![],
                set_files: vec![recent.to_string_lossy().into(), recent_copy.to_string_lossy().into()],
                plugin_presets: vec![],
                workers: None,
            };
            let learned = learn(LearnOptions { daws: Daws(&WITH_SONGS), ..LearnOptions::new(plan, Signal::new()) }).await.unwrap();
            assert_eq!(learned.sets.known, 5, "the Set, the two songs, the one that doesn't read and the Set kept outside; no copy");
            let sets = library_logs(&dir.to_string_lossy()).sets.load().await;
            let named = |name: &str| sets.values().find(|entry| entry.path.ends_with(name)).unwrap_or_else(|| panic!("{name}"));
            assert_eq!(named("Night Drive.als").set.as_ref().unwrap().daw, None, "Live's Sets are written without a DAW");
            let tracker = named("Tracker Song.song").set.as_ref().unwrap();
            assert_eq!((tracker.daw, tracker.tempo, tracker.made_in.as_deref()), (Some(DawId::Renoise), Some(128.), Some("Songs 1.0")));
            assert_eq!(named("Recent Song.song").set.as_ref().unwrap().daw, Some(DawId::Renoise));
            assert_eq!(named("Broken.song").error.as_deref(), Some("no tempo"));
            assert!(!sets.keys().any(|path| path.contains(" copy")), "a copy is left out: {:?}", sets.keys().collect::<Vec<_>>());
            assert!(!sets.keys().any(|path| path.ends_with("Song.backup-1.als")), "{:?}", sets.keys().collect::<Vec<_>>());
            assert!(sets.keys().any(|path| path.ends_with("Song.als")), "{:?}", sets.keys().collect::<Vec<_>>());
            // The producer's habits are Live's alone: its Sets' two folders, not the songs.
            let taste: Value = serde_json::from_str(&std::fs::read_to_string(dir.join("taste.json")).unwrap()).unwrap();
            assert_eq!(taste["sets"], 2);
            // Without the DAW registered, its files aren't projects, and those learned go as the walk no longer finds them.
            let plan = LearnPlan {
                daw: None,
                dir: dir.to_string_lossy().into(),
                sources: vec![Source { path: music.to_string_lossy().into(), label: "Music".into(), kind: SourceKind::Folder }],
                approved_folders: None,
                audio_only: false,
                decode: Default::default(),
                set_folders: vec![],
                set_files: vec![],
                plugin_presets: vec![],
                workers: None,
            };
            learn(LearnOptions::new(plan, Signal::new())).await.unwrap();
            let sets = library_logs(&dir.to_string_lossy()).sets.load().await;
            assert!(sets.keys().all(|path| !path.ends_with("Tracker Song.song")), "{:?}", sets.keys().collect::<Vec<_>>());
        })
        .await;
}

struct Learned(Vec<SetEntry>);

#[async_trait(?Send)]
impl LibraryAccess for Learned {
    async fn sounds(&self) -> Result<Rc<SoundIndex>, RuntimeError> {
        Ok(Rc::new(SoundIndex::new(Vec::<SoundEntry>::new(), &[])))
    }
    async fn presets(&self) -> Result<Vec<PresetEntry>, RuntimeError> {
        Ok(vec![])
    }
    async fn sets(&self) -> Result<Vec<SetEntry>, RuntimeError> {
        Ok(self.0.clone())
    }
    fn learning(&self) -> LearningState {
        LearningState::default()
    }
    fn remember(&self, _: Vec<String>) {}
    async fn folders(&self) -> Vec<String> {
        vec![]
    }
    async fn measure(&self, _: String, _: MeasureOptions) -> Result<SoundEntry, RuntimeError> {
        Err(RuntimeError::plain("no"))
    }
}

fn entry(path: &str, set: SetSummary) -> SetEntry {
    SetEntry { file: Entry { path: path.into(), size: 100, mtime: 1_700_000_000_000, gone: None }, set: Some(set), error: None }
}

/// A project Kumi couldn't read, as learning keeps it.
fn unread(path: &str, mtime: i64, why: &str) -> SetEntry {
    SetEntry { file: Entry { path: path.into(), size: 100, mtime, gone: None }, set: None, error: Some(why.into()) }
}

async fn my_sets(profile: &DawProfile, options: LibraryToolsOptions, access: &Rc<Learned>, input: Value) -> Option<(Value, bool)> {
    let tools = library_tools_for(profile, access.clone(), options);
    let tool = tools.iter().find(|tool| tool.name() == "my_sets")?;
    let result = tool.execute(input.as_object().unwrap().clone(), Signal::new()).await.unwrap();
    Some((serde_json::from_str(&result.text).unwrap_or(Value::String(result.text.clone())), result.is_error))
}

#[tokio::test]
async fn my_sets_finds_a_daws_own_projects_only_in_it_and_lives_sets_only_in_live() {
    // (a whole path as this system writes it: Windows resolves "/Music/…" onto a drive, which isn't what's kept, and my_sets
    // compares a path as resolved)
    let music = if cfg!(windows) { "C:\\Music" } else { "/Music" };
    let at = |file: &str| format!("{music}{}{file}", std::path::MAIN_SEPARATOR);
    let tracker = at("Tracker Song.song");
    let access = Rc::new(Learned(vec![
        entry(
            &at("Night Drive.als"),
            SetSummary {
                name: "Night Drive".into(),
                tempo: Some(124.),
                live: Some("12.2".into()),
                tracks: vec![SetTrack { name: "Bass".into(), ..Default::default() }],
                ..Default::default()
            },
        ),
        entry(
            &tracker,
            SetSummary {
                name: "Tracker Song".into(),
                tempo: Some(128.),
                daw: Some(DawId::Renoise),
                made_in: Some("Songs 1.0".into()),
                tracks: vec![
                    SetTrack { name: "Lead".into(), ..Default::default() },
                    SetTrack { name: "Takes".into(), clips: Clips { session: 0, arrangement: 3 }, ..Default::default() },
                ],
                ..Default::default()
            },
        ),
        entry(&at("Other.rpp"), SetSummary { name: "Other".into(), daw: Some(DawId::Reaper), ..Default::default() }),
    ]));
    let songs = LibraryToolsOptions { projects: Some(&SONG), ..Default::default() };
    let names =
        |found: &Value| found["sets"].as_array().unwrap().iter().map(|row| row["name"].as_str().unwrap().to_owned()).collect::<Vec<_>>();
    // In the DAW whose files Kumi reads: its songs alone, found by name with or without the extension.
    let (found, _) = my_sets(&DawProfile::RENOISE, songs.clone(), &access, json!({})).await.unwrap();
    assert_eq!(names(&found), ["Tracker Song"]);
    assert_eq!(found["known"], 1);
    for name in ["tracker song", "Tracker Song.song", tracker.as_str()] {
        let (song, error) = my_sets(&DawProfile::RENOISE, songs.clone(), &access, json!({"set": name})).await.unwrap();
        assert!(!error, "{name}: {song}");
        assert_eq!(
            (&song["name"], song["tempo"].as_f64(), &song["madeIn"]),
            (&json!("Tracker Song"), Some(128.), &json!("Songs 1.0")),
            "{name}"
        );
    }
    // Of Live's words, only what it has: no scenes, and no clips on a track with none.
    let (song, _) = my_sets(&DawProfile::RENOISE, songs.clone(), &access, json!({"set": "tracker song"})).await.unwrap();
    assert_eq!(song.get("scenes"), None);
    assert_eq!(song["tracks"][0].get("clips"), None);
    assert_eq!(song["tracks"][1]["clips"]["arrangement"], 3);
    let (_, error) = my_sets(&DawProfile::RENOISE, songs.clone(), &access, json!({"set": "night drive"})).await.unwrap();
    assert!(error, "a Live Set isn't one of the DAW's projects");
    // In Live: its Sets alone, as before.
    let (found, _) = my_sets(&DawProfile::ABLETON, LibraryToolsOptions::default(), &access, json!({})).await.unwrap();
    assert_eq!(names(&found), ["Night Drive"]);
    let (set, _) = my_sets(&DawProfile::ABLETON, LibraryToolsOptions::default(), &access, json!({"set": "Night Drive.als"})).await.unwrap();
    assert_eq!(set["madeIn"], "12.2");
    assert_eq!((&set["scenes"], &set["tracks"][0]["clips"]["session"]), (&json!(0), &json!(0)), "Live's Sets say them, as before");
    let (_, error) = my_sets(&DawProfile::ABLETON, LibraryToolsOptions::default(), &access, json!({"set": "tracker song"})).await.unwrap();
    assert!(error, "a song isn't one of Live's Sets");
    // A DAW whose files Kumi doesn't read isn't offered the tool.
    assert!(my_sets(&DawProfile::REAPER, LibraryToolsOptions::default(), &access, json!({})).await.is_none());
}

/// #806: a project asked for that Kumi couldn't read is told as that, with why, not as one it doesn't know.
#[tokio::test]
async fn my_sets_says_why_it_couldnt_read_a_project_asked_for() {
    let music = if cfg!(windows) { "C:\\Music" } else { "/Music" };
    let separator = std::path::MAIN_SEPARATOR;
    let at = |file: &str| format!("{music}{separator}{file}");
    let (night_drive, other_moon) = (at("Night Drive.als"), at(&format!("Sketches{separator}Moon.als")));
    // Newer than what they stand beside, so it's not their age that puts a project Kumi read first.
    let newer = 1_800_000_000_000;
    let access = Rc::new(Learned(vec![
        unread(&night_drive, newer, "That isn't a Live Set."),
        entry(&at("Night Drive Remix.als"), SetSummary { name: "Night Drive Remix".into(), ..Default::default() }),
        entry(&at("Moon.als"), SetSummary { name: "Moon".into(), ..Default::default() }),
        unread(&other_moon, newer, "cut short"),
        unread(&at("Lost Sketch.ALS"), newer, "no end"),
        unread(&at(&format!("Old{separator}Twin.als")), newer - 1, "the older's"),
        unread(&at("Twin.als"), newer, "the newer's"),
        unread(&at(&format!("Older{separator}Twin.als")), newer - 2, "the oldest's"),
        unread(&at("Broken.song"), newer, "no tempo"),
    ]));
    let live = LibraryToolsOptions::default;
    let songs = || LibraryToolsOptions { projects: Some(&SONG), ..Default::default() };
    let said = |place: &str, name: &str, path: &str, why: &str, app: &str| {
        json!(format!(
            "Kumi couldn't read the {place} “{name}” ({path}): {why}. Kumi reads it again when the file changes, so saving it from {app} may help."
        ))
    };
    // By its name, its file's name or its path, and by its whole name before another's that only holds those words.
    for asked in ["Night Drive", "night drive.als", night_drive.as_str()] {
        let (answer, error) = my_sets(&DawProfile::ABLETON, live(), &access, json!({"set": asked})).await.unwrap();
        assert!(error, "{asked}: {answer}");
        assert_eq!(answer, said("Set", "Night Drive", &night_drive, "That isn't a Live Set", "Live"), "{asked}");
    }
    // What only a name it couldn't read holds, whatever the case of the file's extension.
    let (answer, error) = my_sets(&DawProfile::ABLETON, live(), &access, json!({"set": "sketch"})).await.unwrap();
    assert!(error, "{answer}");
    assert_eq!(answer, said("Set", "Lost Sketch", &at("Lost Sketch.ALS"), "no end", "Live"));
    // Of several it couldn't read by that name, the newest.
    let (answer, _) = my_sets(&DawProfile::ABLETON, live(), &access, json!({"set": "twin"})).await.unwrap();
    assert_eq!(answer, said("Set", "Twin", &at("Twin.als"), "the newer's", "Live"));
    // One it read stands before one it couldn't, at each step, but a path names the file it names.
    for (asked, name) in [("moon", "Moon"), ("night drive r", "Night Drive Remix"), ("oo", "Moon"), ("drive", "Night Drive Remix")] {
        let (answer, error) = my_sets(&DawProfile::ABLETON, live(), &access, json!({"set": asked})).await.unwrap();
        assert!(!error, "{asked}: {answer}");
        assert_eq!(answer["name"], name, "{asked}");
    }
    let (answer, error) = my_sets(&DawProfile::ABLETON, live(), &access, json!({"set": other_moon})).await.unwrap();
    assert!(error, "{answer}");
    assert_eq!(answer, said("Set", "Moon", &other_moon, "cut short", "Live"));
    // Only in the DAW whose file it is.
    let (answer, error) = my_sets(&DawProfile::RENOISE, songs(), &access, json!({"set": "broken"})).await.unwrap();
    assert!(error, "{answer}");
    assert_eq!(answer, said("song", "Broken", &at("Broken.song"), "no tempo", "Renoise"));
    let (answer, error) = my_sets(&DawProfile::RENOISE, songs(), &access, json!({"set": "night drive"})).await.unwrap();
    assert!(error, "{answer}");
    assert!(answer.as_str().unwrap().starts_with("Kumi doesn't know a song called “night drive”"), "{answer}");
    let (answer, error) = my_sets(&DawProfile::ABLETON, live(), &access, json!({"set": "broken"})).await.unwrap();
    assert!(error, "{answer}");
    assert!(answer.as_str().unwrap().starts_with("Kumi doesn't know a Set called “broken”"), "{answer}");
    // No search finds what it couldn't read, or counts it.
    let (found, _) = my_sets(&DawProfile::ABLETON, live(), &access, json!({})).await.unwrap();
    assert_eq!((&found["known"], &found["matched"]), (&json!(2), &json!(2)), "{found}");
    let (found, _) = my_sets(&DawProfile::ABLETON, live(), &access, json!({"words": ["sketch"]})).await.unwrap();
    assert_eq!(found["matched"], 0, "{found}");
}

/// #806: what learning itself keeps of a project it couldn't read is what my_sets tells.
#[tokio::test]
async fn a_project_learning_couldnt_read_is_told_with_why() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let home = tempfile::tempdir().unwrap();
            let (music, dir) = (home.path().join("Music"), home.path().join("library"));
            // (Past the 64 bytes the walk leaves out.)
            let set = music.join("Night Drive.als");
            put(&set, &format!("<Ableton Creator=\"Ableton Live 12.2\"><Notes>{}</Notes></Ableton>", "not a Set ".repeat(8)));
            let song = music.join("Broken.song");
            put(&song, &format!("not a tempo\n{}", self::song("1")));
            let plan = LearnPlan {
                daw: None,
                dir: dir.to_string_lossy().into(),
                sources: vec![Source { path: music.to_string_lossy().into(), label: "Music".into(), kind: SourceKind::Folder }],
                approved_folders: None,
                audio_only: false,
                decode: Default::default(),
                set_folders: vec![],
                set_files: vec![],
                plugin_presets: vec![],
                workers: None,
            };
            learn(LearnOptions { daws: Daws(&WITH_SONGS), ..LearnOptions::new(plan, Signal::new()) }).await.unwrap();
            let access = Rc::new(Learned(library_logs(&dir.to_string_lossy()).sets.load().await.into_values().collect()));
            let (answer, error) =
                my_sets(&DawProfile::ABLETON, LibraryToolsOptions::default(), &access, json!({"set": "Night Drive"})).await.unwrap();
            assert!(error, "{answer}");
            let answer = answer.as_str().unwrap().to_owned();
            assert!(answer.starts_with("Kumi couldn't read the Set “Night Drive” ("), "{answer}");
            assert!(answer.contains("): That isn't a Live Set. Kumi reads it again"), "{answer}");
            let songs = LibraryToolsOptions { projects: Some(&SONG), ..Default::default() };
            let (answer, error) = my_sets(&DawProfile::RENOISE, songs, &access, json!({"set": "Broken"})).await.unwrap();
            assert!(error, "{answer}");
            assert!(answer.as_str().unwrap().contains("): no tempo. Kumi reads it again"), "{answer}");
        })
        .await;
}

#[tokio::test]
async fn the_projects_kumi_saw_open_include_a_registered_daws_by_its_extension() {
    let home = tempfile::tempdir().unwrap();
    let projects = home.path().join("projects");
    let (set, song, other) = (home.path().join("Night Drive.als"), home.path().join("Tracker Song.song"), home.path().join("Other.rpp"));
    let id = |digit: &str| digit.repeat(32);
    for (place, file) in [
        (id("a"), &set),
        (format!("renoise-{}", id("b")), &song),
        (format!("reaper-{}", id("c")), &other),
        // A place says whose project it holds: another DAW's file in it isn't that DAW's project, and a place that isn't
        // a saved project's isn't read.
        (id("d"), &song),
        (format!("renoise-{}", id("e")), &set),
        ("renoise-unsaved".to_owned(), &song),
        ("elsewhere".to_owned(), &song),
    ] {
        put(file, "content");
        put(&projects.join(place).join("last-seen.json"), &json!({"path": file}).to_string());
    }
    let seen = |found: Vec<String>| {
        found.into_iter().map(|path| Path::new(&path).file_name().unwrap().to_string_lossy().into_owned()).collect::<Vec<_>>()
    };
    assert_eq!(seen(kumi_sets(projects.to_str(), Daws(&WITH_SONGS)).await), ["Night Drive.als", "Tracker Song.song"]);
    assert_eq!(seen(kumi_sets(projects.to_str(), Daws::default()).await), ["Night Drive.als"]);
}

/// Discovery would consult the producer's DAW/home paths. Restricted planning must never call any of it.
struct ForbiddenDiscovery;

impl LibrarySources for ForbiddenDiscovery {
    fn profile(&self) -> &'static DawProfile {
        &DawProfile::ABLETON
    }
    fn places(&self, _: &SourceOptions) -> Places {
        panic!("restricted sources consulted Live/Splice places")
    }
    fn own_library(&self, _: &SourceOptions) -> Vec<Source> {
        panic!("restricted sources consulted a DAW library")
    }
    fn recent_projects(&self, _: &SourceOptions) -> Vec<String> {
        panic!("restricted sources consulted recent projects")
    }
    fn layout(&self) -> &'static Layout {
        panic!("restricted sources consulted a DAW layout")
    }
}

static FORBIDDEN_DISCOVERY: ForbiddenDiscovery = ForbiddenDiscovery;
static NO_DISCOVERY: [&'static dyn LibrarySources; 1] = [&FORBIDDEN_DISCOVERY];

fn canonical_folder(path: &Path) -> String {
    path.canonicalize().unwrap().to_string_lossy().into_owned()
}

#[tokio::test]
async fn restricted_sources_ignore_discovery_remembered_folders_and_revoked_approvals() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let home = tempfile::tempdir().unwrap();
            let dir = home.path().join("index");
            let approved = home.path().join("approved");
            put(&approved.join("Approved.als"), LIVE_SET);
            let approved = canonical_folder(&approved);
            let forbidden = ["Live/User Library", "Splice", "Music", "Library/Audio/Presets", "remembered", "settings", "recent"];
            for folder in forbidden {
                put(&home.path().join(folder).join("Forbidden.als"), LIVE_SET);
            }
            let remembered = vec![home.path().join("remembered").to_string_lossy().into_owned(), approved.clone()];
            let saved = serde_json::to_string(&remembered).unwrap();
            put(&dir.join("folders.json"), &saved);
            let projects = home.path().join("projects");
            let project = home.path().join("recent/Forbidden.als");
            put(&projects.join("a".repeat(32)).join("last-seen.json"), &json!({"path": project}).to_string());

            // The second run revokes the only approval; folders.json still lists it, but cannot reauthorize it.
            for roots in [vec![approved.clone()], vec![]] {
                let source_options = SourceOptions {
                    home: Some(home.path().to_string_lossy().into()),
                    platform: Some("darwin".into()),
                    folders: Some(vec![home.path().join("settings").to_string_lossy().into()]),
                    approved_folders: Some(roots.clone()),
                    ..Default::default()
                };
                let sources = library_sources(Daws(&NO_DISCOVERY), &source_options);
                assert_eq!(sources.iter().map(|source| source.path.clone()).collect::<Vec<_>>(), roots);
                assert!(sources.iter().all(|source| source.kind == SourceKind::Folder));
                assert!(recent_projects(Daws(&NO_DISCOVERY), &source_options).is_empty());
                let planning = PlanOptions {
                    dir: dir.to_string_lossy().into(),
                    folders: source_options.folders.clone(),
                    sources: Some(source_options.clone()),
                    projects_dir: Some(projects.to_string_lossy().into()),
                    ..Default::default()
                };
                // This is the forked learner's serialization boundary: DAWs are injected by the child.
                let mut planning: PlanOptions = serde_json::from_value(serde_json::to_value(planning).unwrap()).unwrap();
                planning.daws = Daws(&NO_DISCOVERY);
                let plan = plan_learning(planning).await;
                let plan: LearnPlan = serde_json::from_value(serde_json::to_value(plan).unwrap()).unwrap();
                assert_eq!(plan.sources, sources);
                assert_eq!(plan.approved_folders, Some(roots.clone()));
                assert!(plan.set_folders.is_empty());
                assert!(plan.set_files.is_empty());
                assert!(plan.plugin_presets.is_empty());
                let learned = learn(LearnOptions { rebuild: true, daws: Daws(&NO_DISCOVERY), ..LearnOptions::new(plan, Signal::new()) })
                    .await
                    .unwrap();
                assert_eq!(learned.sets.known, roots.len());
                assert_eq!(learned.sounds.known, 0);
                assert_eq!(learned.presets.known, 0);
                // Old log rows cannot make an empty allowlist fall back to all known sounds.
                for path in [Path::new(&approved).join("Old.wav"), home.path().join("remembered/Old.wav")] {
                    put(&path, "");
                    library_logs(&dir.to_string_lossy())
                        .sounds
                        .append(&[SoundEntry {
                            file: Entry { path: path.to_string_lossy().into(), size: 0, mtime: 1, gone: None },
                            ..Default::default()
                        }])
                        .await
                        .unwrap();
                }
                let library = create_library(LibraryOptions {
                    dir: dir.to_string_lossy().into(),
                    folders: source_options.folders.clone(),
                    projects_dir: Some(projects.to_string_lossy().into()),
                    sources: Some(source_options),
                    daws: Daws(&NO_DISCOVERY),
                    fork: Some(false),
                    ..Default::default()
                });
                assert_eq!(library.sources().await, sources);
                assert_eq!(library.sound_index().await.unwrap().entries.len(), roots.len());
                library.remember(vec![home.path().join("remembered").to_string_lossy().into()]);
                assert_eq!(library.sources().await, sources);
                assert_eq!(std::fs::read_to_string(dir.join("folders.json")).unwrap(), saved);
                library.close().await;
            }
        })
        .await;
}

#[test]
fn restricted_sources_reject_network_device_and_relative_roots() {
    let sources = SourceOptions {
        approved_folders: Some(
            [
                r"\\server\share",
                "//server/share",
                r"\\?\UNC\server\share",
                r"\\?\GLOBALROOT\Device\HarddiskVolume1",
                r"\\.\C:\Samples",
                r"\??\C:\Samples",
                "~/Music",
                "relative",
            ]
            .into_iter()
            .map(str::to_owned)
            .collect(),
        ),
        ..Default::default()
    };
    assert!(library_sources(Daws(&NO_DISCOVERY), &sources).is_empty());
}

#[cfg(unix)]
#[tokio::test]
async fn restricted_learning_skips_escaped_symlinks_and_confines_auxiliary_walks() {
    use std::os::unix::fs::symlink;
    tokio::task::LocalSet::new()
        .run_until(async {
            let home = tempfile::tempdir().unwrap();
            let approved = home.path().join("approved");
            let outside = home.path().join("approved-other");
            put(&approved.join("Allowed.als"), LIVE_SET);
            put(&outside.join("Escaped.als"), LIVE_SET);
            let approved = canonical_folder(&approved);
            let outside = canonical_folder(&outside);
            symlink(&outside, Path::new(&approved).join("escaped-directory")).unwrap();
            symlink(Path::new(&outside).join("Escaped.als"), Path::new(&approved).join("escaped-file.als")).unwrap();
            let source_options = SourceOptions { approved_folders: Some(vec![approved.clone()]), ..Default::default() };
            let mut plan = plan_learning(PlanOptions {
                dir: home.path().join("index").to_string_lossy().into(),
                sources: Some(source_options),
                daws: Daws(&NO_DISCOVERY),
                ..Default::default()
            })
            .await;
            // Even stale auxiliary inputs in a serialized plan cannot grant another root.
            plan.sources.push(Source { path: outside.clone(), label: "Outside".into(), kind: SourceKind::Folder });
            plan.sources.push(Source { path: approved.clone(), label: "DAW".into(), kind: SourceKind::Daw(DawId::Ableton) });
            plan.set_folders = vec![outside.clone()];
            plan.set_files = vec![Path::new(&outside).join("Escaped.als").to_string_lossy().into()];
            plan.plugin_presets = vec![outside.clone()];
            let plan: LearnPlan = serde_json::from_value(serde_json::to_value(plan).unwrap()).unwrap();
            let dir = plan.dir.clone();
            let learned = learn(LearnOptions { daws: Daws(&NO_DISCOVERY), ..LearnOptions::new(plan, Signal::new()) }).await.unwrap();
            assert_eq!(learned.sets.known, 1);
            assert_eq!(learned.failed, 0);
            let sets = library_logs(&dir).sets.load().await;
            assert_eq!(sets.keys().collect::<Vec<_>>(), vec![Path::new(&approved).join("Allowed.als").to_string_lossy().into_owned()]);

            // Replacing the approved root after planning must not turn its new symlink target into an approval.
            let options = SourceOptions { approved_folders: Some(vec![approved.clone()]), ..Default::default() };
            let plan = plan_learning(PlanOptions {
                dir: home.path().join("replaced-index").to_string_lossy().into(),
                sources: Some(options.clone()),
                daws: Daws(&NO_DISCOVERY),
                ..Default::default()
            })
            .await;
            std::fs::rename(&approved, home.path().join("old-approved")).unwrap();
            symlink(&outside, &approved).unwrap();
            assert!(library_sources(Daws(&NO_DISCOVERY), &options).is_empty());
            let learned = learn(LearnOptions { daws: Daws(&NO_DISCOVERY), ..LearnOptions::new(plan, Signal::new()) }).await.unwrap();
            assert_eq!(learned.sets.known, 0);
            assert_eq!(learned.sounds.known, 0);
            assert_eq!(learned.presets.known, 0);
        })
        .await;
}
