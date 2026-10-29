//! The session's protocol in this process, as any handle here has it: an aside's words come as events with its id, and
//! stopping it stops it, even a stop that comes first; a request nobody waits for runs at once as far as it goes; the
//! model requests are the model controls' to answer; /music reads the producer's music; and /slots says the slots, and
//! a /slots fetch says what it does, and a stop ends it, even one that came first. (Every request has its arm in
//! `Session::answer`, whose match has no catch-all: a new request doesn't build until the session answers it.)
use futures::FutureExt;
use kumi_agent::core::{
    contracts::*,
    protocol::{models_answer, music_view, tell_now, HandleSide},
};
use kumi_types::errors::RuntimeError;
use std::{cell::RefCell, rc::Rc};

/// What `side` says between replies, kept.
fn heard(side: &HandleSide) -> Rc<RefCell<Vec<SessionEvent>>> {
    let heard = Rc::new(RefCell::new(vec![]));
    let into = heard.clone();
    side.listen(Rc::new(move |event| into.borrow_mut().push(event)));
    heard
}

#[tokio::test(flavor = "current_thread")]
async fn an_asides_words_come_as_events_with_its_id_and_stopping_it_stops_it() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let side = HandleSide::new();
            let heard = heard(&side);
            // Two pieces of words, then it waits until it's stopped.
            let asking = tokio::task::spawn_local({
                let side = side.clone();
                async move {
                    side.aside(4, |on_text, signal| async move {
                        on_text("It's ");
                        on_text("the low shelf.");
                        signal.cancelled().await;
                        Err::<String, _>(RuntimeError::plain("stopped"))
                    })
                    .await
                }
            });
            tokio::task::yield_now().await;
            assert_eq!(
                *heard.borrow(),
                [SessionEvent::Aside { id: 4, text: "It's ".into() }, SessionEvent::Aside { id: 4, text: "the low shelf.".into() }]
            );
            // Another aside's id stops nothing.
            side.stop_aside(5);
            tokio::task::yield_now().await;
            assert!(!asking.is_finished(), "only its own id stops it");
            side.stop_aside(4);
            assert!(asking.await.unwrap().is_err(), "a stopped aside ends");
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn a_stop_that_comes_before_its_aside_starts_still_stops_it() {
    let side = HandleSide::new();
    // Closed (its panel, or Kumi) before the task that asks it ran.
    side.stop_aside(1);
    let stopped = side.aside(1, |_, signal| async move { Ok(signal.is_cancelled().to_string()) }).await.unwrap();
    assert_eq!(stopped, "true", "the aside starts with its stop already given");
}

#[tokio::test(flavor = "current_thread")]
async fn a_request_nobody_waits_for_runs_at_once_as_far_as_it_goes() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let calls = Rc::new(RefCell::new(Vec::<&str>::new()));
            let at_once = calls.clone();
            tell_now(async move { at_once.borrow_mut().push("picked") }.boxed_local());
            assert_eq!(*calls.borrow(), ["picked"], "what's done at once is done at once");
            // One that waits on something goes on in a task of its own.
            let waits = calls.clone();
            tell_now(
                async move {
                    waits.borrow_mut().push("refresh:started");
                    tokio::task::yield_now().await;
                    waits.borrow_mut().push("refresh:done");
                }
                .boxed_local(),
            );
            assert_eq!(*calls.borrow(), ["picked", "refresh:started"]);
            tokio::task::yield_now().await;
            tokio::task::yield_now().await;
            assert_eq!(*calls.borrow(), ["picked", "refresh:started", "refresh:done"]);
        })
        .await;
}

/// The twelve model requests, signing in among them: what `models_answer` answers.
const MODEL_REQUESTS: [&str; 12] = [
    "Providers",
    "LocalModels",
    "ChooseDefault",
    "Models",
    "ChooseModel",
    "SetEffort",
    "SetFast",
    "SetFallback",
    "SaveKey",
    "SignOut",
    "SignInChatgpt",
    "MissingModel",
];

#[tokio::test(flavor = "current_thread")]
async fn the_model_requests_are_the_model_controls_to_answer() {
    let side = HandleSide::new();
    // One of each request, as their wire pins have them: a session without model controls says so for each model
    // request, and leaves every other to the session.
    let requests = crate::pin_wire::requests();
    for request in requests {
        let name = request.name();
        match models_answer(&request, None, &side).await {
            Some(Err(error)) if MODEL_REQUESTS.contains(&name) => assert_eq!(error.message(), "Not available", "{name}"),
            Some(answered) => panic!("{name}: answered {answered:?}"),
            None => assert!(!MODEL_REQUESTS.contains(&name), "{name} is a model request"),
        }
    }
}

#[tokio::test(flavor = "current_thread")]
async fn music_reads_the_producers_music() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let folder = tempfile::tempdir().unwrap();
            let music = music_in(folder.path());
            // Nothing agreed, held or found yet (nowhere to find songs); held once paused.
            let view = music_view(&music).await;
            assert!(!view.agreed && !view.held && view.found.is_none(), "{view:?}");
            music.hold(true);
            assert!(music_view(&music).await.held);
        })
        .await;
}

/// The producer's music, kept in `folder`, with nowhere to find songs and nothing learned while a test runs.
fn music_in(folder: &std::path::Path) -> Rc<kumi_listening::music::learning::Music> {
    use kumi_listening::music::learning;
    let mut options = learning::MusicOptions::new(folder.join("music"), None, Default::default());
    (options.places, options.fork, options.delay_ms) = (Some(vec![]), false, 3_600_000);
    learning::create_music(options)
}

/// /slots, kept in `folder`.
fn slots_in(folder: &std::path::Path) -> kumi_listening::slots::SlotsContext {
    use kumi_listening::slots;
    let store = kumi_ai::auth::store::open_credential_store(folder.join("auth.json"));
    slots::SlotsContext { file: slots::file_in(folder), store: Rc::new(store), env: Default::default(), profile: None }
}

#[tokio::test(flavor = "current_thread")]
async fn slots_says_the_slots_a_line_each() {
    let folder = tempfile::tempdir().unwrap();
    let said = HandleSide::new().slots(1, "", &slots_in(folder.path())).await;
    assert!(matches!(&said, kumi_listening::slots::Said::Slots { lines, .. } if !lines.is_empty()), "{said:?}");
}

#[tokio::test(flavor = "current_thread")]
async fn a_slots_fetch_says_what_it_does_and_a_stop_ends_it_even_one_that_came_first() {
    use kumi_listening::slots::Said;
    use std::time::Duration;
    tokio::task::LocalSet::new()
        .run_until(async {
            let folder = tempfile::tempdir().unwrap();
            // Kumi's models folder is here: what a link fetches would be kept in it.
            std::env::set_var("KUMI_HOME", folder.path());
            // A server on this computer that takes the connection and never answers: the fetch waits until it's stopped.
            let silent = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            let link = format!("https://127.0.0.1:{}/clap.onnx", silent.local_addr().unwrap().port());
            let words = format!("embeddings {link}");
            let side = HandleSide::new();
            let context = Rc::new(slots_in(folder.path()));
            let heard = heard(&side);
            let stopped = |said: &Said| matches!(said, Said::Refused(why) if why.ends_with(": stopped"));
            // Stopped while it fetches, which it said it was doing.
            let fetching = tokio::task::spawn_local({
                let (side, context, words) = (side.clone(), context.clone(), words.clone());
                async move { side.slots(1, &words, &context).await }
            });
            for _ in 0..1_000 {
                if !heard.borrow().is_empty() {
                    break;
                }
                tokio::task::yield_now().await;
            }
            assert_eq!(*heard.borrow(), [SessionEvent::SlotsProgress { id: 1, line: format!("Fetching {link} (Esc stops it)…") }]);
            side.stop_slots(1);
            let said = tokio::time::timeout(Duration::from_secs(5), fetching).await.expect("stopped").unwrap();
            assert!(stopped(&said), "{said:?}");
            // A stop that comes before its request starts still stops it, as the fetch begins.
            side.stop_slots(2);
            let said = tokio::time::timeout(Duration::from_secs(5), side.slots(2, &words, &context)).await.expect("stopped");
            assert!(stopped(&said), "{said:?}");
            drop(silent);
        })
        .await;
}
