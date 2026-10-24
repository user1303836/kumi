//! Every provider in the table, the same way: its list from a tiny answer becomes facts, its key check is accepted,
//! refused or unreachable, and one turn's request takes its format's shape with the provider's own extras. The list's
//! facts come from a small catalog of the test's own, so the golden doesn't follow models.dev. The golden is
//! tests/goldens/providers/rows.json; `KUMI_BLESS=1` writes it afresh.
use async_trait::async_trait;
use kumi_ai::{
    auth::store::{open_credential_store, Credential, OAuthCredential},
    binding::ModelRequest,
    error::LanguageModelError,
    http::{Fetch, FetchInit, Headers, Response},
    providers::{
        catalog::{self, Catalog},
        models::{check_api_key, list_models, ListOptions, Transport},
        resolve_model,
        table::{rows, Listing, ProviderRow},
        Effort, ProviderId, ResolveModelOptions,
    },
    types::*,
};
use kumi_common::time::now_ms;
use serde_json::{json, Map, Value};
use std::{cell::RefCell, collections::HashMap, rc::Rc, sync::Arc, sync::LazyLock};
use tokio::task::LocalSet;

const KEY: &str = "key-fixture-value";
/// What models.dev would say of the fixture's models, for every provider the table names there.
fn fixture_catalog() -> Catalog {
    let models = json!({
        "fixture-chat": {"name":"Fixture Chat","tool_call":true,"reasoning":true,"reasoning_options":[{"type":"effort","values":["low","high"]}],"limit":{"context":200000,"output":32000}},
        "org/fixture-slash": {"name":"Fixture Slash","tool_call":true},
        "fixture-quiet": {"name":"Fixture Quiet","tool_call":false},
        "fixture-image": {"name":"Fixture Image","type":"image"},
        "fixture-old": {"name":"Fixture Old","tool_call":true,"status":"deprecated"}
    });
    let providers: Map<String, Value> =
        rows().filter_map(|row| row.catalog).map(|provider| (provider.to_string(), json!({"models": models.clone()}))).collect();
    Catalog::read(&Value::Object(providers))
}
/// Each list's own shape, naming the fixture's models (and one the catalog doesn't know).
fn list_answer(row: &ProviderRow) -> Value {
    let ids = ["fixture-chat", "org/fixture-slash", "fixture-quiet", "fixture-image", "fixture-old", "fixture-unknown"];
    match row.list.1 {
        Listing::Chatgpt => {
            json!({"models": ids.iter().map(|id| json!({"slug":id,"visibility":"list","supported_reasoning_levels":[{"effort":"medium"}]})).collect::<Vec<_>>()})
        }
        Listing::Anthropic => {
            json!({"data": ids.iter().map(|id| json!({"id":id,"display_name":format!("Listed {id}")})).collect::<Vec<_>>()})
        }
        Listing::Gemini => {
            json!({"object":"list","data": ids.iter().map(|id| json!({"id":format!("models/{id}"),"object":"model"})).collect::<Vec<_>>()})
        }
        Listing::OpenRouter => json!({"data":[
            {"id":"org/fixture-slash","name":"Org: Fixture Slash","context_length":131072,"architecture":{"input_modalities":["text","image","file"],"output_modalities":["text"]},"pricing":{"prompt":"0.000001","completion":"0.000004"},"top_provider":{"max_completion_tokens":16384},"supported_parameters":["tools","reasoning"],"reasoning":{"supported_efforts":["high","low"],"default_effort":"low"},"expiration_date":null},
            {"id":"org/fixture-quiet","name":"Org: Fixture Quiet","context_length":8192,"architecture":{"input_modalities":["text"],"output_modalities":["text"]},"pricing":{"prompt":"0","completion":"0"},"supported_parameters":["max_tokens"],"expiration_date":"2026-12-31"},
            {"id":"org/fixture-image","name":"Org: Fixture Image","architecture":{"input_modalities":["text"],"output_modalities":["image"]}}
        ]}),
        Listing::Venice => json!({"data":[
            {"id":"fixture-chat","type":"text","model_spec":{"name":"Fixture Chat","availableContextTokens":131072,"maxCompletionTokens":8192,"privacy":"private","pricing":{"input":{"usd":0.5},"output":{"usd":2}},"capabilities":{"supportsFunctionCalling":true,"supportsReasoning":true,"supportsReasoningEffort":true,"reasoningEffortOptions":["none","high"],"defaultReasoningEffort":"high","supportsVision":true},"offline":false}},
            {"id":"fixture-quiet","type":"text","model_spec":{"name":"Fixture Quiet","privacy":"anonymized","capabilities":{"supportsFunctionCalling":false},"deprecation":{"date":"2026-11-01T00:00:00.000Z"},"offline":false}},
            {"id":"fixture-gone","type":"text","model_spec":{"name":"Fixture Gone","capabilities":{},"offline":true}}
        ]}),
        Listing::HuggingFace => json!({"object":"list","data":[
            {"id":"org/fixture-slash","architecture":{"input_modalities":["text","image"]},"providers":[{"provider":"one","status":"live","context_length":32768,"supports_tools":false},{"provider":"two","status":"live","context_length":65536,"supports_tools":true}]},
            {"id":"org/fixture-quiet","architecture":{"input_modalities":["text"]},"providers":[{"provider":"one","status":"live","supports_tools":false}]}
        ]}),
        Listing::OpenaiChat | Listing::Ids => {
            json!({"data": ids.iter().enumerate().map(|(n, id)| json!({"id":id,"created":n})).collect::<Vec<_>>()})
        }
    }
}
/// The headers as sent, Kumi's user agent without its version and this computer's system.
fn masked(headers: &Headers) -> Value {
    static AGENT: LazyLock<regex::Regex> = LazyLock::new(|| regex::Regex::new(r"kumi/\S+ \([^)]*\)").unwrap());
    Value::Object(
        headers
            .iter()
            .map(|(name, value)| {
                (
                    name.clone(),
                    json!(if name == "user-agent" {
                        AGENT.replace_all(value, "kumi/<version> (<system>)").into_owned()
                    } else {
                        value.clone()
                    }),
                )
            })
            .collect(),
    )
}
struct Server {
    seen: RefCell<Vec<Value>>,
    answer: Box<dyn Fn(&str) -> Result<Response, LanguageModelError>>,
}
#[async_trait(?Send)]
impl Fetch for Server {
    async fn fetch(&self, url: &str, init: FetchInit) -> Result<Response, LanguageModelError> {
        let mut seen = Map::new();
        seen.insert("url".into(), json!(url));
        seen.insert("headers".into(), masked(&init.headers));
        if let Some(body) = init.body.as_deref() {
            seen.insert("body".into(), serde_json::from_str(body).unwrap_or_else(|_| json!(body)));
        }
        self.seen.borrow_mut().push(Value::Object(seen));
        (self.answer)(url)
    }
}
fn server(answer: impl Fn(&str) -> Result<Response, LanguageModelError> + 'static) -> Rc<Server> {
    Rc::new(Server { seen: RefCell::new(vec![]), answer: Box::new(answer) })
}
fn golden(name: &str, value: &Value) {
    let file = kumi_testkit::workspace_path("crates/kumi-ai/tests/goldens/providers").join(name);
    let mut actual = serde_json::to_string_pretty(value).unwrap();
    actual.push('\n');
    if std::env::var_os("KUMI_BLESS").is_some() {
        std::fs::write(&file, actual).unwrap();
        return;
    }
    let expected =
        std::fs::read_to_string(&file).unwrap_or_else(|_| panic!("{} is missing: run this test with KUMI_BLESS=1", file.display()));
    let line = expected.lines().zip(actual.lines()).position(|(a, b)| a != b).map(|n| n + 1);
    assert!(actual == expected, "{} changed (first at line {line:?}): check it, then run this test with KUMI_BLESS=1", file.display());
}
fn turn() -> ModelRequest {
    let messages: Vec<Message> = serde_json::from_value(json!([
        {"role":"user","content":[{"type":"text","text":"What's the tempo?"}]},
        {"role":"assistant","content":[{"type":"tool-call","toolCallId":"call_1","toolName":"get_tempo","input":{}}]},
        {"role":"tool","content":[{"type":"tool-result","toolCallId":"call_1","toolName":"get_tempo","output":{"type":"text","value":"Tempo 120"}}]}
    ]))
    .unwrap();
    ModelRequest {
        instructions: "fixture instructions".into(),
        messages,
        tools: vec![FunctionTool::new("get_tempo", "The Set's tempo", json!({"type":"object","properties":{}}))],
        session_id: "session-1".into(),
    }
}

#[tokio::test]
async fn every_row_lists_its_models_checks_a_key_and_sends_a_turn_in_its_format() {
    catalog::install(Arc::new(fixture_catalog()));
    LocalSet::new()
        .run_until(async {
            let folder = tempfile::tempdir().unwrap();
            let store = Rc::new(open_credential_store(folder.path().join("auth.json")));
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
            let mut all = Map::new();
            for row in rows() {
                let provider = ProviderId::parse(row.id).unwrap();
                let env: HashMap<String, String> = row.key_env.iter().take(1).map(|name| (name.to_string(), KEY.to_string())).collect();
                // Its list, from the tiny answer: what each model can do, by the list or the catalog.
                let answered = list_answer(row);
                let lister = server(move |_| Ok(Response::json_response(200, answered.clone())));
                let listed = list_models(provider, ListOptions { store: store.clone(), env: Some(env.clone()), fetch: Some(lister.clone()), signal: None })
                    .await
                    .unwrap_or_else(|error| panic!("{}: {error}", row.id));
                let models: Vec<Value> = listed
                    .iter()
                    .map(|model| {
                        json!({"id":model.id,"tools":model.tools,"efforts":model.efforts.iter().map(|e| e.effort.as_str()).collect::<Vec<_>>(),
                            "default":model.default_effort,"status":model.status,"labels":model.labels,"format":model.format,"from":model.facts_from})
                    })
                    .collect();
                // Its key check: accepted, refused, and with no answer.
                let mut checks = Map::new();
                let mut asked = Value::Null;
                if row.key_env.first().is_some() {
                    for (said, status) in [("200", Some(200)), ("401", Some(401)), ("no answer", None)] {
                        let checker = server(move |_| match status {
                            Some(status) => Ok(Response::json_response(status, json!({"data":[]}))),
                            None => Err(LanguageModelError::other("fetch failed")),
                        });
                        let verdict = check_api_key(provider, KEY, Transport { fetch: Some(checker.clone()), signal: None }).await;
                        checks.insert(said.into(), json!(verdict));
                        asked = checker.seen.borrow()[0].clone();
                    }
                }
                // One turn with the first model listed, at high effort.
                let sender = server(|_| Ok(Response::json_response(400, json!({"error":{"message":"fixture rejection"}}))));
                let model = format!("{}/{}", row.id, listed[0].model);
                let options = ResolveModelOptions {
                    model: model.clone(),
                    store: store.clone(),
                    env: Some(env.clone()),
                    fetch: Some(sender.clone()),
                    effort: Some(Effort::High),
                    service_tier: None,
                };
                let binding = resolve_model(options).await.unwrap_or_else(|error| panic!("{model}: {error}"));
                assert!(binding.model.do_stream((binding.prepare)(turn())).await.is_err());
                all.insert(
                    row.id.into(),
                    json!({
                        "list": {"request": lister.seen.borrow()[0], "models": models},
                        "keyCheck": {"request": asked, "verdicts": checks},
                        "turn": {"model": model, "request": sender.seen.borrow().last().cloned()}
                    }),
                );
            }
            golden("rows.json", &Value::Object(all));
        })
        .await;
    catalog::forget();
}
