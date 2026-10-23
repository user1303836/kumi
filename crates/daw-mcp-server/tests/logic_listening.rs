//! (M5) Logic Pro heard through the host's tools against the simulated Logic, which answers as the bridge does: `hear`
//! plays a part and captures what Logic outputs into a WAV file in the folder given (the tracks in `solo` alone),
//! `audition` takes such a pass for each candidate, `render` bounces a part offline (a track soloed for it); each
//! answer names the file and what it holds, never its samples, and nothing in the project changes. The host forwards the
//! fields each method takes and nothing else, bounds the call by the part's length at the tempo Logic's focus says, and
//! refuses a part of no length before asking the bridge. What the bridge receives is a golden of the wire
//! (tests/golden/logic-listening-bridge.ndjson), the folder named `<folder>`.
mod common;

use common::{golden, normalized, Session};
use daw_bridge::files::ensure_secret;
use daw_bridge::paths::BridgePaths;
use daw_bridge::Daw;
use daw_mcp_server::host::Host;
use daw_mcp_server::simulator::logic::LogicScript;
use daw_mcp_server::simulator::Simulator;
use serde_json::{json, Value};
use std::sync::Arc;

const BUNDLE: &str = "com.apple.logic10";

struct Lab {
    _home: tempfile::TempDir,
    session: Session,
    script: LogicScript,
    simulator: Simulator,
    next_id: u64,
}

impl Lab {
    async fn start() -> Lab {
        let home = tempfile::tempdir().unwrap();
        let paths = BridgePaths::new(home.path(), Daw::Logic);
        let secret = ensure_secret(&paths.secret()).unwrap();
        let script = LogicScript::new();
        let mut config = script.config(secret, BUNDLE);
        config.discovery_dir = Some(paths.discovery());
        let simulator = Simulator::start(config).await.unwrap();
        let session = Session::start_with(Arc::new(Host::new(Daw::Logic, home.path())), home.path());
        Lab { _home: home, session, script, simulator, next_id: 100 }
    }

    async fn tool(&mut self, name: &str, arguments: Value) -> (Value, bool) {
        self.next_id += 1;
        let id = self.next_id;
        self.session.tool_full(id, name, arguments, false).await
    }
}

/// A WAV file's rate, channels and length in seconds, from its format chunk and the size of its data chunk.
fn wav(path: &str) -> (u32, u16, f64) {
    let bytes = std::fs::read(path).unwrap();
    assert_eq!((&bytes[..4], &bytes[8..12]), (&b"RIFF"[..], &b"WAVE"[..]));
    let channels = u16::from_le_bytes(bytes[22..24].try_into().unwrap());
    let rate = u32::from_le_bytes(bytes[24..28].try_into().unwrap());
    let block = u16::from_le_bytes(bytes[32..34].try_into().unwrap());
    let data = u32::from_le_bytes(bytes[40..44].try_into().unwrap());
    (rate, channels, f64::from(data) / f64::from(block) / f64::from(rate))
}

#[tokio::test]
async fn a_part_is_heard_auditioned_and_rendered_into_files_and_nothing_in_the_project_changes() {
    let mut lab = Lab::start().await;
    let files = tempfile::tempdir().unwrap();
    let folder = files.path().to_string_lossy().into_owned();

    // Bars 1 to 5 of the whole mix: 16 beats at 160 BPM are 6 s, captured at the output's 44.1 kHz, cut to the part. What
    // only the host takes (the instance pick) and what no method takes (seconds) never reach the bridge.
    let (heard, is_error) =
        lab.tool("hear", json!({"epoch": "1", "fromBeats": 0, "beats": 16, "folder": folder, "resource": BUNDLE, "seconds": 8})).await;
    assert!(!is_error, "{heard}");
    let file = heard["file"].as_str().unwrap().to_owned();
    assert_eq!(file, format!("{folder}/kumi-hear-1.wav"), "{heard}");
    assert_eq!(
        (heard["seconds"].as_f64(), heard["sampleRate"].as_u64(), heard["channels"].as_u64(), heard["lead"].as_f64()),
        (Some(6.0), Some(44_100), Some(2), Some(0.0))
    );
    assert_eq!(
        (heard["fromBeats"].as_f64(), heard["beats"].as_f64(), heard["mute"].clone(), heard["metronome"].clone(), heard["epoch"].clone()),
        (Some(0.0), Some(16.0), json!(false), json!("off for the pass"), json!("1"))
    );
    assert!(heard["peak"].as_f64().is_some_and(|peak| peak > 0.1 && peak <= 0.2) && heard["rms"].as_f64().is_some(), "{heard}");
    assert_eq!(wav(&file), (44_100, 2, 6.0), "the file holds what the answer says");
    let asked = lab.script.asked();
    let (method, params) = asked.last().unwrap();
    assert_eq!((method.as_str(), params), ("hear", &json!({"epoch": "1", "fromBeats": 0, "beats": 16, "folder": folder})));
    assert_eq!(asked[asked.len() - 2].0, "focus", "the tempo the call is bounded by");

    // Keys alone, soloed for the pass and put back; Logic silent while it captures.
    let (alone, _) =
        lab.tool("hear", json!({"epoch": "1", "fromBeats": 4, "beats": 8, "folder": folder, "mute": true, "solo": ["1:track:2"]})).await;
    assert_eq!(alone["seconds"].as_f64(), Some(3.0), "{alone}");
    assert!(alone["rms"].as_f64() < heard["rms"].as_f64(), "one tone of two: {alone}");
    assert_eq!(lab.script.track(2)["solo"], false);

    // Tracks 1 and 2 auditioned: a pass each, in their order, each its candidate and name.
    let (audition, is_error) = lab
        .tool(
            "audition",
            json!({"epoch": "1", "fromBeats": 0, "beats": 16, "folder": folder, "candidates": [{"ref": "1:track:1"}, {"ref": "1:track:2"}]}),
        )
        .await;
    assert!(!is_error, "{audition}");
    let takes = audition["candidates"].as_array().unwrap();
    assert_eq!(
        takes.iter().map(|take| (take["ref"].as_str().unwrap(), take["name"].as_str().unwrap())).collect::<Vec<_>>(),
        [("1:track:1", "Vocal"), ("1:track:2", "Keys")]
    );
    for take in takes {
        assert_eq!(wav(take["file"].as_str().unwrap()), (44_100, 2, 6.0), "{take}");
    }

    // Keys bounced offline, soloed: 32-bit float at the project's 44.1 kHz, its note saying what it passed through.
    let (render, is_error) =
        lab.tool("render", json!({"epoch": "1", "ref": "1:track:2", "fromBeats": 0, "beats": 16, "folder": folder})).await;
    assert!(!is_error, "{render}");
    assert_eq!(
        (render["seconds"].as_f64(), render["sampleRate"].as_u64(), render["channels"].as_u64(), render["bits"].as_u64()),
        (Some(6.0), Some(44_100), Some(2), Some(32))
    );
    assert!(render["note"].as_str().is_some_and(|note| note.contains("the Stereo Out's plug-ins")), "{render}");
    assert_eq!(wav(render["file"].as_str().unwrap()), (44_100, 2, 6.0));
    assert!(lab.script.undo_titles().is_empty() && !lab.script.playing(), "no step of Logic's undo, and Logic stopped as it was");

    // Refused: a part of no length by the host itself (nothing sent), an earlier epoch and a folder that isn't there by
    // the bridge, and every pass while Logic plays for the producer.
    let before = lab.script.asked().len();
    let (none, is_error) = lab.tool("hear", json!({"epoch": "1", "fromBeats": 0, "folder": folder})).await;
    assert!(is_error && none["code"] == "bad-params" && lab.script.asked().len() == before, "{none}");
    let (stale, _) = lab.tool("hear", json!({"epoch": "0", "fromBeats": 0, "beats": 4, "folder": folder})).await;
    assert_eq!(stale["code"], "stale", "{stale}");
    let (missing, _) = lab.tool("render", json!({"epoch": "1", "fromBeats": 0, "beats": 4, "folder": format!("{folder}/not there")})).await;
    assert!(missing["code"] == "bad-params" && missing["error"].as_str().unwrap().contains("Kumi makes no folders"), "{missing}");
    lab.tool("transport_play", json!({})).await;
    for (tool, arguments) in [
        ("hear", json!({"epoch": "1", "fromBeats": 0, "beats": 4, "folder": folder})),
        ("audition", json!({"epoch": "1", "fromBeats": 0, "beats": 4, "folder": folder, "candidates": [{"mix": true}]})),
        ("render", json!({"epoch": "1", "fromBeats": 0, "beats": 4, "folder": folder})),
    ] {
        let (busy, is_error) = lab.tool(tool, arguments).await;
        assert!(is_error && busy["code"] == "busy" && busy["busy"] == true, "{tool}: {busy}");
    }
    lab.tool("transport_stop", json!({})).await;
    golden("logic-listening-bridge.ndjson", &normalized(&lab.simulator.transcript()).replace(&folder, "<folder>"));
    lab.simulator.stop();
}
