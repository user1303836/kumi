//! The map's server: only its token, its own Host and Origin, points by id and never by path, Range requests, a page
//! that fetches nothing from anywhere else, and an action that reaches the input box. And what it serves of /map's
//! data: both maps for the page, and a point and an action by the ids the page sends.
use kumi::map::{asked, songs_layer, sounds_layer, Action, Kind, MapData, Page};
use kumi::map_server::{byte_range, is_id, open, unescaped, Served, PAGE};
use kumi_library::{map::MapSound, SoundClass, SoundKind};
use kumi_listening::music::store::{MapSong, MapStyle};
use serde_json::{json, Value};
use std::{cell::RefCell, path::PathBuf, rc::Rc};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
};

/// What came back: the status, the head's lines, and the body.
pub(crate) async fn ask(port: u16, request: &str) -> (u16, String, Vec<u8>) {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    stream.write_all(request.as_bytes()).await.unwrap();
    let mut bytes = vec![];
    stream.read_to_end(&mut bytes).await.unwrap();
    let end = bytes.windows(4).position(|window| window == b"\r\n\r\n").unwrap();
    let head = String::from_utf8(bytes[..end].to_vec()).unwrap();
    let status = head.split(' ').nth(1).unwrap().parse().unwrap();
    (status, head, bytes[end + 4..].to_vec())
}

pub(crate) fn get(path: &str, host: &str, more: &str) -> String {
    format!("GET {path} HTTP/1.1\r\nHost: {host}\r\n{more}\r\n")
}

pub(crate) fn post(path: &str, host: &str, origin: &str, body: &str) -> String {
    format!(
        "POST {path} HTTP/1.1\r\nHost: {host}\r\nOrigin: {origin}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    )
}

#[tokio::test(flavor = "current_thread")]
async fn the_server_answers_only_its_own_page_by_its_token_and_plays_points_by_id() {
    let dir = tempfile::tempdir().unwrap();
    let song = dir.path().join("Song.wav");
    let tone: Vec<f32> = (0..8_000).map(|t| (t as f32 * 0.35).sin() * 0.5).collect();
    std::fs::write(&song, kumi_media::audio::decode::float_wav(&[tone], 8_000)).unwrap();
    let asked: Rc<RefCell<Vec<(String, String)>>> = Rc::default();
    let served = Served {
        points: Box::new(|| Box::pin(async { r#"{"layers":[],"actions":[]}"#.to_string() })),
        audio: Box::new({
            let song = song.clone();
            move |id| (id == "songs:1").then(|| (song.clone(), 0.25, 0.5))
        }),
        action: Box::new({
            let asked = asked.clone();
            move |action, id| {
                let known = ["more", "reference", "grow"].contains(&action);
                if known {
                    asked.borrow_mut().push((action.to_string(), id.to_string()));
                }
                known
            }
        }),
    };
    tokio::task::LocalSet::new()
        .run_until(async move {
            let server = open(served).await.unwrap();
            let url: url::Url = server.url.parse().unwrap();
            let (port, token) = (url.port().unwrap(), url.path().trim_matches('/').to_string());
            assert_eq!((url.host_str(), token.len()), (Some("127.0.0.1"), 32), "{}", server.url);
            let host = format!("127.0.0.1:{port}");
            let home = format!("/{token}/");

            // The page, and only with its token.
            let (status, head, body) = ask(port, &get(&home, &host, "")).await;
            assert_eq!((status, body.as_slice()), (200, PAGE.as_bytes()));
            assert!(head.contains("Content-Security-Policy: default-src 'none'") && head.contains("Cache-Control: no-store"), "{head}");
            assert_eq!(ask(port, &get(&home, &format!("localhost:{port}"), "")).await.0, 200, "localhost is the same place");
            for wrong in ["/", "/00000000000000000000000000000000/", &format!("/{token}"), &format!("/{token}x/")] {
                assert_eq!(ask(port, &get(wrong, &host, "")).await.0, 404, "{wrong}");
            }
            // Another name for it, as a page elsewhere would use (DNS rebinding), or another page's POST: refused.
            assert_eq!(ask(port, &get(&home, "evil.example", "")).await.0, 403);
            assert_eq!(ask(port, &get(&home, &format!("127.0.0.1:{}", port + 1), "")).await.0, 403);
            assert_eq!(ask(port, &format!("GET {home} HTTP/1.1\r\n\r\n")).await.0, 403, "no Host");
            let action = format!("{home}action");
            assert_eq!(ask(port, &post(&action, &host, "http://evil.example", r#"{"action":"more","id":"songs:1"}"#)).await.0, 403);

            // The points.
            let (status, head, body) = ask(port, &get(&format!("{home}points"), &host, "")).await;
            assert_eq!((status, String::from_utf8(body).unwrap().as_str()), (200, r#"{"layers":[],"actions":[]}"#));
            assert!(head.contains("application/json"));

            // A point plays by its id, as a WAV of its stretch (half a second at 8 kHz, one channel), and in ranges.
            let audio = format!("{home}audio/songs:1");
            let (status, head, wav) = ask(port, &get(&audio, &host, "")).await;
            assert_eq!((status, &wav[..4], wav.len()), (200, &b"RIFF"[..], 44 + 4_000 * 4));
            assert!(head.contains("Content-Type: audio/wav") && head.contains("Accept-Ranges: bytes"), "{head}");
            let (status, head, part) = ask(port, &get(&audio, &host, "Range: bytes=0-43\r\n")).await;
            assert_eq!((status, part.as_slice()), (206, &wav[..44]));
            assert!(head.contains(&format!("Content-Range: bytes 0-43/{}", wav.len())), "{head}");
            assert_eq!(ask(port, &get(&audio, &host, "Range: bytes=999999-\r\n")).await.0, 416);
            // As a browser asks for it, its colon escaped.
            let (status, _, escaped) = ask(port, &get(&format!("{home}audio/songs%3A1"), &host, "")).await;
            assert_eq!((status, escaped), (200, wav.clone()));
            // Never a path.
            for path in ["%2Fetc%2Fpasswd", "..%2F..%2Fetc%2Fpasswd", "../../etc/passwd", "Song.wav"] {
                assert_eq!(ask(port, &get(&format!("{home}audio/{path}"), &host, "")).await.0, 400, "{path}");
            }
            let song_path = song.to_string_lossy().into_owned();
            assert_eq!(ask(port, &get(&format!("{home}audio/{song_path}"), &host, "")).await.0, 400);
            assert_eq!(ask(port, &get(&format!("{home}audio/songs:2"), &host, "")).await.0, 404, "no such point");

            // An action reaches the input box; anything else doesn't.
            assert_eq!(ask(port, &post(&action, &host, &format!("http://{host}"), r#"{"action":"more","id":"songs:1"}"#)).await.0, 204);
            assert_eq!(ask(port, &post(&action, &host, &format!("http://{host}"), r#"{"action":"delete","id":"songs:1"}"#)).await.0, 400);
            assert_eq!(
                ask(port, &post(&action, &host, &format!("http://{host}"), &format!(r#"{{"action":"more","id":"{song_path}"}}"#))).await.0,
                400
            );
            assert_eq!(ask(port, &post(&action, &host, &format!("http://{host}"), "not json")).await.0, 400);
            assert_eq!(ask(port, &get(&action, &host, "")).await.0, 405);
            assert_eq!(*asked.borrow(), [("more".to_string(), "songs:1".to_string())]);

            // Closed: nothing answers there.
            server.close();
            for _ in 0..10 {
                tokio::task::yield_now().await;
            }
            assert!(TcpStream::connect(("127.0.0.1", port)).await.is_err(), "closed");
            let _: PathBuf = song;
        })
        .await;
}

#[test]
fn the_page_fetches_nothing_from_anywhere_else() {
    for outside in ["http:", "https:", "src=", "<link", "@import", "url(", "//cdn", "<iframe"] {
        assert!(!PAGE.contains(outside), "the page has {outside}");
    }
    for own in [r#"fetch("points")"#, r#"fetch("action""#, r#""audio/" +"#] {
        assert!(PAGE.contains(own), "the page asks its own server: {own}");
    }
}

#[test]
fn ranges_and_ids_are_read_strictly() {
    assert_eq!(byte_range("bytes=0-9", 100), Some((0, 9)));
    assert_eq!(byte_range("bytes=90-", 100), Some((90, 99)));
    assert_eq!(byte_range("bytes=-10", 100), Some((90, 99)));
    assert_eq!(byte_range("bytes=50-200", 100), Some((50, 99)));
    for refused in ["bytes=100-", "bytes=5-4", "bytes=0-1,5-6", "items=0-1", "bytes=x-1", "bytes=-"] {
        assert_eq!(byte_range(refused, 100), None, "{refused}");
    }
    assert_eq!(byte_range("bytes=0-0", 0), None, "nothing to give");
    assert!(is_id("songs:12") && is_id("sounds:0") && is_id("a-b_c"));
    assert_eq!(unescaped("sounds%3A12").as_deref(), Some("sounds:12"));
    assert_eq!(unescaped("%2Fetc%2fpasswd").as_deref(), Some("/etc/passwd"), "read, then refused as no id");
    for refused in ["%", "%3", "%zz", "%+1", "%ff"] {
        assert_eq!(unescaped(refused), None, "{refused}");
    }
    for refused in ["", "/tmp/a.wav", "a.wav", "%2F", "a b", &"x".repeat(65)] {
        assert!(!is_id(refused), "{refused}");
    }
}

#[test]
fn the_page_gets_both_maps_and_names_points_and_actions_by_id() {
    let sound = |n: usize, class: Option<SoundClass>, x: f64| MapSound {
        path: format!("/Samples/One {n}.wav"),
        x,
        y: 0.25,
        name: format!("One {n}"),
        place: "Samples / One".into(),
        class,
        kind: Some(SoundKind::OneShot),
        seconds: Some(0.5),
    };
    let song = |id: i64, cluster: Option<usize>| MapSong {
        id,
        x: 0.123456789,
        y: 0.987654321,
        cluster,
        path: format!("/Music/Song {id}.flac"),
        name: format!("Artist – Song {id}"),
        genre: Some("Dub".into()),
        seconds: Some(200.),
    };
    let styles = vec![MapStyle { songs: 1, examples: vec![40], genres: vec!["Dub".into()], names: vec!["dark".into(), "rainy".into()] }];
    let data = MapData {
        sounds: sounds_layer(vec![sound(1, Some(SoundClass::Kick), 0.5), sound(2, None, 0.75)]),
        songs: songs_layer(vec![song(40, Some(0)), song(7, None)], styles),
    };
    let page = Page::new(data.clone());
    let map: Value = serde_json::from_str(&page.points()).unwrap();
    let (sounds, songs) = (&map["layers"][0], &map["layers"][1]);
    assert_eq!((&sounds["kind"], &sounds["title"], &sounds["empty"]), (&json!("sounds"), &json!("Sounds"), &Value::Null));
    assert_eq!(sounds["points"]["id"], json!([0, 1]), "a sound by its place on the map");
    assert_eq!(sounds["points"]["name"], json!(["One 1", "One 2"]));
    assert_eq!(sounds["places"][sounds["points"]["place"][1].as_u64().unwrap() as usize], json!("Samples / One"));
    let kick = &sounds["groups"][sounds["points"]["group"][0].as_u64().unwrap() as usize];
    assert_eq!((&kick["name"], &kick["color"], &kick["count"]), (&json!("kick"), &json!("#ff6b6b"), &json!(1)));
    assert_eq!(songs["points"]["id"], json!([40, 7]), "a song by its id");
    assert_eq!((&songs["points"]["x"][0], &songs["points"]["y"][0]), (&json!(0.12346), &json!(0.98765)), "five decimals");
    assert_eq!(songs["groups"][0]["name"], json!("c1 · dark rainy"));
    assert_eq!(
        map["actions"],
        json!([
            {"action": "more", "label": "Find more like this"},
            {"action": "reference", "label": "Use it as the reference"},
            {"action": "grow", "label": "Grow it: a match run aimed at it"},
        ])
    );
    // A point by the id the page sends, and nothing for one it doesn't have.
    let (kind, point) = page.point("songs:7").unwrap();
    assert_eq!((kind, point.name.as_str()), (Kind::Songs, "Artist – Song 7"));
    assert_eq!(page.point("sounds:1").map(|(kind, point)| (kind, point.path.as_str())), Some((Kind::Sounds, "/Samples/One 2.wav")));
    for none in ["sounds:2", "sounds:-1", "songs:8", "songs:", "tracks:1", "sounds", "", "songs:7:1"] {
        assert!(page.point(none).is_none(), "{none}");
    }
    // An action asks what the terminal's map asks; one it doesn't offer asks nothing.
    assert_eq!(page.asked("more", "songs:40").as_deref(), Some(asked(Kind::Songs, &data.songs.points[0], Action::More).as_str()));
    assert_eq!(page.asked("grow", "sounds:0").unwrap(), "Make a sound like this one, with a match run aimed at it: /Samples/One 1.wav");
    assert!(page.asked("delete", "songs:40").is_none() && page.asked("more", "songs:41").is_none());
}
