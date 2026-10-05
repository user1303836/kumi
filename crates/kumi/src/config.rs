//! Port of `apps/kumi/src/config.ts`.
use kumi_common::js::{
    json::file_text,
    string::{trim, utf16_len},
};
use kumi_runtime::{
    core::errors::RuntimeError,
    library::sources::{is_absolute, join},
    providers::{
        local::{local_servers, parse_local_model_id, ServerSetting, LOCAL_PROVIDERS},
        parse_model_id, provider_info, Effort, ProviderId, SignIn, PROVIDERS,
    },
    system::{self, Env},
    KUMI, KUMI_START,
};
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use std::{io::Write, path::Path, sync::LazyLock};
pub const SUPPORTED_NODE_MAJORS: [u32; 2] = [22, 24];
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InferenceConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    pub auth_file: String,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum LoginMethod {
    Browser,
    Device,
    ImportPi,
    Key,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "kebab-case", rename_all_fields = "camelCase")]
pub enum AppConfig {
    Help,
    Version,
    Bridge {
        yes: bool,
        allow_dirty: bool,
    },
    Auth {
        auth_file: String,
        settings_file: String,
    },
    Login {
        provider: ProviderId,
        method: LoginMethod,
        auth_file: String,
        pi_auth_file: String,
        settings_file: String,
    },
    Logout {
        provider: ProviderId,
        auth_file: String,
    },
    Model {
        settings_file: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        model: Option<String>,
    },
    Doctor,
    Report,
    Update {
        rollback: bool,
        check: bool,
    },
    Uninstall {
        all: bool,
        yes: bool,
    },
    Library {
        rebuild: bool,
    },
    LoginChoose {
        auth_file: String,
        pi_auth_file: String,
        settings_file: String,
    },
    InferenceOnly {
        #[serde(flatten)]
        inference: InferenceConfig,
        #[serde(skip_serializing_if = "Option::is_none")]
        bridge_missing: Option<bool>,
    },
    Live {
        #[serde(flatten)]
        inference: InferenceConfig,
        bridge_config: String,
    },
}
fn home() -> String {
    home::home_dir().unwrap_or_default().to_string_lossy().into()
}
fn controls(s: &str, c1: bool) -> bool {
    s.chars().any(|c| c <= '\u{1f}' || c == '\u{7f}' || c1 && ('\u{80}'..='\u{9f}').contains(&c))
}
fn absolute_file(env: &Env, variable: &str, fallback: String) -> Result<String, RuntimeError> {
    let file = env.get(variable).cloned().unwrap_or(fallback);
    if file.is_empty() || !is_absolute(&file) || controls(&file, true) {
        return Err(RuntimeError::plain(format!("{variable} must be an absolute file path without control characters.")));
    }
    Ok(file)
}
pub fn kumi_dir(env: &Env) -> String {
    env.get("KUMI_HOME").filter(|s| !s.is_empty()).cloned().unwrap_or_else(|| join(&home(), ".kumi"))
}
macro_rules! location {
    ($name:ident,$var:literal,$path:literal) => {
        pub fn $name(env: &Env) -> Result<String, RuntimeError> {
            absolute_file(env, $var, join(&kumi_dir(env), $path))
        }
    };
}
location!(load_auth_file, "KUMI_AUTH_FILE", "auth.json");
location!(load_settings_file, "KUMI_SETTINGS_FILE", "settings.json");
location!(load_memory_file, "KUMI_MEMORY_FILE", "memory.json");
location!(load_techniques_file, "KUMI_TECHNIQUES_FILE", "techniques.json");
location!(load_restore_file, "KUMI_RESTORE_FILE", "audition-restore.json");
location!(load_goals_dir, "KUMI_GOALS_DIR", "goals");
location!(load_playbook_file, "KUMI_PLAYBOOK_FILE", "playbook.json");
location!(load_gaps_file, "KUMI_GAPS_FILE", "gaps.jsonl");
location!(load_recipes_dir, "KUMI_RECIPES_DIR", "recipes");
location!(load_videos_dir, "KUMI_VIDEOS_DIR", "videos");
location!(load_tools_dir, "KUMI_TOOLS_DIR", "tools");
location!(load_input_history_file, "KUMI_INPUT_HISTORY_FILE", "input-history");
location!(load_projects_dir, "KUMI_PROJECTS_DIR", "projects");
location!(load_library_dir, "KUMI_LIBRARY_DIR", "library");
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct VoiceSettings {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub send: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub microphone: Option<String>,
}
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub effort: Option<Effort>,
    /// The model's faster tier, when its provider offers one (`/fast`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fast: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub panel_tab: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub update_check: Option<bool>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub library_folders: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub model_servers: Vec<ServerSetting>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub voice: Option<VoiceSettings>,
}
fn server_settings(value: &Value) -> Vec<ServerSetting> {
    value
        .as_array()
        .into_iter()
        .flatten()
        .take(16)
        .filter_map(|entry| {
            let name = trim(entry.get("name")?.as_str()?);
            let url = url::Url::parse(trim(entry.get("baseURL")?.as_str()?)).ok()?;
            if name.is_empty() || utf16_len(name) > 40 || controls(name, false) || !["http", "https"].contains(&url.scheme()) {
                return None;
            }
            let api_key = entry
                .get("apiKey")
                .and_then(Value::as_str)
                .filter(|k| (1..=4096).contains(&k.len()) && k.bytes().all(|c| (0x21..=0x7e).contains(&c)))
                .map(str::to_string);
            Some(ServerSetting { name: name.into(), base_url: url.to_string(), api_key })
        })
        .collect()
}
fn voice_settings(value: &Value) -> Option<VoiceSettings> {
    let send = (value.get("send") == Some(&Value::Bool(true))).then_some(true);
    let language = value
        .get("language")
        .and_then(Value::as_str)
        .filter(|s| *s == "auto" || (2..=3).contains(&s.len()) && s.bytes().all(|c| c.is_ascii_lowercase()))
        .map(str::to_string);
    let microphone = value
        .get("microphone")
        .and_then(Value::as_str)
        .filter(|s| utf16_len(s) <= 200 && !controls(s, false) && !trim(s).is_empty())
        .map(str::to_string);
    (send.is_some() || language.is_some() || microphone.is_some()).then_some(VoiceSettings { send, language, microphone })
}
fn read_json(file: &str) -> Option<Value> {
    serde_json::from_str(&String::from_utf8_lossy(&std::fs::read(file).ok()?)).ok()
}
pub fn read_settings(file: &str) -> Settings {
    let Some(value) = read_json(file) else { return Settings::default() };
    let model_servers = server_settings(&value["modelServers"]);
    let model = value["model"].as_str().filter(|m| valid_model(Some(m), &model_servers)).map(str::to_string);
    let effort = value["effort"].as_str().and_then(Effort::parse);
    static TAB: LazyLock<Regex> = LazyLock::new(|| Regex::new("^[a-z][a-z0-9-]{0,31}$").unwrap());
    Settings {
        model,
        effort,
        fast: (value["fast"] == true).then_some(true),
        panel_tab: value["panelTab"].as_str().filter(|s| TAB.is_match(s)).map(str::to_string),
        update_check: (value["updateCheck"] == false).then_some(false),
        library_folders: value["libraryFolders"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .filter(|s| !s.is_empty() && utf16_len(s) <= 1024)
            .take(64)
            .map(str::to_string)
            .collect(),
        model_servers,
        voice: voice_settings(&value["voice"]),
    }
}
/// A JSON object preserves the difference between omitting a setting and explicitly clearing it.
pub fn write_settings(file: &str, next: &Value) -> Result<(), RuntimeError> {
    let before = serde_json::to_value(read_settings(file)).unwrap();
    let raw = read_json(file);
    let mut settings = Map::new();
    for key in ["model", "effort", "fast"] {
        if let Some(value) = next.get(key).filter(|v| truthy(v)) {
            settings.insert(key.into(), value.clone());
        }
    }
    let kept = |key: &str| next.get(key).or_else(|| before.get(key));
    if let Some(value) = kept("panelTab").filter(|v| truthy(v)) {
        settings.insert("panelTab".into(), value.clone());
    }
    if kept("updateCheck") == Some(&Value::Bool(false)) {
        settings.insert("updateCheck".into(), json!(false));
    }
    if let Some(value) = kept("libraryFolders").filter(|v| v.as_array().is_some_and(|v| !v.is_empty())) {
        settings.insert("libraryFolders".into(), value.clone());
    }
    if let Some(value) = next.get("modelServers").or_else(|| raw.as_ref().and_then(|r| r.get("modelServers"))) {
        settings.insert("modelServers".into(), value.clone());
    }
    if let Some(value) = kept("voice").and_then(voice_settings) {
        settings.insert("voice".into(), serde_json::to_value(value).unwrap());
    }
    let parent = Path::new(file).parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or(Path::new("."));
    let mut mkdir = std::fs::DirBuilder::new();
    mkdir.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        mkdir.mode(0o700);
    }
    mkdir.create(parent).map_err(|e| RuntimeError::plain(e.to_string()))?;
    let temporary = format!("{file}.{}.tmp", std::process::id());
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options
        .open(&temporary)
        .and_then(|mut f| f.write_all(file_text(&Value::Object(settings)).as_bytes()))
        .and_then(|_| std::fs::rename(temporary, file))
        .map_err(|e| RuntimeError::plain(e.to_string()))
}
fn truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::String(s) => !s.is_empty(),
        Value::Number(n) => n.as_f64().is_some_and(|v| v != 0.0),
        _ => true,
    }
}
pub fn valid_model(model: Option<&str>, servers: &[ServerSetting]) -> bool {
    model.is_some_and(|model| {
        utf16_len(model) <= 256
            && (parse_model_id(model).is_some()
                || local_servers(servers, &system::process_env())
                    .ok()
                    .is_some_and(|servers| parse_local_model_id(model, &servers).is_some()))
    })
}
fn sources() -> String {
    format!("{} (or a server in settings.json)", PROVIDERS.iter().map(|p| p.as_str()).chain(LOCAL_PROVIDERS).collect::<Vec<_>>().join(", "))
}
pub fn load_inference_config(env: &Env) -> Result<InferenceConfig, RuntimeError> {
    let settings = read_settings(&load_settings_file(env)?);
    if let Some(model) = env.get("KUMI_MODEL") {
        if !valid_model(Some(model), &settings.model_servers) {
            return Err(RuntimeError::plain(format!("KUMI_MODEL must be <provider>/<model> with provider one of {}.", sources())));
        }
    }
    Ok(InferenceConfig {
        model: env.get("KUMI_MODEL").cloned().or(settings.model).filter(|s| !s.is_empty()),
        auth_file: load_auth_file(env)?,
    })
}
pub fn live_user_library(env: &Env) -> Option<String> {
    let home = env.get("HOME").cloned().unwrap_or_else(home);
    let preferences = if cfg!(windows) {
        join(&env.get("APPDATA").cloned().unwrap_or_else(|| join(&home, "AppData/Roaming")), "Ableton")
    } else {
        join(&home, "Library/Preferences/Ableton")
    };
    let mut configs = Vec::new();
    for entry in std::fs::read_dir(preferences).ok()? {
        let entry = entry.ok()?;
        if !entry.file_name().to_string_lossy().starts_with("Live ") {
            continue;
        }
        let file = entry.path().join(if cfg!(windows) { "Preferences/Library.cfg" } else { "Library.cfg" });
        if file.exists() {
            let mtime = std::fs::metadata(&file).ok()?.modified().ok()?;
            configs.push((file, mtime));
        }
    }
    configs.sort_by(|a, b| b.1.cmp(&a.1));
    static USER: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?s)<UserLibrary>(.*?)</UserLibrary>").unwrap());
    for (file, _) in configs {
        let text = std::fs::read_to_string(file).ok()?;
        let block = USER.captures(&text).and_then(|c| c.get(1)).map(|m| m.as_str()).unwrap_or("");
        let value = |field: &str| {
            Regex::new(&format!(r#"<{field} Value="([^"]*)""#))
                .unwrap()
                .captures(block)
                .map(|c| c[1].replace("&quot;", "\"").replace("&lt;", "<").replace("&gt;", ">").replace("&amp;", "&"))
        };
        let name = value("ProjectName").filter(|s| !s.is_empty()).unwrap_or("User Library".into());
        if let Some(folder) = value("ProjectPath").filter(|s| !s.is_empty() && is_absolute(s)) {
            return Some(join(&folder, &name));
        }
    }
    None
}
pub fn remote_scripts_dir(env: &Env) -> String {
    if let Some(folder) = env.get("KUMI_REMOTE_SCRIPTS_DIR").filter(|s| !s.is_empty()) {
        return folder.clone();
    }
    if let Some(library) = live_user_library(env) {
        return join(&library, "Remote Scripts");
    }
    let home = home();
    let documents = if cfg!(windows) {
        [join(&home, "OneDrive/Documents"), join(&home, "Documents")]
            .into_iter()
            .find(|p| Path::new(&join(p, "Ableton")).exists())
            .unwrap_or_else(|| join(&home, "Documents"))
    } else {
        join(&home, "Music")
    };
    join(&documents, "Ableton/User Library/Remote Scripts")
}
pub fn find_bridge_config(env: &Env) -> Option<String> {
    let value = read_json(&join(&remote_scripts_dir(env), "AbletonMcpBridge/bridge-reference.json"))?;
    let config = value["config"].as_str()?;
    (is_absolute(config) && Path::new(config).is_file()).then(|| config.into())
}
pub fn load_config(args: &[String], env: &Env) -> Result<AppConfig, RuntimeError> {
    let at = |i: usize| args.get(i).map(String::as_str);
    let first = at(0);
    if args.len() == 1 {
        match first {
            Some("--help") => return Ok(AppConfig::Help),
            Some("--version" | "-v") => return Ok(AppConfig::Version),
            Some("auth") => return Ok(AppConfig::Auth { auth_file: load_auth_file(env)?, settings_file: load_settings_file(env)? }),
            Some("doctor") => return Ok(AppConfig::Doctor),
            Some("report") => return Ok(AppConfig::Report),
            _ => {}
        }
    }
    if first == Some("library") {
        if args.len() > 2 || at(1).is_some_and(|s| s != "--rebuild") {
            return Err(RuntimeError::plain("Use: library [--rebuild]."));
        }
        return Ok(AppConfig::Library { rebuild: at(1) == Some("--rebuild") });
    }
    if first == Some("update") {
        if args.len() > 2 || at(1).is_some_and(|s| s != "--rollback" && s != "--check") {
            return Err(RuntimeError::plain("Use: update [--check | --rollback]."));
        }
        return Ok(AppConfig::Update { rollback: at(1) == Some("--rollback"), check: at(1) == Some("--check") });
    }
    if first == Some("uninstall") {
        if args[1..].iter().any(|s| s != "--all" && s != "--yes") {
            return Err(RuntimeError::plain("Use: uninstall [--all] [--yes]."));
        }
        return Ok(AppConfig::Uninstall { all: args[1..].iter().any(|s| s == "--all"), yes: args[1..].iter().any(|s| s == "--yes") });
    }
    if first == Some("bridge") {
        if args[1..].iter().any(|s| s != "--yes" && s != "--allow-dirty") {
            return Err(RuntimeError::plain("Use: bridge [--yes] [--allow-dirty]."));
        }
        return Ok(AppConfig::Bridge {
            yes: args[1..].iter().any(|s| s == "--yes"),
            allow_dirty: args[1..].iter().any(|s| s == "--allow-dirty"),
        });
    }
    if first == Some("model") && args.len() <= 2 {
        let file = load_settings_file(env)?;
        if at(1).is_some() && !valid_model(at(1), &read_settings(&file).model_servers) {
            return Err(RuntimeError::plain(format!("Use: model <provider>/<model>, with provider one of {}.", sources())));
        }
        return Ok(AppConfig::Model { settings_file: file, model: at(1).filter(|s| !s.is_empty()).map(str::to_string) });
    }
    let pi = || join(&home(), ".pi/agent/auth.json");
    if args.len() == 1 && first == Some("login") {
        return Ok(AppConfig::LoginChoose { auth_file: load_auth_file(env)?, pi_auth_file: pi(), settings_file: load_settings_file(env)? });
    }
    if first == Some("login") || first == Some("logout") {
        if matches!(at(1), Some("ollama" | "lmstudio")) {
            return Err(RuntimeError::plain(format!(
                "{} needs no sign-in: while it's open, its models are in /model, or choose one with: {} model {}/<model>",
                if at(1) == Some("ollama") { "Ollama" } else { "LM Studio" },
                *KUMI,
                at(1).unwrap()
            )));
        }
        let Some(provider) = at(1).and_then(ProviderId::parse) else {
            return Err(RuntimeError::plain(format!(
                "Use: {} <provider>, with provider one of {}.",
                first.unwrap(),
                PROVIDERS.iter().map(|p| p.as_str()).collect::<Vec<_>>().join(", ")
            )));
        };
        if first == Some("logout") {
            if args.len() != 2 {
                return Err(RuntimeError::plain(format!("Use: logout {provider}.")));
            }
            return Ok(AppConfig::Logout { provider, auth_file: load_auth_file(env)? });
        }
        let chatgpt = provider_info(provider).sign_in == SignIn::Chatgpt;
        let method = if chatgpt && args.len() <= 3 {
            match at(2).unwrap_or("") {
                "" => Some(LoginMethod::Browser),
                "--device" => Some(LoginMethod::Device),
                "--from-pi" => Some(LoginMethod::ImportPi),
                _ => None,
            }
        } else if !chatgpt && args.len() == 2 {
            Some(LoginMethod::Key)
        } else {
            None
        };
        let Some(method) = method else {
            return Err(RuntimeError::plain(if chatgpt {
                "Use: login openai-codex [--device | --from-pi].".into()
            } else {
                format!("Use: login {provider}; Kumi asks for the API key, so it stays out of your shell history.")
            }));
        };
        return Ok(AppConfig::Login {
            provider,
            method,
            auth_file: load_auth_file(env)?,
            pi_auth_file: pi(),
            settings_file: load_settings_file(env)?,
        });
    }
    if args.len() == 1 && first == Some("--inference-only") {
        return Ok(AppConfig::InferenceOnly { inference: load_inference_config(env)?, bridge_missing: None });
    }
    if args.is_empty() {
        return Ok(if let Some(bridge_config) = find_bridge_config(env) {
            AppConfig::Live { inference: load_inference_config(env)?, bridge_config }
        } else {
            AppConfig::InferenceOnly { inference: load_inference_config(env)?, bridge_missing: Some(true) }
        });
    }
    if args.len() != 2 || first != Some("--bridge-config") || at(1).is_none_or(|s| s.is_empty() || s.starts_with('-')) {
        return Err(RuntimeError::plain(format!("Use: {} [--bridge-config /absolute/path.json | --inference-only], or doctor, auth, login, logout, model; --help must be used alone.",*KUMI_START)));
    }
    let bridge = at(1).unwrap();
    if !is_absolute(bridge) {
        return Err(RuntimeError::plain("--bridge-config requires an absolute path."));
    }
    if !Path::new(bridge).is_file() || std::fs::File::open(bridge).is_err() {
        return Err(RuntimeError::plain(
            "Bridge configuration must be an existing readable file; its contents are validated by the MCP server.",
        ));
    }
    Ok(AppConfig::Live { inference: load_inference_config(env)?, bridge_config: bridge.into() })
}
/// Node's stripVTControlCharacters expression, for the same diagnostic and terminal input treatment.
pub fn strip_vt(text: &str) -> String {
    static ANSI: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"[\x1b\u{009b}][\[\]()#;?]*(?:(?:(?:(?:;[-a-zA-Z0-9/#&.:=?%@~_]+)*|[a-zA-Z0-9]+(?:;[-a-zA-Z0-9/#&.:=?%@~_]*)*)?(?:\x07|\x1b\\|\u{009c}))|(?:(?:[0-9]{1,4}(?:;[0-9]{0,4})*)?[0-9A-PR-TZcf-nq-uy=><~]))").unwrap()
    });
    ANSI.replace_all(text, "").into_owned()
}
pub fn safe_error(error: Option<&dyn std::error::Error>, secrets: &[String]) -> String {
    safe_error_message(error.map(|e| e.to_string()).as_deref(), secrets)
}
pub fn safe_error_message(message: Option<&str>, secrets: &[String]) -> String {
    let mut message = message.unwrap_or("Unexpected failure").to_string();
    for secret in secrets.iter().filter(|s| !s.is_empty()) {
        message = message.replace(secret, "[redacted]");
    }
    message = strip_vt(&message).chars().map(|c| if c <= '\u{1f}' || ('\u{7f}'..='\u{9f}').contains(&c) { ' ' } else { c }).collect();
    for secret in secrets.iter().filter(|s| !s.is_empty()) {
        message = message.replace(secret, "[redacted]");
    }
    static CREDENTIAL: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r#"(?i)(\b(?:authorization|x-api-key|api[_-]?key|token)\b["']?\s*[:=]\s*["']?)(?:Bearer\s+)?[^\s"';,}]+"#).unwrap()
    });
    message = CREDENTIAL.replace_all(&message, "${1}[redacted]").into_owned();
    if utf16_len(&message) <= 1024 {
        message
    } else {
        format!("{}...", String::from_utf16_lossy(&message.encode_utf16().take(1021).collect::<Vec<_>>()))
    }
}
