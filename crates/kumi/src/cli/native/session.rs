//! Session, terminal and background-library ownership from the CLI.
use super::*;
use crate::{
    awake::KeepAwake,
    history::{log_redactor, open_input_history},
    models::{create_model_control, ModelControlOptions},
    terminal::{create_terminal, Terminal, TerminalOptions},
    tui::app::{create_tui, AwaySettings, ConnectDaw, ContextMeter, MapHover, PanelTab, TuiOptions},
    voice::{create_voice_control, VoiceControlOptions},
};
use async_trait::async_trait;
use kumi_agent::{
    core::{
        context::IntegrationContext,
        contracts::*,
        file_sync::FileSync,
        jobs::{create_job_store, DEFAULT_MAX_JOBS, WORKER_STEPS},
        memory::MemoryStoreOptions,
        session::{JobsOptions, VideoDirectories},
        session_log::SessionLog,
        store_backed::{SqliteMemoryStore, SqlitePlaybookStore, SqliteTechniqueStore},
        store_client::StoreClient,
    },
    *,
};
use kumi_ai::auth::store::{open_credential_store, Credential, CredentialStore};
use kumi_library::{create_library, LibraryOptions};
use kumi_listening::slots;
use kumi_media::programs::{configure_programs, ProgramDefaults};
use kumi_types::errors::KumiError;
use kumi_types::errors::SetupFailure;
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
/// How the watch for Live paces itself: a look at Live's port every `look_ms`, and while it answers
/// but the session isn't connected, a reconnect at once and then every `retry_ms`, at most `tries`
/// times until it stops answering (then the count starts again). It ends after `looks`. A Live that's
/// open but not answering yet gets `hold_ms` before Kumi says how to make it answer.
struct Pace {
    look_ms: u64,
    retry_ms: u64,
    tries: u32,
    looks: u32,
    hold_ms: u64,
}
const PACE: Pace = Pace { look_ms: 2_000, retry_ms: 20_000, tries: 10, looks: 1_800, hold_ms: 15_000 };

/// What the watch says while the DAW isn't open and the producer works in several DAWs here (which to open), and
/// whether this one is open now, from a listing made off the screen's thread.
struct Absent {
    words: Rc<str>,
    opened: Rc<dyn Fn() -> LocalBoxFuture<'static, bool>>,
}

/// How often Kumi looks for a DAW the producer opened while its session isn't connected.
const FOLLOW_MS: u64 = 2_000;
/// How long the session's own DAW stays gone before Kumi follows the producer to a DAW that was open beside it: a restart
/// of it (Live after a plug-in's install, say) isn't a move to another DAW. A DAW opened since is followed at once.
const GONE_MS: u64 = 20_000;
/// While the session is connected, every how many looks Kumi notes which DAWs are open beside its own.
const CONNECTED_LOOKS: u32 = 5;

/// What a look for a DAW the producer opened found: the DAWs open, and the DAW to follow to.
struct Looked {
    open: Vec<kumi_types::daw::DawId>,
    follow: Option<kumi_types::daw::DawId>,
}

/// Following the producer to the DAW they open: while the session isn't connected, each look (`look`) says which DAW is
/// open instead of the session's own, `current` ([`crate::daw::to_follow`]), and the screen is handed it: it closes Kumi,
/// to open again with that DAW, once nothing would be lost, and until then the next look hands it again. A DAW that was
/// open beside the session's own (`open_at_start`, then what the looks see) is handed only once the session's own has
/// been gone `gone_ms`. Ends once handed over, or when the session or the screen has gone.
async fn follow_the_daw<L, F>(
    current: kumi_types::daw::DawId,
    open_at_start: Vec<kumi_types::daw::DawId>,
    look: L,
    controller: Rc<RefCell<Option<Weak<dyn SessionController>>>>,
    terminal: Rc<RefCell<Option<Weak<dyn Terminal>>>>,
    look_ms: u64,
    gone_ms: u64,
) where
    L: Fn() -> F,
    F: std::future::Future<Output = Looked>,
{
    // The DAWs open beside the session's own when it was last seen open (None: not this run), and since when it's gone.
    let mut beside = open_at_start.contains(&current).then_some(open_at_start);
    let mut gone: Option<std::time::Instant> = None;
    let mut looks: u32 = 0;
    loop {
        tokio::time::sleep(std::time::Duration::from_millis(look_ms)).await;
        let Some(session) = controller.borrow().as_ref().and_then(Weak::upgrade) else { break };
        let connected = session.status().connection == ConnectionState::Connected;
        drop(session);
        looks = looks.wrapping_add(1);
        if connected {
            gone = None;
            if looks % CONNECTED_LOOKS != 0 {
                continue;
            }
        }
        let Looked { open, follow } = look().await;
        if connected || open.contains(&current) {
            (beside, gone) = (Some(open), None);
        } else if beside.is_some() {
            gone.get_or_insert_with(std::time::Instant::now);
        }
        let Some(daw) = follow.filter(|_| !connected) else { continue };
        let restarting = beside.as_ref().is_some_and(|beside| beside.contains(&daw))
            && gone.is_some_and(|since| since.elapsed() < std::time::Duration::from_millis(gone_ms));
        if restarting {
            continue;
        }
        let Some(ui) = terminal.borrow().as_ref().and_then(Weak::upgrade) else { break };
        if ui.follow_daw(daw) {
            break;
        }
    }
}

/// Live answers once it's open with AbletonMcpBridge chosen as a Control Surface: then the session
/// reconnects by itself. Live can accept connections before it's ready (a dialog open, a Set still
/// loading), so a failed reconnect is tried again while it answers. Watching ends once connected,
/// after an hour, or when the session has gone.
///
/// A Live that isn't open is told at once. One that's open is most likely still starting (first-run
/// setup has just opened it, say), so Kumi says it's waiting for it, and how to make it answer only if
/// it hasn't connected within `hold_ms` (AbletonMcpBridge not chosen as a Control Surface, say).
///
/// With `absent` (the producer works in several DAWs here), a DAW that isn't open gets those words instead, which say
/// to open one of them: Kumi follows the producer to the one they open. This one opening later is said as it starts,
/// and the hold runs from then.
async fn connect_when_daw_answers<F, A, O, B>(
    answers: F,
    live_open: O,
    words: WatchWords,
    absent: Option<Absent>,
    controller: Rc<RefCell<Option<Weak<dyn SessionController>>>>,
    emit: Rc<dyn Fn(SessionEvent)>,
    watching: Rc<Cell<bool>>,
    told: bool,
    pace: Pace,
) where
    F: Fn() -> A,
    A: std::future::Future<Output = bool>,
    O: FnOnce() -> B,
    B: std::future::Future<Output = bool>,
{
    let mut began = Some(std::time::Instant::now());
    let open = live_open().await;
    let absent = absent.filter(|_| !told && !open);
    // Told what to do already (restart Live, when it runs another version of the bridge): nothing more to say until
    // it connects.
    let mut said = told || (!open && absent.is_none());
    if !told {
        let message = match &absent {
            Some(absent) => {
                began = None;
                absent.words.to_string()
            }
            None => if said { words.unreachable } else { words.starting }.into(),
        };
        emit(SessionEvent::Notice { message });
    }
    let mut tries = 0;
    let mut next: Option<std::time::Instant> = Some(std::time::Instant::now());
    for _ in 0..pace.looks {
        let Some(session) = controller.borrow().as_ref().and_then(Weak::upgrade) else { break };
        if session.status().connection == ConnectionState::Connected {
            break;
        }
        if let Some(absent) = absent.as_ref().filter(|_| began.is_none()) {
            if (absent.opened)().await {
                began = Some(std::time::Instant::now());
                emit(SessionEvent::Notice { message: words.starting.into() });
            }
        }
        if !said && began.is_some_and(|at| at.elapsed() >= std::time::Duration::from_millis(pace.hold_ms)) {
            said = true;
            emit(SessionEvent::Notice { message: words.unreachable.into() });
        }
        if answers().await {
            if tries < pace.tries && next.is_some_and(|at| std::time::Instant::now() >= at) {
                tries += 1;
                // A turn under way makes this fail; it's tried again later.
                if session.has_reconnect() && session.reconnect().await.is_ok() && session.status().connection == ConnectionState::Connected
                {
                    emit(SessionEvent::Notice { message: words.answered.into() });
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
/// Live's watch, as its unit tests drive it: Live's words.
#[cfg(test)]
async fn connect_when_live_answers<F, A, O, B>(
    answers: F,
    live_open: O,
    controller: Rc<RefCell<Option<Weak<dyn SessionController>>>>,
    emit: Rc<dyn Fn(SessionEvent)>,
    watching: Rc<Cell<bool>>,
    told: bool,
    pace: Pace,
) where
    F: Fn() -> A,
    A: std::future::Future<Output = bool>,
    O: FnOnce() -> B,
    B: std::future::Future<Output = bool>,
{
    let words = live_words();
    connect_when_daw_answers(answers, live_open, words, None, controller, emit, watching, told, pace).await
}
/// Live's words for its watch, as the DAW table has them.
#[cfg(test)]
fn live_words() -> WatchWords {
    crate::daw::provider(kumi_types::daw::DawId::Ableton).expect("Live is registered").watch_words()
}
pub(super) async fn run_session(io: CliIo, config: AppConfig, secrets: &mut Vec<String>, answers: Answers) -> Result<i32, RuntimeError> {
    let (inference, bridge_config, bridge_missing, resume, daw, print) = match config {
        AppConfig::InferenceOnly { inference, bridge_missing, resume, daw, print } => {
            (inference, None, bridge_missing.unwrap_or(false), resume, daw, print)
        }
        AppConfig::Live { inference, bridge_config, resume, daw, print } => (inference, Some(bridge_config), false, resume, daw, print),
        _ => unreachable!(),
    };
    // The DAW this session works with: the one chosen (config), else Live.
    let provider = crate::daw::provider(daw.unwrap_or(kumi_types::daw::DawId::Ableton)).expect("a chosen DAW is registered");
    crate::daw::work_with(provider.id());
    // The DAW used last decides only among several bridges: a producer with one (Live alone) never gets the file.
    if bridge_config.is_some() && crate::daw::providers().into_iter().filter(|p| p.bridge_installed(&io.env)).count() > 1 {
        crate::daw::remember_used(provider.id(), &io.env);
    }
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
    let settings_file = load_settings_file(&io.env)?;
    let settings = read_settings(&settings_file);
    let away = AwaySettings {
        notifications: settings.notifications.as_deref() != Some("off"),
        live_status_bar: settings.live_status_bar != Some(false),
        answer_after: settings.answer_after.unwrap_or(AwaySettings::default().answer_after),
    };
    // The computer stays awake while Kumi works, whichever front end shows it.
    let awake = Rc::new(KeepAwake::system(settings.keep_awake != Some(false)));
    let terminal: Rc<RefCell<Option<Weak<dyn Terminal>>>> = Rc::new(RefCell::new(None));
    let emit: Rc<dyn Fn(SessionEvent)> = {
        let terminal = terminal.clone();
        let awake = awake.clone();
        Rc::new(move |event| {
            awake.follow(&event);
            let terminal = terminal.borrow().as_ref().and_then(Weak::upgrade);
            if let Some(terminal) = terminal {
                terminal.handle_event(event)
            }
        })
    };
    // What's new since this home's Kumi last started. A home an earlier Kumi used (an update kept the app
    // before it, or Kumi's database is there) is updated, not new, the first time it has no version noted.
    let used_before = {
        let home = std::path::Path::new(&settings_file).parent().unwrap_or(std::path::Path::new("."));
        home.join("app.previous").exists() || home.join("kumi.db").exists()
    };
    let whats_new_on = settings.whats_new != Some(false);
    let instant = settings.instant_phrases != Some(false);
    let copy_on_select = settings.copy_on_select != Some(false);
    let (mouse, transcript_on_quit) = (settings.mouse.as_deref() != Some("off"), settings.transcript_on_quit != Some(false));
    let full_repaint = settings.full_repaint;
    let context_meter = match settings.context_meter.as_deref() {
        Some("always") => ContextMeter::Always,
        Some("off") => ContextMeter::Off,
        _ => ContextMeter::Auto,
    };
    let projects_dir = load_projects_dir(&io.env)?;
    // Each Set's kept conversations, while Live may come in this session, or when the producer asked for one (then they
    // go on saving where they're kept, Live or not).
    let keep_days = settings.conversations.as_ref().and_then(|c| c.keep_days);
    let conversations = (bridge_config.is_some() || bridge_missing || resume.is_some())
        .then(|| kumi_agent::core::conversations::create_conversation_store_in(&projects_dir, keep_days, crate::daw::registry::places()));
    // Each kept conversation's log, beside it: every event the screen gets passes it first, the session's and the DAW's
    // alike, with keys taken out of the words as the input history takes them out.
    let log = conversations
        .as_ref()
        .map(|_| SessionLog::new(&projects_dir, crate::daw::registry::places(), Rc::new(log_redactor(secrets.clone()))));
    let emit = match &log {
        Some(log) => SessionLog::tee(log, emit),
        None => emit,
    };
    // The conversation asked for is found before anything shows: a word that names none says so, and Kumi stops there.
    // Only this DAW's conversations (and imported ones) go on here: another DAW's goes on in that DAW (review L4).
    let ours = |place: &str| kumi_types::place::kept_for(place, provider.id());
    let (taken_up, choose, resume_notice) = match (&resume, &conversations) {
        (Some(Resume::Named(word)), Some(store)) => match store.find(word).await? {
            ConversationMatch::One(row) if !ours(&row.place) => {
                io.err.write(&format!("{}\n", elsewhere_words(&row, io.command())));
                return Ok(1);
            }
            ConversationMatch::One(row) => (Some((row.place, row.id)), None, None),
            ConversationMatch::Several(_) => (None, Some(word.clone()), None),
            ConversationMatch::Gone => {
                io.err.write(&format!("{}\n", gone_words(store.days_kept().await)));
                return Ok(1);
            }
            ConversationMatch::None => {
                let word: String = word.chars().filter(|c| !c.is_control()).collect();
                io.err.write(&format!("No conversation {}. {} -r lists them.\n", string::head(&word, 60), io.command()));
                return Ok(1);
            }
        },
        (Some(Resume::Last), Some(store)) => match store.all().await?.into_iter().find(|row| ours(&row.place)) {
            Some(row) => (Some((row.place, row.id)), None, None),
            None => (None, None, Some("No conversation is kept yet, so this one starts afresh.".to_string())),
        },
        (Some(Resume::Pick), _) => (None, Some(String::new()), None),
        _ => (None, None, None),
    };
    let tools_dir = load_tools_dir(&io.env)?;
    // The producer's own music, learned from the folders they agreed to (none: not learned).
    let music = {
        let mut options = kumi_listening::music::learning::MusicOptions::new(
            load_music_dir(&io.env)?.into(),
            settings.music_folders.clone(),
            crate::daw::registry::libraries(),
        );
        // The references taste makes from their songs, which forgetting their music deletes too.
        options.references = Some(load_references_dir(&io.env)?.into());
        options.library_folders = settings.library_folders.clone();
        options.offered = settings.music_offered == Some(true);
        // The producer's answer, kept in settings.json beside the rest of their settings: what this Kumi changed, applied
        // to the file as it is now (another Kumi may have changed it since).
        let file = settings_file.clone();
        let before = RefCell::new(settings.music_folders.clone());
        options.save = Some(Rc::new(move |folders: Option<&[String]>, offered: bool| {
            crate::config::write_music_settings(&file, folders, before.borrow().as_deref(), offered).map_err(|error| error.to_string())?;
            *before.borrow_mut() = folders.map(<[String]>::to_vec);
            Ok(())
        }));
        let file = settings_file.clone();
        options.offered_since = Some(Rc::new(move || crate::config::music_offered(&file)));
        kumi_listening::music::learning::create_music(options)
    };
    let library = create_library(LibraryOptions {
        daws: crate::daw::registry::libraries(),
        dir: load_library_dir(&io.env)?,
        folders: Some(settings.library_folders),
        projects_dir: Some(projects_dir.clone()),
        ..Default::default()
    });
    let controller: Rc<RefCell<Option<Weak<dyn SessionController>>>> = Rc::new(RefCell::new(None));
    let listener_store = store.clone();
    // Which model does each listening job: /slots shows, swaps and takes back.
    let slots_file = slots::file_in(std::path::Path::new(&crate::config::kumi_dir(&io.env)));
    let slots_context = Rc::new(slots::SlotsContext {
        file: slots_file.clone(),
        store: store.clone(),
        env: io.env.clone(),
        profile: Some(provider.profile()),
    });
    // Measured references: the reference tool keeps them, the judge works toward them by name.
    let references = Rc::new(kumi_listening::references::store::ReferenceStore::new(load_references_dir(&io.env)?));
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
    // models.dev's catalog of models: the copy Kumi kept, then a newer one at most once a day, never in the way (a slow
    // answer ends with the session, so Kumi's exit doesn't wait on it).
    let catalog = tokio::task::spawn_local(kumi_ai::providers::catalog::refresh(kumi_ai::providers::catalog::RefreshOptions {
        cache: std::path::Path::new(&crate::config::kumi_dir(&io.env)).join("cache").join("models-catalog.json"),
        fetch: kumi_ai::http::default_fetch(),
        now: kumi_common::time::now_ms() as f64,
        ask: settings.model_catalog != Some(false),
        wait_ms: kumi_ai::providers::catalog::WAIT_MS,
    }));
    let kernel_factory: KernelFactory = {
        let models = models.clone();
        Rc::new(move |options| {
            let models = models.clone();
            async move {
                let checkpoint = options.checkpoint.clone();
                let result = async {
                    let binding = models.binding().await?;
                    let kernel = create_agent_kernel(AgentKernelOptions {
                        conversation: options.conversation,
                        instructions: options.instructions,
                        tools: options.tools,
                        signal: options.signal,
                        checkpoint: options.checkpoint,
                        binding,
                        max_steps: None,
                        budget: None,
                    })?;
                    // The producer's fallback carries on when their model can't; one that can't be used is said once.
                    match models.fallback_binding().await {
                        Ok(fallback) => kernel.set_fallback(fallback),
                        Err(error) => models.fallback_unusable(&error),
                    }
                    Ok::<_, RuntimeError>(Rc::new(kernel) as Rc<dyn Kernel>)
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
    // Background workers: the conversation's model, or the one settings name for them, and a shorter answer each.
    let worker_factory: KernelFactory = {
        let models = models.clone();
        let worker_model = settings.worker_model.clone();
        Rc::new(move |options| {
            let (models, worker_model) = (models.clone(), worker_model.clone());
            async move {
                let binding = match &worker_model {
                    Some(model) => models.binding_for(model).await?,
                    None => models.binding().await?,
                };
                let kernel = create_agent_kernel(AgentKernelOptions {
                    conversation: options.conversation,
                    instructions: options.instructions,
                    tools: options.tools,
                    signal: options.signal,
                    checkpoint: options.checkpoint,
                    binding,
                    max_steps: Some(WORKER_STEPS),
                    budget: None,
                })?;
                // A worker waits out a usage limit, or carries on with the producer's fallback, as the conversation does.
                if let Ok(fallback) = models.fallback_binding().await {
                    kernel.set_fallback(fallback);
                }
                Ok(Rc::new(kernel) as Rc<dyn Kernel>)
            }
            .boxed_local()
        })
    };
    let bundled = provider.bundled_bridge_version();
    let project_store = create_project_store(&projects_dir);
    let restore_file = load_restore_file(&io.env)?;
    let env = io.env.clone();
    let trace = io.env.get("KUMI_TRACE").is_some_and(|s| s == "1");
    // A DAW asked for (--daw, KUMI_DAW, a bridge configuration given) is the one Kumi works with. Otherwise Kumi follows
    // the producer to the DAW they open, and while none is open it says to open one of theirs, not how to make this one
    // answer, when they work in several.
    let asked = crate::config::take_daw(&io.args, &io.env).ok().and_then(|(_, asked)| asked);
    let given = io.args.first().is_some_and(|arg| arg == "--bridge-config");
    // (one request with -p is answered with what's open now, and ends)
    let follows = asked.is_none() && !given && print.is_none() && (bridge_config.is_some() || bridge_missing);
    let absent_words: Option<Rc<str>> = follows.then(|| crate::daw::open_one_words(&io.env)).flatten().map(Rc::from);
    // A test's answer for which DAWs run at each look for one the producer opened.
    let running_stand_in = answers.get::<crate::daw::RunningDaws>().cloned();
    // Watching for Live to answer, so a chat without Live connects by itself once it does.
    let watching_live = Rc::new(Cell::new(false));
    // That watch, ended with the session: it would notice only after its look and its sleep, and on Windows a look at a
    // DAW that isn't open takes 1.5 s, past what Kumi's shutdown waits for.
    let live_watch: Rc<RefCell<Option<tokio::task::JoinHandle<()>>>> = Rc::default();
    // Whether the producer was told to restart Live (it runs another version of the bridge): said once a session.
    let restart_said = Rc::new(Cell::new(false));
    // Where the session finds Live: first-run setup sets it once the bridge is in place.
    let live_config = Rc::new(RefCell::new(bridge_config.clone()));
    let full_screen = io.input.is_tty() && io.out.is_tty() && io.env.get("KUMI_UI").map(String::as_str) != Some("plain");
    let integration_factory: IntegrationFactory = {
        let watching_live = watching_live.clone();
        let live_watch = live_watch.clone();
        let restart_said = restart_said.clone();
        let library = library.clone();
        let music = music.clone();
        let emit = emit.clone();
        let controller = controller.clone();
        let live_config = live_config.clone();
        let bundled = bundled.clone();
        let references = references.clone();
        Box::new(move |on_connection| {
            let Some(bridge_config) = live_config.borrow().clone() else {
                return provider.without_daw(Rc::new(move |state| on_connection(state, None)));
            };
            // What the session gives any DAW's integration; the DAW's provider adds what only it has.
            let mut options = IntegrationContext::new(on_connection.clone());
            options.restore_file = Some(restore_file.clone());
            // Plain mode has no FOCUS: a polled DAW is asked about once a second, for busy and gone.
            options.focus_drawn = full_screen;
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
                let music = music.clone();
                Rc::new(move |transport| {
                    let playing = transport.as_ref().is_some_and(|t| t.playing);
                    if playing {
                        library.pause();
                    } else {
                        library.resume();
                    }
                    music.set_playing(playing);
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
            // A look that failed while the DAW was there (it was loading a file): the session looks again by itself.
            options.on_look_again = Some({
                let controller = controller.clone();
                Rc::new(move || {
                    if let Some(c) = controller.borrow().as_ref().and_then(Weak::upgrade) {
                        tokio::task::spawn_local(async move {
                            let _ = c.refresh().await;
                        });
                    }
                })
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
            options.on_judge = Some({
                let emit = emit.clone();
                let controller = controller.clone();
                Rc::new(move |round| {
                    let controller = controller.borrow().as_ref().and_then(Weak::upgrade);
                    if let Some(c) = controller {
                        c.watch(WatchEvent::Judged(round.clone()));
                    }
                    emit(SessionEvent::Judged(round));
                })
            });
            // What a search hears is kept, and the next search on the same device starts from what worked.
            options.search_memory = controller.borrow().as_ref().and_then(Weak::upgrade).and_then(|c| c.settings_memory());
            options.references = Some(references.clone());
            options.listener = Some({
                let store: Rc<dyn CredentialStore> = listener_store.clone();
                let env = env.clone();
                let slots = slots_file.clone();
                Rc::new(move |signal| {
                    let (store, env, slots) = (store.clone(), env.clone(), slots.clone());
                    // The listening slot's model (KUMI_LISTENER wins over it), following the slot when it's swapped.
                    async move { slots::listener(&slots, store, &env, signal).await }.boxed_local()
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
            let session = DawSession {
                env: env.clone(),
                bridge_config: bridge_config.clone(),
                project_store: project_store.clone(),
                answers: answers.clone(),
            };
            let emit = emit.clone();
            let bundled = bundled.clone();
            let controller = controller.clone();
            let watching_live = watching_live.clone();
            let live_watch = live_watch.clone();
            let restart_said = restart_said.clone();
            let env = env.clone();
            let absent_words = absent_words.clone();
            with_fallback_failure(
                provider.integration(options, &session),
                Rc::new(move || {
                    let on_connection = on_connection.clone();
                    provider.without_daw(Rc::new(move |state| on_connection(state, None)))
                }),
                Rc::new(move |failure: &KumiError| {
                    let message = failure.message.clone();
                    let installed = provider.installed_bridge_version(&bridge_config, &env);
                    let current = installed.is_some() && installed == bundled;
                    // Live still runs the bridge it loaded before an update: restarting Live is the fix whether or not
                    // the installed bridge is current (right after kumi bridge it is). Said once; the watch then
                    // connects once Live answers with the one installed.
                    let restart = failure.setup == Some(SetupFailure::RestartDaw);
                    if restart && !restart_said.replace(true) {
                        emit(SessionEvent::Notice { message: message.clone() });
                    }
                    if !current && !restart {
                        emit(SessionEvent::Notice { message });
                    } else if !watching_live.replace(true) {
                        // Said once, by the watch: its own retries fall back here too, quietly.
                        let config = bridge_config.clone();
                        let (env, answers_env, listed_env) = (env.clone(), env.clone(), env.clone());
                        let absent = absent_words.clone().map(|words| Absent {
                            words,
                            opened: Rc::new(move || {
                                let listed = listed_env.clone();
                                async move {
                                    let listing = tokio::task::spawn_blocking(move || {
                                        daw_bridge::detect::list_processes(&|name| listed.get(name).cloned())
                                    })
                                    .await
                                    .unwrap_or_default();
                                    provider.running_now(&listing)
                                }
                                .boxed_local()
                            }),
                        });
                        *live_watch.borrow_mut() = Some(tokio::task::spawn_local(connect_when_daw_answers(
                            move || provider.bridge_answers(&config, &answers_env),
                            move || provider.app_open(&env),
                            provider.watch_words(),
                            absent,
                            controller.clone(),
                            emit.clone(),
                            watching_live.clone(),
                            restart,
                            PACE,
                        )));
                    }
                }),
            )
        })
    };
    let mut options = SessionOptions::new(kernel_factory, integration_factory, emit.clone());
    // The session speaks the words of the DAW it works with (Live's for Live).
    options.profile = *provider.profile();
    // Live may come later in this session, once first-run setup has put the bridge in place.
    let live_possible = bridge_config.is_some() || bridge_missing;
    options.conversations = conversations.map(|store| store as Rc<dyn ConversationStore>);
    options.log = log;
    options.resume = taken_up;
    // Kumi's database keeps notes, techniques, lessons and gaps, with what earlier Kumis kept in files
    // read in. When it can't open, they stay in their files this time, as before, and Kumi says so once.
    let files = json_files(&io.env)?;
    let database = StoreClient::open(load_db_file(&io.env)?.into(), files.clone(), kumi_common::time::now_ms()).await;
    let database_notice = match &database {
        Ok((_, Ok(imported))) => Some(imported.sentences().join(" ")).filter(|said| !said.is_empty()),
        Ok((_, Err(why))) => {
            Some(format!("Kumi couldn't read in the notes and techniques kept in files this time ({why}); it tries again next start."))
        }
        Err(why) => Some(format!("Your notes, techniques and lessons stay in their files this time: {why}.")),
    };
    // Files the start named as not whole aren't named again a turn later.
    let named = match &database {
        Ok((_, Ok(imported))) => imported.not_whole.clone(),
        _ => vec![],
    };
    let database = database.ok().map(|(client, _)| client);
    options.memory = Some(match &database {
        Some(client) => Rc::new(SqliteMemoryStore::with_places(client.clone(), crate::daw::registry::places())) as Rc<dyn MemoryStore>,
        None => kumi_agent::core::memory::create_memory_store_in(
            MemoryStoreOptions { projects_dir: projects_dir.clone().into(), producer_file: load_memory_file(&io.env)?.into() },
            crate::daw::registry::places(),
        ),
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
    // An older Kumi open beside this one writes the files: what it changes comes in a turn later.
    options.files = database.as_ref().map(|client| FileSync::new(client.clone(), files, &named));
    options.store = database;
    let goals = load_goals_dir(&io.env)?;
    options.objectives = Some(create_objective_store(goals.clone()));
    options.references = Some(references);
    options.goals = Some(create_goal_store(goals));
    options.gaps = Some(load_gaps_file(&io.env)?);
    options.timings = Some(load_timings_file(&io.env)?);
    options.watch = Some(VideoDirectories { videos_dir: load_videos_dir(&io.env)?, tools_dir: tools_dir.clone() });
    options.web = true;
    options.library = Some(library.clone());
    options.preset_dir = Some(std::path::Path::new(&crate::config::kumi_dir(&io.env)).join("presets"));
    options.music = Some(music.clone());
    // /model, /effort, /fast, /login and /logout ask the session, which has the models (the kernel's come from them too).
    options.models = Some(models.clone() as Rc<dyn crate::models::ModelController>);
    // /slots and /map, which the session answers: which model does each listening job, and the library's sounds and
    // the songs on their maps, read each time /map opens.
    options.slots = Some(slots_context);
    options.map = Some(crate::map::loader(library.clone(), load_library_dir(&io.env)?.into(), music.clone()));
    options.jobs = Some(JobsOptions {
        worker_factory,
        max: settings.max_jobs.map(|n| n as usize).unwrap_or(DEFAULT_MAX_JOBS),
        // Beside the goals: a Set's interrupted jobs and unheard reports, for the next time Kumi runs.
        store: Some(create_job_store(load_goals_dir(&io.env)?)),
    });
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
    // What the session says of the DAW's bridge: missing, or older than this Kumi's.
    let bridge = provider.bridge_state(&io.env, bridge_config.is_some(), bridge_missing, bundled.as_deref(), io.command());
    let bridge_notice = bridge.notice;
    let notice = match (bridge_notice, database_notice) {
        (Some(bridge), Some(database)) => Some(format!("{bridge} {database}")),
        (bridge, database) => bridge.or(database),
    };
    // An older credential file that couldn't be made readable only by the producer (Windows), said once.
    let notice = match (notice, kumi_ai::auth::store::unprotected_notice()) {
        (Some(notice), Some(credentials)) => Some(format!("{notice} {credentials}")),
        (notice, credentials) => notice.or(credentials),
    };
    let notice = match (notice, resume_notice) {
        (Some(notice), Some(resume)) => Some(format!("{notice} {resume}")),
        (notice, resume) => notice.or(resume),
    };
    // Several DAWs open and none asked for: which one Kumi works with, and how to switch.
    let also = if asked.is_none() {
        let others = crate::daw::also_running(provider.id(), &|other| other.running_now(&crate::daw::processes_in(&io.env)));
        crate::daw::also_running_words(provider.id(), &others, &io.env)
    } else {
        None
    };
    let notice = match (notice, also) {
        (Some(notice), Some(also)) => Some(format!("{notice} {also}")),
        (notice, also) => notice.or(also),
    };
    // In the app, first-run setup puts the bridge in place instead, with the DAW restarting around it (where it runs).
    let connect_why = bridge.why;
    let update_after = Rc::new(Cell::new(false));
    // The DAW Kumi opens again with once this run closes, and whether it was asked for (/daw, setup's question) or
    // followed to (the producer opened it).
    let daw_after: Rc<Cell<Option<(kumi_types::daw::DawId, bool)>>> = Rc::default();
    // What /daw offers: the DAWs on this computer (or with a bridge in), by this session's environment.
    let here: crate::daw::Here = io.daws_here.clone().unwrap_or_else(|| {
        let env = io.env.clone();
        Rc::new(move |id| crate::daw::provider(id).is_some_and(|other| other.here(&env) || other.bridge_installed(&env)))
    });
    crate::daw::offer_here(here.clone());
    // /daw <name>: Kumi closes and opens again with that DAW. Offered once the producer works with more than Live, or has
    // another DAW here.
    let offers_daw = provider.id() != kumi_types::daw::DawId::Ableton
        || crate::daw::providers().into_iter().any(|other| other.id() != kumi_types::daw::DawId::Ableton && here(other.id()));
    let switch_daw = offers_daw.then(|| {
        let daw_after = daw_after.clone();
        Rc::new(move |daw| daw_after.set(Some((daw, true)))) as Rc<dyn Fn(kumi_types::daw::DawId)>
    });
    // Kumi follows the producer to the DAW they open (`follow_the_daw`): it closes, and opens again with that one.
    let follow_daw = follows.then(|| {
        let daw_after = daw_after.clone();
        Rc::new(move |daw| daw_after.set(Some((daw, false)))) as Rc<dyn Fn(kumi_types::daw::DawId)>
    });
    // A first run with several DAWs here, no bridge yet and none open: setup asks which, remembers it, and opens Kumi
    // again with it when it isn't this run's (review L2).
    let ask_daw = if asked.is_none() && bridge_missing {
        crate::daw::to_ask(&io.env, &|other| other.here(&io.env), &|other| other.running_now(&crate::daw::processes_in(&io.env)))
    } else {
        vec![]
    };
    let pick_daw = (!ask_daw.is_empty()).then(|| {
        let (daw_after, env, current) = (daw_after.clone(), io.env.clone(), provider.id());
        Rc::new(move |daw| {
            crate::daw::remember_used(daw, &env);
            if daw != current {
                daw_after.set(Some((daw, true)));
            }
        }) as Rc<dyn Fn(kumi_types::daw::DawId)>
    });
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
    // What's new is for someone reading it: the full screen, or plain lines at a terminal (a screen reader).
    // A script piping Kumi neither sees it nor uses it up.
    let whats_new = if full_screen || io.input.is_tty() {
        crate::whats_new::at_start(&settings_file, KUMI_VERSION, used_before, whats_new_on)
    } else {
        None
    };
    // What /export and /share take out: what `kumi report` takes out, as this computer knows it at each export.
    let redact = crate::export::redactor_maker(&io.env);
    let ui: Rc<dyn Terminal> = if let Some(print) = print.clone() {
        Rc::new(crate::print::Print::new(crate::print::PrintOptions {
            controller: session,
            request: print.request,
            json: print.json,
            out: io.out.clone(),
            err: io.err.clone(),
            secrets: secrets.clone(),
            instant,
            startup_notice: notice,
        }))
    } else if full_screen {
        let mut options = TuiOptions::new(session, io.input.clone(), io.out.clone(), mode);
        // What the terminal is, asked once as the screen starts: its answers come before what its environment says of
        // it, which stays the fallback. A terminal its environment keeps quiet (the Linux console, a dumb one) isn't asked,
        // nor one whose producer said not to (`KUMI_PROBE=off`).
        let platform = kumi_types::system::platform();
        let profile = crate::tui::alerts::detect_terminal_profile(&io.env, platform);
        if io.probe && profile.status && io.env.get("KUMI_PROBE").map(String::as_str) != Some("off") {
            let probed = crate::tui::probe::probe(&io.input, &io.out, crate::tui::probe::BUDGET).await;
            let version = probed.answers.version.clone();
            options.terminal = Some(profile.answered(&probed.answers, &io.env, platform));
            let depth = crate::tui::style::detect_color_depth_here();
            options.color_depth = Some(crate::tui::style::answered_color_depth(depth, version.as_deref(), &io.env));
            options.typeahead = probed.read;
        }
        options.redact = Some(redact);
        options.command = io.command().to_string();
        options.reveal = Some(Rc::new(crate::export::reveal));
        options.startup_notice = if connect_why.is_some() { None } else { notice };
        options.choose_conversation = choose.clone();
        options.whats_new = whats_new;
        options.away = away;
        options.instant = instant;
        options.copy_on_select = copy_on_select;
        options.mouse = mouse;
        options.transcript_on_quit = transcript_on_quit;
        options.full_repaint = full_repaint;
        options.context_meter = context_meter;
        options.keys = read_keys(&load_keys_file(&io.env)?);
        options.next_time = install::next_time(&io.env);
        options.connect_live = live_possible.then(|| ConnectDaw {
            why: connect_why,
            bridge: bundled.clone().unwrap_or_default(),
            live: provider.setup(
                &io.env,
                live_config.clone(),
                io.args.first().is_some_and(|arg| arg == "--bridge-config"),
                controller.clone(),
            ),
        });
        options.secrets = secrets.clone();
        options.history = Some(history);
        options.open_browser = Some(Rc::new(login::open_browser));
        // Over SSH, the producer's browser and clipboard are on another computer: ChatGPT signs in with a code, and
        // copying goes through the terminal alone.
        options.remote = io.env.get("SSH_CONNECTION").is_some_and(|value| !value.is_empty());
        if !options.remote {
            options.copy = Some(Rc::new(login::copy_to_clipboard));
        }
        options.editor = crate::tui::external_editor::editor_command(|name| io.env.get(name).cloned());
        options.reach = Some({
            let env = io.env.clone();
            Rc::new(move || {
                let env = env.clone();
                async move { login::reach_sign_in(&env, None).await }.boxed_local()
            })
        });
        options.updates = Some(updates);
        options.switch_daw = switch_daw;
        options.follow_daw = follow_daw;
        options.ask_daw = ask_daw;
        options.pick_daw = pick_daw;
        options.willington = provider.toggle(&io.env);
        // /map's points play through Kumi's player (Space).
        options.player = Some(Rc::new(kumi_media::audio::player::Player::new()));
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
        // h on the map: whether its points play as the mouse comes to them, kept in settings.json (only when they don't).
        options.map_hover = Some(MapHover {
            load: {
                let settings_file = settings_file.clone();
                Rc::new(move || read_settings(&settings_file).map_hover != Some(false))
            },
            save: {
                let settings_file = settings_file.clone();
                Rc::new(move |on| {
                    let mut settings = read_settings(&settings_file);
                    settings.map_hover = Some(on);
                    let _ = write_settings(&settings_file, &serde_json::to_value(&settings).unwrap());
                })
            },
        });
        Rc::new(create_tui(options))
    } else {
        let mut options = TerminalOptions::new(session, io.input.clone(), io.out.clone(), models.clone(), mode);
        options.startup_notice = notice;
        options.redact = Some(redact);
        options.command = io.command().to_string();
        options.choose_conversation = choose.clone();
        options.whats_new = whats_new;
        options.instant = instant;
        options.next_time = install::next_time(&io.env);
        options.secrets = secrets.clone();
        options.history = Some(history);
        options.updates = Some(updates);
        options.switch_daw = switch_daw;
        options.follow_daw = follow_daw;
        options.willington = provider.toggle(&io.env);
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
    // SIGQUIT (ctrl+\ where the terminal isn't raw): Kumi closes as for SIGTERM, but not while the terminal is lent to
    // the producer's editor, whose key it was.
    #[cfg(unix)]
    let quit = SignalTask(if io.signals {
        let ui = ui.clone();
        let mut signal =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::quit()).map_err(|e| RuntimeError::plain(e.to_string()))?;
        Some(tokio::task::spawn_local(async move {
            while signal.recv().await.is_some() {
                if !ui.lent() {
                    ui.close().await;
                    break;
                }
            }
        }))
    } else {
        None
    });
    let running = ui.run();
    // Following the producer to the DAW they open: one listing a look, made off the screen's thread, or a test's answer.
    let following = follows.then(|| {
        let (env, current) = (io.env.clone(), provider.id());
        let stand_in = running_stand_in.clone();
        let open_at_start: Vec<kumi_types::daw::DawId> = match &stand_in {
            Some(running) => (running.0)(),
            None => crate::daw::providers()
                .filter(|other| other.running_now(&crate::daw::processes_in(&io.env)))
                .map(|other| other.id())
                .collect(),
        };
        tokio::task::spawn_local(follow_the_daw(
            current,
            open_at_start,
            move || {
                let (env, stand_in) = (env.clone(), stand_in.clone());
                async move {
                    let open: Vec<kumi_types::daw::DawId> = match stand_in {
                        Some(running) => (running.0)(),
                        None => {
                            let listed = env.clone();
                            let listing =
                                tokio::task::spawn_blocking(move || daw_bridge::detect::list_processes(&|name| listed.get(name).cloned()))
                                    .await
                                    .unwrap_or_default();
                            crate::daw::providers().filter(|other| other.running_now(&listing)).map(|other| other.id()).collect()
                        }
                    };
                    let follow = crate::daw::to_follow(current, &env, &|other| open.contains(&other.id())).map(|other| other.id());
                    Looked { open, follow }
                }
            },
            controller.clone(),
            terminal.clone(),
            FOLLOW_MS,
            GONE_MS,
        ))
    });
    // Learning the producer's sounds and music goes on in the background of a session that stays; a request with -p
    // only reads what's learned.
    if print.is_none() {
        library.start();
        music.start();
    }
    let settings = read_settings(&settings_file);
    let update_check =
        if print.is_none() && settings.update_check != Some(false) && !io.env.get("KUMI_NO_UPDATE_CHECK").is_some_and(|s| !s.is_empty()) {
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
    // The conversation asked for as Kumi started may not be the one going on now: opened again after an update, Kumi
    // carries on with this one.
    let going = if update_after.get() {
        let session = controller.borrow().as_ref().and_then(Weak::upgrade);
        match session {
            Some(session) => session.conversation_now().await.filter(|now| now.kept).map(|now| now.id),
            None => None,
        }
    } else {
        None
    };
    drop(interrupt);
    #[cfg(unix)]
    drop(terminate);
    #[cfg(unix)]
    drop(quit);
    library.close().await;
    music.close().await;
    catalog.abort();
    models.close();
    fetch_notices.abort();
    configure_programs(ProgramDefaults::default());
    if let Some(task) = update_check {
        task.abort();
    }
    if let Some(task) = following {
        task.abort();
    }
    if let Some(task) = live_watch.borrow_mut().take() {
        task.abort();
    }
    terminal.borrow_mut().take();
    controller.borrow_mut().take();
    if let Some((daw, asked)) = daw_after.get() {
        let mut again = io.clone();
        again.args = if asked { switched_args(&io.args, daw) } else { followed_args(&io.args) };
        return super::reopen(&again).await;
    }
    if update_after.get() {
        let mut again = io.clone();
        again.args = reopened_args(&io.args, going.as_deref());
        super::update_and_reopen(&again).await
    } else {
        Ok(code)
    }
}
/// What `-r` says of a conversation kept in another DAW's place: whose it is, and the command that carries it on there.
fn elsewhere_words(row: &kumi_agent::core::contracts::ConversationSummary, command: &str) -> String {
    let daw = kumi_types::place::Place::parse(&row.place).and_then(|place| place.daw());
    let (app, noun) = daw.and_then(crate::daw::registry::profile).map_or(("another DAW", "project"), |profile| profile.word_pair());
    let title: String = row.title().chars().filter(|c| !c.is_control()).collect();
    format!(
        "“{}” is about {} {app} {noun}: {command} --daw {} -r {} carries it on in {app}.",
        string::head(&title, 60),
        if app.starts_with(['A', 'E', 'I', 'O', 'U']) { "an" } else { "a" },
        daw.map(crate::daw::command_name).unwrap_or_default(),
        row.id
    )
}
/// What Kumi opens again with after an update: the same arguments, but a conversation asked for (`-c`, `-r`) is the
/// one going on as it closed.
fn reopened_args(args: &[String], going: Option<&str>) -> Vec<String> {
    match (crate::config::take_resume(args), going) {
        (Ok((mut rest, Some(_))), Some(id)) => {
            rest.extend(["-r".to_string(), id.to_string()]);
            rest
        }
        _ => args.to_vec(),
    }
}

/// What Kumi opens again with when it follows the producer to the DAW they opened: the same arguments, with no
/// conversation (each DAW's project has its own) and still no DAW asked for, so the start chooses the one open and goes
/// on following.
fn followed_args(args: &[String]) -> Vec<String> {
    crate::config::take_resume(args).map(|(rest, _)| rest).unwrap_or_else(|_| args.to_vec())
}

/// What Kumi opens again with after /daw: the same arguments, with that DAW asked for and no conversation (each DAW's
/// project has its own).
fn switched_args(args: &[String], daw: kumi_types::daw::DawId) -> Vec<String> {
    let rest = crate::config::take_resume(args).map(|(rest, _)| rest).unwrap_or_else(|_| args.to_vec());
    let rest = crate::config::take_daw(&rest, &Env::new()).map(|(rest, _)| rest).unwrap_or(rest);
    ["--daw".to_string(), daw.as_str().to_string()].into_iter().chain(rest).collect()
}

#[cfg(test)]
mod reopen_tests {
    use super::{reopened_args, switched_args};
    use kumi_types::daw::DawId;
    #[test]
    fn opened_again_with_another_daw_kumi_asks_for_it_and_no_conversation() {
        let args = |words: &[&str]| words.iter().map(|w| w.to_string()).collect::<Vec<_>>();
        assert_eq!(switched_args(&args(&[]), DawId::Renoise), args(&["--daw", "renoise"]));
        assert_eq!(switched_args(&args(&["--daw", "reaper", "-r", "night"]), DawId::Ableton), args(&["--daw", "ableton"]));
        assert_eq!(switched_args(&args(&["--daw=renoise", "-c"]), DawId::Reaper), args(&["--daw", "reaper"]));
    }
    #[test]
    fn followed_to_the_daw_opened_kumi_opens_again_with_no_conversation_and_no_daw_asked_for() {
        let args = |words: &[&str]| words.iter().map(|w| w.to_string()).collect::<Vec<_>>();
        assert_eq!(super::followed_args(&args(&[])), args(&[]));
        assert_eq!(super::followed_args(&args(&["-c"])), args(&[]));
        assert_eq!(super::followed_args(&args(&["-r", "night", "drive"])), args(&[]));
    }
    #[test]
    fn opened_again_after_an_update_kumi_carries_on_with_the_conversation_going_on() {
        let args = |words: &[&str]| words.iter().map(|w| w.to_string()).collect::<Vec<_>>();
        assert_eq!(
            reopened_args(&args(&["-r", "night", "drive", "--inference-only"]), Some("mv66")),
            args(&["--inference-only", "-r", "mv66"])
        );
        assert_eq!(reopened_args(&args(&["-c"]), Some("mv66")), args(&["-r", "mv66"]));
        // Not asked for, or nothing kept: as it was.
        assert_eq!(reopened_args(&args(&["--inference-only"]), Some("mv66")), args(&["--inference-only"]));
        assert_eq!(reopened_args(&args(&["-c"]), None), args(&["-c"]));
    }
}
#[cfg(test)]
mod watch_tests {
    use super::*;
    use kumi_agent::core::contracts::{PinnedNode, SessionStatus, TurnState};
    use kumi_types::contracts::ChangeRecord;

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
    async fn watch(connect_on: u32, answers: bool, live_open: bool, hold_ms: u64) -> (u32, Vec<String>, bool) {
        watch_told(connect_on, answers, live_open, hold_ms, false).await
    }
    async fn watch_told(connect_on: u32, answers: bool, live_open: bool, hold_ms: u64, told: bool) -> (u32, Vec<String>, bool) {
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
        let pace = Pace { look_ms: 1, retry_ms: 0, tries: 10, looks: 200, hold_ms };
        connect_when_live_answers(
            move || async move { answers },
            move || async move { live_open },
            controller,
            emit,
            watching.clone(),
            told,
            pace,
        )
        .await;
        let notices = notices.borrow().clone();
        (fake.reconnects.get(), notices, watching.get())
    }

    /// What the watch says with several DAWs here (`absent`), for a DAW open as Kumi starts, or opening at the
    /// `opens_at`th look, never answering.
    async fn watch_absent(open: bool, opens_at: u32) -> Vec<String> {
        let fake = Rc::new(Fake { reconnects: Cell::new(0), connect_on: u32::MAX, connected: Cell::new(false) });
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
        let looked = Rc::new(Cell::new(0u32));
        let absent = Absent {
            words: Rc::from("open one of yours"),
            opened: Rc::new(move || {
                looked.set(looked.get() + 1);
                let open = looked.get() >= opens_at;
                async move { open }.boxed_local()
            }),
        };
        let pace = Pace { look_ms: 1, retry_ms: 0, tries: 10, looks: 60, hold_ms: 5 };
        let watching = Rc::new(Cell::new(true));
        connect_when_daw_answers(
            || async { false },
            move || async move { open },
            live_words(),
            Some(absent),
            controller,
            emit,
            watching,
            false,
            pace,
        )
        .await;
        let notices = notices.borrow().clone();
        notices
    }

    #[tokio::test(flavor = "current_thread")]
    async fn with_several_daws_here_one_not_open_is_said_as_which_to_open() {
        assert_eq!(watch_absent(false, u32::MAX).await, ["open one of yours"], "never opened: nothing more");
        assert_eq!(
            watch_absent(false, 3).await,
            ["open one of yours", live_words().starting, live_words().unreachable],
            "opened later: said as it starts, and how to make it answer once the hold is over"
        );
        assert_eq!(watch_absent(true, 0).await, [live_words().starting, live_words().unreachable], "open as Kumi starts: as before");
    }

    /// A screen that takes the DAW handed to it the `takes`th time (busy before that).
    struct Screen {
        handed: RefCell<Vec<kumi_types::daw::DawId>>,
        takes: usize,
    }
    impl Terminal for Screen {
        fn run(&self) -> LocalBoxFuture<'static, i32> {
            async { 0 }.boxed_local()
        }
        fn handle_event(&self, _: SessionEvent) {}
        fn offer_update(&self, _: &str) {}
        fn interrupt(&self) {}
        fn close(&self) -> LocalBoxFuture<'static, i32> {
            async { 0 }.boxed_local()
        }
        fn follow_daw(&self, daw: kumi_types::daw::DawId) -> bool {
            self.handed.borrow_mut().push(daw);
            self.handed.borrow().len() >= self.takes
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn kumi_follows_to_the_daw_opened_once_the_screen_can_close() {
        tokio::task::LocalSet::new().run_until(follow_while_not_connected()).await;
    }
    async fn follow_while_not_connected() {
        use kumi_types::daw::DawId;
        let fake = Rc::new(Fake { reconnects: Cell::new(0), connect_on: u32::MAX, connected: Cell::new(false) });
        let session: Rc<dyn SessionController> = fake.clone();
        let controller = Rc::new(RefCell::new(Some(Rc::downgrade(&session))));
        let screen = Rc::new(Screen { handed: RefCell::default(), takes: 3 });
        let ui: Rc<dyn Terminal> = screen.clone();
        let terminal = Rc::new(RefCell::new(Some(Rc::downgrade(&ui))));
        let counting = |looks: Rc<Cell<u32>>, follow: Option<DawId>| {
            move || {
                looks.set(looks.get() + 1);
                async move { Looked { open: follow.into_iter().collect(), follow } }
            }
        };
        let looks = Rc::new(Cell::new(0));
        let watch = |look, controller, terminal| follow_the_daw(DawId::Ableton, vec![], look, controller, terminal, 1, 0);
        watch(counting(looks.clone(), Some(DawId::Renoise)), controller.clone(), terminal.clone()).await;
        assert_eq!(*screen.handed.borrow(), [DawId::Renoise; 3], "handed again until the screen takes it");
        assert_eq!(looks.get(), 3);
        // Nothing open instead: nothing handed.
        let looks = Rc::new(Cell::new(0));
        let watching = tokio::task::spawn_local(watch(counting(looks.clone(), None), controller.clone(), terminal.clone()));
        tokio::time::sleep(std::time::Duration::from_millis(30)).await;
        assert!(looks.get() > 1 && screen.handed.borrow().len() == 3, "looked {} times", looks.get());
        watching.abort();
        // Connected: Kumi works with its DAW, and looks for none; the watch ends with the session.
        fake.connected.set(true);
        let looks = Rc::new(Cell::new(0));
        let watching = tokio::task::spawn_local(watch(counting(looks.clone(), Some(DawId::Renoise)), controller, terminal));
        tokio::time::sleep(std::time::Duration::from_millis(30)).await;
        assert!(looks.get() < 30 / CONNECTED_LOOKS + 2, "looked {} times while connected", looks.get());
        assert_eq!(screen.handed.borrow().len(), 3, "nothing handed while connected");
        drop((fake, session));
        tokio::time::timeout(std::time::Duration::from_secs(5), watching).await.expect("the watch ends with the session").unwrap();
    }

    /// The session's own DAW gone a moment (Live restarting) with another open beside it all along: that one is handed
    /// over only once Live has been gone the while given. One opened since is handed over at once.
    #[tokio::test(flavor = "current_thread")]
    async fn a_daw_restarting_isnt_followed_away_from_but_one_opened_since_is_followed_at_once() {
        tokio::task::LocalSet::new().run_until(follow_after_a_restart()).await;
    }
    async fn follow_after_a_restart() {
        use kumi_types::daw::DawId;
        let fake = Rc::new(Fake { reconnects: Cell::new(0), connect_on: u32::MAX, connected: Cell::new(false) });
        let session: Rc<dyn SessionController> = fake.clone();
        let controller = Rc::new(RefCell::new(Some(Rc::downgrade(&session))));
        let handed = |beside: Vec<DawId>| {
            let controller = controller.clone();
            async move {
                let screen = Rc::new(Screen { handed: RefCell::default(), takes: 1 });
                let ui: Rc<dyn Terminal> = screen.clone();
                let terminal = Rc::new(RefCell::new(Some(Rc::downgrade(&ui))));
                // Live and `beside` open as Kumi starts, then Live gone, with REAPER open.
                let look = || async { Looked { open: vec![DawId::Reaper], follow: Some(DawId::Reaper) } };
                let began = std::time::Instant::now();
                follow_the_daw(DawId::Ableton, [vec![DawId::Ableton], beside].concat(), look, controller, terminal, 1, 200).await;
                assert_eq!(*screen.handed.borrow(), [DawId::Reaper]);
                began.elapsed()
            }
        };
        let restart = handed(vec![DawId::Reaper]).await;
        assert!(restart >= std::time::Duration::from_millis(200), "REAPER, open beside Live, followed {restart:?} after Live went");
        let opened = handed(vec![]).await;
        assert!(opened < std::time::Duration::from_millis(200), "REAPER, opened since, followed {opened:?} after Live went");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn told_to_restart_live_the_watch_says_nothing_else_and_connects_once_live_answers() {
        // Live runs another version of the bridge (right after kumi bridge, the installed one is current): the
        // session has said to restart Live, so the watch never says to choose AbletonMcpBridge, and connects once
        // Live, restarted, answers with the one installed.
        let (reconnects, notices, watching) = watch_told(2, true, true, 0, true).await;
        assert_eq!(reconnects, 2);
        assert_eq!(notices, ["Live answered, so Kumi is connected to it now."]);
        assert!(!watching);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_live_already_answering_is_reconnected_and_tried_again_until_connected() {
        let (reconnects, notices, watching) = watch(3, true, false, 0).await;
        assert_eq!(reconnects, 3, "at once, then again while Live answers and the session isn't connected");
        assert_eq!(notices, [live_words().unreachable, "Live answered, so Kumi is connected to it now."]);
        assert!(!watching);
        // A Live that answers but never connects gets a bounded number of tries.
        let (reconnects, _, _) = watch(u32::MAX, true, false, 0).await;
        assert_eq!(reconnects, 10);
        // A Live that never answers isn't asked at all.
        assert_eq!(watch(1, false, false, 0).await.0, 0);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn an_open_live_that_is_still_starting_isnt_called_unreachable() {
        // First-run setup has just opened Live: Kumi says it's waiting, and it connects within the hold.
        let (reconnects, notices, watching) = watch(1, true, true, 60_000).await;
        assert_eq!((reconnects, watching), (1, false));
        assert_eq!(notices, [live_words().starting, "Live answered, so Kumi is connected to it now."]);
        // An open Live that doesn't answer (AbletonMcpBridge not chosen, say): how to fix it, once the hold is over.
        let (_, notices, _) = watch(1, false, true, 20).await;
        assert_eq!(notices, [live_words().starting, live_words().unreachable]);
        // A Live that isn't open is said at once.
        let (_, notices, _) = watch(1, false, false, 60_000).await;
        assert_eq!(notices, [live_words().unreachable]);
    }
}
