//! An instrument's macro mappings added and removed through its file (M4 n-devices), through the host's tools against a
//! simulated Renoise tool that does what the real one does: saves the instrument into Kumi's work folder
//! (`instrument_export`), loads the file the host packed beside it while the instrument is as the host read it, and
//! loads the host's restore file the same way.
#[path = "../common/mod.rs"]
mod common;

use common::Session;
use daw_bridge::files::ensure_secret;
use daw_bridge::paths::BridgePaths;
use daw_bridge::Daw;
use daw_mcp_server::renoise::presets::print_of;
use daw_mcp_server::renoise::zip::Archive;
use daw_mcp_server::simulator::{Simulator, SimulatorConfig};
use serde_json::{json, Value};
use std::io::Write;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

const INSTRUMENT: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<RenoiseInstrument doc_version=\"33\">\n  <Name>Lead</Name>\n  <GlobalProperties>\n    <Macro0>\n      <Value>50</Value>\n      <Visualization>Device only</Visualization>\n      <Name>Tone</Name>\n      <Mappings>\n        <Mapping>\n          <DestChainType>ModulationSet</DestChainType>\n          <DestChainIndex>0</DestChainIndex>\n          <DestDeviceIndex>1</DestDeviceIndex>\n          <DestParameterIndex>1</DestParameterIndex>\n          <Min>0.0</Min>\n          <Max>0.396850258</Max>\n          <Scaling>Linear</Scaling>\n        </Mapping>\n      </Mappings>\n    </Macro0>\n    <Macro1>\n      <Value>50</Value>\n      <Visualization>Device only</Visualization>\n      <Name>Macro 2</Name>\n    </Macro1>\n  </GlobalProperties>\n</RenoiseInstrument>\n";
/// A sample's bytes in the file, which every edit must carry over as they were.
const SAMPLE: &[u8] = b"fLaC not really, but bytes the host must never touch";

fn chains() -> Value {
    json!({
        "modulation": [{"name": "Volume", "devices": [{"name": "AHDSR", "params": ["Attack", "Hold", "Decay", "Sustain", "Release"]}]}],
        "fx": [{"name": "FX", "devices": [{"name": "Mixer", "params": ["Panning", "Volume", "Width"]}, {"name": "Gainer", "params": ["Gain", "Panning"]}]}],
    })
}

/// The macros as Renoise's API reads them from the instrument: each mapping's parameter name, range and scaling.
fn api_macros(xml: &str) -> Value {
    let document = daw_mcp_server::renoise::xml::parse(xml).unwrap();
    let global = document.root.child("GlobalProperties").unwrap();
    let chains = chains();
    let macros: Vec<Value> = (0..2)
        .map(|k| {
            let macro_ = global.child(&format!("Macro{k}")).unwrap();
            let mappings: Vec<Value> = macro_
                .child("Mappings")
                .map(|list| list.elements().collect::<Vec<_>>())
                .unwrap_or_default()
                .into_iter()
                .map(|m| {
                    let n = |name: &str| m.child_text(name).unwrap().parse::<f64>().unwrap();
                    let (list, device) = if m.child_text("DestChainType").unwrap() == "SampleDSP" {
                        ("fx", n("DestDeviceIndex") as usize)
                    } else {
                        ("modulation", n("DestDeviceIndex") as usize - 1)
                    };
                    let param = chains[list][n("DestChainIndex") as usize]["devices"][device]["params"]
                        [n("DestParameterIndex") as usize - 1]
                        .clone();
                    let scaling = match m.child_text("Scaling").unwrap().as_str() {
                        "Exp Fast" => "exp-fast",
                        _ => "linear",
                    };
                    json!({"param": param, "min": n("Min"), "max": n("Max"), "scaling": scaling})
                })
                .collect();
            json!({"name": macro_.child_text("Name"), "mappings": mappings})
        })
        .collect();
    Value::Array(macros)
}

/// A zip with these entries, stored as they are.
fn zip_of(entries: &[(&str, &[u8])]) -> Vec<u8> {
    let mut out = vec![];
    let mut central = vec![];
    for (name, contents) in entries {
        let mut crc = flate2::Crc::new();
        crc.update(contents);
        let offset = out.len() as u32;
        let head = |signature: u32, out: &mut Vec<u8>| {
            out.extend(signature.to_le_bytes());
        };
        head(0x0403_4b50, &mut out);
        out.extend(20u16.to_le_bytes());
        out.extend(0u16.to_le_bytes());
        out.extend(0u16.to_le_bytes());
        out.extend(0u32.to_le_bytes());
        out.extend(crc.sum().to_le_bytes());
        out.extend((contents.len() as u32).to_le_bytes());
        out.extend((contents.len() as u32).to_le_bytes());
        out.extend((name.len() as u16).to_le_bytes());
        out.extend(0u16.to_le_bytes());
        out.extend(name.as_bytes());
        out.extend(*contents);
        head(0x0201_4b50, &mut central);
        central.extend(20u16.to_le_bytes());
        central.extend(20u16.to_le_bytes());
        central.extend(0u16.to_le_bytes());
        central.extend(0u16.to_le_bytes());
        central.extend(0u32.to_le_bytes());
        central.extend(crc.sum().to_le_bytes());
        central.extend((contents.len() as u32).to_le_bytes());
        central.extend((contents.len() as u32).to_le_bytes());
        central.extend((name.len() as u16).to_le_bytes());
        central.extend([0u8; 12]);
        central.extend(offset.to_le_bytes());
        central.extend(name.as_bytes());
    }
    let at = out.len() as u32;
    let size = central.len() as u32;
    out.extend(central);
    out.extend(0x0605_4b50u32.to_le_bytes());
    out.extend([0u8; 4]);
    out.extend((entries.len() as u16).to_le_bytes());
    out.extend((entries.len() as u16).to_le_bytes());
    out.extend(size.to_le_bytes());
    out.extend(at.to_le_bytes());
    out.extend(0u16.to_le_bytes());
    out
}

/// The instrument in slot 1 of a simulated song: its Instrument.xml and the loads the host asked for.
struct Song {
    xml: String,
    loads: Vec<String>,
    work: PathBuf,
    saved: u32,
}

struct Lab {
    _home: tempfile::TempDir,
    session: Session,
    song: Arc<Mutex<Song>>,
    _simulator: Simulator,
    next: u64,
}

fn refusal(code: &str, message: &str) -> Option<Result<Value, (String, String)>> {
    Some(Err((code.into(), message.into())))
}

/// The host's file loaded (the tool's checks): in the work folder, made from the instrument as it is, its sample carried
/// over; the file removed.
fn load(song: &mut Song, file: &str, print: &Value, desc: &Value) -> Result<(), (String, String)> {
    let path = PathBuf::from(file);
    if path.parent() != Some(song.work.as_path()) {
        return Err(("bad-params".into(), "file: an instrument file in Kumi's work folder".into()));
    }
    if print.as_str() != Some(&print_of(&song.xml)) {
        let _ = std::fs::remove_file(&path);
        return Err(("changed".into(), "“Lead”'s macros changed in Renoise since Kumi read it: read it again".into()));
    }
    let bytes = std::fs::read(&path).unwrap();
    std::fs::remove_file(&path).unwrap();
    let archive = Archive::read(&bytes).unwrap();
    let sample = archive.contents(archive.entry("SampleData/Sample01.flac").unwrap()).unwrap();
    assert_eq!(sample, SAMPLE, "the sample carried over as it was");
    let xml = String::from_utf8(archive.contents(archive.entry("Instrument.xml").unwrap()).unwrap()).unwrap();
    song.loads.push(desc.as_str().unwrap().into());
    song.xml = xml;
    Ok(())
}

impl Lab {
    async fn start() -> Lab {
        let home = tempfile::tempdir().unwrap();
        let work = home.path().join("work");
        std::fs::create_dir_all(&work).unwrap();
        let paths = BridgePaths::new(home.path(), Daw::Renoise);
        let secret = ensure_secret(&paths.secret()).unwrap();
        let song = Arc::new(Mutex::new(Song { xml: INSTRUMENT.into(), loads: vec![], work, saved: 0 }));
        let state = song.clone();
        let respond: daw_mcp_server::simulator::Respond = Arc::new(move |method, params| {
            let mut song = state.lock().unwrap();
            let at = json!({"n": 1, "mark": "i1:0", "label": "Lead"});
            let target = json!({"ref": "e1:instrument:1", "kind": "instrument"});
            match method {
                "instrument_export" => {
                    song.saved += 1;
                    let file = song.work.join(format!("instrument-1-{}.xrni", song.saved));
                    let bytes = zip_of(&[("Instrument.xml", song.xml.as_bytes()), ("SampleData/Sample01.flac", SAMPLE)]);
                    std::fs::File::create(&file).unwrap().write_all(&bytes).unwrap();
                    Some(Ok(json!({
                        "epoch": "e1", "layout": 0, "ref": "e1:instrument:1", "file": file.to_string_lossy(), "name": "Lead", "label": "Lead",
                        "frames": 1000, "floats": 0, "print": print_of(&song.xml), "macros": api_macros(&song.xml), "chains": chains(),
                        "at": at, "target": target,
                    })))
                }
                "change_apply" => {
                    let input = &params["input"];
                    if let Err((code, message)) = load(&mut song, input["file"].as_str().unwrap(), &input["print"], &params["desc"]) {
                        return refusal(&code, &message);
                    }
                    Some(Ok(json!({
                        "epoch": "e1", "layout": 0, "applied": {"print": print_of(&song.xml), "edit": input["edit"]}, "material": {"at": at},
                        "target": target, "display": input["display"], "undo": {"desc": params["desc"], "entry": true},
                    })))
                }
                "change_restore" => {
                    let write = &params["material"]["write"];
                    if let Err((code, message)) = load(&mut song, write["file"].as_str().unwrap(), &write["print"], &params["desc"]) {
                        return refusal(&code, &message);
                    }
                    Some(Ok(
                        json!({"epoch": "e1", "layout": 0, "restored": {"print": print_of(&song.xml)}, "display": write["display"], "undo": {"desc": params["desc"], "entry": true}}),
                    ))
                }
                _ => None,
            }
        });
        let mut config = SimulatorConfig::renoise(secret, "/lab/Renoise")
            .with_methods(&["instrument_export", "change_apply", "change_restore"], respond);
        config.discovery_dir = Some(paths.discovery());
        let simulator = Simulator::start(config).await.unwrap();
        let session = Session::start(Daw::Renoise, home.path());
        Lab { _home: home, session, song, _simulator: simulator, next: 100 }
    }

    async fn tool(&mut self, name: &str, arguments: Value) -> (Value, bool) {
        self.next += 1;
        self.session.tool_full(self.next, name, arguments, false).await
    }

    async fn preview(&mut self, input: Value) -> (Value, bool) {
        self.tool("change_preview", json!({"kind": "instrument-macro-map", "input": input, "epoch": "e1", "layout": 0})).await
    }

    async fn apply(&mut self, preview: &Value, key: &str) -> (Value, bool) {
        self.tool(
            "change_apply",
            json!({"transactionId": preview["transactionId"], "confirmation": preview["confirmation"], "idempotencyKey": key}),
        )
        .await
    }

    async fn undo(&mut self, preview: &Value, key: &str) -> (Value, bool) {
        self.tool("change_undo", json!({"transactionId": preview["transactionId"], "confirmation": "undo", "idempotencyKey": key})).await
    }

    fn xml(&self) -> String {
        self.song.lock().unwrap().xml.clone()
    }

    fn loads(&self) -> usize {
        self.song.lock().unwrap().loads.len()
    }

    fn left_in_work(&self) -> usize {
        std::fs::read_dir(&self.song.lock().unwrap().work).unwrap().count()
    }

    fn edit_xml(&self, from: &str, to: &str) {
        let mut song = self.song.lock().unwrap();
        song.xml = song.xml.replacen(from, to, 1);
    }
}

#[tokio::test]
async fn a_mapping_added_through_the_file_loads_in_one_step_and_kumis_undo_loads_the_file_back_as_it_was() {
    let mut lab = Lab::start().await;
    let (preview, is_error) =
        lab.preview(json!({"ref": "e1:instrument:1", "macro": 2, "add": [{"fx": 1, "device": 2, "param": "Gain", "max": 0.5}]})).await;
    assert!(!is_error, "{preview}");
    assert_eq!(preview["display"]["title"], "Lead macro 2: mapped to Gain (Gainer in sample FX chain 1)");
    assert_eq!(preview["prior"]["macro"], 2);
    assert!(preview["prior"].get("xml").is_none(), "the file's text is the host's: {preview}");
    assert_eq!((lab.loads(), lab.left_in_work()), (0, 0), "a preview loads nothing and leaves no file");
    let (applied, is_error) = lab.apply(&preview, "k1").await;
    assert!(!is_error, "{applied}");
    assert_eq!(lab.loads(), 1);
    assert!(lab.xml().contains("<DestChainType>SampleDSP</DestChainType>"));
    assert_eq!(lab.left_in_work(), 0);
    let (undone, is_error) = lab.undo(&preview, "k2").await;
    assert!(!is_error, "{undone}");
    assert_eq!(lab.xml(), INSTRUMENT, "the file as it was, byte for byte");
    assert_eq!((lab.loads(), lab.left_in_work()), (2, 0));
}

#[tokio::test]
async fn a_mapping_change_is_refused_when_the_macro_moved_since_the_preview_and_its_undo_when_it_moved_since_the_apply() {
    let mut lab = Lab::start().await;
    let (preview, _) = lab.preview(json!({"ref": "e1:instrument:1", "macro": 1, "remove": [1]})).await;
    lab.edit_xml("<Max>0.396850258</Max>", "<Max>0.5</Max>");
    let (refused, is_error) = lab.apply(&preview, "k1").await;
    assert!(is_error, "{refused}");
    assert_eq!(refused["code"], "changed", "{refused}");
    assert_eq!((lab.loads(), lab.left_in_work()), (0, 0));
    // Previewed again and applied; the producer maps it again in Renoise; Kumi's undo is refused, nothing loaded.
    lab.edit_xml("<Max>0.5</Max>", "<Max>0.396850258</Max>");
    let (preview, _) = lab.preview(json!({"ref": "e1:instrument:1", "macro": 1, "remove": [1]})).await;
    let (applied, is_error) = lab.apply(&preview, "k2").await;
    assert!(!is_error, "{applied}");
    assert!(!lab.xml().contains("<Mappings>"));
    lab.edit_xml("<Name>Tone</Name>\n", "<Name>Tone</Name>\n      <Mappings>\n        <Mapping>\n          <DestChainType>SampleDSP</DestChainType>\n          <DestChainIndex>0</DestChainIndex>\n          <DestDeviceIndex>1</DestDeviceIndex>\n          <DestParameterIndex>2</DestParameterIndex>\n          <Min>0.0</Min>\n          <Max>1.0</Max>\n          <Scaling>Linear</Scaling>\n        </Mapping>\n      </Mappings>\n");
    let (refused, is_error) = lab.undo(&preview, "k3").await;
    assert!(is_error, "{refused}");
    assert_eq!(refused["code"], "changed", "{refused}");
    assert_eq!((lab.loads(), lab.left_in_work()), (1, 0));
}
