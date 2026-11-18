//! A kit Kumi builds fills its Drum Rack's pads in order from C1, row by row, and drums written by General MIDI's names
//! play those pads: on the bridge's simulated Live, and (an explicit operator fixture, never run in CI) on real Live,
//! open on a disposable Set, with KUMI_DRUM_PADS_LIVE_CONFIG naming Kumi's bridge configuration (bridge-config.json,
//! owner-only) and KUMI_DRUM_PADS_LIVE_REPORT where the evidence goes.
#[path = "../support/live_host.rs"]
mod live_host;
use ableton_mcp_server::{
    bridge::remote_adapter::{RemoteScriptEndpoint, RemoteScriptLiveAdapter},
    host::{McpHost, McpHostOptions},
    lifecycle::delivery::read_secret_file,
    live::{AsyncLiveAdapter, DeterministicLiveSimulator},
};
use futures::FutureExt;
use kumi_agent::{mcp::client::McpEndpoint, Integration};
use kumi_common::abort::Signal;
use kumi_daw_ableton::{integration::Ableton, notes, options::AbletonOptions};
use live_host::HostEndpoint;
use serde_json::{json, Value};
use std::{collections::BTreeSet, path::Path, rc::Rc};

/// A kit's sounds, in the order the producer asked for them.
const SOUNDS: [&str; 16] = [
    "Kick 909",
    "Claps",
    "909hh",
    "VEH3 Open",
    "Snare Tight",
    "Rim Shot",
    "Low Tom",
    "High Tom",
    "Crash",
    "Ride",
    "Shaker",
    "Cowbell",
    "Conga",
    "Clave",
    "Tamb",
    "Triangle",
];

/// A tenth of a second of a tone at `hz`, as a 16-bit mono WAV file at `path`.
fn tone(path: &Path, hz: f64) {
    let rate = 44_100u32;
    let samples: Vec<i16> = (0..rate / 10).map(|i| ((i as f64 * hz * std::f64::consts::TAU / rate as f64).sin() * 8000.) as i16).collect();
    let data = samples.len() as u32 * 2;
    let mut wav = Vec::new();
    wav.extend(b"RIFF");
    wav.extend((36 + data).to_le_bytes());
    wav.extend(b"WAVEfmt ");
    for field in [16u32.to_le_bytes().to_vec(), 1u16.to_le_bytes().to_vec(), 1u16.to_le_bytes().to_vec()] {
        wav.extend(field);
    }
    wav.extend(rate.to_le_bytes());
    wav.extend((rate * 2).to_le_bytes());
    wav.extend(2u16.to_le_bytes());
    wav.extend(16u16.to_le_bytes());
    wav.extend(b"data");
    wav.extend(data.to_le_bytes());
    for sample in samples {
        wav.extend(sample.to_le_bytes());
    }
    std::fs::write(path, wav).unwrap();
}

/// Every pitch in a read's rows.
fn pitches(value: &Value, out: &mut BTreeSet<u64>) {
    match value {
        Value::Object(map) => {
            if let Some(pitch) = map.get("pitch").and_then(Value::as_u64) {
                out.insert(pitch);
            }
            map.values().for_each(|value| pitches(value, out));
        }
        Value::Array(items) => items.iter().for_each(|value| pitches(value, out)),
        _ => {}
    }
}

/// Kumi's Live integration over the bridge's own host on `adapter`, connected as a session's is.
async fn kumi_on(adapter: Rc<dyn AsyncLiveAdapter>) -> Rc<Ableton> {
    let host = Rc::new(McpHost::new(adapter.clone(), McpHostOptions::default()).unwrap());
    let start = json!({"jsonrpc":"2.0","id":"init","method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"drum-pads-fixture","version":"1"}}});
    host.handle_async(&start, None).await.unwrap();
    host.handle_async(&json!({"jsonrpc":"2.0","method":"notifications/initialized"}), None).await.unwrap();
    let endpoint = Rc::new(HostEndpoint(host));
    let mut options = AbletonOptions::new(Rc::new(|_, _| {}));
    options.connect = Some(Rc::new(move |_| {
        let endpoint: Rc<dyn McpEndpoint> = endpoint.clone();
        async move { Ok(endpoint) }.boxed_local()
    }));
    let integration = Ableton::new(options);
    let connection = integration.connection.clone();
    connection.start(Signal::new()).await.unwrap();
    connection.tools().unwrap().refresh(Signal::new()).await.unwrap();
    connection.available.set(true);
    connection.epoch.set(Some(adapter.status().unwrap().epoch.unwrap() as f64));
    integration
}

/// Which Live a run is on: real Live names a pad after the sample loaded onto it; the simulated one keeps "Pad 1" to
/// "Pad 16", so there a kit's pads can't be found by what they hold.
#[derive(Clone, Copy, PartialEq)]
enum Live {
    Real,
    Simulated,
}

/// Kits of each of `counts` drums, built as the instructions say (a track, a Drum Rack, its samples with no note), each
/// checked and its pads undone: what each left on its pads. On real Live, the kit of 4 also gets a clip written by
/// General MIDI's names of its drums.
async fn kits(integration: &Ableton, counts: &[usize], live: Live) -> Vec<Value> {
    let connection = integration.connection.clone();
    let folder = tempfile::tempdir().unwrap();
    let mut report = vec![];
    for &count in counts {
        let samples: Vec<String> = SOUNDS[..count]
            .iter()
            .enumerate()
            .map(|(at, name)| {
                let path = folder.path().join(format!("{name}.wav"));
                tone(&path, 110. * (at + 1) as f64);
                path.display().to_string()
            })
            .collect();
        // (the Set looked at, as a turn starts)
        integration.observe(Signal::new(), None).await.unwrap();
        let mut steps = vec![
            json!({"tool":"add_tracks_and_scenes","input":{"tracks":[{"name":format!("Kit of {count}"),"kind":"midi"}],"scenes":[]},"as":"track"}),
            json!({"tool":"load_device","input":{"trackRef":"@track","itemId":"instruments/Drum Rack"},"as":"rack"}),
            json!({"tool":"load_sample_to_pad","input":{"deviceRef":"@rack"},"each":{"sample":samples}}),
        ];
        if count == 4 && live == Live::Real {
            let notation = "kick /16 1|1 x...x...x...x...\nclap /16 1|1 ....x.......x...\nhat /16 1|1 x.x.x.x.x.x.x.x.\nopenhat /16 1|1 .x.x.x.x.x.x.x.x";
            steps.push(json!({"tool":"write_midi_clip","input":{"trackRef":"@track","sceneIndex":0,"name":"Beat","notation":notation},"as":"clip"}));
        }
        let made = integration.mutations.make_changes(json!({"steps":steps}).as_object().unwrap().clone(), Signal::new()).await.unwrap();
        assert!(!made.is_error, "{count}: {}", made.text);
        let named = integration.mutations.names.borrow().clone();
        let pads = notes::drum_pads(&connection, &named["rack"], &Signal::new()).await.expect("the rack's pads read");
        let held: Vec<(u8, String)> = pads.iter().filter(|pad| pad.holds).map(|pad| (pad.note, pad.name.clone())).collect();
        let asked: Vec<(u8, String)> = SOUNDS[..count].iter().enumerate().map(|(at, name)| (36 + at as u8, name.to_string())).collect();
        if live == Live::Real {
            assert_eq!(held, asked, "a kit of {count}: its sounds on its first pads, in the order asked");
        } else {
            let notes = |pads: &[(u8, String)]| pads.iter().map(|(note, _)| *note).collect::<Vec<_>>();
            assert_eq!(notes(&held), notes(&asked), "a kit of {count}: on its first pads");
        }
        let mut row = json!({"drums": count, "pads": held.iter().map(|(note, name)| json!([note, name])).collect::<Vec<_>>()});
        if count == 4 && live == Live::Real {
            let read = notes::read_notes(
                json!({"clipRef": named["clip"], "format": "json"}).as_object().unwrap(),
                &connection,
                None,
                Signal::new(),
            )
            .await;
            assert!(!read.is_error, "{}", read.text);
            let mut played = BTreeSet::new();
            pitches(&serde_json::from_str::<Value>(&read.text).unwrap(), &mut played);
            assert_eq!(played, BTreeSet::from([36, 37, 38, 39]), "kick, clap, hat and openhat play the kit's pads");
            row["clipPitches"] = json!(played);
            // (the clip first: its undo is the last change's)
            assert!(!integration.history.undo("last", Signal::new(), false).await.unwrap().is_error);
        }
        // The pads' change undone: every pad empty again.
        let undone = integration.history.undo("last", Signal::new(), false).await.unwrap();
        assert!(!undone.is_error, "{}", undone.text);
        let pads = notes::drum_pads(&connection, &named["rack"], &Signal::new()).await.expect("the rack's pads read");
        assert!(pads.iter().all(|pad| !pad.holds), "undo clears a kit of {count}");
        row["undoneEmpty"] = json!(true);
        report.push(row);
    }
    report
}

#[tokio::test(flavor = "current_thread")]
async fn on_the_bridges_simulated_live_a_kit_fills_its_pads_in_order() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let integration = kumi_on(Rc::new(DeterministicLiveSimulator::new())).await;
            kits(&integration, &[4, 16], Live::Simulated).await;
            integration.close().await.unwrap();
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
#[ignore = "requires real Live with Kumi's bridge, on a disposable Set"]
async fn on_real_live_a_kit_fills_its_drum_racks_pads_in_order_and_its_drums_play_them() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let path = std::env::var("KUMI_DRUM_PADS_LIVE_CONFIG").expect("Kumi's bridge configuration");
            let config: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
            let bridge = &config["bridge"];
            let secret = read_secret_file(Path::new(bridge["secretFile"].as_str().unwrap())).unwrap();
            let host = bridge["host"].as_str().unwrap_or("127.0.0.1");
            let mut endpoint = RemoteScriptEndpoint::new(host, bridge["port"].as_u64().unwrap() as u16, &secret);
            endpoint.timeout_ms = Some(15000.);
            let adapter = Rc::new(RemoteScriptLiveAdapter::connect(endpoint).await.unwrap());
            let integration = kumi_on(adapter.clone()).await;
            let report = kits(&integration, &[4, 8, 16], Live::Real).await;
            let output = std::env::var("KUMI_DRUM_PADS_LIVE_REPORT").expect("where the evidence goes");
            let evidence = json!({"scope":"Kumi make_changes -> the bridge's host -> the Remote Script in Live","kits":report});
            std::fs::write(output, serde_json::to_vec_pretty(&evidence).unwrap()).unwrap();
            integration.close().await.unwrap();
            adapter.close().await.unwrap();
        })
        .await;
}
