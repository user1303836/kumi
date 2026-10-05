//! Stream normalization for local models.

use futures::{stream, StreamExt};
use kumi_runtime::{
    ai::http::{ByteStream, Response},
    providers::compat::{mend_stream, think_splitter},
};
use serde_json::{json, Value};
fn delta(value: Value, finish: Option<&str>) -> Value {
    json!({"object":"chat.completion.chunk","choices":[{"index":0,"delta":value,"finish_reason":finish}]})
}
#[test]
fn thinking_written_into_the_words_is_split_out_only_when_the_answer_opens_with_it_wherever_the_tags_fall() {
    fn split(chunks: &[&str]) -> [String; 2] {
        let mut splitter = think_splitter();
        let mut output = [String::new(), String::new()];
        for chunk in chunks {
            let split = splitter.push(chunk);
            output[0] += &split.reasoning;
            output[1] += &split.text;
        }
        let split = splitter.flush();
        output[0] += &split.reasoning;
        output[1] += &split.text;
        output
    }
    assert_eq!(split(&["<think>Hmm.</think>\n\nAnswer."]), ["Hmm.", "Answer."]);
    assert_eq!(split(&["  <th", "ink>Hm", "m.</thi", "nk>", "\n", "Answer."]), ["Hmm.", "Answer."]);
    assert_eq!(split(&["Use <think> tags like this."]), ["", "Use <think> tags like this."]);
    assert_eq!(split(&["<b>Bold</b>"]), ["", "<b>Bold</b>"]);
    assert_eq!(split(&["<think>Still thinking"]), ["Still thinking", ""]);
}
async fn mend(events: &str) -> Vec<Value> {
    let body: ByteStream = Box::pin(stream::iter(events.as_bytes().chunks(3).map(|chunk| Ok(chunk.to_vec())).collect::<Vec<_>>()));
    let mut response = Response::text_response(200, "");
    response.body = Some(mend_stream(body));
    response
        .text()
        .await
        .unwrap()
        .lines()
        .filter(|line| line.starts_with("data: ") && *line != "data: [DONE]")
        .map(|line| serde_json::from_str(&line[6..]).unwrap())
        .collect()
}
#[tokio::test]
async fn a_mended_stream_gives_each_call_one_id_its_arguments_as_text_and_a_finish_without_touching_a_stream_that_needs_none() {
    let events = [
        delta(json!({"tool_calls":[{"index":0,"id":"","function":{"name":"get_tempo","arguments":""}}]}), None),
        delta(json!({"tool_calls":[{"index":0,"function":{"arguments":"{\"precise\":"}}]}), None),
        delta(json!({"tool_calls":[{"index":0,"function":{"arguments":"true}"}}]}), None),
        delta(json!({"tool_calls":[{"function":{"name":"play","arguments":{}}}]}), None),
    ]
    .iter()
    .map(|chunk| format!("data: {chunk}\n\n"))
    .collect::<String>()
        + "data: [DONE]\n\n";
    let chunks = mend(&events).await;
    let ids: Vec<_> = chunks[..3].iter().map(|chunk| chunk["choices"][0]["delta"]["tool_calls"][0]["id"].as_str().unwrap()).collect();
    assert!(ids.iter().all(|id| *id == ids[0]));
    assert!(ids[0].starts_with("call_"));
    let second = &chunks[3]["choices"][0]["delta"]["tool_calls"][0];
    assert_eq!(second["index"], json!(1));
    assert_ne!(second["id"], ids[0]);
    assert_eq!(second["function"]["arguments"], "{}");
    assert_eq!(chunks.last().unwrap()["choices"][0]["finish_reason"], "tool_calls");
    let plain = [delta(json!({"content":"Hi."}), None), delta(json!({}), Some("stop"))]
        .iter()
        .map(|chunk| format!("data: {chunk}\n\n"))
        .collect::<String>()
        + "data: [DONE]\n\n";
    assert_eq!(mend(&plain).await, [delta(json!({"content":"Hi."}), None), delta(json!({}), Some("stop"))]);
}
#[tokio::test]
async fn mended_streams_preserve_utf8_and_thinking_across_byte_boundaries() {
    let events = [delta(json!({"content":"<think>想</think>\n答🌱"}), None), delta(json!({}), Some("stop"))]
        .iter()
        .map(|chunk| format!("data: {chunk}\n\n"))
        .collect::<String>();
    let output = mend(&events).await;
    assert_eq!(output[0]["choices"][0]["delta"]["reasoning_content"], "想");
    assert_eq!(output[0]["choices"][0]["delta"]["content"], "答🌱");
    let body = Box::pin(stream::iter([Err(kumi_runtime::ai::error::LanguageModelError::other("disconnected"))]));
    let output: Vec<_> = mend_stream(body).collect().await;
    assert_eq!(output.len(), 1);
    assert!(output[0].is_err());
}
