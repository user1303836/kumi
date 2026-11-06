//! Reading audio files as blocks of samples, without loading a whole song into memory.

use crate::programs::{find_ffmpeg, run, FfmpegOptions, RunOptions, VideoFailure};
use kumi_common::{
    abort::{Signal, SignalExt},
    js::number::to_fixed,
};
use std::path::{Path, PathBuf};
use tokio::{
    fs::File,
    io::{AsyncReadExt, AsyncSeekExt},
};

/// External decoding policy. Defaults retain standalone discovery and downloads.
/// With `installed_only`, compressed audio requires the explicitly approved local
/// `ffmpeg`; PATH discovery, downloads, ffprobe and platform converters are disabled.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct DecodeOptions {
    #[serde(default)]
    pub installed_only: bool,
    /// An approved absolute executable path, never a program name to resolve on PATH.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ffmpeg: Option<PathBuf>,
}
impl DecodeOptions {
    pub fn is_default(&self) -> bool {
        !self.installed_only && self.ffmpeg.is_none()
    }
}

#[derive(Clone, Copy)]
struct PreparationPolicy<'a> {
    decode: &'a DecodeOptions,
    native_range: bool,
}

/// Restricted decoding accepts self-contained audio containers, never playlists or secondary-file demuxers.
pub(crate) const LOCAL_AUDIO_INPUT: [&str; 4] =
    ["-protocol_whitelist", "file,pipe", "-format_whitelist", "wav,aiff,mp3,flac,aac,mov,ogg,caf"];
// Keep floating-point peaks (including >0 dBFS) and stereo comparisons intact. At
// <=48 kHz and <=2 channels, twelve minutes plus the analyzer's one-second read
// margin fit below this cap, with room for the WAV header and muxer buffering.
const MAX_DECODED_BYTES: u64 = 272 * 1024 * 1024;
const MAX_RESTRICTED_SECONDS: f64 = 721.;

pub const AUDIO_EXTENSIONS: &[&str] =
    &[".wav", ".wave", ".aif", ".aiff", ".aifc", ".mp3", ".m4a", ".aac", ".mp4", ".flac", ".ogg", ".oga", ".opus", ".caf"];
#[derive(Debug, Clone, thiserror::Error)]
#[error("{0}")]
pub struct AudioError(pub String);
/// What a lack of room to convert a file starts with: it says nothing of the file itself.
const NO_ROOM: &str = "Kumi reads that file by converting it first.";
/// What a conversion that ran out of time starts with: the file may convert another time.
const TIMED_OUT: &str = "Converting that file took over";
impl AudioError {
    pub fn no_room(full: &str) -> Self {
        Self(format!("{NO_ROOM} {full}"))
    }
    /// There was no room to convert the file: a learner stops rather than record it as the file's.
    pub fn is_no_room(&self) -> bool {
        self.0.starts_with(NO_ROOM)
    }
    pub fn timed_out(seconds: f64) -> Self {
        Self(format!("{TIMED_OUT} {} minutes, so Kumi stopped: try again later, or export it as WAV.", (seconds / 60.).floor()))
    }
    /// The conversion ran out of time: it may finish another time (a busy computer, a slow disk), so a learner hears the
    /// file later rather than record it as one that won't decode.
    pub fn is_timed_out(&self) -> bool {
        self.0.starts_with(TIMED_OUT)
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
    /// Where the copy begins in the file, seconds: 0 unless only a stretch was converted (`prepare_audio_range`).
    pub start: f64,
    /// The copy holds the whole file, whatever stretch was asked for: a Mac's afconvert can't stop at one.
    whole: bool,
    /// The file's length is known, as `seconds` or as the copy's own: not when a copy stops at a reach and the file's
    /// length couldn't be read.
    length_known: bool,
    folder: Option<PathBuf>,
}
impl PreparedAudio {
    pub async fn cleanup(&self) {
        if let Some(folder) = &self.folder {
            let _ = tokio::fs::remove_dir_all(folder).await;
        }
    }
    /// A copy of the whole file again, for another stretch of it: the same copy, linked under a path of its own in a
    /// folder of its own, rather than the file converted whole once more. None for any other copy, or when it can't be
    /// linked.
    pub async fn linked(&self) -> Option<PreparedAudio> {
        if !self.whole {
            return None;
        }
        let folder = std::env::temp_dir().join(format!("kumi-audio-{}", uuid::Uuid::new_v4()));
        tokio::fs::create_dir(&folder).await.ok()?;
        let path = folder.join("converted.wav");
        if tokio::fs::hard_link(&self.path, &path).await.is_err() {
            let _ = tokio::fs::remove_dir_all(&folder).await;
            return None;
        }
        Some(PreparedAudio {
            path,
            format: self.format.clone(),
            seconds: self.seconds,
            start: self.start,
            whole: true,
            length_known: self.length_known,
            folder: Some(folder),
        })
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
/// isn't written out whole (2.5 GB of it) for its first twelve minutes, and the length it's read as is kept.
pub async fn prepare_audio_to(path: impl AsRef<Path>, signal: Option<Signal>, until: Option<f64>) -> Result<PreparedAudio, AudioError> {
    prepare_audio_to_with_options(path, signal, until, &DecodeOptions::default()).await
}
/// As `prepare_audio_to`, with an explicit external decoding policy.
pub async fn prepare_audio_to_with_options(
    path: impl AsRef<Path>,
    signal: Option<Signal>,
    until: Option<f64>,
    options: &DecodeOptions,
) -> Result<PreparedAudio, AudioError> {
    prepare(path.as_ref(), signal, until, None, None, PreparationPolicy { decode: options, native_range: false }).await
}
/// `known`: the file's length in seconds when it's known already (a song's tags say it), read again from the file only
/// when it isn't.
async fn prepare(
    path: &Path,
    signal: Option<Signal>,
    until: Option<f64>,
    from: Option<f64>,
    known: Option<f64>,
    policy: PreparationPolicy<'_>,
) -> Result<PreparedAudio, AudioError> {
    let PreparationPolicy { decode: options, native_range } = policy;
    let extension = extension(path);
    if !AUDIO_EXTENSIONS.contains(&extension.as_str()) {
        return Err(AudioError(format!(
            "{} isn't an audio format Kumi reads (WAV, AIFF, MP3, M4A, FLAC, Ogg).",
            if extension.is_empty() { "That file" } else { &extension }
        )));
    }
    if [".wav", ".wave", ".aif", ".aiff", ".aifc"].contains(&extension.as_str()) {
        return Ok(PreparedAudio {
            path: path.into(),
            format: None,
            seconds: None,
            start: 0.,
            whole: false,
            length_known: true,
            folder: None,
        });
    }
    // Restricted conversion is always an excerpt, even when the caller requests
    // an unbounded file. Its measured end is the decoded EOF, not the probed length.
    let until = if options.installed_only && !native_range {
        Some(until.unwrap_or(MAX_RESTRICTED_SECONDS).min(MAX_RESTRICTED_SECONDS))
    } else {
        until
    };
    let folder = std::env::temp_dir().join(format!("kumi-audio-{}", uuid::Uuid::new_v4()));
    kumi_types::private::create_dir_all(&folder)?;
    let mut prepared = PreparedAudio {
        path: folder.join("converted.wav"),
        format: Some(extension[1..].into()),
        seconds: None,
        start: 0.,
        whole: false,
        length_known: false,
        folder: Some(folder),
    };
    match convert(path, &prepared.path, signal, until, from, known, policy).await {
        Ok((length, start, whole)) => {
            (prepared.seconds, prepared.start, prepared.whole) =
                (if native_range { length } else { length_past(until, length) }, start, whole);
            // A copy that stops at a reach tells the file's length only when it was read.
            prepared.length_known = whole || until.is_none() || length.is_some();
        }
        Err(error) => {
            prepared.cleanup().await;
            return Err(error);
        }
    }
    Ok(prepared)
}
/// A stretch of a file Kumi can read as PCM: `seconds` from `from` on. WAV and AIFF are read in place; other formats are
/// converted from `from` (ffmpeg seeks there first, so a song's middle minute is decoded without what comes before),
/// or whole where only a Mac's afconvert is there. `start` says where the copy begins in the file.
pub async fn prepare_audio_range(
    path: impl AsRef<Path>,
    signal: Option<Signal>,
    from: f64,
    seconds: f64,
) -> Result<PreparedAudio, AudioError> {
    prepare_audio_range_of(path, signal, from, seconds, None).await
}
/// As prepare_audio_range, for a file whose length (seconds) may be known already: a song's tags say it, and a scan
/// starts no process to read it again.
pub async fn prepare_audio_range_of(
    path: impl AsRef<Path>,
    signal: Option<Signal>,
    from: f64,
    seconds: f64,
    length: Option<f64>,
) -> Result<PreparedAudio, AudioError> {
    prepare(
        path.as_ref(),
        signal,
        Some(from.max(0.) + seconds.max(0.)),
        Some(from.max(0.)).filter(|from| *from > 0.),
        length,
        PreparationPolicy { decode: &DecodeOptions::default(), native_range: false },
    )
    .await
}

/// A bounded, native-rate/channel decode for approved reference measurement. Unlike
/// ordinary restricted listening, this never negotiates a downmix or sample rate.
/// The copy starts on an integral second before the requested window; callers then
/// select exact sample frames, without depending on a converter's fractional seek.
pub async fn open_audio_range_with_options(
    path: impl AsRef<Path>,
    signal: Option<Signal>,
    from: f64,
    seconds: f64,
    options: &DecodeOptions,
) -> Result<AudioRange, AudioError> {
    if !options.installed_only {
        return Err(AudioError("Reference measurement needs the installed-only decoder policy.".into()));
    }
    if !from.is_finite() || from < 0. || !seconds.is_finite() || seconds <= 0. || !(from + seconds).is_finite() {
        return Err(AudioError("Choose a finite, non-negative start and a positive finite duration.".into()));
    }
    let copy_start = (from.floor() - 1.).max(0.);
    let end = from + seconds + 1.;
    let prepared = prepare(
        path.as_ref(), signal, Some(end), Some(copy_start), None, PreparationPolicy { decode: options, native_range: true },
    ).await?;
    let handle = File::open(&prepared.path).await?;
    let mut source = open_pcm(handle).await?;
    let native_pcm = prepared.format.is_none();
    let original_frames = native_pcm.then_some(source.frames);
    let original_seconds = if native_pcm { Some(source.frames as f64 / source.sample_rate) } else { prepared.seconds };
    let start = if native_pcm { 0. } else { prepared.start };
    if let Some(format) = &prepared.format {
        source.format = format.clone();
    }
    source.prepared = Some(prepared);
    Ok(AudioRange { source, start, original_frames, original_seconds })
}

/// Source identity remains the original file. `source` contains a possibly bounded
/// decoded copy whose sample zero corresponds to `start` in that original.
pub struct AudioRange {
    pub source: AudioSource,
    pub start: f64,
    pub original_frames: Option<usize>,
    /// Container duration when available; compressed containers may only estimate it.
    pub original_seconds: Option<f64>,
}
fn extension(path: &Path) -> String {
    path.extension().map(|v| format!(".{}", v.to_string_lossy().to_lowercase())).unwrap_or_default()
}
pub async fn open_audio(path: impl AsRef<Path>, signal: Option<Signal>) -> Result<AudioSource, AudioError> {
    open_audio_to(path, signal, None).await
}
/// As open_audio, reading no further than `until` seconds in (see prepare_audio_to); `frames` is still the whole file's
/// when its length is read (a length that can't be read leaves the copy's, and `length` says so).
pub async fn open_audio_to(path: impl AsRef<Path>, signal: Option<Signal>, until: Option<f64>) -> Result<AudioSource, AudioError> {
    open_audio_to_with_options(path, signal, until, &DecodeOptions::default()).await
}
/// As `open_audio_to`, with an explicit external decoding policy.
pub async fn open_audio_to_with_options(
    path: impl AsRef<Path>,
    signal: Option<Signal>,
    until: Option<f64>,
    options: &DecodeOptions,
) -> Result<AudioSource, AudioError> {
    let prepared = prepare_audio_to_with_options(path, signal, until, options).await?;
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
/// An explicitly approved program may be canonicalized to a local verbatim disk
/// path on Windows. No UNC, generic device or NT namespace is an approved local disk.
fn local_decoder_path(path: &Path, named: &str) -> bool {
    if !path.is_absolute() || named.chars().any(char::is_control) {
        return false;
    }
    #[cfg(windows)]
    {
        use std::path::{Component, Prefix};
        matches!(
            path.components().next(),
            Some(Component::Prefix(prefix)) if matches!(prefix.kind(), Prefix::Disk(_) | Prefix::VerbatimDisk(_))
        )
    }
    #[cfg(not(windows))]
    {
        !named.starts_with('\\') && !named.starts_with("//")
    }
}
/// The copy converted, the file's length when it was read (or `known`), where the copy begins in the file (`from`, when a
/// converter could seek to it), and whether it holds the whole file whatever was asked (afconvert's).
async fn convert(
    input: &Path,
    output: &Path,
    signal: Option<Signal>,
    until: Option<f64>,
    from: Option<f64>,
    known: Option<f64>,
    policy: PreparationPolicy<'_>,
) -> Result<(Option<f64>, f64, bool), AudioError> {
    let PreparationPolicy { decode: options, native_range } = policy;
    let mac = cfg!(target_os = "macos") && !options.installed_only;
    let ffmpeg = if let Some(path) = &options.ffmpeg {
        let named = path.to_str().ok_or_else(|| AudioError("The approved ffmpeg dependency path isn't valid UTF-8.".into()))?;
        // Refuse network/device namespaces and control characters before touching the path.
        if !local_decoder_path(path, named) {
            return Err(AudioError("The approved ffmpeg dependency is unavailable: select a local absolute executable path without control characters at setup, or export as WAV/AIFF.".into()));
        }
        if !path.is_file() {
            return Err(AudioError("The approved ffmpeg dependency is unavailable: select an existing absolute executable path at setup, or export as WAV/AIFF.".into()));
        }
        named.to_owned()
    } else if options.installed_only {
        return Err(AudioError(
            "The approved ffmpeg dependency is unavailable: approve a local decoder at setup, or export as WAV/AIFF.".into(),
        ));
    } else if mac {
        "ffmpeg".into()
    } else {
        match find_ffmpeg(FfmpegOptions { signal: signal.clone(), ..Default::default() }).await {
            Ok(found) => found.unwrap_or_else(|| "ffmpeg".into()),
            Err(VideoFailure::Video(e)) => return Err(AudioError(e.0)),
            Err(_) => "ffmpeg".into(),
        }
    };
    let input_s = if options.installed_only {
        let name = input.to_str().ok_or_else(|| AudioError("The audio path isn't valid UTF-8.".into()))?;
        if !local_decoder_path(input, name) {
            return Err(AudioError("Restricted decoding needs a local absolute audio path.".into()));
        }
        let canonical = tokio::fs::canonicalize(input).await?;
        let name = canonical.to_str().ok_or_else(|| AudioError("The audio path isn't valid UTF-8.".into()))?;
        if !local_decoder_path(&canonical, name) {
            return Err(AudioError("Restricted decoding needs a local absolute audio path.".into()));
        }
        std::borrow::Cow::Owned(format!("file:{name}"))
    } else {
        input.to_string_lossy()
    };
    let output_s = output.to_string_lossy();
    let folder = output.parent().map(|folder| folder.to_string_lossy().into_owned()).unwrap_or_default();
    let isolated_dir = if options.installed_only {
        Some(output.parent().ok_or_else(|| AudioError("Decoded audio needs an owned scratch folder.".into()))?)
    } else {
        None
    };
    let output_limit = options.installed_only.then(|| MAX_DECODED_BYTES.to_string());
    // How long it is (ffprobe, or what ffmpeg says of it, decoding nothing): to ask room for what will be written, and to
    // keep the file's length when the copy stops short of it. A reading can be wrong, so where ffmpeg stops isn't up to it.
    // (A stretch's length is known: nothing to read first. Nor is a length known already read again.)
    //
    // Read after the copy, `later`, when the reading changes nothing before it: there's room for the copy to the reach
    // (so for one stopping sooner too), and the reach's conversion has the same time whatever the reading. ffmpeg then
    // says the file's length as it opens it to convert (its Duration line: the reading the probe makes), and a file
    // ending well before the reach keeps the copy's length whatever the probe would say: the probe runs only for the rest.
    let later = native_range || match until {
        Some(until) if from.is_none() && known.is_none() && until <= READ_LATER_UP_TO => {
            kumi_types::disk::low_disk(&folder, until * BYTES_A_SECOND, "Kumi reads audio on").await.is_none()
        }
        _ => false,
    };
    let mut length = match until {
        Some(_) if from.is_none() => match known {
            Some(known) => Some(known),
            None if later => None,
            None => crate::frames::duration_of_in(&ffmpeg, &input_s, signal.clone(), isolated_dir).await,
        },
        _ => None,
    };
    // From a seek, ffmpeg's -t is the stretch's length.
    let reach = until.map(|until| until - from.unwrap_or(0.));
    let until_text = reach.map(|reach| if native_range { reach.to_string() } else { to_fixed(reach, 3) });
    let from_text = from.map(|from| if native_range { from.to_string() } else { to_fixed(from, 3) });
    // How big the file is: how long converting it may take when its length isn't known.
    let bytes = tokio::fs::metadata(input).await.map_or(0, |metadata| metadata.len());
    let (mut no_room, mut timed_out) = (None, None);
    let mut attempts = attempts(
        mac,
        &ffmpeg,
        &input_s,
        &output_s,
        reach.zip(until_text.as_deref()),
        length,
        from_text.as_deref(),
        later,
        output_limit.as_deref(),
    );
    if native_range {
        for attempt in &mut attempts {
            if let Some(at) = attempt.args.iter().position(|arg| *arg == "-af") {
                attempt.args.drain(at..at + 2);
            }
        }
    }
    for Attempt { command, args, mut writes } in attempts {
        if let Some(s) = &signal {
            s.check()?;
        }
        // ffmpeg didn't convert it: its length is read now, as it would have been first, for afconvert's room.
        if command == "afconvert" && later {
            length = crate::frames::duration_of_in(&ffmpeg, &input_s, signal.clone(), isolated_dir).await;
            writes = length;
        }
        // A Mac's afconvert copies the whole file, a stretch's too, whose length wasn't read: read it now, to ask room
        // for the copy.
        if command == "afconvert" && writes.is_none() && from.is_some() {
            writes = match known {
                Some(known) => Some(known),
                None => crate::frames::duration_of_in(&ffmpeg, &input_s, signal.clone(), isolated_dir).await,
            };
        }
        match writes {
            Some(seconds) => {
                if let Some(full) = kumi_types::disk::low_disk(&folder, seconds * BYTES_A_SECOND, "Kumi reads audio on").await {
                    no_room = Some(full);
                    continue;
                }
            }
            // How much it would write isn't known: not tried once another had no room.
            None if no_room.is_some() => continue,
            None => {}
        }
        let timeout = conversion_timeout_ms(writes, bytes);
        let timeout = if options.installed_only { timeout.min(120_000) } else { timeout };
        // What ffmpeg says as it opens the file, when it says it, is more than its errors: room for it.
        let max_buffer = if later && command != "afconvert" { SAYS_LENGTH_BUFFER } else { 1024 * 1024 };
        let result = run(
            command,
            &args,
            RunOptions {
                signal: signal.clone(),
                timeout_ms: Some(timeout),
                max_buffer: Some(max_buffer),
                isolated_dir: isolated_dir.map(Path::to_path_buf),
            },
        )
        .await;
        if options.installed_only
            && result.is_ok()
            && tokio::fs::metadata(output).await.is_ok_and(|metadata| metadata.len() >= MAX_DECODED_BYTES)
        {
            return Err(AudioError("Decoded audio exceeds the 272 MiB limit; choose a shorter excerpt.".into()));
        }
        match result {
            Ok(_) if command == "afconvert" => return Ok((None, 0., true)),
            Ok(output) if native_range => return Ok((crate::frames::duration_in(&output.stderr), from.unwrap_or(0.), false)),
            Ok(output) if later => {
                let length = match (crate::frames::duration_in(&output.stderr), until) {
                    // Whole in its copy: read by the probe, it would be as long as the copy (not past the reach).
                    (Some(said), Some(until)) if said + WHOLE_BEFORE_REACH <= until => Some(said),
                    _ => crate::frames::duration_of_in(&ffmpeg, &input_s, signal.clone(), isolated_dir).await,
                };
                // Stopped while it was read: stopped, as when it was read first, not a length unknown.
                if let Some(s) = &signal {
                    s.check()?;
                }
                return Ok((length, 0., false));
            }
            Ok(_) => return Ok((length, from.unwrap_or(0.), false)),
            Err(e) => {
                if signal.as_ref().is_some_and(Signal::is_cancelled) {
                    return Err(e.into());
                }
                if e.is_timed_out() {
                    timed_out = timed_out.max(Some(timeout));
                }
            }
        }
    }
    if let Some(full) = no_room {
        return Err(AudioError::no_room(&full));
    }
    if let Some(timeout) = timed_out {
        return Err(AudioError::timed_out(timeout as f64 / 1000.));
    }
    if options.installed_only {
        return Err(AudioError(
            "The approved ffmpeg dependency couldn't decode this file. Check the approved executable, or export as WAV/AIFF.".into(),
        ));
    }
    Err(AudioError(format!("Kumi couldn't decode {} here: it reads WAV and AIFF itself, and other formats with {}ffmpeg. Install ffmpeg, or export the file as WAV.",input.extension().unwrap_or_default().to_string_lossy().to_uppercase(),if mac {"macOS's afconvert or "}else{""})))
}
/// How far before the reach a file must end, by ffmpeg's reading of its length as it converts, for the copy to stand for
/// its length without the probe: well past the difference between that reading (rounded to hundredths) and the probe's.
const WHOLE_BEFORE_REACH: f64 = 1.0;
/// The farthest reach whose length is read after the copy: up to it, a conversion has two minutes whatever the length
/// read (`conversion_timeout_ms`).
const READ_LATER_UP_TO: f64 = 1_200.0;
/// What ffmpeg may say converting a file it says the length of: its errors, and what it opens and writes.
const SAYS_LENGTH_BUFFER: usize = 16 * 1024 * 1024;
/// f32 at up to 48 kHz stereo: a copy's size a second.
const BYTES_A_SECOND: f64 = 384_000.0;
/// How long a conversion may take: two minutes, or a second for every ten seconds of audio it writes when that's longer
/// (a Mac's afconvert copies a two-hour mix whole, and a busy computer converts slowly), up to an hour: a converter that
/// stalls (on a drive that went away, say) holds the scan no longer. A length that isn't known is read from the file's
/// size as a 64 kbps file's: longer than most files that size are.
fn conversion_timeout_ms(writes: Option<f64>, bytes: u64) -> u64 {
    let seconds = writes.unwrap_or(bytes as f64 / 8_000.);
    (seconds * 100.).clamp(120_000., 3_600_000.) as u64
}
/// A converter to try: its command and arguments, and how many seconds of the file it writes, when that's known.
struct Attempt<'a> {
    command: &'a str,
    args: Vec<&'a str>,
    writes: Option<f64>,
}
/// The converters, in the order they're tried. ffmpeg stops at `until` (its text beside it) whatever the length read says,
/// since a reading can fall short (an MP3's estimate); a Mac's afconvert can't stop, and copies whole. So a read with a
/// reach tries ffmpeg first, and a whole read afconvert, as always.
fn attempts<'a>(
    mac: bool,
    ffmpeg: &'a str,
    input: &'a str,
    output: &'a str,
    until: Option<(f64, &'a str)>,
    length: Option<f64>,
    from: Option<&'a str>,
    says_length: bool,
    restricted_output_limit: Option<&'a str>,
) -> Vec<Attempt<'a>> {
    // Saying the file's length as it opens it (its Duration line, at info): nothing else changes in what it writes.
    let mut args = Vec::with_capacity(24);
    if says_length {
        args.extend(["-hide_banner", "-nostats", "-v", "info", "-nostdin", "-y"]);
    } else {
        args.extend(["-v", "error", "-nostdin", "-y"]);
    }
    if restricted_output_limit.is_some() {
        if !says_length {
            args.push("-hide_banner");
        }
        args.extend(LOCAL_AUDIO_INPUT);
    }
    if let Some(from) = from {
        args.extend(["-ss", from]);
    }
    args.extend(["-i", input, "-vn"]);
    if let Some((_, until)) = until {
        args.extend(["-t", until]);
    }
    args.extend(["-acodec", "pcm_f32le", "-f", "wav"]);
    if let Some(limit) = restricted_output_limit {
        // Negotiate only bounded layouts/rates, retaining mono and common native
        // sample rates rather than upmixing everything or quantizing to integer PCM.
        args.extend(["-af", "aformat=sample_rates=48000|44100|32000|24000|22050|16000|12000|11025|8000:channel_layouts=mono|stereo"]);
        args.extend(["-fs", limit]);
    }
    args.push(output);
    // No further than the reach, nor than the file's length when it's read as shorter.
    let writes = until.map(|(until, _)| length.map_or(until, |length| until.min(length)));
    let by_ffmpeg = Attempt { command: ffmpeg, args, writes };
    if !mac {
        return vec![by_ffmpeg];
    }
    let by_afconvert = Attempt { command: "afconvert", args: vec!["-f", "WAVE", "-d", "LEF32", input, output], writes: length };
    if until.is_some() {
        vec![by_ffmpeg, by_afconvert]
    } else {
        vec![by_afconvert, by_ffmpeg]
    }
}
/// The file's length, kept when the copy stops short of it: when it's read as longer than `until`.
fn length_past(until: Option<f64>, length: Option<f64>) -> Option<f64> {
    until.zip(length).filter(|(until, length)| until < length).map(|(_, length)| length)
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
    /// The file's whole length in seconds: None when what's open is a copy that stops at a reach and the file's length
    /// couldn't be read (`frames` is then the copy's).
    pub fn length(&self) -> Option<f64> {
        self.prepared.as_ref().is_none_or(|prepared| prepared.length_known).then(|| self.frames as f64 / self.sample_rate)
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
/// Channels of samples as a 32-bit float WAV.
pub fn float_wav(channels: &[Vec<f32>], rate: u32) -> Vec<u8> {
    let count = channels.len().max(1) as u16;
    let frames = channels.first().map_or(0, Vec::len);
    let data = frames * count as usize * 4;
    let mut wav = Vec::with_capacity(44 + data);
    wav.extend(b"RIFF");
    wav.extend((36 + data as u32).to_le_bytes());
    wav.extend(b"WAVEfmt ");
    wav.extend(16u32.to_le_bytes());
    wav.extend(3u16.to_le_bytes());
    wav.extend(count.to_le_bytes());
    wav.extend(rate.to_le_bytes());
    wav.extend((rate * count as u32 * 4).to_le_bytes());
    wav.extend((count * 4).to_le_bytes());
    wav.extend(32u16.to_le_bytes());
    wav.extend(b"data");
    wav.extend((data as u32).to_le_bytes());
    for frame in 0..frames {
        for channel in channels {
            wav.extend(channel.get(frame).copied().unwrap_or(0.).to_le_bytes());
        }
    }
    wav
}

/// `most` seconds of a file from `from` (seconds) as two channels (a mono file's twice, a file of more channels its first two),
/// with its rate.
pub async fn read_stereo(path: impl AsRef<Path>, signal: &Signal, from: f64, most: f64) -> Result<([Vec<f32>; 2], f64), AudioError> {
    let mut source = open_audio_to(path, Some(signal.clone()), Some(from + most)).await?;
    let rate = source.sample_rate;
    source.seek(from * rate);
    let wanted = (rate * most) as usize;
    let (mut left, mut right): (Vec<f32>, Vec<f32>) = (Vec::new(), Vec::new());
    while left.len() < wanted {
        let Some(block) = source.read(65_536).await? else { break };
        let Some(first) = block.first() else { break };
        left.extend_from_slice(first);
        right.extend_from_slice(block.get(1).unwrap_or(first));
    }
    let _ = source.close().await;
    left.truncate(wanted);
    right.truncate(wanted);
    Ok(([left, right], rate))
}

/// The most channels a file's peaks are read across: more than any sound file Kumi meets.
const MAX_CHANNELS: usize = 64;

/// A file at a glance: how long it is in seconds, and its level along it as `columns` peaks, each the loudest sample
/// (of any channel) in its stretch, scaled so the file's loudest is 1. Read a block at a time, so a song isn't held
/// in memory.
pub async fn peaks(path: impl AsRef<Path>, columns: usize, signal: &Signal) -> Result<(f64, Vec<f32>), AudioError> {
    let mut source = open_audio(path, Some(signal.clone())).await?;
    // (a header claiming thousands of channels would make each block as big as the file)
    if source.channels > MAX_CHANNELS {
        let _ = source.close().await;
        return Err(AudioError(format!("That file says it has {} channels.", source.channels)));
    }
    let frames = source.frames.max(1);
    let columns = columns.clamp(1, frames);
    let mut peaks = vec![0f32; columns];
    let mut at = 0;
    while let Some(block) = source.read(65_536).await? {
        if signal.is_cancelled() {
            let _ = source.close().await;
            return Err(AudioError("Stopped.".into()));
        }
        let got = block.first().map_or(0, Vec::len);
        for frame in 0..got {
            let column = ((at + frame) * columns / frames).min(columns - 1);
            let level = block.iter().filter_map(|channel| channel.get(frame)).fold(0f32, |most, sample| most.max(sample.abs()));
            peaks[column] = peaks[column].max(level);
        }
        at += got;
    }
    let seconds = frames as f64 / source.sample_rate;
    let _ = source.close().await;
    let loudest = peaks.iter().copied().fold(0f32, f32::max);
    if loudest > 0. {
        peaks.iter_mut().for_each(|peak| *peak = (*peak / loudest).min(1.));
    }
    Ok((seconds, peaks))
}

/// A 16-bit mono WAV of `mono` at `rate`: the tests' sound files.
#[cfg(any(test, feature = "test-support"))]
pub fn write_test_wav(path: &Path, mono: &[f32], rate: u32) {
    let frames = mono.len() as u32;
    let mut bytes = Vec::with_capacity(44 + mono.len() * 2);
    bytes.extend(b"RIFF");
    bytes.extend((36 + frames * 2).to_le_bytes());
    bytes.extend(b"WAVEfmt ");
    bytes.extend(16u32.to_le_bytes());
    bytes.extend(1u16.to_le_bytes());
    bytes.extend(1u16.to_le_bytes());
    bytes.extend(rate.to_le_bytes());
    bytes.extend((rate * 2).to_le_bytes());
    bytes.extend(2u16.to_le_bytes());
    bytes.extend(16u16.to_le_bytes());
    bytes.extend(b"data");
    bytes.extend((frames * 2).to_le_bytes());
    for value in mono {
        bytes.extend(((value.clamp(-1., 1.) * 32767.) as i16).to_le_bytes());
    }
    std::fs::write(path, bytes).unwrap();
}

/// `most` seconds of a file from `from` (seconds) as one mono channel (its channels averaged), with its rate and its whole
/// length in seconds, when that's known (`AudioSource::length`).
pub async fn read_mono(path: impl AsRef<Path>, signal: &Signal, from: f64, most: f64) -> Result<(Vec<f32>, f64, Option<f64>), AudioError> {
    let mut source = open_audio_to(path, Some(signal.clone()), Some(from + most)).await?;
    let rate = source.sample_rate;
    source.seek(from * rate);
    let wanted = (rate * most) as usize;
    let mut mono: Vec<f32> = Vec::new();
    while mono.len() < wanted {
        let Some(block) = source.read(65_536).await? else { break };
        let channels = block.len().max(1);
        let frames = block.first().map_or(0, Vec::len);
        for frame in 0..frames {
            mono.push(block.iter().map(|channel| channel[frame]).sum::<f32>() / channels as f32);
        }
    }
    let length = source.length();
    let _ = source.close().await;
    mono.truncate(wanted);
    Ok((mono, rate, length))
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

    #[cfg(windows)]
    #[test]
    fn approved_decoder_paths_accept_canonical_local_disks_but_not_remote_or_device_namespaces() {
        for named in [r"C:\tools\ffmpeg.exe", r"\\?\C:\tools\ffmpeg.exe"] {
            assert!(local_decoder_path(Path::new(named), named), "{named}");
        }
        for named in [
            r"\\server\share\ffmpeg.exe",
            "//server/share/ffmpeg.exe",
            r"\\?\UNC\server\share\ffmpeg.exe",
            r"\\.\C:\tools\ffmpeg.exe",
            r"\\?\GLOBALROOT\Device\HarddiskVolume1\ffmpeg.exe",
            r"\??\C:\tools\ffmpeg.exe",
            r"\\?\Volume{12345678-1234-1234-1234-123456789abc}\ffmpeg.exe",
            r"C:ffmpeg.exe",
            r"\tools\ffmpeg.exe",
            "C:\\tools\\decoder\n.exe",
        ] {
            assert!(!local_decoder_path(Path::new(named), named), "{named}");
        }
    }

    #[test]
    fn a_copy_asks_room_for_what_it_will_hold_and_stops_at_the_reach_whatever_the_length_read() {
        // Each converter tried, in order: where it stops (`-t`), and the seconds it's asked room for.
        let plan = |mac, until: Option<f64>, length| {
            let text = until.map(|until| to_fixed(until, 3));
            attempts(mac, "ffmpeg", "in.mp3", "out.wav", until.zip(text.as_deref()), length, None, false, None)
                .into_iter()
                .map(|tried| {
                    let stop = tried.args.iter().position(|arg| *arg == "-t").map(|at| tried.args[at + 1].to_string());
                    (tried.command.to_string(), stop, tried.writes)
                })
                .collect::<Vec<_>>()
        };
        let at = |seconds: &str| Some(seconds.to_string());
        // A one-shot measured (a reach of 31 s): half a second's copy, 192 kB, not 31 s's.
        assert_eq!(plan(false, Some(31.0), Some(0.5)), [("ffmpeg".to_string(), at("31.000"), Some(0.5))]);
        assert_eq!(0.5 * BYTES_A_SECOND, 192_000.0);
        // A 3-minute song's form (901 s): its 3 minutes.
        assert_eq!(plan(false, Some(901.0), Some(180.0)), [("ffmpeg".to_string(), at("901.000"), Some(180.0))]);
        // A 10-minute mix measured: cut at 31 s, and its length is kept.
        assert_eq!(plan(false, Some(31.0), Some(600.0)), [("ffmpeg".to_string(), at("31.000"), Some(31.0))]);
        assert_eq!(length_past(Some(31.0), Some(600.0)), Some(600.0));
        // A length read short (an MP3's estimate of its first frames) or not read: the copy still stops at the reach.
        assert_eq!(plan(false, Some(31.0), Some(20.0)), [("ffmpeg".to_string(), at("31.000"), Some(20.0))]);
        assert_eq!(length_past(Some(31.0), Some(20.0)), None);
        assert_eq!(plan(false, Some(31.0), None), [("ffmpeg".to_string(), at("31.000"), Some(31.0))]);
        assert_eq!(length_past(Some(31.0), None), None);
        // A Mac's afconvert copies whole: with a reach it's tried after ffmpeg, asked room for the whole length read (or,
        // not known, not tried once ffmpeg had no room).
        assert_eq!(
            plan(true, Some(31.0), Some(600.0)),
            [("ffmpeg".to_string(), at("31.000"), Some(31.0)), ("afconvert".to_string(), None, Some(600.0))]
        );
        assert_eq!(plan(true, Some(31.0), None), [("ffmpeg".to_string(), at("31.000"), Some(31.0)), ("afconvert".to_string(), None, None)]);
        // Read whole: afconvert first on a Mac, as always, and no room asked.
        assert_eq!(plan(true, None, None), [("afconvert".to_string(), None, None), ("ffmpeg".to_string(), None, None)]);
        assert_eq!(plan(false, None, None), [("ffmpeg".to_string(), None, None)]);
        assert!(AudioError::no_room("Only 5 MB is free.").is_no_room());
        assert!(!AudioError("That file is too short to be audio.".into()).is_no_room());
        // Standalone's external decoder contract stays unchanged: info exposes Duration without changing
        // PCM conversion, stream selection or the bounded reach; quiet suppresses it and never reads stdin.
        let said = attempts(false, "ffmpeg", "in.mp3", "out.wav", Some((31.0, "31.000")), None, None, true, None);
        assert_eq!(
            said[0].args,
            [
                "-hide_banner",
                "-nostats",
                "-v",
                "info",
                "-nostdin",
                "-y",
                "-i",
                "in.mp3",
                "-vn",
                "-t",
                "31.000",
                "-acodec",
                "pcm_f32le",
                "-f",
                "wav",
                "out.wav"
            ]
        );
        let quiet = attempts(false, "ffmpeg", "in.mp3", "out.wav", Some((31.0, "31.000")), None, None, false, None);
        assert_eq!(said[0].args[6..], quiet[0].args[4..]);
        assert_eq!(quiet[0].args[..4], ["-v", "error", "-nostdin", "-y"]);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_files_peaks_follow_its_level_along_it() {
        let folder = std::env::temp_dir().join(format!("kumi-peaks-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&folder).unwrap();
        let path = folder.join("hit.wav");
        // A second at 1 kHz: quiet, then loud for a quarter, then silent.
        let level = |n: usize| {
            if n < 500 {
                0.1
            } else if n < 750 {
                0.5
            } else {
                0.
            }
        };
        let mono: Vec<f32> = (0..1000).map(|n| if n % 2 == 0 { level(n) } else { -level(n) }).collect();
        write_test_wav(&path, &mono, 1000);
        let (seconds, peaks) = peaks(&path, 4, &Signal::new()).await.unwrap();
        assert_eq!(seconds, 1.);
        assert_eq!(peaks.len(), 4);
        assert!((peaks[0] - 0.2).abs() < 0.01 && (peaks[1] - 0.2).abs() < 0.01, "{peaks:?}");
        assert_eq!((peaks[2], peaks[3]), (1., 0.), "the loudest is 1, silence 0");
        // More columns than samples: one a sample.
        assert_eq!(super::peaks(&path, 5000, &Signal::new()).await.unwrap().1.len(), 1000);
        std::fs::remove_dir_all(&folder).unwrap();
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_copy_of_the_whole_file_is_linked_for_another_stretch_and_no_other_copy_is() {
        let folder = std::env::temp_dir().join(format!("kumi-audio-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&folder).unwrap();
        let path = folder.join("converted.wav");
        std::fs::write(&path, b"RIFF the whole mix").unwrap();
        let whole = PreparedAudio {
            path,
            format: Some("mp3".into()),
            seconds: None,
            start: 0.,
            whole: true,
            length_known: true,
            folder: Some(folder.clone()),
        };
        let linked = whole.linked().await.expect("linked");
        assert_ne!(linked.path, whole.path);
        assert_eq!((linked.format.as_deref(), linked.seconds, linked.start), (Some("mp3"), None, 0.));
        assert_eq!(std::fs::read(&linked.path).unwrap(), b"RIFF the whole mix");
        // Each copy goes with its own folder, and the other's stays.
        drop(whole);
        assert!(!folder.exists());
        let again = linked.linked().await.expect("linked again");
        let gone = linked.folder.clone().unwrap();
        drop(linked);
        assert!(!gone.exists());
        assert_eq!(std::fs::read(&again.path).unwrap(), b"RIFF the whole mix");
        // A stretch's own copy isn't the whole file.
        let stretch = PreparedAudio {
            path: again.path.clone(),
            format: Some("mp3".into()),
            seconds: Some(600.),
            start: 445.,
            whole: false,
            length_known: false,
            folder: None,
        };
        assert!(stretch.linked().await.is_none());
    }

    #[test]
    fn a_conversion_has_time_for_the_audio_it_writes() {
        // A stretch, or a short file: two minutes, as before.
        assert_eq!(conversion_timeout_ms(Some(31.), 50_000_000), 120_000);
        assert_eq!(conversion_timeout_ms(None, 5_000_000), 120_000);
        // A two-hour mix copied whole: twelve minutes; its length not known, from its size (a 320 kbps MP3's 288 MB).
        assert_eq!(conversion_timeout_ms(Some(7_200.), 0), 720_000);
        assert_eq!(conversion_timeout_ms(None, 288_000_000), 3_600_000);
        // An hour at most, however big the file: a 2 GB one would otherwise wait seven hours on a converter that stalls.
        assert_eq!(conversion_timeout_ms(None, 2_000_000_000), 3_600_000);
        // Up to the farthest reach whose length is read after the copy, the time is the same whatever the length read.
        for writes in [0., 0.5, 31., 600., READ_LATER_UP_TO] {
            assert_eq!(conversion_timeout_ms(Some(writes), 0), 120_000);
        }
        assert!(conversion_timeout_ms(Some(READ_LATER_UP_TO + 10.), 0) > 120_000);
        let late = AudioError::timed_out(720.);
        assert_eq!(late.to_string(), "Converting that file took over 12 minutes, so Kumi stopped: try again later, or export it as WAV.");
        assert!(late.is_timed_out() && !late.is_no_room() && !AudioError::no_room("Only 5 MB is free.").is_timed_out());
    }

    /// A compressed file read to a reach (`read_mono`'s 6 s) whose length its converter can't say: the copy's length isn't
    /// the file's. With ffprobe absent and ffmpeg's own probe failing or saying, here by a fake ffmpeg.
    #[cfg(unix)]
    #[tokio::test(flavor = "current_thread")]
    async fn a_copy_to_a_reach_says_the_files_length_only_when_it_was_read() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        write_test_wav(&dir.path().join("copy.wav"), &[0.25; 48_000], 8_000);
        let ffmpeg = dir.path().join("ffmpeg");
        let script = format!(
            "#!/bin/sh\ncase \"$*\" in *\"-f null\"*) [ -f '{0}/probe' ] || exit 1; /bin/cat '{0}/probe' >&2; exit 0;; esac\nfor last; do :; done\n/bin/cp '{0}/copy.wav' \"$last\"\n",
            dir.path().display()
        );
        std::fs::write(&ffmpeg, script).unwrap();
        std::fs::set_permissions(&ffmpeg, std::fs::Permissions::from_mode(0o755)).unwrap();
        if cfg!(target_os = "macos") {
            std::env::set_var("PATH", dir.path());
        } else {
            std::env::set_var("KUMI_FFMPEG", &ffmpeg);
        }
        let sample = dir.path().join("Sample.mp3");
        std::fs::write(&sample, b"an mp3").unwrap();
        let signal = Signal::new();
        let (mono, rate, length) = read_mono(&sample, &signal, 0., 6.).await.unwrap();
        assert_eq!((mono.len(), rate, length), (48_000, 8_000., None), "6 s read, the file's length not known");
        std::fs::write(dir.path().join("probe"), "  Duration: 00:06:00.00, start: 0.000000, bitrate: 128 kb/s\n").unwrap();
        let (_, _, length) = read_mono(&sample, &signal, 0., 6.).await.unwrap();
        assert_eq!(length, Some(360.), "the length ffmpeg read");
    }

    /// A stretch from a file's start keeps the file's length past it, read by the probe: after the copy, and only when
    /// ffmpeg, converting, didn't say the file ends well before the reach. A length known already (a song's tags say it)
    /// isn't read again, and no process starts for it. A copy ffmpeg couldn't make reads the length before afconvert's,
    /// as it was read before.
    #[cfg(unix)]
    #[tokio::test(flavor = "current_thread")]
    async fn a_files_length_is_read_only_when_the_copy_cant_say_it() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let (copy, ran, said, fail, slow, probing) = (
            dir.path().join("copy.wav"),
            dir.path().join("ran"),
            dir.path().join("said"),
            dir.path().join("fail"),
            dir.path().join("slow"),
            dir.path().join("probing"),
        );
        write_test_wav(&copy, &[0.25; 8_000], 8_000);
        let scripts = [
            (
                "ffprobe",
                format!(
                    "#!/bin/sh\necho probe >> '{}'\n[ -f '{}' ] && {{ : > '{}'; exec /bin/sleep 5; }}\necho 240.5\n",
                    ran.display(),
                    slow.display(),
                    probing.display()
                ),
            ),
            (
                "ffmpeg",
                format!(
                    "#!/bin/sh\necho convert >> '{0}'\n[ -f '{1}' ] && exit 1\n[ -f '{2}' ] && /bin/cat '{2}' >&2\nfor last; do :; done\n[ \"$last\" = - ] || /bin/cp '{3}' \"$last\"\n",
                    ran.display(),
                    fail.display(),
                    said.display(),
                    copy.display()
                ),
            ),
            ("afconvert", format!("#!/bin/sh\necho afconvert >> '{}'\nfor last; do :; done\n/bin/cp '{}' \"$last\"\n", ran.display(), copy.display())),
        ];
        for (name, script) in scripts {
            std::fs::write(dir.path().join(name), script).unwrap();
            std::fs::set_permissions(dir.path().join(name), std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        if cfg!(target_os = "macos") {
            std::env::set_var("PATH", dir.path());
        } else {
            std::env::set_var("KUMI_FFMPEG", dir.path().join("ffmpeg"));
        }
        let song = dir.path().join("Song.mp3");
        std::fs::write(&song, b"an mp3").unwrap();
        let runs = || {
            let runs = std::fs::read_to_string(&ran).unwrap_or_default().lines().map(str::to_owned).collect::<Vec<_>>();
            let _ = std::fs::remove_file(&ran);
            runs
        };
        let say = |duration: &str| {
            let line = format!("Input #0, mp3, from 'Song.mp3':\n  Metadata:\n    title           : Song\n  Duration: {duration}, start: 0.025057, bitrate: 128 kb/s\n");
            std::fs::write(&said, line).unwrap();
        };
        // ffmpeg says nothing of its length: read after the copy.
        let read = prepare_audio_range(&song, None, 0., 30.).await.unwrap();
        assert_eq!((runs(), read.seconds, read.length_known), (vec!["convert".to_string(), "probe".to_string()], Some(240.5), true));
        // It says the file is longer than the reach: read, and the probe's reading kept (as before, not ffmpeg's).
        say("00:04:00.48");
        let read = prepare_audio_range(&song, None, 0., 30.).await.unwrap();
        assert_eq!((runs(), read.seconds, read.length_known), (vec!["convert".to_string(), "probe".to_string()], Some(240.5), true));
        // It says it ends within a second of the reach: read, to say what the probe says.
        say("00:00:29.50");
        let read = prepare_audio_range(&song, None, 0., 30.).await.unwrap();
        assert_eq!((runs(), read.seconds), (vec!["convert".to_string(), "probe".to_string()], Some(240.5)));
        // It says it ends well before: the copy is the whole file, its length known, and no probe.
        say("00:00:05.02");
        let read = prepare_audio_range(&song, None, 0., 30.).await.unwrap();
        assert_eq!((runs(), read.seconds, read.length_known), (vec!["convert".to_string()], None, true));
        let mut source = open_audio_to(&song, None, Some(30.)).await.unwrap();
        assert_eq!((runs(), source.length()), (vec!["convert".to_string()], Some(1.)), "the copy's length");
        assert!(source.read(8_000).await.unwrap().is_some());
        // A length known: no probe, whatever ffmpeg says.
        std::fs::remove_file(&said).unwrap();
        let known = prepare_audio_range_of(&song, None, 0., 30., Some(240.)).await.unwrap();
        assert_eq!((runs(), known.seconds), (vec!["convert".to_string()], Some(240.)), "the length known: no probe");
        // Stopped while its length is read after the copy: stopped, as when it was read first, not a length unknown.
        say("00:04:00.48");
        std::fs::write(&slow, "").unwrap();
        let signal = Signal::new();
        // Stopped once the probe is under way (the fake one says so as it begins), not at a time a slow runner can miss.
        let stopping = {
            let (signal, probing) = (signal.clone(), probing.clone());
            tokio::spawn(async move {
                let began = std::time::Instant::now();
                // (Inside the ci profile's 30 s, so a probe that never began says so before nextest stops the test.)
                while !probing.exists() && began.elapsed() < std::time::Duration::from_secs(20) {
                    tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                }
                signal.cancel();
            })
        };
        let stopped = prepare_audio_range(&song, Some(signal), 0., 30.).await;
        stopping.await.unwrap();
        assert!(probing.exists(), "the probe never began");
        assert!(stopped.is_err(), "stopped, not read");
        // (ffmpeg's own reading, tried when the probe stops, may start before it's stopped too.)
        assert_eq!(runs()[..2], ["convert", "probe"]);
        std::fs::remove_file(&slow).unwrap();
        std::fs::remove_file(&said).unwrap();
        // ffmpeg can't convert it: on a Mac afconvert copies it whole, its length read first as before.
        std::fs::write(&fail, "").unwrap();
        let tried = prepare_audio_range(&song, None, 0., 30.).await;
        if cfg!(target_os = "macos") {
            let tried = tried.unwrap();
            assert_eq!((runs(), tried.whole), (vec!["convert".to_string(), "probe".to_string(), "afconvert".to_string()], true));
        } else {
            assert!(tried.is_err());
            assert_eq!(runs(), ["convert"], "no probe for a copy that failed");
        }
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
