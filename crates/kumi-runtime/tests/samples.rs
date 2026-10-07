#[path = "../../../tests/support/fixture_paths.rs"]
mod fixture_paths;
use kumi_common::{abort::Signal, js::string::locale_compare_numeric_base};
use kumi_runtime::{integrations::ableton::samples::*, library::sources::join};
use serde_json::Value;
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
#[test]
fn a_sample_named_on_a_network_share_is_never_looked_at() {
    use kumi_runtime::integrations::ableton::change_context::SampleBank;
    let folder = tempfile::tempdir().unwrap();
    let root = folder.path().canonicalize().unwrap().to_string_lossy().into_owned();
    std::fs::write(format!("{root}/kick.wav"), b"RIFF").unwrap();
    let bank = SampleBank::default();
    assert!(bank.sample(&format!("{root}/kick.wav")).is_some());
    // "//" before a local path is that same path off Windows: only the refusal keeps it out there.
    for path in [
        format!("/{root}/kick.wav"),
        r"\\host\share\kick.wav".into(),
        r"/\host\share\kick.wav".into(),
        r"\??\UNC\host\share\kick.wav".into(),
    ] {
        assert!(bank.sample(&path).is_none(), "{path}");
    }
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
