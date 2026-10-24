//! The tags pass at the size of a big library: 300,000 songs well inside ten minutes (their tags faked, so this times
//! the walk's bookkeeping and music.db, not the disk), and nothing written when nothing changed. In `extended`.

use kumi_common::abort::Signal;
use kumi_listening::music::{
    folders::Walked,
    scan::{apply_walk, TagsPass},
    store::MusicStore,
    tags::from_path,
};
use std::{path::Path, time::Instant};

const SONGS: usize = 300_000;

#[test]
fn three_hundred_thousand_songs_pass_well_inside_ten_minutes() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = MusicStore::open(&dir.path().join("music.db")).unwrap();
    let folder = "/library".to_string();
    let walked = || -> Vec<Walked> {
        (0..SONGS)
            .map(|n| Walked {
                path: format!("/library/Artist {}/Album {}/{:02} Song {n}.mp3", n / 400, n / 12, n % 12 + 1),
                folder: folder.clone(),
                size: 5_000_000 + n as u64,
                mtime: 1_700_000_000_000 + n as i64,
                ..Default::default()
            })
            .collect()
    };
    let read = |file: &Path, folder: &Path| from_path(file, folder);
    let signal = Signal::new();
    let pass = TagsPass { folders: std::slice::from_ref(&folder), skip: &[], signal: &signal, readers: 4, progress: &|_| {}, hold: &|| {} };
    let started = Instant::now();
    let first = apply_walk(&mut store, std::slice::from_ref(&folder), std::slice::from_ref(&folder), &[], walked(), &read, &pass).unwrap();
    let took = started.elapsed();
    assert_eq!((first.new, store.counts().unwrap().songs), (SONGS, SONGS));
    eprintln!("{SONGS} songs' first pass: {took:?}");
    assert!(took.as_secs() < 600, "{took:?}");
    let started = Instant::now();
    let again = apply_walk(&mut store, std::slice::from_ref(&folder), std::slice::from_ref(&folder), &[], walked(), &read, &pass).unwrap();
    eprintln!("again, nothing changed: {:?}", started.elapsed());
    assert_eq!(again.written, 0);
}
