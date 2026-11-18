//! Process-level startup checks: exercise the shipped entrypoint and existing data schema.
use std::process::{Command, Output, Stdio};
fn run(home: &std::path::Path, args: &[&str], installed: bool) -> Output {
    command(home, args, installed).output().unwrap()
}
fn command(home: &std::path::Path, args: &[&str], installed: bool) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_kumi"));
    command.args(args).stdin(Stdio::null()).env("KUMI_HOME", home).env("KUMI_NO_UPDATE_CHECK", "1").env("KUMI_UI", "plain");
    for name in [
        "KUMI_MODEL",
        "KUMI_SETTINGS_FILE",
        "KUMI_AUTH_FILE",
        "KUMI_BRIDGE_CONFIG",
        "AI_GATEWAY_API_KEY",
        "OPENAI_API_KEY",
        "ANTHROPIC_API_KEY",
        "OPENCODE_API_KEY",
        "LM_API_TOKEN",
    ] {
        command.env_remove(name);
    }
    if installed {
        command.env("KUMI_INSTALLED", "1");
    } else {
        command.env_remove("KUMI_INSTALLED");
    }
    command
}
#[test]
fn native_entrypoint_serves_version_help_and_errors() {
    let home = tempfile::tempdir().unwrap();
    let version = run(home.path(), &["--version"], false);
    assert!(version.status.success(), "{}", String::from_utf8_lossy(&version.stderr));
    assert_eq!(String::from_utf8_lossy(&version.stdout), format!("Kumi {}\n", kumi::KUMI_VERSION));
    assert!(version.stderr.is_empty());
    for installed in [false, true] {
        let help = run(home.path(), &["--help"], installed);
        assert!(help.status.success());
        assert_eq!(String::from_utf8_lossy(&help.stdout), kumi::cli::help(installed));
        assert!(help.stderr.is_empty());
    }
    let invalid = run(home.path(), &["--unknown"], false);
    assert_eq!(invalid.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&invalid.stderr).starts_with("Kumi:"));
    assert!(!home.path().join("settings.json").exists());
    assert!(!home.path().join("auth.json").exists());
}
#[test]
fn native_process_reads_existing_settings_and_credentials_without_rewriting_them() {
    let home = tempfile::tempdir().unwrap();
    let settings=br#"{"model":"anthropic/claude-sonnet-5-5","effort":"high","libraryFolders":[],"updateCheck":false,"voice":{"language":"ja","send":true}}"#;
    let auth = br#"{"version":1,"credentials":{"anthropic":{"type":"api-key","key":"sk-ant-existing-private-fixture-0000"}}}"#;
    std::fs::write(home.path().join("settings.json"), settings).unwrap();
    let auth_file = home.path().join("auth.json");
    std::fs::write(&auth_file, auth).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&auth_file, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
    let model = run(home.path(), &["model"], true);
    assert!(model.status.success(), "{}", String::from_utf8_lossy(&model.stderr));
    assert!(String::from_utf8_lossy(&model.stdout).contains("anthropic/claude-sonnet-5-5"));
    let providers = run(home.path(), &["auth"], true);
    assert!(providers.status.success(), "{}", String::from_utf8_lossy(&providers.stderr));
    assert!(String::from_utf8_lossy(&providers.stdout).contains("anthropic     API key saved in Kumi"));
    assert!(!String::from_utf8_lossy(&providers.stdout).contains("sk-ant-existing"));
    assert_eq!(std::fs::read(home.path().join("settings.json")).unwrap(), settings);
    assert_eq!(std::fs::read(auth_file).unwrap(), auth);
}
/// A decaying tone, as a 16-bit mono WAV: a hit `kumi design` can read without a decoder.
fn hit(path: &std::path::Path, hz: f64) {
    let (rate, samples) = (44_100u32, 22_050u32);
    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"RIFF");
    bytes.extend_from_slice(&(36 + samples * 2).to_le_bytes());
    bytes.extend_from_slice(b"WAVEfmt ");
    for (value, width) in [(16u32, 4), (1, 2), (1, 2), (rate, 4), (rate * 2, 4), (2, 2), (16, 2)] {
        bytes.extend_from_slice(&value.to_le_bytes()[..width]);
    }
    bytes.extend_from_slice(b"data");
    bytes.extend_from_slice(&(samples * 2).to_le_bytes());
    for n in 0..samples {
        let t = n as f64 / rate as f64;
        bytes.extend_from_slice(&((0.7 * (std::f64::consts::TAU * hz * t).sin() * (-t * 8.0).exp() * 32767.0) as i16).to_le_bytes());
    }
    std::fs::write(path, bytes).unwrap();
}
#[test]
fn kumi_design_renders_a_request_headless_and_refuses_in_json() {
    let home = tempfile::tempdir().unwrap();
    let (low, high) = (home.path().join("Low.wav"), home.path().join("High.wav"));
    hit(&low, 98.0);
    hit(&high, 1500.0);
    let request = serde_json::json!({"op": "fuse", "mode": "loop", "seed": 3, "options": {"bpm": 100, "bars": 1},
        "sources": [{"file": low, "name": "Low"}, {"file": high}]})
    .to_string();
    let render = |out: &std::path::Path| {
        let done = run(home.path(), &["design", "--json", &request, "--out", out.to_str().unwrap()], false);
        assert!(done.status.success(), "{}", String::from_utf8_lossy(&done.stderr));
        serde_json::from_slice::<serde_json::Value>(&done.stdout).unwrap()
    };
    let (first, second) = (home.path().join("one.wav"), home.path().join("two.wav"));
    let answer = render(&first);
    assert_eq!(answer["seed"], 3);
    assert_eq!(answer["duration"], 2.4, "one bar at 100 BPM");
    assert!(answer.get("artifact_id").is_none() && !answer.to_string().contains(home.path().to_str().unwrap()), "no paths come back");
    render(&second);
    assert!(std::fs::read(&first).unwrap() == std::fs::read(&second).unwrap(), "the same request and seed, the same bytes");
    // A refusal is a JSON line too, and nothing is written.
    let bad = serde_json::json!({"op": "stack", "sources": [{"file": low}]}).to_string();
    let none = home.path().join("none.wav");
    let refused = run(home.path(), &["design", "--json", &bad, "--out", none.to_str().unwrap()], false);
    assert_eq!(refused.status.code(), Some(1));
    let said: serde_json::Value = serde_json::from_slice(&refused.stdout).unwrap();
    assert!(said["error"].as_str().unwrap().contains("2 or more"), "{said}");
    assert!(!none.exists());
    assert!(!home.path().join("settings.json").exists(), "no settings read or written");
}

/// The built `kumi` gives its release to the crates under it as it starts (`kumi_types::version::set`), so a provider
/// hears it in the user agent: here the doctor asking a model server on this computer (OLLAMA_HOST), never kumi-types'
/// stand-in for a program that didn't set it.
#[test]
fn the_built_kumi_says_its_release_in_its_user_agent() {
    use std::io::{Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let (heads, seen) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { return };
            let (mut head, mut buffer) = (Vec::new(), [0u8; 4096]);
            while !head.windows(4).any(|end| end == b"\r\n\r\n") {
                match stream.read(&mut buffer) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => head.extend_from_slice(&buffer[..n]),
                }
            }
            let body = br#"{"version":"0.12.0","models":[]}"#;
            let answer =
                format!("HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n", body.len());
            let _ = stream.write_all(answer.as_bytes()).and_then(|()| stream.write_all(body));
            if heads.send(String::from_utf8_lossy(&head).to_ascii_lowercase()).is_err() {
                return;
            }
        }
    });
    let home = tempfile::tempdir().unwrap();
    let mut doctor = command(home.path(), &["doctor"], false);
    doctor.env("OLLAMA_HOST", address.to_string()).env("NO_PROXY", "*");
    for proxy in ["HTTPS_PROXY", "HTTP_PROXY", "ALL_PROXY", "https_proxy", "http_proxy", "all_proxy", "no_proxy"] {
        doctor.env_remove(proxy);
    }
    let ran = doctor.output().unwrap();
    let heads: Vec<String> = seen.try_iter().collect();
    let agent = format!("\r\nuser-agent: kumi/{} (", kumi::KUMI_VERSION);
    assert!(!heads.is_empty(), "the doctor asked the model server nothing: {}", String::from_utf8_lossy(&ran.stdout));
    assert!(heads.iter().all(|head| head.contains(&agent)), "{heads:?}");
    assert!(!heads.iter().any(|head| head.contains(kumi_types::version::UNSET)), "{heads:?}");
}

/// The same release in the bridge's handshake (MCP clientInfo): `kumi serve` connecting to REAPER starts the host beside
/// its own program, here a stand-in beside a link to the built `kumi` that keeps the first line it's sent.
#[cfg(unix)]
#[test]
fn the_built_kumi_says_its_release_in_the_bridges_handshake() {
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir_in(env!("CARGO_TARGET_TMPDIR")).unwrap();
    let (bin, user, transport, state) =
        (dir.path().join("bin"), dir.path().join("user"), dir.path().join("transport"), dir.path().join("plugin/state"));
    for folder in [&bin, &user, &transport, state.parent().unwrap()] {
        std::fs::create_dir_all(folder).unwrap();
    }
    let kumi = bin.join("kumi");
    if std::fs::hard_link(env!("CARGO_BIN_EXE_kumi"), &kumi).is_err() {
        std::fs::copy(env!("CARGO_BIN_EXE_kumi"), &kumi).unwrap();
    }
    let host = bin.join("daw-mcp-server");
    std::fs::write(&host, "#!/bin/sh\nIFS= read -r line\nprintf '%s\\n' \"$line\" > \"${0%/*}/handshake.json\"\n").unwrap();
    std::fs::set_permissions(&host, std::fs::Permissions::from_mode(0o700)).unwrap();
    // Kumi's extension in REAPER, as its receipt in the runtime's Kumi home says.
    let receipt = transport.join("bridges/reaper/install-receipt.json");
    std::fs::create_dir_all(receipt.parent().unwrap()).unwrap();
    let plugins = dir.path().join("REAPER/UserPlugins");
    std::fs::write(
        &receipt,
        serde_json::json!({
            "schema": daw_bridge::lifecycle::RECEIPT_SCHEMA, "daw": "reaper", "status": "installed",
            "target": plugins.join("reaper_kumi.so"), "reference": plugins.join("kumi-bridge-reference.json"),
            "generation": null, "version": null, "sha256": null, "updatedMs": 0,
        })
        .to_string(),
    )
    .unwrap();
    let mut serve = Command::new(&kumi)
        .args(["serve", "--stdio", "--state-dir"])
        .arg(&state)
        .arg("--kumi-home")
        .arg(&transport)
        .args(["--daw", "reaper"])
        .env("HOME", &user)
        .env("KUMI_HOME", user.join(".kumi"))
        .env("KUMI_PROCESSES", "none")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut to = serve.stdin.take().unwrap();
    let (lines, from) = std::sync::mpsc::channel();
    let stdout = serve.stdout.take().unwrap();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            if lines.send(serde_json::from_str::<serde_json::Value>(&line).unwrap()).is_err() {
                return;
            }
        }
    });
    let answer = |to: &mut std::process::ChildStdin, message: serde_json::Value| {
        writeln!(to, "{message}").unwrap();
        let id = message["id"].clone();
        loop {
            let said = from.recv_timeout(std::time::Duration::from_secs(30)).expect("an answer");
            if said["id"] == id {
                return said;
            }
        }
    };
    let initialize =
        serde_json::json!({"protocolVersion": "2025-11-25", "capabilities": {}, "clientInfo": {"name": "test", "version": "1"}});
    answer(&mut to, serde_json::json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": initialize}));
    writeln!(to, "{}", serde_json::json!({"jsonrpc": "2.0", "method": "notifications/initialized"})).unwrap();
    let connect = serde_json::json!({"name": "connect", "arguments": {}});
    let connected = answer(&mut to, serde_json::json!({"jsonrpc": "2.0", "id": 2, "method": "tools/call", "params": connect}));
    drop(to);
    let _ = serve.wait();
    let line = std::fs::read_to_string(bin.join("handshake.json")).unwrap_or_else(|_| panic!("no host started: {connected}"));
    let handshake: serde_json::Value = serde_json::from_str(&line).unwrap();
    assert_eq!(handshake["method"], "initialize", "{handshake}");
    assert_eq!(handshake["params"]["clientInfo"], serde_json::json!({"name": "kumi", "version": kumi::KUMI_VERSION}));
}
