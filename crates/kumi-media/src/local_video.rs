//! Bounded, selected-tool-only measurement of an already-approved local video.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use kumi_common::abort::{Signal, SignalExt};

use crate::audio::decode::local_decoder_path;
use crate::programs::{run, RunOptions, VideoFailure};

const HEADER_BYTES: usize = 4096;
const TIMEOUT_MS: u64 = 20_000;
// run's max_buffer is PER PIPE and its reader can overshoot by one 16 KiB read.
// Even the two buffers at the point of refusal must fit inside 1 MiB together.
const PIPE_BYTES: usize = 512 * 1024 - 16 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Container {
    Mov,
    Webm,
}

impl Container {
    fn demuxer(self) -> &'static str {
        match self {
            Self::Mov => "mov",
            Self::Webm => "matroska",
        }
    }

    fn input_name(self) -> &'static str {
        match self {
            Self::Mov => "mov,mp4,m4a,3gp,3g2,mj2",
            Self::Webm => "matroska,webm",
        }
    }
}

/// Measure the container duration of a verified owned stage, never discovering a program.
/// The caller checks the stage's identity before and after this call and supplies its open
/// handle (in particular, do not reopen a Windows NamedTempFile for header inspection).
/// This is the container's reported duration, not a decoded-frame completeness promise.
pub async fn seconds(ffmpeg: &Path, input: &Path, opened: &File, signal: &Signal) -> Result<f64, VideoFailure> {
    signal.check()?;
    // Both lexical guards precede all filesystem access, including the handle's header read.
    let program = local_name(ffmpeg)?;
    local_name(input)?;
    if !ffmpeg.is_file() {
        return Err(VideoFailure::video("The selected ffmpeg is unavailable; select an existing local ffmpeg at setup."));
    }
    let canonical = std::fs::canonicalize(input)?;
    let name = local_name(&canonical)?;
    let container = sniff(input, opened)?;
    let scratch = Scratch::new()?;
    signal.check()?;
    probe(program, &format!("file:{name}"), container, &scratch.0, signal, TIMEOUT_MS).await
}

fn local_name(path: &Path) -> Result<&str, VideoFailure> {
    let name = path.to_str().ok_or_else(|| VideoFailure::video("Video probing requires a UTF-8 local path."))?;
    if !local_decoder_path(path, name) {
        return Err(VideoFailure::video("Video probing requires an explicit local absolute path without control characters."));
    }
    Ok(name)
}

struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Result<Self, VideoFailure> {
        let path = std::env::temp_dir().join(format!("kumi-video-{}", uuid::Uuid::new_v4()));
        local_name(&path)?;
        let mut builder = std::fs::DirBuilder::new();
        #[cfg(unix)]
        std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);
        // Exclusive creation: never adopt a pre-existing directory or symlink.
        builder.create(&path)?;
        let mut scratch = Self(path);
        scratch.0 = std::fs::canonicalize(&scratch.0)?;
        local_name(&scratch.0)?;
        Ok(scratch)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn sniff(input: &Path, opened: &File) -> Result<Container, VideoFailure> {
    let mut header = [0; HEADER_BYTES];
    let mut reader = opened;
    let position = reader.stream_position()?;
    reader.seek(SeekFrom::Start(0))?;
    let result = (|| -> std::io::Result<usize> {
        let mut used = 0;
        while used < header.len() {
            let count = reader.read(&mut header[used..])?;
            if count == 0 {
                break;
            }
            used += count;
        }
        Ok(used)
    })();
    // Restore the caller's cursor on failure too; there is no second file open.
    reader.seek(SeekFrom::Start(position))?;
    let count = result?;
    container(input, &header[..count]).ok_or_else(|| {
        VideoFailure::video("The video extension and header must agree: MP4/M4V, MOV, or WebM (not a playlist or another EBML format).")
    })
}

fn container(input: &Path, header: &[u8]) -> Option<Container> {
    let extension = input.extension()?.to_str()?;
    if extension.eq_ignore_ascii_case("webm") {
        return webm_header(header).then_some(Container::Webm);
    }
    if !["mp4", "m4v", "mov"].iter().any(|ext| extension.eq_ignore_ascii_case(ext)) {
        return None;
    }
    // ISO BMFF and modern QuickTime begin with a complete, bounded ftyp box. Older
    // QuickTime may begin with a moov/mdat/wide atom instead, but is not mislabeled MP4.
    let size = u32::from_be_bytes(header.get(..4)?.try_into().ok()?) as usize;
    let kind = header.get(4..8)?;
    if kind == b"ftyp" {
        if size < 16 || size > header.len() || (size - 16) % 4 != 0 {
            return None;
        }
        let brands = std::iter::once(&header[8..12]).chain(header[16..size].chunks_exact(4));
        let valid = brands.into_iter().any(|brand| {
            if extension.eq_ignore_ascii_case("mov") {
                brand == b"qt  "
            } else {
                matches!(
                    brand,
                    b"isom"
                        | b"iso2"
                        | b"iso3"
                        | b"iso4"
                        | b"iso5"
                        | b"iso6"
                        | b"iso7"
                        | b"iso8"
                        | b"iso9"
                        | b"mp41"
                        | b"mp42"
                        | b"avc1"
                        | b"M4V "
                        | b"M4VH"
                        | b"M4VP"
                        | b"dash"
                )
            }
        });
        return valid.then_some(Container::Mov);
    }
    (extension.eq_ignore_ascii_case("mov") && size >= 8 && matches!(kind, b"moov" | b"mdat" | b"wide")).then_some(Container::Mov)
}

// Read just the finite EBML header and its direct children. Searching for the word
// "webm" anywhere would accept Matroska/playlist content with a planted string.
fn webm_header(bytes: &[u8]) -> bool {
    let mut at = 0;
    if vint(bytes, &mut at, false) != Some(0x1a45dfa3) {
        return false;
    }
    let Some(size) = vint(bytes, &mut at, true).and_then(|n| usize::try_from(n).ok()) else { return false };
    let Some(end) = at.checked_add(size).filter(|end| *end <= bytes.len()) else { return false };
    let mut found = false;
    while at < end {
        let Some(id) = vint(&bytes[..end], &mut at, false) else { return false };
        let Some(size) = vint(&bytes[..end], &mut at, true).and_then(|n| usize::try_from(n).ok()) else { return false };
        let Some(next) = at.checked_add(size).filter(|next| *next <= end) else { return false };
        if id == 0x4282 {
            if found || &bytes[at..next] != b"webm" {
                return false;
            }
            found = true;
        }
        at = next;
    }
    found
}

fn vint(bytes: &[u8], at: &mut usize, size: bool) -> Option<u64> {
    let first = *bytes.get(*at)?;
    let length = first.leading_zeros() as usize + 1;
    if length > if size { 8 } else { 4 } {
        return None;
    }
    let mut value = u64::from(if size { first & (0xffu16 >> length) as u8 } else { first });
    for byte in bytes.get(*at + 1..(*at).checked_add(length)?)? {
        value = (value << 8) | u64::from(*byte);
    }
    *at += length;
    // Unknown-size EBML headers/children are not a bounded format declaration.
    if size && value == (1u64 << (length * 7)) - 1 {
        return None;
    }
    Some(value)
}

async fn probe(
    program: &str,
    input: &str,
    container: Container,
    scratch: &Path,
    signal: &Signal,
    timeout_ms: u64,
) -> Result<f64, VideoFailure> {
    // A required real video stream (not album art), stream copy, zero output packets.
    // The null muxer opens no output URL, so neither a pipe protocol nor a file is needed.
    let args = [
        "-enable_drefs",
        "0",
        "-nostdin",
        "-hide_banner",
        "-loglevel",
        "info",
        "-nostats",
        "-protocol_whitelist",
        "file",
        "-f",
        container.demuxer(),
        "-i",
        input,
        "-map",
        "0:V:0",
        "-map_metadata",
        "-1",
        "-map_chapters",
        "-1",
        "-c",
        "copy",
        "-frames:v",
        "0",
        "-f",
        "null",
        "-",
    ];
    // The MOV-only option is absent, not merely set to zero, for WebM.
    let args = &args[if container == Container::Mov { 0 } else { 2 }..];
    let output = run(
        program,
        args,
        RunOptions {
            signal: Some(signal.clone()),
            timeout_ms: Some(timeout_ms),
            max_buffer: Some(PIPE_BYTES),
            isolated_dir: Some(scratch.to_path_buf()),
        },
    )
    .await?;
    signal.check()?;
    measurement(&output.stdout, &output.stderr, container, input)
}

fn measurement(stdout: &[u8], stderr: &str, container: Container, input: &str) -> Result<f64, VideoFailure> {
    // Do not accept a partial answer, an unexpected binary output, or an ignored external track.
    if !stdout.is_empty() || stderr.contains("Skipped opening external track:") {
        return Err(VideoFailure::video("The video probe did not report a self-contained input."));
    }
    duration(stderr, container, input).ok_or_else(|| VideoFailure::video("The video has no unambiguous positive container duration."))
}

fn duration(stderr: &str, container: Container, input: &str) -> Option<f64> {
    if !stderr.ends_with('\n') {
        return None;
    }
    let expected = format!("Input #0, {}, from '{input}':", container.input_name());
    let mut inputs = 0;
    let mut durations = 0;
    let mut measured = None;
    for line in stderr.lines() {
        // CRLF is removed by lines(); other terminal controls cannot rewrite a header.
        if line.chars().any(|c| c.is_control() && c != '\t') {
            return None;
        }
        if line.starts_with("Input #") {
            inputs += 1;
            if line != expected {
                return None;
            }
        }
        if let Some(rest) = line.strip_prefix("  Duration: ") {
            durations += 1;
            if inputs != 1 {
                return None;
            }
            let (time_and_start, bitrate) = rest.split_once(", bitrate: ")?;
            let time = if let Some((time, start)) = time_and_start.split_once(", start: ") {
                if !start.parse::<f64>().ok()?.is_finite() {
                    return None;
                }
                time
            } else {
                time_and_start
            };
            if bitrate.is_empty() {
                return None;
            }
            measured = timestamp(time);
        }
    }
    // Count across the ENTIRE output, never stop at a tag-supplied fake section boundary.
    // av_dump_format always writes the real input duration (including N/A). Its metadata
    // VALUES indent continuation lines, but KEYS can contain literal newlines: such a
    // forged two-space Duration line is an extra candidate, not a replacement for N/A.
    // See FFmpeg libavformat/dump.c: dump_metadata/dump_dictionary and av_dump_format.
    (inputs == 1 && durations == 1).then_some(measured).flatten()
}

fn timestamp(time: &str) -> Option<f64> {
    let mut fields = time.split(':');
    let hours = fields.next()?;
    let minutes = fields.next()?;
    let seconds = fields.next()?;
    if fields.next().is_some() || hours.len() < 2 || minutes.len() != 2 || seconds.len() != 5 {
        return None;
    }
    if !hours.bytes().chain(minutes.bytes()).all(|c| c.is_ascii_digit())
        || seconds.as_bytes()[2] != b'.'
        || !seconds.bytes().enumerate().all(|(i, c)| i == 2 || c.is_ascii_digit())
    {
        return None;
    }
    let hours = hours.parse::<f64>().ok()?;
    let minutes = minutes.parse::<u8>().ok()?;
    let seconds = seconds.parse::<f64>().ok()?;
    let total = hours * 3600. + f64::from(minutes) * 60. + seconds;
    (minutes < 60 && seconds < 60. && total.is_finite() && total > 0.).then_some(total)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    const MP4: &[u8] = b"\0\0\0\x18ftypisom\0\0\0\0isommp42";
    const MOV: &[u8] = b"\0\0\0\x14ftypqt  \0\0\0\0qt  ";
    const WEBM: &[u8] = b"\x1a\x45\xdf\xa3\x87\x42\x82\x84webm";

    #[test]
    fn extensions_and_structured_headers_must_agree() {
        for (name, bytes, expected) in [
            ("a.mp4", MP4, Container::Mov),
            ("a.M4V", MP4, Container::Mov),
            ("a.mov", MOV, Container::Mov),
            ("a.WEBM", WEBM, Container::Webm),
        ] {
            assert_eq!(container(Path::new(name), bytes), Some(expected));
        }
        assert_eq!(container(Path::new("a.webm"), b"\x1a\x45\xdf\xa3\x01\0\0\0\0\0\0\x07\x42\x82\x84webm"), Some(Container::Webm));
        for (name, bytes) in [
            ("a.mov", MP4),
            ("a.mp4", MOV),
            ("a.webm", MP4),
            ("a.mp4", WEBM),
            ("a.mkv", WEBM),
            ("a.webm", b"\x1a\x45\xdf\xa3\x8b\x42\x82\x88matroska".as_slice()),
            ("a.webm", b"\x1a\x45\xdf\xa3\xff\x42\x82\x84webm".as_slice()),
            ("a.webm", b"\x1a\x45\xdf\xa3\x8e\x42\x82\x84webm\x42\x82\x84webm".as_slice()),
        ] {
            assert_eq!(container(Path::new(name), bytes), None, "{name}");
        }
        for bytes in [
            b"#EXTM3U\nhttp://127.0.0.1/secret".as_slice(),
            b"ffconcat version 1.0\nfile '/private/local'",
            b"webm",
            b"\x1a\x45\xdf\xa3\x87\x42\x82\x85webm",
        ] {
            for extension in ["mp4", "m4v", "mov", "webm"] {
                assert_eq!(container(Path::new(&format!("a.{extension}")), bytes), None);
            }
        }
    }

    #[test]
    fn sniff_uses_the_open_handle_and_restores_its_cursor() {
        let mut file = tempfile::NamedTempFile::new().unwrap();
        file.write_all(MP4).unwrap();
        file.seek(SeekFrom::Start(7)).unwrap();
        assert_eq!(sniff(Path::new("not-an-existing-path.mp4"), file.as_file()).unwrap(), Container::Mov);
        assert_eq!(file.stream_position().unwrap(), 7);
    }

    fn report(time: &str) -> String {
        format!("Input #0, mov,mp4,m4a,3gp,3g2,mj2, from 'file:/video.mp4':\n  Metadata:\n    title           : innocent\n  Duration: {time}, start: 0.000000, bitrate: 8 kb/s\n  Stream #0:0: Video: h264\n")
    }

    #[test]
    fn only_one_real_input_duration_not_metadata_or_partial_output() {
        let parse = |text: &str| duration(text, Container::Mov, "file:/video.mp4");
        assert_eq!(parse(&report("01:02:03.45")), Some(3723.45));
        assert_eq!(parse(&report("00:00:02.00").replace(", start: 0.000000", "")), Some(2.));
        for value in ["N/A", "00:00:00.00", "00:60:00.00", "00:00:60.00", "00:00:NaN", "00:00:inf", "-1:00:00.00"] {
            assert_eq!(parse(&report(value)), None);
        }
        let forged = "  Duration: 00:09:00.00, start: 0.000000, bitrate: 8 kb/s\n";
        // Value continuations are indented by ffmpeg and cannot be header candidates.
        let value_tag = report("00:00:02.00")
            .replace("    title           : innocent", &format!("    title           : innocent\n                    : {forged}"));
        assert_eq!(parse(&value_tag), Some(2.));
        // Metadata keys are not escaped by ffmpeg: reject the extra candidate everywhere,
        // even after a forged Output boundary or before a real missing-duration line.
        for actual in ["00:00:02.00", "N/A"] {
            let real = report(actual);
            assert_eq!(parse(&format!("{real}{forged}")), None);
            assert_eq!(parse(&real.replace("  Metadata:\n", &format!("  Metadata:\n    key\n{forged}    rest: value\n"))), None);
            assert_eq!(parse(&real.replace("  Metadata:\n", &format!("Output #0, null, to '-':\n{forged}"))), None);
        }
        assert_eq!(parse(forged), None);
        assert_eq!(parse(&format!("{}{}", report("00:00:02.00"), report("00:00:02.00"))), None);
        assert_eq!(parse(&report("00:00:02.00").replace("innocent", "evil\r  Duration: 00:09:00.00")), None);
        assert_eq!(parse("Input #0, mov,mp4,m4a,3gp,3g2,mj2, from 'file:/video.mp4':\n  Duration: 00:00:02."), None);
    }

    // Real ffmpeg 8.1 / libavformat 62.12.100 input-header excerpts, captured from
    // two-second 32x32 clips, not shell stand-ins. Only the input path is normalized.
    // MP4 tags were muxed with use_metadata_tags; literal newlines in KEYS survive
    // dump_dictionary, whereas those in VALUES acquire a continuation prefix.
    // These exercise our parser without requiring ffmpeg on CI. The separate raw
    // CLI smoke established demuxer/no-fetch behavior; these do not simulate that.
    #[test]
    fn real_mp4_metadata_dump_cannot_replace_the_container_duration() {
        let value = r#"Input #0, mov,mp4,m4a,3gp,3g2,mj2, from 'file:/video.mp4':
  Metadata:
    minor_version   : 512
    major_brand     : isom
    compatible_brands: isomiso2mp41
    title           : innocent
                    :   Duration: 00:09:00.00, start: 0.000000, bitrate: 8 kb/s
    encoder         : Lavf62.12.100
  Duration: 00:00:02.00, start: 0.000000, bitrate: 5 kb/s
  Stream #0:0[0x1](und): Video: mpeg4 (Simple Profile) (mp4v / 0x7634706D), yuv420p, 32x32 [SAR 1:1 DAR 1:1], 0 kb/s, 2 fps, 2 tbr, 16384 tbn (default)
    Metadata:
      handler_name    : VideoHandler
      encoder         : Lavc62.28.100 mpeg4
"#;
        let key = r#"Input #0, mov,mp4,m4a,3gp,3g2,mj2, from 'file:/video.mp4':
  Metadata:
    minor_version   : 512
    major_brand     : isom
    compatible_brands: isomiso2mp41
    innocent
  Duration: 00:09:00.00, start: 0.000000, bitrate: 8 kb/s
rest: value
    encoder         : Lavf62.12.100
  Duration: 00:00:02.00, start: 0.000000, bitrate: 5 kb/s
  Stream #0:0[0x1](und): Video: mpeg4 (Simple Profile) (mp4v / 0x7634706D), yuv420p, 32x32 [SAR 1:1 DAR 1:1], 0 kb/s, 2 fps, 2 tbr, 16384 tbn (default)
    Metadata:
      handler_name    : VideoHandler
      encoder         : Lavc62.28.100 mpeg4
"#;
        let boundary = r#"Input #0, mov,mp4,m4a,3gp,3g2,mj2, from 'file:/video.mp4':
  Metadata:
    minor_version   : 512
    major_brand     : isom
    compatible_brands: isomiso2mp41
    innocent
Output #0, null, to '-':
  Duration: 00:09:00.00, start: 0.000000, bitrate: 8 kb/s
rest: value
    encoder         : Lavf62.12.100
  Duration: 00:00:02.00, start: 0.000000, bitrate: 5 kb/s
  Stream #0:0[0x1](und): Video: mpeg4 (Simple Profile) (mp4v / 0x7634706D), yuv420p, 32x32 [SAR 1:1 DAR 1:1], 0 kb/s, 2 fps, 2 tbr, 16384 tbn (default)
    Metadata:
      handler_name    : VideoHandler
      encoder         : Lavc62.28.100 mpeg4
"#;
        // All three real invocations exited zero with empty stdout. Exit success
        // therefore cannot substitute for rejecting an ambiguous metadata dump.
        assert_eq!(measurement(b"", value, Container::Mov, "file:/video.mp4").unwrap(), 2.);
        for attack in [key, boundary] {
            assert!(measurement(b"", attack, Container::Mov, "file:/video.mp4").is_err());
        }
    }

    #[test]
    fn real_successful_mov_probe_still_refuses_a_skipped_external_track() {
        // A two-track MOV with only the second dref replaced by an alis entry
        // (path :target.fifo, levels 1/1) exits ZERO: the mapped first video works.
        // With enable_drefs=0 the FIFO was untouched; changing it to 1 on the
        // single-track fixture produced an observed FIFO reader. Preserve the
        // diagnostic fence even when process success and duration both look valid.
        let stderr = r#"[in#0 @ 0x7877014000] Skipped opening external track: stream 1, alias: path='/target.fifo', dir='(null)', filename='target.fifo', volume='', nlvl_from=1, nlvl_to=1.Set enable_drefs to allow this.
[in#0/mov,mp4,m4a,3gp,3g2,mj2 @ 0x7877014000] Could not find codec parameters for stream 1 (Video: mpeg4 (Simple Profile) (mp4v / 0x7634706D), none, 0 kb/s): unspecified size
Consider increasing the value for the 'analyzeduration' (0) and 'probesize' (5000000) options
Input #0, mov,mp4,m4a,3gp,3g2,mj2, from 'file:/video.mov':
  Metadata:
    major_brand     : qt  
    minor_version   : 512
    compatible_brands: qt  
    encoder         : Lavf62.12.100
  Duration: 00:00:02.00, start: 0.000000, bitrate: 7 kb/s
  Stream #0:0[0x1]: Video: mpeg4 (Simple Profile) (mp4v / 0x7634706D), yuv420p, 32x32 [SAR 1:1 DAR 1:1], 0 kb/s, 2 fps, 2 tbr, 16384 tbn (default)
    Metadata:
      handler_name    : VideoHandler
      vendor_id       : FFMP
      encoder         : Lavc62.28.100 mpeg4
  Stream #0:1[0x2]: Video: mpeg4 (Simple Profile) (mp4v / 0x7634706D), none, 0 kb/s, 2 fps, 2 tbr, 16384 tbn
    Metadata:
      handler_name    : VideoHandler
      vendor_id       : FFMP
      encoder         : Lavc62.28.100 mpeg4
"#;
        assert_eq!(duration(stderr, Container::Mov, "file:/video.mov"), Some(2.));
        assert!(measurement(b"", stderr, Container::Mov, "file:/video.mov").is_err());
    }

    #[test]
    fn real_webm_linked_chapter_dump_measures_only_the_local_container() {
        // Prev/NextUID, Prev/NextFilename and ChapterSegmentUID were inserted in a
        // real WebM, targeting a FIFO or loopback HTTP. Both raw probes exited 0,
        // reported this header and opened neither target. External links are not
        // measured; the local container duration remains two seconds.
        let stderr = r#"Input #0, matroska,webm, from 'file:/video.webm':
  Metadata:
    ENCODER         : Lavf62.12.100
  Duration: 00:00:02.00, start: 0.000000, bitrate: 2 kb/s
  Chapters:
    Chapter #0:0: start 0.000000, end 2.000000
  Stream #0:0: Video: vp8, yuv420p(tv, progressive), 32x32, SAR 1:1 DAR 1:1, 2 fps, 2 tbr, 1k tbn
    Metadata:
      ENCODER         : Lavc62.28.100 libvpx
      DURATION        : 00:00:02.000000000
"#;
        assert_eq!(measurement(b"", stderr, Container::Webm, "file:/video.webm").unwrap(), 2.);
    }

    #[cfg(unix)]
    fn selected(dir: &Path, body: &str) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let path = dir.join("selected-only");
        std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
        path
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn selected_probe_has_required_fences_and_removes_scratch() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let input = dir.path().join("video.mp4");
        std::fs::write(&input, MP4).unwrap();
        let record = dir.path().join("record");
        let program = selected(dir.path(), &format!(
            "printf '%s\\n' \"$PWD\" \"$HOME\" \"$TMPDIR\" \"${{HTTP_PROXY-unset}}\" \"${{DYLD_INSERT_LIBRARIES-unset}}\" \"${{FFREPORT-unset}}\" \"$LC_ALL\" > '{}'\n[ \"${{KUMI_PRIVATE_VIDEO_TEST_SECRET-unset}}\" = unset ] || exit 93\n[ -t 0 ] && exit 1\nIFS= read -r line && exit 1\nfor arg; do printf '%s\\n' \"$arg\"; done >> '{}'\nprintf \"Input #0, mov,mp4,m4a,3gp,3g2,mj2, from 'file:{}':\\n  Duration: 00:00:02.00, start: 0.000000, bitrate: 8 kb/s\\n\" >&2",
            record.display(), record.display(), input.canonicalize().unwrap().display()
        ));
        // A sibling ffprobe is a tripwire, not a dependency or a fallback.
        let trap = dir.path().join("ffprobe");
        std::fs::write(&trap, "#!/bin/sh\nexit 91\n").unwrap();
        std::fs::set_permissions(&trap, std::fs::Permissions::from_mode(0o700)).unwrap();
        let file = File::open(&input).unwrap();
        let previous = std::env::var_os("KUMI_PRIVATE_VIDEO_TEST_SECRET");
        std::env::set_var("KUMI_PRIVATE_VIDEO_TEST_SECRET", "must-not-reach-the-child");
        let result = seconds(&program, &input, &file, &Signal::new()).await;
        match previous {
            Some(value) => std::env::set_var("KUMI_PRIVATE_VIDEO_TEST_SECRET", value),
            None => std::env::remove_var("KUMI_PRIVATE_VIDEO_TEST_SECRET"),
        }
        assert_eq!(result.unwrap(), 2.);
        let text = std::fs::read_to_string(&record).unwrap();
        let lines: Vec<_> = text.lines().collect();
        let scratch = Path::new(lines[0]);
        assert_eq!(lines[0], lines[1]);
        assert_eq!(lines[0], lines[2]);
        assert_eq!(&lines[3..7], &["unset", "unset", "unset", "C"]);
        assert!(!scratch.exists(), "owned scratch must be removed after success");
        assert_eq!(
            &lines[7..18],
            &[
                "-enable_drefs",
                "0",
                "-nostdin",
                "-hide_banner",
                "-loglevel",
                "info",
                "-nostats",
                "-protocol_whitelist",
                "file",
                "-f",
                "mov"
            ]
        );
        assert_eq!(
            &lines[20..],
            &["-map", "0:V:0", "-map_metadata", "-1", "-map_chapters", "-1", "-c", "copy", "-frames:v", "0", "-f", "null", "-"]
        );
        assert!(seconds(Path::new("ffmpeg"), &input, &file, &Signal::new()).await.is_err());
        assert!(seconds(&dir.path().join("missing-selected"), &input, &file, &Signal::new()).await.is_err());
        let stopped = Signal::new();
        stopped.cancel();
        assert_eq!(seconds(&program, &input, &file, &stopped).await, Err(VideoFailure::Aborted));
        std::fs::write(&input, b"#EXTM3U\nfile:/not-approved").unwrap();
        assert!(seconds(&program, &input, &file, &Signal::new()).await.is_err());
        assert_eq!(std::fs::read_to_string(&record).unwrap(), text, "a disguised playlist must not execute the selected tool");
        std::fs::write(&input, MP4).unwrap();
        let program = selected(dir.path(), &format!("printf '%s' \"$PWD\" > '{}'\nexit 1", record.display()));
        assert!(seconds(&program, &input, &file, &Signal::new()).await.is_err());
        let failed_scratch = std::fs::read_to_string(&record).unwrap();
        assert!(!Path::new(&failed_scratch).exists(), "owned scratch must be removed after process failure");
        let private = Scratch::new().unwrap();
        assert_eq!(std::fs::metadata(&private.0).unwrap().permissions().mode() & 0o777, 0o700);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn webm_has_no_mov_option_and_process_failures_never_measure() {
        let dir = tempfile::tempdir().unwrap();
        let scratch = Scratch::new().unwrap();
        let signal = Signal::new();
        let program = selected(dir.path(), "for arg; do [ \"$arg\" = -enable_drefs ] && exit 92; done\nprintf \"Input #0, matroska,webm, from 'file:/video.webm':\\n  Duration: 00:00:02.00, start: 0.000000, bitrate: 8 kb/s\\n\" >&2");
        assert_eq!(
            probe(program.to_str().unwrap(), "file:/video.webm", Container::Webm, &scratch.0, &signal, TIMEOUT_MS).await.unwrap(),
            2.
        );
        // These stand-ins test our process/result boundary, NOT ffmpeg's media parser.
        for body in [
            "printf 'unexpected output'; exit 0",
            "printf '  Duration: 00:00:02.00, start: 0.0, bitrate: 8 kb/s\\n' >&2; exit 1",
            "printf 'Skipped opening external track: alias\\n' >&2",
            "while :; do printf '%01024d' 0; done",
            "while :; do printf '%01024d' 0 >&2; done",
        ] {
            let program = selected(dir.path(), body);
            let result = probe(program.to_str().unwrap(), "file:/video.webm", Container::Webm, &scratch.0, &signal, TIMEOUT_MS).await;
            assert!(result.is_err());
            assert!(!matches!(result, Err(VideoFailure::TimedOut(_))), "output cap must stop flooding, not the deadline");
        }
        assert_eq!(2 * (PIPE_BYTES + 16 * 1024), 1024 * 1024);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn timeout_and_live_cancellation_refuse_partial_duration() {
        let dir = tempfile::tempdir().unwrap();
        let scratch = Scratch::new().unwrap();
        let began = dir.path().join("began");
        let program =
            selected(dir.path(), &format!("printf \"{}\" >&2\n: > '{}'\nwhile :; do :; done", report("00:00:02.00"), began.display()));
        let signal = Signal::new();
        let result = probe(program.to_str().unwrap(), "file:/video.mp4", Container::Mov, &scratch.0, &signal, 1).await;
        assert!(matches!(result, Err(VideoFailure::TimedOut(_))));
        let _ = std::fs::remove_file(&began);
        let stopping = async {
            tokio::time::timeout(std::time::Duration::from_secs(5), async {
                while !began.exists() {
                    tokio::task::yield_now().await;
                }
            })
            .await
            .expect("the selected child must start before cancellation");
            signal.cancel();
        };
        let (result, ()) =
            tokio::join!(probe(program.to_str().unwrap(), "file:/video.mp4", Container::Mov, &scratch.0, &signal, TIMEOUT_MS), stopping);
        assert_eq!(result, Err(VideoFailure::Aborted));
        let path = scratch.0.clone();
        drop(scratch);
        assert!(!path.exists());
    }
}
