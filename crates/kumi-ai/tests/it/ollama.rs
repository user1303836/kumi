//! Ollama requests and streams.

use async_trait::async_trait;
use futures::{future::FutureExt, stream, StreamExt};
use kumi_runtime::{
    ai::{
        error::LanguageModelError,
        http::{Fetch, FetchInit, Response},
        types::{CallOptions, FinishReasonUnified, FunctionTool, Message, StreamPart},
    },
    core::errors::{FailureKind, KumiError},
    providers::ollama::{ollama_chat, FailurePhase, OllamaChatSettings, OllamaShape},
};
use serde_json::{json, Value};
use std::{cell::RefCell, rc::Rc};

struct FakeFetch {
    body: String,
    status: u16,
    requests: Rc<RefCell<Vec<(String, FetchInit)>>>,
}
#[async_trait(?Send)]
impl Fetch for FakeFetch {
    async fn fetch(&self, url: &str, init: FetchInit) -> Result<Response, LanguageModelError> {
        self.requests.borrow_mut().push((url.into(), init));
        let mut response = Response::text_response(self.status, "");
        response.body = Some(Box::pin(stream::iter(self.body.as_bytes().chunks(3).map(|bytes| Ok(bytes.to_vec())).collect::<Vec<_>>())));
        Ok(response)
    }
}
fn settings(
    body: &str,
    status: u16,
    requests: Rc<RefCell<Vec<(String, FetchInit)>>>,
    images: bool,
    phases: Rc<RefCell<Vec<FailurePhase>>>,
) -> OllamaChatSettings {
    OllamaChatSettings {
        base_url: "http://fixture:11434".into(),
        model: "qwen3:8b".into(),
        fetch: Rc::new(FakeFetch { body: body.into(), status, requests }),
        shape: Rc::new(move |options| {
            async move { Ok(OllamaShape { num_ctx: 65_536.0, think: Some(json!("high")), options, images }) }.boxed_local()
        }),
        failure: Rc::new(move |error, phase| {
            phases.borrow_mut().push(phase);
            KumiError::new(FailureKind::Provider, format!("{}: {error}", if phase == FailurePhase::Request { "request" } else { "answer" }))
        }),
    }
}
#[tokio::test]
async fn native_chat_preserves_ollama_wire_messages_tools_images_context_and_thinking() {
    let requests = Rc::new(RefCell::new(vec![]));
    let model = ollama_chat(settings(
        "{\"message\":{\"content\":\"Done.\"},\"done\":true,\"done_reason\":\"stop\"}\n",
        200,
        requests.clone(),
        true,
        Rc::default(),
    ));
    let prompt: Vec<Message> = serde_json::from_value(json!([
        {"role":"system","content":"Produce."},
        {"role":"user","content":[{"type":"text","text":"Show it"},{"type":"file","mediaType":"image/png","data":{"type":"data","data":{"0":1,"1":2,"2":3}}}]},
        {"role":"assistant","content":[{"type":"reasoning","text":"Read first"},{"type":"tool-call","toolCallId":"call-1","toolName":"tempo","input":{"precise":true}}]},
        {"role":"tool","content":[{"type":"tool-result","toolCallId":"call-1","toolName":"tempo","output":{"type":"content","value":[{"type":"text","text":"120"},{"type":"file","mediaType":"image/png","data":{"type":"data","data":"AQID"}}]}}]}
    ])).unwrap();
    let parts: Vec<_> = model
        .do_stream(CallOptions {
            prompt,
            tools: Some(vec![FunctionTool::new("tempo", "Read tempo", json!({"type":"object"}))]),
            ..Default::default()
        })
        .await
        .unwrap()
        .collect()
        .await;
    assert!(matches!(parts.last(), Some(StreamPart::Finish { finish_reason, .. }) if finish_reason.unified == FinishReasonUnified::Stop));
    let requests = requests.borrow();
    let (url, init) = &requests[0];
    assert_eq!(url, "http://fixture:11434/api/chat");
    assert_eq!(init.method, "POST");
    assert_eq!(
        serde_json::from_str::<Value>(init.body.as_ref().unwrap()).unwrap(),
        json!({
            "model":"qwen3:8b","messages":[{"role":"system","content":"Produce."},{"role":"user","content":"Show it","images":["AQID"]},
            {"role":"assistant","content":"","thinking":"Read first","tool_calls":[{"id":"call-1","function":{"name":"tempo","arguments":{"precise":true}}}]},
            {"role":"tool","content":"120\n[An image this model can't be shown.]","tool_name":"tempo","tool_call_id":"call-1"}],
            "stream":true,"tools":[{"type":"function","function":{"name":"tempo","description":"Read tempo","parameters":{"type":"object"}}}],"think":"high","options":{"num_ctx":65536},"truncate":false,"shift":false
        })
    );
}
#[tokio::test]
async fn native_answer_separates_thinking_streams_complete_tool_inputs_and_sums_cached_usage() {
    let chunks = [
        json!({"message":{"content":"<thi"},"done":false}),
        json!({"message":{"content":"nk>想</think>\n答🌱"},"done":false}),
        json!({"message":{"tool_calls":[{"id":"tool-1","function":{"name":"tempo","arguments":{"precise":true}}},{"function":{"name":"play","arguments":"{}"}}]},"done":false}),
        json!({"done":true,"done_reason":"stop","prompt_eval_count":10,"prompt_eval_cached_count":5,"eval_count":7}),
    ].iter().map(|chunk| format!("{chunk}\n")).collect::<String>();
    let model = ollama_chat(settings(&chunks, 200, Rc::default(), false, Rc::default()));
    let parts: Vec<_> = model.do_stream(CallOptions::default()).await.unwrap().collect().await;
    let reasoning: String = parts
        .iter()
        .filter_map(|part| match part {
            StreamPart::ReasoningDelta { delta, .. } => Some(delta.as_str()),
            _ => None,
        })
        .collect();
    let text: String = parts
        .iter()
        .filter_map(|part| match part {
            StreamPart::TextDelta { delta, .. } => Some(delta.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(reasoning, "想");
    assert_eq!(text, "答🌱");
    let calls: Vec<_> = parts
        .iter()
        .filter_map(|part| match part {
            StreamPart::ToolCall(call) => Some(call),
            _ => None,
        })
        .collect();
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0].tool_call_id, "tool-1");
    assert_eq!(calls[0].input, "{\"precise\":true}");
    assert!(calls[1].tool_call_id.starts_with("call_"));
    assert!(parts
        .iter()
        .any(|part| matches!(part, StreamPart::ToolInputDelta { id,delta,.. } if id == "tool-1" && delta == "{\"precise\":true}")));
    match parts.last().unwrap() {
        StreamPart::Finish { finish_reason, usage, .. } => {
            assert_eq!(finish_reason.unified, FinishReasonUnified::ToolCalls);
            assert_eq!(usage.input_tokens.total, Some(15.0));
            assert_eq!(usage.input_tokens.no_cache, Some(10.0));
            assert_eq!(usage.input_tokens.cache_read, Some(5.0));
            assert_eq!(usage.output_tokens.total, Some(7.0));
        }
        _ => panic!("missing finish"),
    }
}
#[tokio::test]
async fn native_answer_reports_request_and_interrupted_stream_failures_in_their_actual_phases() {
    for (body, status, phase) in [
        ("out of memory", 500, FailurePhase::Request),
        ("{\"message\":{\"content\":\"partial\"},\"done\":false}\n", 200, FailurePhase::Answer),
        ("{\"error\":\"out of memory\"}\n", 200, FailurePhase::Answer),
    ] {
        let phases = Rc::new(RefCell::new(vec![]));
        let model = ollama_chat(settings(body, status, Rc::default(), false, phases.clone()));
        match model.do_stream(CallOptions::default()).await {
            Err(error) => assert!(error.to_string().contains("request")),
            Ok(parts) => {
                let parts: Vec<_> = parts.collect().await;
                assert!(matches!(parts.last(),Some(StreamPart::Error{error}) if error.to_string().contains("answer")));
                assert!(!parts.iter().any(|part| matches!(part, StreamPart::Finish { .. })));
            }
        }
        assert_eq!(*phases.borrow(), [phase]);
    }
}
#[tokio::test]
async fn native_chat_uses_words_for_images_when_the_model_has_no_vision_and_accepts_a_last_line_without_newline() {
    let requests = Rc::new(RefCell::new(vec![]));
    let model = ollama_chat(settings("{\"message\":{\"content\":\"Hello\"},\"done\":true}", 200, requests.clone(), false, Rc::default()));
    let prompt=serde_json::from_value(json!([{"role":"user","content":[{"type":"text","text":"See"},{"type":"file","mediaType":"image/png","data":{"type":"data","data":"AQID"}}]}])).unwrap();
    let parts: Vec<_> = model.do_stream(CallOptions { prompt, ..Default::default() }).await.unwrap().collect().await;
    assert!(matches!(parts.last(), Some(StreamPart::Finish { .. })));
    let body: Value = serde_json::from_str(requests.borrow()[0].1.body.as_ref().unwrap()).unwrap();
    assert_eq!(body["messages"][0]["content"], "See\n[An image this model can't be shown.]");
}
