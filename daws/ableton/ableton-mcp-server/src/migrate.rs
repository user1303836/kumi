use crate::{
    command::{number, resolve, CommandOutput},
    delivery::*,
    live::LiveError,
};
use serde_json::json;
use std::collections::HashMap;
const VALUES: &[&str] = &["--input", "--output", "--bridge-host", "--bridge-port", "--secret-file", "--timeout-ms", "--realtime-port"];
pub fn run(args: &[String]) -> CommandOutput {
    let mut out = CommandOutput::default();
    let mut values = HashMap::<&str, &str>::new();
    let mut force = false;
    let mut i = 0;
    while let Some(option) = args.get(i) {
        i += 1;
        if option == "--force" {
            if force {
                out.report("migration: repeated --force");
            }
            force = true;
            continue;
        }
        if !VALUES.contains(&option.as_str()) {
            out.report(format!("migration: unknown option {option}"));
            continue;
        }
        if values.contains_key(option.as_str()) {
            out.report(format!("migration: repeated {option}"));
        }
        let Some(value) = args.get(i).filter(|v| !v.is_empty() && !v.starts_with("--")) else {
            out.report(format!("migration: {option} requires a value"));
            continue;
        };
        values.insert(option, value);
        i += 1;
    }
    let get = |key: &str| values.get(key).copied();
    let bridge_requested =
        values.keys().any(|key| key.starts_with("--bridge-") || ["--secret-file", "--timeout-ms", "--realtime-port"].contains(key));
    if out.code != 0
        || get("--input").is_none()
        || get("--output").is_none()
        || (bridge_requested && ["--bridge-host", "--bridge-port", "--secret-file"].iter().any(|key| get(key).is_none()))
    {
        out.report("usage: ableton-mcp-migrate --input PATH --output PATH [--force] [--bridge-host 127.0.0.1 --bridge-port N --secret-file ABSOLUTE_PATH [--timeout-ms N] [--realtime-port N]]");
        return out;
    }
    let result = (|| -> Result<CommandOutput, LiveError> {
        let bridge = if bridge_requested {
            let mut bridge = json!({"host":get("--bridge-host"),"port":number(get("--bridge-port")),"secretFile":resolve(get("--secret-file").unwrap())?,"timeoutMs":number(Some(get("--timeout-ms").unwrap_or("5000")))});
            if let Some(port) = get("--realtime-port") {
                bridge["realtimePort"] = json!(number(Some(port)));
            }
            Some(bridge)
        } else {
            None
        };
        let output = resolve(get("--output").unwrap())?;
        let migrated = migrate_config(&resolve(get("--input").unwrap())?, &output, force, bridge.as_ref())?;
        let version = match migrated {
            AnyConfig::Bridge(c) => c.version,
            AnyConfig::Server(c) => c.version,
        };
        Ok(CommandOutput::json(&json!({"migrated":output,"version":version})))
    })();
    result.unwrap_or_else(|error| CommandOutput::error(error.message(), 1))
}
pub fn main() -> i32 {
    run(&kumi_common::env::args().skip(1).collect::<Vec<_>>()).emit()
}
