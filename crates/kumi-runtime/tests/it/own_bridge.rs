//! REAPER's, Renoise's and Logic's integrations over Kumi's host, with a stand-in host that answers as the M2 contract says:
//! the observation carries the project as the DAW names it, its tracks in short refs, and the conversation's place;
//! the model's read pages in short refs and passes them back long; a DAW that isn't there is said in its words.
use async_trait::async_trait;
use futures::FutureExt;
use kumi_common::abort::Signal;
use kumi_runtime::core::{
    context::IntegrationContext,
    contracts::*,
    errors::RuntimeError,
    view::{BodyRequest, FocusBody},
};
use kumi_runtime::integrations::{
    logic::create_logic_integration,
    own_bridge::{Connect, OwnOptions},
    reaper::create_reaper_integration,
    renoise::create_renoise_integration,
};
use kumi_runtime::mcp::client::{McpEndpoint, StderrStatus};
use kumi_runtime::mcp::types::{CallToolResult, Implementation, ListToolsResult};
use serde_json::{json, Value};
use std::{cell::RefCell, rc::Rc};

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
        context: IntegrationContext::new(Rc::new(move |state, _| states.borrow_mut().push(state))),
        kumi_home: std::env::temp_dir(),
        connect: Some(connect),
        open_file: None,
    }
}

const PROJECT_ID: &str = "0123456789abcdef0123456789abcdef";
const BASS: &str = "r:track:{8A5D2C1E-0000-4000-8000-000000000002}";

fn reaper_snapshot() -> Value {
    json!({
        "epoch": "1",
        "project": {"id": PROJECT_ID, "identity": "4242:0x7f01", "name": "Night Drive", "path": "/Users/someone/Music/Night Drive.rpp", "tempo": 124, "timeSignature": [4, 4], "playState": "stopped", "loop": {"on": false}},
        "transport": {"playing": false, "position": 0, "loop": {"on": false}},
        "tracks": [
            {"ref": "r:track:{8A5D2C1E-0000-4000-8000-000000000001}", "index": 1, "name": "Drums", "depth": 0, "folder": 1},
            {"ref": BASS, "index": 2, "name": "Bass", "depth": 0, "folder": 0, "selected": true},
        ],
        "selection": {"trackRef": BASS},
    })
}

#[tokio::test(flavor = "current_thread")]
async fn reapers_observation_is_its_project_in_short_refs_at_its_place() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let host = Rc::new(Host {
                answers: RefCell::new(vec![("status", json!({"connected": true}), false), ("snapshot", reaper_snapshot(), false)]),
                asked: RefCell::default(),
            });
            let states = Rc::new(RefCell::new(vec![]));
            let reaper = create_reaper_integration(options(host.clone(), states.clone()));
            reaper.start(Signal::new()).await.unwrap();
            assert_eq!(*states.borrow(), [ConnectionState::Connecting, ConnectionState::Connected]);
            let seen = reaper.observe(Signal::new(), None).await.unwrap();
            assert_eq!(seen.label, "REAPER — Night Drive");
            assert_eq!(seen.project.as_ref().map(|p| p.id.as_str()), Some(format!("reaper-{PROJECT_ID}").as_str()));
            assert_eq!(seen.set.as_deref(), Some("4242:0x7f01"));
            assert_eq!(seen.tracks.as_deref(), Some(&["Drums".to_string(), "Bass".to_string()][..]));
            let context: Value = serde_json::from_str(&seen.context).unwrap();
            assert_eq!(context["daw"], "REAPER");
            assert_eq!(context["project"]["name"], "Night Drive");
            // Kumi's identity for the project, the folders it's in and the transport's second copy stay out.
            assert_eq!(context["project"]["file"], "Night Drive.rpp");
            for key in ["identity", "path", "playState", "loop"] {
                assert!(context["project"].get(key).is_none(), "{key}: {}", context["project"]);
            }
            assert!(!seen.context.contains("/Users/someone"), "{}", seen.context);
            assert_eq!(context["tracks"][1]["ref"], "track:2");
            assert_eq!(context["selection"]["trackRef"], "track:2");
            assert!(!seen.context.contains("{8A5D"), "no GUID reaches the model: {}", seen.context);
            // With every request: the read, REAPER's core change kinds and actions, the plan and the undo (the other kinds wait
            // behind find_tools), and the variations tool (#401: SFX variations as items).
            assert_eq!(
                seen.tools.iter().filter(|tool| !tool.deferred()).map(|tool| tool.name().to_string()).collect::<Vec<_>>(),
                [
                    "reaper_read",
                    "track_volume",
                    "track_pan",
                    "track_mute",
                    "track_add",
                    "tempo_set",
                    "fx_add",
                    "fx_param",
                    "notes_add",
                    "play",
                    "stop",
                    "make_changes",
                    "undo_change",
                    "variations"
                ]
            );

            // The model reads the Bass track's items in short refs; the host is asked in long ones.
            host.answers.borrow_mut().push((
                "read",
                json!({"items": [{"ref": "r:item:{FF00-1}", "position": 4.0, "length": 8.0, "name": "Bass DI", "takeRef": "r:take:{FF00-2}"}], "epoch": "1"}),
                false,
            ));
            let read = seen.tools[0].clone();
            let page = read.execute(json!({"kind": "item", "parent": "track:2"}).as_object().unwrap().clone(), Signal::new()).await.unwrap();
            assert!(!page.is_error, "{}", page.text);
            // Lengthened, with the epoch the track was read in.
            assert_eq!(host.asked.borrow().last().unwrap().1, json!({"kind": "item", "parent": BASS, "epoch": "1"}));
            let page: Value = serde_json::from_str(&page.text).unwrap();
            assert_eq!(page["items"][0]["ref"], "item:1");
            assert_eq!(page["items"][0]["takeRef"], "take:1");
        })
        .await;
}

/// (#361 review K14) The project's fingerprint, for what Kumi logs of the producer's reactions: from the last look.
#[tokio::test(flavor = "current_thread")]
async fn a_projects_fingerprint_is_its_name_tempo_meter_and_the_roles_its_tracks_play() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let host = Rc::new(Host {
                answers: RefCell::new(vec![("status", json!({"connected": true}), false), ("snapshot", reaper_snapshot(), false)]),
                asked: RefCell::default(),
            });
            let reaper = create_reaper_integration(options(host, Rc::new(RefCell::new(vec![]))));
            reaper.start(Signal::new()).await.unwrap();
            assert_eq!(reaper.fingerprint(), None, "not before a look");
            reaper.observe(Signal::new(), None).await.unwrap();
            let print = reaper.fingerprint().unwrap();
            assert_eq!(print["name"], "Night Drive");
            assert_eq!(print["tempo"], 124);
            assert_eq!(print["meter"], "4/4");
            assert_eq!(print["roles"], json!({"bass": 1, "drums": 1}));
            assert_eq!(print["tokens"], json!(["drums", "bass"]));
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn an_unsaved_project_is_the_dawss_unsaved_place_and_a_daw_not_there_is_said_in_its_words() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let mut snapshot = reaper_snapshot();
            snapshot["project"]["id"] = Value::Null;
            let host = Rc::new(Host {
                answers: RefCell::new(vec![("status", json!({"connected": true}), false), ("snapshot", snapshot, false)]),
                asked: RefCell::default(),
            });
            let states = Rc::new(RefCell::new(vec![]));
            let reaper = create_reaper_integration(options(host.clone(), states.clone()));
            reaper.start(Signal::new()).await.unwrap();
            let seen = reaper.observe(Signal::new(), None).await.unwrap();
            assert!(seen.project.is_none());
            assert_eq!(seen.set.as_deref(), Some("4242:0x7f01"));

            // REAPER quits: the host says so, and Kumi carries on without it, in REAPER's words.
            host.answers.borrow_mut().push((
                "snapshot",
                json!({"error": "no REAPER with Kumi's bridge is running", "connected": false}),
                true,
            ));
            let away = reaper.observe(Signal::new(), None).await.unwrap();
            assert!(away.label.contains("No REAPER access"), "{}", away.label);
            assert!(away.label.contains("no REAPER with Kumi's bridge is running"), "{}", away.label);
            assert!(away.tools.is_empty());
            assert_eq!(states.borrow().last(), Some(&ConnectionState::Disconnected));
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn renoises_refs_are_positions_under_the_songs_epoch_and_a_new_song_retires_them() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let snapshot = |epoch: &str, name: &str| {
                json!({
                    "epoch": epoch,
                    "project": {"id": null, "identity": format!("99:{epoch}"), "name": name, "bpm": 140, "lpb": 4},
                    "transport": {"playing": true},
                    "tracks": [{"ref": format!("{epoch}:track:1"), "index": 1, "name": "Kick", "type": "sequencer"}],
                    "selection": {"trackRef": format!("{epoch}:track:1")},
                })
            };
            let host = Rc::new(Host {
                answers: RefCell::new(vec![("status", json!({"connected": true}), false), ("snapshot", snapshot("3", "Acid"), false)]),
                asked: RefCell::default(),
            });
            let renoise = create_renoise_integration(options(host.clone(), Rc::new(RefCell::new(vec![]))));
            renoise.start(Signal::new()).await.unwrap();
            let seen = renoise.observe(Signal::new(), None).await.unwrap();
            assert_eq!(seen.label, "Renoise — Acid");
            let context: Value = serde_json::from_str(&seen.context).unwrap();
            assert_eq!(context["song"]["bpm"], 140, "Renoise's project is its song");
            assert_eq!(context["tracks"][0]["ref"], "track:1");

            // Another song opens: the old refs are retired, and the new song's tracks get names never used before, so
            // a stale track:1 in the conversation can't read the new song's first track.
            host.answers.borrow_mut().push(("snapshot", snapshot("4", "Breaks"), false));
            let next = renoise.observe(Signal::new(), None).await.unwrap();
            let context: Value = serde_json::from_str(&next.context).unwrap();
            assert_eq!(context["tracks"][0]["ref"], "track:2");
            host.answers.borrow_mut().push(("read", json!({"items": [], "epoch": "4"}), false));
            next.tools[0]
                .execute(json!({"kind": "device", "parent": "track:2"}).as_object().unwrap().clone(), Signal::new())
                .await
                .unwrap();
            assert_eq!(host.asked.borrow().last().unwrap().1["parent"], "4:track:1", "the new name reads the new song's track");
            let asked_before = host.asked.borrow().len();
            let retired = next.tools[0]
                .execute(json!({"kind": "device", "parent": "track:1"}).as_object().unwrap().clone(), Signal::new())
                .await
                .unwrap();
            assert!(retired.is_error && retired.text.contains("isn't one of this turn's refs"), "{}", retired.text);
            assert_eq!(host.asked.borrow().len(), asked_before, "the retired name reads nothing of the new song: Renoise isn't asked");
        })
        .await;
}

/// (m7) Each look carries the producer's setup as the bridge read it, with the instructions to respect it; a render folder
/// outside the project is named by its last folder only.
#[tokio::test(flavor = "current_thread")]
async fn each_look_carries_the_producers_setup_and_the_instructions_to_respect_it() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let mut snapshot = reaper_snapshot();
            snapshot["setup"] = json!({
                "newItemFades": "off", "loopSourceOfNewItems": {"imported": false, "midi": true}, "newTrackVolumeDb": -6.0,
                "extensions": {"SWS": true, "ReaPack": true, "ReaImGui": false, "js_ReaScriptAPI": false},
                "render": {"source": "region render matrix", "bounds": "all project regions", "channels": 2, "sampleRate": 48000,
                           "pattern": "$region", "folder": "/Users/someone/Game/Renders/"},
            });
            let host = Rc::new(Host {
                answers: RefCell::new(vec![("status", json!({"connected": true}), false), ("snapshot", snapshot, false)]),
                asked: RefCell::default(),
            });
            let reaper = create_reaper_integration(options(host.clone(), Rc::new(RefCell::new(vec![]))));
            reaper.start(Signal::new()).await.unwrap();
            let seen = reaper.observe(Signal::new(), None).await.unwrap();
            let context: Value = serde_json::from_str(&seen.context).unwrap();
            assert_eq!(context["setup"]["newItemFades"], "off");
            assert_eq!(context["setup"]["loopSourceOfNewItems"]["imported"], false);
            assert_eq!(context["setup"]["extensions"]["SWS"], true);
            assert_eq!(context["setup"]["render"]["pattern"], "$region");
            assert_eq!(context["setup"]["render"]["folder"], "…/Renders");
            assert!(!seen.context.contains("/Users/someone"), "{}", seen.context);
            for words in ["newItemFades", "loopSourceOfNewItems", "setup.extensions", "File > Render"] {
                assert!(seen.instructions.contains(words), "{words}");
            }

            let song = json!({
                "epoch": "3",
                "project": {"id": null, "identity": "99:3", "name": "Amen", "bpm": 340, "lpb": 4},
                "transport": {"playing": false},
                "tracks": [{"ref": "3:track:1", "index": 1, "name": "Break", "type": "sequencer"}],
                "selection": {"trackRef": "3:track:1"},
                "setup": {"patternLines": 128, "groups": [{"name": "Drums", "color": "#ff0000", "tracks": 3}], "tools": ["Paketti"], "paketti": true},
            });
            let host = Rc::new(Host {
                answers: RefCell::new(vec![("status", json!({"connected": true}), false), ("snapshot", song, false)]),
                asked: RefCell::default(),
            });
            let renoise = create_renoise_integration(options(host.clone(), Rc::new(RefCell::new(vec![]))));
            renoise.start(Signal::new()).await.unwrap();
            let seen = renoise.observe(Signal::new(), None).await.unwrap();
            let context: Value = serde_json::from_str(&seen.context).unwrap();
            assert_eq!(context["setup"]["patternLines"], 128);
            assert_eq!(context["setup"]["groups"][0]["name"], "Drums");
            assert_eq!(context["setup"]["paketti"], true);
            for words in ["patternLines", "paketti", "2× or 4×"] {
                assert!(seen.instructions.contains(words), "{words}");
            }
        })
        .await;
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn with_nothing_drawing_the_focus_the_daw_is_asked_about_once_a_second() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let focus = json!({"epoch": "1", "view": "arrange", "selection": {}, "transport": {"playing": false}});
            let mut calls = vec![];
            for drawn in [true, false] {
                let host = Rc::new(Host {
                    answers: RefCell::new(vec![("status", json!({"connected": true}), false), ("focus", focus.clone(), false)]),
                    asked: RefCell::default(),
                });
                let mut options = options(host.clone(), Rc::new(RefCell::new(vec![])));
                options.context.on_focus = Some(Rc::new(|_| {}));
                options.context.focus_drawn = drawn;
                let reaper = create_reaper_integration(options);
                reaper.start(Signal::new()).await.unwrap();
                tokio::time::sleep(std::time::Duration::from_millis(3_050)).await;
                reaper.close().await.unwrap();
                calls.push(host.asked.borrow().iter().filter(|(tool, _)| tool == "focus").count());
            }
            assert!(calls[0] >= 15, "FOCUS on screen follows the DAW closely: {calls:?}");
            assert_eq!(calls[1], 3, "plain mode asks once a second: {calls:?}");
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn reapers_focus_is_read_while_focus_shows_and_its_bodies_come_neutral() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let focus = json!({
                "epoch": "1", "view": "midi", "changed": 7,
                "selection": {"trackRef": BASS, "track": {"name": "Bass", "color": "#3c7cf5", "type": "midi"}, "itemRef": "r:item:{I1}", "takeRef": "r:take:{T1}", "itemName": "Bass take", "fxRef": "r:fx:{F1}", "fxName": "ReaComp", "param": {"name": "Thresh", "value": "-12.0 dB"}},
                "transport": {"playing": true, "beat": 33.5, "tempo": 124, "beatsPerBar": 4},
            });
            let host = Rc::new(Host {
                answers: RefCell::new(vec![("status", json!({"connected": true}), false), ("focus", focus, false)]),
                asked: RefCell::default(),
            });
            let seen: Rc<RefCell<Vec<Option<kumi_runtime::core::view::DawFocus>>>> = Rc::default();
            let beats: Rc<RefCell<Vec<Option<f64>>>> = Rc::default();
            let mut options = options(host.clone(), Rc::new(RefCell::new(vec![])));
            let (into, ticks) = (seen.clone(), beats.clone());
            options.context.on_focus = Some(Rc::new(move |focus| into.borrow_mut().push(focus)));
            options.context.on_transport = Some(Rc::new(move |transport| ticks.borrow_mut().push(transport.and_then(|t| t.beat))));
            options.context.focus_interval_ms = Some(50);
            let reaper = create_reaper_integration(options);
            reaper.start(Signal::new()).await.unwrap();
            tokio::time::sleep(std::time::Duration::from_millis(180)).await;
            let focus = seen.borrow().first().cloned().flatten().expect("the focus, read once it shows");
            assert_eq!(seen.borrow().len(), 1, "told again only when it changes");
            assert!(beats.borrow().len() >= 2, "the beat light follows every read while playing");
            assert_eq!(beats.borrow()[0], Some(33.5));
            assert_eq!(focus.track.as_ref().map(|t| t.name.as_str()), Some("Bass"));
            assert_eq!(focus.track_ref.as_deref(), Some(BASS));
            assert_eq!(focus.item.as_ref().and_then(|i| i.item_ref.as_deref()), Some("r:take:{T1}"), "the MIDI editor's notes are the take's");
            assert_eq!(focus.editor_view.as_ref().map(|v| v.label.as_str()), Some("MIDI editor"));
            assert_eq!(focus.device.as_deref(), Some("ReaComp"));
            assert_eq!(focus.parameter.as_ref().map(|p| p.name.as_str()), Some("Thresh"));

            use kumi_runtime::core::view::{BodyKind, BodyRequest, FocusBody, RouteKind};
            assert!(reaper.can_show(BodyKind::Notes) && reaper.can_show(BodyKind::Routing) && !reaper.can_show(BodyKind::Tracker));
            host.answers.borrow_mut().push((
                "focus_body",
                json!({"nodes": [
                    {"ref": "r:fx:{F1}", "name": "ReaComp", "enabled": true},
                    {"ref": "r:fx:{C1}", "name": "Container", "container": true, "childNodes": [
                        {"ref": "r:fx:{A}", "name": "ReaEQ"}, {"ref": "r:fx:{B}", "name": "ReaDelay", "parallel": true}
                    ]}
                ]}),
                false,
            ));
            let Some(FocusBody::Tree(tree)) = reaper.focus_body(&BodyRequest::Tree { track_ref: BASS.into() }, Signal::new()).await.unwrap() else {
                panic!("a tree")
            };
            assert_eq!(host.asked.borrow().last().unwrap().1, json!({"kind": "fx", "ref": BASS}));
            let chains = tree.devices[1].chains.as_ref().unwrap();
            assert_eq!(chains.len(), 1, "a container keeps its one chain");
            let delay = &chains[0].devices.as_ref().unwrap()[1];
            assert_eq!((delay.name.as_str(), delay.note.as_deref()), ("ReaDelay", Some("parallel")), "a parallel FX says so beside its name");

            host.answers.borrow_mut().push((
                "focus_body",
                json!({"routes": [{"category": 0, "otherName": "Reverb Bus", "volumeDb": -12.0, "mode": 0}, {"category": -1, "otherName": "Kick", "mode": 3, "mute": true}], "parentSend": true}),
                false,
            ));
            let Some(FocusBody::Routing(routing)) = reaper.focus_body(&BodyRequest::Routing { track_ref: BASS.into() }, Signal::new()).await.unwrap() else {
                panic!("routing")
            };
            assert_eq!((routing.routes[0].kind, routing.routes[0].tap.as_deref()), (RouteKind::Send, Some("post-fader")));
            assert_eq!((routing.routes[1].kind, routing.routes[1].muted), (RouteKind::Receive, true));
            assert_eq!(routing.parent_send, Some(true));

            // The arrange view: a screenful of lanes asked for, never a page of every track, and how many there are.
            host.answers.borrow_mut().push((
                "focus_body",
                json!({"length": 64, "beatsPerBar": 4, "playhead": 8, "trackCount": 200, "tracks": [{"ref": BASS, "name": "Bass", "depth": 0, "items": []}], "nextCursor": "c1.1.24"}),
                false,
            ));
            let Some(FocusBody::Timeline(strip)) = reaper.focus_body(&BodyRequest::Timeline, Signal::new()).await.unwrap() else { panic!("the arrange view") };
            assert_eq!(host.asked.borrow().last().unwrap().1, json!({"kind": "arrange", "limit": 24, "around": BASS}), "centred on the selected track");
            assert_eq!((strip.lanes.as_ref().map(Vec::len), strip.track_count), (Some(1), Some(200)));
            reaper.close().await.unwrap();
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn renoises_bodies_are_its_pattern_matrix_and_keyzones_in_its_own_notation() {
    tokio::task::LocalSet::new()
        .run_until(async {
            use kumi_runtime::core::view::{BodyRequest, FocusBody};
            let host = Rc::new(Host { answers: RefCell::new(vec![("status", json!({"connected": true}), false)]), asked: RefCell::default() });
            let renoise = create_renoise_integration(options(host.clone(), Rc::new(RefCell::new(vec![]))));
            renoise.start(Signal::new()).await.unwrap();
            host.answers.borrow_mut().push((
                "focus_body",
                json!({"ref": "3:pattern:2", "name": "Drop", "lines": 64, "from": 1, "cursor": 2, "playing": 1, "lpb": 4, "tracks": [
                    {"ref": "3:track:1", "name": "Kick", "lines": [
                        {"line": 1, "notes": [{"note": "C-4", "instrument": "01"}], "effects": [{"number": "0A", "amount": "10"}]},
                        {"line": 2, "notes": [{"note": "OFF"}]}
                    ]}
                ]}),
                false,
            ));
            let Some(FocusBody::Tracker(grid)) = renoise.focus_body(&BodyRequest::Tracker { item_ref: Some("3:pattern:2".into()) }, Signal::new()).await.unwrap() else {
                panic!("a pattern")
            };
            assert_eq!(host.asked.borrow().last().unwrap().1, json!({"kind": "pattern", "ref": "3:pattern:2"}));
            assert_eq!((grid.from, grid.cursor, grid.playing), (0, Some(1), Some(0)), "lines from 1 in Renoise, from 0 in FOCUS");
            assert_eq!(grid.tracks[0].cells, ["C-4 01 .. .. .. 0A10", "OFF .. .. .. .. ...."], "each column kept in its place");

            renoise.focus_body(&BodyRequest::Tracker { item_ref: Some("3:phrase:1.2".into()) }, Signal::new()).await.unwrap();
            assert_eq!(host.asked.borrow().last().unwrap().1["kind"], "phrase", "a phrase is asked for as one");

            host.answers.borrow_mut().push((
                "focus_body",
                json!({"tracks": ["Kick", "Bass"], "slots": [{"pattern": 1, "section": "Intro"}, {"pattern": 2, "mutedTracks": [2]}], "current": 2}),
                false,
            ));
            let Some(FocusBody::Matrix(matrix)) = renoise.focus_body(&BodyRequest::Matrix, Signal::new()).await.unwrap() else { panic!("the matrix") };
            assert_eq!((matrix.current, matrix.slots[1].muted.as_slice()), (Some(1), &[1][..]));
            assert_eq!(matrix.slots[0].section.as_deref(), Some("Intro"));

            host.answers.borrow_mut().push((
                "focus_body",
                json!({"name": "Drumkit", "keyzones": [{"sample": "Kick", "noteRange": [60, 60], "velocityRange": [0, 127], "baseNote": 60}], "macros": [{"name": "Drive", "value": 0.4}], "phrases": ["Fill"]}),
                false,
            ));
            let Some(FocusBody::Keyzones(map)) =
                renoise.focus_body(&BodyRequest::Keyzones { item_ref: "3:instrument:1".into() }, Signal::new()).await.unwrap()
            else {
                panic!("keyzones")
            };
            assert_eq!((map.zones[0].notes, map.macros.len(), map.phrases.len()), ((60, 60), 1, 1));
            assert!(renoise.focus_body(&BodyRequest::Timeline, Signal::new()).await.unwrap().is_none(), "Renoise has no timeline strip");
            renoise.close().await.unwrap();
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn renoises_focus_is_its_middle_frame_with_the_panes_open_beside_it() {
    tokio::task::LocalSet::new()
        .run_until(async {
            use kumi_runtime::core::view::ViewKind;
            let focus = json!({
                "epoch": "3", "view": "pattern", "matrixVisible": true, "dspVisible": true, "changed": 2,
                "selection": {"trackRef": "3:track:2", "track": {"name": "Bass", "type": "sequencer"}, "patternRef": "3:pattern:4", "patternName": "Drop", "sequence": 3, "deviceRef": "3:device:2.2", "deviceName": "Delay", "line": 12},
                "transport": {"playing": true, "followPlayer": true, "beat": 8.0, "tempo": 128, "beatsPerBar": 4},
            });
            let host = Rc::new(Host {
                answers: RefCell::new(vec![("status", json!({"connected": true}), false), ("focus", focus, false)]),
                asked: RefCell::default(),
            });
            let seen: Rc<RefCell<Vec<Option<kumi_runtime::core::view::DawFocus>>>> = Rc::default();
            let mut options = options(host.clone(), Rc::new(RefCell::new(vec![])));
            let into = seen.clone();
            options.context.on_focus = Some(Rc::new(move |focus| into.borrow_mut().push(focus)));
            options.context.focus_interval_ms = Some(50);
            let renoise = create_renoise_integration(options);
            renoise.start(Signal::new()).await.unwrap();
            tokio::time::sleep(std::time::Duration::from_millis(120)).await;
            let focus = seen.borrow().first().cloned().flatten().expect("the focus");
            let kind = |view: Option<&kumi_runtime::core::view::FocusView>| view.map(|v| v.kind);
            assert_eq!(kind(focus.main_view.as_ref()), Some(ViewKind::PatternEditor), "the middle frame");
            assert_eq!(focus.editor_view.as_ref().map(|v| v.label.as_str()), Some("Track DSPs"), "the DSP frame below it");
            assert_eq!(focus.panes.iter().flatten().map(|v| v.kind).collect::<Vec<_>>(), [ViewKind::Sequencer], "the matrix beside it");
            assert_eq!(focus.section_follows, Some(true), "playing and following: the sequence slot is playback's");
            assert_eq!(focus.section.as_ref().and_then(|s| s.index), Some(2.), "slots from 1 in Renoise, from 0 in FOCUS");
            assert_eq!(focus.item.as_ref().and_then(|i| i.item_ref.as_deref()), Some("3:pattern:4"));
            assert_eq!(focus.line, Some(12), "the pattern's edit line, by Renoise's numbers (review U7)");
            renoise.close().await.unwrap();
        })
        .await;
}

/// A project bigger than the snapshot: the model is told how many tracks there are and how to read the rest, never
/// shown 200 as if they were all.
#[tokio::test(flavor = "current_thread")]
async fn a_project_bigger_than_the_snapshot_says_so() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let mut snapshot = reaper_snapshot();
            // (review K38: REAPER sends at most 100 of its 1,250 markers, and names the total)
            let markers: Vec<Value> =
                (0..100).map(|n| json!({"ref": format!("r:marker:{{M{n}}}"), "name": format!("M{n}"), "position": n})).collect();
            snapshot["markers"] = json!(markers);
            snapshot["truncated"] =
                json!({"tracks": {"total": 1000, "nextCursor": "c1.1.200"}, "markers": {"total": 1250, "nextCursor": "c1.2.100"}});
            let host = Rc::new(Host {
                answers: RefCell::new(vec![("status", json!({"connected": true}), false), ("snapshot", snapshot, false)]),
                asked: RefCell::default(),
            });
            let reaper = create_reaper_integration(options(host.clone(), Rc::new(RefCell::new(vec![]))));
            reaper.start(Signal::new()).await.unwrap();
            let context: Value = serde_json::from_str(&reaper.observe(Signal::new(), None).await.unwrap().context).unwrap();
            assert_eq!(context["moreTracks"], "1,000 tracks in all, 2 listed here; reaper_read kind track pages through every one");
            assert_eq!(context["markers"].as_array().map(Vec::len), Some(30));
            assert_eq!(context["moreMarkers"], "1,220 more markers; reaper_read kind marker pages through every one");

            let renoise_snapshot = json!({
                "epoch": "3", "moreTracks": true,
                "project": {"id": null, "identity": "99:1", "name": "Big", "bpm": 140},
                "tracks": [{"ref": "3:track:1", "index": 1, "name": "Kick", "type": "sequencer"}],
                "selection": {"trackRef": "3:track:1"},
            });
            let host = Rc::new(Host {
                answers: RefCell::new(vec![("status", json!({"connected": true}), false), ("snapshot", renoise_snapshot, false)]),
                asked: RefCell::default(),
            });
            let renoise = create_renoise_integration(options(host.clone(), Rc::new(RefCell::new(vec![]))));
            renoise.start(Signal::new()).await.unwrap();
            let context: Value = serde_json::from_str(&renoise.observe(Signal::new(), None).await.unwrap().context).unwrap();
            assert_eq!(context["moreTracks"], "Not every track is listed here; renoise_read kind track pages through every one");
        })
        .await;
}

/// Review K30: a project saved as a copy in another folder carries its original's id (REAPER's travels with the file),
/// but while the original is still there the copy is a project of its own, by core's rules: its own place, the same
/// each time and after a restart, so it never carries on the original's conversation. A copy beside the original is a
/// version of the same song, and a project moved (its original gone) stays the same one.
#[tokio::test(flavor = "current_thread")]
async fn a_copy_saved_elsewhere_is_a_project_of_its_own_while_its_original_is_there() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let dir = tempfile::tempdir().unwrap();
            let file = |at: &str| {
                let path = dir.path().join(at);
                std::fs::create_dir_all(path.parent().unwrap()).unwrap();
                std::fs::write(&path, "<REAPER_PROJECT").unwrap();
                path.to_string_lossy().into_owned()
            };
            let (original, elsewhere, beside) = (file("A/Song A.rpp"), file("B/Song B.rpp"), file("A/Song A v2.rpp"));
            let home = dir.path().join(".kumi");
            let place_of = |path: &String| {
                let (path, home) = (path.clone(), home.clone());
                async move {
                    let mut snapshot = reaper_snapshot();
                    snapshot["project"]["path"] = json!(path);
                    let host = Rc::new(Host {
                        answers: RefCell::new(vec![("status", json!({"connected": true}), false), ("snapshot", snapshot, false)]),
                        asked: RefCell::default(),
                    });
                    let mut options = options(host, Rc::default());
                    options.kumi_home = home;
                    let reaper = create_reaper_integration(options);
                    reaper.start(Signal::new()).await.unwrap();
                    let seen = reaper.observe(Signal::new(), None).await.unwrap();
                    reaper.close().await.unwrap();
                    seen.project.map(|project| project.id).unwrap()
                }
            };
            let ours = format!("reaper-{PROJECT_ID}");
            assert_eq!(place_of(&original).await, ours, "the original, first seen");
            let copy = place_of(&elsewhere).await;
            assert_ne!(copy, ours, "Save As in another folder, the original still there");
            assert_eq!(place_of(&elsewhere).await, copy, "the same place each time, through a restart");
            assert_eq!(place_of(&original).await, ours, "the original stays itself");
            assert_eq!(place_of(&beside).await, ours, "a version beside it is the same song");
            // The song's files deleted (the version, seen last, too): what's left of it is where it went.
            std::fs::remove_file(&original).unwrap();
            std::fs::remove_file(&beside).unwrap();
            assert_eq!(place_of(&elsewhere).await, ours, "moved");
        })
        .await;
}

/// A look that can't read the song (Renoise behind a dialog, or quit) keeps the song's key and project, as Live's do:
/// the conversation stays with it. A saved song's key is its id, so it holds through a restart of Renoise.
#[tokio::test(flavor = "current_thread")]
async fn a_failed_look_keeps_the_songs_key_and_project() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let snapshot = json!({
                "epoch": "3",
                "project": {"id": PROJECT_ID, "identity": "4242:2", "name": "Acid", "path": "/songs/Acid.xrns", "bpm": 140},
                "tracks": [{"ref": "3:track:1", "index": 1, "name": "Kick", "type": "sequencer"}],
                "selection": {"trackRef": "3:track:1"},
            });
            let host = Rc::new(Host {
                answers: RefCell::new(vec![("status", json!({"connected": true}), false), ("snapshot", snapshot, false)]),
                asked: RefCell::default(),
            });
            let states = Rc::new(RefCell::new(vec![]));
            let renoise = create_renoise_integration(options(host.clone(), states.clone()));
            renoise.start(Signal::new()).await.unwrap();
            let seen = renoise.observe(Signal::new(), None).await.unwrap();
            assert_eq!(seen.key, format!("renoise:{PROJECT_ID}"), "a saved song is its id");
            let project = seen.project.clone().expect("a saved song");
            // Behind a dialog: busy, still there.
            host.answers.borrow_mut().push(("snapshot", json!({"error": "a dialog is open", "code": "busy", "connected": true}), true));
            let busy = renoise.observe(Signal::new(), None).await.unwrap();
            assert_eq!((busy.key.as_str(), busy.project.as_ref()), (seen.key.as_str(), Some(&project)), "busy isn't another song");
            assert!(!states.borrow().contains(&ConnectionState::Disconnected), "busy isn't gone: {:?}", states.borrow());
            // Quit: gone, and still the same song's conversation.
            host.answers.borrow_mut().push(("snapshot", json!({"error": "no Renoise is running", "connected": false}), true));
            let gone = renoise.observe(Signal::new(), None).await.unwrap();
            assert_eq!((gone.key.as_str(), gone.project.as_ref()), (seen.key.as_str(), Some(&project)), "away isn't another song");
            assert_eq!(states.borrow().last(), Some(&ConnectionState::Disconnected));
        })
        .await;
}

/// (#361 review K33) A fixed phrase after a look Renoise refused isn't planned from the look before it (a track may have
/// been put above Drums since, and Renoise's refs are places): the model takes it. The project's fingerprint stays.
#[tokio::test(flavor = "current_thread")]
async fn a_fixed_phrase_after_a_failed_look_goes_to_the_model_not_the_last_look() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let snapshot = json!({
                "epoch": "3",
                "project": {"id": PROJECT_ID, "identity": "4242:2", "name": "Acid", "path": "/songs/Acid.xrns", "bpm": 140},
                "tracks": [
                    {"ref": "3:track:1", "index": 1, "name": "Kick", "type": "sequencer"},
                    {"ref": "3:track:2", "index": 2, "name": "Drums", "type": "sequencer"},
                ],
                "selection": {"trackRef": "3:track:1"},
            });
            let host = Rc::new(Host {
                answers: RefCell::new(vec![("status", json!({"connected": true}), false), ("snapshot", snapshot, false)]),
                asked: RefCell::default(),
            });
            let renoise = create_renoise_integration(options(host.clone(), Rc::new(RefCell::new(vec![]))));
            renoise.start(Signal::new()).await.unwrap();
            renoise.observe(Signal::new(), None).await.unwrap();
            host.answers.borrow_mut().push(("snapshot", json!({"error": "a dialog is open", "code": "busy", "connected": true}), true));
            renoise.observe(Signal::new(), None).await.unwrap();
            let asked = host.asked.borrow().len();
            let intent = kumi_runtime::core::phrases::parse("mute drums").expect("a fixed phrase");
            assert!(renoise.instant(&intent, Signal::new()).await.unwrap().is_none(), "the model takes it");
            assert_eq!(host.asked.borrow().len(), asked, "nothing asked of Renoise");
            assert_eq!(renoise.fingerprint().unwrap()["name"], "Acid", "the song's fingerprint stays");
        })
        .await;
}

/// The model's reads stay readable and its refs plain: an error names objects in short refs, REAPER's names hold
/// through an epoch change (its refs are GUIDs), a page too big for the model is asked for again with fewer rows (its
/// nextCursor kept), markers come within the look's budget, and what the producer pinned in FOCUS is in the look.
#[tokio::test(flavor = "current_thread")]
async fn reads_and_looks_stay_plain_short_and_whole() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let mut snapshot = reaper_snapshot();
            snapshot["markers"] = json!((1..=40)
                .map(|n| json!({"ref": format!("r:marker:{{M{n}}}"), "name": format!("M{n}"), "position": n}))
                .collect::<Vec<_>>());
            let host = Rc::new(Host {
                answers: RefCell::new(vec![("status", json!({"connected": true}), false), ("snapshot", snapshot.clone(), false)]),
                asked: RefCell::default(),
            });
            let reaper = create_reaper_integration(options(host.clone(), Rc::new(RefCell::new(vec![]))));
            reaper.start(Signal::new()).await.unwrap();
            let pin = PinnedNode {
                track_ref: BASS.into(),
                r#ref: "r:fx:{F1}".into(),
                node: PinKind::Device,
                name: "ReaComp".into(),
                trail: vec![],
                siblings: vec![],
                track: Some("Bass".into()),
                live: None,
                time: None,
                layout: None,
            };
            let seen = reaper.observe(Signal::new(), Some(ObserveHints { pinned: Some(pin), continuing: None })).await.unwrap();
            let context: Value = serde_json::from_str(&seen.context).unwrap();
            assert_eq!(context["markers"].as_array().unwrap().len(), 30);
            assert_eq!(context["moreMarkers"], "10 more markers; reaper_read kind marker pages through every one");
            assert_eq!(context["pinned"]["name"], "ReaComp");
            assert!(!seen.context.contains("r:fx:") && !seen.context.contains("r:track:"), "{}", seen.context);
            let bass = context["tracks"][1]["ref"].as_str().unwrap().to_owned();

            // An error naming an object: in the turn's short ref.
            host.answers.borrow_mut().push((
                "read",
                json!({"error": format!("{BASS} isn't in the project (deleted, or another tab)"), "code": "failed", "connected": true}),
                true,
            ));
            let failed =
                seen.tools[0].execute(json!({"kind": "item", "parent": bass}).as_object().unwrap().clone(), Signal::new()).await.unwrap();
            assert!(failed.is_error && failed.text.starts_with(&bass) && !failed.text.contains("r:track:"), "{}", failed.text);
            // The read names the epoch its ref came from, so REAPER refuses it in another project (a Save As copy in
            // another tab shares the GUIDs).
            assert_eq!(host.asked.borrow().last().unwrap().1["epoch"], "1");
            host.answers.borrow_mut().push(("read", json!({"error": "stale", "code": "stale", "connected": true}), true));
            let stale =
                seen.tools[0].execute(json!({"kind": "item", "parent": bass}).as_object().unwrap().clone(), Signal::new()).await.unwrap();
            assert_eq!(
                stale.text,
                format!("{bass} was read in another project than the one REAPER has open now: read again for current refs")
            );
            host.answers.borrow_mut().push(("read", json!({"error": "moved", "code": "changed", "connected": true}), true));
            let changed = seen.tools[0]
                .execute(json!({"kind": "item", "parent": bass, "cursor": "c1.1.50~x"}).as_object().unwrap().clone(), Signal::new())
                .await
                .unwrap();
            assert!(changed.is_error && changed.text.starts_with("REAPER changed while reading"), "{}", changed.text);

            // A page past the kernel's cut is asked for again with half the rows, keeping its nextCursor.
            let row = |n: usize| json!({"ref": format!("r:item:{{I{n}}}"), "name": "x".repeat(400)});
            host.answers.borrow_mut().push((
                "read",
                json!({"items": (0..200).map(row).collect::<Vec<_>>(), "nextCursor": "c1.1.200", "epoch": "1"}),
                false,
            ));
            let asked_before = host.asked.borrow().len();
            let _ = seen.tools[0]
                .execute(json!({"kind": "item", "parent": bass, "limit": 200}).as_object().unwrap().clone(), Signal::new())
                .await;
            let limits: Vec<Value> = host.asked.borrow()[asked_before..].iter().map(|(_, args)| args["limit"].clone()).collect();
            assert_eq!(limits[..2], [json!(200), json!(100)], "asked again with half the rows");
            assert!(limits.len() <= 5, "a few times at most: {limits:?}");

            // Another epoch (a tab switched back and forth): REAPER's GUID refs keep their names.
            let mut later = snapshot.clone();
            later["epoch"] = json!("2");
            host.answers.borrow_mut().push(("snapshot", later, false));
            let again: Value = serde_json::from_str(&reaper.observe(Signal::new(), None).await.unwrap().context).unwrap();
            assert_eq!(again["tracks"][1]["ref"], json!(bass), "the same track, the same name");
            // Read again in the new epoch, the ref carries it.
            host.answers.borrow_mut().push(("read", json!({"items": [], "epoch": "2"}), false));
            let tools = reaper.observe(Signal::new(), None).await.unwrap().tools;
            let _ = tools[0].execute(json!({"kind": "item", "parent": bass}).as_object().unwrap().clone(), Signal::new()).await.unwrap();
            assert_eq!(host.asked.borrow().last().unwrap().1["epoch"], "2");
        })
        .await;
}

/// Renoise quit and started again on the same song: it opens on its empty song first and loads the file a moment later.
/// The look waits for the file, so the conversation stays the song's; a song still unsaved after a while is a new one.
#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn a_song_reopened_after_a_restart_keeps_its_conversation_not_the_empty_song_renoise_starts_on() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let song = |identity: &str| {
                json!({"epoch": "1", "layout": 0, "project": {"id": PROJECT_ID, "identity": identity, "name": "Acid", "path": "/songs/Acid.xrns", "bpm": 140}, "tracks": [], "selection": {}})
            };
            let empty = |identity: &str| {
                json!({"epoch": "1", "layout": 0, "project": {"id": "9999999999999999999999999999abcd", "identity": identity, "name": "", "path": null, "bpm": 120}, "tracks": [], "selection": {}})
            };
            let host = Rc::new(Host {
                answers: RefCell::new(vec![("status", json!({"connected": true}), false), ("snapshot", song("t1:1"), false)]),
                asked: RefCell::default(),
            });
            let traced: Rc<RefCell<Vec<String>>> = Rc::default();
            let mut options = options(host.clone(), Rc::new(RefCell::new(vec![])));
            let into = traced.clone();
            options.context.on_dispatch = Some(Rc::new(move |line| into.borrow_mut().push(line)));
            let renoise = create_renoise_integration(options);
            renoise.start(Signal::new()).await.unwrap();
            let before = renoise.observe(Signal::new(), None).await.unwrap().key;
            // KUMI_TRACE shows the key each look took the song to be.
            assert_eq!(traced.borrow().last().cloned(), Some(format!("look: {before}")));
            // Renoise quits.
            host.answers.borrow_mut().push(("snapshot", json!({"error": "Renoise closed", "connected": false}), true));
            renoise.observe(Signal::new(), None).await.unwrap();
            // Started again: its empty song first, the file half a second later.
            host.answers.borrow_mut().push(("snapshot", empty("t2:1"), false));
            let later = host.clone();
            tokio::task::spawn_local(async move {
                tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                later.answers.borrow_mut().push(("snapshot", song("t2:2"), false));
            });
            let after = renoise.observe(Signal::new(), None).await.unwrap();
            assert_eq!(after.key, before, "the same song, the same conversation");
            // A song still unsaved once Renoise has settled is a new one.
            host.answers.borrow_mut().push(("snapshot", json!({"error": "Renoise closed", "connected": false}), true));
            renoise.observe(Signal::new(), None).await.unwrap();
            host.answers.borrow_mut().push(("snapshot", empty("t3:1"), false));
            let started = tokio::time::Instant::now();
            let fresh = renoise.observe(Signal::new(), None).await.unwrap();
            assert_ne!(fresh.key, before);
            assert!(started.elapsed() >= std::time::Duration::from_millis(2_500), "waited for a file that never came");
            // Settled: the next look doesn't wait.
            let started = tokio::time::Instant::now();
            renoise.observe(Signal::new(), None).await.unwrap();
            assert!(started.elapsed() < std::time::Duration::from_millis(100));
        })
        .await;
}

/// A look that runs into Renoise's own change is taken again; one that fails while Renoise is there keeps the project and
/// Renoise's tools (no fresh conversation), and asks the session to look again by itself.
#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn a_look_the_daw_interrupts_is_taken_again_and_a_failed_one_keeps_the_tools_and_looks_again() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let song = json!({"epoch": "1", "layout": 0, "project": {"id": PROJECT_ID, "identity": "t1:1", "name": "Acid", "path": "/songs/Acid.xrns", "bpm": 140}, "tracks": [], "selection": {}});
            let changed = json!({"error": "the song changed while this was being read: read again", "code": "epoch-changed", "connected": true});
            let host = Rc::new(Host {
                answers: RefCell::new(vec![("status", json!({"connected": true}), false), ("snapshot", song.clone(), false)]),
                asked: RefCell::default(),
            });
            let looks_again = Rc::new(std::cell::Cell::new(0));
            let mut options = options(host.clone(), Rc::new(RefCell::new(vec![])));
            let counted = looks_again.clone();
            options.context.on_look_again = Some(Rc::new(move || counted.set(counted.get() + 1)));
            let renoise = create_renoise_integration(options);
            renoise.start(Signal::new()).await.unwrap();
            let first = renoise.observe(Signal::new(), None).await.unwrap();
            // Interrupted once: taken again a moment later, the same look.
            host.answers.borrow_mut().push(("snapshot", changed.clone(), true));
            let later = host.clone();
            let song_again = song.clone();
            tokio::task::spawn_local(async move {
                tokio::time::sleep(std::time::Duration::from_millis(300)).await;
                later.answers.borrow_mut().push(("snapshot", song_again, false));
            });
            let again = renoise.observe(Signal::new(), None).await.unwrap();
            assert_eq!((again.key.as_str(), again.label.as_str()), (first.key.as_str(), first.label.as_str()));
            assert_eq!(looks_again.get(), 0);
            // Interrupted for good: the project and the tools stay, and the session is asked to look again.
            host.answers.borrow_mut().push(("snapshot", changed, true));
            let failed = renoise.observe(Signal::new(), None).await.unwrap();
            assert_eq!(failed.key, first.key, "the same song's conversation");
            assert_eq!(failed.revision, first.revision, "no tools change for the session");
            let names = |tools: &[Rc<dyn KernelTool>]| tools.iter().map(|t| t.name().to_string()).collect::<Vec<_>>();
            assert_eq!(names(&failed.tools), names(&first.tools));
            assert!(failed.label.starts_with("Renoise — couldn't look just now"), "{}", failed.label);
            tokio::time::sleep(std::time::Duration::from_millis(1_100)).await;
            assert_eq!(looks_again.get(), 1, "looked again without a turn");
        })
        .await;
}

/// An answer read in another track layout than the book's: newer (a tracks event lost on the way) retires the book's
/// names so none reads the wrong track; older (a shift landed first) is read again, carried to now.
#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn an_answer_in_another_track_layout_never_leaves_a_name_on_the_wrong_track() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let snapshot = json!({"epoch": "1", "layout": 0, "project": {"id": PROJECT_ID, "identity": "t1:1", "name": "Acid", "path": "/songs/Acid.xrns"}, "tracks": [{"ref": "1:track:1", "name": "Kick"}, {"ref": "1:track:2", "name": "Bass"}], "selection": {}});
            let host = Rc::new(Host {
                answers: RefCell::new(vec![("status", json!({"connected": true}), false), ("snapshot", snapshot, false)]),
                asked: RefCell::default(),
            });
            let renoise = create_renoise_integration(options(host.clone(), Rc::new(RefCell::new(vec![]))));
            renoise.start(Signal::new()).await.unwrap();
            let seen = renoise.observe(Signal::new(), None).await.unwrap();
            let read = seen.tools[0].clone();
            let context: Value = serde_json::from_str(&seen.context).unwrap();
            assert_eq!(context["tracks"][0]["ref"], "track:1");
            // Two tracks were inserted at the top and Kumi never heard: the answer is two shifts on.
            host.answers.borrow_mut().push((
                "read",
                json!({"epoch": "1", "layout": 2, "items": [{"ref": "1:track:3", "name": "Kick"}]}),
                false,
            ));
            let page = read.execute(json!({"kind": "track"}).as_object().unwrap().clone(), Signal::new()).await.unwrap();
            let page: Value = serde_json::from_str(&page.text).unwrap();
            assert_ne!(page["items"][0]["ref"], "track:1", "Kick's new name isn't the old track:1");
            let old = read.execute(json!({"kind": "device", "parent": "track:1"}).as_object().unwrap().clone(), Signal::new()).await.unwrap();
            assert!(old.is_error && old.text.contains("isn't one of this turn's refs"), "{}", old.text);
            // An answer from before a shift the book follows (layout 1 against 2): read again, a few times at most.
            host.answers.borrow_mut().push(("read", json!({"epoch": "1", "layout": 1, "items": []}), false));
            let before = host.asked.borrow().iter().filter(|(tool, _)| tool == "read").count();
            let stale = read.execute(json!({"kind": "track"}).as_object().unwrap().clone(), Signal::new()).await.unwrap();
            let reads = host.asked.borrow().iter().filter(|(tool, _)| tool == "read").count() - before;
            assert_eq!(reads, 5, "read again, carried to now");
            // Still older after that: refused, never shortened into the book's newer names.
            assert!(stale.is_error && stale.text.contains("changed while reading"), "{}", stale.text);
        })
        .await;
}

/// Renoise moves its epoch for a device, pattern or instrument edit, and no event says so: the next answer from that
/// epoch retires the track names, and a request that names no track never carries a layout to be refused for.
#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn an_edit_no_event_tells_of_never_leaves_renoise_unreadable() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let snapshot = json!({"epoch": "1", "layout": 0, "project": {"id": PROJECT_ID, "identity": "t1:1", "name": "Acid", "path": "/songs/Acid.xrns"}, "tracks": [{"ref": "1:track:1", "name": "Kick"}], "selection": {}});
            let host = Rc::new(Host {
                answers: RefCell::new(vec![("status", json!({"connected": true}), false), ("snapshot", snapshot, false)]),
                asked: RefCell::default(),
            });
            let renoise = create_renoise_integration(options(host.clone(), Rc::new(RefCell::new(vec![]))));
            renoise.start(Signal::new()).await.unwrap();
            let read = renoise.observe(Signal::new(), None).await.unwrap().tools[0].clone();
            let last_read = || host.asked.borrow().iter().rev().find(|(tool, _)| tool == "read").unwrap().1.clone();
            // A track was inserted (layout 1), as an answer shows.
            host.answers.borrow_mut().push(("read", json!({"epoch": "1", "layout": 1, "items": [{"ref": "1:track:2", "name": "Kick"}]}), false));
            let page = read.execute(json!({"kind": "track"}).as_object().unwrap().clone(), Signal::new()).await.unwrap();
            assert!(last_read().get("layout").is_none(), "a read naming no track names no layout: {}", last_read());
            let kick = serde_json::from_str::<Value>(&page.text).unwrap()["items"][0]["ref"].as_str().unwrap().to_owned();
            host.answers.borrow_mut().push(("read", json!({"epoch": "1", "layout": 1, "items": []}), false));
            read.execute(json!({"kind": "device", "parent": kick}).as_object().unwrap().clone(), Signal::new()).await.unwrap();
            assert_eq!(last_read()["layout"], 1, "a read naming a track names the layout its ref is at");
            // Then a device was added: Renoise's epoch moved, with no event. The next answer shows it.
            host.answers.borrow_mut().push(("read", json!({"epoch": "2", "layout": 0, "items": [{"ref": "2:pattern:1"}]}), false));
            let song = read.execute(json!({"kind": "pattern"}).as_object().unwrap().clone(), Signal::new()).await.unwrap();
            assert!(!song.is_error, "{}", song.text);
            // The track names from before the edit are retired, so none reads the wrong object.
            let old = read.execute(json!({"kind": "device", "parent": kick}).as_object().unwrap().clone(), Signal::new()).await.unwrap();
            assert!(old.is_error && old.text.contains("isn't one of this turn's refs"), "{}", old.text);
        })
        .await;
}

/// A look that fails because the DAW is busy behind a dialog doesn't keep asking, every second, for a look that would
/// wait again: the next is asked for once the focus says the dialog is gone.
#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn a_busy_daw_is_looked_at_again_once_it_is_free_not_every_second() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let busy = json!({"error": "REAPER is busy: a dialog is open", "code": "busy", "connected": true});
            let host = Rc::new(Host {
                answers: RefCell::new(vec![
                    ("status", json!({"connected": true, "busy": "modal"}), false),
                    ("snapshot", reaper_snapshot(), false),
                    (
                        "focus",
                        json!({"epoch": "1", "view": "arrange", "selection": {}, "transport": {"playing": false}, "busy": "modal"}),
                        false,
                    ),
                ]),
                asked: RefCell::default(),
            });
            let looks_again = Rc::new(std::cell::Cell::new(0));
            let mut options = options(host.clone(), Rc::new(RefCell::new(vec![])));
            let counted = looks_again.clone();
            options.context.on_look_again = Some(Rc::new(move || counted.set(counted.get() + 1)));
            options.context.on_focus = Some(Rc::new(|_| {}));
            let reaper = create_reaper_integration(options);
            reaper.start(Signal::new()).await.unwrap();
            let seen = reaper.observe(Signal::new(), None).await.unwrap();
            // A dialog opens: the look waits a while, then keeps the project and REAPER's tools.
            host.answers.borrow_mut().push(("snapshot", busy, true));
            let failed = reaper.observe(Signal::new(), None).await.unwrap();
            assert_eq!(failed.tools.len(), seen.tools.len());
            tokio::time::sleep(std::time::Duration::from_secs(5)).await;
            assert_eq!(looks_again.get(), 0, "no look asked for while the dialog stays");
            // The dialog closes: one look is asked for.
            host.answers.borrow_mut().push(("status", json!({"connected": true}), false));
            host.answers.borrow_mut().push((
                "focus",
                json!({"epoch": "1", "view": "arrange", "selection": {}, "transport": {"playing": false}}),
                false,
            ));
            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
            assert_eq!(looks_again.get(), 1, "looked at again once it was free");
        })
        .await;
}

/// A host whose calls can fail (the caller stopped, a time limit) and that can go away (its pipe closed).
struct Leaving {
    inner: Rc<Host>,
    fail: std::cell::Cell<bool>,
    gone: RefCell<Vec<Rc<dyn Fn()>>>,
}
#[async_trait(?Send)]
impl McpEndpoint for Leaving {
    fn pid(&self) -> Option<u32> {
        None
    }
    fn server_info(&self) -> Option<Implementation> {
        None
    }
    async fn list(&self, cursor: Option<&str>, signal: Signal) -> Result<ListToolsResult, RuntimeError> {
        self.inner.list(cursor, signal).await
    }
    async fn call(&self, name: &str, args: JsonObject, signal: Signal) -> Result<CallToolResult, RuntimeError> {
        if self.fail.replace(false) {
            return Err(RuntimeError::plain("MCP request cancelled"));
        }
        self.inner.call(name, args, signal).await
    }
    fn on_catalog_changed(&self, _: Rc<dyn Fn()>) -> Box<dyn Fn()> {
        Box::new(|| {})
    }
    fn on_disconnect(&self, listener: Rc<dyn Fn()>) -> Box<dyn Fn()> {
        self.gone.borrow_mut().push(listener);
        Box::new(|| {})
    }
    fn stderr_status(&self) -> StderrStatus {
        StderrStatus { bytes: 0, truncated: false }
    }
    async fn close(&self) -> Result<(), RuntimeError> {
        Ok(())
    }
}

#[tokio::test(flavor = "current_thread")]
async fn a_failed_call_keeps_kumis_host_and_only_its_going_away_starts_another() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let inner = Rc::new(Host {
                answers: RefCell::new(vec![("status", json!({"connected": true}), false), ("snapshot", reaper_snapshot(), false)]),
                asked: RefCell::default(),
            });
            let host = Rc::new(Leaving { inner, fail: std::cell::Cell::new(false), gone: RefCell::default() });
            let started = Rc::new(std::cell::Cell::new(0));
            let (hosts, endpoint) = (started.clone(), host.clone());
            let connect: Connect = Rc::new(move |_| {
                hosts.set(hosts.get() + 1);
                let endpoint = endpoint.clone();
                async move { Ok(endpoint as Rc<dyn McpEndpoint>) }.boxed_local()
            });
            let options = OwnOptions {
                context: IntegrationContext::new(Rc::new(|_, _| {})),
                kumi_home: std::env::temp_dir(),
                connect: Some(connect),
                open_file: None,
            };
            let reaper = create_reaper_integration(options);
            reaper.start(Signal::new()).await.unwrap();
            reaper.observe(Signal::new(), None).await.unwrap();
            assert_eq!(started.get(), 1);
            // A call that fails (Esc, a time limit) leaves the host as it is: its journal holds Kumi's changes.
            host.fail.set(true);
            let _ = reaper.observe(Signal::new(), None).await;
            reaper.observe(Signal::new(), None).await.unwrap();
            assert_eq!(started.get(), 1, "the same host after a failed call");
            // The host goes away: the next call starts another.
            let listeners = host.gone.borrow().clone();
            for gone in listeners {
                gone();
            }
            reaper.observe(Signal::new(), None).await.unwrap();
            assert_eq!(started.get(), 2, "another host once the first is gone");
        })
        .await;
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
            // Logic's tools stay (its read first, its changes with it): lacking a permission takes none away.
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
            let mut options = options(host.clone(), Rc::new(RefCell::new(vec![])));
            options.context.on_focus = Some(Rc::new(move |focus| {
                if let Some(busy) = focus.and_then(|focus| focus.busy) {
                    into.borrow_mut().push(busy);
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
                for _ in 0..200 {
                    if seen.borrow().iter().any(|said| said == words) {
                        break;
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                }
                assert!(seen.borrow().iter().any(|said| said == words), "{code}: {:?}", seen.borrow());
            }
            logic.close().await.unwrap();
        })
        .await;
}

/// A host whose events a test sends: Kumi's host passing on the bridge's.
struct Eventful {
    host: Rc<Host>,
    listener: RefCell<Option<Rc<dyn Fn(JsonObject)>>>,
}
#[async_trait(?Send)]
impl McpEndpoint for Eventful {
    fn pid(&self) -> Option<u32> {
        None
    }
    fn server_info(&self) -> Option<Implementation> {
        None
    }
    async fn list(&self, server: Option<&str>, signal: Signal) -> Result<ListToolsResult, RuntimeError> {
        self.host.list(server, signal).await
    }
    async fn call(&self, name: &str, args: JsonObject, signal: Signal) -> Result<CallToolResult, RuntimeError> {
        self.host.call(name, args, signal).await
    }
    fn on_catalog_changed(&self, _: Rc<dyn Fn()>) -> Box<dyn Fn()> {
        Box::new(|| {})
    }
    fn on_disconnect(&self, _: Rc<dyn Fn()>) -> Box<dyn Fn()> {
        Box::new(|| {})
    }
    fn on_live_event(&self, listener: Rc<dyn Fn(JsonObject)>) -> Box<dyn Fn()> {
        *self.listener.borrow_mut() = Some(listener);
        Box::new(|| {})
    }
    fn stderr_status(&self) -> StderrStatus {
        StderrStatus { bytes: 0, truncated: false }
    }
    async fn close(&self) -> Result<(), RuntimeError> {
        Ok(())
    }
}

// Review K3: a Renoise device pinned in FOCUS keeps naming it when a track is added above its track, and once Renoise
// renumbers past what Kumi can follow, the pin is told by its name only.
#[tokio::test(flavor = "current_thread")]
async fn a_pinned_renoise_device_keeps_naming_it_when_tracks_move_above_it() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let snapshot = |layout: u64, names: &[&str]| {
                let tracks: Vec<Value> =
                    names.iter().enumerate().map(|(i, name)| json!({"ref": format!("1:track:{}", i + 1), "name": name})).collect();
                json!({"epoch": "1", "layout": layout, "project": {"id": PROJECT_ID, "identity": "t1:1", "name": "Acid", "path": "/songs/Acid.xrns"}, "tracks": tracks, "selection": {}})
            };
            let host = Rc::new(Host {
                answers: RefCell::new(vec![("status", json!({"connected": true}), false), ("snapshot", snapshot(0, &["Kick", "Bass"]), false)]),
                asked: RefCell::default(),
            });
            let eventful = Rc::new(Eventful { host: host.clone(), listener: RefCell::default() });
            let connect: Connect = {
                let eventful = eventful.clone();
                Rc::new(move |_| {
                    let eventful = eventful.clone();
                    async move { Ok(eventful as Rc<dyn McpEndpoint>) }.boxed_local()
                })
            };
            let renoise = create_renoise_integration(OwnOptions {
                context: IntegrationContext::new(Rc::new(|_, _| {})),
                kumi_home: std::env::temp_dir(),
                connect: Some(connect), open_file: None });
            renoise.start(Signal::new()).await.unwrap();
            let pin = PinnedNode {
                track_ref: "1:track:2".into(),
                r#ref: "1:device:2.2".into(),
                node: PinKind::Device,
                name: "Compressor".into(),
                trail: vec![],
                siblings: vec![],
                track: Some("Bass".into()),
                live: None,
                time: None,
                // (FOCUS read its tree at layout 0)
                layout: Some(0),
            };
            let look = |pin: &PinnedNode| {
                let hints = ObserveHints { pinned: Some(pin.clone()), continuing: None };
                renoise.observe(Signal::new(), Some(hints))
            };
            let seen = look(&pin).await.unwrap();
            let first: Value = serde_json::from_str::<Value>(&seen.context).unwrap()["pinned"].clone();
            assert_eq!(first["trackRef"], "track:2", "{first}");
            // A track added at the top: Bass is track 3 now, and its Compressor 3.2.
            let send = eventful.listener.borrow().clone().expect("Kumi listens to the host's events");
            send(json!({"name": "tracks", "data": {"epoch": "1", "layout": 1, "made": [1]}}).as_object().unwrap().clone());
            host.answers.borrow_mut().push(("snapshot", snapshot(1, &["New", "Kick", "Bass"]), false));
            let seen = look(&pin).await.unwrap();
            let pinned: Value = serde_json::from_str::<Value>(&seen.context).unwrap()["pinned"].clone();
            assert_eq!(pinned, first, "the same names, which name Bass and its Compressor where they are now");
            host.answers.borrow_mut().push(("read", json!({"epoch": "1", "layout": 1, "items": []}), false));
            seen.tools[0]
                .execute(json!({"kind": "param", "parent": pinned["ref"]}).as_object().unwrap().clone(), Signal::new())
                .await
                .unwrap();
            assert_eq!(host.asked.borrow().last().unwrap().1["parent"], "1:device:3.2", "the Compressor on Bass, now track 3");
            // Shifts Kumi didn't hear of (a gap in the layouts): the names retire, and the pin is told by its name only.
            send(json!({"name": "tracks", "data": {"epoch": "1", "layout": 4, "made": [1]}}).as_object().unwrap().clone());
            host.answers.borrow_mut().push(("snapshot", snapshot(4, &["A", "B", "C", "New", "Kick", "Bass"]), false));
            let seen = look(&pin).await.unwrap();
            let pinned: Value = serde_json::from_str::<Value>(&seen.context).unwrap()["pinned"].clone();
            assert!(pinned["gone"].as_str().unwrap().starts_with("The producer pinned “Compressor” on “Bass” in FOCUS, and Renoise has renumbered"), "{pinned}");
            assert!(pinned.get("ref").is_none());
        })
        .await;
}

/// A Renoise song's snapshot at a layout, with these tracks.
fn song_at(layout: u64, names: &[&str]) -> Value {
    let tracks: Vec<Value> = names.iter().enumerate().map(|(i, name)| json!({"ref": format!("1:track:{}", i + 1), "name": name})).collect();
    json!({"epoch": "1", "layout": layout, "project": {"id": PROJECT_ID, "identity": "t1:1", "name": "Acid", "path": "/songs/Acid.xrns"}, "tracks": tracks, "selection": {}})
}

/// A Renoise song's host for the pin tests: Kick and Bass at layout 0, looked at once, whose events a test sends.
async fn pinned_song() -> (Rc<Host>, Rc<Eventful>, Rc<dyn Integration>) {
    let host = Rc::new(Host {
        answers: RefCell::new(vec![
            ("status", json!({"connected": true}), false),
            ("snapshot", song_at(0, &["Kick", "Bass"]), false),
            ("focus_body", json!({"epoch": "1", "layout": 0, "devices": []}), false),
            ("read", json!({"epoch": "1", "layout": 1, "items": []}), false),
        ]),
        asked: RefCell::default(),
    });
    let eventful = Rc::new(Eventful { host: host.clone(), listener: RefCell::default() });
    let connect: Connect = {
        let eventful = eventful.clone();
        Rc::new(move |_| {
            let eventful = eventful.clone();
            async move { Ok(eventful as Rc<dyn McpEndpoint>) }.boxed_local()
        })
    };
    let renoise = create_renoise_integration(OwnOptions {
        context: IntegrationContext::new(Rc::new(|_, _| {})),
        kumi_home: std::env::temp_dir(),
        connect: Some(connect),
        open_file: None,
    });
    renoise.start(Signal::new()).await.unwrap();
    renoise.observe(Signal::new(), None).await.unwrap();
    // The producer pins from FOCUS's tree of Bass: it says the layout its refs were read at.
    let Some(FocusBody::Tree(tree)) =
        renoise.focus_body(&BodyRequest::Tree { track_ref: "1:track:2".into() }, Signal::new()).await.unwrap()
    else {
        panic!("a tree")
    };
    assert_eq!(tree.layout, Some(0));
    (host, eventful, renoise)
}

/// The pin FOCUS makes from Bass's tree as read at layout 0: its Compressor, stamped with that layout.
fn compressor_on_bass() -> PinnedNode {
    PinnedNode {
        track_ref: "1:track:2".into(),
        r#ref: "1:device:2.2".into(),
        node: PinKind::Device,
        name: "Compressor".into(),
        trail: vec![],
        siblings: vec![],
        track: Some("Bass".into()),
        live: None,
        time: None,
        layout: Some(0),
    }
}

/// A track added at the top, as the host tells it: Bass is track 3 now, and its Compressor 3.2.
fn track_added_at_the_top(host: &Host, eventful: &Eventful) {
    let send = eventful.listener.borrow().clone().expect("Kumi listens to the host's events");
    send(json!({"name": "tracks", "data": {"epoch": "1", "layout": 1, "made": [1]}}).as_object().unwrap().clone());
    host.answers.borrow_mut().push(("snapshot", song_at(1, &["New", "Kick", "Bass"]), false));
}

// #361 final review (K3): a pin made from a FOCUS tree read before a track was added at the top, first seen by Kumi after
// it, names what was pinned where it is now, carried from the layout the tree was read at (not taken to be at the layout
// Kumi is at when it first sees it).
#[tokio::test(flavor = "current_thread")]
async fn a_pin_first_seen_after_a_shift_names_what_was_pinned() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let (host, eventful, renoise) = pinned_song().await;
            track_added_at_the_top(&host, &eventful);
            let hints = ObserveHints { pinned: Some(compressor_on_bass()), continuing: None };
            let seen = renoise.observe(Signal::new(), Some(hints)).await.unwrap();
            let pinned: Value = serde_json::from_str::<Value>(&seen.context).unwrap()["pinned"].clone();
            assert!(pinned.get("gone").is_none(), "{pinned}");
            seen.tools[0]
                .execute(json!({"kind": "param", "parent": pinned["ref"]}).as_object().unwrap().clone(), Signal::new())
                .await
                .unwrap();
            assert_eq!(host.asked.borrow().last().unwrap().1["parent"], "1:device:3.2", "the Compressor on Bass, not Kick's device");
        })
        .await;
}

// #361 final review (K3): /recipe fills its blanks from the pin as the integration carries it, in the turn's short refs,
// never with the raw refs FOCUS pinned with: after a track is added at the top, the recipe edits the pinned device.
#[tokio::test(flavor = "current_thread")]
async fn a_recipes_blanks_take_the_pin_where_it_is_now() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let (host, eventful, renoise) = pinned_song().await;
            track_added_at_the_top(&host, &eventful);
            let seen = renoise.observe(Signal::new(), None).await.unwrap();
            let blank = renoise.pin_for_recipe(&compressor_on_bass()).expect("the pin, carried");
            assert_ne!(blank.r#ref, "1:device:2.2", "never the raw ref");
            seen.tools[0]
                .execute(json!({"kind": "param", "parent": blank.r#ref}).as_object().unwrap().clone(), Signal::new())
                .await
                .unwrap();
            assert_eq!(host.asked.borrow().last().unwrap().1["parent"], "1:device:3.2", "the pinned Compressor where it is now");
            // Renumbered past what Kumi can follow: no blank is filled from it.
            let send = eventful.listener.borrow().clone().unwrap();
            send(json!({"name": "tracks", "data": {"epoch": "1", "layout": 4, "made": [1]}}).as_object().unwrap().clone());
            assert!(renoise.pin_for_recipe(&compressor_on_bass()).is_none());
        })
        .await;
}

// #361 final review (K3, reviewer999): FOCUS reading `1:track:2`'s tree again after the shift (now Kick's) changes nothing
// for a pin made from the tree read before it: the pin says where its own refs were read.
#[tokio::test(flavor = "current_thread")]
async fn a_pin_keeps_the_layout_of_its_own_tree_when_the_same_ref_is_read_again() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let (host, eventful, renoise) = pinned_song().await;
            track_added_at_the_top(&host, &eventful);
            host.answers.borrow_mut().push(("focus_body", json!({"epoch": "1", "layout": 1, "devices": []}), false));
            renoise.focus_body(&BodyRequest::Tree { track_ref: "1:track:2".into() }, Signal::new()).await.unwrap();
            let hints = ObserveHints { pinned: Some(compressor_on_bass()), continuing: None };
            let seen = renoise.observe(Signal::new(), Some(hints)).await.unwrap();
            let pinned: Value = serde_json::from_str::<Value>(&seen.context).unwrap()["pinned"].clone();
            seen.tools[0]
                .execute(json!({"kind": "param", "parent": pinned["ref"]}).as_object().unwrap().clone(), Signal::new())
                .await
                .unwrap();
            assert_eq!(host.asked.borrow().last().unwrap().1["parent"], "1:device:3.2", "Bass's Compressor, not Kick's device");
            assert!(pinned.get("layout").is_none(), "the model isn't told where its refs were read: {pinned}");
        })
        .await;
}
