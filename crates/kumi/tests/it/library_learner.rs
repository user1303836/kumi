#[path = "../support/library.rs"]
mod fixture;
use fixture::Studio;
use kumi_library::{
    create_library,
    learn::library_logs,
    learner::{LearnerMessage, LearnerOptions, LearnerReply},
    plan::PlanOptions,
    sources::SourceOptions,
    state::read_state,
    Library, LibraryOptions,
};
use std::{
    process::Stdio,
    rc::Rc,
    time::{Duration, Instant},
};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines},
    process::{Child, ChildStdin, ChildStdout, Command},
};
struct Learner {
    child: Child,
    input: Option<ChildStdin>,
    lines: Lines<BufReader<ChildStdout>>,
}
impl Learner {
    fn new() -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_kumi"))
            .arg("__library-learner")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let input = child.stdin.take();
        let lines = BufReader::new(child.stdout.take().unwrap()).lines();
        Self { child, input, lines }
    }
    async fn send(&mut self, message: LearnerMessage) {
        self.input.as_mut().unwrap().write_all(format!("{}\n", serde_json::to_string(&message).unwrap()).as_bytes()).await.unwrap();
        self.input.as_mut().unwrap().flush().await.unwrap();
    }
    async fn next(&mut self) -> LearnerReply {
        let line = tokio::time::timeout(Duration::from_secs(15), self.lines.next_line()).await.unwrap().unwrap().expect("learner reply");
        serde_json::from_str(&line).unwrap()
    }
    async fn finish(&mut self) {
        let status = tokio::time::timeout(Duration::from_secs(10), self.child.wait()).await.unwrap().unwrap();
        assert!(status.success(), "{status}");
    }
}
fn options(studio: &Studio, paused: bool) -> LearnerOptions {
    LearnerOptions {
        plan: PlanOptions {
            daws: kumi::daw::registry::libraries(),
            dir: studio.dir.to_string_lossy().into(),
            folders: Some(vec![studio.extra.to_string_lossy().into()]),
            sources: Some(SourceOptions {
                home: Some(studio.home.path().to_string_lossy().into()),
                platform: Some("darwin".into()),
                applications: Some(studio.home.path().join("Applications").to_string_lossy().into()),
                ..Default::default()
            }),
            find_sets: Some(false),
            workers: Some(1),
            ..Default::default()
        },
        paused,
        rebuild: false,
    }
}
#[tokio::test]
async fn child_waits_while_paused_reports_progress_and_persists_completed_state() {
    let studio = Studio::new();
    let mut child = Learner::new();
    child.send(LearnerMessage::Learn { options: options(&studio, true) }).await;
    loop {
        if let LearnerReply::Progress { progress } = child.next().await {
            if progress.phase == kumi_library::learn::LearnPhase::Presets {
                break;
            }
        }
    }
    tokio::time::sleep(Duration::from_millis(250)).await;
    let logs = library_logs(studio.dir.to_str().unwrap());
    assert_eq!(logs.presets.load().await.len(), 0);
    assert_eq!(logs.sounds.load().await.len(), 0);
    let mut second = Learner::new();
    second.send(LearnerMessage::Learn { options: options(&studio, false) }).await;
    assert!(matches!(second.next().await, LearnerReply::Busy));
    second.finish().await;
    child.send(LearnerMessage::Resume).await;
    loop {
        match child.next().await {
            LearnerReply::Done { progress } => {
                assert_eq!([progress.sounds.known, progress.presets.known, progress.sets.known], [7, 4, 2]);
                break;
            }
            LearnerReply::Failed { message } => panic!("{message}"),
            _ => {}
        }
    }
    child.finish().await;
    let state = read_state(studio.dir.to_str().unwrap()).await.unwrap();
    assert_eq!(state.last.unwrap().sounds, 7);
    assert!(state.learning.is_none());
    assert!(!studio.dir.join("learning.lock").exists());
}
#[tokio::test]
async fn stop_and_parent_disconnect_end_paused_children_and_release_locks() {
    for disconnect in [false, true] {
        let studio = Studio::new();
        let mut child = Learner::new();
        child.send(LearnerMessage::Learn { options: options(&studio, true) }).await;
        loop {
            if let LearnerReply::Progress { progress } = child.next().await {
                if progress.phase == kumi_library::learn::LearnPhase::Presets {
                    break;
                }
            }
        }
        if disconnect {
            drop(child.input.take());
        } else {
            child.send(LearnerMessage::Stop).await;
        }
        child.finish().await;
        // A disconnected learner may be force-ended while paused, leaving a stale lock; the next
        // process reclaims it using the PID. Explicit stop must release immediately.
        if !disconnect {
            assert!(!studio.dir.join("learning.lock").exists());
            assert!(read_state(studio.dir.to_str().unwrap()).await.unwrap().learning.is_none());
        }
    }
}
#[tokio::test]
async fn resume_immediately_after_learn_keeps_message_order() {
    let studio = Studio::new();
    let mut child = Learner::new();
    child.send(LearnerMessage::Learn { options: options(&studio, true) }).await;
    child.send(LearnerMessage::Resume).await;
    loop {
        if matches!(child.next().await, LearnerReply::Done { .. }) {
            break;
        }
    }
    child.finish().await;
}

/// The studio's library as the app keeps one, its learner this program (`kumi __library-learner`), learning at once.
fn studio_library(studio: &Studio) -> Rc<Library> {
    create_library(LibraryOptions {
        daws: kumi::daw::registry::libraries(),
        dir: studio.dir.to_string_lossy().into(),
        folders: Some(vec![studio.extra.to_string_lossy().into()]),
        sources: Some(SourceOptions {
            home: Some(studio.home.path().to_string_lossy().into()),
            platform: Some("darwin".into()),
            applications: Some(studio.home.path().join("Applications").to_string_lossy().into()),
            ..Default::default()
        }),
        find_sets: Some(false),
        workers: Some(1),
        delay_ms: Some(0),
        ..Default::default()
    })
}

/// Waits (15 s at most) until `ready`.
async fn until(ready: impl Fn() -> bool) {
    tokio::time::timeout(Duration::from_secs(15), async {
        while !ready() {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    })
    .await
    .expect("in time");
}

/// Quitting just as the library starts learning: a learner still starting (here one that never gets further) reads no
/// stop, so closing the library ends it at once rather than waiting out the time it's given to stop.
#[cfg(unix)]
#[tokio::test(flavor = "current_thread")]
async fn closing_as_the_library_starts_learning_ends_its_learner_at_once() {
    use std::os::unix::fs::PermissionsExt;
    let studio = Studio::new();
    let starting = studio.home.path().join("starting-learner");
    std::fs::write(&starting, "#!/bin/sh\nexec sleep 30\n").unwrap();
    std::fs::set_permissions(&starting, std::fs::Permissions::from_mode(0o700)).unwrap();
    // Read by this test's process alone: each test is a process of its own.
    std::env::set_var("KUMI_LIBRARY_LEARNER_BIN", &starting);
    tokio::task::LocalSet::new()
        .run_until(async {
            let library = studio_library(&studio);
            library.start();
            until(|| library.status().state == kumi_types::contracts::LibraryState::Learning).await;
            let closing = Instant::now();
            library.close().await;
            assert!(closing.elapsed() < Duration::from_millis(500), "{:?}", closing.elapsed());
        })
        .await;
}

/// A learner that has said where it is stops as the library closes, within the time it's given, and puts its state and
/// its lock away.
#[tokio::test(flavor = "current_thread")]
async fn closing_while_the_library_learns_stops_its_learner() {
    let studio = Studio::new();
    tokio::task::LocalSet::new()
        .run_until(async {
            let library = studio_library(&studio);
            // Paused, the learner says what it found (seven sounds) and waits.
            library.pause();
            library.start();
            until(|| library.status().todo == Some(7)).await;
            let closing = Instant::now();
            library.close().await;
            assert!(closing.elapsed() < Duration::from_millis(1500), "{:?}", closing.elapsed());
            assert!(!studio.dir.join("learning.lock").exists());
            assert!(read_state(studio.dir.to_str().unwrap()).await.unwrap().learning.is_none());
        })
        .await;
}

/// A learner ended before it spoke can't let go of a lock it took, so closing the library does: a quit leaves nothing
/// behind. A lock another process holds is theirs, and stays.
#[cfg(unix)]
#[tokio::test(flavor = "current_thread")]
async fn closing_lets_go_of_the_lock_a_learner_was_ended_holding_and_only_that() {
    use std::os::unix::fs::PermissionsExt;
    let studio = Studio::new();
    std::fs::create_dir_all(&studio.dir).unwrap();
    let lock = studio.dir.join("learning.lock");
    // A learner that takes the lock as one does (its pid, which `exec` keeps), then gets no further.
    let locking = studio.home.path().join("locking-learner");
    std::fs::write(
        &locking,
        format!("#!/bin/sh\nprintf '{{\"pid\":%s,\"at\":%s000}}' \"$$\" \"$(date +%s)\" > '{}'\nexec sleep 30\n", lock.display()),
    )
    .unwrap();
    std::fs::set_permissions(&locking, std::fs::Permissions::from_mode(0o700)).unwrap();
    // One that doesn't get as far as the lock.
    let starting = studio.home.path().join("starting-learner");
    std::fs::write(&starting, "#!/bin/sh\nexec sleep 30\n").unwrap();
    std::fs::set_permissions(&starting, std::fs::Permissions::from_mode(0o700)).unwrap();
    tokio::task::LocalSet::new()
        .run_until(async {
            // Read by this test's process alone: each test is a process of its own.
            std::env::set_var("KUMI_LIBRARY_LEARNER_BIN", &locking);
            let library = studio_library(&studio);
            library.start();
            until(|| std::fs::read_to_string(&lock).is_ok_and(|held| held.ends_with('}'))).await;
            library.close().await;
            assert!(!lock.exists());
            // Another's lock, while a learner of this library's is ended before it spoke.
            std::fs::write(&lock, r#"{"pid":1,"at":0}"#).unwrap();
            std::env::set_var("KUMI_LIBRARY_LEARNER_BIN", &starting);
            let library = studio_library(&studio);
            library.start();
            until(|| library.status().state == kumi_types::contracts::LibraryState::Learning).await;
            library.close().await;
            assert_eq!(std::fs::read_to_string(&lock).unwrap(), r#"{"pid":1,"at":0}"#);
        })
        .await;
}

/// One still there after it's ended (here one that won't heed the end it's sent) keeps its lock: it could still write
/// the library, and `still_running` holds the lock for it while it lives.
#[cfg(unix)]
#[tokio::test(flavor = "current_thread")]
async fn a_learner_still_there_after_its_end_keeps_its_lock() {
    use std::os::unix::fs::PermissionsExt;
    let studio = Studio::new();
    std::fs::create_dir_all(&studio.dir).unwrap();
    let lock = studio.dir.join("learning.lock");
    // It takes the lock as a learner does, then ignores the end it's sent (an ignored signal stays ignored past `exec`).
    let stuck = studio.home.path().join("stuck-learner");
    std::fs::write(
        &stuck,
        format!(
            "#!/bin/sh\ntrap '' TERM\nprintf '{{\"pid\":%s,\"at\":%s000}}' \"$$\" \"$(date +%s)\" > '{}'\nexec sleep 30\n",
            lock.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&stuck, std::fs::Permissions::from_mode(0o700)).unwrap();
    // Read by this test's process alone: each test is a process of its own.
    std::env::set_var("KUMI_LIBRARY_LEARNER_BIN", &stuck);
    tokio::task::LocalSet::new()
        .run_until(async {
            let library = studio_library(&studio);
            library.start();
            until(|| std::fs::read_to_string(&lock).is_ok_and(|held| held.ends_with('}'))).await;
            library.close().await;
            let held: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&lock).unwrap()).unwrap();
            let pid = held["pid"].as_i64().unwrap() as libc::pid_t;
            assert_eq!(unsafe { libc::kill(pid, 0) }, 0, "it's still there, its lock its own");
            unsafe { libc::kill(pid, libc::SIGKILL) };
        })
        .await;
}
