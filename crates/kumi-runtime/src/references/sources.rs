//! What a reference is, as tracks to fetch: a file or a folder of them, a YouTube video or playlist, a Spotify link
//! (its names; the audio comes from YouTube), or words: a genre (MusicBrainz's genres), an artist (their most listened
//! recordings, from ListenBrainz) or an album. Words that could mean more than one thing come back as one question.

use kumi_common::abort::Signal;
use serde_json::Value;
use std::{
    cell::Cell,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

/// One example track to measure.
#[derive(Debug, Clone, PartialEq)]
pub struct Wanted {
    pub artist: String,
    pub title: String,
    /// Its length when known, to pick the right upload.
    pub seconds: Option<f64>,
    /// A video to take the audio from as it is.
    pub url: Option<String>,
    pub file: Option<PathBuf>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Files,
    Video,
    Spotify,
    Artist,
    Album,
    Genre,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Resolved {
    Tracks {
        name: String,
        kind: Kind,
        tracks: Vec<Wanted>,
    },
    /// The words could mean more than one thing: one question for the producer, with what it could be.
    Ask {
        question: String,
        options: Vec<String>,
    },
}

const AUDIO: [&str; 7] = ["wav", "aif", "aiff", "flac", "mp3", "m4a", "ogg"];

pub struct Sources {
    client: reqwest::Client,
    musicbrainz: String,
    listenbrainz: String,
    spotify: String,
    /// When MusicBrainz was last asked: it takes one request a second.
    last: Cell<Option<Instant>>,
    pace: Duration,
    genres: std::cell::RefCell<Option<Vec<String>>>,
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
    /// Without waiting between requests (for a server that isn't MusicBrainz's).
    pub fn unpaced(mut self) -> Self {
        self.pace = Duration::ZERO;
        self
    }

    /// The tracks a reference stands for (at most `count`), or the one question that settles what it is.
    pub async fn resolve(&self, what: &str, count: usize, signal: Signal) -> Result<Resolved, String> {
        let what = what.trim().trim_matches(|c| c == '"' || c == '“' || c == '”');
        if what.is_empty() {
            return Err("Say what the reference is: a file, a folder, a YouTube or Spotify link, or an artist, album or genre.".into());
        }
        if what.starts_with("http://") || what.starts_with("https://") {
            return if what.contains("spotify.com/") {
                self.spotify(what, count, signal).await
            } else if crate::video::youtube_id(what).is_some() || what.contains("youtube.com/playlist") {
                Ok(Resolved::Tracks {
                    name: what.into(),
                    kind: Kind::Video,
                    tracks: vec![Wanted { artist: String::new(), title: what.into(), seconds: None, url: Some(what.into()), file: None }],
                })
            } else {
                Err("Kumi takes YouTube and Spotify links for references, or an audio file or folder.".into())
            };
        }
        let path = crate::audio::audio_path(what);
        if what.starts_with('/') || what.starts_with('~') || what.starts_with('.') || Path::new(&path).exists() {
            return files(&path, count);
        }
        self.words(what, count, signal).await
    }

    async fn get(&self, url: &str, signal: &Signal) -> Result<Value, String> {
        let text = self.text(url, signal).await?;
        serde_json::from_str(&text).map_err(|_| format!("{} didn't answer with JSON.", host(url)))
    }

    /// A page's text; MusicBrainz asked at its pace, and again (up to three times) when it says it's too busy.
    async fn text(&self, url: &str, signal: &Signal) -> Result<String, String> {
        for attempt in 0..3 {
            match self.text_once(url, signal).await {
                Err(why) if why.ends_with("503 Service Unavailable") && attempt < 2 => {
                    tokio::select! {
                        _ = tokio::time::sleep(Duration::from_millis(1500 * (attempt + 1))) => {}
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
        sent.text().await.map_err(|error| format!("{} broke off: {error}", host(url)))
    }

    /// MusicBrainz's genres, asked once.
    async fn genres(&self, signal: &Signal) -> Vec<String> {
        if let Some(known) = self.genres.borrow().clone() {
            return known;
        }
        let listed = self
            .text(&format!("{}/ws/2/genre/all?fmt=txt", self.musicbrainz), signal)
            .await
            .map(|text| text.lines().map(|line| line.trim().to_lowercase()).filter(|line| !line.is_empty()).collect::<Vec<_>>())
            .unwrap_or_default();
        *self.genres.borrow_mut() = Some(listed.clone());
        listed
    }

    async fn words(&self, words: &str, count: usize, signal: Signal) -> Result<Resolved, String> {
        let lower = words.to_lowercase();
        if self.genres(&signal).await.iter().any(|genre| *genre == lower) {
            return self.genre(words, count, &signal).await;
        }
        let found =
            self.get(&format!("{}/ws/2/artist?query={}&fmt=json&limit=25", self.musicbrainz, query("artist", words)), &signal).await?;
        let artists: Vec<&Value> = found["artists"].as_array().into_iter().flatten().collect();
        let score = |artist: &Value| artist["score"].as_f64().unwrap_or(0.);
        let same: Vec<&&Value> = artists.iter().filter(|artist| folded(artist["name"].as_str().unwrap_or("")) == folded(words)).collect();
        // The one everybody means stands well clear of the others by that name: by how well it matches, else by how
        // many people listen to it (ten times the next). Two close ones need a question.
        let close: Vec<&Value> = same.iter().filter(|artist| score(artist) >= 76.).map(|artist| **artist).collect();
        match close.as_slice() {
            [] => {}
            [one] => return self.artist(one, count, &signal).await,
            several => {
                let owned: Vec<Value> = several.iter().map(|artist| (*artist).clone()).collect();
                let listeners = self.listeners(&owned, &signal).await;
                let heard = |artist: &Value| artist["id"].as_str().and_then(|id| listeners.get(id)).copied().unwrap_or(0.);
                let mut ranked = several.to_vec();
                ranked.sort_by(|a, b| heard(b).total_cmp(&heard(a)));
                if heard(ranked[0]) >= 100. && heard(ranked[0]) >= heard(ranked[1]) * 10. {
                    return self.artist(ranked[0], count, &signal).await;
                }
                return Ok(Resolved::Ask {
                    question: format!("Which {words} do you mean?"),
                    options: ranked.iter().take(5).map(|artist| described(artist)).collect(),
                });
            }
        }
        let albums = self
            .get(&format!("{}/ws/2/release-group?query={}&fmt=json&limit=5", self.musicbrainz, query("releasegroup", words)), &signal)
            .await?;
        if let Some(album) =
            albums["release-groups"].as_array().into_iter().flatten().find(|group| {
                group["score"].as_f64().unwrap_or(0.) >= 95. && folded(group["title"].as_str().unwrap_or("")) == folded(words)
            })
        {
            return self.album(album, count, &signal).await;
        }
        if let Some(artist) = artists.first().filter(|artist| artist["score"].as_f64().unwrap_or(0.) >= 98.) {
            return self.artist(artist, count, &signal).await;
        }
        Ok(Resolved::Ask {
            question: format!("Kumi couldn't place “{words}”: which artist, album or genre is it (a track or a link works too)?"),
            options: artists.iter().take(3).map(|artist| described(artist)).collect(),
        })
    }

    /// An artist's most listened recordings.
    async fn artist(&self, artist: &Value, count: usize, signal: &Signal) -> Result<Resolved, String> {
        let name = artist["name"].as_str().unwrap_or("").to_string();
        let id = artist["id"].as_str().unwrap_or("");
        let tracks = self.top_recordings(id, &name, count, signal).await?;
        if tracks.is_empty() {
            return Err(format!("Kumi found {name} but no recordings to listen to; give a track or a link."));
        }
        Ok(Resolved::Tracks { name, kind: Kind::Artist, tracks })
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
        for (_, title, seconds) in ranked {
            let key = base_title(title);
            if seen.contains(&key) {
                continue;
            }
            seen.push(key);
            tracks.push(Wanted { artist: name.into(), title: title.clone(), seconds: *seconds, url: None, file: None });
            if tracks.len() >= count {
                break;
            }
        }
        Ok(tracks)
    }

    /// How many times people listened to each recording, from ListenBrainz (none when it doesn't answer).
    async fn listens(&self, ids: &[&str], signal: &Signal) -> std::collections::HashMap<String, f64> {
        if ids.is_empty() {
            return Default::default();
        }
        let sent = tokio::select! {
            sent = self.client.post(format!("{}/1/popularity/recording", self.listenbrainz)).json(&serde_json::json!({"recording_mbids": ids})).send() => sent,
            _ = signal.cancelled() => return Default::default(),
        };
        let Ok(read) = sent else { return Default::default() };
        let rows: Value = read.json().await.unwrap_or_default();
        rows.as_array()
            .into_iter()
            .flatten()
            .filter_map(|row| Some((row["recording_mbid"].as_str()?.to_string(), row["total_listen_count"].as_f64().unwrap_or(0.))))
            .collect()
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
        Ok(Resolved::Tracks { name: genre.to_lowercase(), kind: Kind::Genre, tracks })
    }

    /// How many people listen to each artist, from ListenBrainz (none when it doesn't answer).
    async fn listeners(&self, artists: &[Value], signal: &Signal) -> std::collections::HashMap<String, f64> {
        let ids: Vec<&str> = artists.iter().filter_map(|artist| artist["id"].as_str()).collect();
        let sent = tokio::select! {
            sent = self.client.post(format!("{}/1/popularity/artist", self.listenbrainz)).json(&serde_json::json!({"artist_mbids": ids})).send() => sent,
            _ = signal.cancelled() => return Default::default(),
        };
        let Ok(read) = sent else { return Default::default() };
        let rows: Value = read.json().await.unwrap_or_default();
        rows.as_array()
            .into_iter()
            .flatten()
            .filter_map(|row| Some((row["artist_mbid"].as_str()?.to_string(), row["total_user_count"].as_f64().unwrap_or(0.))))
            .collect()
    }

    /// An album's tracks.
    async fn album(&self, group: &Value, count: usize, signal: &Signal) -> Result<Resolved, String> {
        let title = group["title"].as_str().unwrap_or("").to_string();
        let artist = group["artist-credit"][0]["name"].as_str().unwrap_or("").to_string();
        let Some(release) = group["releases"].as_array().and_then(|releases| releases.first()).and_then(|release| release["id"].as_str())
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
                url: None,
                file: None,
            })
            .filter(|track| !track.title.is_empty())
            .take(count)
            .collect();
        Ok(Resolved::Tracks { name: format!("{title} by {artist}"), kind: Kind::Album, tracks })
    }

    /// A Spotify link's names, from its public embed page (no sign-in): a track, an album's or a playlist's tracks,
    /// or an artist (then their most listened recordings).
    async fn spotify(&self, link: &str, count: usize, signal: Signal) -> Result<Resolved, String> {
        let Some((kind, id)) = spotify_parts(link) else {
            return Err("That Spotify link isn't a track, album, playlist or artist.".into());
        };
        let page = self.text(&format!("{}/embed/{kind}/{id}", self.spotify), &signal).await?;
        let entity = embed_entity(&page).ok_or("Spotify's page for that link didn't list what it is.")?;
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
                url: None,
                file: None,
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
                    url: None,
                    file: None,
                })
                .filter(|track| !track.title.is_empty())
                .take(count)
                .collect()
        };
        if tracks.is_empty() {
            return Err("Spotify's page for that link listed no tracks.".into());
        }
        Ok(Resolved::Tracks { name, kind: Kind::Spotify, tracks })
    }
}

/// A file, or a folder's audio files (at most `count`, by name).
fn files(path: &str, count: usize) -> Result<Resolved, String> {
    let path = PathBuf::from(path);
    let audio = |file: &Path| file.extension().and_then(|ext| ext.to_str()).is_some_and(|ext| AUDIO.contains(&ext.to_lowercase().as_str()));
    let mut found: Vec<PathBuf> = if path.is_dir() {
        std::fs::read_dir(&path)
            .map_err(|_| "Kumi couldn't read that folder.".to_string())?
            .filter_map(|entry| entry.ok().map(|entry| entry.path()))
            .filter(|file| audio(file))
            .collect()
    } else if path.is_file() && audio(&path) {
        vec![path.clone()]
    } else {
        return Err(format!("{} isn't an audio file or a folder of them.", path.display()));
    };
    found.sort();
    found.truncate(count);
    if found.is_empty() {
        return Err(format!("{} has no audio files in it.", path.display()));
    }
    let name = path.file_stem().and_then(|name| name.to_str()).unwrap_or("reference").to_string();
    let tracks = found
        .into_iter()
        .map(|file| Wanted {
            artist: String::new(),
            title: file.file_stem().and_then(|name| name.to_str()).unwrap_or("").to_string(),
            seconds: None,
            url: None,
            file: Some(file),
        })
        .collect();
    Ok(Resolved::Tracks { name, kind: Kind::Files, tracks })
}

/// A Lucene query for one MusicBrainz field, quoted and escaped, in a URL.
fn query(field: &str, words: &str) -> String {
    let quoted = format!("{field}:\"{}\"", words.replace('\\', "\\\\").replace('"', "\\\""));
    url_encode(&quoted)
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

/// An artist as a question's option: their name and what tells them apart.
fn described(artist: &Value) -> String {
    let name = artist["name"].as_str().unwrap_or("");
    let mut about: Vec<String> = vec![];
    if let Some(note) = artist["disambiguation"].as_str().filter(|note| !note.is_empty()) {
        about.push(note.into());
    }
    if let Some(area) = artist["area"]["name"].as_str() {
        about.push(area.into());
    }
    if let Some(begin) = artist["life-span"]["begin"].as_str() {
        about.push(format!("from {}", &begin[..begin.len().min(4)]));
    }
    if about.is_empty() {
        name.into()
    } else {
        format!("{name} ({})", about.join(", "))
    }
}

fn host(url: &str) -> String {
    url.split("://").nth(1).and_then(|rest| rest.split('/').next()).unwrap_or(url).to_string()
}

/// A Spotify link's kind and id.
pub fn spotify_parts(link: &str) -> Option<(&'static str, String)> {
    let after = link.split("spotify.com/").nth(1)?;
    let mut parts = after.split(['/', '?']).filter(|part| !part.is_empty() && !part.starts_with("intl-"));
    let kind = match parts.next()? {
        "track" => "track",
        "album" => "album",
        "playlist" => "playlist",
        "artist" => "artist",
        _ => return None,
    };
    let id = parts.next()?.to_string();
    id.chars().all(|c| c.is_ascii_alphanumeric()).then_some((kind, id))
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
