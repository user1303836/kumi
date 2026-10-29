//! The library learner's music job (`kumi-library-learner --music`): told what to learn over its stdin, it walks the
//! producer's music folders, hears their songs and says how it's going; while another Kumi learns them, it leaves them.
use kumi_listening::music::{
    database,
    store::MusicStore,
    worker::{ScanMessage, ScanOptions, ScanPhase, ScanReply},
};
use std::{path::Path, process::Stdio, time::Duration};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    process::Command,
};

/// A mono tone `seconds` long, at 8 kHz.
fn song(path: &Path, seconds: f64) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let samples: Vec<f32> = (0..(seconds * 8_000.) as usize).map(|n| (n as f32 * 0.07).sin() * 0.4).collect();
    std::fs::write(path, kumi_media::audio::decode::float_wav(&[samples], 8_000)).unwrap();
}

/// The music job with Kumi's folder at `home` (the style model off: nothing is fetched), told to learn `music`; its
/// replies until it ends.
async fn learn(home: &Path, music: &Path) -> Vec<ScanReply> {
    std::fs::create_dir_all(home).unwrap();
    std::fs::write(home.join("slots.json"), r#"{"embeddings":{"now":{"use":"off"}}}"#).unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_kumi"))
        .arg("__library-learner")
        .arg("--music")
        .env("KUMI_HOME", home)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    // Learning on the battery too: a laptop on its battery would hold the job, waiting for the mains.
    let options = ScanOptions {
        dir: home.join("music").to_string_lossy().into(),
        folders: vec![music.to_string_lossy().into()],
        on_battery: true,
        ..Default::default()
    };
    let mut input = child.stdin.take().unwrap();
    input.write_all(format!("{}\n", serde_json::to_string(&ScanMessage::Learn { options }).unwrap()).as_bytes()).await.unwrap();
    input.flush().await.unwrap();
    let mut lines = BufReader::new(child.stdout.take().unwrap()).lines();
    let mut replies = vec![];
    while let Some(line) = tokio::time::timeout(Duration::from_secs(60), lines.next_line()).await.unwrap().unwrap() {
        replies.push(serde_json::from_str::<ScanReply>(&line).unwrap());
    }
    let status = tokio::time::timeout(Duration::from_secs(10), child.wait()).await.unwrap().unwrap();
    assert!(status.success(), "{status}");
    drop(input);
    replies
}

#[tokio::test(flavor = "current_thread")]
async fn the_learner_hears_the_producers_music_as_a_job_of_its_own() {
    let dir = tempfile::tempdir().unwrap();
    let music = dir.path().join("Music");
    song(&music.join("A/Album/01 One.wav"), 31.);
    song(&music.join("B/Album/01 Two.wav"), 32.);
    song(&music.join("C/Album/01 Short.wav"), 4.);
    let home = dir.path().join("kumi");
    let replies = learn(&home, &music).await;
    let Some(ScanReply::Done { progress }) = replies.last() else { panic!("{replies:?}") };
    assert_eq!((progress.phase, progress.songs, progress.full, progress.skipped), (ScanPhase::Done, 3, 2, 1), "{progress:?}");
    assert!(replies.iter().any(|reply| matches!(reply, ScanReply::Progress { progress } if progress.phase == ScanPhase::Looking)));
    let store = MusicStore::open_read(&database(&home.join("music"))).unwrap().unwrap();
    assert_eq!(store.counts().unwrap().full, 2);
    assert!(!home.join("music/learning.lock").exists(), "the lock is let go");
}

#[tokio::test(flavor = "current_thread")]
async fn while_another_kumi_learns_the_music_the_learner_leaves_it() {
    let dir = tempfile::tempdir().unwrap();
    let music = dir.path().join("Music");
    song(&music.join("A/Album/01 One.wav"), 31.);
    let home = dir.path().join("kumi");
    std::fs::create_dir_all(home.join("music")).unwrap();
    // This test's own process holds it, as another Kumi would.
    let held = serde_json::json!({"pid": std::process::id(), "at": kumi_common::time::now_ms()});
    std::fs::write(home.join("music/learning.lock"), held.to_string()).unwrap();
    let replies = learn(&home, &music).await;
    assert!(matches!(replies[..], [ScanReply::Busy]), "{replies:?}");
    assert!(!database(&home.join("music")).exists(), "nothing walked");
}

/// Quitting just as the music starts being learned: a scan still starting (here one that never gets further) reads no
/// stop, so closing ends it at once rather than waiting out the time it's given to stop.
#[cfg(unix)]
#[tokio::test(flavor = "current_thread")]
async fn closing_as_the_music_starts_being_learned_ends_its_scan_at_once() {
    use kumi_listening::music::learning::{create_music, MusicOptions};
    use std::os::unix::fs::PermissionsExt;
    let home = tempfile::tempdir().unwrap();
    let starting = home.path().join("starting-learner");
    std::fs::write(&starting, "#!/bin/sh\nexec sleep 30\n").unwrap();
    std::fs::set_permissions(&starting, std::fs::Permissions::from_mode(0o700)).unwrap();
    // Read by this test's process alone: each test is a process of its own.
    std::env::set_var("KUMI_LIBRARY_LEARNER_BIN", &starting);
    let songs = home.path().join("Songs");
    song(&songs.join("one.wav"), 1.);
    tokio::task::LocalSet::new()
        .run_until(async {
            let folders = Some(vec![songs.to_string_lossy().into_owned()]);
            let music = create_music(MusicOptions::new(home.path().join("music"), folders, kumi::daw::registry::libraries()));
            music.learn_now();
            let closing = std::time::Instant::now();
            music.close().await;
            assert!(closing.elapsed() < Duration::from_millis(500), "{:?}", closing.elapsed());
        })
        .await;
}

/// A scan ended before it spoke can't let go of a lock it took, so closing does: a quit leaves nothing behind.
#[cfg(unix)]
#[tokio::test(flavor = "current_thread")]
async fn closing_lets_go_of_the_lock_a_scan_was_ended_holding() {
    use kumi_listening::music::learning::{create_music, MusicOptions};
    use std::os::unix::fs::PermissionsExt;
    let home = tempfile::tempdir().unwrap();
    let dir = home.path().join("music");
    std::fs::create_dir_all(&dir).unwrap();
    let lock = dir.join("learning.lock");
    // A scan that takes the lock as one does (its pid, which `exec` keeps), then gets no further.
    let locking = home.path().join("locking-learner");
    std::fs::write(
        &locking,
        format!("#!/bin/sh\nprintf '{{\"pid\":%s,\"at\":%s000}}' \"$$\" \"$(date +%s)\" > '{}'\nexec sleep 30\n", lock.display()),
    )
    .unwrap();
    std::fs::set_permissions(&locking, std::fs::Permissions::from_mode(0o700)).unwrap();
    // Read by this test's process alone: each test is a process of its own.
    std::env::set_var("KUMI_LIBRARY_LEARNER_BIN", &locking);
    let songs = home.path().join("Songs");
    song(&songs.join("one.wav"), 1.);
    tokio::task::LocalSet::new()
        .run_until(async {
            let folders = Some(vec![songs.to_string_lossy().into_owned()]);
            let music = create_music(MusicOptions::new(dir.clone(), folders, kumi::daw::registry::libraries()));
            music.learn_now();
            tokio::time::timeout(Duration::from_secs(15), async {
                while !std::fs::read_to_string(&lock).is_ok_and(|held| held.ends_with('}')) {
                    tokio::time::sleep(Duration::from_millis(1)).await;
                }
            })
            .await
            .expect("the lock is taken in time");
            music.close().await;
            assert!(!lock.exists());
        })
        .await;
}
