//! The sounds map: sounds-map.json read as the layout writes it, and joined with what the library knows of each sound.
use kumi_library::{
    learn::SoundEntry,
    map::{read_sounds_map, sounds_on_map, sounds_on_map_by_turns, SoundsMap, SOUNDS_MAP},
    search::SoundIndex,
    sources::{Source, SourceKind},
    store::Entry,
    SoundClass, SoundKind,
};

#[test]
fn the_map_is_read_as_the_layout_writes_it_and_nothing_else_is() {
    let dir = tempfile::tempdir().unwrap();
    assert_eq!(read_sounds_map(dir.path()), None, "none yet");
    let file = dir.path().join(SOUNDS_MAP);
    std::fs::write(
        &file,
        r#"{"version": 1, "made": 1700000000000, "points": [["/s/Kick.wav", 0.25, 0.75], ["/s/Off.wav", 1.5, 0.5], ["/s/Snare.wav", 1, 0]]}"#,
    )
    .unwrap();
    let map = read_sounds_map(dir.path()).unwrap();
    assert_eq!(map.made, 1_700_000_000_000);
    assert_eq!(map.points, [("/s/Kick.wav".to_string(), 0.25, 0.75), ("/s/Snare.wav".to_string(), 1., 0.)], "off the map: left out");
    // The layout writes it through the same form.
    let text = serde_json::to_string(&map).unwrap();
    assert!(text.contains(r#"["/s/Kick.wav",0.25,0.75]"#), "{text}");
    // Another form (a newer Kumi's), or a broken file: none.
    std::fs::write(&file, r#"{"version": 2, "made": 1, "points": []}"#).unwrap();
    assert_eq!(read_sounds_map(dir.path()), None);
    std::fs::write(&file, "{").unwrap();
    assert_eq!(read_sounds_map(dir.path()), None);
}

#[test]
fn a_sound_on_the_map_carries_its_name_place_and_class() {
    let source = tempfile::tempdir().unwrap();
    let root = source.path().to_str().unwrap().to_string();
    let path = |relative: &str| format!("{root}{}{}", std::path::MAIN_SEPARATOR, relative.replace('/', std::path::MAIN_SEPARATOR_STR));
    let entry = |relative: &str, class: Option<SoundClass>| SoundEntry {
        file: Entry { path: path(relative), size: 1, mtime: 1, gone: None },
        r#class: class,
        kind: Some(SoundKind::OneShot),
        seconds: Some(0.5),
        ..Default::default()
    };
    let index = SoundIndex::new(
        [entry("Drums/Kicks/Kick 808.wav", Some(SoundClass::Kick)), entry("Pads/Air.wav", None)],
        &[Source { path: root.clone(), label: "Samples".into(), kind: SourceKind::Folder }],
    );
    let map = SoundsMap {
        version: 1,
        made: 1,
        points: vec![(path("Pads/Air.wav"), 0.1, 0.2), (path("Gone.wav"), 0.5, 0.5), (path("Drums/Kicks/Kick 808.wav"), 0.9, 0.8)],
    };
    let sounds = sounds_on_map(&map, &index);
    assert_eq!(sounds.len(), 2, "a sound the library doesn't offer is left out");
    assert_eq!((sounds[0].name.as_str(), sounds[0].place.as_str(), sounds[0].class), ("Air", "Samples / Pads", None));
    assert_eq!(
        (sounds[1].name.as_str(), sounds[1].place.as_str(), sounds[1].class),
        ("Kick 808", "Samples / Drums / Kicks", Some(SoundClass::Kick))
    );
    assert_eq!((sounds[1].x, sounds[1].y, sounds[1].kind, sounds[1].seconds), (0.9, 0.8, Some(SoundKind::OneShot), Some(0.5)));
    // A slice at a time, as Kumi's own thread reads it: the same.
    let by_turns = tokio::runtime::Builder::new_current_thread().build().unwrap().block_on(sounds_on_map_by_turns(&map, &index));
    assert_eq!(by_turns, sounds);
}
