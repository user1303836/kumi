//! Exa's public MCP search and reader.
use super::{
    mcp_call::{mcp_tool, McpCall},
    net::{WebClient, WebError, WebFailure},
};
use kumi_common::{
    abort::Signal,
    js::string::{head, trim},
};
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::{rc::Rc, sync::LazyLock};
pub const EXA_URL: &str = "https://mcp.exa.ai/mcp";
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Found {
    pub title: String,
    pub url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub published: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ReadText {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    pub text: String,
}
#[derive(Clone, Default)]
pub struct SearchOptions {
    pub about: Option<String>,
    pub count: usize,
    pub signal: Option<Signal>,
}
fn explain(text: &str) -> Option<String> {
    if text.contains("NOT_FOUND") {
        Some("found nothing at that address".into())
    } else if text.to_ascii_lowercase().contains("timeout") {
        Some("timed out".into())
    } else {
        None
    }
}
pub fn parse_exa_results(text: &str) -> Vec<Found> {
    static SEPARATOR: LazyLock<fancy_regex::Regex> = LazyLock::new(|| fancy_regex::Regex::new(r"\n+---\n+(?=Title: )").unwrap());
    static HIGHLIGHTS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?ms)^Highlights:\n(.*)$").unwrap());
    SEPARATOR
        .split(text)
        .filter_map(Result::ok)
        .filter_map(|block| {
            let field = |key: &str| {
                let prefix = format!("{key}: ");
                block
                    .split(['\r', '\n', '\u{2028}', '\u{2029}'])
                    .find_map(|line| line.strip_prefix(&prefix))
                    .map(|text| trim(text).to_string())
            };
            let url = field("URL").filter(|s| s.starts_with("http://") || s.starts_with("https://"))?;
            let published = field("Published").filter(|s| !s.is_empty() && s != "N/A").map(|s| head(&s, 10));
            let text = HIGHLIGHTS.captures(block).map(|c| trim(&c[1]).to_string()).filter(|s| !s.is_empty());
            Some(Found { title: field("Title").filter(|s| !s.is_empty()).unwrap_or_else(|| url.clone()), url, published, text })
        })
        .collect()
}
pub async fn exa_search(client: &dyn WebClient, query: &str, options: SearchOptions) -> Result<Vec<Found>, WebFailure> {
    let args = json!({"query":query,"numResults":options.count,"objective":options.about.filter(|s|!s.is_empty()).unwrap_or_else(||format!("The pages that best answer: {query}"))});
    let text = mcp_tool(
        client,
        EXA_URL,
        "Exa",
        "web_search_exa",
        args.as_object().unwrap().clone(),
        McpCall { timeout_ms: 20_000, signal: options.signal, explain: Some(Rc::new(explain)) },
    )
    .await?;
    Ok(parse_exa_results(&text))
}
pub async fn exa_read(client: &dyn WebClient, url: &str, signal: Option<Signal>) -> Result<ReadText, WebFailure> {
    let text = mcp_tool(
        client,
        EXA_URL,
        "Exa",
        "web_fetch_exa",
        json!({"urls":[url],"maxCharacters":400_000}).as_object().unwrap().clone(),
        McpCall { timeout_ms: 90_000, signal, explain: Some(Rc::new(explain)) },
    )
    .await?;
    let lines: Vec<_> = text.split('\n').collect();
    let title = lines
        .first()
        .and_then(|s| s.strip_prefix("# "))
        .filter(|s| !s.is_empty() && !s.contains(['\r', '\u{2028}', '\u{2029}']))
        .map(|s| trim(s).to_string());
    let mut start = usize::from(title.as_ref().is_some_and(|s| !s.is_empty()));
    while start < lines.len() && ["URL: ", "Published: ", "Author: ", "Title: "].iter().any(|prefix| lines[start].starts_with(prefix)) {
        start += 1;
    }
    let body = lines[start..].join("\n");
    let body = trim(&body).to_string();
    if body.is_empty() {
        return Err(WebError::new("Exa found no text there.").into());
    }
    Ok(ReadText { title: title.filter(|s| !s.is_empty()), text: body })
}
