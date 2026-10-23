//! The host's listening tools (M5 contract §2) against the simulated REAPER and Renoise: a render started as a job and
//! followed to its files, cancelled, refused while another runs, failing; a live capture; a file played out loud; the
//! audio behind a ref; scratch copies; the checks each tool makes before it asks the bridge; an older bridge; and the
//! host stopping what still runs when it goes.
#[path = "../common/mod.rs"]
mod common;

use common::Session;
use daw_bridge::files::ensure_secret;
use daw_bridge::paths::BridgePaths;
use daw_bridge::Daw;
use daw_mcp_server::simulator::reaper::ReaperScript;
use daw_mcp_server::simulator::renoise::RenoiseScript;
use daw_mcp_server::simulator::{Simulator, SimulatorConfig};
use serde_json::{json, Value};
use std::path::PathBuf;

struct Lab {
    home: tempfile::TempDir,
    session: Session,
    _simulator: Simulator,
    next_id: u64,
}

impl Lab {
    async fn start(daw: Daw, config: impl FnOnce(daw_bridge::auth::Secret, PathBuf) -> SimulatorConfig) -> Lab {
        let home = tempfile::tempdir().unwrap();
        let paths = BridgePaths::new(home.path(), daw);
        let secret = ensure_secret(&paths.secret()).unwrap();
        let mut config = config(secret, paths.root.join("listening"));
        config.discovery_dir = Some(paths.discovery());
        let simulator = Simulator::start(config).await.unwrap();
        let session = Session::start(daw, home.path());
        Lab { home, session, _simulator: simulator, next_id: 100 }
    }

    async fn reaper(script: &ReaperScript) -> Lab {
        let script = script.clone();
        Lab::start(Daw::Reaper, move |secret, folder| {
            script.listening().set_folder(&folder);
            script.config(secret, "/lab/REAPER")
        })
        .await
    }

    async fn renoise(script: &RenoiseScript) -> Lab {
        let script = script.clone();
        Lab::start(Daw::Renoise, move |secret, folder| {
            script.listening().set_folder(&folder);
            script.config(secret, "/lab/Renoise")
        })
        .await
    }

    async fn tool(&mut self, name: &str, arguments: Value) -> (Value, bool) {
        self.next_id += 1;
        let id = self.next_id;
        self.session.tool_full(id, name, arguments, false).await
    }

    /// The job followed to its end.
    async fn wait(&mut self, job: &Value) -> Value {
        for _ in 0..20 {
            let (status, is_error) = self.tool("listen_job", json!({"jobId": job["jobId"], "waitMs": 1000})).await;
            assert!(!is_error, "{status}");
            if ["done", "failed", "cancelled"].contains(&status["state"].as_str().unwrap()) {
                return status;
            }
        }
        panic!("the job never ended");
    }
}

fn two_bars() -> Value {
    json!({"fromBeat": 16, "beats": 8})
}

#[tokio::test]
async fn a_render_is_a_job_followed_to_its_files_in_kumis_folder_with_what_each_holds() {
    let script = ReaperScript::new(3, 0);
    let mut lab = Lab::reaper(&script).await;
    let (status, _) = lab.tool("status", json!({})).await;
    assert_eq!(status["listening"]["render"], true, "{status}");
    assert_eq!(status["listening"]["capture"], true);
    let (job, is_error) = lab
        .tool(
            "render",
            json!({"sources": [{"id": "mix", "what": "master"}, {"id": "bass", "what": "track", "ref": "r:track:{T0002}"}], "range": two_bars(), "preRollBeats": 4, "epoch": "sim-1"}),
        )
        .await;
    assert!(!is_error, "{job}");
    assert_eq!((job["kind"].as_str(), job["state"].as_str()), (Some("render"), Some("running")));
    assert!(job["jobId"].as_str().unwrap().starts_with('j'));
    // Planned at once: the part and its pre-roll, 6 s at 120 BPM, in Kumi's folder.
    assert_eq!(job["seconds"], 6.0);
    let folder = lab.home.path().join("bridges/reaper/listening");
    for file in job["files"].as_array().unwrap() {
        assert!(PathBuf::from(file["file"].as_str().unwrap()).starts_with(&folder), "{file}");
    }
    // The bridge was told the job's id and the room Kumi's folder has.
    let asked = &script.listening().renders()[0];
    assert_eq!(asked["jobId"], job["jobId"]);
    assert!(asked["freeBytes"].as_u64().is_some_and(|free| free > 0), "{asked}");
    let done = lab.wait(&job).await;
    assert_eq!((done["state"].as_str(), done["progress"].as_f64()), (Some("done"), Some(1.0)), "{done}");
    let files = done["files"].as_array().unwrap();
    assert_eq!(files[0]["stage"], "master");
    assert_eq!(files[1]["stage"], "pre-fader", "a track renders before its fader by default");
    assert_eq!(files[1]["start"], 2.0, "the part starts after the pre-roll");
    for file in files {
        let bytes = std::fs::metadata(file["file"].as_str().unwrap()).unwrap().len();
        assert!(bytes > 6 * 48_000 * 8, "{file}: {bytes} bytes");
    }
    assert_eq!(script.listening().restored(), 1, "what the render changed is put back");
}

#[tokio::test]
async fn a_render_stopped_part_way_is_put_back_and_its_files_go() {
    let script = ReaperScript::new(3, 0);
    script.listening().set_steps(5);
    let mut lab = Lab::reaper(&script).await;
    let (job, _) = lab.tool("render", json!({"sources": [{"id": "mix", "what": "master"}], "range": two_bars()})).await;
    let (status, _) = lab.tool("listen_job", json!({"jobId": job["jobId"]})).await;
    assert_eq!(status["state"], "running");
    assert!(status["progress"].as_f64().unwrap() < 1.0);
    // Another render while this one runs is refused at once: one job at a time.
    let (second, is_error) = lab.tool("render", json!({"sources": [{"id": "mix", "what": "master"}], "range": two_bars()})).await;
    assert!(is_error && second["code"] == "busy" && second["busy"] == true, "{second}");
    let (cancelled, is_error) = lab.tool("listen_cancel", json!({"jobId": job["jobId"]})).await;
    assert!(!is_error, "{cancelled}");
    assert_eq!(cancelled["state"], "cancelled");
    assert!(!PathBuf::from(job["files"][0]["file"].as_str().unwrap()).exists());
    assert_eq!(script.listening().restored(), 1);
    let (status, _) = lab.tool("listen_job", json!({"jobId": job["jobId"]})).await;
    assert_eq!(status["state"], "cancelled");
}

#[tokio::test]
async fn a_render_that_fails_says_why_in_the_daws_words() {
    let script = ReaperScript::new(3, 0);
    script.listening().fail_next("failed", "REAPER couldn't write the render: the disk is full; every render setting is back");
    let mut lab = Lab::reaper(&script).await;
    let (job, _) = lab.tool("render", json!({"sources": [{"id": "mix", "what": "master"}], "range": two_bars()})).await;
    let ended = lab.wait(&job).await;
    assert_eq!((ended["state"].as_str(), ended["code"].as_str()), (Some("failed"), Some("failed")), "{ended}");
    assert!(ended["error"].as_str().unwrap().contains("every render setting is back"));
}

#[tokio::test]
async fn each_tool_checks_what_it_is_given_before_it_asks_the_bridge() {
    let script = ReaperScript::new(3, 0);
    let mut lab = Lab::reaper(&script).await;
    let mix = json!([{"id": "mix", "what": "master"}]);
    for (tool, arguments) in [
        ("render", json!({"sources": [], "range": two_bars()})),
        ("render", json!({"sources": mix, "range": {"fromBeat": 0, "beats": 0}})),
        ("render", json!({"sources": mix, "range": two_bars(), "preRollBeats": 65})),
        ("render", json!({"sources": mix, "range": two_bars(), "sampleRate": 22050})),
        (
            "render",
            json!({"sources": [{"id": "a", "what": "track", "ref": "r:track:{T0001}", "stage": "items"}, {"id": "b", "what": "master"}], "range": two_bars()}),
        ),
        ("hear", json!({"seconds": 0.5})),
        ("hear", json!({"seconds": 61})),
        ("listen_job", json!({"jobId": "j1", "waitMs": 10_001})),
        ("audition", json!({"file": "relative.wav"})),
        ("audition", json!({"file": "/no/such/file.wav"})),
        ("audio_file", json!({})),
        ("scratch", json!({"action": "make", "ref": "r:track:{T0001}", "count": 17, "label": "search"})),
        (
            "scratch",
            json!({"action": "set", "scratchId": "s1", "values": [{"ref": "r:track:{C0001}", "device": 0, "param": 0, "normalized": 2}]}),
        ),
        ("scratch", json!({"action": "melt"})),
    ] {
        let (answer, is_error) = lab.tool(tool, arguments.clone()).await;
        assert!(is_error && answer["code"] == "bad-params", "{tool} {arguments}: {answer}");
    }
    assert!(script.listening().renders().is_empty(), "nothing reached the bridge");
}

#[tokio::test]
async fn reaper_hears_what_it_plays_only_while_it_plays_with_where_the_project_was() {
    let script = ReaperScript::new(3, 0);
    let mut lab = Lab::reaper(&script).await;
    let (refused, is_error) = lab.tool("hear", json!({"seconds": 4})).await;
    assert!(is_error && refused["error"].as_str().unwrap().contains("REAPER isn't playing"), "{refused}");
    script.listening().set_playing(true);
    let (job, is_error) = lab.tool("hear", json!({"seconds": 4})).await;
    assert!(!is_error && job["kind"] == "capture", "{job}");
    let done = lab.wait(&job).await;
    assert_eq!(done["files"][0]["id"], "master");
    assert_eq!(done["marks"][0][0], 0, "a mark at the first frame: {done}");
}

#[tokio::test]
async fn a_file_plays_out_loud_once_and_stops_when_asked() {
    let script = ReaperScript::new(3, 0);
    let mut lab = Lab::reaper(&script).await;
    let file = lab.home.path().join("take.wav");
    daw_mcp_server::simulator::listening::write_noise(&file, 3., 0.3, 1).unwrap();
    let (playing, is_error) = lab.tool("audition", json!({"file": file, "seconds": 2})).await;
    assert!(!is_error, "{playing}");
    assert_eq!(
        (playing["playing"].as_bool(), playing["via"].as_str(), playing["seconds"].as_f64()),
        (Some(true), Some("preview"), Some(2.0))
    );
    assert!(playing["previewId"].as_str().unwrap().starts_with('p'));
    let (stopped, _) = lab.tool("audition_stop", json!({"previewId": playing["previewId"]})).await;
    assert_eq!(stopped["stopped"], true);
    let (again, _) = lab.tool("audition_stop", json!({})).await;
    assert_eq!(again["stopped"], false, "nothing was playing");
    script.listening().set_audition(false);
    let (refused, is_error) = lab.tool("audition", json!({"file": file})).await;
    assert!(is_error && refused["error"].as_str().unwrap().contains("can't play"), "{refused}");
}

#[tokio::test]
async fn a_render_stops_what_still_plays_out_loud_and_says_so_but_not_a_play_that_ended() {
    let script = ReaperScript::new(3, 0);
    let mut lab = Lab::reaper(&script).await;
    let file = lab.home.path().join("take.wav");
    daw_mcp_server::simulator::listening::write_noise(&file, 3., 0.3, 1).unwrap();
    let render = json!({"sources": [{"id": "mix", "what": "master"}], "range": two_bars(), "epoch": "sim-1"});
    let stopped = "The audition playing was stopped";
    let (playing, _) = lab.tool("audition", json!({"file": file, "seconds": 2})).await;
    assert_eq!(playing["playing"], true, "{playing}");
    let (job, _) = lab.tool("render", render.clone()).await;
    let done = lab.wait(&job).await;
    assert!(done["notes"].as_array().is_some_and(|notes| notes.iter().any(|note| note.as_str().unwrap().starts_with(stopped))), "{done}");
    // A play of a fifth of a second is over by the next render: nothing was stopped.
    let (playing, _) = lab.tool("audition", json!({"file": file, "seconds": 0.2})).await;
    assert_eq!(playing["playing"], true, "{playing}");
    tokio::time::sleep(std::time::Duration::from_millis(400)).await;
    let (job, _) = lab.tool("render", render).await;
    let done = lab.wait(&job).await;
    assert!(!done.to_string().contains(stopped), "{done}");
}

#[tokio::test]
async fn a_tool_the_daws_list_doesnt_have_is_refused_before_its_bridge_is_asked() {
    async fn refused(lab: &mut Lab, name: &str) -> Value {
        lab.next_id += 1;
        let message = json!({"jsonrpc": "2.0", "id": lab.next_id, "method": "tools/call", "params": {"name": name, "arguments": {}}});
        lab.session.call(message).await
    }
    // Renoise's tool hears nothing it plays and has no cursor in beats or seconds; REAPER has no Renoise actions.
    let script = RenoiseScript::new();
    let mut lab = Lab::renoise(&script).await;
    let (listed, _) = (lab.session.call(json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list"})).await, ());
    let names: Vec<&str> = listed["result"]["tools"].as_array().unwrap().iter().filter_map(|t| t["name"].as_str()).collect();
    assert!(!names.contains(&"hear") && !names.contains(&"cursor_set"), "{names:?}");
    for name in ["hear", "cursor_set"] {
        let answer = refused(&mut lab, name).await;
        assert_eq!(answer["error"]["message"], format!("Unknown tool: {name}"), "{answer}");
    }
    assert!(
        script.asked().iter().all(|(method, _)| !matches!(method.as_str(), "capture_start" | "transport_play")),
        "the bridge wasn't asked"
    );
    let script = ReaperScript::new(3, 0);
    let mut lab = Lab::reaper(&script).await;
    for name in ["audition_note", "select", "instrument_save", "device_editor"] {
        let answer = refused(&mut lab, name).await;
        assert_eq!(answer["error"]["message"], format!("Unknown tool: {name}"), "{answer}");
    }
    assert!(script.asked().iter().all(|(method, _)| !matches!(method.as_str(), "audition_note" | "select" | "instrument_save")));
}

#[tokio::test]
async fn jobs_nobody_followed_are_asked_about_before_a_new_one_is_refused_and_a_forgotten_one_says_so() {
    let script = ReaperScript::new(3, 0);
    let mut lab = Lab::reaper(&script).await;
    let render = json!({"sources": [{"id": "mix", "what": "master"}], "range": two_bars(), "epoch": "sim-1"});
    // As many renders as the host keeps, none followed to its end (a client that lost its connection, say).
    for _ in 0..daw_mcp_server::listening::MAX_JOBS {
        let (job, is_error) = lab.tool("render", render.clone()).await;
        assert!(!is_error, "{job}");
        script.listening().finish_jobs();
    }
    // The next one: the host learns from the bridge that they ended, so it starts.
    let (job, is_error) = lab.tool("render", render).await;
    assert!(!is_error, "{job}");
    assert_eq!(lab.wait(&job).await["state"], "done");
    // A job the host doesn't keep has its own code, not a malformed request's.
    for tool in ["listen_job", "listen_cancel"] {
        let (answer, is_error) = lab.tool(tool, json!({"jobId": "j999zzzz"})).await;
        assert!(is_error && answer["code"] == "unknown-job", "{tool}: {answer}");
    }
}

#[tokio::test]
async fn at_the_end_of_input_a_running_render_is_stopped_without_waiting_for_requests_in_flight() {
    let script = ReaperScript::new(3, 0);
    script.listening().set_steps(1_000);
    let mut lab = Lab::reaper(&script).await;
    let render = json!({"sources": [{"id": "mix", "what": "master"}], "range": two_bars(), "epoch": "sim-1"});
    let (job, _) = lab.tool("render", render).await;
    // A request in flight that would wait 10 s for the render to end.
    let wait = json!({"jsonrpc": "2.0", "id": 900, "method": "tools/call", "params": {"name": "listen_job", "arguments": {"jobId": job["jobId"], "waitMs": 10_000}}});
    lab.session.send(wait).await;
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    let ended = std::time::Instant::now();
    lab.session.finish().await;
    assert!(ended.elapsed() < std::time::Duration::from_secs(5), "the host ended in {:?}", ended.elapsed());
    assert_eq!(script.listening().restored(), 1, "the render was stopped and put back");
}

#[tokio::test]
async fn each_daws_tools_say_what_they_write_in_its_own_words() {
    async fn listed(lab: &mut Lab) -> Vec<Value> {
        lab.session.call(json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list"})).await["result"]["tools"].as_array().unwrap().clone()
    }
    fn tool<'a>(tools: &'a [Value], name: &str) -> &'a Value {
        tools.iter().find(|tool| tool["name"] == name).unwrap_or_else(|| panic!("no {name}"))
    }
    let script = ReaperScript::new(3, 0);
    let mut lab = Lab::reaper(&script).await;
    let tools = listed(&mut lab).await;
    // A save that may write over a file on request is destructive; a render writes files and changes render settings.
    assert_eq!(tool(&tools, "fx_chain_save")["annotations"]["destructiveHint"], true);
    for name in ["render", "audio_file"] {
        assert_eq!(tool(&tools, name)["annotations"]["readOnlyHint"], false, "{name}");
    }
    let play = &tool(&tools, "transport_play")["inputSchema"]["properties"];
    assert!(play.get("sequence").is_none() && play.get("line").is_none(), "REAPER plays from where it is: {play}");
    let script = RenoiseScript::new();
    let mut lab = Lab::renoise(&script).await;
    let tools = listed(&mut lab).await;
    for name in ["instrument_save", "preset_save"] {
        assert_eq!(tool(&tools, name)["annotations"]["destructiveHint"], true, "{name}");
    }
    for name in ["render", "audio_file"] {
        assert_eq!(tool(&tools, name)["annotations"]["readOnlyHint"], false, "{name}");
    }
    let said = [
        tool(&tools, "read").to_string(),
        tool(&tools, "status").to_string(),
        tool(&tools, "snapshot").to_string(),
        tool(&tools, "focus_body")["description"].to_string(),
    ]
    .join(" ");
    for reapers in ["open project", "Renoise project", "FX", "track's items", "the take", "items or sends"] {
        assert!(!said.contains(reapers), "{reapers:?} in Renoise's words: {said}");
    }
    assert!(tool(&tools, "transport_play")["inputSchema"]["properties"].get("sequence").is_some());
    // A field and its …Ref naming different things: one of the two.
    let (answer, is_error) = lab.tool("select", json!({"track": "1:track:1", "trackRef": "1:track:2"})).await;
    assert!(is_error && answer["error"].as_str().unwrap().contains("pass one of the two"), "{answer}");
    assert!(!script.asked().iter().any(|(method, _)| method == "select"), "the tool wasn't asked");
}

#[tokio::test]
async fn an_items_audio_is_its_source_file_and_a_midi_item_says_it_has_none() {
    let script = ReaperScript::new(3, 0);
    script.listening().set_silent_ref("r:item:{I-T0002}");
    let mut lab = Lab::reaper(&script).await;
    let (audio, is_error) = lab.tool("audio_file", json!({"ref": "r:item:{I-T0001}"})).await;
    assert!(!is_error, "{audio}");
    assert_eq!(audio["kind"], "source");
    assert!(PathBuf::from(audio["file"].as_str().unwrap()).is_file());
    let (midi, is_error) = lab.tool("audio_file", json!({"ref": "r:item:{I-T0002}"})).await;
    assert!(is_error && midi["error"].as_str().unwrap().starts_with("That's a MIDI item"), "{midi}");
}

#[tokio::test]
async fn scratch_copies_are_made_set_heard_and_dropped() {
    let script = ReaperScript::new(3, 0);
    let mut lab = Lab::reaper(&script).await;
    let (made, is_error) = lab
        .tool("scratch", json!({"action": "make", "ref": "r:track:{T0001}", "count": 3, "label": "Kumi: search", "epoch": "sim-1"}))
        .await;
    assert!(!is_error, "{made}");
    let copies: Vec<String> = made["copies"].as_array().unwrap().iter().map(|copy| copy["ref"].as_str().unwrap().to_string()).collect();
    assert_eq!(copies.len(), 3);
    assert!(made["copies"][0]["name"].as_str().unwrap().starts_with("Kumi · try "));
    let values: Vec<Value> =
        copies.iter().enumerate().map(|(n, copy)| json!({"ref": copy, "device": 0, "param": 2, "normalized": n as f64 / 2.})).collect();
    let (set, is_error) = lab.tool("scratch", json!({"action": "set", "scratchId": made["scratchId"], "values": values})).await;
    assert!(!is_error && set["set"] == 3, "{set}");
    let sources: Vec<Value> =
        copies.iter().enumerate().map(|(n, copy)| json!({"id": format!("try{n}"), "what": "track", "ref": copy})).collect();
    let (job, is_error) = lab.tool("render", json!({"sources": sources, "range": two_bars(), "epoch": "sim-1"})).await;
    assert!(!is_error, "{job}");
    assert_eq!(lab.wait(&job).await["files"].as_array().unwrap().len(), 3);
    let (dropped, _) = lab.tool("scratch", json!({"action": "drop", "scratchId": made["scratchId"]})).await;
    assert_eq!((dropped["dropped"].as_u64(), dropped["left"].as_u64()), (Some(3), Some(0)));
    assert_eq!(script.listening().scratch_copies(), 0);
}

#[tokio::test]
async fn refs_from_another_project_in_the_tab_are_stale() {
    let script = ReaperScript::new(3, 0);
    let mut lab = Lab::reaper(&script).await;
    script.switch_tab();
    let (refused, is_error) = lab
        .tool("render", json!({"sources": [{"id": "a", "what": "track", "ref": "r:track:{T0001}"}], "range": two_bars(), "epoch": "sim-1"}))
        .await;
    assert!(is_error && refused["code"] == "stale", "{refused}");
}

#[tokio::test]
async fn renoise_renders_a_track_soloed_through_the_master_and_never_hears_what_it_plays() {
    let script = RenoiseScript::new();
    let mut lab = Lab::renoise(&script).await;
    let (listed, _) = lab.tool_list().await;
    assert!(listed.iter().all(|name| name != "hear"), "Renoise offers no live hearing: {listed:?}");
    let (job, is_error) = lab
        .tool("render", json!({"sources": [{"id": "kick", "what": "track", "ref": "1:track:1"}], "range": two_bars(), "epoch": "1"}))
        .await;
    assert!(!is_error, "{job}");
    let done = lab.wait(&job).await;
    assert_eq!((done["files"][0]["stage"].as_str(), done["files"][0]["throughMaster"].as_bool()), (Some("post-fader"), Some(true)));
    let (refused, is_error) =
        lab.tool("render", json!({"sources": [{"id": "pre", "what": "master", "beforeChain": true}], "range": two_bars()})).await;
    assert!(is_error && refused["error"].as_str().unwrap().contains("after its Master track's devices"), "{refused}");
    let (sample, is_error) = lab.tool("audio_file", json!({"ref": "1:sample:1.1"})).await;
    assert!(!is_error && sample["kind"] == "exported", "{sample}");
    // A pattern's audio is rendered: a job, followed like any render.
    let (pattern, is_error) = lab.tool("audio_file", json!({"ref": "1:pattern:1"})).await;
    assert!(!is_error && pattern["kind"] == "render", "{pattern}");
    assert_eq!(lab.wait(&pattern).await["state"], "done");
}

#[tokio::test]
async fn the_renoise_demo_says_it_cant_render() {
    let script = RenoiseScript::new();
    script.listening().set_demo(true);
    let mut lab = Lab::renoise(&script).await;
    let (status, _) = lab.tool("status", json!({})).await;
    assert_eq!(status["listening"]["render"], false, "{status}");
    let (refused, is_error) = lab.tool("render", json!({"sources": [{"id": "mix", "what": "master"}], "range": two_bars()})).await;
    assert!(is_error && refused["error"].as_str().unwrap().starts_with("The Renoise demo can't render"), "{refused}");
}

#[tokio::test]
async fn an_older_bridge_that_cant_listen_says_to_update_it() {
    let lab_home = tempfile::tempdir().unwrap();
    let paths = BridgePaths::new(lab_home.path(), Daw::Reaper);
    let secret = ensure_secret(&paths.secret()).unwrap();
    let mut config = SimulatorConfig::reaper(secret, "/lab/REAPER");
    config.discovery_dir = Some(paths.discovery());
    let _simulator = Simulator::start(config).await.unwrap();
    let mut session = Session::start(Daw::Reaper, lab_home.path());
    let (refused, is_error) =
        session.tool_full(1, "render", json!({"sources": [{"id": "mix", "what": "master"}], "range": two_bars()}), false).await;
    assert!(is_error && refused["error"].as_str().unwrap().contains("update Kumi's bridge"), "{refused}");
}

#[tokio::test]
async fn a_job_still_running_when_the_host_goes_is_stopped_and_put_back() {
    let script = ReaperScript::new(3, 0);
    script.listening().set_steps(50);
    let mut lab = Lab::reaper(&script).await;
    let (job, _) = lab.tool("render", json!({"sources": [{"id": "mix", "what": "master"}], "range": two_bars()})).await;
    assert_eq!(job["state"], "running");
    let Lab { session, _simulator, home, .. } = lab;
    session.finish().await;
    assert_eq!(script.listening().restored(), 1, "the host stopped it as it went");
    drop((_simulator, home));
}

impl Lab {
    /// The names of the tools the host lists.
    async fn tool_list(&mut self) -> (Vec<String>, Value) {
        self.next_id += 1;
        let answer = self.session.call(json!({"jsonrpc":"2.0","id":self.next_id,"method":"tools/list"})).await;
        let names = answer["result"]["tools"].as_array().unwrap().iter().map(|tool| tool["name"].as_str().unwrap().to_string()).collect();
        (names, answer)
    }
}
