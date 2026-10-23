//! Native lifecycle command and retained standalone command name.
use crate::{
    command::CommandOutput,
    delivery::default_package_root,
    lifecycle::{run_lifecycle, LifecycleOptions, LIFECYCLE_ACTIONS},
    live::LiveError,
};
use serde_json::json;
use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
};
const USAGE:&str="usage: ableton-mcp-lifecycle <install|activate|upgrade|repair|rollback|uninstall|status> --remote-scripts-dir ABSOLUTE_PATH [--state-dir ABSOLUTE_PATH] [--package-root ABSOLUTE_PATH] --artifact ABSOLUTE_TARBALL --artifact-sha256 HEX [--config PATH] [--secret PATH] [--host 127.0.0.1|::1] [--port N] [--realtime-port N] [--timeout-ms N] [--apply] [--confirm-live-stopped] [--purge-secret] [--enable-bridge-diagnostics] [--allow-dirty-private-build]";
fn default_state_directory() -> Result<PathBuf, LiveError> {
    if cfg!(windows) {
        let app_data = std::env::var_os("APPDATA")
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
            .ok_or_else(|| LiveError::error("APPDATA is unavailable; provide --state-dir"))?;
        crate::command::resolve(app_data.join("ableton-mcp"))
    } else {
        let home = home::home_dir().ok_or_else(|| LiveError::error("home directory is unavailable; provide --state-dir"))?;
        crate::command::resolve(home.join(".config/ableton-mcp"))
    }
}
fn integer(value: &str, name: &str) -> Result<f64, LiveError> {
    if value.is_empty() || !value.bytes().all(|c| c.is_ascii_digit()) {
        return Err(LiveError::error(format!("{name} requires an integer")));
    }
    let parsed = kumi_common::js::number::parse(value).unwrap_or(f64::INFINITY);
    if !parsed.is_finite() || parsed > 9_007_199_254_740_991. {
        return Err(LiveError::error(format!("{name} is outside the safe integer range")));
    }
    Ok(parsed)
}
pub fn parse(args: &[String]) -> Result<LifecycleOptions, LiveError> {
    let Some(action) = args.first().filter(|action| LIFECYCLE_ACTIONS.contains(&action.as_str())) else {
        return Err(LiveError::error(USAGE));
    };
    let value_options = [
        "--remote-scripts-dir",
        "--state-dir",
        "--package-root",
        "--artifact",
        "--artifact-sha256",
        "--config",
        "--secret",
        "--host",
        "--port",
        "--realtime-port",
        "--timeout-ms",
    ];
    let flag_options =
        ["--apply", "--confirm-live-stopped", "--purge-secret", "--enable-bridge-diagnostics", "--allow-dirty-private-build"];
    let mut values = HashMap::<&str, &str>::new();
    let mut flags = HashSet::new();
    let mut i = 1;
    while let Some(key) = args.get(i) {
        i += 1;
        if values.contains_key(key.as_str()) || flags.contains(key.as_str()) {
            return Err(LiveError::error(format!("duplicate option: {key}")));
        }
        if value_options.contains(&key.as_str()) {
            let Some(value) = args.get(i).filter(|v| !v.is_empty() && !v.starts_with("--")) else {
                return Err(LiveError::error(format!("{key} requires a value")));
            };
            values.insert(key, value);
            i += 1;
        } else if flag_options.contains(&key.as_str()) {
            flags.insert(key.as_str());
        } else {
            return Err(LiveError::error(format!("unknown option: {key}")));
        }
    }
    let Some(remote_scripts) = values.get("--remote-scripts-dir") else {
        return Err(LiveError::error(USAGE));
    };
    let get = |key: &str| values.get(key).copied();
    Ok(LifecycleOptions {
        action: action.clone(),
        package_root: get("--package-root").map(PathBuf::from).unwrap_or_else(default_package_root),
        state_directory: match get("--state-dir") {
            Some(path) => path.into(),
            None => default_state_directory()?,
        },
        remote_scripts_directory: remote_scripts.into(),
        artifact_path: get("--artifact").map(Into::into),
        artifact_sha256: get("--artifact-sha256").map(Into::into),
        config_path: get("--config").map(Into::into),
        secret_path: get("--secret").map(Into::into),
        host: get("--host").map(Into::into),
        port: get("--port").map(|v| integer(v, "--port")).transpose()?,
        realtime_port: get("--realtime-port").map(|v| integer(v, "--realtime-port")).transpose()?,
        timeout_ms: get("--timeout-ms").map(|v| integer(v, "--timeout-ms")).transpose()?,
        apply: flags.contains("--apply"),
        confirm_live_stopped: flags.contains("--confirm-live-stopped"),
        purge_secret: flags.contains("--purge-secret"),
        enable_bridge_diagnostics: flags.contains("--enable-bridge-diagnostics"),
        allow_dirty_private_build: flags.contains("--allow-dirty-private-build"),
        fault_at: None,
    })
}
pub async fn run(args: &[String]) -> CommandOutput {
    let result = match parse(args) {
        Ok(options) => run_lifecycle(&options).await,
        Err(error) => Err(error),
    };
    match result {
        Ok(result) => {
            let mut output = CommandOutput::json(&result);
            if result["state"] == "blocked" || result["state"] == "failed" {
                output.code = 2;
            }
            output
        }
        Err(error) => {
            let redaction = regex::Regex::new(r#"(?:[A-Za-z]:\\|/)[^\s"']+"#).unwrap();
            let reason = redaction.replace_all(error.message(), "<redacted-path>");
            let reason = kumi_common::js::string::head(&reason, 512);
            CommandOutput {
                stdout: String::new(),
                stderr: format!(
                    "{}\n",
                    kumi_common::js::json::stringify(&json!({"version":"ableton-mcp-lifecycle-error/v1","reason":reason}))
                ),
                code: 2,
            }
        }
    }
}
pub fn main() -> i32 {
    let runtime = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
        Ok(runtime) => runtime,
        Err(error) => return CommandOutput::error(error.to_string(), 2).emit(),
    };
    tokio::task::LocalSet::new().block_on(&runtime, run(&kumi_common::env::args().skip(1).collect::<Vec<_>>())).emit()
}
