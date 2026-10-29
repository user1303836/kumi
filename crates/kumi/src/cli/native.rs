//! Command dispatch and native process I/O.
mod session;
mod shutdown;
use crate::{
    config::*,
    doctor::{self, DoctorIo},
    export::{self, ExportRequest},
    input::TerminalInput,
    install::{self, InstalledIo},
    library::{run_library, LibraryIo},
    login::{self, AuthIo, LoginIo},
    report::{write_report, ReportIo},
    spinner::step,
    tui::tty::TtyOutput,
    update::{self, CheckIo, UpdateIo},
};
use futures::{future::LocalBoxFuture, FutureExt};
use kumi_agent::core::export::Format;
use kumi_ai::providers::{table, SignIn};
use kumi_common::{
    abort::{self, Signal},
    js::{number, string},
};
use kumi_daw::{
    check::{Bundled, Probe, Probes},
    io::{Answers, DawIo},
    provider::{BridgeCommand, DawSession, WatchWords},
};
use kumi_types::{errors::RuntimeError, system::Env, version::KUMI_VERSION};
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};
#[derive(Clone)]
pub struct CliIo {
    pub input: Rc<dyn TerminalInput>,
    pub out: Rc<dyn TtyOutput>,
    pub err: Rc<dyn TtyOutput>,
    pub env: Env,
    pub args: Vec<String>,
    pub signals: bool,
    /// The process's own terminal, which the full screen asks what it is as it starts (`tui::probe`); tests' terminals
    /// aren't asked.
    pub probe: bool,
    /// Opens Kumi again with these arguments, in place of starting the program (tests).
    pub reopen: Option<Rc<dyn Fn(Vec<String>) -> LocalBoxFuture<'static, i32>>>,
    /// Which DAWs are on this computer, for /daw to offer (tests); else the DAWs' own detection says.
    pub daws_here: Option<crate::daw::Here>,
    /// The session on a thread of its own, whichever the front end (the app's). Tests build theirs in this one unless they
    /// ask for this, and one with stand-ins (`Answers`) always does.
    pub threaded: bool,
    exit_after_reopen: Rc<Cell<bool>>,
    /// The session's thread still had something running past its grace as it ended: the shutdown says so, once.
    left_running: Rc<Cell<bool>>,
}
impl CliIo {
    pub fn new(input: Rc<dyn TerminalInput>, out: Rc<dyn TtyOutput>, err: Rc<dyn TtyOutput>, env: Env) -> Self {
        Self {
            input,
            out,
            err,
            env,
            args: vec![],
            signals: false,
            probe: false,
            reopen: None,
            daws_here: None,
            threaded: false,
            exit_after_reopen: Rc::new(Cell::new(false)),
            left_running: Rc::new(Cell::new(false)),
        }
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
    first.push((
        cmd.into(),
        "Sign in, then open your DAW (Live, REAPER or Renoise), before or after Kumi: Kumi\nconnects to the one open and puts its bridge in the first time. Then talk about\nwhat's open; /login and /model change the sign-in and model.",
    ));
    first.push((format!("{cmd} bridge"), "Or, with Live closed: put the bridge into Live, or bring it up to date"));
    first.push((format!("{cmd} --daw live|reaper|renoise"), "Work with that DAW only, whichever DAW is open"));
    first.push((
        format!("{cmd} --daw reaper|renoise bridge"),
        "Put Kumi's extension into REAPER, or its tool into Renoise, or bring it up to date",
    ));
    let mut more=vec![("-c, --continue","Continue the conversation you had last, in any project"),("-r, --resume [id or name]","Continue a kept conversation by its id, the start of one, or its name;\nalone, choose one (both work with --inference-only too)"),("export [id or name]","Save a kept conversation as Markdown (--md, the default; --html a web page;\n--json a file another Kumi imports); your latest without an id or with --last.\n--full adds each step's input and result, -o <path> chooses where, --raw leaves\nkeys and paths in"),("import <file>","Keep a conversation saved with export --json, to carry on with -r"),("-p, --print <request>","Run one request, print its answer and end, for scripts; --json prints each\nevent Kumi sends as a JSON line instead. Exit code: 0 answered, 1 failed,\n3 stopped at its step limit, 130 stopped with ctrl+c"),("--inference-only","Chat without a DAW"),("--bridge-config <path>","Use this bridge configuration (an absolute path)"),("login","Sign in: asks whether with ChatGPT or an API key"),("login <provider>","Sign in to one provider: openai-codex with a ChatGPT plan (--device\nwithout a browser); anthropic, openai, opencode and the others auth lists with an\nAPI key (asked for)"),("logout <provider>","Remove Kumi's sign-in for that provider"),("model [<provider>/<model>]","Show or choose the model; ollama/<model> or lmstudio/<model> for one on this computer"),("auth","Show which providers are usable (no secrets)"),("doctor","Check the installation, sign-in, your DAWs and Kumi's bridges in them, and the terminal"),("library","What Kumi knows of your sounds, presets and projects (it learns them in the\nbackground); --rebuild learns them all again"),("update","Bring Kumi up to date, and its bridges in your DAWs when they're older"),("update --check","Say whether there's a newer Kumi, without installing it")];
    let doctor = more.iter().position(|(s, _)| *s == "doctor").map_or(more.len(), |at| at + 1);
    more.insert(doctor, ("doctor --json", "The doctor's checks as JSON, without keys, to share"));
    if installed {
        more.extend([
            ("update --rollback", "Go back to the Kumi you had before the last update"),
            ("update --version <x.y.z>", "Install that release, newer or older (update --rollback returns)"),
            ("uninstall", "Remove Kumi (your conversations and notes stay unless you say);\n--dry-run says what would go"),
        ]);
    }
    more.extend([("report", "Write a file to send when something goes wrong (no keys in it)"), ("--version", "Show Kumi's version")]);
    let more: Vec<_> = more.into_iter().map(|(s, text)| (format!("{cmd} {s}"), text)).collect();
    format!(
        "Kumi {KUMI_VERSION} — producer assistant for Ableton Live, REAPER and Renoise\n\nFirst run:\n{}\n\nMore:\n{}\n\n{}",
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
/// The doctor, looking through each DAW's bridge with its own probe (and this run's stand-ins), and comparing each
/// DAW's installed bridge with the one this Kumi brings: each DAW's checks ask for their own DAW's, whichever DAW the
/// doctor speaks for.
fn doctor_io(io: &CliIo, answers: &Answers) -> DoctorIo {
    let mut doctor = DoctorIo::new(io.out.clone(), io.env.clone());
    let (env, stand_ins) = (io.env.clone(), answers.clone());
    let probes = Probes(Rc::new(move |daw, config| match crate::daw::provider(daw) {
        Some(provider) => {
            let probe = provider.probe(config, &stand_ins, &env);
            async move { Ok(probe.await) }.boxed_local()
        }
        None => async { Ok(Probe::default()) }.boxed_local(),
    }));
    let bundled = Bundled(Rc::new(|daw| crate::daw::provider(daw).and_then(|provider| provider.bundled_bridge_version())));
    doctor.answers = answers.clone().with(probes).with(bundled);
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
pub async fn run(io: CliIo, answers: impl Into<Answers>) -> i32 {
    let answers = answers.into();
    io.exit_after_reopen.set(false);
    let mut secrets: Vec<String> =
        crate::report::KEY_VARIABLES.into_iter().filter_map(|s| io.env.get(s).filter(|s| !s.is_empty()).cloned()).collect();
    if let Ok(file) = load_settings_file(&io.env) {
        for server in read_settings(&file).model_servers {
            if let Some(key) = server.api_key.filter(|s| !s.is_empty()) {
                secrets.push(key);
            }
        }
    }
    match dispatch(&io, answers, &mut secrets).await {
        Ok(code) => code,
        Err(error) => {
            io.err.write(&format!("Kumi: {}\n", safe_error_message(Some(&error.message()), &secrets)));
            1
        }
    }
}
async fn dispatch(io: &CliIo, answers: Answers, secrets: &mut Vec<String>) -> Result<i32, RuntimeError> {
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
        AppConfig::Doctor { json, daw } => {
            // The doctor speaks in the words of the DAW asked for, else the one this producer works with (Live's, for Live).
            crate::daw::work_with(crate::daw::chosen(*daw, &io.env).0.id());
            let mut doctor = doctor_io(io, &answers);
            doctor.json = *json;
            doctor::run_doctor(doctor).await
        }
        AppConfig::Report { daw } => {
            crate::daw::work_with(crate::daw::chosen(*daw, &io.env).0.id());
            write_report(ReportIo::new(doctor_io(io, &answers))).await
        }
        AppConfig::Export { conversation, format, out, full, raw, daw } => {
            // In the words of the DAW asked for, else the one this producer works with, as the doctor's.
            crate::daw::work_with(crate::daw::chosen(*daw, &io.env).0.id());
            let format = match format.as_str() {
                "html" => Format::Html,
                "json" => Format::Json,
                _ => Format::Markdown,
            };
            let request = ExportRequest { conversation: conversation.clone(), format, out: out.clone(), full: *full, raw: *raw };
            export::export_command(request, &io.env, io.out.as_ref(), io.err.as_ref(), io.command()).await
        }
        AppConfig::Import { file } => export::import_command(file, &io.env, io.out.as_ref(), io.err.as_ref(), io.command()).await,
        AppConfig::Update { rollback, check, version } => {
            if *rollback && !io.installed() {
                io.out.write("This Kumi runs from a copy of its repository, so there's no earlier Kumi kept to go back to. Check out the commit you want with git, then run: cargo build --release --workspace\n");
                return Ok(1);
            }
            if let Some(version) = version {
                if !io.installed() {
                    io.out.write(&format!("This Kumi runs from a copy of its repository: check out that release with git (git checkout v{version}), then run: cargo build --release --workspace\n"));
                    return Ok(1);
                }
                let mut installed = installed_io(io);
                installed.version = Some(version.clone());
                return install::update_installed(installed).await;
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
        AppConfig::Uninstall { all, yes, dry_run } => {
            if io.installed() {
                install::uninstall_installed(installed_io(io), install::UninstallOptions { all: *all, yes: *yes, dry_run: *dry_run }).await
            } else {
                io.out
                    .write("This Kumi runs from a copy of its repository; delete that folder to remove it (your files are in ~/.kumi).\n");
                Ok(1)
            }
        }
        AppConfig::Library { rebuild, daw } => {
            crate::daw::work_with(crate::daw::chosen(*daw, &io.env).0.id());
            let (signal, _guard) = interrupt_signal(io);
            let mut options = LibraryIo::new(io.out.clone(), io.env.clone());
            options.rebuild = *rebuild;
            options.signal = Some(signal);
            run_library(options).await
        }
        AppConfig::Bridge { yes, allow_dirty, daw } => {
            let wait_ms = io
                .env
                .get("KUMI_BRIDGE_WAIT_SECONDS")
                .filter(|s| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()))
                .and_then(|s| s.parse::<u64>().ok())
                .map(|n| n.saturating_mul(1000));
            let command = BridgeCommand { yes: *yes, allow_dirty: *allow_dirty, wait_ms };
            let provider = daw.and_then(crate::daw::provider).unwrap_or_else(crate::daw::current);
            let daw = DawIo {
                out: Rc::new(crate::spinner::Terminal::new(io.out.clone(), &io.env)),
                ask: Rc::new(crate::process::TerminalAsk { input: Some(io.input.clone()), out: io.out.clone(), confirm: None }),
                env: io.env.clone(),
                run: kumi_daw::process::default_run(),
                answers: answers.clone(),
            };
            provider.setup_bridge(daw, command).await
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
            let choices: Vec<_> = table::first_choices()
                .into_iter()
                .map(|(provider, label)| {
                    let how = if provider.row().sign_in == SignIn::Chatgpt {
                        "sign in with your ChatGPT plan (opens your browser)"
                    } else {
                        "paste an API key"
                    };
                    (provider, format!("{label}: {how}"))
                })
                .collect();
            if !io.input.is_tty() {
                return Err(RuntimeError::plain(format!(
                    "Use: {} login <provider>, with provider one of {}.",
                    io.command(),
                    choices.iter().map(|(id, _)| id.as_str()).collect::<Vec<_>>().join(", ")
                )));
            }
            io.out.write(&format!(
                "How do you want to sign in?\n{}\nOther providers (OpenRouter, Gemini, DeepSeek and more): {} login <provider>; {} auth lists them.\n",
                choices.iter().enumerate().map(|(i, (_, s))| format!("  {}  {s}", i + 1)).collect::<Vec<_>>().join("\n"),
                io.command(),
                io.command()
            ));
            let answer = read_answer(io, &format!("Choose 1–{}: ", choices.len())).await;
            let index = number::parse(string::trim(&answer)).unwrap_or(f64::NAN) - 1.0;
            let chosen = (index.is_finite() && index.fract() == 0.0 && index >= 0.0).then(|| choices.get(index as usize)).flatten();
            if let Some((provider, _)) = chosen {
                let config = AppConfig::Login {
                    provider: *provider,
                    method: if provider.row().sign_in == SignIn::Chatgpt { chatgpt_sign_in(&io.env) } else { LoginMethod::Key },
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
                let installed = installed_io(io).daw();
                for provider in crate::daw::providers() {
                    provider.finish_transition(&installed).await;
                }
            }
            session::run_session(io.clone(), config, secrets, answers).await
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
        return Ok(reopen(io.args.clone()).await);
    }
    let exe = if io.installed() {
        std::path::Path::new(&install::kumi_home(&io.env)).join("app").join(kumi_daw::process::executable_name("kumi"))
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
/// Bind the command loop to the process, with answers for what each DAW would otherwise find out for itself (none, for
/// Kumi itself: each DAW's own integration).
pub fn main_with(answers: impl Into<Answers>) -> i32 {
    let answers = answers.into();
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
    io.probe = true;
    io.threaded = true;
    if let Ok(executable) = std::env::current_exe() {
        if let Err(error) = install::ensure_native_launcher(&io.env, &executable) {
            io.err.write(&format!("Kumi: could not update its launcher: {error}\n"));
        }
    }
    let err = io.err.clone();
    let explicit_exit = io.exit_after_reopen.clone();
    let left_running = io.left_running.clone();
    let local = tokio::task::LocalSet::new();
    let code = local.block_on(&runtime, run(io, answers));
    shutdown::finish(runtime, local, code, explicit_exit.get(), left_running.get(), err.as_ref())
}

#[cfg(test)]
mod doctor_io_tests {
    use super::*;
    use crate::input::ByteListener;
    use kumi_types::daw::DawId;

    struct Silent;
    impl TerminalInput for Silent {
        fn is_tty(&self) -> bool {
            false
        }
        fn is_raw(&self) -> bool {
            false
        }
        fn set_raw_mode(&self, _: bool) -> std::io::Result<()> {
            Ok(())
        }
        fn resume(&self, _: ByteListener) {}
        fn pause(&self) {}
        fn on_end(&self, _: Rc<dyn Fn()>) {}
    }
    struct Nowhere;
    impl TtyOutput for Nowhere {
        fn is_tty(&self) -> bool {
            false
        }
        fn columns(&self) -> Option<i32> {
            None
        }
        fn rows(&self) -> Option<i32> {
            None
        }
        fn write(&self, _: &str) {}
    }

    /// The doctor speaking for REAPER (used last) still probes Live for Live's checks, and judges Live's bridge by
    /// Live's version, never REAPER's.
    #[tokio::test(flavor = "current_thread")]
    async fn lives_checks_probe_live_whichever_daw_the_doctor_speaks_for() {
        tokio::task::LocalSet::new().run_until(probe_live_as_reaper()).await;
    }
    async fn probe_live_as_reaper() {
        crate::daw::work_with(DawId::Reaper);
        let probed = Rc::new(Cell::new(0));
        let count = probed.clone();
        let factory = kumi_daw_ableton::app::AbletonFactory(Rc::new(move |options| {
            count.set(count.get() + 1);
            kumi_daw_ableton::create_ableton_integration(options)
        }));
        let io = CliIo::new(Rc::new(Silent), Rc::new(Nowhere), Rc::new(Nowhere), Env::new());
        let doctor = doctor_io(&io, &Answers::default().with(factory));
        let live = crate::daw::provider(DawId::Ableton).unwrap();
        let bundled = doctor.answers.get::<Bundled>().unwrap();
        assert_eq!((bundled.0)(DawId::Ableton), live.bundled_bridge_version());
        let probe = doctor.answers.get::<Probes>().unwrap();
        let _ = (probe.0)(DawId::Ableton, "/nowhere/bridge-config.json".into()).await;
        assert_eq!(probed.get(), 1, "Live's probe made Live's integration");
        crate::daw::work_with(DawId::Ableton);
    }

    /// Review K42: the doctor starts Kumi's host for REAPER or Renoise once (through the session's host factory), and
    /// a host that won't start says why.
    #[tokio::test(flavor = "current_thread")]
    async fn the_doctor_starts_kumis_host_for_reaper_and_renoise() {
        tokio::task::LocalSet::new().run_until(probe_own_hosts()).await;
    }
    async fn probe_own_hosts() {
        let asked = Rc::new(std::cell::RefCell::new(vec![]));
        let seen = asked.clone();
        let hosts = kumi_daw_own::provider::OwnHosts(Rc::new(move |daw| {
            seen.borrow_mut().push(daw);
            Rc::new(|_| async { Err(RuntimeError::plain("daw-mcp-server wasn't found")) }.boxed_local())
        }));
        let io = CliIo::new(Rc::new(Silent), Rc::new(Nowhere), Rc::new(Nowhere), Env::new());
        let doctor = doctor_io(&io, &Answers::default().with(hosts));
        let probe = doctor.answers.get::<Probes>().expect("the doctor probes Kumi's own hosts").clone();
        for daw in [DawId::Reaper, DawId::Renoise] {
            let host = (probe.0)(daw, String::new()).await.unwrap();
            assert!(!host.started && host.said.as_deref().is_some_and(|said| said.contains("daw-mcp-server wasn't found")), "{host:?}");
        }
        assert_eq!(*asked.borrow(), [DawId::Reaper, DawId::Renoise]);
    }
}
