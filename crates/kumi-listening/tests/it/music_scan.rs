//! The listening scan: the order songs are heard in, hearing short synthetic songs (tones and noise written here) with
//! a style model of the test's own (vectors injected, so no model is needed), stopping and going on, and a new style
//! model's own vector file. One test with the real model runs only when KUMI_TEST_MODELS names it.

use async_trait::async_trait;
use kumi_listening::{
    checklist::Profile,
    embed::Say,
    music::{
        database,
        folders::Walked,
        hear::Style,
        scan::{next, spread_sample, Aside, Next},
        store::{MusicStore, Stage, TOO_QUIET, TOO_SHORT},
        tags::{NamesFrom, Tags},
        vectors::{file_for, quantize, scale_of, VectorReader, DIMS, SCALE_FROM},
        worker::{run, Control, ScanOptions, ScanPhase, ScanProgress, ScanReply},
    },
};
use std::{
    cell::{Cell, RefCell},
    collections::{HashMap, HashSet, VecDeque},
    path::Path,
    rc::Rc,
};

const RATE: u32 = 8_000;

/// A mono song: a tone at `hz` (with a little noise), `seconds` long, at `level` (peak).
fn song(path: &Path, seconds: f64, hz: f64, level: f32) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let mut noise = 12345u32;
    let samples: Vec<f32> = (0..(seconds * RATE as f64) as usize)
        .map(|n| {
            noise = noise.wrapping_mul(1_103_515_245).wrapping_add(12_345);
            let tone = (2. * std::f64::consts::PI * hz * n as f64 / RATE as f64).sin() as f32;
            level * (0.8 * tone + 0.2 * ((noise >> 16) as f32 / 32_768. - 1.))
        })
        .collect();
    std::fs::write(path, kumi_media::audio::decode::float_wav(&[samples], RATE)).unwrap();
}

/// A style model of the test's own: each song's vector points the way its artist's folder names, so songs in one
/// folder sound alike. It counts what it heard, and stops the scan after `stop_after` songs when told to.
struct Fake {
    model: String,
    heard: RefCell<HashMap<String, usize>>,
    stop_after: Option<(usize, Control)>,
    calls: Cell<usize>,
}

impl Fake {
    fn new(model: &str) -> Rc<Fake> {
        Rc::new(Fake { model: model.into(), heard: RefCell::default(), stop_after: None, calls: Cell::new(0) })
    }
}

#[async_trait(?Send)]
impl Style for Fake {
    fn model(&self) -> &str {
        &self.model
    }
    async fn embed(
        &self,
        file: &Path,
        starts: &[f64],
        _: Option<f64>,
        _: Say<'_>,
        _: &kumi_common::abort::Signal,
    ) -> Result<(Vec<f32>, String), String> {
        assert!(!starts.is_empty());
        // (Its artist's folder: Music/<artist>/<album>/<song>.)
        let folder = file.parent().and_then(Path::parent).and_then(|folder| folder.file_name()).unwrap().to_string_lossy().into_owned();
        *self.heard.borrow_mut().entry(file.to_string_lossy().into_owned()).or_default() += 1;
        self.calls.set(self.calls.get() + 1);
        if let Some((after, control)) = &self.stop_after {
            if self.calls.get() >= *after {
                control.stop();
            }
        }
        let mut vector = vec![0.01f32; DIMS];
        vector[folder.bytes().map(usize::from).sum::<usize>() % DIMS] = 1.;
        Ok((vector, self.model.clone()))
    }
}

/// A library of songs: two low tones, two high ones, one too short and one too quiet.
fn library(root: &Path) -> String {
    let music = root.join("Music");
    for n in 0..2 {
        song(&std::path::Path::new(&at(&music.to_string_lossy(), &format!("Low/Album/0{} Low {n}.wav", n + 1))), 31. + n as f64, 110., 0.5);
        song(
            &std::path::Path::new(&at(&music.to_string_lossy(), &format!("High/Album/0{} High {n}.wav", n + 1))),
            31. + n as f64,
            2_000.,
            0.5,
        );
    }
    song(Path::new(&at(&music.to_string_lossy(), "Short/Album/01 Short.wav")), 5., 440., 0.5);
    song(Path::new(&at(&music.to_string_lossy(), "Quiet/Album/01 Quiet.wav")), 31., 440., 0.000_02);
    music.to_string_lossy().into_owned()
}

async fn scan(dir: &Path, music: &str, style: Rc<dyn Style>, control: &Control) -> ScanProgress {
    let options = ScanOptions { dir: dir.to_string_lossy().into_owned(), folders: vec![music.into()], ..Default::default() };
    run(&options, Default::default(), control, Some(style), &|_| {}).await.unwrap()
}

/// A song's path under the music folder, a component at a time: the system's own separators, as the walk keeps them.
fn at(music: &str, relative: &str) -> String {
    relative.split('/').fold(std::path::PathBuf::from(music), |path, part| path.join(part)).to_string_lossy().into_owned()
}

fn store(dir: &Path) -> MusicStore {
    MusicStore::open(&database(dir)).unwrap()
}

#[test]
fn the_spread_sample_takes_each_artist_in_turn_newest_first() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = MusicStore::open(&database(dir.path())).unwrap();
    let mut rows = vec![];
    for (n, artist) in ["A", "A", "A", "A", "A", "B", "B", "B", "C"].into_iter().enumerate() {
        let file = Walked { path: format!("/m/{artist}/{n}.mp3"), folder: "/m".into(), size: 1, mtime: n as i64, ..Default::default() };
        let tags = Tags { artist: Some(artist.into()), title: Some(format!("{n}")), names_from: NamesFrom::Tags, ..Default::default() };
        rows.push((file, tags));
    }
    store.write_songs(&rows).unwrap();
    let names = |ids: Vec<i64>| -> Vec<String> {
        ids.into_iter().map(|id| store.song(id).unwrap().unwrap().path.trim_start_matches("/m/").to_string()).collect()
    };
    assert_eq!(names(spread_sample(&store, 4).unwrap()), ["C/8.mp3", "B/7.mp3", "A/4.mp3", "B/6.mp3"]);
    assert_eq!(spread_sample(&store, 100).unwrap().len(), 9);
}

#[test]
fn songs_are_heard_used_first_then_the_spread_sample_then_lightly_newest_first() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = MusicStore::open(&database(dir.path())).unwrap();
    let rows: Vec<(Walked, Tags)> = (0..6)
        .map(|n| {
            let file = Walked { path: format!("/m/{n}.mp3"), folder: "/m".into(), size: 1, mtime: n, ..Default::default() };
            (file, Tags { artist: Some(format!("artist {n}")), title: Some("t".into()), names_from: NamesFrom::Tags, ..Default::default() })
        })
        .collect();
    store.write_songs(&rows).unwrap();
    let id = |n: i64| store.song_at(&format!("/m/{n}.mp3")).unwrap().unwrap().id;
    store.connection().execute("UPDATE songs SET used_at = 5 WHERE id = ?1", [id(2)]).unwrap();
    store.connection().execute("UPDATE songs SET stage = 1 WHERE id = ?1", [id(5)]).unwrap();
    let spread = vec![id(0), id(1)];
    let (mut missing, mut at) = (VecDeque::new(), 0);
    let mut busy = HashSet::new();
    let mut order = vec![];
    while let Some(next) = next(&store, &Aside { songs: &busy, folders: &[], since: i64::MAX }, &mut missing, &spread, &mut at).unwrap() {
        let (song, stage) = match &next {
            Next::Hear(song, stage) => (song.clone(), *stage),
            Next::Restyle(..) => unreachable!(),
        };
        let n: i64 = song.path.trim_start_matches("/m/").trim_end_matches(".mp3").parse().unwrap();
        order.push((n, stage));
        busy.insert(song.id);
        // Heard: lightly heard songs come round again in full.
        store.connection().execute("UPDATE songs SET stage = ?2 WHERE id = ?1", [song.id, stage as i64]).unwrap();
        if stage == Stage::Light {
            busy.remove(&song.id);
        }
    }
    assert_eq!(
        order,
        [
            (2, Stage::Full),
            (0, Stage::Full),
            (1, Stage::Full),
            (4, Stage::Light),
            (3, Stage::Light),
            (5, Stage::Full),
            (4, Stage::Full),
            (3, Stage::Full)
        ]
    );
}

#[tokio::test(flavor = "current_thread")]
async fn the_scan_hears_measures_and_embeds_songs_and_says_why_it_skips_one() {
    let dir = tempfile::tempdir().unwrap();
    let music = library(dir.path());
    let style = Fake::new("test-model");
    let progress = scan(&dir.path().join("kumi/music"), &music, style.clone(), &Control::new(false)).await;
    assert_eq!(progress.phase, ScanPhase::Done);
    assert_eq!((progress.songs, progress.full, progress.skipped, progress.heard), (6, 4, 2, 6), "{progress:?}");
    let dir = dir.path().join("kumi/music");
    let store = store(&dir);
    let why = |name: &str| store.song_at(&at(&music, name)).unwrap().unwrap().skipped;
    assert_eq!(why("Short/Album/01 Short.wav").as_deref(), Some(TOO_SHORT));
    assert_eq!(why("Quiet/Album/01 Quiet.wav").as_deref(), Some(TOO_QUIET));
    let low = store.song_at(&at(&music, "Low/Album/01 Low 0.wav")).unwrap().unwrap();
    assert_eq!((low.stage, low.cluster), (Stage::Full as i64, Some(0)));
    let profile: Profile = serde_json::from_str(low.profile.as_deref().unwrap()).unwrap();
    assert!(profile.integrated.is_some() && profile.vibe.is_none(), "measured, its vector kept apart: {profile:?}");
    // Four vectors, fewer than a file's scale needs: the scale was set when the scan had nothing left to hear.
    assert!(file_for(&dir, "test-model").is_file());
    let reader = VectorReader::open(&dir, &store, "test-model").unwrap();
    let high = store.song_at(&at(&music, "High/Album/01 High 0.wav")).unwrap().unwrap();
    assert!(reader.distance(low.id, &reader.get(low.id).unwrap()).unwrap() < 1e-3);
    assert!(reader.distance(low.id, &reader.get(high.id).unwrap()).unwrap() > 0.5, "the two folders sound apart");
    assert_eq!(style.heard.borrow().len(), 4, "each song heard once: {:?}", style.heard.borrow());
    assert!(store.meta("clusters").unwrap().is_some_and(|clusters| clusters.contains("\"heard\":4")));
    // On the map: the four songs heard, made at the scan's end; the songs skipped aren't.
    let place = |id: i64| {
        store
            .connection()
            .query_row("SELECT x, y FROM songs WHERE id = ?1", [id], |row| {
                Ok((row.get::<_, Option<f64>>(0)?, row.get::<_, Option<f64>>(1)?))
            })
            .unwrap()
    };
    assert!([low.id, high.id]
        .iter()
        .all(|id| matches!(place(*id), (Some(x), Some(y)) if (0. ..=1.).contains(&x) && (0. ..=1.).contains(&y))));
    assert_eq!(place(store.song_at(&at(&music, "Short/Album/01 Short.wav")).unwrap().unwrap().id), (None, None));
    let made = kumi_listening::music::map::made(&store).unwrap().unwrap();
    assert_eq!((made.model.as_str(), made.songs), ("test-model", 4));
}

#[tokio::test(flavor = "current_thread")]
async fn a_stopped_scan_goes_on_where_it_stopped_hearing_no_song_twice() {
    let dir = tempfile::tempdir().unwrap();
    let music = library(dir.path());
    let dir = dir.path().join("music");
    let control = Control::new(false);
    let style = Rc::new(Fake {
        model: "test-model".into(),
        heard: RefCell::default(),
        stop_after: Some((2, control.clone())),
        calls: Cell::new(0),
    });
    let first = scan(&dir, &music, style.clone(), &control).await;
    assert_eq!(first.phase, ScanPhase::Hearing, "stopped: {first:?}");
    let heard_before: HashMap<i64, i64> = {
        let store = store(&dir);
        let mut statement = store.connection().prepare("SELECT id, heard_at FROM songs WHERE heard_at IS NOT NULL").unwrap();
        statement.query_map([], |row| Ok((row.get(0)?, row.get(1)?))).unwrap().map(Result::unwrap).collect()
    };
    assert!(!heard_before.is_empty() && heard_before.len() < 6, "{heard_before:?}");
    let again = Fake::new("test-model");
    let second = scan(&dir, &music, again.clone(), &Control::new(false)).await;
    assert_eq!((second.phase, second.full, second.skipped), (ScanPhase::Done, 4, 2));
    let store = store(&dir);
    for (id, at) in heard_before {
        assert_eq!(store.song(id).unwrap().unwrap().heard_at, Some(at), "song {id} was heard once");
    }
    assert_eq!(second.heard + first.heard, 6, "{first:?} {second:?}");
}

#[tokio::test(flavor = "current_thread")]
async fn another_style_model_hears_the_songs_into_its_own_file_and_keeps_their_profiles() {
    let dir = tempfile::tempdir().unwrap();
    let music = library(dir.path());
    let dir = dir.path().join("music");
    scan(&dir, &music, Fake::new("model-a"), &Control::new(false)).await;
    let profiles = |store: &MusicStore| -> Vec<Option<String>> {
        let mut statement = store.connection().prepare("SELECT profile FROM songs ORDER BY id").unwrap();
        statement.query_map([], |row| row.get(0)).unwrap().map(Result::unwrap).collect()
    };
    let before = profiles(&store(&dir));
    let other = Fake::new("model-b");
    let progress = scan(&dir, &music, other.clone(), &Control::new(false)).await;
    assert_eq!(progress.phase, ScanPhase::Done);
    assert_eq!(other.heard.borrow().len(), 4, "each heard song heard again by the new model");
    let store = store(&dir);
    assert_eq!(profiles(&store), before, "the profiles stay");
    assert!(file_for(&dir, "model-a").is_file() && file_for(&dir, "model-b").is_file());
    let reader = VectorReader::open(&dir, &store, "model-b").unwrap();
    assert_eq!(store.heard_ids().unwrap().into_iter().filter(|id| reader.has(*id)).count(), 4);
}

#[test]
fn eight_bit_vectors_find_the_same_nearest_songs() {
    // Unit vectors in groups, as songs in a few styles are.
    let mut random = kumi_listening::cmaes::seeded(11);
    let mut gaussian = || (-2. * random().max(1e-12).ln()).sqrt() * (2. * std::f64::consts::PI * random()).cos();
    let centres: Vec<Vec<f64>> = (0..40).map(|_| (0..DIMS).map(|_| gaussian()).collect()).collect();
    let unit = |vector: Vec<f64>| -> Vec<f32> {
        let length = vector.iter().map(|value| value * value).sum::<f64>().sqrt();
        vector.iter().map(|value| (value / length) as f32).collect()
    };
    let songs: Vec<Vec<f32>> = (0..3_000).map(|n| unit(centres[n % 40].iter().map(|value| value + 1.5 * gaussian()).collect())).collect();
    let scale = scale_of(&songs[..SCALE_FROM]);
    let rows: Vec<Vec<i8>> = songs.iter().map(|song| quantize(song, scale)).collect();
    let nearest = |query: usize, alike: &dyn Fn(usize) -> f64| -> Vec<usize> {
        let mut all: Vec<(f64, usize)> = (0..songs.len()).filter(|other| *other != query).map(|other| (alike(other), other)).collect();
        all.sort_by(|a, b| b.0.total_cmp(&a.0));
        all.into_iter().take(10).map(|(_, other)| other).collect()
    };
    let mut agreed = 0;
    for query in (0..songs.len()).step_by(60) {
        let exact = nearest(query, &|other| songs[query].iter().zip(&songs[other]).map(|(a, b)| (*a * *b) as f64).sum());
        let eight = nearest(query, &|other| {
            let (row, mut dot, mut length) = (&rows[other], 0f64, 0f64);
            for (value, wanted) in row.iter().zip(&songs[query]) {
                dot += *value as f64 * *wanted as f64;
                length += (*value as f64).powi(2);
            }
            dot / length.sqrt()
        });
        agreed += exact.iter().filter(|song| eight.contains(song)).count();
    }
    let queries = songs.len().div_ceil(60);
    assert!(agreed as f64 / queries as f64 >= 9., "{agreed} of {} agree", queries * 10);
}

#[tokio::test(flavor = "current_thread")]
async fn the_real_style_model_hears_the_scans_songs() {
    let Some(folder) = std::env::var("KUMI_TEST_MODELS").ok().filter(|folder| !folder.is_empty()) else {
        eprintln!("skipped: set KUMI_TEST_MODELS to a folder named models holding onnxruntime-1.23.2/ and clap-music-audio.onnx to run it");
        return;
    };
    assert!(Path::new(&folder).ends_with("models"), "KUMI_TEST_MODELS has to be a folder named models: {folder}");
    std::env::set_var("KUMI_HOME", Path::new(&folder).parent().unwrap());
    let dir = tempfile::tempdir().unwrap();
    let music = library(dir.path());
    let options =
        ScanOptions { dir: dir.path().join("music").to_string_lossy().into_owned(), folders: vec![music.clone()], ..Default::default() };
    let progress = run(&options, Default::default(), &Control::new(false), None, &|_| {}).await.unwrap();
    assert_eq!((progress.phase, progress.full, progress.unstyled), (ScanPhase::Done, 4, None));
    let dir = dir.path().join("music");
    let store = store(&dir);
    let model = kumi_listening::embed::own_style_id();
    let reader = VectorReader::open(&dir, &store, &model).unwrap();
    let id = |name: &str| store.song_at(&at(&music, name)).unwrap().unwrap().id;
    let (low, low_again, high) = (id("Low/Album/01 Low 0.wav"), id("Low/Album/02 Low 1.wav"), id("High/Album/01 High 0.wav"));
    let apart = |a: i64, b: i64| reader.distance(a, &reader.get(b).unwrap()).unwrap();
    eprintln!("low to low {:.3}, low to high {:.3}", apart(low, low_again), apart(low, high));
    assert!(apart(low, low_again) < apart(low, high));
}

/// What hearing a song costs on this computer, for the songs in the folder KUMI_SCAN_TIMING names, each heard lightly
/// and in full with the real style model (KUMI_TEST_MODELS), and the most memory it took. Run it optimized and with
/// its output: `cargo test --release -p kumi-listening --test it music_scan::timing -- --nocapture`.
#[tokio::test(flavor = "current_thread")]
async fn timing_songs_heard_lightly_and_in_full() {
    let (Some(models), Some(folder)) = (
        std::env::var("KUMI_TEST_MODELS").ok().filter(|folder| !folder.is_empty()),
        std::env::var("KUMI_SCAN_TIMING").ok().filter(|folder| !folder.is_empty()),
    ) else {
        eprintln!("skipped: set KUMI_TEST_MODELS (as for the style model's tests) and KUMI_SCAN_TIMING to a folder of songs");
        return;
    };
    std::env::set_var("KUMI_HOME", Path::new(&models).parent().unwrap());
    use kumi_listening::music::hear::{hear, Clap, Ears, Heard};
    let signal = kumi_common::abort::Signal::new();
    let clap = Clap::from_slot(&signal).await.expect("the style model");
    let dir = tempfile::tempdir().unwrap();
    let mut store = store(dir.path());
    let pass = kumi_listening::music::scan::TagsPass {
        folders: std::slice::from_ref(&folder),
        skip: &[],
        signal: &signal,
        readers: 4,
        progress: &|_| {},
        hold: &|| {},
    };
    let started = std::time::Instant::now();
    let (passed, _) = kumi_listening::music::scan::tags_pass(&mut store, &pass).unwrap();
    eprintln!("tags pass: {} songs in {:.2} s", passed.walked, started.elapsed().as_secs_f64());
    let say = |_: &str| {};
    let ears = Ears { style: Some(&clap), say: &say, signal: &signal };
    let songs = store.songs_where("1", "id", 50).unwrap();
    let (mut light, mut full) = (vec![], vec![]);
    for song in &songs {
        for (stage, times) in [(Stage::Light, &mut light), (Stage::Full, &mut full)] {
            let started = std::time::Instant::now();
            let heard = hear(song, stage, &ears).await;
            let took = started.elapsed().as_secs_f64();
            let seconds = match &heard {
                Heard::Done { seconds, vector, .. } => {
                    assert!(vector.is_some(), "{} wasn't embedded", song.path);
                    *seconds
                }
                other => panic!("{}: {other:?}", song.path),
            };
            eprintln!("{:?} {:>6.1} s song: {took:.2} s  ({})", stage, seconds, song.path.rsplit('/').next().unwrap());
            times.push(took);
        }
    }
    let mean = |times: &[f64]| times.iter().sum::<f64>() / times.len().max(1) as f64;
    eprintln!("light: {:.2} s a song; full: {:.2} s a song ({} songs)", mean(&light), mean(&full), songs.len());
    #[cfg(unix)]
    {
        let mut usage: libc::rusage = unsafe { std::mem::zeroed() };
        unsafe { libc::getrusage(libc::RUSAGE_SELF, &mut usage) };
        // macOS counts bytes, Linux kilobytes.
        let bytes = if cfg!(target_os = "macos") { usage.ru_maxrss as f64 } else { usage.ru_maxrss as f64 * 1024. };
        eprintln!("most memory: {:.0} MB", bytes / 1e6);
    }
}

/// Waits (a while at most) for the music's status to come to `state`.
async fn until(music: &kumi_listening::music::learning::Music, state: kumi_listening::music::learning::MusicState) {
    for _ in 0..1_200 {
        if music.status().state == state {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    panic!("never {state:?}: {:?}", music.status());
}

#[tokio::test(flavor = "current_thread")]
async fn the_app_sees_the_scan_wait_while_kumi_listens_then_learn_and_finish() {
    use kumi_listening::music::learning::{create_music, MusicOptions, MusicState};
    tokio::task::LocalSet::new()
        .run_until(async {
            let dir = tempfile::tempdir().unwrap();
            let music = library(dir.path());
            let mut options = MusicOptions::new(dir.path().join("kumi/music"), Some(vec![music.clone()]), Default::default());
            (options.fork, options.style, options.delay_ms) = (false, Some(Fake::new("test-model") as Rc<dyn Style>), 0);
            let handle = create_music(options);
            let told = Rc::new(RefCell::new(vec![]));
            let _unlisten = handle.on_status({
                let told = told.clone();
                Rc::new(move |status: kumi_listening::music::learning::MusicStatus| told.borrow_mut().push(status.state))
            });
            // Kumi is playing the Set: the scan waits, without walking a folder.
            handle.set_playing(true);
            handle.start();
            until(&handle, MusicState::Paused).await;
            tokio::time::sleep(std::time::Duration::from_millis(300)).await;
            assert!(!database(&dir.path().join("kumi/music")).exists(), "nothing done while paused");
            handle.set_playing(false);
            until(&handle, MusicState::Done).await;
            let status = handle.status();
            assert_eq!((status.songs, status.full, status.skipped, status.folders), (6, 4, 2, vec![music]));
            assert!(status.learned_at.is_some());
            assert!(told.borrow().contains(&MusicState::Learning), "{:?}", told.borrow());
            // No folders: Kumi stops learning the music.
            handle.set_folders(Some(vec![])).unwrap();
            assert_eq!(handle.status().state, MusicState::Off);
            handle.close().await;
        })
        .await;
}

/// A style model of the test's own that does something as it hears: `hook` is told each call's count and file.
struct Hooked {
    calls: Cell<usize>,
    hook: Box<dyn Fn(usize, &Path)>,
}

#[async_trait(?Send)]
impl Style for Hooked {
    fn model(&self) -> &str {
        "test-model"
    }
    async fn embed(
        &self,
        file: &Path,
        _: &[f64],
        _: Option<f64>,
        _: Say<'_>,
        _: &kumi_common::abort::Signal,
    ) -> Result<(Vec<f32>, String), String> {
        self.calls.set(self.calls.get() + 1);
        (self.hook)(self.calls.get(), file);
        let folder = file.parent().and_then(Path::parent).and_then(|folder| folder.file_name()).unwrap().to_string_lossy().into_owned();
        let mut vector = vec![0.01f32; DIMS];
        vector[folder.bytes().map(usize::from).sum::<usize>() % DIMS] = 1.;
        Ok((vector, "test-model".into()))
    }
}

/// A song as music.db keeps it, for a song that's in it.
fn kept(store: &MusicStore, path: &str) -> kumi_listening::music::store::Song {
    store.song_at(path).unwrap().unwrap()
}

#[test]
fn the_full_pass_takes_the_songs_heard_lightly_before_this_run() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = MusicStore::open(&database(dir.path())).unwrap();
    let rows: Vec<(Walked, Tags)> = (0..3)
        .map(|n| {
            let file = Walked { path: format!("/m/{n}.mp3"), folder: "/m".into(), size: 1, mtime: n, ..Default::default() };
            (file, Tags { artist: Some(format!("artist {n}")), title: Some("t".into()), names_from: NamesFrom::Tags, ..Default::default() })
        })
        .collect();
    store.write_songs(&rows).unwrap();
    // 0 heard lightly before this run (which began at 200), 1 during it, 2 not yet.
    store.connection().execute("UPDATE songs SET stage = 1, heard_at = 100 WHERE path = '/m/0.mp3'", []).unwrap();
    store.connection().execute("UPDATE songs SET stage = 1, heard_at = 500 WHERE path = '/m/1.mp3'", []).unwrap();
    let mut busy = HashSet::new();
    let mut order = vec![];
    while let Some(next) = next(&store, &Aside { songs: &busy, folders: &[], since: 200 }, &mut VecDeque::new(), &[], &mut 0).unwrap() {
        let Next::Hear(song, stage) = next else { unreachable!() };
        order.push((song.path.clone(), stage));
        busy.insert(song.id);
    }
    assert_eq!(
        order,
        [("/m/2.mp3".to_string(), Stage::Light), ("/m/0.mp3".to_string(), Stage::Full)],
        "this run's light pass waits for the next run"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn a_song_whose_file_isnt_there_now_is_left_for_later() {
    use kumi_listening::music::hear::{hear, Ears, Heard};
    let dir = tempfile::tempdir().unwrap();
    let mut store = MusicStore::open(&database(&dir.path().join("kumi"))).unwrap();
    let music = dir.path().join("Music").to_string_lossy().into_owned();
    std::fs::create_dir_all(&music).unwrap();
    let file = |folder: &str, name: &str| Walked { path: at(folder, name), folder: folder.into(), size: 1, mtime: 1, ..Default::default() };
    let unplugged = dir.path().join("Unplugged").to_string_lossy().into_owned();
    store
        .write_songs(&[(file(&music, "A/B/01 Gone.wav"), Tags::default()), (file(&unplugged, "C/D/01 Away.wav"), Tags::default())])
        .unwrap();
    let (signal, say) = (kumi_common::abort::Signal::new(), |_: &str| {});
    let ears = Ears { style: None, say: &say, signal: &signal };
    let heard = hear(&kept(&store, &at(&music, "A/B/01 Gone.wav")), Stage::Light, &ears).await;
    assert!(matches!(heard, Heard::Later { away: false, .. }), "gone since the walk, which sees it next: {heard:?}");
    let heard = hear(&kept(&store, &at(&unplugged, "C/D/01 Away.wav")), Stage::Light, &ears).await;
    assert!(matches!(&heard, Heard::Later { away: true, why } if why.contains("Unplugged")), "its drive away: {heard:?}");
}

#[tokio::test(flavor = "current_thread")]
async fn a_stop_while_a_songs_length_is_read_keeps_nothing_against_it() {
    use kumi_listening::music::hear::{hear, Ears, Heard};
    let dir = tempfile::tempdir().unwrap();
    let mut store = MusicStore::open(&database(&dir.path().join("kumi"))).unwrap();
    let music = dir.path().join("Music").to_string_lossy().into_owned();
    // No length in its tags: its file is decoded to find it.
    let path = at(&music, "A/B/01 Untagged.mp3");
    std::fs::create_dir_all(Path::new(&path).parent().unwrap()).unwrap();
    std::fs::write(&path, b"not really an mp3").unwrap();
    store
        .write_songs(&[(Walked { path: path.clone(), folder: music.clone(), size: 17, mtime: 1, ..Default::default() }, Tags::default())])
        .unwrap();
    let (signal, say) = (kumi_common::abort::Signal::new(), |_: &str| {});
    signal.cancel();
    let heard = hear(&kept(&store, &path), Stage::Light, &Ears { style: None, say: &say, signal: &signal }).await;
    assert!(matches!(heard, Heard::Stop(_)), "stopped, not a song that won't decode: {heard:?}");
}

#[cfg(not(target_os = "macos"))]
#[tokio::test(flavor = "current_thread")]
async fn with_no_ffmpeg_here_the_scan_stops_and_keeps_nothing_against_the_song() {
    use kumi_listening::music::hear::{hear, Ears, Heard};
    let dir = tempfile::tempdir().unwrap();
    // No ffmpeg Kumi can find, and none it can fetch.
    std::env::set_var("KUMI_FFMPEG", dir.path().join("no-ffmpeg"));
    let mut store = MusicStore::open(&database(&dir.path().join("kumi"))).unwrap();
    let music = dir.path().join("Music").to_string_lossy().into_owned();
    let path = at(&music, "A/B/01 Song.mp3");
    std::fs::create_dir_all(Path::new(&path).parent().unwrap()).unwrap();
    std::fs::write(&path, b"an mp3 Kumi can't read itself").unwrap();
    let tags = Tags { seconds: Some(200.), ..Default::default() };
    store.write_songs(&[(Walked { path: path.clone(), folder: music.clone(), size: 29, mtime: 1, ..Default::default() }, tags)]).unwrap();
    let (signal, say) = (kumi_common::abort::Signal::new(), |_: &str| {});
    let heard = hear(&kept(&store, &path), Stage::Light, &Ears { style: None, say: &say, signal: &signal }).await;
    assert!(matches!(&heard, Heard::Stop(why) if why.contains("ffmpeg")), "{heard:?}");
}

#[tokio::test(flavor = "current_thread")]
async fn a_scan_over_songs_too_short_to_hear_ends_done_having_skipped_them() {
    let dir = tempfile::tempdir().unwrap();
    let music = dir.path().join("Music");
    for n in 1..=3 {
        song(Path::new(&at(&music.to_string_lossy(), &format!("A{n}/Album/01 Short {n}.wav"))), 2., 440., 0.5);
    }
    let reports = RefCell::new(vec![]);
    let options = ScanOptions {
        dir: dir.path().join("kumi").to_string_lossy().into_owned(),
        folders: vec![music.to_string_lossy().into_owned()],
        ..Default::default()
    };
    let progress =
        run(&options, Default::default(), &Control::new(false), Some(Fake::new("test-model")), &|reply| reports.borrow_mut().push(reply))
            .await
            .unwrap();
    assert_eq!(
        (progress.phase, progress.songs, progress.skipped, progress.heard, progress.light),
        (ScanPhase::Done, 3, 3, 3, 0),
        "{progress:?}"
    );
    assert!(reports.borrow().iter().any(|reply| matches!(reply, ScanReply::Progress { progress } if progress.phase == ScanPhase::Hearing)));
}

#[tokio::test(flavor = "current_thread")]
async fn a_song_away_while_its_heard_waits_for_its_drive_and_is_heard_once_its_back() {
    let dir = tempfile::tempdir().unwrap();
    let (near, far) = (dir.path().join("Near"), dir.path().join("Far"));
    // Heard newest first: the far drive's song, the oldest, is picked after the first song is heard.
    let songs = [(&near, "A/Album/01 A.wav", 110.), (&near, "B/Album/01 B.wav", 2_000.), (&far, "C/Album/01 C.wav", 440.)];
    for (n, (folder, name, hz)) in songs.iter().enumerate() {
        let path = at(&folder.to_string_lossy(), name);
        song(Path::new(&path), 31., *hz, 0.5);
        let when = std::time::SystemTime::now() - std::time::Duration::from_secs(600 * n as u64);
        std::fs::File::options().write(true).open(&path).unwrap().set_modified(when).unwrap();
    }
    let kumi = dir.path().join("kumi");
    let options = ScanOptions {
        dir: kumi.to_string_lossy().into_owned(),
        folders: vec![near.to_string_lossy().into_owned(), far.to_string_lossy().into_owned()],
        ..Default::default()
    };
    // The far drive is unplugged as the first song is heard (all three were walked).
    let (unplugged, moved) = (dir.path().join("Unplugged"), far.clone());
    let style = Rc::new(Hooked {
        calls: Cell::new(0),
        hook: Box::new(move |call, _| {
            if call == 1 {
                std::fs::rename(&moved, &unplugged).unwrap();
            }
        }),
    });
    let said = RefCell::new(vec![]);
    let report = |reply: ScanReply| {
        if let ScanReply::Said { text } = reply {
            said.borrow_mut().push(text);
        }
    };
    let first = run(&options, Default::default(), &Control::new(false), Some(style.clone()), &report).await.unwrap();
    assert_eq!((first.phase, first.full, first.skipped), (ScanPhase::Done, 2, 0), "{first:?}");
    let far_song = kept(&store(&kumi), &at(&far.to_string_lossy(), "C/Album/01 C.wav"));
    assert_eq!((far_song.stage, far_song.skipped, far_song.gone), (0, None, false), "nothing kept against it");
    assert!(said.borrow().iter().any(|text| text.contains("Far") && text.contains("its songs wait")), "{:?}", said.borrow());
    // Plugged in again: heard.
    std::fs::rename(dir.path().join("Unplugged"), &far).unwrap();
    let second = run(&options, Default::default(), &Control::new(false), Some(Fake::new("test-model")), &|_| {}).await.unwrap();
    assert_eq!((second.phase, second.full), (ScanPhase::Done, 3), "{second:?}");
}

#[tokio::test(flavor = "current_thread")]
async fn songs_moved_one_for_one_are_on_the_map_and_in_the_clusters_at_the_scans_end() {
    let dir = tempfile::tempdir().unwrap();
    let music = library(dir.path());
    let kumi = dir.path().join("kumi");
    scan(&kumi, &music, Fake::new("test-model"), &Control::new(false)).await;
    // The producer moves an artist's folder: as many songs, at new paths.
    std::fs::rename(at(&music, "Low"), at(&music, "Bass")).unwrap();
    let progress = scan(&kumi, &music, Fake::new("test-model"), &Control::new(false)).await;
    // Heard lightly this run (in full the next).
    assert_eq!((progress.phase, progress.full, progress.light), (ScanPhase::Done, 2, 2), "{progress:?}");
    let store = store(&kumi);
    let moved = kept(&store, &at(&music, "Bass/Album/01 Low 0.wav"));
    let place: (Option<f64>, Option<i64>) = store
        .connection()
        .query_row("SELECT x, cluster FROM songs WHERE id = ?1", [moved.id], |row| Ok((row.get(0)?, row.get(1)?)))
        .unwrap();
    assert!(place.0.is_some() && place.1.is_some(), "the moved songs are on the map, in a style: {place:?}");
    let (songs, _) = store.map().unwrap();
    assert_eq!(songs.len(), 4, "the songs gone are off it");
}

#[tokio::test(flavor = "current_thread")]
async fn while_hearing_the_counts_said_follow_the_songs_heard() {
    let dir = tempfile::tempdir().unwrap();
    let music = library(dir.path());
    let reports = RefCell::new(vec![]);
    let options = ScanOptions { dir: dir.path().join("kumi").to_string_lossy().into_owned(), folders: vec![music], ..Default::default() };
    let progress =
        run(&options, Default::default(), &Control::new(false), Some(Fake::new("test-model")), &|reply| reports.borrow_mut().push(reply))
            .await
            .unwrap();
    let said: Vec<ScanProgress> = reports
        .borrow()
        .iter()
        .filter_map(|reply| match reply {
            ScanReply::Progress { progress } if progress.phase == ScanPhase::Hearing => Some(progress.clone()),
            _ => None,
        })
        .collect();
    assert!(!said.is_empty());
    for (before, after) in said.iter().zip(&said[1..]) {
        assert!(after.light + after.full + after.skipped >= before.light + before.full + before.skipped, "{before:?} then {after:?}");
    }
    assert!(said.iter().all(|progress| progress.light + progress.full + progress.skipped <= progress.songs));
    assert_eq!((progress.full, progress.skipped), (4, 2));
}

#[tokio::test(flavor = "current_thread")]
async fn a_pause_lets_the_songs_under_way_finish_before_the_scan_waits() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let dir = tempfile::tempdir().unwrap();
            let music = library(dir.path());
            let kumi = dir.path().join("kumi");
            let control = Control::new(false);
            // Kumi starts listening as the first song is with the style model; another song is under way.
            let pausing = control.clone();
            let style = Rc::new(Hooked {
                calls: Cell::new(0),
                hook: Box::new(move |call, _| {
                    if call == 1 {
                        pausing.pause(true);
                    }
                }),
            });
            let options = ScanOptions { dir: kumi.to_string_lossy().into_owned(), folders: vec![music], ..Default::default() };
            let running = tokio::task::spawn_local({
                let (control, style) = (control.clone(), style.clone());
                async move { run(&options, Default::default(), &control, Some(style), &|_| {}).await.unwrap() }
            });
            let heard = || -> i64 {
                let Ok(store) = MusicStore::open(&database(&kumi)) else { return 0 };
                store.connection().query_row("SELECT count(*) FROM songs WHERE stage >= 1", [], |row| row.get(0)).unwrap()
            };
            // While it waits, the song that was under way is heard and kept, not left to its converter's deadline.
            let started = std::time::Instant::now();
            while heard() < 2 {
                assert!(started.elapsed() < std::time::Duration::from_secs(120), "the song under way sits unheard: {}", heard());
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            }
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            assert_eq!((heard(), style.calls.get()), (2, 2), "and nothing more while it waits");
            assert!(!running.is_finished());
            control.pause(false);
            let progress = running.await.unwrap();
            assert_eq!((progress.phase, progress.full, progress.skipped), (ScanPhase::Done, 4, 2));
        })
        .await;
}
