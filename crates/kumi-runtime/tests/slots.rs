//! Model slots: asked in plain words, kept in a file and taken back, refused honestly where Kumi can't run a model
//! yet, and a listening model swapped only after it hears a known clip right (a model on this computer, stood in for
//! by a small server here that really listens: it compares the takes' brightness), followed from the next listen.
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
    // A file Kumi can't read leaves the defaults.
    std::fs::write(&file, "{not json").unwrap();
    assert_eq!(Slots::load(&file), Slots::default());
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
