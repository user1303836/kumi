//! Read pages, code, repositories, Max patches and pictures, using free readers when needed.
use super::{
    free::{free_services, free_trouble, FreeFailure, FreeServices},
    github::{decode, github_target, raw_url, read_github_tree, GithubTarget},
    html::read_html,
    net::{carries_key, checked_url, decode_text, status_words, WebClient, WebError, WebFailure, WebRequest, WebResponse},
};
use crate::devices::amxd::decode_amxd;
use kumi_common::{
    abort::{Signal, SignalExt},
    js::{
        json::stringify,
        number::{round, to_fixed, to_string},
        string::trim,
    },
};
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{rc::Rc, sync::LazyLock};
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PageImage {
    pub data: Vec<u8>,
    pub media_type: String,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Page {
    pub url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    pub kind: String,
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub via: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reader: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub truncated: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image: Option<PageImage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub files: Option<usize>,
}
impl Page {
    fn new(url: &str, kind: &str, text: String) -> Self {
        Self { url: url.into(), title: None, kind: kind.into(), text, via: None, reader: None, truncated: None, image: None, files: None }
    }
}
fn size_words(bytes: usize) -> String {
    if bytes < 1024 {
        format!("{bytes} bytes")
    } else if bytes < 1024 * 1024 {
        format!("{} KB", to_string(round(bytes as f64 / 1024.0)))
    } else {
        format!("{} MB", to_fixed(bytes as f64 / 1024.0 / 1024.0, 1))
    }
}
fn file_name(path: &str) -> Option<String> {
    path.split('/').filter(|s| !s.is_empty()).last().and_then(decode).filter(|s| !s.is_empty())
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PictureSize {
    pub width: u32,
    pub height: u32,
}
pub fn picture_size(data: &[u8], kind: &str) -> Option<PictureSize> {
    let le16 = |at| u16::from_le_bytes(data[at..at + 2].try_into().unwrap()) as u32;
    let be16 = |at| u16::from_be_bytes(data[at..at + 2].try_into().unwrap()) as u32;
    let be32 = |at| u32::from_be_bytes(data[at..at + 4].try_into().unwrap());
    let le32 = |at| u32::from_le_bytes(data[at..at + 4].try_into().unwrap());
    let le24 = |at| u32::from_le_bytes([data[at], data[at + 1], data[at + 2], 0]);
    if kind == "image/png" && data.len() >= 24 && &data[12..16] == b"IHDR" {
        return Some(PictureSize { width: be32(16), height: be32(20) });
    }
    if kind == "image/gif" && data.len() >= 10 && &data[..3] == b"GIF" {
        return Some(PictureSize { width: le16(6), height: le16(8) });
    }
    if kind == "image/webp" && data.len() >= 30 && &data[..4] == b"RIFF" && &data[8..12] == b"WEBP" {
        return match &data[12..16] {
            b"VP8X" => Some(PictureSize { width: 1 + le24(24), height: 1 + le24(27) }),
            b"VP8 " => Some(PictureSize { width: le16(26) & 0x3fff, height: le16(28) & 0x3fff }),
            b"VP8L" => {
                let bits = le32(21);
                Some(PictureSize { width: (bits & 0x3fff) + 1, height: ((bits >> 14) & 0x3fff) + 1 })
            }
            _ => None,
        };
    }
    if kind == "image/jpeg" && data.starts_with(&[0xff, 0xd8]) {
        let mut at = 2;
        while at + 9 < data.len() {
            if data[at] != 0xff {
                return None;
            }
            let marker = data[at + 1];
            if marker == 0xff {
                at += 1;
                continue;
            }
            if marker == 1 || (0xd0..=0xd8).contains(&marker) {
                at += 2;
                continue;
            }
            if (0xc0..=0xcf).contains(&marker) && ![0xc4, 0xc8, 0xcc].contains(&marker) {
                return Some(PictureSize { height: be16(at + 5), width: be16(at + 7) });
            }
            at += 2 + be16(at + 2) as usize;
        }
    }
    None
}
pub fn max_patch_summary(root: &Value) -> Option<String> {
    let top = root.get("patcher")?;
    if !top["boxes"].is_array() {
        return None;
    }
    let mut controls = Vec::new();
    let mut code = Vec::new();
    let mut objects = indexmap::IndexMap::<String, usize>::new();
    fn walk(
        patcher: &Value,
        at: &str,
        depth: usize,
        controls: &mut Vec<String>,
        code: &mut Vec<(String, String)>,
        objects: &mut indexmap::IndexMap<String, usize>,
    ) {
        if depth > 12 {
            return;
        }
        for entry in patcher["boxes"].as_array().into_iter().flatten() {
            let b = &entry["box"];
            if !b.is_object() {
                continue;
            }
            let kind = b["maxclass"].as_str().unwrap_or("");
            let text = b["text"].as_str().map(trim).unwrap_or("");
            if kind == "codebox" {
                if let Some(c) = b["code"].as_str() {
                    code.push((at.into(), c.into()));
                }
            } else if kind == "newobj" && !text.is_empty() {
                *objects.entry(text.split_whitespace().next().unwrap().into()).or_default() += 1;
            }
            let value = &b["saved_attribute_attributes"]["valueof"];
            if let Some(name) = value["parameter_longname"].as_str().filter(|_| kind.starts_with("live.")) {
                let range = if let Some(enumeration) = value["parameter_enum"].as_array() {
                    enumeration
                        .iter()
                        .map(|v| if v.is_null() { String::new() } else { super::net::js_string(v) })
                        .collect::<Vec<_>>()
                        .join(" / ")
                } else if value.get("parameter_mmin").is_some() || value.get("parameter_mmax").is_some() {
                    format!(
                        "{} to {}",
                        super::net::js_string(value.get("parameter_mmin").filter(|v| !v.is_null()).unwrap_or(&Value::from(0))),
                        super::net::js_string(value.get("parameter_mmax").filter(|v| !v.is_null()).unwrap_or(&Value::from(127)))
                    )
                } else {
                    String::new()
                };
                controls.push(format!(
                    "{name} ({kind}{}{})",
                    if range.is_empty() { String::new() } else { format!(", {range}") },
                    value.get("parameter_initial").map_or_else(String::new, |v| format!(", starts at {}", stringify(v)))
                ));
            }
            if b["patcher"].is_object() {
                let at = if text.is_empty() { at.into() } else { format!("{at} › {}", text.split_whitespace().next().unwrap()) };
                walk(&b["patcher"], &at, depth + 1, controls, code, objects);
            }
        }
    }
    walk(top, "the patch", 0, &mut controls, &mut code, &mut objects);
    if controls.is_empty() && code.is_empty() && objects.is_empty() {
        return None;
    }
    let mut lines = Vec::new();
    if !controls.is_empty() {
        lines.push(format!("Its controls: {}.", controls.join("; ")));
    }
    if !objects.is_empty() {
        let mut objects: Vec<_> = objects.into_iter().collect();
        objects.sort_by(|a, b| b.1.cmp(&a.1));
        lines.push(format!(
            "Made of: {}.",
            objects
                .into_iter()
                .take(80)
                .map(|(name, count)| if count > 1 { format!("{name} ×{count}") } else { name })
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    for (index, (at, block)) in code.iter().enumerate() {
        lines.extend([
            String::new(),
            format!("Code {} of {}, a codebox in {at}:", index + 1, code.len()),
            "```".into(),
            trim(&block.replace("\r\n", "\n").replace('\r', "\n")).into(),
            "```".into(),
        ]);
    }
    Some(lines.join("\n"))
}
async fn through_reader(
    client: &dyn WebClient,
    address: &str,
    why: &str,
    kind: &str,
    signal: Option<Signal>,
    services: &FreeServices,
) -> Result<Page, FreeFailure> {
    let read = services
        .first(
            "read",
            |service| {
                let signal = signal.clone();
                async move { service.read(client, address, signal).await }
            },
            signal.clone(),
            None,
        )
        .await?;
    let mut page = Page::new(address, kind, read.value.text);
    page.title = read.value.title.filter(|s| !s.is_empty());
    page.via = Some(why.into());
    page.reader = Some(read.service.name().into());
    Ok(page)
}
fn picture(url: &str, response: WebResponse, title: Option<String>) -> Page {
    let size = picture_size(&response.body, &response.content_type);
    let words = size.map_or_else(
        || size_words(response.body.len()),
        |s| format!("{}×{} pixels, {}", s.width, s.height, size_words(response.body.len())),
    );
    let mut page = Page::new(url, "a picture", String::new());
    page.title = title;
    if response.truncated
        || response.body.len() > 3_500_000
        || size.is_none_or(|s| s.width > 7000 || s.height > 7000 || s.width == 0 || s.height == 0)
    {
        page.text = format!("A picture ({words}), too large for Kumi to show.");
    } else {
        page.text = format!("A picture, {words}: it's shown to you after this.");
        page.image = Some(PageImage { data: response.body, media_type: response.content_type });
    }
    page
}
fn pretty(value: &Value) -> String {
    kumi_common::js::json::stringify_pretty(value, 1)
}

fn patch(url: &str, value: &Value, kind: &str, title: Option<String>, truncated: bool) -> Option<Page> {
    let summary = max_patch_summary(value)?;
    let mut page = Page::new(url, kind, format!("{summary}\n\nThe whole patch, as Max saves it:\n{}", pretty(value)));
    page.title = title;
    page.truncated = truncated.then_some(true);
    Some(page)
}
pub async fn read_page(
    client: &dyn WebClient,
    address: &str,
    signal: Option<Signal>,
    services: Option<Rc<FreeServices>>,
) -> Result<Page, WebFailure> {
    if carries_key(address) {
        return Err(WebError::new("That address carries what looks like a key or token, so Kumi won't read it: every server that sees an address gets what's in it. Read it without the key.").into());
    }
    let url = checked_url(address, None)?;
    let target = github_target(&url);
    let services = services.unwrap_or_else(free_services);
    if let Some(target) = target.as_ref().filter(|t| !matches!(t, GithubTarget::Blob { .. })) {
        let listing = read_github_tree(client, target, signal).await?;
        let mut page = Page::new(
            url.as_str(),
            if matches!(target, GithubTarget::Repo { .. }) { "a GitHub repository" } else { "a folder on GitHub" },
            listing.text,
        );
        page.title = Some(listing.title);
        page.files = Some(listing.files);
        return Ok(page);
    }
    let response = client
        .fetch(
            &target.as_ref().map_or_else(|| url.to_string(), |t| raw_url(t.owner(), t.repo(), t.rest())),
            WebRequest {
                signal: signal.clone(),
                wants: Some(Rc::new(|kind| kind != "application/pdf" && !kind.starts_with("video/") && !kind.starts_with("audio/"))),
                ..Default::default()
            },
        )
        .await?;
    let named = if target.is_some() { url.to_string() } else { response.url.clone() };
    let from = url::Url::parse(&response.url).map_err(|e| WebError::new(e.to_string()))?;
    let host = from.host_str().unwrap_or("");
    let path = target.as_ref().map_or(from.path(), GithubTarget::rest);
    if response.skipped {
        if response.content_type == "application/pdf" {
            return pdf(client, &response.url, signal, &services).await;
        }
        return Err(WebError::new(if response.content_type.starts_with("video/") {
            "That's a video: watch_video watches it."
        } else {
            "That's a sound file: listen hears one saved on this computer."
        })
        .into());
    }
    if response.status >= 400 {
        if target.is_none() && [401, 403, 429, 503].contains(&response.status) {
            match through_reader(client, &named, &format!("{host} {}", status_words(response.status)), "a page", signal.clone(), &services)
                .await
            {
                Ok(page) => return Ok(page),
                Err(_) => {
                    if let Some(signal) = &signal {
                        signal.check()?;
                    }
                }
            }
        }
        return Err(WebError::with_status(
            if let Some(target) = target.as_ref().filter(|_| response.status == 404) {
                format!("GitHub has no file there ({}/{}/{}).", target.owner(), target.repo(), target.rest())
            } else {
                format!("Kumi couldn't read {host}: {}.", status_words(response.status))
            },
            response.status,
        )
        .into());
    }
    let kind = &response.content_type;
    if ["image/png", "image/jpeg", "image/gif", "image/webp"].contains(&kind.as_str()) {
        let title = file_name(path);
        return Ok(picture(&named, response, title));
    }
    if kind == "application/pdf" || response.body.starts_with(b"%PDF-") {
        return pdf(client, &response.url, signal, &services).await;
    }
    if let Some(device) = decode_amxd(&response.body) {
        return Ok(patch(&named, &device.patcher, "a Max for Live device", file_name(path), response.truncated)
            .unwrap_or_else(|| Page::new(&named, "a Max for Live device", pretty(&device.patcher))));
    }
    if response.body[..response.body.len().min(8192)].contains(&0) {
        return Err(WebError::new(format!(
            "That's a file of another kind ({}, {}), not text Kumi can read.",
            if kind.is_empty() { "unnamed" } else { kind },
            size_words(response.body.len())
        ))
        .into());
    }
    let text = decode_text(&response.body, response.charset.as_deref());
    static HTML: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)^\s*<(!doctype html|html)\b").unwrap());
    if kind == "text/html" || kind == "application/xhtml+xml" || (kind.is_empty() && HTML.is_match(&text)) {
        let read = read_html(&text, &response.url);
        if read.scripted {
            match through_reader(client, &named, "its page is built by scripts", "a page", signal.clone(), &services).await {
                Ok(page) => return Ok(page),
                Err(_) => {
                    if let Some(signal) = &signal {
                        signal.check()?;
                    }
                }
            }
        }
        let lead = read.description.filter(|d| !d.is_empty() && !read.text.contains(d)).map_or_else(String::new, |d| format!("{d}\n\n"));
        let mut page = Page::new(&named, "a page", format!("{lead}{}", read.text));
        page.title = read.title.filter(|s| !s.is_empty());
        page.truncated = response.truncated.then_some(true);
        return Ok(page);
    }
    let lower = path.to_ascii_lowercase();
    if kind.contains("json") || [".maxpat", ".maxhelp", ".gendsp", ".json"].iter().any(|s| lower.ends_with(s)) {
        if let Ok(value) = serde_json::from_str(&text) {
            if let Some(read) = patch(
                &named,
                &value,
                if lower.ends_with(".gendsp") { "a gen~ patch" } else { "a Max patch" },
                file_name(path),
                response.truncated,
            ) {
                return Ok(read);
            }
        }
    }
    static CODE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?i)\.(c|h|cc|cpp|cxx|hh|hpp|hxx|inl|ino|m|mm|swift|rs|go|java|kt|scala|cs|fs|js|mjs|cjs|jsx|ts|tsx|py|rb|php|lua|pl|r|jl|dart|zig|nim|sh|bash|zsh|ps1|bat|cmake|mk|gradle|toml|ya?ml|ini|cfg|json|xml|gendsp|genexpr|maxpat|maxhelp|amxd|dsp|lib|sc|scd|ck|pd|csd|orc|sco|jsfx|eel|vhdl?|sv|asm|s|glsl|hlsl|wgsl|metal|cu|cl)$").unwrap()
    });
    let mut page = Page::new(&named, if CODE.is_match(path) { "code" } else { "text" }, text);
    page.title = file_name(path);
    page.truncated = response.truncated.then_some(true);
    Ok(page)
}
async fn pdf(client: &dyn WebClient, address: &str, signal: Option<Signal>, services: &FreeServices) -> Result<Page, WebFailure> {
    match through_reader(client, address, "it's a PDF", "a PDF", signal.clone(), services).await {
        Ok(page) => Ok(page),
        Err(error) => {
            if let Some(signal) = &signal {
                signal.check()?;
            }
            let said = match error {
                FreeFailure::NoService(none) => free_trouble(&none),
                FreeFailure::Other(error) => error.to_string().strip_suffix('.').unwrap_or(&error.to_string()).to_string(),
            };
            Err(WebError::new(format!(
                "Kumi reads a PDF through a free reader ({}), and none could read this one: {said}.",
                services.services.iter().map(|s| s.name()).collect::<Vec<_>>().join(", ")
            ))
            .into())
        }
    }
}
