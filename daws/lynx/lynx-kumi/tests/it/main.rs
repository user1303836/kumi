#![cfg(unix)]

mod document;
mod support;

use daw_bridge::{
    handshake::{ClientHandshake, ClientStep},
    wire::{Frame, Request},
};
use lynx_kumi::{
    native,
    project::{epoch, Bridge},
    server,
};
use serde_json::{json, Value};
use std::time::Duration;
use support::Fake;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

#[tokio::test]
async fn native_server_proves_itself_before_any_client_proof_or_payload() {
    let fake = Fake::start(true).await;
    let error = native::Client::new(fake.path()).call("session.inspect", json!({})).await.unwrap_err();
    assert!(error.message.contains("server proof failed"));
    let state = fake.finish().await;
    assert_eq!(state.authenticated, 0);
    assert!(state.calls.is_empty());
    assert_eq!(state.after_bad_challenge, Some(String::new()));
}

#[tokio::test]
async fn native_mutation_has_no_bearer_token_and_numeric_revision_is_canonical() {
    let fake = Fake::start(false).await;
    let mut bridge = Bridge::new(native::Client::new(fake.path()));
    let session = bridge.session().await.unwrap();
    let preview =
        bridge.preview(&json!({"kind":"tempo.set","input":{"bpm":125,"expectedRevision":1.0},"epoch":epoch(&session)})).await.unwrap();
    let args = json!({"kind":"tempo.set","input":{"bpm":125,"expectedRevision":1.0},"fence":preview["fence"],"epoch":preview["epoch"],"desc":"Kumi: Tempo 120 to 125 [k1234]"});
    let first = bridge.apply(&args).await.unwrap();
    let second = bridge.apply(&args).await.unwrap();
    assert_eq!(first, second);
    assert_eq!(first["undo"]["entry"], true);
    assert!(first["display"]["title"].as_str().unwrap().contains("120"));
    let state = fake.finish().await;
    let writes = state.calls.iter().filter(|(op, _)| op == "tempo.set").collect::<Vec<_>>();
    assert_eq!(writes.len(), 1);
    assert_eq!(writes[0].1["expectedRevision"].as_u64(), Some(1));
    assert_eq!(state.session["bpm"], 125);
    assert_eq!(state.tokens_on_wire, 0);
}

#[tokio::test]
async fn lost_write_response_is_uncertain_and_the_same_apply_is_never_sent_again() {
    let fake = Fake::start(false).await;
    let mut bridge = Bridge::new(native::Client::new(fake.path()));
    let session = bridge.session().await.unwrap();
    let input = json!({"bpm":125,"expectedRevision":1});
    let preview = bridge.preview(&json!({"kind":"tempo.set","input":input,"epoch":epoch(&session)})).await.unwrap();
    fake.state.lock().unwrap().drop_mutation_response = true;
    let args = json!({"kind":"tempo.set","input":input,"fence":preview["fence"],"desc":"Kumi: uncertain tempo [k3456]"});
    assert_eq!(bridge.apply(&args).await.unwrap_err().code, "uncertain");
    assert_eq!(bridge.apply(&args).await.unwrap_err().code, "uncertain");
    let state = fake.finish().await;
    assert_eq!(state.session["bpm"], 125);
    assert_eq!(state.calls.iter().filter(|(op, _)| op == "tempo.set").count(), 1);
}

#[tokio::test]
async fn compatible_launch_attaches_and_records_current_project_without_changing_it() {
    let fake = Fake::start(false).await;
    let home = tempfile::tempdir().unwrap();
    let record = lynx_kumi::launch::run(home.path(), &fake.path(), Some(std::path::Path::new("/tmp/a-different-project.lynx")), false)
        .await
        .unwrap();
    assert_eq!(record["started"], false);
    assert_eq!(record["pid"], std::process::id());
    let kept = home.path().join("bridges/lynx/launched.json");
    assert!(daw_bridge::owner::is_owner_only(&kept));
    let content = std::fs::read_to_string(kept).unwrap();
    assert!(!content.contains(&"a".repeat(64)));
    assert!(content.ends_with('\n'));
    assert_eq!(lynx_kumi::launch::serving_path(home.path()).unwrap(), Some(fake.path()));
    let state = fake.finish().await;
    assert_eq!(state.calls.len(), 1);
    assert_eq!(state.calls[0].0, "session.inspect");
}

#[test]
fn connecting_without_native_control_does_not_launch_an_app_or_enable_it() {
    use std::os::unix::fs::PermissionsExt;
    let home = tempfile::tempdir().unwrap();
    let app = home.path().join("LYNX");
    let started = home.path().join("app-started");
    std::fs::write(&app, "#!/bin/sh\n: > \"$LYNX_TEST_STARTED\"\nexit 0\n").unwrap();
    std::fs::set_permissions(&app, std::fs::Permissions::from_mode(0o755)).unwrap();
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_lynx-kumi"))
        .arg("launch")
        .arg("--kumi-home")
        .arg(home.path())
        .arg("--native-config")
        .arg(home.path().join("absent.json"))
        .env("KUMI_LYNX_APP", &app)
        .env("LYNX_TEST_STARTED", &started)
        .env("KUMI_PROCESSES", "none")
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(!started.exists(), "attaching must not start the selected app");
    assert!(!home.path().join("bridges/lynx/launched.json").exists());
}

#[test]
fn companion_refuses_a_shared_or_linked_attachment_record() {
    use std::os::unix::fs::{symlink, PermissionsExt};
    let home = tempfile::tempdir().unwrap();
    let folder = home.path().join("bridges/lynx");
    std::fs::create_dir_all(&folder).unwrap();
    let record = folder.join("launched.json");
    let bytes = serde_json::to_vec(&json!({"schema":"lynx-kumi-launch/1","nativeConfig":"/tmp/lynx/control/connection.json"})).unwrap();
    daw_bridge::owner::write_owner_only(&record, &bytes).unwrap();
    assert_eq!(lynx_kumi::launch::serving_path(home.path()).unwrap(), Some("/tmp/lynx/control/connection.json".into()));
    std::fs::set_permissions(&record, std::fs::Permissions::from_mode(0o644)).unwrap();
    assert!(lynx_kumi::launch::serving_path(home.path()).is_err());
    std::fs::remove_file(&record).unwrap();
    let target = home.path().join("record-target.json");
    daw_bridge::owner::write_owner_only(&target, &bytes).unwrap();
    symlink(target, record).unwrap();
    assert!(lynx_kumi::launch::serving_path(home.path()).is_err());
}

#[tokio::test]
async fn stale_preview_and_producer_edit_both_refuse_writes_or_shared_undo() {
    let fake = Fake::start(false).await;
    let mut bridge = Bridge::new(native::Client::new(fake.path()));
    let session = bridge.session().await.unwrap();
    let stale =
        bridge.preview(&json!({"kind":"tempo.set","input":{"bpm":125,"expectedRevision":0},"epoch":epoch(&session)})).await.unwrap_err();
    assert_eq!(stale.code, "changed");
    let preview =
        bridge.preview(&json!({"kind":"tempo.set","input":{"bpm":125,"expectedRevision":1},"epoch":epoch(&session)})).await.unwrap();
    let desc = "Kumi: Tempo [k1234]";
    let applied = bridge
        .apply(&json!({"kind":"tempo.set","input":{"bpm":125,"expectedRevision":1},"fence":preview["fence"],"desc":desc}))
        .await
        .unwrap();
    {
        let mut state = fake.state.lock().unwrap();
        state.session["revision"] = 3.into();
        state.session["bpm"] = 130.into();
    }
    let refused = bridge
        .restore(&json!({"kind":"tempo.set","prior":preview["prior"],"target":preview["target"],"applied":applied["applied"],"of":desc}))
        .await
        .unwrap_err();
    assert_eq!(refused.code, "changed");
    let state = fake.finish().await;
    assert_eq!(state.session["bpm"], 130);
    assert!(!state.calls.iter().any(|(op, _)| op == "history.undo"));
}

#[tokio::test]
async fn only_top_owned_step_is_undone_and_earlier_owned_step_follows_its_restored_revision() {
    let fake = Fake::start(false).await;
    let mut bridge = Bridge::new(native::Client::new(fake.path()));
    let mut records = Vec::new();
    for bpm in [125, 130] {
        let s = bridge.session().await.unwrap();
        let input = json!({"bpm":bpm,"expectedRevision":s["revision"]});
        let p = bridge.preview(&json!({"kind":"tempo.set","input":input,"epoch":epoch(&s)})).await.unwrap();
        let desc = format!("Kumi: Tempo {bpm} [k{bpm}]");
        let a = bridge.apply(&json!({"kind":"tempo.set","input":input,"fence":p["fence"],"desc":desc})).await.unwrap();
        records.push((p, a, desc));
    }
    let restore = |(p, a, desc): &(Value, Value, String)| json!({"kind":"tempo.set","target":p["target"],"prior":p["prior"],"applied":a["applied"],"of":desc});
    assert_eq!(bridge.restore(&restore(&records[0])).await.unwrap_err().code, "changed");
    bridge.restore(&restore(&records[1])).await.unwrap();
    bridge.restore(&restore(&records[0])).await.unwrap();
    let state = fake.finish().await;
    assert_eq!(state.session["bpm"], 120);
    assert_eq!(state.session["revision"], 5);
    assert_eq!(state.calls.iter().filter(|(op, _)| op == "history.undo").count(), 2);
}

#[tokio::test]
async fn manual_plugin_parameter_change_without_song_revision_blocks_targeted_undo() {
    let fake = Fake::start(false).await;
    let mut bridge = Bridge::new(native::Client::new(fake.path()));
    let session = bridge.session().await.unwrap();
    let input = json!({"bpm":125,"expectedRevision":1});
    let preview = bridge.preview(&json!({"kind":"tempo.set","input":input,"epoch":epoch(&session)})).await.unwrap();
    let desc = "Kumi: tempo [k5678]";
    let applied = bridge.apply(&json!({"kind":"tempo.set","input":input,"fence":preview["fence"],"desc":desc})).await.unwrap();
    {
        let mut state = fake.state.lock().unwrap();
        state.gui_value = 0.9;
        assert_eq!(state.session["revision"], 2);
    }
    let refusal = bridge
        .restore(&json!({"kind":"tempo.set","target":preview["target"],"prior":preview["prior"],"applied":applied["applied"],"of":desc}))
        .await
        .unwrap_err();
    assert_eq!(refusal.code, "changed");
    let state = fake.finish().await;
    assert_eq!(state.gui_value, 0.9);
    assert_eq!(state.session["revision"], 2);
    assert!(!state.calls.iter().any(|(op, _)| op == "history.undo"));
}

#[tokio::test]
async fn undoing_a_later_owned_change_does_not_promote_an_earlier_one_past_a_producer_step() {
    let fake = Fake::start(false).await;
    let mut bridge = Bridge::new(native::Client::new(fake.path()));
    let mut records = Vec::new();
    for bpm in [125, 140] {
        if bpm == 140 {
            let mut state = fake.state.lock().unwrap();
            state.session["revision"] = 3.into();
            state.session["bpm"] = 130.into();
        }
        let session = bridge.session().await.unwrap();
        let input = json!({"bpm":bpm,"expectedRevision":session["revision"]});
        let preview = bridge.preview(&json!({"kind":"tempo.set","input":input,"epoch":epoch(&session)})).await.unwrap();
        let desc = format!("Kumi: tempo {bpm} [k{bpm}]");
        let applied = bridge.apply(&json!({"kind":"tempo.set","input":input,"fence":preview["fence"],"desc":desc})).await.unwrap();
        records.push((preview, applied, desc));
    }
    let restore = |(p, a, d): &(Value, Value, String)| json!({"kind":"tempo.set","target":p["target"],"prior":p["prior"],"applied":a["applied"],"of":d});
    bridge.restore(&restore(&records[1])).await.unwrap();
    assert_eq!(bridge.restore(&restore(&records[0])).await.unwrap_err().code, "changed");
    let state = fake.finish().await;
    assert_eq!(state.session["bpm"], 130);
    assert_eq!(state.session["revision"], 5);
    assert_eq!(state.calls.iter().filter(|(op, _)| op == "history.undo").count(), 1);
}

#[tokio::test]
async fn notes_require_every_current_page_and_ref_alone_reads_are_revision_bound() {
    let fake = Fake::start(false).await;
    let mut bridge = Bridge::new(native::Client::new(fake.path()));
    let s = bridge.session().await.unwrap();
    let input = json!({"index":0,"notes":[],"expectedRevision":1});
    let args = json!({"kind":"pattern.replace_notes","input":input,"epoch":epoch(&s)});
    assert!(bridge.preview(&args).await.unwrap_err().message.contains("every page"));
    let first = bridge.read(&json!({"kind":"note","parent":fixture_ref("pattern", &[0]),"limit":1})).await.unwrap();
    assert!(first.get("nextCursor").is_some());
    assert!(bridge.preview(&args).await.is_err());
    let second = bridge.read(&json!({"ref":fixture_ref("note", &[0,1])})).await.unwrap();
    assert_eq!(second["items"][0]["key"], 64);
    let next = second_cursor(&first, &mut bridge).await;
    let third = bridge.read(&json!({"kind":"note","parent":fixture_ref("pattern", &[0]),"limit":1,"cursor":next})).await.unwrap();
    assert_eq!(third["items"][0]["key"], 67);
    assert!(bridge.preview(&args).await.is_ok());
    let row = bridge.read(&json!({"ref":fixture_ref("track", &[0])})).await.unwrap();
    assert_eq!(row["items"][0]["name"], "Synth");
    {
        fake.state.lock().unwrap().session["revision"] = 2.into();
    }
    assert_eq!(bridge.read(&json!({"ref":fixture_ref("track", &[0])})).await.unwrap_err().code, "changed");
    let now_epoch = { epoch(&fake.state.lock().unwrap().session) };
    assert!(bridge
        .preview(&json!({"kind":"pattern.replace_notes","input":{"index":0,"notes":[],"expectedRevision":2},"epoch":now_epoch}))
        .await
        .is_err());
    fake.finish().await;
}
async fn second_cursor(first: &Value, bridge: &mut Bridge) -> Value {
    let page =
        bridge.read(&json!({"kind":"note","parent":fixture_ref("pattern", &[0]),"limit":1,"cursor":first["nextCursor"]})).await.unwrap();
    page["nextCursor"].clone()
}

#[tokio::test]
async fn bounded_context_and_raw_call_never_silently_cut_an_oversized_pattern() {
    let fake = Fake::start(false).await;
    {
        let mut state = fake.state.lock().unwrap();
        let channel = state.session["channels"][0].clone();
        state.session["channels"] = (0..250)
            .map(|i| {
                let mut row = channel.clone();
                row["index"] = i.into();
                row["name"] = format!("Channel {i}").into();
                row
            })
            .collect::<Vec<_>>()
            .into();
        state.notes[0]["largeField"] = "x".repeat(60 * 1024).into();
    }
    let mut bridge = Bridge::new(native::Client::new(fake.path()));
    let snapshot = bridge.snapshot().await.unwrap();
    assert!(snapshot.to_string().len() < 48 * 1024);
    assert!(snapshot["tracks"].as_array().unwrap().len() <= 50);
    assert_eq!(snapshot["project"]["counts"]["channels"], 250);
    assert!(snapshot["truncated"]["tracks"]["nextCursor"].is_string());
    let error = bridge.read(&json!({"kind":"call","fn":"pattern.notes","args":{"index":0,"offset":0,"limit":1}})).await.unwrap_err();
    assert_eq!(error.code, "limit");
    fake.finish().await;
}

#[tokio::test]
async fn own_wire_authenticates_and_serves_the_same_snapshot_to_the_shared_host() {
    let fake = Fake::start(false).await;
    let home = tempfile::tempdir().unwrap();
    let secret_path = home.path().join("bridge.secret");
    let secret = daw_bridge::files::ensure_secret(&secret_path).unwrap();
    let discovery_dir = home.path().join("discovery");
    let config = server::Config::new(fake.path(), secret_path, discovery_dir.clone());
    let (stop, stopped) = tokio::sync::oneshot::channel();
    let running = tokio::spawn(server::serve(config, async {
        let _ = stopped.await;
    }));
    let file = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if let Ok(mut entries) = std::fs::read_dir(&discovery_dir) {
                if let Some(Ok(entry)) = entries.next() {
                    break entry.path();
                }
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let discovery: Value = serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();
    let stream = tokio::net::TcpStream::connect(("127.0.0.1", discovery["port"].as_u64().unwrap() as u16)).await.unwrap();
    let mut stream = BufReader::new(stream);
    let (mut handshake, hello) = ClientHandshake::start(secret, "lynx-integration-test");
    stream.get_mut().write_all(&hello).await.unwrap();
    let challenge = next_line(&mut stream).await;
    let ClientStep::Send(auth) = handshake.on_line(&challenge).unwrap() else { panic!("challenge") };
    stream.get_mut().write_all(&auth).await.unwrap();
    let welcome = next_line(&mut stream).await;
    let ClientStep::Welcome(welcome) = handshake.on_line(&welcome).unwrap() else { panic!("welcome") };
    assert_eq!(welcome.identity.daw, "lynx");
    assert!(welcome.methods.iter().any(|m| m == "change_restore"));
    stream
        .get_mut()
        .write_all(&Frame::Request(Request { id: 1, method: "snapshot".into(), params: json!({}), deadline_ms: None }).encode())
        .await
        .unwrap();
    loop {
        let line = next_line(&mut stream).await;
        if let Frame::Response(response) = Frame::parse(&line).unwrap() {
            assert!(response.ok);
            let snapshot = response.result.unwrap();
            assert_eq!(snapshot["project"]["name"], "Instrument test");
            assert_eq!(snapshot["tracks"][0]["ref"], fixture_ref("track", &[0]));
            break;
        }
    }
    drop(stream);
    let _ = stop.send(());
    running.await.unwrap().unwrap();
    assert!(!file.exists());
    fake.finish().await;
}
async fn next_line(stream: &mut BufReader<tokio::net::TcpStream>) -> String {
    let mut line = String::new();
    stream.read_line(&mut line).await.unwrap();
    assert!(!line.is_empty());
    line
}

#[cfg(unix)]
#[test]
fn private_discovery_refuses_legacy_protocol_dead_pid_symlinks_and_shared_mode() {
    use std::{
        io::Write,
        os::unix::fs::{symlink, PermissionsExt},
    };
    let mut config = tempfile::NamedTempFile::new().unwrap();
    let text = json!({"protocol":"lynx-control/1","host":"127.0.0.1","port":12345,"pid":std::process::id(),"token":"a".repeat(64)});
    write!(config, "{text}").unwrap();
    assert!(native::discovery(config.path()).is_ok());
    std::fs::set_permissions(config.path(), std::fs::Permissions::from_mode(0o644)).unwrap();
    assert!(native::discovery(config.path()).is_err());
    std::fs::set_permissions(config.path(), std::fs::Permissions::from_mode(0o600)).unwrap();
    let directory = tempfile::tempdir().unwrap();
    let linked = directory.path().join("linked.json");
    symlink(config.path(), &linked).unwrap();
    assert!(native::discovery(&linked).is_err());
    let mut legacy = text.clone();
    legacy["protocol"] = "lynx-kumi/1".into();
    std::fs::write(config.path(), legacy.to_string()).unwrap();
    assert!(native::discovery(config.path()).is_err());
    let mut dead = text;
    dead["pid"] = u32::MAX.into();
    std::fs::write(config.path(), dead.to_string()).unwrap();
    assert!(native::discovery(config.path()).is_err());
}

fn fixture_ref(kind: &str, parts: &[u64]) -> String {
    lynx_kumi::project::reference(
        kind,
        parts,
        &json!({"revision":1,"controlEpoch":"b".repeat(64),"project":{"id":"1234567890abcdef1234567890abcdef"}}),
    )
}
