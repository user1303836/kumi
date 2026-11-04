//! `read` against a fake SoundCraft that shows the session SoundCraft 0.17.0 showed (tests/vectors), with a group added:
//! rows named by refs, each under a key that may hold one; pages whose cursors find their row again, or say the list
//! changed; refs and cursors from another session or table refused as stale; and what a read can be refused for without
//! the app refused before it's asked.
use super::fake::{ok, Answer, Fake};
use super::vectors::vectors;
use daw_bridge::contract::{key_holds_a_ref, key_holds_refs};
use serde_json::{json, Value};
use soundcraft_kumi::control::Control;
use soundcraft_kumi::read::Reads;
use soundcraft_kumi::refs::Ref;
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

/// A fake SoundCraft showing `session` (the recorded one, with a group of Kick and Keys), whose MIDI clips have `notes`.
struct App {
    fake: Fake,
    session: Arc<Mutex<Value>>,
    notes: Arc<Mutex<Value>>,
    reads: Reads,
}

impl App {
    async fn new(notes: Value) -> App {
        let mut shown = recorded();
        shown["groups"] = json!([{"id": 33, "name": "Drums", "letter": "a", "members": [3, 23], "active": true}]);
        let (session, notes) = (Arc::new(Mutex::new(shown)), Arc::new(Mutex::new(notes)));
        let (shown, played) = (session.clone(), notes.clone());
        let fake = Fake::start(16, move |request| match (request["method"].as_str(), request["params"]["command"].as_str()) {
            (Some("session.inspect"), _) => Answer::Now(ok(shown.lock().unwrap().clone())),
            (Some("engine.execute"), Some("midi.notes")) => Answer::Now(ok(json!({"notes": played.lock().unwrap().clone()}))),
            _ => Answer::Now(json!({"ok": false, "error": "unknown method `x`"})),
        })
        .await;
        let reads = Reads::new(Control::new(fake.port(), None));
        App { fake, session, notes, reads }
    }

    /// A read as Kumi's app sends it: a ref or parent with the epoch it came from, here the epoch now.
    async fn read(&mut self, mut params: Value) -> Result<Value, (&'static str, String)> {
        if (params.get("ref").is_some() || params.get("parent").is_some()) && params.get("epoch").is_none() {
            params["epoch"] = self.reads.epoch().into();
        }
        self.read_bare(params).await
    }

    /// A read as given.
    async fn read_bare(&mut self, params: Value) -> Result<Value, (&'static str, String)> {
        self.reads.read(&params, Instant::now() + Duration::from_secs(5)).await
    }

    /// How often the app was asked for `method` (and an engine command, `command`).
    fn asked(&self, method: &str, command: Option<&str>) -> usize {
        let asked = |request: &Value| request["method"] == method && command.is_none_or(|command| request["params"]["command"] == command);
        self.fake.requests().iter().filter(|request| asked(request)).count()
    }
}

fn refs(answer: &Value) -> Vec<String> {
    answer["items"].as_array().unwrap().iter().map(|row| row["ref"].as_str().unwrap().to_string()).collect()
}

fn note(start: i64, pitch: u8, channel: u8, length: i64) -> Value {
    json!({"start": start, "pitch": pitch, "channel": channel, "length": length, "velocity": 100, "release_velocity": 64})
}

/// What in an answer isn't where Kumi's app looks for refs: a SoundCraft ref under a key that holds none, or under a key
/// that holds refs, something that isn't one. (daw-bridge's `misplaced_refs` knows only some of SoundCraft's kinds.)
fn misplaced(answer: &Value) -> Vec<String> {
    fn walk(value: &Value, path: &str, holds: (bool, bool), found: &mut Vec<String>) {
        let is_ref = |text: &str| text.parse::<Ref>().is_ok();
        match value {
            Value::String(text) if is_ref(text) != holds.0 => found.push(format!("{path} = {text}")),
            Value::Array(items) => {
                for (index, item) in items.iter().enumerate() {
                    match item {
                        Value::String(text) if is_ref(text) != holds.1 => found.push(format!("{path}[{index}] = {text}")),
                        Value::String(_) => {}
                        other => walk(other, &format!("{path}[{index}]"), (false, false), found),
                    }
                }
            }
            Value::Object(fields) => {
                for (key, field) in fields {
                    let here = if path.is_empty() { key.clone() } else { format!("{path}.{key}") };
                    walk(field, &here, (key_holds_a_ref(key), key_holds_refs(key)), found);
                }
            }
            _ => {}
        }
    }
    let mut found = vec![];
    walk(answer, "", (false, false), &mut found);
    found
}

#[tokio::test]
async fn every_kind_reads_as_rows_with_each_ref_under_a_key_that_holds_one() {
    let mut app = App::new(json!([note(0, 60, 0, 480), note(480, 62, 0, 240)])).await;
    for (params, rows) in [
        (json!({"kind": "session"}), 1),
        (json!({"kind": "track"}), 9),
        (json!({"kind": "clip"}), 0),
        (json!({"kind": "insert"}), 0),
        (json!({"kind": "send"}), 0),
        (json!({"kind": "bus"}), 0),
        (json!({"kind": "marker"}), 0),
        (json!({"kind": "group"}), 1),
        (json!({"kind": "source"}), 0),
        (json!({"kind": "note", "parent": "s:clip:25"}), 2),
    ] {
        let answer = app.read(params.clone()).await.unwrap();
        assert_eq!(misplaced(&answer), Vec::<String>::new(), "{params}: {answer}");
        let read = answer["items"].as_array().unwrap().len();
        assert!(read > 0 && (rows == 0 || read == rows), "{params}: {read} rows");
    }
    let group = app.read(json!({"ref": "s:group:33"})).await.unwrap();
    assert_eq!((&group["items"][0]["name"], &group["items"][0]["memberRefs"]), (&json!("Drums"), &json!(["s:track:3", "s:track:23"])));
    // The check itself: a ref under a key that holds none is found, as is a key that holds refs holding something else.
    let wrong = json!({"items": [{"ref": "s:clip:4", "source": "s:source:2", "trackRef": "Kick", "memberRefs": ["s:track:3", "Keys"]}]});
    assert_eq!(misplaced(&wrong), ["items[0].source = s:source:2", "items[0].trackRef = Kick", "items[0].memberRefs[1] = Keys"]);
}

#[tokio::test]
async fn tracks_read_as_rows_named_by_refs() {
    let mut app = App::new(json!([])).await;
    let answer = app.read(json!({"kind": "track"})).await.unwrap();
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
    // Keys' instrument is its insert too, and reads by its ref.
    let keys = app.read(json!({"kind": "insert", "parent": "s:track:23"})).await.unwrap();
    assert_eq!(
        keys["items"],
        json!([{"ref": "s:insert:23:instrument", "trackRef": "s:track:23", "slot": "instrument", "plugin": "subtractive_synth"}])
    );
    assert_eq!(app.read(json!({"ref": "s:insert:23:instrument"})).await.unwrap()["items"][0], keys["items"][0]);
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
async fn a_ref_or_parent_without_its_epoch_is_refused_and_one_from_another_table_is_stale() {
    let mut app = App::new(json!([])).await;
    assert_eq!(app.read_bare(json!({"ref": "s:track:23"})).await.unwrap_err().0, "bad-params");
    assert_eq!(app.read_bare(json!({"kind": "clip", "parent": "s:track:3"})).await.unwrap_err().0, "bad-params");
    assert_eq!(app.read_bare(json!({"kind": "track"})).await.unwrap()["total"], 9, "a list of everything needs none");
    // The bridge started again, with SoundCraft still open: its table names the same ids, and refs from before aren't its.
    let before = Reads::new(Control::new(app.fake.port(), None)).epoch().to_string();
    let stale = app.read_bare(json!({"ref": "s:track:23", "epoch": before})).await.unwrap_err();
    assert_eq!(stale.0, "stale", "{stale:?}");
}

#[tokio::test]
async fn a_clips_notes_read_under_it_and_notes_alike_are_marked_as_a_tie() {
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
async fn a_read_it_can_be_refused_for_without_the_app_is_refused_before_the_app_is_asked() {
    let mut app = App::new(json!([])).await;
    for params in [
        json!({}),
        json!({"kind": "item"}),
        json!({"kind": "track", "limit": 0}),
        json!({"kind": "track", "limit": 201}),
        json!({"kind": 3}),
        json!({"ref": "track 3"}),
        json!({"kind": "clip", "parent": "s:track:x"}),
        json!({"kind": "clip", "parent": "s:bus:1"}),
        json!({"kind": "note", "parent": "s:track:3"}),
        json!({"kind": "track", "parent": "s:track:3"}),
        json!({"kind": "track", "cursor": "c1.nope"}),
        json!({"kind": "track", "cursor": "c1.1a-1~s:track:3#1"}),
    ] {
        assert_eq!(app.read(params.clone()).await.unwrap_err().0, "bad-params", "{params}");
    }
    assert_eq!(app.asked("session.inspect", None), 0);
    // (and a read whose deadline has passed asks nothing either)
    assert_eq!(app.reads.read(&json!({"kind": "track"}), Instant::now()).await.unwrap_err().0, "deadline");
    assert_eq!(app.asked("session.inspect", None), 0);
}

#[tokio::test]
async fn an_audio_clip_has_no_notes_to_read_and_the_app_isnt_asked_for_them() {
    let mut app = App::new(json!([note(0, 60, 0, 480)])).await;
    let refused = app.read(json!({"kind": "note", "parent": "s:clip:4"})).await.unwrap_err();
    assert_eq!(refused, ("bad-params", "parent: s:clip:4 isn't a MIDI clip, so it has no notes".to_string()));
    assert_eq!(app.asked("engine.execute", Some("midi.notes")), 0, "SoundCraft would say so in its status line");
}

#[tokio::test]
async fn a_cursor_inside_notes_alike_goes_on_from_the_one_it_named_or_says_the_list_changed() {
    let alike = |length: i64| note(480, 62, 0, length);
    let mut app = App::new(json!([note(0, 60, 0, 480), alike(240), alike(480), note(960, 64, 0, 480)])).await;
    let first = app.read(json!({"kind": "note", "parent": "s:clip:25", "limit": 2})).await.unwrap();
    let cursor = first["nextCursor"].as_str().unwrap().to_string();
    assert!(cursor.ends_with("~s:note:25:480.62.0#2"), "the second of the two: {cursor}");
    // A note added before them: the page starts at the second, which it named, not the first again.
    *app.notes.lock().unwrap() = json!([note(0, 60, 0, 480), note(240, 60, 0, 120), alike(240), alike(480), note(960, 64, 0, 480)]);
    let next = app.read(json!({"kind": "note", "parent": "s:clip:25", "limit": 2, "cursor": cursor})).await.unwrap();
    let lengths: Vec<&Value> = next["items"].as_array().unwrap().iter().map(|row| &row["length"]).collect();
    assert_eq!(lengths, [&json!(480), &json!(480)], "{next}");
    // One of them deleted: the cursor's note can't be told from the other, so the list changed.
    *app.notes.lock().unwrap() = json!([note(0, 60, 0, 480), alike(480), note(960, 64, 0, 480)]);
    let changed = app.read(json!({"kind": "note", "parent": "s:clip:25", "limit": 2, "cursor": cursor})).await.unwrap_err();
    assert_eq!(changed.0, "changed");
}

#[tokio::test]
async fn a_video_clip_reads_with_its_video_and_offset() {
    let mut app = App::new(json!([])).await;
    let video = json!({"id": 34, "name": "Picture", "start": 0, "end": 48000, "length": 48000, "gain_db": 0.0, "muted": false, "fade_in": 0, "fade_out": 0, "locked": false, "video": 7, "offset": 480});
    app.session.lock().unwrap()["tracks"][0]["clips"].as_array_mut().unwrap().push(video);
    let row = app.read(json!({"ref": "s:clip:34"})).await.unwrap()["items"][0].clone();
    assert_eq!((&row["video"], &row["offset"], row.get("notes")), (&json!(7), &json!(480), None), "{row}");
}

#[tokio::test]
async fn another_session_under_the_same_name_and_path_is_a_new_epoch() {
    // File > New on an unsaved session with nothing to undo: "Untitled" again, empty, and its first track takes id 1, as
    // like the old one as can be. Only that the look between saw none of the old objects tells.
    let mut app = App::new(json!([])).await;
    let audio = json!({"id": 1, "kind": "audio", "format": "Mono", "name": "Audio 1", "instrument": null, "inserts": [], "clips": []});
    *app.session.lock().unwrap() = json!({"name": "Untitled", "path": null, "sample_rate": 48000, "tracks": [audio.clone()]});
    let before = app.read(json!({"kind": "track"})).await.unwrap()["epoch"].as_str().unwrap().to_string();
    app.session.lock().unwrap()["tracks"] = json!([]);
    tokio::time::sleep(STALE_AFTER).await;
    assert_ne!(app.read(json!({"kind": "track"})).await.unwrap()["epoch"], before.as_str(), "none of the objects: another session");
    app.session.lock().unwrap()["tracks"] = json!([audio]);
    tokio::time::sleep(STALE_AFTER).await;
    let stale = app.read(json!({"ref": "s:track:1", "epoch": before})).await.unwrap_err();
    assert_eq!(stale.0, "stale", "{stale:?}");
    // The same objects, but an undo history emptied with nothing to redo: a session opened again (Revert).
    let mut app = App::new(json!([])).await;
    app.session.lock().unwrap()["undo"] = json!("Rename Track");
    let before = app.read(json!({"kind": "track"})).await.unwrap()["epoch"].as_str().unwrap().to_string();
    app.session.lock().unwrap()["redo"] = json!("Rename Track");
    app.session.lock().unwrap()["undo"] = Value::Null;
    tokio::time::sleep(STALE_AFTER).await;
    assert_eq!(app.read(json!({"kind": "track"})).await.unwrap()["epoch"], before.as_str(), "an undo: the same session");
    app.session.lock().unwrap()["redo"] = Value::Null;
    tokio::time::sleep(STALE_AFTER).await;
    assert_ne!(app.read(json!({"kind": "track"})).await.unwrap()["epoch"], before.as_str(), "nothing to redo either: another session");
}

#[tokio::test]
async fn a_look_between_two_reads_serves_the_next_read_its_rows_and_lives_from_one_look() {
    let mut app = App::new(json!([])).await;
    app.read(json!({"kind": "track"})).await.unwrap();
    // The look loop sees Kick's id on another track; the next read, within the second, doesn't read the old one under it.
    let changed = {
        let mut session = app.session.lock().unwrap();
        session["tracks"][0]["name"] = json!("Bass");
        session.clone()
    };
    app.reads.saw(&changed);
    let rows = app.read(json!({"kind": "track"})).await.unwrap();
    assert_eq!((&rows["items"][0]["ref"], &rows["items"][0]["name"]), (&json!("s:track:3.2"), &json!("Bass")));
}
