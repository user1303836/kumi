//! Taking requests back in REAPER and Renoise: whole sessions as own_session_catchup's (Kumi's agent kernel on a scripted
//! model, the DAW's integration over Kumi's host in process in front of a simulated DAW), with the conversation and its
//! log kept as the app keeps them. Kumi undoes the requests' changes in the DAW newest first, through its own records,
//! stops at the first undo that fails, and the conversation goes back to just before the requests that went.
use crate::own_session_catchup::{answer, call, recording, simulator, Recorded, Step};
use daw_bridge::{files::ensure_secret, paths::BridgePaths, Daw};
use daw_mcp_server::{host::Host, reaper::simulator::ReaperScript, renoise::simulator::RenoiseScript};
use futures::FutureExt;
use kumi_agent::core::context::IntegrationContext;
use kumi_agent::core::contracts::*;
use kumi_agent::core::conversations::create_conversation_store_in;
use kumi_agent::core::session_log::SessionLog;
use kumi_agent::mcp::client::McpEndpoint;
use kumi_agent::{create_session, Session, SessionOptions};
use kumi_daw_own::bridge::{Connect, OwnOptions};
use kumi_types::contracts::*;
use kumi_types::daw::DawProfile;
use serde_json::json;
use std::{cell::Cell, cell::RefCell, path::Path, rc::Rc, sync::Arc};

kumi_testkit::in_process!();
use in_process::InProcess;

/// A session in a DAW of Kumi's own, its conversations and their logs kept in `home`: a change reaches the session and
/// the screen (through the log in front of it) as the app has it from the integration.
struct Kumi {
    session: Session,
    model: Rc<Recorded>,
}

impl Kumi {
    async fn start(daw: Daw, home: &Path) -> Kumi {
        let host = Rc::new(InProcess { host: Arc::new(Host::new(daw, home)), id: Cell::new(0) });
        let (profile, create): (DawProfile, fn(OwnOptions) -> Rc<dyn Integration>) = match daw {
            Daw::Reaper => (DawProfile::REAPER, kumi_daw_reaper::create_reaper_integration),
            Daw::Renoise => (DawProfile::RENOISE, kumi_daw_renoise::create_renoise_integration),
        };
        let places = kumi::daw::registry::places();
        let log = SessionLog::new(home, places, Rc::new(|text: &str| text.to_owned()));
        let emit = SessionLog::tee(&log, Rc::new(|_| {}));
        let watching: Rc<RefCell<Option<Session>>> = Rc::default();
        let (home_path, told, watcher) = (home.to_path_buf(), emit.clone(), watching.clone());
        let integration: IntegrationFactory = Box::new(move |listener: ConnectionListener| {
            let host = host.clone();
            let connect: Connect = Rc::new(move |_| {
                let host = host.clone();
                async move { Ok(host as Rc<dyn McpEndpoint>) }.boxed_local()
            });
            let mut context = IntegrationContext::new(listener);
            let (told, watcher) = (told.clone(), watcher.clone());
            context.on_change = Some(Rc::new(move |change: ChangeRecord| {
                let session = watcher.borrow().clone();
                if let Some(session) = session {
                    session.watch(WatchEvent::Change(change.clone()));
                }
                told(SessionEvent::Change { change });
            }));
            create(OwnOptions { context, kumi_home: home_path.clone(), connect: Some(connect) })
        });
        let model = Rc::new(Recorded::default());
        let mut options = SessionOptions::new(recording(model.clone()), integration, emit);
        options.profile = profile;
        options.timeout_ms = Some(20_000);
        options.close_timeout_ms = Some(5_000);
        options.conversations = Some(create_conversation_store_in(home, None, places));
        options.log = Some(log);
        let session = create_session(options).unwrap();
        *watching.borrow_mut() = Some(session.clone());
        session.start().await.unwrap();
        Kumi { session, model }
    }

    /// A request, answered as `steps` say.
    async fn ask(&self, question: &str, steps: Vec<Step>) {
        self.model.script.borrow_mut().extend(steps);
        self.session.submit(question, None).await.unwrap();
    }

    /// The newest `requests` requests taken back.
    async fn take_back(&self, requests: usize) -> TakenBack {
        let plan = self.session.take_back_plan().await.unwrap();
        self.session.take_back(plan[requests - 1].at).await.unwrap()
    }
}

fn titles(changes: &[ChangeRecord]) -> Vec<&str> {
    changes.iter().map(|change| change.title.as_str()).collect()
}

#[tokio::test(flavor = "current_thread")]
async fn a_request_taken_back_in_reaper_is_undone_in_reaper_newest_first_until_an_undo_fails() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let home = tempfile::tempdir().unwrap();
            let paths = BridgePaths::new(home.path(), Daw::Reaper);
            let script = ReaperScript::new(3, 0);
            script.set_project_id(Some("0123456789abcdef0123456789abcdef"));
            let _reaper = simulator(script.config(ensure_secret(&paths.secret()).unwrap(), "/lab/REAPER"), &paths).await;
            let kumi = Kumi::start(Daw::Reaper, home.path()).await;
            kumi.ask(
                "Turn Track 1 down.",
                vec![Box::new(|_| call("track_volume", json!({"ref": "track:1", "volumeDb": -6}))), Box::new(|_| answer("Done."))],
            )
            .await;
            kumi.ask(
                "Mute Track 2 and lower Track 3.",
                vec![
                    Box::new(|_| call("track_mute", json!({"ref": "track:2", "mute": true}))),
                    Box::new(|_| call("track_volume", json!({"ref": "track:3", "volumeDb": -3}))),
                    Box::new(|_| answer("Done.")),
                ],
            )
            .await;
            assert_eq!((script.volume(1), script.track(2)["mute"].clone(), script.volume(3)), (-6.0, json!(true), -3.0));
            // The last request taken back: its two changes go, newest first, each a new step of REAPER's undo.
            let steps = script.undo_history().len();
            let taken = kumi.take_back(1).await;
            assert_eq!((taken.requests, taken.words.as_deref()), (1, Some("Mute Track 2 and lower Track 3.")));
            assert_eq!(titles(&taken.undone), ["Track 3 volume -3 dB", "Track 2 muted"]);
            assert_eq!((script.volume(1), script.track(2)["mute"].clone(), script.volume(3)), (-6.0, json!(false), 0.0));
            assert_eq!(script.undo_history().len(), steps + 2);
            // The producer moved Track 1's fader since: Kumi leaves it, and the first request stays.
            script.set_volume(1, -3.0);
            let taken = kumi.take_back(1).await;
            assert_eq!((taken.requests, taken.undone.len()), (0, 0));
            let failed = taken.failed.expect("the undo that failed");
            assert_eq!((failed.title.as_str(), failed.state), ("Track 1 volume -6 dB", ChangeState::Kept));
            assert!(failed.note.as_deref().is_some_and(|note| note.contains("changed since Kumi set it")), "{:?}", failed.note);
            assert_eq!(script.volume(1), -3.0);
            let left = kumi.session.take_back_plan().await.unwrap();
            assert_eq!(left.iter().map(|asked| asked.words.as_str()).collect::<Vec<_>>(), ["Turn Track 1 down."]);
            kumi.session.close().await.unwrap();
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn requests_taken_back_in_renoise_are_undone_in_renoise_mute_and_all() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let home = tempfile::tempdir().unwrap();
            let paths = BridgePaths::new(home.path(), Daw::Renoise);
            let script = RenoiseScript::new();
            let _renoise = simulator(script.config_with_changes(ensure_secret(&paths.secret()).unwrap(), "/lab/Renoise"), &paths).await;
            let kumi = Kumi::start(Daw::Renoise, home.path()).await;
            kumi.ask(
                "Turn Track 1 down.",
                vec![Box::new(|_| call("track_volume", json!({"ref": "track:1", "volumeDb": -6}))), Box::new(|_| answer("Done."))],
            )
            .await;
            // A mute is no step of Renoise's own undo; Kumi's takes it back all the same.
            kumi.ask(
                "And mute it.",
                vec![Box::new(|_| call("track_mute", json!({"ref": "track:1", "mute": true}))), Box::new(|_| answer("Muted."))],
            )
            .await;
            assert_eq!((script.track_state(1).0, script.track_state(1).1), (-6.0, true));
            // Both taken back: the mute, then the fader.
            let taken = kumi.take_back(2).await;
            assert_eq!((taken.requests, taken.words.as_deref(), taken.failed.is_none()), (2, Some("Turn Track 1 down."), true));
            assert_eq!(taken.undone.len(), 2);
            assert!(taken.undone[0].title.contains("muted"), "{:?}", titles(&taken.undone));
            assert_eq!((script.track_state(1).0, script.track_state(1).1), (0.0, false));
            assert!(kumi.session.take_back_plan().await.unwrap().is_empty());
            kumi.session.close().await.unwrap();
        })
        .await;
}
