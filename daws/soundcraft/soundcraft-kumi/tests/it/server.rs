//! The bridge's server for Kumi's host, against a fake SoundCraft. The host connects with its own client, through the
//! discovery file. `ping` and `status` answer from the last look, a dialog or an app that doesn't answer makes the bridge
//! busy, and a new session is told as a `project` event. The bridge ends when it's stopped or when SoundCraft quits, and
//! takes its discovery file with it. Run as the program the host starts, it serves one copy of SoundCraft per bridge and
//! ends with its stdin.
use super::fake::{ok, refused, Answer, Fake};
use daw_bridge::auth::Secret;
use daw_bridge::files::ensure_secret;
use daw_bridge::handshake::{ClientHandshake, ClientStep};
use daw_mcp_server::client::{BridgeClient, RequestError};
use serde_json::{json, Value};
use soundcraft_kumi::launch::{Launched, LAUNCHED};
use soundcraft_kumi::server::{self, Config, End};
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

/// What the fake app shows: its session's name, an open dialog, and whether it answers late.
struct Shown {
    name: String,
    dialog: Option<String>,
    late: bool,
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
        let shown = Arc::new(Mutex::new(Shown { name: "Midnight Groove".into(), dialog: None, late: false }));
        let answering = shown.clone();
        let fake = Fake::start(16, move |request| {
            let shown = answering.lock().unwrap();
            match request["method"].as_str() {
                Some("session.inspect") => Answer::Now(ok(json!({"name": shown.name, "path": null}))),
                Some("ui.inspect") if shown.late => Answer::After(ms(400), ok(json!({"dialog": null}))),
                Some("ui.inspect") => Answer::Now(ok(json!({"dialog": shown.dialog}))),
                _ => Answer::Now(refused("unknown method")),
            }
        })
        .await;
        let home = tempfile::tempdir().unwrap();
        let secret = ensure_secret(&home.path().join("bridges/soundcraft/bridge.secret")).unwrap();
        Self { home, fake, shown, secret }
    }

    fn folder(&self) -> PathBuf {
        self.home.path().join("bridges").join("soundcraft")
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
        daw_bridge::owner::write_owner_only(&self.folder().join(LAUNCHED), &serde_json::to_vec(launched).unwrap()).unwrap();
    }

    /// Looks and heartbeats each 100 ms, for a copy whose process is `pid`.
    fn config(&self, pid: u32) -> Config {
        Config {
            look: ms(100),
            heartbeat_ms: 100,
            ..Config::new(self.folder().join("bridge.secret"), self.folder().join("discovery"), self.launched(pid), "/lab/profile".into())
        }
    }

    fn discovery_files(&self) -> Vec<PathBuf> {
        let Ok(entries) = std::fs::read_dir(self.folder().join("discovery")) else { return vec![] };
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
        if !program.exists() && std::fs::hard_link(&this, &program).is_err() {
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

/// Asks for `status` until `until` holds of it (the look loop takes a few looks to see a change).
async fn status_until(client: &BridgeClient, until: impl Fn(&Value) -> bool) -> Value {
    let deadline = Instant::now() + WAIT;
    loop {
        let status = client.request("status", json!({}), WAIT).await.unwrap();
        if until(&status) || Instant::now() > deadline {
            return status;
        }
        tokio::time::sleep(ms(50)).await;
    }
}

#[tokio::test]
async fn the_host_connects_through_the_discovery_file_and_status_answers_from_the_last_look() {
    let lab = Lab::new().await;
    let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
    let serving = tokio::spawn(server::serve(lab.config(std::process::id()), async move {
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
    assert_eq!(offered, (vec!["ping".to_string(), "status".into(), "read".into()], vec!["project".to_string()]));
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
    let _serving = tokio::spawn(server::serve(lab.config(std::process::id()), std::future::pending()));
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
    let _serving = tokio::spawn(server::serve(lab.config(std::process::id()), std::future::pending()));
    let client = lab.connect().await;
    assert!(client.welcome().methods.iter().any(|method| method == "read"), "{:?}", client.welcome().methods);
    // (the lab's SoundCraft shows a session with no tracks)
    let tracks = client.request("read", json!({"kind": "track"}), WAIT).await.unwrap();
    assert_eq!((&tracks["items"], &tracks["total"]), (&json!([]), &json!(0)), "{tracks}");
    assert!(tracks["epoch"].is_string());
    lab.shown.lock().unwrap().dialog = Some("Save Changes".into());
    status_until(&client, |status| status["busy"] == "modal").await;
    let began = Instant::now();
    let refused = client.request("read", json!({"kind": "track"}), WAIT).await.unwrap_err();
    let RequestError::Bridge { code, message } = refused else { panic!("{refused:?}") };
    assert_eq!((code.as_str(), message.as_str()), ("busy", "a dialog is open in SoundCraft (Save Changes); nothing was read"));
    assert!(began.elapsed() < ms(500), "at once: {:?}", began.elapsed());
}

#[tokio::test]
async fn a_dialog_or_an_app_that_does_not_answer_makes_the_bridge_busy_until_it_clears() {
    let lab = Lab::new().await;
    let _serving = tokio::spawn(server::serve(lab.config(std::process::id()), std::future::pending()));
    let client = lab.connect().await;
    lab.shown.lock().unwrap().dialog = Some("Save Changes".into());
    let modal = status_until(&client, |status| status["busy"] == "modal").await;
    assert_eq!((&modal["busy"], &modal["busyReason"]), (&json!("modal"), &json!("a dialog is open in SoundCraft (Save Changes)")));
    lab.shown.lock().unwrap().dialog = None;
    lab.shown.lock().unwrap().late = true;
    let stalled = status_until(&client, |status| status["busy"] == "stalled").await;
    assert_eq!((&stalled["busy"], &stalled["busyReason"]), (&json!("stalled"), &json!("SoundCraft isn't answering")));
    lab.shown.lock().unwrap().late = false;
    assert_eq!(status_until(&client, |status| status["busy"] == false).await["busy"], false);
}

#[tokio::test]
async fn a_new_session_is_told_as_a_project_event_and_the_discovery_file_follows() {
    let lab = Lab::new().await;
    let _serving = tokio::spawn(server::serve(lab.config(std::process::id()), std::future::pending()));
    let client = lab.connect().await;
    let mut events = client.events();
    lab.shown.lock().unwrap().name = "Next Song".into();
    let told = tokio::time::timeout(WAIT, events.recv()).await.unwrap().unwrap();
    assert_eq!((told.name.as_str(), &told.data), ("project", &json!({"name": "Next Song"})));
    assert_eq!(lab.discovery().await["project"]["name"], "Next Song");
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
    let serving = tokio::spawn(server::serve(lab.config(app.id()), std::future::pending()));
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
