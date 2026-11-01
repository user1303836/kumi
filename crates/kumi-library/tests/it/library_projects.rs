//! A DAW's own project files (#399 K9): the library learns them through its `LibrarySources::projects` as that DAW's
//! projects, beside Live's Sets, and `my_sets` finds them only in that DAW. The format here is a stand-in: a text file
//! whose first line is its tempo, made by "Renoise" (any DAW id serves).

use async_trait::async_trait;
use kumi_common::abort::{Signal, SignalExt};
use kumi_library::{
    daws::{Daws, LibrarySources, ProjectFormat},
    features::MeasureOptions,
    learn::{learn, library_logs, LearnOptions, LearnPlan, PresetEntry, SetEntry, SoundEntry},
    plan::kumi_sets,
    search::SoundIndex,
    sets::{Clips, SetSummary, SetTrack},
    sources::{Source, SourceKind},
    store::Entry,
    tools::{library_tools_for, LearningState, LibraryAccess, LibraryToolsOptions},
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
                dir: dir.to_string_lossy().into(),
                sources: vec![Source { path: music.to_string_lossy().into(), label: "Music".into(), kind: SourceKind::Folder }],
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
            assert!(!sets.keys().any(|path| path.contains(" copy")), "a copy is left out: {:?}", sets.keys());
            assert!(!sets.keys().any(|path| path.ends_with("Song.backup-1.als")), "{:?}", sets.keys());
            assert!(sets.keys().any(|path| path.ends_with("Song.als")), "{:?}", sets.keys());
            // The producer's habits are Live's alone: its Sets' two folders, not the songs.
            let taste: Value = serde_json::from_str(&std::fs::read_to_string(dir.join("taste.json")).unwrap()).unwrap();
            assert_eq!(taste["sets"], 2);
            // Without the DAW registered, its files aren't projects, and those learned go as the walk no longer finds them.
            let plan = LearnPlan {
                dir: dir.to_string_lossy().into(),
                sources: vec![Source { path: music.to_string_lossy().into(), label: "Music".into(), kind: SourceKind::Folder }],
                set_folders: vec![],
                set_files: vec![],
                plugin_presets: vec![],
                workers: None,
            };
            learn(LearnOptions::new(plan, Signal::new())).await.unwrap();
            let sets = library_logs(&dir.to_string_lossy()).sets.load().await;
            assert!(sets.keys().all(|path| !path.ends_with("Tracker Song.song")), "{:?}", sets.keys());
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

async fn my_sets(profile: &DawProfile, options: LibraryToolsOptions, access: &Rc<Learned>, input: Value) -> Option<(Value, bool)> {
    let tools = library_tools_for(profile, access.clone(), options);
    let tool = tools.iter().find(|tool| tool.name() == "my_sets")?;
    let result = tool.execute(input.as_object().unwrap().clone(), Signal::new()).await.unwrap();
    Some((serde_json::from_str(&result.text).unwrap_or(Value::String(result.text.clone())), result.is_error))
}

#[tokio::test]
async fn my_sets_finds_a_daws_own_projects_only_in_it_and_lives_sets_only_in_live() {
    let access = Rc::new(Learned(vec![
        entry(
            "/Music/Night Drive.als",
            SetSummary {
                name: "Night Drive".into(),
                tempo: Some(124.),
                live: Some("12.2".into()),
                tracks: vec![SetTrack { name: "Bass".into(), ..Default::default() }],
                ..Default::default()
            },
        ),
        entry(
            "/Music/Tracker Song.song",
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
        entry("/Music/Other.rpp", SetSummary { name: "Other".into(), daw: Some(DawId::Reaper), ..Default::default() }),
    ]));
    let songs = LibraryToolsOptions { projects: Some(&SONG), ..Default::default() };
    let names =
        |found: &Value| found["sets"].as_array().unwrap().iter().map(|row| row["name"].as_str().unwrap().to_owned()).collect::<Vec<_>>();
    // In the DAW whose files Kumi reads: its songs alone, found by name with or without the extension.
    let (found, _) = my_sets(&DawProfile::RENOISE, songs.clone(), &access, json!({})).await.unwrap();
    assert_eq!(names(&found), ["Tracker Song"]);
    assert_eq!(found["known"], 1);
    for name in ["tracker song", "Tracker Song.song", "/Music/Tracker Song.song"] {
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
