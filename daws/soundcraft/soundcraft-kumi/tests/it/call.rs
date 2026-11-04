//! `read {kind: "call"}` against a fake SoundCraft that shows the session SoundCraft 0.17.0 showed (tests/vectors) and
//! lists its commands: a read-only command runs with its refs sent as ids and its answer's ids back as refs; anything
//! else is refused before it's sent, with what it is; a ref from another epoch or life is stale; and an answer past the
//! bytes a read may take is cut, saying where.
use super::fake::{ok, refused, Answer, Fake};
use super::read::{misplaced, recorded};
use super::vectors::vectors;
use serde_json::{json, Value};
use soundcraft_kumi::control::Control;
use soundcraft_kumi::read::Reads;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// The bytes a read's answer may take (read.rs's `ANSWER_BYTES`).
const ANSWER_BYTES: usize = 224 * 1024;

/// A fake SoundCraft showing the recorded session, listing 0.17.0's commands, and answering the engine commands a test
/// sets (by id), else `unknown command`.
struct App {
    fake: Fake,
    answers: Arc<Mutex<Value>>,
    reads: Reads,
}

impl App {
    async fn new() -> App {
        let session = recorded();
        let commands = vectors("soundcraft-0.17.0.ndjson").remove(0)["reply"]["result"].clone();
        let answers = Arc::new(Mutex::new(json!({})));
        let answering = answers.clone();
        let fake = Fake::start(16, move |request| match (request["method"].as_str(), request["params"]["command"].as_str()) {
            (Some("session.inspect"), _) => Answer::Now(ok(session.clone())),
            (Some("engine.commands"), _) => Answer::Now(ok(commands.clone())),
            (Some("engine.execute"), Some(command)) => match answering.lock().unwrap().get(command) {
                Some(answer) => Answer::Now(answer.clone()),
                None => Answer::Now(refused(&format!("unknown command `{command}`"))),
            },
            _ => Answer::Now(refused("unknown method `x`")),
        })
        .await;
        let reads = Reads::new(Control::new(fake.port(), None));
        App { fake, answers, reads }
    }

    /// What the app answers `command`: `{ok, result | error}`.
    fn answers(&self, command: &str, answer: Value) {
        self.answers.lock().unwrap()[command] = answer;
    }

    /// A call as Kumi's app sends it, with the epoch now.
    async fn call(&mut self, command: &str, params: Value) -> Result<Value, (&'static str, String)> {
        let epoch = self.reads.epoch().to_string();
        self.read(json!({"kind": "call", "command": command, "params": params, "epoch": epoch})).await
    }

    async fn read(&mut self, params: Value) -> Result<Value, (&'static str, String)> {
        self.reads.read(&params, Instant::now() + Duration::from_secs(5)).await
    }

    /// The engine commands the app was sent, with their params.
    fn executed(&self) -> Vec<Value> {
        self.fake
            .requests()
            .into_iter()
            .filter(|request| request["method"] == "engine.execute")
            .map(|request| request["params"].clone())
            .collect()
    }

    fn asked(&self, method: &str) -> usize {
        self.fake.requests().iter().filter(|request| request["method"] == method).count()
    }
}

/// The recorded session's track `id`, as `track.inspect` answers it.
fn track(id: u64) -> Value {
    recorded()["tracks"].as_array().unwrap().iter().find(|track| track["id"] == id).unwrap().clone()
}

#[tokio::test]
async fn a_read_only_command_gets_its_refs_as_ids_and_gives_the_ids_in_its_answer_back_as_refs() {
    let mut app = App::new().await;
    app.answers("track.inspect", ok(track(15)));
    let pad = app.call("track.inspect", json!({"track": "s:track:15"})).await.unwrap();
    assert_eq!(app.executed(), [json!({"command": "track.inspect", "params": {"track": 15}})], "the app sees the id");
    assert_eq!((&pad["command"], &pad["epoch"]), (&json!("track.inspect"), &json!(app.reads.epoch())));
    let result = &pad["result"];
    assert_eq!((&result["ref"], &result["name"], result.get("id")), (&json!("s:track:15"), &json!("Pad"), None));
    assert_eq!(result["inserts"][0]["ref"], "s:insert:15:a");
    assert_eq!((&result["sends"][0]["ref"], &result["sends"][0]["targetRef"]), (&json!("s:send:15:0"), &json!("s:bus:1")));
    assert_eq!(result["automation"][0]["ref"], "s:env:15:volume");
    assert_eq!((&result["clips"][0]["ref"], &result["clips"][0]["sourceRef"]), (&json!("s:clip:16"), &json!("s:source:14")));
    assert_eq!(misplaced(&pad), Vec::<String>::new(), "{pad}");
    assert!(pad.get("truncated").is_none());

    // The session as inspect shows it: each object by its ref, and the selection too.
    app.answers("session.inspect", ok(recorded()));
    let session = app.call("session.inspect", json!({"detail": "full"})).await.unwrap();
    let result = &session["result"];
    assert_eq!((&result["tracks"][0]["ref"], &result["busses"][0]["ref"]), (&json!("s:track:3"), &json!("s:bus:1")));
    assert_eq!((&result["markers"][0]["ref"], &result["sources"][0]["ref"]), (&json!("s:marker:29"), &json!("s:source:2")));
    assert_eq!((&result["selection"]["trackRefs"], &result["selection"]["clipRefs"]), (&json!(["s:track:3"]), &json!([])));
    assert_eq!(result["tracks"][7]["inputRef"], "s:bus:1", "Verb's input, the bus");
    assert_eq!(misplaced(&session), Vec::<String>::new(), "{session}");

    // An answer whose objects are named by other keys.
    let issue = json!({"code": "missing_source", "track_id": 3, "clip_id": 4, "source_id": 2, "track": "Kick", "clip": "Kick"});
    app.answers("session.audio_health", ok(json!({"issue_count": 1, "issues": [issue]})));
    let health = app.call("session.audio_health", json!({})).await.unwrap();
    let issue = &health["result"]["issues"][0];
    assert_eq!(
        (&issue["trackRef"], &issue["clipRef"], &issue["sourceRef"], issue.get("track_id")),
        (&json!("s:track:3"), &json!("s:clip:4"), &json!("s:source:2"), None)
    );

    // And one with no object in it, which needs no look at the session and no epoch.
    app.answers("engine.history", ok(json!({"undo": ["New Track"], "can_redo": false})));
    let looks = app.asked("session.inspect");
    let history = app.read(json!({"kind": "call", "command": "engine.history"})).await.unwrap();
    assert_eq!(history["result"], json!({"undo": ["New Track"], "can_redo": false}));
    assert_eq!(app.asked("session.inspect"), looks);
}

#[tokio::test]
async fn a_command_that_changes_something_is_refused_and_never_sent() {
    let mut app = App::new().await;
    for (command, does) in [
        ("track.delete", "changes the session or the app"),
        ("transport.status", "changes the session or the app"),
        ("view.clip_gain_info", "changes the session or the app"),
        ("window.config_new", "changes the session or the app"),
        ("window.mix", "only opens or closes a window"),
        ("ui.new_tracks_dialog", "only opens a dialog"),
    ] {
        let refused = app.call(command, json!({})).await.unwrap_err();
        assert_eq!(refused, ("bad-params", format!("Kumi can't change SoundCraft yet: {command} {does}")));
    }
    assert_eq!(app.executed(), Vec::<Value>::new());
    assert_eq!(app.asked("engine.commands"), 1, "the app's list, asked once");
}

#[tokio::test]
async fn a_method_of_the_control_channel_is_refused_without_asking_the_app() {
    let mut app = App::new().await;
    for method in ["ui.click", "ui.screenshot", "engine.execute", "app.quit", "document.inspect"] {
        let (code, words) = app.call(method, json!({})).await.unwrap_err();
        assert_eq!(code, "bad-params");
        assert!(words.starts_with(&format!("{method} is a method of SoundCraft's control channel, which call doesn't reach")), "{words}");
    }
    assert_eq!(app.fake.requests(), Vec::<Value>::new());
}

#[tokio::test]
async fn a_plug_in_scan_is_refused_without_asking_the_app() {
    let mut app = App::new().await;
    for command in ["engine.clap_plugins", "engine.vst3_plugins"] {
        let (code, words) = app.call(command, json!({"rescan": true})).await.unwrap_err();
        assert_eq!(code, "bad-params");
        assert!(words.starts_with(&format!("{command} scans plug-ins inside SoundCraft's own process")), "{words}");
    }
    assert_eq!(app.fake.requests(), Vec::<Value>::new());
}

#[tokio::test]
async fn an_unknown_command_is_refused_with_the_ids_near_it() {
    let mut app = App::new().await;
    let refused = app.call("track.inspct", json!({})).await.unwrap_err();
    assert_eq!(refused.0, "bad-params");
    assert!(refused.1.starts_with("no such command in SoundCraft: track.inspct; perhaps track.inspect"), "{}", refused.1);
    let refused = app.call("zzz", json!({})).await.unwrap_err();
    assert_eq!(refused, ("bad-params", "no such command in SoundCraft: zzz (engine.commands lists them)".to_string()));
    assert_eq!(app.executed(), Vec::<Value>::new());
    assert_eq!(app.asked("engine.commands"), 1);
    // (and without the command's id at all)
    assert_eq!(app.read(json!({"kind": "call"})).await.unwrap_err().0, "bad-params");
}

#[tokio::test]
async fn an_id_where_a_ref_goes_or_a_ref_where_none_goes_is_refused_before_the_app_is_asked() {
    let mut app = App::new().await;
    let refused = app.call("track.inspect", json!({"track": 15})).await.unwrap_err();
    assert_eq!(refused.0, "bad-params");
    assert!(refused.1.starts_with("track: pass a ref from read (s:track:…), not SoundCraft's id 15"), "{}", refused.1);
    let refused = app.call("edit.strip_silence_preview", json!({"tracks": ["s:track:3", 6]})).await.unwrap_err();
    assert!(refused.1.contains("pass a ref from read"), "{}", refused.1);
    for (command, params) in [
        ("track.inspect", json!({"track": "Pad"})),
        ("clip.inspect", json!({"clip": "s:track:15"})),
        ("edit.strip_silence_preview", json!({"tracks": "s:track:3"})),
        ("session.format_time", json!({"at": "s:marker:29"})),
    ] {
        assert_eq!(app.call(command, params.clone()).await.unwrap_err().0, "bad-params", "{command} {params}");
    }
    let without_epoch = app.read(json!({"kind": "call", "command": "track.inspect", "params": {"track": "s:track:15"}})).await;
    assert_eq!(without_epoch.unwrap_err().0, "bad-params");
    assert_eq!(app.read(json!({"kind": "call", "command": "engine.history", "params": [1]})).await.unwrap_err().0, "bad-params");
    assert_eq!(app.fake.requests(), Vec::<Value>::new());
}

#[tokio::test]
async fn a_ref_from_another_epoch_or_an_earlier_life_is_stale() {
    let mut app = App::new().await;
    app.answers("track.inspect", ok(track(15)));
    let before = Reads::new(Control::new(app.fake.port(), None)).epoch().to_string();
    let params = json!({"kind": "call", "command": "track.inspect", "params": {"track": "s:track:15"}, "epoch": before});
    assert_eq!(app.read(params).await.unwrap_err().0, "stale", "another table's");
    let stale = app.call("track.inspect", json!({"track": "s:track:15.2"})).await.unwrap_err();
    assert_eq!(stale, ("stale", "s:track:15.2 isn't from this session: read again".to_string()));
    assert_eq!(app.call("track.inspect", json!({"track": "s:track:999"})).await.unwrap_err().0, "stale");
    assert_eq!(app.executed(), Vec::<Value>::new());
}

#[tokio::test]
async fn an_answer_past_the_bytes_a_read_may_take_is_cut_and_says_where() {
    let mut app = App::new().await;
    let note = |at: i64| json!({"start": at * 120, "length": 120, "pitch": 60, "velocity": 100, "release_velocity": 64, "channel": 0});
    let notes: Vec<Value> = (0..20_000).map(note).collect();
    app.answers("midi.notes", ok(json!({"notes": notes})));
    let answer = app.call("midi.notes", json!({"clip": "s:clip:25"})).await.unwrap();
    assert_eq!(app.executed(), [json!({"command": "midi.notes", "params": {"clip": 25}})]);
    let shown = answer["result"]["notes"].as_array().unwrap().len();
    assert_eq!(answer["truncated"], json!({"/result/notes": {"total": 20_000, "shown": shown}}));
    assert!(shown > 1000 && shown < 20_000, "{shown}");
    assert!(serde_json::to_string(&answer["result"]).unwrap().len() <= ANSWER_BYTES);
    assert_eq!(answer["result"]["notes"][shown - 1], note(shown as i64 - 1), "the first notes, as they were");

    // A text alone too long is cut at its start.
    app.answers("engine.parity", ok(json!({"report": "x".repeat(300 * 1024)})));
    let parity = app.call("engine.parity", json!({})).await.unwrap();
    let shown = parity["result"]["report"].as_str().unwrap().len();
    assert_eq!(parity["truncated"], json!({"/result/report": {"totalBytes": 300 * 1024, "shownBytes": shown}}));
    assert!(serde_json::to_string(&parity["result"]).unwrap().len() <= ANSWER_BYTES);
}

#[tokio::test]
async fn a_busy_app_or_a_passed_deadline_reads_nothing() {
    let mut app = App::new().await;
    app.answers("engine.history", refused("control queue full"));
    let busy = app.call("engine.history", json!({})).await.unwrap_err();
    assert_eq!(busy.0, "busy", "{busy:?}");
    assert!(busy.1.ends_with("nothing was read"), "{}", busy.1);
    let asked = app.fake.requests().len();
    let late = app.reads.read(&json!({"kind": "call", "command": "engine.history"}), Instant::now()).await.unwrap_err();
    assert_eq!(late.0, "deadline");
    assert_eq!(app.fake.requests().len(), asked, "nothing asked");
    // (a command the app refuses says so, in its words)
    app.answers("clip.inspect", refused("bad parameters for `clip.inspect`: no such clip"));
    let failed = app.call("clip.inspect", json!({"clip": "s:clip:25"})).await.unwrap_err();
    assert_eq!(failed, ("failed", "clip.inspect: bad parameters for `clip.inspect`: no such clip".to_string()));
}
