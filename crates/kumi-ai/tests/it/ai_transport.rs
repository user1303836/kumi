//! Native HTTP/SSE behavior corresponding to `@ai-sdk/provider-utils`.

use async_trait::async_trait;
use futures::StreamExt;
use kumi_common::abort::Signal;
use kumi_runtime::ai::{
    anthropic::{anthropic, AnthropicSettings},
    error::LanguageModelError,
    http::{post_json, Fetch, FetchInit, Headers, HttpFetch, Response},
    openai_responses::{openai_responses, ResponsesSettings},
    sse::{json_stream, safe_json, Event, SseParser},
    types::{CallOptions, StreamPart},
};
use serde_json::json;
use std::rc::Rc;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};

#[test]
fn sse_preserves_utf8_crlf_multiline_data_comments_and_optional_fields_at_every_chunk_boundary() {
    let input = "\u{feff}: comment\r\nevent: response\r\nid: 9\r\ndata: {\"text\":\r\ndata: \"🌱\"}\r\n\r\ndata: second\n\n".as_bytes();
    let expected = vec![
        Event { data: "{\"text\":\n\"🌱\"}".into(), event: Some("response".into()), id: Some("9".into()) },
        Event { data: "second".into(), event: None, id: None },
    ];
    for size in 1..=input.len() {
        let mut parser = SseParser::default();
        let actual: Vec<_> = input.chunks(size).flat_map(|bytes| parser.push(bytes)).collect();
        assert_eq!(actual, expected, "chunk size {size}");
    }
}
#[test]
fn sse_does_not_flush_an_unterminated_event_and_blank_events_reset_the_id() {
    let mut parser = SseParser::default();
    assert!(parser.push(b"id: discarded\n\n").is_empty());
    assert_eq!(parser.push(b"data: yes\n\n"), [Event { data: "yes".into(), event: None, id: None }]);
    assert!(parser.push(b"data: unfinished\n").is_empty());
    let mut parser = SseParser::default();
    assert!(parser.push(b"data: unfinished\r\r").is_empty());
    assert_eq!(parser.push(b"\n").len(), 1);
}
#[tokio::test]
async fn sse_json_discards_done_and_unterminated_events_without_losing_finished_ones() {
    let source = futures::stream::iter([Ok(b"data: {\"a\":1}\n\ndata: [DONE]\n\ndata: {\"a\":2}".to_vec())]);
    let values: Vec<_> = json_stream(Box::pin(source)).collect().await;
    assert_eq!(values, [Ok(json!({"a":1}))]);
}
#[test]
fn json_parsing_rejects_prototype_keys_at_any_depth() {
    for raw in [r#"{"__proto__":{}}"#, r#"[{"constructor":{"prototype":{}}}]"#] {
        assert!(safe_json(raw).unwrap_err().to_string().contains("forbidden prototype property"));
    }
    assert_eq!(safe_json(r#"{"constructor":"label"}"#).unwrap(), json!({"constructor":"label"}));
}
struct StatusFetch(u16);
#[async_trait(?Send)]
impl Fetch for StatusFetch {
    async fn fetch(&self, _: &str, init: FetchInit) -> Result<Response, LanguageModelError> {
        assert_eq!(init.method, "POST");
        assert_eq!(init.body.as_deref(), Some("{\"value\":1}"));
        assert_eq!(init.headers["content-type"], "application/json");
        Ok(Response::json_response(self.0, json!({"error":{"message":"rate limited"}})))
    }
}
#[tokio::test]
async fn json_posts_preserve_request_and_response_details_and_sdk_retry_policy() {
    for (status, retryable) in [(401, false), (429, true), (500, true)] {
        let error = post_json(&StatusFetch(status), "http://fixture/api", Headers::new(), json!({"value":1.0}), None).await.err().unwrap();
        let api = error.api_call().unwrap();
        assert_eq!(api.message, "rate limited");
        assert_eq!(api.status_code, Some(status));
        assert_eq!(api.is_retryable, retryable);
        assert_eq!(api.request_body_values, Some(json!({"value":1.0})));
        assert!(api.response_body.as_ref().unwrap().contains("rate limited"));
    }
}
#[tokio::test]
async fn native_http_streams_a_response_and_cancels_a_body_that_is_still_arriving() {
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut bytes = [0; 2048];
        socket.read(&mut bytes).await.unwrap();
        socket.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 1000\r\nX-Test: yes\r\n\r\nfirst").await.unwrap();
        std::future::pending::<()>().await;
    });
    let signal = Signal::new();
    let fetch: Rc<dyn Fetch> = Rc::new(HttpFetch::default());
    let mut response =
        fetch.fetch(&format!("http://{address}/stream"), FetchInit { signal: Some(signal.clone()), ..Default::default() }).await.unwrap();
    assert_eq!(response.headers["x-test"], "yes");
    let mut body = response.body.take().unwrap();
    assert_eq!(body.next().await.unwrap().unwrap(), b"first");
    signal.cancel();
    assert!(body.next().await.unwrap().unwrap_err().to_string().contains("aborted"));
    assert!(body.next().await.is_none());
    server.abort();
}

struct InterruptedFetch;
#[async_trait(?Send)]
impl Fetch for InterruptedFetch {
    async fn fetch(&self, _: &str, _: FetchInit) -> Result<Response, LanguageModelError> {
        let mut response = Response::text_response(200, "");
        response.headers.insert("x-fixture".into(), "yes".into());
        response.body = Some(Box::pin(futures::stream::iter([Err(LanguageModelError::other("terminated"))])));
        Ok(response)
    }
}
#[tokio::test]
async fn successful_response_read_errors_keep_the_sdk_request_status_and_headers() {
    let response = post_json(&InterruptedFetch, "http://fixture/api", Headers::new(), json!({"request":true}), None).await.unwrap();
    let error = response.text().await.unwrap_err();
    let error = error.api_call().unwrap();
    assert_eq!(error.message, "Failed to process successful response");
    assert_eq!(error.status_code, Some(200));
    assert_eq!(error.cause.as_deref(), Some("terminated"));
    assert_eq!(error.request_body_values, Some(json!({"request":true})));
    assert_eq!(error.response_headers.as_ref().unwrap()["x-fixture"], "yes");
}
#[tokio::test]
async fn an_anthropic_answer_whose_body_breaks_before_its_first_event_keeps_its_retryable_failure() {
    let model = anthropic(AnthropicSettings {
        model: "claude-haiku-4-5".into(),
        base_url: "http://fixture/v1".into(),
        api_key: Some("fixture-key".into()),
        auth_token: None,
        headers: Default::default(),
        fetch: Rc::new(InterruptedFetch),
    });
    let Err(error) = model.do_stream(CallOptions::default()).await else { panic!("the stream broke before its first event") };
    // The kernel retries it, as it does any body that breaks off after the headers.
    let error = error.api_call().expect("the failed call as it came, not a bare message");
    assert_eq!((error.status_code, error.is_retryable, error.cause.as_deref()), (Some(200), true, Some("terminated")));
}
struct Events(String);
#[async_trait(?Send)]
impl Fetch for Events {
    async fn fetch(&self, _: &str, _: FetchInit) -> Result<Response, LanguageModelError> {
        Ok(Response::text_response(200, self.0.clone()))
    }
}
#[tokio::test]
async fn a_responses_stream_that_closes_before_it_says_it_is_done_broke_off() {
    let events = |last: Option<serde_json::Value>| {
        let mut events = vec![
            json!({"type":"response.created","response":{"id":"r1","created_at":10,"model":"gpt-6-sol"}}),
            json!({"type":"response.output_item.added","output_index":0,"item":{"type":"message","id":"m1"}}),
            json!({"type":"response.output_text.delta","item_id":"m1","delta":"Half an ans"}),
        ];
        events.extend(last);
        events.iter().map(|event| format!("data: {event}\n\n")).collect::<String>() + "data: [DONE]\n\n"
    };
    let parts = |events: String| async move {
        let model = openai_responses(ResponsesSettings {
            model: "gpt-6-sol".into(),
            base_url: "http://fixture/v1".into(),
            api_key: "fixture-key".into(),
            headers: Default::default(),
            fetch: Rc::new(Events(events)),
        });
        model.do_stream(CallOptions::default()).await.unwrap().collect::<Vec<_>>().await
    };
    // No response.completed: the text is cut short, so the answer broke off and can be tried again.
    let cut = parts(events(None)).await;
    let errors: Vec<_> =
        cut.iter().filter_map(|part| if let StreamPart::Error { error } = part { error.api_call() } else { None }).collect();
    assert_eq!(errors.iter().map(|e| (e.status_code, e.is_retryable)).collect::<Vec<_>>(), [(Some(200), true)], "{cut:?}");
    assert!(matches!(cut.last(), Some(StreamPart::Finish { .. })));
    // Done, it finishes as before.
    let done = parts(events(Some(json!({"type":"response.completed","response":{"usage":{"input_tokens":2,"output_tokens":3}}})))).await;
    assert!(!done.iter().any(|part| matches!(part, StreamPart::Error { .. })), "{done:?}");
}
