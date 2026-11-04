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
    /// The tool can't remove the host's file, as Lua's `os.remove` can't on Windows in a folder whose name isn't in the
    /// system's code page.
    keeps_files: bool,
    /// Renoise can't load the host's file.
    refuses: bool,
}

struct Lab {
    _home: tempfile::TempDir,
    session: Session,
    song: Arc<Mutex<Song>>,
    config: SimulatorConfig,
    simulator: Simulator,
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
    if song.refuses {
        if !song.keeps_files {
            let _ = std::fs::remove_file(&path);
        }
        return Err(("failed".into(), "Renoise couldn't load the instrument file Kumi's host made".into()));
    }
    if print.as_str() != Some(&print_of(&song.xml)) {
        if !song.keeps_files {
            let _ = std::fs::remove_file(&path);
        }
        return Err(("changed".into(), "“Lead”'s macros changed in Renoise since Kumi read it: read it again".into()));
    }
    let bytes = std::fs::read(&path).unwrap();
    if !song.keeps_files {
        std::fs::remove_file(&path).unwrap();
    }
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
        Lab::start_with(|_| {}).await
    }

    /// `start`, with the simulated Renoise's configuration changed first.
    async fn start_with(configure: impl FnOnce(&mut SimulatorConfig)) -> Lab {
        let home = tempfile::tempdir().unwrap();
        let work = home.path().join("work");
        std::fs::create_dir_all(&work).unwrap();
        let paths = BridgePaths::new(home.path(), Daw::Renoise);
        let secret = ensure_secret(&paths.secret()).unwrap();
        let song = Arc::new(Mutex::new(Song { xml: INSTRUMENT.into(), loads: vec![], work, saved: 0, keeps_files: false, refuses: false }));
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
        configure(&mut config);
        let simulator = Simulator::start(config.clone()).await.unwrap();
        let session = Session::start(Daw::Renoise, home.path());
        Lab { _home: home, session, song, config, simulator, next: 100 }
    }

    /// Renoise quit and opened again in the same folder, its song as it was: a new run of the tool, started later.
    async fn reopen(&mut self) {
        self.simulator.stop();
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        self.simulator = Simulator::start(self.config.clone()).await.unwrap();
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

/// #399 (N-N9): the host removes the instrument file it wrote once the tool has answered, loaded or refused, even when the
/// tool couldn't (Lua's `os.remove` on Windows, in a folder whose name isn't in the system's code page).
#[tokio::test]
async fn the_host_removes_its_instrument_files_when_the_tool_cant() {
    let mut lab = Lab::start().await;
    lab.song.lock().unwrap().keeps_files = true;
    let (preview, _) = lab.preview(json!({"ref": "e1:instrument:1", "macro": 2, "add": [{"fx": 1, "device": 2, "param": "Gain"}]})).await;
    let (applied, is_error) = lab.apply(&preview, "k1").await;
    assert!(!is_error, "{applied}");
    assert_eq!((lab.loads(), lab.left_in_work()), (1, 0), "the host's file is gone once the tool loaded it");
    let (undone, is_error) = lab.undo(&preview, "k2").await;
    assert!(!is_error, "{undone}");
    assert_eq!((lab.loads(), lab.left_in_work()), (2, 0), "and the restore's");
    // Refused by the tool after the host made the file: the file goes too.
    let (preview, _) = lab.preview(json!({"ref": "e1:instrument:1", "macro": 1, "remove": [1]})).await;
    lab.song.lock().unwrap().refuses = true;
    let (refused, is_error) = lab.apply(&preview, "k3").await;
    assert!(is_error && refused["code"] == "failed", "{refused}");
    assert_eq!((lab.loads(), lab.left_in_work()), (2, 0), "{refused}");
}

/// #399 (N-N9): instrument files left in Kumi's work folder an hour ago or more are removed as Renoise's host starts;
/// newer ones (a change in flight in another Kumi) and anything else stay, and REAPER's host leaves the folder alone.
#[test]
fn renoises_host_sweeps_old_instrument_files_from_kumis_work_folder() {
    let home = tempfile::tempdir().unwrap();
    let work = BridgePaths::new(home.path(), Daw::Renoise).root.join("work");
    std::fs::create_dir_all(&work).unwrap();
    let two_hours_ago = std::time::SystemTime::now() - std::time::Duration::from_secs(7200);
    let made = |name: &str, at: Option<std::time::SystemTime>| {
        let file = std::fs::File::create(work.join(name)).unwrap();
        if let Some(at) = at {
            file.set_modified(at).unwrap();
        }
    };
    made("instrument-1700000000-1.xrni", Some(two_hours_ago));
    made("instrument-1700000000-1.kumi.xrni", Some(two_hours_ago));
    made("instrument-1700007000-2.kumi.xrni", None);
    made("notes.txt", Some(two_hours_ago));
    let left = || {
        let mut names: Vec<String> =
            std::fs::read_dir(&work).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
        names.sort();
        names
    };
    let all = left();
    let _reaper = daw_mcp_server::host::Host::new(Daw::Reaper, home.path());
    assert_eq!(left(), all, "REAPER's host leaves Renoise's work folder alone");
    let _renoise = daw_mcp_server::host::Host::new(Daw::Renoise, home.path());
    assert_eq!(left(), ["instrument-1700007000-2.kumi.xrni", "notes.txt"]);
}

/// #399 (N-S10): Renoise on Windows can't say its pid (0), so a change previewed in it is bound to when its bridge
/// started: the Renoise opened again in the same folder is another one, and the change isn't made there.
#[tokio::test]
async fn a_change_previewed_before_renoise_without_a_pid_was_opened_again_isnt_made_in_the_new_one() {
    let mut lab = Lab::start_with(|config| config.identity.pid = 0).await;
    let (preview, is_error) = lab.preview(json!({"ref": "e1:instrument:1", "macro": 1, "remove": [1]})).await;
    assert!(!is_error, "{preview}");
    lab.reopen().await;
    let (refused, is_error) = lab.apply(&preview, "k1").await;
    assert!(is_error && refused["code"] == "stale", "{refused}");
    assert_eq!(refused["error"], "this change was made in Renoise before it was opened again (/lab/Renoise): preview it again");
    assert_eq!(lab.loads(), 0, "nothing loaded in the Renoise opened again");
    // Previewed in the Renoise running now, it's made there.
    let (preview, _) = lab.preview(json!({"ref": "e1:instrument:1", "macro": 1, "remove": [1]})).await;
    let (applied, is_error) = lab.apply(&preview, "k2").await;
    assert!(!is_error, "{applied}");
    assert_eq!(lab.loads(), 1);
}
