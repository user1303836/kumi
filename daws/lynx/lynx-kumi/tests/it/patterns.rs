use super::support::Fake;
use lynx_kumi::{
    native,
    project::{epoch, reference, Bridge},
};
use serde_json::{json, Value};

async fn apply(bridge: &mut Bridge, kind: &str, mut input: Value, records: &mut Vec<(String, Value, Value, String)>) -> Value {
    let session = bridge.session().await.unwrap();
    input["expectedRevision"] = session["revision"].clone();
    let preview = bridge.preview(&json!({"kind":kind,"input":input,"epoch":epoch(&session)})).await.unwrap();
    let desc = format!("Kumi: pattern edit {} [k{}]", records.len(), records.len());
    let args = json!({"kind":kind,"input":input,"fence":preview["fence"],"desc":desc});
    let applied = bridge.apply(&args).await.unwrap();
    assert_eq!(bridge.apply(&args).await.unwrap(), applied, "a repeated apply must not add another native step");
    assert_eq!(applied["undo"]["entry"], true);
    records.push((kind.into(), preview, applied.clone(), desc));
    applied
}

#[tokio::test]
async fn new_pattern_notes_clone_and_metadata_are_owned_steps_with_revision_bound_reads() {
    let fake = Fake::start(false).await;
    let baseline = {
        let mut state = fake.state.lock().unwrap();
        state.session["playlist"] = json!([{"kind":"pattern","pattern":0,"clipId":"0000000000000001","start":0,"length":3840}]);
        state.session.clone()
    };
    let mut bridge = Bridge::new(native::Client::new(fake.path()));
    let session = bridge.session().await.unwrap();
    let input = json!({"index":0,"name":"Lead","lengthTicks":3840,"expectedRevision":1});
    let preview = bridge.preview(&json!({"kind":"pattern.create","input":input,"epoch":epoch(&session)})).await.unwrap();
    {
        let state = fake.state.lock().unwrap();
        assert_eq!(state.session, baseline, "preview must leave pattern layout untouched");
        assert!(!state.calls.iter().any(|(op, _)| op == "pattern.create"));
    }
    let mut records = vec![];
    let desc = "Kumi: create Lead [kcreate]";
    let args = json!({"kind":"pattern.create","input":input,"fence":preview["fence"],"desc":desc});
    let created = bridge.apply(&args).await.unwrap();
    assert_eq!(created, bridge.apply(&args).await.unwrap());
    assert_eq!(created["applied"]["index"], 0);
    assert_ne!(created["applied"]["patternId"], baseline["patterns"][0]["patternId"]);
    records.push(("pattern.create".into(), preview, created, desc.into()));
    let session = bridge.session().await.unwrap();
    assert_eq!(session["patterns"][1]["patternId"], baseline["patterns"][0]["patternId"]);
    assert_eq!(session["playlist"][0]["pattern"], 1);
    assert_eq!(bridge.read(&json!({"ref":reference("pattern",&[0],&baseline)})).await.unwrap_err().code, "changed");
    let read = bridge.read(&json!({"kind":"note","parent":reference("pattern",&[0],&session)})).await.unwrap();
    assert_eq!(read["items"], json!([]));
    let note = json!({"start":0,"length":960,"channel":1,"key":67,"velocity":0.75,"pan":-0.2,"muted":false,"finePitch":0.1,"slide":true,"porta":false,"release":0.4,"modX":0.2,"modY":0.8,"colour":4,"group":7});
    apply(&mut bridge, "pattern.replace_notes", json!({"index":0,"notes":[note.clone()]}), &mut records).await;
    let cloned = apply(&mut bridge, "pattern.clone", json!({"index":0,"name":"Lead double"}), &mut records).await;
    assert_eq!(cloned["applied"]["index"], 1);
    assert_ne!(cloned["applied"]["patternId"], cloned["target"]["patternId"]);
    let session = bridge.session().await.unwrap();
    assert_eq!(session["patterns"][1]["lengthTicks"], 3840);
    assert_eq!(session["patterns"][1]["colour"], session["patterns"][0]["colour"]);
    assert_eq!(session["playlist"][0]["pattern"], 2);
    let read = bridge.read(&json!({"kind":"note","parent":reference("pattern",&[1],&session)})).await.unwrap();
    let mut observed = read["items"][0].clone();
    observed.as_object_mut().unwrap().remove("ref");
    assert_eq!(observed, note, "cloning must preserve every note expression field");
    apply(&mut bridge, "pattern.set", json!({"index":1,"name":"Final Lead","lengthTicks":1920}), &mut records).await;
    let session = bridge.session().await.unwrap();
    assert_eq!(session["patterns"][1]["name"], "Final Lead");
    assert_eq!(session["patterns"][1]["lengthTicks"], 1920);
    assert_eq!(session["patterns"][1]["noteCount"], 1);
    for (kind, preview, applied, desc) in records.iter().rev() {
        bridge
            .restore(&json!({"kind":kind,"target":preview["target"],"prior":preview["prior"],"applied":applied["applied"],"of":desc}))
            .await
            .unwrap();
    }
    let state = fake.finish().await;
    assert_eq!(state.session["patterns"], baseline["patterns"]);
    assert_eq!(state.session["playlist"], baseline["playlist"]);
    assert_eq!(state.session["currentPattern"], baseline["currentPattern"]);
    for kind in ["pattern.create", "pattern.replace_notes", "pattern.clone", "pattern.set"] {
        assert_eq!(state.calls.iter().filter(|(op, _)| op == kind).count(), 1, "{kind}");
    }
    assert_eq!(state.calls.iter().filter(|(op, _)| op == "history.undo").count(), 4);
}

#[tokio::test]
async fn invalid_pattern_previews_and_stale_apply_never_send_a_pattern_write() {
    let fake = Fake::start(false).await;
    let mut bridge = Bridge::new(native::Client::new(fake.path()));
    let session = bridge.session().await.unwrap();
    for (kind, mut input) in [
        ("pattern.create", json!({"name":""})),
        ("pattern.create", json!({"name":" \t "})),
        ("pattern.clone", json!({"index":0,"name":"bad\nname"})),
        ("pattern.create", json!({"lengthTicks":-1})),
        ("pattern.create", json!({"lengthTicks":1.5})),
        ("pattern.create", json!({"lengthTicks":1_000_000_001})),
        ("pattern.create", json!({"index":2})),
        ("pattern.clone", json!({})),
        ("pattern.clone", json!({"index":1})),
        ("pattern.set", json!({"index":0})),
        ("pattern.set", json!({"index":1,"name":"Missing"})),
        ("pattern.create", json!({"notes":[]})),
    ] {
        input["expectedRevision"] = 1.into();
        assert!(bridge.preview(&json!({"kind":kind,"input":input,"epoch":epoch(&session)})).await.is_err(), "{kind}: {input}");
    }
    assert!(bridge.preview(&json!({"kind":"pattern.create","input":{},"epoch":epoch(&session)})).await.is_err());
    let input = json!({"name":"Stale","expectedRevision":1});
    let preview = bridge.preview(&json!({"kind":"pattern.create","input":input,"epoch":epoch(&session)})).await.unwrap();
    fake.state.lock().unwrap().session["revision"] = 2.into();
    assert_eq!(
        bridge
            .apply(&json!({"kind":"pattern.create","input":input,"fence":preview["fence"],"desc":"Kumi: stale create [kstale]"}))
            .await
            .unwrap_err()
            .code,
        "changed"
    );
    let state = fake.finish().await;
    assert!(!state.calls.iter().any(|(op, _)| matches!(op.as_str(), "pattern.create" | "pattern.clone" | "pattern.set" | "history.undo")));
}

#[tokio::test]
async fn unchanged_pattern_metadata_has_no_owned_undo_step() {
    let fake = Fake::start(false).await;
    let mut bridge = Bridge::new(native::Client::new(fake.path()));
    let session = bridge.session().await.unwrap();
    let input = json!({"index":0,"name":"Pattern 1","lengthTicks":0,"expectedRevision":1});
    let preview = bridge.preview(&json!({"kind":"pattern.set","input":input,"epoch":epoch(&session)})).await.unwrap();
    let applied = bridge
        .apply(&json!({"kind":"pattern.set","input":input,"fence":preview["fence"],"desc":"Kumi: unchanged pattern [knoop]"}))
        .await
        .unwrap();
    assert_eq!(applied["undo"]["entry"], false);
    assert_eq!(applied["layout"], 1);
    let state = fake.finish().await;
    assert_eq!(state.session["revision"], 1);
    assert_eq!(state.session["canUndo"], false);
}
