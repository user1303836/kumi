//! `watch`: what a YouTube video the producer names says and shows, for the host's model (CC-21).
//!
//! - Only yt-dlp touches the network: the copy setup pinned and passed as `--tool yt-dlp`, run isolated in scratch
//!   (its config, plugins, cache and home are the scratch folder's), and given only the video's canonical URL.
//! - The page (`-J`) and the captions are read inline. The page is kept per video for a day.
//! - The picture is downloaded once per video by a `watch` job: one video-only https file, at most 720p and 150 MB,
//!   which serve watches grow and stops past the cap. A second call for the same video joins it; once no call waits
//!   for it, it's cancelled. HLS, through yt-dlp's own downloader, is the fallback when no https file fits.
//! - ffmpeg (CC-10's) reads frames from that one local file only. Each frame is a JPEG at most 1280 px wide and
//!   256 KB, kept as an artifact; a chat gets at most 24 of a video.
//! - `<state>/videos/youtube-<id>/` keeps the page, the picture and each chat's frame count: 1 GB in all, the least
//!   recently used video out first.

use super::ctx::{make_private_dir, Call, Ctx, Effect, JsonObject, OpError, OpFuture, Operation, Scratch};
use super::jobs::JobHandle;
use kumi_common::abort::{self, Signal};
use kumi_common::time::now_ms;
use kumi_media::captions::{format_time, parse_captions, transcript_lines, Cue, TranscriptOptions};
use kumi_media::frames::{Region, REGIONS};
use kumi_media::programs::{self, RunOptions, RunOutput, VideoFailure};
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, HashMap};
use std::ffi::{OsStr, OsString};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::LazyLock;
use std::time::{Duration, SystemTime};
use tokio::sync::watch as channel;

pub const WATCH: Operation = Operation {
    name: "watch",
    description: "Read a YouTube video the producer named, by its https link: its title, length, chapters and caption \
                  languages; with captions, its words; with at (up to 4 moments, in seconds), a JPEG frame at each, \
                  optionally a close-up region. Fetches from YouTube with the yt-dlp setup installed; the first frames of \
                  a video download a copy of its picture (up to 150 MB). Changes nothing in the DAW.",
    schema,
    effect: fetches,
    run,
};

const PAGE_MS: u64 = 120_000;
const CAPTIONS_MS: u64 = 90_000;
/// The download's wall clock.
const DOWNLOAD_MS: u64 = 600_000;
const FRAME_MS: u64 = 60_000;
/// The most `-J` may say.
const PAGE_MOST: usize = 16 * 1024 * 1024;
const CAPTION_FILE_MOST: u64 = 16 * 1024 * 1024;
/// The picture's cap, as yt-dlp's `--max-filesize 150M` counts it.
const STREAM_MOST: u64 = 150 * 1024 * 1024;
const STREAM_HEIGHT_MOST: u64 = 720;
const TRANSCRIPT_MOST: usize = 24_000;
const MOMENTS_MOST: usize = 4;
const LATEST_SECONDS: f64 = 86_400.0;
const FRAMES_PER_CHAT: u64 = 24;
const JPEG_MOST: u64 = 256 * 1024;
/// Each try at a frame's JPEG: the widest it may be and ffmpeg's quality, smaller until it fits [`JPEG_MOST`].
const ENCODINGS: [(u32, u8); 3] = [(1280, 3), (960, 8), (640, 16)];
const CACHE_MOST: u64 = 1024 * 1024 * 1024;
const PAGE_FRESH_MS: i64 = 24 * 60 * 60 * 1000;
const GROWTH_EVERY: Duration = Duration::from_millis(500);
/// What ffmpeg may read the picture as: mp4 and webm.
const VIDEO_EXTENSIONS: [&str; 2] = ["mp4", "webm"];
/// Every yt-dlp run's: no config, plugins or cache of the producer's, one video, and no post-processing (so yt-dlp
/// never runs ffmpeg itself).
const ISOLATION: [&str; 8] =
    ["--ignore-config", "--no-plugin-dirs", "--no-cache-dir", "--no-playlist", "--no-warnings", "--no-progress", "--fixup", "never"];
/// The proxies yt-dlp keeps from serve's environment, as a standalone run inherits them.
const PROXIES: [&str; 6] = ["HTTP_PROXY", "HTTPS_PROXY", "NO_PROXY", "http_proxy", "https_proxy", "no_proxy"];

fn schema() -> Value {
    let regions: Vec<&str> = REGIONS.iter().map(|(name, _)| *name).collect();
    json!({
        "type": "object", "additionalProperties": false, "required": ["url"],
        "properties": {
            "url": {"type": "string", "minLength": 1, "maxLength": 2048},
            "captions": {"type": "boolean"},
            "at": {"type": "array", "items": {"type": "number", "minimum": 0, "maximum": LATEST_SECONDS}, "maxItems": MOMENTS_MOST},
            "region": {"type": "string", "enum": regions}
        }
    })
}

fn fetches(_: &JsonObject) -> Effect {
    Effect::Fetch
}

/// The downloads of videos' pictures running now, by video id: a second call for a video joins its download.
#[derive(Default)]
pub struct Watching {
    downloads: RefCell<HashMap<String, Rc<Download>>>,
}

struct Download {
    job: String,
    done: channel::Receiver<Option<Result<Downloaded, OpError>>>,
    /// The calls waiting for it.
    waiting: Cell<usize>,
}

#[derive(Clone, Debug, PartialEq)]
struct Downloaded {
    path: PathBuf,
    note: String,
}

/// One call waiting for a download: when the last stops waiting before it's done, the download is cancelled.
struct Waiting<'a> {
    ctx: &'a Ctx,
    download: Rc<Download>,
}

impl Drop for Waiting<'_> {
    fn drop(&mut self) {
        let left = self.download.waiting.get().saturating_sub(1);
        self.download.waiting.set(left);
        if left == 0 && self.download.done.borrow().is_none() {
            self.ctx.jobs().cancel(&self.download.job, "watch");
        }
    }
}

struct Asked {
    id: String,
    captions: bool,
    at: Vec<f64>,
    region: Option<Region>,
}

fn run(ctx: &Ctx, call: Call) -> OpFuture<'_> {
    Box::pin(async move {
        let asked = asked(&call.args)?;
        let ytdlp = program(ctx, "yt-dlp").ok_or_else(|| refused("no_yt_dlp", "yt-dlp isn't set up: run Kumi's setup to read videos."))?;
        let ffmpeg = match asked.at.is_empty() {
            true => None,
            false => Some(program(ctx, "ffmpeg").ok_or_else(|| {
                refused("no_ffmpeg", "Frames need the ffmpeg allowed at Kumi's setup: allow it there, or ask for the captions.")
            })?),
        };
        let folder = video_folder(ctx, &asked.id)?;
        let chat = call.host.session_id.clone().unwrap_or_default();
        if !asked.at.is_empty() {
            let seen = frames_seen(&folder, &chat);
            if seen + asked.at.len() as u64 > FRAMES_PER_CHAT {
                let left = FRAMES_PER_CHAT.saturating_sub(seen);
                return Err(refused(
                    "frame_limit",
                    format!("A chat gets at most {FRAMES_PER_CHAT} frames of a video, and this one has had {seen}: ask for {left} or fewer."),
                ));
            }
        }
        check(&call.signal)?;
        let url = canonical(&asked.id);
        let scratch = ctx.scratch("watch")?;
        let mut notes = Vec::new();
        call.progress.report(0.0, None, Some("Reading the video's page"));
        let page = page(ytdlp, &url, &asked.id, &folder, scratch.path(), &call.signal).await?;
        let transcript = match asked.captions {
            true => {
                call.progress.report(1.0, None, Some("Reading the video's captions"));
                transcript(ytdlp, &url, &page, scratch.path(), &call.signal, &mut notes).await?
            }
            false => None,
        };
        let frames = match ffmpeg {
            Some(ffmpeg) => {
                let taking = Frames { ctx, call: &call, ytdlp, ffmpeg, url: &url, page: &page, folder: &folder, scratch: &scratch };
                taking.take(&asked, &chat, &mut notes).await?
            }
            None => Vec::new(),
        };
        used(&folder);
        prune(ctx, &folder).await;
        Ok(json!({"video": page.view(&url), "transcript": transcript, "frames": frames, "notes": notes}))
    })
}

fn asked(args: &JsonObject) -> Result<Asked, OpError> {
    let invalid = |reason: &str| OpError::Invalid { reason: reason.into() };
    if args.keys().any(|key| !matches!(key.as_str(), "url" | "captions" | "at" | "region")) {
        return Err(invalid("watch takes only url, captions, at and region."));
    }
    let url = args.get("url").and_then(Value::as_str).ok_or_else(|| invalid("watch needs the video's url."))?;
    let id = youtube_id(url).ok_or_else(|| {
        refused(
            "not_youtube",
            "Kumi reads only YouTube videos, by their https link: youtube.com/watch?v=…, youtu.be/… or youtube.com/shorts/….",
        )
    })?;
    let captions = match args.get("captions") {
        None => false,
        Some(Value::Bool(captions)) => *captions,
        Some(_) => return Err(invalid("captions is true or false.")),
    };
    let at = match args.get("at") {
        None => Vec::new(),
        Some(Value::Array(moments)) if moments.len() <= MOMENTS_MOST => moments
            .iter()
            .map(|moment| moment.as_f64().filter(|seconds| seconds.is_finite() && (0.0..=LATEST_SECONDS).contains(seconds)))
            .collect::<Option<Vec<f64>>>()
            .ok_or_else(|| invalid("Each moment in at is a number of seconds, from 0 to 86400."))?,
        Some(_) => return Err(invalid("at is a list of up to 4 moments, in seconds.")),
    };
    let region = match args.get("region") {
        None => None,
        Some(region) => Some(region.as_str().and_then(Region::parse).ok_or_else(|| {
            invalid("region is top, bottom, left, right, center, top-left, top-right, bottom-left or bottom-right.")
        })?),
    };
    Ok(Asked { id, captions, at, region })
}

/// The video's id, from an https link to it on YouTube in one of its three forms; None for anything else.
fn youtube_id(url: &str) -> Option<String> {
    static FORMS: LazyLock<[Regex; 3]> = LazyLock::new(|| {
        [
            Regex::new(r"^https://(?:www\.|m\.)?youtube\.com/watch\?(?:[^#]*&)?v=([A-Za-z0-9_-]{11})(?:$|[&#])").unwrap(),
            Regex::new(r"^https://(?:www\.|m\.)?youtube\.com/shorts/([A-Za-z0-9_-]{11})(?:$|[?#/])").unwrap(),
            Regex::new(r"^https://youtu\.be/([A-Za-z0-9_-]{11})(?:$|[?#/])").unwrap(),
        ]
    });
    FORMS.iter().find_map(|form| form.captures(url)).map(|found| found[1].to_string())
}

/// The only URL yt-dlp is ever given: rebuilt from the id, so nothing the model wrote reaches it.
fn canonical(id: &str) -> String {
    format!("https://www.youtube.com/watch?v={id}")
}

fn program<'a>(ctx: &'a Ctx, name: &str) -> Option<&'a str> {
    ctx.media().tool(name).and_then(Path::to_str)
}

fn os(text: impl AsRef<OsStr>) -> OsString {
    text.as_ref().to_owned()
}

/// yt-dlp, isolated in `scratch`, for one step, on the canonical `url` after `--`.
async fn yt_dlp(
    program: &str,
    step: Vec<OsString>,
    url: &str,
    scratch: &Path,
    timeout_ms: u64,
    max_buffer: usize,
    signal: Signal,
) -> Result<RunOutput, VideoFailure> {
    let mut args: Vec<OsString> = ISOLATION.iter().map(os).collect();
    args.extend(step);
    args.extend([os("--"), os(url)]);
    let options = RunOptions {
        signal: Some(signal),
        timeout_ms: Some(timeout_ms),
        max_buffer: Some(max_buffer),
        isolated_dir: Some(scratch.to_path_buf()),
        keep_env: &PROXIES,
    };
    programs::run(program, &args, options).await
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct Page {
    id: String,
    title: String,
    duration: Option<f64>,
    language: Option<String>,
    chapters: Vec<Chapter>,
    /// By language code, each `[{ext}]`, as `-J` lists them (without their URLs).
    subtitles: Value,
    automatic_captions: Value,
    formats: Vec<Format>,
    /// When it was read (ms).
    at: i64,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
struct Chapter {
    title: String,
    start: f64,
    end: Option<f64>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
struct Format {
    format_id: String,
    protocol: String,
    vcodec: String,
    acodec: String,
    ext: String,
    height: Option<u64>,
    /// `filesize`, or else `filesize_approx`.
    size: Option<u64>,
}

impl Page {
    fn of(info: &Value, id: &str) -> Option<Page> {
        if info["id"].as_str() != Some(id) {
            return None;
        }
        let text = |value: &Value| value.as_str().unwrap_or_default().to_string();
        let number = |value: &Value| value.as_f64().filter(|number| number.is_finite() && *number >= 0.0);
        let tracks = |value: &Value| {
            let mut kept = serde_json::Map::new();
            for (code, list) in value.as_object().into_iter().flatten() {
                let formats = list.as_array().into_iter().flatten().filter_map(|item| item["ext"].as_str()).map(|ext| json!({"ext": ext}));
                kept.insert(code.clone(), Value::Array(formats.collect()));
            }
            Value::Object(kept)
        };
        let chapters = info["chapters"].as_array().into_iter().flatten();
        let formats = info["formats"].as_array().into_iter().flatten();
        Some(Page {
            id: id.to_string(),
            title: text(&info["title"]),
            duration: number(&info["duration"]),
            language: info["language"].as_str().map(str::to_string),
            chapters: chapters
                .filter_map(|chapter| {
                    Some(Chapter { title: text(&chapter["title"]), start: number(&chapter["start_time"])?, end: number(&chapter["end_time"]) })
                })
                .collect(),
            subtitles: tracks(&info["subtitles"]),
            automatic_captions: tracks(&info["automatic_captions"]),
            formats: formats
                .map(|format| Format {
                    format_id: text(&format["format_id"]),
                    protocol: text(&format["protocol"]),
                    vcodec: text(&format["vcodec"]),
                    acodec: text(&format["acodec"]),
                    ext: text(&format["ext"]),
                    height: format["height"].as_u64(),
                    size: number(&format["filesize"]).or_else(|| number(&format["filesize_approx"])).map(|size| size as u64),
                })
                .collect(),
            at: now_ms(),
        })
    }

    /// The caption languages worth naming: the producer-written ones, then YouTube's own in the video's language.
    fn caption_languages(&self) -> Vec<String> {
        let spoken = self.language.as_deref().unwrap_or_default().split(['-', '_']).next().unwrap_or_default().to_lowercase();
        let codes = |tracks: &Value| tracks.as_object().into_iter().flatten().map(|(code, _)| code.clone()).collect::<Vec<_>>();
        let mut languages: Vec<String> = codes(&self.subtitles).into_iter().filter(|code| !code.contains("live_chat")).collect();
        for code in codes(&self.automatic_captions) {
            let own = code.ends_with("-orig") || code.to_lowercase() == spoken;
            if own && !languages.contains(&code) {
                languages.push(code);
            }
        }
        languages
    }

    fn view(&self, url: &str) -> Value {
        json!({
            "id": self.id, "url": url, "title": self.title, "duration": self.duration, "chapters": self.chapters,
            "captions": self.caption_languages(),
        })
    }
}

impl Format {
    fn fits(&self, protocols: &[&str]) -> bool {
        protocols.contains(&self.protocol.as_str())
            && self.acodec == "none"
            && !matches!(self.vcodec.as_str(), "" | "none")
            && self.height.is_some_and(|height| height > 0 && height <= STREAM_HEIGHT_MOST)
            && self.size.is_some_and(|size| size <= STREAM_MOST)
            && VIDEO_EXTENSIONS.contains(&self.ext.as_str())
            && !self.format_id.is_empty()
            && self.format_id.len() <= 32
            && !self.format_id.starts_with('-')
            && self.format_id.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.'))
    }

    /// Every ffmpeg decodes avc1; an ffmpeg without dav1d can't read av01.
    fn codec_rank(&self) -> u8 {
        match self.vcodec.split('.').next().unwrap_or_default() {
            "avc1" => 0,
            "vp9" | "vp09" => 1,
            "av01" => 2,
            _ => 3,
        }
    }
}

/// The picture to download: the tallest video-only single https file at most 720p within the cap, avc1 first; else
/// the same over HLS (true), which yt-dlp's own downloader writes as one local file.
fn stream_format(formats: &[Format]) -> Result<(Format, bool), OpError> {
    for (protocols, hls) in [(&["https"][..], false), (&["m3u8_native", "m3u8"][..], true)] {
        let best = formats
            .iter()
            .filter(|format| format.fits(protocols))
            .min_by_key(|format| (std::cmp::Reverse(format.height), format.codec_rank(), format.size));
        if let Some(best) = best {
            return Ok((best.clone(), hls));
        }
    }
    Err(refused(
        "too_large",
        "This video has no picture Kumi can keep for frames (one video-only file at 720p or less, up to 150 MB): it's \
         too long for Kumi's frame limit. Ask for its captions instead.",
    ))
}

/// The video's page: the one kept for it if it's under a day old, else read with `-J`.
async fn page(ytdlp: &str, url: &str, id: &str, folder: &Path, scratch: &Path, signal: &Signal) -> Result<Page, OpError> {
    let kept = folder.join("meta.json");
    let fresh = std::fs::read(&kept)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Page>(&bytes).ok())
        .filter(|page| page.id == id && (0..PAGE_FRESH_MS).contains(&(now_ms() - page.at)));
    if let Some(page) = fresh {
        return Ok(page);
    }
    let output = yt_dlp(ytdlp, vec![os("-J")], url, scratch, PAGE_MS, PAGE_MOST, signal.clone())
        .await
        .map_err(|failure| fetch_error("the video's page", failure))?;
    let info: Value =
        serde_json::from_slice(&output.stdout).map_err(|_| refused("fetch_failed", "yt-dlp's answer about the video wasn't readable."))?;
    let page = Page::of(&info, id).ok_or_else(|| refused("fetch_failed", "yt-dlp answered about another video than the one asked."))?;
    write_private(&kept, &serde_json::to_vec(&page).map_err(failed)?).map_err(failed)?;
    Ok(page)
}

/// The video's words, through yt-dlp: None, with a note, when it has no captions or YouTube won't give them.
async fn transcript(
    ytdlp: &str,
    url: &str,
    page: &Page,
    scratch: &Path,
    signal: &Signal,
    notes: &mut Vec<String>,
) -> Result<Option<Value>, OpError> {
    let info = json!({"language": page.language, "subtitles": page.subtitles, "automatic_captions": page.automatic_captions});
    let named = |code: &str| {
        code.len() <= 32
            && code.bytes().next().is_some_and(|b| b.is_ascii_alphanumeric())
            && code.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
    };
    let Some(track) = kumi_web::video::caption_track(&info).filter(|track| named(&track.code)) else {
        notes.push("This video has no captions, so there are no words from it.".into());
        return Ok(None);
    };
    let into = scratch.join("captions");
    make_private_dir(&into).map_err(failed)?;
    let step = vec![
        os(if track.automatic { "--write-auto-subs" } else { "--write-subs" }),
        os("--skip-download"),
        os("--sub-langs"),
        os(&track.code),
        os("--sub-format"),
        os("json3/vtt/srt/best"),
        os("-o"),
        os(into.join("%(id)s.%(ext)s")),
    ];
    match yt_dlp(ytdlp, step, url, scratch, CAPTIONS_MS, 1024 * 1024, signal.clone()).await {
        Ok(_) => {}
        Err(VideoFailure::Aborted) => return Err(cancelled()),
        Err(failure) => {
            notes.push(format!("YouTube didn't give the video's captions: {}", failure.message()));
            return Ok(None);
        }
    }
    let cues = cues_in(&into);
    if cues.is_empty() {
        notes.push("The video's captions held no words.".into());
        return Ok(None);
    }
    let (text, truncated) = transcript_text(&cues);
    if truncated {
        notes.push(format!("The captions are cut at {TRANSCRIPT_MOST} characters."));
    }
    let kind = if track.automatic { "auto" } else { "manual" };
    Ok(Some(json!({"language": track.code, "kind": kind, "text": text, "truncated": truncated})))
}

/// The cues of the first caption file yt-dlp wrote in `folder` that has any.
fn cues_in(folder: &Path) -> Vec<Cue> {
    let mut files: Vec<PathBuf> = std::fs::read_dir(folder).into_iter().flatten().filter_map(Result::ok).map(|entry| entry.path()).collect();
    files.sort();
    for file in files {
        let Some(ext) = file.extension().and_then(OsStr::to_str).filter(|ext| ["json3", "vtt", "srt"].contains(ext)) else { continue };
        let small = std::fs::symlink_metadata(&file).is_ok_and(|meta| meta.is_file() && meta.len() <= CAPTION_FILE_MOST);
        let Some(bytes) = small.then(|| std::fs::read(&file).ok()).flatten() else { continue };
        let cues = parse_captions(&kumi_web::video::caption_text(&bytes), ext);
        if !cues.is_empty() {
            return cues;
        }
    }
    Vec::new()
}

/// The cues as timed lines, `[m:ss] words`, cut at a line before [`TRANSCRIPT_MOST`] characters (true when cut).
fn transcript_text(cues: &[Cue]) -> (String, bool) {
    let mut text = String::new();
    let mut length = 0;
    for line in transcript_lines(cues, TranscriptOptions::default()) {
        let line = format!("[{}] {}", format_time(line.at), line.text);
        let added = line.chars().count() + usize::from(!text.is_empty());
        if length + added > TRANSCRIPT_MOST {
            return (text, true);
        }
        if !text.is_empty() {
            text.push('\n');
        }
        text.push_str(&line);
        length += added;
    }
    (text, false)
}

/// What a call's frames are taken with.
struct Frames<'a> {
    ctx: &'a Ctx,
    call: &'a Call,
    ytdlp: &'a str,
    ffmpeg: &'a str,
    url: &'a str,
    page: &'a Page,
    folder: &'a Path,
    scratch: &'a Scratch,
}

impl Frames<'_> {
    /// A frame at each moment asked, each kept as an artifact: contract 4's records. A moment ffmpeg can't take is a
    /// note, not a failure.
    async fn take(&self, asked: &Asked, chat: &str, notes: &mut Vec<String>) -> Result<Vec<Value>, OpError> {
        let (ctx, signal) = (self.ctx, &self.call.signal);
        let moments = moments(&asked.at, self.page.duration, notes);
        let video = match cached_video(self.folder) {
            Some(video) => video,
            None => {
                self.call.progress.report(2.0, None, Some("Downloading the video's picture"));
                let downloaded = self.downloaded().await?;
                notes.push(downloaded.note);
                downloaded.path
            }
        };
        let into = self.scratch.path().join("frames");
        make_private_dir(&into).map_err(failed)?;
        let mut seen = frames_seen(self.folder, chat);
        let mut frames = Vec::with_capacity(moments.len());
        for (n, at) in moments.into_iter().enumerate() {
            check(signal)?;
            self.call.progress.report(3.0 + n as f64, None, Some("Taking the video's frames"));
            let out = into.join(format!("{n}.jpg"));
            match frame(self.ffmpeg, &video, at, asked.region, &out, self.scratch.path(), signal).await {
                Ok(Some((width, height, bytes))) => {
                    let staged = ctx.artifacts().stage_scratch(ctx, self.scratch, &out, signal).await?;
                    let saved = ctx.artifacts().publish(ctx, staged, "image", "image/jpeg", "watch", signal).await?;
                    seen += 1;
                    count_frames(self.folder, chat, seen)?;
                    frames.push(json!({
                        "at": at, "artifact_id": saved.id, "path": saved.path, "mime": "image/jpeg",
                        "width": width, "height": height, "bytes": bytes,
                    }));
                }
                Ok(None) => notes.push(format!(
                    "The frame at {} stayed over 256 KB however small Kumi made it, so it's left out.",
                    format_time(at)
                )),
                Err(VideoFailure::Aborted) => return Err(cancelled()),
                Err(failure) => notes.push(format!("Kumi couldn't take the frame at {}: {}", format_time(at), failure.message())),
            }
        }
        Ok(frames)
    }

    /// The picture, from the video's download: the one running, joined, or a new `watch` job. Cancelled with the call
    /// when no other call waits for it.
    async fn downloaded(&self) -> Result<Downloaded, OpError> {
        let ctx = self.ctx;
        let running = ctx.watching().downloads.borrow().get(&self.page.id).cloned();
        let download = match running {
            Some(download) => download,
            None => {
                let (format, hls) = stream_format(&self.page.formats)?;
                let (tell, done) = channel::channel(None);
                let (id, ytdlp, url, folder) = (self.page.id.clone(), self.ytdlp.to_owned(), self.url.to_owned(), self.folder.to_owned());
                let job = ctx.start_job("watch", move |ctx, job| async move {
                    let outcome = download(&ctx, &job, &ytdlp, &url, &id, &format, hls, &folder).await;
                    ctx.watching().downloads.borrow_mut().remove(&id);
                    tell.send_replace(Some(outcome.clone()));
                    let downloaded = outcome?;
                    let bytes = std::fs::metadata(&downloaded.path).map(|meta| meta.len()).unwrap_or_default();
                    Ok(json!({"video": id, "format": format.format_id, "height": format.height, "bytes": bytes}))
                })?;
                let download = Rc::new(Download { job, done, waiting: Cell::new(0) });
                ctx.watching().downloads.borrow_mut().insert(self.page.id.clone(), download.clone());
                download
            }
        };
        download.waiting.set(download.waiting.get() + 1);
        let _waiting = Waiting { ctx, download: download.clone() };
        let mut done = download.done.clone();
        let outcome = tokio::select! {
            outcome = done.wait_for(Option::is_some) => outcome.ok().and_then(|outcome| (*outcome).clone()),
            _ = self.call.signal.cancelled() => return Err(cancelled()),
        };
        outcome.unwrap_or_else(|| {
            let mut downloads = ctx.watching().downloads.borrow_mut();
            if downloads.get(&self.page.id).is_some_and(|known| Rc::ptr_eq(known, &download)) {
                downloads.remove(&self.page.id);
            }
            Err(failed("the video's download stopped unexpectedly"))
        })
    }
}

/// The `watch` job's work: one stream of the video into the job's own scratch, stopped past the cap, then moved whole
/// into the video's folder. Cancelled or failed, the scratch goes with whatever was written.
#[allow(clippy::too_many_arguments)]
async fn download(
    ctx: &Ctx,
    job: &JobHandle,
    ytdlp: &str,
    url: &str,
    id: &str,
    format: &Format,
    hls: bool,
    folder: &Path,
) -> Result<Downloaded, OpError> {
    let scratch = ctx.scratch("watch")?;
    let into = scratch.path().join("download");
    make_private_dir(&into).map_err(failed)?;
    let mut step = vec![os("-f"), os(&format.format_id), os("--max-filesize"), os("150M"), os("-o"), os(into.join("%(id)s.%(ext)s"))];
    if hls {
        step.extend([os("--downloader"), os("m3u8:native")]);
    }
    let over = Signal::new();
    let fetching = yt_dlp(ytdlp, step, url, scratch.path(), DOWNLOAD_MS, 1024 * 1024, abort::any([job.signal.clone(), over.clone()]));
    // `--max-filesize` trusts the size YouTube says: serve watches the bytes itself.
    let growing = async {
        loop {
            tokio::time::sleep(GROWTH_EVERY).await;
            let bytes = folder_bytes(&into);
            job.report(bytes as f64, Some(STREAM_MOST as f64), Some("Downloading the video's picture"));
            if bytes > STREAM_MOST {
                over.cancel();
                break;
            }
        }
        std::future::pending::<()>().await
    };
    let fetched = tokio::select! {
        fetched = fetching => fetched,
        _ = growing => unreachable!("watching the download never ends by itself"),
    };
    match fetched {
        Ok(_) => {}
        Err(VideoFailure::Aborted) if over.is_cancelled() => {
            return Err(refused("too_large", "The video's picture grew past 150 MB as it downloaded, so Kumi stopped: ask for its captions."))
        }
        Err(failure) => return Err(fetch_error("the video's picture", failure)),
    }
    let file = into.join(format!("{id}.{}", format.ext));
    let size = match std::fs::symlink_metadata(&file) {
        Ok(meta) if meta.is_file() && meta.len() > 0 => meta.len(),
        _ => {
            return Err(refused(
                "fetch_failed",
                "yt-dlp finished without the video's picture: it may have been over the 150 MB cap.",
            ))
        }
    };
    if size > STREAM_MOST {
        return Err(refused("too_large", "The video's picture is over 150 MB: ask for its captions."));
    }
    make_private_dir(folder).map_err(failed)?;
    // One picture per video.
    for ext in VIDEO_EXTENSIONS {
        let _ = std::fs::remove_file(folder.join(format!("video.{ext}")));
    }
    let kept = folder.join(format!("video.{}", format.ext));
    std::fs::rename(&file, &kept).map_err(failed)?;
    let height = format.height.unwrap_or_default();
    let note = format!("Kumi kept a {height}p copy of the video's picture ({} MB) in its data for frames.", size.div_ceil(1_000_000));
    Ok(Downloaded { path: kept, note })
}

/// The moments to take, each within the video and at most one per tenth of a second.
fn moments(at: &[f64], duration: Option<f64>, notes: &mut Vec<String>) -> Vec<f64> {
    let mut moments = Vec::with_capacity(at.len());
    for &asked in at {
        let mut moment = asked;
        if let Some(duration) = duration {
            let last = (duration - 0.5).max(0.0);
            if moment > last {
                notes.push(format!(
                    "The video is {} long, so the frame asked at {} is taken at {}.",
                    format_time(duration),
                    format_time(asked),
                    format_time(last)
                ));
                moment = last;
            }
        }
        let moment = (moment * 10.0).round() / 10.0;
        if !moments.contains(&moment) {
            moments.push(moment);
        }
    }
    moments
}

/// One frame of the local `video` at `at` into `out`, made smaller until it's at most [`JPEG_MOST`]: its width,
/// height and bytes, or None when it never fits. ffmpeg reads only that file.
async fn frame(
    ffmpeg: &str,
    video: &Path,
    at: f64,
    region: Option<Region>,
    out: &Path,
    scratch: &Path,
    signal: &Signal,
) -> Result<Option<(u64, u64, u64)>, VideoFailure> {
    let mut input = OsString::from("file:");
    input.push(video);
    for (width, quality) in ENCODINGS {
        let scale = format!("scale='min({width},iw)':-2");
        let filter = match region {
            Some(region) => format!("crop={},{scale}", region.crop()),
            None => scale,
        };
        let args = [
            os("-hide_banner"),
            os("-loglevel"),
            os("error"),
            os("-nostdin"),
            os("-protocol_whitelist"),
            os("file"),
            os("-format_whitelist"),
            os("mov,matroska"),
            os("-ss"),
            os(format!("{at:.2}")),
            os("-i"),
            input.clone(),
            os("-frames:v"),
            os("1"),
            os("-vf"),
            os(filter),
            os("-q:v"),
            os(quality.to_string()),
            os("-f"),
            os("image2"),
            os("-c:v"),
            os("mjpeg"),
            os("-y"),
            os(out),
        ];
        let options = RunOptions {
            signal: Some(signal.clone()),
            timeout_ms: Some(FRAME_MS),
            max_buffer: Some(1024 * 1024),
            isolated_dir: Some(scratch.to_path_buf()),
            ..Default::default()
        };
        programs::run(ffmpeg, &args, options).await?;
        let bytes = std::fs::symlink_metadata(out)?.len();
        if bytes > JPEG_MOST {
            continue;
        }
        let (width, height) = jpeg_size(&std::fs::read(out)?).ok_or_else(|| VideoFailure::other("ffmpeg's frame isn't a JPEG"))?;
        return Ok(Some((width, height, bytes)));
    }
    Ok(None)
}

/// A JPEG's width and height, from its frame header.
fn jpeg_size(jpeg: &[u8]) -> Option<(u64, u64)> {
    if !jpeg.starts_with(&[0xff, 0xd8]) {
        return None;
    }
    let mut at = 2;
    while at + 4 <= jpeg.len() {
        if jpeg[at] != 0xff {
            return None;
        }
        let marker = jpeg[at + 1];
        if marker == 0xff {
            at += 1;
            continue;
        }
        if matches!(marker, 0x01 | 0xd0..=0xd9) {
            at += 2;
            continue;
        }
        if matches!(marker, 0xc0..=0xc3 | 0xc5..=0xc7 | 0xc9..=0xcb | 0xcd..=0xcf) {
            let header = jpeg.get(at + 4..at + 9)?;
            let height = u16::from_be_bytes([header[1], header[2]]);
            let width = u16::from_be_bytes([header[3], header[4]]);
            return (width > 0 && height > 0).then_some((width.into(), height.into()));
        }
        at += 2 + usize::from(u16::from_be_bytes([jpeg[at + 2], jpeg[at + 3]]));
    }
    None
}

/// `<state>/videos/youtube-<id>/`, made the owner's alone.
fn video_folder(ctx: &Ctx, id: &str) -> Result<PathBuf, OpError> {
    let videos = ctx.paths().state_dir.join("videos");
    let folder = videos.join(format!("youtube-{id}"));
    make_private_dir(&videos).and_then(|_| make_private_dir(&folder)).map_err(failed)?;
    Ok(folder)
}

/// The picture downloaded for the video before, if it's whole.
fn cached_video(folder: &Path) -> Option<PathBuf> {
    VIDEO_EXTENSIONS.iter().map(|ext| folder.join(format!("video.{ext}"))).find(|path| {
        std::fs::symlink_metadata(path).is_ok_and(|meta| meta.is_file() && meta.len() > 0 && meta.len() <= STREAM_MOST)
    })
}

/// How many frames of the video a chat has had (`""`: calls with no session).
fn frames_seen(folder: &Path, chat: &str) -> u64 {
    frame_counts(folder).get(chat).copied().unwrap_or_default()
}

fn frame_counts(folder: &Path) -> BTreeMap<String, u64> {
    std::fs::read(folder.join("frames.json")).ok().and_then(|bytes| serde_json::from_slice(&bytes).ok()).unwrap_or_default()
}

fn count_frames(folder: &Path, chat: &str, seen: u64) -> Result<(), OpError> {
    let mut counts = frame_counts(folder);
    counts.insert(chat.to_string(), seen);
    write_private(&folder.join("frames.json"), &serde_json::to_vec(&counts).map_err(failed)?).map_err(failed)
}

/// `bytes` into `path` whole, the owner's alone.
fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let mut file = tempfile::NamedTempFile::new_in(path.parent().unwrap_or(Path::new(".")))?;
    file.write_all(bytes)?;
    file.persist(path).map_err(|error| error.error)?;
    Ok(())
}

fn folder_bytes(folder: &Path) -> u64 {
    std::fs::read_dir(folder)
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .filter_map(|entry| std::fs::symlink_metadata(entry.path()).ok())
        .filter(std::fs::Metadata::is_file)
        .map(|meta| meta.len())
        .sum()
}

/// The video was just used: the cache keeps it longest.
fn used(folder: &Path) {
    if let Ok(file) = std::fs::File::options().write(true).open(folder.join("meta.json")) {
        let _ = file.set_modified(SystemTime::now());
    }
}

/// The videos kept beyond [`CACHE_MOST`] in all go, the least recently used first; `keep` (this call's) never does.
async fn prune(ctx: &Ctx, keep: &Path) {
    let videos = ctx.paths().state_dir.join("videos");
    let keep = keep.to_owned();
    let _ = tokio::task::spawn_blocking(move || {
        let mut folders: Vec<(SystemTime, u64, PathBuf)> = std::fs::read_dir(&videos)
            .into_iter()
            .flatten()
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| std::fs::symlink_metadata(path).is_ok_and(|meta| meta.is_dir()))
            .map(|path| {
                let used = std::fs::metadata(path.join("meta.json")).and_then(|meta| meta.modified()).unwrap_or(SystemTime::UNIX_EPOCH);
                (used, folder_bytes(&path), path)
            })
            .collect();
        folders.sort_by(|a, b| b.0.cmp(&a.0));
        let mut total = 0;
        for (_, bytes, path) in folders {
            if path != keep && total + bytes > CACHE_MOST {
                // Tidying: a folder that can't go now goes at a later call.
                let _ = std::fs::remove_dir_all(&path);
                continue;
            }
            total += bytes;
        }
    })
    .await;
}

fn refused(code: &'static str, reason: impl Into<String>) -> OpError {
    OpError::Refused { code, reason: reason.into(), owner: None }
}

fn cancelled() -> OpError {
    refused("cancelled", "Watching the video was cancelled.")
}

fn failed(error: impl std::fmt::Display) -> OpError {
    OpError::Failed { reason: error.to_string() }
}

fn check(signal: &Signal) -> Result<(), OpError> {
    if signal.is_cancelled() {
        Err(cancelled())
    } else {
        Ok(())
    }
}

/// A yt-dlp step's failure, said with its code: `timeout` past its time, `fetch_failed` otherwise.
fn fetch_error(what: &str, failure: VideoFailure) -> OpError {
    match failure {
        VideoFailure::Aborted => cancelled(),
        VideoFailure::TimedOut(said) => refused("timeout", format!("YouTube took too long while Kumi read {what} ({said}): try again later.")),
        other => refused("fetch_failed", format!("Kumi couldn't read {what}: {}", other.message())),
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::super::ctx::{Host, Progress};
    use super::super::Args;
    use super::*;
    use kumi_types::{daw::DawId, system::Env};
    use std::future::Future;
    use std::os::unix::fs::PermissionsExt;

    const ID: &str = "dQw4w9WgXcQ";
    const URL: &str = "https://www.youtube.com/watch?v=dQw4w9WgXcQ";
    const VIDEO: &[u8] = b"a stand-in for the video's picture";

    struct Fixture {
        _dir: tempfile::TempDir,
        ctx: Rc<Ctx>,
        fixtures: PathBuf,
        log: PathBuf,
    }

    impl Fixture {
        /// A runtime given the fake programs named in `tools`: a yt-dlp that answers `-J` with `page.json`, writes
        /// `captions.vtt` as the captions and `video.mp4` as the download, and an ffmpeg that writes `frame.jpg`
        /// (`big.jpg`, when there is one, at the first quality). Each records its arguments, yt-dlp its environment.
        fn new(tools: &[&str]) -> Fixture {
            let dir = tempfile::tempdir().unwrap();
            let root = dir.path().canonicalize().unwrap();
            let (fixtures, log) = (root.join("fixtures"), root.join("log"));
            std::fs::create_dir_all(&fixtures).unwrap();
            std::fs::create_dir_all(&log).unwrap();
            let (f, l, id) = (fixtures.display(), log.display(), ID);
            let ytdlp = format!(
                r#"#!/bin/sh
for a in "$@"; do printf '%s\n' "$a"; done >> "{l}/yt-dlp.args"
printf '%s\n' '---' >> "{l}/yt-dlp.args"
/usr/bin/env >> "{l}/yt-dlp.env"
printf '%s\n' '---' >> "{l}/yt-dlp.env"
out=""; prev=""; mode=page
for a in "$@"; do
  if [ "$prev" = "-o" ]; then out="$a"; fi
  if [ "$a" = "--skip-download" ]; then mode=captions; fi
  if [ "$a" = "-f" ]; then mode=download; fi
  prev="$a"
done
base="${{out%"%(id)s.%(ext)s"}}"
case "$mode" in
  page) exec /bin/cat "{f}/page.json" ;;
  captions) exec /bin/cp "{f}/captions.vtt" "${{base}}{id}.en.vtt" ;;
  download)
    if [ -e "{f}/slow" ]; then : > "{l}/downloading"; exec /bin/sleep 30; fi
    exec /bin/cp "{f}/video.mp4" "${{base}}{id}.mp4" ;;
esac
"#
            );
            let ffmpeg = format!(
                r#"#!/bin/sh
for a in "$@"; do printf '%s\n' "$a"; done >> "{l}/ffmpeg.args"
printf '%s\n' '---' >> "{l}/ffmpeg.args"
prev=""; frame="{f}/frame.jpg"
for a in "$@"; do
  if [ "$prev" = "-q:v" ] && [ "$a" = "3" ] && [ -e "{f}/big.jpg" ]; then frame="{f}/big.jpg"; fi
  out="$a"; prev="$a"
done
exec /bin/cp "$frame" "$out"
"#
            );
            let mut given = BTreeMap::new();
            for (name, script) in [("yt-dlp", ytdlp), ("ffmpeg", ffmpeg)] {
                let path = fixtures.join(name);
                std::fs::write(&path, script).unwrap();
                std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
                if tools.contains(&name) {
                    given.insert(name.to_string(), path);
                }
            }
            std::fs::write(fixtures.join("page.json"), page(good_formats()).to_string()).unwrap();
            std::fs::write(fixtures.join("captions.vtt"), captions(600)).unwrap();
            std::fs::write(fixtures.join("video.mp4"), VIDEO).unwrap();
            std::fs::write(fixtures.join("frame.jpg"), jpeg(320, 180, 1000)).unwrap();
            let args = Args {
                state_dir: root.join("state"),
                kumi_home: root.join("home"),
                hermes_home: None,
                hermes_root: None,
                daw: DawId::Reaper,
                library_roots: Vec::new(),
                media_roots: Vec::new(),
                tools: given,
            };
            let ctx = Ctx::new(args, Env::new(), Default::default()).unwrap();
            Fixture { _dir: dir, ctx, fixtures, log }
        }

        fn state(&self) -> &Path {
            &self.ctx.paths().state_dir
        }

        async fn watch(&self, args: Value, chat: Option<&str>) -> Result<Value, OpError> {
            self.watch_until(args, chat, Signal::new()).await
        }

        async fn watch_until(&self, args: Value, chat: Option<&str>, signal: Signal) -> Result<Value, OpError> {
            let host = Host { session_id: chat.map(str::to_string), ..Host::default() };
            let call = Call { args: args.as_object().unwrap().clone(), host, signal, progress: Progress::none() };
            (WATCH.run)(&self.ctx, call).await
        }

        /// Each run's arguments, as the fake `program` recorded them.
        fn runs(&self, program: &str) -> Vec<Vec<String>> {
            let text = std::fs::read_to_string(self.log.join(format!("{program}.args"))).unwrap_or_default();
            text.split("---\n").filter(|run| !run.is_empty()).map(|run| run.lines().map(str::to_string).collect()).collect()
        }

        fn downloads(&self) -> Vec<Vec<String>> {
            self.runs("yt-dlp").into_iter().filter(|run| run.iter().any(|arg| arg == "-f")).collect()
        }

        fn kept_video(&self) -> PathBuf {
            self.state().join(format!("videos/youtube-{ID}/video.mp4"))
        }
    }

    async fn local<F: Future>(work: F) -> F::Output {
        tokio::task::LocalSet::new().run_until(work).await
    }

    fn page(formats: Value) -> Value {
        json!({
            "id": ID, "title": "Serum 2 for beginners", "duration": 300.0, "language": "en",
            "chapters": [{"title": "Intro", "start_time": 0.0, "end_time": 30.0}, {"title": "FX", "start_time": 30.0, "end_time": 300.0}],
            "subtitles": {"en": [{"ext": "json3", "url": "https://example.invalid/en.json3"}, {"ext": "vtt", "url": "https://example.invalid/en.vtt"}],
                          "live_chat": [{"ext": "json"}]},
            "automatic_captions": {"en-orig": [{"ext": "vtt"}], "fr": [{"ext": "vtt"}]},
            "formats": formats,
        })
    }

    fn good_formats() -> Value {
        json!([
            {"format_id": "298", "protocol": "https", "vcodec": "avc1.4d401f", "acodec": "none", "ext": "mp4", "height": 720, "filesize": 39_000_000},
            {"format_id": "302", "protocol": "https", "vcodec": "vp9", "acodec": "none", "ext": "webm", "height": 720, "filesize": 27_000_000},
            {"format_id": "311", "protocol": "m3u8_native", "vcodec": "avc1.4d401f", "acodec": "none", "ext": "mp4", "height": 720, "filesize_approx": 30_000_000},
            {"format_id": "18", "protocol": "https", "vcodec": "avc1.42001E", "acodec": "mp4a.40.2", "ext": "mp4", "height": 360, "filesize": 10_000_000},
        ])
    }

    /// `lines` cues of WebVTT, two seconds each.
    fn captions(lines: usize) -> String {
        let stamp = |seconds: usize| format!("00:{:02}:{:02}.000", seconds / 60 % 60, seconds % 60);
        let mut vtt = String::from("WEBVTT\n\n");
        for n in 0..lines {
            let (start, end) = (n * 2 % 3600, (n * 2 + 2) % 3600);
            vtt += &format!("{} --> {}\nline {n} of the tutorial, about the filter's cutoff and its resonance.\n\n", stamp(start), stamp(end));
        }
        vtt
    }

    /// A JPEG's headers, `width` × `height`, and `padding` bytes for its picture.
    fn jpeg(width: u16, height: u16, padding: usize) -> Vec<u8> {
        let mut bytes = vec![0xff, 0xd8, 0xff, 0xe0, 0x00, 0x10];
        bytes.extend(b"JFIF\0");
        bytes.extend([1, 1, 0, 0, 1, 0, 1, 0, 0]);
        bytes.extend([0xff, 0xc0, 0x00, 0x11, 0x08]);
        bytes.extend(height.to_be_bytes());
        bytes.extend(width.to_be_bytes());
        bytes.extend([3, 1, 0x22, 0, 2, 0x11, 1, 3, 0x11, 1]);
        bytes.extend(vec![0; padding]);
        bytes.extend([0xff, 0xd9]);
        bytes
    }

    fn format(id: &str, protocol: &str, vcodec: &str, acodec: &str, height: u64, size: Option<u64>) -> Format {
        let ext = if vcodec.starts_with("avc1") || vcodec.starts_with("av01") { "mp4" } else { "webm" };
        Format {
            format_id: id.into(),
            protocol: protocol.into(),
            vcodec: vcodec.into(),
            acodec: acodec.into(),
            ext: ext.into(),
            height: Some(height),
            size,
        }
    }

    #[test]
    fn only_a_youtube_videos_https_link_names_a_video() {
        for link in [
            URL,
            "https://youtube.com/watch?feature=share&v=dQw4w9WgXcQ&t=42",
            "https://m.youtube.com/watch?v=dQw4w9WgXcQ#t=1",
            "https://youtu.be/dQw4w9WgXcQ?si=abc",
            "https://www.youtube.com/shorts/dQw4w9WgXcQ",
        ] {
            assert_eq!(youtube_id(link).as_deref(), Some(ID), "{link}");
        }
        for link in [
            "http://www.youtube.com/watch?v=dQw4w9WgXcQ",
            "https://www.youtube.com.evil.example/watch?v=dQw4w9WgXcQ",
            "https://evil.example/?u=https://youtu.be/dQw4w9WgXcQ",
            "https://music.youtube.com/watch?v=dQw4w9WgXcQ",
            "https://www.youtube.com/watch?v=dQw4w9WgXcQX",
            "https://www.youtube.com/embed/dQw4w9WgXcQ",
            "https://user@youtube.com/watch?v=dQw4w9WgXcQ",
            "https://www.youtube.com:443/watch?v=dQw4w9WgXcQ",
            "https://youtu.be/--exec=rm-rf",
            "--exec=x https://youtu.be/dQw4w9WgXcQ",
            "file:///tmp/dQw4w9WgXcQ.mp4",
        ] {
            assert_eq!(youtube_id(link), None, "{link}");
        }
        assert_eq!(canonical(ID), URL);
    }

    #[test]
    fn the_picture_is_one_video_only_https_file_within_the_caps() {
        let avc1 = format("298", "https", "avc1.4d401f", "none", 720, Some(39_000_000));
        let formats = vec![
            format("398", "https", "av01.0.05M.08", "none", 720, Some(19_000_000)),
            format("302", "https", "vp9", "none", 720, Some(27_000_000)),
            avc1.clone(),
            format("299", "https", "avc1.64002a", "none", 1080, Some(60_000_000)),
            format("311", "m3u8_native", "avc1.4d401f", "none", 720, Some(30_000_000)),
            format("135", "https", "avc1.4d401e", "none", 480, Some(9_000_000)),
        ];
        assert_eq!(stream_format(&formats).unwrap(), (avc1, false), "720p avc1 over vp9 and av01, never 1080p");
        let refused = vec![
            format("298", "https", "avc1.4d401f", "none", 720, Some(STREAM_MOST + 1)),
            format("22", "https", "avc1.64001F", "mp4a.40.2", 720, Some(20_000_000)),
            format("136", "http_dash_segments", "avc1.4d401f", "none", 720, Some(20_000_000)),
            format("135", "https", "avc1.4d401e", "none", 480, None),
            format("-x", "https", "avc1.4d401e", "none", 480, Some(1_000)),
        ];
        assert_eq!(stream_format(&refused).unwrap_err().code(), "too_large");
        let hls = format("231", "m3u8_native", "avc1.4d401e", "none", 480, Some(20_000_000));
        let with_hls: Vec<Format> = refused.iter().cloned().chain([hls.clone()]).collect();
        assert_eq!(stream_format(&with_hls).unwrap(), (hls, true), "HLS only when no https file fits");
    }

    #[test]
    fn a_frames_size_is_read_from_its_jpeg() {
        assert_eq!(jpeg_size(&jpeg(1280, 720, 64)), Some((1280, 720)));
        assert_eq!(jpeg_size(b"\x89PNG\r\n"), None);
    }

    #[tokio::test]
    async fn a_link_that_isnt_youtube_or_a_missing_program_is_refused_before_anything_runs() {
        let fixture = Fixture::new(&["yt-dlp", "ffmpeg"]);
        let error = fixture.watch(json!({"url": "https://vimeo.com/123456"}), None).await.unwrap_err();
        assert_eq!(error.code(), "not_youtube");
        assert!(fixture.runs("yt-dlp").is_empty());
        let bare = Fixture::new(&[]);
        let error = bare.watch(json!({"url": URL, "captions": true}), None).await.unwrap_err();
        assert_eq!((error.code(), error.reason()), ("no_yt_dlp", "yt-dlp isn't set up: run Kumi's setup to read videos."));
        let unframed = Fixture::new(&["yt-dlp"]);
        assert_eq!(unframed.watch(json!({"url": URL, "at": [10]}), None).await.unwrap_err().code(), "no_ffmpeg");
        assert!(unframed.runs("yt-dlp").is_empty());
    }

    #[tokio::test]
    async fn yt_dlp_runs_isolated_on_the_canonical_url_and_the_captions_are_capped() {
        let fixture = Fixture::new(&["yt-dlp"]);
        let result = fixture.watch(json!({"url": "https://youtu.be/dQw4w9WgXcQ?si=--exec", "captions": true}), None).await.unwrap();
        assert_eq!(
            result["video"],
            json!({"id": ID, "url": URL, "title": "Serum 2 for beginners", "duration": 300.0,
                   "chapters": [{"title": "Intro", "start": 0.0, "end": 30.0}, {"title": "FX", "start": 30.0, "end": 300.0}],
                   "captions": ["en", "en-orig"]})
        );
        let transcript = &result["transcript"];
        assert_eq!((&transcript["language"], &transcript["kind"], &transcript["truncated"]), (&json!("en"), &json!("manual"), &json!(true)));
        let text = transcript["text"].as_str().unwrap();
        assert!(text.starts_with("[0:00] line 0 of the tutorial"), "{}", &text[..80]);
        assert!(text.chars().count() <= TRANSCRIPT_MOST && text.chars().count() > TRANSCRIPT_MOST - 400, "{}", text.len());
        assert_eq!(result["frames"], json!([]));

        let runs = fixture.runs("yt-dlp");
        assert_eq!(runs.len(), 2, "the page, then the captions: {runs:?}");
        for run in &runs {
            assert_eq!(&run[..ISOLATION.len()], ISOLATION, "{run:?}");
            assert_eq!(&run[run.len() - 2..], ["--", URL], "only the canonical URL, last: {run:?}");
            assert!(!run.iter().any(|arg| arg.contains("exec")), "{run:?}");
        }
        assert_eq!(runs[0][ISOLATION.len()..run_end(&runs[0])], ["-J"]);
        assert_eq!(runs[1][ISOLATION.len()..ISOLATION.len() + 6], ["--write-subs", "--skip-download", "--sub-langs", "en", "--sub-format", "json3/vtt/srt/best"]);

        let scratch = fixture.state().join("scratch/watch");
        let allowed = [
            "HOME", "USERPROFILE", "TMPDIR", "TMP", "TEMP", "XDG_CONFIG_HOME", "XDG_CACHE_HOME", "APPDATA", "LOCALAPPDATA",
            "LC_ALL", "PWD", "OLDPWD", "SHLVL", "_", "PATH",
        ];
        let recorded = std::fs::read_to_string(fixture.log.join("yt-dlp.env")).unwrap();
        let environments: Vec<&str> = recorded.split("---\n").filter(|env| !env.is_empty()).collect();
        assert_eq!(environments.len(), 2);
        for environment in environments {
            let env: BTreeMap<&str, &str> = environment.lines().filter_map(|line| line.split_once('=')).collect();
            let home = env["HOME"];
            assert!(Path::new(home).starts_with(&scratch), "{home}");
            for name in ["USERPROFILE", "TMPDIR", "TMP", "TEMP", "XDG_CONFIG_HOME", "XDG_CACHE_HOME", "APPDATA", "LOCALAPPDATA"] {
                assert_eq!(env[name], home, "{name}");
            }
            assert_eq!(env["LC_ALL"], "C");
            for name in env.keys() {
                assert!(allowed.contains(name) || PROXIES.contains(name) || name.starts_with("__CF"), "{name} reached yt-dlp");
            }
        }
        assert!(std::fs::read_dir(&scratch).unwrap().next().is_none(), "the call's scratch went with it");

        fixture.watch(json!({"url": URL}), None).await.unwrap();
        assert_eq!(fixture.runs("yt-dlp").len(), 2, "the page is kept for the video's next call");
    }

    /// Where a run's own arguments end: before `-- <url>`.
    fn run_end(run: &[String]) -> usize {
        run.len() - 2
    }

    #[tokio::test]
    async fn a_video_without_a_picture_within_the_caps_is_refused_and_nothing_is_downloaded() {
        let fixture = Fixture::new(&["yt-dlp", "ffmpeg"]);
        let formats = json!([
            {"format_id": "136", "protocol": "http_dash_segments", "vcodec": "avc1.4d401f", "acodec": "none", "ext": "mp4", "height": 720, "filesize": 20_000_000},
            {"format_id": "298", "protocol": "https", "vcodec": "avc1.4d401f", "acodec": "none", "ext": "mp4", "height": 720, "filesize": 200_000_000},
            {"format_id": "311", "protocol": "m3u8_native", "vcodec": "avc1.4d401f", "acodec": "none", "ext": "mp4", "height": 720},
        ]);
        std::fs::write(fixture.fixtures.join("page.json"), page(formats).to_string()).unwrap();
        let error = fixture.watch(json!({"url": URL, "at": [30]}), None).await.unwrap_err();
        assert_eq!(error.code(), "too_large");
        assert!(error.reason().contains("captions"), "{}", error.reason());
        assert!(fixture.downloads().is_empty());
        assert!(fixture.runs("ffmpeg").is_empty());
    }

    #[tokio::test]
    async fn frames_come_from_one_local_copy_and_are_kept_as_artifacts() {
        local(async {
            let fixture = Fixture::new(&["yt-dlp", "ffmpeg"]);
            let first = fixture.watch(json!({"url": URL, "at": [12.34, 400], "region": "top-left"}), Some("chat-1")).await.unwrap();
            let frames = first["frames"].as_array().unwrap();
            assert_eq!(frames.len(), 2);
            assert_eq!((&frames[0]["at"], &frames[1]["at"]), (&json!(12.3), &json!(299.5)), "within the video's 300 s");
            let picture = jpeg(320, 180, 1000);
            for frame in frames {
                let kept = fixture.ctx.artifacts().resolve(&fixture.ctx, frame["artifact_id"].as_str().unwrap()).unwrap();
                assert_eq!(frame["path"], json!(kept));
                assert_eq!(std::fs::read(&kept).unwrap(), picture);
                assert_eq!(
                    (&frame["mime"], &frame["width"], &frame["height"], &frame["bytes"]),
                    (&json!("image/jpeg"), &json!(320), &json!(180), &json!(picture.len()))
                );
            }
            let notes = first["notes"].as_array().unwrap();
            assert!(notes.iter().any(|note| note.as_str().unwrap().contains("taken at 4:59")), "{notes:?}");
            assert_eq!(std::fs::read(fixture.kept_video()).unwrap(), VIDEO);

            let downloads = fixture.downloads();
            assert_eq!(downloads.len(), 1);
            let download = &downloads[0];
            assert_eq!(&download[..ISOLATION.len()], ISOLATION);
            assert_eq!(download[ISOLATION.len()..ISOLATION.len() + 5], ["-f", "298", "--max-filesize", "150M", "-o"]);
            assert!(download[ISOLATION.len() + 5].ends_with("/download/%(id)s.%(ext)s"));
            assert_eq!(&download[download.len() - 2..], ["--", URL]);

            let input = format!("file:{}", fixture.kept_video().display());
            let runs = fixture.runs("ffmpeg");
            assert_eq!(runs.len(), 2);
            for (run, at) in runs.iter().zip(["12.30", "299.50"]) {
                assert_eq!(
                    run[..12],
                    ["-hide_banner", "-loglevel", "error", "-nostdin", "-protocol_whitelist", "file", "-format_whitelist", "mov,matroska",
                     "-ss", at, "-i", input.as_str()]
                );
                assert!(run.iter().any(|arg| arg == "crop=iw*0.5:ih*0.5:0:0,scale='min(1280,iw)':-2"), "{run:?}");
                assert!(!run.iter().any(|arg| arg.contains("http")), "{run:?}");
            }

            let again = fixture.watch(json!({"url": URL, "at": [60]}), Some("chat-1")).await.unwrap();
            assert_eq!(again["frames"].as_array().unwrap().len(), 1);
            assert_eq!(fixture.downloads().len(), 1, "the kept copy is read again, never downloaded again");
        })
        .await;
    }

    #[tokio::test]
    async fn a_chat_gets_at_most_24_frames_of_a_video() {
        local(async {
            let fixture = Fixture::new(&["yt-dlp", "ffmpeg"]);
            for _ in 0..6 {
                fixture.watch(json!({"url": URL, "at": [1, 2, 3, 4]}), Some("chat-1")).await.unwrap();
            }
            let error = fixture.watch(json!({"url": URL, "at": [5]}), Some("chat-1")).await.unwrap_err();
            assert_eq!(error.code(), "frame_limit");
            assert_eq!(fixture.runs("ffmpeg").len(), 24);
            let other = fixture.watch(json!({"url": URL, "at": [5]}), Some("chat-2")).await.unwrap();
            assert_eq!(other["frames"].as_array().unwrap().len(), 1, "another chat has its own 24");
        })
        .await;
    }

    #[tokio::test]
    async fn a_frame_over_256_kb_is_made_again_smaller() {
        local(async {
            let fixture = Fixture::new(&["yt-dlp", "ffmpeg"]);
            std::fs::write(fixture.fixtures.join("big.jpg"), jpeg(1280, 720, 300 * 1024)).unwrap();
            let result = fixture.watch(json!({"url": URL, "at": [10]}), None).await.unwrap();
            assert_eq!(result["frames"][0]["width"], 320);
            let runs = fixture.runs("ffmpeg");
            assert_eq!(runs.len(), 2);
            assert!(runs[1].windows(2).any(|pair| pair == ["-q:v", "8"]), "{:?}", runs[1]);
            assert!(runs[1].iter().any(|arg| arg == "scale='min(960,iw)':-2"), "{:?}", runs[1]);
        })
        .await;
    }

    #[tokio::test]
    async fn calls_for_one_video_share_its_download() {
        local(async {
            let fixture = Fixture::new(&["yt-dlp", "ffmpeg"]);
            let (one, two) = futures::join!(
                fixture.watch(json!({"url": URL, "at": [1]}), Some("chat-1")),
                fixture.watch(json!({"url": URL, "at": [2]}), Some("chat-2")),
            );
            assert_eq!((one.unwrap()["frames"].as_array().unwrap().len(), two.unwrap()["frames"].as_array().unwrap().len()), (1, 1));
            assert_eq!(fixture.downloads().len(), 1);
        })
        .await;
    }

    #[tokio::test]
    async fn cancelling_the_only_waiting_call_cancels_the_download_and_keeps_nothing() {
        local(async {
            let fixture = Fixture::new(&["yt-dlp", "ffmpeg"]);
            std::fs::write(fixture.fixtures.join("slow"), b"").unwrap();
            let signal = Signal::new();
            let cancel = async {
                while !fixture.log.join("downloading").exists() {
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
                let job = fixture.ctx.watching().downloads.borrow()[ID].job.clone();
                signal.cancel();
                job
            };
            let (result, job) = futures::join!(fixture.watch_until(json!({"url": URL, "at": [1]}), None, signal.clone()), cancel);
            assert_eq!(result.unwrap_err().code(), "cancelled");
            while fixture.ctx.jobs().get(&job, fixture.state()).unwrap()["state"] == "running" {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            assert_eq!(fixture.ctx.jobs().get(&job, fixture.state()).unwrap()["state"], "cancelled");
            assert!(!fixture.kept_video().exists());
            assert!(fixture.ctx.watching().downloads.borrow().is_empty());
            let scratch = fixture.state().join("scratch/watch");
            assert!(std::fs::read_dir(&scratch).unwrap().next().is_none(), "the download's scratch went with it");
        })
        .await;
    }
}
