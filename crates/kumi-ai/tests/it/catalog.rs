//! models.dev's catalog at run time: the copy kept in Kumi's cache folder is used at once, a newer one is asked for at
//! most once a day with its ETag, and nothing that goes wrong (no answer, a slow one, a refusal, or asking turned off)
//! leaves Kumi without a catalog or holds up a model list.
use async_trait::async_trait;
use kumi_ai::{
    auth::store::open_credential_store,
    error::LanguageModelError,
    http::{Fetch, FetchInit, Headers, Response},
    providers::{
        catalog::{self, RefreshOptions, Refreshed, DAY_MS},
        models::{list_models, ListOptions},
        ProviderId,
    },
};
use serde_json::{json, Value};
use std::{cell::RefCell, path::PathBuf, rc::Rc};

/// models.dev's answer, made up: OpenAI with one model the bundled snapshot doesn't have.
fn answer(model: &str) -> Value {
    json!({"openai":{"id":"openai","npm":"@ai-sdk/openai","env":["OPENAI_API_KEY"],"models":{model:{"id":model,"name":"Fixture Model","tool_call":false,"reasoning":true,"reasoning_options":[{"type":"effort","values":["minimal","high"]}],"limit":{"context":1000,"output":100}}}},"elsewhere":{"models":{"x":{"name":"X"}}}})
}
/// A models.dev that answers as `respond` says, keeping each request's headers.
struct ModelsDev {
    asked: RefCell<Vec<Headers>>,
    respond: Box<dyn Fn(&Headers) -> Option<Response>>,
}
#[async_trait(?Send)]
impl Fetch for ModelsDev {
    async fn fetch(&self, url: &str, init: FetchInit) -> Result<Response, LanguageModelError> {
        assert_eq!(url, "https://models.dev/api.json");
        self.asked.borrow_mut().push(init.headers.clone());
        match (self.respond)(&init.headers) {
            Some(response) => Ok(response),
            None => {
                // No answer: only Kumi's own wait ends it.
                init.signal.expect("the refresh waits a while only").cancelled().await;
                Err(LanguageModelError::other("aborted"))
            }
        }
    }
}
fn models_dev(respond: impl Fn(&Headers) -> Option<Response> + 'static) -> Rc<ModelsDev> {
    Rc::new(ModelsDev { asked: RefCell::new(vec![]), respond: Box::new(respond) })
}
fn options(cache: &PathBuf, fetch: Rc<ModelsDev>, now: f64) -> RefreshOptions {
    RefreshOptions { cache: cache.clone(), fetch, now, ask: true, wait_ms: 50 }
}
fn knows(model: &str) -> bool {
    catalog::current().model("openai", model).is_some()
}
fn kept(cache: &PathBuf) -> Value {
    serde_json::from_slice(&std::fs::read(cache).unwrap()).unwrap()
}

#[tokio::test(flavor = "current_thread")]
async fn a_new_catalog_is_kept_and_used_then_asked_for_again_a_day_later_with_its_etag() {
    catalog::forget();
    let folder = tempfile::tempdir().unwrap();
    let cache = folder.path().join("cache/models-catalog.json");
    let now = 1_800_000_000_000.;
    assert!(!knows("fixture-model-1"), "the bundled snapshot doesn't know it");
    // A 200: kept, cut to Kumi's providers and fields, and in use.
    let fresh = models_dev(|_| {
        let mut response = Response::json_response(200, answer("fixture-model-1"));
        response.headers.insert("etag".into(), "W/\"first\"".into());
        Some(response)
    });
    assert_eq!(catalog::refresh(options(&cache, fresh.clone(), now)).await, Refreshed::Fresh);
    assert!(fresh.asked.borrow()[0].get("if-none-match").is_none());
    assert!(fresh.asked.borrow()[0]["user-agent"].starts_with("kumi/"));
    assert!(knows("fixture-model-1"));
    let copy = kept(&cache);
    assert_eq!(copy["etag"], "W/\"first\"");
    assert_eq!(copy["checkedAt"].as_f64(), Some(now));
    assert_eq!(copy["catalog"].as_object().unwrap().keys().collect::<Vec<_>>(), ["openai"]);
    assert_eq!(
        copy["catalog"]["openai"]["models"]["fixture-model-1"],
        json!({"name":"Fixture Model","tool_call":false,"reasoning":true,"reasoning_options":[{"type":"effort","values":["minimal","high"]}],"limit":{"context":1000,"output":100}})
    );
    // Within the day: the copy kept, nothing asked.
    let never = models_dev(|_| panic!("asked within a day"));
    assert_eq!(catalog::refresh(options(&cache, never, now + DAY_MS - 1.)).await, Refreshed::Kept);
    // A day later, a 304: the copy is current; it says when it was checked.
    let same = models_dev(|headers| {
        assert_eq!(headers["if-none-match"], "W/\"first\"");
        Some(Response::text_response(304, ""))
    });
    assert_eq!(catalog::refresh(options(&cache, same, now + DAY_MS)).await, Refreshed::Unchanged);
    assert_eq!(kept(&cache)["checkedAt"].as_f64(), Some(now + DAY_MS));
    assert!(knows("fixture-model-1"));
}

#[tokio::test(flavor = "current_thread")]
async fn without_an_answer_the_copy_kept_is_used_or_else_the_snapshot_kumi_carries() {
    catalog::forget();
    let folder = tempfile::tempdir().unwrap();
    let cache = folder.path().join("models-catalog.json");
    let now = 1_800_000_000_000.;
    // Nothing kept, and models.dev refuses: the bundled snapshot stays in use.
    let refused = models_dev(|_| Some(Response::text_response(503, "down")));
    assert_eq!(catalog::refresh(options(&cache, refused, now)).await, Refreshed::Failed("models.dev answered HTTP 503".into()));
    assert!(!cache.exists());
    assert!(knows("gpt-5.5") && !knows("fixture-model-2"), "the bundled snapshot");
    // A copy kept from before: it's in use even when models.dev doesn't answer, or answers what isn't a catalog.
    std::fs::write(&cache, json!({"etag":"W/\"old\"","checkedAt":0,"catalog":answer("fixture-model-2")}).to_string()).unwrap();
    let silent = models_dev(|_| None);
    let said = catalog::refresh(options(&cache, silent, now)).await;
    assert_eq!(said, Refreshed::Failed("models.dev didn't answer within 50 ms".into()));
    assert!(knows("fixture-model-2"));
    let garbled = models_dev(|_| Some(Response::text_response(200, "<html>")));
    assert_eq!(catalog::refresh(options(&cache, garbled, now)).await, Refreshed::Failed("models.dev's answer wasn't JSON".into()));
    assert!(knows("fixture-model-2"));
    assert_eq!(kept(&cache)["etag"], "W/\"old\"", "a failure keeps the copy as it was");
}

#[tokio::test(flavor = "current_thread")]
async fn with_asking_turned_off_models_dev_is_never_asked_and_the_copy_kept_is_used() {
    catalog::forget();
    let folder = tempfile::tempdir().unwrap();
    let cache = folder.path().join("models-catalog.json");
    std::fs::write(&cache, json!({"checkedAt":0,"catalog":answer("fixture-model-3")}).to_string()).unwrap();
    let never = models_dev(|_| panic!("asked while modelCatalog is false"));
    let mut off = options(&cache, never.clone(), 1_800_000_000_000.);
    off.ask = false;
    assert_eq!(catalog::refresh(off).await, Refreshed::Kept);
    assert!(never.asked.borrow().is_empty());
    assert!(knows("fixture-model-3"));
}

/// OpenAI's list, from a server that answers at once.
struct Openai;
#[async_trait(?Send)]
impl Fetch for Openai {
    async fn fetch(&self, url: &str, _: FetchInit) -> Result<Response, LanguageModelError> {
        assert_eq!(url, "https://api.openai.com/v1/models");
        Ok(Response::json_response(200, json!({"data":[{"id":"gpt-5.5","created":2},{"id":"fixture-model-4","created":1}]})))
    }
}

#[tokio::test(flavor = "current_thread")]
async fn a_slow_models_dev_never_holds_up_a_model_list() {
    catalog::forget();
    tokio::task::LocalSet::new()
        .run_until(async {
            let folder = tempfile::tempdir().unwrap();
            let cache = folder.path().join("models-catalog.json");
            let silent = models_dev(|_| None);
            let mut slow = options(&cache, silent.clone(), 1_800_000_000_000.);
            slow.wait_ms = 60_000;
            let refreshing = tokio::task::spawn_local(catalog::refresh(slow));
            tokio::task::yield_now().await;
            assert_eq!(silent.asked.borrow().len(), 1, "models.dev is being asked");
            // The list comes at once, with the snapshot Kumi carries.
            let store = Rc::new(open_credential_store(folder.path().join("auth.json")));
            let env = [("OPENAI_API_KEY".to_string(), "key-fixture-value".to_string())].into();
            let listed = list_models(
                ProviderId::parse("openai").unwrap(),
                ListOptions { store, env: Some(env), fetch: Some(Rc::new(Openai)), signal: None },
            )
            .await
            .unwrap();
            assert_eq!(listed.iter().map(|model| model.name.as_str()).collect::<Vec<_>>(), ["GPT-5.5"]);
            assert!(!refreshing.is_finished());
            refreshing.abort();
        })
        .await;
}
