//! Opt-in run of Kumi on real Live with your sign-in and model: requests in a row, as the producer would type them,
//! with each judged round, tool call and answer printed as it happens. It uses the bridge from the same build and a
//! throwaway folder for notes and recipes, so ~/.kumi isn't touched. It changes the open Set: use a disposable one.
//!   cargo build --profile ci-release -p ableton-mcp-server --bins
//!   cargo run --profile ci-release -p kumi --example loop_live -- --set "<Set name>" [--listener] "<request>" […]
//! With --listener, the judge asks the listening model as the app would (an OpenAI API key, or KUMI_LISTENER).
use futures::FutureExt;
use kumi::config::{find_bridge_config, load_inference_config, safe_error};
use kumi_common::{
    abort::Signal,
    js::string::{head, trim},
    time::perf_now,
};
use kumi_runtime::{
    core::{
        contracts::{ActionEvent, KernelEvent, KernelTool, ToolResult, WatchEvent},
        memory::MemoryStoreOptions,
    },
    create_ableton_integration, create_agent_kernel, create_memory_store, create_recipe_store, create_session,
    integrations::ableton::{connection::Connect, options::ListenerSource, AbletonOptions},
    kernel::agent::AgentKernelOptions,
    kernel::agent::ModelBinding,
    listening::listener::{listener_from_env, listening_off, openai_listener, Listener},
    mcp::client,
    open_credential_store,
    providers::{api_key_for, resolve_model, ProviderId, ResolveModelOptions},
    system::process_env,
    ChangeRecord, IntegrationFactory, Kernel, KernelFactory, KernelOptions, RuntimeError, Session, SessionController, SessionEvent,
    SessionOptions, BRIDGE_TOOLS,
};
use std::{
    cell::RefCell,
    path::{Path, PathBuf},
    rc::{Rc, Weak},
};

fn main() {
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().expect("a runtime");
    let code = tokio::task::LocalSet::new().block_on(&runtime, async {
        match run().await {
            Ok(code) => code,
            Err(error) => {
                eprintln!("loop-live: {}", safe_error(Some(&error), &[]));
                1
            }
        }
    });
    std::process::exit(code);
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

/// A tool that prints its failures (the model sees them; the log should too).
struct Logged(Rc<dyn KernelTool>);
#[async_trait::async_trait(?Send)]
impl KernelTool for Logged {
    fn name(&self) -> &str {
        self.0.name()
    }
    fn description(&self) -> &str {
        self.0.description()
    }
    fn input_schema(&self) -> kumi_runtime::core::contracts::JsonObject {
        self.0.input_schema()
    }
    async fn execute(&self, input: kumi_runtime::core::contracts::JsonObject, signal: Signal) -> Result<ToolResult, RuntimeError> {
        let result = self.0.execute(input, signal).await;
        match &result {
            Ok(done) if done.is_error => println!("  ✗ {}: {}", self.0.name(), head(&done.text, 400)),
            Err(error) => println!("  ✗ {}: {}", self.0.name(), head(&error.to_string(), 400)),
            _ => {}
        }
        result
    }
}

/// A fresh binding of the same model for each kernel.
fn rebind(binding: &Rc<ModelBinding>) -> ModelBinding {
    let (prepare, budget) = (binding.clone(), binding.clone());
    ModelBinding {
        id: binding.id.clone(),
        model: binding.model.clone(),
        prepare: Box::new(move |request| (prepare.prepare)(request)),
        budget: binding.budget.as_ref().map(|_| Box::new(move |fixed| (budget.budget.as_ref().unwrap())(fixed)) as Box<_>),
    }
}

async fn run() -> Result<i32, RuntimeError> {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let at = argv.iter().position(|arg| arg == "--set").ok_or_else(|| RuntimeError::plain("Name the Set: --set \"<Set name>\""))?;
    let wanted = trim(argv.get(at + 1).map(String::as_str).unwrap_or("")).to_owned();
    let listening = argv.iter().any(|arg| arg == "--listener");
    let prompts: Vec<String> = argv
        .iter()
        .enumerate()
        .filter(|(index, arg)| *index != at && *index != at + 1 && *arg != "--listener")
        .map(|(_, prompt)| prompt.clone())
        .collect();
    if wanted.is_empty() || prompts.is_empty() {
        return Err(RuntimeError::plain("Usage: loop_live --set \"<Set name>\" \"<request>\" [\"<request>\" …]"));
    }
    let env = process_env();
    let bridge_config = find_bridge_config(&env).ok_or_else(|| RuntimeError::plain("The Ableton bridge isn't installed."))?;
    let bridge = bridge_program();
    // The open Set must be the one named: a look at it first, then nothing is asked if it isn't.
    let set = {
        let mut options = AbletonOptions::new(Rc::new(|_, _| {}));
        options.bridge_config = Some(bridge_config.clone());
        options.connect = Some(connect_to(bridge.clone(), bridge_config.clone()));
        let look = create_ableton_integration(options);
        look.start(Signal::new()).await?;
        let observation = look.observe(Signal::new(), None).await?;
        let _ = look.close().await;
        let context: serde_json::Value = serde_json::from_str(&observation.context).unwrap_or_default();
        context["set"]["name"].as_str().unwrap_or("").to_owned()
    };
    if set != wanted {
        println!("The open Set is “{set}”, not “{wanted}”; nothing was asked.");
        return Ok(2);
    }
    let config = load_inference_config(&env)?;
    let binding = Rc::new(
        resolve_model(ResolveModelOptions {
            model: config.model.clone().unwrap_or_default(),
            store: Rc::new(open_credential_store(&config.auth_file)),
            env: Some(env.clone()),
            fetch: None,
            effort: None,
            service_tier: None,
        })
        .await?,
    );
    println!("Model: {}", binding.id);
    // The listening model, found as the app finds it.
    let listener: Option<ListenerSource> = listening.then(|| {
        let store = Rc::new(open_credential_store(&config.auth_file));
        let env = env.clone();
        Rc::new(move |signal: Signal| {
            let (store, env) = (store.clone(), env.clone());
            async move {
                if listening_off(&env) {
                    return None;
                }
                if let Some(listener) = listener_from_env(&env) {
                    return Some(Rc::new(listener) as Rc<dyn Listener>);
                }
                let key = api_key_for(ProviderId::Openai, store.as_ref(), Some(&env)).await.ok().flatten()?.key;
                openai_listener(&key, signal).await.map(|listener| Rc::new(listener) as Rc<dyn Listener>)
            }
            .boxed_local()
        }) as ListenerSource
    });
    if let Some(find) = &listener {
        match find(Signal::new()).await {
            Some(found) => println!("Listening model: {}", found.name()),
            None => println!("Listening model: none found (no OpenAI API key, and KUMI_LISTENER isn't set)"),
        }
    }
    let folder = tempfile::Builder::new().prefix("kumi-loop-live-").tempdir().map_err(|error| RuntimeError::plain(error.to_string()))?;
    let place = folder.path().to_path_buf();
    let controller = Rc::new(RefCell::new(None::<Weak<Session>>));
    let started = perf_now();
    let kernel_factory: KernelFactory = {
        let binding = binding.clone();
        Rc::new(move |options: KernelOptions| {
            let made = create_agent_kernel(AgentKernelOptions {
                conversation: None,
                instructions: options.instructions,
                // Logged tools can't stream, so a plan runs once it's written.
                tools: options.tools.into_iter().map(|tool| Rc::new(Logged(tool)) as Rc<dyn KernelTool>).collect(),
                signal: options.signal,
                checkpoint: options.checkpoint,
                binding: rebind(&binding),
                max_steps: None,
                budget: None,
            });
            async move { made.map(|made| Rc::new(made) as Rc<dyn Kernel>) }.boxed_local()
        })
    };
    let integration_factory: IntegrationFactory = {
        let controller = controller.clone();
        Box::new(move |on_connection| {
            let mut options = AbletonOptions::new(on_connection);
            options.listener = listener.clone();
            options.bridge_config = Some(bridge_config.clone());
            options.connect = Some(connect_to(bridge.clone(), bridge_config.clone()));
            let watch = |controller: &Rc<RefCell<Option<Weak<Session>>>>, event: WatchEvent| {
                if let Some(session) = controller.borrow().as_ref().and_then(Weak::upgrade) {
                    session.watch(event);
                }
            };
            options.on_change = Some({
                let controller = controller.clone();
                Rc::new(move |change: ChangeRecord| watch(&controller, WatchEvent::Change(change)))
            });
            options.on_action = Some({
                let controller = controller.clone();
                Rc::new(move |action: ActionEvent| watch(&controller, WatchEvent::Action(action)))
            });
            options.on_audition = Some({
                let controller = controller.clone();
                Rc::new(move |event| watch(&controller, WatchEvent::Audition(event)))
            });
            options.on_judge = Some({
                let controller = controller.clone();
                Rc::new(move |round: kumi_runtime::listening::round::Round| {
                    println!();
                    for line in round.lines() {
                        println!("  | {line}");
                    }
                    watch(&controller, WatchEvent::Judged(round));
                })
            });
            create_ableton_integration(options)
        })
    };
    let text = Rc::new(RefCell::new(String::new()));
    let on_event: Rc<dyn Fn(SessionEvent)> = {
        let text = text.clone();
        Rc::new(move |event: SessionEvent| match event {
            SessionEvent::Kernel(KernelEvent::ToolStart { name, .. }) => {
                println!("  [{:>6.1} s] → {name}", (perf_now() - started) / 1000.);
            }
            SessionEvent::Kernel(KernelEvent::ToolEnd { name, is_error, elapsed_ms, .. }) => {
                if is_error {
                    println!("  [{:>6.1} s] ✗ {name} ({} ms)", (perf_now() - started) / 1000., elapsed_ms);
                }
            }
            SessionEvent::Kernel(KernelEvent::Text { text: words }) => text.borrow_mut().push_str(&words),
            SessionEvent::Notice { message } => println!("  notice: {}", head(&message, 300)),
            _ => {}
        })
    };
    let mut options = SessionOptions::new(kernel_factory, integration_factory, on_event);
    options.timeout_ms = Some(3_600_000);
    options.memory =
        Some(create_memory_store(MemoryStoreOptions { projects_dir: place.join("projects"), producer_file: place.join("memory.json") }));
    options.recipes = Some(create_recipe_store(place.join("recipes")));
    options.listen = true;
    let session = Rc::new(create_session(options)?);
    *controller.borrow_mut() = Some(Rc::downgrade(&session));
    session.start().await?;
    for prompt in &prompts {
        println!("\n> {prompt}");
        text.borrow_mut().clear();
        session.submit(prompt, None).await?;
        println!("\n{}\n  ({:.0} s in)", trim(&text.borrow()), (perf_now() - started) / 1000.);
    }
    session.close().await?;
    Ok(0)
}
