//! Catch-up and watch_me in REAPER and Renoise end to end (M6): Kumi's integration over its host (daw-mcp-server's own
//! code, in process) in front of a simulated DAW. What changed while Kumi was closed, or while the DAW was away, is told
//! to the producer and the model in the DAW's words; a routine the producer did while Kumi watched comes back as the
//! DAW's changes and a draft recipe of its change kinds.
use daw_bridge::{files::ensure_secret, paths::BridgePaths, Daw};
use daw_mcp_server::{
    host::Host,
    reaper::simulator::ReaperScript,
    renoise::simulator::RenoiseScript,
    simulator::{Simulator, SimulatorConfig},
};
use futures::FutureExt;
use kumi_common::abort::Signal;
use kumi_daw_reaper::create_reaper_integration;
use kumi_runtime::core::{context::IntegrationContext, contracts::*};
use kumi_runtime::integrations::{
    own_bridge::{Connect, OwnOptions},
    renoise::create_renoise_integration,
};
use kumi_runtime::mcp::client::McpEndpoint;
use regex::Regex;
use serde_json::{json, Value};
use std::{cell::Cell, cell::RefCell, path::Path, rc::Rc, sync::Arc};

kumi_testkit::in_process!();
use in_process::InProcess;

const PROJECT_ID: &str = "0123456789abcdef0123456789abcdef";

/// Live's words, which none of this may say in another DAW.
fn live_words(text: &str) -> bool {
    Regex::new(r"\b(Live|Set|Sets|Ableton|Session|Arrangement|Main|Drum Rack|[Rr]acks?|[Cc]lips?|[Ss]cenes?)\b").unwrap().is_match(text)
}

/// What one run of Kumi saw: catch-ups told, watching on and off.
#[derive(Default)]
struct Seen {
    catch_ups: RefCell<Vec<CatchUp>>,
    watching: RefCell<Vec<bool>>,
}

/// A run of Kumi: a host of its own (as each Kumi starts one) on the shared folder, and the DAW's integration over it.
fn kumi(daw: Daw, home: &Path, seen: Rc<Seen>) -> Rc<dyn Integration> {
    let endpoint = Rc::new(InProcess { host: Arc::new(Host::new(daw, home)), id: Cell::new(0) });
    let connect: Connect = Rc::new(move |_| {
        let endpoint = endpoint.clone();
        async move { Ok(endpoint as Rc<dyn McpEndpoint>) }.boxed_local()
    });
    let mut context = IntegrationContext::new(Rc::new(|_, _| {}));
    let into = seen.clone();
    context.on_catch_up = Some(Rc::new(move |catch_up| into.catch_ups.borrow_mut().push(catch_up)));
    let into = seen.clone();
    context.on_watch = Some(Rc::new(move |on| into.watching.borrow_mut().push(on)));
    context.focus_interval_ms = Some(50);
    let options = OwnOptions { context, kumi_home: home.into(), connect: Some(connect) };
    match daw {
        Daw::Reaper => create_reaper_integration(options),
        Daw::Renoise => create_renoise_integration(options),
    }
}

/// Waits (a few seconds at most) until `done` holds.
async fn until(mut done: impl FnMut() -> bool) {
    for _ in 0..400 {
        if done() {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    panic!("waited too long");
}

async fn start_simulator(config: SimulatorConfig, paths: &BridgePaths) -> Simulator {
    let mut config = config;
    config.discovery_dir = Some(paths.discovery());
    Simulator::start(config).await.unwrap()
}

#[tokio::test(flavor = "current_thread")]
async fn what_changed_in_reaper_while_kumi_was_closed_is_told_once_in_reapers_words() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let home = tempfile::tempdir().unwrap();
            let paths = BridgePaths::new(home.path(), Daw::Reaper);
            let secret = ensure_secret(&paths.secret()).unwrap();
            let script = ReaperScript::new(3, 0);
            script.set_project_id(Some(PROJECT_ID));
            let _simulator = start_simulator(script.config(secret, "/lab/REAPER"), &paths).await;

            // The first time Kumi sees the project there's nothing to tell; closing keeps it as Kumi left it.
            let first = Rc::new(Seen::default());
            let reaper = kumi(Daw::Reaper, home.path(), first.clone());
            reaper.start(Signal::new()).await.unwrap();
            reaper.observe(Signal::new(), None).await.unwrap();
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
            assert!(first.catch_ups.borrow().is_empty(), "nothing to tell the first time");
            reaper.close().await.unwrap();

            // Kumi is closed; the producer works in REAPER.
            script.set_volume(1, -6.0);
            script.rename(2, "Bass");
            script.add_fx_with(3, "ReaComp", &[("Threshold", 0.5)]);
            script.set_tempo(124.0);

            let second = Rc::new(Seen::default());
            let reaper = kumi(Daw::Reaper, home.path(), second.clone());
            reaper.start(Signal::new()).await.unwrap();
            let observed = reaper.observe(Signal::new(), None).await.unwrap();
            until(|| !second.catch_ups.borrow().is_empty()).await;
            let told = second.catch_ups.borrow()[0].clone();
            assert_eq!(
                told.lines,
                ["Tempo 120 → 124 BPM", "Renamed track “Track 2” → “Bass”", "Changed track “Track 1” (mixer)", "Added FX “ReaComp”"]
            );
            assert_eq!(told.set, observed.identity.as_ref().unwrap().name, "the project as the app shows it");
            assert!(told.last_seen_at > 0);
            assert_eq!(told.after_reconnect, None);
            // The model is told too, on every look while the project stays open.
            let context: Value = serde_json::from_str(&reaper.observe(Signal::new(), None).await.unwrap().context).unwrap();
            assert_eq!(context["sinceLastTime"]["changes"], json!(told.lines));
            assert_eq!(context["sinceLastTime"]["lastSeen"], "just now");
            for line in &told.lines {
                assert!(!live_words(line), "{line}");
            }
            // Told once: the next look doesn't ask again.
            assert_eq!(second.catch_ups.borrow().len(), 1);
            reaper.close().await.unwrap();
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn a_reaper_restarted_while_kumi_runs_is_told_as_changed_while_it_was_away() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let home = tempfile::tempdir().unwrap();
            let paths = BridgePaths::new(home.path(), Daw::Reaper);
            let secret = ensure_secret(&paths.secret()).unwrap();
            let script = ReaperScript::new(3, 0);
            script.set_project_id(Some(PROJECT_ID));
            let simulator = start_simulator(script.config(secret.clone(), "/lab/REAPER"), &paths).await;
            let seen = Rc::new(Seen::default());
            let reaper = kumi(Daw::Reaper, home.path(), seen.clone());
            reaper.start(Signal::new()).await.unwrap();
            reaper.observe(Signal::new(), None).await.unwrap();
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
            // REAPER quits; the producer opens the project again in a new REAPER and changes it.
            simulator.stop();
            let away = reaper.observe(Signal::new(), None).await.unwrap();
            assert!(away.label.contains("No REAPER access"), "{}", away.label);
            script.restart();
            script.set_volume(3, -12.0);
            let _again = start_simulator(script.config(secret, "/lab/REAPER"), &paths).await;
            reaper.observe(Signal::new(), None).await.unwrap();
            until(|| !seen.catch_ups.borrow().is_empty()).await;
            let told = seen.catch_ups.borrow()[0].clone();
            assert_eq!(told.after_reconnect, Some(true), "while REAPER was away");
            assert_eq!(told.lines, ["Changed track “Track 3” (mixer)"]);
            reaper.close().await.unwrap();
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn what_changed_in_renoise_while_kumi_was_closed_is_told_in_renoises_words() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let home = tempfile::tempdir().unwrap();
            let paths = BridgePaths::new(home.path(), Daw::Renoise);
            let secret = ensure_secret(&paths.secret()).unwrap();
            let script = RenoiseScript::new();
            let _simulator = start_simulator(script.config(secret, "/lab/Renoise/V3.5.4"), &paths).await;
            let first = Rc::new(Seen::default());
            let renoise = kumi(Daw::Renoise, home.path(), first.clone());
            renoise.start(Signal::new()).await.unwrap();
            renoise.observe(Signal::new(), None).await.unwrap();
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
            renoise.close().await.unwrap();
            // In Renoise meanwhile: the Hat track down, Bass named Sub (tracks have no lasting identity there), faster.
            script.set_volume(2, -6.0);
            script.rename_track(4, "Sub");
            script.set_bpm(140.0);
            let second = Rc::new(Seen::default());
            let renoise = kumi(Daw::Renoise, home.path(), second.clone());
            renoise.start(Signal::new()).await.unwrap();
            renoise.observe(Signal::new(), None).await.unwrap();
            until(|| !second.catch_ups.borrow().is_empty()).await;
            let told = second.catch_ups.borrow()[0].clone();
            assert_eq!(told.lines, ["Tempo 128 → 140 BPM", "Renamed track “Bass” → “Sub”", "Changed track “Hat” (mixer)"]);
            for line in &told.lines {
                assert!(!live_words(line), "{line}");
            }
            renoise.close().await.unwrap();
        })
        .await;
}

/// Every key of `input` is one the tool's schema offers, and what it requires is there.
fn within_schema(schema: &JsonObject, input: &JsonObject) {
    let properties = schema["properties"].as_object().unwrap();
    for key in input.keys() {
        assert!(properties.contains_key(key), "{key} isn't in the schema: {schema:?}");
    }
    for key in schema.get("required").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str) {
        assert!(input.contains_key(key), "{key} is required: {input:?}");
    }
}

#[tokio::test(flavor = "current_thread")]
async fn a_routine_watched_in_reaper_comes_back_as_reapers_changes_and_a_recipe_of_its_kinds() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let home = tempfile::tempdir().unwrap();
            let paths = BridgePaths::new(home.path(), Daw::Reaper);
            let secret = ensure_secret(&paths.secret()).unwrap();
            let script = ReaperScript::new(3, 0);
            let simulator = start_simulator(script.config(secret, "/lab/REAPER"), &paths).await;
            let seen = Rc::new(Seen::default());
            let reaper = kumi(Daw::Reaper, home.path(), seen.clone());
            reaper.start(Signal::new()).await.unwrap();
            let observed = reaper.observe(Signal::new(), None).await.unwrap();
            let watch = observed.tools.iter().find(|tool| tool.name() == "watch_me").expect("watch_me").clone();
            assert!(watch.deferred(), "asked for, not sent with every request");
            assert!(!live_words(watch.description()), "{}", watch.description());
            let input = |action: &str| json!({"action": action}).as_object().unwrap().clone();
            let started = watch.execute(input("start"), Signal::new()).await.unwrap();
            assert!(!started.is_error, "{}", started.text);
            assert!(started.text.contains("go ahead in REAPER"), "{}", started.text);
            // The producer, by hand: a compressor on Track 2 (selected), its threshold turned after a look saw it come in,
            // and Track 2 pulled down.
            simulator.emit("selection", json!({"tracks": [{"index": 2, "guid": "{T0002}", "name": "Track 2"}]}));
            let comp = script.add_fx_with(2, "ReaComp", &[("Threshold", 0.5), ("Ratio", 0.1)]);
            tokio::time::sleep(std::time::Duration::from_millis(1600)).await;
            script.set_param(&comp, 0, 0.3);
            script.set_volume(2, -4.5);
            let stopped = watch.execute(input("stop"), Signal::new()).await.unwrap();
            assert!(!stopped.is_error, "{}", stopped.text);
            assert_eq!(*seen.watching.borrow(), [true, false]);
            assert!(!live_words(&stopped.text), "{}", stopped.text);
            let reply: Value = serde_json::from_str(&stopped.text).unwrap();
            let changes = reply["changes"].as_array().unwrap();
            assert!(changes.iter().any(|c| c["changed"] == "track" && c["name"] == "Track 2"), "{reply}");
            let added = changes.iter().find(|c| c["added"] == "FX").unwrap_or_else(|| panic!("{reply}"));
            assert_eq!(added["on"], "Track 2");
            assert_eq!(added["parameters"], json!([{"name": "Threshold", "value": {"value": 0.3, "shows": "0.3"}}]));
            let steps = reply["recipeSteps"]["steps"].as_array().unwrap();
            assert_eq!(
                steps.iter().map(|step| step["tool"].as_str().unwrap()).collect::<Vec<_>>(),
                ["track_volume", "fx_add", "fx_params"],
                "{reply}"
            );
            assert_eq!(reply["recipeSteps"]["params"][0]["name"], "track", "the selected track is the blank");
            assert_eq!(steps[1]["input"], json!({"name": "ReaComp", "ref": "$track"}));
            // Each drafted step is one of REAPER's change kinds, as its schema takes it.
            for step in steps {
                let tool = observed
                    .tools
                    .iter()
                    .find(|tool| tool.name() == step["tool"].as_str().unwrap())
                    .unwrap_or_else(|| panic!("{} isn't a change kind here", step["tool"]));
                within_schema(&tool.input_schema(), step["input"].as_object().unwrap());
            }
            // Stopping again without starting is said in Kumi's words.
            let again = watch.execute(input("stop"), Signal::new()).await.unwrap();
            assert!(again.is_error && again.text.contains("start first"), "{}", again.text);
            reaper.close().await.unwrap();
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn an_unsaved_reaper_project_is_caught_up_while_reaper_keeps_it_open_and_a_new_one_has_nothing_to_tell() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let home = tempfile::tempdir().unwrap();
            let paths = BridgePaths::new(home.path(), Daw::Reaper);
            let secret = ensure_secret(&paths.secret()).unwrap();
            let script = ReaperScript::new(3, 0);
            let simulator = start_simulator(script.config(secret.clone(), "/lab/REAPER"), &paths).await;
            let first = Rc::new(Seen::default());
            let reaper = kumi(Daw::Reaper, home.path(), first.clone());
            reaper.start(Signal::new()).await.unwrap();
            assert!(reaper.observe(Signal::new(), None).await.unwrap().project.is_none(), "unsaved: REAPER's unsaved place");
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
            reaper.close().await.unwrap();
            // Kumi closes and opens again; REAPER kept the same unsaved project open, and the producer changed it.
            script.set_volume(2, -9.0);
            let second = Rc::new(Seen::default());
            let reaper = kumi(Daw::Reaper, home.path(), second.clone());
            reaper.start(Signal::new()).await.unwrap();
            reaper.observe(Signal::new(), None).await.unwrap();
            until(|| !second.catch_ups.borrow().is_empty()).await;
            assert_eq!(second.catch_ups.borrow()[0].lines, ["Changed track “Track 2” (mixer)"]);
            reaper.close().await.unwrap();
            // REAPER restarted on a new unsaved project: another project, nothing to compare.
            simulator.stop();
            script.restart();
            let _again = start_simulator(script.config(secret, "/lab/REAPER"), &paths).await;
            let third = Rc::new(Seen::default());
            let reaper = kumi(Daw::Reaper, home.path(), third.clone());
            reaper.start(Signal::new()).await.unwrap();
            reaper.observe(Signal::new(), None).await.unwrap();
            tokio::time::sleep(std::time::Duration::from_millis(300)).await;
            assert!(third.catch_ups.borrow().is_empty(), "{:?}", third.catch_ups.borrow());
            reaper.close().await.unwrap();
        })
        .await;
}
