//! Command dispatch and native process I/O.
mod session;
mod shutdown;
use crate::{
    bridge_setup::{self, BridgeSetupIo},
    config::*,
    doctor::{self, DoctorIo},
    input::TerminalInput,
    install::{self, InstalledIo},
    library::{run_library, LibraryIo},
    live_app,
    login::{self, AuthIo, LoginIo},
    report::{write_report, ReportIo},
    spinner::step,
    tui::tty::TtyOutput,
    update::{self, CheckIo, UpdateIo},
};
use futures::{future::LocalBoxFuture, FutureExt};
use kumi_common::{
    abort::{self, Signal},
    js::{number, string},
};
use kumi_runtime::{
    core::contracts::Integration, integrations::ableton::AbletonOptions, providers::ProviderId, system::Env, RuntimeError, KUMI_VERSION,
};
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};
pub type AbletonFactory = Rc<dyn Fn(AbletonOptions) -> Rc<dyn Integration>>;
#[derive(Clone)]
pub struct CliIo {
    pub input: Rc<dyn TerminalInput>,
    pub out: Rc<dyn TtyOutput>,
    pub err: Rc<dyn TtyOutput>,
    pub env: Env,
    pub args: Vec<String>,
    pub signals: bool,
    pub reopen: Option<Rc<dyn Fn() -> LocalBoxFuture<'static, i32>>>,
    exit_after_reopen: Rc<Cell<bool>>,
}
impl CliIo {
    pub fn new(input: Rc<dyn TerminalInput>, out: Rc<dyn TtyOutput>, err: Rc<dyn TtyOutput>, env: Env) -> Self {
        Self { input, out, err, env, args: vec![], signals: false, reopen: None, exit_after_reopen: Rc::new(Cell::new(false)) }
    }
    pub fn installed(&self) -> bool {
        self.env.get("KUMI_INSTALLED").is_some_and(|s| s == "1")
    }
    pub fn command(&self) -> &str {
        if self.installed() {
            "kumi"
        } else {
            "cargo run -p kumi --"
        }
    }
}
fn help_rows(rows: &[(&str, &str)]) -> String {
    let width = rows.iter().map(|(s, _)| string::utf16_len(s)).max().unwrap_or(0) + 3;
    rows.iter()
        .map(|(s, text)| format!("  {}{}", string::pad_end(s, width), text.replace('\n', &format!("\n  {}", " ".repeat(width)))))
        .collect::<Vec<_>>()
        .join("\n")
}
pub fn help(installed: bool) -> String {
    let cmd = if installed { "kumi" } else { "cargo run -p kumi --" };
    let mut first = vec![];
    if !installed {
        first.push(("cargo build --release --workspace".to_string(), "Build Kumi and the Ableton bridge"));
    }
    first.push((format!("{cmd} bridge"), "With Live closed: put the bridge into Live, or bring it up to date"));
    first.push((cmd.into(),"Talk about the open Live Set; the installed bridge is found automatically.\nSign in there with /login, and choose a model with /model."));
    let mut more=vec![("--inference-only","Chat without Live"),("--bridge-config <path>","Use this bridge configuration (an absolute path)"),("login","Sign in: asks whether with ChatGPT or an API key"),("login <provider>","Sign in to one provider: openai-codex with a ChatGPT plan (--device\nwithout a browser); anthropic, openai, opencode with an API key (asked for)"),("logout <provider>","Remove Kumi's sign-in for that provider"),("model [<provider>/<model>]","Show or choose the model; ollama/<model> or lmstudio/<model> for one on this computer"),("auth","Show which providers are usable (no secrets)"),("doctor","Check sign-in, the bridge, Live and the terminal"),("library","What Kumi knows of your sounds, presets and Sets (it learns them in the\nbackground); --rebuild learns them all again"),("update","Bring Kumi up to date, and the bridge in Live when it's older"),("update --check","Say whether there's a newer Kumi, without installing it")];
    if installed {
        more.extend([
            ("update --rollback", "Go back to the Kumi you had before the last update"),
            ("uninstall", "Remove Kumi (your conversations and notes stay unless you say)"),
        ]);
    }
    more.extend([("report", "Write a file to send when something goes wrong (no keys in it)"), ("--version", "Show Kumi's version")]);
    let more: Vec<_> = more.into_iter().map(|(s, text)| (format!("{cmd} {s}"), text)).collect();
    format!(
        "Kumi {KUMI_VERSION} — producer assistant for Ableton Live\n\nFirst run:\n{}\n\nMore:\n{}\n\n{}",
        help_rows(&first.iter().map(|(s, t)| (s.as_str(), *t)).collect::<Vec<_>>()),
        help_rows(&more.iter().map(|(s, t)| (s.as_str(), *t)).collect::<Vec<_>>()),
        include_str!("help-tail.txt")
    )
}
struct SignalTask(Option<tokio::task::JoinHandle<()>>);
impl Drop for SignalTask {
    fn drop(&mut self) {
        if let Some(task) = self.0.take() {
            task.abort();
        }
    }
}
fn interrupt_signal(io: &CliIo) -> (Signal, SignalTask) {
    let signal = Signal::new();
    let copy = signal.clone();
    let task = io.signals.then(|| {
        tokio::task::spawn_local(async move {
            if tokio::signal::ctrl_c().await.is_ok() {
                copy.cancel();
            }
        })
    });
    (signal, SignalTask(task))
}
async fn read_answer(io: &CliIo, prompt: &str) -> String {
    io.out.write(prompt);
    let (tx, rx) = tokio::sync::oneshot::channel();
    let tx = Rc::new(RefCell::new(Some(tx)));
    let bytes = Rc::new(RefCell::new(vec![]));
    io.input.on_end(Rc::new({
        let tx = tx.clone();
        let bytes = bytes.clone();
        let input = Rc::downgrade(&io.input);
        move || {
            if let Some(tx) = tx.borrow_mut().take() {
                if let Some(input) = input.upgrade() {
                    input.pause();
                }
                let _ = tx.send(String::from_utf8_lossy(&bytes.borrow()).into_owned());
            }
        }
    }));
    let input = Rc::downgrade(&io.input);
    io.input.resume(Rc::new(move |data| {
        let end = data.iter().position(|b| matches!(b, b'\n' | b'\r'));
        bytes.borrow_mut().extend_from_slice(&data[..end.unwrap_or(data.len())]);
        if end.is_some() {
            if let Some(tx) = tx.borrow_mut().take() {
                if let Some(input) = input.upgrade() {
                    input.pause();
                }
                let _ = tx.send(String::from_utf8_lossy(&bytes.borrow()).into_owned());
            }
        }
    }));
    let answer = rx.await.unwrap_or_default();
    io.input.pause();
    answer
}
fn installed_io(io: &CliIo) -> InstalledIo {
    let mut out = InstalledIo::new(io.out.clone(), io.env.clone());
    out.input = Some(io.input.clone());
    out
}
fn doctor_io(io: &CliIo, factory: AbletonFactory) -> DoctorIo {
    let mut doctor = DoctorIo::new(io.out.clone(), io.env.clone());
    doctor.bundled_bridge_version = bridge_setup::bundled_bridge_version();
    doctor.probe_live = Some(Rc::new(move |config| {
        let factory = factory.clone();
        async move { Ok(session::probe_live(config, factory).await) }.boxed_local()
    }));
    doctor
}
async fn login_with(config: &AppConfig, io: &CliIo) -> Result<(), RuntimeError> {
    let (cancel, _guard) = interrupt_signal(io);
    login::login(
        config,
        LoginIo {
            out: io.out.clone(),
            env: io.env.clone(),
            input: Some(io.input.clone()),
            signal: abort::any([cancel, abort::timeout(15 * 60000)]),
            open_browser: io.out.is_tty().then(|| Rc::new(|url: String| login::open_browser(&url)) as Rc<dyn Fn(String)>),
            fetch: None,
        },
    )
    .await
}
pub async fn run(io: CliIo, factory: AbletonFactory) -> i32 {
    io.exit_after_reopen.set(false);
    let mut secrets: Vec<String> = ["AI_GATEWAY_API_KEY", "OPENAI_API_KEY", "ANTHROPIC_API_KEY", "OPENCODE_API_KEY", "LM_API_TOKEN"]
        .into_iter()
        .filter_map(|s| io.env.get(s).filter(|s| !s.is_empty()).cloned())
        .collect();
    if let Ok(file) = load_settings_file(&io.env) {
        for server in read_settings(&file).model_servers {
            if let Some(key) = server.api_key.filter(|s| !s.is_empty()) {
                secrets.push(key);
            }
        }
    }
    match dispatch(&io, factory, &mut secrets).await {
        Ok(code) => code,
        Err(error) => {
            io.err.write(&format!("Kumi: {}\n", safe_error_message(Some(&error.message()), &secrets)));
            1
        }
    }
}
async fn dispatch(io: &CliIo, factory: AbletonFactory, secrets: &mut Vec<String>) -> Result<i32, RuntimeError> {
    let config = load_config(&io.args, &io.env)?;
    match &config {
        AppConfig::Help => {
            io.out.write(&help(io.installed()));
            Ok(0)
        }
        AppConfig::Version => {
            io.out.write(&format!("Kumi {KUMI_VERSION}\n"));
            Ok(0)
        }
        AppConfig::Doctor => doctor::run_doctor(doctor_io(io, factory)).await,
        AppConfig::Report => write_report(ReportIo::new(doctor_io(io, factory))).await,
        AppConfig::Update { rollback, check } => {
            if *rollback && !io.installed() {
                io.out.write("This Kumi runs from a copy of its repository, so there's no earlier Kumi kept to go back to. Check out the commit you want with git, then run: cargo build --release --workspace\n");
                return Ok(1);
            }
            if *check {
                let latest = step(
                    io.out.clone(),
                    &io.env,
                    "Looking for a newer Kumi…",
                    async {
                        if io.installed() {
                            install::check_release(&io.env, None).await
                        } else {
                            update::check_checkout(CheckIo::default()).await
                        }
                    },
                    false,
                )
                .await;
                match latest {
                    Ok(latest) => {
                        io.out.write(
                            &latest
                                .map(|latest| {
                                    format!("Kumi {latest} is out (this is {KUMI_VERSION}). Update with: {} update\n", io.command())
                                })
                                .unwrap_or_else(|| format!("Kumi is up to date ({KUMI_VERSION}).\n")),
                        );
                        Ok(0)
                    }
                    Err(error) => {
                        io.out.write(&format!("{}.\n", safe_error_message(Some(&error.message()), secrets)));
                        Ok(1)
                    }
                }
            } else if !io.installed() {
                Ok(update::run_update(UpdateIo::new(io.out.clone(), io.env.clone())).await)
            } else if *rollback {
                install::rollback_installed(installed_io(io)).await
            } else {
                install::update_installed(installed_io(io)).await
            }
        }
        AppConfig::Uninstall { all, yes } => {
            if io.installed() {
                install::uninstall_installed(installed_io(io), install::UninstallOptions { all: *all, yes: *yes }).await
            } else {
                io.out
                    .write("This Kumi runs from a copy of its repository; delete that folder to remove it (your files are in ~/.kumi).\n");
                Ok(1)
            }
        }
        AppConfig::Library { rebuild } => {
            let (signal, _guard) = interrupt_signal(io);
            let mut options = LibraryIo::new(io.out.clone(), io.env.clone());
            options.rebuild = *rebuild;
            options.signal = Some(signal);
            run_library(options).await
        }
        AppConfig::Bridge { yes, allow_dirty } => {
            let mut options = BridgeSetupIo::new(io.out.clone(), io.env.clone());
            options.input = Some(io.input.clone());
            options.yes = *yes;
            options.allow_dirty = *allow_dirty;
            options.wait_ms = io
                .env
                .get("KUMI_BRIDGE_WAIT_SECONDS")
                .filter(|s| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()))
                .and_then(|s| s.parse::<u64>().ok())
                .map(|n| n.saturating_mul(1000));
            bridge_setup::setup_bridge(options).await
        }
        AppConfig::Auth { .. } => {
            login::auth_status(&config, AuthIo { out: io.out.clone(), env: io.env.clone(), fetch: None }).await?;
            Ok(0)
        }
        AppConfig::Logout { .. } => {
            login::logout(&config, AuthIo { out: io.out.clone(), env: io.env.clone(), fetch: None }).await?;
            Ok(0)
        }
        AppConfig::Model { settings_file, model } => {
            let mut settings = read_settings(settings_file);
            if let Some(model) = model {
                settings.model = Some(model.clone());
                write_settings(settings_file, &serde_json::to_value(&settings).unwrap())?;
            }
            io.out.write(&model.as_ref().map(|m| format!("Model set to {m}.\n")).unwrap_or_else(|| {
                format!(
                    "Model: {}. Change it with: {} model <provider>/<model>\n",
                    settings.model.as_deref().unwrap_or("not chosen"),
                    io.command()
                )
            }));
            if let Some(model) = io.env.get("KUMI_MODEL").filter(|s| !s.is_empty()) {
                io.out.write(&format!("KUMI_MODEL={model} currently overrides it.\n"));
            }
            Ok(0)
        }
        AppConfig::LoginChoose { auth_file, pi_auth_file, settings_file } => {
            let choices = [
                (ProviderId::OpenaiCodex, "ChatGPT: sign in with your ChatGPT plan (opens your browser)"),
                (ProviderId::Anthropic, "Anthropic: paste an API key"),
                (ProviderId::Openai, "OpenAI: paste an API key"),
                (ProviderId::Opencode, "OpenCode: paste an API key"),
            ];
            if !io.input.is_tty() {
                return Err(RuntimeError::plain(format!(
                    "Use: {} login <provider>, with provider one of {}.",
                    io.command(),
                    choices.iter().map(|(id, _)| id.as_str()).collect::<Vec<_>>().join(", ")
                )));
            }
            io.out.write(&format!(
                "How do you want to sign in?\n{}\n",
                choices.iter().enumerate().map(|(i, (_, s))| format!("  {}  {s}", i + 1)).collect::<Vec<_>>().join("\n")
            ));
            let answer = read_answer(io, "Choose 1–4: ").await;
            let index = number::parse(string::trim(&answer)).unwrap_or(f64::NAN) - 1.0;
            let chosen = (index.is_finite() && index.fract() == 0.0 && index >= 0.0).then(|| choices.get(index as usize)).flatten();
            if let Some((provider, _)) = chosen {
                let config = AppConfig::Login {
                    provider: *provider,
                    method: if *provider == ProviderId::OpenaiCodex { LoginMethod::Browser } else { LoginMethod::Key },
                    auth_file: auth_file.clone(),
                    pi_auth_file: pi_auth_file.clone(),
                    settings_file: settings_file.clone(),
                };
                login_with(&config, io).await?;
                Ok(0)
            } else {
                io.out.write("Nothing chosen; nothing changed.\n");
                Ok(1)
            }
        }
        AppConfig::Login { .. } => {
            login_with(&config, io).await?;
            Ok(0)
        }
        AppConfig::InferenceOnly { .. } | AppConfig::Live { .. } => {
            if io.installed() && matches!(config, AppConfig::Live { .. }) {
                install::finish_legacy_transition(&installed_io(io)).await;
            }
            session::run_session(io.clone(), config, secrets, factory).await
        }
    }
}
async fn update_and_reopen(io: &CliIo) -> Result<i32, RuntimeError> {
    let result = reopen_after_update(io).await;
    // An exception follows the source catch/normal-exit path instead.
    io.exit_after_reopen.set(result.is_ok());
    result
}
async fn reopen_after_update(io: &CliIo) -> Result<i32, RuntimeError> {
    io.input.pause();
    io.out.write("\n");
    let updated = if io.installed() {
        // The TUI's Ctrl-C handler is gone, but the process keeps its SIGINT disposition: Ctrl-C now
        // stops the download, as the source's default handler ended a stuck update.
        let cancel = kumi_common::abort::Signal::new();
        let stop = SignalTask(io.signals.then(|| {
            let cancel = cancel.clone();
            tokio::task::spawn_local(async move {
                if tokio::signal::ctrl_c().await.is_ok() {
                    cancel.cancel();
                }
            })
        }));
        let mut installed = installed_io(io);
        installed.cancel = Some(cancel);
        let updated = install::update_installed(installed).await;
        drop(stop);
        updated?
    } else {
        update::run_update(UpdateIo::new(io.out.clone(), io.env.clone())).await
    };
    if updated != 0 {
        io.out.write(&format!("\nKumi wasn't updated; this one still works: {}\n", io.command()));
        return Ok(updated);
    }
    io.out.write("\nOpening Kumi again…\n");
    reopen(io).await
}
/// Open Kumi again, with the same arguments and conversation.
async fn reopen(io: &CliIo) -> Result<i32, RuntimeError> {
    io.input.pause();
    if let Some(reopen) = &io.reopen {
        return Ok(reopen().await);
    }
    let exe = if io.installed() {
        std::path::Path::new(&install::kumi_home(&io.env)).join("app").join(bridge_setup::executable_name("kumi"))
    } else {
        std::env::current_exe().map_err(|e| RuntimeError::plain(e.to_string()))?
    };
    let ignored = io.signals.then(|| {
        tokio::task::spawn_local(async {
            loop {
                if tokio::signal::ctrl_c().await.is_err() {
                    break;
                }
            }
        })
    });
    let guard = SignalTask(ignored);
    let code = tokio::process::Command::new(exe).args(&io.args).envs(&io.env).status().await.map(|s| s.code().unwrap_or(1)).unwrap_or(1);
    drop(guard);
    Ok(code)
}
/// Bind the command loop to the process after supplying the native Ableton factory.
pub fn main_with(factory: AbletonFactory) -> i32 {
    // Before anything is written: Windows consoles need VT processing for Kumi's escape sequences.
    let _vt = crate::tui::tty::VtOutput::enable();
    let runtime = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
        Ok(runtime) => runtime,
        Err(error) => {
            eprintln!("Kumi: {error}");
            return 1;
        }
    };
    let mut io = CliIo::new(
        Rc::new(crate::tui::tty::Stdin::new()),
        Rc::new(crate::tui::tty::Stdout::new()),
        Rc::new(crate::tui::tty::Stdout::stderr()),
        kumi_common::env::vars(),
    );
    io.args = kumi_common::env::args().skip(1).collect();
    io.signals = true;
    if let Ok(executable) = std::env::current_exe() {
        if let Err(error) = install::ensure_native_launcher(&io.env, &executable) {
            io.err.write(&format!("Kumi: could not update its launcher: {error}\n"));
        }
    }
    let err = io.err.clone();
    let explicit_exit = io.exit_after_reopen.clone();
    let local = tokio::task::LocalSet::new();
    let code = local.block_on(&runtime, run(io, factory));
    shutdown::finish(runtime, local, code, explicit_exit.get(), err.as_ref())
}
