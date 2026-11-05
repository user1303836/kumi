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
    std::fs::File::create(&oversized).unwrap().set_len(64 * 1024 * 1024 + 1).unwrap();
    let made = root.path().join("made");
    let decoder = root.path().join("ffmpeg");
    // Model a decoder that returns success after disregarding its file-size cap.
    std::fs::write(
        &decoder,
        format!(
            "#!/bin/sh\nfor last; do :; done\nprintf '%s' \"$last\" > '{}'\n/bin/cp '{}' \"$last\"\n",
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
    assert_eq!(std::fs::metadata(oversized).unwrap().len(), 64 * 1024 * 1024 + 1);
}
