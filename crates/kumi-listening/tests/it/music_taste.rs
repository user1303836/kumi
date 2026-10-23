//! The taste tool and what it says: a fixed library of 150 songs in three styles (their tags, profiles and style vectors
//! written here, nothing heard) answered for each kind of request, byte for byte; the offer, made once; learning when
//! the producer says yes; the brief in words; forgetting it all. `KUMI_BLESS=1` writes the goldens afresh.

use async_trait::async_trait;
use kumi_common::abort::Signal;
use kumi_listening::{
    checklist::Profile,
    embed::Say,
    music::{
        database,
        describe::describe,
        folders::Walked,
        hear::Style,
        learning::{create_music, Music, MusicOptions, MusicState},
        scan::recluster,
        store::{MusicStore, Stage},
        tags::{NamesFrom, Tags},
        tool::{taste_tools_for, TASTE_TOOL},
        vectors::{VectorWriter, DIMS},
    },
};
use kumi_types::daw::DawProfile;
use serde_json::{json, Value};
use std::{
    cell::RefCell,
    path::{Path, PathBuf},
    rc::Rc,
};

const MODEL: &str = "test-model";

/// Checks `actual` against the golden `name` (KUMI_BLESS=1 writes it instead).
fn golden(name: &str, actual: &str) {
    let file = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/goldens/music").join(name);
    if std::env::var_os("KUMI_BLESS").is_some() {
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, actual).unwrap();
        return;
    }
    let expected =
        std::fs::read_to_string(&file).unwrap_or_else(|_| panic!("{} is missing: run this test with KUMI_BLESS=1", file.display()));
    assert_eq!(actual, expected, "{} changed: check the difference, then run this test with KUMI_BLESS=1", file.display());
}

/// A song's profile: its loudness, tilt, lows, dynamics, contrast, reverb and echo.
fn profile(loudness: f64, tilt: f64, lows: f64, plr: f64, range: f64, decay: f64, echo: Option<(f64, f64)>) -> Profile {
    let spread = |value: f64, half: f64| json!({"mid": value, "low": value - half, "high": value + half});
    let mut sound = json!({"decayTime": spread(decay, 0.2)});
    if let Some((ms, falls)) = echo {
        sound["echoTime"] = spread(ms, 10.);
        sound["echoFalls"] = spread(falls, 1.5);
    }
    serde_json::from_value(json!({
        "name": "a song",
        "tracks": 1,
        "regions": (0..8).map(|region| spread(-12. + region as f64, 1.5)).collect::<Vec<_>>(),
        "integrated": spread(loudness, 1.),
        "plr": spread(plr, 1.5),
        "crest": spread(plr - 2., 1.5),
        "lowWidth": spread(lows, 4.),
        "tilt": spread(tilt, 0.5),
        "range": spread(range, 1.5),
        "sound": sound
    }))
    .unwrap()
}

/// A style's songs: its genre tag, its artists and albums, the way its vectors point, and its sound.
struct Styled {
    genre: &'static str,
    artists: [(&'static str, &'static str); 3],
    titles: [&'static str; 3],
    direction: usize,
    sound: fn(f64) -> Profile,
}

const STYLES: [Styled; 3] = [
    Styled {
        genre: "Dubstep",
        artists: [("Burial", "Untrue"), ("Kode9", "Black Sun"), ("Shackleton", "Three EPs")],
        titles: ["Archangel", "9 Samurai", "Blood on My Hands"],
        direction: 3,
        sound: |n| profile(-9. - n, -2.0 - n, -30., 9.5 + n, 6., 2.2, Some((375., 12.))),
    },
    Styled {
        genre: "Dub Techno",
        artists: [("Basic Channel", "BCD"), ("Rhythm & Sound", "Rhythm & Sound"), ("Deepchord", "Hash-Bar Loops")],
        titles: ["Phylyps Trak", "Mango Drive", "Grey Area"],
        direction: 77,
        sound: |n| profile(-12. - n, -2.8 - n, -28., 12.5 + n, 3., 1.2, Some((375., 20.))),
    },
    Styled {
        genre: "Pop",
        artists: [("Robyn", "Body Talk"), ("Carly Rae Jepsen", "Emotion"), ("Charli XCX", "Brat")],
        titles: ["Dancing on My Own", "Run Away with Me", "360"],
        direction: 200,
        sound: |n| profile(-7. + n, 0.6 + n, -8., 7.5 + n, 4.5, 0.5, None),
    },
];

/// The test's style model: the library's own vectors are written here; a file it's asked to hear sounds like dubstep.
struct Fixed;

#[async_trait(?Send)]
impl Style for Fixed {
    fn model(&self) -> &str {
        MODEL
    }
    async fn embed(&self, _: &Path, _: &[f64], _: Option<f64>, _: Say<'_>, _: &Signal) -> Result<(Vec<f32>, String), String> {
        Ok((direction(3, 0, 0), MODEL.into()))
    }
}

/// A unit vector pointing mostly at `at` (a style), leaning toward its artist's own, a little apart for each song.
fn direction(at: usize, artist: usize, song: usize) -> Vec<f32> {
    let mut vector = vec![0f32; DIMS];
    vector[at] = 1.;
    vector[at + 1 + artist] = 0.3;
    vector[(at + 5 + song % 7) % DIMS] = 0.05 + (song % 5) as f32 * 0.02;
    kumi_listening::embed::normalized(vector)
}

/// The library: 50 songs of each style, heard in full, their vectors in the style model's file and in clusters. The
/// first dubstep song is a real file (a file's seed).
fn library(root: &Path) -> (PathBuf, String) {
    let dir = root.join("kumi/music");
    let mut store = MusicStore::open(&database(&dir)).unwrap();
    let archangel = root.join("Music/Burial/Untrue/01 Archangel.wav");
    std::fs::create_dir_all(archangel.parent().unwrap()).unwrap();
    std::fs::write(&archangel, kumi_media::audio::decode::float_wav(&[vec![0.1; 800]], 8_000)).unwrap();
    let archangel = archangel.to_string_lossy().into_owned();
    let mut rows = vec![];
    for (style, styled) in STYLES.iter().enumerate() {
        for n in 0..50 {
            let (artist, album) = styled.artists[n % 3];
            let title = if n < 3 { styled.titles[n].to_string() } else { format!("{} Track {n}", styled.genre) };
            let path = if style == 0 && n == 0 { archangel.clone() } else { format!("/library/{artist}/{album}/{n:02} {title}.mp3") };
            let file = Walked { path, folder: "/library".into(), size: 1, mtime: (style * 50 + n) as i64, protected: false };
            let tags = Tags {
                artist: Some(artist.into()),
                album: Some(album.into()),
                title: Some(title),
                genre: Some(styled.genre.into()),
                year: Some(2007),
                seconds: Some(240.),
                names_from: NamesFrom::Tags,
                ..Default::default()
            };
            rows.push((file, tags, style, n));
        }
    }
    store.write_songs(&rows.iter().map(|(file, tags, ..)| (file.clone(), tags.clone())).collect::<Vec<_>>()).unwrap();
    let mut writer = VectorWriter::open(&dir, MODEL).unwrap();
    for (file, _, style, n) in &rows {
        let id = store.song_at(&file.path).unwrap().unwrap().id;
        let sound = (STYLES[*style].sound)((*n % 5) as f64 * 0.1);
        store.heard(id, Stage::Full, &serde_json::to_string(&sound).unwrap(), 1_700_000_000_000).unwrap();
        writer.put(&store, id, &direction(STYLES[*style].direction, *n % 3, *n)).unwrap();
    }
    writer.settle(&store).unwrap();
    recluster(&mut store, &dir, MODEL).unwrap();
    (dir, archangel)
}

/// The producer's answer, as kept: the folders and the offer, each time it's saved.
type Saved = Rc<RefCell<Vec<(Option<Vec<String>>, bool)>>>;

fn music(dir: &Path, folders: Option<Vec<String>>, offered: bool, places: Vec<PathBuf>) -> (Rc<Music>, Saved) {
    let saved: Saved = Rc::default();
    let mut options = MusicOptions::new(dir.to_path_buf(), folders, Default::default());
    options.offered = offered;
    options.places = Some(places);
    (options.fork, options.style, options.delay_ms) = (false, Some(Rc::new(Fixed) as Rc<dyn Style>), 3_600_000);
    options.save = Some({
        let saved = saved.clone();
        Rc::new(move |folders: Option<&[String]>, offered: bool| {
            saved.borrow_mut().push((folders.map(<[String]>::to_vec), offered));
            Ok(())
        })
    });
    (create_music(options), saved)
}

/// Waits for the music's counts to be read from music.db.
async fn counted(music: &Music) {
    for _ in 0..400 {
        if music.status().songs > 0 {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    panic!("never counted: {:?}", music.status());
}

async fn ask(music: &Rc<Music>, input: Value) -> String {
    let tool = taste_tools_for(&DawProfile::ABLETON, music.clone(), None, None).remove(0);
    assert_eq!(tool.name(), TASTE_TOOL);
    let result = tool.execute(input.as_object().unwrap().clone(), Signal::new()).await.unwrap();
    assert!(!result.is_error, "{result:?}");
    result.text
}

/// Pretty, as a golden keeps it, with the test's folders put by name.
fn pinned(answer: &str, root: &Path) -> String {
    let value: Value = serde_json::from_str(answer).unwrap();
    serde_json::to_string_pretty(&value).unwrap().replace(&root.to_string_lossy().into_owned(), "<root>") + "\n"
}

fn local(test: impl std::future::Future<Output = ()>) {
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    runtime.block_on(tokio::task::LocalSet::new().run_until(test));
}

#[test]
fn not_set_up_it_offers_once_when_it_finds_songs_and_never_when_it_finds_none() {
    local(async {
        let dir = tempfile::tempdir().unwrap();
        let found = dir.path().join("Music");
        for n in 1..=3 {
            std::fs::create_dir_all(found.join("Artist/Album")).unwrap();
            std::fs::write(found.join(format!("Artist/Album/0{n} Song.mp3")), b"tagless").unwrap();
        }
        let (taste, saved) = music(&dir.path().join("kumi/music"), None, false, vec![dir.path().join("Nowhere"), found]);
        let first = ask(&taste, json!({})).await;
        golden("offer.json", &pinned(&first, dir.path()));
        assert_eq!(*saved.borrow(), [(None, true)], "offered: kept");
        golden("offered-before.json", &pinned(&ask(&taste, json!({"like": "Burial"})).await, dir.path()));
        // Nothing found: no offer, and nothing kept, so one comes once their music is there.
        let (none, saved) = music(&dir.path().join("kumi/music"), None, false, vec![dir.path().join("Nowhere")]);
        assert_eq!(ask(&none, json!({})).await, r#"{"learned":0}"#);
        assert!(saved.borrow().is_empty());
        assert!(!none.offered());
    });
}

#[test]
fn a_yes_keeps_the_folders_and_starts_learning_and_forget_takes_it_all_away() {
    local(async {
        let dir = tempfile::tempdir().unwrap();
        let songs = dir.path().join("Music");
        std::fs::create_dir_all(songs.join("A/B")).unwrap();
        std::fs::write(songs.join("A/B/01 One.wav"), kumi_media::audio::decode::float_wav(&[vec![0.2; 8_000 * 2]], 8_000)).unwrap();
        let kumi = dir.path().join("kumi/music");
        let (taste, saved) = music(&kumi, None, true, vec![]);
        let folder = songs.to_string_lossy().into_owned();
        let answer = ask(&taste, json!({"learn": [folder.clone(), dir.path().join("not there").to_string_lossy()]})).await;
        golden("learn.json", &pinned(&answer, dir.path()));
        assert_eq!(saved.borrow().last(), Some(&(Some(vec![folder.clone()]), true)));
        assert_eq!(taste.folders(), [folder]);
        assert!(matches!(taste.status().state, MusicState::Learning | MusicState::Done), "{:?}", taste.status());
        for _ in 0..400 {
            if database(&kumi).exists() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        taste.forget().await.unwrap();
        assert!(!kumi.exists(), "music.db, the vectors and the map are gone");
        assert_eq!((taste.status().state, taste.agreed(), taste.offered()), (MusicState::Off, false, true));
        assert_eq!(saved.borrow().last(), Some(&(None, true)), "nothing to learn, and no offer again");
        let refused = taste_tools_for(&DawProfile::ABLETON, taste.clone(), None, None)
            .remove(0)
            .execute(json!({"learn": ["/no/such/folder"]}).as_object().unwrap().clone(), Signal::new())
            .await
            .unwrap();
        assert!(refused.is_error);
    });
}

#[test]
fn its_answers_say_the_producers_styles_and_their_songs_nearest_what_they_named() {
    local(async {
        let dir = tempfile::tempdir().unwrap();
        let (kumi, archangel) = library(dir.path());
        let (taste, _) = music(&kumi, Some(vec!["/library".into()]), true, vec![]);
        counted(&taste).await;
        for (name, input) in [
            ("styles.json", json!({})),
            ("artist.json", json!({"like": "burial", "count": 6})),
            ("song.json", json!({"like": "Robyn – Dancing on My Own", "count": 4})),
            ("genre.json", json!({"like": "Dub Techno", "count": 4})),
            ("file.json", json!({"like": archangel, "count": 4})),
            ("words.json", json!({"words": "deep dub techno at night", "count": 3})),
            ("words-unknown.json", json!({"words": "baroque harpsichord"})),
            ("unknown.json", json!({"like": "Aphex Twin"})),
        ] {
            golden(name, &pinned(&ask(&taste, input).await, dir.path()));
        }
        // While it learns, it says how far it's got.
        taste.hold(true);
        taste.learn_now();
        assert_eq!(taste.status().state, MusicState::Paused);
        golden("learning.json", &pinned(&ask(&taste, json!({"like": "Kode9", "count": 2})).await, dir.path()));
        taste.close().await;
        // The songs it named are heard in full first.
        let store = MusicStore::open_read(&database(&kumi)).unwrap().unwrap();
        assert!(store.song_at(&archangel).unwrap().unwrap().used_at.is_some());
    });
}

#[test]
fn agreed_but_nothing_heard_yet_it_says_so() {
    local(async {
        let dir = tempfile::tempdir().unwrap();
        let (taste, _) = music(&dir.path().join("kumi/music"), Some(vec!["/library".into()]), true, vec![]);
        golden("nothing-heard.json", &pinned(&ask(&taste, json!({"like": "Burial"})).await, dir.path()));
    });
}

fn spread(value: f64) -> kumi_listening::checklist::Spread {
    kumi_listening::checklist::Spread::point(value, 1.)
}

fn groove(swing: f64, push: f64) -> kumi_listening::checklist::Groove {
    kumi_listening::checklist::Groove { swing_ratio: spread(swing), push_ms: spread(push) }
}

#[test]
fn a_profile_reads_as_one_sentence() {
    let lines: Vec<String> = [
        (STYLES[0].sound)(0.),
        (STYLES[1].sound)(0.),
        (STYLES[2].sound)(0.),
        profile(-16., -3.5, -5., 16., 11., 0.3, None),
        profile(-6., 2., -40., 6., 2., 1.9, Some((250., 40.))),
        Profile {
            tempo: Some(spread(140.2)),
            key: Some("F minor".into()),
            busyness: Some(spread(13.)),
            groove: Some(groove(1.6, 14.)),
            ..(STYLES[0].sound)(0.)
        },
        Profile {
            tempo: Some(spread(96.)),
            key: None,
            busyness: Some(spread(3.)),
            groove: Some(groove(1.02, -11.)),
            ..(STYLES[2].sound)(0.)
        },
    ]
    .iter()
    .map(describe)
    .collect();
    golden("describe.txt", &(lines.join("\n") + "\n"));
}

#[test]
fn keep_hears_the_nearest_songs_in_full_and_keeps_them_for_the_judge_by_name() {
    use kumi_listening::{
        music::worker::{run, Control, ScanOptions},
        references::store::{ReferenceStore, KEPT_VERSION},
    };
    local(async {
        let dir = tempfile::tempdir().unwrap();
        let songs = dir.path().join("Music");
        for (artist, n) in [("Artist A", 1), ("Artist A", 2), ("Artist A", 3), ("Artist B", 1), ("Artist B", 2), ("Artist B", 3)] {
            let path = songs.join(format!("{artist}/Album/0{n} Song {n}.wav"));
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            let samples: Vec<f32> = (0..8_000 * (31 + n)).map(|t| (t as f32 * 0.03 * n as f32).sin() * 0.3).collect();
            std::fs::write(&path, kumi_media::audio::decode::float_wav(&[samples], 8_000)).unwrap();
        }
        let kumi = dir.path().join("kumi/music");
        let folder = songs.to_string_lossy().into_owned();
        let options = ScanOptions { dir: kumi.to_string_lossy().into_owned(), folders: vec![folder.clone()], ..Default::default() };
        run(&options, Default::default(), &Control::new(false), Some(Rc::new(Fixed)), &|_| {}).await.unwrap();
        // Two heard only lightly.
        let store = MusicStore::open(&database(&kumi)).unwrap();
        store.connection().execute("UPDATE songs SET stage = 1 WHERE title IN ('Song 1')", []).unwrap();
        drop(store);
        let references = Rc::new(ReferenceStore::new(dir.path().join("references")));
        let (taste, _) = music(&kumi, Some(vec![folder]), true, vec![]);
        counted(&taste).await;
        let tool = taste_tools_for(&DawProfile::ABLETON, taste.clone(), None, Some(references.clone())).remove(0);
        let result = tool.execute(json!({"like": "Artist A", "keep": true}).as_object().unwrap().clone(), Signal::new()).await.unwrap();
        let answer: Value = serde_json::from_str(&result.text).unwrap();
        assert_eq!(answer["reference"], "your music near Artist A", "{answer}");
        assert!(answer["note"].as_str().unwrap().contains("judge with goal.reference \"your music near Artist A\""));
        let kept = references.load("your music near Artist A").await.unwrap();
        assert_eq!((kept.kind.as_str(), kept.version), ("music", KEPT_VERSION));
        assert_eq!(kept.tracks.len(), 6, "an artist's three and the three nearest: {:?}", kept.tracks);
        assert!(kept.tracks.iter().all(|track| Path::new(&track.source).is_file()), "its tracks are the song files");
        assert_eq!((kept.profile.tracks, kept.profile.vibe_model.as_deref()), (6, Some(MODEL)));
        assert!(kept.profile.vibe.is_some(), "the judge guards their style");
        // Heard in full now.
        let store = MusicStore::open_read(&database(&kumi)).unwrap().unwrap();
        let light: i64 = store.connection().query_row("SELECT count(*) FROM songs WHERE stage < 2", [], |row| row.get(0)).unwrap();
        assert_eq!(light, 0);
        // A style by its label, kept the same way; never the whole library.
        let result = tool.execute(json!({"keep": true}).as_object().unwrap().clone(), Signal::new()).await.unwrap();
        let answer: Value = serde_json::from_str(&result.text).unwrap();
        assert_eq!(answer["seed"], "c1", "{answer}");
        assert!(answer["reference"].as_str().unwrap().starts_with("your music near "), "{answer}");
        taste.close().await;
    });
}

#[test]
fn a_reference_kept_by_an_older_kumi_is_measured_again() {
    use kumi_listening::references::store::{KeptReference, ReferenceStore, KEPT_VERSION};
    local(async {
        let dir = tempfile::tempdir().unwrap();
        let references = ReferenceStore::new(dir.path());
        let kept = |version: u32| KeptReference {
            version,
            key: ReferenceStore::key("dub techno"),
            name: "dub techno".into(),
            kind: "genre".into(),
            tracks: vec![],
            profile: (STYLES[1].sound)(0.),
            at: 1,
            mbid: None,
            stamp: None,
        };
        references.save(&kept(KEPT_VERSION - 1)).await.unwrap();
        assert!(references.load("dub techno").await.is_none(), "an older one is measured again (its tempo, key and groove)");
        references.save(&kept(KEPT_VERSION)).await.unwrap();
        assert!(references.load("dub techno").await.is_some());
    });
}
