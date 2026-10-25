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
