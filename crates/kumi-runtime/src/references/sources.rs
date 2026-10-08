//! What a reference is, as tracks to fetch: a file or a folder of them, a YouTube video or playlist, a Spotify link
//! (its names; the audio comes from a YouTube search), or words: a genre (MusicBrainz's genres), an artist (their most
//! listened recordings, from ListenBrainz) or an album. Words that could mean more than one thing come back as one
//! question, whose options name exactly what each is (`artist:<id>`, `album:<id>`), so the answer settles it.

use kumi_common::abort::Signal;
use serde::Serialize;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    cell::{Cell, RefCell},
    collections::HashMap,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

/// One example track to measure.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Wanted {
    pub artist: String,
    pub title: String,
    /// Its length when known, to pick the right upload.
    pub seconds: Option<f64>,
    /// A video to take the audio from as it is.
    pub url: Option<String>,
    pub file: Option<PathBuf>,
    /// Its MusicBrainz recording, when it came from there.
    pub mbid: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Files,
    Video,
    Playlist,
    Spotify,
    Artist,
    Album,
    Genre,
}

impl Kind {
    /// Made from words (or a link that names music, not audio): such a reference can be found again by its name.
    pub fn named(kind: &str) -> bool {
        matches!(kind, "spotify" | "artist" | "album" | "genre")
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Resolved {
    Tracks {
        name: String,
        kind: Kind,
        tracks: Vec<Wanted>,
        /// The artist or album on MusicBrainz, when it's one.
        mbid: Option<String>,
    },
    /// The words could mean more than one thing: one question for the producer, with what it could be.
    Ask { question: String, options: Vec<Choice> },
}

/// One answer to a question: what the producer is shown, and what `reference` is called with once they pick it.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Choice {
    pub label: String,
    /// "artist" or "album".
    pub kind: &'static str,
    pub mbid: String,
    /// `artist:<mbid>` or `album:<mbid>`.
    pub what: String,
}

impl Choice {
    fn new(kind: &'static str, mbid: &str, label: String) -> Self {
        Self { label, kind, mbid: mbid.into(), what: format!("{kind}:{mbid}") }
    }
}

const AUDIO: [&str; 7] = ["wav", "aif", "aiff", "flac", "mp3", "m4a", "ogg"];
/// How many people must listen to the one meant, and how many times more than to the next, to take it without asking.
const CLEAR_LISTENERS: f64 = 100.;
const CLEAR_RATIO: f64 = 10.;

pub struct Sources {
    client: reqwest::Client,
    musicbrainz: String,
    listenbrainz: String,
    spotify: String,
    /// When MusicBrainz was last asked: it takes one request a second.
    last: Cell<Option<Instant>>,
    pace: Duration,
    /// MusicBrainz's genres, once they're read (a failed read isn't kept: the next asks again).
    genres: RefCell<Option<Vec<String>>>,
}

impl Default for Sources {
    fn default() -> Self {
        Self::new("https://musicbrainz.org", "https://api.listenbrainz.org", "https://open.spotify.com")
    }
}

impl Sources {
    /// With other servers (for tests).
    pub fn new(musicbrainz: &str, listenbrainz: &str, spotify: &str) -> Self {
        let client = reqwest::Client::builder()
            .user_agent(format!("Kumi/{} ( https://github.com/user1303836/kumi )", crate::KUMI_VERSION))
            .timeout(Duration::from_secs(20))
            .build()
            .expect("an HTTP client");
        Self {
            client,
            musicbrainz: musicbrainz.trim_end_matches('/').into(),
            listenbrainz: listenbrainz.trim_end_matches('/').into(),
            spotify: spotify.trim_end_matches('/').into(),
            last: Cell::new(None),
            pace: Duration::from_millis(1100),
            genres: Default::default(),
        }
    }
    /// MusicBrainz asked at most once every `every` (and again after a wait half as long again when it's busy).
    pub fn pace(mut self, every: Duration) -> Self {
        self.pace = every;
        self
    }
    /// Without waiting between requests (for a server that isn't MusicBrainz's).
    pub fn unpaced(self) -> Self {
        self.pace(Duration::ZERO)
    }

    /// The tracks a reference stands for (at most `count`), or the one question that settles what it is.
    pub async fn resolve(&self, what: &str, count: usize, signal: Signal) -> Result<Resolved, String> {
        let what = what.trim().trim_matches(|c| c == '"' || c == '“' || c == '”');
        if what.is_empty() {
            return Err("Say what the reference is: a file, a folder, a YouTube or Spotify link, or an artist, album or genre.".into());
        }
        // A question's answer: exactly the artist or album picked.
        if let Some((kind, mbid)) = picked(what) {
            return if kind == "artist" {
                self.artist_by_id(&mbid, count, &signal).await
            } else {
                self.album_by_id(&mbid, count, &signal).await
            };
        }
        if what.starts_with("http://") || what.starts_with("https://") {
            return match link(what) {
                Some(Link::Spotify(kind, id)) => self.spotify(kind, &id, count, signal).await,
                Some(Link::Video(url)) => Ok(Resolved::Tracks {
                    name: what.into(),
                    kind: Kind::Video,
                    tracks: vec![Wanted { title: what.into(), url: Some(url), ..Default::default() }],
                    mbid: None,
                }),
                Some(Link::Playlist(url)) => Ok(Resolved::Tracks {
                    name: what.into(),
                    kind: Kind::Playlist,
                    tracks: vec![Wanted { title: what.into(), url: Some(url), ..Default::default() }],
                    mbid: None,
                }),
                None => Err("Kumi takes YouTube and Spotify links for references, or an audio file or folder.".into()),
            };
        }
        if looks_like_path(what) {
            return files(&crate::audio::audio_path(what), count);
        }
        // An audio file's name, or a Windows path, that isn't there is said so, not searched for as words (a name with
        // a slash, AC/DC say, still is).
        if audio(Path::new(what.trim())) || what.contains('\\') {
            return Err(format!("There's no such file: {}.", crate::audio::audio_path(what)));
        }
        self.words(what, count, signal).await
    }

    async fn get(&self, url: &str, signal: &Signal) -> Result<Value, String> {
        let text = self.text(url, signal).await?;
        serde_json::from_str(&text).map_err(|_| format!("{} didn't answer with JSON.", host(url)))
    }

    /// A page's text; MusicBrainz asked at its pace, and again (up to three times) when it says it's too busy.
    async fn text(&self, url: &str, signal: &Signal) -> Result<String, String> {
        for attempt in 0..3u32 {
            match self.text_once(url, signal).await {
                Err(why) if why.ends_with("503 Service Unavailable") && attempt < 2 => {
                    let wait = self.pace.max(Duration::from_millis(10)) * 3 * (attempt + 1) / 2;
                    tokio::select! {
                        _ = tokio::time::sleep(wait) => {}
                        _ = signal.cancelled() => return Err("stopped".into()),
                    }
                }
                done => return done,
            }
        }
        unreachable!()
    }

    async fn text_once(&self, url: &str, signal: &Signal) -> Result<String, String> {
        if url.starts_with(&self.musicbrainz) {
            if let Some(last) = self.last.get() {
                let wait = self.pace.saturating_sub(last.elapsed());
                if !wait.is_zero() {
                    tokio::select! {
                        _ = tokio::time::sleep(wait) => {}
                        _ = signal.cancelled() => return Err("stopped".into()),
                    }
                }
            }
            self.last.set(Some(Instant::now()));
        }
        let sent = tokio::select! {
            sent = self.client.get(url).header("Accept", "application/json").send() => sent.map_err(|error| format!("{} didn't answer: {error}", host(url)))?,
            _ = signal.cancelled() => return Err("stopped".into()),
        };
        if !sent.status().is_success() {
            return Err(format!("{} answered {}", host(url), sent.status()));
        }
        // A page is read up to 8 MB: a longer answer is refused rather than held in memory.
        tokio::select! {
            body = crate::web::net::body_at_most(sent, 8_000_000) => body
                .map(|body| String::from_utf8_lossy(&body).into_owned())
                .map_err(|why| format!("{}: {why}", host(url))),
            _ = signal.cancelled() => Err("stopped".into()),
        }
    }

    /// MusicBrainz's genres: read once, and kept only once they're read.
    async fn genres(&self, signal: &Signal) -> Vec<String> {
        if let Some(known) = self.genres.borrow().clone() {
            return known;
        }
        let Ok(text) = self.text(&format!("{}/ws/2/genre/all?fmt=txt", self.musicbrainz), signal).await else {
            return vec![];
        };
        let listed: Vec<String> = text.lines().map(|line| line.trim().to_lowercase()).filter(|line| !line.is_empty()).collect();
        if !listed.is_empty() {
            *self.genres.borrow_mut() = Some(listed.clone());
        }
        listed
    }

    async fn words(&self, words: &str, count: usize, signal: Signal) -> Result<Resolved, String> {
        let lower = words.to_lowercase();
        if self.genres(&signal).await.contains(&lower) {
            return self.genre(words, count, &signal).await;
        }
        let found =
            self.get(&format!("{}/ws/2/artist?query={}&fmt=json&limit=25", self.musicbrainz, query("artist", words)), &signal).await?;
        let artists: Vec<Value> = found["artists"].as_array().cloned().unwrap_or_default();
        let score = |value: &Value| value["score"].as_f64().unwrap_or(0.);
        // Artists by that very name, and albums by that very title (each album by someone else: an artist's own
        // titled album is the artist).
        let named: Vec<&Value> = artists
            .iter()
            .filter(|artist| score(artist) >= 76. && folded(artist["name"].as_str().unwrap_or("")) == folded(words))
            .collect();
        let albums = format!("{} AND primarytype:album", query_raw("releasegroup", words));
        let found =
            self.get(&format!("{}/ws/2/release-group?query={}&fmt=json&limit=10", self.musicbrainz, url_encode(&albums)), &signal).await?;
        let groups: Vec<Value> = found["release-groups"].as_array().cloned().unwrap_or_default();
        let titled: Vec<&Value> = groups
            .iter()
            .filter(|group| {
                score(group) >= 90.
                    && folded(group["title"].as_str().unwrap_or("")) == folded(words)
                    && group["secondary-types"].as_array().is_none_or(|kinds| kinds.is_empty())
                    && !named.iter().any(|artist| artist["id"] == album_artist(group)["id"])
            })
            .collect();
        let mut candidates: Vec<(&'static str, &Value)> =
            named.iter().map(|artist| ("artist", *artist)).chain(titled.iter().map(|album| ("album", *album))).collect();
        if candidates.len() > 1 {
            // The one everybody means stands well clear of the others: ten times the listeners of the next (an album
            // by its artist's). Close ones need a question.
            let everyone: Vec<Value> = candidates
                .iter()
                .map(|(kind, value)| if *kind == "artist" { (*value).clone() } else { album_artist(value).clone() })
                .collect();
            let listeners = self.listeners(&everyone, &signal).await;
            let heard = |(kind, value): &(&str, &Value)| {
                let artist = if *kind == "artist" { *value } else { album_artist(value) };
                artist["id"].as_str().and_then(|id| listeners.get(id)).copied().unwrap_or(0.)
            };
            candidates.sort_by(|a, b| heard(b).total_cmp(&heard(a)));
            if !(heard(&candidates[0]) >= CLEAR_LISTENERS && heard(&candidates[0]) >= heard(&candidates[1]) * CLEAR_RATIO) {
                return Ok(Resolved::Ask {
                    question: format!("Which {words} do you mean?"),
                    options: candidates.iter().take(5).map(|(kind, value)| choice(kind, value)).collect(),
                });
            }
        }
        match candidates.first() {
            Some(("artist", artist)) => return self.artist(artist, count, &signal).await,
            Some((_, album)) => return self.album(album, count, &signal).await,
            None => {}
        }
        // An album and its artist in one ("Discovery Daft Punk", "Discovery by Daft Punk").
        if words.split_whitespace().count() > 1 {
            let terms = words.split_whitespace().map(folded).filter(|word| !word.is_empty() && word != "by").collect::<Vec<_>>();
            let both = format!("releasegroup:({0}) AND artist:({0}) AND primarytype:album", terms.join(" "));
            let found = self
                .get(&format!("{}/ws/2/release-group?query={}&fmt=json&limit=10", self.musicbrainz, url_encode(&both)), &signal)
                .await?;
            // All the words, with or without a "by" between: the title and the artist, either way round.
            let wholes = [terms.concat(), folded(words)];
            let matched = found["release-groups"].as_array().into_iter().flatten().find(|group| {
                let title = folded(group["title"].as_str().unwrap_or(""));
                let artist = folded(album_artist(group)["name"].as_str().unwrap_or(""));
                !title.is_empty()
                    && !artist.is_empty()
                    && wholes.iter().any(|whole| *whole == format!("{title}{artist}") || *whole == format!("{artist}{title}"))
            });
            if let Some(album) = matched {
                return self.album(album, count, &signal).await;
            }
        }
        if let Some(artist) = artists.first().filter(|artist| score(artist) >= 98.) {
            return self.artist(artist, count, &signal).await;
        }
        Ok(Resolved::Ask {
            question: format!("Kumi couldn't place “{words}”: which artist, album or genre is it (a track or a link works too)?"),
            options: artists.iter().take(3).map(|artist| choice("artist", artist)).collect(),
        })
    }

    /// The artist a question's answer picked.
    async fn artist_by_id(&self, mbid: &str, count: usize, signal: &Signal) -> Result<Resolved, String> {
        let artist = self.get(&format!("{}/ws/2/artist/{mbid}?fmt=json", self.musicbrainz), signal).await?;
        if artist["name"].as_str().is_none() {
            return Err(format!("MusicBrainz has no artist {mbid}."));
        }
        self.artist(&artist, count, signal).await
    }

    /// The album a question's answer picked.
    async fn album_by_id(&self, mbid: &str, count: usize, signal: &Signal) -> Result<Resolved, String> {
        let group =
            self.get(&format!("{}/ws/2/release-group/{mbid}?inc=releases+artist-credits&fmt=json", self.musicbrainz), signal).await?;
        if group["title"].as_str().is_none() {
            return Err(format!("MusicBrainz has no album {mbid}."));
        }
        self.album(&group, count, signal).await
    }

    /// An artist's most listened recordings; a collective's own too few, its members' besides.
    async fn artist(&self, artist: &Value, count: usize, signal: &Signal) -> Result<Resolved, String> {
        let name = artist["name"].as_str().unwrap_or("").to_string();
        let id = artist["id"].as_str().unwrap_or("").to_string();
        let mut tracks = self.top_recordings(&id, &name, count, signal).await?;
        if tracks.len() < count {
            for member in self.members(&id, signal).await.iter().take(4) {
                let (Some(member_id), Some(member_name)) = (member["id"].as_str(), member["name"].as_str()) else { continue };
                let Ok(theirs) = self.top_recordings(member_id, member_name, 2, signal).await else { continue };
                for track in theirs {
                    if tracks.len() < count && !tracks.iter().any(|kept: &Wanted| base_title(&kept.title) == base_title(&track.title)) {
                        tracks.push(track);
                    }
                }
                if tracks.len() >= count {
                    break;
                }
            }
        }
        if tracks.is_empty() {
            return Err(format!("Kumi found {name} but no recordings to listen to; give a track or a link."));
        }
        Ok(Resolved::Tracks { name, kind: Kind::Artist, tracks, mbid: Some(id).filter(|id| !id.is_empty()) })
    }

    /// A group's members, from MusicBrainz (none when it has none, or doesn't answer).
    async fn members(&self, id: &str, signal: &Signal) -> Vec<Value> {
        if id.is_empty() {
            return vec![];
        }
        let Ok(read) = self.get(&format!("{}/ws/2/artist/{id}?inc=artist-rels&fmt=json", self.musicbrainz), signal).await else {
            return vec![];
        };
        read["relations"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|relation| relation["type"] == "member of band" && relation["direction"] == "backward")
            .map(|relation| relation["artist"].clone())
            .filter(|artist| artist["id"].is_string())
            .collect()
    }

    /// An artist's best-known recordings, with no sign-in anywhere: their studio albums with the most editions
    /// (MusicBrainz), those albums' tracks, ranked by how many times people listened to them (ListenBrainz); without
    /// listens, the albums' tracks in turn.
    async fn top_recordings(&self, id: &str, name: &str, count: usize, signal: &Signal) -> Result<Vec<Wanted>, String> {
        let albums = format!("arid:{id} AND primarytype:album AND status:official");
        let found =
            self.get(&format!("{}/ws/2/release-group?query={}&fmt=json&limit=100", self.musicbrainz, url_encode(&albums)), signal).await?;
        let mut groups: Vec<&Value> = found["release-groups"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|group| group["secondary-types"].as_array().is_none_or(|kinds| kinds.is_empty()))
            .collect();
        groups.sort_by(|a, b| b["count"].as_f64().unwrap_or(0.).total_cmp(&a["count"].as_f64().unwrap_or(0.)));
        // Each album's tracks: (recording id, title, seconds), album by album.
        let mut albums: Vec<Vec<(String, String, Option<f64>)>> = vec![];
        for group in groups.iter().take(count.clamp(3, 5)) {
            let Some(group) = group["id"].as_str() else { continue };
            let address =
                format!("{}/ws/2/release?release-group={group}&inc=recordings&status=official&limit=1&fmt=json", self.musicbrainz);
            let Ok(read) = self.get(&address, signal).await else { continue };
            let tracks: Vec<(String, String, Option<f64>)> = read["releases"][0]["media"]
                .as_array()
                .into_iter()
                .flatten()
                .flat_map(|medium| medium["tracks"].as_array().cloned().unwrap_or_default())
                .filter_map(|track| {
                    let recording = &track["recording"];
                    Some((
                        recording["id"].as_str()?.to_string(),
                        track["title"].as_str().or(recording["title"].as_str())?.to_string(),
                        track["length"].as_f64().or(recording["length"].as_f64()).map(|ms| ms / 1000.),
                    ))
                })
                .collect();
            if !tracks.is_empty() {
                albums.push(tracks);
            }
        }
        let ids: Vec<&str> = albums.iter().flatten().map(|(id, _, _)| id.as_str()).collect();
        let listens = self.listens(&ids, signal).await;
        let mut ranked: Vec<&(String, String, Option<f64>)> = if listens.values().any(|count| *count > 0.) {
            let mut all: Vec<&(String, String, Option<f64>)> = albums.iter().flatten().collect();
            all.sort_by(|a, b| listens.get(&b.0).copied().unwrap_or(0.).total_cmp(&listens.get(&a.0).copied().unwrap_or(0.)));
            all
        } else {
            // No listens to go by: the albums' tracks in turn, first track first.
            let most = albums.iter().map(Vec::len).max().unwrap_or(0);
            (0..most).flat_map(|at| albums.iter().filter_map(move |album| album.get(at))).collect()
        };
        ranked.retain(|(_, title, _)| !title.is_empty() && !title.starts_with('[') && !title.to_lowercase().contains("remix"));
        let mut tracks: Vec<Wanted> = vec![];
        let mut seen: Vec<String> = vec![];
        for (recording, title, seconds) in ranked {
            let key = base_title(title);
            if seen.contains(&key) {
                continue;
            }
            seen.push(key);
            tracks.push(Wanted {
                artist: name.into(),
                title: title.clone(),
                seconds: *seconds,
                mbid: Some(recording.clone()),
                ..Default::default()
            });
            if tracks.len() >= count {
                break;
            }
        }
        Ok(tracks)
    }

    /// How many times people listened to each recording, from ListenBrainz (none when it doesn't answer).
    async fn listens(&self, ids: &[&str], signal: &Signal) -> HashMap<String, f64> {
        if ids.is_empty() {
            return Default::default();
        }
        let rows = self.popularity("recording", serde_json::json!({"recording_mbids": ids}), signal).await;
        rows.as_array()
            .into_iter()
            .flatten()
            .filter_map(|row| Some((row["recording_mbid"].as_str()?.to_string(), row["total_listen_count"].as_f64().unwrap_or(0.))))
            .collect()
    }

    /// How many people listen to each artist, from ListenBrainz (none when it doesn't answer).
    async fn listeners(&self, artists: &[Value], signal: &Signal) -> HashMap<String, f64> {
        let ids: Vec<&str> = artists.iter().filter_map(|artist| artist["id"].as_str()).collect();
        if ids.is_empty() {
            return Default::default();
        }
        let rows = self.popularity("artist", serde_json::json!({"artist_mbids": ids}), signal).await;
        rows.as_array()
            .into_iter()
            .flatten()
            .filter_map(|row| Some((row["artist_mbid"].as_str()?.to_string(), row["total_user_count"].as_f64().unwrap_or(0.))))
            .collect()
    }

    /// ListenBrainz's popularity rows for recordings or artists (none when it doesn't answer).
    async fn popularity(&self, of: &str, body: Value, signal: &Signal) -> Value {
        let asked = async {
            let sent = self.client.post(format!("{}/1/popularity/{of}", self.listenbrainz)).json(&body).send().await.ok()?;
            sent.json::<Value>().await.ok()
        };
        tokio::select! {
            rows = asked => rows.unwrap_or_default(),
            _ = signal.cancelled() => Value::Null,
        }
    }

    /// A genre: the artists most tied to it (how often they're tagged with it, how well they match, and how many people
    /// listen to them), a couple of each one's most listened recordings.
    async fn genre(&self, genre: &str, count: usize, signal: &Signal) -> Result<Resolved, String> {
        let found = self.get(&format!("{}/ws/2/artist?query={}&fmt=json&limit=25", self.musicbrainz, query("tag", genre)), signal).await?;
        let artists: Vec<Value> = found["artists"].as_array().cloned().unwrap_or_default();
        let listeners = self.listeners(&artists, signal).await;
        let lower = genre.to_lowercase();
        let weight = |artist: &Value| {
            let tagged = artist["tags"]
                .as_array()
                .into_iter()
                .flatten()
                .find(|tag| tag["name"].as_str().is_some_and(|name| name.to_lowercase() == lower))
                .and_then(|tag| tag["count"].as_f64())
                .unwrap_or(0.);
            let users = artist["id"].as_str().and_then(|id| listeners.get(id)).copied().unwrap_or(0.);
            tagged * 2. + artist["score"].as_f64().unwrap_or(0.) / 50. + (users + 1.).log10() / 2.
        };
        let mut ranked: Vec<&Value> = artists.iter().collect();
        ranked.sort_by(|a, b| weight(b).total_cmp(&weight(a)));
        let mut tracks = vec![];
        let each = 2;
        for artist in ranked.into_iter().take(count.div_ceil(each).max(3)) {
            let name = artist["name"].as_str().unwrap_or("");
            let id = artist["id"].as_str().unwrap_or("");
            if let Ok(found) = self.top_recordings(id, name, each, signal).await {
                tracks.extend(found);
            }
            if tracks.len() >= count {
                break;
            }
        }
        tracks.truncate(count);
        if tracks.is_empty() {
            return Err(format!("Kumi found no recordings tagged {genre}; name an artist or a track in it."));
        }
        Ok(Resolved::Tracks { name: genre.to_lowercase(), kind: Kind::Genre, tracks, mbid: None })
    }

    /// An album's tracks (from its official release when it lists one).
    async fn album(&self, group: &Value, count: usize, signal: &Signal) -> Result<Resolved, String> {
        let title = group["title"].as_str().unwrap_or("").to_string();
        let artist = album_artist(group)["name"].as_str().or(group["artist-credit"][0]["name"].as_str()).unwrap_or("").to_string();
        let releases: Vec<&Value> = group["releases"].as_array().into_iter().flatten().collect();
        let Some(release) =
            releases.iter().find(|release| release["status"] == "Official").or(releases.first()).and_then(|release| release["id"].as_str())
        else {
            return Err(format!("Kumi found {title} but not its tracks; give a track or a link."));
        };
        let read = self.get(&format!("{}/ws/2/release/{release}?inc=recordings&fmt=json", self.musicbrainz), signal).await?;
        let tracks: Vec<Wanted> = read["media"]
            .as_array()
            .into_iter()
            .flatten()
            .flat_map(|medium| medium["tracks"].as_array().cloned().unwrap_or_default())
            .map(|track| Wanted {
                artist: artist.clone(),
                title: track["title"].as_str().unwrap_or("").to_string(),
                seconds: track["length"].as_f64().map(|ms| ms / 1000.),
                mbid: track["recording"]["id"].as_str().map(str::to_owned),
                ..Default::default()
            })
            .filter(|track| !track.title.is_empty())
            .take(count)
            .collect();
        let mbid = group["id"].as_str().map(str::to_owned);
        Ok(Resolved::Tracks { name: format!("{title} by {artist}"), kind: Kind::Album, tracks, mbid })
    }

    /// A Spotify link's names, from its public embed page (no sign-in): a track, an album's or a playlist's tracks.
    /// An artist's link is looked up on MusicBrainz, which knows most artists' Spotify pages; failing that, their name.
    /// When the embed page's data can't be read, Spotify's oEmbed title stands in.
    async fn spotify(&self, kind: &'static str, id: &str, count: usize, signal: Signal) -> Result<Resolved, String> {
        let link = format!("https://open.spotify.com/{kind}/{id}");
        if kind == "artist" {
            if let Some(artist) = self.linked_artist(&link, &signal).await {
                return self.artist(&artist, count, &signal).await;
            }
        }
        let page = self.text(&format!("{}/embed/{kind}/{id}", self.spotify), &signal).await;
        let Some(entity) = page.as_deref().ok().and_then(embed_entity) else {
            let title = self.oembed_title(&link, &signal).await.ok_or_else(|| match &page {
                Err(why) => format!("Spotify didn't say what that link is: {why}"),
                Ok(_) => "Spotify's page for that link didn't say what it is.".to_string(),
            })?;
            return match kind {
                "track" => Ok(Resolved::Tracks {
                    name: title.clone(),
                    kind: Kind::Spotify,
                    tracks: vec![Wanted { title, ..Default::default() }],
                    mbid: None,
                }),
                "playlist" => {
                    Err(format!("Spotify didn't list the tracks of {title}; give the tracks' names, a YouTube playlist or files."))
                }
                _ => self.words(&title, count, signal).await,
            };
        };
        let name = entity["name"].as_str().or(entity["title"].as_str()).unwrap_or("").to_string();
        if kind == "artist" {
            return self.words(&name, count, signal).await;
        }
        let artists = |value: &Value| -> String {
            value["artists"].as_array().into_iter().flatten().filter_map(|artist| artist["name"].as_str()).collect::<Vec<_>>().join(", ")
        };
        let tracks: Vec<Wanted> = if kind == "track" {
            vec![Wanted {
                artist: artists(&entity),
                title: name.clone(),
                seconds: entity["duration"].as_f64().map(|ms| ms / 1000.),
                ..Default::default()
            }]
        } else {
            entity["trackList"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|track| Wanted {
                    artist: track["subtitle"].as_str().unwrap_or("").to_string(),
                    title: track["title"].as_str().unwrap_or("").to_string(),
                    seconds: track["duration"].as_f64().map(|ms| ms / 1000.),
                    ..Default::default()
                })
                .filter(|track| !track.title.is_empty())
                .take(count)
                .collect()
        };
        if tracks.is_empty() {
            return Err("Spotify's page for that link listed no tracks.".into());
        }
        Ok(Resolved::Tracks { name, kind: Kind::Spotify, tracks, mbid: None })
    }

    /// The artist MusicBrainz ties a link to (none when it ties none, or doesn't answer).
    async fn linked_artist(&self, link: &str, signal: &Signal) -> Option<Value> {
        let read = self
            .get(&format!("{}/ws/2/url?resource={}&inc=artist-rels&fmt=json", self.musicbrainz, url_encode(link)), signal)
            .await
            .ok()?;
        read["relations"]
            .as_array()?
            .iter()
            .map(|relation| relation["artist"].clone())
            .find(|artist| artist["id"].is_string() && artist["name"].is_string())
    }

    /// A Spotify link's title from its oEmbed answer.
    async fn oembed_title(&self, link: &str, signal: &Signal) -> Option<String> {
        let read = self.get(&format!("{}/oembed?url={}", self.spotify, url_encode(link)), signal).await.ok()?;
        read["title"].as_str().map(str::trim).filter(|title| !title.is_empty()).map(str::to_owned)
    }
}

/// A link Kumi takes: a Spotify page (its kind and id), or a YouTube video or playlist (as YouTube's own address).
enum Link {
    Spotify(&'static str, String),
    Video(String),
    Playlist(String),
}

fn link(what: &str) -> Option<Link> {
    let url = url::Url::parse(what).ok()?;
    if !matches!(url.scheme(), "http" | "https") {
        return None;
    }
    match url.host_str()?.to_lowercase().as_str() {
        "open.spotify.com" | "play.spotify.com" => spotify_parts(what).map(|(kind, id)| Link::Spotify(kind, id)),
        "youtube.com" | "www.youtube.com" | "m.youtube.com" | "music.youtube.com" | "youtu.be" => {
            if let Some(id) = crate::video::youtube_id(what) {
                return Some(Link::Video(watch_url(&id)));
            }
            if url.path() != "/playlist" {
                return None;
            }
            let list = url.query_pairs().find(|(key, _)| key == "list")?.1.to_string();
            let fits = !list.is_empty() && list.len() <= 64 && list.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
            fits.then(|| Link::Playlist(format!("https://www.youtube.com/playlist?list={list}")))
        }
        _ => None,
    }
}

/// A YouTube video's own address.
pub fn watch_url(id: &str) -> String {
    format!("https://www.youtube.com/watch?v={id}")
}

/// A question's answer: `artist:<mbid>` or `album:<mbid>`.
fn picked(what: &str) -> Option<(&'static str, String)> {
    let (kind, id) = what.split_once(':')?;
    let kind = match kind.trim().to_lowercase().as_str() {
        "artist" => "artist",
        "album" | "release-group" => "album",
        _ => return None,
    };
    let id = uuid::Uuid::parse_str(id.trim()).ok()?;
    Some((kind, id.hyphenated().to_string()))
}

/// Whether what was asked for names a file or folder rather than words: a path's start (`/`, `~`, `./`, `../`, a
/// drive), or a separator or an audio file's name with something there. ".38 Special" and "Music" are words.
pub fn looks_like_path(what: &str) -> bool {
    let bytes = what.as_bytes();
    let rooted = what.starts_with(['/', '~'])
        || what.starts_with("./")
        || what.starts_with("../")
        || what.starts_with(".\\")
        || what.starts_with("..\\")
        || what.starts_with("\\\\")
        || (bytes.len() > 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' && matches!(bytes[2], b'\\' | b'/'));
    rooted || ((what.contains(['/', '\\']) || audio(Path::new(what))) && Path::new(&crate::audio::audio_path(what)).exists())
}

/// Whether what was asked for is a place (a path or a link), found again only by itself, never by a name.
pub fn is_place(what: &str) -> bool {
    let what = what.trim();
    what.starts_with("http://") || what.starts_with("https://") || looks_like_path(what)
}

fn audio(file: &Path) -> bool {
    file.extension().and_then(|ext| ext.to_str()).is_some_and(|ext| AUDIO.contains(&ext.to_lowercase().as_str()))
}

/// A folder's audio files, by name: files only, none hidden (the `._` twins macOS leaves on shared drives).
fn audio_files(folder: &Path) -> Result<Vec<PathBuf>, String> {
    let mut found: Vec<PathBuf> = std::fs::read_dir(folder)
        .map_err(|_| "Kumi couldn't read that folder.".to_string())?
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|file| {
            let hidden = file.file_name().and_then(|name| name.to_str()).is_none_or(|name| name.starts_with('.'));
            !hidden && file.is_file() && audio(file)
        })
        .collect();
    found.sort();
    Ok(found)
}

/// A file, or a folder's audio files (at most `count`, by name).
fn files(path: &str, count: usize) -> Result<Resolved, String> {
    let path = PathBuf::from(path);
    let mut found: Vec<PathBuf> = if path.is_dir() {
        audio_files(&path)?
    } else if path.is_file() && audio(&path) {
        vec![path.clone()]
    } else {
        return Err(format!("{} isn't an audio file or a folder of them.", path.display()));
    };
    found.truncate(count);
    if found.is_empty() {
        return Err(format!("{} has no audio files in it.", path.display()));
    }
    let name = path.file_stem().and_then(|name| name.to_str()).unwrap_or("reference").to_string();
    let tracks = found
        .into_iter()
        .map(|file| Wanted {
            title: file.file_stem().and_then(|name| name.to_str()).unwrap_or("").to_string(),
            file: Some(file),
            ..Default::default()
        })
        .collect();
    Ok(Resolved::Tracks { name, kind: Kind::Files, tracks, mbid: None })
}

/// How a file or a folder's audio files are now (names, sizes, times changed), to tell when they change; none for
/// what isn't a file or folder.
pub fn stamp(what: &str) -> Option<String> {
    let path = PathBuf::from(crate::audio::audio_path(what.trim()));
    let files = if path.is_dir() {
        audio_files(&path).ok()?
    } else if path.is_file() {
        vec![path]
    } else {
        return None;
    };
    let mut hasher = Sha256::new();
    for file in files {
        let about = std::fs::metadata(&file).ok()?;
        let changed = about.modified().ok().and_then(|at| at.duration_since(std::time::UNIX_EPOCH).ok()).map_or(0, |at| at.as_millis());
        hasher.update(format!("{}\t{}\t{changed}\n", file.file_name()?.to_string_lossy(), about.len()));
    }
    Some(hex::encode(hasher.finalize()))
}

/// A Lucene query for one MusicBrainz field, quoted and escaped, in a URL.
fn query(field: &str, words: &str) -> String {
    url_encode(&query_raw(field, words))
}

fn query_raw(field: &str, words: &str) -> String {
    format!("{field}:\"{}\"", words.replace('\\', "\\\\").replace('"', "\\\""))
}

fn url_encode(text: &str) -> String {
    text.bytes()
        .map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => (byte as char).to_string(),
            _ => format!("%{byte:02X}"),
        })
        .collect()
}

/// A name compared without case, accents' marks or punctuation.
pub fn folded(text: &str) -> String {
    text.to_lowercase().chars().filter(|c| c.is_alphanumeric()).collect()
}

/// A title without what versions add ("(Remastered)", "- Live").
fn base_title(title: &str) -> String {
    let cut = title.find(['(', '[']).map_or(title, |at| &title[..at]);
    let cut = cut.split(" - ").next().unwrap_or(cut);
    folded(cut)
}

/// An album's (first) artist.
fn album_artist(group: &Value) -> &Value {
    &group["artist-credit"][0]["artist"]
}

/// An artist or album as a question's option: its name and what tells it apart.
fn choice(kind: &'static str, value: &Value) -> Choice {
    let id = value["id"].as_str().unwrap_or("");
    if kind == "album" {
        let title = value["title"].as_str().unwrap_or("");
        let by = album_artist(value)["name"].as_str().or(value["artist-credit"][0]["name"].as_str()).unwrap_or("");
        let year = value["first-release-date"].as_str().filter(|date| date.len() >= 4).map(|date| format!(", {}", &date[..4]));
        return Choice::new("album", id, format!("{title} (album by {by}{})", year.unwrap_or_default()));
    }
    let name = value["name"].as_str().unwrap_or("");
    let mut about: Vec<String> = vec![];
    if let Some(note) = value["disambiguation"].as_str().filter(|note| !note.is_empty()) {
        about.push(note.into());
    }
    if let Some(area) = value["area"]["name"].as_str() {
        about.push(area.into());
    }
    if let Some(begin) = value["life-span"]["begin"].as_str() {
        about.push(format!("from {}", &begin[..begin.len().min(4)]));
    }
    let label = if about.is_empty() { name.to_string() } else { format!("{name} ({})", about.join(", ")) };
    Choice::new("artist", id, label)
}

fn host(url: &str) -> String {
    url.split("://").nth(1).and_then(|rest| rest.split('/').next()).unwrap_or(url).to_string()
}

/// A Spotify link's kind and id.
pub fn spotify_parts(link: &str) -> Option<(&'static str, String)> {
    let url = url::Url::parse(link).ok()?;
    if !matches!(url.host_str()?.to_lowercase().as_str(), "open.spotify.com" | "play.spotify.com") {
        return None;
    }
    let mut parts = url.path_segments()?.filter(|part| !part.is_empty() && !part.starts_with("intl-") && *part != "embed");
    let kind = match parts.next()? {
        "track" => "track",
        "album" => "album",
        "playlist" => "playlist",
        "artist" => "artist",
        _ => return None,
    };
    let id = parts.next()?.to_string();
    (!id.is_empty() && id.len() <= 64 && id.chars().all(|c| c.is_ascii_alphanumeric())).then_some((kind, id))
}

/// The entity a Spotify embed page describes, from its page data.
pub fn embed_entity(page: &str) -> Option<Value> {
    let start = page.find("id=\"__NEXT_DATA__\"")?;
    let open = start + page[start..].find('>')? + 1;
    let close = open + page[open..].find("</script>")?;
    let data: Value = serde_json::from_str(&page[open..close]).ok()?;
    let entity = data["props"]["pageProps"]["state"]["data"]["entity"].clone();
    (!entity.is_null()).then_some(entity)
}
