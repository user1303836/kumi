use kumi_ai::providers::{
    local::{local_servers, parse_local_model_id, ServerSetting, LOCAL_PROVIDERS},
    parse_model_id, table, Effort, ProviderId, SignIn,
};
use kumi_common::js::{
    json::file_text,
    string::{trim, utf16_len},
};
use kumi_library::sources::{is_absolute, join};
use kumi_types::daw::DawId;
use kumi_types::{
    command::{KUMI, KUMI_START},
    errors::RuntimeError,
    system::{self, Env},
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
        /// The DAW whose bridge is set up, asked for with --daw or KUMI_DAW, when it isn't Live (never written for Live).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        daw: Option<DawId>,
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
    Doctor {
        json: bool,
        /// The DAW asked for with --daw or KUMI_DAW, whose words the doctor speaks.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        daw: Option<DawId>,
    },
    Report {
        /// As the doctor's.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        daw: Option<DawId>,
    },
    Update {
        rollback: bool,
        check: bool,
        /// `--version x.y.z`: that release, newer or older.
        #[serde(skip_serializing_if = "Option::is_none")]
        version: Option<String>,
    },
    Uninstall {
        all: bool,
        yes: bool,
        dry_run: bool,
    },
    Library {
        rebuild: bool,
    },
    /// `kumi export`: a kept conversation as a file.
    Export {
        /// Its id, the start of one or its name; None for the latest.
        #[serde(skip_serializing_if = "Option::is_none")]
        conversation: Option<String>,
        /// "md", "html" or "json".
        format: String,
        /// Where it goes: a file, or a folder for the usual name; None for the folder Kumi runs in.
        #[serde(skip_serializing_if = "Option::is_none")]
        out: Option<String>,
        full: bool,
        raw: bool,
    },
    /// `kumi import`: a conversation saved with `kumi export --json`.
    Import {
        file: String,
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
        #[serde(default, skip_serializing_if = "Option::is_none")]
        resume: Option<Resume>,
        /// The DAW chosen with --daw or KUMI_DAW, when it isn't Live (never written for Live).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        daw: Option<DawId>,
        /// `-p`: one request, answered, and Kumi ends.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        print: Option<Print>,
    },
    Live {
        #[serde(flatten)]
        inference: InferenceConfig,
        bridge_config: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        resume: Option<Resume>,
        /// The DAW chosen with --daw or KUMI_DAW, when it isn't Live (never written for Live).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        daw: Option<DawId>,
        /// `-p`: one request, answered, and Kumi ends.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        print: Option<Print>,
    },
}
/// `-p <request>` (or `--print`): one request, answered, for a script; with `--json`, every event the session sends, a
/// JSON line each, in place of the answer's words.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Print {
    pub request: String,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub json: bool,
}
/// Which kept conversation Kumi takes up as it starts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Resume {
    /// `-c`: the one used last, in any Set.
    Last,
    /// `-r <words>`: its id, the start of it, or its name.
    Named(String),
    /// `-r` alone: the producer chooses.
    Pick,
}
/// What's wrong with KUMI_DAW, when it names no DAW Kumi knows.
pub(crate) fn daw_env_problem(env: &Env) -> Option<String> {
    env.get("KUMI_DAW").filter(|s| !s.is_empty())?;
    take_daw(&[], env).err().map(|error| error.message())
}

/// The DAWs this Kumi integrates, by the names `--daw` and KUMI_DAW take.
fn registered_daws() -> String {
    crate::daw::registry::registered().filter_map(|profile| profile.id).map(DawId::as_str).collect::<Vec<_>>().join(", ")
}

/// A DAW this Kumi integrates, by its name in any case: "live" is another name for Live's ("ableton", the one written), as
/// /daw takes it.
fn known_daw(text: &str) -> Option<DawId> {
    let text = text.trim().to_ascii_lowercase();
    let daw = if text == "live" { Some(DawId::Ableton) } else { DawId::parse(&text) };
    daw.filter(|daw| crate::daw::registry::profile(*daw).is_some())
}

/// The DAW asked for with `--daw <id>` (or `--daw=<id>`) alone, and the arguments left: what `bridge` and
/// `--bridge-config` take, so KUMI_DAW never sends Live's bridge or Live's configuration to another DAW (`kumi bridge` is
/// Live's in every message Kumi gives for Live, and in an update's last step).
pub(crate) fn take_daw_flag(args: &[String]) -> Result<(Vec<String>, Option<DawId>), RuntimeError> {
    let (mut rest, mut asked) = (vec![], None);
    let mut at = 0;
    while at < args.len() {
        let arg = args[at].as_str();
        let word = if arg == "--daw" {
            at += 1;
            Some(args.get(at).map(String::as_str).unwrap_or(""))
        } else {
            arg.strip_prefix("--daw=")
        };
        match word {
            Some(word) => {
                let daw = known_daw(word)
                    .ok_or_else(|| RuntimeError::plain(format!("Use: --daw <daw>, with daw one of {}.", registered_daws())))?;
                asked = Some(daw);
            }
            None => rest.push(args[at].clone()),
        }
        at += 1;
    }
    Ok((rest, asked))
}

/// The DAW asked for with `--daw <id>` (or `--daw=<id>`), else KUMI_DAW: one this Kumi integrates, None for none asked.
pub(crate) fn take_daw(args: &[String], env: &Env) -> Result<(Vec<String>, Option<DawId>), RuntimeError> {
    let (rest, mut asked) = take_daw_flag(args)?;
    if asked.is_none() {
        if let Some(word) = env.get("KUMI_DAW").filter(|s| !s.is_empty()) {
            asked = Some(known_daw(word).ok_or_else(|| RuntimeError::plain(format!("KUMI_DAW must be one of: {}.", registered_daws())))?);
        }
    }
    // Live asked for is kept: among several bridges it decides; the configuration never writes it (as before).
    Ok((rest, asked))
}
/// `-c`/`--continue` and `-r`/`--resume [id or name]` taken out of a session's arguments: what's left, and which
/// conversation to take up.
/// `-p <request>` (or `--print <request>`, `--print=<request>`) and `--json` beside it, out of the arguments. The word
/// after `-p` is the request whatever it starts with (`-p "-3 dB on the kick"`); `--json` without `-p` stays, for what
/// reads the rest to refuse.
pub(crate) fn take_print(args: &[String]) -> Result<(Vec<String>, Option<Print>), RuntimeError> {
    let usage = || RuntimeError::plain(format!("Use: {} -p \"<request>\" [--json]", *KUMI_START));
    let (mut rest, mut request) = (vec![], None);
    let mut at = 0;
    while at < args.len() {
        let arg = args[at].as_str();
        let words = match arg {
            "-p" | "--print" => {
                at += 1;
                Some(args.get(at).ok_or_else(usage)?.clone())
            }
            _ => arg.strip_prefix("--print=").map(str::to_string),
        };
        match words {
            Some(words) if trim(&words).is_empty() || request.is_some() => return Err(usage()),
            Some(words) => request = Some(words),
            None => rest.push(args[at].clone()),
        }
        at += 1;
    }
    let Some(request) = request else { return Ok((rest, None)) };
    let json = rest.iter().any(|arg| arg == "--json");
    rest.retain(|arg| arg != "--json");
    Ok((rest, Some(Print { request, json })))
}
pub(crate) fn take_resume(args: &[String]) -> Result<(Vec<String>, Option<Resume>), RuntimeError> {
    let both = || {
        RuntimeError::plain(format!(
            "Use {start} -c (the conversation you had last) or {start} -r <id or name>, not both.",
            start = *KUMI_START
        ))
    };
    let (mut rest, mut resume) = (vec![], None);
    let mut at = 0;
    while at < args.len() {
        let arg = args[at].as_str();
        let taken = match arg {
            "-c" | "--continue" => Some(Resume::Last),
            "-r" | "--resume" => {
                // The words after it, up to the next option: an id, or a name with spaces in it.
                let words: Vec<&str> = args[at + 1..].iter().map(String::as_str).take_while(|word| !word.starts_with('-')).collect();
                at += words.len();
                Some(if words.is_empty() { Resume::Pick } else { Resume::Named(words.join(" ")) })
            }
            _ => arg.strip_prefix("--resume=").map(|word| if trim(word).is_empty() { Resume::Pick } else { Resume::Named(word.into()) }),
        };
        match taken {
            Some(_) if resume.is_some() => return Err(both()),
            Some(taken) => resume = Some(taken),
            None => rest.push(args[at].clone()),
        }
        at += 1;
    }
    Ok((rest, resume))
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
/// A release's version as `update --version` and the installer take it: "1.15.0", "v1.15.0", or "latest".
pub fn release_version(text: &str) -> Option<String> {
    let text = text.trim();
    if text == "latest" {
        return Some(text.into());
    }
    let parts: Vec<_> = text
        .strip_prefix('v')
        .unwrap_or(text)
        .split('.')
        .map(|p| p.bytes().all(|b| b.is_ascii_digit()).then(|| p.parse::<u32>().ok()).flatten())
        .collect();
    match parts[..] {
        [Some(major), Some(minor), Some(patch)] => Some(format!("{major}.{minor}.{patch}")),
        _ => None,
    }
}
/// Kumi's own folder (kumi-daw's, which a DAW's steps read too).
pub fn kumi_dir(env: &Env) -> String {
    kumi_daw::io::kumi_home(env)
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
location!(load_keys_file, "KUMI_KEYS_FILE", "keys.json");
location!(load_db_file, "KUMI_DB_FILE", "kumi.db");
location!(load_memory_file, "KUMI_MEMORY_FILE", "memory.json");
location!(load_techniques_file, "KUMI_TECHNIQUES_FILE", "techniques.json");
location!(load_restore_file, "KUMI_RESTORE_FILE", "audition-restore.json");
location!(load_goals_dir, "KUMI_GOALS_DIR", "goals");
location!(load_references_dir, "KUMI_REFERENCES_DIR", "references");
location!(load_playbook_file, "KUMI_PLAYBOOK_FILE", "playbook.json");
location!(load_gaps_file, "KUMI_GAPS_FILE", "gaps.jsonl");
location!(load_timings_file, "KUMI_TIMINGS_FILE", "timings.jsonl");
location!(load_recipes_dir, "KUMI_RECIPES_DIR", "recipes");
location!(load_videos_dir, "KUMI_VIDEOS_DIR", "videos");
location!(load_tools_dir, "KUMI_TOOLS_DIR", "tools");
location!(load_input_history_file, "KUMI_INPUT_HISTORY_FILE", "input-history");
location!(load_projects_dir, "KUMI_PROJECTS_DIR", "projects");
location!(load_library_dir, "KUMI_LIBRARY_DIR", "library");
location!(load_music_dir, "KUMI_MUSIC_DIR", "music");
/// The files Kumi kept notes, techniques, lessons and gaps in before its database, where the producer's
/// settings put them.
pub fn json_files(env: &Env) -> Result<kumi_agent::core::store_import::JsonFiles, RuntimeError> {
    Ok(kumi_agent::core::store_import::JsonFiles {
        memory: load_memory_file(env)?.into(),
        projects: load_projects_dir(env)?.into(),
        techniques: load_techniques_file(env)?.into(),
        playbook: load_playbook_file(env)?.into(),
        gaps: load_gaps_file(env)?.into(),
    })
}
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
    /// false: the map's points don't play as the mouse comes to them (h on the map).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub map_hover: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub update_check: Option<bool>,
    /// false: no "What's new" as Kumi first starts after an update (`/changelog` still shows it).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub whats_new: Option<bool>,
    /// false: fixed phrases ("tempo 124", "mute drums") go to the model like any other message.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub instant_phrases: Option<bool>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub library_folders: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub model_servers: Vec<ServerSetting>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub voice: Option<VoiceSettings>,
    /// "off": no desktop notification (or bell) while Kumi's window isn't the one in front; "auto" otherwise.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub notifications: Option<String>,
    /// false: no line in Live's status bar when Kumi finishes or needs the producer.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub live_status_bar: Option<bool>,
    /// false: the computer may sleep while Kumi works.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub keep_awake: Option<bool>,
    /// Seconds before Kumi takes the answer it recommended when nobody's there to pick; 0 never does.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub answer_after: Option<u32>,
    /// "<provider>/<model>" that carries on when the model's usage limit is reached or its provider is down.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fallback_model: Option<String>,
    /// How many background jobs run at once (1–8).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_jobs: Option<u32>,
    /// The model background workers use, from a provider's own list; none uses the conversation's.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub worker_model: Option<String>,
    /// How long kept conversations stay.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub conversations: Option<ConversationSettings>,
    /// The folders Kumi learns the producer's taste from (their own music): none until they agree, and empty once they
    /// took every one away.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub music_folders: Option<Vec<String>>,
    /// Kumi has offered to learn their music (once).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub music_offered: Option<bool>,
    /// false: Kumi doesn't fetch models.dev's catalog of models; the copy it has stays in use.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model_catalog: Option<bool>,
    /// "off": Kumi leaves the mouse to the terminal, whose own selection and scrolling then work; "on" otherwise.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mouse: Option<String>,
    /// false: text dragged across in Kumi's window is copied by Ctrl-C or right-click, not as the button comes up.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub copy_on_select: Option<bool>,
    /// false: quitting doesn't print the conversation into the terminal's scrollback.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub transcript_on_quit: Option<bool>,
    /// "dark" or "light"; the terminal's own background chooses ("system") otherwise.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub theme: Option<String>,
    /// "always" or "off": when the header shows how full the model's context is; "auto" shows it past half.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context_meter: Option<String>,
    /// Whether every frame is drawn whole; the terminal decides when none is set (Windows Terminal draws whole).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub full_repaint: Option<bool>,
}
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConversationSettings {
    /// Days a conversation is kept after it was last used, beyond its Set's 20 newest (90); 0 keeps every one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub keep_days: Option<u32>,
}
/// The longest keeping a setting asks for: a hundred years.
const MAX_KEEP_DAYS: u64 = 36_500;
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
            // How it takes a request: Chat Completions unless it says another Kumi writes.
            let format = entry.get("format").and_then(|format| serde_json::from_value(format.clone()).ok());
            Some(ServerSetting { name: name.into(), base_url: url.to_string(), api_key, format })
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
/// A list of folders as settings keep it: words of a sensible length, at most 64.
fn folder_list(value: &Value) -> Vec<String> {
    value
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .filter(|s| !s.is_empty() && utf16_len(s) <= 1024)
        .take(64)
        .map(str::to_string)
        .collect()
}
fn read_json(file: &str) -> Option<Value> {
    serde_json::from_str(&String::from_utf8_lossy(&std::fs::read(file).ok()?)).ok()
}
/// keys.json, the producer's own keys for Kumi's actions: none when there's no file, or why it isn't JSON.
pub fn read_keys(file: &str) -> Option<Result<Value, String>> {
    let text = String::from_utf8_lossy(&std::fs::read(file).ok()?).into_owned();
    Some(serde_json::from_str(&text).map_err(|error| format!("keys.json isn't JSON Kumi can read ({error}), so Kumi keeps its own keys")))
}
pub fn read_settings(file: &str) -> Settings {
    let Some(value) = read_json(file) else { return Settings::default() };
    // The producer's answer about their music: settings.json's, or the copy beside it when an older Kumi rewrote
    // settings.json without the keys it doesn't know.
    let kept_beside;
    let music = if value.get("musicFolders").is_none() && value.get("musicOffered").is_none() {
        kept_beside = read_json(&music_answer_file(file)).unwrap_or_default();
        &kept_beside
    } else {
        &value
    };
    let model_servers = server_settings(&value["modelServers"]);
    let model = value["model"].as_str().filter(|m| valid_model(Some(m), &model_servers)).map(str::to_string);
    let effort = value["effort"].as_str().and_then(Effort::parse);
    static TAB: LazyLock<Regex> = LazyLock::new(|| Regex::new("^[a-z][a-z0-9-]{0,31}$").unwrap());
    Settings {
        model,
        effort,
        fast: (value["fast"] == true).then_some(true),
        panel_tab: value["panelTab"].as_str().filter(|s| TAB.is_match(s)).map(str::to_string),
        map_hover: (value["mapHover"] == false).then_some(false),
        update_check: (value["updateCheck"] == false).then_some(false),
        whats_new: (value["whatsNew"] == false).then_some(false),
        instant_phrases: (value["instantPhrases"] == false).then_some(false),
        library_folders: folder_list(&value["libraryFolders"]),
        voice: voice_settings(&value["voice"]),
        notifications: (value["notifications"] == "off").then(|| "off".to_string()),
        live_status_bar: (value["liveStatusBar"] == false).then_some(false),
        keep_awake: (value["keepAwake"] == false).then_some(false),
        answer_after: value["answerAfter"].as_u64().filter(|seconds| *seconds <= MAX_ANSWER_AFTER).map(|seconds| seconds as u32),
        fallback_model: value["fallbackModel"].as_str().filter(|m| valid_model(Some(m), &model_servers)).map(str::to_string),
        max_jobs: value["maxJobs"].as_u64().filter(|n| (1..=8).contains(n)).map(|n| n as u32),
        worker_model: value["workerModel"].as_str().filter(|m| valid_model(Some(m), &model_servers)).map(str::to_string),
        conversations: value["conversations"]["keepDays"]
            .as_u64()
            .filter(|days| *days <= MAX_KEEP_DAYS)
            .map(|days| ConversationSettings { keep_days: Some(days as u32) }),
        music_folders: music["musicFolders"].is_array().then(|| folder_list(&music["musicFolders"])),
        music_offered: (music["musicOffered"] == true).then_some(true),
        model_catalog: (value["modelCatalog"] == false).then_some(false),
        model_servers,
        mouse: (value["mouse"] == "off").then(|| "off".to_string()),
        copy_on_select: (value["copyOnSelect"] == false).then_some(false),
        transcript_on_quit: (value["transcriptOnQuit"] == false).then_some(false),
        theme: value["theme"].as_str().filter(|theme| ["dark", "light"].contains(theme)).map(str::to_string),
        context_meter: value["contextMeter"].as_str().filter(|meter| ["always", "off"].contains(meter)).map(str::to_string),
        full_repaint: value["fullRepaint"].as_bool(),
    }
}
/// The settings this Kumi reads: any other key in the file is a newer Kumi's, kept as it is when this one writes.
const KNOWN_SETTINGS: &[&str] = &[
    "model",
    "effort",
    "fast",
    "panelTab",
    "mapHover",
    "updateCheck",
    "whatsNew",
    "instantPhrases",
    "libraryFolders",
    "modelServers",
    "voice",
    "notifications",
    "liveStatusBar",
    "keepAwake",
    "answerAfter",
    "fallbackModel",
    "maxJobs",
    "workerModel",
    "conversations",
    "musicFolders",
    "musicOffered",
    "modelCatalog",
    "mouse",
    "copyOnSelect",
    "transcriptOnQuit",
    "theme",
    "contextMeter",
    "fullRepaint",
];
/// The longest wait before Kumi answers for the producer: an hour.
const MAX_ANSWER_AFTER: u64 = 3600;
/// Keeps the producer's answer about their music (`musicFolders`, `musicOffered`) and changes nothing else in the file,
/// so a newer Kumi's settings and the ones this Kumi can't read stay as they are. The folders are applied to what the
/// file holds now: what this Kumi added or took away since `before` (the folders it last kept, or read), so another
/// Kumi's folders stay too. None forgets them all.
pub fn write_music_settings(file: &str, folders: Option<&[String]>, before: Option<&[String]>, offered: bool) -> Result<(), RuntimeError> {
    let mut next = match read_json(file) {
        Some(Value::Object(raw)) => raw,
        _ => Map::new(),
    };
    let now = read_settings(file);
    let folders = folders.map(|folders| {
        let before = before.unwrap_or_default();
        let mut kept = now.music_folders.unwrap_or_default();
        kept.retain(|folder| folders.contains(folder) || !before.contains(folder));
        for folder in folders.iter().filter(|folder| !before.contains(folder)) {
            if !kept.contains(folder) {
                kept.push(folder.clone());
            }
        }
        kept
    });
    let offered = offered || now.music_offered == Some(true);
    next.insert("musicFolders".into(), json!(folders));
    if offered {
        next.insert("musicOffered".into(), json!(true));
    }
    write_settings(file, &Value::Object(next))?;
    write_private(&music_answer_file(file), &file_text(&json!({"musicFolders": folders, "musicOffered": offered})))
}
/// Whether the settings say Kumi offered to learn the producer's music (another Kumi may have since this one read them).
pub fn music_offered(file: &str) -> bool {
    read_settings(file).music_offered == Some(true)
}
/// The producer's answer about their music is kept beside settings.json too (`write_music_settings`), as whats-new.json
/// is: an older Kumi writing its settings drops the keys it doesn't know, and this copy is read when they're gone.
pub const MUSIC_ANSWER_FILE: &str = "music-answer.json";
fn music_answer_file(settings: &str) -> String {
    Path::new(settings).with_file_name(MUSIC_ANSWER_FILE).to_string_lossy().into_owned()
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
    if kept("mapHover") == Some(&Value::Bool(false)) {
        settings.insert("mapHover".into(), json!(false));
    }
    if kept("updateCheck") == Some(&Value::Bool(false)) {
        settings.insert("updateCheck".into(), json!(false));
    }
    if kept("whatsNew") == Some(&Value::Bool(false)) {
        settings.insert("whatsNew".into(), json!(false));
    }
    if kept("instantPhrases") == Some(&Value::Bool(false)) {
        settings.insert("instantPhrases".into(), json!(false));
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
    if kept("notifications").and_then(Value::as_str) == Some("off") {
        settings.insert("notifications".into(), json!("off"));
    }
    for key in ["liveStatusBar", "keepAwake"] {
        if kept(key) == Some(&Value::Bool(false)) {
            settings.insert(key.into(), json!(false));
        }
    }
    if let Some(value) = kept("answerAfter").filter(|v| v.as_u64().is_some_and(|seconds| seconds <= MAX_ANSWER_AFTER)) {
        settings.insert("answerAfter".into(), value.clone());
    }
    if let Some(value) = kept("fallbackModel").filter(|v| truthy(v)) {
        settings.insert("fallbackModel".into(), value.clone());
    }
    if let Some(value) = kept("maxJobs").filter(|v| v.as_u64().is_some_and(|n| (1..=8).contains(&n))) {
        settings.insert("maxJobs".into(), value.clone());
    }
    if let Some(value) = kept("workerModel").filter(|v| v.as_str().is_some_and(|m| !m.is_empty())) {
        settings.insert("workerModel".into(), value.clone());
    }
    if let Some(days) = kept("conversations").and_then(|v| v["keepDays"].as_u64()).filter(|days| *days <= MAX_KEEP_DAYS) {
        settings.insert("conversations".into(), json!({ "keepDays": days }));
    }
    if let Some(value) = kept("musicFolders").filter(|v| v.is_array()) {
        settings.insert("musicFolders".into(), json!(folder_list(value)));
    }
    if kept("musicOffered") == Some(&Value::Bool(true)) {
        settings.insert("musicOffered".into(), json!(true));
    }
    if kept("modelCatalog") == Some(&Value::Bool(false)) {
        settings.insert("modelCatalog".into(), json!(false));
    }
    // The terminal's settings: as `next` sets them, else as the file has them, even a value this Kumi can't read (a
    // newer Kumi's, back after a rollback); none is written as the one Kumi takes when it's left out. fullRepaint has
    // no such value: left out, the terminal decides, so either one stays.
    for (key, default) in [
        ("mouse", Some(json!("on"))),
        ("copyOnSelect", Some(json!(true))),
        ("transcriptOnQuit", Some(json!(true))),
        ("theme", Some(json!("system"))),
        ("contextMeter", Some(json!("auto"))),
        ("fullRepaint", None),
    ] {
        if let Some(value) = next.get(key).or_else(|| raw.as_ref().and_then(|raw| raw.get(key))) {
            if !value.is_null() && Some(value) != default.as_ref() {
                settings.insert(key.into(), value.clone());
            }
        }
    }
    // A newer Kumi's settings stay, so going back to this one and forward again loses none of them.
    if let Some(Value::Object(raw)) = &raw {
        for (key, value) in raw {
            if !KNOWN_SETTINGS.contains(&key.as_str()) && !settings.contains_key(key) {
                settings.insert(key.clone(), value.clone());
            }
        }
    }
    write_private(file, &file_text(&Value::Object(settings)))
}
/// A file only the producer can read (its folder too), written whole or not at all.
fn write_private(file: &str, text: &str) -> Result<(), RuntimeError> {
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
        .and_then(|mut f| f.write_all(text.as_bytes()))
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
    format!("{} (or a server in settings.json)", table::rows().map(|row| row.id).chain(LOCAL_PROVIDERS).collect::<Vec<_>>().join(", "))
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
/// How ChatGPT signs in when nothing says: in this computer's browser, or over SSH, where the browser isn't the
/// producer's, with a code entered in a browser anywhere.
pub fn chatgpt_sign_in(env: &Env) -> LoginMethod {
    if env.get("SSH_CONNECTION").is_some_and(|value| !value.is_empty()) {
        LoginMethod::Device
    } else {
        LoginMethod::Browser
    }
}
/// Kumi's commands, for an unknown one that's probably one of them misspelled.
const COMMANDS: [&str; 14] = [
    "auth",
    "bridge",
    "doctor",
    "export",
    "help",
    "import",
    "library",
    "login",
    "logout",
    "model",
    "report",
    "uninstall",
    "update",
    "version",
];
/// `kumi export [id or name | --last] [--md | --html | --json] [-o <path>] [--full] [--raw]`: which conversation (the
/// latest without one), its format (Markdown without one), where it goes and what's in it.
fn export_config(args: &[String]) -> Result<AppConfig, RuntimeError> {
    let usage = || RuntimeError::plain("Use: export [id or name | --last] [--md | --html | --json] [-o <path>] [--full] [--raw].");
    let (mut words, mut format, mut out, mut full, mut raw, mut last) = (vec![], None, None, false, false, false);
    let mut at = 0;
    while at < args.len() {
        let chosen = match args[at].as_str() {
            "--md" | "--markdown" => Some("md"),
            "--html" => Some("html"),
            "--json" => Some("json"),
            _ => None,
        };
        match args[at].as_str() {
            _ if chosen.is_some() => {
                if format.is_some_and(|format| Some(format) != chosen) {
                    return Err(RuntimeError::plain("Choose one of --md, --html and --json."));
                }
                format = chosen;
            }
            "--full" => full = true,
            "--raw" => raw = true,
            "--last" => last = true,
            "-o" | "--output" => {
                at += 1;
                let path = args.get(at).filter(|path| !path.is_empty() && !controls(path, true)).ok_or_else(usage)?;
                out = Some(path.clone());
            }
            word if word.starts_with('-') => return Err(usage()),
            word => words.push(word.to_string()),
        }
        at += 1;
    }
    if last && !words.is_empty() {
        return Err(usage());
    }
    Ok(AppConfig::Export {
        conversation: (!words.is_empty()).then(|| words.join(" ")),
        format: format.unwrap_or("md").into(),
        out,
        full,
        raw,
    })
}
/// What Kumi says about words it doesn't know, leading to its help.
fn unknown(word: Option<&str>) -> RuntimeError {
    let help = format!("{} --help lists the commands.", *KUMI_START);
    // Only a short word is said back: a key pasted as an argument stays out of the message.
    let Some(word) = word.filter(|w| (2..=24).contains(&w.len()) && w.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')) else {
        return RuntimeError::plain(format!("That isn't a command; {help}"));
    };
    if word.starts_with('-') {
        return RuntimeError::plain(format!("No option called {word}; {help}"));
    }
    if COMMANDS.contains(&word) {
        return RuntimeError::plain(format!("{} {word} doesn't take that; {help}", *KUMI_START));
    }
    // A short word is near only one letter off: "path" isn't "auth".
    let most = if word.len() <= 4 { 1 } else { 2 };
    let near = COMMANDS.iter().find(|command| edits(command, &word.to_ascii_lowercase()) <= most);
    RuntimeError::plain(match near {
        Some(near) => format!("No command called {word} (did you mean {near}?); {help}"),
        None => format!("No command called {word}; {help}"),
    })
}
/// How many letters to add, drop or change to turn `a` into `b`.
fn edits(a: &str, b: &str) -> usize {
    let b: Vec<char> = b.chars().collect();
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for (i, x) in a.chars().enumerate() {
        let mut diagonal = row[0];
        row[0] = i + 1;
        for (j, y) in b.iter().enumerate() {
            let next = (diagonal + usize::from(x != *y)).min(row[j] + 1).min(row[j + 1] + 1);
            diagonal = row[j + 1];
            row[j + 1] = next;
        }
    }
    row[b.len()]
}
pub fn load_config(args: &[String], env: &Env) -> Result<AppConfig, RuntimeError> {
    let at = |i: usize| args.get(i).map(String::as_str);
    let first = at(0);
    if matches!(first, Some("help" | "--help" | "-h")) || args.iter().any(|arg| arg == "--help" || arg == "-h") {
        return Ok(AppConfig::Help);
    }
    if args.len() == 1 {
        match first {
            Some("--version" | "-v" | "-V" | "version") => return Ok(AppConfig::Version),
            Some("auth") => return Ok(AppConfig::Auth { auth_file: load_auth_file(env)?, settings_file: load_settings_file(env)? }),
            _ => {}
        }
    }
    // The doctor and the report speak of the DAW asked for (`--daw reaper doctor`, KUMI_DAW), else of the one a session
    // would work with.
    if args.iter().any(|arg| arg == "doctor" || arg == "report") {
        // A KUMI_DAW Kumi doesn't know never stops them: the doctor says what's wrong with it.
        let mut known = env.clone();
        if daw_env_problem(env).is_some() {
            known.remove("KUMI_DAW");
        }
        let (rest, daw) = take_daw(args, &known)?;
        match rest.iter().map(String::as_str).collect::<Vec<_>>().as_slice() {
            ["doctor"] => return Ok(AppConfig::Doctor { json: false, daw }),
            ["doctor", "--json"] => return Ok(AppConfig::Doctor { json: true, daw }),
            ["report"] => return Ok(AppConfig::Report { daw }),
            _ => {}
        }
    }
    if first == Some("library") {
        if args.len() > 2 || at(1).is_some_and(|s| s != "--rebuild") {
            return Err(RuntimeError::plain("Use: library [--rebuild]."));
        }
        return Ok(AppConfig::Library { rebuild: at(1) == Some("--rebuild") });
    }
    if first == Some("export") {
        return export_config(&args[1..]);
    }
    if first == Some("import") {
        return match &args[1..] {
            [file] if !file.is_empty() && !file.starts_with('-') && !controls(file, true) => Ok(AppConfig::Import { file: file.clone() }),
            _ => Err(RuntimeError::plain("Use: import <file> (a conversation saved with: export --json).")),
        };
    }
    if first == Some("update") {
        if at(1) == Some("--version") {
            let Some(version) = at(2).filter(|_| args.len() == 3).and_then(release_version) else {
                return Err(RuntimeError::plain("Use: update --version <x.y.z>, such as update --version 1.15.0."));
            };
            return Ok(AppConfig::Update { rollback: false, check: false, version: (version != "latest").then_some(version) });
        }
        if args.len() > 2 || at(1).is_some_and(|s| s != "--rollback" && s != "--check") {
            return Err(RuntimeError::plain("Use: update [--check | --rollback | --version <x.y.z>]."));
        }
        return Ok(AppConfig::Update { rollback: at(1) == Some("--rollback"), check: at(1) == Some("--check"), version: None });
    }
    if first == Some("uninstall") {
        if args[1..].iter().any(|s| s != "--all" && s != "--yes" && s != "--dry-run") {
            return Err(RuntimeError::plain("Use: uninstall [--all] [--yes] [--dry-run]."));
        }
        let has = |flag: &str| args[1..].iter().any(|s| s == flag);
        return Ok(AppConfig::Uninstall { all: has("--all"), yes: has("--yes"), dry_run: has("--dry-run") });
    }
    // `bridge` sets up Live's bridge, or the bridge of the DAW asked for with --daw (`--daw reaper bridge`); KUMI_DAW never
    // moves it, so Live's messages and an update's last step mean Live's.
    let (bridge_args, bridge_daw) = if args.iter().any(|arg| arg == "bridge") { take_daw_flag(args)? } else { (vec![], None) };
    if bridge_args.first().map(String::as_str) == Some("bridge") {
        let args = &bridge_args;
        if args[1..].iter().any(|s| s != "--yes" && s != "--allow-dirty") {
            return Err(RuntimeError::plain("Use: bridge [--yes] [--allow-dirty]."));
        }
        return Ok(AppConfig::Bridge {
            yes: args[1..].iter().any(|s| s == "--yes"),
            allow_dirty: args[1..].iter().any(|s| s == "--allow-dirty"),
            daw: bridge_daw.filter(|daw| *daw != DawId::Ableton),
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
                table::rows().map(|row| row.id).collect::<Vec<_>>().join(", ")
            )));
        };
        if first == Some("logout") {
            if args.len() != 2 {
                return Err(RuntimeError::plain(format!("Use: logout {provider}.")));
            }
            return Ok(AppConfig::Logout { provider, auth_file: load_auth_file(env)? });
        }
        let chatgpt = provider.row().sign_in == SignIn::Chatgpt;
        let method = if chatgpt && args.len() <= 3 {
            match at(2).unwrap_or("") {
                "" => Some(chatgpt_sign_in(env)),
                "--browser" => Some(LoginMethod::Browser),
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
                "Use: login openai-codex [--browser | --device | --from-pi].".into()
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
    let (args, print) = take_print(args)?;
    let (args, resume) = take_resume(&args)?;
    // Choosing a conversation needs someone there to choose.
    if print.is_some() && resume == Some(Resume::Pick) {
        return Err(RuntimeError::plain(format!("With -p, say which conversation: {} -p \"<request>\" -r <id or name>.", *KUMI_START)));
    }
    let (args, flagged) = take_daw_flag(&args)?;
    let daw = match flagged {
        Some(daw) => Some(daw),
        None => take_daw(&[], env)?.1,
    };
    let at = |i: usize| args.get(i).map(String::as_str);
    let first = at(0);
    if args.len() == 1 && first == Some("--inference-only") {
        return Ok(AppConfig::InferenceOnly {
            inference: load_inference_config(env)?,
            bridge_missing: None,
            resume,
            daw: daw.filter(|daw| *daw != DawId::Ableton),
            print,
        });
    }
    if args.is_empty() {
        // The DAW asked for, else the first whose bridge is installed; without one, chat without a DAW.
        let (provider, found) = crate::daw::chosen(daw, env);
        let daw = Some(provider.id()).filter(|daw| *daw != DawId::Ableton);
        return Ok(if let Some(bridge_config) = found {
            AppConfig::Live { inference: load_inference_config(env)?, bridge_config, resume, daw, print }
        } else {
            AppConfig::InferenceOnly { inference: load_inference_config(env)?, bridge_missing: Some(true), resume, daw, print }
        });
    }
    if !matches!(first, Some("--bridge-config" | "--inference-only")) {
        return Err(unknown(first));
    }
    if args.len() != 2 || first != Some("--bridge-config") || at(1).is_none_or(|s| s.is_empty() || s.starts_with('-')) {
        return Err(RuntimeError::plain(format!(
            "Use: {start} [--bridge-config /absolute/path.json | --inference-only]; {start} --help lists the commands.",
            start = *KUMI_START
        )));
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
    // A bridge configuration given is Live's unless --daw says whose: KUMI_DAW doesn't move it to another DAW.
    let daw = flagged;
    Ok(AppConfig::Live { inference: load_inference_config(env)?, bridge_config: bridge.into(), resume, daw, print })
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
