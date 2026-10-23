//! Opt-in check on real Live of what Kumi tells a producer working in Live: a line in Live's status bar (the bridge's
//! `live_message`, never its dialog), and whether Live records, which holds a desktop notification back. Live stopped,
//! playing and recording (arrangement recording with no track armed, so nothing is recorded), then stopped again. No
//! model and no sign-in. It works in the open Set (it never opens or closes one): name it, and use a disposable one.
//! The line needs a bridge that shows it as the Control Surface's own (1.0.92 and later).
//!   cargo build --release -p ableton-mcp-server --bins
//!   cargo run --release -p kumi --example away_live -- --set "<Set name>"
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
    time::Duration,
};

fn main() {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let value = |flag: &str| argv.iter().position(|arg| arg == flag).and_then(|at| argv.get(at + 1)).map(|value| trim(value).to_owned());
    let Some(wanted) = value("--set").filter(|name| !name.is_empty()) else {
        eprintln!("Name the open Set, as Live shows it: cargo run --release -p kumi --example away_live -- --set \"<Set name>\"");
        eprintln!("It plays and records the Set for a few seconds (no track armed), so use a disposable Set.");
        std::process::exit(2);
    };
    let env: Env = std::env::vars().collect();
    let Some(bridge_config) = find_bridge_config(&env) else {
        eprintln!("The Ableton bridge isn't installed; run Kumi's bridge setup first.");
        std::process::exit(2);
    };
    let bridge = bridge_program();
    if !bridge.is_file() {
        eprintln!(
            "The Ableton bridge isn't built ({}); build it first: cargo build --release -p ableton-mcp-server --bins",
            bridge.display()
        );
        std::process::exit(2);
    }
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().expect("a runtime");
    let local = tokio::task::LocalSet::new();
    std::process::exit(local.block_on(&runtime, away_live(wanted, bridge_config, bridge)));
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

async fn away_live(wanted: String, bridge_config: String, bridge: PathBuf) -> i32 {
    let mut options = AbletonOptions::new(Rc::new(|_, _| {}));
    options.bridge_config = Some(bridge_config.clone());
    options.connect = Some(connect_to(bridge, bridge_config));
    let run = Run { integration: create_ableton_integration(options), observation: RefCell::default(), passed: RefCell::default() };
    let code = match run.check(&wanted).await {
        Ok(()) => {
            let passed = run.passed.borrow();
            let good = passed.iter().filter(|ok| **ok).count();
            println!("\n{good} of {} passed.", passed.len());
            i32::from(good != passed.len())
        }
        Err(error) => {
            eprintln!("away-live: {}", head(&error.message(), 300));
            1
        }
    };
    // Whatever happened, Live is left stopped and not recording.
    let _ = run.python("(setattr(song, 'record_mode', False), song.stop_playing(), True)[-1]").await;
    let _ = run.integration.close().await;
    code
}

impl Run {
    fn say(&self, ok: bool, what: &str) {
        self.passed.borrow_mut().push(ok);
        println!("  {}  {what}", if ok { "ok  " } else { "FAIL" });
    }

    async fn check(&self, wanted: &str) -> Result<(), RuntimeError> {
        self.integration.start(signal()).await?;
        let observation = self.integration.observe(signal(), None).await?;
        *self.observation.borrow_mut() = Some(observation.clone());
        let context: Value = serde_json::from_str(&observation.context).unwrap_or(Value::Null);
        let set = context["set"]["name"].as_str().unwrap_or("?").to_owned();
        if set != wanted {
            println!("The open Set is “{set}”, not “{wanted}”; nothing was changed.");
            return Err(RuntimeError::plain("not the Set named"));
        }
        println!("Live's status bar");
        let shown = self.integration.show_message("Kumi · real check · this line is Kumi's", signal()).await;
        self.say(matches!(shown, Ok(true)), &format!("a line shown in Live's status bar, with bridge 1.0.92 or later ({shown:?})"));
        // A line in passing never holds Live: a modal alert would hold its main thread, and this read with it.
        let began = std::time::Instant::now();
        let answered = self.integration.recording(abort::timeout(5_000)).await;
        self.say(
            answered.is_ok() && began.elapsed() < Duration::from_secs(5),
            &format!("Live answers at once after it ({:?} in {} ms)", answered, began.elapsed().as_millis()),
        );

        println!("Whether Live records");
        self.python("(setattr(song, 'record_mode', False), song.stop_playing(), True)[-1]").await?;
        tokio::time::sleep(Duration::from_millis(1500)).await;
        let stopped = self.integration.recording(signal()).await;
        self.say(stopped == Ok(Some(false)), &format!("stopped: not recording ({stopped:?})"));
        self.python("(song.start_playing(), True)[-1]").await?;
        tokio::time::sleep(Duration::from_millis(1500)).await;
        let playing = self.integration.recording(signal()).await;
        self.say(playing == Ok(Some(false)), &format!("playing: not recording ({playing:?})"));
        self.python("(setattr(song, 'record_mode', True), True)[-1]").await?;
        tokio::time::sleep(Duration::from_millis(1500)).await;
        let recording = self.integration.recording(signal()).await;
        self.say(recording == Ok(Some(true)), &format!("arrangement recording: recording ({recording:?})"));
        self.python("(setattr(song, 'record_mode', False), song.stop_playing(), True)[-1]").await?;
        tokio::time::sleep(Duration::from_millis(1500)).await;
        let after = self.integration.recording(signal()).await;
        self.say(after == Ok(Some(false)), &format!("stopped again: not recording ({after:?})"));
        Ok(())
    }

    async fn call(&self, name: &str, input: Value) -> Result<Value, String> {
        let tool: Option<Rc<dyn KernelTool>> =
            self.observation.borrow().as_ref().and_then(|seen| seen.tools.iter().find(|tool| tool.name() == name).cloned());
        let Some(tool) = tool else { return Err(format!("{name} isn't offered for this Set")) };
        let input = match input {
            Value::Object(input) => input,
            _ => JsonObject::new(),
        };
        match tool.execute(input, signal()).await {
            Ok(result) if !result.is_error => Ok(serde_json::from_str(&result.text).unwrap_or(Value::String(result.text))),
            Ok(result) => Err(head(&result.text, 400)),
            Err(error) => Err(head(&error.message(), 400)),
        }
    }

    async fn python(&self, code: &str) -> Result<Value, RuntimeError> {
        let done = self.call("run_python", json!({"code": code, "mode": "eval"})).await.map_err(RuntimeError::plain)?;
        let done = if done.get("ok").is_some() { done } else { done["live"].clone() };
        if done["ok"] != true {
            return Err(RuntimeError::plain(format!("Live's Python: {}", head(&done.to_string(), 300))));
        }
        Ok(done["result"].clone())
    }
}

fn signal() -> Signal {
    abort::timeout(120_000)
}
