#[path = "support/library_fixture_paths.rs"]
mod library_fixture_paths;
use kumi_runtime::library::{
    classify::{ClassFrom, SoundClass, SoundKind},
    features::VECTOR_LENGTH,
    learn::{PresetEntry, SetEntry, SoundEntry},
    search::{
        class_for_word, describe_track, is_descriptor, search_presets, search_sets, LikeSound, PresetQuery, SetQuery, SoundIndex,
        SoundQuery,
    },
    sets::SetTrack,
    sources::{Source, SourceKind},
    store::{pack_vector, Entry},
};
use serde::Serialize;
use serde_json::Value;
fn normalized(value: impl Serialize) -> Value {
    serde_json::from_str(&kumi_common::js::json::stringify(&serde_json::to_value(value).unwrap())).unwrap()
}
#[tokio::test]
async fn rankings_explanations_float32_similarity_and_rows_match_typescript() {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir(root.path().join("Nested")).unwrap();
    let fixture = library_fixture_paths::load(include_str!("support/library-search-oracle.json"), root.path());
    let entries: Vec<SoundEntry> = serde_json::from_value(fixture["entries"].clone()).unwrap();
    let sources: Vec<Source> = serde_json::from_value(fixture["sources"].clone()).unwrap();
    let index = SoundIndex::new(entries.clone(), &sources);
    let built = SoundIndex::build(entries, &sources).await;
    assert_eq!(index.size(), fixture["size"].as_u64().unwrap() as usize);
    assert_eq!(index.measured(), fixture["measured"].as_u64().unwrap() as usize);
    for case in fixture["soundCases"].as_array().unwrap() {
        let query: SoundQuery = serde_json::from_value(case["query"].clone()).unwrap();
        assert_eq!(normalized(index.search(&query)), case["expected"], "{}", case["query"]);
        assert_eq!(normalized(built.search(&query)), case["expected"], "async {}", case["query"]);
    }
    let presets: Vec<PresetEntry> = serde_json::from_value(fixture["presets"].clone()).unwrap();
    for case in fixture["presetCases"].as_array().unwrap() {
        let query: PresetQuery = serde_json::from_value(case["query"].clone()).unwrap();
        assert_eq!(normalized(search_presets(&presets, &query)), case["expected"], "preset {}", case["query"]);
    }
    let sets: Vec<SetEntry> = serde_json::from_value(fixture["sets"].clone()).unwrap();
    for case in fixture["setCases"].as_array().unwrap() {
        let query: SetQuery = serde_json::from_value(case["query"].clone()).unwrap();
        assert_eq!(normalized(search_sets(&sets, &query)), case["expected"], "set {}", case["query"]);
    }
    for case in fixture["tracks"].as_array().unwrap() {
        let track: SetTrack = serde_json::from_value(case["track"].clone()).unwrap();
        assert_eq!(normalized(describe_track(&track)), case["expected"]);
    }
    for case in fixture["classWords"].as_array().unwrap() {
        let word = case["word"].as_str().unwrap();
        assert_eq!(normalized(class_for_word(word)), case["class"], "{word}");
        assert_eq!(is_descriptor(word), case["descriptor"].as_bool().unwrap(), "{word}");
    }
    let result = index.search(&SoundQuery { random: true, limit: 8, ..Default::default() });
    assert_eq!(result.matched, index.size());
    assert_eq!(result.hits.len(), 8);
    assert_eq!(result.hits.iter().map(|h| &h.entry.path).collect::<std::collections::HashSet<_>>().len(), 8);
}
#[test]
fn a_place_at_a_drive_or_share_root_holds_its_sounds() {
    use kumi_runtime::library::sources::below;
    assert_eq!(below("/Drums/Kicks/808.wav", "/"), Some("Drums/Kicks/808.wav"));
    assert_eq!(below("/Samples/808.wav", "/Samples"), Some("808.wav"));
    assert_eq!(below("/Samples Extra/808.wav", "/Samples"), None);
    assert_eq!(below("/Samples", "/Samples"), None);
    if cfg!(windows) {
        assert_eq!(below("Z:\\Drums/808.wav", "Z:\\Drums"), Some("808.wav"));
        assert_eq!(below("Z:\\Drums\\808.wav", "Z:\\"), Some("Drums\\808.wav"));
        assert_eq!(below("\\\\NAS\\Samples\\Drums\\808.wav", "\\\\NAS\\Samples\\"), Some("Drums\\808.wav"));
        assert_eq!(below("Z:\\Drums\\808.wav", "Z:\\Drums"), Some("808.wav"));
    }
    // A place at the root of the disk: its sounds are found, said where they are, and kept to a folder asked for.
    let sound = |path: &str| SoundEntry {
        file: Entry { path: path.into(), size: 1000, mtime: 1, gone: None },
        kind: Some(SoundKind::OneShot),
        r#class: Some(SoundClass::Kick),
        class_from: Some(ClassFrom::Name),
        ..Default::default()
    };
    let index = SoundIndex::new(
        [sound("/Drums/Kicks/808 kick.wav"), sound("/Loops/kick loop.wav")],
        &[Source { path: "/".into(), label: "Disk".into(), kind: SourceKind::Place }],
    );
    assert_eq!(index.size(), 2);
    assert!(index.holds("/Drums"));
    let found = index.search(&SoundQuery { words: vec!["808".into()], limit: 5, ..Default::default() });
    assert_eq!(
        found.hits.iter().map(|hit| (hit.name.as_str(), hit.r#where.as_str())).collect::<Vec<_>>(),
        [("808 kick", "Disk / Drums / Kicks")]
    );
    let kept = index.search(&SoundQuery { folders: Some(vec!["/Loops".into()]), limit: 5, ..Default::default() });
    assert_eq!(kept.hits.iter().map(|hit| hit.name.as_str()).collect::<Vec<_>>(), ["kick loop"]);
}
#[test]
fn fifty_thousand_sounds_search_with_bounded_latency() {
    let root = tempfile::tempdir().unwrap();
    let mut seed = 1_u32;
    let mut random = || {
        seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
        seed as f64 / 4294967296.
    };
    let classes = [
        SoundClass::Kick,
        SoundClass::Snare,
        SoundClass::Hat,
        SoundClass::Bass,
        SoundClass::Pad,
        SoundClass::Vocal,
        SoundClass::Fx,
        SoundClass::Perc,
    ];
    let entries: Vec<_> = (0..50000)
        .map(|index| {
            let class = classes[index % 8];
            SoundEntry {
                file: Entry {
                    path: root
                        .path()
                        .join(format!(
                            "Pack {}/{}s/{} {index} {}.wav",
                            index % 97,
                            class,
                            class,
                            ["dark", "bright", "vinyl", "tight"][index % 4]
                        ))
                        .to_string_lossy()
                        .into(),
                    size: 1000 + index as u64,
                    mtime: 1,
                    gone: None,
                },
                seconds: Some(0.2 + random() * 4.),
                kind: Some(if index % 5 != 0 { SoundKind::OneShot } else { SoundKind::Loop }),
                r#class: Some(class),
                class_from: Some(ClassFrom::Name),
                bpm: if index % 5 == 0 { Some((80 + index % 9 * 10) as f64) } else { None },
                loudness: Some(-20. + random() * 10.),
                peak: Some(-1.),
                brightness: Some(200. + random() * 9000.),
                flatness: Some(random() * 0.5),
                attack: Some(random() * 50.),
                decay: Some(50. + random() * 900.),
                width: Some(random()),
                onsets: Some(random() * 5.),
                low: Some(random()),
                high: Some(random()),
                vector: Some(pack_vector(&(0..VECTOR_LENGTH).map(|_| random() * 4. - 2.).collect::<Vec<_>>())),
                features: Some(1),
                ..Default::default()
            }
        })
        .collect();
    let index = SoundIndex::new(
        entries,
        &[Source { path: root.path().to_string_lossy().into(), label: "Library".into(), kind: SourceKind::Folder }],
    );
    drop(root);
    assert_eq!(index.size(), 50000);
    let like = LikeSound {
        vector: (0..VECTOR_LENGTH).map(|_| random() * 4. - 2.).collect(),
        name: "ref.wav".into(),
        brightness: 1000.,
        attack: 5.,
        seconds: 1.,
        path: None,
    };
    for query in [
        SoundQuery { words: vec!["kick".into()], limit: 20, ..Default::default() },
        SoundQuery { words: vec!["dusty".into(), "snare".into(), "vinyl".into()], limit: 20, ..Default::default() },
        SoundQuery { classes: vec![SoundClass::Bass], kind: Some(SoundKind::Loop), bpm: Some(120.), limit: 20, ..Default::default() },
        SoundQuery { like: Some(like.clone()), limit: 20, ..Default::default() },
        SoundQuery { like: Some(like), classes: vec![SoundClass::Kick], words: vec!["dark".into()], limit: 50, ..Default::default() },
        SoundQuery { words: vec!["pack 12".into()], random: true, limit: 10, ..Default::default() },
    ] {
        index.search(&query);
        let mut times = vec![];
        for _ in 0..5 {
            let start = std::time::Instant::now();
            index.search(&query);
            times.push(start.elapsed());
        }
        times.sort();
        eprintln!("50k search {:?}: {:?}", query.words, times[2]);
        // 100 ms for an optimized build; a debug build on a shared runner gets more, which still catches a search
        // that stops using its index.
        let bound = std::time::Duration::from_millis(if cfg!(debug_assertions) { 250 } else { 100 });
        assert!(times[2] < bound, "median {:?}: {:?}", query.words, times[2]);
    }
    assert_eq!(index.search(&SoundQuery { words: vec!["kick".into(), "dark".into()], limit: 5, ..Default::default() }).hits.len(), 5);
}
