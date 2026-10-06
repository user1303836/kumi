//! Session, terminal and background-library ownership from the CLI.
use super::*;
use crate::{
    history::open_input_history,
    models::{create_model_control, ModelControlOptions},
    terminal::{create_terminal, Terminal, TerminalOptions},
    tui::app::{create_tui, ConnectLive, LiveSetup, PanelTab, TuiOptions},
    voice::{create_voice_control, VoiceControlOptions},
};
use async_trait::async_trait;
use kumi_runtime::{
    auth::store::{open_credential_store, Credential, CredentialStore},
    core::{
        contracts::*,
        memory::MemoryStoreOptions,
        session::VideoDirectories,
        store_backed::{SqliteMemoryStore, SqlitePlaybookStore, SqliteTechniqueStore},
        store_client::StoreClient,
    },
    library::{create_library, LibraryOptions},
    video::programs::{configure_programs, ProgramDefaults},
    *,
};
use std::{cell::Cell, rc::Weak, sync::Arc};
struct UnavailableKernel {
    error: RuntimeError,
    checkpoint: Option<KernelCheckpoint>,
}
#[async_trait(?Send)]
impl Kernel for UnavailableKernel {
    async fn run(&self, _: &str, _: Signal, _: KernelEmit) -> Result<TurnResult, RuntimeError> {
        Err(self.error.clone())
    }
    async fn close(&self) {}
    fn has_checkpoint(&self) -> bool {
        self.checkpoint.is_some()
    }
    fn checkpoint(&self) -> Result<KernelCheckpoint, RuntimeError> {
        self.checkpoint.clone().ok_or_else(|| RuntimeError::plain("No checkpoint"))
    }
}
/// Live's side of first-run setup, for the app: Live the app, Kumi's bridge in it, and the session
/// connecting through the bridge once it's in place.
struct LiveSide {
    env: Env,
    run: bridge_setup::Run,
    /// The bridge configuration the session's integration starts from, and whether it was given
    /// (`--bridge-config`), so the one put in place doesn't replace it.
    bridge: Rc<RefCell<Option<String>>>,
    given: bool,
    controller: Rc<RefCell<Option<Weak<dyn SessionController>>>>,
}
#[async_trait(?Send)]
impl LiveSetup for LiveSide {
    async fn open_live(&self) -> Option<String> {
        live_app::open_live(&self.run, kumi_runtime::system::platform(), &self.env).await.map(|live| live.app)
    }
    async fn ask_to_quit(&self) {
        live_app::ask_to_quit(&self.run, kumi_runtime::system::platform(), &self.env).await
    }
    async fn closed(&self, stop: &Signal) -> bool {
        live_app::closed(&self.run, kumi_runtime::system::platform(), &self.env, stop).await
    }
    async fn install(&self) -> Result<String, String> {
        let mut options = BridgeSetupIo::quiet(self.env.clone());
        options.run = Some(self.run.clone());
        bridge_setup::install_quietly(options).await
    }
    async fn start(&self, app: Option<String>) -> bool {
        live_app::start(&self.run, kumi_runtime::system::platform(), &self.env, app.as_deref()).await
    }
    async fn connect(&self) {
        if !self.given {
            if let Some(found) = find_bridge_config(&self.env) {
                *self.bridge.borrow_mut() = Some(found);
            }
        }
        // The session may still be starting: a busy one is asked again shortly.
        for _ in 0..40 {
            let session = self.controller.borrow().as_ref().and_then(Weak::upgrade);
            let Some(session) = session else { return };
            if session.reconnect().await.is_ok() {
                return;
            }
            drop(session);
            tokio::time::sleep(std::time::Duration::from_millis(250)).await;
        }
    }
    async fn version(&self, app: Option<String>) -> Option<String> {
        live_app::version(&self.run, kumi_runtime::system::platform(), &self.env, app.as_deref()).await
    }
}

pub(super) async fn probe_live(config: String, factory: AbletonFactory) -> doctor::LiveProbe {
    let mut options = AbletonOptions::new(Rc::new(|_, _| {}));
    options.bridge_config = Some(config);
    let integration = factory(options);
    if integration.start(abort::timeout(20000)).await.is_err() {
        let _ = integration.close().await;
        return doctor::LiveProbe { started: false, ..Default::default() };
    }
    let result = async {
        let observation = integration.observe(abort::timeout(20000), None).await?;
        let context: serde_json::Value = serde_json::from_str(&observation.context).map_err(|e| RuntimeError::plain(e.to_string()))?;
        if context["mode"] == "inference-only" || observation.tools.is_empty() {
            return Ok::<_, RuntimeError>(doctor::LiveProbe { started: true, connected: Some(false), ..Default::default() });
        }
        Ok(doctor::LiveProbe {
            started: true,
            connected: Some(true),
            live_version: context["liveVersion"].as_str().map(str::to_string),
            set: context["set"]["name"].as_str().map(str::to_string),
            real_live: Some(context["provenance"] == "real-live"),
        })
    }
    .await;
    let _ = integration.close().await;
    result.unwrap_or(doctor::LiveProbe { started: true, connected: Some(false), ..Default::default() })
}
/// How the watch for Live paces itself: a look at Live's port every `look_ms`, and while it answers
/// but the session isn't connected, a reconnect at once and then every `retry_ms`, at most `tries`
/// times until it stops answering (then the count starts again). It ends after `looks`.
struct Pace {
    look_ms: u64,
    retry_ms: u64,
    tries: u32,
    looks: u32,
}
const PACE: Pace = Pace { look_ms: 2_000, retry_ms: 20_000, tries: 10, looks: 1_800 };

/// Live answers once it's open with AbletonMcpBridge chosen as a Control Surface: then the session
/// reconnects by itself. Live can accept connections before it's ready (a dialog open, a Set still
/// loading), so a failed reconnect is tried again while it answers. Watching ends once connected,
/// after an hour, or when the session has gone.
async fn connect_when_live_answers<F, A>(
    answers: F,
    controller: Rc<RefCell<Option<Weak<dyn SessionController>>>>,
    emit: Rc<dyn Fn(SessionEvent)>,
    watching: Rc<Cell<bool>>,
    pace: Pace,
) where
    F: Fn() -> A,
    A: std::future::Future<Output = bool>,
{
    let mut tries = 0;
    let mut next: Option<std::time::Instant> = Some(std::time::Instant::now());
    for _ in 0..pace.looks {
        let Some(session) = controller.borrow().as_ref().and_then(Weak::upgrade) else { break };
        if session.status().connection == ConnectionState::Connected {
            break;
        }
        if answers().await {
            if tries < pace.tries && next.is_some_and(|at| std::time::Instant::now() >= at) {
                tries += 1;
                // A turn under way makes this fail; it's tried again later.
                if session.has_reconnect() && session.reconnect().await.is_ok() && session.status().connection == ConnectionState::Connected
                {
                    emit(SessionEvent::Notice { message: "Live answered, so Kumi is connected to it now.".into() });
                    break;
                }
                next = Some(std::time::Instant::now() + std::time::Duration::from_millis(pace.retry_ms));
            }
        } else {
            tries = 0;
            next = Some(std::time::Instant::now());
        }
        drop(session);
        tokio::time::sleep(std::time::Duration::from_millis(pace.look_ms)).await;
    }
    watching.set(false);
}
pub(super) async fn run_session(
    io: CliIo,
    config: AppConfig,
    secrets: &mut Vec<String>,
    factory: AbletonFactory,
) -> Result<i32, RuntimeError> {
    let (inference, bridge_config, bridge_missing) = match config {
        AppConfig::InferenceOnly { inference, bridge_missing } => (inference, None, bridge_missing.unwrap_or(false)),
        AppConfig::Live { inference, bridge_config } => (inference, Some(bridge_config), false),
        _ => unreachable!(),
    };
    let mode = if bridge_config.is_some() { "live" } else { "inference-only" };
    let store = Rc::new(open_credential_store(&inference.auth_file));
    for credential in store.list().await.unwrap_or_default().values() {
        match credential {
            Credential::Oauth(c) => {
                secrets.push(c.access.clone());
                secrets.push(c.refresh.clone());
            }
            Credential::ApiKey { key } => secrets.push(key.clone()),
        }
    }
    let terminal: Rc<RefCell<Option<Weak<dyn Terminal>>>> = Rc::new(RefCell::new(None));
    let emit: Rc<dyn Fn(SessionEvent)> = {
        let terminal = terminal.clone();
        Rc::new(move |event| {
            let terminal = terminal.borrow().as_ref().and_then(Weak::upgrade);
            if let Some(terminal) = terminal {
                terminal.handle_event(event)
            }
        })
    };
    let settings_file = load_settings_file(&io.env)?;
    let settings = read_settings(&settings_file);
    let projects_dir = load_projects_dir(&io.env)?;
    let tools_dir = load_tools_dir(&io.env)?;
    let library = create_library(LibraryOptions {
        dir: load_library_dir(&io.env)?,
        folders: Some(settings.library_folders),
        projects_dir: Some(projects_dir.clone()),
        ..Default::default()
    });
    let controller: Rc<RefCell<Option<Weak<dyn SessionController>>>> = Rc::new(RefCell::new(None));
    let models = Rc::new(create_model_control(ModelControlOptions {
        store,
        settings_file: settings_file.clone(),
        env: io.env.clone(),
        changed: {
            let controller = controller.clone();
            Rc::new(move || {
                let controller = controller.borrow().as_ref().and_then(Weak::upgrade);
                async move {
                    if let Some(controller) = controller {
                        controller.reconfigure().await?;
                    }
                    Ok(())
                }
                .boxed_local()
            })
        },
        say: Some({
            let emit = emit.clone();
            Rc::new(move |message| emit(SessionEvent::Notice { message }))
        }),
        fetch: None,
        installed: None,
    }));
    let kernel_factory: KernelFactory = {
        let models = models.clone();
        Rc::new(move |options| {
            let models = models.clone();
            async move {
                let checkpoint = options.checkpoint.clone();
                let result = async {
                    let binding = models.binding().await?;
                    Ok::<_, RuntimeError>(Rc::new(create_agent_kernel(AgentKernelOptions {
                        conversation: options.conversation,
                        instructions: options.instructions,
                        tools: options.tools,
                        signal: options.signal,
                        checkpoint: options.checkpoint,
                        binding,
                        max_steps: None,
                        budget: None,
                    })?) as Rc<dyn Kernel>)
                }
                .await;
                match result {
                    Err(error) if error.kumi().is_some() => Ok(Rc::new(UnavailableKernel { error, checkpoint }) as Rc<dyn Kernel>),
                    other => other,
                }
            }
            .boxed_local()
        })
    };
    let bundled = bridge_setup::bundled_bridge_version();
    let project_store = create_project_store(&projects_dir);
    let restore_file = load_restore_file(&io.env)?;
    let user_library = live_user_library(&io.env);
    let trace = io.env.get("KUMI_TRACE").is_some_and(|s| s == "1");
    // Watching for Live to answer, so a chat without Live connects by itself once it does.
    let watching_live = Rc::new(Cell::new(false));
    // Where the session finds Live: first-run setup sets it once the bridge is in place.
    let live_config = Rc::new(RefCell::new(bridge_config.clone()));
    let integration_factory: IntegrationFactory = {
        let watching_live = watching_live.clone();
        let library = library.clone();
        let emit = emit.clone();
        let controller = controller.clone();
        let live_config = live_config.clone();
        let bundled = bundled.clone();
        Box::new(move |on_connection| {
            let Some(bridge_config) = live_config.borrow().clone() else {
                return create_inference_only_integration(Rc::new(move |state| on_connection(state, None)));
            };
            let mut options = AbletonOptions::new(on_connection.clone());
            options.bridge_config = Some(bridge_config.clone());
            options.restore_file = Some(restore_file.clone());
            options.project_store = Some(project_store.clone());
            options.user_library = user_library.clone();
            options.on_focus = Some({
                let emit = emit.clone();
                Rc::new(move |focus| emit(SessionEvent::Focus { focus }))
            });
            options.on_pointed = Some({
                let emit = emit.clone();
                Rc::new(move |pin| emit(SessionEvent::Pointed { pin }))
            });
            options.on_transport = Some({
                let emit = emit.clone();
                let library = library.clone();
                Rc::new(move |transport| {
                    if transport.as_ref().is_some_and(|t| t.playing) {
                        library.pause();
                    } else {
                        library.resume();
                    }
                    emit(SessionEvent::Transport { transport });
                })
            });
            options.on_change = Some({
                let emit = emit.clone();
                let controller = controller.clone();
                Rc::new(move |change| {
                    let controller = controller.borrow().as_ref().and_then(Weak::upgrade);
                    if let Some(c) = controller {
                        c.watch(WatchEvent::Change(change.clone()));
                    }
                    emit(SessionEvent::Change { change });
                })
            });
            options.on_action = Some({
                let emit = emit.clone();
                let controller = controller.clone();
                Rc::new(move |action| {
                    let controller = controller.borrow().as_ref().and_then(Weak::upgrade);
                    if let Some(c) = controller {
                        c.watch(WatchEvent::Action(action.clone()));
                    }
                    emit(SessionEvent::Action(action));
                })
            });
            options.on_watch = Some({
                let emit = emit.clone();
                Rc::new(move |on| emit(SessionEvent::Watching { on }))
            });
            options.on_audition = Some({
                let emit = emit.clone();
                let controller = controller.clone();
                Rc::new(move |event| {
                    let controller = controller.borrow().as_ref().and_then(Weak::upgrade);
                    if let Some(c) = controller {
                        c.watch(WatchEvent::Audition(event.clone()));
                    }
                    emit(SessionEvent::Auditioned(event));
                })
            });
            options.on_catch_up = Some({
                let emit = emit.clone();
                Rc::new(move |catch_up| emit(SessionEvent::CatchUp { catch_up }))
            });
            if trace {
                options.on_dispatch = Some({
                    let emit = emit.clone();
                    Rc::new(move |name| emit(SessionEvent::Notice { message: format!("[MCP dispatch] {name}") }))
                });
            }
            let emit = emit.clone();
            let bundled = bundled.clone();
            let controller = controller.clone();
            let watching_live = watching_live.clone();
            with_fallback(
                factory(options),
                Rc::new(move || {
                    let on_connection = on_connection.clone();
                    create_inference_only_integration(Rc::new(move |state| on_connection(state, None)))
                }),
                Rc::new(move |message| {
                    let installed = doctor::read_bridge_server(&bridge_config).ok().and_then(|s| s.version);
                    let current = installed.is_some() && installed == bundled;
                    if !current {
                        emit(SessionEvent::Notice { message });
                    } else if !watching_live.replace(true) {
                        // Said once: the watch's own retries fall back here too, quietly.
                        emit(SessionEvent::Notice {
                            message: "Kumi's bridge couldn't reach Live, so this is chat without Live. Open Live and choose AbletonMcpBridge as a Control Surface (Settings → Link, Tempo & MIDI); if Live is showing a dialog, answer it. Kumi connects by itself once Live answers.".into(),
                        });
                        let config = bridge_config.clone();
                        tokio::task::spawn_local(connect_when_live_answers(
                            move || {
                                let config = config.clone();
                                async move { bridge_setup::remote_script_answers(&config).await }
                            },
                            controller.clone(),
                            emit.clone(),
                            watching_live.clone(),
                            PACE,
                        ));
                    }
                }),
            )
        })
    };
    let mut options = SessionOptions::new(kernel_factory, integration_factory, emit.clone());
    // Live may come later in this session, once first-run setup has put the bridge in place.
    let live_possible = bridge_config.is_some() || bridge_missing;
    if live_possible {
        options.conversations = Some(create_conversation_store(&projects_dir));
    }
    // Kumi's database keeps notes, techniques, lessons and gaps, with what earlier Kumis kept in files
    // read in. When it can't open, they stay in their files this time, as before, and Kumi says so once.
    let database = StoreClient::open(load_db_file(&io.env)?.into(), json_files(&io.env)?, kumi_common::time::now_ms()).await;
    let database_notice = match &database {
        Ok((_, Ok(imported))) => Some(imported.sentences().join(" ")).filter(|said| !said.is_empty()),
        Ok((_, Err(why))) => {
            Some(format!("Kumi couldn't read in the notes and techniques kept in files this time ({why}); it tries again next start."))
        }
        Err(why) => Some(format!("Your notes, techniques and lessons stay in their files this time: {why}.")),
    };
    let database = database.ok().map(|(client, _)| client);
    options.memory = Some(match &database {
        Some(client) => Rc::new(SqliteMemoryStore::new(client.clone())) as Rc<dyn MemoryStore>,
        None => create_memory_store(MemoryStoreOptions {
            projects_dir: projects_dir.clone().into(),
            producer_file: load_memory_file(&io.env)?.into(),
        }),
    });
    options.listen = true;
    options.recipes = Some(create_recipe_store(load_recipes_dir(&io.env)?));
    options.techniques = Some(match &database {
        Some(client) => Rc::new(SqliteTechniqueStore::new(client.clone())) as Rc<dyn TechniqueStore>,
        None => create_technique_store(load_techniques_file(&io.env)?),
    });
    options.playbook = Some(match &database {
        Some(client) => Rc::new(SqlitePlaybookStore::new(client.clone())) as Rc<dyn PlaybookStore>,
        None => create_playbook_store(load_playbook_file(&io.env)?),
    });
    options.store = database;
    options.goals = Some(create_goal_store(load_goals_dir(&io.env)?));
    options.gaps = Some(load_gaps_file(&io.env)?);
    options.timings = Some(load_timings_file(&io.env)?);
    options.watch = Some(VideoDirectories { videos_dir: load_videos_dir(&io.env)?, tools_dir: tools_dir.clone() });
    options.web = true;
    options.library = Some(library.clone());
    let session: Rc<dyn SessionController> = Rc::new(create_session(options)?);
    *controller.borrow_mut() = Some(Rc::downgrade(&session));
    let (fetch_tx, mut fetch_rx) = tokio::sync::mpsc::unbounded_channel::<String>();
    configure_programs(ProgramDefaults {
        tools_dir: Some(tools_dir.clone()),
        on_fetch: Some(Arc::new(move |message| {
            let _ = fetch_tx.send(message.to_string());
        })),
    });
    let fetch_notices = {
        let emit = emit.clone();
        tokio::task::spawn_local(async move {
            while let Some(message) = fetch_rx.recv().await {
                emit(SessionEvent::Notice { message })
            }
        })
    };
    // A switch from a JavaScript bridge with the same Remote Script changes only the host, which Kumi
    // already runs natively: nothing to ask of the producer (a start with Live closed finishes it).
    let stale = bridge_config
        .as_ref()
        .and_then(|_| update::older_bridge(&io.env, bundled.as_deref()))
        .filter(|stale| !(stale.runtime_migration && crate::install::only_the_host_differs(&io.env)));
    let bridge_notice = if bridge_missing {
        Some(format!("The Ableton bridge isn't installed yet, so Kumi can't see Live; chatting without it. To connect Live, quit Live and run: {} bridge",io.command()))
    } else {
        stale.as_ref().map(|stale| if stale.runtime_migration {
            format!("The bridge in Live still uses JavaScript. Quit Kumi and Live, then run: {} bridge to switch to the native bridge.", io.command())
        } else {
            format!("The bridge in Live is {}, older than this Kumi's ({}), so some changes aren't offered. Quit Kumi and Live, then run: {} update",stale.installed,stale.bundled,io.command())
        })
    };
    let notice = match (bridge_notice, database_notice) {
        (Some(bridge), Some(database)) => Some(format!("{bridge} {database}")),
        (bridge, database) => bridge.or(database),
    };
    // In the app, first-run setup puts the bridge in place instead, with Live restarting around it.
    let connect_why = if bridge_missing {
        Some("The Ableton bridge isn't in Live yet, so Kumi can't see your Set.".to_string())
    } else {
        stale.as_ref().map(|stale| {
            if stale.runtime_migration {
                "The bridge in Live still runs on JavaScript.".to_string()
            } else {
                format!("The bridge in Live is {}, older than Kumi's {}, so some changes aren't offered.", stale.installed, stale.bundled)
            }
        })
    };
    let update_after = Rc::new(Cell::new(false));
    let updates = update::UpdateControl {
        current: KUMI_VERSION.into(),
        check: {
            let env = io.env.clone();
            let installed = io.installed();
            Rc::new(move || {
                let env = env.clone();
                async move {
                    if installed {
                        install::check_release(&env, None).await
                    } else {
                        update::check_checkout(CheckIo::default()).await
                    }
                }
                .boxed_local()
            })
        },
        request: {
            let update_after = update_after.clone();
            Rc::new(move || update_after.set(true))
        },
    };
    let history = Rc::new(RefCell::new(open_input_history(Some(load_input_history_file(&io.env)?.into()), secrets.clone())));
    let full_screen = io.input.is_tty() && io.out.is_tty() && io.env.get("KUMI_UI").map(String::as_str) != Some("plain");
    let ui: Rc<dyn Terminal> = if full_screen {
        let mut options = TuiOptions::new(session, io.input.clone(), io.out.clone(), mode);
        options.models = Some(models);
        options.startup_notice = if connect_why.is_some() { None } else { notice };
        options.connect_live = live_possible.then(|| ConnectLive {
            why: connect_why,
            bridge: bundled.clone().unwrap_or_default(),
            live: Rc::new(LiveSide {
                env: io.env.clone(),
                run: bridge_setup::default_run(),
                bridge: live_config.clone(),
                given: io.args.first().is_some_and(|arg| arg == "--bridge-config"),
                controller: controller.clone(),
            }),
        });
        options.secrets = secrets.clone();
        options.history = Some(history);
        options.open_browser = Some(Rc::new(login::open_browser));
        options.updates = Some(updates);
        options.voice = Some(Rc::new(create_voice_control(VoiceControlOptions {
            env: Some(io.env.clone()),
            tools_dir,
            settings_file: settings_file.clone(),
            open: Some(Rc::new(login::open_browser)),
            platform: None,
        })));
        options.panel_tab = Some(PanelTab {
            load: {
                let settings_file = settings_file.clone();
                Rc::new(move || read_settings(&settings_file).panel_tab)
            },
            save: {
                let settings_file = settings_file.clone();
                Rc::new(move |id| {
                    let mut settings = read_settings(&settings_file);
                    settings.panel_tab = Some(id.into());
                    let _ = write_settings(&settings_file, &serde_json::to_value(&settings).unwrap());
                })
            },
        });
        Rc::new(create_tui(options))
    } else {
        let mut options = TerminalOptions::new(session, io.input.clone(), io.out.clone(), models, mode);
        options.startup_notice = notice;
        options.secrets = secrets.clone();
        options.history = Some(history);
        options.updates = Some(updates);
        Rc::new(create_terminal(options))
    };
    *terminal.borrow_mut() = Some(Rc::downgrade(&ui));
    let interrupt = SignalTask(io.signals.then(|| {
        let ui = ui.clone();
        tokio::task::spawn_local(async move {
            loop {
                if tokio::signal::ctrl_c().await.is_err() {
                    break;
                }
                ui.interrupt();
            }
        })
    }));
    #[cfg(unix)]
    let terminate = SignalTask(if io.signals {
        let ui = ui.clone();
        let mut signal =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()).map_err(|e| RuntimeError::plain(e.to_string()))?;
        Some(tokio::task::spawn_local(async move {
            if signal.recv().await.is_some() {
                ui.close().await;
            }
        }))
    } else {
        None
    });
    let running = ui.run();
    library.start();
    let settings = read_settings(&settings_file);
    let update_check = if settings.update_check != Some(false) && !io.env.get("KUMI_NO_UPDATE_CHECK").is_some_and(|s| !s.is_empty()) {
        let cache = std::path::Path::new(&settings_file)
            .parent()
            .unwrap_or(std::path::Path::new("."))
            .join("update-check.json")
            .to_string_lossy()
            .into_owned();
        let env = io.env.clone();
        let installed = io.installed();
        let ui = ui.clone();
        Some(tokio::task::spawn_local(async move {
            let latest = if installed {
                install::newer_release(&cache, &env, None, None).await
            } else {
                update::newer_kumi(CheckIo { cache_file: cache, ..Default::default() }).await
            };
            if let Some(latest) = latest {
                ui.offer_update(&latest);
            }
        }))
    } else {
        None
    };
    let code = running.await;
    drop(interrupt);
    #[cfg(unix)]
    drop(terminate);
    library.close().await;
    fetch_notices.abort();
    configure_programs(ProgramDefaults::default());
    if let Some(task) = update_check {
        task.abort();
    }
    terminal.borrow_mut().take();
    controller.borrow_mut().take();
    if update_after.get() {
        super::update_and_reopen(&io).await
    } else {
        Ok(code)
    }
}

#[cfg(test)]
mod watch_tests {
    use super::*;
    use kumi_runtime::core::contracts::{ChangeRecord, PinnedNode, SessionStatus, TurnState};

    struct Fake {
        reconnects: Cell<u32>,
        connect_on: u32,
        connected: Cell<bool>,
    }
    #[async_trait(?Send)]
    impl SessionController for Fake {
        async fn start(&self) -> Result<(), RuntimeError> {
            Ok(())
        }
        async fn submit(&self, _: &str, _: Option<PinnedNode>) -> Result<(), RuntimeError> {
            Ok(())
        }
        async fn refresh(&self) -> Result<(), RuntimeError> {
            Ok(())
        }
        async fn new_conversation(&self) -> Result<(), RuntimeError> {
            Ok(())
        }
        async fn cancel(&self) -> Result<(), RuntimeError> {
            Ok(())
        }
        async fn close(&self) -> Result<(), RuntimeError> {
            Ok(())
        }
        fn status(&self) -> SessionStatus {
            SessionStatus {
                state: TurnState::Idle,
                connection: if self.connected.get() { ConnectionState::Connected } else { ConnectionState::Disconnected },
                turns: 0,
                max_turns: None,
                observation: None,
            }
        }
        async fn undo(&self, _: Option<&str>) -> Result<Option<ChangeRecord>, RuntimeError> {
            Ok(None)
        }
        fn has_reconnect(&self) -> bool {
            true
        }
        async fn reconnect(&self) -> Result<(), RuntimeError> {
            self.reconnects.set(self.reconnects.get() + 1);
            if self.reconnects.get() >= self.connect_on {
                self.connected.set(true);
            }
            Ok(())
        }
    }
    async fn watch(connect_on: u32, answers: bool) -> (u32, Vec<String>, bool) {
        let fake = Rc::new(Fake { reconnects: Cell::new(0), connect_on, connected: Cell::new(false) });
        let session: Rc<dyn SessionController> = fake.clone();
        let controller = Rc::new(RefCell::new(Some(Rc::downgrade(&session))));
        let notices = Rc::new(RefCell::new(Vec::new()));
        let emit: Rc<dyn Fn(SessionEvent)> = {
            let notices = notices.clone();
            Rc::new(move |event| {
                if let SessionEvent::Notice { message } = event {
                    notices.borrow_mut().push(message)
                }
            })
        };
        let watching = Rc::new(Cell::new(true));
        let pace = Pace { look_ms: 1, retry_ms: 0, tries: 10, looks: 200 };
        connect_when_live_answers(move || async move { answers }, controller, emit, watching.clone(), pace).await;
        let notices = notices.borrow().clone();
        (fake.reconnects.get(), notices, watching.get())
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_given_bridge_config_stays_once_setup_puts_the_bridge_in_place() {
        let dir = tempfile::tempdir().unwrap();
        let reference = dir.path().join("Remote Scripts/AbletonMcpBridge");
        std::fs::create_dir_all(&reference).unwrap();
        let found = dir.path().join("bridge-config.json");
        std::fs::write(&found, "{}").unwrap();
        std::fs::write(reference.join("bridge-reference.json"), serde_json::json!({ "config": found }).to_string()).unwrap();
        let env = Env::from([("KUMI_REMOTE_SCRIPTS_DIR".to_string(), dir.path().join("Remote Scripts").display().to_string())]);
        let side = |given| LiveSide {
            env: env.clone(),
            run: bridge_setup::default_run(),
            bridge: Rc::new(RefCell::new(Some("/given/bridge-config.json".into()))),
            given,
            controller: Rc::new(RefCell::new(None)),
        };
        let given = side(true);
        given.connect().await;
        assert_eq!(given.bridge.borrow().as_deref(), Some("/given/bridge-config.json"), "--bridge-config stays");
        let looked_up = side(false);
        looked_up.connect().await;
        assert_eq!(looked_up.bridge.borrow().as_deref(), found.to_str(), "the one put in place");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_live_already_answering_is_reconnected_and_tried_again_until_connected() {
        let (reconnects, notices, watching) = watch(3, true).await;
        assert_eq!(reconnects, 3, "at once, then again while Live answers and the session isn't connected");
        assert_eq!(notices, ["Live answered, so Kumi is connected to it now."]);
        assert!(!watching);
        // A Live that answers but never connects gets a bounded number of tries.
        let (reconnects, notices, _) = watch(u32::MAX, true).await;
        assert_eq!(reconnects, 10);
        assert!(notices.is_empty());
        // A Live that never answers isn't asked at all.
        assert_eq!(watch(1, false).await.0, 0);
    }
}
