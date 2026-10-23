//! Live's part of `kumi report`: the bridge's lines and errors from Live's own log.
use kumi_daw::files::{clip, modified, names, read};
use kumi_library::sources::join;
use kumi_types::system::{self, Env};

/// Live's log for the report: its heading, and the bridge's lines and errors from the newest log (`logs`, when given,
/// lists the log files in place of Live's).
pub(crate) async fn log_section(env: &Env, home: &str, logs: Option<Vec<String>>) -> (String, String) {
    let logs = match logs {
        Some(logs) => logs,
        None => find_live_logs(env, home).await,
    };
    let body = if let Some(log) = logs.first() {
        format!("From {log}\n\n{}", live_log_lines(log).await.join("\n"))
    } else {
        "Live's log wasn't found".into()
    };
    ("Live's log (the bridge's lines and errors)".into(), body)
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
