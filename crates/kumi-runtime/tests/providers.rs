use async_trait::async_trait;
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use kumi_common::{abort::Signal, time::now_ms};
use kumi_runtime::{
    ai::{
        error::LanguageModelError,
        http::{Fetch, FetchInit, Headers, Response},
        types::*,
    },
    auth::store::{open_credential_store, Credential, CredentialStore, FileCredentialStore, OAuthCredential},
    core::{
        contracts::{JsonObject, KernelEvent, KernelTool, ToolResult},
        errors::RuntimeError,
    },
    kernel::agent::{create_agent_kernel, AgentKernel, AgentKernelOptions, ModelBinding, ModelRequest},
    providers::{parse_model_id, resolve_model, words_only, Effort, ResolveModelOptions},
};
use serde_json::{json, Value};
use std::{cell::RefCell, collections::HashMap, rc::Rc};
use tokio::task::LocalSet;
#[derive(Clone)]
struct Captured {
    url: String,
    headers: Headers,
    body: Value,
    form: HashMap<String, String>,
}
struct Recorder {
    requests: Rc<RefCell<Vec<Captured>>>,
    respond: Rc<dyn Fn(&Captured, usize) -> Response>,
}
#[async_trait(?Send)]
impl Fetch for Recorder {
    async fn fetch(&self, url: &str, init: FetchInit) -> Result<Response, LanguageModelError> {
        let form = init.headers.get("content-type").is_some_and(|v| v == "application/x-www-form-urlencoded");
        let raw = init.body.unwrap_or_default();
        let request = Captured {
            url: url.into(),
            headers: init.headers,
            body: if form || raw.is_empty() { json!({}) } else { serde_json::from_str(&raw).unwrap() },
            form: if form { url::form_urlencoded::parse(raw.as_bytes()).into_owned().collect() } else { HashMap::new() },
        };
        self.requests.borrow_mut().push(request.clone());
        let n = self.requests.borrow().len();
        Ok((self.respond)(&request, n))
    }
}
fn recorder(respond: impl Fn(&Captured, usize) -> Response + 'static) -> (Rc<dyn Fetch>, Rc<RefCell<Vec<Captured>>>) {
    let requests = Rc::new(RefCell::new(vec![]));
    (Rc::new(Recorder { requests: requests.clone(), respond: Rc::new(respond) }), requests)
}
fn store() -> (tempfile::TempDir, Rc<FileCredentialStore>) {
    let dir = tempfile::tempdir().unwrap();
    let store = Rc::new(open_credential_store(dir.path().join("auth.json")));
    (dir, store)
}
fn access(account: &str) -> String {
    format!(
        "e30.{}.sig",
        URL_SAFE_NO_PAD.encode(
            serde_json::to_vec(&json!({"https://api.openai.com/auth":{"chatgpt_account_id":account},"exp":now_ms()/1000+3600})).unwrap()
        )
    )
}
fn credential() -> OAuthCredential {
    OAuthCredential {
        access: access("acct-1"),
        refresh: "refresh-1".into(),
        expires: now_ms() as f64 + 86400000.,
        account_id: "acct-1".into(),
    }
}
fn sse(events: Vec<Value>) -> Response {
    Response::text_response(200, events.iter().map(|v| format!("data: {}\n\n", kumi_common::js::json::stringify(v))).collect::<String>())
}
fn rejection() -> Response {
    Response::json_response(400, json!({"error":{"message":"fixture rejection"}}))
}
fn created(id: &str) -> Value {
    json!({"type":"response.created","response":{"id":id,"created_at":1760000000,"model":"gpt-6-astra"}})
}
fn completed() -> Value {
    json!({"type":"response.completed","response":{"incomplete_details":null,"usage":{"input_tokens":63,"input_tokens_details":{"cached_tokens":5},"output_tokens":15,"output_tokens_details":{"reasoning_tokens":4},"total_tokens":78}}})
}
fn reasoning_call() -> Vec<Value> {
    vec![
        created("resp_1"),
        json!({"type":"response.output_item.added","output_index":0,"item":{"id":"rs_1","type":"reasoning","encrypted_content":"enc-1","summary":[]}}),
        json!({"type":"response.output_item.done","output_index":0,"item":{"id":"rs_1","type":"reasoning","encrypted_content":"enc-1","summary":[]}}),
        json!({"type":"response.output_item.added","output_index":1,"item":{"id":"fc_1","type":"function_call","status":"in_progress","arguments":"","call_id":"call_1","name":"get_tempo"}}),
        json!({"type":"response.function_call_arguments.delta","item_id":"fc_1","output_index":1,"delta":"{}"}),
        json!({"type":"response.output_item.done","output_index":1,"item":{"id":"fc_1","type":"function_call","status":"completed","arguments":"{}","call_id":"call_1","name":"get_tempo"}}),
        completed(),
    ]
}
fn answer() -> Vec<Value> {
    vec![
        created("resp_2"),
        json!({"type":"response.output_item.added","output_index":0,"item":{"id":"msg_2","type":"message"}}),
        json!({"type":"response.output_text.delta","item_id":"msg_2","output_index":0,"delta":"120"}),
        json!({"type":"response.output_item.done","output_index":0,"item":{"id":"msg_2","type":"message"}}),
        completed(),
    ]
}
struct Tempo;
#[async_trait(?Send)]
impl KernelTool for Tempo {
    fn name(&self) -> &str {
        "get_tempo"
    }
    fn description(&self) -> &str {
        "tempo"
    }
    fn input_schema(&self) -> JsonObject {
        json!({"type":"object","properties":{}}).as_object().unwrap().clone()
    }
    async fn execute(&self, _input: JsonObject, _signal: Signal) -> Result<ToolResult, RuntimeError> {
        Ok(ToolResult::text("{\"tempo\":120}"))
    }
}
fn kernel(binding: ModelBinding) -> AgentKernel {
    create_agent_kernel(AgentKernelOptions {
        binding,
        instructions: "fixture instructions".into(),
        signal: Signal::new(),
        tools: vec![Rc::new(Tempo)],
        checkpoint: None,
        max_steps: None,
        budget: None,
    })
    .unwrap()
}
fn options(model: &str, store: Rc<dyn CredentialStore>, fetch: Rc<dyn Fetch>) -> ResolveModelOptions {
    ResolveModelOptions { model: model.into(), store, fetch: Some(fetch), env: None, effort: None }
}
fn request(messages: Vec<Message>) -> ModelRequest {
    ModelRequest { instructions: "fixture instructions".into(), messages, tools: vec![], session_id: "session-1".into() }
}
async fn rejected(binding: ModelBinding, messages: Vec<Message>) {
    let result = binding.model.do_stream((binding.prepare)(request(messages))).await;
    assert!(matches!(result,Err(error)if error.to_string().contains("fixture rejection")));
}
#[tokio::test]
async fn codex_stateless_requests_identify_kumi_carry_account_session_headers_and_replay_reasoning() {
    LocalSet::new()
        .run_until(async {
            let (_dir, store) = store();
            let credential = credential();
            store
                .update_with("openai-codex", {
                    let c = credential.clone();
                    move |_| async move { Ok(Some(Credential::Oauth(c))) }
                })
                .await
                .unwrap();
            let (fetch, requests) = recorder(|_, n| sse(if n == 1 { reasoning_call() } else { answer() }));
            let mut opts = options("openai-codex/gpt-6-astra", store, fetch);
            opts.env = Some([("OPENAI_API_KEY".into(), "sk-not-for-codex".into())].into());
            let kernel = kernel(resolve_model(opts).await.unwrap());
            let text = Rc::new(RefCell::new(String::new()));
            let captured = text.clone();
            let result = kernel
                .run(
                    "tempo?",
                    Signal::new(),
                    Rc::new(move |event| {
                        if let KernelEvent::Text { text, .. } = event {
                            captured.borrow_mut().push_str(&text);
                        }
                        Ok(())
                    }),
                )
                .await
                .unwrap();
            assert_eq!(&*text.borrow(), "120");
            assert_eq!(result.usage.as_ref().unwrap().input_tokens, 126.);
            assert_eq!(result.usage.as_ref().unwrap().cache_read_tokens, 10.);
            {
                let requests = requests.borrow();
                assert_eq!(requests.len(), 2);
                let first = &requests[0];
                assert_eq!(first.url, "https://chatgpt.com/backend-api/codex/responses");
                assert_eq!(first.headers["authorization"], format!("Bearer {}", credential.access));
                assert_eq!(first.headers["chatgpt-account-id"], "acct-1");
                assert_eq!(first.headers["originator"], "kumi");
                assert_eq!(first.headers["openai-beta"], "responses=experimental");
                assert!(first.headers["user-agent"].starts_with("kumi/"));
                let session = &first.headers["session-id"];
                assert!(uuid::Uuid::parse_str(session).is_ok());
                assert_eq!(first.body["prompt_cache_key"], *session);
                assert_eq!(first.body["store"], false);
                assert_eq!(first.body["instructions"], "fixture instructions");
                assert_eq!(first.body["text"], json!({"verbosity":"low"}));
                assert_eq!(first.body["include"], json!(["reasoning.encrypted_content"]));
                assert!(first.body.get("max_output_tokens").is_none());
                assert!(!first.body["input"].to_string().contains("fixture instructions"));
                assert_eq!(
                    &requests[1].body["input"].as_array().unwrap()[1..],
                    &[
                        json!({"type":"reasoning","id":"rs_1","encrypted_content":"enc-1","summary":[]}),
                        json!({"type":"function_call","call_id":"call_1","name":"get_tempo","arguments":"{}"}),
                        json!({"type":"function_call_output","call_id":"call_1","output":"{\"tempo\":120}"})
                    ]
                );
                for r in requests.iter() {
                    assert!(!format!("{:?}", r.headers).contains("sk-not-for-codex"));
                }
            }
            kernel.close().await;
        })
        .await;
}
#[tokio::test]
async fn codex_refreshes_near_expiry_and_persists_the_rotated_token() {
    LocalSet::new()
        .run_until(async {
            let (_dir, store) = store();
            let mut credential = credential();
            credential.expires = now_ms() as f64 + 60000.;
            store.update_with("openai-codex", move |_| async move { Ok(Some(Credential::Oauth(credential))) }).await.unwrap();
            let fresh = access("acct-1");
            let response_access = fresh.clone();
            let (fetch, requests) = recorder(move |r, _| {
                if r.url == "https://auth.openai.com/oauth/token" {
                    Response::json_response(200, json!({"access_token":response_access,"refresh_token":"refresh-2","expires_in":3600}))
                } else {
                    sse(answer())
                }
            });
            let kernel = kernel(resolve_model(options("openai-codex/gpt-6-astra", store.clone(), fetch)).await.unwrap());
            kernel.run("hi", Signal::new(), Rc::new(|_| Ok(()))).await.unwrap();
            {
                let requests = requests.borrow();
                assert_eq!(requests[0].form["grant_type"], "refresh_token");
                assert_eq!(requests[0].form["refresh_token"], "refresh-1");
                assert_eq!(requests.iter().filter(|r| r.url.contains("oauth/token")).count(), 1);
                assert_eq!(requests.last().unwrap().headers["authorization"], format!("Bearer {fresh}"));
            }
            let Some(Credential::Oauth(stored)) = store.get("openai-codex").await.unwrap() else { panic!() };
            assert_eq!(stored.refresh, "refresh-2");
            assert!(stored.expires > now_ms() as f64 + 1800000.);
            kernel.close().await;
        })
        .await;
}
#[tokio::test]
async fn codex_missing_or_revoked_signin_fails_without_model_requests() {
    LocalSet::new().run_until(async {
    let (_dir, store) = store();
    let (fetch, requests) = recorder(|_, _| rejection());
    let result = resolve_model(options("openai-codex/gpt-6-astra", store.clone(), fetch)).await;
    assert!(matches!(result,Err(error)if error.to_string().contains("login openai-codex")));
    assert!(requests.borrow().is_empty());
    let mut credential = credential();
    credential.expires = 0.;
    store.update_with("openai-codex", move |_| async move { Ok(Some(Credential::Oauth(credential))) }).await.unwrap();
    let (fetch, _) = recorder(|_, _| Response::json_response(401, json!({})));
    let result = resolve_model(options("openai-codex/gpt-6-astra", store, fetch)).await;
    assert!(
        matches!(result,Err(error)if error.to_string().contains("could not be refreshed (HTTP 401)")&&error.to_string().contains("login openai-codex"))
    );
    }).await;
}
#[tokio::test]
async fn api_key_providers_use_their_endpoints_credentials_and_cache_session_hints() {
    let (_dir, store) = store();
    for (model, url) in [
        ("openai/gpt-6-luna", "https://api.openai.com/v1/responses"),
        ("anthropic/claude-sonnet-5", "https://api.anthropic.com/v1/messages"),
        ("opencode/claude-sonnet-5", "https://opencode.ai/zen/v1/messages"),
        ("opencode/gpt-5.5", "https://opencode.ai/zen/v1/responses"),
        ("opencode/kimi-k2.6", "https://opencode.ai/zen/v1/chat/completions"),
        ("opencode-go/gpt-5.5", "https://opencode.ai/zen/go/v1/responses"),
    ] {
        let (fetch, requests) = recorder(|_, _| rejection());
        let mut opts = options(model, store.clone(), fetch);
        opts.env = Some(
            [
                ("OPENAI_API_KEY".into(), "key-fixture-value".into()),
                ("ANTHROPIC_API_KEY".into(), "key-fixture-value".into()),
                ("OPENCODE_API_KEY".into(), "key-fixture-value".into()),
            ]
            .into(),
        );
        rejected(resolve_model(opts).await.unwrap(), vec![Message::user_text("hi")]).await;
        let requests = requests.borrow();
        let r = &requests[0];
        assert_eq!(r.url, url);
        assert!(r.headers["user-agent"].starts_with("kumi/"));
        if model.starts_with("anthropic/") {
            assert_eq!(r.headers["x-api-key"], "key-fixture-value");
            assert_eq!(r.body["system"], json!([{"type":"text","text":"fixture instructions","cache_control":{"type":"ephemeral"}}]));
            assert_eq!(r.body["messages"][0]["content"][0]["cache_control"], json!({"type":"ephemeral"}));
        } else {
            assert_eq!(r.headers["authorization"], "Bearer key-fixture-value");
        }
        if model.starts_with("opencode") {
            assert_eq!(r.headers["x-opencode-session"], "session-1");
        }
        if model.starts_with("openai/") {
            assert_eq!(r.body["store"], false);
            assert!(!r.headers.contains_key("chatgpt-account-id"));
        }
        if model.contains("kimi-") {
            assert_eq!(r.body["messages"][0]["role"], "system");
        }
    }
}
#[tokio::test]
async fn saved_keys_take_precedence_and_chosen_effort_goes_on_the_wire() {
    let (_dir, store) = store();
    for (provider, key) in [("anthropic", "sk-ant-saved-0000"), ("openai", "sk-openai-saved-0000")] {
        store.update_with(provider, move |_| async move { Ok(Some(Credential::ApiKey { key: key.into() })) }).await.unwrap();
    }
    for (model, header, key, field) in [
        ("anthropic/claude-sonnet-5-5", "x-api-key", "sk-ant-saved-0000", "output_config"),
        ("openai/gpt-6-luna", "authorization", "Bearer sk-openai-saved-0000", "reasoning"),
    ] {
        for effort in [Some(Effort::Low), None] {
            let (fetch, requests) = recorder(|_, _| rejection());
            let mut opts = options(model, store.clone(), fetch);
            opts.effort = effort;
            opts.env = Some([("OPENAI_API_KEY".into(), "stale".into()), ("ANTHROPIC_API_KEY".into(), "stale".into())].into());
            rejected(resolve_model(opts).await.unwrap(), vec![Message::user_text("hi")]).await;
            let requests = requests.borrow();
            assert_eq!(requests[0].headers[header], key);
            if effort.is_some() {
                assert_eq!(requests[0].body[field]["effort"], "low");
            } else {
                assert!(requests[0].body.get(field).is_none());
            }
        }
    }
}
#[tokio::test]
async fn missing_keys_unknown_providers_and_unsupported_routes_never_echo_values() {
    let (_dir, store) = store();
    for (model, message) in [
        ("anthropic/claude-sonnet-5", "ANTHROPIC_API_KEY"),
        ("opencode/gemini-3.5-pro", "Gemini"),
        ("gateway/secret-value", "KUMI_MODEL"),
        ("openai-codex/", "KUMI_MODEL"),
        ("openai-codex/has space", "KUMI_MODEL"),
        ("anthropic", "KUMI_MODEL"),
    ] {
        let (fetch, requests) = recorder(|_, _| rejection());
        let mut opts = options(model, store.clone(), fetch);
        opts.env = Some([("OPENCODE_API_KEY".into(), "k".into())].into());
        let result = resolve_model(opts).await;
        assert!(matches!(result,Err(error)if error.to_string().contains(message)&&!error.to_string().contains("secret-value")));
        assert!(requests.borrow().is_empty());
    }
    assert!(parse_model_id(&format!("openai/{}", "a".repeat(128))).is_some());
    assert!(parse_model_id(&format!("openai/{}", "a".repeat(129))).is_none());
    assert!(parse_model_id("openai/a\n").is_none());
}
#[tokio::test]
async fn tool_images_follow_each_native_api_and_words_only_keeps_other_metadata() {
    let messages:Vec<Message>=serde_json::from_value(json!([{"role":"user","content":[{"type":"text","text":"watch this"}]},{"role":"assistant","content":[{"type":"tool-call","toolCallId":"c1","toolName":"watch_video","input":{}}]},{"role":"tool","content":[{"type":"tool-result","toolCallId":"c1","toolName":"watch_video","output":{"type":"content","value":[{"type":"text","text":"Video"},{"type":"text","text":"Frame at 0:05"},{"type":"file","data":{"type":"data","data":{"0":255,"1":216,"2":255}},"mediaType":"image/jpeg"}]}}]}])).unwrap();
    let (_dir, store) = store();
    for model in ["openai/gpt-6-luna", "anthropic/claude-sonnet-5", "opencode/kimi-k2.6"] {
        let (fetch, requests) = recorder(|_, _| rejection());
        let mut opts = options(model, store.clone(), fetch);
        opts.env = Some(
            [
                ("OPENAI_API_KEY".into(), "key".into()),
                ("ANTHROPIC_API_KEY".into(), "key".into()),
                ("OPENCODE_API_KEY".into(), "key".into()),
            ]
            .into(),
        );
        rejected(resolve_model(opts).await.unwrap(), messages.clone()).await;
        let requests = requests.borrow();
        let body = &requests[0].body;
        if model.starts_with("openai/") {
            let output = &body["input"].as_array().unwrap().iter().find(|v| v["type"] == "function_call_output").unwrap()["output"];
            assert_eq!(
                *output,
                json!([{"type":"input_text","text":"Video"},{"type":"input_text","text":"Frame at 0:05"},{"type":"input_image","image_url":"data:image/jpeg;base64,/9j/","detail":"high"}])
            );
        } else if model.starts_with("anthropic/") {
            let result = body["messages"]
                .as_array()
                .unwrap()
                .iter()
                .flat_map(|v| v["content"].as_array().unwrap())
                .find(|v| v["type"] == "tool_result")
                .unwrap();
            assert_eq!(
                result["content"].as_array().unwrap().last().unwrap(),
                &json!({"type":"image","source":{"type":"base64","media_type":"image/jpeg","data":"/9j/"}})
            );
        } else {
            assert_eq!(
                body["messages"].as_array().unwrap().iter().find(|m| m["role"] == "tool").unwrap()["content"],
                "Video\nFrame at 0:05\n[An image this model can't be shown.]"
            );
        }
    }
    assert_eq!(words_only(vec![Message::user_text("unchanged")]), vec![Message::user_text("unchanged")]);
}
