//! The bridge's server for Kumi's host, against a fake SoundCraft. The host connects with its own client, through the
//! discovery file. `ping`, `status` and `focus` answer from the last looks, a dialog or an app that doesn't answer makes
//! the bridge busy, and a new session is told as a `project` event. The bridge ends when it's stopped or when SoundCraft
//! quits, and takes its discovery file with it. Run as the program the host starts, it serves one copy of SoundCraft per
//! bridge and ends with its stdin.
use super::fake::{ok, refused, Answer, Fake};
use daw_bridge::auth::Secret;
use daw_bridge::daw::Daw;
use daw_bridge::files::{ensure_secret, Launched};
use daw_bridge::handshake::{ClientHandshake, ClientStep};
use daw_bridge::loopback::BIND_ADDRESS;
use daw_bridge::paths::BridgePaths;
use daw_mcp_server::client::{BridgeClient, RequestError};
use serde_json::{json, Value};
use soundcraft_kumi::server::{self, Config, End, Schedule};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt};

const WAIT: Duration = Duration::from_secs(5);
const TOKEN: &str = "t0ken";

fn ms(n: u64) -> Duration {
    Duration::from_millis(n)
}

/// A listener on any free loopback port, where the bridge's own [`server::bind`] takes one in SoundCraft's range: tests
/// running at once would use the range up.
fn any_port() -> std::net::TcpListener {
    std::net::TcpListener::bind((BIND_ADDRESS, 0)).unwrap()
}

/// A child process, killed and waited for when the test ends, however it ends.
struct Reaped(Child);

impl Drop for Reaped {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

impl std::ops::Deref for Reaped {
    type Target = Child;
    fn deref(&self) -> &Child {
        &self.0
    }
}

impl std::ops::DerefMut for Reaped {
    fn deref_mut(&mut self) -> &mut Child {
        &mut self.0
    }
}

/// What the fake app shows: its session's name, tracks, selection and last undo, an open dialog, whether it answers late,
/// whether it answers nothing in time, and whether a full inspection (a read's) takes it two seconds; its windows
/// (`ui.inspect`'s `ui`), its playhead and whether it plays or records, and its tempo; and its undo history as
/// `engine.history` answers it.
struct Shown {
    name: String,
    tracks: Value,
    selection: Value,
    undo: Value,
    dialog: Option<String>,
    late: bool,
    hung: bool,
    slow_reads: bool,
    ui: Value,
    position: u64,
    playing: bool,
    recording: bool,
    bpm: f64,
    /// What the session's look says of the transport while it hasn't caught up with the app (`playing`, `recording`).
    held: Option<(bool, bool)>,
    history: Value,
}

/// A fake SoundCraft, and a Kumi folder for its bridge.
struct Lab {
    home: tempfile::TempDir,
    fake: Fake,
    shown: Arc<Mutex<Shown>>,
    secret: Secret,
}

impl Lab {
    async fn new() -> Self {
        let shown = Arc::new(Mutex::new(Shown {
            name: "Midnight Groove".into(),
            tracks: json!([]),
            selection: json!({"tracks": [], "clips": [], "start": 0, "end": 0}),
            undo: Value::Null,
            dialog: None,
            late: false,
            hung: false,
            slow_reads: false,
            ui: json!({"window": "Edit", "show_midi_editor": false, "plugin_windows": []}),
            position: 0,
            playing: false,
            recording: false,
            bpm: 120.0,
            held: None,
            history: json!({"undo": [], "can_redo": false}),
        }));
        let answering = shown.clone();
        let fake = Fake::start(16, move |request| {
            let shown = answering.lock().unwrap();
            match (request["method"].as_str(), request["params"]["command"].as_str()) {
                _ if shown.hung => Answer::After(ms(400), ok(json!({}))),
                (Some("session.inspect"), _) => {
                    let session = ok(json!({
                        "name": shown.name, "path": null, "tracks": shown.tracks, "undo": shown.undo, "redo": null,
                        "sample_rate": 48000, "tempo": [{"bpm": shown.bpm, "tick": 0}], "meter": [{"denominator": 4, "numerator": 4, "tick": 0}],
                        "selection": shown.selection,
                        "transport": {"playing": shown.held.map_or(shown.playing, |held| held.0), "position": shown.position, "recording": shown.held.map_or(shown.recording, |held| held.1)},
                    }));
                    match request["params"]["detail"] == "full" && shown.slow_reads {
                        true => Answer::After(ms(2000), session),
                        false => Answer::Now(session),
                    }
                }
                (Some("ui.inspect"), _) if shown.late => Answer::After(ms(400), ok(json!({"dialog": null}))),
                (Some("ui.inspect"), _) => {
                    Answer::Now(ok(json!({"dialog": shown.dialog, "playing": shown.playing, "position": shown.position, "ui": shown.ui})))
                }
                (Some("engine.execute"), Some("engine.history")) => Answer::Now(ok(shown.history.clone())),
                (Some("engine.execute"), Some("engine.plugins")) => {
                    Answer::Now(ok(json!([{"id": "eq_1band", "name": "EQ 1-Band", "instrument": false, "params": []}])))
                }
                _ => Answer::Now(refused("unknown method")),
            }
        })
        .await;
        let home = tempfile::tempdir().unwrap();
        let secret = ensure_secret(&home.path().join("bridges/soundcraft/bridge.secret")).unwrap();
        Self { home, fake, shown, secret }
    }

    fn paths(&self) -> BridgePaths {
        BridgePaths::new(self.home.path(), Daw::SoundCraft)
    }

    fn launched(&self, pid: u32) -> Launched {
        Launched {
            pid,
            started_ms: 1,
            port: self.fake.port(),
            token: TOKEN.into(),
            app: "/lab/SoundCraft.app".into(),
            version: Some("0.17.0".into()),
        }
    }

    /// The record a start leaves, owner-only as a start writes it.
    fn record(&self, launched: &Launched) {
        daw_bridge::owner::write_owner_only(&self.paths().launch_record(), &serde_json::to_vec(launched).unwrap()).unwrap();
    }

    /// Looks and heartbeats each 100 ms, for a copy whose process is `pid`.
    fn config(&self, pid: u32) -> Config {
        Config {
            look: ms(100),
            heartbeat_ms: 100,
            ..Config::new(self.paths().secret(), self.paths().discovery(), self.launched(pid), "/lab/profile".into())
        }
    }

    fn discovery_files(&self) -> Vec<PathBuf> {
        let Ok(entries) = std::fs::read_dir(self.paths().discovery()) else { return vec![] };
        entries.flatten().map(|entry| entry.path()).filter(|path| path.extension().is_some_and(|ext| ext == "json")).collect()
    }

    /// The discovery file, once there's one of which `until` holds.
    async fn discovery_until(&self, until: impl Fn(&Value) -> bool) -> Value {
        let deadline = Instant::now() + WAIT;
        loop {
            if let Some(file) = self.discovery_files().first() {
                if let Ok(discovery) = serde_json::from_slice::<Value>(&std::fs::read(file).unwrap_or_default()) {
                    if until(&discovery) {
                        return discovery;
                    }
                }
            }
            assert!(Instant::now() < deadline, "no such discovery file");
            tokio::time::sleep(ms(20)).await;
        }
    }

    /// The discovery file, once there is one.
    async fn discovery(&self) -> Value {
        self.discovery_until(|_| true).await
    }

    async fn port(&self) -> u16 {
        self.discovery().await["port"].as_u64().unwrap() as u16
    }

    async fn connect(&self) -> BridgeClient {
        let address = SocketAddr::from(([127, 0, 0, 1], self.port().await));
        BridgeClient::connect(address, self.secret.clone(), "daw-mcp-server/test").await.unwrap()
    }

    /// A fake SoundCraft in a process of its own (launch's), under SoundCraft's program name, which prints its port and
    /// answers only with the token: a look for the copy checks both.
    fn soundcraft(&self) -> (Reaped, u16) {
        use std::io::BufRead;
        let program = self.home.path().join(if cfg!(windows) { "soundcraft.exe" } else { "soundcraft" });
        let this = std::env::current_exe().unwrap();
        // (a copy on macOS, which names a hard-linked program's process by whichever of its names it looked up last)
        if !program.exists() && (cfg!(target_os = "macos") || std::fs::hard_link(&this, &program).is_err()) {
            std::fs::copy(&this, &program).unwrap();
        }
        let mut app = Reaped(
            Command::new(program)
                .args(["--exact", "launch::a_fake_soundcraft", "--ignored", "--nocapture", "--test-threads=1"])
                .env("SOUNDCRAFT_CONTROL_TOKEN", TOKEN)
                .stdout(Stdio::piped())
                .spawn()
                .unwrap(),
        );
        let printed = std::io::BufReader::new(app.stdout.take().unwrap());
        let port =
            printed.lines().map_while(Result::ok).find_map(|line| line.strip_prefix("SOUNDCRAFT_CONTROL_PORT=")?.parse().ok()).unwrap();
        (app, port)
    }
}

/// Asks for `method` until `until` holds of its answer (the look loop takes a few looks to see a change). A refusal is
/// no answer yet.
async fn asked_until(client: &BridgeClient, method: &str, until: impl Fn(&Value) -> bool) -> Value {
    let deadline = Instant::now() + WAIT;
    loop {
        let answer = client.request(method, json!({}), WAIT).await.unwrap_or(Value::Null);
        if until(&answer) || Instant::now() > deadline {
            return answer;
        }
        tokio::time::sleep(ms(20)).await;
    }
}

/// Waits for the look loop to ask for `what` (a method, or an engine command) twice more, so once after anything shown
/// before now.
async fn looked(lab: &Lab, what: &str) {
    let looks = || lab.fake.requests().iter().filter(|request| request["method"] == what || request["params"]["command"] == what).count();
    let (before, deadline) = (looks(), Instant::now() + WAIT);
    while looks() < before + 2 && Instant::now() < deadline {
        tokio::time::sleep(ms(20)).await;
    }
}

#[tokio::test]
async fn the_host_connects_through_the_discovery_file_and_status_answers_from_the_last_look() {
    let lab = Lab::new().await;
    let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
    let serving = tokio::spawn(server::serve(any_port(), lab.config(std::process::id()), async move {
        let _ = stopped.await;
    }));
    let discovery = lab.discovery_until(|discovery| discovery["project"]["name"] == "Midnight Groove").await;
    assert_eq!(
        (&discovery["daw"], &discovery["pid"], &discovery["resource"], &discovery["dawVersion"]),
        (&json!("soundcraft"), &json!(std::process::id()), &json!("/lab/profile"), &json!("0.17.0"))
    );
    let client = lab.connect().await;
    let welcome = client.welcome();
    assert_eq!((welcome.identity.daw.as_str(), welcome.identity.pid), ("soundcraft", std::process::id()));
    assert_eq!(welcome.identity.project.as_ref().and_then(|project| project.name.as_deref()), Some("Midnight Groove"));
    let offered = (welcome.methods.clone(), welcome.events.clone());
    let methods = ["ping", "status", "focus", "read", "snapshot", "focus_body"].map(String::from).to_vec();
    let events = ["project", "transport", "tracks", "selection"].map(String::from).to_vec();
    assert_eq!(offered, (methods, events));
    assert_eq!(client.request("ping", json!({}), WAIT).await.unwrap(), json!({}));
    let status = client.request("status", json!({}), WAIT).await.unwrap();
    assert_eq!((&status["busy"], &status["project"]["name"]), (&json!(false), &json!("Midnight Groove")));
    stop.send(()).unwrap();
    assert_eq!(serving.await.unwrap(), Ok(End::Stopped));
    assert!(lab.discovery_files().is_empty(), "the discovery file went with the bridge");
    let until = Instant::now() + WAIT;
    while !client.is_closed() && Instant::now() < until {
        tokio::time::sleep(ms(20)).await;
    }
    assert!(client.is_closed(), "the bridge said bye");
}

/// A connection that has passed the handshake, its bytes from here the test's own.
async fn authenticated(port: u16, secret: &Secret) -> tokio::io::BufStream<tokio::net::TcpStream> {
    let mut stream = tokio::io::BufStream::new(tokio::net::TcpStream::connect(("127.0.0.1", port)).await.unwrap());
    let (mut handshake, hello) = ClientHandshake::start(secret.clone(), "test");
    stream.write_all(&hello).await.unwrap();
    stream.flush().await.unwrap();
    loop {
        let mut line = String::new();
        tokio::time::timeout(WAIT, stream.read_line(&mut line)).await.unwrap().unwrap();
        match handshake.on_line(line.trim_end()).unwrap() {
            ClientStep::Send(bytes) => {
                stream.write_all(&bytes).await.unwrap();
                stream.flush().await.unwrap();
            }
            ClientStep::Welcome(_) => return stream,
        }
    }
}

#[tokio::test]
async fn a_connection_closed_for_a_bad_line_after_its_handshake_leaves_room_for_the_next() {
    let lab = Lab::new().await;
    let _serving = tokio::spawn(server::serve(any_port(), lab.config(std::process::id()), std::future::pending()));
    let port = lab.port().await;
    for _ in 0..3 {
        let mut stream = authenticated(port, &lab.secret).await;
        stream.write_all(b"not json\n").await.unwrap();
        stream.flush().await.unwrap();
        let mut rest = Vec::new();
        tokio::time::timeout(WAIT, stream.read_to_end(&mut rest)).await.expect("closed for its bad line").ok();
    }
    let client = lab.connect().await;
    assert_eq!(client.request("ping", json!({}), WAIT).await.unwrap(), json!({}), "welcomed, and answered");
}

#[tokio::test]
async fn the_host_reads_through_the_bridge_and_a_read_while_the_app_is_busy_is_refused_at_once() {
    let lab = Lab::new().await;
    let _serving = tokio::spawn(server::serve(any_port(), lab.config(std::process::id()), std::future::pending()));
    let client = lab.connect().await;
    assert!(client.welcome().methods.iter().any(|method| method == "read"), "{:?}", client.welcome().methods);
    // (the lab's SoundCraft shows a session with no tracks)
    let tracks = client.request("read", json!({"kind": "track"}), WAIT).await.unwrap();
    assert_eq!((&tracks["items"], &tracks["total"]), (&json!([]), &json!(0)), "{tracks}");
    let snapshot = client.request("snapshot", json!({}), WAIT).await.unwrap();
    assert_eq!(
        (&snapshot["epoch"], &snapshot["project"]["name"], &snapshot["tracks"]),
        (&tracks["epoch"], &json!("Midnight Groove"), &json!([]))
    );
    let mixer = client.request("focus_body", json!({"kind": "mixer"}), WAIT).await.unwrap();
    assert_eq!((&mixer["kind"], &mixer["epoch"], &mixer["strips"]), (&json!("mixer"), &tracks["epoch"], &json!([])));
    lab.shown.lock().unwrap().dialog = Some("Save Changes".into());
    asked_until(&client, "status", |status| status["busy"] == "modal").await;
    let began = Instant::now();
    let refused = client.request("read", json!({"kind": "track"}), WAIT).await.unwrap_err();
    let RequestError::Bridge { code, message } = refused else { panic!("{refused:?}") };
    assert_eq!((code.as_str(), message.as_str()), ("busy", "a dialog is open in SoundCraft (Save Changes); nothing was read"));
    assert!(began.elapsed() < ms(500), "at once: {:?}", began.elapsed());
    let body = client.request("focus_body", json!({"kind": "mixer"}), WAIT).await.unwrap_err();
    assert!(matches!(&body, RequestError::Bridge { code, .. } if code == "busy"), "{body:?}");
}

#[tokio::test]
async fn the_host_calls_a_read_only_command_through_the_bridge_and_a_call_while_the_app_is_busy_is_refused_at_once() {
    let lab = Lab::new().await;
    let _serving = tokio::spawn(server::serve(any_port(), lab.config(std::process::id()), std::future::pending()));
    let client = lab.connect().await;
    let history = client.request("read", json!({"kind": "call", "command": "engine.history"}), WAIT).await.unwrap();
    assert_eq!((&history["command"], &history["result"]), (&json!("engine.history"), &json!({"undo": [], "can_redo": false})));
    assert!(history["epoch"].is_string(), "{history}");
    lab.shown.lock().unwrap().dialog = Some("Save Changes".into());
    asked_until(&client, "status", |status| status["busy"] == "modal").await;
    let asked = lab.fake.requests().len();
    let began = Instant::now();
    let refused = client.request("read", json!({"kind": "call", "command": "engine.history"}), WAIT).await.unwrap_err();
    let RequestError::Bridge { code, message } = refused else { panic!("{refused:?}") };
    assert_eq!((code.as_str(), message.as_str()), ("busy", "a dialog is open in SoundCraft (Save Changes); nothing was read"));
    assert!(began.elapsed() < ms(500), "at once: {:?}", began.elapsed());
    let sent = lab.fake.requests()[asked..].iter().filter(|request| request["method"] == "engine.execute").count();
    assert_eq!(sent, 0, "nothing sent to the app");
}

/// Kick, with an EQ in its slot a, and Keys, an instrument track.
fn kick_and_keys() -> Value {
    json!([
        {"id": 3, "kind": "audio", "format": "Mono", "name": "Kick", "instrument": null, "clips": [],
         "inserts": [{"slot": 0, "plugin": "eq_1band", "bypass": false, "params": {}}]},
        {"id": 5, "kind": "instrument", "format": "Stereo", "name": "Keys", "instrument": "subtractive_synth", "inserts": [], "clips": []},
    ])
}

#[tokio::test]
async fn focus_answers_with_the_window_the_selection_and_the_playhead() {
    let lab = Lab::new().await;
    {
        let mut shown = lab.shown.lock().unwrap();
        shown.tracks = kick_and_keys();
        shown.selection = json!({"tracks": [3], "clips": [], "start": 0, "end": 0});
        // (two seconds in at 120 bpm: four beats, the first of bar 2)
        shown.position = 96000;
    }
    let _serving = tokio::spawn(server::serve(any_port(), lab.config(std::process::id()), std::future::pending()));
    let client = lab.connect().await;
    let focus = asked_until(&client, "focus", |focus| focus["view"].is_string()).await;
    let status = client.request("status", json!({}), WAIT).await.unwrap();
    assert_eq!(focus["epoch"], status["project"]["epoch"]);
    assert_eq!(
        (&focus["view"], &focus["selection"]["trackRef"], &focus["selection"]["track"]),
        (&json!("edit"), &json!("s:track:3"), &json!({"name": "Kick", "type": "audio"}))
    );
    let transport = &focus["transport"];
    assert_eq!(
        (&transport["state"], &transport["seconds"], &transport["beat"], &transport["bar"], &transport["beatInBar"], &transport["tempo"]),
        (&json!("stopped"), &json!(2.0), &json!(4.0), &json!(2), &json!(1.0), &json!(120.0))
    );
    assert_eq!(focus["changed"], 0);
    assert!(focus.get("busy").is_none(), "free, so no busy at all (Kumi takes any busy for busy): {focus}");
    // The windows the producer opens.
    let shows = |ui: Value| lab.shown.lock().unwrap().ui = ui;
    shows(json!({"window": "Mix", "show_midi_editor": false, "plugin_windows": []}));
    assert_eq!(asked_until(&client, "focus", |focus| focus["view"] == "mixer").await["view"], "mixer");
    // The MIDI Editor is a panel of the Edit window: left open while Mix shows, it isn't what the producer sees.
    shows(json!({"window": "Mix", "show_midi_editor": true, "plugin_windows": []}));
    looked(&lab, "ui.inspect").await;
    assert_eq!(client.request("focus", json!({}), WAIT).await.unwrap()["view"], "mixer");
    shows(json!({"window": "Edit", "show_midi_editor": true, "plugin_windows": []}));
    assert_eq!(asked_until(&client, "focus", |focus| focus["view"] == "midi").await["view"], "midi");
    // With Mix in a window of its own, the main window draws Edit, and the MIDI Editor in it.
    shows(json!({"window": "Mix", "mix_separate": true, "show_midi_editor": true, "plugin_windows": []}));
    looked(&lab, "ui.inspect").await;
    assert_eq!(client.request("focus", json!({}), WAIT).await.unwrap()["view"], "midi");
    shows(json!({"window": "Edit", "show_midi_editor": false, "plugin_windows": [[3, 0]]}));
    let fx = asked_until(&client, "focus", |focus| focus["selection"]["fxRef"].is_string()).await;
    assert_eq!(
        (&fx["view"], &fx["selection"]["fxRef"], &fx["selection"]["fxName"]),
        (&json!("fx"), &json!("s:insert:3:a"), &json!("EQ 1-Band"))
    );
    // The last opened is the one shown: Keys' instrument, whose slot is the largest number there is (and whose name the
    // app's built-ins don't give, so it stays its id).
    shows(json!({"window": "Edit", "show_midi_editor": false, "plugin_windows": [[3, 0], [5, u64::MAX]]}));
    let instrument = asked_until(&client, "focus", |focus| focus["selection"]["fxRef"] == "s:insert:5:instrument").await;
    // The window is on Keys while Kick is selected: its track is named, as `read` names it.
    assert_eq!(
        (
            &instrument["selection"]["fxRef"],
            &instrument["selection"]["fxName"],
            &instrument["selection"]["fxTrackRef"],
            &instrument["selection"]["fxTrackName"],
            &instrument["selection"]["trackRef"]
        ),
        (&json!("s:insert:5:instrument"), &json!("subtractive_synth"), &json!("s:track:5"), &json!("Keys"), &json!("s:track:3"))
    );
}

#[tokio::test]
async fn a_bodys_ref_from_before_another_session_opened_is_stale_though_the_body_sees_the_new_session_first() {
    let lab = Lab::new().await;
    lab.shown.lock().unwrap().tracks = kick_and_keys();
    // Looks four seconds apart: a body's own look at the session sees the switch before the look loop does.
    let config = Config { look: ms(4000), ..lab.config(std::process::id()) };
    let _serving = tokio::spawn(server::serve(any_port(), config, std::future::pending()));
    let client = lab.connect().await;
    let focus = asked_until(&client, "focus", |focus| focus["view"].is_string()).await;
    // FOCUS sends a body's refs with no epoch: they're read in the last look's.
    let kick = client.request("focus_body", json!({"kind": "fx", "ref": "s:track:3"}), WAIT).await.unwrap();
    assert_eq!((&kick["epoch"], &kick["name"]), (&focus["epoch"], &json!("Kick")));
    // Another song, with Pad on Kick's id; the look's summary grows old.
    {
        let mut shown = lab.shown.lock().unwrap();
        shown.name = "Next Song".into();
        shown.tracks =
            json!([{"id": 3, "kind": "audio", "format": "Stereo", "name": "Pad", "instrument": null, "inserts": [], "clips": []}]);
    }
    tokio::time::sleep(ms(1100)).await;
    for params in [json!({"kind": "fx", "ref": "s:track:3"}), json!({"kind": "mixer", "around": "s:track:3"})] {
        let refused = client.request("focus_body", params.clone(), WAIT).await.unwrap_err();
        assert!(matches!(&refused, RequestError::Bridge { code, .. } if code == "stale"), "{params}: {refused:?}");
    }
    // A body that names no ref is drawn in the new session.
    let mixer = client.request("focus_body", json!({"kind": "mixer"}), WAIT).await.unwrap();
    assert_ne!(mixer["epoch"], focus["epoch"]);
    assert_eq!(mixer["strips"][0]["name"], "Pad");
}

#[tokio::test]
async fn in_the_midi_editor_focus_names_the_clip_it_shows() {
    let lab = Lab::new().await;
    {
        let mut shown = lab.shown.lock().unwrap();
        shown.tracks = kick_and_keys();
        shown.tracks[1]["clips"] = json!([{"id": 7, "name": "Keys", "start": 0, "length": 96000, "notes": 4, "muted": false}]);
        shown.selection = json!({"tracks": [5], "clips": [], "start": 0, "end": 0});
    }
    let _serving = tokio::spawn(server::serve(any_port(), lab.config(std::process::id()), std::future::pending()));
    let client = lab.connect().await;
    let edit = asked_until(&client, "focus", |focus| focus["view"] == "edit" && focus["selection"]["trackRef"] == "s:track:5").await;
    assert!(edit["selection"].get("midiClipRef").is_none(), "only in the MIDI Editor: {edit}");
    lab.shown.lock().unwrap().ui = json!({"window": "Edit", "show_midi_editor": true, "plugin_windows": []});
    let midi = asked_until(&client, "focus", |focus| focus["view"] == "midi").await;
    assert_eq!(midi["selection"]["midiClipRef"], "s:clip:7");
    // Kick selected: no MIDI track, so the MIDI Editor shows no clip.
    lab.shown.lock().unwrap().selection = json!({"tracks": [3], "clips": [], "start": 0, "end": 0});
    let none = asked_until(&client, "focus", |focus| focus["selection"]["trackRef"] == "s:track:3").await;
    assert!(none["selection"].get("midiClipRef").is_none(), "{none}");
}

#[tokio::test]
async fn changed_moves_with_each_edit_undo_and_redo_and_nothing_else() {
    let lab = Lab::new().await;
    {
        let mut shown = lab.shown.lock().unwrap();
        shown.tracks = kick_and_keys();
        shown.history = json!({"undo": ["New Track"], "can_redo": false});
    }
    let _serving = tokio::spawn(server::serve(any_port(), lab.config(std::process::id()), std::future::pending()));
    let client = lab.connect().await;
    // A selection isn't an edit.
    lab.shown.lock().unwrap().selection = json!({"tracks": [5], "clips": [], "start": 0, "end": 0});
    asked_until(&client, "focus", |focus| focus["selection"]["trackRef"] == "s:track:5").await;
    looked(&lab, "engine.history").await;
    assert_eq!(client.request("focus", json!({}), WAIT).await.unwrap()["changed"], 0);
    let moves = [
        ("an edit", json!({"undo": ["New Track", "Rename Track"], "can_redo": false})),
        ("its undo", json!({"undo": ["New Track"], "can_redo": true})),
        ("its redo", json!({"undo": ["New Track", "Rename Track"], "can_redo": false})),
    ];
    for (n, (what, history)) in moves.into_iter().enumerate() {
        lab.shown.lock().unwrap().history = history;
        let focus = asked_until(&client, "focus", |focus| focus["changed"] != n).await;
        assert_eq!(focus["changed"], n + 1, "{what}");
    }
}

#[tokio::test]
async fn while_soundcraft_plays_its_playhead_is_looked_at_four_times_as_often_and_focus_moves_it_on() {
    let lab = Lab::new().await;
    let _serving = tokio::spawn(server::serve(any_port(), lab.config(std::process::id()), std::future::pending()));
    let client = lab.connect().await;
    asked_until(&client, "focus", |focus| focus["view"].is_string()).await;
    let looks_in_a_second = || async {
        let looks = || lab.fake.requests().iter().filter(|request| request["method"] == "ui.inspect").count();
        let before = looks();
        tokio::time::sleep(ms(1000)).await;
        looks() - before
    };
    let stopped = looks_in_a_second().await;
    lab.shown.lock().unwrap().playing = true;
    asked_until(&client, "focus", |focus| focus["transport"]["playing"] == true).await;
    let playing = looks_in_a_second().await;
    assert!(playing >= 2 * stopped, "{playing} looks in a second while it plays, {stopped} while it's stopped");
    // The look saw the playhead at the start: focus moves it on by the look's age (one 10 ms old at least, to tell).
    let focus = asked_until(&client, "focus", |focus| focus["ageMs"].as_u64() >= Some(10)).await;
    let (seconds, age) = (focus["transport"]["seconds"].as_f64().unwrap(), focus["ageMs"].as_f64().unwrap());
    assert!(age >= 10.0 && (seconds * 1000.0 - age).abs() < 5.0, "{focus}");
}

#[tokio::test]
async fn a_take_shows_as_recording_and_a_stopped_one_as_stopped() {
    let lab = Lab::new().await;
    let _serving = tokio::spawn(server::serve(any_port(), lab.config(std::process::id()), std::future::pending()));
    let client = lab.connect().await;
    asked_until(&client, "focus", |focus| focus["view"].is_string()).await;
    let records = |recording: bool| {
        let mut shown = lab.shown.lock().unwrap();
        (shown.playing, shown.recording) = (recording, recording);
    };
    records(true);
    let take = asked_until(&client, "focus", |focus| focus["transport"]["state"] == "recording").await;
    assert_eq!(take["transport"]["state"], "recording");
    records(false);
    let stopped = asked_until(&client, "focus", |focus| focus["transport"]["playing"] == false).await;
    assert_eq!(stopped["transport"]["state"], "stopped");
}

#[tokio::test]
async fn changed_also_moves_when_the_session_changes_though_its_history_reads_the_same() {
    let lab = Lab::new().await;
    let fader = |db: f64| json!([{"id": 3, "kind": "audio", "format": "Mono", "name": "Kick", "instrument": null, "inserts": [], "clips": [], "volume_db": db}]);
    {
        let mut shown = lab.shown.lock().unwrap();
        (shown.tracks, shown.history) = (fader(0.0), json!({"undo": ["Volume"], "can_redo": false}));
    }
    let _serving = tokio::spawn(server::serve(any_port(), lab.config(std::process::id()), std::future::pending()));
    let client = lab.connect().await;
    asked_until(&client, "focus", |focus| focus["view"].is_string()).await;
    // The playhead moving, or the selection, isn't a change.
    {
        let mut shown = lab.shown.lock().unwrap();
        (shown.position, shown.selection) = (48000, json!({"tracks": [3], "clips": [], "start": 0, "end": 0}));
    }
    looked(&lab, "session.inspect").await;
    assert_eq!(client.request("focus", json!({}), WAIT).await.unwrap()["changed"], 0);
    // A fader gesture SoundCraft merges into its last step: the history reads as it did, the session doesn't.
    lab.shown.lock().unwrap().tracks = fader(-6.0);
    assert_eq!(asked_until(&client, "focus", |focus| focus["changed"] != 0).await["changed"], 1);
}

#[tokio::test]
async fn a_soundcraft_that_answers_nothing_is_asked_only_ui_inspect_and_is_stalled_after_three_looks() {
    let lab = Lab::new().await;
    let _serving = tokio::spawn(server::serve(any_port(), lab.config(std::process::id()), std::future::pending()));
    let client = lab.connect().await;
    asked_until(&client, "focus", |focus| focus["view"].is_string()).await;
    let hung = Instant::now();
    lab.shown.lock().unwrap().hung = true;
    let before = lab.fake.requests().len();
    let stalled = asked_until(&client, "status", |status| status["busy"] == "stalled").await;
    let took = hung.elapsed();
    assert_eq!(stalled["busy"], "stalled");
    // From the first look it missed, nothing but `ui.inspect` was asked of it.
    let asked: Vec<Value> = lab.fake.requests()[before..].iter().map(|request| request["method"].clone()).collect();
    let first_miss = asked.iter().position(|method| method == "ui.inspect").unwrap();
    assert!(asked[first_miss..].iter().all(|method| method == "ui.inspect"), "{asked:?}");
    // (three looks of 100 ms, where asking the history and the session too would take two seconds)
    assert!(took < ms(1500), "stalled after {took:?}");
}

#[test]
fn the_session_is_looked_at_ten_times_its_cost_apart_from_half_a_look_to_five() {
    let (every, start) = (Duration::from_secs(1), Instant::now());
    for (cost, wait) in [(5, 500), (80, 800), (700, 5000)] {
        let mut schedule = Schedule::new(every, start);
        let ended = start + ms(cost);
        schedule.looked_at_session(start, ended);
        assert!(!schedule.session_due(ended + ms(wait - 1)), "a {cost} ms look: not sooner than {wait} ms after it");
        assert!(schedule.session_due(ended + ms(wait)), "a {cost} ms look: due {wait} ms after it");
    }
}

#[test]
fn something_changing_brings_the_session_look_forward_as_far_as_its_cost_allows() {
    let (every, start) = (Duration::from_secs(1), Instant::now());
    let looked = |cost: u64| {
        let mut schedule = Schedule::new(every, start);
        schedule.looked_at_session(start, start + ms(cost));
        schedule
    };
    // A 10 ms look: the next would wait half a look; an edit brings it to 100 ms after the last was asked, and one after
    // that, at once.
    let mut cheap = looked(10);
    cheap.soon(start + ms(20));
    assert!(!cheap.session_due(start + ms(99)) && cheap.session_due(start + ms(100)));
    let mut later = looked(10);
    later.soon(start + ms(300));
    assert!(later.session_due(start + ms(300)));
    // A 300 ms look: an edit can't bring the next sooner than three seconds after the last was asked.
    let mut costly = looked(300);
    costly.soon(start + ms(400));
    assert!(!costly.session_due(start + ms(2999)) && costly.session_due(start + ms(3000)));
}

#[test]
fn a_look_at_the_session_may_take_ten_times_its_last_cost_from_a_look_to_five() {
    let (every, start) = (Duration::from_secs(1), Instant::now());
    assert_eq!(Schedule::new(every, start).session_deadline(), ms(5000), "five looks before any cost is known");
    for (cost, deadline) in [(5, 1000), (300, 3000), (700, 5000)] {
        let mut schedule = Schedule::new(every, start);
        schedule.looked_at_session(start, start + ms(cost));
        assert_eq!(schedule.session_deadline(), ms(deadline), "after a {cost} ms look");
    }
}

#[tokio::test]
async fn while_a_dialog_is_open_soundcraft_is_asked_only_ui_inspect() {
    let lab = Lab::new().await;
    let _serving = tokio::spawn(server::serve(any_port(), lab.config(std::process::id()), std::future::pending()));
    let client = lab.connect().await;
    asked_until(&client, "focus", |focus| focus["view"].is_string()).await;
    lab.shown.lock().unwrap().dialog = Some("Save Changes".into());
    asked_until(&client, "status", |status| status["busy"] == "modal").await;
    let before = lab.fake.requests().len();
    looked(&lab, "ui.inspect").await;
    looked(&lab, "ui.inspect").await;
    let asked: Vec<Value> = lab.fake.requests()[before..].iter().map(|request| request["method"].clone()).collect();
    assert!(asked.len() >= 4 && asked.iter().all(|method| method == "ui.inspect"), "{asked:?}");
}

#[tokio::test]
async fn a_dialog_or_an_app_that_does_not_answer_makes_the_bridge_busy_until_it_clears() {
    let lab = Lab::new().await;
    let _serving = tokio::spawn(server::serve(any_port(), lab.config(std::process::id()), std::future::pending()));
    let client = lab.connect().await;
    lab.shown.lock().unwrap().dialog = Some("Save Changes".into());
    let modal = asked_until(&client, "status", |status| status["busy"] == "modal").await;
    assert_eq!(client.request("focus", json!({}), WAIT).await.unwrap()["busy"], "modal");
    assert_eq!((&modal["busy"], &modal["busyReason"]), (&json!("modal"), &json!("a dialog is open in SoundCraft (Save Changes)")));
    lab.shown.lock().unwrap().dialog = None;
    lab.shown.lock().unwrap().late = true;
    let stalled = asked_until(&client, "status", |status| status["busy"] == "stalled").await;
    assert_eq!((&stalled["busy"], &stalled["busyReason"]), (&json!("stalled"), &json!("SoundCraft isn't answering")));
    lab.shown.lock().unwrap().late = false;
    assert_eq!(asked_until(&client, "status", |status| status["busy"] == false).await["busy"], false);
    assert!(
        asked_until(&client, "focus", |focus| focus.get("busy").is_none()).await.get("busy").is_none(),
        "no busy at all once it's free"
    );
}

#[tokio::test]
async fn a_new_session_is_told_as_a_project_event_and_the_discovery_file_follows() {
    let lab = Lab::new().await;
    let track =
        |name: &str| json!([{"id": 1, "kind": "audio", "format": "Mono", "name": name, "instrument": null, "inserts": [], "clips": []}]);
    lab.shown.lock().unwrap().tracks = track("Kick");
    let _serving = tokio::spawn(server::serve(any_port(), lab.config(std::process::id()), std::future::pending()));
    let client = lab.connect().await;
    let before = client.request("status", json!({}), WAIT).await.unwrap()["project"]["epoch"].clone();
    let mut events = client.events();
    // Another song: another name, and objects not these.
    {
        let mut shown = lab.shown.lock().unwrap();
        (shown.name, shown.tracks) = ("Next Song".into(), track("Pad"));
    }
    let told = tokio::time::timeout(WAIT, events.recv()).await.unwrap().unwrap();
    assert_eq!((told.name.as_str(), &told.data["name"]), ("project", &json!("Next Song")));
    assert!(told.data["epoch"].is_string() && told.data["epoch"] != before, "a new epoch for its refs: {:?} after {before}", told.data);
    assert_eq!(lab.discovery().await["project"]["name"], "Next Song");
    // Saved under another name, nothing else changed: told, and the same session, its refs kept. (The new session's tracks
    // and selection, told in its epoch, come first.)
    lab.shown.lock().unwrap().name = "Next Song v2".into();
    let saved = loop {
        let told = next_event(&mut events).await;
        if told.name == "project" {
            break told;
        }
    };
    assert_eq!((&saved.data["name"], &saved.data["epoch"]), (&json!("Next Song v2"), &told.data["epoch"]));
}

#[tokio::test]
async fn another_session_under_the_same_name_is_told_as_a_project_event_with_a_new_epoch() {
    let lab = Lab::new().await;
    let audio = json!([{"id": 1, "kind": "audio", "format": "Mono", "name": "Audio 1", "instrument": null, "inserts": [], "clips": []}]);
    {
        let mut shown = lab.shown.lock().unwrap();
        (shown.name, shown.tracks, shown.undo) = ("Untitled".into(), audio.clone(), json!("New Track"));
    }
    let _serving = tokio::spawn(server::serve(any_port(), lab.config(std::process::id()), std::future::pending()));
    let client = lab.connect().await;
    let before = client.request("status", json!({}), WAIT).await.unwrap()["project"]["epoch"].clone();
    let mut events = client.events();
    // File > New: another Untitled, unsaved, with nothing to undo, whose first track is "Audio 1" on id 1 again.
    lab.shown.lock().unwrap().undo = Value::Null;
    let told = tokio::time::timeout(WAIT, events.recv()).await.unwrap().unwrap();
    assert_eq!((told.name.as_str(), &told.data["name"]), ("project", &json!("Untitled")));
    assert!(told.data["epoch"].is_string() && told.data["epoch"] != before, "{:?} after {before}", told.data);
}

/// The next event, within a few seconds.
async fn next_event(events: &mut tokio::sync::broadcast::Receiver<daw_bridge::wire::Event>) -> daw_bridge::wire::Event {
    tokio::time::timeout(WAIT, events.recv()).await.expect("an event").unwrap()
}

/// After a few looks at the app's windows and its session that change nothing, no event has come.
async fn quiet(lab: &Lab, events: &mut tokio::sync::broadcast::Receiver<daw_bridge::wire::Event>) {
    looked(lab, "ui.inspect").await;
    looked(lab, "session.inspect").await;
    looked(lab, "session.inspect").await;
    let told = events.try_recv();
    assert!(matches!(told, Err(tokio::sync::broadcast::error::TryRecvError::Empty)), "nothing changed, yet {told:?}");
}

/// A bridge serving the lab's SoundCraft, a client of it listening for events, and a look at the session since it
/// started (the first look tells nothing).
async fn listening(lab: &Lab) -> (BridgeClient, tokio::sync::broadcast::Receiver<daw_bridge::wire::Event>) {
    tokio::spawn(server::serve(any_port(), lab.config(std::process::id()), std::future::pending()));
    let client = lab.connect().await;
    let mut events = client.events();
    asked_until(&client, "focus", |focus| focus["view"].is_string()).await;
    quiet(lab, &mut events).await;
    (client, events)
}

#[tokio::test]
async fn the_transport_is_told_once_when_soundcraft_plays_records_or_changes_tempo() {
    let lab = Lab::new().await;
    let (_client, mut events) = listening(&lab).await;
    lab.shown.lock().unwrap().playing = true;
    let played = next_event(&mut events).await;
    assert_eq!((played.name.as_str(), &played.data), ("transport", &json!({"playing": true, "recording": false, "bpm": 120.0})));
    quiet(&lab, &mut events).await;
    lab.shown.lock().unwrap().recording = true;
    let take = next_event(&mut events).await;
    assert_eq!((take.name.as_str(), &take.data), ("transport", &json!({"playing": true, "recording": true, "bpm": 120.0})));
    quiet(&lab, &mut events).await;
    lab.shown.lock().unwrap().bpm = 90.0;
    let tempo = next_event(&mut events).await;
    assert_eq!((tempo.name.as_str(), &tempo.data), ("transport", &json!({"playing": true, "recording": true, "bpm": 90.0})));
    quiet(&lab, &mut events).await;
    {
        let mut shown = lab.shown.lock().unwrap();
        (shown.playing, shown.recording) = (false, false);
    }
    let stopped = next_event(&mut events).await;
    assert_eq!((stopped.name.as_str(), &stopped.data), ("transport", &json!({"playing": false, "recording": false, "bpm": 90.0})));
    quiet(&lab, &mut events).await;
}

#[tokio::test]
async fn the_tracks_are_told_once_when_one_comes_or_goes_and_not_when_one_changes() {
    let lab = Lab::new().await;
    lab.shown.lock().unwrap().tracks = kick_and_keys();
    let (client, mut events) = listening(&lab).await;
    let epoch = client.request("status", json!({}), WAIT).await.unwrap()["project"]["epoch"].clone();
    let pad = json!({"id": 8, "kind": "audio", "format": "Stereo", "name": "Pad", "instrument": null, "inserts": [], "clips": []});
    lab.shown.lock().unwrap().tracks.as_array_mut().unwrap().push(pad);
    let more = next_event(&mut events).await;
    assert_eq!((more.name.as_str(), &more.data), ("tracks", &json!({"count": 3, "epoch": epoch})));
    quiet(&lab, &mut events).await;
    // Renamed: the same tracks.
    lab.shown.lock().unwrap().tracks[2]["name"] = json!("Pad 2");
    quiet(&lab, &mut events).await;
    lab.shown.lock().unwrap().tracks.as_array_mut().unwrap().remove(0);
    let fewer = next_event(&mut events).await;
    assert_eq!((fewer.name.as_str(), &fewer.data), ("tracks", &json!({"count": 2, "epoch": epoch})));
    quiet(&lab, &mut events).await;
}

#[tokio::test]
async fn the_selected_tracks_are_told_once_when_they_change() {
    let lab = Lab::new().await;
    lab.shown.lock().unwrap().tracks = kick_and_keys();
    let (_client, mut events) = listening(&lab).await;
    lab.shown.lock().unwrap().selection = json!({"tracks": [5, 3], "clips": [], "start": 0, "end": 0});
    let selected = next_event(&mut events).await;
    assert_eq!(
        (selected.name.as_str(), &selected.data),
        ("selection", &json!({"tracks": [{"guid": "s:track:5", "name": "Keys"}, {"guid": "s:track:3", "name": "Kick"}]}))
    );
    quiet(&lab, &mut events).await;
    // The time selected, the insertion point or a clip moving isn't the tracks selected.
    lab.shown.lock().unwrap().selection = json!({"tracks": [5, 3], "clips": [], "start": 48000, "end": 96000});
    quiet(&lab, &mut events).await;
    lab.shown.lock().unwrap().selection = json!({"tracks": [], "clips": [], "start": 0, "end": 0});
    let none = next_event(&mut events).await;
    assert_eq!((none.name.as_str(), &none.data), ("selection", &json!({"tracks": []})));
    quiet(&lab, &mut events).await;
}

#[tokio::test]
async fn a_take_started_from_a_stop_is_told_recording_from_the_first() {
    let lab = Lab::new().await;
    let (_client, mut events) = listening(&lab).await;
    // Record from a stop, the window's look seeing it play before the session's look sees the take: nothing told on
    // the window's word alone, then the take, recording from the first.
    {
        let mut shown = lab.shown.lock().unwrap();
        (shown.held, shown.playing, shown.recording) = (Some((false, false)), true, true);
    }
    looked(&lab, "ui.inspect").await;
    looked(&lab, "ui.inspect").await;
    lab.shown.lock().unwrap().held = None;
    let take = next_event(&mut events).await;
    assert_eq!((take.name.as_str(), &take.data), ("transport", &json!({"playing": true, "recording": true, "bpm": 120.0})));
    quiet(&lab, &mut events).await;
}

#[tokio::test]
async fn another_session_on_the_same_ids_tells_its_tracks_and_selection_again() {
    let lab = Lab::new().await;
    {
        let mut shown = lab.shown.lock().unwrap();
        (shown.tracks, shown.undo) = (kick_and_keys(), json!("New Track"));
        shown.selection = json!({"tracks": [5], "clips": [], "start": 0, "end": 0});
    }
    let (_client, mut events) = listening(&lab).await;
    // File > New with the same tracks, ids and selection: nothing left to undo, so another session and a new epoch. Its
    // refs read the same, but they're its own: told again in its epoch.
    lab.shown.lock().unwrap().undo = Value::Null;
    let project = next_event(&mut events).await;
    assert_eq!(project.name, "project");
    let tracks = next_event(&mut events).await;
    assert_eq!((tracks.name.as_str(), &tracks.data), ("tracks", &json!({"count": 2, "epoch": project.data["epoch"]})));
    let selection = next_event(&mut events).await;
    assert_eq!((selection.name.as_str(), &selection.data), ("selection", &json!({"tracks": [{"guid": "s:track:5", "name": "Keys"}]})));
    quiet(&lab, &mut events).await;
}

#[tokio::test]
async fn a_new_session_is_told_while_a_read_holds_the_reads() {
    let lab = Lab::new().await;
    lab.shown.lock().unwrap().slow_reads = true;
    let _serving = tokio::spawn(server::serve(any_port(), lab.config(std::process::id()), std::future::pending()));
    let client = lab.connect().await;
    let mut events = client.events();
    // A read the app takes two seconds over holds the reads, and the session switches meanwhile.
    let read = async {
        let answer = client.request("read", json!({"kind": "track"}), WAIT).await;
        (Instant::now(), answer)
    };
    let told = async {
        tokio::time::sleep(ms(100)).await;
        lab.shown.lock().unwrap().name = "Next Song".into();
        let event = tokio::time::timeout(WAIT, events.recv()).await.unwrap().unwrap();
        (Instant::now(), event)
    };
    let ((read_at, answer), (told_at, event)) = tokio::join!(read, told);
    assert!(answer.is_ok(), "{answer:?}");
    assert_eq!((event.name.as_str(), &event.data["name"]), ("project", &json!("Next Song")));
    assert!(told_at < read_at, "told while the read went on, not after it");
}

#[tokio::test]
async fn the_lives_learn_from_the_look_loop_so_a_track_undone_and_made_again_between_two_reads_is_a_new_life() {
    let lab = Lab::new().await;
    let bass = |clips: Value| json!([{"id": 30, "kind": "audio", "format": "Mono", "name": "Bass DI", "instrument": null, "inserts": [], "clips": clips}]);
    lab.shown.lock().unwrap().tracks = bass(json!([{"id": 31, "name": "Bass DI", "start": 0, "length": 48000, "offset": 0, "source": 32}]));
    let _serving = tokio::spawn(server::serve(any_port(), lab.config(std::process::id()), std::future::pending()));
    let client = lab.connect().await;
    let read = client.request("read", json!({"kind": "track"}), WAIT).await.unwrap();
    assert_eq!(read["items"][0]["ref"], "s:track:30");
    // Undone, which a look sees; then another Bass DI, with nothing in it, takes the id: no read between.
    lab.shown.lock().unwrap().tracks = json!([]);
    looked(&lab, "session.inspect").await;
    lab.shown.lock().unwrap().tracks = bass(json!([]));
    looked(&lab, "session.inspect").await;
    let refused = client.request("read", json!({"ref": "s:track:30", "epoch": read["epoch"]}), WAIT).await.unwrap_err();
    let RequestError::Bridge { code, .. } = refused else { panic!("{refused:?}") };
    assert_eq!(code, "stale");
}

#[tokio::test]
async fn when_soundcraft_quits_the_bridge_ends_and_takes_its_discovery_file() {
    let lab = Lab::new().await;
    let mut app = Reaped(
        Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "server::a_process_that_waits", "--ignored", "--test-threads=1"])
            .stdout(Stdio::null())
            .spawn()
            .unwrap(),
    );
    let serving = tokio::spawn(server::serve(any_port(), lab.config(app.id()), std::future::pending()));
    lab.discovery().await;
    lab.fake.quit();
    app.kill().unwrap();
    app.wait().unwrap();
    let ended = tokio::time::timeout(WAIT, serving).await.expect("the bridge ended").unwrap();
    assert_eq!(ended, Ok(End::AppQuit));
    assert!(lab.discovery_files().is_empty());
}

#[test]
#[ignore = "run as SoundCraft's process by when_soundcraft_quits_the_bridge_ends_and_takes_its_discovery_file"]
fn a_process_that_waits() {
    std::thread::sleep(Duration::from_secs(60));
}

/// The bridge program, as Kumi's host starts it: `--kumi-home`, with its stdin a pipe.
fn bridge(lab: &Lab) -> Reaped {
    Reaped(
        Command::new(env!("CARGO_BIN_EXE_soundcraft-kumi"))
            .arg("--kumi-home")
            .arg(lab.home.path())
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap(),
    )
}

/// What the program said on stderr, once it has ended.
fn ended_saying(mut program: Reaped) -> (Option<i32>, String) {
    let mut said = String::new();
    std::io::Read::read_to_string(&mut program.stderr.take().unwrap(), &mut said).unwrap();
    (program.wait().unwrap().code(), said)
}

#[tokio::test]
async fn the_program_serves_one_copy_per_bridge_and_ends_with_its_stdin() {
    let lab = Lab::new().await;
    let (app, port) = lab.soundcraft();
    lab.record(&Launched { port, ..lab.launched(app.id()) });
    let mut first = bridge(&lab);
    lab.discovery().await;
    let (code, said) = ended_saying(bridge(&lab));
    assert_eq!(code, Some(1), "{said}");
    assert!(said.contains("another bridge serves this SoundCraft"), "{said}");
    drop(first.stdin.take());
    let until = Instant::now() + WAIT;
    while first.try_wait().unwrap().is_none() && Instant::now() < until {
        tokio::time::sleep(ms(20)).await;
    }
    assert_eq!(first.try_wait().unwrap().map(|status| status.success()), Some(true), "it ended with its stdin");
    assert!(lab.discovery_files().is_empty());
}

#[tokio::test]
async fn a_bridge_started_as_the_last_one_ends_serves_the_copy() {
    let lab = Lab::new().await;
    let (app, port) = lab.soundcraft();
    lab.record(&Launched { port, ..lab.launched(app.id()) });
    let mut first = bridge(&lab);
    lab.discovery().await;
    // Its host lets go: it removes its file, then says bye for a moment. A host that finds no file starts another at once.
    drop(first.stdin.take());
    let until = Instant::now() + WAIT;
    while !lab.discovery_files().is_empty() && Instant::now() < until {
        tokio::time::sleep(ms(5)).await;
    }
    let mut second = bridge(&lab);
    let served = lab.discovery().await;
    assert_eq!(served["pid"], app.id(), "{served}");
    assert_eq!(second.try_wait().unwrap(), None, "serving, not turned away as another bridge's copy");
    assert_eq!(first.wait().unwrap().code(), Some(0));
}

#[tokio::test]
async fn the_program_ends_by_itself_when_soundcraft_quits_while_its_host_still_holds_its_stdin() {
    let lab = Lab::new().await;
    let (mut app, port) = lab.soundcraft();
    lab.record(&Launched { port, ..lab.launched(app.id()) });
    let mut serving = bridge(&lab);
    lab.discovery().await;
    app.kill().unwrap();
    app.wait().unwrap();
    let until = Instant::now() + WAIT;
    while serving.try_wait().unwrap().is_none() && Instant::now() < until {
        tokio::time::sleep(ms(50)).await;
    }
    let ended = serving.try_wait().unwrap();
    let _held = serving.stdin.take();
    assert_eq!(ended.map(|status| status.success()), Some(true), "ended by itself, its stdin still open");
    let (_, said) = ended_saying(serving);
    assert!(said.contains("SoundCraft quit"), "{said}");
    assert!(lab.discovery_files().is_empty());
}

#[tokio::test]
async fn the_program_with_no_soundcraft_to_serve_says_how_to_open_it() {
    let lab = Lab::new().await;
    let (code, said) = ended_saying(bridge(&lab));
    assert_eq!(code, Some(1));
    assert!(said.contains("SoundCraft isn't running with its control channel: Kumi opens it with `kumi --daw soundcraft`"), "{said}");
}

/// What the installers and Kumi's update run once beside a new bundle (macOS checks a new program at its first launch):
/// it says its version, Kumi's, and ends, reading nothing and needing no home folder.
#[test]
fn the_program_says_its_version() {
    let said =
        Command::new(env!("CARGO_BIN_EXE_soundcraft-kumi")).arg("--version").env_remove("KUMI_HOME").stdin(Stdio::null()).output().unwrap();
    assert!(said.status.success(), "{said:?}");
    assert_eq!(String::from_utf8(said.stdout).unwrap(), format!("soundcraft-kumi {}\n", server::VERSION));
}
