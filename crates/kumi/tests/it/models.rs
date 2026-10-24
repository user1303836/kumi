use async_trait::async_trait;
use futures::FutureExt;
use kumi::models::*;
use kumi_ai::{
    auth::store::{open_credential_store, Credential, CredentialStore},
    error::LanguageModelError,
    http::{default_fetch, Fetch, FetchInit, Response},
    providers::{
        catalog::{self, Catalog},
        models::ApiKeyCheck,
        table::{add_test_rows, ProviderRow, API_KEY_ROW},
        Effort, ProviderId,
    },
};
use kumi_types::{errors::FailureKind, system::Env};
use serde_json::{json, Value};
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
    sync::Arc,
};
const GOOD: &str = "sk-ant-fixture-good-0001";
struct Anthropic;
#[async_trait(?Send)]
impl Fetch for Anthropic {
    async fn fetch(&self, url: &str, init: FetchInit) -> Result<Response, LanguageModelError> {
        tokio::task::yield_now().await;
        if url != "https://api.anthropic.com/v1/models?limit=100" {
            return Ok(Response::text_response(404, ""));
        }
        if init.headers.get("x-api-key").map(String::as_str) != Some(GOOD) {
            return Ok(Response::json_response(401, json!({"error":{"message":"invalid x-api-key"}})));
        }
        Ok(Response::json_response(
            200,
            json!({"data":[{"id":"claude-sonnet-5-5","display_name":"Claude Sonnet 5.5","capabilities":{"effort":{"supported":true,"low":{"supported":true},"medium":{"supported":true},"high":{"supported":true},"xhigh":{"supported":true},"max":{"supported":true}}}},{"id":"claude-haiku-4-5","display_name":"Claude Haiku 4.5","capabilities":{"effort":{"supported":false}}}]}),
        ))
    }
}
struct Offline;
#[async_trait(?Send)]
impl Fetch for Offline {
    async fn fetch(&self, _: &str, _: FetchInit) -> Result<Response, LanguageModelError> {
        Err(LanguageModelError::other("offline"))
    }
}
/// ChatGPT's model list: Astra offers a faster tier, Luna none; GPT-5.5 hidden, as ChatGPT hides a model it's retiring.
struct Chatgpt;
#[async_trait(?Send)]
impl Fetch for Chatgpt {
    async fn fetch(&self, url: &str, _: FetchInit) -> Result<Response, LanguageModelError> {
        if !url.starts_with("https://chatgpt.com/backend-api/codex/models") {
            return Ok(Response::text_response(404, ""));
        }
        Ok(Response::json_response(
            200,
            json!({"models":[
                {"slug":"gpt-6-astra","display_name":"GPT-6 Astra","priority":1,"visibility":"list","service_tiers":[{"id":"priority","name":"Fast","description":"2x speed, increased usage"}]},
                {"slug":"gpt-6-luna","display_name":"GPT-6 Luna","priority":2,"visibility":"list","service_tiers":[]},
                {"slug":"gpt-5.5","display_name":"GPT-5.5","priority":13,"visibility":"hide","service_tiers":[]}
            ]}),
        ))
    }
}
struct Fixture {
    _folder: tempfile::TempDir,
    control: ModelControl,
    store: Rc<dyn CredentialStore>,
    settings_file: String,
    changes: Rc<Cell<usize>>,
}
impl Fixture {
    fn new(env: Env) -> Self {
        Self::with_fetch(env, Rc::new(Anthropic))
    }
    fn with_fetch(env: Env, fetch: Rc<dyn Fetch>) -> Self {
        let folder = tempfile::tempdir().unwrap();
        let store: Rc<dyn CredentialStore> = Rc::new(open_credential_store(folder.path().join("auth.json")));
        let settings_file = folder.path().join("settings.json").to_string_lossy().into_owned();
        let changes = Rc::new(Cell::new(0));
        let control = create_model_control(ModelControlOptions {
            store: store.clone(),
            settings_file: settings_file.clone(),
            env,
            fetch: Some(fetch),
            changed: Rc::new({
                let changes = changes.clone();
                move || {
                    changes.set(changes.get() + 1);
                    async { Ok(()) }.boxed_local()
                }
            }),
            say: None,
            installed: Some(Rc::new(|_| false)),
        });
        Self { _folder: folder, control, store, settings_file, changes }
    }
    fn settings(&self) -> Value {
        serde_json::from_str(&std::fs::read_to_string(&self.settings_file).unwrap()).unwrap()
    }
}
#[tokio::test(flavor = "current_thread")]
async fn keys_are_kept_only_when_the_provider_accepts_them_then_models_are_available() {
    let f = Fixture::new(Env::new());
    assert!(!f.control.providers().await.unwrap().iter().find(|p| p.id == ProviderId::parse("anthropic").unwrap()).unwrap().signed_in);
    let error = f.control.choose("anthropic/claude-sonnet-5-5").await.unwrap_err();
    assert_eq!(error.kumi().unwrap().kind, FailureKind::Auth);
    assert_eq!(error.kumi().unwrap().provider.as_deref(), Some("anthropic"));
    assert_eq!(
        f.control.save_key(ProviderId::parse("anthropic").unwrap(), "sk-ant-fixture-bad-0002", None).await.unwrap(),
        ApiKeyCheck::Refused
    );
    assert!(f.store.get("anthropic").await.unwrap().is_none());
    assert_eq!(f.control.save_key(ProviderId::parse("anthropic").unwrap(), &format!(" {GOOD}\n"), None).await.unwrap(), ApiKeyCheck::Ok);
    assert_eq!(f.store.get("anthropic").await.unwrap(), Some(Credential::ApiKey { key: GOOD.into() }));
    assert_eq!(
        f.control.providers().await.unwrap().iter().find(|p| p.id == ProviderId::parse("anthropic").unwrap()).unwrap().via.as_deref(),
        Some("saved key")
    );
    assert_eq!(
        f.control.models("anthropic", false).await.unwrap().iter().map(|m| m.name.as_str()).collect::<Vec<_>>(),
        ["Claude Sonnet 5.5", "Claude Haiku 4.5"]
    );
}
#[tokio::test(flavor = "current_thread")]
async fn model_and_effort_persist_an_unsupported_effort_reverts_and_signout_is_applied() {
    let f = Fixture::new(Env::new());
    f.control.save_key(ProviderId::parse("anthropic").unwrap(), GOOD, None).await.unwrap();
    assert_eq!(f.control.choose_default().await.unwrap().unwrap().model.id, "anthropic/claude-sonnet-5-5");
    f.control.set_effort(Some(Effort::Max)).await.unwrap();
    assert_eq!(f.settings(), json!({"model":"anthropic/claude-sonnet-5-5","effort":"max"}));
    let mut current = serde_json::to_value(f.control.current()).unwrap();
    current["efforts"] = json!(current["efforts"].as_array().unwrap().len());
    assert_eq!(
        current,
        json!({"model":"anthropic/claude-sonnet-5-5","provider":"anthropic","name":"Claude Sonnet 5.5","effort":"max","efforts":5,"pinned":false})
    );
    let binding = f.control.binding().await.unwrap();
    assert_eq!(binding.id, "anthropic/claude-sonnet-5-5");
    assert!(Rc::ptr_eq(&binding.model, &f.control.binding().await.unwrap().model));
    f.control.choose("anthropic/claude-haiku-4-5").await.unwrap();
    assert_eq!(f.settings(), json!({"model":"anthropic/claude-haiku-4-5"}));
    assert!(f.changes.get() >= 3);
    assert!(f.control.sign_out(ProviderId::parse("anthropic").unwrap()).await.unwrap());
    let error = f.control.binding().await.err().unwrap();
    assert_eq!(error.kumi().unwrap().kind, FailureKind::Auth);
    assert_eq!(error.kumi().unwrap().provider.as_deref(), Some("anthropic"));
    assert!(!f.control.sign_out(ProviderId::parse("anthropic").unwrap()).await.unwrap());
}
/// Signed in to ChatGPT, with a token good for a day.
async fn sign_in_chatgpt(store: &Rc<dyn CredentialStore>) {
    use base64::Engine;
    let claims = json!({"https://api.openai.com/auth":{"chatgpt_account_id":"acct-1"},"exp":kumi_common::time::now_ms()/1000+3600});
    let access = format!("e30.{}.sig", base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(serde_json::to_vec(&claims).unwrap()));
    let credential = kumi_ai::auth::store::OAuthCredential {
        access,
        refresh: "refresh-1".into(),
        expires: kumi_common::time::now_ms() as f64 + 86400000.,
        account_id: "acct-1".into(),
    };
    store.update("openai-codex", Box::new(move |_| async move { Ok(Some(Credential::Oauth(credential))) }.boxed_local())).await.unwrap();
}
#[tokio::test(flavor = "current_thread")]
async fn fast_uses_the_tier_the_models_list_offers_persists_and_reaches_the_request() {
    let f = Fixture::with_fetch(Env::new(), Rc::new(Chatgpt));
    sign_in_chatgpt(&f.store).await;
    let tier = |control: &ModelControl| {
        let control = control.clone();
        async move {
            let binding = control.binding().await.unwrap();
            let request =
                kumi_ai::binding::ModelRequest { instructions: "i".into(), messages: vec![], tools: vec![], session_id: "s".into() };
            (binding.prepare)(request).provider_options.unwrap()["openai"].get("serviceTier").cloned()
        }
    };
    f.control.choose("openai-codex/gpt-6-luna").await.unwrap();
    assert_eq!(f.control.set_fast(true).await.unwrap(), None, "Luna's list offers no faster tier");
    assert!(f.settings().get("fast").is_none());
    f.control.choose("openai-codex/gpt-6-astra").await.unwrap();
    assert_eq!(f.control.set_fast(true).await.unwrap().map(|t| t.name), Some("Fast".into()));
    assert_eq!(f.settings(), json!({"model":"openai-codex/gpt-6-astra","fast":true}));
    assert_eq!(f.control.current().fast.map(|t| t.name).as_deref(), Some("Fast"));
    assert_eq!(tier(&f.control).await, Some(json!("priority")));
    f.control.choose("openai-codex/gpt-6-luna").await.unwrap();
    assert_eq!(f.control.current().fast, None, "on, but this model has no faster tier");
    assert!(f.control.fast_enabled());
    assert_eq!(tier(&f.control).await, None);
    // A saved "on" turns off from a model without a tier too, so the next model doesn't get it unasked.
    assert_eq!(f.control.set_fast(false).await.unwrap(), None);
    assert_eq!(f.settings(), json!({"model":"openai-codex/gpt-6-luna"}));
    f.control.choose("openai-codex/gpt-6-astra").await.unwrap();
    assert_eq!(tier(&f.control).await, None);
}
#[tokio::test(flavor = "current_thread")]
async fn opencode_zen_and_go_share_new_keys_and_invalidate_the_current_binding() {
    let folder = tempfile::tempdir().unwrap();
    let file = folder.path().join("settings.json");
    std::fs::write(&file, json!({"model":"opencode-go/some-model"}).to_string()).unwrap();
    let changes = Rc::new(Cell::new(0));
    let control = create_model_control(ModelControlOptions {
        store: Rc::new(open_credential_store(folder.path().join("auth.json"))),
        settings_file: file.to_string_lossy().into(),
        env: Env::new(),
        fetch: Some(Rc::new(Offline)),
        changed: Rc::new({
            let changes = changes.clone();
            move || {
                changes.set(changes.get() + 1);
                async { Ok(()) }.boxed_local()
            }
        }),
        say: None,
        installed: None,
    });
    assert_eq!(
        control.save_key(ProviderId::parse("opencode").unwrap(), "oc-fixture-key-0001", None).await.unwrap(),
        ApiKeyCheck::Unreachable
    );
    assert_eq!(changes.get(), 1);
}
#[tokio::test(flavor = "current_thread")]
async fn a_saved_model_its_providers_list_no_longer_shows_is_said() {
    let f = Fixture::with_fetch(Env::new(), Rc::new(Chatgpt));
    sign_in_chatgpt(&f.store).await;
    f.control.choose("openai-codex/gpt-5.5").await.unwrap();
    assert_eq!(missing_model(&f.control).await.as_deref(), Some("ChatGPT no longer offers gpt-5.5 to your sign-in"));
    f.control.choose("openai-codex/gpt-6-luna").await.unwrap();
    assert_eq!(missing_model(&f.control).await, None);
    // Nothing new when the list can't be read (offline, signed out) or KUMI_MODEL picks the model.
    let offline = Fixture::with_fetch(Env::new(), Rc::new(Offline));
    sign_in_chatgpt(&offline.store).await;
    offline.control.choose("openai-codex/gpt-5.5").await.unwrap();
    assert_eq!(missing_model(&offline.control).await, None);
    f.control.choose("openai-codex/gpt-5.5").await.unwrap();
    assert!(f.control.sign_out(ProviderId::parse("openai-codex").unwrap()).await.unwrap());
    assert_eq!(missing_model(&f.control).await, None);
    let pinned = Fixture::with_fetch(Env::from([("KUMI_MODEL".into(), "openai-codex/gpt-5.5".into())]), Rc::new(Chatgpt));
    sign_in_chatgpt(&pinned.store).await;
    assert_eq!(missing_model(&pinned.control).await, None);
}
#[tokio::test(flavor = "current_thread")]
async fn environment_model_choices_are_temporary_and_environment_keys_cannot_be_removed() {
    let f =
        Fixture::new(Env::from([("KUMI_MODEL".into(), "anthropic/claude-sonnet-5-5".into()), ("ANTHROPIC_API_KEY".into(), GOOD.into())]));
    assert!(f.control.current().pinned);
    assert_eq!(
        f.control.providers().await.unwrap().iter().find(|p| p.id == ProviderId::parse("anthropic").unwrap()).unwrap().via.as_deref(),
        Some("environment")
    );
    f.control.choose("anthropic/claude-haiku-4-5").await.unwrap();
    assert_eq!(f.control.current().model.as_deref(), Some("anthropic/claude-haiku-4-5"));
    assert_eq!(f.settings(), json!({}));
    assert!(!f.control.sign_out(ProviderId::parse("anthropic").unwrap()).await.unwrap());
    let none = Fixture::new(Env::new());
    assert!(none.control.binding().await.err().unwrap().to_string().contains("Choose a model to talk to: type /model"));
}
async fn ollama() -> (String, tokio::task::JoinHandle<()>) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let task = tokio::spawn(async move {
        loop {
            let Ok((mut socket, _)) = listener.accept().await else { break };
            tokio::spawn(async move {
                let mut raw = Vec::new();
                let mut buffer = [0; 4096];
                let (header_end, length) = loop {
                    let n = socket.read(&mut buffer).await.unwrap();
                    if n == 0 {
                        return;
                    }
                    raw.extend_from_slice(&buffer[..n]);
                    if let Some(end) = raw.windows(4).position(|s| s == b"\r\n\r\n") {
                        let header = String::from_utf8_lossy(&raw[..end]);
                        let length = header
                            .lines()
                            .find_map(|l| {
                                l.to_ascii_lowercase().strip_prefix("content-length:").and_then(|v| v.trim().parse::<usize>().ok())
                            })
                            .unwrap_or(0);
                        break (end + 4, length);
                    }
                };
                while raw.len() < header_end + length {
                    let n = socket.read(&mut buffer).await.unwrap();
                    if n == 0 {
                        return;
                    }
                    raw.extend_from_slice(&buffer[..n]);
                }
                let head = String::from_utf8_lossy(&raw[..header_end]);
                let path = head.split_whitespace().nth(1).unwrap();
                let body = serde_json::from_slice::<Value>(&raw[header_end..header_end + length]).unwrap_or(Value::Null);
                let (code, response) = match path {
                    "/api/version" => (200, json!({"version":"0.12.6"})),
                    "/api/tags" => (
                        200,
                        json!({"models":[{"name":"qwen3:8b","model":"qwen3:8b","details":{"parameter_size":"8B","quantization_level":"Q4_K_M"}},{"name":"gemma3:4b","model":"gemma3:4b","details":{"parameter_size":"8B","quantization_level":"Q4_K_M"}}]}),
                    ),
                    "/api/ps" => (200, json!({"models":[{"name":"qwen3:8b","model":"qwen3:8b"}]})),
                    "/api/show" => match body["model"].as_str() {
                        Some("qwen3:8b") => (200, json!({"capabilities":["completion","tools"],"model_info":{}})),
                        Some("gemma3:4b") => (200, json!({"capabilities":["completion","vision"],"model_info":{}})),
                        _ => (404, json!({"error":"not found"})),
                    },
                    _ => (404, json!({"error":"not found"})),
                };
                let response = response.to_string();
                let response = format!(
                    "HTTP/1.1 {code} OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{response}",
                    response.len()
                );
                let _ = socket.write_all(response.as_bytes()).await;
            });
        }
    });
    (url, task)
}
struct NoStudio;
#[async_trait(?Send)]
impl Fetch for NoStudio {
    async fn fetch(&self, url: &str, init: FetchInit) -> Result<Response, LanguageModelError> {
        if url.starts_with("http://127.0.0.1:1234/") {
            return Err(LanguageModelError::other("connect ECONNREFUSED 127.0.0.1:1234"));
        }
        default_fetch().fetch(url, init).await
    }
}
#[tokio::test(flavor = "current_thread")]
async fn local_models_need_no_signin_choose_a_loaded_tool_model_and_explain_limitations_once() {
    tokio::task::LocalSet::new().run_until(async{
let(server,task)=ollama().await;let folder=tempfile::tempdir().unwrap();let file=folder.path().join("settings.json");let servers=json!([{"name":"llama.cpp","baseURL":"http://127.0.0.1:9/v1"}]);std::fs::write(&file,json!({"modelServers":servers}).to_string()).unwrap();let said=Rc::new(RefCell::new(Vec::new()));let make=|file:&std::path::Path,auth:&str|create_model_control(ModelControlOptions{store:Rc::new(open_credential_store(folder.path().join(auth))),settings_file:file.to_string_lossy().into(),env:Env::from([("OLLAMA_HOST".into(),server.clone())]),fetch:Some(Rc::new(NoStudio)),changed:Rc::new(||async{Ok(())}.boxed_local()),say:Some(Rc::new({let said=said.clone();move|message|said.borrow_mut().push(message)})),installed:Some(Rc::new(|_|false))});let control=make(&file,"auth.json");assert_eq!(serde_json::to_value(control.local().await).unwrap(),json!([{"id":"ollama","name":"Ollama","where":"on this computer","running":true},{"id":"llama-cpp","name":"llama.cpp","where":"on this computer","running":false,"start":"Start it, or check its address (http://127.0.0.1:9/v1) in ~/.kumi/settings.json"}]));assert_eq!(control.models("ollama",false).await.unwrap().into_iter().map(|m|(m.id,m.description)).collect::<Vec<_>>(),vec![("ollama/qwen3:8b".into(),Some("8B · Q4_K_M · loaded".into())),("ollama/gemma3:4b".into(),Some("8B · Q4_K_M · can't change the Set".into()))]);let chosen=control.choose_default().await.unwrap().unwrap();assert_eq!(chosen.model.id,"ollama/qwen3:8b");assert_eq!(chosen.note,None);let mut current=serde_json::to_value(control.current()).unwrap();current.as_object_mut().unwrap().remove("efforts");assert_eq!(current,json!({"model":"ollama/qwen3:8b","provider":"ollama","name":"qwen3:8b","pinned":false,"where":"on this computer"}));assert_eq!(serde_json::from_str::<Value>(&std::fs::read_to_string(&file).unwrap()).unwrap(),json!({"model":"ollama/qwen3:8b","modelServers":servers}));assert_eq!(control.choose("ollama/gemma3:4b").await.unwrap().as_deref(),Some("gemma3:4b can't use tools, so Kumi can talk with it about your Set but can't change anything. qwen3:8b on Ollama can: /model chooses it."));assert_eq!(control.choose("ollama/gemma3:4b").await.unwrap(),None);assert_eq!(control.binding().await.unwrap().id,"ollama/gemma3:4b");assert!(said.borrow().is_empty());for(id,name)in[("ollama","Ollama"),("llama-cpp","llama.cpp"),("anthropic","Anthropic")]{assert_eq!(control.provider_name(id),name);}let racing=make(&folder.path().join("fresh-settings.json"),"fresh-auth.json");let(announced,bound)=tokio::join!(racing.choose_default(),racing.binding());assert_eq!(announced.unwrap().unwrap().model.id,"ollama/qwen3:8b");assert_eq!(bound.unwrap().id,"ollama/qwen3:8b");let later=make(&file,"auth.json");let binding=later.binding_with_info().await.unwrap();binding.asked.unwrap().await;tokio::task::yield_now().await;assert_eq!(said.borrow().len(),1);assert!(said.borrow()[0].starts_with("gemma3:4b can't use tools"));task.abort();
}).await;
}
/// A provider added after the first five, as Venice was: only this test's thread has it.
static ADDED: [ProviderRow; 1] = [ProviderRow {
    id: "acme",
    name: "Acme AI",
    short_name: "Acme",
    credential: "acme",
    key_env: &["ACME_API_KEY"],
    base_url: "https://api.acme.example.test/v1",
    ..API_KEY_ROW
}];
fn control_with(folder: &tempfile::TempDir, settings: Value) -> ModelControl {
    let settings_file = folder.path().join("settings.json");
    std::fs::write(&settings_file, settings.to_string()).unwrap();
    create_model_control(ModelControlOptions {
        store: Rc::new(open_credential_store(folder.path().join("auth.json"))),
        settings_file: settings_file.to_string_lossy().into_owned(),
        env: Env::new(),
        fetch: Some(Rc::new(Offline)),
        changed: Rc::new(|| async { Ok(()) }.boxed_local()),
        say: None,
        installed: Some(Rc::new(|_| false)),
    })
}
#[tokio::test(flavor = "current_thread")]
async fn a_custom_server_keeps_its_id_and_hides_a_provider_added_later_that_has_it() {
    add_test_rows(&ADDED);
    let folder = tempfile::tempdir().unwrap();
    let control = control_with(
        &folder,
        json!({"model":"acme/qwen3-coder","modelServers":[{"name":"Acme","baseURL":"https://acme.example.test/v1"}]}),
    );
    // The model saved as the server's is still the server's.
    let current = control.current();
    assert_eq!((current.provider.as_deref(), current.r#where.as_deref()), (Some("acme"), Some("on acme.example.test")));
    assert_eq!(control.provider_name("acme"), "Acme");
    assert_eq!(control.cloud_provider("acme"), None);
    assert_eq!(missing_model(&control).await, None);
    // /login and the picker don't offer the provider it hides.
    let offered: Vec<_> = control.providers().await.unwrap().into_iter().map(|status| status.id.as_str()).collect();
    assert!(!offered.contains(&"acme") && offered.contains(&"venice"), "{offered:?}");
    // Without that server, the provider is offered, last, by its own name.
    let control = control_with(&folder, json!({"model":"acme/qwen3-coder"}));
    assert_eq!((control.current().provider.as_deref(), control.current().r#where), (Some("acme"), None));
    assert_eq!(control.provider_name("acme"), "Acme AI");
    assert_eq!(control.cloud_provider("acme"), ProviderId::parse("acme"));
    let offered: Vec<_> = control.providers().await.unwrap().into_iter().map(|status| status.id.as_str()).collect();
    assert_eq!(offered.last(), Some(&"acme"));
}
/// OpenAI's API for a turn: its list, then each request kept and answered; a model that can't use tools turns a
/// request with tools down, as a provider would.
struct ToolsApi {
    seen: Rc<RefCell<Vec<Value>>>,
}
#[async_trait(?Send)]
impl Fetch for ToolsApi {
    async fn fetch(&self, url: &str, init: FetchInit) -> Result<Response, LanguageModelError> {
        if url == "https://api.openai.com/v1/models" {
            return Ok(Response::json_response(
                200,
                json!({"data":[{"id":"gpt-fixture-chat","created":2},{"id":"gpt-fixture-tools","created":1}]}),
            ));
        }
        let body: Value = serde_json::from_str(init.body.as_deref().unwrap_or("{}")).unwrap();
        self.seen.borrow_mut().push(body.clone());
        Ok(if body["model"] == "gpt-fixture-tools" && body["tools"].is_array() {
            Response::json_response(400, json!({"error":{"message":"This model does not support tools."}}))
        } else {
            Response::json_response(400, json!({"error":{"message":"fixture rejection"}}))
        })
    }
}
fn turn() -> kumi_ai::binding::ModelRequest {
    kumi_ai::binding::ModelRequest {
        instructions: "fixture instructions".into(),
        messages: vec![kumi_ai::types::Message::user_text("tempo?")],
        tools: vec![kumi_ai::types::FunctionTool::new("get_tempo", "tempo", json!({"type":"object","properties":{}}))],
        session_id: "session-1".into(),
    }
}
#[tokio::test(flavor = "current_thread")]
async fn a_model_that_cant_use_tools_talks_without_them_whether_its_list_says_so_or_a_call_does() {
    // What models.dev says of the two, on this test's thread only.
    catalog::install(Arc::new(Catalog::read(&json!({"openai":{"models":{
        "gpt-fixture-chat":{"name":"Fixture Chat","tool_call":false},
        "gpt-fixture-tools":{"name":"Fixture Tools","tool_call":true}
    }}}))));
    let seen = Rc::new(RefCell::new(vec![]));
    let f =
        Fixture::with_fetch(Env::from([("OPENAI_API_KEY".into(), "key-fixture-value".into())]), Rc::new(ToolsApi { seen: seen.clone() }));
    let listed = f.control.models("openai", false).await.unwrap();
    assert_eq!(
        listed.iter().map(|m| (m.name.as_str(), m.tools)).collect::<Vec<_>>(),
        [("Fixture Chat", Some(false)), ("Fixture Tools", Some(true))]
    );
    let no_tools = "This model can't use tools here: you see the Set only as it's described above";
    // Its list says it can't: chosen, Kumi says so once, and its requests go without tools.
    assert_eq!(
        f.control.choose("openai/gpt-fixture-chat").await.unwrap().as_deref(),
        Some(
            "Fixture Chat can't use tools, so Kumi can talk with it about your Set but can't change anything. /model chooses one that can."
        )
    );
    let binding = f.control.binding().await.unwrap();
    assert!(binding.model.do_stream((binding.prepare)(turn())).await.is_err());
    let body = seen.borrow().last().unwrap().clone();
    assert!(body.get("tools").is_none(), "{body}");
    assert!(body["instructions"].as_str().unwrap().contains(no_tools), "{body}");
    // Its list says it can, but a call is turned down for its tools: the turn says so, and from then on it's sent none.
    assert_eq!(f.control.choose("openai/gpt-fixture-tools").await.unwrap(), None);
    let binding = f.control.binding().await.unwrap();
    let refused = binding.model.do_stream((binding.prepare)(turn())).await.err().unwrap().to_string();
    assert!(refused.starts_with("Fixture Tools can't use tools, so Kumi can talk with it about your Set"), "{refused}");
    assert!(seen.borrow().last().unwrap()["tools"].is_array());
    assert!(binding.model.do_stream((binding.prepare)(turn())).await.err().unwrap().to_string().contains("fixture rejection"));
    let body = seen.borrow().last().unwrap().clone();
    assert!(body.get("tools").is_none() && body["instructions"].as_str().unwrap().contains(no_tools), "{body}");
    // The same model bound again (a new kernel) remembers it for the session.
    let again = f.control.binding_for("openai/gpt-fixture-tools").await.unwrap();
    let _ = again.model.do_stream((again.prepare)(turn())).await;
    assert!(seen.borrow().last().unwrap().get("tools").is_none());
}
/// A custom server that takes Responses: its list, then each request kept.
struct ResponsesBox {
    seen: Rc<RefCell<Vec<String>>>,
}
#[async_trait(?Send)]
impl Fetch for ResponsesBox {
    async fn fetch(&self, url: &str, _: FetchInit) -> Result<Response, LanguageModelError> {
        self.seen.borrow_mut().push(url.into());
        Ok(if url.ends_with("/models") {
            Response::json_response(200, json!({"data":[{"id":"gpt-local"}]}))
        } else {
            Response::json_response(400, json!({"error":{"message":"fixture rejection"}}))
        })
    }
}
#[tokio::test(flavor = "current_thread")]
async fn a_custom_server_that_takes_responses_is_sent_responses() {
    let folder = tempfile::tempdir().unwrap();
    let settings_file = folder.path().join("settings.json");
    let servers = json!([{"name":"Box","baseURL":"http://192.168.1.30:8000/v1","format":"responses"}]);
    std::fs::write(&settings_file, json!({"modelServers":servers}).to_string()).unwrap();
    let seen = Rc::new(RefCell::new(vec![]));
    let control = create_model_control(ModelControlOptions {
        store: Rc::new(open_credential_store(folder.path().join("auth.json"))),
        settings_file: settings_file.to_string_lossy().into_owned(),
        env: Env::new(),
        fetch: Some(Rc::new(ResponsesBox { seen: seen.clone() })),
        changed: Rc::new(|| async { Ok(()) }.boxed_local()),
        say: None,
        installed: Some(Rc::new(|_| false)),
    });
    let listed = control.models("box", false).await.unwrap();
    assert_eq!(
        listed.iter().map(|m| (m.id.as_str(), m.facts_from)).collect::<Vec<_>>(),
        [("box/gpt-local", Some(kumi_ai::providers::models::FactsFrom::Guess))]
    );
    assert_eq!(control.choose("box/gpt-local").await.unwrap(), None);
    let binding = control.binding().await.unwrap();
    assert!(binding.model.do_stream((binding.prepare)(turn())).await.is_err());
    assert_eq!(seen.borrow().last().map(String::as_str), Some("http://192.168.1.30:8000/v1/responses"));
}
#[tokio::test(flavor = "current_thread")]
async fn a_key_from_the_environment_names_the_variable_it_came_from() {
    let f = Fixture::new(Env::from([("GOOGLE_API_KEY".into(), "google-fixture-key".into())]));
    let gemini = f.control.providers().await.unwrap().into_iter().find(|p| p.id.as_str() == "gemini").unwrap();
    assert_eq!((gemini.via.as_deref(), gemini.key_env.as_deref()), (Some("environment"), Some("GOOGLE_API_KEY")));
    // Not signed in, it names the first variable it would read.
    let openrouter = f.control.providers().await.unwrap().into_iter().find(|p| p.id.as_str() == "openrouter").unwrap();
    assert_eq!((openrouter.signed_in, openrouter.key_env.as_deref()), (false, Some("OPENROUTER_API_KEY")));
}
