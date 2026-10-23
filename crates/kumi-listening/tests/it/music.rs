//! The producer's music by its tags: the usual places, the walk and its skips, the tags pass and music.db.

use kumi_common::abort::Signal;
use kumi_listening::music::{
    folders::{found_in, Found, Walked},
    scan::{tags_pass, TagsPass, TagsPassed},
    store::{MusicStore, PROTECTED},
    tags::{NamesFrom, Tags},
};
use lofty::{
    config::WriteOptions,
    tag::{Accessor, ItemKey, Tag, TagExt, TagType},
};
use std::path::{Path, PathBuf};

/// A short song (a quiet tone), tagged with an artist, a title and an album when they're given.
fn song(path: &Path, seconds: f64, tags: Option<(&str, &str, &str)>) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let rate = 8_000;
    let samples: Vec<f32> = (0..(seconds * rate as f64) as usize).map(|n| (n as f32 * 0.05).sin() * 0.1).collect();
    std::fs::write(path, kumi_media::audio::decode::float_wav(&[samples], rate)).unwrap();
    if let Some((artist, title, album)) = tags {
        let mut tag = Tag::new(TagType::Id3v2);
        tag.set_artist(artist.into());
        tag.set_title(title.into());
        tag.set_album(album.into());
        tag.set_genre("Dubstep".into());
        tag.set_track(2);
        tag.insert_text(ItemKey::RecordingDate, "2007-10-29".into());
        tag.save_to_path(path, WriteOptions::default()).unwrap();
    }
}

fn text(path: &Path) -> String {
    path.to_str().unwrap().to_string()
}

/// A path under `base`, a component at a time: the system's own separators, as the walk keeps them.
fn at(base: &Path, relative: &str) -> PathBuf {
    relative.split('/').fold(base.to_path_buf(), |path, part| path.join(part))
}

fn pass(store: &mut MusicStore, folders: &[String], skip: &[String]) -> TagsPassed {
    let signal = Signal::new();
    tags_pass(store, &TagsPass { folders, skip, signal: &signal, readers: 2, progress: &|_| {} }).unwrap()
}

/// A tree of songs as a producer keeps them, with what a walk leaves out beside them.
fn tree(root: &Path) -> PathBuf {
    let music = root.join("Music");
    song(&at(&music, "Burial/Untrue/02 Archangel.wav"), 3., Some(("Burial", "Archangel", "Untrue")));
    song(&at(&music, "Someone/An Album/01 Untagged Song.wav"), 2., None);
    // A DAW project's folder: its bounce and samples are the project's.
    song(&at(&music, "Project/Bounce.wav"), 1., None);
    std::fs::write(at(&music, "Project/Song.als"), b"a project").unwrap();
    song(&at(&music, ".hidden/Secret.wav"), 1., None);
    song(&at(&music, "Samples/Kick.wav"), 1., None);
    std::fs::write(music.join("Purchased.m4p"), b"protected audio").unwrap();
    std::fs::write(music.join("Clip.mp4"), b"a video").unwrap();
    std::fs::write(music.join("cover.jpg"), b"a picture").unwrap();
    // A link back up the tree.
    #[cfg(unix)]
    std::os::unix::fs::symlink(&music, music.join("Loop")).unwrap();
    music
}

#[test]
fn songs_are_known_by_their_tags_or_their_paths() {
    let dir = tempfile::tempdir().unwrap();
    let music = tree(dir.path());
    let mut store = MusicStore::open(&at(&dir.path(), "kumi/music/music.db")).unwrap();
    let skip = [text(&music.join("Samples"))];
    let passed = pass(&mut store, &[text(&music)], &skip);
    assert_eq!((passed.walked, passed.new, passed.gone), (3, 3, 0), "{passed:?}");

    let tagged = store.song_at(&text(&at(&music, "Burial/Untrue/02 Archangel.wav"))).unwrap().unwrap();
    assert_eq!(tagged.tags.names_from, NamesFrom::Tags);
    assert_eq!(
        (tagged.tags.artist.as_deref(), tagged.tags.title.as_deref(), tagged.tags.album.as_deref()),
        (Some("Burial"), Some("Archangel"), Some("Untrue"))
    );
    assert_eq!((tagged.tags.genre.as_deref(), tagged.tags.year, tagged.tags.track), (Some("Dubstep"), Some(2007), Some(2)));
    assert!(tagged.tags.seconds.is_some_and(|seconds| (seconds - 3.).abs() < 0.1), "{:?}", tagged.tags.seconds);
    assert_eq!((tagged.folder, tagged.stage, tagged.skipped), (text(&music), 0, None));

    let untagged = store.song_at(&text(&at(&music, "Someone/An Album/01 Untagged Song.wav"))).unwrap().unwrap();
    assert_eq!(untagged.tags.names_from, NamesFrom::Path);
    assert_eq!(
        (untagged.tags.artist.as_deref(), untagged.tags.album.as_deref(), untagged.tags.title.as_deref(), untagged.tags.track),
        (Some("Someone"), Some("An Album"), Some("Untagged Song"), Some(1))
    );
    assert!(untagged.tags.seconds.is_some_and(|seconds| (seconds - 2.).abs() < 0.1), "{:?}", untagged.tags.seconds);

    let protected = store.song_at(&text(&music.join("Purchased.m4p"))).unwrap().unwrap();
    assert_eq!((protected.skipped.as_deref(), protected.tags.title.as_deref()), (Some(PROTECTED), Some("Purchased")));
    let counts = store.counts().unwrap();
    assert_eq!((counts.songs, counts.skipped, counts.light, counts.full), (3, 1, 0, 0));
}

#[test]
fn unchanged_files_write_nothing_and_changes_are_found() {
    let dir = tempfile::tempdir().unwrap();
    let music = tree(dir.path());
    let mut store = MusicStore::open(&dir.path().join("music.db")).unwrap();
    let folders = [text(&music)];
    pass(&mut store, &folders, &[]);
    let again = pass(&mut store, &folders, &[]);
    assert_eq!(again, TagsPassed { walked: 4, ..Default::default() }, "nothing changed, nothing written");

    // Retagged: heard again from the start.
    let archangel = at(&music, "Burial/Untrue/02 Archangel.wav");
    song(&archangel, 3., Some(("Burial", "Archangel (Remix)", "Untrue")));
    let untagged = at(&music, "Someone/An Album/01 Untagged Song.wav");
    let kept = std::fs::read(&untagged).unwrap();
    let modified = std::fs::metadata(&untagged).unwrap().modified().unwrap();
    std::fs::remove_file(&untagged).unwrap();
    let passed = pass(&mut store, &folders, &[]);
    assert_eq!((passed.changed, passed.gone, passed.new), (1, 1, 0), "{passed:?}");
    assert_eq!(store.song_at(&text(&archangel)).unwrap().unwrap().tags.title.as_deref(), Some("Archangel (Remix)"));
    assert!(store.song_at(&text(&untagged)).unwrap().unwrap().gone);

    // Back as it was: known again, nothing read.
    std::fs::write(&untagged, kept).unwrap();
    std::fs::File::options().write(true).open(&untagged).unwrap().set_modified(modified).unwrap();
    let passed = pass(&mut store, &folders, &[]);
    assert_eq!((passed.back, passed.changed, passed.new, passed.gone), (1, 0, 0, 0), "{passed:?}");
    assert!(!store.song_at(&text(&untagged)).unwrap().unwrap().gone);
}

#[test]
fn a_folder_that_cant_be_read_loses_no_songs() {
    let dir = tempfile::tempdir().unwrap();
    let (first, second) = (dir.path().join("First"), dir.path().join("Second"));
    song(&at(&first, "A/B/01 One.wav"), 1., None);
    song(&at(&second, "C/D/01 Two.wav"), 1., None);
    let mut store = MusicStore::open(&dir.path().join("music.db")).unwrap();
    let folders = [text(&first), text(&second)];
    assert_eq!(pass(&mut store, &folders, &[]).new, 2);
    // A drive that isn't there now: its songs wait for it.
    std::fs::rename(&second, dir.path().join("Away")).unwrap();
    assert_eq!(pass(&mut store, &folders, &[]).gone, 0);
    // A folder the producer took away: its songs go.
    assert_eq!(pass(&mut store, &folders[..1], &[]).gone, 1);
    assert_eq!(store.counts().unwrap().songs, 1);
}

#[test]
fn duplicates_are_one_song_kept_as_the_lossless_file() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = MusicStore::open(&dir.path().join("music.db")).unwrap();
    let file = |name: &str, size: u64| Walked { path: format!("/m/{name}"), folder: "/m".into(), size, mtime: 1, protected: false };
    let tags = |artist: Option<&str>, title: &str, seconds: f64| Tags {
        artist: artist.map(str::to_string),
        title: Some(title.into()),
        seconds: Some(seconds),
        names_from: NamesFrom::Tags,
        ..Default::default()
    };
    store
        .write_songs(&[
            (file("a.mp3", 9_000_000), tags(Some("Burial"), "Archangel", 238.)),
            (file("a.flac", 30_000_000), tags(Some("burial "), "ARCHANGEL", 239.5)),
            (file("b.mp3", 8_000_000), tags(Some("Burial"), "Archangel", 300.)),
            (file("c.wav", 1_000), tags(None, "Archangel", 238.)),
        ])
        .unwrap();
    assert_eq!(store.resolve_duplicates().unwrap(), 1);
    let id = |name: &str| store.song_at(&format!("/m/{name}")).unwrap().unwrap();
    assert_eq!(id("a.mp3").duplicate_of, Some(id("a.flac").id));
    assert_eq!((id("a.flac").duplicate_of, id("b.mp3").duplicate_of, id("c.wav").duplicate_of), (None, None, None));
    let counts = store.counts().unwrap();
    assert_eq!((counts.songs, counts.duplicates), (3, 1));
    assert_eq!(store.resolve_duplicates().unwrap(), 0, "found again, nothing changes");
}

#[test]
fn the_first_usual_place_with_songs_is_offered() {
    let dir = tempfile::tempdir().unwrap();
    let (apple, music) = (at(&dir.path(), "Music/Music/Media.localized/Music"), dir.path().join("Music"));
    std::fs::create_dir_all(&apple).unwrap();
    song(&at(&music, "Artist/Album/01 One.wav"), 1., None);
    song(&at(&music, "Artist/Album/02 Two.wav"), 1., None);
    // The music app's folder is empty: the Music folder around it is offered.
    assert_eq!(found_in(vec![apple.clone(), music.clone()], &[]), Some(Found { folder: text(&music), songs: 2, capped: false }));
    song(&at(&apple, "Someone/Else/01 Three.wav"), 1., None);
    assert_eq!(found_in(vec![apple.clone(), music.clone()], &[]), Some(Found { folder: text(&apple), songs: 1, capped: false }));
    assert_eq!(found_in(vec![dir.path().join("nowhere")], &[]), None);
}

#[test]
fn the_map_has_the_songs_laid_out_and_the_styles_with_their_words() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("music.db");
    let store = MusicStore::open(&file).unwrap();
    // A song laid out in each style, one heard but not laid out yet, and one laid out but gone since.
    for (path, artist, title, cluster, at, gone) in [
        ("/m/Burial/Archangel.flac", "'Burial'", "'Archangel'", "1", "0.25, 0.75", 0),
        ("/m/Loose/Untitled 3.wav", "NULL", "NULL", "0", "1.0, 0.0", 0),
        ("/m/New/Fresh.wav", "'New'", "'Fresh'", "NULL", "NULL, NULL", 0),
        ("/m/Old/Gone.wav", "'Old'", "'Gone'", "0", "0.5, 0.5", 1),
    ] {
        let sql = format!(
            "INSERT INTO songs (path, folder, size, mtime, names_from, artist, title, genre, seconds, cluster, x, y, gone) \
             VALUES ('{path}', '/m', 1, 1, 'tags', {artist}, {title}, 'Dubstep; Garage', 240.0, {cluster}, {at}, {gone})"
        );
        store.connection().execute(&sql, []).unwrap();
    }
    let clusters = r#"{"model": "clap", "heard": 3, "centres": [], "clusters": [
        {"songs": 2, "examples": [2], "genres": ["Garage"]},
        {"songs": 1, "examples": [1], "genres": ["Dubstep"], "names": ["dark", "rainy"]}]}"#;
    store.set_meta("clusters", clusters).unwrap();
    drop(store);
    let (songs, styles) = MusicStore::open_read(&file).unwrap().unwrap().map().unwrap();
    let shown: Vec<(&str, Option<usize>, f64, f64)> = songs.iter().map(|song| (song.name.as_str(), song.cluster, song.x, song.y)).collect();
    assert_eq!(shown, [("Burial – Archangel", Some(1), 0.25, 0.75), ("Untitled 3.wav", Some(0), 1., 0.)], "laid out, and not gone");
    assert_eq!((songs[0].genre.as_deref(), songs[0].seconds), (Some("Dubstep; Garage"), Some(240.)));
    assert_eq!(styles.len(), 2);
    assert_eq!((styles[0].songs, styles[0].examples.as_slice()), (2, &[2][..]));
    assert!(styles[0].names.is_empty(), "named once the text half names it");
    assert_eq!(
        (styles[1].genres.as_slice(), styles[1].names.as_slice()),
        (&["Dubstep".to_string()][..], &["dark".to_string(), "rainy".to_string()][..])
    );
}

#[test]
fn a_song_plays_from_the_window_its_style_was_heard_in() {
    use kumi_listening::music::hear::heard_first;
    assert_eq!(heard_first(240.), (115., 10.), "ten seconds halfway through");
    assert_eq!(heard_first(6.), (0., 6.), "all of a short one");
}
