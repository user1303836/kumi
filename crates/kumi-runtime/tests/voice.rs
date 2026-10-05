//! Voice input, with meter oracles and process failure cases.
use kumi_runtime::{
    system::{self, Env},
    video::programs::*,
    voice::{
        microphone::{microphone_input, start_capture, Meter},
        *,
    },
};
use serde_json::{json, Value};
use std::{
    path::Path,
    time::{Duration, Instant},
};
fn voice(seconds: f64, words: &[(f64, f64)]) -> Vec<i16> {
    let rate = 48000.0;
    let mut x = 3.0;
    let mut out = vec![0f32; (seconds * rate).round() as usize];
    for sample in &mut out {
        x = (x * 1103515245.0 + 12345.0) % 2147483648.0;
        *sample = (0.0015 * (x / 1073741824.0 - 1.0)) as f32;
    }
    for &(from, to) in words {
        for i in (from * rate).round() as usize..((to * rate).round() as usize).min(out.len()) {
            out[i] = (out[i] as f64 + 0.25 * (2.0 * ((i as f64 * 180.0 / rate) % 1.0) - 1.0)) as f32;
        }
    }
    out.iter().step_by(3).map(|s| kumi_common::js::number::round(*s as f64 * 32767.0) as i16).collect()
}
fn pcm(samples: &[i16]) -> Vec<u8> {
    samples.iter().flat_map(|s| s.to_le_bytes()).collect()
}
#[test]
fn microphone_inputs_and_device_listings() {
    assert_eq!(microphone_input("darwin", None), ["-f", "avfoundation", "-i", ":default"]);
    assert_eq!(microphone_input("darwin", Some("Scarlett 2i2 USB")), ["-f", "avfoundation", "-i", ":Scarlett 2i2 USB"]);
    assert_eq!(
        microphone_input("win32", Some("Microphone (Realtek(R) Audio)")),
        ["-f", "dshow", "-audio_buffer_size", "50", "-i", "audio=Microphone (Realtek(R) Audio)"]
    );
    assert_eq!(microphone_input("linux", None), ["-f", "pulse", "-i", "default"]);
    let mac="[AVFoundation indev @ 0x7f8] AVFoundation video devices:\n[AVFoundation indev @ 0x7f8] [0] FaceTime HD Camera\n[AVFoundation indev @ 0x7f8] [1] Capture screen 0\n[AVFoundation indev @ 0x7f8] AVFoundation audio devices:\n[AVFoundation indev @ 0x7f8] [0] MacBook Pro Microphone\n[AVFoundation indev @ 0x7f8] [1] Scarlett 2i2 USB\n[in#0 @ 0x600] Error opening input: Input/output error";
    assert_eq!(parse_microphones(mac, "darwin"), ["MacBook Pro Microphone", "Scarlett 2i2 USB"]);
    assert!(parse_microphones(mac, "linux").is_empty());
    assert_eq!(parse_microphones("[dshow @ 000001] \"Integrated Camera\" (video)\r\n[dshow @ 000001] Alternative name \"alt\"\r\n[dshow @ 000001] \"Microphone (Realtek(R) Audio)\" (audio)\r\n[dshow @ 000001] \"マイク (USB Audio)\" (audio)\r\ndummy: Immediate exit requested","win32"),["Microphone (Realtek(R) Audio)","マイク (USB Audio)"]);
    assert_eq!(parse_microphones("[dshow @ 02] DirectShow video devices (some may be both video and audio devices)\n[dshow @ 02]  \"Integrated Camera\"\n[dshow @ 02] DirectShow audio devices\n[dshow @ 02]  \"Line In (Focusrite USB)\"","win32"),["Line In (Focusrite USB)"]);
}
#[test]
fn meter_distinguishes_digital_silence_room_voice_and_clicks() {
    let mut silent = Meter::new();
    silent.push(&vec![0; 16000]);
    assert_eq!(silent.peak, 0);
    assert!(!silent.spoke());
    let mut room = Meter::new();
    room.push(&voice(2.0, &[]));
    assert!(room.peak > 0 && !room.spoke() && !room.speaking());
    assert!(room.take() < 0.15);
    let mut talking = Meter::new();
    talking.push(&voice(4.0, &[(0.5, 1.2), (1.5, 2.2)]));
    assert!(talking.spoke() && talking.speaking());
    assert!((talking.quiet_ms() - 1800.0).abs() < 100.0);
    assert!(talking.floor() < -55.0);
    assert!(talking.take() > 0.6);
    assert_eq!(talking.take(), 0.0);
    let mut click = Meter::new();
    click.push(&voice(2.0, &[(1.0, 1.04)]));
    assert!(!click.spoke());
}
#[test]
fn source_meter_states_words_and_prompts_match() {
    let fixture: Value = serde_json::from_str(include_str!("support/voice/reference.json")).unwrap();
    for c in fixture["clean"].as_array().unwrap() {
        assert_eq!(clean_words(c["input"].as_str().unwrap()), c["expected"].as_str().unwrap());
    }
    for c in fixture["prompt"].as_array().unwrap() {
        assert_eq!(voice_prompt(&serde_json::from_value::<Vec<String>>(c["names"].clone()).unwrap()), c["expected"].as_str().unwrap());
    }
    for c in fixture["records"].as_array().unwrap() {
        let seed = c["seed"].as_f64().unwrap();
        let mut x = seed;
        let samples: Vec<i16> = (0..16000)
            .map(|i| {
                x = (x * 1103515245.0 + 12345.0) % 2147483648.0;
                kumi_common::js::number::round((x / 1073741824.0 - 1.0) * if i > 4000 && i < 8000 { seed * 1700.0 } else { seed }) as i16
            })
            .collect();
        let mut meter = Meter::new();
        for (chunk, expected) in samples.chunks(137).zip(c["result"].as_array().unwrap()) {
            meter.push(chunk);
            assert_eq!(meter.peak, expected["peak"].as_i64().unwrap() as i32);
            assert_eq!(meter.floor(), expected["floor"].as_f64().unwrap());
            assert_eq!(meter.speaking(), expected["speaking"]);
            assert_eq!(meter.spoke(), expected["spoke"]);
            assert_eq!(meter.quiet_ms(), expected["quietMs"].as_f64().unwrap());
            assert!((meter.take() - expected["level"].as_f64().unwrap()).abs() < 1e-14);
        }
    }
}
#[test]
fn wav_context_and_terminal_names() {
    let file = wav_file(&vec![1; 32000]);
    assert_eq!(file.len(), 32044);
    assert_eq!(&file[..4], b"RIFF");
    assert_eq!(&file[8..12], b"WAVE");
    assert_eq!(&file[36..40], b"data");
    assert_eq!(u16::from_le_bytes(file[22..24].try_into().unwrap()), 1);
    assert_eq!(u32::from_le_bytes(file[24..28].try_into().unwrap()), 16000);
    assert_eq!(u16::from_le_bytes(file[34..36].try_into().unwrap()), 16);
    assert_eq!(u32::from_le_bytes(file[40..44].try_into().unwrap()), 32000);
    assert_eq!(audio_context_for(1.0), 192.0);
    assert!(audio_context_for(4.5) < audio_context_for(12.0) && audio_context_for(12.0) < 1500.0);
    assert_eq!(audio_context_for(40.0), 1500.0);
    for (env, name) in [
        (Env::from([("__CFBundleIdentifier".into(), "com.googlecode.iterm2".into()), ("TERM_PROGRAM".into(), "tmux".into())]), "iTerm2"),
        (Env::from([("TERM_PROGRAM".into(), "Apple_Terminal".into())]), "Terminal"),
        (Env::from([("TERM_PROGRAM".into(), "ghostty".into())]), "Ghostty"),
        (Env::new(), "your terminal app"),
    ] {
        assert_eq!(terminal_app(&env), name);
    }
}
fn options(folder: &Path) -> VoiceOptions {
    VoiceOptions { tools_dir: folder.join("tools").to_string_lossy().into(), ..Default::default() }
}
fn voice_error(error: VoiceFailure) -> VoiceError {
    match error {
        VoiceFailure::Voice(error) => error,
        other => panic!("{other}"),
    }
}
#[tokio::test(flavor = "current_thread")]
async fn mac_missing_programs_are_one_install_command_and_missing_input_is_named() {
    let folder = tempfile::tempdir().unwrap();
    let missing = folder.path().join("not-here").to_string_lossy().into_owned();
    let base = VoiceOptions {
        env: Some(Env::from([("KUMI_FFMPEG".into(), missing.clone()), ("KUMI_WHISPER".into(), missing.clone())])),
        platform: Some("darwin".into()),
        ..options(folder.path())
    };
    let error = voice_error(listen(ListenOptions { voice: base.clone(), ..Default::default() }).await.err().unwrap());
    assert_eq!(error.trouble, VoiceTrouble::Missing);
    assert!(error.message.ends_with("needs ffmpeg, which hears the microphone, and whisper.cpp, which writes down what you say on this computer. Install them: brew install ffmpeg whisper-cpp"));
    let error = voice_error(
        listen(ListenOptions {
            voice: base,
            ffmpeg: Some(std::env::current_exe().unwrap().to_string_lossy().into()),
            ..Default::default()
        })
        .await
        .err()
        .unwrap(),
    );
    assert!(error.message.ends_with("Install it: brew install whisper-cpp"));
    let error = voice_error(
        listen(ListenOptions {
            voice: VoiceOptions { env: Some(Env::from([("KUMI_VOICE_INPUT".into(), missing.clone())])), ..options(folder.path()) },
            ..Default::default()
        })
        .await
        .err()
        .unwrap(),
    );
    assert_eq!(error.message, format!("KUMI_VOICE_INPUT names {missing}, which isn't there."));
}
async fn hear_file(folder: &Path, name: &str, samples: &[i16], ffmpeg: &str) -> Listening {
    let file = folder.join(name);
    std::fs::write(&file, wav_file(&pcm(samples))).unwrap();
    let mut env = system::process_env();
    env.insert("KUMI_VOICE_INPUT".into(), file.to_string_lossy().into());
    env.insert("KUMI_WHISPER".into(), std::env::current_exe().unwrap().to_string_lossy().into());
    listen(ListenOptions { voice: VoiceOptions { env: Some(env), ..options(folder) }, ffmpeg: Some(ffmpeg.into()), ..Default::default() })
        .await
        .unwrap()
}
#[tokio::test(flavor = "current_thread")]
async fn simulated_voice_arrives_at_its_own_pace_moves_the_meter_and_ends() {
    let Some(ffmpeg) = find_ffmpeg(FfmpegOptions { installed_only: true, ..Default::default() }).await.unwrap() else { return };
    let folder = tempfile::tempdir().unwrap();
    let start = Instant::now();
    let listening = hear_file(folder.path(), "voice.wav", &voice(2.5, &[(0.3, 1.0), (1.3, 1.9)]), &ffmpeg).await;
    let mut levels = Vec::new();
    let mut interval = tokio::time::interval(Duration::from_millis(100));
    loop {
        tokio::select! {ended=listening.ended()=>{assert!(ended.is_none());break;},_=interval.tick()=>levels.push(listening.level())}
    }
    assert!(start.elapsed() > Duration::from_millis(1500));
    assert!(listening.spoke());
    assert!(levels.iter().any(|&l| l > 0.6) && levels.iter().any(|&l| l < 0.2), "{levels:?}");
    let heard = listening.stop().await;
    assert!((heard.seconds - 2.5).abs() < 0.2);
    assert!(heard.peak > 5000 && heard.spoke);
    assert_eq!(heard.pcm.len(), (heard.seconds * 16000.0).round() as usize * 2);
}
#[tokio::test(flavor = "current_thread")]
async fn only_a_voice_is_written_down_silence_and_quiet_explain_what_is_wrong() {
    let Some(ffmpeg) = find_ffmpeg(FfmpegOptions { installed_only: true, ..Default::default() }).await.unwrap() else { return };
    let folder = tempfile::tempdir().unwrap();
    for (platform, samples, trouble, phrase) in [
        (
            "darwin",
            vec![0; 16000],
            VoiceTrouble::Silence,
            "Kumi got only silence from the microphone. Allow Terminal in System Settings › Privacy & Security › Microphone",
        ),
        (
            "win32",
            vec![0; 16000],
            VoiceTrouble::Silence,
            "desktop apps may use the microphone (Settings › Privacy & security › Microphone)",
        ),
        ("linux", voice(1.5, &[]), VoiceTrouble::Quiet, "didn't hear you"),
    ] {
        let listening = hear_file(folder.path(), &format!("{platform}.wav"), &samples, &ffmpeg).await;
        assert!(listening.ended().await.is_none());
        let error = voice_error(
            write_down(
                listening.stop().await,
                WriteDownOptions {
                    voice: VoiceOptions {
                        env: Some(Env::from([("TERM_PROGRAM".into(), "Apple_Terminal".into())])),
                        platform: Some(platform.into()),
                        ..options(folder.path())
                    },
                    ..Default::default()
                },
            )
            .await
            .unwrap_err(),
        );
        assert_eq!(error.trouble, trouble);
        assert!(error.message.contains(phrase), "{}", error.message);
    }
}
#[tokio::test(flavor = "current_thread")]
async fn readiness_reads_present_programs_and_models_without_fetching() {
    let folder = tempfile::tempdir().unwrap();
    let model = folder.path().join("model.bin");
    std::fs::write(&model, "a model").unwrap();
    let exe = std::env::current_exe().unwrap().to_string_lossy().into_owned();
    let ready = voice_readiness(ReadinessOptions {
        env: Some(Env::from([
            ("KUMI_FFMPEG".into(), exe.clone()),
            ("KUMI_WHISPER".into(), exe.clone()),
            ("KUMI_WHISPER_MODEL".into(), model.to_string_lossy().into()),
        ])),
        tools_dir: folder.path().join("ready").to_string_lossy().into(),
        platform: Some("linux".into()),
        language: None,
    })
    .await
    .unwrap();
    assert_eq!(
        serde_json::to_value(ready).unwrap(),
        json!({"ffmpeg":exe,"whisper":exe,"model":{"name":"ggml-small.en-q5_1.bin","path":model},"fetches":true})
    );
    let missing = folder.path().join("none").to_string_lossy().into_owned();
    let bare = voice_readiness(ReadinessOptions {
        env: Some(Env::from([("KUMI_FFMPEG".into(), missing.clone()), ("KUMI_WHISPER".into(), missing)])),
        tools_dir: folder.path().join("bare").to_string_lossy().into(),
        platform: Some("win32".into()),
        language: Some("ja".into()),
    })
    .await
    .unwrap();
    assert_eq!(serde_json::to_value(bare).unwrap(), json!({"model":{"name":"ggml-small-q5_1.bin"},"fetches":true}));
}
#[cfg(unix)]
fn script(folder: &Path, name: &str, text: &str) -> String {
    use std::os::unix::fs::PermissionsExt;
    let path = folder.join(name);
    std::fs::write(&path, text).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
    path.to_string_lossy().into()
}
#[cfg(unix)]
#[tokio::test(flavor = "current_thread")]
async fn capture_joins_partial_samples_reports_failure_and_suppresses_ended_when_stopped() {
    let folder = tempfile::tempdir().unwrap();
    let command = script(
        folder.path(),
        "capture",
        "#!/bin/sh\nprintf '\\001'\nsleep 0.02\nprintf '\\000\\002\\000'\nprintf 'first\\nlast problem\\n' >&2\nexit 3\n",
    );
    let capture = start_capture(&command, &[]);
    capture.started().await;
    assert_eq!(capture.ended().await.as_deref(), Some("last problem"));
    assert_eq!(capture.pcm(), [1, 0, 2, 0]);
    assert_eq!(capture.meter.lock().unwrap().peak, 2);
    assert_eq!(capture.seconds(), 2.0 / 16000.0);
    let command = script(folder.path(), "waiting", "#!/bin/sh\nprintf '\\001\\000'\nexec sleep 10\n");
    let capture = start_capture(&command, &[]);
    capture.started().await;
    capture.stop().await;
    assert!(tokio::time::timeout(Duration::from_millis(30), capture.ended()).await.is_err());
    assert_eq!(capture.pcm(), [1, 0]);
    let capture = start_capture(folder.path().join("absent").to_str().unwrap(), &[]);
    assert!(capture.ended().await.unwrap().ends_with("ENOENT"));
}
#[cfg(unix)]
#[tokio::test(flavor = "current_thread")]
async fn writing_retries_without_vad_uses_private_temporary_audio_and_reports_whisper_failures() {
    let folder = tempfile::tempdir().unwrap();
    let command = script(
        folder.path(),
        "whisper",
        r#"#!/bin/sh
root="$(dirname "$0")"
printf '%s\n' "$@" >> "$root/args"
vad=false
while [ "$#" -gt 0 ]; do
 case "$1" in
 -of) shift; out="$1";;
 -f) shift; wav="$1";;
 --vad) vad=true;;
 esac
 shift
done
printf '%s' "$wav" > "$root/wav"
if [ "$vad" = true ]; then printf 'unknown vad option\n' >&2; exit 2; fi
if [ -f "$root/fail" ]; then printf 'broken whisper\n' >&2; exit 3; fi
words='Make the bass darker [BLANK_AUDIO]'
if [ -f "$root/phantom" ]; then words='Thank you.'; fi
printf '{"transcription":[{"offsets":{"from":0,"to":1000},"text":"%s"}]}' "$words" > "$out.json"
"#,
    );
    let model = folder.path().join("model.bin");
    std::fs::write(&model, "model").unwrap();
    let models = folder.path().join("tools/whisper-models");
    std::fs::create_dir_all(&models).unwrap();
    std::fs::write(models.join(VAD_MODEL), "vad").unwrap();
    let options = WriteDownOptions {
        voice: VoiceOptions {
            env: Some(Env::from([("KUMI_WHISPER".into(), command), ("KUMI_WHISPER_MODEL".into(), model.to_string_lossy().into())])),
            ..options(folder.path())
        },
        language: Some("en-US".into()),
        names: vec!["Bass".into()],
    };
    let heard = Heard { pcm: vec![1; 32000], seconds: 1.0, peak: 5000, spoke: true };
    assert_eq!(write_down(heard.clone(), options.clone()).await.unwrap(), "Make the bass darker");
    let args = std::fs::read_to_string(folder.path().join("args")).unwrap();
    assert_eq!(args.lines().filter(|s| *s == "--vad").count(), 1);
    assert_eq!(args.lines().filter(|s| *s == "--prompt").count(), 2);
    assert!(args.contains("-ac\n192\n"));
    let wav = std::fs::read_to_string(folder.path().join("wav")).unwrap();
    assert!(!Path::new(&wav).parent().unwrap().exists());
    std::fs::write(folder.path().join("phantom"), "").unwrap();
    assert_eq!(voice_error(write_down(heard.clone(), options.clone()).await.unwrap_err()).trouble, VoiceTrouble::Words);
    std::fs::write(folder.path().join("fail"), "").unwrap();
    let error = voice_error(write_down(heard, options).await.unwrap_err());
    assert_eq!(error.trouble, VoiceTrouble::Failed);
    assert_eq!(error.message, "Kumi couldn't write down what you said (broken whisper).");
    let wav = std::fs::read_to_string(folder.path().join("wav")).unwrap();
    assert!(!Path::new(&wav).parent().unwrap().exists());
}
#[tokio::test(flavor = "current_thread")]
async fn speech_model_fetch_announces_purpose_progress_and_vad_uses_its_own_location() {
    use futures::FutureExt;
    use sha2::{Digest, Sha256};
    use std::sync::{Arc, Mutex};
    let folder = tempfile::tempdir().unwrap();
    let told = Arc::new(Mutex::new(Vec::new()));
    let progress = Arc::new(Mutex::new(Vec::new()));
    let options = ProgramOptions {
        tools_dir: folder.path().to_string_lossy().into(),
        env: Some(Env::new()),
        download: Some(Arc::new(|url, _| {
            async move {
                Ok(if url.contains("/api/models/") {
                    serde_json::to_vec(
                        &json!([{"path":"ggml-tiny.en.bin","size":14,"lfs":{"oid":hex::encode(Sha256::digest(b"a speech model"))}}]),
                    )
                    .unwrap()
                } else {
                    b"a speech model".to_vec()
                })
            }
            .boxed()
        })),
        free: Some(Arc::new(|_| async { Some(1e12) }.boxed())),
        purpose: Some("to write down what you say".into()),
        on_fetch: Some(Arc::new({
            let told = told.clone();
            move |message| told.lock().unwrap().push(message.to_string())
        })),
        on_progress: Some(Arc::new({
            let progress = progress.clone();
            move |fraction| progress.lock().unwrap().push(fraction)
        })),
        ..Default::default()
    };
    whisper_model("ggml-tiny.en.bin", &options).await.unwrap();
    assert_eq!(*told.lock().unwrap(), ["Kumi is fetching a speech model, to write down what you say (once, about 0 MB)."]);
    assert_eq!(*progress.lock().unwrap(), [1.0]);
    let asked = Arc::new(Mutex::new(Vec::new()));
    let path = vad_model(&ProgramOptions {
        env: Some(Env::from([("KUMI_WHISPER_MODEL".into(), folder.path().join("elsewhere.bin").to_string_lossy().into())])),
        download: Some(Arc::new({
            let asked = asked.clone();
            move |url, _| {
                asked.lock().unwrap().push(url.clone());
                async move {
                    Ok(if url.contains("/api/models/") {
                        serde_json::to_vec(
                            &json!([{"path":VAD_MODEL,"size":22,"lfs":{"oid":hex::encode(Sha256::digest(b"a voice activity model"))}}]),
                        )
                        .unwrap()
                    } else {
                        b"a voice activity model".to_vec()
                    })
                }
                .boxed()
            }
        })),
        ..options
    })
    .await
    .unwrap();
    assert_eq!(Path::new(&path), folder.path().join("whisper-models").join(VAD_MODEL));
    assert_eq!(
        *asked.lock().unwrap(),
        [
            "https://huggingface.co/api/models/ggml-org/whisper-vad/tree/main".to_string(),
            format!("https://huggingface.co/ggml-org/whisper-vad/resolve/main/{VAD_MODEL}")
        ]
    );
    assert_eq!(told.lock().unwrap().len(), 1);
}
#[tokio::test(flavor = "current_thread")]
async fn optional_real_speech_is_written_down_locally() {
    if system::platform() != "darwin" || std::env::var_os("KUMI_WHISPER_MODEL").is_none() {
        eprintln!("needs whisper.cpp, KUMI_WHISPER_MODEL and macOS's say");
        return;
    }
    let folder = tempfile::tempdir().unwrap();
    let Some(ffmpeg) = find_ffmpeg(FfmpegOptions { installed_only: true, ..Default::default() }).await.unwrap() else { return };
    if find_whisper(&ProgramOptions { tools_dir: folder.path().to_string_lossy().into(), installed_only: true, ..Default::default() })
        .await
        .unwrap()
        .is_none()
    {
        return;
    }
    let spoken = folder.path().join("spoken.aiff");
    assert!(tokio::process::Command::new("say")
        .arg("-o")
        .arg(&spoken)
        .arg("Put a Saturator after the Operator on the bass.")
        .status()
        .await
        .unwrap()
        .success());
    let mut env = system::process_env();
    env.insert("KUMI_VOICE_INPUT".into(), spoken.to_string_lossy().into());
    let listening = listen(ListenOptions {
        voice: VoiceOptions { env: Some(env), ..options(folder.path()) },
        ffmpeg: Some(ffmpeg),
        ..Default::default()
    })
    .await
    .unwrap();
    listening.ended().await;
    let words = write_down(
        listening.stop().await,
        WriteDownOptions { voice: options(folder.path()), language: Some("en".into()), names: vec!["Bass".into()] },
    )
    .await
    .unwrap();
    assert!(words.to_lowercase().contains("saturator after the operator on the bass"));
}
