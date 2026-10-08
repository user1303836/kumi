//! References for the judge: words, links and files resolved to example tracks (MusicBrainz, ListenBrainz and Spotify
//! stood in for by a local server), one question when words could mean more than one thing, profiles with spread, kept.
use kumi_common::abort::Signal;
use kumi_runtime::{
    listening::{
        checklist::{Profile, Spread},
        measure::measure_samples,
    },
    references::{
        sources::{embed_entity, spotify_parts, Kind, Resolved, Sources},
        store::{KeptReference, KeptTrack, ReferenceStore},
        tool::reference_tools,
    },
    video::programs::ProgramOptions,
};
use serde_json::{json, Value};
use std::rc::Rc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// A server answering each request's path (and query) with what `answer` gives: a body, JSON when it parses.
async fn serve(answer: fn(&str) -> Option<String>) -> String {
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
            let path = request.split_whitespace().nth(1).unwrap_or("/").to_string();
            let reply = match answer(&path) {
                Some(body) => format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()),
                None => "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".into(),
            };
            let _ = socket.write_all(reply.as_bytes()).await;
        }
    });
    format!("http://{address}")
}

/// An artist's albums (by how many editions each has) and each album's tracks: (album id, editions, [(recording,
/// title, seconds)]).
fn discography(artist: &str) -> Vec<(&'static str, f64, Vec<(&'static str, &'static str, f64)>)> {
    match artist {
        "burial" => vec![
            ("untrue", 30., vec![("r-archangel", "Archangel", 239.), ("r-near-dark", "Near Dark", 236.), ("r-untrue", "Untrue", 371.)]),
            ("burial", 20., vec![("r-distant", "Distant Lights", 340.), ("r-archangel-2", "Archangel (Remastered)", 239.)]),
        ],
        "bc" => vec![("bc-album", 10., vec![("r-phylyps", "Phylyps Trak", 444.), ("r-q11", "Q1.1", 400.)])],
        "dc" => vec![("dc-album", 8., vec![("r-vantage", "Vantage Isle", 600.), ("r-sommerset", "Sommerset", 480.)])],
        _ => vec![],
    }
}

fn musicbrainz(path: &str) -> Option<String> {
    let path = path.replace("%22", "\"").replace("%3A", ":").replace("%20", " ");
    if path.starts_with("/ws/2/genre/all") {
        return Some("dub\ndub techno\ntechno\n".into());
    }
    if path.starts_with("/ws/2/release-group?query=arid:") {
        let artist = path["/ws/2/release-group?query=arid:".len()..].split(' ').next().unwrap_or("");
        let groups: Vec<Value> =
            discography(artist).iter().map(|(id, count, _)| json!({"id": id, "count": count, "secondary-types": []})).collect();
        return Some(json!({"release-groups": groups}).to_string());
    }
    if let Some(group) = path.strip_prefix("/ws/2/release?release-group=") {
        let group = group.split('&').next().unwrap_or("");
        let tracks: Vec<Value> = ["burial", "bc", "dc"]
            .iter()
            .flat_map(|artist| discography(artist))
            .filter(|(id, _, _)| *id == group)
            .flat_map(|(_, _, tracks)| tracks)
            .map(|(id, title, seconds)| json!({"title": title, "length": seconds * 1000., "recording": {"id": id, "title": title}}))
            .collect();
        return Some(json!({"releases": [{"media": [{"tracks": tracks}]}]}).to_string());
    }
    if path.contains("tag:\"dub techno\"") {
        return Some(
            json!({"artists": [{"id": "bc", "name": "Basic Channel", "score": 100}, {"id": "dc", "name": "Deepchord", "score": 90}]})
                .to_string(),
        );
    }
    if path.contains("artist:\"Burial\"") {
        return Some(json!({"artists": [{"id": "burial", "name": "Burial", "score": 100, "disambiguation": "UK producer"}, {"id": "burial-2", "name": "Burial", "score": 100, "disambiguation": "punk band"}]}).to_string());
    }
    if path.contains("artist:\"Blue\"") {
        return Some(
            json!({"artists": [
                {"id": "b1", "name": "Blue", "score": 100, "disambiguation": "UK boy band", "life-span": {"begin": "2000"}},
                {"id": "b2", "name": "Blue", "score": 98, "disambiguation": "Scottish rock band", "life-span": {"begin": "1973"}}
            ]})
            .to_string(),
        );
    }
    if path.starts_with("/ws/2/artist") {
        return Some(json!({"artists": [{"id": "x", "name": "Something Else", "score": 40}]}).to_string());
    }
    if path.starts_with("/ws/2/release-group") {
        return Some(json!({"release-groups": []}).to_string());
    }
    None
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
                {"artist_mbid": "b1", "total_user_count": 3000},
                {"artist_mbid": "b2", "total_user_count": 2000}
            ])
            .to_string(),
        ),
        _ => None,
    }
}

fn spotify(path: &str) -> Option<String> {
    let data = json!({"props": {"pageProps": {"state": {"data": {"entity": {
        "type": "album", "name": "Untrue",
        "trackList": [
            {"title": "Archangel", "subtitle": "Burial", "duration": 239000},
            {"title": "Near Dark", "subtitle": "Burial", "duration": 236000}
        ]
    }}}}}});
    path.starts_with("/embed/album/")
        .then(|| format!("<html><script id=\"__NEXT_DATA__\" type=\"application/json\">{data}</script></html>"))
}

async fn sources() -> Sources {
    Sources::new(&serve(musicbrainz).await, &serve(listenbrainz).await, &serve(spotify).await).unpaced()
}

#[tokio::test(flavor = "current_thread")]
async fn words_become_an_artists_or_a_genres_tracks_and_unclear_words_one_question() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let sources = sources().await;
            // An artist: their most listened recordings, one of each title.
            let Ok(Resolved::Tracks { name, kind, tracks }) = sources.resolve("Burial", 6, Signal::new()).await else { panic!() };
            assert_eq!((name.as_str(), kind), ("Burial", Kind::Artist));
            // Their albums' tracks, the most listened first, one version of each.
            let titles: Vec<&str> = tracks.iter().map(|track| track.title.as_str()).collect();
            assert_eq!(titles, ["Near Dark", "Untrue", "Archangel", "Distant Lights"]);
            assert_eq!(tracks[0].seconds, Some(236.));
            // A genre: its main artists' recordings.
            let Ok(Resolved::Tracks { kind, tracks, .. }) = sources.resolve("dub techno", 4, Signal::new()).await else { panic!() };
            assert_eq!(kind, Kind::Genre);
            assert_eq!(tracks.len(), 4);
            assert!(tracks.iter().any(|track| track.artist == "Deepchord"), "{tracks:?}");
            // Two artists by one name: one question, with who each is.
            let Ok(Resolved::Ask { question, options }) = sources.resolve("Blue", 6, Signal::new()).await else { panic!() };
            assert!(question.contains("Which Blue"), "{question}");
            assert_eq!(options, ["Blue (UK boy band, from 2000)", "Blue (Scottish rock band, from 1973)"]);
            // Words nothing matches: one question too.
            let Ok(Resolved::Ask { question, .. }) = sources.resolve("zzqx", 6, Signal::new()).await else { panic!() };
            assert!(question.contains("couldn't place"), "{question}");
            // A Spotify album: its tracks' names, with their lengths to match uploads by.
            let Ok(Resolved::Tracks { name, kind, tracks }) =
                sources.resolve("https://open.spotify.com/album/2aBcD3?si=x", 6, Signal::new()).await
            else {
                panic!()
            };
            assert_eq!((name.as_str(), kind, tracks.len()), ("Untrue", Kind::Spotify, 2));
            assert_eq!((tracks[1].artist.as_str(), tracks[1].seconds), ("Burial", Some(236.)));
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
    assert!(embed_entity("<html>no data</html>").is_none());
    assert_eq!(ReferenceStore::key("  Dub Techno! "), "dub-techno");
    assert_eq!(ReferenceStore::key("https://youtu.be/dQw4w9WgXcQ"), "https-youtu-be-dQw4w9WgXcQ");
}

#[test]
fn several_tracks_make_one_profile_with_the_range_they_keep_to() {
    let one = |loudness: f64, tilt: f64| Profile {
        name: "t".into(),
        tracks: 1,
        regions: vec![Spread::point(-6., 1.)],
        integrated: Some(Spread::point(loudness, 1.)),
        plr: None,
        crest: Some(Spread::point(9., 1.5)),
        low_width: None,
        tilt: Spread::point(tilt, 0.5),
        range: None,
        attack: None,
        decay: None,
        sustain: None,
        centroid: None,
        noise: None,
    };
    let tracks: Vec<Profile> = [-7., -8., -9., -10., -11., -6.5].iter().map(|loudness| one(*loudness, -3.)).collect();
    let combined = Profile::combine("dub techno", &tracks).unwrap();
    assert_eq!((combined.name.as_str(), combined.tracks), ("dub techno", 6));
    let loudness = combined.integrated.unwrap();
    // Six tracks: from the 10th to the 90th percentile of them, centred on their median.
    assert!(loudness.low < -10. && loudness.high > -7. && (loudness.mid + 8.5).abs() < 0.01, "{loudness:?}");
    // Two alike: never narrower than one track's own range.
    let alike = Profile::combine("two", &[one(-8., -3.), one(-8., -3.)]).unwrap();
    assert_eq!(alike.integrated.unwrap(), Spread { mid: -8., low: -9., high: -7. });
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

#[tokio::test(flavor = "current_thread")]
async fn a_folder_is_measured_into_a_kept_profile_and_read_back_next_time() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let place = tempfile::tempdir().unwrap();
            let folder = place.path().join("refs");
            std::fs::create_dir_all(&folder).unwrap();
            for (index, amplitude) in [0.2, 0.3, 0.4].iter().enumerate() {
                wav(&folder.join(format!("take {index}.wav")), 4., 220. * (index + 1) as f64, *amplitude);
            }
            let store = Rc::new(ReferenceStore::new(place.path().join("kept")));
            let tool = reference_tools(store.clone(), ProgramOptions::default(), None).remove(0);
            let what = folder.to_string_lossy().to_string();
            let first = tool.execute(json!({"what": what}).as_object().unwrap().clone(), Signal::new()).await.unwrap();
            assert!(!first.is_error, "{}", first.text);
            let read: Value = serde_json::from_str(&first.text).unwrap();
            assert_eq!(read["tracks"].as_array().unwrap().len(), 3);
            assert_eq!(read["kept"], "measured now, kept for next time");
            assert!(read["measures"]["loudness"].as_str().unwrap().contains("LUFS"), "{read}");
            // Asked again (by what it was, or by its name), it's read back, not measured.
            let again = tool.execute(json!({"what": what}).as_object().unwrap().clone(), Signal::new()).await.unwrap();
            assert_eq!(serde_json::from_str::<Value>(&again.text).unwrap()["kept"], "measured before, read back");
            assert!(store.load("refs").await.is_some());
            // A kept reference survives as a file.
            let kept = KeptReference {
                version: 1,
                key: "x".into(),
                name: "X".into(),
                kind: "artist".into(),
                tracks: vec![KeptTrack { artist: "a".into(), title: "t".into(), source: "s".into() }],
                profile: Profile::of("x", &measure_samples(&[0.1; 44_100], &[0.1; 44_100], 44_100.)),
                at: 0,
            };
            store.save(&kept).await.unwrap();
            assert_eq!(ReferenceStore::new(place.path().join("kept")).load("X").await.unwrap(), kept);
        })
        .await;
}
