//! `read` against a fake SoundCraft that shows the session SoundCraft 0.17.0 showed (tests/vectors): rows named by refs,
//! each under a key that may hold one; pages whose cursors find their place again, or say the list changed; refs and
//! cursors from an earlier session refused as stale.
use super::fake::{ok, Answer, Fake};
use super::vectors::vectors;
use daw_bridge::contract::misplaced_refs;
use serde_json::{json, Value};
use soundcraft_kumi::control::Control;
use soundcraft_kumi::read::Reads;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// How long one inspection serves the reads after it, and a moment more.
const STALE_AFTER: Duration = Duration::from_millis(1100);

/// The session SoundCraft 0.17.0 showed (`session.inspect {"detail":"full"}`).
fn recorded() -> Value {
    let all = vectors("soundcraft-0.17.0.ndjson");
    let full = all.iter().find(|exchange| exchange["method"] == "session.inspect" && exchange["params"]["detail"] == "full").unwrap();
    full["reply"]["result"].clone()
}

/// A fake SoundCraft showing `session`, whose MIDI clips have `notes`.
struct App {
    _fake: Fake,
    session: Arc<Mutex<Value>>,
    reads: Reads,
}

impl App {
    async fn new(notes: Value) -> App {
        let session = Arc::new(Mutex::new(recorded()));
        let shown = session.clone();
        let fake = Fake::start(16, move |request| match (request["method"].as_str(), request["params"]["command"].as_str()) {
            (Some("session.inspect"), _) => Answer::Now(ok(shown.lock().unwrap().clone())),
            (Some("engine.execute"), Some("midi.notes")) => Answer::Now(ok(json!({"notes": notes}))),
            _ => Answer::Now(json!({"ok": false, "error": "unknown method `x`"})),
        })
        .await;
        let reads = Reads::new(Control::new(fake.port(), None));
        App { _fake: fake, session, reads }
    }

    async fn read(&mut self, params: Value) -> Result<Value, (&'static str, String)> {
        self.reads.read(&params, Instant::now() + Duration::from_secs(5)).await
    }
}

fn refs(answer: &Value) -> Vec<String> {
    answer["items"].as_array().unwrap().iter().map(|row| row["ref"].as_str().unwrap().to_string()).collect()
}

#[tokio::test]
async fn tracks_read_as_rows_named_by_refs_under_keys_that_may_hold_them() {
    let mut app = App::new(json!([])).await;
    let answer = app.read(json!({"kind": "track"})).await.unwrap();
    assert_eq!(misplaced_refs(&answer), Vec::<String>::new(), "{answer}");
    let rows = answer["items"].as_array().unwrap();
    let session = recorded();
    assert_eq!((rows.len(), &answer["total"], &answer["indexFrom"]), (session["tracks"].as_array().unwrap().len(), &json!(9), &json!(0)));
    assert_eq!(answer["epoch"], app.reads.epoch());
    let kick = &rows[0];
    assert_eq!(
        (&kick["ref"], &kick["name"], &kick["selected"], &kick["insertCount"]),
        (&json!("s:track:3"), &json!("Kick"), &json!(true), &json!(2))
    );
    let verb = rows.iter().find(|row| row["name"] == "Verb").unwrap();
    assert_eq!((&verb["kind"], &verb["inputRef"], &verb["output"]), (&json!("aux"), &json!("s:bus:1"), &json!("Main")));
    let keys = rows.iter().find(|row| row["name"] == "Keys").unwrap();
    assert_eq!((&keys["instrument"], &keys["sendCount"]), (&json!("subtractive_synth"), &json!(1)));
    for kind in ["clip", "insert", "send", "bus", "marker", "group", "source", "session"] {
        let answer = app.read(json!({"kind": kind})).await.unwrap();
        assert_eq!(misplaced_refs(&answer), Vec::<String>::new(), "{kind}: {answer}");
    }
}

#[tokio::test]
async fn a_tracks_clips_inserts_and_sends_read_under_it() {
    let mut app = App::new(json!([])).await;
    let clips = app.read(json!({"kind": "clip", "parent": "s:track:3"})).await.unwrap();
    assert!(!refs(&clips).is_empty());
    assert!(
        clips["items"].as_array().unwrap().iter().all(|clip| clip["trackRef"] == "s:track:3" && clip["sourceRef"].is_string()),
        "{clips}"
    );
    let inserts = app.read(json!({"kind": "insert", "parent": "s:track:3"})).await.unwrap();
    assert_eq!(refs(&inserts), ["s:insert:3:a", "s:insert:3:b"]);
    let plugins: Vec<(&Value, &Value)> =
        inserts["items"].as_array().unwrap().iter().map(|insert| (&insert["plugin"], &insert["paramCount"])).collect();
    assert_eq!(plugins, [(&json!("eq_7band"), &json!(23)), (&json!("compressor"), &json!(11))]);
    let sends = app.read(json!({"kind": "send", "parent": "s:track:23"})).await.unwrap();
    assert_eq!(
        (refs(&sends), &sends["items"][0]["targetRef"], &sends["items"][0]["levelDb"]),
        (vec!["s:send:23:0".to_string()], &json!("s:bus:1"), &json!(-12.0))
    );
    let refused = app.read(json!({"kind": "clip", "parent": "s:bus:1"})).await.unwrap_err();
    assert_eq!(refused, ("bad-params", "parent: a track (s:track:…), not s:bus:1".to_string()));
}

#[tokio::test]
async fn a_page_ends_at_its_limit_and_its_cursor_goes_on_from_the_row_it_named() {
    let mut app = App::new(json!([])).await;
    let (mut seen, mut cursor) = (vec![], None::<String>);
    for _ in 0..3 {
        let answer = app.read(json!({"kind": "track", "limit": 4, "cursor": cursor})).await.unwrap();
        seen.extend(refs(&answer));
        cursor = answer["nextCursor"].as_str().map(str::to_string);
    }
    assert_eq!(cursor, None, "the third page was the last");
    let all = refs(&app.read(json!({"kind": "track"})).await.unwrap());
    assert_eq!(seen, all, "every track once, in order");
    // A track added before the cursor's row: the next page starts at the row the cursor named, wherever it is now.
    let first = app.read(json!({"kind": "track", "limit": 4})).await.unwrap();
    let cursor = first["nextCursor"].as_str().unwrap().to_string();
    {
        let mut session = app.session.lock().unwrap();
        let tracks = session["tracks"].as_array_mut().unwrap();
        let mut added = tracks[0].clone();
        added["id"] = json!(90);
        added["name"] = json!("Added");
        tracks.insert(1, added);
    }
    tokio::time::sleep(STALE_AFTER).await;
    let next = app.read(json!({"kind": "track", "limit": 4, "cursor": cursor})).await.unwrap();
    assert_eq!(refs(&next)[0], all[4], "no row skipped or read twice");
}

#[tokio::test]
async fn a_cursor_whose_row_has_gone_says_the_list_changed() {
    let mut app = App::new(json!([])).await;
    let first = app.read(json!({"kind": "track", "limit": 4})).await.unwrap();
    let cursor = first["nextCursor"].as_str().unwrap().to_string();
    let named = refs(&app.read(json!({"kind": "track"})).await.unwrap())[4].clone();
    app.session.lock().unwrap()["tracks"].as_array_mut().unwrap().retain(|track| format!("s:track:{}", track["id"]) != named);
    tokio::time::sleep(STALE_AFTER).await;
    let changed = app.read(json!({"kind": "track", "limit": 4, "cursor": cursor})).await.unwrap_err();
    assert_eq!(changed, ("changed", "the list changed since that cursor: read it again from the start".to_string()));
    let garbled = app.read(json!({"kind": "track", "cursor": "c1.nope"})).await.unwrap_err();
    assert_eq!(garbled.0, "bad-params");
}

#[tokio::test]
async fn refs_and_cursors_from_an_earlier_session_are_stale() {
    let mut app = App::new(json!([])).await;
    let first = app.read(json!({"kind": "track", "limit": 4})).await.unwrap();
    let (epoch, cursor) = (first["epoch"].as_str().unwrap().to_string(), first["nextCursor"].as_str().unwrap().to_string());
    app.session.lock().unwrap()["name"] = json!("Another Song");
    tokio::time::sleep(STALE_AFTER).await;
    let stale = app.read(json!({"kind": "track", "epoch": epoch})).await.unwrap_err();
    assert_eq!(stale.0, "stale", "{stale:?}");
    assert_ne!(app.read(json!({"kind": "track"})).await.unwrap()["epoch"], epoch.as_str(), "a new epoch for the new session");
    assert_eq!(app.read(json!({"kind": "track", "cursor": cursor})).await.unwrap_err().0, "stale");
}

#[tokio::test]
async fn one_object_reads_by_its_ref() {
    let mut app = App::new(json!([])).await;
    let read_one = |answer: Value| answer["items"][0].clone();
    let track = read_one(app.read(json!({"ref": "s:track:23"})).await.unwrap());
    assert_eq!((&track["name"], &track["ref"]), (&json!("Keys"), &json!("s:track:23")));
    let insert = read_one(app.read(json!({"ref": "s:insert:3:a"})).await.unwrap());
    assert_eq!(insert["plugin"], "eq_7band");
    let clip = read_one(app.read(json!({"ref": "s:clip:25"})).await.unwrap());
    assert_eq!((&clip["trackRef"], &clip["notes"]), (&json!("s:track:23"), &json!(180)));
    let marker = read_one(app.read(json!({"ref": "s:marker:29"})).await.unwrap());
    assert_eq!((&marker["name"], &marker["number"]), (&json!("Intro"), &json!(1)));
    assert_eq!(app.read(json!({"ref": "s:track:999"})).await.unwrap_err().0, "stale", "not of this session");
    assert_eq!(app.read(json!({"ref": "s:insert:3:j"})).await.unwrap_err().0, "failed", "no insert in slot j");
    assert_eq!(app.read(json!({"ref": "s:param:3:a:hpf_freq"})).await.unwrap_err().0, "bad-params", "not read yet");
    assert_eq!(app.read(json!({"ref": "track 3"})).await.unwrap_err().0, "bad-params");
}

#[tokio::test]
async fn a_clips_notes_read_under_it_and_notes_alike_are_marked_as_a_tie() {
    let note = |start: i64, pitch: u8, channel: u8, length: i64| json!({"start": start, "pitch": pitch, "channel": channel, "length": length, "velocity": 100, "release_velocity": 64});
    let notes = json!([note(0, 60, 0, 480), note(480, 62, 0, 240), note(480, 62, 0, 480), note(960, 64, 0, 480)]);
    let mut app = App::new(notes).await;
    let read = app.read(json!({"kind": "note", "parent": "s:clip:25"})).await.unwrap();
    assert_eq!(refs(&read), ["s:note:25:0.60.0", "s:note:25:480.62.0", "s:note:25:480.62.0", "s:note:25:960.64.0"]);
    let ties: Vec<&Value> = read["items"].as_array().unwrap().iter().map(|row| &row["tie"]).collect();
    assert_eq!(ties, [&Value::Null, &json!(2), &json!(2), &Value::Null]);
    assert_eq!(app.read(json!({"ref": "s:note:25:960.64.0"})).await.unwrap()["items"][0]["length"], 480);
    assert_eq!(app.read(json!({"ref": "s:note:25:480.62.0"})).await.unwrap_err().0, "failed", "a tie isn't guessed");
    assert_eq!(app.read(json!({"kind": "note"})).await.unwrap_err().0, "bad-params", "notes need their clip");
}

#[tokio::test]
async fn a_read_without_a_kind_or_a_limit_it_can_take_is_refused() {
    let mut app = App::new(json!([])).await;
    for params in [
        json!({}),
        json!({"kind": "item"}),
        json!({"kind": "track", "limit": 0}),
        json!({"kind": "track", "limit": 201}),
        json!({"kind": 3}),
    ] {
        assert_eq!(app.read(params.clone()).await.unwrap_err().0, "bad-params", "{params}");
    }
}
