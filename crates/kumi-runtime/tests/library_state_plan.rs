use kumi_runtime::library::{
    learn::{plugin_preset_folders, set_folders, Counts, LearnPhase, LearnProgress},
    plan::{kumi_sets, plan_learning, remembered_folders, PlanOptions},
    sources::SourceOptions,
    state::{acquire_lock, alive, read_state, write_state},
};
use serde_json::json;
use std::path::Path;
fn put(path: &Path, body: impl AsRef<[u8]>) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, body).unwrap();
}
#[tokio::test]
async fn state_preserves_last_completed_run_and_clears_stopped_or_dead_learning() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().to_str().unwrap();
    assert!(read_state(dir).await.is_none());
    let mut progress = LearnProgress {
        phase: LearnPhase::Done,
        started_at: 100,
        finished_at: Some(200),
        failed: 1,
        sounds: Counts { known: 7, todo: 7, done: 7 },
        presets: Counts { known: 4, todo: 4, done: 4 },
        sets: Counts { known: 2, todo: 2, done: 2 },
        ..Default::default()
    };
    write_state(dir, &progress, false).await.unwrap();
    let previous = read_state(dir).await.unwrap();
    assert!(previous.learning.is_none());
    assert_eq!(
        serde_json::to_value(previous.last.as_ref().unwrap()).unwrap(),
        json!({"startedAt":100,"finishedAt":200,"sounds":7,"presets":4,"sets":2,"failed":1})
    );
    progress.phase = LearnPhase::Sounds;
    progress.started_at = 300;
    write_state(dir, &progress, false).await.unwrap();
    let mut state = read_state(dir).await.unwrap();
    assert_eq!(state.last, previous.last);
    assert_eq!(state.learning.as_ref().unwrap().pid, std::process::id() as i64);
    assert!(alive(std::process::id() as f64));
    assert!(!alive(f64::NAN));
    state.learning.as_mut().unwrap().pid = i32::MAX as i64;
    put(&tmp.path().join("state.json"), serde_json::to_vec(&state).unwrap());
    assert!(read_state(dir).await.unwrap().learning.is_none());
    write_state(dir, &progress, true).await.unwrap();
    assert_eq!(read_state(dir).await.unwrap(), previous);
    put(&tmp.path().join("state.json"), b"{\"version\":2}");
    assert!(read_state(dir).await.is_none());
}
#[tokio::test]
async fn locks_exclude_other_processes_but_replace_same_process_dead_and_day_old_holders() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("library");
    let dir = dir.to_str().unwrap();
    let first = acquire_lock(dir).await.unwrap().unwrap();
    let second = acquire_lock(dir).await.unwrap().unwrap();
    first.release().await.unwrap();
    second.release().await.unwrap();
    let lock = Path::new(dir).join("learning.lock");
    #[cfg(unix)]
    {
        let mut child = std::process::Command::new("sleep").arg("30").spawn().unwrap();
        put(&lock, serde_json::to_vec(&json!({"pid":child.id(),"at":kumi_common::time::now_ms()})).unwrap());
        assert!(acquire_lock(dir).await.unwrap().is_none());
        put(&lock, serde_json::to_vec(&json!({"pid":child.id(),"at":0})).unwrap());
        acquire_lock(dir).await.unwrap().unwrap().release().await.unwrap();
        // Taken a minute before that process started: its pid was a learner's that's gone.
        put(&lock, serde_json::to_vec(&json!({"pid":child.id(),"at":kumi_common::time::now_ms() - 60_000})).unwrap());
        acquire_lock(dir).await.unwrap().unwrap().release().await.unwrap();
        child.kill().unwrap();
        child.wait().unwrap();
    }
    for body in ["half-written", "{\"pid\":2147483647,\"at\":9999999999999}", "{\"pid\":\"1\",\"at\":1}"] {
        put(&lock, body);
        acquire_lock(dir).await.unwrap().unwrap().release().await.unwrap();
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let holder = acquire_lock(dir).await.unwrap().unwrap();
        assert_eq!(std::fs::metadata(&lock).unwrap().permissions().mode() & 0o777, 0o600);
        holder.release().await.unwrap();
    }
}
#[tokio::test]
async fn plan_reads_bounded_project_records_remembers_folders_and_excludes_pack_demos() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path();
    let dir = home.join("library");
    let projects = home.join("projects");
    // Windows forbids double quotes in filenames; its path separators still exercise
    // JSON escaping. Retain the quoted filename case on supporting filesystems.
    let name = if cfg!(windows) { "Night 'Drive'.als" } else { "Night \"Drive\".als" };
    let song = home.join("Songs").join("Song Project").join(name);
    let pack_demo = home.join("Splice").join("sounds").join("Demo.als");
    for file in [&song, &pack_demo] {
        put(file, "content");
    }
    for (id, body) in [
        ("a".repeat(32), json!({"path":song}).to_string()),
        ("b".repeat(32), json!({"path":pack_demo}).to_string()),
        ("c".repeat(32), format!("{}{}", " ".repeat(2048), json!({"path":song}))),
        ("d".repeat(32), format!("{{\"path\": {}}}", json!(song))),
        ("invalid".into(), json!({"path":song}).to_string()),
    ] {
        put(&projects.join(id).join("last-seen.json"), body);
    }
    let found = kumi_sets(projects.to_str()).await;
    assert_eq!(found, vec![song.to_string_lossy(), pack_demo.to_string_lossy()]);
    assert!(kumi_sets(None).await.is_empty());
    let named = home.join("Named");
    let remembered = home.join("Remembered");
    let ignored = home.join("Ignored source option");
    let plugins = home.join("Library").join("Audio").join("Presets");
    for folder in [&named, &remembered, &ignored, &plugins] {
        std::fs::create_dir_all(folder).unwrap();
    }
    put(&dir.join("folders.json"), json!([remembered, null, 12]).to_string());
    assert_eq!(remembered_folders(dir.to_str().unwrap()).await, vec![remembered.to_string_lossy()]);
    let options = PlanOptions {
        dir: dir.to_string_lossy().into(),
        folders: Some(vec![named.to_string_lossy().into()]),
        projects_dir: Some(projects.to_string_lossy().into()),
        sources: Some(SourceOptions {
            home: Some(home.to_string_lossy().into()),
            platform: Some("darwin".into()),
            applications: Some(home.join("Applications").to_string_lossy().into()),
            folders: Some(vec![ignored.to_string_lossy().into()]),
            ..Default::default()
        }),
        workers: Some(0),
        ..Default::default()
    };
    let plan = plan_learning(options.clone()).await;
    assert_eq!(plan.set_files, vec![song.to_string_lossy()]);
    assert_eq!(plan.set_folders, vec![home.join("Music").to_string_lossy(), home.join("Songs").to_string_lossy()]);
    assert_eq!(plan.plugin_presets, vec![plugins.to_string_lossy()]);
    assert!(plan.sources.iter().any(|s| s.path == remembered.to_string_lossy()));
    assert!(!plan.sources.iter().any(|s| s.path == ignored.to_string_lossy()));
    assert_eq!(plan.workers, Some(0));
    let disabled = plan_learning(PlanOptions { find_sets: Some(false), ..options }).await;
    assert!(disabled.set_folders.is_empty() && disabled.set_files.is_empty() && disabled.plugin_presets.is_empty());
}
#[test]
fn project_and_plugin_folders_follow_platform_and_preserve_first_seen_order() {
    use kumi_runtime::library::sources::join;
    let home = join(std::env::temp_dir().to_str().unwrap(), "producer");
    let recent = vec![join(&home, "Songs/One/A.als"), join(&home, "Songs/Two/B.als"), join(&home, "Direct.als")];
    assert_eq!(
        set_folders(&recent, Some(&home), Some("win32")),
        [join(&home, "Documents/Ableton"), join(&home, "Music"), join(&home, "Songs")]
    );
    assert_eq!(plugin_preset_folders(Some(&home), Some("darwin")), [join(&home, "Library/Audio/Presets")]);
    assert_eq!(plugin_preset_folders(Some(&home), Some("win32")), [join(&home, "Documents/VST3 Presets")]);
    assert_eq!(plugin_preset_folders(Some(&home), Some("linux")), [join(&home, ".vst3/presets")]);
}
