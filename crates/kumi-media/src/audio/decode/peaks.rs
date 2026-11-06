//! Optional pictures from local compressed audio, with an installed decoder only. PCM travels through a pipe in
//! blocks, never through a disk copy or a buffer the size of a song. No resampling or channel mixing loses peaks.

use std::path::Path;
use std::process::Stdio;

use kumi_common::abort::{self, Signal, SignalExt};
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::process::Command;

use super::{extension, local_decoder_path, AudioError, FfmpegOptions, LOCAL_AUDIO_INPUT, MAX_CHANNELS};

const WAIT_MS: u64 = 20_000;
const HEADER_BYTES: usize = 65_536;
const PCM_BYTES: u64 = 512 * 1024 * 1024;
const MOST_COLUMNS: usize = 4096;

/// A local file's peaks without a disk conversion. WAV and AIFF use the in-process reader; other self-contained
/// audio formats use an ffmpeg already installed. Absence, cancellation, a stalled decoder or a stream past the
/// byte budget fails closed: a prefix is never returned as the whole waveform. Discovery and decoding share a
/// twenty-second cancellation deadline (installed-program discovery also makes synchronous filesystem looks).
pub async fn local_peaks(path: impl AsRef<Path>, columns: usize, signal: &Signal) -> Result<(f64, Vec<f32>), AudioError> {
    let path = path.as_ref();
    let signal = abort::any([signal.clone(), abort::timeout(WAIT_MS)]);
    signal.check()?;
    let pcm = [".wav", ".wave", ".aif", ".aiff", ".aifc"].contains(&extension(path).as_str());
    if !super::AUDIO_EXTENSIONS.contains(&extension(path).as_str()) {
        return Err(AudioError("That file is not a supported audio container.".into()));
    }
    let local = |path: &Path| path.to_str().is_some_and(|name| local_decoder_path(path, name) && !kumi_common::path::reserved_device(name));
    if !local(path) {
        return Err(AudioError("A waveform needs a local audio file.".into()));
    }
    let path = tokio::select! {
        biased;
        () = signal.cancelled() => return Err(AudioError("Stopped.".into())),
        path = tokio::fs::canonicalize(path) => path?,
    };
    if !local(&path) {
        return Err(AudioError("A waveform needs a regular local audio file.".into()));
    }
    let metadata = tokio::select! {
        biased;
        () = signal.cancelled() => return Err(AudioError("Stopped.".into())),
        metadata = tokio::fs::metadata(&path) => metadata?,
    };
    if !metadata.is_file() {
        return Err(AudioError("A waveform needs a regular local audio file.".into()));
    }
    if pcm {
        return tokio::select! {
            biased;
            () = signal.cancelled() => Err(AudioError("Stopped.".into())),
            peaks = super::peaks(&path, columns, &signal) => peaks,
        };
    }
    let ffmpeg = super::find_ffmpeg(FfmpegOptions { installed_only: true, signal: Some(signal.clone()), ..Default::default() })
        .await?
        .ok_or_else(|| AudioError("No installed audio decoder.".into()))?;
    signal.check()?;
    decoded_peaks(&ffmpeg, &path, columns, &signal, PCM_BYTES).await
}

async fn decoded_peaks(ffmpeg: &str, path: &Path, columns: usize, signal: &Signal, most: u64) -> Result<(f64, Vec<f32>), AudioError> {
    signal.check()?;
    let input = format!("file:{}", path.to_string_lossy());
    let mut command = Command::new(ffmpeg);
    command
        .args(["-v", "error", "-xerror", "-nostdin", "-threads", "1", "-probesize", "65536", "-analyzeduration", "1000000"])
        .args(LOCAL_AUDIO_INPUT)
        .args([
            "-i",
            &input,
            "-map",
            "0:a:0",
            "-vn",
            "-sn",
            "-dn",
            "-map_metadata",
            "-1",
            "-map_chapters",
            "-1",
            "-c:a",
            "pcm_f32le",
            "-f",
            "wav",
            "-rf64",
            "never",
            "pipe:1",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    #[cfg(windows)]
    command.creation_flags(0x0800_0000);
    let mut child = command.spawn()?;
    let Some(mut stdout) = child.stdout.take() else {
        let _ = child.start_kill();
        let _ = child.wait().await;
        return Err(AudioError("The decoder has no audio pipe.".into()));
    };
    let result = {
        let work = async {
            let peaks = streamed_peaks(&mut stdout, columns, most, signal).await?;
            let status = child.wait().await?;
            if !status.success() {
                return Err(AudioError("The audio decoder could not read the whole file.".into()));
            }
            Ok(peaks)
        };
        tokio::pin!(work);
        tokio::select! {
            biased;
            () = signal.cancelled() => Err(AudioError("Stopped.".into())),
            result = &mut work => result,
        }
    };
    if result.is_err() {
        let _ = child.start_kill();
        let _ = child.wait().await;
    }
    result
}

/// ffmpeg's float WAV on a pipe has an unknown data length. Read only its small header, then count the actual frames.
async fn header(reader: &mut (impl AsyncRead + Unpin)) -> Result<(usize, u32, Option<u64>), AudioError> {
    let bad = || AudioError("The decoder did not return a supported float WAV stream.".into());
    let mut start = [0; 12];
    reader.read_exact(&mut start).await?;
    if &start[..4] != b"RIFF" || &start[8..] != b"WAVE" {
        return Err(bad());
    }
    let mut format = None;
    let mut used = 12;
    loop {
        let mut head = [0; 8];
        reader.read_exact(&mut head).await?;
        used += head.len();
        if used > HEADER_BYTES {
            return Err(bad());
        }
        let length = u32::from_le_bytes(head[4..].try_into().unwrap());
        if &head[..4] == b"data" {
            let (channels, rate) = format.ok_or_else(bad)?;
            return Ok((channels, rate, (length != u32::MAX).then_some(u64::from(length))));
        }
        let padded = length as usize + length as usize % 2;
        if padded > HEADER_BYTES - used {
            return Err(bad());
        }
        let mut body = vec![0; padded];
        reader.read_exact(&mut body).await?;
        used += padded;
        if &head[..4] == b"fmt " {
            if length < 16 {
                return Err(bad());
            }
            let word = |at| u16::from_le_bytes(body[at..at + 2].try_into().unwrap());
            let code = if word(0) == 0xfffe && body.len() >= 40 { word(24) } else { word(0) };
            let channels = word(2) as usize;
            let rate = u32::from_le_bytes(body[4..8].try_into().unwrap());
            if code != 3
                || word(14) != 32
                || channels == 0
                || channels > MAX_CHANNELS
                || word(12) as usize != channels * 4
                || rate == 0
                || rate > 384_000
            {
                return Err(bad());
            }
            format = Some((channels, rate));
        }
    }
}

/// At most twice the requested columns: on filling them, neighbouring peaks meet. The last, incomplete bucket keeps
/// its actual extent; at the end each bucket marks every display column it overlaps, never losing a transient.
/// This can broaden an edge by one display column, but never invents a shorter file or cancels opposing channels.
struct Envelope {
    columns: usize,
    stride: u64,
    frames: u64,
    pending: u64,
    level: f32,
    bins: Vec<f32>,
}
impl Envelope {
    fn new(columns: usize) -> Self {
        Self { columns: columns.clamp(1, MOST_COLUMNS), stride: 1, frames: 0, pending: 0, level: 0., bins: Vec::new() }
    }
    fn push(&mut self, level: f32) {
        self.frames += 1;
        self.pending += 1;
        self.level = self.level.max(level);
        if self.pending == self.stride {
            self.bins.push(self.level);
            (self.pending, self.level) = (0, 0.);
            if self.bins.len() == 2 * self.columns {
                for at in 0..self.columns {
                    self.bins[at] = self.bins[2 * at].max(self.bins[2 * at + 1]);
                }
                self.bins.truncate(self.columns);
                self.stride *= 2;
            }
        }
    }
    fn finish(mut self, rate: u32) -> Result<(f64, Vec<f32>), AudioError> {
        if self.frames == 0 {
            return Err(AudioError("The decoder returned no samples.".into()));
        }
        if self.pending > 0 {
            self.bins.push(self.level);
        }
        let columns = self.columns.min(self.frames as usize);
        let mut peaks = vec![0f32; columns];
        for (at, level) in self.bins.into_iter().enumerate() {
            let (start, end) = (at as u64 * self.stride, ((at as u64 + 1) * self.stride).min(self.frames));
            for column in start * columns as u64 / self.frames..=(end - 1) * columns as u64 / self.frames {
                peaks[column as usize] = peaks[column as usize].max(level);
            }
        }
        let loudest = peaks.iter().copied().fold(0f32, f32::max);
        if loudest > 0. {
            peaks.iter_mut().for_each(|peak| *peak = (*peak / loudest).min(1.));
        }
        Ok((self.frames as f64 / f64::from(rate), peaks))
    }
}

async fn streamed_peaks(
    reader: &mut (impl AsyncRead + Unpin),
    columns: usize,
    most: u64,
    signal: &Signal,
) -> Result<(f64, Vec<f32>), AudioError> {
    let (channels, rate, expected) = header(reader).await?;
    let frame_bytes = channels * 4;
    let mut buffer = vec![0; 32_768 + frame_bytes];
    let (mut pending, mut read) = (0, 0u64);
    let mut envelope = Envelope::new(columns);
    loop {
        signal.check()?;
        let got = reader.read(&mut buffer[pending..]).await?;
        if got == 0 {
            break;
        }
        read += got as u64;
        if read > most {
            return Err(AudioError("The waveform's decoded audio exceeds its byte budget.".into()));
        }
        let filled = pending + got;
        let whole = filled / frame_bytes * frame_bytes;
        for frame in buffer[..whole].chunks_exact(frame_bytes) {
            let mut level = 0f32;
            for word in frame.chunks_exact(4) {
                let sample = f32::from_le_bytes(word.try_into().unwrap());
                if !sample.is_finite() {
                    return Err(AudioError("The decoder returned a non-finite sample.".into()));
                }
                level = level.max(sample.abs());
            }
            envelope.push(level);
        }
        buffer.copy_within(whole..filled, 0);
        pending = filled - whole;
    }
    if pending != 0 || expected.is_some_and(|expected| expected != read) {
        return Err(AudioError("The audio decoder returned an incomplete stream.".into()));
    }
    envelope.finish(rate)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::AsyncWriteExt;

    #[tokio::test]
    async fn streamed_peaks_keep_opposite_phase_channels_and_split_frames_with_bounded_memory() {
        // Opposite phase must not become silence, and a loud right-only sample must not disappear either.
        let left = vec![0.1, -0.1, 0.5, -0.5, 0., 0., 0., 0.];
        let right = vec![-0.1, 0.1, -0.5, 0.5, 0., 0., 1., -1.];
        let wav = super::super::float_wav(&[left, right], 8);
        let (mut writer, mut reader) = tokio::io::duplex(3);
        let writing = tokio::spawn(async move {
            for part in wav.chunks(3) {
                writer.write_all(part).await.unwrap();
            }
        });
        let (seconds, peaks) = streamed_peaks(&mut reader, 4, 1024, &Signal::new()).await.unwrap();
        writing.await.unwrap();
        assert_eq!(seconds, 1.);
        assert_eq!(peaks, [0.1, 0.5, 0., 1.]);
        // Unknown-length WAV is the decoder's pipe contract, and the true EOF determines its length.
        let mut wav = super::super::float_wav(&[vec![0.25; 8]], 8);
        let data = wav.windows(4).position(|tag| tag == b"data").unwrap();
        wav[data + 4..data + 8].copy_from_slice(&u32::MAX.to_le_bytes());
        assert_eq!(streamed_peaks(&mut &wav[..], 4, 1024, &Signal::new()).await.unwrap(), (1., vec![1.; 4]));
        assert!(streamed_peaks(&mut &wav[..], 4, 8, &Signal::new()).await.unwrap_err().0.contains("budget"), "no prefix as a whole wave");
        wav.pop();
        assert!(streamed_peaks(&mut &wav[..], 4, 1024, &Signal::new()).await.unwrap_err().0.contains("incomplete"));

        let mut envelope = Envelope::new(240);
        for at in 0..1_000_000 {
            envelope.push(if at == 750_000 { 0.8 } else { 0. });
        }
        assert!(envelope.bins.len() <= 2 * 240, "a long file holds only bounded peaks");
        let (seconds, peaks) = envelope.finish(1000).unwrap();
        assert_eq!((seconds, peaks.len()), (1000., 240));
        assert_eq!(peaks[180], 1., "the transient at three quarters stays there");
        assert!(peaks[..179].iter().chain(&peaks[182..]).all(|peak| *peak == 0.), "pooling stays within one display column");

        // A large decoder header is refused before allocating or reading its claimed size.
        let mut header = b"RIFF\xff\xff\xff\xffWAVEJUNK".to_vec();
        header.extend(u32::MAX.to_le_bytes());
        assert!(streamed_peaks(&mut &header[..], 4, 1024, &Signal::new()).await.is_err());
    }

    #[cfg(unix)]
    fn executable(path: &Path, body: &str) {
        use std::os::unix::fs::PermissionsExt;
        std::fs::write(path, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn compressed_pictures_use_an_installed_decoder_pipe_and_keep_no_audio_copy() {
        let dir = tempfile::tempdir().unwrap();
        let pcm = dir.path().join("stream.wav");
        std::fs::write(&pcm, super::super::float_wav(&[vec![0.5; 1000]], 1000)).unwrap();
        let args = dir.path().join("args");
        let program = dir.path().join("ffmpeg");
        executable(&program, &format!("printf '%s\\n' \"$@\" > '{}'\nexec /bin/cat '{}'", args.display(), pcm.display()));
        std::env::set_var("KUMI_FFMPEG", &program);
        for extension in ["flac", "ogg", "mp3", "m4a"] {
            let file = dir.path().join(format!("sample.{extension}"));
            std::fs::write(&file, b"fixture audio decoded by the local program").unwrap();
            let (seconds, peaks) = local_peaks(&file, 240, &Signal::new()).await.unwrap();
            assert_eq!((seconds, peaks.len()), (1., 240));
            assert!(peaks.iter().all(|peak| *peak == 1.));
            let args = std::fs::read_to_string(&args).unwrap().lines().map(str::to_owned).collect::<Vec<_>>();
            assert_eq!(
                args[args.iter().position(|arg| arg == "-i").unwrap() + 1],
                format!("file:{}", file.canonicalize().unwrap().display())
            );
            assert!(args.windows(2).any(|pair| pair == ["-protocol_whitelist", "file,pipe"]));
            assert_eq!(args.last().unwrap(), "pipe:1");
            assert!(!args.iter().any(|arg| matches!(arg.as_str(), "-ar" | "-ac")), "no downsampling or mono cancellation");
        }
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 7, "only fixtures, the decoder and its arguments; no derivative");
        // A decoder that cannot finish or exceeds its output budget yields no waveform, even after valid samples.
        executable(&program, &format!("/bin/cat '{}'\nexit 1", pcm.display()));
        assert!(local_peaks(dir.path().join("sample.flac"), 240, &Signal::new()).await.is_err());
        assert!(decoded_peaks(program.to_str().unwrap(), &dir.path().join("sample.flac"), 240, &Signal::new(), 16).await.is_err());
        std::env::set_var("KUMI_FFMPEG", dir.path().join("absent-decoder"));
        assert!(local_peaks(dir.path().join("sample.flac"), 240, &Signal::new()).await.unwrap_err().0.contains("No installed"));
        for path in ["//nas/sample.flac", "//nas/sample.wav"] {
            assert!(local_peaks(Path::new(path), 240, &Signal::new()).await.is_err());
        }
        for extension in ["wav", "flac"] {
            let linked = dir.path().join(format!("device.{extension}"));
            std::os::unix::fs::symlink("/dev/null", &linked).unwrap();
            assert!(local_peaks(&linked, 240, &Signal::new()).await.is_err(), "a device behind a file-like path");
        }
    }

    #[cfg(unix)]
    #[tokio::test(flavor = "current_thread")]
    async fn cancellation_and_the_waveform_deadline_kill_and_reap_the_decoder() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("sample.flac");
        std::fs::write(&file, "fixture").unwrap();
        let pid = dir.path().join("pid");
        let program = dir.path().join("ffmpeg");
        executable(&program, &format!("echo $$ > '{}'\nexec /bin/sleep 60", pid.display()));
        std::env::set_var("KUMI_FFMPEG", &program);
        for timed_out in [false, true] {
            let _ = std::fs::remove_file(&pid);
            let signal = Signal::new();
            let mut reading = tokio::spawn({
                let (file, signal) = (file.clone(), signal.clone());
                async move { local_peaks(&file, 240, &signal).await }
            });
            // The child announces it has started; neither stop depends on process-start latency.
            let child: f64 = loop {
                if let Ok(pid) = std::fs::read_to_string(&pid) {
                    if let Ok(pid) = pid.trim().parse() {
                        break pid;
                    }
                }
                tokio::select! {
                    result = &mut reading => panic!("the decoder did not start: {result:?}"),
                    () = tokio::task::yield_now() => {}
                }
            };
            if timed_out {
                tokio::time::pause();
                tokio::time::advance(std::time::Duration::from_millis(WAIT_MS)).await;
            } else {
                signal.cancel();
            }
            assert!(reading.await.unwrap().is_err());
            assert!(!kumi_types::system::alive(child), "the decoder was killed and reaped before returning");
            if timed_out {
                tokio::time::resume();
            }
        }
    }
}
