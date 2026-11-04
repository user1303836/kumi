//! Starting SoundCraft with its channel on, and finding it again. A fake SoundCraft (this test binary, run as a program)
//! prints its port as the app does and answers only with the token it was started with, as SoundCraft does once it
//! checks tokens.
use super::fake::{ok, refused, Answer, Fake};
use soundcraft_kumi::launch::{self, App, Launched, Start};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const TOKEN: &str = "SOUNDCRAFT_CONTROL_TOKEN";

fn app() -> App {
    App { path: PathBuf::from("/lab/SoundCraft.app"), version: Some("0.17.0".into()) }
}

/// The fake app as a start runs it: one of this binary's ignored tests, with what a start gives it.
fn fake_app(test: &str, start: &Start) -> Result<Child, String> {
    let file = |path: &Path| std::fs::OpenOptions::new().append(true).open(path).unwrap();
    Command::new(std::env::current_exe().unwrap())
        .args(["--exact", test, "--ignored", "--nocapture", "--test-threads=1"])
        .env(TOKEN, start.token)
        .stdin(Stdio::null())
        .stdout(file(start.out))
        .stderr(file(start.err))
        .spawn()
        .map_err(|error| error.to_string())
}

/// A start of a fake app that runs until the test kills it, kept in `started`.
fn starting(test: &'static str, started: Arc<Mutex<Option<Child>>>) -> impl FnOnce(&Start) -> Result<Option<u32>, String> {
    move |start: &Start| {
        let child = fake_app(test, start)?;
        let pid = child.id();
        *started.lock().unwrap() = Some(child);
        Ok(Some(pid))
    }
}

/// A start of a fake app that ends by itself, waited for as the launcher's own start waits for the app.
fn ending(test: &'static str) -> impl FnOnce(&Start) -> Result<Option<u32>, String> {
    move |start: &Start| {
        let mut child = fake_app(test, start)?;
        let pid = child.id();
        std::thread::spawn(move || child.wait());
        Ok(Some(pid))
    }
}

struct Ends(Arc<Mutex<Option<Child>>>);

impl Drop for Ends {
    fn drop(&mut self) {
        if let Some(mut child) = self.0.lock().unwrap().take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

#[cfg(unix)]
fn owner_only(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path).unwrap().permissions().mode() & 0o077 == 0
}

#[tokio::test]
async fn a_start_waits_for_the_apps_port_and_token_and_a_later_session_finds_that_copy() {
    let home = tempfile::tempdir().unwrap();
    let folder = home.path().join("bridges").join("soundcraft");
    let started = Arc::new(Mutex::new(None));
    let _ends = Ends(started.clone());
    let launched =
        launch::launch_with(&app(), None, &folder, false, Duration::from_secs(30), starting("launch::a_fake_soundcraft", started.clone()))
            .await
            .unwrap();
    let pid = started.lock().unwrap().as_ref().unwrap().id();
    assert_eq!((launched.pid, launched.version.as_deref(), launched.token.len()), (pid, Some("0.17.0"), 43));
    assert_ne!(launched.port, 0);
    let kept: Launched = serde_json::from_slice(&std::fs::read(folder.join(launch::LAUNCHED)).unwrap()).unwrap();
    assert_eq!(kept, launched);
    #[cfg(unix)]
    assert!(owner_only(&folder.join(launch::LAUNCHED)), "launched.json holds the token");
    // A later session finds the same copy, and its channel answers with that token.
    assert_eq!(launch::attach(&folder), Some(launched.clone()));
    assert!(launched.control().call("ui.inspect", serde_json::json!({}), Duration::from_secs(5)).await.is_ok());
    // Once it has quit, nothing is found.
    let mut child = started.lock().unwrap().take().unwrap();
    child.kill().unwrap();
    child.wait().unwrap();
    assert_eq!(launch::attach(&folder), None);
}

#[tokio::test]
async fn an_app_that_ends_before_it_opens_its_channel_is_told_in_its_own_words_at_once() {
    let home = tempfile::tempdir().unwrap();
    let folder = home.path().join("bridges").join("soundcraft");
    let began = Instant::now();
    let failed = launch::launch_with(
        &app(),
        None,
        &folder,
        false,
        Duration::from_secs(30),
        ending("launch::a_soundcraft_that_cant_open_its_channel"),
    )
    .await
    .unwrap_err();
    assert_eq!(failed, "SoundCraft ended before it opened its control channel: control server failed to bind 127.0.0.1:0: refused");
    assert!(began.elapsed() < Duration::from_secs(20), "not the whole wait: {:?}", began.elapsed());
    assert!(!folder.join(launch::LAUNCHED).exists());
}

#[test]
fn a_session_soundcraft_will_offer_to_recover_is_one_whose_app_has_gone() {
    let settings = tempfile::tempdir().unwrap();
    let autosave = settings.path().join("Autosave");
    let gone = u32::MAX - 7;
    for (folder, holds_session) in [
        (format!("Big Song 1793767041074-{gone}"), true),
        (format!("Big Song 1793767041074-{gone}-2"), true),
        (format!("Empty 1793767041074-{gone}"), false),
        (format!("Open Now 1793767041074-{}", std::process::id()), true),
    ] {
        std::fs::create_dir_all(autosave.join(&folder)).unwrap();
        if holds_session {
            std::fs::write(autosave.join(&folder).join("session.scraft"), b"{}").unwrap();
        }
    }
    assert_eq!(launch::recovery_waiting(settings.path()), ["Big Song"]);
    assert!(launch::recovery_waiting(&settings.path().join("none")).is_empty());
}

#[test]
fn the_settings_folder_is_the_one_soundcraft_config_dir_names() {
    let named = |value: &'static str| move |name: &str| (name == "SOUNDCRAFT_CONFIG_DIR").then(|| value.to_string());
    assert_eq!(launch::settings_folder(&named("/lab/profile")), Some(PathBuf::from("/lab/profile")));
    assert_eq!(launch::settings_folder(&named("")), None, "set empty, SoundCraft keeps no settings");
}

#[test]
fn the_app_kumi_soundcraft_app_names_is_used_when_it_is_there() {
    let folder = tempfile::tempdir().unwrap();
    let program = folder.path().join("soundcraft-somewhere");
    std::fs::write(&program, b"").unwrap();
    let named = |path: PathBuf| move |name: &str| (name == launch::APP_VARIABLE).then(|| path.display().to_string());
    assert_eq!(launch::find(&named(program.clone())).map(|app| app.path), Some(program));
    assert_eq!(launch::find(&named(folder.path().join("missing"))), None);
}

#[tokio::test]
#[ignore = "run as the app by a_start_waits_for_the_apps_port_and_token_and_a_later_session_finds_that_copy"]
async fn a_fake_soundcraft() {
    let token = std::env::var(TOKEN).unwrap();
    let fake = Fake::start(16, move |request| match request["token"].as_str() == Some(token.as_str()) {
        true => Answer::Now(ok(serde_json::json!({"playing": false}))),
        false => Answer::ThenClose(refused("unauthorized: the token is wrong")),
    })
    .await;
    println!("\nSOUNDCRAFT_CONTROL_PORT={}", fake.port());
    std::future::pending::<()>().await;
}

#[test]
#[ignore = "run as the app by an_app_that_ends_before_it_opens_its_channel_is_told_in_its_own_words_at_once"]
fn a_soundcraft_that_cant_open_its_channel() {
    eprintln!("control server failed to bind 127.0.0.1:0: refused");
    std::process::exit(1);
}
