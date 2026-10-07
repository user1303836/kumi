#[path = "support/library.rs"]
mod fixture;
use fixture::{put, Studio};
use kumi_common::abort::Signal;
use kumi_runtime::{
    core::contracts::{KernelTool, LibraryState},
    library::{
        create_library, library_dir, sources::SourceOptions, tools::LibraryAccess, LearnNowOptions, Library, LibraryOptions,
        LibraryToolsOptions,
    },
};
use serde_json::{json, Value};
use std::{cell::RefCell, rc::Rc, time::Duration};
fn make(studio: &Studio, fork: bool) -> Rc<Library> {
    create_library(LibraryOptions {
        dir: studio.dir.to_string_lossy().into(),
        folders: Some(vec![studio.extra.to_string_lossy().into()]),
        sources: Some(SourceOptions {
            home: Some(studio.home.path().to_string_lossy().into()),
            platform: Some("darwin".into()),
            applications: Some(studio.home.path().join("Applications").to_string_lossy().into()),
            ..Default::default()
        }),
        fork: Some(fork),
        workers: Some(if fork { 1 } else { 0 }),
        find_sets: Some(false),
        delay_ms: Some(0),
        ..Default::default()
    })
}
async fn result(tools: &[Rc<dyn KernelTool>], name: &str, input: Value) -> Value {
    let result = tools.iter().find(|t| t.name() == name).unwrap().execute(input.as_object().unwrap().clone(), Signal::new()).await.unwrap();
    assert!(!result.is_error, "{}", result.text);
    serde_json::from_str(&result.text).unwrap()
}
async fn wait(check: impl Fn() -> bool) {
    tokio::time::timeout(Duration::from_secs(20), async {
        while !check() {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
}
#[tokio::test]
async fn full_library_learns_searches_compares_files_describes_sets_and_remembers_taste() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let studio = Studio::new();
            let library = make(&studio, false);
            assert_eq!(library.status().state, LibraryState::New);
            let learned = library.learn_now(LearnNowOptions::new(Signal::new())).await.unwrap().unwrap();
            assert_eq!([learned.sounds.known, learned.presets.known, learned.sets.known], [7, 4, 2]);
            assert_eq!(library.status().state, LibraryState::Ready);
            let tools = library.tools(LibraryToolsOptions::default());
            assert_eq!(tools.len(), 4);
            let kicks = result(&tools, "find_sounds", json!({"words":["kick"]})).await;
            assert_eq!(
                kicks["sounds"].as_array().unwrap().iter().map(|s| s["name"].as_str().unwrap()).collect::<Vec<_>>(),
                ["Kick Deep", "Kick Short", "Untitled 7"]
            );
            assert!(kicks["sounds"][2]["why"].as_str().unwrap().contains("a kick by its sound"));
            let loops = result(&tools, "find_sounds", json!({"kind":"loop","tempo":120})).await;
            assert_eq!(loops["sounds"][0]["name"], "Beat 120 bpm");
            assert_eq!(loops["sounds"].as_array().unwrap().len(), 1);
            let dark = result(&tools, "find_sounds", json!({"words":["kick","dark"]})).await;
            assert_eq!(dark["sounds"][0]["name"], "Kick Deep");
            assert!(dark["sounds"][0]["why"].as_str().unwrap().contains("dark: brightness"));
            let reference = studio.home.path().join("reference-hat.wav");
            put(&reference, fixture::wav(&[fixture::hat(0.1, 9)], 44100));
            let like = result(&tools, "find_sounds", json!({"like":reference,"limit":1})).await;
            assert_eq!(like["sounds"][0]["name"], "Hat Closed");
            let known = library.sound_index().await.unwrap();
            let again = library.sound_index().await.unwrap();
            assert!(Rc::ptr_eq(&known, &again), "unchanged readers retain the in-memory index");
            let wavetable = result(&tools, "find_presets", json!({"device":"Wavetable"})).await;
            assert_eq!(wavetable["presets"][0]["browser"], "user_library/Presets/Instruments/Wavetable/Rolling Bass.adv");
            let night = result(&tools, "my_sets", json!({"set":"night drive"})).await;
            assert_eq!(night["arrangement"], "32 bars");
            assert_eq!(night["main"], json!(["Glue Compressor", "Limiter"]));
            let bass = night["tracks"].as_array().unwrap().iter().find(|t| t["name"] == "Reese Bass").unwrap();
            assert_eq!(bass["devices"], json!(["Serum [VST3]", "Saturator", "EQ Eight"]));
            assert_eq!(bass["role"], "bass");
            assert!(library.instructions().await.unwrap().contains("Tempo: usually 124–126 BPM"));
            assert!(library.forget_taste("tempo").await.unwrap());
            assert!(!library.forget_taste("tempo").await.unwrap());
            assert!(!library.forget_taste("unknown").await.unwrap());
            assert!(!library.taste().await.unwrap().iter().any(|t| t.id == "tempo"));
            assert!(!library.instructions().await.unwrap().contains("Tempo: usually"));
            library.close().await;
            let reopened = make(&studio, false);
            wait(|| reopened.status().state == LibraryState::Ready).await;
            // Searches that come together while there's no index wait for one build and share it (#258), where each
            // used to build its own.
            let together = futures::future::join_all((0..4).map(|_| reopened.sound_index())).await;
            let first = together[0].as_ref().unwrap();
            assert!(
                together.iter().all(|index| Rc::ptr_eq(first, index.as_ref().unwrap())),
                "one build serves searches that came together"
            );
            assert_eq!(reopened.status().sounds, 7);
            assert!(!reopened.taste().await.unwrap().iter().any(|t| t.id == "tempo"));
            reopened.close().await;
        })
        .await;
}
#[tokio::test]
async fn background_learning_holds_while_paused_notifies_and_closes_native_child() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let studio = Studio::new();
            let library = make(&studio, true);
            let statuses = Rc::new(RefCell::new(vec![]));
            let seen = statuses.clone();
            let unsubscribe = library.on_status(Rc::new(move |s| seen.borrow_mut().push(s.state)));
            library.pause();
            library.start();
            wait(|| statuses.borrow().contains(&LibraryState::Paused)).await;
            tokio::time::sleep(Duration::from_millis(600)).await;
            assert_eq!(library.status().sounds, 0);
            library.resume();
            wait(|| library.status().state == LibraryState::Ready).await;
            assert_eq!([library.status().sounds, library.status().presets, library.status().sets], [7, 4, 2]);
            assert!(statuses.borrow().contains(&LibraryState::Learning));
            unsubscribe();
            let count = statuses.borrow().len();
            library.pause();
            tokio::time::sleep(Duration::from_millis(450)).await;
            assert_eq!(statuses.borrow().len(), count);
            library.close().await;
            let paused = make(&studio, true);
            paused.pause();
            paused.start();
            wait(|| paused.status().state == LibraryState::Paused).await;
            paused.close().await;
            wait(|| !studio.dir.join("learning.lock").exists()).await;
        })
        .await;
}
#[tokio::test]
async fn unknown_folders_are_saved_and_learned_and_reference_cancellation_stops_worker() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let studio = Studio::new();
            let library = make(&studio, false);
            library.learn_now(LearnNowOptions::new(Signal::new())).await.unwrap();
            let downloads = studio.home.path().join("Downloads");
            put(&downloads.join("New Kick.wav"), fixture::wav(&[fixture::kick(50., 0.2)], 44100));
            let tools = library.tools(LibraryToolsOptions::default());
            let found = result(&tools, "find_sounds", json!({"words":["kick"],"folders":[downloads]})).await;
            assert_eq!(found["sounds"][0]["name"], "New Kick");
            assert!(found["note"].as_str().unwrap().contains("names only"));
            wait(|| library.status().sounds == 8 && library.status().state == LibraryState::Ready).await;
            let remembered: Vec<String> = serde_json::from_slice(&std::fs::read(studio.dir.join("folders.json")).unwrap()).unwrap();
            assert_eq!(remembered, vec![downloads.to_string_lossy()]);
            let signal = Signal::new();
            signal.cancel();
            let measured = LibraryAccess::measure(
                library.as_ref(),
                downloads.join("New Kick.wav").to_string_lossy().into(),
                kumi_runtime::library::features::MeasureOptions { signal: Some(signal), ..Default::default() },
            )
            .await;
            assert!(measured.unwrap_err().is_aborted());
            library.close().await;
        })
        .await;
}
#[test]
fn library_location_honors_only_nonempty_override() {
    let env = std::collections::HashMap::from([("KUMI_LIBRARY_DIR".into(), "/kept".into())]);
    assert_eq!(library_dir("/kumi", Some(&env)), "/kept");
    let empty = std::collections::HashMap::from([("KUMI_LIBRARY_DIR".into(), String::new())]);
    assert_eq!(library_dir("/kumi", Some(&empty)), kumi_runtime::library::sources::join("/kumi", "library"));
}
