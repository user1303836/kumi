//! What words mean to the style model: RoBERTa's tokenizer pinned against the reference tokenizer's ids for twenty
//! texts (its vocabulary and merges as the model came with them), a swapped style model left without words, and, with
//! the real model in KUMI_TEST_MODELS, a text's vector as the converted model gave it.

use kumi_listening::{
    embed::distance,
    music::hear::{Clap, Style, SWAPPED},
    words::Tokenizer,
};
use serde_json::Value;
use std::io::Read;

fn gunzip(bytes: &[u8]) -> String {
    let mut text = String::new();
    flate2::read::GzDecoder::new(bytes).read_to_string(&mut text).unwrap();
    text
}

fn tokenizer() -> Tokenizer {
    Tokenizer::new(
        &gunzip(include_bytes!("../support/clap-text/vocab.json.gz")),
        &gunzip(include_bytes!("../support/clap-text/merges.txt.gz")),
    )
    .unwrap()
}

#[test]
fn twenty_texts_tokenize_as_the_reference_tokenizer_does() {
    let tokenizer = tokenizer();
    let pins: Value = serde_json::from_str(include_str!("../support/clap-text/pins.json")).unwrap();
    for pin in pins.as_array().unwrap() {
        let want: Vec<i64> = pin["ids"].as_array().unwrap().iter().map(|id| id.as_i64().unwrap()).collect();
        assert_eq!(tokenizer.encode(pin["text"].as_str().unwrap()), want, "{}", pin["text"]);
    }
}

#[test]
fn a_long_text_keeps_its_first_and_last_tokens() {
    let ids = tokenizer().encode(&"dub ".repeat(600));
    assert_eq!((ids.len(), ids[0], *ids.last().unwrap()), (512, 0, 2));
}

#[tokio::test(flavor = "current_thread")]
async fn a_swapped_style_model_has_no_words() {
    let home = tempfile::tempdir().unwrap();
    std::fs::write(home.path().join("slots.json"), r#"{"embeddings":{"now":{"use":"file","path":"/somewhere/other-clap.onnx"}}}"#).unwrap();
    std::env::set_var("KUMI_HOME", home.path());
    let signal = kumi_common::abort::Signal::new();
    let clap = Clap::from_slot(&signal).await.expect("a style model");
    assert_eq!(clap.words("dub techno", &|_| {}, &signal).await, Err(SWAPPED.to_string()));
}

#[tokio::test(flavor = "current_thread")]
async fn the_real_words_model_reads_a_text_as_the_converted_model_did() {
    let Some(folder) = std::env::var("KUMI_TEST_MODELS").ok().filter(|folder| !folder.is_empty()) else {
        eprintln!("skipped: set KUMI_TEST_MODELS to a folder named models holding onnxruntime-1.23.2/, clap-music-text.onnx, clap-music-vocab.json and clap-music-merges.txt to run it");
        return;
    };
    assert!(std::path::Path::new(&folder).ends_with("models"), "KUMI_TEST_MODELS has to be a folder named models: {folder}");
    std::env::set_var("KUMI_HOME", std::path::Path::new(&folder).parent().unwrap());
    let fixture: Value = serde_json::from_str(include_str!("../support/clap-text/embedding.json")).unwrap();
    let want: Vec<f32> = fixture["embedding"].as_array().unwrap().iter().map(|value| value.as_f64().unwrap() as f32).collect();
    let signal = kumi_common::abort::Signal::new();
    let got = kumi_listening::words::words(fixture["text"].as_str().unwrap(), &|said| eprintln!("{said}"), &signal).await.unwrap();
    let apart = distance(&got, &want).unwrap();
    assert!(apart < 1e-3, "{apart}");
}
