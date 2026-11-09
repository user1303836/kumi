//! Process-level startup checks: exercise the shipped entrypoint and existing data schema.
use std::process::{Command, Output, Stdio};
fn run(home: &std::path::Path, args: &[&str], installed: bool) -> Output {
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
    command.output().unwrap()
}
#[test]
fn native_entrypoint_serves_version_help_and_errors() {
    let home = tempfile::tempdir().unwrap();
    let version = run(home.path(), &["--version"], false);
    assert!(version.status.success(), "{}", String::from_utf8_lossy(&version.stderr));
    assert_eq!(String::from_utf8_lossy(&version.stdout), format!("Kumi {}\n", kumi_types::version::KUMI_VERSION));
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
