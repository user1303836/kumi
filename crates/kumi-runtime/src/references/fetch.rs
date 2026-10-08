//! A reference track's audio: the producer's file as it is, a video's sound, or YouTube Music's upload whose length
//! matches the track's (within a few seconds), each downloaded once and kept by its video id.

use super::sources::{folded, Wanted};
use crate::video::programs::{find_yt_dlp, run, yt_dlp_extras, ProgramOptions, RunOptions};
use kumi_common::abort::Signal;
use serde_json::Value;
use std::path::{Path, PathBuf};

pub struct Fetcher {
    /// Where downloaded audio is kept.
    pub folder: PathBuf,
    pub programs: ProgramOptions,
    ytdlp: tokio::sync::OnceCell<Result<(String, Vec<String>), String>>,
}

impl Fetcher {
    pub fn new(folder: impl Into<PathBuf>, programs: ProgramOptions) -> Self {
        Self { folder: folder.into(), programs, ytdlp: tokio::sync::OnceCell::new() }
    }

    async fn ytdlp(&self, signal: &Signal) -> Result<(String, Vec<String>), String> {
        self.ytdlp
            .get_or_init(|| async {
                let mut programs = self.programs.clone();
                programs.signal = Some(signal.clone());
                programs.purpose = Some("to fetch a reference's audio".into());
                let found = find_yt_dlp(&programs).await.map_err(|error| error.message())?;
                let extras = yt_dlp_extras(&found, Some(signal.clone())).await;
                Ok((found, extras))
            })
            .await
            .clone()
    }

    /// The track's audio file, fetched if it has to be.
    pub async fn audio(&self, wanted: &Wanted, signal: Signal) -> Result<PathBuf, String> {
        if let Some(file) = &wanted.file {
            return Ok(file.clone());
        }
        let url = match &wanted.url {
            Some(url) => url.clone(),
            None => self.search(wanted, &signal).await?,
        };
        self.download(&url, &signal).await
    }

    /// A playlist's videos (at most `count`), as tracks to fetch one by one.
    pub async fn playlist(&self, url: &str, count: usize, signal: Signal) -> Result<Vec<Wanted>, String> {
        let (ytdlp, extras) = self.ytdlp(&signal).await?;
        let mut args: Vec<String> =
            vec![url.into(), "--flat-playlist".into(), "-J".into(), "--no-warnings".into(), "--playlist-end".into(), count.to_string()];
        args.extend(extras);
        let found = run(&ytdlp, &args, RunOptions { signal: Some(signal), timeout_ms: Some(60_000), max_buffer: Some(8 << 20) })
            .await
            .map_err(|error| format!("Kumi couldn't read that playlist: {}", error.message()))?;
        let listed: Value = serde_json::from_str(&found.stdout_text()).map_err(|_| "yt-dlp's playlist wasn't JSON.".to_string())?;
        Ok(listed["entries"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|entry| {
                let url = entry["url"]
                    .as_str()
                    .map(str::to_owned)
                    .or_else(|| entry["id"].as_str().map(|id| format!("https://www.youtube.com/watch?v={id}")))?;
                Some(Wanted {
                    artist: entry["channel"].as_str().unwrap_or("").to_string(),
                    title: entry["title"].as_str().unwrap_or("").to_string(),
                    seconds: entry["duration"].as_f64(),
                    url: Some(url),
                    file: None,
                })
            })
            .take(count)
            .collect())
    }

    /// The upload that matches: YouTube Music's results for the artist and title, the closest in length within 3
    /// seconds of the track's (else within 8, else within a twentieth of it), or the first when the length isn't known.
    async fn search(&self, wanted: &Wanted, signal: &Signal) -> Result<String, String> {
        let (ytdlp, extras) = self.ytdlp(signal).await?;
        let terms = format!("{} {}", wanted.artist, wanted.title).trim().to_string();
        let mut args: Vec<String> = vec![format!("ytsearch5:{terms}"), "--flat-playlist".into(), "-J".into(), "--no-warnings".into()];
        args.extend(extras);
        let found = run(&ytdlp, &args, RunOptions { signal: Some(signal.clone()), timeout_ms: Some(60_000), max_buffer: Some(8 << 20) })
            .await
            .map_err(|error| format!("YouTube search for {terms} failed: {}", error.message()))?;
        let listed: Value = serde_json::from_str(&found.stdout_text()).map_err(|_| "yt-dlp's search wasn't JSON.".to_string())?;
        let entries: Vec<&Value> = listed["entries"].as_array().into_iter().flatten().collect();
        let url = |entry: &Value| {
            entry["url"]
                .as_str()
                .map(str::to_owned)
                .or_else(|| entry["id"].as_str().map(|id| format!("https://www.youtube.com/watch?v={id}")))
        };
        // Last, an upload whose title names both the track and the artist (another release's length, say).
        let named = |entry: &&&Value| {
            let said = folded(&format!("{} {}", entry["title"].as_str().unwrap_or(""), entry["channel"].as_str().unwrap_or("")));
            said.contains(&folded(&wanted.title)) && said.contains(&folded(&wanted.artist))
        };
        let pick = match wanted.seconds {
            Some(seconds) => [3., 8., (seconds * 0.05).max(8.)]
                .iter()
                .find_map(|within| {
                    entries
                        .iter()
                        .filter_map(|entry| Some((entry, (entry["duration"].as_f64()? - seconds).abs())))
                        .filter(|(_, off)| off <= within)
                        .min_by(|a, b| a.1.total_cmp(&b.1))
                        .map(|(entry, _)| *entry)
                })
                .or_else(|| entries.iter().find(named).copied()),
            None => entries.first().copied(),
        };
        pick.and_then(url).ok_or_else(|| format!("No upload of {terms} matched its length."))
    }

    /// A video's sound, downloaded once into the folder by its id.
    async fn download(&self, url: &str, signal: &Signal) -> Result<PathBuf, String> {
        let id = crate::video::youtube_id(url).unwrap_or_else(|| folded_id(url));
        if let Some(kept) = kept(&self.folder, &id) {
            return Ok(kept);
        }
        tokio::fs::create_dir_all(&self.folder).await.map_err(|error| format!("Kumi couldn't make {}: {error}", self.folder.display()))?;
        let (ytdlp, extras) = self.ytdlp(signal).await?;
        let template = self.folder.join(format!("{id}.%(ext)s"));
        let mut args: Vec<String> = vec![
            url.into(),
            "-f".into(),
            "bestaudio[ext=m4a]/bestaudio".into(),
            "--no-playlist".into(),
            "--no-warnings".into(),
            "--no-progress".into(),
            "-o".into(),
            template.to_string_lossy().into(),
        ];
        args.extend(extras);
        run(&ytdlp, &args, RunOptions { signal: Some(signal.clone()), timeout_ms: Some(300_000), max_buffer: Some(1 << 20) })
            .await
            .map_err(|error| format!("Kumi couldn't fetch {url}: {}", error.message()))?;
        kept(&self.folder, &id).ok_or_else(|| format!("yt-dlp finished, but {url}'s audio isn't there."))
    }
}

/// The file a video's id was kept as, if it was.
fn kept(folder: &Path, id: &str) -> Option<PathBuf> {
    let entries = std::fs::read_dir(folder).ok()?;
    entries
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .find(|path| path.file_stem().and_then(|stem| stem.to_str()) == Some(id) && path.extension().is_some_and(|ext| ext != "part"))
}

/// A name a link can be kept under (its letters and digits).
fn folded_id(url: &str) -> String {
    let id: String = url.chars().filter(|c| c.is_ascii_alphanumeric()).collect();
    id.chars().rev().take(40).collect::<String>().chars().rev().collect()
}
