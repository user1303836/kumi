//! SoundCraft 0.17.0's own answers (tests/vectors, recorded from the app by examples/record.rs), replayed by the fake
//! server: the client reads each one as the app meant it.
use super::fake::{Answer, Fake};
use serde_json::Value;
use soundcraft_kumi::control::{Control, Error};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

fn vectors(name: &str) -> Vec<Value> {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/vectors").join(name);
    let text = std::fs::read_to_string(&path).unwrap_or_else(|error| panic!("{}: {error}", path.display()));
    text.lines().map(|line| serde_json::from_str(line).unwrap()).collect()
}

#[tokio::test]
async fn soundcraft_0_17_0s_answers_are_read_as_the_app_meant_them() {
    let vectors = Arc::new(vectors("soundcraft-0.17.0.ndjson"));
    let (recorded, next) = (vectors.clone(), Arc::new(AtomicUsize::new(0)));
    let fake = Fake::start(16, move |request| {
        let vector = &recorded[next.fetch_add(1, Ordering::SeqCst)];
        assert_eq!((&request["method"], &request["params"]), (&vector["method"], &vector["params"]), "the scene, in order");
        Answer::Now(vector["reply"].clone())
    })
    .await;
    let control = Control::new(fake.port(), None);
    let mut refusals = vec![];
    for vector in vectors.iter() {
        let method = vector["method"].as_str().unwrap();
        let result = control.call(method, vector["params"].clone(), Duration::from_secs(5)).await;
        match (vector["reply"]["ok"].as_bool(), result) {
            (Some(true), Ok(result)) => assert_eq!(result, vector["reply"]["result"], "{method} {}", vector["params"]),
            (Some(false), Err(error)) => {
                assert!(!error.uncertain(), "the app ran nothing: {error:?}");
                refusals.push(error);
            }
            (said, read) => panic!("{method} {}: the app said ok = {said:?}, the client read {read:?}", vector["params"]),
        }
    }
    assert_eq!(refusals.len(), 4, "{refusals:?}");
    assert_eq!(refusals[0].unknown_command(), Some("mix.nope"));
    assert!(matches!(&refusals[1], Error::Refused(said) if said.starts_with("bad parameters for `mix.volume`")), "{:?}", refusals[1]);
    assert!(matches!(&refusals[2], Error::Refused(said) if said.starts_with("`edit.cut` is not available")), "{:?}", refusals[2]);
    assert_eq!(refusals[3], Error::Protocol("unknown method `mix.volume`".into()));
    assert_eq!(fake.connections(), 1, "the app's refusals keep the connection");
}
