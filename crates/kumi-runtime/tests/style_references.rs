//! References for the judge: words, links and files resolved to example tracks (MusicBrainz, ListenBrainz and Spotify
//! stood in for by a local server, yt-dlp by a script), one question when words could mean more than one thing and its
//! answer settling it, profiles with spread, kept while their files stay as they were.
use kumi_common::abort::Signal;
use kumi_runtime::{
    core::contracts::KernelTool,
    listening::{
        checklist::{Profile, Spread},
        measure::measure_samples,
    },
    references::{
        fetch::{kept, Fetcher},
        sources::{embed_entity, spotify_parts, stamp, Kind, Resolved, Sources},
        store::{KeptReference, KeptTrack, ReferenceStore},
        tool::{reference_tools, ReferenceTool},
    },
    video::programs::ProgramOptions,
};
use serde_json::{json, Value};
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
    time::{Duration, Instant},
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const B1: &str = "b1000000-0000-4000-8000-000000000001";
const B2: &str = "b2000000-0000-4000-8000-000000000002";
const LEE: &str = "c1000000-0000-4000-8000-000000000001";
const DUNCAN: &str = "c2000000-0000-4000-8000-000000000002";
const JONI: &str = "e1000000-0000-4000-8000-000000000001";
const BLUE_ALBUM: &str = "a2000000-0000-4000-8000-000000000002";
const DAFT_PUNK: &str = "d2000000-0000-4000-8000-000000000002";
const DISCOVERY_BAND: &str = "d1000000-0000-4000-8000-000000000001";
const DISCOVERY: &str = "a1000000-0000-4000-8000-000000000001";
const SPECIAL: &str = "f3800000-0000-4000-8000-000000000038";

/// A server answering each request's path (and query) with what `answer` gives: a status and a body.
async fn serve_with(answer: impl Fn(&str) -> (u16, String) + 'static) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::task::spawn_local(async move {
        loop {
            let Ok((mut socket, _)) = listener.accept().await else { return };
            let mut input = vec![];
            let mut chunk = [0; 4096];
            while !input.windows(4).any(|w| w == b"\r\n\r\n") {
                let n = socket.read(&mut chunk).await.unwrap_or(0);
                if n == 0 {
                    break;
                }
                input.extend_from_slice(&chunk[..n]);
            }
            let request = String::from_utf8_lossy(&input).to_string();
            let (status, body) = answer(&decoded(request.split_whitespace().nth(1).unwrap_or("/")));
            let reason = match status {
                200 => "OK",
                404 => "Not Found",
                503 => "Service Unavailable",
                _ => "Error",
            };
            let reply = format!("HTTP/1.1 {status} {reason}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
            let _ = socket.write_all(reply.as_bytes()).await;
        }
    });
    format!("http://{address}")
}

/// A server answering with a body (404 without one).
async fn serve(answer: fn(&str) -> Option<String>) -> String {
    serve_with(move |path| answer(path).map_or((404, String::new()), |body| (200, body))).await
}

fn decoded(path: &str) -> String {
    let bytes = path.as_bytes();
    let mut out = vec![];
    let mut at = 0;
    while at < bytes.len() {
        let hex = (bytes[at] == b'%' && at + 3 <= bytes.len())
            .then(|| u8::from_str_radix(std::str::from_utf8(&bytes[at + 1..at + 3]).unwrap_or(""), 16).ok())
            .flatten();
        match hex {
            Some(byte) => {
                out.push(byte);
                at += 3;
            }
            None => {
                out.push(bytes[at]);
                at += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).to_string()
}

/// An album's tracks: (recording, title, seconds).
type Tracks = Vec<(&'static str, &'static str, f64)>;

/// An artist's albums (by how many editions each has) and each album's tracks: (album id, editions, tracks).
fn discography(artist: &str) -> Vec<(&'static str, f64, Tracks)> {
    match artist {
        "burial" => vec![
            ("untrue", 30., vec![("r-archangel", "Archangel", 239.), ("r-near-dark", "Near Dark", 236.), ("r-untrue", "Untrue", 371.)]),
            ("burial", 20., vec![("r-distant", "Distant Lights", 340.), ("r-archangel-2", "Archangel (Remastered)", 239.)]),
        ],
        "bc" => vec![("bc-album", 10., vec![("r-phylyps", "Phylyps Trak", 444.), ("r-q11", "Q1.1", 400.)])],
        "dc" => vec![("dc-album", 8., vec![("r-vantage", "Vantage Isle", 600.), ("r-sommerset", "Sommerset", 480.)])],
        B1 => vec![("blue-1", 5., vec![("r-all-rise", "All Rise", 220.)])],
        LEE => vec![("lee-1", 2., vec![("r-lee-a", "Lee A", 200.), ("r-lee-b", "Lee B", 210.)])],
        DUNCAN => vec![("duncan-1", 2., vec![("r-duncan-a", "Duncan A", 190.)])],
        SPECIAL => vec![("special-1", 3., vec![("r-hold-on", "Hold On Loosely", 280.)])],
        _ => vec![],
    }
}

fn credit(id: &str, name: &str) -> Value {
    json!([{"name": name, "artist": {"id": id, "name": name}}])
}

fn musicbrainz(path: &str) -> Option<String> {
    if path.starts_with("/ws/2/genre/all") {
        return Some("dub\ndub techno\ntechno\n".into());
    }
    if let Some(resource) = path.strip_prefix("/ws/2/url?resource=") {
        return resource
            .starts_with("https://open.spotify.com/artist/SPBURIAL&")
            .then(|| json!({"relations": [{"type": "free streaming", "artist": {"id": "burial", "name": "Burial"}}]}).to_string());
    }
    if let Some(id) = path.strip_prefix("/ws/2/artist/") {
        let id = id.split('?').next().unwrap_or("");
        let (name, relations) = match id {
            B1 => (
                "Blue",
                json!([
                    {"type": "member of band", "direction": "backward", "artist": {"id": LEE, "name": "Lee"}},
                    {"type": "member of band", "direction": "backward", "artist": {"id": DUNCAN, "name": "Duncan"}},
                    {"type": "member of band", "direction": "forward", "artist": {"id": "super", "name": "A Supergroup"}}
                ]),
            ),
            SPECIAL => (".38 Special", json!([])),
            _ => return Some(json!({"error": "Not Found"}).to_string()),
        };
        return Some(json!({"id": id, "name": name, "relations": relations}).to_string());
    }
    if let Some(group) = path.strip_prefix("/ws/2/release-group/") {
        return group.starts_with(BLUE_ALBUM).then(|| {
            json!({"id": BLUE_ALBUM, "title": "Blue", "artist-credit": credit(JONI, "Joni Mitchell"),
                "releases": [{"id": "rel-blue-bootleg", "status": "Bootleg"}, {"id": "rel-blue", "status": "Official"}]})
            .to_string()
        });
    }
    if let Some(artist) = path.strip_prefix("/ws/2/release-group?query=arid:") {
        let artist = artist.split(' ').next().unwrap_or("");
        let groups: Vec<Value> =
            discography(artist).iter().map(|(id, count, _)| json!({"id": id, "count": count, "secondary-types": []})).collect();
        return Some(json!({"release-groups": groups}).to_string());
    }
    if let Some(group) = path.strip_prefix("/ws/2/release?release-group=") {
        let group = group.split('&').next().unwrap_or("");
        let tracks: Vec<Value> = ["burial", "bc", "dc", B1, LEE, DUNCAN, SPECIAL]
            .iter()
            .flat_map(|artist| discography(artist))
            .filter(|(id, _, _)| *id == group)
            .flat_map(|(_, _, tracks)| tracks)
            .map(|(id, title, seconds)| json!({"title": title, "length": seconds * 1000., "recording": {"id": id, "title": title}}))
            .collect();
        return Some(json!({"releases": [{"media": [{"tracks": tracks}]}]}).to_string());
    }
    if let Some(release) = path.strip_prefix("/ws/2/release/") {
        let tracks = match release.split('?').next().unwrap_or("") {
            "rel-discovery" => json!([
                {"title": "One More Time", "length": 320000, "recording": {"id": "r-omt"}},
                {"title": "Aerodynamic", "length": 212000, "recording": {"id": "r-aero"}}
            ]),
            "rel-blue" => json!([
                {"title": "All I Want", "length": 214000, "recording": {"id": "r-aiw"}},
                {"title": "River", "length": 240000, "recording": {"id": "r-river"}}
            ]),
            _ => return None,
        };
        return Some(json!({"media": [{"tracks": tracks}]}).to_string());
    }
    let discovery = json!({"id": DISCOVERY, "title": "Discovery", "score": 100, "artist-credit": credit(DAFT_PUNK, "Daft Punk"),
        "first-release-date": "2001-03-12", "releases": [{"id": "rel-discovery", "status": "Official"}]});
    if path.starts_with("/ws/2/release-group?query=releasegroup:(") {
        // An album and its artist in one search.
        return Some(
            json!({"release-groups": if path.contains("discovery") && path.contains("daft") { vec![discovery] } else { vec![] }})
                .to_string(),
        );
    }
    if path.starts_with("/ws/2/release-group?query=releasegroup:") {
        let groups = if path.contains("releasegroup:\"Discovery\"") {
            vec![discovery]
        } else if path.contains("releasegroup:\"Blue\"") {
            vec![
                json!({"id": BLUE_ALBUM, "title": "Blue", "score": 100, "artist-credit": credit(JONI, "Joni Mitchell"), "first-release-date": "1971-06-22"}),
            ]
        } else if path.contains("releasegroup:\"Burial\"") {
            // The artist's own titled album is the artist.
            vec![json!({"id": "burial", "title": "Burial", "score": 100, "artist-credit": credit("burial", "Burial")})]
        } else {
            vec![]
        };
        return Some(json!({"release-groups": groups}).to_string());
    }
    if path.contains("tag:\"dub techno\"") {
        return Some(
            json!({"artists": [{"id": "bc", "name": "Basic Channel", "score": 100}, {"id": "dc", "name": "Deepchord", "score": 90}]})
                .to_string(),
        );
    }
    let artists = if path.contains("artist:\"Burial\"") {
        json!([{"id": "burial", "name": "Burial", "score": 100, "disambiguation": "UK producer"}, {"id": "burial-2", "name": "Burial", "score": 100, "disambiguation": "punk band"}])
    } else if path.contains("artist:\"Blue\"") {
        json!([
            {"id": B1, "name": "Blue", "score": 100, "disambiguation": "UK boy band", "life-span": {"begin": "2000"}},
            {"id": B2, "name": "Blue", "score": 98, "disambiguation": "Scottish rock band", "life-span": {"begin": "1973"}}
        ])
    } else if path.contains("artist:\"Discovery\"") {
        json!([{"id": DISCOVERY_BAND, "name": "Discovery", "score": 100, "disambiguation": "US indie band"}])
    } else if path.contains("artist:\".38 Special\"") {
        json!([{"id": SPECIAL, "name": ".38 Special", "score": 100}])
    } else if path.starts_with("/ws/2/artist") {
        json!([{"id": "x", "name": "Something Else", "score": 40}])
    } else {
        return None;
    };
    Some(json!({ "artists": artists }).to_string())
}

fn listenbrainz(path: &str) -> Option<String> {
    // Listens: Near Dark most, then Untrue, then Archangel; the rest fewer.
    match path {
        "/1/popularity/recording" => Some(
            json!([
                {"recording_mbid": "r-near-dark", "total_listen_count": 900},
                {"recording_mbid": "r-untrue", "total_listen_count": 700},
                {"recording_mbid": "r-archangel", "total_listen_count": 500},
                {"recording_mbid": "r-archangel-2", "total_listen_count": 400},
                {"recording_mbid": "r-distant", "total_listen_count": 100},
                {"recording_mbid": "r-q11", "total_listen_count": 50},
                {"recording_mbid": "r-phylyps", "total_listen_count": 40},
                {"recording_mbid": "r-vantage", "total_listen_count": 30}
            ])
            .to_string(),
        ),
        "/1/popularity/artist" => Some(
            json!([
                {"artist_mbid": "bc", "total_user_count": 9000},
                {"artist_mbid": "dc", "total_user_count": 8000},
                {"artist_mbid": "burial", "total_user_count": 50000},
                {"artist_mbid": "burial-2", "total_user_count": 40},
                {"artist_mbid": B1, "total_user_count": 3000},
                {"artist_mbid": B2, "total_user_count": 2000},
                {"artist_mbid": JONI, "total_user_count": 20000},
                {"artist_mbid": DISCOVERY_BAND, "total_user_count": 50},
                {"artist_mbid": DAFT_PUNK, "total_user_count": 90000}
            ])
            .to_string(),
        ),
        _ => None,
    }
}

fn spotify(path: &str) -> Option<String> {
    let page = |entity: Value| {
        let data = json!({"props": {"pageProps": {"state": {"data": {"entity": entity}}}}});
        format!("<html><script id=\"__NEXT_DATA__\" type=\"application/json\">{data}</script></html>")
    };
    if path.starts_with("/embed/album/") {
        return Some(page(json!({
            "type": "album", "name": "Untrue",
            "trackList": [
                {"title": "Archangel", "subtitle": "Burial", "duration": 239000},
                {"title": "Near Dark", "subtitle": "Burial", "duration": 236000}
            ]
        })));
    }
    match path {
        "/embed/track/T1" => Some(page(json!({"type": "track", "name": "Near Dark", "artists": [{"name": "Burial"}], "duration": 236000}))),
        "/embed/track/T2" => Some("<html>no data</html>".into()),
        "/oembed?url=https://open.spotify.com/track/T2" => Some(json!({"title": "Archangel", "type": "rich"}).to_string()),
        "/embed/artist/UNKNOWN" => Some(page(json!({"type": "artist", "name": "Burial"}))),
        _ => None,
    }
}

async fn sources() -> Sources {
    Sources::new(&serve(musicbrainz).await, &serve(listenbrainz).await, &serve(spotify).await).unpaced()
}

fn titles(resolved: &Resolved) -> Vec<String> {
    match resolved {
        Resolved::Tracks { tracks, .. } => tracks.iter().map(|track| track.title.clone()).collect(),
        Resolved::Ask { .. } => panic!("asked: {resolved:?}"),
    }
}

#[tokio::test(flavor = "current_thread")]
async fn words_become_an_artists_an_albums_or_a_genres_tracks() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let sources = sources().await;
            // An artist (whose own titled album doesn't compete): their most listened recordings, one of each title.
            let Ok(Resolved::Tracks { name, kind, tracks, mbid }) = sources.resolve("Burial", 6, Signal::new()).await else { panic!() };
            assert_eq!((name.as_str(), kind, mbid.as_deref()), ("Burial", Kind::Artist, Some("burial")));
            let named: Vec<&str> = tracks.iter().map(|track| track.title.as_str()).collect();
            assert_eq!(named, ["Near Dark", "Untrue", "Archangel", "Distant Lights"]);
            assert_eq!((tracks[0].seconds, tracks[0].mbid.as_deref()), (Some(236.), Some("r-near-dark")));
            // A genre: its main artists' recordings.
            let Ok(Resolved::Tracks { kind, tracks, .. }) = sources.resolve("dub techno", 4, Signal::new()).await else { panic!() };
            assert_eq!(kind, Kind::Genre);
            assert_eq!(tracks.len(), 4);
            assert!(tracks.iter().any(|track| track.artist == "Deepchord"), "{tracks:?}");
            // An album everybody means (its artist far more listened to than the band by that name): its tracks.
            let found = sources.resolve("Discovery", 6, Signal::new()).await.unwrap();
            assert_eq!(titles(&found), ["One More Time", "Aerodynamic"]);
            let Resolved::Tracks { name, kind, mbid, .. } = found else { panic!() };
            assert_eq!((name.as_str(), kind, mbid.as_deref()), ("Discovery by Daft Punk", Kind::Album, Some(DISCOVERY)));
            // An album with its artist, either way round.
            for words in ["Discovery Daft Punk", "Discovery by Daft Punk", "Daft Punk - Discovery"] {
                assert_eq!(titles(&sources.resolve(words, 6, Signal::new()).await.unwrap()), ["One More Time", "Aerodynamic"], "{words}");
            }
            // Words that only look like a path are words.
            let found = sources.resolve(".38 Special", 6, Signal::new()).await.unwrap();
            assert_eq!(titles(&found), ["Hold On Loosely"]);
            assert!(matches!(sources.resolve("Music", 6, Signal::new()).await, Ok(Resolved::Ask { .. })));
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn unclear_words_ask_once_and_the_answer_settles_it() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let sources = sources().await;
            // Two artists and an album by one name, none far ahead: one question, with what each is.
            let Ok(Resolved::Ask { question, options }) = sources.resolve("Blue", 6, Signal::new()).await else { panic!() };
            assert!(question.contains("Which Blue"), "{question}");
            let labels: Vec<&str> = options.iter().map(|option| option.label.as_str()).collect();
            assert_eq!(
                labels,
                ["Blue (album by Joni Mitchell, 1971)", "Blue (UK boy band, from 2000)", "Blue (Scottish rock band, from 1973)"]
            );
            assert_eq!((options[1].kind, options[1].mbid.as_str(), options[1].what.clone()), ("artist", B1, format!("artist:{B1}")));
            // The answer: the boy band, whose own few tracks are filled out with its members'.
            let answered = sources.resolve(&options[1].what, 4, Signal::new()).await.unwrap();
            assert_eq!(titles(&answered), ["All Rise", "Lee A", "Lee B", "Duncan A"]);
            let Resolved::Tracks { kind, mbid, tracks, .. } = answered else { panic!() };
            assert_eq!((kind, mbid.as_deref(), tracks[1].artist.as_str()), (Kind::Artist, Some(B1), "Lee"));
            // Or the album, from its official release.
            let answered = sources.resolve(&options[0].what, 6, Signal::new()).await.unwrap();
            assert_eq!(titles(&answered), ["All I Want", "River"]);
            // Ids are checked; words nothing matches ask too.
            assert!(matches!(sources.resolve("artist:not-an-id", 6, Signal::new()).await, Ok(Resolved::Ask { .. })));
            let Ok(Resolved::Ask { question, .. }) = sources.resolve("zzqx", 6, Signal::new()).await else { panic!() };
            assert!(question.contains("couldn't place"), "{question}");
            // The tool hands the model the options with what to call it with.
            let place = tempfile::tempdir().unwrap();
            let store = Rc::new(ReferenceStore::new(place.path().join("kept")));
            let tool = ReferenceTool {
                fetcher: Rc::new(Fetcher::new(store.audio_folder(), ProgramOptions::default())),
                store,
                sources: Rc::new(sources),
                resolve: None,
            };
            let asked = tool.execute(json!({"what": "Blue"}).as_object().unwrap().clone(), Signal::new()).await.unwrap();
            let asked: Value = serde_json::from_str(&asked.text).unwrap();
            assert_eq!(
                asked["options"][1],
                json!({"label": "Blue (UK boy band, from 2000)", "kind": "artist", "mbid": B1, "what": format!("artist:{B1}")})
            );
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn links_are_taken_from_their_own_sites_only() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let sources = sources().await;
            // A Spotify album and track: their names, with lengths to match uploads by.
            let Ok(Resolved::Tracks { name, kind, tracks, .. }) =
                sources.resolve("https://open.spotify.com/album/2aBcD3?si=x", 6, Signal::new()).await
            else {
                panic!()
            };
            assert_eq!((name.as_str(), kind, tracks.len()), ("Untrue", Kind::Spotify, 2));
            assert_eq!((tracks[1].artist.as_str(), tracks[1].seconds), ("Burial", Some(236.)));
            let Ok(Resolved::Tracks { tracks, .. }) = sources.resolve("https://open.spotify.com/track/T1", 6, Signal::new()).await else {
                panic!()
            };
            assert_eq!((tracks[0].artist.as_str(), tracks[0].title.as_str(), tracks[0].seconds), ("Burial", "Near Dark", Some(236.)));
            // Its page's data gone: oEmbed's title stands in.
            let found = sources.resolve("https://open.spotify.com/track/T2", 6, Signal::new()).await.unwrap();
            assert_eq!(titles(&found), ["Archangel"]);
            // An artist's link, settled by MusicBrainz (no question), or by its name when MusicBrainz doesn't know it.
            for link in ["https://open.spotify.com/artist/SPBURIAL", "https://open.spotify.com/artist/UNKNOWN"] {
                let Ok(Resolved::Tracks { kind, mbid, .. }) = sources.resolve(link, 6, Signal::new()).await else { panic!("{link}") };
                assert_eq!((kind, mbid.as_deref()), (Kind::Artist, Some("burial")), "{link}");
            }
            // YouTube: a video or a playlist, as YouTube's own address.
            let Ok(Resolved::Tracks { kind, tracks, .. }) =
                sources.resolve("https://music.youtube.com/watch?v=dQw4w9WgXcQ&feature=share", 6, Signal::new()).await
            else {
                panic!()
            };
            assert_eq!((kind, tracks[0].url.as_deref()), (Kind::Video, Some("https://www.youtube.com/watch?v=dQw4w9WgXcQ")));
            let Ok(Resolved::Tracks { kind, tracks, .. }) =
                sources.resolve("https://www.youtube.com/playlist?list=PLx_y-1&si=2", 6, Signal::new()).await
            else {
                panic!()
            };
            assert_eq!((kind, tracks[0].url.as_deref()), (Kind::Playlist, Some("https://www.youtube.com/playlist?list=PLx_y-1")));
            // Anything else that only mentions those sites is refused.
            for link in [
                "https://evil.example/youtube.com/playlist.rss",
                "https://evil.example/open.spotify.com/track/T1",
                "https://www.youtube.com.evil.example/playlist?list=PL1",
                "https://www.youtube.com/playlist?list=--update-to=owner/repo",
                "http://192.168.1.2/youtube.com/playlist?list=PL1",
            ] {
                assert!(sources.resolve(link, 6, Signal::new()).await.is_err(), "{link}");
            }
        })
        .await;
}

#[test]
fn links_and_keys_read_as_they_should() {
    assert_eq!(
        spotify_parts("https://open.spotify.com/intl-de/track/4uLU6hMCjMI75M1A2tKUQC?si=1"),
        Some(("track", "4uLU6hMCjMI75M1A2tKUQC".into()))
    );
    assert_eq!(spotify_parts("https://open.spotify.com/show/4uLU6"), None);
    assert_eq!(spotify_parts("https://evil.example/open.spotify.com/track/4uLU6"), None);
    assert!(embed_entity("<html>no data</html>").is_none());
    assert_eq!(ReferenceStore::key("  Dub Techno! "), "dub-techno");
    assert_eq!(ReferenceStore::key("https://youtu.be/dQw4w9WgXcQ"), "https-youtu-be-dQw4w9WgXcQ");
}

#[tokio::test(flavor = "current_thread")]
async fn musicbrainz_is_asked_at_its_pace_and_again_when_busy_and_a_failed_genre_list_isnt_kept() {
    tokio::task::LocalSet::new()
        .run_until(async {
            // Paced: no two requests closer than the pace.
            let times = Rc::new(RefCell::new(vec![]));
            let heard = times.clone();
            let paced = serve_with(move |path| {
                heard.borrow_mut().push(Instant::now());
                musicbrainz(path).map_or((404, String::new()), |body| (200, body))
            })
            .await;
            let sources = Sources::new(&paced, &serve(listenbrainz).await, &serve(spotify).await).pace(Duration::from_millis(150));
            assert!(matches!(sources.resolve("dub techno", 2, Signal::new()).await, Ok(Resolved::Tracks { kind: Kind::Genre, .. })));
            let times = times.borrow().clone();
            assert!(times.len() >= 4, "{}", times.len());
            assert!(times.windows(2).all(|pair| pair[1] - pair[0] >= Duration::from_millis(100)), "{times:?}");
            // The genre list fails, then MusicBrainz is busy once, then it answers: the failure isn't kept, and the busy
            // answer is asked again.
            let asked = Rc::new(Cell::new(0));
            let count = asked.clone();
            let flaky = serve_with(move |path| {
                if path.starts_with("/ws/2/genre/all") {
                    count.set(count.get() + 1);
                    match count.get() {
                        1 => return (500, String::new()),
                        2 => return (503, String::new()),
                        _ => {}
                    }
                }
                musicbrainz(path).map_or((404, String::new()), |body| (200, body))
            })
            .await;
            let sources = Sources::new(&flaky, &serve(listenbrainz).await, &serve(spotify).await).unpaced();
            assert!(!matches!(sources.resolve("dub techno", 2, Signal::new()).await, Ok(Resolved::Tracks { kind: Kind::Genre, .. })));
            assert!(matches!(sources.resolve("dub techno", 2, Signal::new()).await, Ok(Resolved::Tracks { kind: Kind::Genre, .. })));
            assert_eq!(asked.get(), 3);
            // Once read, it's kept.
            sources.resolve("dub techno", 2, Signal::new()).await.unwrap();
            assert_eq!(asked.get(), 3);
            // Nobody answering: an error that says so, not a hang.
            let closed = {
                let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
                format!("http://{}", listener.local_addr().unwrap())
            };
            let offline = Sources::new(&closed, &closed, &closed).unpaced();
            let failed = offline.resolve("Burial", 6, Signal::new()).await.unwrap_err();
            assert!(failed.contains("didn't answer"), "{failed}");
        })
        .await;
}

#[test]
fn several_tracks_make_one_profile_with_the_range_they_keep_to() {
    let tracks: Vec<Profile> = [-7., -8., -9., -10., -11., -6.5].iter().map(|loudness| profile(*loudness)).collect();
    let combined = Profile::combine("dub techno", &tracks).unwrap();
    assert_eq!((combined.name.as_str(), combined.tracks), ("dub techno", 6));
    let loudness = combined.integrated.unwrap();
    // Six tracks: from the 10th to the 90th percentile of them, centred on their median.
    assert!(loudness.low < -10. && loudness.high > -7. && (loudness.mid + 8.5).abs() < 0.01, "{loudness:?}");
    // Two alike: never narrower than one track's own range.
    let alike = Profile::combine("two", &[profile(-8.), profile(-8.)]).unwrap();
    assert_eq!(alike.integrated.unwrap(), Spread { mid: -8., low: -9., high: -7. });
}

fn profile(loudness: f64) -> Profile {
    Profile {
        name: "t".into(),
        tracks: 1,
        regions: vec![Spread::point(-6., 1.)],
        integrated: Some(Spread::point(loudness, 1.)),
        plr: None,
        crest: Some(Spread::point(9., 1.5)),
        low_width: None,
        tilt: Spread::point(-3., 0.5),
        range: None,
        attack: None,
        decay: None,
        sustain: None,
        centroid: None,
        noise: None,
        sound: Default::default(),
    }
}

fn wav(path: &std::path::Path, seconds: f64, hz: f64, amplitude: f64) {
    let rate = 44_100u32;
    let frames = (seconds * rate as f64) as usize;
    let mut data = Vec::with_capacity(frames * 4);
    for n in 0..frames {
        let sample = (amplitude * (2. * std::f64::consts::PI * hz * n as f64 / rate as f64).sin() * 32767.) as i16;
        data.extend_from_slice(&sample.to_le_bytes());
        data.extend_from_slice(&sample.to_le_bytes());
    }
    let mut file = Vec::new();
    file.extend(b"RIFF");
    file.extend((36 + data.len() as u32).to_le_bytes());
    file.extend(b"WAVEfmt ");
    file.extend(16u32.to_le_bytes());
    file.extend(1u16.to_le_bytes());
    file.extend(2u16.to_le_bytes());
    file.extend(rate.to_le_bytes());
    file.extend((rate * 4).to_le_bytes());
    file.extend(4u16.to_le_bytes());
    file.extend(16u16.to_le_bytes());
    file.extend(b"data");
    file.extend((data.len() as u32).to_le_bytes());
    file.extend(data);
    std::fs::write(path, file).unwrap();
}

fn kept_reference(key: &str, name: &str, kind: &str, stamp: Option<String>) -> KeptReference {
    KeptReference {
        version: 1,
        key: key.into(),
        name: name.into(),
        kind: kind.into(),
        tracks: vec![KeptTrack { artist: "a".into(), title: "t".into(), source: "s".into(), mbid: Some("r-1".into()) }],
        profile: Profile::of("x", &measure_samples(&[0.1; 44_100], &[0.1; 44_100], 44_100.)),
        at: 0,
        mbid: None,
        stamp,
    }
}

#[tokio::test(flavor = "current_thread")]
async fn a_folders_finished_tracks_are_measured_kept_and_measured_again_once_they_change() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let place = tempfile::tempdir().unwrap();
            let folder = place.path().join("refs");
            std::fs::create_dir_all(folder.join("more.wav")).unwrap();
            wav(&folder.join("song.wav"), 31., 220., 0.3);
            wav(&folder.join("hit.wav"), 2., 440., 0.3);
            // What macOS leaves on shared drives, and a hidden file: never taken for tracks.
            std::fs::write(folder.join("._hit.wav"), b"\0\x05\x16\x07junk").unwrap();
            std::fs::write(folder.join(".song.wav"), b"junk").unwrap();
            let what = folder.to_string_lossy().to_string();
            let found = sources().await.resolve(&what, 6, Signal::new()).await.unwrap();
            assert_eq!(titles(&found), ["hit", "song"]);
            // Measured: the song, and the one-shot passed over with why.
            let store = Rc::new(ReferenceStore::new(place.path().join("kept")));
            let tool = reference_tools(store.clone(), ProgramOptions::default(), None).remove(0);
            let first = tool.execute(json!({"what": what}).as_object().unwrap().clone(), Signal::new()).await.unwrap();
            assert!(!first.is_error, "{}", first.text);
            let read: Value = serde_json::from_str(&first.text).unwrap();
            assert_eq!(read["tracks"], json!(["song"]));
            assert!(read["passedOver"][0].as_str().unwrap().starts_with("hit: 2 s long"), "{read}");
            assert_eq!(read["kept"], "measured now, kept for next time");
            assert!(read["note"].as_str().unwrap().contains(&what), "{read}");
            // Asked again, it's read back while its files are as they were; a folder isn't found by its name.
            let again = tool.execute(json!({"what": what}).as_object().unwrap().clone(), Signal::new()).await.unwrap();
            assert_eq!(serde_json::from_str::<Value>(&again.text).unwrap()["kept"], "measured before, read back");
            assert!(store.load("refs").await.is_none());
            std::fs::write(folder.join("._song.wav"), b"junk").unwrap();
            assert!(store.load(&what).await.is_some());
            wav(&folder.join("another.wav"), 1., 330., 0.3);
            assert!(store.load(&what).await.is_none());
            // One silent file the producer gives: refused, and said why (one short file is taken as it is).
            let take = place.path().join("take.wav");
            wav(&take, 2., 220., 0.);
            let refused = tool.execute(json!({"what": take.to_string_lossy()}).as_object().unwrap().clone(), Signal::new()).await.unwrap();
            assert!(refused.is_error && refused.text.contains("silent, nothing to measure"), "{}", refused.text);
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn kept_references_are_found_by_name_only_among_ones_made_from_words() {
    let place = tempfile::tempdir().unwrap();
    let folder = place.path().join("techno");
    std::fs::create_dir_all(&folder).unwrap();
    wav(&folder.join("a.wav"), 0.1, 220., 0.3);
    let store = ReferenceStore::new(place.path().join("kept"));
    let path = folder.to_string_lossy().to_string();
    store.save(&kept_reference(&ReferenceStore::key(&path), "techno", "files", stamp(&path))).await.unwrap();
    assert!(store.load(&path).await.is_some());
    assert!(store.load("techno").await.is_none());
    store.save(&kept_reference("tag-techno", "Techno", "genre", None)).await.unwrap();
    assert_eq!(store.load("TECHNO").await.unwrap().kind, "genre");
    // A kept reference survives as a file, MusicBrainz ids and all.
    let kept = kept_reference("x", "X", "artist", None);
    store.save(&kept).await.unwrap();
    assert_eq!(ReferenceStore::new(place.path().join("kept")).load("X").await.unwrap(), kept);
}

/// A stand-in yt-dlp: it writes each call's arguments to `args.log` (a call per block), answers searches and playlists
/// with the JSON beside it, and downloads by writing a small file where it's told (a WAV by its name, so nothing but
/// Kumi reads it; failing for the id FAILFAILFAI).
#[cfg(unix)]
fn fake_ytdlp(folder: &std::path::Path) -> String {
    use std::os::unix::fs::PermissionsExt;
    let script = folder.join("yt-dlp");
    let at = |name: &str| folder.join(name).to_string_lossy().to_string();
    std::fs::write(
        &script,
        format!(
            r#"#!/bin/sh
for a in "$@"; do printf '%s\n' "$a" >> '{log}'; done
printf '%s\n' '====' >> '{log}'
if [ "$1" = "--version" ]; then echo 2025.10.22; exit 0; fi
out=""; prev=""; last=""
for a in "$@"; do if [ "$prev" = "-o" ]; then out="$a"; fi; prev="$a"; last="$a"; done
case " $* " in *" --flat-playlist "*)
  case "$last" in ytsearch*) cat '{search}';; *) cat '{playlist}';; esac
  exit 0;;
esac
if [ -n "$out" ]; then
  case "$last" in *FAILFAILFAI*) printf x > "$(printf '%s' "$out" | sed 's/%(ext)s/wav.part/')"; exit 1;; esac
  printf audio > "$(printf '%s' "$out" | sed 's/%(ext)s/wav/')"
fi
exit 0
"#,
            log = at("args.log"),
            search = at("search.json"),
            playlist = at("playlist.json"),
        ),
    )
    .unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    script.to_string_lossy().to_string()
}

/// Each yt-dlp call's arguments (the version check left out).
#[cfg(unix)]
fn calls(folder: &std::path::Path) -> Vec<Vec<String>> {
    let log = std::fs::read_to_string(folder.join("args.log")).unwrap_or_default();
    log.split("====\n")
        .map(|call| call.lines().map(str::to_owned).collect::<Vec<_>>())
        .filter(|call| !call.is_empty() && call[0] != "--version")
        .collect()
}

#[cfg(unix)]
#[tokio::test(flavor = "current_thread")]
async fn audio_comes_from_youtube_videos_only_by_length_or_name_and_goes_once_measured() {
    use kumi_runtime::references::sources::Wanted;
    tokio::task::LocalSet::new()
        .run_until(async {
            let place = tempfile::tempdir().unwrap();
            let tools = place.path().join("tools");
            std::fs::create_dir_all(&tools).unwrap();
            let ytdlp = fake_ytdlp(&tools);
            let programs = ProgramOptions {
                env: Some([("KUMI_YTDLP".to_string(), ytdlp)].into_iter().collect()),
                tools_dir: tools.to_string_lossy().into(),
                ..Default::default()
            };
            let audio = place.path().join("audio");
            let fetcher = Fetcher::new(&audio, programs.clone());
            let video = |id: &str| format!("https://www.youtube.com/watch?v={id}");
            // A playlist: its YouTube videos no longer than a track; anything else in it is dropped.
            std::fs::write(
                tools.join("playlist.json"),
                json!({"entries": [
                    {"url": video("AAAAAAAAAAA"), "title": "One", "channel": "C", "duration": 200},
                    {"url": "--update-to=owner/repo", "title": "bad"},
                    {"url": "https://evil.example/watch?v=BBBBBBBBBBB", "title": "evil"},
                    {"url": "https://youtu.be/CCCCCCCCCCC", "title": "A long mix", "duration": 3600},
                    {"url": "https://youtu.be/DDDDDDDDDDD", "title": "Two", "duration": 180}
                ]})
                .to_string(),
            )
            .unwrap();
            let listed = fetcher.playlist("https://www.youtube.com/playlist?list=PL1", 3, Signal::new()).await.unwrap();
            let urls: Vec<Option<&str>> = listed.iter().map(|track| track.url.as_deref()).collect();
            assert_eq!(urls, [Some(video("AAAAAAAAAAA").as_str()), Some(video("DDDDDDDDDDD").as_str())]);
            // A track by its names: the upload closest in length, never an entry that isn't a YouTube video.
            std::fs::write(
                tools.join("search.json"),
                json!({"entries": [
                    {"url": video("EEEEEEEEEEE"), "title": "Burial - Near Dark (live)", "duration": 400},
                    {"url": video("FFFFFFFFFFF"), "title": "Near Dark", "channel": "Burial", "duration": 237},
                    {"url": "--exec=touch x", "title": "Near Dark Burial", "duration": 236}
                ]})
                .to_string(),
            )
            .unwrap();
            let near_dark = Wanted { artist: "Burial".into(), title: "Near Dark".into(), seconds: Some(236.), ..Default::default() };
            let got = fetcher.audio(&near_dark, Signal::new()).await.unwrap();
            assert_eq!((got.source.as_str(), got.fetched), (video("FFFFFFFFFFF").as_str(), true));
            assert!(got.file.ends_with("FFFFFFFFFFF.wav") && got.file.is_file(), "{got:?}");
            // Its length unknown: only an upload that names it, never just the first.
            std::fs::write(
                tools.join("search.json"),
                json!({"entries": [
                    {"url": video("GGGGGGGGGGG"), "title": "Something else"},
                    {"url": video("HHHHHHHHHHH"), "title": "Near Dark", "channel": "Burial"}
                ]})
                .to_string(),
            )
            .unwrap();
            let unknown = Wanted { seconds: None, ..near_dark.clone() };
            assert_eq!(fetcher.audio(&unknown, Signal::new()).await.unwrap().source, video("HHHHHHHHHHH"));
            let elsewhere = Wanted { title: "Untrue".into(), ..unknown.clone() };
            assert!(fetcher.audio(&elsewhere, Signal::new()).await.unwrap_err().contains("No upload named"));
            // Every call has `--` right before its address; downloads pass over long videos.
            let made = calls(&tools);
            for call in &made {
                let at = call.len() - 2;
                assert_eq!(call[at], "--", "{call:?}");
            }
            assert_eq!(made[0].last().unwrap(), "https://www.youtube.com/playlist?list=PL1");
            assert_eq!(made[1].last().unwrap(), "ytsearch5:Burial Near Dark");
            let download = made.iter().find(|call| call.contains(&"-f".to_string())).unwrap();
            assert!(download.windows(2).any(|pair| pair == ["--match-filter", "duration<1200"]), "{download:?}");
            // Downloaded once: a copy there is used again; a half-download isn't one.
            let before = calls(&tools).len();
            let again = Wanted { url: Some(video("FFFFFFFFFFF")), ..near_dark.clone() };
            assert_eq!(fetcher.audio(&again, Signal::new()).await.unwrap().file, got.file);
            assert_eq!(calls(&tools).len(), before);
            std::fs::write(audio.join("IIIIIIIIIII.m4a.part"), b"x").unwrap();
            assert!(kept(&audio, "IIIIIIIIIII").is_none());
            // A failed download leaves nothing behind; an address that isn't a YouTube video is never handed on.
            let failing = Wanted { url: Some(video("FAILFAILFAI")), ..near_dark.clone() };
            assert!(fetcher.audio(&failing, Signal::new()).await.is_err());
            assert!(!std::fs::read_dir(&audio).unwrap().any(|entry| entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with("FAILFAILFAI")));
            let before = calls(&tools).len();
            let smuggled = Wanted { url: Some("--update-to=owner/repo".into()), ..near_dark.clone() };
            assert!(fetcher.audio(&smuggled, Signal::new()).await.unwrap_err().contains("YouTube videos only"));
            assert_eq!(calls(&tools).len(), before);
            // Through the tool: fetched audio goes once it's been measured (here, found not to be audio at all).
            std::fs::remove_dir_all(&audio).unwrap();
            std::fs::write(
                tools.join("search.json"),
                json!({"entries": [{"url": video("JJJJJJJJJJJ"), "title": "Near Dark", "channel": "Burial", "duration": 236}]}).to_string(),
            )
            .unwrap();
            let store = Rc::new(ReferenceStore::new(place.path().join("kept")));
            let tool = ReferenceTool {
                fetcher: Rc::new(Fetcher::new(store.audio_folder(), programs)),
                store: store.clone(),
                sources: Rc::new(sources().await),
                resolve: None,
            };
            let measured = tool
                .execute(json!({"what": "https://open.spotify.com/track/T1"}).as_object().unwrap().clone(), Signal::new())
                .await
                .unwrap();
            assert!(measured.is_error && measured.text.contains("Burial – Near Dark"), "{}", measured.text);
            assert_eq!(std::fs::read_dir(store.audio_folder()).unwrap().count(), 0);
        })
        .await;
}
