pub mod captions;
pub mod frames;
pub mod moments;
pub mod programs;
pub mod speech;
pub mod tool;

use crate::{audio::audio_path, system::Env};
pub use captions::{format_time, parse_captions, parse_time, transcript_lines, Cue, TranscriptLine};
use captions::{said_around, TranscriptOptions};
use frames::{duration_of, frame_at, sound_between, Input};
pub use frames::{Region, Thumb, REGIONS};
use futures::{stream::FuturesUnordered, StreamExt};
use kumi_common::{
    abort::{Signal, SignalExt},
    js::{
        json::stringify,
        number::{round, to_fixed, to_string},
        string::{head, trim, utf16_len},
    },
    time::now_ms,
};
use moments::MomentOptions;
pub use moments::{choose_moments, Chapter};
use programs::{ffmpeg_hint, run, whisper_hint, whisper_model, yt_dlp_extras, FfmpegOptions, ProgramOptions, RunOptions};
pub use programs::{find_ffmpeg, find_whisper, find_yt_dlp, whisper_asset, yt_dlp_asset, VideoError, VideoFailure};
use regex::Regex;
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use speech::{speech_model_for, speech_prompt, transcribe, TranscribeOptions};
use std::{
    path::Path,
    rc::Rc,
    sync::{LazyLock, Mutex},
    time::{SystemTime, UNIX_EPOCH},
};
use tokio::io::AsyncWriteExt;

pub const VIDEO_EXTENSIONS: [&str; 6] = [".mp4", ".mov", ".m4v", ".mkv", ".webm", ".avi"];
pub const MAX_SOUND: f64 = 120.0;
#[derive(Clone, Default)]
pub struct WatchRequest {
    pub url: String,
    pub from: Option<f64>,
    pub to: Option<f64>,
    pub look_at: Option<Vec<f64>>,
    pub zoom: Option<Region>,
    pub frames: Option<f64>,
    pub listen: Option<SoundSpan>,
}
#[derive(Clone, Default)]
pub struct WatchOptions {
    pub videos_dir: String,
    pub tools_dir: String,
    pub env: Option<Env>,
    pub signal: Option<Signal>,
    pub on_fetch: Option<programs::OnFetch>,
    pub on_progress: Option<Rc<dyn Fn(&str)>>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SoundSpan {
    pub from: f64,
    pub to: f64,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Words {
    pub language: String,
    pub source: crate::core::contracts::WordsSource,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct VideoMeta {
    pub version: u8,
    pub key: String,
    pub url: String,
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub channel: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration: Option<f64>,
    pub chapters: Vec<Chapter>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub words: Option<Words>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WatchedFrame {
    pub at: f64,
    pub said: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub region: Option<Region>,
    pub jpeg: Vec<u8>,
    pub thumb: Thumb,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Sound {
    pub file: String,
    pub from: f64,
    pub to: f64,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Watched {
    #[serde(flatten)]
    pub meta: VideoMeta,
    pub from: f64,
    pub to: f64,
    pub lines: Vec<TranscriptLine>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cut_at: Option<f64>,
    pub frames: Vec<WatchedFrame>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sound: Option<Sound>,
    pub notes: Vec<String>,
}
impl std::ops::Deref for Watched {
    type Target = VideoMeta;
    fn deref(&self) -> &VideoMeta {
        &self.meta
    }
}
pub fn youtube_id(url: &str) -> Option<String> {
    static ID: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(
            r"^https?://(?:www\.|m\.|music\.)?(?:youtube\.com/(?:watch\?(?:[^#]*&)?v=|shorts/|live/|embed/)|youtu\.be/)([A-Za-z0-9_-]{11})",
        )
        .unwrap()
    });
    ID.captures(trim(url)).map(|c| c[1].into())
}
pub fn public_address(value: &str) -> bool {
    let Ok(url) = url::Url::parse(value) else {
        return false;
    };
    if !matches!(url.scheme(), "https" | "http") {
        return false;
    }
    let host = url.host_str().unwrap_or("").to_lowercase();
    let host = host.trim_start_matches('[').trim_end_matches(']');
    if host == "localhost"
        || [".localhost", ".local", ".internal"].iter().any(|s| host.ends_with(s))
        || (!host.contains('.') && !host.contains(':'))
    {
        return false;
    }
    if let Ok(ip) = host.parse::<std::net::Ipv4Addr>() {
        let [a, b, _, _] = ip.octets();
        return !(a == 0
            || a == 10
            || a == 127
            || (a == 169 && b == 254)
            || (a == 172 && (16..=31).contains(&b))
            || (a == 192 && b == 168)
            || (a == 100 && (64..=127).contains(&b))
            || a >= 224);
    }
    if host.contains(':') {
        return !(host == "::"
            || host == "::1"
            || host.starts_with("fc")
            || host.starts_with("fd")
            || ["fe8", "fe9", "fea", "feb", "::ffff:"].iter().any(|s| host.starts_with(s)));
    }
    true
}
fn safe(text: &str) -> String {
    static BAD: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[^a-z0-9_-]+").unwrap());
    let s = BAD.replace_all(&text.to_lowercase(), "-").trim_matches('-').chars().take(80).collect::<String>();
    if s.is_empty() {
        "video".into()
    } else {
        s
    }
}
fn join(folder: &str, file: &str) -> String {
    Path::new(folder).join(file).to_string_lossy().into()
}
async fn read_json<T: DeserializeOwned>(path: &str) -> Option<T> {
    serde_json::from_slice(&tokio::fs::read(path).await.ok()?).ok()
}
async fn mkdir(path: &str) -> Result<(), VideoFailure> {
    let mut builder = tokio::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    builder.mode(0o700);
    builder.create(path).await?;
    Ok(())
}
async fn write_json(path: &str, value: &impl Serialize) -> Result<(), VideoFailure> {
    let text = stringify(&serde_json::to_value(value).map_err(|e| VideoFailure::other(e.to_string()))?);
    let mut options = tokio::fs::OpenOptions::new();
    options.create(true).truncate(true).write(true);
    #[cfg(unix)]
    options.mode(0o600);
    let mut file = options.open(path).await?;
    file.write_all(text.as_bytes()).await?;
    file.flush().await?;
    Ok(())
}
async fn prune(folder: &str) -> Result<(), VideoFailure> {
    let Ok(mut entries) = tokio::fs::read_dir(folder).await else {
        return Ok(());
    };
    let mut folders = Vec::new();
    while let Some(entry) = entries.next_entry().await? {
        if let Ok(meta) = tokio::fs::metadata(entry.path().join("meta.json")).await {
            if let Ok(used) = meta.modified() {
                folders.push((entry.path(), used));
            }
        }
    }
    folders.sort_by(|a, b| b.1.cmp(&a.1));
    for (path, _) in folders.into_iter().skip(24) {
        match tokio::fs::remove_dir_all(path).await {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(e.into()),
            _ => {}
        }
    }
    Ok(())
}
#[derive(Clone)]
struct Track {
    code: String,
    automatic: bool,
    url: Option<String>,
    ext: Option<String>,
}
fn caption_track(info: &Value) -> Option<Track> {
    let spoken = info["language"].as_str().unwrap_or("en").split(['-', '_']).next().unwrap_or("").to_lowercase();
    let mut tracks = Vec::new();
    for (key, automatic) in [("subtitles", false), ("automatic_captions", true)] {
        for (code, list) in info[key].as_object().into_iter().flatten() {
            if !list.is_array() || code.to_lowercase().contains("live_chat") {
                continue;
            }
            let base = code.split(['-', '_']).next().unwrap_or("").to_lowercase();
            let rank = if !automatic {
                if base == "en" {
                    0
                } else if base == spoken {
                    1
                } else {
                    6
                }
            } else if code.ends_with("-orig") && base == spoken {
                2
            } else if code.to_lowercase() == spoken {
                3
            } else if base == "en" {
                4
            } else {
                9
            };
            if rank < 9 {
                tracks.push((code, list, automatic, rank));
            }
        }
    }
    tracks.sort_by_key(|t| t.3);
    let (code, list, automatic, _) = tracks.first()?;
    let format = ["json3", "vtt", "srt"]
        .into_iter()
        .find_map(|ext| list.as_array().unwrap().iter().find(|item| item["ext"] == ext && item["url"].is_string()));
    Some(Track {
        code: (*code).clone(),
        automatic: *automatic,
        url: format.and_then(|v| v["url"].as_str()).filter(|s| !s.is_empty()).map(str::to_string),
        ext: format.and_then(|v| v["ext"].as_str()).map(str::to_string),
    })
}
#[derive(Clone, Default)]
struct Sources {
    video: Option<Input>,
    sharp: Option<Input>,
    audio: Option<Input>,
}
fn streams(info: &Value) -> Sources {
    let formats: Vec<_> = info["formats"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|v| v["url"].as_str().is_some_and(public_address) && matches!(v["protocol"].as_str().unwrap_or("https"), "http" | "https"))
        .collect();
    let input = |f: Option<&&Value>| {
        f.map(|f| Input {
            url: f["url"].as_str().unwrap().into(),
            headers: f["http_headers"]
                .as_object()
                .map(|o| o.iter().filter_map(|(k, v)| v.as_str().map(|v| (k.clone(), v.into()))).collect()),
        })
    };
    let picture = |limit: f64| {
        let mut all: Vec<_> = formats
            .iter()
            .filter(|f| {
                f["vcodec"].as_str().is_some_and(|s| !s.is_empty() && s != "none")
                    && f["height"].as_f64().is_some_and(|h| h > 0.0 && h <= limit)
            })
            .collect();
        all.sort_by(|a, b| {
            b["height"].as_f64().unwrap().total_cmp(&a["height"].as_f64().unwrap()).then_with(|| {
                usize::from(b["vcodec"].as_str().unwrap_or("").starts_with("avc1"))
                    .cmp(&usize::from(a["vcodec"].as_str().unwrap_or("").starts_with("avc1")))
            })
        });
        all.first().copied()
    };
    let video = picture(720.0);
    let sharp = picture(1440.0);
    let has_audio = |f: &&Value| f["acodec"].as_str().is_some_and(|s| !s.is_empty() && s != "none");
    let mut sounds: Vec<_> =
        formats.iter().filter(|f| has_audio(f) && f["vcodec"].as_str().is_none_or(|s| s.is_empty() || s == "none")).collect();
    let rate = |f: &&&Value| f["abr"].as_f64().or_else(|| f["tbr"].as_f64()).unwrap_or(0.0);
    sounds.sort_by(|a, b| usize::from(b["ext"] == "m4a").cmp(&usize::from(a["ext"] == "m4a")).then(rate(b).total_cmp(&rate(a))));
    let combined = formats.iter().find(|f| has_audio(f) && f["vcodec"].as_str().is_some_and(|s| !s.is_empty() && s != "none"));
    Sources {
        video: input(video.or(combined)),
        sharp: input(sharp.or(video).or(combined)),
        audio: input(sounds.first().copied().or(combined)),
    }
}
async fn captions_for(
    info: &Value,
    url: &str,
    ytdlp: &str,
    folder: &str,
    signal: Option<Signal>,
) -> Result<(Vec<Cue>, Option<Track>, bool), VideoFailure> {
    let Some(track) = caption_track(info) else {
        return Ok((Vec::new(), None, false));
    };
    if let (Some(address), Some(ext)) = (&track.url, &track.ext) {
        if public_address(address) {
            let fetched = async {
                let response = reqwest::Client::new().get(address).header("User-Agent", "Mozilla/5.0").send().await?;
                let status = response.status();
                let body = if status.is_success() { response.text().await? } else { String::new() };
                Ok::<_, reqwest::Error>((status, body))
            };
            let result = tokio::select! {result=fetched=>Some(result),_=async{if let Some(signal)=&signal{signal.cancelled().await;}else{std::future::pending::<()>().await;}}=>None};
            match result {
                Some(Ok((status, _))) if status.as_u16() == 429 || status.as_u16() == 403 => return Ok((Vec::new(), Some(track), true)),
                Some(Ok((status, body))) if status.is_success() => {
                    let cues = parse_captions(&body, ext);
                    if !cues.is_empty() {
                        return Ok((cues, Some(track), false));
                    }
                }
                _ => {
                    if let Some(signal) = &signal {
                        signal.check()?;
                    }
                }
            }
        }
    }
    let scratch = join(folder, "captions");
    mkdir(&scratch).await?;
    let result: Result<Vec<Cue>, VideoFailure> = async {
        let mut args: Vec<String> = ["--skip-download", "--no-playlist", "--no-warnings"].into_iter().map(str::to_string).collect();
        args.extend(yt_dlp_extras(ytdlp, signal.clone()).await);
        args.extend([
            if track.automatic { "--write-auto-subs" } else { "--write-subs" }.into(),
            "--sub-langs".into(),
            track.code.clone(),
            "--sub-format".into(),
            "json3/vtt/srt/best".into(),
            "-o".into(),
            join(&scratch, "captions.%(ext)s"),
            "--".into(),
            url.into(),
        ]);
        run(ytdlp, &args, RunOptions { timeout_ms: Some(90_000), signal: signal.clone(), ..Default::default() }).await?;
        let mut entries = tokio::fs::read_dir(&scratch).await?;
        let mut names = Vec::new();
        while let Some(e) = entries.next_entry().await? {
            names.push(e.file_name().to_string_lossy().into_owned());
        }
        names.sort();
        for name in names {
            let ext = name.rsplit('.').next().unwrap_or("");
            if ["json3", "vtt", "srt"].contains(&ext) {
                let cues = parse_captions(&tokio::fs::read_to_string(join(&scratch, &name)).await?, ext);
                if !cues.is_empty() {
                    return Ok(cues);
                }
            }
        }
        Ok(Vec::new())
    }
    .await;
    let removed = tokio::fs::remove_dir_all(&scratch).await;
    if let Err(e) = removed {
        if e.kind() != std::io::ErrorKind::NotFound {
            return Err(e.into());
        }
    }
    match result {
        Ok(cues) => Ok((cues, Some(track), false)),
        Err(_) => {
            if let Some(signal) = &signal {
                signal.check()?;
            }
            Ok((Vec::new(), Some(track), false))
        }
    }
}
static PAGES: LazyLock<Mutex<indexmap::IndexMap<String, (Value, i64)>>> = LazyLock::new(|| Mutex::new(indexmap::IndexMap::new()));
/// Stretches of speech Kumi couldn't take or transcribe, and why, by turn: a closer look at the video
/// in the same answer doesn't wait for the same failure, and the next request tries again. A turn is
/// known by its signal, which each of its tools is given. The last few turns are kept, so turns that
/// overlap (tests running together, say) keep their own.
static UNHEARD: LazyLock<Mutex<Vec<(Signal, std::collections::HashMap<String, String>)>>> = LazyLock::new(Default::default);
const UNHEARD_TURNS: usize = 8;
fn unheard(turn: Option<&Signal>, stretch: &str) -> Option<String> {
    let turn = turn?;
    let turns = UNHEARD.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    turns.iter().find(|(known, _)| known.same_as(turn))?.1.get(stretch).cloned()
}
fn remember_unheard(turn: Option<&Signal>, stretch: &str, why: String) {
    let Some(turn) = turn else { return };
    let mut turns = UNHEARD.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let index = match turns.iter().position(|(known, _)| known.same_as(turn)) {
        Some(index) => index,
        None => {
            if turns.len() == UNHEARD_TURNS {
                turns.remove(0);
            }
            turns.push((turn.clone(), Default::default()));
            turns.len() - 1
        }
    };
    turns[index].1.insert(stretch.to_string(), why);
}
struct Watcher<'a> {
    options: &'a WatchOptions,
    address: String,
    file: Option<String>,
    ytdlp: Option<String>,
    info: Option<Value>,
    sources: Option<Sources>,
}
impl Watcher<'_> {
    fn progress(&self, text: &str) {
        if let Some(progress) = &self.options.on_progress {
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| progress(text)));
        }
    }
    fn programs(&self) -> ProgramOptions {
        ProgramOptions {
            env: self.options.env.clone(),
            tools_dir: self.options.tools_dir.clone(),
            signal: self.options.signal.clone(),
            on_fetch: self.options.on_fetch.clone(),
            ..Default::default()
        }
    }
    async fn ytdlp(&mut self) -> Result<String, VideoFailure> {
        if let Some(program) = &self.ytdlp {
            return Ok(program.clone());
        }
        let program = find_yt_dlp(&self.programs()).await?;
        self.ytdlp = Some(program.clone());
        Ok(program)
    }
    async fn page(&mut self) -> Result<Value, VideoFailure> {
        if let Some((info, at)) = PAGES.lock().unwrap().get(&self.address) {
            if now_ms() - at < 3_600_000 {
                return Ok(info.clone());
            }
        }
        let program = self.ytdlp().await?;
        self.progress("reading the video's page");
        let mut args: Vec<String> = ["-J", "--no-playlist", "--no-warnings"].into_iter().map(str::to_string).collect();
        args.extend(yt_dlp_extras(&program, self.options.signal.clone()).await);
        args.extend(["--".into(), self.address.clone()]);
        let result =
            run(&program, &args, RunOptions { timeout_ms: Some(120_000), signal: self.options.signal.clone(), ..Default::default() }).await;
        let text = match result {
            Ok(out) => out.stdout,
            Err(e) => {
                if let Some(signal) = &self.options.signal {
                    signal.check()?;
                }
                return Err(VideoFailure::video(format!("Kumi couldn't read that video's page: {e}")));
            }
        };
        let page: Value =
            serde_json::from_slice(&text).map_err(|_| VideoFailure::video("Kumi couldn't make sense of that video's page."))?;
        let mut pages = PAGES.lock().unwrap();
        pages.insert(self.address.clone(), (page.clone(), now_ms()));
        let keys: Vec<_> = pages.keys().cloned().collect();
        for key in keys {
            if pages.len() > 16 || pages.get(&key).is_some_and(|(_, at)| now_ms() - at >= 3_600_000) {
                pages.shift_remove(&key);
            }
        }
        Ok(page)
    }
    async fn streams(&mut self) -> Result<Sources, VideoFailure> {
        if let Some(sources) = &self.sources {
            return Ok(sources.clone());
        }
        let sources = if let Some(file) = &self.file {
            let input = Some(Input { url: file.clone(), headers: None });
            Sources { video: input.clone(), sharp: input.clone(), audio: input }
        } else {
            if self.info.is_none() {
                self.info = Some(self.page().await?);
            }
            streams(self.info.as_ref().unwrap())
        };
        self.sources = Some(sources.clone());
        Ok(sources)
    }
}

/// Watch a video's words, selected frames, close-ups and a requested stretch of its sound.
pub async fn watch_video(request: WatchRequest, options: WatchOptions) -> Result<Watched, VideoFailure> {
    use crate::core::contracts::WordsSource;
    let signal = options.signal.clone();
    let mut address = trim(&request.url).to_string();
    if address.to_ascii_lowercase().starts_with("file://") {
        address = url::Url::parse(&address)
            .map_err(|e| VideoFailure::other(e.to_string()))?
            .to_file_path()
            .map_err(|_| VideoFailure::other("File URL path must be absolute"))?
            .to_string_lossy()
            .into();
    }
    let remote = address.to_ascii_lowercase().starts_with("http://") || address.to_ascii_lowercase().starts_with("https://");
    let file = if remote {
        None
    } else {
        let file = audio_path(&address);
        if !Path::new(&file).exists() {
            return Err(VideoFailure::video(
                "There's no video there: give a video's address (a YouTube link, say) or a video file's path.",
            ));
        }
        let ext = Path::new(&file).extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default();
        if !VIDEO_EXTENSIONS.contains(&ext.to_lowercase().as_str()) {
            return Err(VideoFailure::video(format!(
                "{} isn't a video Kumi reads ({}).",
                if ext.is_empty() { "That file" } else { &ext },
                VIDEO_EXTENSIONS.join(", ")
            )));
        }
        Some(file)
    };
    let local_key = if let Some(file) = &file {
        let meta = std::fs::metadata(file)?;
        let time = meta.modified()?.duration_since(UNIX_EPOCH).unwrap_or_default();
        let ms = time.as_secs() as f64 * 1000.0 + time.subsec_nanos() as f64 / 1_000_000.0;
        Some(format!("file-{}", &hex::encode(Sha256::digest(format!("{file}|{}|{}", meta.len(), to_string(ms))))[..16]))
    } else {
        None
    };
    let known = local_key.clone().or_else(|| youtube_id(&address).map(|id| format!("youtube-{id}")));
    let mut meta: Option<VideoMeta> =
        if let Some(key) = &known { read_json(&join(&join(&options.videos_dir, key), "meta.json")).await } else { None };
    let mut cues: Option<Vec<Cue>> =
        if let Some(key) = &known { read_json(&join(&join(&options.videos_dir, key), "cues.json")).await } else { None };
    let mut watcher = Watcher { options: &options, address: address.clone(), file: file.clone(), ytdlp: None, info: None, sources: None };
    let mut notes = Vec::new();
    let mut refused = false;
    if meta.is_none() || cues.is_none() {
        if let Some(file) = &file {
            let ffmpeg = find_ffmpeg(FfmpegOptions { env: options.env.clone(), signal: signal.clone(), ..Default::default() }).await?;
            let duration = if let Some(ffmpeg) = ffmpeg { duration_of(&ffmpeg, file, signal.clone()).await } else { None };
            let path = Path::new(file);
            let stem = path.file_stem().unwrap_or_default().to_string_lossy();
            let mut found = VideoMeta {
                version: 1,
                key: local_key.clone().unwrap(),
                url: file.clone(),
                title: stem.into_owned(),
                channel: None,
                duration,
                chapters: Vec::new(),
                words: None,
            };
            let mut words = Vec::new();
            for ext in ["srt", "vtt"] {
                let sidecar = path.with_extension(ext);
                if sidecar.exists() {
                    words = parse_captions(&tokio::fs::read_to_string(sidecar).await?, ext);
                    found.words = Some(Words { language: String::new(), source: WordsSource::Captions });
                    break;
                }
            }
            meta = Some(found);
            cues = Some(words);
        } else {
            let info = watcher.page().await?;
            let key = format!(
                "{}-{}",
                safe(info["extractor_key"].as_str().unwrap_or("video")),
                safe(
                    &info["id"]
                        .as_str()
                        .map(str::to_string)
                        .unwrap_or_else(|| hex::encode(Sha256::digest(address.as_bytes()))[..16].into())
                )
            );
            let folder = join(&options.videos_dir, &key);
            meta = read_json(&join(&folder, "meta.json")).await;
            cues = if meta.is_some() { read_json(&join(&folder, "cues.json")).await } else { None };
            if meta.is_none() || cues.is_none() {
                mkdir(&folder).await?;
                watcher.progress("reading the captions");
                let ytdlp = watcher.ytdlp().await?;
                let (words, track, denied) = captions_for(&info, &address, &ytdlp, &folder, signal.clone()).await?;
                refused = denied;
                let captions = if words.is_empty() {
                    None
                } else {
                    track.map(|t| Words {
                        language: t.code,
                        source: if t.automatic { WordsSource::Automatic } else { WordsSource::Captions },
                    })
                };
                let channel = info.get("channel").filter(|v| !v.is_null()).or_else(|| info.get("uploader"));
                meta = Some(VideoMeta {
                    version: 1,
                    key,
                    url: info["webpage_url"].as_str().unwrap_or(&address).into(),
                    title: head(info["title"].as_str().unwrap_or("Untitled video"), 200),
                    channel: channel
                        .filter(|v| v.as_str().is_none_or(|s| !s.is_empty()) && !v.is_null())
                        .map(|v| head(&crate::web::net::js_string(v), 120)),
                    duration: info["duration"].as_f64(),
                    chapters: info["chapters"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(|c| {
                            c["start_time"].as_f64().map(|start| Chapter {
                                start,
                                title: head(
                                    &c.get("title").filter(|v| !v.is_null()).map(crate::web::net::js_string).unwrap_or_default(),
                                    120,
                                ),
                            })
                        })
                        .take(64)
                        .collect(),
                    words: captions,
                });
                cues = Some(words);
            }
            watcher.info = Some(info);
        }
    }
    let mut meta = meta.unwrap();
    let mut cues = cues.unwrap();
    let folder = join(&options.videos_dir, &meta.key);
    mkdir(&folder).await?;
    let end = meta.duration.unwrap_or_else(|| cues.iter().map(|c| c.end).fold(0.0, f64::max));
    let from = request.from.unwrap_or(0.0).min(end).max(0.0);
    let to = request.to.unwrap_or(end).min(if end != 0.0 { end } else { f64::INFINITY }).max(from);
    let ffmpeg = find_ffmpeg(FfmpegOptions { env: options.env.clone(), signal: signal.clone(), ..Default::default() }).await?;
    if cues.is_empty() && meta.words.is_none() {
        let whisper = if ffmpeg.is_some() { find_whisper(&watcher.programs()).await.unwrap_or(None) } else { None };
        let why = if refused {
            "YouTube turned away the request for its captions"
        } else if file.is_some() {
            "it has no captions beside it (a .srt or .vtt of the same name)"
        } else {
            "it has no captions"
        };
        if ffmpeg.is_none() {
            notes.push(format!("There's no transcript: {why}, and transcribing its speech needs ffmpeg ({}).", ffmpeg_hint()));
        } else if whisper.is_none() {
            notes.push(format!(
                "There's no transcript: {why}, and Kumi transcribes speech with whisper.cpp ({}). The frames show what it does.",
                whisper_hint()
            ));
        } else if let Some(audio) = watcher.streams().await?.audio {
            let language = watcher.info.as_ref().and_then(|info| info["language"].as_str()).unwrap_or("en").to_string();
            let model = whisper_model(speech_model_for(Some(&language)), &watcher.programs()).await?;
            let whole = end == 0.0 || end <= 5400.0;
            let start = if whole { 0.0 } else { from };
            let stop = if whole {
                if end == 0.0 {
                    5400.0
                } else {
                    end
                }
            } else {
                to.min(from + 5400.0)
            };
            let stretch = format!("{}|{}-{}", meta.key, to_fixed(start, 0), to_fixed(stop, 0));
            let heard = if let Some(why) = unheard(signal.as_ref(), &stretch) {
                notes.push(format!(
                    "Kumi couldn't transcribe the video's speech earlier in this request ({why}), so it didn't try again; it will on the next request."
                ));
                Ok(None)
            } else {
                watcher.progress("taking the video's speech");
                // Taking the speech can fail as transcribing it can (a stream that stalls, say): either way,
                // the frames still come.
                match sound_between(
                    ffmpeg.as_deref().unwrap(),
                    &audio,
                    start,
                    stop,
                    &join(&folder, &format!("speech-{}-{}.wav", to_fixed(start, 0), to_fixed(stop, 0))),
                    signal.clone(),
                    true,
                )
                .await
                {
                    Ok(wav) => {
                        watcher.progress("transcribing what's said");
                        let on_progress = options.on_progress.clone();
                        let heard = transcribe(
                            whisper.as_deref().unwrap(),
                            &model,
                            &wav,
                            TranscribeOptions {
                                language: Some(language.clone()),
                                prompt: Some(speech_prompt(&meta.title)),
                                signal: signal.clone(),
                                on_progress: Some(Rc::new(move |percent| {
                                    if let Some(progress) = &on_progress {
                                        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                                            progress(&format!("transcribing what's said · {}%", to_string(percent)))
                                        }));
                                    }
                                })),
                                ..Default::default()
                            },
                        )
                        .await;
                        match tokio::fs::remove_file(&wav).await {
                            Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(e.into()),
                            _ => {}
                        }
                        heard.map(Some)
                    }
                    Err(error) => Err(error),
                }
                .inspect_err(|error| {
                    if !error.is_aborted() {
                        remember_unheard(signal.as_ref(), &stretch, head(&error.to_string(), 160));
                    }
                })
            };
            match heard {
                Ok(None) => {}
                Ok(Some(heard)) => {
                    cues = heard
                        .into_iter()
                        .map(|mut c| {
                            c.start += start;
                            c.end += start;
                            c
                        })
                        .collect();
                    if whole {
                        meta.words = Some(Words { language, source: WordsSource::Transcribed });
                    } else {
                        notes.push(format!(
                            "Kumi transcribed {}–{} of this long video; ask for a later stretch to hear more.",
                            format_time(start),
                            format_time(stop)
                        ));
                    }
                }
                Err(error) => {
                    if let Some(signal) = &signal {
                        signal.check()?;
                    }
                    notes.push(format!("Kumi couldn't transcribe the video's speech ({}).", head(&error.to_string(), 160)));
                }
            }
        } else {
            notes.push(format!("There's no transcript: {why}, and Kumi couldn't find the video's sound to transcribe."));
        }
    }
    if meta.words.is_some() || cues.is_empty() {
        let empty = Vec::new();
        write_json(&join(&folder, "cues.json"), if meta.words.is_some() { &cues } else { &empty }).await?;
    }
    let meta_path = join(&folder, "meta.json");
    write_json(&meta_path, &meta).await?;
    if let Ok(file) = std::fs::File::options().write(true).open(&meta_path) {
        let now = SystemTime::now();
        let _ = file.set_times(std::fs::FileTimes::new().set_accessed(now).set_modified(now));
    }
    prune(&options.videos_dir).await?;
    let mut lines = Vec::new();
    let mut size = 0;
    let mut cut_at = None;
    for line in transcript_lines(&cues, TranscriptOptions { from: Some(from), to: Some(to), chars: None }) {
        if size + utf16_len(&line.text) > 24_000 {
            cut_at = Some(line.at);
            break;
        }
        size += utf16_len(&line.text) + 12;
        lines.push(line);
    }
    let region = request.zoom;
    let wanted = if let Some(look) = request.look_at.filter(|v| !v.is_empty()) {
        let mut unique = Vec::new();
        for at in look.into_iter().filter(|t| *t >= 0.0 && (end == 0.0 || *t <= end)).map(|t| round(t * 10.0) / 10.0) {
            if !unique.contains(&at) {
                unique.push(at);
            }
            if unique.len() == 12 {
                break;
            }
        }
        unique
    } else {
        choose_moments(
            &cues,
            MomentOptions {
                from,
                to,
                count: request.frames.unwrap_or(if cues.is_empty() { 12.0 } else { 8.0 }),
                chapters: meta.chapters.clone(),
            },
        )
    };
    let mut frames = Vec::new();
    let mut sound = None;
    let listen = request.listen.filter(|span| span.to > span.from);
    if (!wanted.is_empty() || listen.is_some()) && ffmpeg.is_none() {
        notes.push(format!("Frames and the video's sound need ffmpeg ({}); this is the transcript alone.", ffmpeg_hint()));
    } else if let Some(ffmpeg) = ffmpeg {
        let frame_path = |time| {
            join(
                &join(&folder, "frames"),
                &format!("{}{}.jpg", to_fixed(time, 1), region.map_or_else(String::new, |r| format!("-{}", r.as_str()))),
            )
        };
        let missing = wanted.iter().any(|time| !Path::new(&frame_path(*time)).exists());
        let input = if missing {
            let sources = watcher.streams().await?;
            if region.is_some() {
                sources.sharp
            } else {
                sources.video
            }
        } else {
            None
        };
        if missing && input.is_none() {
            notes.push("Kumi couldn't find a stream of that video to take frames from; this is the transcript alone.".into());
        } else {
            for chunk in wanted.chunks(3) {
                let mut tasks = FuturesUnordered::new();
                for time in chunk {
                    watcher.progress(&format!("looking at {}", format_time(*time)));
                    let path = frame_path(*time);
                    let input = input.as_ref();
                    let ffmpeg = &ffmpeg;
                    let signal = signal.clone();
                    tasks.push(async move { (*time, frame_at(ffmpeg, input, *time, &path, signal, region).await) });
                }
                while let Some((time, frame)) = tasks.next().await {
                    match frame {
                        Ok(frame) => frames.push(WatchedFrame {
                            at: time,
                            said: said_around(&cues, time, None),
                            region,
                            jpeg: frame.jpeg,
                            thumb: frame.thumb,
                        }),
                        Err(error) => {
                            if let Some(signal) = &signal {
                                signal.check()?;
                            }
                            notes.push(format!(
                                "Kumi couldn't take the frame at {} ({}).",
                                format_time(time),
                                head(&error.to_string(), 120)
                            ));
                        }
                    }
                }
            }
            frames.sort_by(|a, b| a.at.total_cmp(&b.at));
        }
        if let Some(listen) = listen {
            let start = listen.from.max(0.0);
            let stop = listen.to.min(start + MAX_SOUND).min(if end == 0.0 { f64::INFINITY } else { end });
            let audio = if stop > start { watcher.streams().await?.audio } else { None };
            if let Some(audio) = audio {
                watcher.progress(&format!("taking the sound at {}–{}", format_time(start), format_time(stop)));
                match sound_between(
                    &ffmpeg,
                    &audio,
                    start,
                    stop,
                    &join(&join(&folder, "sound"), &format!("{}-{}.wav", to_fixed(start, 1), to_fixed(stop, 1))),
                    signal.clone(),
                    false,
                )
                .await
                {
                    Ok(file) => sound = Some(Sound { file, from: start, to: stop }),
                    Err(error) => {
                        if let Some(signal) = &signal {
                            signal.check()?;
                        }
                        notes.push(format!(
                            "Kumi couldn't take the sound at {}–{} ({}).",
                            format_time(start),
                            format_time(stop),
                            head(&error.to_string(), 120)
                        ));
                    }
                }
            } else {
                notes.push("Kumi couldn't find the video's sound to take.".into());
            }
        }
    }
    Ok(Watched { meta, from, to, lines, cut_at, frames, sound, notes })
}
