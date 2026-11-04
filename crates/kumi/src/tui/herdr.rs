//! Kumi in a herdr pane (herdr.dev): herdr's sidebar shows Kumi working, waiting for the producer or idle, and when
//! herdr starts again, the pane starts Kumi again on the conversation it had. Kumi tells herdr through herdr's own
//! program, as herdr asks of an agent that tells it itself: in the background, one report at a time and only the newest,
//! and a report that fails is dropped. Outside herdr, nothing.

use std::{
    cell::RefCell,
    process::Stdio,
    rc::Rc,
    sync::atomic::{AtomicU64, Ordering},
    time::Duration,
};

use futures::{future::LocalBoxFuture, FutureExt};
use kumi_types::system::Env;
use tokio::task::JoinHandle;

use super::alerts::ProgramState;

/// Who tells herdr (herdr's own integrations start `herdr:`), and the name its sidebar shows.
const SOURCE: &str = "kumi";
const AGENT: &str = "kumi";
/// A report herdr hasn't taken in this long is given up.
const REPORT_WAIT: Duration = Duration::from_secs(2);
/// Quitting waits this long at most for herdr to take its pane back.
const RELEASE_WAIT: Duration = Duration::from_secs(1);
/// The longest command herdr keeps to start Kumi again: more, and it refuses the whole report.
const MOST_WORDS: usize = 64;
const MOST_BYTES: usize = 8 * 1024;

/// Runs herdr's program with these arguments, given up after the time given; tests keep the arguments instead.
pub type HerdrRunner = Rc<dyn Fn(Vec<String>, Duration) -> LocalBoxFuture<'static, ()>>;

/// Kumi's pane in herdr, and how Kumi starts again in it.
#[derive(Clone)]
pub struct HerdrPane {
    /// herdr's program, the one that owns the pane (`HERDR_BIN_PATH`).
    pub program: String,
    /// The pane Kumi runs in (`HERDR_PANE_ID`).
    pub pane: String,
    /// How Kumi was started, without a conversation asked for (`kumi --daw reaper`): what starts it again in the pane,
    /// with `-r <id>` once there's a conversation to take up.
    pub start: Vec<String>,
    /// Runs herdr's program (tests); None runs `program`.
    pub run: Option<HerdrRunner>,
}

impl HerdrPane {
    /// Kumi's pane when Kumi runs inside herdr (`HERDR_ENV=1`, with the pane, herdr's program and its socket named);
    /// None anywhere else.
    pub fn from_env(env: &Env, start: Vec<String>) -> Option<HerdrPane> {
        let named = |name: &str| env.get(name).filter(|value| !value.is_empty()).cloned();
        if env.get("HERDR_ENV").map(String::as_str) != Some("1") || named("HERDR_SOCKET_PATH").is_none() {
            return None;
        }
        Some(HerdrPane { program: named("HERDR_BIN_PATH")?, pane: named("HERDR_PANE_ID")?, start, run: None })
    }
}

/// Kumi's state in herdr's words: working, blocked on the producer (and what for), or idle.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AgentState {
    Idle,
    Working,
    Blocked(&'static str),
}

impl AgentState {
    /// The terminal status's state as herdr has it: an answer done or failed leaves Kumi idle, ready for the producer.
    pub fn of(state: ProgramState) -> AgentState {
        match state {
            ProgramState::Working => AgentState::Working,
            ProgramState::Blocked(blocked) => AgentState::Blocked(blocked.words()),
            ProgramState::Idle | ProgramState::Done | ProgramState::Error => AgentState::Idle,
        }
    }

    fn name(self) -> &'static str {
        match self {
            AgentState::Idle => "idle",
            AgentState::Working => "working",
            AgentState::Blocked(_) => "blocked",
        }
    }
}

/// What herdr hears: Kumi's state, and the conversation herdr's restart takes up (None starts Kumi afresh).
#[derive(Clone, Debug, PartialEq, Eq)]
struct Report {
    state: AgentState,
    conversation: Option<String>,
}

/// Tells herdr Kumi's state and conversation as they change, while Kumi runs in its pane.
pub struct Reporter(Rc<Shared>);

struct Shared {
    pane: HerdrPane,
    told: RefCell<Told>,
}

#[derive(Default)]
struct Told {
    /// Kumi's state as the screen last showed it; None before the first frame.
    state: Option<AgentState>,
    /// The conversation the session last said a restart takes up.
    conversation: Option<String>,
    /// What herdr last heard, or is hearing.
    heard: Option<Report>,
    /// The report on its way.
    sending: Option<JoinHandle<()>>,
    /// Kumi is closing: herdr hears nothing more.
    closed: bool,
}

impl Reporter {
    pub fn new(pane: HerdrPane) -> Reporter {
        Reporter(Rc::new(Shared { pane, told: RefCell::default() }))
    }

    /// Kumi's state as the screen shows it, which herdr hears when it changes. Whether it did.
    pub fn state(&self, state: AgentState) -> bool {
        if self.0.told.borrow().state == Some(state) {
            return false;
        }
        self.0.told.borrow_mut().state = Some(state);
        self.send();
        true
    }

    /// The conversation a restart of herdr takes up: kept, and had or taken up in this run. None starts Kumi afresh.
    pub fn conversation(&self, id: Option<String>) {
        if self.0.told.borrow().conversation == id {
            return;
        }
        self.0.told.borrow_mut().conversation = id;
        self.send();
    }

    /// Sends what herdr hasn't heard. While a report is on its way, what changes meanwhile waits for it to end, and
    /// only the newest goes then.
    fn send(&self) {
        let mut told = self.0.told.borrow_mut();
        if told.closed || told.sending.is_some() {
            return;
        }
        let shared = Rc::downgrade(&self.0);
        told.sending = Some(tokio::task::spawn_local(async move {
            while let Some(running) = shared.upgrade().and_then(|shared| shared.next()) {
                running.await;
            }
        }));
    }

    /// Kumi closes: herdr hears nothing more, and the report on its way stops. Quitting gives the pane back too (unlike
    /// closing to open again at once, where the Kumi that opens tells herdr itself), within RELEASE_WAIT.
    pub fn close(&self, give_back: bool) -> LocalBoxFuture<'static, ()> {
        let mut told = self.0.told.borrow_mut();
        if told.closed {
            return async {}.boxed_local();
        }
        told.closed = true;
        if let Some(sending) = told.sending.take() {
            sending.abort();
        }
        if !give_back || told.heard.is_none() {
            return async {}.boxed_local();
        }
        self.0.run(release_args(&self.0.pane.pane, next_seq()), RELEASE_WAIT)
    }
}

impl Shared {
    /// The newest report herdr hasn't heard, running, now heard; None once herdr has heard it all (nothing on its way).
    fn next(&self) -> Option<LocalBoxFuture<'static, ()>> {
        let report = {
            let mut told = self.told.borrow_mut();
            let report = told.state.map(|state| Report { state, conversation: told.conversation.clone() });
            match report {
                Some(report) if !told.closed && told.heard.as_ref() != Some(&report) => {
                    told.heard = Some(report.clone());
                    report
                }
                _ => {
                    told.sending = None;
                    return None;
                }
            }
        };
        Some(self.run(report_args(&self.pane, &report, next_seq()), REPORT_WAIT))
    }

    fn run(&self, args: Vec<String>, wait: Duration) -> LocalBoxFuture<'static, ()> {
        match &self.pane.run {
            Some(run) => run(args, wait),
            None => run_herdr(self.pane.program.clone(), args, wait).boxed_local(),
        }
    }
}

/// herdr's program with `args`, stopped if it hasn't ended after `wait`; what it says doesn't matter.
async fn run_herdr(program: String, args: Vec<String>, wait: Duration) {
    let mut command = tokio::process::Command::new(program);
    command.args(args).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).kill_on_drop(true);
    #[cfg(windows)]
    command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW: no console flashes up for it.
    if let Ok(mut child) = command.spawn() {
        let _ = tokio::time::timeout(wait, child.wait()).await;
    }
}

/// `herdr pane report-agent`: Kumi's state, what it waits for when blocked, its conversation, and the command that
/// starts Kumi again on it (left out where herdr would refuse it, so the state still goes).
fn report_args(pane: &HerdrPane, report: &Report, seq: u64) -> Vec<String> {
    let mut args = words(&["pane", "report-agent", &pane.pane, "--source", SOURCE, "--agent", AGENT, "--state", report.state.name()]);
    args.extend(["--seq".to_string(), seq.to_string()]);
    if let AgentState::Blocked(words) = report.state {
        args.extend(["--message".to_string(), words.to_string()]);
    }
    if let Some(id) = &report.conversation {
        args.extend(["--agent-session-id".to_string(), id.clone()]);
    }
    if let Some(argv) = resume_argv(&pane.start, report.conversation.as_deref()) {
        args.push("--".into());
        args.extend(argv);
    }
    args
}

/// `herdr pane release-agent`: the pane given back.
fn release_args(pane: &str, seq: u64) -> Vec<String> {
    let mut args = words(&["pane", "release-agent", pane, "--source", SOURCE, "--agent", AGENT]);
    args.extend(["--seq".to_string(), seq.to_string()]);
    args
}

fn words(words: &[&str]) -> Vec<String> {
    words.iter().map(|word| word.to_string()).collect()
}

/// The command that starts Kumi again on `conversation` (afresh without one), when herdr takes it: a command's plain
/// name first, at most 64 words and 8 KiB, and no apostrophe or control character in any word.
fn resume_argv(start: &[String], conversation: Option<&str>) -> Option<Vec<String>> {
    let mut argv = start.to_vec();
    if let Some(id) = conversation {
        argv.extend(["-r".to_string(), id.to_string()]);
    }
    let plain = argv.first().is_some_and(|name| !name.is_empty() && !name.contains(['/', '\\']));
    let clean = argv.iter().all(|word| !word.contains('\'') && !word.chars().any(char::is_control));
    let bytes: usize = argv.iter().map(|word| word.len() + 1).sum();
    (plain && clean && argv.len() <= MOST_WORDS && bytes <= MOST_BYTES).then_some(argv)
}

/// A report's number: above every one before it from this Kumi and from one before it in the pane (the clock's
/// milliseconds), so herdr drops a report that comes late.
fn next_seq() -> u64 {
    static LAST: AtomicU64 = AtomicU64::new(0);
    let now = u64::try_from(kumi_common::time::now_ms()).unwrap_or(0);
    let mut last = LAST.load(Ordering::Relaxed);
    loop {
        let next = now.max(last + 1);
        match LAST.compare_exchange_weak(last, next, Ordering::Relaxed, Ordering::Relaxed) {
            Ok(_) => return next,
            Err(seen) => last = seen,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strings(words: &[&str]) -> Vec<String> {
        super::words(words)
    }

    #[test]
    fn kumi_tells_herdr_only_inside_one_of_its_panes() {
        let inside: Env = [
            ("HERDR_ENV", "1"),
            ("HERDR_PANE_ID", "w1:p2"),
            ("HERDR_BIN_PATH", "/opt/herdr/herdr"),
            ("HERDR_SOCKET_PATH", "/run/user/1000/herdr.sock"),
        ]
        .into_iter()
        .map(|(name, value)| (name.to_string(), value.to_string()))
        .collect();
        let pane = HerdrPane::from_env(&inside, strings(&["kumi"])).expect("inside herdr");
        assert_eq!((pane.program.as_str(), pane.pane.as_str()), ("/opt/herdr/herdr", "w1:p2"));
        for name in ["HERDR_ENV", "HERDR_PANE_ID", "HERDR_BIN_PATH", "HERDR_SOCKET_PATH"] {
            let mut env = inside.clone();
            env.insert(name.into(), String::new());
            assert!(HerdrPane::from_env(&env, vec![]).is_none(), "{name} empty");
            env.remove(name);
            assert!(HerdrPane::from_env(&env, vec![]).is_none(), "{name} unset");
        }
        let mut env = inside.clone();
        env.insert("HERDR_ENV".into(), "0".into());
        assert!(HerdrPane::from_env(&env, vec![]).is_none());
    }

    #[test]
    fn the_command_that_starts_kumi_again_is_one_herdr_takes_or_none() {
        assert_eq!(
            resume_argv(&strings(&["kumi", "--daw", "reaper"]), Some("mgj6k3cw0a0b0c")),
            Some(strings(&["kumi", "--daw", "reaper", "-r", "mgj6k3cw0a0b0c"]))
        );
        assert_eq!(resume_argv(&strings(&["kumi", "--inference-only"]), None), Some(strings(&["kumi", "--inference-only"])));
        assert_eq!(
            resume_argv(&strings(&["cargo", "run", "-p", "kumi", "--"]), Some("mgj6k3cw0a0b0c")),
            Some(strings(&["cargo", "run", "-p", "kumi", "--", "-r", "mgj6k3cw0a0b0c"]))
        );
        // herdr refuses these, and the state with them: no command goes.
        assert_eq!(resume_argv(&strings(&["/usr/local/bin/kumi"]), None), None);
        assert_eq!(resume_argv(&strings(&["C:\\Users\\me\\.kumi\\bin\\kumi.cmd"]), None), None);
        assert_eq!(resume_argv(&strings(&["kumi", "--bridge-config", "C:\\Users\\o'brien\\live.json"]), None), None);
        assert_eq!(resume_argv(&strings(&["kumi", "--bridge-config", "/tmp/a\nb.json"]), None), None);
        // At most 64 words, and 8 KiB with a separator after each.
        let words = |count: usize| std::iter::once("kumi".to_string()).chain((1..count).map(|_| "-x".to_string())).collect::<Vec<_>>();
        assert!(resume_argv(&words(62), Some("mgj6k3cw0a0b0c")).is_some());
        assert!(resume_argv(&words(63), Some("mgj6k3cw0a0b0c")).is_none());
        let long = |bytes: usize| strings(&["kumi", "--bridge-config", &"a".repeat(bytes - "kumi --bridge-config ".len() - 1)]);
        assert!(resume_argv(&long(MOST_BYTES), None).is_some());
        assert!(resume_argv(&long(MOST_BYTES + 1), None).is_none());
    }

    #[test]
    fn each_report_is_numbered_above_the_last_and_the_clock() {
        let before = u64::try_from(kumi_common::time::now_ms()).unwrap();
        let numbers: Vec<u64> = (0..100).map(|_| next_seq()).collect();
        assert!(numbers[0] >= before);
        assert!(numbers.windows(2).all(|pair| pair[1] > pair[0]), "{numbers:?}");
    }
}
