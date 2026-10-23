//! Listening in REAPER and Renoise end to end: Kumi's integration and the shared listening tools over Kumi's host (in
//! process) in front of a bridge that renders, captures and plays like REAPER's extension or Renoise's tool. A part is
//! rendered offline and the main thing heard played out loud after; REAPER hears what it plays while it plays; a listen
//! stopped part way stops its render; the demo and an older bridge offer no listening. Nothing said is in Live's words,
//! and nothing says "clip".
use daw_bridge::{files::ensure_secret, paths::BridgePaths, Daw};
use daw_mcp_server::{
    host::Host,
    reaper::simulator::ReaperScript,
    renoise::simulator::RenoiseScript,
    simulator::{Simulator, SimulatorConfig},
};
use futures::FutureExt;
use kumi_common::abort::Signal;
use kumi_runtime::core::{context::IntegrationContext, contracts::*, errors::RuntimeError};
use kumi_runtime::integrations::{
    own_bridge::{Connect, OwnOptions},
    reaper::create_reaper_integration,
    renoise::create_renoise_integration,
};
use kumi_runtime::mcp::client::McpEndpoint;
use regex::Regex;
use serde_json::{json, Value};
use std::{cell::Cell, cell::RefCell, path::Path, rc::Rc, sync::Arc, sync::LazyLock};

kumi_testkit::in_process!();
use in_process::InProcess;

/// Live's words, whole, and a clip: none may reach the producer or the model over REAPER or Renoise.
static NOT_OURS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\b(Live|Set|Sets|Ableton|Session|Arrangement|Remote Script|Control Surface|Main|Drum Rack|Operator|EQ Eight|Utility|[Rr]acks?|[Cc]lips?|[Ss]cenes?)\b")
        .unwrap()
});

fn assert_ours(said: &[String]) {
    let found: Vec<String> = said
        .iter()
        .flat_map(|text| {
            NOT_OURS.find_iter(text).map(|word| format!("{:?} in {}", word.as_str(), &text[..text.len().min(400)])).collect::<Vec<_>>()
        })
        .collect();
    assert!(found.is_empty(), "{}", found.join("\n"));
}

fn object(value: Value) -> JsonObject {
    value.as_object().cloned().unwrap()
}

/// An integration over Kumi's host in front of `config`'s simulated bridge, with what it tells the app.
fn integration(daw: Daw, home: &Path, create: fn(OwnOptions) -> Rc<dyn Integration>) -> (Rc<dyn Integration>, Rc<RefCell<Vec<String>>>) {
    let endpoint = Rc::new(InProcess { host: Arc::new(Host::new(daw, home)), id: Cell::new(0) });
    let connect: Connect = Rc::new(move |_| {
        let endpoint = endpoint.clone();
        async move { Ok(endpoint as Rc<dyn McpEndpoint>) }.boxed_local()
    });
    let told: Rc<RefCell<Vec<String>>> = Rc::default();
    let into = told.clone();
    let mut context = IntegrationContext::new(Rc::new(|_, _| {}));
    context.on_action = Some(Rc::new(move |event: ActionEvent| into.borrow_mut().push(event.title)));
    context.focus_interval_ms = Some(50);
    (create(OwnOptions { context, kumi_home: home.into(), connect: Some(connect) }), told)
}

async fn reaper_lab(script: &ReaperScript, home: &Path, keep: impl FnOnce(&mut SimulatorConfig)) -> Simulator {
    let paths = BridgePaths::new(home, Daw::Reaper);
    let secret = ensure_secret(&paths.secret()).unwrap();
    script.listening().set_folder(&paths.root.join("listening"));
    let mut config = script.config(secret, "/lab/REAPER");
    config.discovery_dir = Some(paths.discovery());
    keep(&mut config);
    Simulator::start(config).await.unwrap()
}

async fn renoise_lab(script: &RenoiseScript, home: &Path) -> Simulator {
    let paths = BridgePaths::new(home, Daw::Renoise);
    let secret = ensure_secret(&paths.secret()).unwrap();
    script.listening().set_folder(&paths.root.join("listening"));
    let mut config = script.config(secret, "/lab/Renoise");
    config.discovery_dir = Some(paths.discovery());
    Simulator::start(config).await.unwrap()
}

fn tool(observed: &Observation, name: &str) -> Rc<dyn KernelTool> {
    observed.tools.iter().find(|tool| tool.name() == name).cloned().unwrap_or_else(|| panic!("no {name} tool"))
}

/// The tools' descriptions and schemas, as the model reads them.
fn described(observed: &Observation) -> Vec<String> {
    observed
        .tools
        .iter()
        .map(|tool| json!({"name": tool.name(), "description": tool.description(), "inputSchema": tool.input_schema()}).to_string())
        .collect()
}

#[tokio::test(flavor = "current_thread")]
async fn reaper_renders_a_part_to_hear_it_and_plays_what_it_heard_out_loud() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let home = tempfile::tempdir().unwrap();
            let script = ReaperScript::new(3, 0);
            let simulator = reaper_lab(&script, home.path(), |_| {}).await;
            // (the warm candidate sits low in the mix)
            script.set_volume(2, -15.0);
            let (reaper, told) = integration(Daw::Reaper, home.path(), create_reaper_integration);
            reaper.start(Signal::new()).await.unwrap();
            let observed = reaper.observe(Signal::new(), None).await.unwrap();
            let names: Vec<&str> = observed.tools.iter().map(|tool| tool.name()).collect();
            for listening in ["audition", "judge", "tune", "sound", "form", "render"] {
                assert!(names.contains(&listening), "{listening} offered: {names:?}");
            }
            assert!(reaper.has_hear() && reaper.has_audition() && reaper.has_audio_file());
            let context: Value = serde_json::from_str(&observed.context).unwrap();
            let track = |at: usize| context["tracks"][at]["ref"].as_str().unwrap().to_owned();
            let mut said = described(&observed);

            // An audition: both candidates rendered offline in one job, before their faders, the first played out loud.
            let since = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() as i64;
            assert_eq!(reaper.first_heard(since), None, "nothing played yet");
            let audition = tool(&observed, "audition")
                .execute(
                    object(json!({"candidates": [{"track": track(2), "label": "warm"}, {"track": track(3), "label": "bright"}], "from_beat": 16, "beats": 8})),
                    Signal::new(),
                )
                .await
                .unwrap();
            assert!(!audition.is_error, "{}", audition.text);
            let reply: Value = serde_json::from_str(&audition.text).unwrap();
            assert_eq!(reply["takes"].as_array().unwrap().len(), 2, "{reply}");
            assert!(reply["takes"].as_array().unwrap().iter().all(|take| take["heard"].is_string()), "{reply}");
            assert!(!audition.text.contains("didn't hear this round"), "the first candidate was played out loud: {}", audition.text);
            let asked = script.listening().renders();
            assert_eq!(asked[0]["range"], json!({"fromBeat": 16.0, "beats": 8.0}));
            assert_eq!(asked[0]["preRollBeats"], 4.0, "a bar before the part, so effects are warm");
            let stages: Vec<&Value> = asked[0]["sources"].as_array().unwrap().iter().map(|source| &source["stage"]).collect();
            assert_eq!(stages, [&json!("pre-fader"), &json!("pre-fader")]);
            let played = script.listening().previews();
            assert_eq!(played.len(), 1, "the first candidate, out loud");
            assert_eq!(played[0]["fromSeconds"], 2.0, "from where the part starts, past the pre-roll");
            // (#361 review K16) Rendered before its fader, played at its fader's level: as loud as in the mix, as Kumi
            // measured it.
            assert_eq!(played[0]["gainDb"], -15.0, "{}", played[0]);
            assert!(reaper.first_heard(since).is_some_and(|at| at >= since), "the producer heard Kumi's listen");
            said.push(audition.text);
            // A round REAPER couldn't play out loud is said to be rendered, not heard.
            script.listening().set_audition(false);
            let unheard = tool(&observed, "audition")
                .execute(object(json!({"candidates": [{"track": track(2)}, {"track": track(3)}], "from_beat": 16, "beats": 8})), Signal::new())
                .await
                .unwrap();
            assert!(unheard.text.contains("The producer didn't hear this round: REAPER rendered it without playing it out loud."), "{}", unheard.text);
            script.listening().set_audition(true);
            said.push(unheard.text);

            // A judged run's first listen: the mix, rendered and played out loud.
            let judge = tool(&observed, "judge")
                .execute(object(json!({"goal": {"loudness": -14}, "from_beat": 0, "beats": 8})), Signal::new())
                .await
                .unwrap();
            assert!(!judge.is_error, "{}", judge.text);
            let mix = script.listening().renders().last().cloned().unwrap();
            assert!(mix["sources"].as_array().unwrap().iter().any(|source| source["what"] == "master"), "{mix}");
            said.push(judge.text);

            // The listen tool's hearing of the project: rendered, played out loud, said so.
            let heard = reaper.hear(&HearRequest { mix: Some(true), from_beat: Some(0.), beats: Some(8.), ..Default::default() }, Signal::new()).await.unwrap().unwrap();
            assert!(!heard[0].live && Path::new(&heard[0].file).is_file(), "{heard:?}");
            assert!(heard[0].note.as_deref().is_some_and(|note| note.contains("rendered it offline")), "{heard:?}");

            // While REAPER plays: what it plays, heard live (the master output); tracks on their own are rendered instead.
            script.set_playing(true);
            let live = reaper.hear(&HearRequest { mix: Some(true), seconds: Some(4.), ..Default::default() }, Signal::new()).await.unwrap().unwrap();
            assert!(live[0].live, "{live:?}");
            let tracks = reaper.hear(&HearRequest { tracks: vec![track(2)], ..Default::default() }, Signal::new()).await.unwrap();
            assert!(tracks.as_ref().is_err_and(|why| why.contains("give from_beat and beats")), "{tracks:?}");
            script.set_playing(false);
            // A track heard on its own is said by its name, not its ref.
            let one = reaper.hear(&HearRequest { tracks: vec![track(2)], from_beat: Some(0.), beats: Some(4.), ..Default::default() }, Signal::new()).await;
            assert!(one.as_ref().is_ok_and(|heard| heard.is_ok()), "{one:?}");
            let name = context["tracks"][2]["name"].as_str().unwrap();
            assert!(told.borrow().iter().any(|title| title.ends_with(&format!("· listening to {name}"))), "{:?}", told.borrow());

            // The render tool: a track's own items before its FX.
            let render = tool(&observed, "render").execute(object(json!({"track": track(1), "from_beat": 0, "beats": 4})), Signal::new()).await.unwrap();
            assert!(!render.is_error, "{}", render.text);
            assert!(render.text.contains("The track's own items, before its devices"), "{}", render.text);
            assert_eq!(script.listening().renders().last().unwrap()["sources"][0]["stage"], "items");
            said.push(render.text);

            let told = told.borrow().clone();
            assert!(told.iter().any(|title| title.starts_with("Rendering bars")), "{told:?}");
            assert!(told.iter().any(|title| title.starts_with("Playing bars") && title.ends_with("out loud")), "{told:?}");
            assert!(!told.iter().any(|title| title.starts_with("Playing bars") && title.contains(" · ")), "no part is said to play while it renders: {told:?}");
            said.extend(told);
            said.extend(heard.iter().chain(live.iter()).filter_map(|take| take.note.clone()));
            assert_ours(&said);
            reaper.close().await.unwrap();
            simulator.stop();
        })
        .await;
}

/// (#361 review K22) Esc stops what Kumi plays out loud, as it stops the answer: the listening call's signal, cancelled
/// after the call answered while the play goes on. A play nobody stopped plays on.
#[tokio::test(flavor = "current_thread")]
async fn esc_stops_kumis_play_out_loud_and_a_play_nobody_stopped_plays_on() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let home = tempfile::tempdir().unwrap();
            let script = ReaperScript::new(3, 0);
            let simulator = reaper_lab(&script, home.path(), |_| {}).await;
            let (reaper, _) = integration(Daw::Reaper, home.path(), create_reaper_integration);
            reaper.start(Signal::new()).await.unwrap();
            let observed = reaper.observe(Signal::new(), None).await.unwrap();
            let context: Value = serde_json::from_str(&observed.context).unwrap();
            let track = context["tracks"][2]["ref"].as_str().unwrap().to_owned();
            let audition = |signal: Signal| {
                let tool = tool(&observed, "audition");
                let input = object(json!({"candidates": [{"track": track}], "from_beat": 16, "beats": 8}));
                async move { tool.execute(input, signal).await.unwrap() }
            };
            let answer = Signal::new();
            assert!(!audition(answer.clone()).await.is_error);
            assert!(script.listening().playing().is_some(), "Kumi plays what it heard");
            answer.cancel();
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            assert_eq!(script.listening().playing(), None, "Esc stopped it");
            let answer = Signal::new();
            assert!(!audition(answer.clone()).await.is_error);
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            assert!(script.listening().playing().is_some(), "plays on");
            reaper.close().await.unwrap();
            simulator.stop();
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn a_listen_after_kumis_last_one_ended_by_itself_says_nothing_was_stopped() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let home = tempfile::tempdir().unwrap();
            let script = ReaperScript::new(3, 0);
            let simulator = reaper_lab(&script, home.path(), |_| {}).await;
            let (reaper, told) = integration(Daw::Reaper, home.path(), create_reaper_integration);
            reaper.start(Signal::new()).await.unwrap();
            reaper.observe(Signal::new(), None).await.unwrap();
            let half_a_second = HearRequest { mix: Some(true), from_beat: Some(0.), beats: Some(1.), ..Default::default() };
            reaper.hear(&half_a_second, Signal::new()).await.unwrap().unwrap();
            // Its half second played out (and Kumi's second of slack): the next render stops nothing.
            tokio::time::sleep(std::time::Duration::from_millis(1700)).await;
            let heard = reaper.hear(&half_a_second, Signal::new()).await.unwrap().unwrap();
            let stopped = "The audition playing was stopped";
            assert!(!told.borrow().iter().any(|title| title.starts_with(stopped)), "{:?}", told.borrow());
            assert!(!heard[0].note.clone().unwrap_or_default().contains(stopped), "{heard:?}");
            reaper.close().await.unwrap();
            simulator.stop();
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn a_listen_stopped_part_way_stops_its_render_and_reaper_is_put_back() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let home = tempfile::tempdir().unwrap();
            let script = ReaperScript::new(3, 0);
            script.listening().set_steps(1_000);
            let simulator = reaper_lab(&script, home.path(), |_| {}).await;
            let (reaper, _) = integration(Daw::Reaper, home.path(), create_reaper_integration);
            reaper.start(Signal::new()).await.unwrap();
            reaper.observe(Signal::new(), None).await.unwrap();
            let signal = Signal::new();
            let stop = signal.clone();
            tokio::task::spawn_local(async move {
                tokio::time::sleep(std::time::Duration::from_millis(400)).await;
                stop.cancel();
            });
            let heard =
                reaper.hear(&HearRequest { mix: Some(true), from_beat: Some(0.), beats: Some(8.), ..Default::default() }, signal).await;
            assert!(matches!(heard, Err(RuntimeError::Aborted)), "{heard:?}");
            assert_eq!(script.listening().restored(), 1, "the render was stopped and what it changed put back");
            // A render that can't stop at once (it says it's stopping) is waited for, so the next listen finds the DAW free.
            script.listening().set_slow_stop(true);
            let signal = Signal::new();
            let stop = signal.clone();
            tokio::task::spawn_local(async move {
                tokio::time::sleep(std::time::Duration::from_millis(400)).await;
                stop.cancel();
            });
            let heard =
                reaper.hear(&HearRequest { mix: Some(true), from_beat: Some(0.), beats: Some(8.), ..Default::default() }, signal).await;
            assert!(matches!(heard, Err(RuntimeError::Aborted)), "{heard:?}");
            assert_eq!(script.listening().restored(), 2, "the stop waited for the render's end");
            script.listening().set_steps(1);
            let next = reaper
                .hear(&HearRequest { mix: Some(true), from_beat: Some(0.), beats: Some(8.), ..Default::default() }, Signal::new())
                .await
                .unwrap();
            assert!(next.is_ok(), "the next listen isn't refused as busy: {next:?}");
            reaper.close().await.unwrap();
            simulator.stop();
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn renoise_renders_each_track_through_its_master_and_offers_no_render_of_a_tracks_own_audio() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let home = tempfile::tempdir().unwrap();
            let script = RenoiseScript::new();
            let simulator = renoise_lab(&script, home.path()).await;
            let (renoise, _) = integration(Daw::Renoise, home.path(), create_renoise_integration);
            renoise.start(Signal::new()).await.unwrap();
            let observed = renoise.observe(Signal::new(), None).await.unwrap();
            let names: Vec<&str> = observed.tools.iter().map(|tool| tool.name()).collect();
            assert!(names.contains(&"audition") && names.contains(&"judge") && !names.contains(&"render"), "{names:?}");
            let context: Value = serde_json::from_str(&observed.context).unwrap();
            let first = context["tracks"][0]["ref"].as_str().unwrap().to_owned();
            let audition = tool(&observed, "audition")
                .execute(object(json!({"candidates": [{"track": first, "label": "kick"}], "from_beat": 0, "beats": 4})), Signal::new())
                .await
                .unwrap();
            assert!(!audition.is_error, "{}", audition.text);
            assert_eq!(script.listening().previews().len(), 1, "played out loud");
            let mut said = described(&observed);
            said.push(audition.text);
            assert_ours(&said);
            renoise.close().await.unwrap();
            simulator.stop();
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn the_renoise_demo_and_an_older_bridge_offer_no_listening_and_say_why() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let home = tempfile::tempdir().unwrap();
            let script = RenoiseScript::new();
            script.listening().set_demo(true);
            let simulator = renoise_lab(&script, home.path()).await;
            let (renoise, _) = integration(Daw::Renoise, home.path(), create_renoise_integration);
            renoise.start(Signal::new()).await.unwrap();
            let observed = renoise.observe(Signal::new(), None).await.unwrap();
            assert!(!observed.tools.iter().any(|tool| tool.name() == "audition"));
            assert!(!renoise.has_hear());
            let refused =
                renoise.hear(&HearRequest { mix: Some(true), from_beat: Some(0.), ..Default::default() }, Signal::new()).await.unwrap();
            assert!(refused.is_err_and(|why| why.starts_with("The Renoise demo can't render")));
            renoise.close().await.unwrap();
            simulator.stop();

            let home = tempfile::tempdir().unwrap();
            let script = ReaperScript::new(2, 0);
            // (a bridge from before listening: its status says nothing of it)
            let simulator = reaper_lab(&script, home.path(), |config| config.status_more = None).await;
            let (reaper, _) = integration(Daw::Reaper, home.path(), create_reaper_integration);
            reaper.start(Signal::new()).await.unwrap();
            let observed = reaper.observe(Signal::new(), None).await.unwrap();
            assert!(!observed.tools.iter().any(|tool| tool.name() == "audition"));
            let refused =
                reaper.hear(&HearRequest { mix: Some(true), from_beat: Some(0.), ..Default::default() }, Signal::new()).await.unwrap();
            assert!(refused.is_err_and(|why| why.contains("update Kumi's bridge")));
            reaper.close().await.unwrap();
            simulator.stop();
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn a_judged_round_brings_reapers_level_back_with_a_volume_fx_it_adds_on_the_master() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let home = tempfile::tempdir().unwrap();
            let script = ReaperScript::new(3, 0);
            let simulator = reaper_lab(&script, home.path(), |_| {}).await;
            let (reaper, told) = integration(Daw::Reaper, home.path(), create_reaper_integration);
            reaper.start(Signal::new()).await.unwrap();
            let observed = reaper.observe(Signal::new(), None).await.unwrap();
            let judge = tool(&observed, "judge");
            let start = judge
                .execute(object(json!({"goal": {"loudness": -6, "problems": false}, "from_beat": 0, "beats": 8})), Signal::new())
                .await
                .unwrap();
            assert!(!start.is_error, "{}", start.text);
            // The model changes something that makes the mix louder, toward the goal: kept, and the rest of the way is the
            // gain stage's (a gain FX added last on the master, and turned up).
            script.listening().set_master_level(0.4);
            let context: Value = serde_json::from_str(&observed.context).unwrap();
            let bass = context["tracks"][2]["ref"].as_str().unwrap().to_owned();
            let change =
                tool(&observed, "track_volume").execute(object(json!({"ref": bass, "volumeDb": -3})), Signal::new()).await.unwrap();
            assert!(!change.is_error, "{}", change.text);
            let round = judge.execute(object(json!({"change": "Bass down 3 dB"})), Signal::new()).await.unwrap();
            assert!(!round.is_error, "{}", round.text);
            assert_eq!(script.fx_names("r:track:master"), ["JS: Volume Adjustment"], "the gain FX rebalancing added: {}", round.text);
            let adjusted = script.fx_param("r:track:master", "Volume Adjustment", "Adjustment (dB)").unwrap();
            assert!(adjusted > 0.5, "turned up toward −6 LUFS: {adjusted}");
            // REAPER adds the gain FX at +6 dB: what it is set to is the turn the round says, not +6 dB more.
            let said = Regex::new(r"Volume Adjustment gain \+?(-?[0-9.]+) dB").unwrap();
            let turned: f64 = said.captures(&round.text).and_then(|found| found[1].parse().ok()).unwrap_or(f64::NAN);
            assert!(
                (-150. + 300. * adjusted - turned).abs() < 0.06,
                "set to {} dB for a {turned} dB turn: {}",
                -150. + 300. * adjusted,
                round.text
            );
            assert!(round.text.contains("\"kept\":true"), "{}", round.text);
            // Each listen of the run stopped the last one playing out loud: said once.
            let stopped = told.borrow().iter().filter(|title| title.starts_with("The audition playing was stopped")).count();
            assert_eq!(stopped, 1, "{:?}", told.borrow());
            assert!(!round.text.contains("The audition playing was stopped"), "said once, at the run's first: {}", round.text);
            assert_ours(&[round.text]);
            reaper.close().await.unwrap();
            simulator.stop();
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn a_goal_searches_scratch_copies_of_its_candidates_and_settles_the_winner_on_the_track() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let home = tempfile::tempdir().unwrap();
            let script = ReaperScript::new(3, 0);
            let eq = script.add_fx("r:track:{T0002}", "ReaEQ");
            let simulator = reaper_lab(&script, home.path(), |_| {}).await;
            let (reaper, told) = integration(Daw::Reaper, home.path(), create_reaper_integration);
            reaper.start(Signal::new()).await.unwrap();
            reaper.observe(Signal::new(), None).await.unwrap();
            assert!(reaper.has_goal());
            let reference = home.path().join("reference.wav");
            daw_mcp_server::simulator::listening::write_noise(&reference, 4., 0.4, 9).unwrap();
            let request = AuditionRequest {
                candidates: vec![AuditionCandidate { track: "Track 2".into(), clip: None, label: Some("pad".into()), mix: None }],
                from_beat: Some(0.),
                beats: Some(4.),
                reference: Some(reference.to_string_lossy().into_owned()),
                reference_from: None,
                reference_seconds: None,
                focus: None,
            };
            let rig = reaper.goal(&request, Signal::new()).await.unwrap().unwrap();
            assert!(!rig.screens(), "two seconds: heard whole");
            let slots = rig.slots();
            assert_eq!(slots.len(), 1);
            assert_eq!(script.fx_names("r:track:{T0002}"), ["VST: ReaEQ (Cockos)", "VST: ReaLimit (Cockos)"], "a safety limiter last");
            assert!(slots[0].chain.contains("ReaEQ") && slots[0].knobs.iter().any(|knob| knob.name == "Gain-Band 2"), "{:?}", slots[0]);
            // Never the safety limiter Kumi put last, nor REAPER's Bypass, Wet and Delta: they aren't the sound.
            assert!(slots[0].knobs.iter().all(|knob| !knob.device.contains("ReaLimit")), "{:?}", slots[0].knobs);
            assert!(slots[0].knobs.iter().all(|knob| !["Bypass", "Wet", "Delta"].contains(&knob.name.as_str())), "{:?}", slots[0].knobs);
            // A generation: the trial's values on the slot's copy, its copy rendered and scored.
            let knobs: Vec<_> = slots[0].knobs.iter().filter(|knob| knob.name == "Gain-Band 2").cloned().collect();
            let trial = GenerationTrial { slot: slots[0].name.clone(), knobs: knobs.clone(), values: vec![0.7], fresh: None };
            let generation = rig.generation(&[trial.clone()], Signal::new(), None).await.unwrap();
            assert!(generation.scores.contains_key(&slots[0].name), "{generation:?}");
            let rendered = script.listening().renders().last().cloned().unwrap();
            let copy = rendered["sources"][0]["ref"].as_str().unwrap().to_owned();
            assert!(copy.starts_with("r:track:{C"), "the copy is heard, not the track: {rendered}");
            assert_eq!(script.listening().copy_values(&copy)[0]["normalized"], 0.7);
            assert_eq!(
                script.fx_param("r:track:{T0002}", "ReaEQ", "Gain-Band 2"),
                Some(0.5),
                "the track itself is untouched while searching"
            );
            // The same values again come from what was heard.
            let again = rig.generation(&[trial], Signal::new(), None).await.unwrap();
            assert_eq!(again.cached, 1);
            // Keeping the best: a fresh copy at these values kept as a track of the project (scratch-keep); keeping again
            // takes the last one away first (Kumi's undo of keeping it).
            assert_eq!(rig.keep_best(&slots[0].name, &knobs, &[0.7], Signal::new()).await.unwrap(), "Kumi · Goal best");
            let first = script.listening().kept();
            assert_eq!(first.len(), 1, "{first:?}");
            assert_eq!(first[0].1, "Kumi · Goal best");
            assert_eq!(rig.keep_best(&slots[0].name, &knobs, &[0.7], Signal::new()).await.unwrap(), "Kumi · Goal best");
            let second = script.listening().kept();
            assert_eq!(second.len(), 1, "the last best went: {second:?}");
            assert_ne!(second[0].0, first[0].0, "a fresh copy is kept");
            // One the producer worked on since stays theirs: the new best goes on a track of its own, and that's said.
            script.listening().work_on_kept(&second[0].0, "My best");
            assert_eq!(rig.keep_best(&slots[0].name, &knobs, &[0.7], Signal::new()).await.unwrap(), "Kumi · Goal best");
            let third = script.listening().kept();
            assert_eq!(third.len(), 2, "the producer's stays: {third:?}");
            assert!(third.iter().any(|(reference, name)| *reference == second[0].0 && name == "My best"), "{third:?}");
            assert!(
                told.borrow().iter().any(|title| title.starts_with("The last “Kumi · Goal best” stays where it is (")),
                "{:?}",
                told.borrow()
            );
            // The producer put an FX first on the track meanwhile: the winner still lands on the ReaEQ, found by its id, as
            // one change (REAPER's fx-params).
            script.insert_fx_first("r:track:{T0002}", "Volume Adjustment");
            assert_eq!(rig.settle(&slots[0].name, &knobs, &[0.7], Signal::new()).await.unwrap(), None);
            assert_eq!(script.fx_param("r:track:{T0002}", "ReaEQ", "Gain-Band 2"), Some(0.7));
            assert_eq!(
                script.fx_param("r:track:{T0002}", "Volume Adjustment", "Adjustment (dB)"),
                Some(0.52),
                "the FX now first is untouched"
            );
            let applied: Vec<Value> = script
                .asked()
                .into_iter()
                .filter(|(method, params)| method == "change_apply" && params["input"]["values"][0]["normalized"] == 0.7)
                .map(|(_, params)| params)
                .collect();
            assert_eq!(applied.len(), 1, "{applied:?}");
            assert_eq!(applied[0]["kind"], "fx-params");
            // The ReaEQ gone: nothing is set or kept, and why is said.
            script.remove_fx(&eq);
            let refused = rig.settle(&slots[0].name, &knobs, &[0.4], Signal::new()).await.unwrap();
            assert!(refused.as_deref().is_some_and(|why| why.contains("moved or went")), "{refused:?}");
            let kept = rig.keep_best(&slots[0].name, &knobs, &[0.4], Signal::new()).await.unwrap();
            assert!(kept.contains("moved or went"), "{kept}");
            rig.close().await.unwrap();
            assert_eq!(script.listening().scratch_copies(), 0, "the copies are gone");
            assert_eq!(script.listening().kept().len(), 2, "the bests stay tracks of the project's");
            assert_ours(&told.borrow());
            reaper.close().await.unwrap();
            simulator.stop();
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn each_candidate_is_heard_over_its_own_part_in_an_audition_and_in_a_goal() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let home = tempfile::tempdir().unwrap();
            let script = ReaperScript::new(3, 0);
            // Two parts far apart: bar 17 on Track 2, bar 49 on Track 3 (two bars each).
            let (a, _) = script.add_midi_item("r:track:{T0002}", "Verse", 64., 8., &eighths(0., 100, 100));
            let (b, _) = script.add_midi_item("r:track:{T0003}", "Chorus", 192., 8., &eighths(0., 100, 100));
            let simulator = reaper_lab(&script, home.path(), |_| {}).await;
            let (reaper, told) = integration(Daw::Reaper, home.path(), create_reaper_integration);
            reaper.start(Signal::new()).await.unwrap();
            reaper.observe(Signal::new(), None).await.unwrap();
            let candidate = |track: &str, part: &str| AuditionCandidate {
                track: track.into(),
                clip: Some(part.into()),
                label: Some(track.to_lowercase()),
                mix: None,
            };
            let reference = home.path().join("reference.wav");
            daw_mcp_server::simulator::listening::write_noise(&reference, 4., 0.4, 9).unwrap();
            let request = |candidates: Vec<AuditionCandidate>| AuditionRequest {
                candidates,
                from_beat: None,
                beats: None,
                reference: Some(reference.to_string_lossy().into_owned()),
                reference_from: None,
                reference_seconds: None,
                focus: None,
            };
            let ranges =
                |from: usize| -> Vec<Value> { script.listening().renders()[from..].iter().map(|render| render["range"].clone()).collect() };

            // An audition: each candidate rendered over its own part, not over the first one's.
            let before = script.listening().renders().len();
            let heard = reaper.audition(&request(vec![candidate("Track 2", &a), candidate("Track 3", &b)]), Signal::new()).await.unwrap();
            assert!(heard.is_ok(), "{heard:?}");
            assert_eq!(ranges(before), [json!({"fromBeat": 64.0, "beats": 8.0}), json!({"fromBeat": 192.0, "beats": 8.0})]);
            assert_eq!(script.listening().previews().len(), 1, "one candidate played out loud for the round");
            // Parts of different lengths are refused in words, nothing rendered.
            let (long, _) = script.add_midi_item("r:track:{T0002}", "Bridge", 256., 12., &eighths(0., 100, 100));
            let before = script.listening().renders().len();
            let refused =
                reaper.audition(&request(vec![candidate("Track 2", &long), candidate("Track 3", &b)]), Signal::new()).await.unwrap();
            let said = refused.map(|result| result.notes.join(" ")).unwrap_or_else(|why| why);
            assert!(said.contains("different lengths (“track 2” 12 beats, “track 3” 8 beats): give beats"), "{said}");
            assert_eq!(script.listening().renders().len(), before);
            // A part longer than 32 beats is heard for its first 32.
            let (longest, _) = script.add_midi_item("r:track:{T0003}", "Outro", 320., 40., &eighths(0., 100, 100));
            let before = script.listening().renders().len();
            let capped = reaper.audition(&request(vec![candidate("Track 3", &longest)]), Signal::new()).await.unwrap();
            assert!(capped.is_ok(), "{capped:?}");
            assert_eq!(ranges(before), [json!({"fromBeat": 320.0, "beats": 32.0})]);

            // A goal from the same candidates: each slot's copy made and heard over its own part.
            let made = |since: usize| -> Vec<Value> {
                script.asked()[since..]
                    .iter()
                    .filter(|(method, params)| method == "scratch" && params["action"] == "make" && params["label"] == "Kumi: goal search")
                    .map(|(_, params)| params["range"].clone())
                    .collect()
            };
            let asked = script.asked().len();
            let rig =
                reaper.goal(&request(vec![candidate("Track 2", &a), candidate("Track 3", &b)]), Signal::new()).await.unwrap().unwrap();
            assert_eq!(made(asked), [json!({"fromBeat": 64.0, "beats": 8.0}), json!({"fromBeat": 192.0, "beats": 8.0})]);
            let slots = rig.slots();
            let trials: Vec<GenerationTrial> = slots
                .iter()
                .map(|slot| GenerationTrial { slot: slot.name.clone(), knobs: vec![], values: vec![], fresh: Some(true) })
                .collect();
            let before = script.listening().renders().len();
            rig.generation(&trials, Signal::new(), None).await.unwrap();
            assert_eq!(ranges(before), [json!({"fromBeat": 64.0, "beats": 8.0}), json!({"fromBeat": 192.0, "beats": 8.0})]);
            rig.close().await.unwrap();
            // A goal heard again names each track's first part: where that track first plays.
            let asked = script.asked().len();
            let again = reaper
                .goal(&request(vec![candidate("Track 2", "first"), candidate("Track 3", "first")]), Signal::new())
                .await
                .unwrap()
                .unwrap();
            assert_eq!(made(asked), [json!({"fromBeat": 64.0, "beats": 8.0}), json!({"fromBeat": 192.0, "beats": 8.0})]);
            again.close().await.unwrap();
            assert_ours(&told.borrow());
            reaper.close().await.unwrap();
            simulator.stop();
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn a_renoise_goal_settles_its_winner_as_one_device_params_change_that_kumis_undo_takes_back() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let home = tempfile::tempdir().unwrap();
            let script = RenoiseScript::new();
            let eq = script.add_device(4, "EQ 10");
            let simulator = renoise_lab(&script, home.path()).await;
            let (renoise, told) = integration(Daw::Renoise, home.path(), create_renoise_integration);
            renoise.start(Signal::new()).await.unwrap();
            renoise.observe(Signal::new(), None).await.unwrap();
            let reference = home.path().join("reference.wav");
            daw_mcp_server::simulator::listening::write_noise(&reference, 4., 0.4, 9).unwrap();
            let request = AuditionRequest {
                candidates: vec![AuditionCandidate { track: "Bass".into(), clip: None, label: Some("bass".into()), mix: None }],
                from_beat: Some(0.),
                beats: Some(4.),
                reference: Some(reference.to_string_lossy().into_owned()),
                reference_from: None,
                reference_seconds: None,
                focus: None,
            };
            let rig = renoise.goal(&request, Signal::new()).await.unwrap().unwrap();
            let slots = rig.slots();
            assert_eq!(slots[0].chain, "TrackVolPan → EQ 10 → Maximizer", "a safety Maximizer last");
            let knobs: Vec<_> = slots[0].knobs.iter().filter(|knob| knob.name.starts_with("Gain 2 ")).cloned().collect();
            assert_eq!(knobs.len(), 1, "{:?}", slots[0].knobs);
            let trial = GenerationTrial { slot: slots[0].name.clone(), knobs: knobs.clone(), values: vec![6.], fresh: None };
            let generation = rig.generation(&[trial], Signal::new(), None).await.unwrap();
            assert!(generation.scores.contains_key(&slots[0].name), "{generation:?}");
            assert_eq!(rig.keep_best(&slots[0].name, &knobs, &[6.], Signal::new()).await.unwrap(), "Kumi · Goal best");
            assert_eq!(script.device_value(4, eq, "Gain 2 (63 Hz)"), Some(0.), "the track itself is untouched while searching");
            // The copies moved Renoise's epoch since the knobs were read: the winner settles all the same, in one change.
            assert_eq!(rig.settle(&slots[0].name, &knobs, &[6.], Signal::new()).await.unwrap(), None);
            assert_eq!(script.device_value(4, eq, "Gain 2 (63 Hz)"), Some(6.));
            let applied: Vec<Value> = script
                .asked()
                .into_iter()
                .filter(|(method, params)| {
                    method == "change_apply" && params["kind"].as_str().is_some_and(|kind| kind.starts_with("device-param"))
                })
                .map(|(_, params)| params["input"].clone())
                .collect();
            assert_eq!(applied.len(), 2, "the settle and the safety Maximizer's Boost at 0 dB: {applied:?}");
            assert_eq!(applied[0]["values"], json!([{"param": 2, "value": 6.0}]), "{applied:?}");
            // Kumi's undo takes the settle back as a new step.
            let observed = renoise.observe(Signal::new(), None).await.unwrap();
            let undo = tool(&observed, "undo_change").execute(object(json!({"change": "last"})), Signal::new()).await.unwrap();
            assert!(!undo.is_error && undo.text.contains("EQ 10 1 parameter"), "{}", undo.text);
            assert_eq!(script.device_value(4, eq, "Gain 2 (63 Hz)"), Some(0.));
            // The producer puts a device before the EQ: Renoise's refs are places, so the knobs' device isn't where it was
            // and nothing is set.
            script.insert_device(4, 2, "Gainer");
            let refused = rig.settle(&slots[0].name, &knobs, &[-3.], Signal::new()).await.unwrap();
            assert!(refused.as_deref().is_some_and(|why| why.contains("moved or went")), "{refused:?}");
            assert_eq!(script.device_value(4, eq + 1, "Gain 2 (63 Hz)"), Some(0.), "the EQ, now one further, is untouched");
            rig.close().await.unwrap();
            assert_ours(&told.borrow());
            renoise.close().await.unwrap();
            simulator.stop();
        })
        .await;
}

/// (#361 review K27) A Renoise goal that fails to open takes its safety Maximizers off again, each found on its track's
/// chain as it is now: the second one's add moved the device refs the first was added under.
#[tokio::test(flavor = "current_thread")]
async fn a_renoise_goal_that_fails_to_open_takes_its_safety_maximizers_off_again() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let home = tempfile::tempdir().unwrap();
            let script = RenoiseScript::new();
            script.add_device(4, "EQ 10");
            script.add_device(3, "EQ 10");
            script.listening().refuse_copies_of("track:3", "Renoise can't copy a group track for Kumi");
            let simulator = renoise_lab(&script, home.path()).await;
            let (renoise, told) = integration(Daw::Renoise, home.path(), create_renoise_integration);
            renoise.start(Signal::new()).await.unwrap();
            renoise.observe(Signal::new(), None).await.unwrap();
            let reference = home.path().join("reference.wav");
            daw_mcp_server::simulator::listening::write_noise(&reference, 4., 0.4, 9).unwrap();
            let candidate = |track: &str| AuditionCandidate { track: track.into(), clip: None, label: None, mix: None };
            let request = AuditionRequest {
                candidates: vec![candidate("Bass"), candidate("Drums")],
                from_beat: Some(0.),
                beats: Some(4.),
                reference: Some(reference.to_string_lossy().into_owned()),
                reference_from: None,
                reference_seconds: None,
                focus: None,
            };
            let refused = renoise.goal(&request, Signal::new()).await.unwrap().err().expect("the group can't be copied");
            assert!(refused.contains("group"), "{refused}");
            for track in [4, 3] {
                assert_eq!(
                    script.device_paths(track),
                    ["Audio/Effects/Native/TrackVolPan", "Audio/Effects/Native/EQ 10"],
                    "track {track}'s safety Maximizer is gone: {:?}",
                    told.borrow()
                );
            }
            renoise.close().await.unwrap();
            simulator.stop();
        })
        .await;
}

/// (#361 review K17) A song with tempo commands: each render plays them into Renoise's transport. The producer's tempo
/// is set back once for a whole goal (before its best plays out loud), as one step of Renoise's undo that Kumi says.
#[tokio::test(flavor = "current_thread")]
async fn a_renoise_goal_sets_the_tempo_its_renders_moved_back_once_and_says_it_is_an_undo_step() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let home = tempfile::tempdir().unwrap();
            let script = RenoiseScript::new();
            script.add_device(4, "EQ 10");
            script.set_render_bpm(Some(140.));
            let simulator = renoise_lab(&script, home.path()).await;
            let (renoise, told) = integration(Daw::Renoise, home.path(), create_renoise_integration);
            renoise.start(Signal::new()).await.unwrap();
            renoise.observe(Signal::new(), None).await.unwrap();
            let reference = home.path().join("reference.wav");
            daw_mcp_server::simulator::listening::write_noise(&reference, 4., 0.4, 9).unwrap();
            let request = AuditionRequest {
                candidates: vec![AuditionCandidate { track: "Bass".into(), clip: None, label: Some("bass".into()), mix: None }],
                from_beat: Some(0.),
                beats: Some(4.),
                reference: Some(reference.to_string_lossy().into_owned()),
                reference_from: None,
                reference_seconds: None,
                focus: None,
            };
            let rig = renoise.goal(&request, Signal::new()).await.unwrap().unwrap();
            let slots = rig.slots();
            let knobs: Vec<_> = slots[0].knobs.iter().filter(|knob| knob.name.starts_with("Gain 2 ")).cloned().collect();
            for value in [1., 2., 3.] {
                let trial = GenerationTrial { slot: slots[0].name.clone(), knobs: knobs.clone(), values: vec![value], fresh: None };
                rig.generation(&[trial], Signal::new(), None).await.unwrap();
            }
            let tempo_steps =
                || script.asked().into_iter().filter(|(method, params)| method == "change_apply" && params["kind"] == "song-bpm").count();
            assert_eq!(tempo_steps(), 0, "not while the search renders");
            assert_eq!(rig.keep_best(&slots[0].name, &knobs, &[3.], Signal::new()).await.unwrap(), "Kumi · Goal best");
            assert_eq!(tempo_steps(), 1, "once, before the best plays out loud");
            assert_eq!(script.song()["bpm"], 128.0, "the producer's tempo");
            assert!(rig.close().await.unwrap().is_empty(), "nothing more to set back");
            assert_eq!(tempo_steps(), 1);
            let said = told.borrow().iter().filter(|title| title.contains("tempo back")).cloned().collect::<Vec<_>>();
            assert_eq!(said, ["Kumi set the tempo back to 128 BPM, LPB 4 (1 step in Renoise's undo)."]);
            renoise.close().await.unwrap();
            simulator.stop();
        })
        .await;
}

/// A two-bar 8th-note part: on the beat at `on`, off it at `off`, `late` beats behind the 8th.
fn eighths(late: f64, on: i64, off: i64) -> Vec<(i64, f64, f64, i64)> {
    let mut notes: Vec<(i64, f64, f64, i64)> = (0..16)
        .map(|n| {
            let offbeat = n % 2 == 1;
            (42, n as f64 * 0.5 + if offbeat { late } else { 0. }, 0.25, if offbeat { off } else { on })
        })
        .collect();
    notes.extend((0..4).map(|n| (36, n as f64 * 2., 0.5, 110)));
    notes
}

#[tokio::test(flavor = "current_thread")]
async fn groove_moves_a_reaper_items_notes_toward_a_references_feel_in_one_change_of_its_take() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let home = tempfile::tempdir().unwrap();
            let script = ReaperScript::new(3, 0);
            let (part, take) = script.add_midi_item("r:track:{T0002}", "Drums", 0., 8., &eighths(0., 100, 100));
            let (reference, _) = script.add_midi_item("r:track:{T0003}", "Reference", 0., 8., &eighths(0.08, 115, 70));
            let simulator = reaper_lab(&script, home.path(), |_| {}).await;
            let (reaper, told) = integration(Daw::Reaper, home.path(), create_reaper_integration);
            reaper.start(Signal::new()).await.unwrap();
            let observed = reaper.observe(Signal::new(), None).await.unwrap();
            let groove = tool(&observed, "groove");
            assert!(groove.input_schema()["properties"].get("item").is_some(), "the part is an item");
            let start = groove.execute(object(json!({"item": part, "reference": reference})), Signal::new()).await.unwrap();
            assert!(!start.is_error, "{}", start.text);
            assert!(start.text.contains("“Drums”") && start.text.contains("“Reference”"), "{}", start.text);
            // The model pushes an off-beat early, away from the reference's late ones: the round takes it back with Kumi's
            // undo of the take's notes.
            let before = script.midi_notes(&take);
            let pushed = tool(&observed, "notes_change")
                .execute(object(json!({"ref": take, "changes": [{"index": 2, "start": 0.3}]})), Signal::new())
                .await
                .unwrap();
            assert!(!pushed.is_error, "{}", pushed.text);
            assert_ne!(script.midi_notes(&take), before);
            let judged = groove.execute(object(json!({"change": "pushed the first off-beat"})), Signal::new()).await.unwrap();
            let reply: Value = serde_json::from_str(&judged.text).unwrap();
            assert_eq!(reply["kept"], false, "{reply}");
            assert!(reply["log"].to_string().contains("taken back"), "{reply}");
            assert_eq!(script.midi_notes(&take), before, "put back");
            let applied = groove.execute(object(json!({"apply": true})), Signal::new()).await.unwrap();
            assert!(!applied.is_error, "{}", applied.text);
            let reply: Value = serde_json::from_str(&applied.text).unwrap();
            assert_eq!(reply["kept"], true, "{reply}");
            let notes = script.midi_notes(&take);
            let offbeat = notes.iter().find(|note| note.0 == 42 && note.1 > 0.4 && note.1 < 0.7).copied().unwrap();
            assert!(offbeat.1 > 0.55 && offbeat.3 < 100, "the off-beat moved later and softer, toward the reference: {offbeat:?}");
            assert_eq!(notes.len(), 20, "only moved");
            let done = groove.execute(object(json!({"done": true})), Signal::new()).await.unwrap();
            assert!(!done.is_error, "{}", done.text);
            let mut said = vec![start.text, judged.text, applied.text, done.text];
            said.extend(told.borrow().iter().cloned());
            said.extend(described(&observed).into_iter().filter(|text| text.contains("\"groove\"")));
            assert_ours(&said);
            reaper.close().await.unwrap();
            simulator.stop();
        })
        .await;
}

/// A pattern's 8th notes as Renoise's lines (32 lines at 4 a beat): on-beat cells plain, off-beat ones with `delay` and
/// `volume` when given.
fn pattern_eighths(delay: Option<&str>, volume: Option<&str>) -> Value {
    let rows: Vec<Value> = (0..16)
        .map(|n| {
            let mut cell = json!({"col": 1, "note": "C-4", "instrument": "00"});
            if n % 2 == 1 {
                if let Some(delay) = delay {
                    cell["delay"] = delay.into();
                }
                if let Some(volume) = volume {
                    cell["volume"] = volume.into();
                }
            }
            // (lines by Renoise's numbers, from 00, as the tool's rows give them)
            json!({"line": n * 2, "notes": [cell]})
        })
        .collect();
    Value::Array(rows)
}

#[tokio::test(flavor = "current_thread")]
async fn groove_moves_a_renoise_pattern_tracks_notes_by_their_delays_and_volumes_in_one_write() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let home = tempfile::tempdir().unwrap();
            let script = RenoiseScript::new();
            script.set_lines(1, 1, pattern_eighths(None, None));
            script.set_lines(3, 1, pattern_eighths(Some("50"), Some("48")));
            let simulator = renoise_lab(&script, home.path()).await;
            let (renoise, told) = integration(Daw::Renoise, home.path(), create_renoise_integration);
            renoise.start(Signal::new()).await.unwrap();
            let observed = renoise.observe(Signal::new(), None).await.unwrap();
            let groove = tool(&observed, "groove");
            let start = groove
                .execute(object(json!({"pattern": "1:pattern:1 1:track:1", "reference": "1:pattern:3 1:track:1"})), Signal::new())
                .await
                .unwrap();
            assert!(!start.is_error, "{}", start.text);
            assert!(start.text.contains("“Kick” in pattern 00"), "{}", start.text);
            let applied = groove.execute(object(json!({"apply": true})), Signal::new()).await.unwrap();
            assert!(!applied.is_error, "{}", applied.text);
            let reply: Value = serde_json::from_str(&applied.text).unwrap();
            assert_eq!(reply["kept"], true, "{reply}");
            let lines = script.lines(1, 1);
            let offbeat = lines.as_array().unwrap().iter().find(|row| row["line"] == 2).unwrap()["notes"][0].clone();
            assert!(offbeat["delay"].is_string() && offbeat["volume"].is_string(), "the off-beat delayed and softer: {offbeat}");
            assert_eq!(lines.as_array().unwrap().len(), 16, "the notes stay on their lines");
            // A slot and a track name the part too, and a ref that isn't one says how to name it.
            let wrong = groove.execute(object(json!({"done": true})), Signal::new()).await.unwrap();
            assert!(!wrong.is_error, "{}", wrong.text);
            let again =
                groove.execute(object(json!({"pattern": "1:seq:3 1:track:1", "reference": "1:track:1"})), Signal::new()).await.unwrap();
            assert!(again.is_error && again.text.contains("its pattern's (or a sequence slot's) ref and its track's"), "{}", again.text);
            let mut said = vec![start.text, applied.text, again.text];
            said.extend(told.borrow().iter().cloned());
            assert_ours(&said);
            renoise.close().await.unwrap();
            simulator.stop();
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn reaper_says_what_it_noted_when_a_render_fails_and_a_capture_is_split_where_the_playhead_jumped() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let home = tempfile::tempdir().unwrap();
            let script = ReaperScript::new(3, 0);
            let simulator = reaper_lab(&script, home.path(), |_| {}).await;
            let (reaper, told) = integration(Daw::Reaper, home.path(), create_reaper_integration);
            reaper.start(Signal::new()).await.unwrap();
            reaper.observe(Signal::new(), None).await.unwrap();
            // A render that fails says why, and what the bridge noted on the way.
            script.listening().note_next(&["REAPER stopped playback to render"]);
            script.listening().fail_next("failed", "REAPER couldn't write the file");
            let failed = reaper
                .hear(&HearRequest { mix: Some(true), from_beat: Some(0.), beats: Some(8.), ..Default::default() }, Signal::new())
                .await
                .unwrap();
            let why = failed.unwrap_err();
            assert!(why.contains("couldn't write the file (REAPER stopped playback to render)"), "{why}");
            // While REAPER plays, the loop wrapped two seconds in: the stretches aren't parts heard at once, so the first longest
            // one is measured, and the jump is said.
            script.set_playing(true);
            script.listening().set_capture_marks(json!([[0, 10.0, 20.0, true], [96000, 10.0, 20.0, true]]));
            let heard = reaper
                .hear(&HearRequest { mix: Some(true), seconds: Some(4.), ..Default::default() }, Signal::new())
                .await
                .unwrap()
                .unwrap();
            let takes: Vec<(&str, f64, Option<f64>)> = heard.iter().map(|take| (take.label.as_str(), take.start, take.seconds)).collect();
            assert_eq!(takes, [("The mix from bar 6", 0., Some(2.))]);
            let note = heard[0].note.clone().unwrap_or_default();
            assert!(note.contains("REAPER jumped while Kumi listened (The mix from bar 6, The mix from bar 6 (pass 2))"), "{note}");
            // Copies an earlier session left, swept by the bridge: the producer is told which, once.
            simulator.emit("scratch_swept", json!({"count": 1, "names": ["Kumi · try ab12 1"]}));
            for _ in 0..50 {
                if told.borrow().iter().any(|said| said.contains("an earlier session left")) {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            }
            let swept: Vec<String> = told.borrow().iter().filter(|said| said.contains("an earlier session left")).cloned().collect();
            assert_eq!(swept, ["REAPER removed Kumi's scratch copy an earlier session left in the project: “Kumi · try ab12 1”"]);
            // Copies REAPER's undo brought back after Kumi dropped them, removed again.
            simulator.emit("scratch_swept", json!({"count": 2, "names": ["Kumi · try cd34 1", "Kumi · try cd34 2"], "why": "undo"}));
            for _ in 0..50 {
                if told.borrow().iter().any(|said| said.contains("brought back")) {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            }
            let back: Vec<String> = told.borrow().iter().filter(|said| said.contains("brought back")).cloned().collect();
            assert_eq!(back, ["REAPER's undo brought back Kumi's scratch copies “Kumi · try cd34 1”, “Kumi · try cd34 2”, which Kumi had removed: removed them again"]);
            assert_ours(&told.borrow());
            reaper.close().await.unwrap();
            simulator.stop();
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn an_items_audio_is_only_what_it_plays_of_its_source_the_way_it_plays_it() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let home = tempfile::tempdir().unwrap();
            let script = ReaperScript::new(3, 0);
            let simulator = reaper_lab(&script, home.path(), |_| {}).await;
            let (reaper, _) = integration(Daw::Reaper, home.path(), create_reaper_integration);
            reaper.start(Signal::new()).await.unwrap();
            reaper.observe(Signal::new(), None).await.unwrap();
            // The item plays a second of its 4-second source from half a second in, reversed.
            script.listening().set_item_part(0.5, 1.0, 1.0, true);
            let file = reaper.audio_file("r:item:{I-T0001}", Signal::new()).await.unwrap().unwrap();
            assert!(file.contains("reversed"), "{file}");
            let bytes = std::fs::read(&file).unwrap();
            let data = u32::from_le_bytes(bytes[40..44].try_into().unwrap()) as f64;
            assert!((data / (48_000. * 2. * 4.) - 1.).abs() < 0.01, "one second of 32-bit stereo: {data} bytes");
            reaper.close().await.unwrap();
            simulator.stop();
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn a_renoise_pattern_is_auditioned_over_its_first_slot_in_the_sequence() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let home = tempfile::tempdir().unwrap();
            let script = RenoiseScript::new();
            let simulator = renoise_lab(&script, home.path()).await;
            let (renoise, told) = integration(Daw::Renoise, home.path(), create_renoise_integration);
            renoise.start(Signal::new()).await.unwrap();
            renoise.observe(Signal::new(), None).await.unwrap();
            let request = AuditionRequest {
                candidates: vec![AuditionCandidate {
                    track: "1:track:1".into(),
                    clip: Some("1:pattern:2".into()),
                    label: Some("kick".into()),
                    mix: None,
                }],
                from_beat: None,
                beats: None,
                reference: None,
                reference_from: None,
                reference_seconds: None,
                focus: None,
            };
            let result = renoise.audition(&request, Signal::new()).await.unwrap();
            assert!(result.is_ok(), "{result:?}");
            // Pattern 2 first plays in slot 2, after pattern 1's 32 lines (8 beats at 4 lines a beat).
            assert_eq!(script.listening().renders().last().unwrap()["range"], json!({"fromBeat": 8.0, "beats": 8.0}));
            let played = script.listening().previews();
            assert_eq!(played.len(), 1, "the render, played with the song's own playback");
            assert_ours(&told.borrow());
            renoise.close().await.unwrap();
            simulator.stop();
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn a_limiter_that_cant_turn_down_far_enough_leaves_the_rest_to_a_gain_fx_and_monitoring_fx_are_no_gain_stage() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let home = tempfile::tempdir().unwrap();
            let script = ReaperScript::new(3, 0);
            // ReaLimit on the master at its 0 dB threshold (as far up as it goes), and a monitoring limiter after it.
            script.add_fx("r:track:master", "ReaLimit");
            script.add_input_fx("r:track:master", "ReaLimit");
            let simulator = reaper_lab(&script, home.path(), |_| {}).await;
            let (reaper, _) = integration(Daw::Reaper, home.path(), create_reaper_integration);
            reaper.start(Signal::new()).await.unwrap();
            let observed = reaper.observe(Signal::new(), None).await.unwrap();
            let judge = tool(&observed, "judge");
            let start = judge
                .execute(object(json!({"goal": {"loudness": -6, "problems": false}, "from_beat": 0, "beats": 8})), Signal::new())
                .await
                .unwrap();
            assert!(!start.is_error, "{}", start.text);
            // A change that takes the mix past the goal's loudness, but closer to it than before: kept, and brought back down.
            script.listening().set_master_level(0.5);
            let context: Value = serde_json::from_str(&observed.context).unwrap();
            let bass = context["tracks"][2]["ref"].as_str().unwrap().to_owned();
            let change = tool(&observed, "track_volume").execute(object(json!({"ref": bass, "volumeDb": 3})), Signal::new()).await.unwrap();
            assert!(!change.is_error, "{}", change.text);
            let round = judge.execute(object(json!({"change": "Bass up 3 dB"})), Signal::new()).await.unwrap();
            assert!(!round.is_error, "{}", round.text);
            // The main chain's ReaLimit can't raise its threshold past 0 dB: a Volume Adjustment after it turns down; the
            // monitoring ReaLimit is never touched.
            assert_eq!(
                script.fx_names("r:track:master"),
                ["VST: ReaLimit (Cockos)", "VST: ReaLimit (Cockos)", "JS: Volume Adjustment"],
                "{}",
                round.text
            );
            let adjusted = script.fx_param("r:track:master", "Volume Adjustment", "Adjustment (dB)").unwrap();
            assert!(adjusted < 0.5, "turned down: {adjusted} ({})", round.text);
            reaper.close().await.unwrap();
            simulator.stop();
        })
        .await;
}

/// (#361 review K41) A judged run's render that heard only silence asks what's in the part, not whether REAPER's audio
/// runs (an offline render needs none); the listen tool says a render stops the DAW's playback.
#[tokio::test(flavor = "current_thread")]
async fn an_offline_renders_silence_and_the_listen_tool_are_said_as_rendering_is() {
    use kumi_runtime::core::daw::DawProfile;
    for profile in [DawProfile::REAPER, DawProfile::RENOISE] {
        assert!(profile.vocab.listen_method.contains("a render stops"), "{}", profile.vocab.listen_method);
    }
    tokio::task::LocalSet::new()
        .run_until(async {
            let home = tempfile::tempdir().unwrap();
            let script = ReaperScript::new(3, 0);
            let simulator = reaper_lab(&script, home.path(), |_| {}).await;
            script.listening().set_master_level(0.);
            let (reaper, _) = integration(Daw::Reaper, home.path(), create_reaper_integration);
            reaper.start(Signal::new()).await.unwrap();
            let observed = reaper.observe(Signal::new(), None).await.unwrap();
            let judged = tool(&observed, "judge")
                .execute(object(json!({"goal": {"loudness": -14}, "from_beat": 0, "beats": 8})), Signal::new())
                .await
                .unwrap();
            assert!(judged.text.contains("heard only silence"), "{}", judged.text);
            assert!(!judged.text.contains("audio running"), "{}", judged.text);
            reaper.close().await.unwrap();
            simulator.stop();
        })
        .await;
}

/// (#361 review K28) A paused goal's best waits muted beside the producer's untouched track (the part never plays twice),
/// and says so; a better best still replaces it. A finished goal, its candidates tidied, plays its best.
#[tokio::test(flavor = "current_thread")]
async fn a_paused_goals_best_waits_muted_beside_the_untouched_track() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let home = tempfile::tempdir().unwrap();
            let script = ReaperScript::new(3, 0);
            script.add_fx("r:track:{T0002}", "ReaEQ");
            let simulator = reaper_lab(&script, home.path(), |_| {}).await;
            let (reaper, told) = integration(Daw::Reaper, home.path(), create_reaper_integration);
            reaper.start(Signal::new()).await.unwrap();
            reaper.observe(Signal::new(), None).await.unwrap();
            let reference = home.path().join("reference.wav");
            daw_mcp_server::simulator::listening::write_noise(&reference, 4., 0.4, 9).unwrap();
            let request = AuditionRequest {
                candidates: vec![AuditionCandidate { track: "Track 2".into(), clip: None, label: None, mix: None }],
                from_beat: Some(0.),
                beats: Some(4.),
                reference: Some(reference.to_string_lossy().into_owned()),
                reference_from: None,
                reference_seconds: None,
                focus: None,
            };
            let rig = reaper.goal(&request, Signal::new()).await.unwrap().unwrap();
            let slots = rig.slots();
            let knobs: Vec<_> = slots[0].knobs.iter().filter(|knob| knob.name == "Gain-Band 2").cloned().collect();
            let trial = GenerationTrial { slot: slots[0].name.clone(), knobs: knobs.clone(), values: vec![0.7], fresh: None };
            rig.generation(&[trial], Signal::new(), None).await.unwrap();
            // Paused: no tidy before the best is kept.
            assert_eq!(rig.keep_best(&slots[0].name, &knobs, &[0.7], Signal::new()).await.unwrap(), "Kumi · Goal best");
            let first = script.listening().kept();
            assert!(script.listening().kept_muted(&first[0].0), "the best waits muted");
            assert!(
                told.borrow().iter().any(|title| title.starts_with("“Kumi · Goal best” waits muted beside “Track 2”")),
                "{:?}",
                told.borrow()
            );
            // Finished: the candidates tidied (muted for an A/B), and the best, replacing the last, plays.
            assert!(rig.tidy(&[slots[0].name.clone()], Signal::new()).await.unwrap().is_empty());
            assert_eq!(rig.keep_best(&slots[0].name, &knobs, &[0.7], Signal::new()).await.unwrap(), "Kumi · Goal best");
            let second = script.listening().kept();
            assert_eq!(second.len(), 1, "the last best went, its mute taken back first: {second:?} {:?}", told.borrow());
            assert!(!script.listening().kept_muted(&second[0].0), "a finished goal's best plays");
            rig.close().await.unwrap();
            reaper.close().await.unwrap();
            simulator.stop();
        })
        .await;
}

/// (#361 review K26) A goal's slot is its track by REAPER's GUID, not by a name that moves: a track inserted above an
/// unnamed "Track 2" makes it "Track 3", and the winner still settles on the track the goal searched.
#[tokio::test(flavor = "current_thread")]
async fn a_goal_settles_on_its_own_track_when_reapers_unnamed_track_names_shift() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let home = tempfile::tempdir().unwrap();
            let script = ReaperScript::new(3, 0);
            script.add_fx("r:track:{T0002}", "ReaEQ");
            script.add_fx("r:track:{T0003}", "ReaEQ");
            let simulator = reaper_lab(&script, home.path(), |_| {}).await;
            let (reaper, _) = integration(Daw::Reaper, home.path(), create_reaper_integration);
            reaper.start(Signal::new()).await.unwrap();
            reaper.observe(Signal::new(), None).await.unwrap();
            let reference = home.path().join("reference.wav");
            daw_mcp_server::simulator::listening::write_noise(&reference, 4., 0.4, 9).unwrap();
            let request = AuditionRequest {
                candidates: vec![AuditionCandidate { track: "Track 2".into(), clip: None, label: None, mix: None }],
                from_beat: Some(0.),
                beats: Some(4.),
                reference: Some(reference.to_string_lossy().into_owned()),
                reference_from: None,
                reference_seconds: None,
                focus: None,
            };
            let rig = reaper.goal(&request, Signal::new()).await.unwrap().unwrap();
            let slots = rig.slots();
            let knobs: Vec<_> = slots[0].knobs.iter().filter(|knob| knob.name == "Gain-Band 2").cloned().collect();
            // A track inserted above: REAPER numbers the unnamed tracks again.
            script.set_track(2, "name", json!("Track 3"));
            script.set_track(3, "name", json!("Track 2"));
            tokio::time::sleep(std::time::Duration::from_millis(1100)).await;
            assert_eq!(rig.settle(&slots[0].name, &knobs, &[0.7], Signal::new()).await.unwrap(), None);
            assert_eq!(script.fx_param("r:track:{T0002}", "ReaEQ", "Gain-Band 2"), Some(0.7), "the track the goal searched");
            assert_eq!(script.fx_param("r:track:{T0003}", "ReaEQ", "Gain-Band 2"), Some(0.5), "the one now called Track 2 is untouched");
            rig.close().await.unwrap();
            reaper.close().await.unwrap();
            simulator.stop();
        })
        .await;
}

/// (#361 review K25) A plug-in limiter Kumi doesn't know, last on the master: the gain FX a quieter round adds goes before
/// it, so turning up never goes past its ceiling.
#[tokio::test(flavor = "current_thread")]
async fn a_gain_fx_rebalancing_adds_goes_before_a_plugin_limiter_last_on_the_master() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let home = tempfile::tempdir().unwrap();
            let script = ReaperScript::new(3, 0);
            script.add_plugin_fx("r:track:master", "VST3: Pro-L 2 (FabFilter)");
            let simulator = reaper_lab(&script, home.path(), |_| {}).await;
            let (reaper, _) = integration(Daw::Reaper, home.path(), create_reaper_integration);
            reaper.start(Signal::new()).await.unwrap();
            let observed = reaper.observe(Signal::new(), None).await.unwrap();
            let judge = tool(&observed, "judge");
            let start = judge
                .execute(object(json!({"goal": {"loudness": -6, "problems": false}, "from_beat": 0, "beats": 8})), Signal::new())
                .await
                .unwrap();
            assert!(!start.is_error, "{}", start.text);
            script.listening().set_master_level(0.4);
            let context: Value = serde_json::from_str(&observed.context).unwrap();
            let bass = context["tracks"][2]["ref"].as_str().unwrap().to_owned();
            let change =
                tool(&observed, "track_volume").execute(object(json!({"ref": bass, "volumeDb": -3})), Signal::new()).await.unwrap();
            assert!(!change.is_error, "{}", change.text);
            let round = judge.execute(object(json!({"change": "Bass down 3 dB"})), Signal::new()).await.unwrap();
            assert!(!round.is_error, "{}", round.text);
            assert_eq!(
                script.fx_names("r:track:master"),
                ["JS: Volume Adjustment", "VST3: Pro-L 2 (FabFilter)"],
                "turned up before the limiter: {}",
                round.text
            );
            reaper.close().await.unwrap();
            simulator.stop();
        })
        .await;
}

/// (#361 review K24) An FX is known by what it is, whatever the producer named it: a master ReaLimit renamed "Bus Limit"
/// is still the gain stage a judged round turns (no gain FX added after it), and a ReaLimit renamed "Glue" last on a
/// goal's candidate is its safety limiter (no second one added).
#[tokio::test(flavor = "current_thread")]
async fn a_renamed_fx_is_still_what_it_is_to_a_judged_round_and_a_goal() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let home = tempfile::tempdir().unwrap();
            let script = ReaperScript::new(3, 0);
            let limiter = script.add_fx("r:track:master", "ReaLimit");
            script.rename_fx(&limiter, "Bus Limit");
            script.add_fx("r:track:{T0002}", "ReaEQ");
            let glue = script.add_fx("r:track:{T0002}", "ReaLimit");
            script.rename_fx(&glue, "Glue");
            let simulator = reaper_lab(&script, home.path(), |_| {}).await;
            let (reaper, _) = integration(Daw::Reaper, home.path(), create_reaper_integration);
            reaper.start(Signal::new()).await.unwrap();
            let observed = reaper.observe(Signal::new(), None).await.unwrap();
            let judge = tool(&observed, "judge");
            let start = judge
                .execute(object(json!({"goal": {"loudness": -6, "problems": false}, "from_beat": 0, "beats": 8})), Signal::new())
                .await
                .unwrap();
            assert!(!start.is_error, "{}", start.text);
            script.listening().set_master_level(0.4);
            let context: Value = serde_json::from_str(&observed.context).unwrap();
            let bass = context["tracks"][2]["ref"].as_str().unwrap().to_owned();
            let change =
                tool(&observed, "track_volume").execute(object(json!({"ref": bass, "volumeDb": -3})), Signal::new()).await.unwrap();
            assert!(!change.is_error, "{}", change.text);
            let round = judge.execute(object(json!({"change": "Bass down 3 dB"})), Signal::new()).await.unwrap();
            assert!(!round.is_error, "{}", round.text);
            assert_eq!(script.fx_names("r:track:master"), ["Bus Limit"], "the renamed ReaLimit is the gain stage: {}", round.text);
            let reference = home.path().join("reference.wav");
            daw_mcp_server::simulator::listening::write_noise(&reference, 4., 0.4, 9).unwrap();
            let request = AuditionRequest {
                candidates: vec![AuditionCandidate { track: "Track 2".into(), clip: None, label: Some("pad".into()), mix: None }],
                from_beat: Some(0.),
                beats: Some(4.),
                reference: Some(reference.to_string_lossy().into_owned()),
                reference_from: None,
                reference_seconds: None,
                focus: None,
            };
            let rig = reaper.goal(&request, Signal::new()).await.unwrap().unwrap();
            assert_eq!(script.fx_names("r:track:{T0002}"), ["VST: ReaEQ (Cockos)", "Glue"], "Glue is the safety limiter");
            rig.close().await.unwrap();
            reaper.close().await.unwrap();
            simulator.stop();
        })
        .await;
}

/// (#361 review K23) An FX's track is the one its row names: a tune reads that track's chain, not every track's, however
/// far down the project it is; a take's FX is said to be one, not lost.
#[tokio::test(flavor = "current_thread")]
async fn a_tune_finds_its_fxs_track_from_the_fx_and_says_a_takes_fx_is_one() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let home = tempfile::tempdir().unwrap();
            let script = ReaperScript::new(24, 0);
            let eq = script.add_fx("r:track:{T0024}", "ReaEQ");
            let (_, take) = script.add_midi_item("r:track:{T0003}", "Lead", 0., 8., &eighths(0., 100, 80));
            let take_eq = script.add_fx(&take, "ReaEQ");
            script.listening().set_steps(0);
            let simulator = reaper_lab(&script, home.path(), |_| {}).await;
            let (reaper, _) = integration(Daw::Reaper, home.path(), create_reaper_integration);
            reaper.start(Signal::new()).await.unwrap();
            let observed = reaper.observe(Signal::new(), None).await.unwrap();
            let goal = json!({"goal": {"loudness": -14, "problems": false}, "track": "Track 24", "from_beat": 0, "beats": 2});
            assert!(!tool(&observed, "judge").execute(object(goal), Signal::new()).await.unwrap().is_error);
            let asked_before = script.asked().len();
            let tuned = tool(&observed, "tune")
                .execute(
                    object(json!({"device": eq, "how": "search", "knobs": ["Gain-Band 2", "Gain-Band 3"], "from_beat": 0, "beats": 2})),
                    Signal::new(),
                )
                .await
                .unwrap();
            assert!(!tuned.is_error, "{}", tuned.text);
            let chains: std::collections::BTreeSet<String> = script.asked()[asked_before..]
                .iter()
                .filter(|(method, params)| method == "read" && params["kind"] == "fx")
                .filter_map(|(_, params)| params["parent"].as_str().map(str::to_owned))
                .collect();
            assert_eq!(chains, ["r:track:{T0024}".to_string()].into(), "only the FX's own track's chain");
            let goal = json!({"goal": {"loudness": -14, "problems": false}, "track": "Track 3", "from_beat": 0, "beats": 2});
            assert!(!tool(&observed, "judge").execute(object(goal), Signal::new()).await.unwrap().is_error);
            let refused = tool(&observed, "tune")
                .execute(object(json!({"device": take_eq, "how": "search", "knobs": ["Gain-Band 2", "low cut"]})), Signal::new())
                .await
                .unwrap();
            assert!(refused.text.contains("on an item's take"), "{}", refused.text);
            reaper.close().await.unwrap();
            simulator.stop();
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn reaper_measures_sounds_lays_out_the_form_and_tunes_a_reaeq_on_scratch_copies() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let home = tempfile::tempdir().unwrap();
            let script = ReaperScript::new(3, 0);
            let eq = script.add_fx("r:track:{T0002}", "ReaEQ");
            // (each render done at its first status: a search renders many)
            script.listening().set_steps(0);
            let simulator = reaper_lab(&script, home.path(), |_| {}).await;
            let (reaper, told) = integration(Daw::Reaper, home.path(), create_reaper_integration);
            reaper.start(Signal::new()).await.unwrap();
            let observed = reaper.observe(Signal::new(), None).await.unwrap();
            let mut said = vec![];
            let sound = tool(&observed, "sound")
                .execute(object(json!({"tracks": ["Track 2"], "from_beat": 0, "beats": 8})), Signal::new())
                .await
                .unwrap();
            assert!(!sound.is_error, "{}", sound.text);
            said.push(sound.text);
            let form = tool(&observed, "form").execute(object(json!({"from_beat": 0, "beats": 16})), Signal::new()).await.unwrap();
            assert!(!form.is_error, "{}", form.text);
            said.push(form.text);
            let judge = tool(&observed, "judge");
            let start = judge
                .execute(
                    object(json!({"goal": {"loudness": -14, "problems": false}, "track": "Track 2", "from_beat": 0, "beats": 2})),
                    Signal::new(),
                )
                .await
                .unwrap();
            assert!(!start.is_error, "{}", start.text);
            let tuned = tool(&observed, "tune")
                .execute(object(json!({"device": eq, "how": "search", "knobs": ["Gain-Band 2", "Gain-Band 3"]})), Signal::new())
                .await
                .unwrap();
            assert!(!tuned.is_error, "{}", tuned.text);
            assert!(script.listening().renders().iter().any(|render| render["sources"].to_string().contains("r:track:{C")), "copies heard");
            assert_eq!(script.listening().scratch_copies(), 0, "the copies are gone after the search");
            said.push(tuned.text);
            said.extend(told.borrow().iter().cloned());
            assert_ours(&said);
            reaper.close().await.unwrap();
            simulator.stop();
        })
        .await;
}

/// A goal's request over one candidate, toward a reference of noise.
fn goal_request(home: &Path, track: &str) -> AuditionRequest {
    let reference = home.join("reference.wav");
    daw_mcp_server::simulator::listening::write_noise(&reference, 4., 0.4, 9).unwrap();
    AuditionRequest {
        candidates: vec![AuditionCandidate { track: track.into(), clip: None, label: None, mix: None }],
        from_beat: Some(0.),
        beats: Some(4.),
        reference: Some(reference.to_string_lossy().into_owned()),
        reference_from: None,
        reference_seconds: None,
        focus: None,
    }
}

/// (#361 final review) A meta device's Dest. Track, Effect and Parameter say where it aims, not how it sounds: a goal
/// never searches them, so a copy of the track kept as the best is never re-aimed at the producer's tracks.
#[tokio::test(flavor = "current_thread")]
async fn a_goal_never_searches_where_a_meta_device_aims() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let home = tempfile::tempdir().unwrap();
            let script = RenoiseScript::new();
            script.add_device(4, "EQ 10");
            script.add_device(4, "*LFO");
            let simulator = renoise_lab(&script, home.path()).await;
            let (renoise, _) = integration(Daw::Renoise, home.path(), create_renoise_integration);
            renoise.start(Signal::new()).await.unwrap();
            renoise.observe(Signal::new(), None).await.unwrap();
            let rig = renoise.goal(&goal_request(home.path(), "Bass"), Signal::new()).await.unwrap().unwrap();
            let names: Vec<String> = rig.slots()[0].knobs.iter().map(|knob| knob.name.clone()).collect();
            assert!(names.iter().all(|name| !name.starts_with("Dest. ")), "{names:?}");
            assert!(names.contains(&"Amplitude".to_string()), "the LFO's own knobs are searched: {names:?}");
            rig.close().await.unwrap();
            renoise.close().await.unwrap();
            simulator.stop();
        })
        .await;
}

/// (#361 final review) Two tracks called "Bass" (one duplicated): a goal's slot is the track it searched, as the book
/// follows it, never the first track of that name.
#[tokio::test(flavor = "current_thread")]
async fn a_renoise_goals_slot_is_its_own_track_when_another_has_its_name() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let home = tempfile::tempdir().unwrap();
            let script = RenoiseScript::new();
            script.add_device(3, "EQ 10");
            let eq = script.add_device(4, "EQ 10");
            script.rename_track(3, "Bass");
            let simulator = renoise_lab(&script, home.path()).await;
            let (renoise, _) = integration(Daw::Renoise, home.path(), create_renoise_integration);
            renoise.start(Signal::new()).await.unwrap();
            renoise.observe(Signal::new(), None).await.unwrap();
            let rig = renoise.goal(&goal_request(home.path(), "1:track:4"), Signal::new()).await.unwrap().unwrap();
            let slots = rig.slots();
            let knobs: Vec<_> = slots[0].knobs.iter().filter(|knob| knob.name.starts_with("Gain 2 ")).cloned().collect();
            assert_eq!(rig.settle(&slots[0].name, &knobs, &[6.], Signal::new()).await.unwrap(), None);
            assert_eq!(script.device_value(4, eq, "Gain 2 (63 Hz)"), Some(6.), "the track the goal searched");
            assert_eq!(script.device_value(3, eq, "Gain 2 (63 Hz)"), Some(0.), "the other “Bass” is untouched");
            rig.close().await.unwrap();
            renoise.close().await.unwrap();
            simulator.stop();
        })
        .await;
}

/// (#361 final review) A settle sets only what won, and never a knob the producer turned on the track since the search
/// began (it says so); when nothing won, nothing is set.
#[tokio::test(flavor = "current_thread")]
async fn a_goal_settles_only_what_won_and_leaves_a_knob_the_producer_turned_since() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let home = tempfile::tempdir().unwrap();
            let script = RenoiseScript::new();
            let eq = script.add_device(4, "EQ 10");
            let simulator = renoise_lab(&script, home.path()).await;
            let (renoise, _) = integration(Daw::Renoise, home.path(), create_renoise_integration);
            renoise.start(Signal::new()).await.unwrap();
            renoise.observe(Signal::new(), None).await.unwrap();
            let rig = renoise.goal(&goal_request(home.path(), "Bass"), Signal::new()).await.unwrap().unwrap();
            let slots = rig.slots();
            let knobs: Vec<_> = slots[0]
                .knobs
                .iter()
                .filter(|knob| knob.name.starts_with("Gain 2 ") || knob.name.starts_with("Gain 3 "))
                .cloned()
                .collect();
            assert_eq!(knobs.len(), 2, "{knobs:?}");
            let settles = || {
                script.asked().into_iter().filter(|(method, params)| method == "change_apply" && params["kind"] == "device-params").count()
            };
            // Nothing won: the track stays as it is.
            assert_eq!(rig.settle(&slots[0].name, &knobs, &[0., 0.], Signal::new()).await.unwrap(), None);
            assert_eq!(settles(), 0, "nothing set when nothing won");
            // The producer turns Gain 2 meanwhile: it stays as they set it, and Gain 3's win is set.
            script.set_device_value(4, eq, "Gain 2 (63 Hz)", 3.);
            let said = rig.settle(&slots[0].name, &knobs, &[6., -2.], Signal::new()).await.unwrap();
            assert!(said.as_deref().is_some_and(|said| said.contains("you turned EQ 10 Gain 2 (63 Hz)")), "{said:?}");
            assert_eq!(script.device_value(4, eq, "Gain 2 (63 Hz)"), Some(3.), "the producer's turn stays");
            assert_eq!(script.device_value(4, eq, "Gain 3 (125 Hz)"), Some(-2.), "the winner of the knob they left");
            rig.close().await.unwrap();
            renoise.close().await.unwrap();
            simulator.stop();
        })
        .await;
}

/// (#361 final review) A track that already ends in the producer's own Maximizer gets no safety limiter, and a settle
/// leaves their Maximizer's Boost as they set it.
#[tokio::test(flavor = "current_thread")]
async fn a_settle_leaves_the_producers_own_limiter_as_they_set_it() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let home = tempfile::tempdir().unwrap();
            let script = RenoiseScript::new();
            let eq = script.add_device(4, "EQ 10");
            let theirs = script.add_device(4, "Maximizer");
            script.set_device_value(4, theirs, "Boost", 4.);
            let simulator = renoise_lab(&script, home.path()).await;
            let (renoise, _) = integration(Daw::Renoise, home.path(), create_renoise_integration);
            renoise.start(Signal::new()).await.unwrap();
            renoise.observe(Signal::new(), None).await.unwrap();
            let rig = renoise.goal(&goal_request(home.path(), "Bass"), Signal::new()).await.unwrap().unwrap();
            let slots = rig.slots();
            assert_eq!(slots[0].chain, "TrackVolPan → EQ 10 → Maximizer", "no safety Maximizer after theirs");
            let knobs: Vec<_> = slots[0].knobs.iter().filter(|knob| knob.name.starts_with("Gain 2 ")).cloned().collect();
            assert_eq!(rig.settle(&slots[0].name, &knobs, &[6.], Signal::new()).await.unwrap(), None);
            assert_eq!(script.device_value(4, eq, "Gain 2 (63 Hz)"), Some(6.));
            assert_eq!(script.device_value(4, theirs, "Boost"), Some(4.), "the producer's Boost stays as they set it");
            rig.close().await.unwrap();
            renoise.close().await.unwrap();
            simulator.stop();
        })
        .await;
}

/// (#361 final review) A settle over two devices is one row of HISTORY, and Kumi's undo of it takes both back.
#[tokio::test(flavor = "current_thread")]
async fn a_goals_settle_over_two_devices_is_one_change_kumis_undo_takes_back() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let home = tempfile::tempdir().unwrap();
            let script = RenoiseScript::new();
            let eq = script.add_device(4, "EQ 10");
            let gainer = script.add_device(4, "Gainer");
            let simulator = renoise_lab(&script, home.path()).await;
            let (renoise, _) = integration(Daw::Renoise, home.path(), create_renoise_integration);
            renoise.start(Signal::new()).await.unwrap();
            renoise.observe(Signal::new(), None).await.unwrap();
            let rig = renoise.goal(&goal_request(home.path(), "Bass"), Signal::new()).await.unwrap().unwrap();
            let slots = rig.slots();
            let knobs: Vec<_> = slots[0]
                .knobs
                .iter()
                .filter(|knob| knob.name.starts_with("Gain 2 ") || (knob.device.ends_with("Gainer") && knob.name == "Gain"))
                .cloned()
                .collect();
            assert_eq!(knobs.len(), 2, "{knobs:?}");
            assert_eq!(rig.settle(&slots[0].name, &knobs, &[6., 2.], Signal::new()).await.unwrap(), None);
            assert_eq!((script.device_value(4, eq, "Gain 2 (63 Hz)"), script.device_value(4, gainer, "Gain")), (Some(6.), Some(2.)));
            let observed = renoise.observe(Signal::new(), None).await.unwrap();
            let undo = tool(&observed, "undo_change").execute(object(json!({"change": "last"})), Signal::new()).await.unwrap();
            assert!(!undo.is_error, "{}", undo.text);
            assert_eq!(
                (script.device_value(4, eq, "Gain 2 (63 Hz)"), script.device_value(4, gainer, "Gain")),
                (Some(0.), Some(1.)),
                "one row: both back ({})",
                undo.text
            );
            rig.close().await.unwrap();
            renoise.close().await.unwrap();
            simulator.stop();
        })
        .await;
}

/// (#361 final review) A paused goal's best waits muted. When the producer worked on it, a better best can't take it away
/// (Kumi's undo refuses), and it stays muted rather than playing beside the original.
#[tokio::test(flavor = "current_thread")]
async fn a_goal_best_the_producer_worked_on_stays_muted_when_a_better_one_comes() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let home = tempfile::tempdir().unwrap();
            let script = ReaperScript::new(3, 0);
            script.add_fx("r:track:{T0002}", "ReaEQ");
            let simulator = reaper_lab(&script, home.path(), |_| {}).await;
            let (reaper, told) = integration(Daw::Reaper, home.path(), create_reaper_integration);
            reaper.start(Signal::new()).await.unwrap();
            reaper.observe(Signal::new(), None).await.unwrap();
            let rig = reaper.goal(&goal_request(home.path(), "Track 2"), Signal::new()).await.unwrap().unwrap();
            let slots = rig.slots();
            let knobs: Vec<_> = slots[0].knobs.iter().filter(|knob| knob.name == "Gain-Band 2").cloned().collect();
            let trial = GenerationTrial { slot: slots[0].name.clone(), knobs: knobs.clone(), values: vec![0.7], fresh: None };
            rig.generation(&[trial], Signal::new(), None).await.unwrap();
            assert_eq!(rig.keep_best(&slots[0].name, &knobs, &[0.7], Signal::new()).await.unwrap(), "Kumi · Goal best");
            let first = script.listening().kept();
            assert!(script.listening().kept_muted(&first[0].0), "the best waits muted");
            // The producer works on it (renames it): it's theirs now.
            script.listening().work_on_kept(&first[0].0, "My best");
            assert_eq!(rig.keep_best(&slots[0].name, &knobs, &[0.7], Signal::new()).await.unwrap(), "Kumi · Goal best");
            assert!(script.listening().is_kept(&first[0].0), "the producer's stays");
            assert!(script.listening().kept_muted(&first[0].0), "and stays muted: {:?}", told.borrow());
            rig.close().await.unwrap();
            reaper.close().await.unwrap();
            simulator.stop();
        })
        .await;
}

/// (#361 final review) A stop (or a failed read) right after the safety limiter's add still takes it off when the goal
/// doesn't open: it's kept before the chain is read again.
#[tokio::test(flavor = "current_thread")]
async fn a_goal_that_fails_right_after_its_safety_limiters_add_takes_it_off_again() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let home = tempfile::tempdir().unwrap();
            let script = RenoiseScript::new();
            script.add_device(4, "EQ 10");
            let paths = BridgePaths::new(home.path(), Daw::Renoise);
            let secret = ensure_secret(&paths.secret()).unwrap();
            script.listening().set_folder(&paths.root.join("listening"));
            let mut config = script.config(secret, "/lab/Renoise");
            config.discovery_dir = Some(paths.discovery());
            // The chain's read right after the Maximizer's add fails once.
            let inner = config.respond.take().unwrap();
            let added = Arc::new(std::sync::atomic::AtomicBool::new(false));
            config.respond = Some(Arc::new(move |method: &str, params: &Value| {
                use std::sync::atomic::Ordering::SeqCst;
                if method == "change_apply" && params["kind"] == "device-add" {
                    added.store(true, SeqCst);
                } else if method == "read" && params["kind"] == "device" && added.swap(false, SeqCst) {
                    return Some(Err(("busy".to_string(), "Renoise is showing a dialog".to_string())));
                }
                inner(method, params)
            }));
            let simulator = Simulator::start(config).await.unwrap();
            let (renoise, _) = integration(Daw::Renoise, home.path(), create_renoise_integration);
            renoise.start(Signal::new()).await.unwrap();
            renoise.observe(Signal::new(), None).await.unwrap();
            let refused = renoise.goal(&goal_request(home.path(), "Bass"), Signal::new()).await.unwrap();
            assert!(refused.is_err(), "the goal doesn't open");
            assert_eq!(
                script.device_paths(4),
                ["Audio/Effects/Native/TrackVolPan", "Audio/Effects/Native/EQ 10"],
                "the safety Maximizer is gone"
            );
            renoise.close().await.unwrap();
            simulator.stop();
        })
        .await;
}

/// (#361 final review) A pattern's audio is a render in Renoise: the tempo its tempo commands played into the transport
/// goes back as the call ends, said as a step of Renoise's undo, as a listen's does.
#[tokio::test(flavor = "current_thread")]
async fn a_patterns_audio_puts_back_the_tempo_its_render_moved() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let home = tempfile::tempdir().unwrap();
            let script = RenoiseScript::new();
            script.set_render_bpm(Some(140.));
            let simulator = renoise_lab(&script, home.path()).await;
            let (renoise, told) = integration(Daw::Renoise, home.path(), create_renoise_integration);
            renoise.start(Signal::new()).await.unwrap();
            renoise.observe(Signal::new(), None).await.unwrap();
            let file = renoise.audio_file("1:pattern:2", Signal::new()).await.unwrap();
            assert!(file.is_some(), "the pattern's render");
            assert_eq!(script.song()["bpm"], 128.0, "the producer's tempo");
            let said = told.borrow().iter().filter(|title| title.contains("tempo back")).cloned().collect::<Vec<_>>();
            assert_eq!(said, ["Kumi set the tempo back to 128 BPM, LPB 4 (1 step in Renoise's undo)."]);
            renoise.close().await.unwrap();
            simulator.stop();
        })
        .await;
}

/// (#361 final review) An instrument track (only a VSTi): the gain FX a quieter round adds goes after the synth, where
/// the track's sound is, so the turn is heard.
#[tokio::test(flavor = "current_thread")]
async fn a_gain_fx_rebalancing_adds_goes_after_an_instrument_last_on_the_track() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let home = tempfile::tempdir().unwrap();
            let script = ReaperScript::new(3, 0);
            script.add_plugin_fx("r:track:{T0002}", "VSTi: Serum (Xfer Records)");
            let simulator = reaper_lab(&script, home.path(), |_| {}).await;
            let (reaper, _) = integration(Daw::Reaper, home.path(), create_reaper_integration);
            reaper.start(Signal::new()).await.unwrap();
            let observed = reaper.observe(Signal::new(), None).await.unwrap();
            let judge = tool(&observed, "judge");
            let start = judge
                .execute(
                    object(json!({"goal": {"loudness": -6, "problems": false}, "track": "Track 2", "from_beat": 0, "beats": 8})),
                    Signal::new(),
                )
                .await
                .unwrap();
            assert!(!start.is_error, "{}", start.text);
            script.listening().set_level("r:track:{T0002}", 0.4);
            let context: Value = serde_json::from_str(&observed.context).unwrap();
            let tracks = context["tracks"].as_array().unwrap();
            let synth = tracks.iter().find(|track| track["name"] == "Track 2").expect("Track 2")["ref"].as_str().unwrap().to_owned();
            let change =
                tool(&observed, "track_volume").execute(object(json!({"ref": synth, "volumeDb": -3})), Signal::new()).await.unwrap();
            assert!(!change.is_error, "{}", change.text);
            let round = judge.execute(object(json!({"change": "Track 2 down 3 dB"})), Signal::new()).await.unwrap();
            assert!(!round.is_error, "{}", round.text);
            // (the simulator's track renders don't hear a gain FX on the track, so the round takes it back again: where it
            // was added is what counts)
            let added: Vec<Value> = script
                .asked()
                .into_iter()
                .filter(|(method, params)| method == "change_apply" && params["kind"] == "fx-add")
                .map(|(_, params)| params["input"].clone())
                .collect();
            assert_eq!(added.len(), 1, "{added:?} {}", round.text);
            assert_eq!(added[0]["name"], "JS: Volume Adjustment");
            assert!(added[0].get("index").is_none(), "appended after the synth, not put before it: {:?}", added[0]);
            reaper.close().await.unwrap();
            simulator.stop();
        })
        .await;
}
