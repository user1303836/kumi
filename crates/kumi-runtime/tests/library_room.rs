//! A learner with no room to convert sounds. Its own binary: the measuring worker is named by an environment variable.
#![cfg(unix)]
#[path = "support/library.rs"]
mod fixture;
use fixture::Studio;
use kumi_common::abort::Signal;
use kumi_runtime::library::learn::{learn, library_logs, LearnOptions};

#[tokio::test]
async fn no_room_to_convert_ends_the_run_and_keeps_nothing_of_the_sounds() {
    use std::os::unix::fs::PermissionsExt;
    let studio = Studio::new();
    std::fs::create_dir_all(&studio.dir).unwrap();
    // A worker with no room to convert: it says so for each sound it's given.
    let worker = studio.dir.join("roomless-worker");
    std::fs::write(
        &worker,
        r#"#!/bin/sh
while IFS= read -r line; do
  id="${line#*\"id\":}"; id="${id%%,*}"
  printf '{"id":%s,"entry":{"path":"x","size":1,"mtime":1},"stop":"Kumi reads that file by converting it first. Only 5 MB is free."}\n' "$id"
done
"#,
    )
    .unwrap();
    std::fs::set_permissions(&worker, std::fs::Permissions::from_mode(0o700)).unwrap();
    // This binary's only test reads it, after it's set.
    std::env::set_var("KUMI_LIBRARY_MEASURE_BIN", &worker);
    let error = learn(LearnOptions::new(studio.plan(1), Signal::new())).await.unwrap_err();
    assert!(error.to_string().contains("Only 5 MB is free"), "{error}");
    // No sound kept with that as its error: the next run, with room, measures them.
    assert!(library_logs(studio.dir.to_str().unwrap()).sounds.load().await.is_empty());
}
