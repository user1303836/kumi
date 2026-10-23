//! Public, keyless readers and searches, taking turns and resting after service trouble.
use super::{
    exa::{exa_read, exa_search, Found, ReadText, SearchOptions},
    mcp_call::{mcp_tool, McpCall},
    net::{service_trouble, Method, WebClient, WebError, WebFailure, WebRequest, WebResponse},
};
use async_trait::async_trait;
use kumi_common::{
    abort::{Signal, SignalExt},
    js::{
        json::stringify,
        number::{round, to_string},
        string::{head, trim},
    },
    time::now_ms_f64,
};
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    cell::{Cell, RefCell},
    collections::HashMap,
    future::Future,
    rc::Rc,
    sync::LazyLock,
};
#[async_trait(?Send)]
pub trait FreeService {
    fn name(&self) -> &str;
    async fn search(&self, client: &dyn WebClient, query: &str, options: SearchOptions) -> Result<Vec<Found>, WebFailure>;
    async fn read(&self, client: &dyn WebClient, url: &str, signal: Option<Signal>) -> Result<ReadText, WebFailure>;
}
pub const PARALLEL_URL: &str = "https://search.parallel.ai/mcp";
pub const KEENABLE_URL: &str = "https://api.keenable.ai";
pub const FIRECRAWL_URL: &str = "https://api.firecrawl.dev";
static SESSION: LazyLock<String> = LazyLock::new(|| hex::encode(rand::random::<[u8; 16]>()));
#[derive(Clone, Copy)]
pub enum Builtin {
    Exa,
    Parallel,
    Keenable,
    Firecrawl,
}
pub const EXA: Builtin = Builtin::Exa;
pub const PARALLEL: Builtin = Builtin::Parallel;
pub const KEENABLE: Builtin = Builtin::Keenable;
pub const FIRECRAWL: Builtin = Builtin::Firecrawl;
fn words(value: &Value) -> &str {
    value.as_str().map(trim).unwrap_or("")
}
fn strings(value: &Value, join: &str) -> String {
    value
        .as_array()
        .map(|v| v.iter().filter_map(Value::as_str).filter(|s| !trim(s).is_empty()).collect::<Vec<_>>().join(join))
        .unwrap_or_default()
}
fn parsed(text: &str, service: &str) -> Result<Value, WebFailure> {
    let data: Value = serde_json::from_str(text).map_err(|_| WebError::new(format!("{service} answered in a way Kumi doesn't follow.")))?;
    Ok(if data.is_object() { data } else { json!({}) })
}
fn answer(response: WebResponse, service: &str) -> Result<Value, WebFailure> {
    if response.status != 200 {
        return Err(service_trouble(service, &response).into());
    }
    parsed(&String::from_utf8_lossy(&response.body), service)
}
fn results(items: &Value, count: usize, said: impl Fn(&Value) -> String) -> Vec<Found> {
    items
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter_map(|item| {
                    let url = words(&item["url"]);
                    if !url.starts_with("http://") && !url.starts_with("https://") {
                        return None;
                    }
                    let text = said(item);
                    let published = words(
                        item.get("publish_date")
                            .filter(|v| !v.is_null())
                            .or_else(|| item.get("published_date").filter(|v| !v.is_null()))
                            .or_else(|| item.get("publishedDate"))
                            .unwrap_or(&Value::Null),
                    );
                    let title = words(&item["title"]);
                    Some(Found {
                        title: if title.is_empty() { url } else { title }.into(),
                        url: url.into(),
                        published: (!published.is_empty()).then(|| head(published, 10)),
                        text: (!text.is_empty()).then_some(text),
                    })
                })
                .take(count)
                .collect()
        })
        .unwrap_or_default()
}
fn page(service: &str, title: &Value, text: String) -> Result<ReadText, WebFailure> {
    if text.is_empty() {
        return Err(WebError::new(format!("{service} found no text there.")).into());
    }
    Ok(ReadText { title: (!words(title).is_empty()).then(|| words(title).into()), text })
}
#[async_trait(?Send)]
impl FreeService for Builtin {
    fn name(&self) -> &str {
        match self {
            Self::Exa => "Exa",
            Self::Parallel => "Parallel",
            Self::Keenable => "Keenable",
            Self::Firecrawl => "Firecrawl",
        }
    }
    async fn search(&self, client: &dyn WebClient, query: &str, options: SearchOptions) -> Result<Vec<Found>, WebFailure> {
        match self {
            Self::Exa => exa_search(client, query, options).await,
            Self::Parallel => {
                let text=mcp_tool(client,PARALLEL_URL,"Parallel","web_search",json!({"objective":head(options.about.as_deref().filter(|s|!s.is_empty()).unwrap_or(query),1000),"search_queries":[query],"session_id":*SESSION}).as_object().unwrap().clone(),McpCall{timeout_ms:20_000,signal:options.signal,..Default::default()}).await?;
                Ok(results(&parsed(&text, "Parallel")?["results"], options.count, |item| strings(&item["excerpts"], "\n")))
            }
            Self::Keenable | Self::Firecrawl => {
                let keen = matches!(self, Self::Keenable);
                let mut headers = std::collections::BTreeMap::new();
                if keen {
                    headers.insert("x-keenable-title".into(), "kumi".into());
                }
                headers.insert("content-type".into(), "application/json".into());
                let response = client
                    .fetch(
                        &format!(
                            "{}{}",
                            if keen { KEENABLE_URL } else { FIRECRAWL_URL },
                            if keen { "/v1/search/public" } else { "/v2/search" }
                        ),
                        WebRequest {
                            method: Method::Post,
                            headers,
                            body: Some(stringify(&if keen {
                                json!({"query":query,"max_results":options.count})
                            } else {
                                json!({"query":query,"limit":options.count})
                            })),
                            timeout_ms: Some(20_000),
                            max_bytes: Some(4 * 1024 * 1024),
                            signal: options.signal,
                            ..Default::default()
                        },
                    )
                    .await?;
                let data = answer(response, self.name())?;
                let inner = &data["data"];
                let list = if keen {
                    &data["results"]
                } else if inner.is_array() {
                    inner
                } else {
                    inner
                        .get("web")
                        .filter(|v| !v.is_null())
                        .or_else(|| inner.get("results").filter(|v| !v.is_null()))
                        .or_else(|| data.get("web").filter(|v| !v.is_null()))
                        .or_else(|| data.get("results"))
                        .unwrap_or(&Value::Null)
                };
                Ok(results(list, options.count, |item| {
                    let first = words(&item[if keen { "snippet" } else { "description" }]);
                    if first.is_empty() { words(&item[if keen { "description" } else { "snippet" }]) } else { first }.into()
                }))
            }
        }
    }
    async fn read(&self, client: &dyn WebClient, url: &str, signal: Option<Signal>) -> Result<ReadText, WebFailure> {
        match self {
            Self::Exa => exa_read(client, url, signal).await,
            Self::Parallel => {
                let text = mcp_tool(
                    client,
                    PARALLEL_URL,
                    "Parallel",
                    "web_fetch",
                    json!({"urls":[url],"full_content":true,"session_id":*SESSION}).as_object().unwrap().clone(),
                    McpCall { timeout_ms: 60_000, signal, ..Default::default() },
                )
                .await?;
                let data = parsed(&text, "Parallel")?;
                let result = &data["results"][0];
                let mut body = words(&result["full_content"]).to_string();
                if body.is_empty() {
                    body = words(&result["content"]).into();
                }
                if body.is_empty() {
                    body = trim(&strings(&result["excerpts"], "\n\n")).into();
                }
                if body.is_empty() {
                    static BAD: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[^a-zA-Z0-9_ .-]").unwrap());
                    let kind = head(&BAD.replace_all(words(&data["errors"][0]["error_type"]), ""), 60);
                    return Err(WebError::new(format!(
                        "Parallel couldn't read it{}.",
                        if kind.is_empty() { String::new() } else { format!(" ({kind})") }
                    ))
                    .into());
                }
                page("Parallel", &result["title"], body)
            }
            Self::Keenable => {
                let query = url::form_urlencoded::Serializer::new(String::new())
                    .extend_pairs([("url", url), ("live", "true"), ("max_chars", "400000")])
                    .finish();
                let response = client
                    .fetch(
                        &format!("{KEENABLE_URL}/v1/fetch/public?{query}"),
                        WebRequest {
                            headers: [("x-keenable-title".into(), "kumi".into())].into(),
                            timeout_ms: Some(60_000),
                            max_bytes: Some(8 * 1024 * 1024),
                            signal,
                            ..Default::default()
                        },
                    )
                    .await?;
                let data = answer(response, "Keenable")?;
                page("Keenable", &data["title"], words(&data["content"]).into())
            }
            Self::Firecrawl => {
                let response = client
                    .fetch(
                        &format!("{FIRECRAWL_URL}/v2/scrape"),
                        WebRequest {
                            method: Method::Post,
                            headers: [("content-type".into(), "application/json".into())].into(),
                            body: Some(stringify(&json!({"url":url,"formats":["markdown"]}))),
                            timeout_ms: Some(60_000),
                            max_bytes: Some(8 * 1024 * 1024),
                            signal,
                            ..Default::default()
                        },
                    )
                    .await?;
                let data = answer(response, "Firecrawl")?;
                page("Firecrawl", &data["data"]["metadata"]["title"], words(&data["data"]["markdown"]).into())
            }
        }
    }
}
#[derive(Clone, Debug)]
pub struct Failure {
    pub service: String,
    pub error: WebError,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Resting {
    pub service: String,
    pub busy: bool,
}
#[derive(Clone, Debug)]
pub struct NoFreeService {
    pub failures: Vec<Failure>,
    pub resting: Vec<Resting>,
    pub back_in_ms: Option<f64>,
}
impl std::fmt::Display for NoFreeService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let messages = self
            .failures
            .iter()
            .map(|v| v.error.message.clone())
            .chain(self.resting.iter().map(|v| format!("{} is resting.", v.service)))
            .collect::<Vec<_>>()
            .join(" ");
        f.write_str(if messages.is_empty() { "No free service answered." } else { &messages })
    }
}
impl std::error::Error for NoFreeService {}
#[derive(Clone, Debug)]
pub enum FreeFailure {
    NoService(NoFreeService),
    Other(WebFailure),
}
impl std::fmt::Display for FreeFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoService(e) => e.fmt(f),
            Self::Other(e) => e.fmt(f),
        }
    }
}
impl std::error::Error for FreeFailure {}
impl From<WebFailure> for FreeFailure {
    fn from(e: WebFailure) -> Self {
        Self::Other(e)
    }
}
#[derive(Clone)]
struct Rest {
    until: f64,
    busy: bool,
}
pub struct FreeServices {
    pub services: Vec<Rc<dyn FreeService>>,
    cursor: Cell<usize>,
    rests: RefCell<HashMap<String, Rest>>,
    now: Rc<dyn Fn() -> f64>,
}
pub struct First<T> {
    pub value: T,
    pub service: Rc<dyn FreeService>,
    pub failures: Vec<Failure>,
}
impl Default for FreeServices {
    fn default() -> Self {
        Self::new(
            vec![Rc::new(EXA), Rc::new(PARALLEL), Rc::new(KEENABLE), Rc::new(FIRECRAWL)],
            Rc::new(now_ms_f64),
            rand::random_range(0..4),
        )
    }
}
impl FreeServices {
    pub fn new(services: Vec<Rc<dyn FreeService>>, now: Rc<dyn Fn() -> f64>, start: usize) -> Self {
        assert!(!services.is_empty(), "at least one free service is required");
        let cursor = Cell::new(start % services.len());
        Self { services, cursor, rests: RefCell::new(HashMap::new()), now }
    }
    pub async fn first<T, F, Fut>(
        &self,
        kind: &str,
        mut run: F,
        signal: Option<Signal>,
        empty: Option<&dyn Fn(&T) -> bool>,
    ) -> Result<First<T>, FreeFailure>
    where
        F: FnMut(Rc<dyn FreeService>) -> Fut,
        Fut: Future<Output = Result<T, WebFailure>>,
    {
        let start = self.cursor.get();
        self.cursor.set((start + 1) % self.services.len());
        let now = (self.now)();
        let order: Vec<_> = self.services[start..].iter().chain(&self.services[..start]).cloned().collect();
        let key = |service: &Rc<dyn FreeService>| format!("{kind} {}", service.name());
        let resting: Vec<_> = order.iter().filter(|s| self.rests.borrow().get(&key(s)).is_some_and(|r| r.until > now)).cloned().collect();
        let mut failures = Vec::new();
        let mut empty_answer = None;
        for service in order.iter().filter(|s| !resting.iter().any(|r| Rc::ptr_eq(r, s))) {
            match run(service.clone()).await {
                Ok(value) => {
                    if empty.is_some_and(|is_empty| is_empty(&value)) {
                        if empty_answer.is_none() {
                            empty_answer = Some((value, service.clone()));
                        }
                        continue;
                    }
                    return Ok(First { value, service: service.clone(), failures });
                }
                Err(error) => {
                    if let Some(signal) = &signal {
                        signal.check().map_err(|_| FreeFailure::Other(WebFailure::Aborted))?;
                    }
                    let trouble = match error {
                        WebFailure::Web(error) => error,
                        WebFailure::Aborted => WebError::new(format!("{} failed: This operation was aborted.", service.name())),
                    };
                    let wait = if trouble.trouble.busy {
                        trouble.trouble.retry_after_ms.unwrap_or(120_000.0).min(86_400_000.0)
                    } else if trouble.trouble.unreachable {
                        60_000.0
                    } else {
                        0.0
                    };
                    if wait > 0.0 {
                        self.rests.borrow_mut().insert(key(service), Rest { until: (self.now)() + wait, busy: trouble.trouble.busy });
                    }
                    failures.push(Failure { service: service.name().into(), error: trouble });
                }
            }
        }
        if let Some((value, service)) = empty_answer {
            return Ok(First { value, service, failures });
        }
        let later = (self.now)();
        let back = order
            .iter()
            .filter_map(|s| self.rests.borrow().get(&key(s)).map(|r| r.until - later))
            .filter(|wait| *wait > 0.0)
            .reduce(f64::min);
        Err(FreeFailure::NoService(NoFreeService {
            failures,
            resting: resting
                .iter()
                .map(|s| Resting { service: s.name().into(), busy: self.rests.borrow().get(&key(s)).is_some_and(|r| r.busy) })
                .collect(),
            back_in_ms: back,
        }))
    }
}
thread_local! {static FREE_SERVICES:Rc<FreeServices>=Rc::new(FreeServices::default());}
pub fn free_services() -> Rc<FreeServices> {
    FREE_SERVICES.with(Clone::clone)
}
fn list(names: &[&str]) -> String {
    if names.len() < 2 {
        names.join("")
    } else {
        format!("{} and {}", names[..names.len() - 1].join(", "), names[names.len() - 1])
    }
}
pub fn wait_words(ms: f64) -> String {
    if ms < 90_000.0 {
        "a minute".into()
    } else if ms < 90.0 * 60_000.0 {
        format!("{} minutes", to_string((ms / 60_000.0).ceil()))
    } else {
        format!("{} hours", to_string(round(ms / 3_600_000.0)))
    }
}
pub fn without_final_period(message: &str) -> &str {
    message.strip_suffix(" for now.").or_else(|| message.strip_suffix('.')).unwrap_or(message)
}
pub fn free_trouble(none: &NoFreeService) -> String {
    let busy: Vec<_> = none.resting.iter().filter(|r| r.busy).map(|r| r.service.as_str()).collect();
    let down: Vec<_> = none.resting.iter().filter(|r| !r.busy).map(|r| r.service.as_str()).collect();
    let mut said: Vec<_> = none.failures.iter().map(|f| without_final_period(&f.error.message).to_string()).collect();
    if !busy.is_empty() {
        said.push(format!("{} {} had too many requests from here", list(&busy), if busy.len() > 1 { "have" } else { "has" }));
    }
    if !down.is_empty() {
        said.push(format!("{} didn't answer a moment ago", list(&down)));
    }
    said.join("; ")
}
pub fn offline(none: &NoFreeService, last: &WebFailure) -> bool {
    !none.failures.is_empty()
        && none.resting.iter().all(|r| !r.busy)
        && none
            .failures
            .iter()
            .map(|f| Some(&f.error))
            .chain([last.web()])
            .all(|e| e.is_some_and(|e| e.trouble.unreachable && e.status.is_none()))
}
