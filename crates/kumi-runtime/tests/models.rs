use async_trait::async_trait;
use kumi_common::time::now_ms;
use kumi_runtime::{
    ai::{
        error::LanguageModelError,
        http::{Fetch, FetchInit, Headers, Response},
    },
    auth::store::{open_credential_store, Credential, FileCredentialStore, OAuthCredential},
    core::errors::{FailureKind, RuntimeError},
    providers::{
        models::{check_api_key, list_models, ApiKeyCheck, ListOptions, Transport},
        Effort, ProviderId,
    },
};
use serde_json::{json, Value};
use std::{cell::RefCell, rc::Rc};
struct Server {
    seen: Rc<RefCell<Vec<(String, Headers)>>>,
    respond: Rc<dyn Fn(&str, &Headers) -> Result<Response, LanguageModelError>>,
}
#[async_trait(?Send)]
impl Fetch for Server {
    async fn fetch(&self, url: &str, init: FetchInit) -> Result<Response, LanguageModelError> {
        self.seen.borrow_mut().push((url.into(), init.headers.clone()));
        (self.respond)(url, &init.headers)
    }
}
fn server(
    respond: impl Fn(&str, &Headers) -> Result<Response, LanguageModelError> + 'static,
) -> (Rc<dyn Fetch>, Rc<RefCell<Vec<(String, Headers)>>>) {
    let seen = Rc::new(RefCell::new(vec![]));
    (Rc::new(Server { seen: seen.clone(), respond: Rc::new(respond) }), seen)
}
fn store() -> (tempfile::TempDir, Rc<FileCredentialStore>) {
    let dir = tempfile::tempdir().unwrap();
    let store = Rc::new(open_credential_store(dir.path().join("auth.json")));
    (dir, store)
}
fn opts(store: Rc<FileCredentialStore>, fetch: Rc<dyn Fetch>) -> ListOptions {
    ListOptions { store, fetch: Some(fetch), env: None, signal: None }
}
#[tokio::test]
async fn chatgpt_catalog_keeps_priority_zero_and_its_supported_effort_levels() {
    let (_dir, store) = store();
    store
        .update_with("openai-codex", |_| async {
            Ok(Some(Credential::Oauth(OAuthCredential {
                access: "access".into(),
                refresh: "refresh-1".into(),
                expires: now_ms() as f64 + 86400000.,
                account_id: "acct-1".into(),
            })))
        })
        .await
        .unwrap();
    let (fetch, seen) = server(|url, _| {
        assert_eq!(url, "https://chatgpt.com/backend-api/codex/models?client_version=1.0.0");
        Ok(Response::json_response(
            200,
            json!({"models":[
                {"slug":"gpt-6-luna","display_name":"GPT-6 Luna","description":"Fast","priority":3,"visibility":"list","default_reasoning_level":"low","supported_reasoning_levels":[{"effort":"low","description":"Quick"},{"effort":"medium"}]},
                {"slug":"gpt-reserve","display_name":"Reserve","priority":0,"visibility":"hide"},
                {"slug":"gpt-6-astra","display_name":"GPT-6 Astra","priority":1,"visibility":"list","default_reasoning_level":"medium","supported_reasoning_levels":[{"effort":"low"},{"effort":"medium"},{"effort":"high"},{"effort":"xhigh"},{"effort":"turbo"}]},
                {"slug":"gpt-6-nova","display_name":"GPT-6 Nova","priority":0,"visibility":"list"},
                {"slug":"gpt-6-draft","display_name":"GPT-6 Draft","visibility":"list"}
            ]}),
        ))
    });
    let models = list_models(ProviderId::OpenaiCodex, opts(store, fetch)).await.unwrap();
    assert_eq!(
        models.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(),
        ["openai-codex/gpt-6-nova", "openai-codex/gpt-6-astra", "openai-codex/gpt-6-luna", "openai-codex/gpt-6-draft"]
    );
    assert_eq!(models[1].efforts.iter().map(|e| e.effort).collect::<Vec<_>>(), [Effort::Low, Effort::Medium, Effort::High, Effort::Xhigh]);
    assert_eq!(models[1].default_effort, Some(Effort::Medium));
    assert_eq!(
        serde_json::to_value(&models[2]).unwrap(),
        json!({"id":"openai-codex/gpt-6-luna","provider":"openai-codex","model":"gpt-6-luna","name":"GPT-6 Luna","description":"Fast","efforts":[{"effort":"low","description":"Quick"},{"effort":"medium"}],"defaultEffort":"low"})
    );
    let seen = seen.borrow();
    assert_eq!(seen[0].1["chatgpt-account-id"], "acct-1");
    assert_eq!(seen[0].1["originator"], "kumi");
    assert!(seen[0].1["user-agent"].starts_with("kumi/"));
}
#[tokio::test]
async fn saved_or_environment_keys_list_models_with_provider_capabilities_and_filters() {
    let (_dir, store) = store();
    let (fetch, _) = server(|_, _| panic!("missing credentials must fail before network"));
    assert!(
        matches!(list_models(ProviderId::Anthropic,opts(store.clone(),fetch)).await,Err(RuntimeError::Kumi(e))if e.kind==FailureKind::Auth&&e.provider.as_deref()==Some("anthropic"))
    );
    store.update_with("anthropic", |_| async { Ok(Some(Credential::ApiKey { key: "sk-ant-saved-0000".into() })) }).await.unwrap();
    let (fetch, seen) = server(|url, headers| {
        Ok(match url {
            "https://api.anthropic.com/v1/models?limit=100" if headers["x-api-key"] == "sk-ant-saved-0000" => Response::json_response(
                200,
                json!({"data":[
            {"id":"claude-sonnet-5-5","display_name":"Claude Sonnet 5.5","capabilities":{"effort":{"supported":true,"low":{"supported":true},"medium":{"supported":true},"high":{"supported":true},"xhigh":{"supported":true},"max":{"supported":true}}}},
            {"id":"claude-opus-4-5","display_name":"Claude Opus 4.5","capabilities":{"effort":{"supported":true,"low":{"supported":true},"medium":{"supported":true},"high":{"supported":true}}}},
            {"id":"claude-haiku-4-5","display_name":"Claude Haiku 4.5","capabilities":{"effort":{"supported":false}}},
            {"id":"claude-fable-5-1","display_name":"Claude Fable 5.1"}]}),
            ),
            "https://api.anthropic.com/v1/models?limit=100" => {
                Response::json_response(401, json!({"error":{"message":"invalid x-api-key"}}))
            }
            "https://api.openai.com/v1/models" => Response::json_response(
                200,
                json!({"data":[{"id":"gpt-6-luna","created":2},{"id":"gpt-realtime-6","created":3},{"id":"text-embedding-4","created":4},{"id":"o5-mini","created":1},{"id":"gpt-6-astra","created":5}]}),
            ),
            "https://opencode.ai/zen/v1/models" => {
                Response::json_response(200, json!({"data":[{"id":"claude-sonnet-5-5"},{"id":"gemini-3.5-pro"},{"id":"kimi-k2.6"}]}))
            }
            _ => Response::text_response(404, "not found"),
        })
    });
    let models = list_models(ProviderId::Anthropic, opts(store.clone(), fetch.clone())).await.unwrap();
    assert_eq!(
        models
            .iter()
            .map(|m| (m.name.as_str(), m.efforts.iter().map(|e| e.effort.as_str()).collect::<Vec<_>>().join(" ")))
            .collect::<Vec<_>>(),
        [
            ("Claude Sonnet 5.5", "low medium high xhigh max".into()),
            ("Claude Opus 4.5", "low medium high".into()),
            ("Claude Haiku 4.5", "".into()),
            ("Claude Fable 5.1", "low medium high xhigh max".into())
        ]
    );
    let mut options = opts(store.clone(), fetch.clone());
    options.env = Some([("OPENAI_API_KEY".into(), "sk-env-openai".into())].into());
    let models = list_models(ProviderId::Openai, options).await.unwrap();
    assert_eq!(models.iter().map(|m| m.model.as_str()).collect::<Vec<_>>(), ["gpt-6-astra", "gpt-6-luna", "o5-mini"]);
    assert_eq!(models[0].efforts.len(), 5);
    assert_eq!(models[2].efforts.len(), 3);
    let mut options = opts(store.clone(), fetch.clone());
    options.env = Some([("OPENCODE_API_KEY".into(), "oc-env-key".into())].into());
    let models = list_models(ProviderId::Opencode, options).await.unwrap();
    assert_eq!(models.iter().map(|m| m.model.as_str()).collect::<Vec<_>>(), ["claude-sonnet-5-5", "kimi-k2.6"]);
    let mut options = opts(store.clone(), fetch.clone());
    options.env = Some([("ANTHROPIC_API_KEY".into(), "sk-ant-env-1111".into())].into());
    list_models(ProviderId::Anthropic, options).await.unwrap();
    assert_eq!(seen.borrow().last().unwrap().1["x-api-key"], "sk-ant-saved-0000");
    store.update_with("anthropic", |_| async { Ok(None) }).await.unwrap();
    let mut options = opts(store, fetch);
    options.env = Some([("ANTHROPIC_API_KEY".into(), "sk-ant-env-1111".into())].into());
    assert!(list_models(ProviderId::Anthropic, options).await.is_err());
    assert_eq!(seen.borrow().last().unwrap().1["x-api-key"], "sk-ant-env-1111");
}
#[tokio::test]
async fn keys_are_checked_before_saving_and_transport_failure_is_not_rejection() {
    let (fetch, _) = server(|_, headers| {
        Ok(if headers["x-api-key"] == "good-key-123" {
            Response::json_response(200, json!({"data":[]}))
        } else {
            Response::json_response(401, json!({}))
        })
    });
    assert_eq!(
        check_api_key(ProviderId::Anthropic, "good-key-123", Transport { fetch: Some(fetch.clone()), signal: None }).await,
        ApiKeyCheck::Ok
    );
    assert_eq!(
        check_api_key(ProviderId::Anthropic, "bad-key-456", Transport { fetch: Some(fetch), signal: None }).await,
        ApiKeyCheck::Refused
    );
    let (fetch, _) = server(|_, _| Err(LanguageModelError::other("fetch failed")));
    assert_eq!(
        check_api_key(ProviderId::Anthropic, "good-key-123", Transport { fetch: Some(fetch), signal: None }).await,
        ApiKeyCheck::Unreachable
    );
    let (fetch, _) = server(|_, _| Ok(Response::json_response(500, json!({}))));
    assert_eq!(
        check_api_key(ProviderId::Anthropic, "good-key-123", Transport { fetch: Some(fetch), signal: None }).await,
        ApiKeyCheck::Unreachable
    );
}
#[tokio::test]
async fn catalogs_handle_malformed_rows_and_bound_names_using_js_string_lengths() {
    let (_dir, store) = store();
    store
        .update_with("openai-codex", |_| async {
            Ok(Some(Credential::Oauth(OAuthCredential {
                access: "access".into(),
                refresh: "refresh".into(),
                account_id: "account".into(),
                expires: now_ms() as f64 + 86400000.,
            })))
        })
        .await
        .unwrap();
    let (fetch, _) = server(|_, _| {
        Ok(Response::json_response(
            200,
            json!({"models":[null,[],5,{"slug":1},{"slug":"last"},{"slug":"first","priority":" -1 ","display_name":format!("  {}  ","😀".repeat(31)),"description":"  description  "},{"slug":"middle","priority":"0x10"}]}),
        ))
    });
    let models = list_models(ProviderId::OpenaiCodex, opts(store, fetch)).await.unwrap();
    assert_eq!(models.iter().map(|m| m.model.as_str()).collect::<Vec<_>>(), ["first", "middle", "last"]);
    assert_eq!(models[0].name, "😀".repeat(30));
    assert_eq!(models[0].description.as_deref(), Some("description"));
    let (fetch, _) = server(|_, _| Ok(Response::json_response(200, Value::Null)));
    assert_eq!(check_api_key(ProviderId::Openai, "key", Transport { fetch: Some(fetch), signal: None }).await, ApiKeyCheck::Ok);
}
