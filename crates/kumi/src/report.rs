//! `kumi report`, with native runtime version information.
use crate::{
    config::{load_auth_file, load_gaps_file, load_projects_dir, load_settings_file, load_timings_file},
    doctor::{doctor_checks, format_doctor, Check, DoctorIo},
    spinner::step,
    tui::style::os_release,
};
use chrono::{DateTime, Utc};
use futures::future::LocalBoxFuture;
use kumi_common::js::{
    json::stringify,
    number,
    string::{trim, trim_end, utf16_len},
};
use kumi_runtime::{
    auth::store::{open_credential_store, CredentialStore},
    core::errors::RuntimeError,
    library::sources::join,
    system::{self, Env},
    KUMI_VERSION,
};
use serde_json::Value;
use std::{path::Path, rc::Rc, time::UNIX_EPOCH};
pub struct ReportIo {
    pub doctor: DoctorIo,
    pub folder: Option<String>,
    pub home: Option<String>,
    pub user: Option<String>,
    pub now: Option<Rc<dyn Fn() -> DateTime<Utc>>>,
    pub live_logs: Option<Rc<dyn Fn() -> LocalBoxFuture<'static, Result<Vec<String>, RuntimeError>>>>,
}
impl ReportIo {
    pub fn new(doctor: DoctorIo) -> Self {
        Self { doctor, folder: None, home: None, user: None, now: None, live_logs: None }
    }
}
/// Secrets are removed before paths and usernames, with the same bounded heuristic patterns as the source.
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
    let name = (utf16_len(user) >= 3)
        .then(|| fancy_regex::Regex::new(&format!(r"(?i)(?<![A-Za-z0-9]){}(?![A-Za-z0-9])", regex::escape(user))).unwrap());
    let whitespace = r"\x09-\x0d \u{00a0}\u{1680}\u{2000}-\u{200a}\u{2028}\u{2029}\u{202f}\u{205f}\u{3000}\u{feff}";
    let bearer = regex::Regex::new(&format!(r#"(?i)(?-u:\b)Bearer[{whitespace}]+[^{whitespace}"']+"#)).unwrap();
    let key = regex::Regex::new(r"(?-u:\b)(sk|pk|rk)-[A-Za-z0-9_-]{12,}").unwrap();
    let jwt = regex::Regex::new(r"(?-u:\b)eyJ[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{8,}").unwrap();
    let field = regex::Regex::new(&format!(
        r#"(?i)("?(?:api[_-]?key|token|secret|password|access|refresh)"?[{whitespace}]*[:=][{whitespace}]*)"([^"]*)""#
    ))
    .unwrap();
    let long = fancy_regex::Regex::new(r"(?<![A-Za-z0-9+_-])[A-Za-z0-9+_-]{48,}={0,2}(?![A-Za-z0-9+_-])").unwrap();
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
        out = field
            .replace_all(
                &out,
                |caps: &regex::Captures| if utf16_len(&caps[2]) >= 8 { format!("{}\"[secret]\"", &caps[1]) } else { caps[0].to_string() },
            )
            .into_owned();
        out = long.replace_all(&out, "[long value]").into_owned();
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
fn clip(text: &str, most: usize) -> String {
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
async fn read(file: &str) -> Option<String> {
    tokio::fs::read(file).await.ok().map(|b| String::from_utf8_lossy(&b).into_owned())
}
async fn names(folder: &str) -> Vec<String> {
    let Ok(mut reader) = tokio::fs::read_dir(folder).await else { return vec![] };
    let mut result = vec![];
    while let Ok(Some(entry)) = reader.next_entry().await {
        result.push(entry.file_name().to_string_lossy().into_owned())
    }
    result.sort();
    result
}
async fn modified(file: &str) -> Option<f64> {
    let time = tokio::fs::metadata(file).await.ok()?.modified().ok()?;
    match time.duration_since(UNIX_EPOCH) {
        Ok(time) => Some(time.as_secs_f64() * 1000.),
        Err(error) => Some(-error.duration().as_secs_f64() * 1000.),
    }
}
async fn last_conversation(projects: &str) -> Option<(f64, Value)> {
    let mut best: Option<(String, f64)> = None;
    for place in names(projects).await {
        let folder = join(projects, &format!("{place}/conversations"));
        for name in names(&folder).await {
            if !name.ends_with(".json") {
                continue;
            }
            let file = join(&folder, &name);
            if let Some(at) = modified(&file).await {
                if best.as_ref().is_none_or(|best| at > best.1) {
                    best = Some((file, at))
                }
            }
        }
    }
    let (file, at) = best?;
    Some((at, serde_json::from_str(&read(&file).await?).ok()?))
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
        "{} turns. Medians: {} an answer · model {} · tools {} · {} model calls · first part {} · {} Live requests · {} KB sent",
        turns.len(),
        seconds(median("ms")),
        seconds(median("modelMs")),
        seconds(median("toolMs")),
        median("modelCalls"),
        seconds(first.get(first.len() / 2).copied().unwrap_or(0.)),
        median("liveRequests"),
        (median("sentBytes") / 1024.).round(),
    );
    let count = |n: f64, what: &str| format!("{n} {what}{}", if n == 1. { "" } else { "s" });
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
async fn find_live_logs(env: &Env, home: &str) -> Vec<String> {
    let root = if system::platform() == "win32" {
        join(env.get("APPDATA").map(String::as_str).unwrap_or(&join(home, "AppData/Roaming")), "Ableton")
    } else {
        join(home, "Library/Preferences/Ableton")
    };
    let mut found = vec![];
    for version in names(&root).await {
        for file in [join(&root, &format!("{version}/Log.txt")), join(&root, &format!("{version}/Preferences/Log.txt"))] {
            if let Some(at) = modified(&file).await {
                found.push((file, at))
            }
        }
    }
    found.sort_by(|a, b| b.1.total_cmp(&a.1));
    found.into_iter().map(|p| p.0).collect()
}
async fn live_log_lines(file: &str) -> Vec<String> {
    let text = read(file).await.unwrap_or_default();
    let units: Vec<_> = text.encode_utf16().collect();
    let text = String::from_utf16_lossy(&units[units.len().saturating_sub(4 * 1024 * 1024)..]);
    let pick = regex::Regex::new(r"AbletonMcp|Traceback|RemoteScriptError|Python:.*(?-u:\b\w*)(Error|Exception)(?-u:\b)").unwrap();
    let follow = regex::Regex::new(r"^[\s\u{feff}]|Python:|^(?-u:\w*)(Error|Exception)(?-u:\b)").unwrap();
    let mut picked = vec![];
    let mut trailing = 0;
    for line in text.split('\n').map(|line| line.strip_suffix('\r').unwrap_or(line)) {
        if pick.is_match(line) {
            picked.push(line);
            trailing = if line.contains("Traceback") { 12 } else { 0 }
        } else if trailing > 0 && follow.is_match(line) {
            picked.push(line);
            trailing -= 1
        } else {
            trailing = 0
        }
    }
    picked.into_iter().rev().take(150).collect::<Vec<_>>().into_iter().rev().map(|s| clip(s, 400)).collect()
}
fn username() -> String {
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
pub async fn write_report(io: ReportIo) -> Result<i32, RuntimeError> {
    let env = &io.doctor.env;
    let home = io.home.clone().unwrap_or_else(|| home::home_dir().unwrap_or_default().display().to_string());
    let user = io.user.clone().unwrap_or_else(username);
    let now = io.now.as_ref().map(|now| now()).unwrap_or_else(Utc::now);
    let mut secrets = vec![];
    if let Ok(file) = load_auth_file(env) {
        if let Ok(credentials) = open_credential_store(file).list().await {
            for credential in credentials.values() {
                strings(&serde_json::to_value(credential).unwrap_or(Value::Null), &mut secrets);
            }
        }
    }
    for name in ["AI_GATEWAY_API_KEY", "OPENAI_API_KEY", "ANTHROPIC_API_KEY", "OPENCODE_API_KEY"] {
        secrets.push(env.get(name).cloned().unwrap_or_default())
    }
    let redact = redactor(&secrets, &home, &user);
    let file = step(io.doctor.out.clone(), env, "Writing Kumi's report…", compose(&io, &redact, &home, now), false).await?;
    io.doctor.out.write(&format!("Kumi's report is in {}\nIt has Kumi's versions, the doctor's checks, what Kumi did in your last conversation, and the bridge's lines from Live's log. Keys and tokens are taken out. Send it with a few words about what happened.\n",redact(&file)));
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
    let checks = doctor_checks(&io.doctor)
        .await
        .unwrap_or_else(|error| vec![Check::fix(format!("The doctor stopped: {}", head(&error.message(), 200)), None)]);
    section("Doctor", trim_end(&format_doctor(&checks)).to_string());
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
        if let Some((at, value)) = conversation {
            let date = DateTime::from_timestamp_millis(at as i64).unwrap_or_default().to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
            format!("Saved {date}\n\n{}", describe_conversation(&value).join("\n"))
        } else {
            "none kept yet".into()
        },
    );
    let gaps = match load_gaps_file(env) {
        Ok(file) => read(&file).await.unwrap_or_default(),
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
    let logs = if let Some(logs) = &io.live_logs { logs().await.unwrap_or_default() } else { find_live_logs(env, home).await };
    section(
        "Live's log (the bridge's lines and errors)",
        if let Some(log) = logs.first() {
            format!("From {log}\n\n{}", live_log_lines(log).await.join("\n"))
        } else {
            "Live's log wasn't found".into()
        },
    );
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
