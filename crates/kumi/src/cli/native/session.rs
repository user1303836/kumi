//! Session, terminal and background-library ownership from the CLI.
use super::*;
use crate::{
    history::open_input_history,
    models::{create_model_control, ModelControlOptions},
    terminal::{create_terminal, Terminal, TerminalOptions},
    tui::app::{create_tui, PanelTab, TuiOptions},
    voice::{create_voice_control, VoiceControlOptions},
};
use async_trait::async_trait;
use kumi_runtime::{
    auth::store::{open_credential_store, Credential, CredentialStore},
    core::{contracts::*, memory::MemoryStoreOptions, session::VideoDirectories},
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
    let bundled = bridge_setup::bridge_version(&bridge_setup::bundled_bridge_dir());
    let project_store = create_project_store(&projects_dir);
    let restore_file = load_restore_file(&io.env)?;
    let user_library = live_user_library(&io.env);
    let trace = io.env.get("KUMI_TRACE").is_some_and(|s| s == "1");
    let integration_factory: IntegrationFactory = {
        let library = library.clone();
        let emit = emit.clone();
        let controller = controller.clone();
        let bridge_config = bridge_config.clone();
        let bundled = bundled.clone();
        Box::new(move |on_connection| {
            let Some(bridge_config) = bridge_config.clone() else {
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
            with_fallback(
                factory(options),
                Rc::new(move || {
                    let on_connection = on_connection.clone();
                    create_inference_only_integration(Rc::new(move |state| on_connection(state, None)))
                }),
                Rc::new(move |message| {
                    let installed = doctor::read_bridge_server(&bridge_config).ok().and_then(|s| s.version);
                    let current = installed.is_some() && installed == bundled;
                    emit(SessionEvent::Notice {
                        message: if current {
                            "Kumi's bridge couldn't reach Live, so this is chat without Live. Open Live and choose AbletonMcpBridge as a Control Surface (Settings → Link, Tempo & MIDI); if Live is showing a dialog, answer it. Then /reconnect.".into()
                        } else {
                            message
                        },
                    })
                }),
            )
        })
    };
    let mut options = SessionOptions::new(kernel_factory, integration_factory, emit.clone());
    if bridge_config.is_some() {
        options.conversations = Some(create_conversation_store(&projects_dir));
    }
    options.memory = Some(create_memory_store(MemoryStoreOptions {
        projects_dir: projects_dir.clone().into(),
        producer_file: load_memory_file(&io.env)?.into(),
    }));
    options.listen = true;
    options.recipes = Some(create_recipe_store(load_recipes_dir(&io.env)?));
    options.techniques = Some(create_technique_store(load_techniques_file(&io.env)?));
    options.playbook = Some(create_playbook_store(load_playbook_file(&io.env)?));
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
    let notice = if bridge_missing {
        Some(format!("The Ableton bridge isn't installed yet, so Kumi can't see Live; chatting without it. To connect Live, quit Live and run: {} bridge",io.command()))
    } else {
        stale.map(|stale| if stale.runtime_migration {
            format!("The bridge in Live still uses JavaScript. Quit Kumi and Live, then run: {} bridge to switch to the native bridge.", io.command())
        } else {
            format!("The bridge in Live is {}, older than this Kumi's ({}), so some changes aren't offered. Quit Kumi and Live, then run: {} update",stale.installed,stale.bundled,io.command())
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
        options.startup_notice = notice;
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
