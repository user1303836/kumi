//! Opt-in: does a conversation keep its provider's prompt cache when its kernel is rebuilt, as on a
//! resume? Short turns with the configured model and sign-in, never Live. Prints one JSON line a turn
//! with its input and cached tokens: warm in one kernel, then rebuilt from the same history with the
//! same conversation, with another conversation, and with none (a key of its own, as before).
//!
//! Run: cargo run -p kumi --example probe_cache

use std::{cell::RefCell, rc::Rc};

use async_trait::async_trait;
use futures::StreamExt;
use kumi::config::load_inference_config;
use kumi_agent::{
    core::contracts::{KernelCheckpoint, KernelEmit},
    create_agent_kernel, AgentKernel, AgentKernelOptions, KernelEvent,
};
use kumi_ai::{
    auth::store::open_credential_store,
    providers::{resolve_model, ResolveModelOptions},
};
use kumi_ai::{
    error::LanguageModelError,
    http::{Fetch, FetchInit, HttpFetch, Response},
};
use kumi_common::abort::Signal;
use kumi_types::{errors::RuntimeError, system};
use serde_json::{json, Value};

/// The provider's own usage report and the cache key sent, on stderr: no credentials.
struct Showing(HttpFetch);

#[async_trait(?Send)]
impl Fetch for Showing {
    async fn fetch(&self, url: &str, init: FetchInit) -> Result<Response, LanguageModelError> {
        if let Some(body) = init.body.as_deref().and_then(|body| serde_json::from_str::<Value>(body).ok()) {
            eprintln!(
                "  sent: prompt_cache_key={} store={} session-id={}",
                body["prompt_cache_key"],
                body["store"],
                init.headers.get("session-id").map_or("-", String::as_str)
            );
        }
        let mut response = self.0.fetch(url, init).await?;
        if let Some(body) = response.body.take() {
            let mut pending = String::new();
            response.body = Some(Box::pin(body.map(move |chunk| {
                if let Ok(bytes) = &chunk {
                    pending.push_str(&String::from_utf8_lossy(bytes));
                    while let Some(at) = pending.find('\n') {
                        let line: String = pending.drain(..=at).collect();
                        let event = line.strip_prefix("data: ").and_then(|data| serde_json::from_str::<Value>(data.trim()).ok());
                        if let Some(event) = event.filter(|event| event["type"] == "response.completed") {
                            eprintln!("  usage: {}", event["response"]["usage"]);
                        }
                    }
                }
                chunk
            })));
        }
        Ok(response)
    }
}

/// Long enough to be cached (providers cache prompts from about 1,024 tokens), and the same every run.
fn instructions() -> String {
    let rules: Vec<String> =
        (1..=300).map(|n| format!("Rule {n}: when asked for word {n}, answer with exactly that word and nothing else.")).collect();
    format!("You are a terse test assistant. Reply with only what is asked.\n{}", rules.join("\n"))
}

async fn kernel(model: &str, checkpoint: Option<KernelCheckpoint>, conversation: Option<String>) -> Result<AgentKernel, RuntimeError> {
    let env = system::process_env();
    let config = load_inference_config(&env)?;
    let binding = resolve_model(ResolveModelOptions {
        model: model.into(),
        store: Rc::new(open_credential_store(&config.auth_file)),
        env: Some(env),
        fetch: Some(Rc::new(Showing(HttpFetch::default()))),
        effort: None,
        service_tier: None,
    })
    .await?;
    create_agent_kernel(AgentKernelOptions {
        instructions: instructions(),
        tools: vec![],
        signal: Signal::new(),
        checkpoint,
        binding,
        max_steps: None,
        budget: None,
        conversation,
    })
}

async fn turn(kernel: &AgentKernel, label: &str, words: &str) -> Result<(), RuntimeError> {
    let text = Rc::new(RefCell::new(String::new()));
    let heard = text.clone();
    let emit: KernelEmit = Rc::new(move |event| {
        if let KernelEvent::Text { text } = event {
            heard.borrow_mut().push_str(&text);
        }
        Ok(())
    });
    let result = kernel.run(&format!("Reply with only the word: {words}"), Signal::new(), emit).await?;
    let usage = result.usage.unwrap_or_default();
    println!("{}", json!({ "turn": label, "input": usage.input_tokens, "cached": usage.cache_read_tokens, "text": text.borrow().trim() }));
    Ok(())
}

async fn run() -> Result<(), RuntimeError> {
    let env = system::process_env();
    let config = load_inference_config(&env)?;
    let model = config.model.ok_or_else(|| RuntimeError::plain("No model is chosen"))?;
    println!("{}", json!({ "model": model }));
    let id = format!("probe-cache-{}", hex::encode(rand::random::<[u8; 8]>()));
    let first = kernel(&model, None, Some(id.clone())).await?;
    turn(&first, "cold", "one").await?;
    turn(&first, "warm, same kernel", "two").await?;
    let saved: KernelCheckpoint = first.checkpoint()?.into();
    first.close().await;
    for (label, conversation) in [
        ("rebuilt, another conversation", Some(format!("{id}-other"))),
        ("rebuilt, no conversation", None),
        ("rebuilt, same conversation", Some(id.clone())),
    ] {
        let next = kernel(&model, Some(saved.clone()), conversation).await?;
        turn(&next, label, "three").await?;
        next.close().await;
    }
    Ok(())
}

fn main() {
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().expect("runtime");
    if let Err(error) = tokio::task::LocalSet::new().block_on(&runtime, run()) {
        eprintln!("probe_cache failed: {}", error.message());
        std::process::exit(1);
    }
}
