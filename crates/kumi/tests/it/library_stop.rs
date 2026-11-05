//! Stopping a learner mid-measurement. Its own binary: the measuring worker is named by an environment variable.
#[path = "../support/library.rs"]
mod fixture;
use fixture::Studio;
use kumi_common::abort::Signal;
use kumi_library::learn::{learn, LearnOptions};
use std::time::Duration;

#[tokio::test]
async fn a_stop_doesnt_wait_for_the_measurement_under_way() {
    use std::os::unix::fs::PermissionsExt;
    let studio = Studio::new();
    let worker = studio.dir.join("hanging-worker");
    let said = studio.dir.join("worker-pid");
    std::fs::create_dir_all(&studio.dir).unwrap();
    std::fs::write(&worker, format!("#!/bin/sh\nIFS= read -r job || exit 1\necho $$ > '{}'\nexec sleep 30\n", said.display())).unwrap();
    std::fs::set_permissions(&worker, std::fs::Permissions::from_mode(0o700)).unwrap();
    // This binary's only test reads it, after it's set.
    std::env::set_var("KUMI_LIBRARY_MEASURE_BIN", &worker);
    let signal = Signal::new();
    let stop = signal.clone();
    let options = LearnOptions::new(studio.plan(1), signal);
    let mut learning = Box::pin(learn(options));
    // A phase notification precedes worker startup. Cancel only after a worker
    // actually consumes its measurement job, regardless of scheduler load.
    let measuring = async {
        loop {
            if let Some(pid) = std::fs::read_to_string(&said)
                .ok()
                .filter(|text| text.ends_with('\n'))
                .and_then(|text| text.trim().parse::<libc::pid_t>().ok())
                .filter(|pid| *pid > 0)
            {
                break pid;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    };
    let pid = tokio::select! {
        pid = measuring => pid,
        _ = &mut learning => panic!("the learner ended before the measurement worker consumed its job"),
    };
    stop.cancel();
    assert!(learning.await.unwrap_err().is_aborted());
    assert_ne!(unsafe { libc::kill(pid, 0) }, 0, "the worker is gone");
}
