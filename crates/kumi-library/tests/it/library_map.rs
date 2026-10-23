//! The sounds map: sounds-map.json read as the layout writes it.
use kumi_library::map::{read_sounds_map, SOUNDS_MAP};

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
