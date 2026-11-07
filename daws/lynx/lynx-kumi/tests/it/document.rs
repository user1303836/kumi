use super::support::Fake;
use lynx_kumi::{
    native,
    project::{epoch, reference, Bridge},
};
use serde_json::{json, Value};

#[tokio::test]
async fn complete_document_is_decoded_into_bounded_pointer_pages_then_edited_as_one_journal_step() {
    let fake = Fake::start(false).await;
    fake.state.lock().unwrap().session["channels"][0]["opaque"] = Value::String("界".repeat(20000));
    let mut bridge = Bridge::new(native::Client::new(fake.path()));
    let session = bridge.session().await.unwrap();
    let root = bridge.read(&json!({"kind":"document","limit":1})).await.unwrap();
    assert!(root["items"].as_array().unwrap().len() == 1 && root.get("nextCursor").is_some());
    assert_eq!(root["snapshotId"], "e".repeat(32));
    assert_eq!(root["documentHash"].as_str().unwrap().len(), 64);
    let page = bridge.read(&json!({"kind":"document","args":{"path":"/channels/0/opaque"}})).await.unwrap();
    assert_eq!(page["total"], 20000);
    let mut text = page["items"][0]["value"].as_str().unwrap().to_owned();
    let mut cursor = page.get("nextCursor").cloned();
    while let Some(next) = cursor {
        let page = bridge.read(&json!({"kind":"document","args":{"path":"/channels/0/opaque"},"cursor":next})).await.unwrap();
        assert!(page.to_string().len() < 48 * 1024);
        text.push_str(page["items"][0]["value"].as_str().unwrap());
        cursor = page.get("nextCursor").cloned();
    }
    assert_eq!(text, "界".repeat(20000));
    let input = json!({"snapshotId":root["snapshotId"],"expectedDocumentHash":root["documentHash"],"expectedRevision":1,"patch":[{"op":"replace","path":"/tempo","value":132},{"op":"replace","path":"/channels/0/name","value":"Document edit"}]});
    let preview = bridge.preview(&json!({"kind":"project.edit","input":input,"epoch":epoch(&session)})).await.unwrap();
    assert_eq!(fake.state.lock().unwrap().session["bpm"], 120);
    let args = json!({"kind":"project.edit","input":input,"fence":preview["fence"],"desc":"Kumi: Document transaction [doc1]"});
    let applied = bridge.apply(&args).await.unwrap();
    assert_eq!(bridge.apply(&args).await.unwrap(), applied);
    assert_eq!(applied["undo"]["covered"], true);
    assert_eq!(fake.state.lock().unwrap().session["bpm"], 132);
    bridge.restore(&json!({"kind":"project.edit","of":"Kumi: Document transaction [doc1]","prior":preview["prior"],"target":preview["target"],"applied":applied["applied"]})).await.unwrap();
    let state = fake.finish().await;
    assert_eq!(state.session["bpm"], 120);
    assert_eq!(state.session["channels"][0]["name"], "Synth");
    assert_eq!(state.calls.iter().filter(|(op, _)| op == "project.edit").count(), 1);
    assert!(state.calls.iter().filter(|(op, _)| op == "project.read").count() >= 2);
}

#[tokio::test]
async fn restart_bound_refs_and_opaque_vendor_state_protect_targeted_undo() {
    let fake = Fake::start(false).await;
    let mut bridge = Bridge::new(native::Client::new(fake.path()));
    let session = bridge.session().await.unwrap();
    let old_ref = reference("track", &[0], &session);
    fake.state.lock().unwrap().session["controlEpoch"] = "d".repeat(64).into();
    assert_eq!(bridge.read(&json!({"ref":old_ref})).await.unwrap_err().code, "stale");
    let session = bridge.session().await.unwrap();
    let input = json!({"bpm":126,"expectedRevision":1});
    let preview = bridge.preview(&json!({"kind":"tempo.set","input":input,"epoch":epoch(&session)})).await.unwrap();
    let applied = bridge
        .apply(&json!({"kind":"tempo.set","input":input,"fence":preview["fence"],"desc":"Kumi: Opaque state [opaque1]"}))
        .await
        .unwrap();
    fake.state.lock().unwrap().opaque_device_state = "A different wavetable with unchanged exposed parameters".into();
    assert_eq!(bridge.restore(&json!({"kind":"tempo.set","of":"Kumi: Opaque state [opaque1]","prior":preview["prior"],"target":preview["target"],"applied":applied["applied"]})).await.unwrap_err().code,"changed");
    let state = fake.finish().await;
    assert_eq!(state.calls.iter().filter(|(op, _)| op == "history.undo").count(), 0);
}

#[test]
fn native_proof_matches_an_independent_known_answer_transcript() {
    // Python standard-library hmac/sha256 over the documented lynx-control/1 transcript.
    assert_eq!(
        native::proof(&"a".repeat(64), "server", &"b".repeat(32), &"c".repeat(32)),
        "bc1e14ac2bfd4d18a7de372ecc86162bfa653e32b36319617efdba9ccc9250e5"
    );
}
