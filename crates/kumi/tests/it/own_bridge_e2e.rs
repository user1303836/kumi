//! REAPER's and Renoise's integrations end to end: Kumi's host (daw-mcp-server's own code, in process) in front of a
//! bridge that answers like REAPER's extension, or replays what Renoise's tool answered. What the host answers is what
//! the integration reads: the observation, the model's read, the focus and FOCUS's bodies.
use daw_bridge::{files::ensure_secret, paths::BridgePaths, Daw};
use daw_mcp_server::{host::Host, reaper::simulator::ReaperScript, renoise::simulator::RenoiseScript, simulator::Simulator};
use futures::FutureExt;
use kumi_common::abort::Signal;
use kumi_daw_reaper::create_reaper_integration;
use kumi_daw_renoise::create_renoise_integration;
use kumi_runtime::core::{
    context::IntegrationContext,
    contracts::*,
    errors::RuntimeError,
    view::{BodyKind, BodyRequest, DawFocus, FocusBody},
};
use kumi_runtime::integrations::own_bridge::{Connect, OwnOptions};
use kumi_runtime::mcp::client::McpEndpoint;
use serde_json::{json, Value};
use std::{cell::Cell, cell::RefCell, rc::Rc, sync::Arc};

kumi_testkit::in_process!();
use in_process::InProcess;

/// Every key of `input` is one the tool's schema offers (it takes no others), and what it requires is there.
fn within_schema(schema: &JsonObject, input: &JsonObject) {
    let properties = schema["properties"].as_object().unwrap();
    for key in input.keys() {
        assert!(properties.contains_key(key), "{key} isn't in the schema: {schema:?}");
    }
    for key in schema.get("required").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str) {
        assert!(input.contains_key(key), "{key} is required");
    }
}

/// The last read the tool was asked (the focus is polled meanwhile, so it needn't be the last request).
fn last_read(script: &RenoiseScript) -> Value {
    script.asked().into_iter().rev().find(|(method, _)| method == "read").map(|(_, params)| params).unwrap_or(Value::Null)
}

#[tokio::test(flavor = "current_thread")]
async fn reapers_integration_reads_what_kumis_host_answers() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let home = tempfile::tempdir().unwrap();
            let paths = BridgePaths::new(home.path(), Daw::Reaper);
            let secret = ensure_secret(&paths.secret()).unwrap();
            let script = ReaperScript::new(3, 0);
            let mut config = script.config(secret, "/lab/REAPER");
            config.discovery_dir = Some(paths.discovery());
            let simulator = Simulator::start(config).await.unwrap();
            let endpoint = Rc::new(InProcess { host: Arc::new(Host::new(Daw::Reaper, home.path())), id: Cell::new(0) });
            let connect: Connect = Rc::new(move |_| {
                let endpoint = endpoint.clone();
                async move { Ok(endpoint as Rc<dyn McpEndpoint>) }.boxed_local()
            });
            let seen: Rc<RefCell<Vec<Option<DawFocus>>>> = Rc::default();
            let into = seen.clone();
            let mut context = IntegrationContext::new(Rc::new(|_, _| {}));
            context.on_focus = Some(Rc::new(move |focus| into.borrow_mut().push(focus)));
            context.focus_interval_ms = Some(50);
            let reaper = create_reaper_integration(OwnOptions { context, kumi_home: home.path().into(), connect: Some(connect) });

            reaper.start(Signal::new()).await.unwrap();
            let observed = reaper.observe(Signal::new(), None).await.unwrap();
            assert!(observed.label.starts_with("REAPER — "), "{}", observed.label);
            let context: Value = serde_json::from_str(&observed.context).unwrap();
            assert_eq!(context["daw"], "REAPER");
            let tracks = context["tracks"].as_array().unwrap();
            assert_eq!(tracks.len(), 4, "the master and three tracks: {context}");
            assert!(tracks.iter().all(|track| track["ref"].as_str().is_some_and(|r| r.starts_with("track:"))), "{context}");
            assert_eq!(context["selection"]["trackRef"], tracks[1]["ref"]);
            assert!(!observed.context.contains("r:track:"), "no long ref reaches the model: {}", observed.context);

            let read = observed.tools[0].clone();
            let page = read
                .execute(json!({"kind": "item", "parent": tracks[1]["ref"]}).as_object().unwrap().clone(), Signal::new())
                .await
                .unwrap();
            assert!(!page.is_error, "{}", page.text);
            assert!(!page.text.contains("r:item:") && page.text.contains("item:"), "{}", page.text);
            // What folders, send modes and times read as.
            for words in ["folder 1 starts a folder and -1 or less closes", "0 post-fader, 1 pre-FX, 3 pre-fader", "quarter notes"] {
                assert!(read.description().contains(words), "{words}: {}", read.description());
            }

            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
            let focus = seen.borrow().iter().flatten().next().cloned().expect("the focus, as the host reads it");
            let track = focus.track_ref.clone().expect("the selected track");
            for (kind, request) in [
                (BodyKind::Timeline, BodyRequest::Timeline),
                (BodyKind::Routing, BodyRequest::Routing { track_ref: track.clone() }),
                (BodyKind::Tree, BodyRequest::Tree { track_ref: track.clone() }),
            ] {
                assert!(reaper.can_show(kind));
                let body = reaper.focus_body(&request, Signal::new()).await.unwrap();
                assert!(
                    matches!(
                        (&request, &body),
                        (BodyRequest::Timeline, Some(FocusBody::Timeline(_)))
                            | (BodyRequest::Routing { .. }, Some(FocusBody::Routing(_)))
                            | (BodyRequest::Tree { .. }, Some(FocusBody::Tree(_)))
                    ),
                    "{request:?}: {body:?}"
                );
            }
            let arrange =
                script.asked().into_iter().rev().find(|(method, params)| method == "focus_body" && params["kind"] == "arrange").unwrap();
            assert_eq!(arrange.1["around"].as_str(), Some(track.as_str()), "the page is centred on the selected track");
            // (m4-tui) REAPER has no status bar Kumi can write to: nothing is asked.
            assert!(!reaper.has_show_message());
            assert!(!reaper.show_message("Kumi · done", Signal::new()).await.unwrap());
            assert!(!script.asked().iter().any(|(method, _)| method == "show_status"));
            reaper.close().await.unwrap();
            simulator.stop();
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn renoises_integration_reads_what_kumis_host_answers_from_the_tool() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let home = tempfile::tempdir().unwrap();
            let paths = BridgePaths::new(home.path(), Daw::Renoise);
            let secret = ensure_secret(&paths.secret()).unwrap();
            let script = RenoiseScript::new();
            let mut config = script.config(secret, "/lab/Renoise");
            config.discovery_dir = Some(paths.discovery());
            let simulator = Simulator::start(config).await.unwrap();
            let endpoint = Rc::new(InProcess { host: Arc::new(Host::new(Daw::Renoise, home.path())), id: Cell::new(0) });
            let connect: Connect = Rc::new(move |_| {
                let endpoint = endpoint.clone();
                async move { Ok(endpoint as Rc<dyn McpEndpoint>) }.boxed_local()
            });
            let seen: Rc<RefCell<Vec<Option<DawFocus>>>> = Rc::default();
            let into = seen.clone();
            let mut context = IntegrationContext::new(Rc::new(|_, _| {}));
            context.on_focus = Some(Rc::new(move |focus| into.borrow_mut().push(focus)));
            context.focus_interval_ms = Some(50);
            let renoise = create_renoise_integration(OwnOptions { context, kumi_home: home.path().into(), connect: Some(connect) });

            renoise.start(Signal::new()).await.unwrap();
            let observed = renoise.observe(Signal::new(), None).await.unwrap();
            assert_eq!(observed.label, "Renoise — Demo");
            let context: Value = serde_json::from_str(&observed.context).unwrap();
            assert_eq!((context["daw"].as_str(), context["song"]["bpm"].as_i64()), (Some("Renoise"), Some(128)), "{context}");
            let tracks = context["tracks"].as_array().unwrap();
            assert_eq!(tracks.len(), 7, "{context}");
            assert_eq!(
                (tracks[0]["ref"].as_str(), tracks[0]["groupRef"].as_str()),
                (Some("track:1"), Some("track:3")),
                "a group in short refs too"
            );
            assert_eq!(context["selection"]["patternRef"], "pattern:1");
            assert!(
                !observed.context.contains("1:track:") && !observed.context.contains("1:pattern:"),
                "no long ref reaches the model: {}",
                observed.context
            );
            assert!(observed.project.as_ref().is_some_and(|project| project.id.starts_with("renoise-")), "a saved song is its place");

            // The model reads a track's devices and a pattern's lines (which need the track) in short refs.
            let read = observed.tools[0].clone();
            let devices =
                read.execute(json!({"kind": "device", "parent": "track:1"}).as_object().unwrap().clone(), Signal::new()).await.unwrap();
            assert!(!devices.is_error && devices.text.contains("EQ 5") && devices.text.contains("device:"), "{}", devices.text);
            assert!(!devices.text.contains("1:device:"), "{}", devices.text);
            // A pattern's lines, asked as the model can ask through the tool's schema: the track by its ref, lengthened.
            let input = json!({"kind": "line", "parent": "pattern:1", "trackRef": "track:1"}).as_object().unwrap().clone();
            within_schema(&read.input_schema(), &input);
            let _ = read.execute(input, Signal::new()).await.unwrap();
            assert_eq!(last_read(&script), json!({"kind": "line", "parent": "1:pattern:1", "track": "1:track:1", "layout": 0}));
            // The columns the track hides, asked for as the schema allows.
            let input = json!({"kind": "line", "parent": "pattern:1", "trackRef": "track:1", "hidden": true}).as_object().unwrap().clone();
            within_schema(&read.input_schema(), &input);
            let _ = read.execute(input, Signal::new()).await.unwrap();
            assert_eq!(last_read(&script)["hidden"], true);
            assert!(
                read.description().contains("line under a pattern") && read.description().contains("trackRef"),
                "{}",
                read.description()
            );
            // What the lines' hex columns mean, so the model reads them without guessing.
            for words in ["volume (hex 00-80, 80 full", "panning (hex 00-80, 40 centre)", "delay (hex 00-FF"] {
                assert!(read.description().contains(words), "{words}: {}", read.description());
            }

            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
            let focus = seen.borrow().iter().flatten().next().cloned().expect("the focus, as the host reads it");
            assert_eq!((focus.track_ref.as_deref(), focus.device.as_deref()), (Some("1:track:1"), Some("Doofer")));
            assert_eq!(focus.item.as_ref().and_then(|item| item.item_ref.as_deref()), Some("1:pattern:1"));
            assert_eq!((focus.editor_view.as_ref(), focus.panes.as_ref(), focus.section_follows), (None, None, None), "neither pane open");

            // The pattern FOCUS asks for is the focus's own, by its ref.
            let pattern = BodyRequest::Tracker { item_ref: focus.item.as_ref().and_then(|item| item.item_ref.clone()) };
            let Some(FocusBody::Tracker(grid)) = renoise.focus_body(&pattern, Signal::new()).await.unwrap() else { panic!("the pattern") };
            let asked = script.asked();
            assert_eq!(
                asked.iter().rev().find(|(method, _)| method == "focus_body").unwrap().1,
                // (No layout: a pattern isn't on a track, so no track shift moves it.)
                json!({"kind": "pattern", "ref": "1:pattern:1"})
            );
            assert_eq!((grid.lines, grid.from, grid.cursor), (32, 0, Some(0)));
            let kick = &grid.tracks[0];
            assert_eq!(kick.cells.len(), 32, "every line of the window, though the tool sends only those with something in them");
            assert_eq!(kick.cells[0], "C-4 00 .. .. .. .... ....");
            assert_eq!(kick.cells[1], "--- .. .. .. .. .... ....");
            assert_eq!(kick.cells[4], "D#4 00 40 .. .. 0A20 ....", "line 5 where Renoise shows it");
            assert_eq!(grid.tracks[3].cells[2], "C-3 01 .. .. .. OFF .. .. .. .. ....", "both of Bass's note columns");
            assert_eq!(grid.tracks[4].cells[1], "--- .. .. .. .. 0A40 ....", "Mix's two effect columns");

            let Some(FocusBody::Matrix(matrix)) = renoise.focus_body(&BodyRequest::Matrix, Signal::new()).await.unwrap() else {
                panic!("the matrix")
            };
            assert_eq!((matrix.slots.len(), matrix.current, matrix.slots[2].muted.as_slice()), (4, Some(0), &[1][..]));
            let Some(FocusBody::Keyzones(map)) =
                renoise.focus_body(&BodyRequest::Keyzones { item_ref: "1:instrument:1".into() }, Signal::new()).await.unwrap()
            else {
                panic!("the keyzones")
            };
            assert_eq!((map.name.as_str(), map.zones.len(), map.macros[0].0.as_str()), ("Kit", 2, "Cutoff"));
            let Some(FocusBody::Tree(tree)) =
                renoise.focus_body(&BodyRequest::Tree { track_ref: "1:track:1".into() }, Signal::new()).await.unwrap()
            else {
                panic!("the DSP chain")
            };
            assert_eq!(tree.devices.iter().map(|d| d.name.as_str()).collect::<Vec<_>>(), ["EQ 5", "Doofer"]);
            assert_eq!(tree.devices[1].can_have_chains, Some(true), "a Doofer holds devices");
            assert!(renoise.can_show(BodyKind::Tracker) && !renoise.can_show(BodyKind::Timeline));
            // (m4-tui) A line in Renoise's status bar, for a producer working there: through the host to the tool's show_status.
            assert!(renoise.has_show_message());
            assert!(renoise.show_message("Kumi · done · the bass is darker", Signal::new()).await.unwrap());
            let shown = script.asked().into_iter().rev().find(|(method, _)| method == "show_status").expect("asked of the tool");
            assert_eq!(shown.1["text"], "Kumi · done · the bass is darker");

            // A track inserted above Kick: the tool says so (`tracks`), the host passes it on, and track:1 follows Kick to
            // its new place; reads name the layout their refs are at.
            simulator.emit("tracks", json!({"count": 8, "made": [1], "gone": [], "layout": 1}));
            for _ in 0..100 {
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
                let _ = read.execute(json!({"kind": "device", "parent": "track:1"}).as_object().unwrap().clone(), Signal::new()).await;
                if last_read(&script)["parent"] == "1:track:2" {
                    break;
                }
            }
            let asked = last_read(&script);
            assert_eq!(asked, json!({"kind": "device", "parent": "1:track:2", "layout": 1}), "track:1 follows Kick");

            // The same shift again (it reached Kumi after the book was past it): nothing moves twice.
            simulator.emit("tracks", json!({"count": 8, "made": [1], "gone": [], "layout": 1}));
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            let _ = read.execute(json!({"kind": "device", "parent": "track:1"}).as_object().unwrap().clone(), Signal::new()).await;
            assert_eq!(last_read(&script)["parent"], "1:track:2", "track:1 is still Kick, not moved again");
            // A gap (layout 3 after 1): the shifts between are unknown, so the track names go.
            simulator.emit("tracks", json!({"count": 10, "made": [1], "gone": [], "layout": 3}));
            let mut last = String::new();
            for _ in 0..200 {
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
                last = read
                    .execute(json!({"kind": "device", "parent": "track:1"}).as_object().unwrap().clone(), Signal::new())
                    .await
                    .unwrap()
                    .text;
                if last.contains("isn't one of this turn's refs") {
                    break;
                }
            }
            assert!(last.contains("isn't one of this turn's refs"), "{last}");

            // Events dropped on the way (the host fell behind): a shift among them is unknown, so the track names go and are
            // read again; none is left on the wrong track.
            simulator.emit("events_lost", json!({}));
            // (Until the event lands, the read goes to the simulator, which has nothing scripted for that track's devices.)
            let mut last = String::new();
            for _ in 0..200 {
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
                let answer =
                    read.execute(json!({"kind": "device", "parent": "track:1"}).as_object().unwrap().clone(), Signal::new()).await.unwrap();
                last = answer.text;
                if answer.is_error && last.contains("isn't one of this turn's refs") {
                    break;
                }
            }
            assert!(last.contains("isn't one of this turn's refs"), "{last}");

            // Another song opens: the old song's refs are retired, so a stale track:1 reads nothing of the new song.
            script.switch_song();
            let next = renoise.observe(Signal::new(), None).await.unwrap();
            let context: Value = serde_json::from_str(&next.context).unwrap();
            assert_ne!(context["tracks"][0]["ref"], "track:1", "the new song's tracks get names never used before: {context}");
            let stale = next.tools[0]
                .execute(json!({"kind": "device", "parent": "track:1"}).as_object().unwrap().clone(), Signal::new())
                .await
                .unwrap();
            assert!(stale.is_error, "{}", stale.text);
            renoise.close().await.unwrap();
            simulator.stop();
        })
        .await;
}

/// One of the observation's tools, by name.
fn tool(observed: &Observation, name: &str) -> Rc<dyn KernelTool> {
    observed.tools.iter().find(|tool| tool.name() == name).cloned().unwrap_or_else(|| panic!("no {name} tool"))
}

fn input(value: Value) -> JsonObject {
    value.as_object().unwrap().clone()
}

#[tokio::test(flavor = "current_thread")]
async fn reapers_changes_go_through_kumis_host_into_history_and_back_out_as_new_steps() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let home = tempfile::tempdir().unwrap();
            let paths = BridgePaths::new(home.path(), Daw::Reaper);
            let secret = ensure_secret(&paths.secret()).unwrap();
            let script = ReaperScript::new(3, 0);
            let mut config = script.config(secret, "/lab/REAPER");
            config.discovery_dir = Some(paths.discovery());
            let simulator = Simulator::start(config).await.unwrap();
            let endpoint = Rc::new(InProcess { host: Arc::new(Host::new(Daw::Reaper, home.path())), id: Cell::new(0) });
            let connect: Connect = Rc::new(move |_| {
                let endpoint = endpoint.clone();
                async move { Ok(endpoint as Rc<dyn McpEndpoint>) }.boxed_local()
            });
            let told: Rc<RefCell<Vec<ChangeRecord>>> = Rc::default();
            let into = told.clone();
            let mut context = IntegrationContext::new(Rc::new(|_, _| {}));
            context.on_change = Some(Rc::new(move |record| into.borrow_mut().push(record)));
            let reaper = create_reaper_integration(OwnOptions { context, kumi_home: home.path().into(), connect: Some(connect) });
            reaper.start(Signal::new()).await.unwrap();
            let observed = reaper.observe(Signal::new(), None).await.unwrap();
            let tracks: Value = serde_json::from_str::<Value>(&observed.context).unwrap()["tracks"].clone();
            let (first, second) = (tracks[1]["ref"].as_str().unwrap().to_owned(), tracks[2]["ref"].as_str().unwrap().to_owned());
            assert!(observed.instructions.contains("undo_change takes back one of yours as a new step"), "{}", observed.instructions);

            // A plan: a fader, a track added and named by what the step before made, a mute. Each step is one change in
            // HISTORY, in REAPER's words, made through the host's journal.
            let plan = tool(&observed, "make_changes");
            let steps = json!({"steps": [
                {"tool": "track_volume", "input": {"ref": first, "volumeDb": -6}},
                {"tool": "track_add", "as": "pad", "input": {"name": "Pad"}},
                {"tool": "track_rename", "input": {"ref": "@pad", "name": "Pads"}},
                {"tool": "track_mute", "input": {"ref": second, "mute": true}},
            ]});
            within_schema(&plan.input_schema(), &input(steps.clone()));
            let done = plan.execute(input(steps), Signal::new()).await.unwrap();
            assert!(!done.is_error, "{}", done.text);
            assert_eq!(script.volume(1), -6.0);
            assert_eq!((script.track(4)["name"].clone(), script.track(2)["mute"].clone()), (json!("Pads"), json!(true)));
            let titles: Vec<String> = told.borrow().iter().map(|record| record.title.clone()).collect();
            assert_eq!(titles, ["Track 1 volume -6 dB", "Added track “Pad”", "Renamed track “Pad” → “Pads”", "Track 2 muted"]);
            // The fader's picture: where it was, where it went, within its span.
            let fader = told.borrow()[0].clone();
            assert_eq!((fader.from, fader.to, fader.range), (Some(0.0), Some(-6.0), Some([-60.0, 12.0])));
            assert_eq!(told.borrow()[3].range, None, "a mute has no meter");
            assert!(!done.text.contains("r:track:"), "no long ref reaches the model: {}", done.text);
            // The plan is one step of REAPER's undo, named by what it did.
            let undo_steps = script.undo_history();
            assert_eq!(undo_steps.len(), 1, "{undo_steps:?}");
            assert!(undo_steps[0].starts_with("Kumi: Track 1 volume -6 dB and 3 more (k"), "{undo_steps:?}");

            // A look the producer stops (Esc) while REAPER is slow to answer: the host stays, with its journal, and HISTORY
            // with it (a host started again would retire every change).
            simulator.hold("snapshot");
            let stop = Signal::new();
            let stopper = stop.clone();
            tokio::task::spawn_local(async move {
                tokio::time::sleep(std::time::Duration::from_millis(300)).await;
                stopper.cancel();
            });
            let stopped = reaper.observe(stop, None).await;
            assert!(stopped.as_ref().is_err_and(RuntimeError::is_aborted), "{:?}", stopped.map(|o| o.label));
            simulator.release();

            // The next look lists them for the model, each applied.
            let again = reaper.observe(Signal::new(), None).await.unwrap();
            let latest = serde_json::from_str::<Value>(&again.context).unwrap()["kumiChanges"].clone();
            assert_eq!(latest.as_array().map(Vec::len), Some(4), "{latest}");
            assert!(latest.as_array().unwrap().iter().all(|row| row["state"] == "applied"), "{latest}");

            // Undo the last: the mute goes, as a new step of REAPER's undo.
            let undo = tool(&again, "undo_change");
            let undone = undo.execute(input(json!({"change": "last", "final": true})), Signal::new()).await.unwrap();
            assert!(!undone.is_error, "{}", undone.text);
            assert_eq!(undone.reply.as_deref(), Some("Undone: Track 2 muted."));
            assert_eq!(script.track(2)["mute"], json!(false));
            assert_eq!(script.undo_history().len(), 2, "the undo is a new step");

            // The producer moved the fader since: Kumi leaves it, says why, and HISTORY keeps the change.
            script.set_volume(1, -3.0);
            let volume = told.borrow()[0].id.clone();
            let kept = undo.execute(input(json!({"change": volume})), Signal::new()).await.unwrap();
            assert!(kept.is_error && kept.text.contains("changed since Kumi set it"), "{}", kept.text);
            assert_eq!(script.volume(1), -3.0);
            assert_eq!(told.borrow().last().map(|record| record.state), Some(ChangeState::Kept));

            // A change on its own is a step of its own, named by its title.
            let third = tracks[3]["ref"].as_str().unwrap().to_owned();
            let renamed = tool(&again, "make_changes")
                .execute(input(json!({"steps": [{"tool": "track_rename", "input": {"ref": third, "name": "Lead"}}]})), Signal::new())
                .await
                .unwrap();
            assert!(!renamed.is_error, "{}", renamed.text);
            assert!(
                script.undo_history().last().unwrap().starts_with("Kumi: Renamed track “Track 3” → “Lead” (k"),
                "{:?}",
                script.undo_history()
            );
            // /undo in the app takes back the latest change still standing.
            let record = reaper.undo(None, Signal::new()).await.unwrap();
            assert_eq!(record.title, "Renamed track “Track 3” → “Lead”");
            assert_eq!(script.track(3)["name"], json!("Track 3"));

            // The producer presses REAPER's undo on the plan: the bridge names its step, and HISTORY marks the plan's
            // changes as undone, the fader Kumi kept too (REAPER's undo reached past the producer's own edit back to before
            // the plan, so Kumi's change is gone: accept-reaper).
            let plan_step = script.undo_history()[0].clone();
            simulator.emit("undone_in_daw", json!({"descs": [plan_step], "how": "undo"}));
            let state_of = |title: &str| told.borrow().iter().rev().find(|record| record.title == title).map(|record| record.state);
            for _ in 0..100 {
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                if state_of("Added track “Pad”") == Some(ChangeState::Undone) {
                    break;
                }
            }
            assert_eq!(
                (state_of("Added track “Pad”"), state_of("Renamed track “Pad” → “Pads”"), state_of("Track 1 volume -6 dB")),
                (Some(ChangeState::Undone), Some(ChangeState::Undone), Some(ChangeState::Undone))
            );

            // A value it already has: nothing to change, no step and no row.
            let rows = told.borrow().len();
            let same = tool(&again, "track_volume").execute(input(json!({"ref": first, "volumeDb": -3})), Signal::new()).await.unwrap();
            assert!(!same.is_error && same.text.contains("\"unchanged\":true"), "{}", same.text);
            assert_eq!(told.borrow().len(), rows);
            // A ref this turn never gave: refused in Kumi's words, nothing sent.
            let unknown =
                tool(&again, "track_volume").execute(input(json!({"ref": "track:99", "volumeDb": -3})), Signal::new()).await.unwrap();
            assert!(unknown.is_error && unknown.text.contains("isn't one of this turn's refs"), "{}", unknown.text);

            // A dialog during the first step's preview, closed before the next: the plan stops there, and nothing after it
            // is made out of order.
            script.busy_once();
            let asked_before = script.asked().len();
            let held = tool(&again, "make_changes")
                .execute(
                    input(json!({"steps": [
                        {"tool": "track_volume", "input": {"ref": first, "volumeDb": -9}},
                        {"tool": "track_mute", "input": {"ref": second, "mute": true}},
                        {"tool": "track_volume", "input": {"ref": second, "volumeDb": -12}},
                    ]})),
                    Signal::new(),
                )
                .await
                .unwrap();
            assert!(
                held.is_error && held.text.contains("REAPER is busy") && held.text.contains("the steps after it weren't run"),
                "{}",
                held.text
            );
            let previews = script.asked()[asked_before..].iter().filter(|(method, _)| method == "change_preview").count();
            assert_eq!(previews, 1, "only the first step was asked");
            assert_eq!((script.volume(1), script.track(2)["mute"].clone(), script.volume(2)), (-3.0, json!(false), 0.0));
            reaper.close().await.unwrap();
            simulator.stop();
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn renoises_changes_follow_its_epoch_and_its_undo_that_skips_mute() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let home = tempfile::tempdir().unwrap();
            let paths = BridgePaths::new(home.path(), Daw::Renoise);
            let secret = ensure_secret(&paths.secret()).unwrap();
            let script = RenoiseScript::new();
            let mut config = script.config_with_changes(secret, "/lab/Renoise");
            config.discovery_dir = Some(paths.discovery());
            let simulator = Simulator::start(config).await.unwrap();
            let endpoint = Rc::new(InProcess { host: Arc::new(Host::new(Daw::Renoise, home.path())), id: Cell::new(0) });
            let connect: Connect = Rc::new(move |_| {
                let endpoint = endpoint.clone();
                async move { Ok(endpoint as Rc<dyn McpEndpoint>) }.boxed_local()
            });
            let told: Rc<RefCell<Vec<ChangeRecord>>> = Rc::default();
            let into = told.clone();
            let mut context = IntegrationContext::new(Rc::new(|_, _| {}));
            context.on_change = Some(Rc::new(move |record| into.borrow_mut().push(record)));
            let renoise = create_renoise_integration(OwnOptions { context, kumi_home: home.path().into(), connect: Some(connect) });
            renoise.start(Signal::new()).await.unwrap();
            let observed = renoise.observe(Signal::new(), None).await.unwrap();
            assert!(observed.instructions.contains("devices go at 2 or later"), "{}", observed.instructions);

            // Track 1's devices, read in the song's first epoch.
            let read = tool(&observed, "renoise_read");
            let devices = read.execute(input(json!({"kind": "device", "parent": "track:1"})), Signal::new()).await.unwrap();
            let devices: Value = serde_json::from_str(&devices.text).unwrap();
            let first_device = devices["items"][0]["ref"].as_str().unwrap().to_owned();

            // A fader, then a device: the device moves Renoise's epoch, and the ref it made is named in the new one.
            let plan = tool(&observed, "make_changes");
            let steps = json!({"steps": [
                {"tool": "track_volume", "input": {"ref": "track:1", "volumeDb": -6}},
                {"tool": "device_add", "as": "eq", "input": {"ref": "track:1", "path": "EQ 10"}},
            ]});
            within_schema(&plan.input_schema(), &input(steps.clone()));
            let done = plan.execute(input(steps), Signal::new()).await.unwrap();
            assert!(!done.is_error, "{}", done.text);
            assert_eq!(script.track_state(1), (-6.0, false, 1));
            assert!(done.text.contains("\"ref\":\"device:"), "the device it made, in a short ref: {}", done.text);
            assert!(!done.text.contains(":device:"), "no long ref reaches the model: {}", done.text);
            // Only the devices moved (`kept`): the track names stay, and Renoise takes them; mute is no step of its undo,
            // and Kumi says so.
            let steps_before = script.undo_history().len();
            let muted = tool(&observed, "track_mute").execute(input(json!({"ref": "track:1", "mute": true})), Signal::new()).await.unwrap();
            assert!(
                !muted.is_error && muted.text.contains("Track 1 muted (Renoise's own undo won't take this back; Kumi can)"),
                "{}",
                muted.text
            );
            let row = told.borrow().iter().rev().find(|record| record.title == "Track 1 muted").cloned().unwrap();
            assert_eq!(row.note.as_deref(), Some("Renoise's own undo won't take this back; Kumi can."), "HISTORY's row says so too");
            assert_eq!(script.track_state(1).1, true);
            assert_eq!(script.undo_history().len(), steps_before, "no step in Renoise's undo");
            // The device names from before are retired: refused in Kumi's words, nothing sent.
            let old = plan
                .execute(
                    input(json!({"steps": [{"tool": "device_bypass", "input": {"ref": first_device, "bypass": true}}]})),
                    Signal::new(),
                )
                .await
                .unwrap();
            assert!(old.is_error && old.text.contains("isn't one of this turn's refs"), "{}", old.text);
            assert!(!old.text.contains(":device:"), "{}", old.text);
            let again = renoise.observe(Signal::new(), None).await.unwrap();

            // Kumi undoes each as a new step: the mute, the device (the epoch moves again, and Kumi follows), the fader.
            let undo = tool(&again, "undo_change");
            for title in ["Track 1 muted", "Added “EQ 10” to “Track 1”", "Track 1 volume -6 dB"] {
                let id = told.borrow().iter().find(|record| record.title == title).map(|record| record.id.clone()).unwrap();
                let undone = undo.execute(input(json!({"change": id})), Signal::new()).await.unwrap();
                assert!(!undone.is_error, "{title}: {}", undone.text);
            }
            assert_eq!(script.track_state(1), (0.0, false, 0));
            assert!(
                script.undo_history().iter().filter(|step| step.starts_with("Kumi: undo ")).count() == 3,
                "{:?}",
                script.undo_history()
            );

            // The producer moved the fader after Kumi's change: Kumi's undo leaves it.
            let latest = renoise.observe(Signal::new(), None).await.unwrap();
            let kick = serde_json::from_str::<Value>(&latest.context).unwrap()["tracks"][0]["ref"].as_str().unwrap().to_owned();
            let set = tool(&latest, "track_volume").execute(input(json!({"ref": kick, "volumeDb": -3})), Signal::new()).await.unwrap();
            assert!(!set.is_error, "{}", set.text);
            script.set_volume(1, -1.0);
            let kept = tool(&latest, "undo_change").execute(input(json!({"change": "last"})), Signal::new()).await.unwrap();
            assert!(kept.is_error && kept.text.contains("changed in Renoise since Kumi set it"), "{}", kept.text);
            assert_eq!(script.track_state(1).0, -1.0);
            // Lua run in Renoise: a HISTORY row, and only Renoise's own undo takes it back (Kumi's undo leaves it, in Renoise's words).
            let ran = plan
                .execute(input(json!({"steps": [{"tool": "run_lua", "input": {"code": "song.transport.bpm = 140"}}]})), Signal::new())
                .await
                .unwrap();
            assert!(!ran.is_error && ran.text.contains("Ran Lua"), "{}", ran.text);
            assert!(
                ran.text.contains("\"printed\":[\"set\"]") && ran.text.contains("\"result\":140"),
                "what it printed and returned: {}",
                ran.text
            );
            assert!(script.undo_history().last().is_some_and(|step| step.contains("Ran Lua")), "{:?}", script.undo_history());
            let left = tool(&latest, "undo_change").execute(input(json!({"change": "last"})), Signal::new()).await.unwrap();
            assert!(left.is_error && left.text.contains("Renoise's own undo can"), "{}", left.text);
            let row = told.borrow().iter().rev().find(|record| record.title.starts_with("Ran Lua")).cloned().unwrap();
            assert_eq!(row.state, ChangeState::Kept, "{row:?}");
            // The song changed behind Kumi's back (another epoch): the tool refuses, and Kumi says so in the model's ref.
            script.switch_song();
            let stale = tool(&latest, "track_volume").execute(input(json!({"ref": kick, "volumeDb": -2})), Signal::new()).await.unwrap();
            assert!(stale.is_error && stale.text.starts_with(&format!("{kick} is from before the song changed")), "{}", stale.text);
            renoise.close().await.unwrap();
            simulator.stop();
        })
        .await;
}

/// Reviews K11 and K37: closing the integration (a /reconnect, a restart) ends Kumi's undo with its host, and each row
/// says so in Renoise's words: a mute, which Renoise's undo skips, can be changed only by hand now; a fader Renoise's
/// undo may still take back; and a change Kumi undid may be back, if Renoise's undo took Kumi's undo back unseen.
#[tokio::test(flavor = "current_thread")]
async fn closing_retires_each_row_in_the_daws_words() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let home = tempfile::tempdir().unwrap();
            let paths = BridgePaths::new(home.path(), Daw::Renoise);
            let secret = ensure_secret(&paths.secret()).unwrap();
            let script = RenoiseScript::new();
            let mut config = script.config_with_changes(secret, "/lab/Renoise");
            config.discovery_dir = Some(paths.discovery());
            let simulator = Simulator::start(config).await.unwrap();
            let endpoint = Rc::new(InProcess { host: Arc::new(Host::new(Daw::Renoise, home.path())), id: Cell::new(0) });
            let connect: Connect = Rc::new(move |_| {
                let endpoint = endpoint.clone();
                async move { Ok(endpoint as Rc<dyn McpEndpoint>) }.boxed_local()
            });
            let told: Rc<RefCell<Vec<ChangeRecord>>> = Rc::default();
            let into = told.clone();
            let mut context = IntegrationContext::new(Rc::new(|_, _| {}));
            context.on_change = Some(Rc::new(move |record| into.borrow_mut().push(record)));
            let renoise = create_renoise_integration(OwnOptions { context, kumi_home: home.path().into(), connect: Some(connect) });
            renoise.start(Signal::new()).await.unwrap();
            let observed = renoise.observe(Signal::new(), None).await.unwrap();
            let plan = tool(&observed, "make_changes");
            let steps = json!({"steps": [
                {"tool": "track_volume", "input": {"ref": "track:1", "volumeDb": -6}},
                {"tool": "track_mute", "input": {"ref": "track:1", "mute": true}},
                {"tool": "track_volume", "input": {"ref": "track:2", "volumeDb": -3}},
            ]});
            let done = plan.execute(input(steps), Signal::new()).await.unwrap();
            assert!(!done.is_error, "{}", done.text);
            let again = renoise.observe(Signal::new(), None).await.unwrap();
            let id = |title: &str| told.borrow().iter().find(|record| record.title == title).map(|record| record.id.clone()).unwrap();
            let undone = tool(&again, "undo_change").execute(input(json!({"change": id("Track 2 volume -3 dB")})), Signal::new()).await;
            assert!(!undone.unwrap().is_error);
            renoise.close().await.unwrap();
            let last = |title: &str| told.borrow().iter().rev().find(|record| record.title == title).cloned().unwrap();
            let fader = last("Track 1 volume -6 dB");
            assert_eq!(fader.state, ChangeState::Expired, "{fader:?}");
            assert!(fader.note.as_deref().is_some_and(|note| note.starts_with("Kumi's link to Renoise started again")), "{fader:?}");
            let mute = last("Track 1 muted");
            assert_eq!(mute.state, ChangeState::Expired, "{mute:?}");
            assert!(mute.note.as_deref().is_some_and(|note| note.contains("neither can now, so change it by hand")), "{mute:?}");
            assert!(!mute.note.as_deref().unwrap().contains("Live"), "{mute:?}");
            let back = last("Track 2 volume -3 dB");
            assert_eq!(back.state, ChangeState::Undone, "{back:?}");
            assert!(back.note.as_deref().is_some_and(|note| note.contains("took back Kumi's undo, this is in effect again")), "{back:?}");
            simulator.stop();
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn a_change_the_daws_undo_put_back_is_undone_again_and_an_uncertain_one_is_never_undone_blindly() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let home = tempfile::tempdir().unwrap();
            let paths = BridgePaths::new(home.path(), Daw::Reaper);
            let secret = ensure_secret(&paths.secret()).unwrap();
            let script = ReaperScript::new(3, 0);
            let mut config = script.config(secret, "/lab/REAPER");
            config.discovery_dir = Some(paths.discovery());
            let simulator = Simulator::start(config).await.unwrap();
            let endpoint = Rc::new(InProcess { host: Arc::new(Host::new(Daw::Reaper, home.path())), id: Cell::new(0) });
            let connect: Connect = Rc::new(move |_| {
                let endpoint = endpoint.clone();
                async move { Ok(endpoint as Rc<dyn McpEndpoint>) }.boxed_local()
            });
            let told: Rc<RefCell<Vec<ChangeRecord>>> = Rc::default();
            let into = told.clone();
            let mut context = IntegrationContext::new(Rc::new(|_, _| {}));
            context.on_change = Some(Rc::new(move |record| into.borrow_mut().push(record)));
            let reaper = create_reaper_integration(OwnOptions { context, kumi_home: home.path().into(), connect: Some(connect) });
            reaper.start(Signal::new()).await.unwrap();
            let observed = reaper.observe(Signal::new(), None).await.unwrap();
            let state_of = |title: &str| told.borrow().iter().rev().find(|record| record.title == title).map(|record| record.state);

            // Kumi undoes a change; the producer's Cmd-Z takes Kumi's undo back (REAPER names the undo's own step), so the
            // change is back: HISTORY follows, and Kumi's next undo is made again, not answered from before.
            let volume = tool(&observed, "track_volume");
            let made = volume.execute(input(json!({"ref": "track:2", "volumeDb": -6})), Signal::new()).await.unwrap();
            assert!(!made.is_error, "{}", made.text);
            let undo = tool(&observed, "undo_change");
            assert!(!undo.execute(input(json!({"change": "last"})), Signal::new()).await.unwrap().is_error);
            assert_eq!(script.volume(1), 0.0);
            let restore = script.undo_history().last().cloned().unwrap();
            assert!(restore.starts_with("Kumi: undo "), "{restore}");
            script.set_volume(1, -6.0);
            simulator.emit("undone_in_daw", json!({"descs": [restore], "how": "undo"}));
            for _ in 0..100 {
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                if state_of("Track 1 volume -6 dB") == Some(ChangeState::Applied) {
                    break;
                }
            }
            assert_eq!(state_of("Track 1 volume -6 dB"), Some(ChangeState::Applied), "the change is back");
            let again = undo.execute(input(json!({"change": "last"})), Signal::new()).await.unwrap();
            assert!(!again.is_error, "{}", again.text);
            assert_eq!(script.volume(1), 0.0, "undone again, for real");

            // An apply REAPER took and couldn't say how it ended: Unsure, the plan stops, and Kumi never undoes it blindly.
            script.uncertain_once();
            let plan = tool(&observed, "make_changes");
            let steps = json!({"steps": [
                {"tool": "track_mute", "input": {"ref": "track:2", "mute": true}},
                {"tool": "track_volume", "input": {"ref": "track:3", "volumeDb": -9}},
            ]});
            let stopped = plan.execute(input(steps), Signal::new()).await.unwrap();
            assert!(stopped.is_error && stopped.text.contains("may or may not have happened"), "{}", stopped.text);
            assert_eq!(script.volume(2), 0.0, "the step after it never ran");
            assert_eq!(state_of("Track 1 muted"), Some(ChangeState::Unsure));
            let blind = undo.execute(input(json!({"change": "last"})), Signal::new()).await.unwrap();
            assert!(blind.is_error && blind.text.contains("won't undo it blindly"), "{}", blind.text);
            assert_eq!(script.track(1)["mute"], json!(true), "left as REAPER has it");
            let note = told.borrow().iter().rev().find(|record| record.title == "Track 1 muted").and_then(|record| record.note.clone());
            assert!(note.as_deref().is_some_and(|note| note.contains("together with the plan's other changes")), "{note:?}");

            // Kumi's undo REAPER answered without being able to say how it ended: Unsure, with REAPER's own words leading
            // (in the answer and on the row), and never sent again.
            let made = volume.execute(input(json!({"ref": "track:3", "volumeDb": -4})), Signal::new()).await.unwrap();
            assert!(!made.is_error, "{}", made.text);
            script.uncertain_restore_once();
            let unsure = undo.execute(input(json!({"change": "last"})), Signal::new()).await.unwrap();
            assert!(unsure.is_error && unsure.text.contains("REAPER couldn't finish Kumi's undo"), "{}", unsure.text);
            assert!(unsure.text.contains("didn't finish putting it back in time"), "REAPER's words are kept: {}", unsure.text);
            assert!(!unsure.text.contains("didn't confirm"), "REAPER answered: {}", unsure.text);
            assert_eq!(state_of("Track 2 volume -4 dB"), Some(ChangeState::Unsure));
            let note =
                told.borrow().iter().rev().find(|record| record.title == "Track 2 volume -4 dB").and_then(|record| record.note.clone());
            assert!(
                note.as_deref().is_some_and(|note| note.starts_with("Kumi's undo didn't finish: ") && note.contains("in time")),
                "{note:?}"
            );
            let again = undo.execute(input(json!({"change": "last"})), Signal::new()).await.unwrap();
            assert!(again.is_error && again.text.contains("won't undo it again"), "{}", again.text);
            let kept =
                told.borrow().iter().rev().find(|record| record.title == "Track 2 volume -4 dB").and_then(|record| record.note.clone());
            assert_eq!(kept, note, "a refused repeat keeps what the row said of the undo before it");

            // An undo REAPER made in part says what it left out, in its answer and on the row: never read as whole.
            let made = volume.execute(input(json!({"ref": "track:2", "volumeDb": -5})), Signal::new()).await.unwrap();
            assert!(!made.is_error, "{}", made.text);
            script.left_out_once("Its 2 send envelope(s) were not put back.");
            let partial = undo.execute(input(json!({"change": "last"})), Signal::new()).await.unwrap();
            assert!(!partial.is_error && partial.text.contains("were not put back"), "{}", partial.text);
            let row = told.borrow().iter().rev().find(|record| record.title == "Track 1 volume -5 dB").cloned().unwrap();
            assert_eq!(row.state, ChangeState::Undone);
            assert_eq!(row.note.as_deref(), Some("Its 2 send envelope(s) were not put back."));
            reaper.close().await.unwrap();
            simulator.stop();
        })
        .await;
}

// (r-tracks) An action REAPER makes no undo point of is no row in HISTORY; one it does is a row only REAPER's undo takes
// back; and a save is an action, with the refs it names lengthened for the host.
#[tokio::test(flavor = "current_thread")]
async fn reapers_actions_make_a_row_only_with_a_point_and_saves_are_no_change() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let home = tempfile::tempdir().unwrap();
            let paths = BridgePaths::new(home.path(), Daw::Reaper);
            let secret = ensure_secret(&paths.secret()).unwrap();
            let script = ReaperScript::new(3, 0);
            let mut config = script.config(secret, "/lab/REAPER");
            config.discovery_dir = Some(paths.discovery());
            let simulator = Simulator::start(config).await.unwrap();
            let endpoint = Rc::new(InProcess { host: Arc::new(Host::new(Daw::Reaper, home.path())), id: Cell::new(0) });
            let connect: Connect = Rc::new(move |_| {
                let endpoint = endpoint.clone();
                async move { Ok(endpoint as Rc<dyn McpEndpoint>) }.boxed_local()
            });
            let told: Rc<RefCell<Vec<ChangeRecord>>> = Rc::default();
            let into = told.clone();
            let mut context = IntegrationContext::new(Rc::new(|_, _| {}));
            context.on_change = Some(Rc::new(move |record| into.borrow_mut().push(record)));
            let reaper = create_reaper_integration(OwnOptions { context, kumi_home: home.path().into(), connect: Some(connect) });
            reaper.start(Signal::new()).await.unwrap();
            let observed = reaper.observe(Signal::new(), None).await.unwrap();
            let tracks: Value = serde_json::from_str::<Value>(&observed.context).unwrap()["tracks"].clone();
            let first = tracks[1]["ref"].as_str().unwrap().to_owned();
            let plan = tool(&observed, "make_changes");
            // Unselect all: no point of REAPER's, so no row, and the answer says why.
            let done =
                plan.execute(input(json!({"steps": [{"tool": "run_action", "input": {"action": 40297}}]})), Signal::new()).await.unwrap();
            assert!(!done.is_error && done.text.contains("no row in HISTORY"), "{}", done.text);
            assert!(told.borrow().is_empty(), "no row");
            // Insert track: a row, which only REAPER's undo takes back.
            let done =
                plan.execute(input(json!({"steps": [{"tool": "run_action", "input": {"action": 40001}}]})), Signal::new()).await.unwrap();
            assert!(!done.is_error, "{}", done.text);
            let rows: Vec<(String, Option<String>)> = told.borrow().iter().map(|r| (r.title.clone(), r.note.clone())).collect();
            assert_eq!(
                rows,
                [(
                    "Ran action “Track: Insert new track”".to_string(),
                    Some("Only REAPER's own undo can take this back (Edit > Undo).".to_string())
                )]
            );
            // Saving tracks as a template: an action, its track refs the host's long ones.
            let done = plan
                .execute(
                    input(json!({"steps": [{"tool": "save_track_template", "input": {"trackRefs": [first], "name": "Kit"}}]})),
                    Signal::new(),
                )
                .await
                .unwrap();
            assert!(!done.is_error, "{}", done.text);
            let asked = script.asked();
            let (_, params) = asked.iter().rev().find(|(method, _)| method == "track_template_save").expect("asked");
            assert_eq!(params["trackRefs"], json!(["r:track:{T0001}"]));
            assert!(params["epoch"].is_string(), "{params}");
            assert_eq!(told.borrow().len(), 1, "no row for a save");
            reaper.close().await.unwrap();
            simulator.stop();
        })
        .await;
}

/// The host in process, with the tools it was asked and what each answered, in order.
struct Recorded {
    inner: InProcess,
    calls: RefCell<Vec<(String, Value, Value)>>,
}
#[async_trait::async_trait(?Send)]
impl McpEndpoint for Recorded {
    fn pid(&self) -> Option<u32> {
        self.inner.pid()
    }
    fn server_info(&self) -> Option<kumi_runtime::mcp::types::Implementation> {
        self.inner.server_info()
    }
    async fn list(&self, cursor: Option<&str>, signal: Signal) -> Result<kumi_runtime::mcp::types::ListToolsResult, RuntimeError> {
        self.inner.list(cursor, signal).await
    }
    async fn call(&self, name: &str, args: JsonObject, signal: Signal) -> Result<kumi_runtime::mcp::types::CallToolResult, RuntimeError> {
        let answer = self.inner.call(name, args.clone(), signal).await;
        let shown = answer.as_ref().ok().and_then(|result| serde_json::to_value(result).ok()).unwrap_or(Value::Null);
        self.calls.borrow_mut().push((name.to_string(), Value::Object(args), shown));
        answer
    }
    fn on_catalog_changed(&self, listener: Rc<dyn Fn()>) -> Box<dyn Fn()> {
        self.inner.on_catalog_changed(listener)
    }
    fn on_disconnect(&self, listener: Rc<dyn Fn()>) -> Box<dyn Fn()> {
        self.inner.on_disconnect(listener)
    }
    fn on_live_event(&self, listener: Rc<dyn Fn(JsonObject)>) -> Box<dyn Fn()> {
        self.inner.on_live_event(listener)
    }
    fn stderr_status(&self) -> kumi_runtime::mcp::client::StderrStatus {
        self.inner.stderr_status()
    }
    async fn close(&self) -> Result<(), RuntimeError> {
        self.inner.close().await
    }
}

impl Recorded {
    /// The transactions the host was told to release.
    fn released(&self) -> Vec<String> {
        self.calls
            .borrow()
            .iter()
            .filter(|(name, _, _)| name == "change_release")
            .flat_map(|(_, args, _)| args["transactionIds"].as_array().cloned().unwrap_or_default())
            .filter_map(|id| id.as_str().map(str::to_owned))
            .collect()
    }

    /// The transaction of the last answer to `tool` whose structured answer `pick` takes.
    fn last_transaction(&self, tool: &str, pick: impl Fn(&Value) -> bool) -> Option<String> {
        self.calls.borrow().iter().rev().find(|(name, _, answer)| name == tool && pick(&answer["structuredContent"])).and_then(
            |(_, args, answer)| {
                answer["structuredContent"]["transactionId"].as_str().or_else(|| args["transactionId"].as_str()).map(str::to_owned)
            },
        )
    }
}

#[tokio::test(flavor = "current_thread")]
async fn an_m3_kinds_no_op_and_change_are_as_before_and_an_action_reaper_made_no_point_of_is_released_at_the_host() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let home = tempfile::tempdir().unwrap();
            let paths = BridgePaths::new(home.path(), Daw::Reaper);
            let secret = ensure_secret(&paths.secret()).unwrap();
            let script = ReaperScript::new(3, 0);
            let mut config = script.config(secret, "/lab/REAPER");
            config.discovery_dir = Some(paths.discovery());
            let simulator = Simulator::start(config).await.unwrap();
            let endpoint = Rc::new(Recorded {
                inner: InProcess { host: Arc::new(Host::new(Daw::Reaper, home.path())), id: Cell::new(0) },
                calls: RefCell::default(),
            });
            let recorded = endpoint.clone();
            let connect: Connect = Rc::new(move |_| {
                let endpoint = endpoint.clone();
                async move { Ok(endpoint as Rc<dyn McpEndpoint>) }.boxed_local()
            });
            let told: Rc<RefCell<Vec<ChangeRecord>>> = Rc::default();
            let into = told.clone();
            let mut context = IntegrationContext::new(Rc::new(|_, _| {}));
            context.on_change = Some(Rc::new(move |record| into.borrow_mut().push(record)));
            let reaper = create_reaper_integration(OwnOptions { context, kumi_home: home.path().into(), connect: Some(connect) });
            reaper.start(Signal::new()).await.unwrap();
            let observed = reaper.observe(Signal::new(), None).await.unwrap();
            let tracks: Value = serde_json::from_str::<Value>(&observed.context).unwrap()["tracks"].clone();
            let first = tracks[1]["ref"].as_str().unwrap().to_owned();
            let plan = tool(&observed, "make_changes");
            let settle = || async {
                for _ in 0..200 {
                    tokio::task::yield_now().await;
                }
            };
            // An M3 kind asked for what is so already: the preview says so, nothing is applied, no row, the host forgets it.
            let volume = |db: f64| input(json!({"steps": [{"tool": "track_volume", "input": {"ref": first, "volumeDb": db}}]}));
            let done = plan.execute(volume(0.0), Signal::new()).await.unwrap();
            assert!(!done.is_error && done.text.contains("it was so already"), "{}", done.text);
            settle().await;
            assert!(told.borrow().is_empty(), "no row");
            assert!(recorded.calls.borrow().iter().all(|(name, _, _)| name != "change_apply"), "nothing applied");
            let previewed = recorded.last_transaction("change_preview", |_| true).expect("a preview");
            assert!(recorded.released().contains(&previewed), "the host forgets the preview");
            // An M3 change: applied, a row, as ever (its apply says nothing of unchanged).
            let done = plan.execute(volume(-6.0), Signal::new()).await.unwrap();
            assert!(!done.is_error, "{}", done.text);
            let applied = recorded.last_transaction("change_apply", |_| true).expect("an apply");
            settle().await;
            assert_eq!(told.borrow().len(), 1);
            assert!(!recorded.released().contains(&applied), "its record is kept for its undo");
            // An action REAPER made no point of: applied, no row, and the host forgets its record too.
            let done =
                plan.execute(input(json!({"steps": [{"tool": "run_action", "input": {"action": 40297}}]})), Signal::new()).await.unwrap();
            assert!(!done.is_error && done.text.contains("no row in HISTORY"), "{}", done.text);
            let action = recorded
                .last_transaction("change_apply", |answer| answer["display"]["unchanged"] == true)
                .expect("an apply that changed nothing the project keeps");
            settle().await;
            assert!(recorded.released().contains(&action), "the host forgets it");
            assert_eq!(told.borrow().len(), 1, "no row for it");
            reaper.close().await.unwrap();
            simulator.stop();
        })
        .await;
}

/// (M7) Renoise with Kumi's host in front of the simulator's song, and the changes Kumi makes told to `told`.
async fn renoise_with_changes(
    home: &std::path::Path,
) -> (RenoiseScript, Simulator, Rc<dyn Integration>, Observation, Rc<RefCell<Vec<ChangeRecord>>>) {
    let paths = BridgePaths::new(home, Daw::Renoise);
    let secret = ensure_secret(&paths.secret()).unwrap();
    let script = RenoiseScript::new();
    script.listening().set_folder(&home.join("renders"));
    let mut config = script.config_with_changes(secret, "/lab/Renoise");
    config.discovery_dir = Some(paths.discovery());
    let simulator = Simulator::start(config).await.unwrap();
    let (renoise, observed, told) = kumi_on_renoise(home).await;
    (script, simulator, renoise, observed, told)
}

/// (M7) Kumi (another run of it, when called again) on the Renoise the simulator plays.
async fn kumi_on_renoise(home: &std::path::Path) -> (Rc<dyn Integration>, Observation, Rc<RefCell<Vec<ChangeRecord>>>) {
    let endpoint = Rc::new(InProcess { host: Arc::new(Host::new(Daw::Renoise, home)), id: Cell::new(0) });
    let connect: Connect = Rc::new(move |_| {
        let endpoint = endpoint.clone();
        async move { Ok(endpoint as Rc<dyn McpEndpoint>) }.boxed_local()
    });
    let told: Rc<RefCell<Vec<ChangeRecord>>> = Rc::default();
    let into = told.clone();
    let mut context = IntegrationContext::new(Rc::new(|_, _| {}));
    context.on_change = Some(Rc::new(move |record| into.borrow_mut().push(record)));
    let renoise = create_renoise_integration(OwnOptions { context, kumi_home: home.into(), connect: Some(connect) });
    renoise.start(Signal::new()).await.unwrap();
    let observed = renoise.observe(Signal::new(), None).await.unwrap();
    (renoise, observed, told)
}

async fn run_tool(tool: &Rc<dyn KernelTool>, value: Value) -> Value {
    let done = tool.execute(input(value.clone()), Signal::new()).await.unwrap();
    assert!(!done.is_error, "{value}: {}", done.text);
    serde_json::from_str(&done.text).unwrap()
}

const ARP: &str = r#"[{"line": 0, "notes": [{"col": 1, "note": "C-4"}]}, {"line": 4, "notes": [{"col": 1, "note": "E-4"}]},
    {"line": 8, "notes": [{"col": 1, "note": "G-4"}]}, {"line": 12, "notes": [{"col": 1, "note": "B-4", "volume": "40"}]}]"#;

/// The user's loop (M7): "make me 9 phrases for this instrument" — then "I like these 5, change these 4 in different ways" —
/// then the rest dropped. Each take is its own change; the kept takes are marked and never touched; phrase takes are heard.
#[tokio::test(flavor = "current_thread")]
async fn nine_phrases_keep_five_change_four_in_different_ways_and_drop_the_rest_in_one_step() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let home = tempfile::tempdir().unwrap();
            let (script, simulator, renoise, observed, told) = renoise_with_changes(home.path()).await;
            let read = tool(&observed, "renoise_read");
            let instruments = run_tool(&read, json!({"kind": "instrument"})).await;
            let pad = instruments["items"][1]["ref"].as_str().unwrap().to_owned();
            let variations = tool(&observed, "variations");
            within_schema(&variations.input_schema(), &input(json!({"action": "make", "source": {"instrumentRef": pad}, "count": 9, "seed": 7})));
            let arp: Value = serde_json::from_str(ARP).unwrap();
            let made = run_tool(
                &variations,
                json!({"action": "make", "source": {"instrumentRef": pad}, "takes": [{"direction": "rising arp", "lines": arp}], "count": 9, "seed": 7}),
            )
            .await;
            let takes = made["takes"].as_array().unwrap();
            assert_eq!(takes.len(), 9, "{made}");
            assert!(made["skipped"].as_array().unwrap().is_empty(), "{made}");
            assert!(takes[0]["line"].as_str().unwrap().starts_with("V1 (rising arp): lines as written"), "{made}");
            assert!(takes.iter().all(|take| take["line"].as_str().unwrap().contains(" · heard: peak −")), "phrase takes are heard: {made}");
            assert!(!made.to_string().contains(":phrase:"), "no long ref reaches the model: {made}");
            let phrases = script.made_phrases(2);
            assert_eq!(phrases.len(), 9);
            assert_eq!(phrases[0].0, "V1 \u{b7} Pad");
            let distinct: std::collections::BTreeSet<String> =
                phrases.iter().map(|(_, rows)| rows.iter().map(Value::to_string).collect()).collect();
            assert_eq!(distinct.len(), 9, "nine different takes");
            assert_eq!(told.borrow().len(), 9, "each take its own change in HISTORY");

            let kept = run_tool(&variations, json!({"action": "keep", "names": ["V1", "V3", "V5", "V7", "V9"]})).await;
            assert_eq!(kept["rest"], json!(["V2", "V4", "V6", "V8"]), "{kept}");
            assert_eq!(told.borrow().len(), 14, "each kept take renamed, a change each");
            let names: Vec<String> = script.made_phrases(2).into_iter().map(|(name, _)| name).collect();
            assert_eq!(names[0], "V1 \u{b7} Pad \u{b7} kept");
            assert_eq!(names[1], "V2 \u{b7} Pad");
            let before = script.made_phrases(2);

            let changed = run_tool(&variations, json!({"action": "change", "names": ["rest"]})).await;
            let lines = changed["changed"].as_array().unwrap();
            assert_eq!(lines.len(), 4, "{changed}");
            let directions: std::collections::BTreeSet<String> =
                lines.iter().map(|line| line["line"].as_str().unwrap().split(['(', ')']).nth(1).unwrap().to_owned()).collect();
            assert_eq!(directions.len(), 4, "four different ways: {changed}");
            let after = script.made_phrases(2);
            for k in [0, 2, 4, 6, 8] {
                assert_eq!(after[k], before[k], "the kept V{} is as it was", k + 1);
            }
            for k in [1, 3, 5, 7] {
                assert_ne!(after[k].1, before[k].1, "V{} changed", k + 1);
            }
            assert_eq!(told.borrow().len(), 18, "one change per take changed");
            let refused = variations.execute(input(json!({"action": "change", "names": ["V3"]})), Signal::new()).await.unwrap();
            assert!(refused.is_error && refused.text.contains("V3 is kept"), "{}", refused.text);

            let dropped = run_tool(&variations, json!({"action": "drop"})).await;
            assert_eq!(dropped["dropped"], json!(["V2", "V4", "V6", "V8"]), "{dropped}");
            assert_eq!(told.borrow().len(), 19, "the rest go in one change");
            let left: Vec<String> = script.made_phrases(2).into_iter().map(|(name, _)| name).collect();
            assert!(left.iter().all(|name| name.ends_with(" \u{b7} kept")) && left.len() == 5, "{left:?}");

            // (a make on the same source goes on numbering: V10)
            let more = run_tool(&variations, json!({"action": "make", "source": {"instrumentRef": pad}, "takes": [{"from": "V1", "ops": [{"op": "octave", "amount": 1, "up": true}]}], "seed": 8})).await;
            assert_eq!(more["takes"][0]["take"], "V10", "{more}");
            renoise.close().await.unwrap();
            simulator.stop();
        })
        .await;
}

/// (V1) What Kumi kept outlives Kumi: another run of it finds the set (kept and shown takes), and after Renoise started again
/// (its places unknown) finds the takes by their names; "rest" never reaches a kept take.
#[tokio::test(flavor = "current_thread")]
async fn kept_takes_outlive_a_restart_of_kumi_and_of_renoise() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let home = tempfile::tempdir().unwrap();
            let (script, simulator, renoise, observed, _) = renoise_with_changes(home.path()).await;
            let read = tool(&observed, "renoise_read");
            let pad = run_tool(&read, json!({"kind": "instrument"})).await["items"][1]["ref"].as_str().unwrap().to_owned();
            let arp: Value = serde_json::from_str(ARP).unwrap();
            run_tool(&tool(&observed, "variations"), json!({"action": "make", "source": {"instrumentRef": pad}, "takes": [{"lines": arp}], "count": 4, "seed": 3, "listen": false})).await;
            run_tool(&tool(&observed, "variations"), json!({"action": "keep", "names": ["V2"]})).await;
            let kept = std::fs::read_dir(home.path().join("variations")).map(|dir| dir.count()).unwrap_or(0);
            assert_eq!(kept, 1, "the song's sets are kept in Kumi's folder");
            renoise.close().await.unwrap();

            // (Kumi again, and Renoise started again: places from before are unknown)
            script.forget_places();
            let (again, observed, told) = kumi_on_renoise(home.path()).await;
            let listed = run_tool(&tool(&observed, "variations"), json!({"action": "list"})).await;
            assert!(listed["takes"].as_array().unwrap().iter().any(|line| line.as_str().unwrap().starts_with("V2") && line.as_str().unwrap().ends_with("(kept)")), "{listed}");
            let dropped = run_tool(&tool(&observed, "variations"), json!({"action": "drop"})).await;
            assert_eq!(dropped["dropped"], json!(["V1", "V3", "V4"]), "found by their names, the kept one left: {dropped}");
            assert_eq!(told.borrow().len(), 1);
            let left: Vec<String> = script.made_phrases(2).into_iter().map(|(name, _)| name).collect();
            assert_eq!(left, ["V2 \u{b7} Pad \u{b7} kept"]);
            again.close().await.unwrap();
            simulator.stop();
        })
        .await;
}

/// (M7) "I like this chop: give me some similar variations": takes of a pattern track's lines as slots of their own, the anchors
/// and the source as they were; a take the producer edited is left out of the drop.
#[tokio::test(flavor = "current_thread")]
async fn variations_of_a_chop_are_slots_of_their_own_and_an_edited_take_is_never_dropped() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let home = tempfile::tempdir().unwrap();
            let (script, simulator, renoise, observed, told) = renoise_with_changes(home.path()).await;
            let chop = json!([
                {"line": 0, "notes": [{"col": 1, "note": "C-4", "instrument": "00"}], "effects": [{"col": 1, "number": "0S", "amount": "00"}]},
                {"line": 3, "notes": [{"col": 1, "note": "C-4", "instrument": "00", "volume": "20"}], "effects": [{"col": 1, "number": "0S", "amount": "40"}]},
                {"line": 4, "notes": [{"col": 1, "note": "C-4", "instrument": "00"}], "effects": [{"col": 1, "number": "0S", "amount": "80"}]},
                {"line": 6, "notes": [{"col": 1, "note": "C-4", "instrument": "00"}], "effects": [{"col": 1, "number": "0S", "amount": "60"}]},
                {"line": 7, "notes": [{"col": 1, "note": "C-4", "instrument": "00", "volume": "30"}], "effects": [{"col": 1, "number": "0S", "amount": "20"}]},
                {"line": 12, "notes": [{"col": 1, "note": "C-4", "instrument": "00"}], "effects": [{"col": 1, "number": "0S", "amount": "80"}]},
                {"line": 14, "notes": [{"col": 1, "note": "C-4", "instrument": "00"}], "effects": [{"col": 1, "number": "0S", "amount": "A0"}]},
                {"line": 20, "notes": [{"col": 1, "note": "C-4", "instrument": "00"}]},
            ]);
            script.set_lines(2, 1, chop.clone());
            let read = tool(&observed, "renoise_read");
            let patterns = run_tool(&read, json!({"kind": "pattern"})).await;
            let second = patterns["items"][1]["ref"].as_str().unwrap().to_owned();
            let variations = tool(&observed, "variations");
            let made = run_tool(
                &variations,
                json!({"action": "make", "source": {"patternRef": second, "trackRef": "track:1", "first": 0, "last": 15}, "anchors": [0, 4], "seed": 3,
                    "takes": [{"direction": "swapped slices", "ops": [{"op": "shuffle", "amount": 1}]},
                              {"direction": "more rolls", "ops": [{"op": "retrig", "amount": 0.5}]},
                              {"direction": "reversed hits", "ops": [{"op": "reverse", "amount": 0.5}]}]}),
            )
            .await;
            assert_eq!(made["takes"].as_array().unwrap().len(), 3, "{made}");
            assert!(made["source"].as_str().unwrap().starts_with("lines 00–15 of"), "{made}");
            assert!(made["note"].as_str().unwrap().contains("play at the song's end"), "{made}");
            let slots = script.variation_slots();
            assert_eq!(
                slots.iter().map(|(name, _)| name.as_str()).collect::<Vec<_>>(),
                ["V1 \u{b7} Pattern 01", "V2 \u{b7} Pattern 01", "V3 \u{b7} Pattern 01"]
            );
            assert_eq!(script.lines(2, 1), chop, "the source is as it was");
            for (_, pattern) in &slots {
                let lines = script.lines(*pattern, 1);
                let at = |n: u64| lines.as_array().unwrap().iter().find(|row| row["line"] == n).cloned();
                assert_eq!(at(0), Some(chop[0].clone()), "the anchors stay");
                assert_eq!(at(4), Some(chop[2].clone()), "the anchors stay");
                assert_eq!(at(20), Some(chop[7].clone()), "lines past the range are the source's");
                assert_ne!(lines, chop, "each take varies");
            }
            assert_eq!(told.borrow().len(), 3);

            // The producer edits V3; then keeps V2 (renamed, a change) and drops the rest: V3 is left, named.
            let third = slots[2].1;
            let mut edited = script.lines(third, 1);
            edited.as_array_mut().unwrap().push(json!({"line": 9, "notes": [{"col": 1, "note": "D-4"}]}));
            script.set_lines(third, 1, edited);
            run_tool(&variations, json!({"action": "keep", "names": ["V2"]})).await;
            let dropped = run_tool(&variations, json!({"action": "drop", "names": ["rest"]})).await;
            assert_eq!(dropped["dropped"], json!(["V1"]), "{dropped}");
            assert!(dropped["skipped"][0].as_str().unwrap().contains("V3 was edited in Renoise"), "{dropped}");
            assert_eq!(
                script.variation_slots().iter().map(|(name, _)| name.as_str()).collect::<Vec<_>>(),
                ["V2 \u{b7} Pattern 01 \u{b7} kept", "V3 \u{b7} Pattern 01"]
            );
            assert_eq!(told.borrow().len(), 5, "the rename, then one change for the drop");
            renoise.close().await.unwrap();
            simulator.stop();
        })
        .await;
}

/// (M7) Each pattern take is rendered before it's shown: one that clips where the source doesn't is drawn again and rewritten
/// (a change of its own each time, at most twice), and its line says what was heard. While Renoise plays, nothing is rendered.
#[tokio::test(flavor = "current_thread")]
async fn a_take_that_clips_where_the_source_doesnt_is_drawn_again_and_nothing_renders_while_renoise_plays() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let home = tempfile::tempdir().unwrap();
            let (script, simulator, renoise, observed, told) = renoise_with_changes(home.path()).await;
            script.set_lines(
                2,
                1,
                json!([
                    {"line": 0, "notes": [{"col": 1, "note": "C-4", "instrument": "00"}]},
                    {"line": 2, "notes": [{"col": 1, "note": "D-4", "instrument": "00", "volume": "30"}]},
                    {"line": 4, "notes": [{"col": 1, "note": "E-4", "instrument": "00"}]},
                    {"line": 6, "notes": [{"col": 1, "note": "C-4", "instrument": "00"}]},
                ]),
            );
            let variations = tool(&observed, "variations");
            let source = json!({"patternRef": "pattern:2", "trackRef": "track:1", "first": 0, "last": 7});
            run_tool(&tool(&observed, "renoise_read"), json!({"kind": "pattern"})).await;
            let rolls = json!([{"direction": "rolls", "ops": [{"op": "retrig", "amount": 0.5}]}]);
            let made = run_tool(&variations, json!({"action": "make", "source": source, "seed": 5, "takes": rolls})).await;
            assert!(made["takes"][0]["line"].as_str().unwrap().contains(" · heard: peak −"), "{made}");
            assert_eq!(told.borrow().len(), 1);
            // (the song's end plays louder: the takes clip there, the source doesn't)
            script.listening().set_level_from("1:track:1", 32., 1.0);
            let made = run_tool(&variations, json!({"action": "make", "source": source, "seed": 6, "takes": rolls})).await;
            let line = made["takes"][0]["line"].as_str().unwrap();
            assert!(line.contains("(drawn again after the render: it clips (peak −0.0 dBFS))"), "{made}");
            assert!(line.ends_with(" · heard: it clips (peak −0.0 dBFS)"), "still clipping after two draws, it says so: {made}");
            assert_eq!(told.borrow().len(), 4, "the take, and two rewrites: {made}");
            // (Renoise plays: a render would stop it)
            script.set_transport_playing(true);
            let renders = script.listening().renders().len();
            let made = run_tool(&variations, json!({"action": "make", "source": source, "seed": 7, "takes": rolls})).await;
            assert!(made["heard"][0].as_str().unwrap().starts_with("Renoise is playing"), "{made}");
            assert_eq!(script.listening().renders().len(), renders, "nothing rendered");
            renoise.close().await.unwrap();
            simulator.stop();
        })
        .await;
}

/// (#400 review notes) A pattern take whose varied track the producer wrote on outside the take's lines isn't dropped: its
/// digest covers only those lines, so the drop compares the rest with the source's, says why, and leaves it.
#[tokio::test(flavor = "current_thread")]
async fn a_pattern_take_written_on_outside_its_lines_isnt_dropped() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let home = tempfile::tempdir().unwrap();
            let (script, simulator, renoise, observed, _told) = renoise_with_changes(home.path()).await;
            script.set_lines(
                2,
                1,
                json!([
                    {"line": 0, "notes": [{"col": 1, "note": "C-4", "instrument": "00"}]},
                    {"line": 2, "notes": [{"col": 1, "note": "D-4", "instrument": "00", "volume": "30"}]},
                    {"line": 4, "notes": [{"col": 1, "note": "E-4", "instrument": "00"}]},
                    {"line": 6, "notes": [{"col": 1, "note": "C-4", "instrument": "00"}]},
                ]),
            );
            let variations = tool(&observed, "variations");
            run_tool(&tool(&observed, "renoise_read"), json!({"kind": "pattern"})).await;
            let source = json!({"patternRef": "pattern:2", "trackRef": "track:1", "first": 0, "last": 7});
            let rolls = json!([{"direction": "rolls", "ops": [{"op": "retrig", "amount": 0.5}]}, {"direction": "sparser", "ops": [{"op": "thin", "amount": 0.4}]}]);
            let made = run_tool(&variations, json!({"action": "make", "source": source, "seed": 5, "takes": rolls, "listen": false})).await;
            let first: u64 = made["takes"][0]["ref"].as_str().and_then(|r| r.rsplit(':').next()).and_then(|n| n.parse().ok()).unwrap_or_else(|| panic!("{made}"));
            // (the producer writes on the source past the lines Kumi varied: the takes are as Kumi wrote them, still droppable)
            let mut source_lines = script.lines(2, 1).as_array().cloned().unwrap_or_default();
            source_lines.push(json!({"line": 14, "notes": [{"col": 1, "note": "A-4", "instrument": "00"}]}));
            script.set_lines(2, 1, Value::Array(source_lines));
            // (the producer writes a note on V1's track past the lines Kumi varied)
            let mut lines = script.lines(first, 1).as_array().cloned().unwrap_or_default();
            lines.push(json!({"line": 12, "notes": [{"col": 1, "note": "G-4", "instrument": "00"}]}));
            script.set_lines(first, 1, Value::Array(lines));
            let dropped = run_tool(&variations, json!({"action": "drop"})).await;
            assert_eq!(dropped["dropped"], json!(["V2"]), "{dropped}");
            assert!(dropped["skipped"].to_string().contains("outside the varied ones"), "{dropped}");
            renoise.close().await.unwrap();
            simulator.stop();
        })
        .await;
}

/// (review W6, W9, W5) Phrase takes of a pattern's lines are heard through players of their phrases, and changed and dropped as
/// phrases; they're a set apart from the range's pattern takes. Speed is refused for pattern takes, before anything is drawn.
#[tokio::test(flavor = "current_thread")]
async fn phrase_takes_of_a_patterns_lines_are_heard_changed_and_dropped_as_phrases() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let home = tempfile::tempdir().unwrap();
            let (script, simulator, renoise, observed, told) = renoise_with_changes(home.path()).await;
            script.set_lines(
                2,
                1,
                json!([
                    {"line": 8, "notes": [{"col": 1, "note": "C-4", "instrument": "00"}]},
                    {"line": 10, "notes": [{"col": 1, "note": "D-4", "instrument": "00", "volume": "30"}]},
                    {"line": 12, "notes": [{"col": 1, "note": "E-4", "instrument": "00"}]},
                    {"line": 14, "notes": [{"col": 1, "note": "G-4", "instrument": "00"}]},
                ]),
            );
            run_tool(&tool(&observed, "renoise_read"), json!({"kind": "pattern"})).await;
            let variations = tool(&observed, "variations");
            let source = json!({"patternRef": "pattern:2", "trackRef": "track:1", "first": 8, "last": 15});
            let speed = variations
                .execute(
                    input(json!({"action": "make", "source": source, "takes": [{"ops": [{"op": "speed", "factor": 2}]}]})),
                    Signal::new(),
                )
                .await
                .unwrap();
            assert!(speed.is_error && speed.text.contains("speed is for phrase takes"), "{}", speed.text);
            let patterns = run_tool(&variations, json!({"action": "make", "source": source, "count": 1, "seed": 2, "listen": false})).await;
            assert_eq!(patterns["set"], 1, "{patterns}");
            let made = run_tool(&variations, json!({"action": "make", "source": source, "place": "phrases", "count": 2, "seed": 3})).await;
            assert_eq!(made["set"], 2, "phrase takes of the range are a set of their own: {made}");
            let takes = made["takes"].as_array().unwrap();
            assert_eq!(takes.len(), 2, "{made}");
            assert!(takes.iter().all(|take| take["line"].as_str().unwrap().contains(" \u{b7} heard: peak \u{2212}")), "heard: {made}");
            assert_eq!(script.made_phrases(1).len(), 2);
            let before = told.borrow().len();
            let lines_before = script.made_phrases(1)[0].1.clone();
            let changed = run_tool(&variations, json!({"action": "change", "names": ["V2"], "listen": false})).await;
            assert_eq!(changed["changed"].as_array().unwrap().len(), 1, "a phrase take rewritten as a phrase: {changed}");
            assert_eq!(told.borrow().len(), before + 1);
            assert_ne!(script.made_phrases(1)[0].1, lines_before, "its phrase holds the new lines");
            let dropped = run_tool(&variations, json!({"action": "drop"})).await;
            assert_eq!(dropped["dropped"], json!(["V2", "V3"]), "{dropped}");
            assert!(script.made_phrases(1).is_empty());
            renoise.close().await.unwrap();
            simulator.stop();
        })
        .await;
}

/// (review W10) When places are lost, a take is found only by a name no other phrase has: a copy the producer made of it, named
/// the same, is never dropped for it.
#[tokio::test(flavor = "current_thread")]
async fn a_copy_named_like_a_take_is_never_taken_for_it() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let home = tempfile::tempdir().unwrap();
            let (script, simulator, renoise, observed, _) = renoise_with_changes(home.path()).await;
            let read = tool(&observed, "renoise_read");
            let pad = run_tool(&read, json!({"kind": "instrument"})).await["items"][1]["ref"].as_str().unwrap().to_owned();
            let arp: Value = serde_json::from_str(ARP).unwrap();
            let variations = tool(&observed, "variations");
            run_tool(&variations, json!({"action": "make", "source": {"instrumentRef": pad}, "takes": [{"lines": arp}], "count": 2, "seed": 4, "listen": false})).await;
            let phrases = run_tool(&read, json!({"kind": "phrase", "parent": pad})).await;
            let second = phrases["items"].as_array().unwrap().iter().find(|row| row["name"] == "V2 \u{b7} Pad").cloned().unwrap();
            let rows = script.made_phrases(2)[1].1.clone();
            let copy = json!({"tool": "phrase_add", "input": {"ref": pad, "name": "V2 \u{b7} Pad", "lines": second["lines"], "lpb": second["lpb"], "content": rows}});
            run_tool(&tool(&observed, "make_changes"), json!({"steps": [copy]})).await;
            script.forget_places();
            let dropped = run_tool(&variations, json!({"action": "drop"})).await;
            assert_eq!(dropped["dropped"], json!(["V1"]), "{dropped}");
            assert!(dropped["skipped"][0].as_str().unwrap().contains("can't find V2"), "{dropped}");
            let names: Vec<String> = script.made_phrases(2).into_iter().map(|(name, _)| name).collect();
            assert_eq!(names, ["V2 \u{b7} Pad", "V2 \u{b7} Pad"], "the take and the producer's copy both stay");
            renoise.close().await.unwrap();
            simulator.stop();
        })
        .await;
}

/// (#400 review notes) A phrase take changed in speed and lines is two steps, the LPB first: the take names both, and Kumi's
/// undo takes back the lines, then the LPB.
#[tokio::test(flavor = "current_thread")]
async fn a_phrase_take_changed_in_speed_and_lines_names_both_changes_and_each_undoes() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let home = tempfile::tempdir().unwrap();
            let (script, simulator, renoise, observed, told) = renoise_with_changes(home.path()).await;
            let pad = run_tool(&tool(&observed, "renoise_read"), json!({"kind": "instrument"})).await["items"][1]["ref"].as_str().unwrap().to_owned();
            let arp: Value = serde_json::from_str(ARP).unwrap();
            let variations = tool(&observed, "variations");
            run_tool(&variations, json!({"action": "make", "source": {"instrumentRef": pad}, "takes": [{"lines": arp}], "count": 1, "seed": 5, "listen": false})).await;
            let before = script.made_phrases(2)[0].1.clone();
            let changed = run_tool(
                &variations,
                json!({"action": "change", "names": ["V1"], "takes": [{"direction": "faster, an octave up", "ops": [{"op": "speed", "factor": 2}, {"op": "transpose", "semitones": 12}]}], "listen": false}),
            )
            .await;
            let line = changed["changed"][0]["line"].as_str().unwrap().to_owned();
            let ids = regex::Regex::new(r"\((c\d+) and (c\d+)\)").unwrap().captures(&line).map(|found| (found[1].to_owned(), found[2].to_owned()));
            let (speed, lines) = ids.unwrap_or_else(|| panic!("both changes named: {line}"));
            assert_eq!(told.borrow().len(), 3, "the take, the LPB, then the lines");
            assert_ne!(script.made_phrases(2)[0].1, before, "its lines transposed");
            let undo = tool(&observed, "undo_change");
            for id in [lines, speed] {
                let undone = undo.execute(input(json!({"change": id})), Signal::new()).await.unwrap();
                assert!(!undone.is_error, "{id}: {}", undone.text);
            }
            assert_eq!(script.made_phrases(2)[0].1, before, "its lines as they were");
            renoise.close().await.unwrap();
            simulator.stop();
        })
        .await;
}

/// (review W12) A phrase take changed only in speed is one phrase_set: changed, told once, and still Kumi's to drop.
#[tokio::test(flavor = "current_thread")]
async fn a_phrase_take_changed_only_in_speed_is_one_change_and_stays_kumis() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let home = tempfile::tempdir().unwrap();
            let (script, simulator, renoise, observed, told) = renoise_with_changes(home.path()).await;
            let pad = run_tool(&tool(&observed, "renoise_read"), json!({"kind": "instrument"})).await["items"][1]["ref"].as_str().unwrap().to_owned();
            let arp: Value = serde_json::from_str(ARP).unwrap();
            let variations = tool(&observed, "variations");
            run_tool(&variations, json!({"action": "make", "source": {"instrumentRef": pad}, "takes": [{"lines": arp}], "count": 2, "seed": 5, "listen": false})).await;
            let before = script.made_phrases(2)[0].1.clone();
            let changed = run_tool(
                &variations,
                json!({"action": "change", "names": ["V1"], "takes": [{"direction": "twice as fast", "ops": [{"op": "speed", "factor": 2}]}], "listen": false}),
            )
            .await;
            assert_eq!(changed["changed"].as_array().unwrap().len(), 1, "{changed}");
            assert!(changed["changed"][0]["line"].as_str().unwrap().contains("LPB 8"), "{changed}");
            assert_eq!(told.borrow().len(), 3, "the two takes, then the LPB alone");
            assert_eq!(script.made_phrases(2)[0].1, before, "its lines as they were");
            let dropped = run_tool(&variations, json!({"action": "drop"})).await;
            assert_eq!(dropped["dropped"], json!(["V1", "V2"]), "never taken for edited: {dropped}");
            renoise.close().await.unwrap();
            simulator.stop();
        })
        .await;
}
