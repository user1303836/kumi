//! How Kumi starts, word for word: which integration it picks, and what it says when it carries on without Live.
use kumi::{
    cli::{run, AbletonFactory, CliIo},
    config::load_config,
    input::{ByteListener, TerminalInput},
    tui::tty::TtyOutput,
};
use kumi_agent::core::contracts::*;
use kumi_common::abort::Signal;
use kumi_types::{
    errors::{FailureKind, KumiError, RuntimeError},
    system::Env,
};
use serde_json::{json, Value};
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};
#[derive(Default)]
struct Input {
    tty: bool,
    raw: Cell<bool>,
    data: RefCell<Option<ByteListener>>,
    end: RefCell<Option<Rc<dyn Fn()>>>,
    ready: tokio::sync::Notify,
}
impl TerminalInput for Input {
    fn is_tty(&self) -> bool {
        self.tty
    }
    fn is_raw(&self) -> bool {
        self.raw.get()
    }
    fn set_raw_mode(&self, v: bool) -> std::io::Result<()> {
        self.raw.set(v);
        Ok(())
    }
    fn resume(&self, f: ByteListener) {
        *self.data.borrow_mut() = Some(f);
        self.ready.notify_one();
    }
    fn pause(&self) {
        self.data.borrow_mut().take();
    }
    fn on_end(&self, f: Rc<dyn Fn()>) {
        *self.end.borrow_mut() = Some(f)
    }
}
impl Input {
    async fn wait_ready(&self) {
        tokio::time::timeout(std::time::Duration::from_secs(10), async {
            while self.data.borrow().is_none() {
                self.ready.notified().await;
            }
        })
        .await
        .expect("CLI did not register its input listener");
    }
    fn write(&self, s: &str) {
        let f = self.data.borrow().clone().expect("wait for the CLI input listener before writing");
        f(s.as_bytes())
    }
    fn end(&self) {
        let f = self.end.borrow().clone();
        if let Some(f) = f {
            f()
        }
    }
}
#[derive(Default)]
struct Output(RefCell<String>, tokio::sync::Notify);
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
        self.1.notify_one();
    }
}
struct Fixture {
    folder: tempfile::TempDir,
    input: Rc<Input>,
    out: Rc<Output>,
    err: Rc<Output>,
    env: Env,
}
impl Fixture {
    fn new() -> Self {
        let folder = tempfile::tempdir().unwrap();
        let env = Env::from([
            ("KUMI_HOME".into(), folder.path().display().to_string()),
            ("KUMI_NO_UPDATE_CHECK".into(), "1".into()),
            ("KUMI_UI".into(), "plain".into()),
            ("KUMI_REMOTE_SCRIPTS_DIR".into(), folder.path().join("Remote Scripts").display().to_string()),
        ]);
        Self { folder, input: Rc::new(Input::default()), out: Rc::default(), err: Rc::default(), env }
    }
    fn io(&self, args: &[&str]) -> CliIo {
        let mut io = CliIo::new(self.input.clone(), self.out.clone(), self.err.clone(), self.env.clone());
        io.args = args.iter().map(|s| s.to_string()).collect();
        io
    }
    fn settings(&self, value: Value) {
        std::fs::write(self.folder.path().join("settings.json"), serde_json::to_vec(&value).unwrap()).unwrap();
    }
    /// What Kumi wrote, without its library's progress: the library it learns is this computer's (Live's own sounds where
    /// Live is installed), so its lines differ from one computer to the next.
    fn output(&self) -> String {
        let library = regex::Regex::new(r"; (Learning your library|Your library:)[^;\n]*").unwrap();
        let text = self.out.0.borrow();
        let kept: Vec<String> = text
            .lines()
            .filter(|l| !l.starts_with("Learning your library") && !l.starts_with("Your library:"))
            .map(|l| library.replace_all(l, "").into_owned())
            .collect();
        kept.iter().map(|l| format!("{l}\n")).collect()
    }
}
fn golden(name: &str, actual: &str) {
    let file = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/goldens/startup").join(name);
    if std::env::var_os("KUMI_BLESS").is_some() {
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, actual).unwrap();
        return;
    }
    let expected =
        std::fs::read_to_string(&file).unwrap_or_else(|_| panic!("{} is missing: run this test with KUMI_BLESS=1", file.display()));
    if actual != expected {
        panic!("{}", difference(&file, &expected, actual));
    }
}

/// Where a golden and what a test made first part: the line before, the golden's lines and the new ones from there,
/// and, for a long line, the column where they part.
fn difference(file: &std::path::Path, expected: &str, actual: &str) -> String {
    let (old, new): (Vec<&str>, Vec<&str>) = (expected.split('\n').collect(), actual.split('\n').collect());
    let at = old.iter().zip(&new).position(|(a, b)| a != b).unwrap_or(old.len().min(new.len()));
    let mut said = format!(
        "{} changed at line {} ({} lines now, {} before): check the difference, then run this test with KUMI_BLESS=1\n",
        file.display(),
        at + 1,
        new.len(),
        old.len()
    );
    let shown = |line: &str, from: usize| -> String {
        let chars: Vec<char> = line.chars().collect();
        let start = from.saturating_sub(60);
        let end = (from + 100).min(chars.len());
        let cut: String = chars[start.min(end)..end].iter().collect();
        format!("{}{cut}{}", if start > 0 { "…" } else { "" }, if end < chars.len() { "…" } else { "" })
    };
    let column = match (old.get(at), new.get(at)) {
        (Some(a), Some(b)) => a.chars().zip(b.chars()).position(|(x, y)| x != y).unwrap_or(a.chars().count().min(b.chars().count())),
        _ => usize::MAX,
    };
    if column != usize::MAX {
        said.push_str(&format!("(the line parts at column {})\n", column + 1));
    }
    let column = if column == usize::MAX { 0 } else { column };
    let before = at.saturating_sub(2);
    for (n, line) in old.iter().enumerate().skip(before).take(at - before) {
        said.push_str(&format!("  {:>5} | {}\n", n + 1, shown(line, column)));
    }
    for (n, line) in old.iter().enumerate().skip(at).take(3) {
        said.push_str(&format!("- {:>5} | {}\n", n + 1, shown(line, column)));
    }
    for (n, line) in new.iter().enumerate().skip(at).take(3) {
        said.push_str(&format!("+ {:>5} | {}\n", n + 1, shown(line, column)));
    }
    said
}
fn golden_json(name: &str, value: &Value) {
    golden(name, &format!("{}\n", serde_json::to_string_pretty(value).unwrap()));
}
async fn wait_for(what: &str, done: impl Fn() -> bool) {
    for _ in 0..2500 {
        if done() {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(4)).await;
    }
    panic!("timed out waiting for {what}");
}
fn unused_factory() -> AbletonFactory {
    AbletonFactory(Rc::new(|_| panic!("without a bridge, Kumi makes no Ableton integration")))
}
macro_rules! case {
    ($name:ident,$body:expr) => {
        #[tokio::test(flavor = "current_thread")]
        async fn $name() {
            tokio::task::LocalSet::new().run_until($body).await;
        }
    };
}
struct Failing(String, Rc<Cell<u32>>);
#[async_trait::async_trait(?Send)]
impl Integration for Failing {
    async fn start(&self, _: Signal) -> Result<(), RuntimeError> {
        self.1.set(self.1.get() + 1);
        // As Live's integration says it: another bridge in Live is a failure restarting Live fixes.
        let failure = KumiError::new(FailureKind::Live, self.0.clone());
        if self.0.contains(kumi_common::bridge::ANOTHER_BRIDGE) {
            return Err(failure.with_setup(kumi_types::errors::SetupFailure::RestartDaw).into());
        }
        Err(failure.into())
    }
    async fn observe(&self, _: Signal, _: Option<ObserveHints>) -> Result<Observation, RuntimeError> {
        Err(RuntimeError::plain("not started"))
    }
    async fn close(&self) -> Result<(), RuntimeError> {
        Ok(())
    }
}

/// The configuration as JSON, with the test's own folder written as <home> and paths with forward slashes.
fn portable(value: Value, home: &str) -> Value {
    match value {
        Value::String(s) => Value::String(s.replace(home, "<home>").replace('\\', "/")),
        Value::Array(items) => Value::Array(items.into_iter().map(|v| portable(v, home)).collect()),
        Value::Object(items) => Value::Object(items.into_iter().map(|(k, v)| (k, portable(v, home))).collect()),
        other => other,
    }
}
#[test]
fn without_a_bridge_kumi_starts_inference_only_and_with_one_it_starts_with_live() {
    let f = Fixture::new();
    let home = f.folder.path().display().to_string();
    let config = |args: &[&str]| {
        let args: Vec<String> = args.iter().map(|s| s.to_string()).collect();
        portable(serde_json::to_value(load_config(&args, &f.env).unwrap()).unwrap(), &home)
    };
    let bridge = f.folder.path().join("bridge-config.json");
    std::fs::write(&bridge, "{}").unwrap();
    let mut chosen = serde_json::Map::new();
    chosen.insert("no bridge in Live".into(), config(&[]));
    chosen.insert("--inference-only".into(), config(&["--inference-only"]));
    chosen.insert("--bridge-config".into(), config(&["--bridge-config", bridge.to_str().unwrap()]));
    let reference = f.folder.path().join("Remote Scripts/AbletonMcpBridge");
    std::fs::create_dir_all(&reference).unwrap();
    std::fs::write(reference.join("bridge-reference.json"), json!({"config": f.folder.path().join("gone.json")}).to_string()).unwrap();
    chosen.insert("the bridge's configuration gone".into(), config(&[]));
    std::fs::write(reference.join("bridge-reference.json"), json!({ "config": bridge }).to_string()).unwrap();
    chosen.insert("the bridge in Live".into(), config(&[]));
    chosen.insert("the bridge in Live, --inference-only".into(), config(&["--inference-only"]));
    golden_json("chosen.json", &Value::Object(chosen));
}
/// What the plain terminal shows Kumi waits on: commands answer in order, so an unknown one's answer marks the end.
const MARK: &str = "Unknown command. Use /help.";
/// /status, asked until the session has finished what it was doing (starting, reconnecting): where the first asking and
/// the answer kept begin, in the output's lines.
async fn idle_status(f: &Fixture) -> (usize, usize) {
    let first = f.output().lines().count();
    for _ in 0..100 {
        let before = f.output().lines().count();
        f.input.write("/status\n/mark\n");
        for _ in 0..2500 {
            if f.output().lines().skip(before).any(|l| l == MARK) {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(4)).await;
        }
        let said: Vec<String> = f.output().lines().skip(before).filter(|l| l.starts_with("[status] ")).map(str::to_owned).collect();
        if said.first().is_some_and(|s| s.starts_with("[status] idle;")) {
            return (first, before);
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    panic!("the session never settled: {}", f.output());
}
/// The status lines from `from` on, once Kumi has closed.
fn status_from(output: &str, from: usize) -> Vec<&str> {
    output.lines().skip(from).filter(|l| l.starts_with("[status] ")).collect()
}
async fn quit(f: &Fixture, task: tokio::task::JoinHandle<i32>) {
    f.input.write("/quit\n");
    f.input.end();
    assert_eq!(tokio::time::timeout(std::time::Duration::from_secs(10), task).await.unwrap().unwrap(), 0);
    assert!(f.err.0.borrow().is_empty(), "{}", f.err.0.borrow());
}
case!(inference_only_starts_and_says_this, async {
    // No bridge: Kumi never makes Ableton's integration, and runs inference-only.
    let f = Fixture::new();
    f.settings(json!({"model":"anthropic/claude-sonnet-5-5","updateCheck":false,"libraryFolders":[]}));
    let task = tokio::task::spawn_local(run(f.io(&["--inference-only"]), unused_factory()));
    f.input.wait_ready().await;
    wait_for("the first look", || f.output().lines().any(|l| l.starts_with("[observation] "))).await;
    let (first, from) = idle_status(&f).await;
    quit(&f, task).await;
    let output = f.output();
    let mut kept: Vec<&str> = output.lines().take(first).collect();
    kept.extend(status_from(&output, from));
    kept.extend(output.lines().last());
    golden("inference-only.txt", &format!("{}\n", kept.join("\n")));
});
/// Ableton's integration can't start (Live's setup isn't right, saying `message`): Kumi carries on without Live. Started
/// three times (at first, then on /reconnect or by the watch for Live), what it says.
async fn falls_back(message: &str) -> Value {
    let f = Fixture::new();
    let config = f.folder.path().join("bridge.json");
    std::fs::write(&config, "{}").unwrap();
    f.settings(json!({"model":"anthropic/claude-sonnet-5-5","updateCheck":false,"libraryFolders":[]}));
    let starts = Rc::new(Cell::new(0));
    let count = starts.clone();
    let message = message.to_string();
    let factory = AbletonFactory(Rc::new(move |_| Rc::new(Failing(message.clone(), count.clone())) as Rc<dyn Integration>));
    let task = tokio::task::spawn_local(run(f.io(&["--bridge-config", config.to_str().unwrap()]), factory));
    f.input.wait_ready().await;
    let observations = || f.output().lines().filter(|l| l.starts_with("[observation] ")).count();
    wait_for("the first look", || observations() >= 1).await;
    // Up to the first look, whatever starts after it.
    let output = f.output();
    let first: Vec<&str> =
        output.lines().scan(false, |seen, l| (!std::mem::replace(seen, l.starts_with("[observation] ") || *seen)).then_some(l)).collect();
    while starts.get() < 3 {
        let before = starts.get();
        f.input.write("/reconnect\n");
        for _ in 0..1250 {
            if starts.get() > before {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(4)).await;
        }
    }
    let (polled, from) = idle_status(&f).await;
    let started = starts.get();
    quit(&f, task).await;
    let all = f.output();
    let lines: Vec<&str> = all.lines().collect();
    // Past the two lines Kumi starts with and before the last, the notices: every line not tagged.
    let said: Vec<&str> = lines[2..polled].iter().copied().filter(|l| !l.starts_with('[') && !l.is_empty() && *l != MARK).collect();
    json!({
        "first lines": first,
        "notices": said,
        "starts": if started == 3 { json!(3) } else { json!(format!("{started}, the watch for Live started it again")) },
        "status": status_from(&all, from),
        "last line": lines.last(),
    })
}
case!(a_live_setup_failure_falls_back_and_says_so, async {
    let mut said = serde_json::Map::new();
    // Said each time it starts.
    said.insert("live setup failure".into(), falls_back("Live isn't open; open it first.").await);
    // Live runs another version of the bridge: restarting Live is the fix, said once a session however often it starts.
    let mut again = falls_back(kumi_common::bridge::ANOTHER_BRIDGE).await;
    assert!(again["starts"] == 3 || again["starts"].is_string(), "{again}");
    again["starts"] = json!("3 or more");
    said.insert("another bridge".into(), again);
    golden_json("fallback.json", &Value::Object(said));
});
