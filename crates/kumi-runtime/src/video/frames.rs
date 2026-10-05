//! Frames and stretches of sound taken by ffmpeg, with atomic cache writes.
use super::programs::{run, RunOptions, VideoFailure};
use kumi_common::{abort::Signal, js::number::to_fixed};
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::{future::Future, path::Path, sync::LazyLock};
#[derive(Clone, Debug, Default)]
pub struct Input {
    pub url: String,
    pub headers: Option<indexmap::IndexMap<String, String>>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Thumb {
    pub width: usize,
    pub height: usize,
    pub rgb: Vec<u8>,
}
pub const THUMB_WIDTH: usize = 32;
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
fn source(input: &Input, at: f64) -> Vec<String> {
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
    }
    args.extend(["-ss".into(), to_fixed(at, 2), "-i".into(), input.url.clone()]);
    args
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
            args.extend(source(input, at));
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
    Ok(Frame { jpeg: tokio::fs::read(path).await?, thumb: thumb_of(ffmpeg, path, signal).await? })
}
pub async fn thumb_of(ffmpeg: &str, jpeg: &str, signal: Option<Signal>) -> Result<Thumb, VideoFailure> {
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
            "scale=32:-2",
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
    if width != THUMB_WIDTH || !(2..=THUMB_WIDTH).contains(&height) || data.len() - start != width * height * 3 {
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
            args.extend(source(input, from));
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
    let output = run(
        &probe,
        &["-v", "error", "-show_entries", "format=duration", "-of", "csv=p=0", file],
        RunOptions { timeout_ms: Some(20_000), signal, ..Default::default() },
    )
    .await
    .ok()?;
    output.stdout_text().trim().parse::<f64>().ok().filter(|n| n.is_finite() && *n > 0.0)
}
