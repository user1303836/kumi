//! Catching up on a Logic Pro project in the app (`kumi::cli::run` with `--daw logic`, as main() makes the session),
//! Kumi's host in process in front of the simulated Logic: the session hands its project store to Logic's integration,
//! the project's baseline is kept beside its conversations under Logic's place, and the next session tells the
//! producer what changed in Logic meanwhile, in Logic's words.
#[path = "support/live_words.rs"]
mod live_words;
use async_trait::async_trait;
use daw_bridge::{files::ensure_secret, paths::BridgePaths, Daw};
use daw_mcp_server::{
    host::Host,
    simulator::{logic::LogicScript, Simulator},
};
use futures::FutureExt;
use kumi::{
    cli::{run, CliIo},
    daw::{DawFactory, OwnHostFactory},
    input::{ByteListener, TerminalInput},
    tui::tty::TtyOutput,
};
use kumi_common::abort::Signal;
use kumi_runtime::core::{contracts::*, errors::RuntimeError};
use kumi_runtime::mcp::client::{McpEndpoint, StderrStatus};
use kumi_runtime::mcp::types::{CallToolResult, Implementation, ListToolsResult};
use kumi_runtime::system::Env;
use serde_json::json;
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
    sync::Arc,
};

#[derive(Default)]
struct Input {
    data: RefCell<Option<ByteListener>>,
    end: RefCell<Option<Rc<dyn Fn()>>>,
}
impl TerminalInput for Input {
    fn is_tty(&self) -> bool {
        false
    }
    fn is_raw(&self) -> bool {
        false
    }
    fn set_raw_mode(&self, _raw: bool) -> std::io::Result<()> {
        Ok(())
    }
    fn resume(&self, f: ByteListener) {
        *self.data.borrow_mut() = Some(f);
    }
    fn pause(&self) {}
    fn on_end(&self, f: Rc<dyn Fn()>) {
        *self.end.borrow_mut() = Some(f);
    }
}
impl Input {
    fn write(&self, s: &str) {
        let f = self.data.borrow().clone();
        if let Some(f) = f {
            f(s.as_bytes())
        }
    }
    fn end(&self) {
        let f = self.end.borrow().clone();
        if let Some(f) = f {
            f()
        }
    }
}
#[derive(Default)]
struct Output(RefCell<String>);
impl TtyOutput for Output {
    fn is_tty(&self) -> bool {
        false
    }
    fn columns(&self) -> Option<i32> {
        None
    }
    fn rows(&self) -> Option<i32> {
        None
    }
    fn write(&self, s: &str) {
        self.0.borrow_mut().push_str(s);
    }
}

/// Kumi's host, called in process.
struct InProcess {
    host: Arc<Host>,
    id: Cell<u64>,
}
#[async_trait(?Send)]
impl McpEndpoint for InProcess {
    fn pid(&self) -> Option<u32> {
        None
    }
    fn server_info(&self) -> Option<Implementation> {
        None
    }
    async fn list(&self, _: Option<&str>, _: Signal) -> Result<ListToolsResult, RuntimeError> {
        Ok(serde_json::from_value(json!({"tools": daw_mcp_server::mcp::tools(&self.host)})).unwrap())
    }
    async fn call(&self, name: &str, args: JsonObject, _: Signal) -> Result<CallToolResult, RuntimeError> {
        self.id.set(self.id.get() + 1);
        let message = json!({"jsonrpc": "2.0", "id": self.id.get(), "method": "tools/call", "params": {"name": name, "arguments": args}});
        let answer = daw_mcp_server::mcp::handle(&self.host, &message).await.expect("an answer");
        Ok(serde_json::from_value(answer["result"].clone()).unwrap())
    }
    fn on_catalog_changed(&self, _: Rc<dyn Fn()>) -> Box<dyn Fn()> {
        Box::new(|| {})
    }
    fn on_disconnect(&self, _: Rc<dyn Fn()>) -> Box<dyn Fn()> {
        Box::new(|| {})
    }
    fn stderr_status(&self) -> StderrStatus {
        StderrStatus { bytes: 0, truncated: false }
    }
    async fn close(&self) -> Result<(), RuntimeError> {
        self.host.close().await;
        Ok(())
    }
}

async fn wait_for(what: &str, out: &Output, done: impl Fn(&str) -> bool) {
    for _ in 0..2500 {
        if done(&out.0.borrow()) {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(4)).await;
    }
    panic!("timed out waiting for {what}:\n{}", out.0.borrow());
}

/// A home where Kumi's Logic bridge is in place (as its receipt says), the simulated Logic behind Kumi's host.
struct Lab {
    home: tempfile::TempDir,
    env: Env,
    factory: DawFactory,
}
impl Lab {
    fn new(paths: &BridgePaths, home: tempfile::TempDir) -> Lab {
        let kumi_home = home.path().join(".kumi");
        let receipt = daw_bridge::lifecycle::Receipt {
            schema: daw_bridge::lifecycle::RECEIPT_SCHEMA.into(),
            daw: Daw::Logic.id().into(),
            status: "installed".into(),
            target: home.path().join("bridge").display().to_string(),
            reference: home.path().join("bridge-reference.json").display().to_string(),
            generation: Some("g1".into()),
            version: Some("1.0.0".into()),
            sha256: None,
            previous: vec![],
            updated_ms: 0,
            history: vec![],
            copies: vec![],
        };
        std::fs::write(paths.receipt(), serde_json::to_string(&receipt).unwrap()).unwrap();
        std::fs::write(
            kumi_home.join("settings.json"),
            json!({"model": "anthropic/claude-sonnet-5-5", "updateCheck": false, "libraryFolders": [], "keepAwake": false, "notifications": "off"})
                .to_string(),
        )
        .unwrap();
        let env = Env::from([
            ("HOME".into(), home.path().display().to_string()),
            ("KUMI_HOME".into(), kumi_home.display().to_string()),
            ("KUMI_NO_UPDATE_CHECK".into(), "1".into()),
            ("KUMI_REMOTE_SCRIPTS_DIR".into(), home.path().join("Remote Scripts").display().to_string()),
            ("KUMI_UI".into(), "plain".into()),
            ("TERM".into(), "xterm-256color".into()),
        ]);
        let host_home = kumi_home.clone();
        let own_host: OwnHostFactory = Rc::new(move |_| {
            let endpoint = Rc::new(InProcess { host: Arc::new(Host::new(Daw::Logic, &host_home)), id: Cell::new(0) });
            Rc::new(move |_| {
                let endpoint = endpoint.clone();
                async move { Ok(endpoint as Rc<dyn McpEndpoint>) }.boxed_local()
            })
        });
        let factory = DawFactory { ableton: Rc::new(|_| panic!("a Logic session makes no Ableton integration")), own_host: Some(own_host) };
        Lab { home, env, factory }
    }

    /// Kumi in plain lines, as main() starts it with `--daw logic`.
    fn start(&self) -> (Rc<Input>, Rc<Output>, tokio::task::JoinHandle<i32>) {
        let input = Rc::new(Input::default());
        let (out, err) = (Rc::new(Output::default()), Rc::new(Output::default()));
        let mut io = CliIo::new(input.clone(), out.clone(), err, self.env.clone());
        io.args = vec!["--daw".into(), "logic".into()];
        (input, out, tokio::task::spawn_local(run(io, self.factory.clone())))
    }
}

async fn quit(input: &Input, task: tokio::task::JoinHandle<i32>) {
    input.write("/quit\n");
    input.end();
    assert_eq!(tokio::time::timeout(std::time::Duration::from_secs(10), task).await.unwrap().unwrap(), 0);
}

/// The producer works in Logic between two sessions of Kumi's: the second tells them what changed since the first saw
/// the project, in Logic's words, from the baseline the first kept in the session's project store.
#[tokio::test(flavor = "current_thread")]
async fn the_next_session_tells_what_changed_in_logic_since_the_last() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let home = tempfile::tempdir().unwrap();
            let paths = BridgePaths::new(&home.path().join(".kumi"), Daw::Logic);
            let secret = ensure_secret(&paths.secret()).unwrap();
            let script = LogicScript::new();
            let mut config = script.config(secret, "com.apple.logic10");
            config.discovery_dir = Some(paths.discovery());
            let simulator = Simulator::start(config).await.unwrap();
            let lab = Lab::new(&paths, home);
            let id = script.project().id.expect("Scratch is saved");
            let baseline = lab.home.path().join(".kumi").join("projects").join(format!("logic-{id}")).join("last-seen.json");

            // The first session keeps the project as it is, beside its conversations, under Logic's place.
            let (input, out, task) = lab.start();
            wait_for("the first look", &out, |text| text.contains("[observation] Logic Pro — Scratch")).await;
            for _ in 0..500 {
                if baseline.is_file() {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
            assert!(baseline.is_file(), "the baseline, under logic-{id}");
            quit(&input, task).await;
            let first = out.0.borrow().clone();
            assert!(!first.contains("[since last time"), "nothing to tell of a project seen first:\n{first}");

            // In Logic, meanwhile: Vocal renamed.
            let respond = script.respond();
            let epoch = script.epoch().to_string();
            let input_ = json!({"ref": format!("{epoch}:track:1"), "name": "Lead Vocal"});
            let preview = respond("change_preview", &json!({"kind": "track-rename", "input": input_, "epoch": epoch})).unwrap().unwrap();
            respond(
                "change_apply",
                &json!({"kind": "track-rename", "input": input_, "fence": preview["fence"], "desc": "by hand", "epoch": epoch}),
            )
            .unwrap()
            .unwrap();

            // The next session tells the producer.
            let (input, out, task) = lab.start();
            wait_for("the catch-up", &out, |text| text.contains("[since last time · just now] Renamed track “Vocal” → “Lead Vocal”")).await;
            quit(&input, task).await;
            simulator.stop();
            let said = out.0.borrow().clone();
            let leaks = live_words::leaks(&said);
            assert!(leaks.is_empty(), "Live's words:\n{}\n\nin:\n{said}", leaks.join("\n"));
        })
        .await;
}
