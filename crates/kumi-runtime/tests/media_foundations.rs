//! Native audio formats and transport fixtures for audio, listening and video.
use futures::FutureExt;
use kumi_runtime::{
    audio::decode::open_audio,
    ears::{capture::*, osc::*},
    video::programs::*,
};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    path::Path,
    sync::{Arc, Mutex},
};

fn tone(length: usize, hz: f64, amplitude: f64) -> Vec<f32> {
    (0..length).map(|i| (amplitude * (2.0 * std::f64::consts::PI * hz * i as f64 / 48000.0).sin()) as f32).collect()
}
fn wav(path: &Path, channels: &[Vec<f32>], bits: u16) {
    let bytes = bits / 8;
    let size = (channels[0].len() * channels.len() * bytes as usize) as u32;
    let mut out = Vec::new();
    out.extend(b"RIFF");
    out.extend((size + 36).to_le_bytes());
    out.extend(b"WAVEfmt ");
    out.extend(16u32.to_le_bytes());
    out.extend((if bits == 32 { 3u16 } else { 1u16 }).to_le_bytes());
    out.extend((channels.len() as u16).to_le_bytes());
    out.extend(48000u32.to_le_bytes());
    out.extend((48000 * channels.len() as u32 * bytes as u32).to_le_bytes());
    out.extend((channels.len() as u16 * bytes).to_le_bytes());
    out.extend(bits.to_le_bytes());
    out.extend(b"data");
    out.extend(size.to_le_bytes());
    for frame in 0..channels[0].len() {
        for channel in channels {
            let value = (channel[frame] as f64).clamp(-1.0, 1.0);
            if bits == 32 {
                out.extend((value as f32).to_le_bytes());
            } else if bits == 16 {
                out.extend((kumi_common::js::number::round(value * 32767.0) as i16).to_le_bytes());
            } else {
                out.extend(&(kumi_common::js::number::round(value * 8388607.0) as i32).to_le_bytes()[..3]);
            }
        }
    }
    std::fs::write(path, out).unwrap();
}
fn aiff(path: &Path, channel: &[f32]) {
    let mut body = Vec::new();
    body.extend(b"AIFFCOMM");
    body.extend(18u32.to_be_bytes());
    body.extend(1u16.to_be_bytes());
    body.extend((channel.len() as u32).to_be_bytes());
    body.extend(16u16.to_be_bytes());
    body.extend(16398u16.to_be_bytes());
    body.extend(0xbb800000u32.to_be_bytes());
    body.extend(0u32.to_be_bytes());
    body.extend(b"SSND");
    body.extend((8 + channel.len() as u32 * 2).to_be_bytes());
    body.extend([0u8; 8]);
    for value in channel {
        body.extend((kumi_common::js::number::round(*value as f64 * 32767.0) as i16).to_be_bytes());
    }
    let mut out = Vec::new();
    out.extend(b"FORM");
    out.extend((body.len() as u32).to_be_bytes());
    out.extend(body);
    std::fs::write(path, out).unwrap();
}
#[tokio::test]
async fn wav_16_bit_24_bit_float_and_aiff_decode_to_the_same_samples_other_files_are_refused_plainly() {
    let folder = tempfile::tempdir().unwrap();
    let sine = tone(24000, 440.0, 0.5);
    for (name, bits) in [("d16.wav", 16), ("d24.wav", 24), ("dfloat.wav", 32), ("d16.aiff", 16)] {
        let path = folder.path().join(name);
        if name.ends_with("aiff") {
            aiff(&path, &sine);
        } else {
            wav(&path, &[sine.clone()], bits);
        }
        let mut source = open_audio(path, None).await.unwrap();
        assert_eq!(source.sample_rate, 48000.0, "{name}");
        assert_eq!(source.channels, 1, "{name}");
        assert_eq!(source.frames, sine.len(), "{name}");
        let block = source.read(1000).await.unwrap().unwrap();
        assert!((block[0][100] - sine[100]).abs() < 1e-3, "{name}");
        source.seek(23999.0);
        assert_eq!(source.read(1000).await.unwrap().unwrap()[0].len(), 1);
        assert!(source.read(1000).await.unwrap().is_none());
        source.close().await.unwrap();
    }
    std::fs::write(folder.path().join("notes.txt"), "hello").unwrap();
    assert!(open_audio(folder.path().join("notes.txt"), None).await.err().unwrap().0.contains("isn't an audio format"));
    std::fs::write(folder.path().join("fake.wav"), "not a wav file at all").unwrap();
    assert!(open_audio(folder.path().join("fake.wav"), None).await.err().unwrap().0.contains("doesn't look like WAV or AIFF"));
    assert!(open_audio(folder.path().join("missing.wav"), None).await.err().unwrap().0.contains("no file there"));
}
#[test]
fn osc_messages_go_out_and_come_back_as_maxs_udpsend_and_udpreceive_read_them() {
    let packet = encode_osc("/kumi/ears/arm", &[OscArg::Float(12.0), 47290.into(), "a token".into(), 0.5.into()]);
    assert_eq!(packet.len() % 4, 0);
    assert_eq!(
        decode_osc(&packet),
        Some(OscMessage {
            address: "/kumi/ears/arm".into(),
            args: vec![OscValue::Number(12.0), OscValue::Number(47290.0), OscValue::Text("a token".into()), OscValue::Number(0.5)]
        })
    );
    assert_eq!(
        decode_osc(&encode_osc(
            "/kumi/ears/hello",
            &[47324.into(), 9.into(), 1.into(), 48000.into(), "live_set tracks 3 devices 2".into()]
        ))
        .unwrap()
        .args,
        vec![
            OscValue::Number(47324.0),
            OscValue::Number(9.0),
            OscValue::Number(1.0),
            OscValue::Number(48000.0),
            OscValue::Text("live_set tracks 3 devices 2".into())
        ]
    );
    assert_eq!(decode_osc(&encode_osc("/x", &["".into()])).unwrap().args, vec![OscValue::Text("".into())]);
    assert!(decode_osc(b"not osc").is_none());
}
#[derive(Clone, Copy)]
struct Part {
    frames: usize,
    playing: Option<(f64, f64)>,
    audio: bool,
}
fn capture(parts: &[Part], interleaved: bool, big: bool, unrecorded: usize, invert: bool, lag: Option<usize>) -> Vec<u8> {
    let mut frames: Vec<Vec<f32>> = Vec::new();
    let mut beats = Vec::new();
    for part in parts {
        for frame in 0..part.frames {
            let value = if part.audio { (frame % 100) as f32 / 1000.0 } else { 0.0 };
            let beat = part.playing.map(|(from, per)| from + frame as f64 / per);
            beats.push(beat);
            frames.push(vec![value, if invert { -value } else { value }, beat.map_or(1.0, |v| (1.0 + v % 1.0) as f32)]);
        }
    }
    if let Some(lag) = lag {
        for (frame, channels) in frames.iter_mut().enumerate() {
            channels.push(beats[frame.saturating_sub(lag)].unwrap_or(0.0) as f32);
        }
    }
    let channels = if lag.is_some() { 4 } else { 3 };
    for _ in 0..unrecorded {
        frames.push(vec![0.0; channels]);
    }
    let mut bytes = vec![0u8; frames.len() * channels * 4];
    for (frame, values) in frames.iter().enumerate() {
        for (channel, value) in values.iter().enumerate() {
            let at = 4 * if interleaved { frame * channels + channel } else { channel * frames.len() + frame };
            bytes[at..at + 4].copy_from_slice(&if big { value.to_be_bytes() } else { value.to_le_bytes() });
        }
    }
    bytes
}
#[tokio::test]
async fn a_capture_is_read_in_whichever_layout_max_wrote_it_trimmed_and_placed_on_lives_beats_across_a_jump() {
    let beat = 480;
    let parts = [
        Part { frames: 1000, playing: None, audio: false },
        Part { frames: 600, playing: Some((251.3, beat as f64)), audio: false },
        Part { frames: beat * 8, playing: Some((30.0, beat as f64)), audio: true },
    ];
    for interleaved in [true, false] {
        for big in [false, true] {
            let read = parse_capture(&capture(&parts, interleaved, big, 5000, true, None), 3, 48000.0).unwrap();
            assert_eq!(read.left.len(), 1000 + 600 + beat * 8);
            assert!((read.left[1750] - 0.05).abs() < 1e-6 && (read.right[1750] + 0.05).abs() < 1e-6);
            let stretches = runs(&read, Anchors { first: Some(251.3), after_jump: Some(30.0) });
            assert_eq!(stretches.len(), 2);
            assert!((stretches[0].beat - 251.3).abs() < 1e-3 && (stretches[1].beat - 30.0).abs() < 1e-3);
            assert_eq!(stretches[1].samples_per_beat.round(), beat as f64);
            assert_eq!(frame_at(&stretches[1], 32.0), Some(1600 + beat * 2));
            assert_eq!(frame_at(&stretches[1], 40.0), None);
        }
    }
    let read = parse_capture(
        &capture(&[Part { frames: beat * 4, playing: Some((16.25, beat as f64)), audio: false }], true, false, 0, false, None),
        3,
        48000.0,
    )
    .unwrap();
    assert!((runs(&read, Anchors { first: Some(16.21), ..Default::default() })[0].beat - 16.25).abs() < 1e-3);
    let read = parse_capture(
        &capture(&[Part { frames: beat * 3, playing: Some((4.0, beat as f64)), audio: false }], true, false, 0, false, None),
        3,
        48000.0,
    )
    .unwrap();
    assert_eq!(runs(&read, Anchors { first: Some(4.0), ..Default::default() }).len(), 1);
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("part.wav");
    let whole = parse_capture(&capture(&parts, true, false, 0, false, None), 3, 48000.0).unwrap();
    write_capture_wav(&file, &whole, 1600.0, (1600 + beat * 2) as f64).await.unwrap();
    let mut source = open_audio(file, None).await.unwrap();
    assert_eq!(source.sample_rate, 48000.0);
    assert_eq!(source.channels, 2);
    assert_eq!(source.frames, beat * 2);
    source.close().await.unwrap();
    let raw = dir.path().join("capture.raw");
    std::fs::write(&raw, capture(&parts, true, false, 0, false, None)).unwrap();
    assert_eq!(read_capture(&raw, 3, 48000.0).await.unwrap().left.len(), 1000 + 600 + beat * 8);
}
#[test]
fn lives_jump_that_lands_on_a_beat_is_found_by_the_position_the_device_records() {
    let beat = 24000;
    let lag = 480;
    let parts = [
        Part { frames: 1000, playing: None, audio: false },
        Part { frames: beat / 2, playing: Some((251.5, beat as f64)), audio: false },
        Part { frames: beat * 8, playing: Some((30.0, beat as f64)), audio: true },
    ];
    assert_eq!(
        runs(
            &parse_capture(&capture(&parts, true, false, 0, false, None), 3, 48000.0).unwrap(),
            Anchors { first: Some(251.5), ..Default::default() }
        )
        .len(),
        1
    );
    for (interleaved, big) in [(true, true), (false, false)] {
        let read = parse_capture(&capture(&parts, interleaved, big, 2000, false, Some(lag)), 4, 48000.0).unwrap();
        assert_eq!(read.left.len(), 1000 + beat / 2 + beat * 8);
        let stretches = runs(&read, Anchors::default());
        assert_eq!(stretches.len(), 2);
        assert_eq!(stretches[1].from, 1000 + beat / 2);
        assert!((stretches[0].beat - 251.5).abs() < 1e-3 && (stretches[1].beat - 30.0).abs() < 1e-3);
        assert_eq!(frame_at(&stretches[1], 32.0), Some(1000 + beat / 2 + beat * 2));
    }
    let landing = [
        Part { frames: 1000, playing: None, audio: false },
        Part { frames: 7000, playing: Some((14.95, beat as f64)), audio: false },
        Part { frames: 64, playing: None, audio: false },
        Part { frames: beat * 6, playing: Some((64.0 / beat as f64, beat as f64)), audio: false },
    ];
    let read = parse_capture(&capture(&landing, true, false, 0, false, Some(lag)), 4, 48000.0).unwrap();
    let stretches = runs(&read, Anchors::default());
    let landed = stretches.last().unwrap();
    assert_eq!(landed.from, 8064);
    assert_eq!(frame_at(landed, 0.0), Some(8000));
    let shown = [
        Part { frames: 500, playing: None, audio: false },
        Part { frames: 7000, playing: Some((12.3, beat as f64)), audio: false },
        Part { frames: beat * 6, playing: Some((40.75, beat as f64)), audio: false },
    ];
    let read = parse_capture(&capture(&shown, true, false, 0, false, Some(lag)), 4, 48000.0).unwrap();
    assert_eq!(
        runs(&read, Anchors::default()).iter().map(|r| (r.from, (r.beat * 1000.0).round() / 1000.0)).collect::<Vec<_>>(),
        vec![(500, 12.3), (7500, 40.75)]
    );
}
#[test]
fn kumi_fetches_the_build_each_computer_has() {
    for (platform, arch, asset) in [
        ("darwin", "arm64", Some("yt-dlp_macos.zip")),
        ("win32", "x64", Some("yt-dlp_win.zip")),
        ("win32", "arm64", Some("yt-dlp_win_arm64.zip")),
        ("linux", "x64", Some("yt-dlp_linux")),
        ("freebsd", "x64", None),
    ] {
        assert_eq!(yt_dlp_asset(platform, arch), asset);
    }
    assert_eq!(whisper_asset("win32", "x64"), Some("whisper-bin-x64.zip"));
    assert_eq!(whisper_asset("linux", "arm64"), Some("whisper-bin-ubuntu-arm64.tar.gz"));
    assert_eq!(whisper_asset("darwin", "arm64"), None);
}
#[test]
fn ffmpegs_build_for_this_computer_is_the_newest_numbered_lgpl_one() {
    // The release tagged latest names a build by its branch; a dated release, by the commit it's built from.
    let latest = [
        "ffmpeg-master-latest-win64-lgpl.zip",
        "ffmpeg-n8.1-latest-win64-lgpl-8.1.zip",
        "ffmpeg-n9.0-latest-win64-lgpl-9.0.zip",
        "ffmpeg-n9.0-latest-win64-gpl-9.0.zip",
        "ffmpeg-n9.0-latest-winarm64-lgpl-9.0.zip",
        "ffmpeg-n10.0-latest-linux64-lgpl-10.0.tar.xz",
        "ffmpeg-n9.0-latest-linuxarm64-lgpl-9.0.tar.xz",
        "ffmpeg-n9.0-latest-win64-lgpl-shared-9.0.zip",
    ];
    let dated = [
        "ffmpeg-N-127222-g151814650f-win64-lgpl.zip",
        "ffmpeg-n8.1.3-14-g330caae0c1-win64-lgpl-8.1.zip",
        "ffmpeg-n9.0.2-22-g46d8f462ee-win64-lgpl-9.0.zip",
        "ffmpeg-n9.0.2-22-g46d8f462ee-win64-gpl-9.0.zip",
        "ffmpeg-n9.0.2-22-g46d8f462ee-winarm64-lgpl-9.0.zip",
        "ffmpeg-n9.0.2-22-g46d8f462ee-linux64-lgpl-9.0.tar.xz",
        "ffmpeg-n9.0.2-22-g46d8f462ee-linuxarm64-lgpl-9.0.tar.xz",
        "ffmpeg-n9.0.2-22-g46d8f462ee-win64-lgpl-shared-9.0.zip",
        "ffmpeg-n8.1.3-14-g330caae0c1-linux64-lgpl-8.1.tar.xz",
        "ffmpeg-N-127222-g151814650f-linux64-lgpl.tar.xz",
    ];
    for names in [latest.map(str::to_string).to_vec(), dated.map(str::to_string).to_vec()] {
        for (platform, arch, expected) in [
            ("win32", "x64", Some(names[2].clone())),
            ("win32", "arm64", Some(names[4].clone())),
            ("linux", "x64", Some(names[5].clone())),
            ("linux", "arm64", Some(names[6].clone())),
            ("darwin", "arm64", None),
            ("win32", "ia32", None),
        ] {
            assert_eq!(ffmpeg_asset(&names, platform, arch), expected);
        }
    }
}
fn model_download(oid: String) -> Download {
    Arc::new(move |url, _| {
        let oid = oid.clone();
        async move {
            Ok(if url.contains("/api/models/") {
                serde_json::to_vec(&serde_json::json!([{ "path":"ggml-tiny.en.bin","size":14,"lfs":{"oid":oid}}])).unwrap()
            } else {
                b"a speech model".to_vec()
            })
        }
        .boxed()
    })
}
#[tokio::test]
async fn a_download_that_doesnt_match_its_published_checksum_isnt_kept() {
    let root = tempfile::tempdir().unwrap();
    let dir = root.path().join("tools-mismatch");
    let asset = yt_dlp_asset(kumi_runtime::system::platform(), node_arch()).unwrap();
    let options = ProgramOptions {
        tools_dir: dir.to_string_lossy().into_owned(),
        env: Some(HashMap::from([("PATH".into(), "".into())])),
        download: Some(Arc::new(move |url, _| {
            async move {
                Ok(if url.ends_with("SHA2-256SUMS") {
                    format!("{}  {asset}\n", "0".repeat(64)).into_bytes()
                } else {
                    b"not yt-dlp".to_vec()
                })
            }
            .boxed()
        })),
        ..Default::default()
    };
    assert!(find_yt_dlp(&options).await.unwrap_err().to_string().contains("didn't match"));
    assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0);
    let mut options = ProgramOptions {
        env: Some(HashMap::new()),
        download: Some(model_download("f".repeat(64))),
        free: Some(Arc::new(|_| async { Some(1e12) }.boxed())),
        ..options
    };
    assert!(whisper_model("ggml-tiny.en.bin", &options).await.unwrap_err().to_string().contains("didn't match"));
    assert_eq!(std::fs::read_dir(dir.join("whisper-models")).unwrap().count(), 0);
    options.download = Some(model_download(hex::encode(Sha256::digest(b"a speech model"))));
    let fetched = Arc::new(Mutex::new(Vec::new()));
    let said = fetched.clone();
    options.on_fetch = Some(Arc::new(move |s| said.lock().unwrap().push(s.to_string())));
    let path = whisper_model("ggml-tiny.en.bin", &options).await.unwrap();
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "a speech model");
    assert_eq!(fetched.lock().unwrap().len(), 1);
    options.download = Some(Arc::new(|_, _| async { Err(VideoFailure::other("fetched again")) }.boxed()));
    assert_eq!(whisper_model("ggml-tiny.en.bin", &options).await.unwrap(), path);
    assert!(whisper_model("../escape.bin", &options).await.unwrap_err().to_string().contains("isn't a whisper.cpp model"));
    options.env = Some(HashMap::from([("KUMI_YTDLP".into(), root.path().join("nowhere").to_string_lossy().into_owned())]));
    assert!(find_yt_dlp(&options).await.unwrap_err().to_string().contains("isn't there"));
}
#[test]
fn kumi_goes_only_to_public_addresses() {
    use kumi_runtime::web::net::{checked_url, private_address};
    for inside in [
        "127.0.0.1",
        "10.1.2.3",
        "172.16.0.1",
        "172.31.255.255",
        "192.168.1.1",
        "169.254.169.254",
        "100.64.0.1",
        "0.0.0.0",
        "224.0.0.1",
        "255.255.255.255",
        "198.18.0.1",
        "::1",
        "::",
        "fe80::1",
        "fe80::1%en0",
        "fc00::1",
        "fd12:3456::1",
        "::ffff:127.0.0.1",
        "::ffff:10.0.0.1",
        "64:ff9b::7f00:1",
        "64:ff9b::10.0.0.1",
        "2002::1",
        "not an address",
    ] {
        assert!(private_address(inside), "{inside}");
    }
    for outside in ["8.8.8.8", "1.1.1.1", "140.82.112.3", "172.32.0.1", "2001:4860:4860::8888", "::ffff:8.8.8.8", "64:ff9b::808:808"] {
        assert!(!private_address(outside), "{outside}");
    }
    for bad in [
        "file:///etc/passwd",
        "ftp://example.com/x",
        "http://user:secret@example.com/",
        "http://localhost:8080/",
        "http://api.localhost/",
        "http://printer.local/",
        "http://intranet/",
        "http://nas.lan/",
        "http://0x7f.1/",
        "http://2130706433/",
        "http://[::1]/",
        "http://[::ffff:127.0.0.1]/",
        "http://169.254.169.254/latest/meta-data/",
        "not a url",
    ] {
        assert!(checked_url(bad, None).is_err(), "{bad}");
    }
    assert_eq!(checked_url("https://example.com/a b", None).unwrap().as_str(), "https://example.com/a%20b");
    assert_eq!(checked_url("http://8.8.8.8/", None).unwrap().host_str(), Some("8.8.8.8"));
}
#[tokio::test]
async fn wavetables_shapes_as_harmonics_keyframes_morphing_cycles_cut_from_a_sound_and_serums_frame_marker() {
    use kumi_runtime::audio::wavetable::*;
    assert_eq!(
        shape_harmonics(Shape::Square, Some(0.5), Some(5)).iter().map(|v| (v * 1000.0).round() / 1000.0).collect::<Vec<_>>(),
        vec![1.0, 0.0, 0.333, 0.0, 0.2]
    );
    assert!(shape_harmonics(Shape::Triangle, Some(0.5), Some(3))[2] < 0.0);
    let sine = synthesize(&[1.0]);
    assert_eq!(sine.len(), FRAME);
    assert!((sine[FRAME / 4] - 1.0).abs() < 1e-6);
    let frames = frames_from_keyframes(
        &[Keyframe { shape: Some(Shape::Sine), ..Default::default() }, Keyframe { shape: Some(Shape::Saw), ..Default::default() }],
        Some(16.0),
    )
    .unwrap();
    assert_eq!(frames.len(), 16);
    assert!(frames.iter().flatten().all(|v| v.abs() <= 0.99 + 1e-6));
    let folder = tempfile::tempdir().unwrap();
    let file = folder.path().join("Kumi Sweep.wav");
    write_wavetable(&file, &frames).await.unwrap();
    let bytes = std::fs::read(&file).unwrap();
    assert!(bytes.windows(4).any(|v| v == b"clm "));
    assert!(bytes.windows(7).any(|v| v == b"<!>2048"));
    let mut source = open_audio(&file, None).await.unwrap();
    assert_eq!(source.frames, 16 * FRAME);
    assert_eq!(source.channels, 1);
    source.close().await.unwrap();
    let saw: Vec<f32> = (0..96000)
        .map(|i| {
            ((1..40).map(|k| (2.0 * std::f64::consts::PI * k as f64 * 110.0 * i as f64 / 48000.0).sin() / k as f64).sum::<f64>() * 0.3)
                as f32
        })
        .collect();
    let note = folder.path().join("wavetable-saw.wav");
    wav(&note, &[saw.clone()], 32);
    assert!((period_of(&saw[..48000], 48000.0).unwrap() - 48000.0 / 110.0).abs() < 1.0);
    let cut = frames_from_audio(note.to_str().unwrap(), 8.0, None, None).await.unwrap();
    assert_eq!(cut.len(), 8);
    assert!(cut.iter().all(|f| f.len() == FRAME));
}
/// A build's archive as BtbN packs it: one folder, named like the archive, with the programs in its bin.
fn ffmpeg_archive(root: &Path, folder: &str) -> Vec<u8> {
    let build = root.join("build").join(folder);
    std::fs::create_dir_all(build.join("bin")).unwrap();
    std::fs::write(build.join("bin/ffmpeg"), "#!/bin/sh\necho ffmpeg version fixture\n").unwrap();
    std::fs::write(build.join("bin/ffprobe"), "x").unwrap();
    std::fs::write(build.join("LICENSE.txt"), "LGPL").unwrap();
    let archive = root.join("build.tar.gz");
    assert!(std::process::Command::new(kumi_runtime::system::system_program_default(kumi_runtime::system::SystemProgram::Tar))
        .args(["-czf", archive.to_str().unwrap(), "-C", root.join("build").to_str().unwrap(), folder])
        .status()
        .unwrap()
        .success());
    std::fs::read(archive).unwrap()
}
const FFMPEG_LATEST: &str = "https://api.github.com/repos/BtbN/FFmpeg-Builds/releases/tags/latest";
const FFMPEG_RELEASES: &str = "https://api.github.com/repos/BtbN/FFmpeg-Builds/releases?per_page=10";
#[tokio::test]
async fn off_a_mac_ffmpeg_is_fetched_once_checked_against_its_release_checksum_and_only_the_program_kept() {
    let root = tempfile::tempdir().unwrap();
    let data = ffmpeg_archive(root.path(), "ffmpeg-n9.0-latest-linux64-lgpl-9.0");
    let sha = format!("sha256:{}", hex::encode(Sha256::digest(&data)));
    let asked = Arc::new(Mutex::new(Vec::new()));
    let said = Arc::new(Mutex::new(Vec::new()));
    fn downloader(data: Vec<u8>, digest: String, asked: Arc<Mutex<Vec<String>>>) -> Download {
        Arc::new(move |url, _| {
            let data = data.clone();
            let digest = digest.clone();
            asked.lock().unwrap().push(url.clone());
            async move{Ok(if url.starts_with("https://api.github.com/"){serde_json::to_vec(&serde_json::json!({"assets":[{"name":"ffmpeg-n9.0-latest-linux64-lgpl-9.0.tar.xz","size":141_000_000,"digest":digest,"browser_download_url":"https://github.com/BtbN/FFmpeg-Builds/releases/download/latest/ffmpeg-n9.0-latest-linux64-lgpl-9.0.tar.xz"}]})).unwrap()}else{data})}.boxed()
        })
    }
    let told = said.clone();
    let tools = root.path().join("tools");
    let mut options = FfmpegOptions {
        env: Some(HashMap::new()),
        tools_dir: Some(tools.to_string_lossy().into_owned()),
        platform: Some("linux".into()),
        arch: Some("x64".into()),
        download: Some(downloader(data.clone(), sha, asked.clone())),
        on_fetch: Some(Arc::new(move |s| told.lock().unwrap().push(s.to_string()))),
        free: Some(Arc::new(|_| async { Some(1e12) }.boxed())),
        installed_only: true,
        ..Default::default()
    };
    assert!(find_ffmpeg(options.clone()).await.unwrap().is_none());
    assert!(asked.lock().unwrap().is_empty());
    options.installed_only = false;
    let found = find_ffmpeg(options.clone()).await.unwrap().unwrap();
    assert_eq!(found, tools.join("ffmpeg").join("ffmpeg").to_string_lossy());
    assert_eq!(std::fs::read_dir(tools.join("ffmpeg")).unwrap().count(), 1);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(&found).unwrap().permissions().mode() & 0o111, 0o111);
    }
    assert_eq!(*said.lock().unwrap(), vec!["Kumi is fetching ffmpeg, which it reads audio formats and videos with (once, about 141 MB)."]);
    assert_eq!(asked.lock().unwrap().len(), 2);
    assert_eq!(find_ffmpeg(options.clone()).await.unwrap().unwrap(), found);
    assert_eq!(asked.lock().unwrap().len(), 2);
    let other = root.path().join("other");
    let mut bad = options.clone();
    bad.tools_dir = Some(other.to_string_lossy().into_owned());
    bad.download = Some(downloader(data, format!("sha256:{}", "0".repeat(64)), Arc::new(Mutex::new(Vec::new()))));
    assert!(find_ffmpeg(bad).await.unwrap_err().to_string().contains("didn't match its release's checksum"));
    assert!(!other.join("ffmpeg").exists());
    assert_eq!(std::fs::read_dir(other).unwrap().count(), 0);
    let before = asked.lock().unwrap().len();
    options.tools_dir = Some(root.path().join("third").to_string_lossy().into_owned());
    options.free = Some(Arc::new(|_| async { Some(150_000_000.0) }.boxed()));
    assert!(find_ffmpeg(options)
        .await
        .unwrap_err()
        .to_string()
        .contains("Kumi needs ffmpeg for this, and would fetch it. Only 150 MB is free on the disk Kumi keeps its programs on"));
    assert_eq!(asked.lock().unwrap().len(), before + 1);
}
#[tokio::test]
async fn while_btbn_makes_its_release_tagged_latest_again_ffmpeg_comes_from_its_newest_dated_release() {
    let root = tempfile::tempdir().unwrap();
    let name = "ffmpeg-n9.0.2-22-g46d8f462ee-linux64-lgpl-9.0.tar.xz";
    let data = ffmpeg_archive(root.path(), name.trim_end_matches(".tar.xz"));
    let digest = format!("sha256:{}", hex::encode(Sha256::digest(&data)));
    let address = |tag: &str| format!("https://github.com/BtbN/FFmpeg-Builds/releases/download/{tag}/{name}");
    let release = |tag: &str| serde_json::json!({"tag_name":tag,"assets":[{"name":name,"size":137_903_428,"digest":digest,"browser_download_url":address(tag)}]});
    // Not newest first, as GitHub's list can be: the largest tag is the newest.
    let releases = serde_json::to_vec(&serde_json::json!([
        release("autobuild-2026-10-05-13-07"),
        release("autobuild-2026-10-06-13-06"),
        release("autobuild-2026-10-04-20-51")
    ]))
    .unwrap();
    let newest = address("autobuild-2026-10-06-13-06");
    // The release tagged latest gone (deleted, not yet uploaded anew), or listing no build for this computer.
    type Answer = fn() -> Result<Vec<u8>, VideoFailure>;
    let gone: Answer = || Err(VideoFailure::video("Downloading latest failed (404)."));
    let unlisted: Answer = || Ok(serde_json::to_vec(&serde_json::json!({"tag_name":"latest","assets":[]})).unwrap());
    for (case, latest) in [("gone", gone), ("listing no build", unlisted)] {
        let asked = Arc::new(Mutex::new(Vec::new()));
        let download: Download = {
            let (asked, releases, data, newest) = (asked.clone(), releases.clone(), data.clone(), newest.clone());
            Arc::new(move |url: String, _| {
                asked.lock().unwrap().push(url.clone());
                let answer = match url.as_str() {
                    FFMPEG_LATEST => latest(),
                    FFMPEG_RELEASES => Ok(releases.clone()),
                    _ if url == newest => Ok(data.clone()),
                    _ => Err(VideoFailure::video(format!("{url} isn't one Kumi should ask for."))),
                };
                async move { answer }.boxed()
            })
        };
        let tools = root.path().join(case);
        let options = FfmpegOptions {
            env: Some(HashMap::new()),
            tools_dir: Some(tools.to_string_lossy().into_owned()),
            platform: Some("linux".into()),
            arch: Some("x64".into()),
            download: Some(download),
            on_fetch: Some(Arc::new(|_| {})),
            free: Some(Arc::new(|_| async { Some(1e12) }.boxed())),
            ..Default::default()
        };
        assert_eq!(find_ffmpeg(options).await.unwrap(), Some(tools.join("ffmpeg").join("ffmpeg").to_string_lossy().into_owned()), "{case}");
        assert_eq!(*asked.lock().unwrap(), [FFMPEG_LATEST, FFMPEG_RELEASES, newest.as_str()], "{case}");
    }
}
#[tokio::test]
async fn when_github_lists_no_ffmpeg_builds_kumi_says_so_in_plain_words_and_a_mac_never_asks() {
    let root = tempfile::tempdir().unwrap();
    let asked = Arc::new(Mutex::new(Vec::new()));
    let silent: Download = {
        let asked = asked.clone();
        Arc::new(move |url: String, _| {
            asked.lock().unwrap().push(url);
            async { Err(VideoFailure::other("error sending request")) }.boxed()
        })
    };
    let options = |platform: &str, arch: &str, download: &Download| FfmpegOptions {
        env: Some(HashMap::new()),
        tools_dir: Some(root.path().to_string_lossy().into_owned()),
        platform: Some(platform.into()),
        arch: Some(arch.into()),
        download: Some(download.clone()),
        ..Default::default()
    };
    assert_eq!(
        find_ffmpeg(options("linux", "x64", &silent)).await.unwrap_err().to_string(),
        "Kumi couldn't get the list of ffmpeg's builds from GitHub to fetch it. Try again in a few minutes, or install it with your package manager."
    );
    assert_eq!(*asked.lock().unwrap(), [FFMPEG_LATEST, FFMPEG_RELEASES]);
    assert!(find_ffmpeg(options("win32", "x64", &silent))
        .await
        .unwrap_err()
        .to_string()
        .ends_with("Try again in a few minutes, or install it: winget install ffmpeg."));
    // Listed, but with no build for this computer: there's none, and nothing's wrong.
    let empty: Download = Arc::new(|url: String, _| {
        let listed = if url == FFMPEG_LATEST { serde_json::json!({"tag_name":"latest","assets":[]}) } else { serde_json::json!([]) };
        async move { Ok(serde_json::to_vec(&listed).unwrap()) }.boxed()
    });
    assert_eq!(find_ffmpeg(options("linux", "x64", &empty)).await.unwrap(), None);
    asked.lock().unwrap().clear();
    for (platform, arch) in [("darwin", "arm64"), ("darwin", "x64"), ("win32", "ia32")] {
        assert_eq!(find_ffmpeg(options(platform, arch, &silent)).await.unwrap(), None, "{platform} {arch}");
    }
    assert!(asked.lock().unwrap().is_empty());
}
#[test]
fn kumi_ears_passes_the_sound_through_records_four_channels_and_every_patch_cord_joins_real_inlets_and_outlets() {
    use kumi_runtime::ears::device::*;
    let patch = ears_patcher();
    let patch = &patch["patcher"];
    let boxes: std::collections::HashMap<_, _> =
        patch["boxes"].as_array().unwrap().iter().map(|v| (v["box"]["id"].as_str().unwrap(), &v["box"])).collect();
    let lines = patch["lines"].as_array().unwrap();
    for line in lines {
        let line = &line["patchline"];
        let source = &boxes[line["source"][0].as_str().unwrap()];
        let dest = &boxes[line["destination"][0].as_str().unwrap()];
        assert!(line["source"][1].as_u64().unwrap() < source["numoutlets"].as_u64().unwrap());
        assert!(line["destination"][1].as_u64().unwrap() < dest["numinlets"].as_u64().unwrap());
    }
    let cord = |s: &str, o: u64, d: &str, i: u64| {
        lines
            .iter()
            .any(|v| v["patchline"]["source"] == serde_json::json!([s, o]) && v["patchline"]["destination"] == serde_json::json!([d, i]))
    };
    assert!(cord("obj-plugin", 0, "obj-plugout", 0) && cord("obj-plugin", 1, "obj-plugout", 1));
    assert!(cord("obj-plugin", 0, "obj-record", 0) && cord("obj-plugin", 1, "obj-record", 1) && cord("obj-beat1", 0, "obj-record", 2));
    assert_eq!(boxes["obj-record"]["text"], "record~ ---kumiears 4");
    assert!(cord("obj-sync", 6, "obj-where", 0) && cord("obj-where", 0, "obj-record", 3));
    assert!(!ears_code().contains("__"));
    assert!(ears_code().contains("const VERSION = 3;"));
    assert_eq!(boxes["obj-code"]["code"], ears_code());
    for (index, port) in KUMI_PORTS.iter().enumerate() {
        assert_eq!(boxes[format!("obj-send-{index}").as_str()]["text"], format!("udpsend 127.0.0.1 {port}"));
        assert!(cord("obj-code", 5 + index as u64, &format!("obj-send-{index}"), 0));
    }
    assert_eq!(kumi_runtime::devices::amxd::decode_amxd(&ears_file()).unwrap().kind, kumi_runtime::devices::amxd::DeviceType::AudioEffect);
    assert_eq!(EARS_ITEM, "user_library/Kumi/Kumi Ears");
}
#[tokio::test]
async fn the_device_goes_into_the_user_librarys_kumi_folder_once_and_again_only_when_it_differs() {
    use kumi_runtime::ears::device::*;
    let library = tempfile::tempdir().unwrap();
    let first = install_ears(library.path()).await.unwrap();
    assert!(first.written);
    assert_eq!(first.file, library.path().join("Kumi").join("Kumi Ears.amxd").to_string_lossy());
    assert!(!install_ears(library.path()).await.unwrap().written);
    std::fs::write(&first.file, "changed").unwrap();
    assert!(install_ears(library.path()).await.unwrap().written);
    assert_eq!(std::fs::read(first.file).unwrap(), ears_file());
}
#[tokio::test]
async fn kumis_socket_hears_devices_hellos_and_arms_writes_and_stops_them_by_token() {
    use kumi_runtime::ears::{device::EARS_VERSION, link::*};
    use std::rc::Rc;
    tokio::task::LocalSet::new().run_until(async{
        let link=open_ears_link(EarsOptions{port:Some(0),..Default::default()}).await.unwrap();let device=Rc::new(tokio::net::UdpSocket::bind(("127.0.0.1",0)).await.unwrap());let port=device.local_addr().unwrap().port();let root=tempfile::tempdir().unwrap();let raw=root.path().join("device.raw").to_string_lossy().into_owned();let answering=device.clone();let stop=kumi_common::abort::Signal::new();let closed=stop.clone();
        let worker=tokio::task::spawn_local(async move{let mut packet=vec![0u8;65536];loop{tokio::select!{_ = closed.cancelled()=>break,read=answering.recv_from(&mut packet)=>{let(n,_)=read.unwrap();let msg=decode_osc(&packet[..n]).unwrap();let reply=msg.args[1].as_number().unwrap() as u16;let token=msg.args[2].as_text().unwrap();let (address,args)=match msg.address.as_str(){
            "/kumi/ears/arm"=>("/kumi/ears/armed",vec![token.into(),port.into(),64.5.into(),1.into(),48000.into()]),
            "/kumi/ears/write"=>{let file=msg.args[0].as_text().unwrap();std::fs::write(file,capture(&[Part{frames:480,playing:Some((64.5,480.0)),audio:false}],true,false,0,false,None)).unwrap();("/kumi/ears/written",vec![token.into(),port.into(),file.into(),48000.into(),3.into(),64.5.into(),1.into()])},
            "/kumi/ears/ping"=>("/kumi/ears/pong",vec![token.into(),port.into(),77.into(),(EARS_VERSION as i32).into(),48000.into(),"live_set tracks 2 devices 4".into(),1500.into(),33.25.into(),1.into()]),_=>continue};answering.send_to(&encode_osc(address,&args),("127.0.0.1",reply)).await.unwrap();}}}});
        device.send_to(&encode_osc("/kumi/ears/hello",&[port.into(),77.into(),(EARS_VERSION as i32).into(),48000.into(),"live_set tracks 2 devices 4".into()]),("127.0.0.1",link.port())).await.unwrap();
        let tap=link.wait_for(Rc::new(|tap|tap.path.starts_with("live_set tracks 2 devices ")),2000,None).await.unwrap();assert_eq!((tap.id,tap.port,tap.path.as_str()),(77.0,port,"live_set tracks 2 devices 4"));assert_eq!(link.taps().iter().map(|tap|tap.id).collect::<Vec<_>>(),vec![77.0]);assert_eq!(link.arm(&tap,10.0,None).await.unwrap(),Armed{beats:64.5,running:true,sample_rate:48000.0});let written=link.write(&tap,&raw,None).await.unwrap();assert_eq!(written.file,raw);assert_eq!(written.channels,3);assert_eq!(written.beats,64.5);assert_eq!(read_capture(Path::new(&raw),3,48000.0).await.unwrap().left.len(),480);assert_eq!(link.ping(&tap,None).await.unwrap().path,"live_set tracks 2 devices 4");assert_eq!(link.transport(&tap,None).await.unwrap(),Some(Transport{beats:33.25,running:true}));
        device.send_to(&encode_osc("/kumi/ears/hello",&[port.saturating_add(1).into(),78.into(),(EARS_VERSION as i32+1).into(),48000.into(),"live_set tracks 3 devices 0".into()]),("127.0.0.1",link.port())).await.unwrap();tokio::time::sleep(std::time::Duration::from_millis(50)).await;assert_eq!(link.taps().iter().map(|tap|tap.id).collect::<Vec<_>>(),vec![77.0]);
        let dead=tokio::net::UdpSocket::bind(("127.0.0.1",0)).await.unwrap();assert!(link.arm(&Tap{port:dead.local_addr().unwrap().port(),path:"live_set master_track devices 0".into(),..tap},1.0,None).await.unwrap_err().0.contains("listening device on Main didn't answer"));stop.cancel();worker.await.unwrap();link.close().await;
        assert_eq!(place_of("live_set return_tracks 1 devices 3"),Some(Place{kind:"return".into(),index:1,device:3}));assert_eq!(place_of("live_set master_track devices 0"),Some(Place{kind:"main".into(),index:0,device:0}));assert_eq!(describe("live_set tracks 0 devices 1"),"track 1");
    }).await;
}
fn harmonic_tone(seconds: f64, hz: f64, amplitude: f64, harmonics: impl Fn(usize) -> f64) -> Vec<f32> {
    let count = (seconds * 48000.0).round() as usize;
    let partials: Vec<_> = (1..=40).filter(|k| *k as f64 * hz < 23000.0 && harmonics(*k) != 0.0).collect();
    (0..count)
        .map(|i| {
            (partials.iter().map(|k| harmonics(*k) * (2.0 * std::f64::consts::PI * *k as f64 * hz * i as f64 / 48000.0).sin()).sum::<f64>()
                * amplitude) as f32
        })
        .collect()
}
#[tokio::test]
async fn a_stereo_1khz_sine_at_minus20_dbfs_reads_minus20_lufs_one_band_fully_correlated() {
    use kumi_runtime::audio::analyze::*;
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("sine-1k.wav");
    let sine = tone(48000 * 8, 1000.0, 0.1);
    wav(&file, &[sine.clone(), sine], 16);
    let r = analyze_file(file.to_str().unwrap(), AnalyzeOptions { focus: Some("mix".into()), ..Default::default() }).await.unwrap();
    assert!((r.loudness.integrated_lufs.unwrap() + 20.0).abs() < 0.3);
    assert!((r.loudness.true_peak_dbtp + 20.0).abs() < 0.3);
    assert!(r.loudness.range_lu.unwrap_or(0.0) < 0.5);
    let near = r.balance.bands.iter().filter(|b| b.name == "mids" || b.name == "upper mids").map(|b| 10f64.powf(b.db / 10.0)).sum::<f64>();
    assert!(10.0 * near.log10() > -0.2);
    let stereo = r.stereo.unwrap();
    assert_eq!(stereo.correlation, 1.0);
    assert_eq!(stereo.width, 0.0);
    assert!(stereo.low_end_mono);
    assert_eq!(r.spectrogram.rows.len(), 10);
    assert!(r.spectrogram.rows.iter().any(|r| r.cells.chars().all(|c| c == '9')));
}
#[tokio::test]
async fn out_of_phase_channels_read_as_negative_correlation_and_wide() {
    use kumi_runtime::audio::analyze::*;
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("phase.wav");
    let left = tone(48000 * 3, 300.0, 0.3);
    let right = left.iter().map(|v| -v).collect();
    wav(&file, &[left, right], 16);
    let r = analyze_file(file.to_str().unwrap(), AnalyzeOptions { focus: Some("mix".into()), ..Default::default() }).await.unwrap();
    assert!(r.stereo.unwrap().correlation < -0.95);
    assert!(r.balance.bands.iter().find(|b| b.name == "upper bass").unwrap().width > 0.95);
}
#[tokio::test]
async fn a_saw_a_square_and_a_sine_are_told_apart_with_their_pitch() {
    use kumi_runtime::audio::analyze::*;
    let dir = tempfile::tempdir().unwrap();
    for (name, hz, shape, expected) in [
        ("saw-a2.wav", 110.0, 0, "A2"),
        ("square-a3.wav", 220.0, 1, "A3"),
        ("sine-e4.wav", 329.63, 2, "E4"),
        ("saw-f1.wav", 43.65, 0, "F1"),
    ] {
        let file = dir.path().join(name);
        let samples = harmonic_tone(if expected == "F1" { 2.0 } else { 1.5 }, hz, 0.3, |k| match shape {
            0 => 1.0 / k as f64,
            1 => {
                if k % 2 == 1 {
                    1.0 / k as f64
                } else {
                    0.0
                }
            }
            _ => {
                if k == 1 {
                    1.0
                } else {
                    0.0
                }
            }
        });
        wav(&file, &[samples], 16);
        let r = analyze_file(file.to_str().unwrap(), Default::default()).await.unwrap();
        assert_eq!(r.analyzed.focus, "sound");
        let sound = r.sound.unwrap();
        assert_eq!(sound.pitch.as_ref().unwrap().note, expected);
        assert!((sound.pitch.unwrap().hz - hz).abs() < 1.0);
        let h = sound.harmonics.unwrap();
        assert!(
            h.shape.contains(match shape {
                0 => "saw",
                1 => "square",
                _ => "sine",
            }),
            "{}",
            h.shape
        );
        if name == "saw-a2.wav" {
            assert!((h.slope_db_per_octave + 6.0).abs() < 1.5);
        }
    }
}
fn deterministic_noise(length: usize) -> Vec<f32> {
    let mut x = 7.0;
    (0..length)
        .map(|_| {
            x = (x * 1103515245.0 + 12345.0) % 2147483648.0;
            (0.3 * (x / 1073741824.0 - 1.0)) as f32
        })
        .collect()
}
#[tokio::test]
async fn noise_has_no_pitch_a_shaped_note_has_its_envelope_measured() {
    use kumi_runtime::audio::analyze::*;
    let dir = tempfile::tempdir().unwrap();
    let noise = dir.path().join("noise.wav");
    wav(&noise, &[deterministic_noise(48000)], 16);
    let hiss = analyze_file(noise.to_str().unwrap(), Default::default()).await.unwrap();
    assert!(hiss.sound.unwrap().pitch.is_none());
    let mut shaped = harmonic_tone(1.5, 220.0, 0.5, |k| 1.0 / k as f64);
    for (i, sample) in shaped.iter_mut().enumerate() {
        let t = i as f64 / 48000.0;
        let level = if t < 0.04 {
            t / 0.04
        } else if t < 0.24 {
            1.0 - 0.5 * (t - 0.04) / 0.2
        } else if t < 1.2 {
            0.5
        } else {
            (0.5 * (1.0 - (t - 1.2) / 0.3)).max(0.0)
        };
        *sample = (*sample as f64 * level) as f32;
    }
    let file = dir.path().join("adsr.wav");
    wav(&file, &[shaped], 16);
    let note = analyze_file(file.to_str().unwrap(), Default::default()).await.unwrap();
    let e = note.sound.unwrap().envelope;
    assert!((20.0..=60.0).contains(&e.attack_ms));
    assert!((e.sustain_db + 6.0).abs() < 1.5);
    assert!((100.0..=300.0).contains(&e.decay_ms));
    assert!((150.0..=400.0).contains(&e.release_ms));
}
#[tokio::test]
async fn a_wobbles_rate_is_heard_as_a_filter_lfo_and_named_at_the_tempo() {
    use kumi_runtime::audio::analyze::*;
    let samples: Vec<f32> = (0..48000 * 3)
        .map(|i| {
            let t = i as f64 / 48000.0;
            let open = 0.5 + 0.5 * (2.0 * std::f64::consts::PI * 4.0 * t).sin();
            ((1..=30)
                .map(|k| (if k <= 2 { 1.0 } else { open }) / k as f64 * (2.0 * std::f64::consts::PI * k as f64 * 55.0 * t).sin())
                .sum::<f64>()
                * 0.2) as f32
        })
        .collect();
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("wobble.wav");
    wav(&file, &[samples], 16);
    let r = analyze_file(file.to_str().unwrap(), Default::default()).await.unwrap();
    let lfo = r.sound.unwrap().movement.lfo.unwrap();
    assert!((lfo.hz - 4.0).abs() < 0.3, "{}", lfo.hz);
    assert!(lfo.on.contains("brightness"));
    assert_eq!(note_value(4.0, 120.0), Some("1/8".into()));
    assert_eq!(note_value(3.0, 120.0), Some("1/4 triplet".into()));
    assert_eq!(note_value(6.0, 120.0), Some("1/8 triplet".into()));
    assert_eq!(note_value(5.1, 120.0), None);
}
#[tokio::test]
async fn clicks_at_128_bpm_give_the_tempo_c_major_material_gives_the_key() {
    use kumi_runtime::audio::analyze::*;
    let dir = tempfile::tempdir().unwrap();
    let length = 48000 * 16;
    let mut beats = vec![0f32; length];
    let every = 48000.0 * 60.0 / 128.0;
    let mut beat = 0;
    while (beat as f64) * every < (length as f64) {
        let at = (beat as f64 * every).round() as usize;
        for i in 0..2000.min(length - at) {
            beats[at + i] = ((-(i as f64) / 300.0).exp() * (2.0 * std::f64::consts::PI * 60.0 * i as f64 / 48000.0).sin() * 0.8) as f32;
        }
        beat += 1;
    }
    let file = dir.path().join("clicks-128.wav");
    wav(&file, &[beats.clone(), beats], 16);
    let r = analyze_file(file.to_str().unwrap(), AnalyzeOptions { focus: Some("mix".into()), ..Default::default() }).await.unwrap();
    assert!((r.tempo.unwrap().bpm - 128.0).abs() < 1.5);
    assert!(r.dynamics.onsets_per_second > 1.5 && r.dynamics.onsets_per_second < 3.0);
    let notes = [261.63, 329.63, 392.0, 261.63, 349.23, 440.0, 392.0, 493.88, 293.66, 261.63, 329.63, 392.0];
    let mut song = Vec::new();
    for (i, hz) in notes.iter().enumerate() {
        let part = harmonic_tone(1.0, *hz, 0.2, |k| if k <= 3 { 1.0 / k as f64 } else { 0.0 });
        let bass = tone(48000, [130.81, 174.61, 196.0][i % 3], 0.15);
        song.extend(part.iter().zip(bass).map(|(p, b)| (*p as f64 + b as f64) as f32));
    }
    let file = dir.path().join("c-major.wav");
    wav(&file, &[song.clone(), song], 16);
    let r = analyze_file(file.to_str().unwrap(), AnalyzeOptions { focus: Some("mix".into()), ..Default::default() }).await.unwrap();
    assert_eq!(r.key.unwrap().name, "C major");
}
fn plucked_notes(list: &[(f64, f64, f64)], seconds: f64) -> Vec<f32> {
    let mut out = vec![0f32; (seconds * 48000.0).round() as usize];
    for (time, midi, amplitude) in list {
        let hz = 440.0 * 2f64.powf((midi - 69.0) / 12.0);
        let from = (time * 48000.0).round() as usize;
        for i in 0..9600.min(out.len() - from) {
            let phase = (i as f64 * hz / 48000.0) % 1.0;
            out[from + i] = (out[from + i] as f64
                + amplitude * (2.0 * phase - 1.0) * (-(i as f64) / (0.08 * 48000.0)).exp() * (i as f64 / 48.0).min(1.0))
                as f32;
        }
    }
    out
}
#[tokio::test]
async fn the_notes_in_a_part_are_transcribed_when_each_starts_its_pitch_and_how_hard() {
    use kumi_runtime::audio::analyze::*;
    let played = [(0.1, 60.0, 0.5), (0.4, 64.0, 0.5), (0.7, 67.0, 0.25), (1.0, 72.0, 0.5), (1.3, 48.0, 0.5), (1.6, 60.0, 0.12)];
    let samples = plucked_notes(&played, 2.2);
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("transcribe.wav");
    wav(&file, &[samples.clone(), samples], 16);
    let r = analyze_file(file.to_str().unwrap(), AnalyzeOptions { focus: Some("mix".into()), transcribe: true, ..Default::default() })
        .await
        .unwrap();
    let heard = r.notes.unwrap();
    assert_eq!(heard.len(), played.len(), "{heard:?}");
    for (i, (time, midi, _)) in played.iter().enumerate() {
        assert!((heard[i].time - time).abs() < 0.03);
        assert_eq!(heard[i].midi, Some(*midi));
    }
    assert!(heard[2].velocity < heard[1].velocity && heard[5].velocity < heard[2].velocity);
    assert!(heard.iter().all(|n| n.duration > 0.05 && n.duration < 0.3));
    let timeline = r.timeline.unwrap();
    for (time, _, _) in played {
        let at = (time / timeline.step).round() as usize;
        assert!(
            timeline.onset[at.saturating_sub(3)..(at + 4).min(timeline.onset.len())].iter().copied().fold(f64::NEG_INFINITY, f64::max)
                > 0.2
        );
    }
}
#[tokio::test]
async fn a_dense_line_of_16ths_at_varied_velocities_is_transcribed_note_for_note_a_note_at_the_start_too() {
    use kumi_runtime::audio::analyze::*;
    let played: Vec<_> = [0, 1, 2, 4, 5, 7, 8, 9, 11, 12, 13, 15]
        .iter()
        .enumerate()
        .map(|(i, s)| (*s as f64 * 0.125, [62.0, 65.0, 69.0, 62.0, 70.0, 67.0][i % 6], [0.5, 0.2, 0.35, 0.15, 0.45, 0.25][i % 6]))
        .collect();
    let samples = plucked_notes(&played, 2.3);
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("transcribe-dense.wav");
    wav(&file, &[samples.clone(), samples], 16);
    let r = analyze_file(file.to_str().unwrap(), AnalyzeOptions { focus: Some("mix".into()), transcribe: true, ..Default::default() })
        .await
        .unwrap();
    let heard = r.notes.unwrap();
    let found = played.iter().filter(|(time, _, _)| heard.iter().any(|n| (n.time - time).abs() < 0.03)).count();
    assert!(found >= played.len() - 1, "found {found}: {heard:?}");
    assert!(heard.len() <= played.len() + 1);
}
fn saw(seconds: f64, hz: f64, cutoff: f64) -> Vec<f32> {
    let mut out: Vec<f32> =
        (0..(seconds * 48000.0).round() as usize).map(|i| (0.3 * (2.0 * ((i as f64 * hz / 48000.0) % 1.0) - 1.0)) as f32).collect();
    let a = (-2.0 * std::f64::consts::PI * cutoff / 48000.0).exp();
    for _ in 0..2 {
        let mut state = 0.0;
        for v in &mut out {
            state = (1.0 - a) * *v as f64 + a * state;
            *v = state as f32;
        }
    }
    out
}
fn pattern(seconds: f64, every: f64) -> Vec<f32> {
    let mut out = vec![0f32; (seconds * 48000.0).round() as usize];
    let note = saw(0.08, 110.0, 20000.0);
    let mut at = 0;
    while at + note.len() < out.len() {
        for (i, v) in note.iter().enumerate() {
            out[at + i] = (*v as f64 * (-(i as f64) / (0.02 * 48000.0)).exp()) as f32;
        }
        at += (every * 48000.0).round() as usize;
    }
    out
}
async fn analyze_samples(root: &Path, name: &str, samples: Vec<f32>, focus: &str) -> kumi_runtime::audio::analyze::Analysis {
    let file = root.join(name);
    wav(&file, &[samples.clone(), samples], 16);
    kumi_runtime::audio::analyze::analyze_file(
        file.to_str().unwrap(),
        kumi_runtime::audio::analyze::AnalyzeOptions { focus: Some(focus.into()), ..Default::default() },
    )
    .await
    .unwrap()
}
#[tokio::test]
async fn the_same_sound_against_itself_is_100_a_tone_against_noise_is_low_the_same_tone_darkened_is_in_between() {
    use kumi_runtime::audio::matching::*;
    let dir = tempfile::tempdir().unwrap();
    let bright = analyze_samples(dir.path(), "bright.wav", saw(3.0, 110.0, 20000.0), "sound").await;
    let again = analyze_samples(dir.path(), "again.wav", saw(3.0, 110.0, 20000.0), "sound").await;
    let dark = analyze_samples(dir.path(), "dark.wav", saw(3.0, 110.0, 700.0), "sound").await;
    let hiss = analyze_samples(dir.path(), "hiss.wav", deterministic_noise(3 * 48000), "sound").await;
    let same = closeness(&bright, &again, None);
    let darker = closeness(&dark, &bright, None);
    let unlike = closeness(&hiss, &bright, None);
    assert!(same.score >= 97.0, "{}", same.score);
    assert!(unlike.score <= 45.0, "{}", unlike.score);
    assert!(
        darker.score > unlike.score + 10.0 && darker.score < same.score - 10.0,
        "{} between {} and {}",
        darker.score,
        unlike.score,
        same.score
    );
    assert!(same.gaps.is_empty());
    assert!(darker.gaps.join(" · ").contains("darker") || darker.gaps.join(" · ").contains("brighten"));
    assert!(darker.features.iter().all(|f| (0.0..=100.0).contains(&f.similarity)));
}
#[tokio::test]
async fn a_section_is_judged_on_density_and_rhythm_too() {
    use kumi_runtime::audio::matching::*;
    let dir = tempfile::tempdir().unwrap();
    let part = analyze_samples(dir.path(), "part.wav", pattern(8.0, 0.5), "mix").await;
    let same = analyze_samples(dir.path(), "same.wav", pattern(8.0, 0.5), "mix").await;
    let busy = analyze_samples(dir.path(), "busy.wav", pattern(8.0, 0.125), "mix").await;
    let close = closeness(&same, &part, Some(Focus::Section));
    let dense = closeness(&busy, &part, Some(Focus::Section));
    assert_eq!(close.focus, Focus::Section);
    assert!(close.score >= 95.0);
    assert!(dense.score < close.score - 10.0);
    assert!(dense.features.iter().find(|f| f.name == FeatureName::Density).unwrap().similarity < 60.0);
    assert!(dense.gaps.join(" · ").contains("too dense"));
}
#[tokio::test]
async fn a_gap_no_knob_closes_is_named_with_the_structure_that_closes_it() {
    use kumi_runtime::audio::matching::*;
    let dir = tempfile::tempdir().unwrap();
    let sub = saw(3.0, 110.0, 20000.0)
        .iter()
        .enumerate()
        .map(|(i, v)| (*v as f64 + 0.5 * (2.0 * std::f64::consts::PI * 41.2 * i as f64 / 48000.0).sin()) as f32)
        .collect();
    let reference = analyze_samples(dir.path(), "with-sub.wav", sub, "sound").await;
    let mine = analyze_samples(dir.path(), "no-sub.wav", saw(3.0, 110.0, 20000.0), "sound").await;
    let result = closeness(&mine, &reference, None);
    let structural = result.structural.unwrap();
    assert_eq!(structural.kind, StructuralKind::MissingLow);
    assert!(structural.r#move.contains("sub layer"));
    assert!(closeness(&mine, &mine, None).structural.is_none());
}
#[tokio::test]
async fn a_sections_timing_counts_and_a_part_that_builds_against_one_that_doesnt_is_named() {
    use kumi_runtime::audio::matching::*;
    let dir = tempfile::tempdir().unwrap();
    let hit: Vec<_> =
        saw(0.08, 110.0, 20000.0).iter().enumerate().map(|(i, v)| (*v as f64 * (-(i as f64) / (0.02 * 48000.0)).exp()) as f32).collect();
    let place = |times: &[f64], gain: &dyn Fn(f64) -> f64| {
        let mut out = vec![0f32; (4.2 * 48000.0) as usize];
        for time in times {
            let from = (time * 48000.0).round() as usize;
            for (i, v) in hit.iter().enumerate() {
                if from + i < out.len() {
                    out[from + i] = (out[from + i] as f64 + *v as f64 * gain(*time)) as f32;
                }
            }
        }
        out
    };
    let straight: Vec<_> = (0..16).map(|i| i as f64 * 0.25).collect();
    let other: Vec<_> = straight.iter().enumerate().map(|(i, t)| t + if i % 2 == 1 { 0.125 } else { 0.0 }).collect();
    let reference = analyze_samples(dir.path(), "rhythm-ref.wav", place(&straight, &|_| 1.0), "mix").await;
    let same = closeness(
        &analyze_samples(dir.path(), "rhythm-same.wav", place(&straight, &|_| 1.0), "mix").await,
        &reference,
        Some(Focus::Section),
    );
    let shifted = closeness(
        &analyze_samples(dir.path(), "rhythm-shifted.wav", place(&other, &|_| 1.0), "mix").await,
        &reference,
        Some(Focus::Section),
    );
    let rhythm = |c: &Closeness| c.features.iter().find(|f| f.name == FeatureName::Rhythm).unwrap().similarity;
    assert!(rhythm(&same) > 90.0 && rhythm(&shifted) < rhythm(&same) - 20.0);
    assert!(shifted.score < same.score);
    let swell = analyze_samples(dir.path(), "contour-swell.wav", place(&straight, &|t| 0.05 + t / 4.0), "mix").await;
    let level =
        closeness(&analyze_samples(dir.path(), "contour-level.wav", place(&straight, &|_| 0.5), "mix").await, &swell, Some(Focus::Section));
    assert!(level.gaps.join(" ").contains("the reference builds up over time"));
}
#[tokio::test]
async fn a_song_length_file_is_analyzed_quickly_part_by_part() {
    use kumi_runtime::audio::analyze::*;
    let dir = tempfile::tempdir().unwrap();
    let samples = deterministic_noise(75 * 48000);
    let file = dir.path().join("long.wav");
    wav(&file, &[samples.clone(), samples], 16);
    let started = std::time::Instant::now();
    let r = analyze_file(file.to_str().unwrap(), Default::default()).await.unwrap();
    let elapsed = started.elapsed();
    assert_eq!(r.analyzed.focus, "mix");
    assert_eq!(r.spectrogram.rows[0].cells.chars().count(), 24);
    assert_eq!(r.over_time.lufs.len(), 16);
    assert!(elapsed.as_millis() < 6000, "75 seconds took {} ms", elapsed.as_millis());
    let part = analyze_file(file.to_str().unwrap(), AnalyzeOptions { start: Some(30.0), seconds: Some(30.0), ..Default::default() })
        .await
        .unwrap();
    assert_eq!(part.analyzed.from, "0:30");
    assert_eq!(part.analyzed.to, "1:00");
    assert!(analyze_file(file.to_str().unwrap(), AnalyzeOptions { start: Some(90.0), ..Default::default() })
        .await
        .unwrap_err()
        .0
        .contains("no audio in that part of the file: it's 1:15 long"));
}
#[tokio::test]
async fn native_analysis_and_matching_match_the_typescript_reference_for_seeded_signals() {
    use kumi_runtime::audio::matching::closeness;
    let reference: serde_json::Value = serde_json::from_str(include_str!("support/media/analysis-reference.json")).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let bright = analyze_samples(dir.path(), "bright.wav", saw(3.0, 110.0, 20000.0), "sound").await;
    let dark = analyze_samples(dir.path(), "dark.wav", saw(3.0, 110.0, 700.0), "sound").await;
    let hiss = analyze_samples(dir.path(), "hiss.wav", deterministic_noise(3 * 48000), "sound").await;
    for (name, value) in [
        ("bright", serde_json::to_value(&bright).unwrap()),
        ("dark", serde_json::to_value(&dark).unwrap()),
        ("hiss", serde_json::to_value(&hiss).unwrap()),
        ("darker", serde_json::to_value(closeness(&dark, &bright, None)).unwrap()),
        ("unlike", serde_json::to_value(closeness(&hiss, &bright, None)).unwrap()),
    ] {
        assert_eq!(kumi_common::js::json::stringify(&value), kumi_common::js::json::stringify(&reference[name]), "{name}");
    }
}
#[tokio::test]
async fn a_listening_worker_keeps_the_runtime_responsive_and_honors_cancellation() {
    use kumi_runtime::audio::{hear, AnalyzeOptions};
    let root = tempfile::tempdir().unwrap();
    let file = root.path().join("worker.wav");
    wav(&file, &[deterministic_noise(48000 * 30)], 16);
    let stop = kumi_common::abort::Signal::new();
    let trigger = stop.clone();
    let stopped = tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        trigger.cancel();
    });
    let started = std::time::Instant::now();
    assert!(hear(file.to_str().unwrap(), AnalyzeOptions { signal: Some(stop), ..Default::default() }).await.is_err());
    stopped.await.unwrap();
    assert!(started.elapsed() < std::time::Duration::from_secs(2));
    let result = hear(file.to_str().unwrap(), AnalyzeOptions { seconds: Some(0.2), ..Default::default() }).await.unwrap();
    assert_eq!(result.file, "worker.wav");
}
#[tokio::test]
async fn a_songs_form_sections_where_whats_played_and_how_it_sounds_change_in_bars() {
    use kumi_runtime::audio::{
        structure::*,
        tools::{listening_tools, ListeningOptions},
    };
    let root = tempfile::tempdir().unwrap();
    let mut song = Vec::new();
    for (chord, pad, hats, kick, bass) in [
        (220.0, 0.05, true, false, 0.0),
        (220.0, 0.05, true, true, 55.0),
        (174.61, 0.04, false, false, 0.0),
        (220.0, 0.05, true, true, 55.0),
    ] {
        for i in 0..8 * 2 * 48000 {
            let t = i as f64 / 48000.0;
            let mut value = pad * (2.0 * std::f64::consts::PI * chord * t).sin();
            if hats {
                let since = t % 0.25;
                value += 0.06 * (-since * 120.0).exp() * (((i as f64 * 1103515245.0 + 12345.0) % 2147483648.0) / 1073741824.0 - 1.0);
            }
            if kick {
                let since = t % 0.5;
                value += 0.7 * (-since * 18.0).exp() * (2.0 * std::f64::consts::PI * (50.0 + 70.0 * (-since * 30.0).exp()) * since).sin();
            }
            if bass != 0.0 {
                value += 0.2 * (2.0 * std::f64::consts::PI * bass * t).sin() + 0.08 * (4.0 * std::f64::consts::PI * bass * t).sin();
            }
            song.push(value as f32);
        }
    }
    let file = root.path().join("form.wav");
    wav(&file, &[song.clone(), song], 16);
    let form = hear_form(file.to_str().unwrap(), FormOptions { tempo: Some(120.0), ..Default::default() }).await.unwrap();
    assert_eq!(form.tempo.from, "file");
    assert!((form.tempo.bpm - 120.0).abs() < 2.0);
    assert_eq!(form.bars, 32);
    assert_eq!(form.sections.iter().map(|s| (s.bar, s.bars)).collect::<Vec<_>>(), vec![(1, 8), (9, 8), (17, 8), (25, 8)]);
    assert_eq!(form.sections.iter().map(|s| s.like.as_str()).collect::<Vec<_>>(), vec!["A", "B", "C", "B"]);
    assert_eq!(form.sections.iter().map(|s| s.level.as_str()).collect::<Vec<_>>(), vec!["low", "high", "low", "high"]);
    assert_eq!(form.sections.iter().map(|s| s.low.as_str()).collect::<Vec<_>>(), vec!["thin", "full", "thin", "full"]);
    assert_eq!(form.sections.iter().map(|s| s.role.as_str()).collect::<Vec<_>>(), vec!["intro", "peak", "break", "peak"]);
    assert_eq!(form.sections[2].density, "sparse");
    assert_eq!(form.sections[0].from, "0:00");
    assert_eq!(form.sections[1].from, "0:16");
    assert!(regex::Regex::new(r"^intro 8 · peak 8 · break 8 · peak 8 \(32 bars at 1[0-9][0-9](\.[0-9])? BPM\)$")
        .unwrap()
        .is_match(&form.summary));
    assert_eq!(
        hear_form(file.to_str().unwrap(), FormOptions { tempo: Some(128.0), ..Default::default() }).await.unwrap().at_set_tempo,
        Some("1:00 at the Set's 128 BPM".into())
    );
    let tool = listening_tools(ListeningOptions::default()).remove(0);
    let heard = tool
        .execute(
            serde_json::json!({"file":file.to_string_lossy(),"form":true,"tempo":120}).as_object().unwrap().clone(),
            kumi_common::abort::Signal::new(),
        )
        .await
        .unwrap();
    assert!(!heard.is_error, "{}", heard.text);
    let heard: serde_json::Value = serde_json::from_str(&heard.text).unwrap();
    assert_eq!(heard["form"]["summary"], form.summary);
}
#[tokio::test(flavor = "current_thread")]
async fn a_big_wavetable_is_made_while_kumi_keeps_running() {
    use kumi_runtime::audio::wavetable::{build_wavetable, Keyframe, WavetableSpec};
    let keyframe = Keyframe { harmonics: Some(vec![1.0; 1023]), ..Default::default() };
    let spec = WavetableSpec { keyframes: Some(vec![keyframe; 2]), count: Some(16.0), from_audio: None };
    // Kumi's thread draws the screen and answers the model meanwhile: here, a tick each millisecond.
    let (done, ticks) = (std::cell::Cell::new(false), std::cell::Cell::new(0));
    let build = async {
        let frames = build_wavetable(&spec).await;
        done.set(true);
        frames
    };
    let ticker = async {
        while !done.get() {
            tokio::time::sleep(std::time::Duration::from_millis(1)).await;
            ticks.set(ticks.get() + 1);
        }
    };
    let (frames, ()) = tokio::join!(build, ticker);
    assert_eq!(frames.unwrap().len(), 16);
    assert!(ticks.get() >= 3, "{} ticks while it was made", ticks.get());
}
#[tokio::test]
async fn a_long_compressed_file_is_converted_only_as_far_as_its_read() {
    use kumi_runtime::audio::decode::{open_audio, open_audio_to, prepare_audio_to};
    let Some(ffmpeg) = find_ffmpeg(FfmpegOptions { installed_only: true, ..Default::default() }).await.unwrap() else {
        eprintln!("ffmpeg makes the test file; unavailable");
        return;
    };
    // Ogg: ffmpeg converts it everywhere (a Mac's afconvert, which copies whole, can't). M4A: a Mac's afconvert reads it
    // too, and a read with a reach still goes to ffmpeg first.
    let root = tempfile::tempdir().unwrap();
    for (name, codec) in [("long.ogg", "libvorbis"), ("long.m4a", "aac")] {
        let file = root.path().join(name);
        let made = run(
            &ffmpeg,
            &["-v", "error", "-f", "lavfi", "-i", "sine=frequency=220:duration=40", "-c:a", codec, "-y", &file.to_string_lossy()],
            RunOptions::default(),
        )
        .await;
        if made.is_err() {
            eprintln!("this ffmpeg has no {codec} encoder; {name} unavailable");
            continue;
        }
        // Read for its first 10 s: the copy holds those (and a little), and the file is still 40 s long.
        let cut = prepare_audio_to(&file, None, Some(10.0)).await.unwrap();
        assert!((cut.seconds.unwrap() - 40.0).abs() < 0.1, "{name}: {:?}", cut.seconds);
        let copy = open_audio(&cut.path, None).await.unwrap();
        assert!((copy.frames as f64 / copy.sample_rate - 10.0).abs() < 0.1, "{name}: {}", copy.frames as f64 / copy.sample_rate);
        cut.cleanup().await;
        let mut source = open_audio_to(&file, None, Some(10.0)).await.unwrap();
        assert!((source.frames as f64 / source.sample_rate - 40.0).abs() < 0.1, "{name}");
        let mut read = 0;
        while let Some(block) = source.read(65536).await.unwrap() {
            read += block[0].len();
        }
        assert!((read as f64 / source.sample_rate - 10.0).abs() < 0.1, "{name}: reads end where the copy does");
        source.close().await.unwrap();
        // Read further than it goes: copied whole, as before.
        let whole = prepare_audio_to(&file, None, Some(60.0)).await.unwrap();
        assert_eq!(whole.seconds, None, "{name}");
        let copy = open_audio(&whole.path, None).await.unwrap();
        assert!((copy.frames as f64 / copy.sample_rate - 40.0).abs() < 0.1, "{name}");
        whole.cleanup().await;
    }
}
#[test]
fn a_near_silent_float_sound_has_an_envelope() {
    use kumi_runtime::audio::analyze::analyze_sound;
    // Every level under the 1e-9 floor, quiet and then silent: no peak to start from.
    let mono: Vec<f32> = (0..9600).map(|i| if i < 4800 { 5e-10 } else { 0.0 }).collect();
    let heard = analyze_sound(&mono, 48000.0, None);
    assert_eq!(heard.envelope.length_ms, 95.0);
    assert!(heard.pitch.is_none());
}
#[tokio::test]
async fn a_form_asked_for_in_bars_of_no_beats_or_at_a_tempo_past_the_schema_is_still_heard() {
    use kumi_runtime::audio::structure::{hear_form, FormOptions};
    let root = tempfile::tempdir().unwrap();
    let file = root.path().join("tone.wav");
    let tone: Vec<f32> = (0..12 * 48000).map(|i| (0.3 * (2.0 * std::f64::consts::PI * 220.0 * i as f64 / 48000.0).sin()) as f32).collect();
    wav(&file, &[tone.clone(), tone], 16);
    let path = file.to_str().unwrap();
    // A bar of no beats was endless bars (a capacity overflow): the schema's 1 to 16 holds.
    let none = hear_form(path, FormOptions { beats_per_bar: Some(0.0), ..Default::default() }).await.unwrap();
    assert_eq!(none.beats_per_bar, 1.0);
    let many = hear_form(path, FormOptions { beats_per_bar: Some(64.0), ..Default::default() }).await.unwrap();
    assert_eq!(many.beats_per_bar, 16.0);
    // A Set tempo past 20 to 999 BPM isn't one: the file's own, or 120, counts the bars, as without one.
    let wild = hear_form(path, FormOptions { tempo: Some(1e9), ..Default::default() }).await.unwrap();
    let without = hear_form(path, FormOptions::default()).await.unwrap();
    assert_eq!((wild.bars, &wild.tempo.bpm, &wild.at_set_tempo), (without.bars, &without.tempo.bpm, &None));
}
#[test]
fn section_edges_come_from_diagonal_novelty_snapped_to_four_bar_phrases_never_closer_than_four_bars() {
    use kumi_runtime::audio::structure::boundaries;
    let owner: Vec<_> = [7, 9, 8].iter().enumerate().flat_map(|(i, n)| std::iter::repeat_n(i, *n)).collect();
    let matrix = owner.iter().map(|a| owner.iter().map(|b| if a == b { 1.0 } else { 0.2 }).collect()).collect::<Vec<_>>();
    assert_eq!(boundaries(&matrix, owner.len()), vec![8, 16]);
    assert!(boundaries(&vec![vec![1.0; owner.len()]; owner.len()], owner.len()).is_empty());
}
#[test]
fn transcribed_notes_reach_the_model_as_rows_in_beats_at_the_sets_tempo_as_played() {
    use kumi_runtime::audio::{analyze::HeardNote, tools::transcription};
    let rows = transcription(
        &[
            HeardNote { time: 0.52, duration: 0.24, midi: Some(60.0), velocity: 100.0, confidence: 0.9 },
            HeardNote { time: 1.01, duration: 0.1, midi: None, velocity: 80.0, confidence: 0.0 },
            HeardNote { time: 1.49, duration: 0.5, midi: Some(60.0), velocity: 90.0, confidence: 0.9 },
        ],
        Some(120.0),
    );
    assert_eq!(kumi_common::js::json::stringify(&rows["rows"]), "[[1.04,60,100,0.48],[2.02,null,80,0.2],[2.98,60,90,1]]");
    assert_eq!(rows["pitched"], 2);
    assert_eq!(rows["unpitched"], 1);
    assert_eq!(kumi_common::js::json::stringify(&rows["mostPlayed"]), "[{\"midi\":60,\"count\":2}]");
    assert!(rows["unit"].as_str().unwrap().contains("beats at 120 BPM"));
}
#[tokio::test]
async fn the_listen_tool_hears_a_file_or_sets_it_against_a_reference_with_loudness_matched_and_tells_the_app() {
    use kumi_runtime::audio::tools::*;
    use std::{cell::RefCell, rc::Rc};
    let root = tempfile::tempdir().unwrap();
    let events = Rc::new(RefCell::new(Vec::new()));
    let told = events.clone();
    let tool = listening_tools(ListeningOptions { on_event: Rc::new(move |e| told.borrow_mut().push(e)), ..Default::default() }).remove(0);
    let bright: Vec<_> = deterministic_noise(8 * 48000).iter().map(|v| (*v as f64 / 0.3 * 0.25) as f32).collect();
    let low = tone(8 * 48000, 350.0, 0.3);
    let mix: Vec<_> = bright.iter().zip(low).map(|(v, l)| ((*v as f64 + l as f64) * 0.25) as f32).collect();
    let mine = root.path().join("mine.wav");
    let reference = root.path().join("reference.wav");
    wav(&mine, &[mix.clone(), mix], 16);
    wav(&reference, &[bright.clone(), bright.clone()], 16);
    let call = |v: serde_json::Value| {
        let tool = tool.clone();
        async move { tool.execute(v.as_object().unwrap().clone(), kumi_common::abort::Signal::new()).await.unwrap() }
    };
    let alone = call(serde_json::json!({"file":mine,"focus":"mix"})).await;
    assert!(!alone.is_error, "{}", alone.text);
    let alone: serde_json::Value = serde_json::from_str(&alone.text).unwrap();
    assert_eq!(alone["kumiAudio"], 1);
    assert!(alone["loudness"]["integratedLufs"].as_f64().unwrap() < -10.0);
    let result = call(serde_json::json!({"file":mine,"compare_to":reference,"focus":"mix"})).await;
    assert!(!result.is_error, "{}", result.text);
    let body: serde_json::Value = serde_json::from_str(&result.text).unwrap();
    assert!(
        body["comparison"]["balance"].as_array().unwrap().iter().find(|b| b["band"] == "low mids").unwrap()["difference"].as_f64().unwrap()
            > 3.0
    );
    let headlines = body["comparison"]["headlines"].as_array().unwrap().iter().map(|v| v.as_str().unwrap()).collect::<Vec<_>>().join(" | ");
    assert!(regex::Regex::new(r"low mids \(250–500 Hz\) \+[0-9]+\.[0-9] dB over the reference").unwrap().is_match(&headlines));
    assert!(headlines.contains("LU quieter overall"));
    let short = root.path().join("short-reference.wav");
    wav(&short, &[bright[..3 * 48000].to_vec(), bright[..3 * 48000].to_vec()], 16);
    let later = call(serde_json::json!({"file":mine,"compare_to":short,"focus":"mix","from_seconds":5})).await;
    assert!(!later.is_error, "{}", later.text);
    assert!(serde_json::from_str::<serde_json::Value>(&later.text).unwrap().get("comparison").is_some());
    assert_eq!(events.borrow().len(), 3);
    let events = events.borrow();
    let compared = events[1].compared.as_ref().unwrap();
    assert_eq!(compared.reference, "reference.wav");
    assert_eq!(compared.differences.len(), 10);
    drop(events);
    let missing = call(serde_json::json!({"file":root.path().join("nowhere.wav")})).await;
    assert!(missing.is_error && missing.text.contains("no file there"));
}
