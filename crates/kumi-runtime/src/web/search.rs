//! Search through public services, DuckDuckGo fallback, or GitHub, and share cached requests.
pub use super::exa::Found;
use super::{
    exa::SearchOptions,
    free::{free_services, free_trouble, offline, wait_words, without_final_period, FreeFailure, FreeServices},
    github::search_github,
    html::{decode_entities, html_to_text},
    net::{decode_text, Method, WebClient, WebError, WebFailure, WebRequest, WebTrouble},
};
use futures::{
    future::{LocalBoxFuture, Shared},
    FutureExt,
};
use indexmap::IndexMap;
use kumi_common::{
    abort::{Signal, SignalExt},
    js::{
        number::to_string,
        string::{head, trim},
    },
    time::now_ms_f64,
};
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::{
    cell::{Cell, RefCell},
    future::Future,
    rc::Rc,
    sync::LazyLock,
};
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Searched {
    pub via: String,
    pub results: Vec<Found>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fell_back: Option<String>,
}
type Searching = Shared<LocalBoxFuture<'static, Result<Searched, WebFailure>>>;
struct Kept {
    at: f64,
    id: u64,
    searched: Searching,
}
pub struct SearchCache {
    kept: Rc<RefCell<IndexMap<String, Kept>>>,
    now: Rc<dyn Fn() -> f64>,
    keep_ms: f64,
    most: usize,
    next: Cell<u64>,
}
impl Default for SearchCache {
    fn default() -> Self {
        Self::new(Rc::new(now_ms_f64), 20.0 * 60_000.0, 64)
    }
}
impl SearchCache {
    pub fn new(now: Rc<dyn Fn() -> f64>, keep_ms: f64, most: usize) -> Self {
        Self { kept: Rc::new(RefCell::new(IndexMap::new())), now, keep_ms, most, next: Cell::new(0) }
    }
    pub fn search<F, Fut>(&self, key: String, run: F) -> Searching
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = Result<Searched, WebFailure>> + 'static,
    {
        let now = (self.now)();
        self.kept.borrow_mut().retain(|_, e| now - e.at < self.keep_ms);
        if let Some(held) = self.kept.borrow().get(&key) {
            return held.searched.clone();
        }
        let id = self.next.get();
        self.next.set(id.wrapping_add(1));
        let kept = Rc::downgrade(&self.kept);
        let cleanup_key = key.clone();
        let running = run();
        let searched = async move {
            let result = running.await;
            if result.is_err() {
                if let Some(kept) = kept.upgrade() {
                    let remove = kept.borrow().get(&cleanup_key).is_some_and(|e| e.id == id);
                    if remove {
                        kept.borrow_mut().shift_remove(&cleanup_key);
                    }
                }
            }
            result
        }
        .boxed_local()
        .shared();
        self.kept.borrow_mut().insert(key, Kept { at: now, id, searched: searched.clone() });
        while self.kept.borrow().len() > self.most {
            self.kept.borrow_mut().shift_remove_index(0);
        }
        searched
    }
}
pub const DUCKDUCKGO_URL: &str = "https://html.duckduckgo.com/html/";
pub fn parse_duck_duck_go(html: &str) -> Vec<Found> {
    static LINK: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r#"(?s)<a[^>]*class="result__a"[^>]*href="([^"]+)"[^>]*>(.*?)</a>"#).unwrap());
    static SNIPPET: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"(?s)<a[^>]*class="result__snippet"[^>]*>(.*?)</a>"#).unwrap());
    static SPACES: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\s+").unwrap());
    html.split("<div class=\"result results_links")
        .skip(1)
        .filter_map(|block| {
            if head(block, 200).contains("result--ad") {
                return None;
            }
            let link = LINK.captures(block)?;
            let mut address = decode_entities(&link[1]);
            if address.contains("duckduckgo.com/l/?") {
                let parsed = url::Url::parse(&if address.starts_with("//") { format!("https:{address}") } else { address.clone() }).ok()?;
                if let Some((_, value)) = parsed.query_pairs().find(|(k, _)| k == "uddg") {
                    address = value.into_owned();
                }
            }
            if !address.starts_with("http://") && !address.starts_with("https://") {
                return None;
            }
            let words = |html: &str| trim(&SPACES.replace_all(&html_to_text(html, DUCKDUCKGO_URL), " ")).to_string();
            let title = words(&link[2]);
            let snippet = SNIPPET.captures(block).map(|c| words(&c[1])).unwrap_or_default();
            Some(Found {
                title: if title.is_empty() { address.clone() } else { title },
                url: address,
                text: (!snippet.is_empty()).then_some(snippet),
                published: None,
            })
        })
        .collect()
}
async fn duck_duck_go(client: &dyn WebClient, query: &str, count: usize, signal: Option<Signal>) -> Result<Vec<Found>, WebFailure> {
    let response = client
        .fetch(
            DUCKDUCKGO_URL,
            WebRequest {
                method: Method::Post,
                headers: [
                    ("content-type".into(), "application/x-www-form-urlencoded".into()),
                    ("referer".into(), "https://html.duckduckgo.com/".into()),
                ]
                .into(),
                body: Some(url::form_urlencoded::Serializer::new(String::new()).extend_pairs([("q", query), ("b", "")]).finish()),
                max_bytes: Some(2 * 1024 * 1024),
                signal,
                ..Default::default()
            },
        )
        .await?;
    if response.status != 200 {
        return Err(WebError::with_status(
            if response.status == 202 {
                "DuckDuckGo asked Kumi to prove it's a person.".into()
            } else {
                format!("DuckDuckGo answered {}.", response.status)
            },
            response.status,
        )
        .into());
    }
    let html = decode_text(&response.body, response.charset.as_deref());
    let mut found = parse_duck_duck_go(&html);
    if found.is_empty() && ["anomaly", "captcha", "challenge"].iter().any(|s| html.to_lowercase().contains(s)) {
        return Err(WebError::new("DuckDuckGo asked Kumi to prove it's a person.").into());
    }
    found.truncate(count);
    Ok(found)
}
#[derive(Clone, Copy, PartialEq, Default)]
pub enum SearchWhere {
    #[default]
    Web,
    Github,
}
#[derive(Clone, Default)]
pub struct SearchWebOptions {
    pub about: Option<String>,
    pub count: usize,
    pub scope: SearchWhere,
    pub signal: Option<Signal>,
    pub services: Option<Rc<FreeServices>>,
}
pub async fn search_web(client: &dyn WebClient, query: &str, options: SearchWebOptions) -> Result<Searched, WebFailure> {
    if options.scope == SearchWhere::Github {
        let repos = search_github(client, query, options.count, options.signal).await?;
        return Ok(Searched {
            via: "GitHub".into(),
            fell_back: None,
            results: repos
                .into_iter()
                .map(|repo| {
                    let mut parts = Vec::new();
                    if let Some(d) = repo.description.filter(|s| !s.is_empty()) {
                        parts.push(d);
                    }
                    if let Some(stars) = repo.stars {
                        parts.push(format!("{} stars", to_string(stars)));
                    }
                    if let Some(lang) = repo.language.filter(|s| !s.is_empty()) {
                        parts.push(lang);
                    }
                    if let Some(updated) = repo.updated.filter(|s| !s.is_empty()) {
                        parts.push(format!("last changed {updated}"));
                    }
                    Found { title: repo.name, url: repo.url, text: Some(parts.join(" · ")), published: None }
                })
                .collect(),
        });
    }
    let services = options.services.unwrap_or_else(free_services);
    let search_options = SearchOptions { about: options.about, count: options.count, signal: options.signal.clone() };
    let found = services
        .first(
            "search",
            |service| {
                let search_options = search_options.clone();
                async move { service.search(client, query, search_options).await }
            },
            options.signal.clone(),
            Some(&|results: &Vec<Found>| results.is_empty()),
        )
        .await;
    let none = match found {
        Ok(found) => {
            let said = found.failures.iter().map(|f| without_final_period(&f.error.message)).collect::<Vec<_>>().join("; ");
            return Ok(Searched { via: found.service.name().into(), results: found.value, fell_back: (!said.is_empty()).then_some(said) });
        }
        Err(error) => {
            if let Some(signal) = &options.signal {
                signal.check()?;
            }
            match error {
                FreeFailure::NoService(none) => none,
                FreeFailure::Other(e) => return Err(e),
            }
        }
    };
    match duck_duck_go(client, query, options.count, options.signal.clone()).await {
        Ok(results) => {
            let said = free_trouble(&none);
            Ok(Searched {
                via: "DuckDuckGo".into(),
                results,
                fell_back: Some(if said.is_empty() { "the free search services are resting".into() } else { said }),
            })
        }
        Err(error) => {
            if let Some(signal) = &options.signal {
                signal.check()?;
            }
            if offline(&none, &error) {
                return Err(WebError::with_trouble(
                    format!(
                        "Kumi couldn't reach any search service ({} or DuckDuckGo): is this computer online?",
                        services.services.iter().map(|s| s.name()).collect::<Vec<_>>().join(", ")
                    ),
                    None,
                    WebTrouble::UNREACHABLE,
                )
                .into());
            }
            let said = [free_trouble(&none), error.to_string().strip_suffix('.').unwrap_or(&error.to_string()).to_string()]
                .into_iter()
                .filter(|s| !s.is_empty())
                .collect::<Vec<_>>()
                .join("; ");
            Err(WebError::new(format!(
                "Kumi couldn't search the web just now: {said}. Try again in {}.",
                none.back_in_ms.map_or_else(|| "a minute or two".into(), |wait| format!("about {}", wait_words(wait)))
            ))
            .into())
        }
    }
}
