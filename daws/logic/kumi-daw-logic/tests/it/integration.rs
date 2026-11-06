//! Original M2 Logic integration cases, using a stand-in host and an injected project opener. No real app starts.
use async_trait::async_trait;
use futures::FutureExt;
use kumi_agent::core::{context::IntegrationContext, contracts::*};
use kumi_agent::mcp::client::{McpEndpoint, StderrStatus};
use kumi_agent::mcp::types::{CallToolResult, Implementation, ListToolsResult};
use kumi_common::abort::Signal;
use kumi_daw_logic::create_logic_integration;
use kumi_daw_own::bridge::{Connect, OwnOptions};
use kumi_types::{contracts::*, errors::RuntimeError};
use serde_json::{json, Value};
use std::{cell::RefCell, rc::Rc};
const PROJECT_ID: &str = "0123456789abcdef0123456789abcdef";
/// A host that answers each tool from a table, and remembers what it was asked.
struct Host {
    answers: RefCell<Vec<(&'static str, Value, bool)>>,
    asked: RefCell<Vec<(String, Value)>>,
}
#[async_trait(?Send)]
impl McpEndpoint for Host {
    fn pid(&self) -> Option<u32> {
        None
    }
    fn server_info(&self) -> Option<Implementation> {
        None
    }
    async fn list(&self, _: Option<&str>, _: Signal) -> Result<ListToolsResult, RuntimeError> {
        Ok(serde_json::from_value(json!({"tools": []})).unwrap())
    }
    async fn call(&self, name: &str, args: JsonObject, _: Signal) -> Result<CallToolResult, RuntimeError> {
        // (M6: catch-up, and keeping the project's snapshot, run beside the looks: a project first seen, unless a test
        // scripts them; not counted among what the test asked)
        if matches!(name, "catch_up" | "baseline_save") && !self.answers.borrow().iter().any(|(tool, _, _)| *tool == name) {
            let answer = if name == "catch_up" { json!({"first": true}) } else { json!({"saved": false}) };
            return Ok(
                serde_json::from_value(json!({"content": [{"type": "text", "text": answer.to_string()}], "isError": false})).unwrap()
            );
        }
        self.asked.borrow_mut().push((name.into(), Value::Object(args)));
        let answers = self.answers.borrow();
        let (_, answer, error) = answers.iter().rev().find(|(tool, _, _)| *tool == name).cloned().expect("an answer for each tool asked");
        Ok(serde_json::from_value(json!({"content": [{"type": "text", "text": answer.to_string()}], "isError": error})).unwrap())
    }
    fn on_catalog_changed(&self, _: Rc<dyn Fn()>) -> Box<dyn Fn()> {
        Box::new(|| {})
    }
    fn on_disconnect(&self, _: Rc<dyn Fn()>) -> Box<dyn Fn()> {
        Box::new(|| {})
    }
    fn stderr_status(&self) -> StderrStatus {
        StderrStatus { bytes: 0, truncated: false }
    }
    async fn close(&self) -> Result<(), RuntimeError> {
        Ok(())
    }
}

fn options(host: Rc<Host>, states: Rc<RefCell<Vec<ConnectionState>>>) -> OwnOptions {
    let connect: Connect = Rc::new(move |_| {
        let host = host.clone();
        async move { Ok(host as Rc<dyn McpEndpoint>) }.boxed_local()
    });
    OwnOptions {
        open_file: None,
        context: IntegrationContext::new(Rc::new(move |state, _| states.borrow_mut().push(state))),
        kumi_home: std::env::temp_dir(),
        connect: Some(connect),
    }
}

fn logic_snapshot() -> Value {
    json!({
        "epoch": "1",
        "project": {"id": PROJECT_ID, "identity": "Scratch", "name": "Scratch", "path": "/lab/Scratch.logicx", "tempo": 160.0, "timeSignature": "4/4"},
        "transport": {"playing": false, "recording": false, "cycle": false},
        "tracks": [
            {"ref": "1:track:1", "index": 1, "name": "Vocal", "kind": "audio", "selected": true},
            {"ref": "1:track:2", "index": 2, "name": "Keys", "kind": "instrument"},
        ],
        "selection": {"trackRef": "1:track:1", "regionRef": "1:region:1"},
    })
}

/// Logic's bridge reads Logic through Accessibility: while its helper isn't trusted, a look fails (never busy) and
/// the observation says what to allow, in Logic's words, as the status's `permission` names it. Granted, the next look
/// reads the project at Logic's place, and Logic's regions and plug-ins get this turn's short refs.
#[tokio::test(flavor = "current_thread")]
async fn logics_look_without_accessibility_says_what_to_allow_and_never_counts_as_busy() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let refused = json!({"error": "Accessibility isn't granted to Kumi's helper", "code": "failed", "connected": true});
            let host = Rc::new(Host {
                answers: RefCell::new(vec![
                    ("status", json!({"connected": true, "busy": false, "permission": "accessibility"}), false),
                    ("snapshot", refused, true),
                ]),
                asked: RefCell::default(),
            });
            let states = Rc::new(RefCell::new(vec![]));
            let logic = create_logic_integration(options(host.clone(), states.clone()));
            logic.start(Signal::new()).await.unwrap();
            assert_eq!(*states.borrow(), [ConnectionState::Connecting, ConnectionState::Connected], "connected, though it can't read yet");
            let seen = logic.observe(Signal::new(), None).await.unwrap();
            assert!(seen.label.starts_with("Logic Pro — couldn't look just now: Kumi's helper needs Accessibility"), "{}", seen.label);
            assert!(seen.label.contains("System Settings > Privacy & Security > Accessibility"), "{}", seen.label);
            let tools: Vec<String> = seen.tools.iter().map(|t| t.name().to_string()).collect();
            assert_eq!(tools[0], "logic_read", "{tools:?}");
            assert!(tools.iter().any(|t| t == "make_changes") && tools.iter().any(|t| t == "undo_change"), "{tools:?}");
            assert!(!states.borrow().contains(&ConnectionState::Disconnected), "lacking a permission isn't gone: {:?}", states.borrow());
            // Granted: the project reads, at Logic's own place, in short refs.
            host.answers.borrow_mut().push(("status", json!({"connected": true, "busy": false, "permission": null}), false));
            host.answers.borrow_mut().push(("snapshot", logic_snapshot(), false));
            let read = logic.observe(Signal::new(), None).await.unwrap();
            assert_eq!(read.label, "Logic Pro — Scratch");
            assert_eq!(read.key, format!("logic:{PROJECT_ID}"));
            assert_eq!(read.project.as_ref().map(|p| p.id.as_str()), Some(format!("logic-{PROJECT_ID}").as_str()));
            let context: Value = serde_json::from_str(&read.context).unwrap();
            assert_eq!((context["daw"].as_str(), context["project"]["tempo"].as_f64()), (Some("Logic Pro"), Some(160.0)), "{context}");
            assert_eq!(context["project"]["file"], "Scratch.logicx");
            assert_eq!(context["tracks"][1]["ref"], "track:2");
            assert_eq!(context["selection"]["regionRef"], "region:1", "a region's ref is this turn's too");
            assert!(!read.context.contains("1:track:") && !read.context.contains("1:region:"), "{}", read.context);
            // The read tool passes the region back long, and names the kinds under their parents.
            host.answers.borrow_mut().push(("read", json!({"items": [], "total": 0, "epoch": "1"}), false));
            let tool = read.tools[0].clone();
            let _ = tool.execute(json!({"kind": "note", "parent": "region:1"}).as_object().unwrap().clone(), Signal::new()).await;
            assert_eq!(host.asked.borrow().last().unwrap().1, json!({"kind": "note", "parent": "1:region:1"}));
            for words in ["note under a region", "region under a track", "plugin under a track", "bar, beat, division"] {
                assert!(tool.description().contains(words), "{words}: {}", tool.description());
            }
        })
        .await;
}

/// Logic's recording state is its transport's, as the focus carries it (none while the bridge lacks Accessibility,
/// and never busy), and a kept conversation's project opens again from where a look saw it: the integration keeps
/// each saved project's file by its place, says when it doesn't know one, and hands the file to Logic's opener (here
/// a stand-in), whose refusal is passed on in its words.
#[tokio::test(flavor = "current_thread")]
async fn logics_recording_is_its_transports_and_a_kept_conversations_project_opens_from_a_look() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let focus = |transport: Value| json!({"epoch": "1", "view": "tracks", "selection": {"trackRef": "1:track:1"}, "transport": transport});
            let status = json!({"connected": true, "busy": false, "permission": null, "resource": "com.apple.logic10", "project": {"id": PROJECT_ID, "name": "Scratch", "path": "/lab/Scratch.logicx"}});
            let host = Rc::new(Host {
                answers: RefCell::new(vec![
                    ("status", status, false),
                    ("snapshot", logic_snapshot(), false),
                    ("focus", focus(json!({"playing": true, "recording": true, "cycle": false})), false),
                ]),
                asked: RefCell::default(),
            });
            let states = Rc::new(RefCell::new(vec![]));
            let asked_to_open: Rc<RefCell<Vec<String>>> = Rc::default();
            let mut options = options(host.clone(), states.clone());
            let into = asked_to_open.clone();
            options.open_file = Some(Rc::new(move |path| {
                into.borrow_mut().push(path.clone());
                async move { Err(format!("Logic Pro didn't take {path}.")) }.boxed_local()
            }));
            let logic = create_logic_integration(options);
            assert_eq!(logic.recording(Signal::new()).await.unwrap(), None, "not connected yet");
            logic.start(Signal::new()).await.unwrap();
            assert_eq!(logic.open_set().as_deref(), Some("/lab/Scratch.logicx"), "the status's project, before any look");
            assert_eq!(logic.recording(Signal::new()).await.unwrap(), Some(true));
            host.answers.borrow_mut().push(("focus", focus(json!({"playing": false, "recording": false, "cycle": false})), false));
            assert_eq!(logic.recording(Signal::new()).await.unwrap(), Some(false));
            host.answers.borrow_mut().push(("focus", json!({"epoch": "1", "view": "tracks", "selection": {}}), false));
            assert_eq!(logic.recording(Signal::new()).await.unwrap(), None, "a transport that doesn't say");
            let refused = json!({"error": "Accessibility isn't granted to Kumi's helper", "code": "failed", "connected": true});
            host.answers.borrow_mut().push(("focus", refused, true));
            assert_eq!(logic.recording(Signal::new()).await.unwrap(), None, "refused in words: not busy, and nothing to tell");
            assert!(!states.borrow().contains(&ConnectionState::Disconnected), "{:?}", states.borrow());

            // A place this run never saw can't be opened; the project a look saw opens from its file.
            let unknown = format!("logic-{}", "f".repeat(32));
            assert_eq!(logic.open_place(&unknown, Signal::new()).await.unwrap(), Err("Kumi doesn't know where that project is.".into()));
            let seen = logic.observe(Signal::new(), None).await.unwrap();
            let place = seen.project.as_ref().map(|project| project.id.clone()).expect("a saved project's place");
            assert_eq!(place, format!("logic-{PROJECT_ID}"));
            let refused = logic.open_place(&place, Signal::new()).await.unwrap();
            assert_eq!(refused, Err("Logic Pro didn't take /lab/Scratch.logicx.".into()));
            assert_eq!(*asked_to_open.borrow(), ["/lab/Scratch.logicx"]);
            logic.close().await.unwrap();
            assert_eq!(logic.open_place(&place, Signal::new()).await.unwrap(), Err("Kumi isn't connected to Logic Pro.".into()));
            assert_eq!(logic.recording(Signal::new()).await.unwrap(), None);
        })
        .await;
}

/// Logic's busy words are its own: a dialog, a window on another desktop, a project still opening.
#[tokio::test(flavor = "current_thread")]
async fn logics_busy_words_name_a_dialog_another_desktop_or_a_project_still_opening() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let focus =
                |busy: &str| json!({"epoch": "1", "view": "tracks", "selection": {}, "transport": {"playing": false}, "busy": busy});
            let host = Rc::new(Host {
                answers: RefCell::new(vec![
                    ("status", json!({"connected": true, "busy": "modal"}), false),
                    ("focus", focus("modal"), false),
                ]),
                asked: RefCell::default(),
            });
            let seen: Rc<RefCell<Vec<String>>> = Rc::default();
            let into = seen.clone();
            let changed = Rc::new(tokio::sync::Notify::new());
            let notify = changed.clone();
            let mut options = options(host.clone(), Rc::new(RefCell::new(vec![])));
            options.context.on_focus = Some(Rc::new(move |focus| {
                if let Some(busy) = focus.and_then(|focus| focus.busy) {
                    into.borrow_mut().push(busy);
                    notify.notify_one();
                }
            }));
            options.context.focus_interval_ms = Some(20);
            let logic = create_logic_integration(options);
            logic.start(Signal::new()).await.unwrap();
            for (code, words) in [
                ("modal", "Logic Pro is showing a dialog"),
                ("away", "Logic Pro's window is on another desktop"),
                ("loading", "Logic Pro is still opening the project"),
            ] {
                host.answers.borrow_mut().push(("focus", focus(code), false));
                tokio::time::timeout(std::time::Duration::from_secs(5), async {
                    loop {
                        let next = changed.notified();
                        if seen.borrow().iter().any(|said| said == words) {
                            break;
                        }
                        next.await;
                    }
                })
                .await
                .unwrap();
                assert!(seen.borrow().iter().any(|said| said == words), "{code}: {:?}", seen.borrow());
            }
            logic.close().await.unwrap();
        })
        .await;
}
