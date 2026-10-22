use ableton_mcp_server::host::SERVER_VERSION;
use serde_json::Value;
use std::io::Write;
use std::process::{Command, Stdio};

#[test]
fn normal_stdio_startup_preserves_protocol_and_truthfully_reports_disconnected_live() {
    let dir = tempfile::tempdir().unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_ableton-mcp-server"))
        .current_dir(dir.path())
        .env_remove("ABLETON_MCP_TOOL_POLICY")
        .env_remove("ABLETON_MCP_TOOL_ALLOW")
        .env_remove("ABLETON_MCP_TOOL_DENY")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let requests = [
        serde_json::json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"process-test","version":"1"}}}),
        serde_json::json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
        serde_json::json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"live_status","arguments":{}}}),
    ];
    let mut input = child.stdin.take().unwrap();
    for request in requests {
        writeln!(input, "{request}").unwrap();
    }
    drop(input);
    let out = child.wait_with_output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert!(out.stderr.is_empty());
    let lines: Vec<Value> = String::from_utf8(out.stdout).unwrap().lines().map(|s| serde_json::from_str(s).unwrap()).collect();
    assert_eq!(lines.len(), 2);
    assert_eq!(lines.iter().find(|r| r["id"] == 1).unwrap()["result"]["serverInfo"]["version"], SERVER_VERSION);
    let status: Value =
        serde_json::from_str(lines.iter().find(|r| r["id"] == 2).unwrap()["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(status["connected"], false);
    assert_eq!(status["adapter"], "unavailable");
    assert_ne!(status["provenance"], "real-live");
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
}

#[test]
fn normal_stdio_cli_rejects_options_like_the_source_before_reading_files() {
    for (args, message) in [
        (vec!["--config"], "unknown option"),
        (vec!["--config", ""], "--config requires a path"),
        (vec!["--config", "x", "--config", "y"], "repeated --config"),
        (vec!["--unknown"], "unknown option"),
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_ableton-mcp-server")).args(args).output().unwrap();
        assert_eq!(output.status.code(), Some(2));
        assert!(output.stdout.is_empty());
        assert_eq!(String::from_utf8(output.stderr).unwrap(), format!("mcp-host: {message}\n"));
    }
}
#[test]
fn native_metadata_probe_uses_no_configuration_or_input() {
    let dir = tempfile::tempdir().unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_ableton-mcp-server"))
        .arg("--version")
        .current_dir(dir.path())
        .env("ABLETON_MCP_EXTENSION", "external")
        .env("ABLETON_MCP_EXTENSION_DIR", dir.path().join("absent"))
        .stdin(Stdio::piped())
        .output()
        .unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    assert_eq!(String::from_utf8(output.stdout).unwrap(), format!("ableton-mcp-server {SERVER_VERSION}\n"));
    assert!(output.stderr.is_empty());
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
}
#[tokio::test]
async fn delivery_subcommands_preserve_their_existing_cli_parsers() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let args = vec!["--unknown".into()];
            let cases = [
                ("setup", ableton_mcp_server::setup::run(&args)),
                ("migrate", ableton_mcp_server::migrate::run(&args)),
                ("install-remote-script", ableton_mcp_server::install_remote_script::run(&args)),
                ("diagnostics", ableton_mcp_server::diagnostics::run(&args).await),
                ("lifecycle", ableton_mcp_server::lifecycle_cli::run(&args).await),
            ];
            for (name, expected) in cases {
                let actual = ableton_mcp_server::cli::auxiliary_command(&[name.into(), "--unknown".into()]).await.unwrap();
                assert_eq!(actual, expected, "{name}");
                assert_ne!(actual.code, 0);
            }
            assert!(ableton_mcp_server::cli::auxiliary_command(&[]).await.is_none());
            assert!(ableton_mcp_server::cli::auxiliary_command(&["--version".into(), "--config".into()]).await.is_none());
            let metadata = ableton_mcp_server::cli::auxiliary_command(&["--version".into()]).await.unwrap();
            assert_eq!(metadata.code, 0);
            assert!(serde_json::from_str::<Value>(&metadata.stdout).is_err());
        })
        .await;
}
