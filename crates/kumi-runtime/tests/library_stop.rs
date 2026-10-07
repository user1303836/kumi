//! Stopping a learner mid-measurement. Its own binary: the measuring worker is named by an environment variable.
#[path = "support/library.rs"]
mod fixture;
use fixture::Studio;
use kumi_common::abort::Signal;
use kumi_runtime::library::learn::{learn, LearnOptions, LearnPhase};
use std::{rc::Rc, time::Duration};

#[cfg(unix)]
#[tokio::test]
async fn a_stop_doesnt_wait_for_the_measurement_under_way() {
    use std::os::unix::fs::PermissionsExt;
    let studio = Studio::new();
    let worker = studio.dir.join("hanging-worker");
    std::fs::create_dir_all(&studio.dir).unwrap();
    std::fs::write(&worker, "#!/bin/sh\nexec sleep 30\n").unwrap();
    std::fs::set_permissions(&worker, std::fs::Permissions::from_mode(0o700)).unwrap();
    // This binary's only test reads it, after it's set.
    std::env::set_var("KUMI_LIBRARY_MEASURE_BIN", &worker);
    let signal = Signal::new();
    let stop = signal.clone();
    let mut options = LearnOptions::new(studio.plan(1), signal);
    options.on_progress = Some(Rc::new(move |progress| {
        if progress.phase == LearnPhase::Sounds {
            let stop = stop.clone();
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(300));
                stop.cancel();
            });
        }
    }));
    let started = std::time::Instant::now();
    assert!(learn(options).await.unwrap_err().is_aborted());
    // Not the pool's 90 s timeout: the measurement is left, and its worker ended.
    assert!(started.elapsed() < Duration::from_secs(10), "{:?}", started.elapsed());
}
