//! The listening model through Gemini: picked from Gemini's own model list by what each model does, asked with the
//! audio in a generateContent request, its JSON answer read back; and kept off questions about width or air.
use kumi_common::abort::Signal;
use kumi_runtime::listening::listener::{gemini_listener, Listener};
use std::{cell::RefCell, rc::Rc};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// A server answering each request (its method, path, headers and body as one text) with what `answer` gives, and
/// keeping every request it got.
async fn serve(answer: impl Fn(&str) -> String + 'static) -> (String, Rc<RefCell<Vec<String>>>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let seen = Rc::new(RefCell::new(vec![]));
    let kept = seen.clone();
    tokio::task::spawn_local(async move {
        loop {
            let Ok((mut socket, _)) = listener.accept().await else { return };
            let mut input = vec![];
            let mut chunk = [0; 65536];
            // The head, then as much body as it says.
            loop {
                let n = socket.read(&mut chunk).await.unwrap_or(0);
                if n == 0 {
                    break;
                }
                input.extend_from_slice(&chunk[..n]);
                let Some(end) = input.windows(4).position(|w| w == b"\r\n\r\n") else { continue };
                let head = String::from_utf8_lossy(&input[..end]).to_lowercase();
                let length = head
                    .lines()
                    .find_map(|line| line.strip_prefix("content-length:").map(|value| value.trim().parse::<usize>().unwrap_or(0)))
                    .unwrap_or(0);
                if input.len() >= end + 4 + length {
                    break;
                }
            }
            let request = String::from_utf8_lossy(&input).to_string();
            let body = answer(&request);
            kept.borrow_mut().push(request);
            let reply = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = socket.write_all(reply.as_bytes()).await;
        }
    });
    (format!("http://{address}"), seen)
}

#[tokio::test]
async fn gemini_listens_with_the_newest_model_that_takes_what_its_given_and_answers_in_json() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let models = serde_json::json!({"models": [
                {"name": "models/text-embedding-004", "supportedGenerationMethods": ["embedContent"]},
                {"name": "models/gemini-2.5-flash-preview-tts", "supportedGenerationMethods": ["generateContent"]},
                {"name": "models/gemini-2.0-flash", "supportedGenerationMethods": ["generateContent", "countTokens"]},
                {"name": "models/gemini-2.5-pro", "supportedGenerationMethods": ["generateContent", "countTokens"]},
                {"name": "models/gemini-2.5-flash", "supportedGenerationMethods": ["generateContent", "countTokens"]},
                {"name": "models/gemini-2.5-flash-lite", "supportedGenerationMethods": ["generateContent", "countTokens"]},
                {"name": "models/gemini-2.5-flash-image", "supportedGenerationMethods": ["generateContent"]}
            ]});
            let answer = serde_json::json!({"candidates": [{"content": {"parts": [
                {"text": "{\"closer\": \"second\", \"first\": [\"muddy\"], \"second\": []}"}
            ]}}]});
            let (base, seen) =
                serve(move |request| if request.starts_with("GET /v1beta/models") { models.to_string() } else { answer.to_string() }).await;
            let listener = gemini_listener(&base, "the-key", Signal::new()).await.unwrap().expect("a listener");
            // The newest released version, and of that version the light model that isn't the lite one.
            assert_eq!(listener.name(), "gemini-2.5-flash");
            assert!(!listener.hears_width(), "Gemini hears a mono downmix: no questions about width or air");
            let heard = listener.ask(b"RIFF....WAVE", "punch toward 10 dB", Signal::new()).await.unwrap();
            assert_eq!((heard.closer.as_str(), heard.first.as_slice()), ("second", ["muddy".to_string()].as_slice()));
            let requests = seen.borrow().clone();
            let asked = requests.iter().find(|request| request.starts_with("POST")).unwrap();
            assert!(asked.starts_with("POST /v1beta/models/gemini-2.5-flash:generateContent"), "{}", &asked[..80]);
            assert!(
                asked.to_lowercase().contains("x-goog-api-key: the-key") && asked.contains("audio/wav"),
                "the key in a header, the audio inline"
            );
            // A list with nothing that generates content from audio: no listener (a definite none, not a failure).
            let (base, _) =
                serve(|_| r#"{"models":[{"name":"models/text-embedding-004","supportedGenerationMethods":["embedContent"]}]}"#.into())
                    .await;
            assert!(gemini_listener(&base, "the-key", Signal::new()).await.unwrap().is_none());
        })
        .await;
}
