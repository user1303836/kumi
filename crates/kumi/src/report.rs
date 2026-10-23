//! `kumi report`, with native runtime version information.
use crate::{
    config::{load_auth_file, load_db_file, load_gaps_file, load_projects_dir, load_settings_file, load_timings_file, read_settings},
    doctor::{doctor_sections, format_doctor, format_sections, DoctorIo},
    spinner::step,
    tui::style::os_release,
};
use chrono::{DateTime, Utc};
use kumi_agent::core::gaps::logged_gaps;
use kumi_ai::auth::store::{open_credential_store, CredentialStore};
use kumi_common::js::{
    json::stringify,
    number,
    string::{trim, trim_end, utf16_len},
};
use kumi_daw::check::Check;
use kumi_library::sources::join;
use kumi_types::place::IMPORTED;
use kumi_types::{
    errors::RuntimeError,
    system::{self, Env},
    version::KUMI_VERSION,
};
use serde_json::Value;
use std::{path::Path, rc::Rc, time::UNIX_EPOCH};
pub struct ReportIo {
    pub doctor: DoctorIo,
    pub folder: Option<String>,
    pub home: Option<String>,
    pub user: Option<String>,
    pub now: Option<Rc<dyn Fn() -> DateTime<Utc>>>,
    /// What each DAW's log section reads beyond the environment (`kumi_daw::io::Answers`): a test's stand-in for the
    /// DAW's log files.
    pub answers: kumi_daw::io::Answers,
}
impl ReportIo {
    pub fn new(doctor: DoctorIo) -> Self {
        Self { doctor, folder: None, home: None, user: None, now: None, answers: kumi_daw::io::Answers::default() }
    }
}
/// Secrets are removed first: known ones (sign-ins, keys in the environment), then by their shapes (Bearer tokens, provider
/// keys, JWTs, a URL's password and query secrets, fields named for a secret with their values quoted or bare, long
/// tokens). Then the home folder, and the account name (also where a URL encodes it after %2F or %5C).
pub fn redactor(secrets: &[String], home: &str, user: &str) -> impl Fn(&str) -> String {
    let mut known: Vec<_> = secrets.iter().filter(|s| utf16_len(s) >= 8).cloned().collect();
    known.sort_by_key(|s| std::cmp::Reverse(utf16_len(s)));
    known.dedup();
    let mut homes = vec![];
    for path in [home.to_string(), home.replace('\\', "/"), home.replace('/', "\\")] {
        if utf16_len(&path) > 1 && !homes.contains(&path) {
            homes.push(path)
        }
    }
    let name = (utf16_len(user) >= 3).then(|| {
        fancy_regex::Regex::new(&format!(r"(?i)(?:(?<=%2F)|(?<=%5C)|(?<![A-Za-z0-9])){}(?![A-Za-z0-9])", regex::escape(user))).unwrap()
    });
    let whitespace = r"\x09-\x0d \u{00a0}\u{1680}\u{2000}-\u{200a}\u{2028}\u{2029}\u{202f}\u{205f}\u{3000}\u{feff}";
    let bearer = regex::Regex::new(&format!(r#"(?i)(?-u:\b)Bearer[{whitespace}]+[^{whitespace}"']+"#)).unwrap();
    // Provider keys by their shapes: sk-, pk- and rk- keys, Stripe's, Google's, GitHub's, AWS key ids and Slack's.
    let key = regex::Regex::new(concat!(
        r"(?-u:\b)(?:(?:sk|pk|rk)-[A-Za-z0-9_-]{12,}|[sr]k_(?:live|test)_[A-Za-z0-9]{10,}|AIza[0-9A-Za-z_-]{30,}",
        r"|gh[pousr]_[A-Za-z0-9]{20,}|github_pat_[A-Za-z0-9_]{20,}|(?:AKIA|ASIA)[0-9A-Z]{16}|xox[abposr]-[A-Za-z0-9-]{10,})"
    ))
    .unwrap();
    let jwt = regex::Regex::new(r"(?-u:\b)eyJ[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{8,}").unwrap();
    // A URL's password (scheme://user:password@host), and the secrets its query carries.
    let login = regex::Regex::new(r#"(?i)((?-u:\b)[a-z][a-z0-9+.-]*://[^\s/:@'"]+:)[^\s/@'"]+@"#).unwrap();
    let query = regex::Regex::new(
        r#"(?i)([?&;](?:access_token|refresh_token|id_token|token|api_?key|key|secret|client_secret|password|passwd|pwd|auth|sig|signature|code)=)[^&#\s'"]+"#,
    )
    .unwrap();
    // A field named for a secret (api_key, AWS_SECRET_ACCESS_KEY, db_password, x-auth-token…): a value in double quotes,
    // escaped ones too (JSON inside a string), or for the plainest names in single quotes or bare.
    let named = r"[A-Za-z0-9_-]*(?:api[_-]?key|apikey|token|secret|password|passwd|pwd|pw|passphrase|private[_-]?key)[A-Za-z0-9_-]*";
    let quoted = regex::Regex::new(&format!(
        r#"(?i)((?:\\?")?(?:{named}|access|refresh)(?:\\?")?[{whitespace}]*[:=][{whitespace}]*)(\\?")((?:[^"\\]|\\.)*?)(\\?")"#
    ))
    .unwrap();
    let bare =
        regex::Regex::new(&format!(r#"(?i)((?-u:\b){named}[{whitespace}]*[:=][{whitespace}]*)('[^'\n]*'|[^{whitespace}"'&,;)\]}}<>]+)"#))
            .unwrap();
    let long = fancy_regex::Regex::new(r"(?<![A-Za-z0-9+_-])[A-Za-z0-9+_-]{32,}={0,2}(?![A-Za-z0-9+_-])").unwrap();
    move |text| {
        let mut out = text.to_string();
        for secret in &known {
            out = out.replace(secret, "[secret]")
        }
        for path in &homes {
            out = out.replace(path, "~")
        }
        out = bearer.replace_all(&out, "Bearer [secret]").into_owned();
        out = key.replace_all(&out, "[secret]").into_owned();
        out = jwt.replace_all(&out, "[secret]").into_owned();
        out = login.replace_all(&out, "${1}[secret]@").into_owned();
        out = query.replace_all(&out, "${1}[secret]").into_owned();
        out = quoted
            .replace_all(&out, |caps: &regex::Captures| {
                if utf16_len(&caps[3]) >= 8 {
                    format!("{}{}[secret]{}", &caps[1], &caps[2], &caps[4])
                } else {
                    caps[0].to_string()
                }
            })
            .into_owned();
        out = bare
            .replace_all(&out, |caps: &regex::Captures| {
                let value = caps[2].trim_matches('\'');
                if value == "[secret]" || utf16_len(value) < 8 {
                    caps[0].to_string()
                } else if caps[2].starts_with('\'') {
                    format!("{}'[secret]'", &caps[1])
                } else {
                    format!("{}[secret]", &caps[1])
                }
            })
            .into_owned();
        // A long token: 32 characters with letters and digits in them (as titles hide them), or any 48.
        out = long
            .replace_all(&out, |caps: &fancy_regex::Captures| {
                let run = &caps[0];
                let mixed = run.chars().any(|c| c.is_ascii_digit()) && run.chars().any(|c| c.is_ascii_alphabetic());
                if mixed || run.len() >= 48 {
                    "[long value]".to_string()
                } else {
                    run.to_string()
                }
            })
            .into_owned();
        if let Some(name) = &name {
            out = name.replace_all(&out, "<user>").into_owned()
        }
        out
    }
}
fn strings(value: &Value, out: &mut Vec<String>) {
    match value {
        Value::String(s) => out.push(s.clone()),
        Value::Array(a) => {
            for v in a {
                strings(v, out)
            }
        }
        Value::Object(o) => {
            for v in o.values() {
                strings(v, out)
            }
        }
        _ => {}
    }
}
fn head(text: &str, most: usize) -> String {
    String::from_utf16_lossy(&text.encode_utf16().take(most).collect::<Vec<_>>())
}
pub(crate) fn clip(text: &str, most: usize) -> String {
    let len = utf16_len(text);
    if len > most {
        format!("{}… ({} more characters)", head(text, most), len - most)
    } else {
        text.into()
    }
}
fn string(value: Option<&Value>) -> String {
    match value {
        None => "undefined".into(),
        Some(Value::Null) => "null".into(),
        Some(Value::String(s)) => s.clone(),
        Some(Value::Bool(b)) => b.to_string(),
        Some(Value::Number(n)) => number::to_string(n.as_f64().unwrap_or(f64::NAN)),
        Some(Value::Array(a)) => a.iter().map(|v| if v.is_null() { "".into() } else { string(Some(v)) }).collect::<Vec<_>>().join(","),
        Some(Value::Object(_)) => "[object Object]".into(),
    }
}
pub(crate) async fn read(file: &str) -> Option<String> {
    tokio::fs::read(file).await.ok().map(|b| String::from_utf8_lossy(&b).into_owned())
}
pub(crate) async fn names(folder: &str) -> Vec<String> {
    let Ok(mut reader) = tokio::fs::read_dir(folder).await else { return vec![] };
    let mut result = vec![];
    while let Ok(Some(entry)) = reader.next_entry().await {
        result.push(entry.file_name().to_string_lossy().into_owned())
    }
    result.sort();
    result
}
pub(crate) async fn modified(file: &str) -> Option<f64> {
    let time = tokio::fs::metadata(file).await.ok()?.modified().ok()?;
    match time.duration_since(UNIX_EPOCH) {
        Ok(time) => Some(time.as_secs_f64() * 1000.),
        Err(error) => Some(-error.duration().as_secs_f64() * 1000.),
    }
}
/// The newest kept conversation in any Set's folder: when it was saved, its id and the conversation. Only `<id>.json`
/// files are conversations (a name or pin beside one isn't). One imported from a file, or a copy of one, holds someone
/// else's words, so it's never the one.
async fn last_conversation(projects: &str) -> Option<(f64, String, Value)> {
    let mut best: Option<(String, String, f64)> = None;
    for place in names(projects).await.into_iter().filter(|place| place != IMPORTED) {
        let folder = join(projects, &format!("{place}/conversations"));
        for name in names(&folder).await {
            let Some(id) = name
                .strip_suffix(".json")
                .filter(|id| (6..=24).contains(&id.len()) && id.bytes().all(|b| b.is_ascii_digit() || b.is_ascii_lowercase()))
            else {
                continue;
            };
            let file = join(&folder, &name);
            if let Some(at) = modified(&file).await {
                if best.as_ref().is_none_or(|best| at > best.2) && !imported(&join(&folder, &format!("{id}.meta"))).await {
                    best = Some((file, id.to_string(), at))
                }
            }
        }
    }
    let (file, id, at) = best?;
    Some((at, id, serde_json::from_str(&read(&file).await?).ok()?))
}
/// Whether a conversation's sidecar says it came from a file (or was copied from one that did).
async fn imported(meta: &str) -> bool {
    let Some(text) = read(meta).await.filter(|text| text.len() <= 16 * 1024) else { return false };
    serde_json::from_str::<Value>(&text).ok().and_then(|meta| meta.get("imported").and_then(Value::as_bool)).unwrap_or(false)
}
fn describe_conversation(value: &Value) -> Vec<String> {
    let mut lines = vec![];
    if let Some(messages) = value.get("checkpoint").and_then(|v| v.get("messages")).and_then(Value::as_array) {
        for message in messages.iter().skip(messages.len().saturating_sub(200)) {
            let text;
            let parts = if let Some(content) = message.get("content").and_then(Value::as_str) {
                text = vec![serde_json::json!({"type":"text","text":content})];
                text.as_slice()
            } else {
                message.get("content").and_then(Value::as_array).map(Vec::as_slice).unwrap_or(&[])
            };
            for part in parts {
                match part.get("type").and_then(Value::as_str) {
                    Some("text") => {
                        if let Some(words) = part.get("text").and_then(Value::as_str).map(trim).filter(|s| !s.is_empty()) {
                            let words = words
                                .split(|c: char| trim(&c.to_string()).is_empty())
                                .filter(|s| !s.is_empty())
                                .collect::<Vec<_>>()
                                .join(" ");
                            lines.push(format!(
                                "{}: {}",
                                if message.get("role").and_then(Value::as_str) == Some("user") {
                                    "producer".into()
                                } else {
                                    string(message.get("role"))
                                },
                                clip(&words, 400)
                            ))
                        }
                    }
                    Some("tool-call") => lines.push(format!(
                        "  → {} {}",
                        string(part.get("toolName")),
                        clip(
                            &stringify(
                                part.get("input")
                                    .filter(|v| !v.is_null())
                                    .or_else(|| part.get("args").filter(|v| !v.is_null()))
                                    .unwrap_or(&serde_json::json!({}))
                            ),
                            800
                        )
                    )),
                    Some("tool-result") => lines.push(format!(
                        "  ← {}",
                        clip(
                            &stringify(
                                part.get("output")
                                    .filter(|v| !v.is_null())
                                    .or_else(|| part.get("result").filter(|v| !v.is_null()))
                                    .unwrap_or(&serde_json::json!({}))
                            ),
                            600
                        )
                    )),
                    _ => {}
                }
            }
        }
    }
    if let Some(changes) = value.get("changes").and_then(Value::as_array).filter(|c| !c.is_empty()) {
        lines.extend(["".into(), "HISTORY:".into()]);
        for change in changes.iter().skip(changes.len().saturating_sub(100)) {
            lines.push(format!(
                "  {} · {}{}",
                string(change.get("state")),
                string(change.get("title")),
                change.get("note").and_then(Value::as_str).map(|s| format!(" ({s})")).unwrap_or_default()
            ))
        }
    }
    lines
}
/// Each column's median over the logged turns (so the parts needn't add up to the time); "first part"
/// is each turn's first model call's. Then the slowest tools across them (each turn logs its slowest
/// three), and the efforts and service tiers the model was asked for.
fn summarize_timings(lines: &[&str]) -> String {
    let turns: Vec<Value> = lines.iter().filter_map(|line| serde_json::from_str(line).ok()).collect();
    let median = |key: &str| {
        let mut values: Vec<f64> = turns.iter().filter_map(|turn| turn[key].as_f64()).collect();
        values.sort_by(f64::total_cmp);
        values.get(values.len() / 2).copied().unwrap_or(0.)
    };
    let mut first: Vec<f64> = turns.iter().filter_map(|turn| turn["firstPartMs"][0].as_f64()).collect();
    first.sort_by(f64::total_cmp);
    let seconds = |ms: f64| format!("{:.1} s", ms / 1000.);
    let mut summary = format!(
        "{} turns. Medians: {} an answer · model {} · tools {} · {} model calls · first part {} · {} {} requests · {} KB sent",
        turns.len(),
        seconds(median("ms")),
        seconds(median("modelMs")),
        seconds(median("toolMs")),
        median("modelCalls"),
        seconds(first.get(first.len() / 2).copied().unwrap_or(0.)),
        median("liveRequests"),
        crate::daw::words().app,
        (median("sentBytes") / 1024.).round(),
    );
    let count = |n: f64, what: &str| format!("{n} {what}{}", if n == 1. { "" } else { "s" });
    // Each turn's look at the Set before the model is asked, where it was timed (#191).
    if turns.iter().any(|turn| turn["lookMs"].is_number()) {
        summary.push_str(&format!(
            "\nLooking at the {} before each answer (medians): {} · {} · {} KB",
            crate::daw::words().place,
            seconds(median("lookMs")),
            count(median("lookRequests"), &format!("{} request", crate::daw::words().app)),
            (median("lookBytes") / 1024.).round(),
        ));
    }
    let mut tools: Vec<(String, f64, f64)> = Vec::new();
    for slow in turns.iter().filter_map(|turn| turn["slowTools"].as_array()).flatten() {
        let (Some(tool), Some(calls), Some(ms)) = (slow["tool"].as_str(), slow["calls"].as_f64(), slow["ms"].as_f64()) else { continue };
        match tools.iter_mut().find(|(name, ..)| name == tool) {
            Some((_, all_calls, all_ms)) => (*all_calls, *all_ms) = (*all_calls + calls, *all_ms + ms),
            None => tools.push((tool.to_string(), calls, ms)),
        }
    }
    tools.sort_by(|a, b| b.2.total_cmp(&a.2));
    if !tools.is_empty() {
        let slowest: Vec<_> =
            tools.iter().take(3).map(|(tool, calls, ms)| format!("{tool} {} ({})", seconds(*ms), count(*calls, "call"))).collect();
        summary.push_str(&format!("\nSlowest tools (each turn's three, summed): {}", slowest.join(" · ")));
    }
    // Lines from before efforts were logged have none, and aren't counted.
    for (key, label) in [("effort", "Effort"), ("tier", "Service tier")] {
        let mut asked: Vec<(String, f64)> = Vec::new();
        for value in turns.iter().filter_map(|turn| turn[key].as_str()) {
            match asked.iter_mut().find(|(name, _)| name == value) {
                Some((_, turns)) => *turns += 1.,
                None => asked.push((value.to_string(), 1.)),
            }
        }
        if !asked.is_empty() {
            let said: Vec<_> = asked.iter().map(|(value, turns)| format!("{value} ({})", count(*turns, "turn"))).collect();
            summary.push_str(&format!("\n{label}: {}", said.join(" · ")));
        }
    }
    summary
}
pub fn username() -> String {
    #[cfg(unix)]
    {
        unsafe {
            let mut entry: libc::passwd = std::mem::zeroed();
            let mut result = std::ptr::null_mut();
            let mut buffer = vec![0u8; 16384];
            if libc::getpwuid_r(libc::geteuid(), &mut entry, buffer.as_mut_ptr().cast(), buffer.len(), &mut result) == 0
                && !result.is_null()
                && !entry.pw_name.is_null()
            {
                return std::ffi::CStr::from_ptr(entry.pw_name).to_string_lossy().into_owned();
            }
        }
    }
    #[cfg(windows)]
    {
        #[link(name = "advapi32")]
        unsafe extern "system" {
            fn GetUserNameW(buffer: *mut u16, len: *mut u32) -> i32;
        }
        unsafe {
            let mut buffer = [0u16; 257];
            let mut len = buffer.len() as u32;
            if GetUserNameW(buffer.as_mut_ptr(), &mut len) != 0 {
                return String::from_utf16_lossy(&buffer[..len.saturating_sub(1) as usize]);
            }
        }
    }
    String::new()
}
/// Every saved sign-in's strings, the keys in the environment and model servers' keys: what a report, `doctor --json` or an export takes out.
pub async fn known_secrets(env: &Env) -> Vec<String> {
    let mut secrets = vec![];
    if let Ok(file) = load_auth_file(env) {
        if let Ok(credentials) = open_credential_store(file).list().await {
            for credential in credentials.values() {
                strings(&serde_json::to_value(credential).unwrap_or(Value::Null), &mut secrets);
            }
        }
    }
    for name in KEY_VARIABLES {
        secrets.push(env.get(name).cloned().unwrap_or_default())
    }
    // A model server's key, as settings give it.
    if let Ok(file) = load_settings_file(env) {
        secrets.extend(read_settings(&file).model_servers.into_iter().filter_map(|server| server.api_key));
    }
    secrets
}
/// The variables that hold a key or token Kumi uses.
pub const KEY_VARIABLES: [&str; 8] = [
    "AI_GATEWAY_API_KEY",
    "OPENAI_API_KEY",
    "ANTHROPIC_API_KEY",
    "OPENCODE_API_KEY",
    "LM_API_TOKEN",
    "GEMINI_API_KEY",
    "GOOGLE_API_KEY",
    "KUMI_LISTENER_KEY",
];
/// What's safe to share: a report's redaction, for this computer's home folder and account name.
pub async fn redaction(env: &Env) -> impl Fn(&str) -> String {
    redactor(&known_secrets(env).await, &home::home_dir().unwrap_or_default().display().to_string(), &username())
}
pub async fn write_report(io: ReportIo) -> Result<i32, RuntimeError> {
    let env = &io.doctor.env;
    let home = io.home.clone().unwrap_or_else(|| home::home_dir().unwrap_or_default().display().to_string());
    let user = io.user.clone().unwrap_or_else(username);
    let now = io.now.as_ref().map(|now| now()).unwrap_or_else(Utc::now);
    let redact = redactor(&known_secrets(env).await, &home, &user);
    let file = step(io.doctor.out.clone(), env, "Writing Kumi's report…", compose(&io, &redact, &home, now), false).await?;
    io.doctor.out.write(&format!("Kumi's report is in {}\nIt has Kumi's versions, the doctor's checks, what Kumi did in your last conversation, and the bridge's lines from {}'s log. Keys and tokens are taken out. Send it with a few words about what happened.\n",redact(&file),crate::daw::words().app));
    Ok(0)
}
async fn compose(io: &ReportIo, redact: &dyn Fn(&str) -> String, home: &str, now: DateTime<Utc>) -> Result<String, RuntimeError> {
    let env = &io.doctor.env;
    let mut sections = vec![];
    let mut section = |title: &str, body: String| sections.push(format!("## {title}\n\n{body}"));
    let mut terminal = vec![];
    if let Some(program) = env.get("TERM_PROGRAM").filter(|s| !s.is_empty()) {
        terminal.push(trim(&format!("{program} {}", env.get("TERM_PROGRAM_VERSION").map(String::as_str).unwrap_or(""))).to_string())
    }
    if env.get("WT_SESSION").is_some_and(|s| !s.is_empty()) {
        terminal.push("Windows Terminal".into())
    }
    for key in ["TERM", "COLORTERM"] {
        if let Some(value) = env.get(key).filter(|s| !s.is_empty()) {
            terminal.push(format!("{key}={value}"))
        }
    }
    let arch = match std::env::consts::ARCH {
        "x86_64" => "x64",
        "aarch64" => "arm64",
        other => other,
    };
    section(
        "Versions",
        format!(
            "Kumi {KUMI_VERSION}\nRuntime: native Rust\n{} {} {arch}\nTerminal: {}\nMade {}",
            system::platform(),
            os_release(),
            if terminal.is_empty() { "unknown".into() } else { terminal.join(", ") },
            now.to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
        ),
    );
    let doctor = match doctor_sections(&io.doctor).await {
        Ok(sections) => format_sections(&sections),
        Err(error) => format_doctor(&[Check::fix(format!("The doctor stopped: {}", head(&error.message(), 200)), None)]),
    };
    section("Doctor", trim_end(&doctor).to_string());
    let settings = match load_settings_file(env) {
        Ok(file) => read(&file).await.and_then(|s| serde_json::from_str::<Value>(&s).ok()),
        _ => None,
    };
    section(
        "Settings",
        if let Some(settings) = settings {
            settings
                .as_object()
                .into_iter()
                .flat_map(|o| o.iter())
                .filter(|(_, v)| !v.is_object() && !v.is_array() && !v.is_null())
                .map(|(key, value)| format!("{key}: {}", head(&string(Some(value)), 120)))
                .collect::<Vec<_>>()
                .join("\n")
        } else {
            "none saved".into()
        },
    );
    let conversation = match load_projects_dir(env) {
        Ok(folder) => last_conversation(&folder).await,
        _ => None,
    };
    section(
        "Last conversation",
        if let Some((at, id, value)) = conversation {
            let date = DateTime::from_timestamp_millis(at as i64).unwrap_or_default().to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
            format!("Conversation {id}, saved {date}\n\n{}", describe_conversation(&value).join("\n"))
        } else {
            "none kept yet".into()
        },
    );
    // Kumi's database holds the gap log once Kumi has used it, read without writing anything, with what
    // an older Kumi logged in the file since.
    let logged = match (load_db_file(env), load_gaps_file(env)) {
        (Ok(db), Ok(file)) if std::path::Path::new(&db).is_file() => {
            tokio::task::spawn_blocking(move || logged_gaps(db.as_ref(), file.as_ref())).await.ok().and_then(Result::ok)
        }
        _ => None,
    };
    let gaps = match (&logged, load_gaps_file(env)) {
        (Some(lines), _) => lines.join("\n"),
        (None, Ok(file)) => read(&file).await.unwrap_or_default(),
        _ => String::new(),
    };
    let gaps: Vec<_> = trim(&gaps).split('\n').filter(|s| !s.is_empty()).collect();
    section(
        "What Kumi couldn't do (gap log)",
        if gaps.is_empty() {
            "nothing logged".into()
        } else {
            gaps.iter().skip(gaps.len().saturating_sub(50)).map(|s| clip(s, 500)).collect::<Vec<_>>().join("\n")
        },
    );
    let timings = match load_timings_file(env) {
        Ok(file) => read(&file).await.unwrap_or_default(),
        _ => String::new(),
    };
    let timings: Vec<_> = trim(&timings).split('\n').filter(|s| !s.is_empty()).collect();
    let timings = &timings[timings.len().saturating_sub(50)..];
    section(
        "Turn timing (last 50 turns)",
        if timings.is_empty() {
            "nothing timed yet".into()
        } else {
            format!("{}\n\n{}", summarize_timings(timings), timings.iter().map(|s| clip(s, 500)).collect::<Vec<_>>().join("\n"))
        },
    );
    // Each DAW's log: its bridge's lines and errors.
    // Each DAW's log the producer has: Live's as always, another DAW's once its bridge is in (none for a Live-only
    // producer).
    let has_log = |provider: &&dyn kumi_daw::provider::DawProvider| {
        crate::daw::in_use(*provider, env) && (provider.id() == kumi_types::daw::DawId::Ableton || provider.bridge_installed(env))
    };
    let daw = kumi_daw::io::DawIo { answers: io.answers.clone(), ..kumi_daw::io::DawIo::new(env.clone()) };
    for provider in crate::daw::providers().into_iter().filter(has_log) {
        let (heading, body) = provider.report_log(&daw, home).await;
        section(&heading, body);
    }
    let text=redact(&format!("# Kumi report\n\nSend this file with a few words about what happened. Keys and tokens are taken out; your home folder shows as ~.\n\n{}\n",sections.join("\n\n")));
    let stamp = now.format("%Y-%m-%dT%H-%M-%S");
    let file = join(io.folder.as_deref().unwrap_or(home), &format!("kumi-report-{stamp}.txt"));
    let mut open = tokio::fs::OpenOptions::new();
    open.create(true).truncate(true).write(true);
    #[cfg(unix)]
    open.mode(0o600);
    use tokio::io::AsyncWriteExt;
    let mut output = open.open(Path::new(&file)).await.map_err(|e| RuntimeError::plain(e.to_string()))?;
    output.write_all(text.as_bytes()).await.map_err(|e| RuntimeError::plain(e.to_string()))?;
    // Tokio queues file writes; finish the write before announcing the report is ready.
    output.flush().await.map_err(|e| RuntimeError::plain(e.to_string()))?;
    Ok(file)
}
