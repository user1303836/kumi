//! Reading audio files as blocks of samples, without loading a whole song into memory.

use crate::video::programs::{find_ffmpeg, run, FfmpegOptions, RunOptions, VideoFailure};
use kumi_common::{
    abort::{Signal, SignalExt},
    js::number::to_fixed,
};
use std::path::{Path, PathBuf};
use tokio::{
    fs::File,
    io::{AsyncReadExt, AsyncSeekExt},
};

pub const AUDIO_EXTENSIONS: &[&str] =
    &[".wav", ".wave", ".aif", ".aiff", ".aifc", ".mp3", ".m4a", ".aac", ".mp4", ".flac", ".ogg", ".oga", ".opus", ".caf"];
#[derive(Debug, Clone, thiserror::Error)]
#[error("{0}")]
pub struct AudioError(pub String);
/// What a lack of room to convert a file starts with: it says nothing of the file itself.
const NO_ROOM: &str = "Kumi reads that file by converting it first.";
impl AudioError {
    pub fn no_room(full: &str) -> Self {
        Self(format!("{NO_ROOM} {full}"))
    }
    /// There was no room to convert the file: a learner stops rather than record it as the file's.
    pub fn is_no_room(&self) -> bool {
        self.0.starts_with(NO_ROOM)
    }
}
impl From<std::io::Error> for AudioError {
    fn from(e: std::io::Error) -> Self {
        Self(e.to_string())
    }
}
impl From<VideoFailure> for AudioError {
    fn from(e: VideoFailure) -> Self {
        Self(e.to_string())
    }
}
impl From<kumi_common::abort::Aborted> for AudioError {
    fn from(e: kumi_common::abort::Aborted) -> Self {
        Self(e.to_string())
    }
}
#[derive(Clone, Copy)]
struct Encoding {
    float: bool,
    bits: usize,
    signed: bool,
}
struct Layout {
    sample_rate: f64,
    channels: usize,
    frames: usize,
    data_offset: u64,
    little_endian: bool,
    encoding: Encoding,
    format: String,
}
/// A file Kumi can read as PCM; `cleanup` deletes a converted temporary copy.
pub struct PreparedAudio {
    pub path: PathBuf,
    pub format: Option<String>,
    /// The file's length in seconds when its converted copy stops short of it.
    pub seconds: Option<f64>,
    folder: Option<PathBuf>,
}
impl PreparedAudio {
    pub async fn cleanup(&self) {
        if let Some(folder) = &self.folder {
            let _ = tokio::fs::remove_dir_all(folder).await;
        }
    }
}
impl Drop for PreparedAudio {
    fn drop(&mut self) {
        if let Some(folder) = &self.folder {
            let _ = std::fs::remove_dir_all(folder);
        }
    }
}
pub async fn prepare_audio(path: impl AsRef<Path>, signal: Option<Signal>) -> Result<PreparedAudio, AudioError> {
    prepare_audio_to(path, signal, None).await
}
/// As prepare_audio, converting no further than `until` seconds in, the farthest anything will read: a 2-hour mix
/// isn't written out whole (2.5 GB of it) for its first twelve minutes, and its length is still known.
pub async fn prepare_audio_to(path: impl AsRef<Path>, signal: Option<Signal>, until: Option<f64>) -> Result<PreparedAudio, AudioError> {
    let path = path.as_ref();
    let extension = extension(path);
    if !AUDIO_EXTENSIONS.contains(&extension.as_str()) {
        return Err(AudioError(format!(
            "{} isn't an audio format Kumi reads (WAV, AIFF, MP3, M4A, FLAC, Ogg).",
            if extension.is_empty() { "That file" } else { &extension }
        )));
    }
    if [".wav", ".wave", ".aif", ".aiff", ".aifc"].contains(&extension.as_str()) {
        return Ok(PreparedAudio { path: path.into(), format: None, seconds: None, folder: None });
    }
    let folder = std::env::temp_dir().join(format!("kumi-audio-{}", uuid::Uuid::new_v4()));
    tokio::fs::create_dir(&folder).await?;
    let mut prepared =
        PreparedAudio { path: folder.join("converted.wav"), format: Some(extension[1..].into()), seconds: None, folder: Some(folder) };
    match convert(path, &prepared.path, signal, until).await {
        Ok(seconds) => prepared.seconds = seconds,
        Err(error) => {
            prepared.cleanup().await;
            return Err(error);
        }
    }
    Ok(prepared)
}
fn extension(path: &Path) -> String {
    path.extension().map(|v| format!(".{}", v.to_string_lossy().to_lowercase())).unwrap_or_default()
}
pub async fn open_audio(path: impl AsRef<Path>, signal: Option<Signal>) -> Result<AudioSource, AudioError> {
    open_audio_to(path, signal, None).await
}
/// As open_audio, reading no further than `until` seconds in (see prepare_audio_to); `frames` is still the whole file's.
pub async fn open_audio_to(path: impl AsRef<Path>, signal: Option<Signal>, until: Option<f64>) -> Result<AudioSource, AudioError> {
    let prepared = prepare_audio_to(path, signal, until).await?;
    let handle = File::open(&prepared.path).await.map_err(|e| {
        AudioError(if e.kind() == std::io::ErrorKind::NotFound { "There's no file there." } else { "Kumi couldn't open that file." }.into())
    })?;
    let mut source = open_pcm(handle).await?;
    if let Some(format) = &prepared.format {
        source.format = format.clone();
    }
    if let Some(seconds) = prepared.seconds {
        source.as_long_as(seconds);
    }
    source.prepared = Some(prepared);
    Ok(source)
}
/// The copy converted, and the file's length when the copy stops short of it (`until`).
async fn convert(input: &Path, output: &Path, signal: Option<Signal>, until: Option<f64>) -> Result<Option<f64>, AudioError> {
    let mac = cfg!(target_os = "macos");
    let ffmpeg = if mac {
        "ffmpeg".into()
    } else {
        match find_ffmpeg(FfmpegOptions { signal: signal.clone(), ..Default::default() }).await {
            Ok(found) => found.unwrap_or_else(|| "ffmpeg".into()),
            Err(VideoFailure::Video(e)) => return Err(AudioError(e.0)),
            Err(_) => "ffmpeg".into(),
        }
    };
    let input_s = input.to_string_lossy();
    let output_s = output.to_string_lossy();
    // How long it is (ffprobe, or what ffmpeg says of it, decoding nothing): to copy no further than `until`, and to ask
    // room for what will be written. Not known, it's copied whole without asking, as it always was.
    let length = match until {
        Some(_) => crate::video::frames::duration_of(&ffmpeg, &input_s, signal.clone()).await,
        None => None,
    };
    let cut = cut_at(until, length);
    let cut_text = cut.map(|cut| to_fixed(cut, 3));
    let folder = output.parent().map(|folder| folder.to_string_lossy().into_owned()).unwrap_or_default();
    // Each converter, and how many seconds of the file it writes (when that's known): afconvert copies whole.
    let mut attempts = Vec::new();
    if mac {
        attempts.push(("afconvert", vec!["-f", "WAVE", "-d", "LEF32", &input_s, &output_s], length));
    }
    let mut args = vec!["-v", "error", "-nostdin", "-y", "-i", &input_s, "-vn"];
    if let Some(cut) = &cut_text {
        args.extend(["-t", cut]);
    }
    args.extend(["-acodec", "pcm_f32le", "-f", "wav", &output_s]);
    attempts.push((&ffmpeg, args, cut.or(length)));
    let mut no_room = None;
    for (command, args, writes) in attempts {
        if let Some(s) = &signal {
            s.check()?;
        }
        if let Some(seconds) = writes {
            if let Some(full) = crate::core::disk::low_disk(&folder, seconds * BYTES_A_SECOND, "Kumi reads audio on").await {
                no_room = Some(full);
                continue;
            }
        }
        match run(command, &args, RunOptions { signal: signal.clone(), timeout_ms: Some(120_000), max_buffer: Some(1024 * 1024) }).await {
            Ok(_) => return Ok(if command == "afconvert" { None } else { cut.and(length) }),
            Err(e) => {
                if signal.as_ref().is_some_and(Signal::is_cancelled) {
                    return Err(e.into());
                }
            }
        }
    }
    if let Some(full) = no_room {
        return Err(AudioError::no_room(&full));
    }
    Err(AudioError(format!("Kumi couldn't decode {} here: it reads WAV and AIFF itself, and other formats with {}ffmpeg. Install ffmpeg, or export the file as WAV.",input.extension().unwrap_or_default().to_string_lossy().to_uppercase(),if mac {"macOS's afconvert or "}else{""})))
}
/// f32 at up to 48 kHz stereo: a copy's size a second.
const BYTES_A_SECOND: f64 = 384_000.0;
/// Where the copy stops: `until`, when the file goes on past it.
fn cut_at(until: Option<f64>, length: Option<f64>) -> Option<f64> {
    until.zip(length).filter(|(until, length)| until < length).map(|(until, _)| until)
}
/// Sample frames are read a block at a time; seeking never changes the audio.
pub struct AudioSource {
    pub sample_rate: f64,
    pub channels: usize,
    pub frames: usize,
    pub format: String,
    handle: Option<File>,
    layout: Layout,
    frame: usize,
    prepared: Option<PreparedAudio>,
}
impl AudioSource {
    pub fn seek(&mut self, to: f64) {
        self.frame = to.floor().max(0.0).min(self.frames as f64) as usize;
    }
    /// The file's own length, when what's open is a copy that stops short of it: reads still end where the copy does.
    pub fn as_long_as(&mut self, seconds: f64) {
        self.frames = self.frames.max((seconds * self.sample_rate).round() as usize);
    }
    pub async fn read(&mut self, count: usize) -> Result<Option<Vec<Vec<f32>>>, AudioError> {
        let frames = count.min(self.layout.frames.saturating_sub(self.frame));
        if frames == 0 {
            return Ok(None);
        }
        let align = self.layout.encoding.bits / 8 * self.channels;
        let buffer = bytes_at(
            self.handle.as_mut().ok_or_else(|| AudioError("file closed".into()))?,
            self.layout.data_offset + (self.frame * align) as u64,
            frames * align,
        )
        .await?;
        let got = buffer.len() / align;
        if got == 0 {
            return Ok(None);
        }
        self.frame += got;
        Ok(Some(deinterleave(&buffer, got, &self.layout)))
    }
    pub async fn close(&mut self) -> Result<(), AudioError> {
        self.handle.take();
        if let Some(prepared) = self.prepared.take() {
            prepared.cleanup().await;
        }
        Ok(())
    }
}
async fn open_pcm(mut handle: File) -> Result<AudioSource, AudioError> {
    let layout = read_layout(&mut handle).await?;
    Ok(AudioSource {
        sample_rate: layout.sample_rate,
        channels: layout.channels,
        frames: layout.frames,
        format: layout.format.clone(),
        handle: Some(handle),
        layout,
        frame: 0,
        prepared: None,
    })
}
async fn bytes_at(handle: &mut File, position: u64, length: usize) -> Result<Vec<u8>, AudioError> {
    handle.seek(std::io::SeekFrom::Start(position)).await?;
    let mut out = vec![0; length];
    let mut filled = 0;
    while filled < length {
        let n = handle.read(&mut out[filled..]).await?;
        if n == 0 {
            break;
        }
        filled += n;
    }
    out.truncate(filled);
    Ok(out)
}
fn uint(bytes: &[u8], at: usize, length: usize, little: bool) -> Result<u64, AudioError> {
    let bytes = bytes.get(at..at + length).ok_or_else(|| AudioError("Attempt to access memory outside buffer bounds".into()))?;
    let mut n = 0;
    if little {
        for b in bytes.iter().rev() {
            n = n * 256 + *b as u64;
        }
    } else {
        for b in bytes {
            n = n * 256 + *b as u64;
        }
    }
    Ok(n)
}
async fn read_layout(handle: &mut File) -> Result<Layout, AudioError> {
    let head = bytes_at(handle, 0, 12).await?;
    if head.len() < 12 {
        return Err(AudioError("That file is too short to be audio.".into()));
    }
    let tag = &head[..4];
    let kind = &head[8..12];
    if (tag == b"RIFF" || tag == b"RIFX") && kind == b"WAVE" {
        return read_wav(handle, tag == b"RIFF").await;
    }
    if tag == b"RF64" {
        return Err(AudioError("That WAV is in the RF64 format for files over 4 GB; export a shorter part.".into()));
    }
    if tag == b"FORM" && (kind == b"AIFF" || kind == b"AIFC") {
        return read_aiff(handle, kind == b"AIFC").await;
    }
    Err(AudioError("That file doesn't look like WAV or AIFF audio inside.".into()))
}
async fn read_wav(handle: &mut File, little: bool) -> Result<Layout, AudioError> {
    let size = handle.metadata().await?.len();
    let mut position = 12;
    let mut format = None;
    while position + 8 <= size {
        let head = bytes_at(handle, position, 8).await?;
        if head.len() < 8 {
            break;
        }
        let length = uint(&head, 4, 4, little)?;
        if &head[..4] == b"fmt " {
            let body = bytes_at(handle, position + 8, length.min(40) as usize).await?;
            let mut code = uint(&body, 0, 2, little)?;
            if code == 0xfffe && body.len() >= 26 {
                code = uint(&body, 24, 2, little)?;
            }
            format =
                Some((code, uint(&body, 2, 2, little)? as usize, uint(&body, 4, 4, little)? as f64, uint(&body, 14, 2, little)? as usize));
        } else if &head[..4] == b"data" {
            let (code, channels, rate, bits) =
                format.ok_or_else(|| AudioError("That WAV has its audio before its format; it can't be read.".into()))?;
            let float = code == 3 && (bits == 32 || bits == 64);
            if !(float || code == 1 && [8, 16, 24, 32].contains(&bits)) {
                return Err(AudioError(format!("That WAV's sample format (code {code}, {bits} bits) isn't one Kumi reads.")));
            }
            if channels == 0 || rate == 0.0 {
                return Err(AudioError("That WAV says it has no channels or no sample rate.".into()));
            }
            return Ok(Layout {
                sample_rate: rate,
                channels,
                frames: length.min(size - position - 8) as usize / (channels * bits / 8),
                data_offset: position + 8,
                little_endian: little,
                encoding: Encoding { float, bits, signed: bits != 8 },
                format: "wav".into(),
            });
        }
        position += 8 + length + length % 2;
    }
    Err(AudioError("That WAV has no audio in it.".into()))
}
fn extended(bytes: &[u8]) -> Result<f64, AudioError> {
    let exponent = uint(bytes, 0, 2, false)? & 0x7fff;
    let high = uint(bytes, 2, 4, false)?;
    let low = uint(bytes, 6, 4, false)?;
    if exponent == 0 && high == 0 && low == 0 {
        return Ok(0.0);
    }
    Ok((high as f64 * 2f64.powi(32) + low as f64) * 2f64.powi(exponent as i32 - 16383 - 63) * if bytes[0] & 0x80 != 0 { -1.0 } else { 1.0 })
}
async fn read_aiff(handle: &mut File, compressed: bool) -> Result<Layout, AudioError> {
    let size = handle.metadata().await?.len();
    let mut position = 12;
    let mut common = None;
    while position + 8 <= size {
        let head = bytes_at(handle, position, 8).await?;
        if head.len() < 8 {
            break;
        }
        let length = uint(&head, 4, 4, false)?;
        if &head[..4] == b"COMM" {
            let body = bytes_at(handle, position + 8, length.min(26) as usize).await?;
            let rate = extended(body.get(8..18).ok_or_else(|| AudioError("Attempt to access memory outside buffer bounds".into()))?)?;
            let kind = if compressed && body.len() >= 22 { String::from_utf8_lossy(&body[18..22]).into_owned() } else { "NONE".into() };
            common = Some((
                uint(&body, 0, 2, false)? as usize,
                uint(&body, 2, 4, false)? as usize,
                uint(&body, 6, 2, false)? as usize,
                rate,
                kind,
            ));
        } else if &head[..4] == b"SSND" {
            let (channels, frames, mut bits, rate, kind) =
                common.ok_or_else(|| AudioError("That AIFF has its audio before its format; it can't be read.".into()))?;
            let offset = uint(&bytes_at(handle, position + 8, 4).await?, 0, 4, false)?;
            let float = match kind.as_str() {
                "fl32" | "FL32" => {
                    bits = 32;
                    true
                }
                "fl64" | "FL64" => {
                    bits = 64;
                    true
                }
                _ => false,
            };
            if !(float || ["NONE", "twos", "sowt"].contains(&kind.as_str()) && [8, 16, 24, 32].contains(&bits)) {
                return Err(AudioError(format!(
                    "That AIFF is compressed as “{kind}”, which Kumi doesn't read; export it as WAV or plain AIFF."
                )));
            }
            if channels == 0 || rate == 0.0 {
                return Err(AudioError("That AIFF says it has no channels or no sample rate.".into()));
            }
            let data_offset = position + 16 + offset;
            let available = size.saturating_sub(data_offset) as usize / (channels * bits / 8);
            return Ok(Layout {
                sample_rate: kumi_common::js::number::round(rate),
                channels,
                frames: frames.min(available),
                data_offset,
                little_endian: kind == "sowt",
                encoding: Encoding { float, bits, signed: true },
                format: "aiff".into(),
            });
        }
        position += 8 + length + length % 2;
    }
    Err(AudioError("That AIFF has no audio in it.".into()))
}
fn deinterleave(buffer: &[u8], frames: usize, layout: &Layout) -> Vec<Vec<f32>> {
    if !layout.encoding.float && layout.encoding.bits == 16 {
        // Choose byte order once per block. Every signed 16-bit value divided by
        // 32768 is exactly representable in f32, including the source's f64 round trip.
        return if layout.little_endian {
            deinterleave_i16(buffer, frames, layout.channels, i16::from_le_bytes)
        } else {
            deinterleave_i16(buffer, frames, layout.channels, i16::from_be_bytes)
        };
    }
    let mut out = vec![vec![0f32; frames]; layout.channels];
    let step = layout.encoding.bits / 8;
    let mut at = 0;
    for frame in 0..frames {
        for channel in &mut out {
            let bits = uint(buffer, at, step, layout.little_endian).unwrap();
            let value = if layout.encoding.float {
                if step == 4 {
                    f32::from_bits(bits as u32) as f64
                } else {
                    f64::from_bits(bits)
                }
            } else if step == 1 && !layout.encoding.signed {
                (bits as f64 - 128.0) / 128.0
            } else {
                let shift = 64 - layout.encoding.bits;
                ((bits << shift) as i64 >> shift) as f64 / 2f64.powi(layout.encoding.bits as i32 - 1)
            };
            channel[frame] = value as f32;
            at += step;
        }
    }
    out
}

fn deinterleave_i16(buffer: &[u8], frames: usize, channels: usize, decode: impl Fn([u8; 2]) -> i16) -> Vec<Vec<f32>> {
    let mut out = vec![vec![0.0; frames]; channels];
    let stride = 2 * channels;
    for (channel, samples) in out.iter_mut().enumerate() {
        let mut at = channel * 2;
        for sample in samples {
            *sample = decode([buffer[at], buffer[at + 1]]) as f32 / 32768.0;
            at += stride;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_copy_asks_room_for_what_it_will_hold() {
        // A one-shot measured (a reach of 31 s): half a second's copy, 192 kB, not 31 s's.
        assert_eq!(cut_at(Some(31.0), Some(0.5)).or(Some(0.5)).map(|seconds| seconds * BYTES_A_SECOND), Some(192_000.0));
        // A 3-minute song's form (901 s): its 3 minutes.
        assert_eq!(cut_at(Some(901.0), Some(180.0)).or(Some(180.0)), Some(180.0));
        // A 10-minute mix measured: cut at 31 s.
        assert_eq!(cut_at(Some(31.0), Some(600.0)), Some(31.0));
        // A length not known: no cut (copied whole, without asking room, as before).
        assert_eq!(cut_at(Some(31.0), None), None);
        assert!(AudioError::no_room("Only 5 MB is free.").is_no_room());
        assert!(!AudioError("That file is too short to be audio.".into()).is_no_room());
    }

    #[test]
    fn every_signed_16_bit_sample_decodes_exactly_with_both_byte_orders_and_channel_layouts() {
        for little_endian in [false, true] {
            for channels in [1, 2, 3] {
                let frames = 65536;
                let mut buffer = Vec::with_capacity(frames * channels * 2 + 1);
                for frame in 0..frames {
                    for channel in 0..channels {
                        let sample = (frame as u16).wrapping_add(channel as u16 * 17) as i16;
                        buffer.extend_from_slice(&if little_endian { sample.to_le_bytes() } else { sample.to_be_bytes() });
                    }
                }
                buffer.push(0xab); // AudioSource only forwards complete frames.
                let layout = Layout {
                    sample_rate: 48000.0,
                    channels,
                    frames,
                    data_offset: 0,
                    little_endian,
                    encoding: Encoding { float: false, bits: 16, signed: true },
                    format: "wav".into(),
                };
                let decoded = deinterleave(&buffer, frames, &layout);
                for (channel, samples) in decoded.iter().enumerate() {
                    for (frame, sample) in samples.iter().enumerate() {
                        let integer = (frame as u16).wrapping_add(channel as u16 * 17) as i16;
                        let expected = (integer as f64 / 32768.0) as f32;
                        assert_eq!(sample.to_bits(), expected.to_bits(), "little={little_endian}, channel={channel}, frame={frame}");
                    }
                }
            }
        }
    }
}
