//! Local discovery/binding scenarios.
use async_trait::async_trait;
use kumi_common::{abort::Signal, js::json::stringify};
use kumi_runtime::{
    ai::{
        error::LanguageModelError,
        http::{Fetch, FetchInit, Headers, Response},
    },
    core::{
        contracts::{JsonObject, KernelEvent, KernelTool, ToolResult, TurnResult},
        errors::{FailureKind, KumiError, RuntimeError},
    },
    kernel::agent::{create_agent_kernel, AgentKernelOptions, ModelBinding},
    providers::{local::*, Effort},
};
use serde_json::{json, Value};
use std::{
    cell::{Cell, RefCell},
    collections::HashMap,
    rc::Rc,
};
use tokio::task::LocalSet;
#[derive(Clone)]
struct Seen {
    path: String,
    body: Value,
    headers: Headers,
}
struct Fake {
    seen: RefCell<Vec<Seen>>,
    handle: Box<dyn Fn(&Seen) -> Response>,
}
impl Fake {
    fn new(handle: impl Fn(&Seen) -> Response + 'static) -> Rc<Self> {
        Rc::new(Self { seen: RefCell::new(vec![]), handle: Box::new(handle) })
    }
    fn transport(self: &Rc<Self>) -> Transport {
        Transport { fetch: Some(self.clone()), signal: None }
    }
    fn requests(&self, path: &str) -> Vec<Seen> {
        self.seen.borrow().iter().filter(|r| r.path == path).cloned().collect()
    }
}
#[async_trait(?Send)]
impl Fetch for Fake {
    async fn fetch(&self, url: &str, init: FetchInit) -> Result<Response, LanguageModelError> {
        let request = Seen {
            path: url::Url::parse(url).unwrap().path().into(),
            body: init.body.as_deref().map(|s| serde_json::from_str(s).unwrap()).unwrap_or(json!({})),
            headers: init.headers,
        };
        self.seen.borrow_mut().push(request.clone());
        Ok((self.handle)(&request))
    }
}
fn reply(body: Value) -> Response {
    Response::json_response(200, body)
}
fn ndjson(lines: Vec<Value>) -> Response {
    let mut r = Response::text_response(200, lines.iter().map(|v| format!("{}\n", stringify(v))).collect::<String>());
    r.headers.insert("content-type".into(), "application/x-ndjson".into());
    r
}
fn sse(chunks: Vec<Value>) -> Response {
    let mut r = Response::text_response(
        200,
        format!("{}data: [DONE]\n\n", chunks.iter().map(|v| format!("data: {}\n\n", stringify(v))).collect::<String>()),
    );
    r.headers.insert("content-type".into(), "text/event-stream".into());
    r
}
fn said(content: &str) -> Value {
    json!({"message":{"role":"assistant","content":content},"done":false})
}
fn done() -> Value {
    json!({"message":{"role":"assistant","content":""},"done":true,"done_reason":"stop","prompt_eval_count":900,"eval_count":12})
}
fn delta(value: Value, finish: Option<&str>) -> Value {
    json!({"id":"chatcmpl-1","object":"chat.completion.chunk","choices":[{"index":0,"delta":value,"finish_reason":finish}]})
}
fn ollama(models: Value, chat: impl Fn(&Seen, usize) -> Response + 'static) -> Rc<Fake> {
    let chats = Cell::new(0);
    Fake::new(move |r| match r.path.as_str() {
        "/api/version" => reply(json!({"version":"0.12.6"})),
        "/api/tags" => reply(
            json!({"models":models.as_object().unwrap().keys().map(|name|json!({"name":name,"model":name,"details":{"parameter_size":"8.2B","quantization_level":"Q4_K_M"}})).collect::<Vec<_>>()}),
        ),
        "/api/ps" => reply(
            json!({"models":models.as_object().unwrap().iter().filter(|(_,v)|v["loaded"]==true).map(|(name,_)|json!({"name":name,"model":name})).collect::<Vec<_>>()}),
        ),
        "/api/show" => {
            let model = &models[r.body["model"].as_str().unwrap()];
            if model.is_null() {
                return Response::json_response(404, json!({"error":format!("model '{}' not found",r.body["model"].as_str().unwrap())}));
            }
            let mut show =
                json!({"model_info":{"general.architecture":"qwen3"},"details":{"parameter_size":"8.2B","quantization_level":"Q4_K_M"}});
            if let Some(caps) = model.get("capabilities") {
                show["capabilities"] = caps.clone();
            }
            if let Some(ctx) = model.get("context") {
                show["model_info"]["qwen3.context_length"] = ctx.clone();
            }
            if let Some(think) = model.get("thinking") {
                show["thinking"] = think.clone();
            }
            reply(show)
        }
        "/api/chat" => {
            chats.set(chats.get() + 1);
            chat(r, chats.get())
        }
        _ => Response::json_response(404, json!({"error":"not found"})),
    })
}
fn ollama_at(url: &str) -> LocalServer {
    local_servers(&[], &[("OLLAMA_HOST".into(), url.into())].into()).unwrap().remove(0)
}
fn lm_at() -> LocalServer {
    LocalServer {
        id: "lmstudio".into(),
        kind: LocalKind::Lmstudio,
        name: "LM Studio".into(),
        base_url: "http://127.0.0.1:1234/v1".into(),
        api_key: None,
        r#where: "on this computer".into(),
    }
}
fn custom() -> LocalServer {
    local_servers(&[ServerSetting { name: "llama.cpp".into(), base_url: "http://127.0.0.1:8080".into(), api_key: None }], &HashMap::new())
        .unwrap()
        .remove(2)
}
struct Tool {
    name: String,
    description: String,
    schema: Value,
    calls: Rc<RefCell<Vec<Value>>>,
}
#[async_trait(?Send)]
impl KernelTool for Tool {
    fn name(&self) -> &str {
        &self.name
    }
    fn description(&self) -> &str {
        &self.description
    }
    fn input_schema(&self) -> JsonObject {
        self.schema.as_object().unwrap().clone()
    }
    async fn execute(&self, input: JsonObject, _: Signal) -> Result<ToolResult, RuntimeError> {
        self.calls.borrow_mut().push(Value::Object(input));
        Ok(ToolResult::text("{\"tempo\":120}"))
    }
}
fn tempo(calls: Rc<RefCell<Vec<Value>>>) -> Rc<dyn KernelTool> {
    Rc::new(Tool {
        name: "get_tempo".into(),
        description: "The Set's tempo".into(),
        schema: json!({"type":"object","properties":{"precise":{"type":"boolean"}}}),
        calls,
    })
}
fn catalog() -> Vec<Rc<dyn KernelTool>> {
    (0..70).map(|i|Rc::new(Tool{name:format!("change_{i}"),description:format!("A change to Live. {}","It says what it changes, what it takes, and what it leaves alone. ".repeat(13)),schema:json!({"type":"object","properties":{"ref":{"type":"string","description":"What to change, by its reference from the Set"},"value":{"type":"number"}},"required":["ref"]}),calls:Default::default()})as Rc<dyn KernelTool>).collect()
}
fn instructions() -> String {
    "Kumi's instructions, at their length. ".repeat(430)
}
fn fixed(instructions: &str, tools: &[Rc<dyn KernelTool>]) -> usize {
    instructions.len()
        + stringify(&json!(tools
            .iter()
            .map(|t| json!({"type":"function","name":t.name(),"description":t.description(),"inputSchema":t.input_schema()}))
            .collect::<Vec<_>>()))
        .len()
}
async fn run(binding: Rc<LocalBinding>, instructions: &str, tools: Vec<Rc<dyn KernelTool>>) -> Result<(String, TurnResult), RuntimeError> {
    let prepare = binding.clone();
    let budget = binding.clone();
    let kernel = create_agent_kernel(AgentKernelOptions {
        instructions: instructions.into(),
        tools,
        signal: Signal::new(),
        checkpoint: None,
        binding: ModelBinding {
            id: binding.binding.id.clone(),
            model: binding.binding.model.clone(),
            prepare: Box::new(move |r| (prepare.binding.prepare)(r)),
            budget: Some(Box::new(move |size| budget.binding.budget.as_ref().unwrap()(size))),
        },
        max_steps: None,
        budget: None,
    })
    .unwrap();
    let words = Rc::new(RefCell::new(String::new()));
    let out = words.clone();
    let result = kernel
        .run(
            "What's the tempo?",
            Signal::new(),
            Rc::new(move |event| {
                if let KernelEvent::Text { text } = event {
                    out.borrow_mut().push_str(&text);
                }
                Ok(())
            }),
        )
        .await;
    kernel.close().await;
    let text = words.borrow().clone();
    result.map(|result| (text, result))
}
fn binding(server: LocalServer, model: &str, fetch: Option<Rc<dyn Fetch>>, effort: Option<Effort>) -> Rc<LocalBinding> {
    Rc::new(resolve_local_model(server, model.into(), LocalModelOptions { fetch, effort, on_note: None }))
}
async fn failed(binding: Rc<LocalBinding>, large: bool) -> KumiError {
    let prompt = if large { instructions() } else { "fixture instructions".into() };
    run(binding, &prompt, if large { catalog() } else { vec![tempo(Default::default())] })
        .await
        .err()
        .expect("answer failed")
        .kumi()
        .expect("Kumi failure")
        .clone()
}
#[test]
fn discovery_addresses_and_ids() {
    let address = |host: Option<&str>| {
        local_servers(&[], &host.map(|h| [("OLLAMA_HOST".into(), h.into())].into()).unwrap_or_default()).unwrap().remove(0).base_url
    };
    for (host, want) in [
        (None, "http://127.0.0.1:11434"),
        (Some("0.0.0.0"), "http://127.0.0.1:11434"),
        (Some("0.0.0.0:8000"), "http://127.0.0.1:8000"),
        (Some("http://studio.local:11434/"), "http://studio.local:11434"),
        (Some("studio.local"), "http://studio.local:11434"),
        (Some("https://ollama.example.test"), "https://ollama.example.test"),
        (Some("http://localhost:80"), "http://localhost"),
        (Some("[::]"), "http://[::1]:11434"),
    ] {
        assert_eq!(address(host), want);
    }
    let settings:Vec<ServerSetting>=serde_json::from_value(json!([{"name":"llama.cpp","baseURL":"http://127.0.0.1:8080"},{"name":"Studio PC","baseURL":"http://192.168.1.20:8000/v1/","apiKey":"lan-token"},{"name":"OpenAI","baseURL":"http://127.0.0.1:9000/v1"},{"name":"llama.cpp","baseURL":"http://127.0.0.1:8081/v1"}])).unwrap();
    let servers = local_servers(&settings, &HashMap::new()).unwrap();
    assert_eq!(
        json!(servers.iter().map(|s| json!([s.id, s.base_url, s.r#where])).collect::<Vec<_>>()),
        json!([
            ["ollama", "http://127.0.0.1:11434", "on this computer"],
            ["lmstudio", "http://127.0.0.1:1234/v1", "on this computer"],
            ["llama-cpp", "http://127.0.0.1:8080/v1", "on this computer"],
            ["studio-pc", "http://192.168.1.20:8000/v1", "on 192.168.1.20"],
            ["openai-2", "http://127.0.0.1:9000/v1", "on this computer"],
            ["llama-cpp-2", "http://127.0.0.1:8081/v1", "on this computer"]
        ])
    );
    assert_eq!(servers[3].api_key.as_deref(), Some("lan-token"));
    assert_eq!(parse_local_model_id("ollama/qwen3:8b", &servers).unwrap().model, "qwen3:8b");
    assert_eq!(parse_local_model_id("lmstudio/qwen/qwen3-8b", &servers).unwrap().model, "qwen/qwen3-8b");
    assert_eq!(parse_local_model_id("llama-cpp-2/My Model Q4.gguf", &servers).unwrap().server.base_url, "http://127.0.0.1:8081/v1");
    for id in ["anthropic/claude-sonnet-5", "nowhere/model", "ollama/", "ollama/ spaced", "ollama/bad\u{7}"] {
        assert!(parse_local_model_id(id, &servers).is_none(), "{id}");
    }
}
#[tokio::test]
async fn ollama_catalog_capabilities_and_probe() {
    let fake = ollama(
        json!({"qwen3:8b":{"capabilities":["completion","tools","thinking"],"context":40960,"loaded":true},"gemma3:4b":{"capabilities":["completion","vision"],"context":131072},"gpt-oss:20b":{"capabilities":["completion","tools","thinking"],"context":131072,"thinking":{"values":["low","medium","high"],"default":"medium"}},"nomic-embed-text":{"capabilities":["embedding"]}}),
        |_, _| ndjson(vec![]),
    );
    let server = ollama_at("127.0.0.1");
    assert!(probe_local(&server, fake.transport()).await);
    let models = list_local_models(&server, fake.transport()).await.unwrap();
    assert_eq!(
        json!(models.iter().map(|m| &m.id).collect::<Vec<_>>()),
        json!(["ollama/qwen3:8b", "ollama/gemma3:4b", "ollama/gpt-oss:20b"])
    );
    assert_eq!(
        serde_json::to_value(&models[0]).unwrap(),
        json!({"id":"ollama/qwen3:8b","provider":"ollama","model":"qwen3:8b","name":"qwen3:8b","description":"8.2B · Q4_K_M · loaded","efforts":[],"tools":true,"context":40960.0,"loaded":true,"where":"on this computer"})
    );
    assert_eq!(models[1].description.as_deref(), Some("8.2B · Q4_K_M · can't change the Set"));
    assert_eq!(models[1].tools, Some(false));
    assert_eq!(models[2].efforts.iter().map(|e| e.effort).collect::<Vec<_>>(), vec![Effort::Low, Effort::Medium, Effort::High]);
    assert_eq!(models[2].default_effort, Some(Effort::Medium));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    drop(listener);
    assert!(!probe_local(&ollama_at(&format!("http://{address}")), Transport::default()).await);
}
#[tokio::test]
async fn ollama_kernel_tool_roundtrip() {
    LocalSet::new().run_until(async{let fake=ollama(json!({"qwen3:8b":{"capabilities":["completion","tools","thinking"],"context":40960}}),|_,n|if n==1{ndjson(vec![json!({"message":{"role":"assistant","content":"","thinking":"The tempo first."},"done":false}),json!({"message":{"role":"assistant","content":"","tool_calls":[{"function":{"name":"get_tempo","arguments":{"precise":true}}}]},"done":false}),done()])}else{let mut finish=done();
finish["prompt_eval_count"]=json!(40);
finish["prompt_eval_cached_count"]=json!(900);
finish["eval_count"]=json!(5);
ndjson(vec![said("It's "),said("120 BPM."),finish])});
let calls=Rc::new(RefCell::new(vec![]));
let (text,result)=run(binding(ollama_at("127.0.0.1"),"qwen3:8b",Some(fake.clone()),None),"fixture instructions",vec![tempo(calls.clone())]).await.unwrap();
assert_eq!(text,"It's 120 BPM.");
assert_eq!(*calls.borrow(),vec![json!({"precise":true})]);
let usage=result.usage.unwrap();
assert_eq!(usage.input_tokens,1840.);
assert_eq!(usage.cache_read_tokens,900.);
let chats=fake.requests("/api/chat");
let first=&chats[0];
let second=&chats[1].body;
assert_eq!(first.body["model"],"qwen3:8b");
assert_eq!(first.body["stream"],true);
assert_eq!(first.body["truncate"],false);
assert_eq!(first.body["shift"],false);
assert_eq!(first.body["think"],true);
assert_eq!(first.body["options"]["num_ctx"],32768);
assert_eq!(first.body["messages"][0],json!({"role":"system","content":"fixture instructions"}));
assert_eq!(first.body["tools"],json!([{"type":"function","function":{"name":"get_tempo","description":"The Set's tempo","parameters":{"type":"object","properties":{"precise":{"type":"boolean"}}}}}]));
assert!(first.headers["user-agent"].starts_with("kumi/"));
let call=second["messages"].as_array().unwrap().iter().find(|m|m["role"]=="assistant").unwrap();
assert_eq!(call["thinking"],"The tempo first.");
assert_eq!(call["tool_calls"][0]["function"],json!({"name":"get_tempo","arguments":{"precise":true}}));
assert_eq!(second["messages"].as_array().unwrap().iter().find(|m|m["role"]=="tool").unwrap(),&json!({"role":"tool","content":"{\"tempo\":120}","tool_name":"get_tempo","tool_call_id":call["tool_calls"][0]["id"]}));
assert_eq!(second["options"]["num_ctx"],32768);
}).await;
}
#[tokio::test]
async fn full_requests_get_room_and_short_models_are_refused() {
    LocalSet::new().run_until(async{let fixed=fixed(&instructions(),&catalog());
assert!(fixed>80*1024&&fixed<95*1024);
let fake=ollama(json!({"big:latest":{"capabilities":["completion","tools"],"context":131072},"short:latest":{"capabilities":["completion","tools"],"context":8192}}),|_,_|ndjson(vec![said("Ready."),done()]));
let big=binding(ollama_at("127.0.0.1"),"big:latest",Some(fake.clone()),None);
run(big.clone(),&instructions(),catalog()).await.unwrap();
let asked=fake.requests("/api/chat")[0].body["options"]["num_ctx"].as_f64().unwrap();
assert_eq!(asked,context_for(fixed as f64,None));
assert_eq!(asked,57344.);
let budget=big.binding.budget.as_ref().unwrap()(fixed);
assert!(budget.limit>=48.*1024.&&budget.limit<64.*1024.&&budget.clear_at<budget.limit);
let short=failed(binding(ollama_at("127.0.0.1"),"short:latest",Some(fake.clone()),None),true).await;
assert_eq!(short.kind,FailureKind::Model);
assert_eq!(short.message,"short:latest reads at most 8,192 tokens at once, too few for Kumi's instructions and tools (about 24,000): choose a model that reads more with /model.");
assert_eq!(fake.requests("/api/chat").len(),1);
}).await;
}
#[tokio::test]
async fn tool_less_models_talk_and_name_an_alternative() {
    LocalSet::new().run_until(async{let fake=ollama(json!({"gemma3:4b":{"capabilities":["completion","vision"]},"qwen3:8b":{"capabilities":["completion","tools"],"loaded":true}}),|_,_|ndjson(vec![said("Your Set has two tracks."),done()]));
let chosen=binding(ollama_at("127.0.0.1"),"gemma3:4b",Some(fake.clone()),None);
let (text,_)=run(chosen.clone(),"fixture instructions",vec![tempo(Default::default())]).await.unwrap();
assert_eq!(text,"Your Set has two tracks.");
assert_eq!(chosen.note().as_deref(),Some("gemma3:4b can't use tools, so Kumi can talk with it about your Set but can't change anything. qwen3:8b on Ollama can: /model chooses it."));
let request=&fake.requests("/api/chat")[0].body;
assert!(request.get("tools").is_none());
assert!(request["messages"][0]["content"].as_str().unwrap().starts_with("fixture instructions\n\nThis model can't use tools here"));
assert!(request.get("think").is_none());
let alone=ollama(json!({"gemma3:4b":{"capabilities":["completion"]}}),|_,_|ndjson(vec![]));
let chosen=binding(ollama_at("127.0.0.1"),"gemma3:4b",Some(alone),None);
chosen.asked.clone().await;
assert!(chosen.note().unwrap().ends_with("None of Ollama's models can; pull one that can use tools, then choose it with /model."));
}).await;
}
#[tokio::test]
async fn effort_is_only_sent_when_ollama_offers_it() {
    LocalSet::new().run_until(async{let fake=ollama(json!({"gpt-oss:20b":{"capabilities":["completion","tools","thinking"],"thinking":{"values":["low","medium","high"],"default":"medium"}},"qwen3:8b":{"capabilities":["completion","tools","thinking"],"thinking":{"values":[false,true],"default":true}}}),|_,_|ndjson(vec![said("Ok."),done()]));
for (name,effort) in [("gpt-oss:20b",Some(Effort::High)),("gpt-oss:20b",None),("qwen3:8b",Some(Effort::High))]{run(binding(ollama_at("127.0.0.1"),name,Some(fake.clone()),effort),"fixture instructions",vec![tempo(Default::default())]).await.unwrap();
}assert_eq!(json!(fake.requests("/api/chat").iter().map(|r|&r.body["think"]).collect::<Vec<_>>()),json!(["high","medium",true]));
}).await;
}
struct Down;
#[async_trait(?Send)]
impl Fetch for Down {
    async fn fetch(&self, _: &str, _: FetchInit) -> Result<Response, LanguageModelError> {
        Err(LanguageModelError::other("fetch failed"))
    }
}
#[tokio::test]
async fn ollama_failures_explain_start_pull_memory_and_crash() {
    LocalSet::new().run_until(async {
    let down=failed(binding(ollama_at("127.0.0.1"),"qwen3:8b",Some(Rc::new(Down)),None),false).await;
    assert_eq!(down.kind,FailureKind::Network);
assert_eq!(down.message,"Ollama isn't running: open it, or run `ollama serve`, then send your message again.");
assert_eq!(down.provider.as_deref(),Some("ollama"));
    let away=failed(binding(ollama_at("studio.invalid"),"qwen3:8b",Some(Rc::new(Down)),None),false).await;
    assert_eq!(away.message,"Kumi can't reach Ollama at http://studio.invalid:11434: check that it's running on studio.invalid, then send your message again.");
    let fake=ollama(json!({"qwen3:30b":{"capabilities":["completion","tools"]},"qwen3:14b":{"capabilities":["completion","tools"]}}),|r,_|if r.body["model"]=="qwen3:30b"{Response::json_response(500,json!({"error":"model requires more system memory (21.5 GiB) than is available (12.1 GiB)"}))}else{ndjson(vec![said("Half an answ"),json!({"error":"llama runner process has terminated: exit status 2"})])});
    let absent=failed(binding(ollama_at("127.0.0.1"),"llama9:70b",Some(fake.clone()),None),false).await;
assert_eq!(absent.kind,FailureKind::Model);
assert_eq!(absent.message,"Ollama doesn't have llama9:70b: run `ollama pull llama9:70b`, or choose one you have with /model.");
    let memory=failed(binding(ollama_at("127.0.0.1"),"qwen3:30b",Some(fake.clone()),None),false).await;
assert_eq!(memory.kind,FailureKind::Model);
assert_eq!(memory.message,"Ollama couldn't fit qwen3:30b in this computer's memory with the room Kumi needs (32,768 tokens): choose a smaller model with /model, or close other apps and send your message again (model requires more system memory (21.5 GiB) than is available (12.1 GiB)).");
    let crashed=failed(binding(ollama_at("127.0.0.1"),"qwen3:14b",Some(fake),None),false).await;
assert_eq!(crashed.message,"Ollama stopped answering partway (llama runner process has terminated: exit status 2): if it closed, open it again, then send your message again.");
}).await;
}
#[tokio::test]
async fn native_http_socket_drop_is_a_transport_failure() {
    LocalSet::new().run_until(async {
    use tokio::io::{AsyncReadExt,AsyncWriteExt};
    let listener=tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
let address=listener.local_addr().unwrap();
    let serving=tokio::task::spawn_local(async move {for _ in 0..2 {let(mut socket,_)=listener.accept().await.unwrap();
let mut input=vec![];
let mut chunk=[0;
4096];
while !input.windows(4).any(|w|w==b"\r\n\r\n"){let n=socket.read(&mut chunk).await.unwrap();
if n==0{break;
}input.extend_from_slice(&chunk[..n]);
}let request=String::from_utf8_lossy(&input);
if request.starts_with("POST /api/show "){let body=stringify(&json!({"capabilities":["completion","tools"]}));
socket.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len()).as_bytes()).await.unwrap();
}else{socket.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: application/x-ndjson\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n").await.unwrap();
let body=format!("{}\n",stringify(&said("Let me")));
socket.write_all(format!("{:x}\r\n{body}\r\n",body.len()).as_bytes()).await.unwrap();
socket.flush().await.unwrap();
tokio::time::sleep(std::time::Duration::from_millis(20)).await;
}}});
    let dropped=failed(binding(ollama_at(&format!("http://{address}")),"qwen3:8b",None,None),false).await;
    assert_eq!(dropped.kind,FailureKind::Network);
assert_eq!(dropped.message,"Ollama stopped answering partway (it may have quit, or run out of memory): if it closed, open it again, then send your message again.");
serving.await.unwrap();
}).await;
}
fn lm_model(key: &str, extra: Value) -> Value {
    let mut model = json!({"type":"llm","key":key,"display_name":key.rsplit('/').next(),"params_string":"8B","quantization":{"name":"Q4_K_M"},"max_context_length":131072,"loaded_instances":[],"capabilities":{"vision":false,"trained_for_tool_use":true}});
    model.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
    model
}
fn lm_studio(models: Vec<Value>, chat: impl Fn(&Seen, usize) -> Vec<Value> + 'static) -> Rc<Fake> {
    let models = RefCell::new(models);
    let chats = Cell::new(0);
    Fake::new(move |r| match r.path.as_str() {
        "/v1/models" => reply(json!({"data":models.borrow().iter().map(|m|json!({"id":m["key"],"object":"model"})).collect::<Vec<_>>()})),
        "/api/v1/models" => reply(json!({"models":*models.borrow()})),
        "/api/v1/models/unload" => {
            for model in models.borrow_mut().iter_mut() {
                model["loaded_instances"].as_array_mut().unwrap().retain(|c| c["id"] != r.body["instance_id"]);
            }
            reply(json!({"instance_id":r.body["instance_id"]}))
        }
        "/api/v1/models/load" => {
            let mut models = models.borrow_mut();
            let model = models.iter_mut().find(|m| m["key"] == r.body["model"]).unwrap();
            let key = model["key"].clone();
            model["loaded_instances"].as_array_mut().unwrap().push(json!({"id":key,"config":{"context_length":r.body["context_length"]}}));
            reply(json!({"type":"llm","instance_id":key,"status":"loaded","load_config":{"context_length":r.body["context_length"]}}))
        }
        "/v1/chat/completions" => {
            chats.set(chats.get() + 1);
            sse(chat(r, chats.get()))
        }
        _ => Response::json_response(404, json!({"error":"Unexpected endpoint or method."})),
    })
}
#[tokio::test]
async fn lm_studio_lists_then_loads_once_across_tool_roundtrips() {
    LocalSet::new().run_until(async {
    let fake=lm_studio(vec![lm_model("qwen/qwen3-8b",json!({"capabilities":{"trained_for_tool_use":true,"reasoning":{"allowed_options":["off","low","medium","high"],"default":"medium"}}})),lm_model("google/gemma-3-4b",json!({"capabilities":{"trained_for_tool_use":false}})),json!({"type":"embedding","key":"text-embedding-nomic","loaded_instances":[]})],|_,n|if n==1{vec![delta(json!({"role":"assistant","content":""}),None),delta(json!({"tool_calls":[{"index":0,"id":"call_7","type":"function","function":{"name":"get_tempo","arguments":"{}"}}]}),None),delta(json!({}),Some("tool_calls"))]}else{vec![delta(json!({"reasoning_content":"Tempo read."}),None),delta(json!({"content":"120 BPM."}),None),delta(json!({}),Some("stop"))]});
    let models=list_local_models(&lm_at(),fake.transport()).await.unwrap();
assert_eq!(json!(models.iter().map(|m|json!([m.id,m.name,m.description,m.efforts.iter().map(|e|e.effort.as_str()).collect::<Vec<_>>().join(" ")])).collect::<Vec<_>>()),json!([["lmstudio/qwen/qwen3-8b","qwen3-8b","8B · Q4_K_M","low medium high"],["lmstudio/google/gemma-3-4b","gemma-3-4b","8B · Q4_K_M · can't change the Set",""]]));
    let(text,_)=run(binding(lm_at(),"qwen/qwen3-8b",Some(fake.clone()),Some(Effort::Low)),"fixture instructions",vec![tempo(Default::default())]).await.unwrap();
assert_eq!(text,"120 BPM.");
let loads=fake.requests("/api/v1/models/load");
assert_eq!(loads.len(),1);
assert_eq!(loads[0].body,json!({"model":"qwen/qwen3-8b","context_length":32768,"echo_load_config":true}));
let chats=fake.requests("/v1/chat/completions");
assert_eq!(chats.len(),2);
assert_eq!(chats[0].body["model"],"qwen/qwen3-8b");
assert_eq!(chats[0].body["reasoning_effort"],"low");
assert_eq!(chats[0].body["tools"][0]["function"]["name"],"get_tempo");
}).await;
}
#[tokio::test]
async fn lm_studio_reloads_insufficient_context_and_tells_the_producer() {
    LocalSet::new().run_until(async {
    let fake=lm_studio(vec![lm_model("qwen/qwen3-8b",json!({"loaded_instances":[{"id":"qwen/qwen3-8b","config":{"context_length":4096}}]}))],|_,_|vec![delta(json!({"content":"Ok."}),None),delta(json!({}),Some("stop"))]);
let notes=Rc::new(RefCell::new(vec![]));
let out=notes.clone();
let chosen=Rc::new(resolve_local_model(lm_at(),"qwen/qwen3-8b".into(),LocalModelOptions{fetch:Some(fake.clone()),effort:None,on_note:Some(Rc::new(move|note|out.borrow_mut().push(note)))}));
run(chosen,"fixture instructions",vec![tempo(Default::default())]).await.unwrap();
assert_eq!(json!(fake.seen.borrow().iter().filter(|r|r.path.starts_with("/api/v1/models/")).map(|r|json!([r.path,r.body.get("instance_id").unwrap_or(&r.body["context_length"])] )).collect::<Vec<_>>()),json!([["/api/v1/models/unload","qwen/qwen3-8b"],["/api/v1/models/load",32768]]));
assert_eq!(*notes.borrow(),vec!["LM Studio had qwen3-8b loaded with room for 4,096 tokens, too few for Kumi; Kumi loaded it again with room for 32,768."]);
}).await;
}
#[tokio::test]
async fn compatible_quirks_are_mended_through_the_kernel() {
    LocalSet::new()
        .run_until(async {
            let fake = Fake::new(|r| {
                if r.path == "/v1/models" {
                    return reply(json!({"object":"list","data":[{"id":"qwen3-8b-q4.gguf","object":"model","owned_by":"llamacpp"}]}));
                }
                if r.path == "/props" {
                    return reply(json!({"default_generation_settings":{"n_ctx":65536}}));
                }
                sse(if r.body["messages"].as_array().unwrap().len() <= 2 {
                    vec![
                        delta(json!({"role":"assistant","content":"<think>"}), None),
                        delta(json!({"content":"Tempo first.</th"}), None),
                        delta(json!({"content":"ink>\n\n"}), None),
                        delta(json!({"tool_calls":[{"index":0,"function":{"name":"get_tempo","arguments":{"precise":false}}}]}), None),
                    ]
                } else {
                    vec![delta(json!({"content":"It's 120."}), None)]
                })
            });
            let calls = Rc::new(RefCell::new(vec![]));
            let chosen = binding(custom(), "qwen3-8b-q4.gguf", Some(fake.clone()), None);
            let (text, _) = run(chosen.clone(), "fixture instructions", vec![tempo(calls.clone())]).await.unwrap();
            assert_eq!(text, "It's 120.");
            assert_eq!(*calls.borrow(), vec![json!({"precise":false})]);
            let chats = fake.requests("/v1/chat/completions");
            let second = &chats[1].body;
            let call = second["messages"].as_array().unwrap().iter().find(|m| m["role"] == "assistant").unwrap();
            let id = call["tool_calls"][0]["id"].as_str().unwrap();
            assert!(regex::Regex::new("^call_[0-9a-f]{16}$").unwrap().is_match(id));
            assert_eq!(call["reasoning_content"], "Tempo first.");
            assert_eq!(second["messages"].as_array().unwrap().iter().find(|m| m["role"] == "tool").unwrap()["tool_call_id"], id);
            assert_eq!(chosen.binding.budget.as_ref().unwrap()(1000).limit, (65536. - 8192.) * 3. - 1000.);
        })
        .await;
}
#[tokio::test]
async fn named_servers_explain_flags_and_learn_context_limits() {
    LocalSet::new().run_until(async {
    let down=failed(binding(custom(),"model",Some(Rc::new(Down)),None),false).await;
assert_eq!(down.message,"llama.cpp isn't answering at http://127.0.0.1:8080/v1: start it, then send your message again.");
let answer=Rc::new(RefCell::new(json!({"error":{"code":500,"message":"tools param requires --jinja flag","type":"server_error"}})));
let given=answer.clone();
let fake=Fake::new(move|r|if r.path=="/v1/models"{reply(json!({"data":[{"id":"local-model"}]}))}else{Response::json_response(400,given.borrow().clone())});
    assert_eq!(failed(binding(custom(),"local-model",Some(fake.clone()),None),false).await.message,"llama.cpp needs --jinja to use tools: start it with --jinja, or choose another model with /model.");
    *answer.borrow_mut()=json!({"error":{"code":400,"message":"the request exceeds the available context size, try increasing it","type":"exceed_context_size_error","n_prompt_tokens":9000,"n_ctx":8192}});
let small=failed(binding(custom(),"local-model",Some(fake.clone()),None),true).await;
assert_eq!(small.kind,FailureKind::Model);
assert_eq!(small.message,"llama.cpp gives local-model room for 8,192 tokens, too few for Kumi's instructions and tools (about 24,000): start it with a context of 57,344 tokens or more, or choose another model with /model.");
    *answer.borrow_mut()=json!({"error":{"message":"the request exceeds the available context size, try increasing it","n_ctx":16384}});
let chosen=binding(custom(),"local-model",Some(fake),None);
let short=failed(chosen.clone(),false).await;
assert_eq!(short.kind,FailureKind::Request);
assert!(short.message.contains("Kumi keeps it shorter from now on"));
assert!(chosen.binding.budget.as_ref().unwrap()(1000).limit<30.*1024.);
}).await;
}
#[tokio::test]
async fn authentication_guidance_and_headers() {
    let fake = Fake::new(|_| Response::json_response(401, json!({"error":{"message":"Unauthorized"}})));
    let mut server = custom();
    server.name = "Studio PC".into();
    assert!(probe_local(&server, fake.transport()).await);
    let error = list_local_models(&server, fake.transport()).await.unwrap_err();
    assert_eq!(error.kumi().unwrap().kind, FailureKind::Auth);
    assert_eq!(error.to_string(), "Studio PC didn't accept a request without a key (HTTP 401): set its apiKey in ~/.kumi/settings.json.");
    assert_eq!(
        list_local_models(&lm_at(), fake.transport()).await.unwrap_err().to_string(),
        "LM Studio didn't accept a request without a key (HTTP 401): set LM_API_TOKEN to a token from LM Studio's server settings."
    );
    server.api_key = Some("secret".into());
    assert!(probe_local(&server, fake.transport()).await);
    assert_eq!(fake.seen.borrow().last().unwrap().headers["authorization"], "Bearer secret");
    assert_eq!(
        list_local_models(&server, fake.transport()).await.unwrap_err().to_string(),
        "Studio PC didn't accept the key Kumi has for it (HTTP 401): set its apiKey in ~/.kumi/settings.json."
    );
}
#[tokio::test]
async fn lm_studio_older_api_and_compatible_fallback() {
    LocalSet::new().run_until(async {
    let older=Fake::new(|r|match r.path.as_str(){"/api/v1/models"=>Response::json_response(404,json!({})),"/api/v0/models"=>reply(json!({"data":[{"id":"old-qwen","type":"llm","capabilities":["tool_use"],"max_context_length":131072,"loaded_context_length":65536,"state":"loaded","quantization":"Q4"},{"id":"embed","type":"embedding"}]})),_=>sse(vec![delta(json!({"content":"Ok."}),Some("stop"))])});
    let models=list_local_models(&lm_at(),older.transport()).await.unwrap();
assert_eq!(models.len(),1);
assert_eq!(models[0].context,Some(65536.));
assert_eq!(models[0].description.as_deref(),Some("Q4 · loaded"));
let chosen=binding(lm_at(),"old-qwen",Some(older.clone()),None);
run(chosen.clone(),"fixture instructions",vec![]).await.unwrap();
assert!(older.requests("/api/v1/models/load").is_empty());
assert_eq!(chosen.binding.budget.as_ref().unwrap()(1000).limit,(65536.-8192.)*3.-1000.);
    let fallback=Fake::new(|r|match r.path.as_str(){"/api/v1/models"|"/api/v0/models"=>Response::json_response(404,json!({})),"/v1/models"=>reply(json!({"data":[{"id":"qwen","max_model_len":32768},{"id":"embedding-model"}]})),_=>sse(vec![delta(json!({"content":"Ok."}),Some("stop"))])});
let models=list_local_models(&lm_at(),fallback.transport()).await.unwrap();
assert_eq!(models.len(),1);
assert_eq!(models[0].context,Some(32768.));
run(binding(lm_at(),"qwen",Some(fallback.clone()),None),"fixture instructions",vec![]).await.unwrap();
assert!(fallback.requests("/api/v1/models/load").is_empty());
    let without_load=Fake::new(|r|match r.path.as_str(){"/api/v1/models"=>reply(json!({"models":[lm_model("qwen",json!({}))]})),"/api/v1/models/load"=>Response::json_response(405,json!({})),_=>sse(vec![delta(json!({"content":"Ok."}),Some("stop"))])});
let(text,_)=run(binding(lm_at(),"qwen",Some(without_load.clone()),None),"fixture instructions",vec![]).await.unwrap();
assert_eq!(text,"Ok.");
assert_eq!(without_load.requests("/v1/chat/completions")[0].body["model"],"qwen");
}).await;
}
#[tokio::test]
async fn missing_embedding_and_retry_after_initial_probe() {
    LocalSet::new()
        .run_until(async {
            let fake = ollama(json!({"embed":{"capabilities":["embedding"]}}), |_, _| panic!("must not chat with embedding model"));
            let error = failed(binding(ollama_at("127.0.0.1"), "embed", Some(fake), None), false).await;
            assert_eq!(error.message, "embed on Ollama doesn't chat (it embeds, or makes pictures): choose another model with /model.");
            let fake = lm_studio(vec![], |_, _| panic!("missing model"));
            assert_eq!(
                failed(binding(lm_at(), "absent", Some(fake), None), false).await.message,
                "LM Studio doesn't have absent: download it in LM Studio, or choose one you have with /model."
            );
            struct Wakes {
                ready: Cell<bool>,
                fake: Rc<Fake>,
            }
            #[async_trait(?Send)]
            impl Fetch for Wakes {
                async fn fetch(&self, url: &str, init: FetchInit) -> Result<Response, LanguageModelError> {
                    if !self.ready.replace(true) {
                        return Err(LanguageModelError::other("fetch failed"));
                    }
                    self.fake.fetch(url, init).await
                }
            }
            let fake = ollama(json!({"qwen":{"capabilities":["completion","tools"]}}), |_, _| ndjson(vec![said("Awake."), done()]));
            let chosen = binding(ollama_at("127.0.0.1"), "qwen", Some(Rc::new(Wakes { ready: Cell::new(false), fake })), None);
            chosen.asked.clone().await;
            let (text, _) = run(chosen, "fixture instructions", vec![]).await.unwrap();
            assert_eq!(text, "Awake.");
        })
        .await;
}
