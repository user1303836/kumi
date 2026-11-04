//! Starting SoundCraft with its channel on, and finding it again. A fake SoundCraft (this test binary under SoundCraft's
//! program name, run as a program) prints its port as the app does and answers only with the token it was started with,
//! as SoundCraft does once it checks tokens.
use super::fake::{ok, refused, Answer, Fake};
use daw_bridge::daw::Daw;
use daw_bridge::detect::{self, SoundCraftApp};
use daw_bridge::files::Launched;
use daw_bridge::paths::BridgePaths;
use soundcraft_kumi::launch::{self, Found, Start, Waits};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

const TOKEN: &str = "SOUNDCRAFT_CONTROL_TOKEN";
const SECOND: Duration = Duration::from_secs(1);
const USUAL: Waits = Waits::USUAL;

fn ms(n: u64) -> Duration {
    Duration::from_millis(n)
}

/// This test binary under SoundCraft's program name: a look for the copy checks the program its pid runs. On macOS a copy
/// (a clone, on APFS) rather than a hard link: macOS names a hard-linked program's process by whichever of its names it
/// looked up last.
fn soundcraft_in(dir: &Path) -> SoundCraftApp {
    let program = dir.join(if cfg!(windows) { "soundcraft.exe" } else { "soundcraft" });
    let this = std::env::current_exe().unwrap();
    if cfg!(target_os = "macos") || std::fs::hard_link(&this, &program).is_err() {
        std::fs::copy(&this, &program).unwrap();
    }
    SoundCraftApp { path: program, version: Some("0.17.0".into()) }
}

/// What the bridge's launch record says, as the bridge reads it.
fn recorded(paths: &BridgePaths) -> Option<Launched> {
    Launched::read(&paths.launch_record())
}

/// The fake app as a start runs it: one of this binary's ignored tests, with what a start gives it.
fn fake_app(test: &str, start: &Start) -> Result<Child, String> {
    let file = |path: &Path| std::fs::OpenOptions::new().append(true).open(path).unwrap();
    Command::new(&start.app.path)
        .args(["--exact", test, "--ignored", "--nocapture", "--test-threads=1"])
        .env(TOKEN, start.token)
        .stdin(Stdio::null())
        .stdout(file(start.out))
        .stderr(file(start.err))
        .spawn()
        .map_err(|error| error.to_string())
}

/// A start of a fake app that runs until the test kills it: the launcher gets its process, and `started` its pid.
fn starting(test: &'static str, started: Arc<Mutex<Option<u32>>>) -> impl FnOnce(&Start) -> Result<Option<Child>, String> {
    move |start: &Start| {
        let child = fake_app(test, start)?;
        *started.lock().unwrap() = Some(child.id());
        Ok(Some(child))
    }
}

/// A start of a fake app that ends by itself.
fn ending(test: &'static str) -> impl FnOnce(&Start) -> Result<Option<Child>, String> {
    move |start: &Start| fake_app(test, start).map(Some)
}

/// Kills a process that still runs (a copy the launcher kept, which a thread of its own waits for).
fn kill(pid: u32) {
    if !daw_bridge::files::process_alive(pid) {
        return;
    }
    #[cfg(unix)]
    // SAFETY: sends a signal to the test's own child.
    unsafe {
        libc::kill(pid as libc::pid_t, libc::SIGKILL);
    }
    #[cfg(windows)]
    let _ = Command::new("taskkill").args(["/F", "/PID", &pid.to_string()]).stdout(Stdio::null()).stderr(Stdio::null()).status();
}

/// Ends the started app with the test, however the test ends, unless the launcher closed it already.
struct Ends(Arc<Mutex<Option<u32>>>);

impl Drop for Ends {
    fn drop(&mut self) {
        if let Some(pid) = self.0.lock().unwrap().take() {
            kill(pid);
        }
    }
}

/// Whether the started app has ended within `wait` (the launcher closes a copy through its process, and waits for it).
fn ended(started: &Arc<Mutex<Option<u32>>>, wait: Duration) -> bool {
    let pid = started.lock().unwrap().expect("started");
    let began = Instant::now();
    while began.elapsed() < wait {
        if !daw_bridge::files::process_alive(pid) {
            started.lock().unwrap().take();
            return true;
        }
        std::thread::sleep(ms(25));
    }
    false
}

#[cfg(unix)]
fn owner_only(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path).unwrap().permissions().mode() & 0o077 == 0
}

#[tokio::test]
async fn a_start_waits_for_the_apps_port_and_token_and_a_later_session_finds_that_copy() {
    let home = tempfile::tempdir().unwrap();
    let paths = BridgePaths::new(home.path(), Daw::SoundCraft);
    let app = soundcraft_in(home.path());
    let started = Arc::new(Mutex::new(None));
    let _ends = Ends(started.clone());
    let found =
        launch::launch_with(&app, None, &paths, false, USUAL, starting("launch::a_fake_soundcraft", started.clone())).await.unwrap();
    let pid = started.lock().unwrap().unwrap();
    let launched = found.launched.clone();
    assert!(found.answering);
    assert_eq!((launched.pid, launched.version.as_deref(), launched.token.len()), (pid, Some("0.17.0"), 43));
    assert_ne!(launched.port, 0);
    assert_eq!(recorded(&paths), Some(launched.clone()));
    #[cfg(unix)]
    assert!(owner_only(&paths.launch_record()), "the launch record holds the token");
    assert!(!format!("{launched:?}").contains(&launched.token), "its Debug leaves the token out");
    // A later session finds the same copy, and its channel answers with that token.
    assert_eq!(launch::attach(&paths, SECOND).await, Some(Found { launched: launched.clone(), answering: true }));
    // Once it has quit, nothing is found.
    kill(pid);
    let until = Instant::now() + Duration::from_secs(5);
    while daw_bridge::files::process_alive(pid) && Instant::now() < until {
        tokio::time::sleep(ms(20)).await;
    }
    assert_eq!(launch::attach(&paths, SECOND).await, None);
}

#[tokio::test]
async fn a_copy_still_opening_its_session_is_kumis_busy_copy_kept_and_found_again() {
    let home = tempfile::tempdir().unwrap();
    let paths = BridgePaths::new(home.path(), Daw::SoundCraft);
    let app = soundcraft_in(home.path());
    let started = Arc::new(Mutex::new(None));
    let _ends = Ends(started.clone());
    let waits = Waits { port: Duration::from_secs(30), answer: ms(300) };
    let opening = starting("launch::a_fake_soundcraft_opening_a_big_session", started.clone());
    let found = launch::launch_with(&app, None, &paths, false, waits, opening).await.unwrap();
    assert!(!found.answering, "its window takes no request while it opens the session");
    assert_eq!(recorded(&paths), Some(found.launched.clone()), "kept all the same");
    assert_eq!(launch::attach(&paths, ms(300)).await.map(|found| found.answering), Some(false), "found again, busy");
    // Once the session is open, the same copy answers.
    let began = Instant::now();
    let mut answering = false;
    while !answering && began.elapsed() < Duration::from_secs(10) {
        answering = launch::attach(&paths, ms(500)).await.is_some_and(|again| again.answering && again.launched == found.launched);
    }
    assert!(answering);
}

#[tokio::test]
async fn an_app_that_ends_before_it_opens_its_channel_is_told_in_its_own_words_at_once() {
    let home = tempfile::tempdir().unwrap();
    let paths = BridgePaths::new(home.path(), Daw::SoundCraft);
    let app = soundcraft_in(home.path());
    let began = Instant::now();
    let failed =
        launch::launch_with(&app, None, &paths, false, USUAL, ending("launch::a_soundcraft_that_cant_open_its_channel")).await.unwrap_err();
    assert_eq!(failed, "SoundCraft ended before it opened its control channel: control server failed to bind 127.0.0.1:0: refused");
    assert!(began.elapsed() < Duration::from_secs(20), "not the whole wait: {:?}", began.elapsed());
    assert_eq!(recorded(&paths), None);
}

#[tokio::test]
async fn a_copy_that_doesnt_open_its_channel_in_time_is_closed_and_not_kept() {
    let home = tempfile::tempdir().unwrap();
    let paths = BridgePaths::new(home.path(), Daw::SoundCraft);
    let app = soundcraft_in(home.path());
    let started = Arc::new(Mutex::new(None));
    let _ends = Ends(started.clone());
    let waits = Waits { port: SECOND, answer: SECOND };
    let silent = starting("launch::a_soundcraft_that_never_opens_its_channel", started.clone());
    let failed = launch::launch_with(&app, None, &paths, false, waits, silent).await.unwrap_err();
    assert_eq!(failed, "SoundCraft didn't open its control channel within 1 s, so Kumi closed the copy it started: a dialog waits");
    assert!(ended(&started, Duration::from_secs(10)), "closed, so it doesn't stand in the next start's way");
    assert_eq!(recorded(&paths), None);
}

#[tokio::test]
async fn a_copy_whose_channel_wont_take_kumis_token_is_closed_and_not_kept() {
    let home = tempfile::tempdir().unwrap();
    let paths = BridgePaths::new(home.path(), Daw::SoundCraft);
    let app = soundcraft_in(home.path());
    let started = Arc::new(Mutex::new(None));
    let _ends = Ends(started.clone());
    let refusing = starting("launch::a_soundcraft_that_wants_another_token", started.clone());
    let failed = launch::launch_with(&app, None, &paths, false, USUAL, refusing).await.unwrap_err();
    assert!(failed.contains("(unauthorized: the token is wrong)") && failed.ends_with("so Kumi closed the copy it started"), "{failed}");
    assert!(ended(&started, Duration::from_secs(10)), "closed");
    assert_eq!(recorded(&paths), None);
}

#[tokio::test]
async fn a_copy_that_answers_in_its_own_words_without_kumis_first_request_is_kept() {
    let home = tempfile::tempdir().unwrap();
    let paths = BridgePaths::new(home.path(), Daw::SoundCraft);
    let app = soundcraft_in(home.path());
    let started = Arc::new(Mutex::new(None));
    let _ends = Ends(started.clone());
    let without = starting("launch::a_soundcraft_without_ui_inspect", started.clone());
    let found = launch::launch_with(&app, None, &paths, false, USUAL, without).await.unwrap();
    assert!(found.answering, "it took the token and answered: `unknown method` is its own word");
    assert_eq!(recorded(&paths), Some(found.launched));
    assert!(daw_bridge::files::process_alive(started.lock().unwrap().unwrap()), "kept running");
}

/// macOS's `open` says nothing of the app it started: the app is the process holding its stdout's file.
#[cfg(target_os = "macos")]
#[tokio::test]
async fn where_the_start_cant_say_the_apps_pid_it_is_the_process_holding_the_apps_stdout() {
    let home = tempfile::tempdir().unwrap();
    let app = soundcraft_in(home.path());
    let started = Arc::new(Mutex::new(None));
    let _ends = Ends(started.clone());
    let keep = started.clone();
    let unsaid = move |start: &Start| {
        *keep.lock().unwrap() = Some(fake_app("launch::a_fake_soundcraft", start)?.id());
        Ok(None)
    };
    let found =
        launch::launch_with(&app, None, &BridgePaths::new(&home.path().join("one"), Daw::SoundCraft), false, USUAL, unsaid).await.unwrap();
    assert_eq!(Some(found.launched.pid), *started.lock().unwrap(), "the port's listener, which holds the app's stdout");
    // One that ends at once is told well before the whole wait.
    let began = Instant::now();
    let ends_at_once = |start: &Start| {
        fake_app("launch::a_soundcraft_that_cant_open_its_channel", start)?.wait().map_err(|error| error.to_string())?;
        Ok(None)
    };
    let failed = launch::launch_with(&app, None, &BridgePaths::new(&home.path().join("two"), Daw::SoundCraft), false, USUAL, ends_at_once)
        .await
        .unwrap_err();
    assert!(failed.starts_with("SoundCraft ended before it opened its control channel"), "{failed}");
    assert!(began.elapsed() < Duration::from_secs(15), "{:?}", began.elapsed());
}

#[test]
fn a_second_look_then_start_waits_for_the_first() {
    let home = tempfile::tempdir().unwrap();
    let paths = BridgePaths::new(home.path(), Daw::SoundCraft);
    let first = launch::lock(&paths).unwrap();
    let (said, heard) = std::sync::mpsc::channel();
    let second = paths.clone();
    let waiting = std::thread::spawn(move || {
        let _held = launch::lock(&second).unwrap();
        said.send(()).unwrap();
    });
    assert!(heard.recv_timeout(ms(300)).is_err(), "waits while the first holds the lock");
    drop(first);
    assert!(heard.recv_timeout(Duration::from_secs(5)).is_ok(), "then goes on");
    waiting.join().unwrap();
}

#[test]
fn soundcraft_opened_another_way_is_named_and_nothing_starts() {
    let home = tempfile::tempdir().unwrap();
    let me = std::process::id();
    let run = || {
        Command::new(env!("CARGO_BIN_EXE_soundcraft-kumi"))
            .args(["launch", "--kumi-home"])
            .arg(home.path())
            .env("KUMI_PROCESSES", format!("{me} /Applications/SoundCraft.app/Contents/MacOS/SoundCraft"))
            .env(detect::SOUNDCRAFT_APP, home.path().join("no-app-here"))
            .output()
            .unwrap()
    };
    let said = run();
    let words = String::from_utf8_lossy(&said.stderr).into_owned();
    assert_eq!(said.status.code(), Some(1), "{words}");
    assert!(words.contains(&format!("SoundCraft is open without its control channel (pid {me})")), "{words}");
    assert!(said.stdout.is_empty());
    let paths = BridgePaths::new(home.path(), Daw::SoundCraft);
    assert_eq!(recorded(&paths), None, "nothing started");
    // The copy Kumi itself started, once it no longer takes Kumi's requests, is named as Kumi's.
    let ours = Launched { pid: me, started_ms: 1, port: 9, token: "kept".into(), app: PathBuf::from("/lab/SoundCraft.app"), version: None };
    daw_bridge::owner::write_owner_only(&paths.launch_record(), &serde_json::to_vec(&ours).unwrap()).unwrap();
    let words = String::from_utf8_lossy(&run().stderr).into_owned();
    assert!(words.contains(&format!("The SoundCraft Kumi started (pid {me}) no longer takes Kumi's requests")), "{words}");
}

/// Sets a file's modification time.
fn modified(path: &Path, at: SystemTime) {
    std::fs::File::options().write(true).open(path).unwrap().set_modified(at).unwrap();
}

#[test]
fn the_sessions_soundcraft_will_offer_to_recover_are_the_ones_its_own_rule_keeps() {
    let settings = tempfile::tempdir().unwrap();
    let autosave = settings.path().join("Autosave");
    let now = SystemTime::now();
    let hour = Duration::from_secs(3600);
    let (older, newer) = (settings.path().join("older.scraft"), settings.path().join("newer.scraft"));
    for (file, at) in [(&older, now - hour), (&newer, now + hour)] {
        std::fs::write(file, b"{}").unwrap();
        modified(file, at);
    }
    let gone = u32::MAX - 7;
    let folders: [(&str, u32, &[&str], Option<&Path>); 8] = [
        ("Unsaved Song", gone, &["lock", "session.scraft"], Some(&older)),
        ("Saved Song", gone, &["lock", "session.scraft"], Some(&newer)),
        ("Never Saved", gone, &["lock", "session.scraft"], None),
        ("Moved Away", gone, &["lock", "session.scraft"], Some(Path::new("/nowhere/moved.scraft"))),
        ("Recording", gone, &["lock", "session.scraft", "recording.json"], Some(&newer)),
        ("No Lock", gone, &["session.scraft"], None),
        ("Empty", gone, &["lock"], None),
        ("Open Now", std::process::id(), &["lock", "session.scraft"], None),
    ];
    for (session, pid, files, origin) in folders {
        let folder = autosave.join(format!("{session} 1793767041074-{pid}"));
        std::fs::create_dir_all(&folder).unwrap();
        for file in files {
            std::fs::write(folder.join(file), b"{}").unwrap();
        }
        if let Some(origin) = origin {
            std::fs::write(folder.join("origin"), origin.to_string_lossy().as_bytes()).unwrap();
        }
    }
    assert_eq!(launch::recovery_waiting(settings.path()), ["Moved Away", "Never Saved", "Recording", "Unsaved Song"]);
    assert!(launch::recovery_waiting(&settings.path().join("none")).is_empty());
}

#[test]
fn the_app_kumi_soundcraft_app_names_is_used_when_it_is_there() {
    let folder = tempfile::tempdir().unwrap();
    let program = folder.path().join("soundcraft-somewhere");
    std::fs::write(&program, b"").unwrap();
    let named = |path: PathBuf| move |name: &str| (name == detect::SOUNDCRAFT_APP).then(|| path.display().to_string());
    assert_eq!(launch::find(&named(program.clone())).map(|app| app.path), Some(program));
    assert_eq!(launch::find(&named(folder.path().join("missing"))), None);
}

/// Answers a request with the start's token, and refuses any other, as SoundCraft does once it checks tokens.
fn answer_with(token: &str, request: &serde_json::Value) -> serde_json::Value {
    match request["token"].as_str() == Some(token) {
        true => ok(serde_json::json!({"playing": false})),
        false => refused("unauthorized: the token is wrong"),
    }
}

#[tokio::test]
#[ignore = "run as the app by the tests above"]
async fn a_fake_soundcraft() {
    let token = std::env::var(TOKEN).unwrap();
    let fake = Fake::start(16, move |request| match answer_with(&token, request) {
        answer if answer["ok"] == true => Answer::Now(answer),
        refusal => Answer::ThenClose(refusal),
    })
    .await;
    println!("\nSOUNDCRAFT_CONTROL_PORT={}", fake.port());
    std::future::pending::<()>().await;
}

#[tokio::test]
#[ignore = "run as the app by a_copy_still_opening_its_session_is_kumis_busy_copy_kept_and_found_again"]
async fn a_fake_soundcraft_opening_a_big_session() {
    let token = std::env::var(TOKEN).unwrap();
    // (the window takes requests once the session is open, three seconds on)
    let open = Instant::now() + Duration::from_secs(3);
    let fake =
        Fake::start(16, move |request| Answer::After(open.saturating_duration_since(Instant::now()), answer_with(&token, request))).await;
    println!("\nSOUNDCRAFT_CONTROL_PORT={}", fake.port());
    std::future::pending::<()>().await;
}

#[tokio::test]
#[ignore = "run as the app by a_copy_whose_channel_wont_take_kumis_token_is_closed_and_not_kept"]
async fn a_soundcraft_that_wants_another_token() {
    let fake = Fake::start(16, |request| Answer::ThenClose(answer_with("another", request))).await;
    println!("\nSOUNDCRAFT_CONTROL_PORT={}", fake.port());
    std::future::pending::<()>().await;
}

#[test]
#[ignore = "run as the app by a_copy_that_doesnt_open_its_channel_in_time_is_closed_and_not_kept"]
fn a_soundcraft_that_never_opens_its_channel() {
    eprintln!("a dialog waits");
    std::thread::sleep(Duration::from_secs(120));
}

#[test]
#[ignore = "run as the app by the tests of an app that ends at once"]
fn a_soundcraft_that_cant_open_its_channel() {
    eprintln!("control server failed to bind 127.0.0.1:0: refused");
    std::process::exit(1);
}

#[tokio::test]
#[ignore = "run as the app by a_copy_that_answers_in_its_own_words_without_kumis_first_request_is_kept"]
async fn a_soundcraft_without_ui_inspect() {
    let fake = Fake::start(16, |_| Answer::Now(refused("unknown method `ui.inspect`"))).await;
    println!("\nSOUNDCRAFT_CONTROL_PORT={}", fake.port());
    std::future::pending::<()>().await;
}
