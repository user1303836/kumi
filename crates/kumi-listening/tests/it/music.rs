//! The producer's music by its tags: the usual places, the walk and its skips, the tags pass and music.db.

use kumi_common::abort::Signal;
use kumi_listening::music::{
    database,
    folders::{found_in, walk, Found, Walked},
    map::{self, Changed},
    scan::{apply_walk, next, recluster, tags_pass, Aside, Next, TagsPass, TagsPassed},
    store::{MusicStore, LIVE, PROTECTED, WONT_DECODE},
    tags::{NamesFrom, Tags},
    vectors::{file_for, VectorReader, VectorWriter, DIMS},
};
use lofty::{
    config::WriteOptions,
    tag::{Accessor, ItemKey, Tag, TagExt, TagType},
};
use std::{
    collections::{HashSet, VecDeque},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Mutex,
    },
    time::{Duration, Instant},
};

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
    tags_pass(store, &TagsPass { folders, skip, signal: &signal, readers: 2, progress: &|_| {}, hold: &|| {} }).unwrap().0
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
    let file = |name: &str, size: u64| Walked { path: format!("/m/{name}"), folder: "/m".into(), size, mtime: 1, ..Default::default() };
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
    assert_eq!(found_in(vec![apple.clone(), music.clone()], &[]), Some(Found { folder: text(&music), songs: 2, ..Default::default() }));
    song(&at(&apple, "Someone/Else/01 Three.wav"), 1., None);
    assert_eq!(found_in(vec![apple.clone(), music.clone()], &[]), Some(Found { folder: text(&apple), songs: 1, ..Default::default() }));
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

/// The files a walk of `folders` finds.
fn walked(folders: &[String]) -> Vec<Walked> {
    let mut found = vec![];
    walk(folders, &[], &mut |song| {
        found.push(song);
        true
    });
    found
}

/// A vector pointing at `at`.
fn pointing(at: usize) -> Vec<f32> {
    (0..DIMS).map(|n| if n == at { 1. } else { 0.01 }).collect()
}

/// Songs heard (at the light stage) with no files behind them, named `n` from 0.
fn heard_rows(store: &mut MusicStore, count: usize) -> Vec<i64> {
    let rows: Vec<(Walked, Tags)> = (0..count)
        .map(|n| {
            let file = Walked { path: format!("/m/{n}.mp3"), folder: "/m".into(), size: 1, mtime: n as i64, ..Default::default() };
            (file, Tags { artist: Some(format!("artist {n}")), title: Some("t".into()), names_from: NamesFrom::Tags, ..Default::default() })
        })
        .collect();
    store.write_songs(&rows).unwrap();
    store.connection().execute("UPDATE songs SET stage = 1, heard_at = 1, profile = '{}'", []).unwrap();
    (0..count).map(|n| store.song_at(&format!("/m/{n}.mp3")).unwrap().unwrap().id).collect()
}

#[test]
fn songs_under_a_folder_added_around_theirs_keep_what_was_heard() {
    let dir = tempfile::tempdir().unwrap();
    let outer = dir.path().join("Music");
    let inner = at(&outer, "Music/Media.localized/Music");
    song(&at(&inner, "Burial/Untrue/02 Archangel.wav"), 3., Some(("Burial", "Archangel", "Untrue")));
    song(&at(&inner, "Someone/An Album/01 Untagged Song.wav"), 2., None);
    let mut store = MusicStore::open(&dir.path().join("music.db")).unwrap();
    pass(&mut store, &[text(&inner)], &[]);
    store.connection().execute("UPDATE songs SET stage = 2, profile = '{}', heard_at = 7, cluster = 1, x = 0.5, y = 0.5", []).unwrap();
    // The producer adds the Music folder around it: the same files, under another of their folders.
    let passed = pass(&mut store, &[text(&inner), text(&outer)], &[]);
    assert_eq!((passed.moved, passed.changed, passed.new, passed.gone), (2, 0, 0, 0), "{passed:?}");
    for name in ["Burial/Untrue/02 Archangel.wav", "Someone/An Album/01 Untagged Song.wav"] {
        let song = store.song_at(&text(&at(&inner, name))).unwrap().unwrap();
        assert_eq!((song.folder, song.stage, song.heard_at, song.cluster), (text(&outer), 2, Some(7), Some(1)), "{name}");
    }
    let tagged = store.song_at(&text(&at(&inner, "Burial/Untrue/02 Archangel.wav"))).unwrap().unwrap();
    assert_eq!(tagged.tags.title.as_deref(), Some("Archangel"));
    // And taken away again: back under the folder inside, still as heard.
    let passed = pass(&mut store, &[text(&inner)], &[]);
    assert_eq!((passed.moved, passed.changed, passed.gone), (2, 0, 0), "{passed:?}");
    assert_eq!(store.counts().unwrap().full, 2);
}

#[cfg(any(windows, target_os = "macos"))]
#[test]
fn a_folder_typed_in_another_case_keeps_its_songs_while_its_drive_is_away() {
    let dir = tempfile::tempdir().unwrap();
    let (first, second) = (dir.path().join("First"), dir.path().join("Second"));
    song(&at(&first, "A/B/01 One.wav"), 1., None);
    song(&at(&second, "C/D/01 Two.wav"), 1., None);
    let mut store = MusicStore::open(&dir.path().join("music.db")).unwrap();
    // As the producer typed it; the disk spells it otherwise.
    let folders = [text(&first), text(&dir.path().join("SECOND"))];
    assert_eq!(pass(&mut store, &folders, &[]).new, 2);
    std::fs::rename(&second, dir.path().join("Away")).unwrap();
    assert_eq!(pass(&mut store, &folders, &[]).gone, 0, "still one of their folders, its drive away");
    assert_eq!(store.counts().unwrap().songs, 2);
}

#[test]
fn a_file_the_tags_reader_trips_on_is_named_by_its_path_and_read_once() {
    let dir = tempfile::tempdir().unwrap();
    let music = dir.path().join("Music");
    song(&at(&music, "Artist/Album/01 Fine.wav"), 1., Some(("Artist", "Fine", "Album")));
    song(&at(&music, "Artist/Album/02 Broken.wav"), 1., None);
    let mut store = MusicStore::open(&dir.path().join("music.db")).unwrap();
    let read_now: &'static Mutex<Vec<PathBuf>> = Box::leak(Box::default());
    let read = |file: &Path, folder: &Path| {
        read_now.lock().unwrap().push(file.to_path_buf());
        assert!(!file.ends_with("02 Broken.wav"), "a malformed tag the reader can't take");
        kumi_listening::music::tags::read(file, folder)
    };
    let (folders, signal) = ([text(&music)], Signal::new());
    let tags = TagsPass { folders: &folders, skip: &[], signal: &signal, readers: 2, progress: &|_| {}, hold: &|| {} };
    let passed = apply_walk(&mut store, &folders, &folders, &[], walked(&folders), &read, &tags).unwrap();
    assert_eq!(passed.new, 2, "the pass goes on");
    let broken = store.song_at(&text(&at(&music, "Artist/Album/02 Broken.wav"))).unwrap().unwrap();
    assert_eq!(
        (broken.tags.title.as_deref(), broken.tags.artist.as_deref(), broken.tags.names_from),
        (Some("Broken"), Some("Artist"), NamesFrom::Path)
    );
    // Kept: the next pass reads neither again.
    read_now.lock().unwrap().clear();
    apply_walk(&mut store, &folders, &folders, &[], walked(&folders), &read, &tags).unwrap();
    assert!(read_now.lock().unwrap().is_empty(), "{:?}", read_now.lock().unwrap());
}

#[cfg(unix)]
#[test]
fn songs_behind_a_link_to_a_drive_that_isnt_there_are_not_marked_gone() {
    let dir = tempfile::tempdir().unwrap();
    let (music, drive) = (dir.path().join("Music"), dir.path().join("Drive"));
    song(&at(&music, "A/B/01 Here.wav"), 1., None);
    song(&at(&drive, "C/D/01 There.wav"), 1., None);
    std::os::unix::fs::symlink(&drive, music.join("External")).unwrap();
    let mut store = MusicStore::open(&dir.path().join("music.db")).unwrap();
    let folders = [text(&music)];
    assert_eq!(pass(&mut store, &folders, &[]).new, 2);
    // Unplugged: the link dangles, and what's behind it has gone nowhere.
    std::fs::rename(&drive, dir.path().join("Unplugged")).unwrap();
    assert_eq!(pass(&mut store, &folders, &[]).gone, 0);
    // The link taken away too: gone.
    std::fs::remove_file(music.join("External")).unwrap();
    assert_eq!(pass(&mut store, &folders, &[]).gone, 1);
}

#[test]
fn a_changed_file_is_heard_afresh_with_its_vectors_and_place_forgotten() {
    let dir = tempfile::tempdir().unwrap();
    let music = dir.path().join("Music");
    let (one, two) = (at(&music, "A/B/01 One.wav"), at(&music, "A/B/02 Two.wav"));
    song(&one, 1., None);
    song(&two, 1., None);
    let kumi = dir.path().join("kumi");
    let mut store = MusicStore::open(&database(&kumi)).unwrap();
    pass(&mut store, &[text(&music)], &[]);
    let id = |store: &MusicStore, path: &Path| store.song_at(&text(path)).unwrap().unwrap().id;
    let (first, second) = (id(&store, &one), id(&store, &two));
    let mut writer = VectorWriter::open(&kumi, "model").unwrap();
    writer.put(&store, first, &pointing(3)).unwrap();
    writer.put(&store, second, &pointing(4)).unwrap();
    writer.settle(&store).unwrap();
    // Another model's, still waiting.
    VectorWriter::open(&kumi, "other").unwrap().put(&store, first, &pointing(5)).unwrap();
    store.connection().execute("UPDATE songs SET stage = 2, profile = '{}', x = 0.1, y = 0.2", []).unwrap();
    // The first file is another recording now.
    song(&one, 2., None);
    assert_eq!(pass(&mut store, &[text(&music)], &[]).changed, 1);
    let reader = VectorReader::open(&kumi, &store, "model").unwrap();
    assert!(!reader.has(first) && reader.has(second), "its row emptied, the other's kept");
    assert!(!VectorReader::open(&kumi, &store, "other").unwrap().has(first), "and what it waited with");
    let place = |id: i64| -> (Option<f64>, i64) {
        store.connection().query_row("SELECT x, stage FROM songs WHERE id = ?1", [id], |row| Ok((row.get(0)?, row.get(1)?))).unwrap()
    };
    assert_eq!((place(first), place(second)), ((None, 0), (Some(0.1), 2)));
}

#[test]
fn a_music_db_made_afresh_takes_no_vectors_an_older_one_left() {
    let dir = tempfile::tempdir().unwrap();
    let kumi = dir.path().join("kumi");
    let mut store = MusicStore::open(&database(&kumi)).unwrap();
    let ids = heard_rows(&mut store, 2);
    let mut writer = VectorWriter::open(&kumi, "model").unwrap();
    writer.put(&store, ids[0], &pointing(1)).unwrap();
    writer.settle(&store).unwrap();
    drop((writer, store));
    assert!(file_for(&kumi, "model").is_file());
    // music.db deleted on its own: a new one numbers its songs from 1 again.
    for name in ["music.db", "music.db-wal", "music.db-shm"] {
        let _ = std::fs::remove_file(kumi.join(name));
    }
    let store = MusicStore::open(&database(&kumi)).unwrap();
    assert!(!file_for(&kumi, "model").exists(), "the old songs' rows would stand for new ones");
    // An existing one keeps its files.
    let mut writer = VectorWriter::open(&kumi, "model").unwrap();
    writer.put(&store, 1, &pointing(1)).unwrap();
    writer.settle(&store).unwrap();
    drop((writer, store));
    MusicStore::open(&database(&kumi)).unwrap();
    assert!(file_for(&kumi, "model").is_file());
}

#[test]
fn vectors_left_waiting_beside_the_file_go_into_it() {
    let dir = tempfile::tempdir().unwrap();
    let kumi = dir.path().join("kumi");
    let mut store = MusicStore::open(&database(&kumi)).unwrap();
    let ids = heard_rows(&mut store, 2);
    let mut writer = VectorWriter::open(&kumi, "model").unwrap();
    writer.put(&store, ids[0], &pointing(1)).unwrap();
    writer.put(&store, ids[1], &pointing(2)).unwrap();
    writer.settle(&store).unwrap();
    // A scan that ended between making the file and clearing what waited: an older vector left beside the file.
    let bytes: Vec<u8> = pointing(9).iter().flat_map(|value| value.to_le_bytes()).collect();
    store
        .connection()
        .execute("INSERT INTO vectors_pending (model, id, vector) VALUES ('model', ?1, ?2)", kumi_store::params![ids[1], bytes])
        .unwrap();
    let mut writer = VectorWriter::open(&kumi, "model").unwrap();
    writer.settle_left(&store).unwrap();
    let waiting: i64 = store.connection().query_row("SELECT count(*) FROM vectors_pending", [], |row| row.get(0)).unwrap();
    assert_eq!(waiting, 0);
    let reader = VectorReader::open(&kumi, &store, "model").unwrap();
    let vector = reader.get(ids[1]).unwrap();
    assert!(vector[9] > 0.5, "the one that waited is the row now");
}

#[test]
fn a_style_model_of_another_size_trips_neither_the_clusters_nor_the_map() {
    let dir = tempfile::tempdir().unwrap();
    let kumi = dir.path().join("kumi");
    let mut store = MusicStore::open(&database(&kumi)).unwrap();
    let ids = heard_rows(&mut store, 12);
    // A slot's model of 256 numbers (still waiting, few) and one of 1,024 (in its file).
    let (mut short, mut long) = (VectorWriter::open(&kumi, "short").unwrap(), VectorWriter::open(&kumi, "long").unwrap());
    for (n, id) in ids.iter().enumerate() {
        short.put(&store, *id, &(0..256).map(|at| if at == n % 3 { 1. } else { 0.01 }).collect::<Vec<f32>>()).unwrap();
        long.put(&store, *id, &(0..1_024).map(|at| if at == n % 3 { 1. } else { 0.01 }).collect::<Vec<f32>>()).unwrap();
    }
    long.settle(&store).unwrap();
    for model in ["short", "long"] {
        let reader = VectorReader::open(&kumi, &store, model).unwrap();
        assert!(ids.iter().all(|id| reader.get(*id).is_some_and(|vector| vector.len() == DIMS)), "{model}");
        drop(reader);
        assert_eq!(recluster(&mut store, &kumi, model, None).unwrap(), 12, "{model}");
        let how = kumi_layout::Layout { threads: 1, ..Default::default() };
        assert_eq!(map::lay_out(&mut store, &kumi, model, &how).unwrap(), 12, "{model}");
    }
}

#[test]
fn the_map_and_clusters_are_due_when_their_songs_changed_not_only_their_count() {
    let dir = tempfile::tempdir().unwrap();
    let kumi = dir.path().join("kumi");
    let mut store = MusicStore::open(&database(&kumi)).unwrap();
    let ids = heard_rows(&mut store, 12);
    let mut writer = VectorWriter::open(&kumi, "model").unwrap();
    for (n, id) in ids.iter().enumerate() {
        writer.put(&store, *id, &pointing(n % 3)).unwrap();
    }
    writer.settle(&store).unwrap();
    recluster(&mut store, &kumi, "model", None).unwrap();
    map::lay_out(&mut store, &kumi, "model", &kumi_layout::Layout { threads: 1, ..Default::default() }).unwrap();
    let changed = |store: &MusicStore| map::changed(store, &VectorReader::open(&kumi, store, "model").unwrap()).unwrap();
    assert_eq!(changed(&store), Changed::default());
    // One song gone and another heard in its place (a file moved): as many songs, but others.
    store.set_gone(&[ids[0]], true).unwrap();
    let file = Walked { path: "/m/moved.mp3".into(), folder: "/m".into(), size: 1, mtime: 99, ..Default::default() };
    store.write_songs(&[(file, Tags { title: Some("moved".into()), ..Default::default() })]).unwrap();
    let moved = store.song_at("/m/moved.mp3").unwrap().unwrap().id;
    store.heard(moved, kumi_listening::music::store::Stage::Light, "{}", 2).unwrap();
    writer.put(&store, moved, &pointing(1)).unwrap();
    assert_eq!(changed(&store), Changed { clusters: true, places: true });
    assert!(!map::due(Some(12), 12, true), "the counts alone see nothing");
    recluster(&mut store, &kumi, "model", None).unwrap();
    map::lay_out(&mut store, &kumi, "model", &kumi_layout::Layout { threads: 1, ..Default::default() }).unwrap();
    assert_eq!(changed(&store), Changed::default());
}

#[test]
fn a_lossless_copy_that_wont_decode_hands_over_to_one_that_plays() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = MusicStore::open(&dir.path().join("music.db")).unwrap();
    let file = |name: &str, size: u64| Walked { path: format!("/m/{name}"), folder: "/m".into(), size, mtime: 1, ..Default::default() };
    let tags = Tags {
        artist: Some("Burial".into()),
        title: Some("Ghost Hardware".into()),
        seconds: Some(290.),
        names_from: NamesFrom::Tags,
        ..Default::default()
    };
    store.write_songs(&[(file("a.mp3", 9_000_000), tags.clone()), (file("a.flac", 30_000_000), tags)]).unwrap();
    store.resolve_duplicates().unwrap();
    let song = |store: &MusicStore, name: &str| store.song_at(&format!("/m/{name}")).unwrap().unwrap();
    let (mp3, flac) = (song(&store, "a.mp3"), song(&store, "a.flac"));
    assert_eq!(mp3.duplicate_of, Some(flac.id), "the lossless one is kept");
    store.skip(flac.id, WONT_DECODE, 1).unwrap();
    store.resolve_duplicates().unwrap();
    assert_eq!((song(&store, "a.mp3").duplicate_of, song(&store, "a.flac").duplicate_of), (None, Some(mp3.id)), "the one that plays");
    assert_eq!(store.counts().unwrap().songs, 1);
}

#[test]
fn an_up_to_date_music_db_opens_while_another_holds_its_write_lock() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("music.db");
    drop(MusicStore::open(&path).unwrap());
    let holder = kumi_store::Connection::open(&path).unwrap();
    holder.execute_batch("BEGIN IMMEDIATE").unwrap();
    let started = Instant::now();
    assert!(MusicStore::open(&path).is_ok());
    assert!(started.elapsed() < Duration::from_secs(1), "{:?}", started.elapsed());
    holder.execute_batch("ROLLBACK").unwrap();
}

#[test]
fn clusters_and_places_write_only_the_songs_whose_own_changed() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = MusicStore::open(&dir.path().join("music.db")).unwrap();
    let ids = heard_rows(&mut store, 3);
    store
        .connection()
        .execute_batch(
            "CREATE TEMP TABLE writes (n INTEGER); INSERT INTO writes VALUES (0);
             CREATE TEMP TRIGGER counted AFTER UPDATE ON songs BEGIN UPDATE writes SET n = n + 1; END;",
        )
        .unwrap();
    let writes = |store: &MusicStore| -> i64 { store.connection().query_row("SELECT n FROM writes", [], |row| row.get(0)).unwrap() };
    store.set_clusters(&[(ids[0], 0), (ids[1], 1), (ids[2], 1)]).unwrap();
    assert_eq!(writes(&store), 3);
    store.set_clusters(&[(ids[0], 0), (ids[1], 1), (ids[2], 1)]).unwrap();
    assert_eq!(writes(&store), 3, "nothing changed, nothing written");
    store.set_clusters(&[(ids[0], 0), (ids[1], 0)]).unwrap();
    assert_eq!(writes(&store), 5, "one moved, one gone");
    let cluster = |id: i64| -> Option<i64> {
        store.connection().query_row("SELECT cluster FROM songs WHERE id = ?1", [id], |row| row.get(0)).unwrap()
    };
    assert_eq!((cluster(ids[0]), cluster(ids[1]), cluster(ids[2])), (Some(0), Some(0), None));
    store.set_places(&[(ids[0], [0.1, 0.2]), (ids[1], [0.3, 0.4])]).unwrap();
    store.set_places(&[(ids[0], [0.1, 0.2]), (ids[1], [0.3, 0.5])]).unwrap();
    assert_eq!(writes(&store), 5 + 2 + 1);
}

#[test]
fn a_song_only_in_the_cloud_is_named_by_its_path_and_heard_once_its_on_this_computer() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = MusicStore::open(&dir.path().join("music.db")).unwrap();
    let folder = "/m".to_string();
    let file = |name: &str, cloud: bool| Walked {
        path: format!("/m/Artist/Album/{name}"),
        folder: folder.clone(),
        size: 1,
        mtime: 1,
        cloud,
        ..Default::default()
    };
    let read_now: &'static Mutex<Vec<PathBuf>> = Box::leak(Box::default());
    let read = |file: &Path, folder: &Path| {
        read_now.lock().unwrap().push(file.to_path_buf());
        kumi_listening::music::tags::from_path(file, folder)
    };
    let signal = Signal::new();
    let folders = [folder.clone()];
    let tags = TagsPass { folders: &folders, skip: &[], signal: &signal, readers: 1, progress: &|_| {}, hold: &|| {} };
    let walk = vec![file("01 Here.mp3", false), file("02 Away.mp3", true)];
    apply_walk(&mut store, &folders, &folders, &[], walk, &read, &tags).unwrap();
    assert_eq!(*read_now.lock().unwrap(), [PathBuf::from("/m/Artist/Album/01 Here.mp3")], "reading it would download it");
    let away = store.song_at("/m/Artist/Album/02 Away.mp3").unwrap().unwrap();
    assert_eq!((away.cloud, away.tags.artist.as_deref(), away.tags.title.as_deref()), (true, Some("Artist"), Some("Away")));
    assert_eq!(store.counts().unwrap().cloud, 1);
    let pick = |store: &MusicStore| -> Option<String> {
        let (busy, mut missing, mut at) = (HashSet::new(), VecDeque::new(), 0);
        let aside = Aside { songs: &busy, folders: &[], since: i64::MAX };
        next(store, &aside, &mut missing, &[], &mut at).unwrap().map(|next| match next {
            Next::Hear(song, _) | Next::Restyle(song, _) => song.path,
        })
    };
    assert_eq!(pick(&store).as_deref(), Some("/m/Artist/Album/01 Here.mp3"));
    store.connection().execute("UPDATE songs SET stage = 2 WHERE path LIKE '%Here.mp3'", []).unwrap();
    assert_eq!(pick(&store), None, "never heard while it's in the cloud");
    // On this computer now: its tags read at last, and heard.
    read_now.lock().unwrap().clear();
    let walk = vec![file("01 Here.mp3", false), file("02 Away.mp3", false)];
    assert_eq!(apply_walk(&mut store, &folders, &folders, &[], walk, &read, &tags).unwrap().changed, 1);
    assert_eq!(*read_now.lock().unwrap(), [PathBuf::from("/m/Artist/Album/02 Away.mp3")]);
    assert_eq!(pick(&store).as_deref(), Some("/m/Artist/Album/02 Away.mp3"));
}

#[test]
fn the_tags_pass_waits_between_files_while_its_held() {
    let dir = tempfile::tempdir().unwrap();
    let music = dir.path().join("Music");
    for n in 1..=3 {
        song(&at(&music, &format!("A/B/0{n} Song {n}.wav")), 1., None);
    }
    let mut store = MusicStore::open(&dir.path().join("music.db")).unwrap();
    let (held, done) = (AtomicBool::new(true), AtomicBool::new(false));
    std::thread::scope(|scope| {
        scope.spawn(|| {
            let hold = || {
                while held.load(Ordering::Relaxed) {
                    std::thread::sleep(Duration::from_millis(10));
                }
            };
            let (folders, signal) = ([text(&music)], Signal::new());
            tags_pass(&mut store, &TagsPass { folders: &folders, skip: &[], signal: &signal, readers: 2, progress: &|_| {}, hold: &hold })
                .unwrap();
            done.store(true, Ordering::Relaxed);
        });
        std::thread::sleep(Duration::from_millis(300));
        assert!(!done.load(Ordering::Relaxed), "held: Kumi listens, so the walk waits");
        held.store(false, Ordering::Relaxed);
    });
    assert!(done.load(Ordering::Relaxed));
    assert_eq!(store.counts().unwrap().songs, 3);
}

#[test]
fn the_songs_taste_used_are_found_by_their_own_index() {
    let dir = tempfile::tempdir().unwrap();
    let store = MusicStore::open(&dir.path().join("music.db")).unwrap();
    let used = kumi_listening::music::scan::USED;
    let sql =
        format!("EXPLAIN QUERY PLAN SELECT id FROM songs WHERE {LIVE} AND (({used}) AND cloud = 0) ORDER BY used_at DESC, id LIMIT 1");
    let mut statement = store.connection().prepare(&sql).unwrap();
    let plan: Vec<String> = statement.query_map([], |row| row.get(3)).unwrap().map(Result::unwrap).collect();
    assert!(plan.iter().any(|step| step.contains("songs_used")), "{plan:?}");
}

#[test]
fn a_map_told_to_end_places_nothing_and_one_held_waits() {
    let dir = tempfile::tempdir().unwrap();
    let kumi = dir.path().join("kumi");
    let mut store = MusicStore::open(&database(&kumi)).unwrap();
    let ids = heard_rows(&mut store, 12);
    let mut writer = VectorWriter::open(&kumi, "model").unwrap();
    for (n, id) in ids.iter().enumerate() {
        writer.put(&store, *id, &pointing(n % 3)).unwrap();
    }
    writer.settle(&store).unwrap();
    let how = kumi_layout::Layout { threads: 1, ..Default::default() };
    // Stopped: nothing placed, no map made.
    assert_eq!(map::lay_out_until(&mut store, &kumi, "model", &how, &|| false).unwrap(), None);
    let placed: i64 = store.connection().query_row("SELECT count(*) FROM songs WHERE x IS NOT NULL", [], |row| row.get(0)).unwrap();
    assert_eq!((placed, map::made(&store).unwrap()), (0, None));
    // Held (Kumi listens): it waits, then goes on.
    let (held, done) = (AtomicBool::new(true), AtomicBool::new(false));
    std::thread::scope(|scope| {
        scope.spawn(|| {
            let go_on = || {
                while held.load(Ordering::Relaxed) {
                    std::thread::sleep(Duration::from_millis(10));
                }
                true
            };
            assert_eq!(map::lay_out_until(&mut store, &kumi, "model", &how, &go_on).unwrap(), Some(12));
            done.store(true, Ordering::Relaxed);
        });
        std::thread::sleep(Duration::from_millis(300));
        assert!(!done.load(Ordering::Relaxed), "held: no map yet");
        held.store(false, Ordering::Relaxed);
    });
    assert!(done.load(Ordering::Relaxed));
    assert_eq!(map::made(&store).unwrap().map(|made| made.songs), Some(12));
}
