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
    std::fs::write(&worker, format!("#!/bin/sh\nprintf '%s' $$ > '{0}.next'\nmv '{0}.next' '{0}'\nexec sleep 30\n", said.display()))
        .unwrap();
    std::fs::set_permissions(&worker, std::fs::Permissions::from_mode(0o700)).unwrap();
    // This binary's only test reads it, after it's set.
    std::env::set_var("KUMI_LIBRARY_MEASURE_BIN", &worker);
    let signal = Signal::new();
    let options = LearnOptions::new(studio.plan(1), signal.clone());
    let learning = learn(options);
    tokio::pin!(learning);
    // Reaching Sounds means the work is queued, not that the OS has scheduled the shell. Cancel only once its
    // atomic marker proves the measuring worker is running.
    let pid: libc::pid_t = tokio::select! {
        result = &mut learning => panic!("learning ended before its worker started: {result:?}"),
        pid = fixture::wait_for_worker_marker(&said) => pid.parse().unwrap(),
    };
    signal.cancel();
    // This guard starts after the handshake: stopping leaves the measurement and ends the worker, instead of
    // waiting for its sleep or the pool's 90 s timeout.
    let result = tokio::time::timeout(Duration::from_secs(10), learning).await.expect("stopping left the measurement under way");
    assert!(result.unwrap_err().is_aborted());
    assert_ne!(unsafe { libc::kill(pid, 0) }, 0, "the worker is gone");
}
