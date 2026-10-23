//! GitHub repositories as files and a README, files from their raw address.
use super::net::{decode_text, status_words, WebClient, WebError, WebFailure, WebRequest};
use kumi_common::{
    abort::{Signal, SignalExt},
    js::{
        json::stringify,
        number::{round, to_fixed, to_string},
        string::{head, trim},
    },
    time::now_ms,
};
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::sync::LazyLock;
use url::Url;
pub const GITHUB_API: &str = "https://api.github.com";
pub const GITHUB_RAW: &str = "https://raw.githubusercontent.com";
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum GithubTarget {
    Repo { owner: String, repo: String },
    Tree { owner: String, repo: String, rest: String },
    Blob { owner: String, repo: String, rest: String },
}
impl GithubTarget {
    pub fn owner(&self) -> &str {
        match self {
            Self::Repo { owner, .. } | Self::Tree { owner, .. } | Self::Blob { owner, .. } => owner,
        }
    }
    pub fn repo(&self) -> &str {
        match self {
            Self::Repo { repo, .. } | Self::Tree { repo, .. } | Self::Blob { repo, .. } => repo,
        }
    }
    pub fn rest(&self) -> &str {
        match self {
            Self::Repo { .. } => "",
            Self::Tree { rest, .. } | Self::Blob { rest, .. } => rest,
        }
    }
}
pub(crate) fn encode(text: &str) -> String {
    const SET: &percent_encoding::AsciiSet = &percent_encoding::NON_ALPHANUMERIC
        .remove(b'-')
        .remove(b'_')
        .remove(b'.')
        .remove(b'!')
        .remove(b'~')
        .remove(b'*')
        .remove(b'\'')
        .remove(b'(')
        .remove(b')');
    percent_encoding::utf8_percent_encode(text, SET).to_string()
}
pub(crate) fn decode(text: &str) -> Option<String> {
    let b = text.as_bytes();
    for (i, c) in b.iter().enumerate() {
        if *c == b'%' && (i + 2 >= b.len() || !b[i + 1].is_ascii_hexdigit() || !b[i + 2].is_ascii_hexdigit()) {
            return None;
        }
    }
    percent_encoding::percent_decode_str(text).decode_utf8().ok().map(|s| s.into_owned())
}
pub fn github_target(address: &Url) -> Option<GithubTarget> {
    if !matches!(address.host_str(), Some("github.com" | "www.github.com")) {
        return None;
    }
    let parts: Vec<_> = address.path().split('/').filter(|s| !s.is_empty()).map(decode).collect::<Option<_>>()?;
    let owner = parts.first()?.clone();
    let repo = parts.get(1)?.strip_suffix(".git").unwrap_or(&parts[1]).to_string();
    if "about apps codespaces collections contact copilot customer-stories enterprise events explore features issues login marketplace new notifications organizations orgs pricing pulls readme resources search security settings site sponsors team topics trending users".split(' ').any(|n|n==owner.to_lowercase()){return None;}
    let valid = |s: &str| !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || "_.-".contains(c));
    if !valid(&owner) || !valid(&repo) {
        return None;
    }
    match parts.get(2).map(String::as_str) {
        None => Some(GithubTarget::Repo { owner, repo }),
        Some("tree") if parts.len() > 3 => Some(GithubTarget::Tree { owner, repo, rest: parts[3..].join("/") }),
        Some("blob" | "raw") if parts.len() > 4 => Some(GithubTarget::Blob { owner, repo, rest: parts[3..].join("/") }),
        _ => None,
    }
}
pub fn raw_url(owner: &str, repo: &str, rest: &str) -> String {
    format!("{GITHUB_RAW}/{}/{}/{}", encode(owner), encode(repo), rest.split('/').map(encode).collect::<Vec<_>>().join("/"))
}
async fn api(client: &dyn WebClient, path: &str, signal: Option<Signal>) -> Result<(u16, Option<Value>), WebFailure> {
    let response = client
        .fetch(
            &format!("{GITHUB_API}{path}"),
            WebRequest {
                headers: [("accept".into(), "application/vnd.github+json".into()), ("x-github-api-version".into(), "2022-11-28".into())]
                    .into(),
                max_bytes: Some(24 * 1024 * 1024),
                signal,
                ..Default::default()
            },
        )
        .await?;
    if response.status == 200 {
        return Ok((
            200,
            Some(serde_json::from_slice(&response.body).map_err(|_| WebError::new("GitHub answered in a way Kumi doesn't follow."))?),
        ));
    }
    if matches!(response.status, 403 | 429) && response.headers.get("x-ratelimit-remaining").is_some_and(|v| v == "0") {
        let reset = response.headers.get("x-ratelimit-reset").and_then(|s| s.parse::<f64>().ok()).map(|v| v * 1000.0).unwrap_or(f64::NAN);
        let when = if reset.is_finite() && reset > now_ms() as f64 {
            chrono::DateTime::from_timestamp_millis(reset as i64)
                .map(|v| v.with_timezone(&chrono::Local).format("%I:%M %p").to_string())
                .unwrap_or_else(|| "later this hour".into())
        } else {
            "later this hour".into()
        };
        return Err(WebError::with_status(format!("GitHub allows 60 reads an hour without signing in, and they're used up until {when}. A file can still be read by its own address (github.com/…/blob/…), which doesn't count."),response.status).into());
    }
    Ok((response.status, None))
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Listing {
    pub title: String,
    pub text: String,
    pub files: usize,
}
fn size(bytes: Option<f64>) -> String {
    match bytes {
        None => String::new(),
        Some(b) if b < 1024.0 => format!(" ({} B)", to_string(b)),
        Some(b) if b < 1024.0 * 1024.0 => format!(" ({} KB)", to_string(round(b / 1024.0))),
        Some(b) => format!(" ({} MB)", to_fixed(b / 1024.0 / 1024.0, 1)),
    }
}
fn string<'a>(value: &'a Value, key: &str) -> &'a str {
    value.get(key).and_then(Value::as_str).unwrap_or("")
}
struct Entry {
    path: String,
    kind: String,
    size: Option<f64>,
}
pub async fn read_github_tree(client: &dyn WebClient, target: &GithubTarget, signal: Option<Signal>) -> Result<Listing, WebFailure> {
    let base = format!("/repos/{}/{}", encode(target.owner()), encode(target.repo()));
    let (status, info) = api(client, &base, signal.clone()).await?;
    if status == 404 {
        return Err(WebError::with_status(
            format!("GitHub has no public repository {}/{} (it may be private, renamed or misspelled).", target.owner(), target.repo()),
            404,
        )
        .into());
    }
    let info = info.ok_or_else(|| WebError::with_status(format!("GitHub {}.", status_words(status)), status))?;
    let fallback = format!("{}/{}", target.owner(), target.repo());
    let name = info["full_name"].as_str().unwrap_or(&fallback);
    let default_branch = info["default_branch"].as_str().unwrap_or("HEAD");
    let segments: Vec<_> =
        if matches!(target, GithubTarget::Tree { .. }) { target.rest().split('/').collect() } else { vec![default_branch] };
    let (mut tree, mut reference, mut folder) = (None, segments[0].to_string(), String::new());
    for cut in 1..=3.min(segments.len()) {
        reference = segments[..cut].join("/");
        folder = segments[cut..].join("/");
        let (status, found) = api(client, &format!("{base}/git/trees/{}?recursive=1", encode(&reference)), signal.clone()).await?;
        if found.is_some() {
            tree = found;
            break;
        }
        if status == 409 {
            return Err(WebError::with_status(format!("{name} is empty."), 409).into());
        }
        if !matches!(status, 404 | 422) {
            return Err(WebError::with_status(format!("GitHub {}.", status_words(status)), status).into());
        }
    }
    let tree =
        tree.ok_or_else(|| WebError::with_status(format!("{name} has no branch or tag {}.", stringify(&json!(segments[0]))), 404))?;
    let prefix = if folder.is_empty() { String::new() } else { format!("{}/", folder.strip_suffix('/').unwrap_or(&folder)) };
    static JUNK: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(^|/)(\.DS_Store|__MACOSX|Thumbs\.db)(/|$)").unwrap());
    let entries: Vec<_> = tree["tree"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|entry| {
            let path = entry["path"].as_str()?;
            if !path.starts_with(&prefix) || path == folder || JUNK.is_match(path) {
                return None;
            }
            Some(Entry {
                path: path[prefix.len()..].into(),
                kind: entry["type"].as_str().unwrap_or("blob").into(),
                size: entry["size"].as_f64(),
            })
        })
        .collect();
    if !folder.is_empty() && entries.is_empty() {
        return Err(WebError::with_status(format!("{name} has no folder {folder} on {reference}."), 404).into());
    }
    let files: Vec<_> = entries.iter().filter(|e| e.kind == "blob").collect();
    let mut about = Vec::new();
    let description = trim(string(&info, "description"));
    if !description.is_empty() {
        about.push(description.into());
    }
    if let Some(stars) = info["stargazers_count"].as_f64() {
        about.push(format!("{} stars", to_string(stars)));
    }
    for (value, prefix) in
        [(string(&info, "language"), ""), (string(&info["license"], "spdx_id"), "license "), (string(&info, "homepage"), "homepage ")]
    {
        if !value.is_empty() && !(prefix == "license " && value == "NOASSERTION") {
            about.push(format!("{prefix}{value}"));
        }
    }
    about.push(format!(
        "{} {reference}",
        if Some(reference.as_str()) == info["default_branch"].as_str() { "default branch" } else { "ref" }
    ));
    if !string(&info, "pushed_at").is_empty() {
        about.push(format!("last changed {}", head(string(&info, "pushed_at"), 10)));
    }
    if info["archived"] == true {
        about.push("archived".into());
    }
    if info["fork"] == true {
        about.push("a fork".into());
    }
    let mut lines=vec![format!("GitHub repository {name}{}: {}",if folder.is_empty(){String::new()}else{format!(", folder {folder}")},about.join(" · ")),String::new(),format!("Read a file with read_web and its address, https://github.com/{name}/blob/{reference}/{prefix}<path>; a folder with https://github.com/{name}/tree/{reference}/{prefix}<folder>.")];
    lines.push(String::new());
    if files.len() <= 400 {
        lines.push(format!("Files ({}):", files.len()));
        for entry in &entries {
            if entry.kind == "blob" {
                lines.push(format!("{}{}", entry.path, size(entry.size)));
            } else if entry.kind == "commit" {
                lines.push(format!("{}/ (another repository, linked in)", entry.path));
            }
        }
    } else {
        let mut counts = indexmap::IndexMap::<&str, usize>::new();
        let mut own = Vec::new();
        for entry in &files {
            if let Some((dir, _)) = entry.path.split_once('/') {
                *counts.entry(dir).or_default() += 1;
            } else {
                own.push(format!("{}{}", entry.path, size(entry.size)));
            }
        }
        lines.push(format!("{} files, too many to list at once; the folders, and the files at the top:", files.len()));
        for (dir, count) in counts {
            lines.push(format!("{dir}/ ({count} files)"));
        }
        lines.extend(own);
    }
    if tree["truncated"] == true {
        lines.push("(GitHub listed only part of a repository this large; read a folder for the rest.)".into());
    }
    static README: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)^readme(\.(md|markdown|txt|rst|org))?$").unwrap());
    let mut readmes: Vec<_> = entries.iter().filter(|e| e.kind == "blob" && README.is_match(&e.path)).collect();
    readmes.sort_by_key(|e| !e.path.to_ascii_lowercase().ends_with(".md"));
    if let Some(readme) = readmes.first() {
        let fetched = client
            .fetch(
                &raw_url(target.owner(), target.repo(), &format!("{reference}/{prefix}{}", readme.path)),
                WebRequest { max_bytes: Some(512 * 1024), signal: signal.clone(), ..Default::default() },
            )
            .await;
        match fetched {
            Ok(response) if response.status == 200 => lines.extend([
                String::new(),
                format!("{}:", readme.path),
                String::new(),
                trim(&decode_text(&response.body, response.charset.as_deref())).into(),
            ]),
            Err(error) => {
                if let Some(signal) = &signal {
                    signal.check()?;
                }
                if matches!(error, WebFailure::Aborted) {
                    return Err(error);
                }
            }
            _ => {}
        }
    }
    Ok(Listing {
        title: format!("{name}{}", if folder.is_empty() { String::new() } else { format!("/{folder}") }),
        text: lines.join("\n"),
        files: files.len(),
    })
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RepoFound {
    pub name: String,
    pub url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stars: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub updated: Option<String>,
}
pub async fn search_github(
    client: &dyn WebClient,
    query: &str,
    count: usize,
    signal: Option<Signal>,
) -> Result<Vec<RepoFound>, WebFailure> {
    let (status, data) = api(client, &format!("/search/repositories?q={}&per_page={count}", encode(query)), signal).await?;
    let data = data.ok_or_else(|| {
        WebError::with_status(
            if status == 422 {
                "GitHub couldn't search for that; try other words.".into()
            } else {
                format!("GitHub's search {}.", status_words(status))
            },
            status,
        )
    })?;
    Ok(data["items"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|item| !string(item, "full_name").is_empty() && !string(item, "html_url").is_empty())
        .map(|item| {
            let field = |key| {
                let s = string(item, key);
                (!s.is_empty()).then(|| s.to_string())
            };
            RepoFound {
                name: string(item, "full_name").into(),
                url: string(item, "html_url").into(),
                description: field("description"),
                stars: item["stargazers_count"].as_f64(),
                language: field("language"),
                updated: field("pushed_at").map(|s| head(&s, 10)),
            }
        })
        .collect())
}
