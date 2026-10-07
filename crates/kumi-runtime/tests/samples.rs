#[path = "../../../tests/support/fixture_paths.rs"]
mod fixture_paths;
use kumi_common::{abort::Signal, js::string::locale_compare_numeric_base};
use kumi_runtime::{
    integrations::ableton::{change_context::SampleBank, changes::NoSample, samples::*},
    library::sources::join,
    RuntimeError,
};
use serde_json::Value;
use std::path::{Path, PathBuf};
fn oracle() -> Value {
    serde_json::from_str(include_str!("support/samples-oracle.json")).unwrap()
}
#[test]
fn wav_aiff_partial_odd_streamed_and_malformed_headers_match_source() {
    for (i, case) in oracle()["headers"].as_array().unwrap().iter().enumerate() {
        let result = audio_seconds(&hex::decode(case["hex"].as_str().unwrap()).unwrap(), case["bytes"].as_f64().unwrap());
        assert_eq!(result.is_err(), case["throws"], "case {i}: {case}");
        if let Ok(result) = result {
            assert_eq!(result, case["value"].as_f64(), "case {i}: {case}");
        }
    }
}
/// A local folder spelled as a share or device path that reads as that folder: "//" before it off Windows, and on
/// Windows "\\?\" (its canonical spelling), which is refused like a share. A real share of a local folder needs SMB.
fn as_share(folder: &Path) -> PathBuf {
    if cfg!(windows) {
        folder.canonicalize().unwrap()
    } else {
        PathBuf::from(format!("/{}", folder.display()))
    }
}
fn text(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}
#[tokio::test(flavor = "current_thread")]
async fn a_sample_named_on_a_network_share_is_never_looked_at() {
    let folder = tempfile::tempdir().unwrap();
    let kick = folder.path().join("kick.wav");
    std::fs::write(&kick, b"RIFF").unwrap();
    let bank = SampleBank::default();
    *bank.places.borrow_mut() = Some(vec![]);
    let signal = Signal::new();
    assert!(bank.sample(&text(&kick), &signal).await.unwrap().is_ok());
    // The first is that same file: only the refusal keeps it out.
    for path in
        [text(&as_share(&kick)), r"\\host\share\kick.wav".into(), r"/\host\share\kick.wav".into(), r"\??\UNC\host\share\kick.wav".into()]
    {
        assert_eq!(bank.sample(&path, &signal).await.unwrap(), Err(NoSample::NotThere), "{path}");
    }
}
#[tokio::test(flavor = "current_thread")]
async fn a_sound_in_one_of_lives_places_on_a_share_is_copied_here_first() {
    let folder = tempfile::tempdir().unwrap();
    for (path, bytes) in [("Place/Kicks/kick.wav", &b"RIFF one"[..]), ("Other/snare.wav", b"RIFF")] {
        let path = folder.path().join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, bytes).unwrap();
    }
    let share = as_share(folder.path());
    let bank = SampleBank::default();
    *bank.places.borrow_mut() = Some(vec![text(&share.join("Place"))]);
    let cache = folder.path().join("cache");
    *bank.cache.borrow_mut() = Some(cache.clone());
    let signal = Signal::new();
    let shared = text(&share.join("Place").join("Kicks").join("kick.wav"));
    let copied = bank.sample(&shared, &signal).await.unwrap().unwrap();
    assert!(Path::new(&copied.path).starts_with(&cache) && copied.path.ends_with("kick.wav"), "{copied:?}");
    assert_eq!(text(Path::new(&copied.path).parent().unwrap()), copied.folder);
    assert_eq!(std::fs::read(&copied.path).unwrap(), b"RIFF one");
    // The same file again: the copy is kept. Changed at the Place: copied again, beside the first.
    assert_eq!(bank.sample(&shared, &signal).await.unwrap().unwrap(), copied);
    std::fs::write(folder.path().join("Place/Kicks/kick.wav"), b"RIFF two!").unwrap();
    let changed = bank.sample(&shared, &signal).await.unwrap().unwrap();
    assert_ne!(changed.folder, copied.folder);
    assert_eq!(std::fs::read(&changed.path).unwrap(), b"RIFF two!");
    assert_eq!(std::fs::read(&copied.path).unwrap(), b"RIFF one");
    // Another share isn't opened, nor the Place's share outside it, and nothing of them is copied.
    for path in [share.join("Other").join("snare.wav"), share.join("Place").join("..").join("Other").join("snare.wav")] {
        assert_eq!(bank.sample(&text(&path), &signal).await.unwrap(), Err(NoSample::NotThere), "{path:?}");
    }
    assert_eq!(std::fs::read_dir(&cache).unwrap().count(), 2);
}
#[tokio::test(flavor = "current_thread")]
async fn a_share_sound_kumi_cant_copy_says_why_and_copies_nothing() {
    let folder = tempfile::tempdir().unwrap();
    let place = folder.path().join("Place");
    std::fs::create_dir(&place).unwrap();
    std::fs::write(place.join("empty.wav"), b"").unwrap();
    // Bigger than an import takes (sparse, so it takes no room).
    std::fs::File::create(place.join("huge.wav")).unwrap().set_len(512 * 1024 * 1024 + 1).unwrap();
    std::fs::write(place.join("kick.wav"), b"RIFF").unwrap();
    let share = as_share(folder.path());
    let bank = SampleBank::default();
    // "Gone" is a Place Live names on a share that's off.
    *bank.places.borrow_mut() = Some(vec![text(&share.join("Place")), text(&share.join("Gone"))]);
    let cache = folder.path().join("cache");
    *bank.cache.borrow_mut() = Some(cache.clone());
    let signal = Signal::new();
    for (path, why) in [
        (share.join("Gone").join("kick.wav"), NoSample::ShareUnread),
        (share.join("Place").join("missing.wav"), NoSample::NotThere),
        (share.join("Place").join("empty.wav"), NoSample::OutOfBounds),
        (share.join("Place").join("huge.wav"), NoSample::OutOfBounds),
    ] {
        assert_eq!(bank.sample(&text(&path), &signal).await.unwrap(), Err(why), "{path:?}");
    }
    // A stopped change copies nothing either.
    signal.cancel();
    assert!(matches!(bank.sample(&text(&share.join("Place").join("kick.wav")), &signal).await, Err(RuntimeError::Aborted)));
    assert!(!cache.exists());
}
#[test]
fn natural_sample_name_sort_matches_source_numeric_and_base_collation() {
    let data = oracle();
    let names = data["collation"]["names"].as_array().unwrap();
    for (i, a) in names.iter().enumerate() {
        for (j, b) in names.iter().enumerate() {
            assert_eq!(
                locale_compare_numeric_base(a.as_str().unwrap(), b.as_str().unwrap()),
                data["collation"]["pairs"][i * names.len() + j].as_i64().unwrap().cmp(&0),
                "{a} vs {b}"
            );
        }
    }
}
fn library(data: &Value) -> tempfile::TempDir {
    let root = tempfile::tempdir().unwrap();
    for file in data["files"].as_array().unwrap() {
        let path = root.path().join(file["name"].as_str().unwrap());
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, hex::decode(file["hex"].as_str().unwrap()).unwrap()).unwrap();
    }
    #[cfg(unix)]
    std::os::unix::fs::symlink(root.path(), root.path().join("Drums/loop")).unwrap();
    #[cfg(windows)]
    std::os::windows::fs::symlink_dir(root.path(), root.path().join("Drums/loop")).unwrap();
    root
}
#[tokio::test]
async fn ranked_queries_skip_metadata_hidden_files_and_links_and_describe_only_results() {
    let data = oracle();
    let root = library(&data);
    let root = root.path().to_string_lossy().into_owned();
    for case in data["cases"].as_array().unwrap() {
        let result = find_samples(FindSamplesOptions {
            folders: vec![join(&root, "Nowhere"), root.clone()],
            words: case["words"].as_array().unwrap().iter().map(|v| v.as_str().unwrap().to_string()).collect(),
            limit: case["limit"].as_u64().unwrap() as usize,
            ..Default::default()
        })
        .await
        .unwrap();
        let value = fixture_paths::map_strings(&serde_json::to_value(&result).unwrap(), &|text| {
            fixture_paths::normalize_root(text, &root, "$ROOT")
        });
        assert_eq!(value, case["value"], "{case}");
    }
    let random =
        find_samples(FindSamplesOptions { folders: vec![root], words: vec!["drums".into()], limit: 2, random: true, ..Default::default() })
            .await
            .unwrap();
    assert_eq!(random.samples.len(), 2);
    assert!(random.samples.iter().all(|sample| sample.path.contains("Drums")));
    assert_ne!(random.samples[0].path, random.samples[1].path);
}
#[test]
fn folder_expansion_and_windows_defaults_use_host_path_rules() {
    assert_eq!(folder_path("~/Samples", Some("/home/me")), Some(kumi_runtime::library::sources::resolve(&join("/home/me", "Samples"))));
    assert_eq!(folder_path("Samples", Some("/home/me")), None);
    assert_eq!(user_library(Some("darwin"), Some("/Users/me")), join("/Users/me", "Music/Ableton/User Library"));
    assert_eq!(user_library(Some("win32"), Some("C:\\Users\\me")), join("C:\\Users\\me", "Documents/Ableton/User Library"));
    let root = tempfile::tempdir().unwrap();
    let home = root.path().join("home");
    let data = root.path().join("ProgramData");
    for path in [
        home.join("Documents/Ableton/User Library"),
        home.join("Documents/Ableton/Factory Packs"),
        data.join("Ableton/Live 12/Resources/Core Library/Samples"),
        data.join("Ableton/Live 11/Resources/Core Library/Samples"),
    ] {
        std::fs::create_dir_all(path).unwrap();
    }
    let folders = default_sample_folders(Some("win32"), home.to_str(), data.to_str());
    assert_eq!(folders.len(), 3);
    assert!(folders[1].contains("Live 11"));
}
#[tokio::test]
async fn cancellation_match_cap_and_depth_cap_bound_search_work() {
    let root = tempfile::tempdir().unwrap();
    let signal = Signal::new();
    signal.cancel();
    assert!(find_samples(FindSamplesOptions {
        folders: vec![root.path().to_string_lossy().into_owned()],
        signal: Some(signal),
        ..Default::default()
    })
    .await
    .unwrap_err()
    .is_aborted());
    let mut deep = root.path().to_path_buf();
    for _ in 0..14 {
        deep.push("deep");
        std::fs::create_dir(&deep).unwrap();
    }
    std::fs::write(deep.join("hidden.wav"), []).unwrap();
    for i in 0..5001 {
        std::fs::write(root.path().join(format!("{i}.wav")), []).unwrap();
    }
    let result =
        find_samples(FindSamplesOptions { folders: vec![root.path().to_string_lossy().into_owned()], limit: 2, ..Default::default() })
            .await
            .unwrap();
    assert_eq!(result.scanned, 5001);
    assert_eq!(result.matched, 5000);
    assert!(result.partial);
    assert_eq!(result.samples.len(), 2);
}
