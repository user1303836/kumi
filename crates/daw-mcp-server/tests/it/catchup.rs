//! Catch-up and watch_me in the host (M6) against the simulated REAPER: what changed while Kumi was closed or REAPER was
//! restarted, the unsaved project kept by its identity, a snapshot never kept again before it was compared, and what the
//! producer did while Kumi watched, with the parameters they moved by name.
mod common;

use common::Session;
use daw_bridge::files::ensure_secret;
use daw_bridge::paths::BridgePaths;
use daw_bridge::Daw;
use daw_mcp_server::simulator::reaper::ReaperScript;
use daw_mcp_server::simulator::renoise::RenoiseScript;
use daw_mcp_server::simulator::{Simulator, SimulatorConfig};
use serde_json::{json, Value};

const PROJECT_ID: &str = "0123456789abcdef0123456789abcdef";

struct Lab {
    home: tempfile::TempDir,
    session: Session,
    script: ReaperScript,
    simulator: Simulator,
    next_id: u64,
}

impl Lab {
    async fn start(saved: bool) -> Lab {
        let home = tempfile::tempdir().unwrap();
        let paths = BridgePaths::new(home.path(), Daw::Reaper);
        let secret = ensure_secret(&paths.secret()).unwrap();
        let script = ReaperScript::new(3, 0);
        if saved {
            script.set_project_id(Some(PROJECT_ID));
        }
        let mut config = script.config(secret, "/lab/REAPER");
        config.discovery_dir = Some(paths.discovery());
        let simulator = Simulator::start(config).await.unwrap();
        let session = Session::start(Daw::Reaper, home.path());
        Lab { home, session, script, simulator, next_id: 100 }
    }

    /// Kumi closed and opened again: another host, with nothing in memory.
    fn restart_kumi(&mut self) {
        self.session = Session::start(Daw::Reaper, self.home.path());
    }

    async fn tool(&mut self, name: &str, arguments: Value) -> (Value, bool) {
        self.next_id += 1;
        let id = self.next_id;
        self.session.tool_full(id, name, arguments, false).await
    }

    async fn ok(&mut self, name: &str) -> Value {
        let (answer, is_error) = self.tool(name, json!({})).await;
        assert!(!is_error, "{name}: {answer}");
        answer
    }
}

/// The changes an answer lists, short: "+FX ReaComp", "~track Bass (mixer.volume)".
fn said(answer: &Value) -> Vec<String> {
    answer["changes"]
        .as_array()
        .unwrap_or(&vec![])
        .iter()
        .map(|change| {
            let kind = change["kind"].as_str().unwrap_or("");
            match change["change"].as_str().unwrap_or("") {
                "added" => format!("+{kind} {}", change["item"]["name"].as_str().unwrap_or("")),
                "removed" => format!("-{kind} {}", change["item"]["name"].as_str().unwrap_or("")),
                _ => {
                    let what: Vec<&str> = change["details"].as_array().unwrap().iter().filter_map(|d| d["what"].as_str()).collect();
                    format!("~{kind} {} ({})", change["after"]["name"].as_str().unwrap_or(""), what.join(", "))
                }
            }
        })
        .collect()
}

#[tokio::test]
async fn edits_made_while_kumi_was_closed_are_told_once_and_the_snapshot_follows() {
    let mut lab = Lab::start(true).await;
    let first = lab.ok("catch_up").await;
    assert_eq!(first["first"], true, "{first}");
    assert_eq!(first["place"], format!("reaper-{PROJECT_ID}"));
    // Kumi closes; the producer works in REAPER meanwhile.
    lab.restart_kumi();
    lab.script.set_volume(1, -6.0);
    lab.script.rename(2, "Bass");
    lab.script.add_fx_with(3, "ReaComp", &[("Threshold", 0.5)]);
    lab.script.set_tempo(124.0);
    let caught = lab.ok("catch_up").await;
    assert_eq!(said(&caught), ["~track Track 1 (mixer.volume)", "~track Bass ()", "+FX ReaComp"], "{caught}");
    assert_eq!(caught["changes"][1]["renamed"], true);
    assert_eq!(caught["project"], json!([{"facet": "tempo", "what": "tempo", "from": 120.0, "to": 124.0}]));
    assert!(caught["lastSeenAt"].as_i64().unwrap() > 0);
    // Catch-up says what changed, not to what: the producer did it, and the look shows it now.
    assert_eq!(caught["changes"][0]["details"][0]["to"], Value::Null);
    assert!(caught["changes"][2]["item"].get("ref").is_none(), "no refs in a catch-up: {caught}");
    // Told once: the snapshot was kept again.
    let again = lab.ok("catch_up").await;
    assert_eq!(again["same"], true, "{again}");
    // A digest whose pages are small adds up the same.
    lab.script.digest_pages_of(1);
    lab.script.set_volume(3, -1.0);
    let paged = lab.ok("catch_up").await;
    assert_eq!(said(&paged), ["~track Track 3 (mixer.volume)"], "{paged}");
}

#[tokio::test]
async fn a_restarted_reaper_is_compared_before_its_snapshot_is_kept_again() {
    let mut lab = Lab::start(true).await;
    lab.ok("catch_up").await;
    assert_eq!(lab.ok("baseline_save").await["saved"], true);
    // REAPER quits and opens the project again, with an edit made in between: another run is another identity, so
    // the host won't keep a snapshot over the one it has before catch_up compared them.
    lab.script.restart();
    lab.script.set_volume(2, -3.0);
    let refused = lab.ok("baseline_save").await;
    assert_eq!(refused["saved"], false, "{refused}");
    assert_eq!(refused["compared"], false, "Kumi waits for its next catch-up: {refused}");
    let caught = lab.ok("catch_up").await;
    assert_eq!(said(&caught), ["~track Track 2 (mixer.volume)"], "{caught}");
    assert_eq!(lab.ok("baseline_save").await["saved"], true);
}

#[tokio::test]
async fn an_unsaved_project_is_kept_by_the_identity_reaper_gives_it_while_it_runs() {
    let mut lab = Lab::start(false).await;
    let first = lab.ok("catch_up").await;
    assert_eq!((first["first"].clone(), first["place"].clone()), (json!(true), json!("reaper-unsaved")), "{first}");
    // Kumi closes and comes back while REAPER keeps the same unsaved project open: it's found by its identity.
    lab.restart_kumi();
    lab.script.set_volume(1, -12.0);
    assert_eq!(said(&lab.ok("catch_up").await), ["~track Track 1 (mixer.volume)"]);
    // Saved for the first time (it has an id now): the snapshot it had unsaved carries over to its place.
    lab.script.set_project_id(Some(PROJECT_ID));
    lab.restart_kumi();
    lab.script.rename(1, "Lead");
    let saved = lab.ok("catch_up").await;
    assert_eq!(
        (saved["place"].clone(), said(&saved)),
        (json!(format!("reaper-{PROJECT_ID}")), vec!["~track Lead ()".to_string()]),
        "{saved}"
    );
    // A new unsaved project in another run of REAPER is another project: nothing to compare.
    lab.script.set_project_id(None);
    lab.script.restart();
    assert_eq!(lab.ok("catch_up").await["first"], true);
}

#[tokio::test]
async fn watching_tells_what_the_producer_added_and_the_parameters_they_moved_by_name() {
    let mut lab = Lab::start(true).await;
    let comp = lab.script.add_fx_with(1, "ReaComp", &[("Threshold", 0.5), ("Ratio", 0.1)]);
    let (refused, is_error) = lab.tool("watch_stop", json!({})).await;
    assert!(is_error && refused["code"] == "not-watching", "{refused}");
    assert_eq!(lab.ok("watch_start").await["watching"], true);
    lab.script.set_param(&comp, 1, 0.4);
    let eq = lab.script.add_fx_with(2, "ReaEQ", &[("Band 1 Gain", 0.5), ("Band 1 Freq", 0.25)]);
    lab.simulator.emit("selection", json!({"tracks": [{"index": 2, "guid": "{T0002}", "name": "Track 2"}]}));
    // A look while watching sees the EQ as it came…
    tokio::time::sleep(std::time::Duration::from_millis(1600)).await;
    // …so its gain, turned after, is told against that, not against nothing.
    lab.script.set_param(&eq, 0, 0.75);
    let watched = lab.ok("watch_stop").await;
    assert_eq!(said(&watched), ["~FX ReaComp (parameters.parametersHash, parameters.Ratio)", "+FX ReaEQ"], "{watched}");
    let ratio = &watched["changes"][0]["details"][1];
    assert_eq!((ratio["from"].clone(), ratio["to"].clone()), (json!(0.1), json!({"value": 0.4, "shows": "0.4"})));
    let added = &watched["changes"][1];
    assert_eq!(added["item"]["on"], "Track 2");
    assert_eq!(added["item"]["ref"], format!("r:fx:{eq}"), "watch_me keeps refs: the model may do it again now");
    assert_eq!(added["madeOf"]["parameters"], json!([{"name": "Band 1 Gain", "value": {"value": 0.75, "shows": "0.8"}}]));
    assert_eq!(watched["selected"], json!([{"name": "Track 2", "key": "{T0002}"}]));
    assert!(watched["looks"].as_u64().unwrap() >= 1, "{watched}");
    // Another project open by the time the producer is done: nothing to compare.
    lab.ok("watch_start").await;
    lab.script.switch_tab();
    let (other, is_error) = lab.tool("watch_stop", json!({})).await;
    assert!(is_error && other["code"] == "other-project", "{other}");
}

// The looks go on with the connection the host makes after the bridge let go of the old one; a watch whose looks found no
// connection says when they stopped.
#[tokio::test]
async fn watching_goes_on_with_the_hosts_new_connection_and_says_when_its_looks_stopped() {
    let mut lab = Lab::start(true).await;
    let pause = |ms| tokio::time::sleep(std::time::Duration::from_millis(ms));
    lab.ok("watch_start").await;
    lab.simulator.retire_with_bye();
    pause(200).await;
    assert!(!lab.tool("read", json!({"kind": "track"})).await.1, "the host connects again");
    let eq = lab.script.add_fx_with(2, "ReaEQ", &[("Band 1 Gain", 0.5), ("Band 1 Freq", 0.25)]);
    // A look on the new connection sees the EQ as it came, so its gain, turned after, is told against that.
    pause(1600).await;
    lab.script.set_param(&eq, 0, 0.75);
    let watched = lab.ok("watch_stop").await;
    assert_eq!(said(&watched), ["+FX ReaEQ"], "{watched}");
    assert_eq!(watched["changes"][0]["madeOf"]["parameters"], json!([{"name": "Band 1 Gain", "value": {"value": 0.75, "shows": "0.8"}}]));
    assert!(watched.get("stoppedLooking").is_none(), "{watched}");

    // Lost, and made again only by watch_stop: what the looks missed is said.
    lab.ok("watch_start").await;
    lab.simulator.retire_with_bye();
    pause(1600).await;
    let watched = lab.ok("watch_stop").await;
    let why = watched["stoppedLooking"]["why"].as_str().unwrap_or_default();
    assert!(
        why.starts_with("Kumi's looks found no connection to REAPER from then on") && watched["stoppedLooking"]["at"].is_i64(),
        "{watched}"
    );
}

#[tokio::test]
async fn a_bridge_without_digest_says_it_cant_compare() {
    let home = tempfile::tempdir().unwrap();
    let paths = BridgePaths::new(home.path(), Daw::Reaper);
    let secret = ensure_secret(&paths.secret()).unwrap();
    let mut config = SimulatorConfig::reaper(secret, "/lab/REAPER");
    config.discovery_dir = Some(paths.discovery());
    let _simulator = Simulator::start(config).await.unwrap();
    let mut session = Session::start(Daw::Reaper, home.path());
    let (answer, is_error) = session.tool_full(1, "catch_up", json!({}), false).await;
    assert!(is_error, "{answer}");
    assert_eq!(answer["code"], "unsupported");
    assert!(answer["error"].as_str().unwrap().contains("update Kumi's bridge in REAPER"), "{answer}");
}

/// Renoise: the demo song the tool's vectors hold (its digest as the tool answered it), with what the producer did since.
async fn renoise_lab() -> (tempfile::TempDir, Session, RenoiseScript, Simulator) {
    let home = tempfile::tempdir().unwrap();
    let paths = BridgePaths::new(home.path(), Daw::Renoise);
    let secret = ensure_secret(&paths.secret()).unwrap();
    let script = RenoiseScript::new();
    let mut config = script.config(secret, "/lab/Renoise/V3.5.4");
    config.discovery_dir = Some(paths.discovery());
    let simulator = Simulator::start(config).await.unwrap();
    let session = Session::start(Daw::Renoise, home.path());
    (home, session, script, simulator)
}

#[tokio::test]
async fn renoise_tracks_without_identity_are_matched_and_what_changed_is_told_in_its_words() {
    let (home, mut session, script, _simulator) = renoise_lab().await;
    let (first, _) = session.tool_full(1, "catch_up", json!({}), false).await;
    assert_eq!(first["first"], true, "{first}");
    assert!(first["place"].as_str().unwrap().starts_with("renoise-"), "{first}");
    // Kumi closes; in Renoise the producer turns the Hat track down, names Bass "Sub", adds a device and speeds up.
    let mut session = Session::start(Daw::Renoise, home.path());
    script.set_volume(2, -6.0);
    script.rename_track(4, "Sub");
    script.add_device_by_hand(4);
    script.set_bpm(140.0);
    let (caught, is_error) = session.tool_full(2, "catch_up", json!({}), false).await;
    assert!(!is_error, "{caught}");
    assert_eq!(said(&caught), ["~track Hat (mixer.volume)", "~track Sub ()", "+device EQ 10"], "{caught}");
    assert_eq!(caught["changes"][1]["before"]["name"], "Bass");
    assert_eq!(caught["project"][0]["what"], "bpm");
    let (again, _) = session.tool_full(3, "catch_up", json!({}), false).await;
    assert_eq!(again["same"], true, "{again}");
}

#[tokio::test]
async fn renoise_watching_tells_the_device_the_producer_added_on_its_track() {
    let (_home, mut session, script, _simulator) = renoise_lab().await;
    let (started, _) = session.tool_full(1, "watch_start", json!({}), false).await;
    assert_eq!(started["watching"], true, "{started}");
    script.add_device_by_hand(1);
    let (watched, is_error) = session.tool_full(2, "watch_stop", json!({}), false).await;
    assert!(!is_error, "{watched}");
    // (the track's device count sums its devices up: told by the device, not twice)
    assert_eq!(said(&watched), ["+device EQ 10"], "{watched}");
    assert_eq!(watched["changes"][0]["item"]["on"], "Kick");
}

#[tokio::test]
async fn a_watch_stopped_while_the_daw_is_busy_goes_on_until_it_answers() {
    let mut lab = Lab::start(true).await;
    lab.ok("watch_start").await;
    lab.script.add_fx_with(1, "ReaComp", &[("Threshold", 0.5)]);
    lab.simulator.set_busy(Some("modal"));
    let (busy, is_error) = lab.tool("watch_stop", json!({})).await;
    assert!(is_error, "{busy}");
    lab.simulator.set_busy(None);
    let watched = lab.ok("watch_stop").await;
    assert_eq!(said(&watched), ["+FX ReaComp"], "{watched}");
}
