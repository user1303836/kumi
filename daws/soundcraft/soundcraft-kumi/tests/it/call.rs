//! `read {kind: "call"}` against a fake SoundCraft that shows the session SoundCraft 0.17.0 showed (tests/vectors) and
//! lists its commands: a read-only command runs with its refs sent as ids and its answer's ids back as refs; anything
//! else, a param the command doesn't take, and what the session shows the app would refuse, are refused before they're
//! sent, naming the commands that run; a SoundCraft the list wasn't checked against runs none; a ref from another epoch
//! or life is stale; and an answer past the bytes a read may take is cut, saying where.
use super::fake::{ok, refused, Answer, Fake};
use super::read::{misplaced, recorded};
use super::vectors::vectors;
use serde_json::{json, Value};
use soundcraft_kumi::control::Control;
use soundcraft_kumi::read::call::READ_ONLY;
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
        App::of(Some("0.17.0")).await
    }

    /// The app, saying it's `version` (or saying none).
    async fn of(version: Option<&str>) -> App {
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
        let reads = Reads::new(Control::new(fake.port(), None), version.map(str::to_string));
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

/// How each refusal of a command off the list ends: the commands that run.
fn reads() -> String {
    format!(". The commands Kumi reads: {}", READ_ONLY.iter().map(|command| command.id).collect::<Vec<_>>().join(", "))
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
        ("window.config_new", "changes the session or the app"),
        ("window.mix", "only opens or closes a window"),
        ("ui.new_tracks_dialog", "only opens a dialog"),
    ] {
        let refused = app.call(command, json!({})).await.unwrap_err();
        assert_eq!(refused, ("bad-params", format!("Kumi can't change SoundCraft yet: {command} {does}{}", reads())));
    }
    assert_eq!(app.executed(), Vec::<Value>::new());
    assert_eq!(app.asked("engine.commands"), 1, "the app's list, asked once");
}

#[tokio::test]
async fn a_command_that_looks_like_a_read_is_refused_as_one_kumi_doesnt_read_and_why() {
    let mut app = App::new().await;
    for (command, why) in [
        ("transport.status", "each call takes a place in SoundCraft's command journal"),
        ("view.clip_gain_info", "it's a View > Clip toggle"),
        ("setup.io", "it sets what its params name"),
        ("window.undo_history", "it opens or closes the window that shows what engine.history"),
        ("edit.strip_silence_preview", "it renders the audio of every clip in its range on SoundCraft's window thread, unbounded"),
    ] {
        let (code, words) = app.call(command, json!({})).await.unwrap_err();
        assert_eq!(code, "bad-params");
        assert!(words.starts_with(&format!("{command} isn't one of the commands Kumi reads: {why}")), "{words}");
        assert!(words.ends_with(&reads()), "{words}");
    }
    assert_eq!(app.fake.requests(), Vec::<Value>::new(), "nothing asked, not even the app's list");
}

#[tokio::test]
async fn a_method_of_the_control_channel_is_refused_without_asking_the_app() {
    let mut app = App::new().await;
    for method in ["ui.click", "ui.screenshot", "engine.execute", "app.quit", "document.inspect"] {
        let (code, words) = app.call(method, json!({})).await.unwrap_err();
        assert_eq!(code, "bad-params");
        assert!(words.starts_with(&format!("{method} is a method of SoundCraft's control channel, which call doesn't reach")), "{words}");
        assert!(words.ends_with(&reads()), "{words}");
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
        assert!(words.ends_with(&reads()), "{words}");
    }
    assert_eq!(app.fake.requests(), Vec::<Value>::new());
}

#[tokio::test]
async fn an_unknown_command_is_refused_with_the_commands_that_run_near_it() {
    let mut app = App::new().await;
    let refused = app.call("track.inspct", json!({})).await.unwrap_err();
    assert_eq!(refused.0, "bad-params");
    assert!(refused.1.starts_with("no such command in SoundCraft: track.inspct; perhaps track.inspect"), "{}", refused.1);
    let refused = app.call("zzz", json!({})).await.unwrap_err();
    assert_eq!(refused, ("bad-params", format!("no such command in SoundCraft: zzz{}", reads())));
    // (only the commands that run are suggested: transport.status is a look-alike)
    let refused = app.call("transport.stat", json!({})).await.unwrap_err();
    assert_eq!(refused, ("bad-params", format!("no such command in SoundCraft: transport.stat{}", reads())));
    assert_eq!(app.executed(), Vec::<Value>::new());
    assert_eq!(app.asked("engine.commands"), 1);
    // (and without the command's id at all)
    let refused = app.read(json!({"kind": "call"})).await.unwrap_err();
    assert_eq!(refused.0, "bad-params");
    assert!(refused.1.starts_with("command: the id of one of the commands Kumi reads: session.inspect, track.inspect"), "{}", refused.1);
}

#[tokio::test]
async fn an_id_where_a_ref_goes_a_ref_where_none_goes_or_a_param_it_doesnt_take_is_refused_before_the_app_is_asked() {
    let mut app = App::new().await;
    let refused = app.call("track.inspect", json!({"track": 15})).await.unwrap_err();
    assert_eq!(refused.0, "bad-params");
    assert!(refused.1.starts_with("track: pass a ref from read (s:track:…), not SoundCraft's id 15"), "{}", refused.1);
    for (command, params) in [
        ("track.inspect", json!({"track": "Pad"})),
        ("clip.inspect", json!({"clip": "s:track:15"})),
        ("session.format_time", json!({"at": "s:marker:29"})),
    ] {
        assert_eq!(app.call(command, params.clone()).await.unwrap_err().0, "bad-params", "{command} {params}");
    }
    for (command, params, words) in [
        ("track.inspect", json!({"track": "s:track:15", "detail": "full"}), "detail: track.inspect takes only track"),
        ("engine.history", json!({"set": true}), "set: engine.history takes no params"),
        ("view.flags", json!({"prefix": "view.", "value": true}), "value: view.flags takes only prefix"),
        ("session.format_time", json!({"at": 0, "set": 1}), "set: session.format_time takes only at, format"),
        ("track.inspect", json!({}), "track: track.inspect reads the track it names, as a ref from read (s:track:…)"),
    ] {
        assert_eq!(app.call(command, params).await.unwrap_err(), ("bad-params", words.to_string()));
    }
    let without_epoch = app.read(json!({"kind": "call", "command": "track.inspect", "params": {"track": "s:track:15"}})).await;
    assert_eq!(without_epoch.unwrap_err().0, "bad-params");
    assert_eq!(app.read(json!({"kind": "call", "command": "engine.history", "params": [1]})).await.unwrap_err().0, "bad-params");
    assert_eq!(app.fake.requests(), Vec::<Value>::new());
}

#[tokio::test]
async fn what_the_session_shows_the_app_would_refuse_is_refused_in_kumis_words_and_never_sent() {
    let mut app = App::new().await;
    // (the app would put its refusal in its status line)
    let audio = app.call("midi.notes", json!({"clip": "s:clip:4"})).await.unwrap_err();
    assert_eq!(audio, ("bad-params", "clip: s:clip:4 isn't a MIDI clip, so it has no notes".to_string()));
    let at = app.call("session.format_time", json!({"at": "bar 5"})).await.unwrap_err();
    assert_eq!(at, ("bad-params", "at: a position in samples, a number, not \"bar 5\"".to_string()));
    assert_eq!(app.executed(), Vec::<Value>::new());
    app.answers("session.format_time", ok(json!({"text": "0:01.000"})));
    assert_eq!(app.call("session.format_time", json!({"at": 48_000, "format": "min_secs"})).await.unwrap()["result"]["text"], "0:01.000");
}

#[tokio::test]
async fn only_a_soundcraft_the_list_was_checked_against_runs_a_call() {
    for (version, this) in [
        (Some("0.18.0"), "this is SoundCraft 0.18.0"),
        (Some("0.170.1"), "this is SoundCraft 0.170.1"),
        (None, "this SoundCraft's version isn't known"),
    ] {
        let mut app = App::of(version).await;
        let refused = app.call("engine.history", json!({})).await.unwrap_err();
        let words = format!("call runs only on SoundCraft 0.17.x, the version the commands Kumi reads were checked against, and {this}");
        assert_eq!(refused, ("failed", words));
        assert_eq!(app.fake.requests(), Vec::<Value>::new());
    }
    let mut app = App::of(Some("0.17.3")).await;
    app.answers("engine.history", ok(json!({"undo": [], "can_redo": false})));
    assert!(app.call("engine.history", json!({})).await.is_ok(), "a later 0.17");
}

#[tokio::test]
async fn a_ref_from_another_epoch_or_an_earlier_life_is_stale() {
    let mut app = App::new().await;
    app.answers("track.inspect", ok(track(15)));
    let before = Reads::new(Control::new(app.fake.port(), None), Some("0.17.0".into())).epoch().to_string();
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
async fn an_answer_with_no_one_list_to_cut_empties_the_biggest_and_goes_on() {
    let mut app = App::new().await;
    // (three lists of 160 KiB: none alone can bring the answer under 224 KiB)
    let list = || Value::from(vec![Value::from("x".repeat(1022)); 160]);
    app.answers("engine.history", ok(json!({"a": list(), "b": list(), "c": list()})));
    let answer = app.call("engine.history", json!({})).await.unwrap();
    assert!(serde_json::to_string(&answer["result"]).unwrap().len() <= ANSWER_BYTES);
    let truncated = answer["truncated"].as_object().unwrap();
    assert_eq!(truncated.len(), 2, "the biggest emptied, then one cut: {truncated:?}");
    for key in ["a", "b", "c"] {
        let shown = answer["result"][key].as_array().unwrap().len();
        match truncated.get(&format!("/result/{key}")) {
            Some(told) => assert_eq!(told, &json!({"total": 160, "shown": shown})),
            None => assert_eq!(shown, 160, "{key}: whole when not named"),
        }
    }
    assert_eq!(truncated["/result/a"], json!({"total": 160, "shown": 0}), "emptied");
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
