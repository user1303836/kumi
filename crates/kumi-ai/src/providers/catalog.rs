//! What models.dev (https://models.dev, MIT) knows of the models of the providers Kumi signs in to: the snapshot Kumi
//! carries (`data/models-catalog.json`, made by `scripts/models-catalog.py`), and a newer copy fetched at most once a
//! day, cut the same way and kept in Kumi's cache folder. A model's facts come from its provider's list first, then
//! from here (`facts`).
use super::{table, USER_AGENT};
use crate::http::{Fetch, FetchInit};
use kumi_common::{abort, js::json::file_text};
use serde::Serialize;
use serde_json::{json, Map, Value};
use std::{
    cell::RefCell,
    collections::BTreeMap,
    path::{Path, PathBuf},
    rc::Rc,
    sync::{Arc, LazyLock},
};

/// Where models.dev serves its whole catalog.
pub const SOURCE: &str = "https://models.dev/api.json";
/// How long a copy is good for before Kumi asks for a newer one.
pub const DAY_MS: f64 = 24. * 3600. * 1000.;
/// How long Kumi waits for models.dev's answer.
pub const WAIT_MS: u64 = 30_000;
/// The most of an answer Kumi reads: models.dev's catalog is about 6 MB.
const MOST_BYTES: usize = 64 * 1024 * 1024;

/// models.dev's providers by their names there, written as models.dev and the snapshot write them.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(transparent)]
pub struct Catalog {
    pub providers: BTreeMap<String, CatalogProvider>,
}
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct CatalogProvider {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub env: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub npm: Option<String>,
    pub models: BTreeMap<String, CatalogModel>,
}
/// What models.dev says of one model; what it leaves out is unknown.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct CatalogModel {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<bool>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub reasoning_options: Vec<ReasoningOption>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub modalities: Option<Modalities>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit: Option<Limit>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cost: Option<CatalogCost>,
    /// What it makes when it isn't a chat model (image, embedding, speech…); absent or "chat" for one that chats.
    #[serde(rename = "type", skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    /// Its own API, where it isn't its provider's (`npm` names the SDK, and so the request format).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider: Option<ModelRoute>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub release_date: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub structured_output: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub attachment: Option<bool>,
}
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct ReasoningOption {
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub values: Vec<String>,
}
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Modalities {
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub input: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub output: Vec<String>,
}
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Limit {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output: Option<f64>,
}
/// Dollars per million tokens.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct CatalogCost {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_read: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_write: Option<f64>,
}
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct ModelRoute {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub npm: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api: Option<String>,
}

fn text(value: &Value) -> Option<String> {
    value.as_str().filter(|text| !text.is_empty() && text.len() <= 1024).map(str::to_string)
}
fn texts(value: &Value) -> Vec<String> {
    value.as_array().into_iter().flatten().filter_map(text).take(64).collect()
}
fn number(value: &Value) -> Option<f64> {
    value.as_f64().filter(|n| n.is_finite() && *n >= 0.)
}
/// One model as models.dev describes it, each fact read on its own: a malformed one is left unknown.
fn model(value: &Value) -> CatalogModel {
    let object = |name: &str| value.get(name).filter(|v| v.is_object());
    CatalogModel {
        name: text(&value["name"]),
        tool_call: value["tool_call"].as_bool(),
        reasoning: value["reasoning"].as_bool(),
        reasoning_options: value["reasoning_options"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|option| Some(ReasoningOption { kind: text(&option["type"])?, values: texts(&option["values"]) }))
            .collect(),
        modalities: object("modalities").map(|m| Modalities { input: texts(&m["input"]), output: texts(&m["output"]) }),
        limit: object("limit").map(|l| Limit { context: number(&l["context"]), output: number(&l["output"]) }),
        cost: object("cost").map(|c| CatalogCost {
            input: number(&c["input"]),
            output: number(&c["output"]),
            cache_read: number(&c["cache_read"]),
            cache_write: number(&c["cache_write"]),
        }),
        kind: text(&value["type"]),
        provider: object("provider").map(|p| ModelRoute { npm: text(&p["npm"]), api: text(&p["api"]) }),
        status: text(&value["status"]),
        release_date: text(&value["release_date"]),
        structured_output: value["structured_output"].as_bool(),
        attachment: value["attachment"].as_bool(),
    }
}
impl Catalog {
    /// models.dev's catalog, or a copy cut from it, read for the providers the table names; whatever else it holds
    /// is left out.
    pub fn read(value: &Value) -> Catalog {
        let mut providers = BTreeMap::new();
        for name in table::rows().filter_map(|row| row.catalog) {
            let Some(provider) = value.get(name).filter(|p| p.is_object()) else { continue };
            let models = provider["models"]
                .as_object()
                .into_iter()
                .flatten()
                .filter(|(id, model)| !id.is_empty() && model.is_object())
                .map(|(id, value)| (id.clone(), model(value)))
                .collect();
            providers.insert(
                name.to_string(),
                CatalogProvider { api: text(&provider["api"]), env: texts(&provider["env"]), npm: text(&provider["npm"]), models },
            );
        }
        Catalog { providers }
    }
    /// What models.dev says of `model`, one of `provider`'s (models.dev's name for it).
    pub fn model(&self, provider: &str, model: &str) -> Option<&CatalogModel> {
        self.providers.get(provider)?.models.get(model)
    }
}
impl CatalogModel {
    /// Whether it chats: models.dev gives a non-chat model a type (image, embedding, speech, …).
    pub fn chats(&self) -> bool {
        self.kind.as_deref().is_none_or(|kind| kind == "chat")
    }
    /// The effort levels it takes, as models.dev names them.
    pub fn efforts(&self) -> Option<&[String]> {
        self.reasoning_options.iter().find(|option| option.kind == "effort").map(|option| option.values.as_slice())
    }
}

/// The snapshot this Kumi carries.
pub fn bundled() -> Arc<Catalog> {
    static BUNDLED: LazyLock<Arc<Catalog>> = LazyLock::new(|| {
        // data/models-catalog.json, compacted by build.rs: the binary carries no indentation.
        Arc::new(Catalog::read(
            &serde_json::from_str(include_str!(concat!(env!("OUT_DIR"), "/models-catalog.min.json"))).expect("the bundled catalog is JSON"),
        ))
    });
    BUNDLED.clone()
}
thread_local! {
    /// The newest copy Kumi has, on the thread that lists and asks the models (the session's), which refreshes it.
    static CURRENT: RefCell<Option<Arc<Catalog>>> = const { RefCell::new(None) };
}
/// The catalog in use: the newest copy Kumi has, or the one it carries.
pub fn current() -> Arc<Catalog> {
    CURRENT.with(|current| current.borrow().clone()).unwrap_or_else(bundled)
}
/// Puts `catalog` in use, from the next model list or request on.
pub fn install(catalog: Arc<Catalog>) {
    CURRENT.with(|current| *current.borrow_mut() = Some(catalog));
}
/// Puts the snapshot Kumi carries back in use (tests).
pub fn forget() {
    CURRENT.with(|current| *current.borrow_mut() = None);
}

/// What the cache folder keeps: the last copy, its ETag, and when models.dev was last asked.
struct Cached {
    etag: Option<String>,
    checked_at: f64,
    catalog: Catalog,
}
fn read_cached(path: &Path) -> Option<Cached> {
    let value: Value = serde_json::from_slice(&std::fs::read(path).ok()?).ok()?;
    let catalog = value.get("catalog").filter(|c| c.is_object())?;
    Some(Cached { etag: text(&value["etag"]), checked_at: number(&value["checkedAt"]).unwrap_or(0.), catalog: Catalog::read(catalog) })
}
fn write_cached(path: &Path, cached: &Cached) -> std::io::Result<()> {
    let folder = path.parent().unwrap_or(Path::new("."));
    std::fs::create_dir_all(folder)?;
    let mut value = Map::new();
    if let Some(etag) = &cached.etag {
        value.insert("etag".into(), json!(etag));
    }
    value.insert("checkedAt".into(), json!(cached.checked_at));
    value.insert("catalog".into(), serde_json::to_value(&cached.catalog).expect("a catalog serializes"));
    let temporary = folder.join(format!(".models-catalog.{}.tmp", std::process::id()));
    std::fs::write(&temporary, file_text(&Value::Object(value)))?;
    std::fs::rename(&temporary, path).inspect_err(|_| {
        let _ = std::fs::remove_file(&temporary);
    })
}

pub struct RefreshOptions {
    /// The copy Kumi keeps (`~/.kumi/cache/models-catalog.json`).
    pub cache: PathBuf,
    pub fetch: Rc<dyn Fetch>,
    /// Now, in epoch milliseconds.
    pub now: f64,
    /// false: models.dev isn't asked (settings.json's `modelCatalog`), and the copy kept is used as it is.
    pub ask: bool,
    /// How long to wait for models.dev's answer (`WAIT_MS`).
    pub wait_ms: u64,
}
/// What a refresh came to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refreshed {
    /// A newer catalog came, and is kept and in use.
    Fresh,
    /// models.dev said the copy kept is current (304).
    Unchanged,
    /// The copy kept was checked less than a day ago, or asking is off: it's in use, and nothing was asked.
    Kept,
    /// models.dev couldn't be asked, or its answer used: the copy kept is in use, or else the one Kumi carries.
    Failed(String),
}
/// Puts the copy kept in use, then, when it was last checked a day or more ago, asks models.dev for a newer one with
/// the copy's ETag. Never fails: whatever goes wrong leaves the copy kept in use, or the one Kumi carries.
pub async fn refresh(options: RefreshOptions) -> Refreshed {
    let cached = read_cached(&options.cache);
    if let Some(cached) = &cached {
        install(Arc::new(cached.catalog.clone()));
    }
    if !options.ask || cached.as_ref().is_some_and(|cached| options.now - cached.checked_at < DAY_MS && cached.checked_at <= options.now) {
        return Refreshed::Kept;
    }
    // Kumi says it's Kumi here too; models.dev isn't a model provider, so there's no plan in its answer to keep.
    let mut headers = crate::http::Headers::from([("accept".into(), "application/json".into()), ("user-agent".into(), USER_AGENT.clone())]);
    if let Some(etag) = cached.as_ref().and_then(|cached| cached.etag.clone()) {
        headers.insert("if-none-match".into(), etag);
    }
    let signal = abort::timeout(options.wait_ms);
    let answer = options.fetch.fetch(SOURCE, FetchInit { headers, signal: Some(signal.clone()), ..Default::default() }).await;
    let response = match answer {
        Ok(response) => response,
        Err(_) if signal.is_cancelled() => return Refreshed::Failed(format!("models.dev didn't answer within {} ms", options.wait_ms)),
        Err(error) => return Refreshed::Failed(error.to_string()),
    };
    if response.status == 304 {
        if let Some(mut cached) = cached {
            cached.checked_at = options.now;
            if let Err(error) = write_cached(&options.cache, &cached) {
                return Refreshed::Failed(error.to_string());
            }
        }
        return Refreshed::Unchanged;
    }
    if !response.ok() {
        return Refreshed::Failed(format!("models.dev answered HTTP {}", response.status));
    }
    let etag = response.headers.get("etag").cloned();
    let body = match response.text().await {
        Ok(body) if body.len() <= MOST_BYTES => body,
        Ok(_) => return Refreshed::Failed("models.dev's answer was too large".into()),
        Err(error) => return Refreshed::Failed(error.to_string()),
    };
    let Ok(value) = serde_json::from_str::<Value>(&body) else { return Refreshed::Failed("models.dev's answer wasn't JSON".into()) };
    let catalog = Catalog::read(&value);
    if catalog.providers.is_empty() {
        return Refreshed::Failed("models.dev's answer named none of Kumi's providers".into());
    }
    let fresh = Cached { etag, checked_at: options.now, catalog };
    let kept = write_cached(&options.cache, &fresh);
    install(Arc::new(fresh.catalog));
    match kept {
        Ok(()) => Refreshed::Fresh,
        Err(error) => Refreshed::Failed(format!("the new catalog is in use, but couldn't be kept: {error}")),
    }
}
