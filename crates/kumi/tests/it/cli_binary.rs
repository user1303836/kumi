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
    assert_eq!(String::from_utf8_lossy(&version.stdout), format!("Kumi {}\n", kumi_runtime::KUMI_VERSION));
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
