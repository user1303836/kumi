//! The listen tool against a reference: heard with the file at once when told how, and from memory when compared again.
use kumi_common::abort::Signal;
use kumi_media::audio::tools::{listening_tools, ListeningOptions};
use kumi_types::contracts::KernelTool;
use serde_json::{json, Value};
use std::{path::Path, rc::Rc, time::Duration};

fn tone(seconds: f64, hz: f64, amplitude: f64) -> Vec<f32> {
    (0..(seconds * 48000.) as usize).map(|i| (amplitude * (2. * std::f64::consts::PI * hz * i as f64 / 48000.).sin()) as f32).collect()
}
/// 16-bit stereo at 48 kHz, the channel in both.
fn wav(path: &Path, channel: &[f32]) {
    let size = (channel.len() * 4) as u32;
    let mut out = Vec::new();
    out.extend(b"RIFF");
    out.extend((size + 36).to_le_bytes());
    out.extend(b"WAVEfmt ");
    out.extend(16u32.to_le_bytes());
    out.extend(1u16.to_le_bytes());
    out.extend(2u16.to_le_bytes());
    out.extend(48000u32.to_le_bytes());
    out.extend((48000u32 * 4).to_le_bytes());
    out.extend(4u16.to_le_bytes());
    out.extend(16u16.to_le_bytes());
    out.extend(b"data");
    out.extend(size.to_le_bytes());
    for value in channel {
        let sample = ((*value as f64).clamp(-1., 1.) * 32767.).round() as i16;
        out.extend(sample.to_le_bytes());
        out.extend(sample.to_le_bytes());
    }
    std::fs::write(path, out).unwrap();
}
async fn heard(tool: &Rc<dyn KernelTool>, input: Value) -> String {
    let result = tool.execute(input.as_object().unwrap().clone(), Signal::new()).await.unwrap();
    assert!(!result.is_error, "{}", result.text);
    result.text
}

#[tokio::test]
async fn a_reference_is_heard_with_the_file_when_told_how_and_again_only_once_its_file_changes() {
    let root = tempfile::tempdir().unwrap();
    let (mine, reference) = (root.path().join("mine.wav"), root.path().join("reference.wav"));
    wav(&mine, &tone(3., 220., 0.4));
    wav(&reference, &tone(3., 440., 0.2));
    let told = json!({"file":mine,"compare_to":reference,"focus":"sound"});
    // Told how to hear them, the two are heard at once; not told, the file's length chooses (3 s: a sound) and the file
    // is heard first. The same answer either way.
    let tool = listening_tools(ListeningOptions::default()).remove(0);
    let first = heard(&tool, told.clone()).await;
    let chosen = heard(&listening_tools(ListeningOptions::default()).remove(0), json!({"file":mine,"compare_to":reference})).await;
    assert_eq!(chosen, first);
    assert_eq!(heard(&tool, told.clone()).await, first);
    // While its file is as it was (its size and time), the reference isn't heard again: other sound written into it at
    // that size and time isn't heard.
    let modified = std::fs::metadata(&reference).unwrap().modified().unwrap();
    wav(&reference, &tone(3., 880., 0.5));
    let file = std::fs::File::options().write(true).open(&reference).unwrap();
    file.set_modified(modified).unwrap();
    drop(file);
    assert_eq!(heard(&tool, told.clone()).await, first);
    // Saved again, it is.
    std::fs::File::options().write(true).open(&reference).unwrap().set_modified(modified + Duration::from_secs(2)).unwrap();
    let changed = heard(&tool, told.clone()).await;
    assert_ne!(changed, first);
    assert_eq!(changed, heard(&listening_tools(ListeningOptions::default()).remove(0), told).await);
}

/// Mono 16-bit AIFF at 48 kHz, using the same local samples as WAV tests.
fn aiff(path: &Path, channel: &[f32]) {
    let size = (channel.len() * 2) as u32;
    let mut out = Vec::new();
    out.extend(b"FORM");
    out.extend((46 + size).to_be_bytes());
    out.extend(b"AIFFCOMM");
    out.extend(18u32.to_be_bytes());
    out.extend(1u16.to_be_bytes());
    out.extend((channel.len() as u32).to_be_bytes());
    out.extend(16u16.to_be_bytes());
    out.extend([0x40, 0x0e, 0xbb, 0x80, 0, 0, 0, 0, 0, 0]);
    out.extend(b"SSND");
    out.extend((8 + size).to_be_bytes());
    out.extend(0u32.to_be_bytes());
    out.extend(0u32.to_be_bytes());
    for value in channel {
        let sample = ((*value as f64).clamp(-1., 1.) * 32767.).round() as i16;
        out.extend(sample.to_be_bytes());
    }
    std::fs::write(path, out).unwrap();
}

#[tokio::test]
async fn installed_only_analysis_and_workers_read_wav_and_aiff_without_a_decoder() {
    use kumi_media::audio::{analyze_file, hear, worker, AnalyzeOptions, DecodeOptions};

    let root = tempfile::tempdir().unwrap();
    let samples = tone(0.1, 440., 0.25);
    let wave = root.path().join("tone.wav");
    let aif = root.path().join("tone.aiff");
    wav(&wave, &samples);
    aiff(&aif, &samples);
    for path in [&wave, &aif] {
        let name = path.to_str().unwrap();
        let expected = analyze_file(name, AnalyzeOptions::default()).await.unwrap();
        for ffmpeg in [None, Some(root.path().join("missing-decoder"))] {
            let options = AnalyzeOptions { decode: DecodeOptions { installed_only: true, ffmpeg }, ..Default::default() };
            assert_eq!(analyze_file(name, options.clone()).await.unwrap(), expected);
            assert_eq!(hear(name, options.clone()).await.unwrap(), expected);
            assert_eq!(worker::in_worker(name.into(), options, name.into(), None, None).await.unwrap(), expected);
        }
    }
}

#[tokio::test]
async fn installed_only_compressed_audio_requires_an_approved_absolute_decoder() {
    use kumi_media::audio::{analyze_file, hear, worker, AnalyzeOptions, DecodeOptions};

    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("tone.mp3");
    std::fs::write(&path, b"not a compressed audio stream").unwrap();
    let name = path.to_str().unwrap();
    for ffmpeg in [
        None,
        Some(root.path().join("missing-decoder")),
        Some("ffmpeg".into()),
        Some(root.path().to_path_buf()),
        Some("//server/share/ffmpeg".into()),
        Some(r"\\server\share\ffmpeg.exe".into()),
        Some(r"\\?\UNC\server\share\ffmpeg.exe".into()),
        Some(r"\\.\C:\tools\ffmpeg.exe".into()),
        Some(root.path().join("decoder\n")),
    ] {
        let options = AnalyzeOptions { decode: DecodeOptions { installed_only: true, ffmpeg }, ..Default::default() };
        let direct = analyze_file(name, options.clone()).await.unwrap_err();
        let heard = hear(name, options.clone()).await.unwrap_err();
        let worker = worker::in_worker(name.into(), options, name.into(), None, None).await.unwrap_err();
        for error in [direct, heard, worker] {
            assert!(error.0.starts_with("The approved ffmpeg dependency is unavailable"), "{error}");
        }
    }
}

#[cfg(unix)]
#[tokio::test]
async fn an_approved_decoder_failure_never_discovers_a_sibling_probe_or_platform_converter() {
    use kumi_media::audio::{hear, AnalyzeOptions, DecodeOptions};
    use std::os::unix::fs::PermissionsExt;

    let root = tempfile::tempdir().unwrap();
    let decoder = root.path().join("ffmpeg");
    std::fs::write(&decoder, "#!/bin/sh\nexit 1\n").unwrap();
    std::fs::set_permissions(&decoder, std::fs::Permissions::from_mode(0o700)).unwrap();
    let probe = root.path().join("ffprobe");
    let marker = root.path().join("probe-ran");
    std::fs::write(&probe, format!("#!/bin/sh\n: > '{}'\nexit 1\n", marker.display())).unwrap();
    std::fs::set_permissions(&probe, std::fs::Permissions::from_mode(0o700)).unwrap();
    // Valid PCM behind a conversion extension: an undeclared converter must not rescue it.
    let file = root.path().join("tone.mp3");
    wav(&file, &tone(0.1, 440., 0.25));
    let error = hear(
        file.to_str().unwrap(),
        AnalyzeOptions {
            seconds: Some(1300.),
            decode: DecodeOptions { installed_only: true, ffmpeg: Some(decoder) },
            ..Default::default()
        },
    )
    .await
    .unwrap_err();
    assert!(error.0.starts_with("The approved ffmpeg dependency couldn't decode"), "{error}");
    assert!(!marker.exists(), "an undeclared sibling ffprobe ran");
}

#[cfg(unix)]
#[tokio::test]
async fn installed_only_playlists_are_refused_before_starting_a_decoder() {
    use kumi_media::audio::{hear, AnalyzeOptions, DecodeOptions};
    use std::os::unix::fs::PermissionsExt;

    let root = tempfile::tempdir().unwrap();
    let marker = root.path().join("decoder-ran");
    let decoder = root.path().join("ffmpeg");
    std::fs::write(&decoder, format!("#!/bin/sh\nprintf ran > '{}'\nexit 1\n", marker.display())).unwrap();
    std::fs::set_permissions(&decoder, std::fs::Permissions::from_mode(0o700)).unwrap();
    let outside = root.path().join("outside.wav");
    std::fs::write(&outside, b"user-owned").unwrap();
    let hls = format!(
        "#EXTM3U\n#EXT-X-TARGETDURATION:1\n#EXTINF:1,\nhttp://127.0.0.1:9/audio.ts\n#EXTINF:1,\n{}\n#EXT-X-ENDLIST\n",
        outside.display()
    );
    let concat = format!("ffconcat version 1.0\nfile '{}'\nfile 'http://127.0.0.1:9/audio.wav'\n", outside.display());
    for (name, contents) in [("playlist.m3u8", hls), ("playlist.ffconcat", concat)] {
        let path = root.path().join(name);
        std::fs::write(&path, contents).unwrap();
        let result = hear(
            path.to_str().unwrap(),
            AnalyzeOptions { decode: DecodeOptions { installed_only: true, ffmpeg: Some(decoder.clone()) }, ..Default::default() },
        )
        .await;
        assert!(result.is_err());
        assert!(!marker.exists(), "a playlist must not get as far as the external program");
    }
    assert_eq!(std::fs::read(outside).unwrap(), b"user-owned");
}

#[cfg(unix)]
#[tokio::test]
async fn installed_only_oversized_decoder_output_is_refused_and_removed() {
    use kumi_media::audio::{decode::prepare_audio_to_with_options, DecodeOptions};
    use std::os::unix::fs::PermissionsExt;

    let root = tempfile::tempdir().unwrap();
    let oversized = root.path().join("oversized.wav");
    std::fs::File::create(&oversized).unwrap().set_len(272 * 1024 * 1024 + 1).unwrap();
    let made = root.path().join("made");
    let decoder = root.path().join("ffmpeg");
    // Model a decoder that returns success after disregarding its file-size cap.
    std::fs::write(
        &decoder,
        format!(
            "#!/bin/sh\nfor last; do :; done\nprintf '%s' \"$last\" > '{}'\n/bin/ln '{}' \"$last\"\n",
            made.display(),
            oversized.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&decoder, std::fs::Permissions::from_mode(0o700)).unwrap();
    let source = root.path().join("sample.mp3");
    std::fs::write(&source, b"ID3").unwrap();
    let result =
        prepare_audio_to_with_options(&source, None, Some(1.0), &DecodeOptions { installed_only: true, ffmpeg: Some(decoder) }).await;
    assert!(result.is_err(), "an oversized successful process result must not become usable audio");
    let converted = std::path::PathBuf::from(std::fs::read_to_string(made).unwrap());
    assert!(!converted.parent().unwrap().exists(), "the entire owned decoder scratch is removed");
    assert_eq!(std::fs::read(source).unwrap(), b"ID3");
    assert_eq!(std::fs::metadata(oversized).unwrap().len(), 272 * 1024 * 1024 + 1);
}

#[cfg(unix)]
fn decoder_script(path: &Path, body: &str) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::write(path, format!("#!/bin/sh\nset -eu\n{body}\n")).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).unwrap();
}

/// Inspect individual security boundaries, not an argv snapshot or its ordering
/// beyond ffmpeg's requirement that input options precede the input.
#[cfg(unix)]
fn decoder_input_fence(args: &[&str], source: &Path) {
    let input = args.iter().position(|arg| *arg == "-i").expect("an input");
    assert_eq!(args[input + 1], format!("file:{}", source.canonicalize().unwrap().display()));
    for (key, allowed) in [
        ("-protocol_whitelist", &["file", "pipe"][..]),
        ("-format_whitelist", &["wav", "aiff", "mp3", "flac", "aac", "mov", "ogg", "caf"][..]),
    ] {
        let at = args.iter().position(|arg| *arg == key).expect("an explicit input fence");
        assert!(at + 1 < input, "{key} must constrain this input");
        let selected: Vec<_> = args[at + 1].split(',').collect();
        assert!(!selected.is_empty());
        assert!(selected.iter().all(|value| allowed.contains(value)), "unexpected {key}: {selected:?}");
        assert!(selected.contains(&if key == "-protocol_whitelist" { "file" } else { "mp3" }));
    }
}

#[cfg(unix)]
#[tokio::test]
async fn installed_only_disguised_playlists_reach_only_a_fenced_decoder() {
    use kumi_media::audio::{hear, AnalyzeOptions, DecodeOptions};

    let root = tempfile::tempdir().unwrap();
    let decoder = root.path().join("approved");
    let log = root.path().join("args");
    decoder_script(&decoder, &format!("printf '%s\\n' \"$@\" > '{}'\nexit 1", log.display()));
    for contents in [
        "#EXTM3U\n#EXT-X-TARGETDURATION:1\n#EXTINF:1,\nhttp://127.0.0.1:9/segment.ts\n#EXT-X-ENDLIST\n",
        "ffconcat version 1.0\nfile 'http://127.0.0.1:9/segment.wav'\n",
    ] {
        let source = root.path().join("disguised.mp3");
        std::fs::write(&source, contents).unwrap();
        let error = hear(
            source.to_str().unwrap(),
            AnalyzeOptions { decode: DecodeOptions { installed_only: true, ffmpeg: Some(decoder.clone()) }, ..Default::default() },
        )
        .await
        .unwrap_err();
        assert!(error.0.starts_with("The approved ffmpeg dependency couldn't decode"), "{error}");
        let recorded = std::fs::read_to_string(&log).unwrap();
        let args: Vec<_> = recorded.lines().collect();
        decoder_input_fence(&args, &source);
        // The fixture does not emulate ffmpeg demuxers. The real-ffmpeg smoke
        // verifies rejection/no HTTP requests; here the actual spawn must carry
        // a whitelist excluding both hls and concat even behind an audio extension.
        assert!(!Path::new(args.last().unwrap()).parent().unwrap().exists());
    }
}

#[cfg(unix)]
#[tokio::test]
async fn installed_only_twelve_minute_float_output_fits_and_reports_only_measured_audio() {
    use kumi_media::audio::{analyze_file, AnalyzeOptions, DecodeOptions};
    use std::io::{Seek, SeekFrom, Write};

    let root = tempfile::tempdir().unwrap();
    let decoded = root.path().join("decoded.wav");
    // Sparse f32 stereo at 48 kHz: the usual twelve minutes plus read margin.
    // This exceeds the former 64 MiB cap without allocating a song in the test.
    let bytes = 721u32 * 48_000 * 8;
    let mut fixture = std::fs::File::create(&decoded).unwrap();
    fixture.write_all(b"RIFF").unwrap();
    fixture.write_all(&(bytes + 36).to_le_bytes()).unwrap();
    fixture.write_all(b"WAVEfmt ").unwrap();
    fixture.write_all(&16u32.to_le_bytes()).unwrap();
    fixture.write_all(&3u16.to_le_bytes()).unwrap();
    fixture.write_all(&2u16.to_le_bytes()).unwrap();
    fixture.write_all(&48_000u32.to_le_bytes()).unwrap();
    fixture.write_all(&(48_000u32 * 8).to_le_bytes()).unwrap();
    fixture.write_all(&8u16.to_le_bytes()).unwrap();
    fixture.write_all(&32u16.to_le_bytes()).unwrap();
    fixture.write_all(b"data").unwrap();
    fixture.write_all(&bytes.to_le_bytes()).unwrap();
    fixture.set_len(44 + bytes as u64).unwrap();
    fixture.seek(SeekFrom::End(-8)).unwrap();
    fixture.write_all(&1.25f32.to_le_bytes()).unwrap();
    fixture.write_all(&(-0.5f32).to_le_bytes()).unwrap();
    drop(fixture);
    let decoder = root.path().join("approved");
    let calls = root.path().join("calls");
    decoder_script(
        &decoder,
        &format!(
            "printf '%s\\n' BEGIN \"$@\" >> '{}'\nfor last; do :; done\n[ \"$last\" = - ] || /bin/ln '{}' \"$last\"\nprintf '  Duration: 00:15:00.00, start: 0.0, bitrate: 128 kb/s\\n' >&2",
            calls.display(),
            decoded.display()
        ),
    );
    let source = root.path().join("long.mp3");
    std::fs::write(&source, b"ID3").unwrap();
    let analysis = analyze_file(
        source.to_str().unwrap(),
        AnalyzeOptions {
            start: Some(720.),
            seconds: Some(1300.),
            focus: Some("mix".into()),
            decode: DecodeOptions { installed_only: true, ffmpeg: Some(decoder) },
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert_eq!(analysis.seconds, 900., "file duration is distinct from the measured excerpt");
    assert_eq!((analysis.analyzed.from.as_str(), analysis.analyzed.to.as_str()), ("12:00", "12:01"));
    assert_eq!((analysis.sample_rate, analysis.channels), (48_000., 2));
    assert!(analysis.loudness.sample_peak_dbfs > 1., "floating-point peaks must not be clipped to integer PCM");
    let recorded = std::fs::read_to_string(calls).unwrap();
    let invocations: Vec<_> = recorded.split("BEGIN\n").filter(|call| !call.is_empty()).collect();
    assert_eq!(invocations.len(), 2, "conversion and duration use only the approved decoder");
    for invocation in invocations {
        let args: Vec<_> = invocation.lines().collect();
        decoder_input_fence(&args, &source);
        if args.last() == Some(&"-") {
            continue;
        }
        let value = |flag| args[args.iter().position(|arg| *arg == flag).unwrap() + 1];
        let seconds: f64 = value("-t").parse().unwrap();
        let cap: u64 = value("-fs").parse().unwrap();
        assert!((720.0..=721.0).contains(&seconds));
        assert!(44 + u64::from(bytes) < cap && cap <= 272 * 1024 * 1024);
        assert_eq!(value("-acodec"), "pcm_f32le");
        let filter = value("-af");
        let rates = filter.split("sample_rates=").nth(1).unwrap().split(':').next().unwrap();
        assert!(rates.split('|').all(|rate| rate.parse::<u32>().unwrap() <= 48_000));
        let layouts = filter.split("channel_layouts=").nth(1).unwrap().split(':').next().unwrap();
        assert!(layouts.split('|').all(|layout| ["mono", "stereo"].contains(&layout)));
        assert!(!Path::new(args.last().unwrap()).parent().unwrap().exists(), "successful analysis releases scratch");
    }
}

#[cfg(unix)]
#[tokio::test]
async fn installed_only_cancellation_timeout_and_dropped_future_remove_scratch() {
    use kumi_media::audio::{decode::prepare_audio_to_with_options, DecodeOptions};

    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("sample.mp3");
    std::fs::write(&source, b"ID3").unwrap();
    let decoder = root.path().join("approved");
    let made = root.path().join("made");
    decoder_script(
        &decoder,
        &format!("for last; do :; done\n: > \"$last\"\nprintf '%s' \"$last\" > '{}'\nexec /bin/sleep 300", made.display()),
    );
    for stop in ["cancel", "timeout", "drop"] {
        let signal = Signal::new();
        let options = DecodeOptions { installed_only: true, ffmpeg: Some(decoder.clone()) };
        let task = {
            let source = source.clone();
            let signal = signal.clone();
            tokio::spawn(async move { prepare_audio_to_with_options(source, Some(signal), Some(1.), &options).await })
        };
        tokio::time::timeout(Duration::from_secs(10), async {
            while !std::fs::read_to_string(&made).is_ok_and(|path| !path.is_empty()) {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("decoder starts");
        let converted = std::path::PathBuf::from(std::fs::read_to_string(&made).unwrap());
        match stop {
            "cancel" => signal.cancel(),
            "timeout" => {
                tokio::time::pause();
                tokio::time::advance(Duration::from_secs(121)).await;
            }
            _ => task.abort(),
        }
        let result = task.await;
        if stop == "timeout" {
            tokio::time::resume();
            assert!(result.unwrap().err().unwrap().is_timed_out());
        } else if stop == "cancel" {
            assert!(result.unwrap().is_err());
        } else {
            assert!(matches!(result, Err(error) if error.is_cancelled()));
        }
        assert!(!converted.parent().unwrap().exists(), "{stop} must remove the entire owned scratch");
        assert_eq!(std::fs::read(&source).unwrap(), b"ID3");
        std::fs::remove_file(&made).unwrap();
    }
}

#[cfg(unix)]
#[tokio::test]
async fn installed_only_ignores_discovery_environment_and_scrubs_approved_processes() {
    use kumi_media::audio::{hear, AnalyzeOptions, DecodeOptions};

    const CHILD_ROOT: &str = "KUMI_RESTRICTED_DECODE_TEST_ROOT";
    let Some(root) = std::env::var_os(CHILD_ROOT) else {
        // Confine hostile discovery settings to a child test process: no global
        // environment races with other tests, and no reliance on host decoders.
        let root = tempfile::tempdir().unwrap();
        let marker = root.path().join("unapproved-ran");
        for name in ["ffmpeg", "ffprobe", "afconvert", "whisper-cli", "whisper"] {
            decoder_script(&root.path().join(name), &format!(": > '{}'\nexit 1", marker.display()));
        }
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "listen::installed_only_ignores_discovery_environment_and_scrubs_approved_processes", "--nocapture"])
            .env(CHILD_ROOT, root.path())
            .env("PATH", root.path())
            .env("KUMI_FFMPEG", root.path().join("ffmpeg"))
            .env("KUMI_WHISPER", root.path().join("whisper-cli"))
            .env("KUMI_PRIVATE_DECODER_TEST_SECRET", "not-a-real-secret")
            .env("http_proxy", "http://127.0.0.1:9")
            .env("FFREPORT", "file=must-not-be-written")
            .output()
            .unwrap();
        assert!(output.status.success(), "{}\n{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
        assert!(!marker.exists(), "PATH, sibling, environment, and whisper candidates must never run");
        return;
    };
    let root = std::path::PathBuf::from(root);
    let source = root.join("source.mp3");
    std::fs::write(&source, b"ID3").unwrap();
    let approved = root.join("approved");
    decoder_script(&approved, "exit 1");
    for ffmpeg in [None, Some(approved.clone())] {
        let error = hear(
            source.to_str().unwrap(),
            AnalyzeOptions { transcribe: true, decode: DecodeOptions { installed_only: true, ffmpeg }, ..Default::default() },
        )
        .await
        .unwrap_err();
        assert!(error.0.starts_with("The approved ffmpeg dependency"), "{error}");
    }
    let copy = root.join("copy.wav");
    wav(&copy, &tone(0.1, 440., 0.25));
    let calls = root.join("calls");
    let cwd = root.join("cwd");
    decoder_script(
        &approved,
        &format!(
            "printf '%s\\n' BEGIN \"$@\" >> '{}'\n\
             [ \"${{KUMI_RESTRICTED_DECODE_TEST_ROOT-unset}}\" = unset ]\n\
             [ \"${{KUMI_PRIVATE_DECODER_TEST_SECRET-unset}}\" = unset ]\n\
             [ \"${{KUMI_FFMPEG-unset}}\" = unset ]\n\
             [ \"${{KUMI_WHISPER-unset}}\" = unset ]\n\
             [ \"${{http_proxy-unset}}\" = unset ]\n\
             [ \"${{FFREPORT-unset}}\" = unset ]\n\
             [ \"$LC_ALL\" = C ]\n\
             [ \"$HOME\" = \"$TMPDIR\" ] && [ \"$HOME\" = \"$USERPROFILE\" ]\n\
             [ \"$HOME\" = \"$TMP\" ] && [ \"$HOME\" = \"$TEMP\" ]\n\
             [ \"$HOME\" -ef . ]\n\
             pwd -P >> '{}'\n\
             for last; do :; done\n\
             [ \"$last\" = - ] || /bin/ln '{}' \"$last\"",
            calls.display(),
            cwd.display(),
            copy.display()
        ),
    );
    let measured = hear(
        source.to_str().unwrap(),
        AnalyzeOptions { transcribe: true, decode: DecodeOptions { installed_only: true, ffmpeg: Some(approved) }, ..Default::default() },
    )
    .await
    .unwrap();
    assert_eq!(measured.seconds, 0.1);
    let recorded = std::fs::read_to_string(calls).unwrap();
    let invocations: Vec<_> = recorded.split("BEGIN\n").filter(|call| !call.is_empty()).collect();
    assert_eq!(invocations.len(), 2, "both conversion and duration probe use the isolated approved process");
    for invocation in invocations {
        decoder_input_fence(&invocation.lines().collect::<Vec<_>>(), &source);
    }
    let recorded = std::fs::read_to_string(cwd).unwrap();
    let directories: Vec<_> = recorded.lines().collect();
    assert_eq!(directories.len(), 2);
    assert_eq!(directories[0], directories[1]);
    assert_ne!(Path::new(directories[0]), root.canonicalize().unwrap());
    assert!(!Path::new(directories[0]).exists(), "private cwd/home/temp disappears after analysis");
    assert!(!root.join("unapproved-ran").exists());
}
