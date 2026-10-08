//! Model slots: asked in plain words, kept in a file and taken back, refused honestly where Kumi can't run a model
//! yet, and a listening model swapped only after it hears a known clip right (a model on this computer, stood in for
//! by a small server here that really listens: it compares the takes' brightness), followed from the next listen and
//! off at once. A slots file Kumi can't read turns listening off and isn't written over; a swap made while another is
//! tried stands; a model file is found by its full path, and a link fetched over https only.
use async_trait::async_trait;
use base64::Engine;
use kumi_common::abort::Signal;
use kumi_runtime::{
    auth::store::{open_credential_store, CredentialStore},
    listening::listener::{Answer, Listener},
    slots::{self, command, fits, hears_known_clip, parse, Asked, Choice, Job, Said, Slots, SlotsContext, Wanted},
};
use std::{collections::HashMap, rc::Rc};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// How bright a stretch is: its first difference against itself (a sine's is small, a saw's large).
fn brightness(samples: &[f64]) -> f64 {
    let moved: f64 = samples.windows(2).map(|pair| (pair[1] - pair[0]).abs()).sum();
    moved / samples.iter().map(|sample| sample.abs()).sum::<f64>().max(1e-9)
}

/// Which take of a WAV the judge sends (first, 0.8 s of silence, second) is brighter, by listening to it.
fn brighter(wav: &[u8]) -> &'static str {
    let rate = u32::from_le_bytes(wav[24..28].try_into().unwrap()) as usize;
    let samples: Vec<f64> = wav[44..].chunks_exact(2).map(|pair| i16::from_le_bytes([pair[0], pair[1]]) as f64).collect();
    // The pause splits the takes: the longest run of silence.
    let (mut best, mut run, mut at) = ((0, 0), 0, 0);
    for (index, sample) in samples.iter().enumerate() {
        run = if *sample == 0. { run + 1 } else { 0 };
        if run > best.1 {
            best = (index + 1 - run, run);
            at = index + 1;
        }
    }
    assert!(best.1 > rate / 2, "a pause between the takes");
    if brightness(&samples[..best.0]) > brightness(&samples[at..]) {
        "first"
    } else {
        "second"
    }
}

/// A listener that hears (or one that always says the first).
struct Ears(bool);
#[async_trait(?Send)]
impl Listener for Ears {
    fn name(&self) -> String {
        if self.0 { "ears" } else { "deaf" }.into()
    }
    async fn ask(&self, wav: &[u8], _aim: &str, _signal: Signal) -> Result<Answer, String> {
        Ok(Answer { closer: if self.0 { brighter(wav) } else { "first" }.into(), first: vec![], second: vec![] })
    }
}

/// A model server on this computer, OpenAI-compatible, that takes audio: it answers which take is brighter (or, deaf,
/// always the first), and counts what it's asked. Its address.
async fn server(hears: bool) -> (String, Rc<std::sync::atomic::AtomicUsize>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = format!("http://{}/v1", listener.local_addr().unwrap());
    let asked = Rc::new(std::sync::atomic::AtomicUsize::new(0));
    let counted = asked.clone();
    tokio::task::spawn_local(async move {
        loop {
            let Ok((mut stream, _)) = listener.accept().await else { return };
            let mut request = vec![];
            let mut buffer = [0u8; 65536];
            let body_at = loop {
                let read = stream.read(&mut buffer).await.unwrap_or(0);
                if read == 0 {
                    break None;
                }
                request.extend_from_slice(&buffer[..read]);
                if let Some(at) = request.windows(4).position(|window| window == b"\r\n\r\n") {
                    break Some(at + 4);
                }
            };
            let Some(body_at) = body_at else { continue };
            let head = String::from_utf8_lossy(&request[..body_at]).to_lowercase();
            let length: usize =
                head.lines().find_map(|line| line.strip_prefix("content-length:")).map(|value| value.trim().parse().unwrap()).unwrap_or(0);
            while request.len() < body_at + length {
                let read = stream.read(&mut buffer).await.unwrap_or(0);
                if read == 0 {
                    break;
                }
                request.extend_from_slice(&buffer[..read]);
            }
            if request.len() < body_at + length {
                continue;
            }
            let body: serde_json::Value = serde_json::from_slice(&request[body_at..body_at + length]).unwrap();
            let audio = body["messages"][0]["content"][1]["input_audio"]["data"].as_str().unwrap();
            let wav = base64::engine::general_purpose::STANDARD.decode(audio).unwrap();
            counted.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let closer = if hears { brighter(&wav) } else { "first" };
            let answer = serde_json::json!({"choices":[{"message":{"content":format!("{{\"closer\": \"{closer}\", \"first\": [], \"second\": []}}")}}]}).to_string();
            let reply = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{answer}",
                answer.len()
            );
            let _ = stream.write_all(reply.as_bytes()).await;
            let _ = stream.shutdown().await;
        }
    });
    (address, asked)
}

fn context(folder: &std::path::Path, env: &[(&str, &str)]) -> SlotsContext {
    SlotsContext {
        file: slots::file_in(folder),
        store: Rc::new(open_credential_store(folder.join("auth.json"))) as Rc<dyn CredentialStore>,
        env: env.iter().map(|(key, value)| (key.to_string(), value.to_string())).collect::<HashMap<_, _>>(),
    }
}

fn quiet(_: String) {}

#[test]
fn plain_words_ask_for_a_slot_and_what_it_should_use() {
    assert_eq!(parse(""), Ok(Asked::Show));
    assert_eq!(parse(" show "), Ok(Asked::Show));
    assert_eq!(parse("listening gemini"), Ok(Asked::Swap(Job::Listening, Wanted::Choice(Choice::Gemini))));
    assert_eq!(parse("use OpenAI for listening"), Ok(Asked::Swap(Job::Listening, Wanted::Choice(Choice::Openai))));
    assert_eq!(parse("Gemini, please."), Ok(Asked::Swap(Job::Listening, Wanted::Choice(Choice::Gemini))));
    assert_eq!(parse("ears off"), Ok(Asked::Swap(Job::Listening, Wanted::Choice(Choice::Off))));
    assert_eq!(
        parse("listening http://127.0.0.1:8080/v1/#qwen3-omni"),
        Ok(Asked::Swap(
            Job::Listening,
            Wanted::Choice(Choice::Local { base: "http://127.0.0.1:8080/v1".into(), model: "qwen3-omni".into() })
        ))
    );
    assert_eq!(parse("stems back to Live’s splitter"), Ok(Asked::Back(Job::Stems)));
    assert_eq!(parse("stems Live's splitter"), Ok(Asked::Swap(Job::Stems, Wanted::Choice(Choice::Default))));
    assert_eq!(parse("revert listening"), Ok(Asked::Back(Job::Listening)));
    assert_eq!(
        parse("stems https://huggingface.co/someone/stems-model"),
        Ok(Asked::Swap(Job::Stems, Wanted::Link("https://huggingface.co/someone/stems-model".into())))
    );
    assert_eq!(parse("embeddings ~/models/clap.onnx"), Ok(Asked::Swap(Job::Embeddings, Wanted::File("~/models/clap.onnx".into()))));
    // A path with spaces: in quotes, or running on to its file ending (the words after it still read).
    let spaced = Ok(Asked::Swap(Job::Embeddings, Wanted::File("~/My Models/clap.onnx".into())));
    assert_eq!(parse("embeddings \"~/My Models/clap.onnx\""), spaced);
    assert_eq!(parse("embeddings ‘~/My Models/clap.onnx’"), spaced);
    assert_eq!(parse("embeddings ~/My Models/clap.onnx"), spaced);
    assert_eq!(parse("~/My Models/clap.onnx for embeddings, please."), spaced);
    assert_eq!(parse("embeddings ./clap.onnx."), Ok(Asked::Swap(Job::Embeddings, Wanted::File("./clap.onnx".into()))));
    assert!(parse("back").unwrap_err().contains("Which slot goes back"));
    assert!(parse("listening banana").unwrap_err().contains("doesn't know “banana”"));
    assert!(parse("off").unwrap_err().contains("Which slot"));
}

#[test]
fn a_slot_takes_only_what_kumi_can_run_and_says_why_not() {
    let link = Wanted::Link("https://huggingface.co/someone/model".into());
    assert!(fits(Job::Stems, &link).unwrap_err().contains("can't run a stem model itself yet"));
    assert!(fits(Job::Transcription, &Wanted::File("/models/notes.onnx".into())).unwrap_err().contains("stays on Live's conversions"));
    // Embeddings run in Kumi's own runtime: an ONNX file, or a link to one; a model page isn't one.
    assert!(fits(Job::Embeddings, &link).unwrap_err().contains("ONNX"));
    let onnx = "https://huggingface.co/someone/model/resolve/main/model.onnx";
    assert_eq!(fits(Job::Embeddings, &Wanted::Link(onnx.into())), Ok(Choice::File { path: onnx.into() }));
    assert_eq!(fits(Job::Embeddings, &Wanted::File("/models/clap.onnx".into())), Ok(Choice::File { path: "/models/clap.onnx".into() }));
    assert!(fits(Job::Embeddings, &Wanted::Choice(Choice::Gemini)).is_err());
    assert!(fits(Job::Listening, &link).unwrap_err().contains("llama-server -hf"));
    assert_eq!(fits(Job::Stems, &Wanted::Choice(Choice::Default)), Ok(Choice::Default));
    assert!(fits(Job::Stems, &Wanted::Choice(Choice::Gemini)).is_err());
    assert_eq!(fits(Job::Embeddings, &Wanted::Choice(Choice::Off)), Ok(Choice::Off));
}

#[test]
fn a_slot_is_kept_in_its_file_and_swaps_are_taken_back_in_turn() {
    let folder = tempfile::tempdir().unwrap();
    let file = slots::file_in(folder.path());
    assert_eq!(Slots::load(&file).now(Job::Listening), Choice::Default);
    let mut kept = Slots::default();
    kept.switch(Job::Listening, Choice::Gemini);
    kept.switch(Job::Listening, Choice::Off);
    kept.save(&file).unwrap();
    let mut read = Slots::load(&file);
    assert_eq!(read, kept);
    assert_eq!(read.now(Job::Listening), Choice::Off);
    assert_eq!(read.back(Job::Listening), Some(Choice::Gemini));
    assert_eq!(read.back(Job::Listening), Some(Choice::Default));
    assert_eq!(read.back(Job::Listening), None);
    assert_eq!(read, Slots::default());
    // The embeddings model's file, for the model runtime to load once one is fetched.
    let model = folder.path().join("models").join("embeddings.onnx");
    read.switch(Job::Embeddings, Choice::File { path: model.display().to_string() });
    assert_eq!((read.model_file(Job::Embeddings), read.model_file(Job::Listening)), (Some(model.clone()), None));
    assert_eq!(slots::describe(Job::Embeddings, &read.now(Job::Embeddings)), format!("the model file {}", model.display()));
    assert_eq!(read.back(Job::Embeddings), Some(Choice::Default));
    assert_eq!(read.model_file(Job::Embeddings), None);
    // A file Kumi can't read (a newer Kumi's, a hand edit) turns listening off, and isn't written over: it's copied
    // beside first.
    std::fs::write(&file, "{not json").unwrap();
    assert!(Slots::read(&file).unwrap_err().starts_with("Kumi can't read the model slots in"));
    assert_eq!(Slots::load(&file).now(Job::Listening), Choice::Off);
    let aside = Slots::default().save(&file).unwrap().unwrap();
    assert_eq!((std::fs::read_to_string(&aside).unwrap().as_str(), Slots::read(&file)), ("{not json", Ok(Slots::default())));
}

#[tokio::test]
async fn the_known_clip_tells_a_listener_that_hears_from_one_that_doesnt() {
    assert!(hears_known_clip(&Ears(true), Signal::new()).await.unwrap().contains("heard the known clip right both ways round"));
    let why = hears_known_clip(&Ears(false), Signal::new()).await.unwrap_err();
    assert!(why.contains("didn't hear the known clip right") && why.contains("said first then first"), "{why}");
}

#[tokio::test]
async fn a_listening_model_is_swapped_in_after_its_quick_test_and_swapped_back() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let folder = tempfile::tempdir().unwrap();
            let context = context(folder.path(), &[]);
            let (ears, asked) = server(true).await;
            let said = command(&format!("listening {ears}#local-ears"), &context, &quiet, Signal::new()).await;
            match &said {
                Said::Done(text) => assert!(
                    text.starts_with(&format!("Listening now uses local-ears at {ears}: local-ears heard the known clip right both ways round."))
                        && text.ends_with("/slots back listening takes it back."),
                    "{text}"
                ),
                other => panic!("{other:?}"),
            }
            assert_eq!(asked.load(std::sync::atomic::Ordering::SeqCst), 2, "asked both ways round");
            assert_eq!(Slots::load(&context.file).now(Job::Listening), Choice::Local { base: ears.clone(), model: "local-ears".into() });
            match command("", &context, &quiet, Signal::new()).await {
                Said::Slots { lines, .. } => assert!(lines[2].starts_with("listening (") && lines[2].contains("local-ears") && lines[2].contains("swapped"), "{lines:?}"),
                other => panic!("{other:?}"),
            }
            // One that doesn't hear the clip right isn't switched to.
            let (deaf, _) = server(false).await;
            match command(&format!("listening {deaf}#deaf-ears"), &context, &quiet, Signal::new()).await {
                Said::Refused(why) => assert!(why.starts_with("Listening stays on local-ears") && why.contains("didn't hear the known clip right"), "{why}"),
                other => panic!("{other:?}"),
            }
            // No key: said, nothing asked.
            match command("listening gemini", &context, &quiet, Signal::new()).await {
                Said::Refused(why) => assert!(why.contains("there's no Gemini API key"), "{why}"),
                other => panic!("{other:?}"),
            }
            // And back.
            assert_eq!(
                command("back listening", &context, &quiet, Signal::new()).await,
                Said::Done(format!("Listening is back on {}.", slots::describe(Job::Listening, &Choice::Default)))
            );
            assert_eq!(Slots::load(&context.file), Slots::default());
            assert!(matches!(command("back listening", &context, &quiet, Signal::new()).await, Said::Refused(why) if why.contains("hasn't been swapped")));
            // What Kumi can't run, refused with why.
            match command("stems https://huggingface.co/someone/stems", &context, &quiet, Signal::new()).await {
                Said::Refused(why) => assert!(why.contains("stays on Live's own splitter"), "{why}"),
                other => panic!("{other:?}"),
            }
        })
        .await;
}

#[tokio::test]
async fn the_judges_listener_follows_the_slot_from_its_next_listen() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let folder = tempfile::tempdir().unwrap();
            let context = context(folder.path(), &[]);
            let (one, _) = server(true).await;
            let (two, heard_by_two) = server(true).await;
            let mut kept = Slots::default();
            kept.switch(Job::Listening, Choice::Local { base: one.clone(), model: "ears-one".into() });
            kept.save(&context.file).unwrap();
            let listener = slots::listener(&context.file, context.store.clone(), &context.env, Signal::new()).await.unwrap().unwrap();
            assert_eq!(listener.name(), "ears-one");
            kept.switch(Job::Listening, Choice::Local { base: two.clone(), model: "ears-two".into() });
            kept.save(&context.file).unwrap();
            let answer = listener.ask(&slots::known_clip(true), slots::KNOWN_AIM, Signal::new()).await.unwrap();
            assert_eq!((answer.closer.as_str(), listener.name()), ("second", "ears-two".to_string()));
            assert_eq!(heard_by_two.load(std::sync::atomic::Ordering::SeqCst), 1);
            // Swapped off, it's off at once: the judge skips it, and asked anyway, it sends nothing.
            kept.switch(Job::Listening, Choice::Off);
            kept.save(&context.file).unwrap();
            assert!(listener.off());
            let why = listener.ask(&slots::known_clip(true), slots::KNOWN_AIM, Signal::new()).await.unwrap_err();
            assert!(why.contains("listening is off"), "{why}");
            assert_eq!(heard_by_two.load(std::sync::atomic::Ordering::SeqCst), 1, "nothing sent");
            // Back on, it listens again; a slots file Kumi can't read is off too.
            kept.back(Job::Listening);
            kept.save(&context.file).unwrap();
            assert!(!listener.off());
            std::fs::write(&context.file, "{not json").unwrap();
            assert!(listener.off());
            let why = slots::listener(&context.file, context.store.clone(), &context.env, Signal::new()).await.err().unwrap();
            assert!(why.contains("can't read the model slots") && why.contains("listening is off"), "{why}");
            // KUMI_LISTENER wins over the slot; off is off.
            let named = slots::listener(
                &context.file,
                context.store.clone(),
                &[("KUMI_LISTENER".to_string(), format!("{one}#by-env"))].into(),
                Signal::new(),
            )
            .await
            .unwrap()
            .unwrap();
            assert_eq!(named.name(), "by-env");
            let off: HashMap<String, String> = [("KUMI_LISTENER".to_string(), "off".to_string())].into();
            assert!(slots::listener(&context.file, context.store.clone(), &off, Signal::new()).await.unwrap().is_none());
        })
        .await;
}

#[tokio::test]
async fn a_swap_made_while_another_is_tried_stands() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let folder = tempfile::tempdir().unwrap();
            let context = context(folder.path(), &[]);
            let (ears, _) = server(true).await;
            // The model is tried (a request to the server) while off is said and kept.
            let trying = format!("listening {ears}#slow-ears");
            let (tried, off) =
                tokio::join!(command(&trying, &context, &quiet, Signal::new()), command("listening off", &context, &quiet, Signal::new()));
            assert!(matches!(&off, Said::Done(text) if text.contains("Nothing is sent from now on")), "{off:?}");
            match tried {
                Said::Refused(why) => {
                    assert!(why.starts_with("Listening was swapped to off: the meters alone while Kumi tried this one"), "{why}")
                }
                other => panic!("{other:?}"),
            }
            assert_eq!(Slots::load(&context.file).now(Job::Listening), Choice::Off);
        })
        .await;
}

#[tokio::test]
async fn a_slots_file_kumi_cant_read_leaves_listening_off_until_a_swap_writes_it_afresh() {
    let folder = tempfile::tempdir().unwrap();
    let context = context(folder.path(), &[]);
    std::fs::write(&context.file, r#"{"listening":{"now":{"use":"someday-model"}}}"#).unwrap();
    match command("", &context, &quiet, Signal::new()).await {
        Said::Slots { lines, .. } => {
            assert!(lines[0].starts_with("Kumi can't read the model slots in") && lines[0].contains("listening is off"), "{lines:?}");
            assert!(lines.iter().any(|line| line.starts_with("listening (") && line.ends_with("off: the meters alone")), "{lines:?}");
        }
        other => panic!("{other:?}"),
    }
    assert!(
        matches!(command("back listening", &context, &quiet, Signal::new()).await, Said::Refused(why) if why.contains("no swap to take back"))
    );
    match command("embeddings off", &context, &quiet, Signal::new()).await {
        Said::Done(text) => assert!(text.contains("couldn't read is copied to") && text.contains("slots.json.unreadable"), "{text}"),
        other => panic!("{other:?}"),
    }
    // Listening stays off in the file written afresh; the one Kumi couldn't read is kept as it was.
    let kept = Slots::read(&context.file).unwrap();
    assert_eq!((kept.now(Job::Listening), kept.now(Job::Embeddings)), (Choice::Off, Choice::Off));
    let aside = folder.path().join("slots.json.unreadable");
    assert_eq!(std::fs::read_to_string(aside).unwrap(), r#"{"listening":{"now":{"use":"someday-model"}}}"#);
}

#[tokio::test]
async fn a_model_file_is_found_by_its_full_path_and_a_link_only_over_https() {
    let folder = tempfile::tempdir().unwrap();
    let home = folder.path().join("home");
    std::fs::create_dir_all(home.join("My Models")).unwrap();
    let context = context(folder.path(), &[("HOME", home.to_str().unwrap())]);
    // ~ is the home folder, spaces and all; a file that isn't there is said plainly, before any check.
    match command("embeddings \"~/My Models/clap.onnx\"", &context, &quiet, Signal::new()).await {
        Said::Refused(why) => {
            let wanted = home.join("My Models").join("clap.onnx");
            assert!(
                why.starts_with("Embeddings stays on") && why.ends_with(&format!("there's no such file: {}.", wanted.display())),
                "{why}"
            );
        }
        other => panic!("{other:?}"),
    }
    // Typed with backslashes, as on Windows, it's the same file, said with this computer's own separators.
    match command("embeddings \"~\\My Models\\clap.onnx\"", &context, &quiet, Signal::new()).await {
        Said::Refused(why) => {
            let wanted = home.join("My Models").join("clap.onnx");
            assert!(why.ends_with(&format!("there's no such file: {}.", wanted.display())), "{why}");
        }
        other => panic!("{other:?}"),
    }
    // A folder isn't a model file.
    std::fs::create_dir_all(home.join("models.onnx")).unwrap();
    match command("embeddings ~/models.onnx", &context, &quiet, Signal::new()).await {
        Said::Refused(why) => assert!(why.contains("isn't a model file but a folder"), "{why}"),
        other => panic!("{other:?}"),
    }
    // A link is fetched over https only.
    match command("embeddings http://127.0.0.1:9/model.onnx", &context, &quiet, Signal::new()).await {
        Said::Refused(why) => assert!(why.contains("Kumi fetches models over https only"), "{why}"),
        other => panic!("{other:?}"),
    }
    assert_eq!(Slots::read(&context.file), Ok(Slots::default()));
}
