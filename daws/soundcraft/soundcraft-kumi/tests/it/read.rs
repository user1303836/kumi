//! `read` against a fake SoundCraft that shows the session SoundCraft 0.17.0 showed (tests/vectors), with a group added:
//! rows named by refs, each under a key that may hold one; pages whose cursors find their row again, or say the list
//! changed; refs and cursors from another session or table refused as stale; and what a read can be refused for without
//! the app refused before it's asked.
use super::fake::{ok, Answer, Fake};
use super::vectors::vectors;
use daw_bridge::contract::{key_holds_a_ref, key_holds_refs};
use serde_json::{json, Value};
use soundcraft_kumi::control::Control;
use soundcraft_kumi::read::{content, Reads};
use soundcraft_kumi::refs::Ref;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// How long one inspection serves the reads after it, and a moment more.
const STALE_AFTER: Duration = Duration::from_millis(1100);

/// The session SoundCraft 0.17.0 showed (`session.inspect {"detail":"full"}`).
pub fn recorded() -> Value {
    let all = vectors("soundcraft-0.17.0.ndjson");
    let full = all.iter().find(|exchange| exchange["method"] == "session.inspect" && exchange["params"]["detail"] == "full").unwrap();
    full["reply"]["result"].clone()
}

/// The built-in plug-ins SoundCraft 0.17.0 describes (`engine.plugins`, as soundcraft-cli 0.17.0's engine answered it).
pub fn plugins() -> Value {
    vectors("soundcraft-0.17.0-plugins.json").remove(0)
}

/// A fake SoundCraft showing `session` (the recorded one, with a group of Kick and Keys), whose MIDI clips have `notes`,
/// and which describes its built-in plug-ins as 0.17.0 does.
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
        let (shown, played, described) = (session.clone(), notes.clone(), plugins());
        let fake = Fake::start(16, move |request| match (request["method"].as_str(), request["params"]["command"].as_str()) {
            (Some("session.inspect"), _) => Answer::Now(ok(shown.lock().unwrap().clone())),
            (Some("engine.execute"), Some("midi.notes")) => Answer::Now(ok(json!({"notes": played.lock().unwrap().clone()}))),
            (Some("engine.execute"), Some("engine.plugins")) => Answer::Now(ok(described.clone())),
            _ => Answer::Now(json!({"ok": false, "error": "unknown method `x`"})),
        })
        .await;
        let reads = Reads::new(Control::new(fake.port(), None), Some("0.17.0".into()));
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
pub fn misplaced(answer: &Value) -> Vec<String> {
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
        (json!({"kind": "param", "parent": "s:insert:3:a"}), 23),
        (json!({"kind": "env"}), 1),
        (json!({"kind": "point", "parent": "s:env:15:volume"}), 4),
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
        (&kick["ref"], &kick["name"], &kick["selected"], &kick["deviceCount"]),
        (&json!("s:track:3"), &json!("Kick"), &json!(true), &json!(2))
    );
    let verb = rows.iter().find(|row| row["name"] == "Verb").unwrap();
    assert_eq!((&verb["type"], &verb["inputRef"], &verb["output"]), (&json!("aux"), &json!("s:bus:1"), &json!("Main")));
    let keys = rows.iter().find(|row| row["name"] == "Keys").unwrap();
    assert_eq!(
        (&keys["instrument"], &keys["sendCount"], &keys["deviceCount"]),
        (&json!("subtractive_synth"), &json!(1), &json!(1)),
        "its instrument is a device"
    );
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
    // Another song: another name, and objects not these (a save, under another name with the same ones, isn't another).
    {
        let mut session = app.session.lock().unwrap();
        session["name"] = json!("Another Song");
        session["tracks"][0]["name"] = json!("Snare");
    }
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
    assert_eq!(app.read(json!({"ref": "s:playlist:3:1"})).await.unwrap_err().0, "bad-params", "not read yet");
    assert_eq!(app.read(json!({"ref": "track 3"})).await.unwrap_err().0, "bad-params");
}

#[tokio::test]
async fn a_ref_or_parent_without_its_epoch_is_refused_and_one_from_another_table_is_stale() {
    let mut app = App::new(json!([])).await;
    assert_eq!(app.read_bare(json!({"ref": "s:track:23"})).await.unwrap_err().0, "bad-params");
    assert_eq!(app.read_bare(json!({"kind": "clip", "parent": "s:track:3"})).await.unwrap_err().0, "bad-params");
    assert_eq!(app.read_bare(json!({"kind": "track"})).await.unwrap()["total"], 9, "a list of everything needs none");
    // The bridge started again, with SoundCraft still open: its table names the same ids, and refs from before aren't its.
    let before = Reads::new(Control::new(app.fake.port(), None), Some("0.17.0".into())).epoch().to_string();
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
        json!({"kind": "param", "parent": "s:track:3"}),
        json!({"kind": "param", "parent": "s:insert:23:instrument"}),
        json!({"kind": "point", "parent": "s:track:3"}),
        json!({"kind": "env", "parent": "s:insert:3:a"}),
        json!({"ref": "s:playlist:3:1"}),
        json!({"ref": "s:param:23:instrument:cutoff"}),
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

#[tokio::test]
async fn an_inserts_parameters_read_with_the_names_ranges_and_choices_soundcraft_describes_and_their_values_now() {
    let mut app = App::new(json!([])).await;
    // (Kick's slot c gets a hosted plug-in, whose parameters SoundCraft knows only by number, and keeps by their text)
    app.session.lock().unwrap()["tracks"][0]["inserts"]
        .as_array_mut()
        .unwrap()
        .push(json!({"slot": 2, "plugin": "au:aufx:bpas:appl", "bypass": false, "params": {"0": 5000.0, "1": 600.0, "10": 1.0, "2": 0.5}}));
    let answer = app.read(json!({"kind": "param", "parent": "s:insert:3:a", "limit": 200})).await.unwrap();
    assert_eq!(misplaced(&answer), Vec::<String>::new(), "{answer}");
    let rows = answer["items"].as_array().unwrap();
    let described = plugins();
    let eq = described.as_array().unwrap().iter().find(|plugin| plugin["id"] == "eq_7band").unwrap();
    assert_eq!(rows.len(), eq["params"].as_array().unwrap().len());
    let hpf = rows.iter().find(|row| row["id"] == "hpf_freq").unwrap();
    assert_eq!(
        hpf,
        &json!({"ref": "s:param:3:a:hpf_freq", "insertRef": "s:insert:3:a", "id": "hpf_freq", "name": "HPF Freq", "value": 80.0, "min": 10.0, "max": 2000.0, "default": 80.0, "unit": "Hz", "taper": "Log"})
    );
    let slope = rows.iter().find(|row| row["id"] == "hpf_slope").unwrap();
    assert_eq!((&slope["value"], &slope["choice"], slope["choices"].as_array().map(Vec::len)), (&json!(1.0), &json!("12 dB/oct"), Some(4)));
    assert_eq!(app.read(json!({"ref": "s:param:3:a:hpf_freq"})).await.unwrap()["items"][0], *hpf);
    let hosted = app.read(json!({"kind": "param", "parent": "s:insert:3:c"})).await.unwrap();
    assert_eq!(
        hosted["items"],
        json!([
            {"ref": "s:param:3:c:0", "insertRef": "s:insert:3:c", "id": "0", "value": 5000.0},
            {"ref": "s:param:3:c:1", "insertRef": "s:insert:3:c", "id": "1", "value": 600.0},
            {"ref": "s:param:3:c:2", "insertRef": "s:insert:3:c", "id": "2", "value": 0.5},
            {"ref": "s:param:3:c:10", "insertRef": "s:insert:3:c", "id": "10", "value": 1.0},
        ]),
        "by number, as the plug-in numbers them"
    );
    assert_eq!(app.asked("engine.execute", Some("engine.plugins")), 1, "the descriptions come with the app's version: asked once");
    for (params, code) in
        [(json!({"kind": "param", "parent": "s:insert:3:j"}), "failed"), (json!({"ref": "s:param:3:a:nothing"}), "failed")]
    {
        assert_eq!(app.read(params.clone()).await.unwrap_err().0, code, "{params}");
    }
}

#[tokio::test]
async fn automation_reads_as_lanes_named_as_soundcrafts_automation_commands_name_them_and_as_their_points() {
    let mut app = App::new(json!([])).await;
    let point = |at: i64, value: f64| json!({"at": at, "value": value});
    // (inspect labels lanes for people: `insert A hpf_freq` is what automation.set_point calls `plugin:a:hpf_freq`, and
    // the trim lane, `insert Z trim`, is `plugin:255:trim`; slots past J, and a parameter a command named with a space,
    // have no id a ref can spell, so no row)
    app.session.lock().unwrap()["tracks"][0]["automation"] = json!([
        {"param": "volume", "points": [point(48000, -6.0)]},
        {"param": "insert A hpf_freq", "points": [point(96000, 120.0), point(144000, 200.0)]},
        {"param": "send B level", "points": [point(0, -3.0)]},
        {"param": "pan 2", "points": [point(0, 0.5)]},
        {"param": "insert Z trim", "points": [point(0, -2.0)]},
        {"param": "pan", "points": [point(0, -0.5)]},
        {"param": "mute", "points": [point(0, 1.0)]},
        {"param": "send C pan", "points": [point(0, 0.25)]},
        {"param": "send C mute", "points": [point(0, 0.0)]},
        {"param": "insert B HighGain", "points": [point(0, 2.0)]},
        {"param": "insert K x", "points": [point(0, 1.0)]},
        {"param": "send K level", "points": [point(0, 1.0)]},
        {"param": "insert A hpf freq", "points": [point(0, 1.0)]},
    ]);
    let lanes = app.read(json!({"kind": "env", "parent": "s:track:3"})).await.unwrap();
    assert_eq!(misplaced(&lanes), Vec::<String>::new(), "{lanes}");
    let rows = lanes["items"].as_array().unwrap();
    assert_eq!(
        rows.iter()
            .map(|row| (row["ref"].as_str().unwrap(), row["label"].as_str().unwrap(), row["points"].as_u64().unwrap()))
            .collect::<Vec<_>>(),
        [
            ("s:env:3:volume", "volume", 1),
            ("s:env:3:plugin:a:hpf_freq", "insert A hpf_freq", 2),
            ("s:env:3:send_b_level", "send B level", 1),
            ("s:env:3:pan2", "pan 2", 1),
            ("s:env:3:plugin:255:trim", "insert Z trim", 1),
            ("s:env:3:pan", "pan", 1),
            ("s:env:3:mute", "mute", 1),
            ("s:env:3:send_c_pan", "send C pan", 1),
            ("s:env:3:send_c_mute", "send C mute", 1),
            ("s:env:3:plugin:b:HighGain", "insert B HighGain", 1),
        ]
    );
    assert_eq!(rows[1]["trackRef"], "s:track:3");
    let every = refs(&app.read(json!({"kind": "env"})).await.unwrap());
    assert_eq!(every, [refs(&lanes), vec!["s:env:15:volume".into()]].concat(), "every track's: Kick's, then the Pad's fade");
    // (the Pad's fade in and out, as SoundCraft 0.17.0 showed it)
    let fade = app.read(json!({"kind": "point", "parent": "s:env:15:volume"})).await.unwrap();
    assert_eq!(
        fade["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|point| (point["at"].as_i64().unwrap(), point["value"].as_f64().unwrap()))
            .collect::<Vec<_>>(),
        [(0, -30.0), (443072, -8.0), (2215360, -8.0), (2658432, -24.0)]
    );
    assert_eq!(fade["items"][1]["seconds"], 9.231, "443072 samples at 48 kHz");
    let points = app.read(json!({"kind": "point", "parent": "s:env:3:plugin:a:hpf_freq"})).await.unwrap();
    assert_eq!(
        points["items"],
        json!([
            {"ref": "s:point:3:plugin:a:hpf_freq:96000", "laneRef": "s:env:3:plugin:a:hpf_freq", "at": 96000, "seconds": 2.0, "value": 120.0},
            {"ref": "s:point:3:plugin:a:hpf_freq:144000", "laneRef": "s:env:3:plugin:a:hpf_freq", "at": 144000, "seconds": 3.0, "value": 200.0},
        ])
    );
    assert_eq!(app.read(json!({"ref": "s:env:3:send_b_level"})).await.unwrap()["items"][0], rows[2]);
    assert_eq!(app.read(json!({"ref": "s:point:3:plugin:a:hpf_freq:144000"})).await.unwrap()["items"][0], points["items"][1]);
    for (params, code) in
        [(json!({"kind": "point", "parent": "s:env:3:send_d_level"}), "failed"), (json!({"ref": "s:point:3:volume:1"}), "failed")]
    {
        assert_eq!(app.read(params.clone()).await.unwrap_err().0, code, "{params}");
    }
}

#[tokio::test]
async fn two_points_at_one_sample_from_a_loaded_session_are_a_tie_and_never_a_guess() {
    let mut app = App::new(json!([])).await;
    let point = |at: i64, value: f64| json!({"at": at, "value": value});
    // (SoundCraft's own writes keep one point at a sample; a session file it loaded can hold two)
    app.session.lock().unwrap()["tracks"][0]["automation"] =
        json!([{"param": "volume", "points": [point(0, -6.0), point(0, 0.0), point(48000, -3.0)]}]);
    let points = app.read(json!({"kind": "point", "parent": "s:env:3:volume"})).await.unwrap();
    let ties: Vec<&Value> = points["items"].as_array().unwrap().iter().map(|row| &row["tie"]).collect();
    assert_eq!(ties, [&json!(2), &json!(2), &Value::Null]);
    assert_eq!(app.read(json!({"ref": "s:point:3:volume:0"})).await.unwrap_err().0, "failed", "which of the two isn't known");
    assert_eq!(app.read(json!({"ref": "s:point:3:volume:48000"})).await.unwrap()["items"][0]["value"], -3.0);
}

/// The snapshot as Kumi's look takes it.
async fn snapshot(app: &mut App) -> Value {
    app.reads.snapshot(Instant::now() + Duration::from_secs(5)).await.unwrap()
}

#[tokio::test]
async fn a_snapshot_shows_the_session_on_one_screen() {
    let mut app = App::new(json!([])).await;
    let snapshot = snapshot(&mut app).await;
    assert_eq!(misplaced(&snapshot), Vec::<String>::new(), "{snapshot}");
    let epoch = app.reads.epoch().to_string();
    assert_eq!(snapshot["epoch"], epoch.as_str());
    // (the recorded session was never saved: no place; its identity is the open session's)
    let project = &snapshot["project"];
    assert_eq!((&project["name"], &project["id"], &project["identity"]), (&json!("Midnight Groove"), &Value::Null, &json!(epoch)));
    let tracks = snapshot["tracks"].as_array().unwrap();
    assert_eq!(tracks.len(), 9);
    assert_eq!((&tracks[0]["ref"], &tracks[0]["type"], &tracks[0]["deviceCount"]), (&json!("s:track:3"), &json!("audio"), &json!(2)));
    assert_eq!(
        (&snapshot["selection"]["trackRef"], &snapshot["selection"]["track"]),
        (&json!("s:track:3"), &json!({"name": "Kick", "type": "audio"}))
    );
    // At the start: 104 bpm in 4/4.
    let transport = &snapshot["transport"];
    assert_eq!(
        (&transport["state"], &transport["beat"], &transport["bar"], &transport["beatInBar"]),
        (&json!("stopped"), &json!(0.0), &json!(1), &json!(1.0))
    );
    assert_eq!((&transport["tempo"], &transport["timeSignature"], &transport["beatsPerBar"]), (&json!(104.0), &json!([4, 4]), &json!(4.0)));
    assert_eq!(snapshot["markers"].as_array().map(Vec::len), recorded()["markers"].as_array().map(Vec::len), "every marker");
    assert!(snapshot.get("truncated").is_none(), "nothing left out");
}

/// A session file's path, absolute on every system (Windows' needs a drive), so its place has an id.
fn session_file(path: &str) -> String {
    if cfg!(windows) {
        format!("C:{path}")
    } else {
        path.to_string()
    }
}

#[tokio::test]
async fn a_saved_sessions_id_is_its_files_place_and_the_session_read_says_the_same() {
    let path = json!(session_file("/sessions/Midnight Groove/Midnight Groove.scraft"));
    let mut app = App::new(json!([])).await;
    app.session.lock().unwrap()["path"] = path.clone();
    let project = snapshot(&mut app).await["project"].clone();
    let id = project["id"].as_str().unwrap();
    assert!(id.len() == 32 && id.bytes().all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)), "a place's id: {id}");
    assert_eq!(project["identity"], app.reads.epoch(), "the open session's, not its file's");
    let session = app.read(json!({"kind": "session"})).await.unwrap();
    assert_eq!(
        (&session["items"][0]["id"], &session["items"][0]["identity"]),
        (&project["id"], &project["identity"]),
        "what Kumi compares when the epoch moves"
    );
    // The bridge started again, the same file open: the same id, so its conversations are where they were.
    let mut again = App::new(json!([])).await;
    again.session.lock().unwrap()["path"] = path;
    assert_eq!(snapshot(&mut again).await["project"]["id"], project["id"]);
    // A relative path is resolved from a folder the bridge can't know: no place.
    let mut relative = App::new(json!([])).await;
    relative.session.lock().unwrap()["path"] = json!("Midnight Groove.scraft");
    assert_eq!(snapshot(&mut relative).await["project"]["id"], Value::Null);
}

#[cfg(unix)]
#[tokio::test]
async fn a_session_file_opened_through_a_link_is_the_place_of_the_file_it_links_to() {
    let folder = tempfile::tempdir().unwrap();
    let (file, link) = (folder.path().join("Groove.scraft"), folder.path().join("Groove link.scraft"));
    std::fs::write(&file, b"{}").unwrap();
    std::os::unix::fs::symlink(&file, &link).unwrap();
    let (mut direct, mut linked) = (App::new(json!([])).await, App::new(json!([])).await);
    direct.session.lock().unwrap()["path"] = json!(file.to_str().unwrap());
    linked.session.lock().unwrap()["path"] = json!(link.to_str().unwrap());
    let (direct, linked) = (snapshot(&mut direct).await["project"]["id"].clone(), snapshot(&mut linked).await["project"]["id"].clone());
    assert!(direct.is_string());
    assert_eq!(direct, linked);
}

#[tokio::test]
async fn a_save_or_a_save_as_is_the_same_session_with_its_refs_and_identity() {
    let mut app = App::new(json!([])).await;
    let before = snapshot(&mut app).await;
    let (epoch, identity) = (before["epoch"].clone(), before["project"]["identity"].clone());
    assert_eq!(before["project"]["id"], Value::Null, "never saved");
    let names = |app: &App, name: &str, path: &str| {
        let mut session = app.session.lock().unwrap();
        (session["name"], session["path"]) = (json!(name), json!(path));
    };
    // Its first save: SoundCraft names the session after its file, and nothing else changes.
    names(&app, "Groove", &session_file("/sessions/Groove.scraft"));
    tokio::time::sleep(STALE_AFTER).await;
    let saved = snapshot(&mut app).await;
    assert_eq!((&saved["epoch"], &saved["project"]["identity"]), (&epoch, &identity), "the same session");
    assert!(saved["project"]["id"].is_string(), "with a place now");
    let kick = app.read(json!({"ref": "s:track:3", "epoch": epoch})).await.unwrap();
    assert_eq!(kick["items"][0]["name"], "Kick", "its refs hold");
    // Save As: another file and place, the same session.
    names(&app, "Groove 2", &session_file("/sessions/Groove 2.scraft"));
    tokio::time::sleep(STALE_AFTER).await;
    let saved_as = snapshot(&mut app).await;
    assert_eq!((&saved_as["epoch"], &saved_as["project"]["identity"]), (&epoch, &identity));
    assert_ne!(saved_as["project"]["id"], saved["project"]["id"]);
    // Another file opened, whose objects aren't these: another session.
    names(&app, "Other", &session_file("/sessions/Other.scraft"));
    app.session.lock().unwrap()["tracks"][0]["name"] = json!("Snare");
    tokio::time::sleep(STALE_AFTER).await;
    let other = snapshot(&mut app).await;
    assert_ne!(other["epoch"], epoch);
    assert_ne!(other["project"]["identity"], identity);
}

#[tokio::test]
async fn another_file_whose_objects_are_who_they_were_but_hold_other_things_is_another_session() {
    // A "v2" of the song, opened with nothing to undo in either: every object who it was, but its audio clips moved and
    // Kick's first insert another plug-in.
    let mut app = App::new(json!([])).await;
    let epoch = snapshot(&mut app).await["epoch"].clone();
    {
        let mut session = app.session.lock().unwrap();
        (session["name"], session["path"]) = (json!("Groove v2"), json!(session_file("/sessions/Groove v2.scraft")));
        let tracks = session["tracks"].as_array_mut().unwrap();
        for clip in tracks.iter_mut().flat_map(|track| track["clips"].as_array_mut().into_iter().flatten()) {
            if clip.get("source").is_some() {
                clip["start"] = json!(clip["start"].as_i64().unwrap() + 48000);
            }
        }
        tracks[0]["inserts"][0]["plugin"] = json!("compressor");
    }
    tokio::time::sleep(STALE_AFTER).await;
    assert_ne!(snapshot(&mut app).await["epoch"], epoch);
}

#[tokio::test]
async fn a_session_renamed_on_the_same_path_is_the_same_session() {
    // Setup > Session renames it as a step of its own: the history moves, the file and the objects stay.
    let mut app = App::new(json!([])).await;
    app.session.lock().unwrap()["path"] = json!(session_file("/sessions/Groove.scraft"));
    let epoch = snapshot(&mut app).await["epoch"].clone();
    {
        let mut session = app.session.lock().unwrap();
        (session["name"], session["undo"]) = (json!("Groove (final)"), json!("Session Setup"));
    }
    tokio::time::sleep(STALE_AFTER).await;
    assert_eq!(snapshot(&mut app).await["epoch"], epoch);
}

#[tokio::test]
async fn deleting_every_object_in_one_step_is_the_same_session() {
    let mut app = App::new(json!([])).await;
    let epoch = snapshot(&mut app).await["epoch"].clone();
    {
        let mut session = app.session.lock().unwrap();
        for key in ["tracks", "busses", "markers", "groups", "sources"] {
            session[key] = json!([]);
        }
        session["undo"] = json!("Delete");
    }
    tokio::time::sleep(STALE_AFTER).await;
    assert_eq!(snapshot(&mut app).await["epoch"], epoch, "a step took them, so the history moved: the same session");
}

#[tokio::test]
async fn a_big_selection_is_named_in_part_and_counted_in_full() {
    let mut app = App::new(json!([])).await;
    {
        let mut session = app.session.lock().unwrap();
        let kick = session["tracks"][0].clone();
        let hit = kick["clips"][0].clone();
        assert!(hit.is_object(), "Kick has a clip to copy");
        let tracks = session["tracks"].as_array_mut().unwrap();
        for n in 0..60 {
            let mut copy = kick.clone();
            let mut clip = hit.clone();
            clip["id"] = json!(5000 + n);
            (copy["id"], copy["name"], copy["clips"]) = (json!(1000 + n), json!(format!("Copy {n}")), json!([clip]));
            tracks.push(copy);
        }
        session["selection"]["tracks"] = json!((1000..1060).collect::<Vec<_>>());
        session["selection"]["clips"] = json!((5000..5030).collect::<Vec<_>>());
    }
    let selection = snapshot(&mut app).await["selection"].clone();
    assert_eq!(
        (&selection["trackRef"], &selection["trackCount"], &selection["clipCount"]),
        (&json!("s:track:1000"), &json!(60), &json!(30))
    );
    assert_eq!(
        (selection["trackRefs"].as_array().map(Vec::len), selection["clipRefs"].as_array().map(Vec::len)),
        (Some(50), Some(20)),
        "named as REAPER names them"
    );
}

#[tokio::test]
async fn the_beat_in_a_bar_counts_quarter_notes_in_any_meter() {
    // (at 120 bpm and 48 kHz a quarter note is 24000 samples)
    for ((numerator, denominator), quarters, bar, in_bar) in [((6, 8), 1.5, 1, 2.5), ((6, 8), 3.5, 2, 1.5), ((4, 4), 1.5, 1, 2.5)] {
        let mut app = App::new(json!([])).await;
        {
            let mut session = app.session.lock().unwrap();
            session["tempo"] = json!([{"bpm": 120.0, "tick": 0}]);
            session["meter"] = json!([{"denominator": denominator, "numerator": numerator, "tick": 0}]);
            session["transport"]["position"] = json!(quarters * 24000.0);
        }
        let transport = snapshot(&mut app).await["transport"].clone();
        let at = format!("{numerator}/{denominator}, {quarters} quarter notes in");
        assert_eq!((&transport["bar"], &transport["beatInBar"]), (&json!(bar), &json!(in_bar)), "{at}");
    }
}

#[tokio::test]
async fn a_snapshot_names_the_tracks_it_leaves_out_and_read_goes_on_from_there() {
    let mut app = App::new(json!([])).await;
    {
        let mut session = app.session.lock().unwrap();
        let tracks = session["tracks"].as_array_mut().unwrap();
        let kick = tracks[0].clone();
        for n in 0..241 {
            let mut copy = kick.clone();
            (copy["id"], copy["name"]) = (json!(1000 + n), json!(format!("Copy {n}")));
            tracks.push(copy);
        }
    }
    let snapshot = snapshot(&mut app).await;
    assert_eq!(snapshot["tracks"].as_array().unwrap().len(), 200);
    let left = &snapshot["truncated"]["tracks"];
    assert_eq!(left["total"], 250);
    let rest = app.read(json!({"kind": "track", "cursor": left["nextCursor"], "limit": 100})).await.unwrap();
    assert_eq!(refs(&rest)[0], "s:track:1191", "the 201st track, the first the snapshot left out");
    assert!(rest.get("nextCursor").is_none(), "and the 50 after it are all");
}

#[tokio::test]
async fn a_tempo_map_with_more_than_steps_gives_the_transport_no_beats() {
    let mut app = App::new(json!([])).await;
    app.session.lock().unwrap()["tempo"][0]["ramp"] = json!(true);
    let transport = snapshot(&mut app).await["transport"].clone();
    assert_eq!((&transport["seconds"], &transport["state"]), (&json!(0.0), &json!("stopped")));
    assert!(transport.get("beat").is_none() && transport.get("timeSignature").is_none(), "no beats rather than wrong ones: {transport}");
}

#[test]
fn the_sessions_content_reads_the_same_from_its_summary_and_moves_only_with_an_edit() {
    let full = recorded();
    // (the summary of the same session: no plug-in's parameters, and automation points counted)
    let mut summary = full.clone();
    for track in summary["tracks"].as_array_mut().unwrap() {
        for insert in track["inserts"].as_array_mut().unwrap() {
            insert["params"] = Value::Null;
        }
        for lane in track["automation"].as_array_mut().into_iter().flatten() {
            lane["points"] = json!(lane["points"].as_array().map_or(0, Vec::len));
        }
    }
    assert_eq!(content(&summary), content(&full));
    let mut moved = full.clone();
    moved["selection"]["tracks"] = json!([23]);
    moved["transport"]["position"] = json!(96000);
    moved["dirty"] = json!(true);
    assert_eq!(content(&moved), content(&full), "a selection, the playhead and a save aren't edits");
    let mut edited = full.clone();
    edited["tracks"][0]["volume_db"] = json!(-6.0);
    assert_ne!(content(&edited), content(&full), "a fader is");
}

#[tokio::test]
async fn a_look_that_sees_the_session_as_it_was_leaves_the_reads_their_inspection() {
    let mut app = App::new(json!([])).await;
    let inspections = |app: &App| app.fake.requests().iter().filter(|request| request["params"]["detail"] == "full").count();
    app.read(json!({"kind": "track"})).await.unwrap();
    let session = app.session.lock().unwrap().clone();
    app.reads.looked(&session, content(&session));
    app.read(json!({"kind": "marker"})).await.unwrap();
    assert_eq!(inspections(&app), 1, "nothing new: the first read's inspection serves the next");
    let edited = {
        let mut session = app.session.lock().unwrap();
        session["tracks"][0]["volume_db"] = json!(-6.0);
        session.clone()
    };
    app.reads.looked(&edited, content(&edited));
    app.read(json!({"kind": "track"})).await.unwrap();
    assert_eq!(inspections(&app), 2, "the look saw a change: the next read inspects again");
}
