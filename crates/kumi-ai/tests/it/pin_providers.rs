//! Pins what each model provider does: the models each one lists from a small answer, the requests one turn sends down
//! each route (the cloud providers' and the servers on the producer's computer), and what each API-key check makes of
//! an answer. The goldens are in tests/goldens/providers; `KUMI_BLESS=1` writes them afresh.
use async_trait::async_trait;
use kumi_ai::{
    auth::store::{open_credential_store, Credential, FileCredentialStore, OAuthCredential},
    binding::ModelRequest,
    error::LanguageModelError,
    http::{Fetch, FetchInit, Headers, Response},
    providers::{
        local::{
            list_local_models, local_servers, resolve_local_model, resolve_server_model, LocalModelOptions, LocalServer, ServerSetting,
        },
        models::{check_api_key, list_models, ListOptions, Transport},
        resolve_model, Effort, Format, ProviderId, ResolveModelOptions,
    },
    types::*,
};
use kumi_common::time::now_ms;
use serde_json::{json, Map, Value};
use std::{cell::RefCell, collections::HashMap, rc::Rc, sync::LazyLock};
use tokio::task::LocalSet;

type Answer = dyn Fn(&str, &Value) -> Result<Response, LanguageModelError>;
/// Every request a fixture server saw, as sent.
struct Recorder {
    seen: Rc<RefCell<Vec<Value>>>,
    answer: Rc<Answer>,
}
#[async_trait(?Send)]
impl Fetch for Recorder {
    async fn fetch(&self, url: &str, init: FetchInit) -> Result<Response, LanguageModelError> {
        let body = init.body.as_deref().map(|body| serde_json::from_str(body).unwrap_or_else(|_| json!(body))).unwrap_or(Value::Null);
        let mut seen = Map::new();
        seen.insert("method".into(), json!(if init.method.is_empty() { "GET" } else { init.method.as_str() }));
        seen.insert("url".into(), json!(url));
        seen.insert("headers".into(), masked(&init.headers));
        if !body.is_null() {
            seen.insert("body".into(), body.clone());
        }
        self.seen.borrow_mut().push(Value::Object(seen));
        (self.answer)(url, &body)
    }
}
fn recorder(answer: impl Fn(&str, &Value) -> Result<Response, LanguageModelError> + 'static) -> (Rc<dyn Fetch>, Rc<RefCell<Vec<Value>>>) {
    let seen = Rc::new(RefCell::new(vec![]));
    (Rc::new(Recorder { seen: seen.clone(), answer: Rc::new(answer) }), seen)
}
/// The headers as sent, Kumi's user agent without its version and this computer's system, which differ by machine.
fn masked(headers: &Headers) -> Value {
    static AGENT: LazyLock<regex::Regex> = LazyLock::new(|| regex::Regex::new(r"kumi/\S+ \([^)]*\)").unwrap());
    Value::Object(
        headers
            .iter()
            .map(|(name, value)| {
                let value =
                    if name == "user-agent" { AGENT.replace_all(value, "kumi/<version> (<system>)").into_owned() } else { value.clone() };
                (name.clone(), json!(value))
            })
            .collect(),
    )
}
fn golden(name: &str, value: &Value) {
    let file = kumi_testkit::workspace_path("crates/kumi-ai/tests/goldens/providers").join(name);
    let mut actual = serde_json::to_string_pretty(value).unwrap();
    actual.push('\n');
    if std::env::var_os("KUMI_BLESS").is_some() {
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, actual).unwrap();
        return;
    }
    let expected =
        std::fs::read_to_string(&file).unwrap_or_else(|_| panic!("{} is missing: run this test with KUMI_BLESS=1", file.display()));
    assert!(
        actual == expected,
        "{} changed: check the difference, then run this test with KUMI_BLESS=1\n{}",
        file.display(),
        difference(&expected, &actual)
    );
}
fn difference(expected: &str, actual: &str) -> String {
    let (expected, actual): (Vec<_>, Vec<_>) = (expected.lines().collect(), actual.lines().collect());
    let line = expected.iter().zip(&actual).position(|(a, b)| a != b).unwrap_or(expected.len().min(actual.len()));
    let mut text = format!("first difference at line {} (- expected, + actual):\n", line + 1);
    for (mark, lines) in [('-', &expected), ('+', &actual)] {
        for (i, differs) in lines.iter().enumerate().skip(line).take(3) {
            text += &format!("  {mark} {:>5} {differs}\n", i + 1);
        }
    }
    text
}
fn store() -> (tempfile::TempDir, Rc<FileCredentialStore>) {
    let dir = tempfile::tempdir().unwrap();
    let store = Rc::new(open_credential_store(dir.path().join("auth.json")));
    (dir, store)
}
async fn signed_in_to_chatgpt(store: &FileCredentialStore) {
    store
        .update_with("openai-codex", |_| async {
            Ok(Some(Credential::Oauth(OAuthCredential {
                access: "access-fixture".into(),
                refresh: "refresh-fixture".into(),
                expires: now_ms() as f64 + 86_400_000.,
                account_id: "account-fixture".into(),
            })))
        })
        .await
        .unwrap();
}
const KEY: &str = "key-fixture-value";
fn keys() -> HashMap<String, String> {
    ["OPENAI_API_KEY", "ANTHROPIC_API_KEY", "OPENCODE_API_KEY"].map(|name| (name.to_string(), KEY.to_string())).into()
}
fn provider(id: &str) -> ProviderId {
    ProviderId::parse(id).unwrap()
}
fn rejection() -> Response {
    Response::json_response(400, json!({"error":{"message":"fixture rejection"}}))
}

/// Custom servers beside Ollama and LM Studio: one on another computer, one that takes Responses, one Anthropic's format.
fn servers() -> Vec<LocalServer> {
    local_servers(
        &[
            ServerSetting {
                name: "My Server".into(),
                base_url: "http://192.168.1.20:8080".into(),
                api_key: Some(KEY.into()),
                format: None,
            },
            ServerSetting {
                name: "Responses Box".into(),
                base_url: "http://192.168.1.30:8000/v1".into(),
                api_key: Some(KEY.into()),
                format: Some(Format::Responses),
            },
            ServerSetting {
                name: "Claude Proxy".into(),
                base_url: "https://claude-proxy.example.test/v1".into(),
                api_key: Some(KEY.into()),
                format: Some(Format::Messages),
            },
        ],
        &HashMap::new(),
    )
    .unwrap()
}
fn server(id: &str) -> LocalServer {
    servers().into_iter().find(|server| server.id == id).unwrap()
}
fn ollama_show(model: &str) -> Value {
    match model {
        "qwen3:8b" => {
            json!({"capabilities":["completion","tools","thinking"],"model_info":{"general.architecture":"qwen3","qwen3.context_length":40960},"thinking":{"values":["low","medium","high"],"default":"medium"}})
        }
        "nomic-embed-text:latest" => json!({"capabilities":["embedding"],"model_info":{"general.architecture":"nomic-bert"}}),
        "llama3.2:1b" => json!({"capabilities":["completion"],"model_info":{"general.architecture":"llama","llama.context_length":131072}}),
        "gemma3:27b" => {
            json!({"capabilities":["completion","vision","tools"],"model_info":{"general.architecture":"gemma3","gemma3.context_length":131072},"details":{"parameter_size":"27.4B"}})
        }
        _ => Value::Null,
    }
}
fn lm_studio_models() -> Value {
    json!({"models":[
        {"type":"llm","key":"qwen/qwen3-8b","display_name":"Qwen3 8B","params_string":"8B","quantization":{"name":"Q4_K_M"},"max_context_length":32768,"capabilities":{"trained_for_tool_use":true,"reasoning":{"allowed_options":["low","medium","high"],"default":"medium"},"vision":false},"loaded_instances":[{"id":"qwen/qwen3-8b","config":{"context_length":32768}}]},
        {"type":"embedding","key":"text-embedding-nomic-embed-text-v1.5"},
        {"type":"llm","key":"google/gemma-3-1b","display_name":"Gemma 3 1B","max_context_length":32768,"capabilities":{"trained_for_tool_use":false,"vision":true},"loaded_instances":[]},
        {"type":"llm","key":"mistral/devstral@q8_0","display_name":"  Devstral (Q8) ✦  "}
    ]})
}
/// What every provider and server answers with: a small list for each, an error otherwise.
fn answer(url: &str, body: &Value) -> Result<Response, LanguageModelError> {
    let listed = |value: Value| Ok(Response::json_response(200, value));
    match url {
        "https://chatgpt.com/backend-api/codex/models?client_version=1.0.0" => listed(json!({"models":[
            {"slug":"gpt-6-luna","display_name":"GPT-6 Luna","description":"Fast","priority":3,"visibility":"list","default_reasoning_level":"low","supported_reasoning_levels":[{"effort":"low","description":"Quick"},{"effort":"medium"}]},
            {"slug":"gpt-reserve","display_name":"Reserve","priority":0,"visibility":"hide"},
            {"slug":"gpt-6-astra","display_name":"GPT-6 Astra","priority":1,"visibility":"list","default_reasoning_level":"medium","supported_reasoning_levels":[{"effort":"low"},{"effort":"medium"},{"effort":"high"},{"effort":"xhigh"},{"effort":"minimal"},{"effort":"turbo"}],"service_tiers":[{"id":"priority","name":"Fast","description":"2x speed, increased usage"},{"id":"no spaces allowed","name":"Odd"}]},
            {"slug":"gpt-6-nova","display_name":"  GPT-6 Nova ✦  ","priority":"0","visibility":"list"},
            {"slug":"gpt-5.4-pro","display_name":"GPT-5.4 Pro","priority":5,"visibility":"list","supported_reasoning_levels":[{"effort":"medium"},{"effort":"high"},{"effort":"xhigh"}]},
            {"slug":"codex/mini beta","display_name":null,"visibility":"list"},
            null,
            {"slug":7}
        ]})),
        "https://api.openai.com/v1/models" => listed(json!({"data":[
            {"id":"gpt-6-luna","created":2},{"id":"gpt-realtime-6","created":3},{"id":"text-embedding-4","created":4},{"id":"o5-mini","created":1},
            {"id":"gpt-6-astra","created":5},{"id":"gpt-5.4-pro","created":"6"},{"id":"gpt-5-nano","created":null},{"id":"gpt-image-2","created":7},
            {"id":"gpt-4o-mini-tts","created":8},{"id":"gpt-4o-transcribe"},{"id":"gpt-5-search-api"},{"id":"omni-moderation-latest"},
            {"id":"gpt-3.5-turbo-instruct"},{"id":"dall-e-4"},{"id":"whisper-2"},{"id":"chatgpt-4o-latest"},{"id":"o3","created":0},
            {"id":"gpt-4.1"},{"id":"ft:gpt-4.1:acme::x1"},{"id":7}
        ]})),
        "https://api.anthropic.com/v1/models?limit=100" => listed(json!({"data":[
            {"id":"claude-sonnet-5-5","display_name":"Claude Sonnet 5.5","capabilities":{"effort":{"supported":true,"low":{"supported":true},"medium":{"supported":true},"high":{"supported":true},"xhigh":{"supported":true},"max":{"supported":true}}}},
            {"id":"claude-opus-4-5","display_name":"Claude Opus 4.5","capabilities":{"effort":{"supported":true,"low":{"supported":true},"medium":{"supported":true},"high":{"supported":true}}}},
            {"id":"claude-haiku-4-5","display_name":"Claude Haiku 4.5","capabilities":{"effort":{"supported":false}}},
            {"id":"claude-fable-5-1","display_name":"Claude Fable 5.1"},
            {"id":"claude-haiku-5","display_name":" Claude Haiku 5 ✦ "},
            {"id":"claude-mythos-1","display_name":""}
        ]})),
        "https://opencode.ai/zen/v1/models" => listed(json!({"data":[
            {"id":"claude-sonnet-5-5"},{"id":"gpt-5.5"},{"id":"grok-4"},{"id":"gemini-3.5-pro"},{"id":"kimi-k2.6"},{"id":"glm-5"},
            {"id":"muse-1"},{"id":"qwen3-coder 480b"},{"id":"deepseek/v4"},{"id":"claude-haiku-4-5"},{"id":"big-pickle"}
        ]})),
        "https://opencode.ai/zen/go/v1/models" => listed(json!({"data":[
            {"id":"glm-5"},{"id":"kimi-k2.6"},{"id":"gemini-3-flash"},{"id":"minimax-m2.5"},{"id":"gpt-5.5"}
        ]})),
        "http://127.0.0.1:11434/api/tags" => listed(json!({"models":[
            {"name":"qwen3:8b","model":"qwen3:8b","details":{"parameter_size":"8.2B","quantization_level":"Q4_K_M"}},
            {"name":"nomic-embed-text:latest","details":{"parameter_size":"137M"}},
            {"name":"llama3.2:1b","details":{"parameter_size":"1.2B","quantization_level":"Q8_0"}},
            {"name":null,"model":"gemma3:27b","details":{"quantization_level":"Q4_K_M"}},
            {"name":"hf.co/unsloth/Qwen3-30B:Q4_K_M","details":{}}
        ]})),
        "http://127.0.0.1:11434/api/ps" => listed(json!({"models":[{"name":"gemma3:27b"}]})),
        "http://127.0.0.1:11434/api/show" => match ollama_show(body["model"].as_str().unwrap_or("")) {
            Value::Null => Ok(Response::json_response(404, json!({"error":"model not found"}))),
            show => listed(show),
        },
        "http://127.0.0.1:1234/api/v1/models" => listed(lm_studio_models()),
        "http://192.168.1.30:8000/v1/models" => {
            listed(json!({"object":"list","data":[{"id":"gpt-local","object":"model"},{"id":"embed-local","object":"model"}]}))
        }
        "https://claude-proxy.example.test/v1/models" => {
            listed(json!({"data":[{"id":"claude-local","display_name":"Claude Local","type":"model"}],"has_more":false}))
        }
        "http://192.168.1.20:8080/v1/models" => listed(json!({"data":[
            {"id":"llama-4","owned_by":"me","context_length":65536},{"id":"text-embed-3"},{"id":"qwen/qwen3-coder","max_model_len":131072},{"id":"Odd Name ✦"}
        ]})),
        _ => Ok(rejection()),
    }
}

#[tokio::test]
async fn every_provider_and_server_lists_its_models_from_a_small_answer() {
    LocalSet::new()
        .run_until(async {
            let (_dir, store) = store();
            signed_in_to_chatgpt(&store).await;
            let (fetch, _) = recorder(answer);
            let mut lists = Map::new();
            for id in ["openai-codex", "openai", "anthropic", "opencode", "opencode-go"] {
                let options = ListOptions { store: store.clone(), env: Some(keys()), fetch: Some(fetch.clone()), signal: None };
                let models = list_models(provider(id), options).await.unwrap();
                lists.insert(id.into(), serde_json::to_value(models).unwrap());
            }
            for server in servers() {
                let models = list_local_models(&server, Transport { fetch: Some(fetch.clone()), signal: None }).await.unwrap();
                lists.insert(server.id.clone(), serde_json::to_value(models).unwrap());
            }
            golden("models.json", &Value::Object(lists));
        })
        .await;
}

/// One turn: a call to a tool the request lists, and its result with a picture of the arrangement in it.
fn turn() -> ModelRequest {
    let messages: Vec<Message> = serde_json::from_value(json!([
        {"role":"user","content":[{"type":"text","text":"What's the tempo? Show me the arrangement."}]},
        {"role":"assistant","content":[{"type":"tool-call","toolCallId":"call_1","toolName":"get_tempo","input":{"view":"arrangement"}}]},
        {"role":"tool","content":[{"type":"tool-result","toolCallId":"call_1","toolName":"get_tempo","output":{"type":"content","value":[
            {"type":"text","text":"Tempo 120"},
            {"type":"file","data":{"type":"data","data":{"0":255,"1":216,"2":255}},"mediaType":"image/jpeg"}
        ]}}]}
    ]))
    .unwrap();
    ModelRequest {
        instructions: "fixture instructions".into(),
        messages,
        tools: vec![FunctionTool::new(
            "get_tempo",
            "The Set's tempo, and a picture of a view",
            json!({"type":"object","properties":{"view":{"type":"string"}}}),
        )],
        session_id: "session-1".into(),
    }
}

#[tokio::test]
async fn every_route_sends_one_turn_with_a_tool_an_effort_and_a_picture_as_it_did() {
    LocalSet::new()
        .run_until(async {
            let (_dir, store) = store();
            signed_in_to_chatgpt(&store).await;
            let mut routes = Map::new();
            for model in [
                "openai-codex/gpt-6-astra",
                "openai/gpt-6-luna",
                "anthropic/claude-sonnet-5-5",
                "opencode/gpt-5.5",
                "opencode/grok-4",
                "opencode/muse-1",
                "opencode/claude-sonnet-5-5",
                "opencode/kimi-k2.6",
                "opencode/gemini-3.5-pro",
                "opencode-go/gpt-5.5",
                "opencode-go/claude-sonnet-5-5",
                "opencode-go/glm-5",
                "gateway/fixture-model",
            ] {
                let (fetch, seen) = recorder(|_, _| Ok(rejection()));
                // Every route is offered the faster tier: only the ones that ask for it send it.
                let options = ResolveModelOptions {
                    model: model.into(),
                    store: store.clone(),
                    env: Some(keys()),
                    fetch: Some(fetch),
                    effort: Some(Effort::High),
                    service_tier: Some("priority".into()),
                };
                let route = match resolve_model(options).await {
                    Ok(binding) => {
                        let result = binding.model.do_stream((binding.prepare)(turn())).await;
                        assert!(result.is_err(), "{model}: the fixture turns every request down");
                        json!({"requests": seen.borrow().clone()})
                    }
                    Err(error) => json!({"refused": error.to_string(), "requests": seen.borrow().clone()}),
                };
                routes.insert(model.into(), route);
            }
            for (id, model) in [("ollama", "qwen3:8b"), ("lmstudio", "qwen/qwen3-8b"), ("my-server", "llama-4")] {
                let (fetch, seen) = recorder(|url, body| {
                    if url.ends_with("/chat/completions") || url.ends_with("/api/chat") {
                        return Ok(rejection());
                    }
                    answer(url, body)
                });
                let local = resolve_local_model(
                    server(id),
                    model.into(),
                    LocalModelOptions { fetch: Some(fetch), effort: Some(Effort::High), on_note: None },
                );
                let result = local.binding.model.do_stream((local.binding.prepare)(turn())).await;
                assert!(result.is_err(), "{id}: the fixture turns the chat down");
                routes.insert(format!("{id}/{model}"), json!({"requests": seen.borrow().clone()}));
            }
            for (id, model) in [("responses-box", "gpt-local"), ("claude-proxy", "claude-local")] {
                let (fetch, seen) = recorder(|_, _| Ok(rejection()));
                let binding = resolve_server_model(
                    &server(id),
                    model.into(),
                    LocalModelOptions { fetch: Some(fetch), effort: Some(Effort::High), on_note: None },
                );
                assert!(binding.model.do_stream((binding.prepare)(turn())).await.is_err(), "{id}: the fixture turns the call down");
                routes.insert(format!("{id}/{model}"), json!({"requests": seen.borrow().clone()}));
            }
            golden("requests.json", &Value::Object(routes));
        })
        .await;
}

#[tokio::test]
async fn every_api_key_is_accepted_refused_or_unreachable_as_its_provider_answers() {
    let mut checks = Map::new();
    for id in ["openai", "anthropic", "opencode", "opencode-go"] {
        let mut verdicts = Map::new();
        let mut request = Value::Null;
        for (answered, response) in [
            ("200", Some((200, json!({"data":[]})))),
            ("401", Some((401, json!({"error":{"message":"invalid key"}})))),
            ("403", Some((403, json!({"error":{"message":"forbidden"}})))),
            ("429", Some((429, json!({"error":{"message":"slow down"}})))),
            ("500", Some((500, json!({"error":{"message":"down"}})))),
            ("no answer", None),
        ] {
            let (fetch, seen) = recorder(move |_, _| match &response {
                Some((status, body)) => Ok(Response::json_response(*status, body.clone())),
                None => Err(LanguageModelError::other("fetch failed")),
            });
            let verdict = check_api_key(provider(id), KEY, Transport { fetch: Some(fetch), signal: None }).await;
            verdicts.insert(answered.into(), serde_json::to_value(verdict).unwrap());
            request = seen.borrow()[0].clone();
        }
        checks.insert(id.into(), json!({"request": request, "verdicts": verdicts}));
    }
    golden("key-checks.json", &Value::Object(checks));
}
