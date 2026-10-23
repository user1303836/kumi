//! One public MCP tool asked over plain HTTP, without a session.
use super::net::{busy_words, js_string, service_trouble, Method, WebClient, WebError, WebFailure, WebRequest, WebTrouble};
use kumi_common::{
    abort::Signal,
    js::{
        json::stringify,
        string::{head, trim},
    },
};
use regex::Regex;
use serde_json::{json, Map, Value};
use std::{rc::Rc, sync::LazyLock};
#[derive(Clone)]
pub struct McpCall {
    pub signal: Option<Signal>,
    pub timeout_ms: u64,
    pub explain: Option<Rc<dyn Fn(&str) -> Option<String>>>,
}
impl Default for McpCall {
    fn default() -> Self {
        Self { signal: None, timeout_ms: 20_000, explain: None }
    }
}
pub async fn mcp_tool(
    client: &dyn WebClient,
    endpoint: &str,
    service: &str,
    tool: &str,
    args: Map<String, Value>,
    call: McpCall,
) -> Result<String, WebFailure> {
    let response = client
        .fetch(
            endpoint,
            WebRequest {
                method: Method::Post,
                headers: [
                    ("content-type".into(), "application/json".into()),
                    ("accept".into(), "application/json, text/event-stream".into()),
                ]
                .into(),
                body: Some(stringify(&json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":tool,"arguments":args}}))),
                timeout_ms: Some(call.timeout_ms),
                max_bytes: Some(8 * 1024 * 1024),
                signal: call.signal,
                ..Default::default()
            },
        )
        .await?;
    if response.status != 200 {
        return Err(service_trouble(service, &response).into());
    }
    let raw = String::from_utf8_lossy(&response.body);
    static MESSAGE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#""(result|error)""#).unwrap());
    let message = if response.content_type == "text/event-stream" {
        raw.split(['\r', '\n']).filter_map(|s| s.strip_prefix("data:")).map(trim).filter(|s| MESSAGE.is_match(s)).last().unwrap_or("")
    } else {
        &raw
    };
    let parsed: Value =
        serde_json::from_str(message).map_err(|_| WebError::new(format!("{service} answered in a way Kumi doesn't follow.")))?;
    let busy = || WebError::with_trouble(format!("{service} has had too many requests from here for now."), None, WebTrouble::BUSY);
    if parsed.get("error").is_some_and(|v| !v.is_null() && v != false) {
        let message = parsed["error"].get("message").filter(|v| !v.is_null()).map_or_else(|| "an error".into(), js_string);
        if busy_words(&message) {
            return Err(busy().into());
        }
        return Err(WebError::new(format!("{service} refused: {}", head(&message, 200))).into());
    }
    let text = parsed["result"]["content"]
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter(|item| item["type"] == "text")
                .filter_map(|item| item["text"].as_str())
                .filter(|s| !s.is_empty())
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default();
    if parsed["result"]["isError"].as_bool() == Some(true) {
        if busy_words(&text) {
            return Err(busy().into());
        }
        static PREFIX: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^Error[^:]*:\s*").unwrap());
        let said = call.explain.as_ref().and_then(|explain| explain(&text)).unwrap_or_else(|| {
            let text = head(&PREFIX.replace(&text, ""), 200);
            if text.is_empty() {
                "failed".into()
            } else {
                text
            }
        });
        return Err(WebError::new(format!("{service} {said}.")).into());
    }
    Ok(text)
}
