//! What some providers do their own way: OpenRouter's attribution on every request and Anthropic's prompt cache for
//! its Claude models, an empty balance said as one (Venice, OpenRouter), Gemini's 400 for a key it doesn't know, and
//! the one Gemini key chat and listening share.
use async_trait::async_trait;
use kumi_ai::{
    auth::store::{open_credential_store, Credential},
    binding::ModelRequest,
    error::{ApiCallError, LanguageModelError},
    failure::describe_failure,
    http::{Fetch, FetchInit, Headers, Response},
    providers::{
        api_key_for,
        models::{check_api_key, list_models, ApiKeyCheck, ListOptions, Transport},
        resolve_model, KeySource, ProviderId, ResolveModelOptions,
    },
    types::{FunctionTool, Message},
};
use kumi_types::errors::FailureKind;
use serde_json::{json, Value};
use std::{cell::RefCell, collections::HashMap, rc::Rc};

const KEY: &str = "key-fixture-value";
struct Server {
    seen: RefCell<Vec<(String, Headers, Value)>>,
    answer: Box<dyn Fn(&str) -> Response>,
}
#[async_trait(?Send)]
impl Fetch for Server {
    async fn fetch(&self, url: &str, init: FetchInit) -> Result<Response, LanguageModelError> {
        let body = init.body.as_deref().map(|body| serde_json::from_str(body).unwrap()).unwrap_or(Value::Null);
        self.seen.borrow_mut().push((url.into(), init.headers, body));
        Ok((self.answer)(url))
    }
}
fn server(answer: impl Fn(&str) -> Response + 'static) -> Rc<Server> {
    Rc::new(Server { seen: RefCell::new(vec![]), answer: Box::new(answer) })
}
fn provider(id: &str) -> ProviderId {
    ProviderId::parse(id).unwrap()
}
fn refused(status: u16, body: Value) -> LanguageModelError {
    let mut error = ApiCallError::new(format!("HTTP {status}"), "https://example.test/chat/completions", None, Some(status));
    error.response_body = Some(body.to_string());
    LanguageModelError::ApiCall(error)
}

#[tokio::test]
async fn openrouter_says_who_asks_on_every_request_and_asks_claude_for_its_prompt_cache() {
    let folder = tempfile::tempdir().unwrap();
    let store = Rc::new(open_credential_store(folder.path().join("auth.json")));
    let env: HashMap<String, String> = [("OPENROUTER_API_KEY".into(), KEY.into())].into();
    let openrouter = server(|url| {
        if url.ends_with("/models") {
            Response::json_response(
                200,
                json!({"data":[{"id":"anthropic/claude-sonnet-5.5","name":"Anthropic: Claude Sonnet 5.5","architecture":{"output_modalities":["text"]},"supported_parameters":["tools"]}]}),
            )
        } else {
            Response::json_response(400, json!({"error":{"message":"fixture rejection"}}))
        }
    });
    let listed = list_models(
        provider("openrouter"),
        ListOptions { store: store.clone(), env: Some(env.clone()), fetch: Some(openrouter.clone()), signal: None },
    )
    .await
    .unwrap();
    assert_eq!(listed[0].id, "openrouter/anthropic/claude-sonnet-5.5");
    assert_eq!(
        check_api_key(provider("openrouter"), KEY, Transport { fetch: Some(openrouter.clone()), signal: None }).await,
        ApiKeyCheck::Unreachable
    );
    let binding = resolve_model(ResolveModelOptions {
        model: listed[0].id.clone(),
        store,
        env: Some(env),
        fetch: Some(openrouter.clone()),
        effort: None,
        service_tier: None,
    })
    .await
    .unwrap();
    let turn = ModelRequest {
        instructions: "fixture instructions".into(),
        messages: vec![Message::user_text("tempo?")],
        tools: vec![FunctionTool::new("get_tempo", "tempo", json!({"type":"object","properties":{}}))],
        session_id: "session-1".into(),
    };
    assert!(binding.model.do_stream((binding.prepare)(turn)).await.is_err());
    let seen = openrouter.seen.borrow();
    // Its list, its key check and the turn all say it's Kumi, as OpenRouter's app rankings read it.
    assert_eq!(seen.len(), 3);
    for (url, headers, _) in seen.iter() {
        assert_eq!(headers["http-referer"], "https://github.com/user1303836/kumi", "{url}");
        assert_eq!(headers["x-openrouter-title"], "Kumi", "{url}");
        assert!(!headers.contains_key("x-openrouter-app-visibility"));
    }
    let body = &seen[2].2;
    assert_eq!(body["cache_control"], json!({"type":"ephemeral"}));
    assert_eq!(body["session_id"], "session-1");
}

#[test]
fn an_empty_balance_is_said_as_one() {
    let venice = refused(
        402,
        json!({"error":"Insufficient USD or Diem balance to complete request. Visit https://venice.ai/settings/api to add credits."}),
    );
    let said = describe_failure(&venice, "venice/fixture-chat");
    assert_eq!(
        (said.kind, said.message.as_str()),
        (FailureKind::Billing, "You're out of Venice credit: add some, or choose another model.")
    );
    let openrouter =
        refused(402, json!({"error":{"message":"Insufficient credits. Add more using https://openrouter.ai/settings/credits","code":402}}));
    assert_eq!(
        describe_failure(&openrouter, "openrouter/org/fixture").message,
        "You're out of OpenRouter credit: add some, or choose another model."
    );
    // A 402 for anything else stays as it was said.
    let other = refused(402, json!({"error":{"message":"Payment method declined"}}));
    assert!(describe_failure(&other, "openai/gpt-6-luna").message.starts_with("OpenAI needs billing sorted before it answers (HTTP 402)"));
}

#[tokio::test]
async fn gemini_turns_down_a_key_it_doesnt_know_with_a_400_and_kumi_hears_a_refusal() {
    let gemini = server(|_| {
        Response::json_response(
            400,
            json!([{"error":{"code":400,"message":"API key not valid. Please pass a valid API key.","status":"INVALID_ARGUMENT","details":[{"reason":"API_KEY_INVALID"}]}}]),
        )
    });
    assert_eq!(
        check_api_key(provider("gemini"), "not-a-key-0000", Transport { fetch: Some(gemini.clone()), signal: None }).await,
        ApiKeyCheck::Refused
    );
    assert_eq!(gemini.seen.borrow()[0].0, "https://generativelanguage.googleapis.com/v1beta/openai/models");
    // Another 400 isn't a refusal of the key.
    let other = server(|_| Response::json_response(400, json!({"error":{"message":"bad request"}})));
    assert_eq!(check_api_key(provider("gemini"), KEY, Transport { fetch: Some(other), signal: None }).await, ApiKeyCheck::Unreachable);
}

#[tokio::test]
async fn chat_and_listening_share_one_gemini_key() {
    let folder = tempfile::tempdir().unwrap();
    let store = open_credential_store(folder.path().join("auth.json"));
    let gemini = provider("gemini");
    let env: HashMap<String, String> = [("GOOGLE_API_KEY".into(), "google-fixture-key".into())].into();
    let key = api_key_for(gemini, &store, Some(&env)).await.unwrap().unwrap();
    assert_eq!((key.key.as_str(), key.source, key.variable.as_deref()), ("google-fixture-key", KeySource::Env, Some("GOOGLE_API_KEY")));
    let both: HashMap<String, String> =
        [("GOOGLE_API_KEY".into(), "google-fixture-key".into()), ("GEMINI_API_KEY".into(), "gemini-fixture-key".into())].into();
    assert_eq!(
        api_key_for(gemini, &store, Some(&both)).await.unwrap().unwrap().key,
        "gemini-fixture-key",
        "GEMINI_API_KEY first, as listening reads it"
    );
    // The key listening keeps, saved as `gemini`, comes first.
    store.update_with("gemini", |_| async { Ok(Some(Credential::ApiKey { key: "saved-fixture-key".into() })) }).await.unwrap();
    let key = api_key_for(gemini, &store, Some(&both)).await.unwrap().unwrap();
    assert_eq!((key.key.as_str(), key.source), ("saved-fixture-key", KeySource::Saved));
    assert_eq!(gemini.row().credential, "gemini");
}
