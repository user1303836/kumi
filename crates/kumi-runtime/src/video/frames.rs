//! Frames and stretches of sound taken by ffmpeg, with atomic cache writes.
use super::programs::{ffmpeg_reads_in_pieces, run, RunOptions, VideoFailure};
use kumi_common::{abort::Signal, js::number::to_fixed};
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::{future::Future, path::Path, sync::LazyLock};
#[derive(Clone, Debug, Default)]
pub struct Input {
    pub url: String,
    pub headers: Option<indexmap::IndexMap<String, String>>,
    /// The size of the pieces the site wants its stream asked for in (yt-dlp's `http_chunk_size`).
    pub piece: Option<u64>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Thumb {
    pub width: usize,
    pub height: usize,
    pub rgb: Vec<u8>,
}
/// A frame's thumbnail's longer side.
pub const THUMB_SIZE: usize = 32;
pub const REGIONS: [(&str, &str); 9] = [
    ("top", "iw:ih*0.4:0:0"),
    ("bottom", "iw:ih*0.4:0:ih*0.6"),
    ("left", "iw*0.5:ih:0:0"),
    ("right", "iw*0.5:ih:iw*0.5:0"),
    ("center", "iw*0.6:ih*0.6:iw*0.2:ih*0.2"),
    ("top-left", "iw*0.5:ih*0.5:0:0"),
    ("top-right", "iw*0.5:ih*0.5:iw*0.5:0"),
    ("bottom-left", "iw*0.5:ih*0.5:0:ih*0.5"),
    ("bottom-right", "iw*0.5:ih*0.5:iw*0.5:ih*0.5"),
];
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Region {
    Top,
    Bottom,
    Left,
    Right,
    Center,
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
}
impl Region {
    pub fn as_str(self) -> &'static str {
        REGIONS[self as usize].0
    }
    pub fn crop(self) -> &'static str {
        REGIONS[self as usize].1
    }
    pub fn parse(s: &str) -> Option<Self> {
        [Self::Top, Self::Bottom, Self::Left, Self::Right, Self::Center, Self::TopLeft, Self::TopRight, Self::BottomLeft, Self::BottomRight]
            .into_iter()
            .find(|r| r.as_str() == s)
    }
}
async fn source(ffmpeg: &str, input: &Input, at: f64, signal: &Option<Signal>) -> Result<Vec<String>, VideoFailure> {
    let mut args = Vec::new();
    let headers = input
        .headers
        .as_ref()
        .map(|headers| {
            headers
                .iter()
                .filter(|(k, v)| !k.is_empty() && k.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-') && !v.contains(['\r', '\n']))
                .map(|(k, v)| format!("{k}: {v}\r\n"))
                .collect::<String>()
        })
        .unwrap_or_default();
    if !headers.is_empty() {
        args.extend(["-headers".into(), headers]);
    }
    let lower = input.url.to_ascii_lowercase();
    if lower.starts_with("http:") || lower.starts_with("https:") {
        args.extend(["-rw_timeout".into(), "20000000".into()]);
        if let Some(piece) = input.piece.filter(|piece| *piece > 0) {
            if ffmpeg_reads_in_pieces(ffmpeg, signal.clone()).await? == Some(true) {
                args.extend(["-request_size".into(), piece.to_string(), "-multiple_requests".into(), "1".into()]);
            }
        }
    }
    args.extend(["-ss".into(), to_fixed(at, 2), "-i".into(), input.url.clone()]);
    Ok(args)
}
async fn into<F, Fut>(path: &str, write: F) -> Result<(), VideoFailure>
where
    F: FnOnce(String) -> Fut,
    Fut: Future<Output = Result<(), VideoFailure>>,
{
    let parent = Path::new(path).parent().unwrap_or(Path::new("."));
    let mut builder = tokio::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    builder.mode(0o700);
    builder.create(parent).await?;
    let suffix = path.rfind('.').map_or_else(|| path.chars().last().map(|c| c.to_string()).unwrap_or_default(), |at| path[at..].into());
    let temporary = parent.join(format!(".{}{suffix}", uuid::Uuid::new_v4()));
    let result = async {
        write(temporary.to_string_lossy().into()).await?;
        tokio::fs::rename(&temporary, path).await?;
        Ok(())
    }
    .await;
    if result.is_err() {
        match tokio::fs::remove_file(&temporary).await {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(e.into()),
            _ => {}
        }
    }
    result
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Frame {
    pub jpeg: Vec<u8>,
    pub thumb: Thumb,
}
pub async fn frame_at(
    ffmpeg: &str,
    input: Option<&Input>,
    at: f64,
    path: &str,
    signal: Option<Signal>,
    region: Option<Region>,
) -> Result<Frame, VideoFailure> {
    if !Path::new(path).exists() {
        let input = input.ok_or_else(|| VideoFailure::other("there's no stream to take it from"))?;
        let filter = region.map_or_else(|| "scale='min(1280,iw)':-2".into(), |r| format!("crop={},scale='min(1600,iw)':-2", r.crop()));
        let frame_signal = signal.clone();
        into(path, |temporary| async move {
            let mut args = vec!["-hide_banner".into(), "-loglevel".into(), "error".into(), "-nostdin".into()];
            args.extend(source(ffmpeg, input, at, &frame_signal).await?);
            args.extend([
                "-frames:v".into(),
                "1".into(),
                "-vf".into(),
                filter,
                "-q:v".into(),
                "3".into(),
                "-f".into(),
                "image2".into(),
                "-c:v".into(),
                "mjpeg".into(),
                "-y".into(),
                temporary,
            ]);
            run(ffmpeg, &args, RunOptions { timeout_ms: Some(60_000), signal: frame_signal, ..Default::default() }).await.map(|_| ())
        })
        .await?;
    }
    let jpeg = tokio::fs::read(path).await?;
    // The picture is what the model sees: a thumbnail that can't be made leaves a whole frame without one (the app
    // draws none for it), unless the watch was stopped.
    let thumb = match thumb_of(ffmpeg, path, signal.clone()).await {
        Ok(thumb) => thumb,
        Err(_)
            if jpeg.starts_with(&[0xff, 0xd8]) && jpeg.ends_with(&[0xff, 0xd9]) && !signal.as_ref().is_some_and(Signal::is_cancelled) =>
        {
            Thumb { width: 0, height: 0, rgb: vec![] }
        }
        Err(error) => return Err(error),
    };
    Ok(Frame { jpeg, thumb })
}
pub async fn thumb_of(ffmpeg: &str, jpeg: &str, signal: Option<Signal>) -> Result<Thumb, VideoFailure> {
    // The longer side THUMB_SIZE and the other even, as before: a portrait frame's, or a side close-up's, is that high.
    let scale = format!("scale='if(gte(iw,ih),{THUMB_SIZE},-2)':'if(gte(iw,ih),-2,{THUMB_SIZE})'");
    let output = run(
        ffmpeg,
        &[
            "-hide_banner",
            "-loglevel",
            "error",
            "-nostdin",
            "-i",
            jpeg,
            "-vf",
            &scale,
            "-frames:v",
            "1",
            "-f",
            "image2pipe",
            "-c:v",
            "ppm",
            "-",
        ],
        RunOptions { timeout_ms: Some(20_000), signal, ..Default::default() },
    )
    .await?;
    let data = output.stdout;
    static HEADER: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^P6\s+([0-9]+)\s+([0-9]+)\s+255\s").unwrap());
    let beginning: String = data[..data.len().min(32)].iter().map(|b| char::from(*b)).collect();
    let header = HEADER.captures(&beginning).ok_or_else(|| VideoFailure::other("the frame's thumbnail came out wrong"))?;
    let width = header[1].parse::<usize>().unwrap_or(0);
    let height = header[2].parse::<usize>().unwrap_or(0);
    let start = header[0].chars().count();
    if width.max(height) != THUMB_SIZE || width.min(height) < 2 || data.len() - start != width * height * 3 {
        return Err(VideoFailure::other("the frame's thumbnail came out wrong"));
    }
    Ok(Thumb { width, height, rgb: data[start..].to_vec() })
}
pub async fn sound_between(
    ffmpeg: &str,
    input: &Input,
    from: f64,
    to: f64,
    path: &str,
    signal: Option<Signal>,
    speech: bool,
) -> Result<String, VideoFailure> {
    if !Path::new(path).exists() {
        into(path, |temporary| async move {
            let mut args = vec!["-hide_banner".into(), "-loglevel".into(), "error".into(), "-nostdin".into()];
            args.extend(source(ffmpeg, input, from, &signal).await?);
            args.extend([
                "-t".into(),
                to_fixed(to - from, 2),
                "-vn".into(),
                "-ac".into(),
                if speech { "1" } else { "2" }.into(),
                "-ar".into(),
                if speech { "16000" } else { "44100" }.into(),
                "-c:a".into(),
                "pcm_s16le".into(),
                "-f".into(),
                "wav".into(),
                "-y".into(),
                temporary,
            ]);
            run(ffmpeg, &args, RunOptions { timeout_ms: Some(600_000), signal, ..Default::default() }).await.map(|_| ())
        })
        .await?;
    }
    Ok(path.into())
}
pub async fn duration_of(ffmpeg: &str, file: &str, signal: Option<Signal>) -> Option<f64> {
    let probe = if ffmpeg == "ffmpeg" {
        "ffprobe".into()
    } else {
        Path::new(ffmpeg)
            .parent()
            .unwrap_or(Path::new("."))
            .join(if cfg!(windows) { "ffprobe.exe" } else { "ffprobe" })
            .to_string_lossy()
            .into_owned()
    };
    let probed = run(
        &probe,
        &["-v", "error", "-show_entries", "format=duration", "-of", "csv=p=0", file],
        RunOptions { timeout_ms: Some(20_000), signal: signal.clone(), ..Default::default() },
    )
    .await;
    if let Ok(output) = probed {
        return output.stdout_text().trim().parse::<f64>().ok().filter(|n| n.is_finite() && *n > 0.0);
    }
    // Kumi's own ffmpeg comes without ffprobe: what ffmpeg says of the file it opens, then (it reads nothing past that).
    let output = run(
        ffmpeg,
        &["-hide_banner", "-nostdin", "-i", file, "-t", "0", "-f", "null", "-"],
        RunOptions { timeout_ms: Some(20_000), signal, ..Default::default() },
    )
    .await
    .ok()?;
    static DURATION: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"Duration: ([0-9]+):([0-9]{2}):([0-9]{2}(?:\.[0-9]+)?)").unwrap());
    let found = DURATION.captures(&output.stderr)?;
    let seconds = found[1].parse::<f64>().ok()? * 3600.0 + found[2].parse::<f64>().ok()? * 60.0 + found[3].parse::<f64>().ok()?;
    Some(seconds).filter(|n| n.is_finite() && *n > 0.0)
}
