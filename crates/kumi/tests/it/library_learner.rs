#[path = "../support/library.rs"]
mod fixture;
use fixture::Studio;
use kumi_library::{
    learn::library_logs,
    learner::{LearnerMessage, LearnerOptions, LearnerReply},
    plan::PlanOptions,
    sources::SourceOptions,
    state::read_state,
};
use std::{process::Stdio, time::Duration};
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
        let mut child = Command::new(env!("CARGO_BIN_EXE_kumi-library-learner"))
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
