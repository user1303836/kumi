#[path = "support/library.rs"]
mod fixture;
use fixture::{put, Studio};
use kumi_common::abort::Signal;
use kumi_runtime::library::{
    features::MeasureOptions,
    learn::{learn, learn_sound, library_logs, songs_of, LearnOptions, LearnPhase, SoundEntry},
    measure_worker::{MeasureJob, MeasurePool},
    store::{unpack_vector, Entry},
};
use serde_json::Value;
use std::{cell::Cell, rc::Rc, time::Duration};
#[tokio::test]
async fn learned_entries_match_typescript_including_classification_and_vectors() {
    let root = tempfile::tempdir().unwrap();
    let expected: Value = serde_json::from_str(include_str!("support/library-learn-oracle.json")).unwrap();
    for ((name, channels), expected) in fixture::sound_cases().into_iter().zip(expected.as_array().unwrap()) {
        let path = root.path().join(name);
        let bytes = fixture::wav(&channels, 44100);
        put(&path, &bytes);
        let mut entry = learn_sound(path.to_str().unwrap(), name, bytes.len() as u64, 123, MeasureOptions::default()).await.unwrap();
        entry.file.path = name.into();
        let actual_vector = unpack_vector(entry.vector.as_ref().unwrap());
        let expected_vector = unpack_vector(expected["vector"].as_str().unwrap());
        for (a, b) in actual_vector.iter().zip(&expected_vector) {
            assert!((a - b).abs() < 1e-5, "{name}: {a} != {b}");
        }
        let mut actual: Value = serde_json::from_str(&kumi_common::js::json::stringify(&serde_json::to_value(&entry).unwrap())).unwrap();
        actual.as_object_mut().unwrap().remove("vector");
        let mut expected = expected.clone();
        expected.as_object_mut().unwrap().remove("vector");
        assert_eq!(actual, expected, "{name}");
    }
    let long = learn_sound("Kick Loop 120.mp3", "Kick Loop 120.mp3", 60 * 1024 * 1024 + 1, 1, MeasureOptions::default()).await.unwrap();
    assert_eq!(long.error.as_deref(), Some("too long to measure"));
    assert!(long.r#class.is_some() && long.kind.is_some() && long.vector.is_none());
}
#[tokio::test]
async fn incremental_learning_resumes_after_stop_relearns_changes_and_removes_deleted_files() {
    let studio = Studio::new();
    let signal = Signal::new();
    let stopped = signal.clone();
    let phase = Rc::new(Cell::new(LearnPhase::Looking));
    let sound_count = Rc::new(Cell::new(0));
    let mut options = LearnOptions::new(studio.plan(0), signal);
    let phase_callback = phase.clone();
    options.on_progress = Some(Rc::new(move |p| phase_callback.set(p.phase)));
    options.gate = Some(Rc::new(move || {
        if phase.get() == LearnPhase::Sounds {
            sound_count.set(sound_count.get() + 1);
            if sound_count.get() > 3 {
                stopped.cancel();
            }
        }
        Box::pin(async {})
    }));
    assert!(learn(options).await.unwrap_err().is_aborted());
    let logs = library_logs(studio.dir.to_str().unwrap());
    assert_eq!(logs.sounds.load().await.len(), 3);
    assert_eq!(logs.presets.load().await.len(), 4);
    assert_eq!(logs.sets.load().await.len(), 2);
    let progress = learn(LearnOptions::new(studio.plan(0), Signal::new())).await.unwrap();
    assert_eq!([progress.sounds.todo, progress.presets.todo, progress.sets.todo], [4, 0, 0]);
    assert_eq!([progress.sounds.known, progress.presets.known, progress.sets.known], [7, 4, 2]);
    assert_eq!(learn(LearnOptions::new(studio.plan(0), Signal::new())).await.unwrap().sounds.todo, 0);
    put(&studio.user.join("Samples/Hats/Hat Closed.wav"), fixture::wav(&[fixture::hat(0.2, 5)], 44100));
    let deleted = studio.user.join("Samples/Kicks/Kick Short.wav");
    std::fs::remove_file(&deleted).unwrap();
    let progress = learn(LearnOptions::new(studio.plan(0), Signal::new())).await.unwrap();
    assert_eq!(progress.sounds.todo, 1);
    let kept = logs.sounds.load().await;
    assert_eq!(kept.len(), 6);
    assert!(!kept.contains_key(deleted.to_str().unwrap()));
    assert!(std::fs::read_to_string(&logs.sounds.file).unwrap().starts_with("{\"kumiLibrary\":\"sounds\""));
    let taste: Value = serde_json::from_slice(&std::fs::read(studio.dir.join("taste.json")).unwrap()).unwrap();
    assert_eq!(taste["sets"], 2);
    assert!(taste["lines"].as_array().unwrap().iter().any(|l| l["id"] == "chain-vocal"));
    let mut changed = kept.values().next().unwrap().clone();
    changed.features = Some(0);
    logs.sounds.append(&[changed]).await.unwrap();
    assert_eq!(learn(LearnOptions::new(studio.plan(0), Signal::new())).await.unwrap().sounds.todo, 1);
    let mut options = LearnOptions::new(studio.plan(0), Signal::new());
    options.rebuild = true;
    assert_eq!(learn(options).await.unwrap().sounds.todo, 6);
}
#[cfg(unix)]
#[tokio::test]
async fn a_folder_that_cant_be_read_keeps_what_was_learned_inside_it() {
    use std::os::unix::fs::PermissionsExt;
    let studio = Studio::new();
    learn(LearnOptions::new(studio.plan(0), Signal::new())).await.unwrap();
    let logs = library_logs(studio.dir.to_str().unwrap());
    let kicks = studio.user.join("Samples/Kicks");
    let learned = logs.sounds.load().await;
    assert!(learned.keys().any(|path| path.starts_with(kicks.to_str().unwrap())));
    // As a NAS that stops answering, or a placeholder nothing serves now, leaves it.
    std::fs::set_permissions(&kicks, std::fs::Permissions::from_mode(0o000)).unwrap();
    let again = learn(LearnOptions::new(studio.plan(0), Signal::new())).await;
    std::fs::set_permissions(&kicks, std::fs::Permissions::from_mode(0o755)).unwrap();
    again.unwrap();
    assert_eq!(logs.sounds.load().await.keys().collect::<Vec<_>>(), learned.keys().collect::<Vec<_>>());
}
#[tokio::test]
async fn discovery_keeps_unavailable_roots_and_skips_links_copies_backups_and_unfinished_depth() {
    use kumi_runtime::library::sources::{Source, SourceKind};
    let studio = Studio::new();
    let logs = library_logs(studio.dir.to_str().unwrap());
    let missing = studio.home.path().join("Detached/Kick.wav");
    let entry =
        SoundEntry { file: Entry { path: missing.to_string_lossy().into(), size: 500, mtime: 10, gone: None }, ..Default::default() };
    logs.sounds.append(&[entry]).await.unwrap();
    let mut plan = studio.plan(0);
    plan.sources.push(Source {
        path: studio.home.path().join("Detached").to_string_lossy().into(),
        label: "Detached".into(),
        kind: SourceKind::Folder,
    });
    for dir in [
        "Samples/Recorded",
        "Samples/Processed",
        "Samples/Imported",
        "Samples/Freeze",
        "Samples/Consolidated",
        "Backup",
        "Defaults",
        ".hidden",
        "A.app",
        "node_modules",
    ] {
        put(&studio.user.join(dir).join("Kick.wav"), fixture::wav(&[fixture::kick(50., 0.1)], 44100));
    }
    put(&studio.user.join("Tiny.wav"), "less than 64 bytes");
    put(&studio.user.join("Ignored.aac"), vec![0; 100]);
    #[cfg(unix)]
    std::os::unix::fs::symlink(&studio.extra, studio.user.join("Linked")).unwrap();
    let progress = learn(LearnOptions::new(plan.clone(), Signal::new())).await.unwrap();
    assert_eq!(progress.sounds.known, 8);
    assert!(logs.sounds.load().await.contains_key(missing.to_str().unwrap()));
    // An incomplete walk must not remove entries anywhere within that root.
    let mut deep = studio.user.clone();
    for _ in 0..17 {
        deep = deep.join("nested");
    }
    std::fs::create_dir_all(&deep).unwrap();
    std::fs::remove_file(studio.user.join("Samples/Kicks/Kick Deep.wav")).unwrap();
    assert_eq!(learn(LearnOptions::new(plan, Signal::new())).await.unwrap().sounds.known, 8);
}
#[tokio::test]
async fn native_measurement_pool_learns_reuses_processes_and_replaces_lost_or_timed_out_workers() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("Kick.wav");
    put(&path, fixture::wav(&[fixture::kick(50., 0.2)], 44100));
    let job = MeasureJob {
        id: 0,
        path: path.to_string_lossy().into(),
        relative: "Kick.wav".into(),
        size: std::fs::metadata(&path).unwrap().len(),
        mtime: 1,
        start: None,
        seconds: None,
    };
    let pool = MeasurePool::with_worker(1, env!("CARGO_BIN_EXE_kumi-library-measure").into(), Duration::from_secs(10));
    for _ in 0..2 {
        let entry = pool.run(0, job.clone()).await.unwrap();
        assert!(entry.error.is_none(), "{:?}", entry.error);
        assert!(entry.vector.is_some());
    }
    pool.close().await;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        for (script, why, timeout) in
            [("#!/bin/sh\nexit 7\n", "Kumi couldn't read it", 5000), ("#!/bin/sh\nexec sleep 30\n", "it took too long to read", 100)]
        {
            let worker = root.path().join("worker");
            put(&worker, script);
            std::fs::set_permissions(&worker, std::fs::Permissions::from_mode(0o700)).unwrap();
            let pool = MeasurePool::with_worker(1, worker, Duration::from_millis(timeout));
            for _ in 0..2 {
                assert_eq!(pool.run(0, job.clone()).await.unwrap().error.as_deref(), Some(why));
            }
            pool.close().await;
        }
    }
    let studio = Studio::new();
    let progress = learn(LearnOptions::new(studio.plan(2), Signal::new())).await.unwrap();
    assert_eq!([progress.sounds.known, progress.presets.known, progress.sets.known], [7, 4, 2]);
    assert_eq!(progress.failed, 0);
}
#[tokio::test]
async fn a_worker_that_cant_start_says_nothing_about_the_file() {
    let root = tempfile::tempdir().unwrap();
    // The binary gone for a moment (an update, an antivirus): no entry, so the file isn't kept as unreadable.
    let pool = MeasurePool::with_worker(1, root.path().join("missing-worker"), Duration::from_secs(1));
    let job = MeasureJob { id: 0, path: "/x.wav".into(), relative: "x.wav".into(), size: 1, mtime: 1, start: None, seconds: None };
    assert!(pool.run(0, job).await.is_err());
    pool.close().await;
}
#[cfg(unix)]
#[tokio::test]
async fn a_stopped_worker_takes_what_it_started_and_its_temporary_files_with_it() {
    use std::os::unix::fs::PermissionsExt;
    let root = tempfile::tempdir().unwrap();
    let worker = root.path().join("worker");
    // A worker that converts as the real one does, then hangs: a child at work, a copy in its temporary folder.
    put(
        &worker,
        format!(
            "#!/bin/sh\nmkdir \"$TMPDIR/kumi-audio-1\" && : > \"$TMPDIR/kumi-audio-1/converted.wav\"\nprintf '%s' \"$TMPDIR\" > '{0}/temp'\nsleep 30 &\nprintf '%s' $! > '{0}/child'\nwait\n",
            root.path().display()
        ),
    );
    std::fs::set_permissions(&worker, std::fs::Permissions::from_mode(0o700)).unwrap();
    let pool = MeasurePool::with_worker(1, worker, Duration::from_millis(1000));
    let job = MeasureJob { id: 0, path: "/x.mp3".into(), relative: "x.mp3".into(), size: 1, mtime: 1, start: None, seconds: None };
    assert_eq!(pool.run(0, job).await.unwrap().error.as_deref(), Some("it took too long to read"));
    let temp = std::fs::read_to_string(root.path().join("temp")).unwrap();
    assert!(!std::path::Path::new(&temp).exists(), "{temp} is still there");
    let child: libc::pid_t = std::fs::read_to_string(root.path().join("child")).unwrap().parse().unwrap();
    // Gone once whoever took it in has reaped it.
    let mut ended = false;
    for _ in 0..50 {
        if unsafe { libc::kill(child, 0) } != 0 {
            ended = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    assert!(ended, "the worker's child is still running");
    pool.close().await;
}
#[cfg(unix)]
#[test]
fn a_new_pool_sweeps_the_folders_left_by_a_kumi_thats_gone() {
    let temp = std::env::temp_dir();
    let mut ended = std::process::Command::new("true").spawn().unwrap();
    let gone = ended.id();
    ended.wait().unwrap();
    let folder = |pid: u32| temp.join(format!("kumi-measure-{pid}-{}", uuid::Uuid::new_v4()));
    // A crashed Kumi's (its process has ended), one of a process older than its folder (pid 1), and this one's.
    let (left, older, mine) = (folder(gone), folder(1), folder(std::process::id()));
    // Made in one step: another test's pool, starting meanwhile, may sweep it first, which is what's checked anyway.
    std::fs::create_dir(&left).unwrap();
    for kept in [&older, &mine] {
        put(&kept.join("kumi-audio-1/converted.wav"), "a copy");
    }
    let pool = MeasurePool::with_worker(1, "/nowhere".into(), Duration::from_secs(1));
    let (left_there, older_there, mine_there) = (left.exists(), older.exists(), mine.exists());
    for kept in [&older, &mine] {
        let _ = std::fs::remove_dir_all(kept);
    }
    assert_eq!((left_there, older_there, mine_there), (false, true, true));
    drop(pool);
}
#[cfg(windows)]
#[tokio::test]
async fn a_stopped_worker_takes_what_it_started_and_its_temporary_files_with_it_on_windows() {
    let root = tempfile::tempdir().unwrap();
    let worker = root.path().join("worker.cmd");
    // A worker that converts as the real one does, then hangs: a child holding its copy open, shared with no one.
    put(
        &worker,
        format!(
            concat!(
                "@echo off\r\n",
                "mkdir \"%TMP%\\kumi-audio-1\"\r\n",
                "(echo %TMP%)>\"{0}\\temp\"\r\n",
                "start \"\" /b powershell -NoProfile -NonInteractive -Command \"$held = [IO.File]::Open($env:TMP + '\\kumi-audio-1\\converted.wav', 'OpenOrCreate', 'ReadWrite', 'None'); Set-Content -LiteralPath '{0}\\child' -Value $PID; Start-Sleep -Seconds 60\"\r\n",
                ":wait\r\n",
                "if not exist \"{0}\\child\" (ping -n 2 127.0.0.1 >nul & goto wait)\r\n",
                "ping -n 60 127.0.0.1 >nul\r\n"
            ),
            root.path().display()
        ),
    );
    let pool = MeasurePool::with_worker(1, worker, Duration::from_secs(12));
    let job = MeasureJob { id: 0, path: "C:\\x.mp3".into(), relative: "x.mp3".into(), size: 1, mtime: 1, start: None, seconds: None };
    assert_eq!(pool.run(0, job).await.unwrap().error.as_deref(), Some("it took too long to read"));
    let child: u32 = std::fs::read_to_string(root.path().join("child")).expect("the child held the copy").trim().parse().unwrap();
    let temp = std::fs::read_to_string(root.path().join("temp")).unwrap();
    assert!(!std::path::Path::new(temp.trim()).exists(), "{temp} is still there");
    assert!(!kumi_runtime::library::state::alive(child as f64), "the worker's child is still running");
    pool.close().await;
}
#[test]
fn only_newest_version_of_each_project_contributes_to_taste() {
    use kumi_runtime::library::{learn::SetEntry, sets::SetSummary};
    let entry = |path: &str, mtime, name: &str| SetEntry {
        file: Entry { path: path.into(), mtime, size: 1, gone: None },
        set: Some(SetSummary { name: name.into(), ..Default::default() }),
        error: None,
    };
    let entries = [entry("/songs/A/first.als", 1, "first"), entry("/songs/B/only.als", 4, "only"), entry("/songs/A/new.als", 3, "new")];
    assert_eq!(songs_of(&entries).iter().map(|s| s.name.as_str()).collect::<Vec<_>>(), ["new", "only"]);
}
