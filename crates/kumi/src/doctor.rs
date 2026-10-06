//! `kumi doctor`; native installs also diagnose legacy bridge configurations.
use crate::{
    config::*,
    live_extension::*,
    models::OFFER_ORDER,
    spinner::step,
    tui::{
        style::{detect_color_depth, os_release, ColorDepth},
        tty::TtyOutput,
    },
    voice::system_language,
};
use futures::future::{join_all, LocalBoxFuture};
use kumi_common::{
    js::{
        number,
        string::{head, trim},
    },
    time::now_ms,
};
use kumi_runtime::{
    auth::{
        openai_codex::OPENAI_CODEX,
        store::{open_credential_store, Credential, CredentialStore},
    },
    core::errors::RuntimeError,
    hands::{can_build_hands, open_hands, OpenHandsOptions},
    integrations::ableton::project::since,
    library::{
        learn::LearnPhase,
        sources::{dirname, join},
        state::read_state,
    },
    providers::{
        api_key_for,
        local::{list_local_models, local_installed, local_servers, parse_local_model_id, probe_local, start_hint, LocalKind, LocalServer},
        models::{ModelInfo, Transport},
        parse_model_id, provider_info, KeySource, ProviderId, SignIn,
    },
    system::{self, Env},
    video::programs::{ffmpeg_hint, find_ffmpeg, find_whisper, whisper_hint, FfmpegOptions, ProgramOptions},
    voice::{microphone::terminal_app, voice_readiness, ReadinessOptions, VoiceReadiness},
    INSTALLED, KUMI, KUMI_REPAIR, KUMI_VERSION,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{fs, path::Path, rc::Rc, time::Duration};
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CheckStatus {
    Ok,
    Note,
    Fix,
}
impl CheckStatus {
    fn as_str(&self) -> &str {
        match self {
            Self::Ok => "ok",
            Self::Note => "note",
            Self::Fix => "fix",
        }
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Check {
    pub status: CheckStatus,
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next: Option<String>,
}
impl Check {
    pub fn ok(text: impl Into<String>) -> Self {
        Self { status: CheckStatus::Ok, text: text.into(), next: None }
    }
    pub fn note(text: impl Into<String>, next: Option<String>) -> Self {
        Self { status: CheckStatus::Note, text: text.into(), next }
    }
    pub fn fix(text: impl Into<String>, next: Option<String>) -> Self {
        Self { status: CheckStatus::Fix, text: text.into(), next }
    }
}
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LiveProbe {
    pub started: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub connected: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub live_version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub set: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub real_live: Option<bool>,
    /// Why the bridge didn't start, in Kumi's words.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub said: Option<String>,
}
#[derive(Debug, Clone, Default)]
pub struct TerminalInfo {
    pub is_tty: bool,
    pub columns: Option<i32>,
    pub rows: Option<i32>,
}
#[derive(Debug, Clone, Default)]
pub struct VideoPrograms {
    pub ffmpeg: Option<String>,
    pub whisper: Option<String>,
}
pub type AsyncCheck<T> = Rc<dyn Fn() -> LocalBoxFuture<'static, Result<T, RuntimeError>>>;
#[derive(Clone)]
pub struct DoctorIo {
    pub out: Rc<dyn TtyOutput>,
    pub env: Env,
    /// Optional legacy Node diagnostic; native installations need no JavaScript runtime.
    pub node_version: Option<String>,
    pub terminal: Option<TerminalInfo>,
    pub probe_live: Option<Rc<dyn Fn(String) -> LocalBoxFuture<'static, Result<LiveProbe, RuntimeError>>>>,
    pub node_version_of: Option<Rc<dyn Fn(String) -> LocalBoxFuture<'static, Option<String>>>>,
    pub bundled_bridge_version: Option<String>,
    pub video_programs: Option<AsyncCheck<VideoPrograms>>,
    pub hands: Option<AsyncCheck<Option<Check>>>,
    pub model_servers: Option<AsyncCheck<Vec<ServerFinding>>>,
    pub voice: Option<AsyncCheck<VoiceReadiness>>,
}
impl DoctorIo {
    pub fn new(out: Rc<dyn TtyOutput>, env: Env) -> Self {
        Self {
            out,
            env,
            node_version: None,
            terminal: None,
            probe_live: None,
            node_version_of: None,
            bundled_bridge_version: None,
            video_programs: None,
            hands: None,
            model_servers: None,
            voice: None,
        }
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerFinding {
    pub server: LocalServer,
    pub running: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub models: Option<Vec<ModelInfo>>,
}
fn tilde(path: &str) -> String {
    let home = home::home_dir().unwrap_or_default().display().to_string();
    if !home.is_empty() {
        path.strip_prefix(&home).map(|tail| format!("~{tail}")).unwrap_or_else(|| path.into())
    } else {
        path.into()
    }
}
fn major(version: &str) -> f64 {
    number::parse(version.strip_prefix('v').unwrap_or(version).split('.').next().unwrap_or("")).unwrap_or(f64::NAN)
}
fn newer(left: &str, right: &str) -> bool {
    let a: Vec<_> = left.split('.').map(|s| number::parse(s).unwrap_or(f64::NAN)).collect();
    let b: Vec<_> = right.split('.').map(|s| number::parse(s).unwrap_or(f64::NAN)).collect();
    for i in 0..3 {
        let x = a.get(i).copied().unwrap_or(0.);
        let y = b.get(i).copied().unwrap_or(0.);
        if x != y {
            return x > y;
        }
    }
    false
}
fn node_check(version: &str) -> Check {
    let name = version.strip_prefix('v').unwrap_or(version);
    let m = major(version);
    if m > 24. {
        Check::ok(format!("Node.js {name} (Kumi is tested on 22 and 24)"))
    } else if [22., 24.].contains(&m) {
        Check::ok(format!("Node.js {name}"))
    } else {
        Check::fix(
            format!("Node.js {name} isn't supported (Kumi needs 22 or newer)"),
            Some(if *INSTALLED {
                format!("Run {}, which brings Kumi's own Node back", *KUMI_REPAIR)
            } else {
                format!("Install Node 24 LTS from https://nodejs.org, then run: {}", *KUMI_REPAIR)
            }),
        )
    }
}
async fn signed_in(provider: ProviderId, store: &dyn CredentialStore, env: &Env) -> bool {
    if provider_info(provider).sign_in == SignIn::Chatgpt {
        matches!(store.get(OPENAI_CODEX).await.ok().flatten(), Some(Credential::Oauth(_)))
    } else {
        api_key_for(provider, store, Some(env)).await.ok().flatten().is_some()
    }
}
async fn sign_in_check(env: &Env, servers: &[ServerFinding]) -> Result<Check, RuntimeError> {
    let store = open_credential_store(load_auth_file(env)?);
    let settings = read_settings(&load_settings_file(env)?);
    let model = env.get("KUMI_MODEL").cloned().or(settings.model.clone());
    let Some(model) = model.filter(|m| !m.is_empty()) else {
        for provider in OFFER_ORDER {
            if signed_in(provider, &store, env).await {
                return Ok(Check::ok(format!(
                    "Signed in to {} · Kumi starts with its first model (/model changes it)",
                    provider_info(provider).name
                )));
            }
        }
        if let Some(serving) = servers.iter().find(|f| f.running && f.models.as_ref().is_some_and(|m| !m.is_empty())) {
            return Ok(Check::ok(format!(
                "Kumi starts with a model in {}, {}; no sign-in needed (/model changes it)",
                serving.server.name, serving.server.r#where
            )));
        }
        return Ok(Check::fix("Not signed in to a provider",Some(format!("{} login openai-codex (a ChatGPT plan), or login anthropic, openai or opencode with an API key; or open Ollama or LM Studio",*KUMI))));
    };
    let parsed = parse_model_id(&model);
    let on_server = if parsed.is_none() { parse_local_model_id(&model, &local_servers(&settings.model_servers, env)?) } else { None };
    if let Some(on) = on_server {
        let server = on.server;
        let found = servers.iter().find(|f| f.server.id == server.id);
        if !found.is_some_and(|f| f.running) {
            return Ok(Check::fix(format!("{} isn't running (model {model})", server.name), Some(start_hint(&server))));
        }
        if found
            .and_then(|f| f.models.as_ref())
            .is_some_and(|models| server.kind != LocalKind::OpenaiCompatible && !models.iter().any(|m| m.model == on.model))
        {
            return Ok(Check::fix(
                format!("{} doesn't have {} (model {model})", server.name, on.model),
                Some(if server.kind == LocalKind::Ollama {
                    format!("Run: ollama pull {}", on.model)
                } else {
                    "Download it in LM Studio, or choose another model with /model in Kumi".into()
                }),
            ));
        }
        return Ok(Check::ok(format!("{} {} · model {model}", server.name, server.r#where)));
    }
    let Some(parsed) = parsed else {
        return Ok(Check::fix(
            format!("The model \"{}\" isn't one Kumi knows", head(&model, 80)),
            Some("Choose one with /model in Kumi".into()),
        ));
    };
    let info = provider_info(parsed.provider);
    if !signed_in(parsed.provider, &store, env).await {
        return Ok(Check::fix(
            format!("Not signed in to {} (model {model})", info.name),
            Some(format!("{} login {}", *KUMI, parsed.provider)),
        ));
    }
    if info.sign_in == SignIn::Chatgpt {
        return Ok(Check::ok(format!("Signed in to ChatGPT · model {model}")));
    }
    let key = api_key_for(parsed.provider, &store, Some(env)).await.ok().flatten();
    Ok(Check::ok(format!(
        "{} API key {} · model {model}",
        parsed.provider,
        if key.is_some_and(|key| key.source == KeySource::Env) {
            format!("from {}", info.key_env.unwrap_or(""))
        } else {
            "saved in Kumi".into()
        }
    )))
}
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct BridgeServer {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub entry: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
}
impl BridgeServer {
    pub fn native(&self) -> bool {
        self.command.as_ref().is_some_and(|c| Path::new(c).file_stem().is_some_and(|s| s == "ableton-mcp-server"))
    }
    pub fn package_root(&self) -> Option<String> {
        if self.native() {
            self.command.as_ref().map(|c| dirname(c))
        } else {
            self.entry.as_ref().map(|entry| dirname(&dirname(&dirname(entry))))
        }
    }
}
/// Read legacy Node entry metadata or the native server's sibling package metadata.
pub fn read_bridge_server(config_path: &str) -> Result<BridgeServer, RuntimeError> {
    let config: Value = serde_json::from_slice(&fs::read(config_path).map_err(|e| RuntimeError::plain(e.to_string()))?)
        .map_err(|e| RuntimeError::plain(e.to_string()))?;
    let nonempty = |v: &Value| v.as_str().filter(|s| !s.is_empty()).map(str::to_string);
    let mut server = BridgeServer {
        command: nonempty(&config["server"]["command"]),
        entry: config["server"]["args"].as_array().and_then(|a| a.first()).and_then(nonempty),
        version: None,
    };
    if server.native() {
        server.entry = server.command.clone();
    }
    if let Some(root) = server.package_root() {
        server.version =
            fs::read(join(&root, "package.json")).ok().and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok()).and_then(|v| {
                let version =
                    if server.native() { v.get("bridge").filter(|v| v.is_string()).or_else(|| v.get("version")) } else { v.get("version") };
                version.and_then(Value::as_str).filter(|s| !s.is_empty()).map(str::to_string)
            });
    }
    Ok(server)
}
async fn extension_check(env: &Env, config: &str, server: &BridgeServer, live: &LiveProbe) -> Option<Check> {
    let folder = live_extensions_dir(env, system::platform())?;
    let version = live.live_version.as_deref().unwrap_or("");
    let parts: Vec<_> = version
        .split('.')
        .map(|p| p.bytes().take_while(u8::is_ascii_digit).collect::<Vec<_>>())
        .map(|p| String::from_utf8_lossy(&p).parse::<f64>().unwrap_or(0.))
        .collect();
    let m = parts.first().copied().unwrap_or(0.);
    let minor = parts.get(1).copied().unwrap_or(0.);
    if !version.is_empty() && (m < 12. || (m == 12. && minor < 4.)) {
        return Some(Check::note(
            format!("Live {version} runs no extensions (12.4 and later do), so Kumi can't render tracks without playing them"),
            None,
        ));
    }
    let carried = server.package_root().and_then(|p| extension_source(&p)).and_then(|p| read_extension(&p));
    let installed = installed_extension(&folder);
    let again = Some(format!("Run: {} bridge, then restart Live", *KUMI));
    let Some(installed) = installed else {
        return carried.map(|_| {
            Check::fix(
                "Kumi's extension isn't in Live (it renders tracks without playing them and writes MIDI clips in the Arrangement)",
                again,
            )
        });
    };
    if carried.as_ref().is_some_and(|c| c.digest != installed.digest) {
        return Some(Check::fix("Kumi's extension in Live is from another bridge", again));
    }
    let in_live = extension_data_dir(&folder);
    let running = running_extension(&in_live).or_else(|| running_extension(&join(&dirname(config), "live-extension")));
    if let Some(running) = running {
        return Some(if !extension_answers(running.port, 2000).await {
            Check::fix("Kumi's extension is running but doesn't answer", Some("Restart Live".into()))
        } else {
            Check::ok(if running.folder == in_live {
                "Kumi's extension is running in Live"
            } else {
                "Kumi's extension is running (Kumi started it: Live's Developer Mode is on)"
            })
        });
    }
    Some(if live.connected == Some(true) {
        Check::note("Live hasn't started Kumi's extension",Some("Restart Live: it starts extensions when it opens. With Developer Mode on (Settings → Extensions), Kumi starts it itself while Kumi runs".into()))
    } else {
        Check::ok(format!("Kumi's extension {} is in Live; it starts with Live", installed.version))
    })
}
async fn hands_check() -> Option<Check> {
    if system::platform() == "win32" {
        return Some(Check::ok("Uses Live's own menus for what Live's scripting can't do (grouping, freezing, bouncing, saving)"));
    }
    if system::platform() != "darwin" {
        return None;
    }
    let hands = open_hands(OpenHandsOptions { build: Some(false), timeout_ms: Some(3000), ..Default::default() }).await.ok().flatten();
    let Some(hands) = hands else {
        return Some(if can_build_hands() {
            Check::ok("Uses Live's own menus (Kumi builds its helper the first time it needs it)")
        } else {
            Check::note(
                "Kumi can't use Live's own menus here yet (grouping, freezing, bouncing, saving)",
                Some("Install Xcode's command line tools (xcode-select --install), or update Kumi".into()),
            )
        });
    };
    let result = hands.trusted(false).await.ok().map(|trusted| {
        if trusted {
            Check::ok("Uses Live's own menus (Accessibility is on for this terminal)")
        } else {
            Check::fix(
                "Kumi can't use Live's own menus until Accessibility is on for this terminal",
                Some("System Settings › Privacy & Security › Accessibility: turn on the app Kumi runs in".into()),
            )
        }
    });
    hands.close();
    result
}
fn grouped(n: usize) -> String {
    let raw = n.to_string();
    raw.chars().enumerate().map(|(i, c)| if i > 0 && (raw.len() - i) % 3 == 0 { format!(",{c}") } else { c.to_string() }).collect()
}
async fn library_check(env: &Env) -> Option<Check> {
    let dir = load_library_dir(env).ok()?;
    let state = read_state(&dir).await;
    let counted = |sounds, presets, sets| format!("{} sounds, {} presets, {} Sets", grouped(sounds), grouped(presets), grouped(sets));
    if let Some(state) = state {
        if let Some(learning) = state.learning {
            return Some(Check::ok(format!(
                "Learning your library in the background{}{}",
                if learning.phase == LearnPhase::Sounds && learning.sounds.todo != 0 {
                    format!(": {} of {} new sounds", grouped(learning.sounds.done), grouped(learning.sounds.todo))
                } else {
                    String::new()
                },
                state.last.map(|last| format!(" (knows {})", counted(last.sounds, last.presets, last.sets))).unwrap_or_default()
            )));
        }
        if let Some(last) = state.last {
            return Some(Check::ok(format!(
                "Knows your library: {} (learned {})",
                counted(last.sounds, last.presets, last.sets),
                since(last.finished_at as f64, now_ms() as f64)
            )));
        }
    }
    Some(Check::note(
        "Kumi hasn't learned your library yet",
        Some(format!("It learns by itself while Kumi runs; {} library shows where it's at", *KUMI)),
    ))
}
async fn find_servers(env: &Env) -> Result<Vec<ServerFinding>, RuntimeError> {
    let servers = local_servers(&read_settings(&load_settings_file(env)?).model_servers, env)?;
    Ok(join_all(servers.into_iter().map(|server| async move {
        let running = probe_local(&server, Transport::default()).await;
        let theirs = server.kind == LocalKind::OpenaiCompatible
            || (server.kind == LocalKind::Ollama && env.get("OLLAMA_HOST").is_some_and(|s| !s.is_empty()))
            || local_installed(server.kind, Some(env));
        if !running {
            return theirs.then_some(ServerFinding { server, running, models: None });
        }
        let models = list_local_models(&server, Transport::default()).await.ok();
        Some(ServerFinding { server, running, models })
    }))
    .await
    .into_iter()
    .flatten()
    .collect())
}
fn server_checks(servers: &[ServerFinding], env: &Env, said: Option<&str>) -> Vec<Check> {
    let mut checks = vec![];
    let named: Vec<_> = servers
        .iter()
        .filter(|f| f.running)
        .map(|f| {
            let what = if let Some(models) = &f.models {
                let able = models.iter().filter(|m| m.tools != Some(false)).count();
                format!(
                    "{} {}{}",
                    models.len(),
                    if models.len() == 1 { "model" } else { "models" },
                    if able != models.len() { format!(", {able} can change the Set") } else { String::new() }
                )
            } else {
                "its models unread".into()
            };
            format!("{} {} ({what})", f.server.name, f.server.r#where)
        })
        .collect();
    if !named.is_empty() {
        checks.push(Check::ok(format!("Model servers: {}", named.join("; "))))
    }
    for found in servers {
        let server = &found.server;
        if found.running || Some(server.id.as_str()) == said {
            continue;
        }
        let text = if server.kind == LocalKind::OpenaiCompatible {
            format!("{}, from settings.json, isn't answering at {}", server.name, server.base_url)
        } else if server.kind == LocalKind::Ollama && env.get("OLLAMA_HOST").is_some_and(|s| !s.is_empty()) {
            format!("Ollama isn't answering at {} (OLLAMA_HOST)", server.base_url)
        } else {
            format!("{} is installed but not running", server.name)
        };
        checks.push(Check::note(text, Some(start_hint(server))))
    }
    checks
}
pub fn voice_check(voice: &VoiceReadiness, env: &Env, platform: &str) -> Check {
    if !voice.fetches && (voice.ffmpeg.is_none() || voice.whisper.is_none()) {
        let both = voice.ffmpeg.is_none() && voice.whisper.is_none();
        let mut needed = vec![];
        if voice.ffmpeg.is_none() {
            needed.push(if platform == "darwin" { "ffmpeg" } else { ffmpeg_hint() })
        }
        if voice.whisper.is_none() {
            needed.push(if platform == "darwin" { "whisper-cpp" } else { whisper_hint() })
        }
        let install = if platform == "darwin" { format!("brew install {}", needed.join(" ")) } else { needed.join("; ") };
        return Check::note(
            format!(
                "Talking to Kumi (ctrl+t) needs {}",
                if both {
                    "ffmpeg and whisper.cpp"
                } else if voice.ffmpeg.is_none() {
                    "ffmpeg"
                } else {
                    "whisper.cpp"
                }
            ),
            Some(format!("Install {}: {install}", if both { "them" } else { "it" })),
        );
    }
    if voice.allowed == Some(false) {
        return Check::note(
            format!("Talking to Kumi (ctrl+t): macOS isn't letting {} use the microphone", terminal_app(env)),
            Some("Allow it in System Settings › Privacy & Security › Microphone".into()),
        );
    }
    let mut later = vec![];
    if voice.ffmpeg.is_none() {
        later.push("ffmpeg")
    }
    if voice.whisper.is_none() {
        later.push("whisper.cpp")
    }
    if voice.model.path.is_none() {
        later.push("its speech model (about 190 MB)")
    }
    if !later.is_empty() {
        let mut words = later.join(" and ");
        if later.len() >= 3 {
            words = words.replacen(" and ", ", ", 1)
        }
        return Check::ok(format!("Talking to Kumi (ctrl+t): Kumi fetches {words} the first time you talk"));
    }
    Check::ok("Talking to Kumi (ctrl+t): ffmpeg hears the microphone, whisper.cpp writes down what you say, on this computer")
}
pub async fn doctor_checks(io: &DoctorIo) -> Result<Vec<Check>, RuntimeError> {
    let env = &io.env;
    let node = io.node_version.as_deref().map(node_check).unwrap_or_else(|| Check::ok(format!("Kumi {KUMI_VERSION} (native executable)")));
    let servers = match &io.model_servers {
        Some(find) => find().await,
        None => find_servers(env).await,
    }
    .unwrap_or_default();
    let sign_in = sign_in_check(env, &servers).await?;
    let said = servers.iter().find(|s| sign_in.text.starts_with(&format!("{} isn't running", s.server.name))).map(|s| s.server.id.as_str());
    let mut checks = vec![node.clone(), sign_in];
    checks.extend(server_checks(&servers, env, said));
    if let Some(config) = find_bridge_config(env) {
        let server = read_bridge_server(&config).unwrap_or_default();
        let version = server.version.as_ref().map(|s| format!(" {s}")).unwrap_or_default();
        checks.push(Check::ok(format!("Ableton bridge{version} ({})", tilde(&config))));
        if let (Some(installed), Some(bundled)) = (&server.version, &io.bundled_bridge_version) {
            if newer(bundled, installed) {
                checks.push(Check::fix(
                    format!("The installed bridge ({installed}) is older than this Kumi's ({bundled})"),
                    Some(format!("Quit Live, then run: {} bridge", *KUMI)),
                ))
            }
        }
        let later = if server.native() {
            format!("Run {} bridge to record Kumi's bridge executable", *KUMI)
        } else if *INSTALLED {
            format!("Kumi isn't affected. The next {} bridge records Kumi's own Node", *KUMI)
        } else {
            format!("Kumi isn't affected. Install Node 24 LTS (nodejs.org); the next {} bridge records it", *KUMI)
        };
        if let Some(command) = &server.command {
            if !access(command, true) {
                checks.push(Check::note(
                    if server.native() {
                        format!("Other MCP apps would start the bridge with an executable that's missing ({})", tilde(command))
                    } else {
                        format!("Other MCP apps would start the bridge with a Node that's missing ({})", tilde(command))
                    },
                    Some(later),
                ))
            } else {
                if !server.native() {
                    let bridge_node =
                        if let Some(version) = &io.node_version_of { version(command.clone()).await } else { node_version(command).await };
                    if let Some(version) = bridge_node {
                        if ![22., 24.].contains(&major(&version)) {
                            checks.push(Check::note(
                                format!(
                                    "Other MCP apps would start the bridge with Node.js {}, which it doesn't support",
                                    version.strip_prefix('v').unwrap_or(&version)
                                ),
                                Some(later.clone()),
                            ))
                        }
                    }
                }
                if regex::Regex::new(r"(?i)[\\/](_npx|\.npm[\\/]_npx|tmp|Temp)[\\/]").unwrap().is_match(command) {
                    checks.push(Check::note(
                        if server.native() {
                            "Other MCP apps would start the bridge with an executable from a temporary folder, which can disappear"
                        } else {
                            "Other MCP apps would start the bridge with a Node from a temporary folder, which can disappear"
                        },
                        Some(later),
                    ))
                }
            }
        } else {
            checks.push(Check::note("The bridge configuration names no Node for other MCP apps", Some(later)))
        }
        let live = if let Some(probe) = &io.probe_live { probe(config.clone()).await.unwrap_or_default() } else { LiveProbe::default() };
        if !live.started && live.said.as_deref().is_some_and(|said| said.contains(kumi_common::bridge::ANOTHER_BRIDGE)) {
            checks.push(Check::fix(
                "Live is running another version of Kumi's bridge",
                Some(format!(
                    "Restart Live: it loads its bridge when it starts. If it still says this, quit Live and run {} bridge. Then: {} doctor",
                    *KUMI, *KUMI
                )),
            ))
        } else if !live.started {
            let current = server
                .version
                .as_ref()
                .zip(io.bundled_bridge_version.as_ref())
                .is_some_and(|(installed, bundled)| !newer(bundled, installed));
            checks.push(if node.status==CheckStatus::Fix{Check::note("The bridge didn't start; it needs Node 22 or 24 too",None)}else if current{Check::fix("Kumi's bridge couldn't reach Live",Some(format!("Open Live and choose AbletonMcpBridge as a Control Surface (Settings → Link, Tempo & MIDI); if Live is showing a dialog, answer it first. Then: {} doctor",*KUMI)))}else{Check::fix("The bridge didn't start",Some(format!("{}, and bring Live's part up to date: quit Live, then run {} bridge. Then: {} doctor",if *INSTALLED{format!("Run {} to repair Kumi",*KUMI_REPAIR)}else{format!("Build it ({})",*KUMI_REPAIR)},*KUMI,*KUMI)))})
        } else if live.connected != Some(true) {
            checks.push(Check::fix(
                "Live isn't connected",
                Some("Open Live and choose AbletonMcpBridge as a Control Surface (Settings → Link, Tempo & MIDI)".into()),
            ))
        } else {
            let location = format!(
                "{} connected{}",
                live.live_version.as_ref().filter(|s| !s.is_empty()).map(|s| format!("Live {s}")).unwrap_or("Live".into()),
                live.set.as_ref().filter(|s| !s.is_empty()).map(|s| format!(" · {s}")).unwrap_or_default()
            );
            checks.push(if live.real_live == Some(false) {
                Check::note(format!("{location} (a simulator, not real Live)"), None)
            } else {
                Check::ok(location)
            })
        }
        if live.real_live != Some(false) {
            if let Some(extension) = extension_check(env, &config, &server, &live).await {
                checks.push(extension)
            }
        }
    } else {
        checks.push(Check::fix(
            "The Ableton bridge isn't installed, so Kumi can't see Live",
            Some(format!("Quit Live, then run: {} bridge", *KUMI)),
        ))
    }
    if let Ok(projects) = load_projects_dir(env) {
        checks.push(if access(&projects, false) || access(&dirname(&projects), false) {
            Check::ok(format!("Remembers Sets in {}", tilde(&projects)))
        } else {
            Check::note(
                format!("Can't write {}, so Kumi won't catch you up on Sets", tilde(&projects)),
                Some("Check that folder's permissions, or set KUMI_PROJECTS_DIR".into()),
            )
        })
    }
    if let Some(library) = library_check(env).await {
        checks.push(library)
    }
    let programs = if let Some(programs) = &io.video_programs {
        programs().await
    } else {
        async {
            let tools = load_tools_dir(env)?;
            Ok(VideoPrograms {
                ffmpeg: find_ffmpeg(FfmpegOptions {
                    env: Some(env.clone()),
                    tools_dir: Some(tools.clone()),
                    installed_only: true,
                    ..Default::default()
                })
                .await
                .map_err(|e| RuntimeError::plain(e.to_string()))?,
                whisper: find_whisper(&ProgramOptions {
                    env: Some(env.clone()),
                    tools_dir: tools,
                    installed_only: true,
                    ..Default::default()
                })
                .await
                .map_err(|e| RuntimeError::plain(e.to_string()))?,
            })
        }
        .await
    }
    .unwrap_or_default();
    checks.push(if programs.ffmpeg.is_none() {
        Check::note("Kumi reads a video's words but can't see its frames without ffmpeg", Some(format!("Install it: {}", ffmpeg_hint())))
    } else if programs.whisper.is_none() {
        Check::note("Watches videos; one without captions needs whisper.cpp for its words", Some(format!("Install it: {}", whisper_hint())))
    } else {
        Check::ok("Watches videos: frames with ffmpeg, speech with whisper.cpp")
    });
    let hands = if let Some(hands) = &io.hands { hands().await? } else { hands_check().await };
    if let Some(hands) = hands {
        checks.push(hands)
    }
    let voice = if let Some(voice) = &io.voice {
        voice().await.ok()
    } else {
        match (load_tools_dir(env), load_settings_file(env)) {
            (Ok(tools_dir), Ok(settings)) => voice_readiness(ReadinessOptions {
                env: Some(env.clone()),
                tools_dir,
                language: Some(read_settings(&settings).voice.and_then(|v| v.language).unwrap_or_else(|| system_language(env))),
                platform: None,
            })
            .await
            .ok(),
            _ => None,
        }
    };
    if let Some(voice) = voice {
        checks.push(voice_check(&voice, env, system::platform()))
    }
    let terminal =
        io.terminal.clone().unwrap_or_else(|| TerminalInfo { is_tty: io.out.is_tty(), columns: io.out.columns(), rows: io.out.rows() });
    if !terminal.is_tty {
        checks.push(Check::note("Not a terminal window here, so Kumi uses plain lines", None))
    } else {
        let colour = match detect_color_depth(env, system::platform(), &os_release()) {
            ColorDepth::Truecolor => "24-bit colour",
            ColorDepth::Colors256 => "256 colours",
            ColorDepth::Colors16 => "16 colours",
            ColorDepth::None => "no colour",
        };
        let columns = terminal.columns.unwrap_or(80);
        let rows = terminal.rows.unwrap_or(24);
        let size = format!("{columns}×{rows}");
        checks.push(if columns < 60 || rows < 16 {
            Check::note(format!("Terminal {size} is small for Kumi's full screen"), Some("Make the window bigger".into()))
        } else {
            Check::ok(format!(
                "Terminal {size}, {colour}{}",
                if env.get("KUMI_UI").is_some_and(|v| v == "plain") { ", plain lines (KUMI_UI=plain)" } else { "" }
            ))
        })
    }
    Ok(checks)
}
fn access(path: &str, execute: bool) -> bool {
    #[cfg(unix)]
    {
        let Ok(path) = std::ffi::CString::new(path) else { return false };
        unsafe { libc::access(path.as_ptr(), if execute { libc::X_OK } else { libc::W_OK }) == 0 }
    }
    #[cfg(not(unix))]
    {
        fs::metadata(path).is_ok_and(|m| execute || !m.permissions().readonly())
    }
}
async fn node_version(command: &str) -> Option<String> {
    let mut process = tokio::process::Command::new(command);
    process.arg("--version").env_clear().env("PATH", dirname(command)).kill_on_drop(true);
    #[cfg(windows)]
    process.creation_flags(0x0800_0000);
    let result = tokio::time::timeout(Duration::from_secs(5), process.output()).await.ok()?.ok()?;
    if !result.status.success() {
        return None;
    }
    let text = head(trim(&String::from_utf8_lossy(&result.stdout)), 32);
    (!text.is_empty()).then_some(text)
}
pub async fn run_doctor(io: DoctorIo) -> Result<i32, RuntimeError> {
    let checks = step(io.out.clone(), &io.env, "Checking…", doctor_checks(&io), false).await?;
    io.out.write(&format_doctor(&checks));
    Ok(if checks.iter().any(|c| c.status == CheckStatus::Fix) { 1 } else { 0 })
}
pub fn format_doctor(checks: &[Check]) -> String {
    let mut lines = vec!["Kumi doctor".into(), "".into()];
    for check in checks {
        lines.push(format!("  {:5} {}", check.status.as_str(), check.text));
        if let Some(next) = check.next.as_ref().filter(|s| !s.is_empty()) {
            lines.push(format!("        → {next}"))
        }
    }
    let fixes = checks.iter().filter(|c| c.status == CheckStatus::Fix).count();
    lines.push("".into());
    lines.push(if fixes > 0 {
        format!("{fixes} {} to fix (see →).", if fixes == 1 { "thing" } else { "things" })
    } else {
        "Everything Kumi needs is in place.".into()
    });
    format!("{}\n", lines.join("\n"))
}
