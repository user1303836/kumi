//! Opt-in check on real Live of variations (#401): numbered takes of a Session MIDI clip made, kept, changed and dropped as
//! the tool does them for a producer. It adds a MIDI track with a clip in the first scene (its takes go in the empty slots
//! below it) and one in the last (its takes add scenes), runs the tool on each, then deletes the scenes and the track it
//! added. No model and no sign-in; Kumi's sets of takes are kept in a scratch folder, not ~/.kumi. It works in the open Set
//! (a new one: at least 5 scenes, none with clips): name it, and use a disposable one.
//!   cargo build -p ableton-mcp-server --bins
//!   cargo run -p kumi --example variations_live -- --set "<Set name>"
use futures::FutureExt;
use kumi_agent::{mcp::client, Integration, Observation};
use kumi_common::{
    abort::{self, Signal},
    js::string::{head, trim},
};
use kumi_daw_ableton::app::folders::find_bridge_config;
use kumi_daw_ableton::{connection::Connect, create_ableton_integration, AbletonOptions, BRIDGE_TOOLS};
use kumi_types::{
    contracts::{JsonObject, KernelTool},
    errors::RuntimeError,
    system::Env,
};
use serde_json::{json, Value};
use std::{
    cell::RefCell,
    path::{Path, PathBuf},
    rc::Rc,
};

/// The track the check adds, and its two clips.
const TRACK: &str = "Kumi variations check";

fn main() {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let value = |flag: &str| argv.iter().position(|arg| arg == flag).and_then(|at| argv.get(at + 1)).map(|value| trim(value).to_owned());
    let Some(wanted) = value("--set").filter(|name| !name.is_empty()) else {
        eprintln!("Name the open Set, as Live shows it: cargo run -p kumi --example variations_live -- --set \"<Set name>\"");
        eprintln!("It adds a track and scenes to the Set and deletes them after, so use a disposable Set.");
        std::process::exit(2);
    };
    let env: Env = std::env::vars().collect();
    let Some(bridge_config) = find_bridge_config(&env) else {
        eprintln!("The Ableton bridge isn't installed; run Kumi's bridge setup first.");
        std::process::exit(2);
    };
    let bridge = bridge_program();
    if !bridge.is_file() {
        eprintln!("The Ableton bridge isn't built ({}); build it first: cargo build -p ableton-mcp-server --bins", bridge.display());
        std::process::exit(2);
    }
    let scratch = tempfile::Builder::new().prefix("kumi-variations-").tempdir().expect("a scratch folder");
    std::env::set_var("KUMI_HOME", scratch.path());
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().expect("a runtime");
    let local = tokio::task::LocalSet::new();
    std::process::exit(local.block_on(&runtime, variations_live(wanted, bridge_config, bridge)));
}

fn bridge_program() -> PathBuf {
    let name = if cfg!(windows) { "ableton-mcp-server.exe" } else { "ableton-mcp-server" };
    let mut folder = std::env::current_exe().ok().and_then(|exe| exe.parent().map(Path::to_path_buf)).unwrap_or_default();
    if folder.file_name().is_some_and(|name| name == "examples") {
        folder.pop();
    }
    folder.join(name)
}

fn connect_to(bridge: PathBuf, config: String) -> Connect {
    Rc::new(move |signal| {
        client::connect_mcp(client::Options {
            signal,
            bridge_config: Some(PathBuf::from(&config)),
            entry: Some(bridge.clone()),
            cwd: bridge.parent().map(Path::to_path_buf),
            allow_tools: BRIDGE_TOOLS.clone(),
            ..Default::default()
        })
        .boxed_local()
    })
}

struct Run {
    integration: Rc<dyn Integration>,
    observation: RefCell<Option<Observation>>,
    passed: RefCell<Vec<bool>>,
}

async fn variations_live(wanted: String, bridge_config: String, bridge: PathBuf) -> i32 {
    let mut options = AbletonOptions::new(Rc::new(|_, _| {}));
    options.bridge_config = Some(bridge_config.clone());
    options.connect = Some(connect_to(bridge, bridge_config));
    let run = Run { integration: create_ableton_integration(options), observation: RefCell::default(), passed: RefCell::default() };
    let checked = run.check(&wanted).await;
    // Whatever happened, what the check added goes.
    let cleaned = run.clean_up().await;
    let code = match checked.and(cleaned) {
        Ok(()) => {
            let passed = run.passed.borrow();
            let good = passed.iter().filter(|ok| **ok).count();
            println!("\n{good} of {} passed.", passed.len());
            i32::from(good != passed.len())
        }
        Err(error) => {
            eprintln!("variations-live: {}", head(&error.message(), 400));
            1
        }
    };
    let _ = run.integration.close().await;
    code
}

/// A clip's notes: a bar of eighths up and down from `root`.
fn notes(root: i64) -> Value {
    let steps = [0, 3, 7, 10, 12, 10, 7, 3];
    json!(steps
        .iter()
        .enumerate()
        .map(|(k, step)| json!({"pitch": root + step, "start": k as f64 * 0.5, "duration": 0.4, "velocity": 100 - 5 * k as i64}))
        .collect::<Vec<_>>())
}

impl Run {
    fn say(&self, ok: bool, what: &str) {
        self.passed.borrow_mut().push(ok);
        println!("  {}  {what}", if ok { "ok  " } else { "FAIL" });
    }

    async fn check(&self, wanted: &str) -> Result<(), RuntimeError> {
        self.integration.start(signal()).await?;
        self.look().await?;
        let context: Value = serde_json::from_str(&self.observation.borrow().as_ref().map(|seen| seen.context.clone()).unwrap_or_default())
            .unwrap_or(Value::Null);
        let set = context["set"]["name"].as_str().unwrap_or("?").to_owned();
        if set != wanted {
            println!("The open Set is “{set}”, not “{wanted}”; nothing was changed.");
            return Err(RuntimeError::plain("not the Set named"));
        }
        if self.tool("variations").is_none() {
            return Err(RuntimeError::plain("variations isn't offered for this Set (an older bridge?)"));
        }
        let rows = self.rows("scene", json!({"fields": ["name", "isEmpty"]})).await?;
        let scenes = rows.len();
        if scenes < 5 || rows.iter().any(|scene| scene["isEmpty"] != true) {
            return Err(RuntimeError::plain(format!("the check needs a new Set: at least 5 scenes, none with clips ({scenes} scenes)")));
        }
        if self.track().await.is_some() {
            return Err(RuntimeError::plain(format!(
                "the Set has a track “{TRACK}” already (from a check that stopped?): delete it first"
            )));
        }
        self.call("add_tracks_and_scenes", json!({"tracks": [{"name": TRACK, "kind": "midi"}], "scenes": []}))
            .await
            .map_err(RuntimeError::plain)?;
        let track = self.track().await.ok_or_else(|| RuntimeError::plain("the track wasn't added"))?;
        let last = scenes as u64 - 1;
        for (scene, name, root) in [(0, "Riff", 48), (last, "Hook", 60)] {
            self.call("write_midi_clip", json!({"trackRef": track, "sceneIndex": scene, "name": name, "length": 4, "notes": notes(root)}))
                .await
                .map_err(RuntimeError::plain)?;
        }

        println!("Takes of a clip with empty slots below it");
        let riff = self.clip_at(0).await?.ok_or_else(|| RuntimeError::plain("the Riff clip wasn't written"))?;
        let made = self.call("variations", json!({"action": "make", "source": {"clipRef": riff}, "anchors": [0], "count": 3})).await;
        self.say(made.is_ok(), &format!("make: three takes ({})", brief(&made)));
        let names = self.names().await?;
        let expected = [(1, "V1 \u{b7} Riff"), (2, "V2 \u{b7} Riff"), (3, "V3 \u{b7} Riff")];
        self.say(
            expected.iter().all(|(scene, name)| names.get(scene).map(String::as_str) == Some(*name)),
            &format!("in the slots below it, named V1–V3 ({names:?})"),
        );
        let first = self.notes_at(1).await?;
        self.say(
            first.iter().any(|note| note["pitch"] == 48 && note["start"].as_f64() == Some(0.)),
            "V1 keeps the anchor note (48 at the clip's start)",
        );
        let kept = self.call("variations", json!({"action": "keep", "names": ["V1"]})).await;
        let names = self.names().await?;
        self.say(
            kept.is_ok() && names.get(&1).map(String::as_str) == Some("V1 \u{b7} Riff \u{b7} kept"),
            &format!("keep: V1 renamed kept ({})", brief(&kept)),
        );
        let before = (self.notes_at(2).await?, self.notes_at(3).await?);
        let changed = self.call("variations", json!({"action": "change", "names": ["rest"]})).await;
        let names = self.names().await?;
        let after = (self.notes_at(2).await?, self.notes_at(3).await?);
        self.say(
            changed.is_ok()
                && names.get(&2).map(String::as_str) == Some("V2 \u{b7} Riff")
                && names.get(&3).map(String::as_str) == Some("V3 \u{b7} Riff"),
            &format!("change rest: V2 and V3 written again in their slots ({})", brief(&changed)),
        );
        self.say(before != after, "their notes are new");
        let dropped = self.call("variations", json!({"action": "drop", "names": ["rest"]})).await;
        let names = self.names().await?;
        self.say(
            dropped.is_ok() && !names.contains_key(&2) && !names.contains_key(&3) && names.contains_key(&1),
            &format!("drop rest: V2 and V3 deleted, V1 kept ({})", brief(&dropped)),
        );
        let listed = self.call("variations", json!({"action": "list"})).await;
        self.say(listed.as_ref().is_ok_and(|list| list.to_string().contains("kept")), &format!("list ({})", brief(&listed)));

        println!("Takes of a clip in the last scene");
        let hook = self.clip_at(last).await?.ok_or_else(|| RuntimeError::plain("the Hook clip wasn't written"))?;
        let made = self.call("variations", json!({"action": "make", "source": {"clipRef": hook}, "count": 2})).await;
        let now = self.rows("scene", json!({"fields": ["name"]})).await?.len();
        let names = self.names().await?;
        self.say(
            made.is_ok()
                && now == scenes + 2
                && names.get(&(last + 1)).map(String::as_str) == Some("V1 \u{b7} Hook")
                && names.get(&(last + 2)).map(String::as_str) == Some("V2 \u{b7} Hook"),
            &format!("make: a scene added for each take ({} scenes, {})", now, brief(&made)),
        );
        let dropped = self.call("variations", json!({"action": "drop", "set": "Hook", "names": ["rest"]})).await;
        let names = self.names().await?;
        self.say(
            dropped.is_ok() && !names.contains_key(&(last + 1)) && !names.contains_key(&(last + 2)),
            &format!("drop rest: both deleted ({})", brief(&dropped)),
        );
        let note = dropped.as_ref().ok().and_then(|answer| answer["in"]["note"].as_str()).unwrap_or("").to_owned();
        self.say(
            note.contains("V1 \u{b7} Hook") && note.contains("V2 \u{b7} Hook"),
            &format!("the answer says Kumi's scenes stay, empty ({note})"),
        );
        Ok(())
    }

    /// The scenes the check added (past the Set's first `scenes`) and its track, deleted.
    async fn clean_up(&self) -> Result<(), RuntimeError> {
        if self.observation.borrow().is_none() {
            return Ok(());
        }
        self.look().await?;
        let Some(track) = self.track().await else { return Ok(()) };
        // (scenes named after takes are the check's own: a clip's take names it "V… · Hook")
        let scenes = self.rows("scene", json!({"fields": ["name"]})).await?;
        for scene in scenes.iter().rev().filter(|scene| scene["name"].as_str().is_some_and(|name| name.ends_with("\u{b7} Hook"))) {
            self.call("delete_scene", json!({"sceneRef": scene["ref"]})).await.map_err(RuntimeError::plain)?;
        }
        self.call("delete_track", json!({"trackRef": track})).await.map_err(RuntimeError::plain)?;
        println!("Cleaned up: the check's track and scenes deleted.");
        Ok(())
    }

    /// A look at the Set, as a turn starts: its tools and current refs.
    async fn look(&self) -> Result<(), RuntimeError> {
        let observation = self.integration.observe(signal(), None).await?;
        *self.observation.borrow_mut() = Some(observation);
        Ok(())
    }

    fn tool(&self, name: &str) -> Option<Rc<dyn KernelTool>> {
        self.observation.borrow().as_ref()?.tools.iter().find(|tool| tool.name() == name).cloned()
    }

    async fn call(&self, name: &str, input: Value) -> Result<Value, String> {
        let Some(tool) = self.tool(name) else { return Err(format!("{name} isn't offered for this Set")) };
        let input = match input {
            Value::Object(input) => input,
            _ => JsonObject::new(),
        };
        match tool.execute(input, signal()).await {
            Ok(result) if !result.is_error => Ok(serde_json::from_str(&result.text).unwrap_or(Value::String(result.text))),
            Ok(result) => Err(head(&result.text, 600)),
            Err(error) => Err(head(&error.message(), 600)),
        }
    }

    /// Rows of one kind (a test Set's fit in a page).
    async fn rows(&self, kind: &str, extra: Value) -> Result<Vec<Value>, RuntimeError> {
        let mut input = json!({"kind": kind, "limit": 100});
        input.as_object_mut().unwrap().extend(extra.as_object().cloned().unwrap_or_default());
        let read = self.call("live_discover", input).await.map_err(RuntimeError::plain)?;
        Ok(read["live"]["items"].as_array().cloned().unwrap_or_default())
    }

    /// The check's track (its ref), when the Set has it.
    async fn track(&self) -> Option<Value> {
        let tracks = self.rows("track", json!({"fields": ["name"]})).await.ok()?;
        tracks.iter().rfind(|row| row["name"] == TRACK).map(|row| row["ref"].clone())
    }

    /// The clip in the check's track's slot at `scene`.
    async fn clip_at(&self, scene: u64) -> Result<Option<Value>, RuntimeError> {
        let track = self.track().await.ok_or_else(|| RuntimeError::plain("the check's track is gone"))?;
        let slots = self.rows("clip-slot", json!({"parent": track, "fields": ["sceneIndex", "clipRef"]})).await?;
        Ok(slots.iter().find(|slot| slot["sceneIndex"] == scene).map(|slot| slot["clipRef"].clone()).filter(|clip| clip.is_string()))
    }

    /// The names of the clips on the check's track, by scene.
    async fn names(&self) -> Result<std::collections::BTreeMap<u64, String>, RuntimeError> {
        let track = self.track().await.ok_or_else(|| RuntimeError::plain("the check's track is gone"))?;
        let slots = self.rows("clip-slot", json!({"parent": track, "fields": ["sceneIndex", "clipRef"]})).await?;
        let clips: Vec<(u64, Value)> = slots
            .iter()
            .filter(|slot| slot["clipRef"].is_string())
            .filter_map(|slot| Some((slot["sceneIndex"].as_u64()?, slot["clipRef"].clone())))
            .collect();
        let read =
            self.call("read_notes", json!({"clipRefs": clips.iter().map(|(_, clip)| clip).collect::<Vec<_>>(), "format": "json"})).await;
        let read = read.map_err(RuntimeError::plain)?;
        let mut names = std::collections::BTreeMap::new();
        for (scene, clip) in clips {
            if let Some(row) = read["clips"].as_array().into_iter().flatten().find(|row| row["clip"] == clip) {
                names.insert(scene, row["name"].as_str().unwrap_or("").to_owned());
            }
        }
        Ok(names)
    }

    /// The notes of the clip at `scene` on the check's track.
    async fn notes_at(&self, scene: u64) -> Result<Vec<Value>, RuntimeError> {
        let Some(clip) = self.clip_at(scene).await? else { return Ok(Vec::new()) };
        let read = self.call("read_notes", json!({"clipRef": clip, "format": "json"})).await.map_err(RuntimeError::plain)?;
        Ok(read["clips"][0]["notes"].as_array().cloned().unwrap_or_default())
    }
}

fn brief(result: &Result<Value, String>) -> String {
    match result {
        Ok(answer) => head(&answer.to_string(), 300),
        Err(why) => format!("refused: {why}"),
    }
}

fn signal() -> Signal {
    abort::timeout(120_000)
}
