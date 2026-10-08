//! A reference track's audio: the producer's file as it is, a YouTube video's sound, or, for a track known by its
//! names, the upload a YouTube search finds whose length matches the track's (within a few seconds). Fetched audio is
//! named by its video id, and let go once it's measured.

use super::sources::{folded, watch_url, Wanted};
use crate::video::programs::{find_yt_dlp, run, yt_dlp_extras, ProgramOptions, RunOptions};
use kumi_common::abort::Signal;
use serde_json::Value;
use std::{
    cell::RefCell,
    path::{Path, PathBuf},
};

/// Videos longer than this aren't one track (a mix, a whole album): passed over in searches and playlists, and not
/// downloaded.
pub const LONGEST_VIDEO: f64 = 1200.;

/// A track's audio, ready to measure.
#[derive(Debug, Clone, PartialEq)]
pub struct Audio {
    pub file: PathBuf,
    /// The video it came from, or the producer's file.
    pub source: String,
    /// Fetched by Kumi (so it can go once it's measured), not the producer's own.
    pub fetched: bool,
}

pub struct Fetcher {
    /// Where downloaded audio goes.
    pub folder: PathBuf,
    pub programs: ProgramOptions,
    /// yt-dlp once it's found (not finding it isn't kept: the next fetch looks again).
    ytdlp: RefCell<Option<String>>,
}

impl Fetcher {
    pub fn new(folder: impl Into<PathBuf>, programs: ProgramOptions) -> Self {
        Self { folder: folder.into(), programs, ytdlp: RefCell::new(None) }
    }

    /// yt-dlp and what it's told besides (which yt_dlp_extras keeps once it knows).
    async fn ytdlp(&self, signal: &Signal) -> Result<(String, Vec<String>), String> {
        let known = self.ytdlp.borrow().clone();
        let found = match known {
            Some(found) => found,
            None => {
                let mut programs = self.programs.clone();
                programs.signal = Some(signal.clone());
                programs.purpose = Some("to fetch a reference's audio".into());
                let found = find_yt_dlp(&programs).await.map_err(|error| error.message())?;
                *self.ytdlp.borrow_mut() = Some(found.clone());
                found
            }
        };
        let extras = yt_dlp_extras(&found, Some(signal.clone())).await;
        Ok((found, extras))
    }

    /// The track's audio file, fetched if it has to be.
    pub async fn audio(&self, wanted: &Wanted, signal: Signal) -> Result<Audio, String> {
        if let Some(file) = &wanted.file {
            return Ok(Audio { file: file.clone(), source: file.display().to_string(), fetched: false });
        }
        let url = match &wanted.url {
            Some(url) => url.clone(),
            None => self.search(wanted, &signal).await?,
        };
        let file = self.download(&url, &signal).await?;
        Ok(Audio { file, source: url, fetched: true })
    }

    /// A YouTube playlist's videos (at most `count`, none longer than a track), as tracks to fetch one by one.
    pub async fn playlist(&self, url: &str, count: usize, signal: Signal) -> Result<Vec<Wanted>, String> {
        let (ytdlp, extras) = self.ytdlp(&signal).await?;
        let mut args: Vec<String> =
            vec!["--flat-playlist".into(), "-J".into(), "--no-warnings".into(), "--playlist-end".into(), (count * 2).to_string()];
        args.extend(extras);
        args.extend(["--".into(), url.into()]);
        let found = run(&ytdlp, &args, RunOptions { signal: Some(signal), timeout_ms: Some(60_000), max_buffer: Some(8 << 20) })
            .await
            .map_err(|error| format!("Kumi couldn't read that playlist: {}", error.message()))?;
        let listed: Value = serde_json::from_str(&found.stdout_text()).map_err(|_| "yt-dlp's playlist wasn't JSON.".to_string())?;
        Ok(entries(&listed)
            .into_iter()
            .map(|(entry, url)| Wanted {
                artist: entry["channel"].as_str().unwrap_or("").to_string(),
                title: entry["title"].as_str().unwrap_or("").to_string(),
                seconds: entry["duration"].as_f64(),
                url: Some(url),
                ..Default::default()
            })
            .take(count)
            .collect())
    }

    /// The upload that matches, from a YouTube search for the artist and title: the closest in length within 3 seconds
    /// of the track's (else within 8, else within a twentieth of it), else one whose title names the track and the
    /// artist. When the length isn't known, only one that names them.
    async fn search(&self, wanted: &Wanted, signal: &Signal) -> Result<String, String> {
        let (ytdlp, extras) = self.ytdlp(signal).await?;
        let terms = format!("{} {}", wanted.artist, wanted.title).trim().to_string();
        let mut args: Vec<String> = vec!["--flat-playlist".into(), "-J".into(), "--no-warnings".into()];
        args.extend(extras);
        args.extend(["--".into(), format!("ytsearch5:{terms}")]);
        let found = run(&ytdlp, &args, RunOptions { signal: Some(signal.clone()), timeout_ms: Some(60_000), max_buffer: Some(8 << 20) })
            .await
            .map_err(|error| format!("YouTube search for {terms} failed: {}", error.message()))?;
        let listed: Value = serde_json::from_str(&found.stdout_text()).map_err(|_| "yt-dlp's search wasn't JSON.".to_string())?;
        let found = entries(&listed);
        // An upload whose title (or channel) names both the track and the artist.
        let named = |(entry, _): &&(&Value, String)| {
            let said = folded(&format!("{} {}", entry["title"].as_str().unwrap_or(""), entry["channel"].as_str().unwrap_or("")));
            !folded(&wanted.title).is_empty() && said.contains(&folded(&wanted.title)) && said.contains(&folded(&wanted.artist))
        };
        let pick = match wanted.seconds {
            Some(seconds) => [3., 8., (seconds * 0.05).max(8.)]
                .iter()
                .find_map(|within| {
                    found
                        .iter()
                        .filter_map(|entry| Some((entry, (entry.0["duration"].as_f64()? - seconds).abs())))
                        .filter(|(_, off)| off <= within)
                        .min_by(|a, b| a.1.total_cmp(&b.1))
                        .map(|(entry, _)| entry)
                })
                .or_else(|| found.iter().find(named)),
            None => found.iter().find(named),
        };
        pick.map(|(_, url)| url.clone()).ok_or_else(|| {
            if wanted.seconds.is_some() {
                format!("No upload of {terms} matched its length.")
            } else {
                format!("No upload named {terms}.")
            }
        })
    }

    /// A YouTube video's sound, downloaded into the folder by its id (once: a copy already there is used).
    async fn download(&self, url: &str, signal: &Signal) -> Result<PathBuf, String> {
        let id = crate::video::youtube_id(url).ok_or("Kumi fetches a reference's audio from YouTube videos only.")?;
        if let Some(kept) = kept(&self.folder, &id) {
            return Ok(kept);
        }
        tokio::fs::create_dir_all(&self.folder).await.map_err(|error| format!("Kumi couldn't make {}: {error}", self.folder.display()))?;
        let (ytdlp, extras) = self.ytdlp(signal).await?;
        let template = self.folder.join(format!("{id}.%(ext)s"));
        let mut args: Vec<String> = vec![
            "-f".into(),
            "bestaudio[ext=m4a]/bestaudio".into(),
            "--no-playlist".into(),
            "--no-warnings".into(),
            "--no-progress".into(),
            "--match-filter".into(),
            format!("duration<{LONGEST_VIDEO}"),
            "-o".into(),
            template.to_string_lossy().into(),
        ];
        args.extend(extras);
        args.extend(["--".into(), watch_url(&id)]);
        let fetched =
            run(&ytdlp, &args, RunOptions { signal: Some(signal.clone()), timeout_ms: Some(300_000), max_buffer: Some(1 << 20) }).await;
        if let Err(error) = fetched {
            let_go(&self.folder, &id);
            return Err(format!("Kumi couldn't fetch {url}: {}", error.message()));
        }
        kept(&self.folder, &id).ok_or_else(|| {
            let_go(&self.folder, &id);
            format!("yt-dlp finished, but {url}'s audio isn't there (Kumi passes over videos longer than {} minutes).", LONGEST_VIDEO / 60.)
        })
    }
}

/// A search's or playlist's entries that are YouTube videos no longer than a track, each with its own address.
fn entries(listed: &Value) -> Vec<(&Value, String)> {
    listed["entries"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|entry| entry["duration"].as_f64().is_none_or(|seconds| seconds < LONGEST_VIDEO))
        .filter_map(|entry| Some((entry, watch_url(&crate::video::youtube_id(entry["url"].as_str()?)?))))
        .collect()
}

/// The file a video's id was downloaded as, if it was.
pub fn kept(folder: &Path, id: &str) -> Option<PathBuf> {
    let entries = std::fs::read_dir(folder).ok()?;
    entries.filter_map(|entry| entry.ok().map(|entry| entry.path())).find(|path| {
        path.is_file()
            && path.file_stem().and_then(|stem| stem.to_str()) == Some(id)
            && path.extension().and_then(|ext| ext.to_str()).is_some_and(|ext| !matches!(ext, "part" | "ytdl" | "temp"))
    })
}

/// What a failed download left behind (`<id>.m4a.part` and the like), gone.
fn let_go(folder: &Path, id: &str) {
    let Ok(entries) = std::fs::read_dir(folder) else { return };
    let start = format!("{id}.");
    for path in entries.filter_map(|entry| entry.ok().map(|entry| entry.path())) {
        if path.is_file() && path.file_name().and_then(|name| name.to_str()).is_some_and(|name| name.starts_with(&start)) {
            let _ = std::fs::remove_file(path);
        }
    }
}
