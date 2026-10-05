//! Video watching, with complete-result caption and moment oracles.
use kumi_common::{abort::Signal, js::json::stringify};
use kumi_runtime::video::{
    captions::{said_around, TranscriptOptions},
    moments::MomentOptions,
    programs::{run, FfmpegOptions, RunOptions},
    speech::{cues_from_whisper, speech_model_for},
    *,
};
use serde_json::{json, Value};
use std::{cell::RefCell, path::Path, rc::Rc};
#[test]
fn times_read_as_a_producer_writes_them_and_are_said_back_the_same_way() {
    for (input, expected) in
        [(json!("2:05"), 125.0), (json!("1:02:03"), 3723.0), (json!("90"), 90.0), (json!(12.5), 12.5), (json!("0:07.5"), 7.5)]
    {
        assert_eq!(parse_time(&input), Some(expected));
    }
    for input in [json!(""), json!("soon"), json!("2:75:00x"), json!(-1), Value::Null, json!({})] {
        assert_eq!(parse_time(&input), None);
    }
    assert_eq!(format_time(125.0), "2:05");
    assert_eq!(format_time(3723.0), "1:02:03");
    assert_eq!(format_time(-3.0), "0:00");
}
#[test]
fn youtube_addresses_name_the_video_and_stream_addresses_must_be_public() {
    for url in [
        "https://www.youtube.com/watch?v=W87uuuGcq9c",
        "https://youtu.be/W87uuuGcq9c?t=30",
        "https://www.youtube.com/watch?list=x&v=W87uuuGcq9c",
        "https://www.youtube.com/shorts/W87uuuGcq9c",
    ] {
        assert_eq!(youtube_id(url).as_deref(), Some("W87uuuGcq9c"));
    }
    assert_eq!(youtube_id("https://vimeo.com/12345"), None);
    for url in [
        "https://rr3---sn-abc.googlevideo.com/videoplayback?x=1",
        "https://www.youtube.com/api/timedtext?v=x",
        "http://8.8.8.8/a",
        "https://[2001:4860::8888]/a",
    ] {
        assert!(public_address(url), "{url}");
    }
    for url in [
        "file:///etc/passwd",
        "http://localhost:8080/",
        "http://127.0.0.1/",
        "http://10.0.0.5/",
        "http://192.168.1.1/",
        "http://172.20.0.1/",
        "http://169.254.169.254/latest",
        "http://[::1]/",
        "http://[fd00::1]/",
        "http://router.local/",
        "http://intranet/",
        "concat:a|b",
        "ftp://example.com/x",
        "http://0.0.0.0/",
    ] {
        assert!(!public_address(url), "{url}");
    }
}
#[test]
fn captions_transcripts_and_selected_moments_match_source_results() {
    let reference: Value = serde_json::from_str(include_str!("support/video/reference.json")).unwrap();
    for c in reference["captions"].as_array().unwrap() {
        assert_eq!(
            stringify(&serde_json::to_value(parse_captions(c["text"].as_str().unwrap(), c["format"].as_str().unwrap())).unwrap()),
            stringify(&c["expected"])
        );
    }
    for c in reference["transcripts"].as_array().unwrap() {
        let cues: Vec<Cue> = serde_json::from_value(c["cues"].clone()).unwrap();
        let options = &c["options"];
        let lines = transcript_lines(
            &cues,
            TranscriptOptions {
                from: options["from"].as_f64(),
                to: options["to"].as_f64(),
                chars: options["chars"].as_u64().map(|v| v as usize),
            },
        );
        assert_eq!(stringify(&serde_json::to_value(lines).unwrap()), stringify(&c["expected"]));
        assert_eq!(said_around(&cues, 7.0, None), "then a Saturator with the drive up");
    }
    for c in reference["moments"].as_array().unwrap() {
        let options = &c["options"];
        let chosen = choose_moments(
            &serde_json::from_value::<Vec<Cue>>(c["cues"].clone()).unwrap(),
            MomentOptions {
                from: options["from"].as_f64().unwrap(),
                to: options["to"].as_f64().unwrap(),
                count: options["count"].as_f64().unwrap(),
                chapters: options.get("chapters").map(|v| serde_json::from_value(v.clone()).unwrap()).unwrap_or_default(),
            },
        );
        assert_eq!(stringify(&json!(chosen)), stringify(&c["expected"]));
    }
}
#[test]
fn whisper_output_becomes_timed_lines_without_music_or_silence() {
    let cues=cues_from_whisper(&json!({"transcription":[{"offsets":{"from":0,"to":6480},"text":" Load Operator.  Set voices to 1,"},{"offsets":{"from":6480,"to":9000},"text":" [MUSIC]"},{"offsets":{"from":9000,"to":12000},"text":" (upbeat music)"},{"offsets":{"from":12000},"text":"no end"}]}).to_string());
    assert_eq!(cues, vec![Cue { start: 0.0, end: 6.48, text: "Load Operator. Set voices to 1,".into() }]);
    assert!(cues_from_whisper("{").is_empty());
    assert_eq!(speech_model_for(Some("en")), "ggml-small.en-q5_1.bin");
    assert_eq!(speech_model_for(None), "ggml-small.en-q5_1.bin");
    assert_eq!(speech_model_for(Some("de")), "ggml-small-q5_1.bin");
}
async fn test_video(folder: &Path, name: &str, captions: bool) -> Option<String> {
    let ffmpeg = find_ffmpeg(FfmpegOptions { installed_only: true, ..Default::default() }).await.unwrap()?;
    let file = folder.join(format!("{name}.mp4")).to_string_lossy().into_owned();
    run(
        &ffmpeg,
        &[
            "-hide_banner",
            "-loglevel",
            "error",
            "-f",
            "lavfi",
            "-i",
            "testsrc2=size=640x360:rate=10:duration=12",
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=110:duration=12",
            "-c:v",
            "mpeg4",
            "-c:a",
            "aac",
            "-shortest",
            "-y",
            &file,
        ],
        RunOptions::default(),
    )
    .await
    .unwrap();
    if captions {
        std::fs::write(folder.join(format!("{name}.srt")),"1\n00:00:01,000 --> 00:00:03,000\nload Operator and set the coarse to 1\n\n2\n00:00:06,000 --> 00:00:08,000\nthen a Saturator, drive all the way, like this\n").unwrap();
    }
    Some(file)
}
fn watch_options(folder: &Path, sub: &str) -> WatchOptions {
    WatchOptions {
        videos_dir: folder.join(sub).to_string_lossy().into(),
        tools_dir: folder.join("tools").to_string_lossy().into(),
        ..Default::default()
    }
}
#[tokio::test(flavor = "current_thread")]
async fn video_files_have_captions_frames_closeups_sound_and_are_kept() {
    let folder = tempfile::tempdir().unwrap();
    let Some(video) = test_video(folder.path(), "tutorial", true).await else {
        eprintln!("ffmpeg makes the test video; unavailable");
        return;
    };
    let mut options = watch_options(folder.path(), "videos");
    let progress = Rc::new(RefCell::new(Vec::new()));
    options.on_progress = Some(Rc::new({
        let progress = progress.clone();
        move |text| progress.borrow_mut().push(text.to_string())
    }));
    let watched = watch_video(
        WatchRequest { url: video.clone(), frames: Some(3.0), listen: Some(SoundSpan { from: 2.0, to: 4.0 }), ..Default::default() },
        options.clone(),
    )
    .await
    .unwrap();
    assert_eq!(watched.title, "tutorial");
    assert!((watched.duration.unwrap_or(0.0) - 12.0).abs() < 0.5);
    assert_eq!(watched.words, Some(Words { language: String::new(), source: kumi_runtime::core::contracts::WordsSource::Captions }));
    assert_eq!(watched.lines.iter().map(|l| l.at).collect::<Vec<_>>(), [1.0, 6.0]);
    assert_eq!(watched.frames.len(), 3, "{:?}", watched.notes);
    for frame in &watched.frames {
        assert_eq!(&frame.jpeg[..2], [0xff, 0xd8]);
        assert_eq!((frame.thumb.width, frame.thumb.height, frame.thumb.rgb.len()), (32, 18, 32 * 18 * 3));
    }
    assert!(watched.frames.iter().any(|f| f.said.contains("Saturator")));
    let sound = watched.sound.unwrap();
    assert!(Path::new(&sound.file).exists());
    assert_eq!((sound.from, sound.to), (2.0, 4.0));
    assert!(progress.borrow().iter().any(|t| t.starts_with("looking at")));
    let kept = std::fs::read_dir(&options.videos_dir).unwrap().next().unwrap().unwrap().file_name().to_string_lossy().into_owned();
    assert!(regex::Regex::new("^file-[0-9a-f]{16}$").unwrap().is_match(&kept));
    let again = watch_video(
        WatchRequest { url: video.clone(), look_at: Some(vec![7.0]), zoom: Some(Region::Bottom), frames: Some(0.0), ..Default::default() },
        options.clone(),
    )
    .await
    .unwrap();
    assert_eq!(again.lines.len(), 2);
    assert_eq!(again.frames.len(), 1, "{:?}", again.notes);
    assert_eq!(again.frames[0].region, Some(Region::Bottom));
    assert_eq!((again.frames[0].thumb.width, again.frames[0].thumb.height), (32, 8));
    assert!(Path::new(&options.videos_dir).join(&kept).join("frames/7.0-bottom.jpg").exists());
    let range =
        watch_video(WatchRequest { url: video, from: Some(5.0), to: Some(9.0), frames: Some(0.0), ..Default::default() }, options.clone())
            .await
            .unwrap();
    assert_eq!((range.from, range.to, range.lines.iter().map(|l| l.at).collect::<Vec<_>>()), (5.0, 9.0, vec![6.0]));
    assert!(watch_video(
        WatchRequest { url: folder.path().join("missing.mp4").to_string_lossy().into(), ..Default::default() },
        options.clone()
    )
    .await
    .unwrap_err()
    .to_string()
    .contains("no video there"));
    std::fs::write(folder.path().join("notes.txt"), "x").unwrap();
    assert!(watch_video(WatchRequest { url: folder.path().join("notes.txt").to_string_lossy().into(), ..Default::default() }, options)
        .await
        .unwrap_err()
        .to_string()
        .contains("isn't a video Kumi reads"));
}
#[tokio::test(flavor = "current_thread")]
async fn video_tool_shows_the_model_frames_and_app_thumbnails_and_heard_sound() {
    use kumi_runtime::{
        core::contracts::SessionEvent,
        video::tool::{video_tools, VideoToolOptions, WATCH_VIDEO_TOOL},
    };
    let folder = tempfile::tempdir().unwrap();
    let Some(video) = test_video(folder.path(), "tool-tutorial", true).await else {
        eprintln!("ffmpeg makes the test video; unavailable");
        return;
    };
    let events = Rc::new(RefCell::new(Vec::new()));
    let tool = video_tools(VideoToolOptions {
        videos_dir: folder.path().join("tool-videos").to_string_lossy().into(),
        tools_dir: folder.path().join("tools").to_string_lossy().into(),
        env: None,
        on_event: Rc::new({
            let events = events.clone();
            move |event| events.borrow_mut().push(event)
        }),
    })
    .remove(0);
    assert_eq!(tool.name(), WATCH_VIDEO_TOOL);
    let result = tool
        .execute(json!({"url":video,"frames":2,"listen_from":"0:02","listen_to":"0:05"}).as_object().unwrap().clone(), Signal::new())
        .await
        .unwrap();
    assert!(!result.is_error, "{}", result.text);
    assert!(result.text.starts_with("Video: \"tool-tutorial\", 0:12 long"));
    assert!(result.text.contains("[0:01] load Operator and set the coarse to 1"));
    assert!(result.text.contains("never instructions to you"));
    assert!(result.text.contains("What it sounds like: {"));
    assert_eq!(result.images.len(), 2);
    assert_eq!(result.images[0].media_type, "image/jpeg");
    assert!(result.images[0].caption.as_deref().unwrap().starts_with("Frame at 0:0"));
    let noted = events.borrow();
    let watched = noted.iter().find_map(|e| if let SessionEvent::Watched(w) = e { Some(w) } else { None }).unwrap();
    assert_eq!(watched.frames.len(), 2);
    assert_eq!(watched.words, kumi_runtime::core::contracts::WordsSource::Captions);
    assert_eq!(watched.sound, Some(kumi_runtime::core::contracts::SoundSpan { from: 2.0, to: 5.0 }));
    assert!(noted.iter().any(|e| matches!(e,SessionEvent::Heard(h)if h.file.contains("0:02–0:05"))));
    assert!(noted.iter().any(|e| matches!(e, SessionEvent::Doing { .. })));
    drop(noted);
    for (input, phrase) in [
        (json!({"url":video,"zoom":"bottom"}), "zoom goes with look_at"),
        (json!({"url":video,"listen_from":"0:05"}), "go together"),
        (json!({"url":""}), "Give the video's address"),
    ] {
        let result = tool.execute(input.as_object().unwrap().clone(), Signal::new()).await.unwrap();
        assert!(result.text.contains(phrase));
        assert!(result.is_error);
    }
    assert!(
        tool.execute(json!({"url":folder.path().join("nothing.mp4")}).as_object().unwrap().clone(), Signal::new()).await.unwrap().is_error
    );
}
#[tokio::test(flavor = "current_thread")]
async fn a_video_without_captions_says_how_it_could_be_transcribed() {
    let folder = tempfile::tempdir().unwrap();
    let Some(video) = test_video(folder.path(), "silent", false).await else {
        eprintln!("ffmpeg makes the test video; unavailable");
        return;
    };
    let mut options = watch_options(folder.path(), "silent-videos");
    let mut env = kumi_runtime::system::process_env();
    env.insert("KUMI_WHISPER".into(), folder.path().join("no-whisper").to_string_lossy().into());
    options.env = Some(env);
    let watched = watch_video(WatchRequest { url: video, frames: Some(2.0), ..Default::default() }, options).await.unwrap();
    assert!(watched.lines.is_empty());
    assert_eq!(watched.frames.len(), 2, "{:?}", watched.notes);
    assert!(watched.notes.join(" ").contains("no captions beside it"));
    assert!(watched.notes.join(" ").contains("whisper.cpp"));
}
#[test]
fn frames_kumi_couldnt_take_are_said_once_for_each_reason() {
    let refused = "Server returned 403 Forbidden (access denied)".to_string();
    assert!(missed_frames(&[], 0).is_empty());
    assert_eq!(
        missed_frames(&[(70.0, refused.clone())], 0),
        ["Kumi couldn't take the frame at 1:10 (Server returned 403 Forbidden (access denied))."]
    );
    assert_eq!(
        missed_frames(&[(247.0, refused.clone()), (99.0, refused.clone()), (20.0, refused.clone())], 9),
        ["Kumi couldn't take the frames at 0:20, 1:39 and 4:07 (Server returned 403 Forbidden (access denied)), so it didn't try the other 9."]
    );
    assert_eq!(
        missed_frames(&[(30.0, "timed out".into()), (5.0, refused), (10.0, "timed out".into())], 2),
        [
            "Kumi couldn't take the frames at 0:10 and 0:30 (timed out).",
            "Kumi couldn't take the frame at 0:05 (Server returned 403 Forbidden (access denied)), so it didn't try the other 2."
        ]
    );
}
#[tokio::test(flavor = "current_thread")]
async fn a_video_that_gives_no_frames_is_said_once_and_the_rest_are_not_tried() {
    let folder = tempfile::tempdir().unwrap();
    if find_ffmpeg(FfmpegOptions { installed_only: true, ..Default::default() }).await.unwrap().is_none() {
        eprintln!("ffmpeg takes the frames; unavailable");
        return;
    }
    let video = folder.path().join("broken.mp4");
    std::fs::write(&video, "not a video").unwrap();
    std::fs::write(folder.path().join("broken.srt"), "1\n00:00:01,000 --> 00:00:08,000\nload Operator\n").unwrap();
    let watched = watch_video(
        WatchRequest { url: video.to_string_lossy().into(), look_at: Some(vec![1.0, 2.0, 3.0, 4.0, 5.0, 6.0]), ..Default::default() },
        watch_options(folder.path(), "videos"),
    )
    .await
    .unwrap();
    assert!(watched.frames.is_empty());
    assert_eq!(watched.notes.len(), 1, "{:?}", watched.notes);
    assert!(watched.notes[0].starts_with("Kumi couldn't take the frames at 0:01, 0:02 and 0:03 ("), "{}", watched.notes[0]);
    assert!(watched.notes[0].ends_with("), so it didn't try the other 3."), "{}", watched.notes[0]);
}
/// Serves a file the way YouTube serves its streams: a request for more than a piece of it is refused.
async fn piece_server(body: Vec<u8>, piece: u64) -> String {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let address = format!("http://{}/video.mp4", listener.local_addr().unwrap());
    let body = std::sync::Arc::new(body);
    tokio::spawn(async move {
        while let Ok((mut socket, _)) = listener.accept().await {
            let body = body.clone();
            tokio::spawn(async move {
                let mut buffer = Vec::new();
                let mut read = [0u8; 4096];
                loop {
                    let request = loop {
                        if let Some(end) = buffer.windows(4).position(|w| w == b"\r\n\r\n") {
                            let request = String::from_utf8_lossy(&buffer[..end]).to_ascii_lowercase();
                            buffer.drain(..end + 4);
                            break request;
                        }
                        match socket.read(&mut read).await {
                            Ok(0) | Err(_) => return,
                            Ok(count) => buffer.extend_from_slice(&read[..count]),
                        }
                    };
                    let size = body.len() as u64;
                    let range = request.lines().find_map(|line| line.strip_prefix("range: bytes=")?.split_once('-'));
                    let bounds = range.and_then(|(start, end)| Some((start.trim().parse::<u64>().ok()?, end.trim().parse::<u64>().ok()?)));
                    let reply = match bounds {
                        Some((start, end)) if start < size && start <= end && end - start < piece => {
                            let end = end.min(size - 1);
                            let mut reply = format!(
                                "HTTP/1.1 206 Partial Content\r\nContent-Type: video/mp4\r\nAccept-Ranges: bytes\r\nContent-Range: bytes {start}-{end}/{size}\r\nContent-Length: {}\r\n\r\n",
                                end - start + 1
                            )
                            .into_bytes();
                            reply.extend_from_slice(&body[start as usize..=end as usize]);
                            reply
                        }
                        _ => b"HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\n\r\n".to_vec(),
                    };
                    if socket.write_all(&reply).await.is_err() {
                        return;
                    }
                }
            });
        }
    });
    address
}
#[tokio::test(flavor = "current_thread")]
async fn a_stream_its_site_wants_in_pieces_is_asked_for_in_pieces() {
    use kumi_runtime::video::frames::{frame_at, sound_between, Input};
    let folder = tempfile::tempdir().unwrap();
    let Some(video) = test_video(folder.path(), "pieces", false).await else {
        eprintln!("ffmpeg makes the test video; unavailable");
        return;
    };
    let ffmpeg = find_ffmpeg(FfmpegOptions { installed_only: true, ..Default::default() }).await.unwrap().unwrap();
    if !programs::ffmpeg_reads_in_pieces(&ffmpeg, None).await.unwrap() {
        eprintln!("this ffmpeg asks for a stream whole (before 8.1); unavailable");
        return;
    }
    const PIECE: u64 = 64 * 1024;
    let url = piece_server(std::fs::read(&video).unwrap(), PIECE).await;
    let path = |name: &str| folder.path().join(name).to_string_lossy().into_owned();
    let whole = Input { url: url.clone(), headers: None, piece: None };
    let error = frame_at(&ffmpeg, Some(&whole), 7.0, &path("whole.jpg"), None, None).await.unwrap_err();
    assert!(error.to_string().contains("403"), "{error}");
    let pieces = Input { url, headers: None, piece: Some(PIECE) };
    let frame = frame_at(&ffmpeg, Some(&pieces), 7.0, &path("pieces.jpg"), None, None).await.unwrap();
    assert_eq!(&frame.jpeg[..2], [0xff, 0xd8]);
    let sound = sound_between(&ffmpeg, &pieces, 2.0, 4.0, &path("pieces.wav"), None, false).await.unwrap();
    assert!(std::fs::metadata(sound).unwrap().len() > 44_100 * 2 * 2);
}

/// A program that writes its runs to `runs` beside it, then does `then`.
#[cfg(unix)]
fn program(folder: &Path, name: &str, then: &str) -> String {
    use std::os::unix::fs::PermissionsExt;
    let path = folder.join(name);
    std::fs::write(&path, format!("#!/bin/sh\necho {name} >> \"$(dirname \"$0\")/runs\"\n{then}\n")).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
    path.to_string_lossy().into()
}
#[cfg(unix)]
fn runs(folder: &Path, name: &str) -> usize {
    std::fs::read_to_string(folder.join("runs")).unwrap_or_default().lines().filter(|line| *line == name).count()
}

#[cfg(unix)]
#[tokio::test(flavor = "current_thread")]
async fn speech_that_couldnt_be_transcribed_is_tried_again_only_in_the_next_request() {
    let folder = tempfile::tempdir().unwrap();
    let Some(video) = test_video(folder.path(), "unheard", false).await else {
        eprintln!("ffmpeg makes the test video; unavailable");
        return;
    };
    let whisper = program(folder.path(), "whisper", "echo 'error: the model ran out of memory' >&2\nexit 1");
    std::fs::write(folder.path().join("model.bin"), "").unwrap();
    let mut env = kumi_runtime::system::process_env();
    env.insert("KUMI_WHISPER".into(), whisper);
    env.insert("KUMI_WHISPER_MODEL".into(), folder.path().join("model.bin").to_string_lossy().into());
    let mut options = watch_options(folder.path(), "unheard-videos");
    options.env = Some(env);
    // A turn gives each of its tools its own signal, copied.
    options.signal = Some(kumi_common::abort::any([Signal::new()]));
    let first =
        watch_video(WatchRequest { url: video.clone(), look_at: Some(vec![1.0]), ..Default::default() }, options.clone()).await.unwrap();
    assert_eq!(runs(folder.path(), "whisper"), 1);
    assert!(
        first.notes.contains(&"Kumi couldn't transcribe the video's speech (error: the model ran out of memory).".into()),
        "{:?}",
        first.notes
    );
    assert_eq!(first.frames.len(), 1, "{:?}", first.notes);
    // Another request runs between the two looks, as tests running together do: it hasn't seen the
    // failure, and it doesn't make the first request forget it.
    let turn = options.signal.replace(kumi_common::abort::any([Signal::new()]));
    watch_video(WatchRequest { url: video.clone(), look_at: Some(vec![1.0]), ..Default::default() }, options.clone()).await.unwrap();
    assert_eq!(runs(folder.path(), "whisper"), 2);
    options.signal = turn;
    let again =
        watch_video(WatchRequest { url: video.clone(), look_at: Some(vec![2.0]), ..Default::default() }, options.clone()).await.unwrap();
    assert_eq!(runs(folder.path(), "whisper"), 2, "a closer look in the same request doesn't wait for the same failure");
    assert_eq!(
        again.notes,
        vec!["Kumi couldn't transcribe the video's speech earlier in this request (error: the model ran out of memory), so it didn't try again; it will on the next request."]
    );
    assert_eq!(again.frames.len(), 1, "{:?}", again.notes);
    options.signal = Some(kumi_common::abort::any([Signal::new()]));
    let next = watch_video(WatchRequest { url: video, look_at: Some(vec![2.0]), ..Default::default() }, options).await.unwrap();
    assert_eq!(runs(folder.path(), "whisper"), 3, "the next request tries again");
    assert!(
        next.notes.contains(&"Kumi couldn't transcribe the video's speech (error: the model ran out of memory).".into()),
        "{:?}",
        next.notes
    );
}

#[cfg(unix)]
#[tokio::test(flavor = "current_thread")]
async fn speech_that_couldnt_be_taken_leaves_a_note_and_the_frames() {
    let folder = tempfile::tempdir().unwrap();
    let Some(video) = test_video(folder.path(), "stalled", false).await else {
        eprintln!("ffmpeg makes the test video; unavailable");
        return;
    };
    let real = find_ffmpeg(FfmpegOptions { installed_only: true, ..Default::default() }).await.unwrap().unwrap();
    // ffmpeg that stalls taking the sound (-vn) and takes frames as ever, with ffprobe beside it.
    let tools = folder.path().join("stalling");
    std::fs::create_dir_all(&tools).unwrap();
    let ffmpeg = program(
        &tools,
        "ffmpeg",
        &format!("for arg in \"$@\"; do if [ \"$arg\" = -vn ]; then echo sound >> \"$(dirname \"$0\")/runs\"; echo 'Connection timed out' >&2; exit 1; fi; done\nexec '{real}' \"$@\""),
    );
    let probe = Path::new(&real).with_file_name("ffprobe");
    if probe.exists() {
        std::os::unix::fs::symlink(probe, tools.join("ffprobe")).unwrap();
    }
    let whisper = program(folder.path(), "whisper", "exit 1");
    std::fs::write(folder.path().join("model.bin"), "").unwrap();
    let mut env = kumi_runtime::system::process_env();
    env.insert("KUMI_FFMPEG".into(), ffmpeg);
    env.insert("KUMI_WHISPER".into(), whisper);
    env.insert("KUMI_WHISPER_MODEL".into(), folder.path().join("model.bin").to_string_lossy().into());
    let mut options = watch_options(folder.path(), "stalled-videos");
    options.env = Some(env);
    options.signal = Some(kumi_common::abort::any([Signal::new()]));
    let watched =
        watch_video(WatchRequest { url: video.clone(), look_at: Some(vec![1.0]), ..Default::default() }, options.clone()).await.unwrap();
    assert_eq!((runs(&tools, "sound"), runs(folder.path(), "whisper")), (1, 0));
    assert_eq!(watched.notes, vec!["Kumi couldn't transcribe the video's speech (Connection timed out)."]);
    assert_eq!(watched.frames.len(), 1, "the frames still come");
    let again = watch_video(WatchRequest { url: video, look_at: Some(vec![2.0]), ..Default::default() }, options).await.unwrap();
    assert_eq!(runs(&tools, "sound"), 1, "a stalled stream is remembered for the request, like a failed transcription");
    assert_eq!(again.frames.len(), 1);
}

#[cfg(unix)]
#[tokio::test(flavor = "current_thread")]
async fn speech_process_receives_options_reports_progress_and_cleans_up_after_success_timeout_and_abort() {
    use kumi_runtime::video::speech::{transcribe, TranscribeOptions};
    use std::os::unix::fs::PermissionsExt;
    let folder = tempfile::tempdir().unwrap();
    let command = folder.path().join("whisper");
    std::fs::write(
        &command,
        r#"#!/bin/sh
printf '%s\n' "$@" > "$(dirname "$0")/args"
while [ "$#" -gt 0 ]; do
  if [ "$1" = '-of' ]; then shift; out="$1"; fi
  shift
done
printf '%s' '{"transcription":[{"offsets":{"from":0,"to":500},"text":" hello "}]}' > "$out.json"
printf 'progress = 12%%\nprogress = 100%%\n' >&2
if [ -f "$(dirname "$0")/sleep" ]; then exec sleep 10; fi
"#,
    )
    .unwrap();
    std::fs::set_permissions(&command, std::fs::Permissions::from_mode(0o700)).unwrap();
    let wav = folder.path().join("voice.wav");
    let progress = Rc::new(RefCell::new(Vec::new()));
    let options = TranscribeOptions {
        language: Some("de-DE".into()),
        prompt: Some("Ableton".into()),
        audio_context: Some(96.4),
        vad: Some("vad.bin".into()),
        on_progress: Some(Rc::new({
            let progress = progress.clone();
            move |p| progress.borrow_mut().push(p)
        })),
        ..Default::default()
    };
    let cues = transcribe(command.to_str().unwrap(), "model.bin", wav.to_str().unwrap(), options).await.unwrap();
    assert_eq!(cues, vec![Cue { start: 0.0, end: 0.5, text: "hello".into() }]);
    assert_eq!(*progress.borrow(), vec![12.0, 100.0]);
    let args = std::fs::read_to_string(folder.path().join("args")).unwrap();
    assert!(args.contains("-l\nde\n--prompt\nAbleton\n-ac\n96\n--vad\n-vm\nvad.bin\n"), "{args}");
    assert!(args.contains("\n-sns\n-bs\n1\n-l\n"), "{args}");
    assert!(!args.contains("\n-t\n"), "whisper.cpp keeps its own threads, leaving Live the rest: {args}");
    let leftovers = || {
        std::fs::read_dir(folder.path()).unwrap().flatten().filter(|f| f.file_name().to_string_lossy().starts_with(".transcript-")).count()
    };
    assert_eq!(leftovers(), 0);
    std::fs::write(folder.path().join("sleep"), "").unwrap();
    let error = transcribe(
        command.to_str().unwrap(),
        "model.en.bin",
        wav.to_str().unwrap(),
        // Long enough for a loaded runner to reach the progress lines before the sleep.
        TranscribeOptions { timeout_ms: Some(500), ..Default::default() },
    )
    .await
    .unwrap_err();
    assert_eq!(error.to_string(), "progress = 100%");
    assert_eq!(leftovers(), 0);
    let signal = kumi_common::abort::timeout(50);
    assert!(matches!(
        transcribe(
            command.to_str().unwrap(),
            "model.bin",
            wav.to_str().unwrap(),
            TranscribeOptions { signal: Some(signal), ..Default::default() }
        )
        .await,
        Err(VideoFailure::Aborted)
    ));
    assert_eq!(leftovers(), 0);
}

#[cfg(unix)]
#[test]
fn youtube_uses_the_retained_installer_runtime_with_a_restricted_path() {
    use std::{fs, os::unix::fs::PermissionsExt, process::Command};
    let folder = tempfile::tempdir().unwrap();
    let kumi = folder.path().join("existing Kumi home");
    let node = kumi.join("node/bin/node");
    fs::create_dir_all(node.parent().unwrap()).unwrap();
    fs::write(&node, "#!/bin/sh\nexit 0\n").unwrap();
    fs::set_permissions(&node, fs::Permissions::from_mode(0o755)).unwrap();
    let ytdlp = folder.path().join("yt-dlp fixture");
    fs::write(&ytdlp, "#!/bin/sh\nprintf '2025.11.12\\n'\n").unwrap();
    fs::set_permissions(&ytdlp, fs::Permissions::from_mode(0o755)).unwrap();
    let result = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "youtube_javascript_runtime_child", "--nocapture"])
        .env("KUMI_TEST_YTDLP", &ytdlp)
        .env("KUMI_HOME", &kumi)
        .env("PATH", "/nonexistent")
        .output()
        .unwrap();
    assert!(result.status.success(), "{}\n{}", String::from_utf8_lossy(&result.stdout), String::from_utf8_lossy(&result.stderr));
}

#[cfg(unix)]
#[tokio::test]
async fn youtube_javascript_runtime_child() {
    let Ok(ytdlp) = std::env::var("KUMI_TEST_YTDLP") else { return };
    let expected = format!("node:{}/node/bin/node", std::env::var("KUMI_HOME").unwrap());
    assert_eq!(programs::yt_dlp_extras(&ytdlp, None).await, vec!["--js-runtimes".to_string(), expected]);
    // The source memoizes each executable probe, including the selected runtime.
    std::fs::remove_file(&ytdlp).unwrap();
    assert_eq!(programs::yt_dlp_extras(&ytdlp, None).await.len(), 2);
}
